use uberphoto_core::PhotoAsset;

use crate::naming::natural_cmp;

/// Chiavi di ordinamento disponibili in Fase 1.
///
/// "Data di acquisizione" arriverà con il motore metadati (Fase 3): richiede
/// EXIF `DateTimeOriginal`, non la data di modifica del filesystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Modified,
    Size,
}

impl SortKey {
    pub const ALL: [SortKey; 3] = [SortKey::Name, SortKey::Modified, SortKey::Size];

    pub fn label(self) -> &'static str {
        match self {
            SortKey::Name => "Nome",
            SortKey::Modified => "Data modifica",
            SortKey::Size => "Dimensione",
        }
    }
}

/// Ordina in place; `ascending=false` inverte l'ordine.
/// L'ordinamento è stabile, quindi a parità di chiave rimane l'ordine naturale
/// per nome prodotto dalla scansione.
pub fn sort_assets(assets: &mut [PhotoAsset], key: SortKey, ascending: bool) {
    match key {
        SortKey::Name => assets.sort_by(|a, b| natural_cmp(&a.file_name(), &b.file_name())),
        SortKey::Modified => assets.sort_by_key(|asset| asset.modified),
        SortKey::Size => assets.sort_by_key(|asset| asset.byte_size),
    }
    if !ascending {
        assets.reverse();
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    use super::*;
    use uberphoto_core::FormatKind;

    fn asset(name: &str, size: u64, modified_secs: u64) -> PhotoAsset {
        PhotoAsset {
            path: PathBuf::from(name),
            format: FormatKind::Jpeg,
            byte_size: size,
            modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(modified_secs)),
        }
    }

    #[test]
    fn ordina_per_dimensione_decrescente() {
        let mut assets = vec![
            asset("a.jpg", 10, 3),
            asset("b.jpg", 30, 1),
            asset("c.jpg", 20, 2),
        ];
        sort_assets(&mut assets, SortKey::Size, false);
        let names: Vec<String> = assets.iter().map(|a| a.file_name()).collect();
        assert_eq!(names, vec!["b.jpg", "c.jpg", "a.jpg"]);
    }

    #[test]
    fn ordina_per_data_modifica_crescente() {
        let mut assets = vec![
            asset("a.jpg", 10, 3),
            asset("b.jpg", 30, 1),
            asset("c.jpg", 20, 2),
        ];
        sort_assets(&mut assets, SortKey::Modified, true);
        let names: Vec<String> = assets.iter().map(|a| a.file_name()).collect();
        assert_eq!(names, vec!["b.jpg", "c.jpg", "a.jpg"]);
    }
}
