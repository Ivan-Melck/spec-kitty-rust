//! CF66 — ordered `charter context` execution: paired Source/Native proof.
//!
//! The reference CLI runs two real builds for `--json`: the text build first
//! (`build_charter_context`; `--profile` is an input, `--include-all` is not),
//! then the structured build (`build_charter_context_json`; no profile input,
//! `--include-all` is, and the first build's depth is passed through). This file
//! drives the **actual Source CLI handler** and the **same execution facade the
//! Native CLI calls** ([`sk_cli::compat::charter::execute_context`]) as two
//! actual children, each asserting its own temporary CWD, HOME, all seven XDG
//! roots, TMPDIR/TMP/TEMP, the absolute Source/packs roots and the hash seed
//! before any read.
//!
//! Controls:
//!
//! * stale bundle + invalid inline body + `--json --include-all`: the text stage
//!   completes, the first-load state write survives, and the structured stage
//!   fails — 2 real freshness ingestions, never a fabricated extra append;
//! * stale text-only: one ingestion; fresh: zero;
//! * `--no-mark-loaded`: no state file at all;
//! * profile and `--include-all` each belong to their Source stage: the profile
//!   styleguide reaches the text but not the structured arrays, and
//!   `--include-all` inlines the structured body without changing the text;
//! * an unknown profile is reported before a failing freshness gate;
//! * a real binary smoke check proves the CLI handler consumes this facade.
//!
//! Not whole-command parity, not approval, not performance.

#[path = "cf65_support/mod.rs"]
mod support;

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value as JsonValue, json};

const CHARTER_BODY: &str = "# CF66 CLI Charter\n\n## Policy Summary\n\n- Keep one canonical authority.\n\n## Terminology Canon\n\nUse Mission, never Feature.\n";

/// The valid link-delivered styleguide used by the non-failure cases.
const VALID_STYLEGUIDE: &str = "\
schema_version: \"1.0\"
id: common-docs
title: \"CF66 docs title\"
scope: docs
principles:
  - overlay principle
";

/// The same record with a member the JSON-mode dump refuses.
const LAZY_STYLEGUIDE: &str = "\
schema_version: \"1.0\"
id: common-docs
title: \"CF66 lazy docs title\"
scope: docs
principles:
  - overlay principle
structural_lint_config:
  bytes_value: !!binary /w==
";

const PROJECT_GRAPH: &str = "\
schema_version: '1.0'
generated_at: STATIC
generated_by: cf66-cli-test
nodes:
- urn: styleguide:common-docs
  kind: styleguide
  label: Common Docs
- urn: agent_profile:cf66-profile
  kind: agent_profile
  label: CF66 Profile
- urn: styleguide:cf66-profile-styleguide
  kind: styleguide
  label: CF66 Profile Styleguide
edges:
- source: action:software-dev/implement
  target: styleguide:common-docs
  relation: suggests
- source: agent_profile:cf66-profile
  target: styleguide:cf66-profile-styleguide
  relation: requires
";

const PROFILE: &str = "\
profile-id: cf66-profile
name: CF66 Profile
description: CF66 CLI phase probe profile.
schema-version: \"1.0\"
roles:
- implementer
purpose: Prove the profile channel is a text-stage input.
specialization:
  primary-focus: phase proof
styleguide-references:
- id: cf66-profile-styleguide
  rationale: Prove the profile channel is a text-stage input.
";

/// One case's exact inputs.
#[derive(Clone, Copy)]
struct Case {
    name: &'static str,
    stale: bool,
    valid_styleguide: bool,
    broken_git: bool,
    json: bool,
    include_all: bool,
    mark_loaded: bool,
    profile: Option<&'static str>,
}

