//! The local `ProjectInitialized` lifecycle event.
//!
//! `init` records one local canonical event at
//! `.kittify/canonical-events.jsonl` before any fan-out, so a dashboard reading
//! the project sees a complete history. The wire shape is the pinned contract:
//!
//! ```json
//! {"aggregate_id":"<uuid>","aggregate_type":"Project","event_id":"<ulid>",
//!  "event_type":"ProjectInitialized","payload":{...},"project_slug":"<slug>",
//!  "project_uuid":"<uuid>","schema_version":"5.0.0","timestamp":"<iso>"}
//! ```
//!
//! The payload carries `actor`, `initialized_at`, `project_slug`,
//! `project_uuid` and `runtime_version`, all keys present (null when absent),
//! matching `ProjectInitializedPayload.model_dump(mode="json",
//! exclude_none=False)`.
//!
//! **Write authority.** The write itself is delegated to the canonical
//! `sk-status` project-lifecycle writer
//! ([`sk_status::mutation_lifecycle::persist_lifecycle_event_local`]): the same
//! durable whole-file replacement, the same Python-compatible serialization and
//! the same `project.status.lock` (Git common dir anchored, skipped outside a
//! repository) that every other canonical event writer uses. This module is a
//! thin typed adapter over that authority, not a second writer.
//!
//! [`plan_project_initialized`] remains a read-only preview (validated bytes and
//! a precondition hash) for callers that need to inspect the append before it
//! happens. Idempotency is on `project_uuid`, so a repeated init appends
//! nothing.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::error::InstallError;
use crate::lock::ProjectLock;
use crate::plan::now_utc_iso;

/// Project-relative path of the canonical project event log.
pub const PROJECT_EVENTS_REL: &str = ".kittify/canonical-events.jsonl";

/// The lifecycle event type recorded at init.
pub const PROJECT_INITIALIZED: &str = "ProjectInitialized";

/// The canonical envelope schema version.
pub const LIFECYCLE_SCHEMA_VERSION: &str = "5.0.0";

/// The largest canonical event log this writer will read.
pub const MAX_EVENT_LOG_BYTES: u64 = 16 * 1024 * 1024;

/// The `ProjectInitialized` payload (all fields serialized, null included).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProjectInitializedPayload {
    /// The project UUID.
    pub project_uuid: String,
    /// The project slug, when known.
    pub project_slug: Option<String>,
    /// The actor that initialized the project.
    pub actor: String,
    /// The CLI version that emitted the event, when known.
    pub runtime_version: Option<String>,
    /// When the project was initialized (aware UTC ISO-8601).
    pub initialized_at: String,
}

impl ProjectInitializedPayload {
    /// Build the canonical payload for an init run.
    pub fn new(
        project_uuid: impl Into<String>,
        project_slug: Option<String>,
        actor: impl Into<String>,
        runtime_version: Option<String>,
        initialized_at: impl Into<String>,
    ) -> Self {
        Self {
            project_uuid: project_uuid.into(),
            project_slug,
            actor: actor.into(),
            runtime_version,
            initialized_at: initialized_at.into(),
        }
    }
}

/// A validated, not-yet-committed whole-file append.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectEventAppendPlan {
    project_root: PathBuf,
    path: PathBuf,
    existing: Vec<u8>,
    line: Vec<u8>,
    duplicate: bool,
}

impl ProjectEventAppendPlan {
    /// The log path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether an equivalent `ProjectInitialized` event is already recorded.
    pub fn is_duplicate(&self) -> bool {
        self.duplicate
    }

    /// The exact bytes the commit will write (`existing` when duplicate).
    pub fn desired_bytes(&self) -> Vec<u8> {
        if self.duplicate {
            return self.existing.clone();
        }
        let mut bytes = self.existing.clone();
        bytes.extend_from_slice(&self.line);
        bytes
    }

    /// The SHA-256 of the observed log bytes (the commit precondition).
    pub fn observed_sha256(&self) -> String {
        hex(&Sha256::digest(&self.existing))
    }

