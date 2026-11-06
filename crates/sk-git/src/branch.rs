//! Branch lifecycle primitives: create, advance, and guarded delete.
//!
//! Every operation is prepared as a value first. Planning resolves and records
//! the state it observed; execution re-reads that state and refuses with
//! [`GitError::PlanStale`] if anything moved. Nothing here is a general "run
//! git branch" surface, and nothing contacts a remote.
//!
//! `ProtectedBranches` is a **caller-supplied fact**. Resolving
//! `.kittify/config.yaml`, the remote default branch, or a Mission's
//! `commit_to_target` stays in the application layer; this crate only refuses
//! a protected name unless the plan carries an explicit reason for the
//! override.

use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::error::GitError;
use crate::oid::Oid;
use crate::refname::{BranchName, RefName, StartPoint};
use crate::refs::{RefChange, RefState, RefTransactionOutcome, validate_message};
use crate::repository::GitRepository;
use crate::status::{StatusOptions, UntrackedMode};

/// Namespace reserved for the tool's own bookkeeping refs.
pub const BOOKKEEPING_REF_PREFIX: &str = "refs/spec-kitty/";

/// A validated set of branch names the application layer resolved as protected.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProtectedBranches {
    names: BTreeSet<BranchName>,
}

impl ProtectedBranches {
    /// Build a set from already-validated branch names.
    pub fn new(names: impl IntoIterator<Item = BranchName>) -> Self {
        Self {
            names: names.into_iter().collect(),
        }
    }

    /// Parse a list of short branch names.
    pub fn parse(names: impl IntoIterator<Item = impl AsRef<str>>) -> Result<Self, GitError> {
        let mut set = BTreeSet::new();
        for name in names {
            set.insert(BranchName::parse(name.as_ref())?);
        }
        Ok(Self { names: set })
    }

    /// Whether the set is empty.
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// The branch names, sorted.
    pub fn iter(&self) -> impl Iterator<Item = &BranchName> {
        self.names.iter()
    }

    /// Whether `branch` is protected.
    pub fn contains(&self, branch: &BranchName) -> bool {
        self.names.contains(branch)
    }
}

/// A branch creation prepared from an observed "absent" state.
#[derive(Debug, Clone)]
pub struct BranchCreationPlan {
    branch: BranchName,
    start: StartPoint,
    start_oid: Oid,
    reflog_message: String,
}

impl BranchCreationPlan {
    /// The branch that will be created.
    pub fn branch(&self) -> &BranchName {
        &self.branch
    }

    /// The start point the caller named.
    pub fn start(&self) -> &StartPoint {
        &self.start
    }

    /// The commit the start point resolved to when the plan was prepared.
    pub fn start_oid(&self) -> &Oid {
        &self.start_oid
    }
}

/// A fast-forward or explicit non-fast-forward branch advance.
#[derive(Debug, Clone)]
pub struct BranchAdvancePlan {
    branch: BranchName,
    new: Oid,
    previous: Option<Oid>,
    checkouts: Vec<PathBuf>,
    resync: bool,
    protected_override: Option<String>,
    reflog_message: String,
}

impl BranchAdvancePlan {
    /// The branch that will move.
    pub fn branch(&self) -> &BranchName {
        &self.branch
    }

    /// The tip it will move to.
    pub fn new_tip(&self) -> &Oid {
        &self.new
    }

    /// The tip observed at plan time (`None` for an unborn branch).
    pub fn previous_tip(&self) -> Option<&Oid> {
        self.previous.as_ref()
    }

    /// The worktrees that have the branch checked out.
    pub fn checkouts(&self) -> &[PathBuf] {
        &self.checkouts
    }

    /// Whether checkouts will be hard-reset to the new tip after the ref moves.
    pub fn resyncs_checkouts(&self) -> bool {
        self.resync
    }

    /// The explicit protected-target override reason, when one was carried.
    pub fn protected_override(&self) -> Option<&str> {
        self.protected_override.as_deref()
    }
}

