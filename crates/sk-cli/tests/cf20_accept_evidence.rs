//! CF20: the Source implementation-evidence/independence gate and the operator
//! attestation, driven through the real `accept` adapter.
//!
//! Every fixture is a fresh repository with real commits, a real `lanes.json`,
//! real authored work-package frontmatter and a real `status.events.jsonl`, so
//! the exercised path is the composed pipeline (evidence gate → collector →
//! policy → matrix execution → acceptance record), not a unit stub.
//!
//! Native functional contracts only: the pinned-Source differential run over
//! these fixtures is outstanding (see the task report).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};
use sk_cli::compat::lifecycle::{AcceptArgs, accept};
use sk_run_store::{RunStore, StartRequest};

const MISSION_ID: &str = "01KN2371WNZX28ME4MN97B67PJ";
const SLUG: &str = "cf20-demo";
const PLAN_SLUG: &str = "cf20-plan";
const IMPLEMENTER: &str = "claude:opus:implementer";
const REVIEWER: &str = "codex:gpt-5:reviewer";
const LANE_BRANCH: &str = "kitty/mission-cf20-demo-lane-a";

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, contents).expect("write fixture");
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
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
        "mission_number": 20,
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
            "verification_command": "CF20_FORBIDDEN_MARKER",
            "result": "pending",
            "evidence": null
        }]
    })
}

/// One software-dev Mission with a recorded lane and a real lane branch.
struct LaneFixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
}

impl LaneFixture {
    /// * `implementation` — the lane branch commits a change to `src/a.txt`
    ///   (`true`) or only an unrelated path (`false`, so `IMPLEMENTATION_MISSING`).
    /// * `approver` — the actor that records the approval (independence probe).
    fn new(implementation: bool, approver: &str) -> Self {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().join("repo");
        fs::create_dir_all(&root).expect("repo");
        git(&root, &["init", "-q", "-b", "main"]);
        write(&root.join("README.md"), "# Demo\n");
        let mission = root.join("kitty-specs").join(SLUG);
        write(
            &mission.join("meta.json"),
            &serde_json::to_string_pretty(&json!({
                "flattened": false,
                "friendly_name": "CF20 demo",
                "mission_id": MISSION_ID,
                "mission_number": 20,
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
            &mission.join("acceptance-matrix.json"),
            &format!(
                "{}\n",
                serde_json::to_string_pretty(&matrix_document()).unwrap()
            ),
        );
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "planning"]);
        let planning_sha = git(&root, &["rev-parse", "HEAD"]);
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
                "planning_commit_sha": planning_sha,
            }))
            .unwrap(),
        );
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "lanes"]);

        // The lane branch: a real implementation commit, or an unrelated one.
        git(&root, &["checkout", "-q", "-b", LANE_BRANCH]);
        if implementation {
            write(&root.join("src/a.txt"), "implemented\n");
        } else {
            write(&root.join("docs/notes.md"), "unrelated\n");
        }
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "lane work"]);
        git(&root, &["checkout", "-q", "main"]);

        // The status log: implementer claims/works, *approver* reviews.
        let mut claim = transition(
            "01J00000000000000000000001",
            "planned",
            "claimed",
            IMPLEMENTER,
            "2026-11-02T10:01:00Z",
        );
        claim["policy_metadata"] = json!({ "agent": IMPLEMENTER });
        let mut approval = transition(
            "01J00000000000000000000005",
            "in_review",
            "approved",
            approver,
            "2026-11-02T10:05:00Z",
        );
        approval["review_result"] = json!({
            "reviewer": approver,
            "verdict": "approved",
            "reference": "review-cycle-1:WP01:approved",
        });
        approval["review_ref"] = json!("review-cycle-1:WP01");
        let events = [
            claim,
            transition(
                "01J00000000000000000000002",
                "claimed",
                "in_progress",
                IMPLEMENTER,
                "2026-11-02T10:02:00Z",
            ),
            transition(
                "01J00000000000000000000003",
                "in_progress",
                "for_review",
                IMPLEMENTER,
                "2026-11-02T10:03:00Z",
            ),
            transition(
                "01J00000000000000000000004",
                "for_review",
                "in_review",
                approver,
                "2026-11-02T10:04:00Z",
            ),
            approval,
            transition(
                "01J00000000000000000000006",
                "approved",
                "done",
                approver,
                "2026-11-02T10:06:00Z",
            ),
        ];
        let log: String = events
            .iter()
            .map(|row| serde_json::to_string(row).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        write(&mission.join("status.events.jsonl"), &format!("{log}\n"));

        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "mission"]);

        Self { _temp: temp, root }
    }

    fn mission_file(&self, name: &str) -> PathBuf {
        self.root.join("kitty-specs").join(SLUG).join(name)
    }

    fn head(&self) -> String {
        git(&self.root, &["rev-parse", "HEAD"])
    }

    fn events(&self) -> Vec<Value> {
        fs::read_to_string(self.mission_file("status.events.jsonl"))
            .expect("status log")
            .lines()
            .map(|line| serde_json::from_str(line).expect("event row"))
            .collect()
    }
}

