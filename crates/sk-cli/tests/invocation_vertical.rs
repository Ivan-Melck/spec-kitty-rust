//! End-to-end acceptance tests for the governed invocation vertical.
//!
//! Every test drives the real `spec-kitty` binary against a temporary project
//! with isolated HOME/XDG/TMP roots. The composed `next` tests are the load
//! bearing ones: they prove that one advancing call executes the real pack step
//! contract (one started + one completed Op record per contract step), runs the
//! post-action guard, and advances the durable run exactly once — and that a
//! protected branch or a failing guard advances nothing.

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

/// A temporary project with fully isolated roots.
struct Project {
    temp: TempDir,
    home: PathBuf,
    xdg_cache: PathBuf,
    xdg_config: PathBuf,
    xdg_data: PathBuf,
    xdg_state: PathBuf,
    tmp: PathBuf,
}

impl Project {
    fn new() -> Self {
        let temp = TempDir::new().expect("temp");
        let temp_root = temp.path().to_path_buf();
        let home = temp.path().join("home");
        let xdg_cache = temp.path().join("xdg-cache");
        let xdg_config = temp.path().join("xdg-config");
        let xdg_data = temp.path().join("xdg-data");
        let xdg_state = temp.path().join("xdg-state");
        let tmp = temp.path().join("tmp");
        for directory in [&home, &xdg_cache, &xdg_config, &xdg_data, &xdg_state, &tmp] {
            fs::create_dir_all(directory).expect("isolated root");
        }
        let project = Self {
            temp,
            home,
            xdg_cache,
            xdg_config,
            xdg_data,
            xdg_state,
            tmp,
        };
        project.git(&["init", "-q", "-b", "main"]);
        fs::create_dir_all(project.root().join(".kittify")).expect("kittify dir");
        fs::write(
            project.root().join(".kittify/config.yaml"),
            "agents:\n  available: [codex]\n",
        )
        .expect("config");
        // A resolvable `origin/HEAD` is what makes `main` the primary branch;
        // without it the reference treats the repository as one that keeps no Op
        // records and admits the write on any branch.
        let remote = temp_root.join("remote.git");
        let output = Command::new("git")
            .current_dir(temp_root)
            .args(["init", "-q", "--bare", remote.to_str().expect("utf8")])
            .output()
            .expect("git init bare");
        assert!(output.status.success());
        project.git(&["remote", "add", "origin", remote.to_str().expect("utf8")]);
        fs::write(project.root().join("README.md"), "fixture\n").expect("readme");
        project.git(&["add", "-A"]);
        project.git(&["commit", "-qm", "initial"]);
        project.git(&["push", "-qu", "origin", "main"]);
        project.git(&["remote", "set-head", "origin", "main"]);
        project
    }

    fn root(&self) -> &Path {
        self.temp.path()
    }

    /// Make the primary branch *track* Op records (the condition that turns a
    /// protected branch into a refusal rather than the #6410 skip).
    fn track_op_records(&self) {
        self.write(
            "kitty-ops/01M6GC7EV7RY46WMHMXYWS6XZM.jsonl",
            "{\"event\":\"started\"}\n",
        );
        self.git(&["add", "-A"]);
        self.git(&["commit", "-qm", "track op records"]);
        self.git(&["push", "-q", "origin", "main"]);
    }

