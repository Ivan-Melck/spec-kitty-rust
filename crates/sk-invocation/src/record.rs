//! Typed `started` / `completed` Op records, the canonical id boundary, and
//! pre-v2 (legacy) line dispatch.
//!
//! Source: `specify_cli/invocation/record.py` (schema v2). Schema v2 splits the
//! old dual-purpose record into two frozen models so a blank-default state is
//! unrepresentable: the first JSONL line is a write-once `started` event, and
//! at most one `completed` event is appended after it, carrying `outcome` and
//! `closed_by` and none of the started-only fields.
//!
//! Validation is exactly the Pydantic model's, not more:
//!
//! - `invocation_id` is an exact 26-character upper-case Crockford base32
//!   string (`^[0-9A-HJKMNP-TV-Z]{26}$`, `fullmatch`). This is **not**
//!   `ulid`'s 128-bit parse: `ZZZZ…` (26 `Z`s) is a valid id here.
//! - `profile_id`, `action`, `actor`, `started_at` and `completed_at` are
//!   non-empty strings; `request_text`, `governance_context_hash`,
//!   `router_confidence`, `mission_id`, `wp_id` and `evidence_ref` carry **no**
//!   further constraint (timestamps are not ISO-checked; the hash is not
//!   16-hex-checked; `router_confidence` is free text).
//! - `model_id` is optional but must be non-empty when present.
//! - `mode_of_work`, `outcome` and `closed_by` are literals.
//! - Unknown keys are ignored (`extra="ignore"`); no `deny_unknown_fields`
//!   equivalent is invented.
//! - `governance_context_available` uses Pydantic's lax boolean coercion
//!   (`0`/`1`, `0.0`/`1.0`, and the case-insensitive words `0/1/off/f/false/
//!   n/no` and `1/on/t/true/y/yes`; surrounding whitespace is **not** trimmed).

use std::fmt;
use std::str::FromStr;

use serde_json::{Map, Value};
use ulid::Ulid;

use crate::error::{InvocationIdError, LegacyRecordError, RecordParseError, RecordValidationError};
use crate::modes::ModeOfWork;
use crate::python_json::{PythonJsonObject, python_repr_json};

/// Upper-case Crockford base32, 26 characters. Mirrors `_ULID_RE`.
const CROCKFORD_LENGTH: usize = 26;

fn is_crockford(character: char) -> bool {
    character.is_ascii_digit()
        || matches!(character, 'A'..='H' | 'J' | 'K' | 'M' | 'N' | 'P'..='T' | 'V'..='Z')
}

/// A validated Op invocation id.
///
/// Construction goes through [`InvocationId::parse`], which applies the
/// canonical 26-character Crockford boundary. Callers must construct one
/// before composing an id into a path.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct InvocationId(String);

impl InvocationId {
    /// Validate `value` as an exact 26-character upper-case Crockford id.
    ///
    /// This is the canonical boundary for both event models and every
    /// filesystem lookup keyed by an invocation id.
    pub fn parse(value: &str) -> Result<Self, InvocationIdError> {
        let valid = value.chars().count() == CROCKFORD_LENGTH
            && value.chars().all(is_crockford)
            && value.len() == CROCKFORD_LENGTH;
        if valid {
            Ok(Self(value.to_owned()))
        } else {
            Err(InvocationIdError {
                value: value.to_owned(),
            })
        }
    }

    /// Mint a new id from a genuine ULID.
    ///
    /// The reference mints with `ulid.new().str`; the value is a valid
    /// Crockford id by construction. Minting and parsing are deliberately
    /// different operations: parsing must keep accepting any 26-character
    /// Crockford string, including ones outside `ulid`'s 128-bit range.
    pub fn mint() -> Self {
        Self(Ulid::new().to_string())
    }

    /// The id text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for InvocationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for InvocationId {
    type Err = InvocationIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

/// The terminal state of an Op (`OpCompletedEvent.outcome`).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum OpOutcome {
    /// The work completed.
    Done,
    /// The work was attempted and failed.
    Failed,
    /// The work was abandoned without a result.
    Abandoned,
}

