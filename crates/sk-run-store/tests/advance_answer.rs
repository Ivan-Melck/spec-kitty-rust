//! Acceptance tests: durable advance, stale-plan CAS, governed answers,
//! idempotence and prompt-file containment.

mod support;

use std::collections::BTreeMap;
use std::fs;

use serde_json::{Value, json};
use sk_run_store::{
    ActorIdentity, ActorType, AdvanceRequest, AnswerRequest, NextDecisionExt, RunStoreError,
    StepResult,
};

use support::{
    Fixture, OWNER_ID, agent, audit_yaml, event_types, human, input_step_yaml, json_file,
    run_files, state_bytes, two_step_yaml,
};

fn significance_audit_yaml(scores: [u8; 6]) -> String {
    format!(
        r#"mission:
  key: demo
  name: Demo mission
  version: "1.0.0"
steps:
  - id: specify
    title: Specify
    prompt: Do the specification.
  - id: plan
    title: Plan
    prompt: Do the plan.
    depends_on:
      - specify
audit_steps:
  - id: review
    title: Review
    audit:
      trigger_mode: manual
      enforcement: blocking
    significance:
      dimensions:
        user_customer_impact: {}
        architectural_system_impact: {}
        data_security_compliance_impact: {}
        operational_reliability_impact: {}
        financial_commercial_impact: {}
        cross_team_blast_radius: {}
      hard_triggers: []
    depends_on:
      - plan
"#,
        scores[0], scores[1], scores[2], scores[3], scores[4], scores[5]
    )
}

fn advance(fixture: &Fixture, run: &sk_run_store::RunRef, expected: Option<&str>) -> Value {
    let outcome = fixture
        .store
        .advance(
            run,
            &AdvanceRequest {
                agent_id: "agent-a".to_string(),
                result: StepResult::Success,
                expected_issued_step: expected.map(str::to_string),
            },
        )
        .expect("advance");
    serde_json::to_value(outcome.decision).expect("decision json")
}

#[test]
fn advance_issues_a_step_and_writes_events_before_the_cursor() {
    let fixture = Fixture::with_template_yaml(two_step_yaml());
    let started = fixture.start();
    let run = &started.run;

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
        .expect("advance");

    assert_eq!(outcome.decision.step_id(), Some("specify"));
    assert_eq!(outcome.completed_step_id, None);
    assert_eq!(outcome.state.issued_step_id.as_deref(), Some("specify"));
    assert_eq!(
        event_types(&run.run_dir),
        ["MissionRunStarted", "NextStepIssued"]
    );
    let issued = &support::event_lines(&run.run_dir)[1];
    assert_eq!(issued["payload"]["run_id"], run.run_id);
    assert_eq!(issued["payload"]["step_id"], "specify");
    assert_eq!(issued["payload"]["agent_id"], "agent-a");
    assert_eq!(issued["payload"]["actor"]["actor_type"], "llm");

    let state = json_file(&run.run_dir.join("state.json"));
    assert_eq!(state["issued_step_id"], "specify");
}

#[test]
fn advance_success_completes_the_issued_step_and_issues_the_next() {
    let fixture = Fixture::with_template_yaml(two_step_yaml());
    let started = fixture.start();
    let run = &started.run;
    advance(&fixture, run, None);

    let outcome = fixture
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

    assert_eq!(outcome.completed_step_id.as_deref(), Some("specify"));
    assert_eq!(outcome.decision.step_id(), Some("plan"));
    assert_eq!(outcome.state.completed_steps, ["specify"]);
    assert_eq!(outcome.state.issued_step_id.as_deref(), Some("plan"));
    assert_eq!(
        event_types(&run.run_dir),
        [
            "MissionRunStarted",
            "NextStepIssued",
            "NextStepAutoCompleted",
            "NextStepIssued"
        ]
    );
    let completed = &support::event_lines(&run.run_dir)[2];
    assert_eq!(completed["payload"]["step_id"], "specify");
    assert_eq!(completed["payload"]["result"], "success");
}

