//! CF31 — canonical model dumps for the four pending include/body kinds.
//!
//! The reference renders an artifact body from a **validated Pydantic model**
//! (`_jsonable_artifact_value` → `model_dump(by_alias=True, mode="json",
//! exclude_none=True)`), which is why a body carries model defaults the author
//! omitted, drops `null` optionals, coerces lax scalars and serialises an
//! `ArtifactKind` enum as its canonical value. These tests pin that projection
//! for `glossary_pack`, `paradigm`, `skill` (`PackSkill`) and
//! `mission_step_contract`, which had no canonical dump at all.
//!
//! `serde_json`'s map orders keys, and the reference's
//! `json.dumps(..., sort_keys=True)` sorts them too, so field *order* is not
//! observable in the rendered payload; the field *set*, presence, default and
//! value type are, and those are what is asserted here.
//!
//! The last test is a genuine paired run: the pinned Source child and a
//! re-executed Native child each assert their own CWD/HOME/seven XDG roots/
//! TMPDIR/TMP/TEMP, absolute `PYTHONPATH` and the pinned packs root, then render
//! the `Full artifact:` block for the same four real documents.

mod support;

#[path = "support/isolated.rs"]
mod isolated;

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Value, json};
use sk_charter::ArtifactKind;
use sk_doctrine::{canonical_artifact_body, context_render::include_full_payload_lines};

fn parse(text: &str) -> serde_yaml_ng::Value {
    serde_yaml_ng::from_str(text).expect("fixture YAML")
}

fn dump(kind: ArtifactKind, text: &str) -> Value {
    canonical_artifact_body(kind, &parse(text)).expect("canonical model dump")
}

fn body(kind: ArtifactKind, text: &str) -> Value {
    dump(kind, text)
}

fn keys(value: &Value) -> Vec<String> {
    value
        .as_object()
        .expect("object")
        .keys()
        .cloned()
        .collect::<Vec<String>>()
}

const GLOSSARY_PACK: &str = "\
id: spec-kitty-core
provenance: built-in
description: Canonical terminology.
terms:
  - surface: action context
    definition: Aggregated runtime information.
    confidence: 0.95
    status: active
  - surface: adversary
    definition: A reviewing lens.
    confidence: '0_5.5'
    status: draft
    see_also: [reviewer]
    introduced_in_mission: m-1
    synonyms_to_avoid: []
    aliases: [adv]
    banned_synonyms: []
";

const PARADIGM: &str = "\
schema_version: '1.0'
id: git-flow
name: Git Flow
summary: Long-lived integration branches.
directive_refs:
  - DIRECTIVE_029
references:
  - type: directive
    id: DIRECTIVE_030
";

const PACK_SKILL: &str = "\
schema_version: '1.0'
id: issue-triage
title: Triage GitHub issues
description: Use to triage a batch of open issues.
triggers: [triage issues, triage the tracker]
form: prompt
body_path: issue-triage.skill.md
parameters:
  - name: scope
    required: 'yes'
    description: Which issues
invocation:
  user_invocable: true
  model_invocable: true
  side_effects: [gh-write]
maintainers: ['@spec-kitty/core']
";

const STEP_CONTRACT: &str = "\
schema_version: '1.0'
id: plan
action: plan
mission: software-dev
steps:
  - id: bootstrap
    description: Load charter context for this action
    command: \"spec-kitty charter context --action plan --json\"
    inputs:
      - flag: --mission
        source: mission_slug
        optional: 1
  - id: read_spec
    description: Read the mission specification
    delegates_to:
      kind: directive
      candidates:
        - DIRECTIVE_010
gates:
  - on_transition: for_review->approved
    handler: review_gate
    schema_version: '1.0'
    fail_open: 'no'
    provenance: built-in
";

