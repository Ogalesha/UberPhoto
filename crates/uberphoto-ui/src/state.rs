use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use eframe::egui;
use serde::{Deserialize, Serialize};
use uberphoto_core::{FormatKind, GpsLocation, PhotoAsset};
use uberphoto_db::IndexRow;
use uberphoto_export::{ExportReport, ExportSettings};
use uberphoto_files::{sort_assets, ConflictPolicy, DirNode, FileOpReport, SortKey};
use uberphoto_geo::{GeocodeResult, LocationSuggestion};
use uberphoto_image::{EditRecipe, EditSource};
use uberphoto_metadata::{LocationField, LocationRole, MetadataDocument, MetadataField, RawTag};

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
    /// Ultimo centro della mappa GPS (longitudine, latitudine) e zoom.
    pub map_center: Option<(f64, f64)>,
    pub map_zoom: Option<f64>,
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

/// Destinazione di un suggerimento geografico (risultato di ricerca o
/// reverse geocoding).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestionTarget {
    /// Posiziona il marker sulla mappa.
    Marker,
    /// Compila i campi della località di scatto.
    Created,
    /// Compila i campi della località raffigurata.
    Shown,
}

/// Stato della scheda GPS: mappa, bozza delle coordinate e delle località.
///
/// Il marker è la fonte di verità della bozza: i campi manuali lo
/// alimentano e ne vengono riallineati. Le modifiche ai campi di località
/// restano separate e vengono salvate a parte.
pub struct GpsState {
    /// Tile HTTP di OpenStreetMap, creati alla prima apertura della scheda.
    pub tiles: Option<walkers::HttpTiles>,
    pub memory: walkers::MapMemory,
    /// Posizione di riferimento passata a `Map::new`.
    pub my_position: walkers::Position,
    /// Marker in bozza (None = nessuna coordinata pronta).
    pub marker: Option<GpsLocation>,
    /// Le foto selezionate hanno coordinate diverse (o alcune ne sono prive).
    pub marker_mixed: bool,
    pub manual_lat: String,
    pub manual_lon: String,
    pub manual_alt: String,
    /// Errore di validazione dei campi manuali.
    pub manual_error: Option<String>,
    /// Selezione per cui è stata costruita la bozza.
    pub form_paths: Vec<PathBuf>,
    /// Valori comuni dei campi di località: (ruolo, campo) → testo.
    pub location_values: BTreeMap<(LocationRole, LocationField), String>,
    /// Campi di località con valori diversi nella multi-selezione.
    pub location_mixed: BTreeSet<(LocationRole, LocationField)>,
    /// Campi di località modificati e non ancora salvati.
    pub location_dirty: BTreeSet<(LocationRole, LocationField)>,
    /// Bozza (marker o località) modificata e non ancora salvata.
    pub draft_dirty: bool,
    /// Ultima posizione su cui è stata centrata la mappa automaticamente.
    pub centered_for: Option<GpsLocation>,
    pub search_query: String,
    pub search_results: Vec<GeocodeResult>,
    pub searching: bool,
    /// Errore dell'ultima ricerca o reverse geocoding.
    pub geo_error: Option<String>,
    pub reverse_pending: bool,
    /// Nota sull'ultimo suggerimento reverse applicato.
    pub reverse_note: Option<String>,
    /// Finestra con la mappa grande.
    pub large_map: bool,
}

impl Default for GpsState {
    fn default() -> Self {
        let mut memory = walkers::MapMemory::default();
        let _ = memory.set_zoom(6.0);
        Self {
            tiles: None,
            memory,
            my_position: walkers::lon_lat(12.5, 41.9),
            marker: None,
            marker_mixed: false,
            manual_lat: String::new(),
            manual_lon: String::new(),
            manual_alt: String::new(),
            manual_error: None,
            form_paths: Vec::new(),
            location_values: BTreeMap::new(),
            location_mixed: BTreeSet::new(),
            location_dirty: BTreeSet::new(),
            draft_dirty: false,
            centered_for: None,
            search_query: String::new(),
            search_results: Vec::new(),
            searching: false,
            geo_error: None,
            reverse_pending: false,
            reverse_note: None,
            large_map: false,
        }
    }
}

