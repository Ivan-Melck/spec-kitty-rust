//! CF22 — the actual `doctor ops --close-stale` sweep, end to end.
//!
//! Every fixture here is a fresh temporary directory holding a REAL git
//! repository, and every Op record is created through the product API
//! ([`InvocationWriter::write_started`]). Nothing is stubbed: the sweep closes
//! through `ProfileInvocationExecutor::complete_invocation`, appends the real
//! closure spine, and commits it through the real `sk-git` transaction — so the
//! commit outcome each entry reports is measured, not declared.
//!
//! Ordinary (non-parity) functional smoke for this lane: no Source/oracle
//! pairing, no concurrency, no platform matrix, no performance measurement.
//! Those are listed as remaining debt in the lane report.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sk_invocation::ops_sweep::{
    DEFAULT_SWEEP_THRESHOLD_HOURS, SWEEP_COMMIT_SKIPPED_PREFIX, SweepAction, close_stale_ops,
};
use sk_invocation::{
    ClosedBy, InvocationWriter, ModeOfWork, OpOutcome, OpStartedEvent, OpStartedFields,
    read_op_closures,
};
use sk_kernel::clock::UtcDateTime;
use sk_kernel::clock::UtcZone;

/// A stale fixture id (26-char Crockford, so the canonical id boundary accepts it).
const STALE_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
/// A second valid fixture id, sorting after [`STALE_ID`].
const SECOND_ID: &str = "01BX5ZZKBKACTAV9WEVGEMMVRZ";
/// The fixture record's `started_at`.
const STARTED_AT: &str = "2026-04-21T10:00:00+00:00";

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root resolves")
}

fn git(dir: &Path, arguments: &[&str]) {
    let stdout = git_stdout(dir, arguments);
    let _ = stdout;
}

