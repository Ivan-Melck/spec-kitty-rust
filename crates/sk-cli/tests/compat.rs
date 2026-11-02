//! End-to-end acceptance tests for the `spec-kitty` compatibility binary.
//!
//! Every test executes the real binary against a temporary project. Nothing
//! touches this checkout, the user's home directory, or a real mission.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};
use tempfile::TempDir;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_spec-kitty")
}

fn packs_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packs/built-in")
        .canonicalize()
        .expect("built-in pack tree")
}

fn run_in(dir: &Path, home: &Path, args: &[&str]) -> Output {
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

fn run(dir: &Path, home: &Path, args: &[&str]) -> Output {
    run_in(dir, home, args)
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

/// A configured project directory plus an isolated home.
fn project() -> (TempDir, PathBuf) {
    let temp = TempDir::new().expect("temp");
    let home = temp.path().join("home");
    fs::create_dir_all(&home).expect("home");
    (temp, home)
}

fn meta_json(id: &str, slug: &str, number: u32) -> String {
    json!({
        "mission_id": id,
        "mid8": &id[..8],
        "mission_slug": slug,
        "slug": slug,
        "mission_number": number,
        "mission_type": "software-dev",
        "friendly_name": "Test Feature",
        "target_branch": "main",
        "topology": "lanes",
        "created_at": "2026-02-08T12:00:00Z"
    })
    .to_string()
}

fn wp_markdown(id: &str, dependencies: &[&str]) -> String {
    format!(
        "---\nwork_package_id: {id}\ntitle: {id} task\nlane: planned\ndependencies: {}\nowned_files: [\"src/**\"]\n---\n\n# {id}\n",
        serde_json::to_string(dependencies).expect("deps")
    )
}

const EVENT_ULID: &str = "01HXYZ0000000000000000TEST";

fn event(wp_id: &str, from: &str, to: &str) -> String {
    json!({
        "event_id": EVENT_ULID,
        "mission_slug": "034-test-feature",
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

/// A mission with canonical events, WP files and status drift.
fn mission_project() -> (TempDir, PathBuf, String) {
    let (temp, home) = project();
    let slug = "034-test-feature".to_owned();
    let root = temp.path();
    write(
        &root.join(".kittify/config.yaml"),
        "agents:\n  available: [codex]\n",
    );
    let dir = root.join("kitty-specs").join(&slug);
    write(
        &dir.join("meta.json"),
        &meta_json("01HXYZ00000000000000000000", &slug, 34),
    );
    write(&dir.join("tasks/WP01-first.md"), &wp_markdown("WP01", &[]));
    write(
        &dir.join("tasks/WP02-second.md"),
        &wp_markdown("WP02", &["WP01"]),
    );
    write(
        &dir.join("status.events.jsonl"),
        &format!("{}\n", event("WP01", "planned", "claimed")),
    );
    (temp, home, slug)
}

/// A Git repository with a mission and a lane manifest.
fn lane_project() -> (TempDir, PathBuf, String) {
    let (temp, home) = project();
    let root = temp.path().to_path_buf();
    git(&root, &["init", "-q", "-b", "main"]);
    write(&root.join("README.md"), "fixture\n");
    write(&root.join("src/lib.rs"), "pub fn fixture() {}\n");
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "init"]);
    let slug = "m-01M6EXFH".to_owned();
    write(
        &root.join(".kittify/config.yaml"),
        "agents:\n  available: [codex]\n",
    );
    let dir = root.join("kitty-specs").join(&slug);
    write(
        &dir.join("meta.json"),
        &meta_json("01M6EXFHDZYWBGXWZDR9JW5NF5", &slug, 1),
    );
    write(&dir.join("tasks/WP01-a.md"), &wp_markdown("WP01", &[]));
    write(
        &dir.join("lanes.json"),
        &json!({
            "version": 1,
            "mission_slug": slug,
            "mission_id": "01M6EXFHDZYWBGXWZDR9JW5NF5",
            "mission_branch": format!("kitty/{slug}"),
            "target_branch": "main",
            "lanes": [{
                "lane_id": "lane-a",
                "wp_ids": ["WP01"],
                "write_scope": ["src/**"],
                "predicted_surfaces": ["src"],
                "depends_on_lanes": [],
                "parallel_group": 0
            }],
            "computed_at": "2026-01-01T00:00:00Z",
            "computed_from": "test",
            "planning_artifact_wps": [],
            "planning_commit_sha": null
        })
        .to_string(),
    );
    (temp, home, slug)
}

fn tree_snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, snapshot: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(dir).expect("fixture directory") {
            let entry = entry.expect("fixture entry");
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .expect("relative fixture path")
                .to_path_buf();
            let file_type = entry.file_type().expect("fixture file type");
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

#[test]
fn help_version_and_capabilities_are_stable() {
    let (temp, home) = project();
    let help = run(temp.path(), &home, &["--help"]);
    assert_eq!(help.status.code(), Some(0));
    let text = String::from_utf8(help.stdout).expect("UTF-8 help");
    for route in [
        "init",
        "specify",
        "plan",
        "tasks",
        "implement",
        "next",
        "agent",
        "runtime",
    ] {
        assert!(text.contains(route), "help missing {route}:\n{text}");
    }

    let version = run(temp.path(), &home, &["--version"]);
    assert_eq!(version.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&version.stdout).contains("spec-kitty"));
    let short = run(temp.path(), &home, &["-v"]);
    assert_eq!(short.status.code(), Some(0));

    let matrix = run(temp.path(), &home, &["capabilities", "--json"]);
    assert_eq!(matrix.status.code(), Some(0));
    let value = single_json(&matrix);
    assert_eq!(value["python_runtime"], json!(false));
    let commands: Vec<&str> = value["commands"]
        .as_array()
        .expect("commands")
        .iter()
        .map(|command| command["name"].as_str().expect("name"))
        .collect();
    for route in [
        "init",
        "agent status validate",
        "agent status materialize",
        "agent tasks list-tasks",
        "agent mission create",
        "implement",
        "next",
    ] {
        assert!(commands.contains(&route), "missing {route}: {commands:?}");
    }
    let unsupported: Vec<&str> = value["unsupported"]
        .as_array()
        .expect("unsupported")
        .iter()
        .map(|command| command["name"].as_str().expect("name"))
        .collect();
    assert!(unsupported.contains(&"upgrade"));
    assert!(unsupported.contains(&"agent status emit"));
}

#[test]
fn init_scaffolds_a_project_and_is_idempotent() {
    let (temp, home) = project();
    let root = temp.path();
    let first = run(root, &home, &["init", "--ai", "codex", "--non-interactive"]);
    assert_eq!(
        first.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(root.join(".kittify/config.yaml").is_file());
    assert!(root.join(".kittify/metadata.yaml").is_file());
    assert!(root.join(".agents/skills").is_dir());

    let second = run(root, &home, &["init", "--ai", "codex", "--non-interactive"]);
    assert_eq!(second.status.code(), Some(0));
    let text = String::from_utf8_lossy(&second.stdout).to_lowercase();
    assert!(text.contains("already initialized"), "output: {text}");
}

#[test]
fn init_without_ai_is_refused_in_non_interactive_mode() {
    let (temp, home) = project();
    let output = run(temp.path(), &home, &["init", "--non-interactive"]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
    assert!(stderr.contains("--ai is required"), "stderr: {stderr}");
}

#[test]
fn init_rejects_an_invalid_agent_before_writing() {
    let (temp, home) = project();
    let output = run(
        temp.path(),
        &home,
        &["init", "--ai", "not-an-agent", "--non-interactive"],
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(!temp.path().join(".kittify").exists());
}

#[test]
fn status_materialize_and_validate_use_the_canonical_reducer() {
    let (temp, home, slug) = mission_project();
    let root = temp.path();
    let mission_dir = root.join("kitty-specs").join(&slug);

    let validate_before = run(
        root,
        &home,
        &["agent", "status", "validate", "--mission", &slug, "--json"],
    );
    assert_eq!(validate_before.status.code(), Some(1));
    let before = single_json(&validate_before);
    assert_eq!(before["passed"], json!(false));
    assert!(
        before["errors"]
            .as_array()
            .expect("errors")
            .iter()
            .any(|error| error.as_str().unwrap_or("").contains("status.json")),
        "missing drift error: {before}"
    );

    let materialize = run(
        root,
        &home,
        &[
            "agent",
            "status",
            "materialize",
            "--mission",
            &slug,
            "--json",
        ],
    );
    assert_eq!(materialize.status.code(), Some(0));
    let snapshot = single_json(&materialize);
    assert_eq!(snapshot["mission_slug"], json!(slug));
    assert_eq!(snapshot["event_count"], json!(1));
    assert_eq!(snapshot["work_packages"]["WP01"]["lane"], json!("claimed"));
    let on_disk: Value =
        serde_json::from_str(&fs::read_to_string(mission_dir.join("status.json")).expect("status"))
            .expect("status json");
    assert_eq!(on_disk, snapshot);

    let validate_after = run(
        root,
        &home,
        &["agent", "status", "validate", "--mission", &slug, "--json"],
    );
    assert_eq!(validate_after.status.code(), Some(0));
    let after = single_json(&validate_after);
    assert_eq!(after["passed"], json!(true));
    assert_eq!(after["error_count"], json!(0));
}

#[test]
fn status_validate_detects_illegal_transitions_and_never_mutates() {
    let (temp, home, slug) = mission_project();
    let root = temp.path();
    let events = root
        .join("kitty-specs")
        .join(&slug)
        .join("status.events.jsonl");
    write(
        &events,
        &format!(
            "{}\n{}\n",
            event("WP01", "planned", "claimed"),
            event("WP01", "claimed", "done")
        ),
    );
    let before = tree_snapshot(root);
    let output = run(
        root,
        &home,
        &["agent", "status", "validate", "--mission", &slug, "--json"],
    );
    assert_eq!(output.status.code(), Some(1));
    let value = single_json(&output);
    assert!(
        value["errors"]
            .as_array()
            .expect("errors")
            .iter()
            .any(|error| error.as_str().unwrap_or("").contains("planned")
                || error.as_str().unwrap_or("").contains("approved")
                || error.as_str().unwrap_or("").contains("done")),
        "expected a legality error: {value}"
    );
    assert_eq!(tree_snapshot(root), before, "validate must not mutate");
}

#[test]
fn status_requires_an_explicit_mission_and_reports_unknown_handles() {
    let (temp, home, _slug) = mission_project();
    let root = temp.path();

    let missing = run(root, &home, &["agent", "status", "validate", "--json"]);
    assert_eq!(missing.status.code(), Some(1));
    let value = single_json(&missing);
    assert_eq!(value["error"], json!("--mission <slug> is required"));

    let unknown = run(
        root,
        &home,
        &[
            "agent",
            "status",
            "validate",
            "--mission",
            "999-missing",
            "--json",
        ],
    );
    assert_eq!(unknown.status.code(), Some(1));
    let value = single_json(&unknown);
    assert_eq!(value["error_code"], json!("MISSION_NOT_FOUND"));
}

#[test]
fn task_reads_use_canonical_lanes_and_dependency_edges() {
    let (temp, home, slug) = mission_project();
    let root = temp.path();
    let _ = run(
        root,
        &home,
        &[
            "agent",
            "status",
            "materialize",
            "--mission",
            &slug,
            "--json",
        ],
    );

    let listed = run(
        root,
        &home,
        &["agent", "tasks", "list-tasks", "--mission", &slug, "--json"],
    );
    assert_eq!(listed.status.code(), Some(0));
    let value = single_json(&listed);
    assert_eq!(value["count"], json!(2));
    assert_eq!(value["tasks"][0]["work_package_id"], json!("WP01"));
    assert_eq!(value["tasks"][0]["lane"], json!("claimed"));
    assert_eq!(value["tasks"][1]["lane"], json!("planned"));

    let filtered = run(
        root,
        &home,
        &[
            "agent",
            "tasks",
            "list-tasks",
            "--mission",
            &slug,
            "--lane",
            "claimed",
            "--json",
        ],
    );
    let value = single_json(&filtered);
    assert_eq!(value["count"], json!(1));
    assert_eq!(value["tasks"][0]["work_package_id"], json!("WP01"));

    let board = run(
        root,
        &home,
        &["agent", "tasks", "status", "--mission", &slug, "--json"],
    );
    assert_eq!(board.status.code(), Some(0));
    let value = single_json(&board);
    assert_eq!(value["mission_slug"], json!(slug));
    assert_eq!(value["total_wps"], json!(2));
    assert_eq!(value["by_lane"]["claimed"], json!(1));
    assert_eq!(value["by_lane"]["planned"], json!(1));
    assert_eq!(value["done_count"], json!(0));
    assert_eq!(value["progress_semantics"], json!("weighted_readiness"));
    // Weighted readiness comes from the canonical snapshot, which holds only
    // WP01 here (one claimed WP = 5.0%); the frontmatter-only WP02 is planned.
    assert_eq!(value["progress_percentage"], json!(5.0));
    assert_eq!(value["work_packages"][0]["id"], json!("WP01"));

    let dependents = run(
        root,
        &home,
        &[
            "agent",
            "tasks",
            "list-dependents",
            "WP01",
            "--mission",
            &slug,
            "--json",
        ],
    );
    assert_eq!(dependents.status.code(), Some(0));
    let value = single_json(&dependents);
    assert_eq!(value["wp_id"], json!("WP01"));
    assert_eq!(value["depends_on"], json!([]));
    assert_eq!(value["dependents"], json!(["WP02"]));
}

#[test]
fn specify_plan_and_tasks_form_a_real_authoring_slice() {
    let (temp, home) = project();
    let root = temp.path();
    git(root, &["init", "-q", "-b", "main"]);
    write(&root.join("README.md"), "fixture\n");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "init"]);
    let init = run(root, &home, &["init", "--ai", "codex", "--non-interactive"]);
    assert_eq!(init.status.code(), Some(0));

    let specify = run(root, &home, &["specify", "User Auth", "--json"]);
    assert_eq!(
        specify.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&specify.stderr)
    );
    let created = single_json(&specify);
    assert_eq!(created["result"], json!("success"));
    assert_eq!(created["mission_type"], json!("software-dev"));
    assert_eq!(created["scaffold_only"], json!(true));
    assert_eq!(created["allocation"], json!("planning_artifacts_only"));
    assert_eq!(created["target_branch"], json!("main"));
    let slug = created["mission_slug"].as_str().expect("slug").to_owned();
    assert!(slug.starts_with("user-auth-"), "slug: {slug}");
    let mission_dir = root.join("kitty-specs").join(&slug);
    let spec = fs::read_to_string(mission_dir.join("spec.md")).expect("spec");
    assert!(spec.contains("Mission Specification"), "spec: {spec}");
    assert!(mission_dir.join("meta.json").is_file());

    // A second identical specify is refused by the idempotency guard.
    let duplicate = run(root, &home, &["specify", "User Auth", "--json"]);
    assert_eq!(duplicate.status.code(), Some(1));
    let value = single_json(&duplicate);
    assert!(
        value["error"]
            .as_str()
            .is_some_and(|error| error.contains("already"))
    );

    let plan = run(root, &home, &["plan", "--mission", &slug, "--json"]);
    assert_eq!(plan.status.code(), Some(0));
    let value = single_json(&plan);
    assert_eq!(value["result"], json!("success"));
    assert_eq!(value["scaffold_written"], json!(true));
    let plan_md = fs::read_to_string(mission_dir.join("plan.md")).expect("plan");
    assert!(plan_md.contains("Implementation Plan"), "plan: {plan_md}");

    write(
        &mission_dir.join("tasks.md"),
        "# Tasks\n\n## WP01\n\n- Dependencies: none\n",
    );
    write(
        &mission_dir.join("tasks/WP01-only.md"),
        &wp_markdown("WP01", &[]),
    );
    let tasks = run(root, &home, &["tasks", "--mission", &slug, "--json"]);
    assert_eq!(tasks.status.code(), Some(0));
    let value = single_json(&tasks);
    assert_eq!(value["result"], json!("success"));
    assert_eq!(value["dependencies"]["WP01"], json!([]));
    assert_eq!(value["committed"], json!(false));
}

#[test]
fn coordinated_topology_is_refused_before_any_mutation() {
    let (temp, home) = project();
    let root = temp.path();
    git(root, &["init", "-q", "-b", "main"]);
    write(&root.join("README.md"), "fixture\n");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "init"]);
    assert_eq!(
        run(root, &home, &["init", "--ai", "codex", "--non-interactive"])
            .status
            .code(),
        Some(0)
    );
    let before = tree_snapshot(root);
    let output = run(
        root,
        &home,
        &["specify", "Coord Mission", "--topology", "coord", "--json"],
    );
    assert_eq!(output.status.code(), Some(2));
    let value = single_json(&output);
    assert_eq!(
        value["error_code"],
        json!("COORDINATION_TOPOLOGY_UNSUPPORTED")
    );
    assert_eq!(tree_snapshot(root), before, "refusal must not mutate");
}

#[test]
fn unsupported_required_options_are_refused_before_mutation() {
    let (temp, home) = project();
    let root = temp.path();
    git(root, &["init", "-q", "-b", "main"]);
    write(&root.join("README.md"), "fixture\n");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "init"]);
    assert_eq!(
        run(root, &home, &["init", "--ai", "codex", "--non-interactive"])
            .status
            .code(),
        Some(0)
    );
    let before = tree_snapshot(root);
    let output = run(
        root,
        &home,
        &[
            "agent",
            "mission",
            "create",
            "blocked-mission",
            "--start-branch",
            "feature",
            "--json",
        ],
    );
    assert_eq!(output.status.code(), Some(2));
    let value = single_json(&output);
    assert_eq!(value["error_code"], json!("UNSUPPORTED_REQUIRED_OPTION"));
    assert_eq!(value["option"], json!("--start-branch"));
    assert_eq!(tree_snapshot(root), before, "refusal must not mutate");
}

