//! Lifecycle evidence against real repositories: present, genuinely absent, and
//! failed probes stay distinct.

mod support;

use sk_git::{BranchName, GitRepository, ProtectedBranches};
use sk_mission::MissionTopology;
use sk_workspace::{
    LifecycleEvidence, LifecyclePhase, Probe, ProjectWorkspace, baseline_merge_commit_from_meta,
    derive_lifecycle_phase, probe_target_ref, terminal_completion,
};

use support::{commit_all, init_repo, mission_id, write, write_mission};

#[test]
fn target_ref_probe_distinguishes_present_from_absent() {
    let fixture = init_repo();
    let repository = GitRepository::open(fixture.root()).unwrap();
    support::git(
        fixture.root(),
        &["branch", "kitty/mission-my-mission-01KNXQS9"],
    );
    let present = BranchName::parse("kitty/mission-my-mission-01KNXQS9").unwrap();
    let absent = BranchName::parse("kitty/mission-nope-01KNXQS9").unwrap();
    assert!(matches!(
        probe_target_ref(&repository, &present),
        Probe::Present(_)
    ));
    assert!(probe_target_ref(&repository, &absent).is_absent());
}

#[test]
fn a_broken_repository_is_a_failed_probe_not_absence() {
    let fixture = init_repo();
    let repository = GitRepository::open(fixture.root()).unwrap();
    let branch = BranchName::parse("main").unwrap();
    // Destroy the repository under the already-open handle.
    std::fs::remove_dir_all(fixture.root().join(".git")).unwrap();
    let probe = probe_target_ref(&repository, &branch);
    assert!(probe.is_failed(), "expected a failed probe, got {probe:?}");
    if let Probe::Failed(detail) = probe {
        assert!(!detail.is_empty());
    }
}

#[test]
fn real_evidence_selects_each_phase() {
    let fixture = init_repo();
    let repository = GitRepository::open(fixture.root()).unwrap();
    let target = BranchName::parse("main").unwrap();

    // Absent baseline: pre-consolidation without consulting the target probe.
    let phase = derive_lifecycle_phase(&LifecycleEvidence {
        baseline_merge_commit: Probe::Present("deadbeef".to_owned()),
        target_ref: probe_target_ref(&repository, &target),
        terminal_completion: false,
    })
    .unwrap();
    assert_eq!(phase, LifecyclePhase::Consolidated);

    let missing = BranchName::parse("kitty/mission-my-mission-01KNXQS9").unwrap();
    let published = derive_lifecycle_phase(&LifecycleEvidence {
        baseline_merge_commit: Probe::Present("deadbeef".to_owned()),
        target_ref: probe_target_ref(&repository, &missing),
        terminal_completion: terminal_completion(None, &[("WP01".into(), "done".into())]),
    })
    .unwrap();
    assert_eq!(published, LifecyclePhase::Published);

    let pre = derive_lifecycle_phase(&LifecycleEvidence {
        baseline_merge_commit: Probe::Present("deadbeef".to_owned()),
        target_ref: probe_target_ref(&repository, &missing),
        terminal_completion: false,
    })
    .unwrap();
    assert_eq!(pre, LifecyclePhase::PreConsolidation);
}

#[test]
fn baseline_comes_from_the_manifest_not_from_a_directory_name() {
    let fixture = init_repo();
    let dir = write_mission(
        fixture.root(),
        "my-mission",
        &mission_id(1),
        "lanes",
        "main",
    );
    let workspace = ProjectWorkspace::open(fixture.root()).unwrap();
    let mission = workspace.mission("my-mission").unwrap();
    assert!(baseline_merge_commit_from_meta(mission.mission().meta()).is_absent());

    write(
        &dir.join("meta.json"),
        &serde_json::json!({
            "mission_id": mission_id(1),
            "mission_slug": "my-mission",
            "topology": "lanes",
            "target_branch": "main",
            "baseline_merge_commit": "0123456789abcdef",
        })
        .to_string(),
    );
    let mission = workspace.mission("my-mission").unwrap();
    assert_eq!(
        baseline_merge_commit_from_meta(mission.mission().meta()),
        Probe::Present("0123456789abcdef".to_owned())
    );
    assert_eq!(mission.topology(), MissionTopology::Lanes);
}

