//! Typed guard inputs and the structured review evidence they carry.

/// Actor prefix of an event a migration synthesized (for example
/// `migration:backfill_runtime_state`).
pub const MIGRATION_ACTOR_PREFIX: &str = "migration:";

/// True when the actor is a migration-synthesized identity.
///
/// The match is the exact, case-sensitive prefix, mirroring the Python
/// `is_migration_actor`.
pub fn is_migration_actor(actor: Option<&str>) -> bool {
    actor.is_some_and(|value| value.starts_with(MIGRATION_ACTOR_PREFIX))
}

/// A reviewer's approval record (`DoneEvidence.review`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReviewApproval {
    pub reviewer: String,
    /// `"approved"` or `"changes_requested"`.
    pub verdict: String,
    /// Approval reference or `feedback://` URI.
    pub reference: String,
}

/// Structured review outcome required on every outbound `in_review` edge.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReviewResult {
    pub reviewer: String,
    /// `"approved"` or `"changes_requested"`.
    pub verdict: String,
    /// Approval reference or `feedback://` URI.
    pub reference: String,
    /// Resolved path to a feedback file (rejection only).
    pub feedback_path: Option<String>,
}

/// Completion evidence carried by approval/done transitions.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DoneEvidence {
    pub review: Option<ReviewApproval>,
}

/// Every input a lane guard may consult.
///
/// The tri-state `dependency_ready` is intentionally fail-open: `None` means
/// no verdict was supplied and passes; only `Some(false)` refuses.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GuardContext {
    pub actor: Option<String>,
    pub workspace_context: Option<String>,
    pub subtasks_complete: Option<bool>,
    pub implementation_evidence_present: Option<bool>,
    pub reason: Option<String>,
    pub review_ref: Option<String>,
    pub evidence: Option<DoneEvidence>,
    pub force: bool,
    pub review_result: Option<ReviewResult>,
    pub current_actor: Option<String>,
    pub dependency_ready: Option<bool>,
}
