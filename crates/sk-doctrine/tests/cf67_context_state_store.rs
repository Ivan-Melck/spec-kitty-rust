//! CF67 — the canonical Source context-state store, paired against the real
//! reference module.
//!
//! Source `charter.activation.context_state`:
//!
//! ```python
//! def _load_state(path):
//!     if not path.exists():                     # BEFORE the try
//!         return {"schema_version": "1.0.0", "actions": {}}
//!     try:
//!         data = json.loads(path.read_text(encoding="utf-8"))
//!     except (json.JSONDecodeError, UnicodeDecodeError, OSError):
//!         return {"schema_version": "1.0.0", "actions": {}}
//!     if not isinstance(data, dict):
//!         return {"schema_version": "1.0.0", "actions": {}}
//!     if not isinstance(data.get("actions"), dict):
//!         data["actions"] = {}
//!     return data
//!
//! def _write_state(path, state):
//!     atomic_write(path, json.dumps(state, indent=2, sort_keys=True), mkdir=True)
//! ```
//!
//! This file drives the **actual Source module** and the **new Native store**
//! ([`sk_doctrine::prepare_context_state`] and friends) as two actual children,
//! each asserting its own temporary CWD, HOME, all seven XDG roots,
//! TMPDIR/TMP/TEMP, the absolute Source/packs roots, the hash seed and the pinned
//! interpreter before any business read.
//!
//! Named controls: the exact unknown-key/non-finite/lone-surrogate/big-integer
//! document survives mark/write; malformed and invalid-UTF-8 bodies fall back to
//! the model default; a non-object document resets and a non-object `actions`
//! becomes `{}` while the other keys stay; the uncaught oversized integer is an
//! ESCAPE while the malformed document is a healthy reset; the prepared snapshot
//! is written even after a controlled intervening file change; and an exact key
//! match (not a case-folded one) decides the first load.
//!
//! Not whole-action parity, not approval, not performance.

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value as JsonValue, json};
use sk_doctrine::{
    CONTEXT_STATE_SCHEMA_VERSION, ContextStateBundle, ContextStateError, ContextStateValue,
    MIN_EFFECTIVE_DEPTH, load_context_state, prepare_context_state,
};

/// The isolated roots every child asserts. `(env var, directory name)`.
const ROOTS: [(&str, &str); 9] = [
    ("HOME", "home"),
    ("XDG_CONFIG_HOME", "config"),
    ("XDG_CACHE_HOME", "cache"),
    ("XDG_DATA_HOME", "data"),
    ("XDG_STATE_HOME", "state"),
    ("XDG_RUNTIME_DIR", "runtime"),
    ("TMPDIR", "tmp"),
    ("TMP", "tmp"),
    ("TEMP", "tmp"),
];

/// The two remaining XDG roots the standing orders also require.
const EXTRA_XDG: [(&str, &str); 2] = [
    ("XDG_CONFIG_DIRS", "config-dirs"),
    ("XDG_DATA_DIRS", "data-dirs"),
];

/// The Source state path inside one fixture root.
fn state_path(root: &Path) -> PathBuf {
    root.join(".kittify/charter/context-state.json")
}

/// A fresh, unique, exclusive fixture root with all eleven isolated roots.
fn fixture(case: &str) -> PathBuf {
    let base = support::repository_root().join("target/cf67-context-state");
    std::fs::create_dir_all(&base).expect("fixture base");
    let path = tempfile::Builder::new()
        .prefix(&format!("{case}-"))
        .tempdir_in(&base)
        .expect("fresh exclusive fixture directory")
        .keep();
    for (_, name) in ROOTS.into_iter().chain(EXTRA_XDG) {
        std::fs::create_dir_all(path.join(name)).expect("isolated root");
    }
    path
}

