//! Acceptance vertical: gate evaluation and the real `acceptance-matrix.json`.
//!
//! The cases here are the acceptance half of the create → tasks → implement →
//! review → accept → consolidate chain. Each one supplies canonical evidence
//! (work-package lanes, event-sourced review verdicts, implementer identities,
//! declared owned files, observed changed files, `tasks.md`) and asserts both the
//! gate verdicts and the artifact bytes that a reader will actually see.

use sk_status::acceptance::{
    ACCEPTANCE_MATRIX_FILE_NAME, AcceptanceInputs, ReviewEvidence, WpAcceptanceInput,
    evaluate_acceptance,
};

fn wp(
    wp_id: &str,
    lane: &str,
    implementer: &str,
    reviewer: &str,
    verdict: &str,
    owned: &[&str],
    changed: &[&str],
) -> WpAcceptanceInput {
    WpAcceptanceInput {
        wp_id: wp_id.to_owned(),
        lane: lane.to_owned(),
        implementer: Some(implementer.to_owned()),
        review: (!reviewer.is_empty()).then(|| ReviewEvidence {
            wp_id: wp_id.to_owned(),
            reviewer: reviewer.to_owned(),
            verdict: verdict.to_owned(),
            reference: format!("review-cycle-1:{wp_id}:{verdict}"),
        }),
        owned_files: owned.iter().map(|value| (*value).to_owned()).collect(),
        changed_files: changed.iter().map(|value| (*value).to_owned()).collect(),
    }
}

fn inputs(work_packages: Vec<WpAcceptanceInput>) -> AcceptanceInputs {
    AcceptanceInputs {
        mission_slug: "demo".to_owned(),
        mission_number: Some(7),
        mission_type: Some("software-dev".to_owned()),
        tasks_md: Some("- [x] T001\n- [x] T002\n".to_owned()),
        lanes_present: true,
        attribution_available: true,
        work_packages,
        canceled_with_provenance: Vec::new(),
        spec_requirement_ids: Vec::new(),
    }
}

fn passing_packages() -> Vec<WpAcceptanceInput> {
    vec![
        wp(
            "WP01",
            "done",
            "claude:opus:implementer",
            "codex:gpt-5:reviewer",
            "approved",
            &["src/a/*"],
            &["src/a/lib.rs"],
        ),
        wp(
            "WP02",
            "approved",
            "codex:gpt-5:implementer",
            "gemini:pro:reviewer",
            "approved",
            &["src/b/*"],
            &["src/b/lib.rs"],
        ),
    ]
}

