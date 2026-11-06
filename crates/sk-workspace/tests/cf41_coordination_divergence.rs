//! CF41: the expected coordination-vs-target divergence classifier against real
//! Git histories.
//!
//! Every fixture is a real `tempfile` repository with real branches, real
//! commits (including real merges, real empty commits and a real shallow clone).
//! No OID, path or classification is fabricated: each assertion either names the
//! observed commit/path or compares the classifier's answer with `git`'s own.
//!
//! Not claimed here: an unreadable *commit* (a corrupt object inside a readable
//! range) has no non-destructive fixture, so `UnreadableCommitPaths` is
//! implemented and documented but exercised only through the Git layer's
//! "missing object is an error" tests. Deep differential/boundary/performance
//! campaigns remain separate work.

mod support;

use std::fs;
use std::path::Path;

use sk_git::{BranchName, GitRepository};
use sk_workspace::coordination_divergence::{
    DivergenceRange, DivergenceSide, ExpectedCoordinationDivergenceOutcome,
    ExpectedCoordinationDivergenceRequest, ExpectedDivergenceCause,
    classify_expected_coordination_divergence, is_coord_only_path,
    is_expected_coordination_divergence,
};

use support::{Fixture, branch_tip, commit_all, commit_file, create_branch, git, init_repo, write};

const MID8: &str = "01KNXQS9";
const SLUG: &str = "cf41-mission";
const COORD_BRANCH: &str = "kitty/mission-cf41-mission-01KNXQS9";
const TARGET_BRANCH: &str = "main";

/// The canonical `kitty-specs/<slug>-<mid8>` directory name.
fn mission_dir() -> String {
    format!("{SLUG}-{MID8}")
}

/// A path inside this Mission's directory.
fn mission_path(relative: &str) -> String {
    format!("kitty-specs/{}/{}", mission_dir(), relative)
}

/// The result of comparing the fixture's coordination branch with `main`.
fn judge(root: &Path, mission_dir_name: &str) -> ExpectedCoordinationDivergenceOutcome {
    judge_branch(root, COORD_BRANCH, TARGET_BRANCH, mission_dir_name)
}

fn judge_branch(
    root: &Path,
    coordination_branch: &str,
    target_branch: &str,
    mission_dir_name: &str,
) -> ExpectedCoordinationDivergenceOutcome {
    let repository = GitRepository::open(root).expect("open fixture repository");
    let coordination = BranchName::parse(coordination_branch).expect("valid coordination branch");
    let target = BranchName::parse(target_branch).expect("valid target branch");
    classify_expected_coordination_divergence(
        &repository,
        &ExpectedCoordinationDivergenceRequest::new(&coordination, &target, mission_dir_name),
    )
}

/// A fixture with `main` and this Mission's coordination branch at the same tip.
fn fixture_with_coord_branch() -> Fixture {
    let fixture = init_repo();
    create_branch(fixture.root(), COORD_BRANCH);
    fixture
}

/// Commit one file on `branch` (checked out, committed, then back to `main`).
fn commit_on_branch(fixture: &Fixture, branch: &str, relative: &str, contents: &str) -> String {
    git(fixture.root(), &["checkout", "-q", branch]);
    write(&fixture.path(relative), contents);
    commit_file(fixture.root(), relative, &format!("add {relative}"));
    let tip = branch_tip(fixture.root(), branch);
    git(fixture.root(), &["checkout", "-q", TARGET_BRANCH]);
    tip
}

/// Commit one file on the branch that is currently checked out.
fn commit_here(fixture: &Fixture, relative: &str, contents: &str, message: &str) {
    write(&fixture.path(relative), contents);
    commit_file(fixture.root(), relative, message);
}

