//! Adversarial acceptance tests for the Git lifecycle primitives.
//!
//! Every test runs against a real temporary Git repository. No test mutates the
//! workspace checkout or any user remote. The tests are written around the
//! failure modes the primitives exist to prevent: ref option injection, a ref
//! moving after a plan, dirty checkouts, unsafe deletion, protected targets,
//! detached/unborn HEAD, non-UTF-8 paths, and remote absence versus failure.

mod common;

use std::fs;
use std::path::Path;

use common::{TestRepo, git_ok, git_raw, init_empty_repo_at};
use sk_git::{
    BranchAdvanceOptions, BranchDeletionOptions, BranchName, GitError, GitRepository, Oid,
    ProtectedBranches, RefChange, RefName, StartPoint, WorktreeDiscardPolicy, WorktreeStart,
};

fn oid_from(raw: &[u8]) -> Oid {
    Oid::parse(std::str::from_utf8(raw).expect("utf8 git output").trim()).expect("valid oid")
}

fn head(repo: &TestRepo) -> Oid {
    oid_from(&repo.git(&["rev-parse", "HEAD"]))
}

fn branch(name: &str) -> BranchName {
    BranchName::parse(name).expect("valid branch name")
}

fn refname(name: &str) -> RefName {
    RefName::parse(name).expect("valid ref name")
}

fn protected(names: &[&str]) -> ProtectedBranches {
    ProtectedBranches::parse(names.iter().copied()).expect("valid protected branches")
}

fn commit_file(repo: &TestRepo, name: &str, contents: &str, message: &str) -> Oid {
    fs::write(repo.path.join(name), contents.as_bytes()).expect("write file");
    repo.git(&["add", "--", name]);
    repo.git(&["commit", "-q", "-m", message]);
    head(repo)
}

fn run_git(repo: &TestRepo, args: &[&str]) -> Vec<u8> {
    repo.git(args)
}

fn worktree_add(repo: &TestRepo, path: &Path, branch_name: &str) {
    run_git(
        repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            branch_name,
            path.to_str().expect("utf8 path"),
        ],
    );
}

// ---------------------------------------------------------------------------
// Ref option injection and typed values
// ---------------------------------------------------------------------------

#[test]
fn option_and_revspec_shaped_names_cannot_reach_a_plan() {
    for name in ["-f", "--force", "a..b", "a@{1}", "main^{commit}", "main:a"] {
        assert!(
            matches!(
                BranchName::parse(name),
                Err(GitError::InvalidRefName { .. })
            ),
            "branch {name:?} must be refused"
        );
        assert!(
            matches!(RefName::parse(name), Err(GitError::InvalidRefName { .. })),
            "ref {name:?} must be refused"
        );
    }
    for value in [
        "HEAD~1",
        "abc123",
        "0123",
        "main",
        "0123456789abcdef0123456789abcdef0123456^{commit}",
    ] {
        assert!(
            matches!(Oid::parse(value), Err(GitError::InvalidOid { .. })),
            "oid {value:?} must be refused"
        );
    }
}

#[test]
fn a_branch_named_like_an_option_is_refused_before_git_runs() {
    let repo = TestRepo::new();
    let git = GitRepository::open(&repo.path).unwrap();

    let error = BranchName::parse("-f").unwrap_err();
    assert!(matches!(error, GitError::InvalidRefName { .. }));
    let _ = git
        .plan_branch_creation(&branch("x"), StartPoint::Head, "test: create")
        .unwrap();
    assert!(
        repo.git_raw(&["show-ref", "--verify", "refs/heads/-f"])
            .status
            .code()
            .is_some_and(|code| code != 0)
    );
}

// ---------------------------------------------------------------------------
// Ref transactions
// ---------------------------------------------------------------------------

#[test]
fn ref_transaction_create_update_delete_uses_expected_values() {
    let repo = TestRepo::new();
    let git = GitRepository::open(&repo.path).unwrap();
    let first = head(&repo);
    let reference = refname("refs/heads/txn");

    let created = git
        .plan_ref_transaction(
            "test: create",
            vec![RefChange::create(reference.clone(), first.clone())],
        )
        .and_then(|plan| git.apply_ref_transaction(&plan))
        .unwrap();
    assert_eq!(created.previous()[0].oid(), None);
    assert_eq!(created.applied()[0].oid(), Some(&first));
    assert_eq!(git.read_ref(&reference).unwrap(), Some(first.clone()));

    let second = commit_file(&repo, "b.txt", "two\n", "second");
    let moved = git
        .plan_ref_transaction(
            "test: update",
            vec![RefChange::set(
                reference.clone(),
                second.clone(),
                Some(first.clone()),
            )],
        )
        .and_then(|plan| git.apply_ref_transaction(&plan))
        .unwrap();
    assert_eq!(moved.previous()[0].oid(), Some(&first));
    assert_eq!(moved.applied()[0].oid(), Some(&second));

    let deleted = git
        .plan_ref_transaction(
            "test: delete",
            vec![RefChange::delete(reference.clone(), second.clone())],
        )
        .and_then(|plan| git.apply_ref_transaction(&plan))
        .unwrap();
    assert_eq!(deleted.applied()[0].oid(), None);
    assert_eq!(git.read_ref(&reference).unwrap(), None);
}

