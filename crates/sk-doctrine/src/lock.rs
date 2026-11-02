//! RAII exclusive file lock used by every governance mutation.
//!
//! The primitive itself is `sk_kernel::FileLock` — one OS-lock authority shared
//! with the event log and status writers. This module only gives it the
//! doctrine crate's typed error and the conventional sidecar path used by the
//! YAML writers, so activation and ledger writes cannot invent their own lock.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::DoctrineError;

/// The default bounded wait for a mutation lock.
pub const DEFAULT_LOCK_TIMEOUT: Duration = Duration::from_secs(10);

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
}
