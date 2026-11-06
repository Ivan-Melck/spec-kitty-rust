//! Read-only, bounded inspection of a `status.events.jsonl` stream.
//!
//! This module **inspects**. It is not a reducer and makes no canonical
//! current-state claim: there is no `current_lane`, no last-event-wins fold,
//! and no write path. The canonical reducer belongs to the external events
//! package; exposing a partial replay here would lie about state.
//!
//! Parsed rows are partitioned into three typed families:
//!
//! - lane transition rows,
//! - off-axis `kind == "annotation"` rows,
//! - known non-lane lifecycle/decision/retrospective rows (and unknown
//!   `event_type` extensions, preserved rather than dropped).
//!
//! Unknown `kind` values are a hard error. A complete final JSON object is
//! consumed even without a newline. An incomplete JSON fragment at EOF is a
//! torn tail: it is left unconsumed and reported; malformed complete JSON fails.
//! Inspection is not a schema audit: execution modes, timestamps, force reasons,
//! review-reference invariants and evidence payload semantics are not certified.

use std::collections::BTreeMap;
use std::io::{self, BufRead};

use serde_json::{Map, Value};

use crate::lane::Lane;

/// Default maximum bytes inspected in one call (8 MiB).
pub const DEFAULT_MAX_INPUT_BYTES: usize = 8 * 1024 * 1024;
/// Default maximum bytes in one event line, excluding the newline (1 MiB).
pub const DEFAULT_MAX_LINE_BYTES: usize = 1024 * 1024;
/// Default maximum number of parsed events (100 000).
pub const DEFAULT_MAX_EVENTS: usize = 100_000;

const RETROSPECTIVE_LIFECYCLE_TYPES: &[&str] = &[
    "RetrospectiveCaptured",
    "RetrospectiveCaptureFailed",
    "RetrospectiveSkipped",
];

const LIFECYCLE_EVENT_TYPES: &[&str] = &[
    "ProjectInitialized",
    "MissionCreated",
    "SpecifyStarted",
    "SpecifyCompleted",
    "PlanStarted",
    "PlanCompleted",
    "TasksStarted",
    "TasksCompleted",
    "WPCreated",
    "ReviewerSelfApproval",
    "MissionReopened",
    "FollowUpRecorded",
];

const DECISION_POINT_EVENT_TYPES: &[&str] = &[
    "DecisionPointOpened",
    "DecisionPointResolved",
    "DecisionPointDeferred",
    "DecisionPointCanceled",
    "DecisionPointWidened",
];

const TRANSITION_KEYS: &[&str] = &[
    "event_id",
    "mission_slug",
    "feature_slug",
    "wp_id",
    "from_lane",
    "to_lane",
    "at",
    "actor",
    "force",
    "execution_mode",
    "reason",
    "reason_source",
    "review_ref",
    "evidence",
    "review_result",
    "policy_metadata",
    "mission_id",
    "kind",
];

const ANNOTATION_KEYS: &[&str] = &["event_id", "kind", "wp_id", "at", "actor", "delta"];

/// Input bounds for one inspection call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventStreamLimits {
    pub max_input_bytes: usize,
    pub max_line_bytes: usize,
    pub max_events: usize,
}

impl Default for EventStreamLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: DEFAULT_MAX_INPUT_BYTES,
            max_line_bytes: DEFAULT_MAX_LINE_BYTES,
            max_events: DEFAULT_MAX_EVENTS,
        }
    }
}

/// The discriminated family of one inspected row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Transition,
    Annotation,
    NonLane,
}

/// One lane transition event. Its `from_lane`/`to_lane` are event facts, not
/// a claim about the work package's current lane.
#[derive(Debug, Clone, PartialEq)]
pub struct TransitionRecord {
    pub event_id: String,
    pub mission_slug: Option<String>,
    pub mission_id: Option<String>,
    pub wp_id: String,
    pub from_lane: Lane,
    pub to_lane: Lane,
    pub at: String,
    pub actor: EventActor,
    pub force: bool,
    pub execution_mode: String,
    pub reason: Option<String>,
    pub reason_source: Option<String>,
    pub review_ref: Option<String>,
    pub review_result: Option<ReviewResult>,
    /// Evidence is preserved verbatim; it is not interpreted by this inspector.
    pub evidence: Option<Value>,
    /// Policy metadata is preserved verbatim.
    pub policy_metadata: Option<Value>,
    /// Unrecognized top-level fields, preserved without loss.
    pub extensions: Map<String, Value>,
    /// The complete decoded line, preserved for downstream typed consumers.
    pub raw: Map<String, Value>,
}

