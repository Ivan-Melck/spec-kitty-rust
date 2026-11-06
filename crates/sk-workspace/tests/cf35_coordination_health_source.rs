//! CF35 Source-first paired differential for the read-only coordination health.
//!
//! The PINNED reference (`.venv-rust-oracle`, CPython 3.11.15, verified by the
//! fail-closed `scripts/rust/verify_oracle.py`) executes FIRST, in its own
//! isolated sandbox, and its receipt is what the native leg has to match. The
//! native leg runs in a re-executed child of this same test binary whose whole
//! environment is rebuilt (fresh CWD, HOME, every `XDG_*` root used here,
//! `TMPDIR`/`TMP`/`TEMP`, the lock-exact `PYTHONPATH`, the packs root), and that
//! child asserts every one of those values before it computes anything.
//!
//! Compared: the exact JSON shape of every finding the reference's read-only
//! collector emits for one real fixture — severity, message, `next_step`,
//! `error_code` and the full `extra` payload — over the WHOLE read-only surface
//! (`_collect_coordination_findings`, `check_staleness=True`), which covers:
//!
//! * `_check_git_version`
//! * `_check_tracked_worktrees_content`
//! * `_check_stranded_coord_revert`
//! * `_check_coordination_worktree_health`
//! * `_check_lane_sparse_checkout_drift`
//! * `_check_coord_branch_staleness` (including the expected-divergence exemption)
//!
//! The reference is called through its own collector rather than per function, so
//! the comparison also covers the reference's check ORDER and its caller-side
//! `meta_path` attachment.
//!
//! This is not a whole-leaf parity claim, not a `--fix` proof, and not an approval.

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;
mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use sk_workspace::coordination_health::{CoordinationHealthRequest, coordination_health};
use sk_workspace::workspace::ProjectWorkspace;

use support::{Fixture, git, init_repo, write};

/// The exact test name the native re-exec runs.
const TEST_NAME: &str = "the_composed_read_only_findings_match_the_pinned_source";
/// The child-only environment hook carrying its receipt directory.
const NATIVE_DIR: &str = "SK_CF35_NATIVE_DIR";
/// The child-only environment hook carrying the shared fixture root.
const FIXTURE_ENV: &str = "SK_CF35_FIXTURE_ROOT";

const MISSION_ID: &str = "01KNXQS9ATWWFXS3K5ZJ9E5001";
const MID8: &str = "01KNXQS9";
const SLUG: &str = "cf35-paired";
const COORD_BRANCH: &str = "kitty/mission-cf35-paired-01KNXQS9";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

/// An isolated sandbox whose every OS root is a fresh directory under it.
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("sandbox");
        for directory in [
            "home",
            "cache",
            "config",
            "data",
            "state",
            "runtime",
            "config-dirs",
            "data-dirs",
            "tmp",
            "cwd",
        ] {
            fs::create_dir_all(temp.path().join(directory)).expect("sandbox root");
        }
        Self { root: temp.keep() }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    /// The environment every child asserts verbatim.
    fn env_pairs(&self) -> [(&'static str, &'static str); 11] {
        [
            ("HOME", "home"),
            ("XDG_CACHE_HOME", "cache"),
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_STATE_HOME", "state"),
            ("XDG_RUNTIME_DIR", "runtime"),
            ("XDG_CONFIG_DIRS", "config-dirs"),
            ("XDG_DATA_DIRS", "data-dirs"),
            ("TMPDIR", "tmp"),
            ("TMP", "tmp"),
            ("TEMP", "tmp"),
        ]
    }
}

/// One real repository exercising four composed findings at once.
///
/// Repo-level: a tracked path under `.worktrees/` (the #1772 error). Mission:
/// a coordination worktree that is dirty, plus a coordination branch that has
/// fallen behind its target branch.
fn build_fixture() -> (Fixture, PathBuf) {
    let fixture = init_repo();
    let root = fixture.root();
    write(&root.join(".gitignore"), ".worktrees/\n");
    let worktree = fixture
        .path(".worktrees")
        .join(format!("{SLUG}-{MID8}-coord"));
    write(
        &root.join("kitty-specs").join(SLUG).join("meta.json"),
        &format!(
            "{}\n",
            serde_json::json!({
                "mission_id": MISSION_ID,
                "mission_slug": SLUG,
                "topology": "coord",
                "target_branch": "main",
                "coordination_branch": COORD_BRANCH,
                "mission_type": "software-dev",
            })
        ),
    );
    git(root, &["add", "-A"]);
    git(root, &["commit", "--quiet", "-m", "mission"]);

    git(root, &["branch", COORD_BRANCH]);
    fs::create_dir_all(worktree.parent().expect("worktrees dir")).expect("worktrees dir");
    git(
        root,
        &[
            "worktree",
            "add",
            "--quiet",
            worktree.to_str().expect("utf-8 fixture path"),
            COORD_BRANCH,
        ],
    );

    // Tracked scratch content under .worktrees/ (the reference's error case).
    write(
        &fixture.path(".worktrees").join("stray").join("junk.txt"),
        "scratch\n",
    );
    git(root, &["add", "-f", ".worktrees/stray"]);
    git(root, &["commit", "--quiet", "-m", "track scratch"]);

    // A dirty coordination worktree.
    write(&worktree.join("scratch.txt"), "in flight\n");

    // The target advances, so the coordination branch is now behind it.
    write(&root.join("target-moves.txt"), "target\n");
    git(root, &["add", "-A"]);
    git(root, &["commit", "--quiet", "-m", "target advances"]);

    (fixture, worktree)
}

