//! One door to cross-process advisory file locking.
//!
//! This module is the kernel's only lock primitive. Every durable writer that
//! needs mutual exclusion (event-log appends today; status, queues, and
//! Mission claims as they land) uses [`FileLock`] rather than opening its own
//! OS lock. The safe `fs2` abstraction is used deliberately: the workspace
//! forbids `unsafe`, and raw `flock(2)`/`LockFileEx` calls would be the only
//! reason to break that rule.
//!
//! # Contract
//!
//! [`FileLock::acquire`] takes an exclusive, cross-process lock and returns an
//! RAII guard:
//!
//! - **Dedicated lock-only path.** `path` exists only to be locked. It is never
//!   a payload path: acquire truncates and rewrites the file with holder
//!   metadata. Passing a payload path would destroy that payload.
//! - **Explicit wait policy.** [`FileLock::acquire`] is bounded: contention is
//!   retried until `timeout` elapses, then [`KernelError::LockTimeout`] is
//!   returned, and `Duration::ZERO` means exactly one non-blocking attempt.
//!   [`LockOptions`] additionally exposes the upstream per-family policies:
//!   one non-blocking attempt ([`KernelError::LockNotAcquired`]) or an
//!   explicitly unbounded wait for the claim windows. An unbounded wait is
//!   never implicit.
//! - **No unlink on release.** Release truncates the holder record but keeps the
//!   inode. Unlinking would let a contender create and lock a fresh inode while
//!   a third writer still holds the old one, defeating mutual exclusion.
//! - **Holder diagnostics.** While held, the file contains a JSON
//!   [`LockHolder`] record (schema version, pid, start time, host, version).
//!   [`LockHolder::read`] reads it without taking the lock, for operator
//!   diagnostics and timeout reports. It is advisory metadata, never a
//!   liveness or stale-lock authority.
//! - **Same-thread reentrancy.** [`FileLock::acquire`] reuses a held lock for
//!   the same resolved path on the same thread (no second OS lock, no
//!   deadlock); the outermost guard releases. [`LockOptions::non_reentrant`]
//!   opts a family out, in which case the same thread contends normally. Other
//!   threads and processes always contend normally.
//! - **No subprocess while held.** Kernel code never invokes Git or another
//!   process to discover a lock location. Callers must not run a child process
//!   while holding a kernel lock; lock ownership is not inherited across
//!   process spawn, and holding a lock across a child would make the timeout
//!   the only recovery.
//! - **Non-regular paths refused.** FIFOs, devices, and directories are refused
//!   after a no-follow open. A planted final-component symlink is refused.
//!
//! This is local-filesystem mutual exclusion, not a distributed lock. Network
//! filesystems with broken `flock` semantics are outside the guarantee.
//!
//! # Anchored paths
//!
//! [`anchored_lock_path`] derives a stable lock filename from a resolved anchor
//! path and places it under `<common-dir>/spec-kitty-locks[/<subdirectory>]/`.
//! Every worktree of a repository shares the Git common dir, so writers of one
//! anchor converge on one lock. The digest is the first 32 lower-case hex
//! characters of SHA-256 over the resolved, case-normalised anchor identity.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::marker::PhantomData;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::KernelError;
use crate::clock::{now_utc_iso, parse_iso};
use crate::content_digest::{sha1_prefix, sha256_prefix};
use crate::fs_resolve::{map_resolve_failure, resolve_non_strict};

/// Directory name, under a checkout's Git common dir, holding lock files.
pub const LOCK_DIRECTORY: &str = "spec-kitty-locks";

/// Truncation length of the anchor digest in an anchored lock filename.
pub const ANCHOR_DIGEST_HEX_LENGTH: usize = 32;

/// Default bounded wait used by the upstream sync lock facade.
pub const DEFAULT_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(10);

/// Default age (seconds) above which upstream calls a holder record stale.
pub const STALE_AFTER_S_DEFAULT: f64 = 60.0;

/// Largest holder record this module will read back for diagnostics.
const MAX_HOLDER_RECORD_BYTES: u64 = 64 * 1024;

/// Lock-file record schema version.
pub const LOCK_HOLDER_SCHEMA_VERSION: u32 = 1;

/// Retry interval while waiting for a contended lock.
const RETRY_INTERVAL: Duration = Duration::from_millis(10);

