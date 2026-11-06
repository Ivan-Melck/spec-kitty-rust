//! CF72 — the compact rail's live governance summary, paired Source/Native.
//!
//! The pre-change Control run for this file's summary projection is the actual
//! old Native binary (`target/debug/spec-kitty`) failing on the same fixture:
//! `(none)` template/paradigms/tools with no resolver directives or diagnostics
//! while the pinned Source prints the resolved values. This test drives the
//! actual Source CLI handler and the same Native facade the CLI calls as two
//! isolated children (CWD, HOME, all seven XDG roots, TMPDIR/TMP/TEMP,
//! absolute Source+packs roots and the hash seed asserted before any read).
//!
//! Not whole-text parity: only the governance summary projection (template set,
//! paradigms, tools, directive/tactic ids and diagnostics) is compared.

#[path = "cf65_support/mod.rs"]
mod support;

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value as JsonValue, json};

const CHARTER: &str = "\
schema_version: \"1.1.0\"
governance:
  charter:
    selected_paradigms: [domain-driven-design, deep-module-design, no-such-paradigm]
    selected_directives: [DIRECTIVE_010, no-such-directive]
    available_tools: [pytest, mypy, ruff]
    template_set: software-dev-default
directives:
  directives:
    - id: PROJ_001
      title: Project local rule
";
const ACTIVATION: &str = "\
activated_directives: [DIRECTIVE_024]
deactivated_directives: [DIRECTIVE_010]
";
const MARKDOWN: &str =
    "# CF72 compact charter\n\n## Policy Summary\n\n- One canonical authority.\n";

const ACTIONS: [&str; 3] = ["consolidate", "implement", "implement"];

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("fixture parent");
    }
    std::fs::write(path, contents).expect("write fixture");
}

fn build_project(root: &Path) {
    let project = root.join("project");
    write(
        &project.join(".kittify/config.yaml"),
        "custom_setting: keep-me\n",
    );
    write(&project.join(".kittify/charter/charter.yaml"), CHARTER);
    write(
        &project.join(".kittify/charter/activation.yaml"),
        ACTIVATION,
    );
    write(&project.join(".kittify/charter/charter.md"), MARKDOWN);
    let status = Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .arg(&project)
        .status()
        .expect("git init");
    assert!(status.success(), "git init failed");
}

/// The Source child: assert the isolated environment, then drive the ACTUAL
/// `charter context` CLI handler once per action.
const SOURCE_PROBE: &str = r#"
import contextlib, io, json, os, pathlib, sys
root = pathlib.Path(os.environ["SK_CF72_ROOT"])
assert pathlib.Path.cwd() == root, pathlib.Path.cwd()
assert sys.version_info[:3] == (3, 11, 15), sys.version_info
repo = pathlib.Path(os.environ["SK_CF65_SOURCE_ROOT"])
assert repo.is_absolute()
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
assert sys.getfilesystemencoding().lower() in ("utf-8", "utf8"), sys.getfilesystemencoding()
assert sys.getfilesystemencodeerrors() == "surrogateescape", sys.getfilesystemencodeerrors()

from specify_cli.cli.commands.charter.context import context as context_command

receipt = {"runner": "source-python-" + ".".join(str(p) for p in sys.version_info[:3]), "records": []}
for action in json.loads(os.environ["SK_CF72_ACTIONS"]):
    os.chdir(root / "project")
    out, err = io.StringIO(), io.StringIO()
    code, error, envelope = 0, None, None
    try:
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            context_command(
                owned_checkout=None, mission=None, action=action, include=None,
                mark_loaded=True, profile=None, mission_type="software-dev",
                json_output=True, include_all=False,
            )
    except SystemExit as exc:
        code = exc.code if isinstance(exc.code, int) else 1
    except BaseException as exc:
        code, error = 1, type(exc).__name__ + ": " + str(exc)
    text = out.getvalue()
    if code == 0:
        try:
            envelope = json.loads(text)
        except Exception as exc:
            error = "unparseable envelope: " + repr(exc)
    body = (envelope.get("context") if envelope else "") or ""
    receipt["records"].append({
        "action": action,
        "exit": code,
        "error": error,
        "mode": envelope.get("mode") if envelope else None,
        "text": body.replace(str(root / "project"), "<project>"),
    })
print("CF72_SOURCE_RECEIPT " + json.dumps(receipt))
"#;

fn run_source(root: &Path) -> JsonValue {
    let actions = serde_json::to_string(&ACTIONS.to_vec()).expect("actions JSON");
    let output = support::isolated_command(root, support::python())
        .arg("-c")
        .arg(SOURCE_PROBE)
        .env("SK_CF72_ROOT", root)
        .env("SK_CF72_ACTIONS", actions)
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
        .find(|line| line.starts_with("CF72_SOURCE_RECEIPT "))
        .unwrap_or_else(|| panic!("no Source receipt in {stdout:?}"));
    serde_json::from_str(line["CF72_SOURCE_RECEIPT ".len()..].trim()).expect("receipt JSON")
}

