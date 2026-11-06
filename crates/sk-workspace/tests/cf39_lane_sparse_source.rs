//! CF39 Source-first paired differential for the lane sparse policy + recognizer.
//!
//! The PINNED reference (`.venv-rust-oracle`, CPython 3.11.15, verified by the
//! fail-closed `scripts/rust/verify_oracle.py`) executes FIRST, in its own
//! isolated sandbox, and its receipt is the expectation the native leg has to
//! match. The native leg runs in a re-executed child of this same test binary
//! whose whole environment is rebuilt (fresh CWD, HOME, every `XDG_*` root
//! including the `*_DIRS` variants, `TMPDIR`/`TMP`/`TEMP`, the lock-exact
//! `PYTHONPATH`, the packs root), and that child asserts every one of those
//! values before it computes anything. The Source child asserts the same set
//! from inside Python before it computes.
//!
//! Compared, on a matrix that exercises the grammar's edges:
//!
//! * `coordination.lane_sparse_checkout_patterns(slug, mid8)` — exact strings
//!   and order, including an already-suffixed slug, a legacy `NNN-` slug and a
//!   non-ASCII slug;
//! * `lanes.branch_naming.parse_lane_worktree_dir(dir)` — the greedy
//!   right-anchored `^(.+)-(lane-[a-z]+)$` result, including its `None` cases;
//! * `lanes.branch_naming.lane_id_for_worktree_dir(dir, slug)` — recognition by
//!   recomposition, including the same-prefix impostor.
//!
//! NOT compared here, because it needs real linked worktrees, mutating nothing
//! but hard to reproduce byte for byte in a JSON receipt: the drift scan itself.
//! That arm is covered by `cf39_lane_sparse.rs` against real
//! `git worktree add` checkouts. This is a bounded policy/recognizer
//! differential, not a whole-leaf parity claim, not a `--fix` proof and not an
//! approval.

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};
use sk_workspace::coordination_sparse::lane_sparse_checkout_patterns;
use sk_workspace::naming::{lane_id_for_worktree_dir, parse_lane_worktree_dir};

/// The exact test name the native re-exec runs.
const TEST_NAME: &str = "the_policy_and_recognizer_match_the_pinned_source";
/// The child-only environment hook carrying its receipt directory.
const NATIVE_DIR: &str = "SK_CF39_NATIVE_DIR";
/// The child-only environment hook carrying the compared matrix.
const MATRIX_ENV: &str = "SK_CF39_MATRIX";

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

/// The compared matrix: `(slug, mid8)` policies, directory names, and
/// `(directory, mission slug)` recognition pairs.
fn matrix() -> Value {
    json!({
        "policy": [
            ["cf39-mission", "01KNXQS9"],
            ["cf39-mission-01KNXQS9", "01KNXQS9"],
            ["057-legacy", "01COORD0"],
            ["caf\u{e9}-mission", "01KNXQS9"],
        ],
        "parse": [
            "057-foo-lane-a",
            "foo-lane-a-lane-b",
            "foo-lane-planning",
            "foo-lane-",
            "lane-a",
            "foo-lane-A",
            "foo-lane-a\n",
            "foo-lane-a ",
            "fo\no-lane-a",
            "foo-lane-a\n\n",
            "cf39-mission-01KNXQS9-lane-a",
            "caf\u{e9}-lane-a",
            "a-lane-\u{2028}b",
        ],
        "lane": [
            ["057-foo-lane-a", "057-foo"],
            ["057-foobar-lane-a", "057-foo"],
            ["057-foo-lane-a", "057-foobar"],
            ["foo-lane-planning", "foo"],
            ["foo-lane-a\n", "foo"],
            ["cf39-mission-01KNXQS9-lane-a", "cf39-mission-01KNXQS9"],
            ["cf39-mission-01KNXQS9-lane-a", "cf39-mission"],
        ],
    })
}

/// The reference leg: `lanes.branch_naming` + `coordination` only.
const SOURCE_SCRIPT: &str = r#"
import json, os, sys
from pathlib import Path

XDG = ("XDG_CONFIG_HOME", "XDG_CONFIG_DIRS", "XDG_CACHE_HOME", "XDG_DATA_HOME",
       "XDG_DATA_DIRS", "XDG_STATE_HOME", "XDG_RUNTIME_DIR")
for key in XDG:
    value = os.environ.get(key)
    assert value and os.path.isabs(value), (key, value)
for key in ("HOME", "TMPDIR", "TMP", "TEMP"):
    value = os.environ.get(key)
    assert value and os.path.isabs(value), (key, value)
entries = [Path(part) for part in os.environ.get("PYTHONPATH", "").split(os.pathsep) if part]
assert entries and all(part.is_absolute() for part in entries), entries
assert Path(os.environ["SPEC_KITTY_PACKS_ROOT"]).name == "packs"
env = {key: os.environ.get(key) for key in XDG + ("HOME", "TMPDIR", "TMP", "TEMP",
                                                   "PYTHONPATH", "SPEC_KITTY_PACKS_ROOT")}
env["cwd"] = str(Path.cwd())

from specify_cli.coordination import lane_sparse_checkout_patterns
from specify_cli.lanes.branch_naming import lane_id_for_worktree_dir, parse_lane_worktree_dir

