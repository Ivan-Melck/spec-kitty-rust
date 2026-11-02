//! Remote-read and freshness acceptance tests.
//!
//! Remotes are local temporary bare repositories (or paths deliberately made
//! unreachable). Nothing here contacts a network or a user remote.

mod common;

use std::fs;
use std::path::PathBuf;

use common::{TestRepo, git_ok};
use sk_git::{BranchName, FreshnessState, GitRepository, RemoteBranchLookup, RemoteName};

fn branch(name: &str) -> BranchName {
    BranchName::parse(name).unwrap()
}

fn remote(name: &str) -> RemoteName {
    RemoteName::parse(name).unwrap()
}

struct RemoteFixture {
    repo: TestRepo,
    bare: PathBuf,
}

impl RemoteFixture {
    fn new() -> Self {
        let repo = TestRepo::new();
        let bare = repo.temp.path().join("remote.git");
        git_ok(
            repo.temp.path(),
            &["init", "-q", "--bare", bare.to_str().unwrap()],
        );
        repo.git(&["remote", "add", "origin", bare.to_str().unwrap()]);
        repo.git(&["push", "-q", "origin", "main"]);
        Self { repo, bare }
    }
}

#[test]
fn probe_distinguishes_a_clean_absence_from_an_unreachable_remote() {
    let fixture = RemoteFixture::new();
    let git = GitRepository::open(&fixture.repo.path).unwrap();
    let main = branch("main");
    let missing = branch("missing");
    let tip = git.read_branch_tip(&main).unwrap().unwrap();

    let heads = git
        .probe_remote_heads(&remote("origin"), &[main.clone(), missing.clone()])
        .unwrap();
    assert_eq!(heads.get(&main), Some(&tip));
    assert!(heads.is_absent(&missing));
    assert_eq!(
        git.remote_branch_lookup(&main).unwrap(),
        RemoteBranchLookup::Hit {
            remote: remote("origin"),
            oid: tip
        }
    );
    assert_eq!(
        git.remote_branch_lookup(&missing).unwrap(),
        RemoteBranchLookup::CleanMiss
    );

    // A remote that cannot answer is an error, never an absence.
    let absent_url = fixture.repo.temp.path().join("not-a-repository");
    fixture
        .repo
        .git(&["remote", "set-url", "origin", absent_url.to_str().unwrap()]);
    let error = git
        .probe_remote_heads(&remote("origin"), std::slice::from_ref(&main))
        .unwrap_err();
    assert!(
        matches!(
            error,
            sk_git::GitError::RemoteUnreachable {
                timed_out: false,
                ..
            }
        ),
        "{error:?}"
    );
    assert!(matches!(
        git.remote_branch_lookup(&main).unwrap(),
        RemoteBranchLookup::Error { .. }
    ));
}

#[test]
fn a_repository_without_remotes_retains_local_authority() {
    let repo = TestRepo::new();
    let git = GitRepository::open(&repo.path).unwrap();

    assert!(git.configured_remotes().unwrap().is_empty());
    assert_eq!(
        git.remote_branch_lookup(&branch("main")).unwrap(),
        RemoteBranchLookup::NoRemote
    );
    assert_eq!(
        git.check_branch_freshness(&branch("main")).unwrap().state(),
        FreshnessState::NoRemote
    );
}

