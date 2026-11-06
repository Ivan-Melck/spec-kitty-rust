//! CF62: the Source store's **caught versus escaped** execution outcomes, paired
//! against the pinned reference in TWO real children (the reference first).
//!
//! The reference's `_partition_event_stream_from_text` catches only
//! `JSONDecodeError` per line and `(KeyError, ValueError, TypeError,
//! AttributeError)` around admission. Everything else escapes to the caller: an
//! annotation's `int(Infinity)` `OverflowError`, `json.loads`'s digit-ceiling
//! `ValueError`, the metadata resolver's escaped `OSError`, and the metadata
//! decoder's `RecursionError`. Native must report the same distinction, not a
//! same-message `StoreError` refusal and not a healthy `None`.
//!
//! The Source child asserts its whole environment before importing anything, then
//! runs `read_event_stream_from_text` + `reduce` over fixtures the parent seeded.
//! The Native child asserts the same environment before any business read and runs
//! the typed boundary (`sk_status::source_execution::execute_source_diary`) plus
//! the metadata resolver.

#![cfg(unix)]

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Map, Value as Json, json};
use sk_kernel::PythonString;
use sk_kernel::python_json::{PythonJsonDict, PythonJsonValue, python_repr_value};
use sk_status::source_execution::{
    SourceAnnotationPass, SourceExecutionError, execute_source_diary,
};
use sk_status::source_metadata::SourceSlugResolver;

const CHILD_ENV: &str = "SK_CF62_PAIR_ROOT";
const FEATURE_DIR: &str = "cf62";
const ULID: &str = "01A0CF6200000000000000AA01";
const ULID2: &str = "01A0CF6200000000000000AA02";
const DEEP_DEPTH: usize = 2000;

fn annotation(patch: &str) -> String {
    format!(
        "{{\"actor\": \"claude\", \"at\": \"2026-11-06T10:00:00+00:00\", \"delta\": {patch}, \
         \"event_id\": \"{ULID}\", \"kind\": \"annotation\", \"wp_id\": \"WP-A\"}}\n"
    )
}

fn lane(slug: &str, extra: &str) -> String {
    format!(
        "{{\"actor\": \"claude\", \"at\": \"2026-11-06T10:00:00+00:00\", \"event_id\": \"{ULID2}\", \
         \"evidence\": null, \"execution_mode\": \"worktree\", \"force\": false, \
         \"from_lane\": \"planned\", {extra}\"mission_slug\": \"{slug}\", \"reason\": null, \
         \"review_ref\": null, \"to_lane\": \"done\", \"wp_id\": \"WP-A\"}}\n"
    )
}

