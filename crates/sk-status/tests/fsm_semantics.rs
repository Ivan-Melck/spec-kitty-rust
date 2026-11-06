//! Genesis, uninitialized, force, alias, and guard-message semantics.

mod support;

use sk_status::{
    DoneEvidence, GuardContext, Lane, ReviewApproval, ReviewResult, is_migration_actor,
    validate_transition, validate_transition_names,
};

const FORCE_REQUIRES: &str = "Force transitions require actor and reason";
const REVIEW_RESULT_REQUIRED: &str =
    "Transition from in_review requires review_result (structured review outcome)";
const REVIEWER_APPROVAL_REQUIRED: &str =
    "Transition to approved/done requires evidence (reviewer identity and approval reference)";

fn ok(from: Lane, to: Lane, ctx: &GuardContext) -> bool {
    validate_transition(from, to, ctx).is_ok()
}

fn error(from: Lane, to: Lane, ctx: &GuardContext) -> String {
    validate_transition(from, to, ctx)
        .expect_err("transition should be refused")
        .to_string()
}

fn illegal(from: Lane, to: Lane) -> String {
    format!("Illegal transition: {from} -> {to}")
}

fn force_full() -> GuardContext {
    support::context("force_full")
}

fn actor() -> GuardContext {
    support::context("actor")
}

fn reviewed() -> GuardContext {
    support::context("review_result")
}

fn approved_review() -> ReviewResult {
    ReviewResult {
        reviewer: "r".to_owned(),
        verdict: "approved".to_owned(),
        reference: "ref".to_owned(),
        feedback_path: None,
    }
}

#[test]
fn lane_enum_is_exhaustive_and_metadata_is_complete() {
    assert_eq!(Lane::ALL.len(), 11);
    assert_eq!(
        Lane::CANONICAL.len(),
        9,
        "the nine displayed lifecycle lanes"
    );
    assert_eq!(Lane::NON_DISPLAY.len(), 2);
    assert_eq!(Lane::TERMINAL.len(), 2);

    let expected = [
        (Lane::Genesis, "genesis", "Planned", "not_started"),
        (
            Lane::Uninitialized,
            "uninitialized",
            "Planned",
            "not_started",
        ),
        (Lane::Planned, "planned", "Planned", "not_started"),
        (Lane::Claimed, "claimed", "In Progress", "in_flight"),
        (Lane::InProgress, "in_progress", "In Progress", "in_flight"),
        (Lane::ForReview, "for_review", "Review", "review"),
        (Lane::InReview, "in_review", "In Progress", "review"),
        (Lane::Approved, "approved", "Approved", "review"),
        (Lane::Done, "done", "Done", "terminal"),
        (Lane::Blocked, "blocked", "Blocked", "in_flight"),
        (Lane::Canceled, "canceled", "Canceled", "terminal"),
    ];
    assert_eq!(Lane::ALL.len(), expected.len());
    for (lane, value, category, bucket) in expected {
        assert_eq!(lane.as_str(), value);
        assert_eq!(lane.display_category(), category);
        assert_eq!(lane.progress_bucket(), bucket);
        assert_eq!(Lane::parse_canonical(value), Ok(lane));
    }

    // The derived edge projection still totals 27 lifecycle edges + 2 genesis
    // seeds, exactly as the Python `ALLOWED_TRANSITIONS` count.
    let edges: usize = Lane::ALL
        .iter()
        .map(|lane| lane.allowed_targets().len())
        .sum();
    assert_eq!(edges, 29);
    assert_eq!(
        Lane::Genesis.allowed_targets(),
        &[Lane::Planned, Lane::Canceled]
    );
    assert!(Lane::Uninitialized.allowed_targets().is_empty());
    assert!(Lane::Done.allowed_targets().is_empty());
    assert!(Lane::Canceled.allowed_targets().is_empty());

    assert!(Lane::Done.is_terminal());
    assert!(Lane::Canceled.is_terminal());
    assert!(Lane::Blocked.is_blocked());
    for lane in [
        Lane::Planned,
        Lane::Claimed,
        Lane::InProgress,
        Lane::ForReview,
        Lane::InReview,
        Lane::Approved,
    ] {
        assert!(lane.is_run_affecting(), "{lane} should be run-affecting");
        assert!(!lane.is_terminal());
    }
    for lane in [
        Lane::Genesis,
        Lane::Uninitialized,
        Lane::Done,
        Lane::Blocked,
        Lane::Canceled,
    ] {
        assert!(
            !lane.is_run_affecting(),
            "{lane} should not be run-affecting"
        );
    }
}

