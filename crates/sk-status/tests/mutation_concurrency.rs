//! Concurrency tests: multiple threads and multiple processes writing the same
//! Mission log must not lose rows, and a same-WP race must resolve to exactly
//! one successful transition.

mod mutation_support;

use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Barrier};

use serde_json::Value;
use sk_status::mutation::{ActorValue, TransitionRequest, emit_status_transition};
use sk_status::snapshot::EVENTS_FILENAME;
use sk_status::{Lane, MutationError, TransitionError, reduce_rows};
use tempfile::TempDir;

use mutation_support::{MISSION_SLUG, TempMission, temp_mission};

struct Seeded {
    _temp: TempDir,
    repo_root: std::path::PathBuf,
    feature_dir: std::path::PathBuf,
}

fn seeded(wp_ids: &[&str]) -> Seeded {
    let TempMission {
        _temp,
        repo_root,
        feature_dir,
    } = temp_mission();
    for (index, wp_id) in wp_ids.iter().enumerate() {
        let mut request = TransitionRequest::new(
            &feature_dir,
            MISSION_SLUG,
            *wp_id,
            "planned",
            ActorValue::identity("system"),
        );
        request.repo_root = Some(repo_root.clone());
        request.event_id = Some(seed_id(index));
        request.at = Some(format!("2026-11-02T10:00:0{}+00:00", index));
        emit_status_transition(&request).expect("seed planned");
    }
    Seeded {
        _temp,
        repo_root,
        feature_dir,
    }
}

fn seed_id(index: usize) -> String {
    let suffixes = [
        "01K9M0R3WJ8V6Q1Y4T7B2N5C00",
        "01K9M0R3WK9W7R2Z5V8C3P6D10",
        "01K9M0R3WMADXS8SA6W9D4Q7E2",
        "01K9M0R3WNBEYT9TB7XAE5R8F3",
    ];
    suffixes[index % suffixes.len()].to_owned()
}

fn claim_id(index: usize) -> String {
    let suffixes = [
        "01K9M0R3WPCFZVAWC8YBF6S9GX",
        "01K9M0R3WQDGZVBWD9ZCG7T0H5",
        "01K9M0R3WR0Z0W1X2Y3Z4A5B6C",
        "01K9M0R3WS1A1B2C3D4E5F6G7H",
    ];
    suffixes[index % suffixes.len()].to_owned()
}

fn read_rows(feature_dir: &Path) -> Vec<Value> {
    fs::read_to_string(feature_dir.join(EVENTS_FILENAME))
        .expect("read log")
        .lines()
        .map(|line| serde_json::from_str(line).expect("row"))
        .collect()
}

#[test]
fn concurrent_claims_of_distinct_work_packages_do_not_lose_rows() {
    let mission = seeded(&["WP01", "WP02", "WP03", "WP04"]);
    let barrier = Arc::new(Barrier::new(4));
    let mut handles = Vec::new();
    for (index, wp_id) in ["WP01", "WP02", "WP03", "WP04"].into_iter().enumerate() {
        let feature_dir = mission.feature_dir.clone();
        let repo_root = mission.repo_root.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            let mut request = TransitionRequest::new(
                &feature_dir,
                MISSION_SLUG,
                wp_id,
                "claimed",
                ActorValue::identity("claude"),
            );
            request.repo_root = Some(repo_root);
            request.event_id = Some(claim_id(index));
            request.at = Some(format!("2026-11-02T11:00:0{index}+00:00"));
            barrier.wait();
            emit_status_transition(&request)
        }));
    }
    for handle in handles {
        handle
            .join()
            .expect("thread")
            .expect("every concurrent claim must persist");
    }

    let rows = read_rows(&mission.feature_dir);
    assert_eq!(rows.len(), 8, "4 seeds + 4 claims");
    let snapshot = reduce_rows(&rows).expect("reduce");
    for wp_id in ["WP01", "WP02", "WP03", "WP04"] {
        assert_eq!(
            snapshot.work_packages[wp_id].lane(),
            Some(Lane::Claimed),
            "{wp_id} must be claimed"
        );
    }
}

