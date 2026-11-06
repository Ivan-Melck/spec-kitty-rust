//! Local-first canonical lifecycle event persistence.
//!
//! Native counterpart of `specify_cli.status.lifecycle_events`. Two log targets
//! exist and both are written through the same durable batch primitive:
//!
//! - the **project** log `<repo-root>/.kittify/canonical-events.jsonl`
//!   (`ProjectInitialized`), locked by the `__project__` status lock;
//! - the **mission** log `<feature-dir>/status.events.jsonl` (mission creation,
//!   artifact phase, `WPCreated`, retrospective and decision rows), locked by
//!   the Mission status lock — the same lock the lane-transition writer takes,
//!   which is what closes the lost-write race between the two writers.
//!
//! Preserved baseline semantics:
//!
//! - the nine-key envelope `{event_id, event_type, aggregate_id,
//!   aggregate_type, schema_version: "5.0.0", timestamp, payload,
//!   project_uuid, project_slug}`, serialized with sorted keys and Python's
//!   separators;
//! - idempotence keyed on `(event_type, dedup tuple)`: a re-run that finds a
//!   matching row appends nothing (`artifact_path` compares by file name, so a
//!   relocated artifact does not duplicate its event);
//! - an unknown `event_type` is *skipped*, never written (the baseline's
//!   `LIFECYCLE_EVENT_TYPES` closed set);
//! - local persistence never fan-outs as a side effect here: `append_lifecycle_event`
//!   exists for source-compatibility and documents that the transport seam is a
//!   separate owner, exactly as open-core registers no lifecycle handler.
//!
//! Payloads are validated against the pinned `spec-kitty-events` 11.3.0 models
//! (and the core-local event shapes) by
//! [`crate::mutation_payloads::canonicalize_lifecycle_payload`] before the
//! envelope is built, and are re-encoded in the models' canonical wire shape.
//! `ProjectInitialized` additionally has a built-in builder.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use sk_kernel::locks::LockWait;

use crate::mutation::MutationError;
use crate::mutation_lock::{acquire_mission_lock, acquire_project_lock, canonical_repo_root};
use crate::mutation_log::{AppendOptions, Durability, append_rows_atomic, read_log_text};
use crate::mutation_payloads::canonicalize_lifecycle_payload;
use crate::mutation_ulid::new_ulid;

/// Project-level canonical event log filename.
pub const PROJECT_EVENTS_FILENAME: &str = "canonical-events.jsonl";

/// Mission-level event log filename (shared with lane transitions).
pub const MISSION_EVENTS_FILENAME: &str = "status.events.jsonl";

/// Envelope schema version written by the local lifecycle appenders.
pub const LIFECYCLE_ENVELOPE_SCHEMA_VERSION: &str = "5.0.0";

/// The closed set of canonical lifecycle event types.
pub const LIFECYCLE_EVENT_TYPES: [&str; 12] = [
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

/// Return the project-level canonical event log path for `repo_root`.
pub fn project_event_log_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".kittify").join(PROJECT_EVENTS_FILENAME)
}

/// Return the mission-level event log path for `feature_dir`.
pub fn mission_event_log_path(feature_dir: &Path) -> PathBuf {
    feature_dir.join(MISSION_EVENTS_FILENAME)
}

/// One lifecycle append request.
#[derive(Clone, Debug)]
pub struct LifecycleRequest {
    /// The log to write (`canonical-events.jsonl` or `status.events.jsonl`).
    pub log_path: PathBuf,
    /// One of [`LIFECYCLE_EVENT_TYPES`].
    pub event_type: String,
    /// The event payload object.
    pub payload: Value,
    pub aggregate_id: String,
    pub aggregate_type: String,
    pub project_uuid: Option<String>,
    pub project_slug: Option<String>,
    /// Fields whose payload values identify an already-recorded event.
    pub dedup_keys: BTreeMap<String, Value>,
    /// Present for a mission-scoped write: routes the Mission lock family.
    pub mission_slug: Option<String>,
    /// Lock root override; `None` resolves it from the log path.
    pub repo_root: Option<PathBuf>,
    /// Explicit event id (fixtures pin this).
    pub event_id: Option<String>,
    /// Explicit envelope timestamp (fixtures pin this).
    pub timestamp: Option<String>,
    pub lock_wait: Option<LockWait>,
}