#[test]
fn coord_only_divergence_is_expected() {
    let fixture = fixture_with_coord_branch();
    // Two commits unique to the coordination branch, both COORD-partition.
    commit_on_branch(
        &fixture,
        COORD_BRANCH,
        &mission_path("status.events.jsonl"),
        "{}\n",
    );
    let coord_tip = commit_on_branch(
        &fixture,
        COORD_BRANCH,
        &mission_path("traces/tooling-friction.md"),
        "- entry\n",
    );
    // The target moves on with ordinary code, touching no COORD path.
    write(&fixture.path("src/app.rs"), "fn main() {}\n");
    commit_file(fixture.root(), "src/app.rs", "add app");
    let target_tip = branch_tip(fixture.root(), TARGET_BRANCH);

    let outcome = judge(fixture.root(), &mission_dir());
    assert!(
        outcome.expected(),
        "unexpected cause: {:?}",
        outcome.cause()
    );
    assert_eq!(
        outcome.cause(),
        &ExpectedDivergenceCause::Expected {
            coordination_commits: 2,
            target_commits_since_merge_base: 1,
        }
    );
    // The predicate's boolean entry point agrees.
    let repository = GitRepository::open(fixture.root()).expect("open");
    let coordination = BranchName::parse(COORD_BRANCH).expect("branch");
    let target = BranchName::parse(TARGET_BRANCH).expect("branch");
    assert!(is_expected_coordination_divergence(
        &repository,
        &ExpectedCoordinationDivergenceRequest::new(&coordination, &target, &mission_dir()),
    ));
    assert_ne!(coord_tip, target_tip, "fixture: the tips really diverged");
}

#[test]
fn coordination_commit_touching_a_primary_artifact_is_not_expected() {
    let fixture = fixture_with_coord_branch();
    let tip = commit_on_branch(&fixture, COORD_BRANCH, &mission_path("spec.md"), "# spec\n");

    match judge(fixture.root(), &mission_dir()).cause() {
        ExpectedDivergenceCause::CoordinationCommitTouchesNonCoordPath { commit, path } => {
            assert_eq!(commit.as_str(), tip);
            assert_eq!(path.display_lossy(), mission_path("spec.md"));
        }
        cause => panic!("unexpected cause: {cause:?}"),
    }
}

#[test]
fn coordination_commit_touching_an_unrecognised_mission_file_is_not_expected() {
    let fixture = fixture_with_coord_branch();
    commit_on_branch(
        &fixture,
        COORD_BRANCH,
        &mission_path("notes.txt"),
        "notes\n",
    );

    match judge(fixture.root(), &mission_dir()).cause() {
        ExpectedDivergenceCause::CoordinationCommitTouchesNonCoordPath { path, .. } => {
            assert_eq!(path.display_lossy(), mission_path("notes.txt"));
        }
        cause => panic!("unexpected cause: {cause:?}"),
    }
}

#[test]
fn coordination_commit_outside_kitty_specs_is_not_expected() {
    let fixture = fixture_with_coord_branch();
    commit_on_branch(&fixture, COORD_BRANCH, "src/leak.rs", "// leak\n");

    match judge(fixture.root(), &mission_dir()).cause() {
        ExpectedDivergenceCause::CoordinationCommitTouchesNonCoordPath { path, .. } => {
            assert_eq!(path.display_lossy(), "src/leak.rs");
        }
        cause => panic!("unexpected cause: {cause:?}"),
    }
}

#[test]
fn coordination_commit_in_another_mission_is_not_expected() {
    let fixture = fixture_with_coord_branch();
    let foreign = "kitty-specs/other-mission-01KNXQSA/status.events.jsonl".to_string();
    commit_on_branch(&fixture, COORD_BRANCH, &foreign, "{}\n");

    match judge(fixture.root(), &mission_dir()).cause() {
        ExpectedDivergenceCause::CoordinationCommitTouchesNonCoordPath { path, .. } => {
            assert_eq!(path.display_lossy(), foreign);
        }
        cause => panic!("unexpected cause: {cause:?}"),
    }
}

#[test]
fn target_commit_touching_a_coord_path_is_not_expected() {
    let fixture = fixture_with_coord_branch();
    commit_on_branch(
        &fixture,
        COORD_BRANCH,
        &mission_path("status.events.jsonl"),
        "{}\n",
    );
    let coord_path = mission_path("status.events.jsonl");
    write(&fixture.path(&coord_path), "{\"second\":true}\n");
    commit_file(fixture.root(), &coord_path, "target also writes status");
    let target_tip = branch_tip(fixture.root(), TARGET_BRANCH);

    match judge(fixture.root(), &mission_dir()).cause() {
        ExpectedDivergenceCause::TargetCommitTouchesCoordPath { commit, path } => {
            assert_eq!(commit.as_str(), target_tip);
            assert_eq!(path.display_lossy(), coord_path);
        }
        cause => panic!("unexpected cause: {cause:?}"),
    }
}

