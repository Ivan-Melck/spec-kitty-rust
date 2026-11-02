//! Acceptance tests for the partial-rewrite `spec-kitty-rust` binary.
//!
//! Every test executes the real binary (`CARGO_BIN_EXE_spec-kitty-rust`) against
//! a temporary filesystem fixture. The tests assert the stable JSON envelope,
//! exit-code classes, non-mutation, and the stdout discipline: one JSON
//! document on stdout, diagnostics on stderr.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};
use tempfile::TempDir;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_spec-kitty-rust")
}

fn run_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(binary())
        .current_dir(dir)
        .env_remove("SPECIFY_REPO_ROOT")
        .env_remove("GIT_CEILING_DIRECTORIES")
        .env_remove("SPEC_KITTY_PACKS_ROOT")
        .args(args)
        .output()
        .expect("run spec-kitty-rust")
}

fn run(args: &[&str]) -> Output {
    run_in(Path::new("."), args)
}

fn path(path: &Path) -> &str {
    path.to_str().expect("UTF-8 fixture path")
}

/// Assert stdout holds exactly one JSON document and return it.
fn single_json(output: &Output) -> Value {
    let text = String::from_utf8(output.stdout.clone()).expect("stdout is UTF-8");
    assert!(
        !text.trim().is_empty(),
        "expected one JSON document on stdout; stderr was:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_str(&text).unwrap_or_else(|error| {
        panic!("stdout was not exactly one JSON document ({error}):\n{text}")
    });
    assert!(
        value.is_object(),
        "top-level JSON must be an object: {value}"
    );
    value
}

fn assert_ok(output: &Output, expected_command: &str, expected_exit: i32) -> Value {
    assert_eq!(output.status.code(), Some(expected_exit), "exit code");
    let value = single_json(output);
    assert_eq!(value["ok"], json!(true), "envelope: {value}");
    assert_eq!(value["command"], json!(expected_command));
    assert_eq!(value["exit_code"], json!(expected_exit));
    assert_eq!(value["schema_version"], json!(1));
    assert!(value.get("data").is_some(), "data present: {value}");
    value
}

fn assert_error(
    output: &Output,
    expected_command: &str,
    expected_exit: i32,
    expected_kind: &str,
) -> Value {
    assert_eq!(output.status.code(), Some(expected_exit), "exit code");
    let value = single_json(output);
    assert_eq!(value["ok"], json!(false), "envelope: {value}");
    assert_eq!(value["command"], json!(expected_command));
    assert_eq!(value["exit_code"], json!(expected_exit));
    assert_eq!(value["error"]["kind"], json!(expected_kind));
    assert!(
        value["error"]["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "error message present: {value}"
    );
    assert!(
        !output.stderr.is_empty(),
        "domain/usage failures also write a diagnostic to stderr"
    );
    value
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create fixture parent");
    }
    fs::write(path, contents).expect("write fixture");
}

fn tree_snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, snapshot: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(dir).expect("read fixture directory") {
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
                snapshot.insert(relative, fs::read(&path).expect("read fixture file"));
            }
        }
    }

    let mut snapshot = BTreeMap::new();
    walk(root, root, &mut snapshot);
    snapshot
}

const MISSION_ONE: &str = "01KN2371WNZX28ME4MN97B67PJ";
const MISSION_TWO: &str = "01KN2372WNZX28ME4MN97B67PK";

fn meta_json(id: &str, slug: &str, number: u32) -> String {
    format!(
        r#"{{
  "created_at": "2026-11-02T10:00:00Z",
  "flattened": false,
  "friendly_name": "{slug}",
  "mission_id": "{id}",
  "mission_number": {number},
  "mission_slug": "{slug}",
  "mission_type": "software-dev",
  "slug": "{slug}",
  "target_branch": "main",
  "topology": "single_branch"
}}
"#
    )
}

fn wp_markdown(id: &str, dependencies: &str) -> String {
    format!(
        "---\nwork_package_id: {id}\ntitle: Fixture {id}\ndependencies: {dependencies}\n---\n\n# Work Package {id}\n\nBody text.\n"
    )
}