    fn git(&self, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(self.root())
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

    fn run(&self, args: &[&str]) -> Output {
        Command::new(binary())
            .current_dir(self.root())
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", &self.xdg_cache)
            .env("XDG_CONFIG_HOME", &self.xdg_config)
            .env("XDG_DATA_HOME", &self.xdg_data)
            .env("XDG_STATE_HOME", &self.xdg_state)
            .env("TMPDIR", &self.tmp)
            .env("SPEC_KITTY_PACKS_ROOT", packs_root())
            .env_remove("SPECIFY_REPO_ROOT")
            .env_remove("GIT_CEILING_DIRECTORIES")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(args)
            .output()
            .expect("run spec-kitty")
    }

    fn json(&self, args: &[&str]) -> (Value, i32) {
        let output = self.run(args);
        let text = String::from_utf8(output.stdout.clone()).expect("stdout is UTF-8");
        let code = output.status.code().unwrap_or(-1);
        let value = if text.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).unwrap_or_else(|error| {
                panic!("stdout was not one JSON document ({error}):\n{text}")
            })
        };
        (value, code)
    }

    fn write(&self, relative: &str, contents: &str) {
        let path = self.root().join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("fixture parent");
        }
        fs::write(path, contents).expect("fixture file");
    }

    fn op_records(&self) -> Vec<PathBuf> {
        let directory = self.root().join("kitty-ops");
        let Ok(entries) = fs::read_dir(directory) else {
            return Vec::new();
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(".jsonl") && name != "ops-index.jsonl")
            })
            .collect();
        paths.sort();
        paths
    }

    fn op_lines(&self) -> Vec<Value> {
        self.op_records()
            .iter()
            .flat_map(|path| {
                fs::read_to_string(path)
                    .expect("record")
                    .lines()
                    .map(|line| serde_json::from_str::<Value>(line).expect("record JSON"))
                    .collect::<Vec<Value>>()
            })
            .collect()
    }

    fn run_dir(&self) -> PathBuf {
        let runs = self.root().join(".kittify/runtime/runs");
        let mut entries: Vec<PathBuf> = fs::read_dir(&runs)
            .expect("run store")
            .map(|entry| entry.expect("entry").path())
            .filter(|path| path.is_dir())
            .collect();
        entries.sort();
        assert_eq!(entries.len(), 1, "exactly one durable run");
        entries.pop().expect("run dir")
    }

    fn state(&self) -> Value {
        let path = self.run_dir().join("state.json");
        serde_json::from_str(&fs::read_to_string(path).expect("cursor")).expect("cursor JSON")
    }

    fn journal(&self) -> Vec<Value> {
        let path = self.run_dir().join("run.events.jsonl");
        fs::read_to_string(path)
            .expect("journal")
            .lines()
            .map(|line| serde_json::from_str(line).expect("journal line"))
            .collect()
    }
}

const MISSION_TEMPLATE: &str = "mission:\n  key: software-dev\n  name: Software Dev\n  version: '1.0'\nsteps:\n  - id: specify\n    title: Specify\n    prompt: Write the spec.\n  - id: plan\n    title: Plan\n    depends_on: [specify]\n    prompt: Plan.\n  - id: tasks\n    title: Tasks\n    depends_on: [plan]\n    prompt: Tasks.\n  - id: implement\n    title: Implement\n    depends_on: [tasks]\n    prompt: Implement.\n  - id: review\n    title: Review\n    depends_on: [implement]\n    prompt: Review.\n";

fn mission(project: &Project, with_spec: bool) -> String {
    let slug = "composed-01M6GC7E".to_string();
    project.write(
        ".kittify/missions/software-dev/mission-runtime.yaml",
        MISSION_TEMPLATE,
    );
    project.write(
        &format!("kitty-specs/{slug}/meta.json"),
        &json!({
            "mission_id": "01M6GC7EV7RY46WMHMXYWS6XZM",
            "mid8": "01M6GC7E",
            "mission_slug": slug,
            "slug": slug,
            "mission_number": null,
            "mission_type": "software-dev",
            "friendly_name": "composed mission",
            "target_branch": "main",
            "topology": "lanes",
            "created_at": "2026-02-08T12:00:00Z"
        })
        .to_string(),
    );
    if with_spec {
        project.write(&format!("kitty-specs/{slug}/spec.md"), "# Spec\n");
    }
    project.write(&format!("kitty-specs/{slug}/tasks/.gitkeep"), "");
    project.git(&["add", "-A"]);
    project.git(&["commit", "-qm", "fixture"]);
    slug
}

#[test]
fn dispatch_dry_run_is_read_only_and_source_shaped() {
    let project = Project::new();
    project.git(&["checkout", "-qb", "kitty/topic"]);
    let (value, code) = project.json(&[
        "dispatch",
        "implement the runtime vertical",
        "--profile",
        "implementer-ivan",
        "--dry-run",
        "--json",
    ]);
    assert_eq!(code, 0);
    assert_eq!(value["status"], json!("dry_run"));
    assert_eq!(value["profile_id"], json!("implementer-ivan"));
    assert_eq!(value["router_confidence"], json!("exact"));
    assert!(value.get("invocation_id").is_none());
    assert!(
        project.op_records().is_empty(),
        "a dry run writes no Op record"
    );
}

