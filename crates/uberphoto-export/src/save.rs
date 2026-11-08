//! Salvataggio manuale delle modifiche su una fotografia.
//!
//! Il salvataggio è esplicito: la ricetta colore, i metadati e la posizione
//! vengono applicati solo quando l'utente lo conferma. Sono previste due
//! modalità, entrambe con file di lavoro verificati:
//!
//! - **nuova copia**: rigenera i pixel in `nome-modificata.ext` accanto
//!   all'originale e non tocca il file di partenza;
//! - **sovrascrittura**: rigenera i pixel in un file temporaneo nella stessa
//!   cartella e pubblica il risultato solo dopo la verifica. La versione
//!   precedente resta in `.uberphoto-backups`; una breve assenza del nome
//!   originale è possibile durante la pubblicazione. Anche le sole modifiche
//!   ai metadati seguono il percorso di staging e verifica.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use uberphoto_core::FormatKind;
use uberphoto_files::{backup_file, files_equal, StagedFile};
use uberphoto_image::EditRecipe;
use uberphoto_metadata::{write_edits, ExifTool, MetadataEdit, WriteItem};

use crate::error::ExportError;
use crate::icc::profilo_srgb;
use crate::render::render_export_to;
use crate::service::{argomenti_copia, write_icc_temporaneo};
use crate::settings::{ExportFormat, ExportPreset, ExportSettings, NameMode, Naming, ResizeRules};

/// Modalità scelta dall'utente nel dialogo di salvataggio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveMode {
    /// Crea una copia accanto all'originale.
    NewCopy,
    /// Replaces the original after retaining a verified recovery copy.
    Overwrite,
}

impl SaveMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::NewCopy => "Nuova foto",
            Self::Overwrite => "Sovrascrivi originale",
        }
    }
}

/// Una fotografia da salvare con le modifiche raccolte dalla UI.
#[derive(Debug, Clone)]
pub struct SaveRequest {
    pub path: PathBuf,
    pub kind: FormatKind,
    pub recipe: EditRecipe,
    pub edits: Vec<MetadataEdit>,
    pub mode: SaveMode,
    /// Version shown in the browser when the user started the save.
    pub expected: Option<ExpectedFileVersion>,
}

#[derive(Debug, Clone, Copy)]
pub struct ExpectedFileVersion {
    pub byte_size: u64,
    pub modified: Option<SystemTime>,
}

impl ExpectedFileVersion {
    fn verify(self, path: &Path) -> Result<(), ExportError> {
        let metadata = fs::metadata(path)?;
        if metadata.len() != self.byte_size
            || self
                .modified
                .is_some_and(|expected| metadata.modified().ok() != Some(expected))
        {
            return Err(ExportError::Invalid(
                "la foto è cambiata dopo la selezione; aggiorna la cartella prima di salvare"
                    .to_owned(),
            ));
        }
        Ok(())
    }
}

/// Lotto di salvataggio (una selezione con le stesse modifiche).
#[derive(Debug, Clone)]
pub struct SaveJob {
    pub requests: Vec<SaveRequest>,
}

/// Esito del salvataggio di una fotografia.
#[derive(Debug, Clone)]
pub struct SaveOutcome {
    pub source: PathBuf,
    pub output: PathBuf,
    pub created: bool,
    pub warnings: Vec<String>,
}

/// Eventi emessi dal thread di salvataggio.
#[derive(Debug)]
pub enum SaveEvent {
    Saved(SaveOutcome),
    Failed {
        source: PathBuf,
        error: String,
    },
    Finished {
        saved: usize,
        failed: usize,
        total: usize,
    },
}

/// Servizio seriale di salvataggio in background.
pub struct SaveService {
    jobs: Sender<SaveJob>,
    events: Mutex<Receiver<SaveEvent>>,
    busy: Arc<AtomicBool>,
    repaint: Arc<dyn Fn() + Send + Sync>,
}

