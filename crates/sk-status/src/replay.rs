//! The canonical `status.events.jsonl` diary parser and deterministic reducer.
//!
//! This is a typed, consumer-side replay of the pinned external
//! `spec_kitty_events.diary` contract (11.3.0). It deliberately reproduces the
//! external reducer's fold semantics rather than inventing a "latest event
//! wins" shortcut:
//!
//! - transitions deduplicate by `event_id` (first real occurrence, or the
//!   canonical-minimum payload when every row sharing an id is a
//!   `migration:` reconstruction seed);
//! - transitions sort by `(at, event_id)`, with a real `approved -> X`
//!   departure lifted after its approval and the resulting floor carried to
//!   later same-WP rows;
//! - `from_lane` continuity and same-`at` rollback arbitration decide whether
//!   an event applies;
//! - a losing reconstruction seed may still fold claim/verdict payload no real
//!   event wrote;
//! - annotations fold in a separate post-transition pass and never touch
//!   `force_count`.
//!
//! This module parses and reduces; it never writes.
//!
//! It also owns the **Source event-store admission** seam
//! ([`read_source_event_stream_from_text`]): the reference's
//! `specify_cli.status.store.read_events_from_text` runs a per-row model check
//! before any row reaches the fold, and committed-log consumers select it. JSON
//! syntactic success is not model admission (CF49).
//!
//! Admission and the diary parse are deliberately distinct capabilities: the
//! parser mirrors the pinned *external* `spec_kitty_events.diary` contract, while
//! admission mirrors the reference CLI's own event store, which resolves a legacy
//! `mission_id` and refuses a row whose identity the reference's fold cannot key
//! on. A consumer that answers "healthy / no strand" must use admission.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use crate::lane::Lane;

/// A typed diary parse or decode refusal.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DiaryError {
    #[error("Invalid diary row: expected JSON object, got {kind}")]
    NotObject { kind: &'static str },

    #[error("Unknown event kind {kind:?}")]
    UnknownKind { kind: String },

    #[error("Invalid annotation row: {detail}")]
    InvalidAnnotation { detail: String },

    #[error("Invalid lane-transition row: {detail}")]
    InvalidTransition { detail: String },
}

/// A decoded `actor` value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actor {
    /// A plain scalar identity, coerced exactly as the baseline `str(...)`.
    Text(String),
    /// The `{role, profile, tool, model}` resolved binding.
    Structured(StructuredActor),
}

impl Actor {
    /// The display identity used by snapshot/guard consumers.
    ///
    /// A structured actor projects to `tool`, then `role`, then `""`.
    pub fn identity(&self) -> String {
        match self {
            Actor::Text(text) => text.clone(),
            Actor::Structured(actor) => actor
                .tool
                .clone()
                .or_else(|| actor.role.clone())
                .unwrap_or_default(),
        }
    }

    /// True when this actor is a `migration:` reconstruction seed.
    pub fn is_reconstruction_seed(&self) -> bool {
        self.identity().starts_with("migration:")
    }

    /// The wire JSON value (`str` or the four-key object).
    pub fn to_json_value(&self) -> Value {
        match self {
            Actor::Text(text) => Value::String(text.clone()),
            Actor::Structured(actor) => json!({
                "role": actor.role,
                "profile": actor.profile,
                "tool": actor.tool,
                "model": actor.model,
            }),
        }
    }
}

/// The four-key structured actor accepted by the external diary contract.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StructuredActor {
    pub role: Option<String>,
    pub profile: Option<String>,
    pub tool: Option<String>,
    pub model: Option<String>,
}

/// Reviewer approval carried by done evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewApproval {
    pub reviewer: String,
    pub verdict: String,
    pub reference: String,
}

impl ReviewApproval {
    pub(crate) fn to_json_value(&self) -> Value {
        json!({
            "reviewer": self.reviewer,
            "verdict": self.verdict,
            "reference": self.reference,
        })
    }
}

