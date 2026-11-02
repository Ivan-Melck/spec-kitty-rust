//! Porcelain status parsing and the typed status record.
//!
//! Parsing is done only from `--porcelain=v1 -z` output. Display-oriented
//! `git status` text quotes and rewrites paths, so it is never parsed: a file
//! named `a -> b` would otherwise look like a rename and a path with a space
//! would arrive quoted.

use crate::error::GitError;
use crate::path::GitPath;
use crate::process::GitProcess;

/// Which untracked files `git status` should report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UntrackedMode {
    /// Do not list untracked files (`--untracked-files=no`).
    No,
    /// Collapse a new directory into one `?? dir/` entry (the git default).
    #[default]
    Normal,
    /// List every file inside a new directory individually.
    All,
}

impl UntrackedMode {
    pub(crate) fn as_git_arg(self) -> &'static str {
        match self {
            Self::No => "--untracked-files=no",
            Self::Normal => "--untracked-files=normal",
            Self::All => "--untracked-files=all",
        }
    }
}

/// Options for [`GitRepository::status_with`](crate::GitRepository::status_with).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StatusOptions {
    /// How untracked files are reported. Defaults to [`UntrackedMode::Normal`].
    pub untracked: UntrackedMode,
    /// Include ignored entries (`!!`) in the result. Defaults to `false`.
    pub ignored: bool,
}

/// One `git status --porcelain=v1 -z` record.
///
/// `index` and `worktree` are git's two status letters (`X` and `Y`). A space
/// means "unchanged in that area"; `??` means untracked and `!!` means ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusEntry {
    index: char,
    worktree: char,
    path: GitPath,
    original_path: Option<GitPath>,
    is_directory: bool,
}

impl StatusEntry {
    /// The staged/index status letter (`X`).
    pub fn index(&self) -> char {
        self.index
    }

    /// The unstaged/worktree status letter (`Y`).
    pub fn worktree(&self) -> char {
        self.worktree
    }

    /// The repository-relative path, with exact bytes retained.
    ///
    /// For an untracked directory that git collapsed into one entry, the
    /// trailing `/` is removed and [`StatusEntry::is_directory`] is `true`.
    pub fn path(&self) -> &GitPath {
        &self.path
    }

    /// The source path of a rename or copy, when git reported one.
    pub fn original_path(&self) -> Option<&GitPath> {
        self.original_path.as_ref()
    }

    /// Whether git marked this record as a directory (a trailing `/`).
    pub fn is_directory(&self) -> bool {
        self.is_directory
    }

    /// Whether this is an untracked entry (`??`).
    pub fn is_untracked(&self) -> bool {
        self.index == '?' && self.worktree == '?'
    }

    /// Whether this is an ignored entry (`!!`); only requested statuses contain these.
    pub fn is_ignored(&self) -> bool {
        self.index == '!' && self.worktree == '!'
    }

    /// Whether this record is one of the unmerged/conflict codes.
    pub fn is_conflicted(&self) -> bool {
        matches!(
            (self.index, self.worktree),
            ('D', 'D')
                | ('A', 'U')
                | ('U', 'D')
                | ('U', 'A')
                | ('D', 'U')
                | ('A', 'A')
                | ('U', 'U')
        )
    }

    /// Whether a rename is recorded in the index or the worktree.
    pub fn is_rename(&self) -> bool {
        self.index == 'R' || self.worktree == 'R'
    }

    /// Whether a copy is recorded in the index or the worktree.
    pub fn is_copy(&self) -> bool {
        self.index == 'C' || self.worktree == 'C'
    }

    /// A lossy human-readable form, for diagnostics only. Never parse it back.
    pub fn display_lossy(&self) -> String {
        let mut shown = if self.is_directory {
            format!("{}/", self.path.display_lossy())
        } else {
            self.path.display_lossy()
        };
        if let Some(original) = &self.original_path {
            shown = format!("{} -> {shown}", original.display_lossy());
        }
        format!("{}{} {shown}", self.index, self.worktree)
    }
}