/// Every case: name -> (log text, feature dir relative to the sandbox root).
fn cases() -> Vec<(&'static str, String, String)> {
    let owned = format!("kitty-specs/{FEATURE_DIR}");
    let big = "1".repeat(4301);
    vec![
        (
            "annotation_infinity",
            annotation("{\"shell_pid\": Infinity}"),
            owned.clone(),
        ),
        (
            "annotation_neg_infinity",
            annotation("{\"shell_pid\": -Infinity}"),
            owned.clone(),
        ),
        (
            "annotation_nan",
            annotation("{\"shell_pid\": NaN}"),
            owned.clone(),
        ),
        (
            "annotation_valid",
            annotation("{\"shell_pid\": 12}"),
            owned.clone(),
        ),
        ("annotation_delta_list", annotation("[]"), owned.clone()),
        (
            "annotation_bigint_string",
            annotation(&format!("{{\"shell_pid\": \"{big}\"}}")),
            owned.clone(),
        ),
        (
            "line_bigint",
            lane(FEATURE_DIR, &format!("\"wp_id\": {big}, ")),
            owned.clone(),
        ),
        ("lane_ok", lane(FEATURE_DIR, ""), owned.clone()),
        ("meta_ok", lane("cf62-meta-ok", ""), owned.clone()),
        ("meta_badjson", lane("cf62-meta-badjson", ""), owned.clone()),
        ("meta_array", lane("cf62-meta-array", ""), owned.clone()),
        ("meta_bigint", lane("cf62-meta-bigint", ""), owned.clone()),
        ("meta_missing", lane("cf62-meta-missing", ""), owned.clone()),
        ("meta_deep", lane("cf62-meta-deep", ""), owned.clone()),
        ("meta_eacces", lane("cf62-meta-eacces", ""), owned.clone()),
        (
            "meta_eacces_alias",
            lane("cf62-meta-eacces", ""),
            "specs-alias/cf62".to_owned(),
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

fn write(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent");
    }
    fs::write(path, body).expect("fixture write");
}

/// Seed the sibling `meta.json` variants the resolver matrix names.
fn seed_fixtures(root: &Path) {
    let specs = root.join("kitty-specs");
    fs::create_dir_all(specs.join(FEATURE_DIR)).expect("feature dir");
    write(
        &specs.join("cf62-meta-ok").join("meta.json"),
        "{\"mission_id\": \"01A0CF62META0000000000001\"}",
    );
    write(
        &specs.join("cf62-meta-badjson").join("meta.json"),
        "{not json",
    );
    write(&specs.join("cf62-meta-array").join("meta.json"), "[]");
    write(
        &specs.join("cf62-meta-bigint").join("meta.json"),
        &format!("{{\"mission_id\": {}}}", "1".repeat(4301)),
    );
    write(
        &specs.join("cf62-meta-deep").join("meta.json"),
        &format!("{}{}", "[".repeat(DEEP_DEPTH), "]".repeat(DEEP_DEPTH)),
    );
    // No `cf62-meta-missing` directory: the reference's absent-file arm.
    write(
        &specs.join("cf62-meta-eacces").join("meta.json"),
        "{\"mission_id\": \"denied\"}",
    );
    std::os::unix::fs::symlink("kitty-specs", root.join("specs-alias")).expect("specs alias");
    let denied = specs.join("cf62-meta-eacces");
    let mut permissions = fs::metadata(&denied)
        .expect("meta dir metadata")
        .permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o000);
    fs::set_permissions(&denied, permissions).expect("deny meta dir");
}

fn restore_fixture_permissions(root: &Path) {
    let denied = root.join("kitty-specs").join("cf62-meta-eacces");
    if let Ok(metadata) = fs::metadata(&denied) {
        let mut permissions = metadata.permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        let _ = fs::set_permissions(&denied, permissions);
    }
}

fn case_plan() -> Json {
    let mut plan = Map::new();
    for (name, content, feature_dir) in cases() {
        plan.insert(
            name.to_owned(),
            json!({"log": content, "feature_dir": feature_dir}),
        );
    }
    Json::Object(plan)
}

fn source_receipt(sandbox: &Sandbox) -> Json {
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
from specify_cli.status import reduce
from specify_cli.status.store import read_event_stream_from_text
import specify_cli.status.store as store_module
assert Path(store_module.__file__).is_relative_to(repo / "src")

plan = json.loads(Path(sys.argv[2]).read_text(encoding="utf-8"))
out = {"cases": {}}
for name, case in plan.items():
    feature_dir = sandbox / case["feature_dir"]
    record = {}
    try:
        stream = read_event_stream_from_text(feature_dir, case["log"])
    except BaseException as exc:  # noqa: BLE001 - the Source phase is the datum
        record["admission"] = {
            "outcome": "raised",
            "type": type(exc).__name__,
            "message": str(exc),
        }
        out["cases"][name] = record
        continue
    record["admission"] = {
        "outcome": "ok",
        "transitions": len(stream.transitions),
        "annotations": len(stream.annotations),
        "mission_ids": [repr(t.mission_id) for t in stream.transitions],
    }
    try:
        snapshot = reduce(stream.transitions, stream.annotations)
    except BaseException as exc:  # noqa: BLE001
        record["fold"] = {
            "outcome": "raised",
            "type": type(exc).__name__,
            "message": str(exc),
        }
        out["cases"][name] = record
        continue
    record["fold"] = {
        "outcome": "ok",
        "event_count": snapshot.to_dict()["event_count"],
    }
    out["cases"][name] = record
print(json.dumps(out, sort_keys=True))
"##,
    )
    .expect("source receipt script");

    let plan_path = sandbox.path("plan.json");
    fs::write(&plan_path, serde_json::to_vec(&case_plan()).expect("plan")).expect("plan file");

    let mut command = Command::new(oracle::python());
    command
        .arg(&script)
        .arg(repo_root())
        .arg(&plan_path)
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

/// A parsed object as the canonical ordered mapping (the J1 value domain exposes
/// the exact-object and named-entry forms).
fn as_object(value: &PythonJsonValue) -> PythonJsonDict {
    match value {
        PythonJsonValue::ExactObject(object) => object.clone(),
        PythonJsonValue::Object(entries) => {
            let mut object = PythonJsonDict::new();
            for (name, value) in entries {
                object.insert(PythonString::from_scalars(name), value.clone());
            }
            object
        }
        other => panic!("expected a JSON object, got {other:?}"),
    }
}

fn native_receipt(sandbox: &Sandbox) -> Json {
    let mut out = Map::new();
    for (name, content, feature_dir) in cases() {
        let directory = sandbox.path(&feature_dir);
        let mut record = Map::new();
        match execute_source_diary(&directory, &content, SourceAnnotationPass::Fold) {
            Err(error) => {
                let (kind, class, phase, line, message) = match &error {
                    SourceExecutionError::Store(store) => (
                        "store",
                        "StoreError".to_owned(),
                        store.phase().name().to_owned(),
                        match store {
                            sk_status::replay::SourceEventLogError::InvalidJson {
                                line, ..
                            }
                            | sk_status::replay::SourceEventLogError::NonObjectRow { line } => {
                                Some(*line)
                            }
                            _ => None,
                        },
                        store.to_string(),
                    ),
                    SourceExecutionError::Escaped(raised) => (
                        "escaped",
                        raised.class().name().to_owned(),
                        raised.phase().name().to_owned(),
                        raised.line(),
                        raised.message().to_owned(),
                    ),
                    SourceExecutionError::Operational(operational) => (
                        "operational",
                        String::new(),
                        operational.phase().name().to_owned(),
                        None,
                        operational.detail().to_owned(),
                    ),
                };
                record.insert(
                    "admission".to_owned(),
                    json!({
                        "outcome": kind,
                        "class": class,
                        "phase": phase,
                        "line": line,
                        "message": message,
                    }),
                );
            }
            Ok(execution) => {
                record.insert(
                    "admission".to_owned(),
                    json!({
                        "outcome": "ok",
                        "transitions": execution.stream.transitions.len(),
                        "annotations": execution.stream.annotations.len(),
                        "mission_ids": execution.stream.transitions.iter()
                            .map(|event| event
                                .field("mission_id")
                                .map(native_repr)
                                .unwrap_or_else(|| "None".to_owned()))
                            .collect::<Vec<_>>(),
                    }),
                );
                record.insert(
                    "fold".to_owned(),
                    json!({"outcome": "ok", "event_count": execution.snapshot.event_count}),
                );
            }
        }
        out.insert(name.to_owned(), Json::Object(record));
    }
    // The resolver's own typed outcome is exercised directly too, so the
    // metadata escape cannot be an artifact of the store loop.
    let mut resolver =
        SourceSlugResolver::new(&sandbox.path(&format!("kitty-specs/{FEATURE_DIR}")));
    let parsed =
        sk_kernel::python_json::parse(&lane("cf62-meta-eacces", "")).expect("lane row parses");
    let row = as_object(&parsed);
    let resolves = match resolver.resolve_for_row(&row) {
        Ok(value) => json!({"outcome": "ok", "value": value.is_some()}),
        Err(error) => json!({
            "outcome": "failed",
            "phase": error.phase().name(),
            "message": error.to_string(),
        }),
    };
    out.insert("__resolver_eacces".to_owned(), resolves);
    Json::Object(out)
}

#[test]
fn the_caught_versus_escaped_store_phases_match_the_pinned_reference() {
    let Some(root) = std::env::var_os(CHILD_ENV) else {
        let sandbox = Sandbox::new();
        seed_fixtures(&sandbox.root);
        let receipt = source_receipt(&sandbox);
        fs::write(
            sandbox.path("source-receipt.json"),
            serde_json::to_vec(&receipt).expect("receipt"),
        )
        .expect("receipt file");
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "the_caught_versus_escaped_store_phases_match_the_pinned_reference",
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
        let output = command.output().expect("native child");
        restore_fixture_permissions(&sandbox.root);
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
    let source: Json =
        serde_json::from_slice(&fs::read(sandbox.path("source-receipt.json")).expect("receipt"))
            .expect("receipt json");
    let native = native_receipt(&sandbox);
    let source_cases = source["cases"].as_object().expect("source cases");

    // Non-vacuity: the reference receipt must carry a real escape, a real caught
    // refusal, a real admission and a real resolved mission_id, so an all-null
    // comparison cannot pass.
    let raised = |name: &str| -> &Json { &source_cases[name]["admission"] };
    assert_eq!(
        raised("annotation_infinity")["type"],
        "OverflowError",
        "the reference lets int(Infinity) escape"
    );
    assert_eq!(
        raised("annotation_nan")["type"],
        "StoreError",
        "the reference catches int(NaN)"
    );
    assert_eq!(
        raised("line_bigint")["type"],
        "ValueError",
        "the reference's json.loads digit ceiling is a bare ValueError"
    );
    assert_eq!(
        raised("meta_deep")["type"],
        "RecursionError",
        "the reference's deep meta.json escapes the metadata catch"
    );
    assert_eq!(
        source_cases["meta_ok"]["admission"]["mission_ids"][0], "'01A0CF62META0000000000001'",
        "the reference resolves the legacy mission_id"
    );

    for (name, _, _) in cases() {
        let expected = &source_cases[name];
        let actual = &native[name];
        let expected_outcome = expected["admission"]["outcome"]
            .as_str()
            .expect("source outcome");
        let actual_outcome = actual["admission"]["outcome"]
            .as_str()
            .expect("native outcome");
        match (expected_outcome, actual_outcome) {
            ("ok", "ok") => {
                for field in ["transitions", "annotations", "mission_ids"] {
                    assert_eq!(
                        actual["admission"][field], expected["admission"][field],
                        "{name}: admission {field}"
                    );
                }
                assert_eq!(
                    actual["fold"]["event_count"], expected["fold"]["event_count"],
                    "{name}: fold event_count"
                );
            }
            ("raised", "store") => {
                // The reference's caught `StoreError` text is reproduced exactly.
                assert_eq!(
                    expected["admission"]["type"], "StoreError",
                    "{name}: a `store` bucket must be one the reference caught"
                );
                assert_eq!(
                    actual["admission"]["message"], expected["admission"]["message"],
                    "{name}: the reference's own StoreError text"
                );
            }
            ("raised", "escaped") => {
                let expected_type = expected["admission"]["type"].as_str().expect("source type");
                let actual_class = actual["admission"]["class"].as_str().expect("class");
                assert_ne!(
                    expected_type, "StoreError",
                    "{name}: an escaped outcome must not be a caught StoreError"
                );
                if matches!(name, "meta_eacces" | "meta_eacces_alias") {
                    // Documented divergence: the reference raises the OS-specific
                    // subclass (`PermissionError`); Native names the `OSError`
                    // family rather than guessing the subclass.
                    assert_eq!(
                        expected_type, "PermissionError",
                        "{name}: actual Source permission failure"
                    );
                    assert_eq!(actual_class, "OSError", "{name}: the OSError family");
                    assert_eq!(
                        actual["admission"]["phase"], "mission_id_exists",
                        "{name}: the exists() phase"
                    );
                    continue;
                }
                assert_eq!(actual_class, expected_type, "{name}: the Python class");
                assert_eq!(
                    actual["admission"]["message"], expected["admission"]["message"],
                    "{name}: the reference's own message"
                );
            }
            ("raised", "operational") if name == "meta_deep" => {
                // Documented divergence: Native's J1 reader has an operational
                // frame budget below CPython's own acceptance ceiling, so it says
                // "native limit" instead of fabricating a `RecursionError`.
                assert_eq!(
                    actual["admission"]["phase"], "mission_id_decode",
                    "{name}: the decode phase"
                );
                assert!(
                    actual["admission"]["message"]
                        .as_str()
                        .expect("detail")
                        .contains("frame budget"),
                    "{name}: the native limit is named as one: {actual}"
                );
            }
            (expected, actual) => panic!(
                "{name}: the reference outcome {expected:?} must not become {actual:?} \
                 (a caught refusal, an escape and a healthy answer are different answers)"
            ),
        }
    }

    // The resolver's direct leg agrees: the `exists()` escape is not absorbed.
    assert_eq!(
        native["__resolver_eacces"]["outcome"], "failed",
        "an unreadable meta.json is not a healthy None"
    );
    assert_eq!(
        native["__resolver_eacces"]["phase"], "mission_id_exists",
        "the resolver reports the exists() phase"
    );
}
