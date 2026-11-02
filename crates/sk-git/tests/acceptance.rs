//! Acceptance tests for the read-only `sk-git` adapter.
//!
//! These tests use real temporary Git repositories and the real `git` binary.
//! They cover the public contract: repository discovery, the current branch
//! (including detached and unborn HEAD), dirty-state reporting, porcelain
//! status parsing, worktree listing, raw path bytes, and typed failures.

mod common;

use std::fs;
use std::path::Path;

use common::{TestRepo, git_ok, init_empty_repo_at, init_repo_at};
use sk_git::{GitError, GitPath, GitRepository, StatusOptions, UntrackedMode};

#[test]
fn opens_repository_from_nested_directory() {
    let repo = TestRepo::new();
    let nested = repo.path.join("src").join("nested dir");
    fs::create_dir_all(&nested).unwrap();

    let git = GitRepository::open(&nested).unwrap();

    assert_eq!(git.root(), repo.canonical_path());
}

#[test]
fn opens_repository_from_a_file_path() {
    let repo = TestRepo::new();
    let file = repo.path.join("a.txt");

    let git = GitRepository::open(&file).unwrap();

    assert_eq!(git.root(), repo.canonical_path());
}

#[test]
fn handles_repository_paths_with_spaces_and_shell_metacharacters() {
    let temp = tempfile::tempdir().unwrap();
    // If any layer used a shell, `$(touch pwned)` would create a file.
    let path = temp.path().join("repo $(touch pwned) ; 'quoted'");
    init_repo_at(&path);

    let git = GitRepository::open(&path).unwrap();
    let entries = git.status().unwrap();

    assert_eq!(git.root(), fs::canonicalize(&path).unwrap());
    assert!(entries.is_empty(), "fresh repository must be clean");
    assert!(
        !path.join("pwned").exists(),
        "repository path must be passed as data, never through a shell"
    );
}

#[cfg(unix)]
#[test]
fn retains_non_utf8_repository_path_bytes() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join(OsStr::from_bytes(b"repo-\xff-space"));
    init_repo_at(&path);

    let git = GitRepository::open(&path).unwrap();

    let expected = fs::canonicalize(&path).unwrap();
    assert_eq!(git.root().as_os_str(), expected.as_os_str());
}

#[test]
fn not_a_repository_is_a_typed_error_not_an_absent_answer() {
    let temp = tempfile::tempdir().unwrap();

    let error = GitRepository::open(temp.path()).unwrap_err();

    assert!(
        matches!(error, GitError::NotARepository { .. }),
        "unexpected error: {error:?}"
    );
}

#[test]
fn missing_path_is_a_typed_error() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("does-not-exist");

    let error = GitRepository::open(&missing).unwrap_err();

    assert!(
        matches!(error, GitError::NotARepository { .. }),
        "unexpected error: {error:?}"
    );
}

#[test]
fn bare_repository_is_rejected_explicitly() {
    let temp = tempfile::tempdir().unwrap();
    git_ok(temp.path(), &["init", "-q", "--bare", "."]);

    let error = GitRepository::open(temp.path()).unwrap_err();

    assert!(
        matches!(error, GitError::BareRepository { .. }),
        "unexpected error: {error:?}"
    );
}

#[test]
fn a_deleted_checkout_is_not_reported_as_missing_git() {
    let repo = TestRepo::new();
    let git = GitRepository::open(&repo.path).unwrap();
    fs::remove_dir_all(&repo.path).unwrap();

    let error = git.status().unwrap_err();

    assert!(
        !matches!(error, GitError::GitUnavailable { .. }),
        "a deleted checkout must not be reported as a missing git executable: {error:?}"
    );
}

#[test]
fn a_path_inside_the_git_directory_is_not_a_work_tree() {
    let repo = TestRepo::new();
    let metadata = repo.path.join(".git");

    let error = GitRepository::open(&metadata).unwrap_err();

    assert!(
        matches!(error, GitError::NotARepository { .. }),
        "unexpected error: {error:?}"
    );
}

#[test]
fn a_failed_status_is_an_error_not_a_clean_repository() {
    let repo = TestRepo::new();
    let git = GitRepository::open(&repo.path).unwrap();
    fs::remove_dir_all(repo.path.join(".git")).unwrap();

    let error = git.status().unwrap_err();

    assert!(
        matches!(error, GitError::CommandFailed { .. }),
        "unexpected error: {error:?}"
    );
}

