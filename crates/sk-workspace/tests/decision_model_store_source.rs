//! Source-first regressions for the shared typed `DecisionIndex` model and the
//! read-only index store.
//!
//! Source: `specify_cli.decisions.store.load_index` (missing → the EMPTY index;
//! present → canonical byte decode + `DecisionIndex.model_validate`, with
//! `DecisionIndexReadError` on a corrupt one) and the two pinned consumer views of
//! the validated entries (`acceptance._has_blocking_clarification_marker`'s dict
//! comprehension, i.e. LAST duplicate wins, and `decisions/service.py`'s
//! `next(...)`, i.e. FIRST duplicate wins).
//!
//! The pinned Source runs FIRST (CPython 3.11.15 through the repository oracle,
//! fail-closed) in an asserted temporary CWD/HOME/all-XDG/RUNTIME/TMPDIR with an
//! absolute `PYTHONPATH` and pack root, asserts the INTENDED cause of every
//! fixture, and records exact code points, projected rows and physical
//! footprints. This same binary is then re-executed on those recorded fixtures in
//! its own asserted isolated environment and must reproduce them through
//! `sk_workspace::decision_store` / `sk_workspace::decision_model`.
//!
//! Deliberate non-claims (asserted, not glossed):
//!
//! * CPython-exact failure text is compared for the non-UTF-8, JSON and
//!   non-object classes, because Kernel owns those texts.
//! * The `OSError` text (a directory, an unreadable directory) and the pydantic
//!   `ValidationError` traceback prose are NOT compared: that is the open F11
//!   error-wire family. The test asserts the class boundary, the Source wrapper
//!   text and — where the model names one — the field.
//! * The authorized `summary_json` surrogate-key exception is a merge SERIALIZER
//!   effect only, so this test asserts that LOAD and VALIDATION preserve every
//!   authored key (unpaired surrogates included) exactly.

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use sk_workspace::decision_store::{DecisionIndexLoadError, DecisionIndexReadCause};