impl OpOutcome {
    /// Every variant, in the reference's declaration order.
    pub const ALL: [Self; 3] = [Self::Done, Self::Failed, Self::Abandoned];

    /// The wire value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Abandoned => "abandoned",
        }
    }

    /// Parse a wire value exactly.
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|outcome| outcome.as_str() == value)
    }
}

/// Who closed the Op (`OpCompletedEvent.closed_by`).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ClosedBy {
    /// The invoking agent closed its own Op.
    Agent,
    /// A doctor sweep recorded the closure on the spine.
    DoctorSweep,
}

impl ClosedBy {
    /// Every variant, in the reference's declaration order.
    pub const ALL: [Self; 2] = [Self::Agent, Self::DoctorSweep];

    /// The wire value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::DoctorSweep => "doctor_sweep",
        }
    }

    /// Parse a wire value exactly.
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|closed_by| closed_by.as_str() == value)
    }
}

/// The required and optional inputs of a `started` event.
///
/// Use [`OpStartedEvent::new`] to validate them into an [`OpStartedEvent`].
#[derive(Clone, Debug)]
pub struct OpStartedFields {
    /// The validated invocation id.
    pub invocation_id: InvocationId,
    /// The profile that performed the work.
    pub profile_id: String,
    /// The canonical action token.
    pub action: String,
    /// The request text; may be empty only in query mode (the executor
    /// enforces that, not the model).
    pub request_text: String,
    /// The acting agent ('claude', 'codex', 'operator', …).
    pub actor: String,
    /// How the Op was opened.
    pub mode_of_work: ModeOfWork,
    /// First 16 hex characters of the governance-context SHA-256.
    pub governance_context_hash: String,
    /// Whether governance context was available.
    pub governance_context_available: bool,
    /// Advisory router confidence (`exact`, `canonical_verb`, …), free text.
    pub router_confidence: Option<String>,
    /// ISO-8601 UTC start timestamp (not format-validated).
    pub started_at: String,
    /// Mission id when the Op belongs to a Mission.
    pub mission_id: Option<String>,
    /// Work-package id when the Op belongs to one.
    pub wp_id: Option<String>,
    /// Advisory model id when one was recorded.
    pub model_id: Option<String>,
}

/// The v2 `started` event: the first, write-once line of an Op record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpStartedEvent {
    invocation_id: InvocationId,
    profile_id: String,
    action: String,
    request_text: String,
    actor: String,
    mode_of_work: ModeOfWork,
    governance_context_hash: String,
    governance_context_available: bool,
    router_confidence: Option<String>,
    started_at: String,
    mission_id: Option<String>,
    wp_id: Option<String>,
    model_id: Option<String>,
}

impl OpStartedEvent {
    /// Validate `fields` into a `started` event.
    ///
    /// Applies only the reference model's constraints: non-empty
    /// `profile_id`, `action`, `actor` and `started_at`, and a non-empty
    /// `model_id` when one is supplied.
    pub fn new(fields: OpStartedFields) -> Result<Self, RecordValidationError> {
        non_empty("profile_id", &fields.profile_id)?;
        non_empty("action", &fields.action)?;
        non_empty("actor", &fields.actor)?;
        non_empty("started_at", &fields.started_at)?;
        if let Some(model_id) = fields.model_id.as_deref() {
            non_empty("model_id", model_id)?;
        }
        Ok(Self {
            invocation_id: fields.invocation_id,
            profile_id: fields.profile_id,
            action: fields.action,
            request_text: fields.request_text,
            actor: fields.actor,
            mode_of_work: fields.mode_of_work,
            governance_context_hash: fields.governance_context_hash,
            governance_context_available: fields.governance_context_available,
            router_confidence: fields.router_confidence,
            started_at: fields.started_at,
            mission_id: fields.mission_id,
            wp_id: fields.wp_id,
            model_id: fields.model_id,
        })
    }

