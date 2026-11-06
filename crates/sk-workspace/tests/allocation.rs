//! Real lane allocation against real temporary repositories.
//!
//! Covers: fresh creation, reuse, dirty reuse refusal, crash recovery, journal
//! reconciliation of interrupted creation, unhonorable bases, protected-branch
//! refusal, stale-plan (moved-ref) refusal, and custom-file retention.

mod support;

use sk_git::{
    BranchAdvanceOptions, BranchName, GitRepository, Oid, ProtectedBranches, StartPoint,
    WorktreeStart,
};
use sk_kernel::WpId;
use sk_mission::MissionTopology;
use sk_workspace::{
    AllocationRoute, LaneAllocator, LanePlannerInput, LanesManifest, RecoveryRepair,
    WorkspaceError, WpPlan, compute_lanes, lane_dependency_readiness,
};

use support::init_repo;

fn wp(value: &str) -> WpId {
    value.parse().unwrap()
}

fn planner() -> LanePlannerInput {
    LanePlannerInput::new(
        "my-mission".parse().unwrap(),
        MissionTopology::Lanes,
        "main",
    )
}

fn manifest_with(wps: &[(&str, &str)]) -> LanesManifest {
    let mut input = planner();
    for (id, scope) in wps {
        let id = wp(id);
        input
            .work_packages
            .insert(id.clone(), WpPlan::code(id.clone(), [*scope]));
        input.dependencies.insert(id, Vec::new());
    }
    compute_lanes(&input).unwrap()
}

fn manifest_with_dependency() -> LanesManifest {
    let mut input = planner();
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

fn allocator<'a>(repository: &'a GitRepository, slug: &str) -> LaneAllocator<'a> {
    LaneAllocator::new(
        repository,
        ProtectedBranches::parse(["main"]).unwrap(),
        slug,
    )
    .unwrap()
}

#[test]
fn refinalization_preserves_the_recorded_integration_branch() {
    let mut previous = manifest_with(&[("WP01", "src/a/**")]);
    previous.mission_branch = "kitty/old-recorded-branch".to_owned();
    let mut input = planner().with_previous(previous);
    input
        .work_packages
        .insert(wp("WP01"), WpPlan::code(wp("WP01"), ["src/a/**"]));
    input.dependencies.insert(wp("WP01"), Vec::new());
    assert_eq!(
        compute_lanes(&input).unwrap().mission_branch,
        "kitty/old-recorded-branch"
    );
    input.topology = MissionTopology::SingleBranch;
    assert!(matches!(
        compute_lanes(&input),
        Err(WorkspaceError::SingleBranchCodeLanes { .. })
    ));
}

#[test]
fn fresh_reuse_and_dirty_reuse() {
    let fixture = init_repo();
    let repository = GitRepository::open(fixture.root()).unwrap();
    let allocator = allocator(&repository, "my-mission");
    let manifest = manifest_with(&[("WP01", "src/a/**"), ("WP02", "src/b/**")]);

    let fresh = allocator
        .allocate(
            &manifest,
            Some(MissionTopology::Lanes),
            None,
            &wp("WP01"),
            None,
        )
        .unwrap();
    assert_eq!(fresh.route, AllocationRoute::FreshLegacy);
    assert!(fresh.path.exists());
    assert_eq!(fresh.decision.parent_ref, "kitty/mission-my-mission");
    assert_eq!(
        support::git(&fresh.path, &["rev-parse", "--abbrev-ref", "HEAD"]),
        fresh.branch.as_str()
    );

    let reuse = allocator
        .allocate(
            &manifest,
            Some(MissionTopology::Lanes),
            None,
            &wp("WP01"),
            None,
        )
        .unwrap();
    assert_eq!(reuse.route, AllocationRoute::Reuse);
    assert_eq!(reuse.path, fresh.path);

    support::write(&fresh.path.join("scratch.txt"), "uncommitted");
    let error = allocator
        .allocate(
            &manifest,
            Some(MissionTopology::Lanes),
            None,
            &wp("WP01"),
            None,
        )
        .unwrap_err();
    assert!(matches!(
        error,
        WorkspaceError::DirtyWorktree { ref lane, .. } if lane == "lane-a"
    ));

    // A supplied base can never re-parent an existing lane.
    std::fs::remove_file(fresh.path.join("scratch.txt")).unwrap();
    // Spec-kitty runtime residue does not make a lane dirty.
    support::write(&fresh.path.join(".spec-kitty/review-lock.json"), "{}");
    let clean = allocator
        .allocate(
            &manifest,
            Some(MissionTopology::Lanes),
            None,
            &wp("WP01"),
            None,
        )
        .unwrap();
    assert_eq!(clean.route, AllocationRoute::Reuse);

    let error = allocator
        .allocate(
            &manifest,
            Some(MissionTopology::Lanes),
            None,
            &wp("WP01"),
            Some("main"),
        )
        .unwrap_err();
    assert!(matches!(error, WorkspaceError::UnhonorableBase { .. }));
}

