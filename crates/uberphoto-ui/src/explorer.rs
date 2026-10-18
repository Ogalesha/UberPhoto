//! Pannello sinistro — esploratore cartelle.
//!
//! Albero di cartelle reali del filesystem con caricamento lazy dei rami,
//! ricerca rapida, cartelle recenti e apertura di una nuova radice tramite
//! la finestra di dialogo nativa (invocata a livello di applicazione).
//!
//! Le voci dell'albero e i recenti sono zone di rilascio per le foto
//! trascinate dalla griglia; il menu contestuale offre le operazioni di
//! cartella (nuova, rinomina, elimina se vuota).

use std::path::Path;

use eframe::egui;
use uberphoto_files::DirNode;

use crate::state::{AppState, DragPhotos, DropRequest, UiActions};
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
                    let hover = dir.display().to_string();
                    let (inner, payload) = ui
                        .dnd_drop_zone::<DragPhotos, _>(egui::Frame::NONE, |ui| {
                            ui.selectable_label(false, label).on_hover_text(hover)
                        });
                    if inner.inner.clicked() {
                        actions.open_folder = Some(dir.clone());
                    }
                    if let Some(dropped) = payload {
                        let copy = ui.input(|input| input.modifiers.command);
                        actions.drop_onto = Some(DropRequest {
                            paths: dropped.paths.clone(),
                            target: dir.clone(),
                            copy,
                        });
                    }
                }
            }
        });

    ui.separator();

    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut state.tree_filter)
                .hint_text("Filtra cartelle…")
                .desired_width(150.0),
        );
        if !state.tree_filter.is_empty() && ui.small_button("✕").clicked() {
            state.tree_filter.clear();
        }
    });
    ui.add_space(4.0);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if let Some(root) = &mut state.root {
                let current = state.current_dir.clone();
                let filter = state.tree_filter.trim().to_lowercase();
                tree_node(
                    ui,
                    root,
                    0,
                    current.as_deref(),
                    &filter,
                    actions,
                    &mut state.error,
                );
            } else {
                ui.add_space(8.0);
                ui.weak("Apri una cartella per iniziare la navigazione.");
            }
        });
}

/// True se il nodo va mostrato con il filtro: nome corrispondente o almeno
/// un discendente già caricato che corrisponde.
fn node_matches(node: &DirNode, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    if node.name.to_lowercase().contains(filter) {
        return true;
    }
    node.children
        .as_ref()
        .is_some_and(|children| children.iter().any(|child| node_matches(child, filter)))
}

fn tree_node(
    ui: &mut egui::Ui,
    node: &mut DirNode,
    depth: usize,
    current: Option<&Path>,
    filter: &str,
    actions: &mut UiActions,
    error: &mut Option<String>,
) {
    if !node_matches(node, filter) {
        return;
    }

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
        let label = format!("📁 {}", elide(&node.name, 28));
        let hover = node.path.display().to_string();
        let node_path = node.path.clone();

        let (inner, payload) = ui.dnd_drop_zone::<DragPhotos, _>(egui::Frame::NONE, |ui| {
            ui.selectable_label(is_current, label).on_hover_text(hover)
        });
        let response = inner.inner;

        if response.clicked() {
            actions.browse_dir = Some(node_path.clone());
            if !node.expanded {
                node.expanded = true;
                if let Some(message) = node.load_children() {
                    *error = Some(message);
                }
            }
        }

        response.context_menu(|ui| {
            if ui.button("📂 Apri come radice").clicked() {
                actions.open_folder = Some(node_path.clone());
                ui.close();
            }
            ui.separator();
            if ui.button("➕ Nuova sottocartella…").clicked() {
                actions.create_folder = Some(node_path.clone());
                ui.close();
            }
            if ui.button("✏ Rinomina…").clicked() {
                actions.rename_folder = Some(node_path.clone());
                ui.close();
            }
            if ui.button("🗑 Elimina (solo se vuota)").clicked() {
                actions.delete_folder = Some(node_path.clone());
                ui.close();
            }
        });

        if let Some(dropped) = payload {
            let copy = ui.input(|input| input.modifiers.command);
            actions.drop_onto = Some(DropRequest {
                paths: dropped.paths.clone(),
                target: node_path.clone(),
                copy,
            });
        }
    });

    if node.expanded {
        if let Some(children) = node.children.as_mut() {
            for child in children {
                tree_node(ui, child, depth + 1, current, filter, actions, error);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn node(name: &str, children: Option<Vec<DirNode>>) -> DirNode {
        DirNode {
            path: PathBuf::from(format!("/base/{name}")),
            name: name.to_owned(),
            expanded: false,
            children,
        }
    }

    #[test]
    fn senza_filtro_tutti_i_nodi_corrispondono() {
        let root = node("base", None);
        assert!(node_matches(&root, ""));
    }

    #[test]
    fn il_filtro_trova_per_nome_e_discendenti_caricati() {
        let child = node("vacanze", None);
        let grandchild = node("vacanze-2024", None);
        let child_tree = node("viaggi", Some(vec![grandchild]));
        let root = node("base", Some(vec![child, child_tree]));

        assert!(node_matches(&root, "vacan"));
        assert!(node_matches(&root, "2024"));
        assert!(!node_matches(&root, "montagna"));
    }
}