impl TransitionRecord {
    pub fn kind(&self) -> EventKind {
        EventKind::Transition
    }
}

/// An off-axis annotation row (`kind == "annotation"`).
#[derive(Debug, Clone, PartialEq)]
pub struct AnnotationRecord {
    pub event_id: String,
    pub wp_id: String,
    pub at: String,
    pub actor: EventActor,
    /// The delta is preserved as raw object fields, including unknown ones.
    pub delta: Map<String, Value>,
    /// Unrecognized top-level fields, preserved without loss.
    pub extensions: Map<String, Value>,
    /// The complete decoded line.
    pub raw: Map<String, Value>,
}

impl AnnotationRecord {
    pub fn kind(&self) -> EventKind {
        EventKind::Annotation
    }
}

/// Why a row was classified as non-lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NonLaneCategory {
    /// A known `event_type` lifecycle event.
    Lifecycle,
    /// A known `event_type` Decision Moment event.
    DecisionMoment,
    /// A retrospective lifecycle row using the `type` envelope.
    RetrospectiveLifecycle,
    /// A retrospective row using an `event_name` of `retrospective.*`.
    RetrospectiveEventName,
    /// Any other row carrying `event_type`; preserved, not rejected.
    UnknownExtension,
}

/// A non-lane row sharing `status.events.jsonl`.
#[derive(Debug, Clone, PartialEq)]
pub struct NonLaneRecord {
    pub event_type: Option<String>,
    pub event_name: Option<String>,
    pub category: NonLaneCategory,
    /// The complete row, so no extension payload is lost.
    pub raw: Map<String, Value>,
}

impl NonLaneRecord {
    /// The key used in [`EventCounts::non_lane_by_type`].
    pub fn count_key(&self) -> String {
        if let Some(event_type) = &self.event_type {
            return event_type.clone();
        }
        if let Some(event_name) = &self.event_name {
            return event_name.clone();
        }
        if let Some(event_type) = self.raw.get("type").and_then(Value::as_str) {
            return event_type.to_owned();
        }
        "<non-string>".to_owned()
    }
}

/// A typed actor: a plain identity or the structured resolved binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventActor {
    Text(String),
    Structured(StructuredActor),
}

/// The `{role, profile, tool, model}` resolved binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuredActor {
    pub role: Option<String>,
    pub profile: Option<String>,
    pub tool: Option<String>,
    pub model: Option<String>,
    pub profile_source: Option<String>,
}

/// A structured review result inside a transition row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewResult {
    pub reviewer: String,
    pub verdict: String,
    pub reference: String,
    pub feedback_path: Option<String>,
}

/// One inspected event.
#[derive(Debug, Clone, PartialEq)]
pub enum InspectedEvent {
    Transition(Box<TransitionRecord>),
    Annotation(AnnotationRecord),
    NonLane(NonLaneRecord),
}

impl InspectedEvent {
    pub fn kind(&self) -> EventKind {
        match self {
            InspectedEvent::Transition(record) => record.kind(),
            InspectedEvent::Annotation(record) => record.kind(),
            InspectedEvent::NonLane(_) => EventKind::NonLane,
        }
    }
}

/// Counts by family. Deliberately no lane or current-state aggregate.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventCounts {
    pub transitions: usize,
    pub annotations: usize,
    pub non_lane: usize,
    pub total: usize,
    pub non_lane_by_type: BTreeMap<String, usize>,
}

/// An unterminated trailing fragment that was not parsed or consumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrailingPartial {
    pub bytes: usize,
    /// 1-based line number the fragment occupies.
    pub line: u64,
}

/// The result of inspecting one bounded stream.
#[derive(Debug, Clone, PartialEq)]
pub struct EventStreamInspection {
    pub events: Vec<InspectedEvent>,
    pub counts: EventCounts,
    pub trailing_partial: Option<TrailingPartial>,
    /// Bytes of newline-terminated lines (including blank lines). Bytes in
    /// `trailing_partial` are not consumed.
    pub bytes_consumed: usize,
}

