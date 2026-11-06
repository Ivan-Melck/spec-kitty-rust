//! CF50 — exact merged action/bootstrap delivery: paired Source/Native proof.
//!
//! CF43 made the `--include` fetch exact over the merged record and CF48 made the
//! org raw readers canonical, but the action/bootstrap payload still reopened the
//! **declaring** file with a second YAML parse and the finite
//! `canonical_artifact_body`, so an inherited/overridden record was delivered as
//! whatever its declaring file happened to say. This file proves the consumer
//! wiring: the action payload (`build_action_context` / `to_json_exact`) takes every
//! artifact `body`, `title` and `summary` from the **merged** record's canonical
//! model dump, and the bootstrap text carries the reference's
//! `<id>: <title> — <summary>` line with no inline body on any surface.
//!
//! Both sides are actual processes. The Source child is the pinned CPython 3.11.15
//! oracle and the Native child is this test binary re-executed; each asserts its own
//! CWD, HOME, all seven XDG roots (including `XDG_CONFIG_DIRS`/`XDG_DATA_DIRS`),
//! TMPDIR/TMP/TEMP, the absolute `PYTHONPATH`, the pinned packs root and
//! `PYTHONHASHSEED=0` before answering. Strings travel as code-point arrays so a lone
//! surrogate survives the transport exactly.
//!
//! Not CLI acceptance (the `--json` envelope is proved in
//! `crates/sk-cli/tests/cf50_action_context_cli.rs`) and not a parity claim beyond
//! the exercised fixture.

mod support;

#[path = "support/isolated.rs"]
mod isolated;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value as JsonValue, json};
use sk_doctrine::{
    ContextInputs, ContextMode, ContextRequest, IdentityIndex, ProfileRepository,
    build_action_context, context_roots, load_activation_ledger, load_graph_or_dir, load_org_pack,
    merge_three_layers, required_floor,
};
use sk_kernel::PythonString;
use sk_kernel::python_json::PythonJsonValue;

const EXTRA_XDG_ROOTS: [(&str, &str); 2] = [
    ("XDG_CONFIG_DIRS", "config-dirs"),
    ("XDG_DATA_DIRS", "data-dirs"),
];

/// The overlay: it replaces `title` (non-ASCII code points), `intent` (a lone
/// surrogate) and `procedures`, and inherits `scope`, `integrity_rules`,
/// `validation_criteria`, `explicit_allowances` and `references` from the real
/// built-in `DIRECTIVE_001`.
const DIRECTIVE_OVERLAY: &str = "\
schema_version: \"1.0\"
id: DIRECTIVE_001
title: \"Overlay integrity \\u00e9\\u2603 title\"
intent: \"Overlay intent with a lone surrogate \\ud800 and a smile \\u263a.\\n\"
procedures:
  - overlay procedure one
  - overlay procedure two
";

/// The body fields the two children compare, exactly.
const BODY_FIELDS: [&str; 8] = [
    "title",
    "intent",
    "enforcement",
    "scope",
    "procedures",
    "integrity_rules",
    "validation_criteria",
    "explicit_allowances",
];

/// The inherited `scope` of the real built-in `DIRECTIVE_001`.
const INHERITED_SCOPE: &str = "Applies to all design proposals, implementation plans, and architectural decision records that introduce or modify component boundaries, module interfaces, or cross-cutting concerns.\n";

/// The reference receipt: bootstrap text + the merged `DIRECTIVE_001` entry.
const SOURCE_SCRIPT: &str = r#"
import json, os, pathlib, sys
root = pathlib.Path(os.environ["SK_CF50_ROOT"])
assert pathlib.Path.cwd() == root, pathlib.Path.cwd()
assert sys.version_info[:3] == (3, 11, 15), sys.version_info
repo = pathlib.Path(os.environ["PYTHONPATH"]).parent
assert pathlib.Path(os.environ["PYTHONPATH"]) == repo / "src"
assert pathlib.Path(os.environ["SPEC_KITTY_PACKS_ROOT"]) == repo / "packs"
for key, name in [
    ("HOME", "home"), ("XDG_CONFIG_HOME", "config"), ("XDG_CACHE_HOME", "cache"),
    ("XDG_DATA_HOME", "data"), ("XDG_STATE_HOME", "state"), ("XDG_RUNTIME_DIR", "runtime"),
    ("TMPDIR", "tmp"), ("TMP", "tmp"), ("TEMP", "tmp"),
]:
    assert pathlib.Path(os.environ[key]) == root / name, (key, os.environ[key])
