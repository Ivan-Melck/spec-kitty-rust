//! Acceptance tests: durable start, index identity, non-mutating query.

mod support;

use std::collections::BTreeMap;
use std::fs;

use serde_json::Value;
use sha2::{Digest, Sha256};
use sk_run_store::{AdvanceRequest, NextDecisionExt, RunStoreError, StepResult};

use support::{
    Fixture, MISSION_SLUG, audit_yaml, event_types, input_step_yaml, json_file, run_files,
    two_step_yaml,
};

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn start_creates_the_baseline_run_layout_with_frozen_raw_template_sha() {
    let fixture = Fixture::with_template_yaml(two_step_yaml());
    let outcome = fixture.start();

    assert!(outcome.started);
    assert!(!outcome.resumed);
    assert_eq!(outcome.state.mission_key, "demo");
    assert_eq!(outcome.run.mission_slug.as_deref(), Some(MISSION_SLUG));

    let run_dir = &outcome.run.run_dir;
    assert!(run_dir.is_dir());
    assert_eq!(
        run_dir.parent().and_then(|path| path.file_name()),
        Some(std::ffi::OsStr::new("runs"))
    );
    assert_eq!(
        run_dir.file_name().and_then(|name| name.to_str()),
        Some(outcome.run.run_id.as_str())
    );
    assert_eq!(outcome.run.run_id.len(), 32);
    assert!(
        outcome
            .run
            .run_id
            .chars()
            .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
    );

    let source_bytes = fs::read(&fixture.template_path).expect("source template");
    let frozen_bytes = fs::read(run_dir.join("mission_template_frozen.yaml")).expect("frozen");
    assert_eq!(
        frozen_bytes, source_bytes,
        "frozen template is verbatim raw bytes"
    );
    assert_eq!(outcome.state.template_hash, sha256_hex(&source_bytes));

    let state = json_file(&run_dir.join("state.json"));
    assert_eq!(state["run_id"], outcome.run.run_id);
    assert_eq!(state["mission_key"], "demo");
    assert_eq!(state["template_hash"], outcome.state.template_hash);
    assert_eq!(
        state["template_path"],
        fixture.template_path.to_string_lossy().as_ref()
    );
    assert_eq!(state["issued_step_id"], Value::Null);
    assert_eq!(state["completed_steps"], serde_json::json!([]));
    assert!(state["pending_decisions"].is_object());
    assert!(state["decisions"].is_object());
    // Baseline shape only: no invented persistence fields.
    let mut keys = state
        .as_object()
        .expect("state object")
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    keys.sort();
    assert_eq!(
        keys,
        [
            "blocked_reason",
            "completed_steps",
            "decisions",
            "inputs",
            "issued_step_id",
            "mission_id",
            "mission_key",
            "mission_slug",
            "pending_decisions",
            "policy_snapshot",
            "run_id",
            "template_hash",
            "template_path",
        ]
    );

    assert_eq!(event_types(run_dir), ["MissionRunStarted"]);
    let started = &support::event_lines(run_dir)[0];
    assert_eq!(started["payload"]["run_id"], outcome.run.run_id);
    assert_eq!(started["payload"]["mission_type"], "demo");
    assert_eq!(started["payload"]["actor"]["actor_type"], "service");
    assert_eq!(started["payload"]["actor"]["actor_id"], "system");
}

#[test]
fn index_records_repo_relative_source_index_identity() {
    let fixture = Fixture::with_template_yaml(two_step_yaml());
    let outcome = fixture.start();
    let index = json_file(&fixture.store.index_path());

    let entry = &index["legacy-demo"];
    assert_eq!(entry["run_id"], outcome.run.run_id);
    assert_eq!(
        entry["run_dir"],
        format!(".kittify/runtime/runs/{}", outcome.run.run_id),
        "the index stores a repo-relative token, never an absolute path"
    );
    assert_eq!(entry["mission_type"], "demo");
    assert_eq!(entry["mission_key"], "demo");
    assert_eq!(entry["mission_slug"], MISSION_SLUG);
    assert!(entry["mission_id"].is_null());
}