    /// Validate one already-parsed JSON object as a `started` event.
    ///
    /// The `event` key is optional here because the Pydantic model declares a
    /// default; when present it must be exactly `"started"`.
    pub fn from_object(object: &Map<String, Value>) -> Result<Self, RecordValidationError> {
        if let Some(event) = object.get("event") {
            expect_literal("event", event, &["started"])?;
        }
        let invocation_id = match required_string(object, "invocation_id")? {
            Some(value) => InvocationId::parse(&value)
                .map_err(|error| RecordValidationError::new("invocation_id", error.to_string()))?,
            None => {
                return Err(RecordValidationError::new(
                    "invocation_id",
                    "Field required",
                ));
            }
        };
        let profile_id = required_non_empty(object, "profile_id")?;
        let action = required_non_empty(object, "action")?;
        let request_text = match required_string(object, "request_text")? {
            Some(value) => value,
            None => return Err(RecordValidationError::new("request_text", "Field required")),
        };
        let actor = required_non_empty(object, "actor")?;
        let mode_of_work = match object.get("mode_of_work") {
            None => {
                return Err(RecordValidationError::new("mode_of_work", "Field required"));
            }
            Some(value) => {
                expect_literal(
                    "mode_of_work",
                    value,
                    &["task_execution", "advisory", "mission_step", "query"],
                )?;
                value
                    .as_str()
                    .and_then(ModeOfWork::parse)
                    .expect("literal check accepts only known modes")
            }
        };
        let governance_context_hash = match required_string(object, "governance_context_hash")? {
            Some(value) => value,
            None => {
                return Err(RecordValidationError::new(
                    "governance_context_hash",
                    "Field required",
                ));
            }
        };
        let governance_context_available = match object.get("governance_context_available") {
            None => {
                return Err(RecordValidationError::new(
                    "governance_context_available",
                    "Field required",
                ));
            }
            Some(value) => required_bool("governance_context_available", value)?,
        };
        let router_confidence = optional_string(object, "router_confidence")?;
        let started_at = required_non_empty(object, "started_at")?;
        let mission_id = optional_string(object, "mission_id")?;
        let wp_id = optional_string(object, "wp_id")?;
        let model_id = optional_string(object, "model_id")?;
        if let Some(model_id) = model_id.as_deref() {
            non_empty("model_id", model_id)?;
        }
        Ok(Self {
            invocation_id,
            profile_id,
            action,
            request_text,
            actor,
            mode_of_work,
            governance_context_hash,
            governance_context_available,
            router_confidence,
            started_at,
            mission_id,
            wp_id,
            model_id,
        })
    }

    /// The invocation id.
    pub fn invocation_id(&self) -> &InvocationId {
        &self.invocation_id
    }

    /// The profile id.
    pub fn profile_id(&self) -> &str {
        &self.profile_id
    }

    /// The canonical action token.
    pub fn action(&self) -> &str {
        &self.action
    }

    /// The request text.
    pub fn request_text(&self) -> &str {
        &self.request_text
    }

    /// The acting agent.
    pub fn actor(&self) -> &str {
        &self.actor
    }

    /// How the Op was opened.
    pub fn mode_of_work(&self) -> ModeOfWork {
        self.mode_of_work
    }

    /// The governance-context hash text.
    pub fn governance_context_hash(&self) -> &str {
        &self.governance_context_hash
    }

    /// Whether governance context was available.
    pub fn governance_context_available(&self) -> bool {
        self.governance_context_available
    }

    /// The advisory router confidence, when recorded.
    pub fn router_confidence(&self) -> Option<&str> {
        self.router_confidence.as_deref()
    }

    /// The started-at timestamp text.
    pub fn started_at(&self) -> &str {
        &self.started_at
    }

    /// The Mission id, when recorded.
    pub fn mission_id(&self) -> Option<&str> {
        self.mission_id.as_deref()
    }

    /// The work-package id, when recorded.
    pub fn wp_id(&self) -> Option<&str> {
        self.wp_id.as_deref()
    }

    /// The advisory model id, when recorded.
    pub fn model_id(&self) -> Option<&str> {
        self.model_id.as_deref()
    }

