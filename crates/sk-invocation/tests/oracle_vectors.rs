//! Live, lock-exact oracle vectors for the Op record wire shape.
//!
//! The authority is the pinned Python oracle (`.venv-rust-oracle`, verified by
//! `scripts/rust/verify_oracle.py`): every case below is applied to the real
//! `record.py` models and to this crate, and the outcomes are compared. The
//! oracle is required, never skipped, so a missing or drifted reference fails
//! the test instead of yielding an empty comparison.
//!
//! Covered controls: the contract examples, every optional field (present,
//! absent, empty), non-ASCII (including a supplementary-plane character and
//! ASCII controls), the free-text fields Pydantic does *not* validate, the
//! legacy shapes, the not-a-lifecycle shapes, and the id boundary edges
//! (26 `Z`s, lowercase, `I`/`L`/`O`/`U`, 25/27 characters, trailing newline
//! and space).

mod support;

use std::collections::BTreeMap;
use std::fs;

use serde_json::{Value, json};
use sk_invocation::{
    CorrelationLink, InvocationId, InvocationWriter, OpEvent, RecordParseError, append_op_closure,
    closed_invocation_ids, parse_op_event,
};

use support::{completed, python, run_python, started, started_with};

const LINE_SCRIPT: &str = r#"
import json, sys
from specify_cli.invocation.record import OpCompletedEvent, OpStartedEvent

payload = json.load(sys.stdin)
lines = {}
for case in payload["cases"]:
    fields = case["fields"]
    if case["kind"] == "started":
        lines[case["name"]] = OpStartedEvent.model_validate(fields).to_jsonl_line()
    else:
        lines[case["name"]] = OpCompletedEvent.model_validate(fields).to_jsonl_line()
json.dump(lines, sys.stdout)
"#;

const VALIDATION_SCRIPT: &str = r#"
import json, sys
from specify_cli.invocation.errors import LegacyRecordError
from specify_cli.invocation.record import parse_op_event

payload = json.load(sys.stdin)
out = {}
for case in payload["cases"]:
    try:
        event = parse_op_event(case["value"])
        out[case["name"]] = "ok:" + type(event).__name__
    except LegacyRecordError:
        out[case["name"]] = "legacy"
    except ValueError:
        out[case["name"]] = "value_error"
    except Exception as exc:
        out[case["name"]] = "other:" + type(exc).__name__
json.dump(out, sys.stdout)
"#;

fn started_case(name: &str, fields: Value) -> Value {
    json!({"name": name, "kind": "started", "fields": fields})
}

fn completed_case(name: &str, fields: Value) -> Value {
    json!({"name": name, "kind": "completed", "fields": fields})
}

fn base_started() -> Value {
    json!({
        "invocation_id": "01KTK5JBD69FQ8XVRFV1J630MJ",
        "profile_id": "implementer-iris",
        "action": "implement",
        "request_text": "fix that bug",
        "actor": "claude",
        "mode_of_work": "task_execution",
        "governance_context_hash": "d5ccab5678dcc4c8",
        "governance_context_available": true,
        "router_confidence": "canonical_verb",
        "started_at": "2026-06-10T20:00:00+00:00",
    })
}

fn merged(base: Value, overrides: Value) -> Value {
    let mut base = base;
    let (Some(target), Value::Object(overrides)) = (base.as_object_mut(), overrides) else {
        panic!("expected a JSON object");
    };
    for (key, value) in overrides {
        target.insert(key, value);
    }
    base
}