#[test]
fn a_stale_expected_issued_step_is_refused_and_writes_nothing() {
    let fixture = Fixture::with_template_yaml(two_step_yaml());
    let started = fixture.start();
    let run = &started.run;
    advance(&fixture, run, None);

    let before = run_files(&run.run_dir);
    let error = fixture
        .store
        .advance(
            run,
            &AdvanceRequest {
                agent_id: "agent-a".to_string(),
                result: StepResult::Success,
                expected_issued_step: Some("not-the-issued-step".to_string()),
            },
        )
        .expect_err("stale CAS refused");

    assert_eq!(error.code(), "STALE_ADVANCE_PLAN");
    assert_eq!(run_files(&run.run_dir), before, "refusal writes nothing");
    assert_eq!(
        fixture
            .store
            .query(run)
            .expect("query")
            .state
            .issued_step_id
            .as_deref(),
        Some("specify")
    );
}

#[test]
fn committing_the_same_plan_twice_is_refused_without_duplicate_events() {
    let fixture = Fixture::with_template_yaml(two_step_yaml());
    let started = fixture.start();
    let run = &started.run;
    advance(&fixture, run, None);

    let request = AdvanceRequest {
        agent_id: "agent-a".to_string(),
        result: StepResult::Success,
        expected_issued_step: Some("specify".to_string()),
    };
    let plan = fixture.store.plan_advance(run, &request).expect("plan");
    let first = fixture.store.commit_advance(&plan).expect("first commit");
    assert_eq!(first.decision.step_id(), Some("plan"));

    let before = run_files(&run.run_dir);
    let error = fixture
        .store
        .commit_advance(&plan)
        .expect_err("second commit is stale");
    assert_eq!(error.code(), "STALE_ADVANCE_PLAN");
    assert_eq!(run_files(&run.run_dir), before);
    assert_eq!(
        event_types(&run.run_dir)
            .iter()
            .filter(|event| event.as_str() == "NextStepAutoCompleted")
            .count(),
        1
    );
}

#[test]
fn failed_results_record_the_upstream_blocked_reason() {
    let fixture = Fixture::with_template_yaml(two_step_yaml());
    let started = fixture.start();
    let run = &started.run;
    advance(&fixture, run, None);

    let outcome = fixture
        .store
        .advance(
            run,
            &AdvanceRequest {
                agent_id: "agent-a".to_string(),
                result: StepResult::Failed,
                expected_issued_step: Some("specify".to_string()),
            },
        )
        .expect("advance");

    assert_eq!(outcome.completed_step_id.as_deref(), Some("specify"));
    assert_eq!(outcome.decision.kind().as_str(), "blocked");
    assert_eq!(
        outcome.state.blocked_reason.as_deref(),
        Some("Previous step 'specify' failed; manual intervention required.")
    );
    assert!(outcome.state.completed_steps.is_empty());
}

#[test]
fn input_answer_records_inputs_decisions_and_answered_event() {
    let fixture = Fixture::with_template_yaml(input_step_yaml());
    let started = fixture.start();
    let run = &started.run;

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
        .expect("advance");
    assert_eq!(outcome.decision.kind().as_str(), "decision_required");
    assert_eq!(outcome.decision.decision_id(), Some("input:topic"));
    assert!(run.run_dir.join("state.json").is_file());
    let state = json_file(&run.run_dir.join("state.json"));
    let request = &state["pending_decisions"]["input:topic"];
    assert_eq!(request["step_id"], "specify");
    assert_eq!(
        request["question"],
        "Input required before step 'specify': provide value for 'topic'."
    );
    assert_eq!(request["requested_by"]["actor_id"], "agent-a");
    assert_eq!(request["requested_by"]["actor_type"], "llm");

    let answered = fixture
        .store
        .answer(
            run,
            &AnswerRequest {
                decision_id: "input:topic".to_string(),
                answer: "rust-rewrite".to_string(),
                actor: human("owner-1"),
            },
        )
        .expect("answer");

    assert_eq!(answered.state.inputs["topic"], "rust-rewrite");
    assert_eq!(
        answered.state.decisions["input:topic"]["answer"],
        "rust-rewrite"
    );
    assert_eq!(
        answered.state.decisions["input:topic"]["authority_role"],
        "human"
    );
    assert_eq!(
        answered.state.decisions["input:topic"]["answered_by"]["actor_id"],
        "owner-1"
    );
    assert!(answered.state.pending_decisions.is_empty());
    let types = event_types(&run.run_dir);
    assert_eq!(
        types,
        [
            "MissionRunStarted",
            "DecisionInputRequested",
            "DecisionInputAnswered"
        ]
    );
    let answered_event = support::event_lines(&run.run_dir).last().cloned().unwrap();
    assert_eq!(answered_event["payload"]["decision_id"], "input:topic");
    assert_eq!(answered_event["payload"]["answer"], "rust-rewrite");
    assert_eq!(answered_event["payload"]["actor"]["actor_type"], "human");
}

