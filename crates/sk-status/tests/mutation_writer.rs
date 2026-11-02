//! Writer-substrate tests: durable batch replacement, readback, refusal and
//! recovery, against the pinned oracle fixtures.

mod mutation_support;

use std::fs;
use std::io;

use serde_json::{Value, json};
use sk_status::mutation::{
    AppendOptions, Durability, MutationError, append_rows_atomic, capture_log_snapshot,
    materialize_snapshot_file, read_log_rows, restore_log_snapshot, verify_event_rows,
};
use sk_status::mutation_log::append_rows_with_commit;
use sk_status::snapshot::{EVENTS_FILENAME, SNAPSHOT_FILENAME};

use mutation_support::{copy_fixture_input, expected, read_json, read_text, temp_mission};

#[test]
fn status_append_and_materialize_match_the_pinned_oracle() {
    let mission = copy_fixture_input("status_append");
    let log_path = mission.feature_dir.join(EVENTS_FILENAME);
    let rows = read_json(&mutation_support::fixture("status_append").join("rows_to_append.json"));
    let rows: Vec<Value> = rows.as_array().expect("rows array").clone();

    let before = read_text(&log_path);
    let outcome = append_rows_atomic(&log_path, &rows, &AppendOptions::default()).expect("append");
    assert!(outcome.existed_before);
    assert_eq!(outcome.rows_appended, 2);
    assert_eq!(outcome.durability, Durability::Confirmed);

    let expected_log = read_text(&expected("status_append", EVENTS_FILENAME));
    assert_eq!(
        read_text(&log_path),
        expected_log,
        "appended log must be byte-identical to the pinned writer"
    );
    // The torn tail was repaired without losing any existing byte.
    assert!(expected_log.starts_with(&before));

    let materialize =
        materialize_snapshot_file(&mission.feature_dir).expect("materialize status.json");
    assert!(materialize.changed);
    assert_eq!(
        read_text(&mission.feature_dir.join(SNAPSHOT_FILENAME)),
        read_text(&expected("status_append", SNAPSHOT_FILENAME))
    );

    // Second materialization is byte-identical and reports no change.
    let again = materialize_snapshot_file(&mission.feature_dir).expect("materialize again");
    assert!(!again.changed, "materialization must be no-op stable");
}

#[test]
fn a_malformed_stream_is_refused_before_any_replacement() {
    let mission = temp_mission();
    let log_path = mission.feature_dir.join(EVENTS_FILENAME);
    let original = "{\"event_id\": \"01K9M0R3WJ8V6Q1Y4T7B2N5C0A\"}\nnot json\n";
    fs::write(&log_path, original).unwrap();

    let error = append_rows_atomic(
        &log_path,
        &[json!({"event_id": "x"})],
        &AppendOptions::default(),
    )
    .expect_err("a corrupt stream must be refused");
    match error {
        MutationError::MalformedStream { line, .. } => assert_eq!(line, 2),
        other => panic!("unexpected error: {other}"),
    }
    assert_eq!(read_text(&log_path), original);
    assert!(
        !error.may_have_mutated(),
        "a refused malformed stream must not have mutated anything"
    );
    // No temporary files were left behind.
    let leftovers: Vec<String> = fs::read_dir(&mission.feature_dir)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains("sk-kernel-atomic"))
        .collect();
    assert!(leftovers.is_empty(), "leftover temporaries: {leftovers:?}");
}

#[cfg(unix)]
#[test]
fn a_symlinked_log_path_is_refused() {
    use std::os::unix::fs::symlink;

    let mission = temp_mission();
    let target = mission.repo_root.join("real-events.jsonl");
    fs::write(&target, b"{\"event_id\": \"keep\"}\n").unwrap();
    let log_path = mission.feature_dir.join(EVENTS_FILENAME);
    symlink(&target, &log_path).unwrap();

    let error = append_rows_atomic(
        &log_path,
        &[json!({"event_id": "x"})],
        &AppendOptions::default(),
    )
    .expect_err("a symlink log must be refused");
    assert!(matches!(error, MutationError::SymlinkRefused { .. }));
    assert_eq!(read_text(&target), "{\"event_id\": \"keep\"}\n");
}

#[test]
fn rollback_restores_a_captured_log_only_while_it_still_holds_our_bytes() {
    let mission = temp_mission();
    let log_path = mission.feature_dir.join(EVENTS_FILENAME);
    fs::write(&log_path, b"{\"event_id\": \"old\"}\n").unwrap();

    let snapshot = capture_log_snapshot(&log_path).expect("snapshot");
    append_rows_atomic(
        &log_path,
        &[json!({"event_id": "new"})],
        &AppendOptions::default(),
    )
    .expect("append");
    let written = fs::read(&log_path).unwrap();
    assert!(restore_log_snapshot(&snapshot, &written).expect("rollback"));
    assert_eq!(read_text(&log_path), "{\"event_id\": \"old\"}\n");

    // A foreign write is preserved: the rollback refuses instead of clobbering.
    let foreign = b"{\"event_id\": \"foreign\"}\n".to_vec();
    fs::write(&log_path, &foreign).unwrap();
    let error = restore_log_snapshot(&snapshot, &written)
        .expect_err("a foreign tail must refuse the rollback");
    assert!(matches!(error, MutationError::RollbackRefused { .. }));
    assert_eq!(fs::read(&log_path).unwrap(), foreign);
}