#[test]
fn current_branch_reports_the_checked_out_branch() {
    let repo = TestRepo::new();

    let git = GitRepository::open(&repo.path).unwrap();
    assert_eq!(git.current_branch().unwrap(), Some("main".to_string()));

    repo.git(&["checkout", "-q", "-b", "feature/topic"]);
    assert_eq!(
        git.current_branch().unwrap(),
        Some("feature/topic".to_string())
    );
}

#[test]
fn current_branch_is_none_for_detached_head() {
    let repo = TestRepo::new();
    repo.git(&["checkout", "-q", "--detach"]);

    let git = GitRepository::open(&repo.path).unwrap();

    assert_eq!(git.current_branch().unwrap(), None);
}

#[test]
fn current_branch_is_reported_for_an_unborn_branch() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("unborn");
    init_empty_repo_at(&path);

    let git = GitRepository::open(&path).unwrap();

    assert_eq!(git.current_branch().unwrap(), Some("main".to_string()));
    assert_eq!(git.head_commit().unwrap(), None);
    assert!(!git.is_dirty().unwrap());
}

#[test]
fn head_commit_matches_git_rev_parse() {
    let repo = TestRepo::new();
    let expected = String::from_utf8(repo.git(&["rev-parse", "HEAD"]))
        .unwrap()
        .trim()
        .to_string();

    let git = GitRepository::open(&repo.path).unwrap();

    assert_eq!(git.head_commit().unwrap(), Some(expected));
}

#[test]
fn a_clean_repository_is_not_dirty() {
    let repo = TestRepo::new();
    let git = GitRepository::open(&repo.path).unwrap();

    assert!(!git.is_dirty().unwrap());
    assert!(git.status().unwrap().is_empty());
    assert!(git.dirty_paths().unwrap().is_empty());
}

#[test]
fn an_untracked_file_makes_the_repository_dirty() {
    let repo = TestRepo::new();
    fs::write(repo.path.join("untracked.txt"), b"new\n").unwrap();
    let git = GitRepository::open(&repo.path).unwrap();

    assert!(git.is_dirty().unwrap());
    let entries = git.status().unwrap();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].is_untracked());
    assert_eq!(entries[0].path().as_os_str(), "untracked.txt");
}

#[test]
fn a_staged_change_makes_the_repository_dirty() {
    let repo = TestRepo::new();
    fs::write(repo.path.join("a.txt"), b"changed\n").unwrap();
    repo.git(&["add", "a.txt"]);
    let git = GitRepository::open(&repo.path).unwrap();

    assert!(git.is_dirty().unwrap());
    let entries = git.status().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].index(), 'M');
    assert_eq!(entries[0].worktree(), ' ');
}

#[test]
fn ignored_files_are_not_dirty_by_default_and_are_listed_on_request() {
    let repo = TestRepo::new();
    fs::write(repo.path.join(".gitignore"), b"ignored.txt\n").unwrap();
    repo.git(&["add", ".gitignore"]);
    repo.git(&["commit", "-q", "-m", "ignore rule"]);
    fs::write(repo.path.join("ignored.txt"), b"secret\n").unwrap();

    let git = GitRepository::open(&repo.path).unwrap();
    assert!(!git.is_dirty().unwrap());
    assert!(git.status().unwrap().is_empty());

    let with_ignored = git
        .status_with(&StatusOptions {
            ignored: true,
            ..StatusOptions::default()
        })
        .unwrap();
    assert!(with_ignored.iter().any(|entry| entry.is_ignored()));
}

#[test]
fn untracked_mode_no_ignores_untracked_files() {
    let repo = TestRepo::new();
    fs::write(repo.path.join("untracked.txt"), b"new\n").unwrap();
    let git = GitRepository::open(&repo.path).unwrap();

    let entries = git
        .status_with(&StatusOptions {
            untracked: UntrackedMode::No,
            ..StatusOptions::default()
        })
        .unwrap();

    assert!(entries.is_empty());
}

#[test]
fn status_reports_renames_with_original_and_new_path() {
    let repo = TestRepo::new();
    repo.git(&["mv", "a.txt", "renamed file.txt"]);
    let git = GitRepository::open(&repo.path).unwrap();

    let entries = git.status().unwrap();
    let renamed = entries
        .iter()
        .find(|entry| entry.is_rename())
        .expect("rename entry");

    assert_eq!(renamed.path().as_os_str(), "renamed file.txt");
    assert_eq!(renamed.original_path().unwrap().as_os_str(), "a.txt");
    assert_eq!(renamed.index(), 'R');
}

