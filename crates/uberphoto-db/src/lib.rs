//! Indice locale SQLite (Fasi 2-3).
//!
//! Principio architetturale: **SQLite riflette lo stato del filesystem, non
//! lo sostituisce**. Ogni riga è una cache verificabile rispetto ai file
//! reali: il file resta la fonte di verità e il database può essere
//! ricostruito in ogni momento con una riscansione.
//!
//! Contenuto attuale (Fase 2): tabella `photo_index` con i dati di filesystem
//! delle foto scansionate (percorso, mtime, dimensione, formato, dimensioni)
//! e le colonne dei metadati, che saranno popolate dal motore ExifTool
//! (Fase 3).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use uberphoto_core::PhotoAsset;

/// Versione dello schema salvata in `PRAGMA user_version`.
pub const SCHEMA_VERSION: i64 = 1;

const COLUMNS: &str = "path, mtime, byte_size, format, width, height, has_gps, \
     rating, date_original, keywords, camera_make, camera_model, lens, iso, \
     f_number, exposure_time, focal_length, gps_lat, gps_lon";

/// Riga dell'indice per una fotografia.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct IndexRow {
    pub path: PathBuf,
    /// mtime in secondi dall'epoca Unix, per il confronto con il filesystem.
    pub mtime: Option<i64>,
    pub byte_size: Option<i64>,
    pub format: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub has_gps: bool,
    pub rating: Option<u8>,
    pub date_original: Option<String>,
    pub keywords: Option<String>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub lens: Option<String>,
    pub iso: Option<i64>,
    pub f_number: Option<String>,
    pub exposure_time: Option<String>,
    pub focal_length: Option<String>,
    pub gps_lat: Option<f64>,
    pub gps_lon: Option<f64>,
}

impl IndexRow {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        let width: Option<i64> = row.get(4)?;
        let height: Option<i64> = row.get(5)?;
        let has_gps: i64 = row.get(6)?;
        let rating: Option<i64> = row.get(7)?;
        Ok(Self {
            path: PathBuf::from(row.get::<_, String>(0)?),
            mtime: row.get(1)?,
            byte_size: row.get(2)?,
            format: row.get(3)?,
            width: width.map(|value| value.clamp(0, i64::from(u32::MAX)) as u32),
            height: height.map(|value| value.clamp(0, i64::from(u32::MAX)) as u32),
            has_gps: has_gps != 0,
            rating: rating.map(|value| value.clamp(0, i64::from(u8::MAX)) as u8),
            date_original: row.get(8)?,
            keywords: row.get(9)?,
            camera_make: row.get(10)?,
            camera_model: row.get(11)?,
            lens: row.get(12)?,
            iso: row.get(13)?,
            f_number: row.get(14)?,
            exposure_time: row.get(15)?,
            focal_length: row.get(16)?,
            gps_lat: row.get(17)?,
            gps_lon: row.get(18)?,
        })
    }
}

/// Valori metadati scritti nel database dal motore ExifTool (Fase 3).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MetadataIndex {
    pub has_gps: bool,
    pub rating: Option<u8>,
    pub date_original: Option<String>,
    pub keywords: Option<String>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub lens: Option<String>,
    pub iso: Option<i64>,
    pub f_number: Option<String>,
    pub exposure_time: Option<String>,
    pub focal_length: Option<String>,
    pub gps_lat: Option<f64>,
    pub gps_lon: Option<f64>,
}

/// Indice SQLite delle fotografie scansionate.
pub struct Index {
    conn: Connection,
}

