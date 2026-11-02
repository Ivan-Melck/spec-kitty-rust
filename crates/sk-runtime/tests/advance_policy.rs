//! Advance planning: engine policy folding and compare-and-swap commit checks.

use serde_json::json;
use sk_runtime::{
    ActorType, AuditConfig, AuditEnforcement, AuditStep, DecisionKind, DecisionRequirement,
    MissionMeta, MissionPolicySnapshot, MissionTemplate, PromptStep, RunSnapshot, RuntimeError,
    StepResult, TriggerMode, apply_result, commit_advance, plan_advance, plan_advance_with_actor,
};

fn prompt(id: &str, depends_on: &[&str]) -> PromptStep {
    PromptStep {
        id: id.to_string(),
        title: format!("Title {id}"),
        description: format!("Description {id}"),
        prompt: Some(format!("Run {id}.")),
        prompt_template: None,
        expected_output: None,
        requires_inputs: Vec::new(),
        depends_on: depends_on.iter().map(|value| value.to_string()).collect(),
        raci: None,
        raci_override_reason: None,
        agent_profile: None,
        contract_ref: None,
    }
}

fn low_dims() -> serde_json::Value {
    json!({
        "user_customer_impact": 0,
        "architectural_system_impact": 0,
        "data_security_compliance_impact": 0,
        "operational_reliability_impact": 0,
        "financial_commercial_impact": 0,
        "cross_team_blast_radius": 0
    })
}

fn medium_dims() -> serde_json::Value {
    json!({
        "user_customer_impact": 3,
        "architectural_system_impact": 2,
        "data_security_compliance_impact": 0,
        "operational_reliability_impact": 3,
        "financial_commercial_impact": 0,
        "cross_team_blast_radius": 0
    })
}

fn gate_template(dimensions: serde_json::Value, hard_triggers: &[&str]) -> MissionTemplate {
    let significance: sk_runtime::SignificanceBlock = serde_json::from_value(json!({
        "dimensions": dimensions,
        "hard_triggers": hard_triggers
    }))
    .unwrap();
    MissionTemplate {
        mission: MissionMeta {
            key: "m".to_string(),
            name: "M".to_string(),
            version: "1.0.0".to_string(),
            description: String::new(),
        },
        steps: vec![
            prompt("lead_in", &[]),
            prompt("follow_up", &["review_gate"]),
        ],
        audit_steps: vec![AuditStep {
            id: "review_gate".to_string(),
            title: "Review Gate".to_string(),
            description: "gate".to_string(),
            audit: AuditConfig {
                trigger_mode: TriggerMode::Manual,
                enforcement: AuditEnforcement::Blocking,
                label: None,
                metadata: None,
            },
            significance: Some(significance),
            depends_on: vec!["lead_in".to_string()],
            raci: None,
            raci_override_reason: None,
        }],
    }
}

fn ready_snapshot() -> RunSnapshot {
    let mut snapshot = RunSnapshot::new("run-1", "m", "frozen-hash");
    snapshot.completed_steps = vec!["lead_in".to_string()];
    snapshot
        .inputs
        .insert("mission_owner_id".to_string(), json!("owner-1"));
    snapshot
}

#[test]
fn a_low_gate_auto_proceeds_and_records_the_evaluation() {
    let template = gate_template(low_dims(), &[]);
    let plan = plan_advance_with_actor(
        &template,
        &ready_snapshot(),
        "frozen-hash",
        None,
        StepResult::Success,
        Some("agent-1"),
    )
    .unwrap();

    assert_eq!(plan.decision.kind(), DecisionKind::Step);
    assert_eq!(plan.decision.step_id(), Some("follow_up"));
    assert!(
        plan.snapshot
            .completed_steps
            .contains(&"review_gate".to_string())
    );
    assert!(
        plan.snapshot
            .decisions
            .contains_key("significance:audit:review_gate")
    );
    assert!(plan.snapshot.decisions.contains_key("raci:review_gate"));
    assert!(plan.snapshot.decisions.contains_key("raci:follow_up"));
    let payload = plan.significance.expect("evaluation payload");
    assert_eq!(payload.effective_band.as_str(), "low");
    assert_eq!(payload.decision_id, "audit:review_gate");
    assert_eq!(payload.step_id, "review_gate");
    assert_eq!(payload.actor.actor_type, ActorType::Service);
    assert_eq!(payload.actor.actor_id, "runtime");
    assert_eq!(
        plan.snapshot.decisions["raci:review_gate"]["inferred_rule"],
        "audit_blocking"
    );
}