/// A project with two Missions, one of which has a two-node WP graph.
fn mission_project() -> TempDir {
    let temp = TempDir::new().expect("temporary project");
    fs::create_dir_all(temp.path().join(".kittify")).expect("create .kittify");

    let alpha = temp.path().join("kitty-specs/001-alpha-01KN2371");
    write(
        &alpha.join("meta.json"),
        &meta_json(MISSION_ONE, "001-alpha-01KN2371", 1),
    );
    write(
        &alpha.join("tasks/WP01-alpha.md"),
        &wp_markdown("WP01", "[]"),
    );
    write(
        &alpha.join("tasks/WP02-beta.md"),
        &wp_markdown("WP02", "[WP01]"),
    );

    let beta = temp.path().join("kitty-specs/002-beta-01KN2372");
    write(
        &beta.join("meta.json"),
        &meta_json(MISSION_TWO, "002-beta-01KN2372", 2),
    );

    temp
}

fn charter_project() -> TempDir {
    let temp = TempDir::new().expect("temporary project");
    write(
        &temp.path().join(".kittify/charter/charter.md"),
        "# Fixture Charter\n\nCurated prose, never rewritten.\n",
    );
    write(
        &temp.path().join(".kittify/charter/charter.yaml"),
        "charter:\n  name: fixture\n  version: 1\n",
    );
    fs::create_dir_all(temp.path().join("kitty-specs")).expect("kitty-specs");
    temp
}

fn glossary_project() -> TempDir {
    let temp = TempDir::new().expect("temporary project");
    write(
        &temp.path().join(".kittify/glossaries/team_domain.yaml"),
        "terms:\n  - surface: workspace\n    definition: A checked-out work area.\n    confidence: 1.0\n    status: active\n",
    );
    write(
        &temp.path().join(".kittify/glossaries/mission_local.yaml"),
        "terms:\n  - surface: workspace\n    definition: A mission-specific working directory.\n    confidence: 1.0\n    status: active\n",
    );
    fs::create_dir_all(temp.path().join("kitty-specs")).expect("kitty-specs");
    temp
}

fn runtime_project() -> TempDir {
    let temp = TempDir::new().expect("temporary project");
    write(
        &temp.path().join("template.yaml"),
        "mission:\n  key: software-dev\n  name: Software Dev\n  version: \"1.0.0\"\nsteps:\n  - id: specify\n    title: Specify\n    description: Write the specification.\n  - id: plan\n    title: Plan\n    description: Write the plan.\n    depends_on: [specify]\n",
    );
    write(
        &temp.path().join("state.json"),
        "{\n  \"run_id\": \"run-1\",\n  \"mission_key\": \"software-dev\",\n  \"template_hash\": \"frozen-hash\",\n  \"completed_steps\": []\n}\n",
    );
    write(
        &temp.path().join("blocked-state.json"),
        "{\n  \"run_id\": \"run-1\",\n  \"mission_key\": \"software-dev\",\n  \"template_hash\": \"frozen-hash\",\n  \"blocked_reason\": \"operator decision pending\"\n}\n",
    );
    write(
        &temp.path().join("terminal-state.json"),
        "{\n  \"run_id\": \"run-1\",\n  \"mission_key\": \"software-dev\",\n  \"template_hash\": \"frozen-hash\",\n  \"completed_steps\": [\"specify\", \"plan\"]\n}\n",
    );
    temp
}

fn events_file(temp: &TempDir) -> PathBuf {
    let events = temp.path().join("status.events.jsonl");
    write(
        &events,
        concat!(
            "{\"event_id\":\"01ARZ3NDEKTSV4RRFFQ69G5FAV\",\"mission_slug\":\"001-alpha\",\"wp_id\":\"WP01\",\"from_lane\":\"planned\",\"to_lane\":\"claimed\",\"at\":\"2026-11-02T10:00:00Z\",\"actor\":\"agent-1\",\"force\":false,\"execution_mode\":\"worktree\"}\n",
            "{\"kind\":\"annotation\",\"event_id\":\"01ARZ3NDEKTSV4RRFFQ69G5FAW\",\"wp_id\":\"WP01\",\"at\":\"2026-11-02T10:01:00Z\",\"actor\":\"agent-1\",\"delta\":{\"note\":\"checked\"}}\n",
            "{\"event_type\":\"MissionCreated\",\"mission_slug\":\"001-alpha\"}\n"
        ),
    );
    events
}