matrix = json.loads(sys.argv[1])
print(json.dumps({
    "policy": [[slug, mid8, lane_sparse_checkout_patterns(slug, mid8)]
               for slug, mid8 in matrix["policy"]],
    "parse": [[name, list(parsed) if (parsed := parse_lane_worktree_dir(name)) is not None else None]
              for name in matrix["parse"]],
    "lane": [[name, slug, lane_id_for_worktree_dir(name, slug)]
             for name, slug in matrix["lane"]],
    "env": env,
}, ensure_ascii=True))
"#;

/// Run the reference leg and return its receipt.
fn source_receipt(sandbox: &Sandbox, matrix: &Value) -> Value {
    let mut command = Command::new(oracle::python());
    command
        .args(["-c", SOURCE_SCRIPT])
        .arg(matrix.to_string())
        .current_dir(sandbox.path("cwd"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("PYTHONPATH", repo_root().join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo_root().join("packs"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONUTF8", "1")
        .env("LC_ALL", "C.UTF-8");
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
    assert_eq!(
        env["cwd"],
        Value::from(sandbox.path("cwd").display().to_string())
    );
    for (key, name) in sandbox.env_pairs() {
        assert_eq!(
            env[key],
            Value::from(sandbox.path(name).display().to_string()),
            "reference child {key} leaked"
        );
    }
    assert_eq!(
        env["PYTHONPATH"],
        Value::from(repo_root().join("src").display().to_string())
    );
    assert_eq!(
        env["SPEC_KITTY_PACKS_ROOT"],
        Value::from(repo_root().join("packs").display().to_string())
    );
    receipt
}

/// The native leg, computed from the same matrix.
fn native_rows(matrix: &Value) -> Value {
    let mut policy = Vec::new();
    for row in matrix["policy"].as_array().expect("policy rows") {
        let slug = row[0].as_str().expect("slug");
        let mid8 = row[1].as_str().expect("mid8");
        policy.push(json!([
            slug,
            mid8,
            lane_sparse_checkout_patterns(slug, mid8)
        ]));
    }
    let mut parse = Vec::new();
    for name in matrix["parse"].as_array().expect("parse rows") {
        let name = name.as_str().expect("directory name");
        parse.push(match parse_lane_worktree_dir(name) {
            Some((slug, lane_id)) => json!([name, [slug, lane_id]]),
            None => json!([name, Value::Null]),
        });
    }
    let mut lane = Vec::new();
    for row in matrix["lane"].as_array().expect("lane rows") {
        let name = row[0].as_str().expect("directory name");
        let slug = row[1].as_str().expect("mission slug");
        lane.push(json!([name, slug, lane_id_for_worktree_dir(name, slug)]));
    }
    json!({ "policy": policy, "parse": parse, "lane": lane })
}

/// The child branch: assert the environment, compute, write the receipt.
fn native_child(sandbox_root: &Path, matrix: &Value) {
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

    let receipt = serde_json::to_vec_pretty(&native_rows(matrix)).expect("receipt JSON");
    fs::write(sandbox_root.join("native-receipt.json"), receipt).expect("write receipt");
}

/// Run the native leg in a re-executed child that asserts its own environment.
fn native_receipt(sandbox: &Sandbox, matrix_json: &str) -> Value {
    let mut command = Command::new(std::env::current_exe().expect("test binary"));
    command
        .args(["--exact", TEST_NAME, "--nocapture"])
        .current_dir(sandbox.path("cwd"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env(NATIVE_DIR, &sandbox.root)
        .env(MATRIX_ENV, matrix_json)
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

#[test]
fn the_policy_and_recognizer_match_the_pinned_source() {
    let matrix = matrix();
    let matrix_json = matrix.to_string();

    if let Some(sandbox_root) = std::env::var_os(NATIVE_DIR) {
        let matrix: Value =
            serde_json::from_str(&std::env::var(MATRIX_ENV).expect("matrix for the child"))
                .expect("child matrix JSON");
        native_child(&PathBuf::from(sandbox_root), &matrix);
        return;
    }

    let sandbox = Sandbox::new();

    // Source FIRST: its receipt is the expectation.
    let source = source_receipt(&sandbox, &matrix);
    assert_eq!(
        source["policy"][0],
        json!([
            "cf39-mission",
            "01KNXQS9",
            [
                "/*",
                "!kitty-specs/cf39-mission-01KNXQS9/status.events.jsonl",
                "!kitty-specs/cf39-mission-01KNXQS9/status.json",
            ],
        ]),
        "the reference policy is the isolation contract: {:#?}",
        source["policy"]
    );
    assert_eq!(
        source["parse"][0],
        json!(["057-foo-lane-a", ["057-foo", "lane-a"]])
    );
    assert_eq!(
        source["parse"][4],
        json!(["lane-a", Value::Null]),
        "a name with no slug is not a lane directory"
    );
    assert_eq!(
        source["parse"][0][1][1], "lane-a",
        "the lane id keeps its `lane-` prefix"
    );
    assert_eq!(
        source["lane"][1],
        json!(["057-foobar-lane-a", "057-foo", Value::Null]),
        "the reference rejects the same-prefix impostor by recomposition"
    );

    // Then the native leg, in its own environment-asserting child.
    let native = native_receipt(&sandbox, &matrix_json);
    for key in ["policy", "parse", "lane"] {
        assert_eq!(
            native[key], source[key],
            "native {key} rows must match the reference exactly\nreference: {:#?}",
            source[key]
        );
    }
}
