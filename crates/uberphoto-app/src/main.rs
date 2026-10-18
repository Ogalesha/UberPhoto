#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;

use eframe::egui;
use tracing_subscriber::EnvFilter;

fn main() -> eframe::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("UberPhoto")
        .with_inner_size([1440.0, 900.0])
        .with_min_inner_size([960.0, 600.0]);

    if let Some(icon) = uberphoto_ui::app_icon() {
        viewport = viewport.with_icon(Arc::new(icon));
    } else {
        tracing::warn!("icona dell'applicazione non decodificabile");
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "UberPhoto",
        options,
        Box::new(|cc| Ok(Box::new(uberphoto_ui::UberPhotoApp::new(cc)))),
    )
}
