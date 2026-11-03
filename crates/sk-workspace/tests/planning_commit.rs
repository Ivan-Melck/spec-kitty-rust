//! Planning-commit metadata/ancestry guards, pin recording, and integration.
//!
//! The expectations for the `meta.json` demotion guard and the dependency
//! ordering were checked against the pinned Python oracle
//! (`specify_cli/coordination/planning_commit.py`,
//! `lanes/worktree_allocator.py`) on equivalent temporary repositories; the
//! recorded oracle outputs are in `REMEDIATION-REPORT.md`.

mod support;

use sk_git::{GitRepository, Oid, ProtectedBranches};
use sk_workspace::{
    CoordJournal, CoordPhase, CoordRecordKind, IntegrationOutcome, LanesStore, MetaDemotionGuard,
    PinClass, PlanningCommitPlan, PlanningPinMode, ProjectWorkspace, ProtectionPolicy,
    WorkspaceError, classify_recorded_pin, integrate_dependency_lanes, integrate_planning_commit,
    lane_contains_planning_commit, record_planning_commit, record_planning_commit_with,
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

/// Write a `lanes.json` manifest and return it through the store.
fn lanes_manifest(
    fixture: &support::Fixture,
    slug: &str,
    id: &str,
    lanes: serde_json::Value,
) -> (std::path::PathBuf, sk_workspace::LanesManifest) {
    let dir = fixture.path("kitty-specs").join(slug);
    write(
        &dir.join("lanes.json"),
        &format!(
            "{}\n",
            serde_json::json!({
                "version": 1,
                "mission_slug": slug,
                "mission_id": id,
                "mission_branch": format!("kitty/mission-{slug}"),
                "target_branch": "work",
                "lanes": lanes,
                "computed_at": "2026-11-02T00:00:00Z",
                "computed_from": "fixture",
                "planning_artifact_wps": [],
                "planning_commit_sha": null,
            })
        ),
    );
    let store = LanesStore::new(&dir);
    let manifest = store.read().unwrap().unwrap();
    (dir, manifest)
}

fn lane(id: &str, wps: &[&str], depends: &[&str], parallel_group: u32) -> serde_json::Value {
    serde_json::json!({
        "lane_id": id,
        "wp_ids": wps,
        "write_scope": [],
        "predicted_surfaces": [],
        "depends_on_lanes": depends,
        "parallel_group": parallel_group,
    })
}

/// Re-point one lane's declared dependencies.
fn with_dependencies(
    mut manifest: sk_workspace::LanesManifest,
    lane_id: &str,
    depends: &[&str],
) -> sk_workspace::LanesManifest {
    let target = manifest
        .lanes
        .iter_mut()
        .find(|lane| lane.lane_id == lane_id)
        .expect("the lane exists");
    target.depends_on_lanes = depends.iter().map(|id| (*id).to_owned()).collect();
    manifest
}

fn add_worktree(
    fixture: &support::Fixture,
    slug: &str,
    lane_id: &str,
    branch: &str,
) -> std::path::PathBuf {
    let path = fixture
        .root()
        .join(".worktrees")
        .join(format!("{slug}-{lane_id}"));
    support::git(
        fixture.root(),
        &["worktree", "add", path.to_str().unwrap(), branch],
    );
    path
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
    // The claim + Mission locks are the ones the commit held, not a receipt
    // reconstructed after they were released (the commit used to run unlocked).
    assert_eq!(receipt.lock_receipt.mission_lock_key, slug);
    assert!(
        receipt
            .lock_receipt
            .mission_lock_path
            .as_ref()
            .expect("the canonical Mission guard exposes its path")
            .ends_with("demo.status.lock")
    );
    assert!(
        sk_kernel::lock_families::is_checkout_claim_lock_key(
            &receipt.lock_receipt.checkout_claim_lock.to_string_lossy()
        ),
        "the held lock is the kernel checkout-claim family: {:?}",
        receipt.lock_receipt.checkout_claim_lock
    );
    assert!(receipt.journal_fault.is_none());
    // The commit intent is journaled and resolved, not left pending.
    let journal = CoordJournal::new(workspace.workspace().common_dir(), slug);
    let records = journal.records().unwrap();
    assert!(records.iter().any(|record| {
        record.kind == CoordRecordKind::Commit
            && record.operation == "planning_commit"
            && record.phase == CoordPhase::Applied
    }));
    assert!(journal.pending().unwrap().is_empty());
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

/// Commit a `meta.json` whose `coordination_branch` is `routed`, then rewrite the
/// working copy to `working`. Returns the workspace so the caller can plan.
fn routed_meta_fixture(tail: u8, routed: serde_json::Value, working: serde_json::Value) -> Fixture {
    let Fixture { fixture, slug, id } = planning_fixture(tail);
    let mut head = serde_json::json!({
        "mission_id": id,
        "mission_slug": slug,
        "topology": "coord",
        "target_branch": "work",
        "mission_type": "software-dev",
    });
    head.as_object_mut()
        .unwrap()
        .insert("coordination_branch".to_owned(), routed);
    write(
        &fixture.path("kitty-specs/demo/meta.json"),
        &format!("{}\n", serde_json::to_string_pretty(&head).unwrap()),
    );
    commit_file(fixture.root(), "kitty-specs/demo/meta.json", "route");
    let mut body = serde_json::json!({
        "mission_id": id,
        "mission_slug": slug,
        "topology": "coord",
        "target_branch": "work",
        "mission_type": "software-dev",
    });
    if let Some(working) = working.as_object() {
        for (key, value) in working {
            body.as_object_mut()
                .unwrap()
                .insert(key.clone(), value.clone());
        }
    }
    write(
        &fixture.path("kitty-specs/demo/meta.json"),
        &format!("{}\n", serde_json::to_string_pretty(&body).unwrap()),
    );
    Fixture { fixture, slug, id }
}

fn plan_meta(holder: &Fixture) -> Result<PlanningCommitPlan, WorkspaceError> {
    let workspace = workspace(&holder.fixture, holder.slug);
    PlanningCommitPlan::prepare(
        &workspace,
        &["kitty-specs/demo/meta.json".to_owned()],
        "chore(spec-kitty): commit meta",
        &policy(&holder.fixture),
        None,
    )
}

#[test]
fn a_metadata_demotion_is_refused_before_the_plan_exists() {
    // HEAD is routed; the working copy simply drops the key.
    let holder = routed_meta_fixture(
        32,
        serde_json::json!("kitty/mission-demo"),
        serde_json::json!({}),
    );
    let error = plan_meta(&holder).unwrap_err();
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
fn an_empty_or_null_working_coordination_branch_is_a_demotion() {
    // The reference treats `""` and `null` as falsy, exactly like absence. The
    // previous native guard counted `""` as "routed", which was a fail-open on
    // the one thing this guard computes.
    for working in [serde_json::json!(""), serde_json::Value::Null] {
        let holder = routed_meta_fixture(
            33,
            serde_json::json!("kitty/mission-demo"),
            serde_json::json!({"coordination_branch": working}),
        );
        let error = plan_meta(&holder).unwrap_err();
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
}

#[test]
fn a_preserved_coordination_branch_passes_the_demotion_guard() {
    let holder = routed_meta_fixture(
        34,
        serde_json::json!("kitty/mission-demo"),
        serde_json::json!({"coordination_branch": "kitty/mission-demo"}),
    );
    let plan = plan_meta(&holder).unwrap();
    assert_eq!(plan.meta_guard(), MetaDemotionGuard::Preserved);
}

#[test]
fn an_unborn_or_absent_head_baseline_is_a_first_commit() {
    // HEAD carries no `meta.json` at all: a legitimate first commit.
    let fixture = init_repo();
    let id = mission_id(35);
    create_branch(fixture.root(), "work");
    support::git(fixture.root(), &["checkout", "work"]);
    write_mission_with(
        fixture.root(),
        "demo",
        &id,
        "coord",
        "work",
        serde_json::json!({"coordination_branch": "kitty/mission-demo"}),
    );
    write(&fixture.path("target.txt"), "target\n");
    commit_file(fixture.root(), "target.txt", "advance without meta");
    let workspace = workspace(&fixture, "demo");
    let plan = PlanningCommitPlan::prepare(
        &workspace,
        &["kitty-specs/demo/meta.json".to_owned()],
        "chore(spec-kitty): commit meta",
        &policy(&fixture),
        None,
    )
    .unwrap();
    assert_eq!(plan.meta_guard(), MetaDemotionGuard::NoHeadBaseline);
}

#[test]
fn an_unparseable_head_baseline_is_refused() {
    // HEAD's `meta.json` is not JSON; the working copy is valid. The reference
    // reads the baseline from `git show HEAD:<path>` and refuses a baseline it
    // cannot parse, so this must not silently become "no baseline".
    let fixture = init_repo();
    let id = mission_id(36);
    create_branch(fixture.root(), "work");
    support::git(fixture.root(), &["checkout", "work"]);
    write(&fixture.path("kitty-specs/demo/meta.json"), "not json\n");
    commit_file(fixture.root(), "kitty-specs/demo/meta.json", "broken meta");
    write_mission_with(
        fixture.root(),
        "demo",
        &id,
        "coord",
        "work",
        serde_json::json!({"coordination_branch": "kitty/mission-demo"}),
    );
    let workspace = workspace(&fixture, "demo");
    let error = PlanningCommitPlan::prepare(
        &workspace,
        &["kitty-specs/demo/meta.json".to_owned()],
        "chore(spec-kitty): commit meta",
        &policy(&fixture),
        None,
    )
    .unwrap_err();
    assert!(
        matches!(
            error,
            WorkspaceError::CoordRefused {
                code: "META_DEMOTION_GUARD_CORRUPT",
                ..
            }
        ),
        "{error}"
    );
}

#[test]
fn a_protected_target_is_refused() {
    let Fixture {
        fixture,
        slug,
        id: _,
    } = planning_fixture(37);
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

/// A `single_branch` mission with a recorded `mission_branch` writes THAT
/// branch, even when the target is protected. The destination used to be the
/// target, which over-refused this supported flow.
#[test]
fn a_single_branch_mission_commits_its_recorded_mission_branch() {
    let fixture = init_repo();
    let id = mission_id(38);
    create_branch(fixture.root(), "work");
    support::git(fixture.root(), &["checkout", "work"]);
    let mission_branch = "kitty/mission-demo-01KNXQS9";
    support::git(fixture.root(), &["checkout", "-b", mission_branch]);
    let target_before = branch_tip(fixture.root(), "work");
    let dir = write_mission_with(
        fixture.root(),
        "demo",
        &id,
        "single_branch",
        "work",
        serde_json::json!({"mission_branch": mission_branch}),
    );
    write(&dir.join("spec.md"), "# Spec\n");
    commit_file(fixture.root(), "kitty-specs/demo/meta.json", "scaffold");
    // The target is protected; `commit_to_target` is not set, so the record of
    // the mint decision is what makes the write branch usable.
    write(
        &fixture.root().join(".kittify/config.yaml"),
        "protection:\n  protected_branches: [work]\n",
    );
    let workspace = workspace(&fixture, "demo");
    let plan = PlanningCommitPlan::prepare(
        &workspace,
        &["kitty-specs/demo/spec.md".to_owned()],
        "chore(spec-kitty): record spec",
        &policy(&fixture),
        None,
    )
    .unwrap();
    assert_eq!(plan.destination().as_str(), mission_branch);
    let receipt = plan.apply(&workspace).unwrap();
    assert!(receipt.landed().is_some());
    assert_eq!(
        show_at(
            fixture.root(),
            &branch_tip(fixture.root(), mission_branch),
            "kitty-specs/demo/spec.md"
        ),
        "# Spec"
    );
    assert_eq!(
        branch_tip(fixture.root(), "work"),
        target_before,
        "the protected target is never touched"
    );
}

#[test]
fn a_checkout_on_another_branch_is_refused() {
    let Fixture {
        fixture,
        slug,
        id: _,
    } = planning_fixture(39);
    support::git(fixture.root(), &["checkout", "--detach", "HEAD"]);
    let workspace = workspace(&fixture, slug);
    let error = PlanningCommitPlan::prepare(
        &workspace,
        &["kitty-specs/demo/spec.md".to_owned()],
        "chore(spec-kitty): record spec",
        &policy(&fixture),
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
    } = planning_fixture(40);
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
    let Fixture { fixture, slug, id } = planning_fixture(41);
    let (_dir, _manifest) = lanes_manifest(
        &fixture,
        slug,
        &id,
        serde_json::json!([lane("lane-a", &["WP01"], &[], 1)]),
    );
    let workspace = workspace(&fixture, slug);
    let store = LanesStore::new(workspace.mission_dir());
    let plan = PlanningCommitPlan::prepare(
        &workspace,
        &["kitty-specs/demo/spec.md".to_owned()],
        "chore(spec-kitty): record spec",
        &policy(&fixture),
        None,
    )
    .unwrap();
    let receipt = plan.apply(&workspace).unwrap();
    let landed = receipt.landed().unwrap().commit.clone();
    let updated = record_planning_commit(&store, &landed).unwrap();
    assert_eq!(updated.recorded.as_ref(), Some(&landed));
    assert!(updated.preserved.is_none());
    assert_eq!(
        updated.manifest.planning_commit_sha.as_deref(),
        Some(landed.as_str())
    );
    let reread = store.read().unwrap().unwrap();
    assert_eq!(reread.planning_commit_sha.as_deref(), Some(landed.as_str()));
}

#[test]
fn an_existing_pin_is_preserved_unless_explicitly_repointed() {
    let Fixture { fixture, slug, id } = planning_fixture(42);
    let (_dir, _manifest) = lanes_manifest(
        &fixture,
        slug,
        &id,
        serde_json::json!([lane("lane-a", &["WP01"], &[], 1)]),
    );
    let workspace = workspace(&fixture, slug);
    let store = LanesStore::new(workspace.mission_dir());
    let first = Oid::parse(&branch_tip(fixture.root(), "work")).unwrap();
    let second = Oid::parse(&branch_tip(fixture.root(), "main")).unwrap();

    let initial = record_planning_commit(&store, &first).unwrap();
    assert_eq!(initial.recorded.as_ref(), Some(&first));

    let preserved = record_planning_commit(&store, &second).unwrap();
    assert_eq!(preserved.preserved.as_ref(), Some(&first));
    assert_eq!(preserved.recorded.as_ref(), Some(&first));
    assert_eq!(
        store
            .read()
            .unwrap()
            .unwrap()
            .planning_commit_sha
            .as_deref(),
        Some(first.as_str())
    );

    let replaced = record_planning_commit_with(&store, &second, PlanningPinMode::Replace).unwrap();
    assert_eq!(replaced.recorded.as_ref(), Some(&second));
    assert_eq!(
        store
            .read()
            .unwrap()
            .unwrap()
            .planning_commit_sha
            .as_deref(),
        Some(second.as_str())
    );
}

#[test]
fn a_pin_write_reads_the_current_manifest_under_the_lock() {
    // A concurrent finalize between an unlocked read and the write used to lose
    // lane changes. The single locked read-modify-write must see whatever is on
    // disk at the moment it runs, including a lane added after the store was
    // constructed.
    let Fixture { fixture, slug, id } = planning_fixture(43);
    let (dir, _manifest) = lanes_manifest(
        &fixture,
        slug,
        &id,
        serde_json::json!([lane("lane-a", &["WP01"], &[], 1)]),
    );
    let store = LanesStore::new(&dir);
    // Simulate the other writer: add a lane directly, without the store.
    let mut manifest = store.read().unwrap().unwrap();
    manifest
        .lanes
        .push(serde_json::from_value(lane("lane-b", &["WP02"], &[], 2)).expect("second lane"));
    support::write(
        &dir.join("lanes.json"),
        &format!("{}\n", serde_json::to_string_pretty(&manifest).unwrap()),
    );
    let commit = Oid::parse(&branch_tip(fixture.root(), "work")).unwrap();
    let receipt = record_planning_commit(&store, &commit).unwrap();
    let lane_ids: Vec<&str> = receipt
        .manifest
        .lanes
        .iter()
        .map(|entry| entry.lane_id.as_str())
        .collect();
    assert_eq!(lane_ids, vec!["lane-a", "lane-b"]);
    assert_eq!(
        receipt.manifest.planning_commit_sha.as_deref(),
        Some(commit.as_str())
    );
}

fn planning_commit(holder: &Fixture) -> Oid {
    let workspace = workspace(&holder.fixture, holder.slug);
    let plan = PlanningCommitPlan::prepare(
        &workspace,
        &["kitty-specs/demo/spec.md".to_owned()],
        "chore(spec-kitty): record spec",
        &policy(&holder.fixture),
        None,
    )
    .unwrap();
    plan.apply(&workspace)
        .unwrap()
        .landed()
        .expect("landed")
        .commit
        .clone()
}

#[test]
fn a_lane_fast_forwards_to_the_recorded_planning_commit_then_is_already_integrated() {
    let holder = planning_fixture(44);
    let Fixture { fixture, slug, id } = &holder;
    let planning = planning_commit(&holder);
    let (_dir, manifest) = lanes_manifest(
        fixture,
        slug,
        id,
        serde_json::json!([lane("lane-a", &["WP01"], &[], 1)]),
    );
    // A lane cut from the initial commit predates the planning commit.
    support::git(fixture.root(), &["checkout", "main"]);
    create_branch(fixture.root(), "kitty/mission-demo-lane-a");
    add_worktree(fixture, slug, "lane-a", "kitty/mission-demo-lane-a");
    support::git(fixture.root(), &["checkout", "work"]);
    let workspace = workspace(fixture, slug);
    assert!(
        !lane_contains_planning_commit(
            &GitRepository::open(fixture.root()).unwrap(),
            "kitty/mission-demo-lane-a",
            &planning
        )
        .unwrap()
    );
    let report =
        integrate_planning_commit(&workspace, &manifest, "lane-a", &planning, &unprotected())
            .unwrap();
    assert!(
        matches!(
            report.integrations[0].outcome,
            IntegrationOutcome::FastForwarded(_)
        ),
        "{report:?}"
    );
    assert_eq!(
        branch_tip(fixture.root(), "kitty/mission-demo-lane-a"),
        planning.to_string()
    );
    assert!(
        lane_contains_planning_commit(
            &GitRepository::open(fixture.root()).unwrap(),
            "kitty/mission-demo-lane-a",
            &planning
        )
        .unwrap()
    );
    let again =
        integrate_planning_commit(&workspace, &manifest, "lane-a", &planning, &unprotected())
            .unwrap();
    assert!(matches!(
        again.integrations[0].outcome,
        IntegrationOutcome::AlreadyIntegrated { .. }
    ));
    // The integration intent is journaled and resolved.
    let records = CoordJournal::new(workspace.workspace().common_dir(), slug)
        .records()
        .unwrap();
    assert!(records.iter().any(|record| {
        record.kind == CoordRecordKind::Integration && record.phase == CoordPhase::Applied
    }));
}

#[test]
fn a_divergent_lane_gets_a_real_merge_commit() {
    let holder = planning_fixture(45);
    let Fixture { fixture, slug, id } = &holder;
    let planning = planning_commit(&holder);
    let (_dir, manifest) = lanes_manifest(
        fixture,
        slug,
        id,
        serde_json::json!([lane("lane-a", &["WP01"], &[], 1)]),
    );
    support::git(fixture.root(), &["checkout", "main"]);
    support::git(
        fixture.root(),
        &["checkout", "-b", "kitty/mission-demo-lane-a"],
    );
    write(&fixture.path("lane.txt"), "lane work\n");
    commit_all(fixture.root(), "lane work");
    let lane_tip = Oid::parse(&branch_tip(fixture.root(), "kitty/mission-demo-lane-a")).unwrap();
    // Give the branch back before attaching a worktree to it.
    support::git(fixture.root(), &["checkout", "work"]);
    add_worktree(fixture, slug, "lane-a", "kitty/mission-demo-lane-a");
    let workspace = workspace(fixture, slug);
    let report =
        integrate_planning_commit(&workspace, &manifest, "lane-a", &planning, &unprotected())
            .unwrap();
    let IntegrationOutcome::Merged(merge) = &report.integrations[0].outcome else {
        panic!(
            "expected a real merge, got {:?}",
            report.integrations[0].outcome
        );
    };
    assert_eq!(&merge.merged_tip, &planning);
    assert_eq!(&merge.previous_head, &lane_tip);
    // Both sides are present at the new tip.
    assert_eq!(
        show_at(fixture.root(), merge.new_head.as_str(), "lane.txt"),
        "lane work"
    );
    assert_eq!(
        show_at(
            fixture.root(),
            merge.new_head.as_str(),
            "kitty-specs/demo/spec.md"
        ),
        "# Spec\n\nreal content"
    );
}

#[test]
fn unrelated_histories_fail_the_loop_instead_of_leaving_it_unintegrated() {
    // The reference runs `git merge`, git refuses an unrelated-history merge,
    // and the loop aborts and resets to its pre-loop ref (#1915). Reporting a
    // mutation-free "MergeRequired" receipt instead let an EARLIER dependency's
    // advance survive, the opposite of all-or-nothing.
    let holder = planning_fixture(46);
    let Fixture { fixture, slug, id } = &holder;
    let planning = planning_commit(&holder);
    let (_dir, manifest) = lanes_manifest(
        fixture,
        slug,
        id,
        serde_json::json!([lane("lane-a", &["WP01"], &[], 1)]),
    );
    // An unrelated root commit: no common ancestor with the planning lineage.
    let tree = support::git(fixture.root(), &["write-tree"]);
    let orphan = support::git(fixture.root(), &["commit-tree", &tree, "-m", "orphan"]);
    support::git(
        fixture.root(),
        &["branch", "kitty/mission-demo-lane-a", &orphan],
    );
    add_worktree(fixture, slug, "lane-a", "kitty/mission-demo-lane-a");
    let workspace = workspace(fixture, slug);
    let failure =
        integrate_planning_commit(&workspace, &manifest, "lane-a", &planning, &unprotected())
            .expect_err("an unrelated-history merge cannot be integrated");
    assert!(
        matches!(
            failure.error.as_ref(),
            WorkspaceError::CoordRefused {
                code: "INTEGRATION_UNRELATED_HISTORIES",
                ..
            }
        ),
        "{failure}"
    );
    assert_eq!(
        branch_tip(fixture.root(), "kitty/mission-demo-lane-a"),
        orphan,
        "the failed step itself still mutated nothing"
    );
    assert!(
        failure.report.rollback.is_none(),
        "nothing advanced, so there is nothing to rewind"
    );
    // The journal records the abandoned attempt with its receipts.
    let records = CoordJournal::new(workspace.workspace().common_dir(), slug)
        .records()
        .unwrap();
    let record = records
        .iter()
        .find(|record| record.kind == CoordRecordKind::Integration)
        .expect("an integration record");
    assert_eq!(record.phase, CoordPhase::Abandoned);
}

/// A dependency lane whose history is unrelated to the dependent lane.
#[test]
fn a_later_unrelated_dependency_fails_and_rewinds_earlier_advances() {
    let Fixture { fixture, slug, id } = planning_fixture(52);
    let (_dir, manifest) = lanes_manifest(
        &fixture,
        slug,
        &id,
        serde_json::json!([
            lane("lane-a", &["WP01"], &[], 1),
            lane("lane-c", &["WP03"], &[], 2),
            lane("lane-b", &["WP02"], &["lane-a", "lane-c"], 1),
        ]),
    );
    let base = branch_tip(fixture.root(), "work");
    support::git(
        fixture.root(),
        &["branch", "kitty/mission-demo-lane-b", &base],
    );
    add_worktree(&fixture, slug, "lane-b", "kitty/mission-demo-lane-b");
    // lane-a fast-forwards the dependent lane.
    support::git(
        fixture.root(),
        &["checkout", "-b", "kitty/mission-demo-lane-a", &base],
    );
    write(&fixture.path("dep.txt"), "dep\n");
    commit_file(fixture.root(), "dep.txt", "dependency work");
    let lane_a_tip = branch_tip(fixture.root(), "kitty/mission-demo-lane-a");
    // lane-c is an unrelated root: git refuses the merge.
    let tree = support::git(fixture.root(), &["write-tree"]);
    let orphan = support::git(fixture.root(), &["commit-tree", &tree, "-m", "orphan"]);
    support::git(
        fixture.root(),
        &["branch", "kitty/mission-demo-lane-c", &orphan],
    );
    support::git(fixture.root(), &["checkout", "work"]);
    let workspace = workspace(&fixture, slug);
    let manifest = with_dependencies(manifest, "lane-b", &["lane-a", "lane-c"]);
    let failure = integrate_dependency_lanes(&workspace, &manifest, "lane-b", &unprotected())
        .expect_err("the unrelated dependency fails the loop");
    assert!(
        matches!(
            failure.error.as_ref(),
            WorkspaceError::CoordRefused {
                code: "INTEGRATION_UNRELATED_HISTORIES",
                ..
            }
        ),
        "{failure}"
    );
    let rollback = failure.report.rollback.as_ref().expect("a reported rewind");
    assert_eq!(rollback.from.to_string(), lane_a_tip);
    assert_eq!(rollback.to.to_string(), base);
    assert_eq!(
        branch_tip(fixture.root(), "kitty/mission-demo-lane-b"),
        base,
        "all-or-nothing: the earlier advance was undone"
    );
    let applied: Vec<&str> = failure
        .report
        .applied()
        .iter()
        .map(|entry| entry.lane_id.as_str())
        .collect();
    assert_eq!(
        applied,
        vec!["lane-a"],
        "the earlier advance stays in the receipts"
    );
}

#[test]
fn a_landed_integration_journals_its_applied_outcome() {
    let Fixture { fixture, slug, id } = planning_fixture(59);
    let (_dir, manifest) = lanes_manifest(
        &fixture,
        slug,
        &id,
        serde_json::json!([
            lane("lane-a", &["WP01"], &[], 1),
            lane("lane-b", &["WP02"], &["lane-a"], 2),
        ]),
    );
    let base = branch_tip(fixture.root(), "work");
    support::git(
        fixture.root(),
        &["branch", "kitty/mission-demo-lane-a", &base],
    );
    support::git(
        fixture.root(),
        &["branch", "kitty/mission-demo-lane-b", &base],
    );
    add_worktree(&fixture, slug, "lane-b", "kitty/mission-demo-lane-b");
    support::git(fixture.root(), &["checkout", "kitty/mission-demo-lane-a"]);
    write(&fixture.path("dep.txt"), "dep\n");
    commit_file(fixture.root(), "dep.txt", "dependency work");
    support::git(fixture.root(), &["checkout", "work"]);
    let workspace = workspace(&fixture, slug);
    let manifest = with_dependencies(manifest, "lane-b", &["lane-a"]);
    let report = integrate_dependency_lanes(&workspace, &manifest, "lane-b", &unprotected())
        .expect("the fast-forward lands");
    assert!(report.is_complete(), "{report:?}");
    assert!(
        report.journal_fault.is_none(),
        "a healthy journal leaves no fault: {report:?}"
    );
    assert!(
        report.journal_path.exists(),
        "the receipt names the journal document it wrote"
    );
    // The resolve is a real step: nothing is left pending, and the record
    // carries the applied outcome rather than being dropped.
    let records = CoordJournal::new(workspace.workspace().common_dir(), slug)
        .records()
        .unwrap();
    let record = records
        .iter()
        .find(|record| record.kind == CoordRecordKind::Integration)
        .expect("an integration record");
    assert_eq!(record.phase, CoordPhase::Applied);
}

#[test]
fn an_unreadable_journal_stops_the_loop_before_any_mutation() {
    let Fixture { fixture, slug, id } = planning_fixture(60);
    let (_dir, manifest) = lanes_manifest(
        &fixture,
        slug,
        &id,
        serde_json::json!([
            lane("lane-a", &["WP01"], &[], 1),
            lane("lane-b", &["WP02"], &["lane-a"], 2),
        ]),
    );
    let base = branch_tip(fixture.root(), "work");
    support::git(
        fixture.root(),
        &["branch", "kitty/mission-demo-lane-a", &base],
    );
    support::git(
        fixture.root(),
        &["branch", "kitty/mission-demo-lane-b", &base],
    );
    add_worktree(&fixture, slug, "lane-b", "kitty/mission-demo-lane-b");
    support::git(fixture.root(), &["checkout", "kitty/mission-demo-lane-a"]);
    write(&fixture.path("dep.txt"), "dep\n");
    commit_file(fixture.root(), "dep.txt", "dependency work");
    let dep_tip = branch_tip(fixture.root(), "kitty/mission-demo-lane-a");
    support::git(fixture.root(), &["checkout", "work"]);
    let workspace = workspace(&fixture, slug);
    let manifest = with_dependencies(manifest, "lane-b", &["lane-a"]);
    // A journal that cannot be written: the intent cannot become durable.
    let journal = CoordJournal::new(workspace.workspace().common_dir(), slug);
    std::fs::create_dir_all(journal.path()).unwrap();
    let failure = integrate_dependency_lanes(&workspace, &manifest, "lane-b", &unprotected())
        .expect_err("no durable intent, no mutation");
    assert!(failure.report.integrations.is_empty(), "{failure}");
    assert_eq!(
        branch_tip(fixture.root(), "kitty/mission-demo-lane-b"),
        base,
        "nothing advanced without a durable intent"
    );
    assert!(dep_tip != base);
}

#[test]
fn a_hook_restored_merge_marker_is_never_aborted_or_rewound() {
    // `sk-git` reports `MergeConclusionUnconfirmed` when a post-commit hook
    // re-creates MERGE_HEAD after the commit landed. The merge is real: the
    // destination tip IS the created commit. Aborting (the old blind
    // `merge_abort`) would destroy it, and reporting an ordinary retryable
    // failure would claim the repository is unchanged.
    let holder = planning_fixture(61);
    let Fixture { fixture, slug, id } = &holder;
    let planning = planning_commit(&holder);
    let (_dir, manifest) = lanes_manifest(
        fixture,
        slug,
        id,
        serde_json::json!([lane("lane-a", &["WP01"], &[], 1)]),
    );
    support::git(fixture.root(), &["checkout", "main"]);
    support::git(
        fixture.root(),
        &["checkout", "-b", "kitty/mission-demo-lane-a"],
    );
    write(&fixture.path("lane.txt"), "lane work\n");
    commit_all(fixture.root(), "lane work");
    support::git(fixture.root(), &["checkout", "work"]);
    add_worktree(fixture, slug, "lane-a", "kitty/mission-demo-lane-a");
    let lane_worktree = fixture.root().join(".worktrees").join("demo-lane-a");
    // Re-create the operation marker after the merge commit lands.
    let hooks = fixture.root().join(".git/hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    let hook = hooks.join("post-commit");
    write(
        &hook,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' '{}' > \"$(git rev-parse --git-path MERGE_HEAD)\"\n",
            planning
        ),
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let workspace = workspace(fixture, slug);
    let before = branch_tip(fixture.root(), "kitty/mission-demo-lane-a");
    let failure =
        integrate_planning_commit(&workspace, &manifest, "lane-a", &planning, &unprotected())
            .expect_err("the marker makes the conclusion unconfirmed");
    assert!(
        matches!(
            failure.error.as_ref(),
            WorkspaceError::CoordRefused {
                code: "INTEGRATION_MERGE_UNCONFIRMED",
                ..
            }
        ),
        "{failure}"
    );
    let evidence = failure
        .unconfirmed
        .as_ref()
        .expect("the unconfirmed evidence is retained");
    assert!(evidence.landed, "{evidence:?}");
    let candidate = evidence.candidate.clone().expect("a proven candidate");
    assert_eq!(
        branch_tip(fixture.root(), "kitty/mission-demo-lane-a"),
        candidate.to_string(),
        "the landed merge is preserved, not rewound"
    );
    assert_ne!(
        branch_tip(fixture.root(), "kitty/mission-demo-lane-a"),
        before
    );
    assert!(
        !evidence.operation_consumed,
        "the hook restored the marker: {evidence:?}"
    );
    assert!(
        lane_worktree.join(".git").exists(),
        "the integration checkout is intact"
    );
    let merge_head = support::git(&lane_worktree, &["rev-parse", "--git-path", "MERGE_HEAD"]);
    assert!(
        std::path::Path::new(&merge_head).exists()
            || lane_worktree.join(merge_head.clone()).exists()
            || fixture.root().join(&merge_head).exists(),
        "the restored marker was left in place, never aborted: {merge_head}"
    );
    assert!(
        failure.report.rollback.is_none(),
        "an unconfirmed landing is never rewound"
    );
    let display = failure.to_string();
    assert!(
        !display.contains("rolled"),
        "the failure never claims a rollback: {display}"
    );
    // The receipt still names the merge that landed.
    assert!(matches!(
        failure
            .report
            .integrations
            .last()
            .map(|entry| &entry.outcome),
        Some(IntegrationOutcome::Merged(_))
    ));
}

#[test]
fn dependency_lane_tips_fast_forward_into_a_dependent_lane() {
    let Fixture { fixture, slug, id } = planning_fixture(47);
    let (_dir, manifest) = lanes_manifest(
        &fixture,
        slug,
        &id,
        serde_json::json!([
            lane("lane-a", &["WP01"], &[], 1),
            lane("lane-b", &["WP02"], &["lane-a"], 2),
        ]),
    );
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
    add_worktree(&fixture, slug, "lane-b", "kitty/mission-demo-lane-b");
    support::git(fixture.root(), &["checkout", "kitty/mission-demo-lane-a"]);
    write(&fixture.path("dep.txt"), "dep\n");
    commit_file(fixture.root(), "dep.txt", "dependency work");
    let dep_tip = branch_tip(fixture.root(), "kitty/mission-demo-lane-a");
    support::git(fixture.root(), &["checkout", "work"]);

    let workspace = workspace(&fixture, slug);
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
    assert_eq!(
        show_at(fixture.root(), &dep_tip, "dep.txt"),
        "dep",
        "the dependency commit is real"
    );
}

#[test]
fn a_missing_dependency_branch_is_skipped_with_a_receipt() {
    let Fixture { fixture, slug, id } = planning_fixture(48);
    let (_dir, manifest) = lanes_manifest(
        &fixture,
        slug,
        &id,
        serde_json::json!([
            lane("lane-a", &["WP01"], &[], 1),
            lane("lane-b", &["WP02"], &["lane-a"], 2),
        ]),
    );
    let base = branch_tip(fixture.root(), "work");
    support::git(
        fixture.root(),
        &["branch", "kitty/mission-demo-lane-b", &base],
    );
    add_worktree(&fixture, slug, "lane-b", "kitty/mission-demo-lane-b");
    // No `kitty/mission-demo-lane-a` branch exists: the reference warns and
    // continues, it does not fail the lane.
    let workspace = workspace(&fixture, slug);
    let report =
        integrate_dependency_lanes(&workspace, &manifest, "lane-b", &unprotected()).unwrap();
    assert!(report.is_complete());
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(report.skipped[0].lane_id, "lane-a");
    assert_eq!(report.skipped[0].branch, "kitty/mission-demo-lane-a");
    assert!(report.skipped[0].reason.contains("does not resolve"));
    assert_eq!(
        branch_tip(fixture.root(), "kitty/mission-demo-lane-b"),
        base,
        "a skipped dependency mutates nothing"
    );
}

#[test]
fn dependencies_run_in_parallel_group_then_lane_order() {
    let Fixture { fixture, slug, id } = planning_fixture(49);
    // Declared out of order on purpose; all three branches are missing so the
    // skip list observes the resolution order without needing real commits.
    let (_dir, manifest) = lanes_manifest(
        &fixture,
        slug,
        &id,
        serde_json::json!([
            lane("lane-d", &["WP04"], &[], 1),
            lane("lane-a", &["WP01"], &[], 3),
            lane("lane-b", &["WP02"], &[], 2),
            lane("lane-c", &["WP03"], &[], 2),
        ]),
    );
    let base = branch_tip(fixture.root(), "work");
    support::git(
        fixture.root(),
        &["branch", "kitty/mission-demo-lane-d", &base],
    );
    add_worktree(&fixture, slug, "lane-d", "kitty/mission-demo-lane-d");
    let manifest = with_dependencies(manifest, "lane-d", &["lane-b", "lane-a", "lane-c"]);
    let workspace = workspace(&fixture, slug);
    let report =
        integrate_dependency_lanes(&workspace, &manifest, "lane-d", &unprotected()).unwrap();
    let order: Vec<&str> = report
        .skipped
        .iter()
        .map(|entry| entry.lane_id.as_str())
        .collect();
    assert_eq!(order, vec!["lane-b", "lane-c", "lane-a"]);
}

/// Build one dependent lane and return the workspace plus its manifest.
fn dependency_conflict_setup(
    tail: u8,
) -> (
    Fixture,
    sk_workspace::MissionWorkspace,
    sk_workspace::LanesManifest,
) {
    let holder = planning_fixture(tail);
    let Fixture { fixture, slug, id } = &holder;
    let (_dir, manifest) = lanes_manifest(
        fixture,
        slug,
        id,
        serde_json::json!([
            lane("lane-a", &["WP01"], &[], 1),
            lane("lane-c", &["WP03"], &[], 2),
            lane("lane-b", &["WP02"], &["lane-a", "lane-c"], 1),
        ]),
    );
    let base = branch_tip(fixture.root(), "work");
    // lane-b takes its own commit first.
    support::git(
        fixture.root(),
        &["branch", "kitty/mission-demo-lane-b", &base],
    );
    add_worktree(fixture, slug, "lane-b", "kitty/mission-demo-lane-b");
    let lane_b_path = fixture.root().join(".worktrees").join("demo-lane-b");
    write(&lane_b_path.join("shared.txt"), "b\n");
    commit_file(&lane_b_path, "shared.txt", "lane b");
    let lane_b_tip = branch_tip(fixture.root(), "kitty/mission-demo-lane-b");
    // lane-a is a child of lane-b's tip, so integrating it fast-forwards.
    support::git(
        fixture.root(),
        &["checkout", "-b", "kitty/mission-demo-lane-a", &lane_b_tip],
    );
    write(&fixture.path("own-a.txt"), "a\n");
    commit_file(fixture.root(), "own-a.txt", "lane a");
    support::git(fixture.root(), &["checkout", "work"]);
    // lane-c diverges from the shared base on the same file lane-b changed, so
    // merging it after the clean fast-forward conflicts.
    support::git(
        fixture.root(),
        &["branch", "kitty/mission-demo-lane-c", &base],
    );
    support::git(fixture.root(), &["checkout", "kitty/mission-demo-lane-c"]);
    write(&fixture.path("shared.txt"), "c\n");
    commit_file(fixture.root(), "shared.txt", "lane c");
    support::git(fixture.root(), &["checkout", "work"]);
    let workspace = workspace(fixture, slug);
    let manifest = with_dependencies(manifest, "lane-b", &["lane-a", "lane-c"]);
    (holder, workspace, manifest)
}

#[test]
fn a_later_conflict_rewinds_every_earlier_dependency_advance() {
    let (holder, workspace, manifest) = dependency_conflict_setup(50);
    let Fixture { fixture, slug, .. } = &holder;
    let lane_b_before = branch_tip(fixture.root(), "kitty/mission-demo-lane-b");
    let lane_a_tip = branch_tip(fixture.root(), "kitty/mission-demo-lane-a");
    let failure = integrate_dependency_lanes(&workspace, &manifest, "lane-b", &unprotected())
        .expect_err("the second dependency conflicts");
    // The step's own typed code is preserved (it used to be flattened into a
    // generic `INTEGRATION_FAILED`), and the receipts still carry the rewind.
    assert!(
        matches!(
            failure.error.as_ref(),
            WorkspaceError::CoordRefused {
                code: "INTEGRATION_MERGE_CONFLICT",
                ..
            }
        ),
        "{failure}"
    );
    let rollback = failure.report.rollback.as_ref().expect("a reported rewind");
    assert_eq!(rollback.from.to_string(), lane_a_tip);
    assert_eq!(rollback.to.to_string(), lane_b_before);
    // The earlier clean advance is reported (not silently dropped)…
    let applied: Vec<&str> = failure
        .report
        .applied()
        .iter()
        .map(|entry| entry.lane_id.as_str())
        .collect();
    assert_eq!(applied, vec!["lane-a"]);
    // …and it was actually undone: all-or-nothing.
    assert_eq!(
        branch_tip(fixture.root(), "kitty/mission-demo-lane-b"),
        lane_b_before
    );
    let displayed = failure.to_string();
    assert!(displayed.contains("rolled"), "{displayed}");
    // The journal records the failure and the rewind target.
    let records = CoordJournal::new(workspace.workspace().common_dir(), slug)
        .records()
        .unwrap();
    let record = records
        .iter()
        .find(|record| record.kind == CoordRecordKind::Integration)
        .expect("an integration record");
    assert_eq!(record.phase, CoordPhase::Abandoned);
    assert!(
        record
            .detail
            .as_deref()
            .unwrap_or("")
            .contains("rolled back")
    );
}

#[test]
fn integration_resolves_the_lane_branch_through_the_one_naming_grammar() {
    // N6: a pure legacy `NNN-` slug keeps its prefix on the lane branch. The
    // dependency loop must look for the branch that naming actually creates, or
    // a satisfied dependency reads as missing.
    let fixture = init_repo();
    let id = mission_id(51);
    let slug = "057-lanehead";
    create_branch(fixture.root(), "work");
    support::git(fixture.root(), &["checkout", "work"]);
    write_mission_with(
        fixture.root(),
        slug,
        &id,
        "lanes",
        "work",
        serde_json::json!({}),
    );
    commit_file(
        fixture.root(),
        "kitty-specs/057-lanehead/meta.json",
        "scaffold",
    );
    let branch = sk_workspace::code_lane_branch_name(slug, "lane-a").unwrap();
    assert_eq!(branch, "kitty/mission-057-lanehead-lane-a");
    let (_dir, manifest) = lanes_manifest(
        &fixture,
        slug,
        &id,
        serde_json::json!([
            lane("lane-a", &["WP01"], &[], 1),
            lane("lane-b", &["WP02"], &["lane-a"], 2),
        ]),
    );
    let base = branch_tip(fixture.root(), "work");
    support::git(fixture.root(), &["branch", &branch, &base]);
    support::git(
        fixture.root(),
        &["branch", "kitty/mission-057-lanehead-lane-b", &base],
    );
    support::git(
        fixture.root(),
        &[
            "worktree",
            "add",
            fixture
                .root()
                .join(".worktrees/057-lanehead-lane-b")
                .to_str()
                .unwrap(),
            "kitty/mission-057-lanehead-lane-b",
        ],
    );
    support::git(fixture.root(), &["checkout", &branch]);
    write(&fixture.path("dep.txt"), "dep\n");
    commit_file(fixture.root(), "dep.txt", "dependency work");
    let dep_tip = branch_tip(fixture.root(), &branch);
    support::git(fixture.root(), &["checkout", "work"]);
    let workspace = workspace(&fixture, slug);
    let report =
        integrate_dependency_lanes(&workspace, &manifest, "lane-b", &unprotected()).unwrap();
    assert!(report.skipped.is_empty(), "{report:?}");
    assert_eq!(
        branch_tip(fixture.root(), "kitty/mission-057-lanehead-lane-b"),
        dep_tip
    );
}

#[test]
fn a_dependency_on_the_planning_lane_resolves_to_the_target_branch() {
    // R2: upstream resolves every dependency through the one lane grammar
    // (`lane_branch_name(mission_slug, lane_id, target_branch=…)`), which
    // returns the manifest's target branch for `lane-planning`. Pairing that
    // with `code_lane_branch_name` — which refuses the planning lane by design —
    // failed a dependency the reference merges from the target branch.
    let Fixture { fixture, slug, id } = planning_fixture(62);
    let (_dir, manifest) = lanes_manifest(
        &fixture,
        slug,
        &id,
        serde_json::json!([
            lane("lane-planning", &["WP01"], &[], 1),
            lane("lane-a", &["WP02"], &["lane-planning"], 2),
        ]),
    );
    let base = branch_tip(fixture.root(), "work");
    support::git(
        fixture.root(),
        &["branch", "kitty/mission-demo-lane-a", &base],
    );
    add_worktree(&fixture, slug, "lane-a", "kitty/mission-demo-lane-a");
    // The planning lane has no branch of its own: it *is* the target branch, and
    // the target moved on after the lane was cut.
    write(&fixture.path("plan.txt"), "planning work\n");
    commit_file(fixture.root(), "plan.txt", "planning work");
    let target_tip = branch_tip(fixture.root(), "work");
    assert_ne!(target_tip, base);
    assert_eq!(
        sk_workspace::lane_branch_name(slug, "lane-planning", "work").unwrap(),
        "work"
    );
    let workspace = workspace(&fixture, slug);
    let report = integrate_dependency_lanes(&workspace, &manifest, "lane-a", &unprotected())
        .expect("a planning-lane dependency is integrable");
    assert!(
        report.skipped.is_empty(),
        "the planning lane is not a missing branch: {report:?}"
    );
    assert_eq!(report.integrations.len(), 1);
    assert_eq!(
        report.integrations[0].branch, "work",
        "the dependency resolves to the target branch: {report:?}"
    );
    assert_eq!(
        branch_tip(fixture.root(), "kitty/mission-demo-lane-a"),
        target_tip,
        "the lane fast-forwards to the planning lane's target tip"
    );
}
