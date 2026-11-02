//! Repository discovery and read-only queries.
//!
//! [`GitRepository::open`] resolves a path to its work-tree root and binds a
//! single [`GitProcess`] to it. Every query on the resulting value is a fixed,
//! read-only git invocation; no method accepts caller-supplied arguments,
//! pathspecs, revisions, or remote names, so the adapter cannot be turned into
//! a write or network surface.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::GitError;
use crate::path::{os_string_from_git_bytes, strip_line_feed, utf8_field};
use crate::process::{GitOptions, GitProcess};
use crate::status::{StatusEntry, StatusOptions};
use crate::worktree::{Worktree, parse_worktree_lines, parse_worktree_z};

/// A Git work tree discovered from a filesystem path.
///
/// The value holds the canonicalised work-tree root and a reusable subprocess
/// binding. It performs no writes and never contacts a remote.
///
/// # Example
///
/// ```no_run
/// # fn main() -> Result<(), sk_git::GitError> {
/// let repository = sk_git::GitRepository::open(".")?;
/// println!("root: {}", repository.root().display());
/// if let Some(branch) = repository.current_branch()? {
///     println!("branch: {branch}");
/// }
/// println!("dirty: {}", repository.is_dirty()?);
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct GitRepository {
    root: PathBuf,
    process: GitProcess,
    options: GitOptions,
}