    /// Serialise to the exact single-line JSON shape the reference writes.
    ///
    /// Field order is the declared order, `None` members are omitted, and the
    /// separators/escaping are Python's `json.dumps` defaults.
    pub fn to_jsonl_line(&self) -> String {
        let mut line = PythonJsonObject::new();
        line.string("event", "started");
        line.string("invocation_id", self.invocation_id.as_str());
        line.string("profile_id", &self.profile_id);
        line.string("action", &self.action);
        line.string("request_text", &self.request_text);
        line.string("actor", &self.actor);
        line.string("mode_of_work", self.mode_of_work.as_str());
        line.string("governance_context_hash", &self.governance_context_hash);
        line.boolean(
            "governance_context_available",
            self.governance_context_available,
        );
        line.optional_string("router_confidence", self.router_confidence.as_deref());
        line.string("started_at", &self.started_at);
        line.optional_string("mission_id", self.mission_id.as_deref());
        line.optional_string("wp_id", self.wp_id.as_deref());
        line.optional_string("model_id", self.model_id.as_deref());
        line.finish()
    }
}

/// The required and optional inputs of a `completed` event.
#[derive(Clone, Debug)]
pub struct OpCompletedFields {
    /// The validated invocation id; must match the record file.
    pub invocation_id: InvocationId,
    /// ISO-8601 UTC completion timestamp (not format-validated).
    pub completed_at: String,
    /// The terminal state.
    pub outcome: OpOutcome,
    /// Who closed the Op.
    pub closed_by: ClosedBy,
    /// Tier 2 evidence reference, when promoted.
    pub evidence_ref: Option<String>,
}

/// The v2 `completed` event: appended at most once, after the `started` line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpCompletedEvent {
    invocation_id: InvocationId,
    completed_at: String,
    outcome: OpOutcome,
    closed_by: ClosedBy,
    evidence_ref: Option<String>,
}

impl OpCompletedEvent {
    /// Validate `fields` into a `completed` event (non-empty `completed_at`).
    pub fn new(fields: OpCompletedFields) -> Result<Self, RecordValidationError> {
        non_empty("completed_at", &fields.completed_at)?;
        Ok(Self {
            invocation_id: fields.invocation_id,
            completed_at: fields.completed_at,
            outcome: fields.outcome,
            closed_by: fields.closed_by,
            evidence_ref: fields.evidence_ref,
        })
    }

    /// Validate one already-parsed JSON object as a `completed` event.
    ///
    /// `closed_by` is required (its absence is a legacy line, detected by
    /// [`parse_op_event`] before this validator runs).
    pub fn from_object(object: &Map<String, Value>) -> Result<Self, RecordValidationError> {
        if let Some(event) = object.get("event") {
            expect_literal("event", event, &["completed"])?;
        }
        let invocation_id = match required_string(object, "invocation_id")? {
            Some(value) => InvocationId::parse(&value)
                .map_err(|error| RecordValidationError::new("invocation_id", error.to_string()))?,
            None => {
                return Err(RecordValidationError::new(
                    "invocation_id",
                    "Field required",
                ));
            }
        };
        let completed_at = required_non_empty(object, "completed_at")?;
        let outcome = match object.get("outcome") {
            None => return Err(RecordValidationError::new("outcome", "Field required")),
            Some(value) => {
                expect_literal("outcome", value, &["done", "failed", "abandoned"])?;
                value
                    .as_str()
                    .and_then(OpOutcome::parse)
                    .expect("literal check accepts only known outcomes")
            }
        };
        let closed_by = match object.get("closed_by") {
            None => return Err(RecordValidationError::new("closed_by", "Field required")),
            Some(value) => {
                expect_literal("closed_by", value, &["agent", "doctor_sweep"])?;
                value
                    .as_str()
                    .and_then(ClosedBy::parse)
                    .expect("literal check accepts only known closers")
            }
        };
        let evidence_ref = optional_string(object, "evidence_ref")?;
        Ok(Self {
            invocation_id,
            completed_at,
            outcome,
            closed_by,
            evidence_ref,
        })
    }

    /// The invocation id.
    pub fn invocation_id(&self) -> &InvocationId {
        &self.invocation_id
    }

    /// The completed-at timestamp text.
    pub fn completed_at(&self) -> &str {
        &self.completed_at
    }

    /// The terminal state.
    pub fn outcome(&self) -> OpOutcome {
        self.outcome
    }