/// An artifact-only (`plan`) Mission with a REAL recorded runtime and a corrupt
/// work-package status surface but NO `lanes.json`.
struct PlanFixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
}

impl PlanFixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().join("repo");
        fs::create_dir_all(&root).expect("repo");
        git(&root, &["init", "-q", "-b", "main"]);
        write(&root.join("README.md"), "# Demo\n");
        write(&root.join("docs/plan.md"), "# delivered plan\n");
        write(
            &root.join(".kittify/charter/activation.yaml"),
            "activated_kinds: []\nmission_type_activations: []\n",
        );
        let mission = root.join("kitty-specs").join(PLAN_SLUG);
        fs::create_dir_all(mission.join("tasks")).expect("tasks dir");
        write(
            &mission.join("meta.json"),
            &serde_json::to_string_pretty(&json!({
                "mission_id": MISSION_ID,
                "mission_slug": PLAN_SLUG,
                "mission_number": 20,
                "mission_type": "plan",
                "friendly_name": "CF20 plan",
                "slug": PLAN_SLUG,
                "target_branch": "main",
                "topology": "lanes",
            }))
            .unwrap(),
        );
        write(&mission.join("spec.md"), "# spec\n");
        write(&mission.join("research.md"), "# research\n");
        write(&mission.join("plan.md"), "# plan\n");
        // A corrupt work-package-only artifact: unreadable JSONL. Source never
        // consults it on the artifact-only branch, so acceptance must not either.
        write(
            &mission.join("status.events.jsonl"),
            "{ this is not a status event\n",
        );
        let template_path = root.join("templates/plan-runtime.yaml");
        write(
            &template_path,
            "mission:\n  key: plan\n  name: Planning Kitty\n  version: \"2.0.0\"\n  description: plan\nsteps:\n  - id: specify\n    title: Specify\n  - id: research\n    title: Research\n    depends_on: [specify]\n  - id: plan\n    title: Plan\n    depends_on: [research]\n  - id: review\n    title: Review\n    depends_on: [plan]\n",
        );
        let store = RunStore::open(&root).expect("run store");
        let outcome = store
            .start(&StartRequest {
                mission_slug: PLAN_SLUG.to_owned(),
                mission_id: Some(MISSION_ID.to_owned()),
                template_path,
                inputs: Default::default(),
                policy_snapshot: Default::default(),
            })
            .expect("start run");
        let run_state = store.state_path(&outcome.run);
        let mut state: Value =
            serde_json::from_slice(&fs::read(&run_state).expect("read state")).expect("state");
        state["completed_steps"] = json!(["specify", "research", "plan", "review"]);
        fs::write(
            &run_state,
            serde_json::to_vec_pretty(&state).expect("encode state"),
        )
        .expect("write state");
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "plan mission"]);
        Self { _temp: temp, root }
    }

    fn mission_file(&self, name: &str) -> PathBuf {
        self.root.join("kitty-specs").join(PLAN_SLUG).join(name)
    }
}

fn args(mission: &str) -> AcceptArgs {
    AcceptArgs {
        mission: Some(mission.to_owned()),
        mode: None,
        merge_commit: None,
        target_branch: None,
        attest_first_landing_commit: false,
        origin_check: None,
        lenient: false,
        diagnose: false,
        no_commit: false,
        actor: Some("operator".to_owned()),
        attest_no_implementation: Vec::new(),
        attest_reason: None,
        json: false,
    }
}