#[test]
fn planning_over_an_existing_ref_is_stale_and_writes_nothing() {
    let repo = TestRepo::new();
    let git = GitRepository::open(&repo.path).unwrap();
    let current = head(&repo);
    let reference = refname("refs/heads/existing");
    git.plan_ref_transaction(
        "test: create",
        vec![RefChange::create(reference.clone(), current.clone())],
    )
    .and_then(|plan| git.apply_ref_transaction(&plan))
    .unwrap();

    let error = git
        .plan_ref_transaction(
            "test: recreate",
            vec![RefChange::create(reference.clone(), current.clone())],
        )
        .unwrap_err();

    assert!(matches!(error, GitError::PlanStale { .. }), "{error:?}");
    assert_eq!(git.read_ref(&reference).unwrap(), Some(current));
}

#[test]
fn a_ref_write_cannot_point_a_branch_at_an_annotated_tag_object() {
    let repo = TestRepo::new();
    let git = GitRepository::open(&repo.path).unwrap();
    run_git(&repo, &["tag", "-a", "v1", "-m", "annotated"]);
    let tag_object = oid_from(&git_ok(&repo.path, &["rev-parse", "refs/tags/v1"]));

    let error = git
        .plan_ref_transaction(
            "test: tag object",
            vec![RefChange::create(
                refname("refs/heads/from-tag"),
                tag_object,
            )],
        )
        .unwrap_err();
    assert!(matches!(error, GitError::RefMissing { .. }), "{error:?}");
}

#[test]
fn a_ref_moving_after_a_plan_refuses_with_evidence() {
    let repo = TestRepo::new();
    let git = GitRepository::open(&repo.path).unwrap();
    let original = head(&repo);
    let reference = refname("refs/heads/planned");
    git.plan_ref_transaction(
        "test: create",
        vec![RefChange::create(reference.clone(), original.clone())],
    )
    .and_then(|plan| git.apply_ref_transaction(&plan))
    .unwrap();

    let new_tip = commit_file(&repo, "b.txt", "two\n", "second");
    let plan = git
        .plan_ref_transaction(
            "test: move",
            vec![RefChange::set(
                reference.clone(),
                new_tip.clone(),
                Some(original.clone()),
            )],
        )
        .unwrap();

    // A concurrent writer moves the ref after the plan was prepared.
    run_git(&repo, &["update-ref", reference.as_str(), new_tip.as_str()]);

    let error = git.apply_ref_transaction(&plan).unwrap_err();
    match error {
        GitError::PlanStale { what, detail } => {
            assert_eq!(what, reference.as_str());
            assert!(detail.contains("planned"), "{detail}");
        }
        other => panic!("expected PlanStale, got {other:?}"),
    }
    assert_eq!(git.read_ref(&reference).unwrap(), Some(new_tip));
}

// ---------------------------------------------------------------------------
// Branch creation and advance
// ---------------------------------------------------------------------------

#[test]
fn branch_creation_pins_the_start_commit_and_refuses_duplicates() {
    let repo = TestRepo::new();
    let git = GitRepository::open(&repo.path).unwrap();
    let start = head(&repo);

    let plan = git
        .plan_branch_creation(&branch("feature"), StartPoint::Head, "test: create feature")
        .unwrap();
    assert_eq!(plan.start_oid(), &start);
    git.apply_branch_creation(&plan).unwrap();
    assert_eq!(
        git.read_branch_tip(&branch("feature")).unwrap(),
        Some(start)
    );

    let error = git
        .plan_branch_creation(&branch("feature"), StartPoint::Head, "test: create feature")
        .unwrap_err();
    assert!(matches!(error, GitError::BranchAlreadyExists { .. }));
}

#[test]
fn advancing_a_branch_refuses_non_fast_forward_unless_explicitly_allowed() {
    let repo = TestRepo::new();
    let git = GitRepository::open(&repo.path).unwrap();
    let first = head(&repo);
    let second = commit_file(&repo, "b.txt", "two\n", "second");
    git.plan_branch_creation(
        &branch("feature"),
        StartPoint::Oid(first.clone()),
        "test: create",
    )
    .and_then(|plan| git.apply_branch_creation(&plan))
    .unwrap();

    let options = BranchAdvanceOptions::default();
    let forward = git
        .plan_branch_advance(
            &branch("feature"),
            second.clone(),
            Some(first.clone()),
            &protected(&[]),
            "test: advance",
            &options,
        )
        .unwrap();
    git.apply_branch_advance(&forward).unwrap();
    assert_eq!(
        git.read_branch_tip(&branch("feature")).unwrap(),
        Some(second.clone())
    );

    let error = git
        .plan_branch_advance(
            &branch("feature"),
            first.clone(),
            Some(second.clone()),
            &protected(&[]),
            "test: rewind",
            &options,
        )
        .unwrap_err();
    assert!(
        matches!(error, GitError::NonFastForward { .. }),
        "{error:?}"
    );
    assert_eq!(
        git.read_branch_tip(&branch("feature")).unwrap(),
        Some(second.clone())
    );

    let rewind = git
        .plan_branch_advance(
            &branch("feature"),
            first.clone(),
            Some(second.clone()),
            &protected(&[]),
            "test: rollback",
            &options.clone().allow_non_fast_forward(),
        )
        .unwrap();
    git.apply_branch_advance(&rewind).unwrap();
    assert_eq!(
        git.read_branch_tip(&branch("feature")).unwrap(),
        Some(first)
    );
}

