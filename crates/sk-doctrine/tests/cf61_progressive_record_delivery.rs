//! CF61 — lazy progressive record delivery: paired Source/Native proof.
//!
//! The reference's action payload reads a delivered artifact's `title`/`name`
//! and `intent`/`purpose` from its **model properties**, and only evaluates the
//! JSON-mode model dump for an inline `body` (`progressive_disclosure.
//! _decorate_entry` calls `body_of` only when the entry is inline). A
//! `suggests`-delivered styleguide whose unrequested `structural_lint_config`
//! cannot be JSON-serialized therefore still delivers valid metadata as a link,
//! while the same record under `include_all` raises the reference's real
//! payload error.
//!
//! This file proves that split on actual processes: the Source child is the
//! pinned CPython 3.11.15 oracle and the Native child is this test binary
//! re-executed; each asserts its own CWD, HOME, all seven XDG roots, TMP/TMPDIR/
//! TEMP, absolute `PYTHONPATH`, pinned packs root and `PYTHONHASHSEED=0` before
//! answering. Strings travel as code-point arrays.
//!
//! The fixture also carries a true-missing graph node (no layered record) to
//! prove genuine absence stays a healthy entry, and a project overlay on an
//! inline directive to prove merged/inherited body delivery still works.
//!
//! Not CLI acceptance, not whole-action parity, not approval.

mod support;

#[path = "support/isolated.rs"]
mod isolated;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value as JsonValue, json};
use sk_doctrine::{
    ContextInputs, ContextMode, ContextRequest, DoctrineError, IdentityIndex, PayloadFailure,
    ProfileRepository, ProgressiveDeliveryError, build_action_context, context_roots,
    load_activation_ledger, load_graph_or_dir, load_org_pack, merge_three_layers, required_floor,
};
use sk_kernel::PythonString;
use sk_kernel::python_json::PythonJsonValue;

const EXTRA_XDG_ROOTS: [(&str, &str); 2] = [
    ("XDG_CONFIG_DIRS", "config-dirs"),
    ("XDG_DATA_DIRS", "data-dirs"),
];

/// The invalid unstructured member: pydantic's `model_dump(mode="json")`
/// raises `UnicodeDecodeError` for `bytes` that is not UTF-8.
const LAZY_STYLEGUIDE: &str = "\
schema_version: \"1.0\"
id: common-docs
title: \"CF61 lazy docs \\u00e9 title\"
scope: docs
principles:
  - overlay principle
structural_lint_config:
  bytes_value: !!binary /w==
";

/// A normal inline overlay: replaced `title`/`intent`/`procedures`, inherited
/// `scope` from the real built-in `DIRECTIVE_001`.
const MERGED_DIRECTIVE: &str = "\
schema_version: \"1.0\"
id: DIRECTIVE_001
title: \"CF61 merged \\u00e9 title\"
intent: \"CF61 merged intent\"
procedures:
  - cf61 procedure one
";

/// A project graph node with no layered record (a true missing id), reached by
/// `requires` so it is delivered inline like the reference's eager closure.
const PROJECT_GRAPH: &str = "\
schema_version: '1.0'
generated_at: STATIC
generated_by: cf61-test
nodes:
- urn: tactic:cf61-ghost
  kind: tactic
  label: CF61 Ghost
edges:
- source: action:software-dev/implement
  target: tactic:cf61-ghost
  relation: requires
";

const SOURCE_SCRIPT: &str = r#"
import json, os, pathlib, sys
root = pathlib.Path(os.environ["SK_CF61_ROOT"])
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

from charter.activation.context import build_charter_context_json


def points(text):
    return [ord(char) for char in text]


def entry(payload, kind, item_id):
    return next(item for item in payload[kind] if item["id"] == item_id)


def view(item, body=None):
    return {
        "id": item["id"],
        "source": item["source"],
        "delivery": item["delivery"],
        "title": points(item["title"]) if "title" in item else None,
        "summary": points(item["summary"]) if "summary" in item else None,
        "body_present": "body" in item,
        "body_title": points(item["body"]["title"]) if body and "title" in item.get("body", {}) else None,
        "body_scope": points(item["body"]["scope"]) if body and "scope" in item.get("body", {}) else None,
    }


