//! `sk-status` — the Spec Kitty status domain.
//!
//! Five bounded surfaces live here:
//!
//! - [`lane`]/[`transition`]: the closed eleven-lane work-package FSM, a pure
//!   typed decision function validated row-for-row against the committed Python
//!   parity baseline.
//! - [`event_stream`]: a bounded, read-only typed inspector for
//!   `status.events.jsonl`. It reports kinds, counts and preserved payloads.
//! - [`replay`]: the canonical consumer-side diary parser and deterministic
//!   reducer, preserving the pinned `spec_kitty_events.diary` fold semantics
//!   (dedupe, causal continuity, rollback arbitration, approved-departure
//!   lifting, reconstruction-seed payload folding, and the annotation pass).
//! - [`snapshot`]/[`schema`]: typed read/materialize/rendering of the derived
//!   `status.json`, its schema generations, cancellation/implementer
//!   projections, mission identity, retrospective attachment, and a structural
//!   schema auditor.
//! - [`mutation`]: the **writers** — Mission/project lock authority, whole-file
//!   atomic batch replacement with readback verification, transition emission
//!   (`status.events.jsonl` + `status.json`), lifecycle envelopes
//!   (`canonical-events.jsonl`) and the lifecycle-envelope migration. Writers
//!   never use generic `O_APPEND` and never invoke Python.
//!
//! Golden fixtures in `tests/fixtures/replay` and `tests/fixtures/mutation` were
//! generated once from the pinned oracle and record their provenance. The
//! reader surfaces ([`replay`], [`snapshot`]) stay pure; only [`mutation`]
//! writes, and every write is verified by readback.

#![forbid(unsafe_code)]

pub mod actor_identity;
pub mod event_stream;
pub mod guard;
pub mod lane;
pub mod mutation;
pub mod mutation_commit;
pub mod mutation_delta;
pub mod mutation_emit;
pub mod mutation_json;
pub mod mutation_lifecycle;
pub mod mutation_lock;
pub mod mutation_log;
pub mod mutation_migrate;
pub mod mutation_mirror;
pub mod mutation_payloads;
pub mod mutation_readiness;
pub mod mutation_rollback;
pub mod mutation_ulid;
pub mod replay;
pub mod schema;
pub mod snapshot;
pub mod transition;

pub use actor_identity::{
    GENERIC_IMPLEMENTATION_ACTORS, PROFILE_SOURCE_FIELD, UNKNOWN_ACTOR, actor_full_identity,
    actor_identities_independent, actor_key, actor_value_full_identity, latest_implementer_actor,
    latest_implementer_key, parse_boundary,
};
pub use event_stream::{
    AnnotationRecord, EventActor, EventCounts, EventKind, EventStreamError, EventStreamInspection,
    EventStreamLimits, InspectedEvent, NonLaneCategory, NonLaneRecord, StructuredActor,
    TrailingPartial, TransitionRecord, inspect_event_reader, inspect_event_reader_with_limits,
    inspect_event_stream, inspect_event_stream_with_limits,
};
pub use guard::{
    DoneEvidence, GuardContext, MIGRATION_ACTOR_PREFIX, ReviewApproval, ReviewResult,
    is_migration_actor,
};
pub use lane::{Lane, LaneParseError, resolve_lane_alias};
pub use mutation::MutationError;
pub use mutation_commit::{
    AffectedFile, CommitConfigError, DurabilityClassification, ProjectCommitConfig,
    ReviewCycleArtifact, ReviewCycleError, VerdictDurabilityOutcome, next_review_cycle_number,
    read_auto_commit_config, review_cycle_commit_message, review_cycle_dir, review_cycle_pointer,
    status_commit_message,
};
pub use mutation_delta::{
    InnerStateDelta, ReviewOverride, SCALAR_FIELDS, SUBTASK_NOT_APPLICABLE, SUBTASK_SKIPPED,
    SubtaskValue, python_bool, python_int, python_str,
};
pub use mutation_emit::{EmitOutcome, TransitionRequest, emit_status_transition};
pub use mutation_lifecycle::{LifecycleRequest, append_lifecycle_event};
pub use mutation_log::{AppendOptions, append_rows_atomic, materialize_snapshot_file};
pub use mutation_migrate::{MigrationManifest, migrate_lifecycle_envelope};
pub use mutation_mirror::{
    MirrorOutcome, legacy_lane_mirror_enabled, mirror_phase1_frontmatter_lane, read_status_phase,
};
pub use mutation_payloads::{canonicalize_lifecycle_payload, lifecycle_payload_keys};
pub use mutation_readiness::{
    DependencyVerdict, OPERATOR_REASON_SOURCE, UNRESOLVABLE_MARKER, declared_dependencies,
    has_operator_provenance, is_acceptable_ending, readiness_from_snapshot,
    resolve_dependency_verdict, wp_lanes_from_snapshot,
};
pub use mutation_rollback::{
    HeadCommittedProbe, RollbackOutcome, RollbackPoint, RollbackRefusal, STATUS_ROLLBACK_REFUSED,
    appended_event_ids, capture_rollback_point, rollback_events_log, rollback_status_artifacts,
};
pub use replay::{DiaryError, DiaryStream, InnerStateChanged, StatusEvent, parse_diary};
pub use schema::{SchemaAudit, SchemaAuditor, SchemaFinding, audit_event_rows, audit_snapshot};
pub use snapshot::{
    CURRENT_SNAPSHOT_SCHEMA_VERSION, EVENTS_FILENAME, IMPLEMENTER_OF_RECORD_SCHEMA_VERSION,
    IMPLEMENTER_OF_RECORD_SLOT, LEGACY_SNAPSHOT_SCHEMA_VERSION, META_FILENAME, ReplayInput,
    RetrospectiveSnapshot, SNAPSHOT_FILENAME, Snapshot, SnapshotError, WorkPackageState,
    existing_schema_version, materialize, materialize_dir, read_raw_events, reduce_rows,
    render_snapshot_json, target_schema_version,
};
pub use transition::{
    REVIEW_REF_REQUIRED_EDGES, TransitionError, has_review_ref, requires_review_ref,
    validate_transition, validate_transition_names,
};