impl GpsState {
    /// Imposta il marker riallineando i campi manuali. Non muove la mappa.
    pub fn set_marker(&mut self, marker: Option<GpsLocation>) {
        self.marker = marker;
        self.manual_error = None;
        self.draft_dirty = true;
        match marker {
            Some(location) => {
                self.manual_lat = format!("{:.6}", location.latitude);
                self.manual_lon = format!("{:.6}", location.longitude);
                self.manual_alt = location
                    .altitude
                    .map(|altitude| {
                        if altitude.fract() == 0.0 {
                            format!("{altitude:.0}")
                        } else {
                            format!("{altitude:.1}")
                        }
                    })
                    .unwrap_or_default();
            }
            None => {
                self.manual_lat.clear();
                self.manual_lon.clear();
                self.manual_alt.clear();
            }
        }
    }

    /// Riparsa i campi manuali: aggiorna il marker o segnala l'errore.
    /// Il testo digitato non viene mai sovrascritto qui.
    pub fn apply_manual_fields(&mut self) {
        self.draft_dirty = true;
        let latitudine =
            uberphoto_geo::parse_coordinate(&self.manual_lat, uberphoto_geo::Axis::Latitude);
        let longitudine =
            uberphoto_geo::parse_coordinate(&self.manual_lon, uberphoto_geo::Axis::Longitude);
        let altitudine = match uberphoto_geo::parse_altitude(&self.manual_alt) {
            Ok(value) => value,
            Err(message) => {
                self.marker = None;
                self.manual_error = Some(message);
                return;
            }
        };
        match (latitudine, longitudine) {
            (Ok(latitudine), Ok(longitudine)) => {
                self.marker = Some(GpsLocation {
                    latitude: latitudine,
                    longitude: longitudine,
                    altitude: altitudine,
                });
                self.manual_error = None;
            }
            (Err(message), _) | (_, Err(message)) => {
                self.marker = None;
                self.manual_error = Some(message);
            }
        }
    }

    /// Compila i campi della località indicata con i valori del suggerimento.
    /// I campi senza valore nel suggerimento restano invariati.
    pub fn fill_location(&mut self, role: LocationRole, suggestion: &LocationSuggestion) {
        let coppie: [(LocationField, &Option<String>); 6] = [
            (LocationField::Place, &suggestion.place),
            (LocationField::Sublocation, &suggestion.sublocation),
            (LocationField::City, &suggestion.city),
            (LocationField::State, &suggestion.state),
            (LocationField::Country, &suggestion.country),
            (LocationField::CountryCode, &suggestion.country_code),
        ];
        for (field, value) in coppie {
            if let Some(value) = value {
                self.location_values.insert((role, field), value.clone());
                self.location_dirty.insert((role, field));
            }
        }
        self.draft_dirty = true;
    }

    /// Valore in bozza di un campo di località.
    pub fn location_value(&self, role: LocationRole, field: LocationField) -> &str {
        self.location_values
            .get(&(role, field))
            .map(String::as_str)
            .unwrap_or_default()
    }

    /// Nota se il campo ha valori diversi nella multi-selezione.
    pub fn location_is_mixed(&self, role: LocationRole, field: LocationField) -> bool {
        self.location_mixed.contains(&(role, field))
    }
}