impl SaveService {
    pub fn new(repaint: Arc<dyn Fn() + Send + Sync>) -> Self {
        let (job_tx, job_rx) = mpsc::channel::<SaveJob>();
        let (event_tx, event_rx) = mpsc::channel::<SaveEvent>();
        let busy = Arc::new(AtomicBool::new(false));
        let busy_thread = busy.clone();
        let repaint_thread = repaint.clone();
        std::thread::Builder::new()
            .name("uberphoto-save".to_owned())
            .spawn(move || {
                while let Ok(job) = job_rx.recv() {
                    busy_thread.store(true, Ordering::SeqCst);
                    esegui_lavoro(job, &event_tx, &repaint_thread);
                    busy_thread.store(false, Ordering::SeqCst);
                    repaint_thread();
                }
            })
            .expect("avvio del thread di salvataggio");
        Self {
            jobs: job_tx,
            events: Mutex::new(event_rx),
            busy,
            repaint,
        }
    }

    /// Avvia un lotto; `false` se un salvataggio è già in corso.
    pub fn start(&self, job: SaveJob) -> bool {
        if self.busy.swap(true, Ordering::SeqCst) {
            return false;
        }
        if self.jobs.send(job).is_err() {
            self.busy.store(false, Ordering::SeqCst);
            return false;
        }
        (self.repaint)();
        true
    }

    pub fn busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
    }

    pub fn drain(&self) -> Vec<SaveEvent> {
        let Ok(events) = self.events.lock() else {
            return Vec::new();
        };
        events.try_iter().collect()
    }
}

impl std::fmt::Debug for SaveService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SaveService")
            .field("busy", &self.busy())
            .finish()
    }
}

fn esegui_lavoro(job: SaveJob, eventi: &Sender<SaveEvent>, repaint: &Arc<dyn Fn() + Send + Sync>) {
    let total = job.requests.len();
    let (saved, failed) = match save_batch(&job.requests) {
        Ok(outcomes) => {
            for outcome in outcomes {
                let _ = eventi.send(SaveEvent::Saved(outcome));
            }
            (total, 0)
        }
        Err(error) => {
            for request in &job.requests {
                let _ = eventi.send(SaveEvent::Failed {
                    source: request.path.clone(),
                    error: error.to_string(),
                });
            }
            (0, total)
        }
    };
    let _ = eventi.send(SaveEvent::Finished {
        saved,
        failed,
        total,
    });
    repaint();
}

/// Prepares all photos privately before committing any user file.
fn save_batch(requests: &[SaveRequest]) -> Result<Vec<SaveOutcome>, ExportError> {
    let Some(first) = requests.first() else {
        return Ok(Vec::new());
    };
    if requests.iter().any(|request| request.mode != first.mode) {
        return Err(ExportError::Invalid(
            "un lotto deve usare una sola modalità di salvataggio".to_owned(),
        ));
    }
    let mut work = Vec::new();
    let mut pairs = Vec::new();
    let mut outcomes = Vec::new();
    for request in requests {
        if let Some(expected) = request.expected {
            expected.verify(&request.path)?;
        }
        let staged = StagedFile::copy(&request.path, &request.path)?;
        let mut actual_sidecar = uberphoto_metadata::sidecar_path(&request.path);
        for (original, target) in [
            (
                request.path.with_extension("xmp"),
                staged.path().with_extension("xmp"),
            ),
            (
                uberphoto_metadata::sidecar_path(&request.path),
                uberphoto_metadata::sidecar_path(staged.path()),
            ),
        ] {
            if original.try_exists()? {
                uberphoto_files::writable_regular_file(&original)?;
                StagedFile::copy(&original, &target)?.publish(&target)?;
                actual_sidecar = original;
            }
        }
        let mut private_request = request.clone();
        private_request.path = staged.path().to_owned();
        private_request.expected = None;
        let result = salva_foto(&private_request)?;
        let sidecar =
            !request.kind.capabilities().embedded_metadata && request.mode == SaveMode::Overwrite;
        let (prepared, target, output) = if sidecar {
            let appended = uberphoto_metadata::sidecar_path(staged.path());
            let legacy = staged.path().with_extension("xmp");
            let prepared = if appended.is_file() { appended } else { legacy };
            (prepared, actual_sidecar, request.path.clone())
        } else if request.mode == SaveMode::NewCopy {
            let target = suggested_copy_path(&request.path)?;
            (result.output, target.clone(), target)
        } else {
            (result.output, request.path.clone(), request.path.clone())
        };
        pairs.push((prepared, target));
        outcomes.push(SaveOutcome {
            source: request.path.clone(),
            output,
            created: request.mode == SaveMode::NewCopy,
            warnings: result.warnings,
        });
        work.push(staged);
    }
    for request in requests {
        if let Some(expected) = request.expected {
            expected.verify(&request.path)?;
        }
    }
    uberphoto_files::publish_file_batch(&pairs, first.mode == SaveMode::Overwrite)?;
    Ok(outcomes)
}

