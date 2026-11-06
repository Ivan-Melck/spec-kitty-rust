#![cfg(unix)]

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use sk_kernel::{KernelError, atomic_write, discover_project_root, read_bounded};

fn fifo(path: &Path) {
    assert!(Command::new("mkfifo").arg(path).status().unwrap().success());
}

#[test]
fn a_fifo_without_a_writer_is_rejected_without_blocking() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("untrusted-input");
    fifo(&path);
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(read_bounded(&path, 1024));
    });
    let result = receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("bounded reads must not block opening a FIFO with no writer");
    assert!(matches!(result, Err(KernelError::Io { .. })));
}

#[test]
fn atomic_write_refuses_a_fifo_without_blocking() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("output");
    fifo(&path);
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(atomic_write(&path, b"new"));
    });
    let result = receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("atomic equality checks must not block opening a FIFO");
    assert!(matches!(result, Err(KernelError::Io { .. })));
}

#[test]
fn atomic_write_refuses_a_symlink_to_a_device_without_reading_it() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("output");
    std::os::unix::fs::symlink("/dev/zero", &path).unwrap();
    assert!(matches!(
        atomic_write(&path, b"new"),
        Err(KernelError::Io { .. })
    ));
    assert!(path.symlink_metadata().unwrap().file_type().is_symlink());
}

#[test]
fn discovery_override_child() {
    let Some(root) = std::env::var_os("SK_SPECIAL_DISCOVERY_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    assert_eq!(
        discover_project_root(&root).unwrap(),
        root.canonicalize().unwrap()
    );
}

#[test]
fn project_override_with_fifo_git_marker_does_not_hang() {
    let directory = tempfile::tempdir().unwrap();
    fifo(&directory.path().join(".git"));
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "discovery_override_child"])
        .env("SPECIFY_REPO_ROOT", directory.path())
        .env("SK_SPECIAL_DISCOVERY_ROOT", directory.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("project override must not block on a FIFO .git");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
