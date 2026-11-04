//! Exact-byte commit-message capability.
//!
//! The pinned reference (`specify_cli/git/commit_helpers.py` at
//! `9aed3a8012cb5eb22c0128b14d525039f95fea7a`) hands a Python `str` to
//! `git commit --only -m`, so a `\udc80` code point becomes the single byte
//! `0x80` at the argv boundary and Git — not Spec Kitty — decides how that byte
//! is stored (`C2 80` under UTF-8, raw `0x80` under `i18n.commitEncoding`).
//!
//! These tests exercise the byte API against real Git and, in one child
//! process, against the live pin-locked oracle: identical repository copies are
//! committed natively through [`sk_git::SafeCommitTargetPlan::prepare`] and by
//! the reference's ordinary `safe_commit`, then compared object-for-object.
//! The pre-existing entry point's reflog/warning regression has its own file
//! (`safe_commit_reflog_raw.rs`).
//!
//! Every live probe runs in a fresh temporary root with isolated `HOME`,
//! `TMPDIR`, `XDG` and fixed identity/date metadata.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sk_git::{
    BranchName, CommitMessage, GitError, GitPath, GitRepository, SafeCommitPreflightStage,
    SafeCommitTargetRequest,
};
use tempfile::TempDir;

const LANE: &str = "kitty/mission-bytes-01ABCDEF";

