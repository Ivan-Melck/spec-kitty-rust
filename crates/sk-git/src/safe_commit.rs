//! Path-scoped safe commit: commit exactly the files a caller names.
//!
//! This module is the native port of the upstream `safe_commit` substrate
//! (`specify_cli/git/commit_helpers.py` at the pinned baseline
//! `9aed3a8012cb5eb22c0128b14d525039f95fea7a`). It stages and commits an exact
//! file set without reading, moving, stashing, or restoring any content
//! outside the caller's paths:
//!
//! * staged work for unrelated paths (including a *partially* staged file)
//!   stays staged and out of the commit;
//! * unrelated unstaged and untracked files are untouched;
//! * the real index is not written at all until the commit has landed, so a
//!   refusal or a failed commit cannot strand or partially rewrite the caller's
//!   staging.
//!
//! # The three routes
//!
//! [`SafeCommitRoute::Worktree`] (the default) builds the candidate tree in a
//! **private index** seeded from `HEAD`: the requested paths are staged there
//! with `git add --force`, the repository's commit hooks run through
//! `git hook run` against that index (never `--no-verify`), the tree is written,
//! and `git commit-tree` creates a commit whose parent is the observed `HEAD`.
//! The destination branch is then advanced with a compare-and-swap that requires
//! that parent, so a commit built against a stale `HEAD` can never land on a
//! branch that has moved. A stale preflight is [`GitError::PlanStale`]; an
//! unconfirmed advance after hooks is [`GitError::SafeCommitAdvanceUnconfirmed`],
//! carrying the candidate and observed tip, never overwriting the other writer. Because the private index starts
//! from `HEAD` — never from the operator's index — unrelated staged content
//! cannot enter the commit even transiently. The ordinary route does stage the
//! requested paths in the real index first, exactly like the reference's
//! `git add` plus `commit --only` prepare step, and leaves that preparation in
//! place after landing; a stateful clean filter can therefore leave a requested
//! path staged as a different pass than the committed blob. A hook that stages
//! extra tracked files is visible in the commit; the commit's own changed-path
//! set is read back and compared with the request, and such extras are reported
//! as [`GitError::SafeCommitLanded`] with
//! [`crate::SafeCommitLandedKind::UnrequestedPaths`] after their index entries
//! are resynchronised to the immutable landed commit. The requested-path residue
//! check remains on the pinned-parent and index-deletion routes
//! ([`crate::SafeCommitLandedKind::IndexResidue`]); the ordinary route never
//! invents a clean index the reference does not have.
//!
//! [`SafeCommitRoute::IndexDeletions`] is the same transaction plus index-only
//! removals (the equivalent of `git rm --cached` for a migration), so the file
//! stays on disk and no operator-staged entry can enter the commit.
//!
//! [`SafeCommitRoute::PinnedParent`] is the expected-parent path: the caller
//! names the exact parent commit, the tree is built in a private index, the
//! commit-message hooks run through `git hook run`, the tree is re-isolated (so
//! a hook cannot add paths or change requested contents), optional exact raw
//! bytes are verified against the staged blobs, and the destination ref is
//! advanced with a compare-and-swap that requires the pinned parent. A
//! concurrent writer is never overwritten. An unconfirmed ref advance carries
//! candidate/readback evidence; no commit is automatically retried.
//!
//! # Exact refs and genuinely absent destinations
//!
//! The three routes above are the **short-branch** view: the destination is a
//! [`BranchName`] and its getters borrow a real object id, so it can only
//! describe a branch that already exists. A ref `HEAD` can symbolically name
//! that no short branch name expresses (`refs/x`, `refs/heads/HEAD`,
//! `refs/heads/-name`), or one that is genuinely absent, is published through the
//! exact-ref family ([`crate::SafeCommitPhysicalRequest`] and its plan/outcome).
//! That family feeds the **same** planner body below and the **same** transaction
//! in [`crate::safe_commit_transaction`]; it only adds a real `Born`/`Unborn`
//! destination state, an exact-`HEAD` comparison, git's canonical empty tree as
//! the seed for a root commit, a `commit-tree` invocation without `-p`, the
//! `commit (initial): ` reflog subject, and an absent-ref compare-and-swap
//! create. Path policy, protection, hooks, cleanup, signing, the captured
//! operator index, readback and landed/unconfirmed evidence are shared, so there
//! is exactly one writer.
//!
//! # Expected-state checks
//!
//! Every plan records the observed `HEAD`, the destination branch tip, and the
//! route. [`GitRepository::apply_safe_commit`] re-reads all of them before any
//! mutation and refuses a moved checkout with [`GitError::PlanStale`]. The
//! pinned-parent route additionally makes the ref write itself a
//! compare-and-swap. An in-progress Git operation (merge, rebase, cherry-pick,
//! revert, sequencer) or a held `index.lock` is refused with
//! [`GitError::OperationInProgress`]: safe commit never removes a lock it did not
//! create and never picks up a lock by waiting.
//!
//! # Protected destinations
//!
//! The protection **decision** is the pure function [`evaluate_commit_guard`],
//! exactly like upstream `core/commit_guard.py`. The protected-branch **set** is
//! a caller-resolved fact ([`ProtectedBranches`]); resolving it from
//! `.kittify/config.yaml`, the remote default branch, the operator hatch
//! `SPEC_KITTY_ALLOW_PROTECTED_BRANCH_COMMITS`, or a Mission's
//! `commit_to_target` stays in the application layer. A protected destination is
//! refused unless the call site asserts one of the bookkeeping capabilities.
//! No capability authorizes a push: this crate never pushes.
//!
//! # Compatibility notes
//!
//! * Paths are [`GitPath`] values (raw repository-relative bytes). Absolute
//!   caller paths can be normalised with [`normalize_safe_commit_path`], which
//!   follows symlinked parent directories, keeps the final component unresolved
//!   (a symlink is committed as a link), refuses a path outside the worktree,
//!   refuses `..` escapes, and refuses a symlink loop.
//! * Pathspecs are passed after `--` and under `--literal-pathspecs`, so a path
//!   named `-x`, `:(glob)*`, or `*.txt` is data, never pathspec magic.
//! * A requested directory is refused before any index mutation. A symlinked
//!   directory (one `120000` entry), an absent path (a deletion request), and a
//!   submodule gitlink (`160000`) are not directories for this rule.
//! * Upstream returns a `CommitResult` with a diagnostic when the pinned-parent
//!   commit lands but the real index cannot be resynchronised. This module
//!   reports that as [`GitError::SafeCommitLanded`] instead: the error names the
//!   landed commit, the affected paths, and the repair action, so no caller can
//!   mistake it for "not committed". Post-commit hook failures remain outcome
//!   warnings, matching upstream's warning-level handling.
//! * The commit object is created with `git commit-tree` against the observed
//!   parent and the branch is advanced with a compare-and-swap, so the commit id
//!   is known without a racy `HEAD` read and a concurrent writer can never be
//!   overwritten. Ref errors are read back: a reachable candidate is reconciled;
//!   otherwise the unconfirmed outcome requires inspection before retrying.
//!   Repository hooks may themselves change files or history; they are not rolled back.
//! * Every route needs `git hook run` (Git >= 2.36) to run commit hooks without
//!   executing hook files directly; older Git is refused with
//!   [`GitError::Unsupported`] rather than skipping hooks.
//! * The post-landing real-index reconciliation retries a bounded number of
//!   times when *another* writer holds git's `index.lock`. Each attempt writes
//!   the same idempotent result and never removes the other writer's lock. The
//!   commit itself is never retried.
//! * Ordinary pre-commit hooks run before initially-empty snapshot refusal,
//!   matching upstream; they cannot rescue that initial no-op. Message hooks
//!   observe GIT_EDITOR=: on ordinary routes, which force unsigned commits;
//!   pinned-parent routes retain configured signing and verbatim messages.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Component, Path, PathBuf};

use crate::branch::ProtectedBranches;
use crate::error::GitError;
use crate::index_authority::OperatorIndex;
use crate::message::CommitMessage;
use crate::oid::Oid;
use crate::path::GitPath;
use crate::refname::{BranchName, RefName};
use crate::repository::GitRepository;
use crate::safe_commit_paths::{
    canonical_worktree, native_path_shape_refusal, normalize_paths, path_to_git_bytes,
    refuse_symlink_loop, resolve_absolute_path, validate_path_policy,
};

/// Repository-relative directory whose contents must never be committed from
/// the primary checkout (upstream FR-005 / issue #1887).
pub const WORKTREES_DIR: &str = ".worktrees";

/// Marker file (inside the checkout's own git directory) and operation name for
/// every Git operation that blocks a safe commit.
///
/// Only the *per-worktree* git directory is scanned: the index a safe commit
/// reconciles is that checkout's own index, so a lock or merge state held in a
/// sibling worktree (including the primary one) must not block this checkout.
/// The held-index lock is not in this table: it is resolved through the
/// captured [operator index authority](crate::OperatorIndex), so a custom
/// `GIT_INDEX_FILE` is checked at its own `.lock`.
const COMMIT_BLOCKING_MARKERS: &[(&str, &str)] = &[
    ("rebase-merge", "rebase-merge"),
    ("rebase-apply", "rebase-apply"),
    ("merge", "MERGE_HEAD"),
    ("cherry-pick", "CHERRY_PICK_HEAD"),
    ("revert", "REVERT_HEAD"),
    ("sequencer", "sequencer"),
];