impl LifecycleRequest {
    /// A request with no dedupe keys and no explicit identity.
    pub fn new(
        log_path: impl Into<PathBuf>,
        event_type: impl Into<String>,
        payload: Value,
        aggregate_id: impl Into<String>,
        aggregate_type: impl Into<String>,
    ) -> Self {
        Self {
            log_path: log_path.into(),
            event_type: event_type.into(),
            payload,
            aggregate_id: aggregate_id.into(),
            aggregate_type: aggregate_type.into(),
            project_uuid: None,
            project_slug: None,
            dedup_keys: BTreeMap::new(),
            mission_slug: None,
            repo_root: None,
            event_id: None,
            timestamp: None,
            lock_wait: None,
        }
    }

    fn wait(&self) -> LockWait {
        // `_lifecycle_write_lock` takes `feature_status_lock` /
        // `project_event_log_lock` with their `UNBOUNDED_LOCK_WAIT` defaults.
        self.lock_wait.unwrap_or(LockWait::Unbounded)
    }
}

/// Why a lifecycle append wrote nothing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecycleSkip {
    /// The event type is outside the closed set.
    UnknownType,
    /// A row with the same `(event_type, dedup tuple)` is already present.
    AlreadyPresent,
}

/// The outcome of a lifecycle append.
#[derive(Clone, Debug)]
pub struct LifecycleAppend {
    /// The envelope that was written, when one was.
    pub envelope: Option<Value>,
    /// The log path.
    pub path: PathBuf,
    /// The lock file, when a lock root resolved.
    pub lock_path: Option<PathBuf>,
    /// Set when the append was skipped.
    pub skip: Option<LifecycleSkip>,
    /// Durability of the replacement (meaningful only when `envelope` is set).
    pub durability: Durability,
    /// Rows appended (0 when skipped, 1 otherwise).
    pub rows_appended: usize,
}

/// True when the log already holds a matching lifecycle event.
pub fn has_lifecycle_event(
    log_path: &Path,
    event_type: &str,
    dedup_keys: &BTreeMap<String, Value>,
) -> Result<bool, MutationError> {
    let Some(text) = read_log_text(log_path)? else {
        return Ok(false);
    };
    let rows = read_lifecycle_rows(log_path, &text)?;
    Ok(rows
        .iter()
        .any(|row| matches_lifecycle_event(row, event_type, dedup_keys)))
}

/// Read every lifecycle-shaped row (a dict with a string `event_type`).
///
/// Malformed JSON lines are skipped, matching the baseline's tolerant
/// `_read_lifecycle_lines`; the durable writer's own strict parse runs on the
/// write path.
pub fn read_lifecycle_events(log_path: &Path) -> Result<Vec<Value>, MutationError> {
    let Some(text) = read_log_text(log_path)? else {
        return Ok(Vec::new());
    };
    read_lifecycle_rows(log_path, &text)
}

fn read_lifecycle_rows(log_path: &Path, text: &str) -> Result<Vec<Value>, MutationError> {
    // The durable writer refuses a corrupt stream, so a read that will be
    // followed by a write must fail closed too; tolerant scanning is only used
    // for the read-only accessor, which is the same function. A caller that
    // wants tolerance gets it: invalid lines are skipped, and the *write* path
    // re-reads strictly through `append_rows_atomic`.
    let _ = log_path;
    Ok(text
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }
            let value: Value = serde_json::from_str(trimmed).ok()?;
            value.get("event_type").and_then(Value::as_str)?;
            Some(value)
        })
        .collect())
}

fn matches_lifecycle_event(
    row: &Value,
    event_type: &str,
    dedup_keys: &BTreeMap<String, Value>,
) -> bool {
    if row.get("event_type").and_then(Value::as_str) != Some(event_type) {
        return false;
    }
    let Some(payload) = row.get("payload").and_then(Value::as_object) else {
        return false;
    };
    dedup_keys.iter().all(|(key, expected)| {
        let actual = payload.get(key);
        if actual == Some(expected) {
            return true;
        }
        if key == "artifact_path"
            && let (Some(Value::String(actual)), Some(Value::String(expected))) =
                (actual, Some(expected))
        {
            return Path::new(actual).file_name() == Path::new(expected).file_name();
        }
        false
    })
}

