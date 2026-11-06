//! Source-first policy tests for `sk_status::acceptance::source`.
//!
//! Every expected string/ordering below was observed from the PINNED Source
//! (`9aed3a8012cb5eb22c0128b14d525039f95fea7a`, CPython 3.11.15) by executing
//! `specify_cli.acceptance.collect_feature_summary` over fresh Git fixtures; the
//! task report records the oracle receipts. The tests therefore assert actual
//! Source causes, not a conservative Native approximation:
//!
//! * there is NO owned-file/attribution gate anywhere in Source acceptance, so a
//!   Source-valid mission with no `owned_files` declaration is accepted;
//! * `strict_metadata` (`--lenient`) relaxes EXACTLY the work-package metadata
//!   issues and the mission path conventions;
//! * a malformed/absent `lanes.json` are distinct failures that both block;
//! * a deferred negative invariant is `pass_pending_consolidation` and does NOT
//!   block acceptance, while a pending verdict does.

use sk_status::acceptance::AcceptanceMatrix;
use sk_status::acceptance::source::{
    AcceptanceDiagnostic, ClarificationDecisionStatus, ClarificationScan, LanesManifestFacts,
    MatrixEvidence, MetaTargetBranch, PATH_CONVENTIONS_NOT_SATISFIED,
    PLANNING_ARTIFACT_ONLY_SKIP_REASON, PathConventionFacts, SourceAcceptanceFacts,
    SourceAcceptanceOptions, SourceWpFacts, evaluate_source_acceptance,
};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn wp(lane: &str) -> SourceWpFacts {
    SourceWpFacts {
        wp_id: "WP01".to_owned(),
        title: "\"WP01 probe\"".to_owned(),
        path: "kitty-specs/001-m/tasks/WP01.md".to_owned(),
        lane: Some(lane.to_owned()),
        agent: Some("probe-agent".to_owned()),
        implementer_of_record: Some("probe-agent".to_owned()),
        assignee: Some("probe-agent".to_owned()),
        shell_pid: Some("4242".to_owned()),
        has_operator_provenance: false,
        cancellation_reason: None,
        cancellation_actor: None,
        cancellation_at: None,
    }
}

fn facts(lane: &str) -> SourceAcceptanceFacts {
    SourceAcceptanceFacts {
        mission_slug: "001-m".to_owned(),
        mission_number: Some("1".to_owned()),
        mission_type: Some("software-dev".to_owned()),
        repo_root: "/repo".to_owned(),
        feature_dir: "/repo/kitty-specs/001-m".to_owned(),
        tasks_dir: "/repo/kitty-specs/001-m/tasks".to_owned(),
        worktree_root: "/repo".to_owned(),
        primary_repo_root: "/repo".to_owned(),
        branch: Some("main".to_owned()),
        lanes_manifest: LanesManifestFacts::Readable {
            target_branch: "main".to_owned(),
            mission_branch: "kitty/mission-001-m".to_owned(),
            mission_has_code: true,
        },
        meta_target_branch: MetaTargetBranch::Declared("main".to_owned()),
        work_packages: vec![wp(lane)],
        status_events_present: true,
        status_snapshot_present: true,
        tasks_md: Some("- [x] T001 done\n".to_owned()),
        clarifications: Vec::new(),
        decision_statuses: Default::default(),
        decision_statuses_available: true,
        missing_required_artifacts: Vec::new(),
        missing_optional_artifacts: Vec::new(),
        path_conventions: PathConventionFacts::NotDeclared,
        git_dirty: Vec::new(),
        matrix: MatrixEvidence::Judged {
            matrix: passing_matrix(),
            directory: Some("/repo/kitty-specs/001-m".to_owned()),
            negative_invariants_executed: true,
        },
    }
}

fn passing_matrix() -> AcceptanceMatrix {
    AcceptanceMatrix::from_json(&passing_matrix_json())
}

fn passing_matrix_json() -> serde_json::Map<String, serde_json::Value> {
    let value = serde_json::json!({
        "mission_slug": "001-m",
        "criteria": [{
            "criterion_id": "FR-001",
            "description": "FR-001 is satisfied and verified",
            "proof_type": "code_review",
            "pass_fail": "pass",
            "evidence": "WP01:review:WP01",
            "verified_by": "probe-reviewer",
            "verified_at": "2026-01-01T00:00:00Z"
        }],
        "negative_invariants": []
    });
    value.as_object().expect("object").clone()
}

fn strict(facts: &SourceAcceptanceFacts) -> sk_status::acceptance::source::SourceAcceptanceSummary {
    evaluate_source_acceptance(facts, SourceAcceptanceOptions { lenient: false })
}

fn lenient(
    facts: &SourceAcceptanceFacts,
) -> sk_status::acceptance::source::SourceAcceptanceSummary {
    evaluate_source_acceptance(facts, SourceAcceptanceOptions { lenient: true })
}

