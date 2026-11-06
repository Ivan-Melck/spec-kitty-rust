//! CF65 — the ONE live action-context consumer of the charter-bundle freshness
//! gate, proven against the actual reference and against the real binary.
//!
//! The grant for this lane is narrow and deliberate: `charter context`'s
//! `context_payload` gains the Source freshness/sync call at the Source phase
//! (before context-state preparation, before the charter prose read), passes the
//! resolved canonical read root to that ordinary render read, and the render read
//! itself keeps Source's `exists()` → plain strict-UTF-8 phases (a BOM stays
//! authored text; a directory or an invalid-UTF-8 body raises instead of quietly
//! rendering without prose). Nothing else about the command changes.
//!
//! Controls:
//!
//! * a **paired library child** — the pinned Source child calls the actual
//!   `charter.activation.context.build_charter_context`, the Native child is this
//!   test binary re-executed calling [`sk_cli::compat::charter::context_payload`];
//!   each asserts its own CWD, HOME, all seven XDG roots, TMP/TMPDIR/TEMP,
//!   absolute `PYTHONPATH`, pinned packs root and `PYTHONHASHSEED` before any
//!   business read, in independent fixture trees;
//! * real-binary command controls for the operator-visible exit/payload/record
//!   facts (a directory at the prose path, an unprovable charter, the
//!   CWD-vs-read-root placement and the BOM).
//!
//! Known, disclosed history: this file originally recorded that the Native
//! command composed one build and appended one record for `--json`, while the
//! reference CLI built the context twice (once for the text, once for the
//! structured payload) and appended two. CF66 implemented the reference's
//! ordered execution (`execute_context`), so the real-binary JSON controls below
//! now assert **two** stale-bundle ingestions and the text-mode controls keep
//! one. The paired library control still compares the deliberately single-build
//! `context_payload` helper on both sides.

#[path = "cf65_support/mod.rs"]
mod support;

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value as JsonValue, json};

const CHARTER_BODY: &str = "# Probe Charter\n\n## Policy Summary\n\n- Keep one canonical authority.\n\n## Terminology Canon\n\nUse Mission, never Feature.\n";

const EXTRA_XDG: [(&str, &str); 2] = support::EXTRA_XDG;

/// The cases the paired library control compares.
const PAIRED_CASES: [&str; 5] = [
    "stale",
    "fresh",
    "bom",
    "prose-is-a-directory",
    "subdir-cwd",
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

fn fresh_metadata(body: &str) -> String {
    format!(
        "charter_hash: {}\ntimestamp_utc: '2026-01-01T00:00:00+00:00'\n",
        sk_charter::hash_content(body)
    )
}

/// One case directory: a real Git project with a charter bundle in that shape.
fn build_case(root: &Path, shape: &str) {
    std::fs::create_dir_all(root).expect("case dir");
    git_init(root);
    let charter_dir = root.join(".kittify/charter");
    match shape {
        "stale" => write(&charter_dir.join("charter.md"), CHARTER_BODY),
        "fresh" => {
            write(&charter_dir.join("charter.md"), CHARTER_BODY);
            write(
                &charter_dir.join("metadata.yaml"),
                &fresh_metadata(CHARTER_BODY),
            );
        }
        "bom" => write(
            &charter_dir.join("charter.md"),
            &format!("\u{feff}{CHARTER_BODY}"),
        ),
        "prose-is-a-directory" => {
            std::fs::create_dir_all(charter_dir.join("charter.md")).expect("dir prose");
        }
        "subdir-cwd" => {
            write(&charter_dir.join("charter.md"), CHARTER_BODY);
            std::fs::create_dir_all(root.join("pkg")).expect("pkg dir");
        }
        "absent" => {
            std::fs::create_dir_all(&charter_dir).expect("charter dir");
        }
        other => panic!("unknown case shape {other}"),
    }
}

fn build_cases(root: &Path) {
    std::fs::create_dir_all(root.join("cases")).expect("cases dir");
    for shape in PAIRED_CASES {
        build_case(&root.join("cases").join(shape), shape);
    }
    build_case(&root.join("cases/absent"), "absent");
}

/// The project root the child must use for `subdir-cwd` (the CWD is its `pkg/`).
fn case_cwd(root: &Path, shape: &str) -> PathBuf {
    let case = root.join("cases").join(shape);
    if shape == "subdir-cwd" {
        case.join("pkg")
    } else {
        case
    }
}

fn global_log(root: &Path) -> Vec<JsonValue> {
    let path = root.join(".kittify/encoding-provenance/global.jsonl");
    match std::fs::read_to_string(&path) {
        Ok(text) => text
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_str(line).expect("record JSON"))
            .collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => panic!("unreadable journal {}: {error}", path.display()),
    }
}

