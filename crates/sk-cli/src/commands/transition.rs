//! Pure work-package lane FSM validation.
//!
//! No event is appended and no `status.json` is written. The command answers
//! only whether the transition is admitted under the supplied guard context.

use serde_json::json;
use sk_status::{
    DoneEvidence, GuardContext, Lane, ReviewApproval, ReviewResult, validate_transition,
};

use crate::cli::TransitionCheck;
use crate::output::{CliError, CommandData};

/// Validate one transition and report the FSM decision.
pub fn check(args: &TransitionCheck) -> Result<CommandData, CliError> {
    let from = Lane::parse_input(&args.from).map_err(|error| CliError::usage(error.to_string()))?;
    let to = Lane::parse_input(&args.to).map_err(|error| CliError::usage(error.to_string()))?;

    let evidence = evidence(args)?;
    let review_result = review_result(args)?;
    let context = GuardContext {
        actor: args.actor.clone(),
        workspace_context: args.workspace_context.clone(),
        subtasks_complete: args.subtasks_complete,
        implementation_evidence_present: args.implementation_evidence_present,
        reason: args.reason.clone(),
        review_ref: args.review_ref.clone(),
        evidence,
        force: args.force,
        review_result,
        current_actor: args.current_actor.clone(),
        dependency_ready: args.dependency_ready,
    };

    match validate_transition(from, to, &context) {
        Ok(()) => Ok(CommandData::success(json!({
            "from": from.as_str(),
            "to": to.as_str(),
            "input_from": &args.from,
            "input_to": &args.to,
            "admitted": true,
            "persisted": false,
            "event_written": false,
            "force": args.force,
        }))),
        Err(error) => {
            let message = error.to_string();
            let details = json!({
                "from": from.as_str(),
                "to": to.as_str(),
                "input_from": &args.from,
                "input_to": &args.to,
                "admitted": false,
                "persisted": false,
                "event_written": false,
                "force": args.force,
                "refusal": &message,
            });
            Err(CliError::domain(message).with_details(details))
        }
    }
}

fn evidence(args: &TransitionCheck) -> Result<Option<DoneEvidence>, CliError> {
    let parts = [
        args.evidence_reviewer.as_ref(),
        args.evidence_verdict.as_ref(),
        args.evidence_reference.as_ref(),
    ];
    let supplied = parts.iter().filter(|part| part.is_some()).count();
    if supplied == 0 {
        return Ok(None);
    }
    if supplied != parts.len() {
        return Err(CliError::usage(
            "--evidence-reviewer, --evidence-verdict and --evidence-reference must be supplied together",
        ));
    }

    Ok(Some(DoneEvidence {
        review: Some(ReviewApproval {
            reviewer: args.evidence_reviewer.clone().unwrap_or_default(),
            verdict: args.evidence_verdict.clone().unwrap_or_default(),
            reference: args.evidence_reference.clone().unwrap_or_default(),
        }),
    }))
}

fn review_result(args: &TransitionCheck) -> Result<Option<ReviewResult>, CliError> {
    let parts = [
        args.review_result_reviewer.as_ref(),
        args.review_result_verdict.as_ref(),
        args.review_result_reference.as_ref(),
        args.review_result_feedback_path.as_ref(),
    ];
    let supplied = parts.iter().filter(|part| part.is_some()).count();
    if supplied == 0 {
        return Ok(None);
    }
    if args.review_result_reviewer.is_none()
        || args.review_result_verdict.is_none()
        || args.review_result_reference.is_none()
    {
        return Err(CliError::usage(
            "--review-result-reviewer, --review-result-verdict and --review-result-reference are required together",
        ));
    }

    Ok(Some(ReviewResult {
        reviewer: args.review_result_reviewer.clone().unwrap_or_default(),
        verdict: args.review_result_verdict.clone().unwrap_or_default(),
        reference: args.review_result_reference.clone().unwrap_or_default(),
        feedback_path: args.review_result_feedback_path.clone(),
    }))
}
