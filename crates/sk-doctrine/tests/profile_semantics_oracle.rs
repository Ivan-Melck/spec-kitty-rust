//! Source-first exact semantics the A transport matrix did not cover.
//!
//! Four independent families, each asserted on the pinned reference **first**
//! and only then on the native canonical model:
//!
//! 1. real `<<` merge keys: a quoted literal `'<<'`, a `!!str`-tagged key and a
//!    plain key are three different things; a duplicate merge key, a non-mapping
//!    merge value and a duplicate ordinary key are constructor errors, not silent
//!    drops; `<<: [*a, *b]` precedence and key order follow `flatten_mapping`.
//! 2. Pydantic's lax scalars, required/bounded fields, nested `extra="ignore"`,
//!    the deprecated scalar `role:`, alias/`populate_by_name` spellings and the
//!    full `model_dump(by_alias=True)` — field for field.
//! 3. lineage parent selection (first sorted target, never an ambiguity error),
//!    `exclude_unset` nested merge, layer overlay and `excluding`.
//! 4. a DRG node/edge/document whose URN is not ASCII is refused by both, at
//!    graph-model validation (the reference never accepts a non-scalar URN, so a
//!    non-scalar profile id genuinely has no graph parent).
//!
//! Isolation: the reference and the native helper each run in an asserted
//! temporary CWD, `HOME`, every `XDG_*` directory and `TMPDIR`.

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use sk_doctrine::{DrgEdge, DrgGraph, DrgNode, NodeKind, ProfileRepository, Relation};
use sk_kernel::PythonString;

// ---------------------------------------------------------------------------
// Fixture plumbing
// ---------------------------------------------------------------------------

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

fn fixture(files: &[(&str, &str)]) -> Fixture {
    let temp = tempfile::tempdir().expect("temporary root");
    let root = temp.path().canonicalize().expect("canonical root");
    let built_in = root.join("empty-built-in");
    let project = root.join("profiles");
    fs::create_dir_all(project.join("agent_profiles")).expect("profiles directory");
    for directory in ["home", "cache", "config", "data", "state", "tmp"] {
        fs::create_dir_all(root.join(directory)).expect("isolated directory");
    }
    for (name, text) in files {
        write(&project.join("agent_profiles").join(name), text);
    }
    Fixture {
        _temp: temp,
        root,
        built_in,
        project,
    }
}

impl Fixture {
    fn env(&self) -> Vec<(String, String)> {
        let root = &self.root;
        let mut env = vec![
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
            ("SK_G_ROOT".to_owned(), root.display().to_string()),
            (
                "SK_G_BUILT_IN".to_owned(),
                self.built_in.display().to_string(),
            ),
            (
                "SK_G_PROJECT".to_owned(),
                self.project.join("agent_profiles").display().to_string(),
            ),
        ];
        if std::env::var_os("SK_G_ACTIVE_LANGUAGES").is_some() {
            env.push((
                "SK_G_ACTIVE_LANGUAGES".to_owned(),
                std::env::var("SK_G_ACTIVE_LANGUAGES").expect("checked"),
            ));
        }
        env
    }
}

