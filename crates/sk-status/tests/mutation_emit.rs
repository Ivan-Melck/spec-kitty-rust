//! Emission tests: lane transitions, batches, annotations and gates, compared
//! byte-for-byte with the pinned oracle where a fixture exists.

mod mutation_support;

use std::fs;

use serde_json::{Value, json};
use sk_status::mutation::{
    ActorValue, AnnotationRequest, TransitionRequest, emit_inner_state_changed,
    emit_status_transition, emit_status_transition_batch,
};
use sk_status::snapshot::{EVENTS_FILENAME, SNAPSHOT_FILENAME};
use sk_status::{Lane, MutationError, TransitionError, reduce_rows, render_snapshot_json};

use mutation_support::{
    MISSION_SLUG, copy_fixture_input, expected, fixture, read_json, read_text, temp_mission,
    temp_mission_with,
};

fn actor(value: &Value) -> ActorValue {
    ActorValue::from_json(value).expect("fixture actor")
}

#[test]
fn the_emit_sequence_matches_the_pinned_oracle() {
    let mission = copy_fixture_input("emit_sequence");
    let requests = read_json(&fixture("emit_sequence").join("requests.json"));
    let requests = requests.as_array().expect("requests array");

    for request in requests {
        let mut transition = TransitionRequest::new(
            &mission.feature_dir,
            MISSION_SLUG,
            request["wp_id"].as_str().unwrap(),
            request["to_lane"].as_str().unwrap(),
            actor(&request["actor"]),
        );
        transition.repo_root = Some(mission.repo_root.clone());
        transition.at = request["at"].as_str().map(str::to_owned);
        transition.event_id = request["event_id"].as_str().map(str::to_owned);
        transition.force = request["force"].as_bool().unwrap_or(false);
        let outcome = emit_status_transition(&transition).expect("emit");
        assert!(outcome.persisted);
        assert_eq!(
            outcome.event.to_lane.as_str(),
            request["to_lane"].as_str().unwrap()
        );
        assert!(outcome.materialize_error.is_none());
    }

    assert_eq!(
        read_text(&mission.feature_dir.join(EVENTS_FILENAME)),
        read_text(&expected("emit_sequence", EVENTS_FILENAME)),
        "the emitted log must be byte-identical to the pinned writer"
    );
    assert_eq!(
        read_text(&mission.feature_dir.join(SNAPSHOT_FILENAME)),
        read_text(&expected("emit_sequence", SNAPSHOT_FILENAME)),
        "the materialized snapshot must be byte-identical to the pinned writer"
    );

    // The Mission lock family was used, and its holder sidecar is cleared.
    let lock_dir = mission.repo_root.join(".spec-kitty/spec-kitty-locks");
    let lock = lock_dir.join(format!("{MISSION_SLUG}.status.lock"));
    assert!(lock.is_file(), "mission lock file at {}", lock.display());
    assert!(
        !lock_dir
            .join(format!("{MISSION_SLUG}.status.lock.holder"))
            .exists(),
        "the holder sidecar must be removed when the outermost guard drops"
    );
}

#[test]
fn an_illegal_edge_persists_nothing() {
    let mission = temp_mission();
    let mut request = TransitionRequest::new(
        &mission.feature_dir,
        MISSION_SLUG,
        "WP01",
        "approved",
        ActorValue::identity("claude"),
    );
    request.repo_root = Some(mission.repo_root.clone());
    let error = emit_status_transition(&request).expect_err("planned -> approved is illegal");
    assert!(
        matches!(
            error,
            MutationError::Transition(TransitionError::IllegalEdge { .. })
        ),
        "unexpected error: {error}"
    );
    assert!(!mission.feature_dir.join(EVENTS_FILENAME).exists());
}

#[test]
fn a_legacy_alias_that_resolves_to_the_current_lane_is_a_synthetic_no_op() {
    let mission = temp_mission();
    seed_to_in_progress(&mission.feature_dir, &mission.repo_root);
    let before = read_text(&mission.feature_dir.join(EVENTS_FILENAME));

    let mut request = TransitionRequest::new(
        &mission.feature_dir,
        MISSION_SLUG,
        "WP01",
        "doing",
        ActorValue::identity("claude"),
    );
    request.repo_root = Some(mission.repo_root.clone());
    let outcome = emit_status_transition(&request).expect("alias collapse");
    assert!(!outcome.persisted);
    assert_eq!(outcome.event.to_lane, Lane::InProgress);
    assert_eq!(
        read_text(&mission.feature_dir.join(EVENTS_FILENAME)),
        before,
        "alias collapse must not append a self-transition"
    );
}

