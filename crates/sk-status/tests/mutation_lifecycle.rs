//! Lifecycle envelope tests: byte parity with the pinned producer, dedupe,
//! the closed event-type set, and the built-in `ProjectInitialized` builder.

mod mutation_support;

use std::collections::BTreeMap;
use std::fs;

use serde_json::Value;
use sk_status::mutation::{
    LifecycleRequest, LifecycleSkip, ProjectInitializedRequest, emit_project_initialized,
    has_lifecycle_event, persist_lifecycle_event_local, project_event_log_path,
};

use mutation_support::{MISSION_SLUG, expected, fixture, read_json, read_text};

fn request_from_fixture(kind: &str, log_path: std::path::PathBuf) -> LifecycleRequest {
    let raw = read_json(&fixture(kind).join("request.json"));
    let dedup_keys: BTreeMap<String, Value> = raw["dedup_keys"]
        .as_object()
        .expect("dedup keys")
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let mut request = LifecycleRequest::new(
        log_path,
        raw["event_type"].as_str().expect("event_type"),
        raw["payload"].clone(),
        raw["aggregate_id"].as_str().expect("aggregate_id"),
        raw["aggregate_type"].as_str().expect("aggregate_type"),
    );
    request.project_uuid = raw["project_uuid"].as_str().map(str::to_owned);
    request.project_slug = raw["project_slug"].as_str().map(str::to_owned);
    request.dedup_keys = dedup_keys;
    request.event_id = raw["event_id"].as_str().map(str::to_owned);
    request.timestamp = raw["timestamp"].as_str().map(str::to_owned);
    request
}

#[test]
fn the_project_envelope_matches_the_pinned_producer_and_is_idempotent() {
    let temp = tempfile::tempdir().unwrap();
    let repo_root = temp.path().join("repo");
    fs::create_dir_all(&repo_root).unwrap();
    let log_path = project_event_log_path(&repo_root);
    let mut request = request_from_fixture("lifecycle_project", log_path.clone());
    request.repo_root = Some(repo_root.clone());
    request.mission_slug = None;

    let outcome = persist_lifecycle_event_local(&request).expect("append");
    assert_eq!(outcome.skip, None);
    assert_eq!(outcome.rows_appended, 1);
    assert!(
        outcome.lock_path.is_some(),
        "the project lock family is used"
    );
    assert_eq!(
        read_text(&log_path),
        read_text(&expected("lifecycle_project", "canonical-events.jsonl"))
    );

    let before = read_text(&log_path);
    let again = persist_lifecycle_event_local(&request).expect("second append");
    assert_eq!(again.skip, Some(LifecycleSkip::AlreadyPresent));
    assert_eq!(again.rows_appended, 0);
    assert_eq!(read_text(&log_path), before, "dedupe appends nothing");
}

#[test]
fn the_project_initialized_builder_matches_the_pinned_payload() {
    let temp = tempfile::tempdir().unwrap();
    let repo_root = temp.path().join("repo");
    fs::create_dir_all(&repo_root).unwrap();
    // The pinned payload model normalises a datetime to `...Z`; the builder
    // passes the caller's timestamp through unchanged (recorded as a gap), so
    // feed the normalised form to compare the envelope and encoding.
    let outcome = emit_project_initialized(
        &repo_root,
        ProjectInitializedRequest {
            project_uuid: "1f8bca14-6f1b-4a1c-9a6c-1d6f0f0b2c3d",
            project_slug: Some("mutation-fixture"),
            actor: "cli",
            runtime_version: Some("4.1.0"),
            initialized_at: Some("2026-11-02T09:00:00Z"),
            event_id: Some("01K9M0R3WJ8V6Q1Y4T7B2N5C0A"),
            timestamp: Some("2026-11-02T09:00:00+00:00"),
        },
    )
    .expect("emit");
    assert_eq!(outcome.rows_appended, 1);
    assert_eq!(
        read_text(&project_event_log_path(&repo_root)),
        read_text(&expected("lifecycle_project", "canonical-events.jsonl"))
    );
}

