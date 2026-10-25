//! Pannello destro — Inspector.
//!
//! La scheda "Informazioni" mostra i dati disponibili sul file, la scheda
//! "Metadati" è l'editor EXIF / IPTC / XMP basato su ExifTool (Fase 3).
//! Le altre schede dichiarano esplicitamente la fase in cui verranno
//! implementate: nessuna funzionalità non verificata viene presentata come
//! disponibile.

use eframe::egui;
use uberphoto_core::PhotoAsset;
use uberphoto_metadata::{MetadataDocument, MetadataField, Section};

use crate::i18n::{metadata_field_hint, metadata_field_label, metadata_section_label};
use crate::state::{AppState, InspectorTab, UiActions};
use crate::t;
use crate::thumbnails::ThumbCache;
use crate::util::{elide, format_bytes, format_time};

pub fn ui(ui: &mut egui::Ui, state: &mut AppState, thumbs: &ThumbCache, actions: &mut UiActions) {
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
            InspectorTab::Colori => crate::editor::ui(ui, state, actions),
            InspectorTab::Informazioni => informazioni(ui, state, thumbs),
            InspectorTab::Metadati => metadati(ui, state, actions),
            InspectorTab::Gps => crate::gps::ui(ui, state, actions),
            InspectorTab::Esportazione => crate::export::ui(ui, state, actions),
        });
}

fn informazioni(ui: &mut egui::Ui, state: &AppState, thumbs: &ThumbCache) {
    let selected = state.selected_assets();

    match selected.as_slice() {
        [] => {
            ui.add_space(8.0);
            ui.weak(t!("Nessuna foto selezionata."));
            ui.add_space(6.0);
            ui.weak(t!(
                "Seleziona una o più foto nel browser: i dettagli si aggiornano automaticamente."
            ));
        }
        [asset] => singola(ui, asset, thumbs),
        many => multipla(ui, many),
    }

    ui.add_space(10.0);
    match &state.exiftool {
        Some(version) => ui.weak(t!("Motore metadati: ExifTool {version}", version = version)),
        None => ui.weak(t!("Motore metadati: ExifTool non trovato")),
    };
}

fn singola(ui: &mut egui::Ui, asset: &PhotoAsset, thumbs: &ThumbCache) {
    egui::Grid::new("info_grid")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            row(ui, t!("Nome"), &asset.file_name());
            row(ui, t!("Percorso"), &asset.path.display().to_string());
            row(ui, t!("Formato"), asset.format.label());
            row(ui, t!("Dimensione"), &format_bytes(asset.byte_size));
            if let Some((width, height)) = thumbs.dims(&asset.path) {
                row(
                    ui,
                    t!("Dimensioni"),
                    &t!("{width} × {height} px", width = width, height = height),
                );
                let megapixel = (width as f64 * height as f64) / 1_000_000.0;
                row(ui, t!("Risoluzione"), &format!("{megapixel:.1} MP"));
            }
            if let Some(modified) = asset.modified {
                row(ui, t!("Modificato"), &format_time(modified));
            }
            if asset.is_raw() {
                row(ui, "RAW", t!("sì — l'originale resta di sola lettura"));
            }
        });

    ui.add_space(10.0);
    let capabilities = asset.format.capabilities();
    ui.strong(t!("Capacità del formato"));
    ui.add_space(4.0);
    egui::Grid::new("capabilities_grid")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            capability_row(ui, t!("Anteprima integrata"), capabilities.view);
            capability_row(ui, t!("Editing colore"), capabilities.pixel_edit);
            capability_row(ui, t!("Metadati embedded"), capabilities.embedded_metadata);
            capability_row(ui, t!("Sidecar XMP"), capabilities.xmp_sidecar);
            capability_row(ui, t!("Formato di export"), capabilities.export_target);
        });
}

fn multipla(ui: &mut egui::Ui, assets: &[&PhotoAsset]) {
    ui.add_space(8.0);
    ui.strong(t!("{count} foto selezionate", count = assets.len()));
    ui.add_space(6.0);

    let total_bytes: u64 = assets.iter().map(|asset| asset.byte_size).sum();
    ui.label(t!(
        "Dimensione totale: {size}",
        size = format_bytes(total_bytes)
    ));

    let mut formats: Vec<&str> = assets.iter().map(|asset| asset.format.label()).collect();
    formats.sort_unstable();
    formats.dedup();
    ui.label(t!("Formati: {formats}", formats = formats.join(", ")));

    ui.add_space(10.0);
    ui.weak(
        t!("Usa la scheda Metadati per modificare i campi comuni a tutte le foto selezionate: i campi con valori diversi restano invariati finché non li modifichi."),
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
        ui.label(t!("sì"));
    } else {
        ui.weak(t!("no"));
    }
    ui.end_row();
}