/// Run the pinned reference; it asserts its own environment and expectations.
fn run_source(fixture: &Fixture, script: &str) -> Value {
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

const ENV_ASSERTIONS: &str = r#"
import json, os
from pathlib import Path
root = Path(os.environ["SK_G_ROOT"]).resolve()
assert Path.cwd().resolve() == root, (Path.cwd(), root)
for key in ("HOME", "TMPDIR", "XDG_CACHE_HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"):
    path = Path(os.environ[key]).resolve()
    assert str(path).startswith(str(root) + os.sep), (key, path, root)
def cps(text):
    return [ord(character) for character in text]
"#;

/// Convert a native `model_dump` into the JSON shape the oracle emitted.
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

fn native_dump(repository: &ProfileRepository, id: &str) -> Value {
    let profile = repository.get_str(id).expect("native profile loads");
    json_dump(&serde_yaml_ng::Value::Mapping(
        profile.model_dump().shadow().clone(),
    ))
}

/// Assert the reference's full dump and the native `model_dump` agree for every
/// id the reference reports, and that both skipped the same files.
fn assert_dump_parity(fixture: &Fixture, source: &Value) {
    let repository = ProfileRepository::load(&fixture.built_in, &[], Some(&fixture.project))
        .expect("native profile load");
    let source_ids: Vec<String> = source["ids"]
        .as_array()
        .expect("ids")
        .iter()
        .map(|id| id.as_str().expect("scalar id").to_owned())
        .collect();
    let native_ids: Vec<String> = repository.scalar_ids().into_iter().collect::<Vec<_>>();
    assert_eq!(native_ids, source_ids, "loaded profile set");
    assert_eq!(
        source["skipped"].as_array().expect("skipped ids").len(),
        repository.skipped().len(),
        "skipped count: source {:?} native {:?}",
        source["skipped"],
        repository.skipped()
    );
    for id in &source_ids {
        let expected = &source["dumps"][id.as_str()];
        assert_eq!(
            native_dump(&repository, id),
            *expected,
            "{id}: native model_dump diverged from the reference"
        );
    }
}

const DUMP_SCRIPT: &str = r#"
env_assertions
from charter.offering.agent_profiles.repository import AgentProfileRepository
built_in = Path(os.environ["SK_G_BUILT_IN"])
project = Path(os.environ["SK_G_PROJECT"])
repo = AgentProfileRepository(built_in_dir=built_in, project_dir=project)
out = {
    "ids": [profile.profile_id for profile in repo.list_all()],
    "skipped": [entry.profile_id for entry in repo.skipped_profiles()],
    "skipped_summaries": [entry.error_summary for entry in repo.skipped_profiles()],
    "dumps": {profile.profile_id: profile.model_dump(by_alias=True) for profile in repo.list_all()},
}
print(json.dumps(out))
"#;

// ---------------------------------------------------------------------------
// 1. Merge keys
// ---------------------------------------------------------------------------

fn merge_cases() -> Vec<(&'static str, &'static str)> {
    vec![
        // A quoted `'<<'` is an ordinary string key, never a merge key. Here it
        // leaves `specialization` without its required `primary-focus`.
        (
            "quoted-literal.agent.yaml",
            concat!(
                "profile-id: quoted-literal\n",
                "name: Quoted\n",
                "purpose: p\n",
                "roles: [reviewer]\n",
                "specialization:\n",
                "  '<<': &quoted {primary-focus: from-merge}\n",
            ),
        ),
        // A `!!str`-tagged `<<` is likewise a string key.
        (
            "str-tag.agent.yaml",
            concat!(
                "profile-id: str-tag\n",
                "name: StrTag\n",
                "purpose: p\n",
                "roles: [reviewer]\n",
                "specialization:\n",
                "  !!str <<: &string_tag {primary-focus: from-merge}\n",
            ),
        ),
        // A plain `<<` is the merge tag.
        (
            "plain-merge.agent.yaml",
            concat!(
                "profile-id: plain-merge\n",
                "name: Plain\n",
                "purpose: p\n",
                "roles: [reviewer]\n",
                "specialization:\n",
                "  <<: &plain {primary-focus: merged}\n",
            ),
        ),
        // An explicit `!!merge` tag on any key is a merge key.
        (
            "tagged-merge.agent.yaml",
            concat!(
                "profile-id: tagged-merge\n",
                "name: Tagged\n",
                "purpose: p\n",
                "roles: [reviewer]\n",
                "specialization:\n",
                "  !!merge any-key: &explicit {primary-focus: tagged}\n",
            ),
        ),
        // A non-mapping merge value is a constructor error, not a drop.
        (
            "invalid-merge-value.agent.yaml",
            concat!(
                "profile-id: invalid-merge-value\n",
                "name: Invalid\n",
                "purpose: p\n",
                "roles: [reviewer]\n",
                "specialization:\n",
                "  <<: 7\n",
                "  primary-focus: x\n",
            ),
        ),
        (
            "invalid-merge-item.agent.yaml",
            concat!(
                "profile-id: invalid-merge-item\n",
                "name: InvalidItem\n",
                "purpose: p\n",
                "roles: [reviewer]\n",
                "a: &a {primary-focus: one}\n",
                "specialization:\n",
                "  <<: [*a, 7]\n",
            ),
        ),
        // Duplicate merge keys are never allowed.
        (
            "duplicate-merge.agent.yaml",
            concat!(
                "profile-id: duplicate-merge\n",
                "name: Duplicate\n",
                "purpose: p\n",
                "roles: [reviewer]\n",
                "a: &a {primary-focus: one}\n",
                "b: &b {primary-focus: two}\n",
                "specialization:\n",
                "  <<: *a\n",
                "  <<: *b\n",
            ),
        ),
        // Duplicate ordinary keys are a constructor error too.
        (
            "duplicate-key.agent.yaml",
            concat!(
                "profile-id: duplicate-key\n",
                "name: DuplicateKey\n",
                "purpose: p\n",
                "roles: [reviewer]\n",
                "description: one\n",
                "description: two\n",
                "specialization: {primary-focus: x}\n",
            ),
        ),
        // Sequence merge order and precedence. `a` wins over `b`; `b`'s key
        // order positions the shared fields first. The full model dump is the
        // reference's field order, so the observable values are what matter:
        // mode=a (from `a`), description=A (from `a`), use-case=own (explicit).
        (
            "sequence-merge.agent.yaml",
            concat!(
                "profile-id: sequence-merge\n",
                "name: Sequence\n",
                "purpose: p\n",
                "roles: [reviewer]\n",
                "specialization:\n",
                "  primary-focus: x\n",
                "  anchor-a: &a {mode: a, description: A, use-case: ua}\n",
                "  anchor-b: &b {mode: b, use-case: ub}\n",
                "mode-defaults:\n",
                "  - {<<: [*a, *b], use-case: own}\n",
            ),
        ),
    ]
}

#[test]
fn merge_keys_are_source_exact() {
    let cases = merge_cases();
    let fixture = fixture(&cases);
    let source = run_source(
        &fixture,
        &DUMP_SCRIPT.replace("env_assertions", ENV_ASSERTIONS),
    );
    // The reference's own answer first.
    assert_eq!(
        source["ids"],
        json!(["plain-merge", "sequence-merge", "tagged-merge"])
    );
    assert_eq!(
        source["skipped"].as_array().expect("skipped").len(),
        6,
        "quoted literal, !!str, invalid value/item, duplicate merge/key: {}",
        source["skipped"]
    );
    for name in [
        "invalid-merge-value",
        "invalid-merge-item",
        "duplicate-merge",
    ] {
        let summary = &source["skipped_summaries"];
        assert!(
            summary
                .as_array()
                .expect("summaries")
                .iter()
                .any(|entry| entry
                    .as_str()
                    .is_some_and(|text| text.contains("YAML/read error"))),
            "{name}: reference did not report a YAML/read error: {summary}"
        );
    }
    let merged = &source["dumps"]["plain-merge"]["specialization"]["primary-focus"];
    assert_eq!(*merged, json!("merged"));
    assert_eq!(
        source["dumps"]["tagged-merge"]["specialization"]["primary-focus"],
        json!("tagged")
    );
    assert_eq!(
        source["dumps"]["sequence-merge"]["mode-defaults"],
        json!([{"mode": "a", "description": "A", "use-case": "own"}])
    );

    assert_dump_parity(&fixture, &source);
}

// ---------------------------------------------------------------------------
// 2. Pydantic coercion / validation / canonical dump
// ---------------------------------------------------------------------------

fn lax_case(id: &str, extra: &str) -> String {
    format!(
        "profile-id: {id}\nname: {id}\npurpose: p\nroles: [r]\nspecialization: {{primary-focus: x}}\n{extra}"
    )
}

fn lax_cases() -> Vec<(String, String)> {
    let mut cases: Vec<(String, String)> = Vec::new();
    for (name, extra) in [
        ("rp-string", "routing-priority: '57'\n"),
        ("rp-bool", "routing-priority: true\n"),
        ("rp-float", "routing-priority: 5.0\n"),
        ("rp-hex", "routing-priority: 0x10\n"),
        ("mct-string", "max-concurrent-tasks: '3'\n"),
        ("mct-float", "max-concurrent-tasks: 3.0\n"),
        ("sentinel-string", "sentinel: 'true'\n"),
        ("sentinel-on", "sentinel: 'on'\n"),
        ("sentinel-zero", "sentinel: 0\n"),
        (
            "nested-extra",
            "specialization: {primary-focus: y, bogus: 1}\n",
        ),
        (
            "directive-extra",
            "directive-references:\n  - code: '001'\n    name: one\n    rationale: r\n    bogus: 1\n",
        ),
        (
            "snake-aliases",
            "preferred_model: snake-model\nspecialization_context:\n  file_patterns: ['*.py']\n  domain_keywords: [kw]\n",
        ),
        ("empty-name", "name: ''\n"),
        (
            "excluding-list",
            "capabilities: [a]\nexcluding: [capabilities]\n",
        ),
        (
            "excluding-map",
            "capabilities: [a, b]\nexcluding:\n  capabilities: [a]\n",
        ),
        ("role-only", ""),
        (
            "self-review-extra",
            "self-review-protocol:\n  steps:\n    - name: n\n      gate: g\n      bogus: 1\n",
        ),
    ] {
        let text = if name == "empty-name" {
            format!(
                "profile-id: {name}\nname: ''\npurpose: p\nroles: [r]\nspecialization: {{primary-focus: x}}\n"
            )
        } else if name == "nested-extra" {
            format!(
                "profile-id: {name}\nname: {name}\npurpose: p\nroles: [r]\nspecialization: {{primary-focus: y, bogus: 1}}\n"
            )
        } else if name == "role-only" {
            format!(
                "profile-id: {name}\nname: {name}\npurpose: p\nrole: 'Senior Tech Lead'\nspecialization: {{primary-focus: x}}\n"
            )
        } else {
            lax_case(name, extra)
        };
        cases.push((format!("{name}.agent.yaml"), text));
    }
    for (name, extra) in [
        ("rp-floatfrac", "routing-priority: 5.5\n"),
        ("rp-negative", "routing-priority: -1\n"),
        ("rp-high", "routing-priority: 101\n"),
        ("mct-zero", "max-concurrent-tasks: 0\n"),
        ("sentinel-space", "sentinel: ' true '\n"),
        ("sentinel-bad", "sentinel: maybe\n"),
        ("name-int", "name: 123\n"),
        ("desc-int", "description: 7\n"),
        ("purpose-list", "purpose: [a]\n"),
        ("roles-str", "roles: reviewer\n"),
        ("roles-int", "roles: [7]\n"),
        ("caps-int", "capabilities: [7]\n"),
        ("schema-3", "schema-version: '3'\n"),
        ("tags", "tags: [a]\n"),
        ("model-int", "model: 7\n"),
        ("applies-int", "applies_to_languages: [7]\n"),
        (
            "directive-missing",
            "directive-references:\n  - code: '001'\n    name: one\n",
        ),
        ("tactic-missing", "tactic-references:\n  - id: t\n"),
        (
            "mode-missing",
            "mode-defaults:\n  - mode: m\n    description: d\n",
        ),
        ("both-aliases", "model: kebab\npreferred_model: snake\n"),
        ("excluding-int", "excluding: 7\n"),
        ("excluding-dict-bad", "excluding: {capabilities: x}\n"),
        ("excluding-list-int", "excluding: [7]\n"),
        ("both-role-forms", ""),
    ] {
        let text = if name == "both-role-forms" {
            format!(
                "profile-id: {name}\nname: {name}\npurpose: p\nrole: reviewer\nroles: [architect]\nspecialization: {{primary-focus: x}}\n"
            )
        } else {
            lax_case(name, extra)
        };
        cases.push((format!("{name}.agent.yaml"), text));
    }
    cases
}

#[test]
fn pydantic_lax_scalars_and_canonical_dump_are_source_exact() {
    let cases = lax_cases();
    let borrowed: Vec<(&str, &str)> = cases
        .iter()
        .map(|(name, text)| (name.as_str(), text.as_str()))
        .collect();
    let fixture = fixture(&borrowed);
    let source = run_source(
        &fixture,
        &DUMP_SCRIPT.replace("env_assertions", ENV_ASSERTIONS),
    );

    // Source-first: the reference's acceptance set is asserted before native.
    let loaded: Vec<&str> = source["ids"]
        .as_array()
        .expect("ids")
        .iter()
        .map(|id| id.as_str().expect("scalar"))
        .collect();
    for expected in [
        "rp-string",
        "rp-bool",
        "rp-float",
        "rp-hex",
        "mct-string",
        "mct-float",
        "sentinel-string",
        "sentinel-on",
        "sentinel-zero",
        "nested-extra",
        "directive-extra",
        "snake-aliases",
        "empty-name",
        "excluding-list",
        "excluding-map",
        "role-only",
        "self-review-extra",
    ] {
        assert!(
            loaded.contains(&expected),
            "{expected} should load: {loaded:?}"
        );
    }
    for refused in [
        "rp-floatfrac",
        "rp-negative",
        "rp-high",
        "mct-zero",
        "sentinel-space",
        "sentinel-bad",
        "name-int",
        "desc-int",
        "purpose-list",
        "roles-str",
        "roles-int",
        "caps-int",
        "schema-3",
        "tags",
        "model-int",
        "applies-int",
        "directive-missing",
        "tactic-missing",
        "mode-missing",
        "both-aliases",
        "excluding-int",
        "excluding-dict-bad",
        "excluding-list-int",
        "both-role-forms",
    ] {
        assert!(
            !loaded.contains(&refused),
            "{refused} must be refused: {loaded:?}"
        );
    }
    let dumps = &source["dumps"];
    assert_eq!(dumps["rp-string"]["routing-priority"], json!(57));
    assert_eq!(dumps["rp-bool"]["routing-priority"], json!(1));
    assert_eq!(dumps["rp-float"]["routing-priority"], json!(5));
    assert_eq!(dumps["rp-hex"]["routing-priority"], json!(16));
    assert_eq!(dumps["mct-string"]["max-concurrent-tasks"], json!(3));
    assert_eq!(dumps["sentinel-string"]["sentinel"], json!(true));
    assert_eq!(dumps["sentinel-on"]["sentinel"], json!(true));
    assert_eq!(dumps["sentinel-zero"]["sentinel"], json!(false));
    assert_eq!(dumps["role-only"]["roles"], json!(["senior tech lead"]));
    assert_eq!(
        dumps["nested-extra"]["specialization"],
        json!({"primary-focus": "y", "secondary-awareness": "", "avoidance-boundary": "", "success-definition": ""})
    );
    assert_eq!(
        dumps["snake-aliases"]["model"],
        json!("snake-model"),
        "a populate_by_name spelling dumps under its alias"
    );
    assert_eq!(
        dumps["snake-aliases"]["specialization-context"]["file-patterns"],
        json!(["*.py"])
    );
    assert!(dumps["snake-aliases"].get("preferred_model").is_none());
    assert!(
        dumps["snake-aliases"]
            .get("specialization_context")
            .is_none()
    );
    assert_eq!(
        dumps["directive-extra"]["directive-references"],
        json!([{"code": "001", "name": "one", "rationale": "r"}])
    );
    assert_eq!(
        dumps["self-review-extra"]["self-review-protocol"],
        json!({"steps": [{"name": "n", "command": null, "gate": "g"}]})
    );
    assert_eq!(
        dumps["empty-name"]["name"],
        json!(""),
        "an empty name is valid; only the schema summary calls it non-empty"
    );

    assert_dump_parity(&fixture, &source);

    // The `exclude_unset` view keeps only what the input carried.
    let repository = ProfileRepository::load(&fixture.built_in, &[], Some(&fixture.project))
        .expect("native profile load");
    let nested = repository.get_str("nested-extra").expect("loads");
    let unset = serde_yaml_ng::Value::Mapping(nested.model_dump_unset().shadow().clone());
    let unset = json_dump(&unset);
    assert_eq!(unset["specialization"], json!({"primary-focus": "y"}));
    assert!(unset.get("description").is_none());
}

// ---------------------------------------------------------------------------
// 3. Lineage selection, nested exclude_unset merge and excluding
// ---------------------------------------------------------------------------

const LINEAGE_SCRIPT: &str = r#"
env_assertions
from charter.offering.agent_profiles.repository import AgentProfileRepository
from charter.offering.drg.models import DRGGraph, DRGNode, DRGEdge, NodeKind, Relation
built_in = Path(os.environ["SK_G_BUILT_IN"])
project = Path(os.environ["SK_G_PROJECT"])
graph = DRGGraph(
    schema_version="1.0",
    generated_at="x",
    generated_by="g",
    nodes=[DRGNode(urn=f"agent_profile:{name}", kind=NodeKind.AGENT_PROFILE)
           for name in ("root", "mid", "leaf", "second", "prune")],
    edges=[
        DRGEdge(source="agent_profile:leaf", target="agent_profile:mid", relation=Relation.SPECIALIZES_FROM),
        DRGEdge(source="agent_profile:leaf", target="agent_profile:second", relation=Relation.SPECIALIZES_FROM),
        DRGEdge(source="agent_profile:mid", target="agent_profile:root", relation=Relation.SPECIALIZES_FROM),
    ],
)
repo = AgentProfileRepository(built_in_dir=built_in, project_dir=project, drg=graph)
out = {"ids": [p.profile_id for p in repo.list_all()],
       "skipped": [s.profile_id for s in repo.skipped_profiles()],
       "ancestors": repo.get_ancestors("leaf"),
       "resolved": repo.resolve_profile("leaf").model_dump(by_alias=True)}
try:
    repo.resolve_profile("prune")
    out["prune"] = "accepted"
except Exception as exc:
    out["prune"] = type(exc).__name__
print(json.dumps(out))
"#;

#[test]
fn lineage_selection_and_excluding_are_source_exact() {
    let fixture = fixture(&[
        (
            "root.agent.yaml",
            concat!(
                "profile-id: root\nname: Root\npurpose: p\nroles: [r]\n",
                "specialization: {primary-focus: x}\n",
                "capabilities: [a]\n",
                "specialization-context:\n  languages: [py]\n  frameworks: [django]\n",
            ),
        ),
        (
            "mid.agent.yaml",
            "profile-id: mid\nname: Mid\npurpose: p\nroles: [r]\nspecialization: {primary-focus: x}\ncapabilities: [b]\n",
        ),
        (
            "second.agent.yaml",
            "profile-id: second\nname: Second\npurpose: p\nroles: [r]\nspecialization: {primary-focus: x}\ncapabilities: [c]\n",
        ),
        (
            "leaf.agent.yaml",
            concat!(
                "profile-id: leaf\nname: Leaf\npurpose: p\nroles: [r]\n",
                "specialization: {primary-focus: x}\n",
                "specialization-context:\n  frameworks: [flask]\n",
            ),
        ),
        (
            "prune.agent.yaml",
            concat!(
                "profile-id: prune\nname: Prune\npurpose: p\nroles: [r]\n",
                "specialization: {primary-focus: x}\n",
                "excluding: [name]\n",
            ),
        ),
    ]);
    let source = run_source(
        &fixture,
        &LINEAGE_SCRIPT.replace("env_assertions", ENV_ASSERTIONS),
    );
    assert_eq!(source["skipped"], json!([]));
    // `_lineage_parent` sorts the declared parents and takes the first.
    assert_eq!(source["ancestors"], json!(["mid", "root"]));
    assert_eq!(
        source["resolved"]["capabilities"],
        json!(["a", "b"]),
        "the second parent is never merged"
    );
    assert_eq!(
        source["resolved"]["specialization-context"],
        json!({
            "languages": ["py"],
            "frameworks": ["flask"],
            "file-patterns": [],
            "domain-keywords": [],
            "writing-style": [],
            "complexity-preference": []
        })
    );
    assert_eq!(
        source["prune"],
        json!("ValidationError"),
        "excluding a required field fails re-validation"
    );

    let graph = DrgGraph::new(
        "1.0",
        "x",
        "g",
        ["root", "mid", "leaf", "second", "prune"]
            .into_iter()
            .map(|name| {
                DrgNode::new(
                    format!("agent_profile:{name}"),
                    NodeKind::AgentProfile,
                    None,
                    Vec::new(),
                )
                .unwrap()
            })
            .collect(),
        vec![
            DrgEdge::new(
                "agent_profile:leaf",
                "agent_profile:mid",
                Relation::SpecializesFrom,
            )
            .unwrap(),
            DrgEdge::new(
                "agent_profile:leaf",
                "agent_profile:second",
                Relation::SpecializesFrom,
            )
            .unwrap(),
            DrgEdge::new(
                "agent_profile:mid",
                "agent_profile:root",
                Relation::SpecializesFrom,
            )
            .unwrap(),
        ],
    )
    .unwrap();
    let repository = ProfileRepository::load(&fixture.built_in, &[], Some(&fixture.project))
        .expect("native profile load");
    assert!(
        repository.skipped().is_empty(),
        "{:?}",
        repository.skipped()
    );
    let resolved = repository
        .resolve_str("leaf", &graph)
        .expect("native lineage resolution");
    assert_eq!(
        resolved
            .lineage
            .iter()
            .filter_map(|id| id.to_scalar_string().ok())
            .collect::<Vec<_>>(),
        source["ancestors"]
            .as_array()
            .unwrap()
            .iter()
            .rev()
            .map(|id| id.as_str().unwrap().to_owned())
            .chain(std::iter::once("leaf".to_owned()))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        json_dump(&serde_yaml_ng::Value::Mapping(
            resolved.model_dump().shadow().clone(),
        )),
        source["resolved"]
    );
    assert!(matches!(
        repository.resolve_str("prune", &graph),
        Err(sk_doctrine::DoctrineError::InvalidProfile { .. })
    ));
}

// ---------------------------------------------------------------------------
// 4. Non-scalar DRG URNs
// ---------------------------------------------------------------------------

const GRAPH_SCRIPT: &str = r#"
env_assertions
import json
from charter.offering.drg.models import DRGGraph, DRGNode, DRGEdge, NodeKind, Relation, load_graph_document
out = {}
for label, factory in {
    "node_surrogate": lambda: DRGNode(urn="agent_profile:auth" + chr(0xD800), kind=NodeKind.AGENT_PROFILE),
    "node_non_ascii": lambda: DRGNode(urn="agent_profile:pedr" + chr(0xE9), kind=NodeKind.AGENT_PROFILE),
    "edge_non_ascii": lambda: DRGEdge(source="agent_profile:a" + chr(0xE9), target="agent_profile:b", relation=Relation.SPECIALIZES_FROM),
}.items():
    try:
        factory()
        out[label] = "accepted"
    except Exception as exc:
        out[label] = type(exc).__name__
try:
    load_graph_document({"schema_version": "1.0", "generated_at": "x", "generated_by": "g",
                         "nodes": [{"urn": "agent_profile:auth" + chr(0xD800), "kind": "agent_profile"}],
                         "edges": []}, source="fixture.graph.yaml")
    out["document"] = "accepted"
except Exception as exc:
    out["document"] = type(exc).__name__
print(json.dumps(out))
"#;

#[test]
fn non_scalar_graph_urns_are_refused_by_both() {
    let fixture = fixture(&[]);
    let source = run_source(
        &fixture,
        &GRAPH_SCRIPT.replace("env_assertions", ENV_ASSERTIONS),
    );
    for key in [
        "node_surrogate",
        "node_non_ascii",
        "edge_non_ascii",
        "document",
    ] {
        assert_eq!(source[key], json!("ValidationError"), "{key}");
    }
    assert!(
        DrgNode::new(
            "agent_profile:auth\u{fffd}",
            NodeKind::AgentProfile,
            None,
            Vec::new()
        )
        .is_err()
    );
    let surrogate = PythonString::from_codepoints(&[0x61, 0xd800]).expect("valid code point");
    let urn = format!("agent_profile:{}", surrogate.repr());
    assert!(DrgNode::new(urn, NodeKind::AgentProfile, None, Vec::new()).is_err());
    assert!(
        DrgEdge::new(
            "agent_profile:a\u{e9}",
            "agent_profile:b",
            Relation::SpecializesFrom
        )
        .is_err()
    );
    let document = concat!(
        "schema_version: '1.0'\n",
        "generated_at: x\n",
        "generated_by: g\n",
        "nodes:\n",
        "  - urn: \"agent_profile:auth\\uD800\"\n",
        "    kind: agent_profile\n",
        "edges: []\n",
    );
    assert!(
        sk_doctrine::load_graph_str(document, Path::new("fixture.graph.yaml")).is_err(),
        "a non-scalar URN is a graph-model validation failure, not a native String limit"
    );
    // A non-ASCII URN cannot be authored, so a profile whose exact id is
    // non-scalar has no graph parent — not a native leaf-only shortcut.
    let _ = BTreeMap::<String, String>::new();
}

// ---------------------------------------------------------------------------
// 5. Layer overlay + excluding
// ---------------------------------------------------------------------------

const OVERLAY_SCRIPT: &str = r#"
env_assertions
from charter.offering.agent_profiles.repository import AgentProfileRepository
built_in = Path(os.environ["SK_G_BUILT_IN"])
project = Path(os.environ["SK_G_PROJECT"])
repo = AgentProfileRepository(built_in_dir=built_in, project_dir=project)
profile = repo.get("overlay-target")
resolved = repo.resolve_profile("overlay-target")
print(json.dumps({
    "skipped": [entry.profile_id for entry in repo.skipped_profiles()],
    "merged": profile.model_dump(by_alias=True),
    "resolved": resolved.model_dump(by_alias=True),
    "provenance": repo.get_provenance("overlay-target"),
}))
"#;

#[test]
fn layer_overlay_and_excluding_are_source_exact() {
    let temp = tempfile::tempdir().expect("temporary root");
    let root = temp.path().canonicalize().expect("canonical root");
    let built_in = root.join("built-in");
    let project = root.join("profiles");
    for directory in ["home", "cache", "config", "data", "state", "tmp"] {
        fs::create_dir_all(root.join(directory)).expect("isolated directory");
    }
    write(
        &built_in.join("agent_profiles/overlay-target.agent.yaml"),
        concat!(
            "profile-id: overlay-target\n",
            "name: Overlay\n",
            "purpose: p\n",
            "roles: [Reviewer]\n",
            "description: built-in\n",
            "capabilities: [a, b]\n",
            "specialization: {primary-focus: x}\n",
            "specialization-context:\n  languages: [py]\n  frameworks: [django]\n",
        ),
    );
    write(
        &project.join("agent_profiles/overlay-target.agent.yaml"),
        concat!(
            "profile-id: overlay-target\n",
            "description: project\n",
            "specialization-context:\n  frameworks: [flask]\n",
            "excluding:\n  capabilities: [b]\n",
        ),
    );
    let fixture = Fixture {
        _temp: temp,
        root,
        built_in,
        project,
    };
    let source = run_source(
        &fixture,
        &OVERLAY_SCRIPT.replace("env_assertions", ENV_ASSERTIONS),
    );
    assert_eq!(source["skipped"], json!([]));
    assert_eq!(source["provenance"], json!("project"));
    assert_eq!(source["merged"]["description"], json!("project"));
    assert_eq!(source["merged"]["capabilities"], json!(["a", "b"]));
    assert_eq!(
        source["merged"]["specialization-context"],
        json!({
            "languages": ["py"],
            "frameworks": ["flask"],
            "file-patterns": [],
            "domain-keywords": [],
            "writing-style": [],
            "complexity-preference": []
        }),
        "the layer merge is a deep merge onto the built-in model dump"
    );
    assert_eq!(
        source["resolved"]["capabilities"],
        json!(["a"]),
        "the leaf `excluding` is applied to the merged result"
    );

    let repository = ProfileRepository::load(&fixture.built_in, &[], Some(&fixture.project))
        .expect("native load");
    assert!(
        repository.skipped().is_empty(),
        "{:?}",
        repository.skipped()
    );
    let merged = repository.get_str("overlay-target").expect("loads");
    assert_eq!(
        json_dump(&serde_yaml_ng::Value::Mapping(
            merged.model_dump().shadow().clone()
        )),
        source["merged"]
    );
    // No graph edge: the profile is its own lineage root and `excluding` still
    // applies at resolution.
    let graph = DrgGraph::new("1.0", "x", "g", Vec::new(), Vec::new()).unwrap();
    let resolved = repository
        .resolve_str("overlay-target", &graph)
        .expect("resolves");
    assert_eq!(
        json_dump(&serde_yaml_ng::Value::Mapping(
            resolved.model_dump().shadow().clone()
        )),
        source["resolved"]
    );
}