/// How long an acquire may wait for a contended lock.
///
/// This is the explicit per-family wait policy upstream exposes as
/// `blocking` / `timeout_s`: the status and checkout-claim families queue
/// (bounded or unbounded), while run-cursor and migration probes use a single
/// non-blocking attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LockWait {
    /// One attempt. Contention is [`KernelError::LockNotAcquired`].
    NonBlocking,
    /// Retry until the deadline. `Duration::ZERO` is one attempt and reports
    /// [`KernelError::LockTimeout`].
    Bounded(Duration),
    /// Retry until acquired. Only for the bounded set of callers whose
    /// upstream contract passes `timeout_s=None`; a stuck holder is then a
    /// caller-visible outage rather than a silent success.
    Unbounded,
}

/// Per-family lock acquisition policy.
///
/// [`Default`] matches upstream `kernel.locks.SyncMachineFileLock`'s defaults:
/// one non-blocking attempt and **no** same-thread reentrancy. The frozen
/// [`FileLock::acquire`] contract keeps its historical bounded-10s/reentrant
/// behaviour by building its options explicitly rather than relying on this
/// default.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LockOptions {
    /// Wait strategy for a contended lock.
    pub wait: LockWait,
    /// Reuse a lock this thread already holds for the same resolved path.
    pub reentrant: bool,
}

impl Default for LockOptions {
    fn default() -> Self {
        Self {
            wait: LockWait::NonBlocking,
            reentrant: false,
        }
    }
}

impl LockOptions {
    /// One non-blocking attempt, non-reentrant (upstream sync default).
    pub const fn nonblocking() -> Self {
        Self {
            wait: LockWait::NonBlocking,
            reentrant: false,
        }
    }

    /// Bounded retry, non-reentrant (upstream `blocking=True, timeout_s=t`).
    pub const fn bounded(timeout: Duration) -> Self {
        Self {
            wait: LockWait::Bounded(timeout),
            reentrant: false,
        }
    }

    /// Wait forever, non-reentrant (upstream `blocking=True, timeout_s=None`).
    pub const fn unbounded() -> Self {
        Self {
            wait: LockWait::Unbounded,
            reentrant: false,
        }
    }

    /// Reuse this thread's existing holding for the same resolved path.
    pub const fn reentrant(mut self) -> Self {
        self.reentrant = true;
        self
    }

    /// Never reuse a same-thread holding; a second acquire contends normally.
    pub const fn non_reentrant(mut self) -> Self {
        self.reentrant = false;
        self
    }
}

/// Digest algorithm allowed in an anchored lock filename.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnchorDigestAlgorithm {
    /// SHA-256, the default for the event-log and synthesis families.
    Sha256,
    /// SHA-1, the historical Mission checkout-claim discriminator only.
    Sha1,
}

/// Name policy for an anchored lock file.
///
/// The default reproduces the pre-existing [`anchored_lock_path`] shape:
/// `<digest[0..32]>.lock` with SHA-256. Prefix and suffix must be plain file
/// name fragments; a separator is refused so a family cannot escape the lock
/// directory through its own name policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnchoredLockName {
    /// Text before the digest.
    pub prefix: String,
    /// Text after the digest.
    pub suffix: String,
    /// Digest algorithm.
    pub digest_algorithm: AnchorDigestAlgorithm,
    /// Number of lower-case hex digest characters to keep, 1..=64.
    pub digest_hex_length: usize,
}

impl Default for AnchoredLockName {
    fn default() -> Self {
        Self {
            prefix: String::new(),
            suffix: ".lock".to_owned(),
            digest_algorithm: AnchorDigestAlgorithm::Sha256,
            digest_hex_length: ANCHOR_DIGEST_HEX_LENGTH,
        }
    }
}

impl AnchoredLockName {
    /// Default name: `<sha256[0..32]>.lock`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the filename prefix.
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// Set the filename suffix.
    pub fn with_suffix(mut self, suffix: impl Into<String>) -> Self {
        self.suffix = suffix.into();
        self
    }

    /// Select SHA-1 (checkout-claim compatibility only).
    pub fn with_sha1(mut self) -> Self {
        self.digest_algorithm = AnchorDigestAlgorithm::Sha1;
        self
    }

    /// Set the truncated digest length.
    pub fn with_digest_hex_length(mut self, length: usize) -> Self {
        self.digest_hex_length = length;
        self
    }
}

