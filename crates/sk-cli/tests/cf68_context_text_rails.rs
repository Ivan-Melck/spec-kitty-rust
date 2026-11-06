//! CF68 — the reference's four `charter context` TEXT rails, paired Source/Native.
//!
//! `build_charter_context` has four mutually exclusive text rails, selected in
//! one order, each with its own first-load/depth/mark/prose-read/reference-count
//! policy:
//!
//! 1. an **undeclared non-bootstrap** action returns *before* the charter
//!    presence gate (`compact`, `first_load=false`, no state mark, count `0`);
//! 2. then the **charter presence** gate (`charter.yaml` OR `charter.md`);
//! 3. then the **below-minimum-depth** tier (the widened `compact` bundle,
//!    which marks *before* rendering);
//! 4. else **bootstrap** (renders, *then* marks; its `references_count` is the
//!    `catalog.references` catalogue length).
//!
//! This file drives the **actual Source CLI handler** and the **same facade the
//! Native CLI calls** ([`sk_cli::compat::charter::execute_context`]) as two
//! actual children, each of which asserts its own temporary CWD, HOME, all
//! seven XDG roots, TMPDIR/TMP/TEMP, the absolute Source and packs roots and the
//! hash seed **before** any read.
//!
//! Not whole-command parity: the compact rails' resolved template-set/paradigm/
//! tool text and the full profile/property/token-budget rendering remain
//! parent-owned work, so the compact rails' first line (the rail's own header)
//! is compared and their ownership-resolved body is not.

#[path = "cf65_support/mod.rs"]
mod support;

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value as JsonValue, json};

const CHARTER_BODY: &str = "# CF68 rails charter\n\n## Policy Summary\n\n- Keep one canonical authority.\n\n## Terminology Canon\n\nUse Mission, never Feature.\n";

/// Three catalogue entries; the second names a document that does not exist, so
/// it still counts while contributing no `Reference Docs:` pointer.
const CATALOG: &str = "\
catalog:
  references:
    - id: \"directive:DIRECTIVE_001\"
      title: \"Architectural Integrity\"
      local_path: \"directives/001-architectural-integrity-standard.directive.yaml\"
      kind: \"directive\"
      summary: \"one\"
    - id: \"tactic:no-such-tactic\"
      title: \"No such file\"
      local_path: \"tactics/architecture/no-such-file.tactic.yaml\"
      kind: \"tactic\"
      summary: \"two\"
    - id: \"directive:DIRECTIVE_010\"
      title: \"Specification Fidelity\"
      local_path: \"directives/010-specification-fidelity-requirement.directive.yaml\"
      kind: \"directive\"
      summary: \"three\"
";

/// One rail case.
struct Case {
    name: &'static str,
    /// The actions this case runs, in order, against the same project.
    actions: &'static [&'static str],
    yaml: bool,
    markdown: bool,
    catalog: bool,
    deny: Option<&'static str>,
    mark_loaded: bool,
}

const CASES: &[Case] = &[
    Case {
        name: "bootstrap",
        actions: &["implement"],
        yaml: true,
        markdown: true,
        catalog: true,
        deny: None,
        mark_loaded: true,
    },
    Case {
        name: "repeat",
        // The same normalized action twice: the second run is the steady-state
        // (compact-bundle) rail. `IMPLEMENT` also proves the state key is the
        // Source-normalized spelling.
        actions: &["IMPLEMENT", " implement "],
        yaml: true,
        markdown: true,
        catalog: true,
        deny: None,
        mark_loaded: true,
    },
    Case {
        name: "declared",
        // `action:software-dev/tasks` IS declared in the merged built-in graph:
        // a declared non-fast action must still take the bootstrap rail.
        actions: &["tasks"],
        yaml: true,
        markdown: true,
        catalog: true,
        deny: None,
        mark_loaded: true,
    },
    Case {
        name: "undeclared",
        // No `action:software-dev/consolidate` node exists: compact, and no
        // state mark is written at all.
        actions: &["consolidate"],
        yaml: true,
        markdown: true,
        catalog: true,
        deny: None,
        mark_loaded: true,
    },
    Case {
        name: "yaml_only",
        actions: &["implement"],
        yaml: true,
        markdown: false,
        catalog: true,
        deny: None,
        mark_loaded: true,
    },
    Case {
        name: "md_only",
        actions: &["implement"],
        yaml: false,
        markdown: true,
        catalog: false,
        deny: None,
        mark_loaded: true,
    },
    Case {
        name: "neither",
        actions: &["implement"],
        yaml: false,
        markdown: false,
        catalog: false,
        deny: None,
        mark_loaded: true,
    },
    Case {
        name: "deny",
        // The effective deny drops `DIRECTIVE_010` from the catalogue.
        actions: &["implement"],
        yaml: true,
        markdown: true,
        catalog: true,
        deny: Some("deactivated_directives: [DIRECTIVE_010]\n"),
        mark_loaded: true,
    },
    Case {
        name: "no_mark",
        actions: &["implement"],
        yaml: true,
        markdown: true,
        catalog: true,
        deny: None,
        mark_loaded: false,
    },
];

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("fixture parent");
    }
    std::fs::write(path, contents).expect("write fixture");
}

