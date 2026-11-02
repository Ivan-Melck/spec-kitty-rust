use std::collections::BTreeMap;

use serde_json::json;
use sk_runtime::{
    AuditConfig, AuditEnforcement, AuditStep, DecisionKind, DecisionRequest, DecisionRequirement,
    MissionMeta, MissionTemplate, NextDecision, PromptStep, RunSnapshot, TriggerMode, plan_next,
};

fn mission() -> MissionMeta {
    MissionMeta {
        key: "m-1".to_string(),
        name: "Mission One".to_string(),
        version: "1.0.0".to_string(),
        description: String::new(),
    }
}

fn prompt(id: &str, depends_on: &[&str], requires_inputs: &[&str]) -> PromptStep {
    PromptStep {
        id: id.to_string(),
        title: format!("Title {id}"),
        description: format!("Description {id}"),
        prompt: Some(format!("Run {id}.")),
        prompt_template: None,
        expected_output: None,
        requires_inputs: requires_inputs
            .iter()
            .map(|value| value.to_string())
            .collect(),
        depends_on: depends_on.iter().map(|value| value.to_string()).collect(),
        raci: None,
        raci_override_reason: None,
        agent_profile: None,
        contract_ref: None,
    }
}

fn audit(id: &str, depends_on: &[&str], enforcement: AuditEnforcement) -> AuditStep {
    AuditStep {
        id: id.to_string(),
        title: format!("Audit {id}"),
        description: format!("Audit description {id}"),
        audit: AuditConfig {
            trigger_mode: TriggerMode::Manual,
            enforcement,
            label: None,
            metadata: None,
        },
        significance: None,
        depends_on: depends_on.iter().map(|value| value.to_string()).collect(),
        raci: None,
        raci_override_reason: None,
    }
}

fn template(steps: Vec<PromptStep>, audit_steps: Vec<AuditStep>) -> MissionTemplate {
    MissionTemplate {
        mission: mission(),
        steps,
        audit_steps,
    }
}

fn snapshot() -> RunSnapshot {
    RunSnapshot::new("run-1", "m-1", "frozen-hash")
}

fn pending(id: &str, step_id: &str, options: &[&str]) -> DecisionRequest {
    DecisionRequest::new(
        id,
        step_id,
        format!("Question for {id}"),
        options.iter().map(|value| value.to_string()).collect(),
    )
}

#[test]
fn plan_next_validates_a_programmatically_constructed_template() {
    let mut second = prompt("second", &["first"], &[]);
    second.depends_on = vec!["missing".to_string()];
    let invalid = template(vec![prompt("first", &[], &[]), second], vec![]);

    let error = plan_next(&invalid, &snapshot(), "frozen-hash").unwrap_err();
    assert!(
        matches!(error, sk_runtime::RuntimeError::UnknownDependency { .. }),
        "{error:?}"
    );
}

#[test]
fn blocked_snapshot_beats_drift_and_pending_decisions() {
    let template = template(vec![prompt("a", &[], &[])], vec![]);
    let mut snapshot = snapshot();
    snapshot.blocked_reason = Some("manual block".to_string());
    snapshot
        .pending_decisions
        .insert("input:topic".to_string(), pending("input:topic", "a", &[]));

    let decision = plan_next(&template, &snapshot, "a-different-live-hash").unwrap();
    assert_eq!(decision.kind(), DecisionKind::Blocked);
    assert_eq!(decision.reason(), Some("manual block"));
}

#[test]
fn template_drift_beats_pending_and_steps() {
    let template = template(vec![prompt("a", &[], &[])], vec![]);
    let mut snapshot = snapshot();
    snapshot
        .pending_decisions
        .insert("input:topic".to_string(), pending("input:topic", "a", &[]));

    let decision = plan_next(&template, &snapshot, "live-hash").unwrap();
    assert_eq!(decision.kind(), DecisionKind::Blocked);
    assert_eq!(decision.reason(), Some(sk_runtime::TEMPLATE_DRIFT_REASON));
}

#[test]
fn empty_current_hash_means_no_live_template_and_skips_drift() {
    let template = template(vec![prompt("a", &[], &[])], vec![]);
    let decision = plan_next(&template, &snapshot(), "").unwrap();
    assert_eq!(decision.kind(), DecisionKind::Step);
}

#[test]
fn lexicographically_first_pending_decision_wins() {
    let template = template(vec![prompt("a", &[], &[])], vec![]);
    let mut snapshot = snapshot();
    snapshot
        .pending_decisions
        .insert("input:zeta".to_string(), pending("input:zeta", "a", &[]));
    snapshot
        .pending_decisions
        .insert("input:alpha".to_string(), pending("input:alpha", "a", &[]));
    snapshot.pending_decisions.insert(
        "audit:gate".to_string(),
        pending("audit:gate", "a", &["approve", "reject"]),
    );

    let decision = plan_next(&template, &snapshot, "frozen-hash").unwrap();
    match decision {
        NextDecision::DecisionRequired(required) => match required.requirement {
            DecisionRequirement::PendingDecision {
                decision_id,
                input_key,
                options,
                ..
            } => {
                assert_eq!(decision_id, "audit:gate");
                assert_eq!(input_key, None);
                assert_eq!(options, vec!["approve", "reject"]);
            }
            other => panic!("unexpected requirement: {other:?}"),
        },
        other => panic!("unexpected decision: {other:?}"),
    }
}

