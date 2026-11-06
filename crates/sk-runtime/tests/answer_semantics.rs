//! Decision-answer semantics, including fail-closed handling of corrupt state.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use sk_runtime::{
    ActorIdentity, ActorType, AnswerEffect, AnswerError, DecisionRequest, MissionPolicySnapshot,
    RunSnapshot, Strictness, apply_decision_answer, authorize_decision_answer,
};

const ANSWERED_AT: &str = "2026-01-01T00:00:00Z";

fn human(id: &str) -> ActorIdentity {
    ActorIdentity::new(id, ActorType::Human)
}

fn llm(id: &str) -> ActorIdentity {
    ActorIdentity::new(id, ActorType::Llm)
}

fn snapshot() -> RunSnapshot {
    let mut snapshot = RunSnapshot::new("run-1", "m", "frozen-hash");
    snapshot.inputs.insert(
        "mission_owner_id".to_string(),
        Value::String("owner-1".to_string()),
    );
    snapshot
}

fn with_pending(mut snapshot: RunSnapshot, id: &str, step: &str) -> RunSnapshot {
    snapshot.pending_decisions.insert(
        id.to_string(),
        DecisionRequest::new(id, step, "question?", Vec::new()),
    );
    snapshot
}

fn with_band(mut snapshot: RunSnapshot, decision_id: &str, band: &str) -> RunSnapshot {
    let mut score = json!({
        "dimensions": [],
        "composite": 0,
        "band": {"name": band, "min_score": 0, "max_score": 18},
        "hard_trigger_classes": [],
        "effective_band": {"name": band, "min_score": 0, "max_score": 18}
    });
    score["dimensions"] = json!(
        sk_runtime::DimensionName::ALL
            .into_iter()
            .map(|name| json!({"name": name, "score": 0, "description": ""}))
            .collect::<Vec<_>>()
    );
    snapshot
        .decisions
        .insert(format!("significance:{decision_id}"), score);
    snapshot
}

#[test]
fn an_input_answer_writes_the_input_and_records_authority() {
    let snapshot = with_pending(snapshot(), "input:topic", "s1");
    let applied = apply_decision_answer(
        &snapshot,
        &human("someone"),
        "input:topic",
        "alpha",
        ANSWERED_AT,
    )
    .unwrap();

    assert_eq!(
        applied.snapshot.inputs.get("topic"),
        Some(&Value::String("alpha".to_string()))
    );
    assert!(applied.snapshot.pending_decisions.is_empty());
    assert_eq!(
        applied.effect,
        AnswerEffect::InputRecorded {
            input_key: "topic".to_string()
        }
    );
    assert_eq!(applied.authority.authority_role, "human");
    assert_eq!(applied.event.decision_id, "input:topic");
    assert_eq!(applied.event.run_id, "run-1");
    // The decision record is persisted and the pure result returns it.
    assert_eq!(
        applied.decision_record,
        applied.snapshot.decisions["input:topic"]
    );
    assert_eq!(
        applied.decision_record["answered_by"],
        json!({
            "actor_id": "someone",
            "actor_type": "human",
            "display_name": "",
            "provider": null,
            "model": null,
            "tool": null
        })
    );
    assert_eq!(applied.decision_record["rationale_linkage"], Value::Null);
}

#[test]
fn an_llm_answer_needs_a_delegation_with_a_trimmed_rationale() {
    let mut snapshot = with_pending(snapshot(), "input:topic", "s1");
    snapshot.inputs.insert(
        "llm_delegations".to_string(),
        json!({"*": {"authority_role": 7, "rationale_linkage": "  owner approved  "}}),
    );

    let applied =
        apply_decision_answer(&snapshot, &llm("bot"), "input:topic", "x", ANSWERED_AT).unwrap();
    assert_eq!(applied.authority.authority_role, "delegated_llm");
    assert_eq!(
        applied.authority.rationale_linkage.as_deref(),
        Some("owner approved")
    );
    assert_eq!(applied.decision_record["actor_type"], "llm");
    assert_eq!(applied.decision_record["authority_role"], "delegated_llm");
}

