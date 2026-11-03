//! Per-worktree sparse-checkout operations that preserve unrelated user data.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sk_git::{GitError, GitRepository, SparseCheckoutMode};

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
        for (path, contents) in [
            ("cone/keep.txt", "keep\n"),
            ("cone/secret/hidden.txt", "hidden\n"),
            ("outside/out.txt", "out\n"),
            ("top.txt", "top\n"),
        ] {
            fs::create_dir_all(repo.root.join(path).parent().unwrap()).unwrap();
            fs::write(repo.root.join(path), contents).unwrap();
        }
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
}

fn git_try(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
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
        .unwrap()
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
fn a_cone_set_is_scoped_to_the_given_worktree_and_preserves_user_files() {
    let repo = Repo::new();
    let lane = repo.add_worktree("lane-a");
    // User data outside the cone must survive the sparse operation.
    fs::write(lane.join("outside/user-note.txt"), b"mine\n").unwrap();

    let lane_repository = GitRepository::open(&lane).unwrap();
    let outcome = lane_repository
        .set_sparse_checkout(SparseCheckoutMode::Cone, &["cone".to_string()])
        .unwrap();

    assert!(outcome.state().configured());
    assert_eq!(outcome.state().mode(), SparseCheckoutMode::Cone);
    assert_eq!(outcome.state().patterns(), ["cone"]);
    let pattern_file = outcome.state().pattern_file();
    assert!(
        pattern_file.starts_with(repo.root.join(".git/worktrees")),
        "the pattern file is the per-worktree one: {}",
        pattern_file.display()
    );
    assert!(
        pattern_file.to_string_lossy().contains("lane-a"),
        "the pattern file belongs to lane-a: {}",
        pattern_file.display()
    );
    assert!(!repo.root.join(".git/info/sparse-checkout").exists());
    assert!(lane.join("cone/keep.txt").exists());
    assert!(
        !lane.join("outside/out.txt").exists(),
        "an out-of-cone tracked file is removed from this checkout"
    );
    assert_eq!(
        fs::read(lane.join("outside/user-note.txt")).unwrap(),
        b"mine\n",
        "untracked user data outside the cone is preserved"
    );
    assert_eq!(
        fs::read(lane.join("cone/secret/hidden.txt")).unwrap(),
        b"hidden\n",
        "a nested file under an in-cone directory is checked out"
    );
    assert!(
        outcome
            .skip_worktree()
            .iter()
            .any(|path| path.as_os_str() == "outside/out.txt"),
        "out-of-cone entries are skip-worktree: {:?}",
        outcome.skip_worktree()
    );

    // The primary checkout keeps its full work tree and its own (absent) sparse
    // state: setting the lane's patterns must not leak.
    let primary = repo.open();
    assert!(!primary.sparse_checkout_state().unwrap().configured());
    assert!(repo.root.join("outside/out.txt").exists());
}

#[test]
fn no_cone_patterns_and_negations_round_trip() {
    let repo = Repo::new();
    let lane = repo.add_worktree("lane-b");
    let lane_repository = GitRepository::open(&lane).unwrap();
    let patterns = [
        "cone/".to_string(),
        "!cone/secret/".to_string(),
        "top.txt".to_string(),
    ];
    let outcome = lane_repository
        .set_sparse_checkout(SparseCheckoutMode::NoCone, &patterns)
        .unwrap();
    assert_eq!(outcome.state().mode(), SparseCheckoutMode::NoCone);
    assert_eq!(outcome.state().patterns(), patterns);
    assert!(lane.join("cone/keep.txt").exists());
    assert!(
        !lane.join("cone/secret/hidden.txt").exists(),
        "the negation keeps the nested directory out"
    );
    assert!(lane.join("top.txt").exists());

    let state = lane_repository.sparse_checkout_state().unwrap();
    assert_eq!(state.patterns(), patterns);
    let raw = lane_repository
        .sparse_pattern_file_text()
        .unwrap()
        .expect("pattern file");
    assert_eq!(raw, "cone/\n!cone/secret/\ntop.txt\n");
}

#[test]
fn an_oversized_pattern_file_refuses_a_bounded_raw_read() {
    let repo = Repo::new();
    let lane = repo.add_worktree("lane-raw");
    let lane_repository = GitRepository::open(&lane).unwrap();
    let path = lane_repository
        .sparse_checkout_state()
        .unwrap()
        .pattern_file()
        .to_path_buf();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        vec![b'x'; (sk_git::MAX_SPARSE_PATTERN_FILE_BYTES + 1) as usize],
    )
    .unwrap();
    let error = lane_repository.sparse_pattern_file_text().unwrap_err();
    assert!(
        matches!(error, GitError::SparsePatternFileTooLarge { .. }),
        "{error:?}"
    );
}

