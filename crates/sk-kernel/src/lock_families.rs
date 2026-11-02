//! Named lock families used by the status and checkout-claim writers.
//!
//! Upstream keeps these rules in `specify_cli.status.locking`, on top of
//! `kernel.locks`. The kernel floor owns the parts every writer must agree on:
//! the lock filename, the `.holder` sidecar contract, the wait policy, and the
//! checkout-claim-before-Mission ordering rule. The status owner still owns the
//! whole-file status write, verification and recovery protocol; this module
//! never writes `status.events.jsonl` and never appends to it through the
//! generic event-log helper.
//!
//! # Families
//!
//! - **Mission status**: `<common>/spec-kitty-locks/<mission-dir-name>.status.lock`.
//!   Keyed by the mission directory name, never by a bare slug. Reentrant per
//!   thread, bounded by default in the outage-shaped callers, and unbounded in
//!   the claim windows.
//! - **Project event log**: `<common>/spec-kitty-locks/__project__.status.lock`.
//!   A fixed sentinel because `ProjectInitialized` has no mission directory.
//! - **Checkout claim**: `<common>/spec-kitty-locks/__checkout-<sha1[0..16]>__.status.lock`.
//!   Anchor-keyed (the resolved, case-normalised write checkout), SHA-1 because
//!   that name has been stable since #5819. Taken **before** any Mission lock.
//!
//! Each family's caller owns the common-dir probe and its degrade policy. This
//! module takes an already-resolved common dir, exactly like
//! [`crate::locks::anchored_lock_path`].

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::KernelError;
use crate::clock::now_utc_iso;
use crate::locks::LOCK_DIRECTORY;
use crate::locks::{
    AnchorDigestAlgorithm, AnchoredLockName, FileLock, LockOptions, LockWait,
    anchored_lock_path_with,
};

/// Suffix of a named status lock.
pub const STATUS_LOCK_SUFFIX: &str = ".status.lock";

/// Fixed lock key for the project-level canonical event log.
pub const PROJECT_LOCK_KEY: &str = "__project__";

/// Sidecar suffix recording the current status-lock holder.
pub const HOLDER_SIDECAR_SUFFIX: &str = ".holder";

/// Prefix of the Mission checkout-claim lock filename.
pub const CHECKOUT_CLAIM_LOCK_PREFIX: &str = "__checkout-";

/// Suffix of the Mission checkout-claim lock filename.
pub const CHECKOUT_CLAIM_LOCK_SUFFIX: &str = "__.status.lock";

/// SHA-1 hex length in the checkout-claim lock filename.
pub const CHECKOUT_CLAIM_DIGEST_LENGTH: usize = 16;

/// Default bounded wait for the single-branch checkout claim.
pub const CHECKOUT_CLAIM_LOCK_TIMEOUT: Duration = Duration::from_secs(120);

/// A readable `.holder` sidecar for a status-family lock.
///
/// This is **not** the lock file's own [`crate::LockHolder`] record. Upstream
/// writes its record into the lock inode and truncates it on release, so the
/// status family keeps a separate `<lock>.holder` file that it can write and
/// clear exactly once per outermost acquire. The sidecar is advisory
/// diagnostics; it is never a staleness or liveness authority.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StatusLockHolder {
    /// Process that holds the lock.
    pub pid: u32,
    /// Name of the holding thread, or `"unknown"`.
    pub thread: String,
    /// UTC ISO-8601 instant at which the outermost acquire happened.
    pub acquired_at: String,
}

impl StatusLockHolder {
    /// The sidecar path for `lock_path`.
    pub fn sidecar_path(lock_path: &Path) -> PathBuf {
        let Some(name) = lock_path.file_name() else {
            return lock_path.with_extension("holder");
        };
        let mut sidecar = name.to_os_string();
        sidecar.push(HOLDER_SIDECAR_SUFFIX);
        lock_path.with_file_name(sidecar)
    }

    /// Build a record for the current process/thread.
    pub fn current() -> Self {
        let thread = std::thread::current()
            .name()
            .map(str::to_owned)
            .unwrap_or_else(|| "unknown".to_owned());
        Self {
            pid: std::process::id(),
            thread,
            acquired_at: now_utc_iso(),
        }
    }