/// Repository evidence carried by done evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoEvidence {
    pub repo: String,
    pub branch: String,
    pub commit: String,
    pub files_touched: Vec<String>,
}

impl RepoEvidence {
    pub(crate) fn to_json_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("repo".to_owned(), Value::String(self.repo.clone()));
        map.insert("branch".to_owned(), Value::String(self.branch.clone()));
        map.insert("commit".to_owned(), Value::String(self.commit.clone()));
        if !self.files_touched.is_empty() {
            map.insert(
                "files_touched".to_owned(),
                Value::Array(
                    self.files_touched
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                ),
            );
        }
        Value::Object(map)
    }
}

/// Verification evidence carried by done evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationResult {
    pub command: String,
    pub result: String,
    pub summary: String,
}

impl VerificationResult {
    pub(crate) fn to_json_value(&self) -> Value {
        json!({
            "command": self.command,
            "result": self.result,
            "summary": self.summary,
        })
    }
}

/// The canonical `DoneEvidence` payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoneEvidence {
    pub review: ReviewApproval,
    pub repos: Vec<RepoEvidence>,
    pub verification: Vec<VerificationResult>,
}

impl DoneEvidence {
    pub(crate) fn to_json_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("review".to_owned(), self.review.to_json_value());
        if !self.repos.is_empty() {
            map.insert(
                "repos".to_owned(),
                Value::Array(self.repos.iter().map(RepoEvidence::to_json_value).collect()),
            );
        }
        if !self.verification.is_empty() {
            map.insert(
                "verification".to_owned(),
                Value::Array(
                    self.verification
                        .iter()
                        .map(VerificationResult::to_json_value)
                        .collect(),
                ),
            );
        }
        Value::Object(map)
    }
}

/// The structured review outcome carried by outbound `in_review` edges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewOutcome {
    pub reviewer: String,
    pub verdict: String,
    pub reference: String,
    pub feedback_path: Option<String>,
}

impl ReviewOutcome {
    pub(crate) fn to_json_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("reviewer".to_owned(), Value::String(self.reviewer.clone()));
        map.insert("verdict".to_owned(), Value::String(self.verdict.clone()));
        map.insert(
            "reference".to_owned(),
            Value::String(self.reference.clone()),
        );
        if let Some(path) = &self.feedback_path {
            map.insert("feedback_path".to_owned(), Value::String(path.clone()));
        }
        Value::Object(map)
    }
}

/// One decoded lane transition.
#[derive(Debug, Clone, PartialEq)]
pub struct StatusEvent {
    pub event_id: String,
    pub mission_slug: String,
    pub mission_id: Option<String>,
    pub wp_id: String,
    pub from_lane: Lane,
    pub to_lane: Lane,
    pub at: String,
    pub actor: Actor,
    pub force: bool,
    pub execution_mode: String,
    pub reason: Option<String>,
    /// CLI provenance discriminator. The external diary ignores this field;
    /// the baseline CLI cancellation projection reads it.
    pub reason_source: Option<String>,
    pub review_ref: Option<String>,
    pub evidence: Option<DoneEvidence>,
    pub review_result: Option<ReviewOutcome>,
    pub policy_metadata: Option<Value>,
}