/// A child command carrying the isolated environment and `root` as its CWD.
fn isolated_command(root: &Path, program: impl AsRef<std::ffi::OsStr>) -> Command {
    let repository = support::repository_root();
    let mut command = Command::new(program);
    command
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .current_dir(root)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONHASHSEED", "0")
        .env("SK_CF67_SOURCE_ROOT", &repository)
        .env("PYTHONPATH", repository.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repository.join("packs"));
    for (key, name) in ROOTS.into_iter().chain(EXTRA_XDG) {
        command.env(key, root.join(name));
    }
    command
}

/// Assert every root a child was handed, before any business read.
fn assert_roots(root: &Path) {
    assert_eq!(std::env::current_dir().expect("cwd"), root, "child CWD");
    for (key, name) in ROOTS.into_iter().chain(EXTRA_XDG) {
        assert_eq!(
            std::env::var_os(key).map(PathBuf::from),
            Some(root.join(name)),
            "{key} is the isolated fixture root"
        );
    }
    let repository = support::repository_root();
    assert_eq!(
        std::env::var_os("SPEC_KITTY_PACKS_ROOT").map(PathBuf::from),
        Some(repository.join("packs")),
        "pack root is the absolute repository packs"
    );
    assert_eq!(
        std::env::var_os("PYTHONPATH").map(PathBuf::from),
        Some(repository.join("src")),
        "PYTHONPATH is the absolute repository src"
    );
    assert_eq!(
        std::env::var("PYTHONHASHSEED").as_deref(),
        Ok("0"),
        "the hash seed is pinned"
    );
}

/// One store case: the authored document, the action, the explicit depth, whether
/// the first load is marked, and whether a foreign document is written between
/// the prepare and the mark.
#[derive(Clone, Copy)]
struct Case {
    name: &'static str,
    state: Option<&'static [u8]>,
    action: &'static str,
    depth: Option<usize>,
    mark: bool,
    foreign: bool,
}

/// A 5000-digit integer literal: past the interpreter's 4300-digit ceiling.
const OVERSIZED_INTEGER: &str = "digit-ceiling";