/// The Native child: assert isolation, then call the SAME facade the real CLI
/// command calls, in the same action order.
#[test]
fn cf72_native_compact_child() {
    let Some(root) = std::env::var_os("SK_CF72_NATIVE_ROOT").map(PathBuf::from) else {
        return; // ordinary run: the parent test drives this child
    };
    support::assert_isolated(&root);
    let codec = sk_kernel::python_os_encoding::OsEncodingContext::from_environment();
    assert_eq!(
        codec.label(),
        "utf-8, surrogateescape",
        "actual Native startup codec must match the Source child"
    );

    let mut records = Vec::new();
    for action in ACTIONS {
        std::env::set_current_dir(root.join("project")).expect("child cwd");
        let result = sk_cli::compat::charter::execute_context(
            &root.join("project"),
            action,
            Some("software-dev"),
            None,
            false,
            true,
            true,
        );
        let mut record = json!({"action": action});
        match result {
            Ok(execution) => {
                let text = execution
                    .text
                    .to_scalar_string()
                    .expect("scalar compact text");
                record["exit"] = json!(0);
                record["error"] = JsonValue::Null;
                record["mode"] = json!(match execution.mode {
                    sk_doctrine::ContextMode::Bootstrap => "bootstrap",
                    sk_doctrine::ContextMode::Compact => "compact",
                    sk_doctrine::ContextMode::Missing => "missing",
                });
                record["text"] =
                    json!(text.replace(&root.join("project").display().to_string(), "<project>"));
            }
            Err(error) => {
                record["exit"] = json!(1);
                record["error"] = json!(format!("{error:?}"));
                record["mode"] = JsonValue::Null;
                record["text"] = json!("");
            }
        }
        records.push(record);
    }
    let receipt = json!({"runner": "native-child", "records": records});
    std::fs::write(root.join("native.receipt.json"), receipt.to_string()).expect("native receipt");
}

fn run_native_child(root: &Path) -> JsonValue {
    let executable = std::env::current_exe().expect("test binary");
    let output = support::isolated_command(root, executable)
        .arg("cf72_native_compact_child")
        .arg("--exact")
        .arg("--nocapture")
        .arg("--test-threads=1")
        .env("SK_CF72_NATIVE_ROOT", root)
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
    let receipt = std::fs::read_to_string(root.join("native.receipt.json"))
        .expect("the Native child wrote its receipt");
    serde_json::from_str(&receipt).expect("native receipt JSON")
}

fn record_at(receipt: &JsonValue, index: usize) -> &JsonValue {
    receipt["records"]
        .as_array()
        .expect("records")
        .get(index)
        .unwrap_or_else(|| panic!("no record at {index}"))
}

/// The compact summary projection: the first four lines, the `Directive IDs:` /
/// `Tactic IDs:` sections and the `Diagnostics:` line. Everything after
/// `Section Anchors:`/authority paths stays parent-owned.
fn summary_block(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut in_ids = false;
    for line in text.lines() {
        if line == "Governance:" {
            lines.push(line.to_owned());
            in_ids = false;
            continue;
        }
        if line == "  - Template set: "
            || line.starts_with("  - Template set: ")
            || line.starts_with("  - Paradigms: ")
            || line.starts_with("  - Tools: ")
        {
            lines.push(line.to_owned());
            continue;
        }
        if line == "Directive IDs:" {
            in_ids = true;
            lines.push(line.to_owned());
            continue;
        }
        if in_ids {
            if line.starts_with("  - ") {
                lines.push(line.to_owned());
                continue;
            }
            in_ids = false;
        }
        if line.starts_with("  - Diagnostics: ") {
            lines.push(line.to_owned());
        }
    }
    lines
}

#[test]
fn compact_summary_matches_the_reference_in_two_actual_children() {
    // Two INDEPENDENT fixture trees: the compact-bundle rail mutates
    // `.kittify/charter/context-state.json`, so one shared tree would let the
    // Source child's mark decide the Native child's rail.
    let source_base = support::fixture("cf72-compact-source");
    let native_base = support::fixture("cf72-compact-native");
    build_project(&source_base);
    build_project(&native_base);

    let source = run_source(&source_base);
    let native = run_native_child(&native_base);
    let mut compact_seen = 0;
    for (index, action) in ACTIONS.iter().enumerate() {
        let expected = record_at(&source, index);
        let actual = record_at(&native, index);
        assert_eq!(actual["exit"], expected["exit"], "{action} exit");
        assert_eq!(actual["mode"], expected["mode"], "{action} mode");
        assert_eq!(actual["error"], expected["error"], "{action} error");
        if expected["mode"] != json!("compact") {
            // The first `implement` is the bootstrap rail; its compact summary
            // is not on this rail (the second `implement` is the steady state).
            continue;
        }
        compact_seen += 1;
        let expected_text = expected["text"].as_str().expect("source text");
        let actual_text = actual["text"].as_str().expect("native text");
        assert_eq!(
            summary_block(actual_text),
            summary_block(expected_text),
            "{action} governance summary"
        );
        // The resolved values must be real, not the pre-fix `(none)` rail.
        let block = summary_block(actual_text).join("\n");
        assert!(block.contains("  - Template set: software-dev-default"));
        assert!(!block.contains("  - Template set: (none)"));
        assert!(block.contains("  - Paradigms: ") && !block.contains("  - Paradigms: (none)"));
        assert!(block.contains("domain-driven-design"));
        assert!(block.contains("  - Tools: git, mypy, pytest, ruff, spec-kitty"));
        assert!(block.contains("  - DIRECTIVE_024"));
        assert!(block.contains("  - PROJ_001"));
        assert!(block.contains("Unknown selected paradigm 'no-such-paradigm'"));
        assert!(!block.contains("governance unresolved"));
    }
    assert_eq!(
        compact_seen, 2,
        "one NonBootstrap and one CompactBundle rail"
    );
}
