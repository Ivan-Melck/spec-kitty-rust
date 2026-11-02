//! Differential golden tests against the pinned Python runtime.
//!
//! Each file under `tests/golden/` was captured from
//! `runtime.next._internal_runtime` (the pinned upstream reference) by
//! `tests/oracle/generate_goldens.py`. The file carries both the scenario
//! inputs and the expected output, so this test replays the scenario through
//! the pure Rust policy and compares:
//!
//! * `plan` scenarios: the decision JSON, the resulting snapshot JSON and the
//!   `SignificanceEvaluated` payload from [`plan_advance_with_actor`],
//! * `answer` scenarios: the persisted state JSON and the
//!   `DecisionInputAnswered` event, or the exact refusal message and the
//!   `DecisionAuthorityDenied` payload from [`apply_decision_answer`].
//!
//! Timestamps are normalised to `<TIMESTAMP>` on both sides because the
//! reference uses the wall clock; the Rust API takes the timestamp from the
//! caller. The one documented exception in the snapshot comparison is the
//! Rust-only `actor_context` extension field, which upstream snapshots do not
//! have.

use std::path::{Path, PathBuf};

use serde_json::Value;
use sk_runtime::{
    ActorIdentity, AnswerEffect, AppliedAnswer, MissionTemplate, RunSnapshot, StepResult,
    apply_decision_answer, plan_advance_with_actor,
};

const ANSWERED_AT: &str = "2000-01-01T00:00:00Z";

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
}

fn golden_files() -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(golden_dir())
        .expect("golden directory exists")
        .map(|entry| entry.expect("readable entry").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect();
    files.sort();
    assert!(!files.is_empty(), "golden directory must not be empty");
    files
}