#[test]
fn protected_targets_require_an_explicit_override_reason() {
    let repo = TestRepo::new();
    let git = GitRepository::open(&repo.path).unwrap();
    let _first = head(&repo);
    let second = commit_file(&repo, "b.txt", "two\n", "second");
    let third = commit_file(&repo, "c.txt", "three\n", "third");
    // Build the incoming commit on another branch so `main` stays at `third`.
    run_git(&repo, &["checkout", "-q", "-b", "incoming"]);
    let fourth = commit_file(&repo, "d.txt", "four\n", "fourth");
    run_git(&repo, &["checkout", "-q", "main"]);
    let protection = protected(&["main"]);

    let error = git
        .plan_branch_advance(
            &branch("main"),
            fourth.clone(),
            Some(third.clone()),
            &protection,
            "test: advance",
            &BranchAdvanceOptions::default().reset_clean_checkouts(),
        )
        .unwrap_err();
    assert!(
        matches!(error, GitError::ProtectedTarget { .. }),
        "{error:?}"
    );

    let plan = git
        .plan_branch_advance(
            &branch("main"),
            fourth,
            Some(third),
            &protection,
            "test: explicit release move",
            &BranchAdvanceOptions::default()
                .reset_clean_checkouts()
                .allow_protected_target("operator authorized release advance"),
        )
        .unwrap();
    let _ = second;
    assert_eq!(
        plan.protected_override(),
        Some("operator authorized release advance")
    );
}

#[test]
fn a_moved_ref_after_plan_is_refused_not_clobbered() {
    let repo = TestRepo::new();
    let git = GitRepository::open(&repo.path).unwrap();
    let first = head(&repo);
    let second = commit_file(&repo, "b.txt", "two\n", "second");
    git.plan_branch_creation(
        &branch("feature"),
        StartPoint::Oid(first.clone()),
        "test: create",
    )
    .and_then(|plan| git.apply_branch_creation(&plan))
    .unwrap();

    let plan = git
        .plan_branch_advance(
            &branch("feature"),
            second.clone(),
            Some(first.clone()),
            &protected(&[]),
            "test: advance",
            &BranchAdvanceOptions::default(),
        )
        .unwrap();

    // Another writer moves the branch after the plan.
    run_git(
        &repo,
        &["update-ref", "refs/heads/feature", second.as_str()],
    );

    let error = git.apply_branch_advance(&plan).unwrap_err();
    assert!(matches!(error, GitError::PlanStale { .. }), "{error:?}");
    assert_eq!(
        git.read_branch_tip(&branch("feature")).unwrap(),
        Some(second)
    );
}

#[test]
fn advancing_a_checked_out_branch_resyncs_only_clean_checkouts() {
    let repo = TestRepo::new();
    let git = GitRepository::open(&repo.path).unwrap();
    let first = head(&repo);
    let linked = repo.temp.path().join("feature worktree");
    worktree_add(&repo, &linked, "feature");

    let second = commit_file(&repo, "b.txt", "two\n", "second");
    let plan = git
        .plan_branch_advance(
            &branch("feature"),
            second.clone(),
            Some(first.clone()),
            &protected(&[]),
            "test: advance",
            &BranchAdvanceOptions::default().reset_clean_checkouts(),
        )
        .unwrap();
    assert_eq!(plan.checkouts().to_vec(), vec![linked.clone()]);
    let outcome = git.apply_branch_advance(&plan).unwrap();
    assert_eq!(outcome.resynced().to_vec(), vec![linked.clone()]);
    assert_eq!(
        git.read_branch_tip(&branch("feature")).unwrap(),
        Some(second.clone())
    );
    assert_eq!(
        oid_from(&git_ok(&linked, &["rev-parse", "HEAD"])),
        second,
        "the checkout must not be left behind its own HEAD"
    );
    assert!(git_ok(&linked, &["status", "--porcelain"]).is_empty());
}

#[test]
fn a_dirty_checkout_refuses_the_advance_before_the_ref_moves() {
    let repo = TestRepo::new();
    let git = GitRepository::open(&repo.path).unwrap();
    let first = head(&repo);
    let linked = repo.temp.path().join("dirty worktree");
    worktree_add(&repo, &linked, "feature");
    fs::write(linked.join("untracked.txt"), b"local work\n").unwrap();

    let second = commit_file(&repo, "b.txt", "two\n", "second");
    let error = git
        .plan_branch_advance(
            &branch("feature"),
            second,
            Some(first.clone()),
            &protected(&[]),
            "test: advance",
            &BranchAdvanceOptions::default().reset_clean_checkouts(),
        )
        .unwrap_err();

    match error {
        GitError::DirtyWorktree { path, entries } => {
            assert_eq!(path, linked);
            assert!(entries.iter().any(|entry| entry.contains("untracked.txt")));
        }
        other => panic!("expected DirtyWorktree, got {other:?}"),
    }
    assert_eq!(
        git.read_branch_tip(&branch("feature")).unwrap(),
        Some(first)
    );
}

