//! Where an Op record belongs, and the refusal vocabulary for writing it.
//!
//! Source: `specify_cli/invocation/op_custody.py` (#6564).
//!
//! `find_repo_root` maps a linked worktree back to the repository root
//! checkout on purpose, because governance is discovered there. An Op record
//! is different: it is the trail of work done in the checkout the command runs
//! in, and it is committed on that checkout's branch. Writing it into the
//! repository root checkout from a feature worktree strands untracked files on
//! its protected branch, where the guard then refuses to commit them.
//!
//! This module owns the placement answers and the operator-facing refusal
//! vocabulary. It deliberately does **not** decide admission: the reference's
//! `ProfileInvocationExecutor.op_write_refusal` combines the record root's
//! branch, the project + record-root protection policy (union), and the
//! `op_commit_policy` skip probes (an ignored path, or a primary branch that
//! tracks no `kitty-ops/<ULID>.jsonl`). Those probes need `sk-workspace`'s
//! protection policy and `sk-git`'s ignore/primary-tracking reads, which this
//! stage does not depend on, so no admission decision is faked here.

use std::io;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use sk_kernel::git_topology::{git_common_dir, git_toplevel};
use sk_kernel::resolve_reference_path;
use thiserror::Error;

use crate::python_json::python_repr;
use crate::record::InvocationId;
use crate::writer::InvocationWriter;

/// Error code of the protected-branch admission refusal.
pub const OP_WRITE_PROTECTED_BRANCH: &str = "OP_WRITE_PROTECTED_BRANCH";
/// Error code of the unreadable-protection-configuration refusal.
pub const OP_WRITE_PROTECTION_CONFIG_INVALID: &str = "OP_WRITE_PROTECTION_CONFIG_INVALID";

/// Bound on the `git worktree list` probe, mirroring the reference.
const GIT_TIMEOUT: Duration = Duration::from_secs(30);
const WORKTREE_LINE_PREFIX: &str = "worktree ";

/// The refusal code carried by [`OpWriteRefusal`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpWriteRefusalCode {
    /// The record root is on a branch the commit guard protects.
    ProtectedBranch,
    /// The branch-protection settings could not be read at all.
    ProtectionConfigInvalid,
}

impl OpWriteRefusalCode {
    /// The wire/error code string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProtectedBranch => OP_WRITE_PROTECTED_BRANCH,
            Self::ProtectionConfigInvalid => OP_WRITE_PROTECTION_CONFIG_INVALID,
        }
    }
}

/// Which Op operation the refusal is about.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpWriteAction {
    /// Opening an Op (`invoke`).
    Open,
    /// Closing an Op (`complete_invocation` with `closed_by = agent`).
    Close,
}

impl OpWriteAction {
    /// The phrase the message embeds.
    pub fn phrase(self) -> &'static str {
        match self {
            Self::Open => "open the Op",
            Self::Close => "close the Op",
        }
    }
}

/// Why an Op record may not be written in its checkout.
///
/// `reason` is the guard's answer (`protected branch 'main'`, or why the
/// protection configuration could not be read); it never names the
/// protected-branch override.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpWriteRefusal {
    /// The stable refusal code.
    pub code: OpWriteRefusalCode,
    /// The guard's reason, verbatim.
    pub reason: String,
}

impl OpWriteRefusal {
    /// A protected-branch refusal for `branch`.
    ///
    /// The reason is `protected branch {branch!r}` — CPython's `repr`, which
    /// is what the reference's f-string produces.
    pub fn protected_branch(branch: &str) -> Self {
        Self {
            code: OpWriteRefusalCode::ProtectedBranch,
            reason: format!("protected branch {}", python_repr(branch)),
        }
    }

    /// An unreadable protection-configuration refusal.
    pub fn protection_config_invalid(reason: impl Into<String>) -> Self {
        Self {
            code: OpWriteRefusalCode::ProtectionConfigInvalid,
            reason: reason.into(),
        }
    }

    /// The operator-facing refusal message.
    ///
    /// It never suggests overriding the protected-branch guard.
    pub fn message(&self, record_root: &Path, action: OpWriteAction) -> String {
        let (problem, remedy) = match self.code {
            OpWriteRefusalCode::ProtectedBranch => (
                format!(
                    "its Op record would be written in {} on {}, where the record cannot be committed",
                    record_root.display(),
                    self.reason
                ),
                "Run the command from a topic branch, or from a worktree whose branch is not protected.",
            ),
            OpWriteRefusalCode::ProtectionConfigInvalid => (
                format!(
                    "the branch protection settings for {} cannot be read ({})",
                    record_root.display(),
                    self.reason
                ),
                "Fix the protection: block in .kittify/config.yaml, then run the command again.",
            ),
        };
        format!(
            "Refusing to {}: {problem}. Nothing was written. {remedy}",
            action.phrase()
        )
    }
}