/// Stato della scheda Colori: ricetta non distruttiva, anteprima e cronologia.
///
/// La ricetta vive separata sia dai pixel dell'originale (che non vengono
/// mai riscritti) sia dai metadati (che seguono il proprio flusso di
/// salvataggio). La cronologia undo/redo è per foto: cambiando selezione
/// viene scartata.
pub struct EditState {
    /// Foto a cui appartiene la ricetta corrente.
    pub path: Option<PathBuf>,
    pub mtime: Option<SystemTime>,
    pub recipe: EditRecipe,
    /// Ultima ricetta confermata nel catalogo locale.
    pub saved: EditRecipe,
    pub undo: Vec<EditRecipe>,
    pub redo: Vec<EditRecipe>,
    /// Ricetta all'inizio del trascinamento di un controllo.
    pub drag_base: Option<EditRecipe>,
    /// Sorgente decodificata condivisa col worker di elaborazione.
    pub source: Option<Arc<EditSource>>,
    /// Decodifica della sorgente in corso.
    pub source_requested: bool,
    pub source_error: Option<String>,
    /// Texture dell'originale, caricata alla prima necessità.
    pub original: Option<egui::TextureHandle>,
    pub original_size: egui::Vec2,
    /// Texture dell'anteprima con la ricetta applicata.
    pub edited: Option<egui::TextureHandle>,
    pub edited_size: egui::Vec2,
    /// Ricetta dell'ultima elaborazione richiesta (`None` = originale).
    pub requested: Option<EditRecipe>,
    pub generation: u64,
    pub applied_generation: u64,
    /// Elaborazione in corso nel worker.
    pub busy: bool,
    /// Confronto prima/dopo: il pulsante "Prima" è premuto.
    pub compare: bool,
    /// Preset salvati nel catalogo locale.
    pub presets: Vec<(String, EditRecipe)>,
    pub preset_name: String,
    /// Errore dell'ultimo salvataggio di ricetta o preset.
    pub save_error: Option<String>,
    /// La ricetta è cambiata e va scritta nel catalogo.
    pub pending_save: bool,
}

impl Default for EditState {
    fn default() -> Self {
        Self {
            path: None,
            mtime: None,
            recipe: EditRecipe::default(),
            saved: EditRecipe::default(),
            undo: Vec::new(),
            redo: Vec::new(),
            drag_base: None,
            source: None,
            source_requested: false,
            source_error: None,
            original: None,
            original_size: egui::Vec2::ZERO,
            edited: None,
            edited_size: egui::Vec2::ZERO,
            requested: None,
            generation: 0,
            applied_generation: 0,
            busy: false,
            compare: false,
            presets: Vec::new(),
            preset_name: String::new(),
            save_error: None,
            pending_save: false,
        }
    }
}

impl EditState {
    /// Carica la ricetta di una foto, scartando anteprime e cronologia.
    /// I preset restano: appartengono al catalogo, non alla singola foto.
    pub fn carica(&mut self, path: &Path, mtime: Option<SystemTime>, recipe: EditRecipe) {
        let preset = std::mem::take(&mut self.presets);
        let nome = std::mem::take(&mut self.preset_name);
        let errore = self.save_error.take();
        *self = Self::default();
        self.presets = preset;
        self.preset_name = nome;
        self.save_error = errore;
        self.path = Some(path.to_path_buf());
        self.mtime = mtime;
        self.recipe = recipe;
        self.saved = recipe;
    }

    /// Nessuna foto modificabile: libera anteprime e sorgente.
    pub fn scarica(&mut self) {
        let preset = std::mem::take(&mut self.presets);
        let nome = std::mem::take(&mut self.preset_name);
        let errore = self.save_error.take();
        *self = Self::default();
        self.presets = preset;
        self.preset_name = nome;
        self.save_error = errore;
    }

    /// Registra lo stato attuale per l'annullamento e invalida il redo.
    pub fn push_undo(&mut self) {
        self.undo.push(self.recipe);
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.pending_save = true;
    }

    pub fn undo(&mut self) -> bool {
        let Some(precedente) = self.undo.pop() else {
            return false;
        };
        self.redo.push(self.recipe);
        self.recipe = precedente;
        self.pending_save = true;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(successiva) = self.redo.pop() else {
            return false;
        };
        self.undo.push(self.recipe);
        self.recipe = successiva;
        self.pending_save = true;
        true
    }