#[test]
fn lane_placement_and_reuse_are_identical_from_a_linked_checkout() {
    let fixture = init_repo();
    let primary = GitRepository::open(fixture.root()).unwrap();
    let primary_allocator = allocator(&primary, "my-mission");
    let manifest = manifest_with(&[("WP01", "src/a/**"), ("WP02", "src/b/**")]);
    let lane = primary_allocator
        .allocate(
            &manifest,
            Some(MissionTopology::Lanes),
            None,
            &wp("WP01"),
            None,
        )
        .unwrap();
    let linked = GitRepository::open(&lane.path).unwrap();
    let linked_allocator = allocator(&linked, "my-mission");
    assert_eq!(
        primary_allocator.predict("my-mission", "lane-b").unwrap(),
        linked_allocator.predict("my-mission", "lane-b").unwrap(),
    );
    let reused = linked_allocator
        .allocate(
            &manifest,
            Some(MissionTopology::Lanes),
            None,
            &wp("WP01"),
            None,
        )
        .unwrap();
    assert_eq!(reused.route, AllocationRoute::Reuse);
    assert_eq!(reused.path, lane.path);
}

#[test]
fn user_status_filenames_are_never_treated_as_discardable_residue() {
    let fixture = init_repo();
    let repository = GitRepository::open(fixture.root()).unwrap();
    let allocator = allocator(&repository, "my-mission");
    let manifest = manifest_with(&[("WP01", "src/a/**")]);
    let lane = allocator
        .allocate(
            &manifest,
            Some(MissionTopology::Lanes),
            None,
            &wp("WP01"),
            None,
        )
        .unwrap();
    let paths = [
        "config/status.json",
        "custom/status.events.jsonl",
        "app/status.lock",
    ];
    for path in paths {
        support::write(&lane.path.join(path), "user-owned bytes");
    }
    let retention = allocator.classify_retention(&lane.path).unwrap();
    assert!(retention.owned_residue.is_empty());
    for path in paths {
        assert!(retention.custom.iter().any(|entry| entry == path));
    }
    assert!(matches!(
        allocator.retire_worktree("lane-a", &lane.path),
        Err(WorkspaceError::RetentionRefused { .. })
    ));
    for path in paths {
        assert_eq!(
            std::fs::read_to_string(lane.path.join(path)).unwrap(),
            "user-owned bytes"
        );
    }
}

#[test]
fn crash_recovery_reattaches_a_surviving_branch() {
    let fixture = init_repo();
    let repository = GitRepository::open(fixture.root()).unwrap();
    let allocator = allocator(&repository, "my-mission");
    let manifest = manifest_with(&[("WP03", "src/c/**")]);
    let (path, branch) = allocator.predict("my-mission", "lane-a").unwrap();

    // Simulate a process that created the branch and died before the worktree.
    let base = repository
        .read_branch_tip(&BranchName::parse("main").unwrap())
        .unwrap()
        .unwrap();
    let plan = repository
        .plan_branch_creation(&branch, StartPoint::Oid(base), "fixture: crash branch")
        .unwrap();
    repository.apply_branch_creation(&plan).unwrap();
    assert!(!path.exists());

    let recovered = allocator
        .allocate(
            &manifest,
            Some(MissionTopology::Lanes),
            None,
            &wp("WP03"),
            None,
        )
        .unwrap();
    assert_eq!(recovered.route, AllocationRoute::CrashRecovery);
    assert_eq!(recovered.path, path);
    assert!(path.exists());
}