payload = build_charter_context_json(
    root, action="implement", depth=2, mission_type="software-dev", include_all=False
)
receipt = {
    "runner": "source-python-" + ".".join(str(part) for part in sys.version_info[:3]),
    "cwd": str(pathlib.Path.cwd()),
    "mode": payload["mode"],
    "link": view(entry(payload, "styleguides", "common-docs")),
    "missing": view(entry(payload, "tactics", "cf61-ghost")),
    "merged": view(entry(payload, "directives", "DIRECTIVE_001"), body=True),
}
try:
    build_charter_context_json(
        root, action="implement", depth=2, mission_type="software-dev", include_all=True
    )
except Exception as exc:  # noqa: BLE001 - the reference's own refusal is the result
    receipt["inline_error"] = {"type": type(exc).__name__, "message": str(exc)}
else:
    receipt["inline_error"] = None
print("CF61_RECEIPT " + json.dumps(receipt))
"#;

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("fixture parent");
    }
    std::fs::write(path, contents).expect("fixture write");
}

/// A git-backed project fixture with the lazy styleguide, the merged directive
/// overlay and the true-missing graph node.
fn fixture() -> PathBuf {
    let root = isolated::fixture("cf61-progressive-record");
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
        &root.join(".kittify/charter-packs/styleguide/common-docs.styleguide.yaml"),
        LAZY_STYLEGUIDE,
    );
    write(
        &root.join(
            ".kittify/charter-packs/directive/001-architectural-integrity-standard.directive.yaml",
        ),
        MERGED_DIRECTIVE,
    );
    write(
        &root.join(".kittify/charter-packs/cf61.graph.yaml"),
        PROJECT_GRAPH,
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

#[derive(Debug, PartialEq, Eq)]
struct EntryView {
    id: String,
    source: String,
    delivery: String,
    title: Option<Vec<u32>>,
    summary: Option<Vec<u32>>,
    body_present: bool,
    body_title: Option<Vec<u32>>,
    body_scope: Option<Vec<u32>>,
}

#[derive(Debug, PartialEq, Eq)]
struct Receipt {
    runner: String,
    cwd: String,
    mode: String,
    link: EntryView,
    missing: EntryView,
    merged: EntryView,
    inline_error: Option<(String, String)>,
}

fn code_points(value: &JsonValue) -> Vec<u32> {
    value
        .as_array()
        .expect("code-point array")
        .iter()
        .map(|point| u32::try_from(point.as_u64().expect("code point")).expect("u32"))
        .collect()
}

fn optional_points(value: Option<&JsonValue>) -> Option<Vec<u32>> {
    value.filter(|value| !value.is_null()).map(code_points)
}

fn view(value: &JsonValue) -> EntryView {
    EntryView {
        id: value["id"].as_str().expect("id").to_owned(),
        source: value["source"].as_str().expect("source").to_owned(),
        delivery: value["delivery"].as_str().expect("delivery").to_owned(),
        title: optional_points(value.get("title")),
        summary: optional_points(value.get("summary")),
        body_present: value["body_present"].as_bool().expect("body_present"),
        body_title: optional_points(value.get("body_title")),
        body_scope: optional_points(value.get("body_scope")),
    }
}

fn parse_receipt(stdout: &str) -> Receipt {
    let marker = "CF61_RECEIPT ";
    let line = stdout
        .lines()
        .find(|line| line.starts_with(marker))
        .unwrap_or_else(|| panic!("no receipt in {stdout:?}"));
    let value: JsonValue = serde_json::from_str(line[marker.len()..].trim()).expect("receipt JSON");
    let inline_error = value["inline_error"].as_object().map(|error| {
        (
            error["type"].as_str().expect("error type").to_owned(),
            error["message"].as_str().expect("error message").to_owned(),
        )
    });
    Receipt {
        runner: value["runner"].as_str().expect("runner").to_owned(),
        cwd: value["cwd"].as_str().expect("cwd").to_owned(),
        mode: value["mode"].as_str().expect("mode").to_owned(),
        link: view(&value["link"]),
        missing: view(&value["missing"]),
        merged: view(&value["merged"]),
        inline_error,
    }
}

fn native_context(
    root: &Path,
    include_all: bool,
) -> Result<sk_doctrine::CharterContext, DoctrineError> {
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
    request.include_all = include_all;
    build_action_context(&inputs, &request)
}

fn native_view(context: &sk_doctrine::CharterContext, kind: &str, id: &str) -> EntryView {
    let entries = match kind {
        "styleguides" => &context.styleguides,
        "tactics" => &context.tactics,
        "directives" => &context.directives,
        other => panic!("unexpected kind {other}"),
    };
    let entry = entries
        .iter()
        .find(|entry| entry.id == id)
        .unwrap_or_else(|| panic!("no {kind} entry {id}"));
    let points = |text: Option<&PythonString>| text.map(|text| text.codepoints().to_vec());
    let body_text = |field: &str| match entry.body.as_ref().and_then(|body| body.field(field)) {
        Some(PythonJsonValue::String(text)) => Some(text.codepoints().to_vec()),
        _ => None,
    };
    EntryView {
        id: entry.id.clone(),
        source: entry.source.clone(),
        delivery: match entry.delivery {
            sk_doctrine::Delivery::Inline => "inline".to_owned(),
            sk_doctrine::Delivery::Link => "link".to_owned(),
        },
        title: points(entry.title.as_ref()),
        summary: points(entry.summary.as_ref()),
        body_present: entry.body.is_some(),
        body_title: body_text("title"),
        body_scope: body_text("scope"),
    }
}

fn native_receipt_json(root: &Path) -> String {
    let context = native_context(root, false).expect("native action context");
    let inline = native_context(root, true);
    let (error_type, error_message) = match inline {
        Err(DoctrineError::ProgressiveDelivery { source }) => match *source {
            ProgressiveDeliveryError::Payload {
                cause: PayloadFailure::BytesNotUtf8(cause),
                ..
            } => (
                "UnicodeDecodeError".to_owned(),
                sk_kernel::model_json::model_json_bytes_error_text(&cause),
            ),
            other => panic!("unexpected progressive-delivery phase: {other:?}"),
        },
        Ok(_) => panic!("the inline body must refuse the non-UTF-8 bytes"),
        Err(other) => panic!("unexpected error: {other:?}"),
    };
    let view_json = |entry: &EntryView| {
        let points = |values: &Option<Vec<u32>>| match values {
            Some(values) => json!(values),
            None => JsonValue::Null,
        };
        json!({
            "id": entry.id,
            "source": entry.source,
            "delivery": entry.delivery,
            "title": points(&entry.title),
            "summary": points(&entry.summary),
            "body_present": entry.body_present,
            "body_title": points(&entry.body_title),
            "body_scope": points(&entry.body_scope),
        })
    };
    let receipt = json!({
        "runner": "native",
        "cwd": std::env::current_dir().expect("cwd").display().to_string(),
        "mode": match context.mode {
            ContextMode::Bootstrap => "bootstrap",
            ContextMode::Compact => "compact",
            // Mechanical ripple: the combined single build has no text rail.
            ContextMode::Missing => "missing",
        },
        "link": view_json(&native_view(&context, "styleguides", "common-docs")),
        "missing": view_json(&native_view(&context, "tactics", "cf61-ghost")),
        "merged": view_json(&native_view(&context, "directives", "DIRECTIVE_001")),
        "inline_error": {"type": error_type, "message": error_message},
    });
    receipt.to_string()
}

/// Every authored byte below the fixture's `.kittify` tree, for the no-write
/// control. The Native build must not create or mutate a file.
fn kitty_bytes(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(base: &Path, path: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(base, &path, out);
            } else if let Ok(bytes) = std::fs::read(&path) {
                out.insert(
                    path.strip_prefix(base).expect("relative").to_path_buf(),
                    bytes,
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(&root.join(".kittify"), &root.join(".kittify"), &mut out);
    out
}

/// The Native child: assert isolation, then answer from the canonical API.
#[test]
fn cf61_native_child_and_receipt() {
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
    println!("CF61_RECEIPT {}", native_receipt_json(&root));
}

/// The paired proof: link metadata is lazy, the same record's inline body raises
/// the reference's typed payload phase, and a true missing id stays a healthy
/// entry.
#[test]
fn lazy_link_survives_but_inline_body_reports_the_payload_failure() {
    let root = fixture();
    let authored_before = kitty_bytes(&root);
    let config_dirs = root.join("config-dirs");
    let data_dirs = root.join("data-dirs");

    let source_stdout = isolated::source_script(
        &root,
        SOURCE_SCRIPT,
        &[
            ("SK_CF61_ROOT", root.to_str().expect("root")),
            ("PYTHONHASHSEED", "0"),
            ("XDG_CONFIG_DIRS", config_dirs.to_str().expect("dirs")),
            ("XDG_DATA_DIRS", data_dirs.to_str().expect("dirs")),
        ],
    );
    let source = parse_receipt(&source_stdout);
    // The Source build itself records encoding provenance (a real Source write,
    // not this lane's behaviour). Every pre-existing authored byte is unchanged;
    // the only addition is that Source-owned record.
    let after_source = kitty_bytes(&root);
    for (path, bytes) in &authored_before {
        assert_eq!(
            after_source.get(path),
            Some(bytes),
            "Source must not rewrite authored {}",
            path.display()
        );
    }
    let added: Vec<&PathBuf> = after_source
        .keys()
        .filter(|path| !authored_before.contains_key(*path))
        .collect();
    assert_eq!(
        added,
        vec![&PathBuf::from("encoding-provenance/global.jsonl")],
        "the only Source addition is its encoding-provenance record"
    );
    let authored_before_native = after_source.clone();
    assert_eq!(
        source.runner, "source-python-3.11.15",
        "the Source child is the pinned interpreter"
    );
    assert_eq!(source.cwd, root.display().to_string());

    // The intended Source cause is asserted BEFORE the Native child runs.
    assert_eq!(source.mode, "bootstrap");
    assert_eq!(source.link.id, "common-docs");
    assert_eq!(source.link.source, "project");
    assert_eq!(
        source.link.delivery, "link",
        "a suggests-reached styleguide is a link"
    );
    assert_eq!(
        source.link.title,
        Some("CF61 lazy docs é title".chars().map(u32::from).collect()),
        "the reference reads the merged model's title property"
    );
    assert!(
        !source.link.body_present,
        "the reference never dumps a link's body"
    );
    assert_eq!(source.missing.id, "cf61-ghost");
    assert_eq!(
        source.missing.source, "built-in",
        "an absent record keeps the reference provenance fallback"
    );
    assert_eq!(source.missing.delivery, "inline");
    assert_eq!(source.missing.title, None);
    assert_eq!(source.missing.summary, None);
    assert!(
        !source.missing.body_present,
        "a true missing id contributes no body"
    );
    assert_eq!(source.merged.id, "DIRECTIVE_001");
    assert_eq!(source.merged.source, "project");
    assert_eq!(source.merged.delivery, "inline");
    assert_eq!(
        source.merged.title,
        Some("CF61 merged é title".chars().map(u32::from).collect())
    );
    assert_eq!(
        source.merged.summary,
        Some("CF61 merged intent".chars().map(u32::from).collect())
    );
    assert_eq!(
        source.merged.body_title, source.merged.title,
        "the body carries the overlaid title"
    );
    assert!(
        source.merged.body_scope.is_some(),
        "an overridden-away field is inherited from the built-in record"
    );
    let (source_error, source_message) = source
        .inline_error
        .as_ref()
        .expect("the include_all body must raise");
    assert_eq!(source_error, "UnicodeDecodeError");
    assert!(
        source_message.contains("'utf-8' codec can't decode byte 0xff"),
        "the reference's real payload cause: {source_message}"
    );

    // Now the Native child must reproduce that answer.
    let native_stdout = isolated::native_reexec(
        &root,
        "cf61_native_child_and_receipt",
        &[
            ("PYTHONHASHSEED", "0"),
            ("XDG_CONFIG_DIRS", config_dirs.to_str().expect("dirs")),
            ("XDG_DATA_DIRS", data_dirs.to_str().expect("dirs")),
        ],
    );
    let native = parse_receipt(&native_stdout);
    assert_eq!(native.runner, "native");
    assert_eq!(native.cwd, root.display().to_string());
    assert_eq!(native.mode, source.mode);
    assert_eq!(native.link, source.link);
    assert_eq!(native.missing, source.missing);
    assert_eq!(native.merged, source.merged);
    assert_eq!(native.inline_error, source.inline_error);

    // No-write control: the Native action build left every byte below `.kittify`
    // exactly as the Source probe left it (no new provenance record, no state
    // file, no edit to an authored artifact).
    assert_eq!(
        kitty_bytes(&root),
        authored_before_native,
        "the Native action build must not write below .kittify"
    );
}
