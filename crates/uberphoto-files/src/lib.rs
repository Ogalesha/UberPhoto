//! Gestione del filesystem per UberPhoto.
//!
//! Fase 1: scansione asincrona delle cartelle, ordinamento naturale,
//! listing delle sotto-cartelle per l'albero di navigazione.
//!
//! Fase 2: spostamenti sicuri, gestione conflitti, sidecar, undo.

mod naming;
mod scan;
mod sort;
mod tree;

pub use naming::{is_hidden_name, natural_cmp};
pub use scan::{scan_directory, DirectoryScanner, ScanResult};
pub use sort::{sort_assets, SortKey};
pub use tree::{load_subdirs, DirNode};