#[test]
fn freshness_classifies_up_to_date_ahead_behind_and_diverged() {
    let fixture = RemoteFixture::new();
    let git = GitRepository::open(&fixture.repo.path).unwrap();
    let main = branch("main");

    assert_eq!(
        git.check_branch_freshness(&main).unwrap().state(),
        FreshnessState::UpToDate
    );

    let base = git.read_branch_tip(&main).unwrap().unwrap();
    fs::write(fixture.repo.path.join("ahead.txt"), b"ahead\n").unwrap();
    fixture.repo.git(&["add", "ahead.txt"]);
    fixture.repo.git(&["commit", "-q", "-m", "ahead"]);
    let ahead = git.check_branch_freshness(&main).unwrap();
    assert_eq!(ahead.state(), FreshnessState::Ahead);
    assert_eq!(ahead.ahead(), 1);
    assert_eq!(ahead.behind(), 0);

    fixture.repo.git(&["push", "-q", "origin", "main"]);
    fixture.repo.git(&["reset", "-q", "--hard", base.as_str()]);
    let behind = git.check_branch_freshness(&main).unwrap();
    assert_eq!(behind.state(), FreshnessState::Behind);
    assert_eq!(behind.behind(), 1);

    fs::write(fixture.repo.path.join("local.txt"), b"local\n").unwrap();
    fixture.repo.git(&["add", "local.txt"]);
    fixture.repo.git(&["commit", "-q", "-m", "local"]);
    let diverged = git.check_branch_freshness(&main).unwrap();
    assert_eq!(diverged.state(), FreshnessState::Diverged);
    assert_eq!(diverged.behind(), 1);
    assert_eq!(diverged.ahead(), 1);
}

#[test]
fn freshness_reports_remote_missing_and_local_missing_separately() {
    let fixture = RemoteFixture::new();
    let git = GitRepository::open(&fixture.repo.path).unwrap();

    let verdict = git
        .check_branch_freshness(&branch("never-existed"))
        .unwrap();
    assert_eq!(verdict.state(), FreshnessState::RemoteMissing);
    assert_eq!(verdict.remote_tip(), None);

    // A branch that exists only on the remote.
    let tip = git.read_branch_tip(&branch("main")).unwrap().unwrap();
    git_ok(
        &fixture.bare,
        &["update-ref", "refs/heads/remote-only", tip.as_str()],
    );
    let verdict = git.check_branch_freshness(&branch("remote-only")).unwrap();
    assert_eq!(verdict.state(), FreshnessState::LocalMissing);
    assert_eq!(verdict.remote_tip(), Some(&tip));
    assert_eq!(verdict.local_tip(), None);
}

