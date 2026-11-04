//! Canonical read-only path observation and path-shape authority.
//!
//! One module answers two related questions without becoming a second
//! validator:
//!
//! * the **native planner** asks whether a requested path is safe to stage:
//!   normalisation, `.worktrees/` policy, and the directory/gitlink shape rule
//!   (`native_path_shape_refusal`). Its order and error values are exactly the
//!   ones `safe_commit` has always produced.
//! * the **source-compatible Op target projection** asks the pinned reference's
//!   own pre-guard question (`source_preflight_path`): after source path
//!   normalisation, would the reference raise a `SafeCommitError` at step 6a or
//!   6b before it ever reads protection?
//!
//! Both call the same filesystem observation (`observe_entry`) and the same
//! index gitlink probe (`index_entry_mode`), so the two projections cannot
//! drift in the facts they read. They deliberately keep different *policies*:
//! the source leaves a relative path lexically untouched, treats an unstattable
//! path as "not a directory", and folds a failed gitlink probe into the
//! directory refusal; the native planner keeps its stricter typed errors. The
//! Op layer must not relabel a native-only refusal as a source refusal.
//!
//! Everything here is read-only: `symlink_metadata`, `metadata`, and the
//! `git ls-files --stage` index read. Nothing is created, staged, or written.

use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use crate::error::GitError;
use crate::path::GitPath;
use crate::repository::GitRepository;
use crate::safe_commit::WORKTREES_DIR;

/// Canonicalise the worktree root for path resolution.
pub(crate) fn canonical_worktree(worktree_root: &Path) -> Result<PathBuf, GitError> {
    fs::canonicalize(worktree_root).map_err(|source| GitError::Io {
        context: "canonicalise the safe-commit worktree",
        source,
    })
}

/// Resolve an absolute path, following symlinked parent directories and
/// refusing a loop.
pub(crate) fn resolve_absolute_path(root: &Path, supplied: &Path) -> Result<PathBuf, GitError> {
    let Some(name) = supplied.file_name() else {
        return Err(GitError::SafeCommitPathOutsideWorktree {
            path: supplied.to_path_buf(),
            worktree_root: root.to_path_buf(),
        });
    };
    let parent = match supplied.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => root,
    };
    let resolved_parent = canonicalize_nearest_existing(root, parent, supplied)?;
    Ok(resolved_parent.join(name))
}

/// Canonicalise the nearest existing ancestor of `path` and re-append the
/// missing tail lexically, so a deletion request under a removed directory still
/// resolves.
fn canonicalize_nearest_existing(
    root: &Path,
    path: &Path,
    supplied: &Path,
) -> Result<PathBuf, GitError> {
    let mut missing: Vec<std::ffi::OsString> = Vec::new();
    let mut current = path.to_path_buf();
    loop {
        match fs::canonicalize(&current) {
            Ok(mut resolved) => {
                for name in missing.iter().rev() {
                    resolved.push(name);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                let Some(name) = current.file_name() else {
                    return Err(GitError::SafeCommitPathOutsideWorktree {
                        path: supplied.to_path_buf(),
                        worktree_root: root.to_path_buf(),
                    });
                };
                missing.push(name.to_os_string());
                match current.parent() {
                    Some(parent) if parent != current => current = parent.to_path_buf(),
                    _ => {
                        return Err(GitError::Io {
                            context: "resolve a safe-commit path",
                            source: error,
                        });
                    }
                }
            }
            Err(error) if is_symlink_loop(&error) => {
                return Err(GitError::SafeCommitPathLoop {
                    path: supplied.to_path_buf(),
                    worktree_root: root.to_path_buf(),
                });
            }
            Err(error) => {
                return Err(GitError::Io {
                    context: "resolve a safe-commit path",
                    source: error,
                });
            }
        }
    }
}

/// Refuse a path whose resolution loops. `symlink_metadata` does not follow
/// the final component, so the follow-up `metadata` is what detects a looped
/// link.
pub(crate) fn refuse_symlink_loop(root: &Path, path: &GitPath) -> Result<(), GitError> {
    match fs::metadata(root.join(path.to_path_buf())) {
        Ok(_) => Ok(()),
        Err(error) if is_symlink_loop(&error) => Err(GitError::SafeCommitPathLoop {
            path: path.to_path_buf(),
            worktree_root: root.to_path_buf(),
        }),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(GitError::Io {
            context: "inspect a safe-commit path",
            source: error,
        }),
    }
}

#[cfg(unix)]
pub(crate) fn path_to_git_bytes(relative: &Path) -> Result<Vec<u8>, GitError> {
    use std::os::unix::ffi::OsStrExt;
    Ok(relative.as_os_str().as_bytes().to_vec())
}

#[cfg(not(unix))]
pub(crate) fn path_to_git_bytes(relative: &Path) -> Result<Vec<u8>, GitError> {
    match relative.to_str() {
        Some(text) => Ok(text.replace('\\', "/").into_bytes()),
        None => Err(GitError::NonUtf8 {
            field: "safe-commit path",
        }),
    }
}

/// Whether an I/O error is the operating system's symlink-loop refusal.
///
/// Stable Rust does not expose `ErrorKind::FilesystemLoop` on this toolchain,
/// so the platform `ELOOP`/`ERROR_CANT_RESOLVE_FILENAME` value is compared. The
/// comparison is only used to choose a typed refusal; a platform without a
/// known value reports the generic I/O error instead of guessing.
pub(crate) fn is_symlink_loop(error: &std::io::Error) -> bool {
    #[cfg(target_os = "linux")]
    {
        error.raw_os_error() == Some(40)
    }
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    {
        error.raw_os_error() == Some(62)
    }
    #[cfg(windows)]
    {
        error.raw_os_error() == Some(1921)
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly",
        windows
    )))]
    {
        let _ = error;
        false
    }
}