assert pathlib.Path(os.environ["XDG_CONFIG_DIRS"]) == root / "config-dirs"
assert pathlib.Path(os.environ["XDG_DATA_DIRS"]) == root / "data-dirs"
assert os.environ["PYTHONHASHSEED"] == "0", os.environ["PYTHONHASHSEED"]

from charter.activation.context import build_charter_context, build_charter_context_json

def points(text):
    return [ord(char) for char in text]

def encode(value):
    """Exact transport: strings become code-point arrays, lists recurse."""
    if isinstance(value, str):
        return {"text": points(value)}
    if isinstance(value, list):
        return {"list": [encode(item) for item in value]}
    if value is None:
        return {"null": True}
    return {"text": points(str(value))}

result = build_charter_context(root, action="implement", depth=2, mission_type="software-dev")
payload = build_charter_context_json(root, action="implement", depth=2, mission_type="software-dev")
entry = next(item for item in payload["directives"] if item["id"] == "DIRECTIVE_001")
body = entry["body"]
receipt = {
    "runner": "source-python-" + ".".join(str(part) for part in sys.version_info[:3]),
    "cwd": str(pathlib.Path.cwd()),
    "mode": result.mode,
    "text": points(result.text),
    "entry": {
        "id": entry["id"],
        "source": entry["source"],
        "delivery": entry["delivery"],
        "title": points(entry["title"]),
        "summary": points(entry["summary"]),
        "body_keys": sorted(body.keys()),
        "body": {
            field: encode(body.get(field))
            for field in json.loads(os.environ["SK_CF50_BODY_FIELDS"])
        },
    },
}
print("CF50_RECEIPT " + json.dumps(receipt))
"#;

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("fixture parent");
    }
    std::fs::write(path, contents).expect("fixture write");
}

/// A git-backed project fixture whose project layer shadows the real built-in
/// `DIRECTIVE_001`.
fn fixture() -> PathBuf {
    let root = isolated::fixture("cf50-exact-action-delivery");
    for (_, name) in EXTRA_XDG_ROOTS {
        std::fs::create_dir_all(root.join(name)).expect("extra isolated XDG directory");
    }
    write(
        &root.join(".kittify/config.yaml"),
        "# authored project config\ncustom_setting: keep-me\n",
    );
    write(
        &root.join(".kittify/charter/charter.yaml"),
        "schema_version: \"1.1.0\"\ncatalog:\n  references: []\ngovernance:\n  authority_paths: [docs/context]\n",
    );
    write(
        &root.join(".kittify/charter/charter.md"),
        "# Fixture Charter\n\n## Policy Summary\n\n- one rule\n",
    );
    write(
        &root.join(
            ".kittify/charter-packs/directive/001-architectural-integrity-standard.directive.yaml",
        ),
        DIRECTIVE_OVERLAY,
    );
    let output = std::process::Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(&root)
        .output()
        .expect("run git init");
    assert!(
        output.status.success(),
        "git init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    root
}

/// One child's exact answer for the delivered directive.
#[derive(Debug, PartialEq, Eq)]
struct Receipt {
    runner: String,
    cwd: String,
    mode: String,
    text: Vec<u32>,
    entry: Entry,
}

#[derive(Debug, PartialEq, Eq)]
struct Entry {
    id: String,
    source: String,
    delivery: String,
    title: Vec<u32>,
    summary: Vec<u32>,
    body_keys: Vec<String>,
    body: BTreeMap<String, Encoded>,
}

/// An exactly transported body value.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Encoded {
    Text(Vec<u32>),
    List(Vec<Encoded>),
    Null,
}

fn code_points(value: &JsonValue) -> Vec<u32> {
    value
        .as_array()
        .expect("code-point array")
        .iter()
        .map(|point| u32::try_from(point.as_u64().expect("code point")).expect("u32"))
        .collect()
}

fn decode_encoded(value: &JsonValue) -> Encoded {
    if let Some(text) = value.get("text") {
        return Encoded::Text(code_points(text));
    }
    if let Some(items) = value.get("list") {
        return Encoded::List(
            items
                .as_array()
                .expect("list array")
                .iter()
                .map(decode_encoded)
                .collect(),
        );
    }
    Encoded::Null
}