use support::isolated_command;

/// The Source child's script: assert the environment, then run the actual
/// single-build context seam per case.
const SOURCE_PROBE: &str = r#"
import json, os, pathlib, sys

root = pathlib.Path(os.environ["SK_CF65_CLI_ROOT"])
cwd = pathlib.Path.cwd()
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

from charter.activation.context import build_charter_context

cases = []
for name in ["stale", "fresh", "bom", "prose-is-a-directory", "subdir-cwd", "absent"]:
    case_root = root / "cases" / name
    workdir = case_root / "pkg" if name == "subdir-cwd" else case_root
    os.chdir(workdir)
    journal = workdir / ".kittify/encoding-provenance/global.jsonl"
    before = len(journal.read_text(encoding="utf-8").splitlines()) if journal.exists() else 0
    entry = {"case": name, "cwd": str(workdir), "ok": True, "mode": None,
             "first_load": None, "records": 0, "record_encodings": [],
             "record_paths": [], "has_policy_summary": False, "error_class": None}
    try:
        result = build_charter_context(
            case_root, action="implement", mission_type="software-dev",
            mark_loaded=False,
        )
    except Exception as exc:  # the reference's own escape is the result
        entry["ok"] = False
        entry["error_class"] = type(exc).__name__
    else:
        entry["mode"] = result.mode
        entry["first_load"] = bool(result.first_load)
        entry["has_policy_summary"] = "Policy Summary:" in result.text
    lines = [line for line in journal.read_text(encoding="utf-8").splitlines() if line] if journal.exists() else []
    new = lines[before:]
    entry["records"] = len(new)
    entry["record_encodings"] = [json.loads(line)["source_encoding"] for line in new]
    entry["record_paths"] = [json.loads(line)["file_path"] for line in new]
    cases.append(entry)

payload = {"runner": "source-python-" + ".".join(str(part) for part in sys.version_info[:3]),
           "cases": cases}
print("SOURCE_RECEIPT " + json.dumps(payload))
"#;

fn run_source(root: &Path) -> Vec<JsonValue> {
    let output = isolated_command(root, support::python())
        .arg("-c")
        .arg(SOURCE_PROBE)
        .env("SK_CF65_CLI_ROOT", root)
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
        .find(|line| line.starts_with("SOURCE_RECEIPT "))
        .unwrap_or_else(|| panic!("no Source receipt in {stdout:?}"));
    let payload: JsonValue =
        serde_json::from_str(line["SOURCE_RECEIPT ".len()..].trim()).expect("receipt JSON");
    assert_eq!(payload["runner"], json!("source-python-3.11.15"));
    payload["cases"].as_array().expect("cases").clone()
}

