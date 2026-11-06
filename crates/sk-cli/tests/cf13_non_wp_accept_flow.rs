//! CF13: non-WP (`plan`-family) acceptance through the real CLI adapter.
//!
//! These drive `sk_cli::compat::lifecycle::accept` — the dispatcher entry — over
//! a fresh repository with a REAL recorded Mission runtime (`.kittify/runtime`
//! index + cursor + frozen template, created through the canonical
//! `sk_run_store::RunStore::start`). The exercised path is therefore
//! collector → Source policy → acceptance record with NO acceptance matrix,
//! which is exactly Source's artifact-only branch.
//!
//! Native functional contracts; the pinned-Source differential run over the same
//! fixtures is listed as outstanding in the task report.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};
use sk_cli::compat::lifecycle::{AcceptArgs, accept};
use sk_run_store::{RunStore, StartRequest};

const MISSION_ID: &str = "01KN2371WNZX28ME4MN97B67PJ";
const SLUG: &str = "plan-demo";

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

/// A four-step `plan` runtime template: the shipped plan action ids.
fn plan_template() -> &'static str {
    "mission:\n  key: plan\n  name: Planning Kitty\n  version: \"2.0.0\"\n  description: plan\nsteps:\n  - id: specify\n    title: Specify\n  - id: research\n    title: Research\n    depends_on: [specify]\n  - id: plan\n    title: Plan\n    depends_on: [research]\n  - id: review\n    title: Review\n    depends_on: [plan]\n"
}

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    run_state: PathBuf,
}

