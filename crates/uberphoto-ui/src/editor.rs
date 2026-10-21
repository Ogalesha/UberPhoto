//! Scheda Colori — editing non distruttivo.
//!
//! Ogni regolazione è un valore dentro una [`EditRecipe`] tenuta in bozza:
//! i pixel dell'originale non vengono mai riscritti finché non si preme
//! Salva; la ricetta viene applicata a un'anteprima ridotta in background
//! (worker con politica "latest wins") e ai pixel completi solo al salvataggio
//! o all'esportazione.
//!
//! Convenzioni della scheda:
//! - trascinare uno slider aggiorna l'anteprima in tempo reale; la voce di
//!   undo viene registrata al rilascio, non a ogni frame;
//! - "Prima" mostra l'originale finché resta premuto;
//! - Ctrl+Z annulla, Ctrl+Shift+Z (o Ctrl+Y) ripete;
//! - le regolazioni restano in bozza (anche cambiando foto) finché non premi
//!   Salva: lì scegli se creare una nuova copia o sovrascrivere l'originale.

use eframe::egui;
use uberphoto_image::{CropRect, EditField, EditRecipe};

use crate::state::{AppState, EditState, UiActions};
use crate::util::elide;

pub fn ui(ui: &mut egui::Ui, state: &mut AppState, actions: &mut UiActions) {
    let selected = state.selected_paths();
    let path = match selected.as_slice() {
        [] => {
            ui.add_space(8.0);
            ui.weak("Nessuna foto selezionata.");
            ui.add_space(6.0);
            ui.weak("Seleziona una foto nel browser per modificarne colore ed esposizione.");
            return;
        }
        [path] => path.clone(),
        many => {
            ui.add_space(8.0);
            ui.strong(format!("{} foto selezionate", many.len()));
            ui.add_space(6.0);
            ui.weak(
                "Le regolazioni colore si applicano a una foto alla volta: \
                 seleziona una sola foto per aprire la ricetta. \
                 Puoi però applicare un preset all'intera selezione.",
            );
            ui.add_space(6.0);
            preset_alla_selezione(ui, &state.edit.presets, many.len(), actions);
            return;
        }
    };

    let raw = state
        .selected_assets()
        .first()
        .is_some_and(|asset| asset.is_raw());

    let edit = &mut state.edit;
    if edit.path.as_deref() != Some(path.as_path()) {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.spinner();
            ui.weak("Caricamento della ricetta…");
        });
        return;
    }

    scorciatoie(ui, edit);
    anteprima(ui, edit);
    barra_comandi(ui, edit, actions, &path);
    preset(ui, edit, actions);

    if let Some(error) = edit.source_error.clone() {
        ui.add_space(6.0);
        ui.colored_label(
            ui.visuals().error_fg_color,
            format!("Anteprima non disponibile: {error}"),
        );
    } else if edit.source.is_none() {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.spinner();
            ui.weak("Decodifica dell'immagine…");
        });
    }

    egui::CollapsingHeader::new("Regolazioni")
        .default_open(true)
        .show(ui, |ui| {
            ui.spacing_mut().slider_width = (ui.available_width() - 200.0).max(60.0);

            egui::CollapsingHeader::new("Esposizione e tono")
                .default_open(true)
                .show(ui, |ui| {
                    riga_slider(
                        ui,
                        edit,
                        EditField::Exposure,
                        &limiti(-5.0, 5.0, 0.0, 2, 0.05),
                    );
                    riga_slider(
                        ui,
                        edit,
                        EditField::Brightness,
                        &limiti(-100.0, 100.0, 0.0, 0, 0.5),
                    );
                    riga_slider(
                        ui,
                        edit,
                        EditField::Contrast,
                        &limiti(-100.0, 100.0, 0.0, 0, 0.5),
                    );
                });

            egui::CollapsingHeader::new("Colore")
                .default_open(true)
                .show(ui, |ui| {
                    riga_slider(
                        ui,
                        edit,
                        EditField::Saturation,
                        &limiti(-100.0, 100.0, 0.0, 0, 0.5),
                    );
                    riga_slider(
                        ui,
                        edit,
                        EditField::Vibrance,
                        &limiti(-100.0, 100.0, 0.0, 0, 0.5),
                    );
                    riga_slider(
                        ui,
                        edit,
                        EditField::Temperature,
                        &limiti(-100.0, 100.0, 0.0, 0, 0.5),
                    );
                    riga_slider(
                        ui,
                        edit,
                        EditField::Tint,
                        &limiti(-100.0, 100.0, 0.0, 0, 0.5),
                    );
                });

            egui::CollapsingHeader::new("Luci e ombre")
                .default_open(false)
                .show(ui, |ui| {
                    riga_slider(
                        ui,
                        edit,
                        EditField::Highlights,
                        &limiti(-100.0, 100.0, 0.0, 0, 0.5),
                    );
                    riga_slider(
                        ui,
                        edit,
                        EditField::Shadows,
                        &limiti(-100.0, 100.0, 0.0, 0, 0.5),
                    );
                    riga_slider(
                        ui,
                        edit,
                        EditField::Whites,
                        &limiti(-100.0, 100.0, 0.0, 0, 0.5),
                    );
                    riga_slider(
                        ui,
                        edit,
                        EditField::Blacks,
                        &limiti(-100.0, 100.0, 0.0, 0, 0.5),
                    );
                    riga_slider(ui, edit, EditField::Gamma, &limiti(0.2, 3.0, 1.0, 2, 0.01));
                });

            egui::CollapsingHeader::new("Dettaglio")
                .default_open(false)
                .show(ui, |ui| {
                    riga_slider(
                        ui,
                        edit,
                        EditField::Sharpness,
                        &limiti(0.0, 100.0, 0.0, 0, 0.5),
                    );
                });

            egui::CollapsingHeader::new("Geometria")
                .default_open(false)
                .show(ui, |ui| {
                    riga_rotazione(ui, edit);
                    riga_ritaglio(ui, edit);
                });
        });

    ui.add_space(6.0);
    ui.weak(
        "Editing non distruttivo: la ricetta è salvata nel catalogo locale, separata \
         dai metadati; i pixel vengono rigenerati solo all'esportazione e l'originale \
         non viene mai modificato.",
    );
    if raw {
        ui.weak(
            "Le fotografie RAW ricevono un supporto progressivo: senza un'anteprima \
             decodificabile le regolazioni restano salvate ma non visibili qui.",
        );
    }
}