#[test]
fn a_checkout_that_becomes_dirty_after_planning_refuses_at_apply() {
    let repo = TestRepo::new();
    let git = GitRepository::open(&repo.path).unwrap();
    let first = head(&repo);
    let linked = repo.temp.path().join("late dirty worktree");
    worktree_add(&repo, &linked, "feature");
    let second = commit_file(&repo, "b.txt", "two\n", "second");

    let plan = git
        .plan_branch_advance(
            &branch("feature"),
            second,
            Some(first.clone()),
            &protected(&[]),
            "test: advance",
            &BranchAdvanceOptions::default().reset_clean_checkouts(),
        )
        .unwrap();
    fs::write(linked.join("late.txt"), b"appeared after planning\n").unwrap();

    let error = git.apply_branch_advance(&plan).unwrap_err();
    assert!(matches!(error, GitError::DirtyWorktree { .. }), "{error:?}");
    assert_eq!(
        git.read_branch_tip(&branch("feature")).unwrap(),
        Some(first)
    );
}

#[test]
fn an_unborn_branch_advances_with_the_zero_old_cas() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("unborn");
    init_empty_repo_at(&path);
    fs::write(path.join("a.txt"), b"one\n").unwrap();
    git_ok(&path, &["add", "a.txt"]);
    let tree = std::str::from_utf8(&git_ok(&path, &["write-tree"]))
        .unwrap()
        .trim()
        .to_string();
    let commit = oid_from(&git_ok(
        &path,
        &["commit-tree", tree.as_str(), "-m", "first"],
    ));
    // Leave the checkout clean so the resync is allowed.
    git_ok(&path, &["reset", "-q"]);
    fs::remove_file(path.join("a.txt")).unwrap();
    let git = GitRepository::open(&path).unwrap();
    assert_eq!(git.read_branch_tip(&branch("main")).unwrap(), None);

    let plan = git
        .plan_branch_advance(
            &branch("main"),
            commit.clone(),
            None,
            &protected(&[]),
            "test: first commit",
            &BranchAdvanceOptions::default().reset_clean_checkouts(),
        )
        .unwrap();
    let outcome = git.apply_branch_advance(&plan).unwrap();
    assert_eq!(outcome.previous(), None);
    assert_eq!(outcome.new_tip(), &commit);
    assert_eq!(
        git.read_branch_tip(&branch("main")).unwrap(),
        Some(commit.clone())
    );
    assert_eq!(oid_from(&git_ok(&path, &["rev-parse", "HEAD"])), commit);
}

// ---------------------------------------------------------------------------
// Branch deletion
// ---------------------------------------------------------------------------

#[test]
fn guarded_deletion_refuses_unique_commits_and_protected_targets() {
    let repo = TestRepo::new();
    let git = GitRepository::open(&repo.path).unwrap();
    let base = head(&repo);
    git.plan_branch_creation(
        &branch("feature"),
        StartPoint::Oid(base.clone()),
        "test: create",
    )
    .and_then(|plan| git.apply_branch_creation(&plan))
    .unwrap();
    git_ok(&repo.path, &["checkout", "-q", "feature"]);
    let unique = commit_file(&repo, "unique.txt", "only here\n", "unique work");
    git_ok(&repo.path, &["checkout", "-q", "main"]);

    let error = git
        .plan_branch_deletion(
            &branch("feature"),
            &protected(&[]),
            None,
            BranchDeletionOptions::default(),
            "test: delete",
        )
        .unwrap_err();
    match error {
        GitError::BranchHasUniqueCommits {
            branch: name,
            commits,
        } => {
            assert_eq!(name, "feature");
            assert_eq!(commits, vec![unique.to_string()]);
        }
        other => panic!("expected BranchHasUniqueCommits, got {other:?}"),
    }

    let preserved_ref = refname("refs/spec-kitty/discarded/feature");
    let plan = git
        .plan_branch_deletion(
            &branch("feature"),
            &protected(&[]),
            None,
            BranchDeletionOptions::default()
                .allow_unique_commits("operator discarded the feature")
                .preserve_tip_at(preserved_ref.clone()),
            "test: discard feature",
        )
        .unwrap();
    let outcome = git.apply_branch_deletion(&plan).unwrap();
    assert_eq!(outcome.tip(), &unique);
    assert_eq!(git.read_branch_tip(&branch("feature")).unwrap(), None);
    assert_eq!(git.read_ref(&preserved_ref).unwrap(), Some(unique.clone()));

    run_git(&repo, &["branch", "release", base.as_str()]);
    let error = git
        .plan_branch_deletion(
            &branch("release"),
            &protected(&["release"]),
            Some(&base),
            BranchDeletionOptions::default(),
            "test: delete release",
        )
        .unwrap_err();
    assert!(
        matches!(error, GitError::ProtectedTarget { .. }),
        "{error:?}"
    );
    // The checked-out guard is evaluated before the unique-commit guard.
    assert!(matches!(
        git.plan_branch_deletion(
            &branch("main"),
            &protected(&[]),
            None,
            BranchDeletionOptions::default(),
            "test: delete main",
        )
        .unwrap_err(),
        GitError::BranchCheckedOut { .. }
    ));
}

