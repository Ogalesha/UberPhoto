//! Pannello destro — Inspector.
//!
//! Fase 1: la scheda "Informazioni" mostra i dati disponibili senza motore
//! metadati (nome, percorso, formato, dimensioni, capacità del formato).
//! Le altre schede sono predisposte e dichiarano esplicitamente la fase in
//! cui verranno implementate: nessuna funzionalità non verificata viene
//! presentata come disponibile.

use eframe::egui;
use uberphoto_core::PhotoAsset;

use crate::state::{AppState, InspectorTab, UiActions};
use crate::thumbnails::ThumbCache;
use crate::util::{format_bytes, format_time};

pub fn ui(ui: &mut egui::Ui, state: &mut AppState, thumbs: &ThumbCache, _actions: &mut UiActions) {
    ui.horizontal_wrapped(|ui| {
        for tab in InspectorTab::ALL {
            if ui
                .selectable_label(state.inspector_tab == tab, tab.label())
                .clicked()
            {
                state.inspector_tab = tab;
            }
        }
    });
    ui.separator();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| match state.inspector_tab {
            InspectorTab::Colori => placeholder(
                ui,
                "Regolazioni colore",
                "Slider di esposizione, contrasto, saturazione e gli altri controlli, \
                 anteprima non distruttiva, preset e undo/redo arriveranno con la Fase 5.",
            ),
            InspectorTab::Informazioni => informazioni(ui, state, thumbs),
            InspectorTab::Metadati => placeholder(
                ui,
                "Editor metadati EXIF / IPTC / XMP",
                "Lettura e scrittura dei metadati tramite ExifTool (incorporati nel file \
                 quando il formato lo consente, con sidecar XMP per i RAW) arriveranno \
                 con la Fase 3.",
            ),
            InspectorTab::Gps => placeholder(
                ui,
                "Posizione GPS",
                "La mappa OpenStreetMap con marker trascinabile e salvataggio delle \
                 coordinate arriverà con la Fase 4.",
            ),
            InspectorTab::Esportazione => placeholder(
                ui,
                "Export for Web",
                "Ridimensionamento, qualità JPEG, conversione in sRGB, embedding dei \
                 metadati e report di verifica arriveranno con la Fase 6.",
            ),
        });
}

fn placeholder(ui: &mut egui::Ui, title: &str, body: &str) {
    ui.add_space(12.0);
    ui.strong(title);
    ui.add_space(6.0);
    ui.weak(body);
}

fn informazioni(ui: &mut egui::Ui, state: &AppState, thumbs: &ThumbCache) {
    let selected = state.selected_assets();

    match selected.as_slice() {
        [] => {
            ui.add_space(8.0);
            ui.weak("Nessuna foto selezionata.");
            ui.add_space(6.0);
            ui.weak(
                "Seleziona una o più foto nel browser: i dettagli si aggiornano \
                 automaticamente.",
            );
        }
        [asset] => singola(ui, asset, thumbs),
        many => multipla(ui, many),
    }
}

fn singola(ui: &mut egui::Ui, asset: &PhotoAsset, thumbs: &ThumbCache) {
    egui::Grid::new("info_grid")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            row(ui, "Nome", &asset.file_name());
            row(ui, "Percorso", &asset.path.display().to_string());
            row(ui, "Formato", asset.format.label());
            row(ui, "Dimensione", &format_bytes(asset.byte_size));
            if let Some((width, height)) = thumbs.dims(&asset.path) {
                row(ui, "Dimensioni", &format!("{width} × {height} px"));
                let megapixel = (width as f64 * height as f64) / 1_000_000.0;
                row(ui, "Risoluzione", &format!("{megapixel:.1} MP"));
            }
            if let Some(modified) = asset.modified {
                row(ui, "Modificato", &format_time(modified));
            }
            if asset.is_raw() {
                row(ui, "RAW", "sì — l'originale resta di sola lettura");
            }
        });

    ui.add_space(10.0);
    let capabilities = asset.format.capabilities();
    ui.strong("Capacità del formato");
    ui.add_space(4.0);
    egui::Grid::new("capabilities_grid")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            capability_row(ui, "Anteprima integrata", capabilities.view);
            capability_row(ui, "Editing colore", capabilities.pixel_edit);
            capability_row(ui, "Metadati embedded", capabilities.embedded_metadata);
            capability_row(ui, "Sidecar XMP", capabilities.xmp_sidecar);
            capability_row(ui, "Formato di export", capabilities.export_target);
        });

    ui.add_space(10.0);
    ui.weak("Metadati EXIF/IPTC/XMP, valutazione e GPS saranno mostrati qui con la Fase 3.");
}

fn multipla(ui: &mut egui::Ui, assets: &[&PhotoAsset]) {
    ui.add_space(8.0);
    ui.strong(format!("{} foto selezionate", assets.len()));
    ui.add_space(6.0);

    let total_bytes: u64 = assets.iter().map(|asset| asset.byte_size).sum();
    ui.label(format!("Dimensione totale: {}", format_bytes(total_bytes)));

    let mut formats: Vec<&str> = assets.iter().map(|asset| asset.format.label()).collect();
    formats.sort_unstable();
    formats.dedup();
    ui.label(format!("Formati: {}", formats.join(", ")));

    ui.add_space(10.0);
    ui.weak(
        "L'editing multiplo (valori misti, modifica dei soli campi cambiati, \
         anteprima delle operazioni) arriverà con le fasi successive.",
    );
}

fn row(ui: &mut egui::Ui, key: &str, value: &str) {
    ui.label(egui::RichText::new(key).strong());
    ui.label(value);
    ui.end_row();
}

fn capability_row(ui: &mut egui::Ui, key: &str, supported: bool) {
    ui.label(egui::RichText::new(key).strong());
    if supported {
        ui.label("sì");
    } else {
        ui.weak("no");
    }
    ui.end_row();
}
