//! Pure projection of the retrospective lifecycle at mission terminus.
//!
//! The upstream `retrospective_terminus.py` module is mostly effects: it emits
//! retrospective events, persists a record, prompts the operator and consults
//! the completion gate. This module owns only the part that is deterministic
//! policy: given the resolved mode, whether the runtime has a facilitator and
//! (in human-in-command mode) the operator's choice, which lifecycle events
//! fire in which order, which actor emits them, and that the completion gate is
//! always consulted last.
//!
//! The caller (run-store / CLI owner) turns the plan into event appends, record
//! writes and the final gate call. Nothing here writes, prompts or reads a
//! repository. Facilitator *failures* are not modelled: the code that catches an
//! exception and emits `retrospective.failed(facilitator_error)` needs a
//! callback, so it belongs to the caller that owns the callback.

use serde::{Deserialize, Serialize};

use crate::error::RuntimeError;
use crate::snapshot::{ActorIdentity, ActorType};

/// The retrospective lifecycle step id used in the `requested` payload.
pub const TERMINUS_STEP_ID: &str = "terminus";

/// The deferred facilitator identity upstream records until real wiring exists.
pub const DEFERRED_FACILITATOR_ID: &str = "deferred";

/// The deferred facilitator action id upstream records.
pub const DEFERRED_ACTION_ID: &str = "action:retrospect";

/// The fallback skip reason upstream substitutes for a blank operator reason.
pub const DEFAULT_SKIP_REASON: &str = "(no reason provided)";

/// How the retrospective is being driven.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetrospectiveMode {
    Autonomous,
    HumanInCommand,
}

/// The operator's answer in human-in-command mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "choice", rename_all = "snake_case")]
pub enum HicChoice {
    /// Run the retrospective now.
    Run,
    /// Skip it, optionally giving a reason.
    Skip {
        #[serde(default)]
        reason: Option<String>,
    },
}

/// Who emits the `retrospective.requested` event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequesterActor {
    Runtime,
    Operator,
}

impl RequesterActor {
    /// The stable actor id for the requester.
    pub fn actor_id(self) -> &'static str {
        match self {
            Self::Runtime => "next",
            Self::Operator => "operator",
        }
    }

    /// The actor identity wire record for the requester.
    ///
    /// In human-in-command mode the caller supplies the real operator actor; the
    /// plan carries only the role so it stays deterministic.
    pub fn identity(self) -> ActorIdentity {
        match self {
            Self::Runtime => ActorIdentity::new("next", ActorType::Service),
            Self::Operator => ActorIdentity::new("operator", ActorType::Human),
        }
    }
}

/// One lifecycle event in the terminus plan, in firing order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum TerminusStep {
    /// `retrospective.requested`
    Requested { mode: RetrospectiveMode },
    /// `retrospective.started`
    Started {
        facilitator_profile_id: String,
        action_id: String,
    },
    /// `retrospective.failed` (a deterministic wiring gap, not a callback error)
    Failed { failure_code: String },
    /// `retrospective.completed`
    Completed,
    /// `retrospective.skipped`
    Skipped { skip_reason: String },
    /// The completion gate (`before_mark_done`), always last.
    Gate,
}

impl TerminusStep {
    /// The upstream event name (`retrospective.*`), or `gate`.
    pub fn event_name(&self) -> &'static str {
        match self {
            Self::Requested { .. } => "retrospective.requested",
            Self::Started { .. } => "retrospective.started",
            Self::Failed { .. } => "retrospective.failed",
            Self::Completed => "retrospective.completed",
            Self::Skipped { .. } => "retrospective.skipped",
            Self::Gate => "gate",
        }
    }
}

/// A deterministic terminus plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminusPlan {
    pub mode: RetrospectiveMode,
    /// Who emits `retrospective.requested` (runtime in autonomous mode, the
    /// operator in human-in-command mode).
    pub requester: RequesterActor,
    /// Lifecycle events in firing order, ending with the gate.
    pub steps: Vec<TerminusStep>,
}

impl TerminusPlan {
    /// The event names in firing order, for a compact assertion surface.
    pub fn event_names(&self) -> Vec<&'static str> {
        self.steps.iter().map(TerminusStep::event_name).collect()
    }

    /// The gate is always the final step.
    pub fn ends_with_gate(&self) -> bool {
        matches!(self.steps.last(), Some(TerminusStep::Gate))
    }
}

fn started_step() -> TerminusStep {
    TerminusStep::Started {
        facilitator_profile_id: DEFERRED_FACILITATOR_ID.to_string(),
        action_id: DEFERRED_ACTION_ID.to_string(),
    }
}

/// Plan the retrospective lifecycle at mission terminus.
///
/// * Autonomous mode always emits `started`; it then fails with
///   `facilitator_not_configured` when no facilitator is wired, or completes.
/// * Human-in-command mode with no operator choice fails with
///   `prompt_not_configured`;
/// * Human-in-command `Run` behaves like autonomous after `requested`;
/// * Human-in-command `Skip` records the skip (using
///   [`DEFAULT_SKIP_REASON`] for a blank reason) and never starts.
///
/// The completion gate is always appended last, so a caller can never treat an
/// earlier step as permission to finish the mission.
pub fn plan_terminus(
    mode: RetrospectiveMode,
    has_facilitator: bool,
    hic_choice: Option<HicChoice>,
) -> Result<TerminusPlan, RuntimeError> {
    let requester = match mode {
        RetrospectiveMode::Autonomous => RequesterActor::Runtime,
        RetrospectiveMode::HumanInCommand => RequesterActor::Operator,
    };
    let mut steps = vec![TerminusStep::Requested { mode }];

    match mode {
        RetrospectiveMode::Autonomous => {
            steps.push(started_step());
            if has_facilitator {
                steps.push(TerminusStep::Completed);
            } else {
                steps.push(TerminusStep::Failed {
                    failure_code: "facilitator_not_configured".to_string(),
                });
            }
        }
        RetrospectiveMode::HumanInCommand => {
            let Some(choice) = hic_choice else {
                steps.push(TerminusStep::Failed {
                    failure_code: "prompt_not_configured".to_string(),
                });
                steps.push(TerminusStep::Gate);
                return Ok(TerminusPlan {
                    mode,
                    requester,
                    steps,
                });
            };
            match choice {
                HicChoice::Run => {
                    steps.push(started_step());
                    if has_facilitator {
                        steps.push(TerminusStep::Completed);
                    } else {
                        steps.push(TerminusStep::Failed {
                            failure_code: "facilitator_not_configured".to_string(),
                        });
                    }
                }
                HicChoice::Skip { reason } => {
                    let skip_reason = reason
                        .filter(|reason| !reason.is_empty())
                        .unwrap_or_else(|| DEFAULT_SKIP_REASON.to_string());
                    steps.push(TerminusStep::Skipped { skip_reason });
                }
            }
        }
    }

    steps.push(TerminusStep::Gate);
    Ok(TerminusPlan {
        mode,
        requester,
        steps,
    })
}