fn git_init(root: &Path) {
    let status = Command::new("git")
        .arg("init")
        .arg("-q")
        .arg("-b")
        .arg("main")
        .arg(root)
        .status()
        .expect("git init");
    assert!(status.success(), "git init failed for {}", root.display());
}

fn build_case(root: &Path, case: &Case) {
    let project = root.join("cases").join(case.name);
    write(
        &project.join(".kittify/config.yaml"),
        "custom_setting: keep-me\n",
    );
    if case.yaml {
        let mut body = String::from("schema_version: \"1.1.0\"\n");
        if case.catalog {
            body.push_str(CATALOG);
        }
        write(&project.join(".kittify/charter/charter.yaml"), &body);
    }
    if case.markdown {
        write(&project.join(".kittify/charter/charter.md"), CHARTER_BODY);
    }
    if let Some(deny) = case.deny {
        write(&project.join(".kittify/charter/activation.yaml"), deny);
    }
    git_init(&project);
}

/// The Source child's script: assert the environment, then drive the ACTUAL
/// `charter context` CLI handler once per (case, action) in order.
const SOURCE_PROBE: &str = r#"
import contextlib, io, json, os, pathlib, sys

root = pathlib.Path(os.environ["SK_CF68_ROOT"])
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

CASES = json.loads(os.environ["SK_CF68_CASES"])


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


receipt = {"runner": "source-python-" + ".".join(str(p) for p in sys.version_info[:3]), "records": []}
for case in CASES:
    case_root = root / "cases" / case["name"]
    for action in case["actions"]:
        os.chdir(case_root)
        out, err = io.StringIO(), io.StringIO()
        code, error, envelope = 0, None, None
        try:
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                context_command(
                    owned_checkout=None, mission=None, action=action, include=None,
                    mark_loaded=case["mark_loaded"], profile=None,
                    mission_type="software-dev", json_output=True, include_all=False,
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
            "case": case["name"],
            "action": action,
            "exit": code,
            "error": error,
            "mode": envelope.get("mode") if envelope else None,
            "first_load": envelope.get("first_load") if envelope else None,
            "references_count": envelope.get("references_count") if envelope else None,
            "state_actions": state_actions(case_root),
            "text_head": [line.replace(str(case_root), "<case>") for line in body.splitlines()[:3]],
            "text_full": body.replace(str(case_root), "<case>") if len(body) < 400 else None,
        })
print("CF68_SOURCE_RECEIPT " + json.dumps(receipt))
"#;

fn cases_json() -> JsonValue {
    JsonValue::Array(
        CASES
            .iter()
            .map(|case| {
                json!({
                    "name": case.name,
                    "actions": case.actions,
                    "mark_loaded": case.mark_loaded,
                })
            })
            .collect(),
    )
}

fn run_source(root: &Path) -> JsonValue {
    let output = support::isolated_command(root, support::python())
        .arg("-c")
        .arg(SOURCE_PROBE)
        .env("SK_CF68_ROOT", root)
        .env("SK_CF68_CASES", cases_json().to_string())
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
        .find(|line| line.starts_with("CF68_SOURCE_RECEIPT "))
        .unwrap_or_else(|| panic!("no Source receipt in {stdout:?}"));
    serde_json::from_str(line["CF68_SOURCE_RECEIPT ".len()..].trim()).expect("receipt JSON")
}

