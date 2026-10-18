//! Modelli di dominio di UberPhoto, indipendenti da UI e piattaforma.
//!
//! Fase 1: [`PhotoAsset`] e [`FormatKind`].
//!
//! Le fasi successive aggiungono qui i tipi canonici `MetadataDocument`,
//! `GpsLocation`, `EditRecipe` ed `ExportProfile`.

mod asset;
mod format;

pub use asset::PhotoAsset;
pub use format::{Capabilities, FormatKind};