// ---------------------------------------------------------------------------
// 1. The invented owned-file gate is gone from the Source view
// ---------------------------------------------------------------------------

#[test]
fn strict_default_accepts_source_valid_metadata_without_any_owned_files_declaration() {
    // Source receipt S1: strict default, approved WP with the claim triple, a
    // passing matrix, and no `owned_files` declaration anywhere -> ok.
    let summary = strict(&facts("approved"));
    assert!(summary.ok(), "outstanding: {:?}", summary.outstanding());
    assert!(summary.all_done());
    assert!(summary.failed_checks().is_empty());
    assert!(summary.metadata_issues.is_empty());
    assert!(summary.path_violations.is_empty());
    assert_eq!(summary.lane("approved"), ["WP01".to_owned()]);
}

#[test]
fn the_source_json_payload_has_no_owned_file_or_attribution_key() {
    let payload = strict(&facts("approved")).to_json();
    let rendered = serde_json::to_string(&payload).expect("json");
    assert!(!rendered.contains("owned_file"), "{rendered}");
    assert!(!rendered.contains("attribution"), "{rendered}");
    // Source's own key set, including the fields the invented adapter omitted.
    let object = payload.as_object().expect("object");
    for key in [
        "mission_slug",
        "mission_number",
        "mission_type",
        "branch",
        "lanes",
        "work_packages",
        "metadata_issues",
        "activity_issues",
        "unchecked_tasks",
        "needs_clarification",
        "missing_artifacts",
        "optional_missing",
        "git_dirty",
        "path_violations",
        "warnings",
        "failed_checks",
        "skipped_checks",
        "blocked_checks",
        "recommended_fix_order",
        "canceled_wps",
        "all_done",
        "ok",
    ] {
        assert!(object.contains_key(key), "missing key {key}");
    }
}

#[test]
fn work_package_rows_carry_source_metadata_and_stripped_title() {
    let summary = strict(&facts("approved"));
    let row = &summary.work_packages[0];
    assert_eq!(row.title, "WP01 probe");
    assert_eq!(row.lane, "approved");
    assert_eq!(row.latest_lane.as_deref(), Some("approved"));
    assert!(row.has_lane_entry);
    assert_eq!(
        row.metadata.get("shell_pid").cloned().flatten().as_deref(),
        Some("4242")
    );
    assert_eq!(
        row.metadata.get("agent").cloned().flatten().as_deref(),
        Some("probe-agent")
    );
}

// ---------------------------------------------------------------------------
// 2. Metadata gate: strict-only, active lanes only, Source's exact strings
// ---------------------------------------------------------------------------

#[test]
fn strict_metadata_requires_the_claim_triple_only_in_active_lanes() {
    // Source receipt S2: an in_progress WP with no runtime metadata yields
    // exactly these three messages, in this order.
    let mut input = facts("in_progress");
    input.work_packages[0].agent = None;
    input.work_packages[0].implementer_of_record = None;
    input.work_packages[0].assignee = None;
    input.work_packages[0].shell_pid = None;
    let summary = strict(&input);
    assert_eq!(
        summary.metadata_issues,
        vec![
            "WP01: missing agent in canonical runtime state (never claimed; run `spec-kitty agent tasks move-task WP01 --to in_progress --agent <name>` to repair)".to_owned(),
            "WP01: missing assignee in canonical runtime state".to_owned(),
            "WP01: missing shell_pid in canonical runtime state".to_owned(),
        ]
    );

    // A terminal WP is exempt from assignee/shell_pid, but never from `agent`.
    let mut terminal = facts("approved");
    terminal.work_packages[0].assignee = None;
    terminal.work_packages[0].shell_pid = None;
    assert!(strict(&terminal).metadata_issues.is_empty());

    terminal.work_packages[0].agent = None;
    terminal.work_packages[0].implementer_of_record = None;
    assert_eq!(
        strict(&terminal).metadata_issues,
        vec![
            "WP01: missing agent in canonical runtime state (never claimed; run `spec-kitty agent tasks move-task WP01 --to approved --agent <name>` to repair)".to_owned()
        ]
    );
}

#[test]
fn the_implementer_of_record_slot_is_the_agent_fallback() {
    let mut input = facts("approved");
    input.work_packages[0].agent = None;
    let summary = strict(&input);
    assert!(summary.metadata_issues.is_empty());
    assert_eq!(
        summary.work_packages[0]
            .metadata
            .get("agent")
            .cloned()
            .flatten()
            .as_deref(),
        Some("probe-agent")
    );
}

// ---------------------------------------------------------------------------
// 3. Lenient relaxes exactly two checks
// ---------------------------------------------------------------------------

