//! One-shot migration of legacy lifecycle envelopes to the strict shape.
//!
//! Native counterpart of `specify_cli.status.migrate_lifecycle_envelope`. The
//! migration rewrites **only envelope bookkeeping** on lifecycle rows
//! (`event_type` in [`crate::mutation_lifecycle::LIFECYCLE_EVENT_TYPES`]):
//!
//! - drop `aggregate_type`;
//! - set `schema_version` to `"3.0.0"`;
//! - synthesize `causation_id: null`, `correlation_id = event_id`,
//!   `data_tier: 0`, `node_id`, `lamport_clock: 0`, and
//!   `build_id = uuid5(build-id-namespace, "<project_uuid>:<node_id>")`.
//!
//! Everything else — lane transitions, annotations, retrospective and decision
//! rows — is copied through **byte-identical**, never even parsed as a migration
//! candidate. `ReviewerSelfApproval` (no strict model) and rows with a null
//! `project_uuid` (no strict envelope can be built) are skipped in place.
//!
//! Safety properties, all baseline-derived:
//!
//! - **All-or-nothing.** The whole file is replaced atomically; a mid-batch
//!   failure leaves the original untouched.
//! - **Snapshot refusal.** The pre-migration text is written to
//!   `<name>.pre-migration.bak` before the rewrite, and an existing snapshot is
//!   a refusal, not an overwrite.
//! - **Idempotence.** A re-run over an already strict file migrates nothing and
//!   never plants a spurious snapshot or trips the snapshot refusal.
//! - **Same lock as ordinary writers.** The project log uses the `__project__`
//!   lock; the mission log uses the Mission status lock.
//! - **Content proof.** Each lifecycle row's result carries a SHA-256 of the
//!   canonical `(event_type, payload)` pair before and after, so a matching pair
//!   proves only the envelope changed.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use sk_kernel::content_digest::sha256_hex;
use sk_kernel::locks::LockWait;

use crate::mutation::MutationError;
use crate::mutation_json::python_compact_json;
use crate::mutation_lifecycle::{LIFECYCLE_EVENT_TYPES, PROJECT_EVENTS_FILENAME};
use crate::mutation_lock::{acquire_mission_lock, acquire_project_lock, canonical_repo_root};
use crate::mutation_log::{MAX_EVENT_LOG_BYTES, read_log_text};

/// The six strict-envelope keys synthesized from existing row data.
pub const SYNTHESIZED_KEYS: [&str; 6] = [
    "causation_id",
    "correlation_id",
    "data_tier",
    "node_id",
    "lamport_clock",
    "build_id",
];

/// Schema version written by the migration.
pub const STRICT_SCHEMA_VERSION: &str = "3.0.0";

/// Schema version of the legacy local envelope.
pub const LEGACY_SCHEMA_VERSION: &str = "5.0.0";

/// Suffix of the one-shot pre-migration snapshot.
pub const MIGRATION_BACKUP_SUFFIX: &str = ".pre-migration.bak";

/// Default bounded wait for the migration lock (baseline 10 s).
const DEFAULT_LOCK_WAIT_SECONDS: u64 = 10;

/// The UUID URL namespace from RFC 4122 (`6ba7b811-9dad-11d1-80b4-00c04fd430c8`).
const NAMESPACE_URL: [u8; 16] = [
    0x6b, 0xa7, 0xb8, 0x11, 0x9d, 0xad, 0x11, 0xd1, 0x80, 0xb4, 0x00, 0xc0, 0x4f, 0xd4, 0x30, 0xc8,
];

/// Per-row outcome of a migration pass.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MigrationAction {
    /// The row was rewritten into the strict envelope.
    Migrated,
    /// The row was already strict and is unchanged.
    Unchanged,
    /// `ReviewerSelfApproval` has no strict model.
    SkippedNoModel,
    /// The row records no `project_uuid`, so no strict envelope exists.
    SkippedNoProjectUuid,
}

