//! Scheda "Posizione GPS" — mappa OpenStreetMap, coordinate e località.
//!
//! - mappa interattiva con i tile di OpenStreetMap e marker trascinabile;
//!   un clic sulla mappa posiziona il marker;
//! - coordinate manuali validate (gradi decimali, virgola o punto);
//! - ricerca geografica opzionale su Nominatim, solo su azione esplicita,
//!   senza suggerimenti automatici mentre si digita;
//! - località di scatto e raffigurata secondo IPTC Extension.
//!
//! La mappa non contatta mai il server dei tile prima che la scheda venga
//! aperta: le [`walkers::HttpTiles`] vengono create alla prima apertura.

use std::path::PathBuf;

use eframe::egui;
use uberphoto_core::GpsLocation;
use uberphoto_metadata::{LocationField, LocationRole};

use crate::state::{AppState, GpsState, SuggestionTarget, UiActions};
use crate::util::elide;

const MAP_HEIGHT: f32 = 250.0;
const LARGE_MAP_HEIGHT: f32 = 420.0;

/// Scheda GPS completa.
pub fn ui(ui: &mut egui::Ui, state: &mut AppState, actions: &mut UiActions) {
    if state.exiftool.is_none() {
        ui.add_space(10.0);
        ui.strong("Posizione GPS non disponibile");
        ui.add_space(6.0);
        ui.weak(
            "ExifTool non è stato trovato. Installa exiftool (pacchetto \
             libimage-exiftool-perl su Ubuntu) oppure imposta la variabile \
             UBERPHOTO_EXIFTOOL con il percorso dell'eseguibile, poi riavvia \
             UberPhoto.",
        );
        return;
    }

    let selected = state.selected_paths();
    if selected.is_empty() {
        ui.add_space(8.0);
        ui.weak("Nessuna foto selezionata.");
        ui.add_space(6.0);
        ui.weak(
            "Seleziona una o più foto per leggerne la posizione e salvarla \
             nei metadati.",
        );
        return;
    }

    let pronti = selected
        .iter()
        .all(|path| state.meta.docs.contains_key(path));
    if !pronti {
        let errori: Vec<(String, String)> = selected
            .iter()
            .filter_map(|path| {
                let nome = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("file")
                    .to_owned();
                state
                    .meta
                    .errors
                    .get(path)
                    .map(|error| (nome, error.clone()))
            })
            .collect();
        for (nome, errore) in &errori {
            ui.colored_label(ui.visuals().error_fg_color, format!("{nome}: {errore}"));
        }
        if errori.is_empty() {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.weak("Lettura dei metadati…");
            });
        }
        ui.add_space(6.0);
        if ui.button("Ricarica").clicked() {
            actions.metadata_reload = true;
        }
        return;
    }

    ui.horizontal(|ui| {
        ui.strong(if selected.len() == 1 {
            "1 foto selezionata".to_owned()
        } else {
            format!("{} foto selezionate", selected.len())
        });
        if ui.small_button("Ricarica").clicked() {
            actions.metadata_reload = true;
        }
    });
    ui.add_space(4.0);

    mappa(ui, state, MAP_HEIGHT);
    coordinate(ui, state, actions, selected.len());
    ui.add_space(10.0);
    localita(ui, state, actions);
    ui.add_space(10.0);
    ricerca(ui, state, actions);

    if let Some(asset) = state.selected_assets().first() {
        if !asset.format.capabilities().embedded_metadata {
            ui.add_space(6.0);
            ui.weak(format!(
                "{} non supporta metadati incorporati: coordinate e località \
                 saranno scritte nel sidecar accanto al file.",
                asset.format.label()
            ));
        }
    }

    finestra_mappa_grande(ui, state);
}