const CASES: [Case; 9] = [
    Case {
        name: "stale-invalid-json",
        stale: true,
        valid_styleguide: false,
        broken_git: false,
        json: true,
        include_all: true,
        mark_loaded: true,
        profile: None,
    },
    Case {
        name: "stale-invalid-text",
        stale: true,
        valid_styleguide: false,
        broken_git: false,
        json: false,
        include_all: false,
        mark_loaded: true,
        profile: None,
    },
    Case {
        name: "fresh-json",
        stale: false,
        valid_styleguide: true,
        broken_git: false,
        json: true,
        include_all: false,
        mark_loaded: true,
        profile: None,
    },
    Case {
        name: "fresh-include-all",
        stale: false,
        valid_styleguide: true,
        broken_git: false,
        json: true,
        include_all: true,
        mark_loaded: true,
        profile: None,
    },
    Case {
        name: "fresh-no-mark",
        stale: false,
        valid_styleguide: true,
        broken_git: false,
        json: true,
        include_all: false,
        mark_loaded: false,
        profile: None,
    },
    Case {
        name: "fresh-profile",
        stale: false,
        valid_styleguide: true,
        broken_git: false,
        json: true,
        include_all: false,
        mark_loaded: true,
        profile: Some("cf66-profile"),
    },
    Case {
        name: "normalized-json",
        stale: false,
        valid_styleguide: true,
        broken_git: false,
        json: true,
        include_all: false,
        mark_loaded: true,
        profile: None,
    },
    Case {
        name: "normalized-loaded-json",
        stale: false,
        valid_styleguide: true,
        broken_git: false,
        json: true,
        include_all: false,
        mark_loaded: true,
        profile: None,
    },
    Case {
        name: "broken-gate",
        stale: true,
        valid_styleguide: true,
        broken_git: true,
        json: false,
        include_all: false,
        mark_loaded: true,
        profile: Some("cf66-ghost"),
    },
];

fn case_action(case: &Case) -> &'static str {
    if case.name.starts_with("normalized-") {
        "\u{1c}IMPLEMENT\u{1f}"
    } else {
        "implement"
    }
}

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
    if case.broken_git {
        // A `.git` file outside the worktree topology: `find_repo_root` accepts
        // it (it is not a worktree pointer) while the canonical git-common-dir
        // probe the freshness gate uses refuses.
        write(&root.join(".git"), "gitdir: /nonexistent/cf66-gitdir\n");
    } else {
        git_init(root);
    }
    write(
        &root.join(".kittify/config.yaml"),
        "# authored project config\ncustom_setting: keep-me\n",
    );
    write(
        &root.join(".kittify/charter/charter.yaml"),
        "schema_version: \"1.1.0\"\ncatalog:\n  references: []\ngovernance:\n  authority_paths: [docs/context]\n",
    );
    write(&root.join(".kittify/charter/charter.md"), CHARTER_BODY);
    if case.name == "normalized-loaded-json" {
        write(
            &root.join(".kittify/charter/context-state.json"),
            r#"{"schema_version":"1.0.0","actions":{"implement":"2026-01-01T00:00:00Z"}}"#,
        );
    }
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
        if case.valid_styleguide {
            VALID_STYLEGUIDE
        } else {
            LAZY_STYLEGUIDE
        },
    );
    write(
        &root.join(".kittify/charter-packs/styleguide/cf66-profile-styleguide.styleguide.yaml"),
        "schema_version: \"1.0\"\nid: cf66-profile-styleguide\ntitle: \"CF66 Profile Styleguide\"\nscope: docs\nprinciples:\n  - profile principle\n",
    );
    write(
        &root.join(".kittify/charter-packs/cf66.graph.yaml"),
        PROJECT_GRAPH,
    );
    write(
        &root.join(".kittify/charter-packs/agent_profiles/cf66-profile.agent.yaml"),
        PROFILE,
    );
}

fn build_cases(root: &Path) {
    for case in &CASES {
        build_case(&root.join("cases").join(case.name), case);
    }
}

/// The Source child's script: assert the environment, then drive the ACTUAL
/// `charter context` CLI handler once per case.
const SOURCE_PROBE: &str = r#"
import contextlib, io, json, os, pathlib, sys

root = pathlib.Path(os.environ["SK_CF66_ROOT"])
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

from specify_cli.cli.commands.charter.context import context as context_command

CASES = json.loads(os.environ["SK_CF66_CASES"])


def provenance(case_root):
    path = case_root / ".kittify/encoding-provenance/global.jsonl"
    if not path.exists():
        return 0
    return len([line for line in path.read_text(encoding="utf-8").splitlines() if line])


