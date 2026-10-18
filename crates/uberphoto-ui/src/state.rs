use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use eframe::egui;
use serde::{Deserialize, Serialize};
use uberphoto_core::{FormatKind, PhotoAsset};
use uberphoto_db::IndexRow;
use uberphoto_files::{sort_assets, ConflictPolicy, DirNode, FileOpReport, SortKey};
use uberphoto_metadata::{MetadataDocument, MetadataField, RawTag};

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

/// Filtro per formato nella griglia.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatFilter {
    All,
    /// Tutti i RAW fotografici (DNG, CR2, CR3, NEF, ARW, RAF).
    Raw,
    Kind(FormatKind),
}

impl FormatFilter {
    pub const OPTIONS: [FormatFilter; 10] = [
        FormatFilter::All,
        FormatFilter::Kind(FormatKind::Jpeg),
        FormatFilter::Kind(FormatKind::Png),
        FormatFilter::Kind(FormatKind::Tiff),
        FormatFilter::Kind(FormatKind::Webp),
        FormatFilter::Kind(FormatKind::Gif),
        FormatFilter::Kind(FormatKind::Bmp),
        FormatFilter::Kind(FormatKind::Heic),
        FormatFilter::Kind(FormatKind::Avif),
        FormatFilter::Raw,
    ];

    pub fn label(self) -> &'static str {
        match self {
            FormatFilter::All => "Tutti i formati",
            FormatFilter::Raw => "RAW",
            FormatFilter::Kind(kind) => kind.label(),
        }
    }

    pub fn matches(self, format: FormatKind) -> bool {
        match self {
            FormatFilter::All => true,
            FormatFilter::Raw => format.is_raw(),
            FormatFilter::Kind(kind) => kind == format,
        }
    }
}

/// Stato della UI che sopravvive al riavvio (persistito da eframe in RON).
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PersistedState {
    pub recent_folders: Vec<PathBuf>,
    pub last_dir: Option<PathBuf>,
    pub thumb_size: Option<f32>,
}

/// Oggetto di una rinomina in corso.
#[derive(Debug, Clone)]
pub enum RenameTarget {
    Photo(PathBuf),
    Folder(PathBuf),
}

impl RenameTarget {
    pub fn path(&self) -> &Path {
        match self {
            RenameTarget::Photo(path) | RenameTarget::Folder(path) => path,
        }
    }
}

/// Conferma di spostamento/copia con scelta della politica sui conflitti.
pub struct ConflictPrompt {
    pub paths: Vec<PathBuf>,
    pub target: PathBuf,
    pub copy: bool,
    pub policy: ConflictPolicy,
}

/// Rinomina di una foto o di una cartella.
pub struct RenamePrompt {
    pub target: RenameTarget,
    pub new_name: String,
    pub first_frame: bool,
}

/// Creazione di una sottocartella.
pub struct NewFolderPrompt {
    pub parent: PathBuf,
    pub name: String,
    pub first_frame: bool,
}

/// Conferma di eliminazione di una cartella (solo se vuota).
pub struct DeleteDirPrompt {
    pub dir: PathBuf,
    pub items: usize,
}

/// Esito dell'ultima operazione sul filesystem, mostrato in barra di stato.
pub struct OpStatus {
    pub message: String,
    pub error: Option<String>,
    /// Report dell'ultimo spostamento, per l'annullamento.
    pub undo: Option<FileOpReport>,
}

/// Payload di trascinamento delle foto (griglia → cartelle).
#[derive(Debug, Clone)]
pub struct DragPhotos {
    pub paths: Vec<PathBuf>,
}

/// Rilascio di foto su una cartella.
#[derive(Debug, Clone)]
pub struct DropRequest {
    pub paths: Vec<PathBuf>,
    pub target: PathBuf,
    pub copy: bool,
}

/// Copia/sposta della selezione verso una cartella scelta con dialogo nativo.
#[derive(Debug, Clone)]
pub struct TransferRequest {
    pub paths: Vec<PathBuf>,
    pub copy: bool,
}

