//! Modelli di dominio di UberPhoto, indipendenti da UI e piattaforma.
//!
//! Fase 1: [`PhotoAsset`] e [`FormatKind`]. Fase 4: [`GpsLocation`].
//! Le fasi successive aggiungono qui i tipi canonici `EditRecipe` ed
//! `ExportProfile`.

mod asset;
mod format;
mod gps;

pub use asset::PhotoAsset;
pub use format::{Capabilities, FormatKind};
pub use gps::{GpsError, GpsLocation};