fn roots() -> [(&'static str, &'static str); 9] {
    [
        ("HOME", "home"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_RUNTIME_DIR", "runtime"),
        ("TMPDIR", "tmp"),
        ("TMP", "tmp"),
        ("TEMP", "tmp"),
    ]
}

fn isolated(root: &Path, repo: &Path, program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .current_dir(root)
        .env("SK_D1_ROOT", root)
        .env("PYTHONPATH", repo.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
        .env("PYTHONDONTWRITEBYTECODE", "1");
    for (key, name) in roots() {
        command.env(key, root.join(name));
    }
    command
}

/// The Source program: fixtures first, intended-cause assertions, then records.
const SOURCE: &str = r##"
import json, os
from pathlib import Path

from kernel.meta_decode import MetaDecodeError
from pydantic import ValidationError
from specify_cli.decisions.store import DecisionIndexReadError, load_index

ROOT = Path(os.environ["SK_D1_ROOT"])
MISSIONS = ROOT / "missions"
MISSIONS.mkdir(parents=True, exist_ok=True)


def cp(text):
    return [ord(ch) for ch in text]


def safe(text):
    """A JSON-serializable form of interpreter text (lone surrogates are escaped)."""
    try:
        text.encode("utf-8")
    except UnicodeEncodeError:
        return ascii(text)
    return text


def cause_class(exc):
    if isinstance(exc, MetaDecodeError):
        return "MetaDecodeError"
    if isinstance(exc, ValidationError):
        return "ValidationError"
    if isinstance(exc, OSError):
        return "OSError"
    return type(exc).__name__


def entry(decision_id="D1", status="open", **kw):
    base = {
        "decision_id": decision_id,
        "origin_flow": "specify",
        "step_id": "S1",
        "input_key": "k",
        "question": "q?",
        "status": status,
        "created_at": "2026-01-02T03:04:05Z",
        "mission_id": "M1",
        "mission_slug": "m-one",
    }
    base.update(kw)
    return base


def doc(entries, mission_id="M1", **extra):
    body = {"mission_id": mission_id, "entries": entries}
    body.update(extra)
    return json.dumps(body, ensure_ascii=True).encode("utf-8")


def footprint(directory):
    if not directory.exists():
        return []
    rows = []
    for path in sorted(directory.rglob("*")):
        info = path.lstat()
        rows.append([str(path.relative_to(directory)), info.st_size, info.st_mtime_ns, info.st_mode])
    return rows


FULL = doc(
    [
        entry("D-open", "open"),
        entry("D-res", "resolved", resolved_at="2026-01-02T04:05:06Z", final_answer="yes"),
        entry("D-def", "deferred", rationale="later"),
        entry("D-can", "canceled", resolved_by="ops"),
    ]
)
DUP = doc(
    [
        entry("D-dup", "open", created_at="2026-01-02T03:04:05Z"),
        entry("D-dup", "resolved", created_at="2026-02-03T04:05:06Z"),
    ]
)
ORDER = doc(
    [
        entry("D-late", "open", created_at="2027-01-01T00:00:00Z"),
        entry("D-early", "open", created_at="2020-01-01T00:00:00Z"),
    ]
)
DEFAULTS = doc([{"decision_id": "D-min", "origin_flow": "charter", "slot_key": "slot",
                 "input_key": "k", "question": "q?", "status": "open",
                 "created_at": "2026-01-02", "mission_id": "M1", "mission_slug": "m-one"}])
EPOCH = doc(
    [
        entry("D-int", created_at=1700000000),
        entry("D-float", created_at=1700000000.5),
        entry("D-offset", created_at="2026-01-02T03:04:05.000250+02:30"),
        entry("D-naive", created_at="2026-01-02 03:04:05"),
        entry("D-string-int", created_at="1700000000"),
    ]
)
# The authorized exception is a serializer effect: load/validation must keep every
# authored key, unpaired-surrogate keys included.
SURROGATE_SUMMARY = (
    b'{"mission_id": "M1", "entries": [{"decision_id": "D-sum", "origin_flow": "specify",'
    b' "step_id": "S1", "input_key": "k", "question": "q?", "status": "open",'
    b' "created_at": "2026-01-02T03:04:05Z", "mission_id": "M1", "mission_slug": "m-one",'
    b' "summary_json": {"\\ud800": "first", "\\ud801": "second", "\\u0000": "nul",'
    b' "\\ue000": "pua", "\\ufdd0": "noncharacter", "\\ufffd\\ufffd\\ufffd": "authored"}}]}'
)
SURROGATE_ID = (
    b'{"mission_id": "M1", "entries": [{"decision_id": "D-\\ud800id", "origin_flow": "specify",'
    b' "step_id": "S1", "input_key": "k", "question": "q?", "status": "deferred",'
    b' "created_at": "2026-01-02T03:04:05Z", "mission_id": "M1", "mission_slug": "m-one"}]}'
)
CONSTRAINED_SURROGATE = (
    b'{"mission_id": "M1", "entries": [{"decision_id": "D-bad", "origin_flow": "specify",'
    b' "step_id": "S1", "input_key": "\\ud800", "question": "q?", "status": "open",'
    b' "created_at": "2026-01-02T03:04:05Z", "mission_id": "M1", "mission_slug": "m-one"}]}'
)

CASES = []


def case(name, spec, check):
    CASES.append((name, spec, check))


def ok(name, body, ids, statuses):
    case(name, {"kind": "file", "bytes": list(body)}, ("ok", ids, statuses))


def refusal(name, body, expected_class, needle):
    case(name, {"kind": "file", "bytes": list(body)}, ("read_error", expected_class, needle))


ok("empty-index", doc([]), [], [])
ok("full-four-statuses", FULL,
   ["D-open", "D-res", "D-def", "D-can"],
   ["open", "resolved", "deferred", "canceled"])
ok("defaults", DEFAULTS, ["D-min"], ["open"])
ok("epoch-and-offsets", EPOCH,
   ["D-int", "D-float", "D-offset", "D-naive", "D-string-int"], ["open"] * 5)
ok("duplicates", DUP, ["D-dup", "D-dup"], ["open", "resolved"])
ok("document-order", ORDER, ["D-late", "D-early"], ["open", "open"])
ok("surrogate-summary-keys", SURROGATE_SUMMARY, ["D-sum"], ["open"])
ok("surrogate-decision-id", SURROGATE_ID, ["D-\ud800id"], ["deferred"])

refusal("missing-mission-id", b'{"entries": []}', "ValidationError", "mission_id")
refusal("unknown-status", doc([entry("D1", "done")]), "ValidationError", "status")
refusal("extra-entry-field", doc([entry("D1", written_by="x")]), "ValidationError", "written_by")
refusal("constrained-surrogate", CONSTRAINED_SURROGATE, "ValidationError", "input_key")
refusal("empty-input-key", doc([entry("D1", input_key="")]), "ValidationError", "input_key")
refusal("no-step-or-slot", doc([entry("D1", step_id=None)]), "ValidationError", "step_id")
refusal("extra-top-level", doc([], revision=2), "ValidationError", "revision")
refusal("version-two", doc([], version=2), "ValidationError", "version")
refusal("entries-not-list", b'{"mission_id": "M1", "entries": {}}',
        "ValidationError", "entries")
refusal("entry-not-object", b'{"mission_id": "M1", "entries": [1]}', "ValidationError", "entries")
refusal("corrupt-json", b'{', "MetaDecodeError", "Expecting property name")
refusal("empty-bytes", b'', "MetaDecodeError", "Expecting value")
refusal("bom", b'\xef\xbb\xbf{"mission_id": ""}', "MetaDecodeError", "UTF-8 BOM")
refusal("non-utf8", b'{"mission_id": "\xff"}', "MetaDecodeError", "can't decode byte 0xff")
refusal("top-level-list", b'[]', "MetaDecodeError", "Expected JSON object, got list")
refusal("top-level-string", b'"x"', "MetaDecodeError", "Expected JSON object, got str")
refusal("top-level-null", b'null', "MetaDecodeError", "Expected JSON object, got NoneType")

case("missing-dir", {"kind": "missing_dir"}, ("empty", None, None))
case("missing-index", {"kind": "no_index"}, ("empty", None, None))
case("index-is-directory", {"kind": "dir_at_index"}, ("read_error", "OSError", ""))
case("symlink-to-valid", {"kind": "symlink", "valid_target": True}, ("ok", ["D-link"], ["open"]))
case("broken-symlink", {"kind": "symlink", "valid_target": False}, ("empty", None, None))
case("permission-denied", {"kind": "chmod_dir"}, ("probe_error", "OSError", ""))

records = []
for name, spec, check in CASES:
    mission = MISSIONS / name
    mission.mkdir(parents=True, exist_ok=True)
    kind = spec["kind"]
    if kind == "missing_dir":
        mission.rmdir()
    elif kind == "no_index":
        (mission / "decisions").mkdir()
    elif kind == "file":
        (mission / "decisions").mkdir()
        (mission / "decisions" / "index.json").write_bytes(bytes(spec["bytes"]))
    elif kind == "dir_at_index":
        (mission / "decisions" / "index.json").mkdir(parents=True)
    elif kind == "symlink":
        (mission / "decisions").mkdir()
        target = mission / "decisions" / "target.json"
        if spec["valid_target"]:
            target.write_bytes(doc([entry("D-link", "open")]))
        os.symlink(target.name, mission / "decisions" / "index.json")
    elif kind == "chmod_dir":
        (mission / "decisions").mkdir()
        (mission / "decisions" / "index.json").write_bytes(doc([entry("D-nope")]))
        os.chmod(mission / "decisions", 0o000)

    if kind == "chmod_dir" and os.geteuid() == 0:
        # As root the probe cannot fail; record the premise instead of pretending.
        records.append({"name": name, "spec": spec, "outcome": "skipped_euid_root"})
        os.chmod(mission / "decisions", 0o755)
        continue

    before = footprint(mission)
    try:
        raw_bytes = (mission / "decisions" / "index.json").read_bytes()
    except OSError:
        raw_bytes = None
    record = {"name": name, "spec": spec}
    try:
        index = load_index(mission)
    except DecisionIndexReadError as exc:
        assert check[0] == "read_error", (name, "unexpected read error", safe(str(exc)))
        assert cause_class(exc.cause) == check[1], (name, type(exc.cause).__name__, check[1])
        assert check[2] in str(exc.cause), (name, safe(str(exc.cause)))
        record["outcome"] = "read_error"
        record["cause_class"] = cause_class(exc.cause)
        record["cause_type"] = type(exc.cause).__name__
        record["cause_text"] = safe(str(exc.cause))
        record["error_text"] = safe(str(exc))
        record["path_is_index"] = str(exc.index_path) == str(mission / "decisions" / "index.json")
    except OSError as exc:
        assert check[0] == "probe_error", (name, "unexpected probe error", repr(exc))
        assert cause_class(exc) == check[1], (name, type(exc).__name__)
        record["outcome"] = "probe_error"
        record["cause_class"] = cause_class(exc)
        record["cause_type"] = type(exc).__name__
    else:
        ids = [e.decision_id for e in index.entries]
        statuses = [e.status.value for e in index.entries]
        if check[0] == "empty":
            assert index.mission_id == "", (name, index.mission_id)
            assert index.entries == (), (name, index.entries)
            record["outcome"] = "empty"
        else:
            assert check[0] == "ok", (name, "expected a refusal", ids)
            assert ids == check[1], (name, ids, check[1])
            assert statuses == check[2], (name, statuses, check[2])
            record["outcome"] = "ok"
        record["mission_id"] = cp(index.mission_id)
        record["version"] = index.version
        raw_entries = []
        try:
            document = json.loads(raw_bytes.decode("utf-8")) if raw_bytes is not None else {}
            if isinstance(document.get("entries"), list):
                raw_entries = document["entries"]
        except (AttributeError, TypeError, ValueError):
            raw_entries = []
        entries = []
        for position, e in enumerate(index.entries):
            dumped = e.model_dump(mode="json")
            authored = None
            if position < len(raw_entries):
                summary = raw_entries[position].get("summary_json")
                if isinstance(summary, dict):
                    authored = [[cp(k), cp(v)] for k, v in summary.items()]
            entries.append(
                {
                    "id": cp(e.decision_id),
                    "status": e.status.value,
                    # Exact projected row, re-rendered ASCII-safely so the Rust
                    # side can compare code points (surrogate keys included).
                    "fields_json": json.dumps(dumped, sort_keys=True, ensure_ascii=True),
                    # `model_dump(mode="json")` is where Source's JSON-mode
                    # serializer collapses unpaired-surrogate summary keys; the
                    # authored mapping is what LOAD kept.
                    "source_summary": [
                        [cp(k), cp(v)] for k, v in (dumped.get("summary_json") or {}).items()
                    ],
                    "authored_summary": authored,
                }
            )
        record["entries"] = entries
        # The two pinned consumer views of a duplicate decision_id.
        by_id = {e.decision_id: e for e in index.entries}
        record["last_wins"] = [[cp(k), v.status.value] for k, v in by_id.items()]
        first = []
        seen = set()
        for e in index.entries:
            if e.decision_id not in seen:
                seen.add(e.decision_id)
                first.append([cp(e.decision_id), e.status.value])
        record["first_wins"] = first
    after = footprint(mission)
    if kind == "chmod_dir":
        os.chmod(mission / "decisions", 0o755)
        # The mode change above is this harness's own cleanup, so compare only the
        # index entry, whose bytes and mtime must be untouched.
        before = [row for row in before if row[0].endswith("index.json")]
        after = [row for row in after if row[0].endswith("index.json")]
    record["footprint_unchanged"] = before == after
    record["index_exists"] = (mission / "decisions" / "index.json").exists()
    records.append(record)

(ROOT / "store-cases.json").write_text(json.dumps(records, ensure_ascii=True, indent=1))
print("D1 source cases:", len(records))
"##;

#[test]
fn decision_model_and_store_match_actual_source() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    if let Some(root) = std::env::var_os("SK_D1_NATIVE") {
        native_phase(&repo, PathBuf::from(root));
        return;
    }
    let root = tempfile::tempdir().unwrap().keep();
    for (_, name) in roots() {
        fs::create_dir_all(root.join(name)).unwrap();
    }
    eprintln!("D1 decision model/store evidence: {}", root.display());
    let source = isolated(&root, &repo, oracle::python())
        .args(["-c", SOURCE])
        .output()
        .unwrap();
    fs::write(root.join("source.stdout"), &source.stdout).unwrap();
    fs::write(root.join("source.stderr"), &source.stderr).unwrap();
    assert!(
        source.status.success(),
        "Source FIRST: {} {}",
        String::from_utf8_lossy(&source.stdout),
        String::from_utf8_lossy(&source.stderr)
    );
    let native = isolated(&root, &repo, std::env::current_exe().unwrap())
        .args([
            "--exact",
            "decision_model_and_store_match_actual_source",
            "--nocapture",
        ])
        .env("SK_D1_NATIVE", &root)
        .output()
        .unwrap();
    fs::write(root.join("native.stdout"), &native.stdout).unwrap();
    fs::write(root.join("native.stderr"), &native.stderr).unwrap();
    assert!(
        native.status.success(),
        "Native: {} {}",
        String::from_utf8_lossy(&native.stdout),
        String::from_utf8_lossy(&native.stderr)
    );
}

/// The Native phase: replay every recorded fixture and compare.
fn native_phase(repo: &Path, root: PathBuf) {
    // The isolation claim is asserted, not assumed (L2's protocol flaw).
    assert_eq!(std::env::current_dir().unwrap(), root);
    for (key, name) in roots() {
        assert_eq!(
            std::env::var_os(key).unwrap(),
            root.join(name),
            "{key} must point inside the isolated root"
        );
    }
    assert_eq!(std::env::var_os("PYTHONPATH").unwrap(), repo.join("src"));
    assert_eq!(
        std::env::var_os("SPEC_KITTY_PACKS_ROOT").unwrap(),
        repo.join("packs")
    );

    let records: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("store-cases.json")).unwrap()).unwrap();
    let missions = root.join("native-missions");
    let mut accepted = 0usize;
    let mut refused = 0usize;
    let mut empty = 0usize;
    for record in records.as_array().unwrap() {
        let name = record["name"].as_str().unwrap();
        let outcome = record["outcome"].as_str().unwrap();
        if outcome == "skipped_euid_root" {
            eprintln!("{name}: Source probe premise unavailable as root; not asserted");
            continue;
        }
        let mission = missions.join(name);
        let index_file = mission.join("decisions").join("index.json");
        materialize(name, &record["spec"], &mission);

        let before = footprint(&mission);
        let loaded = sk_workspace::decision_store::load_index(&mission);
        let after = footprint(&mission);
        assert_eq!(
            before, after,
            "{name}: load must not write, create, lock or touch anything"
        );
        assert_eq!(
            sk_workspace::decision_store::index_path(&mission),
            index_file,
            "{name}: index path"
        );

        match outcome {
            "empty" => {
                let store = loaded.expect("a missing index is the validated empty index");
                assert!(store.mission_id().is_empty(), "{name}: empty mission_id");
                assert!(store.entries().is_empty(), "{name}: no entries");
                assert!(store.statuses().is_empty(), "{name}: no statuses");
                assert_eq!(
                    store.version(),
                    &sk_kernel::python_json::PythonJsonValue::Integer("1".to_owned()),
                    "{name}: empty index version keeps the model default"
                );
                empty += 1;
            }
            "ok" => {
                let store = loaded.expect("Source accepted this index");
                let expected_mission = codepoints(&record["mission_id"]);
                assert_eq!(
                    store.mission_id().codepoints(),
                    expected_mission.as_slice(),
                    "{name}: mission_id code points"
                );
                let expected_entries = record["entries"].as_array().unwrap();
                assert_eq!(
                    store.entries().len(),
                    expected_entries.len(),
                    "{name}: entry count"
                );
                for (entry, expected) in store.entries().iter().zip(expected_entries) {
                    assert_eq!(
                        entry.decision_id().codepoints(),
                        codepoints(&expected["id"]).as_slice(),
                        "{name}: decision_id code points"
                    );
                    assert_eq!(
                        entry.status().as_str(),
                        expected["status"].as_str().unwrap(),
                        "{name}: status"
                    );
                    let recorded =
                        sk_kernel::python_json::parse(expected["fields_json"].as_str().unwrap())
                            .unwrap();
                    let sk_kernel::python_json::PythonJsonValue::ExactObject(recorded) = recorded
                    else {
                        panic!("{name}: the recorded projected row must be an object");
                    };
                    // `model_dump(mode="json")` in Source is exactly where its
                    // JSON-mode serializer collapses unpaired-surrogate
                    // `summary_json` keys. That collapse is the ONLY authorized
                    // deviation, so it is asserted explicitly instead of being
                    // folded into a row equality claim.
                    let source_summary = summary_pairs(&expected["source_summary"]);
                    let authored_summary = expected["authored_summary"]
                        .as_array()
                        .map(|_| summary_pairs(&expected["authored_summary"]));
                    let authorized_exception = authored_summary
                        .as_ref()
                        .is_some_and(|authored| authored != &source_summary);
                    compare_row(name, entry, &recorded, authorized_exception);
                    if let Some(authored) = authored_summary.filter(|_| authorized_exception) {
                        assert_eq!(
                            summary_pairs_of(entry.field("summary_json")),
                            authored,
                            "{name}: load/model keep every authored summary key and value"
                        );
                        assert!(
                            source_summary.len() < authored.len(),
                            "{name}: Source's JSON-mode projection collapsed keys (the authorized \
                             deviation), while the loader preserves them"
                        );
                    }
                }
                // The status views pinned by the two Source consumers.
                let last: BTreeMap<Vec<u32>, String> = record["last_wins"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|pair| (codepoints(&pair[0]), pair[1].as_str().unwrap().to_owned()))
                    .collect();
                for (id, status) in &last {
                    let id = sk_kernel::PythonString::from_codepoints(id).unwrap();
                    assert_eq!(
                        store.status_of(&id).map(|value| value.as_str().to_owned()),
                        Some(status.clone()),
                        "{name}: last-wins status view"
                    );
                }
                let first: Vec<(Vec<u32>, String)> = record["first_wins"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|pair| (codepoints(&pair[0]), pair[1].as_str().unwrap().to_owned()))
                    .collect();
                for (id, status) in &first {
                    let id = sk_kernel::PythonString::from_codepoints(id).unwrap();
                    assert_eq!(
                        store
                            .first_status_of(&id)
                            .map(|value| value.as_str().to_owned()),
                        Some(status.clone()),
                        "{name}: first-wins status view"
                    );
                }
                assert_eq!(
                    store.statuses().len(),
                    last.len(),
                    "{name}: status view size"
                );
                accepted += 1;
            }
            "read_error" => {
                let error = loaded.expect_err("Source refused this index");
                let read = error
                    .read()
                    .unwrap_or_else(|| panic!("{name}: a decode failure must be a read error"));
                let expected_class = record["cause_class"].as_str().unwrap();
                let class = match read.cause() {
                    DecisionIndexReadCause::Io(_) => "OSError",
                    DecisionIndexReadCause::NonUtf8(_)
                    | DecisionIndexReadCause::Json(_)
                    | DecisionIndexReadCause::NotObject { .. } => "MetaDecodeError",
                    DecisionIndexReadCause::Model(_) => "ValidationError",
                };
                assert_eq!(class, expected_class, "{name}: cause class");
                // The wrapper text is Source's, with the truthful native cause.
                let text = error.to_string();
                assert!(
                    text.starts_with(&format!("Cannot read {}", index_file.display())),
                    "{name}: Source wrapper prefix: {text}"
                );
                assert!(
                    text.ends_with(
                        "fail-closed (index.json exists but is corrupt or unreadable); run: \
                         spec-kitty doctor"
                    ),
                    "{name}: Source wrapper suffix: {text}"
                );
                assert_eq!(read.path(), index_file.as_path(), "{name}: error path");
                assert!(
                    record["path_is_index"].as_bool().unwrap(),
                    "{name}: Source reports the index path"
                );
                match read.cause() {
                    // Kernel owns these CPython texts, so equality is asserted.
                    DecisionIndexReadCause::NonUtf8(_)
                    | DecisionIndexReadCause::Json(_)
                    | DecisionIndexReadCause::NotObject { .. } => {
                        assert_eq!(
                            &read.cause().to_string(),
                            record["cause_text"].as_str().unwrap(),
                            "{name}: CPython-exact decode cause"
                        );
                    }
                    // pydantic traceback prose and OSError text remain the open
                    // F11 wire: assert the typed class and the field, never the
                    // prose, and never call class-only parity complete.
                    DecisionIndexReadCause::Model(model) => {
                        let source_text = record["cause_text"].as_str().unwrap();
                        if let Some(field) = model.field() {
                            assert!(
                                source_text.contains(field),
                                "{name}: Source names {field:?}, native model names it too: \
                                 {source_text}"
                            );
                        }
                        eprintln!(
                            "{name}: ValidationError prose is Native (F11 open), typed field \
                             {:?}",
                            model.field()
                        );
                    }
                    DecisionIndexReadCause::Io(cause) => {
                        eprintln!("{name}: OSError text is Native (F11 open): {cause}");
                    }
                }
                refused += 1;
            }
            "probe_error" => {
                match loaded.expect_err("Source's existence probe raised") {
                    DecisionIndexLoadError::Probe { path, .. } => {
                        assert_eq!(path.as_path(), index_file.as_path(), "{name}: probe path");
                    }
                    other => {
                        panic!("{name}: a probe failure must not be a read error: {other}")
                    }
                }
                refused += 1;
            }
            other => panic!("{name}: unhandled Source outcome {other}"),
        }
        // Source's own physical-footprint claim is a premise this replay inherits.
        assert!(
            record["footprint_unchanged"].as_bool().unwrap(),
            "{name}: Source recorded a changed footprint; the fixture premise is false"
        );
    }
    assert!(accepted >= 8, "the suite must exercise accepted indexes");
    assert!(refused >= 8, "the suite must exercise refusals");
    assert!(empty >= 2, "the suite must exercise the missing-index rule");
    eprintln!("D1 native checks: {accepted} accepted, {refused} refused, {empty} empty");
}

