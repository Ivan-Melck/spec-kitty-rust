//! Planning-commit metadata/ancestry guards and fast-forward integration.

mod support;

use sk_git::{BranchName, GitRepository, Oid, ProtectedBranches};
use sk_workspace::{
    IntegrationOutcome, LanesStore, MetaDemotionGuard, PinClass, PlanningCommitPlan,
    ProjectWorkspace, ProtectionPolicy, WorkspaceError, classify_recorded_pin,
    integrate_dependency_lanes, integrate_planning_commit, record_planning_commit,
};

use support::{
    branch_tip, commit_all, commit_file, create_branch, init_repo, mission_id, show_at, write,
    write_mission_with,
};

struct Fixture {
    fixture: support::Fixture,
    slug: &'static str,
    id: String,
}

/// A `lanes` mission whose target branch is an unprotected feature branch, with
/// one committed planning artifact.
fn planning_fixture(tail: u8) -> Fixture {
    let fixture = init_repo();
    let id = mission_id(tail);
    let slug = "demo";
    create_branch(fixture.root(), "work");
    support::git(fixture.root(), &["checkout", "work"]);
    let dir = write_mission_with(
        fixture.root(),
        slug,
        &id,
        "lanes",
        "work",
        serde_json::json!({}),
    );
    write(&dir.join("spec.md"), "# Spec\n\nreal content\n");
    // Only `meta.json` is committed: `spec.md` stays untracked so a planning
    // commit always has real content to carry, and the untracked file survives
    // branch switches without being pulled into unrelated lane commits.
    commit_file(fixture.root(), "kitty-specs/demo/meta.json", "scaffold");
    Fixture { fixture, slug, id }
}

fn policy(fixture: &support::Fixture) -> ProtectionPolicy {
    ProtectionPolicy::resolve_with_hatch(fixture.root(), None).unwrap()
}

fn workspace(fixture: &support::Fixture, slug: &str) -> sk_workspace::MissionWorkspace {
    ProjectWorkspace::open(fixture.root())
        .unwrap()
        .mission(slug)
        .unwrap()
}

fn unprotected() -> ProtectedBranches {
    ProtectedBranches::default()
}

#[test]
fn a_planning_commit_lands_on_the_target_with_pinned_bytes() {
    let Fixture {
        fixture,
        slug,
        id: _,
    } = planning_fixture(30);
    let workspace = workspace(&fixture, slug);
    let plan = PlanningCommitPlan::prepare(
        &workspace,
        &["kitty-specs/demo/spec.md".to_owned()],
        "chore(spec-kitty): record spec",
        &policy(&fixture),
        None,
        None,
    )
    .unwrap();
    assert_eq!(plan.pin_class(), PinClass::Indeterminate);
    assert_eq!(plan.meta_guard(), MetaDemotionGuard::NotCommitted);
    let receipt = plan.apply(&workspace).unwrap();
    let landed = receipt.landed().expect("landed");
    assert_eq!(
        show_at(
            fixture.root(),
            landed.commit.as_str(),
            "kitty-specs/demo/spec.md"
        ),
        "# Spec\n\nreal content"
    );
    assert_eq!(receipt.destination.as_str(), "work");
}

#[test]
fn a_coordination_partition_artifact_cannot_ride_the_planning_commit() {
    let Fixture {
        fixture,
        slug,
        id: _,
    } = planning_fixture(31);
    let workspace = workspace(&fixture, slug);
    write(
        &fixture.path("kitty-specs/demo/status.events.jsonl"),
        "{\"event_id\":\"01KNXQS9ATWWFXS3K5ZJ9E5001\"}\n",
    );
    let error = PlanningCommitPlan::prepare(
        &workspace,
        &["kitty-specs/demo/status.events.jsonl".to_owned()],
        "chore(spec-kitty): wrong partition",
        &policy(&fixture),
        None,
        None,
    )
    .unwrap_err();
    assert!(
        matches!(
            error,
            WorkspaceError::CoordRefused {
                code: "PLANNING_PARTITION_REFUSED",
                ..
            }
        ),
        "{error}"
    );
}

