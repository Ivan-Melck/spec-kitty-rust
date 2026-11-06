//! CF7: the real `accept` adapter over a fresh, independent repository.
//!
//! These drive `sk_cli::compat::lifecycle::accept` — the entry the dispatcher
//! calls — so the exercised path is the composed Source pipeline
//! (collector → policy → matrix execution → acceptance record), not a policy
//! unit test. Native functional contracts; the Source differential run is listed
//! as outstanding in the task report.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};
use sk_cli::compat::lifecycle::{AcceptArgs, accept};

const MISSION_ID: &str = "01KN2371WNZX28ME4MN97B67PJ";
const SLUG: &str = "demo";

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

fn transition(event_id: &str, from: &str, to: &str, actor: &str, at: &str) -> Value {
    json!({
        "actor": actor,
        "at": at,
        "event_id": event_id,
        "execution_mode": "worktree",
        "force": false,
        "from_lane": from,
        "mission_slug": SLUG,
        "to_lane": to,
        "wp_id": "WP01",
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
            "verification_command": "CF7_FLOW_FORBIDDEN_MARKER",
            "result": "pending",
            "evidence": null
        }]
    })
}

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
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
        write(&root.join("src/a.txt"), "planned\n");
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
        let mut claim = transition(
            "01J00000000000000000000001",
            "planned",
            "claimed",
            "claude:opus:implementer",
            "2026-11-02T10:01:00Z",
        );
        claim["policy_metadata"] = json!({ "agent": "claude:opus:implementer" });
        let events = [
            claim,
            transition(
                "01J00000000000000000000002",
                "claimed",
                "in_progress",
                "claude:opus:implementer",
                "2026-11-02T10:02:00Z",
            ),
            transition(
                "01J00000000000000000000003",
                "in_progress",
                "for_review",
                "claude:opus:implementer",
                "2026-11-02T10:03:00Z",
            ),
            transition(
                "01J00000000000000000000004",
                "for_review",
                "in_review",
                "codex:gpt-5:reviewer",
                "2026-11-02T10:04:00Z",
            ),
            approval,
            transition(
                "01J00000000000000000000006",
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
        git(&root, &home, &["add", "."]);
        git(&root, &home, &["commit", "-q", "-m", "mission"]);
        // Source's evidence window must name a real planning commit and a
        // real lane tip with a subsequent change to this WP's owned file.
        let planning_sha = git(&root, &home, &["rev-parse", "HEAD"]);
        let lanes_path = mission.join("lanes.json");
        let mut lanes: Value =
            serde_json::from_str(&fs::read_to_string(&lanes_path).unwrap()).unwrap();
        lanes["planning_commit_sha"] = json!(planning_sha);
        write(&lanes_path, &serde_json::to_string_pretty(&lanes).unwrap());
        git(&root, &home, &["add", "."]);
        git(&root, &home, &["commit", "-q", "-m", "pin planning commit"]);
        let branch = sk_workspace::lane_branch_name(SLUG, "lane-a", "main").expect("lane branch");
        git(&root, &home, &["checkout", "-q", "-b", &branch]);
        write(&root.join("src/a.txt"), "implemented\n");
        git(&root, &home, &["add", "src/a.txt"]);
        git(
            &root,
            &home,
            &["commit", "-q", "-m", "implement WP01 owned file"],
        );
        git(&root, &home, &["checkout", "-q", "main"]);

        Self {
            _temp: temp,
            root,
            home,
        }
    }

    fn mission_file(&self, name: &str) -> PathBuf {
        self.root.join("kitty-specs").join(SLUG).join(name)
    }

    fn meta(&self) -> Value {
        serde_json::from_str(&fs::read_to_string(self.mission_file("meta.json")).unwrap()).unwrap()
    }
}

fn args(diagnose: bool, no_commit: bool) -> AcceptArgs {
    AcceptArgs {
        mission: Some(SLUG.to_owned()),
        mode: None,
        merge_commit: None,
        target_branch: None,
        attest_first_landing_commit: false,
        origin_check: None,
        lenient: false,
        diagnose,
        no_commit,
        actor: Some("operator".to_owned()),
        // CF20: Source defaults for the new attestation flags.
        attest_no_implementation: Vec::new(),
        attest_reason: None,
        json: false,
    }
}

