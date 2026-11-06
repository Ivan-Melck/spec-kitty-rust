//! Acceptance tests: torn tails, deterministic crash seams, hash drift,
//! index recovery and non-mutating refusal.

mod support;

use std::collections::BTreeMap;
use std::fs;

use serde_json::json;
use sk_run_store::{AdvanceRequest, AnswerRequest, RunStoreError, StepResult};

use support::{
    Fixture, MISSION_SLUG, event_types, human, json_file, run_files, state_bytes, two_step_yaml,
};

fn issue(fixture: &Fixture, run: &sk_run_store::RunRef) {
    fixture
        .store
        .advance(
            run,
            &AdvanceRequest {
                agent_id: "agent-a".to_string(),
                result: StepResult::Success,
                expected_issued_step: None,
            },
        )
        .expect("issue");
}

#[test]
fn a_torn_event_tail_refuses_append_and_query_without_mutation() {
    let fixture = Fixture::with_template_yaml(two_step_yaml());
    let started = fixture.start();
    let run = &started.run;
    let events_path = run.run_dir.join("run.events.jsonl");
    let mut bytes = fs::read(&events_path).expect("events");
    let torn = b"{\"event_type\":\"NextStepIssued\",\"pay";
    bytes.extend_from_slice(torn);
    fs::write(&events_path, &bytes).expect("torn tail");

    let before = run_files(&run.run_dir);
    let error = fixture
        .store
        .advance(
            run,
            &AdvanceRequest {
                agent_id: "agent-a".to_string(),
                result: StepResult::Success,
                expected_issued_step: None,
            },
        )
        .expect_err("torn tail refuses append");
    assert!(matches!(error, RunStoreError::TornEventTail { .. }));
    assert_eq!(error.code(), "TORN_EVENT_TAIL");
    assert_eq!(run_files(&run.run_dir), before, "a refusal writes nothing");

    let error = fixture
        .store
        .query(run)
        .expect_err("query refuses torn journal");
    assert_eq!(error.code(), "RECOVERY_REQUIRED");
    assert_eq!(run_files(&run.run_dir), before);

    let report = fixture.store.recover(run).expect("recover");
    assert_eq!(report.torn_tail_truncated, torn.len() as u64);
    assert!(fs::read(&events_path).expect("events").ends_with(b"\n"));
    assert_eq!(event_types(&run.run_dir), ["MissionRunStarted"]);

    let outcome = fixture
        .store
        .advance(
            run,
            &AdvanceRequest {
                agent_id: "agent-a".to_string(),
                result: StepResult::Success,
                expected_issued_step: None,
            },
        )
        .expect("advance after recovery");
    assert_eq!(outcome.decision.step_id(), Some("specify"));
}

#[test]
fn events_ahead_of_the_cursor_are_rolled_forward_under_the_lock() {
    let fixture = Fixture::with_template_yaml(two_step_yaml());
    let started = fixture.start();
    let run = &started.run;
    issue(&fixture, run);
    let before_state = state_bytes(&run.run_dir);

    // Complete the issued step and issue the next one, then put the old cursor
    // back: this is exactly the crash window between the event append and
    // `state.json`, reproduced deterministically.
    fixture
        .store
        .advance(
            run,
            &AdvanceRequest {
                agent_id: "agent-a".to_string(),
                result: StepResult::Success,
                expected_issued_step: Some("specify".to_string()),
            },
        )
        .expect("advance");
    fs::write(run.run_dir.join("state.json"), &before_state).expect("roll cursor back");

    let error = fixture
        .store
        .query(run)
        .expect_err("query is non-mutating and reports the gap");
    assert_eq!(error.code(), "RECOVERY_REQUIRED");

    let report = fixture.store.recover(run).expect("recover");
    assert!(report.state_rolled_forward);
    assert!(report.replayed_events >= 3);

    let state = fixture.store.read_state(run).expect("state");
    assert_eq!(state.completed_steps, ["specify"]);
    assert_eq!(state.issued_step_id.as_deref(), Some("plan"));
    assert!(
        fixture
            .store
            .query(run)
            .expect("healthy query")
            .consistency
            .healthy
    );

    // The repaired run is usable and the completion is not duplicated.
    let outcome = fixture
        .store
        .advance(
            run,
            &AdvanceRequest {
                agent_id: "agent-a".to_string(),
                result: StepResult::Success,
                expected_issued_step: Some("plan".to_string()),
            },
        )
        .expect("advance after repair");
    assert_eq!(outcome.state.completed_steps, ["specify", "plan"]);
    assert_eq!(
        event_types(&run.run_dir)
            .iter()
            .filter(|event| event.as_str() == "NextStepAutoCompleted")
            .count(),
        2
    );
}