/// Stato dell'editor metadati: documenti letti, form e cache dei riassunti.
#[derive(Default)]
pub struct MetaState {
    /// Documenti completi delle foto selezionate.
    pub docs: HashMap<PathBuf, MetadataDocument>,
    /// Tag grezzi per la vista avanzata (sola lettura).
    pub raw: HashMap<PathBuf, Vec<RawTag>>,
    pub errors: HashMap<PathBuf, String>,
    pub loading: BTreeSet<PathBuf>,
    /// mtime (secondi Unix) dei documenti in cache, per invalidarli quando
    /// il file cambia sul disco.
    pub loaded: HashMap<PathBuf, i64>,
    /// Selezione per cui è stato costruito il form.
    pub form_paths: Vec<PathBuf>,
    pub form_values: BTreeMap<MetadataField, String>,
    /// Campi con valori diversi nella multi-selezione.
    pub form_mixed: BTreeSet<MetadataField>,
    /// Campi modificati e non ancora salvati.
    pub form_dirty: BTreeSet<MetadataField>,
    pub raw_filter: String,
    /// Riassunti già letti: percorso → mtime Unix in secondi.
    pub summarized: HashMap<PathBuf, i64>,
    /// Richieste di riassunto in volo: percorso → mtime Unix in secondi.
    pub pending_summary: HashMap<PathBuf, i64>,
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
    pub filter_format: FormatFilter,
    /// Mostra solo le foto con almeno questa valutazione.
    pub filter_rating: Option<u8>,
    /// Mostra solo le foto con coordinate GPS.
    pub filter_gps: bool,
    /// Filtro testuale sulle parole chiave indicizzate.
    pub filter_keywords: String,
    pub tree_filter: String,
    pub thumb_size: f32,
    pub recent_folders: Vec<PathBuf>,
    pub inspector_tab: InspectorTab,
    pub loading_dir: Option<PathBuf>,
    pub error: Option<String>,
    /// Righe dell'indice relative alla cartella corrente, per gli ordinamenti
    /// e i filtri che richiedono i metadati.
    pub index_rows: HashMap<PathBuf, IndexRow>,
    pub conflict_prompt: Option<ConflictPrompt>,
    pub rename_prompt: Option<RenamePrompt>,
    pub new_folder_prompt: Option<NewFolderPrompt>,
    pub delete_dir_prompt: Option<DeleteDirPrompt>,
    pub op_status: Option<OpStatus>,
    /// Editor e cache dei metadati.
    pub meta: MetaState,
    /// Versione di ExifTool rilevata all'avvio (`None` = editor disabilitato).
    pub exiftool: Option<String>,
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
            filter_format: FormatFilter::All,
            filter_rating: None,
            filter_gps: false,
            filter_keywords: String::new(),
            tree_filter: String::new(),
            thumb_size: persisted
                .thumb_size
                .unwrap_or(160.0)
                .clamp(MIN_THUMB_SIZE, MAX_THUMB_SIZE),
            recent_folders: persisted.recent_folders,
            inspector_tab: InspectorTab::Informazioni,
            loading_dir: None,
            error: None,
            index_rows: HashMap::new(),
            conflict_prompt: None,
            rename_prompt: None,
            new_folder_prompt: None,
            delete_dir_prompt: None,
            op_status: None,
            meta: MetaState::default(),
            exiftool: None,
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

    /// Percorsi su cui operare partendo da una cella: l'intera selezione se la
    /// cella ne fa parte, altrimenti solo la cella.
    pub fn selection_paths(&self, fallback: &Path) -> Vec<PathBuf> {
        if self.selection.contains(fallback) {
            self.entries
                .iter()
                .filter(|entry| self.selection.contains(&entry.path))
                .map(|entry| entry.path.clone())
                .collect()
        } else {
            vec![fallback.to_path_buf()]
        }
    }

    /// Percorsi selezionati nell'ordine della griglia.
    pub fn selected_paths(&self) -> Vec<PathBuf> {
        self.entries
            .iter()
            .filter(|entry| self.selection.contains(&entry.path))
            .map(|entry| entry.path.clone())
            .collect()
    }

