//! Servizio metadati: letture e scritture ExifTool su un thread dedicato.
//!
//! Le richieste seguono lo stesso schema degli altri servizi (scansione,
//! miniature, operazioni file): canale mpsc, nessun blocco della UI e
//! richiesta di repaint al termine.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, OnceLock};

use uberphoto_core::{FormatKind, GpsLocation};

use crate::error::MetadataError;
use crate::exiftool::{sidecar_path, ExifTool, TagMap};
use crate::model::{
    assignments_for, document_from_tags, raw_tags_from, sidecar_snapshot_assignments,
    summary_from_document, LocationField, LocationRole, MetadataDocument, MetadataEdit,
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
    "XMP-exif:GPSLatitude",
    "XMP-exif:GPSLongitude",
    "XMP-exif:GPSAltitude",
    "XMP-exif:GPSAltitudeRef",
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
    pub edits: Vec<MetadataEdit>,
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
    tx: Sender<(u64, MetadataRequest)>,
    rx: Receiver<(u64, MetadataOutcome)>,
    read_epoch: Arc<AtomicU64>,
    /// True mentre il worker sta servendo una richiesta (lettura o scrittura).
    busy: Arc<AtomicBool>,
}

impl MetadataService {
    pub fn new(repaint: Arc<dyn Fn() + Send + Sync>) -> Self {
        let (tx, requests) = channel::<(u64, MetadataRequest)>();
        let (outcomes_tx, rx) = channel::<(u64, MetadataOutcome)>();
        let busy = Arc::new(AtomicBool::new(false));
        let worker_busy = Arc::clone(&busy);
        let read_epoch = Arc::new(AtomicU64::new(0));
        let worker_epoch = read_epoch.clone();

        std::thread::Builder::new()
            .name("metadata".to_owned())
            .spawn(move || {
                while let Ok((epoch, request)) = requests.recv() {
                    if !matches!(&request, MetadataRequest::Write { .. })
                        && epoch != worker_epoch.load(Ordering::SeqCst)
                    {
                        continue;
                    }
                    worker_busy.store(true, Ordering::Relaxed);
                    if outcomes_tx.send((epoch, execute(request))).is_err() {
                        worker_busy.store(false, Ordering::Relaxed);
                        break;
                    }
                    worker_busy.store(false, Ordering::Relaxed);
                    repaint();
                }
            })
            .expect("avvio del thread metadata");

        Self {
            tx,
            rx,
            busy,
            read_epoch,
        }
    }

    pub fn request(&self, request: MetadataRequest) {
        if self
            .tx
            .send((self.read_epoch.load(Ordering::SeqCst), request))
            .is_err()
        {
            tracing::error!("servizio metadati non disponibile");
        }
    }

    /// True se il worker sta elaborando una richiesta.
    pub fn busy(&self) -> bool {
        self.busy.load(Ordering::Relaxed)
    }

    /// Invalidates pending reads without dropping completed writes.
    pub fn invalidate_reads(&self) {
        self.read_epoch.fetch_add(1, Ordering::SeqCst);
    }

    pub fn try_recv(&self) -> Option<MetadataOutcome> {
        while let Ok((epoch, outcome)) = self.rx.try_recv() {
            if matches!(&outcome, MetadataOutcome::Written { .. })
                || epoch == self.read_epoch.load(Ordering::SeqCst)
            {
                return Some(outcome);
            }
        }
        None
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
            report: write_edits(&items),
        },
    }
}

fn ha_errore(map: &crate::exiftool::TagMap) -> Option<String> {
    map.get("Error")
        .or_else(|| map.get("ExifTool:Error"))
        .map(|value| match value {
            serde_json::Value::String(text) => text.clone(),
            other => other.to_string(),
        })
}

/// An app-managed sidecar holds a complete editable snapshot, including
/// deliberately absent values. Foreign sidecars remain partial overlays.
const SIDECAR_CREATOR: &str = "UberPhoto";

fn managed_sidecar(tags: &TagMap) -> bool {
    tags.get("XMP-xmp:CreatorTool")
        .and_then(serde_json::Value::as_str)
        == Some(SIDECAR_CREATOR)
}

