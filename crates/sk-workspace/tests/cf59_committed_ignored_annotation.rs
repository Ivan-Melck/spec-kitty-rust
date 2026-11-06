//! CF59: the committed DONE-strand consumer with a Source-valid annotation in
//! the committed log, in TWO real children (pinned reference first).
//!
//! The reference's store partitions the log and admits BOTH lane rows and
//! `kind: "annotation"` rows before the caller discards the annotations, then the
//! DONE consumer folds lane transitions only. So a 5-key annotation actor, a
//! `str()`-coerced number in the delta and a container scalar are *admitted* and
//! *ignored*: they can never invent or erase a lane. The second fixture carries a
//! malformed annotation (`event_id` is not a ULID); the same partition admits it
//! in the shared pass, so the whole read refuses — the annotation is ignored by
//! the fold, never by admission.

mod support;

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};
use sk_workspace::coherence::coord_incoherent_done_wps;

use support::{Fixture, commit_all, git, init_repo, write};

const CHILD_ENV: &str = "SK_CF59_CONSUMER_ROOT";
const SLUG: &str = "cf59-committed";
const BRANCH: &str = "kitty/mission-cf59-committed";
const REF: &str = "refs/heads/kitty/mission-cf59-committed";

fn valid_log() -> String {
    let rows = [
        json!({
            "actor": "claude", "at": "2", "event_id": "01A0CF5900000000000000AA01",
            "evidence": null, "execution_mode": "worktree", "force": false,
            "from_lane": "planned", "mission_slug": SLUG, "to_lane": "done",
            "wp_id": "WP-A",
        }),
        json!({
            "actor": {"model": "claude-opus", "profile": "implementer-ivan",
                      "profile_source": "flag", "role": "implementer", "tool": "claude"},
            "at": "3", "delta": {"role": "implementer", "note": 10, "tracker_refs": "ab"},
            "event_id": "01A0CF5900000000000000AA02", "kind": "annotation",
            "wp_id": "WP-A",
        }),
        json!({
            "actor": ["claude"], "at": "4", "delta": {"role": {"nested": true}},
            "event_id": "01A0CF5900000000000000AA03", "kind": "annotation",
            "wp_id": "WP-B",
        }),
    ];
    rows.iter()
        .map(|row| format!("{}\n", serde_json::to_string(row).expect("row")))
        .collect()
}

fn malformed_annotation_log() -> String {
    let rows = [
        json!({
            "actor": "claude", "at": "2", "event_id": "01A0CF5900000000000000AA01",
            "evidence": null, "execution_mode": "worktree", "force": false,
            "from_lane": "planned", "mission_slug": SLUG, "to_lane": "done",
            "wp_id": "WP-A",
        }),
        json!({
            "actor": "claude", "at": "3", "delta": {}, "event_id": "nope",
            "kind": "annotation", "wp_id": "WP-B",
        }),
    ];
    rows.iter()
        .map(|row| format!("{}\n", serde_json::to_string(row).expect("row")))
        .collect()
}

