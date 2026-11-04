//! Concurrent safe-commit contention.
//!
//! These tests exercise the shared lock authority: git's own `index.lock` and
//! the compare-and-swap ref advance. The library never retries, never waits, and
//! never removes a lock it did not create; the *caller* may retry a transient
//! refusal. What must always hold is that operator staging is preserved, no
//! commit carries a path another writer requested, and a transient refusal
//! leaves the repository exactly as it found it.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Barrier;
use std::thread;
use std::time::Duration;

use sk_git::{BranchName, GitError, GitPath, GitRepository, SafeCommitRequest};
use tempfile::TempDir;

const LANE: &str = "kitty/mission-concurrent-01ABCDEF";
const MAX_ATTEMPTS: usize = 600;

struct Repo {
    _temp: TempDir,
    path: PathBuf,
}

impl Repo {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("repo");
        fs::create_dir_all(&path).unwrap();
        git(&path, &["init", "-q", "-b", LANE]);
        git(&path, &["config", "user.name", "Test Author"]);
        git(&path, &["config", "user.email", "test@example.com"]);
        git(&path, &["config", "commit.gpgsign", "false"]);
        fs::write(path.join("seed.txt"), b"seed\n").unwrap();
        fs::write(path.join("operator.txt"), b"operator base\n").unwrap();
        git(&path, &["add", "-A"]);
        git(&path, &["commit", "-q", "-m", "initial"]);
        Self { _temp: temp, path }
    }

    fn git(&self, args: &[&str]) -> String {
        String::from_utf8_lossy(&git(&self.path, args)).into_owned()
    }
}

fn git(cwd: &Path, args: &[&str]) -> Vec<u8> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn transient(error: &GitError) -> bool {
    match error {
        // These fixture callers install no hooks and deliberately choose to
        // rebuild/re-plan after a competing writer; the library never retries.
        GitError::OperationInProgress { .. }
        | GitError::PlanStale { .. }
        | GitError::SafeCommitAdvanceUnconfirmed { .. }
        | GitError::SafeCommitIndexLocked { .. } => true,
        GitError::SafeCommitStageFailed { detail, .. }
        | GitError::SafeCommitCommitFailed { detail, .. } => detail.contains("index.lock"),
        _ => false,
    }
}

fn commit_one(repo: &Path, file: &str, message: &str) -> String {
    let path = GitPath::from_git_bytes(file.as_bytes()).unwrap();
    let destination = BranchName::parse(LANE).unwrap();
    for attempt in 1..=MAX_ATTEMPTS {
        let request = SafeCommitRequest::new(destination.clone(), message, [path.clone()]);
        match GitRepository::open(repo).unwrap().safe_commit(&request) {
            Ok(outcome) => return outcome.commit().as_str().to_string(),
            Err(error) if transient(&error) && attempt < MAX_ATTEMPTS => {
                thread::sleep(Duration::from_millis(2));
            }
            Err(error) => panic!("{file}: {error:?} after {attempt} attempts"),
        }
    }
    unreachable!("the retry loop returns or panics")
}

#[test]
fn concurrent_commits_serialise_and_preserve_operator_staging() {
    let repo = Repo::new();
    fs::write(repo.path.join("operator.txt"), b"operator staged\n").unwrap();
    git(&repo.path, &["add", "operator.txt"]);
    let staged_operator = git(&repo.path, &["show", ":operator.txt"]);

    let files = ["w1.txt", "w2.txt", "w3.txt", "w4.txt"];
    for file in files {
        fs::write(repo.path.join(file), format!("{file}\n")).unwrap();
    }
    let barrier = Barrier::new(files.len());
    let commits: Vec<(String, String)> = thread::scope(|scope| {
        let handles: Vec<_> = files
            .iter()
            .map(|file| {
                let barrier = &barrier;
                let path = repo.path.as_path();
                scope.spawn(move || {
                    barrier.wait();
                    let sha = commit_one(path, file, &format!("commit {file}"));
                    (file.to_string(), sha)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("worker panicked"))
            .collect()
    });

    // Every writer landed exactly one commit.
    let history = repo.git(&["log", "--format=%H"]);
    assert_eq!(history.lines().count(), 1 + files.len());
    for (file, sha) in &commits {
        let paths = repo.git(&[
            "-c",
            "diff.relative=false",
            "show",
            "--name-only",
            "--format=",
            sha,
        ]);
        let mut paths: Vec<&str> = paths.lines().filter(|line| !line.is_empty()).collect();
        paths.sort_unstable();
        assert_eq!(paths, vec![file.as_str()], "commit {sha} carried {paths:?}");
    }

    // The operator's staged work is byte-identical and in no commit.
    assert_eq!(git(&repo.path, &["show", ":operator.txt"]), staged_operator);
    let staged = repo.git(&["diff", "--cached", "--name-only"]);
    assert_eq!(staged.trim(), "operator.txt");
    for (_, sha) in &commits {
        let touched = repo.git(&["show", "--name-only", "--format=", sha]);
        assert!(!touched.contains("operator.txt"));
    }
    // No lock or private index was stranded.
    assert!(!repo.path.join(".git/index.lock").exists());
    let leftovers: Vec<PathBuf> = fs::read_dir(repo.path.join(".git"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name().is_some_and(|name| {
                name.to_string_lossy()
                    .starts_with("spec-kitty-safe-commit.")
            })
        })
        .collect();
    assert!(
        leftovers.is_empty(),
        "stranded private indexes: {leftovers:?}"
    );
}

#[test]
fn a_held_lock_is_refused_without_mutation_and_the_caller_may_retry() {
    let repo = Repo::new();
    let lock = repo.path.join(".git").join("index.lock");
    fs::write(&lock, b"").unwrap();
    fs::write(repo.path.join("w.txt"), b"w\n").unwrap();
    let path = GitPath::from_git_bytes(b"w.txt").unwrap();
    let destination = BranchName::parse(LANE).unwrap();
    let request = SafeCommitRequest::new(destination, "locked", [path]);
    let head_before = repo.git(&["rev-parse", "HEAD"]);
    let index_before = git(&repo.path, &["ls-files", "--stage", "-z"]);

    let first = GitRepository::open(&repo.path)
        .unwrap()
        .safe_commit(&request)
        .unwrap_err();
    assert!(matches!(first, GitError::OperationInProgress { .. }));
    assert!(lock.exists());
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), head_before);
    assert_eq!(
        git(&repo.path, &["ls-files", "--stage", "-z"]),
        index_before
    );

    fs::remove_file(&lock).unwrap();
    let outcome = GitRepository::open(&repo.path)
        .unwrap()
        .safe_commit(&request)
        .unwrap();
    assert_eq!(outcome.committed_paths().len(), 1);
}