/// Maximum accepted commit message length.
///
/// Upstream has no explicit limit; this bound keeps one argv entry and the
/// bounded hook-message file comfortably inside the subprocess seam's budgets.
pub const MAX_COMMIT_MESSAGE_LEN: usize = 64 * 1024;

/// Asserted-at-the-surface authorization for a protected destination.
///
/// Mirrors upstream `GuardCapability`: the value is supplied explicitly by the
/// call site and is never derived from message text, file content, or ambient
/// environment. [`CommitGuardCapability::Standard`] authorizes no
/// protected-branch flow; every other member authorizes the one bookkeeping
/// flow it names. None authorizes a push.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommitGuardCapability {
    /// The ordinary placement-matched commit (the default).
    Standard,
    /// A release-flow bookkeeping commit.
    ReleaseFlow,
    /// An upgrade bookkeeping commit.
    UpgradeBookkeeping,
    /// A merge bookkeeping commit.
    MergeBookkeeping,
    /// A test-mode bookkeeping commit.
    TestMode,
    /// A post-consolidation write onto the resolved primary branch.
    PostConsolidationWrite,
}

impl CommitGuardCapability {
    /// The stable machine-readable name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::ReleaseFlow => "release_flow",
            Self::UpgradeBookkeeping => "upgrade_bookkeeping",
            Self::MergeBookkeeping => "merge_bookkeeping",
            Self::TestMode => "test_mode",
            Self::PostConsolidationWrite => "post_consolidation_write",
        }
    }

    /// Whether this capability authorizes landing on a protected ref.
    fn authorizes_protected_flow(self) -> bool {
        !matches!(self, Self::Standard)
    }
}

/// The pure protection verdict for one commit destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitGuardVerdict {
    allowed: bool,
    destination: BranchName,
    reason: String,
}

impl CommitGuardVerdict {
    /// Whether the commit may land.
    pub fn allowed(&self) -> bool {
        self.allowed
    }

    /// The destination the verdict is about (always echoed, never re-derived).
    pub fn destination(&self) -> &BranchName {
        &self.destination
    }

    /// Why the verdict came out the way it did.
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

/// Decide whether a commit may land on `destination`.
///
/// This is a pure function over explicit inputs, mirroring upstream
/// `commit_guard.evaluate`:
///
/// * an unprotected destination is allowed;
/// * a protected destination is allowed only when `capability` names a
///   bookkeeping flow (anything except [`CommitGuardCapability::Standard`]).
///
/// Message text, file content, and environment play no part. Resolving the
/// protected set is the application layer's responsibility.
pub fn evaluate_commit_guard(
    destination: &BranchName,
    protected: &ProtectedBranches,
    capability: CommitGuardCapability,
) -> CommitGuardVerdict {
    let allowed = evaluate_commit_guard_decision(protected.contains(destination), capability);
    let reason = if !protected.contains(destination) {
        format!("destination {destination:?} is not in the resolved protected set")
    } else if allowed {
        format!(
            "capability {:?} authorizes the bookkeeping flow onto protected branch {destination:?}",
            capability.as_str()
        )
    } else {
        format!(
            "protected branch {destination:?} is refused: capability {:?} authorizes no protected-branch flow",
            capability.as_str()
        )
    };
    CommitGuardVerdict {
        allowed,
        destination: destination.clone(),
        reason,
    }
}

/// The pure guard decision over already-resolved facts.
///
/// This is the ONE decision core: [`evaluate_commit_guard`] delegates to it with
/// the typed destination/set membership, and callers that resolve the source's
/// protection verdict under a differently-normalised name use it with that
/// boolean. No message text, file content, environment, or capability is
/// derived here; a `STANDARD` capability authorizes no protected flow.
pub fn evaluate_commit_guard_decision(protected: bool, capability: CommitGuardCapability) -> bool {
    !protected || capability.authorizes_protected_flow()
}

/// The exact-ref destination verdict, decided by the ONE decision core.
///
/// This is [`evaluate_commit_guard_decision`] with the membership fact computed
/// for a fully-qualified ref instead of a [`BranchName`].
///
/// [`ProtectedBranches`] holds `BranchName`s, so only a destination whose
/// `refs/heads/` suffix is a representable short branch name can be a member of
/// the caller's set; `refs/x`, `refs/heads/HEAD` and `refs/heads/-name` are
/// therefore never protected *by that set*, because its element type cannot
/// name them. Resolving policy for those names stays in the application layer —
/// the Op decides the verdict on the source-judged `J` and passes the
/// projection, exactly as it does today. Nothing here adds a capability:
/// [`CommitGuardCapability::Standard`] still authorizes no protected flow, and
/// the caller-resolved set is still the only protection input.
fn evaluate_exact_guard(
    reference: &RefName,
    protected: &ProtectedBranches,
    capability: CommitGuardCapability,
) -> (bool, String) {
    let member = reference
        .short_branch()
        .is_some_and(|branch| protected.contains(&branch));
    let allowed = evaluate_commit_guard_decision(member, capability);
    let reason = if !member {
        format!("destination {reference:?} is not in the resolved protected set")
    } else if allowed {
        format!(
            "capability {:?} authorizes the bookkeeping flow onto protected ref {reference:?}",
            capability.as_str()
        )
    } else {
        format!(
            "protected ref {reference:?} is refused: capability {:?} authorizes no protected-branch flow",
            capability.as_str()
        )
    };
    (allowed, reason)
}

/// One exact raw-bytes assertion for a pinned-parent commit.
///
/// The bytes are hashed with `git hash-object --stdin --no-filters` and compared
/// with the staged blob, so a clean/smudge filter cannot mask a mismatch.
#[derive(Clone, PartialEq, Eq)]
pub struct ExpectedBlob {
    path: GitPath,
    bytes: Vec<u8>,
}

impl ExpectedBlob {
    /// Assert that `path` stages exactly `bytes`.
    pub fn new(path: GitPath, bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            path,
            bytes: bytes.into(),
        }
    }

    /// The repository-relative path.
    pub fn path(&self) -> &GitPath {
        &self.path
    }

    /// The exact bytes expected in the staged blob.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl fmt::Debug for ExpectedBlob {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExpectedBlob")
            .field("path", &self.path)
            .field("bytes_len", &self.bytes.len())
            .finish()
    }
}

/// The preflight inputs for a safe commit, without a message.
///
/// Construct with [`SafeCommitTargetRequest::new`] and the `with_*` builders.
/// A target plan runs the native preflight without a message: the payload
/// cannot influence path, HEAD, tip, parent, guard, or lock preflight. Native
/// progress is not proof of reference path semantics or of its staging-before-
/// encoding failure footprint; callers must establish those separately.
///
/// # Protection is caller-supplied
///
/// The default protected set is **empty** and the capability is
/// [`CommitGuardCapability::Standard`]. That is a deliberate library boundary,
/// not a safe default for production: the application layer must resolve the
/// real facts (configuration, remote default branch, operator hatch, mission
/// `commit_to_target`) and attach them with
/// [`SafeCommitTargetRequest::with_protection`] before committing anywhere a
/// protected branch is possible. This crate never reads `.kittify`.
#[derive(Debug, Clone)]
pub struct SafeCommitTargetRequest {
    destination: BranchName,
    paths: Vec<GitPath>,
    deletions: Vec<GitPath>,
    expected_parent: Option<Oid>,
    expected_bytes: Vec<ExpectedBlob>,
    protected: ProtectedBranches,
    capability: CommitGuardCapability,
}

impl SafeCommitTargetRequest {
    /// A worktree-content commit of exactly `paths` on `destination`.
    pub fn new(destination: BranchName, paths: impl IntoIterator<Item = GitPath>) -> Self {
        Self {
            destination,
            paths: paths.into_iter().collect(),
            deletions: Vec::new(),
            expected_parent: None,
            expected_bytes: Vec::new(),
            protected: ProtectedBranches::default(),
            capability: CommitGuardCapability::Standard,
        }
    }

    /// Set the index-only removals (the file stays on disk).
    ///
    /// Replaces any previously supplied removals. Without an expected parent
    /// these are committed from a private index seeded from `HEAD`
    /// ([`SafeCommitRoute::IndexDeletions`]). With one, the pinned-parent route
    /// removes them from its candidate tree instead.
    pub fn with_index_deletions(mut self, deletions: impl IntoIterator<Item = GitPath>) -> Self {
        self.deletions = deletions.into_iter().collect();
        self
    }

    /// Pin the exact parent commit ([`SafeCommitRoute::PinnedParent`]).
    pub fn with_expected_parent(mut self, parent: Oid) -> Self {
        self.expected_parent = Some(parent);
        self
    }

