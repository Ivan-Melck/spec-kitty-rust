//! Typed lifecycle payload models and their canonicalizing validation.
//!
//! The pinned producer validates every lifecycle payload against a pydantic
//! model **before** the envelope is built:
//!
//! - `spec_kitty_events.project_lifecycle` (11.3.0) for
//!   `ProjectInitialized`, the six Specify/Plan/Tasks phase events, `WPCreated`
//!   and `ReviewerSelfApproval`;
//! - `spec_kitty_events.lifecycle.MissionCreatedPayload` (via
//!   `core.mission_payload.build_mission_created_payload`) for `MissionCreated`;
//! - the core-local shapes of `MissionReopened` / `FollowUpRecorded`.
//!
//! `persist_lifecycle_event_local` itself is payload-agnostic in the baseline
//! (`Mapping[str, Any]`), so a caller that skips the convenience emitter could
//! write arbitrary JSON. This crate's writer validates at the persistence seam:
//! a known event type with a payload that drifts from the pinned model is a
//! typed refusal rather than a persisted fact, and the accepted payload is
//! re-encoded in the canonical wire shape the models dump.
//!
//! Canonicalisation reproduced here:
//!
//! - required / optional presence and nullability per model;
//! - `min_length=1` on the fields that declare it (an empty string is a
//!   refusal, not a persisted blank);
//! - `ge=0` / `ge=1` integer bounds;
//! - `extra="forbid"` — an unknown key is a refusal. The one documented
//!   exception is the baseline's local-only `artifact_path` on the three
//!   *Started* events (`emit_artifact_phase_local` appends it to the local log
//!   after the canonical model, which does not declare it, has validated);
//! - `apply_keep_none_fields` for `MissionCreated` (only `mission_number`
//!   survives as an explicit `null`) versus `exclude_none=False` for the other
//!   builders (their optional `None`s stay explicit `null`s);
//! - datetime normalisation for the pydantic `datetime` slots (`at`,
//!   `initialized_at`) in the `model_dump(mode="json")` shape: UTC renders as
//!   `...Z`, microseconds are six digits when non-zero, and a naive value keeps
//!   no offset. The kernel parser normalises offsets to UTC, so a non-UTC
//!   offset renders as its UTC instant — a recorded divergence from pydantic,
//!   which preserves the original offset.
//!
//! The `timestamp` field of `ReviewerSelfApproval` and `created_at` of
//! `MissionCreated` are **strings** in their models, not datetimes, so they are
//! validated as non-empty strings and passed through unchanged.

use std::collections::BTreeSet;

use serde_json::{Map, Value, json};

use crate::mutation::MutationError;
use crate::mutation_delta::python_int;

/// The three artifact phase prefixes.
const ARTIFACT_PHASES: [(&str, &str); 3] =
    [("Specify", "specify"), ("Plan", "plan"), ("Tasks", "tasks")];

fn invalid(event_type: &str, reason: impl Into<String>) -> MutationError {
    MutationError::InvalidLifecyclePayload {
        event_type: event_type.to_owned(),
        reason: reason.into(),
    }
}

/// pydantic's lax `int` coercion: an integer number, a bool, or a numeric
/// string. A fractional number is refused (pydantic does not truncate in lax
/// mode), as is a container.
fn integer_field(
    event_type: &str,
    key: &str,
    value: &Value,
    minimum: i64,
) -> Result<i64, MutationError> {
    let parsed = match value {
        Value::Number(number) => number.as_i64(),
        Value::String(_) | Value::Bool(_) => python_int(value),
        _ => None,
    }
    .ok_or_else(|| invalid(event_type, format!("{key} must be an integer, got {value}")))?;
    if parsed < minimum {
        return Err(invalid(
            event_type,
            format!("{key} must be >= {minimum}, got {parsed}"),
        ));
    }
    Ok(parsed)
}

struct Payload<'a> {
    event_type: &'a str,
    map: &'a Map<String, Value>,
    consumed: BTreeSet<&'a str>,
}

