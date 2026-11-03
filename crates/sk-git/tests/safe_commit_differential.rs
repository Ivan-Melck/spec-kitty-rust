//! Pinned differential tests against the upstream Python safe-commit oracle.
//!
//! Oracle: `spec-kitty/spec-kitty` at
//! `9aed3a8012cb5eb22c0128b14d525039f95fea7a`, executed with
//! `.venv-rust-oracle/bin/python` (Python 3.11, `PYTHONPATH=src`, temporary
//! `HOME`/`XDG_*`), calling `specify_cli.git.commit_helpers.safe_commit` on a
//! fresh temporary repository with a pinned branch name and pinned file
//! contents. The expectations below are the *observable structural outcome* the
//! oracle produced (committed path set, index/worktree status, whether HEAD
//! moved, which typed refusal fired). Object ids and timestamps are deliberately
//! not pinned: they are not part of the contract.
//!
//! Regenerate by running the same eight scenarios through the oracle and
//! replacing the `Oracle*` constants. Python is development tooling only; the
//! tests below never invoke it.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sk_git::{
    BranchName, CommitGuardCapability, GitError, GitPath, GitRepository, ProtectedBranches,
    SafeCommitLandedKind, SafeCommitRequest,
};
use tempfile::TempDir;

const LANE: &str = "kitty/mission-diff-01ABCDEF";

// Pinned oracle expectations -------------------------------------------------

const ORACLE_PRESERVE_STATUS: &[&str] = &["?? u.txt", "M  other.txt", "MM partial.txt"];
const ORACLE_PRESERVE_COMMITTED: &[&str] = &["a.txt", "new.txt"];
const ORACLE_PRESERVE_OTHER_IN_HEAD: &str = "base\n";
const ORACLE_PRESERVE_PARTIAL_IN_HEAD: &str = "p0\np1\np2\n";

const ORACLE_RENAME_COMMITTED: &[&str] = &["new.txt", "old.txt"];
const ORACLE_RENAME_NEW_IN_HEAD: &str = "old content\n";

const ORACLE_NOOP_STATUS: &[&str] = &["M  other.txt"];

const ORACLE_DIRECTORY_STATUS: &[&str] = &["?? sub/one.txt", "?? sub/two.txt"];

const ORACLE_PROTECTED_STATUS: &[&str] = &["?? gated.txt"];

const ORACLE_HOOK_FAILURE_STATUS: &[&str] = &["?? blocked.txt"];
const ORACLE_HOOK_FAILURE_DETAIL: &str = "pre-commit boom";

const ORACLE_INDEX_DELETION_STATUS: &[&str] = &[" M d.md", "?? c.md", "M  other.txt"];
const ORACLE_INDEX_DELETION_COMMITTED: &[&str] = &["c.md"];

const ORACLE_HOOK_SWEEP_COMMITTED: &[&str] = &["other.txt", "spec.md"];

struct Fixture {
    _temp: TempDir,
    path: PathBuf,
}

impl Fixture {
    fn new(branch: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("repo");
        fs::create_dir_all(&path).unwrap();
        git_ok(&path, &["init", "-q", "-b", branch]);
        git_ok(&path, &["config", "user.name", "Test User"]);
        git_ok(&path, &["config", "user.email", "test@example.com"]);
        git_ok(&path, &["config", "commit.gpgsign", "false"]);
        write(&path, "seed.txt", "seed\n");
        write(&path, "a.txt", "one\n");
        write(&path, "other.txt", "base\n");
        write(&path, "partial.txt", "p0\np1\np2\n");
        git_ok(&path, &["add", "-A"]);
        git_ok(&path, &["commit", "-q", "-m", "initial"]);
        Self { _temp: temp, path }
    }

    fn lane() -> Self {
        Self::new(LANE)
    }

    fn destination(&self) -> BranchName {
        let observed = GitRepository::open(&self.path)
            .unwrap()
            .current_branch()
            .unwrap()
            .unwrap();
        BranchName::parse(&observed).unwrap()
    }

    fn request(&self, message: &str, paths: &[&str]) -> SafeCommitRequest {
        SafeCommitRequest::new(
            self.destination(),
            message,
            paths
                .iter()
                .map(|path| GitPath::from_git_bytes(path.as_bytes()).unwrap()),
        )
    }

    fn status(&self) -> Vec<String> {
        status(&self.path)
    }

    fn committed_paths(&self) -> Vec<String> {
        committed_paths(&self.path)
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
}

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

fn git_ok(cwd: &Path, args: &[&str]) -> Vec<u8> {
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

fn git_text(cwd: &Path, args: &[&str]) -> String {
    String::from_utf8_lossy(&git_ok(cwd, args)).into_owned()
}

fn status(root: &Path) -> Vec<String> {
    let raw = git_ok(
        root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    );
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
            let _ = parts.next();
        }
    }
    entries.sort();
    entries
}

