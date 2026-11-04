//! Exact fully-qualified ref publication, including a genuinely unborn
//! destination (a zero-parent root commit).
//!
//! The legacy [`SafeCommitRequest`](crate::SafeCommitRequest) family names a
//! short [`BranchName`](crate::BranchName) and its plan/target getters borrow a
//! real object id, so it can only describe a destination that already exists.
//! This module is the exact-ref family: the destination is a validated
//! fully-qualified [`RefName`], and the **planner observes** whether that ref is
//! born or absent. A caller cannot declare the state, and no placeholder object
//! id, seed parent, shadow branch, or substitute `Ready` value exists.
//!
//! # Why the destination is a full ref
//!
//! Git lets `HEAD` name refs a short branch name cannot express — the real
//! reference publication for `refs/x` (with the real `.git/x` ref that makes
//! `symbolic-ref --short` print `refs/x`), a branch literally named `HEAD`
//! (`refs/heads/HEAD`), and a branch named `-name` (`refs/heads/-name`). Forcing
//! any of those through `BranchName` would require inventing a replacement name,
//! so this family takes the exact ref `HEAD` symbolically names.
//!
//! # Publication
//!
//! There is no second writer. This family feeds the **one** planner body and the
//! **one** transaction in [`crate::safe_commit`] / [`crate::safe_commit_transaction`]:
//!
//! * the private index is seeded from the destination commit, or — for an unborn
//!   destination — from git's own canonical empty tree (`read-tree --empty`,
//!   never a hardcoded empty-object id);
//! * exactly the requested paths are staged with the existing literal-path
//!   policy, the repository's own commit hooks run through `git hook run`;
//! * `git commit-tree` creates the object with the observed parent, or with **no
//!   `-p` at all** when the destination is unborn;
//! * one compare-and-swap ref transaction publishes it: `RefChange::set` with the
//!   observed old object for a born ref, `RefChange::create` (a zero old value,
//!   i.e. "must be absent") for an unborn one;
//! * the operator index is reconciled for exactly the requested paths afterwards,
//!   and the landed/unconfirmed evidence is the shared one.
//!
//! The reflog subject is git's own: `commit: <subject>` for a normal commit and
//! `commit (initial): <subject>` for the first commit on the ref. Git owns the
//! whitespace normalization, the reflog of the destination (a non-`refs/heads/`
//! ref such as `refs/x` gets none, exactly as `git commit` leaves it) and the
//! `HEAD` reflog.
//!
//! # Apply-time re-checks
//!
//! [`GitRepository::apply_safe_physical_commit`] re-reads the **full symbolic
//! `HEAD`** and compares it with the exact destination ref (never the ambiguous
//! `--short` display), re-reads the destination value, and for an unborn plan
//! requires the ref to still be absent and `HEAD` still to name no commit. A ref
//! that appeared, or a commit that appeared behind it, is
//! [`GitError::PlanStale`] — never an overwrite and never a guessed parent.
//!
//! # Limits
//!
//! * This family is the ordinary path-only route. Index-only removals, a pinned
//!   parent, and exact raw-byte assertions remain on the legacy short-branch API;
//!   they are refused for an unborn destination rather than faked, and they are
//!   not part of this request type.
//! * [`ProtectedBranches`](crate::ProtectedBranches) holds short branch names, so
//!   a destination no `BranchName` can express (`refs/x`, `refs/heads/HEAD`,
//!   `refs/heads/-name`) is never a member of that set. The protection decision
//!   still runs through the one decision core, and resolving policy for such a
//!   name stays in the application layer (the Op decides it on the source-judged
//!   destination, exactly as before). No capability is added.
//! * Non-UTF-8 ref bytes and non-Unix platforms are outside this increment's
//!   claims.

use std::path::Path;

use crate::branch::ProtectedBranches;
use crate::error::GitError;
use crate::index_authority::OperatorIndex;
use crate::message::CommitMessage;
use crate::oid::Oid;
use crate::path::GitPath;
use crate::refname::RefName;
use crate::repository::GitRepository;
use crate::safe_commit::{
    CommitGuardCapability, DestinationState, SafeCommitCoreOutcome, SafeCommitExecution,
    SafeCommitPlanCore, SafeCommitPlanFailure, SafeCommitPreflightStage, SafeCommitRequestView,
    SafeCommitRoute,
};

