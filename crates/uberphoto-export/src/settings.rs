//! Parametri dell'esportazione e regole di denominazione.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uberphoto_core::FormatKind;

/// Formati di output offerti da Export for Web.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ExportFormat {
    #[default]
    Jpeg,
    Png,
    Webp,
    Tiff,
}

impl ExportFormat {
    pub const ALL: [ExportFormat; 4] = [
        ExportFormat::Jpeg,
        ExportFormat::Png,
        ExportFormat::Webp,
        ExportFormat::Tiff,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ExportFormat::Jpeg => "JPEG",
            ExportFormat::Png => "PNG",
            ExportFormat::Webp => "WebP (senza perdita)",
            ExportFormat::Tiff => "TIFF",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Jpeg => "jpg",
            ExportFormat::Png => "png",
            ExportFormat::Webp => "webp",
            ExportFormat::Tiff => "tif",
        }
    }

    /// Solo il JPEG usa la qualità con perdita.
    pub fn supports_quality(self) -> bool {
        self == ExportFormat::Jpeg
    }

    /// Gruppi di metadati che il formato non può trasportare.
    pub fn unsupported_groups(self) -> &'static [&'static str] {
        match self {
            ExportFormat::Jpeg | ExportFormat::Tiff | ExportFormat::Png => &[],
            ExportFormat::Webp => &["IPTC"],
        }
    }

    pub fn image_format(self) -> image::ImageFormat {
        match self {
            ExportFormat::Jpeg => image::ImageFormat::Jpeg,
            ExportFormat::Png => image::ImageFormat::Png,
            ExportFormat::Webp => image::ImageFormat::WebP,
            ExportFormat::Tiff => image::ImageFormat::Tiff,
        }
    }
}

/// Preset di preservazione dei metadati.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ExportPreset {
    /// Conserva tutti i metadati tecnicamente compatibili.
    #[default]
    FullMetadata,
    /// Solo i campi editoriali scelti.
    WebOptimized,
    /// Rimuove GPS e altre informazioni sensibili selezionate.
    Privacy,
}

impl ExportPreset {
    pub const ALL: [ExportPreset; 3] = [
        ExportPreset::FullMetadata,
        ExportPreset::WebOptimized,
        ExportPreset::Privacy,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ExportPreset::FullMetadata => "Full Metadata",
            ExportPreset::WebOptimized => "Web Optimized",
            ExportPreset::Privacy => "Privacy",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            ExportPreset::FullMetadata => {
                "Conserva tutti i metadati compatibili con il formato scelto."
            }
            ExportPreset::WebOptimized => {
                "Conserva titolo, descrizione, keywords, autore, copyright e valutazione."
            }
            ExportPreset::Privacy => "Rimuove GPS e i dati sensibili selezionati.",
        }
    }
}

/// Opzioni del preset Privacy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivacyOptions {
    pub remove_gps: bool,
    pub remove_serial: bool,
    pub remove_author: bool,
}

impl Default for PrivacyOptions {
    fn default() -> Self {
        Self {
            remove_gps: true,
            remove_serial: true,
            remove_author: false,
        }
    }
}

impl PrivacyOptions {
    /// Esclusioni ExifTool (`--TAG`) corrispondenti alle opzioni attive.
    pub fn exclusions(self) -> Vec<&'static str> {
        let mut esclusioni = Vec::new();
        if self.remove_gps {
            // Esclude GPS e i compositi derivati (GPSLatitude, GPSPosition…).
            esclusioni.push("--GPS*");
        }
        if self.remove_serial {
            esclusioni.push("--*Serial*");
        }
        if self.remove_author {
            esclusioni.extend([
                "--OwnerName",
                "--Artist",
                "--XMP-dc:Creator",
                "--CreatorContactInfo",
            ]);
        }
        esclusioni
    }
}

/// Convenzione di rinomina del file esportato.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum NameMode {
    #[default]
    KeepName,
    Prefix,
    Suffix,
    WebSafe,
}

impl NameMode {
    pub const ALL: [NameMode; 4] = [
        NameMode::KeepName,
        NameMode::Prefix,
        NameMode::Suffix,
        NameMode::WebSafe,
    ];

    pub fn label(self) -> &'static str {
        match self {
            NameMode::KeepName => "Mantieni nome",
            NameMode::Prefix => "Prefisso",
            NameMode::Suffix => "Suffisso",
            NameMode::WebSafe => "Nome web-safe",
        }
    }
}

/// Rinomina applicata al file esportato.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Naming {
    pub mode: NameMode,
    pub prefix: String,
    pub suffix: String,
}

impl Default for Naming {
    fn default() -> Self {
        Self {
            mode: NameMode::KeepName,
            prefix: "web-".to_owned(),
            suffix: "-web".to_owned(),
        }
    }
}

impl Naming {
    /// Nome del file di output, senza gestione delle collisioni.
    pub fn file_name(&self, source: &Path, extension: &str) -> String {
        let stem = source
            .file_stem()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| "foto".to_owned());
        let stem = match self.mode {
            NameMode::KeepName => stem,
            NameMode::Prefix => format!("{}{stem}", self.prefix.trim()),
            NameMode::Suffix => format!("{stem}{}", self.suffix.trim()),
            NameMode::WebSafe => web_safe(&stem),
        };
        format!("{stem}.{extension}")
    }
}

/// Riduce un nome a caratteri sicuri per il web.
fn web_safe(stem: &str) -> String {
    let mut risultato = String::with_capacity(stem.len());
    let mut trattino = false;
    for carattere in stem.chars() {
        if carattere.is_ascii_alphanumeric() {
            risultato.push(carattere.to_ascii_lowercase());
            trattino = false;
        } else if !trattino {
            risultato.push('-');
            trattino = true;
        }
    }
    let pulito = risultato.trim_matches('-');
    if pulito.is_empty() {
        "foto".to_owned()
    } else {
        pulito.to_owned()
    }
}