impl Fixture {
    fn new(completed: &[&str]) -> Self {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().join("repo");
        fs::create_dir_all(&root).expect("repo");
        git(&root, &["init", "-q", "-b", "main"]);
        write(&root.join("README.md"), "# Demo\n");
        // The plan mission declares `paths: {workspace: docs/}`; a real plan
        // Mission writes there, so the strict path-convention gate is satisfied.
        write(&root.join("docs/plan.md"), "# delivered plan\n");
        fs::create_dir_all(root.join(".kittify")).expect("kittify dir");
        write(
            &root.join(".kittify/charter/activation.yaml"),
            "activated_kinds: []\nmission_type_activations: []\n",
        );
        let mission = root.join("kitty-specs").join(SLUG);
        std::fs::create_dir_all(mission.join("tasks")).expect("tasks dir");
        write(
            &mission.join("meta.json"),
            &serde_json::to_string_pretty(&json!({
                "mission_id": MISSION_ID,
                "mission_slug": SLUG,
                "mission_number": 13,
                "mission_type": "plan",
                "friendly_name": "Plan demo",
                "slug": SLUG,
                "target_branch": "main",
                "topology": "lanes",
            }))
            .unwrap(),
        );
        write(&mission.join("spec.md"), "# spec\n");
        write(&mission.join("research.md"), "# research\n");
        write(&mission.join("plan.md"), "# plan\n");

        // The recorded runtime, created through the canonical store.
        let template_path = root.join("templates/plan-runtime.yaml");
        write(&template_path, plan_template());
        let store = RunStore::open(&root).expect("run store");
        let outcome = store
            .start(&StartRequest {
                mission_slug: SLUG.to_owned(),
                mission_id: Some(MISSION_ID.to_owned()),
                template_path,
                inputs: Default::default(),
                policy_snapshot: Default::default(),
            })
            .expect("start run");
        let run_state = store.state_path(&outcome.run);
        let mut state: Value =
            serde_json::from_slice(&fs::read(&run_state).expect("read state")).expect("state");
        state["completed_steps"] = json!(
            completed
                .iter()
                .map(|step| step.to_string())
                .collect::<Vec<_>>()
        );
        fs::write(
            &run_state,
            serde_json::to_vec_pretty(&state).expect("encode state"),
        )
        .expect("write state");

        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "plan mission"]);

        Self {
            _temp: temp,
            root,
            run_state,
        }
    }

    fn mission_file(&self, name: &str) -> PathBuf {
        self.root.join("kitty-specs").join(SLUG).join(name)
    }

    fn meta(&self) -> Value {
        serde_json::from_str(&fs::read_to_string(self.mission_file("meta.json")).unwrap()).unwrap()
    }

    fn head(&self) -> String {
        git(&self.root, &["rev-parse", "HEAD"])
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

const ALL_STEPS: [&str; 4] = ["specify", "research", "plan", "review"];

#[test]
fn a_complete_artifact_only_runtime_is_accepted_without_a_matrix() {
    let fixture = Fixture::new(&ALL_STEPS);
    let (payload, code) = accept(&fixture.root, &args(false, false)).expect("accept runs");
    assert_eq!(code, 0, "the plan Mission should be acceptable: {payload}");
    assert_eq!(payload["ok"], json!(true), "{payload}");
    assert_eq!(payload["acceptance_ok"], json!(true));
    assert_eq!(payload["mutation_performed"], json!(true));
    assert_eq!(payload["diagnose"], json!(false));
    // No matrix: the gate legitimately never ran for an artifact-only Mission.
    assert_eq!(payload["acceptance_matrix_path"], Value::Null, "{payload}");
    assert_eq!(payload["matrix_verdict"], Value::Null, "{payload}");
    assert!(
        payload["skipped_checks"]
            .as_array()
            .is_some_and(|checks| checks.is_empty()),
        "the non-WP branch records no skipped matrix checks: {payload}"
    );

    // The acceptance metadata is recorded and stamped with a real commit.
    let meta = fixture.meta();
    assert_eq!(meta["accepted_by"], json!("operator"));
    assert!(
        meta["accept_commit"]
            .as_str()
            .is_some_and(|commit| commit.len() == 40),
        "{meta}"
    );
    // No matrix artifact was invented on disk.
    assert!(!fixture.mission_file("acceptance-matrix.json").exists());
    // Nothing tracked is left dirty.
    let porcelain = git(
        &fixture.root,
        &["status", "--porcelain", "--untracked-files=no"],
    );
    assert!(porcelain.is_empty(), "{porcelain}");
}

#[test]
fn an_incomplete_runtime_blocks_and_writes_nothing() {
    let fixture = Fixture::new(&["specify", "research"]);
    let head_before = fixture.head();
    let state_before = fs::read(&fixture.run_state).expect("state bytes");
    let (payload, code) = accept(&fixture.root, &args(false, false)).expect("accept runs");
    assert_eq!(code, 1, "an incomplete runtime blocks: {payload}");
    assert_eq!(payload["ok"], json!(false));
    assert_eq!(payload["mutation_performed"], json!(false));
    assert!(
        payload["activity_issues"]
            .as_array()
            .is_some_and(
                |issues| issues
                    .iter()
                    .any(|issue| issue.as_str().is_some_and(|text| text
                        == "Non-WP runtime is not complete; remaining steps: plan, review"))
            ),
        "{payload}"
    );
    assert!(fixture.meta().get("accepted_by").is_none());
    assert_eq!(fixture.head(), head_before);
    assert_eq!(
        fs::read(&fixture.run_state).expect("state bytes"),
        state_before,
        "the runtime is read-only evidence"
    );
}

#[test]
fn a_declared_artifact_missing_from_the_mission_dir_blocks() {
    let fixture = Fixture::new(&ALL_STEPS);
    fs::remove_file(fixture.mission_file("plan.md")).expect("remove plan.md");
    git(&fixture.root, &["add", "-A"]);
    git(&fixture.root, &["commit", "-q", "-m", "drop plan.md"]);
    let (payload, code) = accept(&fixture.root, &args(false, false)).expect("accept runs");
    assert_eq!(code, 1, "{payload}");
    assert_eq!(
        payload["missing_artifacts"],
        json!(["plan.md"]),
        "{payload}"
    );
    assert!(fixture.meta().get("accepted_by").is_none());
}

#[test]
fn diagnose_is_read_only_and_reports_eligibility() {
    let fixture = Fixture::new(&ALL_STEPS);
    let head_before = fixture.head();
    let meta_before = fs::read(fixture.mission_file("meta.json")).expect("meta bytes");
    let (payload, code) = accept(&fixture.root, &args(true, false)).expect("diagnose runs");
    assert_eq!(code, 0, "diagnose reports, it does not refuse: {payload}");
    assert_eq!(payload["diagnose"], json!(true));
    assert_eq!(payload["mutation_performed"], json!(false));
    assert_eq!(payload["ok"], json!(true), "{payload}");
    assert!(payload.get("accept_commit").is_none(), "{payload}");
    assert_eq!(fixture.head(), head_before);
    assert_eq!(
        fs::read(fixture.mission_file("meta.json")).expect("meta bytes"),
        meta_before
    );
}

#[test]
fn no_commit_leaves_the_tree_untouched() {
    let fixture = Fixture::new(&ALL_STEPS);
    let head_before = fixture.head();
    let (payload, code) = accept(&fixture.root, &args(false, true)).expect("accept runs");
    assert_eq!(code, 0, "{payload}");
    assert_eq!(payload["no_commit"], json!(true));
    assert_eq!(payload["mutation_performed"], json!(false));
    assert_eq!(fixture.head(), head_before);
    assert!(fixture.meta().get("accepted_by").is_none());
}

#[test]
fn a_retry_after_a_blocking_run_accepts_once_the_runtime_completes() {
    let fixture = Fixture::new(&["specify", "research", "plan"]);
    let (blocked, code) = accept(&fixture.root, &args(false, false)).expect("accept runs");
    assert_eq!(code, 1, "{blocked}");
    assert!(fixture.meta().get("accepted_by").is_none());

    // Complete the runtime through the same fixture state the store persists.
    let mut state: Value =
        serde_json::from_slice(&fs::read(&fixture.run_state).expect("state")).expect("state");
    state["completed_steps"] = json!(["specify", "research", "plan", "review"]);
    fs::write(
        &fixture.run_state,
        serde_json::to_vec_pretty(&state).expect("encode"),
    )
    .expect("write state");
    git(&fixture.root, &["add", "-A"]);
    git(&fixture.root, &["commit", "-q", "-m", "complete runtime"]);

    let (payload, code) = accept(&fixture.root, &args(false, false)).expect("accept retries");
    assert_eq!(code, 0, "{payload}");
    assert_eq!(payload["ok"], json!(true));
    assert_eq!(fixture.meta()["accepted_by"], json!("operator"));
}
