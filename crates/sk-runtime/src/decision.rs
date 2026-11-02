//! Exhaustive next-decision model.
//!
//! The four decision kinds have distinct payload types. A blocked decision
//! cannot carry a step, a step decision cannot carry a question, and a
//! decision-required payload must name exactly one requirement variant. This
//! makes impossible combinations unrepresentable in safe Rust rather than
//! merely undocumented.
//!
//! Serialization follows the upstream Python `NextDecision` flat JSON shape
//! (all fields present, irrelevant fields `null`) so a caller can compare a
//! decision with the runtime parity fixtures.

use std::collections::BTreeMap;

use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;

use crate::snapshot::MissionPolicySnapshot;

/// The closed set of decision kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionKind {
    Step,
    DecisionRequired,
    Blocked,
    Terminal,
}

impl DecisionKind {
    /// Stable wire name used in `kind`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Step => "step",
            Self::DecisionRequired => "decision_required",
            Self::Blocked => "blocked",
            Self::Terminal => "terminal",
        }
    }
}

impl std::fmt::Display for DecisionKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The context bundle attached to an executable step decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepContextBundle {
    pub run_id: String,
    pub mission_key: String,
    pub step_id: String,
    pub step_title: String,
    pub step_description: String,
    #[serde(default)]
    pub expected_output: Option<String>,
    pub policy_snapshot: MissionPolicySnapshot,
    #[serde(default)]
    pub actor_context: BTreeMap<String, Value>,
}

/// A runnable step was selected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepDecision {
    pub run_id: String,
    pub mission_key: String,
    pub step_id: String,
    pub step_title: String,
    pub prompt: String,
    pub context: StepContextBundle,
}

/// Exactly one thing is blocking progress.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionRequired {
    pub run_id: String,
    pub mission_key: String,
    #[serde(flatten)]
    pub requirement: DecisionRequirement,
}

/// The closed reasons a decision can be required.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum DecisionRequirement {
    /// A decision was already pending in the run snapshot.
    PendingDecision {
        decision_id: String,
        step_id: String,
        #[serde(default)]
        input_key: Option<String>,
        question: String,
        #[serde(default)]
        options: Vec<String>,
    },
    /// A prompt step's `requires_inputs` entry has no value.
    MissingRequiredInput {
        decision_id: String,
        step_id: String,
        input_key: String,
        question: String,
    },
    /// A blocking audit checkpoint must be answered.
    AuditCheckpoint {
        decision_id: String,
        step_id: String,
        step_title: Option<String>,
        question: String,
        options: Vec<String>,
    },
}

/// The run cannot proceed until a human intervention changes its state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockedDecision {
    pub run_id: String,
    pub mission_key: String,
    pub reason: String,
}

/// Every step is complete and nothing remains to schedule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerminalDecision {
    pub run_id: String,
    pub mission_key: String,
    pub reason: String,
}

/// The deterministic result of planning a run snapshot.
#[derive(Debug, Clone, PartialEq)]
pub enum NextDecision {
    Step(StepDecision),
    DecisionRequired(DecisionRequired),
    Blocked(BlockedDecision),
    Terminal(TerminalDecision),
}

impl NextDecision {
    /// The decision kind.
    pub fn kind(&self) -> DecisionKind {
        match self {
            Self::Step(_) => DecisionKind::Step,
            Self::DecisionRequired(_) => DecisionKind::DecisionRequired,
            Self::Blocked(_) => DecisionKind::Blocked,
            Self::Terminal(_) => DecisionKind::Terminal,
        }
    }

    /// The run this decision belongs to.
    pub fn run_id(&self) -> &str {
        match self {
            Self::Step(decision) => &decision.run_id,
            Self::DecisionRequired(decision) => &decision.run_id,
            Self::Blocked(decision) => &decision.run_id,
            Self::Terminal(decision) => &decision.run_id,
        }
    }

    /// The mission this decision belongs to.
    pub fn mission_key(&self) -> &str {
        match self {
            Self::Step(decision) => &decision.mission_key,
            Self::DecisionRequired(decision) => &decision.mission_key,
            Self::Blocked(decision) => &decision.mission_key,
            Self::Terminal(decision) => &decision.mission_key,
        }
    }

    /// The step this decision is about, when it is about one.
    pub fn step_id(&self) -> Option<&str> {
        match self {
            Self::Step(decision) => Some(&decision.step_id),
            Self::DecisionRequired(decision) => match &decision.requirement {
                DecisionRequirement::PendingDecision { step_id, .. }
                | DecisionRequirement::MissingRequiredInput { step_id, .. }
                | DecisionRequirement::AuditCheckpoint { step_id, .. } => Some(step_id),
            },
            Self::Blocked(_) | Self::Terminal(_) => None,
        }
    }

    /// The blocking reason, for blocked and terminal decisions.
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Blocked(decision) => Some(&decision.reason),
            Self::Terminal(decision) => Some(&decision.reason),
            Self::Step(_) => None,
            Self::DecisionRequired(decision) => match &decision.requirement {
                DecisionRequirement::PendingDecision { .. } => Some("pending_decision"),
                DecisionRequirement::MissingRequiredInput { .. } => Some("missing_required_input"),
                DecisionRequirement::AuditCheckpoint { .. } => None,
            },
        }
    }
}

