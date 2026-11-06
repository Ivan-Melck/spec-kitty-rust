//! Acceptance tests for explicit per-family lock policies and the status
//! sidecar/ordering contracts.

use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use sk_kernel::lock_families::{
    StatusLockHolder, acquire_checkout_claim_lock, feature_status_lock_path,
};
use sk_kernel::locks::LockWait;
use sk_kernel::{FileLock, KernelError, LockOptions, StatusLock};

fn temp() -> tempfile::TempDir {
    tempfile::tempdir().expect("temporary directory")
}

fn hold_in_thread(path: &Path) -> (mpsc::Receiver<()>, mpsc::Sender<()>, thread::JoinHandle<()>) {
    let (ready_tx, ready_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let path = path.to_path_buf();
    let handle = thread::spawn(move || {
        let _lock = FileLock::acquire(&path, Duration::from_secs(5)).unwrap();
        ready_tx.send(()).unwrap();
        let _ = release_rx.recv_timeout(Duration::from_secs(10));
    });
    (ready_rx, release_tx, handle)
}

#[test]
fn nonblocking_acquire_reports_not_acquired_without_waiting() {
    let temp = temp();
    let path = temp.path().join("family.lock");
    let (ready, release, handle) = hold_in_thread(&path);
    ready.recv_timeout(Duration::from_secs(5)).unwrap();

    let started = Instant::now();
    let error = FileLock::acquire_with(&path, LockOptions::nonblocking()).unwrap_err();
    assert!(
        matches!(error, KernelError::LockNotAcquired { .. }),
        "{error}"
    );
    assert!(started.elapsed() < Duration::from_millis(500));
    assert!(FileLock::try_acquire(&path).unwrap().is_none());

    release.send(()).unwrap();
    handle.join().unwrap();
    assert!(FileLock::try_acquire(&path).unwrap().is_some());
}

#[test]
fn nonreentrant_same_thread_acquire_contends_instead_of_reusing_the_guard() {
    let temp = temp();
    let path = temp.path().join("family.lock");
    let first = FileLock::acquire_with(&path, LockOptions::nonblocking()).unwrap();
    let error =
        FileLock::acquire_with(&path, LockOptions::nonblocking().non_reentrant()).unwrap_err();
    assert!(
        matches!(error, KernelError::LockNotAcquired { .. }),
        "{error}"
    );
    drop(first);
}

#[test]
fn bounded_zero_is_one_attempt_and_reports_a_timeout() {
    let temp = temp();
    let path = temp.path().join("family.lock");
    let (ready, release, handle) = hold_in_thread(&path);
    ready.recv_timeout(Duration::from_secs(5)).unwrap();

    let error = FileLock::acquire(&path, Duration::ZERO).unwrap_err();
    assert!(matches!(error, KernelError::LockTimeout { .. }), "{error}");
    release.send(()).unwrap();
    handle.join().unwrap();
}

#[test]
fn unbounded_wait_across_threads_acquires_after_release() {
    let temp = temp();
    let path = temp.path().join("family.lock");
    let (ready, release, handle) = hold_in_thread(&path);
    ready.recv_timeout(Duration::from_secs(5)).unwrap();

    thread::spawn(move || {
        thread::sleep(Duration::from_millis(150));
        let _ = release.send(());
    });
    let acquired = FileLock::acquire_unbounded(&path).expect("waits until release");
    drop(acquired);
    handle.join().unwrap();
}

#[cfg(unix)]
#[test]
fn aliased_nested_status_locks_keep_the_outer_holders_sidecar() {
    let temp = temp();
    let common = temp.path().join(".git");
    std::fs::create_dir_all(&common).unwrap();
    let alias = temp.path().join("git-alias");
    std::os::unix::fs::symlink(&common, &alias).unwrap();
    let path = feature_status_lock_path(&common, "mission-01ABCDEF").unwrap();
    let aliased = feature_status_lock_path(&alias, "mission-01ABCDEF").unwrap();
    let outer = StatusLock::acquire_bounded(&path, Duration::from_secs(2)).unwrap();
    let recorded = StatusLockHolder::read(&path).unwrap();
    {
        let _inner = StatusLock::acquire_bounded(&aliased, Duration::from_secs(2)).unwrap();
        assert_eq!(StatusLockHolder::read(&aliased), Some(recorded.clone()));
    }
    assert_eq!(StatusLockHolder::read(&path), Some(recorded));
    assert!(matches!(
        FileLock::acquire_with(&path, LockOptions::nonblocking().non_reentrant()),
        Err(KernelError::LockNotAcquired { .. })
    ));
    drop(outer);
    assert!(StatusLockHolder::read(&path).is_none());
    assert!(FileLock::try_acquire(&path).unwrap().is_some());
}

#[test]
fn status_sidecar_is_written_once_and_cleared_by_the_outermost_guard() {
    let temp = temp();
    let common = temp.path().join(".git");
    std::fs::create_dir_all(&common).unwrap();
    let path = feature_status_lock_path(&common, "mission-01ABCDEF").unwrap();
    let sidecar = StatusLockHolder::sidecar_path(&path);

    let outer = StatusLock::acquire_bounded(&path, Duration::from_secs(2)).unwrap();
    assert!(sidecar.is_file());
    let recorded = StatusLockHolder::read(&path).expect("sidecar");
    assert_eq!(recorded.pid, std::process::id());
    {
        let inner = StatusLock::acquire_bounded(&path, Duration::from_secs(2)).unwrap();
        assert!(
            sidecar.is_file(),
            "nested acquire must not clear the sidecar"
        );
        drop(inner);
        assert!(
            sidecar.is_file(),
            "nested release must not clear an enclosing holder"
        );
    }
    drop(outer);
    assert!(!sidecar.exists());
}

#[test]
fn status_timeout_names_the_sidecar_holder() {
    let temp = temp();
    let common = temp.path().join(".git");
    std::fs::create_dir_all(&common).unwrap();
    let path = feature_status_lock_path(&common, "mission-01ABCDEF").unwrap();
    let holder_path = path.clone();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let handle = thread::Builder::new()
        .name("status-holder".to_owned())
        .spawn(move || {
            let _held = StatusLock::acquire_bounded(&holder_path, Duration::from_secs(5)).unwrap();
            ready_tx.send(()).unwrap();
            let _ = release_rx.recv_timeout(Duration::from_secs(10));
        })
        .unwrap();
    ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();

    let error = StatusLock::acquire_bounded(&path, Duration::from_millis(150)).unwrap_err();
    match error {
        KernelError::StatusLockTimeout {
            path: named,
            holder,
            ..
        } => {
            assert_eq!(named, path);
            assert!(holder.contains("status-holder"), "{holder}");
            let pid = std::process::id().to_string();
            assert!(holder.contains(&pid), "{holder}");
        }
        other => panic!("expected StatusLockTimeout, got {other:?}"),
    }
    release_tx.send(()).unwrap();
    handle.join().unwrap();
}

#[test]
fn checkout_claim_must_be_taken_before_a_mission_lock() {
    let temp = temp();
    let common = temp.path().join(".git");
    std::fs::create_dir_all(&common).unwrap();
    let mission_path = feature_status_lock_path(&common, "mission-01ABCDEF").unwrap();
    let _mission = StatusLock::acquire_bounded(&mission_path, Duration::from_secs(2)).unwrap();

    let error = acquire_checkout_claim_lock(
        temp.path(),
        &common,
        LockWait::Bounded(Duration::from_millis(150)),
    )
    .unwrap_err();
    match error {
        KernelError::LockOrderViolation { held } => {
            assert!(held.contains("mission-01ABCDEF"), "{held}");
        }
        other => panic!("expected LockOrderViolation, got {other:?}"),
    }
    drop(_mission);

    let checkout = acquire_checkout_claim_lock(
        temp.path(),
        &common,
        LockWait::Bounded(Duration::from_secs(2)),
    )
    .unwrap();
    let mission = StatusLock::acquire_bounded(&mission_path, Duration::from_secs(2)).unwrap();
    drop(mission);
    drop(checkout);
}

#[test]
fn checkout_claim_is_reentrant_on_one_thread() {
    let temp = temp();
    let common = temp.path().join(".git");
    std::fs::create_dir_all(&common).unwrap();
    let outer = acquire_checkout_claim_lock(
        temp.path(),
        &common,
        LockWait::Bounded(Duration::from_secs(2)),
    )
    .unwrap();
    let inner = acquire_checkout_claim_lock(
        temp.path(),
        &common,
        LockWait::Bounded(Duration::from_secs(2)),
    )
    .unwrap();
    assert_eq!(outer.path(), inner.path());
    drop(inner);
    drop(outer);
}