fn read_golden(path: &Path) -> Value {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

/// Replace upstream wall-clock timestamps with a stable placeholder.
fn normalize(value: &mut Value) {
    match value {
        Value::String(text) if looks_like_timestamp(text) => {
            *text = "<TIMESTAMP>".to_string();
        }
        Value::Array(items) => {
            for item in items {
                normalize(item);
            }
        }
        Value::Object(map) => {
            for item in map.values_mut() {
                normalize(item);
            }
        }
        _ => {}
    }
}

fn looks_like_timestamp(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() >= 19
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b':'
        && bytes[16] == b':'
}

fn without_rust_extension(mut value: Value) -> Value {
    if let Value::Object(map) = &mut value {
        map.remove("actor_context");
    }
    value
}

fn plan_scenario(scenario: &Value, expected: &Value) {
    let template: MissionTemplate = serde_json::from_value(scenario["template"].clone())
        .expect("scenario template deserializes");
    template.validate().expect("scenario template validates");
    let snapshot: RunSnapshot = serde_json::from_value(scenario["snapshot"].clone())
        .expect("scenario snapshot deserializes");
    let current_hash = scenario["current_template_hash"]
        .as_str()
        .expect("current hash");
    let agent_id = scenario["agent_id"].as_str();
    let result = match scenario.get("result").and_then(Value::as_str) {
        None | Some("success") => StepResult::Success,
        Some("failed") => StepResult::Failed,
        Some("blocked") => StepResult::Blocked,
        Some(other) => panic!("unknown result {other}"),
    };

    let plan = plan_advance_with_actor(&template, &snapshot, current_hash, None, result, agent_id)
        .expect("plan advance succeeds");

    let mut actual_decision = serde_json::to_value(&plan.decision).expect("decision is JSON");
    normalize(&mut actual_decision);
    assert_eq!(
        actual_decision, expected["decision"],
        "decision drift in {}",
        scenario["name"]
    );

    let mut actual_snapshot = serde_json::to_value(&plan.snapshot).expect("snapshot is JSON");
    normalize(&mut actual_snapshot);
    assert_eq!(
        without_rust_extension(actual_snapshot),
        without_rust_extension(expected["snapshot"].clone()),
        "snapshot drift in {}",
        scenario["name"]
    );

    let actual_significance = plan
        .significance
        .as_ref()
        .map(|payload| serde_json::to_value(payload).expect("significance is JSON"));
    match (actual_significance, expected["significance"].as_object()) {
        (None, None) => {}
        (Some(actual), Some(_)) => assert_eq!(
            actual, expected["significance"],
            "significance payload drift in {}",
            scenario["name"]
        ),
        (actual, _) => panic!(
            "significance payload presence drift in {}: {actual:?}",
            scenario["name"]
        ),
    }
}

fn answer_scenario(scenario: &Value, expected: &Value) {
    let snapshot: RunSnapshot = serde_json::from_value(scenario["snapshot"].clone())
        .expect("scenario snapshot deserializes");
    let actor: ActorIdentity =
        serde_json::from_value(scenario["actor"].clone()).expect("scenario actor deserializes");
    let decision_id = scenario["decision_id"].as_str().expect("decision id");
    let answer = scenario["answer"].as_str().expect("answer");

    match expected["error"].as_str() {
        None => {
            let applied =
                apply_decision_answer(&snapshot, &actor, decision_id, answer, ANSWERED_AT)
                    .expect("answer applies");
            assert_applied(scenario, &applied, expected);
        }
        Some(expected_error) => {
            let error = apply_decision_answer(&snapshot, &actor, decision_id, answer, ANSWERED_AT)
                .expect_err("answer is refused");
            assert_eq!(
                error.to_string(),
                expected_error,
                "refusal message drift in {}",
                scenario["name"]
            );
            let events = expected["events"].as_array().expect("events array");
            match &error {
                sk_runtime::AnswerError::AuthorityDenied(denial) => {
                    assert_eq!(
                        events.len(),
                        1,
                        "audit denial appends exactly one event in {}",
                        scenario["name"]
                    );
                    let mut actual = serde_json::to_value(denial.as_ref()).expect("denial is JSON");
                    normalize(&mut actual);
                    assert_eq!(
                        actual, events[0]["payload"],
                        "denial payload drift in {}",
                        scenario["name"]
                    );
                    assert_eq!(events[0]["event_type"], "DecisionAuthorityDenied");
                }
                _ => {
                    assert!(
                        events.is_empty(),
                        "non-denial refusal appends no event in {}",
                        scenario["name"]
                    );
                    // The reference leaves state untouched; the pure call
                    // returned no snapshot, so there is nothing to persist.
                    let mut untouched = scenario["snapshot"].clone();
                    normalize(&mut untouched);
                    assert_eq!(
                        without_rust_extension(untouched),
                        without_rust_extension(expected["state"].clone()),
                        "refused answer left state unchanged in {}",
                        scenario["name"]
                    );
                }
            }
        }
    }
}

fn assert_applied(scenario: &Value, applied: &AppliedAnswer, expected: &Value) {
    let mut actual_state = serde_json::to_value(&applied.snapshot).expect("state is JSON");
    normalize(&mut actual_state);
    assert_eq!(
        without_rust_extension(actual_state),
        without_rust_extension(expected["state"].clone()),
        "state drift in {}",
        scenario["name"]
    );

    let mut actual_event = serde_json::to_value(&applied.event).expect("event is JSON");
    normalize(&mut actual_event);
    let events = expected["events"].as_array().expect("events array");
    assert_eq!(
        events.len(),
        1,
        "accepted answer appends exactly one event in {}",
        scenario["name"]
    );
    assert_eq!(
        events[0]["event_type"], "DecisionInputAnswered",
        "event type in {}",
        scenario["name"]
    );
    assert_eq!(
        actual_event, events[0]["payload"],
        "event payload drift in {}",
        scenario["name"]
    );

    // The persisted decision record equals the one the pure call returned.
    assert_eq!(
        applied.decision_record,
        applied.snapshot.decisions[scenario["decision_id"].as_str().unwrap()],
        "returned record is the persisted record in {}",
        scenario["name"]
    );
    assert_eq!(
        applied.authority.actor_id,
        scenario["actor"]["actor_id"].as_str().unwrap(),
        "authority actor in {}",
        scenario["name"]
    );
    if let AnswerEffect::InputRecorded { input_key } = &applied.effect {
        assert_eq!(
            input_key.as_str(),
            scenario["decision_id"]
                .as_str()
                .unwrap()
                .trim_start_matches("input:"),
            "input effect names the answered key in {}",
            scenario["name"]
        );
    }
}

#[test]
fn every_plan_golden_matches_the_python_reference() {
    let mut seen = 0;
    for path in golden_files() {
        let golden = read_golden(&path);
        if golden["scenario"]["kind"] != "plan" {
            continue;
        }
        plan_scenario(&golden["scenario"], &golden["expected"]);
        seen += 1;
    }
    assert!(seen >= 10, "expected a broad plan golden set, saw {seen}");
}

#[test]
fn every_answer_golden_matches_the_python_reference() {
    let mut accepted = 0;
    let mut refused = 0;
    for path in golden_files() {
        let golden = read_golden(&path);
        if golden["scenario"]["kind"] != "answer" {
            continue;
        }
        answer_scenario(&golden["scenario"], &golden["expected"]);
        if golden["expected"]["error"].is_null() {
            accepted += 1;
        } else {
            refused += 1;
        }
    }
    assert!(accepted >= 6, "expected accepted answers, saw {accepted}");
    assert!(refused >= 6, "expected refused answers, saw {refused}");
}

#[test]
fn golden_denials_are_fail_closed_against_the_reference() {
    // Explicit guardrails for the three audit authority denials plus the
    // forbidden LLM/human mismatch: each must be refused with the reference
    // message and the DecisionAuthorityDenied payload.
    let cases = [
        (
            "answer_audit_denied_llm_actor",
            "Audit decisions require a human actor",
        ),
        (
            "answer_audit_denied_wrong_owner",
            "Audit decisions require mission owner 'owner-1'",
        ),
        (
            "answer_audit_denied_missing_owner",
            "Audit decisions require mission_owner_id to be set in inputs",
        ),
        (
            "answer_audit_medium_rejects_approve",
            "Medium-band decision requires one of ['decide_solo', 'defer', 'open_stand_up'], got: 'approve'",
        ),
        (
            "answer_audit_high_rejects_soft_action",
            "High-band decision requires one of {'approve', 'reject'}, got: 'decide_solo'",
        ),
        (
            "answer_input_llm_not_delegated",
            "LLM actor 'bot' is not delegated for decision 'input:topic'",
        ),
        (
            "answer_input_llm_delegation_without_rationale",
            "LLM delegation for decision 'input:topic' must include non-empty rationale_linkage",
        ),
        (
            "answer_unknown_decision",
            "Decision 'input:nope' not found in pending_decisions for run 'run-1'",
        ),
    ];
    for (name, message) in cases {
        let golden = read_golden(&golden_dir().join(format!("{name}.json")));
        assert_eq!(
            golden["expected"]["error"], message,
            "the committed reference message for {name} changed"
        );
        answer_scenario(&golden["scenario"], &golden["expected"]);
    }
}