    /// Assert exact staged bytes for requested paths (requires a pinned parent).
    ///
    /// Replaces any previously supplied assertions.
    pub fn with_expected_bytes(mut self, expected: impl IntoIterator<Item = ExpectedBlob>) -> Self {
        self.expected_bytes = expected.into_iter().collect();
        self
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

    /// The declared destination branch.
    pub fn destination(&self) -> &BranchName {
        &self.destination
    }

    /// The requested worktree paths, as supplied (not yet normalised).
    pub fn paths(&self) -> &[GitPath] {
        &self.paths
    }

    /// The requested index-only removals, as supplied.
    pub fn index_deletions(&self) -> &[GitPath] {
        &self.deletions
    }

    /// The pinned parent, when one was supplied.
    pub fn expected_parent(&self) -> Option<&Oid> {
        self.expected_parent.as_ref()
    }

    /// The exact-bytes assertions.
    pub fn expected_bytes(&self) -> &[ExpectedBlob] {
        &self.expected_bytes
    }
}

/// A validated safe-commit request, message included.
///
/// Construct with [`SafeCommitRequest::new`] and the `with_*` builders. The
/// destination is a validated short [`BranchName`]. The message is a UTF-8
/// `String` here; [`CommitMessage`] is the exact-byte equivalent, and
/// [`SafeCommitTargetRequest`] is the message-free target these builders
/// delegate to. Protection is caller-supplied exactly as documented on
/// [`SafeCommitTargetRequest`].
#[derive(Debug, Clone)]
pub struct SafeCommitRequest {
    target: SafeCommitTargetRequest,
    message: String,
}

impl SafeCommitRequest {
    /// A worktree-content commit of exactly `paths` on `destination`.
    pub fn new(
        destination: BranchName,
        message: impl Into<String>,
        paths: impl IntoIterator<Item = GitPath>,
    ) -> Self {
        Self {
            target: SafeCommitTargetRequest::new(destination, paths),
            message: message.into(),
        }
    }

    /// Set the index-only removals (the file stays on disk).
    pub fn with_index_deletions(mut self, deletions: impl IntoIterator<Item = GitPath>) -> Self {
        self.target = self.target.with_index_deletions(deletions);
        self
    }

    /// Pin the exact parent commit ([`SafeCommitRoute::PinnedParent`]).
    pub fn with_expected_parent(mut self, parent: Oid) -> Self {
        self.target = self.target.with_expected_parent(parent);
        self
    }

    /// Assert exact staged bytes for requested paths (requires a pinned parent).
    pub fn with_expected_bytes(mut self, expected: impl IntoIterator<Item = ExpectedBlob>) -> Self {
        self.target = self.target.with_expected_bytes(expected);
        self
    }

    /// Supply the caller-resolved protected set and the asserted capability.
    pub fn with_protection(
        mut self,
        protected: ProtectedBranches,
        capability: CommitGuardCapability,
    ) -> Self {
        self.target = self.target.with_protection(protected, capability);
        self
    }

    /// The declared destination branch.
    pub fn destination(&self) -> &BranchName {
        self.target.destination()
    }

    /// The commit message.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The requested worktree paths, as supplied (not yet normalised).
    pub fn paths(&self) -> &[GitPath] {
        self.target.paths()
    }

    /// The requested index-only removals, as supplied.
    pub fn index_deletions(&self) -> &[GitPath] {
        self.target.index_deletions()
    }

    /// The pinned parent, when one was supplied.
    pub fn expected_parent(&self) -> Option<&Oid> {
        self.target.expected_parent()
    }

    /// The exact-bytes assertions.
    pub fn expected_bytes(&self) -> &[ExpectedBlob] {
        self.target.expected_bytes()
    }

    /// The message-free target this request delegates to.
    pub fn target(&self) -> &SafeCommitTargetRequest {
        &self.target
    }
}

/// Which mutation strategy a plan selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SafeCommitRoute {
    /// Requested worktree paths committed from a private index seeded from
    /// `HEAD`; the real index is only reconciled after the commit lands.
    Worktree,
    /// The same private-index transaction plus index-only removals.
    IndexDeletions,
    /// Candidate tree built against a pinned parent, then a compare-and-swap
    /// ref advance.
    PinnedParent,
}

/// Which preflight check refused a message-free target plan.
///
/// The variant names the check that actually ran and refused; it is never
/// parsed out of error text, never guessed from a `Display` string, and never a
/// generic "protected or invalid" bucket. `Message` can only be produced by a
/// message-carrying entry point. Apply-time authority rechecks return plain
/// [`GitError`] values, not preflight stages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SafeCommitPreflightStage {
    /// The captured repository/index authority refuses a redirected write.
    Environment,
    /// The payload is empty, oversized, or contains NUL.
    Message,
    /// Empty changeset, deletion conflict, or inconsistent expected bytes.
    RequestShape,
    /// Path normalisation, an outside-worktree path, or a symlink loop.
    PathResolution,
    /// A `.worktrees/` payload path.
    PathPolicy,
    /// A directory argument or the gitlink index probe that exempts one.
    PathShape,
    /// `HEAD` is not the destination branch, or that read failed.
    HeadAssertion,
    /// The destination tip or `HEAD` commit cannot be resolved or read.
    DestinationExistence,
    /// `HEAD` and the destination tip disagree.
    HeadTipConsistency,
    /// A pinned parent is missing, unreadable, or is not the observed tip.
    ExpectedParent,
    /// The protected-branch verdict refuses this destination.
    Guard,
    /// A merge/rebase marker, a held index lock, or that read blocks the commit.
    OperationInProgress,
}

/// A refused target plan: the typed stage plus the exact original error value.
#[derive(Debug)]
pub struct SafeCommitPlanFailure {
    stage: SafeCommitPreflightStage,
    error: GitError,
}

impl SafeCommitPlanFailure {
    pub(crate) fn new(stage: SafeCommitPreflightStage, error: GitError) -> Self {
        Self { stage, error }
    }

    /// Which check refused the plan.
    pub fn stage(&self) -> SafeCommitPreflightStage {
        self.stage
    }

    /// The original error, unchanged.
    pub fn error(&self) -> &GitError {
        &self.error
    }

    /// Consume the failure and return the original error value.
    pub fn into_error(self) -> GitError {
        self.error
    }
}

impl fmt::Display for SafeCommitPlanFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "safe-commit preflight {:?}: {}",
            self.stage, self.error
        )
    }
}

impl std::error::Error for SafeCommitPlanFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

/// Which identity a plan's destination was expressed as.
///
/// The legacy API names a **short branch**; the exact API names the **fully
/// qualified ref** that `HEAD` symbolically points at. They are different
/// questions (git's `--short` label can be disambiguated to `heads/x`), so the
/// plan records which one it pinned and the apply-time re-check uses that same
/// authority. Nothing here re-derives or normalises one into the other.
#[derive(Debug, Clone)]
pub(crate) enum DestinationAuthority {
    /// A short branch name, compared with git's `--short` label (legacy).
    Branch(BranchName),
    /// A fully-qualified ref, compared with the full symbolic `HEAD` (exact).
    ExactRef(RefName),
}

/// The destination state the planner actually observed.
///
/// `Born` pins a real commit; `Unborn` records that the exact ref was genuinely
/// **absent** and `HEAD` resolved to no commit. There is no third state and no
/// placeholder object id: a corrupt `HEAD`, a failed read, or a ref that exists
/// but names a non-commit object is a typed failure, never `Unborn`.
#[derive(Debug, Clone)]
pub(crate) enum DestinationState {
    /// The destination ref exists and names this commit.
    Born {
        /// The commit `HEAD` resolves to (equal to `tip`).
        head: Oid,
        /// The destination ref's value at plan time.
        tip: Oid,
    },
    /// The destination ref is absent and `HEAD` has no commit.
    Unborn,
}

/// Which HEAD authority an apply-time re-check uses for its destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DestinationMatch {
    /// The legacy short-label comparison (`symbolic-ref --short HEAD`).
    ShortLabel,
    /// The exact full symbolic `HEAD` ref comparison.
    ExactRef,
}

/// The normalised, message-free request one planner body consumes.
///
/// Both public request families (short branch and exact ref) reduce to this
/// view, so the environment, message, shape, path, destination, parent, guard
/// and operation checks exist once.
pub(crate) struct SafeCommitRequestView<'a> {
    authority: DestinationAuthority,
    paths: &'a [GitPath],
    deletions: &'a [GitPath],
    expected_parent: Option<&'a Oid>,
    expected_bytes: &'a [ExpectedBlob],
    protected: &'a ProtectedBranches,
    capability: CommitGuardCapability,
}

impl<'a> SafeCommitRequestView<'a> {
    /// The legacy short-branch view of a target request.
    pub(crate) fn branch(branch: &BranchName, request: &'a SafeCommitTargetRequest) -> Self {
        Self {
            authority: DestinationAuthority::Branch(branch.clone()),
            paths: &request.paths,
            deletions: &request.deletions,
            expected_parent: request.expected_parent.as_ref(),
            expected_bytes: &request.expected_bytes,
            protected: &request.protected,
            capability: request.capability,
        }
    }

