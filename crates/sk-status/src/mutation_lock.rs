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
//!
//! # The shared Mission-write protocol
//!
//! This is the native counterpart of the baseline
//! `specify_cli.status.mission_write` module, not merely a renamed lock file:
//!
//! - **one key per Mission** — [`mission_lock_key`] mirrors the baseline cascade
//!   (`meta.mid8` → declared `mission_id` → slug tail) and the legacy `NNN-` /
//!   flattened carve-outs, failing closed for a modern coordination Mission whose
//!   identity is lost;
//! - **canonical-primary metadata** — the key is read from the primary
//!   `kitty-specs/<name>/meta.json` of the *main* checkout, never from a lane
//!   worktree's stale copy; a linked-worktree CWD still converges on the main
//!   checkout's lock directory;
//! - **registered holds** — while a thread holds a Mission lock, a nested call for
//!   either directory name (and, for a coordination directory, its primary alias)
//!   returns the held key instead of re-deriving it, so a meta write inside the
//!   hold cannot silently move the lock;
//! - **rekey extension** — [`extend_mission_lock_to_moved_key`] additionally holds
//!   the key `meta.json` records *now*, for the one caller that mints identity
//!   inside its own hold;
//! - **birth locks** — [`acquire_mission_birth_lock`] keys on the directory name
//!   because a create's `meta.json` does not exist yet;
//! - **uniform reentrancy** — every acquire goes through [`sk_kernel::StatusLock`],
//!   which is PID-fenced and same-thread reentrant.
//!
//! A `MissionLock` is neither `Send` nor `Sync` (it owns a kernel guard), so the
//! registered-hold bookkeeping can never be moved to another thread.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

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

/// Bound for a Mission write lock take (the baseline's shared 10 s bound).
pub const MISSION_WRITE_LOCK_TIMEOUT: Duration = Duration::from_secs(10);

/// Length of the `-<mid8>` suffix on a coordination directory name.
const MID8_SUFFIX_LENGTH: usize = 9;

/// Upper bound on a `.git` pointer file read.
const MAX_GIT_MARKER_BYTES: u64 = 64 * 1024;

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

/// The Mission write-lock key for `feature_dir`, against an explicit lock root.
///
/// This is the held-aware door: while the calling thread holds a Mission lock
/// for the same directory name under `lock_root`, the held key is returned even
/// if `meta.json` changed inside the hold. Use [`mission_lock_key_with`] with
/// `ignore_held = true` to see the key the Mission records now.
pub fn mission_lock_key(feature_dir: &Path, lock_root: &Path) -> Result<String, MutationError> {
    mission_lock_key_with(feature_dir, Some(lock_root), false)
}

/// The Mission write-lock key for `feature_dir`.
///
/// The directory name, or `<slug>-<mid8>` for a coordination-routed Mission
/// (read from the canonical primary `meta.json`). A coordination-routed Mission
/// whose mid8 cascade is exhausted is refused rather than keyed on a fabricated
/// name, matching the baseline `MissionLockKeyUnresolved` fail-closed rule.
///
/// `repo_root` follows [`resolve_lock_root`]: an explicit root wins, an ad-hoc
/// directory outside `kitty-specs/` anchors on itself, and a `kitty-specs/<slug>`
/// directory resolves the canonical main checkout. `ignore_held` skips the
/// thread's registered-hold lookup and reads `meta.json` now; only
/// [`extend_mission_lock_to_moved_key`] needs that.
pub fn mission_lock_key_with(
    feature_dir: &Path,
    repo_root: Option<&Path>,
    ignore_held: bool,
) -> Result<String, MutationError> {
    let lock_root = resolve_lock_root(feature_dir, repo_root);
    let directory_name = feature_dir
        .file_name()
        .ok_or_else(|| MutationError::InvalidRequest {
            reason: format!(
                "feature directory {} has no usable final path component",
                feature_dir.display()
            ),
        })?;
    if !ignore_held && let Some(held) = held_key(&lock_root, directory_name) {
        return Ok(held);
    }
    let name = directory_name
        .to_str()
        .ok_or_else(|| MutationError::InvalidRequest {
            reason: format!(
                "feature directory {} has a non-UTF-8 name",
                feature_dir.display()
            ),
        })?;
    let meta = if is_under_kitty_specs(feature_dir) {
        canonical_primary_meta(&lock_root, name)?.unwrap_or_default()
    } else {
        read_meta(feature_dir)?.unwrap_or_default()
    };
    lock_name_from_meta(&meta, name)
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
    // The baseline's `_optional_text` keeps a non-empty string byte-for-byte;
    // it does not strip a recorded identity. Only the coordination branch is
    // tested after trimming (to decide whether topology is declared).
    let mission_slug = optional_text(meta, "mission_slug").unwrap_or(directory_name);
    let mission_id = optional_text(meta, "mission_id");
    let mid8 = optional_text(meta, "mid8");
    let mid8 = resolve_transaction_mid8(mission_slug, mission_id, mid8, true).map_err(|_| {
        MutationError::LockKeyUnresolved {
            directory: directory_name.to_owned(),
        }
    })?;
    if mid8.is_empty() {
        return Ok(mission_slug.to_owned());
    }
    Ok(coordination_dir_name(mission_slug, &mid8))
}

