//! Servizio di esportazione su thread dedicato, con avanzamento e annullamento.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use tracing::warn;
use uberphoto_core::FormatKind;
use uberphoto_image::EditRecipe;
use uberphoto_metadata::ExifTool;

use crate::error::ExportError;
use crate::icc::profilo_srgb;
use crate::render::render_export;
use crate::report::{build_report, ExportReport, ReportInput};
use crate::settings::{ExportPreset, ExportSettings};

/// Una fotografia da esportare con la propria ricetta colore.
#[derive(Debug, Clone)]
pub struct ExportItem {
    pub path: PathBuf,
    pub kind: FormatKind,
    pub recipe: EditRecipe,
}

/// Lavoro di esportazione (una selezione con gli stessi parametri).
#[derive(Debug, Clone)]
pub struct ExportJob {
    pub settings: ExportSettings,
    pub items: Vec<ExportItem>,
}

/// Eventi emessi dal thread di esportazione.
#[derive(Debug)]
pub enum ExportEvent {
    /// Un file è stato esportato e verificato.
    Item(ExportReport),
    /// Un file non è stato esportato (nessun output lasciato sul disco).
    Failed { source: PathBuf, error: String },
    /// L'intero lavoro è terminato.
    Finished {
        exported: usize,
        failed: usize,
        total: usize,
        cancelled: bool,
    },
}

/// Servizio di esportazione seriale in background.
pub struct ExportService {
    jobs: Sender<ExportJob>,
    events: Mutex<Receiver<ExportEvent>>,
    busy: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    repaint: Arc<dyn Fn() + Send + Sync>,
}

impl ExportService {
    pub fn new(repaint: Arc<dyn Fn() + Send + Sync>) -> Self {
        let (job_tx, job_rx) = mpsc::channel::<ExportJob>();
        let (event_tx, event_rx) = mpsc::channel::<ExportEvent>();
        let busy = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        let busy_thread = busy.clone();
        let cancel_thread = cancel.clone();
        let repaint_thread = repaint.clone();
        std::thread::Builder::new()
            .name("uberphoto-export".to_owned())
            .spawn(move || {
                while let Ok(job) = job_rx.recv() {
                    busy_thread.store(true, Ordering::SeqCst);
                    esegui_lavoro(job, &event_tx, &cancel_thread, &repaint_thread);
                    busy_thread.store(false, Ordering::SeqCst);
                    repaint_thread();
                }
            })
            .expect("avvio del thread di esportazione");
        Self {
            jobs: job_tx,
            events: Mutex::new(event_rx),
            busy,
            cancel,
            repaint,
        }
    }

    /// Avvia un lavoro; `false` se un altro è in corso.
    pub fn start(&self, job: ExportJob) -> bool {
        if self.busy.swap(true, Ordering::SeqCst) {
            return false;
        }
        self.cancel.store(false, Ordering::SeqCst);
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

    /// Chiede l'interruzione dopo il file in corso.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    pub fn drain(&self) -> Vec<ExportEvent> {
        let Ok(events) = self.events.lock() else {
            return Vec::new();
        };
        events.try_iter().collect()
    }
}

impl std::fmt::Debug for ExportService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExportService")
            .field("busy", &self.busy())
            .finish()
    }
}