    /// The exact-ref view: the ordinary path-only route, with the destination
    /// state (born or unborn) observed by the planner rather than declared by
    /// the caller.
    pub(crate) fn exact(
        destination: &RefName,
        paths: &'a [GitPath],
        protected: &'a ProtectedBranches,
        capability: CommitGuardCapability,
    ) -> Self {
        Self {
            authority: DestinationAuthority::ExactRef(destination.clone()),
            paths,
            deletions: &[],
            expected_parent: None,
            expected_bytes: &[],
            protected,
            capability,
        }
    }
}

/// The single planner output: every fact an execution needs, with a real
/// `Born`/`Unborn` destination state.
#[derive(Debug, Clone)]
pub(crate) struct SafeCommitPlanCore {
    destination_ref: RefName,
    destination_label: String,
    destination_match: DestinationMatch,
    destination: DestinationState,
    worktree_root: PathBuf,
    route: SafeCommitRoute,
    paths: Vec<GitPath>,
    deletions: Vec<GitPath>,
    expected_parent: Option<Oid>,
    expected_bytes: Vec<ExpectedBlob>,
    index_authority: OperatorIndex,
    guard_allowed: bool,
    guard_reason: String,
}

impl SafeCommitPlanCore {
    pub(crate) fn destination_ref(&self) -> &RefName {
        &self.destination_ref
    }

    pub(crate) fn destination(&self) -> &DestinationState {
        &self.destination
    }

    pub(crate) fn worktree_root(&self) -> &Path {
        &self.worktree_root
    }

    pub(crate) fn route(&self) -> SafeCommitRoute {
        self.route
    }

    pub(crate) fn paths(&self) -> &[GitPath] {
        &self.paths
    }

    pub(crate) fn index_deletions(&self) -> &[GitPath] {
        &self.deletions
    }

    pub(crate) fn expected_parent(&self) -> Option<&Oid> {
        self.expected_parent.as_ref()
    }

    pub(crate) fn expected_bytes(&self) -> &[ExpectedBlob] {
        &self.expected_bytes
    }

    pub(crate) fn index_authority(&self) -> &OperatorIndex {
        &self.index_authority
    }

    pub(crate) fn guard_allowed(&self) -> bool {
        self.guard_allowed
    }

    pub(crate) fn guard_reason(&self) -> &str {
        &self.guard_reason
    }

    pub(crate) fn requested_paths(&self) -> Vec<GitPath> {
        let mut all = self.paths.clone();
        all.extend(self.deletions.iter().cloned());
        all
    }

    /// The one execution view, for this exact payload.
    pub(crate) fn execution<'a>(&'a self, message: &'a [u8]) -> SafeCommitExecution<'a> {
        self.execution_with(Some(message))
    }

    /// The same view for a route that obtains its message later.
    ///
    /// `None` is not "an empty message": it records that the payload is
    /// produced at the reference implementation's argv boundary, after the
    /// operator index has been staged. Only the ordinary worktree route accepts
    /// it (see [`crate::safe_commit_transaction`]).
    pub(crate) fn execution_late(&self) -> SafeCommitExecution<'_> {
        self.execution_with(None)
    }

    fn execution_with<'a>(&'a self, message: Option<&'a [u8]>) -> SafeCommitExecution<'a> {
        let (head, destination_tip) = match &self.destination {
            DestinationState::Born { head, tip } => (Some(head), Some(tip)),
            DestinationState::Unborn => (None, None),
        };
        SafeCommitExecution {
            destination_ref: &self.destination_ref,
            destination_label: &self.destination_label,
            destination_match: self.destination_match,
            worktree_root: &self.worktree_root,
            head,
            destination_tip,
            route: self.route,
            paths: &self.paths,
            deletions: &self.deletions,
            expected_parent: self.expected_parent.as_ref(),
            expected_bytes: &self.expected_bytes,
            index_authority: &self.index_authority,
            message,
        }
    }
}

/// A successfully checked native target, without a message.
///
/// Holds the observed HEAD and destination tip, the normalised path set, the
/// chosen route, and the protection verdict — never a payload. It is not a
/// capability to run arbitrary git: the only way to reach a mutation is
/// [`SafeCommitTargetPlan::prepare`], and applying can only produce the commit
/// described here.
///
/// This is the **short-branch** view. Its `head()`/`destination_tip()` getters
/// borrow a real object id, so it cannot represent an absent destination: the
/// planner refuses an unborn short-branch destination with
/// [`GitError::SafeCommitDestinationMissing`], exactly as before. The exact-ref
/// route that *can* represent absence is
/// [`SafeCommitPhysicalPlan`](crate::SafeCommitPhysicalPlan).
#[derive(Debug, Clone)]
pub struct SafeCommitTargetPlan {
    core: SafeCommitPlanCore,
    destination: BranchName,
    guard: CommitGuardVerdict,
}

impl SafeCommitTargetPlan {
    /// The legacy view is born-only: the short-branch planner resolves a real
    /// destination tip before it can produce a plan, and the conversion below
    /// destructures that pinned state. There is no fake or substitute object id.
    fn born(&self) -> (&Oid, &Oid) {
        match &self.core.destination {
            DestinationState::Born { head, tip } => (head, tip),
            DestinationState::Unborn => {
                unreachable!("the short-branch planner never plans an unborn destination")
            }
        }
    }

    /// The destination branch.
    pub fn destination(&self) -> &BranchName {
        &self.destination
    }

    /// The checkout the commit lands in.
    pub fn worktree_root(&self) -> &Path {
        self.core.worktree_root()
    }

    /// The destination tip (`HEAD`) observed when the plan was prepared.
    pub fn head(&self) -> &Oid {
        self.born().0
    }

    /// The destination branch tip observed when the plan was prepared.
    pub fn destination_tip(&self) -> &Oid {
        self.born().1
    }

    /// The selected mutation route.
    pub fn route(&self) -> SafeCommitRoute {
        self.core.route()
    }

    /// The normalised worktree paths to commit.
    pub fn paths(&self) -> &[GitPath] {
        self.core.paths()
    }

    /// The normalised index-only removals.
    pub fn index_deletions(&self) -> &[GitPath] {
        self.core.index_deletions()
    }

    /// The pinned parent, when the pinned-parent route was selected.
    pub fn expected_parent(&self) -> Option<&Oid> {
        self.core.expected_parent()
    }

    /// The normalised exact-bytes assertions.
    pub fn expected_bytes(&self) -> &[ExpectedBlob] {
        self.core.expected_bytes()
    }

    /// The protection verdict recorded at plan time.
    pub fn guard_verdict(&self) -> &CommitGuardVerdict {
        &self.guard
    }

    /// The operator index authority recorded at plan time.
    pub fn index_authority(&self) -> &OperatorIndex {
        self.core.index_authority()
    }

    /// Every path the commit is expected to carry (adds and removals).
    pub fn requested_paths(&self) -> Vec<GitPath> {
        self.core.requested_paths()
    }

    /// Attach an exact-byte message, producing the only applicable plan.
    ///
    /// The message is validated again through the same authority the planner
    /// uses, so an empty, oversized, or NUL-bearing payload is
    /// [`SafeCommitPreflightStage::Message`] — exactly the legacy plan's error
    /// value at the same stage. A target plan is not applicable on its own.
    pub fn prepare(
        self,
        message: CommitMessage,
    ) -> Result<SafeCommitMessagePlan, SafeCommitPlanFailure> {
        message.validate().map_err(|error| {
            SafeCommitPlanFailure::new(SafeCommitPreflightStage::Message, error)
        })?;
        Ok(SafeCommitMessagePlan {
            target: self,
            message,
        })
    }

    /// The one execution view both plan types share, for this exact payload.
    pub(crate) fn execution<'a>(&'a self, message: &'a [u8]) -> SafeCommitExecution<'a> {
        self.core.execution(message)
    }
}

/// A target plan plus the exact message bytes it will commit.
///
/// The only value [`GitRepository::apply_safe_message_commit`] accepts. Message
/// inspection is explicit: [`SafeCommitMessagePlan::message_bytes`] is the
/// payload, [`SafeCommitMessagePlan::message_utf8`] is an optional text view and
/// never a fabricated or lossy `String`.
#[derive(Debug, Clone)]
pub struct SafeCommitMessagePlan {
    target: SafeCommitTargetPlan,
    message: CommitMessage,
}

impl SafeCommitMessagePlan {
    /// The destination branch.
    pub fn destination(&self) -> &BranchName {
        self.target.destination()
    }

    /// The checkout the commit lands in.
    pub fn worktree_root(&self) -> &Path {
        self.target.worktree_root()
    }

    /// The destination tip (`HEAD`) observed when the plan was prepared.
    pub fn head(&self) -> &Oid {
        self.target.head()
    }

    /// The destination branch tip observed when the plan was prepared.
    pub fn destination_tip(&self) -> &Oid {
        self.target.destination_tip()
    }

    /// The selected mutation route.
    pub fn route(&self) -> SafeCommitRoute {
        self.target.route()
    }

    /// The normalised worktree paths to commit.
    pub fn paths(&self) -> &[GitPath] {
        self.target.paths()
    }

    /// The normalised index-only removals.
    pub fn index_deletions(&self) -> &[GitPath] {
        self.target.index_deletions()
    }

    /// The pinned parent, when the pinned-parent route was selected.
    pub fn expected_parent(&self) -> Option<&Oid> {
        self.target.expected_parent()
    }

