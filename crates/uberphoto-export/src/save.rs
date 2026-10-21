//! Salvataggio manuale delle modifiche su una fotografia.
//!
//! Il salvataggio è esplicito: la ricetta colore, i metadati e la posizione
//! vengono applicati solo quando l'utente lo conferma. Sono previste due
//! modalità, entrambe con rollback del file di lavoro:
//!
//! - **nuova copia**: rigenera i pixel in `nome-modificata.ext` accanto
//!   all'originale e non tocca il file di partenza;
//! - **sovrascrittura**: rigenera i pixel in un file temporaneo nella stessa
//!   cartella e sostituisce l'originale solo dopo che metadati e modifiche
//!   sono stati applicati con successo. Con le sole modifiche ai metadati il
//!   file viene aggiornato sul posto da ExifTool e riletto per verifica.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use uberphoto_core::FormatKind;
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
    /// Riscrive il file originale (irreversibile).
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
    let mut saved = 0usize;
    let mut failed = 0usize;
    for request in job.requests {
        match salva_foto(&request) {
            Ok(outcome) => {
                saved += 1;
                let _ = eventi.send(SaveEvent::Saved(outcome));
            }
            Err(error) => {
                failed += 1;
                let _ = eventi.send(SaveEvent::Failed {
                    source: request.path,
                    error: format!("{error:#}"),
                });
            }
        }
        repaint();
    }
    let _ = eventi.send(SaveEvent::Finished {
        saved,
        failed,
        total,
    });
    repaint();
}

/// Applica una richiesta di salvataggio con garanzia di rollback.
fn salva_foto(request: &SaveRequest) -> Result<SaveOutcome, ExportError> {
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

    let target = percorso_copia(&request.path)?;
    let esito = (|| -> Result<Vec<String>, ExportError> {
        if pixel_changes {
            rigenera_e_copia_metadati(request, &target)?;
        } else {
            fs::copy(&request.path, &target)?;
        }
        let warnings = applica_modifiche(&target, request)?;
        copia_permessi(&request.path, &target)?;
        Ok(warnings)
    })();

    match esito {
        Ok(warnings) => Ok(SaveOutcome {
            source: request.path.clone(),
            output: target,
            created: true,
            warnings,
        }),
        Err(error) => {
            let _ = fs::remove_file(&target);
            Err(error)
        }
    }
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

    let target = percorso_temporaneo(&request.path, request.kind)?;
    let esito = (|| -> Result<Vec<String>, ExportError> {
        rigenera_e_copia_metadati(request, &target)?;
        let warnings = applica_modifiche(&target, request)?;
        copia_permessi(&request.path, &target)?;
        sostituisci(&target, &request.path)?;
        Ok(warnings)
    })();

    match esito {
        Ok(warnings) => Ok(SaveOutcome {
            source: request.path.clone(),
            output: request.path.clone(),
            created: false,
            warnings,
        }),
        Err(error) => {
            let _ = fs::remove_file(&target);
            Err(error)
        }
    }
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
    let icc_path = rendered
        .converted_profile
        .then(|| write_icc_temporaneo(&profilo_srgb()));
    let argomenti = argomenti_copia(&request.path, &settings, icc_path.as_deref());
    let esito = tool.write_from(target, &argomenti);
    if let Some(path) = &icc_path {
        let _ = fs::remove_file(path);
    }
    esito?;
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

/// `nome-modificata.ext`, con gestione automatica delle collisioni.
fn percorso_copia(source: &Path) -> Result<PathBuf, ExportError> {
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
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(ExportError::Invalid(
        "impossibile trovare un nome libero per la nuova copia".to_owned(),
    ))
}

fn percorso_temporaneo(source: &Path, kind: FormatKind) -> Result<PathBuf, ExportError> {
    let directory = source.parent().unwrap_or_else(|| Path::new("."));
    let estensione = export_format(kind)
        .map(ExportFormat::extension)
        .ok_or_else(|| ExportError::Invalid("formato non supportato".to_owned()))?;
    for indice in 0..10_000u32 {
        let nome = format!(
            ".uberphoto-save-{}-{indice}.{estensione}",
            std::process::id()
        );
        let candidate = directory.join(nome);
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(ExportError::Invalid(
        "impossibile creare un file temporaneo nella cartella della foto".to_owned(),
    ))
}

fn copia_permessi(source: &Path, target: &Path) -> Result<(), ExportError> {
    let permessi = fs::metadata(source)?.permissions();
    fs::set_permissions(target, permessi)?;
    Ok(())
}

fn sostituisci(temp: &Path, target: &Path) -> Result<(), ExportError> {
    fs::rename(temp, target)?;
    Ok(())
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
    fn la_sovrascrittura_sostituisce_l_originale_in_modo_atomico() {
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
            percorso_copia(&source).unwrap(),
            dir.path().join("foto-modificata.png")
        );
        fs::write(dir.path().join("foto-modificata.png"), b"x").unwrap();
        assert_eq!(
            percorso_copia(&source).unwrap(),
            dir.path().join("foto-modificata-2.png")
        );
    }
}