fn line_cases() -> Vec<Value> {
    let mut cases = vec![
        started_case("contract_started", base_started()),
        started_case(
            "all_optionals",
            merged(
                base_started(),
                json!({"mission_id": "01KTK5JBD69FQ8XVRFV1J630MJ", "wp_id": "WP01", "model_id": "claude-opus-4-6"}),
            ),
        ),
        started_case(
            "no_optionals",
            merged(
                base_started(),
                json!({"router_confidence": null, "mission_id": null, "wp_id": null, "model_id": null}),
            ),
        ),
        started_case(
            "empty_request_query",
            merged(
                base_started(),
                json!({"request_text": "", "mode_of_work": "query"}),
            ),
        ),
        started_case(
            "unicode",
            merged(
                base_started(),
                json!({
                    "profile_id": "implémenter-é",
                    "request_text": "héllo ☃ 𝄞 \u{1} \u{7f} \"q\" \\ tab\tnl\nend",
                    "mission_id": "mïssion-𝄞",
                    "wp_id": "WP-é",
                    "model_id": "modèle-☃",
                }),
            ),
        ),
        started_case(
            "ascii_printable_mix",
            merged(
                base_started(),
                json!({"request_text": "/~!@#$%^&*()_+={}[]|;:,.<>?`'\"\\ ", "mission_id": "https://example.com/m", "router_confidence": "canonical_verb | exact"}),
            ),
        ),
        started_case(
            "ascii_controls",
            merged(
                base_started(),
                json!({"request_text": "\u{0}\u{8}\u{b}\u{c}\r\u{1f}"}),
            ),
        ),
        started_case(
            "id_twenty_six_zs",
            merged(base_started(), json!({"invocation_id": "Z".repeat(26)})),
        ),
        started_case(
            "id_zeros",
            merged(base_started(), json!({"invocation_id": "0".repeat(26)})),
        ),
        completed_case(
            "contract_completed",
            json!({
                "invocation_id": "01KTK5JBD69FQ8XVRFV1J630MJ",
                "completed_at": "2026-06-10T20:25:00+00:00",
                "outcome": "done",
                "closed_by": "agent",
                "evidence_ref": ".kittify/evidence/01KTK5JBD69FQ8XVRFV1J630MJ",
            }),
        ),
        completed_case(
            "completed_minimal",
            json!({
                "invocation_id": "01KTK5JBD69FQ8XVRFV1J630MJ",
                "completed_at": "2026-06-10T20:25:00+00:00",
                "outcome": "done",
                "closed_by": "agent",
                "evidence_ref": null,
            }),
        ),
        completed_case(
            "completed_doctor_sweep_abandoned",
            json!({
                "invocation_id": "01KTK5JBD69FQ8XVRFV1J630MJ",
                "completed_at": "2026-06-10T21:00:00+00:00",
                "outcome": "abandoned",
                "closed_by": "doctor_sweep",
            }),
        ),
        completed_case(
            "completed_empty_evidence",
            json!({
                "invocation_id": "01KTK5JBD69FQ8XVRFV1J630MJ",
                "completed_at": "2026-06-10T21:00:00+00:00",
                "outcome": "failed",
                "closed_by": "agent",
                "evidence_ref": "",
            }),
        ),
    ];
    cases.push(started_case(
        "extra_keys_ignored",
        merged(base_started(), json!({"extra_field": "ignored"})),
    ));
    cases
}

fn native_lines(cases: &[Value]) -> BTreeMap<String, String> {
    let mut lines = BTreeMap::new();
    for case in cases {
        let name = case["name"].as_str().unwrap().to_owned();
        let fields = case["fields"].as_object().unwrap();
        let line = match case["kind"].as_str().unwrap() {
            "started" => sk_invocation::OpStartedEvent::from_object(fields)
                .expect("native started validation")
                .to_jsonl_line(),
            "completed" => sk_invocation::OpCompletedEvent::from_object(fields)
                .expect("native completed validation")
                .to_jsonl_line(),
            other => panic!("unknown case kind {other}"),
        };
        lines.insert(name, line);
    }
    lines
}

#[test]
fn record_bytes_match_the_lock_exact_oracle() {
    let cases = line_cases();
    let request = json!({"cases": cases}).to_string();
    let oracle: BTreeMap<String, String> =
        serde_json::from_str(&run_python(LINE_SCRIPT, Some(&request)))
            .expect("oracle emitted a JSON object of lines");
    let native = native_lines(&cases);

    assert_eq!(
        oracle.len(),
        cases.len(),
        "every case must produce an oracle line"
    );
    assert_eq!(
        native.keys().collect::<Vec<_>>(),
        oracle.keys().collect::<Vec<_>>()
    );
    for (name, expected) in &oracle {
        assert_eq!(
            native.get(name),
            Some(expected),
            "byte mismatch for case {name}\nnative:   {:?}\noracle:   {expected:?}",
            native.get(name)
        );
    }

    // Pin the oracle's own answer for the contract example, so a drift in the
    // compared quantities cannot hide behind two identical mistakes.
    assert_eq!(
        oracle.get("contract_started").map(String::as_str),
        Some(
            r#"{"event": "started", "invocation_id": "01KTK5JBD69FQ8XVRFV1J630MJ", "profile_id": "implementer-iris", "action": "implement", "request_text": "fix that bug", "actor": "claude", "mode_of_work": "task_execution", "governance_context_hash": "d5ccab5678dcc4c8", "governance_context_available": true, "router_confidence": "canonical_verb", "started_at": "2026-06-10T20:00:00+00:00"}"#
        )
    );
    assert!(
        oracle
            .get("unicode")
            .expect("unicode case")
            .contains(r"\ud834\udd1e"),
        "the oracle must escape the supplementary-plane character as a surrogate pair"
    );
}

