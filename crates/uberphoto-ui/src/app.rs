//! Applicazione e orchestrazione dei pannelli.
//!
//! Ciclo del frame:
//! 1. risveglio delle cache (LRU miniature);
//! 2. raccolta dei risultati di scansione, decodifica e operazioni su file;
//! 3. disegno dei pannelli e dei prompt, che raccolgono [`UiActions`];
//! 4. esecuzione delle azioni (navigazione, operazioni, annullamento).

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use crate::t;
use eframe::egui;
use uberphoto_core::GpsLocation;
use uberphoto_db::{Index, MetadataIndex};
use uberphoto_export::{
    ExportItem, ExportJob, ExportService, SaveEvent, SaveJob, SaveMode, SaveRequest, SaveService,
};
use uberphoto_files::{
    create_dir, delete_empty_dir, dir_item_count, rename_dir, ConflictPolicy, DirNode, DirWatcher,
    DirectoryScanner, FileOpRequest, FileOpsService, SortKey,
};
use uberphoto_geo::{GeoOutcome, GeoRequest, GeoService, NominatimClient};
use uberphoto_image::{EditRecipe, EditService, EditSource, ImageKind, ImageResult, ImageService};
use uberphoto_metadata::{
    exiftool_versione, LocationRole, MetadataEdit, MetadataField, MetadataOutcome, MetadataRequest,
    MetadataService, WriteItem,
};