#[test]
fn dispatch_opens_an_op_and_complete_closes_it() {
    let project = Project::new();
    project.git(&["checkout", "-qb", "kitty/topic"]);
    let (value, code) = project.json(&[
        "dispatch",
        "implement the runtime vertical",
        "--profile",
        "implementer-ivan",
        "--json",
    ]);
    assert_eq!(code, 0);
    let invocation_id = value["invocation_id"]
        .as_str()
        .expect("invocation id")
        .to_string();
    assert_eq!(value["status"], json!("open"));
    assert_eq!(value["action"], json!("generate"));
    let records = project.op_records();
    assert_eq!(records.len(), 1, "one Op record");
    let started = project.op_lines();
    assert_eq!(started.len(), 1);
    assert_eq!(started[0]["event"], json!("started"));
    assert_eq!(started[0]["actor"], json!("operator"));
    assert_eq!(started[0]["action"], json!("generate"));

    let (closed, code) = project.json(&[
        "profile-invocation",
        "complete",
        "-i",
        &invocation_id,
        "--outcome",
        "done",
        "--artifact",
        "kitty-specs/composed-01M6GC7E/spec.md",
        "--json",
    ]);
    assert_eq!(code, 0);
    assert_eq!(closed["result"], json!("success"));
    assert_eq!(closed["invocation_id"], json!(invocation_id));
    assert_eq!(closed["outcome"], json!("done"));
    assert_eq!(
        closed["artifact_links"],
        json!(["kitty-specs/composed-01M6GC7E/spec.md"])
    );
    let lines = project.op_lines();
    assert_eq!(lines.len(), 3, "started, completed, artifact_link");
    assert_eq!(lines[1]["event"], json!("completed"));
    assert_eq!(lines[1]["closed_by"], json!("agent"));
    assert_eq!(lines[2]["event"], json!("artifact_link"));

    // A second close is the reference's dedicated exit-1 envelope.
    let output = project.run(&[
        "profile-invocation",
        "complete",
        "-i",
        &invocation_id,
        "--outcome",
        "done",
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("already closed"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn dispatch_on_protected_main_refuses_without_writing() {
    let project = Project::new();
    project.track_op_records();
    let output = project.run(&[
        "dispatch",
        "implement something",
        "--profile",
        "implementer-ivan",
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        project.op_records().len(),
        1,
        "only the pre-existing tracked Op record exists; the refusal wrote nothing"
    );
}

#[test]
fn profiles_list_returns_the_source_descriptor_array() {
    let project = Project::new();
    let (value, code) = project.json(&["profiles", "list", "--json"]);
    assert_eq!(code, 0);
    let profiles = value.as_array().expect("a JSON array");
    assert!(!profiles.is_empty(), "the built-in catalogue is present");
    let ivan = profiles
        .iter()
        .find(|profile| profile["profile_id"] == json!("implementer-ivan"))
        .expect("implementer-ivan is listed");
    assert_eq!(ivan["identifier"], json!("implementer-ivan"));
    assert_eq!(ivan["role"], json!("implementer"));
    assert_eq!(ivan["source"], json!("built-in"));
    assert!(
        ivan["action_domains"]
            .as_array()
            .expect("domains")
            .iter()
            .any(|domain| domain == "implement")
    );
    assert!(ivan.get("state").is_none(), "the default view has no state");

    let (all, code) = project.json(&["profiles", "list", "--all", "--json"]);
    assert_eq!(code, 0);
    let all = all.as_array().expect("array");
    assert!(all.iter().any(|profile| profile.get("state").is_some()));
}

#[test]
fn next_composition_executes_the_contract_once_and_advances_once() {
    let project = Project::new();
    project.git(&["checkout", "-qb", "kitty/topic"]);
    let _ = mission(&project, true);

    // The first advancing call starts the run and issues `specify`.
    let (first, code) =
        project.json(&["next", "--agent", "claude", "--result", "success", "--json"]);
    assert_eq!(code, 0, "{first}");
    assert_eq!(project.state()["issued_step_id"], json!("specify"));
    assert_eq!(project.op_records().len(), 0, "no composition yet");

    // The second advancing call reports `specify` done: compose, guard, advance.
    let (second, code) =
        project.json(&["next", "--agent", "claude", "--result", "success", "--json"]);
    assert_eq!(code, 0, "{second}");
    assert_eq!(second["mission_state"], json!("plan"));
    assert_eq!(project.state()["issued_step_id"], json!("plan"));

    // Exactly one Op record per `specify` contract step, each invoked once and
    // closed once.
    let records = project.op_records();
    assert_eq!(
        records.len(),
        7,
        "software-dev/specify has seven contract steps"
    );
    let lines = project.op_lines();
    assert_eq!(lines.len(), 14, "one started + one completed per step");
    let started: Vec<&Value> = lines
        .iter()
        .filter(|line| line["event"] == json!("started"))
        .collect();
    let completed: Vec<&Value> = lines
        .iter()
        .filter(|line| line["event"] == json!("completed"))
        .collect();
    assert_eq!(started.len(), 7);
    assert_eq!(completed.len(), 7);
    for line in &started {
        assert_eq!(line["profile_id"], json!("researcher-robbie"));
        assert_eq!(line["action"], json!("specify"));
        assert_eq!(line["mode_of_work"], json!("mission_step"));
    }
    for line in &completed {
        assert_eq!(line["outcome"], json!("done"));
        assert_eq!(line["closed_by"], json!("agent"));
    }
    let started_ids: Vec<&str> = started
        .iter()
        .filter_map(|line| line["invocation_id"].as_str())
        .collect();
    let completed_ids: Vec<&str> = completed
        .iter()
        .filter_map(|line| line["invocation_id"].as_str())
        .collect();
    assert_eq!(started_ids, completed_ids, "each Op closed exactly once");

    // Exactly one issue event per advancing call: the start's `specify` and the
    // composition advance's `plan`.
    let issued = project
        .journal()
        .iter()
        .filter(|event| event["event_type"] == json!("NextStepIssued"))
        .count();
    assert_eq!(issued, 2, "start + one composition advance");
}

#[test]
fn next_missing_artifact_reissues_before_composition() {
    // The reference's phase order is dependency/step gate -> composition. A
    // missing `spec.md` is caught by the step guard first, so no contract runs:
    // the decision re-issues `specify` (a step, exit 0) and the cursor holds.
    // The composed guard is therefore defense-in-depth for the same checks,
    // never a second, divergent verdict.
    let project = Project::new();
    project.git(&["checkout", "-qb", "kitty/topic"]);
    let _ = mission(&project, false);
    let (first, code) =
        project.json(&["next", "--agent", "claude", "--result", "success", "--json"]);
    assert_eq!(code, 0, "{first}");
    let (second, code) =
        project.json(&["next", "--agent", "claude", "--result", "success", "--json"]);
    assert_eq!(code, 0, "{second}");
    assert_eq!(second["kind"], json!("step"));
    assert_eq!(second["step_id"], json!("specify"));
    assert_eq!(
        second["guard_failures"],
        json!(["Required artifact missing: spec.md"])
    );
    assert!(
        project.op_records().is_empty(),
        "the pre-composition gate never opens an Op"
    );
    assert_eq!(project.state()["issued_step_id"], json!("specify"));
}

#[test]
fn next_composition_on_protected_main_does_not_advance() {
    let project = Project::new();
    project.track_op_records();
    let _ = mission(&project, true);
    let (first, code) =
        project.json(&["next", "--agent", "claude", "--result", "success", "--json"]);
    assert_eq!(code, 0, "{first}");
    let (second, code) =
        project.json(&["next", "--agent", "claude", "--result", "success", "--json"]);
    assert_eq!(code, 1, "{second}");
    assert_eq!(second["kind"], json!("blocked"));
    let reason = second["reason"].as_str().unwrap_or("");
    assert!(
        reason.contains("OpWriteRefused") || reason.contains("protected"),
        "reason: {reason}"
    );
    assert_eq!(
        project.op_records().len(),
        1,
        "admission refuses before any new Op record is written"
    );
    assert_eq!(project.state()["issued_step_id"], json!("specify"));
}
