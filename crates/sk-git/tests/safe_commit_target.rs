//! Message-free safe-commit target plans and their typed failure progress.
//!
//! `plan_safe_commit_target` and `plan_safe_commit` call **one** planner body:
//! the target entry omits the message stage, the legacy entry keeps
//! `Environment → Message → RequestShape → paths → HEAD → tip → expected
//! parent → guard → operation`. These tests assert that every shared failure
//! carries the same original [`GitError`] value in both entry points, and that
//! the legacy entry point still reports a bad message before a bad path.
//!
//! The typed stage names *only* the check that actually refused; it is never
//! parsed out of error text.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sk_git::{
    BranchName, CommitGuardCapability, CommitMessage, ExpectedBlob, GitError, GitPath,
    GitRepository, Oid, ProtectedBranches, SafeCommitPlanFailure, SafeCommitPreflightStage,
    SafeCommitRequest, SafeCommitRoute, SafeCommitTargetRequest,
};
use tempfile::TempDir;

const LANE: &str = "kitty/mission-target-01ABCDEF";

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
        configure(&path);
        fs::write(path.join("seed.txt"), b"seed\n").unwrap();
        fs::write(path.join("a.txt"), b"one\n").unwrap();
        git_ok(&path, &["add", "-A"]);
        git_ok(&path, &["commit", "-q", "-m", "initial"]);
        fs::write(path.join("a.txt"), b"two\n").unwrap();
        Self { _temp: temp, path }
    }

    fn unborn() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("repo");
        fs::create_dir_all(&path).unwrap();
        git_ok(&path, &["init", "-q", "-b", LANE]);
        configure(&path);
        fs::write(path.join("a.txt"), b"one\n").unwrap();
        Self { _temp: temp, path }
    }

    fn open(&self) -> GitRepository {
        GitRepository::open(&self.path).unwrap()
    }

    fn destination(&self) -> BranchName {
        BranchName::parse(&self.open().current_branch().unwrap().unwrap()).unwrap()
    }

    fn path(&self, bytes: &[u8]) -> GitPath {
        GitPath::from_git_bytes(bytes).unwrap()
    }

    fn target(&self, paths: &[&[u8]]) -> SafeCommitTargetRequest {
        SafeCommitTargetRequest::new(
            self.destination(),
            paths.iter().map(|path| self.path(path)).collect::<Vec<_>>(),
        )
    }

    fn request(&self, message: &str, paths: &[&[u8]]) -> SafeCommitRequest {
        SafeCommitRequest::new(
            self.destination(),
            message,
            paths.iter().map(|path| self.path(path)).collect::<Vec<_>>(),
        )
    }

    fn git(&self, args: &[&str]) -> Vec<u8> {
        git_ok(&self.path, args)
    }
}

