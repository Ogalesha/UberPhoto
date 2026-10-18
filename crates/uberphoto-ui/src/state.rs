use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use eframe::egui;
use serde::{Deserialize, Serialize};
use uberphoto_core::PhotoAsset;
use uberphoto_files::{sort_assets, DirNode, SortKey};

/// Schede del pannello destro.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InspectorTab {
    Colori,
    Informazioni,
    Metadati,
    Gps,
    Esportazione,
}

impl InspectorTab {
    pub const ALL: [InspectorTab; 5] = [
        InspectorTab::Colori,
        InspectorTab::Informazioni,
        InspectorTab::Metadati,
        InspectorTab::Gps,
        InspectorTab::Esportazione,
    ];

    pub fn label(self) -> &'static str {
        match self {
            InspectorTab::Colori => "Colori",
            InspectorTab::Informazioni => "Informazioni",
            InspectorTab::Metadati => "Metadati",
            InspectorTab::Gps => "Posizione GPS",
            InspectorTab::Esportazione => "Esportazione",
        }
    }
}

pub const MIN_THUMB_SIZE: f32 = 96.0;
pub const MAX_THUMB_SIZE: f32 = 320.0;

/// Stato della UI che sopravvive al riavvio (persistito da eframe in RON).
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PersistedState {
    pub recent_folders: Vec<PathBuf>,
    pub last_dir: Option<PathBuf>,
    pub thumb_size: Option<f32>,
}

/// Stato applicativo della UI. Il filesystem resta l'autorità: qui vivono
/// soltanto le viste correnti (cartella aperta, elenco scansionato, selezione).
pub struct AppState {
    pub root: Option<DirNode>,
    pub current_dir: Option<PathBuf>,
    pub entries: Vec<PhotoAsset>,
    pub selection: BTreeSet<PathBuf>,
    pub selection_anchor: Option<PathBuf>,
    pub sort_key: SortKey,
    pub sort_ascending: bool,
    pub filter_text: String,
    pub thumb_size: f32,
    pub recent_folders: Vec<PathBuf>,
    pub inspector_tab: InspectorTab,
    pub loading_dir: Option<PathBuf>,
    pub error: Option<String>,
    /// Logo dell'app, caricato una sola volta come texture della UI.
    pub logo: Option<egui::TextureHandle>,
}

impl AppState {
    pub fn new(persisted: PersistedState) -> Self {
        Self {
            root: None,
            current_dir: None,
            entries: Vec::new(),
            selection: BTreeSet::new(),
            selection_anchor: None,
            sort_key: SortKey::Name,
            sort_ascending: true,
            filter_text: String::new(),
            thumb_size: persisted
                .thumb_size
                .unwrap_or(160.0)
                .clamp(MIN_THUMB_SIZE, MAX_THUMB_SIZE),
            recent_folders: persisted.recent_folders,
            inspector_tab: InspectorTab::Informazioni,
            loading_dir: None,
            error: None,
            logo: None,
        }
    }

    pub fn persisted(&self) -> PersistedState {
        PersistedState {
            recent_folders: self.recent_folders.clone(),
            last_dir: self.current_dir.clone(),
            thumb_size: Some(self.thumb_size),
        }
    }

    /// Inserisce `dir` in testa ai recenti (deduplicando, massimo 10 voci).
    pub fn push_recent(&mut self, dir: &Path) {
        self.recent_folders.retain(|existing| existing != dir);
        self.recent_folders.insert(0, dir.to_path_buf());
        self.recent_folders.truncate(10);
    }

    pub fn selected_assets(&self) -> Vec<&PhotoAsset> {
        self.entries
            .iter()
            .filter(|entry| self.selection.contains(&entry.path))
            .collect()
    }

    pub fn entry_mtime(&self, path: &Path) -> Option<SystemTime> {
        self.entries
            .iter()
            .find(|entry| entry.path == path)
            .and_then(|entry| entry.modified)
    }

    pub fn apply_sort(&mut self) {
        sort_assets(&mut self.entries, self.sort_key, self.sort_ascending);
    }
}

/// Azioni raccolte durante il disegno dei pannelli ed eseguite a fine frame.
/// Evita di mutare lo stato nel mezzo del rendering.
#[derive(Default)]
pub struct UiActions {
    /// Apri `dir` come nuova radice dell'esploratore.
    pub open_folder: Option<PathBuf>,
    /// Naviga nel browser su `dir` senza cambiare la radice.
    pub browse_dir: Option<PathBuf>,
    /// Riscansiona la cartella corrente.
    pub refresh: bool,
    /// Apri la finestra di anteprima grande su questa foto.
    pub open_preview: Option<PathBuf>,
    /// Mostra la finestra di dialogo nativa per scegliere una cartella.
    pub pick_folder: bool,
}