def state_actions(case_root):
    path = case_root / ".kittify/charter/context-state.json"
    if not path.exists():
        return None
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except Exception:
        return ["<unreadable>"]
    actions = data.get("actions")
    return sorted(actions) if isinstance(actions, dict) else []


def styleguide_view(envelope, case_id):
    if envelope is None:
        return None
    items = {entry["id"]: entry for entry in envelope["styleguides"]}
    common = items.get("common-docs")
    return {
        "ids": sorted(items),
        "common_delivery": common.get("delivery") if common else None,
        "common_body": bool(common and "body" in common),
        "profile_styleguide": "cf66-profile-styleguide" in items,
    }


receipt = {"runner": "source-python-" + ".".join(str(part) for part in sys.version_info[:3]),
           "cases": []}
for case in CASES:
    case_root = root / "cases" / case["name"]
    os.chdir(case_root)
    before = provenance(case_root)
    stdout, stderr = io.StringIO(), io.StringIO()
    envelope = None
    error = None
    code = 0
    try:
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            context_command(
                owned_checkout=None, mission=None, action=case["action"], include=None,
                mark_loaded=case["mark_loaded"], profile=case["profile"],
                mission_type="software-dev", json_output=case["json"],
                include_all=case["include_all"],
            )
    except SystemExit as exc:
        code = exc.code if isinstance(exc.code, int) else 1
    except BaseException as exc:  # the reference's own escape is the result
        code = 1
        error = type(exc).__name__ + ": " + str(exc)
    text = stdout.getvalue()
    if case["json"] and code == 0:
        try:
            envelope = json.loads(text)
        except Exception as exc:
            error = "unparseable envelope: " + type(exc).__name__ + ": " + str(exc)
    receipt["cases"].append({
        "case": case["name"],
        "ok": code == 0 and error is None,
        "error": error,
        "stderr": stderr.getvalue(),
        "mode": envelope.get("mode") if envelope else None,
        "action": envelope.get("action") if envelope else None,
        "first_load": envelope.get("first_load") if envelope else None,
        "text": (envelope.get("context") if envelope else text).replace(str(case_root), "<case>"),
        "structured": styleguide_view(envelope, case["name"]),
        "state_actions": state_actions(case_root),
        "provenance_delta": provenance(case_root) - before,
    })
print("CF66_SOURCE_RECEIPT " + json.dumps(receipt))
"#;

fn run_source(root: &Path) -> JsonValue {
    let cases = JsonValue::Array(
        CASES
            .iter()
            .map(|case| {
                json!({
                    "name": case.name,
                    "action": case_action(case),
                    "json": case.json,
                    "include_all": case.include_all,
                    "mark_loaded": case.mark_loaded,
                    "profile": case.profile,
                })
            })
            .collect(),
    );
    let output = support::isolated_command(root, support::python())
        .arg("-c")
        .arg(SOURCE_PROBE)
        .env("SK_CF66_ROOT", root)
        .env("SK_CF66_CASES", cases.to_string())
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
        .find(|line| line.starts_with("CF66_SOURCE_RECEIPT "))
        .unwrap_or_else(|| panic!("no Source receipt in {stdout:?}"));
    serde_json::from_str(line["CF66_SOURCE_RECEIPT ".len()..].trim()).expect("receipt JSON")
}