impl StatusEvent {
    /// The external diary's canonical `to_dict` shape.
    ///
    /// `reason_source` is intentionally absent: `spec_kitty_events==11.3.0`
    /// does not carry it, and its inclusion would change seed tie-breaking.
    pub fn to_json_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("event_id".to_owned(), Value::String(self.event_id.clone()));
        map.insert(
            "mission_slug".to_owned(),
            Value::String(self.mission_slug.clone()),
        );
        map.insert("wp_id".to_owned(), Value::String(self.wp_id.clone()));
        map.insert(
            "from_lane".to_owned(),
            Value::String(self.from_lane.as_str().to_owned()),
        );
        map.insert(
            "to_lane".to_owned(),
            Value::String(self.to_lane.as_str().to_owned()),
        );
        map.insert("at".to_owned(), Value::String(self.at.clone()));
        map.insert("actor".to_owned(), self.actor.to_json_value());
        map.insert("force".to_owned(), Value::Bool(self.force));
        map.insert(
            "execution_mode".to_owned(),
            Value::String(self.execution_mode.clone()),
        );
        map.insert(
            "reason".to_owned(),
            self.reason
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        map.insert(
            "review_ref".to_owned(),
            self.review_ref
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        map.insert(
            "evidence".to_owned(),
            self.evidence
                .as_ref()
                .map(DoneEvidence::to_json_value)
                .unwrap_or(Value::Null),
        );
        map.insert(
            "policy_metadata".to_owned(),
            self.policy_metadata.clone().unwrap_or(Value::Null),
        );
        if let Some(result) = &self.review_result {
            map.insert("review_result".to_owned(), result.to_json_value());
        }
        if let Some(mission_id) = &self.mission_id {
            map.insert("mission_id".to_owned(), Value::String(mission_id.clone()));
        }
        Value::Object(map)
    }

    /// True when this event is a `migration:` reconstruction seed.
    pub fn is_reconstruction_seed(&self) -> bool {
        self.actor.is_reconstruction_seed()
    }
}

/// A review-artifact override carried by an annotation delta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewOverride {
    pub at: String,
    pub actor: String,
    pub wp_id: String,
    pub reason: String,
}

impl ReviewOverride {
    pub(crate) fn is_release_sentinel(&self) -> bool {
        self.at.is_empty()
            && self.actor.is_empty()
            && self.wp_id.is_empty()
            && self.reason.is_empty()
    }

    pub(crate) fn to_json_value(&self) -> Value {
        json!({
            "at": self.at,
            "actor": self.actor,
            "wp_id": self.wp_id,
            "reason": self.reason,
        })
    }
}

/// The typed `WPInnerStateDelta` payload.
///
/// Scalar slots stay `Value` so an unknown/non-string wire value round-trips
/// exactly; the reducer's own empty-string no-op guard is applied at fold time.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct InnerStateDelta {
    pub shell_pid: Option<Value>,
    pub shell_pid_created_at: Option<Value>,
    pub subtasks: Option<BTreeMap<String, String>>,
    pub note: Option<Value>,
    pub tracker_refs: Option<Vec<Value>>,
    pub tracker_refs_replace: Option<Vec<Value>>,
    pub agent: Option<Value>,
    pub assignee: Option<Value>,
    pub review: Option<ReviewOverride>,
    pub role: Option<Value>,
    pub agent_profile: Option<Value>,
    pub agent_profile_version: Option<Value>,
    pub model: Option<Value>,
    pub provider: Option<Value>,
    pub release_runtime_claim: bool,
}

const SCALAR_SLOTS: [&str; 8] = [
    "shell_pid_created_at",
    "agent",
    "assignee",
    "role",
    "agent_profile",
    "agent_profile_version",
    "model",
    "provider",
];

impl InnerStateDelta {
    pub(crate) fn scalar(&self, name: &str) -> Option<&Value> {
        match name {
            "shell_pid_created_at" => self.shell_pid_created_at.as_ref(),
            "agent" => self.agent.as_ref(),
            "assignee" => self.assignee.as_ref(),
            "role" => self.role.as_ref(),
            "agent_profile" => self.agent_profile.as_ref(),
            "agent_profile_version" => self.agent_profile_version.as_ref(),
            "model" => self.model.as_ref(),
            "provider" => self.provider.as_ref(),
            _ => None,
        }
    }
}

/// One decoded off-axis runtime-state annotation.
#[derive(Debug, Clone, PartialEq)]
pub struct InnerStateChanged {
    pub event_id: String,
    pub wp_id: String,
    pub at: String,
    pub actor: Actor,
    pub delta: InnerStateDelta,
}