impl MigrationAction {
    /// The baseline action vocabulary.
    pub fn as_str(self) -> &'static str {
        match self {
            MigrationAction::Migrated => "migrated",
            MigrationAction::Unchanged => "unchanged",
            MigrationAction::SkippedNoModel => "skipped_no_model",
            MigrationAction::SkippedNoProjectUuid => "skipped_no_project_uuid",
        }
    }
}

/// One row's migration evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationRowResult {
    pub event_id: String,
    pub event_type: String,
    pub action: MigrationAction,
    /// SHA-256 of the canonical `(event_type, payload)` pair before.
    pub pre_hash: String,
    /// SHA-256 of the canonical `(event_type, payload)` pair after.
    pub post_hash: String,
}

/// Aggregate outcome of one migration run.
#[derive(Clone, Debug)]
pub struct MigrationManifest {
    pub log_path: PathBuf,
    /// Lifecycle rows examined (non-lifecycle rows are not counted).
    pub total_rows: usize,
    pub migrated_count: usize,
    pub unchanged_count: usize,
    pub skipped_count: usize,
    pub rows: Vec<MigrationRowResult>,
    /// Set when the pass refused to write (for example an existing snapshot).
    pub refused_reason: Option<String>,
    /// True when the manifest was computed without writing.
    pub dry_run: bool,
}

impl MigrationManifest {
    /// True when the pass wrote (or would write) a new file.
    pub fn changed(&self) -> bool {
        self.migrated_count > 0 && self.refused_reason.is_none() && !self.dry_run
    }
}

/// The stable machine identifier the migration synthesizes into `node_id`.
///
/// The baseline derivation is `sha256(f"{hostname}:{getpass.getuser()}")[:12]`.
/// This reads the same environment sources without the `getpass` module: it is
/// the identity of the machine running the migration, never a reconstruction of
/// the original writer's identity (which was never recorded).
pub fn node_id_from_host_user() -> String {
    let host = std::env::var("HOSTNAME")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            std::env::var("COMPUTERNAME")
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        })
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| "unknown".to_owned());
    let user = ["LOGNAME", "USER", "LNAME", "USERNAME"]
        .iter()
        .find_map(|key| {
            std::env::var(key)
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| "unknown".to_owned());
    let digest = sha256_hex(format!("{host}:{user}").as_bytes());
    digest[..12].to_owned()
}

/// `uuid5(build-id-namespace, "<project_uuid>:<node_id>")`.
///
/// This reproduces `specify_cli.identity.project.derive_build_id`: the namespace
/// is `uuid5(NAMESPACE_URL, "spec-kitty:identity:build_id")`.
pub fn build_id_for(project_uuid: &str, node_id: &str) -> String {
    let namespace = uuid::Uuid::new_v5(
        &uuid::Uuid::from_bytes(NAMESPACE_URL),
        b"spec-kitty:identity:build_id",
    );
    let normalized = parse_python_uuid(project_uuid)
        .ok()
        .map(|id| id.hyphenated().to_string());
    uuid::Uuid::new_v5(
        &namespace,
        format!(
            "{}:{node_id}",
            normalized.as_deref().unwrap_or(project_uuid)
        )
        .as_bytes(),
    )
    .hyphenated()
    .to_string()
}

fn parse_python_uuid(value: &str) -> Result<uuid::Uuid, uuid::Error> {
    let stripped = value.replace("urn:", "").replace("uuid:", "");
    let hex = stripped.trim_matches(['{', '}']).replace('-', "");
    uuid::Uuid::parse_str(&hex)
}

