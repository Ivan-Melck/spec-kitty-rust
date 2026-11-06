//! End-to-end acceptance tests for the native governed `next` facade.
//!
//! Every test drives the real `spec-kitty` binary against a temporary project
//! whose runtime template and Mission metadata are authored by the test. The
//! differential test replays the committed pinned-oracle transcript
//! (`crates/sk-run-store/tests/fixtures/next_scenario.json`) through the CLI and
//! compares the decision envelope field by field, so the facade is checked
//! against reference evidence rather than a structural expectation.
//!
//! These tests never touch this checkout, a real home directory or a real
//! Mission.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};
use tempfile::TempDir;

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle_support;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_spec-kitty")
}

fn packs_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packs/built-in")
        .canonicalize()
        .expect("built-in pack tree")
}

fn run(dir: &Path, home: &Path, args: &[&str]) -> Output {
    Command::new(binary())
        .current_dir(dir)
        .env("HOME", home)
        .env("SPEC_KITTY_PACKS_ROOT", packs_root())
        .env_remove("SPECIFY_REPO_ROOT")
        .env_remove("GIT_CEILING_DIRECTORIES")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(args)
        .output()
        .expect("run spec-kitty")
}

fn single_json(output: &Output) -> Value {
    let text = String::from_utf8(output.stdout.clone()).expect("stdout is UTF-8");
    assert!(
        !text.trim().is_empty(),
        "expected one JSON document on stdout; stderr was:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_str(&text).unwrap_or_else(|error| {
        panic!("stdout was not exactly one JSON document ({error}):\n{text}")
    })
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create fixture parent");
    }
    fs::write(path, contents).expect("write fixture");
}

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(dir)
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
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// An empty git project with an isolated home.
fn project() -> (TempDir, PathBuf) {
    let temp = TempDir::new().expect("temp");
    let home = temp.path().join("home");
    fs::create_dir_all(&home).expect("home");
    git(temp.path(), &["init", "-q", "-b", "main"]);
    write(
        &temp.path().join(".kittify/config.yaml"),
        "agents:\n  available: [codex]\n",
    );
    (temp, home)
}

fn meta(slug: &str, mission_type: &str) -> String {
    json!({
        "mission_id": "01M6GC7EV7RY46WMHMXYWS6XZM",
        "mid8": "01M6GC7E",
        "mission_slug": slug,
        "slug": slug,
        "mission_number": null,
        "mission_type": mission_type,
        "friendly_name": "runtime demo",
        "target_branch": "main",
        "topology": "lanes",
        "created_at": "2026-02-08T12:00:00Z"
    })
    .to_string()
}

/// The committed differential transcript (the oracle's own bytes).
fn oracle_fixture() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../sk-run-store/tests/fixtures/next_scenario.json");
    let text = fs::read_to_string(&path).expect("differential fixture");
    serde_json::from_str(&text).expect("differential fixture is JSON")
}

/// A `requires_inputs` mission matching the differential fixture.
fn input_mission() -> (TempDir, PathBuf, String, Value) {
    let (temp, home) = project();
    let fixture = oracle_fixture();
    let slug = "demo-io-01M6GC7E".to_string();
    let root = temp.path();
    write(
        &root.join(".kittify/missions/demo-io/mission-runtime.yaml"),
        fixture["template"].as_str().expect("template"),
    );
    let dir = root.join("kitty-specs").join(&slug);
    write(&dir.join("meta.json"), fixture["meta"].to_string().as_str());
    write(&dir.join("spec.md"), "# Spec\n");
    write(&dir.join("tasks/.gitkeep"), "");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "fixture"]);
    (temp, home, slug, fixture)
}

fn mission_with_steps(
    mission_type: &str,
    template: &str,
    tasks: Option<&[(&str, &str)]>,
) -> (TempDir, PathBuf, String) {
    let (temp, home) = project();
    let slug = format!("{mission_type}-01M6GC7E");
    let root = temp.path();
    write(
        &root.join(format!(
            ".kittify/missions/{mission_type}/mission-runtime.yaml"
        )),
        template,
    );
    let dir = root.join("kitty-specs").join(&slug);
    write(&dir.join("meta.json"), &meta(&slug, mission_type));
    write(&dir.join("spec.md"), "# Spec\n");
    if let Some(tasks) = tasks {
        for (name, body) in tasks {
            write(&dir.join("tasks").join(name), body);
        }
    }
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "fixture"]);
    (temp, home, slug)
}

fn tree_snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, snapshot: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries {
            let entry = entry.expect("fixture entry");
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .expect("relative path")
                .to_path_buf();
            let file_type = entry.file_type().expect("file type");
            if file_type.is_dir() {
                walk(root, &path, snapshot);
            } else if file_type.is_file() {
                snapshot.insert(relative, fs::read(&path).expect("fixture file"));
            }
        }
    }
    let mut snapshot = BTreeMap::new();
    walk(root, root, &mut snapshot);
    snapshot
}

fn run_dir(root: &Path) -> PathBuf {
    let runs = root.join(".kittify/runtime/runs");
    let mut entries: Vec<PathBuf> = fs::read_dir(&runs)
        .expect("run store")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.is_dir())
        .collect();
    entries.sort();
    assert_eq!(entries.len(), 1, "exactly one durable run");
    entries.pop().expect("run dir")
}

fn read_state(root: &Path) -> Value {
    let path = run_dir(root).join("state.json");
    serde_json::from_str(&fs::read_to_string(path).expect("cursor")).expect("cursor JSON")
}

fn wp_markdown(id: &str, dependencies: &[&str]) -> String {
    format!(
        "---\nwork_package_id: {id}\ntitle: {id} task\nlane: planned\ndependencies: {}\nowned_files: [\"src/**\"]\n---\n\n# {id}\n",
        serde_json::to_string(dependencies).expect("deps")
    )
}

fn status_event(wp_id: &str, from: &str, to: &str, event_id: &str) -> String {
    json!({
        "event_id": event_id,
        "mission_slug": "board-demo-01M6GC7E",
        "wp_id": wp_id,
        "from_lane": from,
        "to_lane": to,
        "at": "2026-02-08T12:00:00Z",
        "actor": "claude",
        "force": false,
        "execution_mode": "worktree"
    })
    .to_string()
}

const BOARD_TEMPLATE: &str = "mission:\n  key: demo-board\n  name: Demo Board\n  version: '1.0'\nsteps:\n  - id: specify\n    title: Specify\n    prompt: Write the spec.\n  - id: implement\n    title: Implement\n    depends_on: [specify]\n    prompt: Implement.\n  - id: review\n    title: Review\n    depends_on: [implement]\n    prompt: Review.\n  - id: accept\n    title: Accept\n    depends_on: [review]\n    prompt: Accept.\n";

/// A finalized two-work-package board with a lane manifest.
///
/// `boot_lanes` seeds the canonical log so the board has real lanes: the first
/// entry becomes `claimed`, any other entry stays `genesis` (no event), which is
/// exactly the state a `finalize-tasks` board has before implementation starts.
fn board_mission(
    mission_type: &str,
    template: &str,
    boot: &[(&str, &str, &str)],
) -> (TempDir, PathBuf, String) {
    let (temp, home) = project();
    let slug = format!("{mission_type}-01M6GC7E");
    let root = temp.path();
    write(
        &root.join(format!(
            ".kittify/missions/{mission_type}/mission-runtime.yaml"
        )),
        template,
    );
    let dir = root.join("kitty-specs").join(&slug);
    write(&dir.join("meta.json"), &meta(&slug, mission_type));
    write(&dir.join("spec.md"), "# Spec\n");
    write(&dir.join("tasks.md"), "# Tasks\n");
    write(&dir.join("tasks/WP01-first.md"), &wp_markdown("WP01", &[]));
    write(
        &dir.join("tasks/WP02-second.md"),
        &wp_markdown("WP02", &["WP01"]),
    );
    let events: Vec<String> = boot
        .iter()
        .map(|(wp_id, from, to)| {
            let (wp_id, from, to) = (*wp_id, *from, *to);
            status_event(
                wp_id,
                from,
                to,
                match (wp_id, to) {
                    ("WP01", "planned") => "01HXYZ0000000000000000AAA1",
                    ("WP01", "claimed") => "01HXYZ0000000000000000AAA2",
                    ("WP01", _) => "01HXYZ0000000000000000AAA3",
                    _ => "01HXYZ0000000000000000BBB1",
                },
            )
        })
        .collect();
    write(
        &dir.join("status.events.jsonl"),
        &format!("{}\n", events.join("\n")),
    );
    write(
        &dir.join("lanes.json"),
        &json!({
            "version": 1,
            "mission_slug": slug,
            "mission_id": "01M6GC7EV7RY46WMHMXYWS6XZM",
            "mission_branch": format!("kitty/{slug}"),
            "target_branch": "main",
            "lanes": [{
                "lane_id": "lane-a",
                "wp_ids": ["WP01", "WP02"],
                "write_scope": ["src/**"],
                "predicted_surfaces": ["src"],
                "depends_on_lanes": [],
                "parallel_group": 0
            }],
            "computed_at": "2026-02-08T12:00:00Z",
            "computed_from": "test",
            "planning_artifact_wps": [],
            "planning_commit_sha": null
        })
        .to_string(),
    );
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "fixture"]);
    (temp, home, slug)
}

