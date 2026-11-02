//! Targeted replay semantics: dedupe, continuity, rollback arbitration,
//! approved-departure lifting, annotations, cancellation and schema gates.

mod replay_support;

use serde_json::{Value, json};
use sk_status::snapshot::{
    CURRENT_SNAPSHOT_SCHEMA_VERSION, IMPLEMENTER_OF_RECORD_SLOT, LEGACY_SNAPSHOT_SCHEMA_VERSION,
    ReplayInput, existing_schema_version, materialize, reduce_rows, render_snapshot_json,
    target_schema_version,
};

use replay_support::{read_json, replay_fixture, replay_root};

fn input_for(case: &str) -> ReplayInput {
    ReplayInput::read_dir(&replay_root().join("cases").join(case)).expect("read case")
}

fn materialized(case: &str) -> (sk_status::snapshot::Snapshot, Value) {
    let replayed = replay_fixture(&replay_root().join("cases").join(case));
    (replayed.snapshot, replayed.value)
}

fn lane(value: &Value, wp: &str) -> String {
    value["work_packages"][wp]["lane"]
        .as_str()
        .expect("lane string")
        .to_owned()
}

#[test]
fn diary_dedupe_keeps_the_first_real_occurrence() {
    let input = input_for("dedupe-first-wins");
    let snapshot = materialize(&input).expect("materialize");
    assert_eq!(snapshot.event_count, 1);
    assert_eq!(
        snapshot.work_packages["WP01"]
            .get("actor")
            .and_then(Value::as_str),
        Some("first-agent")
    );
}

#[test]
fn same_id_seed_rows_choose_the_canonical_minimum_payload() {
    let input = input_for("dedupe-seeds");
    let reduced = reduce_rows(&input.rows).expect("diary reduce");
    assert_eq!(reduced.event_count, 1);
    assert_eq!(
        reduced.work_packages["WP01"]
            .get("agent")
            .and_then(Value::as_str),
        Some("seed-aaa"),
        "the external diary dedupe picks the canonical-minimum same-id seed"
    );

    let mut reversed = input.rows.clone();
    reversed.reverse();
    let reversed_reduced = reduce_rows(&reversed).expect("reverse diary reduce");
    assert_eq!(
        reduced.to_json_value(),
        reversed_reduced.to_json_value(),
        "the diary fold itself is file-order independent for seed dedupe"
    );
}

#[test]
fn baseline_implementer_projection_dedupes_first_occurrence_not_canonical_minimum() {
    // Regression pin for the baseline CLI projection's own dedupe: it is a
    // first-occurrence pass, so the same-id seed quirk is preserved rather than
    // "fixed" into a second, unannounced semantics.
    let input = input_for("dedupe-seeds");
    let snapshot = materialize(&input).expect("materialize");
    assert_eq!(
        snapshot.work_packages["WP01"]
            .get(IMPLEMENTER_OF_RECORD_SLOT)
            .and_then(Value::as_str),
        Some("seed-zzz")
    );

    let mut reversed = input.rows.clone();
    reversed.reverse();
    let reversed_snapshot = materialize(&ReplayInput {
        rows: reversed,
        ..input
    })
    .expect("reverse materialize");
    assert_eq!(
        reversed_snapshot.work_packages["WP01"]
            .get(IMPLEMENTER_OF_RECORD_SLOT)
            .and_then(Value::as_str),
        Some("seed-aaa"),
        "baseline provenance projection is first-occurrence by design"
    );
}

#[test]
fn a_stale_approval_never_overwrites_a_causal_rollback() {
    let (_, value) = materialized("stale-approval");
    assert_eq!(lane(&value, "WP01"), "planned");
    assert_eq!(
        value["work_packages"]["WP01"]["last_event_id"],
        "01J00000000000000000000005"
    );
}

#[test]
fn approved_departure_lifting_reorders_clock_skew_and_carries_the_floor() {
    let (_, value) = materialized("approved-departure-clock-skew");
    assert_eq!(lane(&value, "WP01"), "in_progress");
    assert_eq!(
        value["work_packages"]["WP01"]["last_transition_at"], "2026-01-01T00:00:05Z",
        "the stored departure timestamp is untouched"
    );
}