    /// Commit the append under the canonical project lock, re-checking the
    /// precondition.
    ///
    /// The lock is the same machine-state-directory authority init and upgrade
    /// take ([`ProjectLock::for_project`]); a project-relative
    /// `.kittify/.install.lock` here would be a second, uncoordinated writer.
    pub fn commit(&self, lock_wait: Duration) -> Result<Option<ProjectEventLine>, InstallError> {
        if self.duplicate {
            return Ok(None);
        }
        let _lock = ProjectLock::for_project(&self.project_root, lock_wait)?;
        let current = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(source) => return Err(InstallError::io("read", &self.path, source)),
        };
        if current != self.existing {
            return Err(InstallError::LifecycleEventInvalid {
                path: self.path.clone(),
                detail: "event log changed between plan and commit".to_string(),
            });
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|source| InstallError::io("create directory", parent, source))?;
        }
        let desired = self.desired_bytes();
        match sk_kernel::atomic_write(&self.path, &desired) {
            Ok(_) => Ok(Some(ProjectEventLine {
                path: self.path.clone(),
                bytes: desired,
            })),
            Err(error) if error.mutation_may_have_committed() => {
                // The replacement may be visible. Read back instead of retrying.
                let visible = std::fs::read(&self.path).unwrap_or_default();
                if visible == desired {
                    Ok(Some(ProjectEventLine {
                        path: self.path.clone(),
                        bytes: desired,
                    }))
                } else {
                    Err(InstallError::IndeterminateCommit {
                        path: self.path.clone(),
                        detail: error.to_string(),
                    })
                }
            }
            Err(error) => Err(InstallError::io(
                "append lifecycle event",
                &self.path,
                std::io::Error::other(error),
            )),
        }
    }
}

/// A committed append (the whole resulting file, for reporting).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectEventLine {
    /// The log path.
    pub path: PathBuf,
    /// The complete file after the append.
    pub bytes: Vec<u8>,
}

/// Plan a `ProjectInitialized` append, without writing anything.
pub fn plan_project_initialized(
    project_root: &Path,
    payload: &ProjectInitializedPayload,
) -> Result<ProjectEventAppendPlan, InstallError> {
    let path = project_root.join(PROJECT_EVENTS_REL);
    let existing = match sk_kernel::read_bounded(&path, MAX_EVENT_LOG_BYTES) {
        Ok(bytes) => bytes,
        Err(sk_kernel::KernelError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            Vec::new()
        }
        Err(error) => {
            return Err(InstallError::LifecycleEventInvalid {
                path: path.clone(),
                detail: error.to_string(),
            });
        }
    };
    if !existing.is_empty() && !existing.ends_with(b"\n") {
        return Err(InstallError::LifecycleEventInvalid {
            path: path.clone(),
            detail: "log has a torn final line (no trailing newline); refusing to append"
                .to_string(),
        });
    }
    let duplicate = has_project_initialized(&path, &existing, &payload.project_uuid)?;
    let line = if duplicate {
        Vec::new()
    } else {
        let envelope = envelope(payload)?;
        let mut bytes =
            serde_json::to_vec(&envelope).map_err(|error| InstallError::LifecycleEventInvalid {
                path: path.clone(),
                detail: error.to_string(),
            })?;
        bytes.push(b'\n');
        bytes
    };
    Ok(ProjectEventAppendPlan {
        project_root: project_root.to_path_buf(),
        path,
        existing,
        line,
        duplicate,
    })
}

/// Record a local `ProjectInitialized` event (idempotent on `project_uuid`).
///
/// Delegates to the canonical `sk-status` lifecycle writer so the event log has
/// exactly one write authority and one lock family during the migration period.
pub fn emit_project_initialized(
    project_root: &Path,
    payload: &ProjectInitializedPayload,
    lock_wait: Duration,
) -> Result<Option<ProjectEventLine>, InstallError> {
    let payload_value =
        serde_json::to_value(payload).map_err(|error| InstallError::LifecycleEventInvalid {
            path: PathBuf::from("<payload>"),
            detail: error.to_string(),
        })?;
    let mut request = sk_status::LifecycleRequest::new(
        sk_status::mutation_lifecycle::project_event_log_path(project_root),
        PROJECT_INITIALIZED,
        payload_value,
        payload.project_uuid.clone(),
        "Project",
    );
    request.project_uuid = Some(payload.project_uuid.clone());
    request.project_slug = payload.project_slug.clone();
    request.dedup_keys = std::collections::BTreeMap::from([(
        "project_uuid".to_string(),
        serde_json::Value::String(payload.project_uuid.clone()),
    )]);
    // The baseline resolves the lock from the log path's canonical repo root and
    // skips locking outside a Git repository. Supplying the root is equivalent
    // for a real checkout and must not mint a lock directory in a non-Git tree.
    if project_root.join(".git").exists() {
        request.repo_root = Some(project_root.to_path_buf());
    }
    request.lock_wait = Some(sk_kernel::locks::LockWait::Bounded(lock_wait));
    let outcome = sk_status::mutation_lifecycle::persist_lifecycle_event_local(&request).map_err(
        |error| InstallError::LifecycleEventInvalid {
            path: request.log_path.clone(),
            detail: error.to_string(),
        },
    )?;
    if outcome.envelope.is_none() {
        return Ok(None);
    }
    let bytes = std::fs::read(&outcome.path)
        .map_err(|source| InstallError::io("read", &outcome.path, source))?;
    Ok(Some(ProjectEventLine {
        path: outcome.path,
        bytes,
    }))
}

