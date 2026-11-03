//! Writer contract tests for the semantics added on top of the differential
//! fixtures: typed claim annotations persisted with the transition, lane-head
//! stamping, dependency-readiness gating, the phase-1 lane mirror, and the
//! lifecycle payload validation seam.
//!
//! Every case uses a temporary repository root; nothing reads or writes the
//! checkout this test binary lives in.

mod mutation_support;

use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use sk_status::mutation::{
    AnnotationRequest, InnerStateDelta, LifecycleRequest, LifecycleSkip, MutationError,
    SubtaskValue, TransitionRequest, emit_inner_state_changed, emit_status_transition,
    persist_lifecycle_event_local,
};
use sk_status::{Lane, MirrorOutcome};

use mutation_support::{MISSION_SLUG, TempMission, read_text, temp_mission};

fn actor() -> sk_status::mutation::ActorValue {
    sk_status::mutation::ActorValue::identity("claude")
}

fn claim_request(mission: &TempMission, wp_id: &str) -> TransitionRequest {
    let mut request = TransitionRequest::new(
        &mission.feature_dir,
        MISSION_SLUG,
        wp_id,
        "claimed",
        actor(),
    );
    request.repo_root = Some(mission.repo_root.clone());
    request.at = Some("2026-11-02T10:00:00Z".to_owned());
    request
}

fn rows(mission: &TempMission) -> Vec<Value> {
    read_text(&sk_status::mutation::events_path(&mission.feature_dir))
        .lines()
        .map(|line| serde_json::from_str(line).expect("json row"))
        .collect()
}

fn write_wp(dir: &Path, wp_id: &str, body: &str) {
    fs::create_dir_all(dir.join("tasks")).unwrap();
    fs::write(dir.join("tasks").join(format!("{wp_id}.md")), body).unwrap();
}

#[test]
fn the_claim_annotation_lands_in_the_same_replacement_as_the_transition() {
    let mission = temp_mission();
    let mut request = claim_request(&mission, "WP01");
    request.lane_head = Some("0123456789abcdef0123456789abcdef01234567".to_owned());
    request.annotation_delta = Some(InnerStateDelta::new().with_claim(
        "claude",
        Some(4242),
        Some("2026-11-02T09:59:00Z".to_owned()),
    ));

    // A WP is seeded `genesis -> planned` before its first claim.
    let mut seed = claim_request(&mission, "WP01");
    seed.to_lane = "planned".to_owned();
    seed.at = Some("2026-11-02T09:00:00Z".to_owned());
    emit_status_transition(&seed).expect("genesis -> planned");

    let outcome = emit_status_transition(&request).expect("claim");
    assert!(outcome.persisted);
    let annotation = outcome.annotation.as_ref().expect("annotation persisted");
    assert!(matches!(outcome.mirror, Some(MirrorOutcome::Disabled)));

    let rows = rows(&mission);
    assert_eq!(rows.len().min(3), 2 + 1, "seeded row plus one replacement");
    let rows = &rows[1..];
    assert_eq!(rows.len(), 2, "one replacement carries both rows");
    assert_eq!(rows[0]["event_id"], json!(outcome.event.event_id));
    assert_eq!(rows[0]["to_lane"], json!("claimed"));
    assert_eq!(
        rows[0]["policy_metadata"]["lane_head"],
        json!("0123456789abcdef0123456789abcdef01234567")
    );
    assert_eq!(rows[1]["kind"], json!("annotation"));
    assert_eq!(rows[1]["event_id"], json!(annotation.event_id));
    assert_eq!(rows[1]["wp_id"], json!("WP01"));
    assert_eq!(rows[1]["at"], json!("2026-11-02T10:00:00Z"));
    assert_eq!(
        rows[1]["delta"],
        json!({"agent": "claude", "shell_pid": 4242, "shell_pid_created_at": "2026-11-02T09:59:00Z"})
    );
}

#[test]
fn persisted_annotation_deltas_are_normalised_like_the_typed_model() {
    let mission = temp_mission();
    let mut request = AnnotationRequest {
        feature_dir: mission.feature_dir.clone(),
        wp_id: "WP01".to_owned(),
        actor: actor(),
        delta: json!({"agent": "", "subtasks": {"T001": "not_applicable", "T002": "done"}}),
        at: Some("2026-11-02T10:00:00Z".to_owned()),
        event_id: None,
        repo_root: Some(mission.repo_root.clone()),
        lock_wait: None,
    };
    emit_inner_state_changed(&request).expect("annotation");
    let first_pass = rows(&mission);
    assert_eq!(first_pass.len(), 1);
    assert_eq!(
        first_pass[0]["delta"],
        json!({"subtasks": {"T001": "skipped", "T002": "done"}}),
        "a blank scalar is dropped and the `not_applicable` alias folds to `skipped`"
    );

    // An unknown key and a non-string scalar are refusals at the write boundary.
    request.delta = json!({"hostname": "builder"});
    assert!(matches!(
        emit_inner_state_changed(&request),
        Err(MutationError::InvalidRow { .. })
    ));
    request.delta = json!({"agent": 5});
    assert!(matches!(
        emit_inner_state_changed(&request),
        Err(MutationError::InvalidRow { .. })
    ));
    assert_eq!(rows(&mission).len(), 1, "refusals write nothing");
}

