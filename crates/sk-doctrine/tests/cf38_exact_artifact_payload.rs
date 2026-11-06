//! CF38 — exact JSON-backed artifact payload: paired Source/Native proof.
//!
//! The reference renders the `Full artifact:` block from a validated Pydantic
//! model (`model_dump(by_alias=True, mode="json", exclude_none=True)` then
//! `json.dumps(..., indent=2, sort_keys=True)`). `serde_json` cannot carry that
//! domain, so CF38 added the exact Kernel `PythonJsonValue` payload
//! (`sk_doctrine::canonical_artifact_body_exact*`) and the exact renderer
//! (`sk_doctrine::context_render::include_full_payload_lines_exact`).
//!
//! This file proves:
//!
//! 1. the finite scalar adapter (`canonical_artifact_body` +
//!    `include_full_payload_lines`) is the scalar projection of the exact
//!    payload for every document it can represent — old/new equivalence; and
//! 2. the exact payload text matches the **actual pinned Source child** for the
//!    nine typed kinds and for an admitted document carrying non-finite typed
//!    floats, signed zero, non-ASCII text and constructed
//!    `!!set`-free iterables, with both children asserting their own CWD, HOME,
//!    seven XDG roots (including `XDG_CONFIG_DIRS`/`XDG_DATA_DIRS`),
//!    TMPDIR/TMP/TEMP, absolute `PYTHONPATH`, the pinned packs root and a pinned
//!    `PYTHONHASHSEED`.
//!
//! Constructed `!!set` order and marker-carried strings cannot travel through
//! the bare `serde_yaml_ng` tree the Native child parses; those are proved
//! in-crate against the canonical graph in `body::exact`'s unit tests. Nothing
//! here is a whole-include, CLI or workflow acceptance.

mod support;

#[path = "support/isolated.rs"]
mod isolated;

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Value as JsonValue, json};
use sk_charter::ArtifactKind;
use sk_doctrine::context_render::{include_full_payload_lines, include_full_payload_lines_exact};
use sk_doctrine::{
    canonical_artifact_body, canonical_artifact_body_exact_path,
    canonical_artifact_body_exact_value,
};

const EXTRA_XDG_ROOTS: [(&str, &str); 2] = [
    ("XDG_CONFIG_DIRS", "config-dirs"),
    ("XDG_DATA_DIRS", "data-dirs"),
];

/// The nine typed kinds' real built-in documents (repository-relative).
const REPOSITORY_DOCUMENTS: [(&str, &str); 9] = [
    (
        "directive",
        "packs/built-in/directives/010-specification-fidelity-requirement.directive.yaml",
    ),
    (
        "tactic",
        "packs/built-in/tactics/architecture/dependency-hygiene.tactic.yaml",
    ),
    (
        "styleguide",
        "packs/built-in/styleguides/common-docs.styleguide.yaml",
    ),
    (
        "toolguide",
        "packs/built-in/toolguides/sonar.toolguide.yaml",
    ),
    (
        "procedure",
        "packs/built-in/procedures/adversarial-squad-deployment.procedure.yaml",
    ),
    (
        "paradigm",
        "packs/built-in/paradigms/git-flow.paradigm.yaml",
    ),
    (
        "glossary_pack",
        "packs/built-in/glossary_packs/spec-kitty-core.glossary-pack.yaml",
    ),
    ("skill", "packs/internal/skills/issue-triage.skill.yaml"),
    (
        "mission_step_contract",
        "packs/built-in/missions/built_in_step_contracts/plan.step-contract.yaml",
    ),
];

