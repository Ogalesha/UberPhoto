//! Pannello centrale — griglia di miniature virtualizzata.
//!
//! Con `ScrollArea::show_rows` vengono disegnate solo le righe visibili,
//! quindi la navigazione resta fluida anche con migliaia di fotografie.

use std::path::Path;

use eframe::egui;
use uberphoto_files::SortKey;
use uberphoto_image::ImageService;

use crate::state::{AppState, UiActions, MAX_THUMB_SIZE, MIN_THUMB_SIZE};
use crate::thumbnails::{ThumbCache, ThumbState};
use crate::util::{elide, format_bytes};

pub fn ui(
    ui: &mut egui::Ui,
    state: &mut AppState,
    thumbs: &mut ThumbCache,
    images: &ImageService,
    actions: &mut UiActions,
) {
    toolbar(ui, state, actions);
    ui.separator();

    if state.current_dir.is_none() {
        schermata_iniziale(ui, state, actions);
        return;
    }

    let visible = visible_indices(state);

    if state.loading_dir.is_some() && state.entries.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.spinner();
        });
        return;
    }

    if visible.is_empty() {
        let message = if state.entries.is_empty() {
            "Nessuna fotografia in questa cartella"
        } else {
            "Nessuna foto corrisponde al filtro"
        };
        ui.centered_and_justified(|ui| {
            ui.weak(message);
        });
        return;
    }

    let thumb = state.thumb_size;
    let cell_w = thumb + 12.0;
    let cell_h = thumb + 54.0;
    let spacing = ui.spacing().item_spacing;
    let row_h = cell_h + spacing.y;

    let available_w = ui.available_width();
    let cols = (((available_w + spacing.x) / (cell_w + spacing.x)).floor() as usize).max(1);
    let rows = visible.len().div_ceil(cols);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show_rows(ui, row_h, rows, |ui, row_range| {
            for row in row_range {
                ui.horizontal(|ui| {
                    for col in 0..cols {
                        let visible_index = row * cols + col;
                        if visible_index >= visible.len() {
                            break;
                        }
                        cell(
                            ui,
                            state,
                            thumbs,
                            images,
                            actions,
                            &visible,
                            visible_index,
                            cell_w,
                            cell_h,
                        );
                    }
                });
            }
        });
}

fn toolbar(ui: &mut egui::Ui, state: &mut AppState, actions: &mut UiActions) {
    ui.horizontal(|ui| {
        if ui
            .button("🔄 Aggiorna")
            .on_hover_text("Riscansiona la cartella e rileva le modifiche esterne")
            .clicked()
        {
            actions.refresh = true;
        }

        ui.separator();
        ui.label("Ordina:");
        let before = (state.sort_key, state.sort_ascending);
        egui::ComboBox::from_id_salt("sort_key")
            .selected_text(state.sort_key.label())
            .show_ui(ui, |ui| {
                for key in SortKey::ALL {
                    ui.selectable_value(&mut state.sort_key, key, key.label());
                }
            });
        let direction = if state.sort_ascending { "↑" } else { "↓" };
        if ui
            .button(direction)
            .on_hover_text("Inverti l'ordine")
            .clicked()
        {
            state.sort_ascending = !state.sort_ascending;
        }
        if before != (state.sort_key, state.sort_ascending) {
            state.apply_sort();
        }

        ui.separator();
        ui.label("Miniature:");
        ui.add(
            egui::Slider::new(&mut state.thumb_size, MIN_THUMB_SIZE..=MAX_THUMB_SIZE)
                .show_value(false),
        );

        ui.separator();
        ui.add(
            egui::TextEdit::singleline(&mut state.filter_text)
                .hint_text("Filtra per nome…")
                .desired_width(160.0),
        );
        if !state.filter_text.is_empty() && ui.small_button("✕").clicked() {
            state.filter_text.clear();
        }

        if !state.selection.is_empty() {
            ui.separator();
            ui.weak(format!("{} sel.", state.selection.len()));
            if ui.small_button("Deseleziona").clicked() {
                state.selection.clear();
                state.selection_anchor = None;
            }
        }
    });
}