    /// Who closed the Op.
    pub fn closed_by(&self) -> ClosedBy {
        self.closed_by
    }

    /// The Tier 2 evidence reference, when recorded.
    pub fn evidence_ref(&self) -> Option<&str> {
        self.evidence_ref.as_deref()
    }

    /// Serialise to the exact single-line JSON shape the reference writes.
    pub fn to_jsonl_line(&self) -> String {
        let mut line = PythonJsonObject::new();
        line.string("event", "completed");
        line.string("invocation_id", self.invocation_id.as_str());
        line.string("completed_at", &self.completed_at);
        line.string("outcome", self.outcome.as_str());
        line.string("closed_by", self.closed_by.as_str());
        line.optional_string("evidence_ref", self.evidence_ref.as_deref());
        line.finish()
    }
}

/// A dispatched Op lifecycle event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OpEvent {
    /// The `started` event.
    Started(OpStartedEvent),
    /// The `completed` event.
    Completed(OpCompletedEvent),
}

/// Dispatch a parsed JSONL object to the right v2 event model.
///
/// - `completed` without `closed_by`, or any line that is not v2-parseable,
///   is [`RecordParseError::Legacy`] (catchable so readers and the migration
///   can identify legacy lines deliberately);
/// - any other `event` value (including a missing one) is
///   [`RecordParseError::NotLifecycle`], matching the reference's plain
///   `ValueError`.
pub fn parse_op_event(value: &Value) -> Result<OpEvent, RecordParseError> {
    let object = match value {
        Value::Object(object) => object,
        other => {
            return Err(RecordParseError::NotLifecycle(format!(
                "not an Op lifecycle event: {}",
                python_repr_json(other)
            )));
        }
    };
    let invocation_id = object
        .get("invocation_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    match object.get("event") {
        Some(Value::String(event)) if event == "completed" => {
            if !object.contains_key("closed_by") {
                return Err(LegacyRecordError {
                    invocation_id,
                    reason: "completed event lacks 'closed_by'".to_owned(),
                }
                .into());
            }
            OpCompletedEvent::from_object(object)
                .map(OpEvent::Completed)
                .map_err(|error| {
                    LegacyRecordError {
                        invocation_id,
                        reason: format!("completed event not v2-parseable: {error}"),
                    }
                    .into()
                })
        }
        Some(Value::String(event)) if event == "started" => OpStartedEvent::from_object(object)
            .map(OpEvent::Started)
            .map_err(|error| {
                LegacyRecordError {
                    invocation_id,
                    reason: format!("started event not v2-parseable: {error}"),
                }
                .into()
            }),
        other => Err(RecordParseError::NotLifecycle(format!(
            "not an Op lifecycle event: {}",
            python_repr_json(other.unwrap_or(&Value::Null))
        ))),
    }
}

fn non_empty(field: &'static str, value: &str) -> Result<(), RecordValidationError> {
    if value.is_empty() {
        return Err(RecordValidationError::new(
            field,
            "String should have at least 1 character",
        ));
    }
    Ok(())
}