/// Normalise, de-duplicate (order-preserving), and shape-check one path list.
pub(crate) fn normalize_paths(
    root: &Path,
    requested: &[GitPath],
) -> Result<Vec<GitPath>, GitError> {
    let mut normalized: Vec<GitPath> = Vec::with_capacity(requested.len());
    for path in requested {
        let path = crate::safe_commit::normalize_safe_commit_path(root, path.to_path_buf())?;
        if !normalized.contains(&path) {
            normalized.push(path);
        }
    }
    Ok(normalized)
}

/// Whether the first path component is the coordination-worktree directory.
///
/// This is the source's `Path(normalized).parts[0] == ".worktrees"` test: an
/// absolute path's first part is its root, and `.` components are skipped.
pub(crate) fn first_component_is_worktrees(path: &Path) -> bool {
    matches!(
        path.components().next(),
        Some(Component::Normal(name)) if name == OsStr::new(WORKTREES_DIR)
    )
}

/// Refuse a requested path under `.worktrees/`.
pub(crate) fn validate_path_policy(root: &Path, path: &GitPath) -> Result<(), GitError> {
    if first_component_is_worktrees(&path.to_path_buf()) {
        return Err(GitError::SafeCommitPathPolicy {
            path: root.join(path.to_path_buf()),
            worktree_root: root.to_path_buf(),
        });
    }
    Ok(())
}

/// What one read-only filesystem observation found.
pub(crate) enum PathEntry {
    /// A symbolic link (a directory symlink is one `120000` entry).
    Symlink,
    /// A real directory.
    Directory,
    /// A regular file or another non-directory entry.
    Regular,
    /// The path does not exist.
    Missing,
    /// Resolution looped (`ELOOP`).
    Loop,
    /// The entry could not be inspected for another reason.
    Unreadable(std::io::Error),
}

/// Observe one path without following its final component.
pub(crate) fn observe_entry(path: &Path) -> PathEntry {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => PathEntry::Symlink,
        Ok(metadata) if metadata.is_dir() => PathEntry::Directory,
        Ok(_) => PathEntry::Regular,
        Err(error) if error.kind() == ErrorKind::NotFound => PathEntry::Missing,
        Err(error) if is_symlink_loop(&error) => PathEntry::Loop,
        Err(error) => PathEntry::Unreadable(error),
    }
}

