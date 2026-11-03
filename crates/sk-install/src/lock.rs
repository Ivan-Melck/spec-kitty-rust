//! Process-level serialization for init/install commits.
//!
//! Two `spec-kitty init` runs (or an init racing an upgrade) must not interleave
//! their managed-asset writes and manifest replacement. One advisory exclusive
//! lock per project serializes the commit phase; planning stays lock-free and
//! read-only.
//!
//! The lock itself is the kernel's [`sk_kernel::FileLock`] — the single lock
//! authority for the workspace — so this module only supplies the project lock
//! path and the typed error mapping. That inherits the kernel's guarantees:
//! cross-process exclusion, a bounded wait, same-thread reentrancy, a
//! no-follow open, and RAII release on drop, error and panic unwinding.

use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};
use sk_kernel::FileLock;

use crate::error::InstallError;

/// Legacy project-relative path of the install lock file.
///
/// New callers use [`project_lock_path`], which places the lock in the machine
/// state directory so an init leaves no untracked residue in the checkout. The
/// constant stays for source compatibility with callers that still pass an
/// explicit path.
pub const LOCK_REL: &str = ".kittify/.install.lock";

/// An exclusive advisory lock guarding one project's install commit.
#[derive(Debug)]
pub struct ProjectLock {
    file_lock: FileLock,
    path: PathBuf,
}

impl ProjectLock {
    /// Acquire the project lock at `path`, waiting at most `wait`.
    ///
    /// The parent directory is created by the kernel when missing. A zero wait
    /// performs one non-blocking attempt; a contended lock within the budget
    /// returns [`InstallError::LockUnavailable`].
    pub fn acquire(path: &Path, wait: Duration) -> Result<Self, InstallError> {
        match FileLock::acquire(path, wait) {
            Ok(file_lock) => Ok(Self {
                file_lock,
                path: path.to_path_buf(),
            }),
            Err(sk_kernel::KernelError::LockTimeout { .. }) => {
                // The kernel reports an expired bounded wait as `LockTimeout`.
                // Mapping it to the advertised `LockUnavailable` (not generic
                // I/O) is what lets a CLI retry instead of reporting a
                // filesystem fault.
                Err(InstallError::LockUnavailable {
                    path: path.to_path_buf(),
                    waited_ms: u64::try_from(wait.as_millis()).unwrap_or(u64::MAX),
                    source: std::io::Error::new(
                        std::io::ErrorKind::WouldBlock,
                        "project lock wait expired",
                    ),
                })
            }
            Err(sk_kernel::KernelError::LockUnavailable { .. }) => {
                Err(InstallError::LockUnavailable {
                    path: path.to_path_buf(),
                    waited_ms: u64::try_from(wait.as_millis()).unwrap_or(u64::MAX),
                    source: std::io::Error::new(
                        std::io::ErrorKind::WouldBlock,
                        "project lock path is unavailable",
                    ),
                })
            }
            Err(error @ sk_kernel::KernelError::Io { .. }) => {
                // The kernel distinguishes contention by the io error kind; a
                // contention failure is reported as an unavailable lock so the
                // caller can retry rather than treat it as a filesystem fault.
                if is_contention(&error) {
                    Err(InstallError::LockUnavailable {
                        path: path.to_path_buf(),
                        waited_ms: u64::try_from(wait.as_millis()).unwrap_or(u64::MAX),
                        source: std::io::Error::new(
                            std::io::ErrorKind::WouldBlock,
                            error.to_string(),
                        ),
                    })
                } else {
                    Err(InstallError::io("lock", path, std::io::Error::other(error)))
                }
            }
            Err(error) => Err(InstallError::io("lock", path, std::io::Error::other(error))),
        }
    }

    /// Acquire the canonical per-project install lock.
    ///
    /// The lock file lives under the machine state directory, keyed by the
    /// resolved project root, so `init` never creates `.kittify/.install.lock`
    /// (untracked residue that `git add -A` would otherwise commit) and the
    /// same project's concurrent init/install commits still exclude each other.
    pub fn for_project(project_root: &Path, wait: Duration) -> Result<Self, InstallError> {
        let path = project_lock_path(project_root)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|source| InstallError::io("create directory", parent, source))?;
        }
        Self::acquire(&path, wait)
    }

    /// The lock file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The kernel guard, for callers that need holder metadata.
    pub fn file_lock(&self) -> &FileLock {
        &self.file_lock
    }
}

/// The canonical per-project install lock path in the machine state directory.
pub fn project_lock_path(project_root: &Path) -> Result<PathBuf, InstallError> {
    let unresolved = |error: sk_kernel::KernelError| InstallError::LedgerInvalid {
        path: project_root.to_path_buf(),
        detail: format!("could not resolve install lock authority: {error}"),
    };
    let state = sk_kernel::user_state_dir().map_err(unresolved)?;
    let resolved = sk_kernel::paths::resolve_path_non_strict(project_root).map_err(unresolved)?;
    let digest = Sha256::digest(resolved.to_string_lossy().as_bytes());
    let key: String = digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(state
        .join("spec-kitty")
        .join("locks")
        .join("install")
        .join(format!("{key}.lock")))
}

fn is_contention(error: &sk_kernel::KernelError) -> bool {
    match error {
        sk_kernel::KernelError::Io { source, .. } => matches!(
            source.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::PermissionDenied
        ),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn lock_is_exclusive_then_released_on_drop() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join(LOCK_REL);
        let first = ProjectLock::acquire(&path, Duration::ZERO).expect("first lock");

        let competing = path.clone();
        let (sender, receiver) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            let result = ProjectLock::acquire(&competing, Duration::from_millis(50));
            sender.send(result.is_err()).expect("send result");
        });
        assert!(receiver.recv().expect("contended result"));
        handle.join().expect("join contender");

        drop(first);
        ProjectLock::acquire(&path, Duration::ZERO).expect("lock after release");
    }
}
