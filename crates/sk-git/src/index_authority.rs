//! The captured operator-index authority.
//!
//! Git lets a caller redirect every index read and write with
//! `GIT_INDEX_FILE`. A writer that silently replaced that index with
//! `<git-dir>/index` would stage into, lock, and reconcile a different file
//! than the operator's own commands use. It would also be a silent mutation of
//! user-visible state: the caller's staging area would not move, or an
//! unrelated index would.
//!
//! This module captures the selector **once, at repository open**, and binds
//! every later operator-index operation to that captured value:
//!
//! * the process environment is never mutated;
//! * a relative value is resolved against the discovered work-tree root, which
//!   is the directory the child git actually runs in (git resolves a relative
//!   `GIT_INDEX_FILE` against its working directory, which it moves to the
//!   top of the work tree during setup);
//! * the value is passed to child git *verbatim*, so a path that contains a
//!   symlink as its final component keeps that spelling and git keeps its own
//!   follow-through behaviour; the adapter never resolves the final component
//!   to a different target;
//! * a plan records the captured authority and refuses to apply under a
//!   different one, so a plan prepared in one index namespace can never mutate
//!   another.
//!
//! The default index (`<git-dir>/index`) stays implicit: no `GIT_INDEX_FILE` is
//! invented for a repository whose environment did not set one. Only a private
//! scoped index generated inside the checkout's own git directory overrides
//! the captured value, and only for the specific commands that build a
//! candidate tree.
//!
//! Repository-redirecting variables other than `GIT_INDEX_FILE` remain
//! explicit refusals: this module captures an index, not a shared repository
//! authority.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// Where the operator index comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OperatorIndexSource {
    /// No selector was set: git's ordinary per-worktree index.
    Default,
    /// `GIT_INDEX_FILE` was set when the repository was opened.
    Environment,
}

impl OperatorIndexSource {
    /// A stable machine-readable name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Environment => "environment",
        }
    }
}

/// The index every operator-facing read and write of one repository handle uses.
///
/// Created by [`GitRepository::open`](crate::GitRepository::open); there is no
/// public constructor, and the capture is sticky for the lifetime of the
/// handle even if the process environment changes later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorIndex {
    source: OperatorIndexSource,
    /// The value handed to child git for `GIT_INDEX_FILE`, verbatim.
    raw: Option<OsString>,
    /// Absolute path used for lock checks and diagnostics. The final component
    /// is never resolved through a symlink.
    absolute: Option<PathBuf>,
    /// The lock file git will use for this index.
    lock: Option<PathBuf>,
}

impl OperatorIndex {
    /// Capture the ambient selector. Called once, before root normalisation.
    pub(crate) fn capture(probe_root: &Path) -> Self {
        match std::env::var_os("GIT_INDEX_FILE") {
            None => Self {
                source: OperatorIndexSource::Default,
                raw: None,
                absolute: None,
                lock: None,
            },
            Some(raw) => {
                let spelling = Path::new(&raw);
                // `Path::join` with an empty component yields the base path,
                // which is exactly where git's own setup leaves an empty
                // selector: the working directory it runs in.
                let absolute = if spelling.is_absolute() {
                    spelling.to_path_buf()
                } else {
                    probe_root.join(spelling)
                };
                let lock = if raw.is_empty() {
                    // Git's lock file name is `<index>.lock`; an empty index
                    // path therefore names `.lock` in the working directory.
                    probe_root.join(".lock")
                } else {
                    let mut lock = absolute.as_os_str().to_os_string();
                    lock.push(".lock");
                    PathBuf::from(lock)
                };
                Self {
                    source: OperatorIndexSource::Environment,
                    raw: Some(raw),
                    absolute: Some(absolute),
                    lock: Some(lock),
                }
            }
        }
    }

    /// Where the index authority came from.
    pub fn source(&self) -> OperatorIndexSource {
        self.source
    }

    /// Whether an inherited `GIT_INDEX_FILE` was captured.
    pub fn is_captured(&self) -> bool {
        self.source == OperatorIndexSource::Environment
    }

    /// The captured value exactly as git must receive it, or `None` when the
    /// default index applies.
    pub fn raw_value(&self) -> Option<&OsStr> {
        self.raw.as_deref()
    }

    /// The absolute index path, or `None` for git's default index.
    ///
    /// The final path component is preserved as spelled; a symlinked index path
    /// is not resolved to its target.
    pub fn explicit_path(&self) -> Option<&Path> {
        self.absolute.as_deref()
    }

    /// The lock file for the captured index, or `None` for the default index.
    pub fn explicit_lock_path(&self) -> Option<&Path> {
        self.lock.as_deref()
    }

    /// A synthetic explicit authority, for tests that need two handles with
    /// different captured indexes in one process without mutating the process
    /// environment.
    #[cfg(test)]
    pub(crate) fn explicit_for_test(path: &Path) -> Self {
        let mut lock = path.as_os_str().to_os_string();
        lock.push(".lock");
        Self {
            source: OperatorIndexSource::Environment,
            raw: Some(path.as_os_str().to_os_string()),
            absolute: Some(path.to_path_buf()),
            lock: Some(PathBuf::from(lock)),
        }
    }
}
