//! Guarded worktree prune: only registrations git proves stale are removed.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sk_git::{GitError, GitRepository};

struct Repo {
    _temp: tempfile::TempDir,
    root: PathBuf,
}

impl Repo {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        for directory in ["", "home", "config", "tmp"] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        let repo = Self { _temp: temp, root };
        repo.git(&["init", "-q", "-b", "main"]);
        repo.git(&["config", "user.name", "Test"]);
        repo.git(&["config", "user.email", "test@example.invalid"]);
        repo.git(&["config", "commit.gpgsign", "false"]);
        fs::write(repo.root.join("seed.txt"), b"seed\n").unwrap();
        repo.git(&["add", "-A"]);
        repo.git(&["commit", "-qm", "initial"]);
        repo
    }

    fn git(&self, args: &[&str]) -> Vec<u8> {
        git_in(&self.root, args)
    }

    fn open(&self) -> GitRepository {
        GitRepository::open(&self.root).unwrap()
    }

    fn add_worktree(&self, name: &str) -> PathBuf {
        let path = self.root.parent().unwrap().join(name);
        self.git(&["worktree", "add", "-q", path.to_str().unwrap(), "-b", name]);
        path
    }

    fn listed(&self) -> Vec<String> {
        String::from_utf8_lossy(&self.git(&["worktree", "list", "--porcelain"]))
            .lines()
            .filter_map(|line| line.strip_prefix("worktree "))
            .map(str::to_string)
            .collect()
    }
}

fn git_in(root: &Path, args: &[&str]) -> Vec<u8> {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .env("HOME", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("TMPDIR", root.join("tmp"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

#[test]
fn only_a_proven_stale_registration_is_pruned_and_live_checkouts_survive() {
    let repo = Repo::new();
    let stale = repo.add_worktree("stale-lane");
    let live = repo.add_worktree("live-lane");
    fs::write(live.join("live.txt"), b"lane work\n").unwrap();
    // A missing checkout directory is what makes a registration prunable.
    fs::remove_dir_all(&stale).unwrap();

    let repository = repo.open();
    let plan = repository.plan_worktree_prune().unwrap();
    assert_eq!(plan.prunable().len(), 1, "{:?}", plan.prunable());
    let entry = &plan.prunable()[0];
    assert_eq!(entry.path(), stale.as_path());
    assert!(!entry.admin_id().is_empty());
    assert!(entry.reason().is_some());
    assert_eq!(plan.live().len(), 2, "primary and live lane");

    let outcome = repository.apply_worktree_prune(&plan).unwrap();
    assert_eq!(outcome.pruned().len(), 1);
    assert_eq!(outcome.pruned()[0].path(), stale.as_path());
    let listed = repo.listed();
    assert!(!listed.iter().any(|path| Path::new(path) == stale));
    assert!(listed.iter().any(|path| Path::new(path) == live));
    assert!(
        live.join("live.txt").exists(),
        "a live checkout and its user files are untouched"
    );

    // A second plan is empty and its apply is a no-op.
    let empty = repository.plan_worktree_prune().unwrap();
    assert!(empty.prunable().is_empty());
    let noop = repository.apply_worktree_prune(&empty).unwrap();
    assert!(noop.pruned().is_empty());
}

#[test]
fn a_registration_that_recovered_before_apply_refuses_instead_of_pruning() {
    let repo = Repo::new();
    let stale = repo.add_worktree("recover-lane");
    fs::remove_dir_all(&stale).unwrap();
    let repository = repo.open();
    let plan = repository.plan_worktree_prune().unwrap();
    assert_eq!(plan.prunable().len(), 1);

    // Recreate the checkout's `.git` link so git considers it live again.
    fs::create_dir_all(&stale).unwrap();
    let admin = fs::read_dir(repo.root.join(".git/worktrees"))
        .unwrap()
        .filter_map(Result::ok)
        .find(|entry| entry.file_name() == "recover-lane")
        .expect("admin dir");
    let gitdir = fs::read_to_string(admin.path().join("gitdir")).unwrap();
    fs::write(stale.join(".git"), format!("gitdir: {}\n", gitdir.trim())).unwrap();

    let error = repository.apply_worktree_prune(&plan).unwrap_err();
    assert!(
        matches!(error, GitError::PlanStale { ref what, .. } if what == "worktree registrations"),
        "{error:?}"
    );
    assert!(
        repo.listed()
            .iter()
            .any(|path| Path::new(path) == stale.as_path()),
        "the recovered registration is still listed"
    );
}

#[test]
fn a_live_checkout_is_never_a_prune_candidate() {
    let repo = Repo::new();
    let lane = repo.add_worktree("healthy-lane");
    let repository = repo.open();
    let plan = repository.plan_worktree_prune().unwrap();
    assert!(
        plan.prunable().is_empty(),
        "a worktree with a live directory is not prunable: {:?}",
        plan.prunable()
    );
    assert!(plan.live().iter().any(|path| path == &lane));
    repository.apply_worktree_prune(&plan).unwrap();
    assert!(repo.listed().iter().any(|path| Path::new(path) == lane));
}

#[test]
fn the_primary_checkout_is_never_pruned() {
    let repo = Repo::new();
    let repository = repo.open();
    let plan = repository.plan_worktree_prune().unwrap();
    assert!(plan.prunable().is_empty());
    assert_eq!(
        plan.live().first().map(PathBuf::as_path),
        Some(repository.root())
    );
}