/// The read-shape partition of a diary row stream.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DiaryStream {
    pub transitions: Vec<StatusEvent>,
    pub annotations: Vec<InnerStateChanged>,
}

/// Partition raw rows exactly as `spec_kitty_events.diary.parse_diary`.
pub fn parse_diary(rows: &[Value]) -> Result<DiaryStream, DiaryError> {
    let mut transitions = Vec::new();
    let mut annotations = Vec::new();
    for row in rows {
        let Some(object) = row.as_object() else {
            return Err(DiaryError::NotObject {
                kind: json_kind(row),
            });
        };
        if let Some(kind) = object.get("kind").filter(|value| !value.is_null()) {
            if kind.as_str() == Some("annotation") {
                annotations.push(parse_annotation(object)?);
                continue;
            }
            return Err(DiaryError::UnknownKind {
                kind: python_str(kind),
            });
        }
        if is_non_lane_row(object) {
            continue;
        }
        transitions.push(parse_status_event(object)?);
    }
    Ok(DiaryStream {
        transitions,
        annotations,
    })
}

/// True for a diary row that is ignored in place (never fatal, never a lane).
pub fn is_non_lane_row(object: &Map<String, Value>) -> bool {
    if let Some(name) = object.get("event_name").and_then(Value::as_str)
        && name.starts_with("retrospective.")
    {
        return true;
    }
    if let Some(event_type) = object.get("type").and_then(Value::as_str)
        && matches!(
            event_type,
            "RetrospectiveCaptured" | "RetrospectiveCaptureFailed" | "RetrospectiveSkipped"
        )
    {
        return true;
    }
    object.contains_key("event_type")
}

/// The canonical display summary lanes (nine lanes; non-display lanes excluded).
pub const DISPLAY_LANES: [Lane; 9] = Lane::CANONICAL;

// ---------------------------------------------------------------------------
// Row decoders
// ---------------------------------------------------------------------------

fn parse_status_event(object: &Map<String, Value>) -> Result<StatusEvent, DiaryError> {
    let event_id = required_string(object, "event_id")?;
    let mission_slug = mission_slug(object);
    let wp_id = required_value_string(object, "wp_id")?;
    let from_lane = parse_lane(object, "from_lane")?;
    let to_lane = parse_lane(object, "to_lane")?;
    let at = required_value_string(object, "at")?;
    let actor = decode_actor(
        object
            .get("actor")
            .ok_or_else(|| transition("missing required field \"actor\""))?,
    )?;
    let force = object
        .get("force")
        .and_then(Value::as_bool)
        .ok_or_else(|| transition("field \"force\" must be a boolean"))?;
    let execution_mode = required_value_string(object, "execution_mode")?;
    let reason = optional_value_string(object, "reason");
    let reason_source = optional_value_string(object, "reason_source");
    let review_ref = optional_value_string(object, "review_ref");
    let evidence = match object.get("evidence") {
        Some(value) if is_truthy(value) => Some(parse_done_evidence(value)?),
        _ => None,
    };
    let review_result = match object.get("review_result") {
        Some(value) if is_truthy(value) => Some(parse_review_outcome(value)?),
        _ => None,
    };
    let policy_metadata = object
        .get("policy_metadata")
        .filter(|value| !value.is_null())
        .cloned();
    let mission_id = optional_string(object, "mission_id");

    Ok(StatusEvent {
        event_id,
        mission_slug,
        mission_id,
        wp_id,
        from_lane,
        to_lane,
        at,
        actor,
        force,
        execution_mode,
        reason,
        reason_source,
        review_ref,
        evidence,
        review_result,
        policy_metadata,
    })
}