/// The protection verdict recorded for one exact-ref plan.
///
/// Mirrors [`CommitGuardVerdict`](crate::CommitGuardVerdict) for a destination
/// that may not be expressible as a short branch name. The decision itself is
/// made by the one `evaluate_commit_guard_decision` core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafeCommitPhysicalGuardVerdict {
    allowed: bool,
    destination: RefName,
    reason: String,
}

impl SafeCommitPhysicalGuardVerdict {
    fn new(allowed: bool, destination: RefName, reason: String) -> Self {
        Self {
            allowed,
            destination,
            reason,
        }
    }

    /// Whether the commit may land.
    pub fn allowed(&self) -> bool {
        self.allowed
    }

    /// The destination the verdict is about (always echoed, never re-derived).
    pub fn destination(&self) -> &RefName {
        &self.destination
    }

    /// Why the verdict came out the way it did.
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

/// A request to publish exactly `paths` on one exact ref.
///
/// Construct with [`SafeCommitPhysicalRequest::new`]; the builders supply the
/// caller-resolved protected set and the asserted capability. Whether the
/// destination is born or absent is **not** part of the request: the planner
/// observes it, so a caller cannot claim a state.
///
/// # Protection is caller-supplied
///
/// As on the short-branch family, the default set is empty and the capability is
/// [`CommitGuardCapability::Standard`]. That is a library boundary, not a
/// production default: the application layer resolves the real facts. See the
/// module documentation for the exact-ref protection shape.
#[derive(Debug, Clone)]
pub struct SafeCommitPhysicalRequest {
    destination: RefName,
    paths: Vec<GitPath>,
    protected: ProtectedBranches,
    capability: CommitGuardCapability,
}

impl SafeCommitPhysicalRequest {
    /// A worktree-content commit of exactly `paths` on `destination`.
    pub fn new(destination: RefName, paths: impl IntoIterator<Item = GitPath>) -> Self {
        Self {
            destination,
            paths: paths.into_iter().collect(),
            protected: ProtectedBranches::default(),
            capability: CommitGuardCapability::Standard,
        }
    }

    /// Supply the caller-resolved protected set and the asserted capability.
    pub fn with_protection(
        mut self,
        protected: ProtectedBranches,
        capability: CommitGuardCapability,
    ) -> Self {
        self.protected = protected;
        self.capability = capability;
        self
    }

    /// The exact fully-qualified destination ref.
    pub fn destination(&self) -> &RefName {
        &self.destination
    }

    /// The requested worktree paths, as supplied (not yet normalised).
    pub fn paths(&self) -> &[GitPath] {
        &self.paths
    }
}

/// A successfully checked exact-ref target, without a message.
///
/// Sealed: its fields are private and it can only be produced by
/// [`GitRepository::plan_safe_physical_commit`]. It records the exact
/// destination ref, the **real** destination state (`Some` tip for a born ref,
/// `None` for a genuinely absent one), the normalised paths, the captured
/// operator index authority and the protection verdict. It is not a capability
/// to run arbitrary git.
#[derive(Debug, Clone)]
pub struct SafeCommitPhysicalPlan {
    core: SafeCommitPlanCore,
    guard: SafeCommitPhysicalGuardVerdict,
}

impl SafeCommitPhysicalPlan {
    fn from_core(core: SafeCommitPlanCore) -> Self {
        let guard = SafeCommitPhysicalGuardVerdict::new(
            core.guard_allowed(),
            core.destination_ref().clone(),
            core.guard_reason().to_string(),
        );
        Self { core, guard }
    }

    /// The exact fully-qualified ref the transaction compare-and-swaps.
    pub fn destination_ref(&self) -> &RefName {
        self.core.destination_ref()
    }

    /// Whether the destination ref was genuinely absent at plan time.
    ///
    /// `true` means the commit will be a zero-parent root commit and the ref
    /// transition is an absent-ref create; `false` means the plan pinned a real
    /// destination tip.
    pub fn is_unborn(&self) -> bool {
        matches!(self.core.destination(), DestinationState::Unborn)
    }

