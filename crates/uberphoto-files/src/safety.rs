//! Verified staging and persistent recovery copies for file mutations.

// Rust guideline compliant 2026-09-18

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

/// Checks that a mutation will affect a writable regular file.
///
/// # Errors
/// Rejects symbolic links, read-only files, directories and missing files.
pub fn writable_regular_file(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || metadata.permissions().readonly() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "operazione rifiutata su link simbolico, cartella o file di sola lettura",
        ));
    }
    Ok(())
}

/// Compares file contents without loading photographs into memory.
///
/// # Errors
/// Returns an I/O error if either file cannot be read.
pub fn files_equal(left: &Path, right: &Path) -> io::Result<bool> {
    let mut left = File::open(left)?;
    let mut right = File::open(right)?;
    let length = left.metadata()?.len();
    if length != right.metadata()?.len() {
        return Ok(false);
    }
    // Bound verification memory independently of image size.
    let mut a = [0; 64 * 1024];
    let mut b = [0; 64 * 1024];
    let mut remaining = length;
    while remaining > 0 {
        let count = remaining.min(a.len() as u64) as usize;
        left.read_exact(&mut a[..count])?;
        right.read_exact(&mut b[..count])?;
        if a[..count] != b[..count] {
            return Ok(false);
        }
        remaining -= count as u64;
    }
    Ok(left.read(&mut a[..1])? == 0 && right.read(&mut b[..1])? == 0)
}

/// Creates a durable, verified recovery copy next to the original folder.
///
/// Backups live in `.uberphoto-backups`, retain the original filename as a
/// prefix, and are never automatically removed. They share the original disk;
/// an independent backup is still needed for disk failure or loss.
///
/// # Errors
/// Fails before mutation if the copy, verification or synchronization fails.
pub fn backup_file(source: &Path) -> io::Result<PathBuf> {
    writable_regular_file(source)?;
    let parent = source.parent().unwrap_or_else(|| Path::new("."));
    let directory = parent.join(".uberphoto-backups");
    match fs::create_dir(&directory) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    if !fs::symlink_metadata(&directory)?.file_type().is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "cartella backup non sicura",
        ));
    }
    let mut prefix = source.file_name().unwrap_or_default().to_os_string();
    prefix.push(".");
    let mut backup = tempfile::Builder::new()
        .prefix(&prefix)
        .suffix(".pending")
        .tempfile_in(&directory)?;
    let mut input = File::open(source)?;
    io::copy(&mut input, backup.as_file_mut())?;
    backup.as_file().sync_all()?;
    if !files_equal(source, backup.path())? {
        return Err(io::Error::other(
            "la sorgente è cambiata durante il backup; operazione annullata",
        ));
    }
    // A crash during copying must not leave a partial file labelled as a backup.
    let path = backup.path().with_extension("bak");
    backup
        .persist_noclobber(&path)
        .map_err(|error| error.error)?;
    sync_directory(&directory)?;
    sync_directory(parent)?;
    Ok(path)
}

/// A private work directory on the destination filesystem.
#[derive(Debug)]
pub struct StagedFile {
    path: PathBuf,
    permissions: Option<fs::Permissions>,
    _directory: tempfile::TempDir,
}