/// Persist a lifecycle event locally, skipping an already-recorded one.
///
/// This is the durable half of the baseline `append_lifecycle_event`: no
/// transport handler is invoked (open-core registers none).
pub fn persist_lifecycle_event_local(
    request: &LifecycleRequest,
) -> Result<LifecycleAppend, MutationError> {
    if !LIFECYCLE_EVENT_TYPES.contains(&request.event_type.as_str()) {
        return Ok(LifecycleAppend {
            envelope: None,
            path: request.log_path.clone(),
            lock_path: None,
            skip: Some(LifecycleSkip::UnknownType),
            durability: Durability::Confirmed,
            rows_appended: 0,
        });
    }
    // The baseline convenience emitters validate through the pinned pydantic
    // payload models before the envelope is built; the payload-agnostic
    // persistence seam itself does not. Validate here so no caller can persist
    // arbitrary JSON as a canonical lifecycle fact, and re-encode the accepted
    // payload in the model's canonical wire shape.
    let payload = canonicalize_lifecycle_payload(&request.event_type, &request.payload)?;

    let repo_root = match &request.repo_root {
        Some(root) => Some(root.clone()),
        None => repo_root_for_log(&request.log_path),
    };

    let lock = match (&repo_root, &request.mission_slug) {
        (Some(root), Some(_)) => {
            let feature_dir = request
                .log_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| request.log_path.clone());
            Some(LockGuard::Mission(acquire_mission_lock(
                &feature_dir,
                Some(root),
                request.wait(),
            )?))
        }
        (Some(root), None) => Some(LockGuard::Project(acquire_project_lock(
            root,
            request.wait(),
        )?)),
        (None, _) => None,
    };

    // Dedupe and the authoritative read happen under the lock, so two
    // concurrent producers cannot both append the same fact.
    if !request.dedup_keys.is_empty()
        && has_lifecycle_event(&request.log_path, &request.event_type, &request.dedup_keys)?
    {
        return Ok(LifecycleAppend {
            envelope: None,
            path: request.log_path.clone(),
            lock_path: lock.as_ref().map(|guard| guard.path().to_path_buf()),
            skip: Some(LifecycleSkip::AlreadyPresent),
            durability: Durability::Confirmed,
            rows_appended: 0,
        });
    }

    let envelope = build_envelope(request, payload);
    let outcome = append_rows_atomic(
        &request.log_path,
        std::slice::from_ref(&envelope),
        &AppendOptions::default(),
    )?;
    Ok(LifecycleAppend {
        envelope: Some(envelope),
        path: request.log_path.clone(),
        lock_path: lock.as_ref().map(|guard| guard.path().to_path_buf()),
        skip: None,
        durability: outcome.durability,
        rows_appended: outcome.rows_appended,
    })
}

/// Source-compatible alias of [`persist_lifecycle_event_local`].
///
/// The baseline name implied "persist then fan out"; open-core registers no
/// lifecycle transport handler, so the composed and local forms are identical.
/// A future transport owner wires fan-out after this call returns.
pub fn append_lifecycle_event(
    request: &LifecycleRequest,
) -> Result<LifecycleAppend, MutationError> {
    persist_lifecycle_event_local(request)
}

enum LockGuard {
    Mission(crate::mutation_lock::MissionLock),
    Project(crate::mutation_lock::ProjectLock),
}

impl LockGuard {
    fn path(&self) -> &Path {
        match self {
            LockGuard::Mission(guard) => guard.path(),
            LockGuard::Project(guard) => guard.path(),
        }
    }
}

fn repo_root_for_log(log_path: &Path) -> Option<PathBuf> {
    let parent = log_path.parent()?;
    canonical_repo_root(parent).ok()
}