#[test]
fn doing_is_an_input_alias_only() {
    for raw in ["doing", "Doing", "DOING", "  doing  "] {
        assert_eq!(Lane::parse_input(raw), Ok(Lane::InProgress), "{raw:?}");
    }
    assert!(
        Lane::parse_canonical("doing").is_err(),
        "doing is not stored"
    );
    assert_eq!(Lane::parse_input("  planned  "), Ok(Lane::Planned));
    assert_eq!(Lane::parse_input("PLANNED"), Ok(Lane::Planned));
    assert_eq!(
        Lane::parse_input("bogus").unwrap_err().to_string(),
        "Unknown lane: bogus"
    );
    assert_eq!(
        validate_transition_names("bogus", "planned", &GuardContext::default())
            .unwrap_err()
            .to_string(),
        "Unknown lane: bogus"
    );
    assert_eq!(
        validate_transition_names("planned", "bogus", &GuardContext::default())
            .unwrap_err()
            .to_string(),
        "Unknown lane: bogus"
    );
}

#[test]
fn genesis_edges_and_genesis_target_refusal_cover_all_357_baseline_rows() {
    let mut checked = 0usize;
    for ctx_name in support::CONTEXT_NAMES {
        let ctx = support::context(ctx_name);
        for from in Lane::ALL {
            for to in Lane::ALL {
                if from != Lane::Genesis && to != Lane::Genesis {
                    continue;
                }
                checked += 1;

                let (expected_ok, expected_err) = if to == Lane::Genesis {
                    // Genesis is never a persisted/display target, and force
                    // cannot bypass this refusal.
                    (false, Some(illegal(from, to)))
                } else if from == Lane::Genesis {
                    match ctx_name {
                        "force_full" => (true, None),
                        "force_no_actor" | "force_no_reason" => {
                            (false, Some(FORCE_REQUIRES.to_owned()))
                        }
                        _ if matches!(to, Lane::Planned | Lane::Canceled) => (true, None),
                        _ => (false, Some(illegal(from, to))),
                    }
                } else {
                    unreachable!("filtered to genesis-involving pairs")
                };

                let actual = validate_transition(from, to, &ctx);
                assert_eq!(
                    (actual.is_ok(), actual.err().map(|error| error.to_string())),
                    (expected_ok, expected_err),
                    "{from}->{to}[{ctx_name}]"
                );
            }
        }
    }
    assert_eq!(checked, 357, "all genesis-involving baseline rows");
}

#[test]
fn uninitialized_is_non_transitionable_except_force_override() {
    let mut checked = 0usize;
    for ctx_name in support::CONTEXT_NAMES {
        let ctx = support::context(ctx_name);
        for from in Lane::ALL {
            for to in Lane::ALL {
                if from != Lane::Uninitialized && to != Lane::Uninitialized {
                    continue;
                }
                if from == Lane::Genesis || to == Lane::Genesis {
                    continue;
                }
                checked += 1;
                let (expected_ok, expected_err) = match ctx_name {
                    "force_full" => (true, None),
                    "force_no_actor" | "force_no_reason" => {
                        (false, Some(FORCE_REQUIRES.to_owned()))
                    }
                    _ => (false, Some(illegal(from, to))),
                };
                let actual = validate_transition(from, to, &ctx);
                assert_eq!(
                    (actual.is_ok(), actual.err().map(|error| error.to_string())),
                    (expected_ok, expected_err),
                    "{from}->{to}[{ctx_name}]"
                );
            }
        }
    }
    // 21 lane pairs involve `uninitialized` (11 + 11 - 1), across 17 contexts,
    // minus the two genesis-involving pairs (`genesis -> uninitialized` and
    // `uninitialized -> genesis`) that the genesis test owns.
    assert_eq!(checked, 19 * 17, "uninitialized-involving pairs");
}

#[test]
fn force_cannot_bypass_genesis_target_refusal() {
    for from in Lane::ALL {
        let error = validate_transition(from, Lane::Genesis, &force_full())
            .expect_err("genesis target must be refused even under force");
        assert_eq!(error.to_string(), illegal(from, Lane::Genesis));
    }
}