#[test]
fn commits_becoming_unique_after_deletion_planning_refuses_the_delete() {
    let repo = TestRepo::new();
    let git = GitRepository::open(&repo.path).unwrap();
    let base = head(&repo);
    run_git(&repo, &["branch", "feature", base.as_str()]);
    run_git(&repo, &["checkout", "-q", "feature"]);
    let feature_tip = commit_file(&repo, "feature.txt", "feature\n", "feature work");
    run_git(&repo, &["checkout", "-q", "main"]);
    run_git(&repo, &["branch", "backup", feature_tip.as_str()]);

    // The feature commit is currently reachable from `backup`, so deletion is
    // guarded-safe.
    let plan = git
        .plan_branch_deletion(
            &branch("feature"),
            &protected(&[]),
            Some(&base),
            BranchDeletionOptions::default(),
            "test: delete feature",
        )
        .unwrap();
    assert!(plan.preflight().unique_commits().is_empty());

    // Deleting the other ref makes those commits unique without moving
    // `feature`; the approved verdict no longer holds.
    run_git(&repo, &["update-ref", "-d", "refs/heads/backup"]);
    let error = git.apply_branch_deletion(&plan).unwrap_err();
    assert!(matches!(error, GitError::PlanStale { .. }), "{error:?}");
    assert_eq!(
        git.read_branch_tip(&branch("feature")).unwrap(),
        Some(feature_tip)
    );
}

#[test]
fn deletion_refuses_a_checked_out_branch_even_with_overrides() {
    let repo = TestRepo::new();
    let git = GitRepository::open(&repo.path).unwrap();
    let base = head(&repo);
    let linked = repo.temp.path().join("delete guard");
    worktree_add(&repo, &linked, "feature");

    let error = git
        .plan_branch_deletion(
            &branch("feature"),
            &protected(&[]),
            Some(&base),
            BranchDeletionOptions::default()
                .allow_unique_commits("explicit")
                .allow_protected_target("explicit"),
            "test: delete",
        )
        .unwrap_err();
    match error {
        GitError::BranchCheckedOut {
            branch: name,
            worktrees,
        } => {
            assert_eq!(name, "feature");
            assert_eq!(worktrees, vec![linked]);
        }
        other => panic!("expected BranchCheckedOut, got {other:?}"),
    }
}

#[test]
fn a_preserved_tip_must_live_in_the_bookkeeping_namespace() {
    let repo = TestRepo::new();
    let git = GitRepository::open(&repo.path).unwrap();
    let base = head(&repo);
    git.plan_branch_creation(&branch("feature"), StartPoint::Oid(base), "test: create")
        .and_then(|plan| git.apply_branch_creation(&plan))
        .unwrap();

    let error = git
        .plan_branch_deletion(
            &branch("feature"),
            &protected(&[]),
            None,
            BranchDeletionOptions::default().preserve_tip_at(refname("refs/heads/backup")),
            "test: delete",
        )
        .unwrap_err();
    assert!(matches!(error, GitError::UnsafePath { .. }), "{error:?}");
}

#[test]
fn a_ref_that_moves_after_deletion_planning_is_not_deleted() {
    let repo = TestRepo::new();
    let git = GitRepository::open(&repo.path).unwrap();
    let base = head(&repo);
    git.plan_branch_creation(
        &branch("feature"),
        StartPoint::Oid(base.clone()),
        "test: create",
    )
    .and_then(|plan| git.apply_branch_creation(&plan))
    .unwrap();
    let plan = git
        .plan_branch_deletion(
            &branch("feature"),
            &protected(&[]),
            Some(&base),
            BranchDeletionOptions::default(),
            "test: delete",
        )
        .unwrap();
    let moved = commit_file(&repo, "moved.txt", "moved\n", "moved");
    run_git(&repo, &["update-ref", "refs/heads/feature", moved.as_str()]);

    let error = git.apply_branch_deletion(&plan).unwrap_err();
    assert!(matches!(error, GitError::PlanStale { .. }), "{error:?}");
    assert_eq!(
        git.read_branch_tip(&branch("feature")).unwrap(),
        Some(moved)
    );
}

// ---------------------------------------------------------------------------
// Worktrees
// ---------------------------------------------------------------------------