#[test]
fn the_mission_envelope_matches_the_pinned_producer() {
    let temp = tempfile::tempdir().unwrap();
    let repo_root = temp.path().to_path_buf();
    let feature_dir = repo_root.join("kitty-specs").join(MISSION_SLUG);
    fs::create_dir_all(&feature_dir).unwrap();
    let log_path = feature_dir.join("status.events.jsonl");
    let mut request = request_from_fixture("lifecycle_mission", log_path.clone());
    request.repo_root = Some(repo_root.clone());
    request.mission_slug = Some(MISSION_SLUG.to_owned());

    let outcome = persist_lifecycle_event_local(&request).expect("append");
    assert_eq!(outcome.rows_appended, 1);
    assert!(outcome.lock_path.as_deref().is_some_and(|path| {
        path.to_string_lossy()
            .contains("034-mutation-fixture.status.lock")
    }));
    assert_eq!(
        read_text(&log_path),
        read_text(&expected("lifecycle_mission", "status.events.jsonl"))
    );
    assert!(has_lifecycle_event(&log_path, "WPCreated", &request.dedup_keys).unwrap());
}

#[test]
fn an_unknown_event_type_is_skipped_without_writing() {
    let temp = tempfile::tempdir().unwrap();
    let log_path = temp.path().join("canonical-events.jsonl");
    let mut request = LifecycleRequest::new(
        &log_path,
        "NotALifecycleEvent",
        serde_json::json!({"x": 1}),
        "agg",
        "Mission",
    );
    request.repo_root = Some(temp.path().to_path_buf());
    let outcome = persist_lifecycle_event_local(&request).expect("skip");
    assert_eq!(outcome.skip, Some(LifecycleSkip::UnknownType));
    assert!(!log_path.exists());
}

#[test]
fn artifact_path_dedupe_compares_file_names() {
    let temp = tempfile::tempdir().unwrap();
    let repo_root = temp.path().join("repo");
    let feature_dir = repo_root.join("kitty-specs").join(MISSION_SLUG);
    fs::create_dir_all(&feature_dir).unwrap();
    let log_path = feature_dir.join("status.events.jsonl");

    let mut first = LifecycleRequest::new(
        &log_path,
        "SpecifyCompleted",
        serde_json::json!({
            "mission_slug": MISSION_SLUG,
            "artifact_path": "/one/spec.md",
            "actor": "cli",
            "at": "2026-11-02T09:00:00+00:00",
        }),
        MISSION_SLUG,
        "Mission",
    );
    first.repo_root = Some(repo_root.clone());
    first.mission_slug = Some(MISSION_SLUG.to_owned());
    first.event_id = Some("01K9M0R3WJ8V6Q1Y4T7B2N5C0A".to_owned());
    first.timestamp = Some("2026-11-02T09:00:00+00:00".to_owned());
    first.dedup_keys = BTreeMap::from([
        (
            "mission_slug".to_owned(),
            Value::String(MISSION_SLUG.to_owned()),
        ),
        (
            "artifact_path".to_owned(),
            Value::String("/one/spec.md".to_owned()),
        ),
    ]);
    persist_lifecycle_event_local(&first).expect("first");

    let mut second = first.clone();
    second.event_id = Some("01K9M0R3WK9W7R2Z5V8C3P6D1B".to_owned());
    second.payload = serde_json::json!({
        "mission_slug": MISSION_SLUG,
        "artifact_path": "/two/spec.md",
        "actor": "cli",
        "at": "2026-11-02T09:00:01+00:00",
    });
    second.dedup_keys.insert(
        "artifact_path".to_owned(),
        Value::String("/two/spec.md".to_owned()),
    );
    let outcome = persist_lifecycle_event_local(&second).expect("second");
    assert_eq!(outcome.skip, Some(LifecycleSkip::AlreadyPresent));
    assert_eq!(read_text(&log_path).lines().count(), 1);
}

#[test]
fn a_non_object_payload_is_refused() {
    let temp = tempfile::tempdir().unwrap();
    let log_path = temp.path().join("canonical-events.jsonl");
    let mut request = LifecycleRequest::new(
        &log_path,
        "MissionCreated",
        serde_json::json!(["not", "an", "object"]),
        "agg",
        "Mission",
    );
    request.repo_root = Some(temp.path().to_path_buf());
    let error = persist_lifecycle_event_local(&request).expect_err("payload refused");
    assert!(matches!(
        error,
        sk_status::mutation::MutationError::InvalidLifecyclePayload { .. }
    ));
    assert!(!log_path.exists());
}
