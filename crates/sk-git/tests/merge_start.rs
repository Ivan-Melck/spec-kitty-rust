//! Real-repository tests for pinned-head merge start, conclusion and abort.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sk_git::{GitError, GitRepository, MergeStartStatus, Oid, StartPoint};

struct Repo {
    _temp: tempfile::TempDir,
    root: PathBuf,
}

impl Repo {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        for directory in ["", "home", "config", "tmp"] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        let repo = Self { _temp: temp, root };
        repo.git(&["init", "-q", "-b", "main"]);
        repo.git(&["config", "user.name", "Test"]);
        repo.git(&["config", "user.email", "test@example.invalid"]);
        repo.git(&["config", "commit.gpgsign", "false"]);
        fs::write(repo.root.join("base.txt"), b"base\n").unwrap();
        fs::write(repo.root.join("shared.txt"), b"start\n").unwrap();
        repo.git(&["add", "-A"]);
        repo.git(&["commit", "-qm", "base"]);
        repo
    }

    fn git(&self, args: &[&str]) -> Vec<u8> {
        git_in(&self.root, args)
    }

    fn head(&self) -> String {
        String::from_utf8_lossy(&self.git(&["rev-parse", "HEAD"]))
            .trim()
            .to_string()
    }

    fn open(&self) -> GitRepository {
        GitRepository::open(&self.root).unwrap()
    }

    fn commit_on(&self, branch: &str, file: &str, contents: &str, message: &str) {
        self.git(&["checkout", "-q", branch]);
        if let Some(parent) = self.root.join(file).parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(self.root.join(file), contents).unwrap();
        self.git(&["add", "-A"]);
        self.git(&["commit", "-qm", message]);
    }
}