#[test]
fn capabilities_reports_an_accurate_partial_rewrite_matrix() {
    let temp = TempDir::new().expect("temp");
    let output = run_in(temp.path(), &["capabilities", "--json"]);
    let value = assert_ok(&output, "capabilities", 0);
    let data = &value["data"];

    assert_eq!(data["implementation"], json!("partial-rewrite"));
    assert_eq!(data["python_runtime"], json!(false));
    assert_eq!(data["mutates_project"], json!(false));
    assert_eq!(data["full_next"], json!(false));
    assert_eq!(data["event_ledger_writes"], json!(false));

    let names: Vec<&str> = data["commands"]
        .as_array()
        .expect("commands matrix")
        .iter()
        .map(|entry| entry["name"].as_str().expect("command name"))
        .collect();
    for expected in [
        "capabilities",
        "project root",
        "git inspect",
        "mission list",
        "mission show",
        "mission graph",
        "charter inspect",
        "glossary lookup",
        "runtime query",
        "events inspect",
        "transition check",
    ] {
        assert!(names.contains(&expected), "missing {expected}: {names:?}");
    }

    let unsupported: Vec<&str> = data["unsupported"]
        .as_array()
        .expect("unsupported matrix")
        .iter()
        .map(|entry| entry["name"].as_str().expect("unsupported name"))
        .collect();
    for expected in [
        "init",
        "upgrade",
        "worktrees",
        "merge",
        "activation",
        "event-writes",
        "next",
    ] {
        assert!(
            unsupported.contains(&expected),
            "missing unsupported entry {expected}: {unsupported:?}"
        );
    }
    for command in &names {
        assert!(
            !unsupported.contains(command),
            "a command cannot be both implemented and unsupported: {command}"
        );
    }
}

#[test]
fn help_states_the_partial_rewrite_and_limits() {
    let output = run(&["--help"]);
    assert_eq!(output.status.code(), Some(0));
    let text = String::from_utf8(output.stdout).expect("UTF-8 help");
    let lowered = text.to_lowercase();
    assert!(lowered.contains("partial"), "help: {text}");
    assert!(lowered.contains("rewrite"), "help: {text}");
    assert!(
        lowered.contains("read-only") || lowered.contains("read only"),
        "help: {text}"
    );
    assert!(lowered.contains("init"), "help: {text}");
    assert!(lowered.contains("next"), "help: {text}");
}

#[test]
fn project_root_is_discovered_from_a_nested_directory() {
    let temp = TempDir::new().expect("temp");
    fs::create_dir_all(temp.path().join(".kittify")).expect(".kittify");
    let nested = temp.path().join("a/b/c");
    fs::create_dir_all(&nested).expect("nested");

    let output = run_in(&nested, &["project", "root", "--json"]);
    let value = assert_ok(&output, "project.root", 0);
    let expected = fs::canonicalize(temp.path()).expect("canonical root");
    assert_eq!(
        value["data"]["root"],
        json!(expected.to_string_lossy().into_owned())
    );
}

#[test]
fn project_root_flag_is_authoritative_for_commands() {
    let temp = mission_project();
    let output = run_in(
        Path::new("/"),
        &[
            "--project-root",
            path(temp.path()),
            "mission",
            "list",
            "--json",
        ],
    );
    let value = assert_ok(&output, "mission.list", 0);
    assert_eq!(value["data"]["count"], json!(2));
}

#[test]
fn mission_list_show_and_graph_use_strict_selectors() {
    let temp = mission_project();

    let list = run_in(
        temp.path(),
        &["--project-root", path(temp.path()), "mission", "list"],
    );
    let list = assert_ok(&list, "mission.list", 0);
    assert_eq!(list["data"]["count"], json!(2));
    let slugs: Vec<&str> = list["data"]["missions"]
        .as_array()
        .expect("missions array")
        .iter()
        .map(|mission| mission["slug"].as_str().expect("slug"))
        .collect();
    assert_eq!(slugs, vec!["001-alpha-01KN2371", "002-beta-01KN2372"]);
    assert_eq!(
        list["data"]["missions"][0]["mission_id"],
        json!(MISSION_ONE)
    );
    assert_eq!(list["data"]["missions"][0]["mid8"], json!("01KN2371"));

    for selector in [MISSION_ONE, "01KN2371", "001-alpha-01KN2371"] {
        let show = run_in(
            temp.path(),
            &[
                "--project-root",
                path(temp.path()),
                "mission",
                "show",
                selector,
            ],
        );
        let show = assert_ok(&show, "mission.show", 0);
        assert_eq!(show["data"]["mission"]["mission_id"], json!(MISSION_ONE));
        assert_eq!(show["data"]["work_packages"]["count"], json!(2));
    }

    let graph = run_in(
        temp.path(),
        &[
            "--project-root",
            path(temp.path()),
            "mission",
            "graph",
            MISSION_ONE,
        ],
    );
    let graph = assert_ok(&graph, "mission.graph", 0);
    assert_eq!(
        graph["data"]["graph"]["topological_order"],
        json!(["WP01", "WP02"])
    );
    assert_eq!(graph["data"]["graph"]["packages"][1]["id"], json!("WP02"));
    assert_eq!(
        graph["data"]["graph"]["packages"][1]["dependencies"],
        json!(["WP01"])
    );
    assert_eq!(
        graph["data"]["graph"]["edges"],
        json!([{"dependency": "WP01", "dependent": "WP02"}])
    );
}