/// Applica una richiesta di salvataggio con garanzia di rollback.
fn salva_foto(request: &SaveRequest) -> Result<SaveOutcome, ExportError> {
    if let Some(expected) = request.expected {
        expected.verify(&request.path)?;
    }
    let capabilities = request.kind.capabilities();
    let pixel_changes = !request.recipe.is_identity();
    if pixel_changes && !capabilities.pixel_edit {
        return Err(ExportError::Invalid(format!(
            "il colore non può essere applicato al formato {}: l'originale non viene \
             riscritto",
            request.kind.label()
        )));
    }
    if pixel_changes && export_format(request.kind).is_none() {
        return Err(ExportError::Invalid(format!(
            "il formato {} non è supportato dal rendering",
            request.kind.label()
        )));
    }
    if !pixel_changes && request.edits.is_empty() {
        return Err(ExportError::Invalid(
            "nessuna modifica da salvare".to_owned(),
        ));
    }

    match request.mode {
        SaveMode::NewCopy => salva_copia(request, pixel_changes),
        SaveMode::Overwrite => salva_sovrascrivendo(request, pixel_changes),
    }
}

fn salva_copia(request: &SaveRequest, pixel_changes: bool) -> Result<SaveOutcome, ExportError> {
    let capabilities = request.kind.capabilities();
    if !capabilities.pixel_edit {
        return Err(ExportError::Invalid(format!(
            "la nuova copia è disponibile solo per JPEG, PNG, TIFF e WebP; per {} \
             salva i metadati nell'originale",
            request.kind.label()
        )));
    }

    let target = suggested_copy_path(&request.path)?;
    let mut staged = StagedFile::new(&target)?;
    if pixel_changes {
        rigenera_e_copia_metadati(request, staged.path())?;
    } else {
        fs::copy(&request.path, staged.path())?;
        if !files_equal(&request.path, staged.path())? {
            return Err(ExportError::Invalid(
                "sorgente cambiata durante la copia".to_owned(),
            ));
        }
    }
    let warnings = applica_modifiche(staged.path(), request)?;
    staged.preserve_permissions(&request.path)?;
    staged.publish(&target)?;
    Ok(SaveOutcome {
        source: request.path.clone(),
        output: target,
        created: true,
        warnings,
    })
}

fn salva_sovrascrivendo(
    request: &SaveRequest,
    pixel_changes: bool,
) -> Result<SaveOutcome, ExportError> {
    if !pixel_changes {
        let warnings = applica_modifiche(&request.path, request)?;
        return Ok(SaveOutcome {
            source: request.path.clone(),
            output: request.path.clone(),
            created: false,
            warnings,
        });
    }
    let backup = backup_file(&request.path)?;
    if let Some(expected) = request.expected {
        expected.verify(&request.path)?;
    }
    let staged = StagedFile::new(&request.path)?;
    let mut snapshot = request.clone();
    snapshot.path = backup.clone();
    rigenera_e_copia_metadati(&snapshot, staged.path())?;
    let warnings = applica_modifiche(staged.path(), request)?;
    staged.replace(&request.path, &backup)?;
    Ok(SaveOutcome {
        source: request.path.clone(),
        output: request.path.clone(),
        created: false,
        warnings,
    })
}