#[test]
fn missing_manifest_and_unstored_topology_fail_closed() {
    let fixture = init_repo();
    write(
        &fixture.root().join("kitty-specs/ghost/notes.md"),
        "no manifest",
    );
    let workspace = ProjectWorkspace::open(fixture.root()).unwrap();
    // Listing skips a directory without a manifest, so selection reports the
    // selector miss rather than inventing a mission.
    assert!(workspace.mission("ghost").is_err());

    // A mission with a manifest but no stored topology resolves identity and
    // then refuses placement: it never guesses a default shape.
    let dir = write_mission(
        fixture.root(),
        "no-topology",
        &mission_id(2),
        "lanes",
        "main",
    );
    write(
        &dir.join("meta.json"),
        &serde_json::json!({
            "mission_id": mission_id(2),
            "mission_slug": "no-topology",
            "target_branch": "main",
        })
        .to_string(),
    );
    commit_all(fixture.root(), "missions");
    let workspace = ProjectWorkspace::open(fixture.root()).unwrap();
    let error = workspace.mission("no-topology").unwrap_err();
    assert!(
        matches!(
            error,
            sk_workspace::WorkspaceError::TopologyUnresolved { .. }
        ),
        "unexpected error: {error}"
    );
}

#[test]
fn workspace_answers_are_cwd_invariant_across_worktrees() {
    let fixture = init_repo();
    write_mission(
        fixture.root(),
        "my-mission",
        &mission_id(3),
        "lanes",
        "main",
    );
    commit_all(fixture.root(), "mission");
    let branch = "kitty/mission-my-mission-01KNXQS9";
    support::git(fixture.root(), &["branch", branch]);
    let worktree = fixture.path(".worktrees/my-mission-lane-a");
    support::git(
        fixture.root(),
        &["worktree", "add", worktree.to_str().unwrap(), branch],
    );

    let from_root = ProjectWorkspace::open(fixture.root()).unwrap();
    let from_lane = ProjectWorkspace::open(&worktree).unwrap();
    assert_eq!(from_root.primary_root(), from_lane.primary_root());
    assert_eq!(
        from_root.mission("my-mission").unwrap().mission_dir(),
        from_lane.mission("my-mission").unwrap().mission_dir()
    );

    let protected = ProtectedBranches::parse(["main"]).unwrap();
    let write_branch = from_lane
        .mission("my-mission")
        .unwrap()
        .write_branch(&protected)
        .unwrap();
    assert_eq!(write_branch.as_str(), "main");
}

#[test]
fn protected_target_never_becomes_the_single_branch_write_branch() {
    let fixture = init_repo();
    let dir = write_mission(
        fixture.root(),
        "single",
        &mission_id(4),
        "single_branch",
        "main",
    );
    // No mission_branch recorded: writing the protected target must refuse.
    let workspace = ProjectWorkspace::open(fixture.root()).unwrap();
    let mission = workspace.mission("single").unwrap();
    let protected = ProtectedBranches::parse(["main"]).unwrap();
    let error = mission.write_branch(&protected).unwrap_err();
    assert!(matches!(
        error,
        sk_workspace::WorkspaceError::OwnedRefused {
            code: "OWNED_BRANCH_REFUSED",
            ..
        }
    ));

    // With the minted mission branch recorded, that branch is the write branch.
    write(
        &dir.join("meta.json"),
        &serde_json::json!({
            "mission_id": mission_id(4),
            "mission_slug": "single",
            "topology": "single_branch",
            "target_branch": "main",
            "mission_branch": "kitty/mission-single-01KNXQS9",
        })
        .to_string(),
    );
    let mission = workspace.mission("single").unwrap();
    assert_eq!(
        mission.write_branch(&protected).unwrap().as_str(),
        "kitty/mission-single-01KNXQS9"
    );
}