fn envelope(payload: &ProjectInitializedPayload) -> Result<serde_json::Value, InstallError> {
    let payload =
        serde_json::to_value(payload).map_err(|error| InstallError::LifecycleEventInvalid {
            path: PathBuf::from("<payload>"),
            detail: error.to_string(),
        })?;
    let mut envelope = serde_json::Map::new();
    envelope.insert(
        "event_id".to_string(),
        serde_json::Value::String(ulid::Ulid::new().to_string()),
    );
    envelope.insert(
        "event_type".to_string(),
        serde_json::Value::String(PROJECT_INITIALIZED.to_string()),
    );
    envelope.insert(
        "aggregate_id".to_string(),
        serde_json::Value::String(
            payload["project_uuid"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        ),
    );
    envelope.insert(
        "aggregate_type".to_string(),
        serde_json::Value::String("Project".to_string()),
    );
    envelope.insert(
        "schema_version".to_string(),
        serde_json::Value::String(LIFECYCLE_SCHEMA_VERSION.to_string()),
    );
    envelope.insert(
        "timestamp".to_string(),
        serde_json::Value::String(now_utc_iso()),
    );
    envelope.insert("payload".to_string(), payload);
    envelope.insert(
        "project_uuid".to_string(),
        serde_json::Value::String(
            envelope
                .get("aggregate_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
        ),
    );
    envelope.insert(
        "project_slug".to_string(),
        serde_json::Value::String(
            envelope
                .get("payload")
                .and_then(|payload| payload.get("project_slug"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
        ),
    );
    Ok(serde_json::Value::Object(envelope))
}

fn has_project_initialized(
    path: &Path,
    existing: &[u8],
    project_uuid: &str,
) -> Result<bool, InstallError> {
    for line in existing.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        let parsed: serde_json::Value = match serde_json::from_slice(line) {
            Ok(value) => value,
            // A malformed historical row is skipped, matching the tolerant
            // reader: one bad line must not make init impossible.
            Err(_) => continue,
        };
        if parsed.get("event_type").and_then(serde_json::Value::as_str) != Some(PROJECT_INITIALIZED)
        {
            continue;
        }
        let recorded = parsed
            .get("payload")
            .and_then(|payload| payload.get("project_uuid"))
            .and_then(serde_json::Value::as_str)
            .or_else(|| {
                parsed
                    .get("project_uuid")
                    .and_then(serde_json::Value::as_str)
            });
        if recorded == Some(project_uuid) {
            return Ok(true);
        }
    }
    let _ = path;
    Ok(false)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The canonical payload for an init run, with the wall clock filled in.
pub fn payload_for_init(
    project_uuid: impl Into<String>,
    project_slug: Option<String>,
    runtime_version: Option<String>,
) -> ProjectInitializedPayload {
    ProjectInitializedPayload::new(
        project_uuid,
        project_slug,
        "spec-kitty init",
        runtime_version,
        now_utc_z(),
    )
}

/// The canonical payload timestamp shape (`...Z`, matching the payload model).
pub fn now_utc_z() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(uuid: &str) -> ProjectInitializedPayload {
        ProjectInitializedPayload::new(
            uuid,
            Some("demo".to_string()),
            "spec-kitty init",
            Some("4.0.0rc8".to_string()),
            "2026-11-02T19:52:04.080305Z",
        )
    }

    #[test]
    fn envelope_matches_the_pinned_contract_with_sorted_keys() {
        let directory = tempfile::tempdir().expect("temp");
        let event = emit_project_initialized(
            directory.path(),
            &payload("48fe8ecb-fbf7-4893-90f5-714b9987ac60"),
            Duration::from_secs(1),
        )
        .expect("emit")
        .expect("event");
        let text = String::from_utf8(event.bytes).expect("utf8");
        assert!(text.ends_with('\n'));
        let row: serde_json::Value = serde_json::from_str(text.trim()).expect("json");
        let keys: Vec<&String> = row.as_object().unwrap().keys().collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
        assert_eq!(row["event_type"], "ProjectInitialized");
        assert_eq!(row["aggregate_type"], "Project");
        assert_eq!(row["schema_version"], "5.0.0");
        assert_eq!(row["project_uuid"], "48fe8ecb-fbf7-4893-90f5-714b9987ac60");
        assert_eq!(row["aggregate_id"], row["project_uuid"]);
        assert_eq!(row["payload"]["project_slug"], "demo");
        assert_eq!(row["payload"]["runtime_version"], "4.0.0rc8");
        assert_eq!(row["payload"]["actor"], "spec-kitty init");
        // An absent runtime version is present as null, never omitted.
        let absent = serde_json::to_value(payload("x")).unwrap();
        assert!(absent.get("runtime_version").is_some());
    }

    #[test]
    fn repeated_init_is_idempotent_on_project_uuid() {
        let directory = tempfile::tempdir().expect("temp");
        let uuid = "48fe8ecb-fbf7-4893-90f5-714b9987ac60";
        assert!(
            emit_project_initialized(directory.path(), &payload(uuid), Duration::from_secs(1))
                .expect("first")
                .is_some()
        );
        assert!(
            emit_project_initialized(directory.path(), &payload(uuid), Duration::from_secs(1))
                .expect("second")
                .is_none()
        );
        let log = std::fs::read_to_string(directory.path().join(PROJECT_EVENTS_REL)).unwrap();
        assert_eq!(log.lines().count(), 1);
        // A different project appends.
        assert!(
            emit_project_initialized(
                directory.path(),
                &payload("11111111-1111-4111-8111-111111111111"),
                Duration::from_secs(1)
            )
            .expect("third")
            .is_some()
        );
        let log = std::fs::read_to_string(directory.path().join(PROJECT_EVENTS_REL)).unwrap();
        assert_eq!(log.lines().count(), 2);
    }

    #[test]
    fn preserves_existing_rows_and_terminates_a_torn_tail() {
        let directory = tempfile::tempdir().expect("temp");
        let path = directory.path().join(PROJECT_EVENTS_REL);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{\"event_type\":\"MissionCreated\"}\n").unwrap();
        emit_project_initialized(
            directory.path(),
            &payload("48fe8ecb-fbf7-4893-90f5-714b9987ac60"),
            Duration::from_secs(1),
        )
        .expect("emit");
        let log = std::fs::read_to_string(&path).unwrap();
        assert!(log.starts_with("{\"event_type\":\"MissionCreated\"}\n"));

        let torn = directory
            .path()
            .join("torn/.kittify/canonical-events.jsonl");
        std::fs::create_dir_all(torn.parent().unwrap()).unwrap();
        std::fs::write(&torn, "{\"event_type\":\"ProjectInitialized\"}").unwrap();
        // The canonical writer inserts the missing terminator and keeps
        // every existing byte (baseline `append_raw_rows_atomic`).
        emit_project_initialized(
            &directory.path().join("torn"),
            &payload("48fe8ecb-fbf7-4893-90f5-714b9987ac60"),
            Duration::from_secs(1),
        )
        .expect("torn tail is terminated, not refused");
        let log = std::fs::read_to_string(&torn).unwrap();
        assert!(log.starts_with("{\"event_type\":\"ProjectInitialized\"}\n{"));
    }

    #[test]
    fn malformed_historical_rows_are_refused_by_the_canonical_writer() {
        let directory = tempfile::tempdir().expect("temp");
        let path = directory.path().join(PROJECT_EVENTS_REL);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "not-json\n{\"event_type\":\"Other\"}\n").unwrap();
        let error = emit_project_initialized(
            directory.path(),
            &payload("48fe8ecb-fbf7-4893-90f5-714b9987ac60"),
            Duration::from_secs(1),
        )
        .expect_err("corrupt stream is refused before replacement");
        assert_eq!(error.code(), "lifecycle_event_invalid");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "not-json\n{\"event_type\":\"Other\"}\n"
        );
    }
}