#[test]
fn a_forced_review_lane_exit_without_a_note_is_refused() {
    let mission = temp_mission();
    seed_to_in_review(&mission.feature_dir, &mission.repo_root);
    let before = read_text(&mission.feature_dir.join(EVENTS_FILENAME));

    let mut request = TransitionRequest::new(
        &mission.feature_dir,
        MISSION_SLUG,
        "WP01",
        "in_progress",
        ActorValue::identity("operator"),
    );
    request.repo_root = Some(mission.repo_root.clone());
    request.force = true;
    let error = emit_status_transition(&request).expect_err("unnoted forced exit is refused");
    match error {
        MutationError::TransitionRefused { reason } => {
            assert!(reason.contains("cannot leave in_review by force without a note"));
        }
        other => panic!("unexpected error: {other}"),
    }
    assert_eq!(
        read_text(&mission.feature_dir.join(EVENTS_FILENAME)),
        before
    );
}

#[test]
fn a_batch_lands_one_replacement_and_one_snapshot() {
    let mission = temp_mission();
    let mut planned = TransitionRequest::new(
        &mission.feature_dir,
        MISSION_SLUG,
        "WP01",
        "planned",
        ActorValue::identity("claude"),
    );
    planned.repo_root = Some(mission.repo_root.clone());
    planned.at = Some("2026-11-02T10:00:00+00:00".to_owned());
    planned.event_id = Some("01K9M0R3WJ8V6Q1Y4T7B2N5C0A".to_owned());
    let mut claimed = planned.clone();
    claimed.to_lane = "claimed".to_owned();
    claimed.at = Some("2026-11-02T10:00:01+00:00".to_owned());
    claimed.event_id = Some("01K9M0R3WK9W7R2Z5V8C3P6D1B".to_owned());
    let mut started = planned.clone();
    started.to_lane = "in_progress".to_owned();
    started.at = Some("2026-11-02T10:00:02+00:00".to_owned());
    started.event_id = Some("01K9M0R3WMADXS8SA6W9D4Q7E2".to_owned());

    let outcomes = emit_status_transition_batch(&[planned, claimed, started]).expect("batch");
    assert_eq!(outcomes.len(), 3);
    assert!(outcomes.iter().all(|outcome| outcome.persisted));
    let log = read_text(&mission.feature_dir.join(EVENTS_FILENAME));
    assert_eq!(log.lines().count(), 3);
    assert_eq!(
        outcomes[2]
            .snapshot
            .as_ref()
            .expect("snapshot")
            .snapshot
            .work_packages["WP01"]
            .lane(),
        Some(Lane::InProgress)
    );
    let rows: Vec<Value> = log
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let snapshot = reduce_rows(&rows).expect("reduce");
    assert_eq!(
        serde_json::from_str::<Value>(&render_snapshot_json(&snapshot).unwrap()).unwrap()["work_packages"]
            ["WP01"]["lane"],
        json!("in_progress")
    );
}

#[test]
fn the_subtask_gate_blocks_until_the_roster_is_done() {
    let mission = temp_mission_with(&["T001"], &[]);
    seed_to_in_progress(&mission.feature_dir, &mission.repo_root);

    let mut request = TransitionRequest::new(
        &mission.feature_dir,
        MISSION_SLUG,
        "WP01",
        "for_review",
        ActorValue::identity("claude"),
    );
    request.repo_root = Some(mission.repo_root.clone());
    let error = emit_status_transition(&request)
        .expect_err("an unfinished authored subtask must block the review handoff");
    assert!(
        matches!(
            error,
            MutationError::Transition(TransitionError::CompletedSubtasksRequired)
        ),
        "unexpected error: {error}"
    );

    // Record the subtask completion through the annotation channel.
    let annotation = AnnotationRequest {
        feature_dir: mission.feature_dir.clone(),
        wp_id: "WP01".to_owned(),
        actor: ActorValue::identity("claude"),
        delta: json!({"subtasks": {"T001": "done"}}),
        at: Some("2026-11-02T10:00:02+00:00".to_owned()),
        event_id: Some("01K9M0R3WMADXS8SA6W9D4Q7E2".to_owned()),
        repo_root: Some(mission.repo_root.clone()),
        lock_wait: None,
    };
    let outcome = emit_inner_state_changed(&annotation).expect("annotation");
    assert!(outcome.materialize_error.is_none());
    let log = read_text(&mission.feature_dir.join(EVENTS_FILENAME));
    assert!(log.contains("\"kind\": \"annotation\""));

    request.at = Some("2026-11-02T10:00:03+00:00".to_owned());
    request.event_id = Some("01K9M0R3WNBEYT9TB7XAE5R8F3".to_owned());
    let outcome = emit_status_transition(&request).expect("review handoff after completion");
    assert!(outcome.persisted);
    assert_eq!(outcome.event.to_lane, Lane::ForReview);
}