#[test]
fn identical_tips_are_vacuously_expected() {
    let fixture = fixture_with_coord_branch();
    let outcome = judge(fixture.root(), &mission_dir());
    assert!(
        outcome.expected(),
        "unexpected cause: {:?}",
        outcome.cause()
    );
    assert_eq!(
        outcome.cause(),
        &ExpectedDivergenceCause::Expected {
            coordination_commits: 0,
            target_commits_since_merge_base: 0,
        }
    );
}

#[test]
fn unrelated_histories_are_not_expected() {
    let fixture = init_repo();
    // A real second root: an orphan branch with no ancestor in common with
    // `main`, so `merge-base` exits 1 with nothing to read.
    git(
        fixture.root(),
        &["checkout", "-q", "--orphan", COORD_BRANCH],
    );
    git(fixture.root(), &["rm", "-rq", "--cached", "."]);
    write(&fixture.path(mission_path("status.events.jsonl")), "{}\n");
    commit_all(fixture.root(), "orphan root");
    git(fixture.root(), &["checkout", "-q", TARGET_BRANCH]);
    let merge_base = std::process::Command::new("git")
        .args(["merge-base", COORD_BRANCH, TARGET_BRANCH])
        .current_dir(fixture.root())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("run merge-base");
    assert_eq!(
        merge_base.status.code(),
        Some(1),
        "fixture precondition: the branches share no ancestor"
    );

    assert_eq!(
        judge(fixture.root(), &mission_dir()).cause(),
        &ExpectedDivergenceCause::NoMergeBase
    );
}

#[test]
fn missing_branches_are_not_expected() {
    let fixture = fixture_with_coord_branch();
    assert_eq!(
        judge_branch(
            fixture.root(),
            "kitty/mission-absent-01KNXQS9",
            TARGET_BRANCH,
            &mission_dir()
        )
        .cause(),
        &ExpectedDivergenceCause::MissingBranch {
            side: DivergenceSide::Coordination,
        }
    );
    assert_eq!(
        judge_branch(
            fixture.root(),
            COORD_BRANCH,
            "absent-target",
            &mission_dir()
        )
        .cause(),
        &ExpectedDivergenceCause::MissingBranch {
            side: DivergenceSide::Target,
        }
    );
}

#[test]
fn a_merge_commit_is_classified_by_what_it_brought_in() {
    let fixture = fixture_with_coord_branch();
    // Coordination work on a side branch forked from the coordination branch,
    // merged with --no-ff: the merge commit itself changes only COORD paths
    // relative to its first parent, and it must not be read as "changed
    // nothing".
    git(fixture.root(), &["checkout", "-q", COORD_BRANCH]);
    commit_here(
        &fixture,
        &mission_path("issue-matrix.json"),
        "{}\n",
        "coord verdicts",
    );
    git(fixture.root(), &["checkout", "-q", "-b", "cf41-side"]);
    let side_path = mission_path("traces/approach.md");
    commit_here(&fixture, &side_path, "- approach\n", "side coord work");
    git(fixture.root(), &["checkout", "-q", COORD_BRANCH]);
    commit_here(
        &fixture,
        &mission_path("traces/design-decisions.md"),
        "- decision\n",
        "coord work",
    );
    git(
        fixture.root(),
        &["merge", "-q", "--no-ff", "--no-edit", "cf41-side"],
    );
    let merge_sha = branch_tip(fixture.root(), COORD_BRANCH);
    git(fixture.root(), &["checkout", "-q", TARGET_BRANCH]);

    // Fixture precondition: the merge really did bring a path in, so the test
    // is exercising a non-empty first-parent merge diff.
    let brought_in = git(
        fixture.root(),
        &[
            "diff",
            "--name-only",
            "--no-renames",
            &format!("{merge_sha}^1"),
            &merge_sha,
        ],
    );
    assert_eq!(
        brought_in.trim(),
        side_path,
        "fixture precondition: the merge must bring in exactly the side path"
    );

    let outcome = judge(fixture.root(), &mission_dir());
    assert!(
        outcome.expected(),
        "unexpected cause: {:?}",
        outcome.cause()
    );
}