fn validation_cases() -> Vec<Value> {
    let mut cases = vec![
        (
            "valid_started",
            json!({"event": "started", "invocation_id": "01KTK5JBD69FQ8XVRFV1J630MJ", "profile_id": "p", "action": "implement", "request_text": "r", "actor": "claude", "mode_of_work": "task_execution", "governance_context_hash": "h", "governance_context_available": true, "started_at": "s"}),
        ),
        (
            "valid_completed",
            json!({"event": "completed", "invocation_id": "01KTK5JBD69FQ8XVRFV1J630MJ", "completed_at": "c", "outcome": "done", "closed_by": "agent"}),
        ),
        (
            "started_extra_key",
            json!({"event": "started", "invocation_id": "01KTK5JBD69FQ8XVRFV1J630MJ", "profile_id": "p", "action": "implement", "request_text": "r", "actor": "claude", "mode_of_work": "advisory", "governance_context_hash": "", "governance_context_available": "yes", "router_confidence": "", "started_at": "not-a-time", "mission_id": "", "wp_id": "", "unknown": 1}),
        ),
        (
            "started_missing_action",
            json!({"event": "started", "invocation_id": "01KTK5JBD69FQ8XVRFV1J630MJ", "profile_id": "p", "request_text": "r", "actor": "claude", "mode_of_work": "task_execution", "governance_context_hash": "h", "governance_context_available": true, "started_at": "s"}),
        ),
        (
            "started_empty_action",
            json!({"event": "started", "invocation_id": "01KTK5JBD69FQ8XVRFV1J630MJ", "profile_id": "p", "action": "", "request_text": "r", "actor": "claude", "mode_of_work": "task_execution", "governance_context_hash": "h", "governance_context_available": true, "started_at": "s"}),
        ),
        (
            "started_int_profile",
            json!({"event": "started", "invocation_id": "01KTK5JBD69FQ8XVRFV1J630MJ", "profile_id": 5, "action": "implement", "request_text": "r", "actor": "claude", "mode_of_work": "task_execution", "governance_context_hash": "h", "governance_context_available": true, "started_at": "s"}),
        ),
        (
            "started_null_request",
            json!({"event": "started", "invocation_id": "01KTK5JBD69FQ8XVRFV1J630MJ", "profile_id": "p", "action": "implement", "request_text": null, "actor": "claude", "mode_of_work": "task_execution", "governance_context_hash": "h", "governance_context_available": true, "started_at": "s"}),
        ),
        (
            "started_upper_mode",
            json!({"event": "started", "invocation_id": "01KTK5JBD69FQ8XVRFV1J630MJ", "profile_id": "p", "action": "implement", "request_text": "r", "actor": "claude", "mode_of_work": "TASK_EXECUTION", "governance_context_hash": "h", "governance_context_available": true, "started_at": "s"}),
        ),
        (
            "started_bogus_mode",
            json!({"event": "started", "invocation_id": "01KTK5JBD69FQ8XVRFV1J630MJ", "profile_id": "p", "action": "implement", "request_text": "r", "actor": "claude", "mode_of_work": "bogus", "governance_context_hash": "h", "governance_context_available": true, "started_at": "s"}),
        ),
        (
            "started_empty_model_id",
            json!({"event": "started", "invocation_id": "01KTK5JBD69FQ8XVRFV1J630MJ", "profile_id": "p", "action": "implement", "request_text": "r", "actor": "claude", "mode_of_work": "task_execution", "governance_context_hash": "h", "governance_context_available": true, "started_at": "s", "model_id": ""}),
        ),
        (
            "started_bogus_bool",
            json!({"event": "started", "invocation_id": "01KTK5JBD69FQ8XVRFV1J630MJ", "profile_id": "p", "action": "implement", "request_text": "r", "actor": "claude", "mode_of_work": "task_execution", "governance_context_hash": "h", "governance_context_available": 2, "started_at": "s"}),
        ),
        (
            "completed_missing_closed_by",
            json!({"event": "completed", "invocation_id": "01KTK5JBD69FQ8XVRFV1J630MJ", "completed_at": "c", "outcome": "done"}),
        ),
        (
            "completed_bad_closed_by",
            json!({"event": "completed", "invocation_id": "01KTK5JBD69FQ8XVRFV1J630MJ", "completed_at": "c", "outcome": "done", "closed_by": "elf"}),
        ),
        (
            "completed_null_outcome",
            json!({"event": "completed", "invocation_id": "01KTK5JBD69FQ8XVRFV1J630MJ", "completed_at": "c", "outcome": null, "closed_by": "agent"}),
        ),
        (
            "completed_missing_outcome",
            json!({"event": "completed", "invocation_id": "01KTK5JBD69FQ8XVRFV1J630MJ", "completed_at": "c", "closed_by": "agent"}),
        ),
        (
            "completed_upper_outcome",
            json!({"event": "completed", "invocation_id": "01KTK5JBD69FQ8XVRFV1J630MJ", "completed_at": "c", "outcome": "DONE", "closed_by": "agent"}),
        ),
        (
            "completed_empty_timestamp",
            json!({"event": "completed", "invocation_id": "01KTK5JBD69FQ8XVRFV1J630MJ", "completed_at": "", "outcome": "done", "closed_by": "agent"}),
        ),
        (
            "completed_empty_evidence",
            json!({"event": "completed", "invocation_id": "01KTK5JBD69FQ8XVRFV1J630MJ", "completed_at": "c", "outcome": "failed", "closed_by": "doctor_sweep", "evidence_ref": ""}),
        ),
        (
            "non_lifecycle_link",
            json!({"event": "artifact_link", "invocation_id": "01KTK5JBD69FQ8XVRFV1J630MJ"}),
        ),
        (
            "missing_event",
            json!({"invocation_id": "01KTK5JBD69FQ8XVRFV1J630MJ"}),
        ),
        ("event_number", json!({"event": 5})),
        ("event_null", json!({"event": null})),
        (
            "id_lowercase",
            json!({"event": "started", "invocation_id": "abcdefghjkmnpqrstvwxyz12345", "profile_id": "p", "action": "implement", "request_text": "r", "actor": "claude", "mode_of_work": "task_execution", "governance_context_hash": "h", "governance_context_available": true, "started_at": "s"}),
        ),
        (
            "id_contains_i",
            json!({"event": "started", "invocation_id": "01KTK5JBD69FQ8XVRFV1J630MI", "profile_id": "p", "action": "implement", "request_text": "r", "actor": "claude", "mode_of_work": "task_execution", "governance_context_hash": "h", "governance_context_available": true, "started_at": "s"}),
        ),
        (
            "id_contains_u",
            json!({"event": "started", "invocation_id": "01KTK5JBD69FQ8XVRFV1J630MU", "profile_id": "p", "action": "implement", "request_text": "r", "actor": "claude", "mode_of_work": "task_execution", "governance_context_hash": "h", "governance_context_available": true, "started_at": "s"}),
        ),
        (
            "id_25_chars",
            json!({"event": "started", "invocation_id": "Z".repeat(25), "profile_id": "p", "action": "implement", "request_text": "r", "actor": "claude", "mode_of_work": "task_execution", "governance_context_hash": "h", "governance_context_available": true, "started_at": "s"}),
        ),
        (
            "id_27_chars",
            json!({"event": "started", "invocation_id": "Z".repeat(27), "profile_id": "p", "action": "implement", "request_text": "r", "actor": "claude", "mode_of_work": "task_execution", "governance_context_hash": "h", "governance_context_available": true, "started_at": "s"}),
        ),
        (
            "id_trailing_newline",
            json!({"event": "started", "invocation_id": format!("{}\n", "Z".repeat(25)), "profile_id": "p", "action": "implement", "request_text": "r", "actor": "claude", "mode_of_work": "task_execution", "governance_context_hash": "h", "governance_context_available": true, "started_at": "s"}),
        ),
        (
            "id_trailing_space",
            json!({"event": "started", "invocation_id": format!("{} ", "Z".repeat(25)), "profile_id": "p", "action": "implement", "request_text": "r", "actor": "claude", "mode_of_work": "task_execution", "governance_context_hash": "h", "governance_context_available": true, "started_at": "s"}),
        ),
    ];
    cases.push(("id_26_zs", json!({"event": "started", "invocation_id": "Z".repeat(26), "profile_id": "p", "action": "implement", "request_text": "r", "actor": "claude", "mode_of_work": "task_execution", "governance_context_hash": "h", "governance_context_available": true, "started_at": "s"})));
    cases
        .into_iter()
        .map(|(name, value)| json!({"name": name, "value": value}))
        .collect()
}

