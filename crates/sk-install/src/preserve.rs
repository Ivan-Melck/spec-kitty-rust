//! Ownership proof and guarded destructive removal for migration effects.
//!
//! Upstream routes every destructive cleanup a migration performs through
//! `asset_preservation.guard_destructive_removal` with one of three provers
//! (`CanonicalContentProver`, `ManifestProver`, `ManagedPathProver`, composed by
//! `AnyProver`). The charter's user-customisation clause binds that shape: a
//! retired *name* is never proof of package ownership, so an unprovable path is
//! preserved in place or archived verbatim, never deleted.
//!
//! This module is the native equivalent of that seam, so each retirement
//! migration owns only its *candidates* and its *prover*, not a private copy of
//! the ownership decision:
//!
//! | Source prover | Native [`Prover`] | Proof |
//! | --- | --- | --- |
//! | `ManifestProver` | [`Prover::Manifest`] | a managed-skills entry (`copy` delivery) or a command-skills entry whose recorded hash matches the bytes on disk |
//! | `CanonicalContentProver` | [`Prover::CanonicalContent`] | the file carries the package command version marker (or byte-matches a supplied canonical) |
//! | `ManagedPathProver` | [`Prover::ManagedPath`] | the path is a declared package-managed runtime path |
//! | `AnyProver` | [`Prover::Any`] | the first component that proves ownership |
//!
//! Contracts preserved from the pinned source:
//!
//! - a directory is owned only when **every** regular-file member is
//!   manifest-owned and no member is a symlink or unreadable (fail closed);
//! - a missing path is [`RemovalVerdict::Absent`], and nothing is written;
//! - an unprovable path with no `backup_parent` is preserved **in place**;
//! - an unprovable path whose parent is being torn down is copied verbatim into
//!   a fresh collision-safe `.backup-<UTC compact stamp>[-N]/` directory under
//!   `backup_parent` first, and only then removed;
//! - under `dry_run` nothing is removed or archived, and the diagnostic says
//!   what a real run *would* do.
//!
//! Deliberate deviation, fail-closed and consistent with
//! [`crate::template_tree::guard_destructive_removal`]: a symbolic link is never
//! followed to decide ownership, so a symlinked candidate is preserved.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::InstallError;
use crate::manifest::AssetManifest;
use crate::skills::ManagedSkillManifest;

/// The single package command version marker (`asset_generator` injects it into
/// `.md` bodies and inside `.toml` prompt bodies alike).
pub const VERSION_MARKER: &[u8] = b"<!-- spec-kitty-command-version:";

/// Prefix of a preservation backup directory (upstream `_BACKUP_DIR_PREFIX`).
pub const BACKUP_DIR_PREFIX: &str = ".backup-";

/// What proved package ownership of a path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnershipProof {
    /// `manifest`, `managed_path` or `canonical_content`.
    pub kind: &'static str,
    /// Operator-facing provenance, e.g. `.kittify/skills-manifest.json:<rel>`.
    pub detail: String,
}

impl OwnershipProof {
    fn manifest(detail: impl Into<String>) -> Self {
        Self {
            kind: "manifest",
            detail: detail.into(),
        }
    }

    fn managed_path(detail: impl Into<String>) -> Self {
        Self {
            kind: "managed_path",
            detail: detail.into(),
        }
    }

    fn canonical_content(detail: impl Into<String>) -> Self {
        Self {
            kind: "canonical_content",
            detail: detail.into(),
        }
    }
}