#[test]
fn second_start_resumes_the_same_run_without_duplicating_events_or_entries() {
    let fixture = Fixture::with_template_yaml(two_step_yaml());
    let first = fixture.start();
    let second = fixture.start();

    assert!(first.started);
    assert!(!second.started);
    assert!(second.resumed);
    assert_eq!(first.run, second.run);
    assert_eq!(event_types(&first.run.run_dir), ["MissionRunStarted"]);

    let index = json_file(&fixture.store.index_path());
    assert_eq!(index.as_object().expect("index").len(), 1);
    let runs = fs::read_dir(fixture.store.runs_dir()).expect("runs dir");
    assert_eq!(runs.count(), 1, "a lost same-mission race leaves no orphan");
}

#[test]
fn query_is_non_mutating_and_previews_the_first_decision() {
    let fixture = Fixture::with_template_yaml(two_step_yaml());
    let outcome = fixture.start();
    let before = run_files(&fixture.root);

    let query = fixture.store.query(&outcome.run).expect("query");

    assert_eq!(run_files(&fixture.root), before, "query writes nothing");
    assert!(!fixture.store.cursor_lock_path(&outcome.run).exists());
    assert_eq!(query.run, outcome.run);
    assert_eq!(query.state.run_id, outcome.run.run_id);
    assert_eq!(query.issued_step, None);
    assert!(!query.template_drift);
    assert_eq!(query.decision.step_id(), Some("specify"));
    assert!(query.consistency.healthy);
}

#[test]
fn query_surfaces_the_issued_step_and_a_real_contained_prompt_file() {
    let fixture = Fixture::with_template_yaml(two_step_yaml());
    let outcome = fixture.start();
    let advanced = fixture
        .store
        .advance(
            &outcome.run,
            &AdvanceRequest {
                agent_id: "agent-a".to_string(),
                result: StepResult::Success,
                expected_issued_step: None,
            },
        )
        .expect("advance");

    let prompt_path = advanced.prompt_file.clone().expect("real prompt file");
    assert!(prompt_path.is_file(), "prompt file must exist");
    assert!(
        prompt_path.starts_with(&outcome.run.run_dir),
        "prompt file must be contained in the run directory"
    );
    let prompt_text = fs::read_to_string(&prompt_path).expect("prompt text");
    assert!(prompt_text.contains("Do the specification."));
    assert!(prompt_text.contains("step_id: specify"));

    let query = fixture.store.query(&outcome.run).expect("query");
    let issued = query.issued_step.expect("issued step");
    assert_eq!(issued.step_id, "specify");
    assert_eq!(&issued.prompt_file, &prompt_path);
    assert_eq!(query.state.issued_step_id.as_deref(), Some("specify"));
}

#[test]
fn query_reports_live_template_drift_without_rewriting_frozen_state() {
    let fixture = Fixture::with_template_yaml(two_step_yaml());
    let outcome = fixture.start();
    let frozen_before =
        fs::read(outcome.run.run_dir.join("mission_template_frozen.yaml")).expect("frozen before");
    let state_before = support::state_bytes(&outcome.run.run_dir);

    fs::write(
        &fixture.template_path,
        two_step_yaml().replace("Specify", "Changed"),
    )
    .expect("mutate live template");

    let query = fixture.store.query(&outcome.run).expect("query");
    assert!(query.template_drift);
    assert_eq!(query.template_hash, outcome.state.template_hash);
    assert_ne!(
        query.live_template_hash.as_deref(),
        Some(query.template_hash.as_str())
    );
    match query.decision {
        sk_run_store::NextDecision::Blocked(blocked) => {
            assert_eq!(blocked.reason, sk_run_store::TEMPLATE_DRIFT_REASON);
        }
        other => panic!("expected template drift block, got {other:?}"),
    }
    assert_eq!(
        fs::read(outcome.run.run_dir.join("mission_template_frozen.yaml")).unwrap(),
        frozen_before
    );
    assert_eq!(support::state_bytes(&outcome.run.run_dir), state_before);
}

