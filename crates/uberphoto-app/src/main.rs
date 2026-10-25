#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui;
use tracing_subscriber::EnvFilter;

/// Cartella in cui UberPhoto conserva indice, impostazioni, preset e cache.
///
/// Deve restare allineata all'`app_id` passato a `eframe::run_native` e usato
/// da `UberPhotoApp`: la disinstallazione rimuove proprio questa cartella.
const APP_ID: &str = "UberPhoto";

fn main() -> eframe::Result {
    if std::env::args().skip(1).any(|arg| arg == "--uninstall") {
        if let Err(err) = purge_user_data() {
            eprintln!("Disinstallazione non riuscita: {err}");
            std::process::exit(1);
        }
        return Ok(());
    }

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
        APP_ID,
        options,
        Box::new(|cc| Ok(Box::new(uberphoto_ui::UberPhotoApp::new(cc)))),
    )
}

/// Rimuove tutti i dati utente creati dall'applicazione: indice SQLite,
/// impostazioni, preset, cronologia e cache delle mappe.
///
/// I file del programma vengono rimossi dal disinstallatore del sistema
/// (`apt remove`, l'uninstaller di Windows, eliminazione del bundle macOS).
fn purge_user_data() -> std::io::Result<()> {
    let Some(dir) = user_data_dir() else {
        println!("Nessuna cartella dati utente da rimuovere.");
        return Ok(());
    };
    if !dir.exists() {
        println!("Nessun dato utente da rimuovere in {}", dir.display());
        return Ok(());
    }
    std::fs::remove_dir_all(&dir)?;
    println!("Dati utente rimossi: {}", dir.display());
    Ok(())
}

fn user_data_dir() -> Option<PathBuf> {
    eframe::storage_dir(APP_ID)
}