    /// The commit `HEAD` resolved to at plan time, or `None` when unborn.
    pub fn head(&self) -> Option<&Oid> {
        match self.core.destination() {
            DestinationState::Born { head, .. } => Some(head),
            DestinationState::Unborn => None,
        }
    }

    /// The destination ref's value at plan time, or `None` when absent.
    pub fn destination_tip(&self) -> Option<&Oid> {
        match self.core.destination() {
            DestinationState::Born { tip, .. } => Some(tip),
            DestinationState::Unborn => None,
        }
    }

    /// The checkout the commit lands in.
    pub fn worktree_root(&self) -> &Path {
        self.core.worktree_root()
    }

    /// The selected mutation route (this family is the path-only worktree route).
    pub fn route(&self) -> SafeCommitRoute {
        self.core.route()
    }

    /// The normalised worktree paths to commit.
    pub fn paths(&self) -> &[GitPath] {
        self.core.paths()
    }

    /// The protection verdict recorded at plan time.
    pub fn guard_verdict(&self) -> &SafeCommitPhysicalGuardVerdict {
        &self.guard
    }

    /// The operator index authority recorded at plan time.
    pub fn index_authority(&self) -> &OperatorIndex {
        self.core.index_authority()
    }

    /// The paths the commit is expected to carry.
    pub fn requested_paths(&self) -> Vec<GitPath> {
        self.core.requested_paths()
    }

    /// Attach an exact-byte message, producing the only applicable plan.
    ///
    /// The payload goes through the same message authority and the same
    /// [`SafeCommitPreflightStage::Message`] position as every other entry point.
    pub fn prepare(
        self,
        message: CommitMessage,
    ) -> Result<SafeCommitPhysicalMessagePlan, SafeCommitPlanFailure> {
        message.validate().map_err(|error| {
            SafeCommitPlanFailure::new(SafeCommitPreflightStage::Message, error)
        })?;
        Ok(SafeCommitPhysicalMessagePlan {
            target: self,
            message,
        })
    }

    /// The execution view for a message produced at the reference's argv boundary.
    pub(crate) fn execution_late(&self) -> SafeCommitExecution<'_> {
        self.core.execution_late()
    }
}

/// Why a late-message commit did not return a landed outcome.
///
/// `MessageNotProduced` reports a supplier failure at the late payload boundary.
/// `MessageRejected` reports Native validation there: NUL matches a Source argv
/// refusal, but the Native size bound is not proof of Source E2BIG. Both leave
/// the requested stage in place, before hooks or artifact writing. `Commit`
/// covers backend guards and execution/recovery failures; inspect the typed
/// cause and state rather than inferring no mutation or no land from an error.
#[derive(Debug)]
pub enum SafeCommitLateMessage<E> {
    /// The supplier could not produce the payload (for example an unencodable
    /// code point at OS-argv encoding time).
    MessageNotProduced(E),
    /// The produced payload was refused by the one message authority (NUL or
    /// over-long).
    MessageRejected(GitError),
    /// The transaction's typed failure.
    Commit(GitError),
}

impl<E: std::fmt::Display> std::fmt::Display for SafeCommitLateMessage<E> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MessageNotProduced(error) => std::fmt::Display::fmt(error, formatter),
            Self::MessageRejected(error) | Self::Commit(error) => {
                std::fmt::Display::fmt(error, formatter)
            }
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for SafeCommitLateMessage<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(match self {
            Self::MessageNotProduced(error) => error,
            Self::MessageRejected(error) | Self::Commit(error) => error,
        })
    }
}

#[cfg(test)]
mod late_message_error_tests {
    use super::SafeCommitLateMessage;
    use std::error::Error;

    #[test]
    fn the_public_late_failure_preserves_its_standard_error_cause() {
        let failure =
            SafeCommitLateMessage::MessageNotProduced(std::io::Error::other("encoding failed"));
        let boxed: Box<dyn Error> = Box::new(failure);
        assert_eq!(boxed.to_string(), "encoding failed");
        assert_eq!(boxed.source().unwrap().to_string(), "encoding failed");
    }
}

/// An exact-ref target plan plus the exact message bytes it will commit.
///
/// The only value [`GitRepository::apply_safe_physical_commit`] accepts.
#[derive(Debug, Clone)]
pub struct SafeCommitPhysicalMessagePlan {
    target: SafeCommitPhysicalPlan,
    message: CommitMessage,
}