fn esegui_lavoro(
    job: ExportJob,
    eventi: &Sender<ExportEvent>,
    cancel: &AtomicBool,
    repaint: &Arc<dyn Fn() + Send + Sync>,
) {
    let total = job.items.len();
    let mut exported = 0usize;
    let mut failed = 0usize;
    let mut cancelled = false;

    let tool = ExifTool::detect();
    let icc = profilo_srgb();
    let icc_path = (tool.is_some() && !icc.is_empty()).then(|| write_icc_temporaneo(&icc));
    if tool.is_none() {
        // Senza ExifTool non si può garantire né copiare né verificare nulla.
        for item in &job.items {
            failed += 1;
            let _ = eventi.send(ExportEvent::Failed {
                source: item.path.clone(),
                error: "ExifTool non disponibile: impossibile preservare e verificare \
                        i metadati"
                    .to_owned(),
            });
        }
        let _ = eventi.send(ExportEvent::Finished {
            exported,
            failed,
            total,
            cancelled: false,
        });
        return;
    }

    for item in job.items {
        if cancel.swap(false, Ordering::SeqCst) {
            cancelled = true;
            break;
        }
        let profilo = match &tool {
            Some(tool) => match tool.read_blob(&item.path, "icc_profile") {
                Ok(profilo) => profilo,
                Err(err) => {
                    failed += 1;
                    let _ = eventi.send(ExportEvent::Failed {
                        source: item.path.clone(),
                        error: format!("{err:#}"),
                    });
                    repaint();
                    continue;
                }
            },
            None => None,
        };
        let esito = esporta_item(
            &item,
            &job.settings,
            profilo.as_deref(),
            tool.as_ref(),
            icc_path.as_deref(),
        );
        match esito {
            Ok(report) => {
                exported += 1;
                let _ = eventi.send(ExportEvent::Item(report));
            }
            Err(err) => {
                failed += 1;
                let _ = eventi.send(ExportEvent::Failed {
                    source: item.path,
                    error: format!("{err:#}"),
                });
            }
        }
        repaint();
    }

    if let Some(path) = icc_path {
        let _ = std::fs::remove_file(path);
    }
    let _ = eventi.send(ExportEvent::Finished {
        exported,
        failed,
        total,
        cancelled,
    });
    repaint();
}

fn esporta_item(
    item: &ExportItem,
    settings: &ExportSettings,
    profilo: Option<&[u8]>,
    tool: Option<&ExifTool>,
    icc_path: Option<&Path>,
) -> Result<ExportReport, ExportError> {
    if !item.kind.capabilities().view {
        return Err(ExportError::Invalid(format!(
            "il formato {} non è ancora decodificabile per l'esportazione",
            item.kind.label()
        )));
    }
    let Some(tool) = tool else {
        return Err(ExportError::Invalid("ExifTool non disponibile".to_owned()));
    };

    let rendered = render_export(&item.path, settings, &item.recipe, profilo)?;
    match completa_metadati(&item.path, &rendered, settings, tool, icc_path) {
        Ok(report) => Ok(report),
        Err(err) => {
            // Nessun file parziale con metadati non verificati.
            let _ = std::fs::remove_file(&rendered.path);
            Err(err)
        }
    }
}

fn completa_metadati(
    source: &Path,
    rendered: &crate::render::RenderOutput,
    settings: &ExportSettings,
    tool: &ExifTool,
    icc_path: Option<&Path>,
) -> Result<ExportReport, ExportError> {
    let argomenti = argomenti_copia(source, settings, icc_path);
    let mut warnings = tool.write_from(&rendered.path, &argomenti)?;

    if icc_path.is_none() {
        warnings.push("Profilo sRGB non incorporato (profilo non disponibile)".to_owned());
    }

    let tags = tool.read(&[source.to_path_buf(), rendered.path.clone()], &[])?;
    if tags.len() != 2 {
        return Err(ExportError::Invalid(format!(
            "ExifTool ha restituito {} documenti invece di 2",
            tags.len()
        )));
    }
    Ok(build_report(ReportInput {
        source,
        output: &rendered.path,
        bytes: rendered.bytes,
        width: rendered.width,
        height: rendered.height,
        preset: settings.preset,
        privacy: settings.privacy,
        format: settings.format,
        converted_profile: rendered.converted_profile,
        resized: rendered.resized,
        source_tags: &tags[0],
        output_tags: &tags[1],
        warnings,
    }))
}