    /// Ripristina tutte le regolazioni al valore neutro.
    pub fn reset_all(&mut self) {
        if self.recipe.is_identity() {
            return;
        }
        self.push_undo();
        self.recipe = EditRecipe::default();
    }

    /// Applica un preset alla ricetta corrente.
    pub fn apply_preset(&mut self, recipe: EditRecipe) {
        if recipe == self.recipe {
            return;
        }
        self.push_undo();
        self.recipe = recipe;
    }

    /// Vero se l'anteprima modificata è disponibile.
    pub fn showing_edited(&self) -> bool {
        self.edited.is_some()
    }

    /// Dimensione in pixel dell'anteprima attiva (modificata o originale).
    pub fn active_size(&self) -> egui::Vec2 {
        if self.showing_edited() {
            self.edited_size
        } else {
            self.original_size
        }
    }

    /// La ricetta differisce da quella salvata nel catalogo.
    pub fn is_dirty(&self) -> bool {
        self.recipe != self.saved
    }
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
    /// Scheda GPS: mappa, coordinate e località.
    pub gps: GpsState,
    /// Scheda Colori: ricetta non distruttiva, anteprima e preset.
    pub edit: EditState,
    /// Scheda Esportazione: parametri, avanzamento e report verificati.
    pub export: ExportState,
    /// Versione di ExifTool rilevata all'avvio (`None` = editor disabilitato).
    pub exiftool: Option<String>,
    /// Logo dell'app, caricato una sola volta come texture della UI.
    pub logo: Option<egui::TextureHandle>,
}

