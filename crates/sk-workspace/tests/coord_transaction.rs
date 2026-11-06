//! Real coordination transactions against temporary repositories.
//!
//! Every fixture is a genuine Git repository. The expectations were also checked
//! against the pinned Python oracle's coordination surface probe and policy
//! resolution on equivalent temporary repositories (see `COORDINATION-REPORT.md`).

mod support;

#[path = "support/oracle.rs"]
mod oracle_support;

use std::path::Path;

use sk_git::{BranchName, GitRepository, Oid};
use sk_workspace::{
    COORD_SEED_TRAILER, CoordCommitOutcome, CoordJournal, CoordPhase, CoordRecord, CoordRecordKind,
    CoordState, CoordTransaction, MissionWorkspace, ProjectWorkspace, ProtectionPolicy,
    TopologySurface, WorkspaceError, read_coordination_surface, recover_interrupted,
};

use support::{
    branch_tip, commit_all, create_branch, init_repo, mission_id, show_at, trailer, write,
    write_mission_with,
};

/// A real canonical `status.events.jsonl` row.
///
/// The seed parses and validates every row, so the fixture cannot use a prose
/// placeholder here; a restore after an aborted/blocked transaction must return
/// exactly this seeded byte string.
const STATUS_SEED_ROW: &str = "{\"event_id\": \"01KNXQS9ATWWFXS3K5ZJ9E5001\"}\n";

