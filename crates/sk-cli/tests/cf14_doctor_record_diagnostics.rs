//! CF14 focused smoke: the three new read-only `doctor` record diagnostics
//! (`decisions`, `invocation-pairing`, `ops`) against real fixtures and the
//! actual `spec-kitty` binary.
//!
//! These exercise the canonical authorities — the `sk-workspace` decision
//! model/store and placement resolver, `sk-invocation`'s Op-store constants and
//! closure-spine reader, the Kernel Python-JSON/clock authorities — plus the
//! refusals that must fire BEFORE any write.
//!
//! They are **not** a whole-command parity campaign. Deliberately deferred deep
//! proof: the Source `specify_cli.decisions.fork` two-surface report, `--repair`
//! and `--close-stale` behaviour, coordination-topology event-log routing, a
//! corrupt (non-UTF-8) store, huge stores, platform-specific path spellings and
//! byte-level human-output diffs.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};
use sk_workspace::doctor::{
    OpStoreLayout, audit_decisions, audit_invocation_pairing, audit_open_ops, pairing_report,
    read_lifecycle_records,
};

const MISSION_ID: &str = "01KN2371WNZX28ME4MN97B67PJ";
const SLUG: &str = "001-demo";

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_spec-kitty")
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, contents).expect("fixture write");
}

fn git(root: &Path, home: &Path, args: &[&str]) -> Output {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "CF14 Fixture")
        .env("GIT_AUTHOR_EMAIL", "cf14@example.invalid")
        .env("GIT_COMMITTER_NAME", "CF14 Fixture")
        .env("GIT_COMMITTER_EMAIL", "cf14@example.invalid")
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

/// A real repository root with one committed file and one Mission directory.
struct Fixture {
    root: tempfile::TempDir,
    home: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("tempdir");
        let home = tempfile::tempdir().expect("home");
        git(
            root.path(),
            home.path(),
            &["init", "--quiet", "--initial-branch=main"],
        );
        write(&root.path().join("README.md"), "fixture\n");
        git(root.path(), home.path(), &["add", "README.md"]);
        git(
            root.path(),
            home.path(),
            &["commit", "--quiet", "-m", "fixture"],
        );
        let fixture = Self { root, home };
        fixture.mission(
            &format!(
                "{{\"mission_id\":\"{MISSION_ID}\",\"mission_slug\":\"{SLUG}\",\"topology\":\"lanes\",\"target_branch\":\"main\"}}"
            ),
        );
        fixture
    }

    fn path(&self) -> &Path {
        self.root.path()
    }

    fn mission_dir(&self) -> PathBuf {
        self.path().join("kitty-specs").join(SLUG)
    }

    fn mission(&self, meta: &str) {
        write(&self.mission_dir().join("meta.json"), meta);
    }

    fn events(&self, lines: &[String]) {
        write(
            &self.mission_dir().join("status.events.jsonl"),
            &format!("{}\n", lines.join("\n")),
        );
    }

    fn index(&self, entries: &[String]) {
        write(
            &self.mission_dir().join("decisions/index.json"),
            &format!(
                "{{\"version\":1,\"mission_id\":\"{MISSION_ID}\",\"entries\":[{}]}}\n",
                entries.join(",")
            ),
        );
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(binary())
            .current_dir(self.path())
            .env("HOME", self.home.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("SPECIFY_REPO_ROOT")
            .env_remove("GIT_CEILING_DIRECTORIES")
            .env("SPECIFY_KITTY_NO_NAG", "1")
            .env("SPECIFY_KITTY_NO_UPGRADE_CHECK", "1")
            .args(args)
            .output()
            .expect("run spec-kitty")
    }
}