fn native_outcome(value: &Value) -> String {
    match parse_op_event(value) {
        Ok(OpEvent::Started(_)) => "ok:OpStartedEvent".to_owned(),
        Ok(OpEvent::Completed(_)) => "ok:OpCompletedEvent".to_owned(),
        Err(RecordParseError::Legacy(_)) => "legacy".to_owned(),
        Err(RecordParseError::NotLifecycle(_)) => "value_error".to_owned(),
    }
}

#[test]
fn record_validation_outcomes_match_the_lock_exact_oracle() {
    let cases = validation_cases();
    let request = json!({"cases": cases}).to_string();
    let oracle: BTreeMap<String, String> =
        serde_json::from_str(&run_python(VALIDATION_SCRIPT, Some(&request)))
            .expect("oracle emitted a JSON object of outcomes");

    assert_eq!(oracle.len(), cases.len());
    let mut legacy = 0;
    let mut accepted = 0;
    let mut rejected = 0;
    for case in &cases {
        let name = case["name"].as_str().unwrap();
        let expected = oracle
            .get(name)
            .unwrap_or_else(|| panic!("no oracle outcome for {name}"));
        let native = native_outcome(&case["value"]);
        assert_eq!(&native, expected, "outcome mismatch for case {name}");
        match expected.as_str() {
            value if value.starts_with("ok:") => accepted += 1,
            "legacy" => legacy += 1,
            _ => rejected += 1,
        }
    }
    assert!(
        accepted >= 5,
        "the matrix must accept valid shapes: {accepted}"
    );
    assert!(
        legacy >= 10,
        "the matrix must exercise legacy shapes: {legacy}"
    );
    assert!(
        rejected >= 3,
        "the matrix must exercise not-lifecycle shapes: {rejected}"
    );
}

