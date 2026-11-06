//! The centralised-layout migration: classify a project's `.kittify/` files
//! against the shipped package defaults (`runtime/migrate.py`).
//!
//! | disposition | meaning | action |
//! | --- | --- | --- |
//! | `identical` | byte-identical to the package counterpart | remove through the ownership guard |
//! | `superseded` | differs from the counterpart (customised *or* an outdated default) | preserve in place |
//! | `customized` | no package counterpart at all (user-created) | move to `.kittify/overrides/` unless that would overwrite or break a link |
//! | `project_specific` | a declared project-owned path | keep |
//! | `unknown` | anything else | keep, reported |
//!
//! Ownership is decided only by the byte-match: a differing file that merely
//! embeds the command version marker is *not* proven (the prover is
//! `check_marker=False` semantics, i.e. [`Prover::CanonicalBytes`]), and a
//! symbolic link is never followed to move or delete it.

use std::path::{Path, PathBuf};

use crate::error::InstallError;
use crate::migrations::completion_legacy::assets;
use crate::migrations::completion_legacy::paths::{PROJECT_SPECIFIC_PATHS, is_shared_asset};
use crate::preserve::{Prover, RemovalVerdict, guard_destructive_removal};

/// How one per-project `.kittify/` file is classified.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssetDisposition {
    /// Byte-identical to the shipped counterpart.
    Identical,
    /// Differs from the shipped counterpart.
    Superseded,
    /// No shipped counterpart: a genuinely user-created file.
    Customized,
    /// A declared project-specific path.
    ProjectSpecific,
    /// Not classifiable.
    Unknown,
}

/// Why a customized file was *not* moved into `overrides/`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConflictReason {
    /// Something already occupies the destination.
    DestinationExists,
    /// A non-folder blocks the overrides path.
    DestinationParentBlocked,
    /// The source is a symbolic link.
    SourceIsSymlink,
}

impl ConflictReason {
    /// The operator wording the pinned command prints.
    pub fn text(self) -> &'static str {
        match self {
            Self::DestinationExists => "override already exists",
            Self::DestinationParentBlocked => "a non-folder blocks the overrides path",
            Self::SourceIsSymlink => "legacy file is a symbolic link",
        }
    }
}

/// One refused customized move.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationConflict {
    /// The file left in place.
    pub source: PathBuf,
    /// The destination that was not written.
    pub destination: PathBuf,
    /// Why it was refused.
    pub reason: ConflictReason,
}

/// What a layout migration did (or would do under `dry_run`).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MigrationReport {
    /// Files byte-identical to their counterpart: removed.
    pub removed: Vec<PathBuf>,
    /// Files differing from their counterpart: preserved in place.
    pub superseded: Vec<PathBuf>,
    /// `(from, to)` pairs moved into `overrides/`.
    pub moved: Vec<(PathBuf, PathBuf)>,
    /// Project-specific files kept.
    pub kept: Vec<PathBuf>,
    /// Files of unknown disposition kept with a warning.
    pub unknown: Vec<PathBuf>,
    /// Customized files that could not be moved.
    pub conflicts: Vec<MigrationConflict>,
    /// Whether this was a preview.
    pub dry_run: bool,
}

/// The package counterpart of a `.kittify/`-relative shared asset.
fn package_counterpart(
    relative: &Path,
    package_root: Option<&Path>,
    global_home: &Path,
    mission: &str,
) -> Option<PathBuf> {
    match package_root {
        Some(package_root) => {
            // A project mission copy: `missions/<type>/<rest>` maps to the
            // package's own `<type>/<rest>` (the package root *is* a missions
            // tree), so a pack-identical copy is not read as customized.
            let parts: Vec<String> = relative
                .components()
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .collect();
            if parts.len() > 2 && parts[0] == "missions" {
                let candidate = package_root.join(parts[1..].join("/"));
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
            let mission_specific = package_root.join(mission).join(relative);
            if mission_specific.is_file() {
                return Some(mission_specific);
            }
            let direct = package_root.join(relative);
            direct.is_file().then_some(direct)
        }
        None => {
            let mission_specific = global_home.join("missions").join(mission).join(relative);
            if mission_specific.is_file() {
                return Some(mission_specific);
            }
            let direct = global_home.join(relative);
            direct.is_file().then_some(direct)
        }
    }
}

/// Classify one per-project `.kittify/` file.
pub fn classify_asset(
    local_path: &Path,
    global_home: &Path,
    project_kittify: &Path,
    mission: &str,
    package_root: Option<&Path>,
) -> AssetDisposition {
    let Ok(relative) = local_path.strip_prefix(project_kittify) else {
        return AssetDisposition::Unknown;
    };
    let top_level = relative
        .components()
        .next()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .unwrap_or_default();
    if PROJECT_SPECIFIC_PATHS.contains(&top_level.as_str()) {
        return AssetDisposition::ProjectSpecific;
    }
    if !is_shared_asset(relative) {
        return AssetDisposition::Unknown;
    }
    if !local_path.is_file() {
        return AssetDisposition::Unknown;
    }
    let Some(counterpart) = package_counterpart(relative, package_root, global_home, mission)
    else {
        // No shipped counterpart: the file is the user's own.
        return AssetDisposition::Customized;
    };
    let (Ok(local), Ok(shipped)) = (std::fs::read(local_path), std::fs::read(&counterpart)) else {
        return AssetDisposition::Customized;
    };
    if local == shipped {
        AssetDisposition::Identical
    } else {
        AssetDisposition::Superseded
    }
}

fn move_blocker(
    source: &Path,
    destination: &Path,
    overrides_root: &Path,
) -> Option<ConflictReason> {
    if source.is_symlink() {
        return Some(ConflictReason::SourceIsSymlink);
    }
    if destination.is_symlink() || destination.exists() {
        return Some(ConflictReason::DestinationExists);
    }
    let mut chain = vec![overrides_root.to_path_buf()];
    if let Ok(rest) = destination.strip_prefix(overrides_root)
        && let Some(parent) = rest.parent()
    {
        let mut current = overrides_root.to_path_buf();
        for part in parent.components() {
            current.push(part);
            chain.push(current.clone());
        }
    }
    for candidate in chain {
        if (candidate.is_symlink() || candidate.exists()) && !candidate.is_dir() {
            return Some(ConflictReason::DestinationParentBlocked);
        }
    }
    None
}

fn move_customized(
    path: &Path,
    kittify_dir: &Path,
    report: &mut MigrationReport,
    dry_run: bool,
) -> Result<(), InstallError> {
    let overrides_root = kittify_dir.join("overrides");
    let Ok(relative) = path.strip_prefix(kittify_dir) else {
        return Ok(());
    };
    let destination = overrides_root.join(relative);
    if let Some(reason) = move_blocker(path, &destination, &overrides_root) {
        report.conflicts.push(MigrationConflict {
            source: path.to_path_buf(),
            destination,
            reason,
        });
        return Ok(());
    }
    report.moved.push((path.to_path_buf(), destination.clone()));
    if !dry_run {
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| InstallError::io("create directory", parent, error))?;
        }
        std::fs::rename(path, &destination)
            .map_err(|error| InstallError::io("move into overrides", path, error))?;
    }
    Ok(())
}