fn build_envelope(request: &LifecycleRequest, payload: Value) -> Value {
    let mut envelope = Map::new();
    envelope.insert(
        "event_id".to_owned(),
        Value::String(match &request.event_id {
            Some(value) => value.clone(),
            None => new_ulid(),
        }),
    );
    envelope.insert(
        "event_type".to_owned(),
        Value::String(request.event_type.clone()),
    );
    envelope.insert(
        "aggregate_id".to_owned(),
        Value::String(request.aggregate_id.clone()),
    );
    envelope.insert(
        "aggregate_type".to_owned(),
        Value::String(request.aggregate_type.clone()),
    );
    envelope.insert(
        "schema_version".to_owned(),
        Value::String(LIFECYCLE_ENVELOPE_SCHEMA_VERSION.to_owned()),
    );
    envelope.insert(
        "timestamp".to_owned(),
        Value::String(
            request
                .timestamp
                .clone()
                .unwrap_or_else(sk_kernel::clock::now_utc_iso),
        ),
    );
    envelope.insert("payload".to_owned(), payload);
    envelope.insert(
        "project_uuid".to_owned(),
        request
            .project_uuid
            .clone()
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    envelope.insert(
        "project_slug".to_owned(),
        request
            .project_slug
            .clone()
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    Value::Object(envelope)
}

/// Inputs for [`emit_project_initialized`].
#[derive(Clone, Debug)]
pub struct ProjectInitializedRequest<'a> {
    pub project_uuid: &'a str,
    pub project_slug: Option<&'a str>,
    pub actor: &'a str,
    pub runtime_version: Option<&'a str>,
    /// ISO-8601 timestamp; `None` stamps now. The canonical payload model
    /// normalises a datetime to `...Z`, while this builder passes the caller's
    /// string through (a recorded payload-model gap).
    pub initialized_at: Option<&'a str>,
    pub event_id: Option<&'a str>,
    pub timestamp: Option<&'a str>,
}

/// Record the local `ProjectInitialized` event for `repo_root`.
///
/// Idempotent on `project_uuid`. The payload is the canonical
/// `ProjectInitializedPayload` field set
/// (`project_uuid, project_slug, actor, runtime_version, initialized_at`).
pub fn emit_project_initialized(
    repo_root: &Path,
    input: ProjectInitializedRequest<'_>,
) -> Result<LifecycleAppend, MutationError> {
    let payload = json!({
        "project_uuid": input.project_uuid,
        "project_slug": input.project_slug,
        "actor": input.actor,
        "runtime_version": input.runtime_version,
        "initialized_at": input
            .initialized_at
            .map(str::to_owned)
            .unwrap_or_else(sk_kernel::clock::now_utc_iso),
    });
    let mut request = LifecycleRequest::new(
        project_event_log_path(repo_root),
        "ProjectInitialized",
        payload,
        input.project_uuid,
        "Project",
    );
    request.project_uuid = Some(input.project_uuid.to_owned());
    request.project_slug = input.project_slug.map(str::to_owned);
    request.dedup_keys = BTreeMap::from([(
        "project_uuid".to_owned(),
        Value::String(input.project_uuid.to_owned()),
    )]);
    request.repo_root = Some(repo_root.to_path_buf());
    request.event_id = input.event_id.map(str::to_owned);
    request.timestamp = input.timestamp.map(str::to_owned);
    persist_lifecycle_event_local(&request)
}

/// The envelope keys the baseline writes, in sorted order (one authority).
pub const LIFECYCLE_ENVELOPE_KEYS: [&str; 9] = [
    "aggregate_id",
    "aggregate_type",
    "event_id",
    "event_type",
    "payload",
    "project_slug",
    "project_uuid",
    "schema_version",
    "timestamp",
];

#[cfg(test)]
mod wait_policy_tests {
    use super::*;
    use sk_kernel::locks::LockWait;

    #[test]
    fn the_lifecycle_door_defaults_to_the_baselines_unbounded_wait() {
        // `_lifecycle_write_lock` uses `feature_status_lock` /
        // `project_event_log_lock` with their `UNBOUNDED_LOCK_WAIT` defaults.
        let request = LifecycleRequest::new(
            "/tmp/log.jsonl",
            "WPCreated",
            Value::Null,
            "WP01",
            "WorkPackage",
        );
        assert_eq!(request.wait(), LockWait::Unbounded);
    }
}
