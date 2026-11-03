//! Primary/non-owned status commit orchestration.
//!
//! The pure decision and artifact halves live in
//! [`sk_status::mutation_commit`]; this module is the Git boundary. It consumes
//! only the existing public seams:
//!
//! - [`sk_workspace::ProtectionPolicy`] for the protected-branch fact (never a
//!   hand-rolled ref list);
//! - [`sk_git::GitRepository::plan_safe_commit`] / `apply_safe_commit` for the
//!   path-scoped commit (never `git add -A`, never a second commit primitive);
//! - the caller-supplied [`sk_status::HeadCommittedProbe`] for the verified
//!   rollback (never a guessed commit).
//!
//! # Landed / indeterminate / not landed
//!
//! A safe-commit failure is classified so the caller can never truncate or
//! delete state that a commit may already carry:
//!
//! - [`CommitAttempt::Landed`] — the commit exists (including
//!   `SafeCommitLanded`, where only a post-landing index repair failed);
//! - [`CommitAttempt::Indeterminate`] — a commit object may exist (advance
//!   unconfirmed, timeout, spawn/I/O/command failure, unreadable HEAD, …);
//! - [`CommitAttempt::NotLanded`] — a pre-commit refusal (empty changeset,
//!   HEAD mismatch, path policy, protected destination, stage/commit/hook
//!   rejection, plan stale, …).
//!
//! Only `NotLanded` permits the verified rollback of this operation's own
//! uncommitted status tail.

use std::path::{Path, PathBuf};

use serde_json::Value;
use sk_git::{
    BranchName, CommitGuardCapability, GitError, GitPath, GitRepository, ProtectedBranches,
    RefName, SafeCommitRequest, StartPoint, normalize_safe_commit_path,
};
use sk_workspace::ProtectionPolicy;

use crate::compat::support::{CompatError, MissionView};

/// The canonical operator remedy in the protected-primary refusal.
const PROTECTED_REMEDY: &str =
    "--start-branch <topic-branch> or SPEC_KITTY_ALLOW_PROTECTED_BRANCH_COMMITS=1";

/// Resolve the effective auto-commit decision from the project config and flags.
pub(super) fn resolve_auto_commit(
    root: &Path,
    explicit: Option<bool>,
) -> Result<bool, CompatError> {
    let config = sk_status::mutation_commit::read_auto_commit_config(root)
        .map_err(|error| CompatError::domain(error.to_string()))?;
    Ok(config.resolve(explicit))
}

/// True for the modern stored topologies whose status artifacts the reference
/// commits (`LANES` / `SINGLE_BRANCH`). A flat/legacy mission keeps the
/// primary-uncommitted status write.
pub(super) fn transactional_topology(mission: &impl MissionView) -> bool {
    matches!(
        mission.meta_field("topology").and_then(Value::as_str),
        Some("lanes" | "single_branch")
    )
}

/// The resolved destination and protection facts for one auto-commit move.
pub(super) struct CommitAuthority {
    /// The already-opened checkout the commit runs from.
    pub repository: GitRepository,
    /// The validated destination branch.
    pub target: BranchName,
    /// The destination spelling used in payloads.
    pub target_name: String,
    /// The caller-resolved protected set.
    pub protected: ProtectedBranches,
}

impl CommitAuthority {
    /// The repository-relative form of `path` (the evidence reference).
    pub(super) fn evidence_ref(&self, path: &Path) -> Option<String> {
        path.strip_prefix(self.repository.root())
            .ok()
            .and_then(|relative| relative.to_str())
            .map(|relative| relative.replace('\\', "/"))
    }

    /// Read the committed blob at the destination and compare it with `bytes`.
    ///
    /// Durable is never inferred from `safe_commit` returning `Ok`: the exact
    /// committed bytes must be readable at the destination ref.
    pub(super) fn readback_matches(&self, path: &Path, bytes: &[u8]) -> bool {
        let Some(relative) = self.evidence_ref(path) else {
            return false;
        };
        let Ok(git_path) = GitPath::from_git_bytes(relative.as_bytes()) else {
            return false;
        };
        let Ok(reference) = RefName::parse(&format!("refs/heads/{}", self.target_name)) else {
            return false;
        };
        matches!(
            self.repository
                .read_blob_at(&StartPoint::Ref(reference), &git_path),
            Ok(Some(committed)) if committed == bytes
        )
    }
}

