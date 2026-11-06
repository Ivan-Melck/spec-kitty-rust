//! Bounded, prompt-free remote `HEAD` resolution.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sk_git::{GitError, GitRepository, RemoteHeadTarget, RemoteName};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("work");
        for directory in ["", "home", "config", "tmp"] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        let fixture = Self { _temp: temp, root };
        fixture.git(&["init", "-q", "-b", "main"]);
        fixture.git(&["config", "user.name", "Test"]);
        fixture.git(&["config", "user.email", "test@example.invalid"]);
        fixture.git(&["config", "commit.gpgsign", "false"]);
        fs::write(fixture.root.join("seed.txt"), b"seed\n").unwrap();
        fixture.git(&["add", "-A"]);
        fixture.git(&["commit", "-qm", "initial"]);
        fixture
    }

    fn git(&self, args: &[&str]) -> Vec<u8> {
        git_in(&self.root, args)
    }

    fn head(&self) -> String {
        String::from_utf8_lossy(&self.git(&["rev-parse", "HEAD"]))
            .trim()
            .to_string()
    }

    fn open(&self) -> GitRepository {
        GitRepository::open(&self.root).unwrap()
    }

    /// A bare remote with `HEAD -> refs/heads/<branch>`, plus a pushed commit.
    fn bare_remote(&self, name: &str, branch: &str) -> PathBuf {
        let path = self.root.parent().unwrap().join(format!("{name}.git"));
        git_in(
            self.root.parent().unwrap(),
            &["init", "--bare", "-q", "-b", branch, path.to_str().unwrap()],
        );
        self.git(&["remote", "add", name, path.to_str().unwrap()]);
        self.git(&["push", "-q", name, &format!("HEAD:{branch}")]);
        path
    }

    fn empty_bare_remote(&self, name: &str, branch: &str) -> PathBuf {
        let path = self.root.parent().unwrap().join(format!("{name}.git"));
        git_in(
            self.root.parent().unwrap(),
            &["init", "--bare", "-q", "-b", branch, path.to_str().unwrap()],
        );
        self.git(&["remote", "add", name, path.to_str().unwrap()]);
        path
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
        .env("GIT_TERMINAL_PROMPT", "0")
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

fn remote(name: &str) -> RemoteName {
    RemoteName::parse(name).unwrap()
}

#[test]
fn resolves_a_symbolic_remote_head_to_its_branch_and_tip() {
    let fixture = Fixture::new();
    fixture.bare_remote("origin", "trunk");
    let repository = fixture.open();
    let head = repository.resolve_remote_head(&remote("origin")).unwrap();
    assert_eq!(head.remote().as_str(), "origin");
    match head.target() {
        RemoteHeadTarget::Branch { branch, oid } => {
            assert_eq!(branch.as_str(), "trunk");
            assert_eq!(oid.as_str(), fixture.head());
        }
        other => panic!("expected a symbolic branch, got {other:?}"),
    }
    assert_eq!(
        repository
            .remote_default_branch(&remote("origin"))
            .unwrap()
            .map(|branch| branch.as_str().to_string()),
        Some("trunk".to_string())
    );
}

#[test]
fn a_detached_remote_head_reports_its_oid_only() {
    let fixture = Fixture::new();
    let bare = fixture.bare_remote("detached", "trunk");
    // A bare remote can hold a detached HEAD; git refuses `symbolic-ref
    // --delete HEAD`, so write the detached value directly.
    fs::write(bare.join("HEAD"), format!("{}\n", fixture.head())).unwrap();
    let repository = fixture.open();
    let head = repository.resolve_remote_head(&remote("detached")).unwrap();
    match head.target() {
        RemoteHeadTarget::Detached { oid } => assert_eq!(oid.as_str(), fixture.head()),
        other => panic!("expected a detached head, got {other:?}"),
    }
    assert_eq!(
        repository
            .remote_default_branch(&remote("detached"))
            .unwrap(),
        None
    );
}

#[test]
fn an_empty_remote_reports_an_unborn_head_not_an_error() {
    let fixture = Fixture::new();
    fixture.empty_bare_remote("empty", "main");
    let repository = fixture.open();
    let head = repository.resolve_remote_head(&remote("empty")).unwrap();
    assert_eq!(head.target(), &RemoteHeadTarget::Unborn);
    assert_eq!(head.target().oid(), None);
    assert_eq!(
        repository.remote_default_branch(&remote("empty")).unwrap(),
        None
    );
}

#[test]
fn an_unreachable_remote_is_a_typed_refusal_not_an_absence() {
    let fixture = Fixture::new();
    let missing = fixture.root.parent().unwrap().join("not-a-repository");
    fixture.git(&["remote", "add", "broken", missing.to_str().unwrap()]);
    let repository = fixture.open();
    let error = repository
        .resolve_remote_head(&remote("broken"))
        .unwrap_err();
    match error {
        GitError::RemoteUnreachable {
            remote,
            timed_out,
            detail,
            ..
        } => {
            assert_eq!(remote, "broken");
            assert!(!timed_out);
            assert!(!detail.is_empty());
        }
        other => panic!("expected RemoteUnreachable, got {other:?}"),
    }
    assert_eq!(
        repository
            .remote_default_branch(&remote("broken"))
            .unwrap_err()
            .to_string(),
        repository
            .resolve_remote_head(&remote("broken"))
            .unwrap_err()
            .to_string()
    );
}

#[test]
fn remote_names_stay_plain_names() {
    for name in ["origin", "upstream-1"] {
        assert!(RemoteName::parse(name).is_ok());
    }
    for name in ["https://example.com/x.git", "host:path", "-origin", "a/b"] {
        assert!(
            matches!(
                RemoteName::parse(name),
                Err(GitError::InvalidRemoteName { .. })
            ),
            "{name}"
        );
    }
}
