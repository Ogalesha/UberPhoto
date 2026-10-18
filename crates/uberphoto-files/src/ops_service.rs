//! Esecuzione delle operazioni filesystem su un thread dedicato.
//!
//! Le operazioni su più file (soprattutto tra volumi diversi) possono
//! durare a lungo: vengono serializzate su un thread di lavoro e i report
//! tornano alla UI attraverso un canale, come per la scansione cartelle.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use crate::ops::{self, ConflictPolicy, FileOpReport};

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
}

/// Esito di un'operazione, con l'etichetta per la barra di stato.
pub struct FileOpOutcome {
    pub description: &'static str,
    pub report: FileOpReport,
}

pub struct FileOpsService {
    tx: Sender<FileOpRequest>,
    rx: Receiver<FileOpOutcome>,
}

impl FileOpsService {
    pub fn new(repaint: Arc<dyn Fn() + Send + Sync>) -> Self {
        let (tx, requests) = channel::<FileOpRequest>();
        let (outcomes_tx, rx) = channel::<FileOpOutcome>();

        std::thread::Builder::new()
            .name("file-ops".to_owned())
            .spawn(move || {
                while let Ok(request) = requests.recv() {
                    let outcome = execute(request);
                    if outcomes_tx.send(outcome).is_err() {
                        break;
                    }
                    repaint();
                }
            })
            .expect("avvio del thread file-ops");

        Self { tx, rx }
    }

    pub fn request(&self, request: FileOpRequest) {
        if self.tx.send(request).is_err() {
            tracing::error!("servizio operazioni non disponibile");
        }
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
                description: if copy { "Copia" } else { "Spostamento" },
                report,
            }
        }
        FileOpRequest::Rename {
            path,
            new_name,
            policy,
        } => FileOpOutcome {
            description: "Rinomina",
            report: ops::rename_asset(&path, &new_name, policy),
        },
        FileOpRequest::Undo { report } => FileOpOutcome {
            description: "Annullamento",
            report: ops::undo(&report),
        },
    }
}
