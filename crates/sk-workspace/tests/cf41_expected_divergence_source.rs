//! CF41 paired Source/Native proof for the expected-divergence predicate.
//!
//! The reference answer is produced by the pinned interpreter running the real
//! `specify_cli.missions._create.is_expected_coordination_divergence` against the
//! same fixture repositories the native classifier reads. Source runs FIRST; the
//! native answer must match it case for case — the expectation is never written
//! by hand here.
//!
//! # Isolated child, asserted before computation
//!
//! The shared `support/oracle.rs` helper already fails closed on a missing or
//! drifted interpreter. It does not export `XDG_CONFIG_DIRS`/`XDG_DATA_DIRS`, so
//! this test spawns its own child with the complete isolated environment the
//! paired-proof order names: `HOME`, all seven `XDG_*` roots (cache, config,
//! config-dirs, data, data-dirs, state, runtime), `TMPDIR`/`TMP`/`TEMP`, a
//! temporary `CWD`, `PYTHONPATH` at the pinned `src`, and the pinned packs root.
//! The child verifies that environment **before** it imports anything, and exits
//! non-zero if it is wrong, so a mis-isolated child can never produce a verdict.

#[path = "support/oracle.rs"]
mod oracle_support;
mod support;

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use sk_git::{BranchName, GitRepository};
use sk_workspace::coordination_divergence::{
    ExpectedCoordinationDivergenceRequest, classify_expected_coordination_divergence,
};
use support::{commit_file, git, write};

const COORD_BRANCH: &str = "kitty/mission-cf41-mission-01KNXQS9";
const TARGET_BRANCH: &str = "main";
const MISSION_DIR: &str = "cf41-mission-01KNXQS9";

/// One scenario: a fixture name plus the two branch names to compare.
///
/// The two sides share only this table — the fixture construction lives in
/// [`build_fixture`] and the expected verdict comes from Source.
const SCENARIOS: &[(&str, &str, &str)] = &[
    ("coord_only", COORD_BRANCH, TARGET_BRANCH),
    ("coord_primary_file", COORD_BRANCH, TARGET_BRANCH),
    ("coord_unrecognised_file", COORD_BRANCH, TARGET_BRANCH),
    ("coord_outside_specs", COORD_BRANCH, TARGET_BRANCH),
    ("coord_foreign_mission", COORD_BRANCH, TARGET_BRANCH),
    ("target_touches_coord", COORD_BRANCH, TARGET_BRANCH),
    ("identical_tips", TARGET_BRANCH, TARGET_BRANCH),
    ("merge_brings_coord", COORD_BRANCH, TARGET_BRANCH),
    ("merge_brings_primary", COORD_BRANCH, TARGET_BRANCH),
    ("empty_commit", COORD_BRANCH, TARGET_BRANCH),
    ("unrelated_history", COORD_BRANCH, TARGET_BRANCH),
    (
        "missing_branch",
        "kitty/mission-absent-01KNXQS9",
        TARGET_BRANCH,
    ),
    ("exotic_codepoints", COORD_BRANCH, TARGET_BRANCH),
];

fn mission_path(relative: &str) -> String {
    format!("kitty-specs/{MISSION_DIR}/{relative}")
}

fn commit_here(root: &Path, relative: &str, contents: &str, message: &str) {
    write(&root.join(relative), contents);
    commit_file(root, relative, message);
}