/// Rigenera i pixel nel percorso indicato e trasferisce i metadati.
fn rigenera_e_copia_metadati(request: &SaveRequest, target: &Path) -> Result<(), ExportError> {
    let format = export_format(request.kind).ok_or_else(|| {
        ExportError::Invalid(format!(
            "il formato {} non è supportato dal rendering",
            request.kind.label()
        ))
    })?;
    let Some(tool) = ExifTool::detect() else {
        return Err(ExportError::Invalid(
            "ExifTool non disponibile: impossibile preservare i metadati".to_owned(),
        ));
    };
    let profilo = tool.read_blob(&request.path, "icc_profile")?;
    let settings = impostazioni_salvataggio(format);

    let rendered = render_export_to(
        &request.path,
        &settings,
        &request.recipe,
        profilo.as_deref(),
        target,
    )?;

    // Se i pixel sono stati convertiti in sRGB incorporiamo il profilo nuovo;
    // altrimenti lasciamo copiare l'ICC originale da ExifTool.
    let icc_file = rendered
        .converted_profile
        .then(|| write_icc_temporaneo(&profilo_srgb()))
        .transpose()?;
    let argomenti = argomenti_copia(
        &request.path,
        &settings,
        icc_file.as_ref().map(|file| file.path()),
    );
    tool.write_from(target, &argomenti)?;
    Ok(())
}

/// Applica i campi modificati e converte il report in un errore quando la
/// scrittura non è stata confermata.
fn applica_modifiche(target: &Path, request: &SaveRequest) -> Result<Vec<String>, ExportError> {
    if request.edits.is_empty() {
        return Ok(Vec::new());
    }
    let report = write_edits(&[WriteItem {
        path: target.to_path_buf(),
        format: request.kind,
        edits: request.edits.clone(),
    }]);
    if let Some(error) = report.first_error() {
        return Err(ExportError::Invalid(error));
    }
    if report.updated.is_empty() {
        let dettaglio = report
            .skipped
            .first()
            .map(|(_, message)| message.clone())
            .unwrap_or_else(|| "nessun campo applicabile".to_owned());
        return Err(ExportError::Invalid(dettaglio));
    }
    let mut warnings: Vec<String> = report
        .skipped
        .into_iter()
        .map(|(_, message)| message)
        .collect();
    warnings.extend(report.unverified.into_iter().map(|(_, message)| message));
    Ok(warnings)
}

fn impostazioni_salvataggio(format: ExportFormat) -> ExportSettings {
    ExportSettings {
        format,
        quality: 95,
        resize: ResizeRules {
            enabled: false,
            ..ResizeRules::default()
        },
        naming: Naming {
            mode: NameMode::KeepName,
            ..Naming::default()
        },
        preset: ExportPreset::FullMetadata,
        ..ExportSettings::default()
    }
}

fn export_format(kind: FormatKind) -> Option<ExportFormat> {
    match kind {
        FormatKind::Jpeg => Some(ExportFormat::Jpeg),
        FormatKind::Png => Some(ExportFormat::Png),
        FormatKind::Webp => Some(ExportFormat::Webp),
        FormatKind::Tiff => Some(ExportFormat::Tiff),
        _ => None,
    }
}