fn configure(path: &Path) {
    for (key, value) in [
        ("user.name", "Test Author"),
        ("user.email", "test@example.com"),
        ("commit.gpgsign", "false"),
    ] {
        git_ok(path, &["config", key, value]);
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

fn failure(result: Result<impl std::fmt::Debug, SafeCommitPlanFailure>) -> SafeCommitPlanFailure {
    result.expect_err("the plan must be refused")
}

#[test]
fn the_target_plan_needs_no_message_and_prepare_applies_the_exact_bytes() {
    let fixture = Fixture::new();
    let repository = fixture.open();
    let target = fixture.target(&[b"a.txt"]);
    let plan = repository
        .plan_safe_commit_target(&target)
        .expect("a target plan needs no payload");

    assert_eq!(plan.destination(), &fixture.destination());
    assert_eq!(plan.paths(), &[fixture.path(b"a.txt")]);
    assert_eq!(plan.route(), SafeCommitRoute::Worktree);
    assert_eq!(plan.requested_paths(), vec![fixture.path(b"a.txt")]);
    assert!(plan.expected_parent().is_none());
    assert!(plan.expected_bytes().is_empty());

    let legacy = repository
        .plan_safe_commit(&fixture.request("legacy", &[b"a.txt"]))
        .unwrap();
    assert_eq!(plan.head(), legacy.head());
    assert_eq!(plan.destination_tip(), legacy.destination_tip());
    assert_eq!(plan.guard_verdict(), legacy.guard_verdict());
    assert_eq!(plan.index_authority(), legacy.index_authority());

    let prepared = plan
        .clone()
        .prepare(CommitMessage::from_bytes(b"op(\x80): exact\n"))
        .expect("the bytes are an applicable payload");
    assert_eq!(prepared.message_bytes(), b"op(\x80): exact\n");
    assert!(prepared.message_utf8().is_none());
    assert_eq!(prepared.destination(), plan.destination());
    assert_eq!(prepared.head(), plan.head());
    assert_eq!(prepared.destination_tip(), plan.destination_tip());
    assert_eq!(prepared.route(), plan.route());

    let outcome = repository.apply_safe_message_commit(&prepared).unwrap();
    assert_eq!(
        fixture.git(&["rev-parse", "HEAD"]),
        format!("{}\n", outcome.commit()).into_bytes()
    );
    assert_eq!(outcome.committed_paths(), &[fixture.path(b"a.txt")]);
    assert_eq!(outcome.destination(), &fixture.destination());
}

#[test]
fn the_legacy_request_and_plan_getters_are_unchanged() {
    let fixture = Fixture::new();
    let repository = fixture.open();
    let request = fixture
        .request("legacy message", &[b"a.txt"])
        .with_protection(
            ProtectedBranches::default(),
            CommitGuardCapability::Standard,
        );
    assert_eq!(request.message(), "legacy message");
    assert_eq!(request.paths(), &[fixture.path(b"a.txt")]);
    assert_eq!(request.destination(), &fixture.destination());
    assert!(request.expected_parent().is_none());
    assert!(request.index_deletions().is_empty());
    assert!(request.expected_bytes().is_empty());

    let plan = repository.plan_safe_commit(&request).unwrap();
    assert_eq!(plan.message(), "legacy message");
    assert_eq!(plan.destination(), &fixture.destination());
    assert_eq!(plan.paths(), &[fixture.path(b"a.txt")]);
    let outcome = repository.apply_safe_commit(&plan).unwrap();
    assert_eq!(outcome.committed_paths(), &[fixture.path(b"a.txt")]);
}

#[test]
fn a_bad_message_still_precedes_a_bad_path_on_the_legacy_entry_point() {
    let fixture = Fixture::new();
    let repository = fixture.open();
    let request = fixture.request("", &[b"../outside.txt"]);
    let error = repository.plan_safe_commit(&request).unwrap_err();
    assert!(
        matches!(
            &error,
            GitError::SafeCommitInvalidRequest { detail }
                if detail == "a safe commit needs a non-empty message"
        ),
        "the legacy order keeps Message before PathResolution: {error:?}"
    );

    let target = fixture.target(&[b"../outside.txt"]);
    let failure = failure(repository.plan_safe_commit_target(&target));
    assert_eq!(failure.stage(), SafeCommitPreflightStage::PathResolution);
    assert!(matches!(
        failure.error(),
        GitError::SafeCommitPathOutsideWorktree { .. }
    ));
}

#[test]
fn shared_failures_carry_the_same_error_value_and_a_truthful_stage() {
    let fixture = Fixture::new();
    fixture.git(&["commit", "-qam", "second"]);
    fs::write(fixture.path.join("a.txt"), b"three\n").unwrap();
    let repository = fixture.open();
    let parent = Oid::parse(
        std::str::from_utf8(&fixture.git(&["rev-parse", "HEAD~1"]))
            .unwrap()
            .trim(),
    )
    .unwrap();

    let cases: Vec<(
        &str,
        SafeCommitPreflightStage,
        SafeCommitRequest,
        SafeCommitTargetRequest,
    )> = vec![
        (
            "empty changeset",
            SafeCommitPreflightStage::RequestShape,
            fixture.request("m", &[]),
            fixture.target(&[]),
        ),
        (
            "expected bytes without a parent",
            SafeCommitPreflightStage::RequestShape,
            fixture
                .request("m", &[b"a.txt"])
                .with_expected_bytes([ExpectedBlob::new(fixture.path(b"a.txt"), b"one\n")]),
            fixture
                .target(&[b"a.txt"])
                .with_expected_bytes([ExpectedBlob::new(fixture.path(b"a.txt"), b"one\n")]),
        ),
        (
            "deletion conflict",
            SafeCommitPreflightStage::RequestShape,
            fixture
                .request("m", &[b"a.txt"])
                .with_index_deletions([fixture.path(b"a.txt")]),
            fixture
                .target(&[b"a.txt"])
                .with_index_deletions([fixture.path(b"a.txt")]),
        ),
        (
            "path resolution",
            SafeCommitPreflightStage::PathResolution,
            fixture.request("m", &[b"../escape.txt"]),
            fixture.target(&[b"../escape.txt"]),
        ),
        (
            "path policy",
            SafeCommitPreflightStage::PathPolicy,
            fixture.request("m", &[b".worktrees/lane/a.txt"]),
            fixture.target(&[b".worktrees/lane/a.txt"]),
        ),
        (
            "path shape",
            SafeCommitPreflightStage::PathShape,
            fixture.request("m", &[b"sub"]),
            fixture.target(&[b"sub"]),
        ),
        (
            "head assertion",
            SafeCommitPreflightStage::HeadAssertion,
            {
                fixture.git(&["branch", "other"]);
                SafeCommitRequest::new(
                    BranchName::parse("other").unwrap(),
                    "m",
                    [fixture.path(b"a.txt")],
                )
            },
            SafeCommitTargetRequest::new(
                BranchName::parse("other").unwrap(),
                [fixture.path(b"a.txt")],
            ),
        ),
        (
            "expected parent",
            SafeCommitPreflightStage::ExpectedParent,
            fixture
                .request("m", &[b"a.txt"])
                .with_expected_parent(parent.clone()),
            fixture.target(&[b"a.txt"]).with_expected_parent(parent),
        ),
        (
            "guard",
            SafeCommitPreflightStage::Guard,
            fixture.request("m", &[b"a.txt"]).with_protection(
                ProtectedBranches::parse([LANE]).unwrap(),
                CommitGuardCapability::Standard,
            ),
            fixture.target(&[b"a.txt"]).with_protection(
                ProtectedBranches::parse([LANE]).unwrap(),
                CommitGuardCapability::Standard,
            ),
        ),
    ];

    let sub = fixture.path.join("sub");
    fs::create_dir_all(sub.join("nested")).unwrap();
    fs::write(sub.join("one.txt"), b"one\n").unwrap();

    for (name, stage, legacy_request, target_request) in cases {
        let legacy = repository.plan_safe_commit(&legacy_request).unwrap_err();
        let failure = failure(repository.plan_safe_commit_target(&target_request));
        assert_eq!(
            failure.stage(),
            stage,
            "{name}: typed stage (error was {:?})",
            failure.error()
        );
        assert_eq!(
            format!("{:?}", failure.error()),
            format!("{legacy:?}"),
            "{name}: the target entry is the same planner body, not a second validator"
        );
        assert_eq!(
            format!("{:?}", failure.into_error()),
            format!("{legacy:?}"),
            "{name}: exact original value"
        );
    }
}

#[test]
fn an_unborn_destination_is_a_destination_stage() {
    let fixture = Fixture::unborn();
    let repository = fixture.open();
    let target = fixture.target(&[b"a.txt"]);
    let failure = failure(repository.plan_safe_commit_target(&target));
    assert_eq!(
        failure.stage(),
        SafeCommitPreflightStage::DestinationExistence,
        "{:?}",
        failure.error()
    );
    assert!(matches!(
        failure.error(),
        GitError::SafeCommitDestinationMissing { .. }
    ));
    let error: &dyn std::error::Error = &failure;
    assert!(error.source().unwrap().is::<GitError>());
    assert_eq!(
        error.source().unwrap().to_string(),
        failure.error().to_string()
    );
}

#[test]
fn a_held_index_lock_is_an_operation_stage() {
    let fixture = Fixture::new();
    let repository = fixture.open();
    let lock = fixture.path.join(".git/index.lock");
    fs::write(&lock, b"held").unwrap();
    let target = fixture.target(&[b"a.txt"]);
    let failure = failure(repository.plan_safe_commit_target(&target));
    assert_eq!(
        failure.stage(),
        SafeCommitPreflightStage::OperationInProgress
    );
    assert!(matches!(
        failure.error(),
        GitError::OperationInProgress { operation } if operation == "index-lock"
    ));
    assert!(
        lock.exists(),
        "the adapter never removes another writer's lock"
    );
    fs::remove_file(&lock).unwrap();
}

#[test]
fn the_target_plan_records_the_shared_route_and_expected_state() {
    let fixture = Fixture::new();
    let repository = fixture.open();
    let deletions = fixture
        .target(&[b"a.txt"])
        .with_index_deletions([fixture.path(b"seed.txt")]);
    let plan = repository.plan_safe_commit_target(&deletions).unwrap();
    assert_eq!(plan.route(), SafeCommitRoute::IndexDeletions);
    assert_eq!(plan.index_deletions(), &[fixture.path(b"seed.txt")]);

    let head = plan.head().clone();
    let pinned = fixture
        .target(&[b"a.txt"])
        .with_expected_parent(head.clone());
    let plan = repository.plan_safe_commit_target(&pinned).unwrap();
    assert_eq!(plan.route(), SafeCommitRoute::PinnedParent);
    assert_eq!(plan.expected_parent(), Some(&head));
}
