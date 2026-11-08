//! Gestione del filesystem per UberPhoto.
//!
//! Scansione asincrona delle cartelle, aggregazione ricorsiva del sottoalbero,
//! ordinamento naturale e listing delle sotto-cartelle per la navigazione.
//!
//! Fase 2: spostamenti sicuri, gestione conflitti, sidecar, undo,
//! osservazione delle modifiche esterne, esecuzione su thread dedicato.

mod naming;
mod ops;
mod ops_service;
mod recovery;
mod safety;
mod scan;
mod sort;
mod tree;
mod watch;

pub use naming::{is_hidden_name, natural_cmp};
pub use ops::{
    copy_assets, create_dir, delete_empty_dir, dir_item_count, move_assets, rename_asset,
    rename_dir, undo, ConflictPolicy, FileOpReport, PathMove,
};
pub use ops_service::{FileOpKind, FileOpOutcome, FileOpRequest, FileOpsService};
#[doc(inline)]
pub use recovery::{list_backups, restore_backup, BackupEntry};
pub use safety::{backup_file, files_equal, writable_regular_file, StagedFile};
pub use scan::{scan_directory, scan_tree, DirectoryScanner, ScanResult};
pub use sort::{sort_assets, SortKey};
pub use tree::{load_subdirs, DirNode};
pub use watch::DirWatcher;
