//! Applicazione e orchestrazione dei pannelli.
//!
//! Ciclo del frame:
//! 1. risveglio delle cache (LRU miniature);
//! 2. raccolta dei risultati di scansione, decodifica e operazioni su file;
//! 3. disegno dei pannelli e dei prompt, che raccolgono [`UiActions`];
//! 4. esecuzione delle azioni (navigazione, operazioni, annullamento).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use eframe::egui;
use uberphoto_core::GpsLocation;
use uberphoto_db::{Index, MetadataIndex};
use uberphoto_export::{ExportItem, ExportJob, ExportService};
use uberphoto_files::{
    create_dir, delete_empty_dir, dir_item_count, rename_dir, ConflictPolicy, DirNode, DirWatcher,
    DirectoryScanner, FileOpRequest, FileOpsService,
};
use uberphoto_geo::{GeoOutcome, GeoRequest, GeoService, NominatimClient};
use uberphoto_image::{EditRecipe, EditService, EditSource, ImageKind, ImageService};
use uberphoto_metadata::{
    exiftool_versione, LocationRole, MetadataEdit, MetadataField, MetadataOutcome, MetadataRequest,
    MetadataService, WriteItem,
};

use crate::browser;
use crate::explorer;
use crate::inspector;
use crate::preview::PreviewState;
use crate::state::{
    AppState, ConflictPrompt, DeleteDirPrompt, NewFolderPrompt, OpStatus, PersistedState,
    RenamePrompt, RenameTarget, SuggestionTarget, UiActions,
};
use crate::thumbnails::ThumbCache;
use crate::util::elide;

const ERROR_COLOR: egui::Color32 = egui::Color32::from_rgb(224, 122, 118);

pub struct UberPhotoApp {
    state: AppState,
    /// Indice SQLite locale; assente se la cartella dati non è disponibile.
    index: Option<Index>,
    scanner: DirectoryScanner,
    images: ImageService,
    thumbs: ThumbCache,
    preview: PreviewState,
    ops: FileOpsService,
    /// Lettura e scrittura dei metadati su thread dedicato.
    meta: MetadataService,
    /// Ricerche geografiche (Nominatim) su thread dedicato.
    geo: GeoService,
    /// Elaborazione delle anteprime modificate (ricette di editing).
    edit: EditService,
    /// Esportazione for web su thread dedicato.
    export: ExportService,
    /// Osservatore della cartella corrente per le modifiche esterne.
    watcher: Option<DirWatcher>,
    /// Chiusura automatica dopo N secondi, usata dagli smoke test di CI
    /// (variabile d'ambiente `UBERPHOTO_SMOKE_EXIT_SECS`).
    smoke_exit: Option<(std::time::Instant, f64)>,
}

