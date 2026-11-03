//! The single subprocess seam.
//!
//! Every git invocation in this crate goes through [`GitProcess::run`]. The
//! seam is deliberately small and closed: callers pass a program plus a fixed
//! argument list, never a shell string and never a path that is interpreted by
//! a shell. It enforces:
//!
//! * a finite timeout, after which the direct child is killed and reaped;
//! * a non-interactive, read-only-shaped environment (`GIT_TERMINAL_PROMPT=0`,
//!   `GIT_OPTIONAL_LOCKS=0`, no pager), with repository-redirecting variables
//!   removed so the configured working directory really selects the repository;
//! * capture of raw stdout/stderr bytes through anonymous temporary files,
//!   which cannot deadlock on a full pipe;
//! * null stdin, so git can never read from the caller's terminal.

use std::ffi::OsStr;
use std::ffi::OsString;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::error::GitError;

/// The default bound for one git invocation.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// Maximum captured bytes per output stream (16 MiB).
const MAX_OUTPUT_BYTES: u64 = 16 * 1024 * 1024;

/// Maximum bytes accepted on a child's stdin (4 MiB).
const MAX_STDIN_BYTES: u64 = 4 * 1024 * 1024;

/// Environment variables that would redirect git to a different repository
/// than the one the adapter was opened on. They are removed from every child.
const REPOSITORY_REDIRECTING_ENV: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_PREFIX",
    "GIT_CEILING_DIRECTORIES",
    "GIT_DISCOVERY_ACROSS_FILESYSTEM",
    "GIT_NAMESPACE",
    "GIT_SHALLOW_FILE",
    "GIT_GRAFT_FILE",
    "GIT_REPLACE_REF_BASE",
];

/// Configuration for a [`GitRepository`](crate::GitRepository) and its
/// subprocess seam.
///
/// The default is the `git` executable on `PATH` with a ten-second bound per
/// invocation. Override `program` to pin a specific git binary (useful in
/// hermetic environments and tests); override `timeout` to tighten or widen
/// the bound.
#[derive(Debug, Clone)]
pub struct GitOptions {
    /// The git executable to run. Defaults to `git` (resolved through `PATH`).
    pub program: PathBuf,
    /// The maximum wall-clock time for one git invocation.
    ///
    /// A zero duration is legal but means "spawn, then time out at once"; the
    /// child is still killed and reaped.
    pub timeout: Duration,
}