/// Regole di ridimensionamento (mai ingrandisce).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResizeRules {
    pub enabled: bool,
    /// Larghezza massima in pixel (0 = nessun limite).
    pub max_width: u32,
    /// Altezza massima in pixel (0 = nessun limite).
    pub max_height: u32,
    pub keep_aspect: bool,
}

impl Default for ResizeRules {
    fn default() -> Self {
        Self {
            enabled: true,
            max_width: 2048,
            max_height: 2048,
            keep_aspect: true,
        }
    }
}

impl ResizeRules {
    /// Dimensioni finali per un'immagine sorgente; `None` se non cambia nulla.
    pub fn target_size(&self, width: u32, height: u32) -> Option<(u32, u32)> {
        if !self.enabled || width == 0 || height == 0 {
            return None;
        }
        let larghezza = (self.max_width > 0).then_some(self.max_width);
        let altezza = (self.max_height > 0).then_some(self.max_height);
        let (mut nuova_larghezza, mut nuova_altezza) = match (larghezza, altezza) {
            (Some(w), Some(h)) if !self.keep_aspect => (w, h),
            (Some(w), Some(h)) => {
                let scala = (w as f64 / width as f64).min(h as f64 / height as f64);
                (
                    (width as f64 * scala).round() as u32,
                    (height as f64 * scala).round() as u32,
                )
            }
            (Some(w), None) => {
                let scala = w as f64 / width as f64;
                (w, (height as f64 * scala).round().max(1.0) as u32)
            }
            (None, Some(h)) => {
                let scala = h as f64 / height as f64;
                ((width as f64 * scala).round().max(1.0) as u32, h)
            }
            (None, None) => return None,
        };
        nuova_larghezza = nuova_larghezza.max(1);
        nuova_altezza = nuova_altezza.max(1);
        if nuova_larghezza == width && nuova_altezza == height {
            return None;
        }
        if nuova_larghezza > width && nuova_altezza > height {
            return None; // niente ingrandimenti
        }
        Some((nuova_larghezza, nuova_altezza))
    }
}

/// Tutti i parametri dell'esportazione.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportSettings {
    /// Cartella di destinazione; se vuota si usa quella dell'originale.
    pub destination: PathBuf,
    pub format: ExportFormat,
    /// Qualità JPEG 1..=100.
    pub quality: u8,
    pub resize: ResizeRules,
    pub naming: Naming,
    pub preset: ExportPreset,
    pub privacy: PrivacyOptions,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            destination: PathBuf::new(),
            format: ExportFormat::Jpeg,
            quality: 90,
            resize: ResizeRules::default(),
            naming: Naming::default(),
            preset: ExportPreset::default(),
            privacy: PrivacyOptions::default(),
        }
    }
}

impl ExportSettings {
    /// Cartella effettiva per una sorgente.
    pub fn destination_for(&self, source: &Path) -> PathBuf {
        if self.destination.as_os_str().is_empty() {
            source
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."))
        } else {
            self.destination.clone()
        }
    }

    /// Formati accettati in input (tutti quelli visualizzabili).
    pub fn supported_source(kind: FormatKind) -> bool {
        matches!(
            kind,
            FormatKind::Jpeg
                | FormatKind::Png
                | FormatKind::Tiff
                | FormatKind::Webp
                | FormatKind::Gif
                | FormatKind::Bmp
                | FormatKind::Heic
                | FormatKind::Avif
                | FormatKind::Dng
                | FormatKind::Cr2
                | FormatKind::Cr3
                | FormatKind::Nef
                | FormatKind::Arw
                | FormatKind::Raf
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn il_ridimensionamento_mantiene_le_proporzioni_e_non_ingrandisce() {
        let regole = ResizeRules {
            enabled: true,
            max_width: 1000,
            max_height: 1000,
            keep_aspect: true,
        };
        assert_eq!(regole.target_size(4000, 3000), Some((1000, 750)));
        assert_eq!(regole.target_size(3000, 4000), Some((750, 1000)));
        assert_eq!(regole.target_size(800, 600), None);
        assert_eq!(regole.target_size(1000, 750), None);

        let solo_larghezza = ResizeRules {
            max_width: 500,
            max_height: 0,
            ..regole
        };
        assert_eq!(solo_larghezza.target_size(4000, 3000), Some((500, 375)));
    }

    #[test]
    fn la_rinomina_web_safe_normalizza_i_caratteri() {
        let naming = Naming {
            mode: NameMode::WebSafe,
            ..Naming::default()
        };
        assert_eq!(
            naming.file_name(Path::new("/foto/My Photo (1).JPG"), "jpg"),
            "my-photo-1.jpg"
        );
        assert_eq!(
            naming.file_name(Path::new("/foto/€€.jpg"), "jpg"),
            "foto.jpg"
        );

        let prefisso = Naming {
            mode: NameMode::Prefix,
            prefix: "web-".to_owned(),
            ..Naming::default()
        };
        assert_eq!(
            prefisso.file_name(Path::new("/foto/scatto.jpg"), "webp"),
            "web-scatto.webp"
        );
    }

    #[test]
    fn il_preset_privacy_toglie_gps_e_seriali() {
        let esclusioni = PrivacyOptions::default().exclusions();
        assert!(esclusioni.contains(&"--GPS*"));
        assert!(esclusioni.contains(&"--*Serial*"));
        assert!(!esclusioni.contains(&"--Artist"));
    }
}
