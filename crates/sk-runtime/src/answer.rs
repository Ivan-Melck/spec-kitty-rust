//! Pure decision-answer authorization and application.
//!
//! This module owns the *semantics* of answering a pending decision, without
//! writing anything. It reproduces the upstream `provide_decision_answer` /
//! `_apply_decision_answer` policy as two pure steps:
//!
//! 1. [`authorize_decision_answer`] decides whether `actor` may answer and
//!    with which authority. It returns the authority record (the metadata the
//!    run store persists on the answer) or an [`AnswerError`]. An audit denial
//!    is [`AnswerError::AuthorityDenied`], which carries the exact
//!    `DecisionAuthorityDenied` payload the store must append; the snapshot is
//!    unchanged.
//! 2. [`apply_decision_answer`] authorizes and then returns a new
//!    [`RunSnapshot`] plus the `DecisionInputAnswered` event payload, so the
//!    store can persist state and append the event atomically in its own
//!    transaction.
//!
//! # Authority rules
//!
//! * An **audit** decision (`audit:<step>`) requires a human actor whose
//!   `actor_id` equals the run's `mission_owner_id` input. The authority role
//!   is `mission_owner`.
//! * An **input** decision (`input:<key>`) is answerable by the actor's own
//!   type. An LLM actor additionally needs a delegation record under
//!   `inputs.llm_delegations[<decision_id>]` (or `["*"]`) with a non-empty
//!   `rationale_linkage`; its authority role is the record's `authority_role`
//!   when that is a string, else `delegated_llm`.
//! * Audit answers are validated against the recorded significance band:
//!   `medium` accepts `decide_solo` / `open_stand_up` / `defer`, `high` accepts
//!   `approve` / `reject`, and a gate with no recorded significance accepts
//!   `approve` / `reject`.
//!
//! # Application rules
//!
//! * An input decision writes `answer` into `inputs[<key>]`.
//! * A `high`/unsignificant audit `approve` completes the audit step; `reject`
//!   sets `blocked_reason` to `Audit step '<step>' rejected by <actor>`.
//! * A `medium` soft gate records `soft_gate:<decision_id>`; `decide_solo`
//!   completes the audit step, while `open_stand_up`/`defer` leave the gate
//!   pending with its original request.
//!
//! Timestamps are supplied by the caller (`answered_at`) so the operation stays
//! deterministic and testable; the run-store owner passes the wall clock.
//!
//! # Public contract
//!
//! ```text
//! authorize_decision_answer(
//!     snapshot: &RunSnapshot,
//!     actor: &ActorIdentity,
//!     decision_id: &str,
//!     answer: &str,
//! ) -> Result<DecisionAuthority, AnswerError>
//!
//! apply_decision_answer(
//!     snapshot: &RunSnapshot,
//!     actor: &ActorIdentity,
//!     decision_id: &str,
//!     answer: &str,
//!     answered_at: &str,
//! ) -> Result<AppliedAnswer, AnswerError>
//! ```
//!
//! `authorize_decision_answer` is the read-only authority check: the caller gets
//! the metadata to persist, or a refusal. `apply_decision_answer` authorizes and
//! then returns the complete pending->answered transition: the new snapshot, the
//! `decisions[decision_id]` record, the [`AnswerEffect`] and the
//! `DecisionInputAnswered` event body. Neither function opens a file, takes a
//! lock or writes an event, so the run-store owner can apply the snapshot and
//! append the event in one transaction.
//!
//! The caller owns the `answered_at` timestamp because the upstream reference
//! reads the wall clock; passing it in keeps the pure operation deterministic
//! and testable.
//!
//! # Deliberate fail-closed differences
//!
//! A malformed `significance:<decision_id>` record, or a band name outside the
//! closed set, is a typed error here. Upstream silently treats it as "no
//! significance" and falls back to approve/reject; inventing an answer policy
//! from corrupt state is not safe.
//!
//! # Known divergence
//!
//! When an already-pending `audit:` decision is re-evaluated as MEDIUM by
//! [`crate::plan_advance_with_actor`], upstream rebuilds the decision from
//! scratch and therefore loses `reason: "pending_decision"` (it serializes
//! `reason: null`). This crate keeps the pending requirement and its reason,
//! because dropping the reason misrepresents why progress is blocked. A HIGH
//! pending gate is unaffected: both sides keep the pending decision.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