impl AppState {
    pub fn new(persisted: PersistedState) -> Self {
        let mut gps = GpsState::default();
        if let Some((longitude, latitude)) = persisted.map_center {
            let position = walkers::lon_lat(longitude, latitude);
            gps.my_position = position;
            gps.memory.center_at(position);
        }
        if let Some(zoom) = persisted.map_zoom {
            let _ = gps.memory.set_zoom(zoom.clamp(2.0, 19.0));
        }
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
            gps,
            edit: EditState::default(),
            export: ExportState::default(),
        }
    }

    pub fn persisted(&self) -> PersistedState {
        let (longitude, latitude) = self
            .gps
            .memory
            .detached()
            .map(|position| (position.x(), position.y()))
            .unwrap_or((self.gps.my_position.x(), self.gps.my_position.y()));
        PersistedState {
            recent_folders: self.recent_folders.clone(),
            last_dir: self.current_dir.clone(),
            thumb_size: Some(self.thumb_size),
            map_center: Some((longitude, latitude)),
            map_zoom: Some(self.gps.memory.zoom()),
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

    /// Centro della mappa sulla posizione indicata (o sul riferimento
    /// corrente); con una posizione valida alza lo zoom se è troppo ampio.
    pub fn center_gps_map(&mut self, location: Option<GpsLocation>) {
        let target = location
            .map(|location| walkers::lon_lat(location.longitude, location.latitude))
            .unwrap_or(self.gps.my_position);
        self.gps.my_position = target;
        self.gps.memory.center_at(target);
        if location.is_some() && self.gps.memory.zoom() < 10.0 {
            let _ = self.gps.memory.set_zoom(12.0);
        }
    }

    /// Ricostruisce la bozza GPS dai documenti letti delle foto selezionate:
    /// marker comune (o stato misto), campi manuali e località per ruolo.
    /// Senza documenti per tutte le foto non fa nulla: la bozza appare solo
    /// quando la selezione è pronta.
    pub fn rebuild_gps_form(&mut self) {
        let paths = self.selected_paths();
        self.gps.form_paths = paths.clone();
        self.gps.marker = None;
        self.gps.marker_mixed = false;
        self.gps.manual_lat.clear();
        self.gps.manual_lon.clear();
        self.gps.manual_alt.clear();
        self.gps.manual_error = None;
        self.gps.location_values.clear();
        self.gps.location_mixed.clear();
        self.gps.location_dirty.clear();
        self.gps.draft_dirty = false;
        if paths.is_empty() {
            return;
        }

        let mut posizioni: Vec<Option<GpsLocation>> = Vec::with_capacity(paths.len());
        for path in &paths {
            let Some(document) = self.meta.docs.get(path) else {
                return;
            };
            posizioni.push(document.gps.map(|gps| GpsLocation {
                latitude: gps.latitude,
                longitude: gps.longitude,
                altitude: gps.altitude,
            }));
        }
        let primo = posizioni.first().copied().flatten();
        if posizioni.iter().all(|posizione| *posizione == primo) {
            self.gps.marker = primo;
            if let Some(marker) = primo {
                self.gps.manual_lat = format!("{:.6}", marker.latitude);
                self.gps.manual_lon = format!("{:.6}", marker.longitude);
                if let Some(altitude) = marker.altitude {
                    self.gps.manual_alt = if altitude.fract() == 0.0 {
                        format!("{altitude:.0}")
                    } else {
                        format!("{altitude:.1}")
                    };
                }
            }
        } else {
            self.gps.marker_mixed = true;
        }
        if let Some(marker) = self.gps.marker {
            if self.gps.centered_for != Some(marker) {
                self.center_gps_map(Some(marker));
                self.gps.centered_for = Some(marker);
            }
        } else {
            self.gps.centered_for = None;
        }

        for role in LocationRole::ALL {
            for field in LocationField::ALL {
                let mut valori = Vec::with_capacity(paths.len());
                for path in &paths {
                    let Some(document) = self.meta.docs.get(path) else {
                        return;
                    };
                    valori.push(document.location(role).display(field));
                }
                let primo = valori.first().cloned().unwrap_or_default();
                if valori.iter().all(|valore| *valore == primo) {
                    if !primo.is_empty() {
                        self.gps.location_values.insert((role, field), primo);
                    }
                } else {
                    self.gps.location_mixed.insert((role, field));
                }
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

/// Scheda Esportazione: parametri correnti, avanzamento e report.
#[derive(Debug, Default)]
pub struct ExportState {
    pub settings: ExportSettings,
    pub busy: bool,
    /// File completati (esportati o falliti).
    pub done: usize,
    pub total: usize,
    pub reports: Vec<ExportReport>,
    pub failures: Vec<(PathBuf, String)>,
    pub summary: Option<String>,
    pub error: Option<String>,
    pub cancelled: bool,
}

impl ExportState {
    /// Azzera l'esito della precedente esportazione prima di avviarne una.
    pub fn prepara(&mut self, total: usize) {
        self.busy = true;
        self.done = 0;
        self.total = total;
        self.reports.clear();
        self.failures.clear();
        self.summary = None;
        self.error = None;
        self.cancelled = false;
    }

    pub fn registra_report(&mut self, report: ExportReport) {
        self.done += 1;
        self.reports.push(report);
    }

    pub fn registra_errore(&mut self, path: PathBuf, error: String) {
        self.done += 1;
        self.failures.push((path, error));
    }

    /// Avanzamento 0..=1 per la barra di stato.
    pub fn progress(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.done as f32 / self.total as f32).clamp(0.0, 1.0)
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
    /// Applica il marker GPS (coordinate in bozza) alla selezione.
    pub gps_apply: bool,
    /// Rimuove le coordinate GPS dalla selezione.
    pub gps_remove: bool,
    /// Salva i campi di località modificati sulla selezione.
    pub gps_save_location: bool,
    /// Scarta le modifiche in bozza e rilegge i valori dal file.
    pub gps_reset: bool,
    /// Avvia una ricerca Nominatim (solo su azione esplicita).
    pub geocode_search: Option<String>,
    /// Rileva la località dalle coordinate indicate (reverse geocoding).
    pub geocode_reverse: Option<GpsLocation>,
    /// Suggerimento geografico da applicare (marker o località).
    pub gps_use_suggestion: Option<(GeocodeResult, SuggestionTarget)>,
    /// Salva la ricetta corrente della scheda Colori come preset.
    pub edit_save_preset: bool,
    /// Elimina il preset indicato dal catalogo.
    pub edit_delete_preset: Option<String>,
    /// Applica questa ricetta a tutte le foto selezionate.
    pub edit_apply_preset: Option<EditRecipe>,
    /// Avvia l'esportazione della selezione con i parametri correnti.
    pub export_start: bool,
    /// Annulla l'esportazione in corso dopo il file corrente.
    pub export_cancel: bool,
    /// Sceglie la cartella di destinazione con il dialogo nativo.
    pub export_pick_destination: bool,
    /// Cancella report ed errori dell'ultima esportazione.
    pub export_clear: bool,
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
    fn il_marker_sincronizza_i_campi_manuali() {
        let mut gps = GpsState::default();
        gps.set_marker(Some(GpsLocation {
            latitude: 44.4949,
            longitude: 11.3426,
            altitude: Some(54.0),
        }));
        assert_eq!(gps.manual_lat, "44.494900");
        assert_eq!(gps.manual_lon, "11.342600");
        assert_eq!(gps.manual_alt, "54");
        assert!(gps.draft_dirty);

        gps.set_marker(None);
        assert!(gps.manual_lat.is_empty());
        assert!(gps.manual_lon.is_empty());
        assert!(gps.manual_alt.is_empty());
    }

    #[test]
    fn le_coordinate_manuali_accettano_la_virgola() {
        let mut gps = GpsState {
            manual_lat: "44,4949".to_owned(),
            manual_lon: "11,3426".to_owned(),
            manual_alt: "54".to_owned(),
            ..GpsState::default()
        };
        gps.apply_manual_fields();
        let marker = gps.marker.expect("marker dalle coordinate manuali");
        assert_eq!(marker.latitude, 44.4949);
        assert_eq!(marker.longitude, 11.3426);
        assert_eq!(marker.altitude, Some(54.0));
        assert!(gps.manual_error.is_none());
    }

    #[test]
    fn le_coordinate_non_valide_non_producono_marker() {
        let mut gps = GpsState {
            manual_lat: "nord".to_owned(),
            manual_lon: "11.3426".to_owned(),
            ..GpsState::default()
        };
        gps.apply_manual_fields();
        assert!(gps.marker.is_none());
        assert!(gps
            .manual_error
            .as_deref()
            .is_some_and(|e| e.contains("latitudine")));
    }

    #[test]
    fn il_suggerimento_compila_solo_i_campi_presenti() {
        let mut gps = GpsState::default();
        let suggerimento = LocationSuggestion {
            place: Some("Duomo".to_owned()),
            city: Some("Milano".to_owned()),
            country: Some("Italia".to_owned()),
            country_code: Some("IT".to_owned()),
            ..LocationSuggestion::default()
        };
        gps.fill_location(LocationRole::Created, &suggerimento);
        assert_eq!(
            gps.location_value(LocationRole::Created, LocationField::City),
            "Milano"
        );
        assert_eq!(
            gps.location_value(LocationRole::Created, LocationField::CountryCode),
            "IT"
        );
        assert!(gps
            .location_dirty
            .contains(&(LocationRole::Created, LocationField::City)));
        assert!(!gps
            .location_dirty
            .contains(&(LocationRole::Created, LocationField::State)));
        assert!(gps
            .location_value(LocationRole::Shown, LocationField::City)
            .is_empty());
        assert!(gps.draft_dirty);
    }

    #[test]
    fn centro_e_zoom_della_mappa_sopravvivono_al_riavvio() {
        let mut state = AppState::new(PersistedState::default());
        state.center_gps_map(Some(GpsLocation {
            latitude: 45.0,
            longitude: 9.0,
            altitude: None,
        }));
        let persisted = state.persisted();
        let (longitude, latitude) = persisted.map_center.expect("centro persistito");
        assert!((longitude - 9.0).abs() < 1e-9);
        assert!((latitude - 45.0).abs() < 1e-9);

        let riavviato = AppState::new(persisted);
        let centro = riavviato
            .gps
            .memory
            .detached()
            .expect("centro ripristinato");
        assert!((centro.x() - 9.0).abs() < 1e-9);
        assert!((centro.y() - 45.0).abs() < 1e-9);
        assert_eq!(riavviato.gps.memory.zoom(), 12.0);
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

    #[test]
    fn la_cronologia_annulla_e_ripete_le_regolazioni() {
        let mut edit = EditState {
            recipe: EditRecipe {
                exposure: 1.0,
                ..EditRecipe::default()
            },
            ..EditState::default()
        };
        edit.push_undo();
        edit.recipe = EditRecipe {
            exposure: 2.0,
            ..EditRecipe::default()
        };
        assert_eq!(edit.undo.len(), 1);

        assert!(edit.undo());
        assert_eq!(edit.recipe.exposure, 1.0);
        assert_eq!(edit.redo.len(), 1);
        assert!(edit.redo());
        assert_eq!(edit.recipe.exposure, 2.0);

        // Una nuova modifica invalida il redo.
        assert!(edit.undo());
        edit.push_undo();
        assert!(edit.redo.is_empty());
        assert!(!edit.redo());
    }

    #[test]
    fn il_ripristino_totale_torna_alla_ricetta_neutra() {
        let mut edit = EditState {
            recipe: EditRecipe {
                contrast: 40.0,
                rotation: 90,
                ..EditRecipe::default()
            },
            ..EditState::default()
        };
        assert!(!edit.recipe.is_identity());
        edit.reset_all();
        assert!(edit.recipe.is_identity());
        assert_eq!(edit.undo.len(), 1);
        assert!(edit.undo());
        assert_eq!(edit.recipe.contrast, 40.0);
        assert_eq!(edit.recipe.rotation, 90);
    }

    #[test]
    fn il_caricamento_scarta_la_cronologia_e_conserva_i_preset() {
        let mut edit = EditState {
            recipe: EditRecipe {
                exposure: 3.0,
                ..EditRecipe::default()
            },
            ..EditState::default()
        };
        edit.presets
            .push(("Prova".to_owned(), EditRecipe::default()));
        edit.push_undo();

        edit.carica(
            Path::new("/foto/a.jpg"),
            None,
            EditRecipe {
                gamma: 1.5,
                ..EditRecipe::default()
            },
        );
        assert!(edit.undo.is_empty());
        assert!(edit.redo.is_empty());
        assert_eq!(edit.recipe.gamma, 1.5);
        assert_eq!(edit.path.as_deref(), Some(Path::new("/foto/a.jpg")));
        assert_eq!(edit.presets.len(), 1);
        assert!(!edit.is_dirty());

        edit.scarica();
        assert!(edit.path.is_none());
        assert!(edit.recipe.is_identity());
        assert_eq!(edit.presets.len(), 1, "i preset appartengono al catalogo");
    }

    #[test]
    fn il_preset_applicato_entra_nella_cronologia() {
        let mut edit = EditState::default();
        edit.apply_preset(EditRecipe {
            vibrance: 25.0,
            ..EditRecipe::default()
        });
        assert_eq!(edit.recipe.vibrance, 25.0);
        assert_eq!(edit.undo.len(), 1);
        assert!(edit.undo());
        assert!(edit.recipe.is_identity());

        // Applicare la ricetta già attiva non crea voci.
        edit.apply_preset(EditRecipe::default());
        assert!(edit.undo.is_empty());
    }
}