/// Ctrl+Z annulla, Ctrl+Shift+Z / Ctrl+Y ripete (fuori dai campi di testo).
fn scorciatoie(ui: &egui::Ui, edit: &mut EditState) {
    if ui.ctx().egui_wants_keyboard_input() {
        return;
    }
    let (ripeti, annulla) = ui.input_mut(|input| {
        let ripeti = input.consume_key(
            egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
            egui::Key::Z,
        ) || input.consume_key(egui::Modifiers::COMMAND, egui::Key::Y);
        let annulla = input.consume_key(egui::Modifiers::COMMAND, egui::Key::Z);
        (ripeti, annulla)
    });
    if ripeti {
        edit.redo();
    } else if annulla {
        edit.undo();
    }
}

/// Miniatura dell'anteprima attiva (modificata o originale) con etichetta
/// prima/dopo e indicatore di elaborazione.
fn anteprima(ui: &mut egui::Ui, edit: &mut EditState) {
    let disponibile = edit.original.is_some();
    let larghezza = ui.available_width();
    let proporzioni = {
        let size = edit.active_size();
        if size.x > 0.0 {
            size.y / size.x
        } else {
            2.0 / 3.0
        }
    };
    let altezza = (larghezza * proporzioni).clamp(140.0, 300.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(larghezza, altezza), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(
        rect,
        egui::CornerRadius::same(6),
        egui::Color32::from_gray(18),
    );

    if !disponibile {
        let testo = edit
            .source_error
            .as_deref()
            .unwrap_or("Caricamento dell'anteprima…");
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            testo,
            egui::FontId::proportional(13.0),
            egui::Color32::GRAY,
        );
        return;
    }

    let (texture, size) = if edit.compare || edit.edited.is_none() {
        (edit.original.as_ref(), edit.original_size)
    } else {
        (edit.edited.as_ref(), edit.edited_size)
    };
    if let Some(texture) = texture {
        if size.x > 0.0 && size.y > 0.0 {
            let scala = (rect.width() / size.x).min(rect.height() / size.y);
            let mostrata = size * scala;
            painter.image(
                texture.id(),
                egui::Rect::from_center_size(rect.center(), mostrata),
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
    }

    if edit.showing_edited() {
        let etichetta = if edit.compare { "PRIMA" } else { "DOPO" };
        let angolo = if edit.compare {
            rect.right_top() + egui::vec2(-10.0, 10.0)
        } else {
            rect.left_top() + egui::vec2(10.0, 10.0)
        };
        let allineamento = if edit.compare {
            egui::Align2::RIGHT_TOP
        } else {
            egui::Align2::LEFT_TOP
        };
        painter.text(
            angolo,
            allineamento,
            etichetta,
            egui::FontId::proportional(11.0),
            egui::Color32::from_gray(230),
        );
    } else if edit.recipe.is_identity() {
        painter.text(
            rect.left_top() + egui::vec2(10.0, 10.0),
            egui::Align2::LEFT_TOP,
            "ORIGINALE",
            egui::FontId::proportional(11.0),
            egui::Color32::from_gray(150),
        );
    }

    if edit.busy {
        painter.text(
            rect.right_bottom() + egui::vec2(-10.0, -10.0),
            egui::Align2::RIGHT_BOTTOM,
            "Elaborazione…",
            egui::FontId::proportional(11.0),
            egui::Color32::from_gray(150),
        );
    }
}

/// Pulsanti di confronto, cronologia, ripristino e anteprima grande.
fn barra_comandi(
    ui: &mut egui::Ui,
    edit: &mut EditState,
    actions: &mut UiActions,
    path: &std::path::Path,
) {
    ui.horizontal_wrapped(|ui| {
        let confronta = ui
            .button("Prima")
            .on_hover_text("Tieni premuto per confrontare con l'originale");
        edit.compare = confronta.is_pointer_button_down_on();

        let annulla = egui::Button::new(format!("Annulla ({})", edit.undo.len()));
        if ui.add_enabled(!edit.undo.is_empty(), annulla).clicked() {
            edit.undo();
        }
        let ripeti = egui::Button::new(format!("Ripeti ({})", edit.redo.len()));
        if ui.add_enabled(!edit.redo.is_empty(), ripeti).clicked() {
            edit.redo();
        }
        if ui
            .add_enabled(
                !edit.recipe.is_identity(),
                egui::Button::new("Ripristina tutto"),
            )
            .clicked()
        {
            edit.reset_all();
        }
        if ui.button("Anteprima grande").clicked() {
            actions.open_preview = Some(path.to_path_buf());
        }
    });

    if let Some(error) = &edit.save_error {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    if edit.is_dirty() {
        ui.weak("Bozza non salvata: premi Salva (Ctrl+S) per scrivere la foto.");
    }
}

/// Gestione dei preset salvati nel catalogo locale.
fn preset(ui: &mut egui::Ui, edit: &mut EditState, actions: &mut UiActions) {
    egui::CollapsingHeader::new("Preset")
        .default_open(false)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut edit.preset_name)
                        .hint_text("Nome del preset")
                        .desired_width(150.0),
                );
                let salvabile = !edit.preset_name.trim().is_empty();
                if ui
                    .add_enabled(salvabile, egui::Button::new("Salva"))
                    .clicked()
                {
                    actions.edit_save_preset = true;
                }
            });
            if edit.presets.is_empty() {
                ui.weak("Nessun preset salvato.");
            }
            let mut da_applicare: Option<EditRecipe> = None;
            let mut da_eliminare: Option<String> = None;
            for (nome, ricetta) in &edit.presets {
                ui.horizontal(|ui| {
                    ui.label(elide(nome, 22));
                    if ui.small_button("Applica").clicked() {
                        da_applicare = Some(*ricetta);
                    }
                    if ui.small_button("Elimina").clicked() {
                        da_eliminare = Some(nome.clone());
                    }
                });
            }
            if let Some(ricetta) = da_applicare {
                edit.apply_preset(ricetta);
            }
            if let Some(nome) = da_eliminare {
                actions.edit_delete_preset = Some(nome);
            }
        });
}

