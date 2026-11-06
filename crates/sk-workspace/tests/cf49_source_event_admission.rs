//! CF49: Source event-store admission, paired against the pinned reference.
//!
//! The PINNED reference (`.venv-rust-oracle`, CPython 3.11.15, verified by the
//! fail-closed `scripts/rust/verify_oracle.py`) runs FIRST in its own isolated
//! child — fresh CWD/HOME, all seven `XDG_*` roots, `TMPDIR`/`TMP`/`TEMP`,
//! `PYTHONHASHSEED`, the lock-exact `PYTHONPATH` and the packs root — and asserts
//! that environment before it imports anything. Its receipt is the premise.
//!
//! # What is compared
//!
//! `specify_cli.status.store.read_events_from_text` (admission) followed by
//! `specify_cli.status.reduce` (the fold), against
//! `sk_status::replay::read_source_event_stream_from_text` (+ the canonical
//! `reduce_diary` where a fold is reached). For every case:
//!
//! * the reference's **phase** — admitted and folded, refused during admission
//!   (`StoreError`), or admitted and then crashed in the fold (`TypeError`);
//! * the reference's **message text** for every admission refusal;
//! * the reference's **mission-id back-fill** for a legacy slug-only row;
//! * which rows are non-lane (skipped in place) and which are annotations.
//!
//! # What this does NOT claim
//!
//! Exact parity of the reference's Python-internal exception wording for every
//! model failure (a `KeyError`'s `'to_lane'` repr versus this crate's own typed
//! detail), scalar-type ordering divergence inside the fold (a numeric `at` beside
//! a string `at` still crashes the reference and folds here), the reference's
//! `str.splitlines()` separator set, `NaN`/`Infinity` literals, or any health,
//! count or reachability promotion. Those are named in the lane report.

mod support;

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Map, Value, json};
use sk_status::replay::{read_source_event_stream_from_text, read_source_transitions_from_text};
use sk_status::{parse_diary, replay};

use support::write;

const SLUG: &str = "cf49-admission";
const MISSION_ID: &str = "01KJ0000000000000000000049";

// ---------------------------------------------------------------------------
// Cases
// ---------------------------------------------------------------------------

/// A base, Source-valid lane row.
fn base() -> Map<String, Value> {
    let Value::Object(map) = json!({
        "actor": "claude",
        "at": "2026-11-05T10:05:00+00:00",
        "event_id": "01A0CF4900000000000000AA01",
        "evidence": null,
        "execution_mode": "worktree",
        "force": false,
        "from_lane": "approved",
        "mission_slug": SLUG,
        "reason": null,
        "review_ref": null,
        "to_lane": "done",
        "wp_id": "WP-A",
    }) else {
        unreachable!("base row is an object")
    };
    map
}

fn row(overrides: Value) -> Map<String, Value> {
    let mut map = base();
    let Value::Object(patch) = overrides else {
        panic!("row overrides must be an object");
    };
    for (key, value) in patch {
        map.insert(key, value);
    }
    map
}

fn row_without(field: &str) -> Map<String, Value> {
    let mut map = base();
    map.remove(field);
    map
}

fn lines(rows: &[Map<String, Value>]) -> String {
    let mut text = String::new();
    for row in rows {
        text.push_str(&serde_json::to_string(row).expect("serialize row"));
        text.push('\n');
    }
    text
}

/// How the reference behaves for one case.
#[derive(Clone, Copy, Debug)]
enum SourceClass {
    /// Admitted; the fold then answers normally.
    Folded,
    /// Admitted; the reference's own fold then raises `TypeError`.
    FoldCrashes,
    /// Refused during admission with `StoreError`.
    RefusedDuringAdmission,
}

impl SourceClass {
    fn label(self) -> &'static str {
        match self {
            Self::Folded => "folded",
            Self::FoldCrashes => "fold_crashes",
            Self::RefusedDuringAdmission => "refused_during_admission",
        }
    }
}

struct Case {
    name: &'static str,
    content: String,
    class: SourceClass,
    /// The reference's exact admission message, when it is stable across Python
    /// builds (a `JSONDecodeError` message is not, so it is left out).
    source_message: Option<&'static str>,
    /// Tokens this crate's refusal must contain (its model-failure wording is its
    /// own; the offending field or value must still be named).
    native_tokens: &'static [&'static str],
}

