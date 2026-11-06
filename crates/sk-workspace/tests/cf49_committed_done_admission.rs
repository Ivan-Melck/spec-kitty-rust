//! CF49: canonical Source event-store admission on the committed DONE consumer.
//!
//! # The defect this lane closes
//!
//! The stranded-coordination DONE derivation reads the committed
//! `kitty-specs/<mission>/status.events.jsonl` and reduces it. Until this lane the
//! read was "JSON objects, then the diary parser": JSON syntactic success was
//! treated as model admission. The reference does not work that way — it admits a
//! row through `specify_cli.status.store.read_events_from_text`, which validates
//! every row against the CLI event model — and, for a handful of rows the model
//! admits but its *fold* cannot key on (an unhashable `wp_id`, an unorderable
//! `at`), the reference's own `reduce()` raises. Either way the reference never
//! answers "no strand"; the native consumer did.
//!
//! # What is asserted here
//!
//! * The PINNED reference runs FIRST, in its own isolated child (fresh CWD/HOME,
//!   all seven `XDG_*` roots, `TMPDIR`/`TMP`/`TEMP`, `PYTHONHASHSEED`, the
//!   lock-exact `PYTHONPATH` and the packs root), and asserts that environment
//!   before computing. Its receipt is the premise, not a later rationalisation.
//! * The committed consumer must never return the healthy "no strand" answer for
//!   a log the reference cannot admit or cannot fold.
//! * The valid fold — lanes, candidate order, duplicates, blank lines, non-lane
//!   rows — is unchanged, and the derivation stays strictly read-only.
//!
//! Not claimed: allocation/ref breadth, the unbounded read, the remaining
//! scalar-type fold divergences, or any health/count promotion. Those are named in
//! the lane report.

mod support;

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};
use sk_workspace::coherence::coord_incoherent_done_wps;

use support::{Fixture, commit_all, git, init_repo, write};

const SLUG: &str = "cf49-mission";

// ---------------------------------------------------------------------------
// Fixture rows
// ---------------------------------------------------------------------------

fn event(wp_id: Value, to_lane: &str, at: &str, event_id: &str) -> Value {
    json!({
        "actor": "claude",
        "at": at,
        "event_id": event_id,
        "evidence": null,
        "execution_mode": "worktree",
        "force": false,
        "from_lane": "approved",
        "mission_slug": SLUG,
        "reason": null,
        "review_ref": null,
        "to_lane": to_lane,
        "wp_id": wp_id,
    })
}

fn text_row(row: &Value) -> String {
    format!("{}\n", serde_json::to_string(row).expect("serialize row"))
}

/// A row the reference's model admits and whose fold then cannot key on it:
/// `wp_id` is a JSON array, so `departures.setdefault(event.wp_id, …)` raises
/// `TypeError: unhashable type: 'list'`. No lane exists for `WP-A`, and the native
/// consumer used to answer "no strand" for exactly this log.
fn unhashable_wp_id_row() -> Value {
    event(
        json!(["WP-A"]),
        "done",
        "2026-11-05T10:05:00+00:00",
        "01A0CF4900000000000000AA01",
    )
}

/// A row the reference refuses during admission (`StoreError`).
fn unknown_lane_row() -> Value {
    event(
        json!("WP-A"),
        "shipped",
        "2026-11-05T10:06:00+00:00",
        "01A0CF4900000000000000AA02",
    )
}

fn valid_rows() -> Vec<Value> {
    vec![
        event(
            json!("WP-A"),
            "done",
            "2026-11-05T10:05:00+00:00",
            "01A0CF4900000000000000AA03",
        ),
        event(
            json!("WP-B"),
            "approved",
            "2026-11-05T10:06:00+00:00",
            "01A0CF4900000000000000AA04",
        ),
    ]
}

fn log_path(root: &Path, mission_dir_name: &str) -> PathBuf {
    root.join("kitty-specs")
        .join(mission_dir_name)
        .join("status.events.jsonl")
}

fn feature_dir(root: &Path, mission_dir_name: &str) -> PathBuf {
    root.join("kitty-specs").join(mission_dir_name)
}

/// Write the log at `root`, commit it and mint `branch` at that commit.
fn seed_branch(root: &Path, mission_dir_name: &str, content: &str, branch: &str) {
    write(&log_path(root, mission_dir_name), content);
    commit_all(root, "seed status log");
    git(root, &["branch", branch]);
}

fn wps(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| (*id).to_owned()).collect()
}