use crate::raci::bind_actor;
use crate::significance::{RoutingBandName, SignificanceScore, SoftGateAction, SoftGateDecision};
use crate::snapshot::{ActorIdentity, ActorType, RunSnapshot};

/// The prefix of an audit decision id.
pub const AUDIT_PREFIX: &str = "audit:";

/// The prefix of an input-keyed decision id.
pub const INPUT_PREFIX: &str = "input:";

const AUDIT_AUTHORITY_ROLE: &str = "mission_owner";
const DEFAULT_LLM_AUTHORITY_ROLE: &str = "delegated_llm";

/// Why an answer was refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AnswerError {
    /// The decision is not pending in the run.
    #[error("Decision '{decision_id}' not found in pending_decisions for run '{run_id}'")]
    UnknownDecision { run_id: String, decision_id: String },

    /// The actor identity is unusable (an empty `actor_id`).
    #[error("invalid actor identity: actor_id must be non-empty")]
    InvalidActorIdentity,

    /// An audit decision was answered by an actor that may not answer it.
    ///
    /// The payload is the `DecisionAuthorityDenied` event body the store must
    /// append. Nothing else is written.
    #[error("{}", .0.reason)]
    AuthorityDenied(Box<AuthorityDenial>),

    /// An audit answer is not valid for the gate's significance band.
    #[error("{0}")]
    InvalidAnswer(String),

    /// An LLM actor answered a decision without a matching delegation record.
    #[error("LLM actor '{actor_id}' is not delegated for decision '{decision_id}'")]
    LlmNotDelegated {
        actor_id: String,
        decision_id: String,
    },

    /// An LLM delegation is missing its required justification.
    #[error("LLM delegation for decision '{decision_id}' must include non-empty rationale_linkage")]
    DelegationMissingRationale { decision_id: String },

    /// The recorded significance policy is malformed and cannot be interpreted.
    #[error("invalid recorded significance policy for '{decision_id}': {message}")]
    InvalidSignificancePolicy {
        decision_id: String,
        message: String,
    },
}

/// The authority metadata persisted on an answer record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionAuthority {
    pub actor_type: ActorType,
    pub actor_id: String,
    pub authority_role: String,
    pub rationale_linkage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raci_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub override_reason: Option<String>,
}

/// The `DecisionAuthorityDenied` event body produced by an audit denial.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorityDenial {
    pub run_id: String,
    pub decision_id: String,
    pub actor_type: ActorType,
    pub actor_id: String,
    pub authority_role: String,
    pub rationale_linkage: Option<String>,
    pub reason: String,
    pub raci_source: Option<String>,
    pub override_reason: Option<String>,
}

/// The `DecisionInputAnswered` event body produced by an accepted answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionAnswered {
    pub run_id: String,
    pub decision_id: String,
    pub answer: String,
    pub actor: ActorIdentity,
    pub mission_id: Option<String>,
    pub mission_slug: Option<String>,
}

/// What applying the answer did to run progression.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AnswerEffect {
    /// An input-keyed decision wrote the value into `inputs`.
    InputRecorded { input_key: String },
    /// An audit gate was approved (or a medium `decide_solo`), completing it.
    AuditCompleted { step_id: String },
    /// An audit gate was rejected, permanently blocking the run.
    AuditRejected { step_id: String, reason: String },
    /// A medium-band soft gate was recorded but left open.
    SoftGateOpen {
        step_id: String,
        action: SoftGateAction,
    },
    /// A pending decision with no recognised prefix was answered: the answer is
    /// recorded and the pending entry cleared, with no progression effect.
    RecordedOnly { decision_id: String },
}

