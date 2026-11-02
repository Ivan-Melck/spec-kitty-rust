//! Fail-loud boundaries: corrupt diary rows never reduce to a silently wrong state.

mod replay_support;

use serde_json::{Value, json};
use sk_status::replay::parse_diary;
use sk_status::snapshot::{ReplayInput, materialize, read_raw_events};

use replay_support::{read_json, replay_root};

fn temp_dir(label: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path =
        std::env::temp_dir().join(format!("sk-status-{}-{label}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&path).expect("temp dir");
    path
}

fn error_message(rows: &[Value]) -> String {
    parse_diary(rows)
        .expect_err("row must be refused")
        .to_string()
}

fn transition(extra: Value) -> Value {
    let mut row = json!({
        "event_id": "01J00000000000000000000001",
        "mission_slug": "900-errors",
        "wp_id": "WP01",
        "from_lane": "planned",
        "to_lane": "claimed",
        "at": "2026-01-01T00:00:01Z",
        "actor": "agent",
        "force": false,
        "execution_mode": "worktree"
    });
    if let (Some(target), Some(extra)) = (row.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            target.insert(key.clone(), value.clone());
        }
    }
    row
}

#[test]
fn non_object_rows_are_refused() {
    let message = error_message(&[json!([1, 2, 3])]);
    assert!(
        message.contains("expected JSON object"),
        "message was {message}"
    );
}

#[test]
fn unknown_kind_is_refused_rather_than_skipped() {
    let message = error_message(&[json!({"kind": "future-change", "wp_id": "WP01"})]);
    assert!(message.contains("future-change"), "message was {message}");
    assert!(message.contains("kind"), "message was {message}");
}

#[test]
fn malformed_lane_rows_are_refused() {
    assert!(error_message(&[json!({"event_id": "x", "wp_id": "WP01"})]).contains("from_lane"));

    let message = error_message(&[transition(json!({"from_lane": "doing"}))]);
    assert!(message.contains("doing"), "message was {message}");

    let message = error_message(&[transition(json!({"actor": {"role": "implementer"}}))]);
    assert!(message.contains("actor"), "message was {message}");

    let message = error_message(&[transition(json!({"evidence": {"review": {}}}))]);
    assert!(message.contains("review"), "message was {message}");

    let message = error_message(&[transition(
        json!({"review_result": {"reviewer": "r", "verdict": "approved"}}),
    )]);
    assert!(message.contains("reference"), "message was {message}");
}

#[test]
fn malformed_annotations_are_refused() {
    let message = error_message(&[json!({
        "event_id": "not-a-ulid",
        "kind": "annotation",
        "wp_id": "WP01",
        "at": "t",
        "actor": "a",
        "delta": {"note": "x"}
    })]);
    assert!(
        message.contains("ULID") || message.contains("ulid"),
        "message was {message}"
    );

    let message = error_message(&[json!({
        "event_id": "01J00000000000000000000001",
        "kind": "annotation",
        "wp_id": "WP01",
        "at": "t",
        "actor": "a",
        "delta": {"subtasks": {"T001": "invented"}}
    })]);
    assert!(
        message.contains("invented") || message.contains("subtask"),
        "message was {message}"
    );

    let message = error_message(&[json!({
        "event_id": "01J00000000000000000000001",
        "kind": "annotation",
        "wp_id": "WP01",
        "at": "t",
        "actor": "a"
    })]);
    assert!(message.contains("delta"), "message was {message}");
}

#[test]
fn corrupt_json_in_a_feature_file_is_a_line_error() {
    let temp = temp_dir("corrupt-json");
    let path = temp.join("status.events.jsonl");
    std::fs::write(&path, b"{\"ok\": true}\n{oops\n").expect("write");
    let error = read_raw_events(&path).expect_err("invalid JSON must refuse");
    assert!(error.to_string().contains("line 2"), "error was {error}");
    std::fs::remove_dir_all(temp).expect("cleanup");
}

#[test]
fn a_full_feature_directory_fails_loud_on_a_corrupt_row() {
    let temp = temp_dir("corrupt-row");
    let dir = temp.join("900-corrupt");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(
        dir.join("status.events.jsonl"),
        b"{\"kind\": \"annotation\", \"wp_id\": \"WP01\"}\n",
    )
    .expect("write");
    let input = ReplayInput::read_dir(&dir).expect("read input");
    assert!(materialize(&input).is_err());
    std::fs::remove_dir_all(temp).expect("cleanup");
}

#[test]
fn non_lane_rows_are_ignored_in_place_and_never_become_transitions() {
    let rows = [
        json!({"event_type": "DecisionPointOpened", "payload": {}}),
        json!({"event_type": null, "payload": {}}),
        json!({"event_name": "retrospective.completed", "payload": {}}),
        json!({"type": "RetrospectiveCaptured", "payload": {}}),
    ];
    let stream = parse_diary(&rows).expect("non-lane rows are ignored");
    assert!(stream.transitions.is_empty());
    assert!(stream.annotations.is_empty());
}

#[test]
fn an_archive_fixture_parses_without_python() {
    let dir = replay_root().join("archives/053-orchestrator-api-json-contract-fidelity");
    let input = ReplayInput::read_dir(&dir).expect("read archive");
    let snapshot = materialize(&input).expect("materialize archive");
    assert!(snapshot.event_count > 0);
    let _ = read_json(&dir.join("expected.json"));
}