// ---------------------------------------------------------------------------
// Query mode
// ---------------------------------------------------------------------------

#[test]
fn a_bare_next_previews_the_first_step_without_writing_anything() {
    let (temp, home, slug) = mission_with_steps(
        "demo",
        "mission:\n  key: demo\n  name: Demo\n  version: '1.0'\nsteps:\n  - id: discovery\n    title: Discovery\n    prompt: Research.\n  - id: specify\n    title: Specify\n    depends_on: [discovery]\n    prompt: Specify.\n",
        None,
    );
    let root = temp.path();
    let before = tree_snapshot(root);

    let output = run(root, &home, &["next", "--mission", &slug, "--json"]);
    assert_eq!(output.status.code(), Some(0));
    let value = single_json(&output);
    assert_eq!(value["kind"], json!("query"));
    assert_eq!(value["is_query"], json!(true));
    assert_eq!(value["run_id"], Value::Null);
    assert_eq!(value["mission_state"], json!("not_started"));
    assert_eq!(value["preview_step"], json!("discovery"));
    assert_eq!(value["prompt_file"], Value::Null);
    assert_eq!(value["action"], Value::Null);
    assert_eq!(value["options"], Value::Null);
    assert_eq!(value["guard_failure_paths"], json!({}));
    assert_eq!(value["origin"], json!({}));
    assert_eq!(value["mission_number"], json!(""));
    assert_eq!(value["mission_type"], json!("demo"));
    assert_eq!(tree_snapshot(root), before, "query mode wrote nothing");
}

#[test]
fn a_bare_next_reports_a_finalized_board_preview_with_the_persisted_run() {
    let (temp, home, slug) = board_mission(
        "demo-board",
        BOARD_TEMPLATE,
        &[
            ("WP01", "genesis", "planned"),
            ("WP01", "planned", "claimed"),
        ],
    );
    let root = temp.path();

    let start = run(
        root,
        &home,
        &["runtime", "start", "--mission", &slug, "--json"],
    );
    assert_eq!(start.status.code(), Some(0));
    let run_id = single_json(&start)["run"]["run_id"].clone();

    let output = run(root, &home, &["next", "--mission", &slug, "--json"]);
    assert_eq!(output.status.code(), Some(0));
    let value = single_json(&output);
    assert_eq!(value["kind"], json!("query"));
    assert_eq!(value["mission_state"], json!("implement"));
    assert_eq!(value["preview_step"], json!("implement"));
    // Only a dependency-ready `planned` work package is a claimable preview;
    // an in-progress board reports the upstream selection reason instead.
    assert_eq!(value["wp_id"], Value::Null);
    assert_eq!(value["reason"], json!("all_wps_in_progress"));
    assert_eq!(value["run_id"], run_id);
    let progress = value["progress"].as_object().expect("progress object");
    assert_eq!(progress["total_wps"], json!(2));
    // Weighted progress uses the reduced status snapshot (one claimed WP), not
    // every task file.
    assert_eq!(progress["weighted_percentage"], json!(5.0));
}

// ---------------------------------------------------------------------------
// Advancing mode
// ---------------------------------------------------------------------------

