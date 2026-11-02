//! RACI inference and resolution: defaults, human accountability, sentinels.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use sk_runtime::{
    ActorType, AuditConfig, AuditEnforcement, AuditStep, HUMAN_IN_CHARGE_ACTOR_ID,
    MissionPolicySnapshot, MissionTemplate, PromptStep, RaciAssignment, RaciRoleSpec, RaciRule,
    RaciSource, RuntimeError, StepRef, TriggerMode, UNKNOWN_ACTOR_ID, bind_actor, infer_raci,
    resolve_raci, validate_raci_assignment,
};

fn prompt(id: &str) -> PromptStep {
    PromptStep {
        id: id.to_string(),
        title: "Prompt".to_string(),
        description: String::new(),
        prompt: None,
        prompt_template: None,
        expected_output: None,
        requires_inputs: Vec::new(),
        depends_on: Vec::new(),
        raci: None,
        raci_override_reason: None,
        agent_profile: None,
        contract_ref: None,
    }
}

fn audit(id: &str, enforcement: AuditEnforcement) -> AuditStep {
    AuditStep {
        id: id.to_string(),
        title: "Audit".to_string(),
        description: String::new(),
        audit: AuditConfig {
            trigger_mode: TriggerMode::Manual,
            enforcement,
            label: None,
            metadata: None,
        },
        significance: None,
        depends_on: Vec::new(),
        raci: None,
        raci_override_reason: None,
    }
}

fn inputs(pairs: &[(&str, &str)]) -> BTreeMap<String, Value> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), Value::String(value.to_string())))
        .collect()
}

fn role(actor_type: ActorType, actor_id: Option<&str>) -> RaciRoleSpec {
    RaciRoleSpec {
        actor_type,
        actor_id: actor_id.map(str::to_string),
    }
}

fn explicit(
    responsible: RaciRoleSpec,
    accountable: RaciRoleSpec,
    consulted: Vec<RaciRoleSpec>,
) -> RaciAssignment {
    RaciAssignment {
        responsible,
        accountable,
        consulted,
        informed: Vec::new(),
    }
}

#[test]
fn bind_actor_uses_the_per_type_sentinel_for_missing_or_blank_ids() {
    assert_eq!(
        bind_actor(ActorType::Human, None).actor_id,
        HUMAN_IN_CHARGE_ACTOR_ID
    );
    assert_eq!(
        bind_actor(ActorType::Human, Some("  ")).actor_id,
        HUMAN_IN_CHARGE_ACTOR_ID
    );
    assert_eq!(bind_actor(ActorType::Llm, None).actor_id, UNKNOWN_ACTOR_ID);
    assert_eq!(
        bind_actor(ActorType::Service, Some("")).actor_id,
        UNKNOWN_ACTOR_ID
    );
    // A present id is kept verbatim, matching upstream (no trimming).
    assert_eq!(
        bind_actor(ActorType::Llm, Some(" agent ")).actor_id,
        " agent "
    );
}

#[test]
fn inferred_defaults_follow_step_type() {
    let policy = MissionPolicySnapshot::default();
    let prompt_step = prompt("p");
    let advisory = audit("a", AuditEnforcement::Advisory);
    let blocking = audit("b", AuditEnforcement::Blocking);

    let snapshot = infer_raci(StepRef::Prompt(&prompt_step), &policy, None);
    assert_eq!(snapshot.source, RaciSource::Inferred);
    assert_eq!(snapshot.inferred_rule, Some(RaciRule::PromptDefault));
    assert_eq!(snapshot.responsible.actor_type, ActorType::Llm);
    assert_eq!(snapshot.accountable.actor_type, ActorType::Human);
    assert_eq!(snapshot.accountable.actor_id, HUMAN_IN_CHARGE_ACTOR_ID);

    let snapshot = infer_raci(StepRef::Audit(&advisory), &policy, None);
    assert_eq!(snapshot.inferred_rule, Some(RaciRule::AuditAdvisory));
    assert_eq!(snapshot.responsible.actor_type, ActorType::Llm);

    let snapshot = infer_raci(StepRef::Audit(&blocking), &policy, None);
    assert_eq!(snapshot.inferred_rule, Some(RaciRule::AuditBlocking));
    assert_eq!(snapshot.responsible.actor_type, ActorType::Human);
    assert_eq!(snapshot.responsible.actor_id, HUMAN_IN_CHARGE_ACTOR_ID);
}

