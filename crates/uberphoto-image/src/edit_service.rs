//! Servizio di elaborazione dell'anteprima modificata.
//!
//! Un thread dedicato applica le ricette di editing al buffer decodificato.
//! La politica è "latest wins": mentre il worker elabora, le richieste più
//! recenti sostituiscono quelle in attesa, così trascinare uno slider non
//! accumula arretrati. Le generazioni numerano le richieste e la UI applica
//! solo i risultati non superati.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::SystemTime;

use crate::decode::LoadedImage;
use crate::edit::{apply_recipe, EditRecipe, EditedPixels};

/// Immagine sorgente condivisa tra UI e worker di elaborazione.
pub struct EditSource {
    pub path: PathBuf,
    pub mtime: Option<SystemTime>,
    pub image: LoadedImage,
}

struct Job {
    generation: u64,
    source: Arc<EditSource>,
    recipe: EditRecipe,
}

struct Slot {
    job: Mutex<Option<Job>>,
    ready: Condvar,
}

/// Risultato dell'elaborazione di una ricetta.
pub struct EditResult {
    pub generation: u64,
    pub path: PathBuf,
    pub mtime: Option<SystemTime>,
    /// Pixel elaborati, condivisi (la copia tra thread resta economica).
    pub pixels: Arc<EditedPixels>,
}

/// Worker di elaborazione anteprime modificate.
pub struct EditService {
    slot: Arc<Slot>,
    results_rx: Receiver<EditResult>,
    busy: Arc<AtomicBool>,
}

impl EditService {
    /// Avvia il thread di elaborazione; `repaint` risveglia la UI.
    pub fn new(repaint: Arc<dyn Fn() + Send + Sync>) -> Self {
        let slot = Arc::new(Slot {
            job: Mutex::new(None),
            ready: Condvar::new(),
        });
        let (results_tx, results_rx) = channel::<EditResult>();
        let busy = Arc::new(AtomicBool::new(false));

        {
            let slot = slot.clone();
            let busy = busy.clone();
            let results_tx: Sender<EditResult> = results_tx.clone();
            std::thread::Builder::new()
                .name("uberphoto-edit".into())
                .spawn(move || {
                    // Ultima elaborazione riuscita: consente di riusare il
                    // risultato quando si torna a una ricetta precedente
                    // (undo/redo e confronto tra preset).
                    let mut cache: Option<(Arc<EditSource>, EditRecipe, Arc<EditedPixels>)> = None;
                    loop {
                        let job = {
                            let mut guard = slot.job.lock().expect("coda editing avvelenata");
                            guard = slot
                                .ready
                                .wait_while(guard, |job| job.is_none())
                                .expect("coda editing avvelenata");
                            guard.take().expect("job presente dopo l'attesa")
                        };
                        busy.store(true, Ordering::Relaxed);
                        let riusabile = cache.as_ref().is_some_and(|(source, recipe, _)| {
                            Arc::ptr_eq(source, &job.source) && recipe == &job.recipe
                        });
                        let pixels = if riusabile {
                            cache
                                .as_ref()
                                .map(|(_, _, pixels)| pixels.clone())
                                .expect("cache")
                        } else {
                            let pixels = Arc::new(apply_recipe(&job.source.image, &job.recipe));
                            cache = Some((job.source.clone(), job.recipe, pixels.clone()));
                            pixels
                        };
                        busy.store(false, Ordering::Relaxed);
                        let sent = results_tx.send(EditResult {
                            generation: job.generation,
                            path: job.source.path.clone(),
                            mtime: job.source.mtime,
                            pixels,
                        });
                        if sent.is_err() {
                            break;
                        }
                        repaint();
                    }
                })
                .expect("impossibile avviare il worker di editing");
        }

        Self {
            slot,
            results_rx,
            busy,
        }
    }

    /// Accoda (sostituendo) una richiesta di elaborazione. Non bloccante.
    pub fn request(&self, generation: u64, source: Arc<EditSource>, recipe: EditRecipe) {
        {
            let mut guard = self.slot.job.lock().expect("coda editing avvelenata");
            *guard = Some(Job {
                generation,
                source,
                recipe,
            });
        }
        self.slot.ready.notify_one();
    }

    /// Vero se il worker sta elaborando in questo momento.
    pub fn busy(&self) -> bool {
        self.busy.load(Ordering::Relaxed)
    }

    /// Risultati completati disponibili (non bloccante).
    pub fn drain(&self) -> Vec<EditResult> {
        self.results_rx.try_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::apply_recipe_rgba;
    use std::time::Duration;

    fn source(pixels: Vec<u8>, width: u32, height: u32) -> Arc<EditSource> {
        Arc::new(EditSource {
            path: PathBuf::from("/foto/prova.jpg"),
            mtime: None,
            image: LoadedImage {
                rgba: pixels,
                width,
                height,
                source_width: width,
                source_height: height,
            },
        })
    }

    #[test]
    fn recapita_il_risultato_della_ricetta() {
        let service = EditService::new(Arc::new(|| {}));
        let source = source([128, 128, 128, 255].repeat(16), 4, 4);
        let recipe = EditRecipe {
            exposure: 1.0,
            ..EditRecipe::default()
        };
        service.request(1, source.clone(), recipe);

        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut result = None;
        while std::time::Instant::now() < deadline && result.is_none() {
            result = service.drain().into_iter().next();
            if result.is_none() {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        let result = result.expect("nessun risultato entro il timeout");
        assert_eq!(result.generation, 1);
        assert_eq!(result.path, PathBuf::from("/foto/prova.jpg"));
        let atteso = apply_recipe_rgba(&source.image.rgba, 4, 4, &recipe);
        assert_eq!(result.pixels.rgba, atteso.rgba);
        assert!(result.pixels.rgba[0] > 128);
    }

    #[test]
    fn con_richieste_multiple_vince_l_ultima() {
        let service = EditService::new(Arc::new(|| {}));
        let source = source([100, 100, 100, 255].repeat(4), 2, 2);
        for generation in 1..=3u64 {
            let recipe = EditRecipe {
                brightness: (generation as f32) * 10.0,
                ..EditRecipe::default()
            };
            service.request(generation, source.clone(), recipe);
        }

        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut ultima = 0u64;
        while std::time::Instant::now() < deadline {
            for result in service.drain() {
                ultima = ultima.max(result.generation);
            }
            if ultima == 3 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(ultima, 3, "il risultato della generazione più recente");
    }
}
