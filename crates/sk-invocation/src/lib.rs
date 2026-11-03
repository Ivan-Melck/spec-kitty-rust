//! Native Op audit records and profile-invocation primitives.
//!
//! Stage 1 of the native invocation port: the durable Op trail
//! (`kitty-ops/<ULID>.jsonl`, the index, and the closure spine), the typed
//! `started`/`completed` models with pre-v2 detection, `ModeOfWork` and its
//! entry-command derivation, the canonical invocation-id boundary, Op
//! record-root placement with the admission refusal vocabulary, the read-only
//! Git-backed commit-or-skip probes (`primary` tracking and the exact-path
//! ignore/tracking decision), source-backed write admission, and the actual
//! best-effort per-record / closure-spine commit.
//!
//! This crate is deliberately unwired: no dispatcher calls it, the executor /
//! catalog / router / governance / glossary / StepContract surfaces are not
//! implemented here, and no capability is claimed. [`OpCommitService`] composes
//! the accepted skip probes, canonical protection policies, and the real
//! `sk-git` `plan_safe_commit` / `safe_commit`; it does not implement profile
//! dispatch or CLI wiring. Its only effectful authorities are the `sk-kernel`
//! event-log protocol (lock placement, locked append, torn-tail refusal,
//! fsync/readback), the `sk-kernel` clock, `sk-kernel` Git topology, the
//! `sk-git` commit transaction, and the `sk-workspace` protection-policy
//! reader; path identity uses the kernel's read-only `reference_path`
//! authority (the Python non-strict profile, not the containment/lock
//! resolver). It introduces no second lock and mutates no process-global
//! environment.
//!
//! Byte fidelity: Op lines are rendered with Python's `json.dumps` defaults
//! (declared field order, `", "`/`": "` separators, `ensure_ascii=True`), not
//! `serde_json`'s compact UTF-8 shape. The development oracle
//! (`.venv-rust-oracle`, verified lock-exact) is the byte and validation
//! authority; see `tests/oracle_vectors.rs`.

mod error;
mod modes;
mod op_admission;
mod op_commit;
mod op_commit_policy;
mod op_custody;
mod python_json;
mod record;
mod writer;

pub use error::{
    InvocationError, InvocationIdError, LegacyRecordError, RecordParseError, RecordValidationError,
    UnknownEntryCommand,
};
pub use modes::{
    ENTRY_COMMAND_MODES, ModeOfWork, RecordedMode, classify_mode_of_work, derive_mode,
    mode_permits_evidence,
};
pub use op_admission::{OpAdmissionError, admission_probe_path};
pub use op_commit::{
    ADMISSION_PROBE_RECORD, OP_GIT_PROBE_TIMEOUT, OpClosedBy, OpCommitEvidence, OpCommitOutcome,
    OpCommitService, op_git_options,
};
pub use op_commit_policy::{
    COMMON_PRIMARY_BRANCHES, OpCommitProbeError, OpCommitSkipProbe, PrimaryTracking,
    is_op_record_path, op_commit_skip_reason, primary_tracks_op_records, probe_primary_tracking,
    resolve_primary_refs,
};
pub use op_custody::{
    GitProbeError, OP_WRITE_PROTECTED_BRANCH, OP_WRITE_PROTECTION_CONFIG_INVALID, OpWriteAction,
    OpWriteRefusal, OpWriteRefusalCode, OpWriteRefused, is_worktree_of, locate_op_record_root,
    resolve_op_record_root,
};
pub use python_json::python_repr;
pub use record::{
    ClosedBy, InvocationId, OpCompletedEvent, OpCompletedFields, OpEvent, OpOutcome,
    OpStartedEvent, OpStartedFields, parse_op_event,
};
pub use writer::{
    CorrelationLink, EVENTS_DIR, INDEX_PATH, IndexWarning, InvocationWriter, OP_CLOSURES_FILENAME,
    RECORD_MODE, StartedWrite, append_op_closure, closed_invocation_ids, normalise_ref,
    op_closures_path, read_op_closures,
};
