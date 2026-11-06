//! CF64 — Source encoding-read provenance through a real Doctrine reader:
//! paired Source/Native proof.
//!
//! The reference records every successful charter-chokepoint read
//! (`charter.activation._io.load_charter_file`) in
//! `.kittify/encoding-provenance/global.jsonl`, routing a `kitty-specs/<mission>`
//! document to that mission's own log instead. Its DRG graph loader reads YAML
//! directly and records nothing.
//!
//! This file proves the Native composition against actual processes:
//!
//! * the **Source** child is the pinned CPython 3.11.15 oracle, reading each
//!   fixture document through `load_charter_file`;
//! * the **Native** child is this test binary re-executed, reading the same
//!   document in an independent fixture tree through
//!   `sk_doctrine::load_charter_document` / `load_document_text`, which compose
//!   `sk_kernel::text_decode::read_text_with_provenance`;
//! * each child asserts its own CWD, HOME, all seven XDG roots, TMP/TMPDIR/TEMP,
//!   absolute `PYTHONPATH`, pinned packs root and `PYTHONHASHSEED` before
//!   answering, and strings travel as code-point arrays.
//!
//! The two trees are **independent** fresh fixtures, never one tree that the
//! Source child populated for the Native child to "leave alone": the comparison is
//! business read data plus journal footprint, not pre-existing bytes.
//!
//! Not CLI acceptance, not whole-command parity, not approval.

mod support;

#[path = "support/isolated.rs"]
mod isolated;

use std::path::{Path, PathBuf};

use serde_json::{Value as JsonValue, json};
use sk_kernel::encoding_provenance::EncodingProvenanceJournal;
use sk_kernel::text_decode::ProvenanceReadError;

/// The seven XDG roots the standing orders require; `ROOTS` covers HOME, six of
/// them and the three temp variables, so these two are carried explicitly.
const EXTRA_XDG: [(&str, &str); 2] = [
    ("XDG_CONFIG_DIRS", "config-dirs"),
    ("XDG_DATA_DIRS", "data-dirs"),
];

const MISSION_ID: &str = "01M6QNZ0JDENFRX8NS45HBGPD0";

/// The fixture: a project charter, a mission document with a real `meta.json`,
/// an unprovable document and a name that does not exist.
const CHARTER_MD: &str = "# Probe Charter\n\n## Purpose\n\nA provenance probe.\n";
const MISSION_PLAN: &str = "# Plan\n\n- step one\n";
const UNPROVABLE: &[u8] = b"# caf\xe9 is not utf-8\n";
const GRAPH: &str = "\
schema_version: '1.0'
generated_at: '2026-01-01T00:00:00+00:00'
generated_by: cf64-test
nodes:
- urn: 'directive:cf64-node'
  kind: directive
  label: 'CF64 Node'
edges: []
";

/// Build one independent fixture tree. `git init` makes the event-log lock land
/// in the real `<git-common-dir>/spec-kitty-locks/event-logs` namespace, exactly
/// like a real checkout.
fn build_fixture(root: &Path) {
    let charter = root.join(".kittify/charter");
    std::fs::create_dir_all(&charter).expect("charter dir");
    std::fs::write(charter.join("charter.md"), CHARTER_MD).expect("charter.md");
    std::fs::write(charter.join("charter.yaml"), "schema_version: 1.0.0\n").expect("charter.yaml");

    let mission = root.join("kitty-specs/099-probe");
    std::fs::create_dir_all(&mission).expect("mission dir");
    std::fs::write(
        mission.join("meta.json"),
        format!("{{\"mission_id\": \"{MISSION_ID}\"}}"),
    )
    .expect("meta.json");
    std::fs::write(mission.join("plan.md"), MISSION_PLAN).expect("plan.md");
    std::fs::write(mission.join("unprovable.md"), UNPROVABLE).expect("unprovable.md");

    let graph = root.join(".kittify/charter-packs/drg");
    std::fs::create_dir_all(&graph).expect("graph dir");
    std::fs::write(graph.join("cf64.graph.yaml"), GRAPH).expect("graph fixture");

    let status = std::process::Command::new("git")
        .arg("init")
        .arg("-q")
        .arg("-b")
        .arg("main")
        .arg(root)
        .status()
        .expect("git available");
    assert!(status.success(), "git init failed");
}

