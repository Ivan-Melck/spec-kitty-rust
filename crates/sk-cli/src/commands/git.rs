//! Read-only Git inspection through `sk-git`.

use std::path::Path;

use serde_json::{Value, json};
use sk_git::GitRepository;

use crate::commands::path_string;
use crate::commands::project::resolve_root;
use crate::output::{CliError, CommandData};

/// Inspect the repository containing the project root.
pub fn inspect(explicit: Option<&Path>) -> Result<CommandData, CliError> {
    let root = resolve_root(explicit)?;
    let repository = GitRepository::open(&root).map_err(|error| {
        CliError::domain(format!(
            "could not open a Git work tree at {}: {error}",
            root.display()
        ))
    })?;

    let branch = repository
        .current_branch()
        .map_err(|error| CliError::domain(format!("could not read the current branch: {error}")))?;
    let head = repository
        .head_commit()
        .map_err(|error| CliError::domain(format!("could not read HEAD: {error}")))?;
    let status = repository
        .status()
        .map_err(|error| CliError::domain(format!("could not read Git status: {error}")))?;

    let mut untracked = 0usize;
    let mut ignored = 0usize;
    let mut conflicted = 0usize;
    let mut renamed = 0usize;
    let mut copied = 0usize;
    let mut modified = 0usize;
    let mut staged = 0usize;

    let entries: Vec<Value> = status
        .iter()
        .map(|entry| {
            if entry.is_untracked() {
                untracked += 1;
            }
            if entry.is_ignored() {
                ignored += 1;
            }
            if entry.is_conflicted() {
                conflicted += 1;
            }
            if entry.is_rename() {
                renamed += 1;
            }
            if entry.is_copy() {
                copied += 1;
            }
            if !entry.is_untracked()
                && !entry.is_ignored()
                && (entry.index() == 'M' || entry.worktree() == 'M')
            {
                modified += 1;
            }
            if entry.index() != ' ' && entry.index() != '?' && entry.index() != '!' {
                staged += 1;
            }

            let mut value = json!({
                "path": entry.path().display_lossy(),
                "index": entry.index().to_string(),
                "worktree": entry.worktree().to_string(),
                "is_directory": entry.is_directory(),
                "untracked": entry.is_untracked(),
                "ignored": entry.is_ignored(),
                "conflicted": entry.is_conflicted(),
                "rename": entry.is_rename(),
                "copy": entry.is_copy(),
            });
            if let Some(original) = entry.original_path() {
                value["original_path"] = json!(original.display_lossy());
            }
            value
        })
        .collect();

    let total = entries.len();

    Ok(CommandData::success(json!({
        "root": path_string(repository.root()),
        "branch": branch,
        "detached": branch.is_none(),
        "head": head,
        "dirty": !status.is_empty(),
        "counts": {
            "total": total,
            "staged": staged,
            "modified": modified,
            "untracked": untracked,
            "ignored": ignored,
            "conflicted": conflicted,
            "renamed": renamed,
            "copied": copied,
        },
        "entries": entries,
        "read_only": true,
        "worktrees_exposed": false,
    })))
}