#[test]
fn glossary_pack_dump_materialises_required_terms_and_drops_absent_optionals() {
    let body = body(ArtifactKind::GlossaryPack, GLOSSARY_PACK);
    assert_eq!(
        keys(&body),
        vec!["description", "id", "provenance", "terms"],
        "the schema-only dump has exactly the model's fields"
    );
    assert_eq!(body["id"], json!("spec-kitty-core"));
    assert_eq!(body["provenance"], json!("built-in"));
    let terms = body["terms"].as_array().expect("terms");
    assert_eq!(terms.len(), 2);
    // A required term without an authored optional list carries none of them.
    assert_eq!(
        keys(&terms[0]),
        vec!["confidence", "definition", "status", "surface"]
    );
    assert_eq!(terms[0]["confidence"], json!(0.95));
    // A lax float spelling is dumped as the coerced number, and authored
    // optional lists stay present even when empty.
    assert_eq!(terms[1]["confidence"], json!(5.5));
    assert_eq!(
        keys(&terms[1]),
        vec![
            "aliases",
            "banned_synonyms",
            "confidence",
            "definition",
            "introduced_in_mission",
            "see_also",
            "status",
            "surface",
            "synonyms_to_avoid"
        ]
    );
    assert_eq!(terms[1]["see_also"], json!(["reviewer"]));
    assert_eq!(terms[1]["synonyms_to_avoid"], json!([]));
}

#[test]
fn paradigm_dump_emits_default_lists_and_canonical_enum_values() {
    let body = body(ArtifactKind::Paradigm, PARADIGM);
    assert_eq!(
        keys(&body),
        vec![
            "directive_refs",
            "id",
            "name",
            "references",
            "schema_version",
            "summary"
        ]
    );
    assert_eq!(body["schema_version"], json!("1.0"));
    assert_eq!(body["directive_refs"], json!(["DIRECTIVE_029"]));
    assert_eq!(
        body["references"],
        json!([{"type": "directive", "id": "DIRECTIVE_030"}])
    );
    // `directive_refs`/`references` are `default_factory=list`.
    let minimal = dump(
        ArtifactKind::Paradigm,
        "schema_version: '1.0'\nid: p\nname: P\nsummary: S\n",
    );
    assert_eq!(minimal["directive_refs"], json!([]));
    assert_eq!(minimal["references"], json!([]));
}

#[test]
fn pack_skill_dump_emits_model_defaults_and_forces_model_invocable_off() {
    let body = body(ArtifactKind::Skill, PACK_SKILL);
    assert_eq!(
        keys(&body),
        vec![
            "body_path",
            "description",
            "form",
            "id",
            "invocation",
            "maintainers",
            "parameters",
            "schema_version",
            "title",
            "tools",
            "triggers",
            "version"
        ],
        "absent optionals are dropped; defaulted fields are materialised"
    );
    assert_eq!(body["tools"], json!(["*"]));
    assert_eq!(body["version"], json!("1.0.0"));
    // The model's `mode="before"` validator forces the declaration off.
    assert_eq!(
        body["invocation"],
        json!({"user_invocable": true, "model_invocable": false, "side_effects": ["gh-write"]})
    );
    let parameter = &body["parameters"][0];
    assert_eq!(
        keys(parameter),
        vec!["description", "name", "required"],
        "parameter defaults are present, an absent `default` is not"
    );
    assert_eq!(parameter["required"], json!(true));
    // A skill with no invocation authored takes the full nested default.
    let minimal = dump(
        ArtifactKind::Skill,
        "schema_version: '1.0'\nid: s\ntitle: T\ndescription: D\nform: wrapper\nexpands_to:\n  target: builtin:x\n",
    );
    assert_eq!(
        minimal["invocation"],
        json!({"user_invocable": true, "model_invocable": true, "side_effects": []})
    );
    assert_eq!(
        minimal["expands_to"],
        json!({"target": "builtin:x", "args": ""})
    );
    assert_eq!(minimal["parameters"], json!([]));
    assert_eq!(minimal["triggers"], json!([]));
    assert_eq!(minimal["maintainers"], json!([]));
}

