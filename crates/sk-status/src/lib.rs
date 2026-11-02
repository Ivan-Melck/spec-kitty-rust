//! `sk-status` — read-only Spec Kitty status domain.
//!
//! Two bounded surfaces live here:
//!
//! - [`lane`]/[`transition`]: the closed eleven-lane work-package FSM, a pure
//!   typed decision function validated row-for-row against the committed Python
//!   parity baseline.
//! - [`event_stream`]: a bounded, read-only typed inspector for
//!   `status.events.jsonl`. It reports kinds, counts and preserved payloads. It
//!   is deliberately **not** a reducer and makes no current-state claim; the
//!   canonical reducer belongs to the external events package.
//!
//! This crate never writes an event log and never invokes the Python
//! implementation at runtime.

#![forbid(unsafe_code)]

pub mod event_stream;
pub mod guard;
pub mod lane;
pub mod transition;

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
pub use transition::{
    REVIEW_REF_REQUIRED_EDGES, TransitionError, has_review_ref, requires_review_ref,
    validate_transition, validate_transition_names,
};