#[test]
fn a_state_ahead_of_its_answered_event_is_repaired_without_rewriting_state() {
    let fixture = Fixture::with_template_yaml(support::input_step_yaml());
    let started = fixture.start();
    let run = &started.run;
    issue(&fixture, run);
    fixture
        .store
        .answer(
            run,
            &AnswerRequest {
                decision_id: "input:topic".to_string(),
                answer: "the-answer".to_string(),
                actor: human("owner"),
            },
        )
        .expect("answer");

    // Drop the trailing DecisionInputAnswered line: the answer-write order is
    // state first, then the event, so this is its crash window.
    let events_path = run.run_dir.join("run.events.jsonl");
    let text = fs::read_to_string(&events_path).expect("events");
    let mut lines: Vec<&str> = text.lines().collect();
    let last = lines.pop().expect("last event");
    assert!(last.contains("DecisionInputAnswered"));
    fs::write(&events_path, format!("{}\n", lines.join("\n"))).expect("truncate event");

    let state_before = state_bytes(&run.run_dir);
    let error = fixture
        .store
        .query(run)
        .expect_err("query reports the missing event");
    assert_eq!(error.code(), "RECOVERY_REQUIRED");
    assert_eq!(
        state_bytes(&run.run_dir),
        state_before,
        "query wrote nothing"
    );

    let report = fixture.store.recover(run).expect("recover");
    assert_eq!(report.missing_events_appended, 1);
    assert_eq!(
        state_bytes(&run.run_dir),
        state_before,
        "state was already durable"
    );
    let events = support::event_lines(&run.run_dir);
    assert_eq!(
        events.last().unwrap()["event_type"],
        "DecisionInputAnswered"
    );
    assert_eq!(
        events.last().unwrap()["payload"]["decision_id"],
        "input:topic"
    );
    assert_eq!(events.last().unwrap()["payload"]["answer"], "the-answer");
}

#[test]
fn a_state_ahead_of_its_started_event_is_repaired() {
    let fixture = Fixture::with_template_yaml(two_step_yaml());
    let started = fixture.start();
    let run = &started.run;
    fs::write(run.run_dir.join("run.events.jsonl"), b"").expect("erase events");

    let report = fixture.store.recover(run).expect("recover");
    assert!(report.started_event_appended);
    assert_eq!(event_types(&run.run_dir), ["MissionRunStarted"]);
    assert_eq!(
        support::event_lines(&run.run_dir)[0]["payload"]["run_id"],
        run.run_id
    );
    let second = fixture.store.recover(run).expect("second recover");
    assert!(second.is_noop(), "recovery is idempotent");
}

#[test]
fn a_missing_index_entry_is_repaired_from_the_durable_cursor() {
    let fixture = Fixture::with_template_yaml(two_step_yaml());
    let started = fixture.start();
    let run = &started.run;
    fs::remove_file(fixture.store.index_path()).expect("remove index");

    assert!(
        fixture
            .store
            .run_for_mission(MISSION_SLUG, None)
            .expect("read-only lookup")
            .is_none()
    );
    let report = fixture.store.recover(run).expect("recover index");
    assert!(report.index_repaired);
    let reloaded = fixture
        .store
        .run_for_mission(MISSION_SLUG, None)
        .expect("index read")
        .expect("repaired entry");
    assert_eq!(reloaded.run_id, run.run_id);
    assert!(
        fixture
            .store
            .query(run)
            .expect("healthy")
            .consistency
            .healthy
    );
}

