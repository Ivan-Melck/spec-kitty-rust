//! Acceptance tests for the native path-scoped safe commit.
//!
//! Every test uses a real temporary Git repository and the real `git` binary.
//! No test touches the workspace checkout, a user remote, or the operator's
//! environment. These tests pin the contracts implemented in
//! `src/safe_commit.rs` and `src/safe_commit_transaction.rs`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sk_git::{
    BranchName, CommitGuardCapability, ExpectedBlob, GitError, GitPath, GitRepository,
    ProtectedBranches, SafeCommitLandedKind, SafeCommitRecovery, SafeCommitRequest,
    SafeCommitRoute, normalize_safe_commit_path,
};
use tempfile::TempDir;

const LANE: &str = "kitty/mission-safe-01ABCDEF";

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

struct Repo {
    _temp: TempDir,
    path: PathBuf,
}

impl Repo {
    fn new(branch: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("repo");
        fs::create_dir_all(&path).unwrap();
        git_ok(&path, &["init", "-q", "-b", branch]);
        git_ok(&path, &["config", "user.name", "Test Author"]);
        git_ok(&path, &["config", "user.email", "test@example.com"]);
        git_ok(&path, &["config", "commit.gpgsign", "false"]);
        git_ok(&path, &["config", "tag.gpgsign", "false"]);
        fs::write(path.join("seed.txt"), b"seed\n").unwrap();
        fs::write(path.join("a.txt"), b"one\n").unwrap();
        fs::write(path.join("other.txt"), b"base\n").unwrap();
        fs::write(path.join("partial.txt"), b"p0\np1\np2\n").unwrap();
        git_ok(&path, &["add", "-A"]);
        git_ok(&path, &["commit", "-q", "-m", "initial"]);
        Self { _temp: temp, path }
    }

    fn lane() -> Self {
        Self::new(LANE)
    }

    fn open(&self) -> GitRepository {
        GitRepository::open(&self.path).unwrap()
    }

    fn destination(&self) -> BranchName {
        let observed = self.open().current_branch().unwrap().unwrap();
        BranchName::parse(&observed).unwrap()
    }

