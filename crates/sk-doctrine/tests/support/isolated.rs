//! E4's isolated Source/Native execution protocol (test-only).
//!
//! Both sides run as **actual re-executed children**, never as in-process calls:
//!
//! * the Source child is the pinned CPython 3.11.15 oracle (selected and
//!   lock-exact-verified by [`crate::support::python`]), which first asserts its
//!   own CWD, HOME, every XDG/TMP root, `PYTHONPATH` and pack root, then resolves
//!   the requested keys;
//! * the Native child is this test binary re-executed with `SK_E4_NATIVE_ROOT`,
//!   which asserts the same roots and resolves the same keys;
//! * results travel as **code-point arrays** (`[ord(c) for c in s]`), so a lone
//!   surrogate, NUL, comma, tab or newline in a declared name survives the
//!   transport exactly. Tab/comma-delimited ids and `from_utf8_lossy` would not;
//! * a non-zero child status is a failure of the run, never a result. The
//!   Source assertion is checked before the Native one in every test.
//!
//! Fixtures are fresh, unique and exclusive (`tempfile::Builder::tempdir_in`)
//! under the repository's `target/` tree so the built-in pack ancestor walk finds
//! `packs/built-in`. They are kept, never cleaned up or reused.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// The isolated roots every child asserts. `(env var, directory name)`.
pub const ROOTS: [(&str, &str); 9] = [
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

/// The pinned interpreter this protocol requires.
pub const PYTHON_VERSION: &str = "3.11.15";

/// One resolved entry, exactly as a child reported it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetView {
    pub key: String,
    pub resolved: bool,
    /// Each id as its exact code points.
    pub ids: Vec<Vec<u32>>,
    /// Each fallback id as its exact code points.
    pub fallback: Vec<Vec<u32>>,
    pub reason: Option<String>,
}

/// One child's complete answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receipt {
    /// The runner identity the child asserted (`source-python-3.11.15` / `native`).
    pub runner: String,
    /// The CWD the child asserted.
    pub cwd: String,
    pub sets: Vec<SetView>,
    pub error: Option<String>,
}

/// The repository root.
pub fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root resolves")
}

/// A fresh, unique, exclusive fixture directory (kept on disk).
pub fn fixture(case: &str) -> PathBuf {
    let base = repository_root().join("target/e4-effective-set");
    std::fs::create_dir_all(&base).expect("fixture base");
    let path = tempfile::Builder::new()
        .prefix(&format!("{case}-"))
        .tempdir_in(&base)
        .expect("fresh exclusive fixture directory")
        .keep();
    for (_, name) in ROOTS {
        std::fs::create_dir_all(path.join(name)).expect("isolated root");
    }
    path
}

/// The Native re-exec marker, when this process *is* the Native child.
pub fn native_root() -> Option<PathBuf> {
    std::env::var_os("SK_E4_NATIVE_ROOT").map(PathBuf::from)
}

/// Assert the isolated roots this child was handed, and that nothing leaked.
pub fn assert_isolated(root: &Path) {
    let cwd = std::env::current_dir().expect("cwd");
    assert_eq!(cwd, root, "Native child CWD is the fixture");
    for (key, name) in ROOTS {
        assert_eq!(
            std::env::var_os(key).map(PathBuf::from),
            Some(root.join(name)),
            "{key} is the isolated fixture root"
        );
    }
    let repository = repository_root();
    assert_eq!(
        std::env::var_os("PYTHONPATH").map(PathBuf::from),
        Some(repository.join("src")),
        "PYTHONPATH is the absolute repository src"
    );
    assert_eq!(
        std::env::var_os("SPEC_KITTY_PACKS_ROOT").map(PathBuf::from),
        Some(repository.join("packs")),
        "pack root is the absolute repository packs"
    );
}

/// The keys the Native child was asked to resolve.
pub fn requested_keys() -> Vec<String> {
    let raw = std::env::var("SK_E4_KEYS").expect("SK_E4_KEYS");
    let value: serde_json::Value = serde_json::from_str(&raw).expect("keys JSON");
    value
        .as_array()
        .expect("keys array")
        .iter()
        .map(|item| item.as_str().expect("key string").to_owned())
        .collect()
}