/// A real canonical `decisions.events.jsonl` row (see [`STATUS_SEED_ROW`]).
const DECISIONS_SEED_ROW: &str = "{\"event_id\": \"01KNXQS9ATWWFXS3K5ZJ9E5002\"}\n";

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
    write(&dir.join("status.events.jsonl"), STATUS_SEED_ROW);
    // A real canonical log row: the seed validates every row's `event_id` and
    // refuses a malformed stream instead of committing it.
    write(&dir.join("decisions.events.jsonl"), DECISIONS_SEED_ROW);
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
        "{\"event_id\": \"01KNXQS9ATWWFXS3K5ZJ9E5002\"}"
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
fn a_lost_coordination_directory_is_restored_from_the_branch_tip() {
    // The post-fix EMPTY shape: the branch carries the seeded content, the
    // worktree lost its Mission directory. The reference restores from the tip
    // instead of re-forking the log, and so does this seed.
    let CoordFixture {
        fixture,
        workspace,
        slug,
        mission_id,
        coord_branch,
    } = coord_fixture(14, true);
    {
        let _txn = CoordTransaction::acquire(&workspace, "bootstrap", &policy(&fixture)).unwrap();
    }
    let coord_root = workspace.coord_worktree_path().unwrap();
    let coord_dir = coord_root
        .join("kitty-specs")
        .join(format!("demo-{}", &mission_id[..8]));
    let committed = show_at(
        fixture.root(),
        &branch_tip(fixture.root(), &coord_branch),
        &format!(
            "kitty-specs/{slug}-{}/status.events.jsonl",
            &mission_id[..8]
        ),
    );
    std::fs::remove_dir_all(&coord_dir).unwrap();

    let txn = CoordTransaction::acquire(&workspace, "status_append", &policy(&fixture)).unwrap();
    let seed = txn.seed().expect("the EMPTY surface is restored");
    assert!(
        seed.restored_from_branch
            .iter()
            .any(|path| path == "status.events.jsonl"),
        "{seed:?}"
    );
    assert_eq!(
        std::fs::read_to_string(coord_dir.join("status.events.jsonl")).unwrap(),
        format!("{committed}\n")
    );
    // The restored content is already committed at the tip, so there is nothing
    // to re-commit and no new fork commit.
    assert!(seed.landed.is_none(), "{seed:?}");
    drop(txn);
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
    assert_eq!(bytes, DECISIONS_SEED_ROW);
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
    assert_eq!(bytes, DECISIONS_SEED_ROW);
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
    assert_eq!(
        receipt
            .proof
            .as_ref()
            .expect("an established surface")
            .surface,
        TopologySurface::Coord
    );
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
    assert_eq!(again.state_before, Some(CoordState::Materialized));
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

/// Install a `pre-commit` hook that refuses every commit.
fn install_failing_pre_commit(root: &Path) {
    let hooks = root.join(".git/hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    let hook = hooks.join("pre-commit");
    write(&hook, "#!/bin/sh\nexit 1\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[test]
fn a_malformed_event_log_is_refused_rather_than_committed() {
    let CoordFixture {
        fixture, workspace, ..
    } = coord_fixture(52, false);
    write(
        &fixture.path("kitty-specs/demo/decisions.events.jsonl"),
        "# Decisions\n\n- keep this\n",
    );
    let error = CoordTransaction::acquire(&workspace, "status_append", &policy(&fixture))
        .expect_err("refusal");
    assert!(
        matches!(
            error,
            WorkspaceError::CoordRefused {
                code: "COORD_SEED_EVENT_LOG_MALFORMED",
                ..
            }
        ),
        "{error}"
    );
    // The seed refused before it created anything: no husk.
    let coord_dir = workspace
        .coord_worktree_path()
        .unwrap()
        .join("kitty-specs")
        .join("demo-01KNXQS9");
    assert!(!coord_dir.exists());
}

#[test]
fn a_forked_event_log_is_refused_rather_than_merged() {
    let CoordFixture {
        fixture,
        workspace,
        mission_id,
        coord_branch,
        ..
    } = coord_fixture(53, true);
    {
        let _txn = CoordTransaction::acquire(&workspace, "bootstrap", &policy(&fixture)).unwrap();
    }
    let coord_dir = workspace
        .coord_worktree_path()
        .unwrap()
        .join("kitty-specs")
        .join(format!("demo-{}", &mission_id[..8]));
    std::fs::remove_dir_all(&coord_dir).unwrap();
    // A root stream that is not a prefix extension of the committed one.
    write(
        &fixture.path("kitty-specs/demo/status.events.jsonl"),
        "{\"event_id\": \"01KNXQS9ATWWFXS3K5ZJ9E9999\"}\n",
    );
    let error = CoordTransaction::acquire(&workspace, "status_append", &policy(&fixture))
        .expect_err("refusal");
    assert!(
        matches!(
            error,
            WorkspaceError::CoordRefused {
                code: "COORD_SEED_FORK_REFUSED",
                ..
            }
        ),
        "{error}"
    );
    assert!(!coord_dir.exists(), "a refused seed leaves no husk");
    let _ = coord_branch;
}

#[test]
fn an_untracked_root_copy_is_removed_after_the_seed_commit() {
    let CoordFixture {
        fixture, workspace, ..
    } = coord_fixture(54, false);
    // The root copies are untracked: the seed must restore the single home.
    let txn = CoordTransaction::acquire(&workspace, "status_append", &policy(&fixture)).unwrap();
    let seed = txn.seed().expect("seed receipt");
    assert!(seed.landed.is_some());
    for relative in ["status.events.jsonl", "decisions.events.jsonl"] {
        assert!(
            seed.restored_root
                .iter()
                .any(|path| path.ends_with(relative)),
            "{relative} restored: {seed:?}"
        );
        assert!(
            !workspace.mission_dir().join(relative).exists(),
            "{relative} still has a second home"
        );
    }
    drop(txn);
}

#[test]
fn a_failed_seed_commit_leaves_the_surface_empty_not_a_materialized_husk() {
    let CoordFixture {
        fixture, workspace, ..
    } = coord_fixture(55, false);
    install_failing_pre_commit(fixture.root());
    let error = CoordTransaction::acquire(&workspace, "status_append", &policy(&fixture))
        .expect_err("the seed commit is refused");
    assert!(
        matches!(
            error,
            WorkspaceError::CoordRefused {
                code: "COORD_SEED_COMMIT_REFUSED",
                ..
            }
        ),
        "{error}"
    );
    let coord_dir = workspace
        .coord_worktree_path()
        .unwrap()
        .join("kitty-specs")
        .join("demo-01KNXQS9");
    assert!(!coord_dir.exists(), "no authoritative husk");
    // The read surface keeps answering EMPTY (a seed ticket), not MATERIALIZED.
    assert_eq!(
        read_coordination_surface(&workspace).unwrap().state,
        CoordState::Empty
    );
}

/// A coordination-routed mission whose `meta.json` does not record its branch.
fn undeclared_coord_mission(
    tail: u8,
    create_branch: bool,
) -> (support::Fixture, MissionWorkspace, String) {
    let fixture = init_repo();
    let id = mission_id(tail);
    let slug = "demo";
    let branch = sk_workspace::coord_reconstruct_branch(slug, &id[..8]);
    if create_branch {
        support::create_branch(fixture.root(), &branch);
    }
    let dir = write_mission_with(
        fixture.root(),
        slug,
        &id,
        "coord",
        "main",
        serde_json::json!({}),
    );
    write(
        &dir.join("status.events.jsonl"),
        "{\"event_id\": \"01KNXQS9ATWWFXS3K5ZJ9E5001\"}\n",
    );
    commit_all(fixture.root(), "scaffold");
    let workspace = project(&fixture, slug);
    (fixture, workspace, branch)
}

#[test]
fn an_undeclared_coordination_branch_is_derived_when_it_exists() {
    let (fixture, workspace, branch) = undeclared_coord_mission(56, true);
    let txn = CoordTransaction::acquire(&workspace, "status_append", &policy(&fixture)).unwrap();
    assert_eq!(txn.destination().as_str(), branch);
    assert_eq!(txn.state_before(), CoordState::Unmaterialized);
    assert_eq!(txn.surface(), TopologySurface::Coord);
    drop(txn);
}

#[test]
fn an_undeclared_and_absent_coordination_branch_is_not_reported_as_data_loss() {
    let (fixture, workspace, branch) = undeclared_coord_mission(57, false);
    let error = CoordTransaction::acquire(&workspace, "status_append", &policy(&fixture))
        .expect_err("refusal");
    assert!(
        matches!(
            error,
            WorkspaceError::CoordRefused {
                code: "COORD_BRANCH_UNDECLARED_AND_ABSENT",
                ..
            }
        ),
        "{error}"
    );
    let read_error = read_coordination_surface(&workspace).expect_err("refusal");
    assert!(
        matches!(
            read_error,
            WorkspaceError::CoordRefused {
                code: "COORD_BRANCH_UNDECLARED_AND_ABSENT",
                ..
            }
        ),
        "{read_error}"
    );
    assert!(
        !matches!(read_error, WorkspaceError::CoordinationBranchDeleted { .. }),
        "a branch nobody recorded is not data loss"
    );
    let _ = branch;
}

#[test]
fn a_stray_directory_at_the_coordination_path_is_typed_not_adopted() {
    let CoordFixture {
        fixture, workspace, ..
    } = coord_fixture(58, true);
    let coord_root = workspace.coord_worktree_path().unwrap();
    let squatter = coord_root.join("squatter.txt");
    write(&squatter, "not ours\n");
    let error = CoordTransaction::acquire(&workspace, "status_append", &policy(&fixture))
        .expect_err("refusal");
    assert!(
        matches!(
            error,
            WorkspaceError::CoordRefused {
                code: "COORDINATION_WORKTREE_STRAY_DIRECTORY",
                ..
            }
        ),
        "{error}"
    );
    assert!(squatter.exists(), "a stray directory is never deleted");
}

#[test]
fn recovery_accepts_a_landed_candidate_that_was_built_on() {
    let CoordFixture {
        fixture,
        workspace,
        slug,
        coord_branch,
        ..
    } = coord_fixture(59, true);
    {
        let _txn = CoordTransaction::acquire(&workspace, "bootstrap", &policy(&fixture)).unwrap();
    }
    let coord_root = workspace.coord_worktree_path().unwrap();
    let tip = Oid::parse(&branch_tip(fixture.root(), &coord_branch)).unwrap();
    let journal = CoordJournal::new(workspace.workspace().common_dir(), slug);
    for operation in ["status_append", "status_append"] {
        let mut intent = CoordRecord::intent(
            CoordRecordKind::Commit,
            slug,
            operation,
            Some(&coord_branch),
            Some(&coord_root),
            Some(&tip),
        );
        intent.candidate_commit = Some(tip.to_string());
        journal.begin(intent).unwrap();
    }
    // A newer intent of the same shape must not re-label the older one as
    // abandoned: the mutation's outcome is not proven until recovery says so.
    assert_eq!(journal.pending().unwrap().len(), 2);
    // Another writer builds on the landed candidate: the ordinary crash shape.
    support::git(&coord_root, &["commit", "--allow-empty", "-m", "built on"]);
    let report = recover_interrupted(&workspace).unwrap();
    assert!(report.is_clean(), "{report:?}");
    assert_eq!(report.repairs.len(), 2);
    assert!(
        report
            .repairs
            .iter()
            .all(|repair| repair.phase == CoordPhase::Applied),
        "{report:?}"
    );
    assert!(journal.pending().unwrap().is_empty());
}

#[test]
fn a_landed_commit_with_a_journal_fault_is_still_reported_as_landed() {
    let CoordFixture {
        fixture, workspace, ..
    } = coord_fixture(60, true);
    {
        let _txn = CoordTransaction::acquire(&workspace, "bootstrap", &policy(&fixture)).unwrap();
    }
    let mut txn =
        CoordTransaction::acquire(&workspace, "status_append", &policy(&fixture)).unwrap();
    let journal_path = txn.journal().path().to_path_buf();
    std::fs::remove_file(&journal_path).unwrap();
    std::fs::create_dir(&journal_path).unwrap();
    txn.write_artifact(
        Path::new("kitty-specs/demo-01KNXQS9/status.json"),
        b"{\"materialized\": true}\n",
    )
    .unwrap();
    let receipt = txn
        .commit("chore(spec-kitty): landed with a broken journal")
        .expect("a landed commit is never reported as a failed mutation");
    assert!(
        receipt.outcome.landed().is_some(),
        "the commit is immutable"
    );
    assert!(
        receipt.journal_fault.is_some(),
        "the journal fault is attached as evidence"
    );
}

#[test]
fn bootstrap_refuses_a_protected_destination_before_minting_it() {
    let fixture = init_repo();
    let id = mission_id(61);
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
    write(
        &fixture.root().join(".kittify/config.yaml"),
        &format!("protection:\n  protected_branches: [{coord_branch}]\n"),
    );
    let workspace = project(&fixture, slug);
    let error =
        sk_workspace::bootstrap_coordination(&workspace, &policy(&fixture)).expect_err("refusal");
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
    let branch = BranchName::parse(&coord_branch).unwrap();
    assert!(
        GitRepository::open(fixture.root())
            .unwrap()
            .read_branch_tip(&branch)
            .unwrap()
            .is_none(),
        "a refused create must not leave a minted branch behind"
    );
}

#[test]
fn bootstrap_rolls_back_a_branch_it_minted_when_the_surface_fails() {
    let fixture = init_repo();
    let id = mission_id(62);
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
    write(
        &fixture.path("kitty-specs/demo/status.json"),
        "{\"wps\": {}}\n",
    );
    install_failing_pre_commit(fixture.root());
    let workspace = project(&fixture, slug);
    let error =
        sk_workspace::bootstrap_coordination(&workspace, &policy(&fixture)).expect_err("refusal");
    let text = error.to_string();
    assert!(text.contains("mint rollback"), "{text}");
    let branch = BranchName::parse(&coord_branch).unwrap();
    assert!(
        GitRepository::open(fixture.root())
            .unwrap()
            .read_branch_tip(&branch)
            .unwrap()
            .is_none(),
        "the minted branch was rolled back: {text}"
    );
    let refs = support::git(fixture.root(), &["show-ref"]);
    assert!(
        refs.contains(&format!(
            "refs/spec-kitty/coord-rollback/{slug}-{}",
            &id[..8]
        )),
        "the rolled-back tip is preserved: {refs}"
    );
}

/// A coordination-routed mission whose coordination branch tip already carries
/// `status.json`, with the coordination worktree materialized but its Mission
/// directory removed (the `EMPTY` window upstream calls a "post-fix restore"),
/// plus an **untracked** root `status.json` holding operator-authored bytes.
///
/// This is the reference shape behind review finding R3: the branch is the
/// committed home, the root copy was never committed anywhere, and only the
/// seed's own carry decision may delete it.
fn reference_seeded_status_fixture(
    tail: u8,
    coord_bytes: &str,
    root_bytes: &str,
) -> (CoordFixture, std::path::PathBuf) {
    reference_seeded_fixture(tail, coord_bytes, root_bytes, false)
}

/// [`reference_seeded_status_fixture`], optionally committing the two event logs
/// to the branch tip as well, so the seed finds nothing left to commit.
fn reference_seeded_fixture(
    tail: u8,
    coord_bytes: &str,
    root_bytes: &str,
    carry_logs: bool,
) -> (CoordFixture, std::path::PathBuf) {
    let holder = coord_fixture(tail, true);
    let CoordFixture {
        fixture,
        workspace,
        coord_branch,
        ..
    } = &holder;
    let coord_name = workspace.coord_mission_dir_name();
    let root_logs: Vec<(&str, String)> = if carry_logs {
        ["status.events.jsonl", "decisions.events.jsonl"]
            .iter()
            .map(|name| {
                let bytes = std::fs::read_to_string(workspace.mission_dir().join(name)).unwrap();
                (*name, bytes)
            })
            .collect()
    } else {
        Vec::new()
    };
    // The reference already seeded this file onto the coordination branch.
    support::git(fixture.root(), &["checkout", coord_branch]);
    write(
        &fixture
            .path("kitty-specs")
            .join(&coord_name)
            .join("status.json"),
        coord_bytes,
    );
    for (name, bytes) in &root_logs {
        write(
            &fixture.path("kitty-specs").join(&coord_name).join(name),
            bytes,
        );
    }
    commit_all(fixture.root(), "reference-seeded coordination surface");
    support::git(fixture.root(), &["checkout", "main"]);
    // Materialize the worktree, then remove the Mission directory: `EMPTY`.
    let coord_root = workspace
        .coord_worktree_path()
        .expect("coordination worktree");
    support::git(
        fixture.root(),
        &[
            "worktree",
            "add",
            coord_root.to_str().unwrap(),
            coord_branch,
        ],
    );
    std::fs::remove_dir_all(coord_root.join("kitty-specs")).unwrap();
    assert_eq!(
        workspace.probe_coordination().unwrap().state,
        CoordState::Empty
    );
    let root_copy = workspace.mission_dir().join("status.json");
    write(&root_copy, root_bytes);
    (holder, root_copy)
}

#[test]
fn a_differing_untracked_root_copy_is_preserved_not_deleted() {
    // The operator's exact bytes: never committed, not reproducible from the
    // coordination branch, therefore authored divergence.
    const USER_BYTES: &str =
        "{\"authored_by\": \"operator\", \"wps\": {\"WP01\": \"in_progress\"}, \"version\": 7}\n";
    const COORD_BYTES: &str = "{\"authored_by\": \"reference-seed\", \"wps\": {}}\n";
    let (holder, root_copy) = reference_seeded_status_fixture(63, COORD_BYTES, USER_BYTES);
    let CoordFixture {
        fixture,
        workspace,
        coord_branch,
        ..
    } = &holder;
    let txn = CoordTransaction::acquire(workspace, "status_append", &policy(fixture)).unwrap();
    let seed = txn.seed().expect("the EMPTY surface is restored");
    // The bytes are still on disk, byte-for-byte.
    assert_eq!(
        std::fs::read_to_string(&root_copy).expect("the operator's root copy survives"),
        USER_BYTES
    );
    // …and the receipt says so truthfully instead of claiming a deletion as a
    // preservation (or the reverse).
    assert!(
        seed.preserved_root.iter().any(|path| path == "status.json"),
        "preserved_root names the kept copy: {seed:?}"
    );
    assert!(
        !seed
            .restored_root
            .iter()
            .any(|path| path.ends_with("status.json")),
        "a differing copy is never reported as restored: {seed:?}"
    );
    assert!(
        seed.warnings
            .iter()
            .any(|warning| warning.contains("status.json") && warning.contains("differs")),
        "the divergence is reported: {seed:?}"
    );
    // The coordination copy stays the committed one; the user bytes never
    // rewrite it.
    let committed = show_at(
        fixture.root(),
        coord_branch,
        &format!(
            "kitty-specs/{}/status.json",
            workspace.coord_mission_dir_name()
        ),
    );
    assert_eq!(committed, COORD_BYTES.trim_end());
    drop(txn);
}

#[test]
fn a_differing_root_copy_survives_an_unchanged_seed() {
    // The reference's "unchanged / nothing to commit" arm: everything the seed
    // would write is already at the tip, so no commit is created. The root's
    // differing `status.json` must still survive — this is the exact shape in
    // which the old code unlinked it while the receipt called it preserved.
    const USER_BYTES: &str = "{\"authored_by\": \"operator\", \"note\": \"never committed\"}\n";
    const COORD_BYTES: &str = "{\"authored_by\": \"reference-seed\"}\n";
    let (holder, root_copy) = reference_seeded_fixture(68, COORD_BYTES, USER_BYTES, true);
    let CoordFixture {
        fixture, workspace, ..
    } = &holder;
    let txn = CoordTransaction::acquire(workspace, "status_append", &policy(fixture)).unwrap();
    let seed = txn.seed().expect("the EMPTY surface is restored");
    assert!(
        seed.landed.is_none(),
        "the content is already durable at the tip: {seed:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&root_copy).expect("the authored copy survives"),
        USER_BYTES
    );
    assert!(
        seed.preserved_root.iter().any(|path| path == "status.json"),
        "{seed:?}"
    );
    // The receipt is exhaustive and truthful: only the kept copy is listed, and
    // nothing was reported as restored.
    assert_eq!(seed.preserved_root, vec!["status.json".to_owned()]);
    assert!(
        seed.restored_root.is_empty(),
        "nothing was removed in this shape: {seed:?}"
    );
    // The two logs are tracked-clean copies of the committed stream, which the
    // reference leaves alone too (pre-fix history stays untouched).
    for log in ["status.events.jsonl", "decisions.events.jsonl"] {
        assert!(workspace.mission_dir().join(log).exists(), "{log}");
    }
    drop(txn);
}

#[test]
fn an_untracked_root_copy_equal_to_the_committed_copy_is_a_proven_duplicate() {
    const SHARED_BYTES: &str = "{\"authored_by\": \"same\", \"wps\": {}}\n";
    let (holder, root_copy) = reference_seeded_status_fixture(64, SHARED_BYTES, SHARED_BYTES);
    let CoordFixture {
        fixture, workspace, ..
    } = &holder;
    let txn = CoordTransaction::acquire(workspace, "status_append", &policy(fixture)).unwrap();
    let seed = txn.seed().expect("the EMPTY surface is restored");
    assert!(
        seed.restored_root
            .iter()
            .any(|path| path.ends_with("status.json")),
        "a proven duplicate keeps one home: {seed:?}"
    );
    assert!(
        !root_copy.exists(),
        "the duplicate root copy was removed: {seed:?}"
    );
    assert!(
        seed.preserved_root.is_empty(),
        "nothing was preserved: {seed:?}"
    );
    drop(txn);
}

#[test]
fn a_non_utf8_event_log_is_refused_with_its_path_and_never_rewritten() {
    let CoordFixture {
        fixture, workspace, ..
    } = coord_fixture(65, false);
    // A row that is *valid JSON* once lossily decoded: the old `from_utf8_lossy`
    // path substituted U+FFFD and committed the rewritten bytes. The reference
    // opens the log with `encoding="utf-8"` and raises instead.
    let mut bytes = b"{\"event_id\": \"01KNXQS9ATWWFXS3K5ZJ9E5001\", \"note\": \"".to_vec();
    bytes.push(0xFF);
    bytes.extend_from_slice(b"\"}\n");
    let log = workspace.mission_dir().join("status.events.jsonl");
    std::fs::write(&log, &bytes).unwrap();
    let error = CoordTransaction::acquire(&workspace, "status_append", &policy(&fixture))
        .expect_err("a non-UTF-8 log is refused");
    let text = error.to_string();
    assert!(
        matches!(
            error,
            WorkspaceError::CoordRefused {
                code: "COORD_SEED_EVENT_LOG_NOT_UTF8",
                ..
            }
        ),
        "{text}"
    );
    assert!(text.contains("status.events.jsonl"), "{text}");
    assert_eq!(
        std::fs::read(&log).unwrap(),
        bytes,
        "the authored bytes are never rewritten"
    );
    let coord_dir = workspace
        .coord_worktree_path()
        .unwrap()
        .join("kitty-specs")
        .join(workspace.coord_mission_dir_name());
    assert!(!coord_dir.exists(), "a refused seed leaves no husk");
}

#[test]
fn a_legacy_numeric_slug_keeps_its_prefix_in_the_coordination_home() {
    // R1: the coordination Mission directory is composed VERBATIM
    // (`coord_mission_dir_name`), never with the NNN-stripping primary composer,
    // so a reference-seeded `057-` coordination surface is found instead of
    // seeded a second time under a stripped name.
    let fixture = init_repo();
    let id = mission_id(66);
    let mid8 = &id[..8];
    let slug = "057-lanehead";
    let coord_branch = "kitty/mission-057-lanehead";
    create_branch(fixture.root(), coord_branch);
    let dir = write_mission_with(
        fixture.root(),
        slug,
        &id,
        "coord",
        "main",
        serde_json::json!({"coordination_branch": coord_branch}),
    );
    write(&dir.join("status.events.jsonl"), STATUS_SEED_ROW);
    commit_all(fixture.root(), "scaffold");
    let workspace = project(&fixture, slug);
    let verbatim = format!("{slug}-{mid8}");
    let stripped = format!("lanehead-{mid8}");
    assert_eq!(workspace.coord_mission_dir_name(), verbatim);
    assert_eq!(workspace.canonical_mission_dir_name(), stripped);
    // Interop: the reference's own verbatim composer names the same directory,
    // and its NNN-stripping composer names the one this must NOT use.
    assert_eq!(
        oracle_support::run(
            "import sys\nfrom specify_cli.lanes.branch_naming import coord_mission_dir_name\nprint(coord_mission_dir_name(sys.argv[1], mid8=sys.argv[2]))",
            &[slug, mid8],
        ),
        verbatim
    );
    assert_eq!(
        oracle_support::run(
            "import sys\nfrom specify_cli.lanes.branch_naming import mission_dir_name\nprint(mission_dir_name(sys.argv[1], mid8=sys.argv[2]))",
            &[slug, mid8],
        ),
        stripped
    );

    // A reference-created coordination worktree carrying the VERBATIM directory.
    let coord_root = workspace.coord_worktree_path().unwrap();
    support::git(
        fixture.root(),
        &[
            "worktree",
            "add",
            coord_root.to_str().unwrap(),
            coord_branch,
        ],
    );
    let reference_dir = coord_root.join("kitty-specs").join(&verbatim);
    write(&reference_dir.join("status.events.jsonl"), STATUS_SEED_ROW);
    support::git(&coord_root, &["add", "-A"]);
    support::git(&coord_root, &["commit", "-m", "reference seed"]);

    let probe = workspace.probe_coordination().unwrap();
    assert_eq!(probe.state, CoordState::Materialized, "{probe:?}");
    assert_eq!(
        probe.mission_dir.as_deref(),
        Some(reference_dir.as_path()),
        "the probe answers the verbatim coordination home"
    );
    assert_eq!(
        sk_workspace::coordination_mission_dir(&workspace).as_deref(),
        Some(reference_dir.as_path())
    );
    let read = read_coordination_surface(&workspace).unwrap();
    assert_eq!(read.read_dir, reference_dir);
    assert!(!read.degraded_to_primary);

    // A write lands in that directory and nowhere else.
    let mut txn =
        CoordTransaction::acquire(&workspace, "status_append", &policy(&fixture)).unwrap();
    assert!(
        txn.seed().is_none(),
        "a materialized reference surface is reused, not re-seeded"
    );
    assert_eq!(txn.feature_dir(), reference_dir.as_path());
    txn.write_artifact(
        Path::new(&format!("kitty-specs/{verbatim}/status.json")),
        b"{\"materialized\": true}\n",
    )
    .unwrap();
    txn.commit("chore(spec-kitty): append status").unwrap();
    assert!(
        !coord_root.join("kitty-specs").join(&stripped).exists(),
        "no NNN-stripped second home is ever created"
    );
    assert_eq!(
        show_at(
            fixture.root(),
            coord_branch,
            &format!("kitty-specs/{verbatim}/status.json")
        ),
        "{\"materialized\": true}"
    );
}

#[test]
fn recovery_holds_the_claim_and_mission_locks_while_proving_its_verdicts() {
    // R5: recovery reads tips and resolves journal records. Without the crate's
    // claim + Mission pair, a writer landing between the proof and the resolve
    // makes the verdict stale. The pass therefore serializes on the same claim
    // the mission's transactions take — the coordination worktree, not the
    // primary checkout.
    let CoordFixture {
        fixture,
        workspace,
        slug,
        coord_branch,
        ..
    } = coord_fixture(67, true);
    {
        let _txn = CoordTransaction::acquire(&workspace, "bootstrap", &policy(&fixture)).unwrap();
    }
    let coord_root = workspace.coord_worktree_path().unwrap();
    let tip = Oid::parse(&branch_tip(fixture.root(), &coord_branch)).unwrap();
    let journal = CoordJournal::new(workspace.workspace().common_dir(), slug);
    let mut intent = CoordRecord::intent(
        CoordRecordKind::Commit,
        slug,
        "status_append",
        Some(&coord_branch),
        Some(&coord_root),
        Some(&tip),
    );
    intent.candidate_commit = Some(tip.to_string());
    journal.begin(intent).unwrap();

    let claim = sk_kernel::lock_families::checkout_claim_lock_path(
        &coord_root,
        workspace.workspace().common_dir(),
    )
    .unwrap();
    let ready = fixture.root().join("recovery-ready");
    let released = fixture.root().join("recovery-released");
    let script = r#"
import sys, time
from pathlib import Path
from kernel.locks import machine_file_lock

lock_path, ready, released = Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3])
with machine_file_lock(lock_path, blocking=True, timeout_s=10):
    ready.write_text("ready")
    time.sleep(1.5)
    # Written INSIDE the hold: recovery can only have made progress after the
    # release if this file already exists when it returns.
    released.write_text("released")
"#;
    let child = std::process::Command::new(test_oracle_python())
        .arg("-c")
        .arg(script)
        .arg(&claim)
        .arg(&ready)
        .arg(&released)
        .env("PYTHONPATH", {
            let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            root.join("src")
        })
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn the claim holder");
    for _ in 0..600 {
        if ready.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(ready.exists(), "the contender never took the claim");
    let report = recover_interrupted(&workspace).expect("recovery proves the record");
    // Observed BEFORE waiting on the child: an unlocked recovery would return
    // mid-hold, when this marker cannot exist yet.
    let progressed_after_release = released.exists();
    assert!(report.is_clean(), "{report:?}");
    let output = child.wait_with_output().expect("wait for the holder");
    assert!(output.status.success());
    assert!(
        progressed_after_release,
        "recovery only made progress after the claim was released"
    );
    assert!(
        journal.pending().unwrap().is_empty(),
        "the verdict was resolved under the hold"
    );
}

/// The pinned development oracle (never a production path).
fn test_oracle_python() -> std::path::PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let executable = if cfg!(windows) {
        "Scripts/python.exe"
    } else {
        "bin/python"
    };
    let python = std::env::var_os("SK_KERNEL_PYTHON")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| root.join(".venv-rust-oracle").join(executable));
    assert!(
        python.is_file(),
        "the required Python oracle is missing at {}",
        python.display()
    );
    python
}