/// Mappa interattiva con marker trascinabile.
fn mappa(ui: &mut egui::Ui, state: &mut AppState, height: f32) {
    let ctx = ui.ctx().clone();
    let GpsState {
        tiles,
        memory,
        my_position,
        marker,
        ..
    } = &mut state.gps;
    let marker = *marker;
    let my_position = *my_position;

    let tiles = tiles.get_or_insert_with(|| {
        let options = walkers::HttpOptions {
            cache: cache_tile(),
            user_agent: walkers::HeaderValue::from_str(&uberphoto_geo::default_user_agent()).ok(),
            ..Default::default()
        };
        walkers::HttpTiles::with_options(walkers::sources::OpenStreetMap, options, ctx)
    });
    let attribution = walkers::Tiles::attribution(tiles);

    let mut nuova: Option<Option<GpsLocation>> = None;
    let larghezza = ui.available_width();
    ui.allocate_ui(egui::vec2(larghezza, height), |ui| {
        let map = walkers::Map::new(Some(tiles), memory, my_position);
        map.show(ui, |ui, response, projector, _memory| {
            if response.clicked() {
                if let Some(pointer) = response.interact_pointer_pos() {
                    let position = projector.unproject(pointer.to_vec2());
                    nuova = Some(GpsLocation::new(position.y(), position.x(), None).ok());
                }
            }
            if let Some(marker) = marker {
                let schermo = projector
                    .project(walkers::lon_lat(marker.longitude, marker.latitude))
                    .to_pos2();
                let rect = egui::Rect::from_center_size(schermo, egui::vec2(24.0, 24.0));
                let drag = ui.interact(
                    rect,
                    egui::Id::unique("uberphoto-gps-marker"),
                    egui::Sense::drag(),
                );
                if drag.hovered() || drag.dragged() {
                    ui.ctx().set_cursor_icon(if drag.dragged() {
                        egui::CursorIcon::Grabbing
                    } else {
                        egui::CursorIcon::Grab
                    });
                }
                if drag.dragged() {
                    if let Some(pointer) = drag.interact_pointer_pos() {
                        let position = projector.unproject(pointer.to_vec2());
                        nuova = Some(
                            GpsLocation::new(position.y(), position.x(), marker.altitude).ok(),
                        );
                    }
                    ui.ctx().request_repaint();
                }
                disegna_marker(ui.painter(), schermo, drag.dragged());
            }
        });
    });
    if let Some(nuova) = nuova {
        state.set_gps_marker(nuova);
    }

    ui.horizontal(|ui| {
        if ui
            .add_enabled(marker.is_some(), egui::Button::new("Centra").small())
            .clicked()
        {
            state.center_gps_map(marker);
            state.gps.centered_for = marker;
        }
        if ui.small_button("Mappa grande").clicked() {
            state.gps.large_map = true;
        }
        ui.weak(format!("© {}", attribution.text));
        ui.hyperlink_to("Licenza", attribution.url);
    });
}

/// Marker della posizione: nodo trascinabile in stile editor OSM.
fn disegna_marker(painter: &egui::Painter, pos: egui::Pos2, dragging: bool) {
    let raggio = if dragging { 9.5 } else { 8.0 };
    let ombra = egui::Color32::from_black_alpha(110);
    let accento = egui::Color32::from_rgb(110, 155, 209);
    painter.circle_filled(pos + egui::vec2(0.0, 1.5), raggio + 1.0, ombra);
    painter.circle_filled(pos, raggio, accento);
    painter.circle_stroke(pos, raggio, egui::Stroke::new(2.0, egui::Color32::WHITE));
    painter.circle_filled(pos, 2.5, egui::Color32::from_rgb(16, 16, 18));
}

/// Coordinate manuali e azioni sulla selezione.
fn coordinate(ui: &mut egui::Ui, state: &mut AppState, actions: &mut UiActions, count: usize) {
    ui.add_space(8.0);
    ui.strong("Coordinate");
    if state.gps.marker_mixed {
        ui.weak(
            "Le foto selezionate hanno coordinate diverse (o alcune ne sono \
             prive): le coordinate applicate valgono per tutte.",
        );
    }

    let mut cambiato = false;
    egui::Grid::new("gps_manual_grid")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Latitudine");
            cambiato |= ui
                .add(
                    egui::TextEdit::singleline(&mut state.gps.manual_lat)
                        .hint_text("44.4949")
                        .desired_width(130.0),
                )
                .changed();
            ui.end_row();

            ui.label("Longitudine");
            cambiato |= ui
                .add(
                    egui::TextEdit::singleline(&mut state.gps.manual_lon)
                        .hint_text("11.3426")
                        .desired_width(130.0),
                )
                .changed();
            ui.end_row();

            ui.label("Altitudine");
            cambiato |= ui
                .add(
                    egui::TextEdit::singleline(&mut state.gps.manual_alt)
                        .hint_text("metri (facoltativa)")
                        .desired_width(130.0),
                )
                .changed();
            ui.end_row();
        });
    if cambiato {
        state.apply_gps_manual_fields();
    }

    if let Some(error) = &state.gps.manual_error {
        ui.colored_label(ui.visuals().error_fg_color, error.clone());
    }
    match state.gps.marker {
        Some(marker) => {
            ui.weak(format!(
                "Marker: {}{}",
                marker.format_decimal(),
                marker
                    .altitude
                    .map(|altitude| format!(" · {altitude:.0} m"))
                    .unwrap_or_default()
            ));
        }
        None => {
            ui.weak("Nessun marker: clicca sulla mappa o inserisci le coordinate.");
        }
    }

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let rimuovi = if count == 1 {
            "Rimuovi posizione".to_owned()
        } else {
            format!("Rimuovi posizione da {count}")
        };
        if ui
            .add_enabled(state.gps.marker.is_some(), egui::Button::new(rimuovi))
            .clicked()
        {
            actions.gps_remove = true;
        }
    });
    ui.weak(
        "Coordinate e località restano in bozza: verranno scritte in EXIF GPS e XMP \
         quando premi Salva.",
    );
}