/// The full refusal text of a `CompatError`, read through its JSON payload.
fn refusal_text(error: &sk_cli::compat::support::CompatError) -> String {
    let payload = error.to_json();
    payload["error"].as_str().unwrap_or_default().to_owned()
}

#[test]
fn a_real_implementation_with_an_independent_review_accepts() {
    let fixture = LaneFixture::new(true, REVIEWER);
    let (payload, code) = accept(&fixture.root, &args(SLUG)).expect("accept runs");
    assert_eq!(
        code, 0,
        "a genuine, independently reviewed WP accepts: {payload}"
    );
    assert_eq!(payload["acceptance_ok"], json!(true), "{payload}");
    assert_eq!(payload["attested_work_packages"], json!([]), "{payload}");
    assert!(payload["accept_commit"].is_string(), "{payload}");
    assert_eq!(
        payload["required_artifacts_source"],
        json!("expected-artifacts.yaml"),
        "{payload}"
    );
}

#[test]
fn an_approval_with_no_owned_implementation_is_refused_before_any_write() {
    let fixture = LaneFixture::new(false, REVIEWER);
    let head_before = fixture.head();
    let meta_before = fs::read(fixture.mission_file("meta.json")).expect("meta");
    let matrix_before = fs::read(fixture.mission_file("acceptance-matrix.json")).expect("matrix");
    let error = accept(&fixture.root, &args(SLUG)).expect_err("the gate refuses");
    assert_eq!(error.exit(), 1);
    let text = refusal_text(&error);
    assert!(
        text.starts_with("Mission cf20-demo: work packages cannot be landed as reviewed:"),
        "{text}"
    );
    assert!(text.contains("IMPLEMENTATION_MISSING: WP01"), "{text}");
    assert!(
        text.contains("Recovery for IMPLEMENTATION_MISSING:"),
        "{text}"
    );
    assert_eq!(
        error.to_json()["error_code"],
        json!("IMPLEMENTATION_MISSING")
    );
    // Nothing was written: no acceptance record, no matrix mutation, HEAD stable.
    assert_eq!(fixture.head(), head_before, "the refusal wrote nothing");
    assert_eq!(
        fs::read(fixture.mission_file("meta.json")).expect("meta"),
        meta_before
    );
    assert_eq!(
        fs::read(fixture.mission_file("acceptance-matrix.json")).expect("matrix"),
        matrix_before,
        "a pre-gate refusal must not touch the matrix"
    );
}

#[test]
fn an_implementer_approving_its_own_work_is_not_independent() {
    let fixture = LaneFixture::new(true, IMPLEMENTER);
    let error = accept(&fixture.root, &args(SLUG)).expect_err("the gate refuses");
    assert_eq!(error.exit(), 1);
    let text = refusal_text(&error);
    assert!(text.contains("REVIEW_NOT_INDEPENDENT: WP01"), "{text}");
    assert!(
        text.contains("No attestation lifts this."),
        "the recovery line states the limit: {text}"
    );
    assert_eq!(
        error.to_json()["error_code"],
        json!("REVIEW_NOT_INDEPENDENT")
    );
}

