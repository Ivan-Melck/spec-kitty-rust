use serde_json::json;
use sk_runtime::{MissionTemplate, NextDecision, RunSnapshot, RuntimeError, plan_next};

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

#[test]
fn declared_significance_is_refused_until_its_policy_is_implemented() {
    let mut template = audit_template();
    template.audit_steps[0].significance = Some(
        serde_json::from_value(json!({"dimensions": {
            "user_customer_impact": 0,
            "architectural_system_impact": 0,
            "data_security_compliance_impact": 0,
            "operational_reliability_impact": 0,
            "financial_commercial_impact": 0,
            "cross_team_blast_radius": 0
        }}))
        .unwrap(),
    );
    assert!(matches!(
        plan_next(
            &template,
            &RunSnapshot::new("run", "audit-test", "hash"),
            "hash"
        ),
        Err(RuntimeError::UnsupportedPolicy { .. })
    ));
}
