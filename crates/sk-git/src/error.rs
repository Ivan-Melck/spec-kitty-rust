//! Typed failures for the read-only Git adapter.
//!
//! Absence is not an error: a detached HEAD is `Ok(None)` from
//! [`GitRepository::current_branch`](crate::GitRepository::current_branch), and
//! an unborn repository is `Ok(None)` from
//! [`GitRepository::head_commit`](crate::GitRepository::head_commit). Every
//! other inability to answer is one of the variants below, so a caller can
//! never mistake "git could not answer" for "the answer is empty".

use std::io;
use std::path::PathBuf;
use std::time::Duration;

/// A failure while inspecting a Git repository.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    /// The selected repository/index cannot yet share captured writer authority.
    #[error(
        "inherited {variable} is not supported for this write; clear repository selectors rather than silently retargeting the mutation"
    )]
    RepositoryEnvironmentRefused {
        /// The selector name; its potentially sensitive value is never logged.
        variable: &'static str,
    },

    /// The configured git executable could not be found.
    ///
    /// This is distinct from a git command that ran and exited non-zero: the
    /// adapter did not manage to start git at all.
    #[error("git executable {program} is unavailable: {source}")]
    GitUnavailable {
        /// The program the adapter tried to execute.
        program: PathBuf,
        /// The operating-system error that prevented execution.
        #[source]
        source: io::Error,
    },

    /// Git was found but could not be started (for example a permission error).
    #[error("could not start {program} in {cwd}: {source}")]
    Spawn {
        /// The program the adapter tried to execute.
        program: PathBuf,
        /// The directory git was meant to run in.
        cwd: PathBuf,
        /// The operating-system error.
        #[source]
        source: io::Error,
    },

    /// Git ran but exited with a status the adapter does not accept.
    #[error("`git {args:?}` failed in {cwd} (status {status:?}): {stderr}")]
    CommandFailed {
        /// Arguments passed to git, for diagnostics only.
        args: Vec<String>,
        /// The directory git ran in.
        cwd: PathBuf,
        /// The process exit code, or `None` when the process was signalled.
        status: Option<i32>,
        /// Git's standard error, lossily decoded and trimmed.
        stderr: String,
    },

    /// Git did not finish before the configured timeout.
    ///
    /// The direct child was killed and reaped before this error was returned.
    /// Descendants of git (for example a remote helper) are not signalled; the
    /// adapter never runs remote-contacting commands, so this is a bounded
    /// local-process concern only.
    #[error("`git {args:?}` in {cwd} timed out after {timeout:?}; the child was killed and reaped")]
    Timeout {
        /// Arguments passed to git, for diagnostics only.
        args: Vec<String>,
        /// The directory git ran in.
        cwd: PathBuf,
        /// The configured bound.
        timeout: Duration,
    },

    /// Captured stdout or stderr exceeded the per-stream byte budget.
    #[error("git {stream} exceeds the {limit}-byte capture limit")]
    OutputTooLarge {
        /// The captured stream: `stdout` or `stderr`.
        stream: &'static str,
        /// Maximum captured bytes for one stream.
        limit: u64,
    },

    /// The path is not inside a Git work tree.
    ///
    /// The path may not exist, may not be in a repository at all, or may point
    /// at repository metadata that has no work tree.
    #[error("{path} is not inside a Git work tree")]
    NotARepository {
        /// The path (or its canonicalised directory) the adapter probed.
        path: PathBuf,
    },

    /// The path is a bare repository, which has no work tree to inspect.
    #[error("{path} is a bare repository; this adapter requires a work tree")]
    BareRepository {
        /// The path the adapter probed.
        path: PathBuf,
    },

    /// A local I/O operation used by the subprocess seam failed.
    #[error("could not {context}: {source}")]
    Io {
        /// What the adapter was doing.
        context: &'static str,
        /// The operating-system error.
        #[source]
        source: io::Error,
    },

    /// Git produced output the adapter could not interpret.
    #[error("unexpected git output: {detail}")]
    InvalidOutput {
        /// What was wrong with the output.
        detail: String,
    },

    /// A caller-supplied lifecycle plan is internally invalid.
    ///
    /// Nothing was written; the detail names the validation that failed.
    #[error("invalid Git lifecycle plan: {detail}")]
    InvalidPlan {
        /// What is wrong with the plan.
        detail: String,
    },

    /// Git produced bytes where this API promises UTF-8 text.
    ///
    /// Paths never take this path: they are retained as raw bytes on Unix and
    /// as UTF-8 `OsString` values elsewhere. Branch and ref names are exposed
    /// as `String` and therefore require valid UTF-8.
    #[error("git returned non-UTF-8 {field}")]
    NonUtf8 {
        /// The field that was not valid UTF-8.
        field: &'static str,
    },

    /// A value is not a valid object id for this adapter.
    ///
    /// Only a full, lowercase-or-uppercase 40-hex (SHA-1) or 64-hex
    /// (SHA-256) object id is accepted. Abbreviated ids and revspec suffixes
    /// are refused so a caller cannot smuggle git syntax through an id.
    #[error("not a full Git object id: {value:?} ({detail})")]
    InvalidOid {
        /// The rejected value.
        value: String,
        /// What is wrong with it.
        detail: &'static str,
    },

    /// A string is not a valid, fully-qualified or short Git ref name.
    ///
    /// Validation follows `git check-ref-format`'s structural rules and, in
    /// addition, refuses a leading `-` so a name can never be read as an
    /// option by a git command.
    #[error("not a valid {kind}: {name:?} ({detail})")]
    InvalidRefName {
        /// `branch name` or `ref name`.
        kind: &'static str,
        /// The rejected value.
        name: String,
        /// What is wrong with it.
        detail: &'static str,
    },

    /// A remote name is not a plain configured-remote name.
    ///
    /// URLs, scp-like transports (`host:path`), `ext::` helpers and any
    /// leading `-` are refused: remote contact in this crate is restricted to
    /// a name git config already associates with a transport.
    #[error("not a plain remote name: {name:?}")]
    InvalidRemoteName {
        /// The rejected value.
        name: String,
    },

    /// A path cannot be used safely by a worktree operation.
    #[error("unsafe worktree path {path}: {detail}")]
    UnsafePath {
        /// The rejected path.
        path: PathBuf,
        /// Why it was refused.
        detail: String,
    },

    /// A remote could not be asked.
    ///
    /// This is deliberately distinct from a clean "the remote answered and the
    /// ref is absent" outcome. A caller must never read a failed contact as
    /// absence.
    #[error("remote {remote} could not be asked: {detail}")]
    RemoteUnreachable {
        /// The remote name the probe named.
        remote: String,
        /// The transport or git diagnostic.
        detail: String,
        /// Whether the contact hit the configured timeout.
        timed_out: bool,
        /// The bound that applied when `timed_out` is true.
        timeout: Option<Duration>,
    },

    /// A prepared mutation's expected state no longer holds.
    ///
    /// Nothing was written. The detail describes what moved; callers use it as
    /// recovery evidence and re-plan rather than retrying blindly.
    #[error("{what} changed after the plan was prepared: {detail}")]
    PlanStale {
        /// The object whose state moved (a ref, worktree, or checkout).
        what: String,
        /// What was expected and what is now observed.
        detail: String,
    },

    /// A branch or other mutation target is in the caller-supplied protected set.
    #[error(
        "refusing to modify protected branch {branch:?}; protected-branch policy is resolved by the application layer and this plan carried no explicit override"
    )]
    ProtectedTarget {
        /// The protected branch name.
        branch: String,
    },

    /// A branch advance was asked to move backwards or sideways.
    #[error("refusing a non-fast-forward advance of {branch:?} ({old} -> {new})")]
    NonFastForward {
        /// The branch name.
        branch: String,
        /// The current tip.
        old: String,
        /// The requested tip.
        new: String,
    },

    /// A branch ref moved but a checkout could not be resynchronised to it.
    ///
    /// The ref write succeeded, so this is not a plain refusal: the named
    /// checkout is behind its own HEAD and needs repair.
    #[error("advanced the ref but could not resync {checkout}: {detail}")]
    ResyncFailed {
        /// The checkout that failed to resync.
        checkout: PathBuf,
        /// Git's diagnostic.
        detail: String,
    },

    /// A branch is checked out in one or more worktrees.
    #[error("branch {branch:?} is checked out in {worktrees:?}")]
    BranchCheckedOut {
        /// The branch name.
        branch: String,
        /// Every worktree that has it checked out.
        worktrees: Vec<PathBuf>,
    },

    /// A destructive branch operation would discard commits that exist nowhere else.
    #[error("branch {branch:?} holds {commits:?} which exist on no other ref")]
    BranchHasUniqueCommits {
        /// The branch name.
        branch: String,
        /// The unique commit ids.
        commits: Vec<String>,
    },

    /// A branch that a plan expected to create already exists.
    #[error("branch {branch:?} already exists at {tip}")]
    BranchAlreadyExists {
        /// The branch name.
        branch: String,
        /// Its current tip.
        tip: String,
    },

    /// A branch that a plan expected to exist is absent.
    #[error("branch {branch:?} does not exist")]
    BranchMissing {
        /// The branch name.
        branch: String,
    },

    /// A ref or start point does not resolve to a commit.
    #[error("{name:?} does not resolve to a commit")]
    RefMissing {
        /// The ref, object id, or `HEAD` that was requested.
        name: String,
    },

    /// A checkout holds local state a destructive operation would discard.
    #[error("{path} holds local state a destructive operation would discard: {entries:?}")]
    DirtyWorktree {
        /// The checkout that was scanned.
        path: PathBuf,
        /// One display line per offending entry.
        entries: Vec<String>,
    },

    /// A worktree path is not registered with this repository.
    #[error("{path} is not a registered worktree of this repository")]
    WorktreeNotRegistered {
        /// The path that was looked for.
        path: PathBuf,
    },

    /// A worktree path cannot be allocated.
    #[error("refusing to allocate worktree {path}: {detail}")]
    WorktreePathUnavailable {
        /// The requested path.
        path: PathBuf,
        /// Why it is unavailable.
        detail: String,
    },

    /// `git worktree add` failed.
    ///
    /// The flags report what git appears to have left behind, so a caller can
    /// recover instead of guessing: a branch may exist without a registration,
    /// or a directory may exist without either.
    #[error("git worktree add failed for {path} (branch {branch}): {detail}")]
    WorktreeAddFailed {
        /// The requested worktree path.
        path: PathBuf,
        /// The branch the plan named.
        branch: String,
        /// Git's diagnostic plus post-failure observations.
        detail: String,
        /// Whether the branch exists after the failure.
        branch_created: bool,
        /// Whether a worktree registration for the path exists after the failure.
        registered: bool,
    },

    /// `git worktree remove` failed.
    #[error("git worktree remove failed for {path}: {detail}")]
    WorktreeRemoveFailed {
        /// The worktree path.
        path: PathBuf,
        /// Git's diagnostic.
        detail: String,
        /// Whether the worktree is still registered after the failure.
        still_registered: bool,
    },

    /// A git operation (merge, rebase, cherry-pick, revert, or a held index
    /// lock) is in progress.
    #[error("a Git {operation} is in progress in this repository; refusing the mutation")]
    OperationInProgress {
        /// The detected operation.
        operation: String,
    },

    /// There is no in-progress operation to conclude.
    #[error("no merge, revert, or cherry-pick is in progress")]
    NoOperationInProgress,

    /// A requested native primitive is not implemented by this adapter.
    #[error("unsupported Git operation: {feature}")]
    Unsupported {
        /// The operation that was refused rather than simulated.
        feature: &'static str,
    },

    /// `HEAD` exists but cannot be read as a commit.
    ///
    /// This is deliberately distinct from an unborn branch: an unborn `HEAD`
    /// names a branch ref that does not exist yet (a clean "no revision"), while
    /// a `HEAD` whose content is malformed, names a structurally invalid ref, or
    /// points at a missing/non-commit object is unreadable. Callers such as a
    /// rollback-point capture must not treat the second as an absence.
    #[error("HEAD is present but unreadable: {detail}")]
    UnreadableHead {
        /// What was wrong with `HEAD`.
        detail: String,
    },

    /// A requested blob is larger than the caller's bound.
    ///
    /// Nothing was read into memory: the size is asked of git first
    /// (`git cat-file -s`) and the refusal names the object and the bound.
    #[error("blob for {path} is larger than the {limit}-byte read limit")]
    BlobTooLarge {
        /// The repository-relative path that was requested.
        path: String,
        /// The caller's bound.
        limit: u64,
    },

    /// A worktree registration was requested for pruning but is not proven
    /// stale.
    ///
    /// A registration is only prunable when git itself reports it prunable and
    /// its directory is gone; a live checkout is never removed.
    #[error("refusing to prune worktree registration {path}: {detail}")]
    WorktreeNotStale {
        /// The registration path.
        path: PathBuf,
        /// Why it is not proven stale.
        detail: String,
    },

    /// A merge start, conclusion, or abort observed state it did not pin.
    ///
    /// Unlike [`GitError::PlanStale`] this can be raised *after* a mutation, so
    /// the caller must inspect the named state instead of assuming nothing
    /// happened.
    #[error("merge state in {worktree_root} changed outside the pinned plan: {detail}")]
    MergeStateChanged {
        /// The checkout whose merge state moved.
        worktree_root: PathBuf,
        /// What moved.
        detail: String,
    },

    /// `git merge --abort` could not restore the pinned state.
    #[error("git merge --abort failed in {worktree_root}: {detail}")]
    MergeAbortFailed {
        /// The checkout whose merge could not be aborted.
        worktree_root: PathBuf,
        /// Git's diagnostic.
        detail: String,
    },

    /// A merge/revert/cherry-pick conclusion commit may have landed, but the
    /// state after it is not the state the plan pinned.
    ///
    /// Unlike [`GitError::CommandFailed`] this is raised *after* the commit
    /// command ran, so the caller must inspect the named state instead of
    /// assuming nothing happened. Unlike [`GitError::MergeAbortFailed`] it is
    /// not a rollback: nothing is reset, deleted, or reverted here. The boxed
    /// payload keeps the common error path small.
    #[error("{0}")]
    MergeConclusionUnconfirmed(Box<MergeConclusionUnconfirmed>),

    /// A sparse-checkout operation could not be completed by git.
    ///
    /// Git itself refuses to remove locally modified paths; this error carries
    /// its diagnostic so the caller can name the obstruction. The checkout's
    /// untracked and ignored files are never removed by this adapter.
    #[error("sparse checkout failed in {worktree_root}: {detail}")]
    SparseCheckoutFailed {
        /// The checkout that was being changed.
        worktree_root: PathBuf,
        /// Git's diagnostic.
        detail: String,
    },

    /// A sparse-checkout disable was requested on a checkout that is not sparse.
    #[error("sparse checkout is not enabled in {worktree_root}")]
    SparseCheckoutNotConfigured {
        /// The checkout that was inspected.
        worktree_root: PathBuf,
    },

    /// A sparse-checkout pattern file exceeded the bounded read limit.
    #[error("sparse-checkout pattern file {path} exceeds the {limit}-byte limit")]
    SparsePatternFileTooLarge {
        /// The pattern file that was read.
        path: PathBuf,
        /// The read bound.
        limit: u64,
    },

    /// A bounded stdin payload exceeded the adapter's limit.
    #[error("git stdin payload exceeds the {limit}-byte limit")]
    InputTooLarge {
        /// Maximum bytes written to a child's stdin.
        limit: u64,
    },

    // -----------------------------------------------------------------
    // Path-scoped safe-commit refusals. Every variant below is raised
    // BEFORE any repository mutation unless its documentation says the
    // commit already landed, so a caller can distinguish "refused, nothing
    // changed" from "landed, follow-up needs repair".
    // -----------------------------------------------------------------
    /// A safe commit was requested with no paths and no index deletions.
    #[error("refusing an empty safe commit: at least one path is required")]
    SafeCommitEmptyChangeset,

    /// The checkout's HEAD is not the declared destination branch.
    #[error(
        "safe commit worktree {worktree_root} has HEAD {observed_head:?}, expected branch {destination:?}"
    )]
    SafeCommitHeadMismatch {
        /// The branch the caller declared.
        destination: String,
        /// The branch checked out now, or `<detached>`.
        observed_head: String,
        /// The checkout that was inspected.
        worktree_root: PathBuf,
    },

    /// The declared destination branch does not exist in this repository.
    #[error("safe commit destination branch {destination:?} does not exist in {worktree_root}")]
    SafeCommitDestinationMissing {
        /// The branch the caller declared.
        destination: String,
        /// The checkout that was inspected.
        worktree_root: PathBuf,
    },

    /// A requested path is a real directory, not a file, symlink, gitlink, or deletion.
    #[error(
        "refusing directory path {path} in a safe commit; expand it to its files first ({worktree_root})"
    )]
    SafeCommitDirectoryArgument {
        /// The path that is a directory.
        path: PathBuf,
        /// The checkout that was inspected.
        worktree_root: PathBuf,
    },

    /// A requested path is under `.worktrees/`.
    #[error(
        "refusing to stage {path} from {worktree_root}: `.worktrees/` paths are coordination-worktree artefacts"
    )]
    SafeCommitPathPolicy {
        /// The refused path.
        path: PathBuf,
        /// The checkout the path was resolved against.
        worktree_root: PathBuf,
    },

    /// An absolute requested path does not resolve inside the worktree.
    #[error("safe commit path {path} is outside worktree {worktree_root}")]
    SafeCommitPathOutsideWorktree {
        /// The path as supplied.
        path: PathBuf,
        /// The checkout the path was resolved against.
        worktree_root: PathBuf,
    },

    /// Resolving a requested path hit a symlink loop.
    #[error("refusing safe commit path {path}: symlink loop ({worktree_root})")]
    SafeCommitPathLoop {
        /// The path whose resolution looped.
        path: PathBuf,
        /// The checkout the path was resolved against.
        worktree_root: PathBuf,
    },

    /// The destination denies a safe commit for the asserted capability.
    #[error(
        "refusing to commit to protected branch {destination:?} in {worktree_root}: capability {capability:?} authorizes no protected-branch flow"
    )]
    SafeCommitProtectedBranch {
        /// The protected destination branch.
        destination: String,
        /// The checkout that would receive the commit.
        worktree_root: PathBuf,
        /// The capability asserted at the call site.
        capability: &'static str,
    },

    /// A path was requested for commit and also listed as an index deletion.
    #[error("safe commit path(s) both requested and listed as index deletions: {paths:?}")]
    SafeCommitIndexDeletionConflict {
        /// The conflicting repository-relative paths.
        paths: Vec<String>,
    },

    /// A safe-commit request is internally inconsistent.
    #[error("invalid safe commit request: {detail}")]
    SafeCommitInvalidRequest {
        /// What is inconsistent.
        detail: String,
    },

    /// The requested paths produce no change against HEAD (a benign no-op).
    #[error(
        "nothing to commit for safe commit destination {destination:?}: the requested paths already match HEAD ({detail})"
    )]
    SafeCommitNothingToCommit {
        /// The destination branch.
        destination: String,
        /// The evidence that classified the outcome as a no-op.
        detail: String,
    },

    /// Git refused to stage one requested path.
    #[error("safe commit could not stage {path}: {detail}")]
    SafeCommitStageFailed {
        /// The repository-relative path that failed.
        path: PathBuf,
        /// Git's diagnostic.
        detail: String,
    },

    /// Candidate creation succeeded, but ref advancement could not be confirmed.
    /// Hooks or another writer may have changed history; inspect before retrying.
    #[error(
        "safe commit candidate {candidate} has an unconfirmed advance on {destination} (observed {observed_tip:?}): {detail}"
    )]
    SafeCommitAdvanceUnconfirmed {
        /// The requested branch.
        destination: String,
        /// The immutable candidate object created by this operation.
        candidate: crate::Oid,
        /// The branch tip read after the error, if it could be read.
        observed_tip: Option<crate::Oid>,
        /// Original failure and any readback diagnostic.
        detail: String,
    },

    /// Git refused to create the commit for a reason other than an empty changeset.
    #[error("safe commit failed on {destination} (status {status:?}): {detail}")]
    SafeCommitCommitFailed {
        /// The destination branch.
        destination: String,
        /// The process exit code, or `None` when signalled.
        status: Option<i32>,
        /// Git's combined output.
        detail: String,
    },

    /// A commit hook rejected the pinned-parent commit.
    #[error("safe commit hook {hook:?} rejected the commit: {detail}")]
    SafeCommitHookRejected {
        /// The hook name.
        hook: String,
        /// Git's diagnostic.
        detail: String,
    },

    /// A commit hook changed the requested path contents in the isolated index.
    #[error(
        "refusing pinned-parent safe commit: a commit hook changed the requested path contents after they were staged"
    )]
    SafeCommitHookChangedPaths,

    /// An exact-bytes assertion did not match the staged blob.
    #[error("safe commit expected raw bytes for {path} do not match the staged blob: {detail}")]
    SafeCommitExpectedBytesMismatch {
        /// The repository-relative path.
        path: String,
        /// What differed.
        detail: String,
    },

    /// The commit DID land, but a post-landing readback or index resync failed.
    ///
    /// This is not a refusal: `recovery.commit()` names the commit object that
    /// exists and `recovery.affected_paths()` names the paths whose index state
    /// needs repair. Nothing is reset, deleted, or rolled back automatically.
    #[error(
        "safe commit {commit} landed on {destination} in {worktree_root}, but {detail}; \
         repair the index for {paths:?} against that commit (no state was reset automatically)",
        commit = recovery.commit(),
        paths = recovery.affected_paths()
    )]
    SafeCommitLanded {
        /// Which post-landing step failed.
        kind: SafeCommitLandedKind,
        /// The destination branch.
        destination: String,
        /// The checkout whose index needs repair.
        worktree_root: PathBuf,
        /// What failed.
        detail: String,
        /// The landed commit and the paths whose index state needs repair.
        ///
        /// Boxed so the common error path stays small.
        recovery: Box<crate::safe_commit::SafeCommitRecovery>,
    },
}

