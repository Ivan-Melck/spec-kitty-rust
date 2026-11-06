//! Permanent Source-first differential for the charter bundle v1→v2 migration
//! and the v2 manifest-defaults repair.
//!
//! `fixtures/charter_versioning_oracle.json` was produced by the pinned Source
//! (`charter.offering.versioning.migrate_v1_to_v2` and
//! `repair_v2_synthesis_manifest_defaults` under `.venv-rust-oracle`, CPython
//! 3.11.15 / 53 distributions) from the recorded inputs, with the sidecar mtime
//! pinned so `produced_at` is deterministic.
//!
//! The fixture is committed; a missing fixture fails instead of skipping.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::Value as Json;
use sk_install::migrations::completion_modern::charter_bundle;

fn fixture() -> Json {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/charter_versioning_oracle.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "charter versioning oracle fixture is required but unreadable at {}: {error}",
            path.display()
        )
    });
    serde_json::from_str(&text).expect("fixture is valid JSON")
}

fn string_field<'a>(fixture: &'a Json, group: &str, key: &str) -> &'a str {
    fixture
        .get(group)
        .and_then(|group| group.get(key))
        .and_then(Json::as_str)
        .unwrap_or_else(|| panic!("fixture missing {group}.{key}"))
}

fn write_fixture_project(fixture: &Json, root: &Path, mtime: u64) {
    let charter = root.join(".kittify").join("charter");
    std::fs::create_dir_all(charter.join("provenance")).expect("create charter dir");
    std::fs::write(
        charter.join("charter.yaml"),
        string_field(fixture, "inputs", "charter_yaml"),
    )
    .expect("write charter.yaml");
    let sidecar = charter
        .join("provenance")
        .join("directive-project-intent-directive.yaml");
    std::fs::write(&sidecar, string_field(fixture, "inputs", "sidecar_v1")).expect("write sidecar");
    set_mtime(&sidecar, mtime);
    std::fs::write(
        charter.join("synthesis-manifest.yaml"),
        string_field(fixture, "inputs", "manifest_v1"),
    )
    .expect("write manifest");
}

fn set_mtime(path: &Path, epoch_seconds: u64) {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open for mtime");
    let stamp = SystemTime::UNIX_EPOCH + Duration::from_secs(epoch_seconds);
    file.set_times(
        std::fs::FileTimes::new()
            .set_accessed(stamp)
            .set_modified(stamp),
    )
    .expect("set mtime");
}

#[test]
fn v1_to_v2_migration_matches_the_recorded_source_bytes() {
    let fixture = fixture();
    let sandbox = tempfile::tempdir().expect("tempdir");
    let root = sandbox.path();
    let mtime = fixture
        .get("inputs")
        .and_then(|inputs| inputs.get("fixed_mtime"))
        .and_then(Json::as_u64)
        .expect("fixed_mtime");
    write_fixture_project(&fixture, root, mtime);

    let charter = root.join(".kittify").join("charter");
    let result = charter_bundle::migrate_v1_to_v2(&charter, false);
    assert!(
        result.errors.is_empty(),
        "migration reported errors: {:?}",
        result.errors
    );
    let expected_files = fixture
        .get("files")
        .and_then(Json::as_object)
        .expect("files");
    assert_eq!(result.changes_made.len(), expected_files.len());
    for (relative, expected) in expected_files {
        let expected = expected.as_str().expect("fixture file is a string");
        let actual = std::fs::read_to_string(root.join(relative))
            .unwrap_or_else(|error| panic!("{relative} unreadable: {error}"));
        assert_eq!(
            actual, expected,
            "{relative} diverges from the pinned Source bytes"
        );
    }
}