impl<'de> Deserialize<'de> for NextDecision {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Upstream uses a null reason for audit gates. The Rust requirement
        // discriminant is internal and must not change the public wire shape.
        let mut value = Value::deserialize(deserializer)?;
        if value.get("kind").and_then(Value::as_str) == Some("decision_required")
            && value.get("reason").is_none_or(Value::is_null)
            && value
                .get("decision_id")
                .and_then(Value::as_str)
                .is_some_and(|id| id.starts_with("audit:"))
        {
            value["reason"] = Value::String("audit_checkpoint".to_owned());
        }
        #[derive(Deserialize)]
        #[serde(tag = "kind", rename_all = "snake_case")]
        enum WireDecision {
            Step(StepDecision),
            DecisionRequired(DecisionRequired),
            Blocked(BlockedDecision),
            Terminal(TerminalDecision),
        }
        let wire = serde_json::from_value(value).map_err(serde::de::Error::custom)?;
        Ok(match wire {
            WireDecision::Step(value) => Self::Step(value),
            WireDecision::DecisionRequired(value) => Self::DecisionRequired(value),
            WireDecision::Blocked(value) => Self::Blocked(value),
            WireDecision::Terminal(value) => Self::Terminal(value),
        })
    }
}

impl Serialize for NextDecision {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(12))?;
        map.serialize_entry("kind", self.kind().as_str())?;

        match self {
            Self::Step(decision) => {
                map.serialize_entry("run_id", &decision.run_id)?;
                map.serialize_entry("mission_key", &decision.mission_key)?;
                map.serialize_entry("step_id", &decision.step_id)?;
                map.serialize_entry("step_title", &decision.step_title)?;
                map.serialize_entry("prompt", &decision.prompt)?;
                map.serialize_entry("context", &decision.context)?;
                map.serialize_entry("decision_id", &Option::<String>::None)?;
                map.serialize_entry("input_key", &Option::<String>::None)?;
                map.serialize_entry("question", &Option::<String>::None)?;
                map.serialize_entry("options", &Option::<Vec<String>>::None)?;
                map.serialize_entry("reason", &Option::<String>::None)?;
            }
            Self::DecisionRequired(decision) => {
                map.serialize_entry("run_id", &decision.run_id)?;
                map.serialize_entry("mission_key", &decision.mission_key)?;
                match &decision.requirement {
                    DecisionRequirement::PendingDecision {
                        decision_id,
                        step_id,
                        input_key,
                        question,
                        options,
                    } => {
                        map.serialize_entry("step_id", step_id)?;
                        map.serialize_entry("step_title", &Option::<String>::None)?;
                        map.serialize_entry("prompt", &Option::<String>::None)?;
                        map.serialize_entry("context", &Option::<StepContextBundle>::None)?;
                        map.serialize_entry("decision_id", decision_id)?;
                        map.serialize_entry("input_key", input_key)?;
                        map.serialize_entry("question", question)?;
                        let options = (!options.is_empty()).then_some(options);
                        map.serialize_entry("options", &options)?;
                        map.serialize_entry("reason", "pending_decision")?;
                    }
                    DecisionRequirement::MissingRequiredInput {
                        decision_id,
                        step_id,
                        input_key,
                        question,
                    } => {
                        map.serialize_entry("step_id", step_id)?;
                        map.serialize_entry("step_title", &Option::<String>::None)?;
                        map.serialize_entry("prompt", &Option::<String>::None)?;
                        map.serialize_entry("context", &Option::<StepContextBundle>::None)?;
                        map.serialize_entry("decision_id", decision_id)?;
                        map.serialize_entry("input_key", input_key)?;
                        map.serialize_entry("question", question)?;
                        map.serialize_entry("options", &Option::<Vec<String>>::None)?;
                        map.serialize_entry("reason", "missing_required_input")?;
                    }
                    DecisionRequirement::AuditCheckpoint {
                        decision_id,
                        step_id,
                        step_title,
                        question,
                        options,
                    } => {
                        map.serialize_entry("step_id", step_id)?;
                        map.serialize_entry("step_title", step_title)?;
                        map.serialize_entry("prompt", &Option::<String>::None)?;
                        map.serialize_entry("context", &Option::<StepContextBundle>::None)?;
                        map.serialize_entry("decision_id", decision_id)?;
                        map.serialize_entry("input_key", &Option::<String>::None)?;
                        map.serialize_entry("question", question)?;
                        map.serialize_entry("options", options)?;
                        map.serialize_entry("reason", &Option::<String>::None)?;
                    }
                }
            }
            Self::Blocked(decision) => {
                map.serialize_entry("run_id", &decision.run_id)?;
                map.serialize_entry("mission_key", &decision.mission_key)?;
                map.serialize_entry("step_id", &Option::<String>::None)?;
                map.serialize_entry("step_title", &Option::<String>::None)?;
                map.serialize_entry("prompt", &Option::<String>::None)?;
                map.serialize_entry("context", &Option::<StepContextBundle>::None)?;
                map.serialize_entry("decision_id", &Option::<String>::None)?;
                map.serialize_entry("input_key", &Option::<String>::None)?;
                map.serialize_entry("question", &Option::<String>::None)?;
                map.serialize_entry("options", &Option::<Vec<String>>::None)?;
                map.serialize_entry("reason", &decision.reason)?;
            }
            Self::Terminal(decision) => {
                map.serialize_entry("run_id", &decision.run_id)?;
                map.serialize_entry("mission_key", &decision.mission_key)?;
                map.serialize_entry("step_id", &Option::<String>::None)?;
                map.serialize_entry("step_title", &Option::<String>::None)?;
                map.serialize_entry("prompt", &Option::<String>::None)?;
                map.serialize_entry("context", &Option::<StepContextBundle>::None)?;
                map.serialize_entry("decision_id", &Option::<String>::None)?;
                map.serialize_entry("input_key", &Option::<String>::None)?;
                map.serialize_entry("question", &Option::<String>::None)?;
                map.serialize_entry("options", &Option::<Vec<String>>::None)?;
                map.serialize_entry("reason", &decision.reason)?;
            }
        }
        map.end()
    }
}