fn git_stdout(dir: &Path, arguments: &[&str]) -> String {
    let home = repository_root()
        .join("target")
        .join("sk-invocation-git-home");
    fs::create_dir_all(&home).expect("create scratch git home");
    let output = Command::new("git")
        .args(arguments)
        .current_dir(dir)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", home.join("gitconfig"))
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("execute git");
    assert!(
        output.status.success(),
        "git {arguments:?} failed in {}: {}{}",
        dir.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// A real repository on `main` with a local identity and one empty commit.
fn repo(path: &Path) {
    fs::create_dir_all(path).expect("create repository directory");
    git(path, &["init", "-b", "main"]);
    git(path, &["config", "user.email", "t@example.com"]);
    git(path, &["config", "user.name", "T"]);
    git(path, &["commit", "--allow-empty", "-m", "init"]);
}

fn started_record(invocation_id: &str, started_at: &str) -> OpStartedEvent {
    OpStartedEvent::new(OpStartedFields {
        invocation_id: sk_invocation::InvocationId::parse(invocation_id).expect("fixture id"),
        profile_id: "implementer-ivan".to_owned(),
        action: "implement".to_owned(),
        request_text: "implement the work package".to_owned(),
        actor: "claude".to_owned(),
        mode_of_work: ModeOfWork::TaskExecution,
        governance_context_hash: "abcdef0123456789".to_owned(),
        governance_context_available: true,
        router_confidence: None,
        started_at: started_at.to_owned(),
        mission_id: None,
        wp_id: None,
        model_id: None,
    })
    .expect("valid fixture record")
}

/// Open a real Op through the product writer; returns its record path.
fn open_op(root: &Path, invocation_id: &str, started_at: &str) -> PathBuf {
    InvocationWriter::new(root)
        .write_started(&started_record(invocation_id, started_at))
        .expect("the Op starts")
        .path
}

fn at(iso: &str) -> UtcDateTime<UtcZone> {
    sk_kernel::clock::parse_iso(iso).expect("fixture instant")
}

fn spine_path(root: &Path) -> PathBuf {
    root.join("kitty-ops").join("op-closures.jsonl")
}

#[test]
fn a_stale_open_op_really_closes_on_the_spine_and_its_record_is_untouched() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    repo(root);
    let record = open_op(root, STALE_ID, STARTED_AT);
    // A real project commits its Op records; the primary branch must track
    // them, otherwise the sweep's commit is deliberately skipped (#6410).
    git(root, &["add", "kitty-ops"]);
    git(root, &["commit", "-m", "op record"]);
    // Work on an unprotected branch: the closure commit itself must land.
    git(root, &["checkout", "-b", "work"]);
    let before = fs::read(&record).expect("read the record");

    let report = close_stale_ops(
        root,
        DEFAULT_SWEEP_THRESHOLD_HOURS,
        at("2026-04-22T12:00:00+00:00"),
    )
    .expect("the sweep runs");

    assert_eq!(report.threshold_hours, 24.0);
    assert_eq!(report.swept, 1, "{report:?}");
    assert_eq!(report.skipped_fresh, 0);
    assert!(!report.has_errors());
    assert_eq!(report.exit_code(), 0);
    assert_eq!(report.open_ops.len(), 1);
    let entry = &report.open_ops[0];
    assert_eq!(entry.invocation_id, STALE_ID);
    assert_eq!(entry.profile_id, "implementer-ivan");
    assert_eq!(entry.started_at, STARTED_AT);
    assert_eq!(entry.age_hours, Some(26.0));
    assert_eq!(entry.action_taken, SweepAction::ClosedAbandoned);
    assert_eq!(entry.parse_warning, None);
    assert_eq!(entry.error, None);
    assert_eq!(entry.commit.as_deref(), Some("committed"));

    // The per-record file is byte-frozen archive history.
    assert_eq!(
        fs::read(&record).expect("read the record"),
        before,
        "the sweep must never rewrite a per-record Op file"
    );

    // The closure is real: one spine row, the real abandoned/doctor_sweep event.
    let spine = spine_path(root);
    let text = fs::read_to_string(&spine).expect("the spine exists");
    assert_eq!(text.lines().count(), 1, "{text}");
    let closures = read_op_closures(root).expect("read the spine");
    assert_eq!(closures.len(), 1);
    assert_eq!(closures[0].invocation_id().as_str(), STALE_ID);
    assert_eq!(closures[0].outcome(), OpOutcome::Abandoned);
    assert_eq!(closures[0].closed_by(), ClosedBy::DoctorSweep);

    // The commit is real: HEAD carries the spine and the tree is clean.
    let committed = git_stdout(root, &["show", "--name-only", "--pretty=format:", "HEAD"]);
    assert!(
        committed.contains("kitty-ops/op-closures.jsonl"),
        "{committed}"
    );
    assert_eq!(
        git_stdout(root, &["status", "--porcelain"]).trim(),
        "",
        "the sweep committed the spine and left nothing dirty"
    );
}

#[test]
fn a_fresh_open_op_is_reported_but_never_closed() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    repo(root);
    let record = open_op(root, STALE_ID, STARTED_AT);
    let before = fs::read(&record).expect("read the record");

    let report = close_stale_ops(
        root,
        DEFAULT_SWEEP_THRESHOLD_HOURS,
        at("2026-04-21T12:00:00+00:00"),
    )
    .expect("the sweep runs");

    assert_eq!(report.swept, 0);
    assert_eq!(report.skipped_fresh, 1);
    assert_eq!(report.exit_code(), 1, "a remaining fresh Op exits 1");
    let entry = &report.open_ops[0];
    assert_eq!(entry.action_taken, SweepAction::None);
    assert_eq!(entry.age_hours, Some(2.0));
    assert_eq!(entry.commit, None);
    assert!(!spine_path(root).exists(), "nothing may be closed");
    assert_eq!(fs::read(&record).expect("read the record"), before);
}