#[test]
fn a_metadata_demotion_is_refused_before_the_plan_exists() {
    let Fixture {
        fixture,
        slug,
        id: _,
    } = planning_fixture(32);
    let workspace = workspace(&fixture, slug);
    let baseline = serde_json::json!({
        "mission_id": "01KNXQS9ATWWFXS3K5ZJ9E5032",
        "mission_slug": "demo",
        "topology": "coord",
        "coordination_branch": "kitty/mission-demo",
        "target_branch": "work",
    });
    let baseline = baseline.as_object().unwrap().clone();
    let error = PlanningCommitPlan::prepare(
        &workspace,
        &["kitty-specs/demo/meta.json".to_owned()],
        "chore(spec-kitty): commit meta",
        &policy(&fixture),
        Some(&baseline),
        None,
    )
    .unwrap_err();
    assert!(
        matches!(
            error,
            WorkspaceError::CoordRefused {
                code: "META_DEMOTION_REFUSED",
                ..
            }
        ),
        "{error}"
    );
}

#[test]
fn a_preserved_coordination_branch_passes_the_demotion_guard() {
    let Fixture {
        fixture,
        slug,
        id: _,
    } = planning_fixture(33);
    let workspace = workspace(&fixture, slug);
    write(
        &fixture.path("kitty-specs/demo/meta.json"),
        &format!(
            "{}\n",
            serde_json::json!({
                "mission_id": "01KNXQS9ATWWFXS3K5ZJ9E5033",
                "mission_slug": "demo",
                "topology": "coord",
                "coordination_branch": "kitty/mission-demo",
                "target_branch": "work",
            })
        ),
    );
    let baseline = serde_json::json!({"coordination_branch": "kitty/mission-demo"});
    let baseline = baseline.as_object().unwrap().clone();
    let plan = PlanningCommitPlan::prepare(
        &workspace,
        &["kitty-specs/demo/meta.json".to_owned()],
        "chore(spec-kitty): commit meta",
        &policy(&fixture),
        Some(&baseline),
        None,
    )
    .unwrap();
    assert_eq!(plan.meta_guard(), MetaDemotionGuard::Preserved);
}

#[test]
fn a_protected_target_is_refused() {
    let Fixture {
        fixture,
        slug,
        id: _,
    } = planning_fixture(34);
    write(
        &fixture.root().join(".kittify/config.yaml"),
        "protection:\n  protected_branches: [work]\n",
    );
    let workspace = workspace(&fixture, slug);
    let error = PlanningCommitPlan::prepare(
        &workspace,
        &["kitty-specs/demo/spec.md".to_owned()],
        "chore(spec-kitty): record spec",
        &policy(&fixture),
        None,
        None,
    )
    .unwrap_err();
    assert!(
        matches!(
            error,
            WorkspaceError::CoordRefused {
                code: "PROTECTED_BRANCH_REFUSED",
                ..
            }
        ),
        "{error}"
    );
}

#[test]
fn a_checkout_on_another_branch_is_refused() {
    let Fixture {
        fixture,
        slug,
        id: _,
    } = planning_fixture(35);
    support::git(fixture.root(), &["checkout", "--detach", "HEAD"]);
    let workspace = workspace(&fixture, slug);
    let error = PlanningCommitPlan::prepare(
        &workspace,
        &["kitty-specs/demo/spec.md".to_owned()],
        "chore(spec-kitty): record spec",
        &policy(&fixture),
        None,
        None,
    )
    .unwrap_err();
    assert!(
        matches!(
            error,
            WorkspaceError::CoordRefused {
                code: "PLANNING_COMMIT_HEAD_MISMATCH",
                ..
            }
        ),
        "{error}"
    );
}

