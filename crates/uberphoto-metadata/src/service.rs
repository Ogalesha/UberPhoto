//! Servizio metadati: letture e scritture ExifTool su un thread dedicato.
//!
//! Le richieste seguono lo stesso schema degli altri servizi (scansione,
//! miniature, operazioni file): canale mpsc, nessun blocco della UI e
//! richiesta di repaint al termine.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, OnceLock};

use uberphoto_core::FormatKind;

use crate::error::MetadataError;
use crate::exiftool::{sidecar_path, ExifTool};
use crate::model::{
    changes_for, document_from_tags, raw_tags_from, summary_from_tags, MetadataDocument,
    MetadataField, MetadataSummary, RawTag,
};

/// File per invocazione nelle letture riassuntive (indice).
const SUMMARY_CHUNK: usize = 96;
/// File per invocazione nelle letture complete (editor e tag grezzi).
const FULL_CHUNK: usize = 12;

/// Tag letti per popolare l'indice: elenco ridotto per restare veloci
/// anche su cartelle con centinaia di foto.
const SUMMARY_TAGS: &[&str] = &[
    "EXIF:Make",
    "EXIF:Model",
    "EXIF:LensModel",
    "EXIF:ISO",
    "EXIF:FNumber",
    "EXIF:ExposureTime",
    "EXIF:FocalLength",
    "EXIF:DateTimeOriginal",
    "XMP-photoshop:DateCreated",
    "XMP-xmp:Rating",
    "XMP-dc:Subject",
    "IPTC:Keywords",
    "Composite:GPSLatitude",
    "Composite:GPSLongitude",
];

static TOOL: OnceLock<Option<ExifTool>> = OnceLock::new();

fn tool() -> Result<&'static ExifTool, MetadataError> {
    TOOL.get_or_init(ExifTool::detect)
        .as_ref()
        .ok_or(MetadataError::ToolMissing)
}

/// True se ExifTool è disponibile (usato dalla UI per avvisare l'utente).
pub fn exiftool_disponibile() -> bool {
    tool().is_ok()
}

/// Versione di ExifTool, per il pannello Informazioni.
pub fn exiftool_versione() -> Option<String> {
    tool().ok().and_then(|tool| tool.version().ok())
}

/// Richiesta inviata dal thread UI.
pub enum MetadataRequest {
    /// Riassunto dei campi indicizzati per un lotto di foto.
    ReadSummary { paths: Vec<PathBuf> },
    /// Documento completo e tag grezzi per le foto selezionate.
    ReadFull { paths: Vec<PathBuf> },
    /// Scrittura dei campi modificati su una o più foto.
    Write { items: Vec<WriteItem> },
}

/// Modifiche da applicare a una foto.
pub struct WriteItem {
    pub path: PathBuf,
    pub format: FormatKind,
    pub edits: Vec<(MetadataField, Option<String>)>,
}

/// Documento completo con la vista avanzata dei tag.
pub struct FullDocument {
    pub document: MetadataDocument,
    pub raw: Vec<RawTag>,
}

/// Esito di una richiesta.
pub enum MetadataOutcome {
    Summary {
        items: Vec<(PathBuf, MetadataSummary)>,
    },
    Full {
        items: Vec<(PathBuf, Result<FullDocument, String>)>,
    },
    Written {
        report: MetadataWriteReport,
    },
}

/// Esito della scrittura, con la verifica post-scrittura richiesta dalla spec.
#[derive(Debug, Default, Clone)]
pub struct MetadataWriteReport {
    pub updated: Vec<PathBuf>,
    /// Campi non applicabili al formato (es. orientamento su sidecar XMP).
    pub skipped: Vec<(PathBuf, String)>,
    /// Campi scritti ma non confermati dalla rilettura.
    pub unverified: Vec<(PathBuf, String)>,
    pub failed: Vec<(PathBuf, String)>,
}

impl MetadataWriteReport {
    pub fn summary(&self) -> String {
        let mut text = format!("Metadati aggiornati su {} foto", self.updated.len());
        if !self.skipped.is_empty() {
            text.push_str(&format!("; {} campi senza equivalente", self.skipped.len()));
        }
        if !self.unverified.is_empty() {
            text.push_str(&format!(
                "; {} valori non confermati",
                self.unverified.len()
            ));
        }
        if !self.failed.is_empty() {
            text.push_str(&format!("; {} non riuscite", self.failed.len()));
        }
        text
    }

