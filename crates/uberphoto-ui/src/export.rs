//! Pannello destro — scheda "Esportazione" (Export for Web, Fase 6).
//!
//! Raccoglie i parametri, avvia il lavoro in background e mostra il report
//! verificato di ogni file: UberPhoto dichiara solo ciò che scrive e rilegge.

use eframe::egui;
use uberphoto_export::{ExportFormat, ExportPreset, NameMode};

use crate::i18n::{export_format_label, export_preset_hint, export_preset_label, name_mode_label};
use crate::state::{AppState, UiActions};
use crate::t;
use crate::util::format_bytes;

pub fn ui(ui: &mut egui::Ui, state: &mut AppState, actions: &mut UiActions) {
    let selezionati = state.selected_assets().len();
    let exiftool = state.exiftool.clone();
    let export = &mut state.export;

    ui.add_space(4.0);
    ui.strong(t!("Export for Web"));
    ui.weak(
        t!("UberPhoto garantisce solo i campi che scrive e riverifica con ExifTool nel file esportato."),
    );
    if exiftool.is_none() {
        ui.colored_label(
            ui.visuals().error_fg_color,
            t!("ExifTool non disponibile: l'esportazione è disabilitata perché i metadati non possono essere copiati né verificati."),
        );
    }
    ui.add_space(6.0);

    egui::CollapsingHeader::new(t!("Metadati"))
        .default_open(true)
        .show(ui, |ui| {
            for preset in ExportPreset::ALL {
                ui.radio_value(
                    &mut export.settings.preset,
                    preset,
                    export_preset_label(preset),
                )
                .on_hover_text(export_preset_hint(preset));
            }
            if export.settings.preset == ExportPreset::Privacy {
                ui.indent("privacy-options", |ui| {
                    ui.checkbox(
                        &mut export.settings.privacy.remove_gps,
                        t!("Rimuovi posizione GPS"),
                    );
                    ui.checkbox(
                        &mut export.settings.privacy.remove_serial,
                        t!("Rimuovi numeri di serie"),
                    );
                    ui.checkbox(
                        &mut export.settings.privacy.remove_author,
                        t!("Rimuovi autore e proprietario"),
                    );
                });
            }
        });

    egui::CollapsingHeader::new(t!("Immagine"))
        .default_open(true)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(t!("Formato"));
                egui::ComboBox::from_id_salt("export-format")
                    .selected_text(export_format_label(export.settings.format))
                    .show_ui(ui, |ui| {
                        for formato in ExportFormat::ALL {
                            ui.selectable_value(
                                &mut export.settings.format,
                                formato,
                                export_format_label(formato),
                            );
                        }
                    });
            });
            if export.settings.format.supports_quality() {
                ui.add(
                    egui::Slider::new(&mut export.settings.quality, 1..=100)
                        .text(t!("Qualità JPEG")),
                );
            } else if export.settings.format == ExportFormat::Webp {
                ui.weak(t!(
                    "WebP viene scritto senza perdita: la qualità non si applica."
                ));
            }

            ui.separator();
            ui.checkbox(&mut export.settings.resize.enabled, t!("Ridimensiona"));
            ui.add_enabled_ui(export.settings.resize.enabled, |ui| {
                ui.horizontal(|ui| {
                    ui.label(t!("Larghezza max"));
                    ui.add(
                        egui::DragValue::new(&mut export.settings.resize.max_width)
                            .range(0..=20000)
                            .speed(10),
                    );
                    ui.label("px");
                });
                ui.horizontal(|ui| {
                    ui.label(t!("Altezza max"));
                    ui.add(
                        egui::DragValue::new(&mut export.settings.resize.max_height)
                            .range(0..=20000)
                            .speed(10),
                    );
                    ui.label("px");
                });
                ui.checkbox(
                    &mut export.settings.resize.keep_aspect,
                    t!("Mantieni proporzioni"),
                );
                ui.weak(t!(
                    "0 = nessun limite; l'immagine non viene mai ingrandita."
                ));
            });
        });

    egui::CollapsingHeader::new(t!("Destinazione e nome"))
        .default_open(true)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(t!("Cartella"));
                if export.settings.destination.as_os_str().is_empty() {
                    ui.weak(t!("(cartella dell'originale)"));
                } else {
                    ui.add(
                        egui::Label::new(export.settings.destination.display().to_string())
                            .truncate(),
                    );
                }
            });
            ui.horizontal(|ui| {
                if ui.button(t!("Scegli…")).clicked() {
                    actions.export_pick_destination = true;
                }
                if ui
                    .add_enabled(
                        !export.settings.destination.as_os_str().is_empty(),
                        egui::Button::new(t!("Usa cartella dell'originale")),
                    )
                    .clicked()
                {
                    export.settings.destination.clear();
                }
            });

            ui.horizontal(|ui| {
                ui.label(t!("Nome"));
                egui::ComboBox::from_id_salt("export-naming")
                    .selected_text(name_mode_label(export.settings.naming.mode))
                    .show_ui(ui, |ui| {
                        for modo in NameMode::ALL {
                            ui.selectable_value(
                                &mut export.settings.naming.mode,
                                modo,
                                name_mode_label(modo),
                            );
                        }
                    });
            });
            match export.settings.naming.mode {
                NameMode::Prefix => {
                    ui.horizontal(|ui| {
                        ui.label(t!("Prefisso"));
                        ui.text_edit_singleline(&mut export.settings.naming.prefix);
                    });
                }
                NameMode::Suffix => {
                    ui.horizontal(|ui| {
                        ui.label(t!("Suffisso"));
                        ui.text_edit_singleline(&mut export.settings.naming.suffix);
                    });
                }
                NameMode::KeepName => {
                    ui.weak(t!(
                        "Mantiene il nome dell'originale (con estensione del formato)."
                    ));
                }
                NameMode::WebSafe => {
                    ui.weak(t!(
                        "Minuscole, solo lettere e cifre, spazi e simboli diventano «-»."
                    ));
                }
            }
            ui.weak(t!(
                "I file esistenti non vengono sovrascritti: si aggiunge -2, -3, …"
            ));
        });

    ui.add_space(8.0);
    ui.separator();
    ui.horizontal(|ui| {
        if export.busy {
            ui.spinner();
            ui.label(t!(
                "Esportazione… {done}/{total}",
                done = export.done,
                total = export.total
            ));
            if ui.button(t!("Annulla")).clicked() {
                actions.export_cancel = true;
            }
        } else {
            let abilitato = selezionati > 0 && exiftool.is_some();
            let etichetta = if selezionati == 1 {
                t!("Esporta 1 foto").to_owned()
            } else {
                t!("Esporta {count} foto", count = selezionati)
            };
            if ui
                .add_enabled(abilitato, egui::Button::new(etichetta))
                .clicked()
            {
                actions.export_start = true;
            }
            if selezionati == 0 {
                ui.weak(t!("Seleziona almeno una foto."));
            }
            if (!export.reports.is_empty() || !export.failures.is_empty())
                && ui.button(t!("Pulisci report")).clicked()
            {
                actions.export_clear = true;
            }
        }
    });

    if export.total > 0 && (export.busy || export.done > 0) {
        ui.add(
            egui::ProgressBar::new(export.progress())
                .show_percentage()
                .desired_width(ui.available_width()),
        );
    }
    if let Some(errore) = &export.error {
        ui.colored_label(ui.visuals().error_fg_color, errore);
    }
    if let Some(riepilogo) = &export.summary {
        ui.weak(riepilogo);
    }

    if !export.reports.is_empty() {
        ui.add_space(6.0);
        ui.strong(t!("Report ({count})", count = export.reports.len()));
        for (indice, report) in export.reports.iter().enumerate() {
            let titolo = format!(
                "{} · {}×{} · {}",
                report.file_name(),
                report.width,
                report.height,
                format_bytes(report.bytes)
            );
            egui::CollapsingHeader::new(titolo)
                .id_salt(("export-report", indice))
                .default_open(indice == 0)
                .show(ui, |ui| {
                    if report.verified() {
                        ui.colored_label(
                            egui::Color32::from_rgb(126, 196, 130),
                            t!("Verifica superata: nessun campo atteso è andato perso."),
                        );
                    } else {
                        ui.colored_label(
                            ui.visuals().error_fg_color,
                            t!("Verifica incompleta: alcuni campi attesi non sono nel file."),
                        );
                    }
                    ui.weak(report.output.display().to_string());
                    elenco(ui, t!("Preservati"), &report.preserved, None);
                    elenco(
                        ui,
                        t!("Modificati"),
                        &report.adjusted,
                        Some(egui::Color32::from_rgb(196, 178, 118)),
                    );
                    elenco(
                        ui,
                        t!("Non supportati"),
                        &report.unsupported,
                        Some(egui::Color32::from_rgb(214, 160, 110)),
                    );
                    elenco(
                        ui,
                        t!("Persi"),
                        &report.lost,
                        Some(ui.visuals().error_fg_color),
                    );
                    elenco(ui, t!("Avvisi"), &report.warnings, None);
                });
        }
    }

    if !export.failures.is_empty() {
        ui.add_space(6.0);
        egui::CollapsingHeader::new(t!("Errori ({count})", count = export.failures.len()))
            .default_open(true)
            .show(ui, |ui| {
                for (path, errore) in &export.failures {
                    ui.colored_label(ui.visuals().error_fg_color, errore);
                    ui.weak(path.display().to_string());
                    ui.add_space(2.0);
                }
            });
    }
}