pub(crate) fn parse_annotation(
    object: &Map<String, Value>,
) -> Result<InnerStateChanged, DiaryError> {
    let event_id = required_string(object, "event_id")?;
    if !is_ulid(&event_id) {
        return Err(annotation(format!(
            "event_id is not a valid ULID: {event_id:?}"
        )));
    }
    let wp_id = required_value_string(object, "wp_id")?;
    let at = required_value_string(object, "at")?;
    let actor = decode_actor(
        object
            .get("actor")
            .ok_or_else(|| annotation("missing required field \"actor\""))?,
    )?;
    let delta = object
        .get("delta")
        .ok_or_else(|| annotation("missing required field \"delta\""))?
        .as_object()
        .ok_or_else(|| annotation("delta must be a JSON object"))?;
    Ok(InnerStateChanged {
        event_id,
        wp_id,
        at,
        actor,
        delta: parse_inner_state_delta(delta)?,
    })
}

fn parse_inner_state_delta(delta: &Map<String, Value>) -> Result<InnerStateDelta, DiaryError> {
    let mut parsed = InnerStateDelta {
        release_runtime_claim: delta.get("release_runtime_claim").is_some_and(is_truthy),
        ..InnerStateDelta::default()
    };

    parsed.shell_pid = match delta.get("shell_pid").filter(|value| !value.is_null()) {
        Some(value) => Some(Value::Number(serde_json::Number::from(parse_int(value)?))),
        None => None,
    };

    parsed.subtasks = match delta.get("subtasks").filter(|value| !value.is_null()) {
        Some(value) => {
            let object = value
                .as_object()
                .ok_or_else(|| annotation("subtasks must be a JSON object"))?;
            let mut subtasks = BTreeMap::new();
            for (id, status) in object {
                let text = status
                    .as_str()
                    .ok_or_else(|| annotation("subtask status must be a string"))?;
                let normalized = if text == "not_applicable" {
                    "skipped"
                } else {
                    text
                };
                if !is_subtask_status(normalized) {
                    return Err(annotation(format!("unknown subtask status: {text:?}")));
                }
                subtasks.insert(id.clone(), normalized.to_owned());
            }
            Some(subtasks)
        }
        None => None,
    };

    parsed.note = delta.get("note").filter(|value| !value.is_null()).cloned();

    parsed.tracker_refs = match delta.get("tracker_refs").filter(|value| !value.is_null()) {
        Some(value) => Some(parse_ref_list(value)?),
        None => None,
    };
    parsed.tracker_refs_replace = match delta
        .get("tracker_refs_replace")
        .filter(|value| !value.is_null())
    {
        Some(value) => Some(parse_ref_list(value)?),
        None => None,
    };

    parsed.review = match delta.get("review").filter(|value| !value.is_null()) {
        Some(value) => Some(parse_review_override(value)?),
        None => None,
    };

    for name in SCALAR_SLOTS {
        let value = delta.get(name).filter(|value| !value.is_null());
        let normalized = value.and_then(|value| {
            if matches!(value, Value::String(text) if text.is_empty()) {
                None
            } else {
                Some(value.clone())
            }
        });
        match name {
            "shell_pid_created_at" => parsed.shell_pid_created_at = normalized,
            "agent" => parsed.agent = normalized,
            "assignee" => parsed.assignee = normalized,
            "role" => parsed.role = normalized,
            "agent_profile" => parsed.agent_profile = normalized,
            "agent_profile_version" => parsed.agent_profile_version = normalized,
            "model" => parsed.model = normalized,
            "provider" => parsed.provider = normalized,
            _ => {}
        }
    }

    Ok(parsed)
}

fn parse_ref_list(value: &Value) -> Result<Vec<Value>, DiaryError> {
    match value {
        Value::Array(items) => Ok(items.clone()),
        // Python's ``list("abc")`` yields characters; preserve that coercion.
        Value::String(text) => Ok(text.chars().map(|c| Value::String(c.to_string())).collect()),
        _ => Err(annotation("tracker_refs must be a JSON array")),
    }
}

