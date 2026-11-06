//! CF54: committed DONE strand derivation over Source-valued rows, in TWO real
//! children (pinned reference first).
//!
//! The committed log carries (a) numeric `at` values whose Python ordering
//! changes the reduced lane and (b) a 5-key structured actor plus non-boolean
//! `force` on a second WP's `done` row. The reference folds both; the old finite
//! Native view stringified the ordering and refused the actor. The Source child
//! asserts its environment before importing and calls
//! `coord_incoherent_done_wps`; the Native child asserts the same environment,
//! calls the native consumer and compares the answer. The derivation is
//! read-only, asserted after the call.

mod support;

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};
use sk_workspace::coherence::coord_incoherent_done_wps;

use support::{Fixture, commit_all, git, init_repo, write};

const CHILD_ENV: &str = "SK_CF54_CONSUMER_ROOT";
const SLUG: &str = "cf54-committed";
const BRANCH: &str = "kitty/mission-cf54-committed";
const REF: &str = "refs/heads/kitty/mission-cf54-committed";

fn log_text() -> String {
    let rows = [
        json!({
            "actor": "claude", "at": 2, "event_id": "01A0CF5400000000000000AA01",
            "execution_mode": "worktree", "force": false, "from_lane": "planned",
            "mission_slug": SLUG, "to_lane": "done", "wp_id": "WP-A",
        }),
        json!({
            "actor": {"model": "claude-opus", "profile": "implementer-ivan",
                      "profile_source": "flag", "role": "implementer", "tool": "claude"},
            "at": 3, "event_id": "01A0CF5400000000000000AA02",
            "execution_mode": "worktree", "force": "yes", "from_lane": "approved",
            "mission_slug": SLUG, "to_lane": "done", "wp_id": "WP-B",
        }),
        json!({
            "actor": "claude", "at": 10, "event_id": "01A0CF5400000000000000AA03",
            "execution_mode": "worktree", "force": false, "from_lane": "planned",
            "mission_slug": SLUG, "to_lane": "canceled", "wp_id": "WP-A",
        }),
    ];
    rows.iter()
        .map(|row| format!("{}\n", serde_json::to_string(row).expect("row")))
        .collect()
}

fn seed(fixture: &Fixture) {
    write(
        &fixture
            .path("kitty-specs")
            .join(SLUG)
            .join("status.events.jsonl"),
        &log_text(),
    );
    write(
        &fixture.path("kitty-specs").join(SLUG).join("meta.json"),
        &format!(
            "{}\n",
            json!({"mission_id": "01KJ0000000000000000000054", "slug": SLUG})
        ),
    );
    commit_all(fixture.root(), "seed source-valued status log");
    git(fixture.root(), &["branch", BRANCH]);
}

