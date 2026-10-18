use std::path::PathBuf;
use std::time::SystemTime;

use crate::FormatKind;

/// Una fotografia presente sul filesystem.
///
/// Fase 1: contenitore leggero popolato dalla scansione delle cartelle.
/// Le fasi successive lo arricchiranno con rating, keywords, stato GPS e
/// riferimenti all'`EditRecipe` (sempre senza duplicare lo stato del
/// filesystem, che resta l'autorità).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhotoAsset {
    pub path: PathBuf,
    pub format: FormatKind,
    pub byte_size: u64,
    pub modified: Option<SystemTime>,
}

impl PhotoAsset {
    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    pub fn is_raw(&self) -> bool {
        self.format.is_raw()
    }
}