impl UberPhotoApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        setup_visuals(&cc.egui_ctx);

        let persisted: PersistedState = cc
            .storage
            .and_then(|storage| eframe::get_value(storage, eframe::APP_KEY))
            .unwrap_or_default();
        let last_dir = persisted.last_dir.clone();

        let repaint_ctx = cc.egui_ctx.clone();
        let repaint: Arc<dyn Fn() + Send + Sync> = Arc::new(move || repaint_ctx.request_repaint());

        let smoke_exit = std::env::var("UBERPHOTO_SMOKE_EXIT_SECS")
            .ok()
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|seconds| *seconds > 0.0)
            .map(|seconds| (std::time::Instant::now(), seconds));

        let mut app = Self {
            state: AppState::new(persisted),
            index: open_index(),
            scanner: DirectoryScanner::new(repaint.clone()),
            images: ImageService::new(repaint.clone()),
            thumbs: ThumbCache::new(),
            preview: PreviewState::new(),
            ops: FileOpsService::new(repaint.clone()),
            meta: MetadataService::new(repaint.clone()),
            geo: GeoService::new(geo_client(), repaint.clone()),
            edit: EditService::new(repaint.clone()),
            export: ExportService::new(repaint.clone()),
            watcher: DirWatcher::new(repaint),
            smoke_exit,
        };
        app.state.logo = load_logo(&cc.egui_ctx);
        app.state.exiftool = exiftool_versione();
        if app.state.exiftool.is_none() {
            tracing::warn!("ExifTool non trovato: l'editor metadati resterà disabilitato");
        }
        if let Some(index) = &app.index {
            match index.load_presets() {
                Ok(righe) => {
                    app.state.edit.presets = righe
                        .into_iter()
                        .filter_map(|(nome, json)| {
                            EditRecipe::from_json(&json)
                                .ok()
                                .map(|recipe| (nome, recipe.sanitized()))
                        })
                        .collect();
                }
                Err(err) => tracing::warn!("lettura dei preset non riuscita: {err:#}"),
            }
        }

        if let Some(dir) = last_dir {
            if dir.is_dir() {
                app.open_root(dir);
            }
        }

        app
    }

    /// Apre `dir` come nuova radice dell'esploratore e la scansiona.
    fn open_root(&mut self, dir: PathBuf) {
        let mut root = DirNode::new_root(dir.clone());
        if let Some(message) = root.load_children() {
            self.state.error = Some(message);
        }
        self.state.root = Some(root);
        self.state.push_recent(&dir);
        self.start_scan(dir);
    }

    /// Naviga il browser su `dir` (senza toccare la radice).
    fn start_scan(&mut self, dir: PathBuf) {
        self.state.current_dir = Some(dir.clone());
        self.state.loading_dir = Some(dir.clone());
        self.state.entries.clear();
        self.state.index_rows.clear();
        self.state.selection.clear();
        self.state.selection_anchor = None;
        if let Some(watcher) = &mut self.watcher {
            watcher.watch(&dir);
        }
        self.scanner.request(&dir);
        tracing::info!(dir = %dir.display(), "scansione avviata");
    }

    fn pump_scan_results(&mut self) {
        while let Some(result) = self.scanner.try_recv() {
            if self.state.current_dir.as_deref() != Some(result.dir.as_path()) {
                continue; // risultato superato da una navigazione più recente
            }
            self.state.loading_dir = None;
            match result.error {
                Some(error) => {
                    tracing::warn!(dir = %result.dir.display(), "scansione fallita: {error}");
                    self.state.error = Some(error);
                    self.state.entries.clear();
                }
                None => {
                    self.state.entries = result.entries;
                    self.refresh_index_rows(&result.dir);
                    self.state.apply_sort();
                    self.request_metadata_summary();
                }
            }
        }
    }

    /// Allinea l'indice SQLite alla cartella appena scansionata e ne carica
    /// le righe nello stato per ordinamenti e filtri.
    fn refresh_index_rows(&mut self, dir: &Path) {
        let Some(index) = &self.index else { return };
        for entry in &self.state.entries {
            if let Err(err) = index.upsert_file_info(entry) {
                tracing::warn!("scrittura indice non riuscita: {err:#}");
            }
        }
        match index.load_dir(dir) {
            Ok(rows) => {
                self.state.index_rows = rows
                    .into_iter()
                    .map(|row| (row.path.clone(), row))
                    .collect();
            }
            Err(err) => tracing::warn!("lettura indice non riuscita: {err:#}"),
        }
    }

    /// mtime in secondi Unix, la stessa unità usata dall'indice SQLite.
    fn mtime_secs(&self, path: &Path) -> Option<i64> {
        self.state
            .entry_mtime(path)
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs() as i64)
    }

    /// Chiede il riassunto dei metadati per le foto della cartella corrente
    /// che non sono ancora state indicizzate per la loro versione su disco.
    fn request_metadata_summary(&mut self) {
        if self.state.exiftool.is_none() {
            return;
        }
        self.state
            .meta
            .summarized
            .retain(|path, _| self.state.entries.iter().any(|entry| &entry.path == path));

        let mut richieste = Vec::new();
        for entry in &self.state.entries {
            let Some(mtime) = entry
                .modified
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs() as i64)
            else {
                continue;
            };
            let aggiornato = self.state.meta.summarized.get(&entry.path) == Some(&mtime);
            let in_volo = self.state.meta.pending_summary.get(&entry.path) == Some(&mtime);
            if !aggiornato && !in_volo {
                self.state
                    .meta
                    .pending_summary
                    .insert(entry.path.clone(), mtime);
                richieste.push(entry.path.clone());
            }
        }
        if !richieste.is_empty() {
            self.meta
                .request(MetadataRequest::ReadSummary { paths: richieste });
        }
    }

    /// Ricarica le righe indice della cartella corrente (dopo una scrittura).
    fn reload_index_rows(&mut self) {
        let (Some(index), Some(dir)) = (&self.index, self.state.current_dir.clone()) else {
            return;
        };
        match index.load_dir(&dir) {
            Ok(rows) => {
                self.state.index_rows = rows
                    .into_iter()
                    .map(|row| (row.path.clone(), row))
                    .collect();
            }
            Err(err) => tracing::warn!("lettura indice non riuscita: {err:#}"),
        }
    }

    /// Raccoglie gli esiti del motore metadati: aggiorna indice, documenti,
    /// form e barra di stato.
    fn pump_metadata_results(&mut self) {
        while let Some(outcome) = self.meta.try_recv() {
            match outcome {
                MetadataOutcome::Summary { items } => {
                    let mut index_updated = false;
                    for (path, summary) in items {
                        let mtime =
                            self.state.meta.pending_summary.remove(&path).or_else(|| {
                                self.state.index_rows.get(&path).and_then(|row| row.mtime)
                            });
                        if let Some(mtime) = mtime {
                            self.state.meta.summarized.insert(path.clone(), mtime);
                        }
                        let Some(index) = &self.index else { continue };
                        let metadata = MetadataIndex {
                            has_gps: summary.has_gps,
                            rating: summary.rating,
                            date_original: summary.date_original,
                            keywords: summary.keywords,
                            camera_make: summary.camera_make,
                            camera_model: summary.camera_model,
                            lens: summary.lens,
                            iso: summary.iso,
                            f_number: summary.f_number,
                            exposure_time: summary.exposure_time,
                            focal_length: summary.focal_length,
                            gps_lat: summary.gps_lat,
                            gps_lon: summary.gps_lon,
                        };
                        match index.upsert_metadata(&path, mtime, &metadata) {
                            Ok(()) => index_updated = true,
                            Err(err) => tracing::warn!("scrittura indice non riuscita: {err:#}"),
                        }
                    }
                    if index_updated {
                        self.reload_index_rows();
                        self.state.apply_sort();
                    }
                }
                MetadataOutcome::Full { items } => {
                    for (path, result) in items {
                        self.state.meta.loading.remove(&path);
                        match result {
                            Ok(full) => {
                                self.state.meta.errors.remove(&path);
                                self.state.meta.raw.insert(path.clone(), full.raw);
                                self.state.meta.docs.insert(path.clone(), full.document);
                                match self.mtime_secs(&path) {
                                    Some(mtime) => {
                                        self.state.meta.loaded.insert(path, mtime);
                                    }
                                    None => {
                                        self.state.meta.loaded.remove(&path);
                                    }
                                }
                            }
                            Err(error) => {
                                self.state.meta.docs.remove(&path);
                                self.state.meta.raw.remove(&path);
                                self.state.meta.loaded.remove(&path);
                                self.state.meta.errors.insert(path, error);
                            }
                        }
                    }
                    if self.state.meta.form_paths == self.state.selected_paths()
                        && self.state.meta.form_dirty.is_empty()
                    {
                        self.state.rebuild_meta_form();
                    }
                    if self.state.gps.form_paths == self.state.selected_paths()
                        && !self.state.gps.draft_dirty
                    {
                        self.state.rebuild_gps_form();
                    }
                }
                MetadataOutcome::Written { report } => {
                    self.state.op_status = Some(OpStatus {
                        message: report.summary(),
                        error: report.first_error(),
                        undo: None,
                    });
                    self.state.meta.form_dirty.clear();
                    if report.failed.is_empty() {
                        self.state.gps.location_dirty.clear();
                        self.state.gps.draft_dirty = false;
                    }
                    if report.updated.is_empty() {
                        continue;
                    }
                    let selected = self.state.selected_paths();
                    let da_rileggere: Vec<PathBuf> = report
                        .updated
                        .iter()
                        .filter(|path| selected.contains(path))
                        .cloned()
                        .collect();
                    for path in &report.updated {
                        self.state.meta.summarized.remove(path);
                        if let Some(mtime) = self.mtime_secs(path) {
                            self.state.meta.pending_summary.insert(path.clone(), mtime);
                        }
                    }
                    self.meta.request(MetadataRequest::ReadSummary {
                        paths: report.updated,
                    });
                    if !da_rileggere.is_empty() {
                        for path in &da_rileggere {
                            self.state.meta.loading.insert(path.clone());
                        }
                        self.meta.request(MetadataRequest::ReadFull {
                            paths: da_rileggere,
                        });
                    }
                }
            }
        }
    }

    /// Raccoglie gli esiti delle ricerche geografiche e aggiorna la scheda GPS.
    fn pump_geo_results(&mut self) {
        while let Some(outcome) = self.geo.try_recv() {
            match outcome {
                GeoOutcome::Search { query, result } => {
                    self.state.gps.searching = false;
                    match result {
                        Ok(results) => {
                            self.state.gps.geo_error = None;
                            self.state.gps.search_results = results;
                            tracing::info!(
                                query = %query,
                                risultati = self.state.gps.search_results.len(),
                                "ricerca geografica completata"
                            );
                        }
                        Err(error) => {
                            self.state.gps.search_results.clear();
                            self.state.gps.geo_error = Some(error);
                        }
                    }
                }
                GeoOutcome::Reverse { location, result } => {
                    self.state.gps.reverse_pending = false;
                    match result {
                        Ok(Some(suggestion)) => {
                            self.state.gps.geo_error = None;
                            if self.state.gps.marker.is_some() {
                                self.state.gps.fill_location(
                                    LocationRole::Created,
                                    &suggestion.location_suggestion(),
                                );
                                self.state.gps.reverse_note = Some(format!(
                                    "Località di scatto compilata da: {}",
                                    suggestion.display_name
                                ));
                            }
                        }
                        Ok(None) => {
                            self.state.gps.geo_error = Some(format!(
                                "Nessuna località trovata per {:.6}, {:.6}",
                                location.latitude, location.longitude
                            ));
                        }
                        Err(error) => self.state.gps.geo_error = Some(error),
                    }
                }
            }
        }
    }

    /// Allinea il form metadati alla selezione corrente e legge i documenti
    /// mancanti delle foto appena selezionate.
    fn sync_metadata(&mut self) {
        let selected = self.state.selected_paths();
        let mut da_ricaricare = Vec::new();
        for path in &selected {
            if let Some(loaded) = self.state.meta.loaded.get(path) {
                if Some(*loaded) != self.mtime_secs(path) {
                    da_ricaricare.push(path.clone());
                }
            }
        }
        for path in &da_ricaricare {
            self.state.meta.docs.remove(path);
            self.state.meta.raw.remove(path);
            self.state.meta.errors.remove(path);
            self.state.meta.loaded.remove(path);
        }
        if self.state.meta.form_paths != selected {
            self.state.rebuild_meta_form();
            self.state.meta.raw_filter.clear();
        } else if !da_ricaricare.is_empty() && self.state.meta.form_dirty.is_empty() {
            self.state.rebuild_meta_form();
        }
        if self.state.gps.form_paths != selected
            || (!da_ricaricare.is_empty() && !self.state.gps.draft_dirty)
        {
            self.state.rebuild_gps_form();
        }
        if self.state.exiftool.is_none() {
            return;
        }
        let da_leggere: Vec<PathBuf> = selected
            .iter()
            .filter(|path| {
                !self.state.meta.docs.contains_key(*path)
                    && !self.state.meta.loading.contains(*path)
                    && !self.state.meta.errors.contains_key(*path)
            })
            .cloned()
            .collect();
        if !da_leggere.is_empty() {
            for path in &da_leggere {
                self.state.meta.loading.insert(path.clone());
            }
            self.meta
                .request(MetadataRequest::ReadFull { paths: da_leggere });
        }
    }

    /// Allinea la scheda Colori alla selezione: carica la ricetta dal
    /// catalogo, richiede la sorgente decodificata e innesca l'elaborazione
    /// quando la ricetta attiva cambia.
    fn sync_edit(&mut self, ctx: &egui::Context) {
        let selected = self.state.selected_paths();
        let singola = match selected.as_slice() {
            [path] => Some(path.clone()),
            _ => None,
        };

        let mut nuova_foto = false;
        match &singola {
            Some(path) => {
                let mtime = self.state.entry_mtime(path);
                let corrente = self.state.edit.path.as_deref() == Some(path.as_path());
                if !corrente || self.state.edit.mtime != mtime {
                    let recipe = self
                        .index
                        .as_ref()
                        .and_then(|index| index.load_recipe(path).ok().flatten())
                        .and_then(|json| EditRecipe::from_json(&json).ok())
                        .map(|recipe| recipe.sanitized())
                        .unwrap_or_default();
                    self.state.edit.carica(path, mtime, recipe);
                    self.preview.clear_edited();
                    nuova_foto = true;
                }
            }
            None => {
                if self.state.edit.path.is_some() {
                    self.state.edit.scarica();
                    self.preview.clear_edited();
                }
            }
        }

        // La sorgente serve quando la scheda Colori è visibile o quando la
        // finestra di anteprima mostra proprio questa foto.
        let serve_sorgente = self.state.inspector_tab == crate::state::InspectorTab::Colori
            || singola.as_deref().is_some_and(|path| {
                let mtime = self.state.entry_mtime(path);
                self.preview.accepts(path, mtime)
            });
        if nuova_foto || serve_sorgente {
            if let Some(path) = &singola {
                if self.state.edit.source.is_none() && !self.state.edit.source_requested {
                    self.state.edit.source_requested = true;
                    let mtime = self.state.entry_mtime(path);
                    self.images.request_preview(path.clone(), mtime);
                }
            }
        }

        // Texture dell'originale, caricata dalla sorgente decodificata.
        if self.state.edit.original.is_none() {
            if let Some(source) = &self.state.edit.source {
                let image = &source.image;
                let color = egui::ColorImage::from_rgba_unmultiplied(
                    [image.width as usize, image.height as usize],
                    &image.rgba,
                );
                self.state.edit.original = Some(ctx.load_texture(
                    "uberphoto-edit-source",
                    color,
                    egui::TextureOptions::LINEAR,
                ));
                self.state.edit.original_size = egui::vec2(image.width as f32, image.height as f32);
            }
        }

        // Elaborazione: richiesta quando cambia la ricetta desiderata.
        let edit = &mut self.state.edit;
        let desiderata = (!edit.recipe.is_identity()).then_some(edit.recipe);
        if desiderata != edit.requested {
            match desiderata {
                Some(recipe) => {
                    if let Some(source) = &edit.source {
                        edit.generation += 1;
                        let generation = edit.generation;
                        let source = source.clone();
                        edit.requested = Some(recipe);
                        self.edit.request(generation, source, recipe);
                    }
                }
                None => {
                    edit.edited = None;
                    edit.edited_size = egui::Vec2::ZERO;
                    edit.requested = None;
                    self.preview.clear_edited();
                }
            }
        }
    }

    /// Raccoglie le anteprime modificate pronte e le pubblica alla scheda
    /// Colori e alla finestra di anteprima.
    fn pump_edit_results(&mut self, ctx: &egui::Context) {
        for result in self.edit.drain() {
            {
                let edit = &mut self.state.edit;
                if edit.requested.is_none() || result.generation != edit.generation {
                    continue; // richiesta superata
                }
                if edit.path.as_deref() != Some(result.path.as_path()) || edit.mtime != result.mtime
                {
                    continue; // il risultato è per un'altra foto
                }
                let pixels = &result.pixels;
                let color = egui::ColorImage::from_rgba_unmultiplied(
                    [pixels.width as usize, pixels.height as usize],
                    &pixels.rgba,
                );
                let texture = ctx.load_texture(
                    "uberphoto-edit-preview",
                    color,
                    egui::TextureOptions::LINEAR,
                );
                edit.edited_size = egui::vec2(pixels.width as f32, pixels.height as f32);
                edit.applied_generation = result.generation;
                edit.edited = Some(texture);
            }
            if self.preview.accepts(&result.path, result.mtime) {
                if let Some(texture) = &self.state.edit.edited {
                    self.preview
                        .set_edited(texture.clone(), self.state.edit.edited_size);
                }
            }
        }
        self.state.edit.busy = self.edit.busy();
    }

    /// Scrive nel catalogo la ricetta modificata (autosalvataggio distinto
    /// dal salvataggio dei metadati e dai pixel dell'originale).
    fn autosave_recipe(&mut self) {
        if !self.state.edit.pending_save {
            return;
        }
        let Some(path) = self.state.edit.path.clone() else {
            self.state.edit.pending_save = false;
            return;
        };
        let recipe = self.state.edit.recipe;
        let Some(index) = &self.index else {
            self.state.edit.pending_save = false;
            return;
        };
        let esito = if recipe.is_identity() {
            index.remove_recipe(&path)
        } else {
            index.save_recipe(&path, &recipe.to_json())
        };
        match esito {
            Ok(()) => {
                self.state.edit.saved = recipe;
                self.state.edit.pending_save = false;
                self.state.edit.save_error = None;
            }
            Err(err) => {
                self.state.edit.save_error = Some(format!("Ricetta non salvata: {err:#}"));
            }
        }
    }

    /// Raccoglie gli eventi del servizio di esportazione.
    fn pump_export_results(&mut self) {
        for evento in self.export.drain() {
            match evento {
                uberphoto_export::ExportEvent::Item(report) => {
                    self.state.export.registra_report(report);
                }
                uberphoto_export::ExportEvent::Failed { source, error } => {
                    self.state.export.registra_errore(source, error);
                }
                uberphoto_export::ExportEvent::Finished {
                    exported,
                    failed,
                    total,
                    cancelled,
                } => {
                    self.state.export.busy = false;
                    self.state.export.cancelled = cancelled;
                    let stato = if cancelled {
                        "interrotta"
                    } else {
                        "completata"
                    };
                    self.state.export.summary = Some(format!(
                        "Esportazione {stato}: {exported} riuscite, {failed} fallite su {total}."
                    ));
                }
            }
        }
        if !self.export.busy() {
            self.state.export.busy = false;
        }
    }

    /// Avvia l'esportazione della selezione con i parametri della scheda.
    fn start_export(&mut self) {
        let selezionati: Vec<(PathBuf, uberphoto_core::FormatKind)> = self
            .state
            .selected_assets()
            .into_iter()
            .map(|asset| (asset.path.clone(), asset.format))
            .collect();
        if selezionati.is_empty() {
            self.state.export.error = Some("Seleziona almeno una foto da esportare.".to_owned());
            return;
        }
        let items: Vec<ExportItem> = selezionati
            .into_iter()
            .map(|(path, kind)| {
                let recipe = if self.state.edit.path.as_deref() == Some(path.as_path()) {
                    self.state.edit.recipe
                } else {
                    self.index
                        .as_ref()
                        .and_then(|index| index.load_recipe(&path).ok().flatten())
                        .and_then(|json| EditRecipe::from_json(&json).ok())
                        .map(|recipe| recipe.sanitized())
                        .unwrap_or_default()
                };
                ExportItem { path, kind, recipe }
            })
            .collect();
        let totale = items.len();
        let job = ExportJob {
            settings: self.state.export.settings.clone(),
            items,
        };
        self.state.export.prepara(totale);
        if !self.export.start(job) {
            self.state.export.busy = false;
            self.state.export.error = Some("Un'esportazione è già in corso.".to_owned());
        }
    }

    /// Scrive sulla selezione i soli campi modificati nel form.
    fn save_metadata(&mut self) {
        if self.state.meta.form_dirty.is_empty() {
            return;
        }
        let edits: Vec<MetadataEdit> = self
            .state
            .meta
            .form_dirty
            .iter()
            .map(|field| {
                let value = self
                    .state
                    .meta
                    .form_values
                    .get(field)
                    .cloned()
                    .unwrap_or_default();
                let value = value.trim();
                MetadataEdit::Field(*field, (!value.is_empty()).then(|| value.to_owned()))
            })
            .collect();
        if edits.is_empty() {
            return;
        }
        let items: Vec<WriteItem> = self
            .state
            .selected_assets()
            .into_iter()
            .map(|asset| WriteItem {
                path: asset.path.clone(),
                format: asset.format,
                edits: edits.clone(),
            })
            .collect();
        if !items.is_empty() {
            self.meta.request(MetadataRequest::Write { items });
        }
    }

    /// Scrive le coordinate del marker sulla selezione.
    fn apply_gps_draft(&mut self) {
        let Some(location) = self.state.gps.marker else {
            self.state.error =
                Some("Posiziona il marker o inserisci le coordinate prima di applicare".to_owned());
            return;
        };
        self.write_gps_edit(Some(location));
    }

    /// Rimuove le coordinate GPS dalla selezione.
    fn remove_gps_selection(&mut self) {
        self.write_gps_edit(None);
    }

    fn write_gps_edit(&mut self, location: Option<GpsLocation>) {
        let items: Vec<WriteItem> = self
            .state
            .selected_assets()
            .into_iter()
            .map(|asset| WriteItem {
                path: asset.path.clone(),
                format: asset.format,
                edits: vec![MetadataEdit::Gps(location)],
            })
            .collect();
        if items.is_empty() {
            return;
        }
        self.meta.request(MetadataRequest::Write { items });
    }

    /// Scrive i soli campi di località modificati nella bozza.
    fn save_location_edits(&mut self) {
        if self.state.gps.location_dirty.is_empty() {
            return;
        }
        let edits: Vec<MetadataEdit> = self
            .state
            .gps
            .location_dirty
            .iter()
            .map(|(role, field)| {
                let value = self
                    .state
                    .gps
                    .location_values
                    .get(&(*role, *field))
                    .map(|text| text.trim().to_owned())
                    .filter(|text| !text.is_empty());
                MetadataEdit::Location {
                    role: *role,
                    field: *field,
                    value,
                }
            })
            .collect();
        let items: Vec<WriteItem> = self
            .state
            .selected_assets()
            .into_iter()
            .map(|asset| WriteItem {
                path: asset.path.clone(),
                format: asset.format,
                edits: edits.clone(),
            })
            .collect();
        if items.is_empty() {
            return;
        }
        self.meta.request(MetadataRequest::Write { items });
    }

    /// Scrittura rapida della valutazione dal menu contestuale della griglia.
    fn rate_metadata(&mut self, paths: Vec<PathBuf>, stars: u8) {
        let items: Vec<WriteItem> = self
            .state
            .entries
            .iter()
            .filter(|entry| paths.contains(&entry.path))
            .map(|entry| WriteItem {
                path: entry.path.clone(),
                format: entry.format,
                edits: vec![MetadataEdit::Field(
                    MetadataField::Rating,
                    Some(stars.to_string()),
                )],
            })
            .collect();
        if !items.is_empty() {
            self.meta.request(MetadataRequest::Write { items });
        }
    }

    fn pump_image_results(&mut self, ctx: &egui::Context) {
        for result in self.images.drain() {
            match result.kind {
                ImageKind::Thumbnail => {
                    let path = result.path;
                    self.thumbs
                        .on_result(ctx, path.clone(), result.mtime, result.outcome);
                    if let Some(index) = &self.index {
                        if let Some((width, height)) = self.thumbs.dims(&path) {
                            if let Err(err) = index.set_dimensions(&path, width, height) {
                                tracing::warn!("scrittura indice non riuscita: {err:#}");
                            }
                        }
                    }
                }
                ImageKind::Preview => {
                    let path = result.path;
                    let mtime = result.mtime;
                    if self.preview.accepts(&path, mtime) {
                        match &result.outcome {
                            Ok(image) => self.preview.set_image(ctx, image),
                            Err(error) => self.preview.set_error(error.clone()),
                        }
                    }
                    if self.state.edit.path.as_deref() == Some(path.as_path())
                        && self.state.edit.mtime == mtime
                    {
                        match result.outcome {
                            Ok(image) => {
                                self.state.edit.source =
                                    Some(Arc::new(EditSource { path, mtime, image }));
                                self.state.edit.source_requested = false;
                                self.state.edit.source_error = None;
                            }
                            Err(error) => {
                                self.state.edit.source_requested = false;
                                self.state.edit.source_error = Some(error);
                            }
                        }
                    } else if self.state.edit.source_requested {
                        // Il worker "latest wins" ha servito una richiesta più
                        // recente: la sorgente va chiesta di nuovo.
                        self.state.edit.source_requested = false;
                    }
                }
            }
        }
    }

    /// Raccoglie gli esiti delle operazioni su file: aggiorna l'indice,
    /// pubblica lo stato in barra e riallinea la vista alla cartella corrente.
    fn pump_ops_results(&mut self) {
        while let Some(outcome) = self.ops.try_recv() {
            let is_copy = outcome.description == "Copia";
            if !is_copy {
                if let Some(index) = &self.index {
                    for pair in &outcome.report.moved {
                        if let Err(err) = index.move_path(&pair.from, &pair.to) {
                            tracing::warn!("scrittura indice non riuscita: {err:#}");
                        }
                    }
                }
            }
            let undoable = outcome.description == "Spostamento"
                && outcome.report.failed.is_empty()
                && !outcome.report.moved.is_empty();
            let message = outcome.report.summary(outcome.description);
            let error = outcome.report.first_error();
            self.state.op_status = Some(OpStatus {
                message,
                error,
                undo: if undoable { Some(outcome.report) } else { None },
            });
            if let Some(dir) = self.state.current_dir.clone() {
                self.start_scan(dir);
            }
        }
    }

    fn handle_actions(&mut self, actions: UiActions) {
        let picked = if actions.pick_folder {
            rfd::FileDialog::new()
                .set_title("Scegli la cartella da aprire")
                .pick_folder()
        } else {
            None
        };
        if let Some(dir) = actions.open_folder.or(picked) {
            if dir.is_dir() {
                self.open_root(dir);
            } else {
                self.state.error =
                    Some(format!("La cartella \"{}\" non esiste più", dir.display()));
            }
        }
        if let Some(dir) = actions.browse_dir {
            if self.state.current_dir.as_deref() != Some(dir.as_path()) {
                self.start_scan(dir);
            }
        }
        if actions.refresh {
            if let Some(dir) = self.state.current_dir.clone() {
                self.start_scan(dir);
            }
        }
        if let Some(path) = actions.open_preview {
            let mtime = self.state.entry_mtime(&path);
            self.preview.open_for(&path, mtime, &self.images);
        }

        if let Some(request) = actions.drop_onto {
            self.begin_transfer(request.paths, request.target, request.copy);
        }
        if let Some(request) = actions.transfer {
            if let Some(target) = rfd::FileDialog::new()
                .set_title(if request.copy {
                    "Copia le foto in…"
                } else {
                    "Sposta le foto in…"
                })
                .pick_folder()
            {
                self.begin_transfer(request.paths, target, request.copy);
            }
        }

        if let Some(path) = actions.rename_photo {
            let new_name = file_name_of(&path);
            self.state.rename_prompt = Some(RenamePrompt {
                target: RenameTarget::Photo(path),
                new_name,
                first_frame: true,
            });
        }
        if let Some(path) = actions.rename_folder {
            let new_name = file_name_of(&path);
            self.state.rename_prompt = Some(RenamePrompt {
                target: RenameTarget::Folder(path),
                new_name,
                first_frame: true,
            });
        }
        if let Some(parent) = actions.create_folder {
            self.state.new_folder_prompt = Some(NewFolderPrompt {
                parent,
                name: "Nuova cartella".to_owned(),
                first_frame: true,
            });
        }
        if let Some(dir) = actions.delete_folder {
            let items = dir_item_count(&dir);
            self.state.delete_dir_prompt = Some(DeleteDirPrompt { dir, items });
        }

        if let Some((paths, target, copy, policy)) = actions.confirm_transfer {
            self.ops.request(FileOpRequest::Transfer {
                paths,
                target,
                copy,
                policy,
            });
        }
        if let Some((target, new_name)) = actions.confirm_rename {
            match target {
                RenameTarget::Photo(path) => {
                    self.ops.request(FileOpRequest::Rename {
                        path,
                        new_name,
                        policy: ConflictPolicy::RenameAuto,
                    });
                }
                RenameTarget::Folder(dir) => self.apply_folder_rename(&dir, &new_name),
            }
        }
        if let Some((parent, name)) = actions.confirm_new_folder {
            match create_dir(&parent, &name) {
                Ok(_) => self.refresh_tree_children(&parent),
                Err(message) => self.state.error = Some(message),
            }
        }
        if let Some(dir) = actions.confirm_delete_dir {
            match delete_empty_dir(&dir) {
                Ok(()) => {
                    if let Some(index) = &self.index {
                        if let Err(err) = index.remove_dir(&dir) {
                            tracing::warn!("scrittura indice non riuscita: {err:#}");
                        }
                    }
                    if let Some(parent) = dir.parent() {
                        let parent = parent.to_path_buf();
                        self.refresh_tree_children(&parent);
                    }
                    if self.state.current_dir.as_deref() == Some(dir.as_path()) {
                        if let Some(parent) = dir.parent() {
                            self.start_scan(parent.to_path_buf());
                        }
                    }
                }
                Err(message) => self.state.error = Some(message),
            }
        }

        if actions.metadata_save {
            self.save_metadata();
        }
        if actions.metadata_reload {
            let selected = self.state.selected_paths();
            for path in &selected {
                self.state.meta.docs.remove(path);
                self.state.meta.raw.remove(path);
                self.state.meta.errors.remove(path);
                self.state.meta.loading.remove(path);
                self.state.meta.summarized.remove(path);
                self.state.meta.pending_summary.remove(path);
                self.state.meta.loaded.remove(path);
            }
            self.state.meta.form_paths.clear();
            self.request_metadata_summary();
        }
        if let Some((paths, stars)) = actions.metadata_rate {
            self.rate_metadata(paths, stars);
        }

        if actions.gps_apply {
            self.apply_gps_draft();
        }
        if actions.gps_remove {
            self.remove_gps_selection();
        }
        if actions.gps_save_location {
            self.save_location_edits();
        }
        if actions.gps_reset {
            self.state.rebuild_gps_form();
        }
        if let Some((result, target)) = actions.gps_use_suggestion {
            match target {
                SuggestionTarget::Marker => {
                    let altitude = self.state.gps.marker.and_then(|marker| marker.altitude);
                    self.state.gps.set_marker(Some(GpsLocation {
                        latitude: result.latitude,
                        longitude: result.longitude,
                        altitude,
                    }));
                    let marker = self.state.gps.marker;
                    self.state.center_gps_map(marker);
                    self.state.gps.centered_for = marker;
                }
                SuggestionTarget::Created => {
                    let suggestion = result.location_suggestion();
                    self.state
                        .gps
                        .fill_location(LocationRole::Created, &suggestion);
                }
                SuggestionTarget::Shown => {
                    let suggestion = result.location_suggestion();
                    self.state
                        .gps
                        .fill_location(LocationRole::Shown, &suggestion);
                }
            }
        }
        if let Some(query) = actions.geocode_search {
            self.geo.request(GeoRequest::Search { query });
        }
        if let Some(location) = actions.geocode_reverse {
            self.geo.request(GeoRequest::Reverse { location });
        }

        if actions.edit_save_preset {
            self.save_edit_preset();
        }
        if let Some(nome) = actions.edit_delete_preset {
            self.delete_edit_preset(&nome);
        }
        if let Some(ricetta) = actions.edit_apply_preset {
            self.apply_edit_preset_to_selection(ricetta);
        }

        if actions.export_pick_destination {
            if let Some(cartella) = rfd::FileDialog::new()
                .set_title("Cartella di destinazione dell'esportazione")
                .pick_folder()
            {
                self.state.export.settings.destination = cartella;
            }
        }
        if actions.export_clear {
            self.state.export.reports.clear();
            self.state.export.failures.clear();
            self.state.export.summary = None;
            self.state.export.error = None;
            self.state.export.cancelled = false;
            self.state.export.done = 0;
            self.state.export.total = 0;
        }
        if actions.export_cancel {
            self.export.cancel();
        }
        if actions.export_start {
            self.start_export();
        }
    }

    /// Salva la ricetta corrente come preset nel catalogo locale.
    fn save_edit_preset(&mut self) {
        let nome = self.state.edit.preset_name.trim().to_owned();
        if nome.is_empty() {
            return;
        }
        let recipe = self.state.edit.recipe;
        let Some(index) = &self.index else {
            self.state.edit.save_error =
                Some("Preset non salvato: catalogo locale non disponibile".to_owned());
            return;
        };
        match index.save_preset(&nome, &recipe.to_json()) {
            Ok(()) => {
                match self
                    .state
                    .edit
                    .presets
                    .iter_mut()
                    .find(|(esistente, _)| *esistente == nome)
                {
                    Some(esistente) => esistente.1 = recipe,
                    None => {
                        self.state.edit.presets.push((nome, recipe));
                        self.state
                            .edit
                            .presets
                            .sort_by_cached_key(|(esistente, _)| esistente.to_lowercase());
                    }
                }
                self.state.edit.preset_name.clear();
                self.state.edit.save_error = None;
            }
            Err(err) => {
                self.state.edit.save_error = Some(format!("Preset non salvato: {err:#}"));
            }
        }
    }

    /// Applica una ricetta (o la neutra) a tutte le foto selezionate.
    fn apply_edit_preset_to_selection(&mut self, recipe: EditRecipe) {
        let recipe = recipe.sanitized();
        let selezionati = self.state.selected_paths();
        let Some(index) = &self.index else {
            self.state.edit.save_error =
                Some("Preset non applicato: catalogo locale non disponibile".to_owned());
            return;
        };
        let mut errori = 0usize;
        for path in &selezionati {
            let esito = if recipe.is_identity() {
                index.remove_recipe(path)
            } else {
                index.save_recipe(path, &recipe.to_json())
            };
            if esito.is_err() {
                errori += 1;
            }
        }
        if errori > 0 {
            self.state.edit.save_error = Some(format!(
                "Preset applicato con {errori} errori di salvataggio nel catalogo"
            ));
        } else {
            self.state.edit.save_error = None;
        }
        if let Some(path) = self.state.edit.path.clone() {
            if selezionati.iter().any(|selezionata| selezionata == &path) {
                let mtime = self.state.edit.mtime;
                self.state.edit.carica(&path, mtime, recipe);
            }
        }
    }

    /// Elimina un preset dal catalogo locale.
    fn delete_edit_preset(&mut self, nome: &str) {
        let Some(index) = &self.index else {
            return;
        };
        match index.delete_preset(nome) {
            Ok(()) => {
                self.state
                    .edit
                    .presets
                    .retain(|(esistente, _)| esistente != nome);
                self.state.edit.save_error = None;
            }
            Err(err) => {
                self.state.edit.save_error = Some(format!("Preset non eliminato: {err:#}"));
            }
        }
    }

    fn begin_transfer(&mut self, paths: Vec<PathBuf>, target: PathBuf, copy: bool) {
        if paths.is_empty() {
            return;
        }
        if self.state.current_dir.as_deref() == Some(target.as_path()) {
            self.state.error = Some("Le foto si trovano già in questa cartella".to_owned());
            return;
        }
        self.state.conflict_prompt = Some(ConflictPrompt {
            paths,
            target,
            copy,
            policy: ConflictPolicy::RenameAuto,
        });
    }

    /// Rinomina una cartella: aggiorna indice, albero, recenti e vista.
    fn apply_folder_rename(&mut self, dir: &Path, new_name: &str) {
        let new_path = match rename_dir(dir, new_name) {
            Ok(path) => path,
            Err(message) => {
                self.state.error = Some(message);
                return;
            }
        };
        if let Some(index) = &self.index {
            if let Err(err) = index.move_dir(dir, &new_path) {
                tracing::warn!("scrittura indice non riuscita: {err:#}");
            }
        }
        if let Some(root) = &mut self.state.root {
            tree_repath(root, dir, &new_path);
        }

        let update = |path: &mut PathBuf| {
            if let Ok(relative) = path.strip_prefix(dir) {
                *path = if relative.as_os_str().is_empty() {
                    new_path.clone()
                } else {
                    new_path.join(relative)
                };
            }
        };
        for recent in &mut self.state.recent_folders {
            update(recent);
        }
        if let Some(current) = self.state.current_dir.clone() {
            if current.starts_with(dir) {
                let mut updated = current;
                update(&mut updated);
                self.start_scan(updated);
            }
        }
    }

    /// Ricarica i figli del nodo `target` nell'albero, se visibile.
    fn refresh_tree_children(&mut self, target: &Path) {
        let mut error = None;
        if let Some(root) = &mut self.state.root {
            reload_node(root, target, &mut error);
        }
        if let Some(message) = error {
            self.state.error = Some(message);
        }
    }

    fn prompts(&mut self, ctx: &egui::Context, actions: &mut UiActions) {
        self.conflict_prompt_window(ctx, actions);
        self.rename_prompt_window(ctx, actions);
        self.new_folder_window(ctx, actions);
        self.delete_dir_window(ctx, actions);
    }

    fn conflict_prompt_window(&mut self, ctx: &egui::Context, actions: &mut UiActions) {
        let mut confirmed = false;
        let mut cancelled = false;
        if let Some(prompt) = self.state.conflict_prompt.as_mut() {
            let title = if prompt.copy {
                "Copia le foto"
            } else {
                "Sposta le foto"
            };
            egui::Window::new(title)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    ui.label(format!(
                        "{} file verso {}",
                        prompt.paths.len(),
                        elide(&prompt.target.display().to_string(), 48)
                    ));
                    ui.add_space(8.0);
                    ui.checkbox(&mut prompt.copy, "Copia invece di spostare");
                    ui.add_space(8.0);
                    ui.label("Se il nome esiste già nella destinazione:");
                    ui.radio_value(
                        &mut prompt.policy,
                        ConflictPolicy::RenameAuto,
                        "Rinomina automaticamente (foto (1).jpg)",
                    );
                    ui.radio_value(
                        &mut prompt.policy,
                        ConflictPolicy::Replace,
                        "Sostituisci il file esistente",
                    );
                    ui.radio_value(
                        &mut prompt.policy,
                        ConflictPolicy::Skip,
                        "Salta i file già presenti",
                    );
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button("Annulla").clicked() {
                            cancelled = true;
                        }
                        let label = if prompt.copy { "Copia" } else { "Sposta" };
                        if ui.button(label).clicked() {
                            confirmed = true;
                        }
                    });
                });
        }
        if confirmed {
            if let Some(prompt) = self.state.conflict_prompt.take() {
                actions.confirm_transfer =
                    Some((prompt.paths, prompt.target, prompt.copy, prompt.policy));
            }
        } else if cancelled {
            self.state.conflict_prompt = None;
        }
    }

    fn rename_prompt_window(&mut self, ctx: &egui::Context, actions: &mut UiActions) {
        let mut confirmed = false;
        let mut cancelled = false;
        if let Some(prompt) = self.state.rename_prompt.as_mut() {
            let title = match &prompt.target {
                RenameTarget::Photo(_) => "Rinomina foto",
                RenameTarget::Folder(_) => "Rinomina cartella",
            };
            egui::Window::new(title)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    ui.label(elide(&prompt.target.path().display().to_string(), 56));
                    ui.add_space(6.0);
                    let response = ui
                        .add(egui::TextEdit::singleline(&mut prompt.new_name).desired_width(280.0));
                    if prompt.first_frame {
                        response.request_focus();
                        prompt.first_frame = false;
                    }
                    let enter = response.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter));
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button("Annulla").clicked() {
                            cancelled = true;
                        }
                        if ui.button("Rinomina").clicked() || enter {
                            confirmed = true;
                        }
                    });
                });
        }
        if confirmed {
            if let Some(prompt) = self.state.rename_prompt.take() {
                let name = prompt.new_name.trim().to_owned();
                if !name.is_empty() {
                    actions.confirm_rename = Some((prompt.target, name));
                }
            }
        } else if cancelled {
            self.state.rename_prompt = None;
        }
    }

    fn new_folder_window(&mut self, ctx: &egui::Context, actions: &mut UiActions) {
        let mut confirmed = false;
        let mut cancelled = false;
        if let Some(prompt) = self.state.new_folder_prompt.as_mut() {
            egui::Window::new("Nuova cartella")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    ui.label(format!(
                        "Dentro {}",
                        elide(&prompt.parent.display().to_string(), 48)
                    ));
                    ui.add_space(6.0);
                    let response =
                        ui.add(egui::TextEdit::singleline(&mut prompt.name).desired_width(280.0));
                    if prompt.first_frame {
                        response.request_focus();
                        prompt.first_frame = false;
                    }
                    let enter = response.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter));
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button("Annulla").clicked() {
                            cancelled = true;
                        }
                        if ui.button("Crea").clicked() || enter {
                            confirmed = true;
                        }
                    });
                });
        }
        if confirmed {
            if let Some(prompt) = self.state.new_folder_prompt.take() {
                let name = prompt.name.trim().to_owned();
                if !name.is_empty() {
                    actions.confirm_new_folder = Some((prompt.parent, name));
                }
            }
        } else if cancelled {
            self.state.new_folder_prompt = None;
        }
    }

    fn delete_dir_window(&mut self, ctx: &egui::Context, actions: &mut UiActions) {
        let mut confirmed = false;
        let mut cancelled = false;
        if let Some(prompt) = self.state.delete_dir_prompt.as_mut() {
            egui::Window::new("Elimina cartella")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    ui.label(format!(
                        "Eliminare \"{}\"?",
                        elide(&prompt.dir.display().to_string(), 48)
                    ));
                    ui.add_space(6.0);
                    if prompt.items == 0 {
                        ui.weak(
                            "La cartella è vuota e verrà rimossa dal disco. \
                             L'operazione non è annullabile.",
                        );
                    } else {
                        ui.colored_label(
                            ERROR_COLOR,
                            format!(
                                "La cartella contiene {} elementi: UberPhoto elimina \
                                 solo cartelle vuote.",
                                prompt.items
                            ),
                        );
                    }
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button("Annulla").clicked() {
                            cancelled = true;
                        }
                        let enabled = prompt.items == 0;
                        if ui
                            .add_enabled(enabled, egui::Button::new("Elimina"))
                            .clicked()
                        {
                            confirmed = true;
                        }
                    });
                });
        }
        if confirmed {
            if let Some(prompt) = self.state.delete_dir_prompt.take() {
                actions.confirm_delete_dir = Some(prompt.dir);
            }
        } else if cancelled {
            self.state.delete_dir_prompt = None;
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let mut undo_requested = false;
        let mut clear_status = false;
        ui.horizontal(|ui| {
            if self.state.loading_dir.is_some() {
                ui.spinner();
                ui.label("Scansione in corso…");
            } else if let Some(dir) = &self.state.current_dir {
                ui.label(format!(
                    "{} — {} fotografie",
                    elide(&dir.display().to_string(), 70),
                    self.state.entries.len()
                ));
            } else {
                ui.label("Nessuna cartella aperta");
            }

            ui.separator();
            match self.state.selection.len() {
                0 => {}
                1 => {
                    ui.label("1 selezionata");
                }
                count => {
                    ui.label(format!("{count} selezionate"));
                }
            }

            let (cached, inflight) = self.thumbs.stats();
            ui.separator();
            ui.weak(format!("Miniature: {cached} in cache · {inflight} in coda"));

            if let Some(status) = &self.state.op_status {
                ui.separator();
                ui.label(elide(&status.message, 70));
                if let Some(error) = &status.error {
                    ui.colored_label(ERROR_COLOR, elide(error, 70));
                }
                if status.undo.is_some() && ui.small_button("↩ Annulla").clicked() {
                    undo_requested = true;
                }
                if ui.small_button("✕").clicked() {
                    clear_status = true;
                }
            }

            if let Some(error) = self.state.error.clone() {
                ui.separator();
                ui.colored_label(ERROR_COLOR, elide(&error, 90));
                if ui.small_button("✕").clicked() {
                    self.state.error = None;
                }
            }
        });

        if clear_status {
            self.state.op_status = None;
        }
        if undo_requested {
            if let Some(status) = self.state.op_status.take() {
                if let Some(report) = status.undo {
                    self.ops.request(FileOpRequest::Undo { report });
                }
            }
        }
    }
}

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn open_index() -> Option<Index> {
    let dir = eframe::storage_dir("UberPhoto")?;
    let path = dir.join("index.db");
    match Index::open(&path) {
        Ok(index) => Some(index),
        Err(err) => {
            tracing::warn!("indice non disponibile: {err:#}");
            None
        }
    }
}

