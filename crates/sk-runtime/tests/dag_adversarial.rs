use serde_json::json;
use sk_runtime::{
    AuditConfig, AuditEnforcement, AuditStep, MissionMeta, MissionTemplate, PromptStep,
    RunSnapshot, TriggerMode, load_template, plan_next,
};

fn prompt(id: &str, depends_on: &[&str]) -> PromptStep {
    PromptStep {
        id: id.to_string(),
        title: id.to_string(),
        description: String::new(),
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

fn audit(id: &str, depends_on: &[&str], enforcement: AuditEnforcement) -> AuditStep {
    AuditStep {
        id: id.to_string(),
        title: id.to_string(),
        description: String::new(),
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

fn template(steps: Vec<PromptStep>, audits: Vec<AuditStep>) -> MissionTemplate {
    MissionTemplate {
        mission: MissionMeta {
            key: "dag".to_string(),
            name: "DAG".to_string(),
            version: "1.0.0".to_string(),
            description: String::new(),
        },
        steps,
        audit_steps: audits,
    }
}

fn snapshot(completed: &[&str]) -> RunSnapshot {
    let mut snapshot = RunSnapshot::new("run-1", "dag", "hash");
    snapshot.completed_steps = completed.iter().map(|value| value.to_string()).collect();
    snapshot
}

#[test]
fn diamond_dag_advances_one_eligible_step_at_a_time_in_template_order() {
    //     a
    //    / \
    //   b   c
    //    \ /
    //     d
    let template = template(
        vec![
            prompt("a", &[]),
            prompt("b", &["a"]),
            prompt("c", &["a"]),
            prompt("d", &["b", "c"]),
        ],
        vec![],
    );

    assert_eq!(
        plan_next(&template, &snapshot(&[]), "hash")
            .unwrap()
            .step_id(),
        Some("a")
    );
    assert_eq!(
        plan_next(&template, &snapshot(&["a"]), "hash")
            .unwrap()
            .step_id(),
        Some("b")
    );
    assert_eq!(
        plan_next(&template, &snapshot(&["a", "b"]), "hash")
            .unwrap()
            .step_id(),
        Some("c")
    );
    assert_eq!(
        plan_next(&template, &snapshot(&["a", "b", "c"]), "hash")
            .unwrap()
            .step_id(),
        Some("d")
    );
    assert_eq!(
        plan_next(&template, &snapshot(&["a", "b", "c", "d"]), "hash")
            .unwrap()
            .kind(),
        sk_runtime::DecisionKind::Terminal
    );
}

#[test]
fn diamond_with_only_one_branch_complete_still_blocks_d() {
    let template = template(
        vec![
            prompt("a", &[]),
            prompt("b", &["a"]),
            prompt("c", &["a"]),
            prompt("d", &["b", "c"]),
        ],
        vec![],
    );

    // b and c are complete but a is not: d is still not eligible, and a is.
    let decision = plan_next(&template, &snapshot(&["b", "c"]), "hash").unwrap();
    assert_eq!(decision.step_id(), Some("a"));

    // a is complete, b complete, c not: c is the only eligible step, d waits.
    let decision = plan_next(&template, &snapshot(&["a", "b"]), "hash").unwrap();
    assert_eq!(decision.step_id(), Some("c"));
}

#[test]
fn audit_can_sit_between_regular_steps_in_the_dag() {
    let template = template(
        vec![prompt("a", &[]), prompt("b", &["gate"])],
        vec![audit("gate", &["a"], AuditEnforcement::Blocking)],
    );

    assert_eq!(
        plan_next(&template, &snapshot(&[]), "hash")
            .unwrap()
            .step_id(),
        Some("a")
    );
    assert_eq!(
        plan_next(&template, &snapshot(&["a"]), "hash")
            .unwrap()
            .step_id(),
        Some("gate")
    );
    assert_eq!(
        plan_next(&template, &snapshot(&["a", "gate"]), "hash")
            .unwrap()
            .step_id(),
        Some("b")
    );
    assert_eq!(
        plan_next(&template, &snapshot(&["a", "gate", "b"]), "hash")
            .unwrap()
            .kind(),
        sk_runtime::DecisionKind::Terminal
    );
}

#[test]
fn advisory_audit_participates_in_the_dag_like_a_step() {
    let template = template(
        vec![prompt("b", &["audit"])],
        vec![audit("audit", &[], AuditEnforcement::Advisory)],
    );

    assert_eq!(
        plan_next(&template, &snapshot(&[]), "hash")
            .unwrap()
            .step_id(),
        Some("audit")
    );
    assert_eq!(
        plan_next(&template, &snapshot(&["audit"]), "hash")
            .unwrap()
            .step_id(),
        Some("b")
    );
}

#[test]
fn a_dependency_on_an_in_flight_issued_step_blocks_rather_than_terminates() {
    let template = template(vec![prompt("a", &[]), prompt("b", &["a"])], vec![]);
    let mut snapshot = snapshot(&[]);
    snapshot.issued_step_id = Some("a".to_string());

    let decision = plan_next(&template, &snapshot, "hash").unwrap();
    assert_eq!(decision.kind(), sk_runtime::DecisionKind::Blocked);
}

#[test]
fn a_cycle_across_steps_and_audits_is_rejected_by_the_loader() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("mission.yaml");
    std::fs::write(
        &path,
        r#"
mission:
  key: dag
  name: DAG
  version: "1.0.0"
steps:
  - id: step
    title: Step
    depends_on: [gate]
audit_steps:
  - id: gate
    title: Gate
    audit: {trigger_mode: manual, enforcement: blocking}
    depends_on: [step]
"#,
    )
    .unwrap();

    let error = load_template(&path).expect_err("cross-list cycle must fail");
    assert!(
        matches!(error, sk_runtime::RuntimeError::DependencyCycle { .. }),
        "{error:?}"
    );
}

#[test]
fn an_audit_with_an_unknown_dependency_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("mission.yaml");
    std::fs::write(
        &path,
        r#"
mission:
  key: dag
  name: DAG
  version: "1.0.0"
audit_steps:
  - id: gate
    title: Gate
    audit: {trigger_mode: manual, enforcement: blocking}
    depends_on: [nope]
"#,
    )
    .unwrap();

    let error = load_template(&path).expect_err("unknown audit dependency must fail");
    assert!(
        matches!(error, sk_runtime::RuntimeError::UnknownDependency { ref step_id, ref dependency }
            if step_id == "gate" && dependency == "nope"),
        "{error:?}"
    );
}

#[test]
fn completed_steps_outside_the_template_are_ignored_and_do_not_mask_an_unschedulable_run() {
    let template = template(vec![prompt("a", &[]), prompt("b", &["a"])], vec![]);
    let mut snapshot = snapshot(&["ghost"]);
    snapshot.issued_step_id = Some("a".to_string());

    let decision = plan_next(&template, &snapshot, "hash").unwrap();
    assert_eq!(decision.kind(), sk_runtime::DecisionKind::Blocked);
}

#[test]
fn required_input_after_dag_eligibility_is_reported_for_the_selected_step() {
    let mut step = prompt("a", &[]);
    step.requires_inputs = vec!["topic".to_string(), "owner".to_string()];
    let template = MissionTemplate {
        steps: vec![step],
        ..template(Vec::new(), Vec::new())
    };

    let decision = plan_next(&template, &snapshot(&[]), "hash").unwrap();
    let json = serde_json::to_value(&decision).unwrap();
    assert_eq!(json["decision_id"], json!("input:topic"));

    let mut with_topic = snapshot(&[]);
    with_topic.inputs.insert("topic".to_string(), json!("x"));
    let json = serde_json::to_value(plan_next(&template, &with_topic, "hash").unwrap()).unwrap();
    assert_eq!(json["decision_id"], json!("input:owner"));
}

#[test]
fn a_large_dag_does_not_panic_or_mutate_the_snapshot() {
    let mut steps = Vec::new();
    for index in 0..128 {
        let id = format!("s{index:03}");
        let deps: Vec<String> = if index == 0 {
            Vec::new()
        } else {
            vec![format!("s{:03}", index - 1)]
        };
        let mut step = prompt(&id, &[]);
        step.depends_on = deps;
        steps.push(step);
    }
    let template = MissionTemplate {
        mission: MissionMeta {
            key: "big".to_string(),
            name: "Big".to_string(),
            version: "1.0.0".to_string(),
            description: String::new(),
        },
        steps,
        audit_steps: Vec::new(),
    };
    let snapshot = snapshot(&[]);
    let decision = plan_next(&template, &snapshot, "hash").unwrap();
    assert_eq!(decision.step_id(), Some("s000"));
    assert!(snapshot.completed_steps.is_empty());
}