/// Options for planning a branch advance.
#[derive(Debug, Clone, Default)]
pub struct BranchAdvanceOptions {
    resync_clean_checkouts: bool,
    allow_non_fast_forward: bool,
    protected_override: Option<String>,
}

impl BranchAdvanceOptions {
    /// Permit the advance to hard-reset checkouts that are completely clean.
    ///
    /// Without this, a branch checked out anywhere is refused, because moving
    /// its ref would leave that checkout behind its own HEAD. The reset is the
    /// only destructive step; every checkout is rescanned immediately before
    /// the ref moves and a single entry refuses the whole plan.
    pub fn reset_clean_checkouts(mut self) -> Self {
        self.resync_clean_checkouts = true;
        self
    }

    /// Permit a backwards or sideways move.
    ///
    /// This is for an explicit rollback, not for normal advancement. The
    /// compare-and-swap still applies, so a concurrent move refuses.
    pub fn allow_non_fast_forward(mut self) -> Self {
        self.allow_non_fast_forward = true;
        self
    }

    /// Carry an explicit, auditable reason for advancing a protected branch.
    ///
    /// Without this, [`ProtectedBranches::contains`] refuses the plan. The
    /// reason is required to be non-empty and is retained on the plan.
    pub fn allow_protected_target(mut self, reason: impl Into<String>) -> Self {
        self.protected_override = Some(reason.into());
        self
    }
}

/// A deletion preflight: all the facts a caller needs before approving a delete.
#[derive(Debug, Clone)]
pub struct BranchDeletionPreflight {
    branch: BranchName,
    tip: Oid,
    checked_out_in: Vec<PathBuf>,
    unique_commits: Vec<Oid>,
    protected: bool,
}

impl BranchDeletionPreflight {
    /// The branch that would be deleted.
    pub fn branch(&self) -> &BranchName {
        &self.branch
    }

    /// Its tip at preflight time.
    pub fn tip(&self) -> &Oid {
        &self.tip
    }

    /// Every worktree that has this branch checked out.
    pub fn checked_out_in(&self) -> &[PathBuf] {
        &self.checked_out_in
    }

    /// Commits reachable from this branch and from no other ref (or the
    /// supplied creation base).
    pub fn unique_commits(&self) -> &[Oid] {
        &self.unique_commits
    }

    /// Whether the caller's protection facts include this branch.
    pub fn is_protected(&self) -> bool {
        self.protected
    }

    /// Whether a guarded delete would be allowed with no override.
    pub fn is_guarded_safe(&self) -> bool {
        self.checked_out_in.is_empty() && self.unique_commits.is_empty() && !self.protected
    }
}

/// Options that make a delete's destructive intent explicit.
#[derive(Debug, Clone, Default)]
pub struct BranchDeletionOptions {
    allow_unique_commits: Option<String>,
    allow_protected_target: Option<String>,
    preserve_tip_at: Option<RefName>,
}

impl BranchDeletionOptions {
    /// Permit deleting a branch whose commits exist nowhere else.
    ///
    /// The reason must be non-empty and is retained for the audit trail. A
    /// `preserve_tip_at` ref is the recommended companion.
    pub fn allow_unique_commits(mut self, reason: impl Into<String>) -> Self {
        self.allow_unique_commits = Some(reason.into());
        self
    }

    /// Permit deleting a caller-supplied protected branch, with a reason.
    pub fn allow_protected_target(mut self, reason: impl Into<String>) -> Self {
        self.allow_protected_target = Some(reason.into());
        self
    }

    /// Preserve the deleted tip under a `refs/spec-kitty/` bookkeeping ref.
    ///
    /// The ref is created with compare-and-swap "must not exist" semantics, so
    /// an earlier record is never overwritten. If the delete then fails, the
    /// preserved ref remains as recovery evidence.
    pub fn preserve_tip_at(mut self, reference: RefName) -> Self {
        self.preserve_tip_at = Some(reference);
        self
    }
}

