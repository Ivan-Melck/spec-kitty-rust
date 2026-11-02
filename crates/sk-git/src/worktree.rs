//! `git worktree list --porcelain` parsing.
//!
//! The `-z` form (git 2.36 and later) is preferred because it preserves path
//! bytes exactly and survives newlines in a path. For older git versions the
//! adapter falls back to the newline-delimited porcelain form, which is still
//! byte-preserving for every path that does not itself contain a newline.

use std::path::{Path, PathBuf};

use crate::error::GitError;
use crate::path::{os_string_from_git_bytes, utf8_field};

/// One registered worktree of a repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    path: PathBuf,
    head: Option<String>,
    branch: Option<String>,
    detached: bool,
    bare: bool,
    locked: Option<String>,
    prunable: Option<String>,
}

impl Worktree {
    /// The absolute path to the checkout, with exact bytes retained.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The commit HEAD points at, when git reported one.
    pub fn head(&self) -> Option<&str> {
        self.head.as_deref()
    }

    /// The short branch name (`main`, `feature/topic`), when a branch is
    /// checked out. `None` for a detached or bare worktree.
    pub fn branch(&self) -> Option<&str> {
        self.branch.as_deref()
    }

    /// Whether HEAD is detached in this worktree.
    pub fn is_detached(&self) -> bool {
        self.detached
    }

    /// Whether this is the repository's bare main worktree.
    pub fn is_bare(&self) -> bool {
        self.bare
    }

    /// The lock reason, or `Some("")` when locked without a reason. `None`
    /// when the worktree is not locked.
    pub fn locked(&self) -> Option<&str> {
        self.locked.as_deref()
    }

    /// The prunable reason, or `Some("")` when git marked it prunable without
    /// a reason. `None` when it is not prunable.
    pub fn prunable(&self) -> Option<&str> {
        self.prunable.as_deref()
    }
}

/// Parse NUL-delimited `worktree list --porcelain -z` output.
pub(crate) fn parse_worktree_z(raw: &[u8]) -> Result<Vec<Worktree>, GitError> {
    parse_fields(raw.split(|byte| *byte == 0))
}

/// Parse newline-delimited `worktree list --porcelain` output.
///
/// A single trailing carriage return is stripped from each line so CRLF output
/// is accepted, and the final path field keeps its raw bytes.
pub(crate) fn parse_worktree_lines(raw: &[u8]) -> Result<Vec<Worktree>, GitError> {
    parse_fields(
        raw.split(|byte| *byte == b'\n')
            .map(|line| line.strip_suffix(b"\r").unwrap_or(line)),
    )
}

fn parse_fields<'a>(fields: impl Iterator<Item = &'a [u8]>) -> Result<Vec<Worktree>, GitError> {
    let mut worktrees = Vec::new();
    let mut current: Option<Worktree> = None;

    for field in fields {
        if field.is_empty() {
            finish(&mut current, &mut worktrees);
            continue;
        }

        if let Some(raw_path) = field.strip_prefix(b"worktree ") {
            finish(&mut current, &mut worktrees);
            current = Some(Worktree {
                path: PathBuf::from(os_string_from_git_bytes(raw_path, "worktree path")?),
                head: None,
                branch: None,
                detached: false,
                bare: false,
                locked: None,
                prunable: None,
            });
            continue;
        }

        let Some(worktree) = current.as_mut() else {
            return Err(GitError::InvalidOutput {
                detail: format!(
                    "worktree field appeared before a worktree header: {:?}",
                    String::from_utf8_lossy(field)
                ),
            });
        };
        apply_field(worktree, field)?;
    }

    finish(&mut current, &mut worktrees);
    Ok(worktrees)
}

fn finish(current: &mut Option<Worktree>, worktrees: &mut Vec<Worktree>) {
    if let Some(worktree) = current.take() {
        worktrees.push(worktree);
    }
}

fn apply_field(worktree: &mut Worktree, field: &[u8]) -> Result<(), GitError> {
    if let Some(value) = field.strip_prefix(b"HEAD ") {
        worktree.head = Some(utf8_field(value, "HEAD")?);
    } else if let Some(value) = field.strip_prefix(b"branch ") {
        let short = value.strip_prefix(b"refs/heads/").unwrap_or(value);
        worktree.branch = Some(utf8_field(short, "branch name")?);
    } else if field == b"bare" {
        worktree.bare = true;
    } else if field == b"detached" {
        worktree.detached = true;
    } else if field == b"locked" {
        worktree.locked = Some(String::new());
    } else if let Some(reason) = field.strip_prefix(b"locked ") {
        worktree.locked = Some(utf8_field(reason, "lock reason")?);
    } else if field == b"prunable" {
        worktree.prunable = Some(String::new());
    } else if let Some(reason) = field.strip_prefix(b"prunable ") {
        worktree.prunable = Some(utf8_field(reason, "prunable reason")?);
    }
    // Unknown fields are ignored so a newer git can add them without breaking
    // an older adapter.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_z_blocks_with_branch_detached_and_lock_reason() {
        let raw = b"worktree /tmp/main\0HEAD 1111\0branch refs/heads/main\0\0\
worktree /tmp/linked dir\0HEAD 2222\0detached\0locked held for a test\0\0";

        let worktrees = parse_worktree_z(raw).unwrap();

        assert_eq!(worktrees.len(), 2);
        assert_eq!(worktrees[0].path(), Path::new("/tmp/main"));
        assert_eq!(worktrees[0].branch(), Some("main"));
        assert_eq!(worktrees[0].head(), Some("1111"));
        assert_eq!(worktrees[1].path(), Path::new("/tmp/linked dir"));
        assert!(worktrees[1].is_detached());
        assert_eq!(worktrees[1].branch(), None);
        assert_eq!(worktrees[1].locked(), Some("held for a test"));
    }

    #[test]
    fn parses_line_format_and_crlf() {
        let raw = b"worktree /tmp/main\r\nHEAD 1111\r\nbranch refs/heads/main\r\n\r\n\
worktree /tmp/linked\r\nHEAD 2222\r\ndetached\r\nprunable missing\r\n";

        let worktrees = parse_worktree_lines(raw).unwrap();

        assert_eq!(worktrees.len(), 2);
        assert_eq!(worktrees[0].branch(), Some("main"));
        assert!(worktrees[1].is_detached());
        assert_eq!(worktrees[1].prunable(), Some("missing"));
    }

    #[test]
    fn bare_worktree_is_flagged() {
        let worktrees = parse_worktree_z(b"worktree /tmp/bare\0bare\0\0").unwrap();

        assert!(worktrees[0].is_bare());
        assert_eq!(worktrees[0].branch(), None);
    }

    #[test]
    fn field_before_header_is_an_error() {
        let error = parse_worktree_z(b"HEAD 1111\0").unwrap_err();

        assert!(matches!(error, GitError::InvalidOutput { .. }));
    }
}
