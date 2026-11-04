//! Bounded Git adapter for Spec Kitty Rust.
//!
//! `sk-git` answers questions about a checkout and performs a narrow, planned
//! set of lifecycle mutations. It is deliberately narrower than a general Git
//! library: there is no push, fetch, clone, tag update, remote configuration
//! write, hook bypass, or caller-supplied argv. Every invocation goes through
//! one subprocess seam, and every mutation is a value that records the state it
//! observed and re-reads that state before writing.
//!
//! # Guarantees
//!
//! * **One subprocess seam.** Every invocation goes through a single internal
//!   function that takes a program and an argument list. Argument values are
//!   never interpolated into a shell string; repository paths are data, not
//!   syntax. Ref and object-id operands are validated types, and ref writes
//!   travel on `update-ref --stdin` where they cannot be parsed as options.
//! * **Bounded execution.** Each invocation has a finite timeout. On timeout
//!   the direct git child is killed and reaped before the error is returned.
//!   Captured stdout/stderr are bounded per stream (16 MiB by default; a bounded
//!   blob read may request its own limit up to the 64 MiB canonical status-log
//!   bound, which is enforced rather than truncated), and stdin payloads are
//!   bounded.
//! * **No prompts.** Children get `GIT_TERMINAL_PROMPT=0`, a null stdin (or an
//!   explicit bounded payload), and no pager. A remote read adds ssh
//!   `BatchMode=yes` when the effective transport is plain ssh, without
//!   replacing a user-pinned command.
//! * **Read-only probes by default.** Status runs with `--no-optional-locks`
//!   and `GIT_OPTIONAL_LOCKS=0` so it does not refresh/write the index, and
//!   with `core.fsmonitor=false` so a repository-configured hook is not
//!   executed.
//! * **Repository identity is explicit.** Inherited `GIT_DIR`,
//!   `GIT_WORK_TREE`, and related redirecting variables are removed, so a
//!   repository opened for a path is the repository queried. The one captured
//!   exception is `GIT_INDEX_FILE`: it is read once at open, bound to every
//!   operator-index read, write, lock check and reconciliation of that handle,
//!   and re-applied verbatim to child git — never silently retargeted to
//!   `<git-dir>/index` and never written into the process environment.
//! * **Raw paths.** Porcelain output is parsed from `-z` records. Paths are
//!   retained as `OsString` bytes on Unix and never round-tripped through a
//!   lossy `String`.
//! * **Failure is never absence.** A failed command is a typed
//!   [`GitError`]; only a confirmed detached HEAD or unborn HEAD is `None`. A
//!   corrupt HEAD (garbage, empty, an invalid ref, or an empty symbolic target)
//!   is [`GitError::UnreadableHead`], never a missing repository or a plain
//!   command failure.
//! * **Every mutation is planned.** Ref changes are compare-and-swap and carry
//!   the value the caller observed. Branch advances, branch deletions, and
//!   worktree removals are values that record their expected state and re-read
//!   it before writing. A move between plan and execution is
//!   [`GitError::PlanStale`] with the observed value, never an overwrite. A
//!   merge/revert/cherry-pick conclusion reports the commit **this call
//!   created**, proven from invocation-bound evidence (git's own summary or a
//!   reflog entry appended after the pre-invocation publication watermark on the
//!   pinned ref; a parent-set match is checked but is not creation proof), and a
//!   post-hook state that does not match the plan is
//!   [`GitError::MergeConclusionUnconfirmed`], never an unrelated HEAD.
//! * **Path-scoped commits.** A safe commit builds its tree in a private index
//!   seeded from a pinned parent, stages only the requested paths, runs the
//!   repository's hooks, creates a commit object against that parent, and
//!   advances the branch with a compare-and-swap. Unrelated staged, partially
//!   staged, unstaged, and untracked content is never read, moved, stashed, or
//!   restored; the real index is reconciled for the requested paths only after
//!   the commit exists.
//! * **Exact refs, including a genuinely absent destination.** The exact-ref
//!   family ([`SafeCommitPhysicalRequest`]) names the fully-qualified ref `HEAD`
//!   symbolically points at — `refs/x`, `refs/heads/HEAD`, `refs/heads/-name`, a
//!   trailing-NBSP branch — and the *planner* observes whether that ref is born
//!   or absent. An absent ref is published as a zero-parent root commit from
//!   git's own empty tree, through the same planner, hooks, object creation and
//!   one compare-and-swap: the ref transition is `RefChange::create`, i.e. "must
//!   be absent". A ref that appears between plan and apply is
//!   [`GitError::PlanStale`], never an overwrite; no parent, shadow or sentinel
//!   object is invented. The reflog subject is git's own (`commit: …` or
//!   `commit (initial): …`).
//!
//! # Operations
//!
//! Read-only: discovery, branch/HEAD, status, worktree listing, ref reads,
//! ancestry, divergence, git directories, in-progress markers, bounded
//! `ls-remote` probes (including a remote `HEAD`/default-branch read), merge
//! bases, unmerged conflict entries, and bounded typed tree/blob reads at a
//! validated revision and literal path.
//!
//! Mutating, each with an explicit plan: branch create, fast-forward (or
//! explicitly allowed non-fast-forward) advance with optional clean-checkout
//! resync, guarded branch delete with unique-commit analysis and optional tip
//! preservation, worktree add, guarded worktree remove, guarded prune of
//! proven-stale worktree registrations, pinned-head merge start with conflict
//! evidence and guarded abort, merge/revert/cherry-pick conclusion, per-worktree
//! sparse-checkout changes, and path-scoped safe commit (including index-only
//! removals, a pinned parent, and exact raw-bytes assertions).
//!
//! There is **no** push, fetch, clone, tag update, remote configuration write,
//! reset of an unplanned checkout, hook bypass, or arbitrary command surface.
//! Protected-branch configuration, workspace ownership, and placement/merge
//! policy are application-layer facts; this crate only consumes the facts it is
//! given and refuses when they are absent.
//!
//! # Limits
//!
//! * The adapter requires a Git work tree. Bare repositories are rejected with
//!   [`GitError::BareRepository`].
//! * Worktree listing uses `git worktree list --porcelain -z` (git 2.36+). On
//!   older versions it falls back to newline-delimited porcelain output; a
//!   worktree path containing a newline cannot be represented there.
//! * Repository-root discovery uses `git rev-parse --show-toplevel`, which
//!   cannot represent a root path ending in a newline. Branch and ref names are
//!   exposed as `String` and must be UTF-8; paths are not subject to that
//!   restriction on Unix.
//! * The timeout kills the direct git child only. Descendants, such as a
//!   remote helper, are outside the bound.
//! * A safe commit runs hooks with `git hook run`, which requires git >= 2.36;
//!   older Git is refused with [`GitError::Unsupported`] rather than skipping
//!   hooks. Its exact raw-bytes assertion is bounded by the seam's stdin limit.
//! * Commit messages are exact bytes ([`CommitMessage`]). The legacy
//!   `String` request/plan types and their `message() -> &str` getters are
//!   unchanged; [`SafeCommitTargetPlan::prepare`] produces the applicable
//!   exact-byte plan, and one execution body writes the payload to the message
//!   file so Git — not this crate — owns `i18n.commitEncoding`, conversion,
//!   hooks, cleanup, signing, and the UTF-8 warning. A commit message is bounded
//!   at 64 KiB and must not contain NUL, as before; the reference has no such
//!   length bound.
//! * A branch reflog passes bounded pre-conversion message bytes through Git's
//!   whitespace normalization. Whitespace-only pinned messages no longer get
//!   invented fallback text; existing UTF-8 APIs/getters are unchanged. On
//!   non-Unix platforms a reflog message must be
//!   valid UTF-8: a plan whose payload is not valid UTF-8 is refused with
//!   [`GitError::Unsupported`] before any index mutation, rather than
//!   pretending a wide-argv encoding was tested. A hook that rewrites a payload
//!   to non-UTF-8 on such a platform is not a claimed capability.
//! * User and system Git configuration still affects behaviour (for example
//!   `core.autocrlf`, `status.showUntrackedFiles`, `safe.directory`, and
//!   attributes). The adapter does not rewrite configuration and does not claim
//!   byte-for-byte parity with every possible configuration.
//! * The exact-ref family is the ordinary path-only route. Index-only removals, a
//!   pinned parent and exact raw-byte assertions remain on the short-branch API,
//!   and are refused for an unborn destination rather than faked. A destination
//!   no [`BranchName`] can express cannot be a member of a caller-supplied
//!   [`ProtectedBranches`] set either, so policy resolution for those names stays
//!   in the application layer; the guard decision itself is still the one core.
//! * Remote reads do not fetch. A freshness comparison against a listed tip
//!   requires that object to exist locally; otherwise the verdict is
//!   `compare_failed` with that explanation. Upstream fetches tracking refs
//!   before comparing.
//! * Passing tests establish the contracts they cover, not blanket
//!   compatibility with the Python implementation.
//!
//! # Example
//!
//! ```no_run
//! # fn main() -> Result<(), sk_git::GitError> {
//! let repository = sk_git::GitRepository::open(".")?;
//! assert!(repository.root().is_absolute());
//! let branch = repository.current_branch()?;
//! let dirty = repository.is_dirty()?;
//! println!("{} {:?} dirty={dirty}", repository.root().display(), branch);
//! # Ok(())
//! # }
//! ```