    /// Write the sidecar for `lock_path`.
    ///
    /// Upstream treats this as best-effort: a failure must not fail the lock
    /// acquisition. Callers that want that behaviour use
    /// [`StatusLockHolder::record_best_effort`]; this strict form exists so a
    /// test or an operator tool can observe the failure.
    pub fn record(lock_path: &Path, holder: &StatusLockHolder) -> Result<(), KernelError> {
        let sidecar = Self::sidecar_path(lock_path);
        // Upstream writes `json.dumps(payload, sort_keys=True)`. A BTreeMap
        // pins that byte shape regardless of serializer feature unification.
        let mut payload: std::collections::BTreeMap<&str, serde_json::Value> =
            std::collections::BTreeMap::new();
        payload.insert("acquired_at", holder.acquired_at.clone().into());
        payload.insert("pid", holder.pid.into());
        payload.insert("thread", holder.thread.clone().into());
        let bytes = serde_json::to_vec(&payload).map_err(|error| KernelError::Io {
            action: "serialize status lock holder for",
            path: sidecar.clone(),
            source: std::io::Error::other(error),
        })?;
        fs::write(&sidecar, bytes)
            .map_err(|source| KernelError::io("write status lock holder for", sidecar, source))
    }

    /// Best-effort sidecar write; any failure is discarded, as upstream does.
    pub fn record_best_effort(lock_path: &Path) {
        let _ = Self::record(lock_path, &Self::current());
    }

    /// Remove the sidecar for `lock_path`; an absent sidecar is success.
    pub fn clear(lock_path: &Path) -> Result<(), KernelError> {
        let sidecar = Self::sidecar_path(lock_path);
        match fs::remove_file(&sidecar) {
            Ok(()) => Ok(()),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(KernelError::io(
                "remove status lock holder",
                sidecar,
                source,
            )),
        }
    }

    /// Best-effort sidecar removal.
    pub fn clear_best_effort(lock_path: &Path) {
        let _ = Self::clear(lock_path);
    }

    /// Read the sidecar; absent or malformed content is `None`.
    pub fn read(lock_path: &Path) -> Option<Self> {
        let sidecar = Self::sidecar_path(lock_path);
        let bytes = crate::read_bounded(&sidecar, 64 * 1024).ok()?;
        serde_json::from_slice(&bytes).ok()
    }
}

/// Render a sidecar for a timeout message.
pub fn describe_status_lock_holder(holder: Option<&StatusLockHolder>) -> String {
    match holder {
        None => "holder unknown (no holder record)".to_owned(),
        Some(holder) => format!(
            "held by pid {} (thread {}) since {}",
            holder.pid, holder.thread, holder.acquired_at
        ),
    }
}

/// Resolve the common dir a status/checkout family should use, with upstream's
/// degrade policy.
///
/// `specify_cli.status.locking._git_common_dir` prefers the canonical Git
/// common dir and, when the probe cannot answer, degrades to a still-functional
/// per-checkout location:
///
/// 1. `<repo>/.git` when it is an existing directory (a real checkout whose
///    probe failed transiently);
/// 2. `<repo>/.kittify` when that existing directory marks a non-Git project;
/// 3. `<repo>/.spec-kitty` otherwise — a directory name no project-root walk
///    treats as a marker, so a lock never turns the tree into a fake checkout.
///
/// This never creates a directory and never probes with a subprocess.
pub fn status_lock_common_dir(repo_root: &Path) -> Result<PathBuf, KernelError> {
    crate::git_topology::ensure_writer_git_environment(repo_root)?;
    match crate::git_topology::git_common_dir(repo_root) {
        Ok(common) => Ok(common),
        Err(KernelError::NotAGitRepository { .. } | KernelError::GitTopologyUnavailable { .. }) => {
            let dot_git = repo_root.join(".git");
            if dot_git.is_dir() {
                return Ok(dot_git);
            }
            let kittify = repo_root.join(".kittify");
            if kittify.is_dir() {
                return Ok(kittify);
            }
            Ok(repo_root.join(".spec-kitty"))
        }
        Err(error) => Err(error),
    }
}