/// A typed event-stream inspection failure.
#[derive(Debug, thiserror::Error)]
pub enum EventStreamError {
    #[error("Invalid JSON on line {line}: {message}")]
    InvalidJson { line: u64, message: String },

    #[error("Invalid event structure on line {line}: {detail}")]
    InvalidStructure { line: u64, detail: String },

    #[error("Unknown event kind {kind} on line {line}")]
    UnknownEventKind { line: u64, kind: String },

    #[error("Event line {line} exceeds {limit} bytes ({bytes} bytes)")]
    LineTooLong {
        line: u64,
        bytes: usize,
        limit: usize,
    },

    #[error("Event stream exceeds {limit} bytes ({bytes} bytes)")]
    InputTooLarge { bytes: usize, limit: usize },

    #[error("Event stream exceeds the {limit}-event inspection bound")]
    TooManyEvents { limit: usize },

    #[error("Could not read the event stream: {0}")]
    Io(#[from] io::Error),
}

/// Inspect a byte slice with default bounds.
pub fn inspect_event_stream(input: &[u8]) -> Result<EventStreamInspection, EventStreamError> {
    inspect_event_stream_with_limits(input, &EventStreamLimits::default())
}

/// Inspect a byte slice with explicit bounds.
pub fn inspect_event_stream_with_limits(
    input: &[u8],
    limits: &EventStreamLimits,
) -> Result<EventStreamInspection, EventStreamError> {
    let mut cursor = io::Cursor::new(input);
    inspect_event_reader_with_limits(&mut cursor, limits)
}

/// Inspect a buffered reader with default bounds.
pub fn inspect_event_reader<R: BufRead>(
    reader: &mut R,
) -> Result<EventStreamInspection, EventStreamError> {
    inspect_event_reader_with_limits(reader, &EventStreamLimits::default())
}

/// Inspect a buffered reader with explicit bounds.
///
/// Reads one newline-terminated line at a time. The line buffer never grows
/// beyond `max_line_bytes + 1`, and the total is checked against
/// `max_input_bytes`; nothing is buffered past a bound violation.
pub fn inspect_event_reader_with_limits<R: BufRead>(
    reader: &mut R,
    limits: &EventStreamLimits,
) -> Result<EventStreamInspection, EventStreamError> {
    let mut events = Vec::new();
    let mut counts = EventCounts::default();
    let mut trailing_partial = None;
    let mut bytes_consumed = 0usize;
    let mut line_number = 0u64;

    loop {
        let next_line = line_number + 1;
        let mut buffer = Vec::new();
        match read_line(
            reader,
            &mut buffer,
            limits.max_line_bytes,
            limits.max_input_bytes,
            bytes_consumed,
            next_line,
        )? {
            LineRead::Eof => break,
            LineRead::Partial => {
                line_number = next_line;
                let observed = bytes_consumed.saturating_add(buffer.len());
                if observed > limits.max_input_bytes {
                    return Err(EventStreamError::InputTooLarge {
                        bytes: observed,
                        limit: limits.max_input_bytes,
                    });
                }
                if buffer.iter().all(|byte| byte.is_ascii_whitespace()) {
                    bytes_consumed = observed;
                    break;
                }
                // EOF without LF is not itself damage: a complete final JSON
                // object is a readable event, just as Python splitlines reads it.
                match serde_json::from_slice::<Value>(&buffer) {
                    Ok(Value::Object(object)) => {
                        if counts.total >= limits.max_events {
                            return Err(EventStreamError::TooManyEvents {
                                limit: limits.max_events,
                            });
                        }
                        let event = classify(object, line_number)?;
                        record(&mut events, &mut counts, event);
                        bytes_consumed = observed;
                    }
                    Ok(_) => {
                        return Err(EventStreamError::InvalidStructure {
                            line: line_number,
                            detail: "expected JSON object".to_owned(),
                        });
                    }
                    Err(error) if error.is_eof() => {
                        trailing_partial = Some(TrailingPartial {
                            bytes: buffer.len(),
                            line: line_number,
                        });
                    }
                    Err(error) => {
                        return Err(EventStreamError::InvalidJson {
                            line: line_number,
                            message: error.to_string(),
                        });
                    }
                }
                break;
            }
            LineRead::Complete => {
                line_number = next_line;
                bytes_consumed = bytes_consumed.saturating_add(buffer.len());
                if bytes_consumed > limits.max_input_bytes {
                    return Err(EventStreamError::InputTooLarge {
                        bytes: bytes_consumed,
                        limit: limits.max_input_bytes,
                    });
                }

                let content = &buffer[..buffer.len().saturating_sub(1)];
                if content.iter().all(|byte| byte.is_ascii_whitespace()) {
                    continue;
                }

                if counts.total >= limits.max_events {
                    return Err(EventStreamError::TooManyEvents {
                        limit: limits.max_events,
                    });
                }

                let value: Value = serde_json::from_slice(content).map_err(|error| {
                    EventStreamError::InvalidJson {
                        line: line_number,
                        message: error.to_string(),
                    }
                })?;
                let object = match value {
                    Value::Object(object) => object,
                    _ => {
                        return Err(EventStreamError::InvalidStructure {
                            line: line_number,
                            detail: "expected JSON object".to_owned(),
                        });
                    }
                };

                let event = classify(object, line_number)?;
                record(&mut events, &mut counts, event);
            }
        }
    }

    Ok(EventStreamInspection {
        events,
        counts,
        trailing_partial,
        bytes_consumed,
    })
}

enum LineRead {
    Eof,
    Partial,
    Complete,
}

fn read_line<R: BufRead>(
    reader: &mut R,
    output: &mut Vec<u8>,
    max_line_bytes: usize,
    max_input_bytes: usize,
    already_consumed: usize,
    line: u64,
) -> Result<LineRead, EventStreamError> {
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return if output.is_empty() {
                Ok(LineRead::Eof)
            } else {
                Ok(LineRead::Partial)
            };
        }

