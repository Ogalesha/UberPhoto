//! Rilevamento delle modifiche esterne alla cartella corrente (`notify`).
//!
//! Il watcher è non ricorsivo sulla cartella visualizzata; gli eventi
//! vengono calmierati: dopo l'ultimo evento si attende un periodo di quiete
//! prima di dichiarare "cambiamento assestato", per evitare scansioni ripetute
//! multiple durante le operazioni del filesystem.
//!
//! Gli eventi di solo accesso (apertura/chiusura di file) vengono ignorati:
//! su Linux `inotify` li riporta anche quando l'applicazione stessa legge le
//! foto per le miniature o la scansione, generando altrimenti scansioni
//! a catena.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

/// Periodo di quiete dopo l'ultimo evento.
const QUIET_PERIOD: Duration = Duration::from_millis(350);

struct Inner {
    pending: AtomicBool,
    last_event: Mutex<Instant>,
}

pub struct DirWatcher {
    watcher: RecommendedWatcher,
    inner: Arc<Inner>,
    watched: Option<PathBuf>,
}

impl DirWatcher {
    /// True se `event` è una modifica reale e non un semplice accesso.
    fn is_significant(event: &Event) -> bool {
        !matches!(event.kind, EventKind::Access(_))
    }

    pub fn new(repaint: Arc<dyn Fn() + Send + Sync>) -> Option<Self> {
        let inner = Arc::new(Inner {
            pending: AtomicBool::new(false),
            last_event: Mutex::new(Instant::now()),
        });
        let handler_inner = Arc::clone(&inner);
        let watcher = notify::recommended_watcher(move |event: Result<Event, notify::Error>| {
            let Ok(event) = event else {
                return;
            };
            if !Self::is_significant(&event) {
                return;
            }
            handler_inner.pending.store(true, Ordering::Relaxed);
            if let Ok(mut last) = handler_inner.last_event.lock() {
                *last = Instant::now();
            }
            repaint();
        })
        .map_err(|err| {
            tracing::warn!("watcher non disponibile: {err}");
            err
        })
        .ok()?;
        Some(Self {
            watcher,
            inner,
            watched: None,
        })
    }

    /// Mette in osservazione `dir`, sostituendo l'eventuale cartella precedente.
    pub fn watch(&mut self, dir: &Path) {
        if self.watched.as_deref() == Some(dir) {
            return;
        }
        if let Some(old) = self.watched.take() {
            let _ = self.watcher.unwatch(&old);
        }
        match self.watcher.watch(dir, RecursiveMode::NonRecursive) {
            Ok(()) => self.watched = Some(dir.to_path_buf()),
            Err(err) => tracing::warn!(dir = %dir.display(), "osservazione non riuscita: {err}"),
        }
    }

    /// True se esistono eventi non ancora consumati.
    pub fn has_pending(&self) -> bool {
        self.inner.pending.load(Ordering::Relaxed)
    }

    /// True una sola volta, quando gli eventi si sono assestati dopo il
    /// periodo di quiete; azzera lo stato in attesa di nuovi eventi.
    pub fn take_settled_change(&self) -> bool {
        if !self.has_pending() {
            return false;
        }
        let settled = self
            .inner
            .last_event
            .lock()
            .map(|last| last.elapsed() >= QUIET_PERIOD)
            .unwrap_or(true);
        if settled {
            self.inner.pending.store(false, Ordering::Relaxed);
        }
        settled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_watcher() -> DirWatcher {
        DirWatcher::new(Arc::new(|| {})).expect("watcher disponibile")
    }

    /// Attende (in polling) il primo cambiamento assestato, fino a 5 secondi.
    fn attendi_cambiamento(watcher: &DirWatcher) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if watcher.take_settled_change() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    #[test]
    fn la_creazione_di_un_file_viene_rilevata() {
        let temp = tempfile::tempdir().unwrap();
        let mut watcher = test_watcher();
        watcher.watch(temp.path());
        std::fs::write(temp.path().join("nuova.jpg"), b"dati").unwrap();
        assert!(
            attendi_cambiamento(&watcher),
            "la creazione di un file deve produrre un cambiamento assestato"
        );
    }

    #[test]
    fn la_sola_lettura_non_conta_come_modifica() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("foto.jpg"), b"contenuto").unwrap();
        let mut watcher = test_watcher();
        watcher.watch(temp.path());
        let _ = std::fs::read(temp.path().join("foto.jpg")).unwrap();
        let _ = std::fs::metadata(temp.path().join("foto.jpg")).unwrap();
        std::thread::sleep(QUIET_PERIOD + Duration::from_millis(150));
        assert!(
            !watcher.take_settled_change(),
            "la lettura di un file non deve generare cambiamenti"
        );
    }
}