/// An ownership prover, in the shape the pinned migrations declare.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Prover {
    /// Managed-skills and/or command-skills manifest entries with matching hashes.
    Manifest,
    /// The package command version marker (or supplied canonical bytes).
    CanonicalContent,
    /// Exact byte identity with a supplied shipped counterpart (`runtime/migrate.py`'s
    /// `CanonicalContentProver(canonical=…, check_marker=False)`): the only
    /// ownership signal for a shared source asset, where a pasted marker is not
    /// proof and a differing file is preserved.
    CanonicalBytes(Vec<u8>),
    /// A declared package-managed runtime path (project-relative, POSIX).
    ManagedPath(Vec<String>),
    /// Exact byte identity — or the identical literal symlink target — with a
    /// supplied moved copy elsewhere in the project
    /// (`m_4_0_0rc6_charter_pack_cutover._MovedCopyProver`). Proof:
    /// `canonical_content`/`moved-copy:<relative>`.
    MovedCopy {
        /// The copy that must hold exactly what the candidate holds.
        copy: PathBuf,
    },
    /// Only empty directories remain in the candidate tree; a nested empty
    /// directory is accepted, and any file or symlinked directory is not
    /// (`_EmptyTreeProver`). Proof: `managed_path`/`emptied-legacy-root:<relative>`.
    EmptyTree,
    /// Whole-tree regular-file hashes (relative POSIX path → raw hex sha256)
    /// against the actual tree: an extra, missing, edited or symlinked member
    /// fails, and an empty map proves nothing (`_FrozenTreeHashProver`). Proof:
    /// `canonical_content`/`shipped-skill-hash:<relative>`.
    TreeHashes(BTreeMap<String, String>),
    /// Composite: the first component that proves ownership wins.
    Any(Vec<Prover>),
}

impl Prover {
    /// Prove package ownership of `path` under `project_root`.
    pub fn prove(&self, path: &Path, project_root: &Path) -> Option<OwnershipProof> {
        match self {
            Self::Manifest => prove_manifest(path, project_root),
            Self::CanonicalContent => prove_canonical_content(path, project_root, None, true),
            Self::CanonicalBytes(canonical) => {
                prove_canonical_content(path, project_root, Some(canonical), false)
            }
            Self::ManagedPath(declared) => {
                let relative = relative_posix(path, project_root)?;
                declared
                    .iter()
                    .any(|candidate| candidate == &relative)
                    .then(|| OwnershipProof::managed_path(format!("package-managed:{relative}")))
            }
            Self::MovedCopy { copy } => prove_moved_copy(path, project_root, copy),
            Self::EmptyTree => prove_empty_tree(path, project_root),
            Self::TreeHashes(hashes) => prove_tree_hashes(path, project_root, hashes),
            Self::Any(provers) => provers
                .iter()
                .find_map(|prover| prover.prove(path, project_root)),
        }
    }
}

/// The outcome of one guarded removal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RemovalVerdict {
    /// Proven package-owned and removed (or would be removed under `dry_run`).
    Owned {
        /// The proof that authorized the removal.
        proof: OwnershipProof,
    },
    /// The path does not exist (and is not a dangling link).
    Absent,
    /// Unprovable and kept in place.
    Preserved {
        /// The path that survives.
        path: PathBuf,
        /// Why it was kept.
        diagnostic: String,
    },
    /// Unprovable, archived verbatim, and the original removed.
    Archived {
        /// Where the bytes now live.
        path: PathBuf,
        /// Why it was archived.
        diagnostic: String,
    },
}

impl RemovalVerdict {
    /// Whether a real run removed the path (false for absent/preserved/archived
    /// and for every dry-run verdict).
    pub fn removed(&self) -> bool {
        matches!(self, Self::Owned { .. })
    }

    /// The operator-facing diagnostic, when there is one.
    pub fn diagnostic(&self) -> Option<&str> {
        match self {
            Self::Owned { .. } | Self::Absent => None,
            Self::Preserved { diagnostic, .. } | Self::Archived { diagnostic, .. } => {
                Some(diagnostic)
            }
        }
    }

    /// The path that still holds the content, when the verdict preserved it.
    pub fn preserved_path(&self) -> Option<&Path> {
        match self {
            Self::Preserved { path, .. } | Self::Archived { path, .. } => Some(path),
            Self::Owned { .. } | Self::Absent => None,
        }
    }
}

