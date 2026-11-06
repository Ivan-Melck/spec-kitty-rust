//! CF54: the Source-valued event model and fold, paired against the pinned
//! reference in TWO real children.
//!
//! The Source child asserts its whole environment before it imports anything,
//! computes `read_events_from_text` + `reduce`, and prints a receipt. The Native
//! child asserts the same environment before any business read, computes
//! `sk_status::source_event::read_source_event_stream_from_text` +
//! `reduce_source_diary`, and compares the phases/values case by case.
//!
//! Cases are the CF54 gaps: 5-key actor, non-boolean `force`, non-string/big
//! `event_id`, mixed scalar `at` (fold `TypeError`), homogeneous numeric vs
//! string ordering, bool/int duplicate identity, exact/duplicate-key/non-finite
//! parsing, container identity hash failure, and non-mapping `policy_metadata`.

mod support;

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Map, Value, json};
use sk_kernel::python_json::{PythonJsonValue, python_repr_value};
use sk_status::replay::{read_source_event_stream_from_text, reduce_source_diary};

const CHILD_ENV: &str = "SK_CF54_PAIR_ROOT";
const SLUG: &str = "cf54-pair";

fn base() -> Map<String, Value> {
    let Value::Object(map) = json!({
        "actor": "claude",
        "at": "2026-11-05T10:05:00+00:00",
        "event_id": "01A0CF5400000000000000AA01",
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
        unreachable!("base row is an object");
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

fn lines(rows: &[Map<String, Value>]) -> String {
    let mut text = String::new();
    for row in rows {
        text.push_str(&serde_json::to_string(row).expect("serialize row"));
        text.push('\n');
    }
    text
}

fn duplicate_key_text() -> String {
    format!(
        "{{\"actor\":\"claude\",\"at\":\"first\",\"at\":\"second\",\"event_id\":\"01A0CF5400000000000000AA09\",\"execution_mode\":\"worktree\",\"force\":false,\"from_lane\":\"approved\",\"mission_slug\":\"{SLUG}\",\"to_lane\":\"done\",\"wp_id\":\"WP-A\"}}\n"
    )
}

fn cases() -> Vec<(&'static str, String)> {
    vec![
        (
            "actor_5key",
            lines(&[row(json!({"actor": {
                "model": "claude-opus", "profile": "implementer-ivan",
                "profile_source": "flag", "role": "implementer", "tool": "claude"}}))]),
        ),
        (
            "actor_container",
            lines(&[row(json!({"actor": ["claude"]}))]),
        ),
        ("force_yes", lines(&[row(json!({"force": "yes"}))])),
        ("force_empty_list", lines(&[row(json!({"force": []}))])),
        ("event_id_int", lines(&[row(json!({"event_id": 123}))])),
        (
            "event_id_bigint",
            format!(
                "{{\"actor\":\"claude\",\"at\":\"2026-11-05T10:05:00+00:00\",\"event_id\":10000000000000000000000000000000000000000,\"execution_mode\":\"worktree\",\"force\":false,\"from_lane\":\"approved\",\"mission_slug\":\"{SLUG}\",\"to_lane\":\"done\",\"wp_id\":\"WP-A\"}}\n"
            ),
        ),
        (
            "event_id_true_and_one",
            lines(&[
                row(json!({"event_id": true})),
                row(
                    json!({"event_id": 1, "at": "2026-11-05T10:06:00+00:00", "to_lane": "canceled"}),
                ),
            ]),
        ),
        (
            "at_numeric_order",
            lines(&[
                row(
                    json!({"at": 10, "from_lane": "planned", "to_lane": "claimed", "event_id": "01A0CF5400000000000000AA03"}),
                ),
                row(
                    json!({"at": 2, "from_lane": "planned", "to_lane": "canceled", "event_id": "01A0CF5400000000000000AA04"}),
                ),
            ]),
        ),
        (
            "at_string_order",
            lines(&[
                row(
                    json!({"at": "10", "from_lane": "planned", "to_lane": "claimed", "event_id": "01A0CF5400000000000000AA05"}),
                ),
                row(
                    json!({"at": "2", "from_lane": "planned", "to_lane": "canceled", "event_id": "01A0CF5400000000000000AA06"}),
                ),
            ]),
        ),
        (
            "event_id_numeric_tiebreak",
            lines(&[
                row(
                    json!({"at": "same", "event_id": 10, "from_lane": "planned", "to_lane": "claimed"}),
                ),
                row(
                    json!({"at": "same", "event_id": 9, "from_lane": "planned", "to_lane": "canceled"}),
                ),
            ]),
        ),
        (
            "event_id_string_tiebreak",
            lines(&[
                row(
                    json!({"at": "same", "event_id": "10", "from_lane": "planned", "to_lane": "claimed"}),
                ),
                row(
                    json!({"at": "same", "event_id": "9", "from_lane": "planned", "to_lane": "canceled"}),
                ),
            ]),
        ),
        (
            "at_mixed_scalar",
            lines(&[
                base(),
                row(json!({"at": 5, "event_id": "01A0CF5400000000000000AA02"})),
            ]),
        ),
        ("container_wp_id", lines(&[row(json!({"wp_id": ["WP-A"]}))])),
        (
            "container_event_id",
            lines(&[row(json!({"event_id": ["01A0CF5400000000000000AA0A"]}))]),
        ),
        (
            "container_at_approved",
            lines(&[row(json!({"at": {"t": 1}}))]),
        ),
        (
            "container_at_planned",
            lines(&[row(
                json!({"at": {"t": 1}, "from_lane": "planned", "to_lane": "claimed"}),
            )]),
        ),
        ("duplicate_at_key", duplicate_key_text()),
        (
            "at_nan_single",
            format!(
                "{{\"actor\":\"claude\",\"at\":NaN,\"event_id\":\"01A0CF5400000000000000AA01\",\"execution_mode\":\"worktree\",\"force\":false,\"from_lane\":\"approved\",\"mission_slug\":\"{SLUG}\",\"to_lane\":\"done\",\"wp_id\":\"WP-A\"}}\n"
            ),
        ),
        (
            "at_infinity_single",
            format!(
                "{{\"actor\":\"claude\",\"at\":Infinity,\"event_id\":\"01A0CF5400000000000000AA01\",\"execution_mode\":\"worktree\",\"force\":false,\"from_lane\":\"approved\",\"mission_slug\":\"{SLUG}\",\"to_lane\":\"done\",\"wp_id\":\"WP-A\"}}\n"
            ),
        ),
        (
            "policy_metadata_list_claim",
            lines(&[row(json!({
                "from_lane": "planned", "to_lane": "claimed", "policy_metadata": ["x"],
                "event_id": "01A0CF5400000000000000AA0B"}))]),
        ),
    ]
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

    fn feature_dir(&self) -> PathBuf {
        self.path("kitty-specs").join(SLUG)
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

fn source_receipt(sandbox: &Sandbox) -> Value {
    let cases_path = sandbox.path("cases.json");
    let payload: Map<String, Value> = cases()
        .into_iter()
        .map(|(name, content)| (name.to_owned(), Value::String(content)))
        .collect();
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
    except Exception as exc:
        record["admission"] = {"outcome": "raised", "type": type(exc).__name__, "message": str(exc)}
    else:
        record["admission"] = {
            "outcome": "ok",
            "count": len(events),
            "to_lanes": [str(event.to_lane) for event in events],
            "event_ids": [repr(event.event_id) for event in events],
            "ats": [repr(event.at) for event in events],
            "forces": [repr(event.force) for event in events],
            "actors": [
                (event.actor.get("tool") if isinstance(event.actor, dict) else event.actor)
                for event in events
            ],
        }
        try:
            snapshot = reduce(events)
        except Exception as exc:
            record["fold"] = {"outcome": "raised", "type": type(exc).__name__, "message": str(exc)}
        else:
            state = snapshot.work_packages.get("WP-A") or {}
            record["fold"] = {
                "outcome": "ok",
                "event_count": snapshot.event_count,
                "last_event_id": repr(snapshot.last_event_id),
                "materialized_at": repr(snapshot.materialized_at),
                "lane": state.get("lane"),
                "force_count": repr(state.get("force_count")),
            }
    out[name] = record
print(json.dumps({"cases": out}, sort_keys=True))
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
    serde_json::from_slice(&output.stdout).expect("reference JSON receipt")
}

fn native_repr(value: &PythonJsonValue) -> String {
    match value {
        PythonJsonValue::String(text) => text.repr(),
        other => python_repr_value(other),
    }
}

fn native_receipt(sandbox: &Sandbox) -> Value {
    let mut out = Map::new();
    for (name, content) in cases() {
        let mut record = Map::new();
        match read_source_event_stream_from_text(&sandbox.feature_dir(), &content) {
            Err(error) => {
                record.insert(
                    "admission".to_owned(),
                    json!({"outcome": "raised", "message": error.to_string()}),
                );
            }
            Ok(stream) => {
                record.insert(
                    "admission".to_owned(),
                    json!({
                        "outcome": "ok",
                        "count": stream.transitions.len(),
                        "to_lanes": stream.transitions.iter()
                            .map(|event| event.to_lane().as_str().to_owned())
                            .collect::<Vec<_>>(),
                        "event_ids": stream.transitions.iter()
                            .map(|event| native_repr(event.event_id()))
                            .collect::<Vec<_>>(),
                        "ats": stream.transitions.iter()
                            .map(|event| native_repr(event.at()))
                            .collect::<Vec<_>>(),
                        "forces": stream.transitions.iter()
                            .map(|event| native_repr(event.force()))
                            .collect::<Vec<_>>(),
                        "actors": stream.transitions.iter()
                            .map(|event| event.actor_identity().to_scalar_string()
                                .unwrap_or_default())
                            .collect::<Vec<_>>(),
                    }),
                );
                match reduce_source_diary(&stream.transitions, &[]) {
                    Err(error) => {
                        record.insert(
                            "fold".to_owned(),
                            json!({"outcome": "raised", "message": error.to_string()}),
                        );
                    }
                    Ok(snapshot) => {
                        let state = snapshot.work_package_by_str("WP-A");
                        record.insert(
                            "fold".to_owned(),
                            json!({
                                "outcome": "ok",
                                "event_count": snapshot.event_count,
                                "last_event_id": snapshot.last_event_id.as_ref()
                                    .map(native_repr)
                                    .unwrap_or_else(|| "None".to_owned()),
                                "materialized_at": native_repr(&snapshot.materialized_at),
                                "lane": state.and_then(|state| state.lane())
                                    .map(|lane| lane.as_str().to_owned()),
                                "force_count": state.and_then(|state| state.get("force_count"))
                                    .map(native_repr)
                                    .unwrap_or_else(|| "None".to_owned()),
                            }),
                        );
                    }
                }
            }
        }
        out.insert(name.to_owned(), Value::Object(record));
    }
    Value::Object(out)
}

#[test]
fn the_source_value_phases_and_values_match_the_pinned_reference() {
    let Some(root) = std::env::var_os(CHILD_ENV) else {
        let sandbox = Sandbox::new();
        // `meta.json` is absent on purpose: every case rows a modern id or
        // tolerates an unresolvable legacy slug (the reference only warns).
        let receipt = source_receipt(&sandbox);
        fs::write(
            sandbox.path("source-receipt.json"),
            serde_json::to_vec(&receipt).unwrap(),
        )
        .unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "the_source_value_phases_and_values_match_the_pinned_reference",
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
    sandbox.assert_isolation();
    let source: Value =
        serde_json::from_slice(&fs::read(sandbox.path("source-receipt.json")).unwrap()).unwrap();
    let native = native_receipt(&sandbox);
    let source_cases = source["cases"].as_object().expect("cases");
    for (name, _) in cases() {
        let expected = &source_cases[name];
        let actual = &native[name];
        assert_eq!(
            actual["admission"]["outcome"], expected["admission"]["outcome"],
            "{name}: admission phase"
        );
        if expected["admission"]["outcome"] == "ok" {
            for field in ["count", "to_lanes", "event_ids", "ats", "forces", "actors"] {
                assert_eq!(
                    actual["admission"][field], expected["admission"][field],
                    "{name}: admission {field}"
                );
            }
        }
        assert_eq!(
            actual["fold"]["outcome"], expected["fold"]["outcome"],
            "{name}: fold phase"
        );
        if expected["fold"]["outcome"] == "raised" {
            assert_eq!(
                actual["fold"]["message"], expected["fold"]["message"],
                "{name}: the reference's own fold message"
            );
        } else {
            for field in [
                "event_count",
                "last_event_id",
                "materialized_at",
                "lane",
                "force_count",
            ] {
                assert_eq!(
                    actual["fold"][field], expected["fold"][field],
                    "{name}: fold {field}"
                );
            }
        }
    }
}