/// Admitted documents exercising the reference's non-finite/exact domain.
///
/// Negative infinity is included after the parent fixed the canonical reader's
/// double sign. A remaining graph-only edge is proved in-crate:
///
/// * a lone surrogate (`"\ud800"`) — the bare `serde_yaml_ng` carrier refuses
///   that escape (SCANNER error) while the canonical transport carries it
///   (`body::exact::tests::typed_floats_and_exact_codepoints_survive_the_exact_payload`).
const GLOSSARY_EDGES: &str = "\
id: cf38-edges
provenance: cf38
description: CF38 exact payload edges.
terms:
  - surface: nan-term
    definition: nan confidence
    confidence: .nan
    status: active
  - surface: posinf-term
    definition: positive infinity
    confidence: .inf
    status: active
  - surface: neginf-term
    definition: negative infinity
    confidence: -.inf
    status: active
  - surface: negzero-term
    definition: signed zero
    confidence: -0.0
    status: active
  - surface: large-term
    definition: large finite float
    confidence: 1.2345678901234568e+300
    status: active
  - surface: unicode-term
    definition: héllo ☃
    confidence: 0.5
    status: active
";

/// Admitted unstructured content: constructed iterables keep their shape/order,
/// array nulls stay, mapping nulls go, non-finite floats inside `Any` become
/// `null`, and a non-ASCII value is escaped by the encoder.
const STYLEGUIDE_EDGES: &str = "\
id: cf38-edges
schema_version: '1.0'
title: CF38 edges
scope: code
principles: [one]
tooling:
  runner: runner-value
structural_lint_config:
  pairs_value: !!pairs [{a: b}, {a: c}]
  omap_value: !!omap [{x: 1}, {y: 2}]
  bytes_value: !!binary SGVsbG8=
  list_nulls: [1, null, 2]
  map_nulls: {kept: 1, dropped: null}
  nested: {deep: [.nan, .inf]}
  nonfinite: [.inf, -0.0]
  datetime: 2020-01-01T01:02:03Z
  unicode: \"héllo ☃\"
";

/// The reference renderer over the real Pydantic models.
const SOURCE_SCRIPT: &str = r#"
import json, os, pathlib, sys
root = pathlib.Path(os.environ["SK_E4_ROOT"])
assert pathlib.Path.cwd() == root, pathlib.Path.cwd()
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
assert os.environ["PYTHONHASHSEED"] == "0", os.environ["PYTHONHASHSEED"]

from ruamel.yaml import YAML
from charter.activation.context_renderers.artifact_bodies import _format_full_artifact_payload_body
from charter.offering.directives.models import Directive
from charter.offering.tactics.models import Tactic
from charter.offering.styleguides.models import Styleguide
from charter.offering.toolguides.models import Toolguide
from charter.offering.procedures.models import Procedure
from charter.offering.paradigms.models import Paradigm
from charter.offering.glossary_packs.models import GlossaryPack
from charter.offering.pack_skills.models import PackSkill
from charter.offering.missions.step_contracts import MissionStepContract

MODELS = {
    "directive": Directive,
    "tactic": Tactic,
    "styleguide": Styleguide,
    "toolguide": Toolguide,
    "procedure": Procedure,
    "paradigm": Paradigm,
    "glossary_pack": GlossaryPack,
    "skill": PackSkill,
    "mission_step_contract": MissionStepContract,
    "glossary_pack_edges": GlossaryPack,
    "styleguide_edges": Styleguide,
}

yaml = YAML(typ="safe")
docs = {}
for kind, path in json.loads(os.environ["SK_CF38_DOCS"]).items():
    data = yaml.load(pathlib.Path(path).read_text(encoding="utf-8"))
    docs[kind] = _format_full_artifact_payload_body(MODELS[kind].model_validate(data))
print("CF38_RECEIPT " + json.dumps({
    "runner": "source-python-" + ".".join(str(part) for part in sys.version_info[:3]),
    "cwd": str(pathlib.Path.cwd()),
    "docs": docs,
}))
"#;