/// Prove or preserve/archive `path`, performing the removal only when owned.
///
/// `is_tree` selects recursive removal for a proven-owned directory (upstream's
/// `is_tree` flag); a file is always removed as a file. `backup_parent` is the
/// directory the preservation archive is created under, and must sit outside
/// `path` (the release-skill migration uses the project's `.kittify/`).
pub fn guard_destructive_removal(
    path: &Path,
    project_root: &Path,
    prover: &Prover,
    is_tree: bool,
    backup_parent: Option<&Path>,
    dry_run: bool,
) -> Result<RemovalVerdict, InstallError> {
    let relative = relative_display(path, project_root);
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RemovalVerdict::Absent);
        }
        Err(source) => return Err(InstallError::io("inspect", path, source)),
    };

    if let Some(proof) = prover.prove(path, project_root) {
        if !dry_run {
            remove_path(path, &metadata, is_tree)?;
        }
        return Ok(RemovalVerdict::Owned { proof });
    }

    let Some(backup_parent) = backup_parent else {
        return Ok(RemovalVerdict::Preserved {
            path: path.to_path_buf(),
            diagnostic: format!(
                "Preserved {relative}; not package-owned (name is not ownership proof) — left in place"
            ),
        });
    };

    // Archiving copies `path` into a fresh directory under `backup_parent`, so
    // the ONE unsafe relation is an archive parent that sits INSIDE the path
    // being removed (the copy would recurse into the growing archive). Source's
    // guard carries no relation check at all and the release-skill migration
    // legitimately archives a `.kittify/` candidate under `.kittify/`; an
    // earlier inverted test (`path.starts_with(backup_parent)`) therefore
    // preserved every such candidate and skipped the archive the Source
    // performs. Only the genuinely recursive relation keeps failing closed.
    if backup_parent.starts_with(path) {
        return Ok(RemovalVerdict::Preserved {
            path: path.to_path_buf(),
            diagnostic: format!(
                "Preserved {relative}; not package-owned — the archive parent sits inside \
                 the path being removed, so archiving it would recurse into itself"
            ),
        });
    }

    if dry_run {
        return Ok(RemovalVerdict::Preserved {
            path: path.to_path_buf(),
            diagnostic: format!(
                "Would preserve {relative}; not package-owned — would archive before parent removal"
            ),
        });
    }

    let archived = archive_into(path, project_root, backup_parent)?;
    remove_path(path, &metadata, is_tree)?;
    Ok(RemovalVerdict::Archived {
        diagnostic: format!(
            "Preserved {relative}; not package-owned — archived to {} before parent removal",
            relative_display(&archived, project_root)
        ),
        path: archived,
    })
}

/// Project-relative POSIX spelling, or `None` when `path` is outside the root.
pub fn relative_posix(path: &Path, project_root: &Path) -> Option<String> {
    let relative = path.strip_prefix(project_root).ok()?;
    let parts: Vec<String> = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect();
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}

/// Display spelling for a diagnostic: project-relative when possible.
pub fn relative_display(path: &Path, project_root: &Path) -> String {
    relative_posix(path, project_root).unwrap_or_else(|| path.display().to_string())
}

fn prove_manifest(path: &Path, project_root: &Path) -> Option<OwnershipProof> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if metadata.file_type().is_symlink() {
        return None;
    }
    if metadata.is_dir() {
        return prove_manifest_directory(path, project_root);
    }
    if !metadata.is_file() {
        return None;
    }
    let relative = relative_posix(path, project_root)?;
    prove_manifest_file(path, &relative, project_root)
}

fn prove_manifest_file(path: &Path, relative: &str, project_root: &Path) -> Option<OwnershipProof> {
    if let Some(manifest) = ManagedSkillManifest::load(project_root) {
        let bytes = std::fs::read(path).ok()?;
        let current = format!("sha256:{:x}", Sha256::digest(&bytes));
        if manifest.entries.iter().any(|entry| {
            entry.installed_path == relative
                && entry.delivery_mode == "copy"
                && entry.content_hash == current
        }) {
            return Some(OwnershipProof::manifest(format!(
                ".kittify/skills-manifest.json:{relative}"
            )));
        }
    }
    let manifest = AssetManifest::load(project_root).ok().flatten()?;
    let digest = crate::manifest::content_hash(&std::fs::read(path).ok()?);
    manifest
        .entries()
        .iter()
        .any(|entry| entry.path == relative && entry.content_hash == digest)
        .then(|| {
            OwnershipProof::manifest(format!(".kittify/command-skills-manifest.json:{relative}"))
        })
}

