//! CF22 — the `doctor ops --close-stale` command contract.
//!
//! These tests exercise the **public runner** the CLI adapter calls
//! (`sk_invocation::ops_sweep::close_stale_ops`) and pin the command surface the
//! adapter owns: which default threshold applies, the exact JSON document keys
//! Source's CLI test asserts (`set(payload) == {"open_ops", "swept",
//! "skipped_fresh", "threshold_hours", "closure_spine"}`), the per-entry key
//! set, and the exit rule.
//!
//! They deliberately do **not** call
//! `compat::doctor::ops_sweep::run_sweep`: that arm is pending the parent's
//! dispatch wiring, so this is a public-runner contract test, not root CLI
//! coverage. Non-git fixtures are enough for the JSON/exit contract; the real
//! git close/commit path is covered by
//! `sk-invocation/tests/cf22_ops_stale_sweep.rs`.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value;
use sk_invocation::ops_sweep::{DEFAULT_SWEEP_THRESHOLD_HOURS, close_stale_ops};

const STALE_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

fn write_record(root: &Path, invocation_id: &str, started_at: &str) {
    let dir = root.join("kitty-ops");
    fs::create_dir_all(&dir).expect("kitty-ops");
    fs::write(
        dir.join(format!("{invocation_id}.jsonl")),
        format!(
            "{{\"event\": \"started\", \"invocation_id\": \"{invocation_id}\", \
             \"profile_id\": \"implementer-ivan\", \"action\": \"implement\", \
             \"started_at\": \"{started_at}\"}}\n"
        ),
    )
    .expect("write the record");
}

fn keys(value: &Value) -> BTreeSet<String> {
    value
        .as_object()
        .expect("an object")
        .keys()
        .cloned()
        .collect()
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

#[test]
fn the_default_threshold_is_sources_twenty_four_hours() {
    assert_eq!(DEFAULT_SWEEP_THRESHOLD_HOURS, 24.0);
    let temp = tempfile::tempdir().expect("tempdir");
    let now = sk_kernel::clock::parse_iso("2026-04-22T12:00:00+00:00").expect("instant");
    write_record(temp.path(), STALE_ID, "2026-04-22T11:00:00+00:00");
    let report = close_stale_ops(temp.path(), DEFAULT_SWEEP_THRESHOLD_HOURS, now).expect("sweep");
    assert_eq!(report.threshold_hours, 24.0);
    assert_eq!(report.skipped_fresh, 1, "one hour old is fresh");
    assert_eq!(report.exit_code(), 1);
}

#[test]
fn the_json_document_is_sources_shape_and_reports_a_real_close_without_git() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    let now = sk_kernel::clock::parse_iso("2026-04-22T12:00:00+00:00").expect("instant");
    write_record(root, STALE_ID, "2026-04-21T10:00:00+00:00");
    write_record(
        root,
        "01BX5ZZKBKACTAV9WEVGEMMVRZ",
        "2026-04-22T11:00:00+00:00",
    );

    let report = close_stale_ops(root, DEFAULT_SWEEP_THRESHOLD_HOURS, now).expect("sweep");
    assert_eq!(report.swept, 1);
    assert_eq!(report.skipped_fresh, 1);
    assert_eq!(report.exit_code(), 1);

    let text = report.json_text();
    let payload: Value = serde_json::from_str(&text).expect("valid JSON");
    assert_eq!(
        keys(&payload),
        set(&[
            "open_ops",
            "swept",
            "skipped_fresh",
            "threshold_hours",
            "closure_spine"
        ])
    );
    assert_eq!(payload["swept"], Value::from(1));
    assert_eq!(payload["skipped_fresh"], Value::from(1));
    assert_eq!(payload["threshold_hours"], Value::from(24.0));
    assert_eq!(
        payload["closure_spine"],
        Value::from("kitty-ops/op-closures.jsonl")
    );
    assert!(text.contains("\"threshold_hours\": 24.0"), "{text}");

    let open_ops = payload["open_ops"].as_array().expect("an array");
    assert_eq!(open_ops.len(), 2);
    // The stale entry: Source's five base keys plus `commit` (a close ran).
    assert_eq!(
        keys(&open_ops[0]),
        set(&[
            "invocation_id",
            "profile_id",
            "started_at",
            "age_hours",
            "action_taken",
            "commit"
        ])
    );
    assert_eq!(open_ops[0]["action_taken"], Value::from("closed_abandoned"));
    assert_eq!(open_ops[0]["age_hours"], Value::from(26.0));
    // A non-git checkout cannot commit: reported, never claimed.
    assert_eq!(
        open_ops[0]["commit"],
        Value::from("not_committed:not a git worktree")
    );
    // The fresh entry: no `commit`, no `error`, no `parse_warning`.
    assert_eq!(
        keys(&open_ops[1]),
        set(&[
            "invocation_id",
            "profile_id",
            "started_at",
            "age_hours",
            "action_taken"
        ])
    );
    assert_eq!(open_ops[1]["action_taken"], Value::from("none"));
    assert_eq!(open_ops[1]["age_hours"], Value::from(1.0));
}

#[test]
fn the_human_surface_carries_the_counts_and_the_fresh_remedy() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    let now = sk_kernel::clock::parse_iso("2026-04-22T12:00:00+00:00").expect("instant");
    write_record(root, STALE_ID, "2026-04-21T10:00:00+00:00");
    write_record(
        root,
        "01BX5ZZKBKACTAV9WEVGEMMVRZ",
        "2026-04-22T11:00:00+00:00",
    );
    let report = close_stale_ops(root, 24.0, now).expect("sweep");
    let lines = report.render_lines();
    assert!(
        lines.contains(
            &"Ops sweep — threshold 24.0h: 1 closed as abandoned, 1 fresh (skipped)".to_owned()
        ),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.contains("Started At")),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(|line| line
            == "Fresh open Ops remain — close them with spec-kitty profile-invocation complete, \
                or re-run with --threshold 0."),
        "{lines:?}"
    );
}

#[test]
fn a_whole_sweep_refusal_names_its_operator_code() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    // A `kitty-ops/op-closures.jsonl` that is a directory cannot be read as a
    // spine: the sweep refuses instead of classifying every Op as open.
    fs::create_dir_all(root.join("kitty-ops/op-closures.jsonl")).expect("spine directory");
    let now = sk_kernel::clock::parse_iso("2026-04-22T12:00:00+00:00").expect("instant");
    let error = close_stale_ops(root, 24.0, now).expect_err("the spine is unreadable");
    assert_eq!(error.code(), "OP_CLOSURE_SPINE_UNREADABLE");
}
