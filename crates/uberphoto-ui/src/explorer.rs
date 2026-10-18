//! Pannello sinistro — esploratore cartelle.
//!
//! Albero di cartelle reali del filesystem con caricamento lazy dei rami,
//! cartelle recenti e apertura di una nuova radice tramite la finestra di
//! dialogo nativa (invocata a livello di applicazione).
//! (Drag & drop e operazioni di filesystem arrivano con la Fase 2.)

use std::path::Path;

use eframe::egui;
use uberphoto_files::DirNode;

use crate::state::{AppState, UiActions};
use crate::util::elide;

pub fn ui(ui: &mut egui::Ui, state: &mut AppState, actions: &mut UiActions) {
    ui.horizontal(|ui| {
        if ui
            .button("📂 Apri cartella")
            .on_hover_text("Scegli una cartella del filesystem da aprire come radice")
            .clicked()
        {
            actions.pick_folder = true;
        }
    });

    ui.add_space(4.0);
    egui::CollapsingHeader::new("Recenti")
        .default_open(true)
        .show(ui, |ui| {
            if state.recent_folders.is_empty() {
                ui.weak("Nessuna cartella recente");
            } else {
                for dir in &state.recent_folders {
                    let label = elide(&dir.display().to_string(), 34);
                    if ui
                        .selectable_label(false, label)
                        .on_hover_text(dir.display().to_string())
                        .clicked()
                    {
                        actions.open_folder = Some(dir.clone());
                    }
                }
            }
        });

    ui.separator();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if let Some(root) = &mut state.root {
                let current = state.current_dir.clone();
                tree_node(ui, root, 0, current.as_deref(), actions, &mut state.error);
            } else {
                ui.add_space(8.0);
                ui.weak("Apri una cartella per iniziare la navigazione.");
            }
        });
}

fn tree_node(
    ui: &mut egui::Ui,
    node: &mut DirNode,
    depth: usize,
    current: Option<&Path>,
    actions: &mut UiActions,
    error: &mut Option<String>,
) {
    ui.horizontal(|ui| {
        ui.add_space(depth as f32 * 14.0);

        let arrow = match (&node.children, node.expanded) {
            (Some(children), _) if children.is_empty() => None,
            (_, true) => Some("⏷"),
            (_, false) => Some("⏵"),
        };
        match arrow {
            Some(symbol) => {
                if ui
                    .add(egui::Button::new(symbol).small().frame(false))
                    .clicked()
                {
                    node.expanded = !node.expanded;
                    if node.expanded {
                        if let Some(message) = node.load_children() {
                            *error = Some(message);
                        }
                    }
                }
            }
            None => {
                ui.add_space(18.0);
            }
        }

        let is_current = current == Some(node.path.as_path());
        let response = ui
            .selectable_label(is_current, format!("📁 {}", elide(&node.name, 28)))
            .on_hover_text(node.path.display().to_string());
        if response.clicked() {
            actions.browse_dir = Some(node.path.clone());
            if !node.expanded {
                node.expanded = true;
                if let Some(message) = node.load_children() {
                    *error = Some(message);
                }
            }
        }
    });

    if node.expanded {
        if let Some(children) = node.children.as_mut() {
            for child in children {
                tree_node(ui, child, depth + 1, current, actions, error);
            }
        }
    }
}
