//! Baseline run-state models, index references and the public read model.
//!
//! The persisted shapes are the upstream Python shapes: `state.json`,
//! `mission_template_frozen.yaml`, `run.events.jsonl` and
//! `.kittify/runtime/feature-runs.json`. No Rust-only field is persisted.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sk_runtime::{
    ActorIdentity, ActorType, DecisionRequest, MissionPolicySnapshot, NextDecision, RunSnapshot,
};

use crate::error::RunStoreError;

/// The canonical wall-clock shape upstream writes: aware UTC ISO-8601 with
/// microseconds (`datetime.now(UTC).isoformat()`).
pub(crate) fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, false)
}

/// Serialize an actor exactly as the upstream `RuntimeActorIdentity` model
/// dumps it (including the default empty `display_name` and null optionals).
pub(crate) fn actor_payload(actor: &ActorIdentity) -> Value {
    json!({
        "actor_id": actor.actor_id,
        "actor_type": actor.actor_type.to_string(),
        "display_name": actor.display_name,
        "provider": actor.provider,
        "model": actor.model,
        "tool": actor.tool,
    })
}

/// Read an actor back from a persisted/payload JSON object, when present.
pub(crate) fn parse_actor(value: &Value) -> Option<ActorIdentity> {
    let object = value.as_object()?;
    let actor_id = object.get("actor_id")?.as_str()?.to_string();
    let actor_type = match object.get("actor_type")?.as_str()? {
        "human" => ActorType::Human,
        "llm" => ActorType::Llm,
        "service" => ActorType::Service,
        _ => return None,
    };
    let string = |key: &str| object.get(key).and_then(Value::as_str).map(str::to_string);
    Some(ActorIdentity {
        actor_id,
        actor_type,
        display_name: object
            .get("display_name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        provider: string("provider"),
        model: string("model"),
        tool: string("tool"),
    })
}

/// Build the upstream `DecisionRequest` wire object persisted in
/// `pending_decisions`.
pub(crate) fn request_payload(
    decision_id: &str,
    step_id: &str,
    question: &str,
    options: &[String],
    actor: &ActorIdentity,
    requested_at: &str,
) -> Value {
    json!({
        "decision_id": decision_id,
        "step_id": step_id,
        "question": question,
        "options": options,
        "requested_by": actor_payload(actor),
        "requested_at": requested_at,
    })
}

/// The durable mission-run cursor, byte-compatible with upstream `state.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RunState {
    pub run_id: String,
    pub mission_key: String,
    pub template_path: String,
    pub template_hash: String,
    pub policy_snapshot: MissionPolicySnapshot,
    pub completed_steps: Vec<String>,
    pub issued_step_id: Option<String>,
    pub inputs: BTreeMap<String, Value>,
    pub decisions: BTreeMap<String, Value>,
    pub pending_decisions: BTreeMap<String, Value>,
    pub blocked_reason: Option<String>,
    pub mission_id: Option<String>,
    pub mission_slug: Option<String>,
}

impl RunState {
    /// Convert the persisted cursor into the runtime planner's read model.
    ///
    /// Only the cursor-relevant fields are projected; unknown persisted keys are
    /// preserved in `RunState` and never re-serialized away.
    pub fn to_snapshot(&self) -> Result<RunSnapshot, RunStoreError> {
        let mut pending_decisions = BTreeMap::new();
        for (key, value) in &self.pending_decisions {
            let request: DecisionRequest =
                serde_json::from_value(value.clone()).map_err(|error| {
                    RunStoreError::StateCorrupt {
                        path: PathBuf::from("state.json"),
                        message: format!("pending decision {key:?} is malformed: {error}"),
                    }
                })?;
            if &request.decision_id != key {
                return Err(RunStoreError::StateCorrupt {
                    path: PathBuf::from("state.json"),
                    message: format!(
                        "pending decision key {key:?} does not match decision_id {:?}",
                        request.decision_id
                    ),
                });
            }
            pending_decisions.insert(key.clone(), request);
        }
        Ok(RunSnapshot {
            run_id: self.run_id.clone(),
            mission_key: self.mission_key.clone(),
            template_path: self.template_path.clone(),
            template_hash: self.template_hash.clone(),
            policy_snapshot: self.policy_snapshot.clone(),
            completed_steps: self.completed_steps.clone(),
            issued_step_id: self.issued_step_id.clone(),
            inputs: self.inputs.clone(),
            decisions: self.decisions.clone(),
            pending_decisions,
            blocked_reason: self.blocked_reason.clone(),
            mission_id: self.mission_id.clone(),
            mission_slug: self.mission_slug.clone(),
            actor_context: BTreeMap::new(),
        })
    }

