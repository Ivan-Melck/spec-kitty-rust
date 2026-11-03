//! Canonical status **writers**: locking, durable batch replacement, transition
//! emission, lifecycle envelopes, and the lifecycle-envelope migration.
//!
//! This module is the native counterpart of the pinned Python baseline
//! (`specify_cli/status/{locking,store,emit,transition_pipeline,lifecycle_events,
//! migrate_lifecycle_envelope}.py`) at
//! `9aed3a8012cb5eb22c0128b14d525039f95fea7a`. It is deliberately separate from
//! [`crate::replay`] and [`crate::snapshot`]: those are consumer-side reducers
//! and never write.
//!
//! # What is preserved from the baseline
//!
//! - **Lock authority.** Every Mission-scoped write takes one
//!   [`sk_kernel::FileLock`] on `<git-common-dir>/spec-kitty-locks/<key>.status.lock`
//!   (reentrant per thread, bounded or unbounded wait, `.holder` sidecar), keyed
//!   on the Mission *directory* name — or the coordination directory name
//!   `<slug>-<mid8>` when the primary `meta.json` declares
//!   `coordination_branch`. The project-level lifecycle log locks on the fixed
//!   `__project__` sentinel. Both go through
//!   [`sk_kernel::lock_families`] so the filename, sidecar and wait policy cannot
//!   drift from the kernel's single lock door.
//! - **Whole-file atomic replacement, never `O_APPEND`.** Every append reads the
//!   existing bytes, appends the new rows, and replaces the file with
//!   [`sk_kernel::atomic_write`] (temp file in the destination directory, fsync,
//!   rename, parent fsync). A reader therefore sees either the complete old log
//!   or the complete new log. The generic kernel event-log append helper is
//!   *not* used: it is the interim generic journal primitive, and the status
//!   logs keep their own lock family and row-schema verification.
//! - **Readback verification.** After the replacement the writer re-reads the
//!   file and requires byte equality with the intended content; callers then
//!   verify their own rows (event id present, matching mission/WP/lane). An
//!   `AtomicCommitIndeterminate` outcome is *recovered by readback*, never
//!   treated as "nothing happened" and never deleted.
//! - **Row encoding.** `status.events.jsonl`, `canonical-events.jsonl` and the
//!   migrated lifecycle rows use Python's `json.dumps(row, sort_keys=True)`
//!   shape (sorted keys, `", "` / `": "` separators, ASCII-escaped strings)
//!   after the baseline `sanitize_event_for_log` PII strip, so rows stay
//!   byte-identical to the pinned writer and round-trip through the Python
//!   reader unchanged.
//! - **Malformed-stream refusal.** A log holding a non-blank line that is not a
//!   JSON object is refused (with its 1-based line number) *before* anything is
//!   replaced. The baseline appended and then failed verification, which left
//!   the appended row on disk; refusing first is a deliberate fail-closed
//!   strengthening (recorded in `WRITER-REPORT.md`).
//! - **Idempotence.** Lifecycle envelopes dedupe on
//!   `(event_type, dedup tuple)`; the envelope migration is a no-op on an
//!   already-strict row and refuses to overwrite its own snapshot; the snapshot
//!   writer is no-op stable through [`sk_kernel::atomic_write`].
//!
//! # Deliberate gaps (see `WRITER-REPORT.md`)
//!
//! - Lane-head policy stamping (`policy_metadata.lane_head`) needs Git; the
//!   writer never shells out, so no stamp is written.
//! - The phase-1 legacy frontmatter `lane` mirror and the ledger-gated
//!   `execution_projection` refresh are not ported.
//! - Lifecycle payload *models* (`spec_kitty_events.project_lifecycle`) are not
//!   re-implemented; callers supply the payload object and the writer owns the
//!   envelope, dedupe and durability.
//! - Fan-out (`status/adapters.py`, `runtime.next.event_bridge`) is an external
//!   transport seam; local persistence is complete, delivery is not attempted.
//! - The optional dependency on a YAML parser is avoided by a documented
//!   frontmatter subset reader used only for the unforced
//!   `in_progress -> for_review` subtask gate.

use std::io;
use std::path::PathBuf;

use crate::snapshot::SnapshotError;
use crate::transition::TransitionError;

