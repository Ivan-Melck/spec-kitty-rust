//! `lanes.json` persistence and lane-level dependency readiness.

use std::collections::BTreeSet;

use sk_kernel::WpId;
use sk_mission::MissionTopology;
use sk_workspace::{
    LANES_FILE_NAME, LanePlannerInput, LaneReadiness, LanesStore, MissionArtifactKind, WpPlan,
    compute_lanes, lane_dependency_readiness,
};

fn wp(value: &str) -> WpId {
    value.parse().unwrap()
}

fn dependency_manifest() -> sk_workspace::LanesManifest {
    let mut input = LanePlannerInput::new(
        "my-mission".parse().unwrap(),
        MissionTopology::Lanes,
        "main",
    );
    input
        .work_packages
        .insert(wp("WP01"), WpPlan::code(wp("WP01"), ["src/a/**"]));
    input
        .work_packages
        .insert(wp("WP02"), WpPlan::code(wp("WP02"), ["src/b/**"]));
    input.dependencies.insert(wp("WP01"), Vec::new());
    input.dependencies.insert(wp("WP02"), vec![wp("WP01")]);
    compute_lanes(&input).unwrap()
}

#[test]
fn manifest_round_trips_through_the_store() {
    let temp = tempfile::TempDir::new().unwrap();
    let feature_dir = temp.path().join("my-mission");
    std::fs::create_dir_all(&feature_dir).unwrap();
    let store = LanesStore::new(&feature_dir);

    assert!(store.read().unwrap().is_none(), "absent manifest is None");

    let manifest = dependency_manifest();
    let path = store.write(&manifest).unwrap();
    assert_eq!(path.file_name().unwrap(), LANES_FILE_NAME);
    assert!(store.lock_path().exists());

    let decoded = store.read().unwrap().expect("manifest present");
    assert_eq!(decoded, manifest);

    // Wire parity with upstream: the planning commit key is always present
    // (null when absent), and a collapse report carries the derived counts.
    let raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(store.path()).unwrap()).unwrap();
    assert!(raw.get("planning_commit_sha").is_some());
    assert!(raw["planning_commit_sha"].is_null());
    let mut collapsing = LanePlannerInput::new(
        "my-mission".parse().unwrap(),
        MissionTopology::Lanes,
        "main",
    );
    collapsing
        .work_packages
        .insert(wp("WP01"), WpPlan::code(wp("WP01"), ["src/shared/*.rs"]));
    collapsing
        .work_packages
        .insert(wp("WP02"), WpPlan::code(wp("WP02"), ["src/shared/mod.rs"]));
    collapsing.dependencies.insert(wp("WP01"), Vec::new());
    collapsing.dependencies.insert(wp("WP02"), Vec::new());
    let report = compute_lanes(&collapsing).unwrap();
    let report_json = serde_json::to_value(&report).unwrap();
    assert_eq!(report_json["collapse_report"]["total_merges"], 1);
    assert_eq!(
        report_json["collapse_report"]["by_rule"]["write_scope_overlap"],
        1
    );
    assert_eq!(decoded.mission_slug, "my-mission");
    assert_eq!(decoded.lanes.len(), 2);
    assert!(decoded.has_code_lanes());
    assert!(!decoded.is_planning_artifact_only());
}

#[test]
fn corrupt_manifest_fails_closed() {
    let temp = tempfile::TempDir::new().unwrap();
    std::fs::write(temp.path().join(LANES_FILE_NAME), b"{not json").unwrap();
    let store = LanesStore::new(temp.path());
    let error = store.read().unwrap_err();
    assert!(matches!(
        error,
        sk_workspace::WorkspaceError::CorruptLanes { .. }
    ));
}

#[test]
fn lane_readiness_follows_the_lane_dag() {
    let manifest = dependency_manifest();
    let lane_a = manifest
        .lane_for_wp(&wp("WP01"))
        .expect("lane a")
        .lane_id
        .clone();
    let lane_b = manifest
        .lane_for_wp(&wp("WP02"))
        .expect("lane b")
        .lane_id
        .clone();
    assert_ne!(lane_a, lane_b);

    assert_eq!(
        lane_dependency_readiness(&manifest, &lane_a, &BTreeSet::new()),
        LaneReadiness::Ready
    );
    assert_eq!(
        lane_dependency_readiness(&manifest, &lane_b, &BTreeSet::new()),
        LaneReadiness::Blocked {
            lane_id: lane_b.clone(),
            missing: vec![lane_a.clone()],
        }
    );
    let satisfied: BTreeSet<String> = [lane_a].into_iter().collect();
    assert!(lane_dependency_readiness(&manifest, &lane_b, &satisfied).is_ready());
    assert!(matches!(
        lane_dependency_readiness(&manifest, "lane-ghost", &satisfied),
        LaneReadiness::Blocked { .. }
    ));
}

#[test]
fn planning_lane_is_repository_root_and_never_a_worktree() {
    let mut input = LanePlannerInput::new(
        "my-mission".parse().unwrap(),
        MissionTopology::Lanes,
        "main",
    );
    input
        .work_packages
        .insert(wp("WP01"), WpPlan::planning(wp("WP01"), ["kitty-specs/**"]));
    input.dependencies.insert(wp("WP01"), Vec::new());
    let manifest = compute_lanes(&input).unwrap();
    assert!(manifest.is_planning_artifact_only());
    assert!(!manifest.has_code_lanes());
    assert_eq!(
        manifest.planning_artifact_wps,
        vec![wp("WP01")],
        "kind-derived planning view"
    );
    assert_eq!(
        MissionArtifactKind::LaneState.partition(),
        sk_workspace::ArtifactPartition::Primary,
        "lanes.json is primary-partition state"
    );
}
