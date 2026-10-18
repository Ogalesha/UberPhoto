//! Motore metadati (Fase 3).
//!
//! - adapter al processo esterno ExifTool (argomenti strutturati, mai
//!   interpolazione di shell);
//! - modello canonico [`MetadataDocument`] con mappatura esplicita verso i
//!   tag EXIF / IPTC / XMP;
//! - scrittura embedded per i formati che la supportano e sidecar XMP per i
//!   RAW, con verifica post-scrittura dei valori effettivi;
//! - servizio [`MetadataService`] su thread dedicato per non bloccare la UI.

mod error;
mod exiftool;
mod model;
mod service;

pub use error::MetadataError;
pub use exiftool::{sidecar_path, ExifTool, TagMap};
pub use model::{
    changes_for, document_from_tags, raw_tags_from, summary_from_tags, GpsPosition,
    MetadataDocument, MetadataField, MetadataSummary, RawTag, Section, TechnicalInfo,
};
pub use service::{
    exiftool_disponibile, exiftool_versione, FullDocument, MetadataOutcome, MetadataRequest,
    MetadataService, MetadataWriteReport, WriteItem,
};
