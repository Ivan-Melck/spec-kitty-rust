#![forbid(unsafe_code)]
//! Coordinator regressions against the public sk-git API; probe roots retained.

use sk_git::{GitError, GitOptions, GitRepository, Oid};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::Duration,
};

struct Case {
    root: PathBuf,
    pinned: String,
    foreign: Option<String>,
}

fn output(root: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .current_dir(root)
        .args(args)
        .env("HOME", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("TMPDIR", root.join("tmp"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Coordinator")
        .env("GIT_COMMITTER_NAME", "Coordinator")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .unwrap()
}
fn git(root: &Path, args: &[&str]) -> String {
    let result = output(root, args);
    assert!(
        result.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap().trim().to_owned()
}
fn stopped(with_prior: bool) -> Case {
    let root = tempfile::Builder::new()
        .prefix("sk-boundary-assert-")
        .tempdir()
        .unwrap()
        .keep();
    eprintln!("retained probe root: {}", root.display());
    for dir in ["home", "config", "tmp"] {
        fs::create_dir(root.join(dir)).unwrap();
    }
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["config", "user.name", "Coordinator"]);
    git(&root, &["config", "user.email", "test@example.invalid"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    fs::write(root.join("base"), "base\n").unwrap();
    git(&root, &["add", "base"]);
    git(&root, &["commit", "-qm", "base"]);
    let pinned = git(&root, &["rev-parse", "HEAD"]);
    git(&root, &["checkout", "-qb", "theirs"]);
    fs::write(root.join("theirs"), "theirs\n").unwrap();
    git(&root, &["add", "theirs"]);
    git(&root, &["commit", "-qm", "theirs"]);
    git(&root, &["checkout", "-q", "main"]);
    let foreign = if with_prior {
        git(&root, &["merge", "--no-ff", "--no-commit", "theirs"]);
        git(&root, &["commit", "-qm", "prior identical merge"]);
        let prior = git(&root, &["rev-parse", "HEAD"]);
        git(&root, &["update-ref", "refs/heads/foreign", &prior]);
        git(&root, &["reset", "--hard", &pinned]);
        Some(prior)
    } else {
        None
    };
    git(&root, &["merge", "--no-ff", "--no-commit", "theirs"]);
    Case {
        root,
        pinned,
        foreign,
    }
}
fn hook(case: &Case, name: &str, body: &str) {
    let path = case.root.join(".git/hooks").join(name);
    fs::write(&path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}
fn apply(repo: &GitRepository, case: &Case) -> Result<sk_git::MergeConclusionOutcome, GitError> {
    let plan = repo
        .plan_merge_conclusion(
            &Oid::parse(&case.pinned).unwrap(),
            Some("conclude current merge"),
        )
        .unwrap();
    repo.apply_merge_conclusion(&plan)
}

#[test]
fn rejecting_hook_deleting_boundary_does_not_revive_historical_creation_evidence() {
    let case = stopped(true);
    hook(
        &case,
        "pre-commit",
        "#!/bin/sh\ngit reflog delete --rewrite refs/heads/main@{0}\nexit 1\n",
    );
    let repo = GitRepository::open(&case.root).unwrap();
    let result = apply(&repo, &case);
    assert_eq!(git(&case.root, &["rev-parse", "HEAD"]), case.pinned);
    assert!(case.root.join(".git/MERGE_HEAD").exists());
    assert!(
        matches!(result, Err(GitError::CommandFailed { .. })),
        "expected ordinary refusal, got {result:?}"
    );
}

#[test]
fn selective_rewrite_cannot_confirm_preexisting_same_parent_commit() {
    let case = stopped(true);
    let foreign = case.foreign.as_ref().unwrap();
    hook(
        &case,
        "post-commit",
        &format!(
            "#!/bin/sh\nset -eu\ngit rev-parse HEAD > .git/child-created\ngit update-ref refs/heads/main {foreign}\nfor i in 1 2 3; do git reflog delete --rewrite refs/heads/main@{{0}}; done\nfor i in 1 2 3; do git reflog delete --rewrite HEAD@{{0}}; done\ngit prune --expire=now\n"
        ),
    );
    let repo = GitRepository::open(&case.root).unwrap();
    let result = apply(&repo, &case);
    let created = fs::read_to_string(case.root.join(".git/child-created")).unwrap();
    assert_ne!(created.trim(), foreign);
    assert!(
        !output(
            &case.root,
            &["cat-file", "-e", &format!("{}^{{commit}}", created.trim())]
        )
        .status
        .success(),
        "fixture must prune the original child-created object"
    );
    assert_eq!(git(&case.root, &["rev-parse", "HEAD"]), *foreign);
    assert!(
        git(
            &case.root,
            &["log", "-g", "--format=%H|%gs", "refs/heads/main"]
        )
        .contains(&format!("{foreign}|commit (merge): prior identical merge"))
    );
    match result {
        Err(GitError::MergeConclusionUnconfirmed(evidence)) => {
            assert!(
                evidence.candidate.is_none(),
                "historical commit is not evidence: {evidence:?}"
            );
            assert_eq!(
                evidence.observed_head.as_ref().map(Oid::as_str),
                Some(foreign.as_str())
            );
            assert!(evidence.operation_consumed);
            assert!(
                evidence
                    .detail
                    .contains("no longer contains its pre-invocation entry")
            );
        }
        other => panic!("expected unconfirmed, never foreign success: {other:?}"),
    }
}

#[test]
fn started_failure_after_hook_amend_keeps_original_creation_identity() {
    let case = stopped(false);
    hook(
        &case,
        "post-commit",
        "#!/bin/sh\nset -eu\nif [ -n \"${SK_AMENDING:-}\" ]; then exit 0; fi\ngit rev-parse HEAD > .git/child-created\nSK_AMENDING=1 GIT_COMMITTER_DATE='2001-02-03T04:05:06 +0000' git -c commit.gpgsign=false commit --amend --no-edit --quiet\nsleep 2\n",
    );
    let options = GitOptions {
        program: PathBuf::from("git"),
        timeout: Duration::from_millis(1500),
    };
    let repo = GitRepository::open_with_options(&case.root, &options).unwrap();
    let result = apply(&repo, &case);
    let created = fs::read_to_string(case.root.join(".git/child-created")).unwrap();
    let amended = git(&case.root, &["rev-parse", "HEAD"]);
    assert_ne!(created.trim(), amended);
    match result {
        Err(GitError::MergeConclusionUnconfirmed(evidence)) => {
            assert_eq!(
                evidence.candidate.as_ref().map(Oid::as_str),
                Some(created.trim()),
                "amended commit is not this child's original creation"
            );
            assert_eq!(
                evidence.observed_head.as_ref().map(Oid::as_str),
                Some(amended.as_str())
            );
            assert!(evidence.operation_consumed);
        }
        other => panic!("expected started-failure uncertainty: {other:?}"),
    }
}