/// Suggests a new copy name without reserving or replacing a file.
///
/// # Errors
/// Returns an error when all candidate names are occupied.
pub fn suggested_copy_path(source: &Path) -> Result<PathBuf, ExportError> {
    let directory = source.parent().unwrap_or_else(|| Path::new("."));
    let stem = source
        .file_stem()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "foto".to_owned());
    let estensione = source
        .extension()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "jpg".to_owned());
    for indice in 1..10_000 {
        let nome = if indice == 1 {
            format!("{stem}-modificata.{estensione}")
        } else {
            format!("{stem}-modificata-{indice}.{estensione}")
        };
        let candidate = directory.join(nome);
        if fs::symlink_metadata(&candidate).is_err() {
            return Ok(candidate);
        }
    }
    Err(ExportError::Invalid(
        "impossibile trovare un nome libero per la nuova copia".to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageFormat;

    fn exiftool() -> Option<ExifTool> {
        ExifTool::detect()
    }

    fn crea_foto(path: &Path, width: u32, height: u32) {
        let image = image::RgbImage::from_fn(width, height, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])
        });
        image.save_with_format(path, ImageFormat::Jpeg).unwrap();
    }

    #[test]
    fn la_copia_usa_il_nome_automatico_e_non_tocca_l_originale() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("scatto.jpg");
        crea_foto(&source, 40, 30);
        let prima = fs::read(&source).unwrap();
        let request = SaveRequest {
            path: source.clone(),
            kind: FormatKind::Jpeg,
            recipe: EditRecipe {
                rotation: 90,
                ..EditRecipe::default()
            },
            edits: Vec::new(),
            mode: SaveMode::NewCopy,
            expected: None,
        };
        if exiftool().is_none() {
            eprintln!("ExifTool assente: test saltato");
            return;
        }
        let esito = salva_foto(&request).unwrap();
        assert_eq!(esito.output, dir.path().join("scatto-modificata.jpg"));
        assert!(esito.output.is_file());
        assert_eq!(fs::read(&source).unwrap(), prima);
        let copia = image::open(&esito.output).unwrap();
        assert_eq!((copia.width(), copia.height()), (30, 40));
    }

    #[test]
    fn save_rejects_a_photo_changed_since_selection() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("scatto.jpg");
        fs::write(&source, b"new external version").unwrap();
        let request = SaveRequest {
            path: source.clone(),
            kind: FormatKind::Jpeg,
            recipe: EditRecipe::default(),
            edits: vec![MetadataEdit::Field(
                uberphoto_metadata::MetadataField::Rating,
                Some("5".to_owned()),
            )],
            mode: SaveMode::Overwrite,
            expected: Some(ExpectedFileVersion {
                byte_size: 8,
                modified: None,
            }),
        };
        let error = salva_foto(&request).unwrap_err().to_string();
        assert!(error.contains("cambiata"), "{error}");
        assert_eq!(fs::read(&source).unwrap(), b"new external version");
        assert!(!dir.path().join(".uberphoto-backups").exists());
    }

    #[test]
    fn a_bad_second_save_never_commits_the_first_photo() {
        if exiftool().is_none() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.jpg");
        let second = dir.path().join("second.jpg");
        crea_foto(&first, 20, 20);
        crea_foto(&second, 20, 20);
        let original = fs::read(&first).unwrap();
        let requests: Vec<_> = [(&first, "5"), (&second, "99")]
            .into_iter()
            .map(|(path, rating)| SaveRequest {
                path: path.clone(),
                kind: FormatKind::Jpeg,
                recipe: EditRecipe::default(),
                edits: vec![MetadataEdit::Field(
                    uberphoto_metadata::MetadataField::Rating,
                    Some(rating.to_owned()),
                )],
                mode: SaveMode::Overwrite,
                expected: None,
            })
            .collect();
        assert!(save_batch(&requests).is_err());
        assert_eq!(fs::read(&first).unwrap(), original);
        assert_eq!(fs::read(&second).unwrap(), original);
    }

    #[test]
    fn prepared_batches_commit_embedded_and_sidecar_metadata() {
        let Some(tool) = exiftool() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let jpeg = dir.path().join("photo.jpg");
        let raw = dir.path().join("photo.dng");
        crea_foto(&jpeg, 20, 20);
        fs::copy(&jpeg, &raw).unwrap();
        let original_raw = fs::read(&raw).unwrap();
        let requests: Vec<_> = [(&jpeg, FormatKind::Jpeg), (&raw, FormatKind::Dng)]
            .into_iter()
            .map(|(path, kind)| SaveRequest {
                path: path.clone(),
                kind,
                recipe: EditRecipe::default(),
                edits: vec![MetadataEdit::Field(
                    uberphoto_metadata::MetadataField::Rating,
                    Some("5".to_owned()),
                )],
                mode: SaveMode::Overwrite,
                expected: None,
            })
            .collect();
        let outcomes = save_batch(&requests).unwrap();
        assert_eq!(outcomes.len(), 2);
        assert_eq!(fs::read(&raw).unwrap(), original_raw);
        for path in [&jpeg, &uberphoto_metadata::sidecar_path(&raw)] {
            let maps = tool.read(std::slice::from_ref(path), &[]).unwrap();
            assert_eq!(
                uberphoto_metadata::document_from_tags(&maps[0]).rating(),
                Some(5)
            );
        }
    }

    #[test]
    fn prepared_new_copy_batch_keeps_every_original_unchanged() {
        let Some(tool) = exiftool() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let mut requests = Vec::new();
        for name in ["first.jpg", "second.jpg"] {
            let path = dir.path().join(name);
            crea_foto(&path, 20, 20);
            requests.push(SaveRequest {
                path,
                kind: FormatKind::Jpeg,
                recipe: EditRecipe::default(),
                edits: vec![MetadataEdit::Field(
                    uberphoto_metadata::MetadataField::Rating,
                    Some("4".to_owned()),
                )],
                mode: SaveMode::NewCopy,
                expected: None,
            });
        }
        let original = fs::read(&requests[0].path).unwrap();
        for outcome in save_batch(&requests).unwrap() {
            assert_eq!(fs::read(&outcome.source).unwrap(), original);
            let maps = tool
                .read(std::slice::from_ref(&outcome.output), &[])
                .unwrap();
            assert_eq!(
                uberphoto_metadata::document_from_tags(&maps[0]).rating(),
                Some(4)
            );
        }
    }

    #[test]
    fn la_sovrascrittura_conserva_l_originale_e_pubblica_il_risultato() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("scatto.jpg");
        crea_foto(&source, 40, 30);
        let request = SaveRequest {
            path: source.clone(),
            kind: FormatKind::Jpeg,
            recipe: EditRecipe {
                rotation: 90,
                ..EditRecipe::default()
            },
            edits: Vec::new(),
            mode: SaveMode::Overwrite,
            expected: None,
        };
        if exiftool().is_none() {
            eprintln!("ExifTool assente: test saltato");
            return;
        }
        let esito = salva_foto(&request).unwrap();
        assert_eq!(esito.output, source);
        assert!(!esito.created);
        assert!(source.is_file());
        let letta = image::open(&source).unwrap();
        assert_eq!((letta.width(), letta.height()), (30, 40));
        let residui: Vec<PathBuf> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(".uberphoto-save-"))
            })
            .collect();
        assert!(residui.is_empty(), "file temporanei rimasti: {residui:?}");
    }

    #[test]
    fn la_sovrascrittura_aggiorna_i_metadati_senza_toccare_i_pixel() {
        let Some(tool) = exiftool() else {
            eprintln!("ExifTool assente: test saltato");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("scatto.jpg");
        crea_foto(&source, 20, 20);
        tool.write_embedded(&source, &["XMP-xmp:Rating=1".to_owned()])
            .unwrap();
        let prima_pixel = image::open(&source).unwrap().to_rgb8();
        let prima = fs::read(&source).unwrap();
        let request = SaveRequest {
            path: source.clone(),
            kind: FormatKind::Jpeg,
            recipe: EditRecipe::default(),
            edits: vec![MetadataEdit::Field(
                uberphoto_metadata::MetadataField::Rating,
                Some("5".to_owned()),
            )],
            mode: SaveMode::Overwrite,
            expected: None,
        };
        let esito = salva_foto(&request).unwrap();
        assert_eq!(esito.output, source);
        assert!(!esito.created);
        let tags = tool.read(std::slice::from_ref(&source), &[]).unwrap();
        let document = uberphoto_metadata::document_from_tags(&tags[0]);
        assert_eq!(document.rating(), Some(5));
        assert_ne!(fs::read(&source).unwrap(), prima);
        let dopo_pixel = image::open(&source).unwrap().to_rgb8();
        assert_eq!(prima_pixel, dopo_pixel, "i pixel non devono cambiare");
    }

    #[test]
    fn un_errore_di_metadati_non_riscrive_l_originale() {
        if exiftool().is_none() {
            eprintln!("ExifTool assente: test saltato");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("scatto.jpg");
        crea_foto(&source, 40, 30);
        let prima = fs::read(&source).unwrap();
        let request = SaveRequest {
            path: source.clone(),
            kind: FormatKind::Jpeg,
            recipe: EditRecipe {
                rotation: 90,
                ..EditRecipe::default()
            },
            edits: vec![MetadataEdit::Field(
                uberphoto_metadata::MetadataField::Rating,
                Some("99".to_owned()),
            )],
            mode: SaveMode::Overwrite,
            expected: None,
        };
        let errore = salva_foto(&request).unwrap_err().to_string();
        assert!(errore.contains("valutazione"), "{errore}");
        assert_eq!(fs::read(&source).unwrap(), prima);
        let residui: Vec<PathBuf> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(".uberphoto-save-"))
            })
            .collect();
        assert!(residui.is_empty(), "file temporanei rimasti: {residui:?}");
    }

    #[test]
    fn i_metadati_vengono_scritti_nella_copia() {
        let Some(_tool) = exiftool() else {
            eprintln!("ExifTool assente: test saltato");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("scatto.jpg");
        crea_foto(&source, 20, 20);
        let request = SaveRequest {
            path: source.clone(),
            kind: FormatKind::Jpeg,
            recipe: EditRecipe::default(),
            edits: vec![MetadataEdit::Field(
                uberphoto_metadata::MetadataField::Rating,
                Some("5".to_owned()),
            )],
            mode: SaveMode::NewCopy,
            expected: None,
        };
        let esito = salva_foto(&request).unwrap();
        let tool = ExifTool::detect().unwrap();
        let tags = tool.read(std::slice::from_ref(&esito.output), &[]).unwrap();
        let document = uberphoto_metadata::document_from_tags(&tags[0]);
        assert_eq!(document.rating(), Some(5));
        let originali = tool.read(std::slice::from_ref(&source), &[]).unwrap();
        assert_eq!(
            uberphoto_metadata::document_from_tags(&originali[0]).rating(),
            None
        );
    }

    #[test]
    fn la_nuova_copia_non_e_disponibile_per_i_raw() {
        let request = SaveRequest {
            path: PathBuf::from("/tmp/foto.nef"),
            kind: FormatKind::Nef,
            recipe: EditRecipe::default(),
            edits: vec![MetadataEdit::Field(
                uberphoto_metadata::MetadataField::Rating,
                Some("3".to_owned()),
            )],
            mode: SaveMode::NewCopy,
            expected: None,
        };
        let errore = salva_foto(&request).unwrap_err().to_string();
        assert!(errore.contains("nuova copia"), "{errore}");
    }

    #[test]
    fn le_collisioni_del_nome_sono_progressive() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("foto.png");
        fs::write(&source, b"x").unwrap();
        assert_eq!(
            suggested_copy_path(&source).unwrap(),
            dir.path().join("foto-modificata.png")
        );
        fs::write(dir.path().join("foto-modificata.png"), b"x").unwrap();
        assert_eq!(
            suggested_copy_path(&source).unwrap(),
            dir.path().join("foto-modificata-2.png")
        );
    }
}