/// The result of applying an answer to a snapshot.
#[derive(Debug, Clone, PartialEq)]
pub struct AppliedAnswer {
    /// The new run snapshot: nothing here is persisted by this crate.
    pub snapshot: RunSnapshot,
    /// The authority that accepted the answer.
    pub authority: DecisionAuthority,
    /// The `decisions[<decision_id>]` record the store persists.
    pub decision_record: Value,
    /// What happened to run progression.
    pub effect: AnswerEffect,
    /// The `DecisionInputAnswered` event body the store appends.
    pub event: DecisionAnswered,
}

fn audit_denial_reason(actor: &ActorIdentity, snapshot: &RunSnapshot) -> Option<String> {
    let mission_owner_id = mission_owner_id(snapshot);
    if actor.actor_type != ActorType::Human {
        return Some("Audit decisions require a human actor".to_string());
    }
    let Some(owner) = mission_owner_id else {
        return Some("Audit decisions require mission_owner_id to be set in inputs".to_string());
    };
    if actor.actor_id != owner {
        return Some(format!("Audit decisions require mission owner '{owner}'"));
    }
    None
}

fn mission_owner_id(snapshot: &RunSnapshot) -> Option<String> {
    let value = snapshot.inputs.get("mission_owner_id")?.as_str()?.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn raci_binding(snapshot: &RunSnapshot, decision_id: &str) -> (Option<String>, Option<String>) {
    let step_id = if let Some(step_id) = decision_id.strip_prefix(AUDIT_PREFIX) {
        Some(step_id.to_string())
    } else if decision_id.starts_with(INPUT_PREFIX) {
        snapshot.issued_step_id.clone()
    } else {
        None
    };
    let Some(step_id) = step_id else {
        return (None, None);
    };
    let Some(record) = snapshot.decisions.get(&format!("raci:{step_id}")) else {
        return (None, None);
    };
    let source = record
        .get("source")
        .and_then(Value::as_str)
        .map(str::to_string);
    let override_reason = record
        .get("override_reason")
        .and_then(Value::as_str)
        .map(str::to_string);
    (source, override_reason)
}

/// The recorded effective significance band of an audit decision.
///
/// `Ok(None)` means no significance was evaluated for this gate (the plain
/// approve/reject policy). A recorded but malformed record fails closed.
fn significance_band(
    snapshot: &RunSnapshot,
    decision_id: &str,
) -> Result<Option<RoutingBandName>, AnswerError> {
    let key = format!("significance:{decision_id}");
    let Some(record) = snapshot.decisions.get(&key) else {
        return Ok(None);
    };
    let band_value =
        record
            .get("effective_band")
            .ok_or_else(|| AnswerError::InvalidSignificancePolicy {
                decision_id: decision_id.to_string(),
                message: "recorded score has no effective_band".to_string(),
            })?;
    let name = match band_value {
        Value::String(name) => name.as_str(),
        Value::Object(map) => map.get("name").and_then(Value::as_str).ok_or_else(|| {
            AnswerError::InvalidSignificancePolicy {
                decision_id: decision_id.to_string(),
                message: "effective_band has no name".to_string(),
            }
        })?,
        _ => {
            return Err(AnswerError::InvalidSignificancePolicy {
                decision_id: decision_id.to_string(),
                message: "effective_band is neither a name nor a band object".to_string(),
            });
        }
    };
    let band =
        RoutingBandName::from_name(name).ok_or_else(|| AnswerError::InvalidSignificancePolicy {
            decision_id: decision_id.to_string(),
            message: format!("unknown band name {name:?}"),
        })?;
    Ok(Some(band))
}

fn validate_audit_answer(band: Option<RoutingBandName>, answer: &str) -> Result<(), AnswerError> {
    match band {
        Some(RoutingBandName::Medium) => {
            if SoftGateAction::from_name(answer).is_none() {
                return Err(AnswerError::InvalidAnswer(format!(
                    "Medium-band decision requires one of ['decide_solo', 'defer', 'open_stand_up'], got: '{answer}'"
                )));
            }
        }
        Some(RoutingBandName::High) => {
            if !matches!(answer, "approve" | "reject") {
                return Err(AnswerError::InvalidAnswer(format!(
                    "High-band decision requires one of {{'approve', 'reject'}}, got: '{answer}'"
                )));
            }
        }
        Some(RoutingBandName::Low) | None => {
            if !matches!(answer, "approve" | "reject") {
                return Err(AnswerError::InvalidAnswer(format!(
                    "Invalid audit answer '{answer}': must be 'approve' or 'reject'"
                )));
            }
        }
    }
    Ok(())
}

fn delegation_record<'a>(
    inputs: &'a BTreeMap<String, Value>,
    decision_id: &str,
) -> Option<&'a Value> {
    let delegations = inputs.get("llm_delegations")?.as_object()?;
    delegations
        .get(decision_id)
        .or_else(|| delegations.get("*"))
        .filter(|record| record.is_object())
}

