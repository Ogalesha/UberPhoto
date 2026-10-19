//! Editing non distruttivo: ricetta delle regolazioni e pipeline colore.
//!
//! La ricetta è un piccolo documento serializzabile che descrive *come*
//! modificare i pixel; non tocca mai il file originale. L'anteprima applica
//! la ricetta a una copia ridotta dell'immagine, l'export (Fase 6) la
//! applicherà all'immagine completa.
//!
//! La pipeline procede in quest'ordine:
//! 1. geometria: ritaglio, poi rotazione in passi di 90°;
//! 2. correzioni per canale (bilanciamento del bianco, esposizione, curva
//!    tonale, gamma), calcolate una volta come tre LUT da 256 valori;
//! 3. saturazione e vibrance (richiedono la luminanza dei tre canali);
//! 4. nitidezza (maschera di contrasto su una sfocatura 3×3).

use serde::{Deserialize, Serialize};

use crate::decode::LoadedImage;

/// Regione della ricetta, usata per il ripristino selettivo e le etichette.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EditField {
    Exposure,
    Brightness,
    Contrast,
    Saturation,
    Vibrance,
    Temperature,
    Tint,
    Highlights,
    Shadows,
    Whites,
    Blacks,
    Gamma,
    Sharpness,
    Rotation,
    Crop,
}

impl EditField {
    pub const ALL: [EditField; 15] = [
        EditField::Exposure,
        EditField::Brightness,
        EditField::Contrast,
        EditField::Saturation,
        EditField::Vibrance,
        EditField::Temperature,
        EditField::Tint,
        EditField::Highlights,
        EditField::Shadows,
        EditField::Whites,
        EditField::Blacks,
        EditField::Gamma,
        EditField::Sharpness,
        EditField::Rotation,
        EditField::Crop,
    ];

    pub fn label(self) -> &'static str {
        match self {
            EditField::Exposure => "Esposizione",
            EditField::Brightness => "Luminosità",
            EditField::Contrast => "Contrasto",
            EditField::Saturation => "Saturazione",
            EditField::Vibrance => "Vibrance",
            EditField::Temperature => "Temperatura",
            EditField::Tint => "Tinta",
            EditField::Highlights => "Alte luci",
            EditField::Shadows => "Ombre",
            EditField::Whites => "Bianchi",
            EditField::Blacks => "Neri",
            EditField::Gamma => "Gamma",
            EditField::Sharpness => "Nitidezza",
            EditField::Rotation => "Rotazione",
            EditField::Crop => "Ritaglio",
        }
    }

    /// Riporta il campo al valore neutro della ricetta.
    pub fn reset(self, recipe: &mut EditRecipe) {
        let neutra = EditRecipe::default();
        match self {
            EditField::Exposure => recipe.exposure = neutra.exposure,
            EditField::Brightness => recipe.brightness = neutra.brightness,
            EditField::Contrast => recipe.contrast = neutra.contrast,
            EditField::Saturation => recipe.saturation = neutra.saturation,
            EditField::Vibrance => recipe.vibrance = neutra.vibrance,
            EditField::Temperature => recipe.temperature = neutra.temperature,
            EditField::Tint => recipe.tint = neutra.tint,
            EditField::Highlights => recipe.highlights = neutra.highlights,
            EditField::Shadows => recipe.shadows = neutra.shadows,
            EditField::Whites => recipe.whites = neutra.whites,
            EditField::Blacks => recipe.blacks = neutra.blacks,
            EditField::Gamma => recipe.gamma = neutra.gamma,
            EditField::Sharpness => recipe.sharpness = neutra.sharpness,
            EditField::Rotation => recipe.rotation = neutra.rotation,
            EditField::Crop => recipe.crop = None,
        }
    }
}

