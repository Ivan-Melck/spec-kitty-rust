//! Pure retrospective terminus lifecycle projection.

use sk_runtime::{
    DEFAULT_SKIP_REASON, DEFERRED_ACTION_ID, DEFERRED_FACILITATOR_ID, HicChoice, RequesterActor,
    RetrospectiveMode, TERMINUS_STEP_ID, TerminusStep, plan_terminus,
};

#[test]
fn the_terminus_constants_match_upstream() {
    assert_eq!(TERMINUS_STEP_ID, "terminus");
    assert_eq!(DEFERRED_FACILITATOR_ID, "deferred");
    assert_eq!(DEFERRED_ACTION_ID, "action:retrospect");
    assert_eq!(DEFAULT_SKIP_REASON, "(no reason provided)");
}

#[test]
fn autonomous_mode_requests_as_the_runtime_and_always_ends_with_the_gate() {
    let plan = plan_terminus(RetrospectiveMode::Autonomous, true, None).unwrap();
    assert_eq!(plan.requester, RequesterActor::Runtime);
    assert_eq!(
        plan.event_names(),
        vec![
            "retrospective.requested",
            "retrospective.started",
            "retrospective.completed",
            "gate"
        ]
    );
    assert!(plan.ends_with_gate());
    match &plan.steps[1] {
        TerminusStep::Started {
            facilitator_profile_id,
            action_id,
        } => {
            assert_eq!(facilitator_profile_id, DEFERRED_FACILITATOR_ID);
            assert_eq!(action_id, DEFERRED_ACTION_ID);
        }
        other => panic!("expected started, got {other:?}"),
    }
}

#[test]
fn autonomous_mode_without_a_facilitator_fails_and_still_gates() {
    let plan = plan_terminus(RetrospectiveMode::Autonomous, false, None).unwrap();
    assert_eq!(
        plan.event_names(),
        vec![
            "retrospective.requested",
            "retrospective.started",
            "retrospective.failed",
            "gate"
        ]
    );
    assert_eq!(
        plan.steps[2],
        TerminusStep::Failed {
            failure_code: "facilitator_not_configured".to_string()
        }
    );
}

#[test]
fn human_in_command_mode_requests_as_the_operator() {
    let plan = plan_terminus(
        RetrospectiveMode::HumanInCommand,
        true,
        Some(HicChoice::Run),
    )
    .unwrap();
    assert_eq!(plan.requester, RequesterActor::Operator);
    assert_eq!(
        plan.event_names(),
        vec![
            "retrospective.requested",
            "retrospective.started",
            "retrospective.completed",
            "gate"
        ]
    );
}

#[test]
fn human_in_command_without_a_prompt_fails_without_starting() {
    let plan = plan_terminus(RetrospectiveMode::HumanInCommand, true, None).unwrap();
    assert_eq!(
        plan.event_names(),
        vec!["retrospective.requested", "retrospective.failed", "gate"]
    );
    assert_eq!(
        plan.steps[1],
        TerminusStep::Failed {
            failure_code: "prompt_not_configured".to_string()
        }
    );
}

#[test]
fn human_in_command_run_without_a_facilitator_fails_after_starting() {
    let plan = plan_terminus(
        RetrospectiveMode::HumanInCommand,
        false,
        Some(HicChoice::Run),
    )
    .unwrap();
    assert_eq!(
        plan.event_names(),
        vec![
            "retrospective.requested",
            "retrospective.started",
            "retrospective.failed",
            "gate"
        ]
    );
}

#[test]
fn a_skip_records_the_reason_and_never_starts() {
    let plan = plan_terminus(
        RetrospectiveMode::HumanInCommand,
        true,
        Some(HicChoice::Skip {
            reason: Some("no time".to_string()),
        }),
    )
    .unwrap();
    assert_eq!(
        plan.event_names(),
        vec!["retrospective.requested", "retrospective.skipped", "gate"]
    );
    assert_eq!(
        plan.steps[1],
        TerminusStep::Skipped {
            skip_reason: "no time".to_string()
        }
    );
}

#[test]
fn a_blank_skip_reason_uses_the_upstream_fallback() {
    for reason in [None, Some(String::new())] {
        let plan = plan_terminus(
            RetrospectiveMode::HumanInCommand,
            true,
            Some(HicChoice::Skip { reason }),
        )
        .unwrap();
        assert_eq!(
            plan.steps[1],
            TerminusStep::Skipped {
                skip_reason: DEFAULT_SKIP_REASON.to_string()
            }
        );
    }
}