/// A validated, approved branch deletion.
#[derive(Debug, Clone)]
pub struct BranchDeletionPlan {
    preflight: BranchDeletionPreflight,
    creation_base: Option<Oid>,
    preserve_tip_at: Option<RefName>,
    allow_unique_commits: Option<String>,
    allow_protected_target: Option<String>,
    reflog_message: String,
}

impl BranchDeletionPlan {
    /// The preflight facts this plan was approved from.
    pub fn preflight(&self) -> &BranchDeletionPreflight {
        &self.preflight
    }

    /// The creation base used to decide which commits are unique, if any.
    pub fn creation_base(&self) -> Option<&Oid> {
        self.creation_base.as_ref()
    }

    /// The bookkeeping ref that will retain the tip, when requested.
    pub fn preserve_tip_at(&self) -> Option<&RefName> {
        self.preserve_tip_at.as_ref()
    }

    /// The explicit unique-commit override reason, when one was carried.
    pub fn unique_commit_override(&self) -> Option<&str> {
        self.allow_unique_commits.as_deref()
    }

    /// The explicit protected-target override reason, when one was carried.
    pub fn protected_override(&self) -> Option<&str> {
        self.allow_protected_target.as_deref()
    }
}

/// The evidence left by a branch deletion.
#[derive(Debug, Clone)]
pub struct BranchDeletionOutcome {
    branch: BranchName,
    tip: Oid,
    preserved: Option<RefState>,
}

impl BranchDeletionOutcome {
    /// The deleted branch name.
    pub fn branch(&self) -> &BranchName {
        &self.branch
    }

    /// The tip it held when it was deleted.
    pub fn tip(&self) -> &Oid {
        &self.tip
    }

    /// The bookkeeping ref that retains the tip, when one was written.
    pub fn preserved(&self) -> Option<&RefState> {
        self.preserved.as_ref()
    }
}

/// The evidence left by a branch advance.
#[derive(Debug, Clone)]
pub struct BranchAdvanceOutcome {
    branch: BranchName,
    previous: Option<Oid>,
    new: Oid,
    ref_transaction: RefTransactionOutcome,
    resynced: Vec<PathBuf>,
}

impl BranchAdvanceOutcome {
    /// The advanced branch.
    pub fn branch(&self) -> &BranchName {
        &self.branch
    }

    /// The tip before the move.
    pub fn previous(&self) -> Option<&Oid> {
        self.previous.as_ref()
    }

    /// The tip after the move.
    pub fn new_tip(&self) -> &Oid {
        &self.new
    }

    /// The ref transaction's own evidence.
    pub fn ref_transaction(&self) -> &RefTransactionOutcome {
        &self.ref_transaction
    }

    /// The checkouts that were hard-reset to the new tip.
    pub fn resynced(&self) -> &[PathBuf] {
        &self.resynced
    }
}

impl GitRepository {
    /// Prepare a branch creation from a validated start point.
    ///
    /// Refuses with [`GitError::BranchAlreadyExists`] when the branch exists.
    /// `HEAD` is resolved now and pinned, so a later checkout cannot change the
    /// commit the plan will create.
    pub fn plan_branch_creation(
        &self,
        branch: &BranchName,
        start: StartPoint,
        reflog_message: &str,
    ) -> Result<BranchCreationPlan, GitError> {
        validate_message(reflog_message)?;
        if let Some(tip) = self.read_branch_tip(branch)? {
            return Err(GitError::BranchAlreadyExists {
                branch: branch.as_str().to_string(),
                tip: tip.to_string(),
            });
        }
        let start_oid = self.resolve_commit(&start)?;
        Ok(BranchCreationPlan {
            branch: branch.clone(),
            start,
            start_oid,
            reflog_message: reflog_message.to_string(),
        })
    }

