use std::io;
use std::path::PathBuf;
use std::time::Duration;

use thiserror::Error;

/// Typed failures returned by the kernel primitives.
///
/// Every fallible operation in this crate reports a variant of this enum; no
/// production path panics or returns an untyped error.
#[derive(Debug, Error)]
pub enum KernelError {
    /// No project root (`.git` checkout or `.kittify` marker) was found.
    #[error("no Spec Kitty project root found from {start}")]
    ProjectRootNotFound { start: PathBuf },

    /// A root that must exist is missing or is not a directory.
    #[error("root {root} does not exist or is not a directory")]
    RootNotDirectory { root: PathBuf },

    /// The supplied relative path is absolute.
    #[error("path {path} must be relative")]
    AbsoluteRelativePath { path: PathBuf },

    /// The supplied relative path contains a `..` component.
    #[error("path {path} must not contain '..' components")]
    ParentTraversal { path: PathBuf },

    /// The resolved candidate escaped the resolved root.
    #[error("path {resolved} resolves outside root {root}")]
    PathEscape { root: PathBuf, resolved: PathBuf },

    /// A symlink cycle was encountered while resolving a path.
    #[error("symlink loop while resolving {path}")]
    SymlinkLoop { path: PathBuf },

    /// A final-component symlink was refused by a no-follow operation.
    #[error("path {path} is a symbolic link; refusing to follow it")]
    NoFollow { path: PathBuf },

    /// A filesystem operation failed.
    #[error("failed to {action} {path}: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// Replacement is visible, but directory durability could not be confirmed.
    /// Callers must inspect/recover rather than assuming no mutation occurred.
    #[error("replacement of {path} committed but durability is indeterminate: {source}")]
    AtomicCommitIndeterminate {
        path: PathBuf,
        #[source]
        source: Box<KernelError>,
    },

    /// A bounded read found more bytes than the caller permitted.
    #[error("file {path} is larger than the {limit}-byte limit")]
    TooLarge { path: PathBuf, limit: u64 },

    /// A guarded read's parser rejected the content.
    #[error("failed to parse {path}: {reason}")]
    GuardedRead { path: PathBuf, reason: String },

    /// A text read or parse found bytes that are not valid UTF-8.
    #[error("file {path} is not valid UTF-8")]
    InvalidUtf8 { path: PathBuf },

    /// A timestamp could not be parsed or constructed in the canonical form.
    #[error("invalid timestamp {value:?}: {reason}")]
    InvalidTimestamp { value: String, reason: String },

    /// A mission slug did not satisfy the canonical safe-segment grammar.
    #[error(
        "invalid mission slug {value:?}: expected ASCII alphanumerics, '..' guard, and no path separators"
    )]
    InvalidMissionSlug { value: String },

    /// A work-package id did not have the canonical `WP<digits>` form.
    #[error("invalid work package id {value:?}: expected WP<digits> (for example WP01)")]
    InvalidWpId { value: String },

    /// An anchored lock-path digest length is outside the supported range.
    #[error("anchored lock digest length {length} is outside 1..=64")]
    InvalidDigestLength { length: usize },

    /// A blocking lock acquire exceeded its bounded wait.
    #[error("could not acquire file lock {path} within {timeout:?}")]
    LockTimeout { path: PathBuf, timeout: Duration },

    /// A non-blocking lock attempt found another holder.
    #[error("file lock {path} is held by another holder (non-blocking attempt)")]
    LockNotAcquired { path: PathBuf },

    /// A lock file's holder record could not be decoded.
    #[error("lock holder record at {path} is malformed: {reason}")]
    LockHolderMalformed { path: PathBuf, reason: String },

    /// The holder record is inside a mandatory-locked range this platform will
    /// not let another process read. Diagnostics must degrade, not claim there
    /// is no holder.
    #[error("lock holder record at {path} is unreadable while held: {reason}")]
    LockHolderUnreadable { path: PathBuf, reason: String },

    /// A lock-name prefix/suffix tried to escape the lock directory.
    #[error("lock name fragment {fragment:?} is not a plain file-name fragment")]
    UnsafeLockName { fragment: String },

    /// A lock path could not be anchored because the repository topology was
    /// unreadable.
    #[error("cannot place the lock for {path}: {reason}")]
    LockUnavailable { path: PathBuf, reason: String },

    /// An event-log write unit was empty or a line contained a terminator.
    #[error("invalid event-log append unit: {reason}")]
    InvalidLogLine { reason: String },

    /// An event log's final line is not newline-terminated.
    #[error("event log {path} has a torn final line (no trailing newline); the append was refused")]
    TornTail { path: PathBuf },

    /// A write was shorter than the requested unit and was rolled back.
    #[error("short write to {path}: wrote {written} of {expected} bytes; append rolled back")]
    ShortWrite {
        path: PathBuf,
        expected: usize,
        written: usize,
    },

    /// Readback after an append did not reproduce the written bytes.
    #[error("readback mismatch for {path}: appended bytes were rolled back to the previous length")]
    ReadbackMismatch { path: PathBuf },

    /// An append failed and its rollback to the pre-append length also failed.
    #[error("append to {path} failed and rollback also failed: {reason}")]
    AppendRollbackFailed { path: PathBuf, reason: String },

    /// The probed path is not inside a Git repository (or is the repository's
    /// own Git directory, which is never a valid project root).
    #[error("{path} is not inside a Git repository")]
    NotAGitRepository { path: PathBuf },

    /// Git topology could not be determined from the filesystem (for example a
    /// malformed `.git` pointer or an unreadable entry).
    #[error("Git topology probe failed for {path}: {detail}")]
    GitTopologyUnavailable { path: PathBuf, detail: String },

    /// A named status-family lock could not be acquired in budget. `holder` is
    /// the rendered `.holder` sidecar when one was readable.
    #[error("timed out acquiring status lock {path} after {timeout}: {holder}")]
    StatusLockTimeout {
        path: PathBuf,
        timeout: String,
        holder: String,
    },

    /// The fixed checkout-claim-before-Mission lock order was violated.
    #[error(
        "checkout claim lock must be taken before any Mission lock (this thread already holds: {held})"
    )]
    LockOrderViolation { held: String },

    /// The user's home directory could not be determined, so a per-user path
    /// cannot be resolved.
    #[error("cannot determine the user home directory")]
    HomeDirectoryUnavailable,

    /// A built-in pack root or mission-assets root could not be resolved.
    #[error("cannot resolve {what}: {reason}")]
    PackRootNotFound { what: String, reason: String },
}

impl KernelError {
    /// Whether a failed operation may already have committed visible data.
    pub fn mutation_may_have_committed(&self) -> bool {
        matches!(
            self,
            Self::AtomicCommitIndeterminate { .. } | Self::AppendRollbackFailed { .. }
        )
    }

    pub(crate) fn io(action: &'static str, path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            action,
            path: path.into(),
            source,
        }
    }
}