fn codepoints(value: &serde_json::Value) -> Vec<u32> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|unit| unit.as_u64().unwrap() as u32)
        .collect()
}

/// A recorded `[[code points, code points], ...]` summary mapping.
fn summary_pairs(value: &serde_json::Value) -> Vec<(Vec<u32>, Vec<u32>)> {
    value
        .as_array()
        .map(|pairs| {
            pairs
                .iter()
                .map(|pair| (codepoints(&pair[0]), codepoints(&pair[1])))
                .collect()
        })
        .unwrap_or_default()
}

/// The same mapping read off a projected `summary_json` value.
fn summary_pairs_of(
    value: Option<&sk_kernel::python_json::PythonJsonValue>,
) -> Vec<(Vec<u32>, Vec<u32>)> {
    match value {
        Some(sk_kernel::python_json::PythonJsonValue::ExactObject(summary)) => summary
            .iter()
            .map(|(key, value)| match value {
                sk_kernel::python_json::PythonJsonValue::String(text) => {
                    (key.codepoints().to_vec(), text.codepoints().to_vec())
                }
                _ => panic!("a projected summary value is always a string"),
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Compare one projected row, optionally skipping the excepted `summary_json`.
fn compare_row(
    name: &str,
    entry: &sk_workspace::decision_model::DecisionEntry,
    recorded: &sk_kernel::python_json::PythonJsonDict,
    skip_summary: bool,
) {
    assert_eq!(
        entry.fields().len(),
        recorded.len(),
        "{name}: projected row key count"
    );
    let summary = sk_kernel::PythonString::from_scalars("summary_json");
    for (key, value) in recorded.iter() {
        if skip_summary && key == &summary {
            continue;
        }
        let actual = entry.fields().get_codepoints(key.codepoints());
        assert!(
            actual.is_some_and(|actual| sk_kernel::python_json::python_json_equal(actual, value)),
            "{name}: projected field {} differs",
            key.repr()
        );
    }
}

/// Rebuild one recorded fixture exactly as the Source phase created it.
fn materialize(name: &str, spec: &serde_json::Value, mission: &Path) {
    let kind = spec["kind"].as_str().unwrap();
    let decisions = mission.join("decisions");
    match kind {
        "missing_dir" => {
            assert!(!mission.exists(), "{name}: missing-dir premise");
        }
        "no_index" => {
            fs::create_dir_all(&decisions).unwrap();
        }
        "file" => {
            fs::create_dir_all(&decisions).unwrap();
            let bytes: Vec<u8> = spec["bytes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_u64().unwrap() as u8)
                .collect();
            fs::write(decisions.join("index.json"), bytes).unwrap();
        }
        "dir_at_index" => {
            fs::create_dir_all(decisions.join("index.json")).unwrap();
        }
        "symlink" => {
            fs::create_dir_all(&decisions).unwrap();
            if spec["valid_target"].as_bool().unwrap() {
                fs::write(decisions.join("target.json"), link_target_body()).unwrap();
            }
            std::os::unix::fs::symlink("target.json", decisions.join("index.json")).unwrap();
        }
        "chmod_dir" => {
            fs::create_dir_all(&decisions).unwrap();
            fs::write(decisions.join("index.json"), link_target_body()).unwrap();
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&decisions, fs::Permissions::from_mode(0o000)).unwrap();
        }
        other => panic!("{name}: unhandled fixture kind {other}"),
    }
}

/// The one-entry fixture body the symlink/permission cases point at.
fn link_target_body() -> &'static [u8] {
    br#"{"mission_id": "M1", "entries": [{"decision_id": "D-link", "origin_flow": "specify", "step_id": "S1", "input_key": "k", "question": "q?", "status": "open", "created_at": "2026-01-02T03:04:05Z", "mission_id": "M1", "mission_slug": "m-one"}]}"#
}

/// Physical footprint: every path under the mission with size, mtime and mode.
fn footprint(directory: &Path) -> Vec<(String, u64, SystemTime, u32)> {
    use std::os::unix::fs::MetadataExt;
    let mut rows = Vec::new();
    let mut stack = vec![directory.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(read) = fs::read_dir(&current) else {
            continue;
        };
        for entry in read.flatten() {
            let path = entry.path();
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let relative = path
                .strip_prefix(directory)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            rows.push((
                relative,
                metadata.len(),
                metadata.modified().unwrap(),
                metadata.mode(),
            ));
            if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                stack.push(path);
            }
        }
    }
    rows.sort();
    rows
}