fn optional_text<'a>(meta: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    meta.get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
}

/// The canonical primary `meta.json` of `name` under `root`.
///
/// Mirrors the baseline `_primary_meta`: a root whose `.git` is a directory is
/// a repository checkout and is read directly; any other root follows a
/// worktree pointer to the main checkout (plan A4), so a lane worktree's stale
/// copy never decides the key. When the main checkout records nothing for
/// `name` (an owned linked worktree whose Mission exists only in its own
/// checkout), that checkout's own copy is the only record there is.
fn canonical_primary_meta(
    root: &Path,
    name: &str,
) -> Result<Option<Map<String, Value>>, MutationError> {
    if !is_single_segment(name) {
        return Ok(None);
    }
    let own = read_meta(&root.join(KITTY_SPECS_DIR).join(name))?;
    if root.join(".git").is_dir() {
        return Ok(own);
    }
    let main_root = main_repo_root(root);
    let canonical = if main_root == root {
        own.clone()
    } else {
        read_meta(&main_root.join(KITTY_SPECS_DIR).join(name))?
    };
    Ok(canonical.or(own))
}

/// The main checkout root for `root`: a worktree `.git` pointer is followed to
/// the repository root; anything else resolves to itself.
fn main_repo_root(root: &Path) -> PathBuf {
    let marker = root.join(".git");
    if marker.is_file()
        && let Ok(bytes) = sk_kernel::read_bounded(&marker, MAX_GIT_MARKER_BYTES)
        && let Ok(text) = String::from_utf8(bytes)
        && let Some(raw) = text.trim().strip_prefix("gitdir:")
    {
        let raw = raw.trim();
        if !raw.is_empty() {
            let gitdir = Path::new(raw);
            if is_worktree_gitdir(gitdir)
                && let Some(main_root) = gitdir
                    .parent()
                    .and_then(Path::parent)
                    .and_then(Path::parent)
            {
                return resolve_existing(main_root);
            }
        }
    }
    resolve_existing(root)
}

fn is_worktree_gitdir(gitdir: &Path) -> bool {
    let Some(name) = gitdir.parent().and_then(Path::file_name) else {
        return false;
    };
    if name != "worktrees" {
        return false;
    }
    gitdir
        .parent()
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .is_some_and(|name| name.to_string_lossy().ends_with(".git"))
}

/// A path component usable as a single `kitty-specs/` directory name.
fn is_single_segment(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && Path::new(name)
            .file_name()
            .is_some_and(|file_name| file_name == OsStr::new(name))
}

