//! Branch/lane tip and occupancy facts against real repositories.

mod support;

use sk_git::{GitRepository, Oid};
use sk_workspace::{
    LaneTipOutcome, LanesStore, ProjectWorkspace, WorkspaceError, mission_branch_tips,
    mission_occupancy, read_lane_tip, record_lane_tip, record_lane_tip_at,
};

use support::{
    branch_tip, commit_all, create_branch, git, init_repo, mission_id, write, write_mission_with,
};

fn manifest_json(
    slug: &str,
    id: &str,
    mission_branch: &str,
    target: &str,
    planning_commit: Option<&str>,
    lanes: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "version": 1,
        "mission_slug": slug,
        "mission_id": id,
        "mission_branch": mission_branch,
        "target_branch": target,
        "lanes": lanes,
        "computed_at": "2026-11-02T00:00:00Z",
        "computed_from": "fixture",
        "planning_artifact_wps": [],
        "planning_commit_sha": planning_commit,
    })
}

fn lane(id: &str, wps: &[&str], depends: &[&str], group: u32) -> serde_json::Value {
    serde_json::json!({
        "lane_id": id,
        "wp_ids": wps,
        "write_scope": [],
        "predicted_surfaces": [],
        "depends_on_lanes": depends,
        "parallel_group": group,
    })
}

#[test]
fn tips_come_from_the_repository_not_from_the_current_checkout() {
    let fixture = init_repo();
    let id = mission_id(1);
    let slug = "demo";
    let mission_dir = write_mission_with(
        fixture.root(),
        slug,
        &id,
        "lanes_with_coord",
        "main",
        serde_json::json!({"coordination_branch": format!("kitty/mission-{slug}")}),
    );
    let planning = branch_tip(fixture.root(), "main");
    let coord_tip = create_branch(fixture.root(), &format!("kitty/mission-{slug}"));
    let lane_branch = format!("kitty/mission-{slug}-lane-a");
    let lane_tip = create_branch(fixture.root(), &lane_branch);
    write(
        &mission_dir.join("lanes.json"),
        &format!(
            "{}\n",
            manifest_json(
                slug,
                &id,
                &format!("kitty/mission-{slug}"),
                "main",
                Some(&planning),
                serde_json::json!([
                    lane("lane-planning", &["WP01"], &[], 0),
                    lane("lane-a", &["WP02"], &["lane-planning"], 1),
                ]),
            )
        ),
    );
    let store = LanesStore::new(&mission_dir);
    let manifest = store.read().unwrap().unwrap();
    let workspace = ProjectWorkspace::open(fixture.root())
        .unwrap()
        .mission(slug)
        .unwrap();
    let tips = mission_branch_tips(&workspace, Some(&manifest)).unwrap();
    assert_eq!(tips.target.tip.as_ref().unwrap().to_string(), planning);
    assert_eq!(
        tips.coordination
            .as_ref()
            .unwrap()
            .tip
            .as_ref()
            .unwrap()
            .to_string(),
        coord_tip
    );
    assert_eq!(tips.lanes.len(), 1);
    assert_eq!(tips.lanes[0].branch, lane_branch);
    assert_eq!(tips.lanes[0].tip.as_ref().unwrap().to_string(), lane_tip);
    assert_eq!(
        tips.planning_commit.as_ref().unwrap().to_string(),
        planning,
        "the recorded planning commit resolves to an object id"
    );
    assert!(tips.destroyed_lanes().is_empty());
}

#[test]
fn a_destroyed_branch_with_a_recorded_tip_is_never_empty() {
    let fixture = init_repo();
    let repository = GitRepository::open(fixture.root()).unwrap();
    let branch = "kitty/mission-demo-lane-a";
    let tip = create_branch(fixture.root(), branch);
    let oid = Oid::parse(&tip).unwrap();
    record_lane_tip_at(&repository, branch, &oid).unwrap();
    support::git(fixture.root(), &["branch", "-D", branch]);
    let id = mission_id(2);
    let slug = "demo";
    let mission_dir = write_mission_with(
        fixture.root(),
        slug,
        &id,
        "lanes",
        "main",
        serde_json::json!({}),
    );
    write(
        &mission_dir.join("lanes.json"),
        &format!(
            "{}\n",
            manifest_json(
                slug,
                &id,
                "kitty/mission-demo",
                "main",
                None,
                serde_json::json!([lane("lane-a", &["WP01"], &[], 0)]),
            )
        ),
    );
    let manifest = LanesStore::new(&mission_dir).read().unwrap().unwrap();
    let workspace = ProjectWorkspace::open(fixture.root())
        .unwrap()
        .mission(slug)
        .unwrap();
    let tips = mission_branch_tips(&workspace, Some(&manifest)).unwrap();
    assert_eq!(tips.destroyed_lanes().len(), 1);
    assert!(tips.lanes[0].is_destroyed_but_recorded());
    assert!(tips.lanes[0].is_stale());
}

