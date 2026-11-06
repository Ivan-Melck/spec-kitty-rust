//! Lock-authority tests: family paths, keys, holder sidecars, reentrancy,
//! bounded contention and worktree canonicalisation.

mod mutation_support;

use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

use sk_kernel::KernelError;
use sk_kernel::locks::LockWait;
use sk_status::mutation::{
    MutationError, acquire_mission_lock, canonical_repo_root, canonicalize_feature_dir,
    mission_lock_key, mission_lock_path, resolve_lock_root,
};

use mutation_support::{MISSION_ID, MISSION_SLUG, temp_mission};

#[test]
fn an_ad_hoc_directory_locks_under_the_degraded_common_dir() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("ad-hoc-status");
    fs::create_dir_all(&directory).unwrap();
    let lock = acquire_mission_lock(&directory, None, LockWait::Bounded(Duration::from_secs(5)))
        .expect("lock");
    let expected = directory
        .join(".spec-kitty/spec-kitty-locks")
        .join("ad-hoc-status.status.lock");
    assert_eq!(lock.path(), expected);
    // The `.holder` sidecar exists while held and is cleared on release.
    let sidecar = expected.with_file_name("ad-hoc-status.status.lock.holder");
    assert!(sidecar.is_file());
    drop(lock);
    assert!(!sidecar.exists());
}

#[test]
fn a_mission_directory_is_keyed_on_its_directory_name() {
    let mission = temp_mission();
    let key = mission_lock_key(&mission.feature_dir, &mission.repo_root).expect("key");
    assert_eq!(key, MISSION_SLUG);
    let path = mission_lock_path(&mission.feature_dir, Some(&mission.repo_root)).expect("path");
    assert!(path.ends_with(format!(
        ".spec-kitty/spec-kitty-locks/{MISSION_SLUG}.status.lock"
    )));
}

#[test]
fn a_coordination_routed_mission_keys_on_the_coordination_directory() {
    let temp = tempfile::tempdir().unwrap();
    let repo_root = temp.path().to_path_buf();
    let feature_dir = repo_root.join("kitty-specs/060-demo");
    fs::create_dir_all(&feature_dir).unwrap();
    fs::write(
        feature_dir.join("meta.json"),
        serde_json::json!({
            "mission_slug": "060-demo",
            "mission_id": MISSION_ID,
            "coordination_branch": "kitty/mission-060-demo-01K9M0R3",
        })
        .to_string(),
    )
    .unwrap();
    let key = mission_lock_key(&feature_dir, &repo_root).expect("key");
    assert_eq!(key, "060-demo-01K9M0R3");
}

#[test]
fn a_legacy_numbered_coordination_mission_falls_back_to_its_directory_name() {
    let temp = tempfile::tempdir().unwrap();
    let repo_root = temp.path().to_path_buf();
    let feature_dir = repo_root.join("kitty-specs/060-legacy-demo");
    fs::create_dir_all(&feature_dir).unwrap();
    fs::write(
        feature_dir.join("meta.json"),
        serde_json::json!({
            "mission_slug": "060-legacy-demo",
            "coordination_branch": "kitty/mission-060-legacy-demo",
        })
        .to_string(),
    )
    .unwrap();
    assert_eq!(
        mission_lock_key(&feature_dir, &repo_root).expect("legacy carve-out"),
        "060-legacy-demo"
    );
}

#[test]
fn a_modern_coordination_mission_with_no_mid8_is_refused() {
    let temp = tempfile::tempdir().unwrap();
    let repo_root = temp.path().to_path_buf();
    let feature_dir = repo_root.join("kitty-specs/modern-demo");
    fs::create_dir_all(&feature_dir).unwrap();
    fs::write(
        feature_dir.join("meta.json"),
        serde_json::json!({
            "mission_slug": "modern-demo",
            "coordination_branch": "kitty/mission-modern-demo",
        })
        .to_string(),
    )
    .unwrap();
    let error = mission_lock_key(&feature_dir, &repo_root)
        .expect_err("an unresolvable coordination key must fail closed");
    assert!(matches!(error, MutationError::LockKeyUnresolved { .. }));
}

