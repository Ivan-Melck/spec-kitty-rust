//! Pre-write admission for the Op audit trail (#6564).
//!
//! Source: `specify_cli/invocation/executor.py` (`op_write_refusal`,
//! `require_op_write_admitted`). Admission consumes the same per-call
//! source-ordered target adjudication as the real close
//! ([`crate::op_target`]); it does not predict the close with a second order.
//!
//! 1. No judged branch (no work tree) admits: the source reports that after the
//!    write.
//! 2. The #6410 skip probe runs next. An ignored exact path, a primary branch
//!    that tracks no Op records, an unresolved primary, or an undecidable probe
//!    means the repository keeps its records locally; that admits.
//! 3. The target adjudication runs in source preflight order: shape, worktree
//!    relationship, full-HEAD assertion against J, `refs/heads/J` existence,
//!    the source path proof (6a/6b), then protection on J through the ONE
//!    canonical guard core.
//! 4. Only the source's own 6c guard answer refuses the write
//!    ([`OP_WRITE_PROTECTED_BRANCH`] naming J, or
//!    [`OP_WRITE_PROTECTION_CONFIG_INVALID`]). Every other source preflight
//!    failure is a `SafeCommitError` in the reference and therefore admits the
//!    audit write; the close still refuses the commit itself. A source-allowed
//!    shape the native publisher cannot prepare yet (unborn P, unrepresentable
//!    P) admits here as well, truthfully, and never becomes a source refusal.
//!
//! The adjudication is read-only: no `kitty-ops/` directory, index file, record,
//! reference, or worktree is created or changed. A protection refusal never
//! names the operator override. Branch, path, existence, and protection are
//! re-judged by [`OpCommitService::commit_op_record`] at the close; only the
//! primary-tracking answer shares the source's executor lifetime cache.
//!
//! # Limits
//!
//! The source's `_current_branch` also tries `git rev-parse --abbrev-ref HEAD`
//! when `symbolic-ref` answers nothing; the native adapter exposes the symbolic
//! answer. A git probe that cannot run at all is surfaced as an
//! [`OpAdmissionError`], not silently turned into admission. Native-only path
//! restrictions are not treated as source path refusals. No full executor,
//! CLI, or protected-main false-advance repair is claimed.

use std::path::Path;

use sk_git::{GitError, GitPath};
use thiserror::Error;

use crate::op_commit::{ADMISSION_PROBE_RECORD, OpCommitService, git_path_from_relative};
use crate::op_custody::{OpWriteAction, OpWriteRefusal, OpWriteRefused};
use crate::op_target::OpTargetVerdict;

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
        let judged = match self.open_repository() {
            Some(_) => match self.judged_destination() {
                Ok(judged) => judged,
                Err(error) => return Err(OpAdmissionError::Repository(error)),
            },
            None => {
                if let Some(reason) = self.probe_failure() {
                    return Err(OpAdmissionError::RepositoryProbe(reason.to_owned()));
                }
                return Ok(None);
            }
        };
        let Some(judged) = judged else {
            return Ok(None);
        };

        let sentinel = git_path_from_relative(Path::new(ADMISSION_PROBE_RECORD))
            .map_err(OpAdmissionError::Repository)?;
        if self.skip_reason(&sentinel).is_some() {
            return Ok(None);
        }

        // Source preflight order and the same builder the close uses. A source
        // path failure admits the audit write; a protected/invalid 6c verdict
        // refuses it; a native-only non-publishable shape admits because the
        // source would have written (and then committed) there.
        match self.judge_record_target(&judged, &sentinel) {
            OpTargetVerdict::SourceRefuses(_)
            | OpTargetVerdict::SourcePathRefuses(_)
            | OpTargetVerdict::SourceAllowedButNativeCannotPublish { .. }
            | OpTargetVerdict::Ready { .. } => Ok(None),
            OpTargetVerdict::Protected { judged } => {
                Ok(Some(OpWriteRefusal::protected_branch(&judged)))
            }
            OpTargetVerdict::InvalidConfig { detail } => {
                Ok(Some(OpWriteRefusal::protection_config_invalid(detail)))
            }
            OpTargetVerdict::Repository(error) => Err(OpAdmissionError::Repository(error)),
            OpTargetVerdict::Protection(error) => Err(OpAdmissionError::Protection(error)),
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