#[test]
fn an_orphaned_or_foreign_recorded_pin_is_refused() {
    let Fixture {
        fixture,
        slug,
        id: _,
    } = planning_fixture(36);
    let workspace = workspace(&fixture, slug);
    // A commit reachable only from a side branch is an orphaned pin.
    support::git(fixture.root(), &["checkout", "-b", "side"]);
    write(&fixture.path("side.txt"), "side\n");
    commit_file(fixture.root(), "side.txt", "side");
    let side = branch_tip(fixture.root(), "side");
    support::git(fixture.root(), &["checkout", "work"]);
    let repository = GitRepository::open(fixture.root()).unwrap();
    let target_tip = Oid::parse(&branch_tip(fixture.root(), "work")).unwrap();
    assert_eq!(
        classify_recorded_pin(&repository, Some(&side), Some(&target_tip)).unwrap(),
        PinClass::Orphaned
    );
    assert_eq!(
        classify_recorded_pin(
            &repository,
            Some("0000000000000000000000000000000000000001"),
            Some(&target_tip)
        )
        .unwrap(),
        PinClass::Foreign
    );
    let error = PlanningCommitPlan::prepare(
        &workspace,
        &["kitty-specs/demo/spec.md".to_owned()],
        "chore(spec-kitty): record spec",
        &policy(&fixture),
        None,
        Some(&side),
    )
    .unwrap_err();
    assert!(
        matches!(
            error,
            WorkspaceError::CoordRefused {
                code: "PLANNING_PIN_UNUSABLE",
                ..
            }
        ),
        "{error}"
    );
}

#[test]
fn a_landed_planning_commit_is_recorded_in_the_manifest() {
    let Fixture {
        fixture,
        slug,
        id: _,
    } = planning_fixture(37);
    let workspace = workspace(&fixture, slug);
    let dir = workspace.mission_dir().to_path_buf();
    write(
        &dir.join("lanes.json"),
        &format!(
            "{}\n",
            serde_json::json!({
                "version": 1,
                "mission_slug": slug,
                "mission_id": "01KNXQS9ATWWFXS3K5ZJ9E5037",
                "mission_branch": "kitty/mission-demo",
                "target_branch": "work",
                "lanes": [{
                    "lane_id": "lane-a",
                    "wp_ids": ["WP01"],
                    "write_scope": [],
                    "predicted_surfaces": [],
                    "depends_on_lanes": [],
                    "parallel_group": 1,
                }],
                "computed_at": "2026-11-02T00:00:00Z",
                "computed_from": "fixture",
                "planning_artifact_wps": [],
                "planning_commit_sha": null,
            })
        ),
    );
    let store = LanesStore::new(&dir);
    let plan = PlanningCommitPlan::prepare(
        &workspace,
        &["kitty-specs/demo/spec.md".to_owned()],
        "chore(spec-kitty): record spec",
        &policy(&fixture),
        None,
        None,
    )
    .unwrap();
    let receipt = plan.apply(&workspace).unwrap();
    let landed = receipt.landed().unwrap().commit.clone();
    let updated = record_planning_commit(&store, &landed).unwrap();
    assert_eq!(
        updated.planning_commit_sha.as_deref(),
        Some(landed.as_str())
    );
    let reread = store.read().unwrap().unwrap();
    assert_eq!(reread.planning_commit_sha.as_deref(), Some(landed.as_str()));
}

#[test]
fn a_lane_fast_forwards_to_the_recorded_planning_commit_then_is_already_integrated() {
    let Fixture {
        fixture,
        slug,
        id: _,
    } = planning_fixture(38);
    let workspace = workspace(&fixture, slug);
    let plan = PlanningCommitPlan::prepare(
        &workspace,
        &["kitty-specs/demo/spec.md".to_owned()],
        "chore(spec-kitty): record spec",
        &policy(&fixture),
        None,
        None,
    )
    .unwrap();
    let receipt = plan.apply(&workspace).unwrap();
    let planning = receipt.landed().unwrap().commit.clone();
    // A lane cut from the initial commit predates the planning commit.
    support::git(fixture.root(), &["checkout", "main"]);
    create_branch(fixture.root(), "kitty/mission-demo-lane-a");
    support::git(fixture.root(), &["checkout", "work"]);
    let repository = GitRepository::open(fixture.root()).unwrap();
    let lane = BranchName::parse("kitty/mission-demo-lane-a").unwrap();
    let outcome = integrate_planning_commit(&repository, &lane, &planning, &unprotected()).unwrap();
    assert!(
        matches!(outcome, IntegrationOutcome::FastForwarded(_)),
        "{outcome:?}"
    );
    assert_eq!(
        branch_tip(fixture.root(), "kitty/mission-demo-lane-a"),
        planning.to_string()
    );
    let again = integrate_planning_commit(&repository, &lane, &planning, &unprotected()).unwrap();
    assert!(matches!(
        again,
        IntegrationOutcome::AlreadyIntegrated { .. }
    ));
}