/// Authorize one decision answer without changing any state.
///
/// `decision_id` must be pending in `snapshot`. The returned
/// [`DecisionAuthority`] is the metadata to persist on the answer record.
///
/// An audit denial is [`AnswerError::AuthorityDenied`] and carries the
/// `DecisionAuthorityDenied` body the store appends. Answer validation happens
/// only after audit authority passes, matching upstream ordering.
pub fn authorize_decision_answer(
    snapshot: &RunSnapshot,
    actor: &ActorIdentity,
    decision_id: &str,
    answer: &str,
) -> Result<DecisionAuthority, AnswerError> {
    if actor.actor_id.is_empty() {
        return Err(AnswerError::InvalidActorIdentity);
    }
    if !snapshot.pending_decisions.contains_key(decision_id) {
        return Err(AnswerError::UnknownDecision {
            run_id: snapshot.run_id.clone(),
            decision_id: decision_id.to_string(),
        });
    }
    let (raci_source, override_reason) = raci_binding(snapshot, decision_id);

    if decision_id.starts_with(AUDIT_PREFIX) {
        if let Some(reason) = audit_denial_reason(actor, snapshot) {
            return Err(AnswerError::AuthorityDenied(Box::new(AuthorityDenial {
                run_id: snapshot.run_id.clone(),
                decision_id: decision_id.to_string(),
                actor_type: actor.actor_type,
                actor_id: actor.actor_id.clone(),
                authority_role: AUDIT_AUTHORITY_ROLE.to_string(),
                rationale_linkage: None,
                reason,
                raci_source,
                override_reason,
            })));
        }
        let band = significance_band(snapshot, decision_id)?;
        validate_audit_answer(band, answer)?;
        return Ok(DecisionAuthority {
            actor_type: actor.actor_type,
            actor_id: actor.actor_id.clone(),
            authority_role: AUDIT_AUTHORITY_ROLE.to_string(),
            rationale_linkage: None,
            raci_source,
            override_reason,
        });
    }

    if actor.actor_type == ActorType::Llm {
        let Some(record) = delegation_record(&snapshot.inputs, decision_id) else {
            return Err(AnswerError::LlmNotDelegated {
                actor_id: actor.actor_id.clone(),
                decision_id: decision_id.to_string(),
            });
        };
        let authority_role = record
            .get("authority_role")
            .and_then(Value::as_str)
            .unwrap_or(DEFAULT_LLM_AUTHORITY_ROLE)
            .to_string();
        let rationale = record
            .get("rationale_linkage")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| AnswerError::DelegationMissingRationale {
                decision_id: decision_id.to_string(),
            })?;
        return Ok(DecisionAuthority {
            actor_type: actor.actor_type,
            actor_id: actor.actor_id.clone(),
            authority_role,
            rationale_linkage: Some(rationale.to_string()),
            raci_source,
            override_reason,
        });
    }

    Ok(DecisionAuthority {
        actor_type: actor.actor_type,
        actor_id: actor.actor_id.clone(),
        authority_role: actor.actor_type.to_string(),
        rationale_linkage: None,
        raci_source,
        override_reason,
    })
}