/// Which post-landing safe-commit step failed after the commit object was created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SafeCommitLandedKind {
    /// The real index could not be resynchronised for the committed paths.
    IndexResyncFailed,
    /// The real index still stages a path the commit already carries.
    IndexResidue,
    /// The commit carries paths the operation never requested.
    UnrequestedPaths,
    /// The commit's own changed-path set could not be read back.
    CommitPathReadbackFailed,
}

/// Evidence for a conclusion commit that may have landed without a confirmed
/// post-state.
///
/// * `candidate` is a commit **this call created**, proven from evidence bound to
///   the invocation: git's own summary printed by the child, or a reflog entry
///   appended to the pinned ref after a publication watermark captured before
///   the command ran. Its parent set is checked as a necessary condition, never
///   as creation proof, and the observed `HEAD` is never accepted merely because
///   its parents match. `candidate` is `None` when no evidence could prove a
///   created commit; `None` means *unknown*, not `not landed`.
/// * `observed_head` is what `HEAD` reads back as afterwards. A hook may have
///   moved it, so it is reported separately and is never substituted for
///   `candidate`.
/// * `operation_consumed` reports whether the operation marker (`MERGE_HEAD`,
///   `CHERRY_PICK_HEAD`, `REVERT_HEAD`) is gone; `marker` distinguishes a
///   consumed marker from one that is unchanged, rewritten, or unreadable.
#[derive(Debug)]
pub struct MergeConclusionUnconfirmed {
    /// `merge`, `cherry-pick`, or `revert`.
    pub operation: String,
    /// The checkout whose conclusion was attempted.
    pub worktree_root: PathBuf,
    /// The commit this call created, when invocation-bound evidence proved it.
    pub candidate: Option<crate::Oid>,
    /// The HEAD the plan pinned.
    pub previous_head: crate::Oid,
    /// `HEAD` as read back after the command and its hooks.
    pub observed_head: Option<crate::Oid>,
    /// Whether the operation marker is gone.
    pub operation_consumed: bool,
    /// What the operation marker reads back as (consumed, unchanged, changed,
    /// or unreadable).
    pub marker: crate::merge::OperationMarkerState,
    /// Git's diagnostic plus the evidence probes that were tried.
    pub detail: String,
}

impl std::fmt::Display for MergeConclusionUnconfirmed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "a {} conclusion in {} is unconfirmed (created commit {:?}, observed HEAD {:?}, operation marker {:?}): {}; no state was rolled back",
            self.operation,
            self.worktree_root.display(),
            self.candidate,
            self.observed_head,
            self.marker,
            self.detail
        )
    }
}