fn parse_receipt(stdout: &str) -> Receipt {
    let marker = "CF50_RECEIPT ";
    let index = stdout
        .find(marker)
        .unwrap_or_else(|| panic!("no receipt in {stdout:?}"));
    let line = stdout[index + marker.len()..]
        .lines()
        .next()
        .expect("receipt line");
    let value: JsonValue = serde_json::from_str(line.trim()).expect("receipt JSON");
    let entry = &value["entry"];
    let body = entry["body"].as_object().expect("body object");
    Receipt {
        runner: value["runner"].as_str().expect("runner").to_owned(),
        cwd: value["cwd"].as_str().expect("cwd").to_owned(),
        mode: value["mode"].as_str().expect("mode").to_owned(),
        text: code_points(&value["text"]),
        entry: Entry {
            id: entry["id"].as_str().expect("id").to_owned(),
            source: entry["source"].as_str().expect("source").to_owned(),
            delivery: entry["delivery"].as_str().expect("delivery").to_owned(),
            title: code_points(&entry["title"]),
            summary: code_points(&entry["summary"]),
            body_keys: entry["body_keys"]
                .as_array()
                .expect("body keys")
                .iter()
                .map(|key| key.as_str().expect("key").to_owned())
                .collect(),
            body: body
                .iter()
                .map(|(field, value)| (field.clone(), decode_encoded(value)))
                .collect(),
        },
    }
}

/// One exact string field, projected to its scalar view (test reporting only).
fn text_field<'a>(body: &'a PythonJsonValue, field: &str) -> Option<&'a PythonString> {
    match body.field(field)? {
        PythonJsonValue::String(text) => Some(text),
        _ => None,
    }
}

/// One exact field as the receipt's transport shape.
fn encode_exact(body: &PythonJsonValue, field: &str) -> JsonValue {
    match body.field(field) {
        Some(PythonJsonValue::String(text)) => json!({ "text": text.codepoints() }),
        Some(PythonJsonValue::Array(items)) => json!({
            "list": items
                .iter()
                .map(|item| match item {
                    PythonJsonValue::String(text) => json!({ "text": text.codepoints() }),
                    // `_jsonable_artifact_value` maps a non-finite float inside `Any`
                    // to `null`; the reference's own body projection has no other
                    // non-string member in this fixture's fields.
                    PythonJsonValue::Null => json!({ "null": true }),
                    other => json!({ "text": other.render_python(None, true).codepoints() }),
                })
                .collect::<Vec<JsonValue>>(),
        }),
        Some(other) => json!({ "text": other.render_python(None, true).codepoints() }),
        None => json!({ "null": true }),
    }
}