use crate::browser;
use crate::explorer;
use crate::i18n::Language;
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
    /// Salvataggio manuale su thread dedicato.
    save: SaveService,
    /// Osservatore della cartella corrente per le modifiche esterne.
    watcher: Option<DirWatcher>,
    /// Chiusura automatica dopo N secondi, usata dagli smoke test di CI
    /// (variabile d'ambiente `UBERPHOTO_SMOKE_EXIT_SECS`).
    smoke_exit: Option<(std::time::Instant, f64)>,
    /// Conferma di chiusura per modifiche non salvate.
    close_prompt: bool,
    /// La chiusura è stata confermata: non richiedere più il prompt.
    allow_close: bool,
    /// L'indice è cambiato: l'ordinamento va riallineato.
    index_dirty: bool,
    /// Ultimo riallineamento dell'ordinamento, per calmierare i batch.
    last_index_sort: Instant,
    /// Risultati immagine in attesa di essere applicati alla UI.
    pending_images: VecDeque<ImageResult>,
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
            save: SaveService::new(repaint.clone()),
            watcher: DirWatcher::new(repaint),
            smoke_exit,
            close_prompt: false,
            allow_close: false,
            index_dirty: false,
            last_index_sort: Instant::now(),
            pending_images: VecDeque::new(),
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
        self.state.set_entries(Vec::new());
        self.state.subdirs.clear();
        self.state.index_rows.clear();
        self.state.mark_index_changed();
        self.state.selection.clear();
        self.state.selection_anchor = None;
        // I job di miniature della cartella precedente non servono più.
        let epoch = self.images.bump_epoch();
        self.thumbs.set_epoch(epoch);
        self.pending_images.clear();
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
                    self.state.set_entries(Vec::new());
                    self.state.subdirs.clear();
                }
                None => {
                    self.state.set_entries(result.entries);
                    self.state.subdirs = result.subdirs;
                    self.refresh_index_rows(&result.dir);
                    self.state.apply_sort();
                    if let Some(path) = self.state.pending_select.take() {
                        if path.starts_with(&result.dir) && self.state.entry(&path).is_some() {
                            self.state.selection.insert(path.clone());
                            self.state.selection_anchor = Some(path);
                        }
                    }
                    self.request_metadata_summary();
                }
            }
        }
    }

    /// Allinea l'indice SQLite al sottoalbero appena scansionato e ne carica
    /// le righe nello stato per ordinamenti e filtri.
    ///
    /// La scrittura avviene in un'unica transazione: con migliaia di foto
    /// l'autocommit riga per riga bloccava il thread UI per decine di secondi.
    fn refresh_index_rows(&mut self, dir: &Path) {
        let Some(index) = &self.index else { return };
        if let Err(err) = index.upsert_file_info_batch(&self.state.entries) {
            tracing::warn!("scrittura indice non riuscita: {err:#}");
        }
        match index.load_subtree(dir) {
            Ok(rows) => {
                self.state.index_rows = rows
                    .into_iter()
                    .map(|row| (row.path.clone(), row))
                    .collect();
                self.state.mark_index_changed();
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
        let correnti: HashSet<&Path> = self
            .state
            .entries
            .iter()
            .map(|e| e.path.as_path())
            .collect();
        self.state
            .meta
            .summarized
            .retain(|path, _| correnti.contains(path.as_path()));

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

    /// Raccoglie gli esiti del motore metadati: aggiorna indice, documenti,
    /// form e barra di stato.
    fn pump_metadata_results(&mut self) {
        let mut summary_batch: Vec<(PathBuf, Option<i64>, MetadataIndex)> = Vec::new();
        while let Some(outcome) = self.meta.try_recv() {
            match outcome {
                MetadataOutcome::Summary { items } => {
                    for (path, summary) in items {
                        let mtime =
                            self.state.meta.pending_summary.remove(&path).or_else(|| {
                                self.state.index_rows.get(&path).and_then(|row| row.mtime)
                            });
                        if let Some(mtime) = mtime {
                            self.state.meta.summarized.insert(path.clone(), mtime);
                        }
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
                        // Aggiorna subito la riga in memoria: l'ordinamento e
                        // i filtri non devono rileggere tutta la tabella.
                        self.state.apply_index_metadata(&path, mtime, &metadata);
                        summary_batch.push((path, mtime, metadata));
                    }
                    if !summary_batch.is_empty() {
                        self.state.mark_index_changed();
                        self.index_dirty = true;
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
                    let selected_paths = self.state.selected_paths();
                    if self.state.meta.form_paths == selected_paths {
                        self.state.rebuild_meta_form();
                    }
                    if self.state.gps.form_paths == selected_paths {
                        self.state.rebuild_gps_form();
                    }
                }
                MetadataOutcome::Written { report } => {
                    self.state.op_status = Some(OpStatus {
                        message: report.summary(),
                        error: report.first_error(),
                        undo: None,
                    });
                    // Le bozze restano intatte: questa scrittura è la
                    // valutazione rapida, non il salvataggio del form.
                    if report.updated.is_empty() {
                        continue;
                    }
                    let selected = self.state.selected_paths();
                    let selected_set: HashSet<&Path> =
                        selected.iter().map(PathBuf::as_path).collect();
                    let da_rileggere: Vec<PathBuf> = report
                        .updated
                        .iter()
                        .filter(|path| selected_set.contains(path.as_path()))
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

        if !summary_batch.is_empty() {
            if let Some(index) = &self.index {
                if let Err(err) = index.upsert_metadata_batch(&summary_batch) {
                    tracing::warn!("scrittura indice non riuscita: {err:#}");
                }
            }
        }

        // Un solo riallineamento dell'ordine, e solo se la chiave dipende dai
        // metadati. Durante l'arrivo dei riassunti il sort è calmierato.
        if self.index_dirty {
            let needs_sort = matches!(
                self.state.sort_key,
                SortKey::AcquisitionDate | SortKey::Rating
            );
            if !needs_sort {
                self.index_dirty = false;
            } else if self.state.meta.pending_summary.is_empty()
                || self.last_index_sort.elapsed() >= Duration::from_millis(250)
            {
                self.state.apply_sort();
                self.index_dirty = false;
                self.last_index_sort = Instant::now();
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
                                self.state.fill_gps_location(
                                    LocationRole::Created,
                                    &suggestion.location_suggestion(),
                                );
                                self.state.gps.reverse_note = Some(t!(
                                    "Località di scatto compilata da: {name}",
                                    name = suggestion.display_name
                                ));
                            }
                        }
                        Ok(None) => {
                            self.state.gps.geo_error = Some(t!(
                                "Nessuna località trovata per {lat}, {lng}",
                                lat = format!("{:.6}", location.latitude),
                                lng = format!("{:.6}", location.longitude)
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
            self.state.meta.raw_filter.clear();
        }
        if self.state.meta.form_paths != selected || !da_ricaricare.is_empty() {
            self.state.rebuild_meta_form();
        }
        if self.state.gps.form_paths != selected || !da_ricaricare.is_empty() {
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
                    let base = self
                        .index
                        .as_ref()
                        .and_then(|index| index.load_recipe(path).ok().flatten())
                        .and_then(|json| EditRecipe::from_json(&json).ok())
                        .map(|recipe| recipe.sanitized())
                        .unwrap_or_default();
                    let bozza = self.state.edit.drafts.get(path).copied().unwrap_or(base);
                    self.state.edit.carica(path, mtime, base);
                    self.state.edit.recipe = bozza;
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

    /// Conserva la ricetta modificata come bozza in memoria: il salvataggio
    /// su file (nuova copia o sovrascrittura) resta un gesto esplicito.
    fn remember_edit_draft(&mut self) {
        self.state.edit.remember_draft();
    }

    /// Ignora per un breve periodo gli eventi del watcher causati dalle
    /// scritture dell'applicazione (rinomine, salvataggi, metadati).
    fn suppress_watcher(&self) {
        if let Some(watcher) = &self.watcher {
            watcher.suppress_for(Duration::from_millis(800));
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
                        t!("interrotta")
                    } else {
                        t!("completata")
                    };
                    self.state.export.summary = Some(t!(
                        "Esportazione {stato}: riuscite {exported}, fallite {failed} su {total}.",
                        stato = stato,
                        exported = exported,
                        failed = failed,
                        total = total
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
            self.state.export.error =
                Some(t!("Seleziona almeno una foto da esportare.").to_owned());
            return;
        }
        let items: Vec<ExportItem> = selezionati
            .into_iter()
            .map(|(path, kind)| {
                let recipe = self.save_recipe_for(&path);
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
            self.state.export.error = Some(t!("Un'esportazione è già in corso.").to_owned());
        }
    }

    /// Apre il dialogo di conferma del salvataggio manuale.
    fn open_save_dialog(&mut self) {
        if self.state.save_dialog.is_some() || self.save.busy() {
            return;
        }
        if self.state.selection.is_empty() {
            self.state.error = Some(t!("Seleziona almeno una foto da salvare.").to_owned());
            return;
        }
        if let Some(error) = &self.state.gps.manual_error {
            self.state.error = Some(t!(
                "Correggi la posizione prima di salvare: {error}",
                error = error
            ));
            return;
        }
        if !self.state.has_pending_changes() {
            self.state.op_status = Some(OpStatus {
                message: t!("Nessuna modifica da salvare.").to_owned(),
                error: None,
                undo: None,
            });
            return;
        }
        let dialog = crate::state::SaveDialog {
            targets: self.state.save_targets(),
            ..Default::default()
        };
        self.state.save_dialog = Some(dialog);
    }

    /// Ricetta effettiva da salvare: bozza corrente, bozza in memoria o
    /// ricetta del catalogo (compatibilità con le versioni precedenti).
    fn save_recipe_for(&self, path: &Path) -> EditRecipe {
        if self.state.edit.path.as_deref() == Some(path) {
            return self.state.edit.recipe;
        }
        if let Some(bozza) = self.state.edit.drafts.get(path) {
            return bozza.sanitized();
        }
        self.index
            .as_ref()
            .and_then(|index| index.load_recipe(path).ok().flatten())
            .and_then(|json| EditRecipe::from_json(&json).ok())
            .map(|recipe| recipe.sanitized())
            .unwrap_or_default()
    }

    /// Avvia il salvataggio delle foto coinvolte, validando prima formati e
    /// modalità per non lasciare file parziali.
    fn start_save(&mut self, mode: SaveMode) {
        let Some(mut dialog) = self.state.save_dialog.take() else {
            return;
        };
        dialog.mode = mode;
        dialog.error = None;
        let targets = if dialog.targets.is_empty() {
            self.state.save_targets()
        } else {
            dialog.targets.clone()
        };
        if targets.is_empty() {
            dialog.error = Some(t!("Nessuna modifica da salvare.").to_owned());
            dialog.targets.clear();
            self.state.save_dialog = Some(dialog);
            return;
        }

        self.suppress_watcher();
        let mut requests = Vec::with_capacity(targets.len());
        let mut formati_non_supportati = Vec::new();
        for path in &targets {
            let Some(asset) = self.state.entry(path) else {
                dialog.error = Some(t!(
                    "{name} non è più nella cartella corrente.",
                    name = path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string())
                ));
                dialog.targets = targets.clone();
                self.state.save_dialog = Some(dialog);
                return;
            };
            let capabilities = asset.format.capabilities();
            let recipe = self.save_recipe_for(path);
            let mut edits = self.state.metadata_edits(path);
            edits.extend(self.state.gps_edits(path));
            if (mode == SaveMode::NewCopy || !recipe.is_identity()) && !capabilities.pixel_edit {
                formati_non_supportati.push(asset.format.label());
            }
            if !edits.is_empty() && !capabilities.embedded_metadata && !capabilities.xmp_sidecar {
                formati_non_supportati.push(asset.format.label());
            }
            requests.push(SaveRequest {
                path: asset.path.clone(),
                kind: asset.format,
                recipe,
                edits,
                mode,
            });
        }
        formati_non_supportati.sort_unstable();
        formati_non_supportati.dedup();
        if !formati_non_supportati.is_empty() {
            let formati = formati_non_supportati.join(", ");
            dialog.error = Some(match mode {
                SaveMode::NewCopy => t!(
                    "La nuova copia è disponibile solo per JPEG, PNG, TIFF e WebP. Formati non supportati: {formats}. Scegli “Sovrascrivi originale”.",
                    formats = formati
                ),
                SaveMode::Overwrite => t!(
                    "Le modifiche non possono essere applicate a: {formats}. Il colore richiede un formato modificabile; i metadati richiedono scrittura embedded o sidecar XMP.",
                    formats = formati
                ),
            });
            dialog.targets = targets;
            self.state.save_dialog = Some(dialog);
            return;
        }

        dialog.busy = true;
        dialog.targets = targets;
        dialog.completed.clear();
        dialog.created = 0;
        dialog.failure = None;
        self.state.save_dialog = Some(dialog);
        if !self.save.start(SaveJob { requests }) {
            if let Some(dialog) = self.state.save_dialog.as_mut() {
                dialog.busy = false;
                dialog.error = Some(t!("Un salvataggio è già in corso.").to_owned());
            }
        }
    }

    /// Raccoglie gli esiti del salvataggio, aggiorna il dialogo e ricarica la
    /// cartella per mostrare i file appena scritti.
    fn pump_save_results(&mut self) {
        let mut outcomes = Vec::new();
        let mut failures = Vec::new();
        let mut finished = None;
        for event in self.save.drain() {
            match event {
                SaveEvent::Saved(outcome) => outcomes.push(outcome),
                SaveEvent::Failed { source, error } => {
                    if let Some(dialog) = self.state.save_dialog.as_mut() {
                        if dialog.failure.is_none() {
                            dialog.failure = Some(t!(
                                "{name}: {error}",
                                name = source
                                    .file_name()
                                    .map(|name| name.to_string_lossy().into_owned())
                                    .unwrap_or_else(|| source.display().to_string()),
                                error = error
                            ));
                        }
                    }
                    failures.push((source, error));
                }
                SaveEvent::Finished {
                    saved,
                    failed,
                    total,
                } => finished = Some((saved, failed, total)),
            }
        }

        for outcome in outcomes {
            for warning in &outcome.warnings {
                tracing::warn!(
                    file = %outcome.output.display(),
                    "salvataggio: {warning}"
                );
            }
            self.state.edit.forget_draft(&outcome.source);
            self.state.meta.drafts.remove(&outcome.source);
            self.state.gps.drafts.remove(&outcome.source);
            // I metadati possono essere cambiati anche via sidecar, senza
            // toccare l'mtime dell'immagine: invalida le cache per rileggerli.
            for path in [&outcome.source, &outcome.output] {
                self.state.meta.docs.remove(path);
                self.state.meta.raw.remove(path);
                self.state.meta.loaded.remove(path);
                self.state.meta.summarized.remove(path);
            }
            if let Some(index) = &self.index {
                if let Err(err) = index.remove_recipe(&outcome.source) {
                    tracing::warn!("rimozione della ricetta non riuscita: {err:#}");
                }
            }
            if self.state.edit.path.as_deref() == Some(outcome.source.as_path()) {
                self.state.edit.recipe = EditRecipe::default();
                self.state.edit.saved = EditRecipe::default();
                self.state.edit.undo.clear();
                self.state.edit.redo.clear();
                self.state.edit.drag_base = None;
                self.state.edit.requested = None;
                self.state.edit.edited = None;
            }
            self.state.pending_select = Some(outcome.output);
            if let Some(dialog) = self.state.save_dialog.as_mut() {
                dialog.completed.insert(outcome.source.clone());
                dialog.targets.retain(|path| path != &outcome.source);
                if outcome.created {
                    dialog.created += 1;
                }
            }
        }

        let Some((_saved, _failed, total)) = finished else {
            return;
        };
        let Some(mut dialog) = self.state.save_dialog.take() else {
            return;
        };
        dialog.busy = false;
        let saved = dialog.completed.len();
        let failed = total.saturating_sub(saved);
        let nuove = dialog.created;

        if failed == 0 {
            self.state.meta.form_dirty.clear();
            self.state.gps.location_dirty.clear();
            self.state.gps.coordinates_dirty = false;
            self.state.gps.draft_dirty = false;
            let messaggio = if nuove == saved {
                t!("Salvate {count} nuove foto.", count = saved)
            } else if nuove == 0 {
                t!("Aggiornate {count} foto.", count = saved)
            } else {
                t!(
                    "Foto salvate: {saved} ({new} nuove copie).",
                    saved = saved,
                    new = nuove
                )
            };
            self.state.op_status = Some(OpStatus {
                message: messaggio,
                error: None,
                undo: None,
            });
        } else {
            let primo = dialog
                .failure
                .clone()
                .or_else(|| {
                    failures.first().map(|(path, error)| {
                        t!(
                            "{name}: {error}",
                            name = path
                                .file_name()
                                .map(|name| name.to_string_lossy().into_owned())
                                .unwrap_or_else(|| path.display().to_string()),
                            error = error
                        )
                    })
                })
                .unwrap_or_default();
            dialog.error = Some(t!(
                "Salvate: {saved} su {total}; non riuscite: {failed}. {first}",
                saved = saved,
                total = total,
                failed = failed,
                first = primo
            ));
            self.state.op_status = Some(OpStatus {
                message: t!(
                    "Salvataggio incompleto: riuscite {saved}, fallite {failed}.",
                    saved = saved,
                    failed = failed
                ),
                error: Some(primo),
                undo: None,
            });
        }
        if failed == 0 {
            self.state.save_dialog = None;
        } else {
            self.state.save_dialog = Some(dialog);
        }

        if saved > 0 {
            if let Some(dir) = self.state.current_dir.clone() {
                self.suppress_watcher();
                self.start_scan(dir);
            }
        }
    }

    /// Scrittura rapida della valutazione dal menu contestuale della griglia.
    fn rate_metadata(&mut self, paths: Vec<PathBuf>, stars: u8) {
        let items: Vec<WriteItem> = paths
            .iter()
            .filter_map(|path| self.state.entry(path))
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
            self.suppress_watcher();
            self.meta.request(MetadataRequest::Write { items });
        }
    }

    fn pump_image_results(&mut self, ctx: &egui::Context) {
        /// Quante texture caricare al massimo in un frame: l'upload in VRAM e
        /// la conversione RGBA costano, e un batch di centinaia di miniature
        /// congelava la UI.
        const MAX_THUMB_UPLOADS_PER_FRAME: usize = 10;

        // Le anteprime (una alla volta) hanno precedenza; le miniature
        // restano in coda e vengono smaltite a poco a poco.
        let mut previews = Vec::new();
        for result in self.images.drain() {
            match result.kind {
                ImageKind::Thumbnail => self.pending_images.push_back(result),
                ImageKind::Preview => previews.push(result),
            }
        }

        for result in previews {
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
                        self.state.edit.source = Some(Arc::new(EditSource { path, mtime, image }));
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

        let mut dimensions: Vec<(PathBuf, u32, u32)> = Vec::new();
        let mut uploaded = 0usize;
        while uploaded < MAX_THUMB_UPLOADS_PER_FRAME {
            let Some(result) = self.pending_images.pop_front() else {
                break;
            };
            // La cartella è cambiata: il risultato non serve più e non
            // consuma il budget di upload di questo frame.
            if result.epoch != self.thumbs.epoch() {
                continue;
            }
            uploaded += 1;
            let path = result.path;
            self.thumbs.on_result(
                ctx,
                path.clone(),
                result.mtime,
                result.epoch,
                result.outcome,
            );
            if let Some((width, height)) = self.thumbs.dims(&path) {
                dimensions.push((path, width, height));
            }
        }

        if !dimensions.is_empty() {
            if let Some(index) = &self.index {
                if let Err(err) = index.set_dimensions_batch(&dimensions) {
                    tracing::warn!("scrittura indice non riuscita: {err:#}");
                }
            }
        }
        // Restano miniature da caricare: continua a ridisegnare finché la coda
        // non si svuota, senza bloccare questo frame.
        if !self.pending_images.is_empty() {
            ctx.request_repaint();
        }
    }

    /// Raccoglie gli esiti delle operazioni su file: aggiorna l'indice,
    /// pubblica lo stato in barra e riallinea la vista alla cartella corrente.
    fn pump_ops_results(&mut self) {
        let mut rescan = false;
        while let Some(outcome) = self.ops.try_recv() {
            let is_copy = outcome.description == "Copia";
            if !is_copy {
                for pair in &outcome.report.moved {
                    if let Some(index) = &self.index {
                        if let Err(err) = index.move_path(&pair.from, &pair.to) {
                            tracing::warn!("scrittura indice non riuscita: {err:#}");
                        }
                    }
                    self.state.edit.remap_draft(&pair.from, &pair.to);
                    self.state.remap_photo_drafts(&pair.from, &pair.to);
                }
            }
            let changed = !outcome.report.moved.is_empty();
            let undoable =
                outcome.description == "Spostamento" && outcome.report.failed.is_empty() && changed;
            let verb = match outcome.description {
                "Copia" => t!("Copia"),
                "Spostamento" => t!("Spostamento"),
                "Rinomina" => t!("Rinomina"),
                _ => t!("Annullamento"),
            };
            let mut message = t!(
                "{verb}: {count} file",
                verb = verb,
                count = outcome.report.moved.len()
            );
            if outcome.report.skipped_count() > 0 {
                message.push_str(&t!(
                    " · {count} saltati",
                    count = outcome.report.skipped_count()
                ));
            }
            if outcome.report.failed_count() > 0 {
                message.push_str(&t!(
                    " · {count} errori",
                    count = outcome.report.failed_count()
                ));
            }
            let error = outcome.report.first_error();
            if changed {
                rescan = true;
            }
            self.state.op_status = Some(OpStatus {
                message,
                error,
                undo: if undoable { Some(outcome.report) } else { None },
            });
        }
        if rescan {
            if let Some(dir) = self.state.current_dir.clone() {
                self.suppress_watcher();
                self.start_scan(dir);
            }
        }
    }

    fn handle_actions(&mut self, actions: UiActions) {
        let picked = if actions.pick_folder {
            rfd::FileDialog::new()
                .set_title(t!("Scegli la cartella da aprire"))
                .pick_folder()
        } else {
            None
        };
        if let Some(dir) = actions.open_folder.or(picked) {
            if dir.is_dir() {
                self.open_root(dir);
            } else {
                self.state.error = Some(t!(
                    "La cartella \"{path}\" non esiste più",
                    path = dir.display()
                ));
            }
        }
        if let Some(dir) = actions.browse_dir {
            if self.state.current_dir.as_deref() != Some(dir.as_path()) {
                self.reveal_tree_path(&dir);
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
                    t!("Copia le foto in…")
                } else {
                    t!("Sposta le foto in…")
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
                name: t!("Nuova cartella").to_owned(),
                first_frame: true,
            });
        }
        if let Some(dir) = actions.delete_folder {
            let items = dir_item_count(&dir);
            self.state.delete_dir_prompt = Some(DeleteDirPrompt { dir, items });
        }

        if let Some((paths, target, copy, policy)) = actions.confirm_transfer {
            self.suppress_watcher();
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
                    self.suppress_watcher();
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
            self.suppress_watcher();
            match create_dir(&parent, &name) {
                Ok(_) => {
                    self.refresh_tree_children(&parent);
                    if self.state.current_dir.as_deref() == Some(parent.as_path()) {
                        self.start_scan(parent);
                    }
                }
                Err(message) => self.state.error = Some(message),
            }
        }
        if let Some(dir) = actions.confirm_delete_dir {
            self.suppress_watcher();
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
                    } else if dir.parent() == self.state.current_dir.as_deref() {
                        if let Some(current) = self.state.current_dir.clone() {
                            self.start_scan(current);
                        }
                    }
                }
                Err(message) => self.state.error = Some(message),
            }
        }

        if actions.save_open {
            self.open_save_dialog();
        }
        if actions.save_cancel {
            self.state.save_dialog = None;
        }
        if let Some(mode) = actions.save_confirm {
            self.start_save(mode);
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

        if actions.gps_remove {
            self.state.set_gps_marker(None);
        }
        if actions.gps_reset {
            self.state.discard_gps_drafts();
        }
        if let Some((result, target)) = actions.gps_use_suggestion {
            match target {
                SuggestionTarget::Marker => {
                    let altitude = self.state.gps.marker.and_then(|marker| marker.altitude);
                    self.state.set_gps_marker(Some(GpsLocation {
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
                        .fill_gps_location(LocationRole::Created, &suggestion);
                }
                SuggestionTarget::Shown => {
                    let suggestion = result.location_suggestion();
                    self.state
                        .fill_gps_location(LocationRole::Shown, &suggestion);
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
                .set_title(t!("Cartella di destinazione dell'esportazione"))
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
                Some(t!("Preset non salvato: catalogo locale non disponibile").to_owned());
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
                self.state.edit.save_error = Some(t!(
                    "Preset non salvato: {error}",
                    error = format!("{err:#}")
                ));
            }
        }
    }

    /// Applica una ricetta (o la neutra) alla selezione come bozza in memoria:
    /// nessun file viene modificato finché non si preme Salva.
    fn apply_edit_preset_to_selection(&mut self, recipe: EditRecipe) {
        let recipe = recipe.sanitized();
        let selezionati = self.state.selected_paths();
        for path in &selezionati {
            if recipe.is_identity() {
                self.state.edit.drafts.remove(path);
            } else {
                self.state.edit.drafts.insert(path.clone(), recipe);
            }
        }
        if self.state.edit.path.is_some()
            && self
                .state
                .edit
                .path
                .as_ref()
                .is_some_and(|path| selezionati.contains(path))
        {
            self.state.edit.apply_preset(recipe);
        }
        self.state.edit.save_error = None;
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
                self.state.edit.save_error = Some(t!(
                    "Preset non eliminato: {error}",
                    error = format!("{err:#}")
                ));
            }
        }
    }

    fn begin_transfer(&mut self, paths: Vec<PathBuf>, target: PathBuf, copy: bool) {
        if paths.is_empty() {
            return;
        }
        if self.state.current_dir.as_deref() == Some(target.as_path()) {
            self.state.error = Some(t!("Le foto si trovano già in questa cartella").to_owned());
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
        self.suppress_watcher();
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
        self.state.edit.remap_drafts_in_dir(dir, &new_path);
        self.state.remap_photo_drafts_in_dir(dir, &new_path);
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
            } else if dir.starts_with(&current) {
                self.start_scan(current);
            }
        }
    }

    /// Espande il ramo dell'albero che contiene `path`, così la cartella
    /// aperta dalla barra delle sottocartelle resta visibile a sinistra.
    fn reveal_tree_path(&mut self, path: &Path) {
        let mut error = None;
        if let Some(root) = &mut self.state.root {
            if let Some(node) = root.find_mut(path) {
                node.expanded = true;
                if let Some(message) = node.load_children() {
                    error = Some(message);
                }
            }
        }
        if let Some(message) = error {
            self.state.error = Some(message);
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
        self.save_dialog_modal(ctx, actions);
    }

    /// Modal di salvataggio: riepilogo delle modifiche e scelta tra nuova
    /// copia e sovrascrittura dell'originale.
    fn save_dialog_modal(&mut self, ctx: &egui::Context, actions: &mut UiActions) {
        if self.state.save_dialog.is_none() {
            return;
        }

        let targets = self
            .state
            .save_dialog
            .as_ref()
            .map(|dialog| {
                if dialog.targets.is_empty() {
                    self.state.save_targets()
                } else {
                    dialog.targets.clone()
                }
            })
            .unwrap_or_default();
        let total = targets.len();
        let file_label = match targets.as_slice() {
            [path] => path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
            _ => t!("{count} fotografie", count = total),
        };
        let folder = targets
            .first()
            .and_then(|path| path.parent())
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        let nomi: Vec<String> = targets
            .iter()
            .take(3)
            .map(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string())
            })
            .collect();
        let altri = total.saturating_sub(nomi.len());
        let elenco = if altri > 0 {
            t!(
                "{names} e {others} in più",
                names = nomi.join(", "),
                others = altri
            )
        } else {
            nomi.join(", ")
        };
        let copia_esempio = targets.first().map(|path| nome_copia_libera(path));
        let modifiche_colore = targets
            .iter()
            .filter(|path| !self.state.edit.recipe_for(path).is_identity())
            .count();
        let campi_meta = targets
            .iter()
            .filter_map(|path| self.state.meta.drafts.get(path))
            .flat_map(|draft| draft.keys())
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        let campi_localita = targets
            .iter()
            .filter_map(|path| self.state.gps.drafts.get(path))
            .flat_map(|draft| draft.locations.keys())
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        let coordinate = targets.iter().any(|path| {
            self.state
                .gps
                .drafts
                .get(path)
                .is_some_and(|draft| draft.coordinates.is_some())
        });
        let campi_posizione = campi_localita + usize::from(coordinate);
        let formati_non_copiabili: Vec<&str> = targets
            .iter()
            .filter_map(|path| {
                self.state
                    .entries
                    .iter()
                    .find(|entry| &entry.path == path)
                    .map(|entry| entry.format)
            })
            .filter(|kind| !kind.capabilities().pixel_edit)
            .map(|kind| kind.label())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let anteprima = self
            .state
            .edit
            .edited
            .clone()
            .or_else(|| self.state.edit.original.clone());
        let assets_ready = targets
            .iter()
            .all(|path| self.state.entries.iter().any(|entry| &entry.path == path));
        let pending = targets.iter().any(|path| self.state.path_pending(path));
        let mut close = false;
        let mut confirm = None;

        let response = egui::Modal::new(egui::Id::unique("save_modal"))
            .frame(
                egui::Frame::popup(ctx.style_of(egui::Theme::Dark).as_ref())
                    .inner_margin(egui::Margin::same(18))
                    .corner_radius(egui::CornerRadius::same(12))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(58, 58, 64))),
            )
            .show(ctx, |ui| {
                ui.set_width(540.0);
                let Some(dialog) = self.state.save_dialog.as_mut() else {
                    return;
                };
                let busy = dialog.busy;

                ui.horizontal(|ui| {
                    if let Some(texture) = &anteprima {
                        egui::Frame::new()
                            .fill(egui::Color32::from_rgb(16, 16, 18))
                            .corner_radius(egui::CornerRadius::same(8))
                            .inner_margin(egui::Margin::same(4))
                            .show(ui, |ui| {
                                ui.add(
                                    egui::Image::new(texture)
                                        .max_size(egui::vec2(112.0, 84.0))
                                        .maintain_aspect_ratio(true),
                                );
                            });
                    }
                    ui.vertical(|ui| {
                        ui.heading(t!("Salva le modifiche"));
                        ui.add_space(2.0);
                        ui.label(egui::RichText::new(file_label.clone()).strong());
                        ui.weak(elide(&folder, 64));
                        ui.add_space(8.0);
                        ui.horizontal_wrapped(|ui| {
                            if modifiche_colore > 0 {
                                riepilogo_badge(
                                    ui,
                                    &t!("Colore · {count}", count = modifiche_colore),
                                    egui::Color32::from_rgb(110, 155, 209),
                                );
                            }
                            if campi_meta > 0 {
                                riepilogo_badge(
                                    ui,
                                    &t!("Metadati · {count}", count = campi_meta),
                                    egui::Color32::from_rgb(147, 190, 140),
                                );
                            }
                            if campi_posizione > 0 {
                                riepilogo_badge(
                                    ui,
                                    &t!("Posizione · {count}", count = campi_posizione),
                                    egui::Color32::from_rgb(209, 173, 110),
                                );
                            }
                        });
                    });
                });

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(10.0);
                ui.label(egui::RichText::new(t!("Cosa vuoi fare?")).strong());
                ui.weak(t!("Interessa: {list}", list = elenco));
                ui.add_space(8.0);

                let nuova = dialog.mode == SaveMode::NewCopy;
                let nuova_abilitata = formati_non_copiabili.is_empty() && !busy && assets_ready;
                let nuova_hint = match &copia_esempio {
                    Some(nome) => t!(
                        "Crea “{name}” accanto all'originale. Il file di partenza non viene toccato.",
                        name = nome
                    ),
                    None => t!("Crea una copia accanto all'originale.").to_owned(),
                };
                let nuova_response = mode_card(
                    ui,
                    ModeCard {
                        id: ui.make_persistent_id("save_mode_new"),
                        selected: nuova,
                        enabled: nuova_abilitata,
                        accent: egui::Color32::from_rgb(110, 155, 209),
                        titolo: if total == 1 {
                            t!("Nuova foto")
                        } else {
                            t!("Nuove foto")
                        },
                        sottotitolo: &nuova_hint,
                        badge: Some(t!("Consigliato")),
                    },
                );
                if nuova_response.clicked() && nuova_abilitata {
                    dialog.mode = SaveMode::NewCopy;
                    dialog.overwrite_confirmed = false;
                    dialog.error = None;
                }

                if !nuova_abilitata {
                    ui.add_space(4.0);
                    ui.weak(t!(
                        "Non disponibile per {formats}: per questi formati scegli la sovrascrittura (i metadati finiscono nel sidecar XMP).",
                        formats = formati_non_copiabili.join(", ")
                    ));
                }

                ui.add_space(8.0);
                let sovrascrivi = dialog.mode == SaveMode::Overwrite;
                let overwrite_response = mode_card(
                    ui,
                    ModeCard {
                        id: ui.make_persistent_id("save_mode_overwrite"),
                        selected: sovrascrivi,
                        enabled: !busy && assets_ready,
                        accent: ERROR_COLOR,
                        titolo: t!("Sovrascrivi originale"),
                        sottotitolo: if total == 1 {
                            t!("Riscrive questa foto con colore e metadati. Non è annullabile.")
                        } else {
                            t!("Riscrive le foto selezionate con colore e metadati. Non è annullabile.")
                        },
                        badge: None,
                    },
                );
                if overwrite_response.clicked() && !busy {
                    dialog.mode = SaveMode::Overwrite;
                    dialog.overwrite_confirmed = false;
                    dialog.error = None;
                }

                if dialog.mode == SaveMode::Overwrite {
                    ui.add_space(8.0);
                    egui::Frame::new()
                        .fill(ERROR_COLOR.gamma_multiply(0.12))
                        .stroke(egui::Stroke::new(1.0, ERROR_COLOR.gamma_multiply(0.65)))
                        .corner_radius(egui::CornerRadius::same(8))
                        .inner_margin(egui::Margin::symmetric(10, 8))
                        .show(ui, |ui| {
                            ui.colored_label(ERROR_COLOR, t!("Attenzione"));
                            ui.weak(t!(
                                "L'originale verrà riscritto sul disco. Non esiste un annullamento: per sicurezza usa “Nuova foto”."
                            ));
                            ui.add_space(4.0);
                            ui.checkbox(
                                &mut dialog.overwrite_confirmed,
                                if total == 1 {
                                    t!("Confermo di voler sovrascrivere l'originale")
                                } else {
                                    t!("Confermo di voler sovrascrivere gli originali")
                                },
                            );
                        });
                }

                if let Some(error) = &dialog.error {
                    ui.add_space(8.0);
                    ui.colored_label(ERROR_COLOR, error);
                }

                ui.add_space(14.0);
                ui.separator();
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if busy {
                        ui.spinner();
                        ui.weak(t!("Scrittura dei file…"));
                    } else if !assets_ready {
                        ui.spinner();
                        ui.weak(t!("Aggiornamento della cartella…"));
                    } else {
                        ui.weak(if dialog.mode == SaveMode::NewCopy {
                            t!("Viene creato un nuovo file; l'originale resta intatto.")
                        } else {
                            t!("Il salvataggio viene confermato solo dopo la verifica del file.")
                        });
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let etichetta = match dialog.mode {
                            SaveMode::NewCopy if total == 1 => t!("Salva nuova foto"),
                            SaveMode::NewCopy => t!("Salva nuove foto"),
                            SaveMode::Overwrite => t!("Sovrascrivi"),
                        };
                        let confermabile = pending
                            && !busy
                            && assets_ready
                            && match dialog.mode {
                                SaveMode::NewCopy => nuova_abilitata,
                                SaveMode::Overwrite => dialog.overwrite_confirmed,
                            };
                        let colore = match dialog.mode {
                            SaveMode::NewCopy => egui::Color32::from_rgb(110, 155, 209),
                            SaveMode::Overwrite => ERROR_COLOR,
                        };
                        let testo = if dialog.mode == SaveMode::NewCopy {
                            egui::Color32::from_rgb(14, 18, 24)
                        } else {
                            egui::Color32::from_rgb(24, 14, 14)
                        };
                        let pulsante =
                            egui::Button::new(egui::RichText::new(etichetta).strong().color(testo))
                                .fill(colore)
                                .min_size(egui::vec2(150.0, 30.0));
                        if ui.add_enabled(confermabile, pulsante).clicked() {
                            confirm = Some(dialog.mode);
                        }
                        if ui
                            .add_enabled(!busy, egui::Button::new(t!("Annulla")))
                            .clicked()
                        {
                            close = true;
                        }
                    });
                });

                if !busy && ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
                    close = true;
                }
            });
        let backdrop_clicked = response.backdrop_response.clicked();

        if let Some(mode) = confirm {
            actions.save_confirm = Some(mode);
        } else if close
            || (backdrop_clicked && !self.state.save_dialog.as_ref().is_some_and(|d| d.busy))
        {
            actions.save_cancel = true;
        }
    }

    /// Conferma la chiusura quando ci sono bozze non salvate.
    fn close_prompt_modal(&mut self, ctx: &egui::Context) {
        if !self.close_prompt {
            return;
        }
        let mut close = false;
        let mut stay = false;
        let response = egui::Modal::new(egui::Id::unique("close_modal"))
            .frame(
                egui::Frame::popup(ctx.style_of(egui::Theme::Dark).as_ref())
                    .inner_margin(egui::Margin::same(18))
                    .corner_radius(egui::CornerRadius::same(12))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(58, 58, 64))),
            )
            .show(ctx, |ui| {
                ui.set_width(430.0);
                ui.heading(t!("Modifiche non salvate"));
                ui.add_space(6.0);
                ui.label(t!(
                    "Le modifiche a colore, metadati o posizione non sono state salvate su file. Se chiudi ora andranno perse."
                ));
                ui.add_space(14.0);
                ui.separator();
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.weak(t!("Puoi tornare indietro e usare Salva (Ctrl+S)."));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let chiudi = egui::Button::new(
                            egui::RichText::new(t!("Chiudi senza salvare")).color(ERROR_COLOR),
                        );
                        if ui.add(chiudi).clicked() {
                            close = true;
                        }
                        if ui.button(t!("Continua a lavorare")).clicked() {
                            stay = true;
                        }
                    });
                });
                if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
                    stay = true;
                }
            });
        if response.backdrop_response.clicked() {
            stay = true;
        }
        if close {
            self.close_prompt = false;
            self.allow_close = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if stay {
            self.close_prompt = false;
        }
    }

    fn conflict_prompt_window(&mut self, ctx: &egui::Context, actions: &mut UiActions) {
        let mut confirmed = false;
        let mut cancelled = false;
        if let Some(prompt) = self.state.conflict_prompt.as_mut() {
            let title = if prompt.copy {
                t!("Copia le foto")
            } else {
                t!("Sposta le foto")
            };
            egui::Window::new(title)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    ui.label(t!(
                        "{count} file verso {target}",
                        count = prompt.paths.len(),
                        target = elide(&prompt.target.display().to_string(), 48)
                    ));
                    ui.add_space(8.0);
                    ui.checkbox(&mut prompt.copy, t!("Copia invece di spostare"));
                    ui.add_space(8.0);
                    ui.label(t!("Se il nome esiste già nella destinazione:"));
                    ui.radio_value(
                        &mut prompt.policy,
                        ConflictPolicy::RenameAuto,
                        t!("Rinomina automaticamente (foto (1).jpg)"),
                    );
                    ui.radio_value(
                        &mut prompt.policy,
                        ConflictPolicy::Replace,
                        t!("Sostituisci il file esistente"),
                    );
                    ui.radio_value(
                        &mut prompt.policy,
                        ConflictPolicy::Skip,
                        t!("Salta i file già presenti"),
                    );
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button(t!("Annulla")).clicked() {
                            cancelled = true;
                        }
                        let label = if prompt.copy {
                            t!("Copia")
                        } else {
                            t!("Sposta")
                        };
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
                RenameTarget::Photo(_) => t!("Rinomina foto"),
                RenameTarget::Folder(_) => t!("Rinomina cartella"),
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
                        if ui.button(t!("Annulla")).clicked() {
                            cancelled = true;
                        }
                        if ui.button(t!("Rinomina")).clicked() || enter {
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
            egui::Window::new(t!("Nuova cartella"))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    ui.label(t!(
                        "Dentro {path}",
                        path = elide(&prompt.parent.display().to_string(), 48)
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
                        if ui.button(t!("Annulla")).clicked() {
                            cancelled = true;
                        }
                        if ui.button(t!("Crea")).clicked() || enter {
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
            egui::Window::new(t!("Elimina cartella"))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    ui.label(t!(
                        "Eliminare \"{name}\"?",
                        name = elide(&prompt.dir.display().to_string(), 48)
                    ));
                    ui.add_space(6.0);
                    if prompt.items == 0 {
                        ui.weak(t!(
                            "La cartella è vuota e verrà rimossa dal disco. L'operazione non è annullabile."
                        ));
                    } else {
                        ui.colored_label(
                            ERROR_COLOR,
                            t!(
                                "La cartella contiene {count} elementi: UberPhoto elimina solo cartelle vuote.",
                                count = prompt.items
                            ),
                        );
                    }
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button(t!("Annulla")).clicked() {
                            cancelled = true;
                        }
                        let enabled = prompt.items == 0;
                        if ui
                            .add_enabled(enabled, egui::Button::new(t!("Elimina")))
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

    /// Barra superiore con lo stato delle modifiche e il pulsante Salva.
    fn save_bar(&mut self, ui: &mut egui::Ui, actions: &mut UiActions) {
        let ctrl_s = ui
            .ctx()
            .input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::S));
        let dirty = self.state.has_unsaved_changes();
        let count = if dirty {
            self.state.save_target_count()
        } else {
            0
        };
        let pending = count > 0;
        // Il conteggio delle bozze fuori selezione evita di clonare tutti i
        // percorsi: si calcola solo se esistono davvero modifiche non salvate.
        let altrove = if dirty {
            self.state.pending_outside_selection_count()
        } else {
            0
        };
        let gps_error = self.state.gps.manual_error.is_some();
        let busy = self.save.busy()
            || self
                .state
                .save_dialog
                .as_ref()
                .is_some_and(|dialog| dialog.busy);

        ui.horizontal(|ui| {
            if let Some(logo) = &self.state.logo {
                ui.add(egui::Image::new(logo).max_height(20.0));
            }
            ui.strong("UberPhoto");
            ui.separator();

            if busy {
                ui.spinner();
                ui.weak(t!("Salvataggio in corso…"));
            } else if gps_error {
                ui.colored_label(
                    ERROR_COLOR,
                    t!("• Coordinate non valide: correggi prima di salvare"),
                );
            } else if pending {
                ui.colored_label(
                    egui::Color32::from_rgb(110, 155, 209),
                    if count == 1 {
                        t!("• 1 foto con modifiche non salvate").to_owned()
                    } else {
                        t!("• {count} foto con modifiche non salvate", count = count)
                    },
                );
                if altrove > 0 {
                    ui.weak(t!("· {count} fuori selezione", count = altrove));
                }
            } else if altrove > 0 {
                ui.colored_label(
                    egui::Color32::from_rgb(110, 155, 209),
                    if altrove == 1 {
                        t!("• 1 bozza non salvata fuori selezione").to_owned()
                    } else {
                        t!(
                            "• {count} bozze non salvate fuori selezione",
                            count = altrove
                        )
                    },
                )
                .on_hover_text(t!("Seleziona quelle foto per salvarle con Salva"));
            } else {
                ui.weak(t!("Tutto salvato"));
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let salva = egui::Button::new(
                    egui::RichText::new(t!("Salva"))
                        .strong()
                        .color(egui::Color32::from_rgb(14, 18, 24)),
                )
                .fill(egui::Color32::from_rgb(110, 155, 209))
                .min_size(egui::vec2(96.0, 28.0));
                if ui
                    .add_enabled(pending && !busy && !gps_error, salva)
                    .on_hover_text(t!("Salva colore, metadati e posizione (Ctrl+S)"))
                    .clicked()
                {
                    actions.save_open = true;
                }
                if ctrl_s && pending && !busy && !gps_error {
                    actions.save_open = true;
                }
                ui.weak("Ctrl+S");
                ui.separator();
                ui.menu_button(format!("🌐 {}", self.state.language.native_name()), |ui| {
                    for language in Language::ALL {
                        let selected = language == self.state.language;
                        if ui
                            .selectable_label(selected, language.native_name())
                            .clicked()
                        {
                            self.state.set_language(language);
                            ui.close();
                        }
                    }
                })
                .response
                .on_hover_text(t!("Lingua dell'interfaccia"));
            });
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let mut undo_requested = false;
        let mut clear_status = false;
        ui.horizontal(|ui| {
            if self.state.loading_dir.is_some() {
                ui.spinner();
                ui.label(t!("Scansione in corso…"));
            } else if let Some(dir) = &self.state.current_dir {
                ui.label(t!(
                    "{folder} — {count} fotografie",
                    folder = elide(&dir.display().to_string(), 70),
                    count = self.state.entries.len()
                ));
            } else {
                ui.label(t!("Nessuna cartella aperta"));
            }

            ui.separator();
            match self.state.selection.len() {
                0 => {}
                1 => {
                    ui.label(t!("1 selezionata"));
                }
                count => {
                    ui.label(t!("{count} selezionate", count = count));
                }
            }

            let (cached, inflight) = self.thumbs.stats();
            ui.separator();
            ui.weak(t!(
                "Miniature: {cached} in cache · {inflight} in coda",
                cached = cached,
                inflight = inflight
            ));

            if let Some(status) = &self.state.op_status {
                ui.separator();
                ui.label(elide(&status.message, 70));
                if let Some(error) = &status.error {
                    ui.colored_label(ERROR_COLOR, elide(error, 70));
                }
                if status.undo.is_some() && ui.small_button(t!("↩ Annulla")).clicked() {
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
                    self.suppress_watcher();
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
/// Piccolo badge colorato usato nel riepilogo del dialogo di salvataggio.
fn riepilogo_badge(ui: &mut egui::Ui, testo: &str, colore: egui::Color32) {
    egui::Frame::new()
        .fill(colore.gamma_multiply(0.16))
        .stroke(egui::Stroke::new(1.0, colore.gamma_multiply(0.45)))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.colored_label(colore, egui::RichText::new(testo).small());
        });
}

/// Contenuto di una card selezionabile del dialogo di salvataggio.
struct ModeCard<'a> {
    id: egui::Id,
    selected: bool,
    enabled: bool,
    accent: egui::Color32,
    titolo: &'a str,
    sottotitolo: &'a str,
    badge: Option<&'a str>,
}

/// Card selezionabile del dialogo di salvataggio, con radio e descrizione.
fn mode_card(ui: &mut egui::Ui, card: ModeCard<'_>) -> egui::Response {
    let ModeCard {
        id,
        selected,
        enabled,
        accent,
        titolo,
        sottotitolo,
        badge,
    } = card;
    let fill = if selected {
        accent.gamma_multiply(0.14)
    } else {
        ui.visuals().faint_bg_color
    };
    let stroke = if selected {
        egui::Stroke::new(1.5, accent)
    } else {
        egui::Stroke::new(1.0, egui::Color32::from_rgb(52, 52, 58))
    };
    let testo = if enabled {
        ui.visuals().strong_text_color()
    } else {
        ui.visuals().weak_text_color()
    };
    let contenuto = egui::Frame::new()
        .fill(fill)
        .stroke(stroke)
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
                let centro = rect.center();
                ui.painter()
                    .circle_stroke(centro, 7.0, egui::Stroke::new(1.5, accent));
                if selected {
                    ui.painter().circle_filled(centro, 3.8, accent);
                }
                ui.add_space(2.0);
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(titolo).strong().color(testo));
                        if let Some(badge) = badge {
                            egui::Frame::new()
                                .fill(accent.gamma_multiply(0.18))
                                .corner_radius(egui::CornerRadius::same(8))
                                .inner_margin(egui::Margin::symmetric(6, 2))
                                .show(ui, |ui| {
                                    ui.colored_label(accent, egui::RichText::new(badge).small());
                                });
                        }
                    });
                    ui.label(egui::RichText::new(sottotitolo).weak());
                });
            });
        });
    let response = ui.interact(
        contenuto.response.rect,
        id,
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    if enabled && response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

/// Primo nome libero della copia che verrà creata accanto all'originale.
fn nome_copia_libera(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| t!("foto").to_owned());
    let estensione = path
        .extension()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "jpg".to_owned());
    for indice in 1..10_000 {
        let suffisso = t!("modificata");
        let nome = if indice == 1 {
            format!("{stem}-{suffisso}.{estensione}")
        } else {
            format!("{stem}-{suffisso}-{indice}.{estensione}")
        };
        if !path.with_file_name(&nome).exists() {
            return nome;
        }
    }
    format!("{stem}-{}-9999.{estensione}", t!("modificata"))
}

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
        self.pump_save_results();
        self.sync_metadata();
        self.sync_edit(&ctx);

        // Le scritture dell'app (metadati, salvataggi, spostamenti) non
        // devono far scattare una seconda scansione: il watcher le ignora
        // finché il servizio corrispondente è al lavoro, più un margine.
        let writing = self.ops.busy() || self.meta.busy() || self.save.busy();
        if let Some(watcher) = &self.watcher {
            watcher.set_suppressed(writing);
        }

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

        egui::Panel::top("save_bar").show(ui, |ui| self.save_bar(ui, &mut actions));
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
        self.remember_edit_draft();
        self.preview.set_prefer_edited(!self.state.edit.compare);
        self.preview.ui(&ctx);

        // Mantiene animati spinner e avanzamento durante le operazioni lunghe.
        let watcher_pending = self.watcher.as_ref().is_some_and(DirWatcher::has_pending);
        if self.state.loading_dir.is_some()
            || self.thumbs.has_inflight()
            || self.preview.is_loading()
            || self.state.edit.busy
            || self.state.export.busy
            || self.save.busy()
            || watcher_pending
        {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        if self.smoke_exit.is_none()
            && !self.allow_close
            && ctx.input(|input| input.viewport().close_requested())
        {
            if self.state.save_dialog.is_some() || self.save.busy() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                let message = if self.save.busy() {
                    t!("Salvataggio in corso: attendi la fine prima di chiudere.")
                } else {
                    t!("Completa o annulla il salvataggio prima di chiudere.")
                };
                self.state.op_status = Some(OpStatus {
                    message: message.to_owned(),
                    error: None,
                    undo: None,
                });
            } else if self.state.has_unsaved_changes() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.close_prompt = true;
            }
        }
        self.close_prompt_modal(&ctx);

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