#[test]
fn implement_allocates_real_lane_worktrees_and_reuses_them() {
    let (temp, home, slug) = lane_project();
    let root = temp.path();

    let first = run(
        root,
        &home,
        &["implement", "WP01", "--mission", &slug, "--json"],
    );
    assert_eq!(
        first.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let value = single_json(&first);
    assert_eq!(value["wp_id"], json!("WP01"));
    assert_eq!(value["lane_id"], json!("lane-a"));
    assert_eq!(value["execution_mode"], json!("worktree"));
    assert_eq!(value["status"], json!("created"));
    assert_eq!(value["workspace"], json!(".worktrees/m-01M6EXFH-lane-a"));
    let worktree = root.join(value["workspace"].as_str().expect("workspace"));
    assert!(worktree.is_dir());
    let branch = value["branch"].as_str().expect("branch").to_owned();
    let list = Command::new("git")
        .current_dir(root)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .expect("worktree list");
    assert!(
        String::from_utf8_lossy(&list.stdout).contains(&branch),
        "branch {branch} not registered"
    );

    let second = run(
        root,
        &home,
        &["implement", "WP01", "--mission", &slug, "--json"],
    );
    assert_eq!(second.status.code(), Some(0));
    let value = single_json(&second);
    assert_eq!(value["status"], json!("reused"));
    assert_eq!(value["allocation_route"], json!("reuse"));
}

#[test]
fn implement_refuses_a_protected_lane_branch_before_allocating() {
    let (temp, home, slug) = lane_project();
    let root = temp.path();
    let manifest_path = root.join("kitty-specs").join(&slug).join("lanes.json");
    let mut manifest: Value =
        serde_json::from_str(&fs::read_to_string(&manifest_path).expect("lanes")).expect("json");
    manifest["mission_branch"] = json!("main");
    write(&manifest_path, &manifest.to_string());

    let output = run(
        root,
        &home,
        &["implement", "WP01", "--mission", &slug, "--json"],
    );
    assert_eq!(output.status.code(), Some(1));
    let value = single_json(&output);
    assert_eq!(value["error_code"], json!("PROTECTED_BRANCH_REFUSED"));
    assert!(!root.join(".worktrees").exists());
}

#[test]
fn coordination_materialization_refuses_before_writing_the_primary_surface() {
    let (temp, home, slug) = mission_project();
    let root = temp.path();
    let path = root.join("kitty-specs").join(&slug).join("meta.json");
    let mut meta: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    meta["coordination_branch"] = json!("kitty/coord-recorded");
    write(&path, &meta.to_string());
    let before = tree_snapshot(root);
    let output = run(
        root,
        &home,
        &[
            "agent",
            "status",
            "materialize",
            "--mission",
            &slug,
            "--json",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        single_json(&output)["error_code"],
        "COORDINATION_SURFACE_UNAVAILABLE"
    );
    assert_eq!(tree_snapshot(root), before);
}

#[test]
fn next_and_runtime_share_one_durable_run() {
    let (temp, home, slug) = mission_project();
    let root = temp.path();
    let _ = run(
        root,
        &home,
        &[
            "agent",
            "status",
            "materialize",
            "--mission",
            &slug,
            "--json",
        ],
    );

    let start = run(
        root,
        &home,
        &["runtime", "start", "--mission", &slug, "--json"],
    );
    assert_eq!(
        start.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&start.stderr)
    );
    let started = single_json(&start);
    assert_eq!(started["started"], json!(true));
    let run_id = started["run"]["run_id"]
        .as_str()
        .expect("run id")
        .to_owned();
    assert!(root.join(".kittify/runtime").is_dir());

    let query = run(root, &home, &["next", "--mission", &slug, "--json"]);
    assert_eq!(query.status.code(), Some(0));
    let value = single_json(&query);
    assert_eq!(value["kind"], json!("query"));
    assert_eq!(value["is_query"], json!(true));
    assert_eq!(value["run_id"], json!(run_id));
    assert!(value["preview_step"].as_str().is_some());
    assert_eq!(value["prompt_file"], Value::Null);

    let advanced = run(
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
    assert!(
        matches!(advanced.status.code(), Some(0) | Some(1)),
        "unexpected exit: {}",
        String::from_utf8_lossy(&advanced.stderr)
    );
    let value = single_json(&advanced);
    assert_eq!(value["is_query"], json!(false));
    assert_eq!(value["run_id"], json!(run_id));
    if value["kind"] == json!("step") {
        let prompt = value["prompt_file"].as_str().expect("step prompt file");
        assert!(Path::new(prompt).is_file(), "prompt missing: {prompt}");
    }

    let query_after = run(
        root,
        &home,
        &["runtime", "query", "--mission", &slug, "--json"],
    );
    assert_eq!(query_after.status.code(), Some(0));
    let value = single_json(&query_after);
    assert_eq!(value["run"]["run_id"], json!(run_id));
    assert_eq!(value["consistency"]["healthy"], json!(true));
}

#[test]
fn unknown_and_unimplemented_routes_fail_with_the_usage_class() {
    let (temp, home) = project();
    let unknown = run(temp.path(), &home, &["not-a-command"]);
    assert_eq!(unknown.status.code(), Some(2));

    let upgrade = run(temp.path(), &home, &["upgrade", "--json"]);
    assert_eq!(upgrade.status.code(), Some(2));
    let value = single_json(&upgrade);
    assert_eq!(value["error_code"], json!("NOT_IMPLEMENTED"));
    assert!(
        value["error"]
            .as_str()
            .is_some_and(|error| error.contains("upgrade"))
    );

    let emit = run(temp.path(), &home, &["agent", "status", "emit", "--json"]);
    assert_eq!(emit.status.code(), Some(2));
    let value = single_json(&emit);
    assert_eq!(value["error_code"], json!("NOT_IMPLEMENTED"));
}

#[test]
fn native_passthrough_serves_the_versioned_inspection_envelope() {
    let (temp, home) = project();
    let root = temp.path();
    fs::create_dir_all(root.join(".kittify")).expect(".kittify");
    let output = run(root, &home, &["native", "project", "root", "--json"]);
    assert_eq!(output.status.code(), Some(0));
    let value = single_json(&output);
    assert_eq!(value["schema_version"], json!(1));
    assert_eq!(value["command"], json!("project.root"));
    assert_eq!(value["data"]["root"], json!(root.to_string_lossy()));
}

#[test]
fn human_and_json_modes_use_the_same_exit_classes() {
    let (temp, home, slug) = mission_project();
    let root = temp.path();
    let human = run(
        root,
        &home,
        &["agent", "status", "materialize", "--mission", &slug],
    );
    assert_eq!(human.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&human.stdout).contains("Materialized"));

    let json = run(
        root,
        &home,
        &[
            "agent",
            "status",
            "materialize",
            "--mission",
            &slug,
            "--json",
        ],
    );
    assert_eq!(json.status.code(), Some(0));
    let _ = single_json(&json);

    let failing_human = run(
        root,
        &home,
        &[
            "agent",
            "status",
            "validate",
            "--mission",
            "missing-mission",
        ],
    );
    assert_eq!(failing_human.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&failing_human.stderr).contains("Error:"));
}