fn existing_sidecar(image: &Path) -> std::io::Result<Option<PathBuf>> {
    for candidate in [sidecar_path(image), image.with_extension("xmp")] {
        match std::fs::symlink_metadata(&candidate) {
            Ok(_) if candidate.is_file() => return Ok(Some(candidate)),
            Ok(_) => return Err(std::io::Error::other("il sidecar non è un file regolare")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(None)
}

fn one_map(tool: &ExifTool, path: &Path) -> Result<TagMap, String> {
    let maps = tool
        .read(&[path.to_path_buf()], &[])
        .map_err(|error| error.to_string())?;
    let map = maps
        .into_iter()
        .next()
        .ok_or("ExifTool non ha restituito il documento richiesto")?;
    if let Some(error) = ha_errore(&map) {
        return Err(error);
    }
    Ok(map)
}

fn read_effective(
    tool: &ExifTool,
    paths: &[PathBuf],
    tags: &[&str],
) -> Vec<(PathBuf, Result<FullDocument, String>)> {
    let images = match tool.read(paths, tags) {
        Ok(maps) if maps.len() == paths.len() => maps,
        Ok(_) => {
            return paths
                .iter()
                .map(|path| (path.clone(), Err("lettura metadati incompleta".to_owned())))
                .collect()
        }
        Err(error) => {
            return paths
                .iter()
                .map(|path| (path.clone(), Err(error.to_string())))
                .collect()
        }
    };
    let mut errors = HashMap::new();
    let mut sidecar_paths = Vec::new();
    let mut sidecar_indices = Vec::new();
    for (index, path) in paths.iter().enumerate() {
        if FormatKind::from_path(path).is_some_and(|kind| kind.capabilities().xmp_sidecar) {
            match existing_sidecar(path) {
                Ok(Some(sidecar)) => {
                    sidecar_indices.push(index);
                    sidecar_paths.push(sidecar);
                }
                Ok(None) => {}
                Err(error) => {
                    errors.insert(index, error.to_string());
                }
            }
        }
    }
    let mut sidecars = HashMap::new();
    if !sidecar_paths.is_empty() {
        match tool.read(&sidecar_paths, &[]) {
            Ok(maps) if maps.len() == sidecar_paths.len() => {
                sidecars.extend(sidecar_indices.iter().copied().zip(maps));
            }
            result => {
                let message = match result {
                    Err(error) => error.to_string(),
                    Ok(_) => "lettura sidecar incompleta".to_owned(),
                };
                for index in sidecar_indices {
                    errors.insert(index, message.clone());
                }
            }
        }
    }
    paths
        .iter()
        .cloned()
        .zip(images)
        .enumerate()
        .map(|(index, (path, mut map))| {
            let result = (|| {
                if let Some(error) = errors.remove(&index).or_else(|| ha_errore(&map)) {
                    return Err(error);
                }
                let mut document = document_from_tags(&map);
                if let Some(sidecar) = sidecars.remove(&index) {
                    if let Some(error) = ha_errore(&sidecar) {
                        return Err(error);
                    }
                    let canonical = managed_sidecar(&sidecar);
                    document.apply_sidecar(&document_from_tags(&sidecar), canonical);
                    if canonical {
                        map.retain(|key, _| {
                            !key.starts_with("XMP-") && !key.starts_with("Composite:GPS")
                        });
                    }
                    map.extend(sidecar.into_iter().filter(|(key, _)| {
                        key.starts_with("XMP-") || key.starts_with("Composite:GPS")
                    }));
                }
                Ok(FullDocument {
                    document,
                    raw: raw_tags_from(&map),
                })
            })();
            (path, result)
        })
        .collect()
}

fn read_summary(paths: &[PathBuf]) -> Vec<(PathBuf, MetadataSummary)> {
    let Ok(tool) = tool() else {
        return Vec::new();
    };
    let mut items = Vec::new();
    for chunk in paths.chunks(SUMMARY_CHUNK) {
        for (path, result) in read_effective(tool, chunk, SUMMARY_TAGS) {
            match result {
                Ok(full) => items.push((path, summary_from_document(&full.document))),
                Err(error) => tracing::warn!(path = %path.display(), "metadati non letti: {error}"),
            }
        }
    }
    items
}

fn read_full(paths: &[PathBuf]) -> Vec<(PathBuf, Result<FullDocument, String>)> {
    let tool = match tool() {
        Ok(tool) => tool,
        Err(error) => {
            return paths
                .iter()
                .map(|path| (path.clone(), Err(error.to_string())))
                .collect()
        }
    };
    paths
        .chunks(FULL_CHUNK)
        .flat_map(|chunk| read_effective(tool, chunk, &[]))
        .collect()
}

fn sidecar_assignments(
    tool: &ExifTool,
    image: &Path,
    previous: Option<&Path>,
    format: FormatKind,
    edits: &[String],
) -> Result<Vec<String>, String> {
    let sidecar = previous.map(|path| one_map(tool, path)).transpose()?;
    if sidecar.as_ref().is_some_and(managed_sidecar) {
        return Ok(edits.to_vec());
    }
    let original = one_map(tool, image)?;
    let mut document = document_from_tags(&original);
    if let Some(sidecar) = &sidecar {
        document.apply_sidecar(&document_from_tags(sidecar), false);
    }
    let mut assignments = sidecar_snapshot_assignments(
        &document,
        format,
        sidecar.as_ref().unwrap_or(&original),
        &original,
    )?;
    assignments.extend_from_slice(edits);
    assignments.push(format!("XMP-xmp:CreatorTool={SIDECAR_CREATOR}"));
    Ok(assignments)
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

/// Confronto tra valori attesi e riletti; tolleranze per i valori numerici
/// che ExifTool converte in frazioni razionali (DMS, altitudine).
fn gps_conferma(
    atteso: Option<GpsLocation>,
    effettivo: Option<crate::model::GpsPosition>,
) -> Result<(), String> {
    match (atteso, effettivo) {
        (None, None) => Ok(()),
        (None, Some(effettivo)) => Err(format!(
            "atteso «(nessuna coordinata)», letto «{:.6}, {:.6}»",
            effettivo.latitude, effettivo.longitude
        )),
        (Some(atteso), None) => Err(format!(
            "atteso «{:.6}, {:.6}», letto «(nessuna coordinata)»",
            atteso.latitude, atteso.longitude
        )),
        (Some(atteso), Some(effettivo)) => {
            let lat_ok = (atteso.latitude - effettivo.latitude).abs() <= 5e-6;
            let lon_ok = (atteso.longitude - effettivo.longitude).abs() <= 5e-6;
            if !lat_ok || !lon_ok {
                return Err(format!(
                    "atteso «{:.6}, {:.6}», letto «{:.6}, {:.6}»",
                    atteso.latitude, atteso.longitude, effettivo.latitude, effettivo.longitude
                ));
            }
            let alt_ok = match (atteso.altitude, effettivo.altitude) {
                (None, None) => true,
                (Some(atteso), Some(effettivo)) => (atteso - effettivo).abs() <= 0.05,
                _ => false,
            };
            if alt_ok {
                Ok(())
            } else {
                let testo = |altitude: Option<f64>| match altitude {
                    Some(value) => format!("{value:.1} m"),
                    None => "(nessuna)".to_owned(),
                };
                Err(format!(
                    "altitudine: atteso «{}», letto «{}»",
                    testo(atteso.altitude),
                    testo(effettivo.altitude)
                ))
            }
        }
    }
}

fn location_conferma(
    document: &MetadataDocument,
    role: LocationRole,
    field: LocationField,
    atteso: Option<&str>,
) -> Result<(), String> {
    // Il codice paese viene normalizzato in maiuscolo prima della scrittura.
    let atteso = atteso.map(str::trim).unwrap_or_default();
    let atteso = if field == LocationField::CountryCode {
        atteso.to_ascii_uppercase()
    } else {
        atteso.to_owned()
    };
    let info = match role {
        LocationRole::Created => &document.location_created,
        LocationRole::Shown => &document.location_shown,
    };
    let effettivo = info.display(field);
    if atteso == effettivo {
        Ok(())
    } else {
        Err(format!(
            "atteso «{}», letto «{}»",
            if atteso.is_empty() {
                "(vuoto)".to_owned()
            } else {
                atteso
            },
            if effettivo.is_empty() {
                "(vuoto)".to_owned()
            } else {
                effettivo
            }
        ))
    }
}

/// Scrive e riverifica un lotto di modifiche già validate.
///
/// È il punto di ingresso usato anche dal salvataggio manuale dei file, che
/// deve applicare i campi dopo aver rigenerato i pixel.
pub fn write_edits(items: &[WriteItem]) -> MetadataWriteReport {
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
        let mut saltate: Vec<String> = Vec::new();
        let mut scritte: Vec<MetadataEdit> = Vec::new();
        let mut errore_validazione = None;

        for edit in &item.edits {
            match assignments_for(edit, item.format, sidecar) {
                Ok(changes) if changes.is_empty() => {
                    saltate.push(edit.label());
                }
                Ok(changes) => {
                    assignments.extend(changes);
                    scritte.push(edit.clone());
                }
                Err(message) => {
                    errore_validazione = Some(format!("{}: {message}", edit.label()));
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

        let sidecar_file = match existing_sidecar(&item.path) {
            Ok(path) => path.unwrap_or_else(|| sidecar_path(&item.path)),
            Err(error) if sidecar => {
                report.failed.push((item.path.clone(), error.to_string()));
                continue;
            }
            Err(_) => sidecar_path(&item.path),
        };
        let destination = if sidecar {
            sidecar_file
        } else {
            item.path.clone()
        };
        let preparation = (|| -> std::io::Result<_> {
            let backup = match std::fs::symlink_metadata(&destination) {
                Ok(_) => Some(uberphoto_files::backup_file(&destination)?),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound && sidecar => None,
                Err(error) => return Err(error),
            };
            let staged = if let Some(backup) = &backup {
                uberphoto_files::StagedFile::copy(backup, &destination)?
            } else {
                uberphoto_files::StagedFile::new(&destination)?
            };
            Ok((staged, backup))
        })();
        let (staged, backup) = match preparation {
            Ok(value) => value,
            Err(error) => {
                report.failed.push((item.path.clone(), error.to_string()));
                continue;
            }
        };
        if sidecar {
            match sidecar_assignments(
                tool,
                &item.path,
                backup.as_deref(),
                item.format,
                &assignments,
            ) {
                Ok(complete) => assignments = complete,
                Err(error) => {
                    report.failed.push((item.path.clone(), error));
                    continue;
                }
            }
        }
        let esito = if sidecar && backup.is_none() {
            tool.create_sidecar(&item.path, staged.path(), &assignments)
        } else {
            tool.write_embedded(staged.path(), &assignments)
        };
        if let Err(err) = esito {
            report.failed.push((item.path.clone(), err.to_string()));
            continue;
        }

        // Verifica post-scrittura: rilegge il file (o il sidecar) e confronta
        // i valori effettivi con quelli richiesti.
        let unverified_before = report.unverified.len();
        match tool.read(&[staged.path().to_path_buf()], &[]) {
            Ok(maps) => match maps.first() {
                Some(map) if ha_errore(map).is_none() => {
                    let document = document_from_tags(map);
                    for edit in &scritte {
                        let esito = match edit {
                            MetadataEdit::Field(field, value) => {
                                let atteso =
                                    normalizza_confronto(*field, value.as_deref().unwrap_or(""));
                                let effettivo =
                                    normalizza_confronto(*field, &document.display(*field));
                                (atteso != effettivo).then(|| {
                                    let atteso_testo = if atteso.is_empty() {
                                        "(vuoto)".to_owned()
                                    } else {
                                        atteso
                                    };
                                    let effettivo_testo = if effettivo.is_empty() {
                                        "(vuoto)".to_owned()
                                    } else {
                                        effettivo
                                    };
                                    format!(
                                        "{}: atteso «{atteso_testo}», letto «{effettivo_testo}»",
                                        field.label()
                                    )
                                })
                            }
                            MetadataEdit::Gps(location) => gps_conferma(*location, document.gps)
                                .err()
                                .map(|message| format!("{}: {message}", edit.label())),
                            MetadataEdit::Location { role, field, value } => {
                                location_conferma(&document, *role, *field, value.as_deref())
                                    .err()
                                    .map(|message| format!("{}: {message}", edit.label()))
                            }
                        };
                        if let Some(message) = esito {
                            report.unverified.push((item.path.clone(), message));
                        }
                    }
                }
                _ => report.unverified.push((
                    item.path.clone(),
                    "rilettura di verifica non riuscita".to_owned(),
                )),
            },
            Err(err) => report.unverified.push((item.path.clone(), err.to_string())),
        }
        if report.unverified.len() == unverified_before {
            let commit = if let Some(backup) = &backup {
                staged.replace(&destination, backup)
            } else {
                staged.publish(&destination)
            };
            match commit {
                Ok(()) => report.updated.push(item.path.clone()),
                Err(error) => report.failed.push((item.path.clone(), error.to_string())),
            }
        }
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::time::{Duration, Instant};

    #[test]
    fn invalidating_reads_discards_old_results_but_keeps_write_reports() {
        let (tx, _requests) = channel();
        let (outcomes, rx) = channel();
        let service = MetadataService {
            tx,
            rx,
            busy: Arc::new(AtomicBool::new(false)),
            read_epoch: Arc::new(AtomicU64::new(0)),
        };
        outcomes
            .send((0, MetadataOutcome::Summary { items: Vec::new() }))
            .unwrap();
        outcomes
            .send((0, MetadataOutcome::Full { items: Vec::new() }))
            .unwrap();
        outcomes
            .send((
                0,
                MetadataOutcome::Written {
                    report: MetadataWriteReport::default(),
                },
            ))
            .unwrap();
        service.invalidate_reads();
        outcomes
            .send((1, MetadataOutcome::Summary { items: Vec::new() }))
            .unwrap();
        assert!(matches!(
            service.try_recv(),
            Some(MetadataOutcome::Written { .. })
        ));
        assert!(matches!(
            service.try_recv(),
            Some(MetadataOutcome::Summary { .. })
        ));
        assert!(service.try_recv().is_none());
    }

    fn raw_fixture(directory: &Path, tool: &ExifTool) -> PathBuf {
        let png = directory.join("fixture.png");
        crea_immagine(&png);
        let raw = directory.join("fixture.dng");
        tool.write_embedded(
            &png,
            &[
                "XMP-dc:Title=Embedded title".to_owned(),
                "XMP-dc:Subject=alpha".to_owned(),
                "XMP-dc:Subject+=with,comma".to_owned(),
                "XMP-xmp:Rating=3".to_owned(),
                "XMP-dc:Creator=Alice".to_owned(),
                "XMP-dc:Creator+=Bob".to_owned(),
                "XMP-xmp:Nickname=Preserve this unknown field".to_owned(),
                "EXIF:Make=Fixture camera".to_owned(),
                "EXIF:DateTimeOriginal=2024:01:02 03:04:05".to_owned(),
            ],
        )
        .unwrap();
        std::fs::rename(png, &raw).unwrap();
        raw
    }

    fn reread(path: &Path) -> MetadataDocument {
        read_full(&[path.to_path_buf()])
            .pop()
            .unwrap()
            .1
            .unwrap()
            .document
    }

    fn write_raw(path: &Path, edits: Vec<MetadataEdit>) {
        let report = write_edits(&[WriteItem {
            path: path.to_path_buf(),
            format: FormatKind::Dng,
            edits,
        }]);
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert!(report.unverified.is_empty(), "{:?}", report.unverified);
        assert_eq!(report.updated, [path]);
    }

    #[test]
    fn sidecar_roundtrip_preserves_original_lists_and_camera_data_then_keeps_deletions() {
        let Some(tool) = exiftool() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let raw = raw_fixture(dir.path(), &tool);
        let original = std::fs::read(&raw).unwrap();
        let gps = GpsLocation {
            latitude: -45.2,
            longitude: -9.8,
            altitude: Some(-15.0),
        };
        write_raw(
            &raw,
            vec![
                MetadataEdit::Field(MetadataField::Title, Some("Edited title".to_owned())),
                MetadataEdit::Field(MetadataField::Rating, Some("5".to_owned())),
                MetadataEdit::Gps(Some(gps)),
            ],
        );
        let document = reread(&raw);
        assert_eq!(document.value(MetadataField::Title), Some("Edited title"));
        assert_eq!(
            document.list(MetadataField::Keywords),
            ["alpha", "with,comma"]
        );
        assert_eq!(document.technical.make.as_deref(), Some("Fixture camera"));
        assert_eq!(
            document.value(MetadataField::DateOriginal),
            Some("2024:01:02 03:04:05")
        );
        assert!(gps_conferma(Some(gps), document.gps).is_ok());
        let summary = read_summary(std::slice::from_ref(&raw)).pop().unwrap().1;
        assert_eq!(summary.rating, Some(5));
        assert!(summary.has_gps);
        assert_eq!(summary.keywords.as_deref(), Some("alpha, with,comma"));
        let sidecar = sidecar_path(&raw);
        let sidecar_tags = one_map(&tool, &sidecar).unwrap();
        assert_eq!(
            sidecar_tags["XMP-xmp:Nickname"],
            "Preserve this unknown field"
        );
        assert!(managed_sidecar(&sidecar_tags));

        write_raw(
            &raw,
            vec![MetadataEdit::Field(
                MetadataField::Keywords,
                Some("changed, values".to_owned()),
            )],
        );
        assert_eq!(
            reread(&raw).list(MetadataField::Keywords),
            ["changed", "values"]
        );
        write_raw(
            &raw,
            vec![
                MetadataEdit::Field(MetadataField::Title, None),
                MetadataEdit::Field(MetadataField::Keywords, None),
                MetadataEdit::Field(MetadataField::DateOriginal, None),
                MetadataEdit::Field(MetadataField::Rating, None),
                MetadataEdit::Gps(None),
            ],
        );
        let document = reread(&raw);
        assert!(document.value(MetadataField::Title).is_none());
        assert!(document.list(MetadataField::Keywords).is_empty());
        assert!(document.value(MetadataField::DateOriginal).is_none());
        assert!(document.gps.is_none());
        let summary = read_summary(std::slice::from_ref(&raw)).pop().unwrap().1;
        assert!(!summary.has_gps);
        assert!(summary.rating.is_none());
        assert!(summary.keywords.is_none());
        assert!(summary.date_original.is_none());
        assert_eq!(std::fs::read(raw).unwrap(), original);
    }

    #[test]
    fn a_partial_legacy_sidecar_is_read_and_adopted_in_place_without_losing_unknown_tags() {
        let Some(tool) = exiftool() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let raw = raw_fixture(dir.path(), &tool);
        let original = std::fs::read(&raw).unwrap();
        let legacy = raw.with_extension("xmp");
        tool.create_sidecar(
            &raw,
            &legacy,
            &[
                "XMP-dc:Title=Legacy title".to_owned(),
                "XMP-dc:Subject=".to_owned(),
                "XMP-dc:Creator=".to_owned(),
                "XMP-xmp:CreatorTool=Foreign editor".to_owned(),
                "XMP-xmp:Nickname=Legacy unknown field".to_owned(),
            ],
        )
        .unwrap();
        let document = reread(&raw);
        assert_eq!(document.value(MetadataField::Title), Some("Legacy title"));
        assert_eq!(
            document.list(MetadataField::Keywords),
            ["alpha", "with,comma"]
        );
        write_raw(&raw, vec![MetadataEdit::Field(MetadataField::Title, None)]);
        assert!(!sidecar_path(&raw).exists());
        assert!(reread(&raw).value(MetadataField::Title).is_none());
        let tags = one_map(&tool, &legacy).unwrap();
        assert_eq!(tags["XMP-xmp:Nickname"], "Legacy unknown field");
        assert_eq!(tags["XMP-dc:Creator"], serde_json::json!(["Alice", "Bob"]));
        assert!(managed_sidecar(&tags));
        assert_eq!(std::fs::read(raw).unwrap(), original);
    }

    #[test]
    fn appended_sidecar_takes_precedence_and_writing_leaves_the_legacy_file_untouched() {
        let Some(tool) = exiftool() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let raw = raw_fixture(dir.path(), &tool);
        let legacy = raw.with_extension("xmp");
        let appended = sidecar_path(&raw);
        tool.create_sidecar(&raw, &legacy, &["XMP-dc:Title=Legacy title".to_owned()])
            .unwrap();
        tool.create_sidecar(
            &raw,
            &appended,
            &["XMP-dc:Title=Preferred title".to_owned()],
        )
        .unwrap();
        let legacy_bytes = std::fs::read(&legacy).unwrap();
        assert_eq!(
            reread(&raw).value(MetadataField::Title),
            Some("Preferred title")
        );
        write_raw(
            &raw,
            vec![MetadataEdit::Field(
                MetadataField::Title,
                Some("Updated title".to_owned()),
            )],
        );
        assert_eq!(
            reread(&raw).value(MetadataField::Title),
            Some("Updated title")
        );
        assert_eq!(std::fs::read(legacy).unwrap(), legacy_bytes);
    }

    #[test]
    fn verification_failure_never_commits_metadata_to_the_original() {
        if exiftool().is_none() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let photo = dir.path().join("photo.png");
        crea_immagine(&photo);
        let original = std::fs::read(&photo).unwrap();
        let report = write_edits(&[WriteItem {
            path: photo.clone(),
            format: FormatKind::Png,
            edits: vec![
                MetadataEdit::Field(MetadataField::Title, Some("First value".to_owned())),
                MetadataEdit::Field(MetadataField::Title, Some("Conflicting value".to_owned())),
            ],
        }]);
        assert!(!report.unverified.is_empty());
        assert!(report.updated.is_empty());
        assert_eq!(std::fs::read(photo).unwrap(), original);
    }

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
                    MetadataEdit::Field(MetadataField::Title, Some("Titolo servizio".to_owned())),
                    MetadataEdit::Field(MetadataField::Rating, Some("5".to_owned())),
                    MetadataEdit::Field(MetadataField::Keywords, Some("mare, tramonto".to_owned())),
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
                edits: vec![MetadataEdit::Field(
                    MetadataField::Title,
                    Some("x".to_owned()),
                )],
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
                    MetadataEdit::Field(MetadataField::Title, Some("Scatto RAW".to_owned())),
                    MetadataEdit::Field(MetadataField::Orientation, Some("6".to_owned())),
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

    #[test]
    fn le_coordinate_gps_vengono_scritte_e_verificate() {
        let Some(_tool) = exiftool() else {
            eprintln!("ExifTool assente: test saltato");
            return;
        };
        let temp = tempfile::tempdir().unwrap();
        let foto = temp.path().join("gps.jpg");
        crea_immagine(&foto);

        let posizione = GpsLocation::new(-33.8688, -70.6693, Some(-120.5)).unwrap();
        let service = MetadataService::new(Arc::new(|| {}));
        service.request(MetadataRequest::Write {
            items: vec![WriteItem {
                path: foto.clone(),
                format: FormatKind::Jpeg,
                edits: vec![MetadataEdit::Gps(Some(posizione))],
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
                let document = items[0].1.as_ref().unwrap();
                let gps = document.document.gps.unwrap();
                assert!((gps.latitude + 33.8688).abs() < 5e-6);
                assert!((gps.longitude + 70.6693).abs() < 5e-6);
                assert!((gps.altitude.unwrap() + 120.5).abs() < 0.05);
            }
            _ => panic!("esito inatteso"),
        }

        service.request(MetadataRequest::Write {
            items: vec![WriteItem {
                path: foto.clone(),
                format: FormatKind::Jpeg,
                edits: vec![MetadataEdit::Gps(None)],
            }],
        });
        match attendi(&service) {
            MetadataOutcome::Written { report } => {
                assert_eq!(report.updated, vec![foto.clone()]);
                assert!(report.unverified.is_empty(), "{:?}", report.unverified);
            }
            _ => panic!("esito inatteso"),
        }
        service.request(MetadataRequest::ReadFull {
            paths: vec![foto.clone()],
        });
        match attendi(&service) {
            MetadataOutcome::Full { items } => {
                assert!(items[0].1.as_ref().unwrap().document.gps.is_none());
            }
            _ => panic!("esito inatteso"),
        }
    }

    #[test]
    fn le_localita_vengono_scritte_e_verificate() {
        let Some(_tool) = exiftool() else {
            eprintln!("ExifTool assente: test saltato");
            return;
        };
        let temp = tempfile::tempdir().unwrap();
        let foto = temp.path().join("luogo.jpg");
        crea_immagine(&foto);

        let service = MetadataService::new(Arc::new(|| {}));
        service.request(MetadataRequest::Write {
            items: vec![WriteItem {
                path: foto.clone(),
                format: FormatKind::Jpeg,
                edits: vec![
                    MetadataEdit::Location {
                        role: LocationRole::Created,
                        field: LocationField::City,
                        value: Some("Sesto".to_owned()),
                    },
                    MetadataEdit::Location {
                        role: LocationRole::Created,
                        field: LocationField::CountryCode,
                        value: Some("it".to_owned()),
                    },
                    MetadataEdit::Location {
                        role: LocationRole::Shown,
                        field: LocationField::Place,
                        value: Some("Duomo".to_owned()),
                    },
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
                let document = &items[0].1.as_ref().unwrap().document;
                assert_eq!(
                    document.location_created.value(LocationField::City),
                    Some("Sesto")
                );
                assert_eq!(
                    document.location_created.value(LocationField::CountryCode),
                    Some("IT")
                );
                assert_eq!(
                    document.location_shown.value(LocationField::Place),
                    Some("Duomo")
                );
            }
            _ => panic!("esito inatteso"),
        }
    }
}
