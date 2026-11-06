//! CF51 — canonical model-JSON mapping-key projection and the real payload failures.
//!
//! The reference dumps a validated Pydantic model with
//! `model_dump(by_alias=True, mode="json", exclude_none=True)` and then applies
//! `_jsonable_artifact_value`. Inside `structural_lint_config: dict[str, Any]` the
//! nested mappings are **unstructured**, so Pydantic's JSON serializer rewrites
//! their keys (`1` → `"1"`, `True` → `"true"`, `None` → `"None"`, a tuple →
//! `"a,1"`), later duplicates win while keeping the first position, and only a
//! member with no UTF-8 form raises. This file is the Source-first proof of that
//! ONE Kernel codec (`sk_kernel::model_json`) and of the real failure phases the
//! Doctrine include consumer now reports.
//!
//! Every case runs the **actual pinned Source child** first (CPython 3.11.15,
//! lock-exact oracle) and asserts the intended Source cause before the Native
//! child is even started. The Native child is this test binary re-executed by the
//! shared isolated protocol: both children assert their own CWD, HOME, all seven
//! XDG roots (including `XDG_CONFIG_DIRS`/`XDG_DATA_DIRS`), TMPDIR/TMP/TEMP, the
//! absolute `PYTHONPATH`, the pinned packs root and `PYTHONHASHSEED=0`.
//!
//! Not a whole-command/CLI acceptance: the `--json` envelope and the text-mode
//! encoding phase belong to the CLI lane, and the typed-key admission gap for a
//! `dict[str, Any]` root is recorded as debt in the lane report.

mod support;

#[path = "support/isolated.rs"]
mod isolated;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value as JsonValue, json};
use sk_doctrine::PayloadFailure;
use sk_doctrine::context_include::{
    IncludeAccess, IncludeError, IncludeInputs, parse_include_selector, render_include_exact,
};

/// The two XDG roots the shared protocol does not set (it predates them).
const EXTRA_XDG_ROOTS: [(&str, &str); 2] = [
    ("XDG_CONFIG_DIRS", "config-dirs"),
    ("XDG_DATA_DIRS", "data-dirs"),
];

const SELECTOR: &str = "styleguide:common-docs";

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("fixture parent");
    }
    std::fs::write(path, contents).expect("fixture write");
}

/// One project fixture that shadows the real built-in `common-docs` styleguide
/// with an overlay carrying `structural_lint_config`.
fn fixture(case: &str, lint_config: &str) -> PathBuf {
    let root = isolated::fixture(case);
    for (_, name) in EXTRA_XDG_ROOTS {
        std::fs::create_dir_all(root.join(name)).expect("extra isolated XDG directory");
    }
    write(
        &root.join(".kittify/config.yaml"),
        "# authored project config\ncustom_setting: keep-me\n",
    );
    write(
        &root.join(".kittify/charter/charter.md"),
        "# Fixture Charter\n",
    );
    let overlay = format!("id: common-docs\nstructural_lint_config:\n{lint_config}");
    write(
        &root.join(".kittify/charter-packs/styleguide/common-docs.styleguide.yaml"),
        &overlay,
    );
    root
}

/// Every authored byte below `.kittify`, for the no-write assertion.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(directory: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                walk(&path, out);
            } else {
                out.insert(path.clone(), std::fs::read(&path).unwrap_or_default());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(&root.join(".kittify"), &mut out);
    out
}

fn extra_env<'a>(config_dirs: &'a Path, data_dirs: &'a Path) -> Vec<(&'a str, &'a str)> {
    vec![
        ("PYTHONHASHSEED", "0"),
        (
            "XDG_CONFIG_DIRS",
            config_dirs.to_str().expect("config dirs"),
        ),
        ("XDG_DATA_DIRS", data_dirs.to_str().expect("data dirs")),
    ]
}

/// The reference renderer over the real merged repositories: text or refusal.
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
    ("TMPDIR", "tmp"), ("TMP", "tmp"), ("TEMP", "tmp"),
]:
    assert pathlib.Path(os.environ[key]) == root / name, (key, os.environ[key])
assert pathlib.Path(os.environ["XDG_CONFIG_DIRS"]) == root / "config-dirs"
assert pathlib.Path(os.environ["XDG_DATA_DIRS"]) == root / "data-dirs"
assert os.environ["PYTHONHASHSEED"] == "0", os.environ["PYTHONHASHSEED"]