#[test]
fn lane_tip_recording_is_monotonic_and_never_rewrites_history() {
    let fixture = init_repo();
    let repository = GitRepository::open(fixture.root()).unwrap();
    let branch = "kitty/mission-demo-lane-a";
    let first = Oid::parse(&create_branch(fixture.root(), branch)).unwrap();

    let created = record_lane_tip_at(&repository, branch, &first).unwrap();
    assert_eq!(created.outcome, LaneTipOutcome::Created);
    assert_eq!(created.previous, None);

    let unchanged = record_lane_tip(&repository, branch).unwrap().unwrap();
    assert_eq!(unchanged.outcome, LaneTipOutcome::Unchanged);

    // Advance the branch and record again.
    support::git(fixture.root(), &["checkout", branch]);
    write(&fixture.path("note.txt"), "second\n");
    commit_all(fixture.root(), "advance");
    let second = Oid::parse(&branch_tip(fixture.root(), branch)).unwrap();
    let advanced = record_lane_tip_at(&repository, branch, &second).unwrap();
    assert_eq!(advanced.outcome, LaneTipOutcome::Advanced);
    assert_eq!(advanced.previous.as_ref(), Some(&first));

    // Recording the older tip again keeps the newer record.
    let behind = record_lane_tip_at(&repository, branch, &first).unwrap();
    assert_eq!(behind.outcome, LaneTipOutcome::KeptNewerRecord);
    assert_eq!(read_lane_tip(&repository, branch).unwrap(), Some(second));

    // Point the branch at an unrelated commit: recording must refuse instead of
    // overwriting the only record of the earlier work.
    support::git(fixture.root(), &["checkout", "main"]);
    support::git(fixture.root(), &["checkout", "-b", "unrelated"]);
    write(&fixture.path("unrelated.txt"), "unrelated\n");
    commit_all(fixture.root(), "unrelated");
    let unrelated = Oid::parse(&branch_tip(fixture.root(), "unrelated")).unwrap();
    support::git(fixture.root(), &["checkout", "main"]);
    support::git(
        fixture.root(),
        &["branch", "-f", branch, unrelated.as_str()],
    );
    let error = record_lane_tip_at(&repository, branch, &unrelated).unwrap_err();
    assert!(
        matches!(
            error,
            WorkspaceError::CoordRefused {
                code: "LANE_TIP_HISTORY_REWRITTEN",
                ..
            }
        ),
        "{error}"
    );
}

#[test]
fn occupancy_reports_registered_checkouts_and_stray_husks() {
    let fixture = init_repo();
    let id = mission_id(3);
    let slug = "demo";
    let mission_dir = write_mission_with(
        fixture.root(),
        slug,
        &id,
        "lanes",
        "main",
        serde_json::json!({}),
    );
    write(
        &mission_dir.join("lanes.json"),
        &format!(
            "{}\n",
            manifest_json(
                slug,
                &id,
                "kitty/mission-demo",
                "main",
                None,
                serde_json::json!([
                    lane("lane-planning", &["WP01"], &[], 0),
                    lane("lane-a", &["WP02"], &[], 1),
                    lane("lane-b", &["WP03"], &[], 1),
                ]),
            )
        ),
    );
    let manifest = LanesStore::new(&mission_dir).read().unwrap().unwrap();
    let lane_a_branch = format!("kitty/mission-{slug}-lane-a");
    create_branch(fixture.root(), &lane_a_branch);
    let lane_a_path = fixture
        .root()
        .join(".worktrees")
        .join(format!("{slug}-lane-a"));
    support::git(
        fixture.root(),
        &[
            "worktree",
            "add",
            lane_a_path.to_str().unwrap(),
            &lane_a_branch,
        ],
    );
    write(&lane_a_path.join("dirty.txt"), "dirty\n");
    // A husk: a directory at the predicted lane path Git does not register.
    let lane_b_path = fixture
        .root()
        .join(".worktrees")
        .join(format!("{slug}-lane-b"));
    std::fs::create_dir_all(&lane_b_path).unwrap();

    let workspace = ProjectWorkspace::open(fixture.root())
        .unwrap()
        .mission(slug)
        .unwrap();
    let occupancy = mission_occupancy(&workspace, Some(&manifest)).unwrap();
    let lane_a = occupancy.lane("lane-a").unwrap();
    assert_eq!(lane_a.checkouts.len(), 1);
    assert!(!lane_a.is_clean(), "the fixture left an untracked file");
    assert!(lane_a.stray_directory.is_none());
    let lane_b = occupancy.lane("lane-b").unwrap();
    assert!(lane_b.stray_directory.is_some());
    assert!(lane_b.is_occupied());
    assert_eq!(occupancy.repo_root.planning_wps, vec!["WP01".to_owned()]);
    assert_eq!(occupancy.occupied_lanes().len(), 2);
    assert!(occupancy.target.exists());
}

#[test]
fn coordination_occupancy_tracks_the_declared_branch() {
    let fixture = init_repo();
    let id = mission_id(4);
    let slug = "demo";
    let coord_branch = format!("kitty/mission-{slug}");
    write_mission_with(
        fixture.root(),
        slug,
        &id,
        "coord",
        "main",
        serde_json::json!({"coordination_branch": coord_branch}),
    );
    create_branch(fixture.root(), &coord_branch);
    let workspace = ProjectWorkspace::open(fixture.root())
        .unwrap()
        .mission(slug)
        .unwrap();
    let occupancy = mission_occupancy(&workspace, None).unwrap();
    let coordination = occupancy.coordination.as_ref().unwrap();
    assert_eq!(coordination.branch, coord_branch);
    assert!(coordination.tip.is_some());
    assert!(coordination.checkouts.is_empty());
}

#[test]
fn a_clean_lane_is_reported_clean() {
    let fixture = init_repo();
    let repository = GitRepository::open(fixture.root()).unwrap();
    let branch = "kitty/mission-demo-lane-c";
    create_branch(fixture.root(), branch);
    let path = fixture.root().join(".worktrees/lane-c");
    support::git(
        fixture.root(),
        &["worktree", "add", path.to_str().unwrap(), branch],
    );
    let checkout = GitRepository::open(&path).unwrap();
    assert!(checkout.status().unwrap().is_empty());
    assert_eq!(repository.worktrees().unwrap().len(), 2);
    let _ = git(fixture.root(), &["worktree", "list"]);
}
