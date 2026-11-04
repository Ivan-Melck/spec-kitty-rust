//! Pre-write admission for the Op audit trail (#6564).
//!
//! Source: `specify_cli/invocation/executor.py` (`op_write_refusal`,
//! `require_op_write_admitted`). The decision mirrors the auto-commit's own
//! order rather than predicting it:
//!
//! 1. No current branch (no work tree) admits: there is no branch whose guard
//!    could refuse the commit, and the source reports that after the write.
//! 2. The #6410 skip probe runs next. An ignored exact path, a primary branch
//!    that tracks no Op records, an unresolved primary, or an undecidable
//!    probe means the repository keeps its records locally; that admits.
//! 3. Only a branch that would actually be committed is preflighted, with the
//!    same message-free `SafeCommitTargetRequest` builder the real commit uses:
//!    project-root OR record-checkout `ProtectionPolicy`, `STANDARD`
//!    capability, the current record branch, and the exact ULID-shaped sentinel
//!    path. The preflight never inspects the payload (the reference's
//!    `preflight_commit` does not either), so no synthetic placeholder message
//!    is handed to the backend.
//! 4. `GitRepository::plan_safe_commit_target` is the canonical read-only
//!    preflight. `SafeCommitProtectedBranch` becomes
//!    [`OP_WRITE_PROTECTED_BRANCH`]; an unreadable/malformed protection
//!    configuration becomes [`OP_WRITE_PROTECTION_CONFIG_INVALID`]; every other
//!    preflight failure admits in this bridge. Known J/P and source path-policy
//!    reachability differences remain; native progress is not a source proof.
//!
//! The plan is read-only: no `kitty-ops/` directory, index file, record,
//! reference, or worktree is created or changed by admission. A protection
//! refusal never names the operator override. Branch and protection are
//! re-judged by [`OpCommitService::commit_op_record`] at the close; only the
//! primary-tracking answer shares the source's executor lifetime cache.
//!
//! # Limits
//!
//! The source's `_current_branch` also tries `git rev-parse --abbrev-ref
//! HEAD` when `symbolic-ref` answers nothing; the native adapter exposes the
//! symbolic answer. A git probe that cannot run at all is surfaced as an
//! [`OpAdmissionError`], not silently turned into admission. No full executor,
//! CLI, or protected-main false-advance repair is claimed.

use std::path::Path;

use sk_git::{BranchName, GitError, GitPath, ProtectedBranches};
use thiserror::Error;

use crate::op_commit::{
    ADMISSION_PROBE_RECORD, OpCommitService, ProtectionRead, build_safe_commit_target,
    git_path_from_relative,
};
use crate::op_custody::{OpWriteAction, OpWriteRefusal, OpWriteRefused};

