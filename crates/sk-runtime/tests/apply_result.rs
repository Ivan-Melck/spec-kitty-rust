use sk_runtime::{
    MissionMeta, MissionTemplate, PromptStep, RunSnapshot, RuntimeError, StepResult, apply_result,
    commit_advance, plan_advance,
};

fn template() -> MissionTemplate {
    MissionTemplate {
        mission: MissionMeta {
            key: "m".to_string(),
            name: "M".to_string(),
            version: "1.0.0".to_string(),
            description: String::new(),
        },
        steps: vec![
            PromptStep {
                id: "first".to_string(),
                title: "First".to_string(),
                description: String::new(),
                prompt: Some("Do first.".to_string()),
                prompt_template: None,
                expected_output: None,
                requires_inputs: Vec::new(),
                depends_on: Vec::new(),
                raci: None,
                raci_override_reason: None,
                agent_profile: None,
                contract_ref: None,
            },
            PromptStep {
                id: "second".to_string(),
                title: "Second".to_string(),
                description: String::new(),
                prompt: Some("Do second.".to_string()),
                prompt_template: None,
                expected_output: None,
                requires_inputs: Vec::new(),
                depends_on: vec!["first".to_string()],
                raci: None,
                raci_override_reason: None,
                agent_profile: None,
                contract_ref: None,
            },
        ],
        audit_steps: Vec::new(),
    }
}

fn issued() -> RunSnapshot {
    let mut snapshot = RunSnapshot::new("run-1", "m", "hash");
    snapshot.issued_step_id = Some("first".to_string());
    snapshot
}

#[test]
fn success_completes_the_issued_step_and_is_pure() {
    let snapshot = issued();
    let applied = apply_result(&snapshot, Some("first"), StepResult::Success).unwrap();

    assert_eq!(applied.completed_step_id.as_deref(), Some("first"));
    assert_eq!(applied.snapshot.issued_step_id, None);
    assert_eq!(applied.snapshot.completed_steps, vec!["first"]);
    assert_eq!(snapshot.issued_step_id.as_deref(), Some("first"));
    assert!(snapshot.completed_steps.is_empty());

    let again = apply_result(&applied.snapshot, None, StepResult::Success).unwrap();
    assert_eq!(again.snapshot, applied.snapshot);
    assert_eq!(again.completed_step_id, None);
}

#[test]
fn failed_and_blocked_record_a_reason_without_completing() {
    for (result, expected_fragment) in [
        (StepResult::Failed, "failed; manual intervention required"),
        (StepResult::Blocked, "reported blocked state"),
    ] {
        let applied = apply_result(&issued(), Some("first"), result).unwrap();
        assert_eq!(applied.completed_step_id.as_deref(), Some("first"));
        assert!(applied.snapshot.completed_steps.is_empty());
        assert_eq!(applied.snapshot.issued_step_id, None);
        let reason = applied.snapshot.blocked_reason.expect("blocked reason");
        assert!(reason.contains(expected_fragment), "{reason}");
    }
}

#[test]
fn no_issued_step_is_a_noop() {
    let snapshot = RunSnapshot::new("run-1", "m", "hash");
    let applied = apply_result(&snapshot, None, StepResult::Success).unwrap();
    assert_eq!(applied.snapshot, snapshot);
    assert_eq!(applied.completed_step_id, None);
}

#[test]
fn a_different_issued_step_is_stale_and_writes_nothing() {
    let snapshot = issued();
    let error = apply_result(&snapshot, Some("second"), StepResult::Success)
        .expect_err("stale issued step must fail");
    match error {
        RuntimeError::StaleIssuedStep {
            run_id,
            expected,
            found,
        } => {
            assert_eq!(run_id, "run-1");
            assert_eq!(expected.as_deref(), Some("second"));
            assert_eq!(found.as_deref(), Some("first"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn plan_advance_plans_from_the_applied_snapshot() {
    let plan = plan_advance(
        &template(),
        &issued(),
        "hash",
        Some("first"),
        StepResult::Success,
    )
    .unwrap();

    assert_eq!(plan.completed_step_id.as_deref(), Some("first"));
    assert_eq!(plan.snapshot.completed_steps, vec!["first"]);
    assert_eq!(plan.decision.step_id(), Some("second"));
    assert_eq!(plan.source, issued());
}

#[test]
fn commit_advance_refuses_a_stale_live_snapshot() {
    let plan = plan_advance(
        &template(),
        &issued(),
        "hash",
        Some("first"),
        StepResult::Success,
    )
    .unwrap();

    let committed = commit_advance(&plan, &issued()).unwrap();
    assert_eq!(committed, plan.snapshot);

    let mut changed = issued();
    changed.blocked_reason = Some("someone else wrote".to_string());
    let error = commit_advance(&plan, &changed).expect_err("stale commit must fail");
    assert!(
        matches!(error, RuntimeError::StaleRunState { .. }),
        "{error:?}"
    );
}