fn path_findings() -> PathConventionFacts {
    PathConventionFacts::Evaluated {
        satisfied: false,
        blocking_text: Some(
            "Path Convention Errors:\n  - Software Dev Kitty expects workspace path: src/ (not found)"
                .to_owned(),
        ),
        warning_text: Some(
            "Path Convention Warnings:\n  - Software Dev Kitty expects workspace path: src/ (not found)"
                .to_owned(),
        ),
        notice: None,
        dedup_tokens: Vec::new(),
    }
}

#[test]
fn lenient_relaxes_metadata_and_path_conventions_and_nothing_else() {
    let mut input = facts("in_progress");
    input.work_packages[0].agent = None;
    input.work_packages[0].implementer_of_record = None;
    input.work_packages[0].assignee = None;
    input.work_packages[0].shell_pid = None;
    input.path_conventions = path_findings();
    input.tasks_md = Some("- [ ] T001 not ticked\n".to_owned());
    input.git_dirty = vec!["M src/lib.rs".to_owned()];
    input.missing_required_artifacts = vec!["plan.md".to_owned()];

    let strict_summary = strict(&input);
    assert_eq!(strict_summary.metadata_issues.len(), 3);
    assert_eq!(strict_summary.path_violations.len(), 1);
    assert!(strict_summary.unchecked_tasks.len() == 1);
    assert!(strict_summary.git_dirty.len() == 1);
    assert!(strict_summary.missing_artifacts.len() == 1);

    let lenient_summary = lenient(&input);
    // Relaxed:
    assert!(lenient_summary.metadata_issues.is_empty());
    assert!(lenient_summary.path_violations.is_empty());
    assert!(
        lenient_summary
            .warnings
            .iter()
            .any(|warning| warning.starts_with("Path Convention Warnings:"))
    );
    // NOT relaxed — hard gates survive:
    assert!(!lenient_summary.ok());
    assert_eq!(lenient_summary.unchecked_tasks.len(), 1);
    assert_eq!(lenient_summary.git_dirty.len(), 1);
    assert_eq!(lenient_summary.missing_artifacts.len(), 1);
    assert!(
        !lenient_summary
            .outstanding()
            .iter()
            .any(|(bucket, _)| bucket == "lane_blockers")
            && lenient_summary
                .outstanding()
                .iter()
                .any(|(bucket, _)| bucket == "not_done"),
        "{:?}",
        lenient_summary.outstanding()
    );
}

#[test]
fn path_violations_fall_back_to_the_source_constant() {
    let mut input = facts("approved");
    input.path_conventions = PathConventionFacts::Evaluated {
        satisfied: false,
        blocking_text: None,
        warning_text: None,
        notice: None,
        dedup_tokens: Vec::new(),
    };
    assert_eq!(
        strict(&input).path_violations,
        vec![PATH_CONVENTIONS_NOT_SATISFIED.to_owned()]
    );
    assert_eq!(
        lenient(&input).warnings.last().map(String::as_str),
        Some(PATH_CONVENTIONS_NOT_SATISFIED)
    );
}

#[test]
fn undeclared_path_conventions_are_a_noop_but_unevaluated_ones_are_reported() {
    let mut undeclared = facts("approved");
    undeclared.path_conventions = PathConventionFacts::NotDeclared;
    assert!(strict(&undeclared).path_violations.is_empty());
    assert!(undedeclared_warnings(&undeclared).is_empty());

    let mut unevaluated = facts("approved");
    unevaluated.path_conventions = PathConventionFacts::Unevaluated {
        detail: "no canonical path-convention validator in this build".to_owned(),
    };
    let summary = strict(&unevaluated);
    // Never a silent pass: the missing authority is disclosed as a blocker.
    assert!(!summary.ok());
    assert!(
        summary
            .activity_issues
            .iter()
            .any(|issue| issue.contains("Mission path conventions cannot be evaluated")),
        "{:?}",
        summary.activity_issues
    );
}

fn undedeclared_warnings(facts: &SourceAcceptanceFacts) -> Vec<String> {
    strict(facts).warnings
}

#[test]
fn optional_artifacts_warning_drops_waived_tokens() {
    let mut input = facts("approved");
    input.missing_optional_artifacts = vec![
        "quickstart.md".to_owned(),
        "contracts".to_owned(),
        "research.md".to_owned(),
    ];
    input.path_conventions = PathConventionFacts::Evaluated {
        satisfied: true,
        blocking_text: None,
        warning_text: None,
        notice: None,
        dedup_tokens: vec!["contracts/".to_owned()],
    };
    let summary = strict(&input);
    assert_eq!(
        summary.warnings,
        vec!["Optional artifacts missing: quickstart.md, research.md".to_owned()]
    );
    assert_eq!(
        summary.optional_missing,
        vec!["quickstart.md", "research.md"]
    );
}

