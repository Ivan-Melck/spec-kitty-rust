//! Lifecycle-envelope migration tests against the pinned oracle fixture.

mod mutation_support;

use std::fs;
use std::path::PathBuf;

use serde_json::Value;
use sk_status::mutation::{
    MigrationAction, MutationError, STRICT_SCHEMA_VERSION, build_id_for,
    migrate_lifecycle_envelope, node_id_from_host_user,
};
use sk_status::snapshot::EVENTS_FILENAME;

use mutation_support::{expected, fixture, read_json, read_text};

const NODE_ID: &str = "abcdef123456";

fn migrated_copy() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().expect("temp dir");
    let root = temp.path().join("repo");
    mutation_support::copy_dir(&fixture("migration").join("input"), &root);
    (temp, root.join(EVENTS_FILENAME))
}

fn manifest_json(manifest: &sk_status::mutation::MigrationManifest) -> Value {
    serde_json::json!({
        "total_rows": manifest.total_rows,
        "migrated_count": manifest.migrated_count,
        "unchanged_count": manifest.unchanged_count,
        "skipped_count": manifest.skipped_count,
        "rows": manifest.rows.iter().map(|row| serde_json::json!({
            "event_id": row.event_id,
            "event_type": row.event_type,
            "action": row.action.as_str(),
            "pre_hash": row.pre_hash,
            "post_hash": row.post_hash,
        })).collect::<Vec<_>>(),
    })
}

#[test]
fn uuid_spellings_and_nonstring_values_match_the_verified_oracle_without_loss() {
    let data = read_json(&fixture("edge-cases.json"));
    let cases = data["uuid_migrations"].as_array().unwrap();
    assert_eq!(cases.len(), 10);
    for case in cases {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(EVENTS_FILENAME);
        let original = serde_json::to_vec(&case["input"]).unwrap();
        fs::write(&path, &original).unwrap();
        let result = migrate_lifecycle_envelope(&path, false, Some(NODE_ID), None);
        if case.get("error_type").is_some() {
            assert!(result.is_err(), "input: {}", case["input"]);
            assert_eq!(fs::read(&path).unwrap(), original);
            assert!(
                !temp
                    .path()
                    .join(format!("{EVENTS_FILENAME}.pre-migration.bak"))
                    .exists()
            );
        } else {
            assert_eq!(result.unwrap().migrated_count, 1);
            assert_eq!(
                read_json(&path),
                case["expected"],
                "input: {}",
                case["input"]
            );
        }
    }
}

#[test]
fn the_migration_matches_the_pinned_oracle() {
    let (_temp, log_path) = migrated_copy();
    let manifest =
        migrate_lifecycle_envelope(&log_path, false, Some(NODE_ID), None).expect("migrate");
    assert_eq!(manifest.refused_reason, None);
    assert_eq!(manifest.migrated_count, 2);
    assert_eq!(manifest.unchanged_count, 1);
    assert_eq!(manifest.skipped_count, 2);

    assert_eq!(
        read_text(&log_path),
        read_text(&expected("migration", EVENTS_FILENAME)),
        "the migrated log must be byte-identical to the pinned migration"
    );
    let backup = log_path.with_file_name(format!("{EVENTS_FILENAME}.pre-migration.bak"));
    assert_eq!(
        read_text(&backup),
        read_text(&expected(
            "migration",
            &format!("{EVENTS_FILENAME}.pre-migration.bak")
        )),
        "the snapshot must be the untouched pre-migration text"
    );
    assert_eq!(
        manifest_json(&manifest),
        read_json(&expected("migration", "manifest.json"))
    );
}

#[test]
fn a_dry_run_computes_the_manifest_without_writing() {
    let (_temp, log_path) = migrated_copy();
    let before = read_text(&log_path);
    let manifest =
        migrate_lifecycle_envelope(&log_path, true, Some(NODE_ID), None).expect("dry run");
    assert!(manifest.dry_run);
    assert!(!manifest.changed());
    assert_eq!(manifest.migrated_count, 2);
    assert_eq!(read_text(&log_path), before);
    assert!(
        !log_path
            .with_file_name(format!("{EVENTS_FILENAME}.pre-migration.bak"))
            .exists()
    );
}

