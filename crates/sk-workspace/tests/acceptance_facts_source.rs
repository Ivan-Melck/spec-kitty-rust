//! Source-**first** paired facts proof for `sk_workspace::acceptance_source`.
//!
//! Each case runs the PINNED Source first (`.venv-rust-oracle`, CPython 3.11.15,
//! fail-closed `verify_oracle.py`) in a fully isolated sandbox — fresh CWD, HOME,
//! every `XDG_*` root including `XDG_RUNTIME_DIR`, and `TMPDIR`/`TMP`/`TEMP` —
//! ASSERTS the intended cause from the actual Source receipt, and only then runs
//! the equivalent Native collection over an identical fixture.
//!
//! The receipts (task report `S-dirty-lanes`, `S-dirty-coord`, `S-decisions`) are
//! what makes these tests behavioural rather than self-referential: the expected
//! dirty set and every decision-status verdict come from executing
//! `specify_cli.acceptance`, not from this file's opinion.
//!
//! Deliberately NOT covered here because the Native authority does not exist yet
//! (see the task report's interface requests):
//! * complete Source-default acceptance and physical write/retry behavior;
//! * mission path conventions / declared artifacts integration.
//!
//! Historical L2 Native legs ran in Cargo's process; the parent now re-executes
//! the Native leg with actual per-fixture CWD/environment assertions.

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;
mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sk_mission::{Mission, list_missions};
use sk_status::acceptance::source::{
    PathConventionFacts, SourceAcceptanceOptions, evaluate_source_acceptance,
};
use sk_workspace::acceptance_source::{MatrixInput, collect_source_acceptance_facts};

use support::{commit_all, init_repo, mission_id, write, write_mission};

const SLUG: &str = "001-dirty-probe";
const MISSION_ID_TAIL: u8 = 7;

// ---------------------------------------------------------------------------
// Sandbox
// ---------------------------------------------------------------------------