#[test]
fn interrupted_creation_is_reconciled_from_the_journal() {
    let fixture = init_repo();
    let repository = GitRepository::open(fixture.root()).unwrap();
    let allocator = allocator(&repository, "my-mission");
    let manifest = manifest_with(&[("WP04", "src/d/**")]);
    let (path, branch) = allocator.predict("my-mission", "lane-a").unwrap();
    let base = repository
        .read_branch_tip(&BranchName::parse("main").unwrap())
        .unwrap()
        .unwrap();

    // Intent + branch, but no worktree.
    let plan = repository
        .plan_branch_creation(
            &branch,
            StartPoint::Oid(base.clone()),
            "fixture: interrupted",
        )
        .unwrap();
    repository.apply_branch_creation(&plan).unwrap();
    allocator
        .journal()
        .begin(sk_workspace::AllocationRecord::intent(
            "my-mission",
            "lane-a",
            branch.as_str(),
            &path,
            "main",
        ))
        .unwrap();
    let report = allocator.recover_interrupted().unwrap();
    assert_eq!(report.conflicts, Vec::new());
    assert!(matches!(
        report.repairs.as_slice(),
        [RecoveryRepair::Reattached { .. }]
    ));
    assert!(path.exists());
    assert!(allocator.journal().pending().unwrap().is_empty());

    // Intent that never mutated anything is cleared.
    let ghost_path = fixture.path(".worktrees/ghost-lane-a");
    allocator
        .journal()
        .begin(sk_workspace::AllocationRecord::intent(
            "my-mission",
            "lane-ghost",
            "kitty/mission-my-mission-lane-ghost",
            &ghost_path,
            "main",
        ))
        .unwrap();
    let report = allocator.recover_interrupted().unwrap();
    assert!(matches!(
        report.repairs.as_slice(),
        [RecoveryRepair::Cleared { .. }]
    ));

    // An intent already satisfied by a completed allocation is marked applied.
    let completed = allocator
        .allocate(
            &manifest,
            Some(MissionTopology::Lanes),
            None,
            &wp("WP04"),
            None,
        )
        .unwrap();
    allocator
        .journal()
        .begin(sk_workspace::AllocationRecord::intent(
            "my-mission",
            "lane-a",
            completed.branch.as_str(),
            &completed.path,
            "main",
        ))
        .unwrap();
    let report = allocator.recover_interrupted().unwrap();
    assert!(matches!(
        report.repairs.as_slice(),
        [RecoveryRepair::Completed { .. }]
    ));

    // Unregistered content is never deleted: it is reported.
    let unregistered = fixture.path(".worktrees/lane-conflict");
    support::write(&unregistered.join("user-file.txt"), "keep me");
    allocator
        .journal()
        .begin(sk_workspace::AllocationRecord::intent(
            "my-mission",
            "lane-conflict",
            "kitty/mission-my-mission-lane-conflict",
            &unregistered,
            "main",
        ))
        .unwrap();
    let report = allocator.recover_interrupted().unwrap();
    assert_eq!(report.conflicts.len(), 1);
    assert!(unregistered.join("user-file.txt").exists());
}

#[test]
fn bases_and_protected_branches_refuse_before_mutation() {
    let fixture = init_repo();
    let repository = GitRepository::open(fixture.root()).unwrap();
    let allocator = allocator(&repository, "my-mission");

    // A dependency-bearing lane cannot take an explicit base.
    let dependency_manifest = manifest_with_dependency();
    let error = allocator
        .allocate(
            &dependency_manifest,
            Some(MissionTopology::Lanes),
            None,
            &wp("WP02"),
            Some("main"),
        )
        .unwrap_err();
    assert!(matches!(error, WorkspaceError::UnhonorableBase { .. }));

    // A manifest whose integration branch is the protected target refuses
    // before creating any branch or worktree.
    let mut protected_manifest = manifest_with(&[("WP05", "src/e/**")]);
    protected_manifest.mission_branch = "main".to_owned();
    let lane_branch = "kitty/mission-my-mission-lane-a";
    assert!(
        repository
            .read_branch_tip(&BranchName::parse(lane_branch).unwrap())
            .unwrap()
            .is_none()
    );
    let error = allocator
        .allocate(
            &protected_manifest,
            Some(MissionTopology::Lanes),
            None,
            &wp("WP05"),
            None,
        )
        .unwrap_err();
    assert!(matches!(
        error,
        WorkspaceError::OwnedRefused {
            code: "OWNED_BRANCH_REFUSED",
            ..
        }
    ));
    assert!(
        repository
            .read_branch_tip(&BranchName::parse(lane_branch).unwrap())
            .unwrap()
            .is_none(),
        "no branch may be left behind"
    );
    assert!(!fixture.path(".worktrees/my-mission-lane-a").exists());
}

#[test]
fn a_moved_ref_makes_the_allocation_plan_stale() {
    let fixture = init_repo();
    let repository = GitRepository::open(fixture.root()).unwrap();
    let allocator = allocator(&repository, "my-mission");
    let manifest = manifest_with(&[("WP06", "src/f/**")]);
    assert!(
        lane_dependency_readiness(&manifest, &manifest.lanes[0].lane_id, &Default::default())
            .is_ready()
    );

    let (path, branch) = allocator.predict("my-mission", "lane-a").unwrap();
    let base = repository
        .read_branch_tip(&BranchName::parse("main").unwrap())
        .unwrap()
        .unwrap();
    let plan = repository
        .plan_worktree_add(
            path.clone(),
            &branch,
            WorktreeStart::NewBranch(StartPoint::Oid(base.clone())),
        )
        .unwrap();

    // Another actor creates the branch between plan and apply.
    let competing = repository
        .plan_branch_creation(&branch, StartPoint::Oid(base), "fixture: competing")
        .unwrap();
    repository.apply_branch_creation(&competing).unwrap();
    let error = repository.apply_worktree_add(&plan).unwrap_err();
    assert!(matches!(error, sk_git::GitError::PlanStale { .. }));

    // The same CAS rule guards branch advances: a stale expected tip refuses.
    support::write(&fixture.root().join("second.txt"), "second");
    support::commit_all(fixture.root(), "second");
    let new_tip = repository
        .read_branch_tip(&BranchName::parse("main").unwrap())
        .unwrap()
        .unwrap();
    let wrong_old = Oid::parse(&"0".repeat(40)).unwrap();
    let error = allocator
        .plan_branch_advance(
            &BranchName::parse("main").unwrap(),
            new_tip,
            Some(wrong_old),
            &BranchAdvanceOptions::default(),
        )
        .unwrap_err();
    assert!(matches!(
        error,
        WorkspaceError::Git(sk_git::GitError::PlanStale { .. })
    ));
}