/// Applica un preset (o la ricetta neutra) a tutte le foto selezionate.
fn preset_alla_selezione(
    ui: &mut egui::Ui,
    presets: &[(String, EditRecipe)],
    quante: usize,
    actions: &mut UiActions,
) {
    ui.strong("Preset");
    if presets.is_empty() {
        ui.weak("Nessun preset salvato: creane uno aprendo una singola foto.");
    }
    for (nome, ricetta) in presets {
        ui.horizontal(|ui| {
            ui.label(elide(nome, 24));
            if ui
                .small_button(format!("Applica a {quante} foto"))
                .clicked()
            {
                actions.edit_apply_preset = Some(*ricetta);
            }
        });
    }
    if ui.small_button("Rimuovi le regolazioni da tutte").clicked() {
        actions.edit_apply_preset = Some(EditRecipe::default());
    }
    ui.weak("Le regolazioni applicate restano in bozza finché non premi Salva.");
}

/// Esito dell'interazione con una riga di regolazione.
#[derive(Default, Clone, Copy)]
struct EsitoRiga {
    /// Il trascinamento è iniziato in questo frame.
    started: bool,
    /// Il valore è cambiato.
    changed: bool,
    /// La modifica è conclusa e merita una voce di cronologia.
    committed: bool,
}

