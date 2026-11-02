//! Evidence against the upstream runtime-parity fixtures.
//!
//! The fixture directory contains four captures:
//!
//! * `snapshot_start_mission_run.json` — run-store creation (engine-owned;
//!   deliberately not modelled by this pure library),
//! * `snapshot_next_step_1.json` — planner output (input-required decision),
//! * `snapshot_provide_decision_answer.json` — a decision-answer write
//!   (engine-owned; the planner only consumes its resulting input),
//! * `snapshot_next_step_2.json` — planner output (step decision).
//!
//! The two planner outputs are reproduced byte-for-byte as JSON values. The
//! other two are asserted to be lifecycle documents so the scope boundary is
//! explicit rather than silently claimed as parity.

use std::path::PathBuf;

use serde_json::Value;
use sk_runtime::{load_template, plan_next};

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("tests/fixtures/runtime_parity")
}

fn fixture(name: &str) -> Value {
    let path = fixture_dir().join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&text).expect("fixture is JSON")
}

fn reference_template() -> sk_runtime::MissionTemplate {
    load_template(&fixture_dir().join("reference_mission/mission.yaml"))
        .expect("reference mission should load")
}

#[test]
fn planner_reproduces_the_upstream_missing_input_decision() {
    let expected = fixture("snapshot_next_step_1.json");
    let template = reference_template();

    let snapshot = sk_runtime::RunSnapshot::new(
        expected["run_id"].as_str().unwrap(),
        expected["mission_key"].as_str().unwrap(),
        "frozen-template-hash",
    );

    let decision = plan_next(&template, &snapshot, "frozen-template-hash").unwrap();
    let actual = serde_json::to_value(&decision).unwrap();

    assert_eq!(
        actual, expected,
        "planner drifted from snapshot_next_step_1"
    );
}

#[test]
fn planner_reproduces_the_upstream_step_decision_after_the_input_answer() {
    let expected = fixture("snapshot_next_step_2.json");
    let template = reference_template();

    let mut snapshot = sk_runtime::RunSnapshot::new(
        expected["run_id"].as_str().unwrap(),
        expected["mission_key"].as_str().unwrap(),
        "frozen-template-hash",
    );
    let answer = fixture("snapshot_provide_decision_answer.json");
    snapshot.inputs.insert(
        answer["decision_id"]
            .as_str()
            .unwrap()
            .strip_prefix("input:")
            .unwrap()
            .to_string(),
        Value::String(answer["answer"].as_str().unwrap().to_string()),
    );
    snapshot.actor_context.insert(
        "agent_id".to_string(),
        Value::String("parity-agent".to_string()),
    );

    let decision = plan_next(&template, &snapshot, "frozen-template-hash").unwrap();
    let actual = serde_json::to_value(&decision).unwrap();

    assert_eq!(
        actual, expected,
        "planner drifted from snapshot_next_step_2"
    );
}

#[test]
fn lifecycle_fixtures_are_not_claimed_as_planner_parity() {
    let start = fixture("snapshot_start_mission_run.json");
    assert!(
        start.get("run_dir").is_some(),
        "start fixture is a run-store lifecycle document"
    );

    let answer = fixture("snapshot_provide_decision_answer.json");
    assert_eq!(answer["actor_type"], "human");
    assert_eq!(answer["decision_id"], "input:topic");
    assert!(
        answer.get("run_id").is_none(),
        "answer fixture describes the answer write, not a planner decision"
    );
}
