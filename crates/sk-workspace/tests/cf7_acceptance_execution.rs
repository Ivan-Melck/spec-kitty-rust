//! CF7: real acceptance-matrix execution against a fresh, independent Git
//! repository — population from recorded review verdicts, negative-invariant
//! enforcement with provenance, locked persistence, idempotent re-runs, the
//! read-only diagnose leg, and the Source row-ownership splice.
//!
//! Native functional contracts, not a Source differential campaign (that run is
//! outstanding in the task report).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};
use sk_status::acceptance::source::MatrixEvidence;
use sk_status::acceptance::{
    AcceptanceCriterion, AcceptanceMatrix, NegativeInvariant, ReviewEvidence,
};
use sk_workspace::ProjectWorkspace;
use sk_workspace::acceptance_execution::{
    AcceptanceMatrixRequest, execute_acceptance_matrix, splice_owned_rows,
};

const SLUG: &str = "demo";
const MISSION_ID: &str = "01KN2371WNZX28ME4MN97B67PJ";
const FORBIDDEN_MARKER: &str = "CF7_FORBIDDEN_MARKER_9f2a";

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, contents).expect("write fixture");
}

fn git(dir: &Path, home: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(dir)
        .env("HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn transition(event_id: &str, wp_id: &str, from: &str, to: &str, actor: &str, at: &str) -> Value {
    json!({
        "actor": actor,
        "at": at,
        "event_id": event_id,
        "execution_mode": "worktree",
        "force": false,
        "from_lane": from,
        "mission_slug": SLUG,
        "to_lane": to,
        "wp_id": wp_id,
    })
}

fn matrix_document() -> Value {
    json!({
        "mission_slug": SLUG,
        "mission_number": 7,
        "mission_type": "software-dev",
        "overall_verdict": "pending",
        "criteria": [{
            "criterion_id": "AC-001",
            "description": "Every work package carries an approved review",
            "proof_type": "code_review",
            "evidence": null,
            "pass_fail": "pending",
            "verified_by": null,
            "verified_at": null,
            "notes": null
        }],
        "negative_invariants": [{
            "invariant_id": "NI-001",
            "description": "The retired marker is gone",
            "verification_method": "grep_absence",
            "verification_command": FORBIDDEN_MARKER,
            "result": "pending",
            "evidence": null
        }]
    })
}

impl Fixture {
    fn new(with_matrix: bool) -> Self {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().join("repo");
        let home = temp.path().join("home");
        fs::create_dir_all(&root).expect("repo");
        fs::create_dir_all(&home).expect("home");
        git(&root, &home, &["init", "-q", "-b", "main"]);
        write(&root.join("README.md"), "# Demo\n");
        write(
            &root.join(".kittify").join("config.yaml"),
            "vcs:\n  type: git\n",
        );
        let mission = root.join("kitty-specs").join(SLUG);
        write(
            &mission.join("meta.json"),
            &serde_json::to_string_pretty(&json!({
                "created_at": "2026-11-02T10:00:00Z",
                "flattened": false,
                "friendly_name": "Demo",
                "mission_id": MISSION_ID,
                "mission_number": 7,
                "mission_slug": SLUG,
                "mission_type": "software-dev",
                "slug": SLUG,
                "target_branch": "main",
                "topology": "lanes",
            }))
            .unwrap(),
        );
        write(&mission.join("spec.md"), "# Demo spec\n");
        write(&mission.join("plan.md"), "# Demo plan\n");
        write(&mission.join("tasks.md"), "- [x] T001 land the lane work\n");
        write(
            &mission.join("tasks/WP01-a.md"),
            "---\nwork_package_id: WP01\ntitle: A\nowned_files:\n  - src/a.txt\n---\n\n# WP01\n",
        );
        write(
            &mission.join("lanes.json"),
            &serde_json::to_string_pretty(&json!({
                "version": 1,
                "mission_slug": SLUG,
                "mission_id": MISSION_ID,
                "mission_branch": format!("kitty/mission-{SLUG}"),
                "target_branch": "main",
                "lanes": [{
                    "lane_id": "lane-a",
                    "wp_ids": ["WP01"],
                    "write_scope": ["src/a.txt"],
                    "predicted_surfaces": [],
                    "depends_on_lanes": [],
                    "parallel_group": 0
                }],
                "computed_at": "2026-11-02T10:00:00Z",
                "computed_from": "test",
                "planning_artifact_wps": [],
                "planning_commit_sha": null
            }))
            .unwrap(),
        );
        let mut approval = transition(
            "01J00000000000000000000005",
            "WP01",
            "in_review",
            "approved",
            "codex:gpt-5:reviewer",
            "2026-11-02T10:05:00Z",
        );
        approval["review_result"] = json!({
            "reviewer": "codex:gpt-5:reviewer",
            "verdict": "approved",
            "reference": "review-cycle-1:WP01:approved",
        });
        approval["review_ref"] = json!("review-cycle-1:WP01");
        let events = [
            transition(
                "01J00000000000000000000001",
                "WP01",
                "planned",
                "claimed",
                "claude:opus:implementer",
                "2026-11-02T10:01:00Z",
            ),
            transition(
                "01J00000000000000000000003",
                "WP01",
                "in_progress",
                "for_review",
                "claude:opus:implementer",
                "2026-11-02T10:03:00Z",
            ),
            transition(
                "01J00000000000000000000004",
                "WP01",
                "for_review",
                "in_review",
                "codex:gpt-5:reviewer",
                "2026-11-02T10:04:00Z",
            ),
            approval,
            transition(
                "01J00000000000000000000006",
                "WP01",
                "approved",
                "done",
                "codex:gpt-5:reviewer",
                "2026-11-02T10:06:00Z",
            ),
        ];
        let log: String = events
            .iter()
            .map(|row| serde_json::to_string(row).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        write(&mission.join("status.events.jsonl"), &format!("{log}\n"));
        if with_matrix {
            write(
                &mission.join("acceptance-matrix.json"),
                &format!(
                    "{}\n",
                    serde_json::to_string_pretty(&matrix_document()).unwrap()
                ),
            );
        }
        write(&root.join("src/a.txt"), "implemented\n");
        git(&root, &home, &["add", "."]);
        git(&root, &home, &["commit", "-q", "-m", "mission"]);

        Self {
            _temp: temp,
            root,
            home,
        }
    }

    fn mission_dir(&self) -> PathBuf {
        self.root.join("kitty-specs").join(SLUG)
    }

    fn matrix_path(&self) -> PathBuf {
        self.mission_dir().join("acceptance-matrix.json")
    }

    fn matrix(&self) -> AcceptanceMatrix {
        let bytes = fs::read(self.matrix_path()).expect("read matrix");
        let document: Value = serde_json::from_slice(&bytes).expect("parse matrix");
        AcceptanceMatrix::from_json(document.as_object().expect("matrix object"))
    }

    fn workspace(&self) -> sk_workspace::MissionWorkspace {
        ProjectWorkspace::open(&self.root)
            .expect("open workspace")
            .mission(SLUG)
            .expect("mission workspace")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.home;
    }
}

fn lanes() -> Vec<(String, String)> {
    vec![("WP01".to_owned(), "done".to_owned())]
}

fn review_rows() -> Vec<ReviewEvidence> {
    vec![ReviewEvidence {
        wp_id: "WP01".to_owned(),
        reviewer: "codex:gpt-5:reviewer".to_owned(),
        verdict: "approved".to_owned(),
        reference: "review-cycle-1:WP01:approved".to_owned(),
    }]
}

#[test]
fn population_enforcement_and_persistence_record_real_provenance() {
    let fixture = Fixture::new(true);
    let workspace = fixture.workspace();
    let head = git(&fixture.root, &fixture.home, &["rev-parse", "HEAD"]);
    let before = fs::read(fixture.matrix_path()).expect("read matrix");

    let run = execute_acceptance_matrix(&AcceptanceMatrixRequest {
        workspace: &workspace,
        lanes: &lanes(),
        review_evidence: &review_rows(),
        diagnose: false,
        lock_timeout: sk_status::mutation_lock::MISSION_WRITE_LOCK_TIMEOUT,
    })
    .expect("matrix runs");

    let MatrixEvidence::Judged {
        matrix,
        directory,
        negative_invariants_executed,
    } = &run.evidence
    else {
        panic!("expected a judged matrix, got {:?}", run.evidence);
    };
    assert!(negative_invariants_executed);
    assert_eq!(
        directory.as_deref(),
        Some(fixture.mission_dir().display().to_string().as_str())
    );
    assert_eq!(matrix.overall_verdict(), "pass");

    let criterion = &matrix.criteria[0];
    assert_eq!(criterion.pass_fail, "pass");
    assert_eq!(
        criterion.verified_by.as_deref(),
        Some("codex:gpt-5:reviewer")
    );
    assert_eq!(
        criterion.evidence.as_deref(),
        Some("WP01:review-cycle-1:WP01:approved")
    );
    assert!(
        criterion
            .notes
            .as_deref()
            .is_some_and(|notes| notes.contains("Auto-derived from WP review evidence")),
        "{:?}",
        criterion.notes
    );
    let stamped = criterion
        .verified_at
        .as_deref()
        .expect("a real timestamp is recorded");
    assert!(stamped.contains('T') && stamped.len() >= 20, "{stamped}");

    let invariant = &matrix.negative_invariants[0];
    assert_eq!(invariant.result, "confirmed_absent");
    assert_eq!(
        invariant.provenance_origin,
        sk_status::acceptance::PROVENANCE_RECORDED
    );
    assert_eq!(invariant.verified_ref.as_deref(), Some(head.as_str()));
    assert_eq!(invariant.verified_surface_kind.as_deref(), Some("primary"));

    // The artifact on disk is the judged one, not the pre-lock snapshot.
    assert_ne!(fs::read(fixture.matrix_path()).unwrap(), before);
    let landed = fixture.matrix();
    assert_eq!(landed.overall_verdict(), "pass");
    assert_eq!(landed.criteria[0].pass_fail, "pass");
    assert_eq!(landed.negative_invariants[0].result, "confirmed_absent");

    // A second run is idempotent: terminal rows are never re-judged and the
    // persisted bytes are stable (the retry contract).
    let after_first = fs::read(fixture.matrix_path()).unwrap();
    let rerun = execute_acceptance_matrix(&AcceptanceMatrixRequest {
        workspace: &workspace,
        lanes: &lanes(),
        review_evidence: &review_rows(),
        diagnose: false,
        lock_timeout: sk_status::mutation_lock::MISSION_WRITE_LOCK_TIMEOUT,
    })
    .expect("rerun");
    assert!(matches!(rerun.evidence, MatrixEvidence::Judged { .. }));
    assert_eq!(fs::read(fixture.matrix_path()).unwrap(), after_first);
}

#[test]
fn diagnose_judges_as_is_and_writes_nothing() {
    let fixture = Fixture::new(true);
    let workspace = fixture.workspace();
    let before = fs::read(fixture.matrix_path()).unwrap();

    let run = execute_acceptance_matrix(&AcceptanceMatrixRequest {
        workspace: &workspace,
        lanes: &lanes(),
        review_evidence: &review_rows(),
        diagnose: true,
        lock_timeout: sk_status::mutation_lock::MISSION_WRITE_LOCK_TIMEOUT,
    })
    .expect("diagnose runs");

    let MatrixEvidence::Judged {
        matrix,
        negative_invariants_executed,
        ..
    } = &run.evidence
    else {
        panic!("expected the matrix to be judged read-only");
    };
    assert!(!negative_invariants_executed);
    assert_eq!(matrix.overall_verdict(), "pending");
    assert_eq!(matrix.criteria[0].pass_fail, "pending");
    assert_eq!(matrix.negative_invariants[0].result, "pending");
    assert!(
        run.skipped_checks
            .iter()
            .any(|check| check.check == "negative_invariants"
                && check.detail.contains("read-only")),
        "{:?}",
        run.skipped_checks
    );
    // Zero writes: the artifact bytes are byte-identical.
    assert_eq!(fs::read(fixture.matrix_path()).unwrap(), before);
}

#[test]
fn a_missing_matrix_is_a_blocking_fact_not_a_silent_pass() {
    let fixture = Fixture::new(false);
    let workspace = fixture.workspace();
    let run = execute_acceptance_matrix(&AcceptanceMatrixRequest {
        workspace: &workspace,
        lanes: &lanes(),
        review_evidence: &review_rows(),
        diagnose: false,
        lock_timeout: sk_status::mutation_lock::MISSION_WRITE_LOCK_TIMEOUT,
    })
    .expect("missing matrix is a fact");

    let MatrixEvidence::Missing { message, directory } = &run.evidence else {
        panic!("expected a missing-matrix fact, got {:?}", run.evidence);
    };
    assert!(message.contains("acceptance-matrix.json"), "{message}");
    assert_eq!(
        directory.as_deref(),
        Some(fixture.mission_dir().display().to_string().as_str())
    );
    assert!(
        run.blocked_checks
            .iter()
            .any(|check| check.check == "acceptance_matrix")
    );
    // Source excludes the presence check itself from the skipped set here
    // ().
    assert!(
        run.skipped_checks
            .iter()
            .any(|check| check.check == "acceptance_matrix_evidence")
    );
    assert!(
        !run.skipped_checks
            .iter()
            .any(|check| check.check == "acceptance_matrix_presence")
    );
    // No matrix is invented to make the gate pass.
    assert!(!fixture.matrix_path().exists());
}

#[test]
fn splice_keeps_a_concurrent_judgement_and_re_authored_rows() {
    let mut snapshot = AcceptanceMatrix::new(
        SLUG,
        vec![
            AcceptanceCriterion::code_review("AC-001", "reviewed"),
            AcceptanceCriterion::code_review("AC-002", "reviewed"),
            AcceptanceCriterion::code_review("AC-003", "reviewed"),
        ],
    );
    snapshot.negative_invariants = vec![NegativeInvariant::pending(
        "NI-001",
        "marker gone",
        "grep_absence",
        Some(FORBIDDEN_MARKER.to_owned()),
    )];

    // `judged` is what this run produced: all rows judged.
    let mut judged = snapshot.clone();
    for row in &mut judged.criteria {
        row.pass_fail = "pass".to_owned();
        row.evidence = Some("this run's evidence".to_owned());
    }
    judged.negative_invariants[0].result = "confirmed_absent".to_owned();

    // `fresh` is the concurrently re-read matrix: AC-001 was judged by another
    // writer, AC-002 is still pending, AC-003 was re-authored underneath this run,
    // and NI-001 was re-registered with a different scope.
    let mut fresh = snapshot.clone();
    fresh.criteria[0].pass_fail = "fail".to_owned();
    fresh.criteria[0].evidence = Some("someone else's judgement".to_owned());
    fresh.criteria[2].description = "re-authored criterion".to_owned();
    fresh.negative_invariants[0].scope = Some("src".to_owned());

    splice_owned_rows(&mut fresh, &snapshot, &judged);

    assert_eq!(fresh.criteria[0].pass_fail, "fail");
    assert_eq!(
        fresh.criteria[0].evidence.as_deref(),
        Some("someone else's judgement")
    );
    assert_eq!(fresh.criteria[1].pass_fail, "pass");
    assert_eq!(fresh.criteria[2].pass_fail, "pending");
    assert_eq!(fresh.negative_invariants[0].result, "pending");
}
