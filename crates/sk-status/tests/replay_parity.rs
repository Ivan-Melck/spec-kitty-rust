//! Byte-exact snapshot parity against the committed pinned-oracle corpus.

mod replay_support;

use std::fs;
use std::path::Path;

use serde_json::Value;
use sk_status::snapshot::{
    CURRENT_SNAPSHOT_SCHEMA_VERSION, IMPLEMENTER_OF_RECORD_SLOT, LEGACY_SNAPSHOT_SCHEMA_VERSION,
    ReplayInput, materialize, render_snapshot_json,
};

use replay_support::{
    expected_text, expected_value, fixture_dirs, fixture_name, provenance, read_json,
    replay_fixture, replay_root, sha256_hex,
};

#[test]
fn every_synthetic_case_matches_the_oracle_exactly() {
    let dirs = fixture_dirs("cases");
    assert!(dirs.len() >= 15, "expected the full synthetic matrix");
    let mut checked = 0usize;
    for dir in dirs {
        let name = fixture_name(&dir);
        let replayed = replay_fixture(&dir);
        assert_eq!(
            replayed.text,
            expected_text(&dir),
            "{name}: rendered snapshot diverged from the oracle"
        );
        assert_eq!(
            replayed.value,
            expected_value(&dir),
            "{name}: parsed snapshot diverged from the oracle"
        );
        checked += 1;
    }
    assert!(checked >= 15, "looped every synthetic fixture");
}

#[test]
fn bounded_archive_corpus_matches_the_oracle_exactly() {
    let dirs = fixture_dirs("archives");
    assert!(dirs.len() >= 10, "bounded archive corpus is present");
    let mut legacy = 0usize;
    let mut current = 0usize;
    let mut total_events = 0usize;

    for dir in dirs {
        let name = fixture_name(&dir);
        let input = ReplayInput::read_dir(&dir).expect("read archive fixture");
        total_events += input.rows.len();
        let replayed = replay_fixture(&dir);
        assert_eq!(
            replayed.text,
            expected_text(&dir),
            "{name}: archived mission replay diverged from the oracle"
        );

        let expected = expected_value(&dir);
        let on_disk = dir.join("status.json");
        if on_disk.exists() {
            let disk = read_json(&on_disk);
            if disk.get("schema_version").is_none() {
                legacy += 1;
                assert!(
                    expected.get("schema_version").is_none(),
                    "{name}: a legacy snapshot must replay without a schema marker"
                );
                for (wp, state) in expected["work_packages"]
                    .as_object()
                    .expect("work_packages object")
                {
                    assert!(
                        state.get(IMPLEMENTER_OF_RECORD_SLOT).is_none(),
                        "{name}/{wp}: legacy replay must not lift implementer_of_record"
                    );
                }
            } else {
                current += 1;
                assert_eq!(
                    expected.get("schema_version").and_then(Value::as_i64),
                    Some(CURRENT_SNAPSHOT_SCHEMA_VERSION),
                    "{name}: v2 snapshot must replay at its own schema"
                );
            }
        }
    }

    assert!(legacy >= 3, "legacy-schema archives are covered");
    assert!(current >= 3, "current-schema archives are covered");
    assert!(total_events <= 500, "archive corpus stays bounded");
}

#[test]
fn provenance_pins_the_oracle_and_matches_uv_lock() {
    let provenance = provenance();
    let version = provenance["oracle"]["spec_kitty_events_version"]
        .as_str()
        .expect("oracle version");
    let lock = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../uv.lock"))
        .expect("read uv.lock");
    let marker = format!("name = \"spec-kitty-events\"\nversion = \"{version}\"");
    assert!(
        lock.contains(&marker),
        "oracle version {version} is not the uv.lock spec-kitty-events pin"
    );
    assert_eq!(
        provenance["oracle"]["external_reducer"].as_str(),
        Some("spec_kitty_events.diary.reduce_parsed")
    );
    assert_eq!(
        provenance["oracle"]["entry_point"].as_str(),
        Some("specify_cli.status.reducer.materialize_snapshot")
    );
}

#[test]
fn provenance_digests_and_bounds_match_the_committed_files() {
    let provenance = provenance();
    let totals = &provenance["totals"];
    let declared_events = totals["events"].as_u64().expect("events total");
    let declared_bytes = totals["input_bytes"].as_u64().expect("input bytes total");
    assert!(declared_events <= 500);
    assert!(declared_bytes <= 400_000);

    let fixtures = provenance["fixtures"].as_array().expect("fixtures array");
    assert_eq!(
        fixtures.len() as u64,
        totals["fixtures"].as_u64().expect("fixture total")
    );

    let mut sum_events = 0u64;
    let mut sum_bytes = 0u64;
    for entry in fixtures {
        let kind = entry["kind"].as_str().expect("fixture kind");
        let name = entry["name"].as_str().expect("fixture name");
        let family = match kind {
            "synthetic" => "cases",
            "archive" => "archives",
            other => other,
        };
        let dir = replay_root().join(family).join(name);
        assert!(
            dir.is_dir(),
            "declared fixture is missing: {}",
            dir.display()
        );
        let events = fs::read(dir.join("status.events.jsonl")).expect("events fixture");
        let expected = fs::read(dir.join("expected.json")).expect("expected fixture");
        assert_eq!(
            entry["events_sha256"].as_str(),
            Some(sha256_hex(&events).as_str()),
            "{name}: input digest drifted"
        );
        assert_eq!(
            entry["expected_sha256"].as_str(),
            Some(sha256_hex(&expected).as_str()),
            "{name}: expected digest drifted"
        );
        sum_events += entry["events"].as_u64().expect("fixture event count");
        sum_bytes += entry["input_bytes"].as_u64().expect("fixture byte count");
    }
    assert_eq!(sum_events, declared_events);
    assert_eq!(sum_bytes, declared_bytes);
}

#[test]
fn reordered_rows_do_not_change_a_duplicate_free_replay() {
    let dir = replay_root().join("cases/shuffled-basic");
    let input = ReplayInput::read_dir(&dir).expect("read case");
    let expected = expected_text(&dir);
    let mut rows = input.rows.clone();
    rows.reverse();
    let replayed = materialize(&ReplayInput {
        rows,
        ..input.clone()
    })
    .expect("materialize reversed rows");
    assert_eq!(
        render_snapshot_json(&replayed).expect("render"),
        expected,
        "transition and annotation passes must both be sorted"
    );
}

#[test]
fn legacy_and_current_schemas_are_both_represented_in_synthetic_cases() {
    let legacy = replay_fixture(&replay_root().join("cases/legacy-schema"));
    assert_eq!(legacy.snapshot.schema_version, None);
    assert!(
        legacy.snapshot.work_packages["WP01"]
            .get(IMPLEMENTER_OF_RECORD_SLOT)
            .is_none()
    );

    let current = replay_fixture(&replay_root().join("cases/current-schema-v2"));
    assert_eq!(
        current.snapshot.schema_version,
        Some(CURRENT_SNAPSHOT_SCHEMA_VERSION)
    );
    assert_eq!(
        current.snapshot.work_packages["WP01"]
            .get(IMPLEMENTER_OF_RECORD_SLOT)
            .and_then(Value::as_str),
        Some("legacy-impl")
    );
    assert_ne!(
        LEGACY_SNAPSHOT_SCHEMA_VERSION,
        CURRENT_SNAPSHOT_SCHEMA_VERSION
    );
}
