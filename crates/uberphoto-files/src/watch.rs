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
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

/// Periodo di quiete dopo l'ultimo evento.
const QUIET_PERIOD: Duration = Duration::from_millis(350);

/// Finestra durante la quale gli eventi vengono differiti (con un margine per
/// gli eventi in ritardo). Anche modifiche esterne concomitanti restano visibili.
const SUPPRESS_GRACE: Duration = Duration::from_millis(800);

struct Inner {
    state: Mutex<WatcherState>,
}

enum ChangeState {
    Idle,
    Pending { last_event: Instant },
}

struct WatcherState {
    change: ChangeState,
    suppress_until: Option<Instant>,
}

impl WatcherState {
    fn is_suppressed(&self, now: Instant) -> bool {
        self.suppress_until.is_some_and(|until| now < until)
    }

    fn record_event(&mut self, now: Instant) -> bool {
        self.change = ChangeState::Pending { last_event: now };
        true
    }

    fn take_settled(&mut self, now: Instant) -> bool {
        if self.is_suppressed(now) {
            return false;
        }
        match self.change {
            ChangeState::Pending { last_event }
                if now.duration_since(last_event) >= QUIET_PERIOD =>
            {
                self.change = ChangeState::Idle;
                true
            }
            _ => false,
        }
    }
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
            state: Mutex::new(WatcherState {
                change: ChangeState::Idle,
                suppress_until: None,
            }),
        });
        let handler_inner = Arc::clone(&inner);
        let watcher = notify::recommended_watcher(move |event: Result<Event, notify::Error>| {
            let Ok(event) = event else {
                return;
            };
            if !Self::is_significant(&event) {
                return;
            }
            if handler_inner
                .state
                .lock()
                .is_ok_and(|mut state| state.record_event(Instant::now()))
            {
                repaint();
            }
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
        self.inner
            .state
            .lock()
            .is_ok_and(|state| matches!(state.change, ChangeState::Pending { .. }))
    }

    /// Estende la finestra di soppressione degli eventi: da usare mentre
    /// l'applicazione stessa sta scrivendo file nella cartella osservata.
    pub fn set_suppressed(&self, suppressed: bool) {
        if suppressed {
            self.extend_suppression(SUPPRESS_GRACE);
        }
    }

    /// Differisce gli eventi per `duration` a partire da adesso (o fino alla
    /// scadenza già impostata, se più lontana). Usata per le operazioni
    /// sincrone.
    pub fn suppress_for(&self, duration: Duration) {
        self.extend_suppression(duration);
    }

    fn extend_suppression(&self, duration: Duration) {
        let until = Instant::now() + duration;
        if let Ok(mut state) = self.inner.state.lock() {
            if state.suppress_until.is_none_or(|current| until > current) {
                state.suppress_until = Some(until);
            }
        }
    }

    /// True una sola volta, quando gli eventi si sono assestati dopo il
    /// periodo di quiete; azzera lo stato in attesa di nuovi eventi.
    pub fn take_settled_change(&self) -> bool {
        // Durante le scritture dell'app non si riscansiona: gli eventi
        // eventualmente già in sospeso restano validi e verranno consumati
        // al termine della finestra di soppressione.
        self.inner
            .state
            .lock()
            .is_ok_and(|mut state| state.take_settled(Instant::now()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_event_cannot_be_consumed_as_an_older_settled_change() {
        let start = Instant::now();
        let mut state = WatcherState {
            change: ChangeState::Idle,
            suppress_until: None,
        };
        assert!(state.record_event(start));
        let settled = start + QUIET_PERIOD;
        assert!(state.record_event(settled));
        assert!(!state.take_settled(settled));
        assert!(state.take_settled(settled + QUIET_PERIOD));
        assert!(!state.take_settled(settled + QUIET_PERIOD));
    }

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
    fn gli_eventi_durante_la_soppressione_vengono_recuperati_dopo() {
        let temp = tempfile::tempdir().unwrap();
        let mut watcher = test_watcher();
        watcher.watch(temp.path());
        std::thread::sleep(QUIET_PERIOD + Duration::from_millis(100));
        let _ = watcher.take_settled_change();

        watcher.suppress_for(Duration::from_millis(800));
        std::fs::write(temp.path().join("scritta-dall-app.jpg"), b"dati").unwrap();
        std::thread::sleep(QUIET_PERIOD + Duration::from_millis(250));
        assert!(
            !watcher.take_settled_change(),
            "la scansione deve attendere la fine della soppressione"
        );
        std::thread::sleep(Duration::from_millis(400));
        assert!(watcher.take_settled_change());
    }
}