#[test]
fn answering_an_unknown_or_already_answered_decision_is_a_typed_refusal() {
    let fixture = Fixture::with_template_yaml(input_step_yaml());
    let started = fixture.start();
    let run = &started.run;
    advance(&fixture, run, None);
    fixture
        .store
        .answer(
            run,
            &AnswerRequest {
                decision_id: "input:topic".to_string(),
                answer: "one".to_string(),
                actor: human("owner-1"),
            },
        )
        .expect("first answer");

    let before = run_files(&run.run_dir);
    let error = fixture
        .store
        .answer(
            run,
            &AnswerRequest {
                decision_id: "input:topic".to_string(),
                answer: "one".to_string(),
                actor: human("owner-1"),
            },
        )
        .expect_err("duplicate answer refused");
    assert_eq!(error.code(), "DECISION_NOT_FOUND");
    assert_eq!(run_files(&run.run_dir), before);

    let error = fixture
        .store
        .answer(
            run,
            &AnswerRequest {
                decision_id: "input:unknown".to_string(),
                answer: "x".to_string(),
                actor: human("owner-1"),
            },
        )
        .expect_err("unknown decision refused");
    assert_eq!(error.code(), "DECISION_NOT_FOUND");
}

#[test]
fn audit_answers_require_the_mission_owner_and_record_denials() {
    let fixture = Fixture::with_template_yaml_and_inputs(
        audit_yaml(),
        BTreeMap::from([("mission_owner_id".to_string(), json!(OWNER_ID))]),
    );
    let started = fixture.start();
    let run = &started.run;
    // specify -> plan -> audit:review
    advance(&fixture, run, None);
    advance(&fixture, run, Some("specify"));
    let audit = advance(&fixture, run, Some("plan"));
    assert_eq!(audit["kind"], "decision_required");
    assert_eq!(audit["decision_id"], "audit:review");
    assert_eq!(audit["options"], json!(["approve", "reject"]));

    let before_state = state_bytes(&run.run_dir);
    let before_events = support::event_lines(&run.run_dir).len();
    let error = fixture
        .store
        .answer(
            run,
            &AnswerRequest {
                decision_id: "audit:review".to_string(),
                answer: "approve".to_string(),
                actor: agent("llm-agent"),
            },
        )
        .expect_err("an LLM cannot approve an audit gate");
    assert_eq!(error.code(), "AUDIT_OWNER_REQUIRED");
    assert_eq!(
        state_bytes(&run.run_dir),
        before_state,
        "denial does not mutate the cursor"
    );
    let events = support::event_lines(&run.run_dir);
    assert_eq!(events.len(), before_events + 1);
    assert_eq!(
        events.last().unwrap()["event_type"],
        "DecisionAuthorityDenied"
    );
    assert_eq!(
        events.last().unwrap()["payload"]["reason"],
        "Audit decisions require a human actor"
    );

    let error = fixture
        .store
        .answer(
            run,
            &AnswerRequest {
                decision_id: "audit:review".to_string(),
                answer: "approve".to_string(),
                actor: human("someone-else"),
            },
        )
        .expect_err("only the owner may approve");
    assert_eq!(error.code(), "AUDIT_OWNER_REQUIRED");

    let approved = fixture
        .store
        .answer(
            run,
            &AnswerRequest {
                decision_id: "audit:review".to_string(),
                answer: "approve".to_string(),
                actor: human(OWNER_ID),
            },
        )
        .expect("owner approval");
    assert_eq!(approved.authority_role, "mission_owner");
    assert!(
        approved
            .state
            .completed_steps
            .contains(&"review".to_string())
    );
    assert!(approved.state.pending_decisions.is_empty());
    assert_eq!(
        approved.state.decisions["audit:review"]["authority_role"],
        "mission_owner"
    );
    assert_eq!(
        event_types(&run.run_dir).last().unwrap(),
        "DecisionInputAnswered"
    );
}