#[test]
fn infer_raci_records_known_actors_before_applying_sentinels() {
    let policy = MissionPolicySnapshot::default();
    let step = prompt("p");
    let given = inputs(&[("agent_id", "claude")]);
    let snapshot = infer_raci(StepRef::Prompt(&step), &policy, Some(&given));
    assert_eq!(snapshot.responsible.actor_id, "claude");
    assert_eq!(snapshot.accountable.actor_id, HUMAN_IN_CHARGE_ACTOR_ID);

    let blocking = audit("b", AuditEnforcement::Blocking);
    let given = inputs(&[("mission_owner_id", "alice")]);
    let snapshot = infer_raci(StepRef::Audit(&blocking), &policy, Some(&given));
    assert_eq!(snapshot.responsible.actor_id, "alice");
    assert_eq!(snapshot.accountable.actor_id, "alice");
}

#[test]
fn resolve_raci_uses_inputs_for_the_accountable_human() {
    let policy = MissionPolicySnapshot::default();
    let step = prompt("p");
    let snapshot = resolve_raci(
        StepRef::Prompt(&step),
        &inputs(&[("mission_owner_id", "owner-42"), ("agent_id", "claude")]),
        &policy,
    )
    .unwrap();
    assert_eq!(snapshot.source, RaciSource::Inferred);
    assert_eq!(snapshot.responsible.actor_id, "claude");
    assert_eq!(snapshot.accountable.actor_id, "owner-42");
    assert_eq!(snapshot.override_reason, None);
}

#[test]
fn resolve_raci_fails_closed_when_the_owner_is_missing() {
    let policy = MissionPolicySnapshot::default();
    let step = prompt("p");
    let error = resolve_raci(
        StepRef::Prompt(&step),
        &inputs(&[("agent_id", "claude")]),
        &policy,
    )
    .unwrap_err();
    assert!(matches!(error, RuntimeError::RaciEscalation { .. }));
    assert_eq!(
        error.to_string(),
        "RACI escalation for step 'p': Cannot resolve accountable actor: 'mission_owner_id' not found in inputs"
    );
}

#[test]
fn explicit_assignment_wins_and_consulted_informed_take_sentinels() {
    let policy = MissionPolicySnapshot::default();
    let mut step = prompt("p");
    step.raci = Some(explicit(
        role(ActorType::Llm, Some("claude")),
        role(ActorType::Human, Some("owner-42")),
        vec![role(ActorType::Human, None), role(ActorType::Llm, Some(""))],
    ));
    step.raci_override_reason = Some("expert oversight".to_string());

    let snapshot = resolve_raci(StepRef::Prompt(&step), &BTreeMap::new(), &policy).unwrap();
    assert_eq!(snapshot.source, RaciSource::Explicit);
    assert_eq!(snapshot.inferred_rule, None);
    assert_eq!(
        snapshot.override_reason.as_deref(),
        Some("expert oversight")
    );
    assert_eq!(snapshot.responsible.actor_id, "claude");
    assert_eq!(snapshot.accountable.actor_id, "owner-42");
    assert_eq!(snapshot.consulted[0].actor_id, HUMAN_IN_CHARGE_ACTOR_ID);
    assert_eq!(snapshot.consulted[1].actor_id, UNKNOWN_ACTOR_ID);
}