#[test]
fn a_repolled_medium_gate_clears_the_pending_reason_without_inventing_a_title() {
    let template = gate_template(medium_dims(), &[]);
    let mut snapshot = ready_snapshot();
    snapshot.pending_decisions.insert(
        "audit:review_gate".to_owned(),
        sk_runtime::DecisionRequest::new(
            "audit:review_gate",
            "review_gate",
            "existing question",
            vec!["approve".to_owned(), "reject".to_owned()],
        ),
    );
    let plan = plan_advance(
        &template,
        &snapshot,
        "frozen-hash",
        None,
        StepResult::Success,
    )
    .unwrap();
    let value = serde_json::to_value(&plan.decision).unwrap();
    assert_eq!(value["reason"], serde_json::Value::Null);
    assert_eq!(value["step_title"], serde_json::Value::Null);
    assert_eq!(value["question"], "existing question");
    assert_eq!(
        value["options"],
        json!(["decide_solo", "open_stand_up", "defer"])
    );
    let round_trip: sk_runtime::NextDecision = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(round_trip).unwrap(), value);
}

#[test]
fn a_medium_gate_offers_the_soft_actions_and_high_keeps_approve_reject() {
    let template = gate_template(medium_dims(), &[]);
    let plan = plan_advance_with_actor(
        &template,
        &ready_snapshot(),
        "frozen-hash",
        None,
        StepResult::Success,
        Some("agent-1"),
    )
    .unwrap();
    assert_eq!(
        plan.significance.as_ref().unwrap().effective_band.as_str(),
        "medium"
    );
    match plan.decision {
        sk_runtime::NextDecision::DecisionRequired(required) => match required.requirement {
            DecisionRequirement::AuditCheckpoint { options, .. } => {
                assert_eq!(options, vec!["decide_solo", "open_stand_up", "defer"])
            }
            other => panic!("expected an audit checkpoint, got {other:?}"),
        },
        other => panic!("expected a decision request, got {other:?}"),
    }
    // MEDIUM keeps the audit step open: nothing completes it until answered.
    assert!(
        !plan
            .snapshot
            .completed_steps
            .contains(&"review_gate".to_string())
    );

    let high = gate_template(low_dims(), &["legal_compliance_regulatory"]);
    let plan = plan_advance_with_actor(
        &high,
        &ready_snapshot(),
        "frozen-hash",
        None,
        StepResult::Success,
        Some("agent-1"),
    )
    .unwrap();
    assert_eq!(
        plan.significance.as_ref().unwrap().effective_band.as_str(),
        "high"
    );
    let wire = serde_json::to_value(&plan.decision).unwrap();
    assert_eq!(wire["options"], json!(["approve", "reject"]));
    assert_eq!(wire["reason"], serde_json::Value::Null);
}

#[test]
fn the_issued_step_context_carries_the_agent_id() {
    let template = gate_template(low_dims(), &[]);
    let plan = plan_advance_with_actor(
        &template,
        &ready_snapshot(),
        "frozen-hash",
        None,
        StepResult::Success,
        Some("agent-1"),
    )
    .unwrap();
    let wire = serde_json::to_value(&plan.decision).unwrap();
    assert_eq!(wire["context"]["actor_context"]["agent_id"], "agent-1");
}

#[test]
fn plan_advance_reads_the_agent_id_from_the_snapshot_extension() {
    let mut snapshot = ready_snapshot();
    snapshot
        .actor_context
        .insert("agent_id".to_string(), json!("context-agent"));
    let template = gate_template(low_dims(), &[]);
    let plan = plan_advance(
        &template,
        &snapshot,
        "frozen-hash",
        None,
        StepResult::Success,
    )
    .unwrap();
    assert_eq!(
        plan.snapshot.decisions["raci:follow_up"]["responsible"]["actor_id"],
        "context-agent"
    );
}

#[test]
fn malformed_significance_policy_fails_closed_at_plan_time() {
    let template = gate_template(low_dims(), &[]);
    let mut snapshot = ready_snapshot();
    snapshot.policy_snapshot.extras.insert(
        "significance_band_cutoffs".to_string(),
        json!({"low": [0, 5], "medium": [7, 11], "high": [12, 18]}),
    );
    let error = plan_advance_with_actor(
        &template,
        &snapshot,
        "frozen-hash",
        None,
        StepResult::Success,
        Some("agent-1"),
    )
    .unwrap_err();
    assert!(
        matches!(error, RuntimeError::InvalidPolicy { .. }),
        "{error}"
    );
}

