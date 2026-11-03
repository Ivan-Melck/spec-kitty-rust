//! Op-record placement over real Git repositories and linked worktrees.
//!
//! These tests run the real `git` binary in isolated temporary repositories
//! (scratch HOME, no system/global config). They exercise the same topology
//! questions the reference asks through `kernel.git_topology` and
//! `git worktree list`: which checkout owns an Op record, and which checkout
//! already holds one.

mod support;

use std::fs;
use std::path::Path;

use sk_invocation::{
    InvocationId, OpWriteAction, OpWriteRefused, is_worktree_of, locate_op_record_root,
    resolve_op_record_root,
};
use support::{git, init_repository};

const ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const OTHER_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";

fn hold_record(checkout: &Path, invocation_id: &str) {
    let directory = checkout.join("kitty-ops");
    fs::create_dir_all(&directory).expect("create kitty-ops");
    fs::write(directory.join(format!("{invocation_id}.jsonl")), "{}\n").expect("write record");
}

fn canonical(path: &Path) -> std::path::PathBuf {
    path.canonicalize().expect("path canonicalizes")
}

#[test]
fn a_linked_worktree_owns_its_op_records() {
    let temp = tempfile::tempdir().unwrap();
    let primary = temp.path().join("primary");
    init_repository(&primary);
    let lane = temp.path().join("lane");
    git(
        &primary,
        &["worktree", "add", "-b", "topic", lane.to_str().unwrap()],
    );
    let subdirectory = lane.join("sub");
    fs::create_dir_all(&subdirectory).unwrap();

    assert_eq!(
        resolve_op_record_root(&primary, &subdirectory),
        canonical(&lane)
    );
    assert!(is_worktree_of(&primary, &canonical(&lane)));
}

#[test]
fn the_repository_root_checkout_owns_its_op_records() {
    let temp = tempfile::tempdir().unwrap();
    let primary = temp.path().join("primary");
    init_repository(&primary);

    assert_eq!(
        resolve_op_record_root(&primary, &primary),
        canonical(&primary)
    );
}

#[test]
fn a_checkout_of_another_repository_falls_back_to_the_project() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let other = temp.path().join("other");
    init_repository(&project);
    init_repository(&other);

    assert_eq!(resolve_op_record_root(&project, &other), project);
    assert!(!is_worktree_of(&project, &other));
}

#[test]
fn a_directory_outside_any_checkout_falls_back_to_the_project() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    init_repository(&project);
    let outside = temp.path().join("outside");
    fs::create_dir_all(&outside).unwrap();

    assert_eq!(resolve_op_record_root(&project, &outside), project);
}

#[test]
fn a_relative_worktree_gitdir_pointer_still_resolves_to_the_worktree() {
    let temp = tempfile::tempdir().unwrap();
    let primary = temp.path().join("primary");
    init_repository(&primary);
    let lane = temp.path().join("lane");
    git(
        &primary,
        &["worktree", "add", "-b", "topic", lane.to_str().unwrap()],
    );
    // git 2.48 can write a worktree's `gitdir:` pointer relative; a
    // subdirectory must still resolve to the worktree.
    fs::write(
        lane.join(".git"),
        "gitdir: ../primary/.git/worktrees/lane\n",
    )
    .unwrap();
    let deep = lane.join("deep").join("sub");
    fs::create_dir_all(&deep).unwrap();

    assert_eq!(resolve_op_record_root(&primary, &deep), canonical(&lane));
}

#[test]
fn locate_prefers_the_invoking_checkout() {
    let temp = tempfile::tempdir().unwrap();
    let primary = temp.path().join("primary");
    init_repository(&primary);
    let lane = temp.path().join("lane");
    git(
        &primary,
        &["worktree", "add", "-b", "topic", lane.to_str().unwrap()],
    );
    hold_record(&primary, ID);
    hold_record(&lane, ID);

    assert_eq!(
        locate_op_record_root(&primary, ID, Some(&canonical(&lane))),
        Some(canonical(&lane))
    );
}

#[test]
fn locate_finds_a_record_held_by_another_worktree() {
    let temp = tempfile::tempdir().unwrap();
    let primary = temp.path().join("primary");
    init_repository(&primary);
    let lane = temp.path().join("lane");
    git(
        &primary,
        &["worktree", "add", "-b", "topic", lane.to_str().unwrap()],
    );
    hold_record(&lane, ID);

    assert_eq!(
        locate_op_record_root(&primary, ID, Some(&canonical(&primary))),
        Some(canonical(&lane))
    );
}

#[test]
fn locate_finds_an_older_record_in_the_root_checkout() {
    let temp = tempfile::tempdir().unwrap();
    let primary = temp.path().join("primary");
    init_repository(&primary);
    let lane = temp.path().join("lane");
    git(
        &primary,
        &["worktree", "add", "-b", "topic", lane.to_str().unwrap()],
    );
    hold_record(&primary, ID);

    assert_eq!(
        locate_op_record_root(&primary, ID, Some(&canonical(&lane))),
        Some(canonical(&primary))
    );
}

#[test]
fn locate_returns_none_when_no_checkout_holds_the_record() {
    let temp = tempfile::tempdir().unwrap();
    let primary = temp.path().join("primary");
    init_repository(&primary);

    assert_eq!(locate_op_record_root(&primary, ID, None), None);
}

#[test]
fn locate_never_finds_an_invalid_id() {
    let temp = tempfile::tempdir().unwrap();
    let primary = temp.path().join("primary");
    init_repository(&primary);
    fs::create_dir_all(primary.join("kitty-ops")).unwrap();
    fs::write(primary.join("kitty-ops/outside.jsonl"), "{}\n").unwrap();

    for invocation_id in ["../../etc/passwd", "nested/path", ".", "", "lowercase"] {
        assert_eq!(locate_op_record_root(&primary, invocation_id, None), None);
    }
}

#[test]
fn locate_still_checks_the_known_checkouts_when_git_cannot_run() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("missing");
    hold_record(temp.path(), ID);

    assert_eq!(
        locate_op_record_root(&missing, ID, Some(temp.path())),
        Some(temp.path().to_path_buf())
    );
    assert_eq!(
        locate_op_record_root(&missing, OTHER_ID, Some(temp.path())),
        None
    );
}

#[test]
fn refused_errors_name_the_record_root_and_action() {
    let refusal = sk_invocation::OpWriteRefusal::protected_branch("main");
    let error = OpWriteRefused::new(refusal, Path::new("/lane"), OpWriteAction::Open);
    let message = error.to_string();
    assert!(message.contains("/lane"), "{message}");
    assert!(message.contains("protected branch 'main'"), "{message}");
    assert!(message.contains("Nothing was written."), "{message}");
    assert!(!message.to_lowercase().contains("override"), "{message}");

    let record_root = Path::new("/repo");
    assert_eq!(
        sk_invocation::OpWriteRefusal::protected_branch("main")
            .message(record_root, OpWriteAction::Close),
        "Refusing to close the Op: its Op record would be written in /repo on protected branch \
         'main', where the record cannot be committed. Nothing was written. Run the command from \
         a topic branch, or from a worktree whose branch is not protected."
    );
    assert_eq!(
        InvocationId::parse(ID).unwrap().to_string(),
        ID,
        "the fixture id must stay valid"
    );
}
