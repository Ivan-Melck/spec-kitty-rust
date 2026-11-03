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
//! cannot enter the commit even transiently, and the real index is not written
//! until the commit exists. A hook that stages extra tracked files is visible in
//! the commit; the commit's own changed-path set is read back and compared with
//! the request, and such extras are reported as [`GitError::SafeCommitLanded`]
//! with [`crate::SafeCommitLandedKind::UnrequestedPaths`] after their index
//! entries are resynchronised to the immutable landed commit. A residue after reconciling the
//! requested paths is a [`crate::SafeCommitLandedKind::IndexResidue`] error,
//! never a silent success.
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
use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use crate::branch::ProtectedBranches;
use crate::error::GitError;
use crate::index_authority::OperatorIndex;
use crate::oid::Oid;
use crate::path::GitPath;
use crate::refname::BranchName;
use crate::repository::GitRepository;

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
    let allowed = !protected.contains(destination) || capability.authorizes_protected_flow();
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

/// A validated safe-commit request.
///
/// Construct with [`SafeCommitRequest::new`] and the `with_*` builders. The
/// destination is a validated short [`BranchName`].
///
/// # Protection is caller-supplied
///
/// The default protected set is **empty** and the capability is
/// [`CommitGuardCapability::Standard`]. That is a deliberate library boundary,
/// not a safe default for production: the application layer must resolve the
/// real facts (configuration, remote default branch, operator hatch, mission
/// `commit_to_target`) and attach them with
/// [`SafeCommitRequest::with_protection`] before committing anywhere a
/// protected branch is possible. This crate never reads `.kittify`.
#[derive(Debug, Clone)]
pub struct SafeCommitRequest {
    destination: BranchName,
    message: String,
    paths: Vec<GitPath>,
    deletions: Vec<GitPath>,
    expected_parent: Option<Oid>,
    expected_bytes: Vec<ExpectedBlob>,
    protected: ProtectedBranches,
    capability: CommitGuardCapability,
}