#[test]
fn answer_refuses_when_the_run_index_points_at_a_missing_cursor() {
    let fixture = Fixture::with_template_yaml(two_step_yaml());
    let outcome = fixture.start();
    fs::remove_file(outcome.run.run_dir.join("state.json")).expect("remove cursor");

    let error = fixture
        .store
        .run_for_mission(MISSION_SLUG, None)
        .expect_err("missing cursor must be loud");
    assert_eq!(error.code(), "RUN_STATE_MISSING");
    assert!(matches!(error, RunStoreError::RunStateMissing { .. }));
}

#[test]
fn query_requires_a_complete_input_decision_before_the_step() {
    let fixture = Fixture::with_template_yaml(input_step_yaml());
    let outcome = fixture.start();
    let advanced = fixture
        .store
        .advance(
            &outcome.run,
            &AdvanceRequest {
                agent_id: "agent-a".to_string(),
                result: StepResult::Success,
                expected_issued_step: None,
            },
        )
        .expect("advance");

    assert_eq!(advanced.decision.kind().as_str(), "decision_required");
    assert_eq!(advanced.decision.decision_id(), Some("input:topic"));
    assert_eq!(advanced.decision.input_key(), Some("topic"));
    assert_eq!(advanced.state.pending_decisions.len(), 1);
    assert_eq!(
        advanced.state.pending_decisions["input:topic"]["step_id"],
        "specify"
    );
    let query = fixture.store.query(&outcome.run).expect("query");
    assert_eq!(query.decision.decision_id(), Some("input:topic"));
    assert_eq!(query.pending.len(), 1);
    assert_eq!(query.pending[0].decision_id, "input:topic");
    assert_eq!(query.pending[0].input_key.as_deref(), Some("topic"));
}

#[test]
fn empty_mission_slug_is_refused_before_any_directory_is_created() {
    let fixture = Fixture::with_template_yaml(two_step_yaml());
    let error = fixture
        .store
        .start(&sk_run_store::StartRequest {
            mission_slug: "../escape".to_string(),
            mission_id: None,
            template_path: fixture.template_path.clone(),
            inputs: BTreeMap::new(),
            policy_snapshot: Default::default(),
        })
        .expect_err("traversal slug refused");
    assert_eq!(error.code(), "INVALID_MISSION_SLUG");
    assert!(!fixture.store.runs_dir().exists());
}

#[test]
fn an_audit_template_starts_without_issuing_the_audit_gate() {
    let fixture = Fixture::with_template_yaml(audit_yaml());
    let outcome = fixture.start();
    let query = fixture.store.query(&outcome.run).expect("query");
    assert_eq!(query.decision.step_id(), Some("specify"));
}

#[test]
fn a_fresh_store_instance_restarts_from_the_durable_index_and_cursor() {
    let fixture = Fixture::with_template_yaml(two_step_yaml());
    let started = fixture.start();
    let issued = fixture
        .store
        .advance(
            &started.run,
            &AdvanceRequest {
                agent_id: "agent-a".to_string(),
                result: StepResult::Success,
                expected_issued_step: None,
            },
        )
        .expect("issue");
    drop(fixture.store);

    let reopened = sk_run_store::RunStore::open(&fixture.root).expect("reopen store");
    let run = reopened
        .run_for_mission(MISSION_SLUG, None)
        .expect("index read")
        .expect("durable run");
    assert_eq!(run, started.run);
    let query = reopened.query(&run).expect("query after restart");
    assert_eq!(query.state.issued_step_id.as_deref(), Some("specify"));
    let next = reopened
        .advance(
            &run,
            &AdvanceRequest {
                agent_id: "agent-a".to_string(),
                result: StepResult::Success,
                expected_issued_step: Some("specify".to_string()),
            },
        )
        .expect("advance after restart");
    assert_eq!(next.decision.step_id(), Some("plan"));
    assert_eq!(next.completed_step_id.as_deref(), Some("specify"));
    assert_eq!(issued.completed_step_id, None);
}