#[test]
fn audit_rejection_blocks_the_run_with_the_upstream_reason() {
    let fixture = Fixture::with_template_yaml_and_inputs(
        audit_yaml(),
        BTreeMap::from([("mission_owner_id".to_string(), json!(OWNER_ID))]),
    );
    let started = fixture.start();
    let run = &started.run;
    advance(&fixture, run, None);
    advance(&fixture, run, Some("specify"));
    advance(&fixture, run, Some("plan"));

    let rejected = fixture
        .store
        .answer(
            run,
            &AnswerRequest {
                decision_id: "audit:review".to_string(),
                answer: "reject".to_string(),
                actor: human(OWNER_ID),
            },
        )
        .expect("reject");
    assert_eq!(
        rejected.state.blocked_reason.as_deref(),
        Some("Audit step 'review' rejected by owner-1")
    );
    assert!(
        !rejected
            .state
            .completed_steps
            .contains(&"review".to_string())
    );
}

#[test]
fn repeated_medium_gate_answers_remain_queryable_and_recoverable() {
    let fixture = Fixture::with_template_yaml_and_inputs(
        &significance_audit_yaml([3, 2, 0, 3, 0, 0]),
        BTreeMap::from([("mission_owner_id".to_owned(), json!(OWNER_ID))]),
    );
    let started = fixture.start();
    let run = &started.run;
    advance(&fixture, run, None);
    advance(&fixture, run, Some("specify"));
    advance(&fixture, run, Some("plan"));
    for action in ["open_stand_up", "defer", "decide_solo"] {
        fixture
            .store
            .answer(
                run,
                &AnswerRequest {
                    decision_id: "audit:review".to_owned(),
                    answer: action.to_owned(),
                    actor: human(OWNER_ID),
                },
            )
            .unwrap();
        fixture.store.query(run).unwrap();
        fixture.store.recover(run).unwrap();
    }
    let state = fixture.store.read_state(run).unwrap();
    assert!(state.completed_steps.iter().any(|step| step == "review"));
    assert!(!state.pending_decisions.contains_key("audit:review"));
}

#[test]
fn medium_band_audit_answers_use_the_soft_gate_action_set() {
    let fixture = Fixture::with_template_yaml_and_inputs(
        audit_yaml(),
        BTreeMap::from([("mission_owner_id".to_string(), json!(OWNER_ID))]),
    );
    let started = fixture.start();
    let run = &started.run;
    advance(&fixture, run, None);
    advance(&fixture, run, Some("specify"));
    advance(&fixture, run, Some("plan"));

    // The frozen template declares no significance, so seed the evaluated band
    // the way the audit-significance planning pass would have recorded it.
    let mut state = json_file(&run.run_dir.join("state.json"));
    state["decisions"]["significance:audit:review"] = json!({
        "dimensions": [
            {"name": "user_customer_impact", "score": 1, "description": ""},
            {"name": "architectural_system_impact", "score": 1, "description": ""},
            {"name": "data_security_compliance_impact", "score": 1, "description": ""},
            {"name": "operational_reliability_impact", "score": 1, "description": ""},
            {"name": "financial_commercial_impact", "score": 1, "description": ""},
            {"name": "cross_team_blast_radius", "score": 1, "description": ""}
        ],
        "composite": 6,
        "band": {"name": "medium", "min_score": 7, "max_score": 11},
        "hard_trigger_classes": [],
        "effective_band": {"name": "medium", "min_score": 7, "max_score": 11}
    });
    support::write_state(&run.run_dir, &state);

    let before = state_bytes(&run.run_dir);
    let error = fixture
        .store
        .answer(
            run,
            &AnswerRequest {
                decision_id: "audit:review".to_string(),
                answer: "approve".to_string(),
                actor: human(OWNER_ID),
            },
        )
        .expect_err("approve is not a medium-band action");
    assert_eq!(error.code(), "AUDIT_ANSWER_INVALID");
    assert_eq!(state_bytes(&run.run_dir), before);

    let decided = fixture
        .store
        .answer(
            run,
            &AnswerRequest {
                decision_id: "audit:review".to_string(),
                answer: "decide_solo".to_string(),
                actor: human(OWNER_ID),
            },
        )
        .expect("decide_solo clears the medium gate");
    assert!(
        decided
            .state
            .completed_steps
            .contains(&"review".to_string())
    );
    assert!(decided.state.decisions["soft_gate:audit:review"]["action"] == "decide_solo");
}