        if let Some(newline) = available.iter().position(|byte| *byte == b'\n') {
            let content_bytes = output.len().saturating_add(newline);
            let line_bytes = content_bytes.saturating_add(1);
            let total = already_consumed.saturating_add(line_bytes);
            if total > max_input_bytes {
                return Err(EventStreamError::InputTooLarge {
                    bytes: total,
                    limit: max_input_bytes,
                });
            }
            if content_bytes > max_line_bytes {
                return Err(EventStreamError::LineTooLong {
                    line,
                    bytes: content_bytes,
                    limit: max_line_bytes,
                });
            }
            output.extend_from_slice(&available[..=newline]);
            reader.consume(newline + 1);
            return Ok(LineRead::Complete);
        }

        let observed = output.len().saturating_add(available.len());
        let total = already_consumed.saturating_add(observed);
        if total > max_input_bytes {
            return Err(EventStreamError::InputTooLarge {
                bytes: total,
                limit: max_input_bytes,
            });
        }
        if observed > max_line_bytes {
            return Err(EventStreamError::LineTooLong {
                line,
                bytes: observed,
                limit: max_line_bytes,
            });
        }
        let consumed = available.len();
        output.extend_from_slice(available);
        reader.consume(consumed);
    }
}

fn record(events: &mut Vec<InspectedEvent>, counts: &mut EventCounts, event: InspectedEvent) {
    match &event {
        InspectedEvent::Transition(_) => counts.transitions += 1,
        InspectedEvent::Annotation(_) => counts.annotations += 1,
        InspectedEvent::NonLane(record) => {
            counts.non_lane += 1;
            *counts
                .non_lane_by_type
                .entry(record.count_key())
                .or_insert(0) += 1;
        }
    }
    counts.total += 1;
    events.push(event);
}

fn classify(object: Map<String, Value>, line: u64) -> Result<InspectedEvent, EventStreamError> {
    if let Some(kind) = object.get("kind").filter(|value| !value.is_null()) {
        return match kind.as_str() {
            Some("annotation") => Ok(InspectedEvent::Annotation(parse_annotation(object, line)?)),
            _ => Err(EventStreamError::UnknownEventKind {
                line,
                kind: render_json(kind),
            }),
        };
    }

    if let Some(event_name) = object.get("event_name").and_then(Value::as_str)
        && event_name.starts_with("retrospective.")
    {
        return Ok(InspectedEvent::NonLane(non_lane(
            object,
            NonLaneCategory::RetrospectiveEventName,
        )));
    }

    if let Some(event_type) = object.get("type").and_then(Value::as_str)
        && RETROSPECTIVE_LIFECYCLE_TYPES.contains(&event_type)
    {
        return Ok(InspectedEvent::NonLane(non_lane(
            object,
            NonLaneCategory::RetrospectiveLifecycle,
        )));
    }

    if object.contains_key("event_type") {
        let category = match object.get("event_type").and_then(Value::as_str) {
            Some(event_type) if LIFECYCLE_EVENT_TYPES.contains(&event_type) => {
                NonLaneCategory::Lifecycle
            }
            Some(event_type) if DECISION_POINT_EVENT_TYPES.contains(&event_type) => {
                NonLaneCategory::DecisionMoment
            }
            _ => NonLaneCategory::UnknownExtension,
        };
        return Ok(InspectedEvent::NonLane(non_lane(object, category)));
    }

    Ok(InspectedEvent::Transition(Box::new(parse_transition(
        object, line,
    )?)))
}

