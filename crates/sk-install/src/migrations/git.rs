//! Bounded Git process helpers used by migrations that own a clone-local effect.
//!
//! Upstream migrations call `git` directly for the clone-local half of an
//! effect (`git config --local`, `git rev-parse --git-path`, `git status`).
//! This module keeps those calls in one place with a project-root CWD and no
//! shell, so a migration never invents its own argument quoting.

use std::path::Path;
use std::process::{Command, Output};

use crate::error::InstallError;

/// Run `git` in `project_root` and return its output, whether or not it exited 0.
pub fn run(project_root: &Path, args: &[&str]) -> Result<Output, InstallError> {
    Command::new("git")
        .args(args)
        .current_dir(project_root)
        .output()
        .map_err(|source| InstallError::io("run git", project_root, source))
}

/// Run `git` and return trimmed stdout only when it exited successfully.
pub fn success_stdout(project_root: &Path, args: &[&str]) -> Result<Option<String>, InstallError> {
    let output = run(project_root, args)?;
    if !output.status.success() {
        return Ok(None);
    }
    let stdout =
        String::from_utf8(output.stdout).map_err(|error| InstallError::UpgradeRefused {
            detail: format!("git {} returned non-UTF-8 output: {error}", args.join(" ")),
        })?;
    Ok(Some(stdout.trim().to_string()))
}

/// Read one `git config --local --get` value.
pub fn config_get(project_root: &Path, key: &str) -> Result<Option<String>, InstallError> {
    success_stdout(project_root, &["config", "--local", "--get", key])
}

/// Set one `git config --local` value, failing when Git rejects it.
pub fn config_set(project_root: &Path, key: &str, value: &str) -> Result<(), InstallError> {
    let output = run(project_root, &["config", "--local", key, value])?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(InstallError::UpgradeRefused {
        detail: format!("git config --local {key} failed: {stderr}"),
    })
}

/// Whether `git check-ignore` says this project-relative path is ignored.
///
/// `None` means Git could not provide a trustworthy answer (not a repository,
/// no git binary, or an unexpected exit code), matching the upstream helper.
pub fn check_ignore(project_root: &Path, relative: &str) -> Result<Option<bool>, InstallError> {
    let output = run(
        project_root,
        &["check-ignore", "--quiet", "--no-index", "--", relative],
    )?;
    match output.status.code() {
        Some(0) => Ok(Some(true)),
        Some(1) => Ok(Some(false)),
        _ => Ok(None),
    }
}