    /// True se la voce supera i filtri della griglia (formato, valutazione,
    /// GPS, parole chiave). I filtri sui metadati richiedono una riga indice.
    pub fn asset_visible(&self, entry: &PhotoAsset) -> bool {
        if !self.filter_format.matches(entry.format) {
            return false;
        }
        let keyword = self.filter_keywords.trim().to_lowercase();
        let richiede_riga = self.filter_rating.is_some() || self.filter_gps || !keyword.is_empty();
        if !richiede_riga {
            return true;
        }
        let Some(row) = self.index_rows.get(&entry.path) else {
            return false;
        };
        if let Some(minimum) = self.filter_rating {
            if row.rating.is_none_or(|rating| rating < minimum) {
                return false;
            }
        }
        if self.filter_gps && !row.has_gps {
            return false;
        }
        if !keyword.is_empty() {
            let keywords = row.keywords.as_deref().unwrap_or_default().to_lowercase();
            if !keywords.contains(&keyword) {
                return false;
            }
        }
        true
    }

    /// Ricostruisce il form dai documenti letti delle foto selezionate.
    /// Senza documenti per tutte le foto non fa nulla: il form appare solo
    /// quando la selezione è pronta.
    pub fn rebuild_meta_form(&mut self) {
        let paths = self.selected_paths();
        self.meta.form_paths = paths.clone();
        self.meta.form_values.clear();
        self.meta.form_mixed.clear();
        self.meta.form_dirty.clear();
        if paths.is_empty() {
            return;
        }
        for field in MetadataField::ALL {
            let mut valori = Vec::with_capacity(paths.len());
            for path in &paths {
                let Some(document) = self.meta.docs.get(path) else {
                    return;
                };
                valori.push(document.display(field));
            }
            let primo = valori.first().cloned().unwrap_or_default();
            if valori.iter().all(|valore| *valore == primo) {
                if !primo.is_empty() {
                    self.meta.form_values.insert(field, primo);
                }
            } else {
                self.meta.form_mixed.insert(field);
            }
        }
    }