/// Editor metadati: form canonico multi-selezione, dati tecnici e tag grezzi.
fn metadati(ui: &mut egui::Ui, state: &mut AppState, actions: &mut UiActions) {
    if state.exiftool.is_none() {
        ui.add_space(10.0);
        ui.strong(t!("Editor metadati non disponibile"));
        ui.add_space(6.0);
        ui.weak(
            t!("ExifTool non è stato trovato. Installa exiftool (pacchetto libimage-exiftool-perl su Ubuntu) oppure imposta la variabile UBERPHOTO_EXIFTOOL con il percorso dell'eseguibile, poi riavvia UberPhoto."),
        );
        return;
    }

    let selected = state.selected_paths();
    if selected.is_empty() {
        ui.add_space(8.0);
        ui.weak(t!("Nessuna foto selezionata."));
        ui.add_space(6.0);
        ui.weak(t!(
            "Seleziona una o più foto per leggerne e modificarne i metadati."
        ));
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
                    .unwrap_or(t!("file"))
                    .to_owned();
                state
                    .meta
                    .errors
                    .get(path)
                    .map(|error| (nome, error.clone()))
            })
            .collect();
        for (nome, errore) in &errori {
            ui.colored_label(
                ui.visuals().error_fg_color,
                t!("{name}: {error}", name = nome, error = errore),
            );
        }
        if errori.is_empty() {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.weak(t!("Lettura dei metadati…"));
            });
        }
        ui.add_space(6.0);
        if ui.button(t!("Ricarica")).clicked() {
            actions.metadata_reload = true;
        }
        return;
    }

    ui.horizontal(|ui| {
        ui.strong(if selected.len() == 1 {
            t!("1 foto selezionata").to_owned()
        } else {
            t!("{count} foto selezionate", count = selected.len())
        });
        if ui.small_button(t!("Ricarica")).clicked() {
            actions.metadata_reload = true;
        }
    });
    ui.add_space(4.0);

    for (titolo, section) in [
        (
            metadata_section_label(Section::Descriptive),
            Section::Descriptive,
        ),
        (metadata_section_label(Section::Author), Section::Author),
        (metadata_section_label(Section::Temporal), Section::Temporal),
    ] {
        egui::CollapsingHeader::new(titolo)
            .default_open(true)
            .show(ui, |ui| {
                egui::Grid::new(format!("meta_grid_{titolo}"))
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        for field in MetadataField::ALL
                            .into_iter()
                            .filter(|field| field.section() == section)
                        {
                            campo(ui, state, field);
                        }
                    });
            });
    }

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let salva = egui::Button::new(t!("Salva…")).min_size(egui::vec2(110.0, 28.0));
        if ui
            .add_enabled(
                state.has_pending_changes() && state.gps.manual_error.is_none(),
                salva,
            )
            .on_hover_text(t!("Salva colore, metadati e posizione (Ctrl+S)"))
            .clicked()
        {
            actions.save_open = true;
        }
        if ui.button(t!("Annulla modifiche")).clicked() {
            state.discard_meta_drafts();
        }
    });
    ui.add_space(4.0);
    ui.weak(
        t!("I campi svuotati vengono rimossi dai metadati; le modifiche a colore, metadati e posizione si salvano insieme con Salva."),
    );
    if let Some(asset) = state.selected_assets().first() {
        if !asset.format.capabilities().embedded_metadata {
            ui.weak(t!(
                "{format} non supporta metadati incorporati: i campi XMP saranno scritti nel sidecar accanto al file.",
                format = asset.format.label()
            ));
        }
    }

    if selected.len() == 1 {
        if let Some(document) = state.meta.docs.get(&selected[0]) {
            tecnica(ui, document);
        }
        avanzata(ui, state, &selected[0]);
    }
}

/// Riga del form: etichetta, indicatore "modificato" ed editor del campo.
fn campo(ui: &mut egui::Ui, state: &mut AppState, field: MetadataField) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(metadata_field_label(field)).strong());
        if state.meta.form_dirty.contains(&field) {
            ui.colored_label(ui.visuals().selection.stroke.color, "•");
        }
    });

    let mixed = state.meta.form_mixed.contains(&field);
    let current = state
        .meta
        .form_values
        .get(&field)
        .cloned()
        .unwrap_or_default();

    match field {
        MetadataField::Rating => {
            let value = current.parse::<u8>().ok().filter(|value| *value <= 5);
            let label = match value {
                Some(0) | None => t!("Nessuna").to_owned(),
                Some(stars) => format!("{stars} {}", "★".repeat(stars as usize)),
            };
            let mut scelto = value;
            egui::ComboBox::from_id_salt("meta_rating")
                .selected_text(label)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut scelto, None, t!("Nessuna"));
                    for stars in 1..=5u8 {
                        ui.selectable_value(
                            &mut scelto,
                            Some(stars),
                            format!("{stars} {}", "★".repeat(stars as usize)),
                        );
                    }
                });
            if scelto != value {
                let text = scelto.map(|stars| stars.to_string()).unwrap_or_default();
                state.set_meta_field(field, Some(text));
            }
        }
        MetadataField::Orientation => {
            let value = current
                .parse::<u8>()
                .ok()
                .filter(|value| (1..=8).contains(value));
            let mut scelto = value;
            egui::ComboBox::from_id_salt("meta_orientation")
                .selected_text(orientation_label(value))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut scelto, None, t!("Non impostato"));
                    for code in 1..=8u8 {
                        ui.selectable_value(&mut scelto, Some(code), orientation_label(Some(code)));
                    }
                });
            if scelto != value {
                let text = scelto.map(|code| code.to_string()).unwrap_or_default();
                state.set_meta_field(field, Some(text));
            }
        }
        _ => {
            let hint = if mixed {
                t!("valori diversi")
            } else {
                metadata_field_hint(field)
            };
            let mut text = current;
            let response = ui.add(
                egui::TextEdit::singleline(&mut text)
                    .hint_text(hint)
                    .desired_width(f32::INFINITY),
            );
            if response.changed() {
                state.set_meta_field(field, Some(text));
            }
        }
    }
    ui.end_row();
}