impl<'a> Payload<'a> {
    fn new(event_type: &'a str, value: &'a Value) -> Result<Self, MutationError> {
        let Value::Object(map) = value else {
            return Err(invalid(event_type, "the payload must be a JSON object"));
        };
        Ok(Self {
            event_type,
            map,
            consumed: BTreeSet::new(),
        })
    }

    fn take(&mut self, key: &'a str) -> Option<&'a Value> {
        let value = self.map.get(key);
        if value.is_some() {
            self.consumed.insert(key);
        }
        value
    }

    /// A required, non-empty string (`str` + `min_length=1`).
    fn require_string(&mut self, key: &'a str) -> Result<String, MutationError> {
        let value = self
            .take(key)
            .ok_or_else(|| invalid(self.event_type, format!("{key} is required")))?;
        let text = value
            .as_str()
            .ok_or_else(|| invalid(self.event_type, format!("{key} must be a string")))?;
        if text.is_empty() {
            return Err(invalid(self.event_type, format!("{key} must not be empty")));
        }
        Ok(text.to_owned())
    }

    /// An optional string, `None` for an absent **or explicitly null** value.
    /// `min_length` mirrors the model's declared constraint on present values.
    fn optional_string(
        &mut self,
        key: &'a str,
        min_length: bool,
    ) -> Result<Option<String>, MutationError> {
        match self.take(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(text)) => {
                if min_length && text.is_empty() {
                    return Err(invalid(self.event_type, format!("{key} must not be empty")));
                }
                Ok(Some(text.clone()))
            }
            Some(other) => Err(invalid(
                self.event_type,
                format!("{key} must be a string, got {other}"),
            )),
        }
    }

    /// A required boolean (pydantic lax: `"true"`/`"false"` and `0`/`1` too).
    fn require_bool(&mut self, key: &'a str) -> Result<bool, MutationError> {
        let value = self
            .take(key)
            .ok_or_else(|| invalid(self.event_type, format!("{key} is required")))?;
        match value {
            Value::Bool(flag) => Ok(*flag),
            Value::String(text) if text == "true" => Ok(true),
            Value::String(text) if text == "false" => Ok(false),
            Value::Number(number) if number.as_i64() == Some(1) => Ok(true),
            Value::Number(number) if number.as_i64() == Some(0) => Ok(false),
            other => Err(invalid(
                self.event_type,
                format!("{key} must be a boolean, got {other}"),
            )),
        }
    }

    /// A required integer with a lower bound (pydantic lax int coercion).
    fn require_int(&mut self, key: &'a str, minimum: i64) -> Result<i64, MutationError> {
        let value = self
            .take(key)
            .ok_or_else(|| invalid(self.event_type, format!("{key} is required")))?;
        integer_field(self.event_type, key, value, minimum)
    }

    /// An optional integer, `None` for absent **or explicitly null**.
    fn optional_int(&mut self, key: &'a str, minimum: i64) -> Result<Option<i64>, MutationError> {
        match self.take(key) {
            None | Some(Value::Null) => Ok(None),
            Some(value) => integer_field(self.event_type, key, value, minimum).map(Some),
        }
    }

    /// An optional array of strings, defaulting to `[]` when absent or null.
    fn string_array(&mut self, key: &'a str) -> Result<Vec<String>, MutationError> {
        match self.take(key) {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(Value::Array(items)) => {
                let mut values = Vec::with_capacity(items.len());
                for item in items {
                    let Value::String(text) = item else {
                        return Err(invalid(
                            self.event_type,
                            format!("{key} must be an array of strings, got {item}"),
                        ));
                    };
                    values.push(text.clone());
                }
                Ok(values)
            }
            Some(other) => Err(invalid(
                self.event_type,
                format!("{key} must be an array of strings, got {other}"),
            )),
        }
    }