/// Resolve in the current (Native child) process and render the receipt line.
pub fn native_receipt_json(root: &Path, keys: &[&str]) -> String {
    let receipt = match sk_doctrine::resolve_effective_sets(
        root,
        keys,
        &sk_doctrine::NATIVE_OFFERING_SERVICE,
    ) {
        Ok(sets) => Receipt {
            runner: "native".to_owned(),
            cwd: std::env::current_dir().expect("cwd").display().to_string(),
            sets: sets
                .iter()
                .map(|set| SetView {
                    key: set.yaml_key.clone(),
                    resolved: set.resolved,
                    ids: set.ids.iter().map(|id| id.codepoints().to_vec()).collect(),
                    fallback: set
                        .fallback_ids
                        .iter()
                        .map(|id| id.codepoints().to_vec())
                        .collect(),
                    reason: set.reason.clone(),
                })
                .collect(),
            error: None,
        },
        Err(error) => Receipt {
            runner: "native".to_owned(),
            cwd: std::env::current_dir().expect("cwd").display().to_string(),
            sets: Vec::new(),
            error: Some(error.to_string()),
        },
    };
    render(&receipt)
}

fn render(receipt: &Receipt) -> String {
    let sets: Vec<serde_json::Value> = receipt
        .sets
        .iter()
        .map(|set| {
            serde_json::json!({
                "key": set.key,
                "resolved": set.resolved,
                "ids": set.ids,
                "fallback": set.fallback,
                "reason": set.reason,
            })
        })
        .collect();
    serde_json::json!({
        "runner": receipt.runner,
        "cwd": receipt.cwd,
        "sets": sets,
        "error": receipt.error,
    })
    .to_string()
}

fn parse_receipt(stdout: &str, marker: &str) -> Receipt {
    let line = stdout
        .lines()
        .find(|line| line.starts_with(marker))
        .unwrap_or_else(|| panic!("no {marker} line in child stdout: {stdout:?}"));
    let value: serde_json::Value =
        serde_json::from_str(line[marker.len()..].trim()).expect("receipt JSON");
    let sets = value["sets"]
        .as_array()
        .expect("sets array")
        .iter()
        .map(|set| SetView {
            key: set["key"].as_str().expect("key").to_owned(),
            resolved: set["resolved"].as_bool().expect("resolved"),
            ids: code_points(&set["ids"]),
            fallback: code_points(&set["fallback"]),
            reason: set["reason"].as_str().map(str::to_owned),
        })
        .collect();
    Receipt {
        runner: value["runner"].as_str().expect("runner").to_owned(),
        cwd: value["cwd"].as_str().expect("cwd").to_owned(),
        sets,
        error: value["error"].as_str().map(str::to_owned),
    }
}

fn code_points(value: &serde_json::Value) -> Vec<Vec<u32>> {
    value
        .as_array()
        .expect("code-point list")
        .iter()
        .map(|item| {
            item.as_array()
                .expect("code-point array")
                .iter()
                .map(|point| u32::try_from(point.as_u64().expect("code point")).expect("u32"))
                .collect()
        })
        .collect()
}

const SOURCE_PROBE: &str = r#"
import json, os, pathlib, sys
root = pathlib.Path(os.environ["SK_E4_ROOT"])
cwd = pathlib.Path.cwd()
assert cwd == root, (str(cwd), str(root))
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
from charter.activation.effective_set import resolve_effective_sets
payload = {
    "runner": "source-python-" + ".".join(str(part) for part in sys.version_info[:3]),
    "cwd": str(cwd),
    "sets": [],
    "error": None,
}
try:
    sets = resolve_effective_sets(root, json.loads(os.environ["SK_E4_KEYS"]))
except Exception as exc:  # noqa: BLE001 - the reference's own refusal is the result
    payload["error"] = "%s: %s" % (type(exc).__name__, exc)
else:
    for key, entry in sets.items():
        payload["sets"].append({
            "key": key,
            "resolved": bool(entry.resolved),
            "ids": [[ord(char) for char in text] for text in sorted(entry.ids)],
            "fallback": [[ord(char) for char in text] for text in sorted(entry.fallback_ids)],
            "reason": entry.reason,
        })
