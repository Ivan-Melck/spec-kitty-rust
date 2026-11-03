//! Real coordination transactions against temporary repositories.
//!
//! Every fixture is a genuine Git repository. The expectations were also checked
//! against the pinned Python oracle's coordination surface probe and policy
//! resolution on equivalent temporary repositories (see `COORDINATION-REPORT.md`).

mod support;

use std::path::Path;

use sk_git::Oid;
use sk_workspace::{
    COORD_SEED_TRAILER, CoordCommitOutcome, CoordJournal, CoordPhase, CoordRecord, CoordRecordKind,
    CoordState, CoordTransaction, MissionWorkspace, ProjectWorkspace, ProtectionPolicy,
    TopologySurface, WorkspaceError, read_coordination_surface, recover_interrupted,
};

use support::{
    branch_tip, commit_all, create_branch, init_repo, mission_id, show_at, trailer, write,
    write_mission_with,
};

struct CoordFixture {
    fixture: support::Fixture,
    workspace: MissionWorkspace,
    slug: &'static str,
    mission_id: String,
    coord_branch: String,
}

fn project(fixture: &support::Fixture, slug: &str) -> MissionWorkspace {
    ProjectWorkspace::open(fixture.root())
        .expect("open project")
        .mission(slug)
        .expect("mission")
}

/// A coordination-routed mission whose coordination branch predates the
/// scaffold: the branch is minted from `main`, then the mission directory is
/// written, exactly like `mission create`.
fn coord_fixture(tail: u8, commit_scaffold: bool) -> CoordFixture {
    let fixture = init_repo();
    let id = mission_id(tail);
    let slug = "demo";
    let coord_branch = format!("kitty/mission-{slug}");
    create_branch(fixture.root(), &coord_branch);
    let dir = write_mission_with(
        fixture.root(),
        slug,
        &id,
        "coord",
        "main",
        serde_json::json!({"coordination_branch": coord_branch}),
    );
    write(
        &dir.join("status.events.jsonl"),
        "{\"event_id\": \"01KNXQS9ATWWFXS3K5ZJ9E5001\"}\n",
    );
    write(
        &dir.join("decisions.events.jsonl"),
        "# Decisions\n\n- keep this\n",
    );
    write(&dir.join("spec.md"), "# Spec\n");
    if commit_scaffold {
        commit_all(fixture.root(), "scaffold");
    }
    let workspace = project(&fixture, slug);
    CoordFixture {
        fixture,
        workspace,
        slug,
        mission_id: id,
        coord_branch,
    }
}

fn policy(fixture: &support::Fixture) -> ProtectionPolicy {
    ProtectionPolicy::resolve_with_hatch(fixture.root(), None).expect("policy")
}

#[test]
fn a_coord_less_mission_writes_its_unprotected_target() {
    let fixture = init_repo();
    let id = mission_id(10);
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
    commit_all(fixture.root(), "scaffold");
    let workspace = project(&fixture, slug);
    let mut txn =
        CoordTransaction::acquire(&workspace, "status_append", &policy(&fixture)).unwrap();
    txn.write_artifact(
        Path::new("kitty-specs/demo/decisions.events.jsonl"),
        b"# Decisions\n\n- native write\n",
    )
    .unwrap();
    let receipt = txn.commit("chore(spec-kitty): record decision").unwrap();
    let landed = receipt.outcome.landed().expect("landed commit");
    assert_eq!(receipt.proof.surface, TopologySurface::Primary);
    assert_eq!(receipt.proof.destination_ref, "work");
    assert_eq!(receipt.proof.mission_lock_key, slug);
    assert!(
        receipt
            .proof
            .mission_lock_path
            .as_ref()
            .unwrap()
            .ends_with("demo.status.lock")
    );
    assert_ne!(
        receipt.proof.mission_lock_key,
        workspace.canonical_mission_dir_name()
    );
    assert!(receipt.proof.checkout_claim_lock.exists());
    assert_eq!(
        show_at(
            fixture.root(),
            landed.commit.as_str(),
            "kitty-specs/demo/decisions.events.jsonl"
        ),
        "# Decisions\n\n- native write"
    );
    let _ = dir;
}