#[test]
fn a_divergent_lane_reports_merge_required_and_is_not_moved() {
    let Fixture {
        fixture,
        slug,
        id: _,
    } = planning_fixture(39);
    let workspace = workspace(&fixture, slug);
    let plan = PlanningCommitPlan::prepare(
        &workspace,
        &["kitty-specs/demo/spec.md".to_owned()],
        "chore(spec-kitty): record spec",
        &policy(&fixture),
        None,
        None,
    )
    .unwrap();
    let planning = plan
        .apply(&workspace)
        .unwrap()
        .landed()
        .unwrap()
        .commit
        .clone();
    // The lane branches from main and takes its own commit: both sides moved.
    support::git(fixture.root(), &["checkout", "main"]);
    support::git(
        fixture.root(),
        &["checkout", "-b", "kitty/mission-demo-lane-a"],
    );
    write(&fixture.path("lane.txt"), "lane work\n");
    commit_all(fixture.root(), "lane work");
    let lane_tip = Oid::parse(&branch_tip(fixture.root(), "kitty/mission-demo-lane-a")).unwrap();
    support::git(fixture.root(), &["checkout", "work"]);
    let repository = GitRepository::open(fixture.root()).unwrap();
    let lane = BranchName::parse("kitty/mission-demo-lane-a").unwrap();
    let outcome = integrate_planning_commit(&repository, &lane, &planning, &unprotected()).unwrap();
    match outcome {
        IntegrationOutcome::MergeRequired {
            destination_tip,
            source_tip,
            detail,
        } => {
            assert_eq!(destination_tip, lane_tip);
            assert_eq!(source_tip, planning);
            assert!(detail.contains("merge commit"));
        }
        other => panic!("expected MergeRequired, got {other:?}"),
    }
    assert_eq!(
        branch_tip(fixture.root(), "kitty/mission-demo-lane-a"),
        lane_tip.to_string(),
        "a MergeRequired outcome mutates nothing"
    );
}

#[test]
fn dependency_lane_tips_fast_forward_into_a_dependent_lane() {
    let Fixture { fixture, slug, id } = planning_fixture(40);
    let workspace = workspace(&fixture, slug);
    let dir = workspace.mission_dir().to_path_buf();
    write(
        &dir.join("lanes.json"),
        &format!(
            "{}\n",
            serde_json::json!({
                "version": 1,
                "mission_slug": slug,
                "mission_id": id,
                "mission_branch": "kitty/mission-demo",
                "target_branch": "work",
                "lanes": [
                    {
                        "lane_id": "lane-a",
                        "wp_ids": ["WP01"],
                        "write_scope": [],
                        "predicted_surfaces": [],
                        "depends_on_lanes": [],
                        "parallel_group": 1,
                    },
                    {
                        "lane_id": "lane-b",
                        "wp_ids": ["WP02"],
                        "write_scope": [],
                        "predicted_surfaces": [],
                        "depends_on_lanes": ["lane-a"],
                        "parallel_group": 2,
                    }
                ],
                "computed_at": "2026-11-02T00:00:00Z",
                "computed_from": "fixture",
                "planning_artifact_wps": [],
                "planning_commit_sha": null,
            })
        ),
    );
    let manifest = LanesStore::new(&dir).read().unwrap().unwrap();

    // lane-b starts at the current tip; lane-a then takes a commit.
    let base = branch_tip(fixture.root(), "work");
    support::git(
        fixture.root(),
        &["branch", "kitty/mission-demo-lane-a", &base],
    );
    support::git(
        fixture.root(),
        &["branch", "kitty/mission-demo-lane-b", &base],
    );
    let lane_b_path = fixture.root().join(".worktrees/demo-lane-b");
    support::git(
        fixture.root(),
        &[
            "worktree",
            "add",
            lane_b_path.to_str().unwrap(),
            "kitty/mission-demo-lane-b",
        ],
    );
    support::git(fixture.root(), &["checkout", "kitty/mission-demo-lane-a"]);
    write(&fixture.path("dep.txt"), "dep\n");
    commit_file(fixture.root(), "dep.txt", "dependency work");
    let dep_tip = branch_tip(fixture.root(), "kitty/mission-demo-lane-a");
    support::git(fixture.root(), &["checkout", "work"]);

    let report =
        integrate_dependency_lanes(&workspace, &manifest, "lane-b", &unprotected()).unwrap();
    assert!(report.is_complete(), "{report:?}");
    assert_eq!(report.integrations.len(), 1);
    assert!(matches!(
        report.integrations[0].outcome,
        IntegrationOutcome::FastForwarded(_)
    ));
    assert_eq!(
        branch_tip(fixture.root(), "kitty/mission-demo-lane-b"),
        dep_tip,
        "the dependent lane branch advanced to the dependency tip"
    );
}