struct Fixture {
    _temp: TempDir,
    path: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        Self::at(LANE)
    }

    fn at(branch: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("repo");
        fs::create_dir_all(&path).unwrap();
        git_ok(&path, &["init", "-q", "-b", branch]);
        git_ok(&path, &["config", "user.name", "Test Author"]);
        git_ok(&path, &["config", "user.email", "test@example.com"]);
        git_ok(&path, &["config", "commit.gpgsign", "false"]);
        git_ok(&path, &["config", "tag.gpgsign", "false"]);
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

    fn destination(&self) -> BranchName {
        BranchName::parse(&self.open().current_branch().unwrap().unwrap()).unwrap()
    }

    fn record(&self) -> GitPath {
        GitPath::from_git_bytes(b"kitty-ops/01ARZ3ND.jsonl").unwrap()
    }

    fn target(&self) -> SafeCommitTargetRequest {
        SafeCommitTargetRequest::new(self.destination(), [self.record()])
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

    fn commit_bytes(&self, sha: &str) -> Vec<u8> {
        self.git(&["cat-file", "commit", sha])
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

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

// ---------------------------------------------------------------------------
// Byte API against real Git
// ---------------------------------------------------------------------------

#[test]
fn raw_bytes_commit_through_the_message_free_target_plan() {
    for (bytes, expected_subject) in [
        (b"op(\x80): a\n".to_vec(), b"op(\xc2\x80): a".to_vec()),
        (b"op(\xff): a\n".to_vec(), b"op(\xc3\xbf): a".to_vec()),
    ] {
        let fixture = Fixture::new();
        let repository = fixture.open();
        let plan = repository
            .plan_safe_commit_target(&fixture.target())
            .expect("the target is plannable without a message");
        let prepared = plan
            .prepare(CommitMessage::from_bytes(bytes.clone()))
            .expect("the exact bytes validate");

        assert_eq!(prepared.message_bytes(), bytes.as_slice());
        assert!(prepared.message_utf8().is_none());

        let outcome = repository
            .apply_safe_message_commit(&prepared)
            .expect("the byte commit lands");
        let commit = fixture.commit_bytes(outcome.commit().as_str());
        assert_eq!(
            fixture.git(&["cat-file", "-t", outcome.commit().as_str()]),
            b"commit\n"
        );
        assert_eq!(
            fixture.git(&["rev-parse", "HEAD"]),
            format!("{}\n", outcome.commit()).into_bytes(),
            "the plan's commit is what HEAD names"
        );
        assert!(
            contains(&commit, &expected_subject) && contains(&commit, b"\n\nop("),
            "the authored subject survives Git's conversion: {:?}",
            String::from_utf8_lossy(&commit)
        );
        assert_eq!(
            prepared.requested_paths(),
            vec![fixture.record()],
            "the plan names exactly the requested path"
        );
        assert_eq!(
            outcome.committed_paths(),
            &[fixture.record()],
            "the commit carries exactly the requested path"
        );
        assert_eq!(
            last_reflog_message(&fixture.reflog()),
            [b"commit: ".to_vec(), bytes[..bytes.len() - 1].to_vec()].concat(),
            "the reflog keeps the exact pre-conversion message bytes"
        );
        assert!(
            outcome
                .warnings()
                .iter()
                .any(|warning| warning.contains("did not conform to UTF-8")),
            "Git's UTF-8 warning is surfaced: {:?}",
            outcome.warnings()
        );
        assert_eq!(fixture.status(), "");
    }
}

#[test]
fn git_encoding_configuration_owns_the_stored_bytes() {
    let fixture = Fixture::new();
    fixture.git(&["config", "i18n.commitEncoding", "ISO-8859-1"]);
    let repository = fixture.open();
    let prepared = repository
        .plan_safe_commit_target(&fixture.target())
        .unwrap()
        .prepare(CommitMessage::from_bytes(b"op(\x80): a\n"))
        .unwrap();
    let outcome = repository.apply_safe_message_commit(&prepared).unwrap();

    let commit = fixture.commit_bytes(outcome.commit().as_str());
    assert!(
        contains(&commit, b"encoding ISO-8859-1\n"),
        "Git records the configured encoding header: {:?}",
        String::from_utf8_lossy(&commit)
    );
    assert!(
        contains(&commit, b"\n\nop(\x80):"),
        "under the configured encoding the raw byte is stored as authored: {:?}",
        String::from_utf8_lossy(&commit)
    );
    assert!(
        outcome.warnings().is_empty(),
        "a conforming configured encoding produces no warning: {:?}",
        outcome.warnings()
    );
    assert_eq!(
        last_reflog_message(&fixture.reflog()),
        b"commit: op(\x80): a".to_vec()
    );
}

#[test]
fn valid_unicode_messages_keep_their_exact_bytes() {
    for (bytes, subject) in [
        ("op(\u{00a0}): a\n".as_bytes().to_vec(), "op(\u{00a0}): a"),
        ("op(\u{1f600}): a\n".as_bytes().to_vec(), "op(\u{1f600}): a"),
    ] {
        let fixture = Fixture::new();
        let repository = fixture.open();
        let prepared = repository
            .plan_safe_commit_target(&fixture.target())
            .unwrap()
            .prepare(CommitMessage::from_bytes(bytes.clone()))
            .unwrap();
        let outcome = repository.apply_safe_message_commit(&prepared).unwrap();
        assert_eq!(
            prepared.message_utf8(),
            Some(std::str::from_utf8(&bytes).unwrap())
        );
        let commit = fixture.commit_bytes(outcome.commit().as_str());
        assert!(
            contains(&commit, subject.as_bytes()),
            "the exact UTF-8 bytes are kept: {:?}",
            String::from_utf8_lossy(&commit)
        );
        assert!(
            outcome.warnings().is_empty(),
            "valid UTF-8 needs no warning: {:?}",
            outcome.warnings()
        );
        assert_eq!(
            last_reflog_message(&fixture.reflog()),
            [b"commit: ".to_vec(), bytes[..bytes.len() - 1].to_vec()].concat()
        );
    }
}

#[test]
fn an_empty_or_oversized_payload_is_refused_at_the_message_stage() {
    let fixture = Fixture::new();
    let repository = fixture.open();
    let plan = repository
        .plan_safe_commit_target(&fixture.target())
        .unwrap();

    let failure = plan
        .clone()
        .prepare(CommitMessage::from_bytes(Vec::new()))
        .unwrap_err();
    assert_eq!(failure.stage(), SafeCommitPreflightStage::Message);
    assert!(matches!(
        failure.error(),
        GitError::SafeCommitInvalidRequest { detail }
            if detail == "a safe commit needs a non-empty message"
    ));

    let failure = plan
        .clone()
        .prepare(CommitMessage::from_bytes(b"has\0nul"))
        .unwrap_err();
    assert_eq!(failure.stage(), SafeCommitPreflightStage::Message);
    assert!(matches!(
        failure.error(),
        GitError::SafeCommitInvalidRequest { detail }
            if detail == "the commit message must not contain NUL"
    ));

    let oversized = vec![b'x'; sk_git::MAX_COMMIT_MESSAGE_LEN + 1];
    let failure = plan
        .clone()
        .prepare(CommitMessage::from_bytes(oversized))
        .unwrap_err();
    assert_eq!(failure.stage(), SafeCommitPreflightStage::Message);
    assert!(matches!(
        failure.error(),
        GitError::SafeCommitInvalidRequest { detail }
            if *detail
                == format!(
                    "the commit message exceeds the {}-byte limit",
                    sk_git::MAX_COMMIT_MESSAGE_LEN
                )
    ));

    let boundary = plan
        .prepare(CommitMessage::from_bytes(vec![
            b'x';
            sk_git::MAX_COMMIT_MESSAGE_LEN
        ]))
        .expect("exactly the bound is accepted");
    assert_eq!(
        boundary.message_bytes().len(),
        sk_git::MAX_COMMIT_MESSAGE_LEN
    );
}

#[test]
fn a_hook_rewrite_keeps_raw_bytes_and_surfaces_the_warning() {
    let fixture = Fixture::new();
    fixture.install_commit_msg_hook(b"op(\\377): rewritten\\n\\nbody\\n");
    let repository = fixture.open();
    let prepared = repository
        .plan_safe_commit_target(&fixture.target())
        .unwrap()
        .prepare(CommitMessage::from_bytes(b"op(\xc3\xbf): a\n"))
        .unwrap();
    let outcome = repository.apply_safe_message_commit(&prepared).unwrap();

    let commit = fixture.commit_bytes(outcome.commit().as_str());
    assert!(
        commit.ends_with(b"\n\nop(\xc3\xbf): rewritten\n\nbody\n"),
        "{:?}",
        String::from_utf8_lossy(&commit)
    );
    assert!(
        outcome
            .warnings()
            .iter()
            .any(|warning| warning.contains("did not conform to UTF-8")),
        "{:?}",
        outcome.warnings()
    );
    assert_eq!(
        last_reflog_message(&fixture.reflog()),
        b"commit: op(\xff): rewritten".to_vec(),
        "the hook's exact first-line bytes reach the reflog"
    );
}

#[test]
fn byte_pinned_parent_preserves_verbatim_payload_and_exact_blob_assertions() {
    let fixture = Fixture::new();
    fixture.git(&["config", "i18n.commitEncoding", "ISO-8859-1"]);
    let repository = fixture.open();
    let parent = repository
        .read_branch_tip(&fixture.destination())
        .unwrap()
        .unwrap();
    let bytes = fs::read(fixture.path.join("kitty-ops/01ARZ3ND.jsonl")).unwrap();
    let target = fixture
        .target()
        .with_expected_parent(parent.clone())
        .with_expected_bytes([sk_git::ExpectedBlob::new(fixture.record(), bytes.clone())]);
    let prepared = repository
        .plan_safe_commit_target(&target)
        .unwrap()
        .prepare(CommitMessage::from_bytes(b"op(\x80): pinned\n"))
        .unwrap();
    assert_eq!(prepared.route(), sk_git::SafeCommitRoute::PinnedParent);
    let outcome = repository.apply_safe_message_commit(&prepared).unwrap();
    let object = fixture.commit_bytes(outcome.commit().as_str());
    assert!(contains(&object, format!("parent {parent}\n").as_bytes()));
    assert!(
        object.ends_with(b"\n\nop(\x80): pinned\n\n"),
        "pinned route retains its always-append-LF rule"
    );
    assert!(contains(&object, b"encoding ISO-8859-1\n"));
    assert_eq!(outcome.committed_paths(), &[fixture.record()]);
    assert_eq!(
        fs::read(fixture.path.join("kitty-ops/01ARZ3ND.jsonl")).unwrap(),
        bytes
    );
    assert_eq!(last_reflog_message(&fixture.reflog()), b"op(\x80): pinned");
}

#[test]
fn byte_index_deletion_keeps_the_worktree_file() {
    let fixture = Fixture::new();
    let repository = fixture.open();
    let seed = GitPath::from_git_bytes(b"seed.txt").unwrap();
    let target = fixture.target().with_index_deletions([seed.clone()]);
    let prepared = repository
        .plan_safe_commit_target(&target)
        .unwrap()
        .prepare(CommitMessage::from_bytes(b"op(\x80): remove index entry"))
        .unwrap();
    assert_eq!(prepared.route(), sk_git::SafeCommitRoute::IndexDeletions);
    let outcome = repository.apply_safe_message_commit(&prepared).unwrap();
    assert_eq!(outcome.committed_paths(), &[fixture.record(), seed]);
    assert_eq!(fs::read(fixture.path.join("seed.txt")).unwrap(), b"seed\n");
    assert!(
        fixture
            .git(&["ls-tree", "HEAD", "--", "seed.txt"])
            .is_empty()
    );
    assert_eq!(fixture.git(&["status", "--porcelain=v1"]), b"?? seed.txt\n");
}

#[test]
fn byte_pinned_route_honors_signing_while_ordinary_route_stays_unsigned() {
    let fixture = Fixture::new();
    fixture.git(&["config", "commit.gpgsign", "true"]);
    let signer = fixture.path.join("missing-signer");
    fixture.git(&["config", "gpg.program", signer.to_str().unwrap()]);
    let repository = fixture.open();
    let parent = repository
        .read_branch_tip(&fixture.destination())
        .unwrap()
        .unwrap();
    let before_index = fs::read(fixture.path.join(".git/index")).unwrap();
    let prepared = repository
        .plan_safe_commit_target(&fixture.target().with_expected_parent(parent.clone()))
        .unwrap()
        .prepare(CommitMessage::from_bytes(b"op(\x80): signed"))
        .unwrap();
    let failure = repository.apply_safe_message_commit(&prepared).unwrap_err();
    assert!(
        matches!(failure, GitError::SafeCommitCommitFailed { .. }),
        "{failure:?}"
    );
    assert_eq!(
        repository.read_branch_tip(&fixture.destination()).unwrap(),
        Some(parent)
    );
    assert_eq!(
        fs::read(fixture.path.join(".git/index")).unwrap(),
        before_index
    );
    let ordinary = repository
        .plan_safe_commit_target(&fixture.target())
        .unwrap()
        .prepare(CommitMessage::from_bytes(b"op(\x80): unsigned"))
        .unwrap();
    let outcome = repository.apply_safe_message_commit(&ordinary).unwrap();
    assert!(!contains(
        &fixture.commit_bytes(outcome.commit().as_str()),
        b"\ngpgsig "
    ));
    assert_eq!(outcome.committed_paths(), &[fixture.record()]);
}

#[test]
fn unrelated_staging_is_preserved_by_the_byte_route() {
    let fixture = Fixture::new();
    fs::write(fixture.path.join("seed.txt"), b"changed\n").unwrap();
    fixture.git(&["add", "seed.txt"]);
    fs::write(fixture.path.join("seed.txt"), b"changed again\n").unwrap();

    let repository = fixture.open();
    let prepared = repository
        .plan_safe_commit_target(&fixture.target())
        .unwrap()
        .prepare(CommitMessage::from_bytes(b"op(\x80): a\n"))
        .unwrap();
    let outcome = repository.apply_safe_message_commit(&prepared).unwrap();

    assert_eq!(outcome.committed_paths(), &[fixture.record()]);
    let staged_raw = fixture.git(&["diff", "--cached", "--name-only"]);
    let staged = String::from_utf8_lossy(&staged_raw);
    assert_eq!(
        staged, "seed.txt\n",
        "the unrelated staged entry stays staged and out of the commit"
    );
}

#[test]
fn a_prepared_plan_from_another_checkout_is_refused() {
    let fixture = Fixture::new();
    let other = Fixture::at("other-lane");
    let repository = fixture.open();
    let prepared = repository
        .plan_safe_commit_target(&fixture.target())
        .unwrap()
        .prepare(CommitMessage::from_bytes(b"op(\x80): a\n"))
        .unwrap();
    let before = other.git(&["rev-parse", "HEAD"]);
    let error = other
        .open()
        .apply_safe_message_commit(&prepared)
        .unwrap_err();
    assert!(
        matches!(error, GitError::InvalidPlan { .. }),
        "a plan is bound to its checkout: {error:?}"
    );
    assert_eq!(other.git(&["rev-parse", "HEAD"]), before);
}

// ---------------------------------------------------------------------------
// Live pinned-source parity (exact-byte capability, ordinary route)
// ---------------------------------------------------------------------------

#[test]
fn byte_commits_match_the_live_pinned_source_object_for_object() {
    let root = tempfile::tempdir().unwrap();
    for directory in [
        "home",
        "xdg-config",
        "xdg-cache",
        "xdg-data",
        "xdg-state",
        "tmp",
    ] {
        fs::create_dir_all(root.path().join(directory)).unwrap();
    }
    let child = std::env::current_exe().unwrap();
    let output = Command::new(child)
        .args([
            "--ignored",
            "--exact",
            "oracle_byte_case_helper",
            "--nocapture",
            "--",
        ])
        .arg(root.path())
        .current_dir(root.path())
        .env_clear()
        .env(
            "PATH",
            std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
        )
        .env("HOME", root.path().join("home"))
        .env("XDG_CONFIG_HOME", root.path().join("xdg-config"))
        .env("XDG_CACHE_HOME", root.path().join("xdg-cache"))
        .env("XDG_DATA_HOME", root.path().join("xdg-data"))
        .env("XDG_STATE_HOME", root.path().join("xdg-state"))
        .env("TMPDIR", root.path().join("tmp"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_DATE", "2020-01-01T00:00:00+0000")
        .env("GIT_COMMITTER_DATE", "2020-01-01T00:00:00+0000")
        .env("TZ", "UTC")
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("RUST_BACKTRACE", "1")
        .output()
        .expect("spawn the oracle byte helper");
    assert!(
        output.status.success(),
        "live pinned-source byte comparison failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Subprocess helper: native byte commit and live pinned-source commit on
/// identical repository copies, compared object-for-object.
///
/// The Python message is passed as hex bytes and reconstructed with
/// `os.fsdecode`, the exact inverse of the interruptible `os.fsencode` boundary
/// the reference applies to its own `str` — a `\udc80..\udcff` code point
/// therefore reaches `git commit -m` as the single byte the native route
/// carries.
#[test]
#[ignore = "subprocess helper for byte_commits_match_the_live_pinned_source_object_for_object"]
fn oracle_byte_case_helper() {
    let args: Vec<String> = std::env::args().collect();
    let separator = args
        .iter()
        .position(|arg| arg == "--")
        .expect("helper needs `-- <root>`");
    let root = PathBuf::from(&args[separator + 1]);
    assert_eq!(
        std::env::current_dir().unwrap(),
        root,
        "oracle helper must run from its isolated root, not the source checkout"
    );

    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo_root = crate_root.parent().unwrap().parent().unwrap();
    let python = repo_root.join(".venv-rust-oracle/bin/python");
    assert!(
        python.is_file(),
        "the lock-exact oracle interpreter is missing at {}",
        python.display()
    );
    let python_env = |command: &mut Command| {
        command
            .env("PYTHONPATH", repo_root.join("src"))
            .env("SPEC_KITTY_PACKS_ROOT", repo_root.join("packs"));
    };

    // Prove the pinned oracle closure before comparing anything.
    let mut verify = Command::new(&python);
    verify
        .arg(repo_root.join("scripts/rust/verify_oracle.py"))
        .arg("--root")
        .arg(repo_root);
    python_env(&mut verify);
    let verify = verify.output().expect("run verify_oracle.py");
    assert!(
        verify.status.success(),
        "verify_oracle.py failed: {}",
        String::from_utf8_lossy(&verify.stderr)
    );
    let verification = String::from_utf8_lossy(&verify.stdout);
    assert!(
        verification.contains("\"ok\": true"),
        "oracle closure is not verified: {verification}"
    );
    assert!(
        verification.contains("\"drift\": []") && verification.contains("\"missing\": []"),
        "oracle drift or missing requirements: {verification}"
    );

    // (name, exact message bytes for both routes, configured encoding)
    let cases: &[(&str, &[u8], Option<&str>)] = &[
        ("raw80", b"op(\x80): a\n", None),
        ("rawff", b"op(\xff): a\n", None),
        ("nbspan", "op(\u{00a0}): a\n".as_bytes(), None),
        ("astral", "op(\u{1f600}): a\n".as_bytes(), None),
        ("raw80_latin1", b"op(\x80): a\n", Some("ISO-8859-1")),
        ("rawff_latin1", b"op(\xff): a\n", Some("ISO-8859-1")),
        ("hook_rewrite80", b"op(\x80): a\n", None),
    ];

    for (name, bytes, encoding) in cases {
        let base = root.join(name);
        fs::create_dir_all(&base).unwrap();
        let native_repo = base.join("native");
        fs::create_dir_all(&native_repo).unwrap();
        run_git(&native_repo, &["init", "-q", "-b", LANE]);
        for (key, value) in [
            ("user.name", "Test Author"),
            ("user.email", "test@example.com"),
            ("commit.gpgsign", "false"),
        ] {
            run_git(&native_repo, &["config", key, value]);
        }
        if let Some(encoding) = encoding {
            run_git(&native_repo, &["config", "i18n.commitEncoding", encoding]);
        }
        fs::write(native_repo.join("seed.txt"), b"seed\n").unwrap();
        run_git(&native_repo, &["add", "-A"]);
        run_git(&native_repo, &["commit", "-q", "-m", "initial"]);
        fs::create_dir_all(native_repo.join("kitty-ops")).unwrap();
        fs::write(native_repo.join("kitty-ops/01ARZ3ND.jsonl"), b"{\"a\":1}\n").unwrap();
        if *name == "hook_rewrite80" {
            let hooks = native_repo.join(".git/hooks");
            fs::create_dir_all(&hooks).unwrap();
            let hook = hooks.join("commit-msg");
            fs::write(
                &hook,
                b"#!/bin/sh\nprintf 'op(\\200): rewritten\\n\\nbody\\n' > \"$1\"\n",
            )
            .unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
            }
        }

        let source_repo = base.join("source");
        copy_tree(&native_repo, &source_repo);

        // Native byte route.
        let repository = GitRepository::open(&native_repo).unwrap();
        let destination =
            BranchName::parse(&repository.current_branch().unwrap().unwrap()).unwrap();
        let record = GitPath::from_git_bytes(b"kitty-ops/01ARZ3ND.jsonl").unwrap();
        let target = SafeCommitTargetRequest::new(destination, [record]);
        let prepared = repository
            .plan_safe_commit_target(&target)
            .unwrap_or_else(|failure| panic!("{name}: target plan failed: {failure:?}"))
            .prepare(CommitMessage::from_bytes(bytes.to_vec()))
            .unwrap_or_else(|failure| panic!("{name}: prepare failed: {failure:?}"));
        let native = repository
            .apply_safe_message_commit(&prepared)
            .unwrap_or_else(|error| panic!("{name}: native byte apply failed: {error:?}"));
        let native_oid = native.commit().as_str().to_string();

        // Live pinned source, ordinary route, from the exact fsencode bytes.
        let script = root.join("source_commit.py");
        fs::write(
            &script,
            r#"
import json, os, sys
from pathlib import Path
root = Path(sys.argv[1])
message = os.fsdecode(bytes.fromhex(sys.argv[2]))
ref = sys.argv[3]
from mission_runtime import CommitTarget
from specify_cli.core.commit_guard import GuardCapability
from specify_cli.git.commit_helpers import safe_commit
result = safe_commit(
    repo_root=root,
    worktree_root=root,
    target=CommitTarget(ref=ref),
    message=message,
    paths=(root / "kitty-ops" / "01ARZ3ND.jsonl",),
    capability=GuardCapability.STANDARD,
)
print("OID=" + result.sha)
"#,
        )
        .unwrap();
        let mut source = Command::new(&python);
        source
            .arg(&script)
            .arg(&source_repo)
            .arg(hex(bytes))
            .arg(LANE);
        python_env(&mut source);
        let run = source.output().expect("run the pinned source commit");
        assert!(
            run.status.success(),
            "{name}: pinned source refused: {}\n{}",
            String::from_utf8_lossy(&run.stderr),
            String::from_utf8_lossy(&run.stdout)
        );
        let payload = String::from_utf8_lossy(&run.stdout);
        let source_oid = payload
            .lines()
            .find_map(|line| line.strip_prefix("OID="))
            .unwrap_or_else(|| panic!("{name}: cannot read the source oid from {payload}"))
            .to_string();

        assert_eq!(
            native_oid, source_oid,
            "{name}: the native byte commit and the reference commit are different objects"
        );
        let native_object = run_git_capture(&native_repo, &["cat-file", "commit", &native_oid]);
        let source_object = run_git_capture(&source_repo, &["cat-file", "commit", &source_oid]);
        assert_eq!(
            native_object, source_object,
            "{name}: full commit object bytes differ"
        );
        assert_eq!(
            last_reflog_message(&read_reflog(&native_repo)),
            last_reflog_message(&read_reflog(&source_repo)),
            "{name}: reflog message bytes differ (objects were equal)"
        );
        let native_status = run_git_capture(&native_repo, &["status", "--porcelain=v1"]);
        let source_status = run_git_capture(&source_repo, &["status", "--porcelain=v1"]);
        assert_eq!(
            native_status, source_status,
            "{name}: index/worktree state differs"
        );
        let native_paths = run_git_capture(
            &native_repo,
            &["show", "--name-only", "--format=", &native_oid],
        );
        let source_paths = run_git_capture(
            &source_repo,
            &["show", "--name-only", "--format=", &source_oid],
        );
        assert_eq!(native_paths, source_paths, "{name}: committed paths differ");
        assert_eq!(
            native_paths, b"kitty-ops/01ARZ3ND.jsonl\n",
            "{name}: the reference commits exactly the requested path"
        );
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

fn read_reflog(repo: &Path) -> Vec<u8> {
    fs::read(repo.join(".git/logs/refs/heads").join(LANE)).unwrap_or_default()
}

fn run_git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed in {}: {}",
        cwd.display(),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_git_capture(cwd: &Path, args: &[&str]) -> Vec<u8> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed in {}: {}",
        cwd.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = fs::metadata(entry.path()).unwrap().permissions().mode();
                fs::set_permissions(&target, fs::Permissions::from_mode(mode)).unwrap();
            }
        }
    }
}

#[test]
fn a_stale_checkout_refuses_the_byte_plan_without_writing() {
    let fixture = Fixture::new();
    let repository = fixture.open();
    let prepared = repository
        .plan_safe_commit_target(&fixture.target())
        .unwrap()
        .prepare(CommitMessage::from_bytes(b"op(\x80): a\n"))
        .unwrap();

    // Another writer moves the destination after the plan was prepared.
    fs::write(fixture.path.join("seed.txt"), b"moved\n").unwrap();
    fixture.git(&["commit", "-q", "-am", "concurrent"]);
    let before = fixture.git(&["rev-parse", "HEAD"]);

    let error = repository
        .apply_safe_message_commit(&prepared)
        .expect_err("a stale plan is refused before mutation");
    assert!(
        matches!(error, GitError::PlanStale { .. }),
        "the shared apply recheck refuses a moved checkout: {error:?}"
    );
    assert_eq!(fixture.git(&["rev-parse", "HEAD"]), before);
}

#[test]
fn the_byte_route_honours_the_captured_alternate_index() {
    let root = tempfile::tempdir().unwrap();
    let child = std::env::current_exe().unwrap();
    let output = Command::new(child)
        .args([
            "--ignored",
            "--exact",
            "byte_alternate_index_child",
            "--nocapture",
            "--",
        ])
        .arg(root.path())
        .current_dir(root.path())
        .env_clear()
        .env(
            "PATH",
            std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
        )
        .env("RUST_BACKTRACE", "1")
        .output()
        .expect("spawn the alternate-index helper");
    assert!(
        output.status.success(),
        "captured alternate-index byte commit failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Subprocess helper: prepare the repository, then hand it to a worker whose
/// process environment carries the captured `GIT_INDEX_FILE`.
#[test]
#[ignore = "subprocess helper for the_byte_route_honours_the_captured_alternate_index"]
fn byte_alternate_index_child() {
    let args: Vec<String> = std::env::args().collect();
    let separator = args
        .iter()
        .position(|arg| arg == "--")
        .expect("helper needs `-- <root>`");
    let root = PathBuf::from(&args[separator + 1]);
    assert_eq!(
        std::env::current_dir().unwrap(),
        root,
        "alternate-index helper must run from its isolated root"
    );
    let repo = root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    for directory in ["home", "config", "cache", "data", "state", "tmp"] {
        fs::create_dir_all(root.join(directory)).unwrap();
    }
    let mut init = Command::new("git");
    init.args(["init", "-q", "-b", LANE]).current_dir(&repo);
    isolate(&root, &mut init);
    init.env_remove("GIT_INDEX_FILE");
    assert!(init.output().unwrap().status.success());
    for (key, value) in [
        ("user.name", "Test Author"),
        ("user.email", "test@example.com"),
        ("commit.gpgsign", "false"),
    ] {
        let mut config = Command::new("git");
        config.args(["config", key, value]).current_dir(&repo);
        isolate(&root, &mut config);
        config.env_remove("GIT_INDEX_FILE");
        assert!(config.output().unwrap().status.success());
    }
    fs::write(repo.join("seed.txt"), b"seed\n").unwrap();
    for args in [vec!["add", "-A"], vec!["commit", "-q", "-m", "initial"]] {
        let mut command = Command::new("git");
        command.args(&args).current_dir(&repo);
        isolate(&root, &mut command);
        command.env_remove("GIT_INDEX_FILE");
        assert!(command.output().unwrap().status.success());
    }
    fs::create_dir_all(repo.join("kitty-ops")).unwrap();
    fs::write(repo.join("kitty-ops/01ARZ3ND.jsonl"), b"{\"a\":1}\n").unwrap();
    let alternate = root.join("custom-index");
    fs::copy(repo.join(".git/index"), &alternate).unwrap();
    fs::write(
        root.join("default-index-before"),
        fs::read(repo.join(".git/index")).unwrap(),
    )
    .unwrap();

    let mut worker = Command::new(std::env::current_exe().unwrap());
    worker
        .args([
            "--ignored",
            "--exact",
            "byte_alternate_index_worker",
            "--nocapture",
            "--",
        ])
        .arg(&root)
        .current_dir(&repo)
        .env_clear()
        .env(
            "PATH",
            std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
        )
        .env("RUST_BACKTRACE", "1")
        .env("GIT_INDEX_FILE", &alternate);
    isolate(&root, &mut worker);
    let output = worker.output().expect("spawn the alternate-index worker");
    assert!(
        output.status.success(),
        "captured alternate-index byte commit failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The worker: every git child inherits the captured `GIT_INDEX_FILE`.
#[test]
#[ignore = "subprocess helper for the_byte_route_honours_the_captured_alternate_index"]
fn byte_alternate_index_worker() {
    let args: Vec<String> = std::env::args().collect();
    let separator = args
        .iter()
        .position(|arg| arg == "--")
        .expect("worker needs `-- <root>`");
    let root = PathBuf::from(&args[separator + 1]);
    let repo = root.join("repo");
    assert_eq!(std::env::current_dir().unwrap(), repo);
    let default_index_before = fs::read(root.join("default-index-before")).unwrap();

    let repository = GitRepository::open(&repo).unwrap();
    assert!(
        repository.operator_index().is_captured(),
        "the alternate index is captured at open"
    );
    let destination = BranchName::parse(&repository.current_branch().unwrap().unwrap()).unwrap();
    let record = GitPath::from_git_bytes(b"kitty-ops/01ARZ3ND.jsonl").unwrap();
    let target = SafeCommitTargetRequest::new(destination, [record]);
    let prepared = repository
        .plan_safe_commit_target(&target)
        .unwrap()
        .prepare(CommitMessage::from_bytes(b"op(\x80): a\n"))
        .unwrap();
    let outcome = repository.apply_safe_message_commit(&prepared).unwrap();

    let mut staged = Command::new("git");
    staged
        .args(["diff", "--cached", "--name-only"])
        .current_dir(&repo);
    isolate(&root, &mut staged);
    let staged = staged.output().unwrap();
    assert!(staged.status.success());
    assert!(
        staged.stdout.is_empty(),
        "the captured index is reconciled: {}",
        String::from_utf8_lossy(&staged.stdout)
    );
    let mut ls = Command::new("git");
    ls.args(["ls-files", "--stage"]).current_dir(&repo);
    isolate(&root, &mut ls);
    let listed = String::from_utf8_lossy(&ls.output().unwrap().stdout).into_owned();
    assert!(
        listed.contains("kitty-ops/01ARZ3ND.jsonl"),
        "the commit's index entry lives in the captured index: {listed}"
    );
    assert_eq!(
        fs::read(repo.join(".git/index")).unwrap(),
        default_index_before,
        "the default index is never touched while a captured index is in force"
    );

    let mut head = Command::new("git");
    head.args(["rev-parse", "HEAD"]).current_dir(&repo);
    isolate(&root, &mut head);
    assert_eq!(
        String::from_utf8_lossy(&head.output().unwrap().stdout).trim(),
        outcome.commit().as_str(),
        "the alternate-index commit is the destination tip"
    );
}

fn isolate(root: &Path, command: &mut Command) {
    command
        .env("HOME", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("TMPDIR", root.join("tmp"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_DATE", "2020-01-01T00:00:00+0000")
        .env("GIT_COMMITTER_DATE", "2020-01-01T00:00:00+0000")
        .env("TZ", "UTC")
        .env("LC_ALL", "C")
        .env("LANG", "C");
}