#[test]
fn a_missing_work_package_file_is_a_roster_refusal() {
    let mission = temp_mission_with(&["T001"], &[]);
    fs::remove_dir_all(mission.feature_dir.join("tasks")).unwrap();
    seed_to_in_progress(&mission.feature_dir, &mission.repo_root);

    let mut request = TransitionRequest::new(
        &mission.feature_dir,
        MISSION_SLUG,
        "WP01",
        "for_review",
        ActorValue::identity("claude"),
    );
    request.repo_root = Some(mission.repo_root.clone());
    let error = emit_status_transition(&request).expect_err("a missing roster must fail closed");
    match error {
        MutationError::TransitionRefused { reason } => {
            assert!(reason.contains("Cannot resolve subtask roster for WP01"));
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn an_explicit_event_id_must_be_a_canonical_ulid() {
    let mission = temp_mission();
    let mut request = TransitionRequest::new(
        &mission.feature_dir,
        MISSION_SLUG,
        "WP01",
        "planned",
        ActorValue::identity("claude"),
    );
    request.repo_root = Some(mission.repo_root.clone());
    request.event_id = Some("not-a-ulid".to_owned());
    let error = emit_status_transition(&request).expect_err("invalid ULID must be refused");
    assert!(matches!(error, MutationError::InvalidRow { .. }));
}

#[test]
fn an_empty_annotation_delta_is_refused() {
    let mission = temp_mission();
    let annotation = AnnotationRequest {
        feature_dir: mission.feature_dir.clone(),
        wp_id: "WP01".to_owned(),
        actor: ActorValue::identity("claude"),
        delta: json!({}),
        at: None,
        event_id: None,
        repo_root: Some(mission.repo_root.clone()),
        lock_wait: None,
    };
    let error = emit_inner_state_changed(&annotation).expect_err("empty delta refused");
    assert!(matches!(error, MutationError::InvalidRow { .. }));
}

fn seed(
    feature_dir: &std::path::Path,
    repo_root: &std::path::Path,
    to_lane: &str,
    id: &str,
    at: &str,
) {
    let mut request = TransitionRequest::new(
        feature_dir,
        MISSION_SLUG,
        "WP01",
        to_lane,
        ActorValue::identity("claude"),
    );
    request.repo_root = Some(repo_root.to_path_buf());
    request.event_id = Some(id.to_owned());
    request.at = Some(at.to_owned());
    emit_status_transition(&request).expect("seed emit");
}

fn seed_to_in_progress(feature_dir: &std::path::Path, repo_root: &std::path::Path) {
    seed(
        feature_dir,
        repo_root,
        "planned",
        "01K9M0R3WJ8V6Q1Y4T7B2N5C0A",
        "2026-11-02T10:00:00+00:00",
    );
    seed(
        feature_dir,
        repo_root,
        "claimed",
        "01K9M0R3WK9W7R2Z5V8C3P6D1B",
        "2026-11-02T10:00:01+00:00",
    );
    seed(
        feature_dir,
        repo_root,
        "in_progress",
        "01K9M0R3WMADXS8SA6W9D4Q7E2",
        "2026-11-02T10:00:02+00:00",
    );
}

fn seed_to_in_review(feature_dir: &std::path::Path, repo_root: &std::path::Path) {
    seed_to_in_progress(feature_dir, repo_root);
    seed(
        feature_dir,
        repo_root,
        "for_review",
        "01K9M0R3WNBEYT9TB7XAE5R8F3",
        "2026-11-02T10:00:03+00:00",
    );
    seed(
        feature_dir,
        repo_root,
        "in_review",
        "01K9M0R3WPCFZVAWC8YBF6S9GX",
        "2026-11-02T10:00:04+00:00",
    );
}
