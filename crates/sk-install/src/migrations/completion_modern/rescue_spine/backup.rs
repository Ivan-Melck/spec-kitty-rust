//! The `3.0.0_canonical_context` backup, restore and copy discipline.
//!
//! Source `migration/runner.py` has four properties a naive `fs::copy` loop gets
//! wrong, and each one is observable:
//!
//! * **A failed backup is non-fatal** (`_create_backup` returns `None` and the
//!   caller appends `Could not create backup — proceeding without safety net`).
//!   It is not a refusal.
//! * **A stale tool-owned backup is removed with a chmod retry**
//!   (`remove_tool_owned_tree`): a backup left read-only by an earlier run must
//!   not turn this run into a refusal. A failure there is only the same
//!   non-fatal warning.
//! * **Copies are `shutil.copy2`** — bytes **and** the observed file mode and
//!   mtime travel, in both directions. The times are the ones actually read from
//!   disk; none is invented.
//! * **A file that cannot be read makes the backup fail** (Source's `copytree`
//!   raises, `_create_backup` catches `OSError`), so the run continues with the
//!   warning instead of pretending a partial safety net exists.
//!
//! Removal of a tool-owned tree never follows a symlink and never deletes a
//! foreign directory: the only tree this module removes is the migration backup
//! directory itself (and, during a rollback, the `.kittify` children this run's
//! own backup covers).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The migration backup root, a sibling of everything the runner rewrites.
pub const BACKUP_DIR_NAME: &str = ".migration-backup";

/// Backup child holding the pre-run `kitty-specs/` snapshot.
pub const KITTY_SPECS_BACKUP: &str = "kitty-specs-backup";

/// Backup child holding the pre-run `.gitignore`.
pub const GITIGNORE_BACKUP: &str = "gitignore-backup";

/// Backup sentinel recording that there was no `.gitignore` before the run.
pub const GITIGNORE_ABSENT: &str = "gitignore-absent";

/// Source's exact non-fatal backup warning (`runner.py`).
pub const BACKUP_WARNING: &str = "Could not create backup — proceeding without safety net";

/// The times and mode observed on a file, in both directions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObservedStamp {
    /// The observed permission bits.
    pub mode: u32,
    /// The observed modification time.
    pub modified: Option<SystemTime>,
}

/// The observed stamp of one path, if it can be read.
pub fn observe_stamp(path: &Path) -> io::Result<ObservedStamp> {
    let metadata = fs::metadata(path)?;
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode()
    };
    #[cfg(not(unix))]
    let mode = if metadata.permissions().readonly() {
        0o444
    } else {
        0o644
    };
    Ok(ObservedStamp {
        mode,
        modified: metadata.modified().ok(),
    })
}

/// Apply a stamp to `path` (best effort: metadata is not the migration's data).
fn apply_stamp(path: &Path, stamp: ObservedStamp) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(stamp.mode));
    }
    if let Some(modified) = stamp.modified
        && let Ok(handle) = fs::File::options().write(true).open(path)
    {
        let _ = handle.set_modified(modified);
    }
}

/// `shutil.copy2`: copy the bytes and then the observed mode and mtime.
pub fn copy2(from: &Path, to: &Path) -> io::Result<()> {
    let stamp = observe_stamp(from)?;
    fs::copy(from, to)?;
    apply_stamp(to, stamp);
    Ok(())
}

/// Recursively copy `source` into `destination`, `copy2`-style, skipping one
/// child name when asked.
///
/// A symlink is reproduced as a symlink (never followed), so a link back into
/// the tree cannot make the backup read outside `.kittify/` or loop.
pub fn copy_tree(source: &Path, destination: &Path, skip: Option<&str>) -> io::Result<()> {
    let stamp = observe_stamp(source)?;
    fs::create_dir_all(destination)?;
    let entries = fs::read_dir(source)?;
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        if let Some(skip) = skip
            && skip == name.to_string_lossy().as_ref()
        {
            continue;
        }
        let from = entry.path();
        let to = destination.join(&name);
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            copy_tree(&from, &to, None)?;
        } else if file_type.is_symlink() {
            copy_symlink(&from, &to)?;
        } else {
            copy2(&from, &to)?;
        }
    }
    // Directory modes last, so a read-only source directory still receives its
    // children before the write bit is taken away.
    apply_stamp(destination, stamp);
    Ok(())
}

#[cfg(unix)]
fn copy_symlink(from: &Path, to: &Path) -> io::Result<()> {
    let target = fs::read_link(from)?;
    std::os::unix::fs::symlink(target, to)
}

#[cfg(not(unix))]
fn copy_symlink(from: &Path, to: &Path) -> io::Result<()> {
    copy2(from, to)
}