const CASES: [Case; 10] = [
    Case {
        name: "unknown-fields",
        state: Some(
            br#"{"schema_version":"1.0.0","extra":{"a":[1,2,{"deep":null}]},"cs":"\u001c\u001f","uni":"\u65e5\u672c\u8a9e","big":1234567890123456789012345678901234567890,"nan":NaN,"inf":Infinity,"neg":-Infinity,"actions":{"review":"2026-01-01T00:00:00Z"}}"#,
        ),
        action: "implement",
        depth: None,
        mark: true,
        foreign: false,
    },
    Case {
        name: "malformed",
        state: Some(b"{oops"),
        action: "implement",
        depth: None,
        mark: true,
        foreign: false,
    },
    Case {
        name: "invalid-utf8",
        state: Some(b"{\"k\":\"\xff\xfe\",\"actions\":{}}"),
        action: "implement",
        depth: None,
        mark: true,
        foreign: false,
    },
    Case {
        name: "nonobject",
        state: Some(b"[1,2,3]"),
        action: "implement",
        depth: None,
        mark: true,
        foreign: false,
    },
    Case {
        name: "actions-scalar",
        state: Some(br#"{"version":"9","actions":"nope","keep":"me"}"#),
        action: "implement",
        depth: None,
        mark: true,
        foreign: false,
    },
    Case {
        name: "actions-missing",
        state: Some(br#"{"custom":true}"#),
        action: "implement",
        depth: None,
        mark: true,
        foreign: false,
    },
    Case {
        name: OVERSIZED_INTEGER,
        state: None, // written below: a document holding a 5000-digit integer
        action: "implement",
        depth: None,
        mark: true,
        foreign: false,
    },
    Case {
        name: "loaded-key",
        state: Some(br#"{"schema_version":"1.0.0","actions":{"implement":"2026-01-01T00:00:00Z"}}"#),
        action: "implement",
        depth: None,
        mark: true,
        foreign: false,
    },
    Case {
        name: "wrong-case-key",
        state: Some(br#"{"schema_version":"1.0.0","actions":{"IMPLEMENT":"2026-01-01T00:00:00Z"}}"#),
        action: "implement",
        depth: None,
        mark: true,
        foreign: false,
    },
    Case {
        name: "snapshot-vs-foreign",
        state: Some(br#"{"before":1,"actions":{}}"#),
        action: "implement",
        depth: Some(7),
        mark: true,
        foreign: true,
    },
];

/// The exact bytes of the oversized-integer document.
fn oversized_document() -> Vec<u8> {
    let mut document = String::from("{\"huge\":");
    document.push('7');
    for _ in 0..4999 {
        document.push('3');
    }
    document.push_str(",\"actions\":{}}");
    document.into_bytes()
}

fn build_cases(root: &Path) {
    for case in &CASES {
        let case_root = root.join("cases").join(case.name);
        std::fs::create_dir_all(&case_root).expect("case dir");
        if let Some(state) = case.state {
            let path = state_path(&case_root);
            std::fs::create_dir_all(path.parent().expect("state parent")).expect("state parent");
            std::fs::write(&path, state).expect("state fixture");
        }
        if case.name == OVERSIZED_INTEGER {
            let path = state_path(&case_root);
            std::fs::create_dir_all(path.parent().expect("state parent")).expect("state parent");
            std::fs::write(&path, oversized_document()).expect("state fixture");
        }
    }
}

/// The foreign document written between prepare and mark.
const FOREIGN_DOCUMENT: &[u8] =
    br#"{"foreign":"must be overwritten","actions":{"review":"2030-01-01T00:00:00Z"}}"#;

/// Replace every strict `%Y-%m-%dT%H:%M:%SZ` stamp with `b"<STAMP>"`.
fn normalize_stamps(bytes: &[u8]) -> (Vec<u8>, usize) {
    fn is_stamp(window: &[u8]) -> bool {
        let digits = |range: std::ops::Range<usize>| window[range].iter().all(u8::is_ascii_digit);
        digits(0..4)
            && window[4] == b'-'
            && digits(5..7)
            && window[7] == b'-'
            && digits(8..10)
            && window[10] == b'T'
            && digits(11..13)
            && window[13] == b':'
            && digits(14..16)
            && window[16] == b':'
            && digits(17..19)
            && window[19] == b'Z'
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut stamps = 0;
    let mut index = 0;
    while index < bytes.len() {
        if index + 20 <= bytes.len() && is_stamp(&bytes[index..index + 20]) {
            out.extend_from_slice(b"<STAMP>");
            stamps += 1;
            index += 20;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    (out, stamps)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn from_hex(text: &str) -> Vec<u8> {
    assert_eq!(text.len() % 2, 0, "even hex length");
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).expect("hex pair"))
        .collect()
}

/// The Source child's script: assert the environment, then drive the ACTUAL
/// reference module once per case.
const SOURCE_PROBE: &str = r#"
import json, os, pathlib, sys

root = pathlib.Path(os.environ["SK_CF67_ROOT"])
assert pathlib.Path.cwd() == root, pathlib.Path.cwd()
assert sys.version_info[:3] == (3, 11, 15), sys.version_info
repo = pathlib.Path(os.environ["SK_CF67_SOURCE_ROOT"])
assert repo.is_absolute(), repo
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

from charter.activation.context_state import (
    _load_state, _mark_action_loaded, _prepare_context_state, _write_state,
)

assert callable(_load_state) and callable(_write_state)
assert callable(_mark_action_loaded) and callable(_prepare_context_state)

CASES = json.loads(os.environ["SK_CF67_CASES"])
FOREIGN = json.loads(os.environ["SK_CF67_FOREIGN"])


def state_bytes(case_root):
    path = case_root / ".kittify/charter/context-state.json"
    if not path.exists():
        return None
    return path.read_bytes().hex()


receipt = {"runner": "source-python-" + ".".join(str(part) for part in sys.version_info[:3]),
           "cases": []}
for case in CASES:
    case_root = root / "cases" / case["name"]
    os.chdir(case_root)
    entry = {"case": case["name"], "first_load": None, "effective_depth": None,
             "escaped": None, "state_hex": None}
    try:
        bundle = _prepare_context_state(case_root, case["action"], case["depth"])
        entry["first_load"] = bool(bundle.first_load)
        entry["effective_depth"] = int(bundle.effective_depth)
        if case["mark"] and bundle.first_load:
            if case["foreign"]:
                bundle.state_path.write_bytes(bytes(FOREIGN))
            _mark_action_loaded(bundle.state, bundle.state_path, case["action"])
    except BaseException as exc:  # the reference's own escape is the result
        entry["escaped"] = type(exc).__name__
    entry["state_hex"] = state_bytes(case_root)
    receipt["cases"].append(entry)
print("CF67_SOURCE_RECEIPT " + json.dumps(receipt))
"#;

fn run_source(root: &Path) -> JsonValue {
    let cases = JsonValue::Array(
        CASES
            .iter()
            .map(|case| {
                json!({
                    "name": case.name,
                    "action": case.action,
                    "depth": case.depth,
                    "mark": case.mark,
                    "foreign": case.foreign,
                })
            })
            .collect(),
    );
    let output = isolated_command(root, support::python())
        .arg("-c")
        .arg(SOURCE_PROBE)
        .env("SK_CF67_ROOT", root)
        .env("SK_CF67_CASES", cases.to_string())
        .env("SK_CF67_FOREIGN", json!(FOREIGN_DOCUMENT).to_string())
        .output()
        .expect("run the pinned Source child");
    std::fs::write(root.join("source.stdout"), &output.stdout).expect("source stdout");
    std::fs::write(root.join("source.stderr"), &output.stderr).expect("source stderr");
    assert!(
        output.status.success(),
        "Source child failed ({:?}): {} {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("Source stdout is UTF-8");
    let line = stdout
        .lines()
        .find(|line| line.starts_with("CF67_SOURCE_RECEIPT "))
        .unwrap_or_else(|| panic!("no Source receipt in {stdout:?}"));
    serde_json::from_str(line["CF67_SOURCE_RECEIPT ".len()..].trim()).expect("receipt JSON")
}

/// The Native child: assert isolation, then exercise the new store.
#[test]
fn cf67_native_store_child() {
    let Some(root) = std::env::var_os("SK_CF67_NATIVE_ROOT").map(PathBuf::from) else {
        return; // ordinary run: the parent test drives this child
    };
    assert_roots(&root);

    let mut cases = Vec::new();
    for case in &CASES {
        let case_root = root.join("cases").join(case.name);
        std::env::set_current_dir(&case_root).expect("child cwd");
        let mut entry = json!({
            "case": case.name,
            "first_load": JsonValue::Null,
            "effective_depth": JsonValue::Null,
            "escaped": JsonValue::Null,
            "state_hex": JsonValue::Null,
        });
        match prepare_context_state(&case_root, case.action, case.depth) {
            Ok(mut bundle) => {
                entry["first_load"] = json!(bundle.first_load());
                entry["effective_depth"] = json!(bundle.effective_depth());
                if case.mark && bundle.first_load() {
                    if case.foreign {
                        std::fs::write(state_path(&case_root), FOREIGN_DOCUMENT)
                            .expect("controlled intervening write");
                    }
                    if let Err(error) = bundle.mark_action_loaded(case.action) {
                        entry["escaped"] = json!(error_kind(&error));
                    }
                }
            }
            Err(error) => entry["escaped"] = json!(error_kind(&error)),
        }
        if let Ok(bytes) = std::fs::read(state_path(&case_root)) {
            entry["state_hex"] = json!(hex(&bytes));
        }
        cases.push(entry);
    }
    println!(
        "CF67_NATIVE_RECEIPT {}",
        json!({"runner": "native", "cases": cases})
    );
}

/// The stable name of an escaped failure for receipt comparison.
fn error_kind(error: &ContextStateError) -> &'static str {
    match error {
        ContextStateError::Exists { .. } => "Exists",
        ContextStateError::IntegerDigitLimit { .. } => "IntegerDigitLimit",
        ContextStateError::RecursionBudget { .. } => "RecursionBudget",
        ContextStateError::IntegerDigitLimitSetting { .. } => "IntegerDigitLimitSetting",
        ContextStateError::NumberConversion { .. } => "NumberConversion",
        ContextStateError::Write { .. } => "Write",
    }
}

fn run_native_child(root: &Path) -> JsonValue {
    let executable = std::env::current_exe().expect("test binary");
    let output = isolated_command(root, executable)
        .args(["--exact", "cf67_native_store_child", "--nocapture"])
        .env("SK_CF67_NATIVE_ROOT", root)
        .output()
        .expect("run the Native child");
    std::fs::write(root.join("native.stdout"), &output.stdout).expect("native stdout");
    std::fs::write(root.join("native.stderr"), &output.stderr).expect("native stderr");
    assert!(
        output.status.success(),
        "Native child failed ({:?}): {} {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("Native stdout is UTF-8");
    let line = stdout
        .lines()
        .find(|line| line.starts_with("CF67_NATIVE_RECEIPT "))
        .unwrap_or_else(|| panic!("no Native receipt in {stdout:?}"));
    serde_json::from_str(line["CF67_NATIVE_RECEIPT ".len()..].trim()).expect("receipt JSON")
}

fn by_case<'a>(receipt: &'a JsonValue, name: &str) -> &'a JsonValue {
    receipt["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .find(|case| case["case"] == json!(name))
        .unwrap_or_else(|| panic!("no {name} case"))
}

/// The paired control: both real children, Source first, then every store fact.
#[test]
fn store_phases_match_the_reference() {
    let source_root = fixture("cf67-store-source");
    build_cases(&source_root);
    let source = run_source(&source_root);

    let native_root = fixture("cf67-store-native");
    build_cases(&native_root);
    let native = run_native_child(&native_root);

    let mut failures: Vec<String> = Vec::new();
    for case in &CASES {
        let source_case = by_case(&source, case.name);
        let native_case = by_case(&native, case.name);
        for field in ["first_load", "effective_depth"] {
            if source_case[field] != native_case[field] {
                failures.push(format!(
                    "{}: {field} differs: source={} native={}",
                    case.name, source_case[field], native_case[field]
                ));
            }
        }
        // The reference reports a Python exception class; the Native side reports
        // its own escaped variant. Escaped-versus-absorbed is the comparable fact;
        // the one vocabulary mapping is pinned right below.
        if source_case["escaped"].is_null() != native_case["escaped"].is_null() {
            failures.push(format!(
                "{}: escape phase differs: source={} native={}",
                case.name, source_case["escaped"], native_case["escaped"]
            ));
        }
        if source_case["state_hex"].is_null() != native_case["state_hex"].is_null() {
            failures.push(format!("{}: state-file presence differs", case.name));
            continue;
        }
        if source_case["state_hex"].is_null() {
            continue;
        }
        let source_bytes = from_hex(source_case["state_hex"].as_str().expect("source hex"));
        let native_bytes = from_hex(native_case["state_hex"].as_str().expect("native hex"));
        let (source_norm, source_stamps) = normalize_stamps(&source_bytes);
        let (native_norm, native_stamps) = normalize_stamps(&native_bytes);
        if source_stamps != native_stamps {
            failures.push(format!(
                "{}: stamp count differs: source={source_stamps} native={native_stamps}",
                case.name
            ));
        }
        if source_norm != native_norm {
            failures.push(format!(
                "{}: the state document differs\n  native: {}\n  source: {}",
                case.name,
                String::from_utf8_lossy(&native_norm),
                String::from_utf8_lossy(&source_norm)
            ));
        }
        if !native_norm.is_ascii() {
            failures.push(format!(
                "{}: the written document is ASCII (ensure_ascii=True)",
                case.name
            ));
        }
        if native_norm.last().copied() == Some(b'\n') {
            failures.push(format!(
                "{}: json.dumps writes no trailing newline",
                case.name
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} case(s):\n{}",
        failures.len(),
        failures.join("\n")
    );

    // The ONE Python-exception → Native-variant mapping this lane introduces: the
    // reference's bare `ValueError` from the integer digit ceiling is the escaped
    // `IntegerDigitLimit`, and nothing else escapes in this case set.
    assert_eq!(
        by_case(&source, OVERSIZED_INTEGER)["escaped"],
        json!("ValueError")
    );
    assert_eq!(
        by_case(&native, OVERSIZED_INTEGER)["escaped"],
        json!("IntegerDigitLimit")
    );
    for receipt in [&source, &native] {
        for case in &CASES {
            if case.name == OVERSIZED_INTEGER {
                continue;
            }
            assert_eq!(
                by_case(receipt, case.name)["escaped"],
                JsonValue::Null,
                "{}: no other case escapes",
                case.name
            );
        }
    }
}

/// The reference's own facts for each named store phase, so a both-sides-broken
/// run cannot pass the paired comparison vacuously.
#[test]
fn reference_store_carries_the_authored_data() {
    let source_root = fixture("cf67-store-source-facts");
    build_cases(&source_root);
    let source = run_source(&source_root);

    let document = |name: &str| {
        String::from_utf8(from_hex(
            by_case(&source, name)["state_hex"]
                .as_str()
                .expect("state hex"),
        ))
        .expect("ASCII document")
    };

    let unknown = document("unknown-fields");
    assert!(unknown.contains(r#""extra": {"#), "{unknown}");
    assert!(unknown.contains(r#""cs": "\u001c\u001f""#), "{unknown}");
    assert!(
        unknown.contains(r#""uni": "\u65e5\u672c\u8a9e""#),
        "{unknown}"
    );
    assert!(
        unknown.contains(r#""big": 1234567890123456789012345678901234567890"#),
        "{unknown}"
    );
    assert!(unknown.contains(r#""nan": NaN"#), "{unknown}");
    assert!(unknown.contains(r#""inf": Infinity"#), "{unknown}");
    assert!(unknown.contains(r#""neg": -Infinity"#), "{unknown}");
    // The already-loaded action keeps its authored stamp, and a first-load mark
    // adds exactly the requested one.
    assert!(
        unknown.contains(r#""review": "2026-01-01T00:00:00Z""#),
        "{unknown}"
    );

    let malformed = document("malformed");
    assert_eq!(
        malformed.matches("\"schema_version\"").count(),
        1,
        "{malformed}"
    );
    assert!(
        malformed.contains(r#""schema_version": "1.0.0""#),
        "{malformed}"
    );

    let scalar = document("actions-scalar");
    assert!(scalar.contains(r#""actions": {"#), "{scalar}");
    assert!(scalar.contains(r#""keep": "me""#), "{scalar}");
    assert!(scalar.contains(r#""version": "9""#), "{scalar}");
    // Source `data["actions"] = {}` is dict assignment: the key keeps its
    // position between the members that surround it.
    let actions_at = scalar.find("\"actions\"").expect("actions member");
    let keep_at = scalar.find("\"keep\"").expect("keep member");
    let version_at = scalar.find("\"version\"").expect("version member");
    assert!(actions_at < keep_at && keep_at < version_at, "{scalar}");

    let missing = document("actions-missing");
    assert!(missing.contains(r#""actions": {"#), "{missing}");
    assert!(missing.contains(r#""custom": true"#), "{missing}");

    let utf8 = document("invalid-utf8");
    assert!(!utf8.contains(r#""k""#), "{utf8}");
    assert!(utf8.contains(r#""schema_version": "1.0.0""#), "{utf8}");

    let nonobject = document("nonobject");
    assert_eq!(
        nonobject.matches("\"schema_version\"").count(),
        1,
        "{nonobject}"
    );
    assert!(!nonobject.contains('['), "{nonobject}");

    // The oversized integer escapes, while the malformed document resets.
    assert_eq!(
        by_case(&source, OVERSIZED_INTEGER)["escaped"],
        json!("ValueError"),
        "the digit ceiling is a bare ValueError, not a JSONDecodeError"
    );
    assert_eq!(by_case(&source, "malformed")["escaped"], JsonValue::Null);

    // An exact key match decides the first load; a case-folded one does not.
    assert_eq!(by_case(&source, "loaded-key")["first_load"], json!(false));
    assert_eq!(by_case(&source, "loaded-key")["effective_depth"], json!(1));
    assert_eq!(
        by_case(&source, "wrong-case-key")["first_load"],
        json!(true)
    );
    assert_eq!(
        by_case(&source, "wrong-case-key")["effective_depth"],
        json!(MIN_EFFECTIVE_DEPTH)
    );

    // An explicit depth is used verbatim (no Source cap, no clamp).
    assert_eq!(
        by_case(&source, "snapshot-vs-foreign")["effective_depth"],
        json!(7)
    );
    let snapshot = document("snapshot-vs-foreign");
    assert!(snapshot.contains(r#""before": 1"#), "{snapshot}");
    assert!(
        !snapshot.contains("foreign"),
        "the prepared snapshot is written, not the intervening file: {snapshot}"
    );
    assert!(
        !snapshot.contains("2030-01-01"),
        "the intervening stamp is overwritten: {snapshot}"
    );
}

/// Native-only control: the exact byte shape with an injected clock.
///
/// This is the deterministic half of the write claim — the paired controls above
/// normalise the one dynamic stamp, and this one pins the whole document.
#[test]
fn frozen_clock_write_is_the_reference_byte_shape() {
    let root = fixture("cf67-frozen");
    let path = state_path(&root);
    std::fs::create_dir_all(path.parent().expect("state parent")).expect("state parent");
    std::fs::write(
        &path,
        br#"{"zeta":1,"unknown":{"b":[true,null],"a":"\u65e5"},"actions":"nope","alpha":NaN}"#,
    )
    .expect("fixture state");

    let mut bundle: ContextStateBundle =
        prepare_context_state(&root, "implement", None).expect("prepare");
    assert!(bundle.first_load());
    assert_eq!(bundle.effective_depth(), MIN_EFFECTIVE_DEPTH);
    let frozen = sk_kernel::clock::FrozenClock::new(
        sk_kernel::clock::parse_stamp(
            "2026-01-01T02:03:04Z",
            sk_kernel::clock::UTC_SECOND_TIMESTAMP_FORMAT,
        )
        .expect("frozen stamp"),
    );
    bundle
        .mark_action_loaded_with("implement", &frozen)
        .expect("mark");

    let written = std::fs::read_to_string(&path).expect("written state");
    assert_eq!(
        written,
        concat!(
            "{\n",
            "  \"actions\": {\n",
            "    \"implement\": \"2026-01-01T02:03:04Z\"\n",
            "  },\n",
            "  \"alpha\": NaN,\n",
            "  \"unknown\": {\n",
            "    \"a\": \"\\u65e5\",\n",
            "    \"b\": [\n",
            "      true,\n",
            "      null\n",
            "    ]\n",
            "  },\n",
            "  \"zeta\": 1\n",
            "}"
        ),
        "the exact json.dumps(indent=2, sort_keys=True) bytes"
    );
}

/// Native-only controls for the failures this store must not absorb, and for the
/// two resets it must.
#[test]
fn store_absorbs_only_the_reference_catch_set() {
    let root = fixture("cf67-catch-set");

    // Absent file: the model default, no error.
    let missing = load_context_state(&state_path(&root)).expect("absent");
    assert_eq!(missing, ContextStateValue::default_state());

    // A directory where the document should be: `read_text` raises `OSError`
    // inside the reference's `try`, so it is caught.
    let path = state_path(&root);
    std::fs::create_dir_all(&path).expect("directory at the state path");
    assert_eq!(
        load_context_state(&path).expect("directory is a caught OSError"),
        ContextStateValue::default_state()
    );
    std::fs::remove_dir(&path).expect("remove directory");

    // The integer digit ceiling is a bare ValueError: ESCAPED, not a reset.
    std::fs::write(&path, oversized_document()).expect("oversized document");
    let escaped = load_context_state(&path).expect_err("the digit ceiling escapes");
    assert!(matches!(
        escaped,
        ContextStateError::IntegerDigitLimit { .. }
    ));
    assert!(escaped.is_escaped());
    assert!(!escaped.is_native_boundary());

    // A malformed document is the healthy reset.
    std::fs::write(&path, b"{oops").expect("malformed document");
    assert_eq!(
        load_context_state(&path).expect("malformed resets"),
        ContextStateValue::default_state()
    );

    // A path the probe cannot stat (EACCES is not a Source absence): ESCAPED.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let guarded = root.join("guarded");
        std::fs::create_dir_all(&guarded).expect("guarded directory");
        std::fs::set_permissions(&guarded, std::fs::Permissions::from_mode(0o000))
            .expect("remove search permission");
        let probe = guarded.join("context-state.json");
        let escaped = load_context_state(&probe);
        std::fs::set_permissions(&guarded, std::fs::Permissions::from_mode(0o755))
            .expect("restore search permission");
        let error = escaped.expect_err("EACCES on the existence probe escapes");
        assert!(matches!(error, ContextStateError::Exists { .. }), "{error}");
    }

    // The write phase failure escapes with the writer's own message.
    let blocked = root.join("blocked");
    std::fs::write(&blocked, b"not a directory").expect("blocking file");
    let error = sk_doctrine::write_context_state(
        &blocked.join("context-state.json"),
        &ContextStateValue::default_state(),
    )
    .expect_err("an unusable parent must refuse");
    assert!(matches!(error, ContextStateError::Write { .. }), "{error}");

    // The default document is exactly the reference's two members.
    assert_eq!(
        ContextStateValue::default_state().render(),
        format!(
            "{{\n  \"actions\": {{}},\n  \"schema_version\": \"{CONTEXT_STATE_SCHEMA_VERSION}\"\n}}"
        )
    );
}

/// Native-only control: a mark never mutates a foreign document or re-reads it.
#[test]
fn mark_writes_the_prepared_snapshot_only() {
    let root = fixture("cf67-prepared-snapshot");
    let path = state_path(&root);
    std::fs::create_dir_all(path.parent().expect("state parent")).expect("state parent");
    std::fs::write(&path, br#"{"before":1,"actions":{}}"#).expect("fixture state");

    let mut bundle = prepare_context_state(&root, "implement", None).expect("prepare");
    std::fs::write(&path, FOREIGN_DOCUMENT).expect("controlled intervening write");
    bundle.mark_action_loaded("implement").expect("mark");
    let after_mark = std::fs::read_to_string(&path).expect("written state");
    assert!(after_mark.contains(r#""before": 1"#), "{after_mark}");
    assert!(!after_mark.contains("foreign"), "{after_mark}");

    // A second action is added to the SAME snapshot, keeping the first stamp.
    let mut second = prepare_context_state(&root, "review", None).expect("prepare review");
    assert!(second.first_load(), "review was never loaded");
    second.mark_action_loaded("review").expect("mark review");
    let after_second = std::fs::read_to_string(&path).expect("written state");
    assert!(after_second.contains(r#""implement": ""#), "{after_second}");
    assert!(after_second.contains(r#""review": ""#), "{after_second}");
    assert!(after_second.contains(r#""before": 1"#), "{after_second}");
}