fn prove_manifest_directory(path: &Path, project_root: &Path) -> Option<OwnershipProof> {
    let mut members: Vec<PathBuf> = Vec::new();
    let mut stack = vec![path.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries = std::fs::read_dir(&current).ok()?;
        for entry in entries {
            let entry = entry.ok()?;
            let member = entry.path();
            let metadata = std::fs::symlink_metadata(&member).ok()?;
            if metadata.file_type().is_symlink() {
                return None;
            }
            if metadata.is_dir() {
                stack.push(member);
            } else if metadata.is_file() {
                members.push(member);
            } else {
                return None;
            }
        }
    }
    if members.is_empty() {
        return None;
    }
    for member in &members {
        let relative = relative_posix(member, project_root)?;
        prove_manifest_file(member, &relative, project_root)?;
    }
    let relative = relative_posix(path, project_root)?;
    Some(OwnershipProof::manifest(format!("manifest-dir:{relative}")))
}

/// `_MovedCopyProver`: a symlink proves on the identical literal target; a
/// regular file proves on exact bytes. Any other shape (or an unreadable node)
/// proves nothing.
fn prove_moved_copy(path: &Path, project_root: &Path, copy: &Path) -> Option<OwnershipProof> {
    let relative = relative_posix(path, project_root)?;
    let source_meta = std::fs::symlink_metadata(path).ok()?;
    let copy_meta = std::fs::symlink_metadata(copy).ok()?;
    let same = if source_meta.file_type().is_symlink() {
        copy_meta.file_type().is_symlink()
            && std::fs::read_link(path).ok()? == std::fs::read_link(copy).ok()?
    } else {
        !copy_meta.file_type().is_symlink()
            && source_meta.is_file()
            && copy_meta.is_file()
            && std::fs::read(path).ok()? == std::fs::read(copy).ok()?
    };
    same.then(|| OwnershipProof::canonical_content(format!("moved-copy:{relative}")))
}

/// `_EmptyTreeProver`: the candidate is a directory whose whole subtree holds
/// no file and no symlink (nested empty directories are fine).
fn prove_empty_tree(path: &Path, project_root: &Path) -> Option<OwnershipProof> {
    let relative = relative_posix(path, project_root)?;
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return None;
    }
    if !is_empty_tree(path) {
        return None;
    }
    Some(OwnershipProof::managed_path(format!(
        "emptied-legacy-root:{relative}"
    )))
}

fn is_empty_tree(directory: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(directory) else {
        // Source's os.walk has no onerror handler: an unreadable directory
        // contributes no filenames. The actual removal still reports its own
        // filesystem failure; a prover must not invent a different refusal.
        return true;
    };
    for entry in entries.flatten() {
        let Ok(metadata) = std::fs::symlink_metadata(entry.path()) else {
            return false;
        };
        if metadata.file_type().is_symlink() {
            return false;
        }
        if metadata.is_dir() {
            if !is_empty_tree(&entry.path()) {
                return false;
            }
        } else {
            return false;
        }
    }
    true
}

/// `_FrozenTreeHashProver`: every regular file's digest must equal the supplied
/// map exactly; a directory member contributes nothing, a symlink fails.
fn prove_tree_hashes(
    path: &Path,
    project_root: &Path,
    hashes: &BTreeMap<String, String>,
) -> Option<OwnershipProof> {
    if hashes.is_empty() {
        return None;
    }
    let relative = relative_posix(path, project_root)?;
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return None;
    }
    let mut found: BTreeMap<String, String> = BTreeMap::new();
    if !collect_tree_hashes(path, path, &mut found) {
        return None;
    }
    let expected: BTreeMap<String, String> = hashes
        .iter()
        .map(|(member, digest)| (member.clone(), format!("sha256:{digest}")))
        .collect();
    (found == expected)
        .then(|| OwnershipProof::canonical_content(format!("shipped-skill-hash:{relative}")))
}

