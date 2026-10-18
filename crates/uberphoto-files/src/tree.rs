use std::fs;
use std::path::{Path, PathBuf};

use crate::naming::{is_hidden_name, natural_cmp};

/// Nodo dell'albero delle cartelle mostrato nel pannello sinistro.
///
/// I figli vengono caricati in modo lazy alla prima espansione. Fase 1: il
/// listing è sincrono (una sola `read_dir`, senza metadati); se in futuro
/// risulterà lento su volumi di rete verrà spostato su un thread dedicato.
#[derive(Debug, Clone)]
pub struct DirNode {
    pub path: PathBuf,
    pub name: String,
    pub expanded: bool,
    /// `None` = figli non ancora caricati; `Some(vec![])` = cartella vuota.
    pub children: Option<Vec<DirNode>>,
}

impl DirNode {
    pub fn new_root(path: PathBuf) -> Self {
        let name = display_name(&path);
        Self {
            path,
            name,
            expanded: true,
            children: None,
        }
    }

    /// Carica i figli se non sono ancora stati caricati.
    /// Restituisce un messaggio di errore leggibile se la lettura fallisce.
    pub fn load_children(&mut self) -> Option<String> {
        if self.children.is_some() {
            return None;
        }
        match load_subdirs(&self.path) {
            Ok(children) => {
                self.children = Some(children);
                None
            }
            Err(err) => Some(format!(
                "Impossibile leggere \"{}\": {err}",
                self.path.display()
            )),
        }
    }

    /// Trova il nodo corrispondente a `path` nell'albero, espandendo i rami
    /// necessari. Usato per riallineare la selezione dopo un "Apri cartella".
    pub fn find_mut(&mut self, path: &Path) -> Option<&mut DirNode> {
        if self.path == path {
            return Some(self);
        }
        if !path.starts_with(&self.path) {
            return None;
        }
        self.load_children();
        if let Some(children) = self.children.as_mut() {
            for child in children {
                if let Some(found) = child.find_mut(path) {
                    return Some(found);
                }
            }
        }
        None
    }
}

/// Sotto-cartelle dirette di `path`, ordinate naturalmente, nascoste escluse.
/// I symlink verso directory sono seguiti in lettura.
pub fn load_subdirs(path: &Path) -> std::io::Result<Vec<DirNode>> {
    let mut dirs = Vec::new();

    for entry in fs::read_dir(path)? {
        let Ok(entry) = entry else { continue };

        let name = entry.file_name().to_string_lossy().into_owned();
        if is_hidden_name(&name) {
            continue;
        }

        let entry_path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let is_dir = file_type.is_dir() || (file_type.is_symlink() && entry_path.is_dir());
        if !is_dir {
            continue;
        }

        dirs.push(DirNode {
            path: entry_path,
            name,
            expanded: false,
            children: None,
        });
    }

    dirs.sort_by(|a, b| natural_cmp(&a.name, &b.name));
    Ok(dirs)
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carica_solo_sottocartelle_ordinate() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();

        fs::create_dir(root.join("zeta")).unwrap();
        fs::create_dir(root.join("alfa")).unwrap();
        fs::create_dir(root.join(".nascosta")).unwrap();
        fs::write(root.join("foto.jpg"), b"x").unwrap();

        let dirs = load_subdirs(root).expect("listing");
        let names: Vec<&str> = dirs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["alfa", "zeta"]);

        let mut node = DirNode::new_root(root.to_path_buf());
        assert!(node.children.is_none());
        assert!(node.load_children().is_none());
        assert!(node.children.is_some());

        let alfa = root.join("alfa");
        assert!(node.find_mut(&alfa).is_some());
        assert!(node.find_mut(&root.join("non-esiste")).is_none());
    }
}