fn json_of(output: &Output) -> Value {
    let text = String::from_utf8(output.stdout.clone()).expect("stdout is UTF-8");
    serde_json::from_str(text.trim()).unwrap_or_else(|error| {
        panic!(
            "stdout was not one JSON document ({error}):\n{text}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

/// A byte/mode/mtime/inode snapshot of every entry below `root`.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, String> {
    let mut entries = BTreeMap::new();
    fn walk(base: &Path, path: &Path, entries: &mut BTreeMap<PathBuf, String>) {
        let metadata = fs::symlink_metadata(path).expect("metadata");
        #[cfg(unix)]
        let mode = {
            use std::os::unix::fs::PermissionsExt;
            metadata.permissions().mode()
        };
        #[cfg(not(unix))]
        let mode = 0u32;
        #[cfg(unix)]
        let inode = {
            use std::os::unix::fs::MetadataExt;
            metadata.ino()
        };
        #[cfg(not(unix))]
        let inode = 0u64;
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let contents = if metadata.is_file() {
            fs::read(path)
                .map(|bytes| format!("{}:{}", bytes.len(), fnv1a(&bytes)))
                .unwrap_or_else(|_| "unreadable".to_owned())
        } else {
            String::new()
        };
        entries.insert(
            path.strip_prefix(base).unwrap_or(path).to_path_buf(),
            format!("mode={mode} inode={inode} mtime={modified} {contents}"),
        );
        if metadata.is_dir() {
            let mut children: Vec<PathBuf> = fs::read_dir(path)
                .expect("read dir")
                .flatten()
                .map(|entry| entry.path())
                .collect();
            children.sort();
            for child in children {
                walk(base, &child, entries);
            }
        }
    }
    walk(root, root, &mut entries);
    entries
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn opened_line(id: &str, at: &str) -> String {
    format!(
        r#"{{"event_id":"e-{id}-1","event_type":"DecisionPointOpened","payload":{{"decision_point_id":"{id}","origin_flow":"specify","step_id":"s","input_key":"k","question":"q","options":[],"state_entered_at":"{at}","actor_id":"claude","mission_id":"{MISSION_ID}","mission_slug":"{SLUG}"}}}}"#
    )
}

fn resolved_line(id: &str, outcome: &str, at: &str) -> String {
    format!(
        r#"{{"event_id":"e-{id}-2","event_type":"DecisionPointResolved","payload":{{"decision_point_id":"{id}","terminal_outcome":"{outcome}","state_entered_at":"{at}","resolved_by":"operator"}}}}"#
    )
}

fn index_entry(id: &str, status: &str) -> String {
    format!(
        r#"{{"decision_id":"{id}","origin_flow":"specify","step_id":"s","slot_key":null,"input_key":"k","question":"q","options":[],"status":"{status}","final_answer":null,"rationale":null,"other_answer":false,"summary_json":null,"created_at":"2026-01-01T00:00:00Z","resolved_at":null,"resolved_by":null,"opened_by":"claude","mission_id":"{MISSION_ID}","mission_slug":"{SLUG}"}}"#
    )
}

fn lifecycle_record(action: &str, phase: &str, at: &str, wp: Option<&str>) -> String {
    match wp {
        Some(wp) => format!(
            r#"{{"canonical_action_id":"{action}","phase":"{phase}","at":"{at}","agent":"claude","mission_id":"{MISSION_ID}","wp_id":"{wp}"}}"#
        ),
        None => format!(
            r#"{{"canonical_action_id":"{action}","phase":"{phase}","at":"{at}","agent":"claude","mission_id":"{MISSION_ID}"}}"#
        ),
    }
}

// ---------------------------------------------------------------------------
// decisions
// ---------------------------------------------------------------------------

#[test]
fn decisions_reports_a_clean_mission_and_exits_zero() {
    let fixture = Fixture::new();
    fixture.events(&[opened_line("d1", "2026-01-01T00:00:00+00:00")]);
    fixture.index(&[index_entry("d1", "open")]);

    let output = fixture.run(&["doctor", "decisions", "--mission", SLUG, "--json"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload = json_of(&output);
    assert_eq!(payload["mission_slug"], json!(SLUG));
    assert_eq!(payload["clean"], json!(true));
    assert_eq!(payload["log_decision_ids"], json!(["d1"]));
    assert_eq!(payload["index_decision_ids"], json!(["d1"]));
    assert_eq!(payload["repaired"], json!(false));
    assert!(payload.get("unevaluated_findings").is_none());
    assert_eq!(payload["forked"], json!(false));
    assert_eq!(payload["findings"], json!([]));
    assert_eq!(payload["streams"][0]["state"], json!("single_home"));
    assert_eq!(payload["streams"][1]["state"], json!("absent"));
    assert_eq!(payload["ledger"]["state"], json!("primary"));
}

#[test]
fn decisions_reports_missing_orphaned_and_stale_entries_with_exit_zero() {
    let fixture = Fixture::new();
    fixture.events(&[
        opened_line("d-missing", "2026-01-01T00:00:00+00:00"),
        opened_line("d-stale", "2026-01-01T00:00:00+00:00"),
        resolved_line("d-stale", "resolved", "2026-01-02T00:00:00+00:00"),
    ]);
    fixture.index(&[
        index_entry("d-stale", "deferred"),
        index_entry("d-orphan", "open"),
    ]);

    let output = fixture.run(&["doctor", "decisions", "--mission", SLUG, "--json"]);
    assert!(output.status.success(), "read-only diagnose is report-only");
    let payload = json_of(&output);
    assert_eq!(payload["clean"], json!(false));
    assert_eq!(payload["missing_from_index"], json!(["d-missing"]));
    assert_eq!(payload["orphaned_in_index"], json!(["d-orphan"]));
    assert_eq!(
        payload["status_mismatch"],
        json!([{
            "decision_id": "d-stale",
            "index_status": "deferred",
            "folded_status": "resolved",
        }])
    );
}

#[test]
fn decisions_reports_a_malformed_group_and_names_the_remedy() {
    let fixture = Fixture::new();
    fixture.events(&[
        opened_line("d1", "2026-01-01T00:00:00+00:00"),
        resolved_line("d1", "resolved", "2026-01-02T00:00:00+00:00"),
        resolved_line("d1", "canceled", "2026-01-03T00:00:00+00:00"),
    ]);
    fixture.index(&[index_entry("d1", "resolved")]);

    let output = fixture.run(&["doctor", "decisions", "--mission", SLUG, "--json"]);
    assert!(output.status.success());
    let payload = json_of(&output);
    assert_eq!(payload["malformed_folds"], json!(["d1"]));
    assert_eq!(payload["clean"], json!(false));
    let remedy = payload["remedy"].as_str().expect("remedy");
    assert!(remedy.contains(&format!("--mission {SLUG}")), "{remedy}");
    assert!(remedy.contains("status.events.jsonl"), "{remedy}");
}

#[test]
fn decisions_refuses_a_corrupt_index_instead_of_reading_it_as_clean() {
    let fixture = Fixture::new();
    fixture.events(&[]);
    write(
        &fixture.mission_dir().join("decisions/index.json"),
        "{not json",
    );

    let output = fixture.run(&["doctor", "decisions", "--mission", SLUG, "--json"]);
    assert_eq!(output.status.code(), Some(1));
    let payload = json_of(&output);
    assert_eq!(payload["error_code"], json!("DECISION_INDEX_UNREADABLE"));
}

#[test]
fn decisions_repair_is_refused_before_any_write() {
    let fixture = Fixture::new();
    fixture.events(&[opened_line("d1", "2026-01-01T00:00:00+00:00")]);
    fixture.index(&[index_entry("d1", "resolved")]);
    let before = snapshot(fixture.path());

    let output = fixture.run(&[
        "doctor",
        "decisions",
        "--mission",
        SLUG,
        "--repair",
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(2));
    let payload = json_of(&output);
    assert_eq!(
        payload["error_code"],
        json!("UNSUPPORTED_DOCTOR_DECISIONS_REPAIR")
    );
    assert!(
        payload["coverage"]
            .as_array()
            .is_some_and(|items| items.len() >= 2)
    );
    assert_eq!(
        snapshot(fixture.path()),
        before,
        "no write, lock or state file"
    );
}

// ---------------------------------------------------------------------------
// invocation-pairing
// ---------------------------------------------------------------------------

#[test]
fn invocation_pairing_reports_an_orphan_and_exits_one() {
    let fixture = Fixture::new();
    write(
        &fixture.path().join("kitty-ops/lifecycle.jsonl"),
        &format!(
            "{}\n{}\n",
            lifecycle_record(
                "plan::specify",
                "started",
                "2026-01-01T00:00:00+00:00",
                Some("WP01")
            ),
            lifecycle_record(
                "plan::specify",
                "started",
                "2026-01-02T00:00:00+00:00",
                None
            )
            .replace(MISSION_ID, "01ZZZZZZZZZZZZZZZZZZZZZZZZ")
        ),
    );

    let output = fixture.run(&["doctor", "invocation-pairing", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    let payload = json_of(&output);
    assert_eq!(payload["orphan_count"], json!(2));
    assert_eq!(payload["total_groups"], json!(2));
    assert_eq!(payload["pairing_rate"], json!(0.0));
    assert_eq!(payload["orphans"][0]["wp_id"], json!("WP01"));
    assert_eq!(
        payload["orphans"][0]["started_at"],
        json!("2026-01-01T00:00:00+00:00")
    );
    // `sort_keys=True` shape: sorted at every level.
    let keys: Vec<_> = payload
        .as_object()
        .expect("object")
        .keys()
        .cloned()
        .collect();
    assert_eq!(
        keys,
        vec!["orphan_count", "orphans", "pairing_rate", "total_groups"]
    );
    let entry_keys: Vec<_> = payload["orphans"][0]
        .as_object()
        .expect("entry")
        .keys()
        .cloned()
        .collect();
    assert_eq!(
        entry_keys,
        vec![
            "agent",
            "canonical_action_id",
            "mission_id",
            "started_at",
            "wp_id"
        ]
    );
}

#[test]
fn invocation_pairing_is_green_when_every_start_is_paired() {
    let fixture = Fixture::new();
    write(
        &fixture.path().join("kitty-ops/lifecycle.jsonl"),
        &format!(
            "{}\n{}\n",
            lifecycle_record(
                "plan::specify",
                "started",
                "2026-01-01T00:00:00+00:00",
                None
            ),
            lifecycle_record("plan::specify", "failed", "2026-01-01T01:00:00+00:00", None)
        ),
    );

    let output = fixture.run(&["doctor", "invocation-pairing", "--json"]);
    assert!(output.status.success(), "paired store is green");
    let payload = json_of(&output);
    assert_eq!(payload["orphan_count"], json!(0));
    assert_eq!(payload["pairing_rate"], json!(1.0));
    assert_eq!(payload["total_groups"], json!(1));
}

// ---------------------------------------------------------------------------
// ops
// ---------------------------------------------------------------------------

#[test]
fn ops_lists_an_open_record_and_exits_one() {
    let fixture = Fixture::new();
    let ops_dir = fixture.path().join("kitty-ops");
    write(&ops_dir.join("01OPEN.jsonl"), r#"{"event":"started"}"#);

    let output = fixture.run(&["doctor", "ops", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        json_of(&output),
        json!([{"path": "kitty-ops/01OPEN.jsonl"}])
    );
}

#[test]
fn ops_ignores_completed_records_spine_closures_and_non_record_files() {
    let fixture = Fixture::new();
    let ops_dir = fixture.path().join("kitty-ops");
    write(
        &ops_dir.join("01DONE.jsonl"),
        "{\"event\":\"started\"}\n{\"event\":\"completed\"}\n",
    );
    write(&ops_dir.join("01SWEPT.jsonl"), "{\"event\":\"started\"}\n");
    write(&ops_dir.join("ops-index.jsonl"), "{\"not\":\"a record\"}\n");
    write(&ops_dir.join("lifecycle.jsonl"), "{\"not\":\"a record\"}\n");
    write(
        &ops_dir.join("propagation-errors.jsonl"),
        "{\"not\":\"a record\"}\n",
    );
    write(
        &ops_dir.join("op-closures.jsonl"),
        "{\"event\":\"completed\",\"invocation_id\":\"01SWEPT000000000000000000A\",\"completed_at\":\"2026-01-01T00:00:00+00:00\",\"outcome\":\"abandoned\",\"closed_by\":\"doctor_sweep\"}\n",
    );
    // The closure spine names a valid ULID; rename the record to match it.
    fs::rename(
        ops_dir.join("01SWEPT.jsonl"),
        ops_dir.join("01SWEPT000000000000000000A.jsonl"),
    )
    .expect("rename swept record");

    let output = fixture.run(&["doctor", "ops", "--json"]);
    assert!(
        output.status.success(),
        "stderr: {}\nstdout: {}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(json_of(&output), json!([]));
}

#[test]
fn ops_close_stale_closes_on_spine_and_retry_preserves_record() {
    let fixture = Fixture::new();
    let id = "01ARZ3NDEKTSV4RRFFQ69G5FAV"; // Valid, explicitly authored fixture id.
    let started = sk_invocation::OpStartedEvent::new(sk_invocation::OpStartedFields {
        invocation_id: sk_invocation::InvocationId::parse(id).expect("fixture id"),
        profile_id: "implementer-ivan".to_owned(),
        action: "implement".to_owned(),
        request_text: "fixture work".to_owned(),
        actor: "fixture".to_owned(),
        mode_of_work: sk_invocation::ModeOfWork::TaskExecution,
        governance_context_hash: "fixture-context".to_owned(),
        governance_context_available: true,
        router_confidence: None,
        started_at: sk_kernel::clock::now_utc_iso(),
        mission_id: None,
        wp_id: None,
        model_id: None,
    })
    .expect("valid started record");
    let record = sk_invocation::InvocationWriter::new(fixture.path())
        .write_started(&started)
        .expect("real started publication")
        .path;
    let before = fs::read(&record).expect("record bytes");
    let argv = [
        "doctor",
        "ops",
        "--close-stale",
        "--threshold",
        "0",
        "--json",
    ];
    let output = fixture.run(&argv);
    assert_eq!(output.status.code(), Some(0), "{:?}", output);
    let payload = json_of(&output);
    assert_eq!(payload["swept"], json!(1), "{payload}");
    assert_eq!(payload["skipped_fresh"], json!(0));
    assert_eq!(payload["open_ops"][0]["invocation_id"], json!(id));
    assert_eq!(
        payload["open_ops"][0]["action_taken"],
        json!("closed_abandoned")
    );
    let closures = sk_invocation::read_op_closures(fixture.path()).expect("actual spine");
    assert_eq!(closures.len(), 1);
    assert_eq!(closures[0].invocation_id().as_str(), id);
    assert_eq!(closures[0].outcome(), sk_invocation::OpOutcome::Abandoned);
    assert_eq!(
        closures[0].closed_by(),
        sk_invocation::ClosedBy::DoctorSweep
    );
    assert_eq!(fs::read(&record).expect("byte-frozen record"), before);

    let spine = fixture
        .path()
        .join(sk_invocation::EVENTS_DIR)
        .join(sk_invocation::OP_CLOSURES_FILENAME);
    let first_spine = fs::read(&spine).expect("spine bytes");
    let retry = fixture.run(&argv);
    assert_eq!(retry.status.code(), Some(0), "{:?}", retry);
    assert_eq!(json_of(&retry)["swept"], json!(0));
    assert_eq!(fs::read(spine).expect("retry spine"), first_spine);
    assert_eq!(fs::read(record).expect("retry record"), before);
}

#[test]
fn ops_threshold_without_close_stale_is_a_usage_refusal() {
    let fixture = Fixture::new();
    let output = fixture.run(&["doctor", "ops", "--threshold", "4", "--json"]);
    assert_eq!(output.status.code(), Some(2));
    let payload = json_of(&output);
    assert!(payload["error"].is_string());
    assert!(
        payload["error"]
            .as_str()
            .expect("error")
            .contains("--close-stale")
    );
}

// ---------------------------------------------------------------------------
// read-only footprint
// ---------------------------------------------------------------------------

#[test]
fn every_new_read_only_leaf_leaves_the_tree_byte_identical() {
    let fixture = Fixture::new();
    fixture.events(&[opened_line("d1", "2026-01-01T00:00:00+00:00")]);
    fixture.index(&[index_entry("d1", "open")]);
    write(
        &fixture.path().join("kitty-ops/lifecycle.jsonl"),
        &format!(
            "{}\n",
            lifecycle_record(
                "plan::specify",
                "started",
                "2026-01-01T00:00:00+00:00",
                None
            )
        ),
    );
    write(
        &fixture.path().join("kitty-ops/01OPEN.jsonl"),
        r#"{"event":"started"}"#,
    );

    let before = snapshot(fixture.path());
    let runs = [
        fixture.run(&["doctor", "decisions", "--mission", SLUG]),
        fixture.run(&["doctor", "invocation-pairing"]),
        fixture.run(&["doctor", "ops"]),
    ];
    for (index, output) in runs.iter().enumerate() {
        assert!(
            output.status.code().is_some(),
            "run {index} did not complete: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let after = snapshot(fixture.path());
    let added: BTreeSet<&PathBuf> = after
        .keys()
        .filter(|key| !before.contains_key(*key))
        .collect();
    assert!(added.is_empty(), "new entries: {added:?}");
    assert_eq!(before, after, "bytes/mode/inode/mtime changed");
}

// ---------------------------------------------------------------------------
// the workspace diagnosis, driven directly (no git, no CLI)
// ---------------------------------------------------------------------------

#[test]
fn the_decision_audit_reads_the_canonical_store_and_log() {
    let root = tempfile::tempdir().expect("tempdir");
    let mission = root.path().join("kitty-specs").join(SLUG);
    fs::create_dir_all(&mission).expect("mission dir");
    fs::write(
        mission.join("status.events.jsonl"),
        format!("{}\n", opened_line("d1", "2026-01-01T00:00:00+00:00")),
    )
    .expect("log");
    write(
        &mission.join("decisions/index.json"),
        &format!("{{\"version\":1,\"mission_id\":\"{MISSION_ID}\",\"entries\":[]}}\n"),
    );

    let audit = audit_decisions(&mission, &mission, SLUG).expect("audit");
    assert_eq!(audit.missing_from_index, vec!["d1"]);
    assert!(!audit.clean());
}

#[test]
fn the_pairing_report_groups_by_mission_and_action() {
    let root = tempfile::tempdir().expect("tempdir");
    write(
        &root.path().join("kitty-ops/lifecycle/lifecycle.json"),
        "not a record\n",
    );
    write(
        &root.path().join("kitty-ops/lifecycle/01REC.json"),
        &lifecycle_record("plan::plan", "started", "2026-01-01T00:00:00Z", None),
    );
    let records = read_lifecycle_records(root.path()).expect("records");
    assert_eq!(records.len(), 1);
    let report = pairing_report(&records);
    assert_eq!(report.total_groups, 1);
    assert_eq!(report.orphan_count(), 1);

    let audit = audit_invocation_pairing(root.path()).expect("audit");
    assert_eq!(audit, report);
}

#[test]
fn the_ops_audit_uses_the_supplied_store_layout() {
    let root = tempfile::tempdir().expect("tempdir");
    let ops_dir = root.path().join("kitty-ops");
    write(&ops_dir.join("01OPEN.jsonl"), r#"{"event":"started"}"#);
    let closed: BTreeSet<String> = BTreeSet::new();
    let non_record = [
        "ops-index.jsonl",
        "lifecycle.jsonl",
        "propagation-errors.jsonl",
    ];
    let audit = audit_open_ops(&OpStoreLayout::new(
        &ops_dir,
        "op-closures.jsonl",
        &non_record,
        &closed,
    ))
    .expect("audit");
    assert_eq!(audit.open_ops, vec![ops_dir.join("01OPEN.jsonl")]);
    assert_eq!(
        audit.to_json(root.path()),
        json!([{"path": "kitty-ops/01OPEN.jsonl"}])
    );
}