/// The Native child: this test binary re-executed with the fixture root.
#[test]
fn cf65_native_context_child() {
    let Some(root) = std::env::var_os("SK_CF65_CLI_NATIVE_ROOT").map(PathBuf::from) else {
        return; // ordinary run: the parent test drives this child
    };
    support::assert_isolated(&root);
    for (key, name) in EXTRA_XDG {
        assert_eq!(
            std::env::var_os(key).map(PathBuf::from),
            Some(root.join(name)),
            "{key} is the isolated fixture root"
        );
    }
    assert_eq!(std::env::var("PYTHONHASHSEED").as_deref(), Ok("0"));

    let mut cases = Vec::new();
    for name in [
        "stale",
        "fresh",
        "bom",
        "prose-is-a-directory",
        "subdir-cwd",
        "absent",
    ] {
        let case_root = root.join("cases").join(name);
        let workdir = case_cwd(&root, name);
        // The child runs one case at a time with its own CWD, exactly like the
        // operator invoking the command from that directory.
        std::env::set_current_dir(&workdir).expect("child cwd");
        assert_eq!(std::env::current_dir().expect("cwd"), workdir);
        let before = global_log(&workdir).len();
        let mut entry = json!({
            "case": name,
            "cwd": workdir.display().to_string(),
            "ok": true,
            "mode": JsonValue::Null,
            "first_load": JsonValue::Null,
            "records": 0,
            "record_encodings": Vec::<String>::new(),
            "record_paths": Vec::<String>::new(),
            "has_policy_summary": false,
            "error_class": JsonValue::Null,
        });
        match sk_cli::compat::charter::context_payload(
            &case_root,
            "implement",
            Some("software-dev"),
            None,
            false,
        ) {
            Ok(outcome) => {
                let text = outcome.text.to_scalar_string().unwrap_or_default();
                entry["mode"] = json!(match outcome.mode {
                    sk_doctrine::ContextMode::Bootstrap => "bootstrap",
                    sk_doctrine::ContextMode::Compact => "compact",
                    // Mechanical ripple: the single-build helper never takes the
                    // text-only `missing` rail.
                    sk_doctrine::ContextMode::Missing => "missing",
                });
                entry["first_load"] = json!(outcome.first_load);
                entry["has_policy_summary"] = json!(text.contains("Policy Summary:"));
            }
            Err(error) => {
                entry["ok"] = json!(false);
                entry["error_class"] = json!(error.message());
            }
        }
        // The placement fact is read from the CWD the child actually ran in.
        let records = global_log(&workdir);
        let new: Vec<&JsonValue> = records.iter().skip(before).collect();
        entry["records"] = json!(new.len());
        entry["record_encodings"] = json!(
            new.iter()
                .map(|record| record["source_encoding"].as_str().unwrap_or_default())
                .collect::<Vec<_>>()
        );
        entry["record_paths"] = json!(
            new.iter()
                .map(|record| record["file_path"].as_str().unwrap_or_default())
                .collect::<Vec<_>>()
        );
        cases.push(entry);
    }
    println!(
        "NATIVE_RECEIPT {}",
        json!({"runner": "native", "cases": cases})
    );
}

fn run_native_child(root: &Path) -> Vec<JsonValue> {
    let executable = std::env::current_exe().expect("test binary");
    let output = isolated_command(root, executable)
        .args(["--exact", "cf65_native_context_child", "--nocapture"])
        .env("SK_CF65_CLI_NATIVE_ROOT", root)
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
        .find(|line| line.starts_with("NATIVE_RECEIPT "))
        .unwrap_or_else(|| panic!("no Native receipt in {stdout:?}"));
    let payload: JsonValue =
        serde_json::from_str(line["NATIVE_RECEIPT ".len()..].trim()).expect("receipt JSON");
    assert_eq!(payload["runner"], json!("native"));
    payload["cases"].as_array().expect("cases").clone()
}