fn cases() -> Vec<Case> {
    let mut cases = Vec::new();
    let mut push = |name: &'static str,
                    content: String,
                    class: SourceClass,
                    source_message: Option<&'static str>,
                    native_tokens: &'static [&'static str]| {
        cases.push(Case {
            name,
            content,
            class,
            source_message,
            native_tokens,
        });
    };

    // --- admitted and folded -------------------------------------------------
    push(
        "valid_single",
        lines(&[base()]),
        SourceClass::Folded,
        None,
        &[],
    );
    push(
        "valid_blank_and_whitespace_lines",
        format!("\n   \n{}", lines(&[base()])),
        SourceClass::Folded,
        None,
        &[],
    );
    push(
        "valid_two_rows",
        lines(&[
            base(),
            row(
                json!({"event_id": "01A0CF4900000000000000AA02", "wp_id": "WP-B", "to_lane": "approved"}),
            ),
        ]),
        SourceClass::Folded,
        None,
        &[],
    );
    push(
        "valid_duplicate_event_id",
        lines(&[
            base(),
            row(json!({"from_lane": "planned", "to_lane": "claimed"})),
        ]),
        SourceClass::Folded,
        None,
        &[],
    );
    push(
        "nonlane_event_type_is_skipped",
        lines(&[
            row(json!({"event_id": "01A0CF4900000000000000AA03"})),
            json!({"event_type": "MissionCreated", "mission_slug": SLUG})
                .as_object()
                .expect("object")
                .clone(),
        ]),
        SourceClass::Folded,
        None,
        &[],
    );
    push(
        "annotation_is_admitted_not_folded",
        lines(&[
            row(json!({"event_id": "01A0CF4900000000000000AA04"})),
            json!({
                "actor": "claude",
                "at": "2026-11-05T10:06:00+00:00",
                "delta": {"note": "runtime note"},
                "event_id": "01A0CF4900000000000000AB01",
                "kind": "annotation",
                "wp_id": "WP-A",
            })
            .as_object()
            .expect("object")
            .clone(),
        ]),
        SourceClass::Folded,
        None,
        &[],
    );
    push(
        "legacy_slug_only_row",
        lines(&[row_without("mission_id")]),
        SourceClass::Folded,
        None,
        &[],
    );

    // --- admitted, then the reference's own fold crashes ---------------------
    push(
        "container_wp_id",
        lines(&[row(json!({"wp_id": ["WP-A"]}))]),
        SourceClass::FoldCrashes,
        None,
        &["unhashable type: 'list'"],
    );
    push(
        "container_at",
        lines(&[row(json!({"at": {"t": 1}}))]),
        SourceClass::FoldCrashes,
        None,
        &["unhashable type: 'dict'"],
    );
    push(
        "container_event_id",
        lines(&[row(json!({"event_id": ["01A0CF4900000000000000AA05"]}))]),
        SourceClass::FoldCrashes,
        None,
        &["unhashable type: 'list'"],
    );

    // --- refused during admission -------------------------------------------
    push(
        "container_mission_slug",
        lines(&[row(json!({"mission_slug": [SLUG]}))]),
        SourceClass::RefusedDuringAdmission,
        Some("Invalid event structure on line 1: unhashable type: 'list'"),
        &["unhashable type: 'list'"],
    );
    push(
        "numeric_mission_slug",
        lines(&[row(json!({"mission_slug": 5}))]),
        SourceClass::RefusedDuringAdmission,
        Some("Invalid event structure on line 1: 'int' object has no attribute 'strip'"),
        &["'int' object has no attribute 'strip'"],
    );
    push(
        "falsy_slug_with_container_feature_slug",
        lines(&[row(json!({"feature_slug": [SLUG], "mission_slug": null}))]),
        SourceClass::RefusedDuringAdmission,
        Some("Invalid event structure on line 1: unhashable type: 'list'"),
        &["unhashable type: 'list'"],
    );
    push(
        "bad_json",
        "{not json\n".to_owned(),
        SourceClass::RefusedDuringAdmission,
        None,
        &["Invalid JSON on line 1"],
    );
    push(
        "non_object_row",
        "[1, 2, 3]\n".to_owned(),
        SourceClass::RefusedDuringAdmission,
        Some("Invalid event structure on line 1: expected JSON object"),
        &[],
    );
    push(
        "unknown_kind",
        lines(&[json!({"kind": "weird"})
            .as_object()
            .expect("object")
            .clone()]),
        SourceClass::RefusedDuringAdmission,
        Some("Unknown event kind 'weird' on line 1"),
        &[],
    );
    push(
        "missing_to_lane",
        lines(&[row_without("to_lane")]),
        SourceClass::RefusedDuringAdmission,
        Some("Invalid event structure on line 1: 'to_lane'"),
        &["to_lane"],
    );
    push(
        "unknown_lane",
        lines(&[row(json!({"to_lane": "shipped"}))]),
        SourceClass::RefusedDuringAdmission,
        Some("Invalid event structure on line 1: 'shipped' is not a valid Lane"),
        &["'shipped' is not a valid Lane"],
    );
    push(
        "lane_not_string",
        lines(&[row(json!({"to_lane": 5}))]),
        SourceClass::RefusedDuringAdmission,
        Some("Invalid event structure on line 1: 5 is not a valid Lane"),
        &["5 is not a valid Lane"],
    );
    push(
        "missing_actor",
        lines(&[row_without("actor")]),
        SourceClass::RefusedDuringAdmission,
        Some("Invalid event structure on line 1: 'actor'"),
        &["actor"],
    );
    push(
        "actor_wrong_keys",
        lines(&[row(
            json!({"actor": {"role": "implementer", "profile": null, "tool": "claude"}}),
        )]),
        SourceClass::RefusedDuringAdmission,
        None,
        &["actor", "role"],
    );
    push(
        "actor_profile_source_without_profile",
        lines(&[row(json!({"actor": {
            "model": null,
            "profile": null,
            "profile_source": "flag",
            "role": "implementer",
            "tool": "claude",
        }}))]),
        SourceClass::RefusedDuringAdmission,
        Some(
            "Invalid event structure on line 1: structured actor profile_source must be null exactly when profile is null",
        ),
        &["actor", "profile_source"],
    );
    // --- admitted and folded by the reference, refused here (declared gaps) ---
    push(
        "actor_with_profile_source",
        lines(&[row(json!({"actor": {
            "model": "claude-opus",
            "profile": "implementer-ivan",
            "profile_source": "flag",
            "role": "implementer",
            "tool": "claude",
        }}))]),
        SourceClass::Folded,
        None,
        &[],
    );
    push(
        "force_not_a_boolean",
        lines(&[row(json!({"force": "yes"}))]),
        SourceClass::Folded,
        None,
        &[],
    );
    push(
        "numeric_event_id",
        lines(&[row(json!({"event_id": 123}))]),
        SourceClass::Folded,
        None,
        &[],
    );
    push(
        "evidence_review_missing_verdict",
        lines(&[row(
            json!({"evidence": {"review": {"reference": "ref", "reviewer": "r"}}}),
        )]),
        SourceClass::RefusedDuringAdmission,
        Some("Invalid event structure on line 1: 'verdict'"),
        &["verdict"],
    );
    push(
        "evidence_review_not_object",
        lines(&[row(json!({"evidence": {"review": "x"}}))]),
        SourceClass::RefusedDuringAdmission,
        None,
        &["review"],
    );
    push(
        "evidence_repos_not_a_list",
        lines(&[row(json!({"evidence": {
            "repos": "abc",
            "review": {"reference": "ref", "reviewer": "r", "verdict": "approved"},
        }}))]),
        SourceClass::RefusedDuringAdmission,
        None,
        &["evidence", "repos"],
    );
    push(
        "annotation_unknown_subtask_status",
        lines(&[
            row(json!({"event_id": "01A0CF4900000000000000AA06"})),
            json!({
                "actor": "claude",
                "at": "2026-11-05T10:06:00+00:00",
                "delta": {"subtasks": {"1": "nonsense"}},
                "event_id": "01A0CF4900000000000000AB02",
                "kind": "annotation",
                "wp_id": "WP-A",
            })
            .as_object()
            .expect("object")
            .clone(),
        ]),
        SourceClass::RefusedDuringAdmission,
        Some("Invalid event structure on line 2: 'nonsense' is not a valid Lane"),
        &["nonsense", "is not a valid Lane"],
    );
    push(
        "annotation_event_id_not_a_ulid",
        lines(&[
            row(json!({"event_id": "01A0CF4900000000000000AA07"})),
            json!({
                "actor": "claude",
                "at": "2026-11-05T10:06:00+00:00",
                "delta": {"note": "x"},
                "event_id": "nope",
                "kind": "annotation",
                "wp_id": "WP-A",
            })
            .as_object()
            .expect("object")
            .clone(),
        ]),
        SourceClass::RefusedDuringAdmission,
        Some(
            "Invalid event structure on line 2: InnerStateChanged.event_id is not a valid ULID: 'nope'",
        ),
        &["ULID"],
    );

    cases
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

    /// The Mission directory inside an isolated `kitty-specs/` root.
    fn feature_dir(&self) -> PathBuf {
        self.path("kitty-specs").join(SLUG)
    }

    fn write_meta(&self, mission_id: Option<&str>) {
        let body = match mission_id {
            Some(mission_id) => json!({"mission_id": mission_id, "slug": SLUG}),
            None => json!({"slug": SLUG}),
        };
        write(
            &self.feature_dir().join("meta.json"),
            &format!("{}\n", body),
        );
    }
}

