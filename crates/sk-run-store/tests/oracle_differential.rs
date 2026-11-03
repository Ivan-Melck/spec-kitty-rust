//! Differential replay of a pinned-oracle `next` transcript through the store.
//!
//! `tests/fixtures/next_scenario.json` was captured by
//! `tests/oracle/generate_next_fixtures.py` from the oracle interpreter
//! (`spec-kitty 4.0.0rc8` at baseline `9aed3a80`) driving a real
//! `requires_inputs` mission:
//!
//! 1. `next --agent claude --result success` creates the run, records a
//!    `DecisionInputRequested` for `input:owner`, and issues no step;
//! 2. `next --agent claude --answer alice --result success` records the human
//!    answer and then issues the `specify` step.
//!
//! This test replays the same two transactions through [`RunStore`] and
//! compares the durable cursor and journal with the oracle's own bytes, after
//! normalizing the values that cannot match across machines (run id, template
//! path, wall-clock stamps). It is evidence about the store's persisted
//! contract, not about the CLI's decision envelope.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;
use sk_run_store::{
    ActorIdentity, ActorType, AdvanceRequest, AnswerRequest, RunRef, RunState, RunStore,
    StartRequest, StepResult,
};
use tempfile::TempDir;

fn fixture() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("next_scenario.json");
    let text = std::fs::read_to_string(&path).expect("differential fixture");
    serde_json::from_str(&text).expect("differential fixture is JSON")
}

/// Normalize the volatile fields the oracle fixture already normalized.
fn normalize(value: &Value, run_id: &str, template_path: &Path) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, item)| {
                    let item = match key.as_str() {
                        "run_id" => Value::String("<RUN_ID>".to_string()),
                        "template_path" => Value::String("<TEMPLATE_PATH>".to_string()),
                        "timestamp" | "answered_at" | "requested_at" => {
                            Value::String("<TIMESTAMP>".to_string())
                        }
                        _ => normalize(item, run_id, template_path),
                    };
                    (key.clone(), item)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| normalize(item, run_id, template_path))
                .collect(),
        ),
        Value::String(text) => {
            let replaced = text.replace(run_id, "<RUN_ID>").replace(
                &template_path.to_string_lossy().into_owned(),
                "<TEMPLATE_PATH>",
            );
            if is_timestamp(&replaced) {
                Value::String("<TIMESTAMP>".to_string())
            } else {
                Value::String(replaced)
            }
        }
        other => other.clone(),
    }
}

fn is_timestamp(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() >= 20
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && (text.ends_with('Z') || text.rfind(['+', '-']).is_some_and(|at| at > 10))
}

fn state_value(state: &RunState) -> Value {
    serde_json::to_value(state).expect("cursor is serializable")
}

fn events_value(root: &TempDir, run: &RunRef) -> Value {
    let path = run.run_dir.join("run.events.jsonl");
    let _ = root;
    let text = std::fs::read_to_string(path).expect("journal");
    Value::Array(
        text.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("journal line"))
            .collect(),
    )
}

fn assert_matches(fixture_step: &Value, actual_state: &Value, actual_events: &Value, label: &str) {
    let expected_state = fixture_step["state"].clone();
    if &expected_state != actual_state {
        let expected = serde_json::to_string_pretty(&expected_state).unwrap();
        let actual = serde_json::to_string_pretty(actual_state).unwrap();
        panic!(
            "{label}: cursor differs from the oracle\n--- oracle ---\n{expected}\n--- native ---\n{actual}"
        );
    }
    let expected_events = fixture_step["events"].clone();
    if &expected_events != actual_events {
        let expected = serde_json::to_string_pretty(&expected_events).unwrap();
        let actual = serde_json::to_string_pretty(actual_events).unwrap();
        panic!(
            "{label}: journal differs from the oracle\n--- oracle ---\n{expected}\n--- native ---\n{actual}"
        );
    }
}

fn scenario() -> (TempDir, PathBuf, Value) {
    let fixture = fixture();
    let root = TempDir::new().expect("temp project");
    let template = root.path().join("mission-runtime.yaml");
    std::fs::write(
        &template,
        fixture["template"].as_str().expect("template text"),
    )
    .expect("write template");
    (root, template, fixture)
}

#[test]
fn the_store_replays_the_oracle_advance_and_human_answer_byte_for_byte() {
    let (root, template, fixture) = scenario();
    let store = RunStore::open(root.path()).expect("store");
    let meta = &fixture["meta"];
    let slug = meta["mission_slug"].as_str().unwrap();
    let mission_id = meta["mission_id"].as_str().unwrap();

    let outcome = store
        .start(&StartRequest {
            mission_slug: slug.to_string(),
            mission_id: Some(mission_id.to_string()),
            template_path: template.clone(),
            inputs: BTreeMap::new(),
            policy_snapshot: Default::default(),
        })
        .expect("start");
    assert!(outcome.started);
    let run = outcome.run.clone();

    // Step 1: the first `--result success` records the pending input request.
    store
        .advance(
            &run,
            &AdvanceRequest {
                agent_id: "claude".to_string(),
                result: StepResult::Success,
                expected_issued_step: None,
            },
        )
        .expect("advance");
    let state = store.read_state(&run).expect("cursor");
    let steps = fixture["steps"].as_array().expect("steps");
    assert_matches(
        &steps[0],
        &normalize(&state_value(&state), &run.run_id, &template),
        &normalize(&events_value(&root, &run), &run.run_id, &template),
        "first advance",
    );

    // Step 2: the human answer, then the advance that issues the step.
    store
        .answer(
            &run,
            &AnswerRequest {
                decision_id: "input:owner".to_string(),
                answer: "alice".to_string(),
                actor: ActorIdentity::new("claude", ActorType::Human),
            },
        )
        .expect("answer");
    store
        .advance(
            &run,
            &AdvanceRequest {
                agent_id: "claude".to_string(),
                result: StepResult::Success,
                expected_issued_step: None,
            },
        )
        .expect("advance");
    let state = store.read_state(&run).expect("cursor");
    assert_matches(
        &steps[1],
        &normalize(&state_value(&state), &run.run_id, &template),
        &normalize(&events_value(&root, &run), &run.run_id, &template),
        "answered advance",
    );
}

#[test]
fn the_fixture_records_its_oracle_provenance() {
    let fixture = fixture();
    assert_eq!(
        fixture["provenance"]["source_baseline"],
        serde_json::json!("9aed3a8012cb5eb22c0128b14d525039f95fea7a")
    );
    assert_eq!(
        fixture["provenance"]["spec_kitty_version"],
        serde_json::json!("4.0.0rc8")
    );
    let steps = fixture["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 2);
    assert_eq!(
        steps[0]["decision"]["kind"],
        serde_json::json!("decision_required")
    );
    assert_eq!(
        steps[1]["decision"]["answered"],
        serde_json::json!("input:owner")
    );
}
