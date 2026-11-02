//! Acceptance tests for the kernel's cross-process `FileLock` primitive.
//!
//! These tests are deliberately black-box: they exercise only the public
//! contract (`FileLock::acquire`, `FileLock::path`, holder diagnostics and the
//! anchored lock-path helper). Multi-process cases spawn this same test binary,
//! so the lock is proven cross-process rather than merely cross-thread.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use sk_kernel::{FileLock, KernelError, LockHolder, anchored_lock_path};
use tempfile::TempDir;

fn temp() -> TempDir {
    tempfile::tempdir().expect("temporary directory")
}

fn lock_path(temp: &TempDir, name: &str) -> PathBuf {
    temp.path().join(name)
}

fn wait_for_file(path: &Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("{} did not appear within {timeout:?}", path.display());
}

fn wait_for_exit(child: &mut Child, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if child.try_wait().expect("try_wait").is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("child did not exit within {timeout:?}");
}

/// Child-process helper: hold one lock and advertise readiness, then exit.
///
/// Runs only when the parent sets `SK_KERNEL_LOCK_CHILD`. The parent targets
/// this exact test name in this same test binary.
#[test]
fn lock_child_holds_for_parent() {
    let Some(path) = std::env::var_os("SK_KERNEL_LOCK_CHILD") else {
        return;
    };
    let path = PathBuf::from(path);
    let ready = PathBuf::from(std::env::var_os("SK_KERNEL_LOCK_READY").expect("ready path"));
    let hold = std::env::var("SK_KERNEL_LOCK_HOLD_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(1_000);
    let lock = FileLock::acquire(&path, Duration::from_secs(10)).expect("child acquires lock");
    fs::write(&ready, b"ready").expect("write readiness marker");
    std::thread::sleep(Duration::from_millis(hold));
    drop(lock);
}

#[test]
fn lock_child_acquires_and_exits_without_ready_marker() {
    let Some(path) = std::env::var_os("SK_KERNEL_KILL_CHILD") else {
        return;
    };
    let path = PathBuf::from(path);
    let ready = PathBuf::from(std::env::var_os("SK_KERNEL_KILL_READY").expect("ready path"));
    let _lock = FileLock::acquire(&path, Duration::from_secs(10)).expect("child acquires lock");
    fs::write(&ready, b"ready").expect("write readiness marker");
    std::thread::sleep(Duration::from_secs(60));
}

fn spawn_child(test_name: &str, lock: &Path, ready: &Path, env_key: &str) -> Child {
    Command::new(std::env::current_exe().expect("current exe"))
        .args(["--exact", test_name, "--nocapture"])
        .env(env_key, lock)
        .env("SK_KERNEL_LOCK_READY", ready)
        .env("SK_KERNEL_LOCK_HOLD_MS", "1200")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn child")
}

#[test]
fn lock_path_is_the_dedicated_file_and_release_never_unlinks_it() {
    let temp = temp();
    let path = lock_path(&temp, "dedicated.lock");
    let lock = FileLock::acquire(&path, Duration::from_secs(1)).expect("acquire");
    assert_eq!(lock.path(), path);
    assert!(path.is_file(), "the dedicated lock file exists while held");

    let holder = lock.holder();
    assert_eq!(holder.schema_version, 1);
    assert_eq!(holder.pid, std::process::id());
    assert!(!holder.started_at.is_empty());
    assert_eq!(holder.version, env!("CARGO_PKG_VERSION"));

    let observed = LockHolder::read(&path)
        .expect("read holder")
        .expect("holder record present while held");
    assert_eq!(observed.pid, std::process::id());

    drop(lock);
    assert!(
        path.is_file(),
        "release truncates but must never unlink the lock inode"
    );
    assert_eq!(fs::metadata(&path).unwrap().len(), 0);
    assert!(
        LockHolder::read(&path)
            .expect("read released holder")
            .is_none(),
        "release clears the holder record"
    );
}

#[test]
fn holder_diagnostics_distinguish_absent_malformed_and_held() {
    let temp = temp();
    let path = lock_path(&temp, "holder.lock");
    assert!(LockHolder::read(&path).unwrap().is_none());

    fs::write(&path, b"not json").unwrap();
    assert!(matches!(
        LockHolder::read(&path),
        Err(KernelError::LockHolderMalformed { .. })
    ));

    let lock = FileLock::acquire(&path, Duration::from_secs(1)).expect("acquire");
    assert_eq!(
        LockHolder::read(&path).unwrap().expect("holder").pid,
        std::process::id()
    );
    drop(lock);
}

#[test]
fn acquiring_a_nested_missing_lock_directory_creates_it_private() {
    let temp = temp();
    let path = temp.path().join("a/b/c.lock");
    let lock = FileLock::acquire(&path, Duration::from_secs(1)).expect("acquire");
    assert!(path.is_file());
    drop(lock);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(temp.path().join("a"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
}

#[test]
fn same_thread_reentry_reuses_the_held_lock() {
    let temp = temp();
    let path = lock_path(&temp, "reentrant.lock");
    let outer = FileLock::acquire(&path, Duration::from_secs(1)).expect("outer acquire");

    let inner = FileLock::acquire(&path, Duration::ZERO).expect("same-thread reentry is free");
    assert_eq!(inner.path(), path);
    assert_eq!(inner.holder().pid, std::process::id());
    drop(inner);

    let another = FileLock::acquire(&path, Duration::ZERO).expect("outer still holds");
    drop(another);
    drop(outer);
}

#[test]
fn reentrant_guards_count_depth_independently_of_drop_order() {
    let temp = temp();
    let path = lock_path(&temp, "depth.lock");
    let outer = FileLock::acquire(&path, Duration::from_secs(1)).expect("outer");
    let inner = FileLock::acquire(&path, Duration::ZERO).expect("inner");

    // Dropping the outer guard first must not strand the inner guard or make
    // the same thread contend against its own OS lock.
    drop(outer);
    let third = FileLock::acquire(&path, Duration::ZERO).expect("depth still held");
    drop(third);
    drop(inner);

    // All levels are gone; a fresh acquire works immediately.
    let fresh = FileLock::acquire(&path, Duration::ZERO).expect("released at zero depth");
    drop(fresh);
}

#[test]
fn another_thread_contends_and_times_out() {
    let temp = temp();
    let path = lock_path(&temp, "threads.lock");
    let held = FileLock::acquire(&path, Duration::from_secs(1)).expect("acquire");

    let contender = std::thread::spawn(move || {
        FileLock::acquire(&path, Duration::from_millis(150)).map(|lock| lock.path().to_path_buf())
    });
    let result = contender.join().expect("join contender");
    match result {
        Err(KernelError::LockTimeout { path: reported, .. }) => {
            assert!(reported.ends_with("threads.lock"));
        }
        other => panic!("expected LockTimeout, got {other:?}"),
    }

    drop(held);
}

#[test]
fn multi_process_holder_blocks_then_releases() {
    let temp = temp();
    let path = lock_path(&temp, "multi.lock");
    let ready = temp.path().join("ready");
    let mut child = spawn_child(
        "lock_child_holds_for_parent",
        &path,
        &ready,
        "SK_KERNEL_LOCK_CHILD",
    );
    wait_for_file(&ready, Duration::from_secs(10));

    let started = Instant::now();
    let error = FileLock::acquire(&path, Duration::from_millis(200)).expect_err("held elsewhere");
    assert!(matches!(error, KernelError::LockTimeout { .. }), "{error}");
    let waited = started.elapsed();
    assert!(waited >= Duration::from_millis(150), "waited {waited:?}");

    let acquired =
        FileLock::acquire(&path, Duration::from_secs(10)).expect("serializes after exit");
    assert_eq!(acquired.path(), path);
    drop(acquired);
    wait_for_exit(&mut child, Duration::from_secs(10));
}

#[cfg(unix)]
#[test]
fn killed_holder_releases_the_os_lock_without_unlinking() {
    let temp = temp();
    let path = lock_path(&temp, "killed.lock");
    let ready = temp.path().join("kill-ready");
    let mut child = Command::new(std::env::current_exe().expect("current exe"))
        .args([
            "--exact",
            "lock_child_acquires_and_exits_without_ready_marker",
            "--nocapture",
        ])
        .env("SK_KERNEL_KILL_CHILD", &path)
        .env("SK_KERNEL_KILL_READY", &ready)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn child");
    wait_for_file(&ready, Duration::from_secs(10));

    child.kill().expect("kill child");
    child.wait().expect("reap child");

    let acquired = FileLock::acquire(&path, Duration::from_secs(5))
        .expect("OS releases a dead holder's lock even though the file remains");
    drop(acquired);
    assert!(path.is_file(), "no unlink-on-release fallback was used");
}

#[cfg(unix)]
#[test]
fn planted_symlink_lock_is_refused_and_target_is_untouched() {
    use std::os::unix::fs::symlink;

    let temp = temp();
    let target = temp.path().join("victim");
    fs::write(&target, b"must not be truncated").unwrap();
    let path = temp.path().join("lock");
    symlink(&target, &path).unwrap();

    let error = FileLock::acquire(&path, Duration::from_millis(100)).unwrap_err();
    assert!(
        matches!(error, KernelError::NoFollow { .. } | KernelError::Io { .. }),
        "{error}"
    );
    assert_eq!(fs::read(&target).unwrap(), b"must not be truncated");
    assert!(path.symlink_metadata().unwrap().file_type().is_symlink());
}

#[test]
fn anchored_lock_path_is_stable_across_spellings() {
    let temp = temp();
    let root = temp.path().join("repo");
    let common = root.join(".git");
    let log = root.join("kitty-specs/mission/status.events.jsonl");
    fs::create_dir_all(log.parent().unwrap()).unwrap();
    fs::create_dir_all(&common).unwrap();
    fs::write(&log, b"").unwrap();

    let direct = anchored_lock_path(&log, &common, Some(Path::new("event-logs"))).expect("direct");
    let normalized = anchored_lock_path(
        &root.join("./kitty-specs/mission/status.events.jsonl"),
        &common,
        Some(Path::new("event-logs")),
    )
    .expect("normalized");
    assert_eq!(direct, normalized);
    assert_eq!(
        direct.parent().unwrap(),
        common.join("spec-kitty-locks/event-logs")
    );
    let name = direct.file_name().unwrap().to_string_lossy();
    let digest = name.strip_suffix(".lock").unwrap();
    assert_eq!(digest.len(), 32);
    assert!(
        digest
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    );
}

#[cfg(unix)]
#[test]
fn anchored_lock_path_distinguishes_non_utf8_anchors() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let temp = temp();
    let common = temp.path().join(".git");
    fs::create_dir_all(&common).unwrap();
    let first = temp.path().join(OsString::from_vec(vec![b'a', 0xff]));
    let second = temp.path().join(OsString::from_vec(vec![b'a', 0xfe]));
    fs::write(&first, b"").unwrap();
    fs::write(&second, b"").unwrap();

    let first_lock = anchored_lock_path(&first, &common, None).unwrap();
    let second_lock = anchored_lock_path(&second, &common, None).unwrap();
    assert_ne!(first_lock, second_lock);
    assert_eq!(
        first_lock,
        anchored_lock_path(&first, &common, None).unwrap()
    );
}

#[cfg(unix)]
#[test]
fn anchored_lock_path_resolves_a_symlinked_anchor_once() {
    use std::os::unix::fs::symlink;

    let temp = temp();
    let real = temp.path().join("real");
    fs::create_dir_all(&real).unwrap();
    let log = real.join("log.jsonl");
    fs::write(&log, b"").unwrap();
    let alias = temp.path().join("alias");
    symlink(&real, &alias).unwrap();
    let common = temp.path().join(".git");
    fs::create_dir_all(&common).unwrap();

    let first = anchored_lock_path(&log, &common, None).unwrap();
    let second = anchored_lock_path(&alias.join("log.jsonl"), &common, None).unwrap();
    assert_eq!(first, second);
}