pub use crate::actor_identity::{
    GENERIC_IMPLEMENTATION_ACTORS, PROFILE_SOURCE_FIELD, UNKNOWN_ACTOR, actor_full_identity,
    actor_identities_independent, actor_key, actor_value_full_identity, latest_implementer_actor,
    latest_implementer_key, parse_boundary,
};
pub use crate::mutation_commit::{
    AffectedFile, CommitConfigError, DurabilityClassification, ProjectCommitConfig,
    ReviewCycleArtifact, ReviewCycleError, VerdictDurabilityOutcome, next_review_cycle_number,
    read_auto_commit_config, review_cycle_commit_message, review_cycle_dir, review_cycle_pointer,
    status_commit_message,
};
pub use crate::mutation_delta::{
    InnerStateDelta, ReviewOverride, SCALAR_FIELDS, SUBTASK_NOT_APPLICABLE, SUBTASK_SKIPPED,
    SubtaskValue, python_bool, python_int, python_str,
};
pub use crate::mutation_emit::{
    ActorValue, AnnotationDraft, AnnotationOutcome, AnnotationRequest, EmitOutcome, LANE_HEAD_KEY,
    PreparedTransition, StatusEventDraft, TransitionRequest, derive_from_lane,
    emit_inner_state_changed, emit_status_transition, emit_status_transition_batch,
    prepare_transition, read_event_stream,
};
pub use crate::mutation_json::{
    PII_FIELDS, python_compact_json, python_indented_json, sanitize_event_for_log,
    serialize_event_line,
};
pub use crate::mutation_lifecycle::{
    LIFECYCLE_ENVELOPE_KEYS, LIFECYCLE_ENVELOPE_SCHEMA_VERSION, LIFECYCLE_EVENT_TYPES,
    LifecycleAppend, LifecycleRequest, LifecycleSkip, MISSION_EVENTS_FILENAME,
    PROJECT_EVENTS_FILENAME, ProjectInitializedRequest, append_lifecycle_event,
    emit_project_initialized, has_lifecycle_event, mission_event_log_path,
    persist_lifecycle_event_local, project_event_log_path, read_lifecycle_events,
};
pub use crate::mutation_lock::{
    MISSION_WRITE_LOCK_TIMEOUT, MissionLock, MissionLockExtension, ProjectLock,
    acquire_mission_birth_lock, acquire_mission_lock, acquire_mission_lock_with,
    acquire_project_lock, canonical_repo_root, canonicalize_feature_dir,
    extend_mission_lock_to_moved_key, mission_lock_key, mission_lock_key_with, mission_lock_path,
    project_lock_path, resolve_lock_root,
};
pub use crate::mutation_log::{
    AppendOptions, AppendOutcome, Durability, LogSnapshot, MAX_EVENT_LOG_BYTES, MaterializeOutcome,
    append_rows_atomic, capture_log_snapshot, events_path, materialize_snapshot_file,
    parse_log_rows, read_log_rows, read_log_text, restore_log_snapshot, verify_event_rows,
};
pub use crate::mutation_migrate::{
    LEGACY_SCHEMA_VERSION, MIGRATION_BACKUP_SUFFIX, MigrationAction, MigrationManifest,
    MigrationRowResult, STRICT_SCHEMA_VERSION, SYNTHESIZED_KEYS, build_id_for,
    migrate_lifecycle_envelope, node_id_from_host_user,
};
pub use crate::mutation_mirror::{
    MirrorOutcome, legacy_lane_mirror_enabled, mirror_phase1_frontmatter_lane, read_status_phase,
};
pub use crate::mutation_payloads::{canonicalize_lifecycle_payload, lifecycle_payload_keys};
pub use crate::mutation_readiness::{
    DependencyVerdict, OPERATOR_REASON_SOURCE, UNRESOLVABLE_MARKER, declared_dependencies,
    has_operator_provenance, is_acceptable_ending, readiness_from_snapshot,
    resolve_dependency_verdict, wp_lanes_from_snapshot,
};
pub use crate::mutation_rollback::{
    HeadCommittedProbe, RollbackOutcome, RollbackPoint, RollbackRefusal, STATUS_ROLLBACK_REFUSED,
    appended_event_ids, capture_rollback_point, rollback_events_log, rollback_status_artifacts,
};
pub use crate::mutation_ulid::{UlidFactory, is_ulid, new_ulid};