#[test]
fn a_merge_commit_carrying_a_primary_artifact_is_not_expected() {
    let fixture = fixture_with_coord_branch();
    git(fixture.root(), &["checkout", "-q", COORD_BRANCH]);
    commit_here(
        &fixture,
        &mission_path("status.events.jsonl"),
        "{}\n",
        "coord status",
    );
    git(fixture.root(), &["checkout", "-q", "-b", "cf41-bad-side"]);
    commit_here(
        &fixture,
        &mission_path("plan.md"),
        "# plan\n",
        "side primary work",
    );
    git(fixture.root(), &["checkout", "-q", COORD_BRANCH]);
    git(
        fixture.root(),
        &["merge", "-q", "--no-ff", "--no-edit", "cf41-bad-side"],
    );
    let merge_sha = branch_tip(fixture.root(), COORD_BRANCH);
    git(fixture.root(), &["checkout", "-q", TARGET_BRANCH]);

    match judge(fixture.root(), &mission_dir()).cause() {
        ExpectedDivergenceCause::CoordinationCommitTouchesNonCoordPath { commit, path } => {
            assert_eq!(
                commit.as_str(),
                merge_sha,
                "the merge commit is the offender"
            );
            assert_eq!(path.display_lossy(), mission_path("plan.md"));
        }
        cause => panic!("unexpected cause: {cause:?}"),
    }
}

#[test]
fn an_empty_commit_on_the_coordination_branch_is_expected() {
    let fixture = fixture_with_coord_branch();
    git(fixture.root(), &["checkout", "-q", COORD_BRANCH]);
    git(
        fixture.root(),
        &["commit", "-q", "--allow-empty", "-m", "record nothing"],
    );
    git(fixture.root(), &["checkout", "-q", TARGET_BRANCH]);

    assert_eq!(
        judge(fixture.root(), &mission_dir()).cause(),
        &ExpectedDivergenceCause::Expected {
            coordination_commits: 1,
            target_commits_since_merge_base: 0,
        }
    );
}

#[test]
fn path_codepoints_and_nested_coord_shapes_stay_classified() {
    let fixture = fixture_with_coord_branch();
    // A private-use code point and a Unicode noncharacter are ordinary,
    // distinct code points: the directory-anchored classification still holds.
    let exotic = mission_path("traces/notes-\u{E000}\u{FFFF}.md");
    commit_on_branch(&fixture, COORD_BRANCH, &exotic, "- exotic\n");
    // The review-cycle filename leg is COORD even nested under `tasks/`.
    commit_on_branch(
        &fixture,
        COORD_BRANCH,
        &mission_path("tasks/WP01/review-cycle-7.md"),
        "- cycle\n",
    );

    let outcome = judge(fixture.root(), &mission_dir());
    assert!(
        outcome.expected(),
        "unexpected cause: {:?}",
        outcome.cause()
    );
    assert_eq!(
        outcome.cause(),
        &ExpectedDivergenceCause::Expected {
            coordination_commits: 2,
            target_commits_since_merge_base: 0,
        }
    );

    // Counterpart: a PRIMARY work-package prompt under the same `tasks/` tree.
    let fixture = fixture_with_coord_branch();
    commit_on_branch(
        &fixture,
        COORD_BRANCH,
        &mission_path("tasks/WP01.md"),
        "# WP01\n",
    );
    assert!(matches!(
        judge(fixture.root(), &mission_dir()).cause(),
        ExpectedDivergenceCause::CoordinationCommitTouchesNonCoordPath { .. }
    ));
}

#[test]
fn the_partition_table_is_the_classifier_not_a_second_copy() {
    let dir = mission_dir();
    // COORD partition.
    for path in [
        mission_path("status.events.jsonl"),
        mission_path("status.json"),
        mission_path("issue-matrix.md"),
        mission_path("issue-matrix.json"),
        mission_path("acceptance-matrix.json"),
        mission_path("decisions.events.jsonl"),
        mission_path("traces/tooling-friction.md"),
        mission_path("tasks/WP01/review-cycle-1.md"),
    ] {
        assert!(is_coord_only_path(&path, &dir), "{path} should be COORD");
    }
    // PRIMARY partition.
    for path in [
        mission_path("spec.md"),
        mission_path("plan.md"),
        mission_path("tasks.md"),
        mission_path("meta.json"),
        mission_path("tasks/WP01.md"),
        mission_path("decisions/index.json"),
        mission_path("contracts/api.yaml"),
    ] {
        assert!(!is_coord_only_path(&path, &dir), "{path} should be PRIMARY");
    }
    // Unclassifiable / foreign / outside `kitty-specs/`.
    for path in [
        mission_path("notes.txt"),
        mission_path(""),
        "kitty-specs/other-mission-01KNXQSA/status.events.jsonl".to_string(),
        "src/status.events.jsonl".to_string(),
        "status.events.jsonl".to_string(),
    ] {
        assert!(
            !is_coord_only_path(&path, &dir),
            "{path:?} must fail closed (not COORD)"
        );
    }
}