#[test]
fn advancing_issues_a_step_with_a_real_contained_prompt_and_a_durable_cursor() {
    let (temp, home, slug) = mission_with_steps(
        "demo",
        "mission:\n  key: demo\n  name: Demo\n  version: '1.0'\nsteps:\n  - id: discovery\n    title: Discovery\n    prompt: Research the mission.\n",
        None,
    );
    let root = temp.path();

    let output = run(
        root,
        &home,
        &[
            "next",
            "--mission",
            &slug,
            "--agent",
            "claude",
            "--result",
            "success",
            "--json",
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value = single_json(&output);
    assert_eq!(value["kind"], json!("step"));
    assert_eq!(value["is_query"], json!(false));
    assert_eq!(value["action"], json!("research"));
    assert_eq!(value["step_id"], json!("discovery"));
    assert_eq!(value["agent"], json!("claude"));
    let prompt = value["prompt_file"].as_str().expect("prompt file");
    assert!(Path::new(prompt).is_file(), "prompt missing: {prompt}");
    assert!(
        Path::new(prompt).starts_with(run_dir(root)),
        "prompt must stay inside the run directory: {prompt}"
    );
    let origin = value["origin"].as_object().expect("origin");
    // `origin` comes from mission.yaml, not from mission-runtime.yaml; this
    // fixture declares only the runtime template, so the tier is unknown.
    assert_eq!(origin["mission_tier"], json!("unknown"));

    // The cursor keeps the pinned baseline shape: null identity, the issued
    // step, the RACI record, and the mission slug only in `inputs`.
    let state = read_state(root);
    assert_eq!(state["issued_step_id"], json!("discovery"));
    assert_eq!(state["mission_slug"], Value::Null);
    assert_eq!(state["mission_id"], Value::Null);
    assert_eq!(state["inputs"]["mission_slug"], json!(slug));
    assert_eq!(
        state["decisions"]["raci:discovery"]["responsible"]["actor_id"],
        json!("claude")
    );

    // The identity index names the mission (the cursor does not).
    let index: Value = serde_json::from_str(
        &fs::read_to_string(root.join(".kittify/runtime/feature-runs.json")).expect("index"),
    )
    .expect("index JSON");
    assert_eq!(
        index["01M6GC7EV7RY46WMHMXYWS6XZM"]["mission_slug"],
        json!(slug)
    );
    assert_eq!(
        index["01M6GC7EV7RY46WMHMXYWS6XZM"]["mission_id"],
        json!("01M6GC7EV7RY46WMHMXYWS6XZM")
    );
}

#[test]
fn a_missing_artifact_reissues_the_same_step_with_guard_diagnostics() {
    // `plan` is a software-dev guard step, so an arbitrary mission family is
    // not enough: use the software-dev template and a software-dev mission.
    let (temp, home, slug) = mission_with_steps(
        "software-dev",
        &fs::read_to_string(packs_root().join("missions/software-dev/mission-runtime.yaml"))
            .expect("pack template"),
        None,
    );
    let root = temp.path();

    let first = run(
        root,
        &home,
        &[
            "next",
            "--mission",
            &slug,
            "--agent",
            "claude",
            "--result",
            "success",
            "--json",
        ],
    );
    assert_eq!(first.status.code(), Some(0));
    let issued = single_json(&first);
    assert_eq!(issued["step_id"], json!("discovery"));
    let before = read_state(root);

    // The discovery step has no guard; walk to `specify` and then request
    // `plan` without a plan.md on disk.
    let second = run(
        root,
        &home,
        &[
            "next",
            "--mission",
            &slug,
            "--agent",
            "claude",
            "--result",
            "success",
            "--json",
        ],
    );
    assert_eq!(second.status.code(), Some(0));
    assert_eq!(single_json(&second)["step_id"], json!("specify"));

    let third = run(
        root,
        &home,
        &[
            "next",
            "--mission",
            &slug,
            "--agent",
            "claude",
            "--result",
            "success",
            "--json",
        ],
    );
    assert_eq!(third.status.code(), Some(0));
    let value = single_json(&third);
    assert_eq!(value["kind"], json!("step"));
    assert_eq!(value["step_id"], json!("plan"));
    let state_after_plan = read_state(root);
    assert_eq!(state_after_plan["issued_step_id"], json!("plan"));
    assert_ne!(before, state_after_plan);

    // plan.md is still missing: the advance must re-issue `plan` and leave the
    // cursor exactly where it was.
    let fourth = run(
        root,
        &home,
        &[
            "next",
            "--mission",
            &slug,
            "--agent",
            "claude",
            "--result",
            "success",
            "--json",
        ],
    );
    assert_eq!(fourth.status.code(), Some(0));
    let value = single_json(&fourth);
    assert_eq!(value["kind"], json!("step"));
    assert_eq!(value["step_id"], json!("plan"));
    assert_eq!(value["mission_state"], json!("plan"));
    assert_eq!(
        value["guard_failures"],
        json!(["Required artifact missing: plan.md"])
    );
    assert_eq!(
        value["guard_failure_paths"]["plan.md"],
        json!(format!("kitty-specs/{slug}/plan.md"))
    );
    assert_eq!(
        read_state(root),
        state_after_plan,
        "a guard refusal never advances"
    );
}

#[test]
fn a_human_answer_is_recorded_and_the_run_advances() {
    let (temp, home, slug, fixture) = input_mission();
    let root = temp.path();
    let steps = fixture["steps"].as_array().expect("steps");

    let first = run(
        root,
        &home,
        &[
            "next",
            "--mission",
            &slug,
            "--agent",
            "claude",
            "--result",
            "success",
            "--json",
        ],
    );
    assert_eq!(first.status.code(), Some(0));
    let value = single_json(&first);
    assert_eq!(value["kind"], json!("decision_required"));
    assert_eq!(value["decision_id"], json!("input:owner"));
    assert_eq!(value["input_key"], json!("owner"));
    assert_eq!(value["question"], steps[0]["decision"]["question"]);
    assert_eq!(value["options"], Value::Null);

    let second = run(
        root,
        &home,
        &[
            "next",
            "--mission",
            &slug,
            "--agent",
            "claude",
            "--answer",
            "alice",
            "--result",
            "success",
            "--json",
        ],
    );
    assert_eq!(second.status.code(), Some(0));
    let value = single_json(&second);
    assert_eq!(value["kind"], json!("step"));
    assert_eq!(value["step_id"], json!("specify"));
    assert_eq!(value["action"], json!("specify"));
    assert_eq!(value["answered"], json!("input:owner"));
    assert_eq!(value["answer"], json!("alice"));

    let state = read_state(root);
    assert_eq!(state["inputs"]["owner"], json!("alice"));
    assert_eq!(state["pending_decisions"], json!({}));
    assert_eq!(state["issued_step_id"], json!("specify"));
    let record = &state["decisions"]["input:owner"];
    assert_eq!(record["answered_by"]["actor_type"], json!("human"));
    assert_eq!(record["authority_role"], json!("human"));
    assert_eq!(record["answer"], json!("alice"));
}

#[test]
fn the_differential_transcript_matches_the_pinned_oracle_decision_envelopes() {
    let (temp, home, slug, fixture) = input_mission();
    let root = temp.path();
    let steps = fixture["steps"].as_array().expect("steps");

    let first = run(
        root,
        &home,
        &[
            "next",
            "--mission",
            &slug,
            "--agent",
            "claude",
            "--result",
            "success",
            "--json",
        ],
    );
    let second = run(
        root,
        &home,
        &[
            "next",
            "--mission",
            &slug,
            "--agent",
            "claude",
            "--answer",
            "alice",
            "--result",
            "success",
            "--json",
        ],
    );
    for (index, output) in [first, second].iter().enumerate() {
        assert_eq!(output.status.code(), Some(0));
        let value = single_json(output);
        let expected = &steps[index]["decision"];
        for key in [
            "kind",
            "agent",
            "mission_slug",
            "mission_number",
            "mission_type",
            "mission",
            "action",
            "wp_id",
            "workspace_path",
            "reason",
            "guard_failures",
            "guard_failure_paths",
            "progress",
            "step_id",
            "decision_id",
            "input_key",
            "question",
            "options",
            "is_query",
            "preview_step",
        ] {
            assert_eq!(value[key], expected[key], "step {index} field {key}");
        }
        if let Some(expected_answered) = expected.get("answered") {
            assert_eq!(
                value["answered"], *expected_answered,
                "step {index} answered"
            );
            assert_eq!(value["answer"], expected["answer"], "step {index} answer");
        }
    }
}

// ---------------------------------------------------------------------------
// Work-package board composition
// ---------------------------------------------------------------------------

#[test]
fn activated_custom_non_wp_steps_keep_their_templateless_prompts() {
    let (temp, home, slug, fixture) = input_mission();
    let root = temp.path();
    write(
        &root.join(".kittify/config.yaml"),
        "agents:\n  available: [codex]\nmission_type_activations: [demo-io]\n",
    );
    let first = run(
        root,
        &home,
        &[
            "next",
            "--mission",
            &slug,
            "--agent",
            "claude",
            "--result",
            "success",
            "--json",
        ],
    );
    assert!(first.status.success());
    assert_eq!(
        single_json(&first)["kind"],
        fixture["steps"][0]["decision"]["kind"]
    );
    let specified = run(
        root,
        &home,
        &[
            "next",
            "--mission",
            &slug,
            "--agent",
            "claude",
            "--answer",
            "alice",
            "--result",
            "success",
            "--json",
        ],
    );
    let planned = run(
        root,
        &home,
        &[
            "next",
            "--mission",
            &slug,
            "--agent",
            "claude",
            "--result",
            "success",
            "--json",
        ],
    );
    for (output, step) in [(specified, "specify"), (planned, "plan")] {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value = single_json(&output);
        assert_eq!(value["kind"], json!("step"), "{value}");
        assert_eq!(value["step_id"], json!(step));
        let prompt =
            fs::read_to_string(value["prompt_file"].as_str().expect("authored prompt")).unwrap();
        assert!(
            prompt.starts_with(&format!("# demo-io — {step}\n")),
            "{prompt}"
        );
    }
}

#[test]
fn wp_prompt_preserves_frontmatter_and_authored_trailing_whitespace() {
    let (temp, home, slug) = board_mission(
        "demo-board",
        BOARD_TEMPLATE,
        &[("WP01", "genesis", "planned")],
    );
    let root = temp.path();
    let source_path = root
        .join("kitty-specs")
        .join(&slug)
        .join("tasks/WP01-first.md");
    let mut source = fs::read_to_string(&source_path).unwrap();
    source.push_str("\n  \n\n");
    write(&source_path, &source);
    let output = run(
        root,
        &home,
        &[
            "next",
            "--mission",
            &slug,
            "--agent",
            "claude",
            "--result",
            "success",
            "--json",
        ],
    );
    assert!(output.status.success());
    let value = single_json(&output);
    assert_eq!(value["kind"], json!("step"), "{value}");
    let prompt =
        fs::read_to_string(value["prompt_file"].as_str().expect("authored prompt")).unwrap();
    let wp_block = prompt
        .split("WORK PACKAGE PROMPT BEGINS")
        .nth(1)
        .unwrap()
        .split("WORK PACKAGE PROMPT ENDS")
        .next()
        .unwrap();
    assert!(
        wp_block.contains(&source),
        "full authored WP source must be preserved: {wp_block:?}"
    );
    assert!(wp_block.contains("work_package_id: WP01\n"));
    assert!(wp_block.contains("owned_files:"));
}

#[test]
fn the_finalized_board_dispatches_implement_then_review() {
    let (temp, home, slug) = board_mission(
        "demo-board",
        BOARD_TEMPLATE,
        &[("WP01", "genesis", "planned")],
    );
    let root = temp.path();

    let first = run(
        root,
        &home,
        &[
            "next",
            "--mission",
            &slug,
            "--agent",
            "claude",
            "--result",
            "success",
            "--json",
        ],
    );
    assert_eq!(first.status.code(), Some(0));
    let value = single_json(&first);
    assert_eq!(value["kind"], json!("step"));
    assert_eq!(value["action"], json!("implement"));
    assert_eq!(value["wp_id"], json!("WP01"));
    assert_eq!(value["step_id"], json!("implement"));
    let workspace = value["workspace_path"].as_str().expect("workspace");
    assert!(
        workspace.ends_with(".worktrees/demo-board-01M6GC7E-lane-a"),
        "workspace: {workspace}"
    );
    // `next` reports where the work package runs; allocation belongs to
    // `implement`, exactly as upstream's `resolve_workspace_for_wp` contract
    // says the returned path may not exist yet.
    let prompt = value["prompt_file"].as_str().expect("prompt");
    let body = fs::read_to_string(prompt).expect("prompt body");
    assert!(
        body.contains("IMPLEMENT: WP01"),
        "prompt must name the action and work package: {body}"
    );
    assert!(
        body.contains("Governance:") && body.contains("Directive IDs:"),
        "prompt must carry the rendered governance block: {body}"
    );
    assert!(
        body.contains("WORK PACKAGE PROMPT BEGINS"),
        "prompt must carry the work-package prompt block: {body}"
    );
    assert!(
        body.contains("# WP01"),
        "prompt must carry the authored task body: {body}"
    );
    assert!(
        body.contains("move-task WP01 --to for_review"),
        "implement prompt must carry the exact completion command: {body}"
    );

    // The implementation is handed off for review.
    let events = format!(
        "{}\n{}\n",
        status_event(
            "WP01",
            "planned",
            "in_progress",
            "01HXYZ0000000000000000AAA2"
        ),
        status_event(
            "WP01",
            "in_progress",
            "for_review",
            "01HXYZ0000000000000000AAA3"
        ),
    );
    let events_path = root
        .join("kitty-specs")
        .join(&slug)
        .join("status.events.jsonl");
    let mut existing = fs::read_to_string(&events_path).expect("events");
    existing.push_str(&events);
    fs::write(&events_path, existing).expect("append events");

    let second = run(root, &home, &["next", "--mission", &slug, "--json"]);
    assert_eq!(second.status.code(), Some(0));
    let value = single_json(&second);
    assert_eq!(value["kind"], json!("query"));
    assert_eq!(value["mission_state"], json!("review"));
    // The reference fills `override_wp_id` only for an `implement` preview; a
    // review preview leaves `wp_id` null.
    assert_eq!(value["wp_id"], Value::Null);
}

#[test]
fn the_analysis_gate_downgrades_a_claimable_board_to_analyze() {
    // The software-dev runtime template declares an `analyze` step, so a
    // board that would hand out `implement` first proves the analysis report is
    // current. Upstream B3 downgrades both the preview and the advancing
    // decision instead of issuing implementation work.
    let (temp, home, slug) = board_mission(
        "software-dev",
        &fs::read_to_string(packs_root().join("missions/software-dev/mission-runtime.yaml"))
            .expect("pack template"),
        &[("WP01", "genesis", "planned")],
    );
    let root = temp.path();

    let query = run(root, &home, &["next", "--mission", &slug, "--json"]);
    assert_eq!(query.status.code(), Some(0));
    let value = single_json(&query);
    assert_eq!(value["kind"], json!("query"));
    assert_eq!(value["mission_state"], json!("analyze"));
    assert_eq!(value["preview_step"], json!("analyze"));
    assert_eq!(value["wp_id"], Value::Null);
    assert_eq!(value["error_code"], json!("ANALYSIS_REPORT_MISSING"));
    assert_eq!(
        value["guard_failures"],
        json!(["Analysis report missing: analysis-report.md has not been recorded"])
    );

    let advancing = run(
        root,
        &home,
        &[
            "next",
            "--mission",
            &slug,
            "--agent",
            "claude",
            "--result",
            "success",
            "--json",
        ],
    );
    assert_eq!(advancing.status.code(), Some(0));
    let value = single_json(&advancing);
    assert_eq!(value["kind"], json!("step"));
    assert_eq!(value["step_id"], json!("analyze"));
    assert_eq!(value["action"], json!("analyze"));
    assert_eq!(value["mission_state"], json!("analyze"));
    assert_eq!(value["reason"], Value::Null);
    assert_eq!(value["wp_id"], Value::Null);
    assert_eq!(value["workspace_path"], Value::Null);
    assert_eq!(value["error_code"], json!("ANALYSIS_REPORT_MISSING"));
    assert!(
        Path::new(value["prompt_file"].as_str().expect("prompt")).is_file(),
        "the analyze prompt must materialize"
    );
    // The gate must not touch the cursor's issued step.
    assert_eq!(read_state(root)["issued_step_id"], Value::Null);
}

#[test]
fn a_board_stuck_in_review_is_blocked_with_the_sentinel_reason() {
    let (temp, home, slug) = board_mission(
        "demo-board",
        BOARD_TEMPLATE,
        &[
            ("WP01", "genesis", "planned"),
            ("WP01", "planned", "claimed"),
        ],
    );
    let root = temp.path();
    let events_path = root
        .join("kitty-specs")
        .join(&slug)
        .join("status.events.jsonl");
    let mut existing = fs::read_to_string(&events_path).expect("events");
    existing.push_str(&format!(
        "{}\n{}\n",
        status_event(
            "WP01",
            "claimed",
            "in_progress",
            "01HXYZ0000000000000000AAA4"
        ),
        status_event(
            "WP01",
            "in_progress",
            "in_review",
            "01HXYZ0000000000000000AAA5"
        ),
    ));
    fs::write(&events_path, existing).expect("append events");

    // Query mode is read-only and structurally `kind: query`; the board's
    // blocked verdict is reported as `mission_state: blocked` (upstream
    // `_build_finalized_override_query_decision`).
    let output = run(root, &home, &["next", "--mission", &slug, "--json"]);
    assert_eq!(output.status.code(), Some(0));
    let value = single_json(&output);
    assert_eq!(value["kind"], json!("query"));
    assert_eq!(value["mission_state"], json!("blocked"));
    assert!(
        value["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("review in progress")),
        "reason: {}",
        value["reason"]
    );

    // An advancing call reports the same verdict as an actionable block.
    let advancing = run(
        root,
        &home,
        &[
            "next",
            "--mission",
            &slug,
            "--agent",
            "claude",
            "--result",
            "success",
            "--json",
        ],
    );
    assert_eq!(advancing.status.code(), Some(1));
    let value = single_json(&advancing);
    assert_eq!(value["kind"], json!("blocked"));
    assert_eq!(value["mission_state"], json!("blocked"));
    assert!(value["run_id"].is_string());
    assert!(
        value["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("review in progress")),
        "reason: {}",
        value["reason"]
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

#[test]
fn answer_without_result_is_refused_before_any_write() {
    let (temp, home, slug, _fixture) = input_mission();
    let root = temp.path();
    let before = tree_snapshot(root);
    let output = run(
        root,
        &home,
        &["next", "--mission", &slug, "--answer", "alice", "--json"],
    );
    assert_eq!(output.status.code(), Some(1));
    let value = single_json(&output);
    assert_eq!(
        value["error"],
        json!("--answer requires --result because query mode is read-only")
    );
    assert_eq!(tree_snapshot(root), before);
}

#[test]
fn an_unknown_decision_is_refused_without_starting_a_second_run() {
    let (temp, home, slug, _fixture) = input_mission();
    let root = temp.path();
    let output = run(
        root,
        &home,
        &[
            "next",
            "--mission",
            &slug,
            "--agent",
            "claude",
            "--answer",
            "alice",
            "--decision-id",
            "input:owner",
            "--result",
            "success",
            "--json",
        ],
    );
    // The decision does not exist yet, so the answer is refused.
    assert_eq!(output.status.code(), Some(1));
    let value = single_json(&output);
    assert!(
        value["error"]
            .as_str()
            .is_some_and(|error| error.contains("input:owner")),
        "error: {}",
        value["error"]
    );
    // The refusal must not have advanced the run.
    assert_eq!(read_state(root)["issued_step_id"], Value::Null);
    assert_eq!(read_state(root)["pending_decisions"], json!({}));
}

// ---------------------------------------------------------------------------
// Continuation regressions (R1-R8, G2)
// ---------------------------------------------------------------------------

fn pack_template() -> String {
    fs::read_to_string(packs_root().join("missions/software-dev/mission-runtime.yaml"))
        .expect("pack template")
}

fn run_with_packs(dir: &Path, home: &Path, packs: &Path, args: &[&str]) -> Output {
    Command::new(binary())
        .current_dir(dir)
        .env("HOME", home)
        .env("SPEC_KITTY_PACKS_ROOT", packs)
        .env_remove("SPECIFY_REPO_ROOT")
        .env_remove("GIT_CEILING_DIRECTORIES")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(args)
        .output()
        .expect("run spec-kitty")
}

/// The required, lock-exact reference signal for `project`.
fn oracle_infer_languages(project: &Path) -> Value {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root");
    let script = "import json,sys\nfrom pathlib import Path\nfrom charter.activation.language_scope import infer_repo_languages\nprint(json.dumps(infer_repo_languages(Path(sys.argv[1]))))\n";
    let output = Command::new(oracle_support::python())
        .arg("-c")
        .arg(script)
        .arg(project)
        .current_dir(project)
        .env("PYTHONPATH", repo.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
        .output()
        .expect("run required Python oracle");
    assert!(
        output.status.success(),
        "oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("oracle emitted JSON")
}

fn advance(root: &Path, home: &Path, slug: &str) -> Output {
    run(
        root,
        home,
        &[
            "next",
            "--mission",
            slug,
            "--agent",
            "claude",
            "--result",
            "success",
            "--json",
        ],
    )
}

fn set_issued_step(root: &Path, step: &str) {
    let path = run_dir(root).join("state.json");
    let text = fs::read_to_string(&path).expect("cursor");
    let mut state: Value = serde_json::from_str(&text).expect("cursor JSON");
    state["issued_step_id"] = json!(step);
    // The reference's untouched-run board override applies only while nothing
    // has completed. A WP-iteration regression therefore models the R1
    // reproduction: a run that already walked the DAG to `step`, so the
    // dependency/guard gate (not the first-contact board override) is entered.
    let completed = match step {
        "implement" => vec!["specify"],
        "review" => vec!["specify", "implement"],
        _ => Vec::new(),
    };
    state["completed_steps"] = json!(completed);
    fs::write(&path, state.to_string()).expect("write cursor");
}

fn append_events(root: &Path, slug: &str, lines: &[String]) {
    let path = root
        .join("kitty-specs")
        .join(slug)
        .join("status.events.jsonl");
    let mut existing = fs::read_to_string(&path).expect("events");
    for line in lines {
        existing.push_str(line);
        existing.push('\n');
    }
    fs::write(&path, existing).expect("append events");
}

fn lane_event(wp_id: &str, from: &str, to: &str, suffix: &str) -> String {
    json!({
        "event_id": format!("01HXYZ0000000000000000{suffix}"),
        "mission_slug": "board-demo-01M6GC7E",
        "wp_id": wp_id,
        "from_lane": from,
        "to_lane": to,
        "at": "2026-02-08T12:00:00Z",
        "actor": "claude",
        "force": false,
        "execution_mode": "worktree"
    })
    .to_string()
}

#[test]
fn a_wp_iteration_block_keeps_the_issued_step_id_and_the_actionable_reason() {
    let (temp, home, slug) = board_mission(
        "demo-board",
        BOARD_TEMPLATE,
        &[("WP01", "genesis", "planned")],
    );
    let root = temp.path();
    let start = run(
        root,
        &home,
        &["runtime", "start", "--mission", &slug, "--json"],
    );
    assert_eq!(start.status.code(), Some(0));
    set_issued_step(root, "implement");
    append_events(
        root,
        &slug,
        &[
            lane_event("WP01", "planned", "claimed", "AAA2"),
            lane_event("WP01", "claimed", "in_progress", "AAA3"),
            lane_event("WP01", "in_progress", "for_review", "AAA4"),
            lane_event("WP01", "for_review", "in_review", "AAA5"),
        ],
    );

    let output = advance(root, &home, &slug);
    assert_eq!(output.status.code(), Some(1));
    let value = single_json(&output);
    assert_eq!(value["kind"], json!("blocked"));
    // The internal `blocked:review_in_progress` sentinel never reaches the
    // public envelope; the reference reports the issued step.
    assert_eq!(value["mission_state"], json!("implement"));
    assert_eq!(value["step_id"], json!("implement"));
    assert!(
        value["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("review in progress")),
        "reason: {}",
        value["reason"]
    );
}

#[test]
fn a_missing_canonical_status_log_fails_loud_with_the_bootstrap_remedy() {
    let (temp, home, slug) = board_mission(
        "demo-board",
        BOARD_TEMPLATE,
        &[("WP01", "genesis", "planned")],
    );
    let root = temp.path();
    let start = run(
        root,
        &home,
        &["runtime", "start", "--mission", &slug, "--json"],
    );
    assert_eq!(start.status.code(), Some(0));
    set_issued_step(root, "implement");
    fs::remove_file(
        root.join("kitty-specs")
            .join(&slug)
            .join("status.events.jsonl"),
    )
    .expect("remove status log");

    let output = advance(root, &home, &slug);
    assert_eq!(output.status.code(), Some(1));
    let value = single_json(&output);
    assert_eq!(value["kind"], json!("blocked"));
    assert_eq!(value["mission_state"], json!("implement"));
    assert_eq!(value["step_id"], json!("implement"));
    let reason = value["reason"].as_str().expect("reason");
    assert!(
        reason.contains("Canonical status not found") && reason.contains("finalize-tasks"),
        "reason: {reason}"
    );
    assert_eq!(value["guard_failures"], json!([reason]));
}

#[test]
fn an_activated_mission_type_without_a_profile_is_a_blocked_prompt_failure() {
    let (temp, home, slug) = board_mission(
        "demo-board",
        BOARD_TEMPLATE,
        &[("WP01", "genesis", "planned")],
    );
    let root = temp.path();
    write(
        &root.join(".kittify/config.yaml"),
        "agents:\n  available: [codex]\nmission_type_activations: [demo-board]\n",
    );
    append_events(
        root,
        &slug,
        &[
            lane_event("WP01", "planned", "claimed", "AAA2"),
            lane_event("WP01", "claimed", "in_progress", "AAA3"),
            lane_event("WP01", "in_progress", "for_review", "AAA4"),
        ],
    );

    let output = advance(root, &home, &slug);
    assert_eq!(output.status.code(), Some(1));
    let value = single_json(&output);
    assert_eq!(value["kind"], json!("blocked"));
    assert_eq!(value["prompt_file"], Value::Null);
    let reason = value["reason"].as_str().expect("reason");
    assert!(
        reason.contains("prompt resolution failed for action 'review'")
            && reason.contains("UnknownMissionTypeError")
            && reason.contains("has no loadable profile"),
        "reason: {reason}"
    );
}

#[test]
fn the_issued_prompt_is_the_real_pack_prompt_not_a_placeholder() {
    let (temp, home, slug) = mission_with_steps("software-dev", &pack_template(), None);
    let root = temp.path();
    let output = advance(root, &home, &slug);
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value = single_json(&output);
    assert_eq!(value["step_id"], json!("discovery"));
    let prompt = value["prompt_file"].as_str().expect("prompt");
    let body = fs::read_to_string(prompt).expect("prompt body");
    assert!(
        body.contains(&format!("Mission: {slug}")),
        "prompt must carry the mission header: {body}"
    );
    assert!(
        body.contains("Governance:") && body.contains("Directive IDs:"),
        "prompt must carry the real governance block: {body}"
    );
    assert!(
        body.contains("Capture discovery findings"),
        "prompt must carry the pack's mission-step prompt body: {body}"
    );
    assert!(
        !body.contains("Execute step 'discovery'"),
        "the store placeholder must be replaced: {body}"
    );
}

#[test]
fn a_review_prompt_carries_the_move_task_completion_guidance() {
    let (temp, home, slug) = board_mission(
        "software-dev",
        &pack_template(),
        &[("WP01", "genesis", "planned")],
    );
    let root = temp.path();
    append_events(
        root,
        &slug,
        &[
            lane_event("WP01", "planned", "claimed", "AAA2"),
            lane_event("WP01", "claimed", "in_progress", "AAA3"),
            lane_event("WP01", "in_progress", "for_review", "AAA4"),
        ],
    );
    let output = advance(root, &home, &slug);
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value = single_json(&output);
    assert_eq!(value["action"], json!("review"));
    let body =
        fs::read_to_string(value["prompt_file"].as_str().expect("prompt")).expect("prompt body");
    assert!(body.contains("REVIEW: WP01"), "{body}");
    assert!(
        body.contains("APPROVE: spec-kitty agent tasks move-task WP01 --to approved"),
        "review prompt must carry the approve command: {body}"
    );
    assert!(
        body.contains(
            "REJECT:  spec-kitty agent tasks move-task WP01 --to planned --review-feedback-file"
        ),
        "review prompt must carry the reject command: {body}"
    );
    assert!(
        !body.contains("--result success"),
        "the review prompt must not tell the reviewer to re-run next: {body}"
    );
}

#[test]
fn the_board_override_requires_tasks_md() {
    let (temp, home, slug) = board_mission(
        "demo-board",
        BOARD_TEMPLATE,
        &[
            ("WP01", "genesis", "planned"),
            ("WP01", "planned", "claimed"),
        ],
    );
    let root = temp.path();
    fs::remove_file(root.join("kitty-specs").join(&slug).join("tasks.md"))
        .expect("remove tasks.md");
    let output = run(root, &home, &["next", "--mission", &slug, "--json"]);
    assert_eq!(output.status.code(), Some(0));
    let value = single_json(&output);
    assert_eq!(value["kind"], json!("query"));
    assert_eq!(value["mission_state"], json!("not_started"));
    assert_eq!(value["preview_step"], json!("specify"));
}

fn language_scope_fixture(
    extra_config: &str,
    charter_files: &[(&str, &str)],
) -> (TempDir, PathBuf, String) {
    let (temp, home, slug) = board_mission(
        "software-dev",
        BOARD_TEMPLATE,
        &[("WP01", "genesis", "planned")],
    );
    let root = temp.path();
    let org = root.join("org-packs/acme");
    write(
        &org.join("pack.yaml"),
        "pack_id: 01ARZ3NDEKTSV4RRFFQ69G5FAV\nname: acme\npack_level: org\n",
    );
    write(
        &org.join("drg/fragment.yaml"),
        "pack_name: acme\nnodes: []\nedges: []\n",
    );
    write(
        &org.join("org-charter.yaml"),
        "schema_version: 2\norg_name: Acme\n",
    );
    write(
        &org.join("agent_profiles/rust-specialist.agent.yaml"),
        "profile-id: rust-specialist\nname: Rust Specialist\npurpose: Implement Rust.\nroles: [implementer]\napplies_to_languages: [rust]\nspecialization:\n  primary-focus: Rust\n",
    );
    write(
        &root.join(".kittify/config.yaml"),
        &format!(
            "agents:\n  available: [codex]\n{extra_config}charter_packs:\n  org:\n    packs:\n      - name: acme\n        local_path: {}\n",
            org.display()
        ),
    );
    for (relative, contents) in charter_files {
        write(&root.join(relative), contents);
    }
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "language scope fixture"]);
    (temp, home, slug)
}

fn advance_prompt_body(root: &Path, home: &Path, slug: &str) -> String {
    let output = advance(root, home, slug);
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let decision = single_json(&output);
    assert_eq!(decision["action"], json!("implement"), "{decision}");
    fs::read_to_string(decision["prompt_file"].as_str().expect("prompt")).expect("prompt body")
}

/// Reference-valid profiles, artifacts and plural-kind org graph. The selected
/// profile's unique channel link is a positive signal, unlike missing exclusions.
fn language_scoped_profile_mission(interview: Option<&str>) -> (TempDir, PathBuf, String) {
    let (temp, home, slug) = board_mission(
        "software-dev",
        BOARD_TEMPLATE,
        &[("WP01", "genesis", "planned")],
    );
    let root = temp.path();
    let org = root.join("org-packs/acme");
    write(
        &org.join("pack.yaml"),
        "pack_id: 01ARZ3NDEKTSV4RRFFQ69G5FAV\nname: acme\npack_level: org\n",
    );
    write(
        &org.join("drg/fragment.yaml"),
        "pack_name: acme\nnodes:\n  - id: zig-specialist\n    kind: agent_profiles\n  - id: rust-specialist\n    kind: agent_profiles\n  - id: zig-practice\n    kind: tactics\n    title: Zig practice\n  - id: rust-practice\n    kind: tactics\n    title: Rust practice\nedges:\n  - source: agent_profile:zig-specialist\n    target: tactic:zig-practice\n    relation: requires\n  - source: agent_profile:rust-specialist\n    target: tactic:rust-practice\n    relation: requires\n",
    );
    write(
        &org.join("org-charter.yaml"),
        "schema_version: 2\norg_name: Acme\n",
    );
    write(
        &org.join("agent_profiles/zig-specialist.agent.yaml"),
        "profile-id: zig-specialist\nname: Zig Specialist\npurpose: Implement Zig modules.\nroles: [implementer]\napplies_to_languages: [zig]\ntactic-references:\n  - id: zig-practice\n    rationale: Zig-specific practice\nspecialization:\n  primary-focus: Zig\n",
    );
    write(
        &org.join("agent_profiles/rust-specialist.agent.yaml"),
        "profile-id: rust-specialist\nname: Rust Specialist\npurpose: Implement Rust modules.\nroles: [implementer]\napplies_to_languages: [rust]\ntactic-references:\n  - id: rust-practice\n    rationale: Rust-specific practice\nspecialization:\n  primary-focus: Rust\n",
    );
    write(
        &root.join(".kittify/config.yaml"),
        &format!(
            "agents:\n  available: [codex]\ncharter_packs:\n  org:\n    packs:\n      - name: acme\n        local_path: {}\n",
            org.display()
        ),
    );
    write(
        &root
            .join("kitty-specs")
            .join(&slug)
            .join("tasks/WP01-first.md"),
        "---\nwork_package_id: WP01\ntitle: WP01 task\nlane: planned\ndependencies: []\nowned_files: [\"src/**\"]\nagent_profile: zig-specialist\n---\n\n# WP01\n",
    );
    if let Some(interview) = interview {
        write(
            &root.join(".kittify/charter/interview/answers.yaml"),
            interview,
        );
    }
    git(root, &["add", "-A"]);
    for language in ["zig", "rust"] {
        write(
            &org.join(format!("tactics/{language}-practice.tactic.yaml")),
            &format!(
                "schema_version: '1.0'\nid: {language}-practice\nname: {language} practice\npurpose: Authored specialist practice.\napplies_to_languages: [{language}]\n"
            ),
        );
    }
    write(
        &root.join(".kittify/charter/charter.yaml"),
        "catalog:\n  mission: software-dev\n",
    );
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "language-scoped profile fixture"]);
    (temp, home, slug)
}

fn profile_channel_delivered(body: &str, profile: &str, tactic: &str) -> bool {
    !body.contains("governance context unavailable")
        && body
            .lines()
            .any(|line| line == format!("  - {profile} -> tactic:{tactic}"))
        && body.lines().any(|line| line == format!("  - {tactic}"))
}

fn pin_profile(root: &Path, slug: &str, profile: &str) {
    let path = root
        .join("kitty-specs")
        .join(slug)
        .join("tasks/WP01-first.md");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(
        path,
        text.replace(
            "agent_profile: zig-specialist",
            &format!("agent_profile: {profile}"),
        ),
    )
    .unwrap();
}

fn oracle_profile_channel_delivered(project: &Path, profile: &str, tactic: &str) -> bool {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let script = "import json,sys\nfrom pathlib import Path\nfrom charter.activation.context import build_charter_context\nc=build_charter_context(Path(sys.argv[1]),action='implement',mission_type='software-dev',profile=sys.argv[2],mark_loaded=False)\nheader='Profile-Cited Tactics ('+sys.argv[2]+'):'\ntail=c.text.partition(header)[2]\nprint(json.dumps(bool(tail) and sys.argv[3] in tail))\n";
    let output = Command::new(oracle_support::python())
        .arg("-c")
        .arg(script)
        .arg(project)
        .arg(profile)
        .arg(tactic)
        .current_dir(project)
        .env("PYTHONPATH", repo.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "reference-valid context fixture required: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

const PROFILE_ROOT_GRAPH: &str = "schema_version: '1.0'\ngenerated_at: STATIC\ngenerated_by: test\nnodes:\n  - urn: agent_profile:zig-specialist\n    kind: agent_profile\n  - urn: tactic:zig-practice\n    kind: tactic\nedges:\n  - source: agent_profile:zig-specialist\n    target: tactic:zig-practice\n    relation: requires\n";

#[test]
fn missing_selected_profile_cannot_satisfy_admission_evidence() {
    let (temp, home, slug) =
        language_scoped_profile_mission(Some("answers:\n  languages_frameworks: Zig\n"));
    fs::remove_file(
        temp.path()
            .join("org-packs/acme/agent_profiles/zig-specialist.agent.yaml"),
    )
    .unwrap();
    assert!(!oracle_profile_channel_delivered(
        temp.path(),
        "zig-specialist",
        "zig-practice"
    ));
    let body = advance_prompt_body(temp.path(), &home, &slug);
    assert!(!profile_channel_delivered(
        &body,
        "zig-specialist",
        "zig-practice"
    ));
    assert!(
        body.contains("profile \"zig-specialist\" could not be resolved"),
        "unresolved profile must be observable: {body}"
    );
}

#[test]
fn governance_failure_cannot_satisfy_no_signal_admission_evidence() {
    let (temp, home, slug) = language_scoped_profile_mission(None);
    write(&temp.path().join(".kittify/config.yaml"), "charter: 123\n");
    let body = advance_prompt_body(temp.path(), &home, &slug);
    assert!(
        body.contains("governance context unavailable"),
        "positive failure control: {body}"
    );
    assert!(!profile_channel_delivered(
        &body,
        "zig-specialist",
        "zig-practice"
    ));
}

#[test]
fn bom_config_keeps_governance_composition_and_profile_delivery() {
    let (temp, home, slug) =
        language_scoped_profile_mission(Some("answers:\n  languages_frameworks: Zig\n"));
    let config = temp.path().join(".kittify/config.yaml");
    let text = fs::read_to_string(&config).unwrap();
    fs::write(&config, format!("\u{feff}{text}")).unwrap();
    // This reader already supports BOM. The later activation/YAML reader is a
    // separate dependency; do not attribute its failure to sk-charter config.
    assert!(sk_charter::load_project_config(temp.path(), &sk_charter::Limits::default()).is_ok());
    assert_eq!(oracle_infer_languages(temp.path()), json!(["zig"]));
    let body = advance_prompt_body(temp.path(), &home, &slug);
    assert!(
        !body.contains("governance context unavailable"),
        "BOM must not abort composition: {body}"
    );
    assert!(
        profile_channel_delivered(&body, "zig-specialist", "zig-practice"),
        "{body}"
    );
    assert_eq!(
        fs::read_to_string(config).unwrap(),
        format!("\u{feff}{text}")
    );
}

#[test]
fn root_graph_only_org_and_canonical_project_graph_both_deliver() {
    for (project_layer, keep_fragment) in [(false, false), (true, false), (false, true)] {
        let (temp, home, slug) =
            language_scoped_profile_mission(Some("answers:\n  languages_frameworks: Zig\n"));
        if !keep_fragment {
            fs::remove_file(temp.path().join("org-packs/acme/drg/fragment.yaml")).unwrap();
        }
        let graph = if project_layer {
            temp.path().join(".kittify/charter-packs/graph.yaml")
        } else {
            temp.path().join("org-packs/acme/graph.yaml")
        };
        write(&graph, PROFILE_ROOT_GRAPH);
        assert!(oracle_profile_channel_delivered(
            temp.path(),
            "zig-specialist",
            "zig-practice"
        ));
        let body = advance_prompt_body(temp.path(), &home, &slug);
        assert!(
            profile_channel_delivered(&body, "zig-specialist", "zig-practice"),
            "root graph/project={project_layer}/fragment={keep_fragment}: {body}"
        );
        assert_eq!(
            body.lines()
                .filter(|line| *line == "  - zig-specialist -> tactic:zig-practice")
                .count(),
            1,
            "duplicate layered edges must not duplicate profile deliveries"
        );
    }
}

#[test]
fn an_org_required_tactic_survives_a_deny_and_the_active_language_scope() {
    // An `implement` board dispatch renders the WP governance block, which is
    // the composition surface the doctrine oracle pins for the org floor.
    let (temp, home, slug) = board_mission(
        "software-dev",
        BOARD_TEMPLATE,
        &[("WP01", "genesis", "planned")],
    );
    let root = temp.path();
    let org = root.join("org-packs/acme");
    write(
        &org.join("pack.yaml"),
        "pack_id: 01ARZ3NDEKTSV4RRFFQ69G5FAV\nname: acme\npack_level: org\n",
    );
    write(
        &org.join("drg/fragment.yaml"),
        "pack_name: acme\nnodes: []\nedges: []\n",
    );
    // The org pack requires a non-hub tactic that the project ledger denies.
    // The required floor must net the deny so the tactic is still delivered.
    write(
        &org.join("org-charter.yaml"),
        "schema_version: 2\norg_name: Acme\nrequired_tactics: [aggregate-boundary-design]\n",
    );
    write(
        &org.join("agent_profiles/rust-specialist.agent.yaml"),
        "profile-id: rust-specialist\nname: Rust Specialist\npurpose: Implement Rust.\nroles: [implementer]\napplies_to_languages: [rust]\nspecialization:\n  primary-focus: Rust\n",
    );
    write(
        &org.join("agent_profiles/broken-profile.agent.yaml"),
        "profile-id: broken-profile\nname: Broken Profile\n",
    );
    write(
        &root.join(".kittify/config.yaml"),
        &format!(
            "agents:\n  available: [codex]\nlanguages: [python]\ncharter_packs:\n  org:\n    packs:\n      - name: acme\n        local_path: {}\n",
            org.display()
        ),
    );
    write(
        &root.join(".kittify/charter/activation.yaml"),
        "deactivated_tactics:\n  - aggregate-boundary-design\nmission_type_activations:\n  - software-dev\n",
    );
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "org floor fixture"]);

    // The org required floor is resolved by the canonical doctrine authority,
    // whose documented `SPEC_KITTY_PACKS_ROOT` value is the packs parent
    // (`$repo/packs`), exactly as the oracle is provisioned.
    let packs_parent = packs_root().parent().expect("packs root").to_path_buf();
    let output = run_with_packs(
        root,
        &home,
        &packs_parent,
        &[
            "next",
            "--mission",
            &slug,
            "--agent",
            "claude",
            "--result",
            "success",
            "--json",
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let decision = single_json(&output);
    assert_eq!(decision["action"], json!("implement"), "{decision}");
    let body =
        fs::read_to_string(decision["prompt_file"].as_str().expect("prompt")).expect("prompt body");
    assert!(body.contains("Tactic IDs:"), "{body}");
    assert!(
        body.contains("aggregate-boundary-design"),
        "the org-required non-hub tactic must survive the deny: {body}"
    );
    // `languages: [python]` is the active scope: the rust-scoped profile is
    // excluded, and the exclusion is observable instead of silent.
    assert!(
        body.contains("Agent profile rust-specialist was excluded by the active language scope."),
        "the active-language exclusion must be surfaced: {body}"
    );
    // A source-invalid profile is announced rather than dropped.
    assert!(
        body.contains("Agent profile source is invalid: broken-profile"),
        "the skipped profile must be surfaced: {body}"
    );
}

#[test]
fn a_compiled_charter_language_scope_filters_a_language_specific_profile() {
    let (temp, home, slug) = language_scope_fixture(
        "",
        &[(
            ".kittify/charter/charter.yaml",
            "catalog:\n  languages: [python]\n",
        )],
    );
    let body = advance_prompt_body(temp.path(), &home, &slug);
    assert!(
        body.contains("Agent profile rust-specialist was excluded by the active language scope."),
        "the compiled catalog.languages value must filter the rust profile: {body}"
    );
}

#[test]
fn an_interview_language_declaration_filters_a_language_specific_profile() {
    let (temp, home, slug) = language_scope_fixture(
        "",
        &[(
            ".kittify/charter/interview/answers.yaml",
            "answers:\n  languages_frameworks: Python and pytest\n",
        )],
    );
    let body = advance_prompt_body(temp.path(), &home, &slug);
    assert!(
        body.contains("Agent profile rust-specialist was excluded by the active language scope."),
        "the interview language declaration must filter the rust profile: {body}"
    );
}

#[test]
fn a_custom_zig_interview_admits_a_zig_scoped_profile() {
    let (temp, home, slug) =
        language_scoped_profile_mission(Some("answers:\n  languages_frameworks: Zig\n"));
    let body = advance_prompt_body(temp.path(), &home, &slug);

    assert!(
        profile_channel_delivered(&body, "zig-specialist", "zig-practice"),
        "positive cited-doctrine delivery is required: {body}"
    );
    assert!(oracle_profile_channel_delivered(
        temp.path(),
        "zig-specialist",
        "zig-practice"
    ));
    // Both the profile channel and its actual delivered tactic must be present.
    assert!(
        !body.contains("Agent profile zig-specialist was excluded by the active language scope."),
        "the Zig-scoped profile must be admitted: {body}"
    );
    assert!(
        body.contains("Agent profile rust-specialist was excluded by the active language scope."),
        "the Rust-scoped sibling must stay filtered: {body}"
    );
    // The authored WP profile pin and project metadata are untouched by the
    // language binding.
    assert!(
        body.contains(
            "work_package_id: WP01\ntitle: WP01 task\nlane: planned\ndependencies: []\nowned_files: [\"src/**\"]\nagent_profile: zig-specialist"
        ),
        "the authored work-package prompt must arrive whole: {body}"
    );
    assert!(body.contains(&format!("Mission: {slug}")), "{body}");
    assert!(body.contains("Mission Type: software-dev"), "{body}");
    assert!(body.contains("Agent: claude"), "{body}");
    assert!(body.contains("WORKING DIRECTORY:"), "{body}");
    assert!(
        !body.contains("profile \"zig-specialist\" could not be resolved"),
        "{body}"
    );
}

#[test]
fn changing_the_declaration_from_zig_to_rust_filters_the_zig_profile() {
    let (temp, home, slug) =
        language_scoped_profile_mission(Some("answers:\n  languages_frameworks: Rust and Cargo\n"));
    pin_profile(temp.path(), &slug, "rust-specialist");
    let body = advance_prompt_body(temp.path(), &home, &slug);
    assert!(
        profile_channel_delivered(&body, "rust-specialist", "rust-practice"),
        "Rust channel must deliver: {body}"
    );

    assert!(
        body.contains("Agent profile zig-specialist was excluded by the active language scope."),
        "the Rust declaration must filter the Zig-scoped profile: {body}"
    );
    assert!(
        !body.contains("Agent profile rust-specialist was excluded by the active language scope."),
        "the Rust-scoped sibling must be admitted: {body}"
    );
    assert!(body.contains(&format!("Mission: {slug}")), "{body}");
    assert!(body.contains("Mission Type: software-dev"), "{body}");
    assert!(body.contains("Agent: claude"), "{body}");
}

#[test]
fn removing_the_language_declaration_admits_every_scoped_profile() {
    let (temp, home, slug) = language_scoped_profile_mission(None);
    let body = advance_prompt_body(temp.path(), &home, &slug);

    assert!(
        profile_channel_delivered(&body, "zig-specialist", "zig-practice"),
        "no-signal admission needs a positive delivery: {body}"
    );
    assert!(
        !body.contains("excluded by the active language scope"),
        "no signal admits every scoped profile: {body}"
    );
    assert!(
        body.contains("agent_profile: zig-specialist"),
        "the authored Zig profile pin is preserved: {body}"
    );
    assert!(
        body.contains(&format!("Mission: {slug}")),
        "project metadata stays authored: {body}"
    );
}

#[test]
fn the_cli_language_decision_matches_the_required_oracle() {
    for (interview, expected, admitted, filtered) in [
        (
            "answers:\n  languages_frameworks: Zig\n",
            json!(["zig"]),
            "zig-specialist",
            "rust-specialist",
        ),
        (
            "answers:\n  languages_frameworks: Rust and Cargo\n",
            json!(["rust"]),
            "rust-specialist",
            "zig-specialist",
        ),
    ] {
        let (temp, home, slug) = language_scoped_profile_mission(Some(interview));
        let oracle = oracle_infer_languages(temp.path());
        assert_eq!(oracle, expected, "oracle signal for {interview:?}");
        pin_profile(temp.path(), &slug, admitted);
        let body = advance_prompt_body(temp.path(), &home, &slug);
        let tactic = if admitted == "zig-specialist" {
            "zig-practice"
        } else {
            "rust-practice"
        };
        assert!(oracle_profile_channel_delivered(
            temp.path(),
            admitted,
            tactic
        ));
        assert!(
            profile_channel_delivered(&body, admitted, tactic),
            "selected profile channel must actually deliver: {body}"
        );
        assert!(
            !body.contains(&format!(
                "Agent profile {admitted} was excluded by the active language scope."
            )),
            "the oracle-admitted profile {admitted} must load: {body}"
        );
        assert!(
            body.contains(&format!(
                "Agent profile {filtered} was excluded by the active language scope."
            )),
            "the oracle-filtered profile {filtered} must be dropped: {body}"
        );
        // A built-in Python-scoped profile is filtered by both declarations,
        // proving the CLI profile surface follows the same resolved signal.
        assert!(
            body.contains("Agent profile python-pedro was excluded by the active language scope."),
            "the Python-scoped built-in profile must be filtered: {body}"
        );
    }
}

#[test]
fn origin_comes_from_the_mission_yaml_tier_not_the_runtime_template() {
    let (temp, home, slug) = mission_with_steps("software-dev", &pack_template(), None);
    let root = temp.path();
    write(
        &root.join(".kittify/missions/software-dev/mission.yaml"),
        "mission:\n  key: software-dev\n  name: Software Dev Kitty\n  version: '2.2.0'\n",
    );
    let output = advance(root, &home, &slug);
    assert_eq!(output.status.code(), Some(0));
    let value = single_json(&output);
    let origin = value["origin"].as_object().expect("origin");
    assert_eq!(origin["mission_tier"], json!("legacy"));
    let path = origin["mission_path"].as_str().expect("mission path");
    assert!(
        path.ends_with(".kittify/missions/software-dev"),
        "mission path: {path}"
    );
    assert!(!path.contains("packs/built-in"), "mission path: {path}");
}

#[test]
fn both_documented_packs_root_spellings_resolve_the_same_templates() {
    // Each spelling must be exercised on its own fresh run: a second advance on
    // the same run issues the *next* step, so comparing it with the first
    // step's prompt would test progression, not packs-root resolution.
    let (leaf_temp, leaf_home, leaf_slug) =
        mission_with_steps("software-dev", &pack_template(), None);
    let (parent_temp, parent_home, parent_slug) =
        mission_with_steps("software-dev", &pack_template(), None);
    let leaf_root = leaf_temp.path();
    let parent_root = parent_temp.path();
    let parent_packs = packs_root().parent().expect("packs root").to_path_buf();
    // `run` uses the built-in leaf spelling; the sibling run uses the parent.
    let first = run(
        leaf_root,
        &leaf_home,
        &[
            "next",
            "--mission",
            &leaf_slug,
            "--agent",
            "claude",
            "--result",
            "success",
            "--json",
        ],
    );
    let second = run_with_packs(
        parent_root,
        &parent_home,
        &parent_packs,
        &[
            "next",
            "--mission",
            &parent_slug,
            "--agent",
            "claude",
            "--result",
            "success",
            "--json",
        ],
    );
    assert_eq!(first.status.code(), Some(0));
    assert_eq!(second.status.code(), Some(0));
    let leaf_body = fs::read_to_string(single_json(&first)["prompt_file"].as_str().expect("p"))
        .expect("leaf prompt");
    let parent_body = fs::read_to_string(single_json(&second)["prompt_file"].as_str().expect("p"))
        .expect("parent prompt");
    let normalize =
        |body: &str, root: &Path| body.replace(&root.to_string_lossy().to_string(), "<project>");
    let leaf_body = normalize(&leaf_body, leaf_root);
    let parent_body = normalize(&parent_body, parent_root);
    assert_eq!(
        leaf_body, parent_body,
        "both spellings resolve identical prompts"
    );
    assert!(parent_body.contains("Governance:"), "{parent_body}");
    assert!(
        parent_body.contains("Capture discovery findings"),
        "{parent_body}"
    );
}

#[test]
fn a_torn_journal_query_stays_a_query_and_advance_reports_a_blocked_decision() {
    let (temp, home, slug) = mission_with_steps(
        "demo",
        "mission:\n  key: demo\n  name: Demo\n  version: '1.0'\nsteps:\n  - id: specify\n    title: Specify\n    prompt: Write the spec.\n  - id: plan\n    title: Plan\n    depends_on: [specify]\n    prompt: Write the plan.\n",
        None,
    );
    let root = temp.path();
    let start = run(
        root,
        &home,
        &["runtime", "start", "--mission", &slug, "--json"],
    );
    assert_eq!(start.status.code(), Some(0));
    let journal = run_dir(root).join("run.events.jsonl");
    let mut bytes = fs::read(&journal).expect("journal");
    bytes.extend_from_slice(br#"{"event_type": "NextStepIssued", "payload": {"#);
    fs::write(&journal, &bytes).expect("torn tail");

    let query = run(root, &home, &["next", "--mission", &slug, "--json"]);
    assert_eq!(query.status.code(), Some(0));
    let value = single_json(&query);
    assert_eq!(value["kind"], json!("query"));
    assert_eq!(value["is_query"], json!(true));

    let advancing = advance(root, &home, &slug);
    assert_eq!(advancing.status.code(), Some(1));
    let value = single_json(&advancing);
    assert_eq!(value["kind"], json!("blocked"));
    assert_eq!(value["is_query"], json!(false));
    assert!(
        value["reason"]
            .as_str()
            .is_some_and(|reason| reason.starts_with("Runtime engine error: ")),
        "reason: {}",
        value["reason"]
    );
    // Nothing was appended by the refusal.
    assert_eq!(fs::read(&journal).expect("journal"), bytes);
}

#[test]
fn the_tasks_guard_advances_a_mapped_board_to_analyze() {
    // The board is authored at the `tasks` boundary, exactly as the reference
    // walk does: `tasks/` does not exist while the run walks discovery ->
    // specify -> plan -> tasks, so the finalized-board override cannot
    // pre-empt the DAG.
    let (temp, home, slug) = mission_with_steps("software-dev", &pack_template(), None);
    let root = temp.path();
    let dir = root.join("kitty-specs").join(&slug);
    write(&dir.join("spec.md"), "# Spec\n\n- FR-001: do it\n");
    write(&dir.join("plan.md"), "# Plan\n");

    for expected in ["discovery", "specify", "plan", "tasks"] {
        let output = advance(root, &home, &slug);
        assert_eq!(
            output.status.code(),
            Some(0),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(single_json(&output)["step_id"], json!(expected));
    }

    write(
        &dir.join("tasks/WP01-demo.md"),
        "---\nwork_package_id: WP01\ntitle: WP01\ndependencies: []\nrequirement_refs: [FR-001]\nowned_files: [\"src/**\"]\n---\n\n# WP01\n",
    );
    write(
        &dir.join("tasks.md"),
        "# Tasks\n\n## WP01\n\nRequirements: FR-001\n",
    );

    let output = advance(root, &home, &slug);
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let last = single_json(&output);
    assert_eq!(last["step_id"], json!("analyze"), "decision: {last}");
    assert_eq!(last["guard_failures"], json!([]));
}

#[test]
fn the_tasks_guard_emits_the_actionable_requirement_mapping_failure() {
    let (temp, home, slug) = mission_with_steps("software-dev", &pack_template(), None);
    let root = temp.path();
    let dir = root.join("kitty-specs").join(&slug);
    write(&dir.join("spec.md"), "# Spec\n\n- FR-001: do it\n");
    write(&dir.join("plan.md"), "# Plan\n");

    for expected in ["discovery", "specify", "plan", "tasks"] {
        let output = advance(root, &home, &slug);
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(single_json(&output)["step_id"], json!(expected));
    }

    write(
        &dir.join("tasks/WP01-demo.md"),
        "---\nwork_package_id: WP01\ntitle: WP01\ndependencies: []\nowned_files: [\"src/**\"]\n---\n\n# WP01\n",
    );
    write(&dir.join("tasks.md"), "# Tasks\n\n## WP01\n");

    // Two refuses: the mapping gate re-issues `tasks` (never advances) and the
    // failure stays actionable on every attempt.
    for _ in 0..2 {
        let output = advance(root, &home, &slug);
        assert_eq!(output.status.code(), Some(0));
    }
    let output = advance(root, &home, &slug);
    assert_eq!(output.status.code(), Some(0));
    let value = single_json(&output);
    assert_eq!(value["step_id"], json!("tasks"));
    let failures = value["guard_failures"].as_array().expect("failures");
    let mapping = failures
        .iter()
        .filter_map(Value::as_str)
        .find(|failure| failure.starts_with("Requirement mapping incomplete"))
        .expect("actionable requirement mapping failure");
    assert!(
        mapping.contains("missing refs for WPs: WP01")
            && mapping.contains("unmapped FRs: FR-001")
            && mapping.contains("spec-kitty agent tasks map-requirements --batch")
            && mapping.contains(&format!("--mission {slug}")),
        "mapping failure: {mapping}"
    );
}