/// Holder metadata persisted inside a held lock file.
///
/// The field names and meanings match upstream `kernel.locks.LockRecord`:
/// `schema_version`, `pid`, ISO-8601 `started_at`, `host`, and `version`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LockHolder {
    /// Content schema version (currently [`LOCK_HOLDER_SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// Operating-system process id of the holder.
    pub pid: u32,
    /// UTC ISO-8601 instant at which the lock was taken.
    pub started_at: String,
    /// Hostname of the machine that wrote the record, or `"unknown"`.
    pub host: String,
    /// Version of the writing crate, or `"unknown"`.
    pub version: String,
}

impl LockHolder {
    /// Read the holder record from `path` without taking the lock.
    ///
    /// Returns `Ok(None)` for an absent or empty lock file (no holder). A
    /// malformed record is [`KernelError::LockHolderMalformed`], never silently
    /// ignored.
    ///
    /// On Windows a held lock is mandatory: the holder's record sits inside a
    /// range another process may refuse to read. That case is
    /// [`KernelError::LockHolderUnreadable`], never a false `Ok(None)` and
    /// never a claim that the file has no holder. Callers rendering operator
    /// diagnostics should use [`LockHolder::read_diagnostics`], which degrades
    /// to `Ok(None)` for that specific platform limitation; the status
    /// family's own `.holder` sidecar remains the source of truth there.
    pub fn read(path: &Path) -> Result<Option<Self>, KernelError> {
        match Self::try_read(path) {
            Ok(holder) => Ok(holder),
            Err(error) => Err(error),
        }
    }

