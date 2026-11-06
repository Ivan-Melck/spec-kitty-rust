//! CF67 — the canonical Source context-state file through the real
//! `charter context` handler.
//!
//! Source keeps `<repo>/.kittify/charter/context-state.json` in memory as the
//! exact `json.loads` result and writes it back with
//! `json.dumps(indent=2, sort_keys=True)` (default `ensure_ascii`/`allow_nan`)
//! through `kernel.atomic.atomic_write(mkdir=True)`. The first stage's prepared
//! snapshot is what the mark persists — never a second disk read.
//!
//! This file drives the **actual Source CLI handler** and the **same execution
//! facade the Native CLI calls** ([`sk_cli::compat::charter::execute_context`])
//! as two actual children, each asserting its own temporary CWD, HOME, all seven
//! XDG roots, TMPDIR/TMP/TEMP, the absolute Source/packs roots and the hash seed
//! before any read.
//!
//! Named controls:
//!
//! * `exact-unknown` — unknown/nested keys, a JSON `ensure_ascii` payload, an
//!   arbitrary-magnitude integer and a float all survive mark/write;
//! * `nonfinite` — `NaN`/`Infinity`/`-Infinity` survive (a serde reader cannot
//!   even decode them, so it would destroy the whole document);
//! * `surrogate` — an escaped lone surrogate key and value survive;
//! * `malformed` — an undecodable document falls back to the model default;
//! * `actions-scalar` — a non-dict `actions` becomes `{}` while the other keys stay;
//! * `invalid-utf8` — an invalid-UTF-8 body falls back to the model default;
//! * `structured-failure` — the first-stage exact write survives a later
//!   structured-stage failure.
//!
//! Not whole-command parity, not approval, not performance.

#[path = "cf65_support/mod.rs"]
mod support;

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value as JsonValue, json};

const CHARTER_BODY: &str = "# CF67 state Charter\n\n## Policy Summary\n\n- Keep one canonical authority.\n\n## Terminology Canon\n\nUse Mission, never Feature.\n";

/// A member the JSON-mode dump refuses, so the structured stage fails.
const LAZY_STYLEGUIDE: &str = "\
schema_version: \"1.0\"
id: common-docs
title: \"CF67 lazy docs title\"
scope: docs
principles:
  - overlay principle
structural_lint_config:
  bytes_value: !!binary /w==
";

const PROJECT_GRAPH: &str = "\
schema_version: '1.0'
generated_at: STATIC
generated_by: cf67-cli-test
nodes:
- urn: styleguide:common-docs
  kind: styleguide
  label: Common Docs
edges:
- source: action:software-dev/implement
  target: styleguide:common-docs
  relation: suggests
";

/// One case's inputs. `state` is the exact authored `context-state.json` body
/// (`None` means no file at all).
#[derive(Clone, Copy)]
struct Case {
    name: &'static str,
    state: Option<&'static [u8]>,
    json: bool,
    stale: bool,
    invalid_body: bool,
}

const CASES: [Case; 7] = [
    Case {
        name: "exact-unknown",
        state: Some(
            br#"{"schema_version":"1.0.0","custom":{"nested":[1,2,{"x":null}]},"unicode":"caf\u00e9 \u001c sep","big":1234567890123456789012345678901234567890,"ratio":1.5,"actions":{}}"#,
        ),
        json: false,
        stale: false,
        invalid_body: false,
    },
    Case {
        name: "nonfinite",
        state: Some(
            br#"{"schema_version":"1.0.0","nan":NaN,"pos":Infinity,"neg":-Infinity,"actions":{}}"#,
        ),
        json: false,
        stale: false,
        invalid_body: false,
    },
    Case {
        name: "surrogate",
        state: Some(br#"{"schema_version":"1.0.0","\ud800key":"\udc80","actions":{}}"#),
        json: false,
        stale: false,
        invalid_body: false,
    },
    Case {
        name: "malformed",
        state: Some(b"{oops"),
        json: false,
        stale: false,
        invalid_body: false,
    },
    Case {
        name: "actions-scalar",
        state: Some(br#"{"version":"9","actions":"nope","keep":"me"}"#),
        json: false,
        stale: false,
        invalid_body: false,
    },
    Case {
        name: "invalid-utf8",
        state: Some(b"{\"k\":\"\xff\xfe\",\"actions\":{}}"),
        json: false,
        stale: false,
        invalid_body: false,
    },
    Case {
        name: "structured-failure",
        state: Some(
            br#"{"schema_version":"1.0.0","\ud800keep":-Infinity,"big":-123456789012345678901234567890,"actions":{}}"#,
        ),
        json: true,
        stale: true,
        invalid_body: true,
    },
];

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("fixture parent");
    }
    std::fs::write(path, contents).expect("fixture write");
}

