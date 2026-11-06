//! RAII exclusive file locks used by governance mutations.
//!
//! Two lock families exist, and they are deliberately distinct:
//!
//! * **The charter synthesis lock** — the canonical cross-language authority
//!   shared with Python's `charter activate` / `deactivate` / `synthesize`
//!   (`charter.activation.synthesizer.synthesis_lock`). Inside a git repository
//!   it is `<git-common-dir>/spec-kitty-locks/charter-synthesis/<sha256[0..32]>.lock`
//!   keyed on the resolved worktree root; outside a git repository it is
//!   `<root>/.kittify/runtime/locks/charter-synthesis.lock`. A native write that
//!   touched the activation authority under a private sidecar lock could be
//!   interleaved with a Python run, so every activation transaction takes this
//!   one.
//! * **A conventional sidecar** (`<target>.lock`) for documents outside the
//!   activation authority that have no cross-language writer.
//!
//! The primitive itself is `sk_kernel::FileLock` — one OS-lock authority shared
//! with the event log and status writers. This module only resolves paths and
//! gives the doctrine crate its typed errors.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::DoctrineError;

/// The default bounded wait for a mutation lock.
pub const DEFAULT_LOCK_TIMEOUT: Duration = Duration::from_secs(10);

/// Directory under `<git-common-dir>/spec-kitty-locks/` holding per-worktree
/// charter-synthesis lock files.
pub const SYNTHESIS_LOCK_SUBDIRECTORY: &str = "charter-synthesis";

/// The synthesis lock path relative to the project root when there is no git
/// repository.
pub const NON_GIT_SYNTHESIS_LOCK_REL: &str = ".kittify/runtime/locks/charter-synthesis.lock";

/// An exclusive advisory lock on a sidecar lock file.
#[derive(Debug)]
pub struct FileLock {
    inner: sk_kernel::FileLock,
}

impl FileLock {
    /// Acquire the exclusive lock at `path`, waiting at most `timeout`.
    ///
    /// A timeout is a typed error, never a silent proceed without the lock.
    pub fn acquire(path: &Path, timeout: Duration) -> Result<Self, DoctrineError> {
        sk_kernel::FileLock::acquire(path, timeout)
            .map(|inner| Self { inner })
            .map_err(|error| match error {
                sk_kernel::KernelError::LockTimeout { path, timeout } => {
                    DoctrineError::LockTimeout {
                        path,
                        millis: timeout.as_millis() as u64,
                    }
                }
                other => DoctrineError::Kernel(other),
            })
    }

    /// The lock file path.
    pub fn path(&self) -> &Path {
        self.inner.path()
    }
}

/// The conventional sidecar lock path for `target`.
pub fn lock_path_for(target: &Path) -> PathBuf {
    let mut name = target
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(".lock");
    target.with_file_name(name)
}

/// Return the charter synthesis lock file for the worktree rooted at `repo_root`.
///
/// The anchor digest and the placement under the git common dir are owned by
/// the kernel ([`sk_kernel::anchored_lock_path`]); this function adds only the
/// `charter-synthesis` subdirectory, the non-git fallback and the refusal.
///
/// A git topology probe that cannot run at all is
/// [`DoctrineError::SynthesisLockUnavailable`], never a silently different
/// lock home.
pub fn synthesis_lock_path(repo_root: &Path) -> Result<PathBuf, DoctrineError> {
    let resolved = std::fs::canonicalize(repo_root).unwrap_or_else(|_| repo_root.to_path_buf());
    match sk_kernel::git_topology::git_common_dir(&resolved) {
        Ok(common_dir) => sk_kernel::anchored_lock_path(
            &resolved,
            &common_dir,
            Some(Path::new(SYNTHESIS_LOCK_SUBDIRECTORY)),
        )
        .map_err(DoctrineError::Kernel),
        Err(sk_kernel::KernelError::NotAGitRepository { .. }) => {
            Ok(resolved.join(NON_GIT_SYNTHESIS_LOCK_REL))
        }
        Err(error) => Err(DoctrineError::SynthesisLockUnavailable {
            root: repo_root.to_path_buf(),
            reason: error.to_string(),
        }),
    }
}

/// Hold the run-wide charter synthesis lock for `repo_root`.
///
/// Same-thread reentrant (the kernel guard's contract), so a transaction can
/// nest under a caller that already holds it.
pub fn acquire_synthesis_lock(
    repo_root: &Path,
    timeout: Duration,
) -> Result<FileLock, DoctrineError> {
    let path = synthesis_lock_path(repo_root)?;
    sk_kernel::FileLock::acquire(&path, timeout)
        .map(|inner| FileLock { inner })
        .map_err(|error| match error {
            sk_kernel::KernelError::LockTimeout { .. } => DoctrineError::SynthesisLockHeld {
                path,
                millis: timeout.as_millis() as u64,
            },
            other => DoctrineError::Kernel(other),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn contended_acquire_times_out_then_succeeds_after_release() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("resource.lock");
        let (held_sender, held_receiver) = mpsc::channel();
        let holder = std::thread::spawn({
            let path = path.clone();
            move || {
                let lock = FileLock::acquire(&path, Duration::from_millis(200)).unwrap();
                held_sender.send(()).unwrap();
                std::thread::sleep(Duration::from_millis(150));
                drop(lock);
            }
        });
        held_receiver.recv().unwrap();
        let error = FileLock::acquire(&path, Duration::from_millis(50)).unwrap_err();
        assert!(matches!(error, DoctrineError::LockTimeout { .. }));
        holder.join().unwrap();
        FileLock::acquire(&path, Duration::from_millis(500)).unwrap();
    }

    #[test]
    fn lock_is_released_on_drop_from_another_thread() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("resource.lock");
        let (sender, receiver) = mpsc::channel();
        let holder = std::thread::spawn({
            let path = path.clone();
            move || {
                let lock = FileLock::acquire(&path, Duration::from_millis(100)).unwrap();
                sender.send(()).unwrap();
                std::thread::sleep(Duration::from_millis(80));
                drop(lock);
            }
        });
        receiver.recv().unwrap();
        let lock = FileLock::acquire(&path, Duration::from_millis(1000)).unwrap();
        assert_eq!(lock.path(), path.as_path());
        holder.join().unwrap();
    }

    #[test]
    fn non_git_synthesis_lock_uses_the_reference_fallback_path() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let path = synthesis_lock_path(&root).unwrap();
        assert_eq!(path, root.join(NON_GIT_SYNTHESIS_LOCK_REL));
    }

    #[test]
    fn git_synthesis_lock_is_anchored_under_the_common_dir() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join(".git/refs/heads")).unwrap();
        std::fs::create_dir_all(root.join(".git/objects")).unwrap();
        std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(
            root.join(".git/config"),
            "[core]\n\trepositoryformatversion = 0\n",
        )
        .unwrap();
        let path = synthesis_lock_path(&root).unwrap();
        assert!(path.starts_with(root.join(".git/spec-kitty-locks/charter-synthesis")));
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.ends_with(".lock"));
        assert_eq!(name.len(), 32 + ".lock".len());
    }

    #[test]
    fn synthesis_lock_is_reentrant_on_one_thread_and_exclusive_across_threads() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let first = acquire_synthesis_lock(&root, Duration::from_millis(500)).unwrap();
        let second = acquire_synthesis_lock(&root, Duration::from_millis(500)).unwrap();
        drop(second);
        drop(first);
        acquire_synthesis_lock(&root, Duration::from_millis(500)).unwrap();
    }
}