    pub fn first_error(&self) -> Option<String> {
        self.failed
            .first()
            .map(|(path, message)| format!("{}: {message}", path.display()))
            .or_else(|| {
                self.unverified
                    .first()
                    .map(|(path, message)| format!("{}: {message}", path.display()))
            })
    }
}

pub struct MetadataService {
    tx: Sender<MetadataRequest>,
    rx: Receiver<MetadataOutcome>,
}

impl MetadataService {
    pub fn new(repaint: Arc<dyn Fn() + Send + Sync>) -> Self {
        let (tx, requests) = channel::<MetadataRequest>();
        let (outcomes_tx, rx) = channel::<MetadataOutcome>();

        std::thread::Builder::new()
            .name("metadata".to_owned())
            .spawn(move || {
                while let Ok(request) = requests.recv() {
                    if outcomes_tx.send(execute(request)).is_err() {
                        break;
                    }
                    repaint();
                }
            })
            .expect("avvio del thread metadata");

        Self { tx, rx }
    }

    pub fn request(&self, request: MetadataRequest) {
        if self.tx.send(request).is_err() {
            tracing::error!("servizio metadati non disponibile");
        }
    }

    pub fn try_recv(&self) -> Option<MetadataOutcome> {
        self.rx.try_recv().ok()
    }
}

fn execute(request: MetadataRequest) -> MetadataOutcome {
    match request {
        MetadataRequest::ReadSummary { paths } => MetadataOutcome::Summary {
            items: read_summary(&paths),
        },
        MetadataRequest::ReadFull { paths } => MetadataOutcome::Full {
            items: read_full(&paths),
        },
        MetadataRequest::Write { items } => MetadataOutcome::Written {
            report: write_items(&items),
        },
    }
}

fn ha_errore(map: &crate::exiftool::TagMap) -> Option<String> {
    map.get("Error").map(|value| match value {
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    })
}

fn read_summary(paths: &[PathBuf]) -> Vec<(PathBuf, MetadataSummary)> {
    let mut items = Vec::new();
    let tool = match tool() {
        Ok(tool) => tool,
        Err(err) => {
            tracing::warn!("{err}");
            return items;
        }
    };

    for chunk in paths.chunks(SUMMARY_CHUNK) {
        match tool.read(chunk, SUMMARY_TAGS) {
            Ok(maps) => {
                for (path, map) in chunk.iter().zip(maps.iter()) {
                    if let Some(error) = ha_errore(map) {
                        tracing::warn!(path = %path.display(), "metadati non letti: {error}");
                        continue;
                    }
                    items.push((path.clone(), summary_from_tags(map)));
                }
            }
            Err(err) => tracing::warn!("lettura metadati non riuscita: {err}"),
        }
    }
    items
}

fn read_full(paths: &[PathBuf]) -> Vec<(PathBuf, Result<FullDocument, String>)> {
    let mut items = Vec::new();
    let tool = match tool() {
        Ok(tool) => tool,
        Err(err) => {
            let message = err.to_string();
            return paths
                .iter()
                .map(|path| (path.clone(), Err(message.clone())))
                .collect();
        }
    };

    for chunk in paths.chunks(FULL_CHUNK) {
        match tool.read(chunk, &[]) {
            Ok(maps) => {
                for (path, map) in chunk.iter().zip(maps.iter()) {
                    if let Some(error) = ha_errore(map) {
                        items.push((path.clone(), Err(error)));
                        continue;
                    }
                    items.push((
                        path.clone(),
                        Ok(FullDocument {
                            document: document_from_tags(map),
                            raw: raw_tags_from(map),
                        }),
                    ));
                }
            }
            Err(err) => {
                let message = err.to_string();
                for path in chunk {
                    items.push((path.clone(), Err(message.clone())));
                }
            }
        }
    }
    items
}

fn normalizza_confronto(field: MetadataField, value: &str) -> String {
    if field.is_list() {
        value
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        value.trim().to_owned()
    }
}