    fn write(&self, relative: &str, contents: &str) {
        let path = self.path.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    fn write_bytes(&self, relative: impl AsRef<Path>, contents: &[u8]) {
        let path = self.path.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    fn git(&self, args: &[&str]) -> String {
        String::from_utf8_lossy(&git_ok(&self.path, args)).into_owned()
    }

    fn git_bytes(&self, args: &[&str]) -> Vec<u8> {
        git_ok(&self.path, args)
    }

    fn git_raw(&self, args: &[&str]) -> std::process::Output {
        git_raw(&self.path, args)
    }

    fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"]).trim().to_string()
    }

    fn stage_snapshot(&self) -> Vec<u8> {
        self.git_bytes(&["ls-files", "--stage", "-z"])
    }

    fn status(&self) -> Vec<String> {
        let raw = self.git_bytes(&["status", "--porcelain=v1", "-z", "--untracked-files=all"]);
        let mut entries = Vec::new();
        let mut parts = raw.split(|byte| *byte == 0);
        while let Some(record) = parts.next() {
            if record.is_empty() {
                continue;
            }
            let code = String::from_utf8_lossy(&record[..2]).into_owned();
            let path = String::from_utf8_lossy(&record[3..]).into_owned();
            entries.push(format!("{code} {path}"));
            if matches!(code.as_bytes().first(), Some(b'R' | b'C')) {
                // The rename/copy source is the next NUL record.
                let _ = parts.next();
            }
        }
        entries.sort();
        entries
    }

    fn tree_paths(&self) -> Vec<String> {
        let raw = self.git_bytes(&["ls-tree", "-r", "--name-only", "-z", "HEAD"]);
        let mut paths: Vec<String> = raw
            .split(|byte| *byte == 0)
            .filter(|record| !record.is_empty())
            .map(|record| String::from_utf8_lossy(record).into_owned())
            .collect();
        paths.sort();
        paths
    }

    fn commit_paths(&self) -> Vec<String> {
        let raw = self.git_bytes(&[
            "-c",
            "diff.relative=false",
            "show",
            "--name-only",
            "--format=",
            "-z",
            "--no-renames",
            "HEAD",
        ]);
        let mut paths: Vec<String> = raw
            .split(|byte| *byte == 0)
            .filter(|record| !record.is_empty())
            .map(|record| String::from_utf8_lossy(record).into_owned())
            .collect();
        paths.sort();
        paths
    }

    fn install_hook(&self, name: &str, script: &str) {
        let hooks = self.path.join(".git").join("hooks");
        fs::create_dir_all(&hooks).unwrap();
        let hook = hooks.join(name);
        fs::write(&hook, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    fn path_of(&self, relative: &str) -> GitPath {
        GitPath::from_git_bytes(relative.as_bytes()).unwrap()
    }

    fn request(&self, message: &str, paths: &[&str]) -> SafeCommitRequest {
        SafeCommitRequest::new(
            self.destination(),
            message,
            paths.iter().map(|path| self.path_of(path)),
        )
    }
}

fn git_ok(cwd: &Path, args: &[&str]) -> Vec<u8> {
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

fn git_raw(cwd: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run git")
}

fn assert_landed(error: &GitError, kind: SafeCommitLandedKind) -> &SafeCommitRecovery {
    match error {
        GitError::SafeCommitLanded {
            kind: observed,
            recovery,
            ..
        } => {
            assert_eq!(*observed, kind, "unexpected landed kind in {error:?}");
            recovery
        }
        other => panic!("expected SafeCommitLanded({kind:?}), got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Happy path, message, branch and route
// ---------------------------------------------------------------------------

#[test]
fn commits_only_the_requested_file_on_the_declared_branch() {
    let repo = Repo::lane();
    repo.write("alpha.txt", "alpha v1\n");

    let outcome = repo
        .open()
        .safe_commit(&repo.request("WP01: add alpha", &["alpha.txt"]))
        .unwrap();

    assert_eq!(outcome.commit().as_str().len(), 40);
    assert_eq!(outcome.destination().as_str(), LANE);
    assert_eq!(
        outcome.worktree_root(),
        fs::canonicalize(&repo.path).unwrap()
    );
    assert_eq!(
        outcome.committed_paths(),
        vec![repo.path_of("alpha.txt")].as_slice()
    );
    assert!(outcome.warnings().is_empty());
    assert_eq!(outcome.commit().as_str(), repo.head());
    assert_eq!(
        repo.git(&["log", "-1", "--format=%s"]).trim(),
        "WP01: add alpha"
    );
    assert_eq!(
        repo.tree_paths(),
        vec!["a.txt", "alpha.txt", "other.txt", "partial.txt", "seed.txt"]
    );
    assert!(repo.status().is_empty());
}

#[test]
fn plan_records_the_worktree_route_and_guard_verdict() {
    let repo = Repo::lane();
    repo.write("alpha.txt", "alpha\n");
    let request = repo.request("plan", &["alpha.txt"]).with_protection(
        ProtectedBranches::parse(["main"]).unwrap(),
        CommitGuardCapability::Standard,
    );

    let plan = repo.open().plan_safe_commit(&request).unwrap();

    assert_eq!(plan.route(), SafeCommitRoute::Worktree);
    assert_eq!(plan.destination().as_str(), LANE);
    assert_eq!(plan.head(), plan.destination_tip());
    assert_eq!(plan.paths(), &[repo.path_of("alpha.txt")]);
    assert!(plan.index_deletions().is_empty());
    assert!(plan.guard_verdict().allowed());
    assert!(
        plan.guard_verdict()
            .reason()
            .contains("not in the resolved protected set")
    );
}

#[test]
fn preserves_unrelated_staged_partial_and_untracked_content() {
    let repo = Repo::lane();
    repo.write("a.txt", "two\n");
    repo.write("other.txt", "changed\n");
    repo.git(&["add", "other.txt"]);
    repo.write("partial.txt", "p0\np1-WIP\np2\n");
    repo.git(&["add", "partial.txt"]);
    repo.write("partial.txt", "p0\np1-WIP\np2-WIP\n");
    repo.write("u.txt", "u\n");
    repo.write("new.txt", "new\n");

    let outcome = repo
        .open()
        .safe_commit(&repo.request("diff: two", &["a.txt", "new.txt"]))
        .unwrap();

    assert_eq!(outcome.committed_paths().len(), 2);
    assert_eq!(repo.commit_paths(), vec!["a.txt", "new.txt"]);
    assert_eq!(repo.git(&["show", "HEAD:other.txt"]), "base\n");
    assert_eq!(repo.git(&["show", "HEAD:partial.txt"]), "p0\np1\np2\n");
    assert_eq!(
        repo.status(),
        vec!["?? u.txt", "M  other.txt", "MM partial.txt"]
    );
    // The operator's unrelated staged blobs are exactly what they staged.
    assert_eq!(repo.git(&["show", ":other.txt"]), "changed\n");
}

#[test]
fn commits_absolute_paths_relative_to_the_worktree() {
    let repo = Repo::lane();
    repo.write("nested/deep/file.txt", "deep\n");
    let absolute = repo.path.join("nested").join("deep").join("file.txt");

    let normalized = normalize_safe_commit_path(repo.open().root(), &absolute).unwrap();
    assert_eq!(normalized, repo.path_of("nested/deep/file.txt"));

    let outcome = repo
        .open()
        .safe_commit(&repo.request("absolute", &["nested/deep/file.txt"]))
        .unwrap();
    assert_eq!(outcome.committed_paths(), vec![normalized].as_slice());
}

#[test]
fn commits_an_explicitly_ignored_file() {
    let repo = Repo::lane();
    repo.write(".gitignore", "*.log\n");
    repo.git(&["add", ".gitignore"]);
    repo.git(&["commit", "-q", "-m", "ignore logs"]);
    repo.write("debug.log", "log\n");
    assert!(
        repo.git_raw(&["check-ignore", "-q", "debug.log"])
            .status
            .success(),
        "debug.log must be ignored"
    );

    let outcome = repo
        .open()
        .safe_commit(&repo.request("force ignored", &["debug.log"]))
        .unwrap();

    assert_eq!(
        outcome.committed_paths(),
        vec![repo.path_of("debug.log")].as_slice()
    );
}

#[cfg(unix)]
#[test]
fn preserves_non_utf8_path_bytes() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let repo = Repo::lane();
    let raw_name = b"caf\xe9-\xff.txt";
    repo.write_bytes(OsStr::from_bytes(raw_name), b"bytes\n");
    let path = GitPath::from_git_bytes(raw_name).unwrap();

    let request = SafeCommitRequest::new(repo.destination(), "non utf8 path", [path.clone()]);
    let outcome = repo.open().safe_commit(&request).unwrap();

    assert_eq!(outcome.committed_paths(), std::slice::from_ref(&path));
    assert_eq!(
        outcome.committed_paths()[0].as_os_str(),
        OsStr::from_bytes(raw_name)
    );
    let listed = repo.git_bytes(&["ls-tree", "-r", "--name-only", "-z", "HEAD"]);
    assert!(
        listed
            .windows(raw_name.len())
            .any(|window| window == raw_name),
        "the exact non-UTF-8 bytes must be in the commit tree"
    );
}

#[cfg(unix)]
#[test]
fn commits_a_symlink_as_a_link_not_its_target() {
    use std::os::unix::fs::symlink;

    let repo = Repo::lane();
    repo.write("real.md", "base\n");
    repo.git(&["add", "real.md"]);
    repo.git(&["commit", "-q", "-m", "real"]);
    repo.write("real.md", "base\nWIP\n");
    symlink("real.md", repo.path.join("link.md")).unwrap();

    repo.open()
        .safe_commit(&repo.request("link", &["link.md"]))
        .unwrap();

    assert!(
        repo.git(&["ls-tree", "HEAD", "link.md"])
            .starts_with("120000 ")
    );
    assert_eq!(repo.git(&["show", "HEAD:link.md"]), "real.md");
    // The target's unstaged modification is untouched and uncommitted.
    assert!(repo.git(&["diff", "--", "real.md"]).contains("+WIP"));
}

#[test]
fn refuses_a_symlink_loop_before_touching_the_index() {
    let repo = Repo::lane();
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        symlink("b", repo.path.join("a")).unwrap();
        symlink("a", repo.path.join("b")).unwrap();
    }
    #[cfg(not(unix))]
    {
        // A platform without symlinks cannot represent the case; the refusal
        // path is covered by the Unix job.
        return;
    }
    let before = repo.stage_snapshot();
    let absolute = repo.path.join("a");

    let error = repo
        .open()
        .safe_commit(&SafeCommitRequest::new(
            repo.destination(),
            "loop",
            [normalize_safe_commit_path(repo.open().root(), &absolute)
                .unwrap_or_else(|_| repo.path_of("a"))],
        ))
        .unwrap_err();

    assert!(
        matches!(error, GitError::SafeCommitPathLoop { .. }),
        "unexpected error: {error:?}"
    );
    assert_eq!(repo.stage_snapshot(), before);
}

#[test]
fn commits_a_rename_pair_reported_separately() {
    let repo = Repo::lane();
    repo.write("old.txt", "old content\n");
    repo.git(&["add", "old.txt"]);
    repo.git(&["commit", "-q", "-m", "add old"]);
    repo.git(&["mv", "old.txt", "new.txt"]);

    repo.open()
        .safe_commit(&repo.request("rename", &["old.txt", "new.txt"]))
        .unwrap();

    assert_eq!(repo.commit_paths(), vec!["new.txt", "old.txt"]);
    assert_eq!(repo.git(&["show", "HEAD:new.txt"]), "old content\n");
    assert!(!repo.path.join("old.txt").exists());
    assert!(repo.status().is_empty(), "status: {:?}", repo.status());
}

#[test]
fn commits_a_worktree_deletion_of_a_tracked_file() {
    let repo = Repo::lane();
    fs::remove_file(repo.path.join("a.txt")).unwrap();

    let outcome = repo
        .open()
        .safe_commit(&repo.request("delete a", &["a.txt"]))
        .unwrap();

    assert_eq!(
        outcome.committed_paths(),
        vec![repo.path_of("a.txt")].as_slice()
    );
    assert!(
        !repo
            .git_raw(&["cat-file", "-e", "HEAD:a.txt"])
            .status
            .success(),
        "the commit must record the deletion"
    );
    assert!(repo.status().is_empty(), "status: {:?}", repo.status());
}

#[test]
fn treats_looks_like_pathspec_magic_paths_as_data() {
    let repo = Repo::lane();
    let names = [
        "-x.txt",
        "--output=/tmp/pwned",
        "*.txt",
        ":(glob)*.txt",
        "a[bc].txt",
        "colon:name.txt",
        "new\nline.txt",
    ];
    for name in names {
        repo.write(name, &format!("{name}\n"));
    }
    let output_marker = Path::new("/tmp/pwned");
    let _ = fs::remove_file(output_marker);

    for name in names {
        repo.open()
            .safe_commit(&repo.request("literal", &[name]))
            .unwrap_or_else(|error| panic!("{name}: {error:?}"));
        assert_eq!(
            repo.commit_paths(),
            vec![name],
            "committed set for {name:?}"
        );
        assert_eq!(
            repo.git(&["show", &format!("HEAD:{name}")]),
            format!("{name}\n")
        );
    }
    assert!(
        !output_marker.exists(),
        "a path must never become a git option"
    );
    assert!(repo.status().is_empty(), "status: {:?}", repo.status());
}

#[test]
fn normalisation_refuses_parent_escape_and_outside_paths() {
    let repo = Repo::lane();
    let lane = repo.path.join("sub");
    fs::create_dir_all(&lane).unwrap();

    let outside = repo.path.parent().unwrap().join("outside.txt");
    fs::write(&outside, b"outside\n").unwrap();
    let error = normalize_safe_commit_path(repo.open().root(), &outside).unwrap_err();
    assert!(
        matches!(error, GitError::SafeCommitPathOutsideWorktree { .. }),
        "unexpected error: {error:?}"
    );

    let error =
        normalize_safe_commit_path(repo.open().root(), Path::new("../outside.txt")).unwrap_err();
    assert!(
        matches!(error, GitError::SafeCommitPathOutsideWorktree { .. }),
        "unexpected error: {error:?}"
    );
}

// ---------------------------------------------------------------------------
// Refusals that must not touch HEAD or the index
// ---------------------------------------------------------------------------

#[test]
fn refuses_an_empty_path_set() {
    let repo = Repo::lane();
    let error = repo
        .open()
        .safe_commit(&SafeCommitRequest::new(repo.destination(), "empty", []))
        .unwrap_err();
    assert!(matches!(error, GitError::SafeCommitEmptyChangeset));
}

#[test]
fn refuses_an_empty_message_before_mutation() {
    let repo = Repo::lane();
    repo.write("alpha.txt", "alpha\n");
    let before = repo.stage_snapshot();
    let error = repo
        .open()
        .safe_commit(&repo.request("", &["alpha.txt"]))
        .unwrap_err();
    assert!(matches!(error, GitError::SafeCommitInvalidRequest { .. }));
    assert_eq!(repo.stage_snapshot(), before);
}

#[test]
fn refuses_a_head_that_is_not_the_declared_destination() {
    let repo = Repo::lane();
    repo.write("alpha.txt", "alpha\n");
    let before = repo.stage_snapshot();
    let request = SafeCommitRequest::new(
        BranchName::parse("kitty/other-branch").unwrap(),
        "wrong branch",
        [repo.path_of("alpha.txt")],
    );

    let error = repo.open().safe_commit(&request).unwrap_err();

    match error {
        GitError::SafeCommitHeadMismatch {
            destination,
            observed_head,
            ..
        } => {
            assert_eq!(destination, "kitty/other-branch");
            assert_eq!(observed_head, LANE);
        }
        other => panic!("unexpected error: {other:?}"),
    }
    assert_eq!(repo.stage_snapshot(), before);
}

#[test]
fn refuses_a_detached_head() {
    let repo = Repo::lane();
    repo.write("alpha.txt", "alpha\n");
    let request = repo.request("detached", &["alpha.txt"]);
    // Make HEAD detached after the request captured the declared branch.
    git_ok(&repo.path, &["checkout", "--detach", "-q"]);
    let error = repo.open().safe_commit(&request).unwrap_err();
    assert!(
        matches!(error, GitError::SafeCommitHeadMismatch { .. }),
        "unexpected error: {error:?}"
    );
}

#[test]
fn refuses_a_directory_argument_before_any_mutation() {
    let repo = Repo::lane();
    repo.write("sub/one.txt", "one\n");
    repo.write("sub/two.txt", "two\n");
    let before = repo.stage_snapshot();

    let error = repo
        .open()
        .safe_commit(&repo.request("directory", &["sub"]))
        .unwrap_err();

    assert!(
        matches!(error, GitError::SafeCommitDirectoryArgument { .. }),
        "unexpected error: {error:?}"
    );
    assert_eq!(repo.stage_snapshot(), before);
    assert_eq!(repo.status(), vec!["?? sub/one.txt", "?? sub/two.txt"]);
}

#[test]
fn does_not_refuse_a_submodule_gitlink_as_a_directory() {
    let repo = Repo::lane();
    fs::create_dir_all(repo.path.join("vendor")).unwrap();
    let head = repo.head();
    // Record a gitlink entry without needing a real submodule checkout.
    repo.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("160000,{head},vendor"),
    ]);

    let plan = repo
        .open()
        .plan_safe_commit(&repo.request("gitlink", &["vendor"]))
        .unwrap();

    assert_eq!(plan.paths(), &[repo.path_of("vendor")]);
}

#[cfg(unix)]
#[test]
fn does_not_refuse_a_symlinked_directory_as_a_directory() {
    use std::os::unix::fs::symlink;

    let repo = Repo::lane();
    repo.write("real/child.txt", "child\n");
    symlink("real", repo.path.join("linkdir")).unwrap();

    let plan = repo
        .open()
        .plan_safe_commit(&repo.request("symlinked directory", &["linkdir"]))
        .unwrap();

    assert_eq!(plan.paths(), &[repo.path_of("linkdir")]);
}

#[test]
fn refuses_a_worktrees_path() {
    let repo = Repo::lane();
    repo.write(".worktrees/lane/file.txt", "lane\n");
    let before = repo.stage_snapshot();

    let error = repo
        .open()
        .safe_commit(&repo.request("policy", &[".worktrees/lane/file.txt"]))
        .unwrap_err();

    assert!(
        matches!(error, GitError::SafeCommitPathPolicy { .. }),
        "unexpected error: {error:?}"
    );
    assert_eq!(repo.stage_snapshot(), before);
}

#[test]
fn refuses_a_protected_branch_without_a_capability() {
    let repo = Repo::new("main");
    repo.write("gated.txt", "gated\n");
    let before = repo.stage_snapshot();

    let error = repo
        .open()
        .safe_commit(
            &repo
                .request("release: looks privileged", &["gated.txt"])
                .with_protection(
                    ProtectedBranches::parse(["main"]).unwrap(),
                    CommitGuardCapability::Standard,
                ),
        )
        .unwrap_err();

    assert!(
        matches!(error, GitError::SafeCommitProtectedBranch { .. }),
        "unexpected error: {error:?}"
    );
    assert_eq!(repo.stage_snapshot(), before);
    assert_eq!(repo.status(), vec!["?? gated.txt"]);
}

#[test]
fn allows_a_protected_branch_with_a_bookkeeping_capability() {
    let repo = Repo::new("main");
    repo.write("bookkeeping.txt", "done\n");

    let outcome = repo
        .open()
        .safe_commit(
            &repo
                .request("upgrade: record", &["bookkeeping.txt"])
                .with_protection(
                    ProtectedBranches::parse(["main"]).unwrap(),
                    CommitGuardCapability::UpgradeBookkeeping,
                ),
        )
        .unwrap();

    assert_eq!(outcome.committed_paths().len(), 1);
}

#[test]
fn refuses_while_an_index_lock_is_held_and_never_deletes_it() {
    let repo = Repo::lane();
    repo.write("alpha.txt", "alpha\n");
    let lock = repo.path.join(".git").join("index.lock");
    fs::write(&lock, b"").unwrap();
    let before = repo.stage_snapshot();

    let error = repo
        .open()
        .safe_commit(&repo.request("locked", &["alpha.txt"]))
        .unwrap_err();

    assert!(
        matches!(error, GitError::OperationInProgress { ref operation } if operation == "index-lock"),
        "unexpected error: {error:?}"
    );
    assert!(lock.exists(), "the lock belongs to another writer");
    assert_eq!(repo.stage_snapshot(), before);

    fs::remove_file(&lock).unwrap();
    repo.open()
        .safe_commit(&repo.request("locked", &["alpha.txt"]))
        .unwrap();
}

#[test]
fn refuses_exact_bytes_without_a_pinned_parent() {
    let repo = Repo::lane();
    repo.write("alpha.txt", "alpha\n");
    let request = repo
        .request("bytes", &["alpha.txt"])
        .with_expected_bytes([ExpectedBlob::new(repo.path_of("alpha.txt"), b"alpha\n")]);

    let error = repo.open().safe_commit(&request).unwrap_err();

    assert!(matches!(error, GitError::SafeCommitInvalidRequest { .. }));
}

#[test]
fn refuses_expected_bytes_for_a_path_that_was_not_requested() {
    let repo = Repo::lane();
    repo.write("alpha.txt", "alpha\n");
    let parent = sk_git::Oid::parse(&repo.head()).unwrap();
    let request = repo
        .request("bytes", &["alpha.txt"])
        .with_expected_parent(parent)
        .with_expected_bytes([ExpectedBlob::new(repo.path_of("other.txt"), b"base\n")]);

    let error = repo.open().safe_commit(&request).unwrap_err();

    assert!(matches!(error, GitError::SafeCommitInvalidRequest { .. }));
}

#[test]
fn a_sibling_worktrees_lock_does_not_block_this_checkout() {
    let repo = Repo::lane();
    let sibling = repo._temp.path().join("sibling");
    git_ok(
        &repo.path,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "kitty/mission-safe-sibling",
            sibling.to_str().unwrap(),
        ],
    );
    // A lock in the primary checkout's own git dir must not be read as ours.
    let foreign_lock = repo.path.join(".git").join("index.lock");
    fs::write(&foreign_lock, b"").unwrap();
    fs::write(sibling.join("alpha.txt"), b"alpha\n").unwrap();

    let sibling_repository = GitRepository::open(&sibling).unwrap();
    let destination = BranchName::parse("kitty/mission-safe-sibling").unwrap();
    let request = SafeCommitRequest::new(
        destination,
        "sibling commit",
        [GitPath::from_git_bytes(b"alpha.txt").unwrap()],
    );
    let outcome = sibling_repository.safe_commit(&request).unwrap();

    assert_eq!(outcome.committed_paths().len(), 1);
    assert!(
        foreign_lock.exists(),
        "the sibling lock is not ours to remove"
    );
}

#[test]
fn a_failing_configured_signature_never_moves_the_ref() {
    for pinned in [false, true] {
        let repo = Repo::lane();
        git_ok(&repo.path, &["config", "commit.gpgsign", "true"]);
        git_ok(&repo.path, &["config", "gpg.program", "/bin/false"]);
        repo.write("alpha.txt", "alpha\n");
        let parent = sk_git::Oid::parse(&repo.head()).unwrap();
        let head = repo.head();

        let mut request = repo.request("signed", &["alpha.txt"]);
        if pinned {
            request = request.with_expected_parent(parent);
        }
        let result = repo.open().safe_commit(&request);
        if pinned {
            let error = result.unwrap_err();
            assert!(
                matches!(
                    error,
                    GitError::SafeCommitCommitFailed {
                        status: Some(1),
                        ..
                    }
                ),
                "unexpected error: {error:?}"
            );
            assert_eq!(repo.head(), head, "a signing failure must not move the ref");
        } else {
            result.expect("ordinary upstream route explicitly forces unsigned commits");
            assert_ne!(repo.head(), head);
            assert!(
                !repo
                    .git(&["cat-file", "commit", "HEAD"])
                    .contains("\ngpgsig ")
            );
        }
    }
}

// ---------------------------------------------------------------------------
// No-op vs failed commit classification
// ---------------------------------------------------------------------------

#[test]
fn identical_content_is_a_benign_no_op_and_preserves_other_staging() {
    let repo = Repo::lane();
    repo.write("other.txt", "staged elsewhere\n");
    repo.git(&["add", "other.txt"]);
    let head = repo.head();
    let before = repo.stage_snapshot();

    let error = repo
        .open()
        .safe_commit(&repo.request("noop", &["a.txt"]))
        .unwrap_err();

    assert!(
        matches!(error, GitError::SafeCommitNothingToCommit { .. }),
        "unexpected error: {error:?}"
    );
    assert_eq!(repo.head(), head);
    assert_eq!(repo.stage_snapshot(), before);
    assert_eq!(repo.status(), vec!["M  other.txt"]);
}

#[test]
fn a_rejecting_hook_is_a_real_failure_not_an_empty_changeset() {
    let repo = Repo::lane();
    repo.install_hook(
        "pre-commit",
        "#!/bin/sh\necho 'pre-commit boom' >&2\nexit 1\n",
    );
    repo.write("blocked.txt", "blocked\n");
    let head = repo.head();
    let before = repo.stage_snapshot();

    let error = repo
        .open()
        .safe_commit(&repo.request("blocked", &["blocked.txt"]))
        .unwrap_err();

    match error {
        GitError::SafeCommitHookRejected { hook, detail } => {
            assert_eq!(hook, "pre-commit");
            assert!(detail.contains("pre-commit boom"), "detail: {detail}");
        }
        other => panic!("unexpected error: {other:?}"),
    }
    assert_eq!(repo.head(), head);
    assert_eq!(repo.stage_snapshot(), before);
    assert_eq!(repo.status(), vec!["?? blocked.txt"]);
}

#[test]
fn a_hook_printing_nothing_to_commit_still_classifies_as_failure() {
    let repo = Repo::lane();
    repo.install_hook(
        "pre-commit",
        "#!/bin/sh\necho 'nothing to commit, working tree clean'\nexit 1\n",
    );
    repo.write("blocked.txt", "blocked\n");

    let error = repo
        .open()
        .safe_commit(&repo.request("blocked", &["blocked.txt"]))
        .unwrap_err();

    assert!(
        matches!(error, GitError::SafeCommitHookRejected { .. }),
        "hook output text must not decide the classification: {error:?}"
    );
}

#[test]
fn a_failed_stage_leaves_the_real_index_exactly_as_found() {
    let repo = Repo::lane();
    repo.write("a.txt", "two\n");
    repo.git(&["add", "a.txt"]);
    // Make the requested path partially staged: the index carries "two", the
    // worktree carries a further edit.
    repo.write("a.txt", "two\nthree\n");
    let before = repo.git_bytes(&["diff", "--cached", "--binary", "--", "a.txt"]);
    let stage_snapshot = repo.stage_snapshot();

    let error = repo
        .open()
        .safe_commit(&repo.request("stage fail", &["a.txt", "missing.txt"]))
        .unwrap_err();

    assert!(
        matches!(error, GitError::SafeCommitStageFailed { .. }),
        "unexpected error: {error:?}"
    );
    // Staging happened only in the private index, so the operator's partially
    // staged file is byte-identical and the index snapshot is unchanged.
    assert_eq!(
        repo.git_bytes(&["diff", "--cached", "--binary", "--", "a.txt"]),
        before,
        "the requested path's staged state must be untouched"
    );
    assert_eq!(repo.stage_snapshot(), stage_snapshot);
    assert_eq!(repo.status(), vec!["MM a.txt"]);
}

#[test]
fn a_warn_mode_hook_warning_is_reported_on_a_successful_commit() {
    let repo = Repo::lane();
    repo.install_hook(
        "pre-commit",
        "#!/bin/sh\necho '[spec-kitty guard] WARNING: outside owned_files' >&2\nexit 0\n",
    );
    repo.write("alpha.txt", "alpha\n");

    let outcome = repo
        .open()
        .safe_commit(&repo.request("warn", &["alpha.txt"]))
        .unwrap();

    assert!(
        outcome
            .warnings()
            .iter()
            .any(|warning| warning.contains("outside owned_files")),
        "warnings: {:?}",
        outcome.warnings()
    );
}

#[test]
fn a_hook_that_sweeps_tracked_files_is_reported_as_landed_with_extras() {
    let repo = Repo::lane();
    repo.install_hook("pre-commit", "#!/bin/sh\ngit add -u\nexit 0\n");
    repo.write("other.txt", "operator modified\n");
    repo.write("spec.md", "# spec\n");

    let error = repo
        .open()
        .safe_commit(&repo.request("sweep", &["spec.md"]))
        .unwrap_err();

    let recovery = assert_landed(&error, SafeCommitLandedKind::UnrequestedPaths);
    let head = repo.head();
    assert_eq!(recovery.commit().as_str(), head);
    assert_eq!(recovery.affected_paths(), &[repo.path_of("other.txt")]);
    assert_eq!(repo.commit_paths(), vec!["other.txt", "spec.md"]);
    // The extra path's stale staged revert was resynchronised to the new HEAD.
    assert!(repo.status().is_empty(), "status: {:?}", repo.status());
}

#[test]
fn a_commit_that_lands_but_cannot_resync_the_index_says_so_and_keeps_the_state() {
    let repo = Repo::lane();
    // A hook sweeps an extra tracked file into the commit and, after git's own
    // lock is gone, leaves a foreign index lock behind. The post-landing resync
    // of the extra path then cannot run: the commit must be reported as landed,
    // and the foreign lock must not be deleted.
    repo.install_hook(
        "pre-commit",
        "#!/bin/sh\ngit add -u\ntouch .git/index.lock\nexit 0\n",
    );
    repo.write("other.txt", "operator modified\n");
    repo.write("spec.md", "# spec\n");

    let error = repo
        .open()
        .safe_commit(&repo.request("sweep locked", &["spec.md"]))
        .unwrap_err();

    let recovery = assert_landed(&error, SafeCommitLandedKind::IndexResyncFailed);
    assert!(
        repo.path.join(".git/index.lock").exists(),
        "foreign lock preserved"
    );
    assert_eq!(recovery.commit().as_str(), repo.head());
    assert_eq!(recovery.affected_paths(), &[repo.path_of("spec.md")]);
    // The commit still carries the swept path; it is the index that needs repair.
    assert_eq!(repo.commit_paths(), vec!["other.txt", "spec.md"]);
    let mut argv: Vec<String> = recovery
        .repair_argv(std::ffi::OsStr::new("git"))
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert_eq!(argv.remove(0), "git");
    assert_eq!(argv[0], "--literal-pathspecs");
    assert_eq!(argv[1], "reset");
    assert_eq!(argv[3], recovery.commit().as_str());
    assert!(argv.contains(&"spec.md".to_string()));
}

#[test]
fn an_index_deletion_commit_that_lands_but_cannot_resync_reports_the_landed_commit() {
    let repo = Repo::lane();
    repo.write("c.md", "c.md\n");
    repo.git(&["add", "c.md"]);
    repo.git(&["commit", "-q", "-m", "c"]);
    // The private index commit does not hold the operator's index lock, so the
    // hook can leave one behind for the post-landing resync to hit.
    repo.install_hook("pre-commit", "#!/bin/sh\ntouch .git/index.lock\nexit 0\n");

    let request = repo
        .request("landed deletion", &[])
        .with_index_deletions([repo.path_of("c.md")]);
    let error = repo.open().safe_commit(&request).unwrap_err();

    let recovery = assert_landed(&error, SafeCommitLandedKind::IndexResyncFailed);
    assert_eq!(recovery.commit().as_str(), repo.head());
    assert_eq!(recovery.affected_paths(), &[repo.path_of("c.md")]);
    assert!(repo.path.join("c.md").exists());
    assert!(repo.path.join(".git/index.lock").exists());
    assert!(
        !repo
            .git_raw(&["cat-file", "-e", "HEAD:c.md"])
            .status
            .success(),
        "the commit must carry the index deletion"
    );
}

// ---------------------------------------------------------------------------
// Index-only deletions
// ---------------------------------------------------------------------------

#[test]
fn commits_an_index_deletion_while_the_file_stays_and_operator_staging_is_untouched() {
    let repo = Repo::lane();
    repo.write("c.md", "c.md\n");
    repo.write("d.md", "d.md\n");
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-q", "-m", "more"]);
    repo.write("other.txt", "operator staged\n");
    repo.git(&["add", "other.txt"]);
    repo.write("d.md", "d.md\noperator unstaged\n");

    let request = repo
        .request("index deletion", &["a.txt"])
        .with_index_deletions([repo.path_of("c.md")]);
    let outcome = repo.open().safe_commit(&request).unwrap();

    assert_eq!(
        outcome.committed_paths(),
        vec![repo.path_of("c.md")].as_slice()
    );
    assert!(!repo.git(&["ls-files", "c.md"]).contains("c.md"));
    assert!(repo.path.join("c.md").exists(), "the file stays on disk");
    assert!(repo.path.join("d.md").exists());
    assert_eq!(repo.status(), vec![" M d.md", "?? c.md", "M  other.txt"]);
    assert!(
        repo.git(&["diff", "--cached", "--name-only"])
            .contains("other.txt"),
        "operator staging must survive"
    );
}

#[test]
fn an_untracked_index_deletion_is_a_no_op() {
    let repo = Repo::lane();
    repo.write("untracked.md", "untracked\n");
    let head = repo.head();
    let request = repo
        .request("untracked deletion", &[])
        .with_index_deletions([repo.path_of("untracked.md")]);

    let error = repo.open().safe_commit(&request).unwrap_err();

    assert!(matches!(error, GitError::SafeCommitNothingToCommit { .. }));
    assert_eq!(repo.head(), head);
    assert_eq!(repo.status(), vec!["?? untracked.md"]);
}

#[test]
fn refuses_a_path_that_is_both_requested_and_deleted() {
    let repo = Repo::lane();
    repo.write("c.md", "c.md\n");
    repo.git(&["add", "c.md"]);
    repo.git(&["commit", "-q", "-m", "c"]);
    let request = repo
        .request("conflict", &["c.md"])
        .with_index_deletions([repo.path_of("c.md")]);

    let error = repo.open().safe_commit(&request).unwrap_err();

    assert!(matches!(
        error,
        GitError::SafeCommitIndexDeletionConflict { .. }
    ));
}

#[test]
fn a_rejecting_hook_leaves_an_index_deletion_commit_unlanded_and_the_index_unchanged() {
    let repo = Repo::lane();
    repo.write("c.md", "c.md\n");
    repo.git(&["add", "c.md"]);
    repo.git(&["commit", "-q", "-m", "c"]);
    repo.install_hook("pre-commit", "#!/bin/sh\necho 'no deletions' >&2\nexit 1\n");
    let head = repo.head();
    let before = repo.stage_snapshot();

    let request = repo
        .request("blocked deletion", &[])
        .with_index_deletions([repo.path_of("c.md")]);
    let error = repo.open().safe_commit(&request).unwrap_err();

    assert!(matches!(error, GitError::SafeCommitHookRejected { .. }));
    assert_eq!(repo.head(), head);
    assert_eq!(repo.stage_snapshot(), before);
    assert!(repo.path.join("c.md").exists());
}

// ---------------------------------------------------------------------------
// Pinned-parent route: isolation, expected bytes, CAS, hooks
// ---------------------------------------------------------------------------

#[test]
fn a_pinned_parent_commit_runs_hooks_and_advances_with_a_cas() {
    let repo = Repo::lane();
    repo.write("alpha.txt", "alpha\n");
    let parent = sk_git::Oid::parse(&repo.head()).unwrap();
    let request = repo
        .request("pinned", &["alpha.txt"])
        .with_expected_parent(parent.clone());

    let plan = repo.open().plan_safe_commit(&request).unwrap();
    assert_eq!(plan.route(), SafeCommitRoute::PinnedParent);
    let outcome = repo.open().apply_safe_commit(&plan).unwrap();

    assert_eq!(
        outcome.committed_paths(),
        vec![repo.path_of("alpha.txt")].as_slice()
    );
    assert_eq!(
        repo.git(&["rev-parse", "HEAD^"]).trim(),
        parent.as_str(),
        "the pinned parent must be the sole parent"
    );
    assert_eq!(repo.git(&["rev-list", "--count", "HEAD"]).trim(), "2");
    assert!(repo.status().is_empty());
}

#[test]
fn a_pinned_parent_hook_cannot_add_paths_or_change_requested_content() {
    let repo = Repo::lane();
    repo.install_hook("pre-commit", "#!/bin/sh\ngit add -u\nexit 0\n");
    repo.write("other.txt", "operator modified\n");
    repo.write("spec.md", "# spec\n");
    let parent = sk_git::Oid::parse(&repo.head()).unwrap();

    let outcome = repo
        .open()
        .safe_commit(
            &repo
                .request("pinned isolated", &["spec.md"])
                .with_expected_parent(parent),
        )
        .unwrap();

    assert_eq!(
        outcome.committed_paths(),
        vec![repo.path_of("spec.md")].as_slice()
    );
    assert_eq!(repo.status(), vec![" M other.txt"]);
}

#[test]
fn a_pinned_parent_commit_verifies_exact_raw_bytes() {
    let repo = Repo::lane();
    repo.write("alpha.txt", "alpha exact\n");
    let parent = sk_git::Oid::parse(&repo.head()).unwrap();
    let matching = repo
        .request("bytes", &["alpha.txt"])
        .with_expected_parent(parent.clone())
        .with_expected_bytes([ExpectedBlob::new(
            repo.path_of("alpha.txt"),
            b"alpha exact\n",
        )]);
    repo.open().safe_commit(&matching).unwrap();

    repo.write("alpha.txt", "alpha other\n");
    let parent = sk_git::Oid::parse(&repo.head()).unwrap();
    let mismatching = repo
        .request("bytes", &["alpha.txt"])
        .with_expected_parent(parent.clone())
        .with_expected_bytes([ExpectedBlob::new(
            repo.path_of("alpha.txt"),
            b"alpha exact\n",
        )]);
    let head = repo.head();

    let error = repo.open().safe_commit(&mismatching).unwrap_err();

    assert!(
        matches!(error, GitError::SafeCommitExpectedBytesMismatch { .. }),
        "unexpected error: {error:?}"
    );
    assert_eq!(repo.head(), head, "no ref may move on a byte mismatch");
}

#[test]
fn a_stale_pinned_parent_refuses_without_moving_the_ref() {
    let repo = Repo::lane();
    let parent = sk_git::Oid::parse(&repo.head()).unwrap();
    repo.write("alpha.txt", "alpha\n");
    let request = repo
        .request("stale", &["alpha.txt"])
        .with_expected_parent(parent);

    // Another commit moves the branch after the plan was prepared.
    repo.write("elsewhere.txt", "elsewhere\n");
    repo.git(&["add", "elsewhere.txt"]);
    repo.git(&["commit", "-q", "-m", "concurrent"]);
    let head = repo.head();
    let plan_error = repo.open().plan_safe_commit(&request).unwrap_err();
    assert!(
        matches!(plan_error, GitError::PlanStale { .. }),
        "unexpected error: {plan_error:?}"
    );
    assert_eq!(repo.head(), head);
}

#[test]
fn applying_a_stale_plan_changes_nothing() {
    let repo = Repo::lane();
    repo.write("alpha.txt", "alpha\n");
    let plan = repo
        .open()
        .plan_safe_commit(&repo.request("stale apply", &["alpha.txt"]))
        .unwrap();

    repo.write("elsewhere.txt", "elsewhere\n");
    repo.git(&["add", "elsewhere.txt"]);
    repo.git(&["commit", "-q", "-m", "concurrent"]);
    let head = repo.head();
    let before = repo.stage_snapshot();

    let error = repo.open().apply_safe_commit(&plan).unwrap_err();

    assert!(
        matches!(error, GitError::PlanStale { .. }),
        "unexpected error: {error:?}"
    );
    assert_eq!(repo.head(), head);
    assert_eq!(repo.stage_snapshot(), before);
}

#[test]
fn a_pinned_parent_commit_with_an_index_only_removal_keeps_the_file() {
    let repo = Repo::lane();
    repo.write("lock.txt", "lock\n");
    repo.git(&["add", "lock.txt"]);
    repo.git(&["commit", "-q", "-m", "lock"]);
    let parent = sk_git::Oid::parse(&repo.head()).unwrap();
    let request = repo
        .request("pinned removal", &[])
        .with_index_deletions([repo.path_of("lock.txt")])
        .with_expected_parent(parent);

    let outcome = repo.open().safe_commit(&request).unwrap();

    assert_eq!(
        outcome.committed_paths(),
        vec![repo.path_of("lock.txt")].as_slice()
    );
    assert!(repo.path.join("lock.txt").exists());
    assert_eq!(repo.status(), vec!["?? lock.txt"]);
}

#[test]
fn a_pinned_parent_post_commit_hook_failure_is_a_warning_not_a_failure() {
    let repo = Repo::lane();
    repo.install_hook("post-commit", "#!/bin/sh\nexit 3\n");
    repo.write("alpha.txt", "alpha\n");
    let parent = sk_git::Oid::parse(&repo.head()).unwrap();

    let outcome = repo
        .open()
        .safe_commit(
            &repo
                .request("post hook", &["alpha.txt"])
                .with_expected_parent(parent),
        )
        .unwrap();

    assert_eq!(outcome.committed_paths().len(), 1);
    assert!(
        outcome
            .warnings()
            .iter()
            .any(|warning| warning.contains("post-commit")),
        "warnings: {:?}",
        outcome.warnings()
    );
}

#[test]
fn a_pinned_parent_commit_tree_matching_the_parent_is_a_no_op() {
    let repo = Repo::lane();
    let parent = sk_git::Oid::parse(&repo.head()).unwrap();
    let request = repo
        .request("noop pinned", &["a.txt"])
        .with_expected_parent(parent);

    let error = repo.open().safe_commit(&request).unwrap_err();

    assert!(matches!(error, GitError::SafeCommitNothingToCommit { .. }));
}

// ---------------------------------------------------------------------------
// Plan/apply identity and path normalisation surface
// ---------------------------------------------------------------------------

#[test]
fn applying_a_plan_to_another_checkout_is_refused() {
    let repo = Repo::lane();
    let other = Repo::lane();
    repo.write("alpha.txt", "alpha\n");
    let plan = repo
        .open()
        .plan_safe_commit(&repo.request("elsewhere", &["alpha.txt"]))
        .unwrap();

    let error = other.open().apply_safe_commit(&plan).unwrap_err();

    assert!(matches!(error, GitError::InvalidPlan { .. }));
}

#[test]
fn dot_relative_paths_are_normalized_without_weakening_parent_escape_refusal() {
    let repo = Repo::lane();
    repo.write("a.txt", "changed\n");
    assert_eq!(
        normalize_safe_commit_path(&repo.path, "./a.txt").unwrap(),
        repo.path_of("a.txt")
    );
    assert!(normalize_safe_commit_path(&repo.path, "../a.txt").is_err());
    repo.open()
        .safe_commit(&repo.request("dot relative", &["./a.txt"]))
        .unwrap();
    assert_eq!(repo.git(&["show", "HEAD:a.txt"]), "changed\n");
}

#[test]
fn ordinary_messages_use_git_cleanup_and_refuse_a_cleaned_empty_message() {
    let repo = Repo::lane();
    repo.write("a.txt", "changed\n");
    repo.open()
        .safe_commit(&repo.request("subject   \n\n\n\nbody\t\n\n", &["a.txt"]))
        .unwrap();
    let object = repo.git(&["cat-file", "commit", "HEAD"]);
    assert_eq!(object.split_once("\n\n").unwrap().1, "subject\n\nbody\n");
    repo.write("a.txt", "next\n");
    let before = repo.head();
    let index = repo.stage_snapshot();
    assert!(matches!(
        repo.open().safe_commit(&repo.request("   \n", &["a.txt"])),
        Err(GitError::SafeCommitCommitFailed { .. })
    ));
    assert_eq!(repo.head(), before);
    assert_eq!(repo.stage_snapshot(), index);
}

#[test]
fn ordinary_cleanup_configuration_and_pinned_verbatim_messages_are_distinct() {
    for (cleanup, expected) in [
        ("strip", "subject\n\nbody\n"),
        ("verbatim", "subject   \n# note\n\nbody\t\n"),
    ] {
        let repo = Repo::lane();
        repo.git(&["config", "commit.cleanup", cleanup]);
        repo.write("a.txt", "changed\n");
        repo.open()
            .safe_commit(&repo.request("subject   \n# note\n\nbody\t\n", &["a.txt"]))
            .unwrap();
        let object = repo.git(&["cat-file", "commit", "HEAD"]);
        assert_eq!(object.split_once("\n\n").unwrap().1, expected);
    }
    let repo = Repo::lane();
    repo.write("a.txt", "changed\n");
    let request = repo
        .request("subject   \n\nbody\t\n", &["a.txt"])
        .with_expected_parent(sk_git::Oid::parse(&repo.head()).unwrap());
    repo.open().safe_commit(&request).unwrap();
    assert_eq!(
        repo.git(&["cat-file", "commit", "HEAD"])
            .split_once("\n\n")
            .unwrap()
            .1,
        "subject   \n\nbody\t\n\n"
    );
}

#[test]
fn hook_usage_phrases_are_rejections_not_git_version_failures() {
    let repo = Repo::lane();
    repo.install_hook(
        "pre-commit",
        "#!/bin/sh\necho 'unknown command: rustfmt' >&2\nexit 1\n",
    );
    repo.write("a.txt", "changed\n");
    match repo
        .open()
        .safe_commit(&repo.request("rejected", &["a.txt"]))
        .unwrap_err()
    {
        GitError::SafeCommitHookRejected { hook, detail } => {
            assert_eq!(hook, "pre-commit");
            assert!(detail.contains("unknown command: rustfmt"));
        }
        error => panic!("misclassified rejection: {error:?}"),
    }
}

#[test]
fn no_op_candidates_still_run_the_repository_hooks() {
    let repo = Repo::lane();
    repo.install_hook("pre-commit", "#!/bin/sh\necho 'guard ran' >&2\nexit 1\n");
    assert!(matches!(
        repo.open()
            .safe_commit(&repo.request("unchanged", &["a.txt"])),
        Err(GitError::SafeCommitHookRejected { .. })
    ));
}

#[test]
fn message_hooks_observe_cleanup_and_their_rewrites_are_committed() {
    let repo = Repo::lane();
    repo.install_hook(
        "prepare-commit-msg",
        "#!/bin/sh\ntest \"$2\" = message || exit 2\ncp \"$1\" .git/prepared-message\n",
    );
    repo.install_hook("commit-msg", "#!/bin/sh\necho 'rewritten   ' > \"$1\"\n");
    repo.write("a.txt", "changed\n");
    repo.open()
        .safe_commit(&repo.request("subject   \n\n\nbody\t", &["a.txt"]))
        .unwrap();
    assert_eq!(
        fs::read_to_string(repo.path.join(".git/prepared-message")).unwrap(),
        "subject\n\nbody\n"
    );
    assert_eq!(
        repo.git(&["cat-file", "commit", "HEAD"])
            .split_once("\n\n")
            .unwrap()
            .1,
        "rewritten\n"
    );
    assert_eq!(
        repo.git(&["reflog", "-1", "--format=%gs"]),
        "commit: rewritten\n"
    );
}

#[cfg(unix)]
#[test]
fn reconciliation_uses_the_landed_commit_even_when_head_advances_again() {
    use std::os::unix::fs::PermissionsExt;
    for pinned in [false, true] {
        let repo = Repo::lane();
        repo.write("a.txt", "competitor\n");
        repo.git(&["add", "a.txt"]);
        let tree = repo.git(&["write-tree"]).trim().to_owned();
        repo.git(&["reset", "--quiet", "HEAD", "--", "a.txt"]);
        repo.write("a.txt", "ours\n");
        let wrapper = repo._temp.path().join("git-interpose.sh");
        fs::write(&wrapper, format!("#!/bin/sh\nif test \"$1\" = --literal-pathspecs && test \"$2\" = reset && ! test -e .git/interposed; then\n touch .git/interposed\n parent=$(git rev-parse HEAD) || exit\n competitor=$(echo competitor | git commit-tree {tree} -p \"$parent\") || exit\n git update-ref refs/heads/{LANE} \"$competitor\" \"$parent\" || exit\nfi\nexec git \"$@\"\n")).unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
        let options = sk_git::GitOptions {
            program: wrapper,
            ..Default::default()
        };
        let git = GitRepository::open_with_options(&repo.path, &options).unwrap();
        let mut request = repo.request("ours", &["a.txt"]);
        if pinned {
            request = request.with_expected_parent(sk_git::Oid::parse(&repo.head()).unwrap());
        }
        let outcome = git.safe_commit(&request).unwrap();
        assert_ne!(repo.head(), outcome.commit().as_str());
        assert_eq!(repo.git(&["show", ":a.txt"]), "ours\n");
        assert_eq!(repo.git(&["show", "HEAD:a.txt"]), "competitor\n");
        assert_eq!(
            repo.git(&["show", &format!("{}:a.txt", outcome.commit())]),
            "ours\n"
        );
    }
}

#[test]
fn a_hook_nested_commit_returns_candidate_and_observed_history_not_a_clean_refusal() {
    let repo = Repo::lane();
    repo.write("a.txt", "changed\n");
    repo.install_hook(
        "pre-commit",
        "#!/bin/sh\ngit commit --no-verify --allow-empty -qm 'nested from hook'\n",
    );
    match repo
        .open()
        .safe_commit(&repo.request("outer", &["a.txt"]))
        .unwrap_err()
    {
        GitError::SafeCommitAdvanceUnconfirmed {
            candidate,
            observed_tip: Some(observed),
            detail,
            ..
        } => {
            assert_eq!(observed.as_str(), repo.head());
            assert_ne!(candidate, observed);
            assert_eq!(
                repo.git(&["cat-file", "-t", candidate.as_str()]),
                "commit\n"
            );
            assert!(detail.contains("do not assume nothing happened"));
        }
        error => panic!("lost post-hook outcome evidence: {error:?}"),
    }
    assert_eq!(
        repo.git(&["log", "-1", "--format=%s"]),
        "nested from hook\n"
    );
}

#[test]
fn requested_paths_reports_adds_and_removals() {
    let repo = Repo::lane();
    repo.write("alpha.txt", "alpha\n");
    repo.write("c.md", "c\n");
    repo.git(&["add", "c.md"]);
    repo.git(&["commit", "-q", "-m", "c"]);
    let request = repo
        .request("sets", &["alpha.txt"])
        .with_index_deletions([repo.path_of("c.md")]);
    let plan = repo.open().plan_safe_commit(&request).unwrap();

    assert_eq!(
        plan.requested_paths(),
        vec![repo.path_of("alpha.txt"), repo.path_of("c.md")]
    );
}
