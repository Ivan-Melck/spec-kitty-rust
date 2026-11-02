//! Read-model types describing a mission run and its policy snapshot.
//!
//! These are plain data. The runtime library never reads or writes a run
//! store; callers supply a snapshot and act on the returned decision.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Actor kinds accepted by the runtime schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorType {
    Human,
    Llm,
    Service,
}

impl fmt::Display for ActorType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Human => "human",
            Self::Llm => "llm",
            Self::Service => "service",
        };
        formatter.write_str(value)
    }
}

/// Identity of an actor that requested or answered a decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActorIdentity {
    pub actor_id: String,
    pub actor_type: ActorType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
}

/// Policy strictness, matching the upstream schema's closed set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Strictness {
    Off,
    #[default]
    Medium,
    Max,
}

fn default_route() -> String {
    "same_llm_context".to_string()
}

/// The policy snapshot frozen into a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MissionPolicySnapshot {
    #[serde(default)]
    pub strictness: Strictness,
    #[serde(default = "default_route")]
    pub default_route: String,
    #[serde(default)]
    pub extras: BTreeMap<String, Value>,
}

impl Default for MissionPolicySnapshot {
    fn default() -> Self {
        Self {
            strictness: Strictness::Medium,
            default_route: default_route(),
            extras: BTreeMap::new(),
        }
    }
}

/// A decision request stored in a run's `pending_decisions` map.
///
/// The upstream schema requires `requested_by` and `requested_at`. They are
/// optional here so the planner can read the decision-relevant fields from
/// older or partially projected snapshots without inventing metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionRequest {
    pub decision_id: String,
    pub step_id: String,
    pub question: String,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_by: Option<ActorIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_at: Option<String>,
}

impl DecisionRequest {
    /// Build the minimal request the planner needs.
    pub fn new(
        decision_id: impl Into<String>,
        step_id: impl Into<String>,
        question: impl Into<String>,
        options: Vec<String>,
    ) -> Self {
        Self {
            decision_id: decision_id.into(),
            step_id: step_id.into(),
            question: question.into(),
            options,
            requested_by: None,
            requested_at: None,
        }
    }
}

/// A frozen read-model of a mission run.
///
/// `actor_context` is a Rust-library extension: the upstream Python planner
/// accepts actor context as a separate argument, while this crate's fixed
/// public `plan_next` signature does not. Keeping it on the snapshot lets a
/// caller reproduce the emitted step context without adding hidden state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunSnapshot {
    pub run_id: String,
    pub mission_key: String,
    #[serde(default)]
    pub template_path: String,
    pub template_hash: String,
    #[serde(default)]
    pub policy_snapshot: MissionPolicySnapshot,
    #[serde(default)]
    pub completed_steps: Vec<String>,
    #[serde(default)]
    pub issued_step_id: Option<String>,
    #[serde(default)]
    pub inputs: BTreeMap<String, Value>,
    #[serde(default)]
    pub decisions: BTreeMap<String, Value>,
    #[serde(default)]
    pub pending_decisions: BTreeMap<String, DecisionRequest>,
    #[serde(default)]
    pub blocked_reason: Option<String>,
    #[serde(default)]
    pub mission_id: Option<String>,
    #[serde(default)]
    pub mission_slug: Option<String>,
    #[serde(default)]
    pub actor_context: BTreeMap<String, Value>,
}

impl RunSnapshot {
    /// Construct a minimal snapshot with default policy and empty progress.
    pub fn new(
        run_id: impl Into<String>,
        mission_key: impl Into<String>,
        template_hash: impl Into<String>,
    ) -> Self {
        Self {
            run_id: run_id.into(),
            mission_key: mission_key.into(),
            template_path: String::new(),
            template_hash: template_hash.into(),
            policy_snapshot: MissionPolicySnapshot::default(),
            completed_steps: Vec::new(),
            issued_step_id: None,
            inputs: BTreeMap::new(),
            decisions: BTreeMap::new(),
            pending_decisions: BTreeMap::new(),
            blocked_reason: None,
            mission_id: None,
            mission_slug: None,
            actor_context: BTreeMap::new(),
        }
    }
}