fn required_string(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<Option<String>, RecordValidationError> {
    match object.get(field) {
        None => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(RecordValidationError::new(
            field,
            "Input should be a valid string",
        )),
    }
}

fn required_non_empty(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<String, RecordValidationError> {
    match required_string(object, field)? {
        Some(value) => {
            non_empty(field, &value)?;
            Ok(value)
        }
        None => Err(RecordValidationError::new(field, "Field required")),
    }
}

/// A `str | None` field: JSON `null` and absence are both `None`; any string
/// (including the empty string) is accepted.
fn optional_string(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<Option<String>, RecordValidationError> {
    match object.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(RecordValidationError::new(
            field,
            "Input should be a valid string",
        )),
    }
}

fn required_bool(field: &'static str, value: &Value) -> Result<bool, RecordValidationError> {
    match value {
        Value::Bool(value) => Ok(*value),
        Value::Number(number) => match number.as_f64() {
            Some(0.0) => Ok(false),
            Some(1.0) => Ok(true),
            _ => Err(RecordValidationError::new(
                field,
                "Input should be a valid boolean, unable to interpret input",
            )),
        },
        Value::String(text) => match text.to_ascii_lowercase().as_str() {
            "0" | "off" | "f" | "false" | "n" | "no" => Ok(false),
            "1" | "on" | "t" | "true" | "y" | "yes" => Ok(true),
            _ => Err(RecordValidationError::new(
                field,
                "Input should be a valid boolean, unable to interpret input",
            )),
        },
        _ => Err(RecordValidationError::new(
            field,
            "Input should be a valid boolean",
        )),
    }
}

fn expect_literal(
    field: &'static str,
    value: &Value,
    allowed: &[&str],
) -> Result<(), RecordValidationError> {
    if let Value::String(text) = value
        && allowed.contains(&text.as_str())
    {
        return Ok(());
    }
    let rendered = allowed.join("', '");
    Err(RecordValidationError::new(
        field,
        format!("Input should be '{rendered}'"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn started(id: &str) -> OpStartedEvent {
        OpStartedEvent::new(OpStartedFields {
            invocation_id: InvocationId::parse(id).unwrap(),
            profile_id: "implementer-fixture".to_owned(),
            action: "implement".to_owned(),
            request_text: "fix that bug".to_owned(),
            actor: "claude".to_owned(),
            mode_of_work: ModeOfWork::TaskExecution,
            governance_context_hash: "d5ccab5678dcc4c8".to_owned(),
            governance_context_available: true,
            router_confidence: Some("canonical_verb".to_owned()),
            started_at: "2026-06-10T20:00:00+00:00".to_owned(),
            mission_id: None,
            wp_id: None,
            model_id: None,
        })
        .unwrap()
    }

    fn object(value: Value) -> Map<String, Value> {
        match value {
            Value::Object(object) => object,
            other => panic!("expected an object, got {other}"),
        }
    }

    #[test]
    fn id_boundary_is_the_regex_not_ulids_decoder() {
        // `ulid` 1.x's `from_string` is lenient (it accepts 26 `Z`s and even
        // truncates the two overflow bits), so it can never be the boundary.
        let decoded = Ulid::from_string(&"Z".repeat(26));
        assert!(
            decoded.is_ok(),
            "the crate accepts the out-of-range spelling"
        );
        assert_ne!(
            decoded.unwrap().to_string(),
            "Z".repeat(26),
            "…and loses the two overflow bits, so it cannot be the boundary"
        );
        assert!(InvocationId::parse(&"Z".repeat(26)).is_ok());
        assert!(InvocationId::parse(&"0".repeat(26)).is_ok());
        assert!(InvocationId::parse("01KTK5JBD69FQ8XVRFV1J630MJ").is_ok());
        for rejected in [
            "abcdefghjkmnpqrstvwxyz12345".to_owned(),
            "01KTK5JBD69FQ8XVRFV1J630MI".to_owned(),
            "01KTK5JBD69FQ8XVRFV1J630ML".to_owned(),
            "01KTK5JBD69FQ8XVRFV1J630MO".to_owned(),
            "01KTK5JBD69FQ8XVRFV1J630MU".to_owned(),
            "Z".repeat(25),
            "Z".repeat(27),
            format!("{}\n", "Z".repeat(25)),
            "01KTK5JBD69FQ8XVRFV1J630M ".to_owned(),
            String::new(),
        ] {
            assert!(
                InvocationId::parse(&rejected).is_err(),
                "accepted {rejected:?}"
            );
        }
    }

    #[test]
    fn minted_ids_pass_the_boundary_and_round_trip_through_ulid() {
        let id = InvocationId::mint();
        assert_eq!(id.as_str().len(), 26);
        assert!(InvocationId::parse(id.as_str()).is_ok());
        assert_eq!(
            Ulid::from_string(id.as_str()).unwrap().to_string(),
            id.as_str()
        );
    }

    #[test]
    fn started_line_uses_declared_order_and_omits_none() {
        assert_eq!(
            started("01KTK5JBD69FQ8XVRFV1J630MJ").to_jsonl_line(),
            r#"{"event": "started", "invocation_id": "01KTK5JBD69FQ8XVRFV1J630MJ", "profile_id": "implementer-fixture", "action": "implement", "request_text": "fix that bug", "actor": "claude", "mode_of_work": "task_execution", "governance_context_hash": "d5ccab5678dcc4c8", "governance_context_available": true, "router_confidence": "canonical_verb", "started_at": "2026-06-10T20:00:00+00:00"}"#
        );
    }

    #[test]
    fn completed_line_omits_absent_evidence_ref() {
        let completed = OpCompletedEvent::new(OpCompletedFields {
            invocation_id: InvocationId::parse("01KTK5JBD69FQ8XVRFV1J630MJ").unwrap(),
            completed_at: "2026-06-10T20:25:00+00:00".to_owned(),
            outcome: OpOutcome::Done,
            closed_by: ClosedBy::Agent,
            evidence_ref: None,
        })
        .unwrap();
        assert_eq!(
            completed.to_jsonl_line(),
            r#"{"event": "completed", "invocation_id": "01KTK5JBD69FQ8XVRFV1J630MJ", "completed_at": "2026-06-10T20:25:00+00:00", "outcome": "done", "closed_by": "agent"}"#
        );
    }

    #[test]
    fn unknown_keys_are_ignored_and_dropped() {
        let event = OpStartedEvent::from_object(&object(
            json!({"event": "started", "invocation_id": "01KTK5JBD69FQ8XVRFV1J630MJ", "profile_id": "p", "action": "implement", "request_text": "r", "actor": "claude", "mode_of_work": "task_execution", "governance_context_hash": "h", "governance_context_available": true, "started_at": "2026-06-10T20:00:00+00:00", "extra_field": "ignored"}),
        ))
        .unwrap();
        assert!(!event.to_jsonl_line().contains("extra_field"));
    }

    #[test]
    fn free_text_fields_are_not_over_validated() {
        let event = OpStartedEvent::from_object(&object(json!({"event": "started", "invocation_id": "01KTK5JBD69FQ8XVRFV1J630MJ", "profile_id": "p", "action": "implement", "request_text": "", "actor": "claude", "mode_of_work": "query", "governance_context_hash": "", "governance_context_available": true, "router_confidence": "totally arbitrary thing!", "started_at": "not-a-time", "mission_id": "", "wp_id": ""})))
        .unwrap();
        assert_eq!(event.governance_context_hash(), "");
        assert_eq!(event.router_confidence(), Some("totally arbitrary thing!"));
        assert_eq!(event.started_at(), "not-a-time");
        assert_eq!(event.mission_id(), Some(""));
        assert_eq!(event.wp_id(), Some(""));
    }

    #[test]
    fn model_id_of_one_character_is_the_optional_floor() {
        let fields = |model_id: Option<String>| OpStartedFields {
            invocation_id: InvocationId::parse("01KTK5JBD69FQ8XVRFV1J630MJ").unwrap(),
            profile_id: "p".to_owned(),
            action: "implement".to_owned(),
            request_text: "r".to_owned(),
            actor: "claude".to_owned(),
            mode_of_work: ModeOfWork::TaskExecution,
            governance_context_hash: "h".to_owned(),
            governance_context_available: true,
            router_confidence: None,
            started_at: "s".to_owned(),
            mission_id: None,
            wp_id: None,
            model_id,
        };
        assert!(OpStartedEvent::new(fields(Some("m".to_owned()))).is_ok());
        assert!(OpStartedEvent::new(fields(Some(String::new()))).is_err());
    }

    #[test]
    fn lax_boolean_coercion_matches_pydantic() {
        let build = |available: Value| {
            OpStartedEvent::from_object(&object(json!({"event": "started", "invocation_id": "01KTK5JBD69FQ8XVRFV1J630MJ", "profile_id": "p", "action": "implement", "request_text": "r", "actor": "claude", "mode_of_work": "task_execution", "governance_context_hash": "h", "governance_context_available": available, "started_at": "s"})))
                .map(|event| event.governance_context_available())
        };
        for (value, expected) in [
            (json!(true), true),
            (json!(false), false),
            (json!(1), true),
            (json!(0), false),
            (json!(1.0), true),
            (json!(0.0), false),
            (json!("true"), true),
            (json!("TRUE"), true),
            (json!("yes"), true),
            (json!("on"), true),
            (json!("t"), true),
            (json!("off"), false),
            (json!("no"), false),
            (json!("f"), false),
            (json!("1"), true),
            (json!("0"), false),
        ] {
            assert_eq!(build(value.clone()).unwrap(), expected, "value {value}");
        }
        // Rejected spellings: padded text is not trimmed, numeric values other
        // than 0/1 are not booleans, and `null`/containers are type errors.
        for rejected in [
            json!(" true "),
            json!("2"),
            json!(2),
            json!(0.5),
            json!(null),
            json!([]),
            json!({}),
        ] {
            assert!(build(rejected.clone()).is_err(), "accepted {rejected}");
        }
    }

    #[test]
    fn parse_dispatch_and_legacy_detection() {
        let legacy_completed = json!({"event": "completed", "invocation_id": "01KTK5JBD69FQ8XVRFV1J630MJ", "profile_id": "p", "action": "", "actor": "unknown", "completed_at": "2026-06-10T20:25:00+00:00", "outcome": null});
        match parse_op_event(&legacy_completed) {
            Err(RecordParseError::Legacy(error)) => {
                assert_eq!(
                    error.invocation_id.as_deref(),
                    Some("01KTK5JBD69FQ8XVRFV1J630MJ")
                );
                assert_eq!(error.reason, "completed event lacks 'closed_by'");
                assert!(error.to_string().contains("spec-kitty upgrade"));
            }
            other => panic!("expected a legacy error, got {other:?}"),
        }

        let legacy_started = json!({"event": "started", "invocation_id": "01KTK5JBD69FQ8XVRFV1J630MJ", "profile_id": "p", "action": "implement"});
        assert!(matches!(
            parse_op_event(&legacy_started),
            Err(RecordParseError::Legacy(error)) if error.reason.starts_with("started event not v2-parseable")
        ));

        let link = json!({"event": "artifact_link", "invocation_id": "01KTK5JBD69FQ8XVRFV1J630MJ"});
        match parse_op_event(&link) {
            Err(RecordParseError::NotLifecycle(message)) => {
                assert_eq!(message, "not an Op lifecycle event: 'artifact_link'");
            }
            other => panic!("expected a not-lifecycle error, got {other:?}"),
        }
        assert_eq!(
            parse_op_event(&json!({"invocation_id": "x"}))
                .unwrap_err()
                .to_string(),
            "not an Op lifecycle event: None"
        );
        assert_eq!(
            parse_op_event(&json!({"event": 5}))
                .unwrap_err()
                .to_string(),
            "not an Op lifecycle event: 5"
        );
    }

    #[test]
    fn literal_and_required_fields_fail_like_the_reference_model() {
        for (mutation, field) in [
            (json!({"action": ""}), "action"),
            (json!({"profile_id": ""}), "profile_id"),
            (json!({"actor": ""}), "actor"),
            (json!({"started_at": ""}), "started_at"),
            (json!({"mode_of_work": "bogus"}), "mode_of_work"),
            (json!({"mode_of_work": "TASK_EXECUTION"}), "mode_of_work"),
            (json!({"profile_id": 123}), "profile_id"),
            (json!({"request_text": null}), "request_text"),
            (json!({"model_id": ""}), "model_id"),
            (json!({"model_id": 5}), "model_id"),
        ] {
            let mut base = json!({"event": "started", "invocation_id": "01KTK5JBD69FQ8XVRFV1J630MJ", "profile_id": "p", "action": "implement", "request_text": "r", "actor": "claude", "mode_of_work": "task_execution", "governance_context_hash": "h", "governance_context_available": true, "started_at": "s"});
            if let (Value::Object(base), Value::Object(mutation)) = (&mut base, &mutation) {
                for (key, value) in mutation {
                    base.insert(key.clone(), value.clone());
                }
            }
            let error = OpStartedEvent::from_object(&object(base)).unwrap_err();
            assert_eq!(error.field, field, "mutation {mutation}");
        }
    }
}