#[test]
fn an_index_token_outside_the_project_is_never_followed() {
    let fixture = Fixture::with_template_yaml(two_step_yaml());
    let started = fixture.start();
    let outside = fixture.temp.path().join("outside");
    fs::create_dir_all(outside.join("state.json")).ok();
    fs::write(
        fixture.store.index_path(),
        serde_json::to_vec_pretty(&json!({
            "legacy-demo": {
                "run_id": started.run.run_id,
                "run_dir": "../outside",
                "mission_type": "demo",
                "mission_key": "demo",
                "mission_id": null,
                "mission_slug": "demo"
            }
        }))
        .unwrap(),
    )
    .expect("poison index");

    let error = fixture
        .store
        .run_for_mission(MISSION_SLUG, None)
        .expect_err("containment refusal");
    assert_eq!(error.code(), "RUN_DIR_OUTSIDE_REPO");
}

#[test]
fn frozen_template_hash_drift_is_a_typed_corruption_refusal() {
    let fixture = Fixture::with_template_yaml(two_step_yaml());
    let started = fixture.start();
    let run = &started.run;
    let frozen = run.run_dir.join("mission_template_frozen.yaml");
    let mut bytes = fs::read(&frozen).expect("frozen");
    bytes.extend_from_slice(b"\n# drift\n");
    fs::write(&frozen, &bytes).expect("mutate frozen");

    for error in [
        fixture.store.query(run).expect_err("query"),
        fixture.store.recover(run).expect_err("recover"),
    ] {
        assert_eq!(error.code(), "FROZEN_TEMPLATE_DRIFT");
    }
    // The cursor itself is never rewritten by a refused recovery.
    assert_eq!(
        fixture.store.read_state(run).expect("state").template_hash,
        started.state.template_hash
    );
}

#[test]
fn a_corrupt_cursor_is_typed_and_never_overwritten() {
    let fixture = Fixture::with_template_yaml(two_step_yaml());
    let started = fixture.start();
    let run = &started.run;
    fs::write(run.run_dir.join("state.json"), b"{not-json").expect("corrupt");

    let error = fixture.store.query(run).expect_err("query");
    assert_eq!(error.code(), "RUN_STATE_CORRUPT");
    let error = fixture.store.recover(run).expect_err("recover");
    assert_eq!(error.code(), "RUN_STATE_CORRUPT");
    assert_eq!(
        fs::read(run.run_dir.join("state.json")).unwrap(),
        b"{not-json"
    );
}

#[test]
fn a_legacy_run_without_identity_is_not_silently_adopted() {
    let fixture = Fixture::with_template_yaml(two_step_yaml());
    fixture.start();
    // Metadata now identifies the mission, but the indexed run predates that
    // identity. Upstream refuses and asks for verified ownership repair.
    let error = fixture
        .store
        .start(&sk_run_store::StartRequest {
            mission_slug: MISSION_SLUG.to_string(),
            mission_id: Some("01M0IDENTITY000000000000000".to_string()),
            template_path: fixture.template_path.clone(),
            inputs: BTreeMap::new(),
            policy_snapshot: Default::default(),
        })
        .expect_err("unverified adoption refused");
    assert_eq!(error.code(), "RUN_IDENTITY_MIGRATION_REQUIRED");
    let index = json_file(&fixture.store.index_path());
    assert_eq!(index.as_object().unwrap().len(), 1);
    assert!(index["legacy-demo"]["mission_id"].is_null());
}

#[test]
fn a_failed_start_leaves_no_half_created_run() {
    let fixture = Fixture::with_template_yaml("mission: [not-a-mapping");
    let error = fixture.store.start(&sk_run_store::StartRequest {
        mission_slug: MISSION_SLUG.to_string(),
        mission_id: None,
        template_path: fixture.template_path.clone(),
        inputs: BTreeMap::new(),
        policy_snapshot: Default::default(),
    });
    assert!(error.is_err());
    if fixture.store.runs_dir().is_dir() {
        assert_eq!(
            fs::read_dir(fixture.store.runs_dir()).unwrap().count(),
            0,
            "no orphan run directory"
        );
    }
}
