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

    /// A bounded stdin payload exceeded the adapter's limit.
    #[error("git stdin payload exceeds the {limit}-byte limit")]
    InputTooLarge {
        /// Maximum bytes written to a child's stdin.
        limit: u64,
    },
}
