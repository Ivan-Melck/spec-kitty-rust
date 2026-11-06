//! CF17 CLI smoke: `doctor decisions` now reports the REAL two-surface fork
//! facts, not an unevaluated disclosure.
//!
//! Asserted here against the actual `spec-kitty` binary and real Git fixtures:
//! the JSON payload carries `forked` / `streams` / `ledger` / `reconcile_steps`
//! and `findings` computed from those facts, `unevaluated_findings` is GONE from
//! the fork shape, the read-only run exits 0 even when forked, `--repair` still
//! refuses before any read/lock/write, and the human surface prints Source's
//! fork and reconcile lines.
//!
//! Not claimed: byte-level parity with Source's Rich-styled human output, a
//! whole-doctor sweep, or `--repair` behaviour (still an explicit refusal).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

const MISSION_ID: &str = "01KN2371WNZX28ME4MN97B67PJ";
const SLUG: &str = "001-x";
const MID8: &str = "01KN2371";
const COORD_BRANCH: &str = "kitty/mission-001-x";

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
        .env("GIT_AUTHOR_NAME", "CF17 CLI Fixture")
        .env("GIT_AUTHOR_EMAIL", "cf17@example.invalid")
        .env("GIT_COMMITTER_NAME", "CF17 CLI Fixture")
        .env("GIT_COMMITTER_EMAIL", "cf17@example.invalid")
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

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
        Self { root, home }
    }

    fn path(&self) -> &Path {
        self.root.path()
    }

    fn mission_root(&self) -> PathBuf {
        self.path().join("kitty-specs").join(SLUG)
    }

    fn meta(&self) {
        write(
            &self.mission_root().join("meta.json"),
            &format!(
                "{{\"mission_id\":\"{MISSION_ID}\",\"mission_slug\":\"{SLUG}\",\"mid8\":\"{MID8}\",\
                 \"topology\":\"coord\",\"target_branch\":\"main\",\
                 \"coordination_branch\":\"{COORD_BRANCH}\"}}"
            ),
        );
    }

    fn commit(&self, message: &str) {
        git(self.path(), self.home.path(), &["add", "-A"]);
        git(
            self.path(),
            self.home.path(),
            &["commit", "--quiet", "-m", message],
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

fn opened(event_id: &str, decision_id: &str) -> String {
    format!(
        r#"{{"event_id":"{event_id}","event_type":"DecisionPointOpened","payload":{{"decision_point_id":"{decision_id}","origin_flow":"specify","step_id":"s","input_key":"k","question":"q","options":[],"state_entered_at":"2026-01-01T00:00:00+00:00","actor_id":"claude","mission_id":"{MISSION_ID}","mission_slug":"{SLUG}"}}}}"#
    )
}

fn resolved(event_id: &str, decision_id: &str) -> String {
    format!(
        r#"{{"event_id":"{event_id}","event_type":"DecisionPointResolved","payload":{{"decision_point_id":"{decision_id}","terminal_outcome":"resolved","state_entered_at":"2026-01-02T00:00:00+00:00","resolved_by":"operator","final_answer":"yes"}}}}"#
    )
}

fn index_doc(entry_ids: &[&str]) -> String {
    let entries: Vec<String> = entry_ids
        .iter()
        .map(|id| {
            format!(
                r#"{{"decision_id":"{id}","origin_flow":"specify","step_id":"s","slot_key":null,"input_key":"k","question":"q","options":[],"status":"open","final_answer":null,"rationale":null,"other_answer":false,"summary_json":null,"created_at":"2026-01-01T00:00:00Z","resolved_at":null,"resolved_by":null,"opened_by":"claude","mission_id":"{MISSION_ID}","mission_slug":"{SLUG}"}}"#
            )
        })
        .collect();
    format!(
        "{{\"version\":1,\"mission_id\":\"{MISSION_ID}\",\"entries\":[{}]}}\n",
        entries.join(",")
    )
}

/// A forked Mission: PRIMARY and COORDINATION both carry diverging status
/// streams, and the coordination branch holds the only ledger entries for `d2`.
fn forked_fixture() -> Fixture {
    let fixture = Fixture::new();
    fixture.meta();
    write(
        &fixture.mission_root().join("status.events.jsonl"),
        &format!("{}\n", opened("e1", "d1")),
    );
    write(
        &fixture.mission_root().join("decisions/index.json"),
        &index_doc(&["d1"]),
    );
    fixture.commit("primary base");

    let coord_mission_dir = fixture
        .path()
        .join("kitty-specs")
        .join(format!("{SLUG}-{MID8}"));
    git(
        fixture.path(),
        fixture.home.path(),
        &["checkout", "--quiet", "-b", COORD_BRANCH],
    );
    write(
        &coord_mission_dir.join("status.events.jsonl"),
        &format!("{}\n{}\n", opened("e1", "d1"), resolved("e2", "d2")),
    );
    write(
        &coord_mission_dir.join("decisions/index.json"),
        &index_doc(&["d1", "d2"]),
    );
    git(fixture.path(), fixture.home.path(), &["add", "-A"]);
    git(
        fixture.path(),
        fixture.home.path(),
        &["commit", "--quiet", "-m", "coord"],
    );
    git(
        fixture.path(),
        fixture.home.path(),
        &["checkout", "--quiet", "main"],
    );

    write(
        &fixture.mission_root().join("status.events.jsonl"),
        &format!("{}\n{}\n", opened("e1", "d1"), resolved("e3", "d3")),
    );
    // Exercise the second stream's single-home branch with a real PRIMARY row;
    // without this file both surfaces are absent, not single_home.
    write(
        &fixture.mission_root().join("decisions.events.jsonl"),
        "{\"event_id\":\"decision-only-e1\",\"payload\":{\"decision_id\":\"d1\"}}\n",
    );
    fixture.commit("primary fork");
    fixture
}

#[test]
fn forked_json_carries_real_fork_facts_and_no_unevaluated_disclosure() {
    let fixture = forked_fixture();
    let output = fixture.run(&["doctor", "decisions", "--mission", SLUG, "--json"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "read-only decisions is report-only; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload = json_of(&output);

    // The disclosure is replaced by facts.
    assert!(
        payload.get("unevaluated_findings").is_none(),
        "the fork shape must not carry the unevaluated disclosure: {payload}"
    );
    assert_eq!(payload["forked"], Value::Bool(true));
    assert_eq!(payload["clean"], Value::Bool(false));
    assert_eq!(payload["repaired"], Value::Bool(false));
    assert_eq!(payload["ledger_copied"], Value::Bool(false));

    let findings = payload["findings"].as_array().expect("findings array");
    let findings: Vec<&str> = findings
        .iter()
        .map(|value| value.as_str().expect("finding code"))
        .collect();
    assert!(findings.contains(&"DECISION_LOG_FORKED"), "{findings:?}");
    assert!(
        findings.contains(&"DECISION_LEDGER_ONLY_ON_COORDINATION"),
        "{findings:?}"
    );

    let streams = payload["streams"].as_array().expect("streams array");
    assert_eq!(streams.len(), 2);
    let status = &streams[0];
    assert_eq!(status["stream"], "status.events.jsonl");
    assert_eq!(status["state"], "forked");
    assert_eq!(status["primary"]["source"], "worktree");
    assert_eq!(status["primary"]["event_count"], 2);
    assert_eq!(status["coordination"]["source"], "ref");
    assert_eq!(status["coordination"]["event_count"], 2);
    assert_eq!(
        status["decisions_only_on_primary"],
        serde_json::json!(["d3"])
    );
    assert_eq!(
        status["decisions_only_on_coordination"],
        serde_json::json!(["d2"])
    );
    assert_eq!(status["decisions_on_both"], serde_json::json!(["d1"]));
    assert_eq!(streams[1]["stream"], "decisions.events.jsonl");
    assert_eq!(streams[1]["state"], "single_home");
    assert_eq!(streams[1]["primary"]["event_count"], 1);
    assert_eq!(
        streams[1]["decisions_only_on_primary"],
        serde_json::json!(["d1"])
    );

    assert_eq!(payload["ledger"]["state"], "both");
    assert_eq!(
        payload["ledger"]["entries_only_on_coordination"],
        serde_json::json!(["d2"])
    );
    let steps = payload["reconcile_steps"]
        .as_array()
        .expect("reconcile steps");
    let text = steps
        .iter()
        .map(|value| value.as_str().expect("step"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("never auto-merged (C-003)"), "{text}");
    assert!(text.contains("--mission 001-x --repair"), "{text}");

    // Source's per-field facts are unchanged by the fork shape.
    assert_eq!(payload["missing_from_index"], serde_json::json!([]));
    assert_eq!(payload["orphaned_in_index"], serde_json::json!([]));
    assert_eq!(payload["malformed_folds"], serde_json::json!([]));
    assert_eq!(payload["status_mismatch"], serde_json::json!([]));
}

#[test]
fn the_human_surface_prints_the_fork_and_reconcile_lines() {
    let fixture = forked_fixture();
    let output = fixture.run(&["doctor", "decisions", "--mission", SLUG]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).expect("UTF-8");
    assert!(stdout.contains("status.events.jsonl: forked"), "{stdout}");
    assert!(
        stdout.contains("primary=2 event(s), coordination=2 event(s)"),
        "{stdout}"
    );
    assert!(
        stdout.contains("decisions ledger: only on the coordination branch"),
        "{stdout}"
    );
    assert!(
        stdout.contains("reconcile: status.events.jsonl is forked"),
        "{stdout}"
    );
}

#[test]
fn repair_is_still_refused_before_any_read_or_write() {
    let fixture = forked_fixture();
    let before = fs::read(fixture.mission_root().join("decisions/index.json")).expect("index");
    let output = fixture.run(&[
        "doctor",
        "decisions",
        "--mission",
        SLUG,
        "--repair",
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        text.contains("UNSUPPORTED_DOCTOR_DECISIONS_REPAIR"),
        "{text}"
    );
    assert!(text.contains("--repair"), "{text}");
    let after = fs::read(fixture.mission_root().join("decisions/index.json")).expect("index");
    assert_eq!(before, after, "--repair must not write anything");
}

#[test]
fn a_clean_mission_reports_the_fork_facts_too() {
    let fixture = Fixture::new();
    fixture.meta();
    write(
        &fixture.mission_root().join("status.events.jsonl"),
        &format!("{}\n", opened("e1", "d1")),
    );
    write(
        &fixture.mission_root().join("decisions/index.json"),
        &index_doc(&["d1"]),
    );
    fixture.commit("clean");

    let output = fixture.run(&["doctor", "decisions", "--mission", SLUG, "--json"]);
    assert_eq!(output.status.code(), Some(0));
    let payload = json_of(&output);
    assert_eq!(payload["clean"], Value::Bool(true));
    assert_eq!(payload["forked"], Value::Bool(false));
    assert_eq!(payload["findings"], serde_json::json!([]));
    assert!(payload.get("unevaluated_findings").is_none());
    assert_eq!(payload["streams"][0]["state"], "single_home");
    assert_eq!(payload["streams"][1]["state"], "absent");
    assert_eq!(payload["ledger"]["state"], "primary");
    assert_eq!(payload["reconcile_steps"], serde_json::json!([]));
}