/// Run the fixed read-only status query against an arbitrary process binding.
///
/// Used by lifecycle preflights that scan a linked checkout (which is its own
/// work tree) with the parent handle's program and timeout. The argv is built
/// here, so no caller can add a pathspec or option.
pub(crate) fn status_at(
    process: &GitProcess,
    options: &StatusOptions,
) -> Result<Vec<StatusEntry>, GitError> {
    let mut args = vec![
        "-c",
        "core.fsmonitor=false",
        "--no-optional-locks",
        "status",
        "--porcelain=v1",
        "-z",
        options.untracked.as_git_arg(),
    ];
    if options.ignored {
        args.push("--ignored");
    }
    let output = process.checked(&args)?;
    parse_status_z(&output.stdout)
}

/// Parse `git status --porcelain=v1 -z` output.
///
/// Records are `XY <path>\0`; rename and copy records are
/// `XY <new>\0<original>\0`. Truncated records are a typed error rather than a
/// silently dropped entry.
pub(crate) fn parse_status_z(raw: &[u8]) -> Result<Vec<StatusEntry>, GitError> {
    let mut entries = Vec::new();
    let mut records = raw
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty());

    while let Some(record) = records.next() {
        if record.len() < 3 || record[2] != b' ' {
            return Err(malformed(record));
        }
        let index = record[0];
        let worktree = record[1];
        if !index.is_ascii() || !worktree.is_ascii() {
            return Err(malformed(record));
        }

        let (path, is_directory) = status_path(&record[3..])?;
        let two_paths = matches!(index, b'R' | b'C') || matches!(worktree, b'R' | b'C');
        let original_path = if two_paths {
            let original = records.next().ok_or_else(|| GitError::InvalidOutput {
                detail: format!(
                    "git status record ended after {:?}; expected a rename source path",
                    String::from_utf8_lossy(record)
                ),
            })?;
            Some(status_path(original)?.0)
        } else {
            None
        };

        entries.push(StatusEntry {
            index: char::from(index),
            worktree: char::from(worktree),
            path,
            original_path,
            is_directory,
        });
    }

    Ok(entries)
}

fn status_path(raw: &[u8]) -> Result<(GitPath, bool), GitError> {
    let (raw, is_directory) = match raw.strip_suffix(b"/") {
        Some(without_slash) => (without_slash, true),
        None => (raw, false),
    };
    Ok((GitPath::from_git_bytes(raw)?, is_directory))
}

fn malformed(record: &[u8]) -> GitError {
    GitError::InvalidOutput {
        detail: format!(
            "malformed git status record {:?}",
            String::from_utf8_lossy(record)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_untracked_and_modified_records() {
        let entries = parse_status_z(b"?? new file.txt\0 M a.txt\0").unwrap();

        assert_eq!(entries.len(), 2);
        assert!(entries[0].is_untracked());
        assert_eq!(entries[0].path().as_os_str(), "new file.txt");
        assert_eq!(entries[1].index(), ' ');
        assert_eq!(entries[1].worktree(), 'M');
    }

    #[test]
    fn parses_rename_new_then_original() {
        let entries = parse_status_z(b"R  renamed.txt\0a.txt\0").unwrap();

        assert_eq!(entries.len(), 1);
        assert!(entries[0].is_rename());
        assert_eq!(entries[0].path().as_os_str(), "renamed.txt");
        assert_eq!(entries[0].original_path().unwrap().as_os_str(), "a.txt");
    }

    #[test]
    fn marks_a_trailing_slash_as_a_directory_and_strips_it() {
        let entries = parse_status_z(b"?? dir with space/\0").unwrap();

        assert!(entries[0].is_directory());
        assert_eq!(entries[0].path().as_os_str(), "dir with space");
    }

    #[test]
    fn truncated_rename_is_an_error() {
        let error = parse_status_z(b"R  renamed.txt\0").unwrap_err();

        assert!(matches!(error, GitError::InvalidOutput { .. }));
    }

    #[test]
    fn malformed_record_is_an_error() {
        assert!(matches!(
            parse_status_z(b"nonsense\0"),
            Err(GitError::InvalidOutput { .. })
        ));
    }
}