    /// An optional pydantic `datetime` slot in its `model_dump(mode="json")` form.
    fn optional_datetime(&mut self, key: &'a str) -> Result<Option<String>, MutationError> {
        let text = match self.take(key) {
            None | Some(Value::Null) => return Ok(None),
            Some(Value::String(text)) => text.clone(),
            Some(other) => {
                return Err(invalid(
                    self.event_type,
                    format!("{key} must be an ISO-8601 string, got {other}"),
                ));
            }
        };
        Ok(Some(normalize_datetime(self.event_type, key, &text)?))
    }

    /// An optional nested object preserved verbatim (`cleared_merge`).
    fn optional_object(&mut self, key: &'a str) -> Result<Option<Value>, MutationError> {
        match self.take(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Object(_)) => Ok(Some(self.map[key].clone())),
            Some(other) => Err(invalid(
                self.event_type,
                format!("{key} must be an object, got {other}"),
            )),
        }
    }

    fn finish(self, allowed_local_extras: &[&str]) -> Result<(), MutationError> {
        let allowed: BTreeSet<&str> = allowed_local_extras.iter().copied().collect();
        if let Some(extra) = self
            .map
            .keys()
            .find(|key| !self.consumed.contains(key.as_str()) && !allowed.contains(key.as_str()))
        {
            return Err(invalid(
                self.event_type,
                format!(
                    "unknown payload key {extra:?} (the pinned model forbids extra fields; expected one of {:?})",
                    self.consumed
                ),
            ));
        }
        Ok(())
    }
}