impl SafeCommitPhysicalMessagePlan {
    /// The exact fully-qualified destination ref.
    pub fn destination_ref(&self) -> &RefName {
        self.target.destination_ref()
    }

    /// Whether the destination ref was genuinely absent at plan time.
    pub fn is_unborn(&self) -> bool {
        self.target.is_unborn()
    }

    /// The commit `HEAD` resolved to at plan time, or `None` when unborn.
    pub fn head(&self) -> Option<&Oid> {
        self.target.head()
    }

    /// The destination ref's value at plan time, or `None` when absent.
    pub fn destination_tip(&self) -> Option<&Oid> {
        self.target.destination_tip()
    }

    /// The checkout the commit lands in.
    pub fn worktree_root(&self) -> &Path {
        self.target.worktree_root()
    }

    /// The normalised worktree paths to commit.
    pub fn paths(&self) -> &[GitPath] {
        self.target.paths()
    }

    /// The protection verdict recorded at plan time.
    pub fn guard_verdict(&self) -> &SafeCommitPhysicalGuardVerdict {
        self.target.guard_verdict()
    }

    /// The operator index authority recorded at plan time.
    pub fn index_authority(&self) -> &OperatorIndex {
        self.target.index_authority()
    }

    /// The exact message bytes this plan will commit.
    pub fn message_bytes(&self) -> &[u8] {
        self.message.as_bytes()
    }

    /// The message as text only when it is valid UTF-8, never lossily.
    pub fn message_utf8(&self) -> Option<&str> {
        self.message.as_utf8()
    }

    /// Where the message bytes came from.
    pub fn message_origin(&self) -> crate::MessageOrigin {
        self.message.origin()
    }

    /// The message-free target this plan was prepared from.
    pub fn target(&self) -> &SafeCommitPhysicalPlan {
        &self.target
    }

    pub(crate) fn execution(&self) -> SafeCommitExecution<'_> {
        self.target.core.execution(self.message.as_bytes())
    }
}

/// The evidence of one landed exact-ref safe commit.
#[derive(Debug, Clone)]
pub struct SafeCommitPhysicalOutcome {
    commit: Oid,
    destination: RefName,
    was_unborn: bool,
    worktree_root: std::path::PathBuf,
    committed_paths: Vec<GitPath>,
    warnings: Vec<String>,
}

impl SafeCommitPhysicalOutcome {
    pub(crate) fn from_core(core: SafeCommitCoreOutcome, destination: RefName) -> Self {
        Self {
            commit: core.commit,
            destination,
            was_unborn: core.was_unborn,
            worktree_root: core.worktree_root,
            committed_paths: core.committed_paths,
            warnings: core.warnings,
        }
    }

    /// The new commit id.
    pub fn commit(&self) -> &Oid {
        &self.commit
    }

    /// The exact ref the commit landed on.
    pub fn destination_ref(&self) -> &RefName {
        &self.destination
    }

    /// Whether this commit was created against a genuinely absent destination
    /// (a zero-parent root commit published by an absent-ref compare-and-swap).
    pub fn was_unborn(&self) -> bool {
        self.was_unborn
    }

    /// The checkout the commit landed in.
    pub fn worktree_root(&self) -> &Path {
        &self.worktree_root
    }

    /// The commit's own changed-path set, read back from the commit object.
    pub fn committed_paths(&self) -> &[GitPath] {
        &self.committed_paths
    }

    /// Non-fatal diagnostics (for example a failing `post-commit` hook).
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

impl GitRepository {
    /// Prepare an exact-ref safe commit without mutating the repository.
    ///
    /// This is the **one** planner body the short-branch entry points also use,
    /// with the destination expressed as the exact `RefName` that `HEAD`
    /// symbolically names. It observes and pins the real destination state:
    /// a born ref records its tip, a genuinely absent ref records `Unborn`.
    ///
    /// The full symbolic `HEAD` is compared with the exact destination (never the
    /// ambiguous `--short` label). An absent destination is accepted only when
    /// `HEAD` also resolves to no commit; a corrupt `HEAD`, an unreadable ref, or
    /// a ref that names a non-commit object is a typed failure, never a root
    /// commit over a replaced tip.
    ///
    /// Nothing is written. Applying the returned plan re-runs every state check.
    pub fn plan_safe_physical_commit(
        &self,
        request: &SafeCommitPhysicalRequest,
    ) -> Result<SafeCommitPhysicalPlan, SafeCommitPlanFailure> {
        let view = SafeCommitRequestView::exact(
            &request.destination,
            &request.paths,
            &request.protected,
            request.capability,
        );
        let core = self.plan_safe_commit_core(&view, None)?;
        Ok(SafeCommitPhysicalPlan::from_core(core))
    }

