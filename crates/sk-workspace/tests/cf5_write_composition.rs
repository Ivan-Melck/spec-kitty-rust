//! CF5: composed write-observer ledgers through the public doors.
//!
//! These are Native composition smoke tests, not Source-parity evidence: they
//! exercise the public surface a real caller uses and prove the four behaviours
//! the write-observer composition owes its callers —
//!
//! 1. a real status write notifies the installed observer with the exact bytes
//!    the durable writer published, *after* the publication;
//! 2. nested observers keep the reference's outer-first ordering;
//! 3. the mission-number bake captures the `meta.json` bytes it really wrote and
//!    undoes them only while the file still holds them (compare-and-swap);
//! 4. a failed-run restore puts back only bytes this run wrote, and leaves every
//!    other writer's content — changed, created or deleted — exactly as it is.
//!
//! The unit tests beside `write_ledger.rs` cover the same contracts at module
//! level (including typed observer failures and the no-listener state). Deep
//! equivalence campaigns (mid-write races, Windows read-only replacement, error
//! wire formats) remain open and are recorded as debt in the crate report.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use sk_kernel::source_writer::observe_writes;
use sk_status::AppendOptions;
use sk_workspace::planning::write_ledger::{
    BakeUndoOutcome, ObservationSource, append_status_rows_observed, bake_mission_number,
    notify_published, restore_mission_write_scope_locked, snapshot_mission_write_scope,
    undo_meta_bake,
};
use tempfile::{TempDir, tempdir};

/// One `(path, notified bytes, bytes already on disk)` observation.
type Observation = (PathBuf, Vec<u8>, Vec<u8>);

fn init_git(root: &Path) {
    let status = std::process::Command::new("git")
        .args(["init", "--quiet", "--initial-branch=main"])
        .current_dir(root)
        .status()
        .expect("git must be available for the mission-lock fixture");
    assert!(status.success(), "git init failed: {status:?}");
}

fn mission_fixture() -> (TempDir, PathBuf) {
    let root = tempdir().expect("temp root");
    init_git(root.path());
    let mission_dir = root.path().join("kitty-specs").join("001-demo");
    fs::create_dir_all(&mission_dir).expect("mission dir");
    fs::write(
        mission_dir.join("meta.json"),
        b"{\n  \"mission_slug\": \"001-demo\"\n}\n",
    )
    .expect("meta.json");
    (root, mission_dir)
}

#[test]
fn a_real_status_append_notifies_the_exact_published_bytes_after_publication() {
    let dir = tempdir().expect("temp dir");
    let log = dir.path().join("status.events.jsonl");
    let seen: Arc<Mutex<Vec<Observation>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let observer_log = log.clone();
    let token = observe_writes(move |path: &Path, bytes: &[u8]| {
        let on_disk = fs::read(&observer_log).unwrap_or_default();
        sink.lock()
            .expect("sink")
            .push((path.to_path_buf(), bytes.to_vec(), on_disk));
        Ok(())
    });

    let rows = vec![json!({"event_id": "01HXYZ", "wp_id": "WP01", "to_lane": "claimed"})];
    let (outcome, notification) =
        append_status_rows_observed(&log, &rows, &AppendOptions::default()).expect("append");
    sk_kernel::stop_observing_writes(&token).expect("stop observer");

    let disk = fs::read(&log).expect("log");
    assert_eq!(outcome.rows_appended, 1);
    let notification = notification.expect("a notified append");
    assert!(notification.notified);
    assert_eq!(notification.bytes, disk.len());
    assert_eq!(notification.source, ObservationSource::ReadbackVerified);

    let seen = seen.lock().expect("seen");
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, log);
    assert_eq!(seen[0].1, disk, "payload equals the written bytes");
    assert_eq!(seen[0].2, disk, "the file already held them when notified");
}

#[test]
fn nested_observers_stay_outer_first_across_the_public_door() {
    let order: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let outer_order = Arc::clone(&order);
    let outer = observe_writes(move |_path: &Path, _bytes: &[u8]| {
        outer_order.lock().expect("order").push("outer");
        Ok(())
    });
    let inner_order = Arc::clone(&order);
    let inner = observe_writes(move |_path: &Path, _bytes: &[u8]| {
        inner_order.lock().expect("order").push("inner");
        Ok(())
    });
    let dir = tempdir().expect("temp dir");
    assert!(notify_published(&dir.path().join("nested.txt"), b"x").expect("notify"));
    sk_kernel::stop_observing_writes(&inner).expect("stop inner");
    sk_kernel::stop_observing_writes(&outer).expect("stop outer");
    assert_eq!(*order.lock().expect("order"), vec!["outer", "inner"]);
}

#[test]
fn the_number_bake_captures_real_bytes_and_undo_is_a_compare_and_swap() {
    let (_root, mission_dir) = mission_fixture();
    let meta_path = mission_dir.join("meta.json");
    let original = fs::read(&meta_path).expect("original");

    let capture = bake_mission_number(&mission_dir, 12).expect("bake");
    let disk = fs::read(&meta_path).expect("baked");
    assert_eq!(capture.before.as_deref(), Some(&original[..]));
    assert_eq!(capture.written, disk);
    let value: Value = serde_json::from_slice(&disk).expect("JSON");
    assert_eq!(value.get("mission_number"), Some(&json!(12)));

    assert_eq!(
        undo_meta_bake(&capture).expect("undo"),
        BakeUndoOutcome::Restored
    );
    assert_eq!(fs::read(&meta_path).expect("restored"), original);

    // A concurrent field is never erased by the undo.
    let capture = bake_mission_number(&mission_dir, 13).expect("bake again");
    let mut value: Value = serde_json::from_slice(&capture.written).expect("JSON");
    value["status"] = json!("foreign");
    let foreign = serde_json::to_vec(&value).expect("render");
    fs::write(&meta_path, &foreign).expect("foreign write");
    assert_eq!(
        undo_meta_bake(&capture).expect("undo"),
        BakeUndoOutcome::ChangedByAnotherWriter
    );
    assert_eq!(fs::read(&meta_path).expect("kept"), foreign);
}

#[test]
fn the_finalize_restore_reverts_owned_bytes_and_preserves_foreign_content() {
    let (root, mission_dir) = mission_fixture();
    let owned = mission_dir.join("tasks.md");
    let created = mission_dir.join("issue-matrix.md");
    let foreign = mission_dir.join("foreign.md");
    fs::write(&owned, b"before\n").expect("seed tasks");

    let before = snapshot_mission_write_scope(&mission_dir).expect("snapshot");
    let session = sk_workspace::planning::write_ledger::begin_write_ledger();
    sk_kernel::atomic_write(&owned, b"after\n").expect("write tasks");
    notify_published(&owned, b"after\n").expect("notify tasks");
    sk_kernel::atomic_write(&created, b"created\n").expect("write created");
    notify_published(&created, b"created\n").expect("notify created");
    sk_kernel::atomic_write(&foreign, b"foreign\n").expect("foreign write");
    let ledger = Arc::clone(session.ledger());
    session.finish();

    let restore = restore_mission_write_scope_locked(
        &before,
        &mission_dir,
        root.path(),
        Some(&ledger),
        &BTreeSet::new(),
        None,
    )
    .expect("restore");

    assert_eq!(fs::read(&owned).expect("restored"), b"before\n");
    assert!(!created.exists(), "this run's own creation is removed");
    assert_eq!(fs::read(&foreign).expect("kept"), b"foreign\n");
    assert!(restore.kept.contains(&foreign));
    assert!(restore.skipped.is_empty(), "nothing skipped: {restore:?}");
}
