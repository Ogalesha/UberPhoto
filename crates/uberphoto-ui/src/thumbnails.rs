//! Cache delle miniature: texture egui indicizzate per percorso, con
//! generazione in background tramite [`ImageService`].
//!
//! Regole:
//! - una miniatura è "fresca" solo se il suo `mtime` coincide con quello
//!   dell'entry scansionata; se il file cambia, viene rigenerata;
//! - gli errori di decodifica sono memorizzati (niente richieste ripetute
//!   a ogni frame) e mostrati come segnaposto;
//! - la cache è limitata sia per numero di voci sia per byte di texture:
//!   le voci meno usate recentemente vengono sfrattate;
//! - i risultati di un'epoca precedente (cartella cambiata) vengono ignorati.

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
    /// Pixel RGBA della texture, per il budget di memoria.
    bytes: usize,
}

const MAX_ENTRIES: usize = 2000;
/// Limite di VRAM per le miniature. Con 2.000 texture da 256 px erano già
/// ~500 MB; a 512/1024 px si potevano superare diversi GB.
const MAX_BYTES: usize = 512 * 1024 * 1024;

pub struct ThumbCache {
    entries: HashMap<PathBuf, Entry>,
    inflight: HashSet<PathBuf>,
    last_used: HashMap<PathBuf, u64>,
    tick: u64,
    total_bytes: usize,
    epoch: u64,
}

impl ThumbCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            inflight: HashSet::new(),
            last_used: HashMap::new(),
            tick: 0,
            total_bytes: 0,
            epoch: 0,
        }
    }

    /// Epoca corrente delle richieste di miniature.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Passa a una nuova epoca: le richieste della cartella precedente non
    /// sono più attese e i loro risultati verranno scartati.
    pub fn set_epoch(&mut self, epoch: u64) {
        if self.epoch != epoch {
            self.epoch = epoch;
            self.inflight.clear();
            // Le voci ancora in cache restano valide (stesso mtime); i
            // riferimenti LRU di richieste mai completate, invece, non
            // servono più e crescerebbero senza limite cambiando cartella.
            self.last_used
                .retain(|path, _| self.entries.contains_key(path));
        }
    }

    /// Da chiamare una volta per frame: aggiorna il contatore LRU ed esegue
    /// lo sfratto quando la cache supera i limiti.
    pub fn begin_frame(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        if self.entries.len() > MAX_ENTRIES || self.total_bytes > MAX_BYTES {
            self.evict();
        }
    }

    fn evict(&mut self) {
        let keep_entries = MAX_ENTRIES * 3 / 4;
        let keep_bytes = MAX_BYTES * 3 / 4;
        let mut by_age: Vec<(PathBuf, u64)> = self
            .last_used
            .iter()
            .map(|(path, tick)| (path.clone(), *tick))
            .collect();
        by_age.sort_by_key(|(_, tick)| *tick);
        for (path, _) in by_age {
            if self.entries.len() <= keep_entries && self.total_bytes <= keep_bytes {
                break;
            }
            if self.inflight.contains(&path) {
                continue;
            }
            if let Some(entry) = self.entries.remove(&path) {
                self.total_bytes = self.total_bytes.saturating_sub(entry.bytes);
            }
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
        if let Some(tick) = self.last_used.get_mut(path) {
            *tick = self.tick;
        } else {
            self.last_used.insert(path.to_path_buf(), self.tick);
        }
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
        images.request_thumb(path.to_path_buf(), mtime, quantize(wanted_px), self.epoch);
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
    /// I risultati di un'epoca superata vengono ignorati.
    pub fn on_result(
        &mut self,
        ctx: &egui::Context,
        path: PathBuf,
        mtime: Option<SystemTime>,
        epoch: u64,
        outcome: Result<LoadedImage, String>,
    ) {
        if epoch != self.epoch {
            return;
        }
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
        let bytes = match &state {
            ThumbState::Ready { texture, .. } => {
                let [width, height] = texture.size();
                width.saturating_mul(height).saturating_mul(4)
            }
            ThumbState::Failed { .. } => 0,
        };
        self.last_used.insert(path.clone(), self.tick);
        if let Some(old) = self.entries.insert(
            path,
            Entry {
                mtime,
                state,
                bytes,
            },
        ) {
            self.total_bytes = self.total_bytes.saturating_sub(old.bytes);
        }
        self.total_bytes = self.total_bytes.saturating_add(bytes);
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

    #[test]
    fn il_cambio_epoca_scarta_le_richieste_in_volo() {
        let mut cache = ThumbCache::new();
        cache.inflight.insert(PathBuf::from("/vecchia/a.jpg"));
        cache.set_epoch(7);
        assert_eq!(cache.epoch(), 7);
        assert!(!cache.has_inflight());

        // Un risultato della vecchia epoca non entra in cache.
        let ctx = egui::Context::default();
        cache.on_result(
            &ctx,
            PathBuf::from("/vecchia/a.jpg"),
            None,
            6,
            Err("superata".to_owned()),
        );
        assert!(cache.get_fresh(Path::new("/vecchia/a.jpg"), None).is_none());
    }
}