#[test]
fn writer_archive_bytes_match_the_lock_exact_oracle() {
    let id = "01KPQRX2EVGMRVB4Q1JQBAZJV3";
    let record = started(id);
    let temp = tempfile::tempdir().expect("isolated project");
    let writer = InvocationWriter::new(temp.path());
    let started_path = writer.write_started(&record).expect("started write");
    writer
        .append_correlation_link(
            record.invocation_id(),
            CorrelationLink::artifact("spec.md").at("2026-06-10T21:00:00+00:00"),
        )
        .expect("artifact link");
    writer
        .append_correlation_link(
            record.invocation_id(),
            CorrelationLink::commit("abc123").at("2026-06-10T21:00:01+00:00"),
        )
        .expect("commit link");
    let completed_record = completed(id);
    writer
        .write_completed(&completed_record)
        .expect("completed write");
    append_op_closure(temp.path(), &completed_record).expect("closure write");

    // The reference writer builds these dicts in source order and renders
    // them with `json.dumps`; reproducing that here (rather than round-tripping
    // a serde map, which sorts keys) keeps the comparison honest.
    let script = r#"
import json, sys
payload = json.load(sys.stdin)
invocation_id = payload["invocation_id"]
out = {}
out["index"] = json.dumps({
    "invocation_id": invocation_id,
    "profile_id": payload["profile_id"],
    "started_at": payload["started_at"],
})
out["artifact_link"] = json.dumps({
    "event": "artifact_link",
    "invocation_id": invocation_id,
    "at": "2026-06-10T21:00:00+00:00",
    "kind": "artifact",
    "ref": "spec.md",
})
out["commit_link"] = json.dumps({
    "event": "commit_link",
    "invocation_id": invocation_id,
    "at": "2026-06-10T21:00:01+00:00",
    "sha": "abc123",
})
json.dump(out, sys.stdout)
"#;
    let request = json!({
        "invocation_id": id,
        "profile_id": record.profile_id(),
        "started_at": record.started_at(),
    });
    let oracle: BTreeMap<String, String> =
        serde_json::from_str(&run_python(script, Some(&request.to_string())))
            .expect("oracle emitted rendered dicts");

    let index_line = std::fs::read_to_string(temp.path().join("kitty-ops/ops-index.jsonl"))
        .expect("index exists");
    assert_eq!(
        index_line.lines().next(),
        oracle.get("index").map(String::as_str)
    );

    let trail = std::fs::read_to_string(&started_path.path).expect("trail exists");
    let trail_lines: Vec<&str> = trail.lines().collect();
    assert_eq!(
        trail_lines.first().copied(),
        Some(record.to_jsonl_line().as_str())
    );
    assert_eq!(
        trail_lines.get(1).copied(),
        oracle.get("artifact_link").map(String::as_str)
    );
    assert_eq!(
        trail_lines.get(2).copied(),
        oracle.get("commit_link").map(String::as_str)
    );
    assert_eq!(
        trail_lines.get(3).copied(),
        Some(completed_record.to_jsonl_line().as_str())
    );

    let spine = std::fs::read_to_string(temp.path().join("kitty-ops/op-closures.jsonl"))
        .expect("spine exists");
    assert_eq!(
        spine.lines().next(),
        Some(completed_record.to_jsonl_line().as_str())
    );
    assert_eq!(
        closed_invocation_ids(temp.path()).expect("closed ids"),
        [completed_record.invocation_id().clone()]
            .into_iter()
            .collect()
    );

    // Non-vacuity: the oracle must actually be in the loop.
    assert!(python().is_file());
}

