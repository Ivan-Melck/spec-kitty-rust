//! Lock mapping and commit-recovery contracts.

use std::io::{BufRead, Read, Write};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use sk_install::ProjectLock;

/// Kill only the holder we spawned, including when a parent assertion panics.
struct LockHolder(Child);

impl Drop for LockHolder {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "subprocess helper exercised by the contention test"]
fn project_lock_holder() {
    let path = std::env::var_os("SK_INSTALL_TEST_LOCK_PATH").expect("lock path");
    let _lock = ProjectLock::acquire(std::path::Path::new(&path), Duration::from_secs(2))
        .expect("child acquires project lock");
    println!("LOCK_READY");
    std::io::stdout().flush().unwrap();
    // The parent's stdin pipe keeps this process alive until explicit cleanup.
    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input).unwrap();
}

#[test]
fn a_contended_project_lock_maps_to_lock_unavailable() {
    let directory = tempfile::tempdir().expect("temp");
    let lock_path = directory.path().join(".kittify/.install.lock");
    std::fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
    std::fs::write(&lock_path, "").unwrap();

    // A native helper owns exactly one process and one lock descriptor. Never
    // signal a process group: a test must not terminate its agent or harness.
    let child = Command::new(std::env::current_exe().expect("test executable"))
        .args(["--ignored", "--exact", "project_lock_holder", "--nocapture"])
        .env("SK_INSTALL_TEST_LOCK_PATH", &lock_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn project lock holder");
    let mut holder = LockHolder(child);
    let stdout = holder.0.stdout.take().expect("holder stdout");
    let ready = std::io::BufReader::new(stdout)
        .lines()
        .any(|line| line.expect("read holder output").trim() == "LOCK_READY");
    assert!(ready, "holder exited before confirming lock acquisition");

    let error =
        ProjectLock::acquire(&lock_path, Duration::from_millis(100)).expect_err("contended lock");
    assert_eq!(error.code(), "lock_unavailable");
    drop(holder);
    let _lock = ProjectLock::acquire(&lock_path, Duration::from_secs(2))
        .expect("acquire after holder process is reaped");
}
