//! Operazioni sul filesystem: spostamento, copia e rinomina di fotografie
//! con gestione dei conflitti di nome, trasferimento dei sidecar XMP
//! associati e annullamento delle operazioni.
//!
//! Regole di sicurezza:
//! - nessuna sovrascrittura silenziosa: il conflitto si risolve solo con una
//!   politica esplicita (`RenameAuto`, `Replace`, `Skip`);
//! - spostamento tra dischi diversi tramite copia su file temporaneo nella
//!   cartella di destinazione, verifica, rinomina atomica e solo dopo la
//!   rimozione della sorgente;
//! - in caso di errore la sorgente viene conservata.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Politica di risoluzione quando il nome esiste già nella destinazione.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictPolicy {
    /// Nome libero automatico: `foto.jpg` → `foto (1).jpg`.
    RenameAuto,
    /// Sovrascrive il file esistente (scelta esplicita dell'utente).
    Replace,
    /// Salta il file e lo riporta nel report.
    Skip,
}

/// Singolo trasferimento riuscito, base anche per l'annullamento.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathMove {
    pub from: PathBuf,
    pub to: PathBuf,
}

/// Esito di un'operazione su più file.
#[derive(Debug, Default)]
pub struct FileOpReport {
    pub moved: Vec<PathMove>,
    pub skipped: Vec<(PathBuf, String)>,
    pub failed: Vec<(PathBuf, String)>,
}

impl FileOpReport {
    pub fn skipped_count(&self) -> usize {
        self.skipped.len()
    }

    pub fn failed_count(&self) -> usize {
        self.failed.len()
    }

    /// Riepilogo in italiano pronto per la barra di stato.
    pub fn summary(&self, verb: &str) -> String {
        let mut parts = vec![format!("{verb}: {} file", self.moved.len())];
        if self.skipped_count() > 0 {
            parts.push(format!("{} saltati", self.skipped_count()));
        }
        if self.failed_count() > 0 {
            parts.push(format!("{} errori", self.failed_count()));
        }
        parts.join(" · ")
    }

    /// Primo errore disponibile, per la barra di stato.
    pub fn first_error(&self) -> Option<String> {
        self.failed
            .first()
            .map(|(path, error)| format!("{}: {error}", path.display()))
    }
}

/// Sposta `paths` nella cartella `target_dir`.
pub fn move_assets(paths: &[PathBuf], target_dir: &Path, policy: ConflictPolicy) -> FileOpReport {
    transfer(paths, target_dir, policy, false)
}

/// Copia `paths` nella cartella `target_dir` (la sorgente resta).
pub fn copy_assets(paths: &[PathBuf], target_dir: &Path, policy: ConflictPolicy) -> FileOpReport {
    transfer(paths, target_dir, policy, true)
}

/// Rinomina una foto sul posto; i sidecar seguono il nuovo nome.
pub fn rename_asset(path: &Path, new_name: &str, policy: ConflictPolicy) -> FileOpReport {
    let mut report = FileOpReport::default();
    let Some(parent) = path.parent() else {
        report
            .failed
            .push((path.to_path_buf(), "percorso senza cartella".to_owned()));
        return report;
    };
    if let Err(message) = validate_name(new_name) {
        report.failed.push((path.to_path_buf(), message));
        return report;
    }

    let photo_dest = parent.join(new_name);
    if photo_dest == path {
        report.skipped.push((
            path.to_path_buf(),
            "il nuovo nome è identico a quello attuale".to_owned(),
        ));
        return report;
    }

    let assets = AssetSet::collect(path);
    transfer_asset(&assets, &photo_dest, policy, false, false, &mut report);
    report
}

/// Riporta indietro i file elencati nel report (annullamento best-effort).
/// I sidecar non vengono processati singolarmente: seguono la loro foto.
pub fn undo(report: &FileOpReport) -> FileOpReport {
    let mut out = FileOpReport::default();
    let photos: Vec<&PathMove> = report
        .moved
        .iter()
        .filter(|pair| !is_xmp(&pair.to))
        .collect();
    for pair in photos.into_iter().rev() {
        let assets = AssetSet::collect(&pair.to);
        transfer_asset(
            &assets,
            &pair.from,
            ConflictPolicy::RenameAuto,
            false,
            false,
            &mut out,
        );
    }
    out
}

fn is_xmp(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("xmp"))
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("il nome non può essere vuoto".to_owned());
    }
    if name.contains(['/', '\\']) || name == "." || name == ".." {
        return Err("il nome non può contenere separatori di percorso".to_owned());
    }
    Ok(())
}

