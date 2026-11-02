//! Lock authority and write-surface resolution for status writers.
//!
//! The baseline takes **one** Mission lock for every status/lifecycle write, and
//! one project-level lock for `.kittify/canonical-events.jsonl`. This module
//! reproduces the authority rules:
//!
//! - the repository root is the canonical main-repo root even when the caller
//!   holds a linked worktree path (a worktree shares the main checkout's
//!   `.git`, so both converge on one lock directory);
//! - a `kitty-specs/<slug>` feature directory is canonicalised to the main
//!   checkout's copy unless it is a *registered* coordination worktree, whose
//!   own copy is the canonical status surface;
//! - the lock **key** is the Mission directory name, or the coordination
//!   directory name `<slug>-<mid8>` when the primary `meta.json` declares
//!   `coordination_branch` — so a Mission's primary and coordination
//!   directories take the same lock file;
//! - an ad-hoc directory outside `kitty-specs/` is its own lock anchor and is
//!   never rewritten.
//!
//! The lock file itself is created and held by
//! [`sk_kernel::lock_families`]: `<git-common-dir>/spec-kitty-locks/<key>.status.lock`,
//! reentrant per thread, with the `.holder` sidecar written once per outermost
//! acquire. Nothing here runs Git or creates a project marker.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use sk_kernel::lock_families::{
    StatusLock, acquire_feature_status_lock, acquire_project_event_log_lock,
    feature_status_lock_path, project_event_log_lock_path, status_lock_common_dir,
};
use sk_kernel::locks::LockWait;

use crate::mutation::MutationError;

/// Directory that holds Mission planning artifacts relative to the repo root.
const KITTY_SPECS_DIR: &str = "kitty-specs";

/// Suffix that proposes coordination topology (the registry disposes).
const COORD_SUFFIX: &str = "-coord";

/// Worktree container directory.
const WORKTREES_SEGMENT: &str = ".worktrees";

/// Upper bound on `meta.json` (mirrors the snapshot reader).
const MAX_META_BYTES: u64 = 1024 * 1024;

/// The resolved canonical main-repo root for `start`.
///
/// Mirrors the baseline `resolve_canonical_root` on the cases that matter to
/// locking: a regular checkout returns itself; a linked worktree returns the
/// main checkout that owns the shared `.git`; a separate-git-dir/submodule
/// returns its `core.worktree` (or the checkout carrying `.kittify`); a bare
/// repository and a tree outside Git are refusals.
pub fn canonical_repo_root(start: &Path) -> Result<PathBuf, MutationError> {
    let resolved = resolve_existing(start);
    let Some(layout) = sk_kernel::git_topology::discover_git_layout(&resolved)? else {
        return Err(MutationError::RootNotFound { start: resolved });
    };
    if layout.bare {
        return Err(MutationError::RootNotFound { start: resolved });
    }
    if layout
        .common_dir
        .file_name()
        .is_some_and(|name| name == ".git")
        && let Some(parent) = layout.common_dir.parent()
    {
        return Ok(resolve_existing(parent));
    }
    if let Some(worktree) = sk_kernel::git_topology::git_configured_worktree(&layout.common_dir)? {
        return Ok(worktree);
    }
    if let Some(worktree) = layout.worktree {
        for candidate in worktree.ancestors() {
            if candidate.join(".kittify").is_dir() {
                return Ok(resolve_existing(candidate));
            }
        }
        return Ok(worktree);
    }
    Err(MutationError::RootNotFound { start: resolved })
}