/// A fresh, independent fixture tree for one child.
fn fresh_fixture(case: &str) -> PathBuf {
    let root = isolated::fixture(case);
    build_fixture(&root);
    root
}

/// The Source child's script: the same business sequence, through
/// `load_charter_file`.
const SOURCE_PROBE: &str = r#"
import json, os, pathlib, sys, traceback

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

from charter.activation._io import load_charter_file

GLOBAL = root / ".kittify/encoding-provenance/global.jsonl"
MISSION_LOG = root / "kitty-specs/099-probe/.encoding-provenance.jsonl"


def count(path):
    return len(path.read_text(encoding="utf-8").splitlines()) if path.exists() else 0


def line(path):
    return path.read_text(encoding="utf-8").splitlines()[-1] if path.exists() else None


def relative(path):
    return str(path.relative_to(root))


cases = []
charter = root / ".kittify/charter/charter.md"

# 1. ordinary UTF-8 read
content = load_charter_file(charter)
record = json.loads(line(GLOBAL))
cases.append({
    "case": "charter_utf8",
    "text": [ord(char) for char in content.text],
    "source_encoding": content.source_encoding,
    "normalization_applied": content.normalization_applied,
    "confidence": content.confidence,
    "bypass_used": record["bypass_used"],
    "mission_id": record["mission_id"],
    "file_path": record["file_path"].replace(str(root), "<ROOT>"),
    "log": relative(GLOBAL),
    "records": count(GLOBAL),
    "error": None,
})

# 2. the same document re-read with a UTF-8 BOM: append, never deduplicate
charter.write_bytes(b"\xef\xbb\xbf" + charter.read_bytes())
content = load_charter_file(charter)
record = json.loads(line(GLOBAL))
cases.append({
    "case": "charter_bom",
    "text": [ord(char) for char in content.text],
    "source_encoding": content.source_encoding,
    "normalization_applied": content.normalization_applied,
    "confidence": content.confidence,
    "bypass_used": record["bypass_used"],
    "mission_id": record["mission_id"],
    "file_path": record["file_path"].replace(str(root), "<ROOT>"),
    "log": relative(GLOBAL),
    "records": count(GLOBAL),
    "error": None,
})

# 3. a mission document routes to the mission log and never to the global one
plan = root / "kitty-specs/099-probe/plan.md"
content = load_charter_file(plan)
record = json.loads(line(MISSION_LOG))
cases.append({
    "case": "mission_plan",
    "text": [ord(char) for char in content.text],
    "source_encoding": content.source_encoding,
    "normalization_applied": content.normalization_applied,
    "confidence": content.confidence,
    "bypass_used": record["bypass_used"],
    "mission_id": record["mission_id"],
    "file_path": record["file_path"].replace(str(root), "<ROOT>"),
    "log": relative(MISSION_LOG),
    "records": count(MISSION_LOG),
    "error": None,
})
assert count(GLOBAL) == 2, count(GLOBAL)

# 4. an unprovable encoding raises and appends nothing
globals_before = count(GLOBAL)
mission_before = count(MISSION_LOG)
try:
    load_charter_file(root / "kitty-specs/099-probe/unprovable.md")
except Exception as exc:
    cases.append({
        "case": "unprovable",
        "text": None,
        "source_encoding": None,
        "normalization_applied": None,
        "confidence": None,
        "bypass_used": None,
        "mission_id": None,
        "file_path": None,
        "log": None,
        "records": count(GLOBAL) + count(MISSION_LOG),
        "error": getattr(exc, "code", type(exc).__name__),
    })
else:
    raise AssertionError("the unprovable read did not raise")
assert count(GLOBAL) == globals_before and count(MISSION_LOG) == mission_before

# 5. an absent document fails before any record
records_before = count(GLOBAL) + count(MISSION_LOG)
try:
    load_charter_file(root / ".kittify/charter/absent.md")
