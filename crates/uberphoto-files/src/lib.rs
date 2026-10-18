//! Gestione del filesystem per UberPhoto.
//!
//! Fase 1: scansione asincrona delle cartelle, ordinamento naturale,
//! listing delle sotto-cartelle per l'albero di navigazione.
//!
//! Fase 2: spostamenti sicuri, gestione conflitti, sidecar, undo,
//! osservazione delle modifiche esterne, esecuzione su thread dedicato.

mod naming;
mod ops;
mod ops_service;
mod scan;
mod sort;
mod tree;
mod watch;

pub use naming::{is_hidden_name, natural_cmp};
pub use ops::{
    copy_assets, create_dir, delete_empty_dir, dir_item_count, move_assets, rename_asset,
    rename_dir, undo, ConflictPolicy, FileOpReport, PathMove,
};
pub use ops_service::{FileOpOutcome, FileOpRequest, FileOpsService};
pub use scan::{scan_directory, DirectoryScanner, ScanResult};
pub use sort::{sort_assets, SortKey};
pub use tree::{load_subdirs, DirNode};
pub use watch::DirWatcher;