#[test]
fn a_coord_less_mission_on_a_protected_target_refuses_before_writing() {
    let fixture = init_repo();
    let id = mission_id(24);
    let slug = "demo";
    let dir = write_mission_with(
        fixture.root(),
        slug,
        &id,
        "lanes",
        "main",
        serde_json::json!({}),
    );
    commit_all(fixture.root(), "scaffold");
    let workspace = project(&fixture, slug);
    let error = CoordTransaction::acquire(&workspace, "status_append", &policy(&fixture))
        .expect_err("refusal");
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
    assert!(!fixture.root().join(".spec-kitty").exists());
    let _ = dir;
}

#[test]
fn an_unmaterialized_coord_surface_is_attached_then_seeded_and_committed() {
    let CoordFixture {
        fixture,
        workspace,
        slug,
        mission_id,
        coord_branch,
    } = coord_fixture(11, false);
    let mut txn =
        CoordTransaction::acquire(&workspace, "status_append", &policy(&fixture)).unwrap();
    assert_eq!(txn.state_before(), CoordState::Unmaterialized);
    assert_eq!(txn.surface(), TopologySurface::Coord);
    let coord_root = workspace.coord_worktree_path().unwrap();
    assert!(
        coord_root.exists(),
        "the canonical worktree was materialized"
    );
    let seed = txn.seed().expect("seed receipt");
    assert_eq!(seed.state_before, CoordState::Unmaterialized);
    let mut seeded = seed.seeded_paths.clone();
    seeded.sort();
    assert_eq!(
        seeded,
        vec![
            "decisions.events.jsonl".to_owned(),
            "status.events.jsonl".to_owned()
        ],
        "only COORD-partition artifacts are carried"
    );
    let seed_commit = seed.landed.as_ref().expect("seed commit");
    assert_eq!(
        trailer(
            fixture.root(),
            seed_commit.commit.as_str(),
            COORD_SEED_TRAILER
        ),
        mission_id
    );
    assert_eq!(
        show_at(
            fixture.root(),
            seed_commit.commit.as_str(),
            &format!(
                "kitty-specs/{}-{}/decisions.events.jsonl",
                slug,
                &mission_id[..8]
            )
        ),
        "# Decisions\n\n- keep this"
    );

    txn.write_artifact(
        Path::new("kitty-specs/demo-01KNXQS9/status.json"),
        b"{\"materialized\": true}\n",
    )
    .unwrap();
    let receipt = txn.commit("chore(spec-kitty): append status").unwrap();
    let landed = receipt.outcome.landed().expect("landed");
    assert_eq!(landed.destination, coord_branch);
    assert_eq!(receipt.proof.surface, TopologySurface::Coord);
    assert_eq!(receipt.proof.worktree_root, coord_root);
    assert_eq!(
        receipt.proof.worktree_branch.as_deref(),
        Some(coord_branch.as_str())
    );
}

#[test]
fn a_materialized_coord_surface_is_reused_without_reseeding() {
    let CoordFixture {
        fixture,
        workspace,
        coord_branch,
        ..
    } = coord_fixture(12, true);
    // Materialize once.
    {
        let _txn = CoordTransaction::acquire(&workspace, "bootstrap", &policy(&fixture)).unwrap();
    }
    let tip_after_first = branch_tip(fixture.root(), &coord_branch);
    let txn = CoordTransaction::acquire(&workspace, "status_append", &policy(&fixture)).unwrap();
    assert_eq!(txn.state_before(), CoordState::Materialized);
    assert!(
        txn.seed().is_none(),
        "a materialized surface is not seeded again"
    );
    drop(txn);
    assert_eq!(
        branch_tip(fixture.root(), &coord_branch),
        tip_after_first,
        "dropping an uncommitted transaction writes no commit"
    );
}