fn git_init(root: &Path) {
    let status = Command::new("git")
        .arg("init")
        .arg("-q")
        .arg("-b")
        .arg("main")
        .arg(root)
        .status()
        .expect("git is available");
    assert!(status.success(), "git init {root:?} failed");
}

fn build_case(root: &Path, case: &Case) {
    std::fs::create_dir_all(root).expect("case dir");
    git_init(root);
    write(
        &root.join(".kittify/config.yaml"),
        "# authored project config\ncustom_setting: keep-me\n",
    );
    write(
        &root.join(".kittify/charter/charter.yaml"),
        "schema_version: \"1.1.0\"\ncatalog:\n  references: []\ngovernance:\n  authority_paths: [docs/context]\n",
    );
    write(&root.join(".kittify/charter/charter.md"), CHARTER_BODY);
    if !case.stale {
        write(
            &root.join(".kittify/charter/metadata.yaml"),
            &format!(
                "charter_hash: {}\ntimestamp_utc: '2026-01-01T00:00:00+00:00'\n",
                sk_charter::hash_content(CHARTER_BODY)
            ),
        );
    }
    write(
        &root.join(".kittify/charter-packs/styleguide/common-docs.styleguide.yaml"),
        LAZY_STYLEGUIDE,
    );
    write(
        &root.join(".kittify/charter-packs/cf67.graph.yaml"),
        PROJECT_GRAPH,
    );
    if let Some(state) = case.state {
        let path = root.join(".kittify/charter/context-state.json");
        std::fs::create_dir_all(path.parent().expect("state parent")).expect("state parent");
        std::fs::write(&path, state).expect("state fixture");
    }
}

fn build_cases(root: &Path) {
    for case in &CASES {
        build_case(&root.join("cases").join(case.name), case);
    }
}

/// Replace every strict `%Y-%m-%dT%H:%M:%SZ` stamp with `b"<STAMP>"`.
///
/// The two children necessarily run at different wall-clock instants, so the one
/// dynamic value the document carries is normalised by this ONE parent-side rule
/// and nothing else is. `stamps` counts the replacements, so a case that lost its
/// stamp cannot pass by accident.
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

/// The Source child's script: assert the environment, then drive the ACTUAL
/// `charter context` CLI handler once per case.
const SOURCE_PROBE: &str = r#"
import contextlib, io, json, os, pathlib, sys

root = pathlib.Path(os.environ["SK_CF67_ROOT"])
assert pathlib.Path.cwd() == root, pathlib.Path.cwd()
assert sys.version_info[:3] == (3, 11, 15), sys.version_info
repo = pathlib.Path(os.environ["SK_CF65_SOURCE_ROOT"])
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

from charter.activation import context_state as state_module

# The reference authority this lane ports must be the real one.
assert callable(state_module._load_state)
assert callable(state_module._write_state)
assert callable(state_module._mark_action_loaded)
assert callable(state_module._prepare_context_state)

from specify_cli.cli.commands.charter.context import context as context_command