fn collect_tree_hashes(root: &Path, directory: &Path, out: &mut BTreeMap<String, String>) -> bool {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return false;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            return false;
        };
        if metadata.file_type().is_symlink() {
            return false;
        }
        if metadata.is_dir() {
            if !collect_tree_hashes(root, &path, out) {
                return false;
            }
            continue;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            return false;
        };
        let Ok(bytes) = std::fs::read(&path) else {
            return false;
        };
        out.insert(
            relative.to_string_lossy().replace('\\', "/"),
            crate::skills::pack_skill::file_digest(&bytes),
        );
    }
    true
}

fn prove_canonical_content(
    path: &Path,
    project_root: &Path,
    canonical: Option<&[u8]>,
    check_marker: bool,
) -> Option<OwnershipProof> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return None;
    }
    let relative = relative_posix(path, project_root)?;
    let bytes = std::fs::read(path).ok()?;
    if check_marker && contains(&bytes, VERSION_MARKER) {
        return Some(OwnershipProof::canonical_content(format!(
            "version-marker:{relative}"
        )));
    }
    if canonical.is_some_and(|expected| expected == bytes.as_slice()) {
        return Some(OwnershipProof::canonical_content(format!(
            "canonical-bytes:{relative}"
        )));
    }
    None
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn remove_path(
    path: &Path,
    metadata: &std::fs::Metadata,
    is_tree: bool,
) -> Result<(), InstallError> {
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        if !is_tree {
            return std::fs::remove_dir(path)
                .map_err(|source| InstallError::io("remove empty directory", path, source));
        }
        return crate::pack::remove_tree_bounded(path);
    }
    std::fs::remove_file(path).map_err(|source| InstallError::io("remove", path, source))
}

/// Copy `path` verbatim into a fresh `.backup-<stamp>[-N]/` directory under
/// `backup_parent`, returning where it landed.
pub fn archive_into(
    path: &Path,
    project_root: &Path,
    backup_parent: &Path,
) -> Result<PathBuf, InstallError> {
    let backup_dir = allocate_backup_dir(backup_parent)?;
    let relative = relative_posix(path, project_root).unwrap_or_else(|| {
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "asset".to_string())
    });
    let destination = backup_dir.join(relative);
    copy_node(path, &destination)?;
    Ok(destination)
}

fn allocate_backup_dir(parent: &Path) -> Result<PathBuf, InstallError> {
    std::fs::create_dir_all(parent)
        .map_err(|source| InstallError::io("create backup parent", parent, source))?;
    let stamp = utc_compact_stamp();
    let mut suffix = 0u32;
    loop {
        let name = if suffix == 0 {
            format!("{BACKUP_DIR_PREFIX}{stamp}")
        } else {
            format!("{BACKUP_DIR_PREFIX}{stamp}-{suffix}")
        };
        let candidate = parent.join(name);
        match std::fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                suffix += 1;
            }
            Err(source) => {
                return Err(InstallError::io(
                    "create backup directory",
                    &candidate,
                    source,
                ));
            }
        }
    }
}

fn utc_compact_stamp() -> String {
    chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
}