#[test]
fn reading_an_empty_surface_degrades_to_primary_without_mutating() {
    let CoordFixture {
        fixture,
        workspace,
        mission_id,
        ..
    } = coord_fixture(13, true);
    // Materialize, then remove the Mission directory: the EMPTY window.
    {
        let _txn = CoordTransaction::acquire(&workspace, "bootstrap", &policy(&fixture)).unwrap();
    }
    let coord_root = workspace.coord_worktree_path().unwrap();
    let coord_dir = coord_root
        .join("kitty-specs")
        .join(format!("demo-{}", &mission_id[..8]));
    std::fs::remove_dir_all(&coord_dir).unwrap();
    assert!(!coord_dir.exists());

    let read = read_coordination_surface(&workspace).unwrap();
    assert_eq!(read.state, CoordState::Empty);
    assert!(read.degraded_to_primary);
    assert_eq!(read.read_dir, workspace.mission_dir());
    assert!(
        !coord_dir.exists(),
        "a read must not create the coordination Mission directory"
    );
}

#[test]
fn seeding_after_a_lost_directory_refuses_rather_than_forking() {
    let CoordFixture {
        fixture,
        workspace,
        mission_id,
        ..
    } = coord_fixture(14, true);
    {
        let _txn = CoordTransaction::acquire(&workspace, "bootstrap", &policy(&fixture)).unwrap();
    }
    let coord_root = workspace.coord_worktree_path().unwrap();
    std::fs::remove_dir_all(
        coord_root
            .join("kitty-specs")
            .join(format!("demo-{}", &mission_id[..8])),
    )
    .unwrap();
    let error = CoordTransaction::acquire(&workspace, "status_append", &policy(&fixture))
        .expect_err("refusal");
    match error {
        WorkspaceError::CoordRefused { code, .. } => {
            assert!(
                code == "COORD_SURFACE_RESTORE_REQUIRED" || code == "COORD_SEED_FORK_REFUSED",
                "{code}"
            );
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn a_protected_coordination_branch_refuses_before_materializing() {
    let CoordFixture {
        fixture, workspace, ..
    } = coord_fixture(15, false);
    write(
        &fixture.root().join(".kittify/config.yaml"),
        "protection:\n  protected_branches: [kitty/mission-demo]\n",
    );
    let error = CoordTransaction::acquire(&workspace, "status_append", &policy(&fixture))
        .expect_err("refusal");
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
    assert!(
        !workspace.coord_worktree_path().unwrap().exists(),
        "a refusal must happen before any worktree mutation"
    );
}

#[test]
fn a_deleted_coordination_branch_refuses() {
    let CoordFixture {
        fixture,
        workspace,
        coord_branch,
        ..
    } = coord_fixture(16, false);
    support::git(fixture.root(), &["branch", "-D", &coord_branch]);
    let error = CoordTransaction::acquire(&workspace, "status_append", &policy(&fixture))
        .expect_err("refusal");
    assert!(
        matches!(error, WorkspaceError::CoordinationBranchDeleted { .. }),
        "{error}"
    );
}

#[test]
fn an_empty_transaction_refuses_instead_of_committing() {
    let CoordFixture {
        fixture, workspace, ..
    } = coord_fixture(17, true);
    let txn = CoordTransaction::acquire(&workspace, "status_append", &policy(&fixture)).unwrap();
    let error = txn.commit("chore(spec-kitty): empty").expect_err("refusal");
    assert!(
        matches!(
            error,
            WorkspaceError::CoordRefused {
                code: "COORD_EMPTY_CHANGESET",
                ..
            }
        ),
        "{error}"
    );
}

#[test]
fn rewriting_identical_content_is_a_proven_noop() {
    let CoordFixture {
        fixture, workspace, ..
    } = coord_fixture(18, true);
    let path = Path::new("kitty-specs/demo-01KNXQS9/decisions.events.jsonl");
    {
        let mut txn =
            CoordTransaction::acquire(&workspace, "status_append", &policy(&fixture)).unwrap();
        txn.write_artifact(path, b"# Decisions\n\n- once\n")
            .unwrap();
        let receipt = txn.commit("chore(spec-kitty): record").unwrap();
        assert!(receipt.outcome.landed().is_some());
    }
    let mut txn =
        CoordTransaction::acquire(&workspace, "status_append", &policy(&fixture)).unwrap();
    txn.write_artifact(path, b"# Decisions\n\n- once\n")
        .unwrap();
    let receipt = txn.commit("chore(spec-kitty): record again").unwrap();
    assert!(matches!(receipt.outcome, CoordCommitOutcome::NoOp { .. }));
    assert!(receipt.outcome.is_noop());
}

#[test]
fn an_aborted_transaction_restores_the_previous_bytes() {
    let CoordFixture {
        fixture, workspace, ..
    } = coord_fixture(19, true);
    let path = Path::new("kitty-specs/demo-01KNXQS9/decisions.events.jsonl");
    {
        let mut txn =
            CoordTransaction::acquire(&workspace, "status_append", &policy(&fixture)).unwrap();
        txn.write_artifact(path, b"# Decisions\n\n- changed\n")
            .unwrap();
        txn.abort("test abort");
    }
    let worktree = workspace.coord_worktree_path().unwrap();
    let bytes = std::fs::read_to_string(
        worktree
            .join("kitty-specs")
            .join("demo-01KNXQS9")
            .join("decisions.events.jsonl"),
    )
    .unwrap();
    assert_eq!(bytes, "# Decisions\n\n- keep this\n");
}

#[test]
fn writes_outside_the_checkout_and_into_nested_worktrees_are_refused() {
    let CoordFixture {
        fixture, workspace, ..
    } = coord_fixture(20, true);
    let mut txn =
        CoordTransaction::acquire(&workspace, "status_append", &policy(&fixture)).unwrap();
    let escape = txn
        .write_artifact(Path::new("../escape.md"), b"nope")
        .unwrap_err();
    assert!(
        matches!(escape, WorkspaceError::Kernel(_)),
        "a traversal write must be refused by the kernel confinement: {escape}"
    );
    let nested = txn
        .write_artifact(Path::new(".worktrees/other/file.md"), b"nope")
        .unwrap_err();
    assert!(
        matches!(
            nested,
            WorkspaceError::CoordRefused {
                code: "COORD_ARTIFACT_NESTED_WORKTREE",
                ..
            }
        ),
        "{nested}"
    );
}

#[test]
fn recovery_completes_a_landed_commit_and_clears_an_unlanded_one() {
    let CoordFixture {
        fixture,
        workspace,
        coord_branch,
        ..
    } = coord_fixture(21, true);
    let common = workspace.workspace().common_dir().to_path_buf();
    let journal = CoordJournal::new(&common, workspace.slug());
    let before = Oid::parse(&branch_tip(fixture.root(), &coord_branch)).unwrap();

    // A commit that landed but whose journal record was never resolved: a
    // pending intent that already names the landed candidate.
    {
        let mut txn = CoordTransaction::acquire(&workspace, "landed", &policy(&fixture)).unwrap();
        txn.write_artifact(
            Path::new("kitty-specs/demo-01KNXQS9/decisions.events.jsonl"),
            b"# Decisions\n\n- landed\n",
        )
        .unwrap();
        let receipt = txn.commit("chore(spec-kitty): landed").unwrap();
        let landed = receipt.outcome.landed().unwrap().commit.clone();
        let mut intent = CoordRecord::intent(
            CoordRecordKind::Commit,
            workspace.slug(),
            "interrupted_commit",
            Some(&coord_branch),
            Some(&workspace.coord_worktree_path().unwrap()),
            Some(&before),
        );
        intent.candidate_commit = Some(landed.to_string());
        journal.begin(intent).unwrap();
    }

    // A candidate that provably never landed.
    let intent = CoordRecord::intent(
        CoordRecordKind::Commit,
        workspace.slug(),
        "never_landed",
        Some(&coord_branch),
        Some(&workspace.coord_worktree_path().unwrap()),
        Some(&Oid::parse(&branch_tip(fixture.root(), &coord_branch)).unwrap()),
    );
    journal.begin(intent.clone()).unwrap();

    let report = recover_interrupted(&workspace).unwrap();
    assert!(report.is_clean(), "{report:?}");
    let applied: Vec<_> = report
        .repairs
        .iter()
        .filter(|repair| repair.phase == CoordPhase::Applied)
        .collect();
    let abandoned: Vec<_> = report
        .repairs
        .iter()
        .filter(|repair| repair.phase == CoordPhase::Abandoned)
        .collect();
    assert_eq!(applied.len(), 1);
    assert_eq!(abandoned.len(), 1);
    assert!(journal.pending().unwrap().is_empty());
}

#[test]
fn recovery_reports_an_indeterminate_tip_instead_of_guessing() {
    let CoordFixture {
        fixture,
        workspace,
        coord_branch,
        ..
    } = coord_fixture(22, true);
    let common = workspace.workspace().common_dir().to_path_buf();
    let journal = CoordJournal::new(&common, workspace.slug());
    let tip = Oid::parse(&branch_tip(fixture.root(), &coord_branch)).unwrap();
    // Observed tip differs from the real tip and the candidate is not the tip.
    let intent = CoordRecord::intent(
        CoordRecordKind::Commit,
        workspace.slug(),
        "ambiguous",
        Some(&coord_branch),
        Some(&workspace.coord_worktree_path().unwrap()),
        Some(&Oid::parse(&branch_tip(fixture.root(), "main")).unwrap()),
    );
    journal.begin(intent).unwrap();
    let report = recover_interrupted(&workspace).unwrap();
    assert!(!report.is_clean());
    assert_eq!(report.conflicts.len(), 1);
    assert!(report.conflicts[0].detail.contains("cannot decide"));
    assert!(
        !journal.pending().unwrap().is_empty(),
        "the record stays pending"
    );
    let _ = tip;
}

#[test]
fn the_journal_is_mission_scoped_and_durable() {
    let CoordFixture {
        fixture,
        workspace,
        coord_branch,
        ..
    } = coord_fixture(23, true);
    let common = workspace.workspace().common_dir().to_path_buf();
    let journal = CoordJournal::new(&common, workspace.slug());
    let intent = CoordRecord::intent(
        CoordRecordKind::Seed,
        workspace.slug(),
        "seed",
        Some(&coord_branch),
        Some(&workspace.coord_worktree_path().unwrap()),
        None,
    );
    journal.begin(intent.clone()).unwrap();
    let reopened = CoordJournal::new(&common, workspace.slug());
    assert_eq!(reopened.pending().unwrap().len(), 1);
    assert!(reopened.path().is_file());
    let _ = (&fixture, &workspace);
}

#[cfg(unix)]
#[test]
fn a_blocked_ref_advance_is_not_landed_and_the_write_is_restored() {
    use std::os::unix::fs::PermissionsExt;

    let CoordFixture {
        fixture, workspace, ..
    } = coord_fixture(25, true);
    let path = Path::new("kitty-specs/demo-01KNXQS9/decisions.events.jsonl");
    // Materialize first: the hook below aborts every ref update, including the
    // worktree checkout, so it is installed only once the surface is in place.
    {
        let _txn = CoordTransaction::acquire(&workspace, "bootstrap", &policy(&fixture)).unwrap();
    }
    let hook = fixture.root().join(".git/hooks/reference-transaction");
    write(
        &hook,
        "#!/bin/sh\nif [ \"$1\" = \"committed\" ]; then exit 0; fi\nexit 1\n",
    );
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();

    let mut txn =
        CoordTransaction::acquire(&workspace, "status_append", &policy(&fixture)).unwrap();
    txn.write_artifact(path, b"# Decisions\n\n- should not land\n")
        .unwrap();
    let receipt = txn.commit("chore(spec-kitty): blocked advance").unwrap();
    match &receipt.outcome {
        CoordCommitOutcome::NotLanded { observed_tip, .. } => {
            assert_eq!(observed_tip, &receipt.proof.observed_tip);
        }
        other => panic!("expected NotLanded, got {other:?}"),
    }
    // The transaction's own write is restored: the branch never moved, so the
    // file must not keep content that exists in no commit.
    let worktree = workspace.coord_worktree_path().unwrap();
    let bytes = std::fs::read_to_string(
        worktree
            .join("kitty-specs")
            .join("demo-01KNXQS9")
            .join("decisions.events.jsonl"),
    )
    .unwrap();
    assert_eq!(bytes, "# Decisions\n\n- keep this\n");
}

#[test]
fn bootstrap_mints_the_coordination_branch_and_seeds_its_surface() {
    let fixture = init_repo();
    let id = mission_id(26);
    let slug = "demo";
    let coord_branch = format!("kitty/mission-{slug}");
    // The manifest declares the branch before it exists: exactly the create
    // window the window between the scaffold write and the surface bootstrap.
    write_mission_with(
        fixture.root(),
        slug,
        &id,
        "coord",
        "main",
        serde_json::json!({"coordination_branch": coord_branch}),
    );
    write(
        &fixture.path("kitty-specs/demo/status.json"),
        "{\"wps\": {}}\n",
    );
    let workspace = project(&fixture, slug);
    let target_tip = branch_tip(fixture.root(), "main");
    let receipt = sk_workspace::bootstrap_coordination(&workspace, &policy(&fixture)).unwrap();
    assert!(receipt.branch_created);
    assert_eq!(
        receipt.creation_base.as_ref().map(ToString::to_string),
        Some(target_tip)
    );
    assert_eq!(receipt.branch, coord_branch);
    let seed = receipt.seed.as_ref().expect("seed receipt");
    assert!(seed.landed.is_some());
    assert_eq!(receipt.proof.surface, TopologySurface::Coord);
    // The surface is now materialized, and the journal has no pending record.
    assert_eq!(
        read_coordination_surface(&workspace).unwrap().state,
        CoordState::Materialized
    );
    let journal = CoordJournal::new(workspace.workspace().common_dir(), slug);
    assert!(journal.pending().unwrap().is_empty());

    // A second bootstrap is a no-op on an already-established surface.
    let again = sk_workspace::bootstrap_coordination(&workspace, &policy(&fixture)).unwrap();
    assert!(!again.branch_created);
    assert_eq!(again.state_before, CoordState::Materialized);
    assert!(again.seed.is_none());
}

#[test]
fn bootstrap_refuses_a_diverged_coordination_branch() {
    let fixture = init_repo();
    let id = mission_id(27);
    let slug = "demo";
    let coord_branch = format!("kitty/mission-{slug}");
    // A branch with its own unrelated commit, and a target that also moved.
    create_branch(fixture.root(), &coord_branch);
    support::git(fixture.root(), &["checkout", &coord_branch]);
    write(&fixture.path("coord.txt"), "coord\n");
    commit_all(fixture.root(), "coord work");
    support::git(fixture.root(), &["checkout", "main"]);
    write(&fixture.path("target.txt"), "target\n");
    commit_all(fixture.root(), "target work");
    write_mission_with(
        fixture.root(),
        slug,
        &id,
        "coord",
        "main",
        serde_json::json!({"coordination_branch": coord_branch}),
    );
    let workspace = project(&fixture, slug);
    let error =
        sk_workspace::bootstrap_coordination(&workspace, &policy(&fixture)).expect_err("refusal");
    assert!(
        matches!(
            error,
            WorkspaceError::CoordRefused {
                code: "COORDINATION_BRANCH_DIVERGED",
                ..
            }
        ),
        "{error}"
    );
}