impl StagedFile {
    /// Reserves a private directory for a new file with the target extension.
    ///
    /// # Errors
    /// Returns an error if the parent is unavailable or not writable.
    pub fn new(target: &Path) -> io::Result<Self> {
        let parent = target.parent().unwrap_or_else(|| Path::new("."));
        let directory = tempfile::Builder::new()
            .prefix(".uberphoto-work-")
            .tempdir_in(parent)?;
        let path = directory.path().join(target.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "destinazione senza nome file")
        })?);
        Ok(Self {
            path,
            permissions: None,
            _directory: directory,
        })
    }

    /// Copies and verifies a source inside the private work directory.
    ///
    /// # Errors
    /// Fails on I/O errors or if the source changes during copying.
    pub fn copy(source: &Path, target: &Path) -> io::Result<Self> {
        let mut staged = Self::new(target)?;
        let mut input = File::open(source)?;
        let metadata = input.metadata()?;
        let mut output = File::options()
            .create_new(true)
            .write(true)
            .open(staged.path())?;
        io::copy(&mut input, &mut output)?;
        if !files_equal(source, staged.path())? {
            return Err(io::Error::other(
                "copia non verificata; sorgente conservata",
            ));
        }
        if let Ok(modified) = metadata.modified() {
            output.set_modified(modified)?;
        }
        output.sync_all()?;
        staged.permissions = Some(metadata.permissions());
        Ok(staged)
    }

    /// Returns the work file path; external tools may write here.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Preserves source permissions when publishing the completed file.
    ///
    /// # Errors
    /// Returns an error if the source metadata cannot be read.
    pub fn preserve_permissions(&mut self, source: &Path) -> io::Result<()> {
        self.permissions = Some(fs::metadata(source)?.permissions());
        Ok(())
    }

    /// Publishes a complete file without replacing any existing path.
    ///
    /// # Errors
    /// Fails if a target exists, including a dangling symbolic link.
    pub fn publish(self, target: &Path) -> io::Result<()> {
        File::options()
            .read(true)
            .write(true)
            .open(&self.path)?
            .sync_all()?;
        if let Some(permissions) = self.permissions.clone() {
            fs::set_permissions(&self.path, permissions)?;
        }
        let path = tempfile::TempPath::try_from_path(self.path.clone())?;
        path.persist_noclobber(target)
            .map_err(|error| error.error)?;
        sync_directory(target.parent().unwrap_or_else(|| Path::new(".")))
    }

    /// Captures the current file, then publishes the staged version without clobbering.
    /// The capture remains recoverable even if publication fails or the process stops.
    ///
    /// # Errors
    /// Fails before replacement if the target changed or is not writable.
    pub fn replace(mut self, target: &Path, backup: &Path) -> io::Result<()> {
        writable_regular_file(target)?;
        if !files_equal(target, backup)? {
            return Err(io::Error::other(
                "il file è cambiato; ricaricalo prima di riprovare",
            ));
        }
        let capture = capture_path(target)?;
        // Rename captures whichever version exists at the instant of commit.
        // A later external writer cannot be overwritten by publication.
        fs::rename(target, &capture)?;
        sync_directory(target.parent().unwrap_or_else(|| Path::new(".")))?;
        sync_directory(capture.parent().unwrap_or_else(|| Path::new(".")))?;
        if !files_equal(&capture, backup)? {
            // An external edit won the race. Restore it only if the name is
            // still vacant; the captured file and earlier backup remain.
            let restored = Self::copy(&capture, target)?;
            restored.publish(target)?;
            return Err(io::Error::other(
                "il file è cambiato durante la sostituzione; modifica esterna conservata",
            ));
        }
        self.permissions = Some(fs::metadata(&capture)?.permissions());
        self.publish(target)
    }
}

/// Reserves a random backup name without leaving a file that could be mistaken
/// for a completed backup after a crash.
fn capture_path(target: &Path) -> io::Result<PathBuf> {
    let directory = target
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(".uberphoto-backups");
    if !fs::symlink_metadata(&directory)?.file_type().is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "cartella backup non sicura",
        ));
    }
    let mut prefix = target.file_name().unwrap_or_default().to_os_string();
    prefix.push(".");
    let reservation = tempfile::Builder::new()
        .prefix(&prefix)
        .suffix(".pending")
        .tempfile_in(&directory)?;
    let capture = reservation.path().with_extension("bak");
    reservation.close()?;
    if fs::symlink_metadata(&capture).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "nome della copia di recupero già occupato",
        ));
    }
    Ok(capture)
}

pub(crate) fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_retains_the_exact_original_and_rejects_external_changes() {
        let dir = tempfile::tempdir().unwrap();
        let photo = dir.path().join("photo.jpg");
        fs::write(&photo, b"precious original").unwrap();
        let backup = backup_file(&photo).unwrap();
        let staged = StagedFile::new(&photo).unwrap();
        fs::write(staged.path(), b"edited photo").unwrap();
        fs::write(&photo, b"external edit").unwrap();
        assert!(staged.replace(&photo, &backup).is_err());
        assert_eq!(fs::read(&photo).unwrap(), b"external edit");
        assert_eq!(fs::read(backup).unwrap(), b"precious original");
    }

    #[test]
    fn successful_replacement_keeps_both_verified_and_captured_originals() {
        let dir = tempfile::tempdir().unwrap();
        let photo = dir.path().join("photo.jpg");
        fs::write(&photo, b"precious original").unwrap();
        let backup = backup_file(&photo).unwrap();
        let staged = StagedFile::new(&photo).unwrap();
        fs::write(staged.path(), b"edited photo").unwrap();
        staged.replace(&photo, &backup).unwrap();
        assert_eq!(fs::read(&photo).unwrap(), b"edited photo");
        assert_eq!(fs::read(&backup).unwrap(), b"precious original");
        let copies = crate::recovery::list_backups(&photo).unwrap();
        assert_eq!(copies.len(), 2);
        assert!(copies
            .iter()
            .all(|copy| fs::read(&copy.path).unwrap() == b"precious original"));
    }

    #[test]
    fn publishing_cannot_clobber_a_file_created_after_staging() {
        let dir = tempfile::tempdir().unwrap();
        let photo = dir.path().join("photo.jpg");
        let staged = StagedFile::new(&photo).unwrap();
        fs::write(staged.path(), b"export").unwrap();
        fs::write(&photo, b"another photo").unwrap();
        assert!(staged.publish(&photo).is_err());
        assert_eq!(fs::read(photo).unwrap(), b"another photo");
    }
}
