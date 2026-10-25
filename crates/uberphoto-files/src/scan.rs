//! Scansione delle cartelle.
//!
//! La scansione sincrona [`scan_directory`] legge una sola cartella;
//! [`scan_tree`] aggrega ricorsivamente una cartella e tutti i suoi
//! discendenti. Il thread di background gestito da [`DirectoryScanner`] usa
//! la seconda, così la griglia mostra l'intero sottoalbero senza bloccare
//! l'interfaccia.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use uberphoto_core::{FormatKind, PhotoAsset};

use crate::naming::{is_hidden_name, natural_cmp};
use crate::tree::{load_subdirs_filtered, DirNode};

/// Esito di una scansione richiesta a [`DirectoryScanner`].
pub struct ScanResult {
    pub dir: PathBuf,
    pub entries: Vec<PhotoAsset>,
    /// Sotto-cartelle dirette della cartella scansionata, per la barra di
    /// navigazione rapida mostrata sopra la griglia.
    pub subdirs: Vec<DirNode>,
    pub error: Option<String>,
}

/// Legge il contenuto di `dir` e restituisce i file immagine, ordinati per
/// nome (ordinamento naturale, case-insensitive).
///
/// - i file nascosti sono esclusi quando `include_hidden` è `false`;
/// - le directory sono escluse;
/// - i link simbolici verso file regolari sono seguiti;
/// - un file illeggibile viene saltato senza interrompere la scansione.
pub fn scan_directory(dir: &Path, include_hidden: bool) -> Result<Vec<PhotoAsset>, std::io::Error> {
    let mut named = Vec::new();
    collect_files(dir, include_hidden, &mut named)?;
    Ok(ordina(named))
}

/// Legge `dir` e tutti i suoi discendenti, restituendo le foto in un'unica
/// lista ordinata. Le cartelle non leggibili vengono saltate; gli errori di
/// lettura sulla radice interrompono la scansione. I link simbolici verso
/// directory sono seguiti una sola volta, evitando cicli.
pub fn scan_tree(dir: &Path, include_hidden: bool) -> Result<Vec<PhotoAsset>, std::io::Error> {
    let mut named = Vec::new();
    let mut visited = HashSet::new();
    let mut stack = vec![dir.to_path_buf()];

    while let Some(current) = stack.pop() {
        let canonical = fs::canonicalize(&current).unwrap_or_else(|_| current.clone());
        if !visited.insert(canonical) {
            continue;
        }

        let is_root = current.as_path() == dir;
        if let Err(err) = collect_files(&current, include_hidden, &mut named) {
            if is_root {
                return Err(err);
            }
            tracing::warn!(dir = %current.display(), "sottocartella non leggibile: {err}");
        }

        match load_subdirs_filtered(&current, include_hidden) {
            Ok(children) => stack.extend(children.into_iter().map(|child| child.path)),
            Err(err) if is_root => return Err(err),
            Err(err) => tracing::warn!(dir = %current.display(), "listing non riuscito: {err}"),
        }
    }

    Ok(ordina(named))
}

fn collect_files(
    dir: &Path,
    include_hidden: bool,
    named: &mut Vec<(String, PhotoAsset)>,
) -> Result<(), std::io::Error> {
    for entry in fs::read_dir(dir)? {
        let Ok(entry) = entry else { continue };

        let name = entry.file_name().to_string_lossy().into_owned();
        if !include_hidden && is_hidden_name(&name) {
            continue;
        }

        let path = entry.path();
        let Some(format) = FormatKind::from_path(&path) else {
            continue;
        };

        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            continue;
        }
        if !file_type.is_file() && !file_type.is_symlink() {
            continue;
        }

        // `fs::metadata` segue i symlink: un link a un file regolare vale
        // come file; un link a directory viene scartato qui.
        let Ok(meta) = fs::metadata(&path) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }

        named.push((
            name,
            PhotoAsset {
                path,
                format,
                byte_size: meta.len(),
                modified: meta.modified().ok(),
            },
        ));
    }
    Ok(())
}

fn ordina(mut named: Vec<(String, PhotoAsset)>) -> Vec<PhotoAsset> {
    named.sort_by(|a, b| natural_cmp(&a.0, &b.0).then_with(|| a.1.path.cmp(&b.1.path)));
    named.into_iter().map(|(_, asset)| asset).collect()
}

/// Scansiona cartelle su un thread dedicato e recapita i risultati alla UI.
///
/// Se arrivano più richieste mentre una scansione è in corso, viene servita
/// soltanto l'ultima: i risultati obsoleti vengono scartati dal chiamante
/// confrontando `ScanResult::dir` con la cartella corrente.
pub struct DirectoryScanner {
    tx: Sender<PathBuf>,
    rx: Receiver<ScanResult>,
}

impl DirectoryScanner {
    /// `repaint` viene invocato dopo ogni risultato recapitato, per risvegliare
    /// la UI (tipicamente `egui::Context::request_repaint`).
    pub fn new(repaint: Arc<dyn Fn() + Send + Sync>) -> Self {
        let (tx, request_rx) = channel::<PathBuf>();
        let (result_tx, rx) = channel::<ScanResult>();

        std::thread::Builder::new()
            .name("uberphoto-scan".into())
            .spawn(move || {
                while let Ok(mut dir) = request_rx.recv() {
                    // Salta le richieste superate: conta solo l'ultima.
                    while let Ok(newer) = request_rx.try_recv() {
                        dir = newer;
                    }

                    let subdirs = load_subdirs_filtered(&dir, false).unwrap_or_default();
                    let result = match scan_tree(&dir, false) {
                        Ok(entries) => ScanResult {
                            dir,
                            entries,
                            subdirs,
                            error: None,
                        },
                        Err(err) => ScanResult {
                            dir,
                            entries: Vec::new(),
                            subdirs,
                            error: Some(err.to_string()),
                        },
                    };

                    if result_tx.send(result).is_err() {
                        break;
                    }
                    repaint();
                }
            })
            .expect("impossibile avviare il thread di scansione");

        Self { tx, rx }
    }