fn parse_review_override(value: &Value) -> Result<ReviewOverride, DiaryError> {
    let object = value
        .as_object()
        .ok_or_else(|| annotation("review must be a JSON object"))?;
    Ok(ReviewOverride {
        at: python_str(
            object
                .get("at")
                .ok_or_else(|| annotation("review missing at"))?,
        ),
        actor: python_str(
            object
                .get("actor")
                .ok_or_else(|| annotation("review missing actor"))?,
        ),
        wp_id: python_str(
            object
                .get("wp_id")
                .ok_or_else(|| annotation("review missing wp_id"))?,
        ),
        reason: python_str(
            object
                .get("reason")
                .ok_or_else(|| annotation("review missing reason"))?,
        ),
    })
}

fn parse_done_evidence(value: &Value) -> Result<DoneEvidence, DiaryError> {
    let object = value
        .as_object()
        .ok_or_else(|| transition("evidence must be a JSON object"))?;
    let review = object
        .get("review")
        .ok_or_else(|| transition("evidence missing review"))?;
    let review = parse_review_approval(review)?;
    let repos = match object.get("repos") {
        None => Vec::new(),
        Some(value) => value
            .as_array()
            .ok_or_else(|| transition("evidence repos must be an array"))?
            .iter()
            .map(parse_repo_evidence)
            .collect::<Result<Vec<_>, _>>()?,
    };
    let verification = match object.get("verification") {
        None => Vec::new(),
        Some(value) => value
            .as_array()
            .ok_or_else(|| transition("evidence verification must be an array"))?
            .iter()
            .map(parse_verification)
            .collect::<Result<Vec<_>, _>>()?,
    };
    Ok(DoneEvidence {
        review,
        repos,
        verification,
    })
}

fn parse_review_approval(value: &Value) -> Result<ReviewApproval, DiaryError> {
    let object = value
        .as_object()
        .ok_or_else(|| transition("evidence review must be an object"))?;
    Ok(ReviewApproval {
        reviewer: required_string(object, "reviewer")?,
        verdict: required_string(object, "verdict")?,
        reference: required_string(object, "reference")?,
    })
}

fn parse_repo_evidence(value: &Value) -> Result<RepoEvidence, DiaryError> {
    let object = value
        .as_object()
        .ok_or_else(|| transition("evidence repo must be an object"))?;
    let files_touched = match object.get("files_touched") {
        None => Vec::new(),
        Some(value) => value
            .as_array()
            .ok_or_else(|| transition("repo files_touched must be an array"))?
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| transition("repo file name must be a string"))
            })
            .collect::<Result<Vec<_>, _>>()?,
    };
    Ok(RepoEvidence {
        repo: required_string(object, "repo")?,
        branch: required_string(object, "branch")?,
        commit: required_string(object, "commit")?,
        files_touched,
    })
}

fn parse_verification(value: &Value) -> Result<VerificationResult, DiaryError> {
    let object = value
        .as_object()
        .ok_or_else(|| transition("evidence verification must be an object"))?;
    Ok(VerificationResult {
        command: required_string(object, "command")?,
        result: required_string(object, "result")?,
        summary: required_string(object, "summary")?,
    })
}

fn parse_review_outcome(value: &Value) -> Result<ReviewOutcome, DiaryError> {
    let object = value
        .as_object()
        .ok_or_else(|| transition("review_result must be a JSON object"))?;
    Ok(ReviewOutcome {
        reviewer: required_string(object, "reviewer")?,
        verdict: required_string(object, "verdict")?,
        reference: required_string(object, "reference")?,
        feedback_path: optional_string(object, "feedback_path"),
    })
}

fn parse_lane(object: &Map<String, Value>, name: &str) -> Result<Lane, DiaryError> {
    let text = required_string(object, name)?;
    Lane::parse_canonical(&text).map_err(|error| transition(format!("{name}: {error}")))
}