/// The executor refused to write an Op record before writing anything.
#[derive(Clone, Debug, Error)]
#[error("{}", message)]
pub struct OpWriteRefused {
    /// The refusal that caused this.
    pub refusal: OpWriteRefusal,
    /// The checkout whose `kitty-ops/` would have received the record.
    pub record_root: PathBuf,
    /// The rendered operator-facing message.
    message: String,
}

impl OpWriteRefused {
    /// Build the error for `refusal` in `record_root` for `action`.
    pub fn new(refusal: OpWriteRefusal, record_root: &Path, action: OpWriteAction) -> Self {
        let message = refusal.message(record_root, action);
        Self {
            refusal,
            record_root: record_root.to_path_buf(),
            message,
        }
    }
}

/// Return the checkout whose `kitty-ops/` holds the Op records of this command.
///
/// That is the Git checkout containing `start` when it belongs to the
/// project's repository (the repository root checkout or one of its linked
/// worktrees). Anything else (no Git checkout, or a checkout of a different
/// repository) falls back to `project_root`, which is what every Op command
/// used before #6564.
///
/// Both checks are the topology ones the reference's `safe_commit` applies:
/// `git_toplevel`, then a common-dir comparison that holds whatever form a
/// worktree's `gitdir:` pointer takes — including the relative pointer Git
/// 2.48 can write.
pub fn resolve_op_record_root(project_root: &Path, start: &Path) -> PathBuf {
    let Ok(toplevel) = git_toplevel(start) else {
        return project_root.to_path_buf();
    };
    if !is_worktree_of(project_root, &toplevel) {
        return project_root.to_path_buf();
    }
    toplevel
}

/// Whether `worktree_root` belongs to `repo_root`'s repository.
///
/// Mirrors `commit_helpers._is_worktree_of`: the toplevel of `worktree_root`
/// must be itself (a nested checkout is not a worktree of the enclosing
/// repository, even though it shares its common dir), the two roots may be
/// the same directory, and otherwise their canonical common dirs must match.
pub fn is_worktree_of(repo_root: &Path, worktree_root: &Path) -> bool {
    let Ok(toplevel) = git_toplevel(worktree_root) else {
        return false;
    };
    let resolved_worktree = resolve_or_keep(worktree_root);
    if toplevel != resolved_worktree {
        return false;
    }
    if resolved_worktree == resolve_or_keep(repo_root) {
        return true;
    }
    match (git_common_dir(worktree_root), git_common_dir(repo_root)) {
        (Ok(worktree_common), Ok(repo_common)) => worktree_common == repo_common,
        _ => false,
    }
}

/// Return the checkout holding `kitty-ops/<invocation_id>.jsonl`, or `None`.
///
/// Looks in `preferred` (the invoking checkout) first, then the project root,
/// then every other checkout the repository reports, so an Op opened in a
/// linked worktree — or by an older CLI in the repository root checkout — is
/// found from either side. An invalid id is never found.
pub fn locate_op_record_root(
    project_root: &Path,
    invocation_id: &str,
    preferred: Option<&Path>,
) -> Option<PathBuf> {
    let invocation_id = InvocationId::parse(invocation_id).ok()?;
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(preferred) = preferred {
        candidates.push(preferred.to_path_buf());
    }
    candidates.push(project_root.to_path_buf());
    candidates.extend(repository_checkouts(project_root));
    for checkout in candidates {
        let key = resolve_or_keep(&checkout);
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        if holds_record(&checkout, &invocation_id) {
            return Some(checkout);
        }
    }
    None
}

fn holds_record(checkout: &Path, invocation_id: &InvocationId) -> bool {
    match InvocationWriter::new(checkout).invocation_path(invocation_id) {
        Ok(record) => record.is_file(),
        Err(_) => false,
    }
}

