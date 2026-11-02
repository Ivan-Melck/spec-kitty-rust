//! Shared fixtures reconstructed from the Python status tests.

use sk_status::{DoneEvidence, GuardContext, ReviewApproval, ReviewResult};

pub const CONTEXT_NAMES: [&str; 17] = [
    "empty",
    "actor",
    "actor_empty",
    "ws",
    "subtasks_evidence",
    "subtasks_only",
    "evidence_only",
    "reason",
    "review_ref",
    "done_evidence",
    "review_result",
    "rr_empty_reviewer",
    "conflict",
    "same_actor",
    "force_full",
    "force_no_actor",
    "force_no_reason",
];

/// The 17 named `GuardContext` variants, byte-for-byte in intent with
/// `tests/status/test_transitions.py::_parity_context_library`.
pub fn context(name: &str) -> GuardContext {
    match name {
        "empty" => GuardContext::default(),
        "actor" => GuardContext {
            actor: Some("agent-1".to_owned()),
            ..GuardContext::default()
        },
        "actor_empty" => GuardContext {
            actor: Some(String::new()),
            ..GuardContext::default()
        },
        "ws" => GuardContext {
            actor: Some("a".to_owned()),
            workspace_context: Some("worktree:/tmp".to_owned()),
            ..GuardContext::default()
        },
        "subtasks_evidence" => GuardContext {
            actor: Some("a".to_owned()),
            subtasks_complete: Some(true),
            implementation_evidence_present: Some(true),
            ..GuardContext::default()
        },
        "subtasks_only" => GuardContext {
            actor: Some("a".to_owned()),
            subtasks_complete: Some(true),
            ..GuardContext::default()
        },
        "evidence_only" => GuardContext {
            actor: Some("a".to_owned()),
            implementation_evidence_present: Some(true),
            ..GuardContext::default()
        },
        "reason" => GuardContext {
            actor: Some("a".to_owned()),
            reason: Some("r".to_owned()),
            ..GuardContext::default()
        },
        "review_ref" => GuardContext {
            actor: Some("a".to_owned()),
            review_ref: Some("ref".to_owned()),
            ..GuardContext::default()
        },
        "done_evidence" => GuardContext {
            actor: Some("a".to_owned()),
            evidence: Some(DoneEvidence {
                review: Some(ReviewApproval {
                    reviewer: "r".to_owned(),
                    verdict: "approved".to_owned(),
                    reference: "ref".to_owned(),
                }),
            }),
            ..GuardContext::default()
        },
        "review_result" => GuardContext {
            actor: Some("a".to_owned()),
            review_result: Some(ReviewResult {
                reviewer: "r".to_owned(),
                verdict: "approved".to_owned(),
                reference: "ref".to_owned(),
                feedback_path: None,
            }),
            ..GuardContext::default()
        },
        "rr_empty_reviewer" => GuardContext {
            actor: Some("a".to_owned()),
            review_result: Some(ReviewResult {
                reviewer: String::new(),
                verdict: "approved".to_owned(),
                reference: "ref".to_owned(),
                feedback_path: None,
            }),
            ..GuardContext::default()
        },
        "conflict" => GuardContext {
            actor: Some("rev-B".to_owned()),
            current_actor: Some("rev-A".to_owned()),
            ..GuardContext::default()
        },
        "same_actor" => GuardContext {
            actor: Some("rev-A".to_owned()),
            current_actor: Some("rev-A".to_owned()),
            ..GuardContext::default()
        },
        "force_full" => GuardContext {
            force: true,
            actor: Some("admin".to_owned()),
            reason: Some("reopen".to_owned()),
            ..GuardContext::default()
        },
        "force_no_actor" => GuardContext {
            force: true,
            reason: Some("reopen".to_owned()),
            ..GuardContext::default()
        },
        "force_no_reason" => GuardContext {
            force: true,
            actor: Some("admin".to_owned()),
            ..GuardContext::default()
        },
        other => panic!("unknown parity context {other:?}"),
    }
}