/// Initialise a real repository at `root` with one commit on `main`.
fn init_at(root: &Path) {
    std::fs::create_dir_all(root).expect("scenario root");
    git(root, &["init", "-b", "main"]);
    git(root, &["config", "user.name", "CF41 Paired Test"]);
    git(root, &["config", "user.email", "cf41@example.invalid"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    write(&root.join("README.md"), "# fixture\n");
    git(root, &["add", "README.md"]);
    git(root, &["commit", "-m", "initial"]);
}

fn commit_on_coord(root: &Path, relative: &str, contents: &str) {
    git(root, &["checkout", "-q", COORD_BRANCH]);
    commit_here(root, relative, contents, &format!("add {relative}"));
    git(root, &["checkout", "-q", TARGET_BRANCH]);
}

/// Build the real repository for one scenario name at `root`.
fn build_fixture(root: &Path, name: &str) {
    init_at(root);
    // `identical_tips` compares `main` with itself; `unrelated_history` mints the
    // coordination branch as an orphan root instead of branching it off `main`.
    if name != "identical_tips" && name != "unrelated_history" {
        git(root, &["branch", COORD_BRANCH]);
    }
    match name {
        "coord_only" => {
            commit_on_coord(root, &mission_path("status.events.jsonl"), "{}\n");
            commit_here(root, "src/app.rs", "fn main() {}\n", "target work");
        }
        "coord_primary_file" => {
            commit_on_coord(root, &mission_path("spec.md"), "# spec\n");
        }
        "coord_unrecognised_file" => {
            commit_on_coord(root, &mission_path("notes.txt"), "notes\n");
        }
        "coord_outside_specs" => {
            commit_on_coord(root, "src/leak.rs", "// leak\n");
        }
        "coord_foreign_mission" => {
            commit_on_coord(
                root,
                "kitty-specs/other-mission-01KNXQSA/status.events.jsonl",
                "{}\n",
            );
        }
        "target_touches_coord" => {
            commit_on_coord(root, &mission_path("status.events.jsonl"), "{}\n");
            commit_here(
                root,
                &mission_path("status.events.jsonl"),
                "{\"second\":true}\n",
                "target writes status",
            );
        }
        "identical_tips" => {}
        "merge_brings_coord" => {
            git(root, &["checkout", "-q", COORD_BRANCH]);
            commit_here(root, &mission_path("issue-matrix.json"), "{}\n", "verdicts");
            git(root, &["checkout", "-q", "-b", "cf41-side"]);
            commit_here(
                root,
                &mission_path("traces/approach.md"),
                "- approach\n",
                "side coord work",
            );
            git(root, &["checkout", "-q", COORD_BRANCH]);
            git(root, &["merge", "-q", "--no-ff", "--no-edit", "cf41-side"]);
            git(root, &["checkout", "-q", TARGET_BRANCH]);
        }
        "merge_brings_primary" => {
            git(root, &["checkout", "-q", COORD_BRANCH]);
            commit_here(
                root,
                &mission_path("status.events.jsonl"),
                "{}\n",
                "coord status",
            );
            git(root, &["checkout", "-q", "-b", "cf41-bad-side"]);
            commit_here(root, &mission_path("plan.md"), "# plan\n", "side plan");
            git(root, &["checkout", "-q", COORD_BRANCH]);
            git(
                root,
                &["merge", "-q", "--no-ff", "--no-edit", "cf41-bad-side"],
            );
            git(root, &["checkout", "-q", TARGET_BRANCH]);
        }
        "empty_commit" => {
            git(root, &["checkout", "-q", COORD_BRANCH]);
            git(
                root,
                &["commit", "-q", "--allow-empty", "-m", "record nothing"],
            );
            git(root, &["checkout", "-q", TARGET_BRANCH]);
        }
        "unrelated_history" => {
            git(root, &["checkout", "-q", "--orphan", COORD_BRANCH]);
            // Remove the inherited files from BOTH index and worktree: an
            // orphan branch must not carry `main`'s tree, and a leftover
            // untracked README would block the way back to `main`.
            git(root, &["rm", "-rqf", "."]);
            commit_here(
                root,
                &mission_path("status.events.jsonl"),
                "{}\n",
                "orphan root",
            );
            git(root, &["checkout", "-q", TARGET_BRANCH]);
        }
        "missing_branch" => {}
        "exotic_codepoints" => {
            commit_on_coord(
                root,
                &mission_path("traces/notes-\u{E000}\u{FFFF}.md"),
                "- exotic\n",
            );
            commit_on_coord(
                root,
                &mission_path("tasks/WP01/review-cycle-7.md"),
                "- cycle\n",
            );
        }
        other => panic!("unknown scenario {other}"),
    }
}

fn native_verdict(root: &Path, coordination: &str, target: &str) -> bool {
    let repository = GitRepository::open(root).expect("open fixture repository");
    let coordination = BranchName::parse(coordination).expect("valid branch");
    let target = BranchName::parse(target).expect("valid branch");
    classify_expected_coordination_divergence(
        &repository,
        &ExpectedCoordinationDivergenceRequest::new(&coordination, &target, MISSION_DIR),
    )
    .expected()
}

/// The child script: verify isolation, then compute the reference verdicts.
const SOURCE_SCRIPT: &str = r#"
import json, os, sys
from pathlib import Path

spec = json.loads(sys.argv[1])
root = Path(spec["root"])
sandbox = Path(spec["sandbox"])
expected = spec["expected_env"]
sandbox_keys = spec["sandbox_keys"]

problems = []
for key, value in expected.items():
    if os.environ.get(key) != value:
        problems.append(f"{key}: {os.environ.get(key)!r} != {value!r}")
for key in sandbox_keys:
    value = os.environ.get(key, "")
    if not os.path.isabs(value):
        problems.append(f"{key}: {value!r} is not absolute")
    elif Path(value) != sandbox / spec["sandbox_leaf"][key]:
        problems.append(f"{key}: {value!r} is not {sandbox / spec['sandbox_leaf'][key]}")
cwd = Path(os.getcwd())
if cwd != sandbox / "cwd":
    problems.append(f"CWD: {cwd} != {sandbox / 'cwd'}")
if not os.environ.get("PYTHONPATH", "").endswith("/src"):
    problems.append(f"PYTHONPATH: {os.environ.get('PYTHONPATH')!r} does not end at /src")
if Path(os.environ.get("SPEC_KITTY_PACKS_ROOT", "/nonexistent")).name != "packs":
    problems.append("SPEC_KITTY_PACKS_ROOT is not the pinned packs root")
if problems:
    print(json.dumps({"env_error": problems}))
    sys.exit(2)

# Isolation verified in-child BEFORE any reference import or computation.
print(json.dumps({"env_ok": True, "cwd": str(cwd)}), flush=True)

from specify_cli.missions._create import is_expected_coordination_divergence

verdicts = {}
for name, coordination, target in spec["scenarios"]:
    verdicts[name] = is_expected_coordination_divergence(
        root / name,
        coordination_branch=coordination,
        target_branch=target,
        mission_dir_name=spec["mission_dir"],
    )
print(json.dumps({"verdicts": verdicts}))
"#;

#[test]
fn source_and_native_agree_on_expected_coordination_divergence() {
    let workspace = tempfile::tempdir().expect("workspace");
    let fixtures = workspace.path().join("fixtures");
    std::fs::create_dir_all(&fixtures).expect("fixtures root");

    // Build every real fixture first; each scenario owns its own repository.
    for (name, _, _) in SCENARIOS {
        build_fixture(&fixtures.join(name), name);
    }

    // Isolated child sandbox: one directory per XDG root, plus CWD and TMP.
    let sandbox = workspace.path().join("sandbox");
    let sandbox_leaf: BTreeMap<&str, &str> = BTreeMap::from([
        ("HOME", "home"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_CONFIG_DIRS", "config-dirs"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_DATA_DIRS", "data-dirs"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_RUNTIME_DIR", "runtime"),
        ("TMPDIR", "tmp"),
        ("TMP", "tmp"),
        ("TEMP", "tmp"),
    ]);
    for leaf in sandbox_leaf.values() {
        std::fs::create_dir_all(sandbox.join(leaf)).expect("sandbox root");
    }
    std::fs::create_dir_all(sandbox.join("cwd")).expect("sandbox cwd");

    let repository_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let path_value = std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string());
    let mut expected_env: BTreeMap<String, String> = sandbox_leaf
        .iter()
        .map(|(key, leaf)| ((*key).to_string(), sandbox.join(leaf).display().to_string()))
        .collect();
    expected_env.insert("PATH".to_string(), path_value);
    expected_env.insert(
        "PYTHONPATH".to_string(),
        repository_root.join("src").display().to_string(),
    );
    expected_env.insert(
        "SPEC_KITTY_PACKS_ROOT".to_string(),
        repository_root.join("packs").display().to_string(),
    );
    expected_env.insert("PYTHONDONTWRITEBYTECODE".to_string(), "1".to_string());

    let spec = serde_json::json!({
        "root": fixtures.display().to_string(),
        "sandbox": sandbox.display().to_string(),
        "sandbox_leaf": sandbox_leaf,
        "sandbox_keys": sandbox_leaf.keys().collect::<Vec<_>>(),
        "mission_dir": MISSION_DIR,
        "expected_env": expected_env,
        "scenarios": SCENARIOS
            .iter()
            .map(|(name, coordination, target)| serde_json::json!([name, coordination, target]))
            .collect::<Vec<_>>(),
    });

    let mut command = Command::new(oracle_support::python());
    command
        .arg("-c")
        .arg(SOURCE_SCRIPT)
        .arg(spec.to_string())
        .current_dir(sandbox.join("cwd"));
    command.env_clear();
    for (key, value) in &expected_env {
        command.env(key, value);
    }
    // The same Git environment policy the native process seam applies, so both
    // children read the fixtures under one configuration.
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_PAGER", "cat");
    let output = command.output().expect("run the pinned Python oracle");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        output.status.success(),
        "Source child failed ({}):\nstdout: {stdout}\nstderr: {stderr}",
        output.status
    );

    // First line: the child's own pre-computation isolation proof.
    let mut lines = stdout.lines().filter(|line| !line.trim().is_empty());
    let isolation: serde_json::Value =
        serde_json::from_str(lines.next().expect("isolation line")).expect("isolation json");
    assert_eq!(
        isolation.get("env_ok").and_then(|value| value.as_bool()),
        Some(true),
        "child reported an isolation problem: {isolation}"
    );
    assert_eq!(
        isolation.get("cwd").and_then(|value| value.as_str()),
        Some(sandbox.join("cwd").display().to_string().as_str()),
        "child cwd was not the isolated sandbox cwd"
    );

    let source: serde_json::Value =
        serde_json::from_str(lines.next().expect("source verdict line")).expect("verdict json");
    let source_verdicts = source
        .get("verdicts")
        .and_then(|value| value.as_object())
        .expect("source verdict map");

    for (name, coordination, target) in SCENARIOS {
        let repo = fixtures.join(name);
        let source_value = source_verdicts
            .get(*name)
            .and_then(|value| value.as_bool())
            .unwrap_or_else(|| panic!("Source produced no verdict for {name}"));
        let native = native_verdict(&repo, coordination, target);
        assert_eq!(
            native, source_value,
            "scenario {name} disagreed: native={native} source={source_value}"
        );
    }

    // The fixture set must be discriminating, not uniformly one answer.
    let truths = source_verdicts
        .values()
        .filter(|value| value.as_bool() == Some(true))
        .count();
    assert!(
        truths > 0 && truths < source_verdicts.len(),
        "fixture set is not discriminating: {source_verdicts:?}"
    );
    // Receipt: the reference verdicts these fixtures produced.
    println!(
        "source verdicts ({} true of {}): {}",
        truths,
        source_verdicts.len(),
        serde_json::to_string(source_verdicts).expect("serialize verdicts")
    );
}
