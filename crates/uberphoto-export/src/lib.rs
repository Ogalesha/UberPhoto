//! Export for Web (Fase 6).
//!
//! Compone la pipeline non distruttiva di `uberphoto-image` con la copia dei
//! metadati tramite ExifTool e produce un report verificato rileggendo il file
//! esportato. UberPhoto dichiara solo ciò che scrive e riverifica.

mod error;
mod icc;
mod render;
mod report;
mod save;
mod service;
mod settings;

pub use error::ExportError;
pub use icc::{converti_in_srgb, profilo_srgb};
pub use render::{render_export, RenderOutput};
pub use report::ExportReport;
pub use save::{SaveEvent, SaveJob, SaveMode, SaveOutcome, SaveRequest, SaveService};
pub use service::{ExportEvent, ExportItem, ExportJob, ExportService};
pub use settings::{
    ExportFormat, ExportPreset, ExportSettings, NameMode, Naming, PrivacyOptions, ResizeRules,
};