/// The Native child: assert isolation, then call the SAME facade the real CLI
/// command calls.
#[test]
fn cf66_native_context_child() {
    let Some(root) = std::env::var_os("SK_CF66_NATIVE_ROOT").map(PathBuf::from) else {
        return; // ordinary run: the parent test drives this child
    };
    support::assert_isolated(&root);

    let mut cases = Vec::new();
    for case in &CASES {
        let case_root = root.join("cases").join(case.name);
        std::env::set_current_dir(&case_root).expect("child cwd");
        assert_eq!(std::env::current_dir().expect("cwd"), case_root);
        let before = provenance(&case_root);
        let result = sk_cli::compat::charter::execute_context(
            &case_root,
            case_action(case),
            Some("software-dev"),
            case.profile,
            case.include_all,
            case.json,
            case.mark_loaded,
        );
        let mut entry = json!({
            "case": case.name,
            "ok": true,
            "error": JsonValue::Null,
            "mode": JsonValue::Null,
            "action": JsonValue::Null,
            "first_load": JsonValue::Null,
            "text": "",
            "structured": JsonValue::Null,
            "state_actions": state_actions(&case_root),
            "provenance_delta": provenance(&case_root) - before,
        });
        match result {
            Ok(execution) => {
                entry["mode"] = json!(match execution.mode {
                    sk_doctrine::ContextMode::Bootstrap => "bootstrap",
                    sk_doctrine::ContextMode::Compact => "compact",
                    // Mechanical ripple: this phase build never takes the
                    // text-only `missing` rail.
                    sk_doctrine::ContextMode::Missing => "missing",
                });
                entry["action"] = json!(execution.action);
                entry["first_load"] = json!(execution.first_load);
                entry["text"] = json!(
                    execution
                        .text
                        .to_scalar_string()
                        .unwrap_or_default()
                        .replace(&case_root.display().to_string(), "<case>")
                );
                if let Some(payload) = execution.payload {
                    let payload = payload.expect("this paired fixture has a finite scalar payload");
                    entry["structured"] = styleguide_view(&payload);
                }
            }
            Err(error) => {
                entry["ok"] = json!(false);
                entry["error"] = json!(error.message());
            }
        }
        cases.push(entry);
    }
    println!(
        "CF66_NATIVE_RECEIPT {}",
        json!({"runner": "native", "cases": cases})
    );
}

fn run_native_child(root: &Path) -> JsonValue {
    let executable = std::env::current_exe().expect("test binary");
    let output = support::isolated_command(root, executable)
        .args(["--exact", "cf66_native_context_child", "--nocapture"])
        .env("SK_CF66_NATIVE_ROOT", root)
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
        .find(|line| line.starts_with("CF66_NATIVE_RECEIPT "))
        .unwrap_or_else(|| panic!("no Native receipt in {stdout:?}"));
    serde_json::from_str(line["CF66_NATIVE_RECEIPT ".len()..].trim()).expect("receipt JSON")
}

fn provenance(case_root: &Path) -> usize {
    let path = case_root.join(".kittify/encoding-provenance/global.jsonl");
    match std::fs::read_to_string(&path) {
        Ok(text) => text.lines().filter(|line| !line.is_empty()).count(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => panic!("unreadable journal {}: {error}", path.display()),
    }
}

fn state_actions(case_root: &Path) -> Option<Vec<String>> {
    let path = case_root.join(".kittify/charter/context-state.json");
    let text = std::fs::read_to_string(&path).ok()?;
    let value: JsonValue = serde_json::from_str(&text).expect("state JSON");
    let actions = value.get("actions")?.as_object()?;
    Some(actions.keys().cloned().collect())
}

fn styleguide_view(envelope: &JsonValue) -> JsonValue {
    let empty = Vec::new();
    let items = envelope["styleguides"].as_array().unwrap_or(&empty);
    let ids: Vec<&str> = items
        .iter()
        .filter_map(|entry| entry["id"].as_str())
        .collect();
    let common = items
        .iter()
        .find(|entry| entry["id"] == json!("common-docs"));
    json!({
        "ids": ids,
        "common_delivery": common.and_then(|entry| entry["delivery"].as_str()),
        "common_body": common.is_some_and(|entry| entry.get("body").is_some()),
        "profile_styleguide": ids.contains(&"cf66-profile-styleguide"),
    })
}

fn by_case<'a>(receipt: &'a JsonValue, name: &str) -> &'a JsonValue {
    receipt["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .find(|case| case["case"] == json!(name))
        .unwrap_or_else(|| panic!("no {name} case"))
}