/// Admission could not answer, or refused the write.
#[derive(Debug, Error)]
pub enum OpAdmissionError {
    /// The Op may not be written here; the operator-facing refusal is carried.
    #[error(transparent)]
    Refused(#[from] OpWriteRefused),

    /// A Git probe needed for the decision could not run.
    #[error(transparent)]
    Repository(#[from] GitError),

    /// A protection policy read failed for a reason other than a malformed
    /// configuration (a malformed configuration is an `OpWriteRefusal`).
    #[error(transparent)]
    Protection(#[from] sk_workspace::WorkspaceError),

    /// The repository could not even be opened (missing git, spawn/I/O).
    ///
    /// A plain non-repository, bare repository, or unreadable `HEAD` is not
    /// this error: it is the source's admitted "no current branch" state.
    #[error("git repository probe failed: {0}")]
    RepositoryProbe(String),
}

impl OpCommitService {
    /// Say why no Op record may be written here, before anything is written.
    ///
    /// Returns `None` when the write is admitted. The source treats every
    /// commit problem other than the guard's protected destination and an
    /// unreadable protection configuration as a post-write report, so those
    /// admitted cases are `Ok(None)` here.
    pub fn op_write_refusal(&mut self) -> Result<Option<OpWriteRefusal>, OpAdmissionError> {
        let branch = match self.open_repository() {
            Some(_) => match self.current_branch() {
                Ok(branch) => branch,
                Err(error) => return Err(OpAdmissionError::Repository(error)),
            },
            None => {
                if let Some(reason) = self.probe_failure() {
                    return Err(OpAdmissionError::RepositoryProbe(reason.to_owned()));
                }
                return Ok(None);
            }
        };
        let Some(branch) = branch else {
            return Ok(None);
        };

        let sentinel = git_path_from_relative(Path::new(ADMISSION_PROBE_RECORD))
            .map_err(OpAdmissionError::Repository)?;
        if self.skip_reason(&sentinel).is_some() {
            return Ok(None);
        }

        // The source preflight checks the worktree relationship and the
        // destination ref before it reads protection. Those failures are
        // `SafeCommitError`s there (logged and admitted), so they admit here
        // too, before a malformed protection block could be reported.
        if !self.roots_are_linked() || !self.has_destination_commit() {
            return Ok(None);
        }
        let Ok(destination) = BranchName::parse(&branch) else {
            // The source's preflight would fail on the bridge's representation
            // before reaching the guard; that is another admitted commit
            // problem, never an invented write refusal.
            return Ok(None);
        };

        let repository = self
            .open_repository()
            .expect("the branch was read from an open repository");
        let protected = match self.resolve_protected(&branch) {
            Ok(protected) => protected,
            Err(ProtectionRead::InvalidConfig(reason)) => {
                // The source reads protection at step 6c, after its HEAD,
                // destination and path checks. Re-run the same read-only target
                // plan with an empty set. This retains the earlier heuristic,
                // not a source-path proof: J/P and environmental failures still
                // need source-grounded reachability (see remaining F4 work).
                let probe =
                    build_safe_commit_target(&destination, &sentinel, ProtectedBranches::default());
                let reached = match repository.plan_safe_commit_target(&probe) {
                    Ok(_) => true,
                    // OperationInProgress is after the native guard, but the
                    // environment refusal is before it. Preserve the existing
                    // decision here without falsely claiming both prove source
                    // guard reachability; that heuristic remains required work.
                    Err(failure) => matches!(
                        failure.error(),
                        GitError::OperationInProgress { .. }
                            | GitError::RepositoryEnvironmentRefused { .. }
                    ),
                };
                return if reached {
                    Ok(Some(OpWriteRefusal::protection_config_invalid(reason)))
                } else {
                    Ok(None)
                };
            }
            Err(ProtectionRead::Unavailable(error)) => {
                return Err(OpAdmissionError::Protection(error));
            }
            Err(ProtectionRead::BranchName(error)) => {
                return Err(OpAdmissionError::Repository(error));
            }
        };
        let target = build_safe_commit_target(&destination, &sentinel, protected);
        match repository.plan_safe_commit_target(&target) {
            Ok(_) => Ok(None),
            Err(failure) => match failure.error() {
                GitError::SafeCommitProtectedBranch { destination, .. } => {
                    Ok(Some(OpWriteRefusal::protected_branch(destination)))
                }
                // Any other preflight failure is the source's "could not judge
                // the commit" branch: admitted, and observable after the write.
                _ => Ok(None),
            },
        }
    }

    /// Refuse the Op write when [`Self::op_write_refusal`] finds a refusal.
    ///
    /// The error carries the same operator-facing "Nothing was written" message
    /// and the record root, and never suggests overriding the guard.
    pub fn require_op_write_admitted(
        &mut self,
        action: OpWriteAction,
    ) -> Result<(), OpAdmissionError> {
        if let Some(refusal) = self.op_write_refusal()? {
            return Err(OpWriteRefused::new(refusal, self.record_root(), action).into());
        }
        Ok(())
    }
}

/// A convenience read of the admission sentinel, for callers/tests that need
/// the exact path the preflight judges.
pub fn admission_probe_path() -> GitPath {
    git_path_from_relative(Path::new(ADMISSION_PROBE_RECORD))
        .expect("the admission sentinel is a valid Git path")
}
