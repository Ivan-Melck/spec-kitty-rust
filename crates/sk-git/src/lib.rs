//! Read-only Git adapter for Spec Kitty Rust.
//!
//! `sk-git` answers questions about a checkout without changing it and without
//! contacting a remote. It is deliberately narrower than a general Git
//! library: the crate exposes repository discovery, the current branch, HEAD,
//! porcelain status, dirty paths, and the worktree listing. There is no push,
//! fetch, clone, commit, ref update, reset, checkout, worktree add/remove, or
//! arbitrary command surface.
//!
//! # Guarantees
//!
//! * **One subprocess seam.** Every invocation goes through a single internal
//!   function that takes a program and an argument list. Argument values are
//!   never interpolated into a shell string; repository paths are data, not
//!   syntax.
//! * **Bounded execution.** Each invocation has a finite timeout. On timeout
//!   the direct git child is killed and reaped before the error is returned.
//! * **No prompts.** Children get `GIT_TERMINAL_PROMPT=0`, a null stdin, and no
//!   pager. The adapter never runs a remote-contacting command, so credentials
//!   are never requested.
//! * **Read-only probes.** Status runs with `--no-optional-locks` and
//!   `GIT_OPTIONAL_LOCKS=0` so it does not refresh/write the index, and with
//!   `core.fsmonitor=false` so a repository-configured hook is not executed.
//! * **Repository identity is explicit.** Inherited `GIT_DIR`,
//!   `GIT_WORK_TREE`, `GIT_INDEX_FILE`, and related redirecting variables are
//!   removed, so a repository opened for a path is the repository queried.
//! * **Raw paths.** Porcelain output is parsed from `-z` records. Paths are
//!   retained as `OsString` bytes on Unix and never round-tripped through a
//!   lossy `String`.
//! * **Failure is never absence.** A failed command is a typed
//!   [`GitError`]; only a confirmed detached HEAD or unborn HEAD is `None`.
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
//!   hypothetical remote helper, are outside the bound. No API in this crate
//!   starts one.
//! * User and system Git configuration still affects behaviour (for example
//!   `core.autocrlf`, `status.showUntrackedFiles`, `safe.directory`, and
//!   attributes). The adapter does not rewrite configuration and does not claim
//!   byte-for-byte parity with every possible configuration.
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

mod error;
mod path;
mod process;
mod repository;
mod status;
mod worktree;

pub use error::GitError;
pub use path::GitPath;
pub use process::{DEFAULT_TIMEOUT, GitOptions};
pub use repository::GitRepository;
pub use status::{StatusEntry, StatusOptions, UntrackedMode};
pub use worktree::Worktree;