from charter.activation.context import build_charter_context_include

selector = os.environ["SK_CF51_SELECTOR"]
payload = {"runner": "source-python-" + ".".join(str(p) for p in sys.version_info[:3]),
           "cwd": str(pathlib.Path.cwd())}
try:
    text = build_charter_context_include(root, selector)
except Exception as exc:  # noqa: BLE001 - the reference's own refusal is the result
    payload["error"] = "%s: %s" % (type(exc).__name__, exc)
else:
    payload["text"] = [ord(char) for char in text]
print("CF51_RECEIPT " + json.dumps(payload))
"#;

fn parse_receipt(stdout: &str) -> (String, String, Option<Vec<u32>>, Option<String>) {
    let marker = "CF51_RECEIPT ";
    let index = stdout
        .find(marker)
        .unwrap_or_else(|| panic!("no receipt in {stdout:?}"));
    let line = stdout[index + marker.len()..]
        .lines()
        .next()
        .expect("receipt line");
    let value: JsonValue = serde_json::from_str(line.trim()).expect("receipt JSON");
    let text = value.get("text").map(|points| {
        points
            .as_array()
            .expect("code-point array")
            .iter()
            .map(|point| u32::try_from(point.as_u64().expect("code point")).expect("u32"))
            .collect::<Vec<u32>>()
    });
    (
        value["runner"].as_str().expect("runner").to_owned(),
        value["cwd"].as_str().expect("cwd").to_owned(),
        text,
        value
            .get("error")
            .and_then(JsonValue::as_str)
            .map(str::to_owned),
    )
}

/// Run the real Source renderer for one fixture.
fn run_source(root: &Path) -> (Option<Vec<u32>>, Option<String>) {
    let config_dirs = root.join("config-dirs");
    let data_dirs = root.join("data-dirs");
    let mut env = extra_env(&config_dirs, &data_dirs);
    env.push(("SK_CF51_SELECTOR", SELECTOR));
    let stdout = isolated::source_script(root, SOURCE_SCRIPT, &env);
    let (runner, cwd, text, error) = parse_receipt(&stdout);
    assert_eq!(runner, "source-python-3.11.15");
    assert_eq!(cwd, root.display().to_string());
    (text, error)
}

/// The Native child: render the selector exactly, or report the typed refusal.
#[test]
fn cf51_native_child_and_receipt() {
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
    let access = IncludeAccess::load(&root).expect("include access");
    let inputs = IncludeInputs {
        project_root: &root,
        access: &access,
    };
    let selector = std::env::var("SK_CF51_SELECTOR").expect("selector env");
    let parsed = parse_include_selector(&selector).expect("selector parses");
    let mut payload = json!({
        "runner": "native",
        "cwd": std::env::current_dir().expect("cwd").display().to_string(),
    });
    match render_include_exact(&inputs, &parsed, &selector) {
        Ok(text) => payload["text"] = json!(text.codepoints()),
        Err(error) => {
            payload["error"] = json!(error.to_string());
            payload["cause"] = json!(match &error {
                IncludeError::Payload { cause, .. } => Some(cause.to_string()),
                _ => None,
            });
            payload["pending"] = json!(error.is_pending());
        }
    }
    println!("CF51_RECEIPT {}", payload);
}

/// Run the Native child and return `(text, error message, cause, pending)`.
fn run_native(root: &Path) -> (Option<Vec<u32>>, Option<String>, Option<String>, bool) {
    let config_dirs = root.join("config-dirs");
    let data_dirs = root.join("data-dirs");
    let mut env = extra_env(&config_dirs, &data_dirs);
    env.push(("SK_CF51_SELECTOR", SELECTOR));
    let stdout = isolated::native_reexec(root, "cf51_native_child_and_receipt", &env);
    let marker = "CF51_RECEIPT ";
    let index = stdout
        .find(marker)
        .unwrap_or_else(|| panic!("no receipt in {stdout:?}"));
    let line = stdout[index + marker.len()..]
        .lines()
        .next()
        .expect("receipt line");
    let value: JsonValue = serde_json::from_str(line.trim()).expect("receipt JSON");
    assert_eq!(value["runner"], json!("native"));
    assert_eq!(value["cwd"], json!(root.display().to_string()));
    let text = value.get("text").map(|points| {
        points
            .as_array()
            .expect("code-point array")
            .iter()
            .map(|point| u32::try_from(point.as_u64().expect("code point")).expect("u32"))
            .collect::<Vec<u32>>()
    });
    (
        text,
        value
            .get("error")
            .and_then(JsonValue::as_str)
            .map(str::to_owned),
        value
            .get("cause")
            .and_then(JsonValue::as_str)
            .map(str::to_owned),
        value
            .get("pending")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false),
    )
}