/// Remove a directory tree this tool owns, retrying once after making the tree
/// writable (Source `remove_tool_owned_tree`).
///
/// Refuses a symlinked root: a backup path that has become an alias must never
/// be followed. Every other failure is returned to the caller, which decides
/// whether it is fatal (Source treats the backup paths as non-fatal).
pub fn remove_tool_owned_tree(path: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink() {
        return Err(io::Error::other(format!(
            "refusing to remove {}: it is a symlink, not a tool-owned directory",
            path.display()
        )));
    }
    if !metadata.is_dir() {
        return fs::remove_file(path);
    }
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(first) if first.kind() == io::ErrorKind::PermissionDenied => {
            make_tree_writable(path)?;
            fs::remove_dir_all(path).map_err(|second| {
                io::Error::other(format!(
                    "cannot remove {}: {second} (after a chmod retry: {first})",
                    path.display()
                ))
            })
        }
        Err(error) => Err(error),
    }
}

/// Give the owner write/traverse permission on `path` and its children.
fn make_tree_writable(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() {
            return Ok(());
        }
        let mode = metadata.permissions().mode();
        let writable = if metadata.is_dir() {
            mode | 0o700
        } else {
            mode | 0o600
        };
        fs::set_permissions(path, fs::Permissions::from_mode(writable))?;
        if metadata.is_dir() {
            for entry in fs::read_dir(path)? {
                make_tree_writable(&entry?.path())?;
            }
        }
    }
    Ok(())
}

/// One created backup, with the paths the rollback needs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Backup {
    /// The `.kittify/.migration-backup` directory this run created.
    pub dir: PathBuf,
    /// True when the pre-run tree had no `.gitignore` (the sentinel is present).
    pub gitignore_absent: bool,
    /// True when a `kitty-specs/` snapshot was taken.
    pub kitty_specs_saved: bool,
    /// True when the pre-run `.gitignore` bytes were saved.
    pub gitignore_saved: bool,
}

/// The outcome of the backup step: Source's `Path | None` plus its warning.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackupStep {
    /// The backup, when the step succeeded.
    pub backup: Option<Backup>,
    /// The non-fatal warning Source appends when the backup failed.
    pub warning: Option<String>,
}

/// Source `_create_backup`: snapshot `.kittify/`, `kitty-specs/` and
/// `.gitignore`, returning `None` instead of raising.
///
/// Nothing here is fatal: a stale backup that cannot be removed and a file that
/// cannot be read both degrade to Source's `Could not create backup` warning and
/// the nine steps still run.
pub fn create_backup(project_root: &Path) -> BackupStep {
    let kittify = project_root.join(".kittify");
    if !kittify.exists() {
        return BackupStep {
            backup: None,
            warning: None,
        };
    }
    let dir = kittify.join(BACKUP_DIR_NAME);
    if dir.exists()
        && let Err(error) = remove_tool_owned_tree(&dir)
    {
        // Source logs `Could not remove stale backup: …` and proceeds; the copy
        // below then fails (the directory is still there) and becomes the
        // non-fatal warning, never a refusal.
        eprintln!("Could not remove stale backup: {error}");
    }
    match snapshot(project_root, &kittify, &dir) {
        Ok(backup) => BackupStep {
            backup: Some(backup),
            warning: None,
        },
        Err(error) => {
            // Source logs the cause and reports ONLY this fixed warning.
            eprintln!("Failed to create backup: {error}");
            BackupStep {
                backup: None,
                warning: Some(BACKUP_WARNING.to_string()),
            }
        }
    }
}

fn snapshot(project_root: &Path, kittify: &Path, dir: &Path) -> io::Result<Backup> {
    copy_tree(kittify, dir, Some(BACKUP_DIR_NAME))?;
    let kitty_specs =
        project_root.join(crate::migrations::completion_modern::support::KITTY_SPECS_DIR);
    let kitty_specs_saved = kitty_specs.is_dir();
    if kitty_specs_saved {
        copy_tree(&kitty_specs, &dir.join(KITTY_SPECS_BACKUP), None)?;
    }
    let gitignore = project_root.join(".gitignore");
    let gitignore_saved = gitignore.is_file();
    if gitignore_saved {
        copy2(&gitignore, &dir.join(GITIGNORE_BACKUP))?;
    } else {
        fs::write(dir.join(GITIGNORE_ABSENT), b"")?;
    }
    let gitignore_absent = !gitignore_saved;
    Ok(Backup {
        dir: dir.to_path_buf(),
        gitignore_absent,
        kitty_specs_saved,
        gitignore_saved,
    })
}