// ---------------------------------------------------------------------------
// 4. lanes.json: genuinely missing vs malformed are distinct, both blocking
// ---------------------------------------------------------------------------

#[test]
fn missing_and_corrupt_lanes_manifest_have_distinct_messages_and_both_block() {
    let mut missing = facts("approved");
    let missing_message = "lanes.json is required for /repo/kitty-specs/001-m. Run 'spec-kitty agent mission finalize-tasks' to compute execution lanes, or if the mission has already begun executing, run 'spec-kitty doctor mission-state --fix --mission <slug>' to rebuild lanes.json from the event log.";
    missing.lanes_manifest = LanesManifestFacts::Missing {
        message: missing_message.to_owned(),
    };
    let missing_summary = strict(&missing);
    assert!(!missing_summary.ok());
    assert_eq!(
        missing_summary.activity_issues,
        vec![missing_message.to_owned()]
    );
    assert_eq!(
        missing_summary.blocked_checks,
        vec![AcceptanceDiagnostic::new("lanes_manifest", missing_message)]
    );
    assert!(missing_summary.acceptance_matrix_gate_skip_reason.is_none());
    assert_eq!(
        missing_summary
            .skipped_checks
            .iter()
            .map(|check| check.check.as_str())
            .collect::<Vec<_>>(),
        vec![
            "acceptance_matrix_presence",
            "acceptance_matrix_evidence",
            "negative_invariants",
            "acceptance_matrix_verdict"
        ]
    );
    assert!(
        missing_summary
            .skipped_checks
            .iter()
            .all(|check| check.detail.ends_with("skipped: lanes.json is missing"))
    );

    let mut corrupt = facts("approved");
    let corrupt_message = "lanes.json at /repo/kitty-specs/001-m/lanes.json is corrupt or malformed: Expecting property name enclosed in double quotes";
    corrupt.lanes_manifest = LanesManifestFacts::Corrupt {
        message: corrupt_message.to_owned(),
    };
    let corrupt_summary = strict(&corrupt);
    assert!(!corrupt_summary.ok());
    assert_eq!(
        corrupt_summary.activity_issues,
        vec![corrupt_message.to_owned()]
    );
    assert!(corrupt_summary.skipped_checks.iter().all(|check| {
        check
            .detail
            .ends_with("skipped: lanes.json is corrupt or malformed")
    }));
    // The corruption is never re-reported as "missing".
    assert_ne!(
        corrupt_summary.activity_issues,
        missing_summary.activity_issues
    );
}

// ---------------------------------------------------------------------------
// 5. Branch gate and the planning-artifact-only skip
// ---------------------------------------------------------------------------

#[test]
fn code_missions_may_run_on_the_target_or_the_mission_branch() {
    let mut input = facts("approved");
    input.branch = Some("kitty/mission-001-m".to_owned());
    assert!(strict(&input).ok());

    input.branch = Some("feature-x".to_owned());
    let summary = strict(&input);
    assert!(!summary.ok());
    assert_eq!(
        summary.activity_issues,
        vec![
            "Acceptance must run on mission or target branch (kitty/mission-001-m, main), not feature-x"
                .to_owned()
        ]
    );
    assert_eq!(summary.blocked_checks[0].check, "mission_branch");
    assert!(summary.skipped_checks.iter().all(|check| {
        check
            .detail
            .ends_with("skipped: current branch is neither mission branch nor target branch")
    }));
    assert!(summary.acceptance_matrix_dir.is_none());
}

#[test]
fn a_detached_head_is_reported_as_such() {
    let mut input = facts("approved");
    input.branch = None;
    assert_eq!(
        strict(&input).activity_issues,
        vec![
            "Acceptance must run on mission or target branch (kitty/mission-001-m, main), not detached HEAD"
                .to_owned()
        ]
    );
}

#[test]
fn a_target_branch_mismatch_between_meta_and_lanes_blocks() {
    let mut input = facts("approved");
    input.meta_target_branch = MetaTargetBranch::Declared("develop".to_owned());
    let summary = strict(&input);
    assert!(!summary.ok());
    assert_eq!(
        summary.activity_issues,
        vec![
            "Acceptance target branch mismatch: meta.json targets develop, lanes.json targets main"
                .to_owned()
        ]
    );
    assert!(summary.skipped_checks.iter().all(|check| {
        check
            .detail
            .ends_with("skipped: meta.json target_branch does not match lanes.json target_branch")
    }));
}

