use std::path::Path;

/// Formato immagine riconosciuto da UberPhoto (derivato dall'estensione).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FormatKind {
    Jpeg,
    Png,
    Tiff,
    Webp,
    Gif,
    Bmp,
    Heic,
    Avif,
    Dng,
    Cr2,
    Cr3,
    Nef,
    Arw,
    Raf,
}

/// Capacità di un formato, distinte esplicitamente come richiesto dalla spec.
///
/// I flag descrivono ciò che la pipeline attuale sa fare; con l'avanzare delle
/// fasi (editor, motore metadati, export) verranno aggiornati e verificati.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// Anteprima/decodifica con il decoder integrato.
    pub view: bool,
    /// Editing colore non distruttivo applicabile al formato.
    pub pixel_edit: bool,
    /// Scrittura di metadati embedded affidabile.
    pub embedded_metadata: bool,
    /// Il formato richiede sidecar XMP (scrittura embedded non sicura).
    pub xmp_sidecar: bool,
    /// Il formato può essere usato come output di export.
    pub export_target: bool,
}

impl FormatKind {
    /// Riconosce il formato dall'estensione del percorso; `None` se non è
    /// un formato immagine gestito.
    pub fn from_path(path: &Path) -> Option<Self> {
        path.extension()
            .and_then(|ext| ext.to_str())
            .and_then(Self::from_extension)
    }

    pub fn from_extension(ext: &str) -> Option<Self> {
        let ext = ext.to_ascii_lowercase();
        Some(match ext.as_str() {
            "jpg" | "jpeg" | "jpe" => Self::Jpeg,
            "png" => Self::Png,
            "tif" | "tiff" => Self::Tiff,
            "webp" => Self::Webp,
            "gif" => Self::Gif,
            "bmp" => Self::Bmp,
            "heic" | "heif" => Self::Heic,
            "avif" => Self::Avif,
            "dng" => Self::Dng,
            "cr2" => Self::Cr2,
            "cr3" => Self::Cr3,
            "nef" => Self::Nef,
            "arw" => Self::Arw,
            "raf" => Self::Raf,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Jpeg => "JPEG",
            Self::Png => "PNG",
            Self::Tiff => "TIFF",
            Self::Webp => "WebP",
            Self::Gif => "GIF",
            Self::Bmp => "BMP",
            Self::Heic => "HEIC",
            Self::Avif => "AVIF",
            Self::Dng => "DNG",
            Self::Cr2 => "CR2",
            Self::Cr3 => "CR3",
            Self::Nef => "NEF",
            Self::Arw => "ARW",
            Self::Raf => "RAF",
        }
    }

    /// RAW fotografico: i dati originali della fotocamera non vanno mai
    /// riscritti; si usano sidecar e conversione.
    pub fn is_raw(self) -> bool {
        matches!(
            self,
            Self::Dng | Self::Cr2 | Self::Cr3 | Self::Nef | Self::Arw | Self::Raf
        )
    }

    pub fn capabilities(self) -> Capabilities {
        match self {
            Self::Jpeg | Self::Png | Self::Tiff | Self::Webp => Capabilities {
                view: true,
                pixel_edit: true,
                embedded_metadata: true,
                xmp_sidecar: false,
                export_target: true,
            },
            Self::Gif | Self::Bmp => Capabilities {
                view: true,
                pixel_edit: false,
                embedded_metadata: false,
                xmp_sidecar: false,
                export_target: false,
            },
            // Decodifica e scrittura saranno abilitate in fasi successive,
            // previa verifica delle librerie su ogni piattaforma.
            Self::Heic | Self::Avif => Capabilities {
                view: false,
                pixel_edit: false,
                embedded_metadata: false,
                xmp_sidecar: true,
                export_target: false,
            },
            Self::Dng | Self::Cr2 | Self::Cr3 | Self::Nef | Self::Arw | Self::Raf => Capabilities {
                view: false,
                pixel_edit: false,
                embedded_metadata: false,
                xmp_sidecar: true,
                export_target: false,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn riconosce_le_estensioni_senza_distinzione_di_maiuscole() {
        assert_eq!(
            FormatKind::from_path(&PathBuf::from("/tmp/foto.JPG")),
            Some(FormatKind::Jpeg)
        );
        assert_eq!(
            FormatKind::from_path(&PathBuf::from("a.tIfF")),
            Some(FormatKind::Tiff)
        );
        assert_eq!(FormatKind::from_path(&PathBuf::from("greco.txt")), None);
        assert_eq!(
            FormatKind::from_path(&PathBuf::from("senzaestensione")),
            None
        );
    }

    #[test]
    fn riconosce_i_raw() {
        assert!(FormatKind::Nef.is_raw());
        assert!(FormatKind::Cr3.is_raw());
        assert!(!FormatKind::Jpeg.is_raw());
    }

    #[test]
    fn i_jpeg_non_richiedono_sidecar() {
        let caps = FormatKind::Jpeg.capabilities();
        assert!(caps.view && caps.pixel_edit && caps.embedded_metadata);
        assert!(!caps.xmp_sidecar && caps.export_target);
    }

    #[test]
    fn i_raw_usano_il_sidecar_e_non_sono_riscrivibili() {
        let caps = FormatKind::Arw.capabilities();
        assert!(caps.xmp_sidecar);
        assert!(!caps.embedded_metadata && !caps.pixel_edit);
    }
}