except Exception as exc:
    cases.append({
        "case": "absent",
        "text": None,
        "source_encoding": None,
        "normalization_applied": None,
        "confidence": None,
        "bypass_used": None,
        "mission_id": None,
        "file_path": None,
        "log": None,
        "records": count(GLOBAL) + count(MISSION_LOG),
        "error": "FileNotFoundError" if isinstance(exc, FileNotFoundError) else type(exc).__name__,
    })
else:
    raise AssertionError("the absent read did not raise")
assert count(GLOBAL) + count(MISSION_LOG) == records_before

# Source-only observation: the DRG loader is NOT a provenance reader.
from charter.offering.drg.loader import load_graph
before = count(GLOBAL) + count(MISSION_LOG)
graph = load_graph(root / ".kittify/charter-packs/drg/cf64.graph.yaml")
graph_observation = {
    "nodes": len(graph.nodes),
    "records": count(GLOBAL) + count(MISSION_LOG),
    "unchanged": count(GLOBAL) + count(MISSION_LOG) == before,
}

print("READ_RECEIPT " + json.dumps({
    "runner": "source-python-3.11.15",
    "cwd": str(pathlib.Path.cwd()),
    "cases": cases,
    "graph": graph_observation,
}))
"#;

/// Every case name, in fixture order.
const CASES: [&str; 5] = [
    "charter_utf8",
    "charter_bom",
    "mission_plan",
    "unprovable",
    "absent",
];