/// Limiti e formato di una regolazione numerica.
#[derive(Clone, Copy)]
struct Limiti {
    min: f32,
    max: f32,
    neutro: f32,
    decimali: usize,
    passo: f32,
}

const fn limiti(min: f32, max: f32, neutro: f32, decimali: usize, passo: f32) -> Limiti {
    Limiti {
        min,
        max,
        neutro,
        decimali,
        passo,
    }
}

/// Riga compatta: etichetta, slider, valore numerico e ripristino.
fn slider_row(ui: &mut egui::Ui, etichetta: &str, valore: &mut f32, limiti: &Limiti) -> EsitoRiga {
    let mut esito = EsitoRiga::default();
    ui.horizontal(|ui| {
        ui.add_sized([88.0, 18.0], egui::Label::new(etichetta).truncate());
        let iniziale = *valore;
        let slider = ui.add(
            egui::Slider::new(valore, limiti.min..=limiti.max)
                .show_value(false)
                .trailing_fill(true),
        );
        let numerico = ui.add_sized(
            [54.0, 18.0],
            egui::DragValue::new(valore)
                .speed(limiti.passo)
                .range(limiti.min..=limiti.max)
                .max_decimals(limiti.decimali),
        );
        let ripristina = ui
            .small_button("↺")
            .on_hover_text("Ripristina questa regolazione");
        if ripristina.clicked() {
            *valore = limiti.neutro;
        }

        esito.started = slider.drag_started() || numerico.drag_started();
        esito.changed = *valore != iniziale;
        esito.committed = if ripristina.clicked() {
            true
        } else if !esito.changed {
            false
        } else {
            slider.drag_stopped()
                || numerico.drag_stopped()
                || (!slider.dragged() && !numerico.dragged())
        };
    });
    esito
}

/// Registra la voce di cronologia al momento della conferma.
fn applica_commit(edit: &mut EditState, esito: &EsitoRiga) {
    if esito.started {
        edit.drag_base = Some(edit.recipe);
    }
    if esito.committed {
        match edit.drag_base.take() {
            Some(base) => {
                edit.undo.push(base);
                if edit.undo.len() > 100 {
                    edit.undo.remove(0);
                }
                edit.redo.clear();
            }
            None => edit.push_undo(),
        }
    }
}