/// Every checkout `git worktree list --porcelain` reports for the repository.
///
/// The listing is drained while Git runs (a file-backed capture, so a large
/// repository cannot deadlock the probe). A failing, timed-out, or absent Git
/// binary degrades to no extra checkouts, exactly like the reference's
/// `_git_lines`; a `None` here is the reference's empty list.
fn repository_checkouts(project_root: &Path) -> Vec<PathBuf> {
    let Some(stdout) = git_lines(project_root, &["worktree", "list", "--porcelain"]) else {
        return Vec::new();
    };
    stdout
        .lines()
        .filter_map(|line| line.strip_prefix(WORKTREE_LINE_PREFIX))
        .map(|path| resolve_or_keep(Path::new(path.trim())))
        .collect()
}

fn git_lines(cwd: &Path, arguments: &[&str]) -> Option<String> {
    // Git writes to a regular temporary file, never a pipe: `subprocess.run(
    // capture_output=True)` drains concurrently, while reading a pipe only
    // after exit deadlocks as soon as the listing exceeds the pipe capacity
    // (a repository with several hundred worktrees). No output cap is added;
    // the full listing is what the reference parses.
    let mut captured = tempfile::tempfile().ok()?;
    let child_stdout = captured.try_clone().ok()?;
    let mut child = Command::new("git")
        .args(arguments)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(child_stdout))
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + GIT_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                // The direct child only; no process group or global signal.
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    if !status.success() {
        return None;
    }
    captured.seek(std::io::SeekFrom::Start(0)).ok()?;
    let mut bytes = Vec::new();
    captured.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Resolve a checkout path for identity comparison, keeping it verbatim when
/// reference-path resolution refuses it (Python's `except OSError: keep`).
/// Requires no lock and reads no record.
fn resolve_or_keep(path: &Path) -> PathBuf {
    resolve_reference_path(path).unwrap_or_else(|_| path.to_path_buf())
}

/// A Git probe that could not run at all.
///
/// Reserved for callers that must distinguish "no checkout" from "Git is
/// unusable"; the placement functions above deliberately degrade instead.
#[derive(Debug, Error)]
#[error("Git could not be invoked in {cwd}: {source}")]
pub struct GitProbeError {
    /// The directory the probe ran in.
    pub cwd: PathBuf,
    /// The underlying spawn failure.
    #[source]
    pub source: io::Error,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protected_branch_message_matches_the_reference_shape() {
        let refusal = OpWriteRefusal::protected_branch("main");
        assert_eq!(refusal.code, OpWriteRefusalCode::ProtectedBranch);
        assert_eq!(refusal.reason, "protected branch 'main'");
        assert_eq!(
            refusal.message(Path::new("/repo/primary"), OpWriteAction::Open),
            "Refusing to open the Op: its Op record would be written in /repo/primary on \
             protected branch 'main', where the record cannot be committed. Nothing was \
             written. Run the command from a topic branch, or from a worktree whose branch \
             is not protected."
        );
        assert_eq!(
            refusal.message(Path::new("/repo/primary"), OpWriteAction::Close),
            "Refusing to close the Op: its Op record would be written in /repo/primary on \
             protected branch 'main', where the record cannot be committed. Nothing was \
             written. Run the command from a topic branch, or from a worktree whose branch \
             is not protected."
        );
    }

    #[test]
    fn protection_config_message_matches_the_reference_shape() {
        let refusal = OpWriteRefusal::protection_config_invalid("unreadable yaml");
        assert_eq!(refusal.code.as_str(), OP_WRITE_PROTECTION_CONFIG_INVALID);
        assert_eq!(
            refusal.message(Path::new("/repo"), OpWriteAction::Open),
            "Refusing to open the Op: the branch protection settings for /repo cannot be \
             read (unreadable yaml). Nothing was written. Fix the protection: block in \
             .kittify/config.yaml, then run the command again."
        );
    }

    #[test]
    fn refused_error_carries_the_refusal_and_root() {
        let refusal = OpWriteRefusal::protected_branch("release");
        let error = OpWriteRefused::new(refusal.clone(), Path::new("/lane"), OpWriteAction::Close);
        assert_eq!(error.refusal, refusal);
        assert_eq!(error.record_root, Path::new("/lane"));
        assert!(error.to_string().starts_with("Refusing to close the Op:"));
    }

    #[test]
    fn invalid_ids_are_never_found() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("kitty-ops")).unwrap();
        std::fs::write(temp.path().join("kitty-ops").join("outside.jsonl"), "{}\n").unwrap();
        for id in ["../../etc/passwd", "nested/path", "", "lowercase"] {
            assert_eq!(locate_op_record_root(temp.path(), id, None), None);
        }
    }
}