#[test]
fn a_service_actor_answers_an_input_decision_without_delegation() {
    let snapshot = with_pending(snapshot(), "input:topic", "s1");
    let actor = ActorIdentity::new("svc", ActorType::Service);
    let applied =
        apply_decision_answer(&snapshot, &actor, "input:topic", "x", ANSWERED_AT).unwrap();
    assert_eq!(applied.authority.authority_role, "service");
}

#[test]
fn an_empty_actor_id_is_a_malformed_identity() {
    let snapshot = with_pending(snapshot(), "input:topic", "s1");
    let error =
        apply_decision_answer(&snapshot, &human(""), "input:topic", "x", ANSWERED_AT).unwrap_err();
    assert_eq!(error, AnswerError::InvalidActorIdentity);
}

#[test]
fn an_unknown_decision_is_refused_before_any_state_change() {
    let snapshot = with_pending(snapshot(), "input:topic", "s1");
    let error = apply_decision_answer(&snapshot, &human("owner-1"), "input:nope", "x", ANSWERED_AT)
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Decision 'input:nope' not found in pending_decisions for run 'run-1'"
    );
}

#[test]
fn authorize_does_not_change_the_snapshot() {
    let snapshot = with_pending(snapshot(), "input:topic", "s1");
    let before = snapshot.clone();
    authorize_decision_answer(&snapshot, &human("owner-1"), "input:topic", "x").unwrap();
    assert_eq!(snapshot, before);
}

#[test]
fn audit_answers_require_a_human_owner_and_produce_a_denial_payload() {
    let mut snapshot = with_pending(snapshot(), "audit:gate", "gate");
    snapshot.decisions.insert(
        "raci:gate".to_string(),
        json!({"source": "inferred", "override_reason": "why"}),
    );

    let error = apply_decision_answer(&snapshot, &llm("bot"), "audit:gate", "approve", ANSWERED_AT)
        .unwrap_err();
    match error {
        AnswerError::AuthorityDenied(denial) => {
            assert_eq!(denial.run_id, "run-1");
            assert_eq!(denial.actor_type, ActorType::Llm);
            assert_eq!(denial.actor_id, "bot");
            assert_eq!(denial.authority_role, "mission_owner");
            assert_eq!(denial.rationale_linkage, None);
            assert_eq!(denial.reason, "Audit decisions require a human actor");
            assert_eq!(denial.raci_source.as_deref(), Some("inferred"));
            assert_eq!(denial.override_reason.as_deref(), Some("why"));
        }
        other => panic!("expected an authority denial, got {other:?}"),
    }

    let error = apply_decision_answer(
        &snapshot,
        &human("intruder"),
        "audit:gate",
        "approve",
        ANSWERED_AT,
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Audit decisions require mission owner 'owner-1'"
    );

    let mut missing_owner = snapshot.clone();
    missing_owner.inputs.remove("mission_owner_id");
    let error = apply_decision_answer(
        &missing_owner,
        &human("owner-1"),
        "audit:gate",
        "approve",
        ANSWERED_AT,
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Audit decisions require mission_owner_id to be set in inputs"
    );

    // A blank owner behaves as absent, not as an empty match.
    missing_owner.inputs.insert(
        "mission_owner_id".to_string(),
        Value::String("   ".to_string()),
    );
    let error = apply_decision_answer(
        &missing_owner,
        &human(""),
        "audit:gate",
        "approve",
        ANSWERED_AT,
    )
    .unwrap_err();
    assert_eq!(error, AnswerError::InvalidActorIdentity);
}

#[test]
fn audit_authority_is_checked_before_answer_validation() {
    // Upstream order: an unauthorized actor with an invalid answer is denied,
    // not reported as an invalid answer.
    let mut snapshot = with_pending(snapshot(), "audit:gate", "gate");
    snapshot.decisions.insert(
        "significance:audit:gate".to_string(),
        json!({"effective_band": {"name": "medium"}}),
    );
    let error = apply_decision_answer(&snapshot, &llm("bot"), "audit:gate", "yolo", ANSWERED_AT)
        .unwrap_err();
    assert!(
        matches!(error, AnswerError::AuthorityDenied(_)),
        "{error:?}"
    );
}

