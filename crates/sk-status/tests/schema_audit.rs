//! The schema auditor is structural and non-vacuous: a clean oracle snapshot
//! passes, and each injected schema violation is reported by code.

mod replay_support;

use serde_json::json;
use sk_status::schema::{audit_event_rows, audit_snapshot};
use sk_status::snapshot::{ReplayInput, materialize};

use replay_support::{fixture_dirs, replay_fixture, replay_root};

fn codes(snapshot: &sk_status::snapshot::Snapshot) -> Vec<&'static str> {
    audit_snapshot(snapshot).codes()
}

#[test]
fn every_oracle_snapshot_passes_the_structural_audit() {
    for dir in fixture_dirs("cases")
        .into_iter()
        .chain(fixture_dirs("archives"))
    {
        let replayed = replay_fixture(&dir);
        let audit = audit_snapshot(&replayed.snapshot);
        assert!(audit.is_clean(), "{}: {:?}", dir.display(), audit.findings);
    }
}

#[test]
fn display_summary_must_have_exactly_the_nine_lanes_and_matching_counts() {
    let mut snapshot = replay_fixture(&replay_root().join("cases/shuffled-basic")).snapshot;
    snapshot.summary.remove("planned");
    assert!(codes(&snapshot).contains(&"summary.lanes"));

    let mut snapshot = replay_fixture(&replay_root().join("cases/shuffled-basic")).snapshot;
    snapshot.summary.insert("planned".to_owned(), 99);
    assert!(codes(&snapshot).contains(&"summary.counts"));
}

#[test]
fn schema_marker_and_versioned_field_are_audited() {
    let mut snapshot = replay_fixture(&replay_root().join("cases/legacy-schema")).snapshot;
    snapshot.schema_version = Some(1);
    assert!(codes(&snapshot).contains(&"schema.legacy_marker"));

    let mut snapshot = replay_fixture(&replay_root().join("cases/legacy-schema")).snapshot;
    snapshot
        .work_packages
        .get_mut("WP01")
        .expect("WP01")
        .insert("implementer_of_record", json!("someone"));
    assert!(codes(&snapshot).contains(&"schema.implementer_of_record"));
}

#[test]
fn work_package_slots_are_audited() {
    let mut snapshot = replay_fixture(&replay_root().join("cases/shuffled-basic")).snapshot;
    snapshot
        .work_packages
        .get_mut("WP01")
        .expect("WP01")
        .insert("lane", json!("doing"));
    assert!(codes(&snapshot).contains(&"work_package.lane"));

    let mut snapshot = replay_fixture(&replay_root().join("cases/shuffled-basic")).snapshot;
    snapshot
        .work_packages
        .get_mut("WP01")
        .expect("WP01")
        .insert("force_count", json!(-1));
    assert!(codes(&snapshot).contains(&"work_package.force_count"));

    let mut snapshot = replay_fixture(&replay_root().join("cases/shuffled-basic")).snapshot;
    snapshot
        .work_packages
        .get_mut("WP01")
        .expect("WP01")
        .insert("review_result", json!({"reviewer": "r"}));
    assert!(codes(&snapshot).contains(&"work_package.review_result"));

    let mut snapshot = replay_fixture(&replay_root().join("cases/shuffled-basic")).snapshot;
    snapshot
        .work_packages
        .get_mut("WP01")
        .expect("WP01")
        .insert("reason_source", json!("operator"));
    assert!(codes(&snapshot).contains(&"work_package.cancellation"));
}

#[test]
fn transition_event_rows_are_audited_by_the_canonical_schema_rules() {
    let dir = replay_root().join("cases/shuffled-basic");
    let input = ReplayInput::read_dir(&dir).expect("read case");
    let clean = audit_event_rows(&input.rows);
    assert!(
        clean.is_clean(),
        "clean rows findings: {:?}",
        clean.findings
    );

    let mut rows = input.rows.clone();
    let index = rows
        .iter()
        .position(|row| row.get("from_lane").is_some())
        .expect("transition row");
    rows[index]["force"] = json!(true);
    rows[index]
        .as_object_mut()
        .expect("object")
        .remove("reason");
    let audit = audit_event_rows(&rows);
    assert!(
        audit.codes().contains(&"event.force_reason"),
        "{:?}",
        audit.findings
    );

    let mut rows = input.rows.clone();
    let index = rows
        .iter()
        .position(|row| row.get("from_lane").is_some())
        .expect("transition row");
    rows[index]["to_lane"] = json!("genesis");
    let audit = audit_event_rows(&rows);
    assert!(
        audit.codes().contains(&"event.to_lane"),
        "{:?}",
        audit.findings
    );

    let mut rows = input.rows.clone();
    let index = rows
        .iter()
        .position(|row| row.get("from_lane").is_some())
        .expect("transition row");
    rows[index]["execution_mode"] = json!("remote");
    rows[index]["at"] = json!("not-a-timestamp");
    let audit = audit_event_rows(&rows);
    assert!(audit.codes().contains(&"event.execution_mode"));
    assert!(audit.codes().contains(&"event.at"));
}

#[test]
fn unknown_kinds_and_invalid_annotations_cannot_audit_clean() {
    for row in [
        json!({"kind": "future-change"}),
        json!({"kind": "annotation", "event_id": "not-a-ulid"}),
    ] {
        assert!(
            !audit_event_rows(std::slice::from_ref(&row)).is_clean(),
            "{row}"
        );
        assert!(
            materialize(&ReplayInput {
                rows: vec![row],
                ..ReplayInput::default()
            })
            .is_err()
        );
    }
    let input = ReplayInput::read_dir(&replay_root().join("cases/shuffled-basic")).unwrap();
    let transition = input
        .rows
        .iter()
        .find(|row| row.get("from_lane").is_some())
        .unwrap();
    for field in ["from_lane", "to_lane", "at", "execution_mode"] {
        let mut row = transition.clone();
        row[field] = json!(42);
        assert!(!audit_event_rows(&[row]).is_clean(), "{field}");
    }
}

#[test]
fn the_auditor_is_non_vacuous_on_its_own_clean_input() {
    let dir = replay_root().join("cases/shuffled-basic");
    let snapshot = materialize(&ReplayInput::read_dir(&dir).expect("read")).expect("materialize");
    assert!(audit_snapshot(&snapshot).is_clean());
    let mut broken = snapshot;
    broken.event_count = 0;
    assert!(!audit_snapshot(&broken).is_clean());
}