/// Return the per-mission status lock path.
///
/// `lock_key` must be the mission **directory name** (`<slug>-<mid8>`), never a
/// bare slug. A key with a path separator or a `.`/`..` component is refused so
/// a caller cannot address a lock outside the family directory. The checkout
/// claim and project log use fixed keys and are not anchor-keyed.
pub fn feature_status_lock_path(common_dir: &Path, lock_key: &str) -> Result<PathBuf, KernelError> {
    validate_lock_key(lock_key)?;
    Ok(common_dir
        .join(LOCK_DIRECTORY)
        .join(format!("{lock_key}{STATUS_LOCK_SUFFIX}")))
}

/// Return the project-level canonical-event-log lock path.
pub fn project_event_log_lock_path(common_dir: &Path) -> PathBuf {
    common_dir
        .join(LOCK_DIRECTORY)
        .join(format!("{PROJECT_LOCK_KEY}{STATUS_LOCK_SUFFIX}"))
}

/// Return the Mission checkout-claim lock path for `write_checkout`.
///
/// The anchor digest and placement are the shared anchored rule; this family
/// only fixes the prefix, suffix, SHA-1 algorithm, and 16-hex length that the
/// live lock names have used since #5819.
pub fn checkout_claim_lock_path(
    write_checkout: &Path,
    common_dir: &Path,
) -> Result<PathBuf, KernelError> {
    anchored_lock_path_with(
        write_checkout,
        common_dir,
        None,
        &AnchoredLockName::new()
            .with_prefix(CHECKOUT_CLAIM_LOCK_PREFIX)
            .with_suffix(CHECKOUT_CLAIM_LOCK_SUFFIX)
            .with_sha1()
            .with_digest_hex_length(CHECKOUT_CLAIM_DIGEST_LENGTH),
    )
}

/// Whether `lock_key` (a path or a bare name) is a checkout-claim lock.
pub fn is_checkout_claim_lock_key(lock_key: &str) -> bool {
    let name = Path::new(lock_key)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(lock_key);
    name.starts_with(CHECKOUT_CLAIM_LOCK_PREFIX) && name.ends_with(CHECKOUT_CLAIM_LOCK_SUFFIX)
}

/// Digest algorithm used by the checkout-claim family.
pub const CHECKOUT_CLAIM_DIGEST_ALGORITHM: AnchorDigestAlgorithm = AnchorDigestAlgorithm::Sha1;

thread_local! {
    /// Per-thread outer-acquire depths for status-family locks, keyed by the
    /// canonical lock identity, shared with FileLock (including aliased paths).
    static HELD_STATUS: RefCell<(u32, HashMap<PathBuf, usize>)> =
        RefCell::new((std::process::id(), HashMap::new()));
}

/// A held status-family lock plus its sidecar bookkeeping.
///
/// The guard is neither `Send` nor `Sync` because it owns a [`FileLock`]. The
/// sidecar is written on the outermost acquire and cleared only when the
/// outermost guard drops, so a nested block cannot erase an enclosing holder's
/// record.
#[must_use = "hold the status lock for the duration of the protected transaction"]
pub struct StatusLock {
    owner_pid: u32,
    path: PathBuf,
    key: PathBuf,
    _lock: FileLock,
}

impl StatusLock {
    /// Acquire a status-family lock with an explicit wait policy.
    ///
    /// The acquire is same-thread reentrant, matching upstream's
    /// `reentrant=True` status family. Contention inside a bounded wait maps to
    /// [`KernelError::StatusLockTimeout`], including the `.holder` sidecar text
    /// when one is readable.
    pub fn acquire(path: &Path, wait: LockWait) -> Result<Self, KernelError> {
        let key = crate::locks::canonical_lock_key(path)?;
        let depth = held_depth(&key);
        let options = LockOptions {
            wait,
            reentrant: true,
        };
        let lock = FileLock::acquire_with(path, options).map_err(|error| match error {
            KernelError::LockTimeout { path, timeout } => status_timeout(&path, timeout),
            other => other,
        })?;
        set_held_depth(&key, depth + 1);
        if depth == 0 {
            StatusLockHolder::record_best_effort(path);
        }
        Ok(Self {
            owner_pid: std::process::id(),
            path: path.to_path_buf(),
            key,
            _lock: lock,
        })
    }

    /// Acquire with a bounded timeout.
    pub fn acquire_bounded(path: &Path, timeout: Duration) -> Result<Self, KernelError> {
        Self::acquire(path, LockWait::Bounded(timeout))
    }