fn non_lane(object: Map<String, Value>, category: NonLaneCategory) -> NonLaneRecord {
    NonLaneRecord {
        event_type: object
            .get("event_type")
            .and_then(Value::as_str)
            .map(str::to_owned),
        event_name: object
            .get("event_name")
            .and_then(Value::as_str)
            .map(str::to_owned),
        category,
        raw: object,
    }
}

fn parse_transition(
    object: Map<String, Value>,
    line: u64,
) -> Result<TransitionRecord, EventStreamError> {
    let event_id = required_string(&object, "event_id", line)?;
    let wp_id = required_string(&object, "wp_id", line)?;
    let from_lane_text = required_string(&object, "from_lane", line)?;
    let to_lane_text = required_string(&object, "to_lane", line)?;
    let from_lane = Lane::parse_canonical(&from_lane_text)
        .map_err(|error| structure(line, format!("from_lane: {error}")))?;
    let to_lane = Lane::parse_canonical(&to_lane_text)
        .map_err(|error| structure(line, format!("to_lane: {error}")))?;
    let at = required_string(&object, "at", line)?;
    let force = match object.get("force") {
        Some(Value::Bool(force)) => *force,
        Some(_) => return Err(structure(line, "force must be a boolean")),
        None => return Err(structure(line, "missing required field \"force\"")),
    };
    let execution_mode = required_string(&object, "execution_mode", line)?;
    let actor_value = object
        .get("actor")
        .ok_or_else(|| structure(line, "missing required field \"actor\""))?;
    let actor = parse_actor(actor_value, line)?;

    let mission_slug = optional_string(&object, "mission_slug", line)?.or(optional_string(
        &object,
        "feature_slug",
        line,
    )?);
    let mission_id = optional_string(&object, "mission_id", line)?;
    let reason = optional_string(&object, "reason", line)?;
    let reason_source = optional_string(&object, "reason_source", line)?;
    let review_ref = optional_string(&object, "review_ref", line)?;

    let review_result = match object.get("review_result") {
        None | Some(Value::Null) => None,
        Some(Value::Object(value)) => Some(parse_review_result(value, line)?),
        Some(_) => {
            return Err(structure(line, "review_result must be an object"));
        }
    };
    let evidence = object
        .get("evidence")
        .filter(|value| !value.is_null())
        .cloned();
    let policy_metadata = object
        .get("policy_metadata")
        .filter(|value| !value.is_null())
        .cloned();
    let extensions = extensions(&object, TRANSITION_KEYS);

    Ok(TransitionRecord {
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
        review_result,
        evidence,
        policy_metadata,
        extensions,
        raw: object,
    })
}

fn parse_annotation(
    object: Map<String, Value>,
    line: u64,
) -> Result<AnnotationRecord, EventStreamError> {
    let event_id = required_string(&object, "event_id", line)?;
    if !is_ulid(&event_id) {
        return Err(structure(
            line,
            format!("event_id is not a valid ULID: {event_id:?}"),
        ));
    }
    let wp_id = required_string(&object, "wp_id", line)?;
    let at = required_string(&object, "at", line)?;
    let actor_value = object
        .get("actor")
        .ok_or_else(|| structure(line, "missing required field \"actor\""))?;
    let actor = parse_actor(actor_value, line)?;
    let delta = match object.get("delta") {
        Some(Value::Object(delta)) => delta.clone(),
        Some(_) => return Err(structure(line, "delta must be an object")),
        None => return Err(structure(line, "missing required field \"delta\"")),
    };
    let extensions = extensions(&object, ANNOTATION_KEYS);

    Ok(AnnotationRecord {
        event_id,
        wp_id,
        at,
        actor,
        delta,
        extensions,
        raw: object,
    })
}

