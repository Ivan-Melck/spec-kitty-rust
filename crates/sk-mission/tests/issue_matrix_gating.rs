//! Source-first issue-matrix gating for post-merge review.
//!
//! A missing matrix is judged by the mission's DECLARED gating issue
//! references, not blanket "not applicable": a `#NNNN` used as an
//! implementation target requires a row, while a PR reference or an explicit
//! context citation does not. The structured `issue-matrix.json` is validated
//! against the closed verdict set, and a scaffold placeholder never passes.

use std::fs;
use std::path::{Path, PathBuf};

use sk_mission::{DeadCodeEvidence, ReviewInputs, review_mission};

fn write(dir: &Path, name: &str, contents: &str) {
    fs::write(dir.join(name), contents).expect("write fixture");
}

fn evidence() -> DeadCodeEvidence {
    DeadCodeEvidence {
        changed_paths: Vec::new(),
        supported_paths: Vec::new(),
        added_symbols: Vec::new(),
        corpus: Vec::new(),
        discovery_error: None,
    }
}

fn inputs(dir: PathBuf) -> ReviewInputs {
    ReviewInputs {
        repo_root: dir.clone(),
        feature_dir: dir,
        mission_slug: "demo".to_owned(),
        mission_id: Some("01KN2371WNZX28ME4MN97B67PJ".to_owned()),
        friendly_name: Some("Demo".to_owned()),
        baseline_merge_commit: Some("0".repeat(40)),
        acceptance_mode: None,
        pr_merge_evidence: None,
        work_packages: vec![("WP01".to_owned(), "done".to_owned())],
        rejected_review_artifacts: Vec::new(),
        dead_code: Some(evidence()),
    }
}

#[test]
fn a_declared_gating_reference_without_a_matrix_is_a_finding() {
    let temp = tempfile::tempdir().expect("temp");
    write(
        temp.path(),
        "spec.md",
        "# Demo\n\n- Fix #1234 as an implementation target.\n",
    );
    let report = review_mission(&inputs(temp.path().to_path_buf()), Some("post-merge"))
        .expect("review runs");
    assert!(
        report
            .findings
            .iter()
            .any(|finding| finding.kind == "issue_matrix_missing"
                && finding.field("issue_ref") == Some("#1234")),
        "{:?}",
        report.findings
    );
    assert_eq!(report.issue_matrix_present, "false");
    assert!(
        report.native_gaps.is_empty(),
        "the missing-matrix gate must actually run: {:?}",
        report.native_gaps
    );
}

#[test]
fn a_context_only_reference_does_not_require_a_matrix() {
    let temp = tempfile::tempdir().expect("temp");
    write(
        temp.path(),
        "spec.md",
        "# Demo\n\nfollow-up: #1234 is tracked elsewhere.\n",
    );
    let report = review_mission(&inputs(temp.path().to_path_buf()), Some("post-merge"))
        .expect("review runs");
    assert_eq!(report.issue_matrix_present, "not_applicable");
    assert!(
        !report
            .findings
            .iter()
            .any(|finding| finding.kind.starts_with("issue_matrix")),
        "{:?}",
        report.findings
    );
}

#[test]
fn a_valid_structured_matrix_satisfies_the_gate() {
    let temp = tempfile::tempdir().expect("temp");
    write(
        temp.path(),
        "spec.md",
        "# Demo\n\n- Fix #1234 as an implementation target.\n",
    );
    write(
        temp.path(),
        "issue-matrix.json",
        r##"{"schema_version":1,"rows":{"#1234":{"verdict":"fixed","evidence_ref":"abc123"}}}"##,
    );
    let report = review_mission(&inputs(temp.path().to_path_buf()), Some("post-merge"))
        .expect("review runs");
    assert_eq!(report.issue_matrix_present, "true");
    assert!(
        !report
            .findings
            .iter()
            .any(|finding| finding.kind.starts_with("issue_matrix")),
        "{:?}",
        report.findings
    );
}

#[test]
fn a_placeholder_verdict_is_a_real_violation() {
    let temp = tempfile::tempdir().expect("temp");
    write(
        temp.path(),
        "spec.md",
        "# Demo\n\n- Fix #1234 as an implementation target.\n",
    );
    write(
        temp.path(),
        "issue-matrix.json",
        r##"{"schema_version":1,"rows":{"#1234":{"verdict":"unknown","evidence_ref":""}}}"##,
    );
    let report = review_mission(&inputs(temp.path().to_path_buf()), Some("post-merge"))
        .expect("review runs");
    assert_eq!(report.issue_matrix_present, "false");
    assert!(
        report
            .findings
            .iter()
            .any(|finding| finding.kind == "issue_matrix_violation"),
        "{:?}",
        report.findings
    );
}