#[test]
fn llm_answers_require_an_explicit_delegation_with_a_rationale() {
    let fixture = Fixture::with_template_yaml(input_step_yaml());
    let started = fixture.start();
    let run = &started.run;
    advance(&fixture, run, None);

    let llm = ActorIdentity {
        actor_id: "llm-1".to_string(),
        actor_type: ActorType::Llm,
        display_name: String::new(),
        provider: None,
        model: None,
        tool: None,
    };
    let error = fixture
        .store
        .answer(
            run,
            &AnswerRequest {
                decision_id: "input:topic".to_string(),
                answer: "x".to_string(),
                actor: llm.clone(),
            },
        )
        .expect_err("no delegation");
    assert_eq!(error.code(), "LLM_DELEGATION_REQUIRED");

    let mut state = json_file(&run.run_dir.join("state.json"));
    state["inputs"]["llm_delegations"] = json!({
        "input:topic": {
            "authority_role": "delegated_llm",
            "rationale_linkage": "policy/decision-42"
        }
    });
    support::write_state(&run.run_dir, &state);

    let answered = fixture
        .store
        .answer(
            run,
            &AnswerRequest {
                decision_id: "input:topic".to_string(),
                answer: "x".to_string(),
                actor: llm,
            },
        )
        .expect("delegated answer");
    assert_eq!(answered.authority_role, "delegated_llm");
    assert_eq!(
        answered.state.decisions["input:topic"]["rationale_linkage"],
        "policy/decision-42"
    );
}

#[test]
fn every_run_scoped_reference_is_a_serializable_contract() {
    let fixture = Fixture::with_template_yaml(two_step_yaml());
    let started = fixture.start();
    let encoded = serde_json::to_value(&started.run).expect("run ref serializes");
    let decoded: sk_run_store::RunRef =
        serde_json::from_value(encoded.clone()).expect("run ref round-trips");
    assert_eq!(decoded, started.run);
    assert_eq!(encoded["run_id"], started.run.run_id);

    let state: Value = serde_json::to_value(&started.state).expect("state serializes");
    assert_eq!(state["run_id"], started.run.run_id);
}

#[test]
fn nonempty_step_ids_are_preserved_while_prompt_paths_stay_contained() {
    for id in [
        "../escape",
        "spec review",
        "审查",
        "CON",
        "one:two",
        "first",
    ] {
        let template = format!(
            "mission:\n  key: demo\n  name: Demo\n  version: '1.0'\nsteps:\n  - id: {id}\n    title: Review\n    prompt: Work.\n"
        );
        let fixture = Fixture::with_template_yaml(&template);
        let started = fixture.start();
        let outcome = fixture
            .store
            .advance(
                &started.run,
                &AdvanceRequest {
                    agent_id: "agent-a".to_owned(),
                    result: StepResult::Success,
                    expected_issued_step: None,
                },
            )
            .unwrap();
        assert_eq!(outcome.state.issued_step_id.as_deref(), Some(id));
        let path = outcome.prompt_file.unwrap();
        assert!(path.starts_with(started.run.run_dir.join("prompts")));
        assert!(
            fs::read_to_string(path)
                .unwrap()
                .contains(&format!("step_id: {id}"))
        );
        assert!(!fixture.root.join("escape.prompt.md").exists());
    }
}