/// Schermata iniziale senza cartella aperta: logo, invito e azione primaria.
fn schermata_iniziale(ui: &mut egui::Ui, state: &AppState, actions: &mut UiActions) {
    ui.vertical_centered(|ui| {
        ui.add_space(((ui.available_height() - 320.0) * 0.4).max(24.0));
        if let Some(logo) = &state.logo {
            ui.add(
                egui::Image::new(logo)
                    .fit_to_exact_size(egui::vec2(112.0, 112.0))
                    .corner_radius(egui::CornerRadius::same(24)),
            );
            ui.add_space(14.0);
        }
        ui.heading("UberPhoto");
        ui.add_space(6.0);
        ui.weak("Le tue fotografie restano sul tuo disco: apri una cartella per iniziare.");
        ui.add_space(18.0);
        if ui
            .add_sized([210.0, 34.0], egui::Button::new("📂 Apri una cartella…"))
            .clicked()
        {
            actions.pick_folder = true;
        }
    });
}

/// Indici di `state.entries` visibili con il filtro corrente.
fn visible_indices(state: &AppState) -> Vec<usize> {
    let filter = state.filter_text.trim().to_lowercase();
    if filter.is_empty() {
        return (0..state.entries.len()).collect();
    }
    state
        .entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.file_name().to_lowercase().contains(&filter))
        .map(|(index, _)| index)
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn cell(
    ui: &mut egui::Ui,
    state: &mut AppState,
    thumbs: &mut ThumbCache,
    images: &ImageService,
    actions: &mut UiActions,
    visible: &[usize],
    visible_index: usize,
    cell_w: f32,
    cell_h: f32,
) {
    let index = visible[visible_index];
    let entry = &state.entries[index];
    let path = entry.path.clone();
    let mtime = entry.modified;
    let file_name = entry.file_name();
    let path_display = entry.path.display().to_string();
    let format_label = entry.format.label();
    let size_label = format_bytes(entry.byte_size);

    let selected = state.selection.contains(&path);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(cell_w, cell_h), egui::Sense::click());

    if selected {
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::same(4),
            ui.visuals().selection.bg_fill,
        );
    } else if response.hovered() {
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::same(4),
            ui.visuals().widgets.hovered.bg_fill,
        );
    }

    let thumb = state.thumb_size;
    let image_rect = egui::Rect::from_center_size(
        egui::pos2(rect.center().x, rect.top() + 6.0 + thumb / 2.0),
        egui::vec2(thumb, thumb),
    );

    thumbs.ensure(&path, mtime, images, thumb * ui.ctx().pixels_per_point());

    let painter = ui.painter_at(rect);
    let mut hover_detail = format!("{file_name}\n{path_display}");
    match thumbs.get_fresh(&path, mtime) {
        Some(ThumbState::Ready { texture, .. }) => {
            let texture_size = texture.size_vec2();
            let scale =
                (image_rect.width() / texture_size.x).min(image_rect.height() / texture_size.y);
            let shown_rect =
                egui::Rect::from_center_size(image_rect.center(), texture_size * scale);
            painter.image(
                texture.id(),
                shown_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        Some(ThumbState::Failed { error }) => {
            hover_detail.push_str(&format!("\nanteprima non disponibile: {error}"));
            painter.rect_filled(
                image_rect,
                egui::CornerRadius::same(4),
                ui.visuals().extreme_bg_color,
            );
            painter.text(
                image_rect.center(),
                egui::Align2::CENTER_CENTER,
                "n/d",
                egui::FontId::proportional(13.0),
                ui.visuals().weak_text_color(),
            );
        }
        None => {
            painter.rect_filled(
                image_rect,
                egui::CornerRadius::same(4),
                ui.visuals().faint_bg_color,
            );
            painter.text(
                image_rect.center(),
                egui::Align2::CENTER_CENTER,
                "…",
                egui::FontId::proportional(18.0),
                ui.visuals().weak_text_color(),
            );
        }
    }

    let text_x = rect.center().x;
    let name_y = image_rect.bottom() + 3.0;
    painter.text(
        egui::pos2(text_x, name_y),
        egui::Align2::CENTER_TOP,
        elide(&file_name, 26),
        egui::FontId::proportional(12.0),
        ui.visuals().text_color(),
    );
    painter.text(
        egui::pos2(text_x, name_y + 15.0),
        egui::Align2::CENTER_TOP,
        format!("{format_label} · {size_label}"),
        egui::FontId::proportional(10.0),
        ui.visuals().weak_text_color(),
    );
    if let Some((width, height)) = thumbs.dims(&path) {
        painter.text(
            egui::pos2(text_x, name_y + 28.0),
            egui::Align2::CENTER_TOP,
            format!("{width} × {height}"),
            egui::FontId::proportional(10.0),
            ui.visuals().weak_text_color(),
        );
    }

    if response.clicked() {
        let modifiers = ui.input(|input| input.modifiers);
        if modifiers.shift {
            select_range(state, visible, visible_index);
        } else if modifiers.command {
            toggle_selection(state, &path);
        } else {
            set_single_selection(state, &path);
        }
    }
    if response.secondary_clicked() {
        set_single_selection(state, &path);
    }
    if response.double_clicked() {
        actions.open_preview = Some(path.clone());
    }

    response.on_hover_text(hover_detail);
}

fn set_single_selection(state: &mut AppState, path: &Path) {
    state.selection.clear();
    state.selection.insert(path.to_path_buf());
    state.selection_anchor = Some(path.to_path_buf());
}

fn toggle_selection(state: &mut AppState, path: &Path) {
    if !state.selection.remove(path) {
        state.selection.insert(path.to_path_buf());
    }
    state.selection_anchor = Some(path.to_path_buf());
}

/// Selezione ad intervallo con Shift: dall'ancora (o dalla cella cliccata)
/// fino alla cella cliccata, secondo l'ordine visibile corrente.
fn select_range(state: &mut AppState, visible: &[usize], visible_index: usize) {
    let anchor = state
        .selection_anchor
        .as_ref()
        .and_then(|anchor| {
            visible
                .iter()
                .position(|&index| &state.entries[index].path == anchor)
        })
        .unwrap_or(visible_index);
    let (start, end) = if anchor <= visible_index {
        (anchor, visible_index)
    } else {
        (visible_index, anchor)
    };
    state.selection.clear();
    for &index in &visible[start..=end] {
        state.selection.insert(state.entries[index].path.clone());
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::SystemTime;

    use uberphoto_core::{FormatKind, PhotoAsset};

    use super::visible_indices;
    use crate::state::AppState;

    fn asset(name: &str) -> PhotoAsset {
        PhotoAsset {
            path: PathBuf::from(name),
            format: FormatKind::Jpeg,
            byte_size: 1,
            modified: Some(SystemTime::UNIX_EPOCH),
        }
    }

    fn state_with(entries: Vec<PhotoAsset>, filter: &str) -> AppState {
        let mut state = AppState::new(Default::default());
        state.entries = entries;
        state.filter_text = filter.to_owned();
        state
    }

    #[test]
    fn senza_filtro_tutte_le_voci_sono_visibili() {
        let state = state_with(vec![asset("a.jpg"), asset("b.jpg")], "");
        assert_eq!(visible_indices(&state), vec![0, 1]);
    }

    #[test]
    fn il_filtro_e_case_insensitive() {
        let state = state_with(
            vec![
                asset("Foto_Mare.jpg"),
                asset("montagna.png"),
                asset("mare_2.jpg"),
            ],
            "MARE",
        );
        assert_eq!(visible_indices(&state), vec![0, 2]);
    }
}