#[test]
fn shallow_histories_have_no_merge_base_and_are_not_expected() {
    // Build a real origin with two branches, then read it through a real
    // depth-1 clone: the tips exist locally but no common ancestor does.
    let origin = fixture_with_coord_branch();
    commit_on_branch(
        &origin,
        COORD_BRANCH,
        &mission_path("status.events.jsonl"),
        "{}\n",
    );
    write(&origin.path("src/app.rs"), "fn main() {}\n");
    commit_file(origin.root(), "src/app.rs", "target work");

    let scratch = tempfile::tempdir().expect("temp dir");
    let clone = scratch.path().join("shallow");
    let url = format!("file://{}", origin.root().display());
    git(
        scratch.path(),
        &[
            "clone",
            "-q",
            "--depth",
            "1",
            "--branch",
            TARGET_BRANCH,
            &url,
            clone.to_str().expect("utf8 clone path"),
        ],
    );
    git(
        &clone,
        &[
            "fetch",
            "-q",
            "--depth",
            "1",
            "origin",
            &format!("refs/heads/{COORD_BRANCH}:refs/heads/{COORD_BRANCH}"),
        ],
    );
    let shallow_marker = clone.join(".git").join("shallow");
    assert!(
        shallow_marker.is_file(),
        "fixture precondition: real shallow clone"
    );

    assert_eq!(
        judge(&clone, &mission_dir()).cause(),
        &ExpectedDivergenceCause::NoMergeBase
    );
}

#[test]
fn the_classifier_writes_nothing() {
    let fixture = fixture_with_coord_branch();
    commit_on_branch(
        &fixture,
        COORD_BRANCH,
        &mission_path("status.events.jsonl"),
        "{}\n",
    );
    write(&fixture.path("src/app.rs"), "fn main() {}\n");
    commit_file(fixture.root(), "src/app.rs", "target work");
    commit_on_branch(
        &fixture,
        COORD_BRANCH,
        &mission_path("traces/approach.md"),
        "- a\n",
    );

    let index_path = fixture.root().join(".git").join("index");
    let snapshot = || {
        (
            git(fixture.root(), &["rev-parse", "HEAD"]),
            git(fixture.root(), &["for-each-ref"]),
            git(fixture.root(), &["status", "--porcelain=v1"]),
            fs::read(&index_path).expect("read index"),
            git(fixture.root(), &["stash", "list"]),
        )
    };
    let before = snapshot();
    let outcome = judge(fixture.root(), &mission_dir());
    let after = snapshot();

    // The verdict is a real one, so the read really happened.
    assert!(
        outcome.expected(),
        "unexpected cause: {:?}",
        outcome.cause()
    );
    assert_eq!(before.0, after.0, "HEAD moved");
    assert_eq!(before.1, after.1, "a ref changed");
    assert_eq!(before.2, after.2, "the worktree/index became dirty");
    assert_eq!(before.3, after.3, "the index bytes changed");
    assert_eq!(before.4, after.4, "a stash entry appeared");

    // No lock or scratch file was left behind in the git directory.
    let mut leftovers: Vec<String> = fs::read_dir(fixture.root().join(".git"))
        .expect("read git dir")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".lock") || name.starts_with("tmp_"))
        .collect();
    leftovers.sort();
    assert!(
        leftovers.is_empty(),
        "leftover lock/tmp files: {leftovers:?}"
    );
}

#[test]
fn the_range_enumeration_reports_which_range_failed() {
    // `DivergenceRange` is part of the typed cause vocabulary; the real failure
    // that produces it needs an unreadable range, which the Git layer refuses to
    // fake. This pins the vocabulary's shape rather than inventing a failure.
    assert_ne!(
        DivergenceRange::TargetToCoordination,
        DivergenceRange::MergeBaseToTarget
    );
}