#[test]
fn missing_owner_still_records_an_inferred_raci_for_the_gate() {
    let template = gate_template(medium_dims(), &[]);
    let mut snapshot = ready_snapshot();
    snapshot.inputs.clear();
    let plan = plan_advance_with_actor(
        &template,
        &snapshot,
        "frozen-hash",
        None,
        StepResult::Success,
        Some("agent-1"),
    )
    .unwrap();
    let record = &plan.snapshot.decisions["raci:review_gate"];
    // A blocking audit makes both R and A human, so both take the human
    // sentinel when the owner is absent; the llm `agent_id` input is irrelevant.
    assert_eq!(record["accountable"]["actor_id"], "human-in-charge");
    assert_eq!(record["responsible"]["actor_id"], "human-in-charge");
    assert_eq!(record["inferred_rule"], "audit_blocking");
}

#[test]
fn commit_advance_refuses_a_stale_source_snapshot() {
    let template = gate_template(low_dims(), &[]);
    let snapshot = ready_snapshot();
    let plan = plan_advance_with_actor(
        &template,
        &snapshot,
        "frozen-hash",
        None,
        StepResult::Success,
        None,
    )
    .unwrap();

    let mut live = snapshot.clone();
    live.completed_steps.push("someone_else".to_string());
    let error = commit_advance(&plan, &live).unwrap_err();
    assert!(matches!(error, RuntimeError::StaleRunState { .. }));

    assert_eq!(commit_advance(&plan, &snapshot).unwrap(), plan.snapshot);
}

#[test]
fn a_changed_live_state_is_a_stale_run_before_the_issued_step_check() {
    let template = gate_template(low_dims(), &[]);
    let mut snapshot = ready_snapshot();
    snapshot.issued_step_id = Some("lead_in".to_string());
    let plan = plan_advance_with_actor(
        &template,
        &snapshot,
        "frozen-hash",
        Some("lead_in"),
        StepResult::Success,
        None,
    )
    .unwrap();
    assert_eq!(plan.expected_issued_step.as_deref(), Some("lead_in"));

    // Any live difference (including a different issued step) is the broader
    // StaleRunState refusal, matching upstream's widened staleness authority.
    let mut live = snapshot.clone();
    live.issued_step_id = Some("other".to_string());
    let error = commit_advance(&plan, &live).unwrap_err();
    assert!(
        matches!(error, RuntimeError::StaleRunState { .. }),
        "{error}"
    );

    // The apply prelude already refuses a result for a step the caller did not run.
    let error =
        apply_result(&snapshot, Some("not-the-issued-step"), StepResult::Success).unwrap_err();
    assert!(matches!(error, RuntimeError::StaleIssuedStep { .. }));
}

#[test]
fn commit_advance_checks_the_bootstrap_expected_issued_step() {
    // A bridge may build a plan whose source was read before it re-checked the
    // step it actually evaluated. With the snapshot unchanged, a differing
    // expected step is still a typed StaleIssuedStep refusal.
    let template = gate_template(low_dims(), &[]);
    let snapshot = ready_snapshot();
    let mut plan = plan_advance_with_actor(
        &template,
        &snapshot,
        "frozen-hash",
        None,
        StepResult::Success,
        None,
    )
    .unwrap();
    plan.expected_issued_step = Some("bootstrap-step".to_string());

    let error = commit_advance(&plan, &snapshot).unwrap_err();
    assert!(
        matches!(error, RuntimeError::StaleIssuedStep { .. }),
        "{error}"
    );

    plan.expected_issued_step = None;
    assert_eq!(commit_advance(&plan, &snapshot).unwrap(), plan.snapshot);
}

#[test]
fn policy_snapshot_and_strictness_are_carried_into_the_plan() {
    let template = gate_template(low_dims(), &[]);
    let mut snapshot = ready_snapshot();
    snapshot.policy_snapshot = MissionPolicySnapshot {
        strictness: sk_runtime::Strictness::Max,
        default_route: "same_llm_context".to_string(),
        extras: Default::default(),
    };
    let plan = plan_advance_with_actor(
        &template,
        &snapshot,
        "frozen-hash",
        None,
        StepResult::Success,
        None,
    )
    .unwrap();
    assert_eq!(
        plan.snapshot.policy_snapshot.strictness,
        sk_runtime::Strictness::Max
    );
}