#[test]
fn v2_manifest_repair_matches_the_recorded_source_bytes() {
    let fixture = fixture();
    let sandbox = tempfile::tempdir().expect("tempdir");
    let root = sandbox.path();
    write_fixture_project(&fixture, root, 0);
    let charter = root.join(".kittify").join("charter");
    charter_bundle::migrate_v1_to_v2(&charter, false);
    std::fs::write(
        charter.join("synthesis-manifest.yaml"),
        string_field(&fixture, "inputs", "pre_v2_manifest"),
    )
    .expect("write pre-v2 manifest");

    let result = charter_bundle::repair_v2_synthesis_manifest_defaults(&charter, false);
    assert!(
        result.errors.is_empty(),
        "repair errors: {:?}",
        result.errors
    );
    assert_eq!(
        result.changes_made.len(),
        1,
        "exactly the manifest is rewritten"
    );
    let actual = std::fs::read_to_string(charter.join("synthesis-manifest.yaml"))
        .expect("read repaired manifest");
    assert_eq!(
        actual,
        fixture
            .get("repaired_manifest")
            .and_then(Json::as_str)
            .expect("repaired_manifest"),
        "repaired manifest diverges from the pinned Source bytes"
    );
}

#[test]
fn repair_is_idempotent_and_detects_nothing_afterwards() {
    let fixture = fixture();
    let sandbox = tempfile::tempdir().expect("tempdir");
    let root = sandbox.path();
    write_fixture_project(&fixture, root, 0);
    let charter = root.join(".kittify").join("charter");
    // Bring the bundle to v2 first (as a real project would be), then plant the
    // pre-`built_in_only` manifest the repair migration owns.
    charter_bundle::migrate_v1_to_v2(&charter, false);
    std::fs::write(
        charter.join("synthesis-manifest.yaml"),
        string_field(&fixture, "inputs", "pre_v2_manifest"),
    )
    .expect("write pre-v2 manifest");
    assert!(
        charter_bundle::detect_charter_manifest_defaults_repair(root).expect("detect"),
        "a pre-defaults v2 manifest is detected"
    );
    charter_bundle::apply_charter_manifest_defaults_repair(root, false).expect("apply");
    assert!(
        !charter_bundle::detect_charter_manifest_defaults_repair(root).expect("detect"),
        "a repaired manifest is not selected again"
    );
    // The bundle is now v2, so the v1->v2 migration reports no pending work.
    assert!(
        !charter_bundle::detect_charter_bundle_v2(root).expect("detect"),
        "a repaired v2 bundle needs no schema migration"
    );
}

#[test]
fn dry_run_writes_nothing_but_reports_the_same_files() {
    let fixture = fixture();
    let sandbox = tempfile::tempdir().expect("tempdir");
    let root = sandbox.path();
    let mtime = fixture
        .get("inputs")
        .and_then(|inputs| inputs.get("fixed_mtime"))
        .and_then(Json::as_u64)
        .expect("fixed_mtime");
    write_fixture_project(&fixture, root, mtime);
    let charter = root.join(".kittify").join("charter");
    let before_sidecar = std::fs::read_to_string(
        charter
            .join("provenance")
            .join("directive-project-intent-directive.yaml"),
    )
    .expect("read sidecar");
    let result = charter_bundle::migrate_v1_to_v2(&charter, true);
    assert!(result.errors.is_empty());
    assert_eq!(result.changes_made.len(), 3);
    let after_sidecar = std::fs::read_to_string(
        charter
            .join("provenance")
            .join("directive-project-intent-directive.yaml"),
    )
    .expect("read sidecar");
    assert_eq!(before_sidecar, after_sidecar, "dry run must not write");
}

#[test]
fn compatibility_messages_match_the_source_table() {
    use charter_bundle::BundleCompatibilityStatus as Status;
    let cases = [
        (None, Status::MissingVersion, 1),
        (Some(1), Status::NeedsMigration, 1),
        (Some(2), Status::Compatible, 0),
        (Some(0), Status::IncompatibleOld, 1),
        (Some(3), Status::IncompatibleNew, 1),
    ];
    for (version, expected, exit_code) in cases {
        let verdict = charter_bundle::check_bundle_compatibility(version);
        assert_eq!(verdict.status, expected, "version {version:?}");
        assert_eq!(verdict.exit_code, exit_code, "version {version:?}");
        assert_eq!(verdict.supported_min, 1);
        assert_eq!(verdict.supported_max, 2);
    }
    assert_eq!(
        charter_bundle::check_bundle_compatibility(None).message,
        "Bundle schema version not found; treating as v1. Run `spec-kitty upgrade`."
    );
    assert_eq!(
        charter_bundle::check_bundle_compatibility(Some(2)).message,
        "Bundle schema version 2 is supported."
    );
}
