//! The pure lane FSM: structural edge + guard + force decision.
//!
//! [`validate_transition`] is the canonical typed entry point. It mirrors the
//! Python `WPState.check_transition` authority exactly: a genesis target is
//! refused before force is consulted, a `for_review -> in_progress` event must
//! carry a usable `review_ref` (migration actors excepted), and force with
//! actor + reason otherwise bypasses missing edges and guards.

use crate::guard::{GuardContext, is_migration_actor};
use crate::lane::{Lane, LaneParseError};

/// The edges whose persisted event requires a non-blank `review_ref`.
pub const REVIEW_REF_REQUIRED_EDGES: &[(Lane, Lane)] = &[(Lane::ForReview, Lane::InProgress)];

/// True when an event on this edge must carry a `review_ref`.
pub fn requires_review_ref(from: Lane, to: Lane) -> bool {
    REVIEW_REF_REQUIRED_EDGES.contains(&(from, to))
}

/// True when the value is a usable review pointer: non-blank text.
pub fn has_review_ref(review_ref: Option<&str>) -> bool {
    review_ref.is_some_and(|value| !value.trim().is_empty())
}

/// A typed transition refusal. [`Display`](std::fmt::Display) is the exact
/// parity message the Python implementation produced.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransitionError {
    #[error("Unknown lane: {input}")]
    UnknownLane { input: String },

    /// `genesis` is a seed source, never a persisted/display target. Checked
    /// before force, so force cannot create a current genesis WP.
    #[error("Illegal transition: {from} -> {to}")]
    GenesisTargetRefused { from: Lane, to: Lane },

    /// No structural edge, and force was not requested.
    #[error("Illegal transition: {from} -> {to}")]
    IllegalEdge { from: Lane, to: Lane },

    #[error("Force transitions require actor and reason")]
    ForceRequiresActorAndReason,

    #[error(
        "Transition {from} -> {to} requires review_ref: a forced rewind out of review \
         must cite the review feedback it is rewinding (pass `--review-ref <ref>`)."
    )]
    ReviewRefRequired { from: Lane, to: Lane },

    #[error("Transition requires review_ref (review feedback reference)")]
    ReviewRefRequiredForRewrite,

    #[error("Transition requires actor identity")]
    ActorIdentityRequired,

    #[error("Transition claimed -> in_progress requires workspace context")]
    WorkspaceContextRequired,

    #[error(
        "Transition {from} -> {to} blocked: unsatisfied dependencies (force with reason to override)"
    )]
    DependenciesUnsatisfied { from: Lane, to: Lane },

    #[error(
        "Transition in_progress -> for_review requires completed subtasks or force with reason"
    )]
    CompletedSubtasksRequired,

    #[error(
        "Transition in_progress -> for_review requires implementation evidence or force with reason"
    )]
    ImplementationEvidenceRequired,

    #[error("Transition in_progress -> planned requires reason")]
    TransitionReasonRequired,

    #[error(
        "Transition to approved/done requires evidence (reviewer identity and approval reference)"
    )]
    ReviewerApprovalRequired,

    #[error("Transition from in_review requires review_result (structured review outcome)")]
    ReviewResultRequired,

    #[error("Transition from in_review requires review_result with reviewer")]
    ReviewResultReviewerRequired,

    #[error("Transition from in_review requires review_result with verdict")]
    ReviewResultVerdictRequired,

    #[error("Transition from in_review requires review_result with reference")]
    ReviewResultReferenceRequired,

    #[error("review_ref must match review_result.reference")]
    ReviewRefMismatch,

    #[error("Review evidence must match review_result")]
    ReviewEvidenceMismatch,
}

impl TransitionError {
    /// The parity message as an owned string.
    pub fn message(&self) -> String {
        self.to_string()
    }
}

impl From<LaneParseError> for TransitionError {
    fn from(error: LaneParseError) -> Self {
        TransitionError::UnknownLane { input: error.input }
    }
}