#[test]
fn a_complete_chain_passes_every_gate_and_writes_the_matrix() {
    let temp = tempfile::tempdir().expect("temp");
    let report = evaluate_acceptance(&inputs(passing_packages()));
    assert!(report.ok(), "{:?}", report.gates);
    assert_eq!(report.verdict(), "pass");
    assert_eq!(
        report
            .gates
            .iter()
            .map(|gate| (gate.id.as_str(), gate.result.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("lanes_manifest", "pass"),
            ("wp_lane_terminal", "pass"),
            ("review_evidence", "pass"),
            ("owned_file_evidence", "pass"),
            ("tasks_checkbox", "pass"),
            ("acceptance_matrix", "pass"),
        ]
    );

    let path = report.matrix.write_to(temp.path()).expect("write matrix");
    assert_eq!(
        path.file_name().unwrap().to_str().unwrap(),
        ACCEPTANCE_MATRIX_FILE_NAME
    );
    let text = std::fs::read_to_string(&path).expect("read matrix");
    let value: serde_json::Value = serde_json::from_str(&text).expect("matrix json");
    assert_eq!(value["mission_slug"], "demo");
    assert_eq!(value["mission_number"], "7");
    assert_eq!(value["mission_type"], "software-dev");
    assert_eq!(value["overall_verdict"], "pass");
    let criteria = value["criteria"].as_array().expect("criteria");
    assert_eq!(criteria.len(), 2);
    for row in criteria {
        assert_eq!(row["pass_fail"], "pass");
        assert_eq!(row["proof_type"], "code_review");
        assert!(
            row["verified_by"]
                .as_str()
                .is_some_and(|value| !value.is_empty())
        );
        assert!(
            row["evidence"]
                .as_str()
                .is_some_and(|value| value.contains("WP0"))
        );
    }
    assert!(value["negative_invariants"].is_array());
}

#[test]
fn every_single_missing_evidence_element_closes_acceptance() {
    let cases: Vec<(&str, AcceptanceInputs)> = vec![
        (
            "lanes_manifest",
            AcceptanceInputs {
                lanes_present: false,
                ..inputs(passing_packages())
            },
        ),
        (
            "wp_lane_terminal",
            inputs(vec![wp(
                "WP01",
                "for_review",
                "claude:opus:implementer",
                "codex:gpt-5:reviewer",
                "approved",
                &["src/a/*"],
                &["src/a/lib.rs"],
            )]),
        ),
        (
            "review_evidence",
            inputs(vec![wp(
                "WP01",
                "done",
                "claude:opus:implementer",
                "",
                "",
                &["src/a/*"],
                &["src/a/lib.rs"],
            )]),
        ),
        (
            "owned_file_evidence",
            inputs(vec![wp(
                "WP01",
                "done",
                "claude:opus:implementer",
                "codex:gpt-5:reviewer",
                "approved",
                &["src/a/*"],
                &["docs/elsewhere.md"],
            )]),
        ),
        (
            "owned_file_evidence",
            AcceptanceInputs {
                attribution_available: false,
                ..inputs(passing_packages())
            },
        ),
        ("tasks_checkbox", {
            let mut input = inputs(vec![wp(
                "WP01",
                "for_review",
                "claude:opus:implementer",
                "codex:gpt-5:reviewer",
                "approved",
                &["src/a/*"],
                &["src/a/lib.rs"],
            )]);
            input.tasks_md = Some("- [ ] T001 unticked\n".to_owned());
            input
        }),
    ];

    for (expected_gate, input) in cases {
        let report = evaluate_acceptance(&input);
        assert!(!report.ok(), "{expected_gate}: report unexpectedly passed");
        let gate = report
            .gates
            .iter()
            .find(|gate| gate.id == expected_gate)
            .unwrap_or_else(|| panic!("no {expected_gate} gate"));
        assert_eq!(gate.result.as_str(), "fail", "{expected_gate}: {gate:?}");
        assert!(!gate.detail.is_empty(), "{expected_gate}: empty detail");
        // `ok()` is the acceptance decision; the matrix verdict only reports the
        // criteria rows (Source keeps them independent).
        assert!(!report.ok());
    }
}

#[test]
fn a_self_review_can_never_be_stamped_as_approved() {
    let report = evaluate_acceptance(&inputs(vec![wp(
        "WP01",
        "done",
        "claude:opus:implementer",
        "claude:opus:implementer",
        "approved",
        &["src/a/*"],
        &["src/a/lib.rs"],
    )]));
    assert!(!report.ok());
    let gate = report
        .gates
        .iter()
        .find(|gate| gate.id == "review_evidence")
        .expect("review gate");
    assert!(gate.detail.contains("not independent"), "{}", gate.detail);
    // The matrix row stays pending: no approval attribution was fabricated.
    assert_eq!(report.matrix.criteria[0].pass_fail, "pending");
    assert!(report.matrix.criteria[0].verified_by.is_none());
    assert_eq!(report.verdict(), "pending");
}

#[test]
fn an_operator_canceled_work_package_is_terminal_only_with_provenance() {
    let package = wp(
        "WP01",
        "canceled",
        "claude:opus:implementer",
        "",
        "",
        &["src/a/*"],
        &[],
    );
    let without = evaluate_acceptance(&inputs(vec![package.clone()]));
    assert!(!without.ok());
    let with = evaluate_acceptance(&AcceptanceInputs {
        canceled_with_provenance: vec!["WP01".to_owned()],
        spec_requirement_ids: Vec::new(),
        ..inputs(vec![package])
    });
    assert!(with.ok(), "{:?}", with.gates);
    let row = &with.matrix.criteria[0];
    assert_eq!(row.pass_fail, "pass");
    assert_eq!(row.verified_by.as_deref(), Some("operator:attestation"));
    assert!(
        row.evidence
            .as_deref()
            .is_some_and(|value| value.contains("canceled_with_operator_provenance"))
    );
}

#[test]
fn a_mission_with_no_tracked_work_package_is_never_accepted() {
    let report = evaluate_acceptance(&inputs(Vec::new()));
    assert!(!report.ok());
    // An empty mission has no criteria rows, so the matrix is pending.
    assert_eq!(report.verdict(), "pending");
    let lane_gate = report
        .gates
        .iter()
        .find(|gate| gate.id == "wp_lane_terminal")
        .expect("lane gate");
    assert!(lane_gate.detail.contains("no work package"));
}

#[test]
fn the_written_matrix_is_byte_stable_and_atomic() {
    let temp = tempfile::tempdir().expect("temp");
    let report = evaluate_acceptance(&inputs(passing_packages()));
    let path = report.matrix.write_to(temp.path()).expect("write");
    let first = std::fs::read(&path).expect("read");
    assert!(first.ends_with(b"\n"));
    // A second write of the same judged matrix produces identical bytes.
    report.matrix.write_to(temp.path()).expect("rewrite");
    assert_eq!(std::fs::read(&path).expect("read"), first);
}
