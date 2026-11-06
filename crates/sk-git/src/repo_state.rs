//! Repository state that gates a mutation: git directories, in-progress
//! operations, and plumbing-version capability.
//!
//! These are filesystem and version probes. They never write and never contact
//! a remote. A caller uses them before planning a destructive operation so a
//! rebase, merge, cherry-pick, revert, sequencer run, or held index lock is
//! refused rather than corrupted.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::GitError;
use crate::repository::GitRepository;

/// The per-worktree and shared git directories of a checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitDirs {
    git_dir: PathBuf,
    common_dir: PathBuf,
}

impl GitDirs {
    /// This worktree's own git directory.
    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    /// The shared git directory (the primary checkout's `.git` for a linked
    /// worktree).
    pub fn common_dir(&self) -> &Path {
        &self.common_dir
    }

    /// Whether this checkout is a linked worktree rather than the primary one.
    pub fn is_linked_worktree(&self) -> bool {
        self.git_dir != self.common_dir
    }
}

/// One git operation detected by an on-disk marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InProgressOperation {
    /// An interactive or merge-backend rebase (`rebase-merge`).
    RebaseMerge,
    /// An apply-backend rebase or `git am` (`rebase-apply`).
    RebaseApply,
    /// A merge (`MERGE_HEAD`).
    Merge,
    /// A cherry-pick (`CHERRY_PICK_HEAD`).
    CherryPick,
    /// A revert (`REVERT_HEAD`).
    Revert,
    /// A multi-commit sequence (`sequencer`).
    Sequencer,
    /// A held index lock (`index.lock`), i.e. another writer is active.
    IndexLock,
}

impl InProgressOperation {
    /// A stable machine-readable name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RebaseMerge => "rebase-merge",
            Self::RebaseApply => "rebase-apply",
            Self::Merge => "merge",
            Self::CherryPick => "cherry-pick",
            Self::Revert => "revert",
            Self::Sequencer => "sequencer",
            Self::IndexLock => "index-lock",
        }
    }

    /// The marker file this operation leaves in a git directory.
    fn marker(self) -> &'static str {
        match self {
            Self::RebaseMerge => "rebase-merge",
            Self::RebaseApply => "rebase-apply",
            Self::Merge => "MERGE_HEAD",
            Self::CherryPick => "CHERRY_PICK_HEAD",
            Self::Revert => "REVERT_HEAD",
            Self::Sequencer => "sequencer",
            Self::IndexLock => "index.lock",
        }
    }
}

/// The oldest git that supports `git merge-tree --write-tree`.
pub const MERGE_TREE_WRITE_TREE_MIN_VERSION: (u32, u32) = (2, 38);

impl GitRepository {
    /// Resolve this checkout's git directory and shared common directory.
    pub fn git_dirs(&self) -> Result<GitDirs, GitError> {
        let git_dir_output = self
            .process()
            .checked(&["rev-parse", "--absolute-git-dir"])?;
        let git_dir_text = crate::path::utf8_field(
            crate::path::strip_line_feed(&git_dir_output.stdout),
            "git directory",
        )?;
        let git_dir = PathBuf::from(git_dir_text);

        let common_output = self.process().checked(&["rev-parse", "--git-common-dir"])?;
        let common_text = crate::path::utf8_field(
            crate::path::strip_line_feed(&common_output.stdout),
            "git common directory",
        )?;
        let raw_common = PathBuf::from(common_text);
        let common_dir = if raw_common.is_absolute() {
            raw_common
        } else {
            self.root().join(raw_common)
        };
        let common_dir = fs::canonicalize(&common_dir).map_err(|source| GitError::Io {
            context: "canonicalise the shared git directory",
            source,
        })?;
        let git_dir = fs::canonicalize(&git_dir).map_err(|source| GitError::Io {
            context: "canonicalise the worktree git directory",
            source,
        })?;

        Ok(GitDirs {
            git_dir,
            common_dir,
        })
    }

    /// Every in-progress operation marker present for this checkout, sorted.
    ///
    /// Both the per-worktree git directory and the shared common directory are
    /// scanned, because a rebase lives in the former and some locks in the
    /// latter. The held-index lock is resolved through the repository's
    /// [operator index authority](crate::OperatorIndex): a captured
    /// `GIT_INDEX_FILE` is locked at its own `.lock`, never at
    /// `<git-dir>/index.lock`. This is a conservative filesystem probe for the
    /// operation markers: it never runs git.
    pub fn in_progress_operations(&self) -> Result<Vec<InProgressOperation>, GitError> {
        let dirs = self.git_dirs()?;
        let mut found = Vec::new();
        for operation in [
            InProgressOperation::RebaseMerge,
            InProgressOperation::RebaseApply,
            InProgressOperation::Merge,
            InProgressOperation::CherryPick,
            InProgressOperation::Revert,
            InProgressOperation::Sequencer,
        ] {
            let marker = operation.marker();
            if dirs.git_dir().join(marker).exists() || dirs.common_dir().join(marker).exists() {
                found.push(operation);
            }
        }
        if self.operator_index_lock_path()?.exists() {
            found.push(InProgressOperation::IndexLock);
        }
        Ok(found)
    }

    /// Whether any in-progress operation marker is present.
    pub fn is_operation_in_progress(&self) -> Result<bool, GitError> {
        Ok(!self.in_progress_operations()?.is_empty())
    }

    /// The installed git version as `(major, minor, patch)`.
    pub fn git_version(&self) -> Result<(u32, u32, u32), GitError> {
        let output = self.process().checked(&["--version"])?;
        let text = crate::path::utf8_field(&output.stdout, "git version")?;
        parse_version(&text)
    }

    /// Whether `git merge-tree --write-tree` is available (git >= 2.38).
    ///
    /// Fail-closed: an unparseable version is an error, not "available".
    pub fn supports_merge_tree_write_tree(&self) -> Result<bool, GitError> {
        let (major, minor, _patch) = self.git_version()?;
        Ok((major, minor) >= MERGE_TREE_WRITE_TREE_MIN_VERSION)
    }
}

fn parse_version(text: &str) -> Result<(u32, u32, u32), GitError> {
    // `git version 2.43.0`; some builds append `.windows.1` or `-rc1`.
    let token = text
        .split_whitespace()
        .find(|word| word.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .ok_or_else(|| GitError::InvalidOutput {
            detail: format!("git --version printed no version number: {text:?}"),
        })?;
    let mut parts = token.split('.');
    let major = parts
        .next()
        .and_then(|part| part.parse::<u32>().ok())
        .ok_or_else(|| GitError::InvalidOutput {
            detail: format!("git --version printed an unparseable version: {text:?}"),
        })?;
    let minor = parts
        .next()
        .and_then(|part| part.parse::<u32>().ok())
        .ok_or_else(|| GitError::InvalidOutput {
            detail: format!("git --version printed an unparseable version: {text:?}"),
        })?;
    let patch = parts
        .next()
        .map(|part| {
            part.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
        })
        .and_then(|digits| digits.parse::<u32>().ok())
        .unwrap_or(0);
    Ok((major, minor, patch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_and_suffixed_versions() {
        assert_eq!(parse_version("git version 2.43.0").unwrap(), (2, 43, 0));
        assert_eq!(
            parse_version("git version 2.43.0.windows.1").unwrap(),
            (2, 43, 0)
        );
        assert_eq!(parse_version("git version 2.38").unwrap(), (2, 38, 0));
        assert!(parse_version("no version here").is_err());
    }
}