impl Index {
    /// Apre (o crea) l'indice su file, attivando WAL quando possibile.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creazione cartella indice {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("apertura indice {}", path.display()))?;
        Self::new(conn)
    }

    /// Indice in memoria, per test e prove.
    pub fn open_in_memory() -> Result<Self> {
        Self::new(Connection::open_in_memory()?)
    }

    fn new(conn: Connection) -> Result<Self> {
        // WAL non è disponibile per i database in memoria: l'errore è ignorato.
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS photo_index (
                path TEXT PRIMARY KEY,
                mtime INTEGER,
                byte_size INTEGER,
                format TEXT,
                width INTEGER,
                height INTEGER,
                has_gps INTEGER NOT NULL DEFAULT 0,
                rating INTEGER,
                date_original TEXT,
                keywords TEXT,
                camera_make TEXT,
                camera_model TEXT,
                lens TEXT,
                iso INTEGER,
                f_number TEXT,
                exposure_time TEXT,
                focal_length TEXT,
                gps_lat REAL,
                gps_lon REAL
            );
            CREATE INDEX IF NOT EXISTS idx_photo_index_format ON photo_index(format);",
        )
        .context("creazione schema indice")?;
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)
            .context("scrittura versione schema")?;
        Ok(Self { conn })
    }

    /// Versione dello schema presente nel database.
    pub fn schema_version(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))?)
    }

    /// Numero di righe presenti nell'indice.
    pub fn count(&self) -> Result<u64> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM photo_index", [], |row| row.get(0))?;
        Ok(count.max(0) as u64)
    }

    /// Inserisce o aggiorna i dati di filesystem di una foto scansionata.
    pub fn upsert_file_info(&self, asset: &PhotoAsset) -> Result<()> {
        self.conn.execute(
            "INSERT INTO photo_index (path, mtime, byte_size, format) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(path) DO UPDATE SET
                mtime = excluded.mtime,
                byte_size = excluded.byte_size,
                format = excluded.format",
            params![
                path_text(&asset.path),
                mtime_secs(asset.modified),
                asset.byte_size as i64,
                asset.format.label(),
            ],
        )?;
        Ok(())
    }

    /// Salva le dimensioni in pixel, creando la riga se non esiste.
    pub fn set_dimensions(&self, path: &Path, width: u32, height: u32) -> Result<()> {
        self.conn.execute(
            "INSERT INTO photo_index (path, width, height) VALUES (?1, ?2, ?3)
             ON CONFLICT(path) DO UPDATE SET width = excluded.width, height = excluded.height",
            params![path_text(path), i64::from(width), i64::from(height)],
        )?;
        Ok(())
    }

    /// Inserisce o aggiorna i metadati letti dal file (Fase 3).
    pub fn upsert_metadata(
        &self,
        path: &Path,
        mtime: Option<i64>,
        metadata: &MetadataIndex,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO photo_index (
                path, mtime, has_gps, rating, date_original, keywords,
                camera_make, camera_model, lens, iso,
                f_number, exposure_time, focal_length, gps_lat, gps_lon
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
             ON CONFLICT(path) DO UPDATE SET
                mtime = excluded.mtime,
                has_gps = excluded.has_gps,
                rating = excluded.rating,
                date_original = excluded.date_original,
                keywords = excluded.keywords,
                camera_make = excluded.camera_make,
                camera_model = excluded.camera_model,
                lens = excluded.lens,
                iso = excluded.iso,
                f_number = excluded.f_number,
                exposure_time = excluded.exposure_time,
                focal_length = excluded.focal_length,
                gps_lat = excluded.gps_lat,
                gps_lon = excluded.gps_lon",
            params![
                path_text(path),
                mtime,
                metadata.has_gps as i64,
                metadata.rating.map(i64::from),
                metadata.date_original,
                metadata.keywords,
                metadata.camera_make,
                metadata.camera_model,
                metadata.lens,
                metadata.iso,
                metadata.f_number,
                metadata.exposure_time,
                metadata.focal_length,
                metadata.gps_lat,
                metadata.gps_lon,
            ],
        )?;
        Ok(())
    }

    /// Riga relativa a `path`, se presente.
    pub fn get(&self, path: &Path) -> Result<Option<IndexRow>> {
        let sql = format!("SELECT {COLUMNS} FROM photo_index WHERE path = ?1");
        Ok(self
            .conn
            .query_row(&sql, params![path_text(path)], IndexRow::from_row)
            .optional()?)
    }

    /// Righe delle foto contenute direttamente in `dir` (non ricorsivo).
    pub fn load_dir(&self, dir: &Path) -> Result<Vec<IndexRow>> {
        let sql = format!("SELECT {COLUMNS} FROM photo_index WHERE path LIKE ?1 ESCAPE '\\'");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![like_dir_pattern(dir)], IndexRow::from_row)?;
        let mut out = Vec::new();
        for row in rows {
            let row = row?;
            if row.path.parent() == Some(dir) {
                out.push(row);
            }
        }
        Ok(out)
    }

    /// Aggiorna il percorso di una singola riga (rinomina o spostamento),
    /// mantenendo i dati già indicizzati.
    pub fn move_path(&self, from: &Path, to: &Path) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM photo_index WHERE path = ?1",
            params![path_text(to)],
        )?;
        tx.execute(
            "UPDATE photo_index SET path = ?1 WHERE path = ?2",
            params![path_text(to), path_text(from)],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Elimina la riga relativa a `path`.
    pub fn remove(&self, path: &Path) -> Result<()> {
        self.conn.execute(
            "DELETE FROM photo_index WHERE path = ?1",
            params![path_text(path)],
        )?;
        Ok(())
    }

    /// Elimina `dir` e tutte le sue sottocartelle; restituisce le righe rimosse.
    pub fn remove_dir(&self, dir: &Path) -> Result<usize> {
        let paths = self.paths_under(dir)?;
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare("DELETE FROM photo_index WHERE path = ?1")?;
            for path in &paths {
                stmt.execute(params![path_text(path)])?;
            }
        }
        tx.commit()?;
        Ok(paths.len())
    }

    /// Aggiorna i percorsi dopo la rinomina/spostamento di una cartella.
    pub fn move_dir(&self, from: &Path, to: &Path) -> Result<usize> {
        let paths = self.paths_under(from)?;
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare("UPDATE photo_index SET path = ?1 WHERE path = ?2")?;
            for path in &paths {
                if let Ok(relative) = path.strip_prefix(from) {
                    stmt.execute(params![path_text(&to.join(relative)), path_text(path)])?;
                }
            }
        }
        tx.commit()?;
        Ok(paths.len())
    }

    fn paths_under(&self, dir: &Path) -> Result<Vec<PathBuf>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path FROM photo_index WHERE path LIKE ?1 ESCAPE '\\'")?;
        let rows = stmt.query_map(params![like_dir_pattern(dir)], |row| {
            row.get::<_, String>(0)
        })?;
        let mut out = Vec::new();
        for row in rows {
            let path = PathBuf::from(row?);
            if path.starts_with(dir) {
                out.push(path);
            }
        }
        Ok(out)
    }
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn mtime_secs(modified: Option<SystemTime>) -> Option<i64> {
    modified
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
}

