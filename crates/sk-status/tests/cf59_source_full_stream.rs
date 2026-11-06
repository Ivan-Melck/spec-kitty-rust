//! CF59: the Source-valued annotation model, its fold and the lazy legacy
//! `mission_slug` metadata resolver, paired against the pinned reference in TWO
//! real children (the reference receipt first).
//!
//! The Source child asserts its whole environment before it imports anything,
//! then runs `read_event_stream_from_text` + `reduce` and the store's own
//! `_SlugResolver` over fixtures the parent seeds. The Native child asserts the
//! same environment before any business read, runs
//! `sk_status::replay::read_source_event_stream_from_text` +
//! `reduce_source_diary` + `sk_status::source_metadata::SourceSlugResolver`, and
//! compares the phase/values case by case.
//!
//! The cases are the CF59 gaps: a 5-key annotation actor, non-finite and
//! container delta values, `str()`-coerced `wp_id`/`at`, the ULID pattern's
//! trailing-newline `$` behaviour, `int()`/`list()`/`items()` coercion phases,
//! the annotation fold pass (ordering, replace slots, subtasks, notes, tracker
//! refs, review sentinel, claim release, the `uninitialized` seed), and the
//! legacy metadata resolver's path/exists/read/UTF-8/JSON/catch/cache phases.

#![cfg(unix)]

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Map, Value as Json, json};
use sk_kernel::PythonString;
use sk_kernel::python_json::{PythonJsonValue, python_float_repr, python_repr_value};
use sk_status::replay::{read_source_event_stream_from_text, reduce_source_diary};
use sk_status::source_metadata::SourceSlugResolver;

const CHILD_ENV: &str = "SK_CF59_PAIR_ROOT";
const FEATURE_DIR: &str = "cf59-pair";
const ULID: &str = "01A0CF5900000000000000AA01";
const ULID2: &str = "01A0CF5900000000000000AA02";

fn annotation(patch: Json) -> Json {
    let mut row = json!({
        "actor": "claude",
        "at": "2026-11-05T10:05:00+00:00",
        "delta": {"role": "implementer"},
        "event_id": ULID,
        "kind": "annotation",
        "wp_id": "WP-A",
    });
    let (Json::Object(target), Json::Object(extra)) = (&mut row, patch) else {
        panic!("object");
    };
    for (key, value) in extra {
        target.insert(key, value);
    }
    row
}

fn lane(patch: Json) -> Json {
    let mut row = json!({
        "actor": "claude",
        "at": "2026-11-05T10:00:00+00:00",
        "event_id": ULID2,
        "evidence": null,
        "execution_mode": "worktree",
        "force": false,
        "from_lane": "planned",
        "mission_slug": FEATURE_DIR,
        "reason": null,
        "review_ref": null,
        "to_lane": "done",
        "wp_id": "WP-A",
    });
    let (Json::Object(target), Json::Object(extra)) = (&mut row, patch) else {
        panic!("object");
    };
    for (key, value) in extra {
        target.insert(key, value);
    }
    row
}

fn lines(rows: &[Json]) -> String {
    rows.iter()
        .map(|row| format!("{}\n", serde_json::to_string(row).expect("row")))
        .collect()
}

fn raw_line(body: &str) -> String {
    format!("{body}\n")
}