fn valore_campo(recipe: &EditRecipe, field: EditField) -> f32 {
    match field {
        EditField::Exposure => recipe.exposure,
        EditField::Brightness => recipe.brightness,
        EditField::Contrast => recipe.contrast,
        EditField::Saturation => recipe.saturation,
        EditField::Vibrance => recipe.vibrance,
        EditField::Temperature => recipe.temperature,
        EditField::Tint => recipe.tint,
        EditField::Highlights => recipe.highlights,
        EditField::Shadows => recipe.shadows,
        EditField::Whites => recipe.whites,
        EditField::Blacks => recipe.blacks,
        EditField::Gamma => recipe.gamma,
        EditField::Sharpness => recipe.sharpness,
        EditField::Rotation | EditField::Crop => 0.0,
    }
}

fn imposta_campo(recipe: &mut EditRecipe, field: EditField, value: f32) {
    match field {
        EditField::Exposure => recipe.exposure = value,
        EditField::Brightness => recipe.brightness = value,
        EditField::Contrast => recipe.contrast = value,
        EditField::Saturation => recipe.saturation = value,
        EditField::Vibrance => recipe.vibrance = value,
        EditField::Temperature => recipe.temperature = value,
        EditField::Tint => recipe.tint = value,
        EditField::Highlights => recipe.highlights = value,
        EditField::Shadows => recipe.shadows = value,
        EditField::Whites => recipe.whites = value,
        EditField::Blacks => recipe.blacks = value,
        EditField::Gamma => recipe.gamma = value,
        EditField::Sharpness => recipe.sharpness = value,
        EditField::Rotation | EditField::Crop => {}
    }
}

/// Una regolazione numerica completa di cronologia e salvataggio.
fn riga_slider(ui: &mut egui::Ui, edit: &mut EditState, field: EditField, limiti: &Limiti) {
    let mut valore = valore_campo(&edit.recipe, field);
    let esito = slider_row(ui, field.label(), &mut valore, limiti);
    if !esito.changed {
        return;
    }
    applica_commit(edit, &esito);
    imposta_campo(&mut edit.recipe, field, valore);
}

/// Rotazione in passi di 90°.
fn riga_rotazione(ui: &mut egui::Ui, edit: &mut EditState) {
    ui.horizontal(|ui| {
        ui.add_sized([88.0, 18.0], egui::Label::new("Rotazione"));
        for (gradi, testo) in [(0u16, "0°"), (90, "90°"), (180, "180°"), (270, "270°")] {
            let attivo = edit.recipe.rotation == gradi;
            if ui.selectable_label(attivo, testo).clicked() && !attivo {
                edit.push_undo();
                edit.recipe.rotation = gradi;
            }
        }
    });
}

