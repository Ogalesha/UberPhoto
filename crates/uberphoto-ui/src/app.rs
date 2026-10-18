//! Applicazione e orchestrazione dei pannelli.
//!
//! Ciclo del frame:
//! 1. risveglio delle cache (LRU miniature);
//! 2. raccolta dei risultati di scansione e decodifica in background;
//! 3. disegno dei pannelli, che raccolgono [`UiActions`] senza mutare lo stato;
//! 4. esecuzione delle azioni (cambio cartella, refresh, anteprima).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use eframe::egui;
use uberphoto_files::{DirNode, DirectoryScanner};
use uberphoto_image::{ImageKind, ImageService};

use crate::browser;
use crate::explorer;
use crate::inspector;
use crate::preview::PreviewState;
use crate::state::{AppState, PersistedState, UiActions};
use crate::thumbnails::ThumbCache;
use crate::util::elide;

pub struct UberPhotoApp {
    state: AppState,
    scanner: DirectoryScanner,
    images: ImageService,
    thumbs: ThumbCache,
    preview: PreviewState,
    /// Chiusura automatica dopo N secondi, usata dagli smoke test di CI
    /// (variabile d'ambiente `UBERPHOTO_SMOKE_EXIT_SECS`).
    smoke_exit: Option<(std::time::Instant, f64)>,
}

impl UberPhotoApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        setup_visuals(&cc.egui_ctx);

        let persisted: PersistedState = cc
            .storage
            .and_then(|storage| eframe::get_value(storage, eframe::APP_KEY))
            .unwrap_or_default();
        let last_dir = persisted.last_dir.clone();

        let repaint_ctx = cc.egui_ctx.clone();
        let repaint: Arc<dyn Fn() + Send + Sync> = Arc::new(move || repaint_ctx.request_repaint());

        let smoke_exit = std::env::var("UBERPHOTO_SMOKE_EXIT_SECS")
            .ok()
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|seconds| *seconds > 0.0)
            .map(|seconds| (std::time::Instant::now(), seconds));

        let mut app = Self {
            state: AppState::new(persisted),
            scanner: DirectoryScanner::new(repaint.clone()),
            images: ImageService::new(repaint),
            thumbs: ThumbCache::new(),
            preview: PreviewState::new(),
            smoke_exit,
        };
        app.state.logo = load_logo(&cc.egui_ctx);

        if let Some(dir) = last_dir {
            if dir.is_dir() {
                app.open_root(dir);
            }
        }

        app
    }

    /// Apre `dir` come nuova radice dell'esploratore e la scansiona.
    fn open_root(&mut self, dir: PathBuf) {
        let mut root = DirNode::new_root(dir.clone());
        if let Some(message) = root.load_children() {
            self.state.error = Some(message);
        }
        self.state.root = Some(root);
        self.state.push_recent(&dir);
        self.start_scan(dir);
    }

    /// Naviga il browser su `dir` (senza toccare la radice).
    fn start_scan(&mut self, dir: PathBuf) {
        self.state.current_dir = Some(dir.clone());
        self.state.loading_dir = Some(dir.clone());
        self.state.entries.clear();
        self.state.selection.clear();
        self.state.selection_anchor = None;
        self.scanner.request(&dir);
        tracing::info!(dir = %dir.display(), "scansione avviata");
    }

    fn pump_scan_results(&mut self) {
        while let Some(result) = self.scanner.try_recv() {
            if self.state.current_dir.as_deref() != Some(result.dir.as_path()) {
                continue; // risultato superato da una navigazione più recente
            }
            self.state.loading_dir = None;
            match result.error {
                Some(error) => {
                    tracing::warn!(dir = %result.dir.display(), "scansione fallita: {error}");
                    self.state.error = Some(error);
                    self.state.entries.clear();
                }
                None => {
                    self.state.entries = result.entries;
                    self.state.apply_sort();
                }
            }
        }
    }

    fn pump_image_results(&mut self, ctx: &egui::Context) {
        for result in self.images.drain() {
            match result.kind {
                ImageKind::Thumbnail => {
                    self.thumbs
                        .on_result(ctx, result.path, result.mtime, result.outcome);
                }
                ImageKind::Preview => {
                    if self.preview.accepts(&result.path, result.mtime) {
                        match result.outcome {
                            Ok(image) => self.preview.set_image(ctx, image),
                            Err(error) => self.preview.set_error(error),
                        }
                    }
                }
            }
        }
    }

    fn handle_actions(&mut self, actions: UiActions) {
        let picked = if actions.pick_folder {
            rfd::FileDialog::new()
                .set_title("Scegli la cartella da aprire")
                .pick_folder()
        } else {
            None
        };
        if let Some(dir) = actions.open_folder.or(picked) {
            if dir.is_dir() {
                self.open_root(dir);
            } else {
                self.state.error =
                    Some(format!("La cartella \"{}\" non esiste più", dir.display()));
            }
        }
        if let Some(dir) = actions.browse_dir {
            if self.state.current_dir.as_deref() != Some(dir.as_path()) {
                self.start_scan(dir);
            }
        }
        if actions.refresh {
            if let Some(dir) = self.state.current_dir.clone() {
                self.start_scan(dir);
            }
        }
        if let Some(path) = actions.open_preview {
            let mtime = self.state.entry_mtime(&path);
            self.preview.open_for(&path, mtime, &self.images);
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if self.state.loading_dir.is_some() {
                ui.spinner();
                ui.label("Scansione in corso…");
            } else if let Some(dir) = &self.state.current_dir {
                ui.label(format!(
                    "{} — {} fotografie",
                    elide(&dir.display().to_string(), 70),
                    self.state.entries.len()
                ));
            } else {
                ui.label("Nessuna cartella aperta");
            }

            ui.separator();
            match self.state.selection.len() {
                0 => {}
                1 => {
                    ui.label("1 selezionata");
                }
                count => {
                    ui.label(format!("{count} selezionate"));
                }
            }

            let (cached, inflight) = self.thumbs.stats();
            ui.separator();
            ui.weak(format!("Miniature: {cached} in cache · {inflight} in coda"));

            if let Some(error) = self.state.error.clone() {
                ui.separator();
                ui.colored_label(egui::Color32::from_rgb(224, 122, 118), elide(&error, 90));
                if ui.small_button("✕").clicked() {
                    self.state.error = None;
                }
            }
        });
    }
}