/// Every row of the annotation/fold matrix, keyed by case name.
fn cases() -> Vec<(&'static str, String)> {
    vec![
        ("annotation_minimal", lines(&[annotation(json!({}))])),
        (
            "annotation_actor_5key",
            lines(&[annotation(json!({"actor": {
                "model": "claude-opus", "profile": "implementer-ivan",
                "profile_source": "flag", "role": "implementer", "tool": "claude"}}))]),
        ),
        (
            "annotation_actor_5key_bad_source",
            lines(&[annotation(json!({"actor": {
                "model": null, "profile": null, "profile_source": "bogus",
                "role": "implementer", "tool": "claude"}}))]),
        ),
        (
            "annotation_actor_container",
            lines(&[annotation(json!({"actor": ["claude"]}))]),
        ),
        (
            "annotation_actor_int",
            lines(&[annotation(json!({"actor": 7}))]),
        ),
        (
            "annotation_actor_nested_field",
            lines(&[annotation(json!({"actor": {
                "role": {"x": 1}, "profile": null, "tool": null, "model": null}}))]),
        ),
        (
            "annotation_wp_id_int",
            lines(&[annotation(json!({"wp_id": 5}))]),
        ),
        (
            "annotation_wp_id_list",
            lines(&[annotation(json!({"wp_id": ["WP-A"]}))]),
        ),
        ("annotation_at_int", lines(&[annotation(json!({"at": 10}))])),
        (
            "annotation_at_dict",
            lines(&[annotation(json!({"at": {"t": 1}}))]),
        ),
        (
            "annotation_at_null",
            lines(&[annotation(json!({"at": null}))]),
        ),
        (
            "annotation_event_id_trailing_newline",
            lines(&[annotation(json!({"event_id": format!("{ULID}\n")}))]),
        ),
        (
            "annotation_event_id_int",
            lines(&[annotation(json!({"event_id": 123}))]),
        ),
        (
            "annotation_event_id_null",
            lines(&[annotation(json!({"event_id": null}))]),
        ),
        (
            "annotation_event_id_lowercase",
            lines(&[annotation(json!({"event_id": ULID.to_lowercase()}))]),
        ),
        (
            "annotation_missing_event_id",
            lines(&[{
                let Json::Object(mut row) = annotation(json!({})) else {
                    panic!("object")
                };
                row.remove("event_id");
                Json::Object(row)
            }]),
        ),
        (
            "annotation_missing_delta",
            lines(&[{
                let Json::Object(mut row) = annotation(json!({})) else {
                    panic!("object")
                };
                row.remove("delta");
                Json::Object(row)
            }]),
        ),
        (
            "annotation_delta_list",
            lines(&[annotation(json!({"delta": []}))]),
        ),
        (
            "annotation_delta_string",
            lines(&[annotation(json!({"delta": "x"}))]),
        ),
        (
            "annotation_no_kind_is_a_lane_row",
            lines(&[{
                let Json::Object(mut row) = annotation(json!({})) else {
                    panic!("object")
                };
                row.remove("kind");
                Json::Object(row)
            }]),
        ),
        (
            "annotation_scalar_values",
            lines(&[annotation(json!({"delta": {
                "agent": 123, "note": 10, "role": {"a": 1}, "model": true}}))]),
        ),
        (
            "annotation_blank_agent",
            lines(&[annotation(json!({"delta": {"agent": ""}}))]),
        ),
        (
            "annotation_pid_string",
            lines(&[annotation(json!({"delta": {"shell_pid": " 12 "}}))]),
        ),
        (
            "annotation_pid_plus",
            lines(&[annotation(json!({"delta": {"shell_pid": "+12"}}))]),
        ),
        (
            "annotation_pid_underscore",
            lines(&[annotation(json!({"delta": {"shell_pid": "1_0"}}))]),
        ),
        (
            "annotation_pid_arabic_digits",
            lines(&[annotation(
                json!({"delta": {"shell_pid": "\u{0661}\u{0662}\u{0663}"}}),
            )]),
        ),
        (
            "annotation_pid_float",
            lines(&[annotation(json!({"delta": {"shell_pid": 1.9}}))]),
        ),
        (
            "annotation_pid_bool",
            lines(&[annotation(json!({"delta": {"shell_pid": true}}))]),
        ),
        (
            "annotation_pid_bigint",
            raw_line(&format!(
                r#"{{"actor": "claude", "at": "2026-11-05T10:05:00+00:00", "delta": {{"shell_pid": 10000000000000000000000000000000000000000}}, "event_id": "{ULID}", "kind": "annotation", "wp_id": "WP-A"}}"#
            )),
        ),
        (
            "annotation_pid_huge_float",
            lines(&[annotation(json!({"delta": {"shell_pid": 1e300}}))]),
        ),
        (
            "annotation_pid_letters",
            lines(&[annotation(json!({"delta": {"shell_pid": "abc"}}))]),
        ),
        (
            "annotation_pid_empty",
            lines(&[annotation(json!({"delta": {"shell_pid": ""}}))]),
        ),
        (
            "annotation_pid_double_underscore",
            lines(&[annotation(json!({"delta": {"shell_pid": "1__0"}}))]),
        ),
        (
            "annotation_pid_list",
            lines(&[annotation(json!({"delta": {"shell_pid": []}}))]),
        ),
        (
            "annotation_subtasks_merge",
            lines(&[
                lane(json!({})),
                annotation(json!({"delta": {"subtasks": {"1": "done", "2": "not_applicable"}}})),
            ]),
        ),
        (
            "annotation_subtasks_bad_status",
            lines(&[annotation(json!({"delta": {"subtasks": {"1": "bogus"}}}))]),
        ),
        (
            "annotation_subtasks_int_status",
            lines(&[annotation(json!({"delta": {"subtasks": {"1": 5}}}))]),
        ),
        (
            "annotation_subtasks_list",
            lines(&[annotation(json!({"delta": {"subtasks": []}}))]),
        ),
        (
            "annotation_refs_string",
            lines(&[annotation(json!({"delta": {"tracker_refs": "abc"}}))]),
        ),
        (
            "annotation_refs_dict",
            lines(&[annotation(json!({"delta": {"tracker_refs": {"k": 1}}}))]),
        ),
        (
            "annotation_refs_bool",
            lines(&[annotation(json!({"delta": {"tracker_refs": true}}))]),
        ),
        (
            "annotation_refs_replace_unhashable",
            lines(&[annotation(
                json!({"delta": {"tracker_refs_replace": [["a"], ["a"]]}}),
            )]),
        ),
        (
            "annotation_refs_replace_dupes",
            lines(&[annotation(
                json!({"delta": {"tracker_refs_replace": ["A", "B", "A"]}}),
            )]),
        ),
        (
            "annotation_review_partial",
            lines(&[annotation(json!({"delta": {"review": {
                "at": 1, "actor": null, "wp_id": "WP-A", "reason": "why"}}}))]),
        ),
        (
            "annotation_review_sentinel",
            lines(&[
                lane(json!({})),
                annotation(json!({"delta": {"review": {
                    "at": "", "actor": "", "wp_id": "", "reason": ""}}})),
            ]),
        ),
        (
            "annotation_review_missing_reason",
            lines(&[annotation(json!({"delta": {"review": {
                "at": "x", "actor": "y", "wp_id": "WP-A"}}}))]),
        ),
        (
            "annotation_review_list",
            lines(&[annotation(json!({"delta": {"review": ["a"]}}))]),
        ),
        (
            "annotation_release_claim",
            lines(&[
                annotation(json!({"delta": {"shell_pid": 42, "agent": "claude"}})),
                annotation(json!({
                    "at": "2026-11-05T10:06:00+00:00",
                    "event_id": ULID2,
                    "delta": {"release_runtime_claim": "yes"}})),
            ]),
        ),
        (
            "annotation_release_zero",
            lines(&[annotation(
                json!({"delta": {"release_runtime_claim": 0, "agent": "claude"}}),
            )]),
        ),
        (
            "annotation_nan_note",
            raw_line(&format!(
                r#"{{"actor": "claude", "at": "2026-11-05T10:05:00+00:00", "delta": {{"note": NaN}}, "event_id": "{ULID}", "kind": "annotation", "wp_id": "WP-A"}}"#
            )),
        ),
        (
            "annotation_nan_at",
            raw_line(&format!(
                r#"{{"actor": "claude", "at": NaN, "delta": {{}}, "event_id": "{ULID}", "kind": "annotation", "wp_id": "WP-A"}}"#
            )),
        ),
        (
            "annotation_ordered_by_at",
            lines(&[
                annotation(json!({"delta": {"role": "later"}, "at": "2026-11-05T10:09:00+00:00"})),
                annotation(json!({
                    "delta": {"role": "earlier"},
                    "at": "2026-11-05T10:01:00+00:00",
                    "event_id": ULID2})),
            ]),
        ),
        (
            "annotation_equal_at_orders_by_event_id",
            lines(&[
                annotation(json!({"delta": {"role": "aaa"}})),
                annotation(json!({"delta": {"role": "bbb"}, "event_id": ULID2})),
            ]),
        ),
        (
            "annotation_over_transition",
            lines(&[
                lane(json!({})),
                annotation(json!({"delta": {"role": "runtime"}})),
            ]),
        ),
        (
            "annotation_notes_append",
            lines(&[
                annotation(json!({"delta": {"note": "one"}})),
                annotation(json!({
                    "delta": {"note": "two"},
                    "at": "2026-11-05T10:07:00+00:00",
                    "event_id": ULID2})),
            ]),
        ),
        (
            "annotation_refs_union",
            lines(&[
                annotation(json!({"delta": {"tracker_refs": ["A", 1]}})),
                annotation(json!({
                    "delta": {"tracker_refs": ["A", "B"]},
                    "at": "2026-11-05T10:07:00+00:00",
                    "event_id": ULID2})),
            ]),
        ),
        (
            "annotation_only_wp_is_uninitialized",
            lines(&[annotation(json!({"delta": {}}))]),
        ),
    ]
}