/// Client Nominatim con cache su disco accanto all'indice.
fn geo_client() -> NominatimClient {
    match eframe::storage_dir("UberPhoto") {
        Some(dir) => NominatimClient::new(Some(dir.join("geo-cache.json"))),
        None => NominatimClient::new(None),
    }
}

/// Aggiorna i percorsi dei nodi sotto `from` dopo una rinomina di cartella,
/// ricaricando i figli dei rami espansi coinvolti.
fn tree_repath(node: &mut DirNode, from: &Path, to: &Path) {
    if let Some(children) = node.children.as_mut() {
        for child in children {
            tree_repath(child, from, to);
        }
    }
    if !node.path.starts_with(from) {
        return;
    }
    let Ok(relative) = node.path.strip_prefix(from) else {
        return;
    };
    node.path = if relative.as_os_str().is_empty() {
        to.to_path_buf()
    } else {
        to.join(relative)
    };
    node.name = node
        .path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    node.children = None;
    if node.expanded {
        let _ = node.load_children();
    }
}

/// Invalida e ricarica i figli del nodo `target`, se presente nell'albero.
fn reload_node(node: &mut DirNode, target: &Path, error: &mut Option<String>) -> bool {
    if node.path == target {
        node.children = None;
        if node.expanded {
            if let Some(message) = node.load_children() {
                *error = Some(message);
            }
        }
        return true;
    }
    if let Some(children) = node.children.as_mut() {
        for child in children {
            if reload_node(child, target, error) {
                return true;
            }
        }
    }
    false
}