#[test]
fn status_marks_a_collapsed_untracked_directory() {
    let repo = TestRepo::new();
    fs::create_dir_all(repo.path.join("dir with space")).unwrap();
    fs::write(repo.path.join("dir with space").join("file.txt"), b"x\n").unwrap();
    let git = GitRepository::open(&repo.path).unwrap();

    let entries = git.status().unwrap();
    let directory = entries
        .iter()
        .find(|entry| entry.is_directory())
        .expect("collapsed directory entry");

    assert!(directory.is_untracked());
    assert_eq!(directory.path().as_os_str(), "dir with space");
}

#[cfg(unix)]
#[test]
fn status_retains_non_utf8_path_bytes_exactly() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let repo = TestRepo::new();
    let name = OsStr::from_bytes(b"file-\xff-name.txt");
    fs::write(repo.path.join(name), b"x\n").unwrap();

    let git = GitRepository::open(&repo.path).unwrap();
    let entries = git.status().unwrap();
    let raw = entries
        .iter()
        .find(|entry| entry.path().as_os_str() == name)
        .expect("entry for the non-UTF-8 path");

    assert_eq!(raw.path().to_path_buf().as_os_str(), name);
    assert_eq!(raw.path().to_os_string(), name);
    assert!(raw.path().display_lossy().contains('\u{fffd}'));
}

#[test]
fn worktrees_lists_primary_and_linked_checkouts() {
    let repo = TestRepo::new();
    let linked = repo.temp.path().join("linked worktree");
    repo.git(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "feature/topic",
        linked.to_str().unwrap(),
    ]);

    let git = GitRepository::open(&repo.path).unwrap();
    let worktrees = git.worktrees().unwrap();

    assert_eq!(worktrees.len(), 2);

    let primary = worktrees
        .iter()
        .find(|worktree| worktree.path() == git.root())
        .expect("primary worktree");
    assert_eq!(primary.branch(), Some("main"));
    assert!(!primary.is_detached());
    assert!(!primary.is_bare());
    assert!(primary.head().is_some());

    let feature = worktrees
        .iter()
        .find(|worktree| worktree.branch() == Some("feature/topic"))
        .expect("linked worktree");
    assert_eq!(feature.path(), fs::canonicalize(&linked).unwrap().as_path());
}

#[test]
fn worktrees_reports_a_detached_linked_checkout() {
    let repo = TestRepo::new();
    let detached = repo.temp.path().join("detached checkout");
    repo.git(&[
        "worktree",
        "add",
        "-q",
        "--detach",
        detached.to_str().unwrap(),
        "HEAD",
    ]);

    let git = GitRepository::open(&repo.path).unwrap();
    let worktrees = git.worktrees().unwrap();
    let entry = worktrees
        .iter()
        .find(|worktree| worktree.path() == fs::canonicalize(&detached).unwrap())
        .expect("detached worktree");

    assert!(entry.is_detached());
    assert_eq!(entry.branch(), None);
    assert!(entry.head().is_some());
}

#[test]
fn worktrees_reports_lock_reason() {
    let repo = TestRepo::new();
    let linked = repo.temp.path().join("linked");
    repo.git(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "locked-branch",
        linked.to_str().unwrap(),
    ]);
    repo.git(&[
        "worktree",
        "lock",
        "--reason",
        "held for a test",
        linked.to_str().unwrap(),
    ]);

    let git = GitRepository::open(&repo.path).unwrap();
    let worktrees = git.worktrees().unwrap();
    let entry = worktrees
        .iter()
        .find(|worktree| worktree.branch() == Some("locked-branch"))
        .expect("locked worktree");

    assert_eq!(entry.locked(), Some("held for a test"));
}

#[test]
fn opening_from_a_linked_worktree_uses_that_worktrees_root() {
    let repo = TestRepo::new();
    let linked = repo.temp.path().join("linked");
    repo.git(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "linked-branch",
        linked.to_str().unwrap(),
    ]);

    let git = GitRepository::open(&linked).unwrap();

    assert_eq!(git.root(), fs::canonicalize(&linked).unwrap());
    assert_eq!(
        git.current_branch().unwrap(),
        Some("linked-branch".to_string())
    );
    assert_eq!(git.worktrees().unwrap().len(), 2);
}

#[test]
fn path_values_can_be_compared_and_converted() {
    let path = GitPath::from_git_bytes(b"dir/file name.txt").unwrap();
    let copy = GitPath::from_git_bytes(b"dir/file name.txt").unwrap();
    let other = GitPath::from_git_bytes(b"dir/other.txt").unwrap();

    assert_eq!(path, copy);
    assert_ne!(path, other);
    assert_eq!(path.to_path_buf(), Path::new("dir/file name.txt"));
    assert!(!path.is_empty());
    assert!(GitPath::from_git_bytes(b"").unwrap().is_empty());
}