const LARGE_RECORD_SCRIPT: &str = r#"
import json, sys
from pathlib import Path
from specify_cli.invocation.record import OpCompletedEvent, OpStartedEvent
from specify_cli.invocation.writer import InvocationWriter

payload = json.load(sys.stdin)
root = Path(payload["root"])
writer = InvocationWriter(root)
if payload["action"] == "complete":
    writer.write_completed(OpCompletedEvent.model_validate(payload["completed"]))
else:
    writer.write_started(OpStartedEvent.model_validate(payload["started"]))
print("ok")
"#;

fn run_record_script(payload: &Value) -> String {
    run_python(LARGE_RECORD_SCRIPT, Some(&payload.to_string()))
}

#[test]
fn an_over_eight_mebibyte_record_closes_through_both_writers() {
    // F1 control, independent of the coordinator's regression: a trail this
    // writer creates above the kernel's generic 8 MiB precondition default
    // must be closed by the reference and vice versa, byte-for-byte. The
    // reference reads the whole owned record, so neither side may refuse it.
    const LARGE_ID: &str = "01KQZ5N8X3V4R7T2B6M9C1D0EF";
    let temp = tempfile::tempdir().expect("isolated project");
    let native_root = temp.path().join("native-started");
    let oracle_root = temp.path().join("oracle-started");
    fs::create_dir_all(&native_root).unwrap();
    fs::create_dir_all(&oracle_root).unwrap();

    let started_record = started_with(LARGE_ID, |fields| {
        fields.request_text = "x".repeat(9 * 1024 * 1024);
    });
    let started_fields: Value =
        serde_json::from_str(&started_record.to_jsonl_line()).expect("started fields");
    let completed_record = completed(LARGE_ID);
    let completed_fields: Value =
        serde_json::from_str(&completed_record.to_jsonl_line()).expect("completed fields");

    // Native start -> reference close.
    let native_path = InvocationWriter::new(&native_root)
        .write_started(&started_record)
        .expect("native start of a large record")
        .path;
    assert!(
        fs::metadata(&native_path).unwrap().len() > 8 * 1024 * 1024,
        "the fixture must be above the generic 8 MiB precondition default"
    );
    assert_eq!(
        run_record_script(&json!({
            "root": native_root.to_string_lossy(),
            "action": "complete",
            "completed": completed_fields,
        })),
        "ok"
    );

    // Reference start -> native close.
    assert_eq!(
        run_record_script(&json!({
            "root": oracle_root.to_string_lossy(),
            "action": "start",
            "started": started_fields,
        })),
        "ok"
    );
    let oracle_path = InvocationWriter::new(&oracle_root)
        .write_completed(&completed_record)
        .expect("native close of a reference-created large record");
    assert!(fs::metadata(&oracle_path).unwrap().len() > 8 * 1024 * 1024);

    // Closure comparison: both roots hold exactly the two reference lines.
    let native_bytes = fs::read(&native_path).expect("native trail");
    let oracle_bytes = fs::read(&oracle_path).expect("oracle-started trail");
    assert_eq!(native_bytes, oracle_bytes);
    let trail = String::from_utf8(native_bytes).expect("UTF-8 trail");
    assert_eq!(trail.lines().count(), 2);
    let lines: Vec<&str> = trail.lines().collect();
    assert_eq!(lines[0].len(), started_record.to_jsonl_line().len());
    assert_eq!(lines[1], completed_record.to_jsonl_line());

    // The completed record is readable through either writer's own root.
    assert_eq!(
        InvocationWriter::new(&native_root)
            .read_started(&InvocationId::parse(LARGE_ID).unwrap())
            .expect("read the large started row")
            .request_text()
            .len(),
        9 * 1024 * 1024
    );
}