#[test]
fn a_planning_artifact_only_mission_legitimately_skips_the_matrix() {
    let mut input = facts("approved");
    input.lanes_manifest = LanesManifestFacts::Readable {
        target_branch: "main".to_owned(),
        mission_branch: "kitty/mission-001-m".to_owned(),
        mission_has_code: false,
    };
    input.matrix = MatrixEvidence::NotRequired;
    let summary = strict(&input);
    assert!(summary.ok(), "{:?}", summary.outstanding());
    assert_eq!(
        summary.acceptance_matrix_gate_skip_reason.as_deref(),
        Some(PLANNING_ARTIFACT_ONLY_SKIP_REASON)
    );
    assert!(summary.acceptance_matrix_dir.is_none());
    assert_eq!(
        summary
            .skipped_checks
            .iter()
            .map(|check| check.check.as_str())
            .collect::<Vec<_>>(),
        vec![
            "acceptance_matrix_presence",
            "acceptance_matrix_evidence",
            "negative_invariants",
            "acceptance_matrix_verdict"
        ]
    );
    assert!(
        summary
            .skipped_checks
            .iter()
            .all(|check| check.detail.ends_with(
                "skipped: planning_artifact-only missions do not produce acceptance-matrix.json"
            ))
    );
}

// ---------------------------------------------------------------------------
// 6. Matrix: pending blocks, deferral does not, evidence errors are prefixed
// ---------------------------------------------------------------------------

#[test]
fn a_pending_verdict_blocks_and_names_the_matrix_fix() {
    let mut input = facts("approved");
    input.matrix = MatrixEvidence::Judged {
        matrix: AcceptanceMatrix::from_json(
            serde_json::json!({
                "mission_slug": "001-m",
                "criteria": [{
                    "criterion_id": "FR-001",
                    "description": "FR-001 is satisfied and verified",
                    "proof_type": "code_review",
                    "pass_fail": "pending"
                }],
                "negative_invariants": []
            })
            .as_object()
            .expect("object"),
        ),
        directory: Some("/repo/kitty-specs/001-m".to_owned()),
        negative_invariants_executed: true,
    };
    let summary = strict(&input);
    assert!(!summary.ok());
    assert_eq!(
        summary.activity_issues,
        vec![
            "Acceptance matrix verdict is 'pending' — criteria or invariants have not been verified"
                .to_owned()
        ]
    );
    assert!(
        summary.recommended_fix_order.contains(
            &"Resolve pending or failing acceptance matrix criteria and negative invariants."
                .to_owned()
        )
    );
}

#[test]
fn a_deferred_negative_invariant_does_not_block_acceptance_and_is_disclosed() {
    let mut input = facts("approved");
    input.matrix = MatrixEvidence::Judged {
        matrix: AcceptanceMatrix::from_json(
            serde_json::json!({
                "mission_slug": "001-m",
                "criteria": [{
                    "criterion_id": "FR-001",
                    "description": "FR-001 is satisfied and verified",
                    "proof_type": "code_review",
                    "pass_fail": "pass",
                    "evidence": "WP01:review:WP01"
                }],
                "negative_invariants": [{
                    "invariant_id": "NI-001",
                    "description": "no legacy shim remains",
                    "verification_method": "grep_absence",
                    "verification_command": "legacy-shim",
                    "result": "deferred_to_consolidation",
                    "deferred_reason": "subject lands at consolidation",
                    "deferred_to_phase": "POST_CONSOLIDATION"
                }]
            })
            .as_object()
            .expect("object"),
        ),
        directory: Some("/repo/kitty-specs/001-m".to_owned()),
        negative_invariants_executed: true,
    };
    let summary = strict(&input);
    assert!(summary.ok(), "{:?}", summary.outstanding());
    let disclosure = summary
        .skipped_checks
        .iter()
        .find(|check| check.check == "negative_invariants_deferred")
        .expect("deferral disclosed");
    assert!(disclosure.detail.starts_with(
        "One or more negative invariants are deferred to post-consolidation verification"
    ));
}

#[test]
fn the_diagnose_leg_discloses_skipped_negative_invariant_execution() {
    let mut input = facts("approved");
    input.matrix = MatrixEvidence::Judged {
        matrix: passing_matrix(),
        directory: Some("/repo/kitty-specs/001-m".to_owned()),
        negative_invariants_executed: false,
    };
    let summary = strict(&input);
    assert!(summary.ok());
    assert!(summary.skipped_checks.is_empty());
}

#[test]
fn matrix_evidence_errors_are_prefixed_with_evidence() {
    let mut input = facts("approved");
    input.matrix = MatrixEvidence::Judged {
        matrix: AcceptanceMatrix::from_json(
            serde_json::json!({
                "mission_slug": "001-m",
                "criteria": [{
                    "criterion_id": "FR-001",
                    "description": "FR-001 is satisfied and verified",
                    "proof_type": "manual_qa",
                    "pass_fail": "pass",
                    "evidence": "screenshot"
                }],
                "negative_invariants": []
            })
            .as_object()
            .expect("object"),
        ),
        directory: None,
        negative_invariants_executed: true,
    };
    let summary = strict(&input);
    assert!(!summary.ok());
    assert_eq!(
        summary.activity_issues,
        vec![
            "Evidence: FR-001: manual QA requires verified_at timestamp".to_owned(),
            "Evidence: FR-001: manual QA requires verified_by identity".to_owned(),
        ]
    );
    assert!(
        summary
            .recommended_fix_order
            .contains(&"Fill missing acceptance matrix evidence fields.".to_owned())
    );
}