#[test]
fn ordered_execution_matches_the_reference_stages() {
    let source_root = support::fixture("cf66-cli-source");
    build_cases(&source_root);
    let source = run_source(&source_root);

    let native_root = support::fixture("cf66-cli-native");
    build_cases(&native_root);
    let native = run_native_child(&native_root);

    // Source-normalized action and state-key controls only. The loaded case's
    // compact rendering/mode remains a separately recorded Native capability gap.
    assert_eq!(
        by_case(&source, "normalized-loaded-json")["mode"],
        json!("compact")
    );
    for receipt in [&source, &native] {
        for (name, first) in [("normalized-json", true), ("normalized-loaded-json", false)] {
            let case = by_case(receipt, name);
            assert_eq!(case["ok"], json!(true), "{case}");
            assert_eq!(case["action"], json!("implement"), "{case}");
            assert_eq!(case["first_load"], json!(first), "{case}");
            assert_eq!(case["state_actions"], json!(["implement"]), "{case}");
        }
    }

    // --- stale JSON + invalid inline body + include_all -------------------
    // Text stage completes (and marks), structured stage fails; two real
    // ingestions; the first state write survives.
    let source_fail = by_case(&source, "stale-invalid-json");
    let native_fail = by_case(&native, "stale-invalid-json");
    assert_eq!(source_fail["ok"], json!(false), "{source_fail}");
    assert_eq!(native_fail["ok"], json!(false), "{native_fail}");
    assert_eq!(
        source_fail["state_actions"],
        json!(["implement"]),
        "{source_fail}"
    );
    assert_eq!(
        native_fail["state_actions"],
        json!(["implement"]),
        "{native_fail}"
    );
    assert_eq!(source_fail["provenance_delta"], json!(2), "{source_fail}");
    assert_eq!(native_fail["provenance_delta"], json!(2), "{native_fail}");

    // --- stale text-only: one real ingestion, text succeeds ---------------
    for receipt in [&source, &native] {
        let case = by_case(receipt, "stale-invalid-text");
        assert_eq!(case["ok"], json!(true), "{case}");
        assert_eq!(case["state_actions"], json!(["implement"]), "{case}");
        assert_eq!(case["provenance_delta"], json!(1), "{case}");
    }

    // --- fresh JSON: no ingestion, envelope from the first stage ----------
    for receipt in [&source, &native] {
        let case = by_case(receipt, "fresh-json");
        assert_eq!(case["ok"], json!(true), "{case}");
        assert_eq!(case["mode"], json!("bootstrap"), "{case}");
        assert_eq!(case["action"], json!("implement"), "{case}");
        assert_eq!(case["first_load"], json!(true), "{case}");
        assert_eq!(case["state_actions"], json!(["implement"]), "{case}");
        assert_eq!(case["provenance_delta"], json!(0), "{case}");
        assert_eq!(
            case["structured"]["common_delivery"],
            json!("link"),
            "{case}"
        );
        assert_eq!(case["structured"]["common_body"], json!(false), "{case}");
        assert_eq!(
            case["structured"]["profile_styleguide"],
            json!(false),
            "{case}"
        );
    }

    // --- include_all belongs to the structured stage only -----------------
    for receipt in [&source, &native] {
        let plain = by_case(receipt, "fresh-json");
        let inline = by_case(receipt, "fresh-include-all");
        assert_eq!(inline["ok"], json!(true), "{inline}");
        assert_eq!(inline["provenance_delta"], json!(0), "{inline}");
        assert_eq!(
            inline["structured"]["common_delivery"],
            json!("inline"),
            "{inline}"
        );
        assert_eq!(inline["structured"]["common_body"], json!(true), "{inline}");
        assert_eq!(
            inline["text"], plain["text"],
            "the text stage must be byte-identical with and without --include-all"
        );
    }

    // --- --no-mark-loaded writes no state ---------------------------------
    for receipt in [&source, &native] {
        let case = by_case(receipt, "fresh-no-mark");
        assert_eq!(case["ok"], json!(true), "{case}");
        assert_eq!(case["provenance_delta"], json!(0), "{case}");
        assert_eq!(case["state_actions"], JsonValue::Null, "{case}");
    }

    // --- profile is a text-stage input only -------------------------------
    for receipt in [&source, &native] {
        let case = by_case(receipt, "fresh-profile");
        assert_eq!(case["ok"], json!(true), "{case}");
        assert_eq!(
            case["structured"]["profile_styleguide"],
            json!(false),
            "the structured arrays must not receive the text profile channel: {case}"
        );
        let plain = by_case(receipt, "fresh-json");
        assert_ne!(
            case["text"], plain["text"],
            "the profile channel reaches the text stage: {case}"
        );
        assert_eq!(
            case["structured"]["ids"], plain["structured"]["ids"],
            "the structured arrays are identical with and without --profile: {case}"
        );
    }

    // --- unknown profile is reported before the failing freshness gate ----
    // The Source CLI warns on the redirected stderr the child captured; the
    // Native facade writes the same warning to the child process's stderr.
    let source_stderr = by_case(&source, "broken-gate")["stderr"]
        .as_str()
        .expect("Source stderr")
        .to_owned();
    let native_stderr =
        std::fs::read_to_string(native_root.join("native.stderr")).expect("native stderr");
    for (label, stderr) in [("source", &source_stderr), ("native", &native_stderr)] {
        assert!(
            stderr.contains("Warning: profile 'cf66-ghost' not found"),
            "{label} reports the unknown profile before the gate: {stderr}"
        );
    }
    for receipt in [&source, &native] {
        let case = by_case(receipt, "broken-gate");
        assert_eq!(case["ok"], json!(false), "{case}");
        assert_eq!(case["state_actions"], JsonValue::Null, "{case}");
    }
}

