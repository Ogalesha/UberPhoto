#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui;
use tracing_subscriber::EnvFilter;

/// Directory where UberPhoto stores the index, settings, presets and cache.
///
/// It must stay aligned with the `app_id` passed to `eframe::run_native` and
/// used by `UberPhotoApp`: uninstallation removes exactly this directory.
const APP_ID: &str = "UberPhoto";

fn main() -> eframe::Result {
    if std::env::args().skip(1).any(|arg| arg == "--uninstall") {
        if let Err(err) = purge_user_data() {
            eprintln!("Uninstall failed: {err}");
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
        tracing::warn!("application icon could not be decoded");
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

/// Removes all user data created by the application: SQLite index,
/// settings, presets, history and map cache.
///
/// Program files are removed by the system uninstaller (`apt remove`, the
/// Windows uninstaller, deleting the macOS bundle).
fn purge_user_data() -> std::io::Result<()> {
    let Some(dir) = user_data_dir() else {
        println!("No user data directory to remove.");
        return Ok(());
    };
    if !dir.exists() {
        println!("No user data to remove in {}", dir.display());
        return Ok(());
    }
    std::fs::remove_dir_all(&dir)?;
    println!("User data removed: {}", dir.display());
    Ok(())
}

fn user_data_dir() -> Option<PathBuf> {
    let dir = eframe::storage_dir(APP_ID)?;

    // On Windows `eframe` appends a `data` subdirectory to the app directory.
    // Remove the app root as well so the portable build leaves nothing behind.
    #[cfg(windows)]
    if dir.file_name().is_some_and(|name| name == "data") {
        if let Some(parent) = dir.parent() {
            if parent.file_name().is_some_and(|name| name == APP_ID) {
                return Some(parent.to_path_buf());
            }
        }
    }

    Some(dir)
}