/// Typed failure of a canonical status write.
///
/// Every variant is actionable: the message names the file (or lock) and what
/// the caller must do. `may_have_mutated` separates failures that are known
/// not to have replaced anything from failures where the caller must inspect
/// and recover (the kernel's `AtomicCommitIndeterminate` rule).
#[derive(Debug, thiserror::Error)]
pub enum MutationError {
    #[error("status write could not {action} {path}: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error(transparent)]
    Kernel(#[from] sk_kernel::KernelError),

    #[error(transparent)]
    Snapshot(#[from] SnapshotError),

    #[error("could not serialize JSON: {0}")]
    Json(#[from] serde_json::Error),

    #[error("no canonical repository root found from {start}")]
    RootNotFound { start: PathBuf },

    #[error(
        "cannot resolve the Mission lock key for {directory}: the Mission declares coordination topology but records no mid8 (run `spec-kitty migrate backfill-identity`)"
    )]
    LockKeyUnresolved { directory: String },

    #[error(
        "extending the Mission write lock for {directory} needs an enclosing Mission lock hold on this thread"
    )]
    ExtensionRequiresHold { directory: String },

    #[error("refusing to follow symbolic link {path}")]
    SymlinkRefused { path: PathBuf },

    #[error("invalid JSON on line {line} of {path}: {message}")]
    MalformedStream {
        path: PathBuf,
        line: usize,
        message: String,
    },

    #[error("invalid row: {reason}")]
    InvalidRow { reason: String },

    #[error("invalid request: {reason}")]
    InvalidRequest { reason: String },

    #[error("invalid actor: {reason}")]
    InvalidActor { reason: String },

    #[error(transparent)]
    Transition(#[from] TransitionError),

    #[error("transition refused: {reason}")]
    TransitionRefused { reason: String },

    #[error("missing required field {field}")]
    MissingField { field: &'static str },

    #[error(
        "readback of {path} did not match the {expected_bytes}-byte content just written ({actual_bytes} bytes on disk); the previous log was kept and no other writer's content was overwritten"
    )]
    ReadbackMismatch {
        path: PathBuf,
        expected_bytes: u64,
        actual_bytes: u64,
    },

    #[error("refusing to roll back {path}: {reason}")]
    RollbackRefused { path: PathBuf, reason: String },

    #[error(
        "capturing a rollback point for {directory} requires the Mission write lock; hold it across the capture, the write and the rollback"
    )]
    RollbackRequiresHold { directory: String },

    #[error(
        "replacement of {path} is indeterminate ({detail}); read the file back before retrying and never delete it blindly"
    )]
    CommitIndeterminate { path: PathBuf, detail: String },

    #[error(
        "write of {path} may have committed but post-commit verification failed: {source}; inspect the log before retrying"
    )]
    PostCommitVerification {
        path: PathBuf,
        #[source]
        source: Box<MutationError>,
    },

    #[error("event {event_id} is missing from {path} after the append was verified")]
    EventMissing { path: PathBuf, event_id: String },

    #[error("unknown lifecycle event type {event_type:?}")]
    UnknownLifecycleType { event_type: String },

    #[error(
        "refusing to migrate {path}: a prior snapshot already exists at {backup} (the migration is one-shot; remove it manually once satisfied)"
    )]
    MigrationSnapshotExists { path: PathBuf, backup: PathBuf },

    #[error("unsupported lifecycle payload for {event_type}: {reason}")]
    InvalidLifecyclePayload { event_type: String, reason: String },
}

impl MutationError {
    /// True when the failure happened after (or during) a replacement, so the
    /// caller must read the file back rather than assume nothing changed.
    pub fn may_have_mutated(&self) -> bool {
        match self {
            MutationError::ReadbackMismatch { .. }
            | MutationError::PostCommitVerification { .. }
            | MutationError::EventMissing { .. }
            | MutationError::RollbackRefused { .. }
            | MutationError::CommitIndeterminate { .. } => true,
            MutationError::Kernel(error) => error.mutation_may_have_committed(),
            _ => false,
        }
    }

    /// Retain the successful-write boundary when a subsequent read/check fails.
    pub fn after_commit(self, path: impl Into<PathBuf>) -> Self {
        if self.may_have_mutated() {
            self
        } else {
            Self::PostCommitVerification {
                path: path.into(),
                source: Box::new(self),
            }
        }
    }

    /// Wrap an I/O failure with the operation that produced it.
    pub(crate) fn io(action: &'static str, path: impl Into<PathBuf>, source: io::Error) -> Self {
        MutationError::Io {
            action,
            path: path.into(),
            source,
        }
    }
}