    /// Validate the invariants a durable cursor must satisfy.
    pub fn validate(&self, expected_run_id: Option<&str>) -> Result<(), RunStoreError> {
        const PATH: &str = "state.json";
        if self.run_id.trim().is_empty() {
            return Err(RunStoreError::StateCorrupt {
                path: PATH.into(),
                message: "run_id must be non-empty".to_string(),
            });
        }
        if let Some(expected) = expected_run_id
            && expected != self.run_id
        {
            return Err(RunStoreError::StateCorrupt {
                path: PATH.into(),
                message: format!(
                    "run_id {:?} does not match run directory {expected:?}",
                    self.run_id
                ),
            });
        }
        if self.mission_key.trim().is_empty() {
            return Err(RunStoreError::StateCorrupt {
                path: PATH.into(),
                message: "mission_key must be non-empty".to_string(),
            });
        }
        if self.template_hash.len() != 64
            || !self
                .template_hash
                .chars()
                .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
        {
            return Err(RunStoreError::StateCorrupt {
                path: PATH.into(),
                message: format!(
                    "template_hash {:?} is not a lowercase SHA-256 hex digest",
                    self.template_hash
                ),
            });
        }
        for (key, value) in &self.pending_decisions {
            let object = value
                .as_object()
                .ok_or_else(|| RunStoreError::StateCorrupt {
                    path: PATH.into(),
                    message: format!("pending decision {key:?} is not an object"),
                })?;
            let decision_id = object.get("decision_id").and_then(Value::as_str);
            if decision_id != Some(key.as_str()) {
                return Err(RunStoreError::StateCorrupt {
                    path: PATH.into(),
                    message: format!("pending decision key {key:?} does not match its decision_id"),
                });
            }
            for required in ["step_id", "question"] {
                if !object.get(required).is_some_and(Value::is_string) {
                    return Err(RunStoreError::StateCorrupt {
                        path: PATH.into(),
                        message: format!("pending decision {key:?} is missing {required:?}"),
                    });
                }
            }
        }
        Ok(())
    }

    /// The effective significance band recorded for an audit decision, if any.
    pub fn significance_band(&self, decision_id: &str) -> Option<String> {
        let record = self.decisions.get(&format!("significance:{decision_id}"))?;
        match record.get("effective_band")? {
            Value::String(band) => Some(band.clone()),
            Value::Object(object) => object
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_string),
            _ => None,
        }
    }

    /// The raw significance score recorded for an audit decision, if any.
    pub fn significance_score(&self, decision_id: &str) -> Option<Value> {
        self.decisions
            .get(&format!("significance:{decision_id}"))
            .cloned()
    }
}

/// A durable reference to one run. Serializable so the coordinator CLI can
/// pass it across process boundaries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRef {
    pub run_id: String,
    pub run_dir: PathBuf,
    pub mission_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mission_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mission_slug: Option<String>,
}

/// Inputs for starting (or resuming) a governed run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StartRequest {
    pub mission_slug: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mission_id: Option<String>,
    pub template_path: PathBuf,
    #[serde(default)]
    pub inputs: BTreeMap<String, Value>,
    #[serde(default)]
    pub policy_snapshot: MissionPolicySnapshot,
}

/// The result of [`crate::RunStore::start`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StartOutcome {
    pub run: RunRef,
    pub state: RunState,
    /// True when this call created the run; false when an indexed live run was reused.
    pub started: bool,
    pub resumed: bool,
    /// Number of orphan run directories removed after losing a same-mission race.
    pub orphans_removed: usize,
}

/// A pending decision, projected from the durable request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionView {
    pub decision_id: String,
    pub step_id: String,
    pub question: String,
    pub options: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_key: Option<String>,
}

/// The step currently issued to an executor, with its real prompt artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssuedStepView {
    pub step_id: String,
    pub title: String,
    pub prompt: String,
    pub prompt_file: PathBuf,
}

/// A consistency verdict computed without writing anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsistencyReport {
    pub healthy: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub differences: Vec<String>,
}

impl ConsistencyReport {
    pub(crate) fn healthy() -> Self {
        Self {
            healthy: true,
            differences: Vec::new(),
        }
    }

    pub(crate) fn unhealthy(differences: Vec<String>) -> Self {
        Self {
            healthy: false,
            differences,
        }
    }
}

/// The non-mutating read model for one run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryResult {
    pub run: RunRef,
    pub state: RunState,
    /// The planner's next decision. When a step is already issued, that issued
    /// step is reported separately in `issued_step`; the planner describes what
    /// follows it.
    pub decision: NextDecision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issued_step: Option<IssuedStepView>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending: Vec<DecisionView>,
    /// The frozen template hash recorded in the cursor.
    pub template_hash: String,
    /// The live template hash, when the recorded source still exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_template_hash: Option<String>,
    pub template_drift: bool,
    pub consistency: ConsistencyReport,
}

/// Default bounded wait for the shared cursor/index locks (upstream uses 10 s).
pub const DEFAULT_LOCK_TIMEOUT: Duration = Duration::from_secs(10);

/// Maximum size of a persisted cursor, index or frozen template read.
pub const MAX_STATE_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_INDEX_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_EVENT_LOG_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_PROMPT_BYTES: u64 = 4 * 1024 * 1024;

/// Project the pending map into public decision views.
pub(crate) fn pending_views(state: &RunState) -> Vec<DecisionView> {
    state
        .pending_decisions
        .iter()
        .map(|(key, value)| {
            let object = value.as_object();
            let string = |field: &str| {
                object
                    .and_then(|object| object.get(field))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            let options = object
                .and_then(|object| object.get("options"))
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            DecisionView {
                decision_id: string("decision_id"),
                step_id: string("step_id"),
                question: string("question"),
                options,
                input_key: key
                    .strip_prefix("input:")
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
            }
        })
        .collect()
}
