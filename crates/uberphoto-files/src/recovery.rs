//! Lists completed recovery copies and restores them into new files.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::safety::StagedFile;

// Bound the collision search, matching the other file naming operations.
const MAX_RECOVERY_NAMES: u32 = 10_000;

/// A completed recovery copy belonging to one original file.
#[derive(Debug, Clone)]
pub struct BackupEntry {
    /// Completed `.bak` file retained by UberPhoto.
    pub path: PathBuf,
    /// Original filename used to name a newly restored copy.
    pub original: PathBuf,
    /// Last modification time, when available from the filesystem.
    pub modified: Option<SystemTime>,
    /// Backup size in bytes.
    pub byte_size: u64,
}

/// Lists completed backups for one original, newest first.
///
/// Partial `.pending` files, symbolic links, and similarly named sidecars
/// are excluded. A missing backup directory returns an empty list.
///
/// # Errors
/// Rejects a symbolic-link backup directory and propagates directory errors.
pub fn list_backups(original: &Path) -> io::Result<Vec<BackupEntry>> {
    let directory = backup_directory(original)?;
    match fs::symlink_metadata(&directory) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => return Err(unsafe_backup_path()),
    }
    let mut backups = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if !matches_original(&path, original) {
            continue;
        }
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_file() => metadata,
            Ok(_) => continue,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        backups.push(BackupEntry {
            path,
            original: original.to_path_buf(),
            modified: metadata.modified().ok(),
            byte_size: metadata.len(),
        });
    }
    backups.sort_by(|a, b| {
        b.modified
            .cmp(&a.modified)
            .then_with(|| a.path.cmp(&b.path))
    });
    Ok(backups)
}

/// Restores a backup into a new file beside the original.
///
/// The original and backup remain untouched. The restored file keeps the
/// original extension and uses `-recuperata`, followed by a counter when
/// necessary. Publication never replaces an existing path.
///
/// # Errors
/// Rejects unrelated or symbolic-link backups, propagates copy/verification
/// errors, and fails if the bounded collision search cannot find a free name.
pub fn restore_backup(backup: &Path, original: &Path) -> io::Result<PathBuf> {
    validate_backup(backup, original)?;
    let parent = original_parent(original);
    let stem = original.file_stem().ok_or_else(invalid_original)?;
    let extension = original.extension();
    for index in 1..=MAX_RECOVERY_NAMES {
        let mut name = OsString::from(stem);
        name.push("-recuperata");
        if index > 1 {
            name.push(format!("-{index}"));
        }
        if let Some(extension) = extension {
            name.push(".");
            name.push(extension);
        }
        let target = parent.join(name);
        match fs::symlink_metadata(&target) {
            Ok(_) => continue,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        // Validate again after choosing the destination. A stale UI selection
        // must not restore a backup that has since become a symbolic link.
        validate_backup(backup, original)?;
        let staged = StagedFile::copy(backup, &target)?;
        validate_backup(backup, original)?;
        match staged.publish(&target) {
            Ok(()) => return Ok(target),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "troppi nomi occupati per la copia di recupero; nessun file sostituito",
    ))
}

fn backup_directory(original: &Path) -> io::Result<PathBuf> {
    if original.file_name().is_none() {
        return Err(invalid_original());
    }
    Ok(original_parent(original).join(".uberphoto-backups"))
}

fn original_parent(original: &Path) -> &Path {
    original
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn matches_original(backup: &Path, original: &Path) -> bool {
    let (Some(name), Some(original_name)) = (backup.file_name(), original.file_name()) else {
        return false;
    };
    let mut prefix = original_name.to_os_string();
    prefix.push(".");
    let Some(nonce) = name
        .as_encoded_bytes()
        .strip_suffix(b".bak")
        .and_then(|stem| stem.strip_prefix(prefix.as_encoded_bytes()))
    else {
        return false;
    };
    // Tempfile's generated nonce is alphanumeric. Requiring a single nonce
    // component prevents photo.jpg from matching photo.jpg.xmp.<nonce>.bak.
    !nonce.is_empty() && nonce.iter().all(u8::is_ascii_alphanumeric)
}

fn validate_backup(backup: &Path, original: &Path) -> io::Result<()> {
    let directory = backup_directory(original)?;
    if !fs::symlink_metadata(&directory)?.file_type().is_dir()
        || !fs::symlink_metadata(backup)?.file_type().is_file()
        || !matches_original(backup, original)
    {
        return Err(unsafe_backup_path());
    }
    let parent = backup.parent().ok_or_else(unsafe_backup_path)?;
    if fs::canonicalize(parent)? != fs::canonicalize(directory)? {
        return Err(unsafe_backup_path());
    }
    Ok(())
}

fn invalid_original() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "percorso originale senza nome file",
    )
}