/// Campi delle località (IPTC Extension) per ruolo.
fn localita(ui: &mut egui::Ui, state: &mut AppState, actions: &mut UiActions) {
    for role in LocationRole::ALL {
        egui::CollapsingHeader::new(role.label())
            .default_open(matches!(role, LocationRole::Created))
            .show(ui, |ui| {
                ui.weak(role.hint());
                egui::Grid::new(format!("gps_location_{role:?}"))
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        for field in LocationField::ALL {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(field.label()).strong());
                                if state.gps.location_dirty.contains(&(role, field)) {
                                    ui.colored_label(ui.visuals().selection.stroke.color, "•");
                                }
                            });
                            let mut testo = state.gps.location_value(role, field).to_owned();
                            let hint = if state.gps.location_is_mixed(role, field) {
                                "valori diversi".to_owned()
                            } else {
                                field.hint().to_owned()
                            };
                            let response = ui.add(
                                egui::TextEdit::singleline(&mut testo)
                                    .hint_text(hint)
                                    .desired_width(f32::INFINITY),
                            );
                            if response.changed() {
                                state.set_gps_location_field(role, field, testo);
                            }
                            ui.end_row();
                        }
                    });
            });
    }

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("Annulla modifiche").clicked() {
            actions.gps_reset = true;
        }
    });
    ui.weak(
        "I campi svuotati vengono rimossi dai metadati; il codice paese \
         richiede due lettere (es. IT). Le località si salvano con Salva.",
    );
}

/// Ricerca geografica su Nominatim, senza suggerimenti automatici.
fn ricerca(ui: &mut egui::Ui, state: &mut AppState, actions: &mut UiActions) {
    egui::CollapsingHeader::new("Cerca località (OpenStreetMap)")
        .default_open(false)
        .show(ui, |ui| {
            ui.weak(
                "Ricerca su richiesta tramite Nominatim, fino a 5 risultati. \
                 La ricerca non parte mentre si digita.",
            );
            let busy = state.gps.searching || state.gps.reverse_pending;
            let response = ui.add(
                egui::TextEdit::singleline(&mut state.gps.search_query)
                    .hint_text("es. Duomo di Milano")
                    .desired_width(f32::INFINITY),
            );
            let enter =
                response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));

            let mut avvia = None;
            ui.horizontal(|ui| {
                let query = state.gps.search_query.trim().to_owned();
                if ui
                    .add_enabled(!busy && !query.is_empty(), egui::Button::new("Cerca"))
                    .clicked()
                    || (enter && !busy && !query.is_empty())
                {
                    avvia = Some(query);
                }
                let reverse = egui::Button::new("Rileva dal marker");
                if ui
                    .add_enabled(!busy && state.gps.marker.is_some(), reverse)
                    .clicked()
                {
                    actions.geocode_reverse = state.gps.marker;
                    state.gps.reverse_pending = true;
                    state.gps.geo_error = None;
                    state.gps.reverse_note = None;
                }
                if busy {
                    ui.spinner();
                }
            });
            if let Some(query) = avvia {
                state.gps.searching = true;
                state.gps.geo_error = None;
                state.gps.reverse_note = None;
                actions.geocode_search = Some(query);
            }
            if let Some(error) = &state.gps.geo_error {
                ui.colored_label(ui.visuals().error_fg_color, error.clone());
            }
            if let Some(note) = &state.gps.reverse_note {
                ui.weak(note.clone());
            }

            if state.gps.search_results.is_empty() {
                return;
            }
            ui.add_space(4.0);
            ui.weak("Risultati:");
            let risultati = state.gps.search_results.clone();
            for (index, result) in risultati.iter().enumerate() {
                ui.add_space(4.0);
                ui.label(egui::RichText::new(elide(&result.display_name, 54)).small())
                    .on_hover_text(&result.display_name);
                ui.horizontal(|ui| {
                    if ui.small_button("Marker").clicked() {
                        actions.gps_use_suggestion =
                            Some((result.clone(), SuggestionTarget::Marker));
                    }
                    if ui.small_button("Su scatto").clicked() {
                        actions.gps_use_suggestion =
                            Some((result.clone(), SuggestionTarget::Created));
                    }
                    if ui.small_button("Su raffigurata").clicked() {
                        actions.gps_use_suggestion =
                            Some((result.clone(), SuggestionTarget::Shown));
                    }
                });
                if index + 1 < risultati.len() {
                    ui.separator();
                }
            }
        });
}