#[test]
fn same_timestamp_rollback_beats_a_forward_approval() {
    let (_, value) = materialized("rollback-same-timestamp");
    assert_eq!(lane(&value, "WP01"), "planned");
}

#[test]
fn rollback_seed_holds_the_lane_against_its_bare_sibling_approval() {
    let (_, value) = materialized("rollback-seed-bare-sibling");
    assert_eq!(lane(&value, "WP01"), "planned");
}

#[test]
fn a_losing_forward_seed_folds_only_the_unwritten_claim_payload() {
    let (_, value) = materialized("losing-seed-payload");
    let wp = &value["work_packages"]["WP01"];
    assert_eq!(wp["lane"], "in_progress");
    assert_eq!(wp["agent"], "seed-agent");
    assert_eq!(wp["shell_pid"], 999);
    assert_eq!(wp["shell_pid_created_at"], "999.0");
    assert_eq!(wp["last_event_id"], "01J00000000000000000000002");
}

#[test]
fn annotations_are_a_separate_pass_with_runtime_slot_independence() {
    let (snapshot, value) = materialized("annotation-runtime");
    let wp = &value["work_packages"]["WP01"];
    assert_eq!(wp["lane"], "claimed");
    assert_eq!(wp["agent"], "after-release");
    assert_eq!(wp["shell_pid"], 333);
    assert_eq!(wp["shell_pid_created_at"], Value::Null);
    assert_eq!(wp["subtasks"], json!({"T001": "done", "T002": "skipped"}));
    assert_eq!(wp["notes"], json!(["first", "second"]));
    assert_eq!(wp["tracker_refs"], json!(["GH-9"]));
    assert_eq!(wp["review"], Value::Null);
    assert_eq!(wp["role"], "reviewer");
    assert_eq!(wp["agent_profile"], "reviewer-renata");
    assert_eq!(wp["agent_profile_version"], "v1");
    assert_eq!(wp["model"], "m");
    assert_eq!(wp["provider"], "p");
    assert_eq!(wp["assignee"], "assignee-x");
    assert_eq!(wp["implementer_of_record"], "claim-agent");
    assert_eq!(wp["actor"], "impl");

    let runtime_only = &value["work_packages"]["WP02"];
    assert_eq!(runtime_only["lane"], "uninitialized");
    assert_eq!(runtime_only["last_transition_at"], Value::Null);
    assert_eq!(runtime_only["last_event_id"], Value::Null);
    assert_eq!(value["summary"]["claimed"], 1);
    assert_eq!(
        value["summary"].as_object().expect("summary").len(),
        9,
        "uninitialized is excluded from the display summary"
    );
    assert_eq!(
        snapshot.event_count, 1,
        "annotations never move event_count"
    );
}

#[test]
fn cancellation_provenance_is_projected_from_the_raw_stream_only_for_the_final_lane() {
    let (_, value) = materialized("cancellation-provenance");
    assert_eq!(value["work_packages"]["WP01"]["reason_source"], "synthetic");
    assert_eq!(
        value["work_packages"]["WP01"]["cancellation_reason"],
        "Force move to canceled"
    );
    assert_eq!(value["work_packages"]["WP02"]["reason_source"], "operator");
    assert_eq!(value["work_packages"]["WP03"]["reason_source"], "operator");
    assert_eq!(value["work_packages"]["WP04"]["reason_source"], "synthetic");
    assert_eq!(value["work_packages"]["WP04"]["cancellation_reason"], "");
    assert_eq!(value["work_packages"]["WP05"]["reason_source"], "synthetic");
    assert_eq!(lane(&value, "WP06"), "planned");
    assert!(
        value["work_packages"]["WP06"]
            .get("cancellation_reason")
            .is_none()
    );
    assert!(
        value["work_packages"]["WP06"]
            .get("reason_source")
            .is_none()
    );
}

#[test]
fn retrospective_rows_shape_the_derived_snapshot_without_becoming_lane_events() {
    let (snapshot, value) = materialized("retrospective");
    assert_eq!(snapshot.event_count, 1);
    let retro = &value["retrospective"];
    assert_eq!(retro["status"], "completed");
    assert_eq!(retro["mode"]["value"], "autonomous");
    assert_eq!(retro["mode"]["source_signal"]["kind"], "explicit_flag");
    assert_eq!(
        retro["record_path"],
        "kitty-specs/900-retrospective/retrospective.md"
    );
    assert_eq!(retro["proposals_total"], 2);
    assert_eq!(retro["proposals_applied"], 1);
    assert_eq!(retro["proposals_rejected"], 0);
    assert_eq!(retro["proposals_pending"], 1);
}

