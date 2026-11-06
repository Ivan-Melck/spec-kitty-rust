//! Compatibility alias for the ONE canonical decision fold.
//!
//! CF14 put the first native event → decision-status fold here because the
//! decision write path was not ported and nothing else owned the interpretation.
//! CF17 promoted that implementation to the canonical
//! [`crate::decision_fold`] (full `IndexEntry`, one model validator, one reopen
//! rule) and retired the private copy: this module now only re-exports it, so
//! the public `sk_workspace::doctor::decision_fold` path keeps working while
//! there is exactly one fold in the tree.
//!
//! Nothing in this module interprets an event. Adding a second implementation
//! here (or anywhere else) would re-create the split-brain the Source contract
//! ("single fold, no second reducer") forbids.

pub use crate::decision_fold::{
    DECISION_OPENED_EVENT, DECISION_RESOLVED_EVENT, FoldError, FoldedDecision, fold_events,
    fold_status, is_allowed_terminal_reopen,
};