/// Rettangolo di ritaglio in coordinate normalizzate 0-1 (angolo superiore
/// sinistro e inferiore destro), riferite all'immagine orientata.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CropRect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl CropRect {
    pub fn full() -> Self {
        Self {
            left: 0.0,
            top: 0.0,
            right: 1.0,
            bottom: 1.0,
        }
    }

    pub fn is_full(&self) -> bool {
        self.left <= 0.0 && self.top <= 0.0 && self.right >= 1.0 && self.bottom >= 1.0
    }

    /// Limiti sicuri: almeno l'1% per lato, ordine degli angoli garantito.
    pub fn sanitized(&self) -> Self {
        let left = self.left.clamp(0.0, 0.99);
        let top = self.top.clamp(0.0, 0.99);
        let right = self.right.clamp(left + 0.01, 1.0);
        let bottom = self.bottom.clamp(top + 0.01, 1.0);
        Self {
            left,
            top,
            right,
            bottom,
        }
    }
}

/// Ricetta di editing non distruttivo. I valori neutri (tutti a zero,
/// `gamma` a 1, nessuna rotazione né ritaglio) riproducono l'originale.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditRecipe {
    /// Compensazione dell'esposizione in stop EV, -5..=5.
    pub exposure: f32,
    /// Luminosità, -100..=100.
    pub brightness: f32,
    /// Contrasto, -100..=100.
    pub contrast: f32,
    /// Saturazione, -100..=100.
    pub saturation: f32,
    /// Vibrance, -100..=100.
    pub vibrance: f32,
    /// Temperatura colore, -100..=100 (positivo = più caldo).
    pub temperature: f32,
    /// Tinta verde/magenta, -100..=100 (positivo = magenta).
    pub tint: f32,
    /// Alte luci, -100..=100.
    pub highlights: f32,
    /// Ombre, -100..=100.
    pub shadows: f32,
    /// Bianchi, -100..=100.
    pub whites: f32,
    /// Neri, -100..=100.
    pub blacks: f32,
    /// Gamma, 0.2..=3.0 (neutro 1.0).
    pub gamma: f32,
    /// Nitidezza, 0..=100.
    pub sharpness: f32,
    /// Rotazione in senso orario: 0, 90, 180 o 270 gradi.
    pub rotation: u16,
    /// Ritaglio normalizzato; `None` = immagine intera.
    pub crop: Option<CropRect>,
}

impl Default for EditRecipe {
    fn default() -> Self {
        Self {
            exposure: 0.0,
            brightness: 0.0,
            contrast: 0.0,
            saturation: 0.0,
            vibrance: 0.0,
            temperature: 0.0,
            tint: 0.0,
            highlights: 0.0,
            shadows: 0.0,
            whites: 0.0,
            blacks: 0.0,
            gamma: 1.0,
            sharpness: 0.0,
            rotation: 0,
            crop: None,
        }
    }
}

impl EditRecipe {
    /// Vero se la ricetta non altera l'immagine.
    pub fn is_identity(&self) -> bool {
        *self == Self::default()
    }

