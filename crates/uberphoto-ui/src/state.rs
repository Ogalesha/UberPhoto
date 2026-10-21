use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use eframe::egui;
use serde::{Deserialize, Serialize};
use uberphoto_core::{FormatKind, GpsLocation, PhotoAsset};
use uberphoto_db::IndexRow;
use uberphoto_export::{ExportReport, ExportSettings, SaveMode};
use uberphoto_files::{sort_assets, ConflictPolicy, DirNode, FileOpReport, SortKey};
use uberphoto_geo::{GeocodeResult, LocationSuggestion};
use uberphoto_image::{EditRecipe, EditSource};
use uberphoto_metadata::{
    LocationField, LocationRole, MetadataDocument, MetadataEdit, MetadataField, RawTag,
};

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
    /// Campi modificati e non ancora salvati per ciascuna foto.
    pub drafts: HashMap<PathBuf, BTreeMap<MetadataField, Option<String>>>,
    /// Campi modificati nella selezione corrente (vista derivata).
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

/// Bozza GPS di una singola foto, indipendente dalla selezione corrente.
#[derive(Debug, Clone, Default)]
pub struct GpsDraft {
    /// `Some(None)` significa coordinate rimosse, `Some(Some(_))` coordinate
    /// impostate, `None` nessuna modifica.
    pub coordinates: Option<Option<GpsLocation>>,
    /// Campi di località modificati (`None` = campo svuotato).
    pub locations: BTreeMap<(LocationRole, LocationField), Option<String>>,
}

