//! Errori del motore metadati.

use std::fmt;

/// Errore delle operazioni ExifTool.
#[derive(Debug)]
pub enum MetadataError {
    /// ExifTool non è disponibile (né in PATH né via `UBERPHOTO_EXIFTOOL`).
    ToolMissing,
    /// Avvio del processo non riuscito.
    Spawn(std::io::Error),
    /// ExifTool è terminato segnalando un errore.
    Failed { code: Option<i32>, stderr: String },
    /// Output non interpretabile come JSON.
    Json(serde_json::Error),
}

impl fmt::Display for MetadataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ToolMissing => write!(
                f,
                "ExifTool non trovato: installalo e assicurati che sia nel PATH, \
                 oppure indica il percorso con la variabile UBERPHOTO_EXIFTOOL"
            ),
            Self::Spawn(err) => write!(f, "avvio di ExifTool non riuscito: {err}"),
            Self::Failed { code, stderr } => {
                write!(f, "ExifTool è terminato con un errore")?;
                if let Some(code) = code {
                    write!(f, " (codice {code})")?;
                }
                if !stderr.is_empty() {
                    write!(f, ": {stderr}")?;
                }
                Ok(())
            }
            Self::Json(err) => write!(f, "output di ExifTool non valido: {err}"),
        }
    }
}

impl std::error::Error for MetadataError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Spawn(err) => Some(err),
            Self::Json(err) => Some(err),
            _ => None,
        }
    }
}
