//! Smoke test di Fase 2: flusso completo delle operazioni su file verso il
//! thread di lavoro e aggiornamento dell'indice SQLite, replicando la
//! pipeline del pump della UI (richiesta → esito → `move_path` → riallineamento).

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use uberphoto_db::Index;
use uberphoto_files::{
    scan_directory, ConflictPolicy, FileOpOutcome, FileOpRequest, FileOpsService,
};

fn attendi_esito(service: &FileOpsService) -> FileOpOutcome {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Some(outcome) = service.try_recv() {
            return outcome;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("esito dell'operazione non ricevuto entro 10 secondi");
}

fn prepara_cartelle(temp: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let sorgente = temp.join("sorgente");
    let destinazione = temp.join("destinazione");
    fs::create_dir_all(&sorgente).unwrap();
    fs::create_dir_all(&destinazione).unwrap();
    (sorgente, destinazione)
}

fn indice_della_cartella(sorgente: &Path) -> Index {
    let index = Index::open_in_memory().unwrap();
    for entry in scan_directory(sorgente, false).unwrap() {
        index.upsert_file_info(&entry).unwrap();
    }
    index
}

#[test]
fn spostamento_aggiorna_il_file_e_l_indice() {
    let temp = tempfile::tempdir().unwrap();
    let (sorgente, destinazione) = prepara_cartelle(temp.path());
    fs::write(sorgente.join("foto.jpg"), b"contenuto jpeg").unwrap();
    fs::write(sorgente.join("foto.jpg.xmp"), b"<xmp/>").unwrap();

    let index = indice_della_cartella(&sorgente);
    let foto = sorgente.join("foto.jpg");
    assert!(index.get(&foto).unwrap().is_some());

    let service = FileOpsService::new(Arc::new(|| {}));
    service.request(FileOpRequest::Transfer {
        paths: vec![foto.clone()],
        target: destinazione.clone(),
        copy: false,
        policy: ConflictPolicy::RenameAuto,
    });
    let outcome = attendi_esito(&service);
    assert_eq!(outcome.description, "Spostamento");
    assert!(
        outcome.report.failed.is_empty(),
        "{:?}",
        outcome.report.failed
    );
    assert!(!outcome.report.moved.is_empty());

    for pair in &outcome.report.moved {
        index.move_path(&pair.from, &pair.to).unwrap();
    }

    assert!(!foto.exists());
    assert!(destinazione.join("foto.jpg").exists());
    assert!(
        destinazione.join("foto.jpg.xmp").exists(),
        "il sidecar segue la foto"
    );
    assert!(
        index.get(&foto).unwrap().is_none(),
        "la riga sorgente non esiste più"
    );
    let riga = index
        .get(&destinazione.join("foto.jpg"))
        .unwrap()
        .expect("riga spostata presente");
    assert_eq!(riga.format.as_deref(), Some("JPEG"));
}

#[test]
fn copia_non_tocca_l_indice_della_sorgente() {
    let temp = tempfile::tempdir().unwrap();
    let (sorgente, destinazione) = prepara_cartelle(temp.path());
    fs::write(sorgente.join("foto.jpg"), b"contenuto jpeg").unwrap();

    let index = indice_della_cartella(&sorgente);
    let foto = sorgente.join("foto.jpg");

    let service = FileOpsService::new(Arc::new(|| {}));
    service.request(FileOpRequest::Transfer {
        paths: vec![foto.clone()],
        target: destinazione.clone(),
        copy: true,
        policy: ConflictPolicy::RenameAuto,
    });
    let outcome = attendi_esito(&service);
    assert_eq!(outcome.description, "Copia");
    assert!(
        outcome.report.failed.is_empty(),
        "{:?}",
        outcome.report.failed
    );

    // Il pump della UI non applica `move_path` alle copie: la sorgente resta.
    assert!(foto.exists());
    assert!(destinazione.join("foto.jpg").exists());
    assert!(
        index.get(&foto).unwrap().is_some(),
        "la riga sorgente resta nell'indice"
    );
    assert!(index.get(&destinazione.join("foto.jpg")).unwrap().is_none());
}

#[test]
fn rinomina_aggiorna_l_indice_e_l_annullamento_lo_ripristina() {
    let temp = tempfile::tempdir().unwrap();
    let (sorgente, _destinazione) = prepara_cartelle(temp.path());
    fs::write(sorgente.join("vecchia.jpg"), b"contenuto jpeg").unwrap();

    let index = indice_della_cartella(&sorgente);
    let vecchia = sorgente.join("vecchia.jpg");

    let service = FileOpsService::new(Arc::new(|| {}));
    service.request(FileOpRequest::Rename {
        path: vecchia.clone(),
        new_name: "nuova.jpg".to_owned(),
        policy: ConflictPolicy::RenameAuto,
    });
    let outcome = attendi_esito(&service);
    assert_eq!(outcome.description, "Rinomina");
    for pair in &outcome.report.moved {
        index.move_path(&pair.from, &pair.to).unwrap();
    }
    let nuova = sorgente.join("nuova.jpg");
    assert!(nuova.exists());
    assert!(index.get(&vecchia).unwrap().is_none());
    assert!(index.get(&nuova).unwrap().is_some());

    service.request(FileOpRequest::Undo {
        report: outcome.report,
    });
    let undo = attendi_esito(&service);
    assert_eq!(undo.description, "Annullamento");
    for pair in &undo.report.moved {
        index.move_path(&pair.from, &pair.to).unwrap();
    }
    assert!(vecchia.exists());
    assert!(
        index.get(&vecchia).unwrap().is_some(),
        "la riga originale è ripristinata"
    );
    assert!(index.get(&nuova).unwrap().is_none());
}