    /// Ordina `entries`; per data di acquisizione e valutazione la chiave
    /// arriva dall'indice (le righe mancanti vanno in fondo).
    pub fn apply_sort(&mut self) {
        match self.sort_key {
            SortKey::AcquisitionDate => {
                let rows = &self.index_rows;
                let key = |asset: &PhotoAsset| {
                    rows.get(&asset.path)
                        .and_then(|row| row.date_original.clone())
                };
                self.entries.sort_by(|a, b| match (key(a), key(b)) {
                    (Some(left), Some(right)) => left.cmp(&right),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                });
                if !self.sort_ascending {
                    self.entries.reverse();
                }
            }
            SortKey::Rating => {
                let rows = &self.index_rows;
                self.entries.sort_by_key(|asset| {
                    rows.get(&asset.path)
                        .and_then(|row| row.rating)
                        .unwrap_or(0)
                });
                if !self.sort_ascending {
                    self.entries.reverse();
                }
            }
            _ => sort_assets(&mut self.entries, self.sort_key, self.sort_ascending),
        }
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
    /// Foto rilasciate su una cartella.
    pub drop_onto: Option<DropRequest>,
    /// Copia/sposta la selezione scegliendo la destinazione col dialogo nativo.
    pub transfer: Option<TransferRequest>,
    /// Rinomina una foto (apre il prompt).
    pub rename_photo: Option<PathBuf>,
    /// Rinomina una cartella (apre il prompt).
    pub rename_folder: Option<PathBuf>,
    /// Nuova sottocartella dentro questo genitore (apre il prompt).
    pub create_folder: Option<PathBuf>,
    /// Elimina questa cartella, se vuota (apre la conferma).
    pub delete_folder: Option<PathBuf>,
    /// Spostamento/copia confermati dal prompt.
    pub confirm_transfer: Option<(Vec<PathBuf>, PathBuf, bool, ConflictPolicy)>,
    /// Rinomina confermata dal prompt.
    pub confirm_rename: Option<(RenameTarget, String)>,
    /// Creazione cartella confermata dal prompt.
    pub confirm_new_folder: Option<(PathBuf, String)>,
    /// Eliminazione cartella confermata dal prompt.
    pub confirm_delete_dir: Option<PathBuf>,
    /// Salva i campi metadati modificati sulla selezione.
    pub metadata_save: bool,
    /// Rilegge i metadati della selezione.
    pub metadata_reload: bool,
    /// Imposta la valutazione (0–5 stelle) sulle foto indicate.
    pub metadata_rate: Option<(Vec<PathBuf>, u8)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn asset(path: &str) -> PhotoAsset {
        PhotoAsset {
            path: PathBuf::from(path),
            format: FormatKind::Jpeg,
            byte_size: 1,
            modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
        }
    }

    fn row(path: &str, rating: Option<u8>, date: Option<&str>) -> IndexRow {
        IndexRow {
            path: PathBuf::from(path),
            rating,
            date_original: date.map(str::to_owned),
            ..IndexRow::default()
        }
    }

    #[test]
    fn ordinamento_per_valutazione_usa_l_indice() {
        let mut state = AppState::new(PersistedState::default());
        state.entries = vec![asset("/foto/a.jpg"), asset("/foto/b.jpg")];
        state.index_rows.insert(
            PathBuf::from("/foto/a.jpg"),
            row("/foto/a.jpg", Some(1), None),
        );
        state.index_rows.insert(
            PathBuf::from("/foto/b.jpg"),
            row("/foto/b.jpg", Some(5), None),
        );
        state.sort_key = SortKey::Rating;
        state.sort_ascending = false;
        state.apply_sort();
        assert_eq!(state.entries[0].path, PathBuf::from("/foto/b.jpg"));
    }

    #[test]
    fn ordinamento_per_data_manda_in_fondo_le_righe_senza_data() {
        let mut state = AppState::new(PersistedState::default());
        state.entries = vec![
            asset("/foto/a.jpg"),
            asset("/foto/b.jpg"),
            asset("/foto/c.jpg"),
        ];
        state.index_rows.insert(
            PathBuf::from("/foto/a.jpg"),
            row("/foto/a.jpg", None, Some("2023-01-01T10:00:00")),
        );
        state.index_rows.insert(
            PathBuf::from("/foto/c.jpg"),
            row("/foto/c.jpg", None, Some("2021-06-01T10:00:00")),
        );
        state.sort_key = SortKey::AcquisitionDate;
        state.sort_ascending = true;
        state.apply_sort();
        let paths: Vec<&Path> = state
            .entries
            .iter()
            .map(|entry| entry.path.as_path())
            .collect();
        assert_eq!(
            paths,
            vec![
                Path::new("/foto/c.jpg"),
                Path::new("/foto/a.jpg"),
                Path::new("/foto/b.jpg")
            ]
        );
    }

    #[test]
    fn selection_paths_usa_la_selezione_se_la_cella_ne_fa_parte() {
        let mut state = AppState::new(PersistedState::default());
        state.entries = vec![asset("/foto/a.jpg"), asset("/foto/b.jpg")];
        state.selection.insert(PathBuf::from("/foto/a.jpg"));
        state.selection.insert(PathBuf::from("/foto/b.jpg"));
        let paths = state.selection_paths(Path::new("/foto/b.jpg"));
        assert_eq!(paths.len(), 2);

        let single = state.selection_paths(Path::new("/foto/ignota.jpg"));
        assert_eq!(single, vec![PathBuf::from("/foto/ignota.jpg")]);
    }

    #[test]
    fn i_filtri_metadati_usano_l_indice() {
        let mut state = AppState::new(PersistedState::default());
        state.entries = vec![
            asset("/foto/a.jpg"),
            asset("/foto/b.jpg"),
            asset("/foto/c.jpg"),
        ];
        let mut gps = row("/foto/a.jpg", Some(5), None);
        gps.has_gps = true;
        gps.keywords = Some("mare, blu".to_owned());
        state.index_rows.insert(PathBuf::from("/foto/a.jpg"), gps);
        state.index_rows.insert(
            PathBuf::from("/foto/b.jpg"),
            row("/foto/b.jpg", Some(2), None),
        );

        state.filter_rating = Some(3);
        assert!(state.asset_visible(&state.entries[0]));
        assert!(!state.asset_visible(&state.entries[1]));
        assert!(!state.asset_visible(&state.entries[2]));

        state.filter_rating = None;
        state.filter_gps = true;
        assert!(state.asset_visible(&state.entries[0]));
        assert!(!state.asset_visible(&state.entries[1]));

        state.filter_gps = false;
        state.filter_keywords = "MARE".to_owned();
        assert!(state.asset_visible(&state.entries[0]));
        assert!(!state.asset_visible(&state.entries[2]));
    }
}