#[test]
fn worktree_add_reports_real_mutations_and_refuses_unsafe_paths() {
    let repo = TestRepo::new();
    let git = GitRepository::open(&repo.path).unwrap();
    let base = head(&repo);
    let path = repo.temp.path().join("allocated worktree");

    let plan = git
        .plan_worktree_add(
            path.clone(),
            &branch("feature"),
            WorktreeStart::NewBranch(StartPoint::Head),
        )
        .unwrap();
    let outcome = git.apply_worktree_add(&plan).unwrap();
    assert_eq!(outcome.path(), path.as_path());
    assert_eq!(outcome.head(), &base);
    assert!(outcome.created_branch());
    assert_eq!(
        git.read_branch_tip(&branch("feature")).unwrap(),
        Some(base.clone())
    );
    assert_eq!(
        git.checkouts_of(&branch("feature")).unwrap(),
        vec![path.clone()]
    );

    let duplicate = git
        .plan_worktree_add(
            path.clone(),
            &branch("another"),
            WorktreeStart::NewBranch(StartPoint::Head),
        )
        .unwrap_err();
    assert!(matches!(
        duplicate,
        GitError::WorktreePathUnavailable { .. }
    ));

    let inside_git = repo.path.join(".git").join("worktrees").join("bad");
    let error = git
        .plan_worktree_add(
            inside_git,
            &branch("bad"),
            WorktreeStart::NewBranch(StartPoint::Head),
        )
        .unwrap_err();
    assert!(matches!(error, GitError::UnsafePath { .. }), "{error:?}");

    let non_empty = repo.temp.path().join("non-empty");
    fs::create_dir_all(&non_empty).unwrap();
    fs::write(non_empty.join("kept.txt"), b"keep\n").unwrap();
    let error = git
        .plan_worktree_add(
            non_empty,
            &branch("bad2"),
            WorktreeStart::NewBranch(StartPoint::Head),
        )
        .unwrap_err();
    assert!(matches!(error, GitError::WorktreePathUnavailable { .. }));

    let error = git
        .plan_worktree_add(
            repo.temp.path().join("attach-main"),
            &branch("main"),
            WorktreeStart::ExistingBranch,
        )
        .unwrap_err();
    assert!(
        matches!(error, GitError::BranchCheckedOut { .. }),
        "{error:?}"
    );
}

#[cfg(unix)]
#[test]
fn symlinked_ancestors_cannot_hide_git_metadata_allocation_targets() {
    let repo = TestRepo::new();
    let git = GitRepository::open(&repo.path).unwrap();
    let alias = repo.temp.path().join("alias");
    std::os::unix::fs::symlink(&repo.path, &alias).unwrap();
    let target = alias.join(".git").join("hidden-worktree");
    let error = git
        .plan_worktree_add(
            target,
            &branch("hidden"),
            WorktreeStart::NewBranch(StartPoint::Head),
        )
        .unwrap_err();
    assert!(matches!(error, GitError::UnsafePath { .. }), "{error:?}");
    assert!(!repo.path.join(".git/hidden-worktree").exists());
    assert!(git.read_branch_tip(&branch("hidden")).unwrap().is_none());
}

#[test]
fn worktree_removal_is_guarded_by_policy_and_fingerprint() {
    let repo = TestRepo::new();
    let git = GitRepository::open(&repo.path).unwrap();
    let clean = repo.temp.path().join("clean worktree");
    worktree_add(&repo, &clean, "clean");
    let outcome = git
        .plan_worktree_removal(&clean, None, None, WorktreeDiscardPolicy::RefuseDirty)
        .and_then(|plan| git.apply_worktree_removal(&plan))
        .unwrap();
    assert_eq!(outcome.path(), clean.as_path());
    assert!(!clean.exists());
    assert!(git.read_branch_tip(&branch("clean")).unwrap().is_some());

    let untracked = repo.temp.path().join("untracked worktree");
    worktree_add(&repo, &untracked, "with-untracked");
    fs::write(untracked.join("scratch.txt"), b"scratch\n").unwrap();
    let error = git
        .plan_worktree_removal(&untracked, None, None, WorktreeDiscardPolicy::RefuseDirty)
        .unwrap_err();
    assert!(matches!(error, GitError::DirtyWorktree { .. }), "{error:?}");
    let plan = git
        .plan_worktree_removal(
            &untracked,
            None,
            None,
            WorktreeDiscardPolicy::AllowUntracked,
        )
        .unwrap();
    assert!(
        plan.discarded()
            .iter()
            .any(|entry| entry.contains("scratch.txt"))
    );
    git.apply_worktree_removal(&plan).unwrap();
    assert!(!untracked.exists());

    let tracked = repo.temp.path().join("tracked worktree");
    worktree_add(&repo, &tracked, "with-tracked");
    fs::write(tracked.join("a.txt"), b"locally modified\n").unwrap();
    let error = git
        .plan_worktree_removal(&tracked, None, None, WorktreeDiscardPolicy::AllowAll)
        .unwrap_err();
    assert!(matches!(error, GitError::DirtyWorktree { .. }), "{error:?}");
    assert!(tracked.exists());
}