#[test]
fn advance_state_matches_the_baseline_json_shape() {
    let fixture = Fixture::with_template_yaml(two_step_yaml());
    let started = fixture.start();
    let run = &started.run;
    advance(&fixture, run, None);
    let state = json_file(&run.run_dir.join("state.json"));
    assert!(state["completed_steps"].is_array());
    assert!(state["inputs"].is_object());
    assert!(state["decisions"].is_object());
    assert!(state["pending_decisions"].is_object());
    assert_eq!(state["run_id"], run.run_id);
    assert_eq!(state["template_hash"], started.state.template_hash);
    // The persisted cursor never gains Rust-only fields (actor_context etc).
    assert!(state.get("actor_context").is_none());
    assert!(state.get("schema_version").is_none());
}

#[test]
fn external_audit_actors_are_refused_even_with_owner_like_ids() {
    let fixture = Fixture::with_template_yaml_and_inputs(
        audit_yaml(),
        BTreeMap::from([("mission_owner_id".to_string(), json!(OWNER_ID))]),
    );
    let started = fixture.start();
    let run = &started.run;
    advance(&fixture, run, None);
    advance(&fixture, run, Some("specify"));
    advance(&fixture, run, Some("plan"));

    let error = fixture
        .store
        .answer(
            run,
            &AnswerRequest {
                decision_id: "audit:review".to_string(),
                answer: "approve".to_string(),
                actor: ActorIdentity {
                    actor_id: OWNER_ID.to_string(),
                    actor_type: ActorType::Service,
                    display_name: String::new(),
                    provider: None,
                    model: None,
                    tool: None,
                },
            },
        )
        .expect_err("a service actor is not a human owner");
    assert_eq!(error.code(), "AUDIT_OWNER_REQUIRED");
}

#[test]
fn missing_mission_owner_input_refuses_audit_answers() {
    let fixture = Fixture::with_template_yaml(audit_yaml());
    let started = fixture.start();
    let run = &started.run;
    advance(&fixture, run, None);
    advance(&fixture, run, Some("specify"));
    advance(&fixture, run, Some("plan"));

    let error = fixture
        .store
        .answer(
            run,
            &AnswerRequest {
                decision_id: "audit:review".to_string(),
                answer: "approve".to_string(),
                actor: human(OWNER_ID),
            },
        )
        .expect_err("owner id is not configured");
    assert_eq!(error.code(), "AUDIT_OWNER_REQUIRED");
    assert!(
        error
            .to_string()
            .contains("mission_owner_id to be set in inputs")
    );
}

#[test]
fn run_store_error_payloads_are_machine_readable() {
    let fixture = Fixture::with_template_yaml(two_step_yaml());
    let outcome = fixture.start();
    let error = fixture
        .store
        .answer(
            &outcome.run,
            &AnswerRequest {
                decision_id: "input:missing".to_string(),
                answer: "x".to_string(),
                actor: human("owner"),
            },
        )
        .expect_err("unknown decision");
    let info = RunStoreError::info(&error);
    assert_eq!(info.code, "DECISION_NOT_FOUND");
    assert!(!info.message.is_empty());
    let wire = serde_json::to_value(&info).expect("error payload");
    assert_eq!(wire["code"], "DECISION_NOT_FOUND");
    assert!(wire["message"].as_str().is_some());
}

#[test]
fn durable_state_round_trips_through_the_public_model() {
    let fixture = Fixture::with_template_yaml(input_step_yaml());
    let started = fixture.start();
    let run = &started.run;
    advance(&fixture, run, None);
    let read = fixture.store.read_state(run).expect("read state");
    assert_eq!(read.run_id, started.state.run_id);
    assert_eq!(read.pending_decisions.len(), 1);
    assert_eq!(read.template_hash, started.state.template_hash);
    let encoded = fs::read_to_string(run.run_dir.join("state.json")).unwrap();
    let decoded: sk_run_store::RunState = serde_json::from_str(&encoded).expect("round-trip");
    assert_eq!(decoded, read);
    assert!(encoded.contains("\"pending_decisions\""));
}

