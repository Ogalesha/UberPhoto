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
use uberphoto_db::{Index, MetadataIndex};
use uberphoto_files::{
    create_dir, delete_empty_dir, dir_item_count, rename_dir, ConflictPolicy, DirNode, DirWatcher,
    DirectoryScanner, FileOpRequest, FileOpsService,
};
use uberphoto_image::{ImageKind, ImageService};
use uberphoto_metadata::{
    exiftool_versione, MetadataField, MetadataOutcome, MetadataRequest, MetadataService, WriteItem,
};

use crate::browser;
use crate::explorer;
use crate::inspector;
use crate::preview::PreviewState;
use crate::state::{
    AppState, ConflictPrompt, DeleteDirPrompt, NewFolderPrompt, OpStatus, PersistedState,
    RenamePrompt, RenameTarget, UiActions,
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
            watcher: DirWatcher::new(repaint),
            smoke_exit,
        };
        app.state.logo = load_logo(&cc.egui_ctx);
        app.state.exiftool = exiftool_versione();
        if app.state.exiftool.is_none() {
            tracing::warn!("ExifTool non trovato: l'editor metadati resterà disabilitato");
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
                }
                MetadataOutcome::Written { report } => {
                    self.state.op_status = Some(OpStatus {
                        message: report.summary(),
                        error: report.first_error(),
                        undo: None,
                    });
                    self.state.meta.form_dirty.clear();
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

    /// Scrive sulla selezione i soli campi modificati nel form.
    fn save_metadata(&mut self) {
        if self.state.meta.form_dirty.is_empty() {
            return;
        }
        let edits: Vec<(MetadataField, Option<String>)> = self
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
                (*field, (!value.is_empty()).then(|| value.to_owned()))
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
                edits: vec![(MetadataField::Rating, Some(stars.to_string()))],
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
                    if self.preview.accepts(&result.path, result.mtime) {
                        match result.outcome {
                            Ok(image) => self.preview.set_image(ctx, image),
                            Err(error) => self.preview.set_error(error),
                        }
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
        self.sync_metadata();

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
        self.preview.ui(&ctx);

        // Mantiene animati spinner e avanzamento durante le operazioni lunghe.
        let watcher_pending = self.watcher.as_ref().is_some_and(DirWatcher::has_pending);
        if self.state.loading_dir.is_some()
            || self.thumbs.has_inflight()
            || self.preview.is_loading()
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