/// Pattern LIKE per i discendenti di `dir`, con i metacaratteri neutralizzati.
fn like_dir_pattern(dir: &Path) -> String {
    let mut text = path_text(dir);
    if !text.ends_with(['/', '\\']) {
        text.push(std::path::MAIN_SEPARATOR);
    }
    let escaped = text
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("{escaped}%")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use uberphoto_core::FormatKind;

    fn asset(path: &str, size: u64) -> PhotoAsset {
        PhotoAsset {
            path: PathBuf::from(path),
            format: FormatKind::Jpeg,
            byte_size: size,
            modified: Some(UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
        }
    }

    fn sample_metadata() -> MetadataIndex {
        MetadataIndex {
            has_gps: true,
            rating: Some(4),
            date_original: Some("2024-05-01T10:00:00".to_owned()),
            keywords: Some("vacanza, mare".to_owned()),
            camera_make: Some("Fujifilm".to_owned()),
            camera_model: Some("X-T5".to_owned()),
            lens: Some("XF 23mm".to_owned()),
            iso: Some(400),
            f_number: Some("2.8".to_owned()),
            exposure_time: Some("1/250".to_owned()),
            focal_length: Some("23.0 mm".to_owned()),
            gps_lat: Some(45.4642),
            gps_lon: Some(9.19),
        }
    }

    #[test]
    fn schema_alla_versione_corrente() {
        let index = Index::open_in_memory().unwrap();
        assert_eq!(index.schema_version().unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn upsert_e_lettura_dati_filesystem() {
        let index = Index::open_in_memory().unwrap();
        index.upsert_file_info(&asset("/foto/a.jpg", 123)).unwrap();
        let row = index.get(Path::new("/foto/a.jpg")).unwrap().unwrap();
        assert_eq!(row.byte_size, Some(123));
        assert_eq!(row.format.as_deref(), Some(FormatKind::Jpeg.label()));
        assert_eq!(row.mtime, Some(1_700_000_000));
    }

    #[test]
    fn dimensioni_e_metadati_non_perdono_i_dati_filesystem() {
        let index = Index::open_in_memory().unwrap();
        index.upsert_file_info(&asset("/foto/a.jpg", 123)).unwrap();
        index
            .set_dimensions(Path::new("/foto/a.jpg"), 6000, 4000)
            .unwrap();
        index
            .upsert_metadata(Path::new("/foto/a.jpg"), Some(1), &sample_metadata())
            .unwrap();
        let row = index.get(Path::new("/foto/a.jpg")).unwrap().unwrap();
        assert_eq!(row.byte_size, Some(123));
        assert_eq!(row.width, Some(6000));
        assert_eq!(row.height, Some(4000));
        assert!(row.has_gps);
        assert_eq!(row.rating, Some(4));
        assert_eq!(row.gps_lat, Some(45.4642));
        assert_eq!(row.mtime, Some(1));
    }

    #[test]
    fn load_dir_solo_file_diretti() {
        let index = Index::open_in_memory().unwrap();
        for path in [
            "/base/a.jpg",
            "/base/b.jpg",
            "/base/sub/c.jpg",
            "/basix/d.jpg",
        ] {
            index.upsert_file_info(&asset(path, 1)).unwrap();
        }
        let rows = index.load_dir(Path::new("/base")).unwrap();
        let mut names: Vec<String> = rows
            .iter()
            .map(|row| row.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, vec!["a.jpg", "b.jpg"]);
    }

    #[test]
    fn move_dir_aggiorna_i_percorsi() {
        let index = Index::open_in_memory().unwrap();
        for path in ["/base/a.jpg", "/base/sub/c.jpg"] {
            index.upsert_file_info(&asset(path, 1)).unwrap();
        }
        let moved = index
            .move_dir(Path::new("/base"), Path::new("/nuova"))
            .unwrap();
        assert_eq!(moved, 2);
        assert!(index.get(Path::new("/nuova/a.jpg")).unwrap().is_some());
        assert!(index.get(Path::new("/nuova/sub/c.jpg")).unwrap().is_some());
        assert!(index.get(Path::new("/base/a.jpg")).unwrap().is_none());
    }

    #[test]
    fn move_path_aggiorna_una_riga() {
        let index = Index::open_in_memory().unwrap();
        index.upsert_file_info(&asset("/base/a.jpg", 1)).unwrap();
        index
            .set_dimensions(Path::new("/base/a.jpg"), 10, 20)
            .unwrap();
        index
            .move_path(Path::new("/base/a.jpg"), Path::new("/base/b.jpg"))
            .unwrap();
        let row = index.get(Path::new("/base/b.jpg")).unwrap().unwrap();
        assert_eq!(row.width, Some(10));
        assert!(index.get(Path::new("/base/a.jpg")).unwrap().is_none());
    }

    #[test]
    fn remove_dir_elimina_ricorsivo() {
        let index = Index::open_in_memory().unwrap();
        for path in ["/base/a.jpg", "/base/sub/c.jpg", "/altrove/d.jpg"] {
            index.upsert_file_info(&asset(path, 1)).unwrap();
        }
        let removed = index.remove_dir(Path::new("/base")).unwrap();
        assert_eq!(removed, 2);
        assert_eq!(index.count().unwrap(), 1);
    }

    #[test]
    fn riapertura_su_file_conserva_i_dati() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        {
            let index = Index::open(&db_path).unwrap();
            index.upsert_file_info(&asset("/foto/a.jpg", 42)).unwrap();
        }
        let index = Index::open(&db_path).unwrap();
        let row = index.get(Path::new("/foto/a.jpg")).unwrap().unwrap();
        assert_eq!(row.byte_size, Some(42));
    }
}