    /// The normalised exact-bytes assertions.
    pub fn expected_bytes(&self) -> &[ExpectedBlob] {
        self.target.expected_bytes()
    }

    /// The protection verdict recorded at plan time.
    pub fn guard_verdict(&self) -> &CommitGuardVerdict {
        self.target.guard_verdict()
    }

    /// The operator index authority recorded at plan time.
    pub fn index_authority(&self) -> &OperatorIndex {
        self.target.index_authority()
    }

    /// Every path the commit is expected to carry (adds and removals).
    pub fn requested_paths(&self) -> Vec<GitPath> {
        self.target.requested_paths()
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
    pub fn target(&self) -> &SafeCommitTargetPlan {
        &self.target
    }
}

/// A validated, preflighted safe commit with a UTF-8 message.
///
/// Holds the target plan and the legacy `String` payload. [`Self::message`]
/// returns `&str` for the existing callers; [`SafeCommitTargetPlan::prepare`]
/// produces the exact-byte [`SafeCommitMessagePlan`] instead. It is not a
/// capability to run arbitrary git: applying it can only produce the commit
/// described here.
#[derive(Debug, Clone)]
pub struct SafeCommitPlan {
    target: SafeCommitTargetPlan,
    message: String,
}

impl SafeCommitPlan {
    /// The destination branch.
    pub fn destination(&self) -> &BranchName {
        self.target.destination()
    }

    /// The commit message.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The checkout the commit lands in.
    pub fn worktree_root(&self) -> &Path {
        self.target.worktree_root()
    }

    /// The destination tip (`HEAD`) observed when the plan was prepared.
    pub fn head(&self) -> &Oid {
        self.target.head()
    }

    /// The destination branch tip observed when the plan was prepared.
    pub fn destination_tip(&self) -> &Oid {
        self.target.destination_tip()
    }

    /// The selected mutation route.
    pub fn route(&self) -> SafeCommitRoute {
        self.target.route()
    }

    /// The normalised worktree paths to commit.
    pub fn paths(&self) -> &[GitPath] {
        self.target.paths()
    }

    /// The normalised index-only removals.
    pub fn index_deletions(&self) -> &[GitPath] {
        self.target.index_deletions()
    }

    /// The pinned parent, when the pinned-parent route was selected.
    pub fn expected_parent(&self) -> Option<&Oid> {
        self.target.expected_parent()
    }

    /// The normalised exact-bytes assertions.
    pub fn expected_bytes(&self) -> &[ExpectedBlob] {
        self.target.expected_bytes()
    }

    /// The protection verdict recorded at plan time.
    pub fn guard_verdict(&self) -> &CommitGuardVerdict {
        self.target.guard_verdict()
    }

    /// The operator index authority recorded at plan time.
    pub fn index_authority(&self) -> &OperatorIndex {
        self.target.index_authority()
    }

    /// Every path the commit is expected to carry (adds and removals).
    pub fn requested_paths(&self) -> Vec<GitPath> {
        self.target.requested_paths()
    }

    /// The message-free target this plan was prepared from.
    pub fn target(&self) -> &SafeCommitTargetPlan {
        &self.target
    }

    /// The one execution view both plan types share.
    pub(crate) fn execution(&self) -> SafeCommitExecution<'_> {
        self.target.execution(self.message.as_bytes())
    }
}

impl SafeCommitMessagePlan {
    /// The one execution view both plan types share.
    pub(crate) fn execution(&self) -> SafeCommitExecution<'_> {
        self.target.execution(self.message.as_bytes())
    }
}

/// The shared, read-only view one execution body consumes.
///
/// Legacy [`SafeCommitPlan`] and exact-byte [`SafeCommitMessagePlan`] both
/// produce this view, and so does the exact-ref
/// [`SafeCommitPhysicalMessagePlan`](crate::SafeCommitPhysicalMessagePlan), so
/// the transaction, hooks, CAS advance, index reconciliation, and
/// landed/unconfirmed evidence have exactly one implementation.
///
/// `head`/`destination_tip` are `Option` because a real destination can be
/// **unborn**. They are both `None` or both `Some`; the planner is the only
/// constructor of that pairing.
pub(crate) struct SafeCommitExecution<'a> {
    destination_ref: &'a RefName,
    destination_label: &'a str,
    destination_match: DestinationMatch,
    worktree_root: &'a Path,
    head: Option<&'a Oid>,
    destination_tip: Option<&'a Oid>,
    route: SafeCommitRoute,
    paths: &'a [GitPath],
    deletions: &'a [GitPath],
    expected_parent: Option<&'a Oid>,
    expected_bytes: &'a [ExpectedBlob],
    index_authority: &'a OperatorIndex,
    /// `Some` when the payload was prepared by the planner; `None` when the
    /// ordinary route obtains it at the reference argv boundary, after the
    /// operator index has been staged.
    message: Option<&'a [u8]>,
}

impl SafeCommitExecution<'_> {
    /// The exact fully-qualified ref the transaction compare-and-swaps.
    pub(crate) fn destination_ref(&self) -> &RefName {
        self.destination_ref
    }

    /// The destination spelled the way the calling API names it: the short
    /// branch for the legacy route, the full ref for the exact route.
    pub(crate) fn destination_label(&self) -> &str {
        self.destination_label
    }

    /// Which HEAD authority the apply-time re-check uses.
    pub(crate) fn destination_match(&self) -> DestinationMatch {
        self.destination_match
    }

    /// The destination commit pinned at plan time, or `None` when the
    /// destination ref was genuinely absent (an unborn root commit).
    pub(crate) fn head(&self) -> Option<&Oid> {
        self.head
    }

    /// The destination ref value pinned at plan time (`None` = absent).
    pub(crate) fn destination_tip(&self) -> Option<&Oid> {
        self.destination_tip
    }

    /// Whether the pinned destination was absent.
    pub(crate) fn is_unborn(&self) -> bool {
        self.destination_tip.is_none()
    }

    pub(crate) fn route(&self) -> SafeCommitRoute {
        self.route
    }

    pub(crate) fn paths(&self) -> &[GitPath] {
        self.paths
    }

    pub(crate) fn index_deletions(&self) -> &[GitPath] {
        self.deletions
    }

    pub(crate) fn expected_parent(&self) -> Option<&Oid> {
        self.expected_parent
    }

    pub(crate) fn expected_bytes(&self) -> &[ExpectedBlob] {
        self.expected_bytes
    }

    pub(crate) fn index_authority(&self) -> &OperatorIndex {
        self.index_authority
    }

    /// This plan's prepared payload, or `None` for a late-message execution.
    ///
    /// `None` never means an empty message: an empty payload is `Some(&[])` and
    /// is refused by the payload authority, not here.
    pub(crate) fn prepared_message(&self) -> Option<&[u8]> {
        self.message
    }

    pub(crate) fn worktree_root(&self) -> &Path {
        self.worktree_root
    }

    pub(crate) fn requested_paths(&self) -> Vec<GitPath> {
        let mut all = self.paths.to_vec();
        all.extend(self.deletions.iter().cloned());
        all
    }
}

/// The landed commit and the paths whose real-index state needs repair.
///
/// Carried by [`GitError::SafeCommitLanded`]. Nothing is reset or deleted
/// automatically: the caller (or operator) decides, using
/// [`SafeCommitRecovery::repair_argv`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafeCommitRecovery {
    commit: Oid,
    affected_paths: Vec<GitPath>,
}

impl SafeCommitRecovery {
    pub(crate) fn new(commit: Oid, affected_paths: Vec<GitPath>) -> Self {
        Self {
            commit,
            affected_paths,
        }
    }

    /// The commit object that exists and is reachable from the destination.
    pub fn commit(&self) -> &Oid {
        &self.commit
    }

    /// The paths whose index entries need to be resynchronised to `commit`.
    pub fn affected_paths(&self) -> &[GitPath] {
        &self.affected_paths
    }

    /// The exact argv that repairs the index, with no shell quoting involved.
    ///
    /// `program` is the git executable to use (for example the one from this
    /// repository's [`GitOptions`](crate::GitOptions)).
    pub fn repair_argv(&self, program: &OsStr) -> Vec<OsString> {
        let mut argv: Vec<OsString> = vec![
            program.to_os_string(),
            OsString::from("--literal-pathspecs"),
            OsString::from("reset"),
            OsString::from("--quiet"),
            OsString::from(self.commit.as_str()),
            OsString::from("--"),
        ];
        argv.extend(self.affected_paths.iter().map(GitPath::to_os_string));
        argv
    }
}

impl fmt::Display for SafeCommitRecovery {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "commit {} (index paths: {})",
            self.commit,
            self.affected_paths
                .iter()
                .map(GitPath::display_lossy)
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

/// The evidence of one landed safe commit.
#[derive(Debug, Clone)]
pub struct SafeCommitOutcome {
    commit: Oid,
    destination: BranchName,
    worktree_root: PathBuf,
    committed_paths: Vec<GitPath>,
    warnings: Vec<String>,
}

impl SafeCommitOutcome {
    pub(crate) fn new(
        commit: Oid,
        destination: BranchName,
        worktree_root: PathBuf,
        committed_paths: Vec<GitPath>,
        warnings: Vec<String>,
    ) -> Self {
        Self {
            commit,
            destination,
            worktree_root,
            committed_paths,
            warnings,
        }
    }