    /// Read the holder record, degrading an unreadable mandatory-locked range
    /// to `Ok(None)`.
    ///
    /// This is for diagnostics only. It does not make the record optional at
    /// the filesystem level; [`LockHolder::read`] stays the strict door.
    pub fn read_diagnostics(path: &Path) -> Result<Option<Self>, KernelError> {
        match Self::try_read(path) {
            Ok(holder) => Ok(holder),
            Err(KernelError::LockHolderUnreadable { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn try_read(path: &Path) -> Result<Option<Self>, KernelError> {
        let bytes = match crate::read_bounded(path, MAX_HOLDER_RECORD_BYTES) {
            Ok(bytes) => bytes,
            Err(KernelError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(KernelError::Io { source, .. }) if is_mandatory_lock_violation(&source) => {
                return Err(KernelError::LockHolderUnreadable {
                    path: path.to_path_buf(),
                    reason: source.to_string(),
                });
            }
            Err(KernelError::Io { source, .. })
                if source.kind() == io::ErrorKind::PermissionDenied =>
            {
                // Windows reports ERROR_LOCK_VIOLATION as a generic access
                // denial in some runtimes. Keep the distinction explicit
                // rather than pretending the file has no holder.
                return Err(KernelError::LockHolderUnreadable {
                    path: path.to_path_buf(),
                    reason: source.to_string(),
                });
            }
            Err(KernelError::TooLarge { .. }) => {
                return Err(KernelError::LockHolderMalformed {
                    path: path.to_path_buf(),
                    reason: format!("holder record exceeds {MAX_HOLDER_RECORD_BYTES} bytes"),
                });
            }
            Err(error) => return Err(error),
        };
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return Ok(None);
        }
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| KernelError::LockHolderMalformed {
                path: path.to_path_buf(),
                reason: error.to_string(),
            })
    }

    /// Seconds since the record was taken, clamped at zero.
    pub fn age_s(&self) -> f64 {
        match parse_iso(&self.started_at) {
            Ok(started) => {
                let age = (crate::clock::now_utc() - started).num_milliseconds() as f64 / 1000.0;
                if age > 0.0 { age } else { 0.0 }
            }
            Err(_) => 0.0,
        }
    }

    /// Whether the record is older than `threshold_s` (upstream `is_stuck`).
    pub fn is_stuck(&self, threshold_s: f64) -> bool {
        self.age_s() > threshold_s
    }
}

struct Holding {
    file: File,
    holder: LockHolder,
    depth: AtomicUsize,
}

impl Holding {
    fn release(&self) {
        // fork-without-exec inherits descriptors and thread-local entries.
        // A child must close its copy, never mutate the parent's holder file
        // or explicitly unlock their shared open-file description.
        if self.holder.pid != std::process::id() {
            return;
        }
        // Release explicitly after clearing the holder. A concurrent fork can
        // inherit this open file description until exec; closing only our fd
        // would leave the lock held beyond the owning guard's lifetime.
        // Only the owning PID reaches this unlock; a forked child must not
        // unlock the shared description while its parent still owns it.
        let _ = self.file.set_len(0);
        let _ = self.file.sync_data();
        let _ = FileExt::unlock(&self.file);
    }
}

impl Drop for Holding {
    fn drop(&mut self) {
        // Safety net for a guard whose explicit depth release did not run.
        // Holder cleanup and explicit unlock are idempotent.
        self.release();
    }
}

thread_local! {
    static HELD: RefCell<HashMap<PathBuf, Weak<Holding>>> = RefCell::new(HashMap::new());
}

/// An RAII exclusive lock on a dedicated lock file.
///
/// Dropping the last guard for this thread/path truncates the holder record and
/// releases the OS lock. Guards are neither `Send` nor `Sync`: thread-local
/// reentrancy cannot be invalidated by moving a guard to a different thread.
/// Acquire a separate guard on the thread that will perform the transaction.
///
/// ```compile_fail
/// fn move_guard(lock: sk_kernel::FileLock) {
///     std::thread::spawn(move || drop(lock));
/// }
/// ```
#[must_use = "hold the lock guard for the duration of the protected transaction"]
pub struct FileLock {
    path: PathBuf,
    key: PathBuf,
    holding: Arc<Holding>,
    /// Whether this guard participates in the thread-local reentrancy map.
    registered: bool,
    _thread_bound: PhantomData<Rc<()>>,
}

impl FileLock {
    /// Acquire an exclusive cross-process lock on `path`.
    ///
    /// `path` is a dedicated lock-only file. Its parent is created (mode `0700`
    /// on Unix when this call creates it). The wait is bounded by `timeout`;
    /// `Duration::ZERO` performs one non-blocking attempt. This frozen contract
    /// is same-thread reentrant, matching every existing consumer.
    pub fn acquire(path: &Path, timeout: Duration) -> Result<Self, KernelError> {
        Self::acquire_with(path, LockOptions::bounded(timeout).reentrant())
    }

    /// Acquire with an explicit per-family wait/reentrancy policy.
    pub fn acquire_with(path: &Path, options: LockOptions) -> Result<Self, KernelError> {
        let key = canonical_lock_key(path)?;

        if options.reentrant
            && let Some(holding) = held_by_this_thread(&key)
        {
            holding.depth.fetch_add(1, Ordering::SeqCst);
            return Ok(Self {
                path: path.to_path_buf(),
                key,
                holding,
                registered: true,
                _thread_bound: PhantomData,
            });
        }

        ensure_lock_parent(path)?;

        let mut open = OpenOptions::new();
        open.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            open.mode(0o600);
        }
        let mut file = crate::no_follow::open_no_follow(path, &mut open)?;
        let metadata = file
            .metadata()
            .map_err(|source| KernelError::io("inspect lock", path, source))?;
        if !metadata.is_file() {
            return Err(KernelError::io(
                "lock",
                path,
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "lock path is not a regular file",
                ),
            ));
        }

        acquire_exclusive(&file, path, options.wait)?;

        let holder = LockHolder {
            schema_version: LOCK_HOLDER_SCHEMA_VERSION,
            pid: std::process::id(),
            started_at: now_utc_iso(),
            host: host_name(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        };
        if let Err(error) = write_holder(&mut file, path, &holder) {
            let _ = FileExt::unlock(&file);
            return Err(error);
        }

        let holding = Arc::new(Holding {
            file,
            holder,
            depth: AtomicUsize::new(1),
        });
        if options.reentrant {
            HELD.with(|held| {
                held.borrow_mut()
                    .insert(key.clone(), Arc::downgrade(&holding));
            });
        }
        Ok(Self {
            path: path.to_path_buf(),
            key,
            holding,
            registered: options.reentrant,
            _thread_bound: PhantomData,
        })
    }

    /// One non-blocking attempt; `Ok(None)` means another holder has it.
    ///
    /// This is the `try_*` form upstream uses for run cursors and migration
    /// probes: contention is a normal outcome, not a timeout.
    pub fn try_acquire(path: &Path) -> Result<Option<Self>, KernelError> {
        match Self::acquire_with(path, LockOptions::nonblocking().non_reentrant()) {
            Ok(lock) => Ok(Some(lock)),
            Err(KernelError::LockNotAcquired { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Wait until the lock is acquired, with no deadline (upstream
    /// `blocking=True, timeout_s=None`).
    pub fn acquire_unbounded(path: &Path) -> Result<Self, KernelError> {
        Self::acquire_with(path, LockOptions::unbounded().reentrant())
    }

    /// The lock path this guard was acquired for, exactly as supplied.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Holder metadata for this guard.
    pub fn holder(&self) -> &LockHolder {
        &self.holding.holder
    }

    /// Set permissions on the HELD lock inode, never by reopening its path.
    ///
    /// A persistent owner-lock family may require an explicit mode. The path
    /// can have been replaced since acquisition; that foreign node is not ours.
    pub fn set_permissions(&self, permissions: std::fs::Permissions) -> Result<(), KernelError> {
        if self.holding.holder.pid != std::process::id() {
            return Err(KernelError::io(
                "set lock permissions",
                &self.path,
                io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "lock belongs to another process",
                ),
            ));
        }
        self.holding
            .file
            .set_permissions(permissions)
            .map_err(|source| KernelError::io("set lock permissions", &self.path, source))
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        if self.holding.holder.pid != std::process::id() {
            return;
        }
        // Every guard is one reentrancy level. Only the level that reaches
        // zero releases, so dropping an outer guard before an inner one does
        // not strand the inner guard or remove the thread-local entry.
        if self.holding.depth.fetch_sub(1, Ordering::SeqCst) == 1 {
            // The last Arc drops Holding immediately after this method,
            // clearing the holder and explicitly releasing its OS lock.
            if self.registered {
                let key = self.key.clone();
                HELD.with(|held| {
                    held.borrow_mut().remove(&key);
                });
            }
        }
    }
}

impl std::fmt::Debug for FileLock {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FileLock")
            .field("path", &self.path)
            .field("holder", &self.holding.holder)
            .finish()
    }
}

/// Return the anchored lock path for `anchor` under `common_dir`.
///
/// `anchor` is resolved once, so a symlinked or relative spelling of the same
/// file names the same lock. `subdirectory` is a relative, traversal-free path
/// under `spec-kitty-locks`; `None` places the file directly in that directory.
/// Nothing is created and no lock is taken.
pub fn anchored_lock_path(
    anchor: &Path,
    common_dir: &Path,
    subdirectory: Option<&Path>,
) -> Result<PathBuf, KernelError> {
    anchored_lock_path_with(
        anchor,
        common_dir,
        subdirectory,
        &AnchoredLockName::default(),
    )
}

/// Return an anchored lock path with an explicit name policy.
///
/// The digest is always over the resolved, case-normalised anchor identity;
/// only the algorithm, truncation, prefix, and suffix vary by family. This is
/// the additive surface for the Mission checkout-claim family
/// (`__checkout-<sha1[0..16]>__.status.lock`) and any future named family.
pub fn anchored_lock_path_with(
    anchor: &Path,
    common_dir: &Path,
    subdirectory: Option<&Path>,
    name: &AnchoredLockName,
) -> Result<PathBuf, KernelError> {
    validate_lock_name_fragment(&name.prefix)?;
    validate_lock_name_fragment(&name.suffix)?;
    let resolved =
        resolve_non_strict(anchor).map_err(|failure| map_resolve_failure(anchor, failure))?;
    let identity = anchor_identity(&resolved);
    let digest = match name.digest_algorithm {
        AnchorDigestAlgorithm::Sha256 => sha256_prefix(&identity, name.digest_hex_length)?,
        AnchorDigestAlgorithm::Sha1 => sha1_prefix(&identity, name.digest_hex_length)?,
    };

    let mut path = common_dir.join(LOCK_DIRECTORY);
    if let Some(subdirectory) = subdirectory {
        reject_unsafe_subdirectory(subdirectory)?;
        path.push(subdirectory);
    }
    path.push(format!("{}{}{}", name.prefix, digest, name.suffix));
    Ok(path)
}

fn validate_lock_name_fragment(fragment: &str) -> Result<(), KernelError> {
    let has_separator = fragment
        .chars()
        .any(|character| matches!(character, '/' | '\\' | '\0'));
    if has_separator || fragment == "." || fragment == ".." {
        return Err(KernelError::UnsafeLockName {
            fragment: fragment.to_owned(),
        });
    }
    Ok(())
}

fn reject_unsafe_subdirectory(subdirectory: &Path) -> Result<(), KernelError> {
    if subdirectory.is_absolute()
        || subdirectory.components().any(|component| {
            matches!(
                component,
                Component::Prefix(_) | Component::RootDir | Component::ParentDir
            )
        })
    {
        return Err(KernelError::ParentTraversal {
            path: subdirectory.to_path_buf(),
        });
    }
    Ok(())
}

#[cfg(unix)]
fn anchor_identity(resolved: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    resolved.as_os_str().as_bytes().to_vec()
}

#[cfg(not(unix))]
fn anchor_identity(resolved: &Path) -> Vec<u8> {
    // Windows path identity is case-insensitive. `normcase` in the Python
    // baseline lower-cases and normalises separators; `to_string_lossy` plus
    // lower-casing reproduces the practical contract. Non-Unicode paths on
    // Windows are not representable in this digest; that limitation is
    // documented rather than silently ignored.
    resolved.to_string_lossy().to_lowercase().into_bytes()
}

pub(crate) fn canonical_lock_key(path: &Path) -> Result<PathBuf, KernelError> {
    let resolved =
        resolve_non_strict(path).map_err(|failure| map_resolve_failure(path, failure))?;
    #[cfg(windows)]
    {
        Ok(PathBuf::from(resolved.to_string_lossy().to_lowercase()))
    }
    #[cfg(not(windows))]
    {
        Ok(resolved)
    }
}

fn held_by_this_thread(key: &Path) -> Option<Arc<Holding>> {
    HELD.with(|held| {
        let mut held = held.borrow_mut();
        match held.get(key).and_then(Weak::upgrade) {
            Some(holding) if holding.holder.pid == std::process::id() => Some(holding),
            Some(_) | None => {
                held.remove(key);
                None
            }
        }
    })
}

fn acquire_exclusive(file: &File, path: &Path, wait: LockWait) -> Result<(), KernelError> {
    let deadline = match wait {
        LockWait::NonBlocking => None,
        LockWait::Bounded(timeout) => Instant::now().checked_add(timeout),
        LockWait::Unbounded => None,
    };
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(()),
            Err(source) if is_contention(&source) => {
                match wait {
                    LockWait::NonBlocking => {
                        return Err(KernelError::LockNotAcquired {
                            path: path.to_path_buf(),
                        });
                    }
                    LockWait::Bounded(timeout) => {
                        if timeout.is_zero()
                            || deadline.is_none_or(|deadline| Instant::now() >= deadline)
                        {
                            return Err(KernelError::LockTimeout {
                                path: path.to_path_buf(),
                                timeout,
                            });
                        }
                    }
                    LockWait::Unbounded => {}
                }
                let remaining = deadline
                    .map(|deadline| deadline.saturating_duration_since(Instant::now()))
                    .unwrap_or(RETRY_INTERVAL)
                    .min(RETRY_INTERVAL);
                std::thread::sleep(if remaining.is_zero() {
                    Duration::from_millis(1)
                } else {
                    remaining
                });
            }
            Err(source) => {
                return Err(KernelError::io("lock", path, source));
            }
        }
    }
}

fn is_mandatory_lock_violation(error: &io::Error) -> bool {
    #[cfg(windows)]
    {
        // ERROR_LOCK_VIOLATION / ERROR_IO_PENDING are what LockFileEx and a
        // read that crosses a mandatory-locked range can report.
        matches!(error.raw_os_error(), Some(33 | 997))
    }
    #[cfg(not(windows))]
    {
        let _ = error;
        false
    }
}

fn is_contention(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(unix)]
    {
        matches!(
            error.raw_os_error(),
            Some(code)
                if code == rustix::io::Errno::AGAIN.raw_os_error()
                    || code == rustix::io::Errno::ACCESS.raw_os_error()
                    || code == rustix::io::Errno::DEADLK.raw_os_error()
        )
    }
    #[cfg(windows)]
    {
        // ERROR_LOCK_VIOLATION / ERROR_IO_PENDING are what LockFileEx reports.
        matches!(error.raw_os_error(), Some(33 | 997))
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

fn write_holder(file: &mut File, path: &Path, holder: &LockHolder) -> Result<(), KernelError> {
    let record = serde_json::to_vec(holder).map_err(|error| KernelError::LockHolderMalformed {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;
    file.set_len(0)
        .map_err(|source| KernelError::io("truncate lock record", path, source))?;
    file.seek(SeekFrom::Start(0))
        .map_err(|source| KernelError::io("seek lock record", path, source))?;
    file.write_all(&record)
        .map_err(|source| KernelError::io("write lock record", path, source))?;
    file.sync_all()
        .map_err(|source| KernelError::io("sync lock record", path, source))?;
    Ok(())
}

fn ensure_lock_parent(path: &Path) -> Result<(), KernelError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    ensure_directory(parent)
}

fn ensure_directory(directory: &Path) -> Result<(), KernelError> {
    match std::fs::create_dir(directory) {
        Ok(()) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700));
            }
            Ok(())
        }
        Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
            if directory.is_dir() {
                Ok(())
            } else {
                Err(KernelError::io("create lock directory", directory, source))
            }
        }
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            let grandparent = directory
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            if grandparent == directory {
                return Err(KernelError::io("create lock directory", directory, source));
            }
            ensure_directory(grandparent)?;
            match std::fs::create_dir(directory) {
                Ok(()) => {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let _ = std::fs::set_permissions(
                            directory,
                            std::fs::Permissions::from_mode(0o700),
                        );
                    }
                    Ok(())
                }
                Err(retry)
                    if retry.kind() == io::ErrorKind::AlreadyExists && directory.is_dir() =>
                {
                    Ok(())
                }
                Err(retry) => Err(KernelError::io("create lock directory", directory, retry)),
            }
        }
        Err(source) => Err(KernelError::io("create lock directory", directory, source)),
    }
}