fn normalize_log(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn record_count(path: &Path) -> usize {
    std::fs::read_to_string(path)
        .map(|text| text.lines().count())
        .unwrap_or(0)
}

fn code_points(text: &str) -> Vec<u32> {
    text.chars().map(u32::from).collect()
}

fn error_code(error: &ProvenanceReadError) -> String {
    match error {
        ProvenanceReadError::Unprovable { code, .. } => (*code).to_owned(),
        ProvenanceReadError::Read { source, .. } => {
            if matches!(
                source,
                sk_kernel::KernelError::Io { source, .. }
                    if source.kind() == std::io::ErrorKind::NotFound
            ) {
                "FileNotFoundError".to_owned()
            } else {
                format!("Read: {source}")
            }
        }
        ProvenanceReadError::Record { .. } => "Record".to_owned(),
    }
}

/// The Native child's answer: the same business sequence through the Doctrine
/// reader and the Kernel provenance journal.
fn native_receipt(root: &Path) -> JsonValue {
    let journal = EncodingProvenanceJournal::new(root);
    let global = root.join(sk_kernel::encoding_provenance::GLOBAL_LOG_RELATIVE);
    let mission_log = root
        .join("kitty-specs/099-probe")
        .join(sk_kernel::encoding_provenance::MISSION_LOG_FILENAME);
    let mut cases: Vec<JsonValue> = Vec::new();

    // 1. ordinary UTF-8 read (the composed Doctrine charter reader).
    match sk_doctrine::load_charter_document(root, &journal) {
        Ok(Some(read)) => {
            let warn = read.warnings().len();
            assert_eq!(warn, 0, "no provenance write may fail here");
            let line = std::fs::read_to_string(&global)
                .expect("global log")
                .lines()
                .next_back()
                .expect("one record")
                .to_owned();
            let value: JsonValue = serde_json::from_str(&line).expect("record JSON");
            cases.push(json!({
                "case": "charter_utf8",
                "text": code_points(&read.to_scalar_string().expect("scalar text")),
                "source_encoding": read.source_encoding(),
                "normalization_applied": read.normalization_applied(),
                "confidence": read.confidence(),
                "bypass_used": value["bypass_used"].clone(),
                "mission_id": value["mission_id"].clone(),
                "file_path": value["file_path"].as_str().expect("file_path")
                    .replace(&root.display().to_string(), "<ROOT>"),
                "log": normalize_log(root, &global),
                "records": record_count(&global),
                "error": JsonValue::Null,
            }));
        }
        other => panic!("the ordinary charter read failed: {other:?}"),
    }

    // 2. the same document with a UTF-8 BOM.
    let charter = root.join(".kittify/charter/charter.md");
    let bom = [
        b"\xef\xbb\xbf".as_slice(),
        &std::fs::read(&charter).expect("charter"),
    ]
    .concat();
    std::fs::write(&charter, bom).expect("bom charter");
    match sk_doctrine::load_charter_document(root, &journal) {
        Ok(Some(read)) => {
            let line = std::fs::read_to_string(&global)
                .expect("global log")
                .lines()
                .next_back()
                .expect("a record")
                .to_owned();
            let value: JsonValue = serde_json::from_str(&line).expect("record JSON");
            cases.push(json!({
                "case": "charter_bom",
                "text": code_points(&read.to_scalar_string().expect("scalar text")),
                "source_encoding": read.source_encoding(),
                "normalization_applied": read.normalization_applied(),
                "confidence": read.confidence(),
                "bypass_used": value["bypass_used"].clone(),
                "mission_id": value["mission_id"].clone(),
                "file_path": value["file_path"].as_str().expect("file_path")
                    .replace(&root.display().to_string(), "<ROOT>"),
                "log": normalize_log(root, &global),
                "records": record_count(&global),
                "error": JsonValue::Null,
            }));
        }
        other => panic!("the BOM charter read failed: {other:?}"),
    }

    // 3. a mission document routes to the mission log.
    let plan = root.join("kitty-specs/099-probe/plan.md");
    match sk_doctrine::load_document_text(&plan, &journal) {
        Ok(read) => {
            let line = std::fs::read_to_string(&mission_log)
                .expect("mission log")
                .lines()
                .next_back()
                .expect("a record")
                .to_owned();
            let value: JsonValue = serde_json::from_str(&line).expect("record JSON");
            cases.push(json!({
                "case": "mission_plan",
                "text": code_points(&read.to_scalar_string().expect("scalar text")),
                "source_encoding": read.source_encoding(),
                "normalization_applied": read.normalization_applied(),
                "confidence": read.confidence(),
                "bypass_used": value["bypass_used"].clone(),
                "mission_id": value["mission_id"].clone(),
                "file_path": value["file_path"].as_str().expect("file_path")
                    .replace(&root.display().to_string(), "<ROOT>"),
                "log": normalize_log(root, &mission_log),
                "records": record_count(&mission_log),
                "error": JsonValue::Null,
            }));
        }
        Err(error) => panic!("the mission read failed: {error}"),
    }
    assert_eq!(
        record_count(&global),
        2,
        "a mission record never lands in the global log"
    );

    // 4. an unprovable encoding refuses and appends nothing.
    let before = record_count(&global) + record_count(&mission_log);
    match sk_doctrine::load_document_text(
        &root.join("kitty-specs/099-probe/unprovable.md"),
        &journal,
    ) {
        Err(error) => cases.push(json!({
            "case": "unprovable",
            "text": JsonValue::Null,
            "source_encoding": JsonValue::Null,
            "normalization_applied": JsonValue::Null,
            "confidence": JsonValue::Null,
            "bypass_used": JsonValue::Null,
            "mission_id": JsonValue::Null,
            "file_path": JsonValue::Null,
            "log": JsonValue::Null,
            "records": record_count(&global) + record_count(&mission_log),
            "error": error_code(&error),
        })),
        Ok(_) => panic!("the unprovable read did not refuse"),
    }
    assert_eq!(
        record_count(&global) + record_count(&mission_log),
        before,
        "a refused read appends nothing"
    );

    // 5. an absent document fails before any record.
    match sk_doctrine::load_document_text(&root.join(".kittify/charter/absent.md"), &journal) {
        Err(error) => cases.push(json!({
            "case": "absent",
            "text": JsonValue::Null,
            "source_encoding": JsonValue::Null,
            "normalization_applied": JsonValue::Null,
            "confidence": JsonValue::Null,
            "bypass_used": JsonValue::Null,
            "mission_id": JsonValue::Null,
            "file_path": JsonValue::Null,
            "log": JsonValue::Null,
            "records": record_count(&global) + record_count(&mission_log),
            "error": error_code(&error),
        })),
        Ok(_) => panic!("the absent read did not refuse"),
    }
    assert_eq!(
        record_count(&global) + record_count(&mission_log),
        before,
        "an absent read appends nothing"
    );

    json!({
        "runner": "native",
        "cwd": std::env::current_dir().expect("cwd").display().to_string(),
        "cases": cases,
        "graph": JsonValue::Null,
    })
}

fn case<'a>(receipt: &'a JsonValue, name: &str) -> &'a JsonValue {
    receipt["cases"]
        .as_array()
        .expect("cases array")
        .iter()
        .find(|entry| entry["case"] == name)
        .unwrap_or_else(|| panic!("no {name} case in {receipt}"))
}