    /// The new commit id.
    pub fn commit(&self) -> &Oid {
        &self.commit
    }

    /// The branch the commit landed on.
    pub fn destination(&self) -> &BranchName {
        &self.destination
    }

    /// The checkout the commit landed in.
    pub fn worktree_root(&self) -> &Path {
        &self.worktree_root
    }

    /// The commit's own changed-path set, read back from the commit object
    /// (renames split into delete plus add, as upstream does).
    pub fn committed_paths(&self) -> &[GitPath] {
        &self.committed_paths
    }

    /// Non-fatal diagnostics (for example a failing `post-commit` hook).
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

/// The one outcome core every route produces.
///
/// Legacy [`SafeCommitOutcome`] and the exact-ref
/// [`SafeCommitPhysicalOutcome`](crate::SafeCommitPhysicalOutcome) are views over
/// this value: the commit id, whether the destination was genuinely absent, the
/// checkout, the commit's own changed-path set, and hook warnings. There is no
/// second evidence writer.
pub(crate) struct SafeCommitCoreOutcome {
    pub(crate) commit: Oid,
    pub(crate) was_unborn: bool,
    pub(crate) worktree_root: PathBuf,
    pub(crate) committed_paths: Vec<GitPath>,
    pub(crate) warnings: Vec<String>,
}

impl SafeCommitCoreOutcome {
    pub(crate) fn new(
        commit: Oid,
        was_unborn: bool,
        worktree_root: PathBuf,
        committed_paths: Vec<GitPath>,
        warnings: Vec<String>,
    ) -> Self {
        Self {
            commit,
            was_unborn,
            worktree_root,
            committed_paths,
            warnings,
        }
    }

    /// The legacy born-only public view.
    ///
    /// The short-branch planner refuses an unborn destination, so this route
    /// never carries one; the name is kept for symmetry with the physical view.
    pub(crate) fn into_legacy(self, destination: BranchName) -> SafeCommitOutcome {
        SafeCommitOutcome::new(
            self.commit,
            destination,
            self.worktree_root,
            self.committed_paths,
            self.warnings,
        )
    }
}

/// Resolve a caller path to the repository-relative [`GitPath`] safe commit uses.
///
/// * A relative path is resolved against `worktree_root`; `..`, a root, or a
///   Windows prefix component is refused rather than allowed to escape.
/// * An absolute path is resolved against the worktree: symlinked parent
///   directories are followed, the final component is kept as the caller spelled
///   it (a symlink is committed as the link, never its target), and a path whose
///   parents resolve outside `worktree_root` is refused.
/// * A symlink loop anywhere on the path is refused.
///
/// The returned value is not truncated, lower-cased, or UTF-8-normalised: on
/// Unix the exact bytes reach git.
pub fn normalize_safe_commit_path(
    worktree_root: &Path,
    path: impl AsRef<Path>,
) -> Result<GitPath, GitError> {
    let worktree_root = canonical_worktree(worktree_root)?;
    let supplied = path.as_ref();
    let relative = if supplied.is_absolute() {
        let resolved = resolve_absolute_path(&worktree_root, supplied)?;
        resolved
            .strip_prefix(&worktree_root)
            .map_err(|_| GitError::SafeCommitPathOutsideWorktree {
                path: supplied.to_path_buf(),
                worktree_root: worktree_root.clone(),
            })?
            .to_path_buf()
    } else {
        let mut relative = PathBuf::new();
        for component in supplied.components() {
            match component {
                Component::Normal(name) => relative.push(name),
                Component::CurDir => {}
                _ => {
                    return Err(GitError::SafeCommitPathOutsideWorktree {
                        path: supplied.to_path_buf(),
                        worktree_root,
                    });
                }
            }
        }
        relative
    };
    if relative.as_os_str().is_empty() {
        return Err(GitError::SafeCommitEmptyChangeset);
    }
    let bytes = path_to_git_bytes(&relative)?;
    let git_path = GitPath::from_git_bytes(&bytes)?;
    refuse_symlink_loop(&worktree_root, &git_path)?;
    Ok(git_path)
}

impl GitRepository {
    /// Prepare a native safe-commit target without mutating the repository.
    ///
    /// This entry point omits the payload check. Its progress names native
    /// checks, not reference path semantics or staging/encoding order. The
    /// single planner body is shared with
    /// [`GitRepository::plan_safe_commit`], whose legacy order
    /// `Environment → Message → RequestShape → paths → HEAD → tip → expected
    /// parent → guard → operation` and exact error values are unchanged.
    ///
    /// Nothing is written. Applying an applicable plan re-runs the state checks,
    /// so a preflight never grants stale authority.
    pub fn plan_safe_commit_target(
        &self,
        request: &SafeCommitTargetRequest,
    ) -> Result<SafeCommitTargetPlan, SafeCommitPlanFailure> {
        self.plan_safe_commit_inner(request, None)
    }

    /// Prepare a safe commit without mutating the repository.
    ///
    /// Validates, in order: the message, a non-empty path set, request
    /// consistency (index deletions never overlap requested paths, exact-bytes
    /// assertions need a pinned parent and name requested paths), every path
    /// (normalisation, symlink loop, `.worktrees/`, directory argument), HEAD
    /// equals the destination branch, the destination exists, the pinned parent
    /// equals the destination tip, the protected-branch verdict, and the absence
    /// of an in-progress Git operation or held index lock.
    ///
    /// Nothing is written. Applying the returned plan re-runs the state checks,
    /// so a preflight never grants stale authority.
    pub fn plan_safe_commit(
        &self,
        request: &SafeCommitRequest,
    ) -> Result<SafeCommitPlan, GitError> {
        let message = CommitMessage::from_text(&request.message);
        let target = self
            .plan_safe_commit_inner(&request.target, Some(&message))
            .map_err(SafeCommitPlanFailure::into_error)?;
        Ok(SafeCommitPlan {
            target,
            message: request.message.clone(),
        })
    }

    /// The legacy planner: the short-branch destination authority over the one
    /// shared planner body.
    ///
    /// `message: Option<&CommitMessage>` keeps the legacy `Message` stage at its
    /// exact position and makes it unreachable from the message-free target
    /// entry. No second path validator and no reordering exist here.
    fn plan_safe_commit_inner(
        &self,
        request: &SafeCommitTargetRequest,
        message: Option<&CommitMessage>,
    ) -> Result<SafeCommitTargetPlan, SafeCommitPlanFailure> {
        let destination = request.destination.clone();
        let view = SafeCommitRequestView::branch(&destination, request);
        let core = self.plan_safe_commit_core(&view, message)?;
        if matches!(core.destination, DestinationState::Unborn) {
            unreachable!("the short-branch planner never plans an unborn destination")
        }
        let guard = CommitGuardVerdict {
            allowed: core.guard_allowed(),
            destination: destination.clone(),
            reason: core.guard_reason().to_string(),
        };
        Ok(SafeCommitTargetPlan {
            core,
            destination,
            guard,
        })
    }

