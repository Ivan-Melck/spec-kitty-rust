//! Pure result application and stale-state-checked advance planning.
//!
//! The library does not persist anything. [`plan_advance`] computes the next
//! snapshot and decision, folding in the audit significance policy exactly
//! where the upstream engine does; [`commit_advance`] only hands back that
//! snapshot after proving the plan still applies to the live run state.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::decision::{DecisionRequirement, NextDecision};
use crate::error::RuntimeError;
use crate::planner::plan_next;
use crate::raci::{ResolvedRaciSnapshot, StepRef, infer_raci, resolve_raci};
use crate::significance::{
    RoutingBandName, SignificanceEvaluated, SoftGateAction, evaluate_significance,
    parse_band_cutoffs_from_policy,
};
use crate::snapshot::RunSnapshot;
use crate::template::MissionTemplate;

/// The result reported for the currently issued step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepResult {
    Success,
    Failed,
    Blocked,
}

impl StepResult {
    /// The upstream wire value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failed => "failed",
            Self::Blocked => "blocked",
        }
    }
}

/// The immutable result of applying one step result to a snapshot.
#[derive(Debug, Clone, PartialEq)]
pub struct AppliedResult {
    pub snapshot: RunSnapshot,
    pub completed_step_id: Option<String>,
}

/// Apply the issued step's result to a snapshot without writing anything.
///
/// `expected_issued_step` is the step the caller actually evaluated. When it
/// is supplied, a different (or missing) issued step is a stale plan and the
/// call fails closed: it never completes a step the caller did not run.
///
/// On success the issued step is appended to `completed_steps` once and the
/// issued marker is cleared. On `failed`/`blocked` the issued marker is
/// cleared and a blocking reason is recorded, but the step is not completed.
/// With no issued step the snapshot is returned unchanged.
pub fn apply_result(
    snapshot: &RunSnapshot,
    expected_issued_step: Option<&str>,
    result: StepResult,
) -> Result<AppliedResult, RuntimeError> {
    if let Some(expected) = expected_issued_step
        && snapshot.issued_step_id.as_deref() != Some(expected)
    {
        return Err(RuntimeError::StaleIssuedStep {
            run_id: snapshot.run_id.clone(),
            expected: Some(expected.to_string()),
            found: snapshot.issued_step_id.clone(),
        });
    }

    let Some(completed_step_id) = snapshot.issued_step_id.clone() else {
        return Ok(AppliedResult {
            snapshot: snapshot.clone(),
            completed_step_id: None,
        });
    };

    let mut applied = snapshot.clone();
    applied.issued_step_id = None;

    match result {
        StepResult::Success => {
            if !applied
                .completed_steps
                .iter()
                .any(|step| step == &completed_step_id)
            {
                applied.completed_steps.push(completed_step_id.clone());
            }
        }
        StepResult::Failed => {
            applied.blocked_reason = Some(format!(
                "Previous step '{completed_step_id}' failed; manual intervention required."
            ));
        }
        StepResult::Blocked => {
            applied.blocked_reason = Some(format!(
                "Previous step '{completed_step_id}' reported blocked state."
            ));
        }
    }

    Ok(AppliedResult {
        snapshot: applied,
        completed_step_id: Some(completed_step_id),
    })
}

/// A pure plan for advancing a run: the source snapshot, the snapshot the
/// result produced, the selected next decision, and the completed step.
#[derive(Debug, Clone, PartialEq)]
pub struct AdvancePlan {
    pub source: RunSnapshot,
    pub snapshot: RunSnapshot,
    pub decision: NextDecision,
    pub completed_step_id: Option<String>,
    /// The significance evaluation of an audit gate, when one was evaluated.
    /// The run-store owner emits this as `SignificanceEvaluated`.
    pub significance: Option<SignificanceEvaluated>,
    /// The RACI record of the *issued* step, when a step was issued.
    pub raci: Option<ResolvedRaciSnapshot>,
    /// The step id the caller evaluated before this advance was planned (the
    /// bootstrap issued step). [`commit_advance`] refuses the plan when the
    /// live issued step no longer equals it.
    pub expected_issued_step: Option<String>,
}