#[test]
fn mission_step_contract_dump_emits_gate_and_step_defaults() {
    let body = body(ArtifactKind::MissionStepContract, STEP_CONTRACT);
    assert_eq!(
        keys(&body),
        vec![
            "action",
            "gates",
            "id",
            "mission",
            "schema_version",
            "steps"
        ]
    );
    let steps = body["steps"].as_array().expect("steps");
    assert_eq!(steps[0]["inputs"][0]["optional"], json!(true));
    assert_eq!(
        keys(&steps[1]),
        vec!["delegates_to", "description", "id", "inputs"],
        "an absent optional `command`/`guidance` is dropped; the `inputs`\n         `default_factory=list` default is materialised"
    );
    assert_eq!(
        steps[1]["delegates_to"],
        json!({"kind": "directive", "candidates": ["DIRECTIVE_010"]})
    );
    assert_eq!(
        body["gates"][0],
        json!({
            "on_transition": "for_review->approved",
            "handler": "review_gate",
            "handler_kind": "mission_step_contract",
            "schema_version": "1.0",
            "fail_open": false,
            "provenance": "built-in"
        })
    );
    // No gates authored: the `default_factory=list` default is present.
    let no_gates = dump(
        ArtifactKind::MissionStepContract,
        "schema_version: '1.0'\nid: c\naction: a\nmission: m\nsteps:\n  - id: one\n    description: d\n",
    );
    assert_eq!(no_gates["gates"], json!([]));
    assert_eq!(no_gates["steps"][0]["inputs"], json!([]));
    assert!(no_gates["steps"][0].get("delegates_to").is_none());
}

#[test]
fn untyped_kinds_still_have_no_model_dump() {
    for kind in [
        ArtifactKind::Asset,
        ArtifactKind::Template,
        ArtifactKind::AntiPattern,
    ] {
        assert!(
            canonical_artifact_body(kind, &parse("id: a")).is_none(),
            "{kind:?}"
        );
    }
    // A non-mapping document has no dump either.
    assert!(canonical_artifact_body(ArtifactKind::Paradigm, &parse("- a")).is_none());
}

// ---------------------------------------------------------------------------
// Real built-in documents (the same files the include/delivery owners read)
// ---------------------------------------------------------------------------

