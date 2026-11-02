//! The deterministic planner query.
//!
//! Priority is fixed and intentionally simple: blocked state, template drift,
//! the lexicographically first pending decision, the first eligible DAG
//! step/audit in template order, required inputs, then step or terminal. An
//! unschedulable remainder is blocked, never terminal.

use std::collections::BTreeSet;

use crate::decision::{
    BlockedDecision, DecisionRequired, DecisionRequirement, NextDecision, StepContextBundle,
    StepDecision, TerminalDecision,
};
use crate::error::RuntimeError;
use crate::snapshot::RunSnapshot;
use crate::template::{AuditEnforcement, AuditStep, MissionTemplate, PromptStep};

/// Reason text used when the live template hash differs from the frozen hash.
pub const TEMPLATE_DRIFT_REASON: &str = "Template changed during active run. Migration required.";

/// Reason text used when remaining steps cannot be scheduled.
pub const UNSCHEDULABLE_REASON: &str =
    "No eligible steps: remaining steps have unmet dependencies.";

/// Reason text used when every step is complete.
pub const TERMINAL_REASON: &str = "All mission steps completed";

/// Compute the next deterministic decision for a run.
///
/// `current_template_hash` is the caller's hash of the live template bytes.
/// An empty string means "no live template is available"; in that case drift
/// is not evaluated, matching the upstream rule that a missing live file is
/// not drift. Pass the snapshot's frozen hash to explicitly assert no drift.
pub fn plan_next(
    template: &MissionTemplate,
    snapshot: &RunSnapshot,
    current_template_hash: &str,
) -> Result<NextDecision, RuntimeError> {
    template.validate()?;
    validate_snapshot(snapshot)?;

    if let Some(reason) = snapshot
        .blocked_reason
        .as_deref()
        .filter(|reason| !reason.is_empty())
    {
        return Ok(NextDecision::Blocked(BlockedDecision {
            run_id: snapshot.run_id.clone(),
            mission_key: snapshot.mission_key.clone(),
            reason: reason.to_string(),
        }));
    }

    if !current_template_hash.is_empty() && snapshot.template_hash != current_template_hash {
        return Ok(NextDecision::Blocked(BlockedDecision {
            run_id: snapshot.run_id.clone(),
            mission_key: snapshot.mission_key.clone(),
            reason: TEMPLATE_DRIFT_REASON.to_string(),
        }));
    }

    if let Some((key, request)) = snapshot.pending_decisions.iter().next() {
        if key != &request.decision_id {
            return Err(RuntimeError::PendingDecisionMismatch {
                key: key.clone(),
                decision_id: request.decision_id.clone(),
            });
        }
        let input_key = request
            .decision_id
            .strip_prefix("input:")
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        return Ok(NextDecision::DecisionRequired(DecisionRequired {
            run_id: snapshot.run_id.clone(),
            mission_key: snapshot.mission_key.clone(),
            requirement: DecisionRequirement::PendingDecision {
                decision_id: request.decision_id.clone(),
                step_id: request.step_id.clone(),
                input_key,
                question: request.question.clone(),
                options: request.options.clone(),
            },
        }));
    }

    let Some(step) = resolve_next_eligible_step(template, snapshot) else {
        if has_remaining_steps(template, snapshot) {
            return Ok(NextDecision::Blocked(BlockedDecision {
                run_id: snapshot.run_id.clone(),
                mission_key: snapshot.mission_key.clone(),
                reason: UNSCHEDULABLE_REASON.to_string(),
            }));
        }
        return Ok(NextDecision::Terminal(TerminalDecision {
            run_id: snapshot.run_id.clone(),
            mission_key: snapshot.mission_key.clone(),
            reason: TERMINAL_REASON.to_string(),
        }));
    };

    match step {
        EligibleStep::Prompt(step) => prompt_step_decision(snapshot, step),
        EligibleStep::Audit(step) => audit_step_decision(snapshot, step),
    }
}

enum EligibleStep<'a> {
    Prompt(&'a PromptStep),
    Audit(&'a AuditStep),
}

fn resolve_next_eligible_step<'a>(
    template: &'a MissionTemplate,
    snapshot: &RunSnapshot,
) -> Option<EligibleStep<'a>> {
    let completed: BTreeSet<&str> = snapshot
        .completed_steps
        .iter()
        .map(String::as_str)
        .collect();

    for step in &template.steps {
        if step.id == snapshot.issued_step_id.as_deref().unwrap_or_default() {
            continue;
        }
        if completed.contains(step.id.as_str()) {
            continue;
        }
        if step
            .depends_on
            .iter()
            .all(|dependency| completed.contains(dependency.as_str()))
        {
            return Some(EligibleStep::Prompt(step));
        }
    }

    for step in &template.audit_steps {
        if step.id == snapshot.issued_step_id.as_deref().unwrap_or_default() {
            continue;
        }
        if completed.contains(step.id.as_str()) {
            continue;
        }
        if step
            .depends_on
            .iter()
            .all(|dependency| completed.contains(dependency.as_str()))
        {
            return Some(EligibleStep::Audit(step));
        }
    }

    None
}