/// The repo root a status write for `feature_dir` should lock against.
///
/// Mirrors `workspace.root_resolver.resolve_status_lock_root`: an explicit
/// `repo_root` wins; a directory outside `kitty-specs/` anchors on itself; a
/// `kitty-specs/<slug>` directory resolves its canonical root and falls back to
/// the historical `parent.parent` shape when the tree is not a Git repository.
pub fn resolve_lock_root(feature_dir: &Path, repo_root: Option<&Path>) -> PathBuf {
    if let Some(repo_root) = repo_root {
        return repo_root.to_path_buf();
    }
    if !is_under_kitty_specs(feature_dir) {
        return feature_dir.to_path_buf();
    }
    match canonical_repo_root(feature_dir) {
        Ok(root) => root,
        Err(_) => feature_dir
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or_else(|| feature_dir.to_path_buf()),
    }
}

/// Return the canonical `kitty-specs/<slug>` directory for a write surface.
///
/// A registered coordination worktree keeps its own directory (the #1589/#1821
/// split-brain guard). Every other `kitty-specs/<slug>` path is rewritten to the
/// main checkout's copy when that copy exists; anything else is returned
/// unchanged, so ad-hoc test directories keep working.
pub fn canonicalize_feature_dir(feature_dir: &Path) -> PathBuf {
    if !is_under_kitty_specs(feature_dir) {
        return feature_dir.to_path_buf();
    }
    if is_registered_coord_worktree(feature_dir) {
        return feature_dir.to_path_buf();
    }
    let Some(name) = feature_dir.file_name() else {
        return feature_dir.to_path_buf();
    };
    let Ok(root) = canonical_repo_root(feature_dir) else {
        return feature_dir.to_path_buf();
    };
    let candidate = root.join(KITTY_SPECS_DIR).join(name);
    if candidate.exists() {
        candidate
    } else {
        feature_dir.to_path_buf()
    }
}

/// The Mission write-lock key for `feature_dir`.
///
/// The directory name, or `<slug>-<mid8>` for a coordination-routed Mission
/// (read from the canonical primary `meta.json`). A coordination-routed Mission
/// whose mid8 cascade is exhausted is refused rather than keyed on a fabricated
/// name, matching the baseline `MissionLockKeyUnresolved` fail-closed rule.
pub fn mission_lock_key(feature_dir: &Path, lock_root: &Path) -> Result<String, MutationError> {
    let name = feature_dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| MutationError::InvalidRequest {
            reason: format!(
                "feature directory {} has no usable final path component",
                feature_dir.display()
            ),
        })?
        .to_owned();

    let meta = if is_under_kitty_specs(feature_dir) {
        read_meta(&lock_root.join(KITTY_SPECS_DIR).join(&name))
            .or_else(|| read_meta(feature_dir))
            .unwrap_or_default()
    } else {
        read_meta(feature_dir).unwrap_or_default()
    };

    lock_name_from_meta(&meta, &name)
}