#[test]
fn low_significance_audit_auto_completes_and_is_journal_consistent() {
    let template = significance_audit_yaml([0, 0, 0, 0, 0, 0]);
    let fixture = Fixture::with_template_yaml(&template);
    let started = fixture.start();
    let run = &started.run;
    advance(&fixture, run, None);
    advance(&fixture, run, Some("specify"));
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
        .expect("advance");

    assert_eq!(outcome.decision.kind().as_str(), "terminal");
    assert!(
        outcome
            .state
            .completed_steps
            .contains(&"review".to_string())
    );
    assert!(outcome.significance.is_some());
    let types = event_types(&run.run_dir);
    assert!(
        types.iter().any(|event| event == "SignificanceEvaluated"),
        "{types:?}"
    );
    assert_eq!(
        outcome.state.decisions["significance:audit:review"]["effective_band"]["name"],
        "low"
    );
    assert!(
        fixture.store.query(run).expect("query").consistency.healthy,
        "a LOW auto-completion must reconcile with the journal"
    );
    assert!(fixture.store.recover(run).expect("recover").is_noop());
}

#[test]
fn medium_significance_gate_uses_the_real_policy_options() {
    let template = significance_audit_yaml([2, 2, 1, 1, 1, 0]);
    let fixture = Fixture::with_template_yaml_and_inputs(
        &template,
        BTreeMap::from([("mission_owner_id".to_string(), json!(OWNER_ID))]),
    );
    let started = fixture.start();
    let run = &started.run;
    advance(&fixture, run, None);
    advance(&fixture, run, Some("specify"));
    let gate = advance(&fixture, run, Some("plan"));
    assert_eq!(gate["kind"], "decision_required");
    assert_eq!(gate["decision_id"], "audit:review");
    assert_eq!(
        gate["options"],
        json!(["decide_solo", "open_stand_up", "defer"])
    );

    let error = fixture
        .store
        .answer(
            run,
            &AnswerRequest {
                decision_id: "audit:review".to_string(),
                answer: "approve".to_string(),
                actor: human(OWNER_ID),
            },
        )
        .expect_err("approve is not a medium action");
    assert_eq!(error.code(), "AUDIT_ANSWER_INVALID");

    let decided = fixture
        .store
        .answer(
            run,
            &AnswerRequest {
                decision_id: "audit:review".to_string(),
                answer: "decide_solo".to_string(),
                actor: human(OWNER_ID),
            },
        )
        .expect("decide_solo");
    assert!(
        decided
            .state
            .completed_steps
            .contains(&"review".to_string())
    );
    assert_eq!(
        decided.state.decisions["soft_gate:audit:review"]["action"],
        "decide_solo"
    );
    assert!(fixture.store.query(run).expect("query").consistency.healthy);
}

#[test]
fn high_significance_gate_requires_the_owner_approval() {
    let template = significance_audit_yaml([2, 2, 2, 2, 2, 2]);
    let fixture = Fixture::with_template_yaml_and_inputs(
        &template,
        BTreeMap::from([("mission_owner_id".to_string(), json!(OWNER_ID))]),
    );
    let started = fixture.start();
    let run = &started.run;
    advance(&fixture, run, None);
    advance(&fixture, run, Some("specify"));
    let gate = advance(&fixture, run, Some("plan"));
    assert_eq!(gate["decision_id"], "audit:review");
    assert_eq!(gate["options"], json!(["approve", "reject"]));

    let approved = fixture
        .store
        .answer(
            run,
            &AnswerRequest {
                decision_id: "audit:review".to_string(),
                answer: "approve".to_string(),
                actor: human(OWNER_ID),
            },
        )
        .expect("approve");
    assert!(
        approved
            .state
            .completed_steps
            .contains(&"review".to_string())
    );
    assert!(fixture.store.query(run).expect("query").consistency.healthy);
    assert!(fixture.store.recover(run).expect("recover").is_noop());
}
