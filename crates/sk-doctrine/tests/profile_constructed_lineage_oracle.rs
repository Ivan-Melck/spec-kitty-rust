//! Source-first conformance of **constructed YAML kinds through the model
//! composition paths** — layer overlay, lineage union, `exclude_unset` and
//! `excluding`.
//!
//! `profile_yaml_kinds_oracle` proves the constructed values on a single layer.
//! The reference's composition paths then run over the *model* (its
//! `model_dump` / `exclude_unset` views), so a constructed `!!binary` in an
//! overlay must replace the built-in field as its decoded string, a `!!set`
//! must union/order like the reference's Python list, and an `excluding`
//! authored as a `!!set` must remove the whole field before re-validation.
//!
//! The reference runs first in an asserted isolated root; then the native
//! repository is loaded over the same bytes and the merged/resolved dumps,
//! provenance and skipped sets are compared field for field.

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use sk_doctrine::{DrgEdge, DrgGraph, DrgNode, NodeKind, ProfileRepository, Relation};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    built_in: PathBuf,
    project: PathBuf,
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("create directory");
    fs::write(path, text).expect("write fixture");
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("temporary root");
        let root = temp.path().canonicalize().expect("canonical root");
        let built_in = root.join("empty-built-in");
        let project = root.join("profiles");
        fs::create_dir_all(built_in.join("agent_profiles")).expect("built-in");
        fs::create_dir_all(project.join("agent_profiles")).expect("project");
        for directory in ["home", "cache", "config", "data", "state", "tmp"] {
            fs::create_dir_all(root.join(directory)).expect("isolated directory");
        }
        write(
            &built_in.join("agent_profiles/overlay-target.agent.yaml"),
            concat!(
                "profile-id: overlay-target\n",
                "name: !!binary QmFzZQ==\n",
                "purpose: p\n",
                "roles: !!set {Reviewer: null}\n",
                "capabilities: !!set {alpha: null, beta: null}\n",
                "specialization: {primary-focus: x}\n",
            ),
        );
        write(
            &built_in.join("agent_profiles/leaf.agent.yaml"),
            concat!(
                "profile-id: leaf\n",
                "name: Leaf\n",
                "purpose: p\n",
                "roles: [Implementer]\n",
                "specialization: {primary-focus: y}\n",
                "capabilities: !!set {delta: null}\n",
                "excluding: !!set {capabilities: null}\n",
            ),
        );
        write(
            &project.join("agent_profiles/overlay-target.agent.yaml"),
            concat!(
                "profile-id: overlay-target\n",
                "description: !!binary SGVsbG8=\n",
                "capabilities: !!set {gamma: null}\n",
            ),
        );
        Self {
            _temp: temp,
            root,
            built_in,
            project,
        }
    }

    fn env(&self) -> Vec<(String, String)> {
        let root = &self.root;
        vec![
            ("PATH".to_owned(), std::env::var("PATH").unwrap_or_default()),
            ("HOME".to_owned(), root.join("home").display().to_string()),
            (
                "XDG_CACHE_HOME".to_owned(),
                root.join("cache").display().to_string(),
            ),
            (
                "XDG_CONFIG_HOME".to_owned(),
                root.join("config").display().to_string(),
            ),
            (
                "XDG_DATA_HOME".to_owned(),
                root.join("data").display().to_string(),
            ),
            (
                "XDG_STATE_HOME".to_owned(),
                root.join("state").display().to_string(),
            ),
            ("TMPDIR".to_owned(), root.join("tmp").display().to_string()),
            ("PYTHONDONTWRITEBYTECODE".to_owned(), "1".to_owned()),
            ("PYTHONHASHSEED".to_owned(), "0".to_owned()),
            (
                "PYTHONPATH".to_owned(),
                support::repository_root().join("src").display().to_string(),
            ),
            (
                "SPEC_KITTY_PACKS_ROOT".to_owned(),
                support::repository_root()
                    .join("packs")
                    .display()
                    .to_string(),
            ),
            ("SK_G2_ROOT".to_owned(), root.display().to_string()),
            (
                "SK_G2_BUILT_IN".to_owned(),
                self.built_in.join("agent_profiles").display().to_string(),
            ),
            (
                "SK_G2_PROJECT".to_owned(),
                self.project.join("agent_profiles").display().to_string(),
            ),
        ]
    }
}

const ENV_ASSERTIONS: &str = r#"
import json, os, sys
from pathlib import Path
assert sys.version.split()[0] == "3.11.15", sys.version
root = Path(os.environ["SK_G2_ROOT"]).resolve()
assert Path.cwd().resolve() == root, (Path.cwd(), root)
for key in ("HOME", "TMPDIR", "XDG_CACHE_HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"):
    path = Path(os.environ[key]).resolve()
    assert str(path).startswith(str(root) + os.sep), (key, path, root)
"#;