#[test]
fn high_and_unsignificant_gates_only_accept_approve_or_reject() {
    for band in [None, Some("high")] {
        let mut snapshot = with_pending(snapshot(), "audit:gate", "gate");
        if let Some(band) = band {
            snapshot = with_band(snapshot, "audit:gate", band);
        }
        let error = apply_decision_answer(
            &snapshot,
            &human("owner-1"),
            "audit:gate",
            "defer",
            ANSWERED_AT,
        )
        .unwrap_err();
        let expected = if band.is_some() {
            "High-band decision requires one of {'approve', 'reject'}, got: 'defer'"
        } else {
            "Invalid audit answer 'defer': must be 'approve' or 'reject'"
        };
        assert_eq!(error.to_string(), expected);

        let applied = apply_decision_answer(
            &snapshot,
            &human("owner-1"),
            "audit:gate",
            "approve",
            ANSWERED_AT,
        )
        .unwrap();
        assert_eq!(
            applied.effect,
            AnswerEffect::AuditCompleted {
                step_id: "gate".to_string()
            }
        );
        assert!(
            applied
                .snapshot
                .completed_steps
                .contains(&"gate".to_string())
        );
        assert!(applied.snapshot.blocked_reason.is_none());
    }
}

#[test]
fn a_high_band_reject_blocks_the_run_forever() {
    let snapshot = with_band(
        with_pending(snapshot(), "audit:gate", "gate"),
        "audit:gate",
        "high",
    );
    let applied = apply_decision_answer(
        &snapshot,
        &human("owner-1"),
        "audit:gate",
        "reject",
        ANSWERED_AT,
    )
    .unwrap();
    assert_eq!(
        applied.snapshot.blocked_reason.as_deref(),
        Some("Audit step 'gate' rejected by owner-1")
    );
    assert!(matches!(applied.effect, AnswerEffect::AuditRejected { .. }));
}

#[test]
fn a_medium_gate_records_the_soft_decision_and_only_decide_solo_completes() {
    let base = with_band(
        with_pending(snapshot(), "audit:gate", "gate"),
        "audit:gate",
        "medium",
    );

    let applied = apply_decision_answer(
        &base,
        &human("owner-1"),
        "audit:gate",
        "decide_solo",
        ANSWERED_AT,
    )
    .unwrap();
    let gate = &applied.snapshot.decisions["soft_gate:audit:gate"];
    assert_eq!(gate["action"], "decide_solo");
    assert_eq!(gate["outcome"], "decide_solo");
    assert_eq!(gate["participants"], json!([]));
    assert_eq!(
        gate["actor"],
        json!({"actor_type": "human", "actor_id": "owner-1"})
    );
    assert_eq!(gate["timestamp"], ANSWERED_AT);
    assert_eq!(
        gate["significance_score"]["effective_band"]["name"],
        "medium"
    );
    assert!(
        applied
            .snapshot
            .completed_steps
            .contains(&"gate".to_string())
    );
    assert!(applied.snapshot.pending_decisions.is_empty());
}

#[test]
fn a_medium_open_stand_up_keeps_the_original_gate_pending() {
    let base = with_band(
        with_pending(snapshot(), "audit:gate", "gate"),
        "audit:gate",
        "medium",
    );
    let original = base.pending_decisions["audit:gate"].clone();

    for answer in ["open_stand_up", "defer"] {
        let applied =
            apply_decision_answer(&base, &human("owner-1"), "audit:gate", answer, ANSWERED_AT)
                .unwrap();
        assert_eq!(
            applied.snapshot.pending_decisions["audit:gate"], original,
            "original request must be restored for {answer}"
        );
        assert_eq!(
            applied.snapshot.decisions["soft_gate:audit:gate"]["action"],
            answer
        );
        assert_eq!(
            applied.snapshot.decisions["soft_gate:audit:gate"]["outcome"],
            Value::Null
        );
        assert!(applied.snapshot.completed_steps.is_empty());
        assert!(matches!(applied.effect, AnswerEffect::SoftGateOpen { .. }));
    }
}