fn source_receipt(sandbox: &Sandbox, cases: &[Case]) -> Value {
    let cases_path = sandbox.path("cases.json");
    let mut payload = Map::new();
    for case in cases {
        payload.insert(case.name.to_owned(), Value::String(case.content.clone()));
    }
    fs::write(
        &cases_path,
        serde_json::to_string(&Value::Object(payload)).expect("cases json"),
    )
    .expect("cases file");

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
from specify_cli.status import read_events_from_text, reduce
import specify_cli.status as status_module
assert Path(status_module.__file__).is_relative_to(repo / "src")

cases = json.loads(Path(sys.argv[2]).read_text(encoding="utf-8"))
feature_dir = Path(sys.argv[3])
out = {}
for name, content in cases.items():
    record = {"admission": None, "fold": None}
    try:
        events = read_events_from_text(feature_dir, content)
    except Exception as exc:  # noqa: BLE001 - the exact phase IS the receipt
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
            record["fold"] = {
                "outcome": "ok",
                "lane": (snapshot.work_packages.get("WP-A") or {}).get("lane"),
            }
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

/// Every case's Source phase and message, then this crate's admission decision.
#[test]
fn the_admission_decision_matches_the_pinned_reference_phase_by_phase() {
    const CHILD_ENV: &str = "SK_CF49_NATIVE_MATRIX_ROOT";
    let Some(root) = std::env::var_os(CHILD_ENV) else {
        let sandbox = Sandbox::new();
        sandbox.write_meta(Some(MISSION_ID));
        let receipt = source_receipt(&sandbox, &cases());
        fs::write(
            sandbox.path("source-receipt.json"),
            serde_json::to_vec(&receipt).unwrap(),
        )
        .unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "the_admission_decision_matches_the_pinned_reference_phase_by_phase",
                "--nocapture",
            ])
            .current_dir(sandbox.path("cwd"))
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env(CHILD_ENV, &sandbox.root)
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
    // Actual Native child assertions precede receipt/metadata/business reads.
    assert_eq!(std::env::current_dir().unwrap(), sandbox.path("cwd"));
    for (key, name) in sandbox.env_pairs() {
        assert_eq!(
            std::env::var_os(key).map(PathBuf::from),
            Some(sandbox.path(name)),
            "Native child {key}"
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
    let cases = cases();
    let receipt: Value =
        serde_json::from_slice(&fs::read(sandbox.path("source-receipt.json")).unwrap()).unwrap();
    let source = &receipt["cases"];

    for case in &cases {
        let record = &source[case.name];
        let admission = &record["admission"];
        let fold = &record["fold"];
        match case.class {
            SourceClass::Folded => {
                assert_eq!(
                    admission["outcome"], "ok",
                    "{}: the reference admits this row ({admission})",
                    case.name
                );
                assert_eq!(
                    fold["outcome"], "ok",
                    "{}: the reference folds this row ({fold})",
                    case.name
                );
            }
            SourceClass::FoldCrashes => {
                assert_eq!(
                    admission["outcome"], "ok",
                    "{}: the reference ADMITS this row; its fold is what fails ({admission})",
                    case.name
                );
                assert_eq!(fold["outcome"], "raised", "{}: {fold}", case.name);
                assert_eq!(fold["type"], "TypeError", "{}: {fold}", case.name);
            }
            SourceClass::RefusedDuringAdmission => {
                assert_eq!(
                    admission["outcome"], "raised",
                    "{}: the reference refuses this row ({admission})",
                    case.name
                );
                assert_eq!(
                    admission["type"], "StoreError",
                    "{}: {admission}",
                    case.name
                );
                if let Some(expected) = case.source_message {
                    assert_eq!(
                        admission["message"], expected,
                        "{}: the reference's own message",
                        case.name
                    );
                }
            }
        }

        let native = read_source_event_stream_from_text(&sandbox.feature_dir(), &case.content);
        match case.class {
            SourceClass::Folded => {
                let stream = native
                    .unwrap_or_else(|error| panic!("{}: unexpected refusal {error}", case.name));
                assert!(
                    stream.annotations.is_empty() || case.name.starts_with("annotation"),
                    "{}: only the annotation case has annotations",
                    case.name
                );
                // The admitted rows are the reference's own lane rows, in order.
                let lanes: Vec<String> = stream
                    .transitions
                    .iter()
                    .map(|event| event.to_lane().as_str().to_owned())
                    .collect();
                assert_eq!(
                    Value::Array(lanes.iter().cloned().map(Value::String).collect()),
                    admission["lanes"],
                    "{}: admitted lane rows",
                    case.name
                );
                let mission_ids = Value::Array(
                    stream
                        .transitions
                        .iter()
                        .map(|event| {
                            event
                                .field("mission_id")
                                .and_then(|value| value.to_serde())
                                .unwrap_or(Value::Null)
                        })
                        .collect(),
                );
                assert_eq!(
                    mission_ids, admission["mission_ids"],
                    "{}: the legacy mission_id back-fill matches the reference",
                    case.name
                );
            }
            SourceClass::FoldCrashes => {
                let stream = native.unwrap_or_else(|error| {
                    panic!(
                        "{}: the reference ADMITS this row; admission must not refuse it ({error})",
                        case.name
                    )
                });
                let error =
                    replay::reduce_source_diary(&stream.transitions, &[]).expect_err(&format!(
                        "{}: an admitted-but-unfoldable row ({} in the reference) must fail in the \
                         fold, never produce an answer the reference cannot give",
                        case.name,
                        case.class.label()
                    ));
                let text = error.to_string();
                for token in case.native_tokens {
                    assert!(
                        text.contains(token),
                        "{}: fold refusal {text:?} must name {token:?}",
                        case.name
                    );
                }
                assert!(
                    matches!(
                        error,
                        sk_status::source_event::SourceFoldError::TypeError { .. }
                    ),
                    "{}: the refusal is the reference's fold-phase TypeError",
                    case.name
                );
            }
            SourceClass::RefusedDuringAdmission => {
                let error =
                    native.expect_err(&format!("{}: the reference refuses this row", case.name));
                let text = error.to_string();
                for token in case.native_tokens {
                    assert!(
                        text.contains(token),
                        "{}: refusal {text:?} must name {token:?}",
                        case.name
                    );
                }
                if let Some(expected) = case.source_message
                    && matches!(
                        case.name,
                        "non_object_row"
                            | "unknown_kind"
                            | "container_mission_slug"
                            | "unknown_lane"
                            | "lane_not_string"
                            | "numeric_mission_slug"
                            | "falsy_slug_with_container_feature_slug"
                            | "annotation_unknown_subtask_status"
                    )
                {
                    assert_eq!(text, expected, "{}: exact Source message", case.name);
                }
            }
        }
    }
}

fn mission_id_text(event: &sk_status::source_event::SourceStatusEvent) -> Option<String> {
    event.field("mission_id").and_then(|value| match value {
        sk_kernel::python_json::PythonJsonValue::String(text) => text.to_scalar_string().ok(),
        _ => None,
    })
}

/// The legacy back-fill is a real meta.json read, and it degrades to `None` the
/// way the reference's resolver does.
#[test]
fn the_legacy_mission_id_back_fill_reads_the_sibling_meta() {
    let sandbox = Sandbox::new();
    let content = lines(&[row_without("mission_id")]);

    sandbox.write_meta(Some(MISSION_ID));
    let with_meta = read_source_transitions_from_text(&sandbox.feature_dir(), &content)
        .expect("a legacy row is admitted");
    assert_eq!(with_meta.len(), 1);
    assert_eq!(mission_id_text(&with_meta[0]).as_deref(), Some(MISSION_ID));

    sandbox.write_meta(None);
    let without_id = read_source_transitions_from_text(&sandbox.feature_dir(), &content)
        .expect("still admitted");
    assert_eq!(mission_id_text(&without_id[0]), None);

    // A `mission_id` key that is present is never overwritten, even when null.
    let mut present = row_without("mission_id");
    present.insert("mission_id".to_owned(), Value::Null);
    let kept = read_source_transitions_from_text(&sandbox.feature_dir(), &lines(&[present]))
        .expect("admitted");
    assert_eq!(
        mission_id_text(&kept[0]),
        None,
        "a present null mission_id stays null (the reference only injects an absent key)"
    );

    // A modern row's own id wins over any meta.json resolution.
    let mut modern = row_without("mission_id");
    modern.insert(
        "mission_id".to_owned(),
        Value::String("01KJ0000000000000000000099".to_owned()),
    );
    sandbox.write_meta(Some(MISSION_ID));
    let own = read_source_transitions_from_text(&sandbox.feature_dir(), &lines(&[modern]))
        .expect("admitted");
    assert_eq!(
        mission_id_text(&own[0]).as_deref(),
        Some("01KJ0000000000000000000099")
    );
}

/// An unsafe slug is never resolved through the filesystem, and a symlinked
/// mission directory outside the specs root is refused as a resolution.
#[test]
fn an_unsafe_or_escaping_slug_resolves_to_nothing() {
    let sandbox = Sandbox::new();
    let mut traversal = row_without("mission_id");
    traversal.insert(
        "mission_slug".to_owned(),
        Value::String("../outside".to_owned()),
    );
    let resolved = read_source_transitions_from_text(&sandbox.feature_dir(), &lines(&[traversal]))
        .expect("the row is admitted; only the id is unresolvable");
    assert_eq!(mission_id_text(&resolved[0]), None);

    // A sibling `meta.json` outside the specs root, reached through a symlink.
    let outside = sandbox.path("outside");
    write(
        &outside.join("meta.json"),
        &format!("{}\n", json!({"mission_id": MISSION_ID})),
    );
    let escaped = sandbox.path("kitty-specs").join("escaped");
    if std::os::unix::fs::symlink(&outside, &escaped).is_ok() {
        let mut row = row_without("mission_id");
        row.insert(
            "mission_slug".to_owned(),
            Value::String("escaped".to_owned()),
        );
        let resolved = read_source_transitions_from_text(&sandbox.feature_dir(), &lines(&[row]))
            .expect("admitted");
        assert_eq!(
            mission_id_text(&resolved[0]),
            None,
            "a symlink escaping the specs root never supplies a mission id"
        );
    }
}

/// Non-lane records are skipped in place and named, annotations are surfaced, and
/// the transitions-only view is the references' own partition.
#[test]
fn non_lane_rows_are_skipped_in_place_and_annotations_are_surfaced() {
    let sandbox = Sandbox::new();
    let content = format!(
        "{}{}\n{}",
        lines(&[
            base(),
            row(json!({"event_id": "01A0CF4900000000000000AA08"}))
        ]),
        r#"{"event_type": "MissionCreated"}"#,
        r#"{"event_name": "retrospective.captured"}"#
    );
    let stream =
        read_source_event_stream_from_text(&sandbox.feature_dir(), &content).expect("admitted");
    assert_eq!(stream.transitions.len(), 2);
    assert_eq!(stream.non_lane_lines, vec![3, 4]);
    assert!(stream.annotations.is_empty());
}

/// Admission and the diary parser are deliberately distinct capabilities: the
/// diary parser (the `spec_kitty_events.diary` contract) still accepts a row whose
/// identity the reference's fold cannot key on. Only the Source admission refuses
/// it, which is why a committed consumer must not answer from the diary alone.
#[test]
fn the_diary_parser_alone_is_not_source_admission() {
    let sandbox = Sandbox::new();
    let content = lines(&[row(json!({"wp_id": ["WP-A"]}))]);
    let rows: Vec<Value> = content
        .lines()
        .map(|line| serde_json::from_str(line).expect("row json"))
        .collect();
    let stream = parse_diary(&rows).expect("the diary contract admits the row");
    assert_eq!(stream.transitions.len(), 1);
    assert_eq!(
        replay::reduce_diary(&stream.transitions, &[]).event_count,
        1,
        "the diary fold produces a snapshot for a row the reference's fold crashes on"
    );
    let admitted = read_source_event_stream_from_text(&sandbox.feature_dir(), &content)
        .expect("Source admission also admits this row");
    let error = replay::reduce_source_diary(&admitted.transitions, &[])
        .expect_err("the Source-valued fold cannot key on the container identity");
    assert!(
        error.to_string().contains("unhashable type: 'list'"),
        "the fold keeps the reference's TypeError phase: {error}"
    );
}