fn transfer(
    paths: &[PathBuf],
    target_dir: &Path,
    policy: ConflictPolicy,
    copy: bool,
) -> FileOpReport {
    let mut report = FileOpReport::default();
    if !target_dir.is_dir() {
        for path in paths {
            report.failed.push((
                path.clone(),
                "la cartella di destinazione non esiste".to_owned(),
            ));
        }
        return report;
    }

    for path in paths {
        let assets = AssetSet::collect(path);
        let Some(file_name) = path.file_name() else {
            report
                .failed
                .push((path.clone(), "percorso senza nome file".to_owned()));
            continue;
        };
        let photo_dest = target_dir.join(file_name);
        transfer_asset(&assets, &photo_dest, policy, copy, false, &mut report);
    }
    report
}

/// Fotografia con gli eventuali sidecar XMP associati.
#[derive(Debug)]
struct AssetSet {
    photo: PathBuf,
    sidecars: Vec<PathBuf>,
}

impl AssetSet {
    fn collect(photo: &Path) -> Self {
        let mut sidecars = Vec::new();
        let appended = PathBuf::from(format!("{}.xmp", photo.display()));
        let replaced = photo.with_extension("xmp");
        for candidate in [appended, replaced] {
            if candidate != photo && candidate.is_file() && !sidecars.contains(&candidate) {
                sidecars.push(candidate);
            }
        }
        Self {
            photo: photo.to_path_buf(),
            sidecars,
        }
    }
}

/// Trasferisce una foto e i suoi sidecar verso `photo_dest` (nome finale).
/// `force_copy` è usato dai test per esercitare il percorso di copia.
fn transfer_asset(
    assets: &AssetSet,
    photo_dest: &Path,
    policy: ConflictPolicy,
    copy: bool,
    force_copy: bool,
    report: &mut FileOpReport,
) {
    let Some(target_dir) = photo_dest.parent() else {
        report
            .failed
            .push((assets.photo.clone(), "destinazione non valida".to_owned()));
        return;
    };

    if assets.photo.parent() == Some(target_dir) && !copy && photo_dest == assets.photo.as_path() {
        report.skipped.push((
            assets.photo.clone(),
            "il file è già nella cartella di destinazione".to_owned(),
        ));
        return;
    }

    let plan = match plan_destinations(assets, photo_dest, policy) {
        Ok(Some(plan)) => plan,
        Ok(None) => {
            report.skipped.push((
                assets.photo.clone(),
                "esiste già un file con questo nome".to_owned(),
            ));
            return;
        }
        Err(message) => {
            report.failed.push((assets.photo.clone(), message));
            return;
        }
    };

    if policy == ConflictPolicy::Replace {
        if let Err(err) = remove_if_exists(&plan.photo_dest) {
            report.failed.push((
                assets.photo.clone(),
                format!("impossibile sostituire il file esistente: {err}"),
            ));
            return;
        }
    }

    let photo_result = if copy {
        copy_to_destination(&assets.photo, &plan.photo_dest)
    } else {
        move_file(&assets.photo, &plan.photo_dest, force_copy)
    };
    match photo_result {
        Ok(()) => report.moved.push(PathMove {
            from: assets.photo.clone(),
            to: plan.photo_dest.clone(),
        }),
        Err(err) => {
            report
                .failed
                .push((assets.photo.clone(), format!("operazione fallita: {err}")));
            return;
        }
    }

    for (from, to) in &plan.sidecar_dests {
        if policy == ConflictPolicy::Replace {
            let _ = remove_if_exists(to);
        }
        let result = if copy {
            copy_to_destination(from, to)
        } else {
            move_file(from, to, force_copy)
        };
        match result {
            Ok(()) => report.moved.push(PathMove {
                from: from.clone(),
                to: to.clone(),
            }),
            Err(err) => report
                .failed
                .push((from.clone(), format!("sidecar non trasferito: {err}"))),
        }
    }
}

struct DestinationPlan {
    photo_dest: PathBuf,
    sidecar_dests: Vec<(PathBuf, PathBuf)>,
}