fn derive(fixture: &Fixture, branch: &str, candidates: &[&str]) -> Result<Vec<String>, String> {
    coord_incoherent_done_wps(
        branch,
        &wps(candidates),
        fixture.root(),
        &feature_dir(fixture.root(), SLUG),
    )
    .map_err(|error| format!("{error:?}"))
}

// ---------------------------------------------------------------------------
// Isolated pinned-reference child
// ---------------------------------------------------------------------------

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

    fn feature_dir(&self) -> PathBuf {
        self.path("kitty-specs").join(SLUG)
    }
}

/// The rows the reference child is asked about, by case name.
fn source_cases() -> Vec<(&'static str, String)> {
    vec![
        ("valid", {
            let mut text = String::new();
            for row in valid_rows() {
                text.push_str(&text_row(&row));
            }
            text
        }),
        ("unhashable_wp_id", text_row(&unhashable_wp_id_row())),
        ("unknown_lane", text_row(&unknown_lane_row())),
        ("non_object_row", "[1, 2, 3]\n".to_owned()),
    ]
}

/// Run the PINNED reference over the same rows: one isolated child, env asserted.
fn source_receipt(sandbox: &Sandbox) -> Value {
    let cases_path = sandbox.path("cases.json");
    let cases: serde_json::Map<String, Value> = source_cases()
        .into_iter()
        .map(|(name, content)| (name.to_owned(), Value::String(content)))
        .collect();
    fs::write(
        &cases_path,
        serde_json::to_string(&Value::Object(cases)).expect("cases json"),
    )
    .expect("cases file");

    // The mission directory carries the legacy mission-id resolver's `meta.json`.
    write(
        &sandbox.feature_dir().join("meta.json"),
        &format!(
            "{}\n",
            json!({"mission_id": "01KJ0000000000000000000049", "slug": SLUG})
        ),
    );

    let script = sandbox.path("source_receipt.py");
    fs::write(
        &script,
        r##"
import json
import os
import sys
from pathlib import Path

# Assertions run INSIDE the Source child before imports or business reads.
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
from specify_cli.status import reduce, read_events_from_text
import specify_cli.status as status_module
assert Path(status_module.__file__).is_relative_to(repo / "src")

cases = json.loads(Path(sys.argv[2]).read_text(encoding="utf-8"))
feature_dir = Path(sys.argv[3])
out = {}
for name, content in cases.items():
    record = {"admission": None, "fold": None}
    try:
        events = read_events_from_text(feature_dir, content)
    except Exception as exc:  # noqa: BLE001 - the exact phase is the receipt
        record["admission"] = {"outcome": "raised", "type": type(exc).__name__, "message": str(exc)}
    else:
        record["admission"] = {
            "outcome": "ok",
            "count": len(events),
            "lanes": [str(event.to_lane) for event in events],
            "mission_ids": [event.mission_id for event in events],
        }
        try:
            snapshot = reduce(events)
        except Exception as exc:  # noqa: BLE001
            record["fold"] = {"outcome": "raised", "type": type(exc).__name__, "message": str(exc)}
        else:
            record["fold"] = {"outcome": "ok", "lane": (snapshot.work_packages.get("WP-A") or {}).get("lane")}
    out[name] = record

env = {key: os.environ.get(key) for key in (
    "HOME", "XDG_CACHE_HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME",
    "XDG_RUNTIME_DIR", "XDG_CONFIG_DIRS", "XDG_DATA_DIRS", "TMPDIR", "TMP", "TEMP",
    "PYTHONPATH", "SPEC_KITTY_PACKS_ROOT", "PYTHONHASHSEED",
)}
env["cwd"] = str(Path.cwd())
print(json.dumps({"cases": out, "env": env}, sort_keys=True))
"##,
    )
    .expect("source receipt script");

    let mut command = Command::new(oracle::python());
    command
        .arg(&script)
        .arg(repo_root())
        .arg(&cases_path)
        .arg(sandbox.feature_dir())
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
        "reference receipt failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt: Value = serde_json::from_slice(&output.stdout).expect("reference JSON receipt");
    let env = &receipt["env"];
    assert_eq!(env["cwd"], sandbox.path("cwd").display().to_string());
    assert_eq!(env["PYTHONHASHSEED"], "0");
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// The reference's own answer for the three load-bearing rows, then the consumer.
///
/// Source FIRST: the child receipt is asserted before any native claim, so the
/// premise "the reference never answers `no strand` for these logs" is real data
/// from the pinned baseline, not an inference from this lane's reading.
#[test]
fn the_committed_consumer_never_calls_an_unadmitted_or_unfoldable_log_healthy() {
    let sandbox = Sandbox::new();
    let receipt = source_receipt(&sandbox);
    let cases = &receipt["cases"];

    // A Source-valid stream still reduces, with the reference's own lane answer.
    assert_eq!(cases["valid"]["admission"]["outcome"], "ok");
    assert_eq!(cases["valid"]["admission"]["count"], 2);
    assert_eq!(cases["valid"]["fold"]["outcome"], "ok");
    assert_eq!(cases["valid"]["fold"]["lane"], "done");

    // An admitted-but-unfoldable row: the reference's fold raises, so the
    // reference's `coord_incoherent_done_wps` raises too. There is no answer.
    assert_eq!(
        cases["unhashable_wp_id"]["admission"]["outcome"], "ok",
        "the reference ADMITS this row; its fold is what fails"
    );
    assert_eq!(cases["unhashable_wp_id"]["fold"]["outcome"], "raised");
    assert_eq!(cases["unhashable_wp_id"]["fold"]["type"], "TypeError");

    // Rows the reference's admission refuses.
    assert_eq!(cases["unknown_lane"]["admission"]["outcome"], "raised");
    assert_eq!(cases["unknown_lane"]["admission"]["type"], "StoreError");
    assert_eq!(
        cases["unknown_lane"]["admission"]["message"],
        "Invalid event structure on line 1: 'shipped' is not a valid Lane"
    );
    assert_eq!(cases["non_object_row"]["admission"]["type"], "StoreError");

    // And the committed consumer must not answer "no strand" for either of them.
    let fixture = init_repo();
    seed_branch(
        fixture.root(),
        SLUG,
        &text_row(&unhashable_wp_id_row()),
        "cf49-unhashable",
    );
    let unhashable = derive(&fixture, "cf49-unhashable", &["WP-A"]);
    assert!(
        unhashable.is_err(),
        "an admitted-but-unfoldable committed log must be a typed refusal, not a healthy \
         answer; the reference's own fold raises TypeError. Got {unhashable:?}"
    );

    seed_branch(
        fixture.root(),
        SLUG,
        &text_row(&unknown_lane_row()),
        "cf49-unknown-lane",
    );
    let refused = derive(&fixture, "cf49-unknown-lane", &["WP-A"]);
    assert!(
        refused.is_err(),
        "a row the reference's admission refuses must not read as no strand; got {refused:?}"
    );
}

/// The valid fold is untouched: lanes, candidate order, duplicates and skipped
/// non-lane/blank rows all still behave exactly as CF40 pinned them.
#[test]
fn the_valid_fold_order_and_duplicates_are_unchanged() {
    let fixture = init_repo();
    let mut content = String::new();
    content.push('\n');
    content.push_str(r#"{"event_type": "MissionCreated", "mission_slug": "cf49-mission"}"#);
    content.push('\n');
    for row in valid_rows() {
        content.push_str(&text_row(&row));
    }
    content.push_str("   \n");
    // A duplicate `done` row for WP-A must not duplicate the candidate answer.
    content.push_str(&text_row(&event(
        json!("WP-A"),
        "done",
        "2026-11-05T10:07:00+00:00",
        "01A0CF4900000000000000AA05",
    )));
    seed_branch(fixture.root(), SLUG, &content, "cf49-valid");

    let done = derive(&fixture, "cf49-valid", &["WP-A", "WP-B", "WP-A", "WP-C"])
        .expect("a Source-valid committed log is read");
    assert_eq!(done, vec!["WP-A".to_owned(), "WP-A".to_owned()]);
}

/// The derivation is read-only: the checkout, the index and the refs are
/// byte-identical afterwards, for both the refusal and the valid answer.
#[test]
fn the_derivation_writes_nothing() {
    let fixture = init_repo();
    seed_branch(
        fixture.root(),
        SLUG,
        &text_row(&unhashable_wp_id_row()),
        "cf49-readonly",
    );
    let before = git(fixture.root(), &["status", "--porcelain"]);
    let head_before = git(fixture.root(), &["rev-parse", "HEAD"]);
    let index_before = fs::read(fixture.root().join(".git/index")).expect("index bytes");
    let log_before = fs::read(log_path(fixture.root(), SLUG)).expect("log bytes");

    let _ = derive(&fixture, "cf49-readonly", &["WP-A"]);

    assert_eq!(git(fixture.root(), &["status", "--porcelain"]), before);
    assert_eq!(git(fixture.root(), &["rev-parse", "HEAD"]), head_before);
    assert_eq!(
        fs::read(fixture.root().join(".git/index")).expect("index bytes"),
        index_before
    );
    assert_eq!(
        fs::read(log_path(fixture.root(), SLUG)).expect("log bytes"),
        log_before
    );
}