/// The key corpus a real authored styleguide can carry.
///
/// `structural_lint_config` is the ONE `Any`-valued model field, so its members'
/// values are the unstructured domain where Pydantic's JSON key codec applies.
const KEY_CORPUS: &str = r#"  ints:
    1: int_value
    -7: negative_int
    123456789012345678901234567890: big_int
  bools:
    true: bool_true
    false: bool_false
  floats:
    1.5: float_value
    -0.0: negative_zero
    1e20: exponent_float
  nonfinite_none:
    ~: null_key
    .nan: nan_key
    .inf: infinity_key
  bytes:
    !!binary eHk=: bytes_key
  timestamps:
    2020-01-02: date_key
    2020-01-02T03:04:05Z: datetime_key
  tuples:
    ? [a, 1]
    : tuple_key
  text:
    'café': non_ascii_text
  collision:
    1: int_first
    '1': text_second
"#;

/// The paired proof: the Source child renders first, the Native child must
/// reproduce the whole text as code points.
#[test]
fn nested_unstructured_mapping_keys_project_like_the_source_child() {
    let root = fixture("cf51-keys", KEY_CORPUS);
    let before = snapshot(&root);

    let (source_text, source_error) = run_source(&root);
    let source_text = source_text
        .unwrap_or_else(|| panic!("the Source child refused the key corpus: {source_error:?}"));

    // The intended Source cause is asserted BEFORE the Native child runs.
    let text = String::from_utf16(
        &source_text
            .iter()
            .map(|point| u16::try_from(*point).expect("bmp text"))
            .collect::<Vec<u16>>(),
    )
    .expect("source text decodes");
    for expected in [
        // Non-text scalar keys, in Pydantic's JSON spelling.
        "\"1\": \"int_value\"",
        "\"-7\": \"negative_int\"",
        "\"123456789012345678901234567890\": \"big_int\"",
        "\"true\": \"bool_true\"",
        "\"false\": \"bool_false\"",
        "\"1.5\": \"float_value\"",
        "\"-0.0\": \"negative_zero\"",
        "\"1e+20\": \"exponent_float\"",
        // bytes decode, tuples join with `,`, timestamps use the model-JSON form.
        "\"xy\": \"bytes_key\"",
        "\"2020-01-02\": \"date_key\"",
        "\"2020-01-02T03:04:05Z\": \"datetime_key\"",
        "\"a,1\": \"tuple_key\"",
        // `ensure_ascii=True` escapes non-ASCII exactly like the reference.
        "\"caf\\u00e9\": \"non_ascii_text\"",
        // A collision keeps the first position and takes the last value.
        "\"1\": \"text_second\"",
    ] {
        assert!(
            text.contains(expected),
            "the Source child did not emit {expected:?}:\n{text}"
        );
    }
    // `1` (int) and `'1'` (text) collapse to ONE `"1"` key: the later value wins.
    assert!(
        !text.contains("\"int_first\""),
        "the later duplicate key wins in the Source child"
    );
    // `~`, `.nan` and `.inf` all project to `None`, so they collide too; the last
    // authored one wins and the earlier spellings are gone.
    assert!(
        text.contains("\"None\": \"infinity_key\""),
        "a non-finite float spells `None` exactly like a null key:\n{text}"
    );
    assert!(!text.contains("\"null_key\""), "the None key collided");
    assert!(!text.contains("\"nan_key\""), "a NaN key spells `None`");
    let (native_text, native_error, _, pending) = run_native(&root);
    assert!(!pending, "not a Native capability gap: {native_error:?}");
    let native_text =
        native_text.unwrap_or_else(|| panic!("the Native child refused: {native_error:?}"));
    assert_eq!(
        native_text, source_text,
        "the whole rendered text (code points) matches the Source child"
    );

    // The read is a pure projection: no authored byte changed.
    assert_eq!(before, snapshot(&root), "the include path wrote no file");
}

