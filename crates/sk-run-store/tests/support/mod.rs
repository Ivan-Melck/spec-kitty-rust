//! Shared temp-project fixtures for the `sk-run-store` acceptance tests.
//!
//! Every fixture lives under a `tempfile::TempDir`; no test touches the real
//! repository runtime or the user's home directory.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use sk_run_store::{ActorIdentity, ActorType, RunStore, StartRequest};
use tempfile::TempDir;

pub const MISSION_SLUG: &str = "demo";
pub const OWNER_ID: &str = "owner-1";

pub struct Fixture {
    #[allow(dead_code)]
    pub temp: TempDir,
    pub root: PathBuf,
    pub store: RunStore,
    pub template_path: PathBuf,
    pub inputs: BTreeMap<String, Value>,
}

impl Fixture {
    pub fn with_template_yaml(yaml: &str) -> Self {
        Self::with_template_yaml_and_inputs(yaml, BTreeMap::new())
    }

    pub fn with_template_yaml_and_inputs(yaml: &str, inputs: BTreeMap<String, Value>) -> Self {
        let temp = tempfile::tempdir().expect("temp dir");
        let root = temp.path().join("project");
        fs::create_dir_all(&root).expect("project root");
        let template_path = root.join("mission.yaml");
        fs::write(&template_path, yaml.as_bytes()).expect("template");
        let store = RunStore::open(&root).expect("open store");
        Self {
            temp,
            root,
            store,
            template_path,
            inputs,
        }
    }

    pub fn start(&self) -> sk_run_store::StartOutcome {
        self.start_with_inputs(self.inputs.clone())
    }

    pub fn start_with_inputs(&self, inputs: BTreeMap<String, Value>) -> sk_run_store::StartOutcome {
        self.store
            .start(&StartRequest {
                mission_slug: MISSION_SLUG.to_string(),
                mission_id: None,
                template_path: self.template_path.clone(),
                inputs,
                policy_snapshot: Default::default(),
            })
            .expect("start run")
    }

    pub fn run(&self) -> sk_run_store::RunRef {
        self.store
            .run_for_mission(MISSION_SLUG, None)
            .expect("index read")
            .expect("run recorded")
    }
}

pub fn two_step_yaml() -> &'static str {
    r#"mission:
  key: demo
  name: Demo mission
  version: "1.0.0"
steps:
  - id: specify
    title: Specify
    prompt: Do the specification.
  - id: plan
    title: Plan
    prompt: Do the plan.
    depends_on:
      - specify
"#
}

pub fn input_step_yaml() -> &'static str {
    r#"mission:
  key: demo
  name: Demo mission
  version: "1.0.0"
steps:
  - id: specify
    title: Specify
    prompt: Do the specification.
    requires_inputs:
      - topic
  - id: plan
    title: Plan
    prompt: Do the plan.
    depends_on:
      - specify
"#
}

pub fn audit_yaml() -> &'static str {
    r#"mission:
  key: demo
  name: Demo mission
  version: "1.0.0"
steps:
  - id: specify
    title: Specify
    prompt: Do the specification.
  - id: plan
    title: Plan
    prompt: Do the plan.
    depends_on:
      - specify
audit_steps:
  - id: review
    title: Review
    audit:
      trigger_mode: manual
      enforcement: blocking
    depends_on:
      - plan
"#
}

pub fn agent(id: &str) -> ActorIdentity {
    ActorIdentity {
        actor_id: id.to_string(),
        actor_type: ActorType::Llm,
        display_name: String::new(),
        provider: None,
        model: None,
        tool: None,
    }
}

pub fn human(id: &str) -> ActorIdentity {
    ActorIdentity {
        actor_id: id.to_string(),
        actor_type: ActorType::Human,
        display_name: String::new(),
        provider: None,
        model: None,
        tool: None,
    }
}

/// Snapshot every non-lock file under `dir`, keyed by relative path.
///
/// The dedicated `*.lock` sidecars exist only to be locked and are truncated on
/// release; they are excluded so a refused write can be compared semantically.
pub fn run_files(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    collect(dir, dir, &mut files);
    files
}

fn collect(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
    for entry in fs::read_dir(dir).expect("read dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, out);
        } else {
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".lock"))
            {
                continue;
            }
            let relative = path.strip_prefix(root).expect("relative").to_path_buf();
            out.insert(relative, fs::read(&path).expect("read file"));
        }
    }
}

pub fn json_file(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).expect("read json")).expect("valid json")
}

pub fn event_lines(run_dir: &Path) -> Vec<Value> {
    let path = run_dir.join("run.events.jsonl");
    match fs::read(&path) {
        Ok(bytes) => String::from_utf8(bytes)
            .expect("utf8 events")
            .lines()
            .map(|line| serde_json::from_str(line).expect("event json"))
            .collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => panic!("read events: {error}"),
    }
}

pub fn event_types(run_dir: &Path) -> Vec<String> {
    event_lines(run_dir)
        .into_iter()
        .map(|event| {
            event
                .get("event_type")
                .and_then(Value::as_str)
                .expect("event_type")
                .to_string()
        })
        .collect()
}

pub fn state_bytes(run_dir: &Path) -> Vec<u8> {
    fs::read(run_dir.join("state.json")).expect("state.json")
}

pub fn write_state(run_dir: &Path, value: &Value) {
    fs::write(
        run_dir.join("state.json"),
        serde_json::to_vec_pretty(value).expect("state bytes"),
    )
    .expect("write state");
}
