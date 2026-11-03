//! Starting and aborting a pinned-head merge.
//!
//! A dependency or planning integration sometimes needs a real three-way merge,
//! not a fast-forward. This module owns the *start* (leave a merge in progress)
//! and the *abort* (restore the pinned state); the conclusion already lives in
//! [`crate::MergeConclusionPlan`].
//!
//! # What is pinned
//!
//! A start plan records the observed `HEAD`, the checked-out branch, the other
//! commit, the merge base, and the operator-index authority. Applying it
//! re-reads all of them and refuses (`PlanStale`) if anything moved, and
//! refuses when any Git operation is in progress, when the merge tip is not a
//! commit, or when the checkout has a tracked modification. Untracked and
//! ignored files do not block a start: git itself refuses only when it would
//! have to overwrite one, and this adapter never deletes user files to make a
//! merge fit.
//!
//! The merge is run as `git merge --no-ff --no-commit`. It therefore stops
//! **before** committing, leaves `HEAD` exactly where the plan pinned it, and
//! writes the merge state (`MERGE_HEAD`, index, work tree). A clean result is
//! [`MergeStartStatus::Stopped`]; a conflict is
//! [`MergeStartStatus::Conflicted`] carrying the real stage-1/2/3 entries read
//! from the index, not a message scraped from git's stderr.
//!
//! # Abort safety
//!
//! An abort plan records `HEAD`, `MERGE_HEAD`, every unmerged path, and the
//! full status fingerprint. Applying it re-reads exactly those facts and
//! refuses if any changed — a resolved or edited merge is never silently
//! discarded. Only then does it run `git merge --abort` (which restores the
//! index and work tree through the captured operator index).

use crate::error::GitError;
use crate::index_authority::OperatorIndex;
use crate::objects::UnmergedPath;
use crate::oid::Oid;
use crate::refname::{BranchName, StartPoint};
use crate::repository::GitRepository;
use crate::status::{StatusEntry, StatusOptions, UntrackedMode};

/// Where a merge start stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MergeStartStatus {
    /// The other commit is already reachable from `HEAD`; nothing was written.
    AlreadyUpToDate,
    /// A merge is in progress with no conflicts; conclude or abort it.
    Stopped,
    /// A merge is in progress with conflicts; resolve, then conclude or abort.
    Conflicted,
}

impl MergeStartStatus {
    /// A stable machine-readable name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AlreadyUpToDate => "already_up_to_date",
            Self::Stopped => "stopped",
            Self::Conflicted => "conflicted",
        }
    }
}

/// A validated plan to start one merge.
#[derive(Debug, Clone)]
pub struct MergeStartPlan {
    head: Oid,
    branch: Option<BranchName>,
    theirs: Oid,
    merge_base: Option<Oid>,
    index_authority: OperatorIndex,
}

impl MergeStartPlan {
    /// The `HEAD` the plan pinned.
    pub fn head(&self) -> &Oid {
        &self.head
    }

    /// The branch checked out when the plan was prepared, if any.
    pub fn branch(&self) -> Option<&BranchName> {
        self.branch.as_ref()
    }

    /// The commit being merged in.
    pub fn theirs(&self) -> &Oid {
        &self.theirs
    }

    /// The merge base observed at plan time, when the histories are related.
    pub fn merge_base(&self) -> Option<&Oid> {
        self.merge_base.as_ref()
    }

    /// The operator index authority the merge will write through.
    pub fn index_authority(&self) -> &OperatorIndex {
        &self.index_authority
    }
}

/// The evidence left by a merge start.
#[derive(Debug, Clone)]
pub struct MergeStartOutcome {
    head: Oid,
    branch: Option<BranchName>,
    theirs: Oid,
    merge_base: Option<Oid>,
    status: MergeStartStatus,
    conflicts: Vec<UnmergedPath>,
    merge_head: Option<Oid>,
}