fn feature_dir(root: &Path) -> PathBuf {
    root.join("kitty-specs").join(SLUG)
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

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

    fn assert_isolation(&self) {
        assert_eq!(std::env::current_dir().unwrap(), self.path("cwd"));
        for (key, name) in self.env_pairs() {
            assert_eq!(
                std::env::var_os(key).map(PathBuf::from),
                Some(self.path(name)),
                "child {key} leaked"
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
        assert_eq!(std::env::var("PYTHONHASHSEED").unwrap(), "0");
    }
}

fn source_receipt(sandbox: &Sandbox, fixture: &Fixture) -> Value {
    let script = sandbox.path("source_receipt.py");
    fs::write(
        &script,
        r##"
import json
import os
import sys
from pathlib import Path

sandbox = Path(__file__).resolve().parent
assert Path.cwd() == sandbox / "cwd", Path.cwd()
assert sys.version_info[:3] == (3, 11, 15), sys.version_info
assert os.environ.get("PYTHONHASHSEED") == "0", os.environ.get("PYTHONHASHSEED")
for key, name in (
    ("HOME", "home"), ("XDG_CACHE_HOME", "cache"), ("XDG_CONFIG_HOME", "config"),
    ("XDG_DATA_HOME", "data"), ("XDG_STATE_HOME", "state"), ("XDG_RUNTIME_DIR", "runtime"),
    ("XDG_CONFIG_DIRS", "config-dirs"), ("XDG_DATA_DIRS", "data-dirs"),
    ("TMPDIR", "tmp"), ("TMP", "tmp"), ("TEMP", "tmp"),
):
    assert Path(os.environ[key]) == sandbox / name, (key, os.environ[key])
repo = Path(sys.argv[1])
assert repo.is_absolute()
assert Path(os.environ["PYTHONPATH"]) == repo / "src"
assert Path(os.environ["SPEC_KITTY_PACKS_ROOT"]) == repo / "packs"
from specify_cli.coordination.coherence import coord_incoherent_done_wps
import specify_cli.coordination.coherence as coherence_module
assert Path(coherence_module.__file__).is_relative_to(repo / "src")

result = coord_incoherent_done_wps(
    sys.argv[2],
    ["WP-A", "WP-B", "WP-C"],
    repo_root=Path(sys.argv[3]),
    feature_dir=Path(sys.argv[4]),
)
print(json.dumps({"result": result}, sort_keys=True))
"##,
    )
    .expect("source receipt script");

    let mut command = Command::new(oracle::python());
    command
        .arg(&script)
        .arg(repo_root())
        .arg(REF)
        .arg(fixture.root())
        .arg(feature_dir(fixture.root()))
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
        .env("PYTHONHASHSEED", "0")
        .env("LC_ALL", "C.UTF-8")
        .env("SPEC_KITTY_NO_UPGRADE_CHECK", "1");
    for (key, name) in sandbox.env_pairs() {
        command.env(key, sandbox.path(name));
    }
    let output = command.output().expect("run the pinned reference");
    assert!(
        output.status.success(),
        "reference child failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("reference JSON receipt")
}

fn candidates() -> Vec<String> {
    ["WP-A", "WP-B", "WP-C"]
        .into_iter()
        .map(str::to_owned)
        .collect()
}

#[test]
fn the_committed_consumer_matches_the_pinned_reference_on_source_values() {
    if let Some(root) = std::env::var_os(CHILD_ENV) {
        // Native child: assert the whole environment BEFORE any receipt,
        // metadata or business read.
        let sandbox = Sandbox {
            root: PathBuf::from(root),
        };
        sandbox.assert_isolation();
        let fixture_root = PathBuf::from(std::env::var_os("SK_CF54_CONSUMER_FIXTURE").unwrap());
        let expected: Value =
            serde_json::from_slice(&fs::read(sandbox.path("source-receipt.json")).unwrap())
                .unwrap();
        let head_before = fs::read_to_string(sandbox.path("head-before.txt")).unwrap();
        let log_before = fs::read_to_string(sandbox.path("log-before.txt")).unwrap();

        let actual = coord_incoherent_done_wps(
            REF,
            &candidates(),
            &fixture_root,
            &feature_dir(&fixture_root),
        )
        .unwrap_or_else(|error| panic!("native consumer refused: {error:?}"));
        assert_eq!(
            json!(actual),
            expected["result"],
            "the committed consumer must keep every Source-valid candidate the reference folds"
        );
        assert_eq!(
            git(&fixture_root, &["rev-parse", "HEAD"]),
            head_before,
            "the derivation never moves HEAD"
        );
        assert_eq!(
            fs::read_to_string(
                fixture_root
                    .join("kitty-specs")
                    .join(SLUG)
                    .join("status.events.jsonl")
            )
            .unwrap(),
            log_before,
            "the derivation never rewrites the committed log"
        );
        return;
    }

    let fixture = init_repo();
    seed(&fixture);
    let sandbox = Sandbox::new();
    // The pinned reference computes first; its receipt is the premise.
    let receipt = source_receipt(&sandbox, &fixture);
    assert_eq!(
        receipt["result"],
        json!(["WP-A", "WP-B"]),
        "the reference folds numeric ordering and the 5-key actor to both strands"
    );
    fs::write(
        sandbox.path("source-receipt.json"),
        serde_json::to_vec(&receipt).unwrap(),
    )
    .unwrap();
    fs::write(
        sandbox.path("head-before.txt"),
        git(fixture.root(), &["rev-parse", "HEAD"]),
    )
    .unwrap();
    fs::write(
        sandbox.path("log-before.txt"),
        fs::read_to_string(
            fixture
                .path("kitty-specs")
                .join(SLUG)
                .join("status.events.jsonl"),
        )
        .unwrap(),
    )
    .unwrap();

    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "the_committed_consumer_matches_the_pinned_reference_on_source_values",
            "--nocapture",
        ])
        .current_dir(sandbox.path("cwd"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env(CHILD_ENV, &sandbox.root)
        .env("SK_CF54_CONSUMER_FIXTURE", fixture.root())
        .env("PYTHONPATH", repo_root().join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo_root().join("packs"))
        .env("PYTHONHASHSEED", "0")
        .env("LC_ALL", "C.UTF-8");
    for (key, name) in sandbox.env_pairs() {
        command.env(key, sandbox.path(name));
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "Native child failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
