//! Report di esportazione: confronto tra metadati attesi e verificati.

use uberphoto_metadata::{document_from_tags, MetadataDocument, MetadataField, TagMap};

use crate::settings::{ExportFormat, ExportPreset, PrivacyOptions};

/// Campi editoriali verificati su ogni esportazione.
const CAMPI_VERIFICATI: [MetadataField; 6] = [
    MetadataField::Title,
    MetadataField::Description,
    MetadataField::Keywords,
    MetadataField::Creator,
    MetadataField::Copyright,
    MetadataField::Rating,
];

/// Esito verificato di un singolo file esportato.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportReport {
    pub source: std::path::PathBuf,
    pub output: std::path::PathBuf,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
    pub preset: ExportPreset,
    /// Campi presenti e identici all'originale.
    pub preserved: Vec<String>,
    /// Campi volutamente cambiati (orientamento, dimensioni, profilo, privacy).
    pub adjusted: Vec<String>,
    /// Gruppi non supportati dal formato di destinazione.
    pub unsupported: Vec<String>,
    /// Campi attesi ma assenti o diversi nel file esportato.
    pub lost: Vec<String>,
    pub warnings: Vec<String>,
}

impl ExportReport {
    /// Riga compatta per la UI.
    pub fn summary(&self) -> String {
        format!(
            "{} preservati · {} persi · {} non supportati",
            self.preserved.len(),
            self.lost.len(),
            self.unsupported.len()
        )
    }

    /// True se nessun campo atteso è andato perso.
    pub fn verified(&self) -> bool {
        self.lost.is_empty()
    }

