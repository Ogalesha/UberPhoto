//! Rilevamento delle modifiche esterne alla cartella corrente (`notify`).
//!
//! Il watcher segue ricorsivamente la cartella visualizzata, così anche le
//! modifiche nelle sottocartelle aggiornano la vista aggregata. Gli eventi
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

/// Finestra durante la quale gli eventi causati dalle scritture dell'app
/// vengono ignorati (con un margine per gli eventi in ritardo).
const SUPPRESS_GRACE: Duration = Duration::from_millis(800);

struct Inner {
    pending: AtomicBool,
    last_event: Mutex<Instant>,
    suppress_until: Mutex<Option<Instant>>,
}

/// True se gli eventi sono attualmente soppressi.
fn is_suppressed(inner: &Inner) -> bool {
    inner
        .suppress_until
        .lock()
        .ok()
        .and_then(|until| *until)
        .is_some_and(|until| Instant::now() < until)
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
            suppress_until: Mutex::new(None),
        });
        let handler_inner = Arc::clone(&inner);
        let watcher = notify::recommended_watcher(move |event: Result<Event, notify::Error>| {
            let Ok(event) = event else {
                return;
            };
            if !Self::is_significant(&event) || is_suppressed(&handler_inner) {
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
        match self.watcher.watch(dir, RecursiveMode::Recursive) {
            Ok(()) => self.watched = Some(dir.to_path_buf()),
            Err(err) => tracing::warn!(dir = %dir.display(), "osservazione non riuscita: {err}"),
        }
    }

    /// True se esistono eventi non ancora consumati.
    pub fn has_pending(&self) -> bool {
        self.inner.pending.load(Ordering::Relaxed)
    }

    /// Estende la finestra di soppressione degli eventi: da usare mentre
    /// l'applicazione stessa sta scrivendo file nella cartella osservata.
    pub fn set_suppressed(&self, suppressed: bool) {
        if suppressed {
            self.extend_suppression(SUPPRESS_GRACE);
        }
    }

    /// Ignora gli eventi per `duration` a partire da adesso (o fino alla
    /// scadenza già impostata, se più lontana). Usata per le operazioni
    /// sincrone.
    pub fn suppress_for(&self, duration: Duration) {
        self.extend_suppression(duration);
    }

    fn extend_suppression(&self, duration: Duration) {
        let until = Instant::now() + duration;
        if let Ok(mut guard) = self.inner.suppress_until.lock() {
            if guard.is_none_or(|current| until > current) {
                *guard = Some(until);
            }
        }
    }

    /// True una sola volta, quando gli eventi si sono assestati dopo il
    /// periodo di quiete; azzera lo stato in attesa di nuovi eventi.
    pub fn take_settled_change(&self) -> bool {
        // Durante le scritture dell'app non si riscansiona: gli eventi
        // eventualmente già in sospeso restano validi e verranno consumati
        // al termine della finestra di soppressione.
        if is_suppressed(&self.inner) || !self.has_pending() {
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
    fn la_creazione_in_una_sottocartella_viene_rilevata() {
        let temp = tempfile::tempdir().unwrap();
        let sub = temp.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let mut watcher = test_watcher();
        watcher.watch(temp.path());
        std::fs::write(sub.join("nuova.jpg"), b"dati").unwrap();
        assert!(
            attendi_cambiamento(&watcher),
            "una modifica nella sottocartella deve essere rilevata"
        );
    }

    #[test]
    fn la_sola_lettura_non_conta_come_modifica() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("foto.jpg"), b"contenuto").unwrap();
        let mut watcher = test_watcher();
        watcher.watch(temp.path());
        // Drena gli eventuali eventi di assestamento dell'avvio della sorveglianza
        // (su macOS FSEvents può recapitare in ritardo gli eventi della creazione).
        std::thread::sleep(QUIET_PERIOD + Duration::from_millis(300));
        let _ = watcher.take_settled_change();
        let _ = std::fs::read(temp.path().join("foto.jpg")).unwrap();
        let _ = std::fs::metadata(temp.path().join("foto.jpg")).unwrap();
        std::thread::sleep(QUIET_PERIOD + Duration::from_millis(150));
        assert!(
            !watcher.take_settled_change(),
            "la lettura di un file non deve generare cambiamenti"
        );
    }

    #[test]
    fn gli_eventi_durante_la_soppressione_sono_ignorati() {
        let temp = tempfile::tempdir().unwrap();
        let mut watcher = test_watcher();
        watcher.watch(temp.path());
        std::thread::sleep(QUIET_PERIOD + Duration::from_millis(100));
        let _ = watcher.take_settled_change();

        watcher.suppress_for(Duration::from_millis(1_500));
        std::fs::write(temp.path().join("scritta-dall-app.jpg"), b"dati").unwrap();
        std::thread::sleep(QUIET_PERIOD + Duration::from_millis(250));
        assert!(
            !watcher.take_settled_change(),
            "le scritture dell'app non devono causare una riscansione"
        );
    }
}