/// Validate a lane transition. Returns `Ok(())` when admitted.
///
/// This is pure: it performs no I/O and does not inspect any event log. It
/// answers only whether this transition is admitted under this context.
pub fn validate_transition(
    from: Lane,
    to: Lane,
    ctx: &GuardContext,
) -> Result<(), TransitionError> {
    // Genesis is never a target, and this refusal is not force-insurmountable.
    if to == Lane::Genesis {
        return Err(TransitionError::GenesisTargetRefused { from, to });
    }

    // A missing edge needs force before anything else can be considered.
    if !from.may_transition_to(to) && !ctx.force {
        return Err(TransitionError::IllegalEdge { from, to });
    }

    // Force still needs an auditable actor + reason. Audit is checked before
    // the review_ref rule so a reasonless force keeps its historical message.
    if ctx.force {
        check_force(ctx)?;
    }

    // Force-insurmountable: force may bridge an edge or a guard, never persist
    // an event the status schema auditor rejects. A migration actor
    // reconstructs history rather than acting, so it is exempt.
    if requires_review_ref(from, to)
        && !has_review_ref(ctx.review_ref.as_deref())
        && !is_migration_actor(ctx.actor.as_deref())
    {
        return Err(TransitionError::ReviewRefRequired { from, to });
    }

    if ctx.force {
        return Ok(());
    }

    run_guard(from, to, ctx)
}

/// Resolve `doing`-style input aliases and validate by lane names.
///
/// The typed [`validate_transition`] remains the canonical decision function;
/// this convenience only owns boundary parsing so an unknown lane is a typed
/// refusal rather than a panic.
pub fn validate_transition_names(
    from_lane: &str,
    to_lane: &str,
    ctx: &GuardContext,
) -> Result<(), TransitionError> {
    let from = Lane::parse_input(from_lane)?;
    let to = Lane::parse_input(to_lane)?;
    validate_transition(from, to, ctx)
}

fn check_force(ctx: &GuardContext) -> Result<(), TransitionError> {
    if !nonblank(ctx.actor.as_deref()) || !nonblank(ctx.reason.as_deref()) {
        return Err(TransitionError::ForceRequiresActorAndReason);
    }
    Ok(())
}

fn run_guard(from: Lane, to: Lane, ctx: &GuardContext) -> Result<(), TransitionError> {
    match from {
        Lane::Genesis => Ok(()),

        Lane::Uninitialized => Ok(()),

        Lane::Planned => {
            if to == Lane::Claimed {
                if !nonblank(ctx.actor.as_deref()) {
                    return Err(TransitionError::ActorIdentityRequired);
                }
                if ctx.dependency_ready == Some(false) {
                    return Err(TransitionError::DependenciesUnsatisfied {
                        from: Lane::Planned,
                        to: Lane::Claimed,
                    });
                }
            }
            Ok(())
        }

        Lane::Claimed => {
            if to == Lane::InProgress {
                if !nonblank(ctx.workspace_context.as_deref()) {
                    return Err(TransitionError::WorkspaceContextRequired);
                }
                if ctx.dependency_ready == Some(false) {
                    return Err(TransitionError::DependenciesUnsatisfied {
                        from: Lane::Claimed,
                        to: Lane::InProgress,
                    });
                }
            }
            Ok(())
        }

        Lane::InProgress => match to {
            Lane::ForReview => {
                if ctx.subtasks_complete != Some(true) {
                    return Err(TransitionError::CompletedSubtasksRequired);
                }
                if ctx.implementation_evidence_present != Some(true) {
                    return Err(TransitionError::ImplementationEvidenceRequired);
                }
                Ok(())
            }
            Lane::Approved => check_reviewer_approval(ctx),
            Lane::Planned => {
                if !nonblank(ctx.reason.as_deref()) {
                    return Err(TransitionError::TransitionReasonRequired);
                }
                Ok(())
            }
            _ => Ok(()),
        },

        Lane::ForReview => {
            if to == Lane::InReview && !nonblank(ctx.actor.as_deref()) {
                return Err(TransitionError::ActorIdentityRequired);
            }
            // for_review -> in_review is hard allow-only after actor presence;
            // reviewer collision belongs to the in_review re-claim probe.
            Ok(())
        }

        Lane::InReview => {
            // Every outbound in_review edge requires a ReviewResult.
            check_review_result(ctx)?;
            if matches!(to, Lane::Approved | Lane::Done) {
                check_in_review_approval(ctx)?;
            }
            check_review_result_consistency(ctx)
        }

        Lane::Approved => match to {
            Lane::Done => check_reviewer_approval(ctx),
            Lane::InProgress | Lane::Planned => {
                if !nonblank(ctx.review_ref.as_deref()) {
                    return Err(TransitionError::ReviewRefRequiredForRewrite);
                }
                Ok(())
            }
            _ => Ok(()),
        },

        Lane::Done | Lane::Blocked | Lane::Canceled => Ok(()),
    }
}