#[test]
fn two_concurrent_claims_of_one_work_package_resolve_to_exactly_one_winner() {
    let mission = seeded(&["WP01"]);
    let barrier = Arc::new(Barrier::new(2));
    let mut handles = Vec::new();
    for (index, id) in [claim_id(0), claim_id(1)].into_iter().enumerate() {
        let feature_dir = mission.feature_dir.clone();
        let repo_root = mission.repo_root.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            let mut request = TransitionRequest::new(
                &feature_dir,
                MISSION_SLUG,
                "WP01",
                "claimed",
                ActorValue::identity("claude"),
            );
            request.repo_root = Some(repo_root);
            request.event_id = Some(id);
            request.at = Some(format!("2026-11-02T11:00:0{index}+00:00"));
            barrier.wait();
            emit_status_transition(&request)
        }));
    }
    let mut successes = 0;
    let mut refusals = 0;
    for handle in handles {
        match handle.join().expect("thread") {
            Ok(_) => successes += 1,
            Err(MutationError::Transition(TransitionError::IllegalEdge { from, to })) => {
                assert_eq!((from, to), (Lane::Claimed, Lane::Claimed));
                refusals += 1;
            }
            Err(other) => panic!("unexpected error: {other}"),
        }
    }
    assert_eq!(successes, 1);
    assert_eq!(refusals, 1);
    let rows = read_rows(&mission.feature_dir);
    assert_eq!(rows.len(), 2, "one seed and one winning claim");
}

#[test]
fn concurrent_processes_do_not_lose_appends() {
    let mission = seeded(&["WP01", "WP02"]);
    let mut children = Vec::new();
    for (index, wp_id) in ["WP01", "WP02"].into_iter().enumerate() {
        children.push(
            Command::new(std::env::current_exe().expect("current exe"))
                .args(["--exact", "child_append_helper", "--ignored", "--nocapture"])
                .env("SK_STATUS_CHILD_FEATURE", &mission.feature_dir)
                .env("SK_STATUS_CHILD_WP", wp_id)
                .env("SK_STATUS_CHILD_EVENT_ID", claim_id(index))
                .env(
                    "SK_STATUS_CHILD_AT",
                    format!("2026-11-02T11:00:0{index}+00:00"),
                )
                .output()
                .expect("spawn child writer"),
        );
    }
    for output in &children {
        assert!(
            output.status.success(),
            "child writer failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let rows = read_rows(&mission.feature_dir);
    assert_eq!(rows.len(), 4, "2 seeds + 2 cross-process claims");
    let snapshot = reduce_rows(&rows).expect("reduce");
    assert_eq!(snapshot.work_packages["WP01"].lane(), Some(Lane::Claimed));
    assert_eq!(snapshot.work_packages["WP02"].lane(), Some(Lane::Claimed));
}

/// Child-process body. Only runs when the parent supplies the environment; the
/// `--ignored` filter keeps it out of the ordinary suite.
#[test]
#[ignore]
fn child_append_helper() {
    let Ok(feature) = std::env::var("SK_STATUS_CHILD_FEATURE") else {
        return;
    };
    let wp_id = std::env::var("SK_STATUS_CHILD_WP").expect("child wp id");
    let event_id = std::env::var("SK_STATUS_CHILD_EVENT_ID").expect("child event id");
    let at = std::env::var("SK_STATUS_CHILD_AT").expect("child timestamp");
    let feature_dir = std::path::PathBuf::from(feature);
    let repo_root = feature_dir
        .parent()
        .and_then(Path::parent)
        .expect("repo root")
        .to_path_buf();

    let mut request = TransitionRequest::new(
        &feature_dir,
        MISSION_SLUG,
        wp_id,
        "claimed",
        ActorValue::identity("child-process"),
    );
    request.repo_root = Some(repo_root);
    request.event_id = Some(event_id);
    request.at = Some(at);
    emit_status_transition(&request).expect("child emit");
}