// ---------------------------------------------------------------------------
// The ONE Source-valued model/admission/fold lives in `crate::source_event`
// ---------------------------------------------------------------------------
//
// `replay` keeps this finite typed view (the `spec_kitty_events.diary`
// contract) and the re-exports below, so existing callers keep their names.
// The Source admission/model/fold itself is not duplicated here.

pub use crate::source_annotation::{
    CLAIM_RELEASE_SLOTS, DELTA_SCALAR_FIELDS, ReviewFold, SourceAnnotation, SourceDelta,
    SourceReviewOverride, python_int_coercion, python_int_value, python_list_coercion,
    python_list_value,
};
pub use crate::source_event::{
    SourceEventLogError, SourceEventStream, SourceFoldError, SourceSnapshot, SourceStatusEvent,
    SourceWpState, read_source_event_stream_from_text, read_source_transitions_from_text,
    reduce_source_diary,
};
pub use crate::source_execution::{
    SourceAnnotationPass, SourceExecution, SourceExecutionError, SourceExecutionPhase,
    SourceOperational, SourcePythonClass, SourcePythonFailure, SourceRaised, execute_source_diary,
};
pub use crate::source_metadata::{SourceResolverFailure, SourceSlugResolver};

fn mission_slug(object: &Map<String, Value>) -> String {
    if let Some(value) = object.get("mission_slug").filter(|value| is_truthy(value)) {
        return python_str(value);
    }
    if let Some(value) = object.get("feature_slug").filter(|value| is_truthy(value)) {
        return python_str(value);
    }
    String::new()
}

fn decode_actor(value: &Value) -> Result<Actor, DiaryError> {
    if let Value::Object(object) = value {
        const KEYS: [&str; 4] = ["role", "profile", "tool", "model"];
        let actual: BTreeSet<&str> = object.keys().map(String::as_str).collect();
        let expected: BTreeSet<&str> = KEYS.into_iter().collect();
        if actual != expected {
            return Err(transition(format!(
                "structured actor must contain exactly [model, profile, role, tool]; got {:?}",
                actual
            )));
        }
        let field = |name: &str| -> Result<Option<String>, DiaryError> {
            match object.get(name) {
                Some(Value::Null) | None => Ok(None),
                Some(Value::String(text)) => Ok(Some(text.clone())),
                Some(_) => Err(transition(format!(
                    "structured actor field {name:?} must be a string or null"
                ))),
            }
        };
        return Ok(Actor::Structured(StructuredActor {
            role: field("role")?,
            profile: field("profile")?,
            tool: field("tool")?,
            model: field("model")?,
        }));
    }
    Ok(Actor::Text(python_str(value)))
}

// ---------------------------------------------------------------------------
// Deterministic fold
// ---------------------------------------------------------------------------

/// Reduce parsed transitions and annotations into a canonical snapshot.
///
/// This is the finite typed view (the external `spec_kitty_events.diary`
/// contract). It feeds the ONE Source-valued fold through its typed adapter, so
/// there is no second reducer; the snapshot shape is unchanged.
pub fn reduce_diary(
    transitions: &[StatusEvent],
    annotations: &[InnerStateChanged],
) -> crate::snapshot::Snapshot {
    crate::source_event::reduce_typed_diary(transitions, annotations)
}

// ---------------------------------------------------------------------------
// Small shared coercions
// ---------------------------------------------------------------------------

