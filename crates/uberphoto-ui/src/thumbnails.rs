//! Cache delle miniature: texture egui indicizzate per percorso, con
//! generazione in background tramite [`ImageService`].
//!
//! Regole:
//! - una miniatura è "fresca" solo se il suo `mtime` coincide con quello
//!   dell'entry scansionata; se il file cambia, viene rigenerata;
//! - gli errori di decodifica sono memorizzati (niente richieste ripetute
//!   a ogni frame) e mostrati come segnaposto;
//! - la cache è limitata: le voci meno usate recentemente vengono sfrattate.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use eframe::egui;
use uberphoto_image::{ImageService, LoadedImage};

pub enum ThumbState {
    Ready {
        texture: egui::TextureHandle,
        source_size: (u32, u32),
    },
    Failed {
        error: String,
    },
}

struct Entry {
    mtime: Option<SystemTime>,
    state: ThumbState,
}

const MAX_ENTRIES: usize = 2000;

pub struct ThumbCache {
    entries: HashMap<PathBuf, Entry>,
    inflight: HashSet<PathBuf>,
    last_used: HashMap<PathBuf, u64>,
    tick: u64,
}

impl ThumbCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            inflight: HashSet::new(),
            last_used: HashMap::new(),
            tick: 0,
        }
    }

    /// Da chiamare una volta per frame: aggiorna il contatore LRU ed esegue
    /// lo sfratto quando la cache supera il limite.
    pub fn begin_frame(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        if self.entries.len() > MAX_ENTRIES {
            self.evict(MAX_ENTRIES * 3 / 4);
        }
    }

    fn evict(&mut self, keep: usize) {
        let mut by_age: Vec<(PathBuf, u64)> = self
            .last_used
            .iter()
            .map(|(path, tick)| (path.clone(), *tick))
            .collect();
        by_age.sort_by_key(|(_, tick)| *tick);
        for (path, _) in by_age {
            if self.entries.len() <= keep {
                break;
            }
            if self.inflight.contains(&path) {
                continue;
            }
            self.entries.remove(&path);
            self.last_used.remove(&path);
        }
    }

    /// Assicura che esista (o parta) una miniatura per `path`.
    /// `wanted_px` è la dimensione desiderata in pixel fisici.
    pub fn ensure(
        &mut self,
        path: &Path,
        mtime: Option<SystemTime>,
        images: &ImageService,
        wanted_px: f32,
    ) {
        self.last_used.insert(path.to_path_buf(), self.tick);
        if self.inflight.contains(path) {
            return;
        }
        let fresh = self
            .entries
            .get(path)
            .map(|entry| entry.mtime == mtime)
            .unwrap_or(false);
        if fresh {
            return;
        }
        images.request_thumb(path.to_path_buf(), mtime, quantize(wanted_px));
        self.inflight.insert(path.to_path_buf());
    }

    /// Miniatura fresca per `path`, se disponibile.
    pub fn get_fresh(&self, path: &Path, mtime: Option<SystemTime>) -> Option<&ThumbState> {
        self.entries
            .get(path)
            .filter(|entry| entry.mtime == mtime)
            .map(|entry| &entry.state)
    }

    /// Dimensioni originali della foto, quando note dalla miniatura.
    pub fn dims(&self, path: &Path) -> Option<(u32, u32)> {
        match self.entries.get(path).map(|entry| &entry.state) {
            Some(ThumbState::Ready { source_size, .. }) => Some(*source_size),
            _ => None,
        }
    }

    /// Registra il risultato recapitato dal servizio immagini.
    pub fn on_result(
        &mut self,
        ctx: &egui::Context,
        path: PathBuf,
        mtime: Option<SystemTime>,
        outcome: Result<LoadedImage, String>,
    ) {
        self.inflight.remove(&path);
        let state = match outcome {
            Ok(image) => {
                let color = egui::ColorImage::from_rgba_unmultiplied(
                    [image.width as usize, image.height as usize],
                    &image.rgba,
                );
                let texture = ctx.load_texture(
                    format!("thumb:{}", path.display()),
                    color,
                    egui::TextureOptions::LINEAR,
                );
                ThumbState::Ready {
                    texture,
                    source_size: (image.source_width, image.source_height),
                }
            }
            Err(error) => ThumbState::Failed { error },
        };
        self.last_used.insert(path.clone(), self.tick);
        self.entries.insert(path, Entry { mtime, state });
    }

    pub fn stats(&self) -> (usize, usize) {
        (self.entries.len(), self.inflight.len())
    }

    pub fn has_inflight(&self) -> bool {
        !self.inflight.is_empty()
    }
}

/// Arrotonda la dimensione richiesta a bucket prevedibili, così che piccoli
/// spostamenti dello slider non invalidino texture già caricate.
fn quantize(px: f32) -> u32 {
    for bucket in [64.0_f32, 128.0, 256.0, 512.0, 1024.0] {
        if px <= bucket {
            return bucket as u32;
        }
    }
    1024
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantizza_a_bucket_stabili() {
        assert_eq!(quantize(40.0), 64);
        assert_eq!(quantize(64.0), 64);
        assert_eq!(quantize(65.0), 128);
        assert_eq!(quantize(320.0), 512);
        assert_eq!(quantize(2000.0), 1024);
    }
}
