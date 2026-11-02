//! Lock mapping and commit-recovery contracts.

#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::Duration;

use sk_install::ProjectLock;

#[test]
fn a_contended_project_lock_maps_to_lock_unavailable() {
    let directory = tempfile::tempdir().expect("temp");
    let lock_path = directory.path().join(".kittify/.install.lock");
    std::fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
    std::fs::write(&lock_path, "").unwrap();

    let mut command = Command::new("flock");
    command
        .arg(&lock_path)
        .args(["sleep", "2"])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // A process group lets the cleanup kill `flock` and the `sleep` it execs as
    // one, so the inherited lock descriptor is actually released.
    #[cfg(unix)]
    command.process_group(0);
    let mut holder = command.spawn().expect("spawn flock holder");
    // Give flock(1) a moment to take the lock.
    std::thread::sleep(Duration::from_millis(150));
    let error =
        ProjectLock::acquire(&lock_path, Duration::from_millis(100)).expect_err("contended lock");
    assert_eq!(error.code(), "lock_unavailable");
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .args(["-TERM", &format!("-{}", holder.id())])
            .status();
    }
    let _ = holder.kill();
    let _ = holder.wait();
    // Once released, the same path acquires normally (bounded retry so a
    // scheduler delay does not make this flaky).
    let mut acquired = None;
    for _ in 0..50 {
        match ProjectLock::acquire(&lock_path, Duration::from_millis(100)) {
            Ok(lock) => {
                acquired = Some(lock);
                break;
            }
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
    acquired.expect("acquire after release");
}
