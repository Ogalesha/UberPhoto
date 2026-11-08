//! Durable batch transfers. Incomplete batches roll back without deleting photos.

use std::fs::{self, File, TryLockError};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::safety::{backup_file, files_equal, sync_directory, writable_regular_file, StagedFile};

const JOURNALS: &str = ".uberphoto-transactions";

#[derive(Serialize, Deserialize)]
struct Entry {
    source: PathBuf,
    target: PathBuf,
    source_backup: Option<PathBuf>,
    target_backup: Option<PathBuf>,
    previous: PathBuf,
    retained: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    version: u32,
    entries: Vec<Entry>,
}

fn regular(path: &Path) -> io::Result<()> {
    if !fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(io::Error::other("file del registro non sicuro"));
    }
    Ok(())
}

fn safe_directory(path: &Path) -> io::Result<()> {
    if !fs::symlink_metadata(path)?.file_type().is_dir() {
        return Err(io::Error::other("cartella del registro non sicura"));
    }
    Ok(())
}

fn lock_root(root: &Path) -> io::Result<File> {
    safe_directory(root)?;
    let path = root.join("lock");
    match fs::symlink_metadata(&path) {
        Ok(_) => regular(&path)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let file = File::options()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    file.try_lock().map_err(|error| match error {
        TryLockError::WouldBlock => io::Error::new(
            io::ErrorKind::WouldBlock,
            "un'altra operazione sta usando questa cartella",
        ),
        TryLockError::Error(error) => error,
    })?;
    Ok(file)
}

fn marker(directory: &Path, name: &str) -> io::Result<()> {
    let path = directory.join(name);
    if path.try_exists()? {
        return regular(&path);
    }
    File::options()
        .create_new(true)
        .write(true)
        .open(path)?
        .sync_all()?;
    sync_directory(directory)
}

fn marked(directory: &Path, name: &str) -> io::Result<bool> {
    let path = directory.join(name);
    match fs::symlink_metadata(&path) {
        Ok(_) => {
            regular(&path)?;
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn restore(copy: &Path, target: &Path) -> io::Result<()> {
    regular(copy)?;
    match fs::symlink_metadata(target) {
        Ok(_) => {
            regular(target)?;
            if files_equal(copy, target)? {
                return Ok(());
            }
            Err(io::Error::other(format!(
                "modifica esterna conservata in {}; recupero da completare",
                target.display()
            )))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            StagedFile::copy(copy, target)?.publish(target)
        }
        Err(error) => Err(error),
    }
}

fn rollback(directory: &Path, manifest: &Manifest) -> io::Result<()> {
    // Every mutation has a durable intent marker. Retained output files make
    // this repeatable after a second crash, including midway through rollback.
    for (index, entry) in manifest.entries.iter().enumerate().rev() {
        if marked(directory, &format!("retire-{index}"))? {
            if let Some(backup) = &entry.source_backup {
                crate::recovery::validate_backup(backup, &entry.source)?;
                restore(backup, &entry.source)?;
            }
        }
        if !marked(directory, &format!("publish-{index}"))? {
            continue;
        }
        let snapshot = directory.join(format!("source-{index}"));
        regular(&snapshot)?;
        let previous = &entry.previous;
        let retained = &entry.retained;
        if !retained.try_exists()? && entry.target.try_exists()? {
            regular(&entry.target)?;
            if files_equal(&entry.target, &snapshot)? {
                fs::rename(&entry.target, retained)?;
                sync_directory(entry.target.parent().unwrap())?;
                sync_directory(retained.parent().unwrap())?;
            } else if !previous.try_exists()? {
                // Publication never happened: an existing target was untouched.
                continue;
            }
        }
        if previous.try_exists()? {
            restore(previous, &entry.target)?;
        } else if let Some(backup) = &entry.target_backup {
            crate::recovery::validate_backup(backup, &entry.target)?;
            restore(backup, &entry.target)?;
        } else if entry.target.try_exists()? && !retained.try_exists()? {
            return Err(io::Error::other(
                "destinazione esterna conservata; recupero da completare",
            ));
        }
    }
    marker(directory, "rolled-back")
}

fn read_manifest(directory: &Path, target_dir: &Path) -> io::Result<Manifest> {
    let path = directory.join("manifest.json");
    regular(&path)?;
    let file = File::open(path)?;
    if file.metadata()?.len() > 16 * 1024 * 1024 {
        return Err(io::Error::other("registro troppo grande"));
    }
    let manifest: Manifest = serde_json::from_reader(file)?;
    if manifest.version != 1 || manifest.entries.is_empty() {
        return Err(io::Error::other("versione del registro non supportata"));
    }
    for entry in &manifest.entries {
        if !entry.source.is_absolute()
            || !entry.target.is_absolute()
            || entry
                .source
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
            || entry
                .target
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            return Err(io::Error::other("percorsi del registro non validi"));
        }
        for path in [&entry.previous, &entry.retained]
            .into_iter()
            .chain(entry.target_backup.iter())
        {
            validate_capture(path, &entry.target)?;
        }
        if let Some(path) = &entry.source_backup {
            validate_capture(path, &entry.source)?;
        }
    }
    if manifest.entries[0].target.parent() != Some(target_dir) {
        return Err(io::Error::other(
            "registro fuori dalla cartella coordinatrice",
        ));
    }
    Ok(manifest)
}

fn validate_capture(path: &Path, original: &Path) -> io::Result<()> {
    let parent = original.parent().unwrap().join(".uberphoto-backups");
    safe_directory(&parent)?;
    if path.parent() != Some(parent.as_path()) || !crate::recovery::matches_original(path, original)
    {
        return Err(io::Error::other(
            "copia di recupero fuori dalla cartella prevista",
        ));
    }
    Ok(())
}

fn recover_locked(root: &Path, target_dir: &Path) -> io::Result<usize> {
    let mut recovered = 0;
    for item in fs::read_dir(root)? {
        let item = item?;
        if !item.file_type()?.is_dir() {
            continue;
        }
        let directory = item.path();
        if marked(&directory, "committed")?
            || marked(&directory, "rolled-back")?
            || !directory.join("manifest.json").try_exists()?
        {
            continue;
        }
        let manifest = read_manifest(&directory, target_dir)?;
        rollback(&directory, &manifest).map_err(|error| {
            io::Error::other(format!(
                "recupero del lotto in {} non completato: {error}. Le copie sono conservate",
                directory.display()
            ))
        })?;
        recovered += 1;
    }
    Ok(recovered)
}

/// Recovers incomplete batches before a folder is shown or used again.
pub(crate) fn recover_directory(target_dir: &Path) -> io::Result<usize> {
    let target_dir = fs::canonicalize(target_dir)?;
    let root = target_dir.join(JOURNALS);
    match fs::symlink_metadata(&root) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
        Ok(_) => safe_directory(&root)?,
    }
    let _lock = match lock_root(&root) {
        Ok(lock) => lock,
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(0),
        Err(error) => return Err(error),
    };
    recover_locked(&root, &target_dir)
}

pub(crate) fn execute(pairs: &[(PathBuf, PathBuf)], replace: bool, copy: bool) -> io::Result<()> {
    execute_with(pairs, replace, copy, |_, _| Ok(()))
}

/// Publishes a prepared batch, rolling every member back if any commit fails.
/// Prepared sources are retained; interrupted commits recover on the next scan.
pub fn publish_file_batch(pairs: &[(PathBuf, PathBuf)], replace: bool) -> io::Result<()> {
    execute(pairs, replace, true)
}

fn execute_with(
    pairs: &[(PathBuf, PathBuf)],
    replace: bool,
    copy: bool,
    mut checkpoint: impl FnMut(&str, usize) -> io::Result<()>,
) -> io::Result<()> {
    if pairs.is_empty() {
        return Ok(());
    }
    let target_dir = fs::canonicalize(
        pairs[0]
            .1
            .parent()
            .ok_or_else(|| io::Error::other("destinazione senza cartella"))?,
    )?;
    let root = target_dir.join(JOURNALS);
    if !root.try_exists()? {
        fs::create_dir(&root)?;
        sync_directory(&target_dir)?;
    }
    let _lock = lock_root(&root)?;
    recover_locked(&root, &target_dir)?;
    let directory = tempfile::Builder::new()
        .prefix("batch-")
        .tempdir_in(&root)?
        .keep();
    sync_directory(&root)?;
    let mut manifest = Manifest {
        version: 1,
        entries: Vec::new(),
    };
    let mut targets = std::collections::HashSet::new();
    for (index, (source, target)) in pairs.iter().enumerate() {
        regular(source)?;
        let source = fs::canonicalize(source)?;
        let parent = fs::canonicalize(target.parent().unwrap())?;
        let target = parent.join(
            target
                .file_name()
                .ok_or_else(|| io::Error::other("nome mancante"))?,
        );
        if !targets.insert(target.clone())
            || pairs
                .iter()
                .any(|(from, _)| same_file::is_same_file(from, &target).unwrap_or(false))
        {
            return Err(io::Error::other(
                "destinazione duplicata o coincidente con una sorgente",
            ));
        }
        let snapshot = directory.join(format!("source-{index}"));
        StagedFile::copy(&source, &snapshot)?.publish(&snapshot)?;
        let source_backup = if copy {
            None
        } else {
            Some(backup_file(&source)?)
        };
        let target_backup = if fs::symlink_metadata(&target).is_ok() {
            if !replace {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "destinazione già esistente",
                ));
            }
            Some(backup_file(&target)?)
        } else {
            None
        };
        let previous = crate::safety::capture_path(&target)?;
        let retained = crate::safety::capture_path(&target)?;
        manifest.entries.push(Entry {
            source,
            target,
            source_backup,
            target_backup,
            previous,
            retained,
        });
    }
    let mut file = tempfile::NamedTempFile::new_in(&directory)?;
    serde_json::to_writer(file.as_file_mut(), &manifest)?;
    file.as_file_mut().flush()?;
    file.as_file().sync_all()?;
    file.persist_noclobber(directory.join("manifest.json"))
        .map_err(|error| error.error)?;
    sync_directory(&directory)?;
    let result = (|| {
        for (index, entry) in manifest.entries.iter().enumerate() {
            checkpoint("before-publish", index)?;
            marker(&directory, &format!("publish-{index}"))?;
            let snapshot = directory.join(format!("source-{index}"));
            if let Some(backup) = &entry.target_backup {
                writable_regular_file(&entry.target)?;
                if !files_equal(&entry.target, backup)? {
                    return Err(io::Error::other(
                        "destinazione modificata da un altro programma",
                    ));
                }
                let previous = &entry.previous;
                fs::rename(&entry.target, previous)?;
                sync_directory(entry.target.parent().unwrap())?;
                sync_directory(previous.parent().unwrap())?;
                if !files_equal(previous, backup)? {
                    return Err(io::Error::other(
                        "destinazione cambiata durante la pubblicazione",
                    ));
                }
            }
            StagedFile::copy(&snapshot, &entry.target)?.publish(&entry.target)?;
            checkpoint("published", index)?;
        }
        for (index, entry) in manifest.entries.iter().enumerate() {
            let snapshot = directory.join(format!("source-{index}"));
            if !files_equal(&entry.source, &snapshot)? || !files_equal(&entry.target, &snapshot)? {
                return Err(io::Error::other(
                    "file cambiato prima della conferma del lotto",
                ));
            }
        }
        if !copy {
            for (index, entry) in manifest.entries.iter().enumerate() {
                marker(&directory, &format!("retire-{index}"))?;
                writable_regular_file(&entry.source)?;
                let backup = entry.source_backup.as_ref().unwrap();
                fs::rename(&entry.source, backup)?;
                sync_directory(entry.source.parent().unwrap())?;
                sync_directory(backup.parent().unwrap())?;
                if !files_equal(backup, &directory.join(format!("source-{index}")))? {
                    return Err(io::Error::other("sorgente cambiata durante lo spostamento"));
                }
                checkpoint("retired", index)?;
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        rollback(&directory, &manifest).map_err(|recovery| {
            io::Error::other(format!(
                "{error}; {recovery}; registro: {}",
                directory.display()
            ))
        })?;
        return Err(io::Error::other(format!("lotto annullato: {error}")));
    }
    // Once a commit decision has been attempted, never begin rollback: a
    // crash during that rollback could leave a durable commit marker next to
    // partially restored files. Recovery resolves an absent decision instead.
    marker(&directory, "committed").map_err(|error| {
        io::Error::other(format!(
            "conferma del lotto non sincronizzata: {error}; registro: {}",
            directory.display()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{catch_unwind, AssertUnwindSafe};

    fn fixture() -> (tempfile::TempDir, Vec<(PathBuf, PathBuf)>) {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let target = root.path().join("target");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&target).unwrap();
        let pairs = ["a.jpg", "b.jpg"]
            .into_iter()
            .enumerate()
            .map(|(index, name)| {
                let from = source.join(name);
                let to = target.join(name);
                fs::write(&from, format!("precious source {index}")).unwrap();
                fs::write(&to, format!("precious destination {index}")).unwrap();
                (from, to)
            })
            .collect();
        (root, pairs)
    }

    fn assert_originals(pairs: &[(PathBuf, PathBuf)]) {
        for (index, (source, target)) in pairs.iter().enumerate() {
            assert_eq!(
                fs::read(source).unwrap(),
                format!("precious source {index}").as_bytes()
            );
            assert_eq!(
                fs::read(target).unwrap(),
                format!("precious destination {index}").as_bytes()
            );
        }
    }

    #[test]
    fn every_interrupted_publication_and_retirement_recovers_the_entire_batch() {
        for (point, index) in [
            ("before-publish", 0),
            ("before-publish", 1),
            ("published", 0),
            ("published", 1),
            ("retired", 0),
            ("retired", 1),
        ] {
            let (_root, pairs) = fixture();
            let interrupted = catch_unwind(AssertUnwindSafe(|| {
                let _ = execute_with(&pairs, true, false, |phase, item| {
                    if phase == point && item == index {
                        panic!("simulated process stop");
                    }
                    Ok(())
                });
            }));
            assert!(interrupted.is_err());
            assert_eq!(recover_directory(pairs[0].1.parent().unwrap()).unwrap(), 1);
            assert_originals(&pairs);
            assert_eq!(recover_directory(pairs[0].1.parent().unwrap()).unwrap(), 0);
        }
    }

    #[test]
    fn a_failure_on_the_second_photo_rolls_back_the_first_photo() {
        let (_root, pairs) = fixture();
        assert!(execute_with(&pairs, true, false, |phase, item| {
            if phase == "published" && item == 1 {
                return Err(io::Error::other("disk full"));
            }
            Ok(())
        })
        .is_err());
        assert_originals(&pairs);
    }

    #[test]
    fn interrupted_new_copies_are_retained_outside_the_visible_destination() {
        let (_root, pairs) = fixture();
        // Remove only generated test fixtures before exercising new-copy mode.
        for (_, to) in &pairs {
            fs::remove_file(to).unwrap();
        }
        assert!(execute_with(&pairs, false, true, |phase, item| {
            if phase == "published" && item == 0 {
                return Err(io::Error::other("injected failure"));
            }
            Ok(())
        })
        .is_err());
        assert!(pairs
            .iter()
            .all(|(source, target)| source.exists() && !target.exists()));
        let copies = crate::list_backups(&pairs[0].1).unwrap();
        assert_eq!(copies.len(), 1);
        assert_eq!(fs::read(&copies[0].path).unwrap(), b"precious source 0");
    }

    #[test]
    fn external_changes_are_preserved_and_the_journal_remains_recoverable() {
        let (_root, pairs) = fixture();
        assert!(execute_with(&pairs, true, false, |phase, item| {
            if phase == "published" && item == 0 {
                fs::write(&pairs[0].1, b"external edit")?;
                return Err(io::Error::other("injected failure"));
            }
            Ok(())
        })
        .is_err());
        assert_eq!(fs::read(&pairs[0].1).unwrap(), b"external edit");
        assert!(pairs.iter().all(|(source, _)| source.exists()));
        assert!(recover_directory(pairs[0].1.parent().unwrap()).is_err());
        assert!(crate::list_backups(&pairs[0].1)
            .unwrap()
            .iter()
            .any(|copy| fs::read(&copy.path).unwrap() == b"precious destination 0"));
    }

    #[test]
    fn active_batches_cannot_be_recovered_by_another_handle() {
        let (_root, pairs) = fixture();
        execute_with(&pairs, true, true, |phase, _| {
            if phase == "published" {
                assert_eq!(recover_directory(pairs[0].1.parent().unwrap())?, 0);
            }
            Ok(())
        })
        .unwrap();
    }
}