    /// Acquire without a deadline (the claim windows).
    pub fn acquire_unbounded(path: &Path) -> Result<Self, KernelError> {
        Self::acquire(path, LockWait::Unbounded)
    }

    /// The lock file held by this guard.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The recorded sidecar holder, when readable.
    pub fn holder(&self) -> Option<StatusLockHolder> {
        StatusLockHolder::read(&self.path)
    }
}

impl std::fmt::Debug for StatusLock {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StatusLock")
            .field("path", &self.path)
            .finish()
    }
}

impl Drop for StatusLock {
    fn drop(&mut self) {
        // A fork child must not clear the parent's advisory holder or release
        // its bookkeeping. FileLock applies the same PID fence to the OS guard.
        if self.owner_pid != std::process::id() {
            return;
        }
        let depth = held_depth(&self.key);
        if depth <= 1 {
            remove_held(&self.key);
            // Upstream releases the OS lock first and then clears the sidecar;
            // the field drop below performs the release, so clear here and let
            // the lock release immediately afterwards.
            StatusLockHolder::clear_best_effort(&self.path);
        } else {
            set_held_depth(&self.key, depth - 1);
        }
    }
}

/// Acquire a Mission status lock.
pub fn acquire_feature_status_lock(
    common_dir: &Path,
    lock_key: &str,
    wait: LockWait,
) -> Result<StatusLock, KernelError> {
    let path = feature_status_lock_path(common_dir, lock_key)?;
    StatusLock::acquire(&path, wait)
}

/// Acquire the project-level canonical-event-log lock.
pub fn acquire_project_event_log_lock(
    common_dir: &Path,
    wait: LockWait,
) -> Result<StatusLock, KernelError> {
    let path = project_event_log_lock_path(common_dir);
    StatusLock::acquire(&path, wait)
}

/// Acquire a Mission checkout-claim lock, enforcing lock order first.
///
/// The fixed order is **checkout claim first, then any Mission write lock**.
/// Taking a claim while this thread already holds a non-checkout status lock
/// would risk a deadlock against a claimant that takes them in the right order,
/// so it is refused with [`KernelError::LockOrderViolation`] instead.
pub fn acquire_checkout_claim_lock(
    write_checkout: &Path,
    common_dir: &Path,
    wait: LockWait,
) -> Result<StatusLock, KernelError> {
    let path = checkout_claim_lock_path(write_checkout, common_dir)?;
    let key = crate::locks::canonical_lock_key(&path)?;
    if held_depth(&key) == 0 {
        let foreign = held_foreign_keys(&key);
        if !foreign.is_empty() {
            return Err(KernelError::LockOrderViolation {
                held: foreign.join(", "),
            });
        }
    }
    StatusLock::acquire(&path, wait)
}

fn status_timeout(path: &Path, timeout: Duration) -> KernelError {
    let holder = StatusLockHolder::read(path);
    KernelError::StatusLockTimeout {
        path: path.to_path_buf(),
        timeout: format!("{}s", timeout.as_secs_f64()),
        holder: describe_status_lock_holder(holder.as_ref()),
    }
}

fn with_held_status<T>(f: impl FnOnce(&mut HashMap<PathBuf, usize>) -> T) -> T {
    HELD_STATUS.with(|state| {
        let mut state = state.borrow_mut();
        let pid = std::process::id();
        if state.0 != pid {
            state.0 = pid;
            state.1.clear();
        }
        f(&mut state.1)
    })
}

fn held_depth(key: &Path) -> usize {
    with_held_status(|held| held.get(key).copied().unwrap_or(0))
}

fn set_held_depth(key: &Path, depth: usize) {
    with_held_status(|held| {
        held.insert(key.to_owned(), depth);
    });
}

fn remove_held(key: &Path) {
    with_held_status(|held| {
        held.remove(key);
    });
}

fn held_foreign_keys(checkout_key: &Path) -> Vec<String> {
    with_held_status(|held| {
        let mut keys: Vec<String> = held
            .iter()
            .filter(|(key, depth)| **depth > 0 && key.as_path() != checkout_key)
            .filter(|(key, _)| !is_checkout_claim_lock_key(&key.to_string_lossy()))
            .map(|(key, _)| key.to_string_lossy().into_owned())
            .collect();
        keys.sort();
        keys
    })
}