print("SOURCE_RECEIPT " + json.dumps(payload))
"#;

fn child_command(root: &Path, program: impl AsRef<std::ffi::OsStr>) -> Command {
    let repository = repository_root();
    let mut command = Command::new(program);
    command
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .current_dir(root)
        .env("SK_E4_ROOT", root)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONPATH", repository.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repository.join("packs"));
    for (key, name) in ROOTS {
        command.env(key, root.join(name));
    }
    command
}

/// Run an arbitrary Source script in an isolated child, asserting status.
///
/// The script must perform its own CWD/HOME/XDG/TMP/PYTHONPATH/packs assertions
/// (the reference probes in this suite do), print one receipt line and exit 0.
pub fn source_script(root: &Path, script: &str, extra_env: &[(&str, &str)]) -> String {
    let mut command = child_command(root, crate::support::python());
    command.arg("-c").arg(script);
    for (key, value) in extra_env {
        command.env(key, value);
    }
    let output = command.output().expect("run the pinned Source script");
    std::fs::write(root.join("source-script.stdout"), &output.stdout).expect("stdout receipt");
    std::fs::write(root.join("source-script.stderr"), &output.stderr).expect("stderr receipt");
    assert!(
        output.status.success(),
        "Source script failed (status {:?}): {} {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("source stdout is UTF-8")
}

/// Run the actual pinned Source for `keys`, asserting status, receipt, version and CWD.
pub fn run_source(root: &Path, keys: &[&str]) -> Receipt {
    let keys = serde_json::to_string(keys).expect("keys JSON");
    let output = child_command(root, crate::support::python())
        .arg("-c")
        .arg(SOURCE_PROBE)
        .env("SK_E4_KEYS", &keys)
        .output()
        .expect("run the pinned Source");
    std::fs::write(root.join("source.stdout"), &output.stdout).expect("source stdout receipt");
    std::fs::write(root.join("source.stderr"), &output.stderr).expect("source stderr receipt");
    assert!(
        output.status.success(),
        "Source child failed (status {:?}): {} {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("source stdout is UTF-8");
    let receipt = parse_receipt(&stdout, "SOURCE_RECEIPT ");
    assert_eq!(
        receipt.runner,
        format!("source-python-{PYTHON_VERSION}"),
        "the Source child is the pinned interpreter"
    );
    assert_eq!(
        receipt.cwd,
        root.display().to_string(),
        "the Source child ran in the fixture"
    );
    receipt
}

/// Run the Native side as an isolated re-exec of this test binary.
pub fn run_native(root: &Path, keys: &[&str], test_name: &str) -> Receipt {
    let keys = serde_json::to_string(keys).expect("keys JSON");
    let executable = std::env::current_exe().expect("test binary");
    let output = child_command(root, executable)
        .args(["--exact", test_name, "--nocapture"])
        .env("SK_E4_NATIVE_ROOT", root)
        .env("SK_E4_KEYS", &keys)
        .output()
        .expect("run the Native child");
    std::fs::write(root.join("native.stdout"), &output.stdout).expect("native stdout receipt");
    std::fs::write(root.join("native.stderr"), &output.stderr).expect("native stderr receipt");
    assert!(
        output.status.success(),
        "Native child failed (status {:?}): {} {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("native stdout is UTF-8");
    let receipt = parse_receipt(&stdout, "NATIVE_RECEIPT ");
    assert_eq!(receipt.runner, "native");
    assert_eq!(
        receipt.cwd,
        root.display().to_string(),
        "the Native child ran in the fixture"
    );
    receipt
}

/// Run Source first, then Native, for the same fixture and keys.
pub fn run_pair(root: &Path, keys: &[&str], test_name: &str) -> (Receipt, Receipt) {
    let source = run_source(root, keys);
    // The intended Source cause is asserted by the caller *before* the Native
    // run; this helper only guarantees the Source run happened first.
    let native = run_native(root, keys, test_name);
    (source, native)
}

/// The receipt entry for `key`, panicking when the child did not report it.
pub fn entry<'a>(receipt: &'a Receipt, key: &str) -> &'a SetView {
    receipt
        .sets
        .iter()
        .find(|set| set.key == key)
        .unwrap_or_else(|| panic!("no receipt entry for {key}: {:?}", receipt))
}

/// Assert one entry is byte-identical between the two children.
pub fn assert_same(source: &SetView, native: &SetView, what: &str) {
    assert_eq!(native.key, source.key, "{what}: key");
    assert_eq!(native.resolved, source.resolved, "{what}: resolved");
    assert_eq!(native.ids, source.ids, "{what}: ids");
    assert_eq!(native.fallback, source.fallback, "{what}: fallback ids");
    assert_eq!(native.reason, source.reason, "{what}: reason");
}

/// Whether the native entry carries `text` (as code points) among its ids.
pub fn has_id(set: &SetView, text: &str) -> bool {
    let points: Vec<u32> = text.chars().map(|character| character as u32).collect();
    set.ids.contains(&points)
}

/// Whether the native entry carries `text` (as code points) among its fallback ids.
pub fn has_fallback(set: &SetView, text: &str) -> bool {
    let points: Vec<u32> = text.chars().map(|character| character as u32).collect();
    set.fallback.contains(&points)
}

/// One child's full answer: the effective sets **and** the service mappings.
///
/// The service half matters on its own: a mapping the set union happens to mask
/// (the scan still lists a file stem the activation gate excluded from the
/// service) must still be right for the callers that read the service directly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FullReceipt {
    pub runner: String,
    pub cwd: String,
    pub sets: Vec<SetView>,
    pub service: std::collections::BTreeMap<String, Vec<Vec<u32>>>,
    /// Per-attribute exceptions the reference's repository property raised.
    pub service_errors: std::collections::BTreeMap<String, String>,
    /// The pass-level exception (`resolve_effective_sets` raised).
    pub error: Option<String>,
}

