//! Decodifica immagini, orientamento EXIF, scalatura e servizio di caricamento
//! in background per miniature e anteprime.
//!
//! Fase 1: pipeline di sola lettura. La pipeline colore non distruttiva
//! (`EditRecipe`) e l'export arriveranno con le Fasi 5 e 6.

mod decode;
mod orientation;
mod service;

pub use decode::{decode_bytes_rgba, load_image_rgba, LoadedImage};
pub use orientation::apply_orientation;
pub use service::{ImageKind, ImageResult, ImageService, PREVIEW_MAX_DIM};
