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
}