#[test]
fn source_and_native_context_consumers_agree_on_the_written_phases() {
    let source_root = support::fixture("cf65-cli-source");
    build_cases(&source_root);
    let source = run_source(&source_root);

    let by_case = |cases: &[JsonValue], name: &str| -> JsonValue {
        cases
            .iter()
            .find(|case| case["case"] == json!(name))
            .unwrap_or_else(|| panic!("no {name} case"))
            .clone()
    };

    // Source-first: the reference's own answers are asserted before the Native run.
    let stale = by_case(&source, "stale");
    assert_eq!(stale["ok"], json!(true), "{stale}");
    assert_eq!(stale["mode"], json!("bootstrap"));
    assert_eq!(stale["has_policy_summary"], json!(true));
    assert_eq!(stale["records"], json!(1), "one single-build read");
    assert_eq!(stale["record_encodings"], json!(["utf-8"]));
    assert_eq!(
        stale["record_paths"],
        json!([source_root
            .join("cases/stale/.kittify/charter/charter.md")
            .display()
            .to_string()])
    );

    let fresh = by_case(&source, "fresh");
    assert_eq!(fresh["records"], json!(0), "a fresh bundle is not ingested");

    let bom = by_case(&source, "bom");
    assert_eq!(bom["ok"], json!(true), "a BOM charter renders");
    assert_eq!(bom["has_policy_summary"], json!(true));
    assert_eq!(
        bom["record_encodings"],
        json!(["utf-8-sig"]),
        "the provenance read strips the BOM the render kept"
    );

    let broken = by_case(&source, "prose-is-a-directory");
    assert_eq!(broken["ok"], json!(false), "a directory is a read error");
    assert_eq!(broken["records"], json!(0));

    let subdir = by_case(&source, "subdir-cwd");
    assert_eq!(subdir["ok"], json!(true));
    assert_eq!(
        subdir["records"],
        json!(1),
        "the record follows the process CWD"
    );
    assert_eq!(
        subdir["record_paths"],
        json!([source_root
            .join("cases/subdir-cwd/.kittify/charter/charter.md")
            .display()
            .to_string()]),
        "the read root is the project, not the CWD"
    );

    let absent = by_case(&source, "absent");
    assert_eq!(absent["records"], json!(0), "no prose, no ingestion");

    let native_root = support::fixture("cf65-cli-native");
    build_cases(&native_root);
    let native = run_native_child(&native_root);

    for name in PAIRED_CASES {
        let source_case = by_case(&source, name);
        let native_case = by_case(&native, name);
        assert_eq!(
            source_case["ok"], native_case["ok"],
            "case {name}: ok differs (Source {source_case} vs Native {native_case})"
        );
        assert_eq!(
            source_case["records"], native_case["records"],
            "case {name}: provenance record count differs (Source {source_case} vs Native {native_case})"
        );
        assert_eq!(
            source_case["record_encodings"], native_case["record_encodings"],
            "case {name}: recorded encodings differ"
        );
        if name != "subdir-cwd" {
            // `subdir-cwd` reads the project charter from the CWD, so the two
            // fixtures' absolute paths necessarily differ; placement, not the
            // spelling, is the compared fact (asserted per side below).
            assert_eq!(
                source_case["record_paths"].as_array().map(Vec::len),
                native_case["record_paths"].as_array().map(Vec::len),
                "case {name}: record count in the placement list differs"
            );
        }
        if source_case["ok"] == json!(true) {
            assert_eq!(
                source_case["mode"], native_case["mode"],
                "case {name}: mode differs"
            );
            assert_eq!(
                source_case["first_load"], native_case["first_load"],
                "case {name}: first_load differs"
            );
            assert_eq!(
                source_case["has_policy_summary"], native_case["has_policy_summary"],
                "case {name}: rendered policy summary differs"
            );
        }
    }

    // Placement is the load-bearing fact for the CWD case: the record is under the
    // child's own CWD `pkg/`, never under the project root.
    let native_subdir = by_case(&native, "subdir-cwd");
    assert_eq!(
        native_subdir["record_paths"],
        json!([native_root
            .join("cases/subdir-cwd/.kittify/charter/charter.md")
            .display()
            .to_string()])
    );
    assert!(
        global_log(&native_root.join("cases/subdir-cwd")).is_empty(),
        "the read root receives no record for a CWD-placed read"
    );
    assert!(
        global_log(&source_root.join("cases/subdir-cwd")).is_empty(),
        "the reference keeps the same placement"
    );

    // The absent-prose case is compared on the write side only: Native's
    // charter-presence gate (mode "missing") is a recorded parent-owned gap, so
    // its rendered mode is not claimed here.
    let native_absent = by_case(&native, "absent");
    assert_eq!(native_absent["records"], json!(0));
}

