//! Pre-1.0 layout migrations: the `.specify/` rebrand and two retirements.
//!
//! These are the oldest declared migrations that still have real work. They are
//! filesystem-shape changes on the project root, not content rewrites, so each
//! one owns an explicit precondition instead of guessing:
//!
//! | id | precondition | effect |
//! | --- | --- | --- |
//! | `0.2.0_specify_to_kittify` | `.specify/` exists and `.kittify/` does not | rename `.specify/`→`.kittify/` and `specs/`→`kitty-specs/` |
//! | `0.8.0_remove_active_mission` | `.kittify/active-mission` exists | unlink the retired file or symlink |
//!
//! `0.2.0`'s rename is the source's `shutil.move` on the *same* project root, so
//! a plain rename is the same effect. A cross-device move (which `shutil.move`
//! would turn into copy-then-delete) is refused with a typed error rather than
//! silently copying a directory tree: a partially copied `.kittify/` is worse
//! than a retry, and no project layout puts these two paths on different
//! filesystems.
//!
//! Both migrations only ever touch the documented paths. A project that already
//! has both layouts is a `can_apply` refusal, never a merge: the source says
//! "manual merge required", and guessing which side wins would destroy work.

use std::path::Path;

use crate::error::InstallError;
use crate::upgrade::MigrationOutcome;

const SPECIFY_DIR: &str = ".specify";
const KITTIFY_DIR: &str = ".kittify";
const SPECS_DIR: &str = "specs";
const KITTY_SPECS_DIR: &str = "kitty-specs";

/// `0.2.0_specify_to_kittify.detect`.
pub fn detect_specify_layout(project_root: &Path) -> Result<bool, InstallError> {
    Ok(project_root.join(SPECIFY_DIR).exists())
}

/// `0.2.0_specify_to_kittify.can_apply`.
pub fn can_apply_specify_layout(project_root: &Path) -> Result<Option<String>, InstallError> {
    let specify_dir = project_root.join(SPECIFY_DIR);
    let kittify_dir = project_root.join(KITTIFY_DIR);
    let specs_dir = project_root.join(SPECS_DIR);
    let kitty_specs_dir = project_root.join(KITTY_SPECS_DIR);
    if !specify_dir.exists() {
        return Ok(Some(format!("{SPECIFY_DIR}/ directory does not exist")));
    }
    if kittify_dir.exists() {
        return Ok(Some(format!(
            "{KITTIFY_DIR}/ already exists - manual merge required"
        )));
    }
    if specs_dir.exists() && kitty_specs_dir.exists() {
        return Ok(Some(format!(
            "Both /{SPECS_DIR}/ and /{KITTY_SPECS_DIR}/ exist - manual merge required"
        )));
    }
    Ok(None)
}

/// `0.2.0_specify_to_kittify.apply`.
pub fn apply_specify_layout(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let specify_dir = project_root.join(SPECIFY_DIR);
    let kittify_dir = project_root.join(KITTIFY_DIR);
    let specs_dir = project_root.join(SPECS_DIR);
    let kitty_specs_dir = project_root.join(KITTY_SPECS_DIR);
    let mut changes = Vec::new();
    if specify_dir.exists() {
        rename(&specify_dir, &kittify_dir)?;
        changes.push(format!(
            "Renamed {} to {}",
            specify_dir.display(),
            kittify_dir.display()
        ));
    }
    if specs_dir.exists() && !kitty_specs_dir.exists() {
        rename(&specs_dir, &kitty_specs_dir)?;
        changes.push(format!(
            "Renamed {} to {}",
            specs_dir.display(),
            kitty_specs_dir.display()
        ));
    }
    Ok(if changes.is_empty() {
        MigrationOutcome::unchanged(changes)
    } else {
        MigrationOutcome::changed(changes)
    })
}

/// `0.8.0_remove_active_mission.detect`.
///
/// A broken symlink counts as present: `Path.exists()` follows the link, so the
/// source adds `is_symlink()` specifically to catch it, and a dangling
/// `active-mission` link is exactly the residue this migration retires.
pub fn detect_active_mission(project_root: &Path) -> Result<bool, InstallError> {
    let path = project_root.join(KITTIFY_DIR).join("active-mission");
    Ok(std::fs::symlink_metadata(&path).is_ok())
}

