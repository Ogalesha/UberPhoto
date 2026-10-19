//! Decodifica immagini, orientamento EXIF, scalatura, servizio di caricamento
//! in background per miniature e anteprime e pipeline colore non distruttiva.
//!
//! L'editing visivo è una [`EditRecipe`]: un documento serializzabile che
//! descrive le regolazioni; i pixel dell'originale non vengono mai toccati.
//! L'export (Fase 6) applicherà la ricetta all'immagine completa.

mod decode;
mod edit;
mod edit_service;
mod orientation;
mod service;

pub use decode::{decode_bytes_rgba, load_image_rgba, LoadedImage};
pub use edit::{apply_recipe, apply_recipe_rgba, CropRect, EditField, EditRecipe, EditedPixels};
pub use edit_service::{EditResult, EditService, EditSource};
pub use orientation::apply_orientation;
pub use service::{ImageKind, ImageResult, ImageService, PREVIEW_MAX_DIM};