#[test]
fn annotation_wp_ids_are_matched_against_the_canonical_pattern_without_folding() {
    let mission = temp_mission();
    let request = AnnotationRequest {
        feature_dir: mission.feature_dir.clone(),
        wp_id: "wp01".to_owned(),
        actor: actor(),
        delta: json!({"note": "hello"}),
        at: None,
        event_id: None,
        repo_root: Some(mission.repo_root.clone()),
        lock_wait: None,
    };
    let error = emit_inner_state_changed(&request).expect_err("lowercase is refused");
    assert!(
        error.to_string().contains("annotate() refuses wp_id"),
        "unexpected error: {error}"
    );
    assert!(
        !sk_status::mutation::events_path(&mission.feature_dir).exists(),
        "nothing was created"
    );
}

#[test]
fn an_empty_claim_annotation_refuses_the_whole_transition() {
    let mission = temp_mission();
    let mut seed = claim_request(&mission, "WP01");
    seed.to_lane = "planned".to_owned();
    emit_status_transition(&seed).expect("genesis -> planned");
    let mut request = claim_request(&mission, "WP01");
    request.annotation_delta = Some(InnerStateDelta::new());
    let error = emit_status_transition(&request).expect_err("empty delta");
    assert!(error.to_string().contains("empty delta"), "got {error}");
    assert_eq!(rows(&mission).len(), 1, "only the seed row exists");
    assert_eq!(rows(&mission)[0]["to_lane"], json!("planned"));
    assert!(
        rows(&mission).iter().all(|row| row.get("kind").is_none()),
        "no annotation row was appended"
    );
}

#[test]
fn dependency_readiness_gates_the_entry_edge_from_the_declared_frontmatter() {
    let mission = temp_mission();
    write_wp(
        &mission.feature_dir,
        "WP02",
        "---\nwork_package_id: WP02\ntitle: Dependent\ndependencies: [\"WP01\"]\nsubtasks: []\n---\n",
    );

    // Seed WP02 as planned so the guarded `planned -> claimed` edge is the one
    // being exercised.
    let mut seed = TransitionRequest::new(
        &mission.feature_dir,
        MISSION_SLUG,
        "WP02",
        "planned",
        actor(),
    );
    seed.repo_root = Some(mission.repo_root.clone());
    emit_status_transition(&seed).expect("genesis -> planned");

    let claim = claim_request(&mission, "WP02");
    let error = emit_status_transition(&claim).expect_err("WP01 is not ready");
    assert!(
        error.to_string().contains("unsatisfied dependencies"),
        "got {error}"
    );

    // Approve WP01: `approved` is an acceptable ending for a same-mission chain.
    let mut plan_wp01 = claim_request(&mission, "WP01");
    plan_wp01.to_lane = "planned".to_owned();
    emit_status_transition(&plan_wp01).expect("WP01 planned");
    for lane in ["claimed", "in_progress", "approved"] {
        let mut hop = claim_request(&mission, "WP01");
        hop.to_lane = lane.to_owned();
        hop.workspace_context = Some("worktree:/tmp".to_owned());
        hop.evidence = (lane == "approved").then(
            || json!({"review": {"reviewer": "alice", "verdict": "approved", "reference": "PR#1"}}),
        );
        emit_status_transition(&hop).unwrap_or_else(|error| panic!("WP01 -> {lane}: {error}"));
    }

    emit_status_transition(&claim).expect("WP01 approved satisfies the dependency");
    assert_eq!(
        rows(&mission)
            .iter()
            .filter(|row| row["wp_id"] == json!("WP02") && row["to_lane"] == json!("claimed"))
            .count(),
        1
    );
}

#[test]
fn an_operator_cancel_satisfies_a_dependency_but_a_synthetic_one_does_not() {
    let mission = temp_mission();
    write_wp(
        &mission.feature_dir,
        "WP02",
        "---\nwork_package_id: WP02\ntitle: Dependent\ndependencies:\n- WP01\nsubtasks: []\n---\n",
    );
    let mut seed = TransitionRequest::new(
        &mission.feature_dir,
        MISSION_SLUG,
        "WP02",
        "planned",
        actor(),
    );
    seed.repo_root = Some(mission.repo_root.clone());
    emit_status_transition(&seed).expect("genesis -> planned");

    let mut cancel = claim_request(&mission, "WP01");
    cancel.to_lane = "canceled".to_owned();
    cancel.at = Some("2026-11-02T10:01:00Z".to_owned());
    emit_status_transition(&cancel).expect("synthetic cancel");

    let claim = claim_request(&mission, "WP02");
    assert!(
        emit_status_transition(&claim).is_err(),
        "a synthetic cancellation is not an acceptable ending"
    );

    // Re-cancel with operator provenance; a canceled WP is terminal, so the
    // second cancellation is forced with a note.
    let mut operator_cancel = claim_request(&mission, "WP01");
    operator_cancel.to_lane = "canceled".to_owned();
    operator_cancel.force = true;
    operator_cancel.reason = Some("operator withdrew WP01".to_owned());
    operator_cancel.reason_source = Some("operator".to_owned());
    operator_cancel.at = Some("2026-11-02T10:02:00Z".to_owned());
    emit_status_transition(&operator_cancel).expect("operator cancel");

    emit_status_transition(&claim).expect("operator provenance satisfies the dependency");
}