fn copy_node(source: &Path, destination: &Path) -> Result<(), InstallError> {
    let metadata = std::fs::symlink_metadata(source)
        .map_err(|error| InstallError::io("inspect", source, error))?;
    if metadata.file_type().is_symlink() {
        let targets = std::fs::read_link(source)
            .map_err(|error| InstallError::io("read link", source, error))?;
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| InstallError::io("create directory", parent, error))?;
        }
        return symlink(&targets, destination)
            .map_err(|error| InstallError::io("create link", destination, error));
    }
    if metadata.is_dir() {
        std::fs::create_dir_all(destination)
            .map_err(|error| InstallError::io("create directory", destination, error))?;
        let entries = std::fs::read_dir(source)
            .map_err(|error| InstallError::io("read directory", source, error))?;
        let mut children: Vec<PathBuf> = entries
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<Result<_, _>>()
            .map_err(|error| InstallError::io("read directory entry", source, error))?;
        children.sort();
        for child in children {
            let name = child
                .file_name()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("member"));
            copy_node(&child, &destination.join(name))?;
        }
        return Ok(());
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| InstallError::io("create directory", parent, error))?;
    }
    let bytes = std::fs::read(source).map_err(|error| InstallError::io("read", source, error))?;
    sk_kernel::atomic_write(destination, &bytes)
        .map_err(|error| InstallError::io("write", destination, std::io::Error::other(error)))?;
    std::fs::set_permissions(destination, metadata.permissions())
        .map_err(|error| InstallError::io("preserve mode", destination, error))?;
    Ok(())
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    if target.is_dir() {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> tempfile::TempDir {
        tempfile::tempdir().expect("temp")
    }

    fn write(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn a_marked_command_file_is_proven_by_canonical_content() {
        let project = temp();
        let file = project
            .path()
            .join(".claude/commands/spec-kitty.clarify.md");
        write(
            &file,
            b"<!-- spec-kitty-command-version: 2.0.11 -->\n# clarify\n",
        );
        let verdict = guard_destructive_removal(
            &file,
            project.path(),
            &Prover::CanonicalContent,
            false,
            None,
            false,
        )
        .expect("guard");
        assert!(matches!(verdict, RemovalVerdict::Owned { .. }));
        assert!(!file.exists());
    }

    #[test]
    fn a_user_file_reusing_a_retired_name_is_preserved_in_place() {
        let project = temp();
        let file = project
            .path()
            .join(".claude/commands/spec-kitty.clarify.md");
        write(&file, b"# my own clarifying notes\n");
        let verdict = guard_destructive_removal(
            &file,
            project.path(),
            &Prover::CanonicalContent,
            false,
            None,
            false,
        )
        .expect("guard");
        assert!(matches!(verdict, RemovalVerdict::Preserved { .. }));
        assert!(file.exists());
        assert!(verdict.diagnostic().unwrap().contains("Preserved"));
    }

    #[test]
    fn an_unprovable_tree_is_archived_verbatim_before_removal() {
        let project = temp();
        let skill = project.path().join(".claude/skills/release");
        write(&skill.join("SKILL.md"), b"# my release notes\n");
        let verdict = guard_destructive_removal(
            &skill,
            project.path(),
            &Prover::Manifest,
            true,
            Some(&project.path().join(".kittify")),
            false,
        )
        .expect("guard");
        let RemovalVerdict::Archived { path, .. } = &verdict else {
            panic!("expected an archive verdict: {verdict:?}");
        };
        assert!(!skill.exists());
        assert_eq!(
            std::fs::read_to_string(path.join("SKILL.md")).unwrap(),
            "# my release notes\n"
        );
        assert!(path.starts_with(project.path().join(".kittify")));
        // The archive lives at `<backup-parent>/.backup-<stamp>/<project-relative path>`
        // (source `archive_into`), so the collision-safe directory is the first
        // component under the backup parent.
        let relative = path
            .strip_prefix(project.path().join(".kittify"))
            .expect("archive under the backup parent");
        assert!(
            relative
                .components()
                .next()
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .is_some_and(|name| name.starts_with(BACKUP_DIR_PREFIX)),
            "{relative:?}"
        );
    }

    #[test]
    fn a_manifest_entry_proves_a_copy_delivered_file_and_a_drifted_one_does_not() {
        let project = temp();
        let skill = project.path().join(".agents/skills/spk-old/SKILL.md");
        write(&skill, b"# shipped\n");
        let bytes = std::fs::read(&skill).unwrap();
        let mut manifest = ManagedSkillManifest::new("4.0.0rc8", "2026-01-01T00:00:00+00:00");
        manifest.add_entry(crate::skills::ManagedFileEntry::copied(
            "spk-old",
            "SKILL.md",
            ".agents/skills/spk-old/SKILL.md",
            "shared-root-capable",
            "codex",
            &format!("sha256:{:x}", Sha256::digest(&bytes)),
            "2026-01-01T00:00:00+00:00",
        ));
        manifest.save(project.path()).expect("save manifest");

        let verdict = guard_destructive_removal(
            &skill,
            project.path(),
            &Prover::Manifest,
            false,
            None,
            false,
        )
        .expect("guard");
        assert!(verdict.removed(), "a matching manifest entry is proof");

        write(&skill, b"# edited by the user\n");
        let verdict = guard_destructive_removal(
            &skill,
            project.path(),
            &Prover::Manifest,
            false,
            None,
            false,
        )
        .expect("guard");
        assert!(!verdict.removed(), "a drifted file is not package-owned");
        assert!(skill.exists());
    }

    #[test]
    fn a_directory_with_one_unproven_member_is_not_owned() {
        let project = temp();
        let root = project.path().join(".agents/skills/spk-old");
        write(&root.join("SKILL.md"), b"# shipped\n");
        write(&root.join("notes.md"), b"# ours\n");
        let shipped = std::fs::read(root.join("SKILL.md")).unwrap();
        let mut manifest = ManagedSkillManifest::new("4.0.0rc8", "2026-01-01T00:00:00+00:00");
        manifest.add_entry(crate::skills::ManagedFileEntry::copied(
            "spk-old",
            "SKILL.md",
            ".agents/skills/spk-old/SKILL.md",
            "shared-root-capable",
            "codex",
            &format!("sha256:{:x}", Sha256::digest(&shipped)),
            "2026-01-01T00:00:00+00:00",
        ));
        manifest.save(project.path()).expect("save manifest");
        let verdict =
            guard_destructive_removal(&root, project.path(), &Prover::Manifest, true, None, false)
                .expect("guard");
        assert!(!verdict.removed());
        assert!(root.join("notes.md").exists());
    }

    #[test]
    fn managed_path_and_composite_provers_answer_as_declared() {
        let project = temp();
        let runtime = project.path().join(".kittify/.dashboard");
        write(&runtime, b"http://127.0.0.1:8765\n8765\ntoken\n42\n");
        let prover = Prover::Any(vec![
            Prover::CanonicalContent,
            Prover::ManagedPath(vec![".kittify/.dashboard".to_string()]),
        ]);
        let verdict =
            guard_destructive_removal(&runtime, project.path(), &prover, false, None, false)
                .expect("guard");
        assert!(verdict.removed());
        assert!(!runtime.exists());
    }

    #[test]
    fn a_dry_run_reports_without_removing_or_archiving() {
        let project = temp();
        let skill = project.path().join(".claude/skills/release");
        write(&skill.join("SKILL.md"), b"# notes\n");
        let verdict = guard_destructive_removal(
            &skill,
            project.path(),
            &Prover::Manifest,
            true,
            Some(&project.path().join(".kittify")),
            true,
        )
        .expect("guard");
        assert!(!verdict.removed());
        assert!(skill.join("SKILL.md").exists());
        assert!(verdict.diagnostic().unwrap().starts_with("Would preserve"));
        assert!(!project.path().join(".kittify").join(".backup-x").exists());
    }

    #[test]
    fn an_absent_path_is_reported_absent_and_creates_nothing() {
        let project = temp();
        let verdict = guard_destructive_removal(
            &project.path().join(".claude/skills/release"),
            project.path(),
            &Prover::Manifest,
            true,
            None,
            false,
        )
        .expect("guard");
        assert_eq!(verdict, RemovalVerdict::Absent);
        assert!(!project.path().join(".claude").exists());
    }
}