fn unsafe_backup_path() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "copia di recupero non valida per questo originale",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safety::backup_file;

    #[test]
    fn restore_creates_a_verified_copy_and_preserves_original_and_backup() {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("photo.jpg");
        fs::write(&original, b"precious original").unwrap();
        let backup = backup_file(&original).unwrap();
        fs::write(&original, b"current edited photo").unwrap();

        let restored = restore_backup(&backup, &original).unwrap();
        assert_eq!(restored, directory.path().join("photo-recuperata.jpg"));
        assert_eq!(fs::read(restored).unwrap(), b"precious original");
        assert_eq!(fs::read(original).unwrap(), b"current edited photo");
        assert_eq!(fs::read(backup).unwrap(), b"precious original");
    }

    #[test]
    fn restore_skips_occupied_names_without_overwriting_them() {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("photo.jpg");
        fs::write(&original, b"photo").unwrap();
        let backup = backup_file(&original).unwrap();
        let existing = directory.path().join("photo-recuperata.jpg");
        fs::write(&existing, b"another precious photo").unwrap();

        let restored = restore_backup(&backup, &original).unwrap();
        assert_eq!(restored, directory.path().join("photo-recuperata-2.jpg"));
        assert_eq!(fs::read(existing).unwrap(), b"another precious photo");
        assert_eq!(fs::read(restored).unwrap(), b"photo");
        assert_eq!(fs::read(backup).unwrap(), b"photo");
    }

    #[test]
    fn listing_excludes_pending_files_other_originals_and_sidecars() {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("photo.jpg");
        let sidecar = directory.path().join("photo.jpg.xmp");
        fs::write(&original, b"photo").unwrap();
        fs::write(&sidecar, b"metadata").unwrap();
        let backup = backup_file(&original).unwrap();
        let sidecar_backup = backup_file(&sidecar).unwrap();
        fs::write(backup.with_extension("pending"), b"partial").unwrap();
        fs::write(
            backup.parent().unwrap().join("another.jpg.ABC123.bak"),
            b"other",
        )
        .unwrap();

        let backups = list_backups(&original).unwrap();
        assert_eq!(backups.len(), 1);
        assert_eq!(backups[0].path, backup);
        assert_eq!(backups[0].original, original);
        assert_eq!(backups[0].byte_size, 5);
        assert_eq!(list_backups(&sidecar).unwrap()[0].path, sidecar_backup);
    }

    #[test]
    fn unrelated_backup_is_rejected_even_when_its_name_matches() {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("photo.jpg");
        fs::write(&original, b"photo").unwrap();
        let valid_backup = backup_file(&original).unwrap();
        let unrelated = directory.path().join(valid_backup.file_name().unwrap());
        fs::copy(&valid_backup, &unrelated).unwrap();

        assert!(restore_backup(&unrelated, &original).is_err());
        assert_eq!(fs::read(original).unwrap(), b"photo");
        assert_eq!(fs::read(unrelated).unwrap(), b"photo");
    }

    #[test]
    fn a_missing_original_can_be_recovered_from_its_retained_backup() {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("photo.jpg");
        fs::write(&original, b"photo").unwrap();
        let backup = backup_file(&original).unwrap();
        let moved = directory.path().join("moved.jpg");
        fs::rename(&original, &moved).unwrap();

        let restored = restore_backup(&backup, &original).unwrap();
        assert_eq!(fs::read(restored).unwrap(), b"photo");
        assert_eq!(fs::read(moved).unwrap(), b"photo");
        assert_eq!(fs::read(backup).unwrap(), b"photo");
        assert!(!original.exists());
    }

    #[test]
    fn recovery_keeps_unicode_filenames_and_extensions() {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("Viaggio è bello.旅行.JPG");
        fs::write(&original, b"photo").unwrap();
        let backup = backup_file(&original).unwrap();
        assert_eq!(list_backups(&original).unwrap()[0].path, backup);
        let restored = restore_backup(&backup, &original).unwrap();
        assert_eq!(
            restored.file_name().unwrap(),
            "Viaggio è bello.旅行-recuperata.JPG"
        );
        assert_eq!(fs::read(restored).unwrap(), b"photo");
    }

    #[cfg(unix)]
    #[test]
    fn backup_symlinks_and_symlink_directories_are_rejected() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("photo.jpg");
        fs::write(&original, b"photo").unwrap();
        let backup = backup_file(&original).unwrap();
        let symlink_backup = backup.parent().unwrap().join("photo.jpg.ABC123.bak");
        symlink(&original, &symlink_backup).unwrap();
        assert!(restore_backup(&symlink_backup, &original).is_err());
        assert_eq!(list_backups(&original).unwrap().len(), 1);
        let occupied = directory.path().join("photo-recuperata.jpg");
        symlink(&original, &occupied).unwrap();
        let restored = restore_backup(&backup, &original).unwrap();
        assert_eq!(restored, directory.path().join("photo-recuperata-2.jpg"));
        assert!(fs::symlink_metadata(&occupied)
            .unwrap()
            .file_type()
            .is_symlink());

        let other_directory = directory.path().join("other");
        fs::create_dir(&other_directory).unwrap();
        let other_original = other_directory.join("photo.jpg");
        fs::write(&other_original, b"other original").unwrap();
        symlink(
            backup.parent().unwrap(),
            other_directory.join(".uberphoto-backups"),
        )
        .unwrap();
        assert!(list_backups(&other_original).is_err());
        let through_link = other_directory
            .join(".uberphoto-backups")
            .join(backup.file_name().unwrap());
        assert!(restore_backup(&through_link, &other_original).is_err());
        assert_eq!(fs::read(original).unwrap(), b"photo");
        assert_eq!(fs::read(other_original).unwrap(), b"other original");
    }
}