#[test]
fn the_phase1_lane_mirror_updates_the_authored_field_only() {
    let mission = temp_mission();
    // Add `lane` plus `status_phase: 1` to the temp mission.
    let meta = mission.feature_dir.join("meta.json");
    let mut value: Value = serde_json::from_str(&read_text(&meta)).unwrap();
    value["status_phase"] = json!(1);
    fs::write(&meta, serde_json::to_string_pretty(&value).unwrap()).unwrap();
    let wp = mission.feature_dir.join("tasks/WP01.md");
    let authored = read_text(&wp).replacen(
        "title: Mutation fixture work package",
        "title: Mutation fixture work package\nlane: planned  # phase-1",
        1,
    );
    fs::write(&wp, &authored).unwrap();

    let mut seed = claim_request(&mission, "WP01");
    seed.to_lane = "planned".to_owned();
    emit_status_transition(&seed).expect("genesis -> planned");
    let outcome = emit_status_transition(&claim_request(&mission, "WP01")).expect("claim");
    assert!(
        matches!(
            outcome.mirror,
            Some(MirrorOutcome::Mirrored {
                lane: Lane::Claimed,
                ..
            })
        ),
        "unexpected mirror outcome: {:?}",
        outcome.mirror
    );
    let mirrored = read_text(&wp);
    assert!(
        mirrored.contains("lane: claimed  # phase-1"),
        "the value changed and the comment stayed: {mirrored}"
    );
    assert!(
        mirrored.contains("title: Mutation fixture work package"),
        "the rest of the frontmatter is byte-preserved"
    );
    assert!(mirrored.contains("dependencies: []"));
}

#[test]
fn arbitrary_lifecycle_payloads_are_refused_before_the_envelope_is_built() {
    let temp = tempfile::tempdir().unwrap();
    let log_path = temp.path().join(".kittify/canonical-events.jsonl");
    let mut request = LifecycleRequest::new(
        &log_path,
        "ProjectInitialized",
        json!({"project_uuid": "u", "actor": "a", "hostname": "builder"}),
        "u",
        "Project",
    );
    request.repo_root = Some(temp.path().to_path_buf());
    let error = persist_lifecycle_event_local(&request).expect_err("arbitrary payload refused");
    assert!(
        error.to_string().contains("unknown payload key"),
        "got {error}"
    );
    assert!(!log_path.exists(), "nothing was written");

    // A known event type with a valid payload still persists, canonicalised.
    request.payload = json!({
        "project_uuid": "1f8bca14-6f1b-4a1c-9a6c-1d6f0f0b2c3d",
        "actor": "cli",
        "initialized_at": "2026-11-02T09:00:00+00:00",
    });
    let outcome = persist_lifecycle_event_local(&request).expect("valid payload");
    assert_eq!(outcome.skip, None);
    assert_eq!(outcome.rows_appended, 1);
    let row: Value = serde_json::from_str(read_text(&log_path).trim()).unwrap();
    assert_eq!(
        row["payload"]["initialized_at"],
        json!("2026-11-02T09:00:00Z")
    );
    assert_eq!(row["payload"]["project_slug"], Value::Null);

    // An unknown event type is still a no-op skip, never a write.
    let unknown = LifecycleRequest::new(&log_path, "NotAnEvent", json!({}), "u", "Project");
    let skipped = persist_lifecycle_event_local(&unknown).expect("skip");
    assert_eq!(skipped.skip, Some(LifecycleSkip::UnknownType));
}

#[test]
fn subtask_delta_values_round_trip_through_the_public_types() {
    let delta = InnerStateDelta::new()
        .with_subtask("T001", SubtaskValue::Skipped)
        .with_subtask("T002", SubtaskValue::Lane(Lane::Done));
    let encoded = delta.to_json_value();
    assert_eq!(encoded["subtasks"]["T001"], json!("skipped"));
    assert_eq!(
        SubtaskValue::parse("not_applicable").unwrap(),
        SubtaskValue::Skipped
    );
    assert_eq!(
        SubtaskValue::parse("approved").unwrap(),
        SubtaskValue::Lane(Lane::Approved)
    );
    assert!(SubtaskValue::parse("done ").is_err());
    assert_eq!(MISSION_SLUG, "034-mutation-fixture");
}