#[test]
fn action_keys_use_source_strip_and_unicode_lower_without_selector_shortening() {
    assert_eq!(
        sk_doctrine::normalize_context_action("\u{1c}\u{a0}IMPLEMENT\u{1f}"),
        "implement"
    );
    assert_eq!(
        sk_doctrine::normalize_context_action(" action:software-dev/IMPLEMENT "),
        "action:software-dev/implement"
    );
    assert_eq!(sk_doctrine::normalize_context_action(" ΟΣ "), "ος");
}

#[test]
fn exact_execution_survives_an_explicit_finite_adapter_failure() {
    let root = support::fixture("cf66-parent-finite");
    let case = CASES.iter().find(|case| case.name == "fresh-json").unwrap();
    let project = root.join("case");
    build_case(&project, case);
    write(
        &project.join(".kittify/charter-packs/styleguide/common-docs.styleguide.yaml"),
        &VALID_STYLEGUIDE.replace("CF66 docs title", r"\ud800"),
    );
    let execution = sk_cli::compat::charter::execute_context(
        &project,
        "implement",
        Some("software-dev"),
        None,
        false,
        true,
        false,
    )
    .unwrap();
    assert!(
        execution.payload_exact.is_some(),
        "exact execution succeeds"
    );
    assert!(
        execution
            .payload
            .expect("JSON requests a finite view")
            .is_err(),
        "unrepresentable exact data is not successful null"
    );
}

/// Bounded real-binary smoke: the command handler consumes the same facade.
///
/// Not a pair (the child has no in-child environment assertions); the paired
/// proof above is the business comparison.
#[test]
fn real_binary_context_handler_uses_the_ordered_facade() {
    let root = support::fixture("cf66-cli-binary");
    let case = CASES
        .iter()
        .find(|case| case.name == "fresh-json")
        .expect("fresh-json case");
    build_case(&root.join("case"), case);
    let project = root.join("case");
    let output = Command::new(env!("CARGO_BIN_EXE_spec-kitty"))
        .current_dir(&project)
        .env("SPECIFY_REPO_ROOT", &project)
        .env_remove("SPEC_KITTY_PACKS_ROOT")
        .env_remove("PYTHONIOENCODING")
        .env("PYTHONUTF8", "1")
        .args([
            "charter",
            "context",
            "--action",
            "implement",
            "--mission-type",
            "software-dev",
            "--json",
        ])
        .output()
        .expect("real binary");
    assert!(
        output.status.success(),
        "binary failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 stdout");
    for key in [
        "\n  \"action\": \"implement\"",
        "\n  \"mode\": \"bootstrap\"",
        "\n  \"first_load\": true",
        "\n  \"context\": ",
        "\n  \"styleguides\": [",
    ] {
        assert!(stdout.contains(key), "missing {key:?} in {stdout}");
    }
    assert!(
        project
            .join(".kittify/charter/context-state.json")
            .is_file(),
        "the real handler persisted the first-load state"
    );
}