    /// The one planner body every entry point (short branch and exact ref) calls.
    ///
    /// Order is `Environment → Message → RequestShape → paths → destination →
    /// expected parent → guard → operation`. The destination step is the only
    /// kind-specific one: a short branch is compared with git's `--short` label
    /// and must exist (the legacy contract, unchanged), while an exact ref is
    /// compared with the full symbolic `HEAD` and may be genuinely absent.
    ///
    /// Nothing is written. Applying an applicable plan re-runs the state checks.
    pub(crate) fn plan_safe_commit_core(
        &self,
        view: &SafeCommitRequestView<'_>,
        message: Option<&CommitMessage>,
    ) -> Result<SafeCommitPlanCore, SafeCommitPlanFailure> {
        let fail = SafeCommitPlanFailure::new;
        self.process()
            .ensure_unredirected_write()
            .map_err(|error| fail(SafeCommitPreflightStage::Environment, error))?;
        if let Some(message) = message {
            message
                .validate()
                .map_err(|error| fail(SafeCommitPreflightStage::Message, error))?;
        }
        if view.paths.is_empty() && view.deletions.is_empty() {
            return Err(fail(
                SafeCommitPreflightStage::RequestShape,
                GitError::SafeCommitEmptyChangeset,
            ));
        }
        if !view.expected_bytes.is_empty() && view.expected_parent.is_none() {
            return Err(fail(
                SafeCommitPreflightStage::RequestShape,
                GitError::SafeCommitInvalidRequest {
                    detail: "expected raw bytes require an expected parent".to_string(),
                },
            ));
        }

        let root = self.root();
        let paths = normalize_paths(root, view.paths)
            .map_err(|error| fail(SafeCommitPreflightStage::PathResolution, error))?;
        let deletions = normalize_paths(root, view.deletions)
            .map_err(|error| fail(SafeCommitPreflightStage::PathResolution, error))?;
        let conflicts: Vec<String> = deletions
            .iter()
            .filter(|path| paths.contains(path))
            .map(GitPath::display_lossy)
            .collect();
        if !conflicts.is_empty() {
            return Err(fail(
                SafeCommitPreflightStage::RequestShape,
                GitError::SafeCommitIndexDeletionConflict { paths: conflicts },
            ));
        }
        for path in paths.iter().chain(deletions.iter()) {
            validate_path_policy(root, path)
                .map_err(|error| fail(SafeCommitPreflightStage::PathPolicy, error))?;
            native_path_shape_refusal(self, path)
                .map_err(|error| fail(SafeCommitPreflightStage::PathShape, error))?;
        }
        let expected_bytes = self
            .normalize_expected_bytes(view.expected_bytes, &paths)
            .map_err(|error| {
                let stage = match &error {
                    GitError::SafeCommitInvalidRequest { .. } => {
                        SafeCommitPreflightStage::RequestShape
                    }
                    GitError::SafeCommitPathPolicy { .. } => SafeCommitPreflightStage::PathPolicy,
                    _ => SafeCommitPreflightStage::PathResolution,
                };
                fail(stage, error)
            })?;

        let (destination_ref, destination_label, destination_match, destination) = match &view
            .authority
        {
            DestinationAuthority::Branch(branch) => {
                let observed = self
                    .current_branch()
                    .map_err(|error| fail(SafeCommitPreflightStage::HeadAssertion, error))?;
                if observed.as_deref() != Some(branch.as_str()) {
                    return Err(fail(
                        SafeCommitPreflightStage::HeadAssertion,
                        GitError::SafeCommitHeadMismatch {
                            destination: branch.as_str().to_string(),
                            observed_head: observed.unwrap_or_else(|| "<detached>".to_string()),
                            worktree_root: root.to_path_buf(),
                        },
                    ));
                }
                let tip = self
                    .read_branch_tip(branch)
                    .map_err(|error| fail(SafeCommitPreflightStage::DestinationExistence, error))?
                    .ok_or_else(|| {
                        fail(
                            SafeCommitPreflightStage::DestinationExistence,
                            GitError::SafeCommitDestinationMissing {
                                destination: branch.as_str().to_string(),
                                worktree_root: root.to_path_buf(),
                            },
                        )
                    })?;
                let head_text = self
                    .head_commit()
                    .map_err(|error| fail(SafeCommitPreflightStage::DestinationExistence, error))?
                    .ok_or_else(|| {
                        fail(
                            SafeCommitPreflightStage::DestinationExistence,
                            GitError::SafeCommitDestinationMissing {
                                destination: branch.as_str().to_string(),
                                worktree_root: root.to_path_buf(),
                            },
                        )
                    })?;
                let head = Oid::parse(&head_text)
                    .map_err(|error| fail(SafeCommitPreflightStage::DestinationExistence, error))?;
                if head != tip {
                    return Err(fail(
                        SafeCommitPreflightStage::HeadTipConsistency,
                        GitError::PlanStale {
                            what: branch.as_ref_name().to_string(),
                            detail: format!(
                                "HEAD is {head} but the branch tip is {tip}; the checkout is inconsistent"
                            ),
                        },
                    ));
                }
                (
                    branch.as_ref_name(),
                    branch.as_str().to_string(),
                    DestinationMatch::ShortLabel,
                    DestinationState::Born { head, tip },
                )
            }
            DestinationAuthority::ExactRef(reference) => {
                let observed = self
                    .symbolic_head_ref()
                    .map_err(|error| fail(SafeCommitPreflightStage::HeadAssertion, error))?;
                if observed.as_ref() != Some(reference) {
                    return Err(fail(
                        SafeCommitPreflightStage::HeadAssertion,
                        GitError::SafeCommitHeadMismatch {
                            destination: reference.as_str().to_string(),
                            observed_head: observed
                                .map(|observed| observed.as_str().to_string())
                                .unwrap_or_else(|| "<detached>".to_string()),
                            worktree_root: root.to_path_buf(),
                        },
                    ));
                }
                let state = match self
                    .read_ref(reference)
                    .map_err(|error| fail(SafeCommitPreflightStage::DestinationExistence, error))?
                {
                    Some(tip) => {
                        if !self.commit_exists(&tip).map_err(|error| {
                            fail(SafeCommitPreflightStage::DestinationExistence, error)
                        })? {
                            return Err(fail(
                                SafeCommitPreflightStage::DestinationExistence,
                                GitError::SafeCommitInvalidRequest {
                                    detail: format!(
                                        "destination ref {reference} names {tip}, which is not a commit object"
                                    ),
                                },
                            ));
                        }
                        let head_text = self
                            .head_commit()
                            .map_err(|error| {
                                fail(SafeCommitPreflightStage::DestinationExistence, error)
                            })?
                            .ok_or_else(|| {
                                fail(
                                    SafeCommitPreflightStage::DestinationExistence,
                                    GitError::SafeCommitDestinationMissing {
                                        destination: reference.as_str().to_string(),
                                        worktree_root: root.to_path_buf(),
                                    },
                                )
                            })?;
                        let head = Oid::parse(&head_text).map_err(|error| {
                            fail(SafeCommitPreflightStage::DestinationExistence, error)
                        })?;
                        if head != tip {
                            return Err(fail(
                                SafeCommitPreflightStage::HeadTipConsistency,
                                GitError::PlanStale {
                                    what: reference.to_string(),
                                    detail: format!(
                                        "HEAD is {head} but the ref tip is {tip}; the checkout is inconsistent"
                                    ),
                                },
                            ));
                        }
                        DestinationState::Born { head, tip }
                    }
                    // An absent ref is an unborn destination only when
                    // `HEAD` names no commit. A ref that exists but could
                    // not be read, or a `HEAD` that resolves to a commit
                    // while this ref is absent, is an inconsistency, never a
                    // "root" commit over a replaced tip.
                    None => {
                        if self
                            .head_commit()
                            .map_err(|error| {
                                fail(SafeCommitPreflightStage::DestinationExistence, error)
                            })?
                            .is_some()
                        {
                            return Err(fail(
                                    SafeCommitPreflightStage::HeadTipConsistency,
                                    GitError::PlanStale {
                                        what: reference.to_string(),
                                        detail: "HEAD resolves to a commit but the symbolic destination is absent; the checkout is inconsistent"
                                            .to_string(),
                                    },
                                ));
                        }
                        DestinationState::Unborn
                    }
                };
                (
                    reference.clone(),
                    reference.as_str().to_string(),
                    DestinationMatch::ExactRef,
                    state,
                )
            }
        };

        if let Some(parent) = view.expected_parent {
            if !self
                .commit_exists(parent)
                .map_err(|error| fail(SafeCommitPreflightStage::ExpectedParent, error))?
            {
                return Err(fail(
                    SafeCommitPreflightStage::ExpectedParent,
                    GitError::RefMissing {
                        name: parent.to_string(),
                    },
                ));
            }
            let pinned = match &destination {
                DestinationState::Born { tip, .. } => tip,
                DestinationState::Unborn => {
                    return Err(fail(
                        SafeCommitPreflightStage::RequestShape,
                        GitError::SafeCommitInvalidRequest {
                            detail: "an expected parent requires a born destination".to_string(),
                        },
                    ));
                }
            };
            if parent != pinned {
                return Err(fail(
                    SafeCommitPreflightStage::ExpectedParent,
                    GitError::PlanStale {
                        what: destination_ref.to_string(),
                        detail: format!("expected parent {parent}, observed tip {pinned}"),
                    },
                ));
            }
        }
        if !deletions.is_empty() && matches!(destination, DestinationState::Unborn) {
            return Err(fail(
                SafeCommitPreflightStage::RequestShape,
                GitError::SafeCommitInvalidRequest {
                    detail: "index-only removals require a born destination".to_string(),
                },
            ));
        }

        let (guard_allowed, guard_reason) = match &view.authority {
            DestinationAuthority::Branch(branch) => {
                let verdict = evaluate_commit_guard(branch, view.protected, view.capability);
                (verdict.allowed(), verdict.reason().to_string())
            }
            DestinationAuthority::ExactRef(reference) => {
                evaluate_exact_guard(reference, view.protected, view.capability)
            }
        };
        if !guard_allowed {
            return Err(fail(
                SafeCommitPreflightStage::Guard,
                GitError::SafeCommitProtectedBranch {
                    destination: destination_label.clone(),
                    worktree_root: root.to_path_buf(),
                    capability: view.capability.as_str(),
                },
            ));
        }

        self.require_no_commit_operation()
            .map_err(|error| fail(SafeCommitPreflightStage::OperationInProgress, error))?;

        let route = match (view.expected_parent, deletions.is_empty()) {
            (Some(_), _) => SafeCommitRoute::PinnedParent,
            (None, false) => SafeCommitRoute::IndexDeletions,
            (None, true) => SafeCommitRoute::Worktree,
        };
        Ok(SafeCommitPlanCore {
            destination_ref,
            destination_label,
            destination_match,
            destination,
            worktree_root: root.to_path_buf(),
            route,
            paths,
            deletions,
            expected_parent: view.expected_parent.cloned(),
            expected_bytes,
            index_authority: self.operator_index().clone(),
            guard_allowed,
            guard_reason,
        })
    }