impl Default for GitOptions {
    fn default() -> Self {
        Self {
            program: PathBuf::from("git"),
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

/// One completed (or timed-out) git invocation.
#[derive(Debug)]
pub(crate) struct CommandOutput {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

/// The closed environment profiles this crate may run a child under.
///
/// There is deliberately no caller-supplied environment map: a profile can
/// only add the no-prompt, no-remote-write settings this adapter owns. The one
/// repository-redirecting exception is [`EnvProfile::ScopedIndex`], whose value
/// is a [`ScopedIndex`] path generated *inside this checkout's own git
/// directory* — never a caller-provided index path.
#[derive(Debug, Clone)]
pub(crate) enum EnvProfile {
    /// A local repository command: no remote contact, read-shaped locks off.
    Local,
    /// One bounded remote READ (`ls-remote`). `ssh_command` is the effective
    /// non-interactive ssh command chosen by the remote module, or `None` to
    /// leave the inherited `GIT_SSH_COMMAND`/`GIT_SSH` choice untouched.
    RemoteRead { ssh_command: Option<OsString> },
    /// A path-scoped index transaction against one private index file.
    ScopedIndex { index: PathBuf },
    /// A commit hook: private-index scope plus git commit's no-editor context.
    CommitHook {
        index: Option<PathBuf>,
        no_editor: bool,
    },
}

/// A private index file used by one path-scoped index transaction.
///
/// The constructor is crate-private and only accepts a checkout's own git
/// directory (as returned by `git rev-parse --absolute-git-dir`). The file name
/// is generated here, includes the process id, and can never be a path supplied
/// by a caller, so the `GIT_INDEX_FILE` this type feeds to git cannot reopen
/// arbitrary repository redirection. The file and its `.lock` sibling are
/// removed on drop; the operator's real index is never named by this type.
#[derive(Debug)]
pub(crate) struct ScopedIndex {
    path: PathBuf,
}

impl ScopedIndex {
    /// Allocate a new private index path inside `git_dir`.
    ///
    /// The path is not created: git creates it (and its `.lock`) itself when the
    /// first plumbing command writes the index, so the shared lock authority is
    /// still git's.
    pub(crate) fn in_git_dir(git_dir: &Path) -> Result<Self, GitError> {
        if !git_dir.is_absolute() {
            return Err(GitError::InvalidPlan {
                detail: format!(
                    "a scoped index needs an absolute git directory, got {}",
                    git_dir.display()
                ),
            });
        }
        static NEXT_SERIAL: AtomicU64 = AtomicU64::new(0);
        let pid = std::process::id();
        for _ in 0..32 {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0);
            let serial = NEXT_SERIAL.fetch_add(1, Ordering::Relaxed);
            let path = git_dir.join(format!("spec-kitty-safe-commit.{pid}.{nanos}.{serial}.tmp"));
            let mut lock_name = path.as_os_str().to_os_string();
            lock_name.push(".lock");
            if !path.exists() && !PathBuf::from(lock_name).exists() {
                return Ok(Self { path });
            }
        }
        Err(GitError::InvalidPlan {
            detail: "could not allocate a unique private index path in the git directory"
                .to_string(),
        })
    }

    /// The path the child sees as `GIT_INDEX_FILE`.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScopedIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let mut lock_name = self.path.as_os_str().to_os_string();
        lock_name.push(".lock");
        let _ = std::fs::remove_file(PathBuf::from(lock_name));
    }
}

/// A bounded git process bound to one working directory and option set.
#[derive(Debug, Clone)]
pub(crate) struct GitProcess {
    program: PathBuf,
    cwd: PathBuf,
    timeout: Duration,
}

impl GitProcess {
    /// Refuse an inherited selector instead of silently writing another index
    /// or repository. A captured shared authority is required to support these.
    pub(crate) fn ensure_unredirected_write(&self) -> Result<(), GitError> {
        for &variable in REPOSITORY_REDIRECTING_ENV {
            if std::env::var_os(variable).is_some() {
                return Err(GitError::RepositoryEnvironmentRefused { variable });
            }
        }
        Ok(())
    }

    pub(crate) fn new(cwd: impl Into<PathBuf>, options: &GitOptions) -> Self {
        Self {
            program: options.program.clone(),
            cwd: cwd.into(),
            timeout: options.timeout,
        }
    }

    /// Run git with `args` and return its raw outcome, whatever the exit code.
    ///
    /// Errors are reserved for failures to start or finish the process:
    /// [`GitError::GitUnavailable`], [`GitError::Spawn`], [`GitError::Timeout`],
    /// and [`GitError::Io`]. A non-zero exit status is returned to the caller in
    /// [`CommandOutput::status`] so that command-specific "absent" exit codes
    /// (for example `symbolic-ref` exit 1 on a detached HEAD) can be
    /// distinguished from real failures.
    pub(crate) fn run<S: AsRef<OsStr>>(&self, args: &[S]) -> Result<CommandOutput, GitError> {
        self.run_profiled(args, None, EnvProfile::Local)
    }

    /// Run one bounded remote read with the given effective ssh command.
    pub(crate) fn run_remote<S: AsRef<OsStr>>(
        &self,
        args: &[S],
        ssh_command: Option<OsString>,
    ) -> Result<CommandOutput, GitError> {
        self.run_profiled(args, None, EnvProfile::RemoteRead { ssh_command })
    }

    /// Run git against one private index file (raw outcome, whatever the code).
    pub(crate) fn run_index_scoped<S: AsRef<OsStr>>(
        &self,
        args: &[S],
        index: &ScopedIndex,
    ) -> Result<CommandOutput, GitError> {
        self.run_profiled(
            args,
            None,
            EnvProfile::ScopedIndex {
                index: index.path().to_path_buf(),
            },
        )
    }

    /// Run a hook in the route's private-index and editor context.
    pub(crate) fn run_commit_hook<S: AsRef<OsStr>>(
        &self,
        args: &[S],
        index: Option<&ScopedIndex>,
        no_editor: bool,
    ) -> Result<CommandOutput, GitError> {
        self.run_profiled(
            args,
            None,
            EnvProfile::CommitHook {
                index: index.map(|index| index.path().to_path_buf()),
                no_editor,
            },
        )
    }

    /// Run git against one private index file and require success.
    pub(crate) fn checked_index_scoped<S: AsRef<OsStr>>(
        &self,
        args: &[S],
        index: &ScopedIndex,
    ) -> Result<CommandOutput, GitError> {
        let output = self.run_index_scoped(args, index)?;
        if output.status.success() {
            Ok(output)
        } else {
            Err(self.failed(args, &output))
        }
    }

    /// Run git against one private index file with a bounded stdin payload.
    pub(crate) fn checked_with_stdin_index_scoped<S: AsRef<OsStr>>(
        &self,
        args: &[S],
        stdin: &[u8],
        index: &ScopedIndex,
    ) -> Result<CommandOutput, GitError> {
        if stdin.len() as u64 > MAX_STDIN_BYTES {
            return Err(GitError::InputTooLarge {
                limit: MAX_STDIN_BYTES,
            });
        }
        let output = self.run_profiled(
            args,
            Some(stdin),
            EnvProfile::ScopedIndex {
                index: index.path().to_path_buf(),
            },
        )?;
        if output.status.success() {
            Ok(output)
        } else {
            Err(self.failed(args, &output))
        }
    }

    /// Run git with a bounded stdin payload and require a successful status.
    ///
    /// Used by `update-ref --stdin` and `rev-list --stdin`, whose ref/OID
    /// operands are line data rather than option positionals.
    pub(crate) fn checked_with_stdin<S: AsRef<OsStr>>(
        &self,
        args: &[S],
        stdin: &[u8],
    ) -> Result<CommandOutput, GitError> {
        if stdin.len() as u64 > MAX_STDIN_BYTES {
            return Err(GitError::InputTooLarge {
                limit: MAX_STDIN_BYTES,
            });
        }
        let output = self.run_profiled(args, Some(stdin), EnvProfile::Local)?;
        if output.status.success() {
            Ok(output)
        } else {
            Err(self.failed(args, &output))
        }
    }

    fn run_profiled<S: AsRef<OsStr>>(
        &self,
        args: &[S],
        stdin: Option<&[u8]>,
        profile: EnvProfile,
    ) -> Result<CommandOutput, GitError> {
        let mut stdout_file = tempfile::tempfile().map_err(|source| GitError::Io {
            context: "create a temporary stdout file",
            source,
        })?;
        let mut stderr_file = tempfile::tempfile().map_err(|source| GitError::Io {
            context: "create a temporary stderr file",
            source,
        })?;

        let stdout_child = stdout_file.try_clone().map_err(|source| GitError::Io {
            context: "duplicate the stdout file handle",
            source,
        })?;
        let stderr_child = stderr_file.try_clone().map_err(|source| GitError::Io {
            context: "duplicate the stderr file handle",
            source,
        })?;

        // A stdin payload is materialised in an anonymous file before the
        // child starts. A pipe would let a child that never reads block the
        // writer; a regular file cannot, and git's `--stdin` plumbing reads it
        // the same way.
        let stdin_file = match stdin {
            Some(payload) => {
                let mut file = tempfile::tempfile().map_err(|source| GitError::Io {
                    context: "create a temporary stdin file",
                    source,
                })?;
                file.write_all(payload).map_err(|source| GitError::Io {
                    context: "write the child stdin payload",
                    source,
                })?;
                file.seek(SeekFrom::Start(0))
                    .map_err(|source| GitError::Io {
                        context: "rewind the child stdin payload",
                        source,
                    })?;
                Some(file)
            }
            None => None,
        };

        let mut command = self.command_with_profile(args, &profile);
        command
            .stdout(Stdio::from(stdout_child))
            .stderr(Stdio::from(stderr_child));
        if let Some(file) = stdin_file {
            command.stdin(Stdio::from(file));
        }

        let started = Instant::now();
        // Concurrent fork/exec can briefly inherit a writer descriptor for a
        // newly installed executable, yielding ETXTBSY before it runs at all.
        // Retry only that pre-exec failure, within the invocation's original
        // deadline and a separate 100 ms cap; never retry a running mutation.
        let spawn_budget = self.timeout.min(Duration::from_millis(100));
        let spawned = loop {
            let result = command.spawn();
            if let Err(source) = &result
                && source.kind() == std::io::ErrorKind::ExecutableFileBusy
                && started.elapsed() < spawn_budget
            {
                thread::sleep(
                    Duration::from_millis(1).min(spawn_budget.saturating_sub(started.elapsed())),
                );
                continue;
            }
            break result;
        };
        let mut child = spawned.map_err(|source| {
            // A missing working directory also reports `NotFound`. Distinguish
            // it from a missing git executable so a deleted checkout is not
            // described as an installation problem.
            if source.kind() == std::io::ErrorKind::NotFound && self.cwd.exists() {
                GitError::GitUnavailable {
                    program: self.program.clone(),
                    source,
                }
            } else {
                GitError::Spawn {
                    program: self.program.clone(),
                    cwd: self.cwd.clone(),
                    source,
                }
            }
        })?;

        let argv = display_args(args);
        let mut poll_interval = Duration::from_millis(1);
        let status = loop {
            for (stream, file) in [("stdout", &stdout_file), ("stderr", &stderr_file)] {
                if file
                    .metadata()
                    .is_ok_and(|metadata| metadata.len() > MAX_OUTPUT_BYTES)
                {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(GitError::OutputTooLarge {
                        stream,
                        limit: MAX_OUTPUT_BYTES,
                    });
                }
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {
                    let elapsed = started.elapsed();
                    if elapsed >= self.timeout {
                        // `kill` is best-effort: the child may have exited
                        // between `try_wait` and this call. `wait` reaps either
                        // way, so no zombie is left behind.
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(GitError::Timeout {
                            args: argv,
                            cwd: self.cwd.clone(),
                            timeout: self.timeout,
                        });
                    }
                    let remaining = self.timeout - elapsed;
                    thread::sleep(poll_interval.min(remaining));
                    poll_interval = (poll_interval * 2).min(Duration::from_millis(10));
                }
                Err(source) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(GitError::Io {
                        context: "wait for git",
                        source,
                    });
                }
            }
        };

        let stdout = read_output(&mut stdout_file, "stdout")?;
        let stderr = read_output(&mut stderr_file, "stderr")?;
        Ok(CommandOutput {
            status,
            stdout,
            stderr,
        })
    }

    /// Run git and require a successful exit status.
    pub(crate) fn checked<S: AsRef<OsStr>>(&self, args: &[S]) -> Result<CommandOutput, GitError> {
        let output = self.run(args)?;
        if output.status.success() {
            Ok(output)
        } else {
            Err(self.failed(args, &output))
        }
    }

    /// Run git and accept the listed non-zero exit codes as answers.
    ///
    /// Any other non-zero code becomes [`GitError::CommandFailed`]; a signalled
    /// process also becomes a failure.
    pub(crate) fn allowing<S: AsRef<OsStr>>(
        &self,
        args: &[S],
        allowed_codes: &[i32],
    ) -> Result<CommandOutput, GitError> {
        let output = self.run(args)?;
        match output.status.code() {
            Some(0) => Ok(output),
            Some(code) if allowed_codes.contains(&code) => Ok(output),
            _ => Err(self.failed(args, &output)),
        }
    }

    /// Build the child command with the adapter's fixed environment policy.
    #[cfg(test)]
    fn command<S: AsRef<OsStr>>(&self, args: &[S]) -> Command {
        self.command_with_profile(args, &EnvProfile::Local)
    }

    fn command_with_profile<S: AsRef<OsStr>>(&self, args: &[S], profile: &EnvProfile) -> Command {
        let mut command = Command::new(&self.program);
        if matches!(
            profile,
            EnvProfile::CommitHook {
                no_editor: true,
                ..
            }
        ) {
            // Ordinary upstream safe_commit forces unsigned git commits; the
            // config must also propagate to commands invoked by its hooks.
            command.args(["-c", "commit.gpgsign=false"]);
        }
        command.args(args.iter().map(AsRef::as_ref));
        command.current_dir(&self.cwd);
        command.stdin(Stdio::null());

        for key in REPOSITORY_REDIRECTING_ENV {
            command.env_remove(key);
        }
        command
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_PAGER", "cat")
            .env("LC_ALL", "C");
        if let EnvProfile::RemoteRead {
            ssh_command: Some(ssh_command),
        } = profile
        {
            // `GIT_TERMINAL_PROMPT=0` covers git's own prompt; ssh(1) does not
            // honor it, so the remote module resolves plain ssh to
            // `-o BatchMode=yes`. A command already pinned by the user is
            // passed through unchanged (never silently replaced).
            command.env("GIT_SSH_COMMAND", ssh_command);
        }
        if let EnvProfile::ScopedIndex { index }
        | EnvProfile::CommitHook {
            index: Some(index), ..
        } = profile
        {
            // Only a private index allocated by this crate, never caller argv.
            command.env("GIT_INDEX_FILE", index);
        }
        if matches!(
            profile,
            EnvProfile::CommitHook {
                no_editor: true,
                ..
            }
        ) {
            command.env("GIT_EDITOR", ":");
        }
        command
    }

    pub(crate) fn failed<S: AsRef<OsStr>>(&self, args: &[S], output: &CommandOutput) -> GitError {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        GitError::CommandFailed {
            args: display_args(args),
            cwd: self.cwd.clone(),
            status: output.status.code(),
            stderr: if stderr.is_empty() {
                "no error output".to_string()
            } else {
                stderr
            },
        }
    }
}

fn read_output(file: &mut std::fs::File, stream: &'static str) -> Result<Vec<u8>, GitError> {
    file.seek(SeekFrom::Start(0))
        .map_err(|source| GitError::Io {
            context: "rewind git output",
            source,
        })?;
    let mut bytes = Vec::new();
    file.take(MAX_OUTPUT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| GitError::Io {
            context: "read git output",
            source,
        })?;
    if bytes.len() as u64 > MAX_OUTPUT_BYTES {
        return Err(GitError::OutputTooLarge {
            stream,
            limit: MAX_OUTPUT_BYTES,
        });
    }
    Ok(bytes)
}

fn display_args<S: AsRef<OsStr>>(args: &[S]) -> Vec<String> {
    args.iter()
        .map(|arg| arg.as_ref().to_string_lossy().into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::Path;

    use super::*;

    fn env_map(command: &Command) -> HashMap<PathBuf, Option<PathBuf>> {
        command
            .get_envs()
            .map(|(key, value)| (PathBuf::from(key), value.map(PathBuf::from)))
            .collect()
    }

    #[test]
    fn captured_output_is_bounded_even_after_a_fast_child_exits() {
        let mut file = tempfile::tempfile().unwrap();
        file.set_len(MAX_OUTPUT_BYTES + 1).unwrap();
        assert!(matches!(
            read_output(&mut file, "stdout"),
            Err(GitError::OutputTooLarge { .. })
        ));
    }

    #[test]
    fn command_uses_fixed_arguments_and_working_directory() {
        let process = GitProcess {
            program: PathBuf::from("git"),
            cwd: PathBuf::from("/tmp/example"),
            timeout: DEFAULT_TIMEOUT,
        };

        let command = process.command(&["status", "--porcelain=v1", "-z"]);

        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, vec!["status", "--porcelain=v1", "-z"]);
        assert_eq!(command.get_current_dir(), Some(Path::new("/tmp/example")));
    }

    #[test]
    fn command_hardens_environment_and_removes_redirects() {
        let process = GitProcess {
            program: PathBuf::from("git"),
            cwd: PathBuf::from("."),
            timeout: DEFAULT_TIMEOUT,
        };

        let environment = env_map(&process.command(&["status"]));

        assert_eq!(
            environment.get(Path::new("GIT_TERMINAL_PROMPT")),
            Some(&Some(PathBuf::from("0")))
        );
        assert_eq!(
            environment.get(Path::new("GIT_OPTIONAL_LOCKS")),
            Some(&Some(PathBuf::from("0")))
        );
        assert_eq!(
            environment.get(Path::new("GIT_PAGER")),
            Some(&Some(PathBuf::from("cat")))
        );
        assert_eq!(
            environment.get(Path::new("LC_ALL")),
            Some(&Some(PathBuf::from("C")))
        );
        for key in REPOSITORY_REDIRECTING_ENV {
            assert_eq!(
                environment.get(Path::new(key)),
                Some(&None),
                "{key} must be removed, not inherited"
            );
        }
    }

    #[test]
    fn a_scoped_index_profile_sets_only_the_generated_index_file() {
        let git_dir = tempfile::tempdir().unwrap();
        let index = ScopedIndex::in_git_dir(git_dir.path()).unwrap();
        assert!(
            index.path().starts_with(git_dir.path()),
            "the private index must live in the checkout's own git dir"
        );
        assert!(
            index
                .path()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("spec-kitty-safe-commit.")
        );
        assert!(!index.path().exists(), "git creates the index, not us");

        let process = GitProcess {
            program: PathBuf::from("git"),
            cwd: PathBuf::from("."),
            timeout: DEFAULT_TIMEOUT,
        };
        let command = process.command_with_profile(
            &["status"],
            &EnvProfile::ScopedIndex {
                index: index.path().to_path_buf(),
            },
        );
        let environment = env_map(&command);

        assert_eq!(
            environment.get(Path::new("GIT_INDEX_FILE")),
            Some(&Some(index.path().to_path_buf()))
        );
        for key in REPOSITORY_REDIRECTING_ENV {
            if *key == "GIT_INDEX_FILE" {
                continue;
            }
            assert_eq!(
                environment.get(Path::new(key)),
                Some(&None),
                "{key} must still be removed in a scoped-index child"
            );
        }
        assert_eq!(
            environment.get(Path::new("GIT_TERMINAL_PROMPT")),
            Some(&Some(PathBuf::from("0")))
        );
    }

    #[test]
    fn a_scoped_index_refuses_a_relative_git_dir() {
        assert!(matches!(
            ScopedIndex::in_git_dir(Path::new(".git")),
            Err(GitError::InvalidPlan { .. })
        ));
    }

    #[test]
    fn dropping_a_scoped_index_removes_the_index_and_its_lock() {
        let git_dir = tempfile::tempdir().unwrap();
        let index = ScopedIndex::in_git_dir(git_dir.path()).unwrap();
        std::fs::write(index.path(), b"private index").unwrap();
        let mut lock = index.path().as_os_str().to_os_string();
        lock.push(".lock");
        let lock = PathBuf::from(lock);
        std::fs::write(&lock, b"lock").unwrap();

        drop(index);

        assert!(!lock.exists());
        assert_eq!(std::fs::read_dir(git_dir.path()).unwrap().count(), 0);
    }
}