fn elenco(ui: &mut egui::Ui, titolo: &str, voci: &[String], colore: Option<egui::Color32>) {
    if voci.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.strong(titolo);
        match colore {
            Some(colore) => {
                ui.colored_label(colore, voci.join(" · "));
            }
            None => {
                ui.label(voci.join(" · "));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn senza_selezione_la_scheda_si_disegna_e_invita_a_selezionare() {
        let mut state = AppState::new(crate::state::PersistedState::default());
        state.exiftool = Some("13.50".to_owned());
        state.inspector_tab = crate::state::InspectorTab::Esportazione;
        let mut actions = UiActions::default();
        egui::__run_test_ui(|ui| {
            super::ui(ui, &mut state, &mut actions);
        });
        assert!(!actions.export_start);
    }

    #[test]
    fn i_parametri_restano_nel_pannello() {
        let mut state = AppState::new(crate::state::PersistedState::default());
        state.exiftool = Some("13.50".to_owned());
        state.export.settings.preset = ExportPreset::Privacy;
        state.export.settings.resize.enabled = false;
        let mut actions = UiActions::default();
        egui::__run_test_ui(|ui| {
            super::ui(ui, &mut state, &mut actions);
        });
        assert_eq!(state.export.settings.preset, ExportPreset::Privacy);
        assert!(!state.export.settings.resize.enabled);
    }
}