CASES = json.loads(os.environ["SK_CF67_CASES"])


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
    stdout, stderr = io.StringIO(), io.StringIO()
    error = None
    code = 0
    try:
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            context_command(
                owned_checkout=None, mission=None, action="implement", include=None,
                mark_loaded=True, profile=None, mission_type="software-dev",
                json_output=case["json"], include_all=case["invalid_body"],
            )
    except SystemExit as exc:
        code = exc.code if isinstance(exc.code, int) else 1
    except BaseException as exc:  # the reference's own escape is the result
        code = 1
        error = type(exc).__name__ + ": " + str(exc)
    receipt["cases"].append({
        "case": case["name"],
        "ok": code == 0 and error is None,
        "error": error,
        "stdout_empty": stdout.getvalue() == "",
        "state_hex": state_bytes(case_root),
    })
print("CF67_SOURCE_RECEIPT " + json.dumps(receipt))
"#;

fn run_source(root: &Path) -> JsonValue {
    let cases = JsonValue::Array(
        CASES
            .iter()
            .map(|case| {
                json!({
                    "name": case.name,
                    "json": case.json,
                    "invalid_body": case.invalid_body,
                })
            })
            .collect(),
    );
    let output = support::isolated_command(root, support::python())
        .arg("-c")
        .arg(SOURCE_PROBE)
        .env("SK_CF67_ROOT", root)
        .env("SK_CF67_CASES", cases.to_string())
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

/// The Native child: assert isolation, then call the SAME facade the real CLI
/// command calls.
#[test]
fn cf67_native_state_child() {
    let Some(root) = std::env::var_os("SK_CF67_NATIVE_ROOT").map(PathBuf::from) else {
        return; // ordinary run: the parent test drives this child
    };
    support::assert_isolated(&root);

    let mut cases = Vec::new();
    for case in &CASES {
        let case_root = root.join("cases").join(case.name);
        std::env::set_current_dir(&case_root).expect("child cwd");
        assert_eq!(std::env::current_dir().expect("cwd"), case_root);
        let result = sk_cli::compat::charter::execute_context(
            &case_root,
            "implement",
            Some("software-dev"),
            None,
            case.invalid_body,
            case.json,
            true,
        );
        let state_path = case_root.join(".kittify/charter/context-state.json");
        let state_hex = std::fs::read(&state_path).ok().map(|bytes| {
            bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        });
        let mut entry = json!({
            "case": case.name,
            "ok": true,
            "error": JsonValue::Null,
            "state_hex": state_hex,
        });
        if let Err(error) = result {
            entry["ok"] = json!(false);
            entry["error"] = json!(error.message());
        }
        cases.push(entry);
    }
    println!(
        "CF67_NATIVE_RECEIPT {}",
        json!({"runner": "native", "cases": cases})
    );
}

fn run_native_child(root: &Path) -> JsonValue {
    let executable = std::env::current_exe().expect("test binary");
    let output = support::isolated_command(root, executable)
        .args(["--exact", "cf67_native_state_child", "--nocapture"])
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

fn hex_bytes(value: &JsonValue) -> Option<Vec<u8>> {
    let text = value.as_str()?;
    assert_eq!(text.len() % 2, 0, "even hex length");
    Some(
        (0..text.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&text[index..index + 2], 16).expect("hex pair"))
            .collect(),
    )
}

/// The one paired control: both real children, Source first, then the exact
/// state document each produced.
#[test]
fn first_stage_state_document_matches_the_reference() {
    let source_root = support::fixture("cf67-cli-source");
    build_cases(&source_root);
    let source = run_source(&source_root);

    let native_root = support::fixture("cf67-cli-native");
    build_cases(&native_root);
    let native = run_native_child(&native_root);

    let mut failures: Vec<String> = Vec::new();
    for case in &CASES {
        let source_case = by_case(&source, case.name);
        let native_case = by_case(&native, case.name);
        if source_case["ok"] != native_case["ok"] {
            failures.push(format!(
                "{}: command outcome differs: source={source_case} native={native_case}",
                case.name
            ));
            continue;
        }

        let source_bytes = hex_bytes(&source_case["state_hex"]);
        let native_bytes = hex_bytes(&native_case["state_hex"]);
        if source_bytes.is_some() != native_bytes.is_some() {
            failures.push(format!("{}: state file presence differs", case.name));
            continue;
        }
        let (Some(source_bytes), Some(native_bytes)) = (source_bytes, native_bytes) else {
            continue; // a case that legitimately leaves no state file
        };

        let (source_norm, source_stamps) = normalize_stamps(&source_bytes);
        let (native_norm, native_stamps) = normalize_stamps(&native_bytes);
        if source_stamps != 1 {
            failures.push(format!(
                "{}: Source stamps only the action, saw {source_stamps}",
                case.name
            ));
        }
        if native_stamps != 1 {
            failures.push(format!(
                "{}: Native stamps only the action, saw {native_stamps}",
                case.name
            ));
        }
        if native_norm != source_norm {
            failures.push(format!(
                "{}: the written state document differs from the reference\n  native: {}\n  source: {}",
                case.name,
                String::from_utf8_lossy(&native_norm),
                String::from_utf8_lossy(&source_norm)
            ));
        }
        // The reference's writer shape: `ensure_ascii=True` bytes, two-space
        // indent, and no trailing newline.
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
}

/// The named per-case expectations, so a both-sides-broken run cannot pass.
#[test]
fn reference_state_document_carries_the_authored_data() {
    let source_root = support::fixture("cf67-cli-source-carry");
    build_cases(&source_root);
    let source = run_source(&source_root);

    let exact = String::from_utf8(
        hex_bytes(&by_case(&source, "exact-unknown")["state_hex"]).expect("exact state"),
    )
    .expect("ASCII document");
    assert!(exact.contains(r#""custom": {"#), "{exact}");
    assert!(
        exact.contains(r#""unicode": "caf\u00e9 \u001c sep""#),
        "{exact}"
    );
    assert!(
        exact.contains(r#""big": 1234567890123456789012345678901234567890"#),
        "{exact}"
    );
    assert!(exact.contains(r#""ratio": 1.5"#), "{exact}");

    let nonfinite = String::from_utf8(
        hex_bytes(&by_case(&source, "nonfinite")["state_hex"]).expect("nonfinite state"),
    )
    .expect("ASCII document");
    assert!(nonfinite.contains(r#""nan": NaN"#), "{nonfinite}");
    assert!(nonfinite.contains(r#""pos": Infinity"#), "{nonfinite}");
    assert!(nonfinite.contains(r#""neg": -Infinity"#), "{nonfinite}");

    let surrogate = String::from_utf8(
        hex_bytes(&by_case(&source, "surrogate")["state_hex"]).expect("surrogate state"),
    )
    .expect("ASCII document");
    assert!(
        surrogate.contains(r#""\ud800key": "\udc80""#),
        "{surrogate}"
    );

    let malformed = String::from_utf8(
        hex_bytes(&by_case(&source, "malformed")["state_hex"]).expect("malformed state"),
    )
    .expect("ASCII document");
    assert_eq!(
        malformed.matches("\"schema_version\"").count(),
        1,
        "{malformed}"
    );
    assert!(
        malformed.contains(r#""schema_version": "1.0.0""#),
        "{malformed}"
    );

    let scalar = String::from_utf8(
        hex_bytes(&by_case(&source, "actions-scalar")["state_hex"]).expect("scalar state"),
    )
    .expect("ASCII document");
    assert!(scalar.contains(r#""actions": {"#), "{scalar}");
    assert!(scalar.contains(r#""keep": "me""#), "{scalar}");

    let utf8 = String::from_utf8(
        hex_bytes(&by_case(&source, "invalid-utf8")["state_hex"]).expect("utf8 state"),
    )
    .expect("ASCII document");
    assert!(!utf8.contains(r#""k""#), "{utf8}");
    assert!(utf8.contains(r#""schema_version": "1.0.0""#), "{utf8}");
}
