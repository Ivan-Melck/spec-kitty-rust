//! Durable decision answers under the cursor lock.
//!
//! Authorization and answer semantics are owned by [`sk_runtime`]'s pure
//! `apply_decision_answer`; this module is the transaction: it re-reads the
//! cursor, calls the pure policy, records the denial or persists the answer,
//! and appends the answered event. Read, authorize, mutate and write are one
//! serialized unit.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sk_runtime::{ActorIdentity, AnswerEffect, AnswerError, AppliedAnswer, apply_decision_answer};

use crate::error::RunStoreError;
use crate::events::{
    DECISION_AUTHORITY_DENIED, DECISION_INPUT_ANSWERED, EventRecord, append_records,
};
use crate::model::{RunRef, RunState, now_iso};
use crate::recovery::{Mode, RecoveryReport, reconcile};
use crate::store::{RunStore, persist_state};

/// One answer request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnswerRequest {
    pub decision_id: String,
    pub answer: String,
    pub actor: ActorIdentity,
}

/// The durable result of a committed answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnswerOutcome {
    pub run: RunRef,
    pub state: RunState,
    pub decision_id: String,
    pub answer: String,
    pub authority_role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale_linkage: Option<String>,
    pub effect: AnswerEffect,
    pub event: EventRecord,
    pub recovered: RecoveryReport,
}

/// Apply a pure answer result to the durable cursor, keeping raw pending
/// request JSON for that one decision.
pub(crate) fn apply_pure_answer(state: &mut RunState, applied: &AppliedAnswer) {
    state.completed_steps = applied.snapshot.completed_steps.clone();
    state.blocked_reason = applied.snapshot.blocked_reason.clone();
    state.issued_step_id = applied.snapshot.issued_step_id.clone();
    state.inputs = applied.snapshot.inputs.clone();
    state.decisions = applied.snapshot.decisions.clone();
    if !matches!(applied.effect, AnswerEffect::SoftGateOpen { .. }) {
        state.pending_decisions.remove(&applied.event.decision_id);
    }
}

/// Map the pure policy refusal onto the store's stable error surface.
pub(crate) fn map_answer_error(
    error: AnswerError,
    decision_id: &str,
    answer: &str,
) -> RunStoreError {
    match error {
        AnswerError::UnknownDecision {
            run_id,
            decision_id,
        } => RunStoreError::DecisionNotFound {
            run_id,
            decision_id,
        },
        AnswerError::InvalidActorIdentity => RunStoreError::InvalidActor {
            reason: "actor_id must be non-empty".to_string(),
        },
        AnswerError::AuthorityDenied(denial) => RunStoreError::AuditOwnerRequired {
            run_id: denial.run_id.clone(),
            decision_id: denial.decision_id.clone(),
            reason: denial.reason.clone(),
        },
        AnswerError::InvalidAnswer(reason) => RunStoreError::AuditAnswerInvalid {
            decision_id: decision_id.to_string(),
            answer: answer.to_string(),
            reason,
        },
        AnswerError::LlmNotDelegated {
            actor_id,
            decision_id,
        } => RunStoreError::LlmDelegationRequired {
            decision_id,
            actor_id,
            reason: "LLM actor is not delegated for this decision".to_string(),
        },
        AnswerError::DelegationMissingRationale { decision_id } => {
            RunStoreError::LlmDelegationRequired {
                decision_id,
                actor_id: "<unknown>".to_string(),
                reason: "LLM delegation must include non-empty rationale_linkage".to_string(),
            }
        }
        AnswerError::InvalidSignificancePolicy {
            decision_id,
            message,
        } => RunStoreError::StateCorrupt {
            path: std::path::PathBuf::from("state.json"),
            message: format!("invalid recorded significance policy for {decision_id:?}: {message}"),
        },
    }
}

impl RunStore {
    /// Answer a pending decision.
    ///
    /// An already-answered or unknown decision is refused with
    /// [`RunStoreError::DecisionNotFound`] and writes nothing, matching the
    /// baseline contract (there is no silent replay of a second answer).
    pub fn answer(
        &self,
        run: &RunRef,
        request: &AnswerRequest,
    ) -> Result<AnswerOutcome, RunStoreError> {
        let run = self.resolve_ref(run)?;
        let _lock = self.lock_cursor(&run)?;
        let (state, recovered, _consistency) = reconcile(self, &run, Mode::Repair)?;

        if !state.pending_decisions.contains_key(&request.decision_id) {
            return Err(RunStoreError::DecisionNotFound {
                run_id: run.run_id.clone(),
                decision_id: request.decision_id.clone(),
            });
        }

        let snapshot = state.to_snapshot()?;
        let answered_at = now_iso();
        let applied = match apply_decision_answer(
            &snapshot,
            &request.actor,
            &request.decision_id,
            &request.answer,
            &answered_at,
        ) {
            Ok(applied) => applied,
            Err(AnswerError::AuthorityDenied(denial)) => {
                // Upstream records a denial as a journal fact without touching
                // the cursor, so an operator can see who tried.
                let payload = serde_json::to_value(&denial).map_err(|error| {
                    RunStoreError::TemplateInvalid {
                        message: format!("denial payload is not serializable: {error}"),
                    }
                })?;
                append_records(
                    &self.events_path(&run),
                    &[EventRecord {
                        event_type: DECISION_AUTHORITY_DENIED.to_string(),
                        timestamp: answered_at,
                        payload,
                    }],
                )?;
                return Err(map_answer_error(
                    AnswerError::AuthorityDenied(denial),
                    &request.decision_id,
                    &request.answer,
                ));
            }
            Err(error) => {
                return Err(map_answer_error(
                    error,
                    &request.decision_id,
                    &request.answer,
                ));
            }
        };

        let mut next_state = state.clone();
        apply_pure_answer(&mut next_state, &applied);
        next_state.validate(Some(&run.run_id))?;
        persist_state(&run.run_dir, &next_state)?;

        let event = EventRecord {
            event_type: DECISION_INPUT_ANSWERED.to_string(),
            timestamp: answered_at,
            payload: serde_json::to_value(&applied.event).map_err(|error| {
                RunStoreError::TemplateInvalid {
                    message: format!("answered event is not serializable: {error}"),
                }
            })?,
        };
        append_records(&self.events_path(&run), std::slice::from_ref(&event))?;

        Ok(AnswerOutcome {
            run,
            state: next_state,
            decision_id: request.decision_id.clone(),
            answer: request.answer.clone(),
            authority_role: applied.authority.authority_role,
            rationale_linkage: applied.authority.rationale_linkage,
            effect: applied.effect,
            event,
            recovered,
        })
    }
}

/// Answer-event payload for a decision whose event is being reconstructed from
/// the durable cursor: same shape as the pure API's `DecisionAnswered`.
pub(crate) fn answered_event_payload(
    run_id: &str,
    decision_id: &str,
    answer: &str,
    actor: &ActorIdentity,
    mission_id: Option<&str>,
    mission_slug: Option<&str>,
) -> Result<Value, RunStoreError> {
    serde_json::to_value(sk_runtime::DecisionAnswered {
        run_id: run_id.to_string(),
        decision_id: decision_id.to_string(),
        answer: answer.to_string(),
        actor: actor.clone(),
        mission_id: mission_id.map(str::to_string),
        mission_slug: mission_slug.map(str::to_string),
    })
    .map_err(|error| RunStoreError::TemplateInvalid {
        message: format!("answered event is not serializable: {error}"),
    })
}