/// An isolated sandbox whose every OS root is a fresh directory under it.
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("sandbox");
        for directory in [
            "home", "cache", "config", "data", "state", "runtime", "tmp", "cwd", "probe",
        ] {
            fs::create_dir_all(temp.path().join(directory)).expect("sandbox root");
        }
        Self { root: temp.keep() }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn native_receipt() -> Option<serde_json::Value> {
    let root = PathBuf::from(std::env::var_os("SK_ACCEPTANCE_FACTS_NATIVE")?);
    assert_eq!(std::env::current_dir().unwrap(), root.join("cwd"));
    for (key, name) in [
        ("HOME", "home"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_RUNTIME_DIR", "runtime"),
        ("TMPDIR", "tmp"),
        ("TMP", "tmp"),
        ("TEMP", "tmp"),
    ] {
        assert_eq!(std::env::var_os(key).unwrap(), root.join(name));
    }
    assert_eq!(
        std::env::var_os("PYTHONPATH").unwrap(),
        repo_root().join("src")
    );
    assert_eq!(
        std::env::var_os("SPEC_KITTY_PACKS_ROOT").unwrap(),
        repo_root().join("packs")
    );
    Some(serde_json::from_slice(&fs::read(root.join("native-receipt.json")).unwrap()).unwrap())
}

fn native_reexec(sandbox: &Sandbox, test: &str, receipt: &serde_json::Value) {
    fs::write(
        sandbox.path("native-receipt.json"),
        serde_json::to_vec(receipt).unwrap(),
    )
    .unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", test, "--nocapture"])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .current_dir(sandbox.path("cwd"))
        .env("SK_ACCEPTANCE_FACTS_NATIVE", &sandbox.root)
        .env("PYTHONPATH", repo_root().join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo_root().join("packs"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONUTF8", "1")
        .env("LC_ALL", "C.UTF-8");
    for (key, name) in [
        ("HOME", "home"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_RUNTIME_DIR", "runtime"),
        ("TMPDIR", "tmp"),
        ("TMP", "tmp"),
        ("TEMP", "tmp"),
    ] {
        command.env(key, sandbox.path(name));
    }
    let output = command.output().unwrap();
    fs::write(sandbox.path("native.stdout"), &output.stdout).unwrap();
    fs::write(sandbox.path("native.stderr"), &output.stderr).unwrap();
    fs::write(
        sandbox.path("native.status"),
        format!("{:?}\n", output.status.code()),
    )
    .unwrap();
    assert!(
        output.status.success(),
        "actual isolated Native: {} {}",
        text(&output.stdout),
        text(&output.stderr)
    );
}

/// Run one oracle script with the full isolated environment and return its JSON.
///
/// The child itself asserts that its CWD/HOME/XDG/TMP roots are the sandbox's,
/// and echoes them so this side can check the same values independently. A
/// missing oracle, a drifted pin, or a leaked host root is a premise failure,
/// never a passing zero-comparison run.
fn run_oracle(
    sandbox: &Sandbox,
    script_name: &str,
    script: &str,
    args: &[&str],
) -> serde_json::Value {
    let script_path = sandbox.path(script_name);
    fs::write(&script_path, script).expect("write oracle script");
    let output: Output = Command::new(oracle::python())
        .arg(&script_path)
        .args(args)
        .current_dir(sandbox.path("cwd"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", sandbox.path("home"))
        .env("XDG_CACHE_HOME", sandbox.path("cache"))
        .env("XDG_CONFIG_HOME", sandbox.path("config"))
        .env("XDG_DATA_HOME", sandbox.path("data"))
        .env("XDG_STATE_HOME", sandbox.path("state"))
        .env("XDG_RUNTIME_DIR", sandbox.path("runtime"))
        .env("TMPDIR", sandbox.path("tmp"))
        .env("TMP", sandbox.path("tmp"))
        .env("TEMP", sandbox.path("tmp"))
        .env("PROBE_ROOT", sandbox.path("probe"))
        .env("PYTHONPATH", repo_root().join("src"))
        .env(
            "SPEC_KITTY_PACKS_ROOT",
            repo_root().join("packs").display().to_string(),
        )
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONUTF8", "1")
        .env("LC_ALL", "C.UTF-8")
        .env("SPEC_KITTY_NO_UPGRADE_CHECK", "1")
        .output()
        .expect("run the pinned Source oracle");
    fs::write(
        sandbox.path(&format!("{script_name}.stdout")),
        &output.stdout,
    )
    .unwrap();
    fs::write(
        sandbox.path(&format!("{script_name}.stderr")),
        &output.stderr,
    )
    .unwrap();
    fs::write(
        sandbox.path(&format!("{script_name}.status")),
        format!("{:?}\n", output.status.code()),
    )
    .unwrap();
    assert!(
        output.status.success(),
        "the pinned Source oracle failed (harness/premise failure, not a business red):\n{}\n{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    let receipt: serde_json::Value =
        serde_json::from_str(&text(&output.stdout)).unwrap_or_else(|error| {
            panic!(
                "oracle receipt was not JSON ({error}): {}",
                text(&output.stdout)
            )
        });
    // Independent check of the isolation facts the child printed.
    let expected = [
        ("home", sandbox.path("home")),
        ("xdg_config_home", sandbox.path("config")),
        ("xdg_cache_home", sandbox.path("cache")),
        ("xdg_data_home", sandbox.path("data")),
        ("xdg_state_home", sandbox.path("state")),
        ("xdg_runtime_dir", sandbox.path("runtime")),
        ("tmpdir", sandbox.path("tmp")),
        ("tmp", sandbox.path("tmp")),
        ("temp", sandbox.path("tmp")),
        ("cwd", sandbox.path("cwd")),
    ];
    for (key, path) in expected {
        assert_eq!(
            receipt.get(key).and_then(|value| value.as_str()),
            Some(path.display().to_string().as_str()),
            "the Source run did not execute in the isolated `{key}` root"
        );
    }
    receipt
}

// ---------------------------------------------------------------------------
// Probe 1 — the accept dirty gate
// ---------------------------------------------------------------------------

/// Builds the SAME fixture in Source's own temp git repositories and reports
/// `_accept_dirty_gate` for a flat (`lanes`) and a coordination topology.
const DIRTY_ORACLE: &str = r##"
import json, os, subprocess, sys
from pathlib import Path

SLUG = "001-dirty-probe"
MISSION_ID = sys.argv[1]
root = Path(os.environ["PROBE_ROOT"])

def git(repo, *args):
    subprocess.run(["git", *args], cwd=repo, capture_output=True, text=True, check=True)

def write(path, text):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")

def build(topology):
    repo = root / f"repo-{topology}"
    repo.mkdir()
    git(repo, "init", "-q", "-b", "main")
    git(repo, "config", "user.email", "probe@example.invalid")
    git(repo, "config", "user.name", "Probe")
    feature = repo / "kitty-specs" / SLUG
    write(feature / "meta.json", json.dumps({
        "mission_id": MISSION_ID,
        "mission_slug": SLUG,
        "topology": topology,
        "target_branch": "main",
        "mission_type": "software-dev",
    }, indent=2) + "\n")
    write(feature / "spec.md", "# Spec\n")
    write(feature / "plan.md", "# Plan\n")
    write(feature / "tasks.md", "- [x] T001 done\n")
    write(feature / "tasks" / "WP01.md", "---\nwork_package_id: WP01\n---\n")
    write(repo / "src" / "lib.rs", "fn main() {}\n")
    git(repo, "add", "-A")
    git(repo, "commit", "-qm", "fixture")
    return repo

def dirty(repo):
    from specify_cli.acceptance import _accept_dirty_gate
    from specify_cli.task_utils import git_status_entries
    return _accept_dirty_gate(git_status_entries(repo), repo_root=repo, feature=SLUG)

receipt = {"cases": {}, "raw": {}}
for topology in ("lanes", "coord"):
    repo = build(topology)
    feature = repo / "kitty-specs" / SLUG
    write(feature / "acceptance-matrix.json", '{"mission_slug": "%s"}\n' % SLUG)
    write(feature / "status.json", "{}\n")
    write(feature / "decisions" / "index.json", '{"version": 1, "mission_id": "", "entries": []}\n')
    write(feature / "decisions" / "DM-01ABC.md", "# Decision Moment\n")
    write(feature / "traces" / "t1.md", "trace\n")
    write(feature / "status.events.jsonl", "")
    write(repo / "src" / "main.rs", "fn extra() {}\n")
    from specify_cli.task_utils import git_status_entries
    receipt["raw"][topology] = [entry.display() for entry in git_status_entries(repo)]
    receipt["cases"][topology] = dirty(repo)
receipt["python_version"] = "%d.%d.%d" % sys.version_info[:3]
receipt["cwd"] = str(Path.cwd())
receipt["home"] = os.environ.get("HOME")
receipt["xdg_config_home"] = os.environ.get("XDG_CONFIG_HOME")
receipt["xdg_cache_home"] = os.environ.get("XDG_CACHE_HOME")
receipt["xdg_data_home"] = os.environ.get("XDG_DATA_HOME")
receipt["xdg_state_home"] = os.environ.get("XDG_STATE_HOME")
receipt["xdg_runtime_dir"] = os.environ.get("XDG_RUNTIME_DIR")
receipt["tmpdir"] = os.environ.get("TMPDIR")
receipt["tmp"] = os.environ.get("TMP")
receipt["temp"] = os.environ.get("TEMP")
print(json.dumps(receipt, indent=2))
"##;

// ---------------------------------------------------------------------------
// Probe 2 — clarification decision statuses
// ---------------------------------------------------------------------------

/// `acceptance._has_blocking_clarification_marker` per ledger status, plus the
/// unknown-id / absent-ledger / prose / corrupt-ledger boundaries.
const DECISION_ORACLE: &str = r##"
import json, os, sys
from pathlib import Path

MISSION_ID = sys.argv[1]
root = Path(os.environ["PROBE_ROOT"])

def entry(decision_id, status):
    return {
        "decision_id": decision_id,
        "origin_flow": "specify",
        "step_id": "specify",
        "input_key": "q",
        "question": "Which?",
        "options": ["a", "b"],
        "status": status,
        "created_at": "2026-01-01T00:00:00Z",
        "mission_id": MISSION_ID,
        "mission_slug": "001-decision-probe",
    }

def marker(decision_id):
    return f"# Spec\n\n- [NEEDS CLARIFICATION: which one?] <!-- decision_id: {decision_id} -->\n"

def case_dir(name):
    d = root / name
    (d / "decisions").mkdir(parents=True)
    return d

from specify_cli.acceptance import _has_blocking_clarification_marker

def judge(path):
    return _has_blocking_clarification_marker(path, path.read_text(encoding="utf-8"))

receipt = {"cases": {}}
for label, status in (
    ("resolved", "resolved"),
    ("open", "open"),
    ("deferred", "deferred"),
    ("canceled", "canceled"),
):
    d = case_dir(label)
    decision_id = f"DM-{label.upper()}"
    (d / "spec.md").write_text(marker(decision_id), encoding="utf-8")
    (d / "decisions" / "index.json").write_text(
        json.dumps({"version": 1, "mission_id": MISSION_ID, "entries": [entry(decision_id, status)]}),
        encoding="utf-8",
    )
    receipt["cases"][label] = judge(d / "spec.md")

d = case_dir("unknown")
(d / "spec.md").write_text(marker("DM-ABSENT"), encoding="utf-8")
(d / "decisions" / "index.json").write_text(
    json.dumps({"version": 1, "mission_id": MISSION_ID, "entries": []}), encoding="utf-8"
)
receipt["cases"]["unknown_id"] = judge(d / "spec.md")

d = root / "absent_ledger"
d.mkdir()
(d / "spec.md").write_text(marker("DM-ABSENT"), encoding="utf-8")
receipt["cases"]["absent_ledger"] = judge(d / "spec.md")

d = root / "prose"
d.mkdir()
(d / "spec.md").write_text("# Spec\n\n- [NEEDS CLARIFICATION: unlinked prose]\n", encoding="utf-8")
receipt["cases"]["no_decision_id"] = judge(d / "spec.md")

d = case_dir("corrupt")
(d / "spec.md").write_text(marker("DM-CORRUPT"), encoding="utf-8")
(d / "decisions" / "index.json").write_text("{not json", encoding="utf-8")
try:
    judge(d / "spec.md")
    receipt["cases"]["corrupt_ledger"] = "no-error"
except Exception as exc:
    receipt["cases"]["corrupt_ledger"] = type(exc).__name__

receipt["python_version"] = "%d.%d.%d" % sys.version_info[:3]
receipt["cwd"] = str(Path.cwd())
receipt["home"] = os.environ.get("HOME")
receipt["xdg_config_home"] = os.environ.get("XDG_CONFIG_HOME")
receipt["xdg_cache_home"] = os.environ.get("XDG_CACHE_HOME")
receipt["xdg_data_home"] = os.environ.get("XDG_DATA_HOME")
receipt["xdg_state_home"] = os.environ.get("XDG_STATE_HOME")
receipt["xdg_runtime_dir"] = os.environ.get("XDG_RUNTIME_DIR")
receipt["tmpdir"] = os.environ.get("TMPDIR")
receipt["tmp"] = os.environ.get("TMP")
receipt["temp"] = os.environ.get("TEMP")
print(json.dumps(receipt, indent=2))
"##;

// ---------------------------------------------------------------------------
// Native fixture (identical shape to the oracle's)
// ---------------------------------------------------------------------------

fn strings(value: &serde_json::Value, key: &str) -> Vec<String> {
    value[key]
        .as_array()
        .unwrap_or_else(|| panic!("{key} is not an array: {value}"))
        .iter()
        .map(|entry| entry.as_str().expect("string entry").to_owned())
        .collect()
}

/// Build the same dirty fixture through the Native fixture helpers.
fn native_dirty_fixture(topology: &str) -> (support::Fixture, Mission) {
    let fixture = init_repo();
    let feature_dir = write_mission(
        fixture.root(),
        SLUG,
        &mission_id(MISSION_ID_TAIL),
        topology,
        "main",
    );
    write(&feature_dir.join("spec.md"), "# Spec\n");
    write(&feature_dir.join("plan.md"), "# Plan\n");
    write(&feature_dir.join("tasks.md"), "- [x] T001 done\n");
    write(
        &feature_dir.join("tasks").join("WP01.md"),
        "---\nwork_package_id: WP01\n---\n",
    );
    write(&fixture.root().join("src").join("lib.rs"), "fn main() {}\n");
    commit_all(fixture.root(), "fixture");
    write(
        &feature_dir.join("acceptance-matrix.json"),
        &format!("{{\"mission_slug\": \"{SLUG}\"}}\n"),
    );
    write(&feature_dir.join("status.json"), "{}\n");
    write(
        &feature_dir.join("decisions").join("index.json"),
        "{\"version\": 1, \"mission_id\": \"\", \"entries\": []}\n",
    );
    write(
        &feature_dir.join("decisions").join("DM-01ABC.md"),
        "# Decision Moment\n",
    );
    write(&feature_dir.join("traces").join("t1.md"), "trace\n");
    write(&feature_dir.join("status.events.jsonl"), "");
    write(
        &fixture.root().join("src").join("main.rs"),
        "fn extra() {}\n",
    );
    let mission = list_missions(fixture.root())
        .expect("mission inventory")
        .into_iter()
        .find(|mission| mission.slug().as_str() == SLUG)
        .expect("mission present");
    (fixture, mission)
}

// ---------------------------------------------------------------------------
// Cases
// ---------------------------------------------------------------------------

#[test]
fn source_dirty_gate_receipt_precedes_the_native_collection() {
    if let Some(receipt) = native_receipt() {
        assert_native_dirty(&receipt);
        return;
    }
    let sandbox = Sandbox::new();
    let receipt = run_oracle(
        &sandbox,
        "dirty-oracle.py",
        DIRTY_ORACLE,
        &[&mission_id(MISSION_ID_TAIL)],
    );

    // ---- 1. The pinned Source's actual verdict (asserted BEFORE Native) ----
    assert_eq!(
        receipt["python_version"].as_str(),
        Some("3.11.15"),
        "the oracle must be the pinned CPython"
    );
    // Every fixture path is dirty as far as git is concerned …
    assert_eq!(
        strings(&receipt["raw"], "lanes"),
        vec![
            "?? kitty-specs/001-dirty-probe/acceptance-matrix.json",
            "?? kitty-specs/001-dirty-probe/decisions/",
            "?? kitty-specs/001-dirty-probe/status.events.jsonl",
            "?? kitty-specs/001-dirty-probe/status.json",
            "?? kitty-specs/001-dirty-probe/traces/",
            "?? src/main.rs",
        ]
    );
    // … and Source preserves exactly these three on a FLAT mission: accept-owned
    // writes and the decision ledger are always excluded, `status.events.jsonl`
    // (read-only for accept) and a COORD-kind `traces/` still block.
    assert_eq!(
        strings(&receipt["cases"], "lanes"),
        vec![
            "?? kitty-specs/001-dirty-probe/status.events.jsonl",
            "?? kitty-specs/001-dirty-probe/traces/",
            "?? src/main.rs",
        ]
    );
    // Under a coordination topology the residue leg removes `traces/` too.
    assert_eq!(strings(&receipt["cases"], "coord"), vec!["?? src/main.rs"]);

    native_reexec(
        &sandbox,
        "source_dirty_gate_receipt_precedes_the_native_collection",
        &receipt,
    );
}

fn assert_native_dirty(receipt: &serde_json::Value) {
    // ---- 2. The Native collection must agree, fact for fact ----------------
    for topology in ["lanes", "coord"] {
        let (fixture, mission) = native_dirty_fixture(topology);
        let facts =
            collect_source_acceptance_facts(fixture.root(), &mission, MatrixInput::NotRequired)
                .expect("facts");
        assert_eq!(
            facts.git_dirty,
            strings(&receipt["cases"], topology),
            "the Native dirty set must equal the Source receipt for the `{topology}` topology"
        );
    }
}

#[test]
fn the_collector_resolves_the_actual_mission_definition() {
    let fixture = init_repo();
    let feature_dir = write_mission(
        fixture.root(),
        SLUG,
        &mission_id(MISSION_ID_TAIL),
        "lanes",
        "main",
    );
    write(&feature_dir.join("spec.md"), "# Spec\n");
    write(&feature_dir.join("plan.md"), "# Plan\n");
    std::fs::create_dir_all(feature_dir.join("tasks")).expect("tasks dir");
    write(&feature_dir.join("tasks.md"), "- [x] T001\n");
    // The project-tier Mission definition Source resolves through
    // `get_mission_for_feature` (the pack/global tiers need an installed tree;
    // `cf1_acceptance_providers.rs` covers the installed composition).
    write(
        &fixture
            .path(".kittify")
            .join("missions")
            .join("software-dev")
            .join("mission.yaml"),
        "name: \"Software Dev Kitty\"\ndescription: \"fixture\"\nversion: \"1.0.0\"\ndomain: \"software\"\nworkflow:\n  phases:\n    - name: \"implement\"\n      description: \"implement\"\nartifacts:\n  required: [spec.md, plan.md, tasks.md]\n  optional: [data-model.md, contracts/, quickstart.md, research.md, checklists/]\npaths:\n  workspace: \"src/\"\n  tests: \"tests/\"\n  deliverables: \"contracts/\"\n  documentation: \"docs/\"\n",
    );
    commit_all(fixture.root(), "fixture");
    let mission = list_missions(fixture.root())
        .expect("mission inventory")
        .pop()
        .expect("mission");

    let facts = collect_source_acceptance_facts(fixture.root(), &mission, MatrixInput::NotRequired)
        .expect("facts");
    // `software-dev`'s mission.yaml declares `src/ tests/ docs/ contracts/`; the
    // fact is the canonical validator's real verdict, not a guessed no-op.
    match facts.path_conventions {
        PathConventionFacts::Evaluated {
            satisfied,
            ref blocking_text,
            ref dedup_tokens,
            ..
        } => {
            assert!(!satisfied);
            let blocking = blocking_text.as_deref().expect("blocking text");
            assert!(blocking.contains("workspace path: src"), "{blocking}");
            assert_eq!(dedup_tokens, &vec!["contracts".to_owned()]);
        }
        other => panic!("expected a real path-convention evaluation, got {other:?}"),
    }
    // The declared optional set (authored order), never the old fallback list —
    // notably software-dev's `checklists/` is now warned about.
    assert_eq!(
        facts.missing_optional_artifacts,
        vec![
            "data-model.md".to_owned(),
            "contracts".to_owned(),
            "quickstart.md".to_owned(),
            "research.md".to_owned(),
            "checklists".to_owned(),
        ]
    );
}

#[test]
fn source_absent_ledger_blocks_and_native_reports_the_marker_as_open() {
    if let Some(receipt) = native_receipt() {
        assert_native_decisions(&receipt);
        return;
    }
    let sandbox = Sandbox::new();
    let receipt = run_oracle(
        &sandbox,
        "decision-oracle.py",
        DECISION_ORACLE,
        &[&mission_id(MISSION_ID_TAIL)],
    );

    // ---- 1. The pinned Source's actual verdicts (BEFORE Native) ------------
    assert_eq!(receipt["python_version"].as_str(), Some("3.11.15"));
    let cases = &receipt["cases"];
    assert_eq!(cases["resolved"], serde_json::json!(false));
    assert_eq!(cases["canceled"], serde_json::json!(false));
    assert_eq!(cases["open"], serde_json::json!(true));
    assert_eq!(cases["deferred"], serde_json::json!(true));
    // No ledger file → Source's empty index → the id is unknown → blocking.
    assert_eq!(cases["absent_ledger"], serde_json::json!(true));
    assert_eq!(cases["unknown_id"], serde_json::json!(true));
    // A marker with no `decision_id` comment is prose.
    assert_eq!(cases["no_decision_id"], serde_json::json!(false));
    // A corrupt ledger is an ERROR, never a verdict.
    assert_eq!(
        cases["corrupt_ledger"].as_str(),
        Some("DecisionIndexReadError")
    );

    native_reexec(
        &sandbox,
        "source_absent_ledger_blocks_and_native_reports_the_marker_as_open",
        &receipt,
    );
}

fn assert_native_decisions(receipt: &serde_json::Value) {
    // ---- 2. Native must reproduce the absent-ledger case exactly -----------
    let fixture = init_repo();
    let feature_dir = write_mission(
        fixture.root(),
        SLUG,
        &mission_id(MISSION_ID_TAIL),
        "lanes",
        "main",
    );
    std::fs::create_dir_all(feature_dir.join("tasks")).expect("tasks dir");
    write(
        &feature_dir.join("spec.md"),
        "# Spec\n\n- [NEEDS CLARIFICATION: which one?] <!-- decision_id: DM-ABSENT -->\n",
    );
    commit_all(fixture.root(), "fixture");
    let mission = list_missions(fixture.root())
        .expect("mission inventory")
        .pop()
        .expect("mission");
    let facts = collect_source_acceptance_facts(fixture.root(), &mission, MatrixInput::NotRequired)
        .expect("facts");
    // Source treats a missing `index.json` as an empty index, so availability is
    // TRUE and the empty map makes every marker id unknown → blocking.
    assert!(facts.decision_statuses_available);
    assert!(facts.decision_statuses.is_empty());
    let summary = evaluate_source_acceptance(&facts, SourceAcceptanceOptions::default());
    assert_eq!(
        summary.needs_clarification,
        vec![feature_dir.join("spec.md").display().to_string()]
    );

    // ---- 3. Actual PRESENT resolved entry, unlike L2's old empty fixture ---
    write(
        &feature_dir.join("spec.md"),
        "# Spec\n\n- [NEEDS CLARIFICATION: which one?] <!-- decision_id: DM-RESOLVED -->\n",
    );
    let index = serde_json::json!({"version":1,"mission_id":mission_id(MISSION_ID_TAIL),"entries":[{"decision_id":"DM-RESOLVED","origin_flow":"specify","step_id":"specify","input_key":"q","question":"Which?","options":["a","b"],"status":"resolved","created_at":"2026-01-01T00:00:00Z","mission_id":mission_id(MISSION_ID_TAIL),"mission_slug":"001-decision-probe"}]});
    write(
        &feature_dir.join("decisions/index.json"),
        &serde_json::to_string(&index).unwrap(),
    );
    let facts = collect_source_acceptance_facts(fixture.root(), &mission, MatrixInput::NotRequired)
        .unwrap();
    assert!(
        facts.decision_statuses_available,
        "the actual present ledger must be decoded"
    );
    let summary = evaluate_source_acceptance(&facts, SourceAcceptanceOptions::default());
    assert_eq!(
        !summary.needs_clarification.is_empty(),
        receipt["cases"]["resolved"].as_bool().unwrap()
    );
}