/// Ritaglio in percentuali dell'immagine orientata: bordo sinistro, bordo
/// superiore, larghezza e altezza.
fn riga_ritaglio(ui: &mut egui::Ui, edit: &mut EditState) {
    let crop = edit.recipe.crop.unwrap_or_else(CropRect::full).sanitized();
    let mut sinistra = crop.left * 100.0;
    let mut alto = crop.top * 100.0;
    let mut larghezza = (crop.right - crop.left) * 100.0;
    let mut altezza = (crop.bottom - crop.top) * 100.0;

    let esiti = [
        slider_row(
            ui,
            "Bordo sinistro %",
            &mut sinistra,
            &limiti(0.0, 98.0, 0.0, 0, 0.5),
        ),
        slider_row(
            ui,
            "Bordo superiore %",
            &mut alto,
            &limiti(0.0, 98.0, 0.0, 0, 0.5),
        ),
        slider_row(
            ui,
            "Larghezza %",
            &mut larghezza,
            &limiti(2.0, 100.0, 100.0, 0, 0.5),
        ),
        slider_row(
            ui,
            "Altezza %",
            &mut altezza,
            &limiti(2.0, 100.0, 100.0, 0, 0.5),
        ),
    ];

    let attivo = edit.recipe.crop.is_some();
    ui.horizontal(|ui| {
        if ui
            .add_enabled(attivo, egui::Button::new("Ripristina ritaglio"))
            .clicked()
        {
            edit.push_undo();
            edit.recipe.crop = None;
        }
    });

    let mut cambiato = false;
    let mut iniziato = false;
    let mut confermato = false;
    for esito in &esiti {
        cambiato |= esito.changed;
        iniziato |= esito.started;
        confermato |= esito.committed;
    }
    if !cambiato {
        return;
    }
    applica_commit(
        edit,
        &EsitoRiga {
            started: iniziato,
            changed: cambiato,
            committed: confermato,
        },
    );
    edit.recipe.crop = Some(
        CropRect {
            left: sinistra / 100.0,
            top: alto / 100.0,
            right: (sinistra + larghezza) / 100.0,
            bottom: (alto + altezza) / 100.0,
        }
        .sanitized(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::PersistedState;

    #[test]
    fn senza_selezione_la_scheda_invita_a_selezionare() {
        let mut state = AppState::new(PersistedState::default());
        let mut actions = UiActions::default();
        egui::__run_test_ui(|ui| {
            super::ui(ui, &mut state, &mut actions);
        });
    }

    #[test]
    fn con_piu_foto_la_scheda_lo_dichiara() {
        let mut state = AppState::new(PersistedState::default());
        state.entries = vec![
            uberphoto_core::PhotoAsset {
                path: std::path::PathBuf::from("/foto/a.jpg"),
                format: uberphoto_core::FormatKind::Jpeg,
                byte_size: 1,
                modified: None,
            },
            uberphoto_core::PhotoAsset {
                path: std::path::PathBuf::from("/foto/b.jpg"),
                format: uberphoto_core::FormatKind::Jpeg,
                byte_size: 1,
                modified: None,
            },
        ];
        state
            .selection
            .insert(std::path::PathBuf::from("/foto/a.jpg"));
        state
            .selection
            .insert(std::path::PathBuf::from("/foto/b.jpg"));
        state.edit.presets.push((
            "Bianco e nero".to_owned(),
            EditRecipe {
                saturation: -100.0,
                ..EditRecipe::default()
            },
        ));
        let mut actions = UiActions::default();
        egui::__run_test_ui(|ui| {
            super::ui(ui, &mut state, &mut actions);
        });
    }

    #[test]
    fn con_la_ricetta_caricata_la_scheda_si_disegna() {
        let mut state = AppState::new(PersistedState::default());
        state.entries = vec![uberphoto_core::PhotoAsset {
            path: std::path::PathBuf::from("/foto/a.jpg"),
            format: uberphoto_core::FormatKind::Jpeg,
            byte_size: 1,
            modified: None,
        }];
        state
            .selection
            .insert(std::path::PathBuf::from("/foto/a.jpg"));
        state.edit.carica(
            std::path::Path::new("/foto/a.jpg"),
            None,
            EditRecipe {
                exposure: 0.5,
                ..EditRecipe::default()
            },
        );
        let mut actions = UiActions::default();
        egui::__run_test_ui(|ui| {
            super::ui(ui, &mut state, &mut actions);
        });
    }

    #[test]
    fn un_errore_di_decodifica_resta_visibile_nella_scheda() {
        let mut state = AppState::new(PersistedState::default());
        state.entries = vec![uberphoto_core::PhotoAsset {
            path: std::path::PathBuf::from("/foto/a.cr2"),
            format: uberphoto_core::FormatKind::Cr2,
            byte_size: 1,
            modified: None,
        }];
        state
            .selection
            .insert(std::path::PathBuf::from("/foto/a.cr2"));
        state.edit.carica(
            std::path::Path::new("/foto/a.cr2"),
            None,
            EditRecipe::default(),
        );
        state.edit.source_error = Some("decodifica fallita: RAW non supportato".to_owned());
        let mut actions = UiActions::default();
        egui::__run_test_ui(|ui| {
            super::ui(ui, &mut state, &mut actions);
        });
    }
}
