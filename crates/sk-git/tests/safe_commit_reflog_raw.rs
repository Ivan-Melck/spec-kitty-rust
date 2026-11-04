//! Behavioural red for a `commit-msg` hook that writes non-UTF-8 bytes.
//!
//! This test drives the **pre-existing** `GitRepository::safe_commit` entry
//! point, so it compiled and failed before the byte-message capability existed.
//! It stays as the regression control for the two defects it measured:
//!
//! * `git commit-tree`'s warning on an otherwise successful commit was dropped,
//!   although the reference logs exactly that warning; and
//! * the branch reflog subject was rendered with `String::from_utf8_lossy`, so a
//!   raw `0x80` in the hook-rewritten message became `U+FFFD`, while the
//!   reference records the exact pre-conversion bytes.
//!
//! Measured reference facts (live pinned source, retained probe
//! `/tmp/sk-byte-srcprobe.Dgtfsl`): `git commit` stores `C2 80` in the commit
//! object, warns `Warning: commit message did not conform to UTF-8.`, and
//! records `commit: op(\x80): rewritten` in the reflog.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sk_git::{BranchName, GitPath, GitRepository, SafeCommitRequest};
use tempfile::TempDir;

const LANE: &str = "kitty/mission-reflograw-01ABCDEF";

struct Fixture {
    _temp: TempDir,
    path: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("repo");
        fs::create_dir_all(&path).unwrap();
        git_ok(&path, &["init", "-q", "-b", LANE]);
        git_ok(&path, &["config", "user.name", "Test Author"]);
        git_ok(&path, &["config", "user.email", "test@example.com"]);
        git_ok(&path, &["config", "commit.gpgsign", "false"]);
        fs::write(path.join("seed.txt"), b"seed\n").unwrap();
        git_ok(&path, &["add", "-A"]);
        git_ok(&path, &["commit", "-q", "-m", "initial"]);
        fs::create_dir_all(path.join("kitty-ops")).unwrap();
        fs::write(path.join("kitty-ops/01ARZ3ND.jsonl"), b"{\"a\":1}\n").unwrap();
        Self { _temp: temp, path }
    }

    fn open(&self) -> GitRepository {
        GitRepository::open(&self.path).unwrap()
    }

    fn record(&self) -> GitPath {
        GitPath::from_git_bytes(b"kitty-ops/01ARZ3ND.jsonl").unwrap()
    }

    fn install_commit_msg_hook(&self, body: &[u8]) {
        let hooks = self.path.join(".git").join("hooks");
        fs::create_dir_all(&hooks).unwrap();
        let hook = hooks.join("commit-msg");
        let mut script = b"#!/bin/sh\nprintf '".to_vec();
        script.extend_from_slice(body);
        script.extend_from_slice(b"' > \"$1\"\n");
        fs::write(&hook, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    fn git(&self, args: &[&str]) -> Vec<u8> {
        git_ok(&self.path, args)
    }

    fn reflog(&self) -> Vec<u8> {
        fs::read(self.path.join(".git/logs/refs/heads").join(LANE)).unwrap_or_default()
    }

    fn status(&self) -> String {
        String::from_utf8_lossy(&self.git(&["status", "--porcelain=v1"])).into_owned()
    }
}

fn git_ok(cwd: &Path, args: &[&str]) -> Vec<u8> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("HOME", cwd.join("home"))
        .env("XDG_CONFIG_HOME", cwd.join("config"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn last_reflog_message(reflog: &[u8]) -> Vec<u8> {
    let line = reflog
        .split(|byte| *byte == b'\n')
        .rfind(|line| !line.is_empty())
        .expect("reflog has a recorded advance");
    let tab = line
        .iter()
        .position(|byte| *byte == b'\t')
        .expect("a reflog record separates the message with a tab");
    line[tab + 1..].to_vec()
}

#[test]
fn record_reflog_and_success_warning_are_not_lossy() {
    let fixture = Fixture::new();
    fixture.install_commit_msg_hook(b"op(\\200): rewritten\\n\\nbody\\n");
    let repository = fixture.open();
    let destination = BranchName::parse(&repository.current_branch().unwrap().unwrap()).unwrap();
    let request = SafeCommitRequest::new(destination, "op(é): a", [fixture.record()]);
    let outcome = repository.safe_commit(&request).expect("the commit lands");

    let commit = fixture.git(&["cat-file", "commit", outcome.commit().as_str()]);
    assert!(
        commit.ends_with(b"\n\nop(\xc2\x80): rewritten\n\nbody\n"),
        "Git owns the UTF-8 conversion of the hook-rewritten message: {:?}",
        String::from_utf8_lossy(&commit)
    );
    assert!(
        outcome
            .warnings()
            .iter()
            .any(|warning| warning.contains("did not conform to UTF-8")),
        "a successful commit's stderr warning is operator evidence, not noise: {:?}",
        outcome.warnings()
    );
    assert_eq!(
        last_reflog_message(&fixture.reflog()),
        b"commit: op(\x80): rewritten".to_vec(),
        "the reference records the pre-conversion subject bytes, never U+FFFD"
    );
    assert_eq!(fixture.status(), "");
}
