//! Cross-language lock, append and sidecar interoperability.
//!
//! These tests run the pinned upstream Python implementation against the
//! native Rust primitives over the same files. They prove mixed writers share
//! one authority and that a crash releases it, rather than merely checking
//! each implementation in isolation.
//!
//! Development oracle only: the Rust binary never invokes Python at runtime.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use sk_kernel::event_log_append::{EventLogAppendOptions, append_event_log_lines};
use sk_kernel::lock_families::StatusLockHolder;
use sk_kernel::{FileLock, KernelError, StatusLock};

#[path = "support/oracle.rs"]
mod oracle_support;

fn python() -> Option<PathBuf> {
    // Missing or drifted oracles panic; None must never certify zero comparisons.
    Some(oracle_support::python())
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("workspace root")
}

fn python_script() -> &'static str {
    r#"
import json, sys, time
from pathlib import Path
from kernel.locks import LockNotAcquired, machine_file_lock

mode, target, ready = sys.argv[1], Path(sys.argv[2]), Path(sys.argv[3])
if mode == "hold-lock":
    with machine_file_lock(target, blocking=True, timeout_s=10):
        ready.write_text("ready")
        time.sleep(float(sys.argv[4]))
elif mode == "try-lock":
    try:
        with machine_file_lock(target):
            print("acquired")
    except LockNotAcquired:
        print("contended")
elif mode == "hold-event-log":
    from kernel.event_log_append import event_log_lock
    with event_log_lock(target, timeout_s=10):
        ready.write_text("ready")
        time.sleep(float(sys.argv[4]))
elif mode == "print-event-log-lock":
    from kernel.event_log_append import event_log_lock_path
    print(event_log_lock_path(target))
elif mode == "append-event-log":
    from kernel.event_log_append import append_event_log_lines
    for index in range(int(sys.argv[4])):
        append_event_log_lines(target, [f"py-{index}"], timeout_s=10)
elif mode == "read-sidecar":
    print(json.dumps(json.loads(Path(target).read_text(encoding="utf-8")), sort_keys=True))
elif mode == "write-sidecar":
    Path(target).write_text(
        json.dumps({"pid": 4242, "thread": "py-thread", "acquired_at": "2026-01-01T00:00:00+00:00"}, sort_keys=True),
        encoding="utf-8",
    )
else:
    raise SystemExit(f"unknown mode {mode}")
"#
}

fn run_python(mode: &str, target: &Path, ready: &Path, extra: &str) -> std::process::Output {
    Command::new(python().expect("pinned python"))
        .arg("-c")
        .arg(python_script())
        .arg(mode)
        .arg(target)
        .arg(ready)
        .arg(extra)
        .env("PYTHONPATH", repo_root().join("src"))
        .current_dir(repo_root())
        .output()
        .expect("run python")
}

fn spawn_python(mode: &str, target: &Path, ready: &Path, extra: &str) -> Child {
    Command::new(python().expect("pinned python"))
        .arg("-c")
        .arg(python_script())
        .arg(mode)
        .arg(target)
        .arg(ready)
        .arg(extra)
        .env("PYTHONPATH", repo_root().join("src"))
        .current_dir(repo_root())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn python")
}

fn wait_for(path: &Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("{} did not appear", path.display());
}

#[test]
fn a_python_holder_contends_a_rust_lock_and_releases_on_exit() {
    let Some(_) = python() else {
        eprintln!("pinned Python oracle is not installed; skipping cross-language lock test");
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    let lock = temp.path().join("shared.lock");
    let ready = temp.path().join("ready");
    let mut child = spawn_python("hold-lock", &lock, &ready, "1.2");
    wait_for(&ready, Duration::from_secs(10));

    let error = FileLock::acquire(&lock, Duration::from_millis(200)).unwrap_err();
    assert!(matches!(error, KernelError::LockTimeout { .. }), "{error}");

    child.wait().unwrap();
    let acquired = FileLock::acquire(&lock, Duration::from_secs(5)).expect("after holder exit");
    drop(acquired);
}

#[test]
fn a_rust_holder_contends_a_python_lock() {
    let Some(_) = python() else {
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    let lock = temp.path().join("shared.lock");
    let ready = temp.path().join("unused");
    let held = FileLock::acquire(&lock, Duration::from_secs(5)).unwrap();

    let output = run_python("try-lock", &lock, &ready, "");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "contended");

    drop(held);
    let output = run_python("try-lock", &lock, &ready, "");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "acquired");
}