#[test]
fn an_out_of_domain_criterion_verdict_is_a_python_repr_diagnostic() {
    let mut input = facts("approved");
    input.matrix = MatrixEvidence::Judged {
        matrix: AcceptanceMatrix::from_json(
            serde_json::json!({
                "mission_slug": "001-m",
                "criteria": [{
                    "criterion_id": "FR-001",
                    "description": "FR-001 is satisfied and verified",
                    "proof_type": "code_review",
                    "pass_fail": "weird"
                }],
                "negative_invariants": []
            })
            .as_object()
            .expect("object"),
        ),
        directory: None,
        negative_invariants_executed: true,
    };
    let summary = strict(&input);
    assert_eq!(
        summary.activity_issues,
        vec![
            "Evidence: FR-001: pass_fail must be one of fail, pass, pending; got 'weird'"
                .to_owned(),
            "Acceptance matrix verdict is 'fail' — negative invariants or criteria not satisfied"
                .to_owned(),
        ]
    );
}

#[test]
fn a_missing_matrix_blocks_with_three_skipped_checks() {
    let mut input = facts("approved");
    let message = "Acceptance matrix (acceptance-matrix.json) is required for lane-based features but was not found. This file is normally scaffolded automatically. If it is missing, regenerate it: spec-kitty agent mission finalize-tasks --mission 001-m";
    input.matrix = MatrixEvidence::Missing {
        message: message.to_owned(),
        directory: Some("/repo/kitty-specs/001-m".to_owned()),
    };
    let summary = strict(&input);
    assert!(!summary.ok());
    assert_eq!(summary.activity_issues, vec![message.to_owned()]);
    assert_eq!(summary.blocked_checks[0].check, "acceptance_matrix");
    assert_eq!(
        summary
            .skipped_checks
            .iter()
            .map(|check| check.check.as_str())
            .collect::<Vec<_>>(),
        vec![
            "acceptance_matrix_evidence",
            "negative_invariants",
            "acceptance_matrix_verdict"
        ]
    );
    assert!(
        summary.recommended_fix_order.contains(
            &"Create or restore kitty-specs/<mission>/acceptance-matrix.json.".to_owned()
        )
    );
    assert_eq!(
        summary.acceptance_matrix_dir.as_deref(),
        Some("/repo/kitty-specs/001-m")
    );
}

// ---------------------------------------------------------------------------
// 7. Cancellation: derived provenance, report shape, delivered-nothing guard
// ---------------------------------------------------------------------------

fn canceled_facts(operator: bool) -> SourceAcceptanceFacts {
    let mut input = facts("canceled");
    let row = &mut input.work_packages[0];
    row.has_operator_provenance = operator;
    row.cancellation_reason = Some(if operator {
        "not needed for this mission".to_owned()
    } else {
        "Force move to canceled (probe fixture)".to_owned()
    });
    row.cancellation_actor = Some("probe-operator".to_owned());
    row.cancellation_at = Some("2026-01-01T00:00:00Z".to_owned());
    input.tasks_md = Some("- [ ] T001 not ticked\n".to_owned());
    input
}

#[test]
fn a_synthetic_cancellation_blocks_with_the_provenance_hint() {
    let summary = strict(&canceled_facts(false));
    assert!(!summary.ok());
    let blockers = summary
        .outstanding()
        .into_iter()
        .find(|(bucket, _)| bucket == "lane_blockers")
        .map(|(_, values)| values)
        .unwrap_or_default();
    assert_eq!(
        blockers,
        vec![
            "WP01: canonical lane is 'canceled'; work package is canceled; operator-authored cancellation provenance required — reopen or replace it, then move the work package to approved or done."
                .to_owned()
        ]
    );
    assert!(summary.canceled_wps.is_empty());
    assert!(
        summary.activity_issues.contains(
            &"WP01: canonical lane is 'canceled', expected 'approved' or 'done'".to_owned()
        )
    );
}