#[test]
fn threshold_zero_closes_every_open_op_and_a_nan_threshold_closes_none() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    repo(root);
    open_op(root, STALE_ID, "2026-04-22T10:00:00+00:00");
    open_op(root, SECOND_ID, "2026-04-21T10:00:00+00:00");

    let report = close_stale_ops(root, 0.0, at("2026-04-22T12:00:00+00:00")).expect("sweep");
    assert_eq!(report.swept, 2);
    assert_eq!(report.skipped_fresh, 0);
    assert_eq!(report.exit_code(), 0);
    // Nothing tracks Op records on the primary branch here, so the commit is
    // deliberately skipped — and reported as a skip, never as "committed".
    let commit = report.open_ops[0].commit.clone().expect("a commit note");
    assert!(commit.starts_with(SWEEP_COMMIT_SKIPPED_PREFIX), "{commit}");
    assert_eq!(
        fs::read_to_string(spine_path(root))
            .expect("spine")
            .lines()
            .count(),
        2
    );

    let nan_temp = tempfile::tempdir().expect("tempdir");
    let nan_root = nan_temp.path();
    repo(nan_root);
    open_op(nan_root, STALE_ID, "2026-04-22T10:00:00+00:00");
    let report =
        close_stale_ops(nan_root, f64::NAN, at("2026-04-22T12:00:00+00:00")).expect("sweep");
    assert_eq!(report.swept, 0, "Python's `age > nan` is always false");
    assert_eq!(report.skipped_fresh, 1);
    assert_eq!(report.exit_code(), 1);
    assert!(!spine_path(nan_root).exists());
}

#[test]
fn a_closed_op_is_not_listed_and_a_second_sweep_is_a_no_op() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    repo(root);
    // One record closed by an agent (a real `completed` row in the trail).
    let agent_record = open_op(root, SECOND_ID, STARTED_AT);
    let writer = InvocationWriter::new(root);
    writer
        .write_completed(
            &sk_invocation::OpCompletedEvent::new(sk_invocation::OpCompletedFields {
                invocation_id: sk_invocation::InvocationId::parse(SECOND_ID).expect("id"),
                completed_at: "2026-04-21T10:05:00+00:00".to_owned(),
                outcome: OpOutcome::Done,
                closed_by: ClosedBy::Agent,
                evidence_ref: None,
            })
            .expect("completed record"),
        )
        .expect("agent close");
    // One record the sweep will close.
    open_op(root, STALE_ID, STARTED_AT);
    let agent_before = fs::read(&agent_record).expect("read the agent record");

    let now = at("2026-04-22T12:00:00+00:00");
    let first = close_stale_ops(root, DEFAULT_SWEEP_THRESHOLD_HOURS, now).expect("sweep");
    assert_eq!(first.swept, 1);
    assert_eq!(first.open_ops[0].invocation_id, STALE_ID);
    let spine_after_first = fs::read(spine_path(root)).expect("spine");

    let second = close_stale_ops(root, DEFAULT_SWEEP_THRESHOLD_HOURS, now).expect("retry");
    assert_eq!(second.swept, 0, "a retry must not close anything again");
    assert!(second.open_ops.is_empty(), "{second:?}");
    assert_eq!(second.exit_code(), 0);
    assert_eq!(
        fs::read(spine_path(root)).expect("spine"),
        spine_after_first,
        "a retry must not append a second closure"
    );
    assert_eq!(
        fs::read(&agent_record).expect("read"),
        agent_before,
        "the agent-closed record is untouched"
    );
}

#[test]
fn an_unparseable_started_at_is_stale_with_a_parse_warning() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    repo(root);
    let dir = root.join("kitty-ops");
    fs::create_dir_all(&dir).expect("kitty-ops");
    let record = dir.join(format!("{STALE_ID}.jsonl"));
    fs::write(
        &record,
        "{\"event\": \"started\", \"invocation_id\": \"01ARZ3NDEKTSV4RRFFQ69G5FAV\", \
         \"profile_id\": \"implementer-ivan\", \"action\": \"implement\", \
         \"started_at\": \"not-a-timestamp\"}\n",
    )
    .expect("write the broken record");

    let report = close_stale_ops(
        root,
        DEFAULT_SWEEP_THRESHOLD_HOURS,
        at("2026-04-22T12:00:00+00:00"),
    )
    .expect("the sweep runs");

    let entry = &report.open_ops[0];
    assert_eq!(entry.age_hours, None);
    assert_eq!(
        entry.parse_warning.as_deref(),
        Some("unparseable started_at \"not-a-timestamp\"; treated as stale")
    );
    assert_eq!(entry.action_taken, SweepAction::ClosedAbandoned);
    assert_eq!(report.swept, 1);
    assert_eq!(report.exit_code(), 0);
}