    /// Execute a prepared branch creation as a compare-and-swap create.
    pub fn apply_branch_creation(
        &self,
        plan: &BranchCreationPlan,
    ) -> Result<RefTransactionOutcome, GitError> {
        if let Some(tip) = self.read_branch_tip(&plan.branch)? {
            return Err(GitError::PlanStale {
                what: plan.branch.as_ref_name().to_string(),
                detail: format!("planned absent, observed {tip}"),
            });
        }
        let transaction = self.plan_ref_transaction(
            &plan.reflog_message,
            vec![RefChange::create(
                plan.branch.as_ref_name(),
                plan.start_oid.clone(),
            )],
        )?;
        self.apply_ref_transaction(&transaction)
    }

    /// Prepare a branch advance with explicit protection and checkout policy.
    ///
    /// Verifies: the current tip equals `expected_old` (absent when `None`),
    /// the new commit exists, the move is a fast-forward unless explicitly
    /// allowed, the branch is not protected unless an override reason is
    /// carried, and every checkout is clean when resync is requested.
    pub fn plan_branch_advance(
        &self,
        branch: &BranchName,
        new: Oid,
        expected_old: Option<Oid>,
        protected: &ProtectedBranches,
        reflog_message: &str,
        options: &BranchAdvanceOptions,
    ) -> Result<BranchAdvancePlan, GitError> {
        validate_message(reflog_message)?;
        let previous = self.read_branch_tip(branch)?;
        if previous != expected_old {
            return Err(GitError::PlanStale {
                what: branch.as_ref_name().to_string(),
                detail: format!(
                    "expected {}, observed {}",
                    describe_optional(expected_old.as_ref()),
                    describe_optional(previous.as_ref())
                ),
            });
        }
        if !self.commit_exists(&new)? {
            return Err(GitError::RefMissing {
                name: new.to_string(),
            });
        }
        if protected.contains(branch) {
            match options.protected_override.as_deref() {
                Some(reason) if !reason.trim().is_empty() => {}
                _ => {
                    return Err(GitError::ProtectedTarget {
                        branch: branch.as_str().to_string(),
                    });
                }
            }
        }
        if let Some(previous) = &previous
            && !options.allow_non_fast_forward
            && !self.is_ancestor(previous, &new)?
        {
            return Err(GitError::NonFastForward {
                branch: branch.as_str().to_string(),
                old: previous.to_string(),
                new: new.to_string(),
            });
        }

        let checkouts = self.checkouts_of(branch)?;
        if !checkouts.is_empty() && !options.resync_clean_checkouts {
            return Err(GitError::BranchCheckedOut {
                branch: branch.as_str().to_string(),
                worktrees: checkouts,
            });
        }
        if options.resync_clean_checkouts {
            for checkout in &checkouts {
                self.require_clean_checkout(checkout)?;
            }
        }

        Ok(BranchAdvancePlan {
            branch: branch.clone(),
            new,
            previous,
            checkouts,
            resync: options.resync_clean_checkouts,
            protected_override: options.protected_override.clone(),
            reflog_message: reflog_message.to_string(),
        })
    }