impl MergeStartOutcome {
    /// The `HEAD` after the start (unchanged from the plan by construction).
    pub fn head(&self) -> &Oid {
        &self.head
    }

    /// The branch checked out during the start.
    pub fn branch(&self) -> Option<&BranchName> {
        self.branch.as_ref()
    }

    /// The commit that was merged in.
    pub fn theirs(&self) -> &Oid {
        &self.theirs
    }

    /// The merge base observed at plan time.
    pub fn merge_base(&self) -> Option<&Oid> {
        self.merge_base.as_ref()
    }

    /// Where the start stopped.
    pub fn status(&self) -> MergeStartStatus {
        self.status
    }

    /// The unmerged conflict entries for a [`MergeStartStatus::Conflicted`]
    /// start.
    pub fn conflicts(&self) -> &[UnmergedPath] {
        &self.conflicts
    }

    /// The conflict paths, for a compact diagnostic.
    pub fn conflict_paths(&self) -> Vec<&crate::GitPath> {
        self.conflicts.iter().map(UnmergedPath::path).collect()
    }

    /// The `MERGE_HEAD` recorded by git, when the start left a merge state.
    pub fn merge_head(&self) -> Option<&Oid> {
        self.merge_head.as_ref()
    }

    /// Whether a merge is now in progress.
    pub fn in_progress(&self) -> bool {
        self.status != MergeStartStatus::AlreadyUpToDate
    }
}

/// A validated plan to abort one merge this adapter started or pinned.
#[derive(Debug, Clone)]
pub struct MergeAbortPlan {
    head: Oid,
    branch: Option<BranchName>,
    merge_head: Oid,
    index_authority: OperatorIndex,
    conflicts: Vec<UnmergedPath>,
    status: Vec<StatusEntry>,
}

impl MergeAbortPlan {
    /// The pinned `HEAD`.
    pub fn head(&self) -> &Oid {
        &self.head
    }

    /// The branch checked out when the plan was prepared.
    pub fn branch(&self) -> Option<&BranchName> {
        self.branch.as_ref()
    }

    /// The merge tip that will be discarded.
    pub fn merge_head(&self) -> &Oid {
        &self.merge_head
    }

    /// The conflict entries observed when the plan was prepared.
    pub fn conflicts(&self) -> &[UnmergedPath] {
        &self.conflicts
    }

    /// The operator index authority the abort will restore through.
    pub fn index_authority(&self) -> &OperatorIndex {
        &self.index_authority
    }
}

/// The evidence left by a successful abort.
#[derive(Debug, Clone)]
pub struct MergeAbortOutcome {
    restored_head: Oid,
    aborted_merge_head: Oid,
    branch: Option<BranchName>,
    discarded_conflicts: Vec<UnmergedPath>,
}

impl MergeAbortOutcome {
    /// The `HEAD` the abort restored (the pinned head).
    pub fn restored_head(&self) -> &Oid {
        &self.restored_head
    }

    /// The merge tip that was discarded.
    pub fn aborted_merge_head(&self) -> &Oid {
        &self.aborted_merge_head
    }

    /// The branch the merge was aborted on.
    pub fn branch(&self) -> Option<&BranchName> {
        self.branch.as_ref()
    }

    /// The conflict entries that were discarded (never silently thrown away
    /// without being reported).
    pub fn discarded_conflicts(&self) -> &[UnmergedPath] {
        &self.discarded_conflicts
    }
}

impl GitRepository {
    /// Prepare a merge of `theirs` into the current `HEAD`.
    ///
    /// Refuses an in-progress Git operation, an unborn HEAD, a start point that
    /// does not resolve to a commit, and a checkout with a tracked modification.
    /// Nothing is written.
    pub fn plan_merge_start(&self, theirs: &StartPoint) -> Result<MergeStartPlan, GitError> {
        self.require_no_merge_state()?;
        let head = self
            .head_commit()?
            .ok_or_else(|| GitError::RefMissing {
                name: "HEAD".to_string(),
            })
            .and_then(|text| Oid::parse(&text))?;
        let theirs = self.resolve_commit(theirs)?;
        self.require_merge_clean()?;
        let merge_base = self.merge_base(&head, &theirs)?;
        let branch = self
            .current_branch()?
            .map(|name| BranchName::parse(&name))
            .transpose()?;
        Ok(MergeStartPlan {
            head,
            branch,
            theirs,
            merge_base,
            index_authority: self.operator_index().clone(),
        })
    }