#[test]
fn forced_review_rewind_requires_review_ref_unless_migration_actor() {
    let expected = "Transition for_review -> in_progress requires review_ref: a forced rewind out \
                    of review must cite the review feedback it is rewinding (pass `--review-ref <ref>`).";

    assert_eq!(
        error(Lane::ForReview, Lane::InProgress, &force_full()),
        expected
    );
    assert_eq!(
        error(
            Lane::ForReview,
            Lane::InProgress,
            &GuardContext {
                review_ref: Some("   ".to_owned()),
                ..force_full()
            }
        ),
        expected
    );
    assert!(ok(
        Lane::ForReview,
        Lane::InProgress,
        &GuardContext {
            review_ref: Some("ref".to_owned()),
            ..force_full()
        }
    ));
    assert!(ok(
        Lane::ForReview,
        Lane::InProgress,
        &GuardContext {
            force: true,
            actor: Some("migration:backfill_runtime_state".to_owned()),
            reason: Some("reconstruct".to_owned()),
            ..GuardContext::default()
        }
    ));
    // The migration exemption does not grant the missing structural edge
    // without force; it only exempts the review_ref requirement.
    assert_eq!(
        error(
            Lane::ForReview,
            Lane::InProgress,
            &GuardContext {
                actor: Some("migration:backfill_runtime_state".to_owned()),
                reason: Some("reconstruct".to_owned()),
                ..GuardContext::default()
            }
        ),
        illegal(Lane::ForReview, Lane::InProgress)
    );
}

#[test]
fn migration_actor_detection_is_exact_and_case_sensitive() {
    assert!(is_migration_actor(Some("migration:backfill")));
    assert!(is_migration_actor(Some("migration:")));
    assert!(!is_migration_actor(Some("migration")));
    assert!(!is_migration_actor(Some("MIGRATION:backfill")));
    assert!(!is_migration_actor(Some(" migration:backfill")));
    assert!(!is_migration_actor(Some("agent:migration:x")));
    assert!(!is_migration_actor(None));
}

#[test]
fn every_outbound_in_review_edge_requires_review_result_without_force() {
    let outbound = [
        Lane::Approved,
        Lane::Done,
        Lane::InProgress,
        Lane::Planned,
        Lane::Blocked,
        Lane::Canceled,
    ];
    for ctx_name in support::CONTEXT_NAMES {
        if ctx_name.starts_with("force")
            || matches!(ctx_name, "review_result" | "rr_empty_reviewer")
        {
            continue;
        }
        let ctx = support::context(ctx_name);
        for target in outbound {
            assert_eq!(
                error(Lane::InReview, target, &ctx),
                REVIEW_RESULT_REQUIRED,
                "in_review -> {target}[{ctx_name}]"
            );
        }
    }

    // A complete ReviewResult authorises every outbound edge.
    for target in outbound {
        assert!(
            ok(Lane::InReview, target, &reviewed()),
            "in_review -> {target} should accept a complete review result"
        );
    }

    // Force is the explicit FSM override the baseline records.
    for target in outbound {
        assert!(
            ok(Lane::InReview, target, &force_full()),
            "force_full in_review -> {target}"
        );
    }
}

#[test]
fn review_result_field_refusals_are_exact() {
    for (review_result, expected) in [
        (
            ReviewResult {
                reviewer: String::new(),
                verdict: "approved".to_owned(),
                reference: "ref".to_owned(),
                feedback_path: None,
            },
            "Transition from in_review requires review_result with reviewer",
        ),
        (
            ReviewResult {
                reviewer: "   ".to_owned(),
                verdict: "approved".to_owned(),
                reference: "ref".to_owned(),
                feedback_path: None,
            },
            "Transition from in_review requires review_result with reviewer",
        ),
        (
            ReviewResult {
                reviewer: "r".to_owned(),
                verdict: String::new(),
                reference: "ref".to_owned(),
                feedback_path: None,
            },
            "Transition from in_review requires review_result with verdict",
        ),
        (
            ReviewResult {
                reviewer: "r".to_owned(),
                verdict: "approved".to_owned(),
                reference: "   ".to_owned(),
                feedback_path: None,
            },
            "Transition from in_review requires review_result with reference",
        ),
    ] {
        let ctx = GuardContext {
            actor: Some("a".to_owned()),
            review_result: Some(review_result),
            ..GuardContext::default()
        };
        assert_eq!(error(Lane::InReview, Lane::Planned, &ctx), expected);
    }
}