/// Costruisce gli argomenti ExifTool per la copia dei metadati.
fn argomenti_copia(
    source: &Path,
    settings: &ExportSettings,
    icc_path: Option<&Path>,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["-tagsfromfile".into(), source.as_os_str().to_owned()];

    let mut assegnazioni: Vec<String> = vec!["Orientation=1".to_owned()];
    if let Some(path) = icc_path {
        assegnazioni.push(format!("icc_profile<={}", path.display()));
    }

    let mut esclusioni: Vec<String> = Vec::new();
    // Tag strutturali o non coerenti con un file rigenerato.
    let strutturali = [
        "--ICC_Profile",
        "--File:all",
        "--ThumbnailImage",
        "--ImageWidth",
        "--ImageHeight",
        "--ExifImageWidth",
        "--ExifImageHeight",
        "--PixelXDimension",
        "--PixelYDimension",
        "--Orientation",
        "--PreviewImage",
        "--JpgFromRaw",
    ];

    match settings.preset {
        ExportPreset::FullMetadata | ExportPreset::Privacy => {
            args.push("-all:all".into());
            esclusioni.extend(strutturali.iter().map(|tag| (*tag).to_owned()));
            for gruppo in settings.format.unsupported_groups() {
                esclusioni.push(format!("--{gruppo}:all"));
            }
            if settings.preset == ExportPreset::Privacy {
                esclusioni.extend(settings.privacy.exclusions().into_iter().map(str::to_owned));
            }
        }
        ExportPreset::WebOptimized => {
            let selettori = [
                "EXIF:ImageDescription",
                "EXIF:Artist",
                "EXIF:Copyright",
                "IPTC:ObjectName",
                "IPTC:Caption-Abstract",
                "IPTC:Keywords",
                "IPTC:CopyrightNotice",
                "IPTC:Credit",
                "XMP-dc:Title",
                "XMP-dc:Description",
                "XMP-dc:Subject",
                "XMP-dc:Creator",
                "XMP-dc:Rights",
                "XMP-photoshop:Credit",
                "XMP-xmp:Rating",
                "GPS:all",
            ];
            for selettore in selettori {
                let gruppo = selettore.split(':').next().unwrap_or_default();
                if settings.format.unsupported_groups().contains(&gruppo) {
                    continue;
                }
                args.push(format!("-{selettore}").into());
            }
        }
    }

    for esclusione in esclusioni {
        args.push(esclusione.into());
    }
    for assegnazione in assegnazioni {
        args.push(format!("-{assegnazione}").into());
    }
    args
}

fn write_icc_temporaneo(bytes: &[u8]) -> PathBuf {
    static CONTATORE: AtomicU64 = AtomicU64::new(0);
    let indice = CONTATORE.fetch_add(1, Ordering::Relaxed);
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|durata| durata.as_millis())
        .unwrap_or_default();
    let path = std::env::temp_dir().join(format!(
        "uberphoto-srgb-{}-{millis}-{indice}.icc",
        std::process::id()
    ));
    if let Err(err) = std::fs::write(&path, bytes) {
        warn!("profilo sRGB temporaneo non scritto: {err}");
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::ExportFormat;

    #[test]
    fn gli_argomenti_full_escludono_i_tag_strutturali_e_incorporano_srgb() {
        let settings = ExportSettings {
            preset: ExportPreset::FullMetadata,
            format: ExportFormat::Jpeg,
            ..ExportSettings::default()
        };
        let args = argomenti_copia(
            Path::new("/tmp/orig.jpg"),
            &settings,
            Some(Path::new("/tmp/s.icc")),
        );
        let testuali: Vec<String> = args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(testuali.contains(&"-all:all".to_owned()));
        assert!(testuali.contains(&"--ICC_Profile".to_owned()));
        assert!(testuali.contains(&"-Orientation=1".to_owned()));
        assert!(testuali.iter().any(|arg| arg.starts_with("-icc_profile<=")));
    }

    #[test]
    fn il_preset_web_non_copia_tutto_e_rispetta_i_limiti_del_formato() {
        let settings = ExportSettings {
            preset: ExportPreset::WebOptimized,
            format: ExportFormat::Webp,
            ..ExportSettings::default()
        };
        let args = argomenti_copia(Path::new("/tmp/orig.jpg"), &settings, None);
        let testuali: Vec<String> = args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(!testuali.contains(&"-all:all".to_owned()));
        assert!(testuali.contains(&"-XMP-dc:Creator".to_owned()));
        assert!(!testuali.iter().any(|arg| arg.starts_with("-IPTC:")));
    }

    #[test]
    fn il_preset_privacy_esclude_gps_e_seriali() {
        let settings = ExportSettings {
            preset: ExportPreset::Privacy,
            ..ExportSettings::default()
        };
        let args = argomenti_copia(Path::new("/tmp/orig.jpg"), &settings, None);
        let testuali: Vec<String> = args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(testuali.contains(&"--GPS*".to_owned()));
        assert!(testuali.contains(&"--*Serial*".to_owned()));
    }
}