/// Finestra con la mappa a tutta dimensione.
fn finestra_mappa_grande(ui: &mut egui::Ui, state: &mut AppState) {
    if !state.gps.large_map {
        return;
    }
    let ctx = ui.ctx().clone();
    let mut open = true;
    egui::Window::new("Posizione GPS — mappa")
        .open(&mut open)
        .collapsible(false)
        .default_size(egui::vec2(720.0, 520.0))
        .show(&ctx, |ui| {
            mappa(ui, state, LARGE_MAP_HEIGHT);
        });
    if !open {
        state.gps.large_map = false;
    }
}

fn cache_tile() -> Option<PathBuf> {
    eframe::storage_dir("UberPhoto").map(|dir| dir.join("tiles"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{AppState, PersistedState, UiActions};
    use std::path::PathBuf;
    use std::time::SystemTime;
    use uberphoto_core::{FormatKind, PhotoAsset};
    use uberphoto_metadata::document_from_tags;

    fn stato_con_documento() -> AppState {
        let mut tags = serde_json::Map::new();
        tags.insert(
            "Composite:GPSLatitude".to_owned(),
            serde_json::json!(44.4949),
        );
        tags.insert(
            "Composite:GPSLongitude".to_owned(),
            serde_json::json!(11.3426),
        );
        tags.insert("Composite:GPSAltitude".to_owned(), serde_json::json!(54));
        tags.insert(
            "XMP-iptcExt:LocationCreated".to_owned(),
            serde_json::json!([{ "City": "Bologna", "CountryCode": "IT" }]),
        );
        let document = document_from_tags(&tags);

        let path = PathBuf::from("/tmp/uberphoto-test/foto.jpg");
        let mut state = AppState::new(PersistedState::default());
        state.exiftool = Some("13.50".to_owned());
        state.entries = vec![PhotoAsset {
            path: path.clone(),
            format: FormatKind::Jpeg,
            byte_size: 1024,
            modified: Some(SystemTime::UNIX_EPOCH),
        }];
        state.selection.insert(path.clone());
        state.meta.docs.insert(path, document);
        state.rebuild_gps_form();
        state
    }

    #[test]
    fn la_scheda_gps_si_disegna_senza_panico() {
        let mut state = stato_con_documento();
        assert_eq!(
            state.gps.marker.map(|marker| marker.latitude),
            Some(44.4949)
        );
        assert_eq!(
            state.gps.marker.and_then(|marker| marker.altitude),
            Some(54.0)
        );
        assert_eq!(
            state
                .gps
                .location_value(LocationRole::Created, LocationField::City),
            "Bologna"
        );

        let mut actions = UiActions::default();
        egui::__run_test_ui(|ui| {
            super::ui(ui, &mut state, &mut actions);
        });
    }

    #[test]
    fn senza_selezione_la_scheda_invita_a_selezionare() {
        let mut state = AppState::new(PersistedState::default());
        state.exiftool = Some("13.50".to_owned());
        let mut actions = UiActions::default();
        egui::__run_test_ui(|ui| {
            super::ui(ui, &mut state, &mut actions);
        });
    }
}