#[test]
fn an_operator_cancellation_is_reported_with_the_pinned_shape_and_not_a_blocker() {
    let summary = strict(&canceled_facts(true));
    assert_eq!(summary.canceled_wps.len(), 1);
    let entry = &summary.canceled_wps[0];
    assert_eq!(entry.wp_id, "WP01");
    assert_eq!(entry.reason, "not needed for this mission");
    assert_eq!(entry.actor, "probe-operator");
    assert_eq!(entry.at, "2026-01-01T00:00:00Z");
    assert_eq!(
        entry.to_json(),
        serde_json::json!({
            "wp_id": "WP01",
            "reason": "not needed for this mission",
            "actor": "probe-operator",
            "at": "2026-01-01T00:00:00Z",
        })
    );
    assert!(
        !summary
            .outstanding()
            .iter()
            .any(|(bucket, _)| bucket == "lane_blockers")
    );
    // All-canceled: acceptable endings, but nothing was delivered.
    assert!(!summary.all_done());
    assert!(!summary.ok());
    let delivered = summary
        .outstanding()
        .into_iter()
        .find(|(bucket, _)| bucket == "delivered_nothing")
        .map(|(_, values)| values)
        .expect("delivered-nothing guard");
    assert_eq!(
        delivered,
        vec![
            "mission delivered nothing: every work package is canceled; none reached approved or done"
                .to_owned()
        ]
    );
    // Source's lane-only fix-order view still asks to move the WP.
    assert!(
        summary
            .recommended_fix_order
            .contains(&"Move all work packages to approved or done.".to_owned())
    );
}

#[test]
fn a_canceled_wp_beside_a_done_wp_accepts() {
    let mut input = facts("done");
    let mut second = wp("canceled");
    second.wp_id = "WP02".to_owned();
    second.path = "kitty-specs/001-m/tasks/WP02.md".to_owned();
    second.has_operator_provenance = true;
    second.cancellation_reason = Some("superseded".to_owned());
    second.cancellation_actor = Some("operator".to_owned());
    second.cancellation_at = Some("2026-01-01T00:00:00Z".to_owned());
    input.work_packages.push(second);
    let summary = strict(&input);
    assert!(summary.ok(), "{:?}", summary.outstanding());
    assert_eq!(summary.canceled_wps.len(), 1);
    assert_eq!(summary.lane("done"), ["WP01".to_owned()]);
    assert_eq!(summary.lane("canceled"), ["WP02".to_owned()]);
}

// ---------------------------------------------------------------------------
// 8. tasks.md normalization, clarifications
// ---------------------------------------------------------------------------

#[test]
fn terminal_lanes_normalize_unchecked_tasks_but_a_missing_tasks_md_does_not_block() {
    let mut input = facts("approved");
    input.tasks_md = Some("- [ ] T001 never ticked\n".to_owned());
    assert!(strict(&input).unchecked_tasks.is_empty());
    assert!(strict(&input).ok());

    // A non-terminal WP keeps reporting them.
    let mut active = facts("in_progress");
    active.tasks_md = Some("- [ ] T001 never ticked\n".to_owned());
    assert_eq!(
        strict(&active).unchecked_tasks,
        vec!["- [ ] T001 never ticked".to_owned()]
    );

    // `tasks.md missing` is surfaced by the missing-artifacts gate, not here.
    let mut missing = facts("in_progress");
    missing.tasks_md = None;
    assert!(strict(&missing).unchecked_tasks.is_empty());
}

fn clarification(id: Option<&str>) -> ClarificationScan {
    let comment = match id {
        Some(id) => format!(" <!-- decision_id: {id} -->"),
        None => String::new(),
    };
    ClarificationScan {
        path: "/repo/kitty-specs/001-m/spec.md".to_owned(),
        text: format!("- [NEEDS CLARIFICATION: which store?]{comment}\n"),
        observed_decision_statuses: None,
    }
}

#[test]
fn clarification_markers_block_only_for_unknown_open_or_deferred_decisions() {
    let mut unknown = facts("approved");
    unknown.clarifications = vec![clarification(Some("DM-001"))];
    let unknown_summary = strict(&unknown);
    assert_eq!(
        unknown_summary.needs_clarification,
        vec!["/repo/kitty-specs/001-m/spec.md".to_owned()]
    );
    assert!(!unknown_summary.ok());
    assert!(
        unknown_summary
            .recommended_fix_order
            .contains(&"Resolve open NEEDS CLARIFICATION markers.".to_owned())
    );

    for (status, blocks) in [
        (ClarificationDecisionStatus::Open, true),
        (ClarificationDecisionStatus::Deferred, true),
        (ClarificationDecisionStatus::Resolved, false),
        (ClarificationDecisionStatus::Canceled, false),
    ] {
        let mut input = facts("approved");
        input.clarifications = vec![clarification(Some("DM-001"))];
        input
            .decision_statuses
            .insert(sk_kernel::PythonString::from_scalars("DM-001"), status);
        let summary = strict(&input);
        assert_eq!(
            summary.needs_clarification.is_empty(),
            !blocks,
            "{status:?}"
        );
    }

    // A marker without a decision id is prose, never a blocker.
    let mut prose = facts("approved");
    prose.clarifications = vec![clarification(None)];
    assert!(strict(&prose).needs_clarification.is_empty());
    assert!(strict(&prose).ok());
}