// ---------------------------------------------------------------------------
// Real-binary command controls
// ---------------------------------------------------------------------------

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_spec-kitty"))
}

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// Run the real command in `cwd` (project root supplied by `SPECIFY_REPO_ROOT`).
fn run_command(cwd: &Path, args: &[&str], extra_env: &[(&str, &str)]) -> Run {
    // A separate, throwaway root set for HOME/XDG/TMP: the child's own CWD is the
    // fixture the assertions read.
    let roots = tempfile::tempdir().expect("isolated roots");
    for (_, name) in support::ROOTS {
        std::fs::create_dir_all(roots.path().join(name)).expect("isolated root");
    }
    for (_, name) in EXTRA_XDG {
        std::fs::create_dir_all(roots.path().join(name)).expect("isolated root");
    }
    let mut command = isolated_command(roots.path(), binary());
    command.current_dir(cwd);
    command.args(args);
    for (key, value) in extra_env {
        command.env(key, value);
    }
    let output = command.output().expect("run the real binary");
    Run {
        code: output.status.code(),
        stdout: String::from_utf8(output.stdout).expect("stdout is UTF-8"),
        stderr: String::from_utf8(output.stderr).expect("stderr is UTF-8"),
    }
}

fn project_fixture(shape: &str) -> tempfile::TempDir {
    let temp = tempfile::tempdir().expect("temp");
    build_case(temp.path(), shape);
    temp
}

fn context_args(json_output: bool) -> Vec<&'static str> {
    let mut args = vec![
        "charter",
        "context",
        "--action",
        "implement",
        "--mission-type",
        "software-dev",
        "--no-mark-loaded",
    ];
    if json_output {
        args.push("--json");
    }
    args
}

#[test]
fn the_live_consumer_records_one_provenance_read_for_a_stale_bundle() {
    let temp = project_fixture("stale");
    let run = run_command(temp.path(), &context_args(true), &[]);
    assert_eq!(run.code, Some(0), "{}{}", run.stdout, run.stderr);

    let payload: JsonValue = serde_json::from_str(&run.stdout).expect("one JSON document");
    assert_eq!(payload["result"], json!("success"));
    assert_eq!(payload["mode"], json!("bootstrap"));
    assert!(
        payload["context"]
            .as_str()
            .expect("context text")
            .contains("Policy Summary:")
    );

    // CF66: the reference CLI builds the text then the structured payload for
    // `--json`, so a stale bundle is ingested twice. (This test's name records
    // the pre-CF66 single-build assertion for continuity; the reference never
    // made one read.)
    let records = global_log(temp.path());
    assert_eq!(
        records.len(),
        2,
        "the reference's two builds ingest twice: {records:?}"
    );
    assert_eq!(
        records[0]["file_path"],
        json!(
            temp.path()
                .join(".kittify/charter/charter.md")
                .display()
                .to_string()
        )
    );
    assert_eq!(records[0]["source_encoding"], json!("utf-8"));
    assert_eq!(records[0]["actor"], json!("spec-kitty charter"));
    assert_eq!(records[0]["mission_id"], JsonValue::Null);
    for derived in ["governance.yaml", "directives.yaml"] {
        assert!(
            !temp.path().join(".kittify/charter").join(derived).exists(),
            "the retired sync writes no derived artifact"
        );
    }
}