/// The reference's own findings for the composed surface, from a real child.
fn source_receipt(sandbox: &Sandbox, fixture_root: &Path) -> Value {
    let script = sandbox.path("source_receipt.py");
    fs::write(
        &script,
        r#"
import json
import os
import sys
from pathlib import Path

# Assertions run INSIDE the Source child before imports or business reads.
sandbox = Path(__file__).resolve().parent
assert Path.cwd() == sandbox / "cwd"
assert sys.version_info[:3] == (3, 11, 15), sys.version_info
for key, name in (
    ("HOME", "home"), ("XDG_CACHE_HOME", "cache"), ("XDG_CONFIG_HOME", "config"),
    ("XDG_DATA_HOME", "data"), ("XDG_STATE_HOME", "state"), ("XDG_RUNTIME_DIR", "runtime"),
    ("XDG_CONFIG_DIRS", "config-dirs"), ("XDG_DATA_DIRS", "data-dirs"),
    ("TMPDIR", "tmp"), ("TMP", "tmp"), ("TEMP", "tmp"),
):
    assert Path(os.environ[key]) == sandbox / name, (key, os.environ[key])
repo = Path(sys.argv[3])
assert repo.is_absolute()
assert Path(os.environ["PYTHONPATH"]) == repo / "src"
assert Path(os.environ["SPEC_KITTY_PACKS_ROOT"]) == repo / "packs"
from specify_cli.cli.commands import _coordination_doctor as doctor
assert Path(doctor.__file__).is_relative_to(repo / "src")

root = Path(sys.argv[1]).resolve()
slug = sys.argv[2]

# The reference's own read-only collector: every check, in its own order, with
# its own caller-side `meta_path` attachment.
findings = [
    finding.__dict__
    for finding in doctor._collect_coordination_findings(root, check_staleness=True)
]

env = {key: os.environ.get(key) for key in (
    "HOME", "XDG_CACHE_HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME",
    "XDG_STATE_HOME", "XDG_RUNTIME_DIR", "XDG_CONFIG_DIRS", "XDG_DATA_DIRS", "TMPDIR", "TMP", "TEMP",
    "PYTHONPATH", "SPEC_KITTY_PACKS_ROOT",
)}
env["cwd"] = str(Path.cwd())
print(json.dumps({"findings": findings, "env": env}))
"#,
    )
    .expect("source receipt script");

    let mut command = Command::new(oracle::python());
    command
        .arg(&script)
        .arg(fixture_root)
        .arg(SLUG)
        .arg(repo_root())
        .current_dir(sandbox.path("cwd"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("PYTHONPATH", repo_root().join("src"))
        .env(
            "SPEC_KITTY_PACKS_ROOT",
            repo_root().join("packs").display().to_string(),
        )
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONUTF8", "1")
        .env("LC_ALL", "C.UTF-8")
        .env("SPEC_KITTY_NO_UPGRADE_CHECK", "1");
    for (key, name) in sandbox.env_pairs() {
        command.env(key, sandbox.path(name));
    }
    let output = command.output().expect("run the pinned reference");
    assert!(
        output.status.success(),
        "reference receipt failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt: Value = serde_json::from_slice(&output.stdout).expect("reference JSON receipt");

    // The reference child ran where it was told to, with its own roots.
    let env = &receipt["env"];
    assert_eq!(env["cwd"], sandbox.path("cwd").display().to_string());
    for (key, name) in sandbox.env_pairs() {
        assert_eq!(
            env[key],
            sandbox.path(name).display().to_string(),
            "reference child {key} leaked"
        );
    }
    assert_eq!(
        env["PYTHONPATH"],
        repo_root().join("src").display().to_string()
    );
    assert_eq!(
        env["SPEC_KITTY_PACKS_ROOT"],
        repo_root().join("packs").display().to_string()
    );
    receipt
}

/// Run the native leg in a re-executed child that asserts its own environment.
fn native_receipt(sandbox: &Sandbox, fixture_root: &Path) -> Value {
    let mut command = Command::new(std::env::current_exe().expect("test binary"));
    command
        .args(["--exact", TEST_NAME, "--nocapture"])
        .current_dir(sandbox.path("cwd"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env(NATIVE_DIR, &sandbox.root)
        .env(FIXTURE_ENV, fixture_root)
        .env("PYTHONPATH", repo_root().join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo_root().join("packs"))
        .env("LC_ALL", "C.UTF-8");
    for (key, name) in sandbox.env_pairs() {
        command.env(key, sandbox.path(name));
    }
    let output = command.output().expect("run the native child");
    assert!(
        output.status.success(),
        "native child failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt = fs::read(sandbox.path("native-receipt.json")).expect("native receipt");
    serde_json::from_slice(&receipt).expect("native JSON receipt")
}

/// The child branch: assert the environment, compute, write the receipt.
fn native_child(sandbox_root: &Path, fixture_root: &Path) {
    // Every root must be the sandbox's, verbatim.
    assert_eq!(
        std::env::current_dir().expect("cwd"),
        sandbox_root.join("cwd")
    );
    for (key, name) in [
        ("HOME", "home"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_RUNTIME_DIR", "runtime"),
        ("XDG_CONFIG_DIRS", "config-dirs"),
        ("XDG_DATA_DIRS", "data-dirs"),
        ("TMPDIR", "tmp"),
        ("TMP", "tmp"),
        ("TEMP", "tmp"),
    ] {
        assert_eq!(
            std::env::var_os(key).map(PathBuf::from),
            Some(sandbox_root.join(name)),
            "native child {key} leaked"
        );
    }
    assert_eq!(
        std::env::var_os("PYTHONPATH").map(PathBuf::from),
        Some(repo_root().join("src"))
    );
    assert_eq!(
        std::env::var_os("SPEC_KITTY_PACKS_ROOT").map(PathBuf::from),
        Some(repo_root().join("packs"))
    );

    let workspace = ProjectWorkspace::open(fixture_root).expect("fixture repository");
    let report = coordination_health(
        &workspace,
        &CoordinationHealthRequest::new().with_staleness(),
    )
    .expect("native diagnosis answers");
    let payload = serde_json::json!({
        "findings": report.to_json(),
        "unavailable": report
            .unavailable()
            .iter()
            .map(|check| check.check())
            .collect::<Vec<_>>(),
    });
    let receipt = serde_json::to_vec_pretty(&payload).expect("receipt JSON");
    fs::write(sandbox_root.join("native-receipt.json"), receipt).expect("write receipt");
}

#[test]
fn the_composed_read_only_findings_match_the_pinned_source() {
    if let Some(sandbox_root) = std::env::var_os(NATIVE_DIR) {
        let fixture_root =
            PathBuf::from(std::env::var_os(FIXTURE_ENV).expect("fixture root for the child"));
        native_child(&PathBuf::from(sandbox_root), &fixture_root);
        return;
    }

    let (fixture, worktree) = build_fixture();
    let sandbox = Sandbox::new();

    // Source FIRST: its receipt is the expectation.
    let source = source_receipt(&sandbox, fixture.root());
    let source_findings = source["findings"].as_array().expect("source findings");
    assert!(
        source_findings.len() >= 4,
        "the reference should report the fixture's whole surface: {source_findings:#?}"
    );
    let source_codes: Vec<Value> = source_findings
        .iter()
        .map(|finding| finding["error_code"].clone())
        .collect();
    for expected in [
        serde_json::json!("TRACKED_WORKTREES_CONTENT"),
        serde_json::json!("COORDINATION_WORKTREE_DIRTY"),
        serde_json::json!("COORDINATION_BRANCH_STALE_VS_TARGET"),
    ] {
        assert!(
            source_codes.contains(&expected),
            "the reference lost {expected}: {source_findings:#?}"
        );
    }

    // Then the native leg, in its own environment-asserting child.
    let native = native_receipt(&sandbox, fixture.root());
    assert_eq!(
        native["findings"],
        source["findings"],
        "native findings must match the reference's composed surface exactly\nfixture worktree: {}",
        worktree.display()
    );
    assert_eq!(
        native["unavailable"],
        serde_json::json!([]),
        "every requested check is composed, so the report is complete"
    );
}