/// Canonicalise one lifecycle payload for `event_type`.
///
/// `event_type` must be one of the closed set in
/// [`crate::mutation_lifecycle::LIFECYCLE_EVENT_TYPES`]; an unknown type is the
/// caller's skip decision (taken before this function), so it is a refusal here.
pub fn canonicalize_lifecycle_payload(
    event_type: &str,
    payload: &Value,
) -> Result<Value, MutationError> {
    let mut reader = Payload::new(event_type, payload)?;
    let canonical = match event_type {
        "ProjectInitialized" => {
            let project_uuid = reader.require_string("project_uuid")?;
            let actor = reader.require_string("actor")?;
            let project_slug = reader.optional_string("project_slug", true)?;
            let runtime_version = reader.optional_string("runtime_version", true)?;
            let initialized_at = reader.optional_datetime("initialized_at")?;
            reader.finish(&[])?;
            json!({
                "project_uuid": project_uuid,
                "project_slug": project_slug,
                "actor": actor,
                "runtime_version": runtime_version,
                "initialized_at": initialized_at,
            })
        }
        "MissionCreated" => {
            let mission_slug = reader.require_string("mission_slug")?;
            let mission_number = reader.optional_int("mission_number", 1)?;
            let mission_type = reader.require_string("mission_type")?;
            let target_branch = reader.require_string("target_branch")?;
            let wp_count = reader.require_int("wp_count", 0)?;
            let friendly_name = reader.require_string("friendly_name")?;
            let purpose_tldr = reader.require_string("purpose_tldr")?;
            let purpose_context = reader.require_string("purpose_context")?;
            let mission_id = reader.optional_string("mission_id", true)?;
            let actor = reader.optional_string("actor", true)?;
            let created_at = reader.optional_string("created_at", true)?;
            reader.finish(&[])?;
            let mut object = Map::new();
            object.insert("mission_slug".to_owned(), json!(mission_slug));
            // `mission_number` is the one field `apply_keep_none_fields` keeps
            // as an explicit null (the wire distinguishes pre-merge nullity
            // from field absence).
            object.insert("mission_number".to_owned(), json!(mission_number));
            object.insert("mission_type".to_owned(), json!(mission_type));
            object.insert("target_branch".to_owned(), json!(target_branch));
            object.insert("wp_count".to_owned(), json!(wp_count));
            object.insert("friendly_name".to_owned(), json!(friendly_name));
            object.insert("purpose_tldr".to_owned(), json!(purpose_tldr));
            object.insert("purpose_context".to_owned(), json!(purpose_context));
            if let Some(value) = mission_id {
                object.insert("mission_id".to_owned(), json!(value));
            }
            if let Some(value) = actor {
                object.insert("actor".to_owned(), json!(value));
            }
            if let Some(value) = created_at {
                object.insert("created_at".to_owned(), json!(value));
            }
            Value::Object(object)
        }
        "WPCreated" => {
            let mission_slug = reader.require_string("mission_slug")?;
            let mission_number = reader.optional_int("mission_number", 1)?;
            let wp_id = reader.require_string("wp_id")?;
            let wp_title = reader.require_string("wp_title")?;
            let wp_path = reader.optional_string("wp_path", true)?;
            let depends_on = reader.string_array("depends_on")?;
            let actor = reader.require_string("actor")?;
            let created_at = reader.optional_datetime("created_at")?;
            reader.finish(&[])?;
            json!({
                "mission_slug": mission_slug,
                "mission_number": mission_number,
                "wp_id": wp_id,
                "wp_title": wp_title,
                "wp_path": wp_path,
                "depends_on": depends_on,
                "actor": actor,
                "created_at": created_at,
            })
        }
        "ReviewerSelfApproval" => {
            let mission_slug = reader.require_string("mission_slug")?;
            let wp_id = reader.require_string("wp_id")?;
            let implementing_actor = reader.require_string("implementing_actor")?;
            let intended_reviewer = reader.require_string("intended_reviewer")?;
            let failure_reason = reader.require_string("failure_reason")?;
            let fallback_approved = reader.require_bool("fallback_approved")?;
            let timestamp = reader.require_string("timestamp")?;
            reader.finish(&[])?;
            json!({
                "mission_slug": mission_slug,
                "wp_id": wp_id,
                "implementing_actor": implementing_actor,
                "intended_reviewer": intended_reviewer,
                "failure_reason": failure_reason,
                "fallback_approved": fallback_approved,
                "timestamp": timestamp,
            })
        }
        "MissionReopened" => {
            let mission_id = reader.require_string("mission_id")?;
            let mission_slug = reader.require_string("mission_slug")?;
            let reason = reader.require_string("reason")?;
            let reopened_by = reader.require_string("reopened_by")?;
            let reopened_at = reader.require_string("reopened_at")?;
            let cleared_merge = reader.optional_object("cleared_merge")?;
            reader.finish(&[])?;
            json!({
                "mission_id": mission_id,
                "mission_slug": mission_slug,
                "reason": reason,
                "reopened_by": reopened_by,
                "reopened_at": reopened_at,
                "cleared_merge": cleared_merge,
            })
        }
        "FollowUpRecorded" => {
            let mission_id = reader.require_string("mission_id")?;
            let mission_slug = reader.require_string("mission_slug")?;
            let follow_up_type = reader.require_string("follow_up_type")?;
            let commit_sha = reader.optional_string("commit_sha", true)?;
            let pr_number = reader.optional_int("pr_number", 1)?;
            let recorded_by = reader.require_string("recorded_by")?;
            let recorded_at = reader.require_string("recorded_at")?;
            reader.finish(&[])?;
            match follow_up_type.as_str() {
                "commit" if commit_sha.is_none() => {
                    return Err(invalid(
                        event_type,
                        "commit_sha is required when follow_up_type == 'commit'",
                    ));
                }
                "pr" if pr_number.is_none() => {
                    return Err(invalid(
                        event_type,
                        "pr_number is required when follow_up_type == 'pr'",
                    ));
                }
                "commit" | "pr" => {}
                other => {
                    return Err(invalid(
                        event_type,
                        format!("follow_up_type must be 'commit' or 'pr', got {other:?}"),
                    ));
                }
            }
            json!({
                "mission_id": mission_id,
                "mission_slug": mission_slug,
                "follow_up_type": follow_up_type,
                "commit_sha": commit_sha,
                "pr_number": pr_number,
                "recorded_by": recorded_by,
                "recorded_at": recorded_at,
            })
        }
        other => {
            if let Some((prefix, _phase)) = ARTIFACT_PHASES
                .iter()
                .find(|(prefix, _)| other.starts_with(prefix))
            {
                let completed = other.strip_suffix("Completed");
                let started = other.strip_suffix("Started");
                if completed.is_none() && started.is_none() {
                    return Err(invalid(
                        event_type,
                        format!("{other} is not a canonical artifact phase event"),
                    ));
                }
                let mission_slug = reader.require_string("mission_slug")?;
                let mission_number = reader.optional_int("mission_number", 1)?;
                let actor = reader.require_string("actor")?;
                let at = reader.optional_datetime("at")?;
                let mut object = Map::new();
                object.insert("mission_slug".to_owned(), json!(mission_slug));
                object.insert("mission_number".to_owned(), json!(mission_number));
                object.insert("actor".to_owned(), json!(actor));
                object.insert("at".to_owned(), json!(at));
                if completed.is_some() {
                    let artifact_path = reader.require_string("artifact_path")?;
                    let summary = reader.optional_string("summary", false)?;
                    object.insert("artifact_path".to_owned(), json!(artifact_path));
                    object.insert("summary".to_owned(), json!(summary));
                    if *prefix == "Tasks" {
                        let wp_count = reader.require_int("wp_count", 0)?;
                        object.insert("wp_count".to_owned(), json!(wp_count));
                    }
                    reader.finish(&[])?;
                } else {
                    // Local-log-only Started metadata: the canonical model does
                    // not declare it, and `emit_artifact_phase_local` appends it
                    // after validation. It is the sole allowed extra key.
                    let artifact_path = reader.optional_string("artifact_path", true)?;
                    if let Some(value) = artifact_path {
                        object.insert("artifact_path".to_owned(), json!(value));
                    }
                    reader.finish(&["artifact_path"])?;
                }
                Value::Object(object)
            } else {
                return Err(MutationError::UnknownLifecycleType {
                    event_type: other.to_owned(),
                });
            }
        }
    };
    Ok(canonical)
}