    /// Execute a prepared exact-ref commit.
    ///
    /// The same execution body as the short-branch family: the captured operator
    /// index, the exact-`HEAD`/destination compare-and-swap, hooks, cleanup,
    /// signing, index reconciliation, readback and landed/unconfirmed evidence
    /// are shared. An unborn plan publishes with an absent-ref create, so a
    /// destination that appeared after planning is a CAS failure and the other
    /// writer is never overwritten.
    pub fn apply_safe_physical_commit(
        &self,
        plan: &SafeCommitPhysicalMessagePlan,
    ) -> Result<SafeCommitPhysicalOutcome, GitError> {
        let outcome = crate::safe_commit_transaction::execute(self, &plan.execution())?;
        Ok(SafeCommitPhysicalOutcome::from_core(
            outcome,
            plan.destination_ref().clone(),
        ))
    }

    /// Execute a prepared exact-ref commit whose message is produced late.
    ///
    /// `supply` is called at the reference implementation's `-m` argv boundary:
    /// **after** the requested paths have been staged into the real operator
    /// index and **before** any hook or message artifact. A supplier error — or a
    /// payload the message authority refuses — therefore leaves that stage in
    /// place, exactly like the reference's uncaught encoding failure, and is
    /// reported as [`SafeCommitLateMessage::MessageNotProduced`] /
    /// [`SafeCommitLateMessage::MessageRejected`]. Everything else — the captured
    /// operator selector, the bounded git program, the single commit-tree writer,
    /// the ref compare-and-swap, uncertainty evidence and the post-landing index
    /// reconciliation — is the same one body as the prepared route.
    ///
    /// The plan is borrowed, not consumed, and no repository handle is stored in
    /// it: the caller passes the same captured `GitRepository` it planned with.
    pub fn apply_safe_physical_commit_with_message<E, F>(
        &self,
        plan: &SafeCommitPhysicalPlan,
        supply: F,
    ) -> Result<SafeCommitPhysicalOutcome, SafeCommitLateMessage<E>>
    where
        F: FnOnce() -> Result<CommitMessage, E>,
    {
        let outcome =
            crate::safe_commit_transaction::execute_late(self, &plan.execution_late(), supply)
                .map_err(|failure| match failure {
                    crate::safe_commit_transaction::TransactionFailure::MessageNotProduced(
                        error,
                    ) => SafeCommitLateMessage::MessageNotProduced(error),
                    crate::safe_commit_transaction::TransactionFailure::MessageRejected(error) => {
                        SafeCommitLateMessage::MessageRejected(error)
                    }
                    crate::safe_commit_transaction::TransactionFailure::Commit(error) => {
                        SafeCommitLateMessage::Commit(error)
                    }
                })?;
        Ok(SafeCommitPhysicalOutcome::from_core(
            outcome,
            plan.destination_ref().clone(),
        ))
    }

    /// Plan and apply one exact-ref safe commit with an exact-byte message.
    ///
    /// The single-call convenience form. Callers that need a real preflight call
    /// [`GitRepository::plan_safe_physical_commit`] and
    /// [`GitRepository::apply_safe_physical_commit`] separately.
    pub fn safe_physical_commit(
        &self,
        request: &SafeCommitPhysicalRequest,
        message: CommitMessage,
    ) -> Result<SafeCommitPhysicalOutcome, GitError> {
        let plan = self
            .plan_safe_physical_commit(request)
            .map_err(SafeCommitPlanFailure::into_error)?
            .prepare(message)
            .map_err(SafeCommitPlanFailure::into_error)?;
        self.apply_safe_physical_commit(&plan)
    }
}
