//! Contracts for the no-follow write/timestamp helpers and the explicit
//! parent-directory policy on atomic replacement.

use std::fs;
use std::time::{Duration, UNIX_EPOCH};

use sk_kernel::no_follow::{
    chmod_fd, chmod_no_follow, fd_relative_dir_ops_supported, file_mode, read_text_no_follow,
    utime_no_follow, write_text_no_follow,
};
use sk_kernel::{AtomicWriteOptions, KernelError, atomic_write_with_options};

#[test]
fn no_follow_text_round_trips_through_a_regular_file() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("note.txt");
    write_text_no_follow(&path, "hello", 0o600).unwrap();
    assert_eq!(read_text_no_follow(&path, 1024).unwrap(), "hello");
    assert_eq!(file_mode(&path).unwrap(), 0o600);
    assert!(matches!(
        read_text_no_follow(&path, 2),
        Err(KernelError::TooLarge { .. })
    ));
}

#[cfg(unix)]
#[test]
fn no_follow_helpers_refuse_a_planted_symlink() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target.txt");
    fs::write(&target, b"kept").unwrap();
    let link = temp.path().join("link.txt");
    symlink(&target, &link).unwrap();

    assert!(matches!(
        read_text_no_follow(&link, 1024),
        Err(KernelError::NoFollow { .. })
    ));
    assert!(matches!(
        write_text_no_follow(&link, "replaced", 0o600),
        Err(KernelError::NoFollow { .. })
    ));
    assert_eq!(fs::read(&target).unwrap(), b"kept");
}

#[test]
fn chmod_helpers_apply_an_existing_files_mode() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("mode.txt");
    fs::write(&path, b"x").unwrap();
    chmod_no_follow(&path, 0o640).unwrap();
    assert_eq!(file_mode(&path).unwrap(), 0o640);
    let file = fs::OpenOptions::new().read(true).open(&path).unwrap();
    chmod_fd(&file, &path, 0o600).unwrap();
    assert_eq!(file_mode(&path).unwrap(), 0o600);
    assert_eq!(fd_relative_dir_ops_supported(), cfg!(unix));
}

#[cfg(unix)]
#[test]
fn utime_no_follow_sets_regular_file_times() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("times.txt");
    fs::write(&path, b"x").unwrap();
    let seconds = 1_700_000_000_i64;
    utime_no_follow(&path, seconds * 1_000_000_000, seconds * 1_000_000_000).unwrap();
    let metadata = fs::metadata(&path).unwrap();
    let mtime = metadata
        .modified()
        .unwrap()
        .duration_since(UNIX_EPOCH)
        .unwrap();
    assert_eq!(mtime, Duration::from_secs(seconds as u64));
}

#[test]
fn atomic_write_can_create_missing_parents_only_when_asked() {
    let temp = tempfile::tempdir().unwrap();
    let nested = temp.path().join("a/b/c/file.txt");
    assert!(sk_kernel::atomic_write(&nested, b"x").is_err());
    assert!(!nested.exists());

    let created = atomic_write_with_options(
        &nested,
        b"x",
        AtomicWriteOptions {
            create_parents: true,
        },
    )
    .unwrap();
    assert!(created);
    assert_eq!(fs::read(&nested).unwrap(), b"x");

    let again = atomic_write_with_options(
        &nested,
        b"x",
        AtomicWriteOptions {
            create_parents: true,
        },
    )
    .unwrap();
    assert!(!again, "identical content is a no-op");
}