fn check_reviewer_approval(ctx: &GuardContext) -> Result<(), TransitionError> {
    let Some(evidence) = ctx.evidence.as_ref() else {
        return Err(TransitionError::ReviewerApprovalRequired);
    };
    let Some(review) = evidence.review.as_ref() else {
        return Err(TransitionError::ReviewerApprovalRequired);
    };
    if !nonblank(Some(&review.reviewer))
        || !nonblank(Some(&review.reference))
        || review.verdict != "approved"
    {
        return Err(TransitionError::ReviewerApprovalRequired);
    }
    Ok(())
}

fn check_in_review_approval(ctx: &GuardContext) -> Result<(), TransitionError> {
    if ctx.evidence.is_some() {
        return check_reviewer_approval(ctx);
    }
    match ctx.review_result.as_ref() {
        Some(result)
            if nonblank(Some(&result.reviewer))
                && result.verdict == "approved"
                && nonblank(Some(&result.reference)) =>
        {
            Ok(())
        }
        _ => Err(TransitionError::ReviewerApprovalRequired),
    }
}

fn check_review_result(ctx: &GuardContext) -> Result<(), TransitionError> {
    let Some(result) = ctx.review_result.as_ref() else {
        return Err(TransitionError::ReviewResultRequired);
    };
    if !nonblank(Some(&result.reviewer)) {
        return Err(TransitionError::ReviewResultReviewerRequired);
    }
    if !nonblank(Some(&result.verdict)) {
        return Err(TransitionError::ReviewResultVerdictRequired);
    }
    if !nonblank(Some(&result.reference)) {
        return Err(TransitionError::ReviewResultReferenceRequired);
    }
    Ok(())
}

fn check_review_result_consistency(ctx: &GuardContext) -> Result<(), TransitionError> {
    let Some(result) = ctx.review_result.as_ref() else {
        return Err(TransitionError::ReviewResultRequired);
    };

    if let Some(review_ref) = ctx.review_ref.as_deref() {
        if review_ref != result.reference {
            return Err(TransitionError::ReviewRefMismatch);
        }
    }

    let Some(evidence) = ctx.evidence.as_ref() else {
        return Ok(());
    };
    let actual = evidence.review.as_ref().map(|review| {
        (
            review.reviewer.as_str(),
            review.verdict.as_str(),
            review.reference.as_str(),
        )
    });
    let expected = Some((
        result.reviewer.as_str(),
        result.verdict.as_str(),
        result.reference.as_str(),
    ));
    if actual != expected {
        return Err(TransitionError::ReviewEvidenceMismatch);
    }
    Ok(())
}

fn nonblank(value: Option<&str>) -> bool {
    value.is_some_and(|text| !text.trim().is_empty())
}