/// Restore everything this run's backup covers.
///
/// Returns `true` only when every restore operation succeeded. After a partial
/// restore the backup is the only copy of what failed to come back, so the
/// caller must keep it and say where it is.
pub fn restore_backup(project_root: &Path, backup: &Backup) -> bool {
    if !backup.dir.exists() {
        return false;
    }
    let mut restored = true;
    if backup.kitty_specs_saved {
        let kitty_specs =
            project_root.join(crate::migrations::completion_modern::support::KITTY_SPECS_DIR);
        if kitty_specs.is_dir() && remove_tool_owned_tree(&kitty_specs).is_err() {
            restored = false;
        }
        if copy_tree(&backup.dir.join(KITTY_SPECS_BACKUP), &kitty_specs, None).is_err() {
            restored = false;
        }
    }
    let gitignore = project_root.join(".gitignore");
    if backup.gitignore_saved {
        if copy2(&backup.dir.join(GITIGNORE_BACKUP), &gitignore).is_err() {
            restored = false;
        }
    } else if backup.gitignore_absent {
        let _ = fs::remove_file(&gitignore);
    }
    let kittify = project_root.join(".kittify");
    if let Ok(entries) = fs::read_dir(&kittify) {
        for entry in entries.flatten() {
            if entry.file_name() == BACKUP_DIR_NAME {
                continue;
            }
            let path = entry.path();
            let removed = if path.is_dir() {
                remove_tool_owned_tree(&path).is_ok()
            } else {
                fs::remove_file(&path).is_ok()
            };
            if !removed {
                restored = false;
            }
        }
    }
    if let Ok(entries) = fs::read_dir(&backup.dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name == KITTY_SPECS_BACKUP || name == GITIGNORE_BACKUP || name == GITIGNORE_ABSENT {
                continue;
            }
            let from = entry.path();
            let to = kittify.join(&name);
            if from.is_dir() {
                if copy_tree(&from, &to, None).is_err() {
                    restored = false;
                }
            } else if copy2(&from, &to).is_err() {
                restored = false;
            }
        }
    }
    restored
}

/// Remove the backup directory after a successful run (or a full rollback).
pub fn cleanup_backup(project_root: &Path) -> bool {
    let dir = project_root.join(".kittify").join(BACKUP_DIR_NAME);
    if !dir.exists() {
        return true;
    }
    remove_tool_owned_tree(&dir).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy2_carries_the_observed_mode_and_mtime() {
        let temp = tempfile::tempdir().expect("temp");
        let from = temp.path().join("from.txt");
        let to = temp.path().join("to.txt");
        fs::write(&from, b"payload").expect("write");
        let observed = observe_stamp(&from).expect("stamp");
        copy2(&from, &to).expect("copy");
        let copied = observe_stamp(&to).expect("stamp");
        assert_eq!(copied.mode, observed.mode);
        assert_eq!(copied.modified, observed.modified);
        assert_eq!(fs::read(&to).expect("read"), b"payload");
    }

    #[test]
    fn a_read_only_tree_is_removed_after_a_chmod_retry() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().join("backup");
        fs::create_dir_all(root.join("nested")).expect("dirs");
        fs::write(root.join("nested/file.txt"), b"x").expect("write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                root.join("nested/file.txt"),
                fs::Permissions::from_mode(0o400),
            )
            .expect("chmod file");
            fs::set_permissions(root.join("nested"), fs::Permissions::from_mode(0o500))
                .expect("chmod dir");
        }
        remove_tool_owned_tree(&root).expect("removed");
        assert!(!root.exists());
    }

    #[test]
    fn a_symlinked_root_is_never_removed() {
        let temp = tempfile::tempdir().expect("temp");
        let real = temp.path().join("real");
        fs::create_dir_all(&real).expect("dir");
        let alias = temp.path().join("alias");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &alias).expect("symlink");
        #[cfg(not(unix))]
        return;
        #[cfg(unix)]
        {
            let error = remove_tool_owned_tree(&alias).expect_err("refusal");
            assert!(error.to_string().contains("symlink"), "{error}");
            assert!(real.is_dir());
        }
    }

    #[test]
    fn an_unreadable_file_makes_the_backup_non_fatal_not_a_refusal() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();
        fs::create_dir_all(root.join(".kittify")).expect("kittify");
        fs::write(root.join(".kittify/metadata.yaml"), b"spec_kitty: {}\n").expect("metadata");
        let locked = root.join(".kittify/locked.bin");
        fs::write(&locked, b"secret").expect("locked");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("chmod");
        }
        if fs::read(&locked).is_ok() {
            // A privileged test runner can read through 0o000; the assertion
            // below only binds the unprivileged case and is not weakened for it.
            return;
        }
        let step = create_backup(root);
        assert!(step.backup.is_none(), "{:?}", step.backup);
        assert_eq!(
            step.warning.as_deref(),
            Some(BACKUP_WARNING),
            "{:?}",
            step.warning
        );
        assert!(locked.exists());
    }
}