#[test]
fn a_fresh_bundle_and_absent_prose_write_no_record() {
    let fresh = project_fixture("fresh");
    let run = run_command(fresh.path(), &context_args(true), &[]);
    assert_eq!(run.code, Some(0), "{}{}", run.stdout, run.stderr);
    assert!(
        global_log(fresh.path()).is_empty(),
        "a complete fresh bundle is not ingested"
    );

    let absent = project_fixture("absent");
    let run = run_command(absent.path(), &context_args(true), &[]);
    assert_eq!(run.code, Some(0), "{}{}", run.stdout, run.stderr);
    assert!(
        global_log(absent.path()).is_empty(),
        "healthy absence records nothing"
    );
}

#[test]
fn a_directory_at_the_prose_path_is_a_real_read_error_not_healthy_absence() {
    let temp = project_fixture("prose-is-a-directory");
    let run = run_command(temp.path(), &context_args(true), &[]);

    assert_eq!(
        run.code,
        Some(1),
        "the reference exits 1 here: {}",
        run.stdout
    );
    let payload: JsonValue = serde_json::from_str(&run.stdout).expect("error envelope");
    assert!(
        payload.get("error").and_then(JsonValue::as_str).is_some(),
        "the command reports an error document: {payload}"
    );
    assert!(
        payload.get("context").is_none() && payload.get("result").is_none(),
        "no successful render is emitted: {payload}"
    );
    assert!(
        global_log(temp.path()).is_empty(),
        "an unreadable prose path appends nothing"
    );
}

#[test]
fn an_unprovable_charter_refuses_instead_of_rendering_without_prose() {
    // The bytes are not UTF-8 and carry no BOM. The reference's charset
    // tie-break could recover some of these; Native's proven subset refuses
    // (the recorded CF64 ingestion gap). What this control pins is the phase:
    // no successful render, no fabricated provenance record.
    let temp = project_fixture("stale");
    std::fs::write(
        temp.path().join(".kittify/charter/charter.md"),
        b"# Caf\xe9 charter\n\n## Policy Summary\n\n- one\n",
    )
    .expect("unprovable charter");
    let run = run_command(temp.path(), &context_args(true), &[]);

    assert_eq!(run.code, Some(1), "{}", run.stdout);
    let payload: JsonValue = serde_json::from_str(&run.stdout).expect("error envelope");
    assert!(
        payload.get("error").and_then(JsonValue::as_str).is_some(),
        "the refusal is reported as an error document: {payload}"
    );
    assert!(payload.get("result").is_none() && payload.get("context").is_none());
    assert!(global_log(temp.path()).is_empty());
}

#[test]
fn the_record_follows_the_process_cwd_not_the_read_root() {
    let temp = project_fixture("subdir-cwd");
    let project = temp.path();
    let cwd = project.join("pkg");
    let run = run_command(
        &cwd,
        &context_args(true),
        &[("SPECIFY_REPO_ROOT", &project.display().to_string())],
    );
    assert_eq!(run.code, Some(0), "{}{}", run.stdout, run.stderr);

    let cwd_records = global_log(&cwd);
    assert_eq!(
        cwd_records.len(),
        2,
        "both reference builds place their record beside the CWD"
    );
    assert_eq!(
        cwd_records[0]["file_path"],
        json!(
            project
                .join(".kittify/charter/charter.md")
                .display()
                .to_string()
        ),
        "the read itself still uses the resolved project root"
    );
    assert!(
        global_log(project).is_empty(),
        "the read root must not receive a CWD-placed record"
    );
}

#[test]
fn a_bom_charter_renders_and_the_provenance_read_strips_it() {
    let temp = project_fixture("bom");
    let run = run_command(temp.path(), &context_args(false), &[]);
    assert_eq!(run.code, Some(0), "{}{}", run.stdout, run.stderr);
    assert!(
        run.stdout.contains("Policy Summary:"),
        "the bootstrap render keeps the authored BOM and still finds the section: {}",
        run.stdout
    );

    let records = global_log(temp.path());
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["source_encoding"], json!("utf-8-sig"));
    assert_eq!(
        records[0]["normalization_applied"],
        json!(true),
        "the provenance read re-encoded away from plain UTF-8"
    );
}