    /// Execute a prepared advance: compare-and-swap the ref, then resync the
    /// checkouts that were proven clean.
    ///
    /// If the ref moves but a reset fails, the error is
    /// [`GitError::ResyncFailed`]: the ref now holds the new tip and the named
    /// checkout is behind it. That is recovery evidence, not a silent success.
    pub fn apply_branch_advance(
        &self,
        plan: &BranchAdvancePlan,
    ) -> Result<BranchAdvanceOutcome, GitError> {
        let current = self.read_branch_tip(&plan.branch)?;
        if current != plan.previous {
            return Err(GitError::PlanStale {
                what: plan.branch.as_ref_name().to_string(),
                detail: format!(
                    "planned {}, observed {}",
                    describe_optional(plan.previous.as_ref()),
                    describe_optional(current.as_ref())
                ),
            });
        }
        let checkouts = self.checkouts_of(&plan.branch)?;
        if sorted_paths(&checkouts) != sorted_paths(&plan.checkouts) {
            return Err(GitError::PlanStale {
                what: plan.branch.as_ref_name().to_string(),
                detail: format!(
                    "checkouts changed from {:?} to {:?}",
                    plan.checkouts, checkouts
                ),
            });
        }
        if plan.resync {
            // Re-scan immediately before the ref moves: a checkout that became
            // dirty after planning must refuse the whole advance.
            for checkout in &checkouts {
                self.require_clean_checkout(checkout)?;
            }
        }

        let transaction = self.plan_ref_transaction(
            &plan.reflog_message,
            vec![RefChange::set(
                plan.branch.as_ref_name(),
                plan.new.clone(),
                plan.previous.clone(),
            )],
        )?;
        let ref_transaction = self.apply_ref_transaction(&transaction)?;

        let mut resynced = Vec::new();
        if plan.resync {
            for checkout in &checkouts {
                let output = self.process_at(checkout.clone()).run(&[
                    "reset",
                    "--hard",
                    plan.new.as_str(),
                ])?;
                if !output.status.success() {
                    return Err(GitError::ResyncFailed {
                        checkout: checkout.clone(),
                        detail: String::from_utf8_lossy(&output.stderr).trim().to_string(),
                    });
                }
                resynced.push(checkout.clone());
            }
        }

        Ok(BranchAdvanceOutcome {
            branch: plan.branch.clone(),
            previous: plan.previous.clone(),
            new: plan.new.clone(),
            ref_transaction,
            resynced,
        })
    }

    /// Read every fact a deletion decision depends on, without mutating.
    pub fn preflight_branch_deletion(
        &self,
        branch: &BranchName,
        protected: &ProtectedBranches,
        creation_base: Option<&Oid>,
    ) -> Result<BranchDeletionPreflight, GitError> {
        let Some(tip) = self.read_branch_tip(branch)? else {
            return Err(GitError::BranchMissing {
                branch: branch.as_str().to_string(),
            });
        };
        let checked_out_in = self.checkouts_of(branch)?;
        let unique_commits = self.unique_commits(branch, &[], creation_base)?;
        Ok(BranchDeletionPreflight {
            branch: branch.clone(),
            tip,
            checked_out_in,
            unique_commits,
            protected: protected.contains(branch),
        })
    }

    /// Approve a deletion plan after checking the caller's explicit overrides.
    ///
    /// A branch checked out in any worktree is **always** refused. Unique
    /// commits and a protected target each require an explicit, non-empty
    /// reason in [`BranchDeletionOptions`].
    pub fn plan_branch_deletion(
        &self,
        branch: &BranchName,
        protected: &ProtectedBranches,
        creation_base: Option<&Oid>,
        options: BranchDeletionOptions,
        reflog_message: &str,
    ) -> Result<BranchDeletionPlan, GitError> {
        validate_message(reflog_message)?;
        let preflight = self.preflight_branch_deletion(branch, protected, creation_base)?;
        if !preflight.checked_out_in.is_empty() {
            return Err(GitError::BranchCheckedOut {
                branch: branch.as_str().to_string(),
                worktrees: preflight.checked_out_in.clone(),
            });
        }
        if preflight.protected {
            match options.allow_protected_target.as_deref() {
                Some(reason) if !reason.trim().is_empty() => {}
                _ => {
                    return Err(GitError::ProtectedTarget {
                        branch: branch.as_str().to_string(),
                    });
                }
            }
        }
        if !preflight.unique_commits.is_empty() {
            match options.allow_unique_commits.as_deref() {
                Some(reason) if !reason.trim().is_empty() => {}
                _ => {
                    return Err(GitError::BranchHasUniqueCommits {
                        branch: branch.as_str().to_string(),
                        commits: preflight
                            .unique_commits
                            .iter()
                            .map(Oid::to_string)
                            .collect(),
                    });
                }
            }
        }
        if let Some(reference) = &options.preserve_tip_at
            && !reference.is_under(BOOKKEEPING_REF_PREFIX)
        {
            return Err(GitError::UnsafePath {
                path: PathBuf::from(reference.as_str()),
                detail: format!(
                    "a preserved tip must be a {BOOKKEEPING_REF_PREFIX} bookkeeping ref, not a branch or other namespace"
                ),
            });
        }
        Ok(BranchDeletionPlan {
            preflight,
            creation_base: creation_base.cloned(),
            preserve_tip_at: options.preserve_tip_at,
            allow_unique_commits: options.allow_unique_commits,
            allow_protected_target: options.allow_protected_target,
            reflog_message: reflog_message.to_string(),
        })
    }