/// Run the envelope migration over `log_path`.
///
/// `dry_run` computes the manifest without taking a lock or writing anything.
pub fn migrate_lifecycle_envelope(
    log_path: &Path,
    dry_run: bool,
    node_id: Option<&str>,
    wait: Option<LockWait>,
) -> Result<MigrationManifest, MutationError> {
    let node_id = node_id
        .map(str::to_owned)
        .unwrap_or_else(node_id_from_host_user);
    if dry_run {
        let (manifest, _lines) = compute_manifest(log_path, &node_id, true)?;
        return Ok(manifest);
    }

    let _lock = lock_for_log(log_path, wait)?;
    let (mut manifest, new_lines) = compute_manifest(log_path, &node_id, false)?;
    if manifest.migrated_count == 0 {
        // Nothing to migrate (already strict, or empty): no snapshot, no write.
        return Ok(manifest);
    }
    let backup_path = log_path.with_file_name(format!(
        "{}{MIGRATION_BACKUP_SUFFIX}",
        log_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("events")
    ));
    if backup_path.exists() {
        manifest.refused_reason = Some(format!(
            "refusing to migrate: a prior snapshot already exists at {} (migration is one-shot per file; remove the snapshot manually once satisfied with the prior run before re-migrating)",
            backup_path.display()
        ));
        return Ok(manifest);
    }
    let original = read_log_text(log_path)?.unwrap_or_default();
    sk_kernel::atomic_write_with_options(
        &backup_path,
        original.as_bytes(),
        sk_kernel::AtomicWriteOptions {
            create_parents: true,
        },
    )?;
    let mut content = String::new();
    for line in new_lines {
        content.push_str(&line);
        content.push('\n');
    }
    if content.len() as u64 > MAX_EVENT_LOG_BYTES {
        return Err(MutationError::Kernel(sk_kernel::KernelError::TooLarge {
            path: log_path.to_path_buf(),
            limit: MAX_EVENT_LOG_BYTES,
        }));
    }
    sk_kernel::atomic_write_with_options(
        log_path,
        content.as_bytes(),
        sk_kernel::AtomicWriteOptions {
            create_parents: true,
        },
    )?;
    Ok(manifest)
}

// The guard variants exist to hold the RAII lock for the migration's duration;
// their fields are deliberately never read.
#[allow(dead_code)]
enum LogLock {
    Mission(crate::mutation_lock::MissionLock),
    Project(crate::mutation_lock::ProjectLock),
    None,
}

fn lock_for_log(log_path: &Path, wait: Option<LockWait>) -> Result<LogLock, MutationError> {
    let wait = wait.unwrap_or(LockWait::Bounded(std::time::Duration::from_secs(
        DEFAULT_LOCK_WAIT_SECONDS,
    )));
    let Some(parent) = log_path.parent() else {
        return Ok(LogLock::None);
    };
    let Ok(repo_root) = canonical_repo_root(parent) else {
        // Baseline: an unresolvable repo root skips locking (the write itself is
        // still atomic).
        return Ok(LogLock::None);
    };
    let is_project_log = log_path
        .file_name()
        .is_some_and(|name| name == PROJECT_EVENTS_FILENAME);
    if is_project_log {
        return Ok(LogLock::Project(acquire_project_lock(&repo_root, wait)?));
    }
    Ok(LogLock::Mission(acquire_mission_lock(
        parent,
        Some(&repo_root),
        wait,
    )?))
}