/// Normalise a pydantic `datetime` string to its `model_dump(mode="json")` shape.
fn normalize_datetime(event_type: &str, key: &str, text: &str) -> Result<String, MutationError> {
    // pydantic accepts a bare numeric epoch too (`"20261102"` is an int
    // timestamp). Without this branch the value would be refused outright.
    if is_numeric_epoch(text) {
        let seconds: f64 = text
            .parse()
            .map_err(|_| invalid(event_type, format!("{key} is not a number: {text:?}")))?;
        let instant = sk_kernel::clock::from_epoch(seconds).map_err(|error| {
            invalid(
                event_type,
                format!("{key} {text:?} is not a valid epoch: {error}"),
            )
        })?;
        return Ok(render_datetime(instant));
    }
    let instant = sk_kernel::clock::parse_iso(text).map_err(|error| {
        invalid(
            event_type,
            format!("{key} must be an ISO-8601 timestamp, got {text:?}: {error}"),
        )
    })?;
    Ok(render_datetime(instant))
}

fn is_numeric_epoch(text: &str) -> bool {
    let trimmed = text.trim();
    let unsigned = trimmed.strip_prefix(['+', '-']).unwrap_or(trimmed);
    !unsigned.is_empty()
        && unsigned
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.')
        && unsigned.bytes().filter(|byte| *byte == b'.').count() <= 1
        && !unsigned.starts_with('.')
        && !unsigned.ends_with('.')
}

fn render_datetime(instant: sk_kernel::clock::UtcDateTime<sk_kernel::clock::UtcZone>) -> String {
    let mut rendered = instant.format("%Y-%m-%dT%H:%M:%S").to_string();
    let micros = instant.timestamp_subsec_micros();
    if micros != 0 {
        rendered.push_str(&format!(".{micros:06}"));
    }
    rendered.push('Z');
    rendered
}

