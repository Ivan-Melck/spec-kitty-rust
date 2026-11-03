//! Regression cases for baseline run identity and portable legacy references.

mod support;

use std::fs;

use serde_json::{Value, json};
use sk_run_store::{AdvanceRequest, StepResult};
use support::{Fixture, MISSION_SLUG, two_step_yaml};

#[test]
fn baseline_nullable_cursor_identity_uses_the_validated_index_and_survives_recovery() {
    let fixture = Fixture::with_template_yaml(two_step_yaml());
    let started = fixture.start();
    let path = started.run.run_dir.join("state.json");
    let mut state: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    state["mission_slug"] = Value::Null;
    state["mission_id"] = Value::Null;
    fs::write(&path, serde_json::to_vec_pretty(&state).unwrap()).unwrap();
    let run = fixture.run();
    fixture.store.query(&run).unwrap();
    fixture.store.recover(&run).unwrap();
    fixture
        .store
        .advance(
            &run,
            &AdvanceRequest {
                agent_id: "agent-a".to_owned(),
                result: StepResult::Success,
                expected_issued_step: None,
            },
        )
        .unwrap();
    fs::remove_file(fixture.store.index_path()).unwrap();
    fixture.store.recover(&run).unwrap();
    assert_eq!(
        fixture
            .store
            .run_for_mission(MISSION_SLUG, None)
            .unwrap()
            .unwrap()
            .run_id,
        run.run_id
    );
    assert_eq!(fixture.store.read_state(&run).unwrap().mission_slug, None);
}

#[test]
fn an_absolute_in_repo_legacy_index_path_is_read_without_query_rewriting_it() {
    let fixture = Fixture::with_template_yaml(two_step_yaml());
    let started = fixture.start();
    let path = fixture.store.index_path();
    let mut index: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let entry = index.as_object_mut().unwrap().values_mut().next().unwrap();
    entry["run_dir"] = json!(started.run.run_dir);
    fs::write(&path, serde_json::to_vec_pretty(&index).unwrap()).unwrap();
    let before = fs::read(&path).unwrap();
    let run = fixture.run();
    fixture.store.query(&run).unwrap();
    assert_eq!(fs::read(&path).unwrap(), before);
}