// ---------------------------------------------------------------------------
// 9. Fix order
// ---------------------------------------------------------------------------

#[test]
fn the_recommended_fix_order_follows_source_order() {
    let mut input = facts("planned");
    input.work_packages[0].agent = None;
    input.work_packages[0].implementer_of_record = None;
    input.tasks_md = Some("- [ ] T001 not ticked\n".to_owned());
    input.git_dirty = vec!["M src/lib.rs".to_owned()];
    input.path_conventions = path_findings();
    let summary = strict(&input);
    assert_eq!(
        summary.recommended_fix_order,
        vec![
            "Commit, stash, or discard working tree changes before acceptance.".to_owned(),
            "Fix work-package metadata issues.".to_owned(),
            "Move all work packages to approved or done.".to_owned(),
            "Complete unchecked items in tasks.md.".to_owned(),
            "Fix mission path convention violations.".to_owned(),
        ]
    );
}

// ---------------------------------------------------------------------------
// 10. The canonical matrix verdict exemption
// ---------------------------------------------------------------------------

#[test]
fn verdict_pending_dominance_exempts_only_the_empty_scaffold_placeholder() {
    use sk_status::acceptance::{AcceptanceCriterion, SCAFFOLD_TODO_MARKER};

    let scaffold_only = AcceptanceMatrix::new(
        "001-m",
        vec![AcceptanceCriterion::code_review(
            "AC-001",
            SCAFFOLD_TODO_MARKER,
        )],
    );
    assert_eq!(scaffold_only.overall_verdict(), "pending");

    let with_real_row = AcceptanceMatrix::new(
        "001-m",
        vec![
            AcceptanceCriterion {
                description: SCAFFOLD_TODO_MARKER.to_owned(),
                ..AcceptanceCriterion::code_review("AC-001", SCAFFOLD_TODO_MARKER)
            },
            AcceptanceCriterion::code_review("FR-001", "FR-001 is satisfied and verified"),
        ],
    );
    let mut matrix = with_real_row;
    matrix.criteria[1].pass_fail = "pass".to_owned();
    matrix.criteria[1].evidence = Some("WP01:review:WP01".to_owned());
    assert_eq!(matrix.overall_verdict(), "pass");
}

#[test]
fn the_judged_matrix_verdict_is_exposed_for_the_adapter() {
    let summary = strict(&facts("approved"));
    assert_eq!(summary.matrix_verdict.as_deref(), Some("pass"));
    // Not part of the Source JSON shape: Source carries it in the artifact only.
    assert!(
        !summary
            .to_json()
            .as_object()
            .expect("object")
            .contains_key("matrix_verdict")
    );

    let mut absent = facts("approved");
    absent.matrix = MatrixEvidence::Missing {
        message: "missing".to_owned(),
        directory: None,
    };
    assert!(strict(&absent).matrix_verdict.is_none());
}

#[test]
fn an_unreadable_decision_ledger_is_a_cannot_evaluate_not_an_open_decision() {
    let mut input = facts("approved");
    input.clarifications = vec![clarification(Some("DM-001"))];
    input.decision_statuses_available = false;
    let summary = strict(&input);
    // The marker is NOT reported as an operator-open clarification...
    assert!(summary.needs_clarification.is_empty());
    // ...but its unresolvable status is a blocking, truthfully-labelled gap.
    assert!(!summary.ok());
    assert_eq!(
        summary.activity_issues,
        vec![
            "Clarification markers in /repo/kitty-specs/001-m/spec.md cannot be resolved: no decision ledger is readable, so `[NEEDS CLARIFICATION]` markers naming DM-001 cannot be judged open or resolved".to_owned()
        ]
    );
}

// ---------------------------------------------------------------------------
// 11. No fabricated success on an empty mission
// ---------------------------------------------------------------------------

#[test]
fn a_missing_canonical_state_blocks_even_with_nothing_tracked() {
    let mut input = facts("approved");
    input.work_packages.clear();
    input.status_events_present = false;
    input.status_snapshot_present = false;
    let summary = strict(&input);
    // Source: no tracked WP makes `all_done` vacuously true, but the missing
    // canonical state is a blocking activity issue, never a silent pass.
    assert!(summary.all_done());
    assert!(!summary.ok());
    assert_eq!(
        summary.activity_issues,
        vec![
            "No canonical state found for mission '001-m'. Cannot validate acceptance without status.events.jsonl. Run 'spec-kitty agent mission finalize-tasks --mission 001-m' to bootstrap the event log."
                .to_owned()
        ]
    );
}