#[test]
fn review_result_consistency_refusals_are_exact() {
    let mismatched_ref = GuardContext {
        actor: Some("a".to_owned()),
        review_ref: Some("other".to_owned()),
        review_result: Some(approved_review()),
        ..GuardContext::default()
    };
    assert_eq!(
        error(Lane::InReview, Lane::Planned, &mismatched_ref),
        "review_ref must match review_result.reference"
    );

    let mismatched_evidence = GuardContext {
        actor: Some("a".to_owned()),
        review_result: Some(approved_review()),
        evidence: Some(DoneEvidence {
            review: Some(ReviewApproval {
                reviewer: "someone-else".to_owned(),
                verdict: "approved".to_owned(),
                reference: "ref".to_owned(),
            }),
        }),
        ..GuardContext::default()
    };
    assert_eq!(
        error(Lane::InReview, Lane::Planned, &mismatched_evidence),
        "Review evidence must match review_result"
    );

    let matching_evidence = GuardContext {
        actor: Some("a".to_owned()),
        review_result: Some(approved_review()),
        evidence: Some(DoneEvidence {
            review: Some(ReviewApproval {
                reviewer: "r".to_owned(),
                verdict: "approved".to_owned(),
                reference: "ref".to_owned(),
            }),
        }),
        ..GuardContext::default()
    };
    assert!(ok(Lane::InReview, Lane::Planned, &matching_evidence));
    assert!(ok(Lane::InReview, Lane::Approved, &matching_evidence));

    let changes_requested = GuardContext {
        actor: Some("a".to_owned()),
        review_result: Some(ReviewResult {
            reviewer: "r".to_owned(),
            verdict: "changes_requested".to_owned(),
            reference: "ref".to_owned(),
            feedback_path: None,
        }),
        ..GuardContext::default()
    };
    assert_eq!(
        error(Lane::InReview, Lane::Approved, &changes_requested),
        REVIEWER_APPROVAL_REQUIRED
    );
    assert!(ok(Lane::InReview, Lane::Planned, &changes_requested));
}

#[test]
fn approval_and_reason_guards_use_the_parity_messages() {
    assert_eq!(
        error(Lane::Approved, Lane::Done, &actor()),
        REVIEWER_APPROVAL_REQUIRED
    );
    assert_eq!(
        error(Lane::InProgress, Lane::Approved, &support::context("empty")),
        REVIEWER_APPROVAL_REQUIRED
    );
    assert!(ok(
        Lane::Approved,
        Lane::Done,
        &support::context("done_evidence")
    ));
    assert!(ok(
        Lane::InProgress,
        Lane::Approved,
        &support::context("done_evidence")
    ));

    for target in [Lane::InProgress, Lane::Planned] {
        assert_eq!(
            error(Lane::Approved, target, &actor()),
            "Transition requires review_ref (review feedback reference)"
        );
        assert!(ok(Lane::Approved, target, &support::context("review_ref")));
        assert!(ok(Lane::Approved, target, &force_full()));
    }

    assert_eq!(
        error(Lane::InProgress, Lane::Planned, &actor()),
        "Transition in_progress -> planned requires reason"
    );
    assert!(ok(
        Lane::InProgress,
        Lane::Planned,
        &support::context("reason")
    ));
}

#[test]
fn in_progress_to_for_review_requires_both_evidence_flags() {
    assert_eq!(
        error(Lane::InProgress, Lane::ForReview, &actor()),
        "Transition in_progress -> for_review requires completed subtasks or force with reason"
    );
    assert_eq!(
        error(
            Lane::InProgress,
            Lane::ForReview,
            &support::context("subtasks_only")
        ),
        "Transition in_progress -> for_review requires implementation evidence or force with reason"
    );
    assert_eq!(
        error(
            Lane::InProgress,
            Lane::ForReview,
            &support::context("evidence_only")
        ),
        "Transition in_progress -> for_review requires completed subtasks or force with reason"
    );
    assert!(ok(
        Lane::InProgress,
        Lane::ForReview,
        &support::context("subtasks_evidence")
    ));
    assert!(ok(Lane::InProgress, Lane::ForReview, &force_full()));
}