#[test]
fn an_explicit_operator_attestation_lifts_only_the_missing_implementation() {
    let fixture = LaneFixture::new(false, REVIEWER);
    let mut accept_args = args(SLUG);
    accept_args.attest_no_implementation = vec!["WP01".to_owned()];
    accept_args.attest_reason = Some("the deliverable already exists upstream".to_owned());
    let (payload, code) = accept(&fixture.root, &accept_args).expect("attested accept runs");
    assert_eq!(code, 0, "the attested WP accepts: {payload}");
    assert_eq!(
        payload["attested_work_packages"],
        json!(["WP01"]),
        "{payload}"
    );
    // The attestation is a recorded, forced self-transition in the status log.
    let recorded = fixture
        .events()
        .into_iter()
        .find(|row| {
            row["policy_metadata"]["attestation"] == json!("no_implementation")
                && row["wp_id"] == json!("WP01")
        })
        .expect("the attestation event was appended");
    assert_eq!(recorded["force"], json!(true));
    assert_eq!(recorded["from_lane"], json!("done"));
    assert_eq!(recorded["to_lane"], json!("done"));
    assert_eq!(recorded["reason_source"], json!("operator"));
    assert!(
        recorded["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("the deliverable already exists upstream"),
        "{recorded}"
    );
}

#[test]
fn an_attestation_without_a_reason_is_a_usage_refusal_that_records_nothing() {
    let fixture = LaneFixture::new(false, REVIEWER);
    let mut accept_args = args(SLUG);
    accept_args.attest_no_implementation = vec!["WP01".to_owned()];
    let error = accept(&fixture.root, &accept_args).expect_err("the boundary refuses");
    assert_eq!(error.exit(), 2, "Source exits 2 for a missing reason");
    assert!(
        refusal_text(&error).contains("--attest-no-implementation requires --attest-reason"),
        "{}",
        refusal_text(&error)
    );
    assert!(
        fixture
            .events()
            .iter()
            .all(|row| row["policy_metadata"]["attestation"] != json!("no_implementation")),
        "nothing was recorded"
    );
}

#[test]
fn a_read_only_run_cannot_record_an_attestation() {
    let fixture = LaneFixture::new(false, REVIEWER);
    for (diagnose, no_commit) in [(true, false), (false, true)] {
        let mut accept_args = args(SLUG);
        accept_args.diagnose = diagnose;
        accept_args.no_commit = no_commit;
        accept_args.attest_no_implementation = vec!["WP01".to_owned()];
        accept_args.attest_reason = Some("already delivered".to_owned());
        let error = accept(&fixture.root, &accept_args).expect_err("the boundary refuses");
        assert_eq!(error.exit(), 2);
        assert!(
            refusal_text(&error).contains("is read-only. Nothing was recorded."),
            "{}",
            refusal_text(&error)
        );
    }
    assert!(
        fixture
            .events()
            .iter()
            .all(|row| row["policy_metadata"]["attestation"] != json!("no_implementation")),
        "no read-only run recorded an attestation"
    );
}

#[test]
fn diagnose_reports_the_evidence_refusal_as_a_blocked_check_and_writes_nothing() {
    let fixture = LaneFixture::new(false, REVIEWER);
    let head_before = fixture.head();
    let mut accept_args = args(SLUG);
    accept_args.diagnose = true;
    let (payload, code) = accept(&fixture.root, &accept_args).expect("diagnose runs");
    assert_eq!(
        code, 1,
        "an evidence refusal still exits 1 in diagnose: {payload}"
    );
    assert_eq!(payload["diagnose"], json!(true), "{payload}");
    assert_eq!(payload["mutation_performed"], json!(false), "{payload}");
    assert_eq!(payload["error_code"], json!("IMPLEMENTATION_MISSING"));
    assert_eq!(payload["evidence"]["blocked"], json!(true), "{payload}");
    let blocked = payload["blocked_checks"]
        .as_array()
        .expect("blocked checks")
        .iter()
        .any(|check| check["check"] == json!("implementation_evidence"));
    assert!(blocked, "the refusal is a named blocked check: {payload}");
    assert_eq!(fixture.head(), head_before, "diagnose wrote nothing");
}

#[test]
fn non_wp_acceptance_never_consults_the_work_package_status_surface() {
    let fixture = PlanFixture::new();
    // The origin gate legitimately reads status evidence, and `off` contacts
    // nothing (Source), so the corrupt surface below is reachable ONLY through
    // the acceptance path — which must never open it for an artifact-only run.
    let mut accept_args = args(PLAN_SLUG);
    accept_args.origin_check = Some("off".to_owned());
    let (payload, code) = accept(&fixture.root, &accept_args).expect("accept runs");
    assert_eq!(
        code, 0,
        "the artifact-only runtime accepts despite the corrupt WP-only log: {payload}"
    );
    assert_eq!(payload["acceptance_ok"], json!(true), "{payload}");
    // No code lanes and no matrix artifact for the artifact-only branch.
    assert_eq!(payload["acceptance_matrix_path"], Value::Null, "{payload}");
    // The disclosure names the mission type's own declared manifest, not the
    // software-dev trio's source.
    let source = payload["required_artifacts_source"]
        .as_str()
        .expect("disclosure");
    assert_ne!(source, "expected-artifacts.yaml", "{payload}");
    assert_ne!(source, "pinned-default", "{payload}");
    assert!(source.contains("plan"), "{payload}");
    // The corrupt log is untouched (nothing was rewritten to make it readable).
    assert_eq!(
        fs::read_to_string(fixture.mission_file("status.events.jsonl")).unwrap(),
        "{ this is not a status event\n"
    );
}