/// The Native child's re-exec entry point.
#[test]
fn cf64_native_child() {
    let Some(root) = isolated::native_root() else {
        return;
    };
    isolated::assert_isolated(&root);
    // The standing orders require all seven XDG roots and the hash seed, not just
    // the six `ROOTS` covers: `EXTRA_XDG` carries the other two.
    for (key, name) in EXTRA_XDG {
        assert_eq!(
            std::env::var_os(key).map(PathBuf::from),
            Some(root.join(name)),
            "{key} is the isolated fixture root"
        );
    }
    assert_eq!(
        std::env::var("PYTHONHASHSEED").as_deref(),
        Ok("0"),
        "the Native child runs with a pinned hash seed"
    );
    let receipt = native_receipt(&root);
    println!("READ_RECEIPT {}", receipt);
}

/// The extra environment every child gets on top of `child_command`'s roots:
/// the two remaining XDG roots and the pinned hash seed.
fn extra_env(root: &Path) -> Vec<(String, String)> {
    for name in ["config-dirs", "data-dirs"] {
        std::fs::create_dir_all(root.join(name)).expect("extra XDG root");
    }
    vec![
        (
            "XDG_CONFIG_DIRS".to_owned(),
            root.join("config-dirs").display().to_string(),
        ),
        (
            "XDG_DATA_DIRS".to_owned(),
            root.join("data-dirs").display().to_string(),
        ),
        ("PYTHONHASHSEED".to_owned(), "0".to_owned()),
    ]
}