#[test]
fn same_thread_reentrancy_keeps_one_lock_and_one_holder_record() {
    let mission = temp_mission();
    let first = acquire_mission_lock(
        &mission.feature_dir,
        Some(&mission.repo_root),
        LockWait::Bounded(Duration::from_secs(5)),
    )
    .expect("first");
    let sidecar = first.path().with_file_name(format!(
        "{}.holder",
        first.path().file_name().unwrap().to_string_lossy()
    ));
    assert!(sidecar.is_file());
    let second = acquire_mission_lock(
        &mission.feature_dir,
        Some(&mission.repo_root),
        LockWait::Bounded(Duration::from_secs(5)),
    )
    .expect("reentrant");
    assert_eq!(first.path(), second.path());
    drop(first);
    assert!(
        sidecar.is_file(),
        "dropping an inner guard must not clear the outermost holder record"
    );
    drop(second);
    assert!(!sidecar.exists());
}

#[test]
fn a_contended_bounded_acquire_reports_the_lock_timeout() {
    let mission = temp_mission();
    let feature_dir = mission.feature_dir.clone();
    let repo_root = mission.repo_root.clone();
    let (holding_tx, holding_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();

    let holder = std::thread::spawn(move || {
        let _lock = acquire_mission_lock(
            &feature_dir,
            Some(&repo_root),
            LockWait::Bounded(Duration::from_secs(5)),
        )
        .expect("holder lock");
        holding_tx.send(()).unwrap();
        let _ = release_rx.recv();
    });
    holding_rx.recv_timeout(Duration::from_secs(5)).unwrap();

    let error = acquire_mission_lock(
        &mission.feature_dir,
        Some(&mission.repo_root),
        LockWait::Bounded(Duration::from_millis(100)),
    )
    .expect_err("a held lock must time out");
    match error {
        MutationError::Kernel(KernelError::StatusLockTimeout { holder, .. }) => {
            assert!(holder.contains("held by pid"));
        }
        other => panic!("unexpected error: {other}"),
    }
    release_tx.send(()).unwrap();
    holder.join().unwrap();
}

#[test]
fn the_lock_root_mirrors_the_baseline_resolution_rules() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_path_buf();
    assert_eq!(
        resolve_lock_root(Path::new("/x/kitty-specs/m"), None),
        Path::new("/x")
    );
    assert_eq!(
        resolve_lock_root(Path::new("/x/kitty-specs/m"), Some(&root)),
        root
    );
    assert_eq!(
        resolve_lock_root(Path::new("/x/ad-hoc"), None),
        Path::new("/x/ad-hoc")
    );
}

#[test]
fn a_linked_worktree_is_canonicalised_to_the_main_checkout() {
    let temp = tempfile::tempdir().unwrap();
    let main = temp.path().join("main");
    fs::create_dir_all(main.join("kitty-specs").join(MISSION_SLUG)).unwrap();
    fs::write(
        main.join("kitty-specs")
            .join(MISSION_SLUG)
            .join("meta.json"),
        serde_json::json!({"mission_slug": MISSION_SLUG}).to_string(),
    )
    .unwrap();
    git(&main, &["init", "-q"]);
    git(&main, &["add", "."]);
    git_commit(&main, "seed");

    let worktree = temp.path().join("lane-wt");
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            worktree.to_str().unwrap(),
            "-b",
            "lane",
        ],
    );

    let worktree_feature = worktree.join("kitty-specs").join(MISSION_SLUG);
    assert_eq!(
        canonicalize_feature_dir(&worktree_feature),
        main.join("kitty-specs").join(MISSION_SLUG)
    );
    assert_eq!(canonical_repo_root(&worktree_feature).unwrap(), main);
    let lock_path = mission_lock_path(&worktree_feature, None).unwrap();
    assert_eq!(
        lock_path,
        main.join(".git/spec-kitty-locks")
            .join(format!("{MISSION_SLUG}.status.lock"))
    );
}

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_commit(repo: &Path, message: &str) {
    let output = Command::new("git")
        .args([
            "-c",
            "user.email=mutation@example.invalid",
            "-c",
            "user.name=Mutation Fixture",
            "commit",
            "-q",
            "-m",
            message,
            "--allow-empty",
        ])
        .current_dir(repo)
        .output()
        .expect("run git commit");
    assert!(
        output.status.success(),
        "git commit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