#[test]
fn pending_input_decision_derives_input_key() {
    let template = template(vec![prompt("a", &[], &[])], vec![]);
    let mut snapshot = snapshot();
    snapshot
        .pending_decisions
        .insert("input:topic".to_string(), pending("input:topic", "a", &[]));

    let decision = plan_next(&template, &snapshot, "frozen-hash").unwrap();
    let json = serde_json::to_value(&decision).unwrap();
    assert_eq!(json["kind"], "decision_required");
    assert_eq!(json["decision_id"], "input:topic");
    assert_eq!(json["input_key"], "topic");
    assert_eq!(json["reason"], "pending_decision");
    assert_eq!(json["options"], serde_json::Value::Null);
}

#[test]
fn mismatched_pending_key_is_rejected() {
    let template = template(vec![prompt("a", &[], &[])], vec![]);
    let mut snapshot = snapshot();
    snapshot
        .pending_decisions
        .insert("input:topic".to_string(), pending("input:other", "a", &[]));

    let error = plan_next(&template, &snapshot, "frozen-hash").unwrap_err();
    assert!(
        matches!(
            error,
            sk_runtime::RuntimeError::PendingDecisionMismatch { .. }
        ),
        "{error:?}"
    );
}

#[test]
fn missing_required_input_emits_an_input_decision() {
    let template = template(vec![prompt("discovery", &[], &["topic"])], vec![]);
    let decision = plan_next(&template, &snapshot(), "frozen-hash").unwrap();
    assert_eq!(decision.kind(), DecisionKind::DecisionRequired);
    assert_eq!(decision.step_id(), Some("discovery"));
    assert_eq!(decision.reason(), Some("missing_required_input"));

    let json = serde_json::to_value(&decision).unwrap();
    assert_eq!(json["decision_id"], "input:topic");
    assert_eq!(json["input_key"], "topic");
    assert_eq!(
        json["question"],
        "Input required before step 'discovery': provide value for 'topic'."
    );
}

#[test]
fn input_can_be_satisfied_by_inputs_or_by_a_decision_record() {
    let template = template(vec![prompt("discovery", &[], &["topic"])], vec![]);

    let mut from_inputs = snapshot();
    from_inputs
        .inputs
        .insert("topic".to_string(), json!("value"));
    assert_eq!(
        plan_next(&template, &from_inputs, "frozen-hash")
            .unwrap()
            .kind(),
        DecisionKind::Step
    );

    let mut from_decisions = snapshot();
    from_decisions
        .decisions
        .insert("topic".to_string(), json!({"answer": "value"}));
    assert_eq!(
        plan_next(&template, &from_decisions, "frozen-hash")
            .unwrap()
            .kind(),
        DecisionKind::Step
    );
}

#[test]
fn eligible_step_selection_uses_template_order_not_declaration_of_dependencies() {
    let template = template(
        vec![
            prompt("b", &[], &[]),
            prompt("a", &[], &[]),
            prompt("c", &["a", "b"], &[]),
        ],
        vec![],
    );

    let decision = plan_next(&template, &snapshot(), "frozen-hash").unwrap();
    assert_eq!(decision.step_id(), Some("b"));
}

#[test]
fn completed_and_issued_steps_are_skipped() {
    let template = template(vec![prompt("a", &[], &[]), prompt("b", &[], &[])], vec![]);

    let mut completed = snapshot();
    completed.completed_steps.push("a".to_string());
    assert_eq!(
        plan_next(&template, &completed, "frozen-hash")
            .unwrap()
            .step_id(),
        Some("b")
    );

    let mut issued = snapshot();
    issued.issued_step_id = Some("a".to_string());
    assert_eq!(
        plan_next(&template, &issued, "frozen-hash")
            .unwrap()
            .step_id(),
        Some("b")
    );
}

#[test]
fn unmet_dag_is_blocked_not_terminal() {
    let template = template(
        vec![prompt("a", &[], &[]), prompt("b", &["a"], &[])],
        vec![],
    );
    let mut snapshot = snapshot();
    snapshot.issued_step_id = Some("a".to_string());

    let decision = plan_next(&template, &snapshot, "frozen-hash").unwrap();
    assert_eq!(decision.kind(), DecisionKind::Blocked);
    assert_eq!(decision.reason(), Some(sk_runtime::UNSCHEDULABLE_REASON));
}

#[test]
fn all_steps_completed_is_terminal() {
    let template = template(
        vec![prompt("a", &[], &[]), prompt("b", &["a"], &[])],
        vec![],
    );
    let mut snapshot = snapshot();
    snapshot.completed_steps = vec!["a".to_string(), "b".to_string()];

    let decision = plan_next(&template, &snapshot, "frozen-hash").unwrap();
    assert_eq!(decision.kind(), DecisionKind::Terminal);
    assert_eq!(decision.reason(), Some(sk_runtime::TERMINAL_REASON));
}

