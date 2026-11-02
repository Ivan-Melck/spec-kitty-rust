//! Bounded remote-read tests using a fake git executable.
//!
//! These prove the remote path keeps the same timeout, kill/reap, no-prompt,
//! and output-cap guarantees as the local seam, without a network.

#![cfg(unix)]

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use common::TestRepo;
use sk_git::{GitError, GitOptions, GitRepository, RemoteName};

fn real_git() -> String {
    let output = Command::new("sh")
        .arg("-c")
        .arg("command -v git")
        .output()
        .expect("locate git");
    assert!(
        output.status.success(),
        "git must be on PATH for these tests"
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn write_wrapper(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join("fake-git");
    let script = format!("#!/bin/sh\nREAL_GIT='{}'\n{body}", real_git());
    fs::write(&path, script).expect("write fake git");
    let mut permissions = fs::metadata(&path).expect("stat fake git").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).expect("make fake git executable");
    path
}

fn open_with(repo: &TestRepo, program: PathBuf, timeout: Duration) -> GitRepository {
    GitRepository::open_with_options(&repo.path, &GitOptions { program, timeout })
        .expect("open repository through the wrapper")
}

#[cfg(target_os = "linux")]
#[test]
fn a_stalled_remote_read_times_out_and_reaps_the_child() {
    let repo = TestRepo::new();
    let pid_file = repo.temp.path().join("pid");
    let body = format!(
        "if [ \"$1\" = ls-remote ]; then echo $$ > '{}'; exec sleep 30; fi\nexec \"$REAL_GIT\" \"$@\"\n",
        pid_file.display()
    );
    let program = write_wrapper(repo.temp.path(), &body);
    let git = open_with(&repo, program, Duration::from_millis(300));

    let branch = sk_git::BranchName::parse("main").unwrap();
    let started = Instant::now();
    let error = git
        .probe_remote_heads(&RemoteName::parse("origin").unwrap(), &[branch])
        .unwrap_err();
    let elapsed = started.elapsed();

    match error {
        GitError::RemoteUnreachable {
            remote,
            timed_out,
            timeout,
            ..
        } => {
            assert_eq!(remote, "origin");
            assert!(timed_out);
            assert_eq!(timeout, Some(Duration::from_millis(300)));
        }
        other => panic!("expected RemoteUnreachable, got {other:?}"),
    }
    assert!(
        elapsed >= Duration::from_millis(250),
        "returned before the timeout elapsed: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "timeout was not bounded: {elapsed:?}"
    );

    let pid = fs::read_to_string(&pid_file).unwrap().trim().to_string();
    let proc_path = PathBuf::from(format!("/proc/{pid}"));
    let mut gone = false;
    for _ in 0..100 {
        if !proc_path.exists() {
            gone = true;
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(gone, "remote git child {pid} was not killed and reaped");
}

#[test]
fn a_remote_read_that_exceeds_the_capture_cap_is_refused() {
    let repo = TestRepo::new();
    let body = "if [ \"$1\" = ls-remote ]; then dd if=/dev/zero bs=1048576 count=17 2>/dev/null | tr '\\0' 'a'; exit 0; fi\nexec \"$REAL_GIT\" \"$@\"\n";
    let program = write_wrapper(repo.temp.path(), body);
    let git = open_with(&repo, program, Duration::from_secs(10));
    let branch = sk_git::BranchName::parse("main").unwrap();

    let error = git
        .probe_remote_heads(&RemoteName::parse("origin").unwrap(), &[branch])
        .unwrap_err();

    assert!(
        matches!(
            error,
            GitError::OutputTooLarge {
                stream: "stdout",
                ..
            }
        ),
        "{error:?}"
    );
}

#[test]
fn a_failing_remote_read_is_not_a_timeout() {
    let repo = TestRepo::new();
    let body = "if [ \"$1\" = ls-remote ]; then echo 'fatal: could not read from remote' >&2; exit 7; fi\nexec \"$REAL_GIT\" \"$@\"\n";
    let program = write_wrapper(repo.temp.path(), body);
    let git = open_with(&repo, program, Duration::from_secs(10));
    let branch = sk_git::BranchName::parse("main").unwrap();

    let error = git
        .probe_remote_heads(&RemoteName::parse("origin").unwrap(), &[branch])
        .unwrap_err();

    match error {
        GitError::RemoteUnreachable {
            timed_out, detail, ..
        } => {
            assert!(!timed_out);
            assert!(detail.contains("could not read"), "{detail}");
        }
        other => panic!("expected RemoteUnreachable, got {other:?}"),
    }
}

#[test]
fn ssh_command_pins_receive_batch_mode_without_overriding_explicit_policy() {
    for (pin, expected) in [
        ("ssh", "ssh -o BatchMode=yes"),
        ("ssh -i /k", "ssh -i /k -o BatchMode=yes"),
        ("ssh -o BatchMode=yes", "ssh -o BatchMode=yes"),
    ] {
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "the_remote_child_gets_the_no_prompt_environment_and_ssh_batch_mode",
                "--nocapture",
            ])
            .env("GIT_SSH_COMMAND", pin)
            .env_remove("GIT_SSH")
            .env("SPK_TEST_EXPECT_SSH", expected)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{pin}: {} {}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

#[test]
fn the_remote_child_gets_the_no_prompt_environment_and_ssh_batch_mode() {
    let repo = TestRepo::new();
    let env_file = repo.temp.path().join("env");
    let args_file = repo.temp.path().join("args");
    let body = format!(
        "if [ \"$1\" = ls-remote ]; then printf '%s\\n' \"$@\" > '{}'; env > '{}'; exit 0; fi\nexec \"$REAL_GIT\" \"$@\"\n",
        args_file.display(),
        env_file.display()
    );
    let program = write_wrapper(repo.temp.path(), &body);
    let git = open_with(&repo, program, Duration::from_secs(10));
    let branch = sk_git::BranchName::parse("main").unwrap();

    let heads = git
        .probe_remote_heads(&RemoteName::parse("origin").unwrap(), &[branch])
        .unwrap();
    assert!(heads.heads().is_empty());

    let environment = fs::read_to_string(&env_file).unwrap();
    assert!(
        environment
            .lines()
            .any(|line| line == "GIT_TERMINAL_PROMPT=0"),
        "missing no-prompt setting"
    );
    // A user-pinned `GIT_SSH` is preserved as-is (the adapter never replaces a
    // transport choice); every other case resolves to an ssh command carrying
    // `BatchMode=yes`.
    let inherited_ssh_only =
        std::env::var_os("GIT_SSH").is_some() && std::env::var_os("GIT_SSH_COMMAND").is_none();
    if !inherited_ssh_only {
        let ssh = environment
            .lines()
            .find(|line| line.starts_with("GIT_SSH_COMMAND="))
            .expect("remote reads must pin a non-interactive ssh command");
        assert!(ssh.contains("BatchMode=yes"), "{ssh}");
    }

    if let Ok(expected) = std::env::var("SPK_TEST_EXPECT_SSH") {
        assert!(
            environment
                .lines()
                .any(|line| line == format!("GIT_SSH_COMMAND={expected}")),
            "{environment}"
        );
    }

    let args: Vec<String> = fs::read_to_string(&args_file)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(args[0], "ls-remote");
    assert_eq!(args[1], "--heads");
    assert_eq!(args[2], "--");
    assert_eq!(args[3], "origin");
    assert_eq!(args[4], "refs/heads/main");
}
