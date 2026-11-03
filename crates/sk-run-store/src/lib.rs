//! Persistent, governed mission-run storage for the Rust Spec Kitty rewrite.
//!
//! This crate is the durable substrate under `spec-kitty next`: it owns the
//! baseline run layout, the identity index, the event journal, the run-cursor
//! lock, durable start/query/advance/answer transactions, governed answer
//! authorization, and deterministic recovery. Planning itself stays in
//! [`sk_runtime`].
//!
//! # Baseline layout and formats
//!
//! ```text
//! .kittify/runtime/feature-runs.json
//! .kittify/runtime/runs/<uuidhex>/state.json
//! .kittify/runtime/runs/<uuidhex>/mission_template_frozen.yaml
//! .kittify/runtime/runs/<uuidhex>/run.events.jsonl
//! ```
//!
//! The persisted shapes are the upstream Python shapes. No native-only cursor
//! field is written; `actor_context` and other Rust read-model extensions stay
//! in memory. The frozen template is the verbatim source bytes and its
//! SHA-256 is the cursor's `template_hash`.
//!
//! # Lock authority
//!
//! Every mutation takes the upstream per-run `state.json.lock` sidecar through
//! [`sk_kernel::FileLock`]. An advance plans and commits under one hold; a
//! two-phase `plan_advance`/`commit_advance` commit re-reads and refuses a stale
//! plan (compare-and-swap). The run index has its own `feature-runs.json.lock`
//! sidecar, acquired outside the cursor lock on the start path and inside it
//! only for recovery index repair (there is no reverse acquisition, so there is
//! no lock-ordering cycle).
//!
//! # Events and recovery
//!
//! Appends go through the kernel's single locked event-log primitive, preserving
//! upstream torn-tail refusal, one-unit writes, fsync and readback. An explicit
//! [`RunStore::recover`] (also run inside mutating transactions) truncates a
//! torn tail, appends derivable missing `MissionRunStarted` /
//! `DecisionInputAnswered` events, rolls the cursor forward from the journal and
//! repairs a missing index entry. [`RunStore::query`] is non-mutating and returns
//! [`RunStoreError::RecoveryRequired`] when a run is inconsistent.
//!
//! # Scope boundary
//!
//! This crate is the storage substrate, not the command surface. The
//! `spec-kitty next` composition (mission-board override, WP iteration and
//! workspace resolution, guards, prompt authoring, the CLI decision envelope)
//! lives in the `sk-cli` adapter; significance/RACI evaluation and answer policy
//! stay in [`sk_runtime`]. This crate does not implement the event write
//! service. See the crate README, and `NEXT-CONTINUATION-REPORT.md` for the
//! integration evidence and the explicit remaining gaps.

mod advance;
mod answer;
mod error;
mod events;
mod model;
mod prompt;
mod query;
mod recovery;
mod store;

pub use advance::{AdvanceOutcome, AdvanceRequest, PlannedAdvance};
pub use answer::{AnswerOutcome, AnswerRequest};
pub use error::{ErrorInfo, RunStoreError};
pub use events::{
    DECISION_AUTHORITY_DENIED, DECISION_INPUT_ANSWERED, DECISION_INPUT_REQUESTED, EventRecord,
    Journal, MISSION_RUN_COMPLETED, MISSION_RUN_STARTED, NEXT_STEP_AUTO_COMPLETED,
    NEXT_STEP_ISSUED, SIGNIFICANCE_EVALUATED, read_journal,
};
pub use model::{
    ConsistencyReport, DEFAULT_LOCK_TIMEOUT, DecisionView, IssuedStepView, MAX_EVENT_LOG_BYTES,
    MAX_INDEX_BYTES, MAX_PROMPT_BYTES, MAX_STATE_BYTES, QueryResult, RunRef, RunState,
    StartOutcome, StartRequest,
};
pub use recovery::RecoveryReport;
pub use sk_kernel::FileLock;
pub use sk_runtime::{
    ActorIdentity, ActorType, AnswerEffect, DecisionKind, DecisionRequirement,
    MissionPolicySnapshot, NextDecision, ResolvedRaciSnapshot, RuntimeError, SignificanceEvaluated,
    StepResult, TEMPLATE_DRIFT_REASON,
};
pub use store::{
    EVENTS_FILENAME, FROZEN_TEMPLATE_FILENAME, INDEX_FILENAME, LEGACY_KEY_PREFIX, LOCK_SUFFIX,
    RUN_ID_HEX_LENGTH, RunStore, STATE_FILENAME, now_timestamp, run_index_key, validate_mission_id,
    validate_mission_slug, validate_run_id,
};

/// Decision-shape helpers the coordinator CLI needs but `sk-runtime` does not
/// expose as inherent methods.
pub trait NextDecisionExt {
    /// The decision id for a decision-required outcome.
    fn decision_id(&self) -> Option<&str>;
    /// The input key for an input-keyed decision.
    fn input_key(&self) -> Option<&str>;
}

impl NextDecisionExt for NextDecision {
    fn decision_id(&self) -> Option<&str> {
        match self {
            Self::DecisionRequired(required) => match &required.requirement {
                DecisionRequirement::PendingDecision { decision_id, .. }
                | DecisionRequirement::MissingRequiredInput { decision_id, .. }
                | DecisionRequirement::AuditCheckpoint { decision_id, .. } => Some(decision_id),
            },
            _ => None,
        }
    }

    fn input_key(&self) -> Option<&str> {
        match self {
            Self::DecisionRequired(required) => match &required.requirement {
                DecisionRequirement::PendingDecision { input_key, .. } => input_key.as_deref(),
                DecisionRequirement::MissingRequiredInput { input_key, .. } => {
                    Some(input_key.as_str())
                }
                DecisionRequirement::AuditCheckpoint { .. } => None,
            },
            _ => None,
        }
    }
}

/// The crate's compatibility scope, exposed as data for a capability matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompatibilitySurface;

impl CompatibilitySurface {
    /// Persisted file names this crate reads/writes (all baseline names).
    pub const PERSISTED_FILES: [&'static str; 4] = [
        INDEX_FILENAME,
        STATE_FILENAME,
        FROZEN_TEMPLATE_FILENAME,
        EVENTS_FILENAME,
    ];
}