const FULL_SOURCE_PROBE: &str = r#"
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
    ("TMPDIR", "tmp"), ("TMP", "tmp"), ("TEMP", "tmp"),
]:
    assert pathlib.Path(os.environ[key]) == root / name, (key, os.environ[key])
from charter.activation.effective_set import resolve_effective_sets
from charter.activation.active_charter_service_builder import build_active_charter_service
payload = {
    "runner": "source-python-" + ".".join(str(part) for part in sys.version_info[:3]),
    "cwd": str(pathlib.Path.cwd()),
    "sets": [],
    "service": {},
    "service_errors": {},
    "error": None,
}
try:
    service = build_active_charter_service(root)
except Exception as exc:  # noqa: BLE001
    payload["error"] = "%s: %s" % (type(exc).__name__, exc)
else:
    payload["service_errors"] = {}
    for attribute in json.loads(os.environ["SK_E4_ATTRIBUTES"]):
        try:
            mapping = getattr(service, attribute)
        except Exception as exc:  # noqa: BLE001 - a raising repository property is the answer
            payload["service_errors"][attribute] = "%s: %s" % (type(exc).__name__, exc)
            continue
        payload["service"][attribute] = [[ord(char) for char in key] for key in sorted(mapping.keys())]
    try:
        sets = resolve_effective_sets(root, json.loads(os.environ["SK_E4_KEYS"]))
    except Exception as exc:  # noqa: BLE001 - the reference's own refusal is the result
        payload["error"] = "%s: %s" % (type(exc).__name__, exc)
    else:
        for key, entry in sets.items():
            payload["sets"].append({
                "key": key,
                "resolved": bool(entry.resolved),
                "ids": [[ord(char) for char in text] for text in sorted(entry.ids)],
                "fallback": [[ord(char) for char in text] for text in sorted(entry.fallback_ids)],
                "reason": entry.reason,
            })
print("FULL_RECEIPT " + json.dumps(payload))
"#;

