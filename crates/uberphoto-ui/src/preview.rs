//! Finestra di anteprima grande con zoom e pan.
//!
//! L'immagine viene decodificata in background (worker dedicato del
//! [`ImageService`], con politica "latest wins") e caricata come texture.
//! La navigazione è puramente visuale: nessuna modifica ai pixel.
//!
//! Quando la scheda Colori produce un'anteprima modificata per la stessa
//! foto, la finestra mostra quella (marcata "DOPO"); col pulsante "Prima"
//! della scheda Colori si torna temporaneamente all'originale.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use eframe::egui;
use uberphoto_image::{ImageService, LoadedImage};

pub struct PreviewState {
    open: bool,
    path: Option<PathBuf>,
    mtime: Option<SystemTime>,
    texture: Option<egui::TextureHandle>,
    size_px: egui::Vec2,
    /// Anteprima con la ricetta di editing applicata, condivisa con la
    /// scheda Colori (è la stessa texture, il costo è quello di un handle).
    edited: Option<egui::TextureHandle>,
    edited_size: egui::Vec2,
    /// Mostra l'anteprima modificata quando disponibile.
    prefer_edited: bool,
    loading: bool,
    error: Option<String>,
    zoom: f32,
    offset: egui::Vec2,
    needs_fit: bool,
}

impl PreviewState {
    pub fn new() -> Self {
        Self {
            open: false,
            path: None,
            mtime: None,
            texture: None,
            size_px: egui::Vec2::ZERO,
            edited: None,
            edited_size: egui::Vec2::ZERO,
            prefer_edited: true,
            loading: false,
            error: None,
            zoom: 1.0,
            offset: egui::Vec2::ZERO,
            needs_fit: true,
        }
    }

    /// Apre (o riporta in primo piano) l'anteprima della foto indicata,
    /// richiedendone la decodifica se non è già quella corrente.
    pub fn open_for(&mut self, path: &Path, mtime: Option<SystemTime>, images: &ImageService) {
        let already_current =
            self.open && self.path.as_deref() == Some(path) && self.mtime == mtime;
        self.open = true;
        if already_current && (self.texture.is_some() || self.loading) {
            return;
        }

        self.path = Some(path.to_path_buf());
        self.mtime = mtime;
        self.texture = None;
        self.edited = None;
        self.edited_size = egui::Vec2::ZERO;
        self.size_px = egui::Vec2::ZERO;
        self.loading = true;
        self.error = None;
        self.zoom = 1.0;
        self.offset = egui::Vec2::ZERO;
        self.needs_fit = true;
        images.request_preview(path.to_path_buf(), mtime);
    }

    /// Vero se `path`/`mtime` corrispondono all'anteprima corrente
    /// (i risultati tardivi per foto ormai superate vengono scartati).
    pub fn accepts(&self, path: &Path, mtime: Option<SystemTime>) -> bool {
        self.open && self.path.as_deref() == Some(path) && self.mtime == mtime
    }

    pub fn set_image(&mut self, ctx: &egui::Context, image: &LoadedImage) {
        let color = egui::ColorImage::from_rgba_unmultiplied(
            [image.width as usize, image.height as usize],
            &image.rgba,
        );
        self.texture = Some(ctx.load_texture("preview", color, egui::TextureOptions::LINEAR));
        self.size_px = egui::vec2(image.width as f32, image.height as f32);
        self.loading = false;
        self.needs_fit = true;
    }

    /// Anteprima modificata prodotta dalla scheda Colori per la foto
    /// corrente (la texture è condivisa, non duplicata).
    pub fn set_edited(&mut self, texture: egui::TextureHandle, size: egui::Vec2) {
        self.edited = Some(texture);
        self.edited_size = size;
        self.needs_fit = true;
    }

    /// Nessuna anteprima modificata: si torna all'originale.
    pub fn clear_edited(&mut self) {
        self.edited = None;
        self.edited_size = egui::Vec2::ZERO;
    }

    /// Scelta tra originale e modificata (il pulsante "Prima" dell'editor).
    pub fn set_prefer_edited(&mut self, prefer: bool) {
        self.prefer_edited = prefer;
    }

    pub fn set_error(&mut self, error: String) {
        self.loading = false;
        self.error = Some(error);
    }

    pub fn is_loading(&self) -> bool {
        self.open && self.loading
    }