    /// Richiede la scansione di `dir` (non bloccante).
    pub fn request(&self, dir: &Path) {
        let _ = self.tx.send(dir.to_path_buf());
    }

    /// Estrae il prossimo risultato disponibile, se presente (non bloccante).
    pub fn try_recv(&self) -> Option<ScanResult> {
        self.rx.try_recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scansiona_filtra_e_ordina() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();

        fs::write(root.join("IMG_10.jpg"), b"xxx").unwrap();
        fs::write(root.join("img_2.JPG"), b"xx").unwrap();
        fs::write(root.join("foto.png"), b"x").unwrap();
        fs::write(root.join("note.txt"), b"x").unwrap();
        fs::write(root.join(".nascosta.jpg"), b"x").unwrap();
        fs::create_dir(root.join("sottocartella")).unwrap();
        fs::write(root.join("sottocartella").join("dentro.jpg"), b"x").unwrap();

        let entries = scan_directory(root, false).expect("scansione");
        let names: Vec<String> = entries.iter().map(|e| e.file_name()).collect();
        assert_eq!(names, vec!["foto.png", "img_2.JPG", "IMG_10.jpg"]);

        assert_eq!(entries[0].format, FormatKind::Png);
        assert_eq!(entries[1].format, FormatKind::Jpeg);
        assert_eq!(entries[1].byte_size, 2);
        assert!(entries[0].modified.is_some());

        let with_hidden = scan_directory(root, true).expect("scansione con nascosti");
        assert_eq!(with_hidden.len(), 4);
    }

    #[test]
    fn cartella_inesistente_restituisce_errore() {
        let risultato = scan_directory(Path::new("/questa/cartella/non/esiste"), false);
        assert!(risultato.is_err());
        let albero = scan_tree(Path::new("/questa/cartella/non/esiste"), false);
        assert!(albero.is_err());
    }

    #[test]
    fn la_scansione_ricorsiva_aggrega_tutte_le_sottocartelle() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();

        fs::write(root.join("in-radice.jpg"), b"x").unwrap();
        fs::create_dir(root.join("vacanze")).unwrap();
        fs::write(root.join("vacanze").join("mare.jpg"), b"x").unwrap();
        fs::create_dir(root.join("vacanze").join("2024")).unwrap();
        fs::write(root.join("vacanze").join("2024").join("tramonto.jpg"), b"x").unwrap();
        fs::create_dir(root.join(".nascosta")).unwrap();
        fs::write(root.join(".nascosta").join("segreta.jpg"), b"x").unwrap();
        fs::write(root.join("vacanze").join("nota.txt"), b"x").unwrap();

        let entries = scan_tree(root, false).expect("scansione ricorsiva");
        let names: Vec<String> = entries.iter().map(|entry| entry.file_name()).collect();
        assert_eq!(names, vec!["in-radice.jpg", "mare.jpg", "tramonto.jpg"]);

        let with_hidden = scan_tree(root, true).expect("scansione ricorsiva con nascosti");
        let names: Vec<String> = with_hidden.iter().map(|entry| entry.file_name()).collect();
        assert_eq!(
            names,
            vec!["in-radice.jpg", "mare.jpg", "segreta.jpg", "tramonto.jpg"]
        );
    }

    #[test]
    fn i_symlink_di_cartella_non_creano_cicli() {
        #[cfg(unix)]
        {
            let temp = tempfile::tempdir().expect("tempdir");
            let root = temp.path();
            fs::create_dir(root.join("reale")).unwrap();
            fs::write(root.join("reale").join("foto.jpg"), b"x").unwrap();
            std::os::unix::fs::symlink(root, root.join("reale").join("loop")).unwrap();

            let entries = scan_tree(root, false).expect("scansione ricorsiva");
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].file_name(), "foto.jpg");
        }
    }

    /// Benchmark manuale (criterio di accettazione #1: cartelle grandi).
    /// Esegui con: `cargo test -p uberphoto-files -- --ignored --nocapture`
    #[test]
    #[ignore = "benchmark manuale su 2000 file"]
    fn scansione_di_migliaia_di_file_resta_rapida() {
        let temp = tempfile::tempdir().expect("tempdir");
        for index in 0..2000 {
            let name = format!("IMG_{index:05}.jpg");
            fs::write(temp.path().join(name), b"x").unwrap();
        }

        let start = std::time::Instant::now();
        let entries = scan_directory(temp.path(), false).expect("scansione");
        let elapsed = start.elapsed();

        assert_eq!(entries.len(), 2000);
        eprintln!("scansione di 2000 file: {elapsed:?}");
        assert!(
            elapsed.as_secs_f32() < 10.0,
            "scansione troppo lenta: {elapsed:?}"
        );
    }

    #[test]
    fn i_symlink_verso_file_regolari_sono_inclusi() {
        #[cfg(unix)]
        {
            let temp = tempfile::tempdir().expect("tempdir");
            let root = temp.path();
            fs::write(root.join("reale.jpg"), b"x").unwrap();
            std::os::unix::fs::symlink(root.join("reale.jpg"), root.join("link.jpg")).unwrap();

            let entries = scan_directory(root, false).expect("scansione");
            let names: Vec<String> = entries.iter().map(|e| e.file_name()).collect();
            assert_eq!(names, vec!["link.jpg", "reale.jpg"]);
        }
    }
}
