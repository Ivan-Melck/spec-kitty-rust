//! Non-strict path resolution used by `normalise_ref`.
//!
//! `writer.normalise_ref` calls `resolve_rejecting_loops`, i.e. Python's
//! non-strict `Path.resolve()`, and returns the ref verbatim when resolution
//! raises. The kernel keeps its equivalent resolver private to
//! `sk-kernel::fs_resolve`, so this crate carries the minimal form it needs:
//! expand every existing symlink, keep a missing tail lexically, and fail with
//! [`ResolveError::SymlinkLoop`] after the host's 40-link budget.
//!
//! This is a value-level path decision with no locking and no mutation. It is
//! **not** the lock/append authority (that stays with `sk-kernel`), and the
//! containment check used by `InvocationWriter::invocation_path` keeps using
//! `sk_kernel::resolve_within`. If the kernel exports its resolver later,
//! delete this module in favour of it.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

/// Mirrors the host symlink-resolution ceiling (Linux `MAXSYMLINKS`).
const MAX_SYMLINKS: usize = 40;

/// Why a non-strict resolution failed.
#[derive(Debug)]
pub enum ResolveError {
    /// The path could not be inspected (missing permission, interior NUL, …).
    Io(io::Error),
    /// More than [`MAX_SYMLINKS`] links were followed.
    SymlinkLoop,
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::SymlinkLoop => formatter.write_str("symlink loop while resolving a path"),
        }
    }
}

impl From<io::Error> for ResolveError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Resolve `path` to an absolute path with every existing symlink expanded.
///
/// Missing tail components are appended lexically, exactly like Python's
/// non-strict `Path.resolve()`. A dangling symlink is expanded to its target,
/// which is what makes callers classify a broken link by where it points.
pub fn resolve_non_strict(path: &Path) -> Result<PathBuf, ResolveError> {
    // Deliberately not `std::path::absolute`: that normalises `.`/`..`
    // lexically, while Python applies `..` *after* expanding the symlink it
    // follows. Joining keeps the `..` components for the walk below.
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut resolved = PathBuf::new();
    let mut pending: VecDeque<OsString> = VecDeque::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(prefix) => resolved.push(prefix.as_os_str()),
            Component::RootDir => resolved.push(std::path::MAIN_SEPARATOR_STR),
            Component::CurDir => {}
            Component::ParentDir => pending.push_back(OsString::from("..")),
            Component::Normal(name) => pending.push_back(name.to_os_string()),
        }
    }

    let mut followed = 0_usize;
    while let Some(token) = pending.pop_front() {
        if token == ".." {
            resolved.pop();
            continue;
        }
        let candidate = resolved.join(&token);
        match fs::symlink_metadata(&candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                followed += 1;
                if followed > MAX_SYMLINKS {
                    return Err(ResolveError::SymlinkLoop);
                }
                let target = fs::read_link(&candidate)?;
                prepend_target(&target, &mut resolved, &mut pending);
            }
            _ => resolved.push(&token),
        }
    }
    Ok(resolved)
}

fn prepend_target(target: &Path, resolved: &mut PathBuf, pending: &mut VecDeque<OsString>) {
    let mut front: Vec<OsString> = Vec::new();
    for component in target.components() {
        match component {
            Component::Prefix(prefix) => {
                resolved.clear();
                resolved.push(prefix.as_os_str());
            }
            Component::RootDir => {
                resolved.clear();
                resolved.push(std::path::MAIN_SEPARATOR_STR);
            }
            Component::CurDir => {}
            Component::ParentDir => front.push(OsString::from("..")),
            Component::Normal(name) => front.push(name.to_os_string()),
        }
    }
    for token in front.into_iter().rev() {
        pending.push_front(token);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_tail_is_appended_lexically() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("no").join("such").join("file");
        assert_eq!(resolve_non_strict(&missing).unwrap(), missing);
    }

    #[cfg(unix)]
    #[test]
    fn parent_components_are_applied_after_expansion() {
        // `..` after a symlink means the parent of the *target* (Python's
        // non-strict `resolve()`), not a lexical pop of the link's directory.
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        fs::create_dir_all(&target).unwrap();
        let link = temp.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        // "<temp>/link/.." must resolve to the target's parent, not `<temp>`.
        assert_eq!(
            resolve_non_strict(&link.join("..")).unwrap(),
            temp.path().canonicalize().unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn dangling_link_resolves_to_its_target() {
        let temp = tempfile::tempdir().unwrap();
        let outside = temp.path().join("outside.jsonl");
        let link = temp.path().join("link");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        assert_eq!(
            resolve_non_strict(&link).unwrap(),
            temp.path().canonicalize().unwrap().join("outside.jsonl")
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_loop_fails() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("a");
        let second = temp.path().join("b");
        std::os::unix::fs::symlink(&second, &first).unwrap();
        std::os::unix::fs::symlink(&first, &second).unwrap();
        assert!(matches!(
            resolve_non_strict(&first),
            Err(ResolveError::SymlinkLoop)
        ));
    }
}
