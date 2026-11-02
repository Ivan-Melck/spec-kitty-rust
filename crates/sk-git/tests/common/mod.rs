//! Shared helpers for the `sk-git` acceptance tests.
//!
//! Every helper shells out to the real `git` binary. The tests therefore
//! require a working Git installation (>= 2.28, for `git init -b`); this is
//! stated in the crate README.

#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

/// A temporary directory containing one real Git repository at `path`.
pub struct TestRepo {
    pub temp: TempDir,
    pub path: PathBuf,
}

impl TestRepo {
    /// Create a repository with a `main` branch and one commit.
    pub fn new() -> Self {
        let temp = tempfile::tempdir().expect("create tempdir");
        let path = temp.path().join("repo");
        init_repo_at(&path);
        Self { temp, path }
    }

    /// Run a git command inside the repository and require success.
    pub fn git(&self, args: &[&str]) -> Vec<u8> {
        git_ok(&self.path, args)
    }

    /// Run a git command inside the repository and return the raw outcome.
    pub fn git_raw(&self, args: &[&str]) -> std::process::Output {
        git_raw(&self.path, args)
    }

    /// The canonical form of the repository path.
    pub fn canonical_path(&self) -> PathBuf {
        fs::canonicalize(&self.path).expect("canonicalize repository path")
    }
}

/// Run `git <args>` in `cwd`; panic with git's stderr when it fails.
pub fn git_ok(cwd: &Path, args: &[&str]) -> Vec<u8> {
    let output = git_raw(cwd, args);
    assert!(
        output.status.success(),
        "git {args:?} failed in {}: {}\n{}",
        cwd.display(),
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    output.stdout
}

/// Run `git <args>` in `cwd` without asserting success.
pub fn git_raw(cwd: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test Author")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test Committer")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .expect("run git")
}

/// Initialise a repository at `path` with a `main` branch and one commit.
pub fn init_repo_at(path: &Path) {
    fs::create_dir_all(path).expect("create repository directory");
    git_ok(path, &["init", "-q", "-b", "main"]);
    configure_repo(path);
    fs::write(path.join("a.txt"), b"one\n").expect("write tracked file");
    git_ok(path, &["add", "a.txt"]);
    git_ok(path, &["commit", "-q", "-m", "initial"]);
}

/// Initialise an empty repository (unborn `main`) at `path`.
pub fn init_empty_repo_at(path: &Path) {
    fs::create_dir_all(path).expect("create repository directory");
    git_ok(path, &["init", "-q", "-b", "main"]);
    configure_repo(path);
}

fn configure_repo(path: &Path) {
    git_ok(path, &["config", "user.name", "Test Author"]);
    git_ok(path, &["config", "user.email", "test@example.com"]);
    git_ok(path, &["config", "commit.gpgsign", "false"]);
    git_ok(path, &["config", "tag.gpgsign", "false"]);
}