/// The Native child's receipt line, computed from the canonical public API.
fn native_receipt_json(root: &Path) -> String {
    let (built_in_root, org_roots, project_root) = context_roots(root).expect("context roots");
    let built_in = load_graph_or_dir(&built_in_root).expect("built-in graph");
    let mut fragments = Vec::new();
    for (index, org) in org_roots.iter().enumerate() {
        let name = org
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("org")
            .to_owned();
        fragments.push(load_org_pack(&name, org, index + 1).expect("org pack"));
    }
    let project_graph = project_root
        .as_deref()
        .filter(|layer| sk_doctrine::has_graph_files(layer))
        .map(|layer| load_graph_or_dir(layer).expect("project graph"));
    let report =
        merge_three_layers(&built_in, &fragments, project_graph.as_ref()).expect("merge layers");
    let identity = IdentityIndex::from_roots(&built_in_root, &org_roots, project_root.as_deref());
    let profiles = ProfileRepository::load(&built_in_root, &org_roots, project_root.as_deref())
        .expect("profiles");
    let ledger = load_activation_ledger(root).expect("ledger");
    let floor = required_floor(root, &identity)
        .expect("required floor")
        .into_map();
    let inputs = ContextInputs {
        graph: &report.graph,
        ledger: &ledger,
        identity: &identity,
        profiles: &profiles,
        project_root: root,
        required_floor: &floor,
        project_warnings: &[],
    };
    let mut request = ContextRequest::new("implement");
    request.mission_type = Some("software-dev");
    request.depth = 2;
    let context = build_action_context(&inputs, &request).expect("action context");

    let markdown = std::fs::read_to_string(root.join(".kittify/charter/charter.md")).ok();
    let render_inputs = sk_doctrine::context_render::RenderInputs {
        charter_markdown: markdown.as_deref(),
        project_root: Some(root),
        ..sk_doctrine::context_render::RenderInputs::default()
    };
    let text =
        sk_doctrine::context_render::render_with_exact(&context, &render_inputs).expect("render");

    let payload = context.to_json_exact();
    let entries = match payload.field("directives") {
        Some(PythonJsonValue::Array(items)) => items.clone(),
        _ => Vec::new(),
    };
    let entry = entries
        .iter()
        .find(|item| {
            text_field(item, "id")
                .and_then(|id| id.to_scalar_string().ok())
                .as_deref()
                == Some("DIRECTIVE_001")
        })
        .expect("the directive was delivered");
    let body = entry.field("body").expect("an inline body");
    let mut body_keys: Vec<String> = match body {
        PythonJsonValue::ExactObject(object) => object
            .entries()
            .iter()
            .filter_map(|(key, _value)| key.to_scalar_string().ok())
            .collect(),
        PythonJsonValue::Object(object) => object.iter().map(|(key, _value)| key.clone()).collect(),
        _ => Vec::new(),
    };
    body_keys.sort();
    let mut mapped = Map::new();
    for field in BODY_FIELDS {
        mapped.insert(field.to_owned(), encode_exact(body, field));
    }
    let scalar = |field: &str| -> JsonValue {
        match text_field(entry, field).and_then(|text| text.to_scalar_string().ok()) {
            Some(text) => json!(text),
            None => JsonValue::Null,
        }
    };
    let points = |field: &str| -> JsonValue {
        match text_field(entry, field) {
            Some(text) => json!(text.codepoints()),
            None => json!([]),
        }
    };
    json!({
        "runner": "native",
        "cwd": std::env::current_dir().expect("cwd").display().to_string(),
        "mode": match context.mode {
            ContextMode::Bootstrap => "bootstrap",
            ContextMode::Compact => "compact",
            // Mechanical ripple: the combined single build has no text rail.
            ContextMode::Missing => "missing",
        },
        "text": text.codepoints(),
        "entry": {
            "id": scalar("id"),
            "source": scalar("source"),
            "delivery": scalar("delivery"),
            "title": points("title"),
            "summary": points("summary"),
            "body_keys": body_keys,
            "body": JsonValue::Object(mapped),
        },
    })
    .to_string()
}

/// The Native child: assert isolation, then answer from the canonical API.
#[test]
fn cf50_native_child_and_receipt() {
    let Some(root) = isolated::native_root() else {
        return;
    };
    isolated::assert_isolated(&root);
    for (key, name) in EXTRA_XDG_ROOTS {
        assert_eq!(
            std::env::var_os(key).map(PathBuf::from),
            Some(root.join(name)),
            "{key} is asserted inside the actual Native child"
        );
    }
    assert_eq!(
        std::env::var("PYTHONHASHSEED").as_deref(),
        Ok("0"),
        "the attribution is pinned in the actual Native child"
    );
    println!("CF50_RECEIPT {}", native_receipt_json(&root));
}