#[test]
fn freshness_does_not_fetch_a_remote_tip_and_says_so() {
    let fixture = RemoteFixture::new();
    let git = GitRepository::open(&fixture.repo.path).unwrap();

    // A second clone creates a commit this clone has never fetched.
    let clone = fixture.repo.temp.path().join("other clone");
    git_ok(
        fixture.repo.temp.path(),
        &[
            "clone",
            "-q",
            fixture.bare.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    git_ok(&clone, &["config", "user.name", "Other"]);
    git_ok(&clone, &["config", "user.email", "other@example.com"]);
    fs::write(clone.join("other.txt"), b"other\n").unwrap();
    git_ok(&clone, &["add", "other.txt"]);
    git_ok(&clone, &["commit", "-q", "-m", "other"]);
    git_ok(&clone, &["push", "-q", "origin", "main"]);

    let verdict = git.check_branch_freshness(&branch("main")).unwrap();
    assert_eq!(verdict.state(), FreshnessState::CompareFailed);
    let detail = verdict.detail().unwrap();
    assert!(detail.contains("no fetch"), "{detail}");
    assert!(verdict.remote_tip().is_some());
}

#[test]
fn freshness_reports_unreachable_as_a_verdict_not_absence() {
    let fixture = RemoteFixture::new();
    let git = GitRepository::open(&fixture.repo.path).unwrap();
    let absent_url = fixture.repo.temp.path().join("gone");
    fixture
        .repo
        .git(&["remote", "set-url", "origin", absent_url.to_str().unwrap()]);

    let verdict = git.check_branch_freshness(&branch("main")).unwrap();
    assert_eq!(verdict.state(), FreshnessState::Unreachable);
    assert!(verdict.detail().is_some());
    assert_eq!(verdict.remote_tip(), None);
}

#[test]
fn remote_resolution_prefers_branch_config_then_the_single_remote_then_origin() {
    let repo = TestRepo::new();
    let git = GitRepository::open(&repo.path).unwrap();
    let main = branch("main");
    assert_eq!(git.resolve_remote(&main).unwrap(), None);

    repo.git(&[
        "remote",
        "add",
        "origin",
        "https://example.invalid/repo.git",
    ]);
    assert_eq!(git.resolve_remote(&main).unwrap(), Some(remote("origin")));

    repo.git(&[
        "remote",
        "add",
        "upstream",
        "https://example.invalid/up.git",
    ]);
    assert_eq!(
        git.resolve_remote(&main).unwrap(),
        Some(remote("origin")),
        "origin is the documented fallback when several remotes exist"
    );
    repo.git(&["config", "branch.main.remote", "upstream"]);
    assert_eq!(git.resolve_remote(&main).unwrap(), Some(remote("upstream")));

    repo.git(&["config", "branch.main.remote", "."]);
    assert_eq!(
        git.resolve_remote(&main).unwrap(),
        Some(remote("origin")),
        "`.` means this repository and is treated as unset"
    );
}

#[test]
fn two_remotes_where_none_owns_the_branch_are_ambiguous_not_missing() {
    let repo = TestRepo::new();
    let git = GitRepository::open(&repo.path).unwrap();
    repo.git(&["remote", "add", "first", "https://example.invalid/a.git"]);
    repo.git(&["remote", "add", "second", "https://example.invalid/b.git"]);
    repo.git(&["config", "branch.main.remote", "."]);

    let verdict = git.check_branch_freshness(&branch("main")).unwrap();
    assert_eq!(verdict.state(), FreshnessState::RemoteAmbiguous);
    let detail = verdict.detail().unwrap();
    assert!(
        detail.contains("first") && detail.contains("second"),
        "{detail}"
    );
}

#[test]
fn an_empty_probe_contacts_nothing() {
    let repo = TestRepo::new();
    let git = GitRepository::open(&repo.path).unwrap();
    // No remote is configured and the name is never used; the call must not
    // fail or contact anything.
    let heads = git.probe_remote_heads(&remote("origin"), &[]).unwrap();
    assert!(heads.heads().is_empty());
}

#[test]
fn divergence_is_measured_against_a_local_tracking_ref_without_network() {
    let fixture = RemoteFixture::new();
    let git = GitRepository::open(&fixture.repo.path).unwrap();
    let local = sk_git::RefName::branch(&branch("main"));
    let tracking = sk_git::RefName::remote_tracking(&remote("origin"), &branch("main"));
    let base = git.read_branch_tip(&branch("main")).unwrap().unwrap();

    // `git push` also updated the local tracking ref; remove it to model a
    // clone that has never fetched. An absent tracking ref is a typed command
    // failure, never a fabricated zero.
    fixture.repo.git(&["update-ref", "-d", tracking.as_str()]);
    assert_eq!(git.read_ref(&tracking).unwrap(), None);
    assert!(git.divergence(&local, &tracking).is_err());

    fixture
        .repo
        .git(&["update-ref", tracking.as_str(), base.as_str()]);
    let equal = git.divergence(&local, &tracking).unwrap();
    assert_eq!((equal.ahead, equal.behind), (0, 0));

    fs::write(fixture.repo.path.join("b.txt"), b"local\n").unwrap();
    fixture.repo.git(&["add", "b.txt"]);
    fixture.repo.git(&["commit", "-q", "-m", "local"]);
    let ahead = git.divergence(&local, &tracking).unwrap();
    assert_eq!((ahead.ahead, ahead.behind), (1, 0));

    // A commit on the remote side that this clone does not have.
    let tree = String::from_utf8(git_ok(&fixture.repo.path, &["rev-parse", "HEAD^{tree}"]))
        .unwrap()
        .trim()
        .to_string();
    let remote_only = String::from_utf8(git_ok(
        &fixture.repo.path,
        &[
            "commit-tree",
            tree.as_str(),
            "-p",
            base.as_str(),
            "-m",
            "remote",
        ],
    ))
    .unwrap()
    .trim()
    .to_string();
    fixture
        .repo
        .git(&["update-ref", tracking.as_str(), remote_only.as_str()]);
    let diverged = git.divergence(&local, &tracking).unwrap();
    assert_eq!((diverged.ahead, diverged.behind), (1, 1));
    assert!(diverged.diverged());
}