/// A genuine reference payload failure: `!!binary` bytes that are not UTF-8.
///
/// The reference's `model_dump(mode="json")` raises `UnicodeDecodeError` there; the
/// include must report exactly that phase, never "missing" and never a Native gap.
#[test]
fn a_non_utf8_binary_key_is_a_real_payload_failure() {
    let root = fixture(
        "cf51-binary-key",
        "  keys:\n    ? !!binary /w==\n    : value\n",
    );
    let before = snapshot(&root);

    let (source_text, source_error) = run_source(&root);
    assert!(source_text.is_none(), "the Source child rendered a payload");
    let source_error = source_error.expect("the Source child refused");
    assert!(
        source_error.starts_with("UnicodeDecodeError: "),
        "{source_error}"
    );
    assert!(
        source_error.contains("can't decode byte 0xff in position 0: invalid utf-8"),
        "{source_error}"
    );

    let (native_text, native_error, cause, pending) = run_native(&root);
    assert!(native_text.is_none());
    assert!(!pending, "a real payload failure is not a Native gap");
    let cause = cause.expect("a typed payload cause");
    assert!(
        matches!(cause_kind(&cause), Some(CauseKind::BytesNotUtf8)),
        "expected the bytes phase, got {cause}"
    );
    let message = native_error.expect("message");
    assert!(
        message.contains("can't decode byte 0xff in position 0: invalid utf-8"),
        "{message}"
    );
    assert!(
        !message.contains("not a missing selector") || message.contains("payload-serialization"),
        "the message names the payload phase: {message}"
    );
    assert_eq!(before, snapshot(&root), "the refusal wrote no file");
}

/// The same binary-refusal phase for a *value* member (the other position the
/// reference's `mode="json"` decodes).
#[test]
fn a_non_utf8_binary_value_is_the_same_payload_phase() {
    let root = fixture("cf51-binary-value", "  bytes_value: !!binary /w==\n");
    let (source_text, source_error) = run_source(&root);
    assert!(source_text.is_none(), "the Source child rendered a payload");
    let source_error = source_error.expect("the Source child refused");
    assert!(
        source_error.starts_with("UnicodeDecodeError: "),
        "{source_error}"
    );

    let (native_text, native_error, cause, pending) = run_native(&root);
    assert!(native_text.is_none());
    assert!(!pending);
    assert!(matches!(
        cause_kind(&cause.expect("cause")),
        Some(CauseKind::BytesNotUtf8)
    ));
    assert!(
        native_error
            .expect("message")
            .contains("can't decode byte 0xff")
    );
}

/// A lone surrogate in a **key** is a real `UnicodeEncodeError` in the reference
/// (the same surrogate survives in a *value*), so the include reports the key
/// phase, not a generic refusal.
#[test]
fn an_unpaired_surrogate_key_is_the_encode_phase() {
    let root = fixture("cf51-surrogate-key", "  keys:\n    \"\\ud800\": value\n");
    let (source_text, source_error) = run_source(&root);
    assert!(source_text.is_none(), "the Source child rendered a payload");
    let source_error = source_error.expect("the Source child refused");
    assert!(
        source_error.starts_with("UnicodeEncodeError: "),
        "{source_error}"
    );
    assert!(
        source_error
            .contains("can't encode character '\\ud800' in position 0: surrogates not allowed"),
        "{source_error}"
    );

    let (native_text, native_error, cause, pending) = run_native(&root);
    assert!(native_text.is_none());
    assert!(!pending);
    assert!(matches!(
        cause_kind(&cause.expect("cause")),
        Some(CauseKind::KeyNotUtf8)
    ));
    assert!(
        native_error
            .expect("message")
            .contains("surrogates not allowed")
    );
}

/// The exact typed cause the include reported, from its Display text.
#[derive(Debug, Eq, PartialEq)]
enum CauseKind {
    BytesNotUtf8,
    KeyNotUtf8,
}

fn cause_kind(cause: &str) -> Option<CauseKind> {
    if cause.contains("can't decode byte") {
        Some(CauseKind::BytesNotUtf8)
    } else if cause.contains("surrogates not allowed") {
        Some(CauseKind::KeyNotUtf8)
    } else {
        None
    }
}