fn lock_name_from_meta(
    meta: &Map<String, Value>,
    directory_name: &str,
) -> Result<String, MutationError> {
    let coordination_branch = meta
        .get("coordination_branch")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let Some(_coordination_branch) = coordination_branch else {
        return Ok(directory_name.to_owned());
    };
    let mission_slug = meta
        .get("mission_slug")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(directory_name);
    let mission_id = meta
        .get("mission_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let mid8 = meta
        .get("mid8")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let mid8 = resolve_transaction_mid8(
        mission_slug,
        mission_id,
        mid8,
        coordination_branch.is_some(),
    )
    .map_err(|_| MutationError::LockKeyUnresolved {
        directory: directory_name.to_owned(),
    })?;
    if mid8.is_empty() {
        return Ok(directory_name.to_owned());
    }
    Ok(coordination_dir_name(mission_slug, &mid8))
}

/// The baseline `resolve_transaction_mid8` cascade (`meta.mid8` → declared
/// `mission_id[:8]` → the slug's 8-character Crockford tail).
///
/// Returns the bare-slug surface (`""`) for a legacy `NNN-` slug or when no
/// coordination topology is declared, and refuses a modern coordination-routed
/// Mission whose cascade is exhausted.
fn resolve_transaction_mid8(
    mission_slug: &str,
    mission_id: Option<&str>,
    mid8: Option<&str>,
    coordination_topology: bool,
) -> Result<String, ()> {
    if let Some(mid8) = mid8 {
        return Ok(mid8.to_owned());
    }
    if let Some(mission_id) = mission_id
        && mission_id.len() >= 8
    {
        return Ok(mission_id[..8].to_owned());
    }
    if let Some(tail) = mid8_from_slug(mission_slug) {
        return Ok(tail);
    }
    if is_legacy_numbered_slug(mission_slug) {
        return Ok(String::new());
    }
    if coordination_topology {
        return Err(());
    }
    Ok(String::new())
}

fn mid8_from_slug(slug: &str) -> Option<String> {
    let (_, tail) = slug.rsplit_once('-')?;
    (tail.len() == 8 && tail.bytes().all(is_crockford_base32)).then(|| tail.to_owned())
}

fn is_crockford_base32(byte: u8) -> bool {
    byte.is_ascii_digit()
        || (byte.is_ascii_uppercase() && !matches!(byte, b'I' | b'L' | b'O' | b'U'))
}

fn is_legacy_numbered_slug(slug: &str) -> bool {
    let bytes = slug.as_bytes();
    bytes.len() > 4
        && bytes[0].is_ascii_digit()
        && bytes[1].is_ascii_digit()
        && bytes[2].is_ascii_digit()
        && bytes[3] == b'-'
}

fn coordination_dir_name(mission_slug: &str, mid8: &str) -> String {
    if mission_slug.ends_with(&format!("-{mid8}")) {
        mission_slug.to_owned()
    } else {
        format!("{mission_slug}-{mid8}")
    }
}

/// The Mission status lock path for `feature_dir`.
pub fn mission_lock_path(
    feature_dir: &Path,
    repo_root: Option<&Path>,
) -> Result<PathBuf, MutationError> {
    let lock_root = resolve_lock_root(feature_dir, repo_root);
    let common_dir = status_lock_common_dir(&lock_root)?;
    let key = mission_lock_key(feature_dir, &lock_root)?;
    Ok(feature_status_lock_path(&common_dir, &key)?)
}

/// The project-level canonical-event-log lock path for `repo_root`.
pub fn project_lock_path(repo_root: &Path) -> Result<PathBuf, MutationError> {
    let common_dir = status_lock_common_dir(repo_root)?;
    Ok(project_event_log_lock_path(&common_dir))
}

/// A held Mission status lock. Neither `Send` nor `Sync`, like the kernel guard.
#[must_use = "hold the Mission lock for the duration of the protected write"]
pub struct MissionLock {
    guard: StatusLock,
    key: String,
    lock_root: PathBuf,
}

impl MissionLock {
    /// The lock file held by this guard.
    pub fn path(&self) -> &Path {
        self.guard.path()
    }

    /// The Mission lock key (directory name or coordination directory name).
    pub fn key(&self) -> &str {
        &self.key
    }

    /// The repository root the lock was resolved against.
    pub fn lock_root(&self) -> &Path {
        &self.lock_root
    }

    /// The recorded `.holder` sidecar, when readable.
    pub fn holder(&self) -> Option<sk_kernel::StatusLockHolder> {
        self.guard.holder()
    }
}

impl std::fmt::Debug for MissionLock {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MissionLock")
            .field("path", &self.guard.path())
            .field("key", &self.key)
            .finish()
    }
}

/// A held project-level canonical-event-log lock.
#[must_use = "hold the project lock for the duration of the protected write"]
pub struct ProjectLock {
    guard: StatusLock,
}

impl ProjectLock {
    /// The lock file held by this guard.
    pub fn path(&self) -> &Path {
        self.guard.path()
    }

    /// The recorded `.holder` sidecar, when readable.
    pub fn holder(&self) -> Option<sk_kernel::StatusLockHolder> {
        self.guard.holder()
    }
}

