//! Coordination receipts report the held authority, not a guessed directory key.

use sk_kernel::locks::LockWait;
use sk_workspace::coord::MissionHold;
use std::{fs, process::Command, time::Duration};

#[test]
fn mission_hold_receipt_reports_the_recorded_legacy_key_and_actual_path() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let output = Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(output.status.success());
    let mission = root.join("kitty-specs/042-legacy-moved");
    fs::create_dir_all(&mission).unwrap();
    fs::write(
        mission.join("meta.json"),
        r#"{"mission_slug":"042-legacy","coordination_branch":"kitty/mission-042-legacy"}"#,
    )
    .unwrap();
    let hold =
        MissionHold::acquire(&mission, root, LockWait::Bounded(Duration::from_secs(1))).unwrap();
    assert_eq!(hold.key(), "042-legacy");
    let path = hold.path().expect("the real guard always reports its path");
    assert_eq!(path.file_name().unwrap(), "042-legacy.status.lock");
    assert!(path.is_file());
    assert!(hold.holder().is_some());
    assert!(
        !root
            .join(".git/spec-kitty-locks/042-legacy-moved.status.lock")
            .exists()
    );
}