/// Mondo visivo: scuro professionale fotografico — chrome neutro e accenti
/// misurati, così le fotografie dominano la scena senza falsarne i colori.
fn setup_visuals(ctx: &egui::Context) {
    let canvas = egui::Color32::from_rgb(16, 16, 18);
    let panel = egui::Color32::from_rgb(28, 28, 31);
    let surface = egui::Color32::from_rgb(38, 38, 42);
    let surface_hover = egui::Color32::from_rgb(46, 46, 51);
    let surface_active = egui::Color32::from_rgb(58, 58, 64);
    let stroke = egui::Color32::from_rgb(52, 52, 58);
    let accent = egui::Color32::from_rgb(110, 155, 209);
    let selection_bg = egui::Color32::from_rgb(47, 74, 102);

    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = panel;
    visuals.window_fill = panel;
    visuals.extreme_bg_color = canvas;
    visuals.faint_bg_color = egui::Color32::from_rgb(33, 33, 37);
    visuals.hyperlink_color = accent;
    visuals.window_corner_radius = egui::CornerRadius::same(10);

    visuals.widgets.noninteractive.bg_fill = panel;
    visuals.widgets.noninteractive.weak_bg_fill = panel;
    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, stroke);

    visuals.widgets.inactive.bg_fill = surface;
    visuals.widgets.inactive.weak_bg_fill = surface;
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, stroke);

    visuals.widgets.hovered.bg_fill = surface_hover;
    visuals.widgets.hovered.weak_bg_fill = surface_hover;
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, accent);

    visuals.widgets.active.bg_fill = surface_active;
    visuals.widgets.active.weak_bg_fill = surface_active;
    visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, accent);

    visuals.selection.bg_fill = selection_bg;
    visuals.selection.stroke = egui::Stroke::new(1.0, accent);

    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = egui::CornerRadius::same(6);
    }

    ctx.set_theme(egui::Theme::Dark);
    ctx.set_visuals(visuals);
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
    });
}