/// Mondo visivo: scuro professionale fotografico — chrome neutro e accenti
/// misurati, così le fotografie dominano la scena senza falsarne i colori.
fn setup_visuals(ctx: &egui::Context) {
    let canvas = egui::Color32::from_rgb(16, 16, 18);
    let panel = egui::Color32::from_rgb(28, 28, 31);
    let surface = egui::Color32::from_rgb(38, 38, 42);
    let surface_hover = egui::Color32::from_rgb(46, 46, 51);
    let surface_active = egui::Color32::from_rgb(58, 58, 64);
    let stroke = egui::Color32::from_rgb(52, 52, 58);
    let accent = egui::Color32::from_rgb(110, 155, 209);
    let selection_bg = egui::Color32::from_rgb(47, 74, 102);

    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = panel;
    visuals.window_fill = panel;
    visuals.extreme_bg_color = canvas;
    visuals.faint_bg_color = egui::Color32::from_rgb(33, 33, 37);
    visuals.hyperlink_color = accent;
    visuals.window_corner_radius = egui::CornerRadius::same(10);

    visuals.widgets.noninteractive.bg_fill = panel;
    visuals.widgets.noninteractive.weak_bg_fill = panel;
    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, stroke);

    visuals.widgets.inactive.bg_fill = surface;
    visuals.widgets.inactive.weak_bg_fill = surface;
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, stroke);

    visuals.widgets.hovered.bg_fill = surface_hover;
    visuals.widgets.hovered.weak_bg_fill = surface_hover;
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, accent);

    visuals.widgets.active.bg_fill = surface_active;
    visuals.widgets.active.weak_bg_fill = surface_active;
    visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, accent);

    visuals.selection.bg_fill = selection_bg;
    visuals.selection.stroke = egui::Stroke::new(1.0, accent);

    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = egui::CornerRadius::same(6);
    }

    ctx.set_theme(egui::Theme::Dark);
    ctx.set_visuals(visuals);
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
    });
}

/// Decodifica il logo incorporato una sola volta per la texture della UI.
fn load_logo(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let image = uberphoto_image::decode_bytes_rgba(crate::APP_ICON_PNG, Some(256)).ok()?;
    let color_image = egui::ColorImage::from_rgba_unmultiplied(
        [image.width as usize, image.height as usize],
        &image.rgba,
    );
    Some(ctx.load_texture("uberphoto-logo", color_image, egui::TextureOptions::LINEAR))
}

impl eframe::App for UberPhotoApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        self.thumbs.begin_frame();
        self.pump_scan_results();
        self.pump_image_results(&ctx);

        let mut actions = UiActions::default();

        egui::Panel::bottom("status_bar").show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("explorer")
            .resizable(true)
            .default_size(260.0)
            .size_range(180.0..=520.0)
            .show(ui, |ui| explorer::ui(ui, &mut self.state, &mut actions));
        egui::Panel::right("inspector")
            .resizable(true)
            .default_size(320.0)
            .size_range(240.0..=640.0)
            .show(ui, |ui| {
                inspector::ui(ui, &mut self.state, &self.thumbs, &mut actions);
            });
        egui::CentralPanel::default().show(ui, |ui| {
            browser::ui(
                ui,
                &mut self.state,
                &mut self.thumbs,
                &self.images,
                &mut actions,
            );
        });

        self.handle_actions(actions);
        self.preview.ui(&ctx);

        // Mantiene animati spinner e avanzamento durante le operazioni lunghe.
        if self.state.loading_dir.is_some()
            || self.thumbs.has_inflight()
            || self.preview.is_loading()
        {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        if let Some((start, seconds)) = self.smoke_exit {
            if start.elapsed().as_secs_f64() >= seconds {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, &self.state.persisted());
    }
}