impl GitRepository {
    /// Open the repository that contains `path`, using [`GitOptions::default`].
    ///
    /// `path` may be a directory inside a work tree or a file whose parent is
    /// inside one. The path is canonicalised before git is consulted, so
    /// `.`/`..` and symlinks resolve consistently. A path outside any work
    /// tree, a missing path, or a bare repository is a typed error; it is never
    /// reported as an empty repository.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, GitError> {
        Self::open_with_options(path, &GitOptions::default())
    }

    /// Open the repository that contains `path` with explicit options.
    ///
    /// This is [`GitRepository::open`] plus a configurable git executable and
    /// per-invocation timeout. The timeout applies to discovery itself and to
    /// every later query.
    pub fn open_with_options(
        path: impl AsRef<Path>,
        options: &GitOptions,
    ) -> Result<Self, GitError> {
        let supplied = path.as_ref();
        let probe = probe_directory(supplied)?;
        let cwd = fs::canonicalize(&probe).map_err(|source| match source.kind() {
            std::io::ErrorKind::NotFound => GitError::NotARepository {
                path: supplied.to_path_buf(),
            },
            _ => GitError::Io {
                context: "canonicalise the path to open",
                source,
            },
        })?;

        let discovery = GitProcess::new(&cwd, options);
        let inside_args = ["rev-parse", "--is-inside-work-tree"];
        let inside = discovery.run(&inside_args)?;
        if !inside.status.success() {
            // Git's "not a repository" diagnostic is the only failure that
            // means "absent". Any other non-zero exit stays a command failure,
            // so a broken probe is never mistaken for a non-repository.
            let stderr = String::from_utf8_lossy(&inside.stderr).to_ascii_lowercase();
            if stderr.contains("not a git repository") || stderr.contains("not a work tree") {
                return Err(GitError::NotARepository {
                    path: supplied.to_path_buf(),
                });
            }
            return Err(discovery.failed(&inside_args, &inside));
        }

        match strip_line_feed(&inside.stdout) {
            b"true" => {}
            b"false" => {
                // A bare repository and a path inside repository metadata both
                // answer `false`; a second read-only probe decides which typed
                // error applies.
                let bare = discovery.checked(&["rev-parse", "--is-bare-repository"])?;
                return Err(if strip_line_feed(&bare.stdout) == b"true" {
                    GitError::BareRepository {
                        path: supplied.to_path_buf(),
                    }
                } else {
                    GitError::NotARepository {
                        path: supplied.to_path_buf(),
                    }
                });
            }
            other => {
                return Err(GitError::InvalidOutput {
                    detail: format!(
                        "git rev-parse --is-inside-work-tree answered {:?}",
                        String::from_utf8_lossy(other)
                    ),
                });
            }
        }

        let top = discovery.checked(&["rev-parse", "--show-toplevel"])?;
        let raw_root = strip_line_feed(&top.stdout);
        if raw_root.is_empty() {
            return Err(GitError::InvalidOutput {
                detail: "git rev-parse --show-toplevel returned an empty path".to_string(),
            });
        }
        let root_os = os_string_from_git_bytes(raw_root, "repository root")?;
        let root = fs::canonicalize(PathBuf::from(root_os)).map_err(|source| GitError::Io {
            context: "canonicalise the repository root",
            source,
        })?;

        Ok(Self {
            process: GitProcess::new(&root, options),
            root,
            options: options.clone(),
        })
    }

    /// The absolute, canonicalised work-tree root of this repository.
    ///
    /// For a linked worktree this is that checkout's own root, not the main
    /// checkout's.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The options that bound every invocation made by this repository handle.
    ///
    /// Lifecycle plans carry these options so a linked checkout is inspected
    /// with the same git program and the same timeout as the handle that
    /// produced the plan.
    pub fn options(&self) -> &GitOptions {
        &self.options
    }

    /// The subprocess binding at this repository's root.
    pub(crate) fn process(&self) -> &GitProcess {
        &self.process
    }

    /// A subprocess binding at another checkout of the same repository.
    ///
    /// Used by preflights that inspect a linked worktree (its status is its
    /// own) while sharing this handle's program and timeout.
    pub(crate) fn process_at(&self, root: impl Into<PathBuf>) -> GitProcess {
        GitProcess::new(root, &self.options)
    }

    /// The short branch name currently checked out, or `None` for a detached
    /// HEAD.
    ///
    /// An unborn branch (a fresh repository with no commits) still has a
    /// symbolic HEAD and is reported as `Some("main")`-style value.
    ///
    /// # Errors
    ///
    /// A failure to ask git, or a branch name that is not valid UTF-8, is an
    /// error. Only a confirmed non-symbolic HEAD is reported as `None`.
    pub fn current_branch(&self) -> Result<Option<String>, GitError> {
        let output = self
            .process
            .allowing(&["symbolic-ref", "--quiet", "--short", "HEAD"], &[1])?;
        if output.status.code() != Some(0) {
            return Ok(None);
        }

        let raw = strip_line_feed(&output.stdout);
        if raw.is_empty() {
            return Err(GitError::InvalidOutput {
                detail: "git symbolic-ref returned an empty branch name".to_string(),
            });
        }
        Ok(Some(utf8_field(raw, "branch name")?))
    }

    /// The commit HEAD resolves to, or `None` in an unborn repository.
    ///
    /// # Errors
    ///
    /// A git failure, or a commit id that is not valid UTF-8/hex, is an error.
    /// Only git's "no such commit" answer is reported as `None`.
    pub fn head_commit(&self) -> Result<Option<String>, GitError> {
        let output = self
            .process
            .allowing(&["rev-parse", "--verify", "--quiet", "HEAD^{commit}"], &[1])?;
        if output.status.code() != Some(0) {
            return Ok(None);
        }

        let raw = strip_line_feed(&output.stdout);
        if raw.is_empty() {
            return Ok(None);
        }
        if !raw.iter().all(u8::is_ascii_hexdigit) {
            return Err(GitError::InvalidOutput {
                detail: format!(
                    "git rev-parse returned a non-hex commit id {:?}",
                    String::from_utf8_lossy(raw)
                ),
            });
        }
        Ok(Some(utf8_field(raw, "commit id")?))
    }

    /// Whether the checkout has any staged, modified, or untracked path.
    ///
    /// Ignored files alone do not make a repository dirty. This is equivalent
    /// to `!status()?.is_empty()` with [`StatusOptions::default`].
    pub fn is_dirty(&self) -> Result<bool, GitError> {
        Ok(!self.status()?.is_empty())
    }

    /// Every path reported by the default status query.
    ///
    /// The order is git's order and duplicates are not expected. Use
    /// [`GitRepository::status`] when status letters or rename sources are
    /// needed.
    pub fn dirty_paths(&self) -> Result<Vec<crate::GitPath>, GitError> {
        Ok(self
            .status()?
            .into_iter()
            .map(|entry| entry.path().clone())
            .collect())
    }

    /// The default porcelain status of this checkout.
    ///
    /// Untracked files are reported in git's normal collapsed form and ignored
    /// files are omitted. The index is not refreshed on disk
    /// (`--no-optional-locks`, `GIT_OPTIONAL_LOCKS=0`), so this is safe to call
    /// from a read-only context. The user's `core.fsmonitor` hook is disabled
    /// for the query so a status probe cannot execute repository-configured
    /// programs.
    pub fn status(&self) -> Result<Vec<StatusEntry>, GitError> {
        self.status_with(&StatusOptions::default())
    }

    /// The porcelain status of this checkout with explicit options.
    ///
    /// # Errors
    ///
    /// Any non-zero git exit, unparsable porcelain output, or timeout is an
    /// error. A failed status is never returned as an empty (clean) list.
    pub fn status_with(&self, options: &StatusOptions) -> Result<Vec<StatusEntry>, GitError> {
        crate::status::status_at(&self.process, options)
    }

    /// Every worktree registered for this repository, in git's order.
    ///
    /// The primary checkout is listed first. Paths retain their exact bytes;
    /// branch names are short (`main`, `feature/topic`).
    ///
    /// Git 2.36 and later is queried with `--porcelain -z` for exact path
    /// bytes. Older versions fall back to newline-delimited porcelain output;
    /// a worktree path containing a newline cannot be represented by that
    /// older format.
    ///
    /// # Errors
    ///
    /// A git failure, unparsable output, or timeout is an error. It is never
    /// reported as "no worktrees".
    pub fn worktrees(&self) -> Result<Vec<Worktree>, GitError> {
        let zero_terminated = &["worktree", "list", "--porcelain", "-z"];
        match self.process.checked(zero_terminated) {
            Ok(output) => parse_worktree_z(&output.stdout),
            Err(error) if is_unknown_switch(&error) => {
                let output = self.process.checked(&["worktree", "list", "--porcelain"])?;
                parse_worktree_lines(&output.stdout)
            }
            Err(error) => Err(error),
        }
    }
}