#[test]
fn a_medium_gate_rejects_approve_and_defer_is_not_valid_outside_medium() {
    let medium = with_band(
        with_pending(snapshot(), "audit:gate", "gate"),
        "audit:gate",
        "medium",
    );
    let error = apply_decision_answer(
        &medium,
        &human("owner-1"),
        "audit:gate",
        "approve",
        ANSWERED_AT,
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Medium-band decision requires one of ['decide_solo', 'defer', 'open_stand_up'], got: 'approve'"
    );

    let high = with_band(
        with_pending(snapshot(), "audit:gate", "gate"),
        "audit:gate",
        "high",
    );
    let error = apply_decision_answer(
        &high,
        &human("owner-1"),
        "audit:gate",
        "open_stand_up",
        ANSWERED_AT,
    )
    .unwrap_err();
    assert!(matches!(error, AnswerError::InvalidAnswer(_)));
}

#[test]
fn corrupt_medium_scores_cannot_be_persisted_as_soft_gate_decisions() {
    let base = with_band(
        with_pending(snapshot(), "audit:gate", "gate"),
        "audit:gate",
        "medium",
    );
    for (field, value) in [
        ("dimensions", json!([])),
        ("composite", json!(1)),
        (
            "effective_band",
            json!({"name": "medium", "min_score": 5, "max_score": 4}),
        ),
    ] {
        let mut corrupt = base.clone();
        corrupt
            .decisions
            .get_mut("significance:audit:gate")
            .unwrap()[field] = value;
        let error = apply_decision_answer(
            &corrupt,
            &human("owner-1"),
            "audit:gate",
            "decide_solo",
            ANSWERED_AT,
        )
        .unwrap_err();
        assert!(
            matches!(error, AnswerError::InvalidSignificancePolicy { .. }),
            "{error:?}"
        );
        assert!(!corrupt.decisions.contains_key("soft_gate:audit:gate"));
    }
}

#[test]
fn corrupt_recorded_significance_fails_closed_instead_of_guessing() {
    for malformed in [
        json!({}),
        json!({"effective_band": {"name": "extreme"}}),
        json!({"effective_band": 7}),
        json!({"effective_band": {}}),
    ] {
        let mut snapshot = with_pending(snapshot(), "audit:gate", "gate");
        snapshot
            .decisions
            .insert("significance:audit:gate".to_string(), malformed.clone());
        let error = apply_decision_answer(
            &snapshot,
            &human("owner-1"),
            "audit:gate",
            "approve",
            ANSWERED_AT,
        )
        .unwrap_err();
        assert!(
            matches!(error, AnswerError::InvalidSignificancePolicy { .. }),
            "malformed {malformed} must fail closed, got {error:?}"
        );
    }
}

#[test]
fn a_medium_gate_without_a_complete_score_cannot_record_a_soft_decision() {
    let mut snapshot = with_pending(snapshot(), "audit:gate", "gate");
    snapshot.decisions.insert(
        "significance:audit:gate".to_string(),
        json!({"effective_band": {"name": "medium"}}),
    );
    let error = apply_decision_answer(
        &snapshot,
        &human("owner-1"),
        "audit:gate",
        "decide_solo",
        ANSWERED_AT,
    )
    .unwrap_err();
    assert!(
        matches!(error, AnswerError::InvalidSignificancePolicy { .. }),
        "{error:?}"
    );
}

#[test]
fn an_unknown_strictness_fails_to_deserialize() {
    let raw = json!({
        "run_id": "run-1",
        "mission_key": "m",
        "template_path": "",
        "template_hash": "h",
        "policy_snapshot": {"strictness": "paranoid", "default_route": "same_llm_context", "extras": {}}
    });
    assert!(serde_json::from_value::<RunSnapshot>(raw).is_err());

    for strictness in [Strictness::Off, Strictness::Medium, Strictness::Max] {
        let policy = MissionPolicySnapshot {
            strictness,
            default_route: "same_llm_context".to_string(),
            extras: BTreeMap::new(),
        };
        assert_eq!(
            serde_json::to_value(&policy).unwrap()["strictness"],
            json!(strictness)
        );
    }
}

#[test]
fn applying_an_answer_is_pure_and_leaves_the_caller_snapshot_untouched() {
    let snapshot = with_pending(snapshot(), "input:topic", "s1");
    let before = snapshot.clone();
    let applied = apply_decision_answer(
        &snapshot,
        &human("owner-1"),
        "input:topic",
        "x",
        ANSWERED_AT,
    )
    .unwrap();
    assert_eq!(snapshot, before);
    assert_ne!(applied.snapshot, before);
}
