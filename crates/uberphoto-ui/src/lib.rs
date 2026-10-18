//! Interfaccia grafica di UberPhoto (eframe/egui).
//!
//! Layout a tre pannelli ridimensionabili — esploratore cartelle (sinistra),
//! photo browser (centro), inspector (destra) — più una barra di stato.
//!
//! La griglia del browser è virtualizzata (`ScrollArea::show_rows`): con
//! migliaia di foto vengono disegnate soltanto le celle visibili. Miniature e
//! anteprime sono decodificate su thread di background in `uberphoto-image`
//! e recapitate alla UI tramite canali.

mod app;
mod browser;
mod explorer;
mod inspector;
mod preview;
mod state;
mod thumbnails;
mod util;

use eframe::egui;

pub use app::UberPhotoApp;

/// Icona ufficiale dell'applicazione (PNG RGBA incorporato nel binario).
pub const APP_ICON_PNG: &[u8] = include_bytes!("../../../assets/UberPhoto.png");

/// Icona pronta per `ViewportBuilder::with_icon`, ridotta a 256×256
/// (dimensione multipla di 4, come richiesto da `egui::IconData`).
pub fn app_icon() -> Option<egui::IconData> {
    let image = uberphoto_image::decode_bytes_rgba(APP_ICON_PNG, Some(256)).ok()?;
    Some(egui::IconData {
        rgba: image.rgba,
        width: image.width,
        height: image.height,
    })
}