/// The Native child: assert isolation, then call the SAME facade the real CLI
/// command calls, in the same case/action order.
#[test]
fn cf68_native_rails_child() {
    let Some(root) = std::env::var_os("SK_CF68_NATIVE_ROOT").map(PathBuf::from) else {
        return; // ordinary run: the parent test drives this child
    };
    support::assert_isolated(&root);

    let mut records = Vec::new();
    for case in CASES {
        let case_root = root.join("cases").join(case.name);
        for action in case.actions {
            std::env::set_current_dir(&case_root).expect("child cwd");
            assert_eq!(std::env::current_dir().expect("cwd"), case_root);
            let result = sk_cli::compat::charter::execute_context(
                &case_root,
                action,
                Some("software-dev"),
                None,
                false,
                true,
                case.mark_loaded,
            );
            let mut record = json!({
                "case": case.name,
                "action": action,
                "state_actions": state_actions(&case_root),
            });
            match result {
                Ok(execution) => {
                    let text = execution.text.to_scalar_string().expect("scalar rail text");
                    let body = text.replace(&case_root.display().to_string(), "<case>");
                    record["exit"] = json!(0);
                    record["error"] = JsonValue::Null;
                    record["mode"] = json!(match execution.mode {
                        sk_doctrine::ContextMode::Bootstrap => "bootstrap",
                        sk_doctrine::ContextMode::Compact => "compact",
                        sk_doctrine::ContextMode::Missing => "missing",
                    });
                    record["first_load"] = json!(execution.first_load);
                    record["references_count"] = json!(execution.references_count);
                    record["depth"] = json!(execution.depth);
                    record["text_head"] = json!(
                        body.lines()
                            .take(3)
                            .map(str::to_owned)
                            .collect::<Vec<String>>()
                    );
                    record["text_full"] = if body.len() < 400 {
                        json!(body)
                    } else {
                        JsonValue::Null
                    };
                }
                Err(error) => {
                    record["exit"] = json!(1);
                    record["error"] = json!(format!("{error:?}"));
                    record["mode"] = JsonValue::Null;
                    record["first_load"] = JsonValue::Null;
                    record["references_count"] = JsonValue::Null;
                    record["text_head"] = json!([]);
                    record["text_full"] = JsonValue::Null;
                }
            }
            records.push(record);
        }
    }
    let receipt = json!({"runner": "native-child", "records": records});
    std::fs::write(root.join("native.receipt.json"), receipt.to_string()).expect("native receipt");
}

fn state_actions(case_root: &Path) -> JsonValue {
    let path = case_root.join(".kittify/charter/context-state.json");
    let Ok(bytes) = std::fs::read(&path) else {
        return JsonValue::Null;
    };
    let Ok(value) = serde_json::from_slice::<JsonValue>(&bytes) else {
        return json!(["<unreadable>"]);
    };
    match value.get("actions").and_then(JsonValue::as_object) {
        Some(actions) => {
            let mut names: Vec<String> = actions.keys().cloned().collect();
            names.sort();
            json!(names)
        }
        None => json!([]),
    }
}

fn run_native_child(root: &Path) -> JsonValue {
    let executable = std::env::current_exe().expect("test binary");
    let output = support::isolated_command(root, executable)
        .arg("cf68_native_rails_child")
        .arg("--exact")
        .arg("--nocapture")
        .arg("--test-threads=1")
        .env("SK_CF68_NATIVE_ROOT", root)
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

fn by_case<'a>(receipt: &'a JsonValue, case: &str, action: &str) -> &'a JsonValue {
    receipt["records"]
        .as_array()
        .expect("records")
        .iter()
        .find(|record| record["case"] == case && record["action"] == action)
        .unwrap_or_else(|| panic!("no record for {case}/{action}"))
}

/// The rail matrix: every case's mode/first_load/reference count/state mark and
/// the rail's own header line, compared with the live Source result.
#[test]
fn rails_match_the_reference_in_two_actual_children() {
    // Two INDEPENDENT fixture trees: the text stage mutates
    // `.kittify/charter/context-state.json`, so one shared tree would let the
    // Source child's first-load mark decide the Native child's rail.
    let source_base = support::fixture("cf68-rails-source");
    let native_base = support::fixture("cf68-rails-native");
    for case in CASES {
        build_case(&source_base, case);
        build_case(&native_base, case);
    }
    let source = run_source(&source_base);
    let native = run_native_child(&native_base);
    for case in CASES {
        for action in case.actions {
            let expected = by_case(&source, case.name, action);
            let actual = by_case(&native, case.name, action);
            assert_eq!(
                actual["exit"], expected["exit"],
                "{}/{} exit: source error {:?}, native error {:?}",
                case.name, action, expected["error"], actual["error"]
            );
            assert_eq!(
                actual["mode"], expected["mode"],
                "{}/{} mode",
                case.name, action
            );
            assert_eq!(
                actual["first_load"], expected["first_load"],
                "{}/{} first_load",
                case.name, action
            );
            assert_eq!(
                actual["references_count"], expected["references_count"],
                "{}/{} references_count",
                case.name, action
            );
            assert_eq!(
                actual["state_actions"], expected["state_actions"],
                "{}/{} state marks",
                case.name, action
            );
            let expected_head = expected["text_head"].as_array().expect("source head");
            let actual_head = actual["text_head"].as_array().expect("native head");
            let compared = match expected["mode"].as_str() {
                // The compact rails' ownership-resolved body is parent-owned
                // work: only the rail's own header line is in scope here.
                Some("compact") => 1,
                _ => 3,
            };
            for index in 0..compared.min(expected_head.len()) {
                assert_eq!(
                    actual_head.get(index),
                    expected_head.get(index),
                    "{}/{} text line {index}",
                    case.name,
                    action
                );
            }
            // The missing rail's whole body is the reference's fixed text.
            if expected["mode"] == json!("missing") {
                assert_eq!(
                    actual["text_full"], expected["text_full"],
                    "{}/{} missing text",
                    case.name, action
                );
            }
        }
    }
}