fn parse_full(stdout: &str, marker: &str) -> FullReceipt {
    let line = stdout
        .lines()
        .find(|line| line.starts_with(marker))
        .unwrap_or_else(|| panic!("no {marker} line in child stdout: {stdout:?}"));
    let value: serde_json::Value =
        serde_json::from_str(line[marker.len()..].trim()).expect("receipt JSON");
    let sets = value["sets"]
        .as_array()
        .expect("sets array")
        .iter()
        .map(|set| SetView {
            key: set["key"].as_str().expect("key").to_owned(),
            resolved: set["resolved"].as_bool().expect("resolved"),
            ids: code_points(&set["ids"]),
            fallback: code_points(&set["fallback"]),
            reason: set["reason"].as_str().map(str::to_owned),
        })
        .collect();
    let service = value["service"]
        .as_object()
        .expect("service object")
        .iter()
        .map(|(attribute, keys)| (attribute.clone(), code_points(keys)))
        .collect();
    let service_errors = value["service_errors"]
        .as_object()
        .map(|errors| {
            errors
                .iter()
                .map(|(attribute, message)| {
                    (
                        attribute.clone(),
                        message.as_str().unwrap_or_default().to_owned(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    FullReceipt {
        runner: value["runner"].as_str().expect("runner").to_owned(),
        cwd: value["cwd"].as_str().expect("cwd").to_owned(),
        sets,
        service,
        service_errors,
        error: value["error"].as_str().map(str::to_owned),
    }
}

/// Run the reference's service + effective-set pass in the isolated Source child.
pub fn run_source_full(root: &Path, keys: &[&str], attributes: &[&str]) -> FullReceipt {
    let keys = serde_json::to_string(keys).expect("keys JSON");
    let attributes = serde_json::to_string(attributes).expect("attributes JSON");
    let output = child_command(root, crate::support::python())
        .arg("-c")
        .arg(FULL_SOURCE_PROBE)
        .env("SK_E4_KEYS", &keys)
        .env("SK_E4_ATTRIBUTES", &attributes)
        .output()
        .expect("run the pinned Source");
    std::fs::write(root.join("source.stdout"), &output.stdout).expect("source stdout receipt");
    std::fs::write(root.join("source.stderr"), &output.stderr).expect("source stderr receipt");
    assert!(
        output.status.success(),
        "Source child failed (status {:?}): {} {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("source stdout is UTF-8");
    let receipt = parse_full(&stdout, "FULL_RECEIPT ");
    assert_eq!(
        receipt.runner,
        format!("source-python-{PYTHON_VERSION}"),
        "the Source child is the pinned interpreter"
    );
    assert_eq!(receipt.cwd, root.display().to_string());
    receipt
}

/// The Native side of [`run_source_full`], in an isolated re-exec of this binary.
pub fn run_native_full(
    root: &Path,
    keys: &[&str],
    attributes: &[&str],
    test_name: &str,
) -> FullReceipt {
    let keys = serde_json::to_string(keys).expect("keys JSON");
    let attributes = serde_json::to_string(attributes).expect("attributes JSON");
    let executable = std::env::current_exe().expect("test binary");
    let output = child_command(root, executable)
        .args(["--exact", test_name, "--nocapture"])
        .env("SK_E4_NATIVE_ROOT", root)
        .env("SK_E4_KEYS", &keys)
        .env("SK_E4_ATTRIBUTES", &attributes)
        .output()
        .expect("run the Native child");
    std::fs::write(root.join("native.stdout"), &output.stdout).expect("native stdout receipt");
    std::fs::write(root.join("native.stderr"), &output.stderr).expect("native stderr receipt");
    assert!(
        output.status.success(),
        "Native child failed (status {:?}): {} {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("native stdout is UTF-8");
    let receipt = parse_full(&stdout, "NATIVE_RECEIPT ");
    assert_eq!(receipt.runner, "native");
    assert_eq!(receipt.cwd, root.display().to_string());
    receipt
}

/// The Native child's receipt line for the requested keys and service attributes.
pub fn native_full_json(root: &Path, keys: &[&str], attributes: &[&str]) -> String {
    let sets = match sk_doctrine::resolve_effective_sets(
        root,
        keys,
        &sk_doctrine::NATIVE_OFFERING_SERVICE,
    ) {
        Ok(sets) => sets
            .iter()
            .map(|set| {
                serde_json::json!({
                    "key": set.yaml_key,
                    "resolved": set.resolved,
                    "ids": set.ids.iter().map(|id| id.codepoints().to_vec()).collect::<Vec<_>>(),
                    "fallback": set.fallback_ids.iter().map(|id| id.codepoints().to_vec()).collect::<Vec<_>>(),
                    "reason": set.reason,
                })
            })
            .collect::<Vec<_>>(),
        Err(error) => {
            return serde_json::json!({
                "runner": "native",
                "cwd": std::env::current_dir().expect("cwd").display().to_string(),
                "sets": [],
                "service": {},
                "service_errors": {},
                "error": error.to_string(),
            })
            .to_string();
        }
    };
    let context = match sk_doctrine::OfferingContext::resolve(root) {
        Ok(context) => context,
        Err(error) => {
            return serde_json::json!({
                "runner": "native",
                "cwd": std::env::current_dir().expect("cwd").display().to_string(),
                "sets": sets,
                "service": {},
                "service_errors": {},
                "error": error,
            })
            .to_string();
        }
    };
    let service = sk_doctrine::CanonicalOfferingService::new(context);
    let mut mappings = serde_json::Map::new();
    let mut errors = serde_json::Map::new();
    for attribute in attributes {
        let yaml_key = format!("activated_{attribute}");
        match service.ids_for(&yaml_key) {
            Ok(ids) => {
                mappings.insert(
                    (*attribute).to_owned(),
                    serde_json::json!(
                        ids.iter()
                            .map(|id| id.codepoints().to_vec())
                            .collect::<Vec<_>>()
                    ),
                );
            }
            Err(error) => {
                errors.insert(
                    (*attribute).to_owned(),
                    serde_json::json!(error.to_string()),
                );
            }
        }
    }
    serde_json::json!({
        "runner": "native",
        "cwd": std::env::current_dir().expect("cwd").display().to_string(),
        "sets": sets,
        "service": mappings,
        "service_errors": errors,
        "error": null,
    })
    .to_string()
}

/// The `activated_<kind>` attribute names the Native child was asked to map.
pub fn requested_attributes() -> Vec<String> {
    let raw = std::env::var("SK_E4_ATTRIBUTES").expect("SK_E4_ATTRIBUTES");
    let value: serde_json::Value = serde_json::from_str(&raw).expect("attributes JSON");
    value
        .as_array()
        .expect("attributes array")
        .iter()
        .map(|item| item.as_str().expect("attribute string").to_owned())
        .collect()
}

/// Re-execute this test binary for `test_name` and return its stdout.
///
/// Used by the probes whose Native answer is not an effective-set receipt (the
/// identity domain, the mission-type scan): the child still asserts the same
/// isolated roots before it answers.
pub fn native_reexec(root: &Path, test_name: &str, extra_env: &[(&str, &str)]) -> String {
    let executable = std::env::current_exe().expect("test binary");
    let mut command = child_command(root, executable);
    command
        .args(["--exact", test_name, "--nocapture"])
        .env("SK_E4_NATIVE_ROOT", root);
    for (key, value) in extra_env {
        command.env(key, value);
    }
    let output = command.output().expect("run the Native child");
    std::fs::write(root.join("native.stdout"), &output.stdout).expect("native stdout receipt");
    std::fs::write(root.join("native.stderr"), &output.stderr).expect("native stderr receipt");
    assert!(
        output.status.success(),
        "Native child failed (status {:?}): {} {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("native stdout is UTF-8")
}

/// Parse one `<marker> {json}` receipt line out of a child's stdout.
pub fn receipt_json(stdout: &str, marker: &str) -> serde_json::Value {
    let line = stdout
        .lines()
        .find(|line| line.starts_with(marker))
        .unwrap_or_else(|| panic!("no {marker} line in child stdout: {stdout:?}"));
    serde_json::from_str(line[marker.len()..].trim()).expect("receipt JSON")
}

impl FullReceipt {
    /// The receipt entry for `key`, panicking when the child did not report it.
    pub fn entry<'a>(&'a self, key: &str) -> &'a SetView {
        self.sets
            .iter()
            .find(|set| set.key == key)
            .unwrap_or_else(|| panic!("no receipt entry for {key}: {self:?}"))
    }
}
