//! Repository selectors other than the captured operator index are refusals.
//!
//! `GIT_INDEX_FILE` is no longer in this set: it is captured at repository open
//! and honoured by the whole writer (see `tests/index_authority.rs`). Every
//! other repository-redirecting variable still refuses, because no shared
//! repository authority owns both lock discovery and the opened adapter yet.
//! A refusal must not retarget either the real index or the custom one.

use sk_git::{BranchName, GitError, GitPath, GitRepository, SafeCommitRequest};
use std::{fs, path::Path, process::Command};

const SELECTORS: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
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

fn isolated(root: &Path, program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    command
        .current_dir(root)
        .env("HOME", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("TMPDIR", root);
    for variable in SELECTORS {
        command.env_remove(variable);
    }
    command.env_remove("GIT_INDEX_FILE");
    command
}

fn git(root: &Path, args: &[&str]) {
    let output = isolated(root, "git").args(args).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn inherited_repository_selectors_refuse_without_retargeting_either_index() {
    let sandbox = tempfile::tempdir().unwrap();
    let root = sandbox.path();
    git(root, &["init", "-q", "-b", "work"]);
    git(root, &["config", "user.name", "Fixture"]);
    git(root, &["config", "user.email", "fixture@example.invalid"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    fs::write(root.join("a.txt"), "old\n").unwrap();
    git(root, &["add", "a.txt"]);
    git(root, &["commit", "-qm", "initial"]);
    fs::write(root.join("a.txt"), "new\n").unwrap();
    let index = fs::read(root.join(".git/index")).unwrap();
    let custom = root.join("custom-index");
    fs::write(&custom, &index).unwrap();
    let head = fs::read(root.join(".git/refs/heads/work")).unwrap();
    for variable in SELECTORS {
        let output = isolated(root, std::env::current_exe().unwrap())
            .args([
                "--exact",
                "redirected_safe_commit_child",
                "--ignored",
                "--nocapture",
            ])
            .env("SK_SAFE_AUTH_ROOT", root)
            .env("SK_SAFE_AUTH_VARIABLE", variable)
            .env(variable, &custom)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{variable}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
        assert_eq!(fs::read(&custom).unwrap(), index);
        assert_eq!(fs::read(root.join(".git/refs/heads/work")).unwrap(), head);
    }
}

#[test]
#[ignore = "subprocess helper; the parent covers every retained selector"]
fn redirected_safe_commit_child() {
    let root = std::env::var_os("SK_SAFE_AUTH_ROOT").expect("parent root");
    let variable = std::env::var("SK_SAFE_AUTH_VARIABLE").unwrap();
    let repo = GitRepository::open(root).unwrap();
    let request = SafeCommitRequest::new(
        BranchName::parse("work").unwrap(),
        "write",
        [GitPath::from_git_bytes(b"a.txt").unwrap()],
    );
    match repo.safe_commit(&request).unwrap_err() {
        GitError::RepositoryEnvironmentRefused { variable: actual } => assert_eq!(actual, variable),
        error => panic!("wrong refusal: {error:?}"),
    }
}
