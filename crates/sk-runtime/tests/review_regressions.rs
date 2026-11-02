use serde_json::json;
use sk_runtime::{
    DecisionKind, MissionTemplate, NextDecision, RunSnapshot, StepResult, plan_advance_with_actor,
    plan_next,
};

fn audit_template() -> MissionTemplate {
    serde_json::from_value(json!({
        "mission": {"key": "audit-test", "name": "Audit", "version": "1.0", "description": ""},
        "steps": [],
        "audit_steps": [{"id": "review", "title": "Review", "audit": {"trigger_mode": "manual", "enforcement": "blocking"}}]
    })).unwrap()
}

#[test]
fn upstream_audit_decision_has_null_reason_and_round_trips() {
    let decision = plan_next(
        &audit_template(),
        &RunSnapshot::new("run", "audit-test", "hash"),
        "hash",
    )
    .unwrap();
    let value = serde_json::to_value(&decision).unwrap();
    assert_eq!(value["reason"], serde_json::Value::Null);
    assert_eq!(value["decision_id"], "audit:review");
    assert_eq!(decision.reason(), None);
    assert_eq!(
        serde_json::from_value::<NextDecision>(value).unwrap(),
        decision
    );
}

fn significance_template(hard_triggers: &[&str]) -> MissionTemplate {
    let mut template = audit_template();
    template.audit_steps[0].significance = Some(
        serde_json::from_value(json!({
            "dimensions": {
                "user_customer_impact": 0,
                "architectural_system_impact": 0,
                "data_security_compliance_impact": 0,
                "operational_reliability_impact": 0,
                "financial_commercial_impact": 0,
                "cross_team_blast_radius": 0
            },
            "hard_triggers": hard_triggers
        }))
        .unwrap(),
    );
    template
}

#[test]
fn bare_planner_keeps_the_upstream_approve_reject_checkpoint() {
    // The planner query is a pure DAG query: significance policy is folded in by
    // `plan_advance`, exactly like upstream `planner.plan_next` vs
    // `engine.plan_advance`. A significance-bearing gate still emits the plain
    // checkpoint here, with upstream's `reason: null` wire shape.
    let template = significance_template(&["security_privacy_access_control"]);
    let decision = plan_next(
        &template,
        &RunSnapshot::new("run", "audit-test", "hash"),
        "hash",
    )
    .unwrap();
    let value = serde_json::to_value(&decision).unwrap();
    assert_eq!(value["decision_id"], "audit:review");
    assert_eq!(value["options"], json!(["approve", "reject"]));
    assert_eq!(value["reason"], serde_json::Value::Null);
}

#[test]
fn declared_significance_now_folds_into_plan_advance() {
    let template = significance_template(&["security_privacy_access_control"]);
    let plan = plan_advance_with_actor(
        &template,
        &RunSnapshot::new("run", "audit-test", "hash"),
        "hash",
        None,
        StepResult::Success,
        Some("agent-1"),
    )
    .unwrap();

    assert_eq!(plan.decision.kind(), DecisionKind::DecisionRequired);
    let payload = plan.significance.expect("significance evaluated");
    assert_eq!(payload.effective_band.as_str(), "high");
    assert_eq!(payload.hard_trigger_classes.len(), 1);
    assert!(
        plan.snapshot
            .decisions
            .contains_key("significance:audit:review")
    );
    assert!(plan.snapshot.decisions.contains_key("raci:review"));
}
