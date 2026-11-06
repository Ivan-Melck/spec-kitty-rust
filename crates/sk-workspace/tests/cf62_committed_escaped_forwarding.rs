//! CF62: the committed DONE consumer forwards the Source store's **escaped** and
//! **caught** phases, paired against the pinned reference in TWO real children.
//!
//! The reference consumer (`specify_cli.coordination.coherence.coord_incoherent_done_wps`)
//! runs `git show <ref>:kitty-specs/<slug>/status.events.jsonl` and then
//! `read_events_from_text` + the reduce. Its store catches a `JSONDecodeError` line
//! and the admission tuple; an annotation's `int(Infinity)` `OverflowError`, a
//! `NaN` admission (`StoreError`) and a deep-`meta.json` `RecursionError` all reach
//! the caller as different outcomes. Native must keep them different: a caught row
//! refusal, an escaped Python class and a native-only limit can never be read as
//! "no strand" or as each other.
//!
//! The Source child asserts its environment before importing and runs the real
//! consumer; the Native child asserts the same environment before any business
//! read and runs the native consumer. The derivation is read-only, asserted after
//! both calls.

mod support;

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Map, Value, json};
use sk_workspace::coherence::{CoordDoneReadError, coord_incoherent_done_wps};

use support::{Fixture, commit_all, git, init_repo, write};

const CHILD_ENV: &str = "SK_CF62_CONSUMER_ROOT";
const FIXTURE_ENV: &str = "SK_CF62_CONSUMER_FIXTURE";
const DEEP_DEPTH: usize = 2000;

/// A 26-character ULID from the reference's alphabet (no I, L, O or U).
fn ulid(tail: &str) -> String {
    format!("01A0CF62CN{tail:0>16}")
}

fn done_row(slug: &str, tail: &str) -> String {
    format!(
        "{}\n",
        serde_json::to_string(&json!({
            "actor": "claude",
            "at": "2026-11-06T10:00:00+00:00",
            "event_id": ulid(tail),
            "evidence": null,
            "execution_mode": "worktree",
            "force": false,
            "from_lane": "planned",
            "mission_slug": slug,
            "reason": null,
            "review_ref": null,
            "to_lane": "done",
            "wp_id": "WP-A",
        }))
        .expect("row")
    )
}

fn annotation_row(tail: &str, delta: &str) -> String {
    format!(
        "{{\"actor\": \"claude\", \"at\": \"2026-11-06T10:00:01+00:00\", \"delta\": {delta}, \
         \"event_id\": \"{}\", \"kind\": \"annotation\", \"wp_id\": \"WP-A\"}}\n",
        ulid(tail)
    )
}

/// Every consumer case: name -> (slug, committed log, working-tree meta.json).
fn case_table() -> Vec<(&'static str, &'static str, String, Option<String>)> {
    vec![
        (
            "healthy",
            "cf62-healthy",
            done_row("cf62-healthy", "1"),
            Some("{\"mission_id\": \"01A0CF62HEALTHY0000000001\"}".to_owned()),
        ),
        (
            "annotation_infinity",
            "cf62-infinity",
            format!(
                "{}{}",
                done_row("cf62-infinity", "2"),
                annotation_row("3", "{\"shell_pid\": Infinity}")
            ),
            Some("{\"mission_id\": \"01A0CF62INFINITY000000001\"}".to_owned()),
        ),
        (
            "annotation_nan",
            "cf62-nan",
            format!(
                "{}{}",
                done_row("cf62-nan", "4"),
                annotation_row("5", "{\"shell_pid\": NaN}")
            ),
            Some("{\"mission_id\": \"01A0CF62NAN000000000001\"}".to_owned()),
        ),
        (
            "deep_meta",
            "cf62-deep",
            // No `mission_id` key: the legacy slug resolver runs and reads the
            // working tree's `meta.json`, which is deeply nested.
            done_row("cf62-deep", "6"),
            Some(format!(
                "{}{}",
                "[".repeat(DEEP_DEPTH),
                "]".repeat(DEEP_DEPTH)
            )),
        ),
    ]
}

fn branch(slug: &str) -> String {
    format!("kitty/mission-{slug}")
}

