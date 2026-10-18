//! Servizio di caricamento immagini in background.
//!
//! Due code distinte:
//! - **miniature**: pool rayon multi-thread (l'utente ne vede centinaia);
//! - **anteprime**: thread singolo dedicato, che scarta le richieste superate
//!   ("latest wins"): cliccando velocemente tra le foto viene decodificata
//!   solo l'ultima richiesta in coda.
//!
//! I risultati arrivano alla UI tramite un canale e vengono raccolti una volta
//! per frame; il callback `repaint` risveglia la UI quando un job termina.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::SystemTime;

use crate::decode::{load_image_rgba, LoadedImage};

/// Dimensione massima (lato lungo, in pixel) di un'anteprima.
pub const PREVIEW_MAX_DIM: u32 = 2560;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    Thumbnail,
    Preview,
}

pub struct ImageResult {
    pub path: PathBuf,
    pub mtime: Option<SystemTime>,
    pub kind: ImageKind,
    pub outcome: Result<LoadedImage, String>,
}

struct Job {
    path: PathBuf,
    mtime: Option<SystemTime>,
    max_dim: u32,
}

pub struct ImageService {
    thumb_pool: rayon::ThreadPool,
    preview_tx: Sender<Job>,
    results_tx: Sender<ImageResult>,
    results_rx: Receiver<ImageResult>,
    repaint: Arc<dyn Fn() + Send + Sync>,
}

impl ImageService {
    /// `repaint` è tipicamente `egui::Context::request_repaint`.
    pub fn new(repaint: Arc<dyn Fn() + Send + Sync>) -> Self {
        let (preview_tx, preview_rx) = channel::<Job>();
        let (results_tx, results_rx) = channel::<ImageResult>();

        let parallelism = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let thumb_threads = parallelism.saturating_sub(1).clamp(1, 8);
        let thumb_pool = rayon::ThreadPoolBuilder::new()
            .num_threads(thumb_threads)
            .thread_name(|index| format!("uberphoto-thumb-{index}"))
            .build()
            .expect("impossibile creare il pool di decodifica miniature");

        // Worker anteprime: elabora la prima richiesta, poi salta tutte le
        // richieste accodate nel frattempo tranne l'ultima.
        {
            let results_tx = results_tx.clone();
            let repaint = repaint.clone();
            std::thread::Builder::new()
                .name("uberphoto-preview".into())
                .spawn(move || {
                    while let Ok(first) = preview_rx.recv() {
                        let mut job = first;
                        while let Ok(newer) = preview_rx.try_recv() {
                            job = newer;
                        }
                        let outcome = load_image_rgba(&job.path, Some(job.max_dim));
                        if results_tx
                            .send(ImageResult {
                                path: job.path,
                                mtime: job.mtime,
                                kind: ImageKind::Preview,
                                outcome,
                            })
                            .is_err()
                        {
                            break;
                        }
                        repaint();
                    }
                })
                .expect("impossibile avviare il worker delle anteprime");
        }

        Self {
            thumb_pool,
            preview_tx,
            results_tx,
            results_rx,
            repaint,
        }
    }

    /// Richiede una miniatura (lato lungo `max_dim` pixel). Non bloccante.
    pub fn request_thumb(&self, path: PathBuf, mtime: Option<SystemTime>, max_dim: u32) {
        let results_tx = self.results_tx.clone();
        let repaint = self.repaint.clone();
        self.thumb_pool.spawn(move || {
            let outcome = load_image_rgba(&path, Some(max_dim));
            if let Err(message) = &outcome {
                tracing::debug!(path = %path.display(), "miniatura non disponibile: {message}");
            }
            let _ = results_tx.send(ImageResult {
                path,
                mtime,
                kind: ImageKind::Thumbnail,
                outcome,
            });
            repaint();
        });
    }

    /// Richiede un'anteprima grande (fino a [`PREVIEW_MAX_DIM`]). Non bloccante.
    pub fn request_preview(&self, path: PathBuf, mtime: Option<SystemTime>) {
        let _ = self.preview_tx.send(Job {
            path,
            mtime,
            max_dim: PREVIEW_MAX_DIM,
        });
    }

    /// Risultati completati disponibili (non bloccante).
    pub fn drain(&self) -> Vec<ImageResult> {
        self.results_rx.try_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn recapita_una_miniatura_decodificata() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("thumb.png");
        image::RgbaImage::from_fn(64, 32, |_x, _y| image::Rgba([10, 20, 30, 255]))
            .save(&path)
            .expect("salvataggio");

        let service = ImageService::new(Arc::new(|| {}));
        service.request_thumb(path.clone(), None, 32);

        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut result = None;
        while std::time::Instant::now() < deadline && result.is_none() {
            result = service.drain().into_iter().next();
            if result.is_none() {
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        let result = result.expect("nessun risultato entro il timeout");
        assert_eq!(result.path, path);
        assert_eq!(result.kind, ImageKind::Thumbnail);
        let image = result.outcome.expect("decodifica riuscita");
        assert_eq!((image.width, image.height), (32, 16));
        assert_eq!((image.source_width, image.source_height), (64, 32));
    }
}