/// Calcola i nomi finali di foto e sidecar secondo la politica.
/// `Ok(None)` indica "salta" con `ConflictPolicy::Skip`.
fn plan_destinations(
    assets: &AssetSet,
    photo_dest: &Path,
    policy: ConflictPolicy,
) -> Result<Option<DestinationPlan>, String> {
    let build = |dest: &Path| -> Result<Option<DestinationPlan>, String> {
        let sidecar_dests = map_sidecars(assets, dest)?;
        Ok(Some(DestinationPlan {
            photo_dest: dest.to_path_buf(),
            sidecar_dests,
        }))
    };

    let conflicts = |dest: &Path| -> Result<Vec<PathBuf>, String> {
        let mut all = vec![dest.to_path_buf()];
        all.extend(map_sidecars(assets, dest)?.into_iter().map(|(_, to)| to));
        Ok(all.into_iter().filter(|path| path.exists()).collect())
    };

    match policy {
        ConflictPolicy::Replace => build(photo_dest),
        ConflictPolicy::Skip => {
            if conflicts(photo_dest)?.is_empty() {
                build(photo_dest)
            } else {
                Ok(None)
            }
        }
        ConflictPolicy::RenameAuto => {
            let file_name = photo_dest
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| "nome file non valido".to_owned())?;
            let (stem, extension) = split_name(file_name);
            for index in 0..10_000u32 {
                let candidate = if index == 0 {
                    file_name.to_owned()
                } else if extension.is_empty() {
                    format!("{stem} ({index})")
                } else {
                    format!("{stem} ({index}).{extension}")
                };
                let candidate_dest = photo_dest.with_file_name(candidate);
                if conflicts(&candidate_dest)?.is_empty() {
                    return build(&candidate_dest);
                }
            }
            Err("troppi file con lo stesso nome nella destinazione".to_owned())
        }
    }
}

/// Nomi dei sidecar nella destinazione, mappati dal nome della foto.
fn map_sidecars(assets: &AssetSet, photo_dest: &Path) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    let mut out = Vec::new();
    for sidecar in &assets.sidecars {
        let sidecar_name = sidecar
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "nome sidecar non valido".to_owned())?;
        let is_appended = sidecar_name.ends_with(".xmp")
            && assets
                .photo
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|photo_name| sidecar_name == format!("{photo_name}.xmp"));
        let dest_name = if is_appended {
            let photo_name = photo_dest
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| "nome file non valido".to_owned())?;
            format!("{photo_name}.xmp")
        } else {
            let (stem, _) = split_name(
                photo_dest
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| "nome file non valido".to_owned())?,
            );
            format!("{stem}.xmp")
        };
        out.push((sidecar.clone(), photo_dest.with_file_name(dest_name)));
    }
    Ok(out)
}

fn split_name(file_name: &str) -> (&str, &str) {
    match file_name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, extension),
        _ => (file_name, ""),
    }
}

/// Sposta un file: rinomina diretta e, solo per errori tra volumi diversi,
/// copia verificata + rimozione della sorgente.
fn move_file(from: &Path, to: &Path, force_copy: bool) -> io::Result<()> {
    if !force_copy {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(err) if is_cross_device(&err) => {}
            Err(err) => return Err(err),
        }
    }
    copy_then_remove(from, to)
}

fn is_cross_device(err: &io::Error) -> bool {
    matches!(err.kind(), io::ErrorKind::CrossesDevices) || err.raw_os_error() == Some(18)
}

/// Copia su file temporaneo nella destinazione, preserva il mtime,
/// rinomina atomicamente e solo dopo elimina la sorgente.
fn copy_then_remove(from: &Path, to: &Path) -> io::Result<()> {
    copy_to_destination(from, to)?;
    fs::remove_file(from)
}

/// Copia `from` su un file temporaneo accanto a `to`, preserva il mtime,
/// quindi rinomina atomicamente su `to`. Un fallimento lascia la
/// destinazione intatta.
fn copy_to_destination(from: &Path, to: &Path) -> io::Result<()> {
    let temp = temp_path(to);
    let result = copy_temp(from, &temp).and_then(|()| fs::rename(&temp, to));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn copy_temp(from: &Path, temp: &Path) -> io::Result<()> {
    fs::copy(from, temp)?;
    if let Ok(meta) = fs::metadata(from) {
        if let Ok(modified) = meta.modified() {
            let file = fs::File::options().write(true).open(temp)?;
            file.set_modified(modified)?;
        }
    }
    Ok(())
}

fn temp_path(to: &Path) -> PathBuf {
    let name = to
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_owned());
    to.with_file_name(format!(".{name}.uberphoto-tmp"))
}

fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

/// Crea una sottocartella `name` dentro `parent`.
pub fn create_dir(parent: &Path, name: &str) -> Result<PathBuf, String> {
    validate_name(name)?;
    let dir = parent.join(name);
    if dir.exists() {
        return Err(format!("\"{name}\" esiste già"));
    }
    fs::create_dir(&dir).map_err(|err| format!("creazione non riuscita: {err}"))?;
    Ok(dir)
}

