//! Concluding an in-progress merge, revert, or cherry-pick.
//!
//! A conclusion commits the whole index, so it cannot be path-scoped. This
//! module therefore runs exactly one argv — `git commit --no-edit` or
//! `git commit -m <message>` with `commit.gpgsign=false` — and refuses
//! everything that would change its scope or skip a hook:
//! `--no-verify`, `-a`, a pathspec, `--allow-empty`, and `--amend`.
//!
//! The operation, HEAD, and message are pinned in a plan. Execution re-detects
//! the operation and re-reads HEAD, refusing if either moved. A rejected hook
//! leaves the operation in progress; this crate does not abort it.

use crate::error::GitError;
use crate::oid::Oid;
use crate::refname::BranchName;
use crate::repo_state::InProgressOperation;
use crate::repository::GitRepository;

/// The operation a conclusion will commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConcludedOperation {
    /// A merge (`MERGE_HEAD`).
    Merge,
    /// A cherry-pick (`CHERRY_PICK_HEAD`).
    CherryPick,
    /// A revert (`REVERT_HEAD`).
    Revert,
}

impl ConcludedOperation {
    /// A stable machine-readable name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::CherryPick => "cherry-pick",
            Self::Revert => "revert",
        }
    }

    fn from_in_progress(operation: InProgressOperation) -> Option<Self> {
        match operation {
            InProgressOperation::Merge => Some(Self::Merge),
            InProgressOperation::CherryPick => Some(Self::CherryPick),
            InProgressOperation::Revert => Some(Self::Revert),
            _ => None,
        }
    }
}

/// A validated plan to conclude one in-progress operation.
#[derive(Debug, Clone)]
pub struct MergeConclusionPlan {
    operation: ConcludedOperation,
    expected_head: Oid,
    message: Option<String>,
}

impl MergeConclusionPlan {
    /// The operation that will be concluded.
    pub fn operation(&self) -> ConcludedOperation {
        self.operation
    }

    /// The HEAD the plan observed.
    pub fn expected_head(&self) -> &Oid {
        &self.expected_head
    }

    /// The explicit message, or `None` for `--no-edit`.
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }
}

/// The evidence left by a concluded operation.
#[derive(Debug, Clone)]
pub struct MergeConclusionOutcome {
    operation: ConcludedOperation,
    previous_head: Oid,
    new_head: Oid,
    branch: Option<BranchName>,
}

impl MergeConclusionOutcome {
    /// The concluded operation.
    pub fn operation(&self) -> ConcludedOperation {
        self.operation
    }

    /// HEAD before the commit.
    pub fn previous_head(&self) -> &Oid {
        &self.previous_head
    }

    /// HEAD after the commit.
    pub fn new_head(&self) -> &Oid {
        &self.new_head
    }

    /// The branch the conclusion committed on, when HEAD was not detached.
    pub fn branch(&self) -> Option<&BranchName> {
        self.branch.as_ref()
    }
}

impl GitRepository {
    /// Prepare a merge/revert/cherry-pick conclusion.
    ///
    /// Refuses when no concludeable operation is in progress, when a rebase,
    /// sequencer run, squash, or held index lock is present (each has different
    /// semantics this primitive does not claim), when HEAD moved from
    /// `expected_head`, or when an explicit message is empty or contains NUL.
    pub fn plan_merge_conclusion(
        &self,
        expected_head: &Oid,
        message: Option<&str>,
    ) -> Result<MergeConclusionPlan, GitError> {
        let operations = self.in_progress_operations()?;
        let mut concludeable = Vec::new();
        for operation in &operations {
            if let Some(concludeable_operation) = ConcludedOperation::from_in_progress(*operation) {
                concludeable.push(concludeable_operation);
            }
        }
        if concludeable.is_empty() {
            if operations.is_empty() {
                return Err(GitError::NoOperationInProgress);
            }
            return Err(GitError::OperationInProgress {
                operation: operations
                    .iter()
                    .map(|operation| operation.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            });
        }
        if concludeable.len() > 1 {
            return Err(GitError::OperationInProgress {
                operation: "multiple in-progress operations".to_string(),
            });
        }
        let operation = concludeable[0];
        let current_head = self.current_head()?;
        if current_head.as_ref() != Some(expected_head) {
            return Err(GitError::PlanStale {
                what: "HEAD".to_string(),
                detail: format!(
                    "expected {expected_head}, observed {}",
                    current_head
                        .as_ref()
                        .map(Oid::to_string)
                        .unwrap_or_else(|| "unborn".to_string())
                ),
            });
        }
        if let Some(message) = message {
            if message.trim().is_empty() {
                return Err(GitError::InvalidPlan {
                    detail: "an explicit conclusion message must not be empty".to_string(),
                });
            }
            if message.contains('\0') {
                return Err(GitError::InvalidPlan {
                    detail: "a conclusion message must not contain NUL".to_string(),
                });
            }
        }
        Ok(MergeConclusionPlan {
            operation,
            expected_head: expected_head.clone(),
            message: message.map(str::to_string),
        })
    }

    /// Commit the operation after proving it and HEAD are unchanged.
    ///
    /// The argv is fixed: no `-a`, no `--no-verify`, no pathspec, no
    /// `--allow-empty`, no `--amend`. A hook rejection is returned as a command
    /// failure and the operation stays in progress for the caller's abort path.
    pub fn apply_merge_conclusion(
        &self,
        plan: &MergeConclusionPlan,
    ) -> Result<MergeConclusionOutcome, GitError> {
        let operations = self.in_progress_operations()?;
        let detected: Vec<ConcludedOperation> = operations
            .iter()
            .filter_map(|operation| ConcludedOperation::from_in_progress(*operation))
            .collect();
        if detected.len() != 1 || detected[0] != plan.operation {
            return Err(GitError::PlanStale {
                what: "in-progress operation".to_string(),
                detail: format!(
                    "planned {}, observed {:?}",
                    plan.operation.as_str(),
                    operations
                        .iter()
                        .map(|operation| operation.as_str())
                        .collect::<Vec<_>>()
                ),
            });
        }
        let current_head = self.current_head()?;
        if current_head.as_ref() != Some(&plan.expected_head) {
            return Err(GitError::PlanStale {
                what: "HEAD".to_string(),
                detail: format!(
                    "planned {}, observed {}",
                    plan.expected_head,
                    current_head
                        .as_ref()
                        .map(Oid::to_string)
                        .unwrap_or_else(|| "unborn".to_string())
                ),
            });
        }

        let mut args: Vec<&str> = vec!["-c", "commit.gpgsign=false", "commit"];
        if let Some(message) = &plan.message {
            args.push("-m");
            args.push(message);
        } else {
            args.push("--no-edit");
        }
        self.process().checked(&args)?;

        let new_head = self
            .current_head()?
            .ok_or_else(|| GitError::InvalidOutput {
                detail: "HEAD is unborn after a conclusion committed".to_string(),
            })?;
        let branch = self
            .current_branch()?
            .and_then(|name| BranchName::parse(&name).ok());
        Ok(MergeConclusionOutcome {
            operation: plan.operation,
            previous_head: plan.expected_head.clone(),
            new_head,
            branch,
        })
    }

    /// HEAD as a validated object id, or `None` in an unborn repository.
    fn current_head(&self) -> Result<Option<Oid>, GitError> {
        match self.head_commit()? {
            Some(text) => Ok(Some(Oid::parse(&text)?)),
            None => Ok(None),
        }
    }
}