impl std::fmt::Debug for ProjectLock {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProjectLock")
            .field("path", &self.guard.path())
            .finish()
    }
}

/// Acquire the Mission status lock (reentrant per thread).
///
/// `wait` is the caller's policy: a bounded wait for ordinary writes
/// (`Duration::from_secs(10)` in the baseline's outage-shaped callers) or
/// [`LockWait::Unbounded`] for claim windows. Contention inside a bounded wait
/// is [`sk_kernel::KernelError::StatusLockTimeout`], which carries the recorded
/// holder in its message.
pub fn acquire_mission_lock(
    feature_dir: &Path,
    repo_root: Option<&Path>,
    wait: LockWait,
) -> Result<MissionLock, MutationError> {
    let lock_root = resolve_lock_root(feature_dir, repo_root);
    let common_dir = status_lock_common_dir(&lock_root)?;
    let key = mission_lock_key(feature_dir, &lock_root)?;
    let guard = acquire_feature_status_lock(&common_dir, &key, wait)?;
    Ok(MissionLock {
        guard,
        key,
        lock_root,
    })
}

/// Acquire the project-level canonical-event-log lock (reentrant per thread).
pub fn acquire_project_lock(
    repo_root: &Path,
    wait: LockWait,
) -> Result<ProjectLock, MutationError> {
    let common_dir = status_lock_common_dir(repo_root)?;
    let guard = acquire_project_event_log_lock(&common_dir, wait)?;
    Ok(ProjectLock { guard })
}

fn is_under_kitty_specs(path: &Path) -> bool {
    path.parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name == KITTY_SPECS_DIR)
}

/// The nearest `.worktrees/<name>` ancestor of `path`, when there is one.
fn enclosing_worktree_root(path: &Path) -> Option<PathBuf> {
    for candidate in path.ancestors() {
        if candidate
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == WORKTREES_SEGMENT)
        {
            return Some(candidate.to_path_buf());
        }
    }
    None
}

/// True when `path` lives in a **registered** worktree whose name ends with
/// `-coord`.
///
/// The `-coord` suffix only proposes coordination topology; the Git worktree
/// registry disposes. The registry is read from the filesystem
/// (`<git-common-dir>/worktrees/*/gitdir`) instead of `git worktree list
/// --porcelain`, because a status writer never shells out. When the registry
/// cannot be read at all, the baseline's shape fallback is used.
fn is_registered_coord_worktree(path: &Path) -> bool {
    let Some(worktree_root) = enclosing_worktree_root(path) else {
        return false;
    };
    let coord_shaped = worktree_root
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(COORD_SUFFIX));
    match registered_worktrees(path) {
        Some(registry) => coord_shaped && registry.contains(&resolve_existing(&worktree_root)),
        None => coord_shaped,
    }
}

fn registered_worktrees(path: &Path) -> Option<Vec<PathBuf>> {
    let common_dir = sk_kernel::git_topology::git_common_dir(path).ok()?;
    let mut registered = Vec::new();
    if common_dir.file_name().is_some_and(|name| name == ".git")
        && let Some(main_root) = common_dir.parent()
    {
        registered.push(resolve_existing(main_root));
    }
    let entries = fs::read_dir(common_dir.join("worktrees")).ok()?;
    for entry in entries.flatten() {
        let gitdir_file = entry.path().join("gitdir");
        let Ok(text) = fs::read_to_string(&gitdir_file) else {
            continue;
        };
        let Some(raw) = text.trim().strip_prefix("gitdir:") else {
            continue;
        };
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let candidate = Path::new(raw);
        let candidate = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            entry.path().join(candidate)
        };
        registered.push(resolve_existing(&candidate));
    }
    Some(registered)
}

fn read_meta(directory: &Path) -> Option<Map<String, Value>> {
    let path = directory.join("meta.json");
    let bytes = sk_kernel::read_bounded(&path, MAX_META_BYTES).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    value.as_object().cloned()
}

fn resolve_existing(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}