#[test]
fn ignored_state_requires_the_strongest_policy() {
    let repo = TestRepo::new();
    let git = GitRepository::open(&repo.path).unwrap();
    fs::write(repo.path.join(".gitignore"), b"build/\n").unwrap();
    repo.git(&["add", ".gitignore"]);
    repo.git(&["commit", "-q", "-m", "ignore build"]);
    let path = repo.temp.path().join("ignored worktree");
    worktree_add(&repo, &path, "ignored");
    fs::create_dir_all(path.join("build")).unwrap();
    fs::write(path.join("build/out.bin"), b"artifact\n").unwrap();

    let error = git
        .plan_worktree_removal(&path, None, None, WorktreeDiscardPolicy::AllowUntracked)
        .unwrap_err();
    assert!(matches!(error, GitError::DirtyWorktree { .. }), "{error:?}");

    let plan = git
        .plan_worktree_removal(&path, None, None, WorktreeDiscardPolicy::AllowAll)
        .unwrap();
    git.apply_worktree_removal(&plan).unwrap();
    assert!(!path.exists());
}

#[test]
fn worktree_removal_refuses_a_fingerprint_that_moved_after_planning() {
    let repo = TestRepo::new();
    let git = GitRepository::open(&repo.path).unwrap();
    let path = repo.temp.path().join("moving worktree");
    worktree_add(&repo, &path, "moving");

    let plan = git
        .plan_worktree_removal(&path, None, None, WorktreeDiscardPolicy::AllowUntracked)
        .unwrap();
    fs::write(path.join("late.txt"), b"late\n").unwrap();

    let error = git.apply_worktree_removal(&plan).unwrap_err();
    assert!(matches!(error, GitError::PlanStale { .. }), "{error:?}");
    assert!(path.exists());
}

#[test]
fn worktree_removal_refuses_locked_and_detached_is_allowed_with_expected_head() {
    let repo = TestRepo::new();
    let git = GitRepository::open(&repo.path).unwrap();
    let locked = repo.temp.path().join("locked worktree");
    worktree_add(&repo, &locked, "locked");
    repo.git(&["worktree", "lock", locked.to_str().unwrap()]);
    let error = git
        .plan_worktree_removal(&locked, None, None, WorktreeDiscardPolicy::RefuseDirty)
        .unwrap_err();
    assert!(matches!(error, GitError::WorktreePathUnavailable { .. }));

    let detached = repo.temp.path().join("detached worktree");
    repo.git(&[
        "worktree",
        "add",
        "-q",
        "--detach",
        detached.to_str().unwrap(),
        "HEAD",
    ]);
    let expected = oid_from(&git_ok(&detached, &["rev-parse", "HEAD"]));
    let plan = git
        .plan_worktree_removal(
            &detached,
            Some(&expected),
            None,
            WorktreeDiscardPolicy::RefuseDirty,
        )
        .unwrap();
    assert_eq!(plan.expected_head(), Some(&expected));
    assert_eq!(plan.expected_branch(), None);
    git.apply_worktree_removal(&plan).unwrap();
    assert!(!detached.exists());
}

#[cfg(unix)]
#[test]
fn non_utf8_paths_are_retained_in_removal_evidence() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let repo = TestRepo::new();
    let git = GitRepository::open(&repo.path).unwrap();
    let path = repo.temp.path().join("non utf8 worktree");
    worktree_add(&repo, &path, "non-utf8");
    let name = OsStr::from_bytes(b"raw-\xff-file.txt");
    fs::write(path.join(name), b"local\n").unwrap();

    let error = git
        .plan_worktree_removal(&path, None, None, WorktreeDiscardPolicy::RefuseDirty)
        .unwrap_err();
    match error {
        GitError::DirtyWorktree { entries, .. } => {
            assert!(
                entries.iter().any(|entry| entry.contains('\u{fffd}')),
                "display must survive a lossy render: {entries:?}"
            );
        }
        other => panic!("expected DirtyWorktree, got {other:?}"),
    }

    let plan = git
        .plan_worktree_removal(&path, None, None, WorktreeDiscardPolicy::AllowUntracked)
        .unwrap();
    assert_eq!(plan.discarded().len(), 1);
    git.apply_worktree_removal(&plan).unwrap();
}

#[test]
fn worktree_add_refuses_a_branch_created_after_planning() {
    let repo = TestRepo::new();
    let git = GitRepository::open(&repo.path).unwrap();
    let path = repo.temp.path().join("racy worktree");
    let plan = git
        .plan_worktree_add(
            path,
            &branch("racy"),
            WorktreeStart::NewBranch(StartPoint::Head),
        )
        .unwrap();
    run_git(&repo, &["branch", "racy", "HEAD"]);

    let error = git.apply_worktree_add(&plan).unwrap_err();
    assert!(matches!(error, GitError::PlanStale { .. }), "{error:?}");
}

// ---------------------------------------------------------------------------
// Merge conclusion and in-progress state
// ---------------------------------------------------------------------------

fn create_merge_conflict(repo: &TestRepo) -> Oid {
    run_git(repo, &["checkout", "-q", "-b", "side"]);
    fs::write(repo.path.join("a.txt"), b"side\n").unwrap();
    run_git(repo, &["commit", "-q", "-am", "side"]);
    run_git(repo, &["checkout", "-q", "main"]);
    fs::write(repo.path.join("a.txt"), b"main\n").unwrap();
    run_git(repo, &["commit", "-q", "-am", "main"]);
    let output = git_raw(&repo.path, &["merge", "side"]);
    assert!(!output.status.success(), "the fixture must conflict");
    // Resolve the conflict the way an operator would; MERGE_HEAD stays until
    // the conclusion commits it.
    fs::write(repo.path.join("a.txt"), b"resolved\n").unwrap();
    run_git(repo, &["add", "a.txt"]);
    head(repo)
}