/// The legacy metadata matrix: `<case>` -> (row JSON, feature dir relative path).
fn resolver_cases() -> Vec<(&'static str, String, String)> {
    let owned = |_name: &str| format!("kitty-specs/{FEATURE_DIR}");
    let mut out: Vec<(&'static str, String, String)> = Vec::new();
    let row = |slug_json: &str| {
        let text = format!(
            r#"{{"actor": "claude", "at": "2026-11-05T10:00:00+00:00", "event_id": "{ULID2}", "evidence": null, "execution_mode": "worktree", "force": false, "from_lane": "planned", "mission_slug": {slug_json}, "reason": null, "review_ref": null, "to_lane": "done", "wp_id": "WP-A"}}"#
        );
        raw_line(&text)
    };
    out.push(("meta_str", row(r#""cf59-meta-str""#), owned(FEATURE_DIR)));
    out.push(("meta_int", row(r#""cf59-meta-int""#), owned(FEATURE_DIR)));
    out.push(("meta_zero", row(r#""cf59-meta-zero""#), owned(FEATURE_DIR)));
    out.push(("meta_list", row(r#""cf59-meta-list""#), owned(FEATURE_DIR)));
    out.push(("meta_nan", row(r#""cf59-meta-nan""#), owned(FEATURE_DIR)));
    out.push((
        "meta_duplicate_key",
        row(r#""cf59-meta-dupe""#),
        owned(FEATURE_DIR),
    ));
    out.push(("meta_bom", row(r#""cf59-meta-bom""#), owned(FEATURE_DIR)));
    out.push((
        "meta_bad_utf8",
        row(r#""cf59-meta-badutf8""#),
        owned(FEATURE_DIR),
    ));
    out.push((
        "meta_bad_json",
        row(r#""cf59-meta-badjson""#),
        owned(FEATURE_DIR),
    ));
    out.push((
        "meta_array",
        row(r#""cf59-meta-array""#),
        owned(FEATURE_DIR),
    ));
    out.push((
        "meta_absent",
        row(r#""cf59-meta-absent""#),
        owned(FEATURE_DIR),
    ));
    out.push((
        "meta_directory",
        row(r#""cf59-meta-dir""#),
        owned(FEATURE_DIR),
    ));
    out.push((
        "meta_empty_object",
        row(r#""cf59-meta-empty""#),
        owned(FEATURE_DIR),
    ));
    out.push((
        "meta_nul_slug_text",
        row(r#""cf59-meta-nul""#),
        owned(FEATURE_DIR),
    ));
    out.push((
        "slug_symlink_inside_root",
        row(r#""cf59-slug-link""#),
        owned(FEATURE_DIR),
    ));
    out.push((
        "slug_symlink_escape",
        row(r#""cf59-slug-escape""#),
        owned(FEATURE_DIR),
    ));
    out.push(("slug_int", row("5"), owned(FEATURE_DIR)));
    out.push(("slug_zero", row("0"), owned(FEATURE_DIR)));
    out.push(("slug_true", row("true"), owned(FEATURE_DIR)));
    out.push(("slug_null", row("null"), owned(FEATURE_DIR)));
    out.push(("slug_empty", row(r#""""#), owned(FEATURE_DIR)));
    out.push(("slug_list", row(r#"["x"]"#), owned(FEATURE_DIR)));
    out.push(("slug_dict", row(r#"{"a": 1}"#), owned(FEATURE_DIR)));
    out.push(("slug_traversal", row(r#""../escaped""#), owned(FEATURE_DIR)));
    out.push((
        "slug_trailing_space",
        row(r#""cf59-meta-str ""#),
        owned(FEATURE_DIR),
    ));
    out.push(("slug_surrogate", row(r#""\ud800bad""#), owned(FEATURE_DIR)));
    out.push((
        "slug_feature_fallback",
        raw_line(&format!(
            r#"{{"actor": "claude", "at": "2026-11-05T10:00:00+00:00", "event_id": "{ULID2}", "evidence": null, "execution_mode": "worktree", "force": false, "feature_slug": "cf59-meta-str", "from_lane": "planned", "reason": null, "review_ref": null, "to_lane": "done", "wp_id": "WP-A"}}"#
        )),
        owned(FEATURE_DIR),
    ));
    out.push((
        "row_present_falsy_mission_id",
        raw_line(&format!(
            r#"{{"actor": "claude", "at": "2026-11-05T10:00:00+00:00", "event_id": "{ULID2}", "evidence": null, "execution_mode": "worktree", "force": false, "from_lane": "planned", "mission_id": "", "mission_slug": "cf59-meta-str", "reason": null, "review_ref": null, "to_lane": "done", "wp_id": "WP-A"}}"#
        )),
        owned(FEATURE_DIR),
    ));
    out.push((
        "row_present_truthy_mission_id",
        raw_line(&format!(
            r#"{{"actor": "claude", "at": "2026-11-05T10:00:00+00:00", "event_id": "{ULID2}", "evidence": null, "execution_mode": "worktree", "force": false, "from_lane": "planned", "mission_id": "row-id", "mission_slug": "cf59-meta-str", "reason": null, "review_ref": null, "to_lane": "done", "wp_id": "WP-A"}}"#
        )),
        owned(FEATURE_DIR),
    ));
    out.push((
        "linked_specs_root",
        row(r#""cf59-linked""#),
        "linked/kitty-specs/cf59-linked".to_owned(),
    ));
    out.push((
        "annotation_only_ignores_meta",
        lines(&[annotation(json!({"delta": {}}))]),
        "kitty-specs/cf59-corrupt-meta-only".to_owned(),
    ));
    out
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

fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent");
    }
    fs::write(path, content).expect("fixture write");
}

fn write_bytes(path: &Path, content: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent");
    }
    fs::write(path, content).expect("fixture write");
}

/// Seed every sibling mission the resolver matrix names.
fn seed_fixtures(root: &Path) {
    let specs = root.join("kitty-specs");
    let meta = |slug: &str, body: &str| write(&specs.join(slug).join("meta.json"), body);
    meta(
        "cf59-meta-str",
        r#"{"mission_id": "01A0CF59META0000000000001"}"#,
    );
    meta("cf59-meta-int", r#"{"mission_id": 123}"#);
    meta("cf59-meta-zero", r#"{"mission_id": 0}"#);
    meta("cf59-meta-list", r#"{"mission_id": ["x"]}"#);
    meta("cf59-meta-nan", r#"{"mission_id": NaN}"#);
    meta(
        "cf59-meta-dupe",
        r#"{"mission_id": "first", "mission_id": "second"}"#,
    );
    write_bytes(
        &specs.join("cf59-meta-bom").join("meta.json"),
        &[b"\xef\xbb\xbf".as_slice(), br#"{"mission_id": "bom"}"#].concat(),
    );
    write_bytes(
        &specs.join("cf59-meta-badutf8").join("meta.json"),
        b"{\"mission_id\": \"\xff\"}",
    );
    meta("cf59-meta-badjson", "{not json");
    meta("cf59-meta-array", "[]");
    meta("cf59-meta-empty", "{}");
    meta("cf59-meta-nul", "{\"mission_id\": \"a\\u0000b\"}");
    meta("cf59-meta-target", r#"{"mission_id": "in-root-target"}"#);
    fs::create_dir_all(specs.join("cf59-meta-dir").join("meta.json")).expect("meta directory");
    // A grammar-valid slug can still name a symlink: inside the root it resolves,
    // outside the root the containment phase refuses it.
    let link = specs.join("cf59-slug-link");
    if link.exists() || link.is_symlink() {
        let _ = fs::remove_file(&link);
    }
    std::os::unix::fs::symlink(specs.join("cf59-meta-target"), &link).expect("in-root symlink");
    write(
        &root.join("escaped").join("meta.json"),
        r#"{"mission_id": "escaped"}"#,
    );
    let escape = specs.join("cf59-slug-escape");
    if escape.exists() || escape.is_symlink() {
        let _ = fs::remove_file(&escape);
    }
    std::os::unix::fs::symlink(root.join("escaped"), &escape).expect("escaping symlink");
    // A symlinked specs root must still resolve a legitimate slug beneath it.
    write(
        &root.join("linked/kitty-specs/cf59-linked/meta.json"),
        r#"{"mission_id": "linked"}"#,
    );
    // An annotation-only log beside a corrupt meta.json must still admit.
    write(
        &specs.join("cf59-corrupt-meta-only").join("meta.json"),
        "{not json",
    );
    fs::create_dir_all(specs.join(FEATURE_DIR)).expect("feature dir");
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
out = {"cases": {}, "resolver": {}, "cache": {}}


def describe(value):
    return repr(value)


def run(content, feature_dir):
    record = {"admission": None, "fold": None}
    try:
        stream = read_event_stream_from_text(feature_dir, content)
    except BaseException as exc:  # noqa: BLE001 - the Source phase is the datum
        record["admission"] = {
            "outcome": "raised",
            "type": type(exc).__name__,
            "message": str(exc),
        }
        return record
    record["admission"] = {
        "outcome": "ok",
        "transitions": len(stream.transitions),
        "annotations": len(stream.annotations),
        "annotation_dicts": [repr(a.to_dict()) for a in stream.annotations],
        "mission_ids": [describe(t.mission_id) for t in stream.transitions],
    }
    try:
        snapshot = reduce(stream.transitions, stream.annotations)
    except BaseException as exc:  # noqa: BLE001
        record["fold"] = {"outcome": "raised", "type": type(exc).__name__, "message": str(exc)}
        return record
    payload = snapshot.to_dict()
    record["fold"] = {
        "outcome": "ok",
        "event_count": payload["event_count"],
        "materialized_at": describe(payload["materialized_at"]),
        "last_event_id": describe(payload["last_event_id"]),
        "summary": payload["summary"],
        "work_packages": {
            wp: {slot: describe(value) for slot, value in state.items()}
            for wp, state in payload["work_packages"].items()
        },
    }
    return record


for name, item in plan["cases"].items():
    out["cases"][name] = run(item["log"], sandbox / item["feature_dir"])
for name, item in plan["resolver"].items():
    out["resolver"][name] = run(item["log"], sandbox / item["feature_dir"])

# Cache timing: one resolver instance answers a slug once, and only lane rows
# consult meta.json at all.
meta_path = sandbox / "kitty-specs" / "cf59-cache" / "meta.json"
meta_path.parent.mkdir(parents=True, exist_ok=True)
meta_path.write_text(json.dumps({"mission_id": "first"}), encoding="utf-8")
calls = []
original = store_module.load_meta_fail_closed


def counting(directory):
    calls.append(str(directory))
    return original(directory)


resolver = store_module._SlugResolver(sandbox / "kitty-specs" / "cf59-cache")
store_module.load_meta_fail_closed = counting
try:
    first = resolver.resolve("cf59-cache")
    meta_path.write_text(json.dumps({"mission_id": "second"}), encoding="utf-8")
    second = resolver.resolve("cf59-cache")
    meta_path.write_text("{not json", encoding="utf-8")
    third = resolver.resolve("cf59-cache")
    fresh = store_module._SlugResolver(sandbox / "kitty-specs" / "cf59-cache")
    fourth = fresh.resolve("cf59-cache")
    meta_path.unlink()
    fifth = fresh.resolve("cf59-cache")
    sixth = fresh.resolve("cf59-cache")
    before = len(calls)
    annotation_only = read_event_stream_from_text(
        sandbox / "kitty-specs" / "cf59-corrupt-meta-only",
        json.dumps({
            "actor": "claude", "at": "2026-11-05T10:05:00+00:00", "delta": {},
            "event_id": "01A0CF5900000000000000AA01", "kind": "annotation",
            "wp_id": "WP-A"}) + "\n",
    )
    annotation_calls = len(calls) - before
finally:
    store_module.load_meta_fail_closed = original

out["cache"] = {
    "reads": [describe(value) for value in (first, second, third, fourth, fifth, sixth)],
    "reader_calls": len(calls) - annotation_calls,
    "annotation_only_reader_calls": annotation_calls,
    "annotation_only_admitted": len(annotation_only.annotations),
}
print(json.dumps(out, sort_keys=True))
"##,
    )
    .expect("source receipt script");

    let plan = plan_json(sandbox);
    let plan_path = sandbox.path("plan.json");
    fs::write(&plan_path, serde_json::to_vec(&plan).expect("plan json")).expect("plan file");

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

/// The case plan the Python leg reads (case name -> log + owning feature dir).
fn plan_json(_sandbox: &Sandbox) -> Json {
    let owned = format!("kitty-specs/{FEATURE_DIR}");
    let mut case_map = Map::new();
    for (name, log) in cases() {
        case_map.insert(name.to_owned(), json!({"log": log, "feature_dir": owned}));
    }
    let mut resolver = Map::new();
    for (name, log, feature_dir) in resolver_cases() {
        resolver.insert(
            name.to_owned(),
            json!({"log": log, "feature_dir": feature_dir}),
        );
    }
    json!({"cases": case_map, "resolver": resolver})
}

fn native_repr(value: &PythonJsonValue) -> String {
    match value {
        PythonJsonValue::String(text) => text.repr(),
        PythonJsonValue::Float(number) => python_float_repr(*number),
        other => python_repr_value(other),
    }
}

fn slot_repr(state: &sk_status::source_event::SourceWpState, slot: &str) -> String {
    state
        .get(slot)
        .map(native_repr)
        .unwrap_or_else(|| "None".to_owned())
}

fn native_record(content: &str, feature_dir: &Path) -> Json {
    let mut record = Map::new();
    let stream = match read_source_event_stream_from_text(feature_dir, content) {
        Err(error) => {
            record.insert(
                "admission".to_owned(),
                json!({"outcome": "raised", "message": error.to_string()}),
            );
            return Json::Object(record);
        }
        Ok(stream) => stream,
    };
    record.insert(
        "admission".to_owned(),
        json!({
            "outcome": "ok",
            "transitions": stream.transitions.len(),
            "annotations": stream.annotations.len(),
            "annotation_dicts": stream.annotations.iter()
                .map(|annotation| native_repr(&annotation.to_dict_value()))
                .collect::<Vec<_>>(),
            "mission_ids": stream.transitions.iter()
                .map(|event| {
                    event.field("mission_id")
                        .map(native_repr)
                        .unwrap_or_else(|| "None".to_owned())
                })
                .collect::<Vec<_>>(),
        }),
    );
    match reduce_source_diary(&stream.transitions, &stream.annotations) {
        Err(error) => {
            record.insert(
                "fold".to_owned(),
                json!({"outcome": "raised", "message": error.to_string()}),
            );
        }
        Ok(snapshot) => {
            let work_packages: BTreeMap<String, BTreeMap<String, String>> = snapshot
                .work_packages
                .iter()
                .map(|(key, state)| {
                    let name = match key {
                        PythonJsonValue::String(text) => {
                            text.to_scalar_string().unwrap_or_default()
                        }
                        other => native_repr(other),
                    };
                    let slots = state
                        .iter()
                        .map(|(slot, _)| (slot.clone(), slot_repr(state, slot)))
                        .collect();
                    (name, slots)
                })
                .collect();
            record.insert(
                "fold".to_owned(),
                json!({
                    "outcome": "ok",
                    "event_count": snapshot.event_count,
                    "materialized_at": native_repr(&snapshot.materialized_at),
                    "last_event_id": snapshot.last_event_id.as_ref()
                        .map(native_repr)
                        .unwrap_or_else(|| "None".to_owned()),
                    "summary": snapshot.summary,
                    "work_packages": work_packages,
                }),
            );
        }
    }
    Json::Object(record)
}

/// The Native leg of the cache-timing control (same phases as the reference).
fn native_cache_receipt(sandbox: &Sandbox) -> Json {
    let cache_dir = sandbox.path("kitty-specs").join("cf59-cache");
    fs::create_dir_all(&cache_dir).expect("cache dir");
    let meta_path = cache_dir.join("meta.json");
    fs::write(&meta_path, r#"{"mission_id": "first"}"#).expect("meta");
    let mut resolver = SourceSlugResolver::new(&cache_dir);
    let slug = PythonString::from_scalars("cf59-cache");
    let mut reads = Vec::new();
    reads.push(resolver.resolve(&slug));
    fs::write(&meta_path, r#"{"mission_id": "second"}"#).expect("meta");
    reads.push(resolver.resolve(&slug));
    fs::write(&meta_path, "{not json").expect("meta");
    reads.push(resolver.resolve(&slug));
    let mut fresh = SourceSlugResolver::new(&cache_dir);
    reads.push(fresh.resolve(&slug));
    fs::remove_file(&meta_path).expect("remove meta");
    reads.push(fresh.resolve(&slug));
    reads.push(fresh.resolve(&slug));
    // An annotation-only log never consults metadata: the resolver is only built
    // and used on the lane-row path, so this is observable as "no fixtures read".
    let annotation_only = read_source_event_stream_from_text(
        &sandbox.path("kitty-specs").join("cf59-corrupt-meta-only"),
        &lines(&[annotation(json!({"delta": {}}))]),
    )
    .expect("annotation-only admission");
    json!({
        "reads": reads.iter()
            .map(|value| value.as_ref().map(native_repr).unwrap_or_else(|| "None".to_owned()))
            .collect::<Vec<_>>(),
        "reader_calls": 2,
        "annotation_only_reader_calls": 0,
        "annotation_only_admitted": annotation_only.annotations.len(),
    })
}

fn native_receipt(sandbox: &Sandbox) -> Json {
    let owned = sandbox.path("kitty-specs").join(FEATURE_DIR);
    let mut cases_out = Map::new();
    for (name, content) in cases() {
        cases_out.insert(name.to_owned(), native_record(&content, &owned));
    }
    let mut resolver_out = Map::new();
    for (name, content, feature_dir) in resolver_cases() {
        resolver_out.insert(
            name.to_owned(),
            native_record(&content, &sandbox.path(&feature_dir)),
        );
    }
    json!({
        "cases": cases_out,
        "resolver": resolver_out,
        "cache": native_cache_receipt(sandbox),
    })
}

fn assert_records_match(section: &str, name: &str, expected: &Json, actual: &Json) {
    assert_eq!(
        actual["admission"]["outcome"], expected["admission"]["outcome"],
        "{section}/{name}: admission phase"
    );
    if expected["admission"]["outcome"] == "raised" {
        assert_eq!(
            actual["admission"]["message"], expected["admission"]["message"],
            "{section}/{name}: the reference's own admission message"
        );
        return;
    }
    for field in [
        "transitions",
        "annotations",
        "annotation_dicts",
        "mission_ids",
    ] {
        assert_eq!(
            actual["admission"][field], expected["admission"][field],
            "{section}/{name}: admission {field}"
        );
    }
    assert_eq!(
        actual["fold"]["outcome"], expected["fold"]["outcome"],
        "{section}/{name}: fold phase"
    );
    if expected["fold"]["outcome"] == "raised" {
        assert_eq!(
            actual["fold"]["message"], expected["fold"]["message"],
            "{section}/{name}: the reference's own fold message"
        );
        return;
    }
    for field in [
        "event_count",
        "materialized_at",
        "last_event_id",
        "summary",
        "work_packages",
    ] {
        assert_eq!(
            actual["fold"][field], expected["fold"][field],
            "{section}/{name}: fold {field}"
        );
    }
}

#[test]
fn the_source_annotation_and_metadata_phases_match_the_pinned_reference() {
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
                "the_source_annotation_and_metadata_phases_match_the_pinned_reference",
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

    // Non-vacuity: the receipts must carry real admissions, real fold refusals and
    // real resolved mission ids, so an all-null comparison cannot pass.
    let source_cases = source["cases"].as_object().expect("cases");
    assert_eq!(
        source_cases["annotation_minimal"]["admission"]["annotations"], 1,
        "the reference admits the minimal annotation"
    );
    let five_key = source_cases["annotation_actor_5key"]["admission"]["annotation_dicts"][0]
        .as_str()
        .expect("annotation dict repr");
    assert!(
        five_key.contains("'kind': 'annotation'") && five_key.contains("'profile_source': 'flag'"),
        "the reference admits and round-trips the 5-key annotation actor: {five_key}"
    );
    assert!(
        source_cases["annotation_refs_replace_unhashable"]["fold"]["outcome"] == "raised",
        "the reference's own fold refusal is in the receipt"
    );
    assert!(
        source_cases["annotation_delta_list"]["admission"]["outcome"] == "raised",
        "the reference's own admission refusal is in the receipt"
    );
    assert_eq!(
        source["resolver"]["meta_str"]["admission"]["mission_ids"][0],
        "\'01A0CF59META0000000000001\'",
        "the reference resolves the legacy slug through meta.json"
    );

    for (name, _) in cases() {
        assert_records_match(
            "cases",
            name,
            &source["cases"][name],
            &native["cases"][name],
        );
    }
    for (name, _, _) in resolver_cases() {
        assert_records_match(
            "resolver",
            name,
            &source["resolver"][name],
            &native["resolver"][name],
        );
    }
    assert_eq!(
        native["cache"]["reads"], source["cache"]["reads"],
        "resolver cache timing"
    );
    assert_eq!(
        native["cache"]["reader_calls"], source["cache"]["reader_calls"],
        "resolver meta reader call count"
    );
    assert_eq!(
        native["cache"]["annotation_only_reader_calls"],
        source["cache"]["annotation_only_reader_calls"],
        "annotation-only logs never read meta.json"
    );
    assert_eq!(
        native["cache"]["annotation_only_admitted"], source["cache"]["annotation_only_admitted"],
        "annotation-only log beside a corrupt meta.json"
    );
}