/// Write the two fresh fixtures under the isolated root.
fn write_fixtures(root: &Path) -> BTreeMap<String, String> {
    let fixtures = root.join("cf38-fixtures");
    std::fs::create_dir_all(&fixtures).expect("fixture directory");
    std::fs::write(
        fixtures.join("glossary-edges.glossary-pack.yaml"),
        GLOSSARY_EDGES,
    )
    .expect("glossary fixture");
    std::fs::write(
        fixtures.join("styleguide-edges.styleguide.yaml"),
        STYLEGUIDE_EDGES,
    )
    .expect("styleguide fixture");
    let repository = isolated::repository_root();
    let mut docs: BTreeMap<String, String> = REPOSITORY_DOCUMENTS
        .map(|(kind, relative)| {
            (
                kind.to_owned(),
                repository.join(relative).display().to_string(),
            )
        })
        .into_iter()
        .collect();
    docs.insert(
        "glossary_pack_edges".to_owned(),
        fixtures
            .join("glossary-edges.glossary-pack.yaml")
            .display()
            .to_string(),
    );
    docs.insert(
        "styleguide_edges".to_owned(),
        fixtures
            .join("styleguide-edges.styleguide.yaml")
            .display()
            .to_string(),
    );
    docs
}

/// The artifact kind of one receipt key (the two edge fixtures are aliases).
fn canonical_kind(key: &str) -> ArtifactKind {
    match key {
        "glossary_pack_edges" => ArtifactKind::GlossaryPack,
        "styleguide_edges" => ArtifactKind::Styleguide,
        other => ArtifactKind::from_canonical(other).expect("canonical kind"),
    }
}