/// Decodifica il logo incorporato una sola volta per la texture della UI.
fn load_logo(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let image = uberphoto_image::decode_bytes_rgba(crate::APP_ICON_PNG, Some(256)).ok()?;
    let color_image = egui::ColorImage::from_rgba_unmultiplied(
        [image.width as usize, image.height as usize],
        &image.rgba,
    );
    Some(ctx.load_texture("uberphoto-logo", color_image, egui::TextureOptions::LINEAR))
}

impl eframe::App for UberPhotoApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        self.thumbs.begin_frame();
        self.pump_scan_results();
        self.pump_image_results(&ctx);
        self.pump_ops_results();
        self.pump_metadata_results();
        self.pump_geo_results();
        self.pump_edit_results(&ctx);
        self.pump_export_results();
        self.sync_metadata();
        self.sync_edit(&ctx);

        // Modifiche esterne alla cartella corrente: riscansione calmierata.
        let settled = self
            .watcher
            .as_ref()
            .is_some_and(DirWatcher::take_settled_change);
        if settled && self.state.loading_dir.is_none() {
            if let Some(dir) = self.state.current_dir.clone() {
                tracing::info!(dir = %dir.display(), "modifiche esterne rilevate");
                self.start_scan(dir);
            }
        }

        let mut actions = UiActions::default();

        egui::Panel::bottom("status_bar").show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("explorer")
            .resizable(true)
            .default_size(260.0)
            .size_range(180.0..=520.0)
            .show(ui, |ui| explorer::ui(ui, &mut self.state, &mut actions));
        egui::Panel::right("inspector")
            .resizable(true)
            .default_size(320.0)
            .size_range(240.0..=640.0)
            .show(ui, |ui| {
                inspector::ui(ui, &mut self.state, &self.thumbs, &mut actions);
            });
        egui::CentralPanel::default().show(ui, |ui| {
            browser::ui(
                ui,
                &mut self.state,
                &mut self.thumbs,
                &self.images,
                &mut actions,
            );
        });

        self.prompts(&ctx, &mut actions);
        self.handle_actions(actions);
        self.autosave_recipe();
        self.preview.set_prefer_edited(!self.state.edit.compare);
        self.preview.ui(&ctx);

        // Mantiene animati spinner e avanzamento durante le operazioni lunghe.
        let watcher_pending = self.watcher.as_ref().is_some_and(DirWatcher::has_pending);
        if self.state.loading_dir.is_some()
            || self.thumbs.has_inflight()
            || self.preview.is_loading()
            || self.state.edit.busy
            || self.state.export.busy
            || watcher_pending
        {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        if let Some((start, seconds)) = self.smoke_exit {
            if start.elapsed().as_secs_f64() >= seconds {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, &self.state.persisted());
    }
}