fn git_in(root: &Path, args: &[&str]) -> Vec<u8> {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .env("HOME", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("TMPDIR", root.join("tmp"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn merge_head(repo: &Repo) -> Option<String> {
    let path = repo.root.join(".git/MERGE_HEAD");
    if !path.exists() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&fs::read(path).unwrap())
            .trim()
            .to_string(),
    )
}

#[test]
fn a_clean_merge_start_stops_at_the_pinned_head_and_concludes_with_two_parents() {
    let repo = Repo::new();
    repo.commit_on("main", "main.txt", "main\n", "main work");
    let pinned = repo.head();
    repo.git(&["checkout", "-q", "-b", "theirs"]);
    repo.commit_on("theirs", "theirs.txt", "theirs\n", "theirs work");
    let theirs = repo.head();
    repo.git(&["checkout", "-q", "main"]);

    let repository = repo.open();
    let plan = repository
        .plan_merge_start(&StartPoint::Oid(Oid::parse(&theirs).unwrap()))
        .unwrap();
    assert_eq!(plan.head().as_str(), pinned);
    assert_eq!(plan.branch().map(|branch| branch.as_str()), Some("main"));
    assert!(plan.merge_base().is_some());

    let outcome = repository.apply_merge_start(&plan).unwrap();
    assert_eq!(outcome.status(), MergeStartStatus::Stopped);
    assert!(outcome.conflicts().is_empty());
    assert_eq!(
        outcome.merge_head().map(Oid::to_string).as_deref(),
        Some(theirs.as_str())
    );
    assert_eq!(repo.head(), pinned, "the start must not move HEAD");
    assert_eq!(merge_head(&repo).as_deref(), Some(theirs.as_str()));
    let status = String::from_utf8_lossy(&repo.git(&["status", "--porcelain"]))
        .trim()
        .to_string();
    assert!(status.contains("theirs.txt"), "{status}");

    let conclusion = repository
        .plan_merge_conclusion(&Oid::parse(&pinned).unwrap(), Some("merge theirs"))
        .unwrap();
    assert_eq!(
        conclusion
            .expected_operation_head()
            .map(Oid::to_string)
            .as_deref(),
        Some(theirs.as_str()),
        "the conclusion pins MERGE_HEAD"
    );
    let concluded = repository.apply_merge_conclusion(&conclusion).unwrap();
    assert_eq!(merge_head(&repo), None);
    let parents = String::from_utf8_lossy(&repo.git(&[
        "rev-list",
        "--parents",
        "-n",
        "1",
        concluded.new_head().as_str(),
    ]))
    .trim()
    .to_string();
    assert!(
        parents.split_whitespace().count() == 3,
        "a --no-ff conclusion has two parents: {parents}"
    );
}

#[test]
fn a_conflicted_start_reports_real_stage_evidence_and_abort_restores_the_pinned_state() {
    let repo = Repo::new();
    // Diverge both sides on the same path from the same base commit.
    repo.git(&["checkout", "-q", "-b", "theirs"]);
    repo.commit_on("theirs", "shared.txt", "theirs\n", "theirs");
    let theirs = repo.head();
    repo.git(&["checkout", "-q", "main"]);
    repo.commit_on("main", "shared.txt", "ours\n", "ours");
    let pinned = repo.head();

    let repository = repo.open();
    let plan = repository
        .plan_merge_start(&StartPoint::Oid(Oid::parse(&theirs).unwrap()))
        .unwrap();
    let outcome = repository.apply_merge_start(&plan).unwrap();
    assert_eq!(outcome.status(), MergeStartStatus::Conflicted);
    assert_eq!(outcome.conflict_paths().len(), 1);
    let conflict = &outcome.conflicts()[0];
    assert_eq!(conflict.path().as_os_str(), "shared.txt");
    let stages: Vec<u8> = conflict
        .stages()
        .iter()
        .map(|stage| stage.stage())
        .collect();
    assert_eq!(
        stages,
        vec![1, 2, 3],
        "base, ours and theirs are all recorded"
    );
    let oids: Vec<&str> = conflict
        .stages()
        .iter()
        .map(|stage| stage.oid().as_str())
        .collect();
    assert_eq!(oids.len(), 3);
    assert!(oids[0] != oids[1] && oids[1] != oids[2] && oids[0] != oids[2]);
    let ours_blob =
        String::from_utf8_lossy(&repo.git(&["rev-parse", &format!("{pinned}:shared.txt")]))
            .trim()
            .to_string();
    assert_eq!(oids[1], ours_blob, "stage 2 is our committed blob");
    assert_eq!(repo.head(), pinned);

    let abort = repository.plan_merge_abort().unwrap();
    assert_eq!(abort.merge_head().as_str(), theirs);
    assert_eq!(abort.conflicts().len(), 1);
    let aborted = repository.apply_merge_abort(&abort).unwrap();
    assert_eq!(aborted.aborted_merge_head().as_str(), theirs);
    assert_eq!(aborted.restored_head().as_str(), pinned);
    assert_eq!(aborted.discarded_conflicts().len(), 1);
    assert_eq!(merge_head(&repo), None);
    assert_eq!(repo.head(), pinned);
    assert_eq!(
        fs::read(repo.root.join("shared.txt")).unwrap(),
        b"ours\n",
        "the work tree is restored to the pinned content"
    );
    assert!(
        String::from_utf8_lossy(&repo.git(&["status", "--porcelain"]))
            .trim()
            .is_empty(),
        "the abort leaves a clean checkout"
    );
}

#[test]
fn abort_refuses_when_the_merge_was_edited_after_the_plan() {
    let repo = Repo::new();
    repo.commit_on("main", "shared.txt", "ours\n", "ours");
    repo.git(&["checkout", "-q", "-b", "theirs"]);
    repo.commit_on("theirs", "shared.txt", "theirs\n", "theirs");
    let theirs = repo.head();
    repo.git(&["checkout", "-q", "main"]);
    let repository = repo.open();
    repository
        .merge_start(&StartPoint::Oid(Oid::parse(&theirs).unwrap()))
        .unwrap();

    let abort = repository.plan_merge_abort().unwrap();
    fs::write(repo.root.join("shared.txt"), b"resolved\n").unwrap();
    let error = repository.apply_merge_abort(&abort).unwrap_err();
    assert!(
        matches!(error, GitError::PlanStale { ref what, .. } if what == "merge status"),
        "an edited merge must not be discarded: {error:?}"
    );
    assert_eq!(merge_head(&repo).as_deref(), Some(theirs.as_str()));
    assert_eq!(
        fs::read(repo.root.join("shared.txt")).unwrap(),
        b"resolved\n",
        "the refusal does not touch the resolution"
    );
}

#[test]
fn a_dirty_checkout_is_refused_before_a_merge_starts() {
    let repo = Repo::new();
    repo.commit_on("main", "main.txt", "main\n", "main");
    repo.git(&["checkout", "-q", "-b", "theirs"]);
    repo.commit_on("theirs", "theirs.txt", "theirs\n", "theirs");
    let theirs = repo.head();
    repo.git(&["checkout", "-q", "main"]);
    fs::write(repo.root.join("base.txt"), b"operator edit\n").unwrap();

    let repository = repo.open();
    let error = repository
        .plan_merge_start(&StartPoint::Oid(Oid::parse(&theirs).unwrap()))
        .unwrap_err();
    assert!(matches!(error, GitError::DirtyWorktree { .. }), "{error:?}");
    assert_eq!(merge_head(&repo), None);

    // An untracked file does not block a start and survives it.
    repo.git(&["checkout", "-q", "--", "base.txt"]);
    fs::write(repo.root.join("untracked.txt"), b"user data\n").unwrap();
    let plan = repository
        .plan_merge_start(&StartPoint::Oid(Oid::parse(&theirs).unwrap()))
        .unwrap();
    repository.apply_merge_start(&plan).unwrap();
    assert_eq!(
        fs::read(repo.root.join("untracked.txt")).unwrap(),
        b"user data\n"
    );
}

#[test]
fn a_start_refuses_a_moved_head_and_an_in_progress_operation() {
    let repo = Repo::new();
    repo.commit_on("main", "main.txt", "main\n", "main");
    repo.git(&["checkout", "-q", "-b", "theirs"]);
    repo.commit_on("theirs", "theirs.txt", "theirs\n", "theirs");
    let theirs = repo.head();
    repo.git(&["checkout", "-q", "main"]);
    let repository = repo.open();
    let plan = repository
        .plan_merge_start(&StartPoint::Oid(Oid::parse(&theirs).unwrap()))
        .unwrap();

    repo.git(&["checkout", "-q", "-b", "moved"]);
    fs::write(repo.root.join("moved.txt"), b"moved\n").unwrap();
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-qm", "moved"]);
    let error = repository.apply_merge_start(&plan).unwrap_err();
    assert!(
        matches!(error, GitError::PlanStale { ref what, .. } if what == "HEAD"),
        "{error:?}"
    );

    repo.git(&["checkout", "-q", "main"]);
    repository
        .merge_start(&StartPoint::Oid(Oid::parse(&theirs).unwrap()))
        .unwrap();
    let error = repository
        .plan_merge_start(&StartPoint::Oid(Oid::parse(&theirs).unwrap()))
        .unwrap_err();
    assert!(
        matches!(error, GitError::OperationInProgress { .. }),
        "{error:?}"
    );
    repository.merge_abort().unwrap();
}

#[test]
fn merging_an_ancestor_reports_already_up_to_date_without_state() {
    let repo = Repo::new();
    repo.commit_on("main", "main.txt", "main\n", "main");
    let ancestor = repo.head();
    repo.git(&["checkout", "-q", "-b", "later"]);
    repo.commit_on("later", "later.txt", "later\n", "later");
    let repository = repo.open();
    let outcome = repository
        .merge_start(&StartPoint::Oid(Oid::parse(&ancestor).unwrap()))
        .unwrap();
    assert_eq!(outcome.status(), MergeStartStatus::AlreadyUpToDate);
    assert!(!outcome.in_progress());
    assert_eq!(merge_head(&repo), None);
    assert_eq!(outcome.head().as_str(), repo.head());
}

// ---------------------------------------------------------------------------
// Conclusion identity: the commit this call created, not whatever HEAD the
// hooks left behind.
// ---------------------------------------------------------------------------

fn write_hook(repo: &Repo, name: &str, body: &str) {
    let path = repo.root.join(".git/hooks").join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
    }
}

/// A repository stopped mid-merge on `main`, plus a decoy commit the hook will
/// point the branch at.
fn stopped_merge_with_decoy() -> (Repo, String, String) {
    let repo = Repo::new();
    repo.commit_on("main", "main.txt", "main\n", "main work");
    let pinned = repo.head();
    repo.git(&["checkout", "-q", "-b", "theirs"]);
    repo.commit_on("theirs", "theirs.txt", "theirs\n", "theirs work");
    let theirs = repo.head();
    repo.commit_on("theirs", "more.txt", "more\n", "more theirs");
    let decoy = repo.head();
    repo.git(&["checkout", "-q", "main"]);
    repo.git(&["merge", "--no-ff", "--no-commit", &theirs]);
    (repo, pinned, decoy)
}

#[test]
fn a_hook_moved_head_is_never_reported_as_the_created_conclusion_commit() {
    let (repo, pinned, decoy) = stopped_merge_with_decoy();
    write_hook(
        &repo,
        "post-commit",
        &format!(
            "#!/bin/sh
git update-ref refs/heads/main {decoy}
"
        ),
    );

    let repository = repo.open();
    let plan = repository
        .plan_merge_conclusion(&Oid::parse(&pinned).unwrap(), Some("merge theirs"))
        .unwrap();
    assert_eq!(plan.worktree_root(), repository.root());
    assert_eq!(plan.branch().map(|branch| branch.as_str()), Some("main"));
    let error = repository
        .apply_merge_conclusion(&plan)
        .expect_err("a hook-moved HEAD cannot be reported as the created commit");
    let GitError::MergeConclusionUnconfirmed(unconfirmed) = error else {
        panic!("expected MergeConclusionUnconfirmed, got {error:?}");
    };
    let operation = &unconfirmed.operation;
    let candidate = unconfirmed.candidate.clone();
    let previous_head = &unconfirmed.previous_head;
    let observed_head = unconfirmed.observed_head.clone();
    let operation_consumed = unconfirmed.operation_consumed;
    assert_eq!(operation, "merge");
    assert_eq!(previous_head.as_str(), pinned);
    assert_eq!(
        observed_head.as_ref().map(Oid::to_string).as_deref(),
        Some(decoy.as_str()),
        "the observed HEAD is reported separately"
    );
    let created = candidate.expect("the created merge commit is still provable");
    assert_ne!(created.as_str(), decoy, "the decoy was never created here");
    assert!(operation_consumed, "the conclusion did consume MERGE_HEAD");

    // The real merge commit exists, carries both pinned parents, and is the
    // reflog entry git wrote for this conclusion. Nothing was rolled back.
    let parents =
        String::from_utf8_lossy(&repo.git(&["rev-list", "--parents", "-n", "1", created.as_str()]))
            .trim()
            .to_string();
    let parents: Vec<&str> = parents.split_whitespace().collect();
    assert_eq!(parents.len(), 3, "a --no-ff merge commit has two parents");
    assert_eq!(parents[0], created.as_str());
    assert!(parents.contains(&pinned.as_str()));
    let merge_tip = plan
        .expected_operation_head()
        .expect("the plan pinned MERGE_HEAD")
        .to_string();
    assert!(
        parents.contains(&merge_tip.as_str()),
        "the merged tip is the second parent: {parents:?} vs {merge_tip}"
    );
    let reflog =
        String::from_utf8_lossy(&repo.git(&["log", "-g", "--format=%H", "HEAD"])).to_string();
    assert!(
        reflog.lines().any(|line| line.trim() == created.as_str()),
        "the created commit stays recoverable from the reflog: {reflog}"
    );
    assert_eq!(repo.head(), decoy, "the hook's movement is not undone");
    assert_eq!(merge_head(&repo), None);
    assert!(
        fs::read(repo.root.join("theirs.txt")).is_ok(),
        "the merged work tree content is left in place"
    );
}

#[test]
fn a_conclusion_that_leaves_the_operation_marker_is_not_reported_as_success() {
    let (repo, pinned, _decoy) = stopped_merge_with_decoy();
    let theirs = String::from_utf8_lossy(&repo.git(&["rev-parse", "MERGE_HEAD"]))
        .trim()
        .to_string();
    write_hook(
        &repo,
        "post-commit",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' {theirs} > \"$(git rev-parse --absolute-git-dir)/MERGE_HEAD\"\n"
        ),
    );

    let repository = repo.open();
    let plan = repository
        .plan_merge_conclusion(&Oid::parse(&pinned).unwrap(), Some("merge theirs"))
        .unwrap();
    let error = repository
        .apply_merge_conclusion(&plan)
        .expect_err("a conclusion that leaves MERGE_HEAD behind is unconfirmed");
    let GitError::MergeConclusionUnconfirmed(unconfirmed) = error else {
        panic!("expected MergeConclusionUnconfirmed, got {error:?}");
    };
    let candidate = unconfirmed.candidate.clone();
    let observed_head = unconfirmed.observed_head.clone();
    let operation_consumed = unconfirmed.operation_consumed;
    let created = candidate.expect("the created commit is still provable");
    assert_eq!(
        observed_head.as_ref().map(Oid::to_string).as_deref(),
        Some(created.as_str()),
        "HEAD did land on the created commit"
    );
    assert!(!operation_consumed, "the hook re-created MERGE_HEAD");
    // The landed commit is kept: no reset, no delete.
    assert_eq!(repo.head(), created.as_str());
    assert_eq!(merge_head(&repo).as_deref(), Some(theirs.as_str()));
}

