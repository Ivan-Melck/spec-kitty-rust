//! Durability and preservation acceptance tests for `atomic_write`.
//!
//! The write must not lose user content or user-chosen metadata: identical
//! bytes are a true no-op, changed bytes keep the destination's permission
//! bits where the platform can express them, a read-only file stays read-only,
//! and special-file/symlink hazards remain refused or contained.

use std::fs;
use std::io::Write;
use std::path::Path;

use sk_kernel::{KernelError, atomic_write};
use tempfile::TempDir;

fn temp() -> TempDir {
    tempfile::tempdir().expect("temporary directory")
}

fn write_file(path: &Path, bytes: &[u8]) {
    let mut file = fs::File::create(path).expect("create");
    file.write_all(bytes).expect("write");
}

#[cfg(unix)]
#[test]
fn changed_write_preserves_the_destination_mode() {
    use std::os::unix::fs::PermissionsExt;

    let temp = temp();
    let path = temp.path().join("mode");
    write_file(&path, b"old");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();

    assert!(atomic_write(&path, b"new").unwrap());
    assert_eq!(fs::read(&path).unwrap(), b"new");
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o640
    );
}

#[cfg(unix)]
#[test]
fn read_only_destination_is_replaced_and_stays_read_only() {
    use std::os::unix::fs::PermissionsExt;

    let temp = temp();
    let path = temp.path().join("readonly");
    write_file(&path, b"old");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();

    assert!(atomic_write(&path, b"new").unwrap());
    assert_eq!(fs::read(&path).unwrap(), b"new");
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o444, "managed read-only file must self-converge");
}

#[cfg(unix)]
#[test]
fn identical_read_only_write_is_a_true_no_op() {
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::fs::PermissionsExt;

    let temp = temp();
    let path = temp.path().join("readonly-same");
    write_file(&path, b"same");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
    let before = fs::metadata(&path).unwrap();

    assert!(!atomic_write(&path, b"same").unwrap());
    let after = fs::metadata(&path).unwrap();
    assert_eq!(before.ino(), after.ino());
    assert_eq!(after.permissions().mode() & 0o777, 0o444);
    assert_eq!(fs::read(&path).unwrap(), b"same");
}

#[cfg(unix)]
#[test]
fn symlink_destination_is_replaced_without_touching_its_target() {
    use std::os::unix::fs::symlink;

    let temp = temp();
    let target = temp.path().join("target");
    write_file(&target, b"target content");
    let link = temp.path().join("link");
    symlink(&target, &link).unwrap();

    assert!(atomic_write(&link, b"new link content").unwrap());
    assert_eq!(fs::read(&link).unwrap(), b"new link content");
    assert!(
        !link.symlink_metadata().unwrap().file_type().is_symlink(),
        "the rename replaces the link node, not the target"
    );
    assert_eq!(fs::read(&target).unwrap(), b"target content");
}

#[test]
fn new_files_use_the_private_temporary_default() {
    let temp = temp();
    let path = temp.path().join("new");
    assert!(atomic_write(&path, b"content").unwrap());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn directory_destination_is_refused_without_replacement_or_leftovers() {
    let temp = temp();
    let path = temp.path().join("directory");
    fs::create_dir(&path).unwrap();

    let error = atomic_write(&path, b"content").unwrap_err();
    assert!(matches!(error, KernelError::Io { .. }), "{error}");
    assert!(path.is_dir());
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn fifo_destination_is_refused_without_blocking_or_leaving_temporaries() {
    use std::process::Command;
    use std::sync::mpsc;
    use std::time::Duration;

    let temp = temp();
    let path = temp.path().join("fifo");
    assert!(
        Command::new("mkfifo")
            .arg(&path)
            .status()
            .unwrap()
            .success()
    );

    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(atomic_write(&path, b"content"));
    });
    let result = receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("FIFO equality probe must be nonblocking");
    assert!(matches!(result, Err(KernelError::Io { .. })));
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
}

#[cfg(windows)]
#[test]
fn read_only_destination_is_replaced_and_stays_read_only_on_windows() {
    let temp = temp();
    let path = temp.path().join("readonly");
    write_file(&path, b"old");
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&path, permissions).unwrap();

    assert!(atomic_write(&path, b"new").unwrap());
    assert_eq!(fs::read(&path).unwrap(), b"new");
    assert!(fs::metadata(&path).unwrap().permissions().readonly());
}