pub(crate) fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(number) => number.as_f64().is_some_and(|value| value != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

pub(crate) fn python_str(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::Null => "None".to_owned(),
        Value::Number(number) => number.to_string(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn parse_int(value: &Value) -> Result<i64, DiaryError> {
    match value {
        Value::Number(number) => {
            if let Some(value) = number.as_i64() {
                return Ok(value);
            }
            if let Some(value) = number.as_f64() {
                return Ok(value.trunc() as i64);
            }
            Err(annotation("shell_pid is not an integer"))
        }
        Value::String(text) => text
            .trim()
            .parse::<i64>()
            .map_err(|_| annotation("shell_pid is not an integer")),
        Value::Bool(value) => Ok(i64::from(*value)),
        _ => Err(annotation("shell_pid is not an integer")),
    }
}

fn is_subtask_status(value: &str) -> bool {
    matches!(
        value,
        "genesis"
            | "planned"
            | "claimed"
            | "in_progress"
            | "for_review"
            | "in_review"
            | "approved"
            | "done"
            | "blocked"
            | "canceled"
            | "skipped"
    )
}

fn required_string(object: &Map<String, Value>, name: &str) -> Result<String, DiaryError> {
    object
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| transition(format!("field {name:?} must be a string")))
}

fn required_value_string(object: &Map<String, Value>, name: &str) -> Result<String, DiaryError> {
    let value = object
        .get(name)
        .ok_or_else(|| transition(format!("missing required field {name:?}")))?;
    Ok(python_str(value))
}

fn optional_string(object: &Map<String, Value>, name: &str) -> Option<String> {
    object
        .get(name)
        .filter(|value| !value.is_null())
        .map(python_str)
}

fn optional_value_string(object: &Map<String, Value>, name: &str) -> Option<String> {
    optional_string(object, name)
}

fn transition(detail: impl Into<String>) -> DiaryError {
    DiaryError::InvalidTransition {
        detail: detail.into(),
    }
}

fn annotation(detail: impl Into<String>) -> DiaryError {
    DiaryError::InvalidAnnotation {
        detail: detail.into(),
    }
}

/// True for the canonical 26-character Crockford-base32 ULID form.
pub(crate) fn is_ulid(value: &str) -> bool {
    value.len() == 26
        && value.bytes().all(|byte| {
            byte.is_ascii_digit()
                || (b'A'..=b'H').contains(&byte)
                || matches!(byte, b'J' | b'K' | b'M' | b'N' | b'P'..=b'T' | b'V'..=b'Z')
        })
}

/// Validate and return a mission slug usable as a single path segment.
pub fn safe_mission_slug(slug: &str, fallback: &str) -> String {
    if is_safe_path_segment(slug) {
        slug.to_owned()
    } else {
        fallback.to_owned()
    }
}

/// The external diary's `assert_safe_path_segment` predicate.
pub fn is_safe_path_segment(value: &str) -> bool {
    if value.is_empty() || value.trim().is_empty() {
        return false;
    }
    if value != value.trim() {
        return false;
    }
    if value.contains("..") || value.starts_with('.') {
        return false;
    }
    if value.contains('/') || value.contains('\\') || !value.is_ascii() {
        return false;
    }
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    first.is_ascii_alphanumeric()
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_segment_rules_match_the_shared_guard() {
        assert!(is_safe_path_segment("053-orchestrator-api"));
        assert!(is_safe_path_segment("900-foo_bar.v2"));
        assert!(!is_safe_path_segment(""));
        assert!(!is_safe_path_segment("  "));
        assert!(!is_safe_path_segment("../evil"));
        assert!(!is_safe_path_segment(".hidden"));
        assert!(!is_safe_path_segment("a/b"));
        assert!(!is_safe_path_segment("a b"));
        assert!(!is_safe_path_segment("nön"));
        assert_eq!(safe_mission_slug("../../evil", "trusted"), "trusted");
    }

    #[test]
    fn actor_projection_matches_the_documented_identity() {
        let actor = Actor::Structured(StructuredActor {
            role: Some("reviewer".to_owned()),
            profile: None,
            tool: Some("claude".to_owned()),
            model: None,
        });
        assert_eq!(actor.identity(), "claude");
        assert!(Actor::Text("migration:backfill".to_owned()).is_reconstruction_seed());
    }
}