/// The rails themselves, without children: the ordering rule, the undeclared
/// rail's own depth, and the count/catalogue separation.
#[test]
fn rail_selection_follows_the_reference_order() {
    use sk_doctrine::{TextRail, select_text_rail};

    // 4-token fast path ⇒ no declaration needed.
    assert_eq!(select_text_rail(true, false, true, 2), TextRail::Bootstrap);
    // Undeclared non-bootstrap returns BEFORE the presence gate: absent charter
    // still yields the non-bootstrap compact rail, never `missing`.
    assert_eq!(
        select_text_rail(false, false, false, 2),
        TextRail::NonBootstrap
    );
    // Declared-but-activation-starved is still declared (node-URN membership).
    assert_eq!(select_text_rail(false, true, false, 2), TextRail::Missing);
    // The depth tier applies only after presence.
    assert_eq!(
        select_text_rail(true, false, true, 1),
        TextRail::CompactBundle
    );
    assert_eq!(select_text_rail(true, false, true, 2), TextRail::Bootstrap);
    // Mode spelling and mark capability per rail.
    assert_eq!(
        TextRail::NonBootstrap.mode(),
        sk_doctrine::ContextMode::Compact
    );
    assert_eq!(
        TextRail::CompactBundle.mode(),
        sk_doctrine::ContextMode::Compact
    );
    assert_eq!(TextRail::Missing.mode(), sk_doctrine::ContextMode::Missing);
    assert_eq!(
        TextRail::Bootstrap.mode(),
        sk_doctrine::ContextMode::Bootstrap
    );
    assert!(!TextRail::NonBootstrap.marks_first_load());
    assert!(!TextRail::Missing.marks_first_load());
    assert!(TextRail::CompactBundle.marks_first_load());
    assert!(TextRail::Bootstrap.marks_first_load());
    assert_eq!(
        sk_doctrine::context_render::MISSING_CHARTER_TEXT,
        "Charter Context:\n  - Charter file not found at `.kittify/charter/charter.yaml`.\n  - Run `spec-kitty charter interview` then `spec-kitty charter generate`."
    );
}

/// The catalogue reader: presence/parse degradation, deny filtering, exact field
/// text and the count-vs-pointer separation, all on fresh fixtures.
#[test]
fn catalogue_count_is_the_authored_catalogue_not_the_link_set() {
    let base = support::fixture("cf68-catalog");
    let project = base.join("project");
    write(&project.join(".kittify/charter/charter.yaml"), CATALOG);
    git_init(&project);
    let identity = sk_doctrine::IdentityIndex::from_roots(
        &support::repository_root().join("packs/built-in"),
        &[],
        None,
    );
    let entries = sk_doctrine::load_reference_catalog(&project, &identity).expect("catalogue");
    assert_eq!(entries.len(), 3, "an unresolvable local_path still counts");
    assert_eq!(
        entries[0].id.to_scalar_string().as_deref(),
        Ok("directive:DIRECTIVE_001")
    );
    assert_eq!(entries[1].kind.to_scalar_string().as_deref(), Ok("tactic"));
    assert_eq!(
        entries[2].summary.to_scalar_string().as_deref(),
        Ok("three")
    );
    assert_eq!(
        entries[1].local_path.to_scalar_string().as_deref(),
        Ok("tactics/architecture/no-such-file.tactic.yaml")
    );

    // An absent charter.yaml is the empty catalogue, not an error.
    let absent = base.join("absent");
    std::fs::create_dir_all(absent.join(".kittify")).expect("absent dir");
    assert!(
        sk_doctrine::load_reference_catalog(&absent, &identity)
            .expect("absent catalogue")
            .is_empty()
    );

    // A non-list `references` is the empty catalogue too.
    let malformed = base.join("malformed");
    write(
        &malformed.join(".kittify/charter/charter.yaml"),
        "catalog:\n  references: {not: a list}\n",
    );
    assert!(
        sk_doctrine::load_reference_catalog(&malformed, &identity)
            .expect("non-list catalogue")
            .is_empty()
    );

    // An unparseable charter.yaml is the empty catalogue (the reference's catch).
    let broken = base.join("broken");
    write(
        &broken.join(".kittify/charter/charter.yaml"),
        "catalog: [unclosed\n",
    );
    assert!(
        sk_doctrine::load_reference_catalog(&broken, &identity)
            .expect("unparseable catalogue")
            .is_empty()
    );
}
