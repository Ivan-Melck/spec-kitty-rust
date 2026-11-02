//! Tests for the single bounded subprocess seam.
//!
//! These tests use a fake `git` executable (a POSIX shell script) rather than
//! the real git binary, so they can prove bounded execution, kill-and-reap,
//! environment hardening, and argument separation without contacting a remote
//! or waiting on a real network timeout.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use sk_git::{DEFAULT_TIMEOUT, GitError, GitOptions, GitRepository};

fn write_fake_git(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join("fake-git");
    fs::write(&path, body).expect("write fake git");
    let mut permissions = fs::metadata(&path).expect("stat fake git").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).expect("make fake git executable");
    path
}

fn script(body: &str) -> String {
    format!("#!/bin/sh\n{body}")
}

#[test]
fn default_options_are_bounded_and_use_the_git_on_path() {
    let options = GitOptions::default();

    assert_eq!(options.program, PathBuf::from("git"));
    assert_eq!(options.timeout, DEFAULT_TIMEOUT);
    assert!(options.timeout > Duration::ZERO);
}

#[test]
fn missing_git_program_is_a_typed_unavailable_error() {
    let temp = tempfile::tempdir().unwrap();
    let options = GitOptions {
        program: temp.path().join("no-such-git"),
        ..GitOptions::default()
    };

    let error = GitRepository::open_with_options(temp.path(), &options).unwrap_err();

    assert!(
        matches!(error, GitError::GitUnavailable { .. }),
        "unexpected error: {error:?}"
    );
}

#[test]
fn non_zero_exit_is_a_command_failure_not_an_absent_answer() {
    let temp = tempfile::tempdir().unwrap();
    let program = write_fake_git(temp.path(), &script("exit 3\n"));
    let options = GitOptions {
        program,
        ..GitOptions::default()
    };

    let error = GitRepository::open_with_options(temp.path(), &options).unwrap_err();

    match error {
        GitError::CommandFailed {
            ref args,
            status,
            ref stderr,
            ..
        } => {
            assert_eq!(args, &["rev-parse", "--is-inside-work-tree"]);
            assert_eq!(status, Some(3));
            assert!(stderr.contains("no error output") || !stderr.is_empty());
        }
        other => panic!("expected CommandFailed, got {other:?}"),
    }
}

#[cfg(target_os = "linux")]
#[test]
fn timeout_kills_and_reaps_the_git_child() {
    let temp = tempfile::tempdir().unwrap();
    let pid_file = temp.path().join("pid");
    let body = format!("echo $$ > '{}'\nexec sleep 30\n", pid_file.display());
    let program = write_fake_git(temp.path(), &script(&body));
    let options = GitOptions {
        program,
        timeout: Duration::from_millis(300),
    };

    let started = Instant::now();
    let error = GitRepository::open_with_options(temp.path(), &options).unwrap_err();
    let elapsed = started.elapsed();

    match error {
        GitError::Timeout {
            ref args,
            timeout,
            ref cwd,
        } => {
            assert_eq!(args, &["rev-parse", "--is-inside-work-tree"]);
            assert_eq!(timeout, Duration::from_millis(300));
            assert_eq!(cwd, &fs::canonicalize(temp.path()).unwrap());
        }
        other => panic!("expected Timeout, got {other:?}"),
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
    assert!(!pid.is_empty(), "fake git did not record its pid");
    let proc_path = PathBuf::from(format!("/proc/{pid}"));

    // `kill` + `wait` must leave no running process and no zombie behind.
    let mut gone = false;
    for _ in 0..100 {
        if !proc_path.exists() {
            gone = true;
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        gone,
        "git child {pid} was not killed and reaped (still at {})",
        proc_path.display()
    );
}

#[test]
fn child_runs_without_prompts_or_repository_redirecting_environment() {
    let temp = tempfile::tempdir().unwrap();
    let args_file = temp.path().join("args");
    let env_file = temp.path().join("env");
    let body = format!(
        "printf '%s\\n' \"$@\" > '{}'\nenv > '{}'\nexit 1\n",
        args_file.display(),
        env_file.display()
    );
    let program = write_fake_git(temp.path(), &script(&body));
    let options = GitOptions {
        program,
        ..GitOptions::default()
    };

    let _ = GitRepository::open_with_options(temp.path(), &options).unwrap_err();

    let args: Vec<String> = fs::read_to_string(&args_file)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(args, vec!["rev-parse", "--is-inside-work-tree"]);

    let environment = fs::read_to_string(&env_file).unwrap();
    for expected in [
        "GIT_TERMINAL_PROMPT=0",
        "GIT_OPTIONAL_LOCKS=0",
        "GIT_PAGER=cat",
        "LC_ALL=C",
    ] {
        assert!(
            environment.lines().any(|line| line == expected),
            "missing {expected} in child environment"
        );
    }
}

#[test]
fn fake_git_is_executed_directly_not_through_a_shell() {
    let temp = tempfile::tempdir().unwrap();
    let args_file = temp.path().join("args");
    // A shell wrapper would expose the wrapper's own arguments (`-c`, the
    // script text) here. Direct execution sees only git's arguments.
    let body = format!(
        "printf '%s\\n' \"$0\" \"$@\" > '{}'\nexit 1\n",
        args_file.display()
    );
    let program = write_fake_git(temp.path(), &script(&body));
    let options = GitOptions {
        program: program.clone(),
        ..GitOptions::default()
    };

    let _ = GitRepository::open_with_options(temp.path(), &options).unwrap_err();

    let lines: Vec<String> = fs::read_to_string(&args_file)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(
        lines,
        vec![
            program.display().to_string(),
            "rev-parse".to_string(),
            "--is-inside-work-tree".to_string(),
        ]
    );
}
