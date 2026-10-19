//! Errori dell'esportazione.

use std::fmt;

#[derive(Debug)]
pub enum ExportError {
    /// Errore di I/O su file o cartelle.
    Io(std::io::Error),
    /// Decodifica o codifica dell'immagine non riuscita.
    Image(String),
    /// Operazione ExifTool non riuscita.
    Metadata(uberphoto_metadata::MetadataError),
    /// Problema nel profilo colore.
    Color(String),
    /// Configurazione o verifica non valida.
    Invalid(String),
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
            Self::Image(err) => write!(f, "immagine: {err}"),
            Self::Metadata(err) => write!(f, "metadati: {err}"),
            Self::Color(err) => write!(f, "colore: {err}"),
            Self::Invalid(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for ExportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Metadata(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for ExportError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<uberphoto_metadata::MetadataError> for ExportError {
    fn from(err: uberphoto_metadata::MetadataError) -> Self {
        Self::Metadata(err)
    }
}