#[test]
fn retirement_retains_custom_files_and_guards_unique_commits() {
    let fixture = init_repo();
    let repository = GitRepository::open(fixture.root()).unwrap();
    let allocator = allocator(&repository, "my-mission");
    let manifest = manifest_with(&[("WP07", "src/g/**"), ("WP08", "src/h/**")]);

    let base = repository
        .read_branch_tip(&BranchName::parse("main").unwrap())
        .unwrap()
        .unwrap();

    let first = allocator
        .allocate(
            &manifest,
            Some(MissionTopology::Lanes),
            None,
            &wp("WP07"),
            None,
        )
        .unwrap();
    support::write(&first.path.join("custom.txt"), "user state");
    let error = allocator
        .retire_worktree("lane-a", &first.path)
        .unwrap_err();
    assert!(matches!(
        error,
        WorkspaceError::RetentionRefused { ref lane, .. } if lane == "lane-a"
    ));
    assert!(first.path.join("custom.txt").exists());

    // Only spec-kitty runtime residue may be discarded.
    std::fs::remove_file(first.path.join("custom.txt")).unwrap();
    support::write(&first.path.join(".spec-kitty/review-lock.json"), "{}");
    allocator.retire_worktree("lane-a", &first.path).unwrap();
    assert!(!first.path.exists());

    // A lane with a unique commit cannot be deleted by the guarded path.
    let second = allocator
        .allocate(
            &manifest,
            Some(MissionTopology::Lanes),
            None,
            &wp("WP08"),
            None,
        )
        .unwrap();
    support::write(&second.path.join("delivered.txt"), "work");
    support::commit_all(&second.path, "lane work");
    allocator.retire_worktree("lane-b", &second.path).unwrap();
    let error = allocator
        .retire_branch(&second.branch, Some(&base))
        .unwrap_err();
    assert!(matches!(
        error,
        WorkspaceError::Git(sk_git::GitError::BranchHasUniqueCommits { .. })
    ));

    // A lane with no unique commits deletes cleanly.
    let third_manifest = manifest_with(&[("WP09", "src/i/**")]);
    let third = allocator
        .allocate(
            &third_manifest,
            Some(MissionTopology::Lanes),
            None,
            &wp("WP09"),
            None,
        )
        .unwrap();
    allocator.retire_worktree("lane-a", &third.path).unwrap();
    allocator.retire_branch(&third.branch, Some(&base)).unwrap();
    assert!(repository.read_branch_tip(&third.branch).unwrap().is_none());
}

#[test]
fn coord_topology_parents_on_the_coordination_branch() {
    let fixture = init_repo();
    let repository = GitRepository::open(fixture.root()).unwrap();
    let allocator = allocator(&repository, "my-mission");
    let manifest = manifest_with(&[("WP10", "src/j/**")]);
    let coordination_branch = "kitty/mission-my-mission-01KNXQS9";
    let allocation = allocator
        .allocate(
            &manifest,
            Some(MissionTopology::Coord),
            Some(coordination_branch),
            &wp("WP10"),
            None,
        )
        .unwrap();
    assert_eq!(
        allocation.decision.topology,
        sk_workspace::LaneTopology::Coord
    );
    assert_eq!(allocation.decision.parent_ref, coordination_branch);
    assert!(
        repository
            .read_branch_tip(&BranchName::parse(coordination_branch).unwrap())
            .unwrap()
            .is_some(),
        "the coordination integration branch is created from the target"
    );

    // A coordination-routing topology without a declared branch refuses
    // rather than silently using the mission integration branch.
    let error = allocator
        .allocate(
            &manifest,
            Some(MissionTopology::Coord),
            None,
            &wp("WP10"),
            None,
        )
        .unwrap_err();
    assert!(matches!(
        error,
        WorkspaceError::InvalidArtifact {
            artifact: "coordination_branch",
            ..
        }
    ));
}