    /// Execute a prepared deletion with compare-and-swap evidence.
    ///
    /// Optionally creates the preservation ref first. The branch delete is a
    /// CAS delete carrying the planned tip, so a commit that landed after
    /// approval is never made unreachable.
    pub fn apply_branch_deletion(
        &self,
        plan: &BranchDeletionPlan,
    ) -> Result<BranchDeletionOutcome, GitError> {
        let preflight = &plan.preflight;
        let current = self.read_branch_tip(&preflight.branch)?;
        if current.as_ref() != Some(&preflight.tip) {
            return Err(GitError::PlanStale {
                what: preflight.branch.as_ref_name().to_string(),
                detail: format!(
                    "planned {}, observed {}",
                    preflight.tip,
                    describe_optional(current.as_ref())
                ),
            });
        }
        let checkouts = self.checkouts_of(&preflight.branch)?;
        if !checkouts.is_empty() {
            return Err(GitError::BranchCheckedOut {
                branch: preflight.branch.as_str().to_string(),
                worktrees: checkouts,
            });
        }
        // The unique-commit verdict is part of what the operator approved.
        // Another ref being deleted after planning can make commits unique
        // without moving this branch, so re-derive the verdict here.
        let current_unique =
            self.unique_commits(&preflight.branch, &[], plan.creation_base.as_ref())?;
        if current_unique != preflight.unique_commits {
            return Err(GitError::PlanStale {
                what: preflight.branch.as_ref_name().to_string(),
                detail: format!(
                    "unique commits changed from {:?} to {:?}",
                    preflight.unique_commits, current_unique
                ),
            });
        }

        let mut preserved = None;
        if let Some(reference) = &plan.preserve_tip_at {
            match self.read_ref(reference)? {
                Some(existing) if existing == preflight.tip => {}
                Some(existing) => {
                    return Err(GitError::PlanStale {
                        what: reference.to_string(),
                        detail: format!("preservation ref already holds {existing}"),
                    });
                }
                None => {
                    let transaction = self.plan_ref_transaction(
                        &plan.reflog_message,
                        vec![RefChange::create(reference.clone(), preflight.tip.clone())],
                    )?;
                    let outcome = self.apply_ref_transaction(&transaction)?;
                    preserved = outcome.applied().first().cloned();
                }
            }
            if preserved.is_none() {
                preserved = Some(RefState::new(
                    reference.clone(),
                    Some(preflight.tip.clone()),
                ));
            }
        }

        let transaction = self.plan_ref_transaction(
            &plan.reflog_message,
            vec![RefChange::delete(
                preflight.branch.as_ref_name(),
                preflight.tip.clone(),
            )],
        )?;
        self.apply_ref_transaction(&transaction)?;

        Ok(BranchDeletionOutcome {
            branch: preflight.branch.clone(),
            tip: preflight.tip.clone(),
            preserved,
        })
    }