fn host_name() -> String {
    if let Some(host) = std::env::var_os("HOSTNAME").and_then(|value| value.into_string().ok())
        && !host.trim().is_empty()
    {
        return host.trim().to_owned();
    }
    if let Some(host) = std::env::var_os("COMPUTERNAME").and_then(|value| value.into_string().ok())
        && !host.trim().is_empty()
    {
        return host.trim().to_owned();
    }
    #[cfg(unix)]
    {
        if let Ok(bytes) = crate::read_bounded(Path::new("/etc/hostname"), 4096)
            && let Ok(text) = String::from_utf8(bytes)
            && !text.trim().is_empty()
        {
            return text.trim().to_owned();
        }
    }
    "unknown".to_owned()
}

#[cfg(all(test, unix))]
mod process_ownership_tests {
    use super::*;

    #[test]
    fn owner_drop_releases_even_when_the_open_file_description_is_duplicated() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("duplicated.lock");
        let guard = FileLock::acquire(&path, Duration::ZERO).unwrap();
        // A fork-before-exec can duplicate the same open file description.
        // Simulate that descriptor lifetime deterministically without forking.
        let duplicate = guard.holding.file.try_clone().unwrap();
        drop(guard);
        let next = FileLock::try_acquire(&path)
            .unwrap()
            .expect("the owning guard must release independently of duplicate descriptors");
        drop(duplicate);
        drop(next);
    }

    #[test]
    fn inherited_process_ownership_cannot_reenter_or_truncate() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("parent.lock");
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        file.write_all(b"parent holder must survive").unwrap();
        file.try_lock_exclusive().unwrap();
        let key = canonical_lock_key(&path).unwrap();
        let holding = Arc::new(Holding {
            file,
            holder: LockHolder {
                schema_version: LOCK_HOLDER_SCHEMA_VERSION,
                pid: std::process::id().wrapping_add(1),
                started_at: "test".to_owned(),
                host: "test".to_owned(),
                version: "test".to_owned(),
            },
            depth: AtomicUsize::new(1),
        });
        HELD.with(|held| {
            held.borrow_mut()
                .insert(key.clone(), Arc::downgrade(&holding));
        });
        assert!(matches!(
            FileLock::acquire(&path, Duration::ZERO),
            Err(KernelError::LockTimeout { .. })
        ));
        assert!(!HELD.with(|held| held.borrow().contains_key(&key)));
        drop(holding);
        assert_eq!(std::fs::read(&path).unwrap(), b"parent holder must survive");
    }
}