#[test]
fn a_rejecting_pre_commit_hook_is_a_refusal_that_leaves_the_operation_in_progress() {
    let (repo, pinned, _decoy) = stopped_merge_with_decoy();
    write_hook(
        &repo,
        "pre-commit",
        "#!/bin/sh\necho 'pre-commit says no' >&2\nexit 1\n",
    );

    let repository = repo.open();
    let plan = repository
        .plan_merge_conclusion(&Oid::parse(&pinned).unwrap(), Some("merge theirs"))
        .unwrap();
    let error = repository.apply_merge_conclusion(&plan).unwrap_err();
    assert!(
        matches!(error, GitError::CommandFailed { .. }),
        "a rejected conclusion is a plain refusal, not a landing: {error:?}"
    );
    assert_eq!(repo.head(), pinned, "nothing landed");
    assert!(
        merge_head(&repo).is_some(),
        "the operation stays in progress for the caller's abort path"
    );
    let reflog = String::from_utf8_lossy(&repo.git(&["reflog", "show", "HEAD"])).to_string();
    assert!(
        !reflog.lines().any(|line| line.contains("Merge branch")),
        "no conclusion commit reached the reflog: {reflog}"
    );
}

#[test]
fn a_failing_post_commit_hook_cannot_change_a_confirmed_conclusion() {
    let (repo, pinned, _decoy) = stopped_merge_with_decoy();
    write_hook(
        &repo,
        "post-commit",
        "#!/bin/sh\necho ran >> .git/post-commit-ran\nexit 1\n",
    );

    let repository = repo.open();
    let plan = repository
        .plan_merge_conclusion(&Oid::parse(&pinned).unwrap(), Some("merge theirs"))
        .unwrap();
    let outcome = repository
        .apply_merge_conclusion(&plan)
        .expect("git ignores post-commit's exit status; the landing is confirmed");
    assert_eq!(outcome.previous_head().as_str(), pinned);
    assert_eq!(outcome.new_head().as_str(), repo.head());
    assert_eq!(outcome.branch().map(|branch| branch.as_str()), Some("main"));
    assert!(
        repo.root.join(".git/post-commit-ran").exists(),
        "the hook really ran: hooks are preserved, not disabled"
    );
    assert_eq!(merge_head(&repo), None);
}