/// `0.8.0_remove_active_mission.apply`.
pub fn apply_active_mission(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let path = project_root.join(KITTIFY_DIR).join("active-mission");
    if std::fs::symlink_metadata(&path).is_err() {
        // The source reports no change here, only the warning.
        let mut outcome = MigrationOutcome::unchanged(Vec::new());
        outcome
            .warnings
            .push("No .kittify/active-mission found (already migrated or new project)".to_string());
        return Ok(outcome);
    }
    // `unlink` removes a file or a symlink; a directory here is not the shape
    // this migration owns, so it refuses rather than recursing into it.
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|source| InstallError::io("inspect", &path, source))?;
    if metadata.is_dir() {
        return Err(InstallError::UnsupportedNode {
            path: path.clone(),
            detail: ".kittify/active-mission is a directory, not the retired file/symlink"
                .to_string(),
        });
    }
    std::fs::remove_file(&path).map_err(|source| InstallError::io("remove", &path, source))?;
    Ok(MigrationOutcome::changed(vec![
        "Removed deprecated .kittify/active-mission".to_string(),
        "  -> Missions are now selected per-feature during /spec-kitty.specify".to_string(),
        "  -> Existing features will use 'software-dev' mission by default".to_string(),
    ]))
}

fn rename(from: &Path, to: &Path) -> Result<(), InstallError> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
            Err(InstallError::UpgradeRefused {
                detail: format!(
                    "cannot rename {} to {} across filesystems; move it yourself and re-run upgrade",
                    from.display(),
                    to.display()
                ),
            })
        }
        Err(source) => Err(InstallError::io("rename directory", from, source)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> tempfile::TempDir {
        tempfile::tempdir().expect("temp")
    }

    #[test]
    fn the_specify_layout_is_renamed_and_reported_once() {
        let directory = project();
        let root = directory.path();
        std::fs::create_dir_all(root.join(".specify/memory")).unwrap();
        std::fs::write(root.join(".specify/memory/MEMORY.md"), "m\n").unwrap();
        std::fs::create_dir_all(root.join("specs/one")).unwrap();
        std::fs::write(root.join("specs/one/spec.md"), "# spec\n").unwrap();
        assert!(detect_specify_layout(root).unwrap());
        assert_eq!(can_apply_specify_layout(root).unwrap(), None);
        let outcome = apply_specify_layout(root).unwrap();
        assert!(outcome.changed);
        assert_eq!(outcome.changes.len(), 2);
        assert!(!root.join(".specify").exists());
        assert!(!root.join("specs").exists());
        assert!(root.join(".kittify/memory/MEMORY.md").is_file());
        assert!(root.join("kitty-specs/one/spec.md").is_file());
        assert!(!detect_specify_layout(root).unwrap());
    }

    #[test]
    fn an_existing_kittify_layout_is_a_refusal_not_a_merge() {
        let directory = project();
        let root = directory.path();
        std::fs::create_dir_all(root.join(".specify")).unwrap();
        std::fs::create_dir_all(root.join(".kittify")).unwrap();
        assert_eq!(
            can_apply_specify_layout(root).unwrap(),
            Some(".kittify/ already exists - manual merge required".to_string())
        );
        assert!(root.join(".specify").exists());
    }

    #[test]
    fn a_dangling_active_mission_symlink_is_removed() {
        let directory = project();
        let root = directory.path();
        std::fs::create_dir_all(root.join(".kittify")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("gone", root.join(".kittify/active-mission")).unwrap();
        assert!(detect_active_mission(root).unwrap());
        let outcome = apply_active_mission(root).unwrap();
        assert!(outcome.changed);
        assert!(!root.join(".kittify/active-mission").exists());
        assert!(!detect_active_mission(root).unwrap());
    }

    #[test]
    fn a_missing_active_mission_is_a_warning_not_a_change() {
        let directory = project();
        let root = directory.path();
        std::fs::create_dir_all(root.join(".kittify")).unwrap();
        let outcome = apply_active_mission(root).unwrap();
        assert!(!outcome.changed);
        assert!(outcome.changes.is_empty());
        assert_eq!(outcome.warnings.len(), 1);
    }
}