/// Resolve the auto-commit authority, refusing a protected destination and a
/// checkout that is not on the destination branch **before any mutation**.
///
/// `Ok(None)` means the caller chose `--no-auto-commit` (or the project default
/// is off); every write then stays local exactly like the reference's
/// `local_only` leg.
pub(super) fn prepare(
    root: &Path,
    mission: &impl MissionView,
    command: &str,
    auto_commit: bool,
) -> Result<Option<CommitAuthority>, CompatError> {
    if !auto_commit {
        return Ok(None);
    }
    let target_name = mission
        .meta_field("target_branch")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            CompatError::domain(
                "auto-commit needs a stored meta.json target_branch; pass --no-auto-commit to write status locally",
            )
        })?;
    let policy =
        ProtectionPolicy::resolve_for_mission(root, mission.slug_str()).map_err(|error| {
            CompatError::domain(format!(
                "could not resolve the protected-branch policy for auto-commit: {error}"
            ))
        })?;
    if policy.is_protected(target_name) {
        return Err(CompatError::coded(
            "PROTECTED_BRANCH_COMMIT_REFUSED",
            format!(
                "Refusing to run `{command}` with auto-commit on protected branch '{target_name}' before mutating status files. Run status commit operations from an allowed coordination/lane branch, or unblock with: {PROTECTED_REMEDY}."
            ),
        ));
    }
    let repository = GitRepository::open(root).map_err(|error| {
        CompatError::coded(
            "AUTO_COMMIT_REQUIRES_GIT",
            format!(
                "auto-commit needs a readable Git checkout at {}: {error}. Pass --no-auto-commit to write status locally."
            , root.display()),
        )
    })?;
    let observed = repository.current_branch().map_err(|error| {
        CompatError::coded(
            "AUTO_COMMIT_HEAD_UNREADABLE",
            format!("auto-commit could not read the checked-out branch: {error}"),
        )
    })?;
    if observed.as_deref() != Some(target_name) {
        return Err(CompatError::coded(
            "AUTO_COMMIT_TARGET_CHECKOUT_REQUIRED",
            format!(
                "auto-commit records status/review evidence on the mission target branch '{target_name}', but {} has '{}' checked out. Check out '{target_name}' or pass --no-auto-commit.",
                root.display(),
                observed.as_deref().unwrap_or("<detached HEAD>")
            ),
        ));
    }
    let protected = policy.branch_set().map_err(|error| {
        CompatError::domain(format!(
            "could not resolve the protected set for auto-commit: {error}"
        ))
    })?;
    let target = BranchName::parse(target_name)
        .map_err(|error| CompatError::domain(format!("invalid target branch: {error}")))?;
    Ok(Some(CommitAuthority {
        repository,
        target,
        target_name: target_name.to_owned(),
        protected,
    }))
}

/// The classified outcome of one safe-commit attempt.
pub(super) enum CommitAttempt {
    /// The commit exists (including `SafeCommitLanded`, where only a
    /// post-landing index repair failed).
    Landed,
    /// A commit object may exist; keep every byte and report the truth.
    Indeterminate {
        /// The adapter failure.
        error: GitError,
    },
    /// Provably nothing landed; the caller may roll back its own uncommitted rows.
    NotLanded {
        /// The adapter failure.
        error: GitError,
    },
}

/// Plan and apply one path-scoped safe commit.
pub(super) fn commit_paths(
    authority: &CommitAuthority,
    root: &Path,
    paths: &[PathBuf],
    message: &str,
) -> CommitAttempt {
    let mut git_paths: Vec<GitPath> = Vec::with_capacity(paths.len());
    for path in paths {
        match normalize_safe_commit_path(root, path) {
            Ok(path) => git_paths.push(path),
            Err(error) => return CommitAttempt::NotLanded { error },
        }
    }
    if git_paths.is_empty() {
        // Nothing to commit (for example a flat mission with no review artifact):
        // report it as the safe-commit no-op rather than invent a commit.
        return CommitAttempt::NotLanded {
            error: GitError::SafeCommitEmptyChangeset,
        };
    }
    let request = SafeCommitRequest::new(authority.target.clone(), message, git_paths)
        .with_protection(authority.protected.clone(), CommitGuardCapability::Standard);
    let plan = match authority.repository.plan_safe_commit(&request) {
        Ok(plan) => plan,
        // Planning never mutates: the classification is definite.
        Err(error) => return CommitAttempt::NotLanded { error },
    };
    match authority.repository.apply_safe_commit(&plan) {
        Ok(_) => CommitAttempt::Landed,
        Err(error) => classify_apply_error(error),
    }
}