fn decision_record(
    decision_id: &str,
    answer: &str,
    actor: &ActorIdentity,
    answered_at: &str,
    authority: &DecisionAuthority,
) -> Value {
    let mut record = json!({
        "decision_id": decision_id,
        "answer": answer,
        "answered_by": actor_wire(actor),
        "answered_at": answered_at,
        "actor_type": authority.actor_type.to_string(),
        "actor_id": authority.actor_id,
        "authority_role": authority.authority_role,
        "rationale_linkage": authority.rationale_linkage,
    });
    if let Some(source) = &authority.raci_source {
        record["raci_source"] = Value::String(source.clone());
    }
    if let Some(reason) = &authority.override_reason {
        record["override_reason"] = Value::String(reason.clone());
    }
    record
}

fn actor_wire(actor: &ActorIdentity) -> Value {
    json!({
        "actor_id": actor.actor_id,
        "actor_type": actor.actor_type.to_string(),
        "display_name": actor.display_name,
        "provider": actor.provider,
        "model": actor.model,
        "tool": actor.tool,
    })
}

fn recorded_score(
    snapshot: &RunSnapshot,
    decision_id: &str,
) -> Result<SignificanceScore, AnswerError> {
    let record = snapshot
        .decisions
        .get(&format!("significance:{decision_id}"))
        .ok_or_else(|| AnswerError::InvalidSignificancePolicy {
            decision_id: decision_id.to_string(),
            message: "no recorded significance score".to_string(),
        })?;
    let score: SignificanceScore = serde_json::from_value(record.clone()).map_err(|error| {
        AnswerError::InvalidSignificancePolicy {
            decision_id: decision_id.to_string(),
            message: error.to_string(),
        }
    })?;
    score
        .validate()
        .map_err(|error| AnswerError::InvalidSignificancePolicy {
            decision_id: decision_id.to_string(),
            message: error.to_string(),
        })?;
    Ok(score)
}