fn has_remaining_steps(template: &MissionTemplate, snapshot: &RunSnapshot) -> bool {
    let completed: BTreeSet<&str> = snapshot
        .completed_steps
        .iter()
        .map(String::as_str)
        .collect();
    let issued = snapshot.issued_step_id.as_deref();

    template
        .steps
        .iter()
        .map(|step| step.id.as_str())
        .chain(template.audit_steps.iter().map(|step| step.id.as_str()))
        .any(|step_id| !completed.contains(step_id) && Some(step_id) != issued)
}

fn prompt_step_decision(
    snapshot: &RunSnapshot,
    step: &PromptStep,
) -> Result<NextDecision, RuntimeError> {
    if let Some(missing) = step.requires_inputs.iter().find(|required| {
        !snapshot.inputs.contains_key(required.as_str())
            && !snapshot.decisions.contains_key(required.as_str())
    }) {
        let decision_id = format!("input:{missing}");
        return Ok(NextDecision::DecisionRequired(DecisionRequired {
            run_id: snapshot.run_id.clone(),
            mission_key: snapshot.mission_key.clone(),
            requirement: DecisionRequirement::MissingRequiredInput {
                decision_id,
                step_id: step.id.clone(),
                input_key: missing.clone(),
                question: format!(
                    "Input required before step '{}': provide value for '{}'.",
                    step.id, missing
                ),
            },
        }));
    }

    let prompt = step
        .prompt
        .clone()
        .unwrap_or_else(|| format!("Execute step '{}': {}", step.id, step.title));

    Ok(NextDecision::Step(StepDecision {
        run_id: snapshot.run_id.clone(),
        mission_key: snapshot.mission_key.clone(),
        step_id: step.id.clone(),
        step_title: step.title.clone(),
        prompt,
        context: context_for(snapshot, step, step.expected_output.clone()),
    }))
}

fn audit_step_decision(
    snapshot: &RunSnapshot,
    step: &AuditStep,
) -> Result<NextDecision, RuntimeError> {
    if step.significance.is_some() {
        return Err(RuntimeError::UnsupportedPolicy {
            step_id: step.id.clone(),
            policy: "audit significance evaluation",
        });
    }
    match step.audit.enforcement {
        AuditEnforcement::Blocking => Ok(NextDecision::DecisionRequired(DecisionRequired {
            run_id: snapshot.run_id.clone(),
            mission_key: snapshot.mission_key.clone(),
            requirement: DecisionRequirement::AuditCheckpoint {
                decision_id: format!("audit:{}", step.id),
                step_id: step.id.clone(),
                step_title: step.title.clone(),
                question: format!("Audit checkpoint: {}. Approve to continue?", step.title),
                options: vec!["approve".to_string(), "reject".to_string()],
            },
        })),
        AuditEnforcement::Advisory => Ok(NextDecision::Step(StepDecision {
            run_id: snapshot.run_id.clone(),
            mission_key: snapshot.mission_key.clone(),
            step_id: step.id.clone(),
            step_title: step.title.clone(),
            prompt: format!("Execute audit step '{}': {}", step.id, step.title),
            context: context_for(snapshot, step, None),
        })),
    }
}

fn context_for(
    snapshot: &RunSnapshot,
    step: impl StepContextFields,
    expected_output: Option<String>,
) -> StepContextBundle {
    StepContextBundle {
        run_id: snapshot.run_id.clone(),
        mission_key: snapshot.mission_key.clone(),
        step_id: step.id().to_string(),
        step_title: step.title().to_string(),
        step_description: step.description().to_string(),
        expected_output,
        policy_snapshot: snapshot.policy_snapshot.clone(),
        actor_context: snapshot.actor_context.clone(),
    }
}

trait StepContextFields {
    fn id(&self) -> &str;
    fn title(&self) -> &str;
    fn description(&self) -> &str;
}

impl StepContextFields for &PromptStep {
    fn id(&self) -> &str {
        &self.id
    }
    fn title(&self) -> &str {
        &self.title
    }
    fn description(&self) -> &str {
        &self.description
    }
}

impl StepContextFields for &AuditStep {
    fn id(&self) -> &str {
        &self.id
    }
    fn title(&self) -> &str {
        &self.title
    }
    fn description(&self) -> &str {
        &self.description
    }
}

fn validate_snapshot(snapshot: &RunSnapshot) -> Result<(), RuntimeError> {
    if snapshot.run_id.trim().is_empty() {
        return Err(RuntimeError::InvalidRunSnapshot {
            message: "run_id must be non-empty".to_string(),
        });
    }
    if snapshot.mission_key.trim().is_empty() {
        return Err(RuntimeError::InvalidRunSnapshot {
            message: "mission_key must be non-empty".to_string(),
        });
    }
    Ok(())
}