/// Apply a step result and plan the next decision without persisting anything.
///
/// This is the signature-preserving wrapper over [`plan_advance_with_actor`]:
/// the agent id is read from the snapshot's `actor_context["agent_id"]`
/// extension when present.
pub fn plan_advance(
    template: &MissionTemplate,
    snapshot: &RunSnapshot,
    current_template_hash: &str,
    expected_issued_step: Option<&str>,
    result: StepResult,
) -> Result<AdvancePlan, RuntimeError> {
    let agent_id = snapshot
        .actor_context
        .get("agent_id")
        .and_then(Value::as_str);
    plan_advance_with_actor(
        template,
        snapshot,
        current_template_hash,
        expected_issued_step,
        result,
        agent_id,
    )
}

/// Apply a step result, plan the next decision, and fold in engine policy.
///
/// Policy folded in here (matching upstream `engine.plan_advance`):
///
/// * an audit gate with a declared significance block is evaluated; the score
///   is recorded under `decisions["significance:<decision_id>"]` and the
///   gate's RACI under `decisions["raci:<step>"]`,
/// * a LOW gate auto-proceeds (the audit step completes and the run re-plans),
/// * a MEDIUM gate offers `decide_solo` / `open_stand_up` / `defer`,
/// * a HIGH gate keeps the approve/reject checkpoint,
/// * the issued step's RACI is recorded under `decisions["raci:<step>"]`.
///
/// `agent_id` (when supplied) is merged into the actor context as
/// `agent_id` and used to resolve the inferred LLM RACI role.
pub fn plan_advance_with_actor(
    template: &MissionTemplate,
    snapshot: &RunSnapshot,
    current_template_hash: &str,
    expected_issued_step: Option<&str>,
    result: StepResult,
    agent_id: Option<&str>,
) -> Result<AdvancePlan, RuntimeError> {
    let applied = apply_result(snapshot, expected_issued_step, result)?;
    let source = snapshot.clone();
    let mut working = applied.snapshot;
    if let Some(agent_id) = agent_id {
        working
            .actor_context
            .insert("agent_id".to_string(), Value::String(agent_id.to_string()));
    }
    let policy = working.policy_snapshot.clone();

    let mut decision = plan_next(template, &working, current_template_hash)?;

    let mut significance = None;
    if let Some(audit_step_id) = audit_step_id(&decision)
        && let Some(StepRef::Audit(step)) = crate::planner::find_step(template, &audit_step_id)
        && let Some(declared) = step.significance.clone()
    {
        let cutoffs = parse_band_cutoffs_from_policy(&policy)?;
        let score = evaluate_significance(
            &declared.dimensions,
            &declared.hard_triggers,
            cutoffs.as_ref(),
        )?;
        working.decisions.insert(
            format!("significance:audit:{audit_step_id}"),
            serde_json::to_value(&score).expect("score is JSON"),
        );

        let raci_inputs = raci_inputs(&working.inputs, agent_id);
        let step_ref = StepRef::Audit(step);
        let resolved = match resolve_raci(step_ref, &raci_inputs, &policy) {
            Ok(resolved) => resolved,
            Err(_) => infer_raci(step_ref, &policy, Some(&raci_inputs)),
        };
        working.decisions.insert(
            format!("raci:{audit_step_id}"),
            serde_json::to_value(&resolved).expect("raci record is JSON"),
        );

        let event = SignificanceEvaluated::for_gate(
            &working.run_id,
            &format!("audit:{audit_step_id}"),
            &audit_step_id,
            &score,
        );
        significance = Some(event);

        match score.effective_band_name() {
            RoutingBandName::Low => {
                if !working
                    .completed_steps
                    .iter()
                    .any(|step| step == &audit_step_id)
                {
                    working.completed_steps.push(audit_step_id.clone());
                }
                decision = plan_next(template, &working, current_template_hash)?;
            }
            RoutingBandName::Medium => match &mut decision {
                NextDecision::DecisionRequired(required) => match &mut required.requirement {
                    DecisionRequirement::AuditCheckpoint { options, .. } => {
                        *options = medium_band_options();
                    }
                    DecisionRequirement::PendingDecision {
                        decision_id,
                        step_id,
                        question,
                        ..
                    } => {
                        // The engine rebuilds a MEDIUM decision after evaluation,
                        // clearing the pending reason but preserving its null title.
                        required.requirement = DecisionRequirement::AuditCheckpoint {
                            decision_id: decision_id.clone(),
                            step_id: step_id.clone(),
                            step_title: None,
                            question: question.clone(),
                            options: medium_band_options(),
                        };
                    }
                    DecisionRequirement::MissingRequiredInput { .. } => {}
                },
                NextDecision::Step(_) | NextDecision::Blocked(_) | NextDecision::Terminal(_) => {}
            },
            RoutingBandName::High => {}
        }
    }

    let mut raci = None;
    if let NextDecision::Step(step_decision) = &decision
        && let Some(step_ref) = crate::planner::find_step(template, &step_decision.step_id)
    {
        let raci_inputs = raci_inputs(&working.inputs, agent_id);
        let resolved = match resolve_raci(step_ref, &raci_inputs, &policy) {
            Ok(resolved) => resolved,
            Err(_) => infer_raci(step_ref, &policy, Some(&raci_inputs)),
        };
        working.decisions.insert(
            format!("raci:{}", step_decision.step_id),
            serde_json::to_value(&resolved).expect("raci record is JSON"),
        );
        raci = Some(resolved);
    }

    Ok(AdvancePlan {
        source,
        snapshot: working,
        decision,
        completed_step_id: applied.completed_step_id,
        significance,
        raci,
        expected_issued_step: expected_issued_step.map(str::to_string),
    })
}