#[test]
fn a_second_run_is_idempotent_and_does_not_trip_the_snapshot_refusal() {
    let (_temp, log_path) = migrated_copy();
    migrate_lifecycle_envelope(&log_path, false, Some(NODE_ID), None).expect("first");
    let migrated = read_text(&log_path);

    let second = migrate_lifecycle_envelope(&log_path, false, Some(NODE_ID), None).expect("second");
    assert_eq!(second.migrated_count, 0);
    assert_eq!(second.refused_reason, None);
    assert_eq!(read_text(&log_path), migrated);
}

#[test]
fn an_existing_snapshot_refuses_a_second_migration_of_fresh_input() {
    let (_temp, log_path) = migrated_copy();
    let backup = log_path.with_file_name(format!("{EVENTS_FILENAME}.pre-migration.bak"));
    fs::write(&backup, "earlier snapshot\n").unwrap();
    let before = read_text(&log_path);

    let manifest =
        migrate_lifecycle_envelope(&log_path, false, Some(NODE_ID), None).expect("refusal");
    assert!(manifest.refused_reason.is_some());
    assert_eq!(
        manifest.migrated_count, 2,
        "the manifest still explains the work"
    );
    assert_eq!(read_text(&log_path), before, "a refusal writes nothing");
    assert_eq!(read_text(&backup), "earlier snapshot\n");
}

#[test]
fn malformed_rows_are_refused_with_their_line_number() {
    let temp = tempfile::tempdir().unwrap();
    let log_path = temp.path().join(EVENTS_FILENAME);
    fs::write(
        &log_path,
        "{\"event_type\": \"MissionCreated\"}\nnot json\n",
    )
    .unwrap();
    let error = migrate_lifecycle_envelope(&log_path, false, Some(NODE_ID), None)
        .expect_err("malformed input is refused");
    match error {
        MutationError::MalformedStream { line, .. } => assert_eq!(line, 2),
        other => panic!("unexpected error: {other}"),
    }
    assert!(
        !log_path
            .with_file_name(format!("{EVENTS_FILENAME}.pre-migration.bak"))
            .exists()
    );
}

#[test]
fn build_id_matches_the_baseline_uuid5_derivation() {
    // The value the pinned oracle wrote for this (project_uuid, node_id) pair.
    assert_eq!(
        build_id_for("1f8bca14-6f1b-4a1c-9a6c-1d6f0f0b2c3d", NODE_ID),
        "40654732-0e58-5dbd-9379-79d627f888f1"
    );
}

#[test]
fn the_node_id_is_twelve_lower_hex_characters() {
    let node = node_id_from_host_user();
    assert_eq!(node.len(), 12);
    assert!(node.chars().all(|character| character.is_ascii_hexdigit()));
}

#[test]
fn strict_rows_carry_the_migrated_shape() {
    let (_temp, log_path) = migrated_copy();
    migrate_lifecycle_envelope(&log_path, false, Some(NODE_ID), None).expect("migrate");
    let rows = read_text(&log_path);
    let first: Value = serde_json::from_str(rows.lines().next().unwrap()).unwrap();
    assert_eq!(
        first["schema_version"],
        Value::String(STRICT_SCHEMA_VERSION.to_owned())
    );
    assert!(first.get("aggregate_type").is_none());
    assert_eq!(first["data_tier"], Value::from(0));
    assert_eq!(first["lamport_clock"], Value::from(0));
    assert_eq!(first["causation_id"], Value::Null);
    assert_eq!(first["node_id"], Value::String(NODE_ID.to_owned()));
    // The semantic pair is untouched.
    assert_eq!(first["payload"]["mission_slug"], "034-mutation-fixture");
}

#[test]
fn actions_are_reported_per_row() {
    let (_temp, log_path) = migrated_copy();
    let manifest =
        migrate_lifecycle_envelope(&log_path, false, Some(NODE_ID), None).expect("migrate");
    let actions: Vec<MigrationAction> = manifest.rows.iter().map(|row| row.action).collect();
    assert_eq!(
        actions,
        vec![
            MigrationAction::Migrated,
            MigrationAction::Migrated,
            MigrationAction::SkippedNoModel,
            MigrationAction::SkippedNoProjectUuid,
            MigrationAction::Unchanged,
        ]
    );
    for row in &manifest.rows {
        assert_eq!(row.pre_hash, row.post_hash, "only the envelope changes");
    }
}