#[test]
fn ordinary_acceptance_derives_the_matrix_and_records_it() {
    let fixture = Fixture::new(true);
    let (payload, code) = accept(&fixture.root, &args(false, false)).expect("accept runs");
    assert_eq!(code, 0, "acceptance should pass: {payload}");
    assert_eq!(payload["ok"], json!(true), "{payload}");
    assert_eq!(payload["acceptance_ok"], json!(true), "{payload}");
    assert_eq!(payload["diagnose"], json!(false));
    assert_eq!(payload["mutation_performed"], json!(true));
    assert_eq!(
        payload["required_artifacts_source"],
        json!("expected-artifacts.yaml")
    );
    assert_eq!(
        payload["acceptance_matrix_path"],
        json!(
            fixture
                .root
                .join("kitty-specs")
                .join(SLUG)
                .display()
                .to_string()
        )
    );
    assert_eq!(payload["matrix_verdict"], json!("pass"), "{payload}");

    // The matrix on disk carries this run's derived row, not the scaffold.
    let matrix: Value = serde_json::from_str(
        &fs::read_to_string(fixture.mission_file("acceptance-matrix.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(matrix["overall_verdict"], json!("pass"));
    assert_eq!(matrix["criteria"][0]["pass_fail"], json!("pass"));
    assert_eq!(
        matrix["criteria"][0]["evidence"],
        json!("WP01:review-cycle-1:WP01:approved")
    );

    // The acceptance metadata is recorded and stamped with a real commit.
    let meta = fixture.meta();
    assert_eq!(meta["accepted_by"], json!("operator"));
    assert_eq!(meta["acceptance_mode"], json!("local"));
    assert!(
        meta["accept_commit"]
            .as_str()
            .is_some_and(|commit| commit.len() == 40),
        "{meta}"
    );
    // The matrix finalize commit is real and carries the artifact.
    let last = git(&fixture.root, &fixture.home, &["log", "-1", "--pretty=%s"]);
    assert_eq!(last, "Finalize acceptance artifacts for demo");
    // Nothing tracked is left dirty.
    let porcelain = git(
        &fixture.root,
        &fixture.home,
        &["status", "--porcelain", "--untracked-files=no"],
    );
    assert!(porcelain.is_empty(), "{porcelain}");
}

#[test]
fn diagnose_is_read_only_and_reports_the_pending_blocker() {
    let fixture = Fixture::new(true);
    let matrix_before = fs::read(fixture.mission_file("acceptance-matrix.json")).unwrap();
    let head_before = git(&fixture.root, &fixture.home, &["rev-parse", "HEAD"]);

    let (payload, code) = accept(&fixture.root, &args(true, false)).expect("diagnose runs");
    assert_eq!(code, 0, "diagnose reports, it does not refuse: {payload}");
    assert_eq!(payload["diagnose"], json!(true));
    assert_eq!(payload["mutation_performed"], json!(false));
    assert_eq!(payload["ok"], json!(false), "{payload}");
    assert!(
        payload["activity_issues"]
            .as_array()
            .is_some_and(|issues| issues.iter().any(|issue| issue
                .as_str()
                .is_some_and(|text| text.contains("verdict is 'pending'")))),
        "{payload}"
    );
    let skipped = payload["skipped_checks"]
        .as_array()
        .expect("skipped checks");
    assert!(
        skipped
            .iter()
            .any(|check| check["check"] == json!("negative_invariants")),
        "{payload}"
    );

    // Zero writes: the matrix bytes and HEAD are unchanged, and no acceptance
    // metadata was recorded.
    assert_eq!(
        fs::read(fixture.mission_file("acceptance-matrix.json")).unwrap(),
        matrix_before
    );
    assert_eq!(
        git(&fixture.root, &fixture.home, &["rev-parse", "HEAD"]),
        head_before
    );
    assert!(fixture.meta().get("accepted_by").is_none());
}

#[test]
fn a_code_mission_without_a_matrix_refuses_without_inventing_one() {
    let fixture = Fixture::new(false);
    let (payload, code) = accept(&fixture.root, &args(false, false)).expect("accept runs");
    assert_eq!(code, 1, "a missing matrix blocks: {payload}");
    assert_eq!(payload["ok"], json!(false));
    assert_eq!(payload["mutation_performed"], json!(false));
    assert!(
        payload["blocked_checks"]
            .as_array()
            .is_some_and(|checks| checks
                .iter()
                .any(|check| check["check"] == json!("acceptance_matrix"))),
        "{payload}"
    );
    assert!(!fixture.mission_file("acceptance-matrix.json").exists());
    assert!(fixture.meta().get("accepted_by").is_none());
}

#[test]
fn no_commit_resolves_the_matrix_but_creates_no_commit() {
    let fixture = Fixture::new(true);
    let head_before = git(&fixture.root, &fixture.home, &["rev-parse", "HEAD"]);
    let (payload, code) = accept(&fixture.root, &args(false, true)).expect("accept runs");
    assert_eq!(code, 0, "{payload}");
    assert_eq!(payload["no_commit"], json!(true));
    assert_eq!(payload["mutation_performed"], json!(false));
    // Source: `--no-commit` must still resolve the matrix, otherwise the verdict
    // stays pending and the gate can never pass.
    let matrix: Value = serde_json::from_str(
        &fs::read_to_string(fixture.mission_file("acceptance-matrix.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(matrix["overall_verdict"], json!("pass"));
    // HEAD is unchanged and no acceptance metadata was written.
    assert_eq!(
        git(&fixture.root, &fixture.home, &["rev-parse", "HEAD"]),
        head_before
    );
    assert!(fixture.meta().get("accepted_by").is_none());
}