#[test]
fn a_source_compatible_read_leaves_the_same_business_data_and_journal_footprint() {
    // Two independent fixtures: the Source child never populates the Native
    // child's tree, so the Native footprint is its own real read.
    let source_root = fresh_fixture("cf64-source");
    let native_root = fresh_fixture("cf64-native");

    let source_env = extra_env(&source_root);
    let source_env: Vec<(&str, &str)> = source_env
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    // The Source cause is asserted before the Native child runs.
    let source_stdout = isolated::source_script(&source_root, SOURCE_PROBE, &source_env);
    let source = isolated::receipt_json(&source_stdout, "READ_RECEIPT ");
    assert_eq!(source["runner"], "source-python-3.11.15");
    assert_eq!(
        source["cwd"].as_str().expect("cwd"),
        source_root.display().to_string()
    );

    let native_env = extra_env(&native_root);
    let native_env: Vec<(&str, &str)> = native_env
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let native_stdout = isolated::native_reexec(&native_root, "cf64_native_child", &native_env);
    let native = isolated::receipt_json(&native_stdout, "READ_RECEIPT ");
    assert_eq!(native["runner"], "native");
    assert_eq!(
        native["cwd"].as_str().expect("cwd"),
        native_root.display().to_string()
    );

    for name in CASES {
        let left = case(&source, name);
        let right = case(&native, name);
        for field in [
            "text",
            "source_encoding",
            "normalization_applied",
            "confidence",
            "bypass_used",
            "mission_id",
            "file_path",
            "log",
            "records",
            "error",
        ] {
            assert_eq!(right[field], left[field], "{name}: {field}");
        }
    }

    // The business facts themselves, pinned (not only equal-by-accident).
    let utf8 = case(&native, "charter_utf8");
    assert_eq!(utf8["source_encoding"], "utf-8");
    assert_eq!(utf8["normalization_applied"], false);
    assert_eq!(utf8["records"], 1);
    assert_eq!(utf8["log"], ".kittify/encoding-provenance/global.jsonl");
    assert_eq!(utf8["text"], json!(code_points(CHARTER_MD)));

    let bom = case(&native, "charter_bom");
    assert_eq!(bom["source_encoding"], "utf-8-sig");
    assert_eq!(bom["normalization_applied"], true);
    assert_eq!(
        bom["records"], 2,
        "a re-read appends; nothing is deduplicated"
    );
    assert_eq!(bom["text"], json!(code_points(CHARTER_MD)));

    let mission = case(&native, "mission_plan");
    assert_eq!(mission["source_encoding"], "utf-8");
    assert_eq!(mission["mission_id"], MISSION_ID);
    assert_eq!(mission["records"], 1);
    assert_eq!(
        mission["log"],
        "kitty-specs/099-probe/.encoding-provenance.jsonl"
    );
    assert_eq!(mission["text"], json!(code_points(MISSION_PLAN)));

    let unprovable = case(&native, "unprovable");
    assert_eq!(unprovable["error"], "CHARTER_ENCODING_AMBIGUOUS");
    assert_eq!(unprovable["records"], 3, "the refusal appended nothing");

    let absent = case(&native, "absent");
    assert_eq!(absent["error"], "FileNotFoundError");
    assert_eq!(absent["records"], 3, "the absent read appended nothing");

    // Source-only observation, kept separate from the paired claim: the
    // reference's DRG loader appends no provenance record.
    assert_eq!(source["graph"]["nodes"], 1);
    assert_eq!(source["graph"]["unchanged"], true);
    // Native keeps the plain entry points behaviourally identical too.
    let plain =
        sk_doctrine::load_graph(&native_root.join(".kittify/charter-packs/drg/cf64.graph.yaml"))
            .expect("plain graph load");
    assert_eq!(plain.nodes().len(), 1);
    assert_eq!(
        record_count(&native_root.join(".kittify/encoding-provenance/global.jsonl")),
        2
    );
}

/// Native-only capability control: the graph document read *through* the
/// provenance door records exactly one read and parses identically.
#[test]
fn a_graph_document_read_through_the_provenance_door_records_one_read() {
    let root = fresh_fixture("cf64-doctrine-graph");
    let journal = EncodingProvenanceJournal::new(&root);
    let graph_path = root.join(".kittify/charter-packs/drg/cf64.graph.yaml");

    let plain = sk_doctrine::load_graph(&graph_path).expect("plain load");
    assert_eq!(plain.nodes().len(), 1);
    assert!(
        !root
            .join(".kittify/encoding-provenance/global.jsonl")
            .exists(),
        "the plain entry point stays a non-provenance reader"
    );

    let (proven, read) =
        sk_doctrine::load_graph_with_provenance(&graph_path, &journal).expect("proven load");
    assert_eq!(proven.nodes().len(), plain.nodes().len());
    assert_eq!(
        proven.nodes()[0].urn(),
        plain.nodes()[0].urn(),
        "the same document through either door"
    );
    assert_eq!(read.text().to_scalar_string().expect("text"), GRAPH);
    assert!(read.warnings().is_empty());
    let global = root.join(sk_kernel::encoding_provenance::GLOBAL_LOG_RELATIVE);
    assert_eq!(record_count(&global), 1);
    let record = std::fs::read_to_string(&global).expect("record");
    assert!(
        record.contains("\"source_encoding\": \"utf-8\""),
        "{record}"
    );

    // A document the strict boundary refuses appends its record first (the
    // reference's phase order) and then raises the graph error.
    std::fs::write(&graph_path, "not: a graph\n").expect("rewrite");
    let error = sk_doctrine::load_graph_with_provenance(&graph_path, &journal)
        .expect_err("unknown top-level key refuses");
    assert!(matches!(error, sk_doctrine::GraphProvenanceError::Graph(_)));
    assert_eq!(record_count(&global), 2);
}