    /// Execute a prepared merge start.
    ///
    /// `HEAD`, the branch, the merge tip, and the tracked-clean state are
    /// re-read first. The merge is always run with `--no-ff --no-commit`, so
    /// `HEAD` cannot move; if it does anyway, the result is
    /// [`GitError::MergeStateChanged`] rather than a claim that nothing
    /// happened. `merge.autoStash` is forced off for this invocation: a stash
    /// would mutate operator state outside the plan.
    pub fn apply_merge_start(&self, plan: &MergeStartPlan) -> Result<MergeStartOutcome, GitError> {
        if self.operator_index() != plan.index_authority() {
            return Err(GitError::PlanStale {
                what: "operator index".to_string(),
                detail: format!(
                    "planned {:?}, observed {:?}",
                    plan.index_authority().explicit_path(),
                    self.operator_index().explicit_path()
                ),
            });
        }
        self.require_no_merge_state()?;
        let head = self
            .head_commit()?
            .ok_or_else(|| GitError::RefMissing {
                name: "HEAD".to_string(),
            })
            .and_then(|text| Oid::parse(&text))?;
        if &head != plan.head() {
            return Err(GitError::PlanStale {
                what: "HEAD".to_string(),
                detail: format!("planned {}, observed {head}", plan.head()),
            });
        }
        let branch = self
            .current_branch()?
            .map(|name| BranchName::parse(&name))
            .transpose()?;
        if branch != plan.branch {
            return Err(GitError::PlanStale {
                what: "checked-out branch".to_string(),
                detail: format!("planned {:?}, observed {branch:?}", plan.branch),
            });
        }
        self.require_merge_clean()?;
        if !self.commit_exists(plan.theirs())? {
            return Err(GitError::RefMissing {
                name: plan.theirs().to_string(),
            });
        }

        let output = self.process().run(&[
            "-c",
            "merge.autoStash=false",
            "merge",
            "--no-ff",
            "--no-commit",
            plan.theirs().as_str(),
        ])?;
        let merge_head = self.merge_head()?;
        let conflicts = self.unmerged_paths()?;

        let status = match (output.status.code(), &merge_head) {
            (Some(0), None) => MergeStartStatus::AlreadyUpToDate,
            (Some(0), Some(_)) => {
                if conflicts.is_empty() {
                    MergeStartStatus::Stopped
                } else {
                    MergeStartStatus::Conflicted
                }
            }
            (Some(1), Some(recorded)) => {
                if recorded == plan.theirs() {
                    MergeStartStatus::Conflicted
                } else {
                    return Err(GitError::MergeStateChanged {
                        worktree_root: self.root().to_path_buf(),
                        detail: format!(
                            "git merge exited 1 with MERGE_HEAD {recorded}, expected {}",
                            plan.theirs()
                        ),
                    });
                }
            }
            (code, None) => {
                let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
                return Err(GitError::CommandFailed {
                    args: vec![
                        "merge".to_string(),
                        "--no-ff".to_string(),
                        "--no-commit".to_string(),
                        plan.theirs().to_string(),
                    ],
                    cwd: self.root().to_path_buf(),
                    status: code,
                    stderr: if detail.is_empty() {
                        "git merge failed without a diagnostic".to_string()
                    } else {
                        detail
                    },
                });
            }
            (_, Some(recorded)) => {
                return Err(GitError::MergeStateChanged {
                    worktree_root: self.root().to_path_buf(),
                    detail: format!(
                        "git merge exited {:?} but left MERGE_HEAD {recorded}; inspect the checkout",
                        output.status.code()
                    ),
                });
            }
        };

        if let Some(recorded) = &merge_head
            && recorded != plan.theirs()
        {
            return Err(GitError::MergeStateChanged {
                worktree_root: self.root().to_path_buf(),
                detail: format!("MERGE_HEAD {recorded} is not the planned {}", plan.theirs()),
            });
        }
        let after = self
            .head_commit()?
            .ok_or_else(|| GitError::RefMissing {
                name: "HEAD".to_string(),
            })
            .and_then(|text| Oid::parse(&text))?;
        if &after != plan.head() {
            return Err(GitError::MergeStateChanged {
                worktree_root: self.root().to_path_buf(),
                detail: format!(
                    "HEAD moved from {} to {after} during a --no-commit merge",
                    plan.head()
                ),
            });
        }

        Ok(MergeStartOutcome {
            head: plan.head.clone(),
            branch: plan.branch.clone(),
            theirs: plan.theirs.clone(),
            merge_base: plan.merge_base.clone(),
            status,
            conflicts,
            merge_head,
        })
    }

