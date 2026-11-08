//! Esecuzione delle operazioni filesystem su un thread dedicato.
//!
//! Le operazioni su più file (soprattutto tra volumi diversi) possono
//! durare a lungo: vengono serializzate su un thread di lavoro e i report
//! tornano alla UI attraverso un canale, come per la scansione cartelle.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use crate::ops::{self, ConflictPolicy, FileOpReport, PathMove};

/// Richiesta di operazione inviata dal thread UI.
pub enum FileOpRequest {
    Transfer {
        paths: Vec<PathBuf>,
        target: PathBuf,
        copy: bool,
        policy: ConflictPolicy,
    },
    Rename {
        path: PathBuf,
        new_name: String,
        policy: ConflictPolicy,
    },
    Undo {
        report: FileOpReport,
    },
    RestoreCopy {
        backup: PathBuf,
        original: PathBuf,
    },
}

/// Esito di un'operazione, con l'etichetta per la barra di stato.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOpKind {
    Copy,
    Move,
    Rename,
    Undo,
    RestoreCopy,
}

pub struct FileOpOutcome {
    pub kind: FileOpKind,
    pub description: &'static str,
    pub report: FileOpReport,
}

pub struct FileOpsService {
    tx: Sender<FileOpRequest>,
    rx: Receiver<FileOpOutcome>,
    /// True mentre il worker sta eseguendo (o ha in coda) un'operazione.
    busy: Arc<AtomicUsize>,
}

impl FileOpsService {
    pub fn new(repaint: Arc<dyn Fn() + Send + Sync>) -> Self {
        let (tx, requests) = channel::<FileOpRequest>();
        let (outcomes_tx, rx) = channel::<FileOpOutcome>();
        let busy = Arc::new(AtomicUsize::new(0));
        let worker_busy = Arc::clone(&busy);

        std::thread::Builder::new()
            .name("file-ops".to_owned())
            .spawn(move || {
                while let Ok(request) = requests.recv() {
                    let outcome = execute(request);
                    if outcomes_tx.send(outcome).is_err() {
                        worker_busy.fetch_sub(1, Ordering::SeqCst);
                        break;
                    }
                    worker_busy.fetch_sub(1, Ordering::SeqCst);
                    repaint();
                }
            })
            .expect("avvio del thread file-ops");

        Self { tx, rx, busy }
    }

    pub fn request(&self, request: FileOpRequest) {
        self.busy.fetch_add(1, Ordering::SeqCst);
        if self.tx.send(request).is_err() {
            self.busy.fetch_sub(1, Ordering::SeqCst);
            tracing::error!("servizio operazioni non disponibile");
        }
    }

    /// True se c'è un'operazione in corso: la UI la usa per sopprimere gli
    /// eventi del watcher causati dalle proprie scritture.
    pub fn busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst) > 0
    }

    pub fn try_recv(&self) -> Option<FileOpOutcome> {
        self.rx.try_recv().ok()
    }
}

fn execute(request: FileOpRequest) -> FileOpOutcome {
    match request {
        FileOpRequest::Transfer {
            paths,
            target,
            copy,
            policy,
        } => {
            let report = if copy {
                ops::copy_assets(&paths, &target, policy)
            } else {
                ops::move_assets(&paths, &target, policy)
            };
            FileOpOutcome {
                kind: if copy {
                    FileOpKind::Copy
                } else {
                    FileOpKind::Move
                },
                description: if copy { "Copia" } else { "Spostamento" },
                report,
            }
        }
        FileOpRequest::Rename {
            path,
            new_name,
            policy,
        } => FileOpOutcome {
            kind: FileOpKind::Rename,
            description: "Rinomina",
            report: ops::rename_asset(&path, &new_name, policy),
        },
        FileOpRequest::Undo { report } => FileOpOutcome {
            kind: FileOpKind::Undo,
            description: "Annullamento",
            report: ops::undo(&report),
        },
        FileOpRequest::RestoreCopy { backup, original } => {
            let mut report = FileOpReport::default();
            match crate::recovery::restore_backup(&backup, &original) {
                Ok(to) => report.moved.push(PathMove { from: backup, to }),
                Err(error) => report.failed.push((backup, error.to_string())),
            }
            FileOpOutcome {
                kind: FileOpKind::RestoreCopy,
                description: "Copia",
                report,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{Duration, Instant};

    #[test]
    fn recovery_worker_reports_a_new_copy_without_consuming_the_backup() {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("photo.jpg");
        fs::write(&original, b"original photo").unwrap();
        let backup = crate::backup_file(&original).unwrap();
        fs::write(&original, b"current photo").unwrap();
        let service = FileOpsService::new(Arc::new(|| {}));
        service.request(FileOpRequest::RestoreCopy {
            backup: backup.clone(),
            original: original.clone(),
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        let outcome = loop {
            if let Some(outcome) = service.try_recv() {
                break outcome;
            }
            assert!(Instant::now() < deadline, "recovery worker timed out");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(outcome.kind, FileOpKind::RestoreCopy);
        assert_eq!(outcome.description, "Copia");
        assert!(outcome.report.failed.is_empty());
        assert_eq!(outcome.report.moved.len(), 1);
        assert_eq!(outcome.report.moved[0].from, backup);
        assert_eq!(
            fs::read(&outcome.report.moved[0].to).unwrap(),
            b"original photo"
        );
        assert_eq!(fs::read(&backup).unwrap(), b"original photo");
        assert_eq!(fs::read(&original).unwrap(), b"current photo");
    }
}
