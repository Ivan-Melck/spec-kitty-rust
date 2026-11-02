//! Pure result application and stale-state-checked advance planning.
//!
//! The library does not persist anything. [`plan_advance`] computes the next
//! snapshot and decision; [`commit_advance`] only hands back that snapshot
//! after proving the plan still applies to the live run state.

use serde::{Deserialize, Serialize};

use crate::decision::NextDecision;
use crate::error::RuntimeError;
use crate::planner::plan_next;
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
}

/// Apply a step result and plan the next decision without persisting anything.
pub fn plan_advance(
    template: &MissionTemplate,
    snapshot: &RunSnapshot,
    current_template_hash: &str,
    expected_issued_step: Option<&str>,
    result: StepResult,
) -> Result<AdvancePlan, RuntimeError> {
    let applied = apply_result(snapshot, expected_issued_step, result)?;
    let decision = plan_next(template, &applied.snapshot, current_template_hash)?;
    Ok(AdvancePlan {
        source: snapshot.clone(),
        snapshot: applied.snapshot,
        decision,
        completed_step_id: applied.completed_step_id,
    })
}

/// Hand back the planned snapshot only if the live run still matches the
/// snapshot the plan was computed from.
///
/// This is deliberately not an "unchecked advance": a caller cannot commit a
/// plan after another writer changed the run. Persisting the returned snapshot
/// is the caller's responsibility and outside this crate.
pub fn commit_advance(
    plan: &AdvancePlan,
    live_snapshot: &RunSnapshot,
) -> Result<RunSnapshot, RuntimeError> {
    if live_snapshot != &plan.source {
        return Err(RuntimeError::StaleRunState {
            run_id: plan.source.run_id.clone(),
        });
    }
    Ok(plan.snapshot.clone())
}