fn reference(slug: &str) -> String {
    format!("refs/heads/kitty/mission-{slug}")
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

fn seed(fixture: &Fixture) {
    for (_, slug, log, meta) in case_table() {
        let directory = fixture.path("kitty-specs").join(slug);
        write(&directory.join("status.events.jsonl"), &log);
        if let Some(meta) = meta {
            write(&directory.join("meta.json"), &meta);
        }
        commit_all(fixture.root(), &format!("seed {slug}"));
        git(fixture.root(), &["branch", &branch(slug)]);
    }
}

fn feature_dir(root: &Path, slug: &str) -> PathBuf {
    root.join("kitty-specs").join(slug)
}

fn candidates() -> Vec<String> {
    vec!["WP-A".to_owned(), "WP-B".to_owned()]
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

fixture = Path(sys.argv[3])
with open(sys.argv[2], encoding="utf-8") as handle:
    plan = json.load(handle)
out = {"cases": {}}
for name, case in plan.items():
    record = {}
    try:
        record["result"] = coord_incoherent_done_wps(
            case["ref"],
            ["WP-A", "WP-B"],
            repo_root=fixture,
            feature_dir=fixture / "kitty-specs" / case["slug"],
        )
        record["outcome"] = "ok"
    except BaseException as exc:  # noqa: BLE001 - the escaped phase is the datum
        record["outcome"] = "raised"
        record["type"] = type(exc).__name__
        record["message"] = str(exc)
    out["cases"][name] = record
print(json.dumps(out, sort_keys=True))
"##,
    )
    .expect("source receipt script");

    let mut plan = Map::new();
    for (name, slug, _, _) in case_table() {
        plan.insert(
            name.to_owned(),
            json!({"slug": slug, "ref": reference(slug)}),
        );
    }
    let plan_path = sandbox.path("plan.json");
    fs::write(
        &plan_path,
        serde_json::to_vec(&Value::Object(plan)).expect("plan"),
    )
    .expect("plan file");

    let mut command = Command::new(oracle::python());
    command
        .arg(&script)
        .arg(repo_root())
        .arg(&plan_path)
        .arg(fixture.root())
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

fn native_receipt(fixture: &Path) -> Value {
    let mut out = Map::new();
    for (name, slug, _, _) in case_table() {
        let record = match coord_incoherent_done_wps(
            &reference(slug),
            &candidates(),
            fixture,
            &feature_dir(fixture, slug),
        ) {
            Ok(result) => json!({"outcome": "ok", "result": result}),
            Err(error) => match error {
                CoordDoneReadError::EscapedSourceException {
                    class,
                    phase,
                    message,
                    ..
                } => json!({
                    "outcome": "escaped",
                    "class": class,
                    "phase": phase,
                    "message": message,
                }),
                CoordDoneReadError::SourceOperationalLimit { phase, detail, .. } => json!({
                    "outcome": "operational",
                    "phase": phase,
                    "detail": detail,
                }),
                other => json!({
                    "outcome": "refused",
                    "variant": format!("{other:?}"),
                    "message": other.to_string(),
                }),
            },
        };
        out.insert(name.to_owned(), record);
    }
    Value::Object(out)
}

#[test]
fn the_committed_consumer_forwards_the_escaped_and_caught_source_phases() {
    let Some(root) = std::env::var_os(CHILD_ENV) else {
        let fixture = init_repo();
        seed(&fixture);
        let sandbox = Sandbox::new();
        let receipt = source_receipt(&sandbox, &fixture);

        // Non-vacuity: the reference really answers, really raises the escape and
        // really raises the caught `StoreError` on this fixture.
        assert_eq!(
            receipt["cases"]["healthy"],
            json!({"outcome": "ok", "result": ["WP-A"]}),
            "the reference derives the healthy strand"
        );
        assert_eq!(
            receipt["cases"]["annotation_infinity"]["type"], "OverflowError",
            "the reference lets int(Infinity) escape the consumer"
        );
        assert_eq!(
            receipt["cases"]["annotation_nan"]["type"], "StoreError",
            "the reference catches int(NaN) into a StoreError"
        );
        assert_eq!(
            receipt["cases"]["deep_meta"]["type"], "RecursionError",
            "the reference's deep meta.json escapes the consumer"
        );

        fs::write(
            sandbox.path("source-receipt.json"),
            serde_json::to_vec(&receipt).expect("receipt"),
        )
        .expect("receipt file");
        fs::write(
            sandbox.path("head-before.txt"),
            git(fixture.root(), &["rev-parse", "HEAD"]),
        )
        .expect("head");
        let mut logs = Map::new();
        for (_, slug, _, _) in case_table() {
            logs.insert(
                slug.to_owned(),
                Value::String(
                    fs::read_to_string(
                        feature_dir(fixture.root(), slug).join("status.events.jsonl"),
                    )
                    .expect("log"),
                ),
            );
        }
        fs::write(
            sandbox.path("logs-before.json"),
            serde_json::to_vec(&Value::Object(logs)).expect("logs"),
        )
        .expect("logs file");

        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "the_committed_consumer_forwards_the_escaped_and_caught_source_phases",
                "--nocapture",
            ])
            .current_dir(sandbox.path("cwd"))
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env(CHILD_ENV, &sandbox.root)
            .env(FIXTURE_ENV, fixture.root())
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
        return;
    };

    let sandbox = Sandbox {
        root: PathBuf::from(root),
    };
    sandbox.assert_isolation();
    let fixture = PathBuf::from(std::env::var_os(FIXTURE_ENV).expect("fixture"));
    let source: Value =
        serde_json::from_slice(&fs::read(sandbox.path("source-receipt.json")).unwrap()).unwrap();
    let head_before = fs::read_to_string(sandbox.path("head-before.txt")).unwrap();
    let logs_before: Value =
        serde_json::from_slice(&fs::read(sandbox.path("logs-before.json")).unwrap()).unwrap();

    let native = native_receipt(&fixture);
    let source_cases = source["cases"].as_object().expect("source cases");

    // Healthy: the same answer, not merely both non-empty.
    assert_eq!(
        native["healthy"], source_cases["healthy"],
        "the healthy derivation matches the reference"
    );
    // Escaped `OverflowError`: the class and phase survive; never a strand answer.
    assert_eq!(
        native["annotation_infinity"]["outcome"], "escaped",
        "an escaped OverflowError is not a caught refusal: {}",
        native["annotation_infinity"]
    );
    assert_eq!(native["annotation_infinity"]["class"], "OverflowError");
    assert_eq!(
        native["annotation_infinity"]["phase"], "annotation_admission",
        "the escape keeps its phase"
    );
    assert_eq!(
        native["annotation_infinity"]["message"], source_cases["annotation_infinity"]["message"],
        "the reference's own message"
    );
    // Caught `StoreError`: the reference's own text, in the consumer's caught bucket.
    assert_eq!(
        native["annotation_nan"]["outcome"], "refused",
        "a caught StoreError is a refused row: {}",
        native["annotation_nan"]
    );
    let refused = native["annotation_nan"]["message"]
        .as_str()
        .expect("refusal message");
    assert!(
        refused.contains("Invalid event structure on line 2: cannot convert float NaN to integer"),
        "the reference's caught text: {refused}"
    );
    // Deep meta.json: the reference raises `RecursionError`; Native's J1 reader has
    // an operational frame budget below CPython's acceptance ceiling, so it reports
    // a native limit instead of fabricating that class — and never a healthy answer.
    assert_eq!(
        native["deep_meta"]["outcome"], "operational",
        "a native decode limit is labelled, not absorbed: {}",
        native["deep_meta"]
    );
    assert_eq!(native["deep_meta"]["phase"], "mission_id_decode");

    // Read-only: neither child moved HEAD or rewrote a committed log.
    assert_eq!(
        git(&fixture, &["rev-parse", "HEAD"]),
        head_before,
        "the derivation never moves HEAD"
    );
    for (_, slug, _, _) in case_table() {
        assert_eq!(
            json!(
                fs::read_to_string(feature_dir(&fixture, slug).join("status.events.jsonl"))
                    .expect("log")
            ),
            logs_before[slug],
            "{slug}: the derivation never rewrites the committed log"
        );
    }
}