#[test]
fn blocking_audit_becomes_an_approve_reject_decision() {
    let template = template(vec![], vec![audit("gate", &[], AuditEnforcement::Blocking)]);
    let decision = plan_next(&template, &snapshot(), "frozen-hash").unwrap();
    assert_eq!(decision.kind(), DecisionKind::DecisionRequired);
    let json = serde_json::to_value(&decision).unwrap();
    assert_eq!(json["decision_id"], "audit:gate");
    assert_eq!(json["options"], json!(["approve", "reject"]));
    assert_eq!(
        json["question"],
        "Audit checkpoint: Audit gate. Approve to continue?"
    );
    assert_eq!(json["input_key"], serde_json::Value::Null);
}

#[test]
fn advisory_audit_is_an_executable_step() {
    let template = template(vec![], vec![audit("gate", &[], AuditEnforcement::Advisory)]);
    let decision = plan_next(&template, &snapshot(), "frozen-hash").unwrap();
    assert_eq!(decision.kind(), DecisionKind::Step);
    assert_eq!(decision.step_id(), Some("gate"));
    let json = serde_json::to_value(&decision).unwrap();
    assert_eq!(json["prompt"], "Execute audit step 'gate': Audit gate");
    assert_eq!(json["context"]["expected_output"], serde_json::Value::Null);
}

#[test]
fn regular_steps_are_scheduled_before_audit_steps() {
    let template = template(
        vec![prompt("step", &[], &[])],
        vec![audit("audit", &[], AuditEnforcement::Advisory)],
    );
    let decision = plan_next(&template, &snapshot(), "frozen-hash").unwrap();
    assert_eq!(decision.step_id(), Some("step"));

    let mut after_step = snapshot();
    after_step.completed_steps.push("step".to_string());
    let decision = plan_next(&template, &after_step, "frozen-hash").unwrap();
    assert_eq!(decision.step_id(), Some("audit"));
}

#[test]
fn step_prompt_falls_back_when_template_has_no_prompt() {
    let mut step = prompt("a", &[], &[]);
    step.prompt = None;
    let template = template(vec![step], vec![]);
    let decision = plan_next(&template, &snapshot(), "frozen-hash").unwrap();
    let json = serde_json::to_value(&decision).unwrap();
    assert_eq!(json["prompt"], "Execute step 'a': Title a");
}

#[test]
fn policy_snapshot_and_actor_context_are_copied_into_step_context() {
    let template = template(vec![prompt("a", &[], &[])], vec![]);
    let mut snapshot = snapshot();
    snapshot.policy_snapshot.default_route = "fresh_context".to_string();
    snapshot
        .actor_context
        .insert("agent_id".to_string(), json!("agent-7"));

    let decision = plan_next(&template, &snapshot, "frozen-hash").unwrap();
    let json = serde_json::to_value(&decision).unwrap();
    assert_eq!(
        json["context"]["policy_snapshot"]["default_route"],
        "fresh_context"
    );
    assert_eq!(json["context"]["actor_context"]["agent_id"], "agent-7");
}

#[test]
fn decision_serialization_has_the_full_flat_schema_and_roundtrips() {
    let template = template(vec![prompt("a", &[], &[])], vec![]);
    let decision = plan_next(&template, &snapshot(), "frozen-hash").unwrap();
    let json = serde_json::to_value(&decision).unwrap();

    for key in [
        "kind",
        "run_id",
        "mission_key",
        "step_id",
        "step_title",
        "prompt",
        "context",
        "decision_id",
        "input_key",
        "question",
        "options",
        "reason",
    ] {
        assert!(json.get(key).is_some(), "missing key {key}: {json}");
    }
    assert_eq!(json["decision_id"], serde_json::Value::Null);

    let decoded: NextDecision = serde_json::from_value(json).expect("decision should roundtrip");
    assert_eq!(decoded, decision);
}

#[test]
fn pending_options_are_preserved_and_empty_options_serialize_null() {
    let template = template(vec![prompt("a", &[], &[])], vec![]);
    let mut with_options = snapshot();
    with_options.pending_decisions.insert(
        "input:topic".to_string(),
        pending("input:topic", "a", &["one", "two"]),
    );
    let decision = plan_next(&template, &with_options, "frozen-hash").unwrap();
    let json = serde_json::to_value(&decision).unwrap();
    assert_eq!(json["options"], json!(["one", "two"]));

    let mut empty = BTreeMap::new();
    empty.insert("input:topic".to_string(), pending("input:topic", "a", &[]));
    let mut empty_options = snapshot();
    empty_options.pending_decisions = empty;
    let json =
        serde_json::to_value(plan_next(&template, &empty_options, "frozen-hash").unwrap()).unwrap();
    assert_eq!(json["options"], serde_json::Value::Null);
}