/// The index mode of exactly `path`, or `None` when it is not tracked.
pub(crate) fn index_entry_mode(
    repository: &GitRepository,
    path: &GitPath,
) -> Result<Option<String>, GitError> {
    let output = repository.process().checked(&[
        OsStr::new("--literal-pathspecs"),
        OsStr::new("ls-files"),
        OsStr::new("--full-name"),
        OsStr::new("--stage"),
        OsStr::new("-z"),
        OsStr::new("--"),
        path.as_os_str(),
    ])?;
    for record in output.stdout.split(|byte| *byte == 0) {
        if record.is_empty() {
            continue;
        }
        let Some(tab) = record.iter().position(|byte| *byte == b'\t') else {
            continue;
        };
        let listed = GitPath::from_git_bytes(&record[tab + 1..])?;
        if &listed != path {
            continue;
        }
        let mode = record[..tab]
            .split(|byte| *byte == b' ')
            .next()
            .map(|mode| String::from_utf8_lossy(mode).into_owned());
        return Ok(mode);
    }
    Ok(None)
}

/// The native planner's directory/gitlink shape check (6b equivalent).
pub(crate) fn native_path_shape_refusal(
    repository: &GitRepository,
    path: &GitPath,
) -> Result<(), GitError> {
    let root = repository.root();
    let absolute = root.join(path.to_path_buf());
    match observe_entry(&absolute) {
        PathEntry::Symlink | PathEntry::Regular | PathEntry::Missing => Ok(()),
        PathEntry::Directory => {
            if index_entry_mode(repository, path)? == Some("160000".to_string()) {
                Ok(())
            } else {
                Err(GitError::SafeCommitDirectoryArgument {
                    path: absolute,
                    worktree_root: root.to_path_buf(),
                })
            }
        }
        PathEntry::Loop => Err(GitError::SafeCommitPathLoop {
            path: absolute,
            worktree_root: root.to_path_buf(),
        }),
        PathEntry::Unreadable(source) => Err(GitError::Io {
            context: "inspect a safe-commit path",
            source,
        }),
    }
}

/// Why the source-compatible path projection refused a requested path.
#[derive(Debug)]
pub enum SourcePathRefusal {
    /// The reference preflight raises a `SafeCommitError` before 6c (6a, 6b, or
    /// an absolute path resolution loop). Admission admits because the source
    /// classifies it as a safe-commit error; the close refuses.
    Source {
        /// The original refusal value.
        error: GitError,
    },
    /// The reference would raise a non-`SafeCommitError` while normalising an
    /// absolute path; that is an undecidable probe, not an admission.
    Unreadable {
        /// The original probe/IO value.
        error: GitError,
    },
}

impl fmt::Display for SourcePathRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source { error } | Self::Unreadable { error } => error.fmt(formatter),
        }
    }
}

impl std::error::Error for SourcePathRefusal {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Source { error } | Self::Unreadable { error } => Some(error),
        }
    }
}

/// The exact normalized path accepted by the source preflight. Publication
/// consumes this spelling, rather than reusing a possibly symlinked alias.
#[derive(Debug, Clone)]
pub struct SourceCommitPath {
    normalized: GitPath,
}
impl SourceCommitPath {
    /// The path spelling the source returns for staging (possibly absolute
    /// when it lies outside the worktree).
    pub fn normalized(&self) -> &GitPath {
        &self.normalized
    }
}

/// Would the pinned reference's preflight refuse this path at 6a/6b?
///
/// The reference joins an absolute path after resolving its parents (keeping
/// the final component) and passes an outside-worktree result as-is; a relative
/// path is left lexically untouched. Its 6a test uses the first path part, and
/// its 6b test treats a symlink, a non-directory, a missing entry, and an
/// unstattable entry as non-directories, folds a failed gitlink probe into a
/// directory refusal, and only refuses a real directory that is not a gitlink.
///
/// Read-only: no index, record, ref, or worktree is created or changed.
pub fn source_preflight_path(
    repository: &GitRepository,
    path: &GitPath,
) -> Result<(), SourcePathRefusal> {
    plan_source_commit_path(repository, path).map(|_| ())
}