fn cleanup_empty_dirs(kittify_dir: &Path) {
    let mut depth_first: Vec<PathBuf> = Vec::new();
    let mut stack = vec![kittify_dir.to_path_buf()];
    while let Some(directory) = stack.pop() {
        depth_first.push(directory.clone());
        for child in crate::migrations::completion_legacy::paths::directories(&directory) {
            stack.push(child);
        }
    }
    depth_first.sort();
    for directory in depth_first.into_iter().rev() {
        let Ok(relative) = directory.strip_prefix(kittify_dir) else {
            continue;
        };
        let top_level = relative
            .components()
            .next()
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .unwrap_or_default();
        if top_level.is_empty() {
            continue;
        }
        if !crate::migrations::completion_legacy::paths::SHARED_ASSET_DIRS
            .contains(&top_level.as_str())
        {
            continue;
        }
        crate::migrations::completion_legacy::paths::remove_if_empty(&directory);
    }
}

/// Scan and migrate the project's `.kittify/` shared assets.
pub fn execute_migration(
    project_dir: &Path,
    dry_run: bool,
    mission: &str,
) -> Result<MigrationReport, InstallError> {
    let kittify_dir = project_dir.join(".kittify");
    let global_home = assets::kittify_home().unwrap_or_else(|| project_dir.join(".kittify-home"));
    let package_root = assets::package_asset_root();
    let mut report = MigrationReport {
        dry_run,
        ..MigrationReport::default()
    };
    if !kittify_dir.is_dir() {
        return Ok(report);
    }

    let mut files: Vec<PathBuf> = Vec::new();
    let mut stack = vec![kittify_dir.clone()];
    while let Some(directory) = stack.pop() {
        for entry in crate::migrations::completion_legacy::paths::entries(&directory) {
            let Ok(metadata) = std::fs::symlink_metadata(&entry) else {
                continue;
            };
            if metadata.is_dir() {
                stack.push(entry);
                continue;
            }
            files.push(entry);
        }
    }
    files.sort();

    for path in files {
        let disposition = classify_asset(
            &path,
            &global_home,
            &kittify_dir,
            mission,
            package_root.as_deref(),
        );
        match disposition {
            AssetDisposition::Identical | AssetDisposition::Superseded => {
                let relative = path
                    .strip_prefix(&kittify_dir)
                    .unwrap_or(&path)
                    .to_path_buf();
                let canonical =
                    package_counterpart(&relative, package_root.as_deref(), &global_home, mission)
                        .and_then(|counterpart| std::fs::read(counterpart).ok());
                let prover = match canonical {
                    Some(bytes) => Prover::CanonicalBytes(bytes),
                    // Unreadable counterpart: fail closed toward preservation.
                    None => Prover::ManagedPath(Vec::new()),
                };
                let verdict =
                    guard_destructive_removal(&path, project_dir, &prover, false, None, dry_run)?;
                match verdict {
                    RemovalVerdict::Owned { .. } => report.removed.push(path),
                    RemovalVerdict::Absent => {}
                    RemovalVerdict::Preserved { .. } | RemovalVerdict::Archived { .. } => {
                        report.superseded.push(path)
                    }
                }
            }
            AssetDisposition::Customized => {
                move_customized(&path, &kittify_dir, &mut report, dry_run)?;
            }
            AssetDisposition::ProjectSpecific => report.kept.push(path),
            AssetDisposition::Unknown => report.unknown.push(path),
        }
    }

    if !dry_run {
        cleanup_empty_dirs(&kittify_dir);
    }
    Ok(report)
}

/// Whether a project has a `.kittify/` directory at all.
pub fn has_kittify(project_dir: &Path) -> bool {
    project_dir.join(".kittify").is_dir()
}