/// The paired proof: the merged record's inherited **and** overridden fields reach
/// the action payload exactly, the bootstrap text carries no inline body, and the
/// Native child reproduces the Source child's answer.
#[test]
fn merged_action_payload_matches_the_source_child() {
    let root = fixture();
    let config_dirs = root.join("config-dirs");
    let data_dirs = root.join("data-dirs");
    let body_fields = serde_json::to_string(&BODY_FIELDS).expect("body fields JSON");

    let source_stdout = isolated::source_script(
        &root,
        SOURCE_SCRIPT,
        &[
            ("SK_CF50_ROOT", root.to_str().expect("root")),
            ("SK_CF50_BODY_FIELDS", body_fields.as_str()),
            ("PYTHONHASHSEED", "0"),
            ("XDG_CONFIG_DIRS", config_dirs.to_str().expect("dirs")),
            ("XDG_DATA_DIRS", data_dirs.to_str().expect("dirs")),
        ],
    );
    let source = parse_receipt(&source_stdout);
    assert_eq!(
        source.runner, "source-python-3.11.15",
        "the Source child is the pinned interpreter"
    );
    assert_eq!(source.cwd, root.display().to_string());

    // The intended Source cause is asserted BEFORE the Native child runs.
    assert_eq!(source.mode, "bootstrap");
    assert_eq!(source.entry.id, "DIRECTIVE_001");
    assert_eq!(
        source.entry.source, "project",
        "the overlay layer is the record's provenance"
    );
    assert_eq!(
        source.entry.delivery, "inline",
        "the requires closure is delivered inline"
    );
    let overlay_title: Vec<u32> = "Overlay integrity é☃ title"
        .chars()
        .map(u32::from)
        .collect();
    assert_eq!(source.entry.title, overlay_title);
    assert_eq!(
        source.entry.body.get("title"),
        Some(&Encoded::Text(overlay_title)),
        "the body title is the overridden one"
    );
    let surrogate = 0xD800_u32;
    assert!(
        matches!(source.entry.body.get("intent"), Some(Encoded::Text(points)) if points.contains(&surrogate)),
        "the reference keeps the authored lone surrogate in the body intent"
    );
    assert_eq!(
        source.entry.body.get("scope"),
        Some(&Encoded::Text(
            INHERITED_SCOPE.chars().map(u32::from).collect::<Vec<u32>>()
        )),
        "an overridden-away field is inherited from the built-in record"
    );
    assert_eq!(
        source.entry.body.get("procedures"),
        Some(&Encoded::List(vec![
            Encoded::Text("overlay procedure one".chars().map(u32::from).collect()),
            Encoded::Text("overlay procedure two".chars().map(u32::from).collect()),
        ])),
        "the overridden list is the overlay's"
    );
    // The bootstrap TEXT carries the header line only — never the body.
    let source_text: String = source
        .text
        .iter()
        .map(|point| char::from_u32(*point).unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect();
    assert!(
        source_text.contains("- DIRECTIVE_001: Overlay integrity é☃ title — Overlay intent"),
        "the reference bootstrap line is `<id>: <title> — <summary>`"
    );
    assert!(
        !source_text.contains("Intent: "),
        "the reference bootstrap text never inlines an artifact body"
    );

    // Now the Native child must reproduce that whole answer.
    let native_stdout = isolated::native_reexec(
        &root,
        "cf50_native_child_and_receipt",
        &[
            ("PYTHONHASHSEED", "0"),
            ("XDG_CONFIG_DIRS", config_dirs.to_str().expect("dirs")),
            ("XDG_DATA_DIRS", data_dirs.to_str().expect("dirs")),
        ],
    );
    let native = parse_receipt(&native_stdout);
    assert_eq!(native.runner, "native");
    assert_eq!(native.cwd, root.display().to_string());
    assert_eq!(native.mode, source.mode, "the delivery mode");
    // The action-delivery LINE is the CF50 claim, not the rest of the bootstrap
    // composition (profile/selection/activation/guidance blocks and the token
    // budget are separate, still-partial Native surfaces — see the report).
    let contains = |needle: &str| {
        let needle: Vec<u32> = needle.chars().map(u32::from).collect();
        native.text.windows(needle.len()).any(|part| part == needle)
    };
    assert!(contains("- DIRECTIVE_001: Overlay integrity é☃ title"));
    assert!(
        contains("Overlay intent with a lone surrogate"),
        "an exact summary must not disappear"
    );
    assert!(native.text.contains(&0xd800));
    let prefix: Vec<u32> = "    - DIRECTIVE_001:".chars().map(u32::from).collect();
    let row = |text: &[u32]| {
        text.split(|code| *code == 10)
            .find(|line| line.starts_with(&prefix))
            .expect("delivered row")
            .to_vec()
    };
    assert_eq!(
        row(&native.text),
        row(&source.text),
        "the entire delivered row, including surrogate units"
    );
    assert!(
        !contains("Intent: "),
        "the bootstrap text never inlines an artifact body"
    );
    assert_eq!(
        native.entry.source, source.entry.source,
        "record provenance"
    );
    assert_eq!(native.entry.delivery, source.entry.delivery, "cadence");
    assert_eq!(native.entry.title, source.entry.title, "entry title");
    assert_eq!(native.entry.summary, source.entry.summary, "entry summary");
    assert_eq!(native.entry.body_keys, source.entry.body_keys, "body keys");
    for field in BODY_FIELDS {
        assert_eq!(
            native.entry.body.get(field),
            source.entry.body.get(field),
            "body field {field}"
        );
    }
}