#[test]
fn disabling_restores_the_full_work_tree() {
    let repo = Repo::new();
    let lane = repo.add_worktree("lane-c");
    let lane_repository = GitRepository::open(&lane).unwrap();
    lane_repository
        .set_sparse_checkout(SparseCheckoutMode::Cone, &["cone".to_string()])
        .unwrap();
    assert!(!lane.join("outside/out.txt").exists());

    let disabled = lane_repository.disable_sparse_checkout().unwrap();
    assert!(!disabled.state().configured());
    assert!(
        lane.join("outside/out.txt").exists(),
        "disable restores the out-of-cone file"
    );
    assert!(disabled.skip_worktree().is_empty());

    let error = lane_repository.disable_sparse_checkout().unwrap_err();
    assert!(
        matches!(error, GitError::SparseCheckoutNotConfigured { .. }),
        "{error:?}"
    );
}

#[test]
fn a_plan_refuses_a_moved_head_and_invalid_patterns() {
    let repo = Repo::new();
    let lane = repo.add_worktree("lane-d");
    let lane_repository = GitRepository::open(&lane).unwrap();
    let plan = lane_repository
        .plan_sparse_checkout(SparseCheckoutMode::Cone, &["cone".to_string()])
        .unwrap();

    fs::write(lane.join("extra.txt"), b"extra\n").unwrap();
    git_in(&lane, &["add", "extra.txt"]);
    git_in(&lane, &["commit", "-qm", "advance"]);
    let error = lane_repository.apply_sparse_checkout(&plan).unwrap_err();
    assert!(
        matches!(error, GitError::PlanStale { ref what, .. } if what == "HEAD"),
        "{error:?}"
    );
    assert!(
        !lane_repository
            .sparse_checkout_state()
            .unwrap()
            .configured()
    );

    for patterns in [
        Vec::<String>::new(),
        vec![String::new()],
        vec!["--cone".to_string()],
        vec!["a\nb".to_string()],
    ] {
        let error = lane_repository
            .plan_sparse_checkout(SparseCheckoutMode::Cone, &patterns)
            .unwrap_err();
        assert!(
            matches!(error, GitError::InvalidPlan { .. }),
            "{patterns:?}: {error:?}"
        );
    }
}

#[test]
fn a_merge_in_progress_refuses_a_sparse_change() {
    let repo = Repo::new();
    fs::write(repo.root.join("outside/out.txt"), b"conflict\n").unwrap();
    repo.git(&["commit", "-qam", "ours"]);
    repo.git(&["checkout", "-q", "-b", "theirs", "HEAD~1"]);
    fs::write(repo.root.join("outside/out.txt"), b"theirs\n").unwrap();
    repo.git(&["commit", "-qam", "theirs"]);
    repo.git(&["checkout", "-q", "main"]);
    let merge = git_try(&repo.root, &["merge", "--no-ff", "--no-commit", "theirs"]);
    assert_eq!(
        merge.status.code(),
        Some(1),
        "the fixture must leave a conflicted merge: {}",
        String::from_utf8_lossy(&merge.stderr)
    );

    let repository = repo.open();
    let error = repository
        .plan_sparse_checkout(SparseCheckoutMode::Cone, &["cone".to_string()])
        .unwrap_err();
    assert!(
        matches!(error, GitError::OperationInProgress { .. }),
        "{error:?}"
    );
    let abort = git_try(&repo.root, &["merge", "--abort"]);
    assert!(
        abort.status.success(),
        "{}",
        String::from_utf8_lossy(&abort.stderr)
    );
}