#[test]
fn mission_selectors_refuse_traversal_absence_and_ambiguity() {
    let temp = mission_project();

    let traversal = run_in(
        temp.path(),
        &[
            "--project-root",
            path(temp.path()),
            "mission",
            "show",
            "../001-alpha-01KN2371",
        ],
    );
    assert_error(&traversal, "mission.show", 1, "domain");

    let missing = run_in(
        temp.path(),
        &[
            "--project-root",
            path(temp.path()),
            "mission",
            "show",
            "009-nope",
        ],
    );
    let missing = assert_error(&missing, "mission.show", 1, "domain");
    assert!(
        missing["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("no mission matches")),
        "missing selector message: {missing}"
    );

    // There is no sole-mission fallback: listing an empty project is fine, but
    // selecting from it is a domain failure.
    let empty = TempDir::new().expect("empty project");
    fs::create_dir_all(empty.path().join("kitty-specs")).expect("kitty-specs");
    let empty_list = run_in(
        empty.path(),
        &["--project-root", path(empty.path()), "mission", "list"],
    );
    let empty_list = assert_ok(&empty_list, "mission.list", 0);
    assert_eq!(empty_list["data"]["count"], json!(0));
    let empty_show = run_in(
        empty.path(),
        &[
            "--project-root",
            path(empty.path()),
            "mission",
            "show",
            "only-one",
        ],
    );
    assert_error(&empty_show, "mission.show", 1, "domain");
}

#[test]
fn git_inspect_is_read_only_and_reports_real_state() {
    if Command::new("git").arg("--version").output().is_err() {
        eprintln!("git is unavailable; skipping git acceptance test");
        return;
    }

    let temp = TempDir::new().expect("temp");
    fs::create_dir_all(temp.path().join(".kittify")).expect(".kittify");
    write(&temp.path().join("tracked.txt"), "one\n");
    let init = Command::new("git")
        .current_dir(temp.path())
        .args(["init", "--quiet"])
        .status()
        .expect("git init");
    assert!(init.success(), "git init");
    for (key, value) in [
        ("user.email", "fixture@example.com"),
        ("user.name", "Fixture"),
    ] {
        let status = Command::new("git")
            .current_dir(temp.path())
            .args(["config", key, value])
            .status()
            .expect("git config");
        assert!(status.success(), "git config {key}");
    }
    for args in [
        vec!["add", "tracked.txt"],
        vec!["commit", "--quiet", "-m", "fixture"],
    ] {
        let status = Command::new("git")
            .current_dir(temp.path())
            .args(args)
            .status()
            .expect("git command");
        assert!(status.success(), "git fixture command");
    }

    let before_clean = tree_snapshot(temp.path());
    let clean = run_in(
        temp.path(),
        &["--project-root", path(temp.path()), "git", "inspect"],
    );
    let clean = assert_ok(&clean, "git.inspect", 0);
    assert!(
        clean["data"]["branch"]
            .as_str()
            .is_some_and(|branch| !branch.is_empty()),
        "branch: {clean}"
    );
    assert_eq!(clean["data"]["dirty"], json!(false));
    assert_eq!(clean["data"]["counts"]["total"], json!(0));
    assert!(
        clean["data"]["head"]
            .as_str()
            .is_some_and(|head| head.len() >= 7)
    );
    assert_eq!(
        before_clean,
        tree_snapshot(temp.path()),
        "clean inspect must not rewrite files"
    );

    write(&temp.path().join("tracked.txt"), "two\n");
    let before_dirty = tree_snapshot(temp.path());
    let dirty = run_in(
        temp.path(),
        &["--project-root", path(temp.path()), "git", "inspect"],
    );
    let dirty = assert_ok(&dirty, "git.inspect", 0);
    assert_eq!(dirty["data"]["dirty"], json!(true));
    assert_eq!(dirty["data"]["entries"][0]["path"], json!("tracked.txt"));
    assert_eq!(dirty["data"]["entries"][0]["worktree"], json!("M"));
    assert_eq!(dirty["data"]["counts"]["modified"], json!(1));
    assert_eq!(
        before_dirty,
        tree_snapshot(temp.path()),
        "dirty inspect must not rewrite files"
    );
}

#[test]
fn git_inspect_outside_a_repository_is_a_domain_error() {
    let temp = TempDir::new().expect("temp");
    fs::create_dir_all(temp.path().join(".kittify")).expect(".kittify");
    let output = run_in(
        temp.path(),
        &["--project-root", path(temp.path()), "git", "inspect"],
    );
    assert_error(&output, "git.inspect", 1, "domain");
}

#[test]
fn charter_inspect_reports_the_bundle_without_rewriting_it() {
    let temp = charter_project();
    let before = tree_snapshot(temp.path());
    let output = run_in(
        temp.path(),
        &["--project-root", path(temp.path()), "charter", "inspect"],
    );
    let value = assert_ok(&output, "charter.inspect", 0);
    let inspection = &value["data"]["inspection"];
    assert_eq!(value["data"]["built_in_searched"], json!(false));
    assert_eq!(
        inspection["charter"]["charter_md"]["relative_path"],
        json!(".kittify/charter/charter.md")
    );
    assert!(
        inspection["charter"]["charter_md"]["sha256"]
            .as_str()
            .is_some_and(|digest| digest.len() == 64)
    );
    assert_eq!(inspection["config"]["present"], json!(false));
    assert_eq!(
        before,
        tree_snapshot(temp.path()),
        "charter inspect is read-only"
    );
}

#[test]
fn charter_inspect_requires_an_existing_project_root() {
    let temp = TempDir::new().expect("temp");
    let missing = temp.path().join("does-not-exist");
    let output = run_in(
        temp.path(),
        &["--project-root", path(&missing), "charter", "inspect"],
    );
    assert_error(&output, "charter.inspect", 1, "domain");
}

#[test]
fn glossary_lookup_honours_explicit_scopes_and_reports_ambiguity() {
    let temp = glossary_project();
    let before = tree_snapshot(temp.path());

    let ambiguous = run_in(
        temp.path(),
        &[
            "--project-root",
            path(temp.path()),
            "glossary",
            "lookup",
            "workspace",
        ],
    );
    let ambiguous = assert_ok(&ambiguous, "glossary.lookup", 0);
    assert_eq!(ambiguous["data"]["result"]["status"], json!("ambiguous"));
    assert_eq!(
        ambiguous["data"]["result"]["matches"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    let scoped = run_in(
        temp.path(),
        &[
            "--project-root",
            path(temp.path()),
            "glossary",
            "lookup",
            "Workspace",
            "--scope",
            "team_domain",
        ],
    );
    let scoped = assert_ok(&scoped, "glossary.lookup", 0);
    assert_eq!(scoped["data"]["result"]["status"], json!("resolved"));
    assert_eq!(scoped["data"]["result"]["surface"], json!("workspace"));
    assert_eq!(
        scoped["data"]["result"]["matches"][0]["scope"],
        json!("team_domain")
    );
    assert_eq!(scoped["data"]["seeds"][0]["scope"], json!("team_domain"));
    assert_eq!(
        before,
        tree_snapshot(temp.path()),
        "glossary lookup is read-only"
    );
}

#[test]
fn glossary_lookup_supports_explicit_seed_files() {
    let temp = glossary_project();
    let alternate = temp.path().join("alternate.yaml");
    write(
        &alternate,
        "terms:\n  - surface: artifact\n    definition: A produced deliverable.\n    status: active\n",
    );

    let output = run_in(
        temp.path(),
        &[
            "--project-root",
            path(temp.path()),
            "glossary",
            "lookup",
            "artifact",
            "--scope",
            "spec_kitty_core",
            "--seed-file",
            &format!("spec_kitty_core={}", path(&alternate)),
        ],
    );
    let value = assert_ok(&output, "glossary.lookup", 0);
    assert_eq!(value["data"]["result"]["status"], json!("resolved"));
    assert_eq!(value["data"]["seeds"][0]["path"], json!(path(&alternate)));
}

#[test]
fn glossary_usage_errors_exit_two() {
    let temp = glossary_project();
    let bad_scope = run_in(
        temp.path(),
        &[
            "--project-root",
            path(temp.path()),
            "glossary",
            "lookup",
            "workspace",
            "--scope",
            "not_a_scope",
        ],
    );
    assert_error(&bad_scope, "glossary.lookup", 2, "usage");

    let bad_confidence = run_in(
        temp.path(),
        &[
            "--project-root",
            path(temp.path()),
            "glossary",
            "lookup",
            "workspace",
            "--confidence",
            "2.0",
        ],
    );
    assert_error(&bad_confidence, "glossary.lookup", 2, "usage");
}

#[test]
fn runtime_query_is_a_pure_planner_query() {
    let temp = runtime_project();
    let before = tree_snapshot(temp.path());
    let output = run_in(
        temp.path(),
        &[
            "runtime",
            "query",
            "--template",
            path(&temp.path().join("template.yaml")),
            "--state",
            path(&temp.path().join("state.json")),
        ],
    );
    let value = assert_ok(&output, "runtime.query", 0);
    assert_eq!(value["data"]["mode"], json!("pure_planner_query"));
    assert_eq!(value["data"]["advances_run"], json!(false));
    assert_eq!(value["data"]["decision"]["kind"], json!("step"));
    assert_eq!(value["data"]["decision"]["step_id"], json!("specify"));
    assert_eq!(
        before,
        tree_snapshot(temp.path()),
        "runtime query is read-only"
    );
}

#[test]
fn runtime_query_blocked_decision_is_nonzero_but_still_one_json_document() {
    let temp = runtime_project();
    let output = run_in(
        temp.path(),
        &[
            "runtime",
            "query",
            "--template",
            path(&temp.path().join("template.yaml")),
            "--state",
            path(&temp.path().join("blocked-state.json")),
        ],
    );
    let value = assert_ok(&output, "runtime.query", 3);
    assert_eq!(value["data"]["decision"]["kind"], json!("blocked"));
    assert_eq!(
        value["data"]["decision"]["reason"],
        json!("operator decision pending")
    );
}

#[test]
fn runtime_query_terminal_decision_is_success() {
    let temp = runtime_project();
    let output = run_in(
        temp.path(),
        &[
            "runtime",
            "query",
            "--template",
            path(&temp.path().join("template.yaml")),
            "--state",
            path(&temp.path().join("terminal-state.json")),
        ],
    );
    let value = assert_ok(&output, "runtime.query", 0);
    assert_eq!(value["data"]["decision"]["kind"], json!("terminal"));
}

#[test]
fn runtime_query_refuses_malformed_or_missing_input() {
    let temp = runtime_project();
    write(&temp.path().join("bad-state.json"), "{ not json");
    let malformed = run_in(
        temp.path(),
        &[
            "runtime",
            "query",
            "--template",
            path(&temp.path().join("template.yaml")),
            "--state",
            path(&temp.path().join("bad-state.json")),
        ],
    );
    assert_error(&malformed, "runtime.query", 1, "domain");

    let missing = run_in(
        temp.path(),
        &[
            "runtime",
            "query",
            "--template",
            path(&temp.path().join("missing.yaml")),
            "--state",
            path(&temp.path().join("state.json")),
        ],
    );
    assert_error(&missing, "runtime.query", 1, "domain");
}

#[test]
fn events_inspect_reports_statistics_without_a_current_lane_claim() {
    let temp = runtime_project();
    let events = events_file(&temp);
    let output = run_in(temp.path(), &["events", "inspect", path(&events)]);
    let value = assert_ok(&output, "events.inspect", 0);
    let data = &value["data"];
    assert_eq!(data["statistics_only"], json!(true));
    assert_eq!(data["counts"]["transitions"], json!(1));
    assert_eq!(data["counts"]["annotations"], json!(1));
    assert_eq!(data["counts"]["non_lane"], json!(1));
    assert_eq!(data["counts"]["total"], json!(3));
    assert_eq!(
        data["counts"]["non_lane_by_type"],
        json!({"MissionCreated": 1})
    );
    assert_eq!(data["events"].as_array().unwrap().len(), 3);
    assert_eq!(data["events"][0]["kind"], json!("transition"));
    assert_eq!(data["events"][0]["from_lane"], json!("planned"));
    assert_eq!(data["events"][0]["to_lane"], json!("claimed"));
    assert!(
        data.get("current_lane").is_none(),
        "no lane reconstruction: {data}"
    );
    assert!(
        data.get("current_state").is_none(),
        "no state reconstruction: {data}"
    );
}

#[test]
fn events_inspect_refuses_a_missing_stream() {
    let temp = runtime_project();
    let missing = temp.path().join("missing.jsonl");
    let output = run_in(temp.path(), &["events", "inspect", path(&missing)]);
    assert_error(&output, "events.inspect", 1, "domain");
}

#[test]
fn transition_check_uses_the_fsm_and_persists_nothing() {
    let temp = TempDir::new().expect("temp");
    write(&temp.path().join("marker.txt"), "unchanged\n");
    let before = tree_snapshot(temp.path());

    let admitted = run_in(
        temp.path(),
        &[
            "transition",
            "check",
            "planned",
            "claimed",
            "--actor",
            "agent-1",
        ],
    );
    let admitted = assert_ok(&admitted, "transition.check", 0);
    assert_eq!(admitted["data"]["admitted"], json!(true));
    assert_eq!(admitted["data"]["persisted"], json!(false));
    assert_eq!(admitted["data"]["from"], json!("planned"));
    assert_eq!(admitted["data"]["to"], json!("claimed"));

    let no_edge = run_in(
        temp.path(),
        &[
            "transition",
            "check",
            "planned",
            "for_review",
            "--actor",
            "agent-1",
        ],
    );
    let no_edge = assert_error(&no_edge, "transition.check", 1, "domain");
    assert_eq!(no_edge["error"]["details"]["admitted"], json!(false));

    let forced_without_review_ref = run_in(
        temp.path(),
        &[
            "transition",
            "check",
            "for_review",
            "in_progress",
            "--force",
            "--actor",
            "agent-1",
            "--reason",
            "rewind",
        ],
    );
    assert_error(&forced_without_review_ref, "transition.check", 1, "domain");

    let alias = run_in(
        temp.path(),
        &[
            "transition",
            "check",
            "doing",
            "planned",
            "--reason",
            "replan",
        ],
    );
    let alias = assert_ok(&alias, "transition.check", 0);
    assert_eq!(alias["data"]["from"], json!("in_progress"));

    assert_eq!(
        before,
        tree_snapshot(temp.path()),
        "transition check persists nothing"
    );
}

#[test]
fn transition_check_unknown_lane_is_a_usage_error() {
    let temp = TempDir::new().expect("temp");
    let output = run_in(temp.path(), &["transition", "check", "bogus", "planned"]);
    assert_error(&output, "transition.check", 2, "usage");
}

#[test]
fn unsupported_commands_exit_two_with_one_json_document_on_stdout() {
    for command in [
        "init",
        "upgrade",
        "next",
        "merge",
        "worktrees",
        "activation",
    ] {
        let output = run(&[command, "--json"]);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{command} must be a usage failure"
        );
        let value = assert_error(&output, "cli", 2, "usage");
        assert!(
            value["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains(command)),
            "usage diagnostic should name {command}: {value}"
        );
    }
}

#[test]
fn every_command_accepts_the_global_json_flag() {
    let temp = mission_project();
    let cases: Vec<Vec<&str>> = vec![
        vec!["capabilities", "--json"],
        vec!["project", "root", "--json"],
        vec!["mission", "list", "--json"],
        vec!["charter", "inspect", "--json"],
        vec!["glossary", "lookup", "workspace", "--json"],
    ];
    for args in cases {
        let mut full = vec!["--project-root", path(temp.path())];
        full.extend(args);
        let output = run_in(temp.path(), &full);
        let value = single_json(&output);
        assert_eq!(value["schema_version"], json!(1), "args: {full:?}");
        assert!(value["ok"].is_boolean(), "args: {full:?}");
    }
}