#[test]
fn a_dependent_lane_that_diverged_needs_a_merge() {
    let Fixture { fixture, slug, id } = planning_fixture(41);
    let workspace = workspace(&fixture, slug);
    let dir = workspace.mission_dir().to_path_buf();
    write(
        &dir.join("lanes.json"),
        &format!(
            "{}\n",
            serde_json::json!({
                "version": 1,
                "mission_slug": slug,
                "mission_id": id,
                "mission_branch": "kitty/mission-demo",
                "target_branch": "work",
                "lanes": [
                    {
                        "lane_id": "lane-a",
                        "wp_ids": ["WP01"],
                        "write_scope": [],
                        "predicted_surfaces": [],
                        "depends_on_lanes": [],
                        "parallel_group": 1,
                    },
                    {
                        "lane_id": "lane-b",
                        "wp_ids": ["WP02"],
                        "write_scope": [],
                        "predicted_surfaces": [],
                        "depends_on_lanes": ["lane-a"],
                        "parallel_group": 2,
                    }
                ],
                "computed_at": "2026-11-02T00:00:00Z",
                "computed_from": "fixture",
                "planning_artifact_wps": [],
                "planning_commit_sha": null,
            })
        ),
    );
    let manifest = LanesStore::new(&dir).read().unwrap().unwrap();
    let base = branch_tip(fixture.root(), "work");
    support::git(
        fixture.root(),
        &["branch", "kitty/mission-demo-lane-a", &base],
    );
    support::git(
        fixture.root(),
        &["branch", "kitty/mission-demo-lane-b", &base],
    );
    let lane_b_path = fixture.root().join(".worktrees/demo-lane-b");
    support::git(
        fixture.root(),
        &[
            "worktree",
            "add",
            lane_b_path.to_str().unwrap(),
            "kitty/mission-demo-lane-b",
        ],
    );
    // Both lanes take their own commits: they diverge.
    support::git(fixture.root(), &["checkout", "kitty/mission-demo-lane-a"]);
    write(&fixture.path("a.txt"), "a\n");
    commit_file(fixture.root(), "a.txt", "lane a");
    // lane-b is checked out in its own worktree; commit there, never by
    // stealing the branch into the primary checkout.
    write(&lane_b_path.join("b.txt"), "b\n");
    commit_file(&lane_b_path, "b.txt", "lane b");
    let lane_b_tip = branch_tip(fixture.root(), "kitty/mission-demo-lane-b");
    support::git(fixture.root(), &["checkout", "work"]);

    let report =
        integrate_dependency_lanes(&workspace, &manifest, "lane-b", &unprotected()).unwrap();
    assert!(!report.is_complete());
    assert!(!report.unintegrated().is_empty());
    assert_eq!(
        branch_tip(fixture.root(), "kitty/mission-demo-lane-b"),
        lane_b_tip,
        "a MergeRequired outcome mutates nothing"
    );
}