#[test]
fn init_with_a_slash_command_agent_renders_canonical_prompts_from_the_pack() {
    let (temp, home) = project();
    let root = temp.path();
    let output = run(
        root,
        &home,
        &["init", "--ai", "claude", "--non-interactive"],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let command = root.join(".claude/commands/spec-kitty.specify.md");
    let text = fs::read_to_string(&command).expect("generated specify command");
    assert!(
        text.contains("spec-kitty agent mission create"),
        "generated prompt must invoke the native CLI:\n{text}"
    );
    assert!(
        text.contains("spec-kitty next"),
        "generated prompt:\n{text}"
    );
}

#[test]
fn branch_context_is_a_read_only_deterministic_contract() {
    let (temp, home) = project();
    let root = temp.path();
    git(root, &["init", "-q", "-b", "main"]);
    write(&root.join("README.md"), "fixture\n");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "init"]);
    let before = tree_snapshot(root);
    let output = run(
        root,
        &home,
        &["agent", "mission", "branch-context", "--json"],
    );
    assert_eq!(output.status.code(), Some(0));
    let value = single_json(&output);
    assert_eq!(value["result"], json!("success"));
    assert_eq!(value["current_branch"], json!("main"));
    assert_eq!(value["target_branch_source"], json!("current_branch"));
    assert_eq!(value["recommended_strategy"], json!("feature-branch"));
    assert_eq!(value["branch_context"]["primary_branch"], json!("main"));
    assert_eq!(
        tree_snapshot(root),
        before,
        "branch-context must not mutate"
    );
}