fn parse_review_result(
    object: &Map<String, Value>,
    line: u64,
) -> Result<ReviewResult, EventStreamError> {
    Ok(ReviewResult {
        reviewer: required_string(object, "reviewer", line)?,
        verdict: required_string(object, "verdict", line)?,
        reference: required_string(object, "reference", line)?,
        feedback_path: optional_string(object, "feedback_path", line)?,
    })
}

fn parse_actor(value: &Value, line: u64) -> Result<EventActor, EventStreamError> {
    match value {
        Value::String(actor) => Ok(EventActor::Text(actor.clone())),
        Value::Object(object) => {
            let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
            keys.sort_unstable();
            const FOUR_KEY: [&str; 4] = ["model", "profile", "role", "tool"];
            const FIVE_KEY: [&str; 5] = ["model", "profile", "profile_source", "role", "tool"];
            if keys.as_slice() != FOUR_KEY && keys.as_slice() != FIVE_KEY {
                return Err(structure(
                    line,
                    "structured actor must contain exactly \
                     [model, profile, role, tool] or \
                     [model, profile, profile_source, role, tool]",
                ));
            }

            let role = optional_actor_field(object, "role", line)?;
            let profile = optional_actor_field(object, "profile", line)?;
            let tool = optional_actor_field(object, "tool", line)?;
            let model = optional_actor_field(object, "model", line)?;
            let profile_source = optional_actor_field(object, "profile_source", line)?;

            if let Some(source) = profile_source.as_deref() {
                const SOURCES: [&str; 4] = ["invocation", "flag", "agent_string", "wp_frontmatter"];
                if !SOURCES.contains(&source) {
                    return Err(structure(
                        line,
                        format!("structured actor profile_source is not recognized: {source:?}"),
                    ));
                }
                if profile.is_none() {
                    return Err(structure(
                        line,
                        "structured actor profile_source must be null exactly when profile is null",
                    ));
                }
            } else if object.contains_key("profile_source") && profile.is_some() {
                return Err(structure(
                    line,
                    "structured actor profile_source must be null exactly when profile is null",
                ));
            }

            Ok(EventActor::Structured(StructuredActor {
                role,
                profile,
                tool,
                model,
                profile_source,
            }))
        }
        other => Err(structure(
            line,
            format!(
                "actor must be a string or structured object; got {}",
                render_json(other)
            ),
        )),
    }
}

fn optional_actor_field(
    object: &Map<String, Value>,
    name: &str,
    line: u64,
) -> Result<Option<String>, EventStreamError> {
    match object.get(name) {
        Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(structure(
            line,
            format!("structured actor field {name:?} must be a string or null"),
        )),
        None => Ok(None),
    }
}

fn required_string(
    object: &Map<String, Value>,
    name: &str,
    line: u64,
) -> Result<String, EventStreamError> {
    match object.get(name) {
        Some(Value::String(value)) => Ok(value.clone()),
        Some(_) => Err(structure(line, format!("field {name:?} must be a string"))),
        None => Err(structure(line, format!("missing required field {name:?}"))),
    }
}

fn optional_string(
    object: &Map<String, Value>,
    name: &str,
    line: u64,
) -> Result<Option<String>, EventStreamError> {
    match object.get(name) {
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err(structure(line, format!("field {name:?} must be a string"))),
    }
}

fn extensions(object: &Map<String, Value>, known: &[&str]) -> Map<String, Value> {
    object
        .iter()
        .filter(|(key, _)| !known.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn structure(line: u64, detail: impl Into<String>) -> EventStreamError {
    EventStreamError::InvalidStructure {
        line,
        detail: detail.into(),
    }
}

fn render_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "<unrenderable>".to_owned())
}

/// The canonical ULID alphabet, 26 characters (Crockford base32).
fn is_ulid(value: &str) -> bool {
    value.len() == 26
        && value.bytes().all(|byte| {
            byte.is_ascii_digit()
                || (b'A'..=b'H').contains(&byte)
                || matches!(byte, b'J' | b'K' | b'M' | b'N' | b'P'..=b'T' | b'V'..=b'Z')
        })
}