/// Rinomina la cartella `dir` in `new_name`; restituisce il nuovo percorso.
pub fn rename_dir(dir: &Path, new_name: &str) -> Result<PathBuf, String> {
    validate_name(new_name)?;
    let parent = dir
        .parent()
        .ok_or_else(|| "la cartella non ha un genitore".to_owned())?;
    let dest = parent.join(new_name);
    if dest == dir {
        return Ok(dest);
    }
    if dest.exists() {
        return Err(format!("\"{new_name}\" esiste già"));
    }
    fs::rename(dir, &dest).map_err(|err| format!("rinomina non riuscita: {err}"))?;
    Ok(dest)
}

/// Elimina una cartella, solo se vuota: la rimozione ricorsiva non è
/// consentita in Fase 2 per evitare perdite di dati accidentali.
pub fn delete_empty_dir(dir: &Path) -> Result<(), String> {
    let mut entries = fs::read_dir(dir).map_err(|err| format!("lettura non riuscita: {err}"))?;
    if entries.next().is_some() {
        return Err("la cartella non è vuota: svuotala prima di eliminarla".to_owned());
    }
    fs::remove_dir(dir).map_err(|err| format!("eliminazione non riuscita: {err}"))
}

/// Numero di voci (file più cartelle) contenute in `dir`.
pub fn dir_item_count(dir: &Path) -> usize {
    fs::read_dir(dir)
        .map(|entries| entries.flatten().count())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    use tempfile::tempdir;

    fn write_file(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, b"contenuto").unwrap();
    }

    #[test]
    fn spostamento_semplice() {
        let temp = tempdir().unwrap();
        let source = temp.path().join("sorgente");
        let target = temp.path().join("destinazione");
        write_file(&source.join("foto.jpg"));
        fs::create_dir_all(&target).unwrap();

        let report = move_assets(
            &[source.join("foto.jpg")],
            &target,
            ConflictPolicy::RenameAuto,
        );
        assert_eq!(report.moved.len(), 1);
        assert!(!source.join("foto.jpg").exists());
        assert!(target.join("foto.jpg").exists());
    }

    #[test]
    fn conflitto_con_rinomina_automatica() {
        let temp = tempdir().unwrap();
        let source = temp.path().join("a");
        let target = temp.path().join("b");
        write_file(&source.join("foto.jpg"));
        write_file(&target.join("foto.jpg"));
        write_file(&target.join("foto (1).jpg"));

        let report = move_assets(
            &[source.join("foto.jpg")],
            &target,
            ConflictPolicy::RenameAuto,
        );
        assert_eq!(report.moved.len(), 1);
        assert!(target.join("foto (2).jpg").exists());
    }

    #[test]
    fn conflitto_con_sostituzione() {
        let temp = tempdir().unwrap();
        let source = temp.path().join("a");
        let target = temp.path().join("b");
        fs::create_dir_all(&target).unwrap();
        write_file(&source.join("foto.jpg"));
        fs::write(target.join("foto.jpg"), b"vecchio").unwrap();
        fs::write(source.join("foto.jpg"), b"nuovo").unwrap();

        let report = move_assets(&[source.join("foto.jpg")], &target, ConflictPolicy::Replace);
        assert_eq!(report.moved.len(), 1);
        assert_eq!(fs::read(target.join("foto.jpg")).unwrap(), b"nuovo");
    }

    #[test]
    fn conflitto_con_salto() {
        let temp = tempdir().unwrap();
        let source = temp.path().join("a");
        let target = temp.path().join("b");
        write_file(&source.join("foto.jpg"));
        write_file(&target.join("foto.jpg"));

        let report = move_assets(&[source.join("foto.jpg")], &target, ConflictPolicy::Skip);
        assert_eq!(report.moved.len(), 0);
        assert_eq!(report.skipped.len(), 1);
        assert!(source.join("foto.jpg").exists());
    }

    #[test]
    fn i_sidecar_seguono_la_foto() {
        let temp = tempdir().unwrap();
        let source = temp.path().join("a");
        let target = temp.path().join("b");
        write_file(&source.join("foto.jpg"));
        write_file(&source.join("foto.jpg.xmp"));
        write_file(&source.join("foto.xmp"));
        fs::create_dir_all(&target).unwrap();

        let report = move_assets(
            &[source.join("foto.jpg")],
            &target,
            ConflictPolicy::RenameAuto,
        );
        assert_eq!(report.moved.len(), 3);
        assert!(target.join("foto.jpg").exists());
        assert!(target.join("foto.jpg.xmp").exists());
        assert!(target.join("foto.xmp").exists());
        assert!(!source.join("foto.jpg.xmp").exists());
    }

    #[test]
    fn rinomina_in_place_con_sidecar() {
        let temp = tempdir().unwrap();
        write_file(&temp.path().join("foto.jpg"));
        write_file(&temp.path().join("foto.jpg.xmp"));

        let report = rename_asset(
            &temp.path().join("foto.jpg"),
            "vacanza.jpg",
            ConflictPolicy::RenameAuto,
        );
        assert_eq!(report.moved.len(), 2);
        assert!(temp.path().join("vacanza.jpg").exists());
        assert!(temp.path().join("vacanza.jpg.xmp").exists());
        assert!(!temp.path().join("foto.jpg").exists());
    }

    #[test]
    fn annullamento_riporta_indietro() {
        let temp = tempdir().unwrap();
        let source = temp.path().join("a");
        let target = temp.path().join("b");
        write_file(&source.join("foto.jpg"));
        write_file(&source.join("foto.jpg.xmp"));
        fs::create_dir_all(&target).unwrap();

        let report = move_assets(
            &[source.join("foto.jpg")],
            &target,
            ConflictPolicy::RenameAuto,
        );
        assert_eq!(report.moved.len(), 2);

        let undone = undo(&report);
        assert_eq!(undone.failed.len(), 0);
        assert_eq!(undone.moved.len(), 2);
        assert!(source.join("foto.jpg").exists());
        assert!(source.join("foto.jpg.xmp").exists());
        assert!(!target.join("foto.jpg").exists());
    }

    #[test]
    fn copia_lascia_la_sorgente() {
        let temp = tempdir().unwrap();
        let source = temp.path().join("a");
        let target = temp.path().join("b");
        write_file(&source.join("foto.jpg"));
        fs::create_dir_all(&target).unwrap();

        let report = copy_assets(
            &[source.join("foto.jpg")],
            &target,
            ConflictPolicy::RenameAuto,
        );
        assert_eq!(report.moved.len(), 1);
        assert!(source.join("foto.jpg").exists());
        assert!(target.join("foto.jpg").exists());
    }

    #[test]
    fn percorso_di_copia_forzato_preserva_contenuto_e_mtime() {
        let temp = tempdir().unwrap();
        let from = temp.path().join("from.jpg");
        let to = temp.path().join("to.jpg");
        fs::write(&from, b"dati").unwrap();
        let modified = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let file = fs::File::options().write(true).open(&from).unwrap();
        file.set_modified(modified).unwrap();
        drop(file);

        move_file(&from, &to, true).unwrap();
        assert!(!from.exists());
        assert_eq!(fs::read(&to).unwrap(), b"dati");
        let to_modified = fs::metadata(&to).unwrap().modified().unwrap();
        let delta = to_modified
            .duration_since(modified)
            .or_else(|err| Ok::<_, std::time::SystemTimeError>(err.duration()))
            .unwrap();
        assert!(delta.as_secs() < 2, "mtime non preservato: {delta:?}");
    }

    #[test]
    fn destinazione_inesistente_produce_errori() {
        let temp = tempdir().unwrap();
        let source = temp.path().join("a");
        write_file(&source.join("foto.jpg"));

        let report = move_assets(
            &[source.join("foto.jpg")],
            &temp.path().join("mai-creata"),
            ConflictPolicy::RenameAuto,
        );
        assert_eq!(report.failed.len(), 1);
        assert!(source.join("foto.jpg").exists());
    }

    #[test]
    fn solo_cartelle_vuote_si_eliminano() {
        let temp = tempdir().unwrap();
        let vuota = temp.path().join("vuota");
        let piena = temp.path().join("piena");
        fs::create_dir_all(&vuota).unwrap();
        write_file(&piena.join("foto.jpg"));

        assert!(delete_empty_dir(&vuota).is_ok());
        assert!(delete_empty_dir(&piena).is_err());
        assert!(piena.join("foto.jpg").exists());
    }

    #[test]
    fn rinomina_cartella_e_conflitto() {
        let temp = tempdir().unwrap();
        let dir = temp.path().join("vecchia");
        fs::create_dir_all(&dir).unwrap();
        fs::create_dir_all(temp.path().join("nuova")).unwrap();

        assert!(rename_dir(&dir, "nuova").is_err());
        let renamed = rename_dir(&dir, "fresca").unwrap();
        assert!(renamed.is_dir());
        assert!(!dir.exists());
    }

    #[test]
    fn nome_non_valido_rifiutato() {
        let temp = tempdir().unwrap();
        assert!(create_dir(temp.path(), "sotto/cartella").is_err());
        assert!(create_dir(temp.path(), "..").is_err());
        assert!(create_dir(temp.path(), "  ").is_err());
    }
}