    /// Nome del file esportato.
    pub fn file_name(&self) -> String {
        self.output
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// Parametri noti della verifica.
pub struct ReportInput<'a> {
    pub source: &'a std::path::Path,
    pub output: &'a std::path::Path,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
    pub preset: ExportPreset,
    pub privacy: PrivacyOptions,
    pub format: ExportFormat,
    pub converted_profile: bool,
    pub resized: bool,
    pub source_tags: &'a TagMap,
    pub output_tags: &'a TagMap,
    pub warnings: Vec<String>,
}

/// Confronta i metadati attesi con quelli riletti dal file esportato.
pub fn build_report(input: ReportInput<'_>) -> ExportReport {
    let originale = document_from_tags(input.source_tags);
    let esportato = document_from_tags(input.output_tags);
    let mut preserved = Vec::new();
    let mut adjusted = Vec::new();
    let mut unsupported = Vec::new();
    let mut lost = Vec::new();
    let mut warnings = input.warnings;

    let privacy_gps = input.preset == ExportPreset::Privacy && input.privacy.remove_gps;
    let privacy_autore = input.preset == ExportPreset::Privacy && input.privacy.remove_author;

    for campo in CAMPI_VERIFICATI {
        let atteso = if privacy_autore && campo == MetadataField::Creator {
            String::new()
        } else {
            valore(&originale, campo)
        };
        let attuale = valore(&esportato, campo);
        if atteso.is_empty() {
            continue;
        }
        if attuale == atteso {
            preserved.push(campo.label().to_owned());
        } else if attuale.is_empty() {
            lost.push(campo.label().to_owned());
        } else {
            lost.push(format!("{} (valore diverso)", campo.label()));
            warnings.push(format!(
                "{}: atteso \"{atteso}\", trovato \"{attuale}\"",
                campo.label()
            ));
        }
    }

    match (&originale.gps, &esportato.gps) {
        (Some(origine), Some(attuale)) => {
            let vicini = (origine.latitude - attuale.latitude).abs() < 1e-6
                && (origine.longitude - attuale.longitude).abs() < 1e-6;
            if privacy_gps {
                lost.push("Posizione GPS (doveva essere rimossa)".to_owned());
            } else if vicini {
                preserved.push("Posizione GPS".to_owned());
            } else {
                lost.push("Posizione GPS (coordinate diverse)".to_owned());
            }
        }
        (Some(_), None) if privacy_gps => adjusted.push("GPS rimosso".to_owned()),
        (Some(_), None) => lost.push("Posizione GPS".to_owned()),
        (None, Some(_)) => adjusted.push("GPS aggiunto".to_owned()),
        (None, None) => {}
    }

    if input.preset == ExportPreset::Privacy {
        if input.privacy.remove_serial {
            adjusted.push("Numeri di serie rimossi".to_owned());
        }
        if privacy_autore {
            adjusted.push("Autore/Proprietario rimossi".to_owned());
        }
    }

    for gruppo in input.format.unsupported_groups() {
        let prefisso = format!("{gruppo}:");
        if input
            .source_tags
            .keys()
            .any(|key| key.starts_with(&prefisso))
        {
            unsupported.push(format!(
                "{gruppo} non supportato dal formato {}",
                input.format.label()
            ));
        }
    }

    adjusted.push("Orientamento impostato a 1 (pixel raddrizzati)".to_owned());
    if input.resized {
        adjusted.push(format!(
            "Dimensioni ridimensionate a {}×{}",
            input.width, input.height
        ));
    }
    if input.converted_profile {
        adjusted.push("Profilo ICC convertito in sRGB".to_owned());
    }

    ExportReport {
        source: input.source.to_path_buf(),
        output: input.output.to_path_buf(),
        bytes: input.bytes,
        width: input.width,
        height: input.height,
        preset: input.preset,
        preserved,
        adjusted,
        unsupported,
        lost,
        warnings,
    }
}

fn valore(documento: &MetadataDocument, campo: MetadataField) -> String {
    if campo.is_list() {
        documento.list(campo).join(", ")
    } else {
        documento.value(campo).unwrap_or_default().to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn tags(pairs: &[(&str, &str)]) -> TagMap {
        let mut map = TagMap::new();
        for (chiave, valore) in pairs {
            map.insert((*chiave).to_owned(), Value::String((*valore).to_owned()));
        }
        map
    }

    #[test]
    fn il_report_riconosce_preservati_e_persi() {
        let source = tags(&[
            ("XMP-dc:Title", "Tramonto"),
            ("XMP-dc:Creator", "Mario Rossi"),
            ("IPTC:Keywords", "mare"),
        ]);
        let output = tags(&[("XMP-dc:Title", "Tramonto")]);
        let report = build_report(ReportInput {
            source: std::path::Path::new("/tmp/a.jpg"),
            output: std::path::Path::new("/tmp/b.jpg"),
            bytes: 10,
            width: 100,
            height: 50,
            preset: ExportPreset::FullMetadata,
            privacy: PrivacyOptions::default(),
            format: ExportFormat::Jpeg,
            converted_profile: false,
            resized: true,
            source_tags: &source,
            output_tags: &output,
            warnings: Vec::new(),
        });
        assert!(report.preserved.contains(&"Titolo".to_owned()));
        assert!(report.lost.contains(&"Autore".to_owned()));
        assert!(report.adjusted.iter().any(|riga| riga.contains("100×50")));
        assert!(!report.verified());
    }

    #[test]
    fn il_preset_privacy_considera_rimosso_il_gps() {
        let source = tags(&[
            ("XMP-dc:Title", "Tramonto"),
            ("EXIF:GPSLatitude", "45.0"),
            ("EXIF:GPSLongitude", "9.0"),
        ]);
        let output = tags(&[("XMP-dc:Title", "Tramonto")]);
        let report = build_report(ReportInput {
            source: std::path::Path::new("/tmp/a.jpg"),
            output: std::path::Path::new("/tmp/b.jpg"),
            bytes: 10,
            width: 100,
            height: 50,
            preset: ExportPreset::Privacy,
            privacy: PrivacyOptions::default(),
            format: ExportFormat::Jpeg,
            converted_profile: false,
            resized: false,
            source_tags: &source,
            output_tags: &output,
            warnings: Vec::new(),
        });
        assert!(report.adjusted.contains(&"GPS rimosso".to_owned()));
        assert!(report.verified(), "{:?}", report.lost);
    }
}