#[test]
fn legacy_snapshots_never_lift_the_new_read_root_field() {
    let (snapshot, value) = materialized("legacy-schema");
    assert_eq!(snapshot.schema_version, None);
    assert!(value.get("schema_version").is_none());
    assert!(
        value["work_packages"]["WP01"]
            .get(IMPLEMENTER_OF_RECORD_SLOT)
            .is_none()
    );
    assert_eq!(value["work_packages"]["WP01"]["agent"], "legacy-impl");
}

#[test]
fn current_snapshots_carry_the_marker_and_implementer_projection() {
    let (snapshot, value) = materialized("current-schema-v2");
    assert_eq!(
        snapshot.schema_version,
        Some(CURRENT_SNAPSHOT_SCHEMA_VERSION)
    );
    assert_eq!(value["schema_version"], 2);
    assert_eq!(
        value["work_packages"]["WP01"][IMPLEMENTER_OF_RECORD_SLOT],
        "legacy-impl"
    );
}

#[test]
fn schema_target_resolution_matches_the_baseline_reducer() {
    assert_eq!(target_schema_version(None), CURRENT_SNAPSHOT_SCHEMA_VERSION);
    assert_eq!(
        target_schema_version(Some(&json!({}))),
        LEGACY_SNAPSHOT_SCHEMA_VERSION
    );
    assert_eq!(
        existing_schema_version(Some(&json!({"schema_version": 2}))),
        Some(2)
    );
    assert_eq!(
        target_schema_version(Some(&json!({"schema_version": 2}))),
        2
    );
    assert_eq!(
        target_schema_version(Some(&json!({"schema_version": 3}))),
        3,
        "an existing generation keeps replaying at its own number"
    );
    assert_eq!(
        existing_schema_version(Some(&json!({"schema_version": "2"}))),
        None
    );
    assert_eq!(
        target_schema_version(Some(&json!({"schema_version": "2"}))),
        LEGACY_SNAPSHOT_SCHEMA_VERSION
    );
}

#[test]
fn empty_streams_have_the_canonical_summary_shape_and_meta_identity() {
    let (snapshot, value) = materialized("empty-stream");
    assert_eq!(snapshot.event_count, 0);
    assert_eq!(snapshot.mission_slug, "");
    assert_eq!(value["mission_slug"], "");
    assert_eq!(value["mission_number"], "7");
    assert_eq!(value["mission_type"], "documentation");
    assert_eq!(value["materialized_at"], "");
    assert_eq!(value["work_packages"], json!({}));
    assert_eq!(value["summary"].as_object().expect("summary").len(), 9);
}

#[test]
fn an_unsafe_event_slug_is_sanitised_at_the_snapshot_seam() {
    let (snapshot, value) = materialized("unsafe-mission-slug");
    assert_eq!(snapshot.mission_slug, "");
    assert_eq!(value["mission_slug"], "");
}

#[test]
fn materialize_output_is_stable_across_repeated_runs() {
    let input = input_for("shuffled-basic");
    let first = render_snapshot_json(&materialize(&input).expect("first")).expect("render");
    let second = render_snapshot_json(&materialize(&input).expect("second")).expect("render");
    assert_eq!(first, second);
}

#[test]
fn archive_fixture_inputs_stay_unchanged_by_materialization() {
    let dir = replay_root().join("archives/053-orchestrator-api-json-contract-fidelity");
    let before_events = std::fs::read(dir.join("status.events.jsonl")).expect("events");
    let before_status = std::fs::read(dir.join("status.json")).expect("status");
    let input = ReplayInput::read_dir(&dir).expect("input");
    let _ = materialize(&input).expect("materialize");
    assert_eq!(
        before_events,
        std::fs::read(dir.join("status.events.jsonl")).expect("events")
    );
    assert_eq!(
        before_status,
        std::fs::read(dir.join("status.json")).expect("status")
    );
    let _ = read_json(&dir.join("expected.json"));
}