fn committed_paths(root: &Path) -> Vec<String> {
    let raw = git_ok(
        root,
        &[
            "-c",
            "diff.relative=false",
            "show",
            "--name-only",
            "--format=",
            "-z",
            "--no-renames",
            "HEAD",
        ],
    );
    let mut paths: Vec<String> = raw
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .map(|record| String::from_utf8_lossy(record).into_owned())
        .collect();
    paths.sort();
    paths
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

#[test]
fn oracle_preserves_unrelated_staged_partial_and_untracked_content() {
    let fixture = Fixture::lane();
    write(&fixture.path, "a.txt", "two\n");
    write(&fixture.path, "other.txt", "changed\n");
    git_ok(&fixture.path, &["add", "other.txt"]);
    write(&fixture.path, "partial.txt", "p0\np1-WIP\np2\n");
    git_ok(&fixture.path, &["add", "partial.txt"]);
    write(&fixture.path, "partial.txt", "p0\np1-WIP\np2-WIP\n");
    write(&fixture.path, "u.txt", "u\n");
    write(&fixture.path, "new.txt", "new\n");
    let head_before = git_text(&fixture.path, &["rev-parse", "HEAD"]);

    GitRepository::open(&fixture.path)
        .unwrap()
        .safe_commit(&fixture.request("diff: two", &["a.txt", "new.txt"]))
        .unwrap();

    assert_ne!(git_text(&fixture.path, &["rev-parse", "HEAD"]), head_before);
    assert_eq!(
        fixture.committed_paths(),
        strings(ORACLE_PRESERVE_COMMITTED)
    );
    assert_eq!(fixture.status(), strings(ORACLE_PRESERVE_STATUS));
    assert_eq!(
        git_text(&fixture.path, &["show", "HEAD:other.txt"]),
        ORACLE_PRESERVE_OTHER_IN_HEAD
    );
    assert_eq!(
        git_text(&fixture.path, &["show", "HEAD:partial.txt"]),
        ORACLE_PRESERVE_PARTIAL_IN_HEAD
    );
}

#[test]
fn oracle_commits_a_rename_pair_as_delete_plus_add() {
    let fixture = Fixture::lane();
    write(&fixture.path, "old.txt", "old content\n");
    git_ok(&fixture.path, &["add", "old.txt"]);
    git_ok(&fixture.path, &["commit", "-q", "-m", "add old"]);
    git_ok(&fixture.path, &["mv", "old.txt", "new.txt"]);

    GitRepository::open(&fixture.path)
        .unwrap()
        .safe_commit(&fixture.request("diff: rename", &["old.txt", "new.txt"]))
        .unwrap();

    assert_eq!(fixture.committed_paths(), strings(ORACLE_RENAME_COMMITTED));
    assert_eq!(
        git_text(&fixture.path, &["show", "HEAD:new.txt"]),
        ORACLE_RENAME_NEW_IN_HEAD
    );
    assert!(
        fixture.status().is_empty(),
        "status: {:?}",
        fixture.status()
    );
}

#[test]
fn oracle_classifies_identical_content_as_an_empty_changeset() {
    let fixture = Fixture::lane();
    write(&fixture.path, "other.txt", "staged elsewhere\n");
    git_ok(&fixture.path, &["add", "other.txt"]);
    let head_before = git_text(&fixture.path, &["rev-parse", "HEAD"]);

    let error = GitRepository::open(&fixture.path)
        .unwrap()
        .safe_commit(&fixture.request("diff: noop", &["a.txt"]))
        .unwrap_err();

    // Oracle: SafeCommitStagedTreeUnchanged.
    assert!(matches!(error, GitError::SafeCommitNothingToCommit { .. }));
    assert_eq!(git_text(&fixture.path, &["rev-parse", "HEAD"]), head_before);
    assert_eq!(fixture.status(), strings(ORACLE_NOOP_STATUS));
}

#[test]
fn oracle_refuses_a_directory_argument_before_mutation() {
    let fixture = Fixture::lane();
    write(&fixture.path, "sub/one.txt", "one\n");
    write(&fixture.path, "sub/two.txt", "two\n");
    let head_before = git_text(&fixture.path, &["rev-parse", "HEAD"]);
    let index_before = git_ok(&fixture.path, &["ls-files", "-s"]);

    let error = GitRepository::open(&fixture.path)
        .unwrap()
        .safe_commit(&fixture.request("diff: dir", &["sub"]))
        .unwrap_err();

    // Oracle: SAFE_COMMIT_DIRECTORY_ARGUMENT.
    assert!(matches!(
        error,
        GitError::SafeCommitDirectoryArgument { .. }
    ));
    assert_eq!(git_text(&fixture.path, &["rev-parse", "HEAD"]), head_before);
    assert_eq!(git_ok(&fixture.path, &["ls-files", "-s"]), index_before);
    assert_eq!(fixture.status(), strings(ORACLE_DIRECTORY_STATUS));
}

#[test]
fn oracle_refuses_a_protected_branch_with_standard_capability() {
    let fixture = Fixture::new("main");
    write(&fixture.path, "gated.txt", "gated\n");
    let head_before = git_text(&fixture.path, &["rev-parse", "HEAD"]);
    let index_before = git_ok(&fixture.path, &["ls-files", "-s"]);
    let request = fixture
        .request("diff: protected", &["gated.txt"])
        .with_protection(
            // The oracle's default resolved set for a project with no
            // `.kittify/config.yaml`: {main, master} plus the (absent) remote
            // default branch.
            ProtectedBranches::parse(["main", "master"]).unwrap(),
            CommitGuardCapability::Standard,
        );

    let error = GitRepository::open(&fixture.path)
        .unwrap()
        .safe_commit(&request)
        .unwrap_err();

    // Oracle: SAFE_COMMIT_PROTECTED_BRANCH.
    assert!(matches!(error, GitError::SafeCommitProtectedBranch { .. }));
    assert_eq!(git_text(&fixture.path, &["rev-parse", "HEAD"]), head_before);
    assert_eq!(git_ok(&fixture.path, &["ls-files", "-s"]), index_before);
    assert_eq!(fixture.status(), strings(ORACLE_PROTECTED_STATUS));
}

#[test]
fn oracle_treats_a_rejecting_hook_as_a_real_failure() {
    let fixture = Fixture::lane();
    fixture.install_hook(
        "pre-commit",
        "#!/bin/sh\necho 'pre-commit boom' >&2\nexit 1\n",
    );
    write(&fixture.path, "blocked.txt", "blocked\n");
    let head_before = git_text(&fixture.path, &["rev-parse", "HEAD"]);

    let error = GitRepository::open(&fixture.path)
        .unwrap()
        .safe_commit(&fixture.request("diff: blocked", &["blocked.txt"]))
        .unwrap_err();

    // Oracle: a generic RuntimeError that is NOT classified as an empty
    // changeset and carries the hook's stderr. This port refines it to the
    // typed SafeCommitHookRejected, which carries the same evidence.
    match error {
        GitError::SafeCommitHookRejected { hook, detail } => {
            assert_eq!(hook, "pre-commit");
            assert!(detail.contains(ORACLE_HOOK_FAILURE_DETAIL), "{detail}");
        }
        other => panic!("unexpected error: {other:?}"),
    }
    assert_eq!(git_text(&fixture.path, &["rev-parse", "HEAD"]), head_before);
    assert_eq!(fixture.status(), strings(ORACLE_HOOK_FAILURE_STATUS));
}

#[test]
fn oracle_commits_an_index_deletion_and_keeps_the_file() {
    let fixture = Fixture::lane();
    for name in ["c.md", "d.md"] {
        write(&fixture.path, name, &format!("{name}\n"));
    }
    git_ok(&fixture.path, &["add", "-A"]);
    git_ok(&fixture.path, &["commit", "-q", "-m", "more"]);
    write(&fixture.path, "other.txt", "operator staged\n");
    git_ok(&fixture.path, &["add", "other.txt"]);
    write(&fixture.path, "d.md", "d.md\noperator unstaged\n");
    let request = fixture
        .request("diff: index deletion", &["a.txt"])
        .with_index_deletions([GitPath::from_git_bytes(b"c.md").unwrap()]);

    GitRepository::open(&fixture.path)
        .unwrap()
        .safe_commit(&request)
        .unwrap();

    assert_eq!(
        fixture.committed_paths(),
        strings(ORACLE_INDEX_DELETION_COMMITTED)
    );
    assert_eq!(fixture.status(), strings(ORACLE_INDEX_DELETION_STATUS));
    assert!(fixture.path.join("c.md").exists());
    assert!(!git_text(&fixture.path, &["ls-files", "c.md"]).contains("c.md"));
    assert!(git_text(&fixture.path, &["diff", "--cached", "--name-only"]).contains("other.txt"));
}

#[test]
fn oracle_refuses_a_commit_carrying_paths_a_hook_staged() {
    let fixture = Fixture::lane();
    fixture.install_hook("pre-commit", "#!/bin/sh\ngit add -u\nexit 0\n");
    write(&fixture.path, "other.txt", "operator modified\n");
    write(&fixture.path, "spec.md", "# spec\n");

    let error = GitRepository::open(&fixture.path)
        .unwrap()
        .safe_commit(&fixture.request("diff: sweep", &["spec.md"]))
        .unwrap_err();

    // Oracle: SAFE_COMMIT_UNREQUESTED_PATHS, after the commit landed.
    match error {
        GitError::SafeCommitLanded {
            kind: SafeCommitLandedKind::UnrequestedPaths,
            ref recovery,
            ..
        } => {
            assert_eq!(
                recovery.commit().as_str(),
                git_text(&fixture.path, &["rev-parse", "HEAD"]).trim()
            );
            assert!(
                recovery
                    .affected_paths()
                    .iter()
                    .any(|path| path == &GitPath::from_git_bytes(b"other.txt").unwrap())
            );
        }
        other => panic!("unexpected error: {other:?}"),
    }
    assert_eq!(
        fixture.committed_paths(),
        strings(ORACLE_HOOK_SWEEP_COMMITTED)
    );
    assert!(
        fixture.status().is_empty(),
        "status: {:?}",
        fixture.status()
    );
}
