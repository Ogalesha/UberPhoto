//! Optional independent backups. A configured but unavailable disk fails closed.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use crate::safety::{files_equal, sync_directory};

static EXTERNAL: OnceLock<RwLock<Option<PathBuf>>> = OnceLock::new();

pub fn set_external_backup_directory(path: Option<PathBuf>) -> io::Result<()> {
    let path = path
        .map(|path| {
            if !path.is_absolute() {
                return Err(io::Error::other(
                    "il percorso di backup deve essere assoluto",
                ));
            }
            // Keep an unavailable configured disk armed: never silently fall back
            // to local-only backups after a restart.
            Ok::<_, io::Error>(path)
        })
        .transpose()?;
    *EXTERNAL
        .get_or_init(|| RwLock::new(None))
        .write()
        .map_err(|_| io::Error::other("configurazione backup non disponibile"))? = path;
    Ok(())
}

fn configured() -> io::Result<Option<PathBuf>> {
    Ok(EXTERNAL
        .get_or_init(|| RwLock::new(None))
        .read()
        .map_err(|_| io::Error::other("configurazione backup non disponibile"))?
        .clone())
}

fn safe_directory(path: &Path) -> io::Result<()> {
    if !fs::symlink_metadata(path)?.file_type().is_dir() {
        return Err(io::Error::other(
            "la cartella di backup non può essere un link simbolico",
        ));
    }
    Ok(())
}

fn make_directory(path: &Path) -> io::Result<()> {
    match fs::create_dir(path) {
        Ok(()) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
            }
            sync_directory(path.parent().unwrap())?;
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    safe_directory(path)
}

fn identity(original: &Path) -> io::Result<PathBuf> {
    Ok(
        fs::canonicalize(original.parent().unwrap_or_else(|| Path::new(".")))?.join(
            original
                .file_name()
                .ok_or_else(|| io::Error::other("nome originale mancante"))?,
        ),
    )
}

fn directory(root: &Path, original: &Path) -> io::Result<PathBuf> {
    safe_directory(root)?;
    let original = identity(original)?;
    // Stable across processes and Rust versions. The origin file rejects any
    // hash collision instead of merging unrelated photographs.
    let bytes = serde_json::to_vec(&original)?;
    let hash = bytes.iter().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    });
    Ok(root.join("UberPhoto-backups").join(format!("{hash:016x}")))
}

fn verify_origin(directory: &Path, original: &Path) -> io::Result<()> {
    safe_directory(directory)?;
    let path = directory.join("origin.json");
    if !fs::symlink_metadata(&path)?.file_type().is_file() || fs::metadata(&path)?.len() > 64 * 1024
    {
        return Err(io::Error::other("indice backup non sicuro"));
    }
    let stored: PathBuf = serde_json::from_reader(File::open(path)?)?;
    if stored != identity(original)? {
        return Err(io::Error::other(
            "il backup appartiene a un altro originale",
        ));
    }
    Ok(())
}

fn require_independent(root: &Path, source: &Path) -> io::Result<()> {
    #[cfg(unix)]
    let same = {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(root)?.dev() == fs::metadata(source)?.dev()
    };
    #[cfg(windows)]
    let same = {
        let a = fs::canonicalize(root)?;
        let b = fs::canonicalize(source)?;
        a.components().next() == b.components().next()
    };
    #[cfg(not(any(unix, windows)))]
    let same = true;
    if same {
        return Err(io::Error::other(
            "scegli un disco di backup diverso da quello delle foto",
        ));
    }
    Ok(())
}

/// Rejects the same volume; the user must choose a physically separate disk.
pub fn validate_external_backup_directory(root: &Path, source: &Path) -> io::Result<()> {
    safe_directory(root)?;
    require_independent(root, source)
}

pub(crate) fn mirror_backup(original: &Path, backup: &Path) -> io::Result<()> {
    let Some(root) = configured()? else {
        return Ok(());
    };
    safe_directory(&root)
        .map_err(|error| io::Error::other(format!("disco di backup non disponibile: {error}")))?;
    require_independent(&root, original)?;
    mirror_to(&root, original, backup)
}

fn mirror_to(root: &Path, original: &Path, backup: &Path) -> io::Result<()> {
    let directory = directory(root, original)?;
    make_directory(directory.parent().unwrap())?;
    make_directory(&directory)?;
    let origin = directory.join("origin.json");
    if !origin.try_exists()? {
        let mut file = tempfile::NamedTempFile::new_in(&directory)?;
        serde_json::to_writer(file.as_file_mut(), &identity(original)?)?;
        file.as_file().sync_all()?;
        match file.persist_noclobber(&origin) {
            Ok(_) => {}
            Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.error),
        }
        sync_directory(&directory)?;
    }
    verify_origin(&directory, original)?;
    let target = directory.join(backup.file_name().unwrap());
    let mut file = tempfile::NamedTempFile::new_in(&directory)?;
    io::copy(&mut File::open(backup)?, file.as_file_mut())?;
    file.as_file().sync_all()?;
    if !files_equal(backup, file.path())? {
        return Err(io::Error::other("backup indipendente non verificato"));
    }
    file.persist_noclobber(target)
        .map_err(|error| error.error)?;
    sync_directory(&directory)
}

pub(crate) fn external_directory(original: &Path) -> io::Result<Option<PathBuf>> {
    let Some(root) = configured()? else {
        return Ok(None);
    };
    let directory = directory(&root, original)?;
    if !directory.try_exists()? {
        return Ok(None);
    }
    verify_origin(&directory, original)?;
    Ok(Some(directory))
}

pub(crate) fn is_external_backup(backup: &Path, original: &Path) -> io::Result<bool> {
    Ok(external_directory(original)?
        .is_some_and(|directory| backup.parent() == Some(directory.as_path())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_volume_and_an_unavailable_backup_disk_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let photo = dir.path().join("photo.jpg");
        fs::write(&photo, b"precious photo").unwrap();
        assert!(validate_external_backup_directory(dir.path(), &photo).is_err());
        assert!(validate_external_backup_directory(&dir.path().join("unplugged"), &photo).is_err());
        assert_eq!(fs::read(photo).unwrap(), b"precious photo");
    }

    #[test]
    fn independent_copy_is_verified_and_bound_to_its_original_path() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("backup");
        fs::create_dir(&root).unwrap();
        let original = dir.path().join("photo.jpg");
        let backup = dir.path().join("photo.jpg.ABC.bak");
        fs::write(&original, b"photo").unwrap();
        fs::write(&backup, b"original photo").unwrap();
        // Exercise the copy protocol on a temporary disk substitute. The
        // production entry point additionally enforces a separate volume.
        mirror_to(&root, &original, &backup).unwrap();
        let copies = directory(&root, &original).unwrap();
        verify_origin(&copies, &original).unwrap();
        assert_eq!(
            fs::read(copies.join(backup.file_name().unwrap())).unwrap(),
            b"original photo"
        );
        assert!(verify_origin(&copies, &dir.path().join("another.jpg")).is_err());
        assert!(mirror_to(&root, &original, &backup).is_err());
        assert_eq!(fs::read(backup).unwrap(), b"original photo");
    }
}