    /// Plan and start one merge.
    pub fn merge_start(&self, theirs: &StartPoint) -> Result<MergeStartOutcome, GitError> {
        let plan = self.plan_merge_start(theirs)?;
        self.apply_merge_start(&plan)
    }

    /// Prepare an abort of the merge currently in progress.
    ///
    /// Refuses when no merge (or a different operation) is in progress. The
    /// returned plan pins `HEAD`, `MERGE_HEAD`, the conflicts, and the complete
    /// status fingerprint.
    pub fn plan_merge_abort(&self) -> Result<MergeAbortPlan, GitError> {
        let operations = self.in_progress_operations()?;
        if operations.is_empty() {
            return Err(GitError::NoOperationInProgress);
        }
        let only_merge =
            operations.len() == 1 && operations[0] == crate::InProgressOperation::Merge;
        if !only_merge {
            return Err(GitError::OperationInProgress {
                operation: operations
                    .iter()
                    .map(|operation| operation.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            });
        }
        let merge_head = self.merge_head()?.ok_or_else(|| GitError::InvalidOutput {
            detail: "MERGE_HEAD is absent while a merge marker is present".to_string(),
        })?;
        let head = self
            .head_commit()?
            .ok_or_else(|| GitError::RefMissing {
                name: "HEAD".to_string(),
            })
            .and_then(|text| Oid::parse(&text))?;
        let branch = self
            .current_branch()?
            .map(|name| BranchName::parse(&name))
            .transpose()?;
        Ok(MergeAbortPlan {
            head,
            branch,
            merge_head,
            index_authority: self.operator_index().clone(),
            conflicts: self.unmerged_paths()?,
            status: self.merge_status()?,
        })
    }

    /// Execute a prepared abort after proving nothing changed since the plan.
    ///
    /// A resolved, edited, or newly-modified merge is refused rather than
    /// discarded. A successful abort restores `HEAD` to the pinned commit and
    /// leaves no merge state behind.
    pub fn apply_merge_abort(&self, plan: &MergeAbortPlan) -> Result<MergeAbortOutcome, GitError> {
        if self.operator_index() != plan.index_authority() {
            return Err(GitError::PlanStale {
                what: "operator index".to_string(),
                detail: format!(
                    "planned {:?}, observed {:?}",
                    plan.index_authority().explicit_path(),
                    self.operator_index().explicit_path()
                ),
            });
        }
        let operations = self.in_progress_operations()?;
        if operations.len() != 1 || operations[0] != crate::InProgressOperation::Merge {
            return Err(GitError::PlanStale {
                what: "in-progress operation".to_string(),
                detail: format!(
                    "planned merge, observed {:?}",
                    operations
                        .iter()
                        .map(|operation| operation.as_str())
                        .collect::<Vec<_>>()
                ),
            });
        }
        let merge_head = self.merge_head()?.ok_or_else(|| GitError::InvalidOutput {
            detail: "MERGE_HEAD disappeared after the abort plan".to_string(),
        })?;
        if &merge_head != plan.merge_head() {
            return Err(GitError::PlanStale {
                what: "MERGE_HEAD".to_string(),
                detail: format!("planned {}, observed {merge_head}", plan.merge_head()),
            });
        }
        let head = self
            .head_commit()?
            .ok_or_else(|| GitError::RefMissing {
                name: "HEAD".to_string(),
            })
            .and_then(|text| Oid::parse(&text))?;
        if &head != plan.head() {
            return Err(GitError::PlanStale {
                what: "HEAD".to_string(),
                detail: format!("planned {}, observed {head}", plan.head()),
            });
        }
        let branch = self
            .current_branch()?
            .map(|name| BranchName::parse(&name))
            .transpose()?;
        if branch != plan.branch {
            return Err(GitError::PlanStale {
                what: "checked-out branch".to_string(),
                detail: format!("planned {:?}, observed {branch:?}", plan.branch),
            });
        }
        let status = self.merge_status()?;
        if status != plan.status {
            return Err(GitError::PlanStale {
                what: "merge status".to_string(),
                detail: format!(
                    "planned {:?} entries, observed {:?}; resolve, conclude, or reset the merge deliberately",
                    plan.status
                        .iter()
                        .map(StatusEntry::display_lossy)
                        .collect::<Vec<_>>(),
                    status
                        .iter()
                        .map(StatusEntry::display_lossy)
                        .collect::<Vec<_>>()
                ),
            });
        }

        let output = self.process().run(&["merge", "--abort"])?;
        if !output.status.success() {
            return Err(GitError::MergeAbortFailed {
                worktree_root: self.root().to_path_buf(),
                detail: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            });
        }

        if self.merge_head()?.is_some() || self.is_operation_in_progress()? {
            return Err(GitError::MergeStateChanged {
                worktree_root: self.root().to_path_buf(),
                detail: "git merge --abort reported success but merge state remains".to_string(),
            });
        }
        let restored = self
            .head_commit()?
            .ok_or_else(|| GitError::RefMissing {
                name: "HEAD".to_string(),
            })
            .and_then(|text| Oid::parse(&text))?;
        if &restored != plan.head() {
            return Err(GitError::MergeStateChanged {
                worktree_root: self.root().to_path_buf(),
                detail: format!(
                    "git merge --abort restored HEAD {restored}, planned {}",
                    plan.head()
                ),
            });
        }
        Ok(MergeAbortOutcome {
            restored_head: restored,
            aborted_merge_head: plan.merge_head.clone(),
            branch: plan.branch.clone(),
            discarded_conflicts: plan.conflicts.clone(),
        })
    }

    /// Plan and abort the merge currently in progress.
    pub fn merge_abort(&self) -> Result<MergeAbortOutcome, GitError> {
        let plan = self.plan_merge_abort()?;
        self.apply_merge_abort(&plan)
    }

    fn require_no_merge_state(&self) -> Result<(), GitError> {
        let operations = self.in_progress_operations()?;
        if operations.is_empty() {
            return Ok(());
        }
        Err(GitError::OperationInProgress {
            operation: operations
                .iter()
                .map(|operation| operation.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        })
    }

    fn require_merge_clean(&self) -> Result<(), GitError> {
        let entries = self.status_with(&StatusOptions {
            untracked: UntrackedMode::No,
            ignored: false,
        })?;
        if entries.is_empty() {
            return Ok(());
        }
        Err(GitError::DirtyWorktree {
            path: self.root().to_path_buf(),
            entries: entries.iter().map(StatusEntry::display_lossy).collect(),
        })
    }

    fn merge_status(&self) -> Result<Vec<StatusEntry>, GitError> {
        self.status_with(&StatusOptions {
            untracked: UntrackedMode::All,
            ignored: false,
        })
    }
}