    pub fn ui(&mut self, ctx: &egui::Context) {
        if !self.open {
            return;
        }

        let screen = ctx.content_rect().size();
        let title = self
            .path
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| format!("Anteprima — {}", name.to_string_lossy()))
            .unwrap_or_else(|| "Anteprima".to_owned());

        let mut open = self.open;
        egui::Window::new(title)
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .fixed_size(screen * 0.92)
            .show(ctx, |ui| self.window_contents(ui));
        self.open = open;

        // Alla chiusura la texture viene liberata: l'anteprima è ricreabile
        // a costo di una nuova decodifica.
        if !self.open {
            self.texture = None;
            self.edited = None;
            self.edited_size = egui::Vec2::ZERO;
            self.path = None;
            self.mtime = None;
            self.error = None;
            self.loading = false;
        }
    }

    fn window_contents(&mut self, ui: &mut egui::Ui) {
        if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.open = false;
        }

        ui.horizontal(|ui| {
            if ui.button("−").on_hover_text("Riduci").clicked() {
                self.zoom = (self.zoom / 1.25).clamp(0.02, 32.0);
                self.needs_fit = false;
            }
            ui.label(format!("{:.0}%", self.zoom * 100.0));
            if ui.button("+").on_hover_text("Ingrandisci").clicked() {
                self.zoom = (self.zoom * 1.25).clamp(0.02, 32.0);
                self.needs_fit = false;
            }
            if ui.button("Adatta").clicked() {
                self.needs_fit = true;
            }
            if ui
                .button("1:1")
                .on_hover_text("Un pixel fisico dello schermo per pixel dell'immagine")
                .clicked()
            {
                self.zoom = 1.0;
                self.offset = egui::Vec2::ZERO;
                self.needs_fit = false;
            }
            ui.separator();
            if ui.button("Chiudi").clicked() {
                self.open = false;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.weak("trascina per spostare · rotella per zoomare · doppio clic per adattare");
            });
        });
        ui.separator();

        let (rect, response) =
            ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());

        if response.hovered() {
            let scroll = ui.input(|input| input.smooth_scroll_delta.y);
            if scroll != 0.0 {
                let factor = (scroll * 0.002).exp();
                self.zoom = (self.zoom * factor).clamp(0.02, 32.0);
                self.needs_fit = false;
            }
        }
        if response.dragged() {
            self.offset += response.drag_delta();
        }
        if response.double_clicked() {
            self.offset = egui::Vec2::ZERO;
            self.needs_fit = true;
        }

        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, egui::CornerRadius::ZERO, egui::Color32::from_gray(18));

        if let Some(error) = &self.error {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                format!("Impossibile mostrare l'anteprima:\n{error}"),
                egui::FontId::proportional(14.0),
                egui::Color32::LIGHT_RED,
            );
            return;
        }

        let attiva = if self.prefer_edited && self.edited.is_some() {
            self.edited
                .as_ref()
                .map(|texture| (texture, self.edited_size))
        } else {
            None
        }
        .or_else(|| self.texture.as_ref().map(|texture| (texture, self.size_px)));

        let Some((texture, size_px)) = attiva else {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Caricamento anteprima…",
                egui::FontId::proportional(14.0),
                egui::Color32::GRAY,
            );
            return;
        };

        let ppp = ui.ctx().pixels_per_point();
        let size_pts = size_px / ppp;
        if self.needs_fit && size_pts.x > 0.0 && size_pts.y > 0.0 {
            self.zoom = (rect.width() / size_pts.x)
                .min(rect.height() / size_pts.y)
                .min(1.0);
            self.offset = egui::Vec2::ZERO;
            self.needs_fit = false;
        }

        let shown = (size_pts * self.zoom).max(egui::Vec2::splat(1.0));
        let image_rect = egui::Rect::from_center_size(rect.center() + self.offset, shown);
        painter.image(
            texture.id(),
            image_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );

        if self.edited.is_some() {
            let etichetta = if self.prefer_edited { "DOPO" } else { "PRIMA" };
            painter.text(
                rect.left_top() + egui::vec2(10.0, 10.0),
                egui::Align2::LEFT_TOP,
                etichetta,
                egui::FontId::proportional(11.0),
                egui::Color32::from_gray(200),
            );
        }
    }
}