fn parse_receipt(stdout: &str) -> (String, String, BTreeMap<String, Vec<String>>) {
    let marker = "CF38_RECEIPT ";
    let index = stdout
        .find(marker)
        .unwrap_or_else(|| panic!("no receipt in {stdout:?}"));
    let line = stdout[index + marker.len()..]
        .lines()
        .next()
        .expect("receipt line");
    let value: JsonValue = serde_json::from_str(line.trim()).expect("receipt JSON");
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

/// The Native child: assert the isolated roots, dump the same documents.
#[test]
fn cf38_native_child_and_receipt() {
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
    assert_eq!(
        std::env::var("PYTHONHASHSEED").as_deref(),
        Ok("0"),
        "the set-order authority is pinned in the actual Native child"
    );
    let docs: JsonValue =
        serde_json::from_str(&std::env::var("SK_CF38_DOCS").expect("docs env")).expect("docs JSON");
    let mut rendered = serde_json::Map::new();
    for (kind, path) in docs.as_object().expect("docs object") {
        let path = path.as_str().expect("path");
        let canonical = canonical_kind(kind);
        // The canonical reader (transport + construct + graph), never a private
        // parse: the exact payload is only defined over the constructed graph.
        let payload =
            canonical_artifact_body_exact_path(canonical, Path::new(path)).expect("exact payload");
        rendered.insert(
            kind.clone(),
            JsonValue::Array(
                include_full_payload_lines_exact(&payload)
                    .into_iter()
                    .map(JsonValue::String)
                    .collect(),
            ),
        );
    }
    println!(
        "CF38_RECEIPT {}",
        json!({
            "runner": "native",
            "cwd": std::env::current_dir().expect("cwd").display().to_string(),
            "docs": rendered,
        })
    );
}

fn run_native_child(root: &Path, docs_env: &str) -> String {
    let repository = isolated::repository_root();
    let executable = std::env::current_exe().expect("test binary");
    let mut command = std::process::Command::new(executable);
    command
        .args([
            "--exact",
            "cf38_native_child_and_receipt",
            "--nocapture",
            "--test-threads=1",
        ])
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .current_dir(root)
        .env("SK_E4_NATIVE_ROOT", root)
        .env("SK_CF38_DOCS", docs_env)
        .env("PYTHONHASHSEED", "0")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONPATH", repository.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repository.join("packs"));
    for (key, name) in isolated::ROOTS.into_iter().chain(EXTRA_XDG_ROOTS) {
        command.env(key, root.join(name));
    }
    let output = command.output().expect("run the Native child");
    std::fs::write(root.join("cf38-native.stdout"), &output.stdout).expect("stdout receipt");
    std::fs::write(root.join("cf38-native.stderr"), &output.stderr).expect("stderr receipt");
    assert!(
        output.status.success(),
        "Native child failed (status {:?}): {} {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("native stdout is UTF-8")
}

#[test]
fn source_and_native_exact_payloads_agree_for_the_nine_kinds_and_the_edges() {
    let root = isolated::fixture("cf38-exact-artifact-payload");
    for (_, name) in EXTRA_XDG_ROOTS {
        std::fs::create_dir_all(root.join(name)).expect("extra isolated XDG directory");
    }
    let docs = write_fixtures(&root);
    let docs_env = serde_json::to_string(&docs).expect("docs JSON");
    let config_dirs = root.join("config-dirs");
    let data_dirs = root.join("data-dirs");

    let source_stdout = isolated::source_script(
        &root,
        SOURCE_SCRIPT,
        &[
            ("SK_CF38_DOCS", docs_env.as_str()),
            ("PYTHONHASHSEED", "0"),
            (
                "XDG_CONFIG_DIRS",
                config_dirs.to_str().expect("fixture path"),
            ),
            ("XDG_DATA_DIRS", data_dirs.to_str().expect("fixture path")),
        ],
    );
    let (source_runner, source_cwd, source_docs) = parse_receipt(&source_stdout);
    assert_eq!(source_runner, "source-python-3.11.15");
    assert_eq!(source_cwd, root.display().to_string());

    let native_stdout = run_native_child(&root, &docs_env);
    let (native_runner, native_cwd, native_docs) = parse_receipt(&native_stdout);
    assert_eq!(native_runner, "native");
    assert_eq!(native_cwd, root.display().to_string());

    assert_eq!(
        source_docs.keys().collect::<Vec<&String>>(),
        native_docs.keys().collect::<Vec<&String>>(),
        "both children rendered the same documents"
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

    // Targeted assertions so an equal-but-empty payload could not pass.
    let glossary = source_docs.get("glossary_pack_edges").expect("glossary");
    for expected in [
        "            \"confidence\": NaN,",
        "            \"confidence\": Infinity,",
        "            \"confidence\": -Infinity,",
        "            \"confidence\": -0.0,",
        "            \"confidence\": 1.2345678901234568e+300,",
        "            \"definition\": \"h\\u00e9llo \\u2603\",",
    ] {
        assert!(
            glossary.iter().any(|line| line == expected),
            "missing {expected:?} in\n{}",
            glossary.join("\n")
        );
    }
    let styleguide = source_docs.get("styleguide_edges").expect("styleguide");
    for expected in [
        "          \"bytes_value\": \"Hello\",",
        "          \"datetime\": \"2020-01-01T01:02:03Z\",",
        "          \"pairs_value\": [",
        "          \"omap_value\": {",
        "              null,",
        "          \"map_nulls\": {",
    ] {
        assert!(
            styleguide.iter().any(|line| line == expected),
            "missing {expected:?} in\n{}",
            styleguide.join("\n")
        );
    }
}

#[test]
fn finite_scalar_adapter_is_the_projection_of_the_exact_payload() {
    let root = isolated::fixture("cf38-exact-artifact-payload-finite");
    let docs = write_fixtures(&root);
    let mut compared = 0;
    for (kind, path) in &docs {
        let text = std::fs::read_to_string(path).expect("document read");
        let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(&text).expect("document parse");
        let canonical = canonical_kind(kind);
        let Some(payload) = canonical_artifact_body_exact_value(canonical, &value) else {
            continue;
        };
        let exact_lines = include_full_payload_lines_exact(&payload);
        match canonical_artifact_body(canonical, &value) {
            Some(adapter) => {
                assert_eq!(
                    include_full_payload_lines(&adapter),
                    exact_lines,
                    "{kind}: the finite adapter renders the exact payload"
                );
                compared += 1;
            }
            None => {
                // The adapter refuses a payload serde_json cannot carry; it must
                // not print a shortened one.
                assert!(
                    payload.to_serde().is_none(),
                    "{kind}: refusal only when the exact payload is not scalar"
                );
            }
        }
    }
    assert!(compared >= 9, "compared {compared} finite documents");
}