    /// Execute a prepared safe commit.
    ///
    /// Re-reads HEAD and the destination tip first; a checkout that moved after
    /// planning is [`GitError::PlanStale`] with what was expected and observed,
    /// and nothing is written. A commit that lands but whose post-landing index
    /// resynchronisation or readback fails is [`GitError::SafeCommitLanded`],
    /// which names the commit and the repair action; no user state is reset.
    pub fn apply_safe_commit(&self, plan: &SafeCommitPlan) -> Result<SafeCommitOutcome, GitError> {
        let outcome = crate::safe_commit_transaction::execute(self, &plan.execution())?;
        Ok(outcome.into_legacy(plan.destination().clone()))
    }

    /// Execute a prepared exact-byte commit.
    ///
    /// The same one execution body as [`GitRepository::apply_safe_commit`]: the
    /// captured operator index, HEAD/tip compare-and-swap, hooks, cleanup,
    /// signing, and landed/unconfirmed evidence are shared. Only the payload
    /// reaching the message file differs.
    pub fn apply_safe_message_commit(
        &self,
        plan: &SafeCommitMessagePlan,
    ) -> Result<SafeCommitOutcome, GitError> {
        let outcome = crate::safe_commit_transaction::execute(self, &plan.execution())?;
        Ok(outcome.into_legacy(plan.destination().clone()))
    }
    /// Plan and apply one safe commit.
    ///
    /// This is the single-call convenience form. Callers that need a real
    /// preflight (for example to validate before writing a scaffold) call
    /// [`GitRepository::plan_safe_commit`] and
    /// [`GitRepository::apply_safe_commit`] separately.
    pub fn safe_commit(&self, request: &SafeCommitRequest) -> Result<SafeCommitOutcome, GitError> {
        let plan = self.plan_safe_commit(request)?;
        self.apply_safe_commit(&plan)
    }

    fn normalize_expected_bytes(
        &self,
        expected: &[ExpectedBlob],
        paths: &[GitPath],
    ) -> Result<Vec<ExpectedBlob>, GitError> {
        let root = self.root();
        let mut normalized: Vec<ExpectedBlob> = Vec::with_capacity(expected.len());
        for expected in expected {
            let path = normalize_safe_commit_path(root, expected.path().to_path_buf())?;
            if !paths.contains(&path) {
                return Err(GitError::SafeCommitInvalidRequest {
                    detail: format!(
                        "expected raw bytes name {} which is not requested for staging",
                        path.display_lossy()
                    ),
                });
            }
            if normalized.iter().any(|existing| existing.path() == &path) {
                return Err(GitError::SafeCommitInvalidRequest {
                    detail: format!("duplicate expected raw-bytes path {}", path.display_lossy()),
                });
            }
            validate_path_policy(root, &path)?;
            normalized.push(ExpectedBlob::new(path, expected.bytes().to_vec()));
        }
        Ok(normalized)
    }

    /// Refuse a commit while another Git operation or an index lock is present.
    ///
    /// Safe commit never waits for, removes, or overrides a lock it did not
    /// create: git's own lock files are the shared authority. An in-progress
    /// merge/rebase/cherry-pick/revert/sequencer is refused because `git commit`
    /// would create the wrong kind of commit. Only this checkout's own git
    /// directory is scanned, so a sibling worktree's lock does not block an
    /// unrelated lane commit.
    pub(crate) fn require_no_commit_operation(&self) -> Result<(), GitError> {
        let dirs = self.git_dirs()?;
        for (operation, marker) in COMMIT_BLOCKING_MARKERS {
            if dirs.git_dir().join(marker).exists() {
                return Err(GitError::OperationInProgress {
                    operation: (*operation).to_string(),
                });
            }
        }
        // The index lock belongs to the captured authority, which may be a
        // custom index outside the git directory.
        if self.operator_index_lock_path()?.exists() {
            return Err(GitError::OperationInProgress {
                operation: "index-lock".to_string(),
            });
        }
        Ok(())
    }
}

/// Validate a bounded reflog message for the compare-and-swap ref advance.
pub(crate) fn reflog_message(message: &str) -> String {
    let mut bounded = String::new();
    for character in message.chars() {
        if bounded.len() + character.len_utf8() > 2000 {
            break;
        }
        bounded.push(character);
    }
    // Git normalizes whitespace in update-ref -m, including an empty recorded
    // message for ASCII-whitespace-only input. Do not fabricate audit text.
    bounded
}

#[cfg(test)]
mod tests {
    use super::*;

    fn branch(name: &str) -> BranchName {
        BranchName::parse(name).unwrap()
    }

    #[test]
    fn guard_allows_unprotected_and_requires_a_capability_when_protected() {
        let protected = ProtectedBranches::parse(["main"]).unwrap();
        let standard = evaluate_commit_guard(
            &branch("feature/x"),
            &protected,
            CommitGuardCapability::Standard,
        );
        assert!(standard.allowed());

        let refused =
            evaluate_commit_guard(&branch("main"), &protected, CommitGuardCapability::Standard);
        assert!(!refused.allowed());
        assert_eq!(refused.destination().as_str(), "main");

        let allowed = evaluate_commit_guard(
            &branch("main"),
            &protected,
            CommitGuardCapability::ReleaseFlow,
        );
        assert!(allowed.allowed());
    }

    #[test]
    fn message_validation_refuses_empty_nul_and_oversized_messages() {
        assert!(CommitMessage::from_text("ok").validate().is_ok());
        assert!(CommitMessage::from_text("").validate().is_err());
        assert!(CommitMessage::from_text("has\0nul").validate().is_err());
        assert!(
            CommitMessage::from_text(&"x".repeat(MAX_COMMIT_MESSAGE_LEN + 1))
                .validate()
                .is_err()
        );
        assert!(
            CommitMessage::from_bytes(b"op(\x80)").validate().is_ok(),
            "the byte payload uses the same rule set"
        );
    }

    fn git_in(root: &Path, args: &[&str]) -> Vec<u8> {
        let output = std::process::Command::new("git")
            .current_dir(root)
            .args(args)
            .env("HOME", root.join("home"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    #[test]
    fn applying_a_plan_under_a_different_captured_index_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        git_in(&root, &["init", "-q", "-b", "work"]);
        std::fs::write(root.join("a.txt"), b"one\n").unwrap();
        git_in(&root, &["add", "-A"]);
        git_in(&root, &["commit", "-q", "-m", "initial"]);
        std::fs::write(root.join("a.txt"), b"two\n").unwrap();

        let repository = GitRepository::open(&root).unwrap();
        let request = SafeCommitRequest::new(
            branch("work"),
            "index authority",
            [GitPath::from_git_bytes(b"a.txt").unwrap()],
        );
        let plan = repository.plan_safe_commit(&request).unwrap();
        assert!(!plan.index_authority().is_captured());
        let head_before = git_in(&root, &["rev-parse", "HEAD"]);

        let mut other = repository.clone();
        other.replace_index_authority_for_test(&root.join("custom-index"));
        let error = other.apply_safe_commit(&plan).unwrap_err();
        match error {
            GitError::PlanStale { what, detail } => {
                assert_eq!(what, "operator index");
                assert!(detail.contains("custom-index"), "{detail}");
            }
            other => panic!("expected an index-authority refusal, got {other:?}"),
        }
        assert_eq!(git_in(&root, &["rev-parse", "HEAD"]), head_before);
        assert!(!root.join("custom-index").exists());
    }

    #[test]
    fn a_captured_index_lock_is_checked_on_the_captured_authority() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        git_in(&root, &["init", "-q", "-b", "work"]);
        std::fs::write(root.join("a.txt"), b"one\n").unwrap();
        git_in(&root, &["add", "-A"]);
        git_in(&root, &["commit", "-q", "-m", "initial"]);
        std::fs::write(root.join("a.txt"), b"two\n").unwrap();

        let custom = root.join("custom-index");
        std::fs::write(&custom, b"not an index yet").unwrap();
        let mut repository = GitRepository::open(&root).unwrap();
        repository.replace_index_authority_for_test(&custom);

        let lock = root.join("custom-index.lock");
        std::fs::write(&lock, b"held").unwrap();
        let request = SafeCommitRequest::new(
            branch("work"),
            "locked",
            [GitPath::from_git_bytes(b"a.txt").unwrap()],
        );
        let error = repository.plan_safe_commit(&request).unwrap_err();
        assert!(
            matches!(error, GitError::OperationInProgress { ref operation } if operation == "index-lock"),
            "{error:?}"
        );
        assert!(
            lock.exists(),
            "the adapter never removes another writer's lock"
        );
        assert!(
            !root.join(".git/index.lock").exists(),
            "the default index lock is not the captured authority"
        );

        std::fs::remove_file(&lock).unwrap();
        let plan = repository.plan_safe_commit(&request).unwrap();
        assert!(plan.index_authority().is_captured());
    }

    #[test]
    fn empty_protected_set_makes_every_destination_unprotected() {
        let protected = ProtectedBranches::default();
        assert!(
            evaluate_commit_guard(&branch("main"), &protected, CommitGuardCapability::Standard)
                .allowed()
        );
    }
}