/// Resolve the directory to probe: a file is probed through its parent.
fn probe_directory(path: &Path) -> Result<PathBuf, GitError> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => Ok(parent.to_path_buf()),
            _ => Ok(PathBuf::from(".")),
        },
        Ok(_) => Ok(path.to_path_buf()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            Err(GitError::NotARepository {
                path: path.to_path_buf(),
            })
        }
        Err(source) => Err(GitError::Io {
            context: "read the path to open",
            source,
        }),
    }
}

/// Whether a failed git invocation is "this git is too old for `-z`".
fn is_unknown_switch(error: &GitError) -> bool {
    match error {
        GitError::CommandFailed { stderr, .. } => {
            let stderr = stderr.to_ascii_lowercase();
            stderr.contains("unknown option")
                || stderr.contains("unknown switch")
                || stderr.contains("unrecognized option")
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relative_file_name_is_probed_through_the_current_directory() {
        let probe = probe_directory(Path::new("Cargo.toml")).unwrap();

        assert_eq!(probe, PathBuf::from("."));
    }

    #[test]
    fn unknown_switch_detection_matches_git_diagnostics() {
        let error = GitError::CommandFailed {
            args: vec!["worktree".to_string()],
            cwd: PathBuf::from("."),
            status: Some(129),
            stderr: "error: unknown switch `z'".to_string(),
        };

        assert!(is_unknown_switch(&error));
    }
}