/// Classify a failure from the mutating half of safe commit.
fn classify_apply_error(error: GitError) -> CommitAttempt {
    match error {
        // The commit object exists; only a post-landing index repair failed.
        GitError::SafeCommitLanded { .. } => CommitAttempt::Landed,
        // Possibly landed: a candidate object may exist and the ref advance is
        // unconfirmed, or the failure happened while writing/reading state.
        GitError::SafeCommitAdvanceUnconfirmed { .. }
        | GitError::Timeout { .. }
        | GitError::Spawn { .. }
        | GitError::Io { .. }
        | GitError::CommandFailed { .. }
        | GitError::InvalidOutput { .. }
        | GitError::UnreadableHead { .. }
        | GitError::OutputTooLarge { .. }
        | GitError::GitUnavailable { .. }
        | GitError::RepositoryEnvironmentRefused { .. }
        | GitError::NotARepository { .. }
        | GitError::BareRepository { .. }
        | GitError::Unsupported { .. }
        | GitError::InvalidPlan { .. }
        | GitError::BlobTooLarge { .. }
        | GitError::InputTooLarge { .. }
        | GitError::RefMissing { .. }
        | GitError::MergeConclusionUnconfirmed(_) => CommitAttempt::Indeterminate { error },
        // Pre-commit refusals: no commit object can reference the requested
        // paths, so the caller's own uncommitted rows are safe to cut.
        GitError::SafeCommitEmptyChangeset
        | GitError::SafeCommitHookChangedPaths
        | GitError::NoOperationInProgress
        | GitError::SafeCommitHeadMismatch { .. }
        | GitError::SafeCommitDestinationMissing { .. }
        | GitError::SafeCommitDirectoryArgument { .. }
        | GitError::SafeCommitPathPolicy { .. }
        | GitError::SafeCommitPathOutsideWorktree { .. }
        | GitError::SafeCommitPathLoop { .. }
        | GitError::SafeCommitProtectedBranch { .. }
        | GitError::SafeCommitIndexDeletionConflict { .. }
        | GitError::SafeCommitInvalidRequest { .. }
        | GitError::SafeCommitNothingToCommit { .. }
        | GitError::SafeCommitStageFailed { .. }
        | GitError::SafeCommitCommitFailed { .. }
        | GitError::SafeCommitHookRejected { .. }
        | GitError::SafeCommitExpectedBytesMismatch { .. }
        | GitError::PlanStale { .. }
        | GitError::OperationInProgress { .. }
        | GitError::DirtyWorktree { .. }
        | GitError::BranchCheckedOut { .. } => CommitAttempt::NotLanded { error },
        // Any other adapter failure is treated conservatively: a commit object
        // may exist, so the caller must not roll anything back.
        other => CommitAttempt::Indeterminate { error: other },
    }
}

/// The status artifacts a transactional status write commits.
pub(super) fn status_artifact_paths(feature_dir: &Path) -> Vec<PathBuf> {
    [
        sk_status::snapshot::EVENTS_FILENAME,
        sk_status::snapshot::SNAPSHOT_FILENAME,
    ]
    .into_iter()
    .map(|name| feature_dir.join(name))
    .filter(|path| path.exists())
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn oid() -> sk_git::Oid {
        sk_git::Oid::parse(&"0".repeat(40)).expect("zero oid")
    }

    #[test]
    fn applied_failures_are_classified_conservatively() {
        // A candidate object may exist: never roll back committed state.
        assert!(matches!(
            classify_apply_error(GitError::SafeCommitAdvanceUnconfirmed {
                destination: "main".to_owned(),
                candidate: oid(),
                observed_tip: None,
                detail: "hook moved HEAD".to_owned(),
            }),
            CommitAttempt::Indeterminate { .. }
        ));
        assert!(matches!(
            classify_apply_error(GitError::Timeout {
                args: vec!["commit".to_owned()],
                cwd: PathBuf::from("/tmp"),
                timeout: Duration::from_secs(1),
            }),
            CommitAttempt::Indeterminate { .. }
        ));
        // Provably pre-commit refusals may roll back this operation's own rows.
        assert!(matches!(
            classify_apply_error(GitError::SafeCommitHookRejected {
                hook: "pre-commit".to_owned(),
                detail: "exit 1".to_owned(),
            }),
            CommitAttempt::NotLanded { .. }
        ));
        assert!(matches!(
            classify_apply_error(GitError::SafeCommitHeadMismatch {
                destination: "main".to_owned(),
                observed_head: "feature".to_owned(),
                worktree_root: PathBuf::from("/tmp"),
            }),
            CommitAttempt::NotLanded { .. }
        ));
        assert!(matches!(
            classify_apply_error(GitError::SafeCommitNothingToCommit {
                destination: "main".to_owned(),
                detail: "already matches HEAD".to_owned(),
            }),
            CommitAttempt::NotLanded { .. }
        ));
    }
}