    /// Commits reachable from `branch` that no other local branch, remote ref,
    /// tag, or `extra_not` commit reaches.
    ///
    /// `ignoring` names branches the caller is about to delete together with
    /// `branch`, so they do not count as "another ref".
    pub fn unique_commits(
        &self,
        branch: &BranchName,
        ignoring: &[BranchName],
        extra_not: Option<&Oid>,
    ) -> Result<Vec<Oid>, GitError> {
        let own_ref = branch.as_ref_name();
        let mut skipped = BTreeSet::new();
        skipped.insert(own_ref.as_str().to_string());
        for name in ignoring {
            skipped.insert(name.as_ref_name().as_str().to_string());
        }

        let output = self.process().checked(&[
            "for-each-ref",
            "--format=%(refname)%00%(objectname)%00%(*objectname)",
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ])?;
        let text = crate::path::utf8_field(&output.stdout, "ref listing")?;
        let mut negatives: Vec<Oid> = Vec::new();
        for line in text.lines() {
            if line.is_empty() {
                continue;
            }
            let mut fields = line.split('\0');
            let reference = fields.next().unwrap_or_default();
            let objectname = fields.next().unwrap_or_default();
            let peeled = fields.next().unwrap_or_default();
            if skipped.contains(reference) {
                continue;
            }
            let chosen = if peeled.is_empty() {
                objectname
            } else {
                peeled
            };
            if chosen.is_empty() {
                continue;
            }
            negatives.push(Oid::parse(chosen)?);
        }
        if let Some(extra) = extra_not {
            negatives.push(extra.clone());
        }

        let mut stdin = String::new();
        for oid in &negatives {
            stdin.push('^');
            stdin.push_str(oid.as_str());
            stdin.push('\n');
        }
        let output = self
            .process()
            .checked_with_stdin(&["rev-list", "--stdin", own_ref.as_str()], stdin.as_bytes())?;
        let text = crate::path::utf8_field(&output.stdout, "unique commits")?;
        text.lines()
            .filter(|line| !line.is_empty())
            .map(Oid::parse)
            .collect()
    }

    /// Every registered worktree that has `branch` checked out.
    pub fn checkouts_of(&self, branch: &BranchName) -> Result<Vec<PathBuf>, GitError> {
        let mut paths: Vec<PathBuf> = self
            .worktrees()?
            .into_iter()
            .filter(|worktree| worktree.branch() == Some(branch.as_str()))
            .map(|worktree| worktree.path().to_path_buf())
            .collect();
        paths.sort();
        paths.dedup();
        Ok(paths)
    }

    /// Refuse when `checkout` is not clean, including untracked and ignored paths.
    ///
    /// A destructive resync or worktree removal deletes the whole checkout
    /// tree, so this deliberately has no obstruction-only exemption. An
    /// in-progress merge/rebase/sequencer run or a held index lock also refuses:
    /// a hard reset during one of those discards operation state as well as
    /// files.
    pub fn require_clean_checkout(&self, checkout: &std::path::Path) -> Result<(), GitError> {
        let handle = GitRepository::open_with_options(checkout, self.options())?;
        if handle.is_operation_in_progress()? {
            let operations = handle.in_progress_operations()?;
            return Err(GitError::OperationInProgress {
                operation: operations
                    .iter()
                    .map(|operation| operation.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            });
        }
        let entries = crate::status::status_at(
            &self.process_at(checkout.to_path_buf()),
            &StatusOptions {
                untracked: UntrackedMode::All,
                ignored: true,
            },
        )?;
        if entries.is_empty() {
            return Ok(());
        }
        Err(GitError::DirtyWorktree {
            path: checkout.to_path_buf(),
            entries: entries.iter().map(|entry| entry.display_lossy()).collect(),
        })
    }
}

fn describe_optional(oid: Option<&Oid>) -> String {
    match oid {
        Some(oid) => oid.to_string(),
        None => "absent".to_string(),
    }
}

fn sorted_paths(paths: &[PathBuf]) -> Vec<&PathBuf> {
    let mut sorted: Vec<&PathBuf> = paths.iter().collect();
    sorted.sort();
    sorted
}