#[test]
fn explicit_blank_ids_resolve_from_inputs_and_still_fail_closed_without_them() {
    let policy = MissionPolicySnapshot::default();
    let mut step = prompt("p");
    step.raci = Some(explicit(
        role(ActorType::Llm, Some("  ")),
        role(ActorType::Human, None),
        Vec::new(),
    ));
    step.raci_override_reason = Some("why".to_string());

    let snapshot = resolve_raci(
        StepRef::Prompt(&step),
        &inputs(&[("agent_id", "claude"), ("mission_owner_id", "owner-42")]),
        &policy,
    )
    .unwrap();
    assert_eq!(snapshot.responsible.actor_id, "claude");
    assert_eq!(snapshot.accountable.actor_id, "owner-42");

    let error = resolve_raci(StepRef::Prompt(&step), &BTreeMap::new(), &policy).unwrap_err();
    assert!(
        matches!(error, RuntimeError::RaciEscalation { .. }),
        "{error}"
    );
}

#[test]
fn blocking_audit_with_an_llm_responsible_role_is_refused() {
    let step = audit("gate", AuditEnforcement::Blocking);
    let assignment = explicit(
        role(ActorType::Llm, Some("claude")),
        role(ActorType::Human, Some("owner")),
        Vec::new(),
    );
    let errors = validate_raci_assignment(&assignment, StepRef::Audit(&step));
    assert_eq!(errors.len(), 1);
    assert!(
        errors[0].contains("responsible must be human"),
        "{errors:?}"
    );

    let mut step = step;
    step.raci = Some(assignment);
    step.raci_override_reason = Some("why".to_string());
    let error = resolve_raci(
        StepRef::Audit(&step),
        &inputs(&[("mission_owner_id", "owner")]),
        &MissionPolicySnapshot::default(),
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("Invalid explicit RACI"),
        "{error}"
    );
}

#[test]
fn accountable_must_be_human_in_explicit_assignments() {
    let step = prompt("p");
    let assignment = explicit(
        role(ActorType::Llm, Some("claude")),
        role(ActorType::Service, Some("bot")),
        Vec::new(),
    );
    let errors = validate_raci_assignment(&assignment, StepRef::Prompt(&step));
    assert_eq!(errors.len(), 1);
    assert!(
        errors[0].contains("accountable must be human"),
        "{errors:?}"
    );
}

#[test]
fn template_validation_enforces_human_accountability_and_override_reasons() {
    let mut step = prompt("p");
    step.raci = Some(explicit(
        role(ActorType::Llm, None),
        role(ActorType::Llm, Some("bot")),
        Vec::new(),
    ));
    step.raci_override_reason = Some("why".to_string());
    let template = MissionTemplate {
        mission: sk_runtime::MissionMeta {
            key: "m".to_string(),
            name: "M".to_string(),
            version: "1.0.0".to_string(),
            description: String::new(),
        },
        steps: vec![step],
        audit_steps: Vec::new(),
    };
    let error = template.validate().unwrap_err();
    assert!(matches!(
        error,
        RuntimeError::RaciAccountableNotHuman { .. }
    ));

    let mut step = prompt("p");
    step.raci = Some(explicit(
        role(ActorType::Llm, None),
        role(ActorType::Human, Some("owner")),
        Vec::new(),
    ));
    let template = MissionTemplate {
        mission: sk_runtime::MissionMeta {
            key: "m".to_string(),
            name: "M".to_string(),
            version: "1.0.0".to_string(),
            description: String::new(),
        },
        steps: vec![step],
        audit_steps: Vec::new(),
    };
    let error = template.validate().unwrap_err();
    assert!(matches!(error, RuntimeError::RaciOverrideReason { .. }));
}

#[test]
fn resolved_raci_wire_shape_matches_upstream() {
    let policy = MissionPolicySnapshot::default();
    let step = prompt("s1");
    let snapshot = resolve_raci(
        StepRef::Prompt(&step),
        &inputs(&[("mission_owner_id", "owner"), ("agent_id", "claude")]),
        &policy,
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(&snapshot).unwrap(),
        json!({
            "step_id": "s1",
            "responsible": {"actor_type": "llm", "actor_id": "claude"},
            "accountable": {"actor_type": "human", "actor_id": "owner"},
            "consulted": [],
            "informed": [],
            "source": "inferred",
            "inferred_rule": "prompt_default",
            "override_reason": null
        })
    );
}