impl GpsDraft {
    pub fn is_empty(&self) -> bool {
        self.coordinates.is_none() && self.locations.is_empty()
    }
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
    /// Le coordinate del marker sono state cambiate rispetto ai documenti.
    pub coordinates_dirty: bool,
    /// Bozze GPS per foto, conservate cambiando selezione.
    pub drafts: HashMap<PathBuf, GpsDraft>,
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
            coordinates_dirty: false,
            drafts: HashMap::new(),
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
        self.marker_mixed = false;
        self.manual_error = None;
        self.draft_dirty = true;
        self.coordinates_dirty = true;
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
                self.marker_mixed = false;
                self.manual_error = None;
                self.draft_dirty = true;
                self.coordinates_dirty = true;
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
                self.location_mixed.remove(&(role, field));
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
    /// Bozze non ancora salvate, conservate cambiando foto.
    pub drafts: HashMap<PathBuf, EditRecipe>,
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
            drafts: HashMap::new(),
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
        let bozze = std::mem::take(&mut self.drafts);
        *self = Self::default();
        self.presets = preset;
        self.preset_name = nome;
        self.save_error = errore;
        self.drafts = bozze;
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
        let bozze = std::mem::take(&mut self.drafts);
        *self = Self::default();
        self.presets = preset;
        self.preset_name = nome;
        self.save_error = errore;
        self.drafts = bozze;
    }

    /// Registra lo stato attuale per l'annullamento e invalida il redo.
    pub fn push_undo(&mut self) {
        self.undo.push(self.recipe);
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn undo(&mut self) -> bool {
        let Some(precedente) = self.undo.pop() else {
            return false;
        };
        self.redo.push(self.recipe);
        self.recipe = precedente;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(successiva) = self.redo.pop() else {
            return false;
        };
        self.undo.push(self.recipe);
        self.recipe = successiva;
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

    /// Ricetta in bozza da usare per una foto (quella aperta ha precedenza).
    pub fn recipe_for(&self, path: &Path) -> EditRecipe {
        if self.path.as_deref() == Some(path) {
            self.recipe
        } else {
            self.drafts.get(path).copied().unwrap_or_default()
        }
    }

    /// Ricorda la ricetta corrente come bozza in memoria, senza scrivere nel
    /// catalogo: il salvataggio su file resta un gesto esplicito.
    pub fn remember_draft(&mut self) {
        let Some(path) = self.path.clone() else {
            return;
        };
        if self.recipe == self.saved {
            self.drafts.remove(&path);
        } else {
            self.drafts.insert(path, self.recipe);
        }
    }

    /// Dimentica la bozza di una foto dopo che è stata salvata su file.
    pub fn forget_draft(&mut self, path: &Path) {
        self.drafts.remove(path);
    }

    /// Sposta la bozza quando il file viene spostato o rinominato.
    pub fn remap_draft(&mut self, from: &Path, to: &Path) {
        if let Some(recipe) = self.drafts.remove(from) {
            self.drafts.insert(to.to_path_buf(), recipe);
        }
    }

    /// Sposta tutte le bozze contenute in una cartella rinominata.
    pub fn remap_drafts_in_dir(&mut self, from: &Path, to: &Path) {
        let spostate: Vec<(PathBuf, PathBuf)> = self
            .drafts
            .keys()
            .filter_map(|path| {
                path.strip_prefix(from)
                    .ok()
                    .map(|relative| (path.clone(), to.join(relative)))
            })
            .collect();
        for (vecchio, nuovo) in spostate {
            if let Some(recipe) = self.drafts.remove(&vecchio) {
                self.drafts.insert(nuovo, recipe);
            }
        }
    }
}

/// Finestra di conferma del salvataggio manuale.
#[derive(Debug, Clone)]
pub struct SaveDialog {
    pub mode: SaveMode,
    pub overwrite_confirmed: bool,
    pub busy: bool,
    pub error: Option<String>,
    /// Foto rimaste da salvare (dopo un eventuale errore parziale).
    pub targets: Vec<PathBuf>,
    /// Foto già salvate in questo tentativo (per i messaggi di riepilogo).
    pub completed: BTreeSet<PathBuf>,
    /// Quante nuove copie sono state create in questo tentativo.
    pub created: usize,
    /// Ultimo errore ricevuto dal servizio, mostrato nel dialogo.
    pub failure: Option<String>,
}

impl Default for SaveDialog {
    fn default() -> Self {
        Self {
            mode: SaveMode::NewCopy,
            overwrite_confirmed: false,
            busy: false,
            error: None,
            targets: Vec::new(),
            completed: BTreeSet::new(),
            created: 0,
            failure: None,
        }
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
    /// Dialogo di salvataggio manuale (colore + metadati + posizione).
    pub save_dialog: Option<SaveDialog>,
    /// Foto da selezionare al termine della prossima scansione.
    pub pending_select: Option<PathBuf>,
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
            save_dialog: None,
            pending_select: None,
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

    /// Vero se c'è almeno una modifica in bozza da salvare.
    pub fn has_pending_changes(&self) -> bool {
        !self.save_targets().is_empty()
    }

    /// Vero se esistono modifiche non salvate, anche fuori dalla selezione.
    pub fn has_unsaved_changes(&self) -> bool {
        !self.edit.drafts.is_empty()
            || self.meta.drafts.values().any(|draft| !draft.is_empty())
            || self.gps.drafts.values().any(|draft| !draft.is_empty())
            || self.gps.manual_error.is_some()
    }

    /// Foto coinvolte dal salvataggio: quelle con colore, metadati o posizione
    /// in bozza. Le modifiche condivise vengono registrate su tutta la
    /// selezione al momento dell'edit, quindi restano per-foto.
    pub fn save_targets(&self) -> Vec<PathBuf> {
        self.selected_paths()
            .into_iter()
            .filter(|path| self.path_pending(path))
            .collect()
    }

    pub fn path_pending(&self, path: &Path) -> bool {
        self.recipe_pending(path) || self.meta_pending(path) || self.gps_pending(path)
    }

    /// Tutte le foto con almeno una bozza, dentro o fuori la selezione.
    pub fn pending_paths(&self) -> BTreeSet<PathBuf> {
        let mut paths: BTreeSet<PathBuf> = self
            .edit
            .drafts
            .keys()
            .chain(self.meta.drafts.keys())
            .chain(self.gps.drafts.keys())
            .cloned()
            .collect();
        if let Some(path) = &self.edit.path {
            if self.edit.is_dirty() {
                paths.insert(path.clone());
            }
        }
        paths.retain(|path| self.path_pending(path));
        paths
    }

    /// Vero se la foto ha una ricetta in bozza non ancora salvata.
    pub fn recipe_pending(&self, path: &Path) -> bool {
        if self.edit.path.as_deref() == Some(path) {
            self.edit.is_dirty()
        } else {
            self.edit
                .drafts
                .get(path)
                .is_some_and(|recipe| !recipe.is_identity())
        }
    }

    pub fn meta_pending(&self, path: &Path) -> bool {
        self.meta
            .drafts
            .get(path)
            .is_some_and(|draft| !draft.is_empty())
    }

    pub fn gps_pending(&self, path: &Path) -> bool {
        self.gps
            .drafts
            .get(path)
            .is_some_and(|draft| !draft.is_empty())
    }

    /// Modifiche metadati in bozza di una singola foto.
    pub fn metadata_edits(&self, path: &Path) -> Vec<MetadataEdit> {
        let Some(draft) = self.meta.drafts.get(path) else {
            return Vec::new();
        };
        draft
            .iter()
            .map(|(field, value)| {
                let value = value
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned);
                MetadataEdit::Field(*field, value)
            })
            .collect()
    }

    /// Modifiche GPS in bozza di una singola foto.
    pub fn gps_edits(&self, path: &Path) -> Vec<MetadataEdit> {
        let Some(draft) = self.gps.drafts.get(path) else {
            return Vec::new();
        };
        let mut edits = Vec::new();
        if let Some(coordinates) = draft.coordinates {
            edits.push(MetadataEdit::Gps(coordinates));
        }
        for ((role, field), value) in &draft.locations {
            let value = value
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned);
            edits.push(MetadataEdit::Location {
                role: *role,
                field: *field,
                value,
            });
        }
        edits
    }

    /// Registra il valore di un campo metadati su tutta la selezione.
    pub fn set_meta_field(&mut self, field: MetadataField, value: Option<String>) {
        let value = value.unwrap_or_default();
        for path in self.selected_paths() {
            self.meta
                .drafts
                .entry(path)
                .or_default()
                .insert(field, Some(value.clone()));
        }
        self.meta.form_values.insert(field, value);
        self.meta.form_mixed.remove(&field);
        self.meta.form_dirty.insert(field);
    }

    /// Scarta le modifiche metadati della selezione corrente.
    pub fn discard_meta_drafts(&mut self) {
        for path in self.selected_paths() {
            self.meta.drafts.remove(&path);
        }
        self.rebuild_meta_form();
    }

    /// Registra il marker GPS su tutta la selezione.
    pub fn set_gps_marker(&mut self, marker: Option<GpsLocation>) {
        self.gps.set_marker(marker);
        for path in self.selected_paths() {
            self.gps.drafts.entry(path).or_default().coordinates = Some(marker);
        }
    }

    /// Riallinea i campi manuali e, se validi, registra la bozza su tutta la
    /// selezione.
    pub fn apply_gps_manual_fields(&mut self) {
        self.gps.apply_manual_fields();
        if self.gps.manual_error.is_none() {
            let marker = self.gps.marker;
            for path in self.selected_paths() {
                self.gps.drafts.entry(path).or_default().coordinates = Some(marker);
            }
        }
    }

    /// Registra un campo di località su tutta la selezione.
    pub fn set_gps_location_field(
        &mut self,
        role: LocationRole,
        field: LocationField,
        value: String,
    ) {
        for path in self.selected_paths() {
            self.gps
                .drafts
                .entry(path)
                .or_default()
                .locations
                .insert((role, field), Some(value.clone()));
        }
        self.gps.location_values.insert((role, field), value);
        self.gps.location_mixed.remove(&(role, field));
        self.gps.location_dirty.insert((role, field));
        self.gps.draft_dirty = true;
    }

    /// Applica un suggerimento geografico alla località di tutta la selezione.
    pub fn fill_gps_location(&mut self, role: LocationRole, suggestion: &LocationSuggestion) {
        let coppie: [(LocationField, &Option<String>); 6] = [
            (LocationField::Place, &suggestion.place),
            (LocationField::Sublocation, &suggestion.sublocation),
            (LocationField::City, &suggestion.city),
            (LocationField::State, &suggestion.state),
            (LocationField::Country, &suggestion.country),
            (LocationField::CountryCode, &suggestion.country_code),
        ];
        for path in self.selected_paths() {
            let draft = self.gps.drafts.entry(path).or_default();
            for (field, value) in coppie {
                if let Some(value) = value {
                    draft.locations.insert((role, field), Some(value.clone()));
                }
            }
        }
        self.gps.fill_location(role, suggestion);
    }

    /// Scarta le modifiche GPS della selezione corrente.
    pub fn discard_gps_drafts(&mut self) {
        for path in self.selected_paths() {
            self.gps.drafts.remove(&path);
        }
        self.rebuild_gps_form();
    }

    /// Sposta le bozze metadati/GPS quando un file viene rinominato o spostato.
    pub fn remap_photo_drafts(&mut self, from: &Path, to: &Path) {
        if let Some(draft) = self.meta.drafts.remove(from) {
            self.meta.drafts.insert(to.to_path_buf(), draft);
        }
        if let Some(draft) = self.gps.drafts.remove(from) {
            self.gps.drafts.insert(to.to_path_buf(), draft);
        }
    }

    /// Sposta le bozze metadati/GPS contenute in una cartella rinominata.
    pub fn remap_photo_drafts_in_dir(&mut self, from: &Path, to: &Path) {
        let meta: Vec<(PathBuf, PathBuf)> = self
            .meta
            .drafts
            .keys()
            .filter_map(|path| {
                path.strip_prefix(from)
                    .ok()
                    .map(|relative| (path.clone(), to.join(relative)))
            })
            .collect();
        for (vecchio, nuovo) in meta {
            if let Some(draft) = self.meta.drafts.remove(&vecchio) {
                self.meta.drafts.insert(nuovo, draft);
            }
        }
        let gps: Vec<(PathBuf, PathBuf)> = self
            .gps
            .drafts
            .keys()
            .filter_map(|path| {
                path.strip_prefix(from)
                    .ok()
                    .map(|relative| (path.clone(), to.join(relative)))
            })
            .collect();
        for (vecchio, nuovo) in gps {
            if let Some(draft) = self.gps.drafts.remove(&vecchio) {
                self.gps.drafts.insert(nuovo, draft);
            }
        }
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
        self.meta.form_values.clear();
        self.meta.form_mixed.clear();
        self.meta.form_dirty.clear();
        if paths.is_empty() || paths.iter().any(|path| !self.meta.docs.contains_key(path)) {
            // Nessun documento pronto: il form resta vuoto e `form_paths`
            // diverso dalla selezione fa ritentare la ricostruzione.
            self.meta.form_paths.clear();
            return;
        }
        self.meta.form_paths = paths.clone();
        for field in MetadataField::ALL {
            let mut valori = Vec::with_capacity(paths.len());
            let mut in_bozza = false;
            for path in &paths {
                let document = &self.meta.docs[path];
                let valore = match self
                    .meta
                    .drafts
                    .get(path)
                    .and_then(|draft| draft.get(&field))
                {
                    Some(value) => {
                        in_bozza = true;
                        value.clone().unwrap_or_default()
                    }
                    None => document.display(field),
                };
                valori.push(valore);
            }
            let primo = valori.first().cloned().unwrap_or_default();
            if valori.iter().all(|valore| *valore == primo) {
                if !primo.is_empty() {
                    self.meta.form_values.insert(field, primo);
                }
            } else {
                self.meta.form_mixed.insert(field);
            }
            if in_bozza {
                self.meta.form_dirty.insert(field);
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
        self.gps.coordinates_dirty = false;
        if paths.is_empty() || paths.iter().any(|path| !self.meta.docs.contains_key(path)) {
            // Senza tutti i documenti la bozza non è attendibile: resta vuota
            // e viene ricostruita appena la selezione è pronta.
            self.gps.form_paths.clear();
            return;
        }
        self.gps.form_paths = paths.clone();

        let mut posizioni: Vec<Option<GpsLocation>> = Vec::with_capacity(paths.len());
        for path in &paths {
            let document = &self.meta.docs[path];
            let coordinate = match self
                .gps
                .drafts
                .get(path)
                .and_then(|draft| draft.coordinates)
            {
                Some(coordinate) => coordinate,
                None => document.gps.map(|gps| GpsLocation {
                    latitude: gps.latitude,
                    longitude: gps.longitude,
                    altitude: gps.altitude,
                }),
            };
            posizioni.push(coordinate);
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
                let mut in_bozza = false;
                for path in &paths {
                    let document = &self.meta.docs[path];
                    let valore = match self
                        .gps
                        .drafts
                        .get(path)
                        .and_then(|draft| draft.locations.get(&(role, field)))
                    {
                        Some(value) => {
                            in_bozza = true;
                            value.clone().unwrap_or_default()
                        }
                        None => document.location(role).display(field),
                    };
                    valori.push(valore);
                }
                let primo = valori.first().cloned().unwrap_or_default();
                if valori.iter().all(|valore| *valore == primo) {
                    if !primo.is_empty() {
                        self.gps.location_values.insert((role, field), primo);
                    }
                } else {
                    self.gps.location_mixed.insert((role, field));
                }
                if in_bozza {
                    self.gps.location_dirty.insert((role, field));
                }
            }
        }

        self.gps.coordinates_dirty = paths.iter().any(|path| {
            self.gps
                .drafts
                .get(path)
                .is_some_and(|draft| draft.coordinates.is_some())
        });
        self.gps.draft_dirty = paths.iter().any(|path| {
            self.gps
                .drafts
                .get(path)
                .is_some_and(|draft| !draft.is_empty())
        });
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
    /// Apre il dialogo di salvataggio manuale.
    pub save_open: bool,
    /// Conferma il salvataggio nella modalità scelta.
    pub save_confirm: Option<SaveMode>,
    /// Chiude il dialogo di salvataggio.
    pub save_cancel: bool,
    /// Rilegge i metadati della selezione.
    pub metadata_reload: bool,
    /// Imposta la valutazione (0–5 stelle) sulle foto indicate.
    pub metadata_rate: Option<(Vec<PathBuf>, u8)>,
    /// Rimuove le coordinate GPS dalla selezione (in bozza).
    pub gps_remove: bool,
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
    fn le_bozze_colore_sopravvivono_al_cambio_foto() {
        let mut edit = EditState::default();
        edit.carica(Path::new("/foto/a.jpg"), None, EditRecipe::default());
        edit.apply_preset(EditRecipe {
            exposure: 2.0,
            ..EditRecipe::default()
        });
        edit.remember_draft();
        let bozza = edit.recipe;

        edit.carica(Path::new("/foto/b.jpg"), None, EditRecipe::default());
        assert_eq!(edit.recipe_for(Path::new("/foto/a.jpg")), bozza);
        assert_eq!(
            edit.recipe_for(Path::new("/foto/b.jpg")),
            EditRecipe::default()
        );
    }

    #[test]
    fn le_bozze_seguono_file_e_cartelle_rinominati() {
        let mut edit = EditState::default();
        let bozza = EditRecipe {
            contrast: 12.0,
            ..EditRecipe::default()
        };
        edit.drafts.insert(PathBuf::from("/foto/a.jpg"), bozza);
        edit.remap_draft(Path::new("/foto/a.jpg"), Path::new("/foto/b.jpg"));
        assert_eq!(edit.drafts.get(Path::new("/foto/b.jpg")), Some(&bozza));

        edit.remap_drafts_in_dir(Path::new("/foto"), Path::new("/archivio"));
        assert_eq!(edit.drafts.get(Path::new("/archivio/b.jpg")), Some(&bozza));
    }

    #[test]
    fn il_salvataggio_include_solo_le_foto_con_bozze() {
        let mut state = AppState::new(PersistedState::default());
        state.entries = vec![asset("/foto/a.jpg"), asset("/foto/b.jpg")];
        let a = PathBuf::from("/foto/a.jpg");
        let b = PathBuf::from("/foto/b.jpg");
        state.selection.insert(a.clone());
        state
            .edit
            .carica(Path::new("/foto/a.jpg"), None, EditRecipe::default());
        state.edit.apply_preset(EditRecipe {
            gamma: 1.2,
            ..EditRecipe::default()
        });
        state.edit.remember_draft();
        state.set_meta_field(MetadataField::Title, Some("Titolo".to_owned()));
        assert_eq!(state.save_targets(), vec![a.clone()]);

        state.selection.insert(b.clone());
        assert_eq!(state.save_targets(), vec![a.clone()]);

        state.set_meta_field(MetadataField::Title, Some("Insieme".to_owned()));
        assert_eq!(state.save_targets(), vec![a, b]);
    }

    #[test]
    fn le_bozze_metadati_e_gps_sopravvivono_al_cambio_selezione() {
        let mut state = AppState::new(PersistedState::default());
        let a = PathBuf::from("/foto/a.jpg");
        let b = PathBuf::from("/foto/b.jpg");
        state.entries = vec![asset("/foto/a.jpg"), asset("/foto/b.jpg")];
        state
            .meta
            .docs
            .insert(a.clone(), MetadataDocument::default());
        state
            .meta
            .docs
            .insert(b.clone(), MetadataDocument::default());

        state.selection.insert(a.clone());
        state.rebuild_meta_form();
        state.rebuild_gps_form();
        state.set_meta_field(MetadataField::Title, Some("Titolo".to_owned()));
        let posizione = GpsLocation {
            latitude: 45.0,
            longitude: 9.0,
            altitude: None,
        };
        state.set_gps_marker(Some(posizione));
        state.set_gps_location_field(
            LocationRole::Created,
            LocationField::City,
            "Milano".to_owned(),
        );

        state.selection.clear();
        state.selection.insert(b.clone());
        state.rebuild_meta_form();
        state.rebuild_gps_form();
        assert!(state.metadata_edits(&b).is_empty());
        assert!(state.gps_edits(&b).is_empty());

        state.selection.clear();
        state.selection.insert(a.clone());
        state.rebuild_meta_form();
        state.rebuild_gps_form();
        assert_eq!(
            state.meta.form_values.get(&MetadataField::Title),
            Some(&"Titolo".to_owned())
        );
        assert!(state.meta.form_dirty.contains(&MetadataField::Title));
        assert_eq!(state.gps.marker, Some(posizione));
        assert_eq!(
            state
                .gps
                .location_value(LocationRole::Created, LocationField::City),
            "Milano"
        );
        assert_eq!(state.metadata_edits(&a).len(), 1);
        assert_eq!(state.gps_edits(&a).len(), 2);
    }

    #[test]
    fn le_bozze_metadati_e_gps_seguono_i_file_rinominati() {
        let mut state = AppState::new(PersistedState::default());
        let a = PathBuf::from("/foto/a.jpg");
        let b = PathBuf::from("/foto/b.jpg");
        state
            .meta
            .drafts
            .entry(a.clone())
            .or_default()
            .insert(MetadataField::Title, Some("Titolo".to_owned()));
        state.gps.drafts.insert(
            a.clone(),
            GpsDraft {
                coordinates: Some(Some(GpsLocation {
                    latitude: 45.0,
                    longitude: 9.0,
                    altitude: None,
                })),
                ..GpsDraft::default()
            },
        );
        state.remap_photo_drafts(&a, &b);
        assert!(state.meta_pending(&b));
        assert!(state.gps_pending(&b));
        assert!(!state.meta_pending(&a));
        assert!(!state.gps_pending(&a));

        state.remap_photo_drafts_in_dir(Path::new("/foto"), Path::new("/archivio"));
        assert!(state.meta_pending(Path::new("/archivio/b.jpg")));
        assert!(state.gps_pending(Path::new("/archivio/b.jpg")));
    }

    #[test]
    fn le_modifiche_fuori_selezione_contano_come_non_salvate() {
        let mut state = AppState::new(PersistedState::default());
        assert!(!state.has_unsaved_changes());

        state.edit.drafts.insert(
            PathBuf::from("/foto/a.jpg"),
            EditRecipe {
                exposure: 1.0,
                ..EditRecipe::default()
            },
        );
        assert!(state.has_unsaved_changes());

        state.edit.drafts.clear();
        state.gps.drafts.insert(
            PathBuf::from("/foto/b.jpg"),
            GpsDraft {
                coordinates: Some(Some(GpsLocation {
                    latitude: 45.0,
                    longitude: 9.0,
                    altitude: None,
                })),
                ..GpsDraft::default()
            },
        );
        assert!(state.has_unsaved_changes());
    }

    #[test]
    fn il_form_attende_i_documenti_prima_di_ricostruirsi() {
        let mut state = AppState::new(PersistedState::default());
        let path = PathBuf::from("/foto/a.jpg");
        state.entries = vec![asset("/foto/a.jpg")];
        state.selection.insert(path.clone());

        state.rebuild_meta_form();
        state.rebuild_gps_form();
        assert!(state.meta.form_paths.is_empty());
        assert!(state.gps.form_paths.is_empty());

        state
            .meta
            .docs
            .insert(path.clone(), MetadataDocument::default());
        state.rebuild_meta_form();
        state.rebuild_gps_form();
        assert_eq!(state.meta.form_paths, vec![path.clone()]);
        assert_eq!(state.gps.form_paths, vec![path]);
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