/// Apply one decision answer to a snapshot, without persisting anything.
///
/// `answered_at` is the caller-supplied timestamp recorded on the answer (and,
/// for a medium-band gate, on the soft-gate record). On success the returned
/// [`AppliedAnswer`] holds the new snapshot, the authority, the persisted
/// decision record, the progression effect and the `DecisionInputAnswered`
/// event body. On failure no snapshot is produced.
pub fn apply_decision_answer(
    snapshot: &RunSnapshot,
    actor: &ActorIdentity,
    decision_id: &str,
    answer: &str,
    answered_at: &str,
) -> Result<AppliedAnswer, AnswerError> {
    let authority = authorize_decision_answer(snapshot, actor, decision_id, answer)?;
    let record = decision_record(decision_id, answer, actor, answered_at, &authority);

    let pending_request = snapshot
        .pending_decisions
        .get(decision_id)
        .cloned()
        .ok_or_else(|| AnswerError::UnknownDecision {
            run_id: snapshot.run_id.clone(),
            decision_id: decision_id.to_string(),
        })?;

    let mut decisions = snapshot.decisions.clone();
    let mut inputs = snapshot.inputs.clone();
    let mut completed_steps = snapshot.completed_steps.clone();
    let mut blocked_reason = snapshot.blocked_reason.clone();
    let mut pending = snapshot.pending_decisions.clone();

    decisions.insert(decision_id.to_string(), record.clone());
    pending.remove(decision_id);

    let effect = if let Some(step_id) = decision_id.strip_prefix(AUDIT_PREFIX) {
        let step_id = step_id.to_string();
        match significance_band(snapshot, decision_id)? {
            Some(RoutingBandName::Medium) => apply_soft_gate(
                snapshot,
                decision_id,
                &step_id,
                answer,
                actor,
                answered_at,
                &mut decisions,
                &mut completed_steps,
            )?,
            _ => {
                if answer == "approve" {
                    if !completed_steps.contains(&step_id) {
                        completed_steps.push(step_id.clone());
                    }
                    AnswerEffect::AuditCompleted { step_id }
                } else {
                    let reason = format!("Audit step '{step_id}' rejected by {}", actor.actor_id);
                    blocked_reason = Some(reason.clone());
                    AnswerEffect::AuditRejected { step_id, reason }
                }
            }
        }
    } else if let Some(input_key) = decision_id.strip_prefix(INPUT_PREFIX) {
        inputs.insert(input_key.to_string(), Value::String(answer.to_string()));
        AnswerEffect::InputRecorded {
            input_key: input_key.to_string(),
        }
    } else {
        // An unprefixed pending decision still records the answer and clears the
        // pending entry; it has no progression effect, matching upstream.
        AnswerEffect::RecordedOnly {
            decision_id: decision_id.to_string(),
        }
    };

    if matches!(effect, AnswerEffect::SoftGateOpen { .. }) {
        // Keep the gate pending with its ORIGINAL request.
        pending.insert(decision_id.to_string(), pending_request);
    }

    let event = DecisionAnswered {
        run_id: snapshot.run_id.clone(),
        decision_id: decision_id.to_string(),
        answer: answer.to_string(),
        actor: actor.clone(),
        mission_id: snapshot.mission_id.clone(),
        mission_slug: snapshot.mission_slug.clone(),
    };

    let mut applied = snapshot.clone();
    applied.decisions = decisions;
    applied.inputs = inputs;
    applied.completed_steps = completed_steps;
    applied.blocked_reason = blocked_reason;
    applied.pending_decisions = pending;

    Ok(AppliedAnswer {
        snapshot: applied,
        authority,
        decision_record: record,
        effect,
        event,
    })
}

#[allow(clippy::too_many_arguments)]
fn apply_soft_gate(
    snapshot: &RunSnapshot,
    decision_id: &str,
    step_id: &str,
    answer: &str,
    actor: &ActorIdentity,
    answered_at: &str,
    decisions: &mut BTreeMap<String, Value>,
    completed_steps: &mut Vec<String>,
) -> Result<AnswerEffect, AnswerError> {
    let action = SoftGateAction::from_name(answer).ok_or_else(|| {
        AnswerError::InvalidAnswer(format!(
            "Medium-band decision requires one of ['decide_solo', 'defer', 'open_stand_up'], got: '{answer}'"
        ))
    })?;
    let score = recorded_score(snapshot, decision_id)?;
    let soft_gate = SoftGateDecision {
        decision_id: decision_id.to_string(),
        action,
        actor: bind_actor(actor.actor_type, Some(&actor.actor_id)),
        timestamp: answered_at.to_string(),
        significance_score: score,
        participants: Vec::new(),
        outcome: (action == SoftGateAction::DecideSolo).then_some(action),
        rationale: None,
    };
    decisions.insert(
        format!("soft_gate:{decision_id}"),
        serde_json::to_value(&soft_gate).expect("soft gate record is JSON"),
    );

    if action == SoftGateAction::DecideSolo {
        if !completed_steps.contains(&step_id.to_string()) {
            completed_steps.push(step_id.to_string());
        }
        Ok(AnswerEffect::AuditCompleted {
            step_id: step_id.to_string(),
        })
    } else {
        Ok(AnswerEffect::SoftGateOpen {
            step_id: step_id.to_string(),
            action,
        })
    }
}
