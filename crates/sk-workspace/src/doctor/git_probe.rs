//! Read-only Git questions the diagnostics need, answered through `sk-git`.
//!
//! Nothing here starts a Git process itself: [`sk_git::GitRepository`] owns the
//! bounded process seam. This module only translates "is there a usable work
//! tree?" into the tri-state the audits need, so a *broken* repository is never
//! confused with *no* repository.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use sk_git::{GitError, GitRepository};

/// Whether a usable Git work tree answers this question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GitAnswer<T> {
    /// A usable repository answered with `T`.
    Answered(T),
    /// There is no usable work tree here (absent repository, bare repository,
    /// or git unavailable), so file existence decides.
    NoRepository,
    /// A repository exists but the question could not be answered.
    Unreadable(String),
}

/// Open the repository at `root`, distinguishing "absent" from "broken".
pub(crate) fn open(root: &Path) -> GitAnswer<GitRepository> {
    match GitRepository::open(root) {
        Ok(repository) => GitAnswer::Answered(repository),
        Err(
            GitError::NotARepository { .. }
            | GitError::BareRepository { .. }
            | GitError::GitUnavailable { .. },
        ) => GitAnswer::NoRepository,
        Err(error) => GitAnswer::Unreadable(error.to_string()),
    }
}

/// The repository-relative paths git has in its index under `pathspec`.
///
/// The index is the authority: a `git add`-ed but never-committed file is
/// tracked. A broken index is [`GitAnswer::Unreadable`], never an empty set.
pub(crate) fn tracked_paths(root: &Path, pathspec: &str) -> GitAnswer<BTreeSet<PathBuf>> {
    let repository = match open(root) {
        GitAnswer::Answered(repository) => repository,
        GitAnswer::NoRepository => return GitAnswer::NoRepository,
        GitAnswer::Unreadable(detail) => return GitAnswer::Unreadable(detail),
    };
    match repository.index_entries(&[OsStr::new(pathspec)]) {
        Ok(entries) => GitAnswer::Answered(
            entries
                .into_iter()
                .map(|entry| entry.path().to_path_buf())
                .collect(),
        ),
        Err(error) => GitAnswer::Unreadable(error.to_string()),
    }
}

/// `path` relative to `repo_root`, or `None` when it is not below it.
///
/// Both sides are canonicalized first where that succeeds, so a symlinked
/// checkout spelling still compares against git's absolute paths.
pub(crate) fn relative_to(repo_root: &Path, path: &Path) -> Option<PathBuf> {
    let base = std::fs::canonicalize(repo_root).unwrap_or_else(|_| repo_root.to_path_buf());
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    target.strip_prefix(&base).ok().map(Path::to_path_buf)
}

/// The sorted, `stat`-fail-soft directory children of `<root>/kitty-specs/`.
///
/// Mirrors the upstream read-only survey posture: an absent or unreadable
/// `kitty-specs/` is an empty walk, and an entry that cannot be examined is
/// skipped like a non-directory rather than crashing the whole audit.
pub(crate) fn kitty_specs_children(root: &Path) -> Vec<PathBuf> {
    let specs_dir = root.join(SPECS_DIR_NAME);
    let entries = match std::fs::read_dir(&specs_dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    let mut children = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        match std::fs::metadata(&path) {
            Ok(metadata) if metadata.is_dir() => children.push(path),
            _ => continue,
        }
    }
    children.sort();
    children
}

/// The `kitty-specs/` directory name (one spelling for the diagnostics).
pub(crate) const SPECS_DIR_NAME: &str = "kitty-specs";

/// The 3-digit leading numeric prefix of a directory name, when it has one.
pub(crate) fn numeric_prefix(name: &str) -> Option<&str> {
    let bytes = name.as_bytes();
    if bytes.len() >= 4 && bytes[..3].iter().all(u8::is_ascii_digit) && bytes[3] == b'-' {
        Some(&name[..3])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_prefix_requires_exactly_three_digits_and_a_dash() {
        assert_eq!(numeric_prefix("083-foo"), Some("083"));
        assert_eq!(numeric_prefix("083"), None);
        assert_eq!(numeric_prefix("0834-foo"), None);
        assert_eq!(numeric_prefix("ab3-foo"), None);
        assert_eq!(numeric_prefix("foo-bar"), None);
        assert_eq!(numeric_prefix("-083-foo"), None);
    }

    #[test]
    fn absent_kitty_specs_is_an_empty_walk() {
        let root = tempfile::tempdir().unwrap();
        assert!(kitty_specs_children(root.path()).is_empty());
    }
}
