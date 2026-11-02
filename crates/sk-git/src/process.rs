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
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::error::GitError;

/// The default bound for one git invocation.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// Maximum captured bytes per output stream (16 MiB).
const MAX_OUTPUT_BYTES: u64 = 16 * 1024 * 1024;

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

/// A bounded git process bound to one working directory and option set.
#[derive(Debug, Clone)]
pub(crate) struct GitProcess {
    program: PathBuf,
    cwd: PathBuf,
    timeout: Duration,
}

impl GitProcess {
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

        let mut command = self.command(args);
        command
            .stdout(Stdio::from(stdout_child))
            .stderr(Stdio::from(stderr_child));

        let mut child = command.spawn().map_err(|source| {
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
        let started = Instant::now();
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
    fn command<S: AsRef<OsStr>>(&self, args: &[S]) -> Command {
        let mut command = Command::new(&self.program);
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
}