const SOURCE_SCRIPT: &str = r#"
env_assertions
from charter.offering.agent_profiles.repository import AgentProfileRepository
from charter.offering.drg.models import DRGGraph, DRGNode, DRGEdge, NodeKind, Relation
built_in = Path(os.environ["SK_G2_BUILT_IN"])
project = Path(os.environ["SK_G2_PROJECT"])
graph = DRGGraph(
    schema_version="1.0",
    generated_at="x",
    generated_by="g",
    nodes=[
        DRGNode(urn="agent_profile:overlay-target", kind=NodeKind.AGENT_PROFILE),
        DRGNode(urn="agent_profile:leaf", kind=NodeKind.AGENT_PROFILE),
    ],
    edges=[
        DRGEdge(
            source="agent_profile:leaf",
            target="agent_profile:overlay-target",
            relation=Relation.SPECIALIZES_FROM,
        )
    ],
)
repo = AgentProfileRepository(built_in_dir=built_in, project_dir=project, drg=graph)
merged = repo.get("overlay-target")
resolved = repo.resolve_profile("leaf")
assert merged.name == "Base"
assert merged.description == "Hello"
assert merged.capabilities == ["gamma"], merged.capabilities
print(json.dumps({
    "ids": [profile.profile_id for profile in repo.list_all()],
    "skipped": [entry.profile_id for entry in repo.skipped_profiles()],
    "merged": merged.model_dump(by_alias=True),
    "resolved": resolved.model_dump(by_alias=True),
    "provenance": repo.get_provenance("overlay-target"),
}))
"#;

fn run_source(fixture: &Fixture) -> Value {
    let script = format!(
        "{ENV_ASSERTIONS}\n{}",
        SOURCE_SCRIPT.replace("env_assertions\n", "")
    );
    let output = Command::new(support::python())
        .arg("-c")
        .arg(script)
        .env_clear()
        .envs(fixture.env())
        .current_dir(&fixture.root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run source oracle");
    assert!(
        output.status.success(),
        "Source oracle failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("source oracle JSON")
}

fn json_dump(value: &serde_yaml_ng::Value) -> Value {
    match value {
        serde_yaml_ng::Value::Null => Value::Null,
        serde_yaml_ng::Value::Bool(flag) => Value::Bool(*flag),
        serde_yaml_ng::Value::Number(number) => {
            json!(number.as_i64().expect("integer profile dump value"))
        }
        serde_yaml_ng::Value::String(text) => Value::String(text.clone()),
        serde_yaml_ng::Value::Sequence(items) => {
            Value::Array(items.iter().map(json_dump).collect())
        }
        serde_yaml_ng::Value::Mapping(mapping) => {
            let mut object = serde_json::Map::new();
            for (key, value) in mapping {
                object.insert(
                    key.as_str()
                        .expect("profile dump keys are strings")
                        .to_owned(),
                    json_dump(value),
                );
            }
            Value::Object(object)
        }
        serde_yaml_ng::Value::Tagged(tagged) => json_dump(&tagged.value),
    }
}

fn graph() -> DrgGraph {
    DrgGraph::new(
        "1.0",
        "x",
        "g",
        ["overlay-target", "leaf"]
            .into_iter()
            .map(|name| {
                DrgNode::new(
                    format!("agent_profile:{name}"),
                    NodeKind::AgentProfile,
                    None,
                    Vec::new(),
                )
                .expect("node")
            })
            .collect(),
        vec![
            DrgEdge::new(
                "agent_profile:leaf",
                "agent_profile:overlay-target",
                Relation::SpecializesFrom,
            )
            .expect("edge"),
        ],
    )
    .expect("graph")
}

#[test]
fn constructed_values_survive_overlay_lineage_and_excluding() {
    let fixture = Fixture::new();
    let source = run_source(&fixture);

    let repository = ProfileRepository::load(&fixture.built_in, &[], Some(&fixture.project))
        .expect("native load");
    let source_ids: Vec<String> = source["ids"]
        .as_array()
        .expect("ids")
        .iter()
        .map(|id| id.as_str().expect("scalar id").to_owned())
        .collect();
    assert_eq!(repository.scalar_ids(), source_ids);
    assert_eq!(
        source["skipped"].as_array().expect("skipped").len(),
        repository.skipped().len()
    );

    // The overlay merge keeps the built-in defaults, decodes the overlay's
    // `!!binary` description, and replaces the list with the constructed set.
    let merged = repository
        .get_str("overlay-target")
        .expect("merged profile");
    assert_eq!(
        json_dump(&serde_yaml_ng::Value::Mapping(
            merged.model_dump().shadow().clone()
        )),
        source["merged"],
        "overlay-merged full dump"
    );
    assert_eq!(
        repository.provenance(&sk_kernel::PythonString::from_scalars("overlay-target")),
        source["provenance"].as_str(),
        "provenance of the overlaid profile"
    );

    // Lineage unions the child's constructed set onto the parent's, then the
    // child's `excluding: !!set {capabilities: null}` removes the whole field
    // and the result is re-validated.
    let resolved = repository
        .resolve_str("leaf", &graph())
        .expect("lineage resolution");
    assert_eq!(
        json_dump(&serde_yaml_ng::Value::Mapping(
            resolved.model_dump().shadow().clone()
        )),
        source["resolved"],
        "lineage-resolved full dump"
    );
}