fn compute_manifest(
    log_path: &Path,
    node_id: &str,
    dry_run: bool,
) -> Result<(MigrationManifest, Vec<String>), MutationError> {
    let text = read_log_text(log_path)?.unwrap_or_default();
    let mut results = Vec::new();
    let mut new_lines = Vec::new();
    let (mut migrated, mut unchanged, mut skipped) = (0_usize, 0_usize, 0_usize);

    for (index, raw_line) in text.lines().enumerate() {
        let line_number = index + 1;
        if raw_line.trim().is_empty() {
            continue;
        }
        let row: Value =
            serde_json::from_str(raw_line).map_err(|error| MutationError::MalformedStream {
                path: log_path.to_path_buf(),
                line: line_number,
                message: error.to_string(),
            })?;
        let Some(object) = row.as_object() else {
            return Err(MutationError::MalformedStream {
                path: log_path.to_path_buf(),
                line: line_number,
                message: "expected a JSON object".to_owned(),
            });
        };
        let event_type = object.get("event_type").and_then(Value::as_str);
        let Some(event_type) = event_type else {
            // Not a lifecycle row: byte-identical pass-through.
            new_lines.push(raw_line.to_owned());
            continue;
        };
        if !LIFECYCLE_EVENT_TYPES.contains(&event_type) {
            new_lines.push(raw_line.to_owned());
            continue;
        }

        let pre_hash = content_hash(object.get("event_type"), object.get("payload"));
        let (migrated_row, action) = migrate_row(object, node_id)?;
        let post_hash = content_hash(migrated_row.get("event_type"), migrated_row.get("payload"));
        results.push(MigrationRowResult {
            event_id: object
                .get("event_id")
                .map(render_scalar)
                .unwrap_or_default(),
            event_type: event_type.to_owned(),
            action,
            pre_hash,
            post_hash,
        });
        match action {
            MigrationAction::Migrated => {
                migrated += 1;
                new_lines.push(python_compact_json(&Value::Object(migrated_row)));
            }
            MigrationAction::Unchanged => {
                unchanged += 1;
                new_lines.push(raw_line.to_owned());
            }
            MigrationAction::SkippedNoModel | MigrationAction::SkippedNoProjectUuid => {
                skipped += 1;
                new_lines.push(raw_line.to_owned());
            }
        }
    }

    Ok((
        MigrationManifest {
            log_path: log_path.to_path_buf(),
            total_rows: results.len(),
            migrated_count: migrated,
            unchanged_count: unchanged,
            skipped_count: skipped,
            rows: results,
            refused_reason: None,
            dry_run,
        },
        new_lines,
    ))
}

fn migrate_row(
    object: &Map<String, Value>,
    node_id: &str,
) -> Result<(Map<String, Value>, MigrationAction), MutationError> {
    if object.get("event_type").and_then(Value::as_str) == Some("ReviewerSelfApproval") {
        return Ok((object.clone(), MigrationAction::SkippedNoModel));
    }
    if is_already_strict_shaped(object) {
        return Ok((object.clone(), MigrationAction::Unchanged));
    }
    let project_uuid = match object.get("project_uuid") {
        None | Some(Value::Null) => {
            return Ok((object.clone(), MigrationAction::SkippedNoProjectUuid));
        }
        Some(value) => parse_python_uuid(
            &value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| python_compact_json(value)),
        )
        .ok()
        .ok_or_else(|| MutationError::InvalidLifecyclePayload {
            event_type: object
                .get("event_type")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            reason: format!("project_uuid {value} is not a UUID string"),
        })?
        .hyphenated()
        .to_string(),
    };

    let mut migrated = object.clone();
    migrated.remove("aggregate_type");
    migrated.insert(
        "schema_version".to_owned(),
        Value::String(STRICT_SCHEMA_VERSION.to_owned()),
    );
    migrated.insert("causation_id".to_owned(), Value::Null);
    migrated.insert(
        "correlation_id".to_owned(),
        object.get("event_id").cloned().unwrap_or(Value::Null),
    );
    migrated.insert("data_tier".to_owned(), Value::from(0));
    migrated.insert("node_id".to_owned(), Value::String(node_id.to_owned()));
    migrated.insert("lamport_clock".to_owned(), Value::from(0));
    migrated.insert(
        "build_id".to_owned(),
        Value::String(build_id_for(&project_uuid, node_id)),
    );
    Ok((migrated, MigrationAction::Migrated))
}

fn is_already_strict_shaped(row: &Map<String, Value>) -> bool {
    row.get("schema_version").and_then(Value::as_str) == Some(STRICT_SCHEMA_VERSION)
        && SYNTHESIZED_KEYS.iter().all(|key| row.contains_key(*key))
        && !row.contains_key("aggregate_type")
}

fn content_hash(event_type: Option<&Value>, payload: Option<&Value>) -> String {
    let canonical = json!({
        "event_type": event_type.cloned().unwrap_or(Value::Null),
        "payload": payload.cloned().unwrap_or(Value::Null),
    });
    sha256_hex(python_compact_json(&canonical).as_bytes())
}

fn render_scalar(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}