fn seed(fixture: &Fixture, log: &str) {
    write(
        &fixture
            .path("kitty-specs")
            .join(SLUG)
            .join("status.events.jsonl"),
        log,
    );
    write(
        &fixture.path("kitty-specs").join(SLUG).join("meta.json"),
        &format!(
            "{}\n",
            json!({"mission_id": "01KJ0000000000000000000059", "slug": SLUG})
        ),
    );
    commit_all(fixture.root(), "seed a committed annotation log");
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

fn candidates() -> Vec<String> {
    ["WP-A", "WP-B", "WP-C"]
        .into_iter()
        .map(str::to_owned)
        .collect()
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

fn source_receipt(sandbox: &Sandbox, valid: &Fixture, malformed: &Fixture) -> Value {
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

candidates = ["WP-A", "WP-B", "WP-C"]
out = {}
for name, root in (("valid", sys.argv[3]), ("malformed", sys.argv[4])):
    root = Path(root)
    try:
        result = coord_incoherent_done_wps(
            sys.argv[2], candidates, repo_root=root, feature_dir=root / "kitty-specs" / "cf59-committed"
        )
    except BaseException as exc:  # noqa: BLE001 - the reference's own phase
        out[name] = {"raised": True, "type": type(exc).__name__, "message": str(exc)}
    else:
        out[name] = {"raised": False, "result": result}
print(json.dumps(out, sort_keys=True))
"##,
    )
    .expect("source receipt script");

    let mut command = Command::new(oracle::python());
    command
        .arg(&script)
        .arg(repo_root())
        .arg(REF)
        .arg(valid.root())
        .arg(malformed.root())
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

#[test]
fn the_committed_consumer_ignores_admitted_annotations() {
    if let Some(root) = std::env::var_os(CHILD_ENV) {
        // Native child: assert the whole environment BEFORE any receipt,
        // metadata or business read.
        let sandbox = Sandbox {
            root: PathBuf::from(root),
        };
        sandbox.assert_isolation();
        let valid_root = PathBuf::from(std::env::var_os("SK_CF59_VALID_FIXTURE").unwrap());
        let malformed_root = PathBuf::from(std::env::var_os("SK_CF59_MALFORMED_FIXTURE").unwrap());
        let expected: Value =
            serde_json::from_slice(&fs::read(sandbox.path("source-receipt.json")).unwrap())
                .unwrap();
        let head_before = fs::read_to_string(sandbox.path("head-before.txt")).unwrap();
        let log_before = fs::read_to_string(sandbox.path("log-before.txt")).unwrap();

        let actual =
            coord_incoherent_done_wps(REF, &candidates(), &valid_root, &feature_dir(&valid_root))
                .unwrap_or_else(|error| panic!("native consumer refused: {error:?}"));
        assert_eq!(
            json!(actual),
            expected["valid"]["result"],
            "an admitted annotation must be ignored by the fold, never invent a lane"
        );
        assert_eq!(
            git(&valid_root, &["rev-parse", "HEAD"]),
            head_before,
            "the derivation never moves HEAD"
        );
        assert_eq!(
            fs::read_to_string(
                valid_root
                    .join("kitty-specs")
                    .join(SLUG)
                    .join("status.events.jsonl")
            )
            .unwrap(),
            log_before,
            "the derivation never rewrites the committed log"
        );

        let reference_message = expected["malformed"]["message"]
            .as_str()
            .expect("reference message");
        let refusal = coord_incoherent_done_wps(
            REF,
            &candidates(),
            &malformed_root,
            &feature_dir(&malformed_root),
        )
        .expect_err("a malformed annotation still refuses the shared admission pass");
        assert!(
            refusal.to_string().contains(reference_message),
            "the native refusal must carry the reference's own message: {refusal}"
        );
        return;
    }

    let sandbox = Sandbox::new();
    let valid = init_repo();
    seed(&valid, &valid_log());
    let malformed = init_repo();
    seed(&malformed, &malformed_annotation_log());

    // The pinned reference computes first; its receipt is the premise.
    let receipt = source_receipt(&sandbox, &valid, &malformed);
    assert_eq!(
        receipt["valid"]["result"],
        json!(["WP-A"]),
        "the reference folds WP-A to done and leaves the annotation-only WP unseeded"
    );
    assert_eq!(receipt["malformed"]["raised"], json!(true));
    assert!(
        receipt["malformed"]["message"]
            .as_str()
            .expect("message")
            .contains("InnerStateChanged.event_id is not a valid ULID: 'nope'"),
        "the reference's malformed-annotation refusal message"
    );
    fs::write(
        sandbox.path("source-receipt.json"),
        serde_json::to_vec(&receipt).unwrap(),
    )
    .unwrap();
    fs::write(
        sandbox.path("head-before.txt"),
        git(valid.root(), &["rev-parse", "HEAD"]),
    )
    .unwrap();
    fs::write(
        sandbox.path("log-before.txt"),
        fs::read_to_string(
            valid
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
            "the_committed_consumer_ignores_admitted_annotations",
            "--nocapture",
        ])
        .current_dir(sandbox.path("cwd"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env(CHILD_ENV, &sandbox.root)
        .env("SK_CF59_VALID_FIXTURE", valid.root())
        .env("SK_CF59_MALFORMED_FIXTURE", malformed.root())
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