#[test]
fn indeterminate_commit_is_recovered_by_readback() {
    let mission = temp_mission();
    let log_path = mission.feature_dir.join(EVENTS_FILENAME);
    fs::write(&log_path, b"{\"event_id\": \"old\"}\n").unwrap();

    // Simulate the kernel: rename landed, parent sync failed.
    let outcome = append_rows_with_commit(
        &log_path,
        &[json!({"event_id": "new"})],
        &AppendOptions::default(),
        &mut |path, bytes, _create_parents| {
            fs::write(path, bytes).expect("write replacement");
            Err(sk_kernel::KernelError::AtomicCommitIndeterminate {
                path: path.to_path_buf(),
                source: Box::new(sk_kernel::KernelError::Io {
                    action: "sync parent directory of",
                    path: path.to_path_buf(),
                    source: io::Error::other("injected parent sync failure"),
                }),
            })
        },
    )
    .expect("a present-and-correct indeterminate commit is success");
    assert_eq!(outcome.durability, Durability::IndeterminateRecovered);
    assert!(read_text(&log_path).contains("\"new\""));
}

#[test]
fn an_indeterminate_commit_with_unexpected_bytes_is_reported_not_deleted() {
    let mission = temp_mission();
    let log_path = mission.feature_dir.join(EVENTS_FILENAME);
    fs::write(&log_path, b"{\"event_id\": \"old\"}\n").unwrap();

    let error = append_rows_with_commit(
        &log_path,
        &[json!({"event_id": "new"})],
        &AppendOptions::default(),
        &mut |path, _bytes, _create_parents| {
            fs::write(path, b"{\"event_id\": \"someone-else\"}\n").expect("write");
            Err(sk_kernel::KernelError::AtomicCommitIndeterminate {
                path: path.to_path_buf(),
                source: Box::new(sk_kernel::KernelError::Io {
                    action: "sync parent directory of",
                    path: path.to_path_buf(),
                    source: io::Error::other("injected parent sync failure"),
                }),
            })
        },
    )
    .expect_err("unexpected content after an indeterminate commit must be reported");
    assert!(matches!(error, MutationError::CommitIndeterminate { .. }));
    assert!(error.may_have_mutated());
    // Nothing is deleted: the (possibly committed) bytes stay for recovery.
    assert!(read_text(&log_path).contains("someone-else"));
}

#[test]
fn a_readback_mismatch_is_reported_and_the_file_is_left_alone() {
    let mission = temp_mission();
    let log_path = mission.feature_dir.join(EVENTS_FILENAME);

    let error = append_rows_with_commit(
        &log_path,
        &[json!({"event_id": "new"})],
        &AppendOptions::default(),
        &mut |path, _bytes, _create_parents| {
            fs::write(path, b"{\"event_id\": \"different\"}\n").expect("write");
            Ok(true)
        },
    )
    .expect_err("a mismatched readback must fail");
    assert!(matches!(error, MutationError::ReadbackMismatch { .. }));
    assert!(error.may_have_mutated());
    assert!(read_text(&log_path).contains("different"));
}

#[test]
fn a_log_larger_than_the_configured_bound_is_refused() {
    let mission = temp_mission();
    let log_path = mission.feature_dir.join(EVENTS_FILENAME);
    let options = AppendOptions {
        create_parents: true,
        max_bytes: 16,
    };
    let error = append_rows_atomic(&log_path, &[json!({"event_id": "long-id-here"})], &options)
        .expect_err("oversized log must be refused");
    assert!(matches!(
        error,
        MutationError::Kernel(sk_kernel::KernelError::TooLarge { .. })
    ));
    assert!(!log_path.exists(), "nothing is created on refusal");
}

#[test]
fn verify_event_rows_requires_the_expected_identity() {
    let mission = temp_mission();
    let log_path = mission.feature_dir.join(EVENTS_FILENAME);
    let row = json!({
        "event_id": "01K9M0R3WJ8V6Q1Y4T7B2N5C0A",
        "mission_slug": mutation_support::MISSION_SLUG,
        "wp_id": "WP01",
        "to_lane": "planned",
    });
    append_rows_atomic(
        &log_path,
        std::slice::from_ref(&row),
        &AppendOptions::default(),
    )
    .expect("append");
    verify_event_rows(&log_path, std::slice::from_ref(&row)).expect("row is present");

    let mismatched = json!({
        "event_id": "01K9M0R3WJ8V6Q1Y4T7B2N5C0A",
        "mission_slug": mutation_support::MISSION_SLUG,
        "wp_id": "WP01",
        "to_lane": "claimed",
    });
    let error = verify_event_rows(&log_path, std::slice::from_ref(&mismatched))
        .expect_err("mismatched lane must fail verification");
    assert!(matches!(error, MutationError::EventMissing { .. }));
}

#[test]
fn appended_rows_are_readable_through_the_canonical_reader() {
    let mission = temp_mission();
    let log_path = mission.feature_dir.join(EVENTS_FILENAME);
    let rows = vec![
        json!({"event_id": "01K9M0R3WJ8V6Q1Y4T7B2N5C0A", "hostname": "strip-me"}),
        json!({"event_id": "01K9M0R3WK9W7R2Z5V8C3P6D1B", "reason": "caf\u{e9}"}),
    ];
    append_rows_atomic(&log_path, &rows, &AppendOptions::default()).expect("append");
    let read = read_log_rows(&log_path).expect("read rows");
    assert_eq!(read.len(), 2);
    assert!(
        read[0].get("hostname").is_none(),
        "the writer strips PII before persisting"
    );
    assert_eq!(read[1]["reason"], json!("caf\u{e9}"));
}
