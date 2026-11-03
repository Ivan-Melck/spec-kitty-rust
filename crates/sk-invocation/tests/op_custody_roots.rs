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

/// Register `checkout` as a linked worktree of `primary` by writing the same
/// two back-pointer files `git worktree add` writes, so a large listing costs
/// almost nothing to build. `git worktree list` reports the checkout as
/// `detached`, which is irrelevant to the placement question under test.
fn add_linked_worktree(primary: &Path, checkout: &Path, name: &str) {
    let admin = primary.join(".git").join("worktrees").join(name);
    fs::create_dir_all(&admin).expect("create worktree admin dir");
    fs::create_dir_all(checkout).expect("create linked checkout");
    fs::write(
        admin.join("gitdir"),
        format!("{}\n", checkout.join(".git").display()),
    )
    .expect("write admin gitdir");
    fs::write(
        checkout.join(".git"),
        format!("gitdir: {}\n", admin.display()),
    )
    .expect("write checkout gitdir pointer");
}

/// The porcelain listing length, used to prove the fixture is discriminating.
fn porcelain_len(primary: &Path) -> usize {
    let output = std::process::Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(primary)
        .output()
        .expect("run git worktree list");
    assert!(output.status.success());
    output.stdout.len()
}

fn canonical(path: &Path) -> std::path::PathBuf {
    path.canonicalize().expect("path canonicalizes")
}

#[cfg(unix)]
#[test]
fn custody_lookup_does_not_require_a_writable_temp_directory() {
    use std::os::unix::fs::PermissionsExt;
    const CHILD_ROOT: &str = "SK_INV_READONLY_TMP_ROOT";
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        let root = std::path::PathBuf::from(root);
        assert_eq!(
            locate_op_record_root(&root.join("primary"), ID, None),
            Some(canonical(&root.join("lane-0799")))
        );
        return;
    }
    let root = tempfile::Builder::new()
        .prefix("sk-readonly-custody-")
        .tempdir()
        .unwrap()
        .keep();
    eprintln!("retained fixture: {}", root.display());
    let primary = root.join("primary");
    init_repository(&primary);
    for index in 0..800 {
        let name = format!("lane-{index:04}");
        add_linked_worktree(&primary, &root.join(&name), &name);
    }
    hold_record(&root.join("lane-0799"), ID);
    assert!(porcelain_len(&primary) > 64 * 1024);
    let readonly = root.join("readonly-tmp");
    fs::create_dir_all(&readonly).unwrap();
    fs::set_permissions(&readonly, fs::Permissions::from_mode(0o500)).unwrap();
    if fs::write(readonly.join("permission-probe"), "x").is_ok() {
        fs::set_permissions(&readonly, fs::Permissions::from_mode(0o700)).unwrap();
        eprintln!("permission-bypass privileges: read-only TMPDIR case is not discriminating");
        return;
    }
    for directory in ["home", "cache", "config", "data", "state"] {
        fs::create_dir_all(root.join(directory)).unwrap();
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "custody_lookup_does_not_require_a_writable_temp_directory",
            "--nocapture",
        ])
        .current_dir(&primary)
        .env(CHILD_ROOT, &root)
        .env("TMPDIR", &readonly)
        .env("TMP", &readonly)
        .env("TEMP", &readonly)
        .env("HOME", root.join("home"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .output()
        .unwrap();
    fs::set_permissions(&readonly, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        output.status.success(),
        "child lookup failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
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
fn a_small_worktree_listing_finds_a_record_in_another_checkout() {
    // Small control: the parser path works well below the pipe capacity, so a
    // failure of the large case below is the deadlock, not the parser.
    let temp = tempfile::tempdir().unwrap();
    let primary = temp.path().join("primary");
    init_repository(&primary);
    let mut last = None;
    for index in 0..20 {
        let lane = temp.path().join(format!("lane-{index:02}"));
        add_linked_worktree(&primary, &lane, &format!("w{index:02}"));
        last = Some(lane);
    }
    let lane = last.expect("one linked checkout");
    hold_record(&lane, ID);
    assert!(porcelain_len(&primary) < 65536);

    assert_eq!(
        locate_op_record_root(&primary, ID, Some(&canonical(&primary))),
        Some(canonical(&lane))
    );
}

#[test]
fn a_large_worktree_listing_still_finds_a_record_in_another_checkout() {
    // F3 (red-first): a repository with hundreds of worktrees emits a
    // porcelain listing larger than the 64 KiB pipe buffer. Git must be
    // drained while it runs; a piped stdout that is only read after exit
    // deadlocks, hits the 30 s probe bound and silently loses cross-checkout
    // lookup (the record below is in the last listed worktree).
    let temp = tempfile::tempdir().unwrap();
    let primary = temp.path().join("primary");
    init_repository(&primary);
    let mut last = None;
    for index in 0..800 {
        let lane = temp.path().join(format!("lane-{index:04}"));
        add_linked_worktree(&primary, &lane, &format!("w{index:04}"));
        last = Some(lane);
    }
    let lane = last.expect("one linked checkout");
    hold_record(&lane, ID);
    assert!(
        porcelain_len(&primary) > 65536,
        "the fixture must exceed the pipe capacity"
    );

    assert_eq!(
        locate_op_record_root(&primary, ID, Some(&canonical(&primary))),
        Some(canonical(&lane))
    );
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