#[test]
fn native_and_python_place_one_event_log_lock_in_the_same_git_common_dir() {
    let Some(_) = python() else {
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    let status = Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(&repo)
        .status()
        .unwrap();
    assert!(status.success());
    let log = repo.join("kitty-specs/mission/decisions.events.jsonl");
    fs::create_dir_all(log.parent().unwrap()).unwrap();
    fs::write(&log, b"").unwrap();

    let output = run_python(
        "print-event-log-lock",
        &log,
        &temp.path().join("unused"),
        "",
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let python_lock = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    let rust_lock = sk_kernel::event_log_append::event_log_lock_path(&log).unwrap();
    assert_eq!(rust_lock, python_lock);
    assert!(rust_lock.starts_with(repo.join(".git").canonicalize().unwrap()));

    // Mixed writers under the Git-discovered lock, not the sibling fallback.
    let child = spawn_python("append-event-log", &log, &temp.path().join("unused"), "5");
    for index in 0..5 {
        append_event_log_lines(
            &log,
            &[format!("rs-git-{index}")],
            &EventLogAppendOptions::default(),
        )
        .unwrap();
    }
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let text = fs::read_to_string(&log).unwrap();
    assert_eq!(text.lines().count(), 10);
    assert!(text.lines().all(|line| !line.is_empty()));
}

#[test]
fn mixed_python_and_rust_event_log_writers_produce_only_whole_ordered_lines() {
    let Some(_) = python() else {
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    let log = temp.path().join("decisions.events.jsonl");
    fs::write(&log, b"").unwrap();
    let ready = temp.path().join("unused");

    let child = spawn_python("append-event-log", &log, &ready, "25");
    for index in 0..25 {
        append_event_log_lines(
            &log,
            &[format!("rs-{index}")],
            &EventLogAppendOptions::default(),
        )
        .expect("rust append");
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "python writer failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let bytes = fs::read(&log).unwrap();
    assert_eq!(bytes.last(), Some(&b'\n'), "no torn final line");
    let lines: Vec<&str> = std::str::from_utf8(&bytes).unwrap().lines().collect();
    assert_eq!(lines.len(), 50);
    let mut rust_seen = Vec::new();
    let mut python_seen = Vec::new();
    for line in lines {
        assert!(
            line.starts_with("rs-") || line.starts_with("py-"),
            "interleaved or torn row: {line:?}"
        );
        if let Some(index) = line.strip_prefix("rs-") {
            rust_seen.push(index.parse::<usize>().unwrap());
        } else if let Some(index) = line.strip_prefix("py-") {
            python_seen.push(index.parse::<usize>().unwrap());
        }
    }
    assert_eq!(rust_seen, (0..25).collect::<Vec<_>>());
    assert_eq!(python_seen, (0..25).collect::<Vec<_>>());
}

#[cfg(unix)]
#[test]
fn a_killed_python_event_log_holder_releases_the_lock_for_rust() {
    let Some(_) = python() else {
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    let log = temp.path().join("decisions.events.jsonl");
    fs::write(&log, b"").unwrap();
    let ready = temp.path().join("ready");
    let mut child = spawn_python("hold-event-log", &log, &ready, "60");
    wait_for(&ready, Duration::from_secs(10));

    child.kill().unwrap();
    child.wait().unwrap();

    append_event_log_lines(&log, &["after-crash"], &EventLogAppendOptions::default())
        .expect("a killed holder's OS lock is released");
    assert_eq!(fs::read(&log).unwrap(), b"after-crash\n");
}

#[test]
fn status_holder_sidecar_is_readable_in_both_directions() {
    let Some(_) = python() else {
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    let common = temp.path().join(".git");
    fs::create_dir_all(&common).unwrap();
    let lock_path = sk_kernel::feature_status_lock_path(&common, "mission-01ABCDEF").unwrap();

    let guard = StatusLock::acquire_bounded(&lock_path, Duration::from_secs(2)).unwrap();
    let sidecar = StatusLockHolder::sidecar_path(&lock_path);
    let output = run_python("read-sidecar", &sidecar, &temp.path().join("unused"), "");
    assert!(output.status.success());
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(parsed["pid"].as_u64(), Some(u64::from(std::process::id())));
    assert!(parsed["thread"].is_string());
    assert!(parsed["acquired_at"].is_string());
    drop(guard);
    assert!(!sidecar.exists(), "outermost release clears the sidecar");

    run_python("write-sidecar", &sidecar, &temp.path().join("unused"), "");
    let holder = StatusLockHolder::read(&lock_path).expect("python-written sidecar");
    assert_eq!(holder.pid, 4242);
    assert_eq!(holder.thread, "py-thread");
    assert_eq!(holder.acquired_at, "2026-01-01T00:00:00+00:00");
    StatusLockHolder::clear(&lock_path).unwrap();
}