/// A typed `dict[str, Any]` root key is model *validation*, not the key codec.
///
/// `structural_lint_config: {1: v}` makes the reference's model validation fail
/// (`ValidationError`), so the reference never has a model to dump. This crate's
/// typed admission keeps the same refusal and names the phase honestly; turning it
/// into the reference's "document not admitted" skip is the admission owner's work
/// (recorded as debt, not claimed here).
#[test]
fn a_typed_root_key_stays_an_admission_refusal() {
    use sk_charter::ArtifactKind;
    use sk_doctrine::{PayloadError, exact_payload};
    use sk_kernel::python_yaml::{Dialect, GraphDocument, decode_graph};

    let base = "id: s\nschema_version: '1.0'\ntitle: T\nscope: code\nprinciples: [p]\n";
    let document = GraphDocument::new(
        decode_graph(
            &format!("{base}structural_lint_config:\n  1: v\n"),
            Dialect::RuamelSafe12,
        )
        .expect("document constructs"),
    );
    assert_eq!(
        exact_payload(ArtifactKind::Styleguide, document.root()).expect_err("typed root key"),
        PayloadError::Failed(PayloadFailure::TypedKeyNotText {
            python_type_name: "int"
        })
    );

    // One level deeper the same key is unstructured, so it is projected instead.
    let document = GraphDocument::new(
        decode_graph(
            &format!("{base}structural_lint_config:\n  k:\n    1: v\n"),
            Dialect::RuamelSafe12,
        )
        .expect("document constructs"),
    );
    let payload =
        exact_payload(ArtifactKind::Styleguide, document.root()).expect("the payload renders");
    let lint = payload
        .field("structural_lint_config")
        .expect("structural_lint_config");
    assert_eq!(
        lint.field("k")
            .and_then(|mapping| mapping.field("1"))
            .and_then(|value| value.to_serde())
            .and_then(|value| value.as_str().map(str::to_owned)),
        Some("v".to_owned())
    );
}

/// Pydantic builds a **new** dict from the projected keys, so a collision keeps the
/// first position and takes the last value — Python's own `dict` semantics. The
/// `Full artifact:` block sorts its keys, so this is asserted on the payload's
/// object rather than on its rendered text.
#[test]
fn a_projected_mapping_keeps_python_key_positions_and_last_values() {
    use sk_charter::ArtifactKind;
    use sk_doctrine::exact_payload;
    use sk_kernel::python_json::PythonJsonValue;
    use sk_kernel::python_yaml::{Dialect, GraphDocument, decode_graph};

    let base = "id: s\nschema_version: '1.0'\ntitle: T\nscope: code\nprinciples: [p]\n";
    // Authored key order: 'a', 2, 1, 'b', '1'. The int `1` and the text `'1'` are
    // distinct Python keys that both project to `"1"`, so the later value wins at
    // the earlier key's position.
    let document = GraphDocument::new(
        decode_graph(
            &format!(
                "{base}structural_lint_config:\n  k:\n    'a': 1\n    2: two\n    1: int_first\n    'b': 2\n    '1': text_second\n"
            ),
            Dialect::RuamelSafe12,
        )
        .expect("document constructs"),
    );
    let payload =
        exact_payload(ArtifactKind::Styleguide, document.root()).expect("the payload renders");
    let lint = payload
        .field("structural_lint_config")
        .expect("structural_lint_config")
        .field("k")
        .expect("k");
    let PythonJsonValue::ExactObject(object) = lint else {
        panic!("a mapping projects to an exact object: {lint:?}");
    };
    let entries: Vec<(String, String)> = object
        .entries()
        .iter()
        .map(|(key, value)| {
            (
                key.to_scalar_string().expect("scalar key"),
                value
                    .to_serde()
                    .expect("scalar value")
                    .to_string()
                    .trim_matches('"')
                    .to_owned(),
            )
        })
        .collect();
    assert_eq!(
        entries,
        vec![
            ("a".to_owned(), "1".to_owned()),
            ("2".to_owned(), "two".to_owned()),
            ("1".to_owned(), "text_second".to_owned()),
            ("b".to_owned(), "2".to_owned()),
        ]
    );
}