#[test]
fn one_failed_close_is_recorded_and_the_sweep_continues() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    repo(root);
    let dir = root.join("kitty-ops");
    fs::create_dir_all(&dir).expect("kitty-ops");
    // A record whose file name is not a canonical 26-char Crockford id cannot
    // be closed through the id boundary. It sorts first (`!` < `0`), so a sweep
    // that stopped on it would never reach the valid record.
    fs::write(
        dir.join("!!not-a-ulid.jsonl"),
        "{\"event\": \"started\", \"profile_id\": \"implementer-ivan\", \
         \"action\": \"implement\", \"started_at\": \"2026-04-21T10:00:00+00:00\"}\n",
    )
    .expect("write the unnameable record");
    open_op(root, STALE_ID, STARTED_AT);

    let report = close_stale_ops(
        root,
        DEFAULT_SWEEP_THRESHOLD_HOURS,
        at("2026-04-22T12:00:00+00:00"),
    )
    .expect("the sweep runs");

    assert_eq!(report.open_ops.len(), 2, "{report:?}");
    assert_eq!(report.open_ops[0].invocation_id, "!!not-a-ulid");
    let error = report.open_ops[0]
        .error
        .as_deref()
        .expect("the failed close is an error entry");
    assert!(error.contains("26-char ULID"), "{error}");
    assert_eq!(report.open_ops[0].action_taken, SweepAction::None);
    assert_eq!(
        report.open_ops[1].action_taken,
        SweepAction::ClosedAbandoned,
        "one bad record must not block the sweep"
    );
    assert_eq!(report.swept, 1);
    assert!(report.has_errors());
    assert_eq!(report.exit_code(), 1, "a per-Op error exits 1");
    assert_eq!(
        read_op_closures(root).expect("spine").len(),
        1,
        "only the valid record was closed"
    );
}

#[test]
fn the_named_non_record_files_are_never_closed() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    repo(root);
    let dir = root.join("kitty-ops");
    fs::create_dir_all(&dir).expect("kitty-ops");
    let stale_row = "{\"event\": \"started\", \"profile_id\": \"p\", \
                     \"started_at\": \"2020-01-01T00:00:00+00:00\"}\n";
    let mut written = Vec::new();
    for name in [
        "ops-index.jsonl",
        "lifecycle.jsonl",
        "propagation-errors.jsonl",
        "op-closures.jsonl",
    ] {
        let path = dir.join(name);
        fs::write(&path, stale_row).expect("write non-record file");
        written.push((path, stale_row.to_owned()));
    }

    let report = close_stale_ops(root, 0.0, at("2026-04-22T12:00:00+00:00")).expect("sweep");

    assert!(report.open_ops.is_empty(), "{report:?}");
    assert_eq!(report.swept, 0);
    for (path, contents) in written {
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            contents,
            "{} must not be touched",
            path.display()
        );
    }
}

#[test]
fn a_protected_branch_reports_the_refusal_and_keeps_the_closure_on_disk() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    repo(root);
    open_op(root, STALE_ID, STARTED_AT);
    git(root, &["add", "kitty-ops"]);
    git(root, &["commit", "-m", "op record"]);

    let report = close_stale_ops(
        root,
        DEFAULT_SWEEP_THRESHOLD_HOURS,
        at("2026-04-22T12:00:00+00:00"),
    )
    .expect("the sweep runs");

    let entry = &report.open_ops[0];
    assert_eq!(entry.action_taken, SweepAction::ClosedAbandoned);
    assert_eq!(
        entry.commit.as_deref(),
        Some("not_committed:protected branch 'main'"),
        "the guard refusal is reported, never a fake commit"
    );
    assert_eq!(
        report.exit_code(),
        0,
        "a guard refusal is not a per-Op error"
    );
    assert_eq!(
        read_op_closures(root).expect("spine").len(),
        1,
        "the closure stays durable on disk even when the commit is refused"
    );
    assert!(
        !git_stdout(root, &["status", "--porcelain"])
            .trim()
            .is_empty(),
        "the uncommitted spine is visible in the worktree"
    );
}