fn orientation_label(value: Option<u8>) -> &'static str {
    match value {
        Some(1) => t!("1 — Normale"),
        Some(2) => t!("2 — Specchiata orizzontalmente"),
        Some(3) => t!("3 — Ruotata di 180°"),
        Some(4) => t!("4 — Specchiata verticalmente"),
        Some(5) => t!("5 — Specchiata e ruotata 90° CW"),
        Some(6) => t!("6 — Ruotata di 90° CW"),
        Some(7) => t!("7 — Specchiata e ruotata 90° CCW"),
        Some(8) => t!("8 — Ruotata di 90° CCW"),
        _ => t!("Non impostato"),
    }
}

/// Dati della fotocamera: sola lettura, per non falsificare la provenienza.
fn tecnica(ui: &mut egui::Ui, document: &MetadataDocument) {
    ui.add_space(6.0);
    egui::CollapsingHeader::new(t!("Informazioni tecniche (sola lettura)"))
        .default_open(false)
        .show(ui, |ui| {
            let technical = &document.technical;
            let camera = match (&technical.make, &technical.model) {
                (Some(make), Some(model)) => Some(format!("{make} {model}")),
                (Some(make), None) => Some(make.clone()),
                (None, Some(model)) => Some(model.clone()),
                (None, None) => None,
            };
            egui::Grid::new("meta_technical")
                .num_columns(2)
                .spacing([10.0, 4.0])
                .striped(true)
                .show(ui, |ui| {
                    for (key, value) in [
                        (t!("Fotocamera"), camera),
                        (t!("Obiettivo"), technical.lens.clone()),
                        (t!("Esposizione"), technical.exposure_time.clone()),
                        (t!("Apertura"), technical.f_number.clone()),
                        ("ISO", technical.iso.map(|iso| iso.to_string())),
                        (t!("Lunghezza focale"), technical.focal_length.clone()),
                        (t!("Profilo colore"), technical.color_profile.clone()),
                        (t!("Digitalizzazione"), technical.date_created.clone()),
                        (t!("Ultima modifica"), technical.date_modified.clone()),
                    ] {
                        if let Some(value) = value {
                            row(ui, key, &value);
                        }
                    }
                    if let Some(gps) = document.gps {
                        row(
                            ui,
                            t!("Coordinate"),
                            &format!("{:.6}, {:.6}", gps.latitude, gps.longitude),
                        );
                    }
                });
        });
}

/// Vista avanzata dei tag ExifTool: sola lettura, con filtro e scrivibilità.
fn avanzata(ui: &mut egui::Ui, state: &mut AppState, path: &std::path::Path) {
    egui::CollapsingHeader::new(t!("Tag ExifTool (avanzato)"))
        .default_open(false)
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut state.meta.raw_filter)
                    .hint_text(t!("Filtra per gruppo o nome del tag…"))
                    .desired_width(f32::INFINITY),
            );
            let Some(raw) = state.meta.raw.get(path) else {
                ui.weak(t!("Tag non ancora letti."));
                return;
            };
            let filter = state.meta.raw_filter.trim().to_lowercase();
            egui::ScrollArea::vertical()
                .max_height(280.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    egui::Grid::new("meta_raw_tags")
                        .num_columns(3)
                        .spacing([8.0, 2.0])
                        .striped(true)
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new(t!("Tag")).strong());
                            ui.label(egui::RichText::new(t!("Valore")).strong());
                            ui.label(egui::RichText::new(t!("Scrivibile")).strong());
                            ui.end_row();
                            for tag in raw {
                                let nome = format!("{}:{}", tag.group, tag.name);
                                if !filter.is_empty() && !nome.to_lowercase().contains(&filter) {
                                    continue;
                                }
                                ui.label(&nome);
                                ui.label(elide(&tag.value, 60));
                                if tag.writable {
                                    ui.label(t!("sì"));
                                } else {
                                    ui.weak(t!("no"));
                                }
                                ui.end_row();
                            }
                        });
                });
            ui.weak(t!(
                "Vista di sola lettura: l'editing dei singoli tag arriverà in una fase successiva."
            ));
        });
}
