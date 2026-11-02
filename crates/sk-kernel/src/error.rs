use std::io;
use std::path::PathBuf;

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

    /// A filesystem operation failed.
    #[error("failed to {action} {path}: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// A bounded read found more bytes than the caller permitted.
    #[error("file {path} is larger than the {limit}-byte limit")]
    TooLarge { path: PathBuf, limit: u64 },

    /// A mission slug did not satisfy the canonical safe-segment grammar.
    #[error(
        "invalid mission slug {value:?}: expected ASCII alphanumerics, '..' guard, and no path separators"
    )]
    InvalidMissionSlug { value: String },

    /// A work-package id did not have the canonical `WP<digits>` form.
    #[error("invalid work package id {value:?}: expected WP<digits> (for example WP01)")]
    InvalidWpId { value: String },
}

impl KernelError {
    pub(crate) fn io(action: &'static str, path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            action,
            path: path.into(),
            source,
        }
    }
}