fn repository_file(relative: &str) -> String {
    let path = isolated::repository_root()
        .join("packs/built-in")
        .join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

#[test]
fn real_built_in_documents_project_their_declared_ids() {
    let glossary = body(
        ArtifactKind::GlossaryPack,
        &repository_file("glossary_packs/spec-kitty-core.glossary-pack.yaml"),
    );
    assert_eq!(glossary["id"], json!("spec-kitty-core"));
    assert!(
        glossary["terms"]
            .as_array()
            .is_some_and(|terms| terms.len() > 100)
    );

    let paradigm = body(
        ArtifactKind::Paradigm,
        &repository_file("paradigms/git-flow.paradigm.yaml"),
    );
    assert_eq!(paradigm["id"], json!("git-flow"));
    assert_eq!(paradigm["name"], json!("Git Flow"));

    let contract = body(
        ArtifactKind::MissionStepContract,
        &repository_file("missions/built_in_step_contracts/plan.step-contract.yaml"),
    );
    assert_eq!(contract["id"], json!("plan"));
    assert_eq!(contract["action"], json!("plan"));
    assert_eq!(contract["mission"], json!("software-dev"));
    assert!(
        contract["steps"]
            .as_array()
            .is_some_and(|steps| steps.len() > 3)
    );
}

// ---------------------------------------------------------------------------
// Paired Source/Native proof (both actual isolated children)
// ---------------------------------------------------------------------------

/// The reference renderer, run over the real Pydantic models.
const SOURCE_SCRIPT: &str = r#"
import json, os, pathlib, sys
root = pathlib.Path(os.environ["SK_E4_ROOT"])
assert pathlib.Path.cwd() == root
assert sys.version_info[:3] == (3, 11, 15), sys.version_info
repo = pathlib.Path(os.environ["PYTHONPATH"]).parent
assert pathlib.Path(os.environ["PYTHONPATH"]) == repo / "src"
assert pathlib.Path(os.environ["SPEC_KITTY_PACKS_ROOT"]) == repo / "packs"
for key, name in [
    ("HOME", "home"), ("XDG_CONFIG_HOME", "config"), ("XDG_CACHE_HOME", "cache"),
    ("XDG_DATA_HOME", "data"), ("XDG_STATE_HOME", "state"), ("XDG_RUNTIME_DIR", "runtime"),
    ("XDG_CONFIG_DIRS", "config-dirs"), ("XDG_DATA_DIRS", "data-dirs"),
    ("TMPDIR", "tmp"), ("TMP", "tmp"), ("TEMP", "tmp"),
]:
    assert pathlib.Path(os.environ[key]) == root / name, (key, os.environ[key])
from ruamel.yaml import YAML
from charter.activation.context_renderers.artifact_bodies import _format_full_artifact_payload_body
from charter.offering.glossary_packs.models import GlossaryPack
from charter.offering.paradigms.models import Paradigm
from charter.offering.pack_skills.models import PackSkill
from charter.offering.missions.step_contracts import MissionStepContract
MODELS = {
    "glossary_pack": GlossaryPack,
    "paradigm": Paradigm,
    "skill": PackSkill,
    "mission_step_contract": MissionStepContract,
}
docs = {}
for kind, path in json.loads(os.environ["SK_CF31_DOCS"]).items():
    data = YAML(typ="safe").load(pathlib.Path(path).read_text(encoding="utf-8"))
    docs[kind] = _format_full_artifact_payload_body(MODELS[kind].model_validate(data))
print("CF31_RECEIPT " + json.dumps({
    "runner": "source-python-" + ".".join(str(part) for part in sys.version_info[:3]),
    "cwd": str(pathlib.Path.cwd()),
    "docs": docs,
}))
"#;

/// The four documents: three real built-ins plus a fresh skill fixture.
fn fixtures(root: &Path) -> BTreeMap<String, String> {
    let skill = root.join("skill/issue-triage.skill.yaml");
    std::fs::create_dir_all(skill.parent().expect("skill dir")).expect("skill dir");
    std::fs::write(&skill, PACK_SKILL).expect("skill fixture");
    BTreeMap::from([
        (
            "glossary_pack".to_owned(),
            isolated::repository_root()
                .join("packs/built-in/glossary_packs/spec-kitty-core.glossary-pack.yaml")
                .display()
                .to_string(),
        ),
        (
            "paradigm".to_owned(),
            isolated::repository_root()
                .join("packs/built-in/paradigms/git-flow.paradigm.yaml")
                .display()
                .to_string(),
        ),
        (
            "mission_step_contract".to_owned(),
            isolated::repository_root()
                .join("packs/built-in/missions/built_in_step_contracts/plan.step-contract.yaml")
                .display()
                .to_string(),
        ),
        ("skill".to_owned(), skill.display().to_string()),
    ])
}

fn parse_receipt(stdout: &str, marker: &str) -> (String, String, BTreeMap<String, Vec<String>>) {
    // The Native child is a libtest re-exec, so the marker can follow the
    // harness's own `test <name> ...` prefix on the same line.
    let index = stdout
        .find(marker)
        .unwrap_or_else(|| panic!("no {marker} receipt in {stdout:?}"));
    let line = stdout[index + marker.len()..]
        .lines()
        .next()
        .expect("receipt line");
    let value: Value = serde_json::from_str(line.trim()).expect("receipt JSON");
    let docs = value["docs"]
        .as_object()
        .expect("docs object")
        .iter()
        .map(|(kind, lines)| {
            (
                kind.clone(),
                lines
                    .as_array()
                    .expect("lines")
                    .iter()
                    .map(|line| line.as_str().expect("line").to_owned())
                    .collect(),
            )
        })
        .collect();
    (
        value["runner"].as_str().expect("runner").to_owned(),
        value["cwd"].as_str().expect("cwd").to_owned(),
        docs,
    )
}

/// The Native child: assert the isolated roots, dump the same documents, report.
#[test]
fn cf31_native_child_and_receipt() {
    let Some(root) = isolated::native_root() else {
        return;
    };
    isolated::assert_isolated(&root);
    for (key, name) in EXTRA_XDG_ROOTS {
        assert_eq!(
            std::env::var_os(key).map(std::path::PathBuf::from),
            Some(root.join(name)),
            "{key} is asserted inside the actual Native child"
        );
    }
    let docs: serde_json::Value =
        serde_json::from_str(&std::env::var("SK_CF31_DOCS").expect("docs env")).expect("docs JSON");
    let mut rendered = serde_json::Map::new();
    for (kind, path) in docs.as_object().expect("docs object") {
        let path = path.as_str().expect("path");
        let text = std::fs::read_to_string(path).expect("document read");
        let value = parse(&text);
        let canonical = ArtifactKind::from_canonical(kind).expect("canonical kind");
        let body = canonical_artifact_body(canonical, &value).expect("canonical dump");
        rendered.insert(
            kind.clone(),
            Value::Array(
                include_full_payload_lines(&body)
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            ),
        );
    }
    println!(
        "CF31_RECEIPT {}",
        json!({
            "runner": "native",
            "cwd": std::env::current_dir().expect("cwd").display().to_string(),
            "docs": rendered,
        })
    );
}

#[test]
fn source_and_native_full_artifact_payloads_agree_for_the_four_kinds() {
    let root = isolated::fixture("cf31-canonical-models");
    for (_, name) in EXTRA_XDG_ROOTS {
        std::fs::create_dir_all(root.join(name)).expect("extra isolated XDG directory");
    }
    let docs = fixtures(&root);
    let docs_env = serde_json::to_string(&docs).expect("docs JSON");
    let config_dirs = root.join("config-dirs");
    let data_dirs = root.join("data-dirs");
    let source_stdout = isolated::source_script(
        &root,
        SOURCE_SCRIPT,
        &[
            ("SK_CF31_DOCS", docs_env.as_str()),
            (
                "XDG_CONFIG_DIRS",
                config_dirs.to_str().expect("fixture path"),
            ),
            ("XDG_DATA_DIRS", data_dirs.to_str().expect("fixture path")),
        ],
    );
    let (source_runner, source_cwd, source_docs) = parse_receipt(&source_stdout, "CF31_RECEIPT ");
    assert_eq!(source_runner, "source-python-3.11.15");
    assert_eq!(source_cwd, root.display().to_string());

    let native_stdout = run_native_child(&root, &docs_env);
    let (native_runner, native_cwd, native_docs) = parse_receipt(&native_stdout, "CF31_RECEIPT ");
    assert_eq!(native_runner, "native");
    assert_eq!(native_cwd, root.display().to_string());

    assert_eq!(
        source_docs.keys().collect::<Vec<&String>>(),
        native_docs.keys().collect::<Vec<&String>>(),
        "both children rendered the same four documents"
    );
    for (kind, source_lines) in &source_docs {
        let native_lines = native_docs
            .get(kind)
            .unwrap_or_else(|| panic!("native receipt for {kind}"));
        assert!(
            source_lines.len() > 1,
            "{kind}: the reference rendered a payload"
        );
        assert_eq!(native_lines, source_lines, "{kind}: Full artifact payload");
    }
}

const EXTRA_XDG_ROOTS: [(&str, &str); 2] = [
    ("XDG_CONFIG_DIRS", "config-dirs"),
    ("XDG_DATA_DIRS", "data-dirs"),
];

/// Run the Native side as a re-executed child under the same isolated roots.
fn run_native_child(root: &Path, docs_env: &str) -> String {
    let repository = isolated::repository_root();
    let executable = std::env::current_exe().expect("test binary");
    let mut command = std::process::Command::new(executable);
    command
        .args([
            "--exact",
            "cf31_native_child_and_receipt",
            "--nocapture",
            "--test-threads=1",
        ])
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .current_dir(root)
        .env("SK_E4_NATIVE_ROOT", root)
        .env("SK_CF31_DOCS", docs_env)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONPATH", repository.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repository.join("packs"));
    for (key, name) in isolated::ROOTS.into_iter().chain(EXTRA_XDG_ROOTS) {
        command.env(key, root.join(name));
    }
    let output = command.output().expect("run the Native child");
    std::fs::write(root.join("cf31-native.stdout"), &output.stdout).expect("stdout receipt");
    std::fs::write(root.join("cf31-native.stderr"), &output.stderr).expect("stderr receipt");
    assert!(
        output.status.success(),
        "Native child failed (status {:?}): {} {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("native stdout is UTF-8")
}