/// The canonical payload key set per event type (for diagnostics and tests).
pub fn lifecycle_payload_keys(event_type: &str) -> Option<&'static [&'static str]> {
    match event_type {
        "ProjectInitialized" => Some(&[
            "project_uuid",
            "project_slug",
            "actor",
            "runtime_version",
            "initialized_at",
        ]),
        "MissionCreated" => Some(&[
            "mission_slug",
            "mission_number",
            "mission_type",
            "target_branch",
            "wp_count",
            "friendly_name",
            "purpose_tldr",
            "purpose_context",
            "mission_id",
            "actor",
            "created_at",
        ]),
        "WPCreated" => Some(&[
            "mission_slug",
            "mission_number",
            "wp_id",
            "wp_title",
            "wp_path",
            "depends_on",
            "actor",
            "created_at",
        ]),
        "ReviewerSelfApproval" => Some(&[
            "mission_slug",
            "wp_id",
            "implementing_actor",
            "intended_reviewer",
            "failure_reason",
            "fallback_approved",
            "timestamp",
        ]),
        "MissionReopened" => Some(&[
            "mission_id",
            "mission_slug",
            "reason",
            "reopened_by",
            "reopened_at",
            "cleared_merge",
        ]),
        "FollowUpRecorded" => Some(&[
            "mission_id",
            "mission_slug",
            "follow_up_type",
            "commit_sha",
            "pr_number",
            "recorded_by",
            "recorded_at",
        ]),
        "SpecifyStarted" | "PlanStarted" | "TasksStarted" => Some(&[
            "mission_slug",
            "mission_number",
            "actor",
            "at",
            "artifact_path",
        ]),
        "SpecifyCompleted" | "PlanCompleted" => Some(&[
            "mission_slug",
            "mission_number",
            "actor",
            "at",
            "artifact_path",
            "summary",
        ]),
        "TasksCompleted" => Some(&[
            "mission_slug",
            "mission_number",
            "actor",
            "at",
            "artifact_path",
            "summary",
            "wp_count",
        ]),
        _ => None,
    }
}

/// `bool(value)`-compatible truthiness, re-exported for payload callers.
pub use crate::mutation_delta::python_bool as payload_python_bool;
/// `int(value)`-compatible coercion, re-exported for payload callers.
pub use crate::mutation_delta::python_int as payload_python_int;

#[cfg(test)]
mod tests {
    use super::*;

    fn project_payload() -> Value {
        json!({
            "project_uuid": "1f8bca14-6f1b-4a1c-9a6c-1d6f0f0b2c3d",
            "project_slug": "demo",
            "actor": "cli",
            "runtime_version": "4.1.0",
            "initialized_at": "2026-11-02T09:00:00+00:00",
        })
    }

    #[test]
    fn project_payload_normalises_utc_and_keeps_null_optionals() {
        let canonical = canonicalize_lifecycle_payload("ProjectInitialized", &project_payload())
            .expect("canonical");
        assert_eq!(canonical["initialized_at"], json!("2026-11-02T09:00:00Z"));

        let nulls = canonicalize_lifecycle_payload(
            "ProjectInitialized",
            &json!({"project_uuid": "u", "actor": "a"}),
        )
        .unwrap();
        assert_eq!(
            nulls,
            json!({
                "project_uuid": "u",
                "project_slug": null,
                "actor": "a",
                "runtime_version": null,
                "initialized_at": null,
            })
        );
    }

    #[test]
    fn project_payload_refuses_extras_and_empty_required_fields() {
        let mut extra = project_payload();
        extra["hostname"] = json!("builder");
        assert!(canonicalize_lifecycle_payload("ProjectInitialized", &extra).is_err());

        assert!(
            canonicalize_lifecycle_payload(
                "ProjectInitialized",
                &json!({"project_uuid": "", "actor": "a"})
            )
            .is_err()
        );
        assert!(
            canonicalize_lifecycle_payload(
                "ProjectInitialized",
                &json!({"project_uuid": "u", "actor": "a", "project_slug": ""})
            )
            .is_err()
        );
    }