#[test]
fn merge_conclusion_commits_the_in_progress_operation() {
    let repo = TestRepo::new();
    let expected = create_merge_conflict(&repo);
    let git = GitRepository::open(&repo.path).unwrap();
    assert!(
        git.in_progress_operations()
            .unwrap()
            .contains(&sk_git::InProgressOperation::Merge)
    );

    let plan = git.plan_merge_conclusion(&expected, None).unwrap();
    let outcome = git.apply_merge_conclusion(&plan).unwrap();
    assert_eq!(outcome.previous_head(), &expected);
    assert_ne!(outcome.new_head(), &expected);
    assert_eq!(outcome.operation(), sk_git::ConcludedOperation::Merge);
    assert!(git.in_progress_operations().unwrap().is_empty());
    assert!(git.status().unwrap().is_empty());
}

#[test]
fn merge_conclusion_refuses_without_an_operation_or_after_head_moved() {
    let repo = TestRepo::new();
    let git = GitRepository::open(&repo.path).unwrap();
    let current = head(&repo);
    assert!(matches!(
        git.plan_merge_conclusion(&current, None).unwrap_err(),
        GitError::NoOperationInProgress
    ));

    let expected = create_merge_conflict(&repo);
    let plan = git
        .plan_merge_conclusion(&expected, Some("resolve conflict"))
        .unwrap();
    // A concurrent actor concludes the merge between plan and apply.
    run_git(&repo, &["commit", "--no-edit"]);
    let error = git.apply_merge_conclusion(&plan).unwrap_err();
    assert!(matches!(error, GitError::PlanStale { .. }), "{error:?}");
}

#[test]
fn merge_conclusion_rejects_an_empty_message() {
    let repo = TestRepo::new();
    let expected = create_merge_conflict(&repo);
    let git = GitRepository::open(&repo.path).unwrap();

    let error = git
        .plan_merge_conclusion(&expected, Some("  "))
        .unwrap_err();
    assert!(matches!(error, GitError::InvalidPlan { .. }), "{error:?}");
}

// ---------------------------------------------------------------------------
// Repository state
// ---------------------------------------------------------------------------

#[test]
fn git_dirs_distinguish_linked_worktrees_and_version_probe_is_bounded() {
    let repo = TestRepo::new();
    let linked = repo.temp.path().join("linked state");
    worktree_add(&repo, &linked, "state");

    let primary = GitRepository::open(&repo.path).unwrap();
    let linked_handle = GitRepository::open(&linked).unwrap();
    let primary_dirs = primary.git_dirs().unwrap();
    let linked_dirs = linked_handle.git_dirs().unwrap();

    assert!(!primary_dirs.is_linked_worktree());
    assert!(linked_dirs.is_linked_worktree());
    assert_eq!(primary_dirs.common_dir(), linked_dirs.common_dir());
    assert!(primary.git_version().unwrap().0 >= 2);
    assert!(primary.supports_merge_tree_write_tree().unwrap());
}

#[test]
fn a_missing_branch_is_typed_absence_not_a_command_failure() {
    let repo = TestRepo::new();
    let git = GitRepository::open(&repo.path).unwrap();

    assert_eq!(
        git.read_branch_tip(&branch("does-not-exist")).unwrap(),
        None
    );
    assert!(matches!(
        git.resolve_commit(&StartPoint::Ref(refname("refs/heads/does-not-exist")))
            .unwrap_err(),
        GitError::RefMissing { .. }
    ));
}

// ---------------------------------------------------------------------------
// Real repositories only
// ---------------------------------------------------------------------------

#[test]
fn tests_do_not_touch_a_path_outside_the_temporary_repository() {
    let repo = TestRepo::new();
    let before = fs::read_dir(repo.temp.path()).unwrap().count();
    let _ = GitRepository::open(&repo.path)
        .unwrap()
        .worktrees()
        .unwrap();
    assert_eq!(fs::read_dir(repo.temp.path()).unwrap().count(), before);
}

#[test]
fn path_inside_a_repository_nested_checkout_is_still_a_valid_worktree_parent() {
    // Spec Kitty places lane worktrees under `.worktrees/` inside the repo
    // root; this asserts the adapter does not reject that placement while it
    // still rejects a path inside the git metadata directory.
    let repo = TestRepo::new();
    let git = GitRepository::open(&repo.path).unwrap();
    let path = repo.path.join(".worktrees").join("lane-1");
    let plan = git
        .plan_worktree_add(
            path.clone(),
            &branch("lane-1"),
            WorktreeStart::NewBranch(StartPoint::Head),
        )
        .unwrap();
    git.apply_worktree_add(&plan).unwrap();
    assert!(path.join("a.txt").exists());
}