impl SafeCommitRequest {
    /// A worktree-content commit of exactly `paths` on `destination`.
    pub fn new(
        destination: BranchName,
        message: impl Into<String>,
        paths: impl IntoIterator<Item = GitPath>,
    ) -> Self {
        Self {
            destination,
            message: message.into(),
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

    /// The commit message.
    pub fn message(&self) -> &str {
        &self.message
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

/// A validated, preflighted safe commit.
///
/// Holds the observed HEAD and destination tip, the normalised path set, the
/// chosen route, and the protection verdict. It is not a capability to run
/// arbitrary git: applying it can only produce the commit described here.
#[derive(Debug, Clone)]
pub struct SafeCommitPlan {
    destination: BranchName,
    message: String,
    worktree_root: PathBuf,
    head: Oid,
    destination_tip: Oid,
    route: SafeCommitRoute,
    paths: Vec<GitPath>,
    deletions: Vec<GitPath>,
    expected_parent: Option<Oid>,
    expected_bytes: Vec<ExpectedBlob>,
    guard: CommitGuardVerdict,
    index_authority: OperatorIndex,
}

impl SafeCommitPlan {
    /// The destination branch.
    pub fn destination(&self) -> &BranchName {
        &self.destination
    }

    /// The commit message.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The checkout the commit lands in.
    pub fn worktree_root(&self) -> &Path {
        &self.worktree_root
    }

    /// The destination tip (`HEAD`) observed when the plan was prepared.
    pub fn head(&self) -> &Oid {
        &self.head
    }

    /// The destination branch tip observed when the plan was prepared.
    pub fn destination_tip(&self) -> &Oid {
        &self.destination_tip
    }

    /// The selected mutation route.
    pub fn route(&self) -> SafeCommitRoute {
        self.route
    }

    /// The normalised worktree paths to commit.
    pub fn paths(&self) -> &[GitPath] {
        &self.paths
    }

    /// The normalised index-only removals.
    pub fn index_deletions(&self) -> &[GitPath] {
        &self.deletions
    }

    /// The pinned parent, when the pinned-parent route was selected.
    pub fn expected_parent(&self) -> Option<&Oid> {
        self.expected_parent.as_ref()
    }

    /// The normalised exact-bytes assertions.
    pub fn expected_bytes(&self) -> &[ExpectedBlob] {
        &self.expected_bytes
    }

    /// The protection verdict recorded at plan time.
    pub fn guard_verdict(&self) -> &CommitGuardVerdict {
        &self.guard
    }

    /// The operator index authority recorded at plan time.
    ///
    /// Applying the plan against a repository with a different authority (for
    /// example a reopened handle whose `GIT_INDEX_FILE` changed) is refused, so
    /// a plan can never reconcile an index other than the one it planned for.
    pub fn index_authority(&self) -> &OperatorIndex {
        &self.index_authority
    }

    /// Every path the commit is expected to carry (adds and removals).
    pub fn requested_paths(&self) -> Vec<GitPath> {
        let mut all = self.paths.clone();
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

/// Resolve an absolute path, following symlinked parent directories and
/// refusing a loop.
fn resolve_absolute_path(root: &Path, supplied: &Path) -> Result<PathBuf, GitError> {
    let Some(name) = supplied.file_name() else {
        return Err(GitError::SafeCommitPathOutsideWorktree {
            path: supplied.to_path_buf(),
            worktree_root: root.to_path_buf(),
        });
    };
    let parent = match supplied.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => root,
    };
    let resolved_parent = canonicalize_nearest_existing(root, parent, supplied)?;
    Ok(resolved_parent.join(name))
}

/// Canonicalise the nearest existing ancestor of `path` and re-append the
/// missing tail lexically, so a deletion request under a removed directory still
/// resolves.
fn canonicalize_nearest_existing(
    root: &Path,
    path: &Path,
    supplied: &Path,
) -> Result<PathBuf, GitError> {
    let mut missing: Vec<OsString> = Vec::new();
    let mut current = path.to_path_buf();
    loop {
        match fs::canonicalize(&current) {
            Ok(mut resolved) => {
                for name in missing.iter().rev() {
                    resolved.push(name);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                let Some(name) = current.file_name() else {
                    return Err(GitError::SafeCommitPathOutsideWorktree {
                        path: supplied.to_path_buf(),
                        worktree_root: root.to_path_buf(),
                    });
                };
                missing.push(name.to_os_string());
                match current.parent() {
                    Some(parent) if parent != current => current = parent.to_path_buf(),
                    _ => {
                        return Err(GitError::Io {
                            context: "resolve a safe-commit path",
                            source: error,
                        });
                    }
                }
            }
            Err(error) if is_symlink_loop(&error) => {
                return Err(GitError::SafeCommitPathLoop {
                    path: supplied.to_path_buf(),
                    worktree_root: root.to_path_buf(),
                });
            }
            Err(error) => {
                return Err(GitError::Io {
                    context: "resolve a safe-commit path",
                    source: error,
                });
            }
        }
    }
}

/// Refuse a path whose resolution loops. `symlink_metadata` does not follow the
/// final component, so the follow-up `metadata` is what detects a looped link.
fn refuse_symlink_loop(root: &Path, path: &GitPath) -> Result<(), GitError> {
    match fs::metadata(root.join(path.to_path_buf())) {
        Ok(_) => Ok(()),
        Err(error) if is_symlink_loop(&error) => Err(GitError::SafeCommitPathLoop {
            path: path.to_path_buf(),
            worktree_root: root.to_path_buf(),
        }),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(GitError::Io {
            context: "inspect a safe-commit path",
            source: error,
        }),
    }
}

fn canonical_worktree(worktree_root: &Path) -> Result<PathBuf, GitError> {
    fs::canonicalize(worktree_root).map_err(|source| GitError::Io {
        context: "canonicalise the safe-commit worktree",
        source,
    })
}

#[cfg(unix)]
fn path_to_git_bytes(relative: &Path) -> Result<Vec<u8>, GitError> {
    use std::os::unix::ffi::OsStrExt;
    Ok(relative.as_os_str().as_bytes().to_vec())
}

#[cfg(not(unix))]
fn path_to_git_bytes(relative: &Path) -> Result<Vec<u8>, GitError> {
    match relative.to_str() {
        Some(text) => Ok(text.replace('\\', "/").into_bytes()),
        None => Err(GitError::NonUtf8 {
            field: "safe-commit path",
        }),
    }
}

/// Whether an I/O error is the operating system's symlink-loop refusal.
///
/// Stable Rust does not expose `ErrorKind::FilesystemLoop` on this toolchain,
/// so the platform `ELOOP`/`ERROR_CANT_RESOLVE_FILENAME` value is compared. The
/// comparison is only used to choose a typed refusal; a platform without a
/// known value reports the generic I/O error instead of guessing.
fn is_symlink_loop(error: &std::io::Error) -> bool {
    #[cfg(target_os = "linux")]
    {
        error.raw_os_error() == Some(40)
    }
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    {
        error.raw_os_error() == Some(62)
    }
    #[cfg(windows)]
    {
        error.raw_os_error() == Some(1921)
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly",
        windows
    )))]
    {
        let _ = error;
        false
    }
}

fn validate_commit_message(message: &str) -> Result<(), GitError> {
    if message.is_empty() {
        return Err(GitError::SafeCommitInvalidRequest {
            detail: "a safe commit needs a non-empty message".to_string(),
        });
    }
    if message.len() > MAX_COMMIT_MESSAGE_LEN {
        return Err(GitError::SafeCommitInvalidRequest {
            detail: format!("the commit message exceeds the {MAX_COMMIT_MESSAGE_LEN}-byte limit"),
        });
    }
    if message.contains('\0') {
        return Err(GitError::SafeCommitInvalidRequest {
            detail: "the commit message must not contain NUL".to_string(),
        });
    }
    Ok(())
}

/// Normalise, de-duplicate (order-preserving), and shape-check one path list.
fn normalize_paths(root: &Path, requested: &[GitPath]) -> Result<Vec<GitPath>, GitError> {
    let mut normalized: Vec<GitPath> = Vec::with_capacity(requested.len());
    for path in requested {
        let path = normalize_safe_commit_path(root, path.to_path_buf())?;
        if !normalized.contains(&path) {
            normalized.push(path);
        }
    }
    Ok(normalized)
}

fn validate_path_policy(root: &Path, path: &GitPath) -> Result<(), GitError> {
    let first = Path::new(path.as_os_str()).components().next();
    if matches!(first, Some(Component::Normal(name)) if name == OsStr::new(WORKTREES_DIR)) {
        return Err(GitError::SafeCommitPathPolicy {
            path: root.join(path.to_path_buf()),
            worktree_root: root.to_path_buf(),
        });
    }
    Ok(())
}

fn validate_path_shape(repository: &GitRepository, path: &GitPath) -> Result<(), GitError> {
    let root = repository.root();
    let absolute = root.join(path.to_path_buf());
    match fs::symlink_metadata(&absolute) {
        Ok(metadata) if metadata.file_type().is_symlink() => Ok(()),
        Ok(metadata) if metadata.is_dir() => {
            if index_entry_mode(repository, path)? == Some("160000".to_string()) {
                Ok(())
            } else {
                Err(GitError::SafeCommitDirectoryArgument {
                    path: absolute,
                    worktree_root: root.to_path_buf(),
                })
            }
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) if is_symlink_loop(&error) => Err(GitError::SafeCommitPathLoop {
            path: absolute,
            worktree_root: root.to_path_buf(),
        }),
        Err(error) => Err(GitError::Io {
            context: "inspect a safe-commit path",
            source: error,
        }),
    }
}

/// The index mode of exactly `path`, or `None` when it is not tracked.
fn index_entry_mode(
    repository: &GitRepository,
    path: &GitPath,
) -> Result<Option<String>, GitError> {
    let output = repository.process().checked(&[
        OsStr::new("--literal-pathspecs"),
        OsStr::new("ls-files"),
        OsStr::new("--full-name"),
        OsStr::new("--stage"),
        OsStr::new("-z"),
        OsStr::new("--"),
        path.as_os_str(),
    ])?;
    for record in output.stdout.split(|byte| *byte == 0) {
        if record.is_empty() {
            continue;
        }
        let Some(tab) = record.iter().position(|byte| *byte == b'\t') else {
            continue;
        };
        let listed = GitPath::from_git_bytes(&record[tab + 1..])?;
        if &listed != path {
            continue;
        }
        let mode = record[..tab]
            .split(|byte| *byte == b' ')
            .next()
            .map(|mode| String::from_utf8_lossy(mode).into_owned());
        return Ok(mode);
    }
    Ok(None)
}

impl GitRepository {
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
        self.process().ensure_unredirected_write()?;
        validate_commit_message(&request.message)?;
        if request.paths.is_empty() && request.deletions.is_empty() {
            return Err(GitError::SafeCommitEmptyChangeset);
        }
        if !request.expected_bytes.is_empty() && request.expected_parent.is_none() {
            return Err(GitError::SafeCommitInvalidRequest {
                detail: "expected raw bytes require an expected parent".to_string(),
            });
        }

        let root = self.root();
        let paths = normalize_paths(root, &request.paths)?;
        let deletions = normalize_paths(root, &request.deletions)?;
        let conflicts: Vec<String> = deletions
            .iter()
            .filter(|path| paths.contains(path))
            .map(GitPath::display_lossy)
            .collect();
        if !conflicts.is_empty() {
            return Err(GitError::SafeCommitIndexDeletionConflict { paths: conflicts });
        }
        for path in paths.iter().chain(deletions.iter()) {
            validate_path_policy(root, path)?;
            validate_path_shape(self, path)?;
        }
        let expected_bytes = self.normalize_expected_bytes(request, &paths)?;

        let destination = request.destination.clone();
        let observed = self.current_branch()?;
        if observed.as_deref() != Some(destination.as_str()) {
            return Err(GitError::SafeCommitHeadMismatch {
                destination: destination.as_str().to_string(),
                observed_head: observed.unwrap_or_else(|| "<detached>".to_string()),
                worktree_root: root.to_path_buf(),
            });
        }
        let destination_tip = self.read_branch_tip(&destination)?.ok_or_else(|| {
            GitError::SafeCommitDestinationMissing {
                destination: destination.as_str().to_string(),
                worktree_root: root.to_path_buf(),
            }
        })?;
        let head_text =
            self.head_commit()?
                .ok_or_else(|| GitError::SafeCommitDestinationMissing {
                    destination: destination.as_str().to_string(),
                    worktree_root: root.to_path_buf(),
                })?;
        let head = Oid::parse(&head_text)?;
        if head != destination_tip {
            return Err(GitError::PlanStale {
                what: destination.as_ref_name().to_string(),
                detail: format!(
                    "HEAD is {head} but the branch tip is {destination_tip}; the checkout is inconsistent"
                ),
            });
        }
        if let Some(parent) = &request.expected_parent {
            if !self.commit_exists(parent)? {
                return Err(GitError::RefMissing {
                    name: parent.to_string(),
                });
            }
            if parent != &destination_tip {
                return Err(GitError::PlanStale {
                    what: destination.as_ref_name().to_string(),
                    detail: format!("expected parent {parent}, observed tip {destination_tip}"),
                });
            }
        }

        let guard = evaluate_commit_guard(&destination, &request.protected, request.capability);
        if !guard.allowed() {
            return Err(GitError::SafeCommitProtectedBranch {
                destination: destination.as_str().to_string(),
                worktree_root: root.to_path_buf(),
                capability: request.capability.as_str(),
            });
        }

        self.require_no_commit_operation()?;

        let route = match (&request.expected_parent, deletions.is_empty()) {
            (Some(_), _) => SafeCommitRoute::PinnedParent,
            (None, false) => SafeCommitRoute::IndexDeletions,
            (None, true) => SafeCommitRoute::Worktree,
        };
        Ok(SafeCommitPlan {
            destination,
            message: request.message.clone(),
            worktree_root: root.to_path_buf(),
            head,
            destination_tip,
            route,
            paths,
            deletions,
            expected_parent: request.expected_parent.clone(),
            expected_bytes,
            guard,
            index_authority: self.operator_index().clone(),
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
        if plan.worktree_root() != self.root() {
            return Err(GitError::InvalidPlan {
                detail: format!(
                    "safe-commit plan was prepared for {} but applied to {}",
                    plan.worktree_root().display(),
                    self.root().display()
                ),
            });
        }
        crate::safe_commit_transaction::execute(self, plan)
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
        request: &SafeCommitRequest,
        paths: &[GitPath],
    ) -> Result<Vec<ExpectedBlob>, GitError> {
        let root = self.root();
        let mut normalized: Vec<ExpectedBlob> = Vec::with_capacity(request.expected_bytes.len());
        for expected in &request.expected_bytes {
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
    if bounded.trim().is_empty() {
        "safe commit".to_string()
    } else {
        bounded
    }
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
        assert!(validate_commit_message("ok").is_ok());
        assert!(validate_commit_message("").is_err());
        assert!(validate_commit_message("has\0nul").is_err());
        assert!(validate_commit_message(&"x".repeat(MAX_COMMIT_MESSAGE_LEN + 1)).is_err());
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