#[test]
fn actor_workspace_and_dependency_guards_are_exact() {
    assert_eq!(
        error(Lane::Planned, Lane::Claimed, &support::context("empty")),
        "Transition requires actor identity"
    );
    assert_eq!(
        error(
            Lane::Planned,
            Lane::Claimed,
            &support::context("actor_empty")
        ),
        "Transition requires actor identity"
    );
    assert!(ok(Lane::Planned, Lane::Claimed, &actor()));

    assert_eq!(
        error(Lane::Claimed, Lane::InProgress, &actor()),
        "Transition claimed -> in_progress requires workspace context"
    );
    assert_eq!(
        error(
            Lane::Claimed,
            Lane::InProgress,
            &GuardContext {
                actor: Some("a".to_owned()),
                workspace_context: Some("   ".to_owned()),
                ..GuardContext::default()
            }
        ),
        "Transition claimed -> in_progress requires workspace context"
    );
    assert!(ok(Lane::Claimed, Lane::InProgress, &support::context("ws")));

    assert_eq!(
        error(
            Lane::Planned,
            Lane::Claimed,
            &GuardContext {
                actor: Some("a".to_owned()),
                dependency_ready: Some(false),
                ..GuardContext::default()
            }
        ),
        "Transition planned -> claimed blocked: unsatisfied dependencies (force with reason to override)"
    );
    // None is fail-open (no verdict supplied), never a refusal.
    assert!(ok(
        Lane::Planned,
        Lane::Claimed,
        &GuardContext {
            actor: Some("a".to_owned()),
            dependency_ready: None,
            ..GuardContext::default()
        }
    ));
    assert!(ok(
        Lane::Planned,
        Lane::Claimed,
        &GuardContext {
            actor: Some("a".to_owned()),
            dependency_ready: Some(true),
            ..GuardContext::default()
        }
    ));
    assert!(ok(Lane::Planned, Lane::Claimed, &force_full()));

    assert_eq!(
        error(
            Lane::Claimed,
            Lane::InProgress,
            &GuardContext {
                actor: Some("a".to_owned()),
                workspace_context: Some("worktree:/tmp".to_owned()),
                dependency_ready: Some(false),
                ..GuardContext::default()
            }
        ),
        "Transition claimed -> in_progress blocked: unsatisfied dependencies (force with reason to override)"
    );
    assert!(ok(
        Lane::Claimed,
        Lane::InProgress,
        &GuardContext {
            workspace_context: Some("worktree:/tmp".to_owned()),
            dependency_ready: Some(false),
            ..force_full()
        }
    ));
}

#[test]
fn for_review_to_in_review_is_actor_presence_only() {
    assert_eq!(
        error(Lane::ForReview, Lane::InReview, &support::context("empty")),
        "Transition requires actor identity"
    );
    assert!(ok(
        Lane::ForReview,
        Lane::InReview,
        &support::context("conflict")
    ));
    assert!(ok(
        Lane::ForReview,
        Lane::InReview,
        &support::context("same_actor")
    ));
}

#[test]
fn force_requires_actor_and_reason_and_precedes_edge_refusal() {
    for ctx in [
        support::context("force_no_actor"),
        support::context("force_no_reason"),
        GuardContext {
            force: true,
            actor: Some("   ".to_owned()),
            reason: Some("reopen".to_owned()),
            ..GuardContext::default()
        },
        GuardContext {
            force: true,
            actor: Some("admin".to_owned()),
            reason: Some("   ".to_owned()),
            ..GuardContext::default()
        },
    ] {
        // Existing edge, guarded edge, and force-only edge all report the
        // force audit refusal first.
        for (from, to) in [
            (Lane::Planned, Lane::Claimed),
            (Lane::InProgress, Lane::ForReview),
            (Lane::ForReview, Lane::InProgress),
            (Lane::Planned, Lane::Approved),
        ] {
            assert_eq!(error(from, to, &ctx), FORCE_REQUIRES, "{from}->{to}");
        }
    }
}

#[test]
fn terminal_force_exit_covers_every_display_lane() {
    for terminal in [Lane::Done, Lane::Canceled] {
        for target in [
            Lane::Planned,
            Lane::Claimed,
            Lane::InProgress,
            Lane::ForReview,
            Lane::InReview,
            Lane::Approved,
            Lane::Blocked,
        ] {
            assert_eq!(
                error(terminal, target, &actor()),
                illegal(terminal, target),
                "unforced terminal exit"
            );
            assert!(
                ok(terminal, target, &force_full()),
                "force_full {terminal}->{target}"
            );
        }
    }
}