fn write_items(items: &[WriteItem]) -> MetadataWriteReport {
    let mut report = MetadataWriteReport::default();
    let tool = match tool() {
        Ok(tool) => tool,
        Err(err) => {
            let message = err.to_string();
            for item in items {
                report.failed.push((item.path.clone(), message.clone()));
            }
            return report;
        }
    };

    for item in items {
        let capabilities = item.format.capabilities();
        let sidecar = !capabilities.embedded_metadata;
        if sidecar && !capabilities.xmp_sidecar {
            report.skipped.push((
                item.path.clone(),
                format!(
                    "{} non supporta la scrittura dei metadati in questa fase",
                    item.format.label()
                ),
            ));
            continue;
        }
        let mut assignments = Vec::new();
        let mut saltate: Vec<&str> = Vec::new();
        let mut scritte: Vec<(MetadataField, Option<String>)> = Vec::new();
        let mut errore_validazione = None;

        for (field, value) in &item.edits {
            match changes_for(*field, value.as_deref(), item.format, sidecar) {
                Ok(changes) if changes.is_empty() => {
                    if value.is_some() {
                        saltate.push(field.label());
                    }
                }
                Ok(changes) => {
                    assignments.extend(changes);
                    scritte.push((*field, value.clone()));
                }
                Err(message) => {
                    errore_validazione = Some(format!("{}: {message}", field.label()));
                    break;
                }
            }
        }

        if let Some(message) = errore_validazione {
            report.failed.push((item.path.clone(), message));
            continue;
        }
        if assignments.is_empty() {
            if !saltate.is_empty() {
                report.skipped.push((
                    item.path.clone(),
                    format!("non applicabili al formato: {}", saltate.join(", ")),
                ));
            }
            continue;
        }
        if !saltate.is_empty() {
            report.skipped.push((
                item.path.clone(),
                format!("non applicabili al formato: {}", saltate.join(", ")),
            ));
        }

        let sidecar_file = sidecar_path(&item.path);
        let esito = if sidecar {
            if sidecar_file.is_file() {
                tool.update_sidecar(&sidecar_file, &assignments)
            } else {
                tool.create_sidecar(&item.path, &sidecar_file, &assignments)
            }
        } else {
            tool.write_embedded(&item.path, &assignments)
        };
        if let Err(err) = esito {
            report.failed.push((item.path.clone(), err.to_string()));
            continue;
        }

        // Verifica post-scrittura: rilegge il file (o il sidecar) e confronta
        // i valori effettivi con quelli richiesti.
        let target = if sidecar {
            sidecar_file
        } else {
            item.path.clone()
        };
        match tool.read(std::slice::from_ref(&target), &[]) {
            Ok(maps) => match maps.first() {
                Some(map) if ha_errore(map).is_none() => {
                    let document = document_from_tags(map);
                    for (field, value) in &scritte {
                        let atteso = normalizza_confronto(*field, value.as_deref().unwrap_or(""));
                        let effettivo = normalizza_confronto(*field, &document.display(*field));
                        if atteso != effettivo {
                            let atteso_testo = if atteso.is_empty() {
                                "(vuoto)"
                            } else {
                                &atteso
                            };
                            let effettivo_testo = if effettivo.is_empty() {
                                "(vuoto)".to_owned()
                            } else {
                                effettivo
                            };
                            report.unverified.push((
                                item.path.clone(),
                                format!(
                                    "{}: atteso «{atteso_testo}», letto «{effettivo_testo}»",
                                    field.label()
                                ),
                            ));
                        }
                    }
                    report.updated.push(item.path.clone());
                }
                _ => report.unverified.push((
                    item.path.clone(),
                    "rilettura di verifica non riuscita".to_owned(),
                )),
            },
            Err(err) => report.unverified.push((item.path.clone(), err.to_string())),
        }
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::time::{Duration, Instant};

    fn exiftool() -> Option<ExifTool> {
        ExifTool::detect()
    }

    fn crea_immagine(path: &Path) {
        let image = image::RgbImage::from_fn(64, 48, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])
        });
        image.save(path).unwrap();
    }

    fn attendi(service: &MetadataService) -> MetadataOutcome {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if let Some(outcome) = service.try_recv() {
                return outcome;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("esito metadati non ricevuto entro 20 secondi");
    }

    #[test]
    fn il_riassunto_di_un_batch_torna_ordinato() {
        let Some(tool) = exiftool() else {
            eprintln!("ExifTool assente: test saltato");
            return;
        };
        let temp = tempfile::tempdir().unwrap();
        let prima = temp.path().join("prima.jpg");
        let seconda = temp.path().join("seconda.jpg");
        crea_immagine(&prima);
        crea_immagine(&seconda);
        tool.write_embedded(
            &prima,
            &[
                "XMP-xmp:Rating=4".to_owned(),
                "XMP-dc:Subject=mare".to_owned(),
            ],
        )
        .unwrap();

        let service = MetadataService::new(Arc::new(|| {}));
        service.request(MetadataRequest::ReadSummary {
            paths: vec![prima.clone(), seconda.clone()],
        });
        match attendi(&service) {
            MetadataOutcome::Summary { items } => {
                assert_eq!(items.len(), 2);
                assert_eq!(items[0].0, prima);
                assert_eq!(items[0].1.rating, Some(4));
                assert_eq!(items[0].1.keywords.as_deref(), Some("mare"));
                assert_eq!(items[1].0, seconda);
                assert_eq!(items[1].1.rating, None);
            }
            _ => panic!("esito inatteso"),
        }
    }

    #[test]
    fn la_scrittura_verifica_i_valori_riletti() {
        let Some(_tool) = exiftool() else {
            eprintln!("ExifTool assente: test saltato");
            return;
        };
        let temp = tempfile::tempdir().unwrap();
        let foto = temp.path().join("foto.jpg");
        crea_immagine(&foto);

        let service = MetadataService::new(Arc::new(|| {}));
        service.request(MetadataRequest::Write {
            items: vec![WriteItem {
                path: foto.clone(),
                format: FormatKind::Jpeg,
                edits: vec![
                    (MetadataField::Title, Some("Titolo servizio".to_owned())),
                    (MetadataField::Rating, Some("5".to_owned())),
                    (MetadataField::Keywords, Some("mare, tramonto".to_owned())),
                ],
            }],
        });
        match attendi(&service) {
            MetadataOutcome::Written { report } => {
                assert_eq!(report.updated, vec![foto.clone()]);
                assert!(report.failed.is_empty(), "{:?}", report.failed);
                assert!(report.unverified.is_empty(), "{:?}", report.unverified);
            }
            _ => panic!("esito inatteso"),
        }

        service.request(MetadataRequest::ReadFull {
            paths: vec![foto.clone()],
        });
        match attendi(&service) {
            MetadataOutcome::Full { items } => {
                let (path, document) = &items[0];
                assert_eq!(*path, foto);
                let document = document.as_ref().unwrap();
                assert_eq!(
                    document.document.value(MetadataField::Title),
                    Some("Titolo servizio")
                );
                assert_eq!(
                    document.document.list(MetadataField::Keywords),
                    ["mare", "tramonto"]
                );
                assert_eq!(document.document.rating(), Some(5));
                assert!(document.raw.iter().any(|tag| tag.name == "Title"));
            }
            _ => panic!("esito inatteso"),
        }
    }

    #[test]
    fn una_scrittura_su_file_inesistente_finisce_nei_fallimenti() {
        let Some(_tool) = exiftool() else {
            eprintln!("ExifTool assente: test saltato");
            return;
        };
        let service = MetadataService::new(Arc::new(|| {}));
        service.request(MetadataRequest::Write {
            items: vec![WriteItem {
                path: PathBuf::from("/tmp/uberphoto-non-esiste-mai/foto.jpg"),
                format: FormatKind::Jpeg,
                edits: vec![(MetadataField::Title, Some("x".to_owned()))],
            }],
        });
        match attendi(&service) {
            MetadataOutcome::Written { report } => {
                assert!(!report.failed.is_empty());
                assert!(report.updated.is_empty());
            }
            _ => panic!("esito inatteso"),
        }
    }

    #[test]
    fn il_sidecar_viene_creato_per_i_formati_non_embedded() {
        let Some(_tool) = exiftool() else {
            eprintln!("ExifTool assente: test saltato");
            return;
        };
        let temp = tempfile::tempdir().unwrap();
        let raw = temp.path().join("scatto.dng");
        crea_immagine(&temp.path().join("scatto.png"));
        std::fs::rename(temp.path().join("scatto.png"), &raw).unwrap();

        let service = MetadataService::new(Arc::new(|| {}));
        service.request(MetadataRequest::Write {
            items: vec![WriteItem {
                path: raw.clone(),
                format: FormatKind::Dng,
                edits: vec![
                    (MetadataField::Title, Some("Scatto RAW".to_owned())),
                    (MetadataField::Orientation, Some("6".to_owned())),
                ],
            }],
        });
        match attendi(&service) {
            MetadataOutcome::Written { report } => {
                assert_eq!(report.updated, vec![raw.clone()]);
                assert!(report.failed.is_empty(), "{:?}", report.failed);
                assert_eq!(report.skipped.len(), 1, "l'orientamento va saltato");
                assert!(report.unverified.is_empty(), "{:?}", report.unverified);
            }
            _ => panic!("esito inatteso"),
        }
        let sidecar = sidecar_path(&raw);
        assert!(sidecar.is_file(), "il sidecar XMP deve esistere");
    }
}