fn audit_step_id(decision: &NextDecision) -> Option<String> {
    let NextDecision::DecisionRequired(required) = decision else {
        return None;
    };
    let decision_id = match &required.requirement {
        DecisionRequirement::AuditCheckpoint { decision_id, .. }
        | DecisionRequirement::PendingDecision { decision_id, .. } => decision_id,
        DecisionRequirement::MissingRequiredInput { .. } => return None,
    };
    decision_id
        .strip_prefix(crate::answer::AUDIT_PREFIX)
        .map(str::to_string)
}

fn medium_band_options() -> Vec<String> {
    vec![
        SoftGateAction::DecideSolo.as_str().to_string(),
        SoftGateAction::OpenStandUp.as_str().to_string(),
        SoftGateAction::Defer.as_str().to_string(),
    ]
}

fn raci_inputs(
    inputs: &BTreeMap<String, Value>,
    agent_id: Option<&str>,
) -> BTreeMap<String, Value> {
    let mut resolved = inputs.clone();
    if let Some(agent_id) = agent_id {
        resolved.insert("agent_id".to_string(), Value::String(agent_id.to_string()));
    }
    resolved
}

/// Hand back the planned snapshot only if the live run still matches the
/// snapshot the plan was computed from.
///
/// This is deliberately not an "unchecked advance": a caller cannot commit a
/// plan after another writer changed the run, and (when the plan carries an
/// `expected_issued_step`) cannot commit a plan whose evaluated issued step is
/// no longer the live one. Persisting the returned snapshot is the caller's
/// responsibility and outside this crate.
pub fn commit_advance(
    plan: &AdvancePlan,
    live_snapshot: &RunSnapshot,
) -> Result<RunSnapshot, RuntimeError> {
    if live_snapshot != &plan.source {
        return Err(RuntimeError::StaleRunState {
            run_id: plan.source.run_id.clone(),
        });
    }
    if let Some(expected) = plan.expected_issued_step.as_deref()
        && live_snapshot.issued_step_id.as_deref() != Some(expected)
    {
        return Err(RuntimeError::StaleIssuedStep {
            run_id: plan.source.run_id.clone(),
            expected: Some(expected.to_string()),
            found: live_snapshot.issued_step_id.clone(),
        });
    }
    Ok(plan.snapshot.clone())
}