    #[test]
    fn mission_created_keeps_only_the_null_mission_number() {
        let canonical = canonicalize_lifecycle_payload(
            "MissionCreated",
            &json!({
                "mission_slug": "001-demo",
                "mission_number": null,
                "mission_type": "software-dev",
                "target_branch": "main",
                "wp_count": 3,
                "friendly_name": "Demo",
                "purpose_tldr": "Demo",
                "purpose_context": "Context",
                "mission_id": null,
                "actor": null,
                "created_at": "2026-11-02T09:00:00Z",
            }),
        )
        .unwrap();
        assert_eq!(canonical["mission_number"], Value::Null);
        assert!(canonical.get("mission_id").is_none());
        assert!(canonical.get("actor").is_none());
        assert_eq!(canonical["created_at"], json!("2026-11-02T09:00:00Z"));
    }

    #[test]
    fn artifact_started_allows_only_the_local_artifact_path_extra() {
        let canonical = canonicalize_lifecycle_payload(
            "SpecifyStarted",
            &json!({
                "mission_slug": "001-demo",
                "mission_number": null,
                "actor": "cli",
                "at": "2026-11-02T09:00:00Z",
                "artifact_path": "kitty-specs/001-demo/spec.md",
            }),
        )
        .unwrap();
        assert_eq!(
            canonical["artifact_path"],
            json!("kitty-specs/001-demo/spec.md")
        );

        assert!(
            canonicalize_lifecycle_payload(
                "SpecifyStarted",
                &json!({
                    "mission_slug": "001-demo",
                    "actor": "cli",
                    "summary": "not allowed on Started",
                })
            )
            .is_err()
        );
    }

    #[test]
    fn tasks_completed_requires_wp_count_and_bounds_it() {
        let canonical = canonicalize_lifecycle_payload(
            "TasksCompleted",
            &json!({
                "mission_slug": "001-demo",
                "mission_number": 4,
                "actor": "cli",
                "at": "2026-11-02",
                "artifact_path": "tasks.md",
                "wp_count": 0,
            }),
        )
        .unwrap();
        assert_eq!(canonical["at"], json!("2026-11-02T00:00:00Z"));
        assert_eq!(canonical["summary"], Value::Null);
        assert_eq!(canonical["wp_count"], json!(0));

        assert!(
            canonicalize_lifecycle_payload(
                "TasksCompleted",
                &json!({
                    "mission_slug": "m",
                    "actor": "a",
                    "artifact_path": "tasks.md",
                    "wp_count": -1,
                })
            )
            .is_err()
        );
    }

    #[test]
    fn follow_up_requires_the_matching_reference() {
        let base = json!({
            "mission_id": "01K",
            "mission_slug": "001-demo",
            "follow_up_type": "commit",
            "commit_sha": "abc123",
            "pr_number": null,
            "recorded_by": "alice",
            "recorded_at": "2026-11-02T09:00:00Z",
        });
        assert!(canonicalize_lifecycle_payload("FollowUpRecorded", &base).is_ok());

        let mut missing = base.clone();
        missing["commit_sha"] = Value::Null;
        assert!(canonicalize_lifecycle_payload("FollowUpRecorded", &missing).is_err());

        let mut wrong = base;
        wrong["follow_up_type"] = json!("issue");
        assert!(canonicalize_lifecycle_payload("FollowUpRecorded", &wrong).is_err());
    }

    #[test]
    fn unknown_event_type_is_a_typed_refusal_at_this_seam() {
        let error = canonicalize_lifecycle_payload("NotAThing", &json!({})).unwrap_err();
        assert!(matches!(error, MutationError::UnknownLifecycleType { .. }));
    }

    #[test]
    fn numeric_epochs_match_pydantic() {
        // pydantic treats an all-digit string as an epoch timestamp.
        let canonical = canonicalize_lifecycle_payload(
            "ProjectInitialized",
            &json!({"project_uuid": "u", "actor": "a", "initialized_at": "20261102"}),
        )
        .unwrap();
        assert_eq!(canonical["initialized_at"], json!("1970-08-23T12:05:02Z"));
    }

    #[test]
    fn python_truthiness_and_int_helpers_are_exposed() {
        assert!(payload_python_bool(&json!("false")));
        assert_eq!(payload_python_int(&json!(" 7 ")), Some(7));
    }
}