/// Run the same source path proof and retain its exact staging spelling.
/// The compatibility bool-like wrapper delegates here; no second observation
/// or validator is used to recover the normalized path.
pub fn plan_source_commit_path(
    repository: &GitRepository,
    path: &GitPath,
) -> Result<SourceCommitPath, SourcePathRefusal> {
    let root = canonical_worktree(repository.root())
        .map_err(|error| SourcePathRefusal::Unreadable { error })?;
    let normalized = source_normalize_path(&root, path)?;
    if first_component_is_worktrees(&normalized) {
        return Err(SourcePathRefusal::Source {
            error: GitError::SafeCommitPathPolicy {
                path: root.join(&normalized),
                worktree_root: root,
            },
        });
    }
    let candidate = root.join(&normalized);
    if let PathEntry::Directory = observe_entry(&candidate) {
        let gitlink = source_probe_path(&normalized, path)
            .map(|probe| {
                index_entry_mode(repository, &probe)
                    .ok()
                    .flatten()
                    .as_deref()
                    == Some("160000")
            })
            .unwrap_or(false);
        if !gitlink {
            return Err(SourcePathRefusal::Source {
                error: GitError::SafeCommitDirectoryArgument {
                    path: candidate,
                    worktree_root: root,
                },
            });
        }
    }
    let bytes =
        path_to_git_bytes(&normalized).map_err(|error| SourcePathRefusal::Unreadable { error })?;
    let normalized =
        GitPath::from_git_bytes(&bytes).map_err(|error| SourcePathRefusal::Unreadable { error })?;
    Ok(SourceCommitPath { normalized })
}

/// The source's absolute-path normalisation: resolve parents, keep the final
/// component, and leave an outside-worktree result as the supplied absolute
/// path. A loop is the source's typed path refusal.
fn source_normalize_path(root: &Path, path: &GitPath) -> Result<PathBuf, SourcePathRefusal> {
    let supplied = path.to_path_buf();
    if !supplied.is_absolute() {
        return Ok(supplied);
    }
    let resolved = source_resolve_commit_path(root, &supplied)?;
    Ok(resolved
        .strip_prefix(root)
        .map(Path::to_path_buf)
        .unwrap_or(supplied))
}

/// Source kernel `resolve_commit_path`: use the shared Python-compatible
/// resolver for parents, retain the leaf, and reject a looping leaf by stat.
/// The native planner keeps its older stricter normalization unchanged.
fn source_resolve_commit_path(root: &Path, supplied: &Path) -> Result<PathBuf, SourcePathRefusal> {
    let resolve = |path: &Path| {
        sk_kernel::resolve_reference_path(path).map_err(|error| match error {
            sk_kernel::ReferencePathError::SymlinkLoop { .. } => SourcePathRefusal::Source {
                error: GitError::SafeCommitPathLoop {
                    path: supplied.to_path_buf(),
                    worktree_root: root.to_path_buf(),
                },
            },
            sk_kernel::ReferencePathError::Io { source, .. } => SourcePathRefusal::Unreadable {
                error: GitError::Io {
                    context: "resolve a source commit path",
                    source,
                },
            },
            sk_kernel::ReferencePathError::InteriorNul { .. } => SourcePathRefusal::Unreadable {
                error: GitError::InvalidPlan {
                    detail: "a source commit path contains an embedded NUL byte".to_owned(),
                },
            },
        })
    };
    let Some(name) = supplied.file_name() else {
        return resolve(supplied);
    };
    let parent = resolve(supplied.parent().unwrap_or(root))?;
    let leaf = parent.join(name);
    if let Err(error) = fs::metadata(&leaf)
        && is_symlink_loop(&error)
    {
        return Err(SourcePathRefusal::Source {
            error: GitError::SafeCommitPathLoop {
                path: supplied.to_path_buf(),
                worktree_root: root.to_path_buf(),
            },
        });
    }
    Ok(leaf)
}

/// The index pathspec the source would probe for the gitlink exemption.
fn source_probe_path(normalized: &Path, original: &GitPath) -> Option<GitPath> {
    if normalized.is_absolute() {
        return Some(original.clone());
    }
    path_to_git_bytes(normalized)
        .ok()
        .and_then(|bytes| GitPath::from_git_bytes(&bytes).ok())
}