fn conflicted_cherry_pick(repo: &Repo) -> String {
    repo.commit_on("main", "topic.txt", "main\n", "main edits the topic file");
    repo.git(&["checkout", "-q", "-b", "picked"]);
    repo.commit_on("picked", "topic.txt", "picked\n", "picked change");
    let picked = repo.head();
    repo.git(&["checkout", "-q", "main"]);
    repo.commit_on("main", "topic.txt", "later main\n", "main edits again");
    let failed = Command::new("git")
        .current_dir(&repo.root)
        .args(["cherry-pick", &picked])
        .env("HOME", repo.root.join("home"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(!failed.status.success(), "the fixture must conflict");
    fs::write(repo.root.join("topic.txt"), b"resolved cherry-pick\n").unwrap();
    repo.git(&["add", "topic.txt"]);
    assert!(repo.root.join(".git/CHERRY_PICK_HEAD").exists());
    picked
}

#[test]
fn a_conflicted_cherry_pick_concludes_with_the_picked_commit_as_second_parent() {
    let repo = Repo::new();
    let picked = conflicted_cherry_pick(&repo);
    let repository = repo.open();
    let head = Oid::parse(&repo.head()).unwrap();
    let plan = repository.plan_merge_conclusion(&head, None).unwrap();
    assert_eq!(plan.operation(), sk_git::ConcludedOperation::CherryPick);
    assert_eq!(
        plan.expected_operation_head()
            .map(Oid::to_string)
            .as_deref(),
        Some(picked.as_str())
    );
    let outcome = repository.apply_merge_conclusion(&plan).unwrap();
    assert_eq!(outcome.operation(), sk_git::ConcludedOperation::CherryPick);
    assert_eq!(outcome.new_head().as_str(), repo.head());
    assert!(!repo.root.join(".git/CHERRY_PICK_HEAD").exists());
    let parents = String::from_utf8_lossy(&repo.git(&[
        "rev-list",
        "--parents",
        "-n",
        "1",
        outcome.new_head().as_str(),
    ]))
    .trim()
    .to_string();
    let parents: Vec<&str> = parents.split_whitespace().collect();
    assert_eq!(
        parents.len(),
        2,
        "a cherry-pick commit keeps the pinned HEAD as its only parent"
    );
    let message = String::from_utf8_lossy(&repo.git(&[
        "log",
        "-1",
        "--format=%B",
        outcome.new_head().as_str(),
    ]))
    .to_string();
    assert!(
        message.starts_with("picked change"),
        "`--no-edit` keeps git's prepared cherry-pick message: {message}"
    );
}

#[test]
fn a_conflicted_revert_concludes_with_the_reverted_commit_as_second_parent() {
    let repo = Repo::new();
    repo.commit_on("main", "revertible.txt", "one\n", "one");
    repo.commit_on("main", "revertible.txt", "two\n", "two");
    let to_revert = repo.head();
    repo.commit_on("main", "revertible.txt", "three\n", "three");
    let failed = Command::new("git")
        .current_dir(&repo.root)
        .args(["revert", "--no-edit", &to_revert])
        .env("HOME", repo.root.join("home"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(!failed.status.success(), "the fixture must conflict");
    fs::write(repo.root.join("revertible.txt"), b"resolved revert\n").unwrap();
    repo.git(&["add", "revertible.txt"]);
    assert!(repo.root.join(".git/REVERT_HEAD").exists());

    let repository = repo.open();
    let head = Oid::parse(&repo.head()).unwrap();
    let plan = repository
        .plan_merge_conclusion(&head, Some("revert two"))
        .unwrap();
    assert_eq!(plan.operation(), sk_git::ConcludedOperation::Revert);
    let outcome = repository.apply_merge_conclusion(&plan).unwrap();
    assert_eq!(outcome.operation(), sk_git::ConcludedOperation::Revert);
    assert_eq!(outcome.new_head().as_str(), repo.head());
    assert!(!repo.root.join(".git/REVERT_HEAD").exists());
    let parents = String::from_utf8_lossy(&repo.git(&[
        "rev-list",
        "--parents",
        "-n",
        "1",
        outcome.new_head().as_str(),
    ]))
    .trim()
    .to_string();
    assert_eq!(
        parents.split_whitespace().count(),
        2,
        "a revert commit keeps the pinned HEAD as its only parent: {parents}"
    );
    let message = String::from_utf8_lossy(&repo.git(&[
        "log",
        "-1",
        "--format=%s",
        outcome.new_head().as_str(),
    ]))
    .trim()
    .to_string();
    assert_eq!(
        message, "revert two",
        "the explicit conclusion message is used verbatim"
    );
}

#[test]
fn abort_preserves_untracked_files_and_refuses_an_untracked_change_after_planning() {
    let repo = Repo::new();
    repo.commit_on("main", "main.txt", "main\n", "main work");
    let pinned = repo.head();
    repo.git(&["checkout", "-q", "-b", "theirs"]);
    repo.commit_on("theirs", "theirs.txt", "theirs\n", "theirs work");
    let theirs = repo.head();
    repo.git(&["checkout", "-q", "main"]);
    fs::write(repo.root.join("operator-note.txt"), b"mine\n").unwrap();
    repo.git(&["merge", "--no-ff", "--no-commit", &theirs]);

    let repository = repo.open();
    let plan = repository.plan_merge_abort().unwrap();
    // The abort fingerprint covers untracked files: a new one after planning is
    // a stale plan, not a silent discard.
    fs::write(repo.root.join("late-note.txt"), b"late\n").unwrap();
    let error = repository.apply_merge_abort(&plan).unwrap_err();
    assert!(
        matches!(error, GitError::PlanStale { ref what, .. } if what == "merge status"),
        "{error:?}"
    );

    let plan = repository.plan_merge_abort().unwrap();
    let outcome = repository.apply_merge_abort(&plan).unwrap();
    assert_eq!(outcome.restored_head().as_str(), pinned);
    assert_eq!(outcome.aborted_merge_head().as_str(), theirs);
    assert_eq!(merge_head(&repo), None);
    assert_eq!(repo.head(), pinned);
    assert_eq!(
        fs::read(repo.root.join("operator-note.txt")).unwrap(),
        b"mine\n",
        "git merge --abort preserves untracked files"
    );
    assert_eq!(
        fs::read(repo.root.join("late-note.txt")).unwrap(),
        b"late\n"
    );
}

// ---------------------------------------------------------------------------
// Creation evidence: a parent-set match is necessary, not proof that *this*
// call created the commit.
// ---------------------------------------------------------------------------

/// Build the exact `--no-ff` merge result once, then rewind `main` to the
/// pinned head so the same conclusion can be attempted again. The prior commit
/// has exactly the pinned parent pair and stays in the reflog.
fn prior_identical_merge(repo: &Repo, pinned: &str, theirs: &str) -> String {
    repo.git(&["merge", "--no-ff", "--no-commit", theirs]);
    repo.git(&["commit", "-qm", "prior identical merge"]);
    let prior = repo.head();
    repo.git(&["reset", "--hard", pinned]);
    prior
}

fn parent_ids(repo: &Repo, commit: &str) -> Vec<String> {
    String::from_utf8_lossy(&repo.git(&["rev-list", "--parents", "-n", "1", commit]))
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

#[test]
fn a_prior_identical_merge_in_the_reflog_is_not_evidence_this_call_created_one() {
    let repo = Repo::new();
    repo.commit_on("main", "main.txt", "main\n", "main work");
    let pinned = repo.head();
    repo.git(&["checkout", "-q", "-b", "theirs"]);
    repo.commit_on("theirs", "theirs.txt", "theirs\n", "theirs work");
    let theirs = repo.head();
    repo.git(&["checkout", "-q", "main"]);

    let prior = prior_identical_merge(&repo, &pinned, &theirs);
    let prior_parents = parent_ids(&repo, &prior);
    assert_eq!(prior_parents.len(), 3);
    assert!(prior_parents.contains(&pinned));
    assert!(prior_parents.contains(&theirs));
    let reflog = String::from_utf8_lossy(&repo.git(&["log", "-g", "--format=%H", "HEAD"]))
        .trim()
        .to_string();
    assert!(
        reflog.lines().any(|line| line == prior),
        "the prior conclusion stays in the reflog: {reflog}"
    );

    repo.git(&["merge", "--no-ff", "--no-commit", &theirs]);
    write_hook(
        &repo,
        "pre-commit",
        "#!/bin/sh\necho 'pre-commit says no' >&2\nexit 1\n",
    );

    let repository = repo.open();
    let plan = repository
        .plan_merge_conclusion(&Oid::parse(&pinned).unwrap(), Some("merge theirs"))
        .unwrap();
    let error = repository
        .apply_merge_conclusion(&plan)
        .expect_err("a rejected conclusion created nothing");
    assert!(
        matches!(error, GitError::CommandFailed { .. }),
        "a foreign historical merge with the pinned parents is not creation evidence: {error:?}"
    );
    assert_eq!(repo.head(), pinned, "nothing landed");
    assert_eq!(
        merge_head(&repo).as_deref(),
        Some(theirs.as_str()),
        "the rejected operation stays in progress"
    );
}

#[test]
fn a_hook_that_repoints_head_to_a_foreign_same_parent_commit_is_not_a_confirmed_landing() {
    let repo = Repo::new();
    repo.commit_on("main", "main.txt", "main\n", "main work");
    let pinned = repo.head();
    repo.git(&["checkout", "-q", "-b", "theirs"]);
    repo.commit_on("theirs", "theirs.txt", "theirs\n", "theirs work");
    let theirs = repo.head();
    repo.git(&["checkout", "-q", "main"]);

    let foreign = prior_identical_merge(&repo, &pinned, &theirs);
    // Keep the foreign commit alive while the hook prunes this call's object.
    repo.git(&["update-ref", "refs/heads/foreign", &foreign]);
    repo.git(&["merge", "--no-ff", "--no-commit", &theirs]);
    write_hook(
        &repo,
        "post-commit",
        &format!(
            "#!/bin/sh
git update-ref refs/heads/main {foreign}
git reflog expire --expire=now --expire-unreachable=now --all
git prune --expire=now
"
        ),
    );

    let repository = repo.open();
    let plan = repository
        .plan_merge_conclusion(&Oid::parse(&pinned).unwrap(), Some("merge theirs"))
        .unwrap();
    let error = repository
        .apply_merge_conclusion(&plan)
        .expect_err("a pre-existing same-parent commit is never a created landing");
    let GitError::MergeConclusionUnconfirmed(unconfirmed) = &error else {
        panic!("expected MergeConclusionUnconfirmed, got {error:?}");
    };
    assert_eq!(
        unconfirmed.candidate, None,
        "no evidence proves a commit this call created"
    );
    assert_eq!(
        unconfirmed.observed_head.as_ref().map(Oid::to_string),
        Some(foreign.clone()),
        "the hook's foreign target is reported only as the observed HEAD"
    );
    assert!(
        unconfirmed.operation_consumed,
        "the hook only rewrote the branch after the marker was consumed"
    );
    assert_eq!(repo.head(), foreign, "the hook's movement is not undone");
    assert_eq!(merge_head(&repo), None);
    assert!(
        fs::read(repo.root.join("theirs.txt")).is_ok(),
        "the merged work tree content is left in place"
    );
}

#[test]
fn a_noisy_post_commit_hook_cannot_inject_a_fake_summary_candidate() {
    let (repo, pinned, _decoy) = stopped_merge_with_decoy();
    write_hook(
        &repo,
        "post-commit",
        "#!/bin/sh\necho '[main 1111111] fake merge summary'\n",
    );

    let repository = repo.open();
    let plan = repository
        .plan_merge_conclusion(&Oid::parse(&pinned).unwrap(), Some("merge theirs"))
        .unwrap();
    let outcome = repository
        .apply_merge_conclusion(&plan)
        .expect("the real landing is still confirmed");
    assert_eq!(
        outcome.new_head().as_str(),
        repo.head(),
        "the reported commit is the real one, not the hook's fake summary"
    );
    assert_ne!(outcome.new_head().as_str(), "1111111");
    assert_eq!(merge_head(&repo), None);
}

#[test]
fn a_post_commit_hook_that_amends_keeps_the_created_commit_as_the_candidate() {
    let (repo, pinned, _decoy) = stopped_merge_with_decoy();
    write_hook(
        &repo,
        "post-commit",
        "#!/bin/sh\nif [ -n \"$SK_AMENDING\" ]; then exit 0; fi\nSK_AMENDING=1 GIT_COMMITTER_DATE='2001-02-03T04:05:06 +0000' git -c commit.gpgsign=false commit --amend --no-edit --quiet\n",
    );

    let repository = repo.open();
    let plan = repository
        .plan_merge_conclusion(&Oid::parse(&pinned).unwrap(), Some("merge theirs"))
        .unwrap();
    let error = repository
        .apply_merge_conclusion(&plan)
        .expect_err("the hook replaced the created commit with an amended one");
    let GitError::MergeConclusionUnconfirmed(unconfirmed) = &error else {
        panic!("expected MergeConclusionUnconfirmed, got {error:?}");
    };
    let created = unconfirmed
        .candidate
        .clone()
        .expect("git's own summary names the commit this call created");
    let observed = unconfirmed
        .observed_head
        .clone()
        .expect("HEAD landed on the amended commit");
    assert_ne!(
        created, observed,
        "the amended commit is a different object from the one this call created"
    );
    for commit in [&created, &observed] {
        let parents = parent_ids(&repo, commit.as_str());
        assert_eq!(parents.len(), 3);
        assert!(parents.contains(&pinned));
        let merge_tip = plan.expected_operation_head().unwrap();
        assert!(parents.contains(&merge_tip.to_string()));
    }
    assert_eq!(repo.head(), observed.to_string());
    assert!(unconfirmed.operation_consumed);
    assert_eq!(merge_head(&repo), None);
}

#[test]
fn a_post_commit_hook_that_overruns_the_capture_bound_is_unconfirmed_not_retryable() {
    let (repo, pinned, _decoy) = stopped_merge_with_decoy();
    write_hook(
        &repo,
        "post-commit",
        "#!/bin/sh\nhead -c 17825792 /dev/zero >&2\n",
    );

    let repository = repo.open();
    let plan = repository
        .plan_merge_conclusion(&Oid::parse(&pinned).unwrap(), Some("merge theirs"))
        .unwrap();
    let error = repository
        .apply_merge_conclusion(&plan)
        .expect_err("the bounded process failed after the commit may have landed");
    let GitError::MergeConclusionUnconfirmed(unconfirmed) = &error else {
        panic!(
            "a started process that may have mutated must stay typed-unconfirmed, got {error:?}"
        );
    };
    let created = unconfirmed
        .observed_head
        .clone()
        .expect("the commit landed before the hook overran the bound");
    assert_eq!(
        unconfirmed.candidate.as_ref(),
        Some(&created),
        "the created commit is retained as evidence"
    );
    assert!(unconfirmed.operation_consumed);
    assert_eq!(repo.head(), created.to_string());
    assert_eq!(merge_head(&repo), None);
}

#[test]
fn a_failing_conclusion_that_rewrites_the_marker_is_not_a_plain_refusal() {
    let (repo, pinned, decoy) = stopped_merge_with_decoy();
    write_hook(
        &repo,
        "pre-commit",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' {decoy} > \"$(git rev-parse --absolute-git-dir)/MERGE_HEAD\"\nexit 1\n"
        ),
    );

    let repository = repo.open();
    let plan = repository
        .plan_merge_conclusion(&Oid::parse(&pinned).unwrap(), Some("merge theirs"))
        .unwrap();
    let error = repository.apply_merge_conclusion(&plan).unwrap_err();
    let GitError::MergeConclusionUnconfirmed(unconfirmed) = &error else {
        panic!("a rewritten marker is not the untouched pinned operation: {error:?}");
    };
    assert_eq!(
        unconfirmed.marker,
        sk_git::OperationMarkerState::Changed {
            recorded: Oid::parse(&decoy).unwrap()
        }
    );
    assert!(!unconfirmed.operation_consumed);
    assert_eq!(unconfirmed.candidate, None);
    assert_eq!(repo.head(), pinned, "nothing landed");
}

#[test]
fn a_failing_conclusion_that_makes_the_marker_unreadable_is_unconfirmed() {
    let (repo, pinned, _decoy) = stopped_merge_with_decoy();
    write_hook(
        &repo,
        "pre-commit",
        "#!/bin/sh\nprintf 'not an object id\\n' > \"$(git rev-parse --absolute-git-dir)/MERGE_HEAD\"\nexit 1\n",
    );

    let repository = repo.open();
    let plan = repository
        .plan_merge_conclusion(&Oid::parse(&pinned).unwrap(), Some("merge theirs"))
        .unwrap();
    let error = repository.apply_merge_conclusion(&plan).unwrap_err();
    let GitError::MergeConclusionUnconfirmed(unconfirmed) = &error else {
        panic!("an unreadable marker cannot prove the operation is untouched: {error:?}");
    };
    assert_eq!(unconfirmed.marker, sk_git::OperationMarkerState::Unreadable);
    assert!(!unconfirmed.operation_consumed);
    assert_eq!(unconfirmed.candidate, None);
    assert_eq!(repo.head(), pinned, "nothing landed");
}

#[test]
fn a_started_process_failure_before_landing_stays_typed_unconfirmed() {
    let (repo, pinned, _decoy) = stopped_merge_with_decoy();
    write_hook(
        &repo,
        "pre-commit",
        "#!/bin/sh\nhead -c 17825792 /dev/zero >&2\nexit 1\n",
    );

    let repository = repo.open();
    let plan = repository
        .plan_merge_conclusion(&Oid::parse(&pinned).unwrap(), Some("merge theirs"))
        .unwrap();
    let error = repository
        .apply_merge_conclusion(&plan)
        .expect_err("the started child did not finish, so nothing is proven");
    let GitError::MergeConclusionUnconfirmed(unconfirmed) = &error else {
        panic!("a started process failure is never a retryable NotLanded: {error:?}");
    };
    assert_eq!(unconfirmed.candidate, None);
    assert_eq!(unconfirmed.previous_head.as_str(), pinned);
    assert_eq!(
        unconfirmed
            .observed_head
            .as_ref()
            .map(Oid::to_string)
            .as_deref(),
        Some(pinned.as_str())
    );
    assert_eq!(
        unconfirmed.marker,
        sk_git::OperationMarkerState::Unchanged,
        "the marker is unchanged, but the started process cannot be proven harmless"
    );
    assert_eq!(repo.head(), pinned);
    assert!(merge_head(&repo).is_some());
}