fn validate_lock_key(lock_key: &str) -> Result<(), KernelError> {
    let path = Path::new(lock_key);
    let single_component = {
        let mut components = path.components();
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
    };
    if lock_key.is_empty()
        || !single_component
        || lock_key == "."
        || lock_key == ".."
        || lock_key.contains(['/', '\\', '\0'])
    {
        return Err(KernelError::UnsafeLockName {
            fragment: lock_key.to_owned(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_common_dir_degrades_without_creating_a_marker() {
        let temp = tempfile::tempdir().unwrap();
        let bare = temp.path().join("bare");
        fs::create_dir_all(&bare).unwrap();
        assert_eq!(
            status_lock_common_dir(&bare).unwrap(),
            bare.join(".spec-kitty")
        );
        assert!(!bare.join(".spec-kitty").exists());

        let kittify = temp.path().join("kittify");
        fs::create_dir_all(kittify.join(".kittify")).unwrap();
        assert_eq!(
            status_lock_common_dir(&kittify).unwrap(),
            kittify.join(".kittify")
        );

        let checkout = temp.path().join("checkout");
        fs::create_dir_all(checkout.join(".git")).unwrap();
        assert_eq!(
            status_lock_common_dir(&checkout).unwrap(),
            checkout.join(".git")
        );
    }

    #[test]
    fn inherited_status_depths_do_not_become_child_authority() {
        HELD_STATUS.with(|state| {
            let mut state = state.borrow_mut();
            state.0 = std::process::id().wrapping_add(1);
            state.1.insert(PathBuf::from("parent.status.lock"), 1);
        });
        assert_eq!(held_depth(Path::new("parent.status.lock")), 0);
        assert!(held_foreign_keys(Path::new("__checkout-child__.status.lock")).is_empty());
    }

    #[test]
    fn mission_lock_key_is_the_directory_name() {
        let common = Path::new("/repo/.git");
        assert_eq!(
            feature_status_lock_path(common, "mission-01ABCDEF").unwrap(),
            Path::new("/repo/.git/spec-kitty-locks/mission-01ABCDEF.status.lock")
        );
        assert!(feature_status_lock_path(common, "a/b").is_err());
        assert!(feature_status_lock_path(common, "..").is_err());
    }

    #[test]
    fn project_key_is_the_fixed_sentinel() {
        assert_eq!(
            project_event_log_lock_path(Path::new("/repo/.git")),
            Path::new("/repo/.git/spec-kitty-locks/__project__.status.lock")
        );
    }

    #[test]
    fn checkout_claim_name_uses_sha1_and_sixteen_hex() {
        let temp = tempfile::tempdir().unwrap();
        let path = checkout_claim_lock_path(temp.path(), Path::new("/repo/.git")).unwrap();
        let name = path.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with("__checkout-"));
        assert!(name.ends_with("__.status.lock"));
        assert_eq!(
            name.len(),
            "__checkout-".len() + 16 + "__.status.lock".len()
        );
        assert!(is_checkout_claim_lock_key(&name));
        assert!(!is_checkout_claim_lock_key("mission.status.lock"));
    }

    #[test]
    fn holder_sidecar_is_sorted_json_and_describes_itself() {
        let temp = tempfile::tempdir().unwrap();
        let lock = temp.path().join("m.status.lock");
        let holder = StatusLockHolder {
            pid: 7,
            thread: "worker".to_owned(),
            acquired_at: "2026-01-01T00:00:00+00:00".to_owned(),
        };
        StatusLockHolder::record(&lock, &holder).unwrap();
        let raw = fs::read_to_string(StatusLockHolder::sidecar_path(&lock)).unwrap();
        assert_eq!(
            raw,
            r#"{"acquired_at":"2026-01-01T00:00:00+00:00","pid":7,"thread":"worker"}"#
        );
        assert_eq!(StatusLockHolder::read(&lock), Some(holder));
        assert!(
            describe_status_lock_holder(StatusLockHolder::read(&lock).as_ref()).contains("pid 7")
        );
        StatusLockHolder::clear(&lock).unwrap();
        assert_eq!(StatusLockHolder::read(&lock), None);
    }
}