/// The baseline `resolve_transaction_mid8` cascade (`meta.mid8` → declared
/// `mission_id[:8]` → the slug's 8-character Crockford tail).
///
/// Returns the bare-slug surface (`""`) for a legacy `NNN-` slug or when no
/// coordination topology is declared, and refuses a modern coordination-routed
/// Mission whose cascade is exhausted. `mission_id` is untrusted metadata, so
/// the prefix is taken by **characters**, never by an unchecked byte offset.
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
        && mission_id.chars().count() >= 8
    {
        return Ok(mission_id.chars().take(8).collect());
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

/// The primary directory name whose Mission is keyed `key`, when `key` is a
/// coordination name.
///
/// A hold taken through the coordination-named directory must also be found by
/// a nested lock taken through the primary directory after the Mission's meta
/// changed inside the hold (plan A4). `None` unless the primary `meta.json`
/// composes exactly `key`.
fn primary_alias(root: &Path, key: &str) -> Result<Option<String>, MutationError> {
    let characters: Vec<char> = key.chars().collect();
    if characters.len() <= MID8_SUFFIX_LENGTH
        || characters[characters.len() - MID8_SUFFIX_LENGTH] != '-'
    {
        return Ok(None);
    }
    let candidate: String = characters[..characters.len() - MID8_SUFFIX_LENGTH]
        .iter()
        .collect();
    let meta = canonical_primary_meta(root, &candidate)?.unwrap_or_default();
    if lock_name_from_meta(&meta, &candidate)? == key {
        Ok(Some(candidate))
    } else {
        Ok(None)
    }
}

/// The project-level canonical-event-log lock path for `repo_root`.
pub fn project_lock_path(repo_root: &Path) -> Result<PathBuf, MutationError> {
    let common_dir = status_lock_common_dir(repo_root)?;
    Ok(project_event_log_lock_path(&common_dir))
}

/// A registered same-thread hold, restored on drop.
///
/// The map entry is the baseline's per-thread `(lock root, directory name) ->
/// lock key` bookkeeping. `setdefault` semantics matter: an inner registration
/// for a slot an outer hold already owns must not overwrite it, and dropping the
/// inner registration must restore exactly the previous value.
struct HoldRegistration {
    entries: Vec<(HeldSlot, Option<String>)>,
}

impl HoldRegistration {
    /// Register `key` for every directory name in `directory_names` (and for a
    /// directory named after the key itself).
    fn register(root: &Path, directory_names: &[&OsStr], key: &str) -> Self {
        let root = resolve_existing(root);
        let mut slots: Vec<HeldSlot> = directory_names
            .iter()
            .map(|name| (root.clone(), (*name).to_os_string()))
            .collect();
        slots.push((root, OsString::from(key)));
        let mut entries: Vec<(HeldSlot, Option<String>)> =
            slots.iter().map(|slot| (slot.clone(), None)).collect();
        with_held(|held| {
            // Capture every previous value before any insertion, exactly like
            // the baseline `registered_hold`'s up-front `previous` map, so a
            // duplicated slot restores correctly.
            for (slot, before) in &mut entries {
                *before = held.get(slot).cloned();
            }
            for slot in &slots {
                held.entry(slot.clone()).or_insert_with(|| key.to_owned());
            }
        });
        Self { entries }
    }
}

impl Drop for HoldRegistration {
    fn drop(&mut self) {
        let entries = std::mem::take(&mut self.entries);
        with_held(|held| {
            for (slot, before) in entries {
                match before {
                    Some(value) => {
                        held.insert(slot, value);
                    }
                    None => {
                        held.remove(&slot);
                    }
                }
            }
        });
    }
}

/// A held key for one Mission directory, as this thread registered it.
fn held_key(root: &Path, directory_name: &OsStr) -> Option<String> {
    let slot = (resolve_existing(root), directory_name.to_os_string());
    with_held(|held| held.get(&slot).cloned())
}

/// A held Mission status lock. Neither `Send` nor `Sync`, like the kernel guard.
///
/// Field order is load-bearing: the registered holds are released before the OS
/// lock, matching the baseline context-manager exit order.
#[must_use = "hold the Mission lock for the duration of the protected write"]
pub struct MissionLock {
    _holds: HoldRegistration,
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
/// ([`MISSION_WRITE_LOCK_TIMEOUT`] in the baseline's outage-shaped callers) or
/// [`LockWait::Unbounded`] for claim windows. Contention inside a bounded wait
/// is [`sk_kernel::KernelError::StatusLockTimeout`], which carries the recorded
/// holder in its message.
///
/// The key is resolved **before** the lock is entered; the primary directory
/// name and (for a coordination-named directory) its primary alias are then
/// registered for the duration of the hold.
pub fn acquire_mission_lock(
    feature_dir: &Path,
    repo_root: Option<&Path>,
    wait: LockWait,
) -> Result<MissionLock, MutationError> {
    acquire_mission_lock_with(feature_dir, repo_root, wait, false, false)
}

/// Acquire the Mission status lock with the baseline's create/upgrade knobs.
///
/// `fallback_to_dir_name` is for migration/upgrade writers that repair legacy
/// metadata a key cannot be read from: a coordination-routed Mission that
/// records no resolvable mid8 is locked on its directory name instead of
/// [`MutationError::LockKeyUnresolved`]. `birth` is the create's `meta.json`
/// birth write only: the Mission records no `meta.json` yet, so the key is the
/// directory name and nothing is resolved (an owned checkout's create never
/// re-derives the repository root from its own path).
pub fn acquire_mission_lock_with(
    feature_dir: &Path,
    repo_root: Option<&Path>,
    wait: LockWait,
    fallback_to_dir_name: bool,
    birth: bool,
) -> Result<MissionLock, MutationError> {
    let lock_root = resolve_lock_root(feature_dir, repo_root);
    let directory_name = feature_dir
        .file_name()
        .ok_or_else(|| MutationError::InvalidRequest {
            reason: format!(
                "feature directory {} has no usable final path component",
                feature_dir.display()
            ),
        })?;
    let directory_name_text =
        directory_name
            .to_str()
            .ok_or_else(|| MutationError::InvalidRequest {
                reason: format!(
                    "feature directory {} has a non-UTF-8 name",
                    feature_dir.display()
                ),
            })?;
    let key = if birth {
        directory_name_text.to_owned()
    } else {
        match mission_lock_key_with(feature_dir, Some(&lock_root), false) {
            Ok(key) => key,
            Err(MutationError::LockKeyUnresolved { .. }) if fallback_to_dir_name => {
                directory_name_text.to_owned()
            }
            Err(error) => return Err(error),
        }
    };
    let alias = if !birth && directory_name_text == key {
        primary_alias(&lock_root, &key)?
    } else {
        None
    };
    let common_dir = status_lock_common_dir(&lock_root)?;
    let guard = acquire_feature_status_lock(&common_dir, &key, wait)?;
    let alias_name = alias.as_deref().unwrap_or(directory_name_text);
    let holds =
        HoldRegistration::register(&lock_root, &[directory_name, OsStr::new(alias_name)], &key);
    Ok(MissionLock {
        _holds: holds,
        guard,
        key,
        lock_root,
    })
}

/// Acquire the Mission lock for a create's `meta.json` birth write.
///
/// The key is the Mission directory name: there is no `meta.json` to read yet,
/// so the birth lock serialises pre-meta writers of that directory (and the
/// post-birth `<slug>-<mid8>` key converges because create directory names
/// embed the mid8). No metadata is read and no key can be unresolved.
pub fn acquire_mission_birth_lock(
    feature_dir: &Path,
    repo_root: Option<&Path>,
    wait: LockWait,
) -> Result<MissionLock, MutationError> {
    acquire_mission_lock_with(feature_dir, repo_root, wait, false, true)
}

/// A hold that additionally covers the key a `meta.json` write moved inside an
/// enclosing [`acquire_mission_lock`] hold.
///
/// Drop releases the moved key's lock before the enclosing hold's, as the
/// baseline `extend_mission_write_lock_to_moved_key` context manager does.
#[must_use = "keep the extension guard for the part of the hold that depends on the moved key"]
pub struct MissionLockExtension {
    _holds: Option<HoldRegistration>,
    lock: Option<StatusLock>,
    moved: bool,
}

impl MissionLockExtension {
    /// Whether the key actually moved and is now additionally held.
    pub fn moved(&self) -> bool {
        self.moved
    }

    /// The moved key's lock file, when one was taken.
    pub fn path(&self) -> Option<&Path> {
        self.lock.as_ref().map(StatusLock::path)
    }
}

impl std::fmt::Debug for MissionLockExtension {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MissionLockExtension")
            .field("moved", &self.moved)
            .field("path", &self.path())
            .finish()
    }
}

/// While this thread's hold of `mission_dir`'s Mission lock lasts, also hold the
/// key `meta.json` records now.
///
/// A writer that changes `coordination_branch` or `mid8` inside a Mission lock
/// hold (for example the mission-state repair minting a `mission_id`) moves the
/// Mission's lock key, but [`mission_lock_key`] keeps returning the *held* key to
/// this thread, while every other process already locks the new key's file. Call
/// this inside the hold after such a write: it reports `moved() == false` when
/// the key did not move (or cannot be resolved), and `true` once the new key's
/// lock is held as well, until the returned guard drops. Callers re-read what
/// they read before the move, because a writer holding only the new key may have
/// appended in between.
///
/// Lock order is held key first, then the new key. Every other writer takes
/// exactly one Mission key, so there is no wait cycle.
///
/// # Errors
///
/// [`MutationError::ExtensionRequiresHold`] when this thread holds no Mission
/// lock for `mission_dir`; a timeout acquiring the moved key is the kernel's
/// [`sk_kernel::KernelError::StatusLockTimeout`].
pub fn extend_mission_lock_to_moved_key(
    mission_dir: &Path,
    repo_root: Option<&Path>,
    wait: LockWait,
) -> Result<MissionLockExtension, MutationError> {
    let lock_root = resolve_lock_root(mission_dir, repo_root);
    let directory_name = mission_dir
        .file_name()
        .ok_or_else(|| MutationError::InvalidRequest {
            reason: format!(
                "feature directory {} has no usable final path component",
                mission_dir.display()
            ),
        })?;
    if held_key(&lock_root, directory_name).is_none() {
        return Err(MutationError::ExtensionRequiresHold {
            directory: directory_name.to_string_lossy().into_owned(),
        });
    }
    let new_key = match mission_lock_key_with(mission_dir, Some(&lock_root), true) {
        Ok(key) => Some(key),
        Err(MutationError::LockKeyUnresolved { .. }) => None,
        Err(error) => return Err(error),
    };
    let Some(new_key) =
        new_key.filter(|key| held_key(&lock_root, directory_name).as_deref() != Some(key.as_str()))
    else {
        return Ok(MissionLockExtension {
            _holds: None,
            lock: None,
            moved: false,
        });
    };
    let common_dir = status_lock_common_dir(&lock_root)?;
    let lock = acquire_feature_status_lock(&common_dir, &new_key, wait)?;
    let holds = HoldRegistration::register(&lock_root, &[OsStr::new(&new_key)], &new_key);
    Ok(MissionLockExtension {
        _holds: Some(holds),
        lock: Some(lock),
        moved: true,
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

/// Per-thread `(real lock root, directory name) -> held lock key` map.
type HeldSlot = (PathBuf, OsString);

struct HeldState {
    pid: u32,
    slots: HashMap<HeldSlot, String>,
}

thread_local! {
    static HELD_KEYS: RefCell<HeldState> = RefCell::new(HeldState {
        pid: std::process::id(),
        slots: HashMap::new(),
    });
}

/// Run `f` against this thread's hold map, discarding inherited (forked) state.
fn with_held<T>(f: impl FnOnce(&mut HashMap<HeldSlot, String>) -> T) -> T {
    HELD_KEYS.with(|state| {
        let mut state = state.borrow_mut();
        let pid = std::process::id();
        if state.pid != pid {
            state.pid = pid;
            state.slots.clear();
        }
        f(&mut state.slots)
    })
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

fn read_meta(directory: &Path) -> Result<Option<Map<String, Value>>, MutationError> {
    let path = directory.join("meta.json");
    let bytes = match sk_kernel::read_bounded(&path, MAX_META_BYTES) {
        Ok(bytes) => bytes,
        Err(error @ sk_kernel::KernelError::TooLarge { .. }) => return Err(error.into()),
        Err(_) => return Ok(None),
    };
    Ok(serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|value| value.as_object().cloned()))
}

fn resolve_existing(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}
