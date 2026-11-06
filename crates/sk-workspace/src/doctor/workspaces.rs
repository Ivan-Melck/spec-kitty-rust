//! `.worktrees/` husk detection (`doctor workspaces`).
//!
//! A husk is an entry under `.worktrees/` that has no `.git` entry, so it is not
//! a usable worktree: git commands run inside it silently walk up to the primary
//! repository. Each husk is annotated with whether `git worktree list` still
//! registers that path, because a *registered* husk must never be removed
//! automatically — it needs `git worktree repair` or `git worktree remove`.
//!
//! # Read-only
//!
//! This module only reports. The upstream `--fix` arm (which deletes
//! unregistered husks behind a destructive guard) is deliberately not
//! implemented here; the adapter refuses it with a named code instead of
//! silently doing nothing.

use std::path::Path;

use serde_json::{Map, Value, json};

use super::git_probe::{self, GitAnswer};
use super::{DoctorError, require_directory};

/// The directory under the repository root that holds lane worktrees.
pub const WORKTREES_DIRNAME: &str = ".worktrees";

/// One `.worktrees/` entry that lacks a `.git` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HuskEntry {
    /// Path relative to the repository root.
    pub path: String,
    /// `Some(true)` when `git worktree list` still registers the path,
    /// `Some(false)` when it does not, `None` when registration could not be
    /// read.
    pub registered: Option<bool>,
}

impl HuskEntry {
    /// The upstream `--json` row shape.
    pub fn to_json(&self) -> Value {
        json!({"path": self.path, "registered": self.registered})
    }
}

/// The husk scan result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HuskReport {
    /// The scanned `.worktrees/` directory.
    pub worktrees_dir: String,
    /// Every husk found, sorted by path.
    pub husks: Vec<HuskEntry>,
    /// Why the registered-worktree probe failed, when it did.
    pub registration_error: Option<String>,
}

impl HuskReport {
    /// Healthy only when there are no husks and no registration error.
    pub fn healthy(&self) -> bool {
        self.husks.is_empty() && self.registration_error.is_none()
    }

    /// The upstream `--json` payload.
    pub fn to_json(&self) -> Value {
        let mut payload = Map::new();
        payload.insert("worktrees_dir".into(), json!(self.worktrees_dir));
        payload.insert("healthy".into(), json!(self.healthy()));
        payload.insert(
            "registration_error".into(),
            self.registration_error
                .clone()
                .map(Value::from)
                .unwrap_or(Value::Null),
        );
        payload.insert(
            "husks".into(),
            Value::Array(self.husks.iter().map(HuskEntry::to_json).collect()),
        );
        Value::Object(payload)
    }
}

/// List `.worktrees/*` directories that lack a `.git` entry.
///
/// An absent `.worktrees/` is an empty (healthy) scan. A repository that cannot
/// list its worktrees keeps the husks but reports `registered: null` and the
/// registration error, so an operator never reads a guess.
pub fn scan_workspace_husks(root: &Path) -> Result<HuskReport, DoctorError> {
    require_directory(root)?;

    let worktrees_dir = root.join(WORKTREES_DIRNAME);
    let mut report = HuskReport {
        worktrees_dir: worktrees_dir.to_string_lossy().into_owned(),
        husks: Vec::new(),
        registration_error: None,
    };

    let entries = match std::fs::read_dir(&worktrees_dir) {
        Ok(entries) => entries,
        Err(_) => return Ok(report),
    };

    let registered = match git_probe::open(root) {
        GitAnswer::Answered(repository) => match repository.worktrees() {
            Ok(worktrees) => {
                let mut paths = Vec::new();
                for worktree in worktrees {
                    let path = std::fs::canonicalize(worktree.path())
                        .unwrap_or_else(|_| worktree.path().to_path_buf());
                    paths.push(path);
                }
                Ok(paths)
            }
            Err(error) => Err(error.to_string()),
        },
        GitAnswer::NoRepository => Err("not inside a Git work tree".to_owned()),
        GitAnswer::Unreadable(detail) => Err(detail),
    };

    let mut husks = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !std::fs::metadata(&path).is_ok_and(|metadata| metadata.is_dir()) {
            continue;
        }
        if path.join(".git").exists() {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|_| path.to_string_lossy().into_owned());
        let is_registered = match &registered {
            Ok(paths) => {
                let resolved = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
                Some(paths.contains(&resolved))
            }
            Err(_) => None,
        };
        husks.push(HuskEntry {
            path: relative,
            registered: is_registered,
        });
    }
    husks.sort_by(|left, right| left.path.cmp(&right.path));
    report.husks = husks;
    if let Err(detail) = registered {
        report.registration_error = Some(format!("git worktree list failed: {detail}"));
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_worktrees_dir_is_a_healthy_empty_scan() {
        let root = tempfile::tempdir().unwrap();
        let report = scan_workspace_husks(root.path()).unwrap();
        assert!(report.healthy());
        assert!(report.husks.is_empty());
    }

    #[test]
    fn a_directory_without_a_git_entry_is_a_husk_and_is_never_called_registered() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".worktrees/000-abcd-lane-a")).unwrap();
        let report = scan_workspace_husks(root.path()).unwrap();
        assert!(!report.healthy());
        assert_eq!(report.husks.len(), 1);
        assert_eq!(report.husks[0].path, ".worktrees/000-abcd-lane-a");
        // No repository here: registration state is unknown, never a guess.
        assert_eq!(report.husks[0].registered, None);
        assert!(report.registration_error.is_some());
    }

    #[test]
    fn an_entry_with_a_git_marker_is_not_a_husk() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join(".worktrees/real");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".git"), b"gitdir: /elsewhere\n").unwrap();
        let report = scan_workspace_husks(root.path()).unwrap();
        assert!(report.husks.is_empty());
    }
}