    /// Riporta tutti i valori nei limiti previsti dai controlli.
    pub fn sanitized(&self) -> Self {
        Self {
            exposure: self.exposure.clamp(-5.0, 5.0),
            brightness: clamp_slider(self.brightness),
            contrast: clamp_slider(self.contrast),
            saturation: clamp_slider(self.saturation),
            vibrance: clamp_slider(self.vibrance),
            temperature: clamp_slider(self.temperature),
            tint: clamp_slider(self.tint),
            highlights: clamp_slider(self.highlights),
            shadows: clamp_slider(self.shadows),
            whites: clamp_slider(self.whites),
            blacks: clamp_slider(self.blacks),
            gamma: self.gamma.clamp(0.2, 3.0),
            sharpness: self.sharpness.clamp(0.0, 100.0),
            rotation: match self.rotation % 360 {
                90 => 90,
                180 => 180,
                270 => 270,
                _ => 0,
            },
            crop: self.crop.map(|crop| crop.sanitized()),
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_owned())
    }

    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

fn clamp_slider(value: f32) -> f32 {
    value.clamp(-100.0, 100.0)
}

/// Pixel elaborati: buffer RGBA8 e dimensioni (possono differire
/// dall'origine dopo ritaglio o rotazione).
#[derive(Debug, Clone, PartialEq)]
pub struct EditedPixels {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Applica la ricetta a un'immagine decodificata.
pub fn apply_recipe(image: &LoadedImage, recipe: &EditRecipe) -> EditedPixels {
    apply_recipe_rgba(&image.rgba, image.width, image.height, recipe)
}

/// Applica la ricetta a un buffer RGBA8.
///
/// La funzione è deterministica e non modifica l'input: con una ricetta
/// neutra il risultato coincide byte per byte con la sorgente.
pub fn apply_recipe_rgba(
    rgba: &[u8],
    width: u32,
    height: u32,
    recipe: &EditRecipe,
) -> EditedPixels {
    let recipe = recipe.sanitized();
    let (mut pixels, width, height) = geometry(rgba, width, height, &recipe);
    if width == 0 || height == 0 {
        return EditedPixels {
            rgba: pixels,
            width,
            height,
        };
    }

    if needs_channel_ops(&recipe) {
        let luts = build_luts(&recipe);
        for pixel in pixels.chunks_exact_mut(4) {
            pixel[0] = luts[0][pixel[0] as usize];
            pixel[1] = luts[1][pixel[1] as usize];
            pixel[2] = luts[2][pixel[2] as usize];
        }
    }

    if recipe.saturation != 0.0 || recipe.vibrance != 0.0 {
        apply_saturation(&mut pixels, recipe.saturation, recipe.vibrance);
    }

    if recipe.sharpness > 0.0 {
        apply_sharpness(&mut pixels, width, height, recipe.sharpness);
    }

    EditedPixels {
        rgba: pixels,
        width,
        height,
    }
}

fn needs_channel_ops(recipe: &EditRecipe) -> bool {
    recipe.exposure != 0.0
        || recipe.brightness != 0.0
        || recipe.contrast != 0.0
        || recipe.temperature != 0.0
        || recipe.tint != 0.0
        || recipe.highlights != 0.0
        || recipe.shadows != 0.0
        || recipe.whites != 0.0
        || recipe.blacks != 0.0
        || (recipe.gamma - 1.0).abs() > f32::EPSILON
}

/// Ritaglio, poi rotazione. Restituisce sempre un buffer nuovo.
fn geometry(rgba: &[u8], width: u32, height: u32, recipe: &EditRecipe) -> (Vec<u8>, u32, u32) {
    if width == 0 || height == 0 {
        return (rgba.to_vec(), width, height);
    }
    let (cropped, width, height) = match recipe.crop.filter(|crop| !crop.is_full()) {
        Some(crop) => crop_pixels(rgba, width, height, crop.sanitized()),
        None => (rgba.to_vec(), width, height),
    };
    match recipe.rotation % 360 {
        90 => rotate90(&cropped, width, height),
        180 => rotate180(&cropped, width, height),
        270 => {
            let (ruotata, width, height) = rotate90(&cropped, width, height);
            rotate180(&ruotata, width, height)
        }
        _ => (cropped, width, height),
    }
}

fn crop_pixels(rgba: &[u8], width: u32, height: u32, crop: CropRect) -> (Vec<u8>, u32, u32) {
    let left = ((crop.left * width as f32).floor() as u32).min(width - 1);
    let top = ((crop.top * height as f32).floor() as u32).min(height - 1);
    let right = ((crop.right * width as f32).ceil() as u32).clamp(left + 1, width);
    let bottom = ((crop.bottom * height as f32).ceil() as u32).clamp(top + 1, height);
    let new_width = right - left;
    let new_height = bottom - top;
    let mut out = Vec::with_capacity((new_width * new_height * 4) as usize);
    for y in top..bottom {
        let start = ((y * width + left) * 4) as usize;
        out.extend_from_slice(&rgba[start..start + (new_width * 4) as usize]);
    }
    (out, new_width, new_height)
}

/// Rotazione di 90° in senso orario: `dest(x, y) = sorgente(y, h-1-x)`.
fn rotate90(rgba: &[u8], width: u32, height: u32) -> (Vec<u8>, u32, u32) {
    let (new_width, new_height) = (height, width);
    let mut out = vec![0u8; rgba.len()];
    for y in 0..new_height {
        for x in 0..new_width {
            let source = (((height - 1 - x) * width + y) * 4) as usize;
            let destination = ((y * new_width + x) * 4) as usize;
            out[destination..destination + 4].copy_from_slice(&rgba[source..source + 4]);
        }
    }
    (out, new_width, new_height)
}

fn rotate180(rgba: &[u8], width: u32, height: u32) -> (Vec<u8>, u32, u32) {
    let mut out = vec![0u8; rgba.len()];
    for y in 0..height {
        for x in 0..width {
            let source = (((height - 1 - y) * width + (width - 1 - x)) * 4) as usize;
            let destination = ((y * width + x) * 4) as usize;
            out[destination..destination + 4].copy_from_slice(&rgba[source..source + 4]);
        }
    }
    (out, width, height)
}

/// Guadagni per canale del bilanciamento del bianco, normalizzati per
/// preservare la luminanza complessiva.
fn white_balance_gains(temperature: f32, tint: f32) -> (f32, f32, f32) {
    let t = temperature / 100.0;
    let ti = tint / 100.0;
    let r = (1.0 + 0.35 * t) * (1.0 + 0.20 * ti);
    let g = 1.0 - 0.20 * ti;
    let b = (1.0 - 0.35 * t) * (1.0 + 0.20 * ti);
    let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    if luma <= f32::EPSILON {
        (1.0, 1.0, 1.0)
    } else {
        (r / luma, g / luma, b / luma)
    }
}

/// Curve per canale: per ogni valore 0-255 il valore elaborato. Raccoglie
/// bilanciamento del bianco, esposizione, curva tonale e gamma.
fn build_luts(recipe: &EditRecipe) -> [[u8; 256]; 3] {
    let (gain_r, gain_g, gain_b) = white_balance_gains(recipe.temperature, recipe.tint);
    let exposure = 2f32.powf(recipe.exposure);
    let gains = [gain_r * exposure, gain_g * exposure, gain_b * exposure];
    let mut luts = [[0u8; 256]; 3];
    for (channel, lut) in luts.iter_mut().enumerate() {
        for (value, slot) in lut.iter_mut().enumerate() {
            let linear = srgb_to_linear(value as f32 / 255.0) * gains[channel];
            let s = tone_curve(linear_to_srgb(linear), recipe);
            *slot = (s.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        }
    }
    luts
}

/// Curva tonale sulla scala percettiva (0-1): neri, ombre, alte luci,
/// bianchi, luminosità, contrasto e gamma.
fn tone_curve(s: f32, recipe: &EditRecipe) -> f32 {
    let mut s = s;

    let blacks = recipe.blacks / 100.0;
    if blacks != 0.0 {
        s += blacks * 0.20 * (1.0 - s).powi(3);
    }

    let shadows = recipe.shadows / 100.0;
    if shadows != 0.0 {
        s += shadows * 0.35 * (1.0 - s).powi(2);
    }

    let highlights = recipe.highlights / 100.0;
    if highlights != 0.0 {
        s += highlights * 0.35 * s * s;
    }

    let whites = recipe.whites / 100.0;
    if whites != 0.0 {
        s += whites * 0.20 * s.powi(3);
    }

    let brightness = recipe.brightness / 100.0;
    if brightness != 0.0 {
        s += brightness * 0.15;
    }

    let contrast = recipe.contrast / 100.0;
    if contrast != 0.0 {
        s = (s - 0.5) * (1.0 + contrast * 0.5) + 0.5;
    }

    s = s.clamp(0.0, 1.0);
    if (recipe.gamma - 1.0).abs() > f32::EPSILON {
        s = s.powf(1.0 / recipe.gamma);
    }
    s
}

fn srgb_to_linear(s: f32) -> f32 {
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(linear: f32) -> f32 {
    let linear = linear.max(0.0);
    if linear <= 0.003_130_8 {
        12.92 * linear
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    }
}

/// Saturazione uniforme e vibrance (più delicata sui colori già saturi).
fn apply_saturation(pixels: &mut [u8], saturation: f32, vibrance: f32) {
    let saturation = saturation / 100.0;
    let vibrance = vibrance / 100.0;
    for pixel in pixels.chunks_exact_mut(4) {
        let r = f32::from(pixel[0]);
        let g = f32::from(pixel[1]);
        let b = f32::from(pixel[2]);
        let massimo = r.max(g).max(b);
        let minimo = r.min(g).min(b);
        let luma = 0.299 * r + 0.587 * g + 0.114 * b;
        let mut factor = 1.0 + saturation;
        if vibrance != 0.0 {
            factor += vibrance * (1.0 - (massimo - minimo) / 255.0);
        }
        pixel[0] = clamp_u8(luma + (r - luma) * factor);
        pixel[1] = clamp_u8(luma + (g - luma) * factor);
        pixel[2] = clamp_u8(luma + (b - luma) * factor);
    }
}

/// Maschera di contrasto: `v + k·(v − sfocatura 3×3)`.
fn apply_sharpness(pixels: &mut [u8], width: u32, height: u32, amount: f32) {
    let k = (amount / 100.0) * 1.2;
    if k <= 0.0 {
        return;
    }
    let (w, h) = (width as usize, height as usize);
    let blur = blur3x3(pixels, w, h);
    for (index, pixel) in pixels.iter_mut().enumerate() {
        if index % 4 == 3 {
            continue;
        }
        let value = f32::from(*pixel);
        let base = f32::from(blur[index]);
        *pixel = clamp_u8(value + k * (value - base));
    }
}

fn blur3x3(pixels: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; pixels.len()];
    for y in 0..h {
        for x in 0..w {
            let mut sums = [0u32; 3];
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let nx = (x as i64 + dx).clamp(0, w as i64 - 1) as usize;
                    let ny = (y as i64 + dy).clamp(0, h as i64 - 1) as usize;
                    let index = (ny * w + nx) * 4;
                    sums[0] += u32::from(pixels[index]);
                    sums[1] += u32::from(pixels[index + 1]);
                    sums[2] += u32::from(pixels[index + 2]);
                }
            }
            let index = (y * w + x) * 4;
            out[index] = (sums[0] / 9) as u8;
            out[index + 1] = (sums[1] / 9) as u8;
            out[index + 2] = (sums[2] / 9) as u8;
            out[index + 3] = pixels[index + 3];
        }
    }
    out
}

fn clamp_u8(value: f32) -> u8 {
    (value.clamp(0.0, 255.0) + 0.5) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(pixels: &EditedPixels, x: u32, y: u32) -> [u8; 4] {
        let index = ((y * pixels.width + x) * 4) as usize;
        [
            pixels.rgba[index],
            pixels.rgba[index + 1],
            pixels.rgba[index + 2],
            pixels.rgba[index + 3],
        ]
    }

    /// Benchmark manuale: pipeline completa su un'immagine da 24 MP.
    /// Esecuzione: `cargo test -p uberphoto-image --release -- --ignored --nocapture`.
    #[test]
    #[ignore = "benchmark manuale, non fa parte della suite"]
    fn benchmark_ricetta_24mp() {
        let (width, height) = (6000u32, 4000u32);
        let rgba: Vec<u8> = (0..width as usize * height as usize)
            .flat_map(|i| {
                let x = (i % width as usize) as u8;
                let y = (i / width as usize) as u8;
                [x, y, x.wrapping_add(y), 255]
            })
            .collect();
        let image = LoadedImage {
            rgba,
            width,
            height,
            source_width: width,
            source_height: height,
        };
        let recipe = EditRecipe {
            exposure: 0.4,
            contrast: 18.0,
            saturation: 12.0,
            vibrance: 10.0,
            highlights: -12.0,
            shadows: 15.0,
            sharpness: 25.0,
            ..EditRecipe::default()
        };
        let ripetizioni = 3;
        let inizio = std::time::Instant::now();
        for _ in 0..ripetizioni {
            let out = apply_recipe(&image, &recipe);
            std::hint::black_box(out);
        }
        let media = inizio.elapsed() / ripetizioni;
        println!(
            "24 MP ({}x{}), ricetta completa: {:.1} ms per immagine",
            width,
            height,
            media.as_secs_f64() * 1000.0
        );
    }

    fn riempimento(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
        rgba.iter()
            .cycle()
            .take((width * height * 4) as usize)
            .copied()
            .collect()
    }

    #[test]
    fn la_ricetta_identitaria_non_alterata_i_pixel() {
        let source: Vec<u8> = (0..(16 * 16 * 4))
            .map(|index| (index % 256) as u8)
            .collect();
        let edited = apply_recipe_rgba(&source, 16, 16, &EditRecipe::default());
        assert_eq!(edited.rgba, source);
        assert_eq!((edited.width, edited.height), (16, 16));
    }

    #[test]
    fn l_esposizione_regola_la_luminanza() {
        let source = riempimento(4, 4, [128, 128, 128, 255]);

        let piu_chiara = apply_recipe_rgba(
            &source,
            4,
            4,
            &EditRecipe {
                exposure: 1.0,
                ..EditRecipe::default()
            },
        );
        assert!(pixel(&piu_chiara, 0, 0)[0] > 128);
        assert_eq!(pixel(&piu_chiara, 0, 0)[3], 255);

        let piu_scura = apply_recipe_rgba(
            &source,
            4,
            4,
            &EditRecipe {
                exposure: -1.0,
                ..EditRecipe::default()
            },
        );
        assert!(pixel(&piu_scura, 0, 0)[0] < 128);
    }

    #[test]
    fn la_saturazione_al_minimo_produce_grigi_neutri() {
        let source = riempimento(4, 4, [200, 100, 50, 255]);
        let edited = apply_recipe_rgba(
            &source,
            4,
            4,
            &EditRecipe {
                saturation: -100.0,
                ..EditRecipe::default()
            },
        );
        let [r, g, b, _] = pixel(&edited, 0, 0);
        assert!((i32::from(r) - i32::from(g)).abs() <= 2, "r={r} g={g}");
        assert!((i32::from(g) - i32::from(b)).abs() <= 2, "g={g} b={b}");
    }

    #[test]
    fn la_temperatura_sposta_i_toni_verso_caldo_o_freddo() {
        let source = riempimento(4, 4, [128, 128, 128, 255]);
        let calda = apply_recipe_rgba(
            &source,
            4,
            4,
            &EditRecipe {
                temperature: 100.0,
                ..EditRecipe::default()
            },
        );
        let [r, _, b, _] = pixel(&calda, 0, 0);
        assert!(r > b, "r={r} b={b}");

        let fredda = apply_recipe_rgba(
            &source,
            4,
            4,
            &EditRecipe {
                temperature: -100.0,
                ..EditRecipe::default()
            },
        );
        let [r, _, b, _] = pixel(&fredda, 0, 0);
        assert!(b > r, "r={r} b={b}");
    }

    #[test]
    fn la_gamma_schiarisce_o_scurisce_i_mezzi_toni() {
        let source = riempimento(4, 4, [128, 128, 128, 255]);

        let chiara = apply_recipe_rgba(
            &source,
            4,
            4,
            &EditRecipe {
                gamma: 2.0,
                ..EditRecipe::default()
            },
        );
        assert!(pixel(&chiara, 0, 0)[0] > 128);

        let scura = apply_recipe_rgba(
            &source,
            4,
            4,
            &EditRecipe {
                gamma: 0.5,
                ..EditRecipe::default()
            },
        );
        assert!(pixel(&scura, 0, 0)[0] < 128);

        // Gli estremi restano fissi.
        let estremi: Vec<u8> = vec![0, 0, 0, 255, 255, 255, 255, 255];
        let edited = apply_recipe_rgba(
            &estremi,
            2,
            1,
            &EditRecipe {
                gamma: 2.0,
                ..EditRecipe::default()
            },
        );
        assert_eq!(&edited.rgba[0..4], &[0, 0, 0, 255]);
        assert_eq!(&edited.rgba[4..8], &[255, 255, 255, 255]);
    }

    #[test]
    fn il_ritaglio_riduce_e_seleziona_la_regione() {
        let mut source = Vec::new();
        for y in 0..10u8 {
            for x in 0..10u8 {
                source.extend_from_slice(&[x * 10, y * 10, 0, 255]);
            }
        }
        let edited = apply_recipe_rgba(
            &source,
            10,
            10,
            &EditRecipe {
                crop: Some(CropRect {
                    left: 0.5,
                    top: 0.0,
                    right: 1.0,
                    bottom: 0.5,
                }),
                ..EditRecipe::default()
            },
        );
        assert_eq!((edited.width, edited.height), (5, 5));
        assert_eq!(
            pixel(&edited, 0, 0),
            [50, 0, 0, 255],
            "angolo superiore sinistro della metà destra"
        );
    }

    #[test]
    fn la_rotazione_di_90_gradi_scambia_le_dimensioni() {
        let source: Vec<u8> = vec![
            1, 1, 1, 255, 2, 2, 2, 255, // riga 0
            3, 3, 3, 255, 4, 4, 4, 255, // riga 1
            5, 5, 5, 255, 6, 6, 6, 255, // riga 2
        ];
        let edited = apply_recipe_rgba(
            &source,
            2,
            3,
            &EditRecipe {
                rotation: 90,
                ..EditRecipe::default()
            },
        );
        assert_eq!((edited.width, edited.height), (3, 2));
        assert_eq!(pixel(&edited, 0, 0), [5, 5, 5, 255], "in basso a sinistra");
        assert_eq!(pixel(&edited, 2, 0), [1, 1, 1, 255], "in alto a sinistra");
        assert_eq!(pixel(&edited, 0, 1), [6, 6, 6, 255], "in basso a destra");

        let completata = apply_recipe_rgba(
            &source,
            2,
            3,
            &EditRecipe {
                rotation: 180,
                ..EditRecipe::default()
            },
        );
        assert_eq!((completata.width, completata.height), (2, 3));
        assert_eq!(pixel(&completata, 0, 0), [6, 6, 6, 255]);
    }

    #[test]
    fn la_nitidezza_aumenta_il_contrasto_dei_bordi() {
        let mut source = Vec::new();
        for _y in 0..8 {
            for x in 0..32u8 {
                let value = if x < 16 { 100 } else { 160 };
                source.extend_from_slice(&[value, value, value, 255]);
            }
        }
        let edited = apply_recipe_rgba(
            &source,
            32,
            8,
            &EditRecipe {
                sharpness: 100.0,
                ..EditRecipe::default()
            },
        );
        assert!(pixel(&edited, 15, 4)[0] < 100, "lato scuro più scuro");
        assert!(pixel(&edited, 16, 4)[0] > 160, "lato chiaro più chiaro");
        assert_eq!(pixel(&edited, 0, 4)[0], 100, "le zone piatte restano");
    }

    #[test]
    fn la_ricetta_sopravvive_al_giro_json() {
        let recipe = EditRecipe {
            exposure: 1.5,
            vibrance: -35.0,
            gamma: 1.4,
            sharpness: 60.0,
            rotation: 270,
            crop: Some(CropRect {
                left: 0.1,
                top: 0.05,
                right: 0.9,
                bottom: 0.95,
            }),
            ..EditRecipe::default()
        };
        let restaurata =
            EditRecipe::from_json(&recipe.to_json()).expect("deserializzazione della ricetta");
        assert_eq!(restaurata, recipe);
        assert!(!restaurata.is_identity());
    }

    #[test]
    fn la_sanitizzazione_riporta_i_valori_nei_limiti() {
        let sporca = EditRecipe {
            exposure: 42.0,
            gamma: 0.0,
            sharpness: 400.0,
            rotation: 45,
            crop: Some(CropRect {
                left: 0.9,
                top: -1.0,
                right: 0.2,
                bottom: 2.0,
            }),
            ..EditRecipe::default()
        };
        let pulita = sporca.sanitized();
        assert_eq!(pulita.exposure, 5.0);
        assert_eq!(pulita.gamma, 0.2);
        assert_eq!(pulita.sharpness, 100.0);
        assert_eq!(pulita.rotation, 0);
        let crop = pulita.crop.expect("ritaglio presente");
        assert!(crop.left < crop.right);
        assert!(crop.top < crop.bottom);
        assert!((0.0..=1.0).contains(&crop.right));
    }
}