#![deny(missing_docs)]

mod branch;
mod error;
mod ignore;
mod index_authority;
mod merge;
mod merge_start;
mod message;
mod objects;
mod oid;
mod path;
mod process;
mod refname;
mod refs;
mod remote;
mod repo_state;
mod repository;
mod safe_commit;
mod safe_commit_paths;
mod safe_commit_physical;
mod safe_commit_transaction;
mod sparse;
mod status;
mod symbolic;
mod worktree;
mod worktree_ops;
mod worktree_prune;

pub use branch::{
    BOOKKEEPING_REF_PREFIX, BranchAdvanceOptions, BranchAdvanceOutcome, BranchAdvancePlan,
    BranchCreationPlan, BranchDeletionOptions, BranchDeletionOutcome, BranchDeletionPlan,
    BranchDeletionPreflight, ProtectedBranches,
};
pub use error::{GitError, MergeConclusionUnconfirmed, SafeCommitLandedKind};
pub use index_authority::{OperatorIndex, OperatorIndexSource};
pub use merge::{
    ConcludedOperation, MergeConclusionOutcome, MergeConclusionPlan, OperationMarkerState,
};
pub use merge_start::{
    MergeAbortOutcome, MergeAbortPlan, MergeStartOutcome, MergeStartPlan, MergeStartStatus,
};
pub use message::{CommitMessage, MessageOrigin};
pub use objects::{
    BlobLookup, DEFAULT_MAX_BLOB_BYTES, MAX_BLOB_BYTES, SymbolicHeadObservation, TreeEntry,
    TreeEntryKind, UnmergedPath, UnmergedStage,
};
pub use oid::Oid;
pub use path::GitPath;
pub use process::{DEFAULT_TIMEOUT, GitOptions};
pub use refname::{BranchName, RefName, StartPoint};
pub use refs::{RefChange, RefState, RefTransactionOutcome, RefTransactionPlan};
pub use remote::{
    AheadBehind, FreshnessState, FreshnessVerdict, RemoteBranchLookup, RemoteHead,
    RemoteHeadTarget, RemoteHeads, RemoteName,
};
pub use repo_state::{GitDirs, InProgressOperation, MERGE_TREE_WRITE_TREE_MIN_VERSION};
pub use repository::GitRepository;
pub use safe_commit::{
    CommitGuardCapability, CommitGuardVerdict, ExpectedBlob, MAX_COMMIT_MESSAGE_LEN,
    SafeCommitMessagePlan, SafeCommitOutcome, SafeCommitPlan, SafeCommitPlanFailure,
    SafeCommitPreflightStage, SafeCommitRecovery, SafeCommitRequest, SafeCommitRoute,
    SafeCommitTargetPlan, SafeCommitTargetRequest, WORKTREES_DIR, evaluate_commit_guard,
    evaluate_commit_guard_decision, normalize_safe_commit_path,
};
pub use safe_commit_paths::{
    SourceCommitPath, SourcePathRefusal, plan_source_commit_path, source_preflight_path,
};
pub use safe_commit_physical::{
    SafeCommitPhysicalGuardVerdict, SafeCommitPhysicalMessagePlan, SafeCommitPhysicalOutcome,
    SafeCommitPhysicalPlan, SafeCommitPhysicalRequest,
};
pub use sparse::{
    MAX_SPARSE_PATTERN_FILE_BYTES, SPARSE_CHECKOUT_MIN_VERSION, SparseCheckoutMode,
    SparseCheckoutOutcome, SparseCheckoutPlan, SparseCheckoutState,
};
pub use status::{StatusEntry, StatusOptions, UntrackedMode};
pub use worktree::Worktree;
pub use worktree_ops::{
    WorktreeAddOutcome, WorktreeAddPlan, WorktreeDiscardPolicy, WorktreeRemovalOutcome,
    WorktreeRemovalPlan, WorktreeStart,
};
pub use worktree_prune::{
    WORKTREE_PRUNE_EXPIRE_MIN_VERSION, WorktreePruneEntry, WorktreePruneOutcome, WorktreePrunePlan,
};
