use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use sk_kernel::{KernelError, atomic_write, read_bounded};
use tempfile::TempDir;

fn temp() -> TempDir {
    tempfile::tempdir().expect("temporary directory")
}

fn write_file(path: &Path, bytes: &[u8]) {
    let mut file = fs::File::create(path).expect("create file");
    file.write_all(bytes).expect("write file");
}

#[test]
fn reads_bytes_up_to_the_limit() {
    let temp = temp();
    let path = temp.path().join("data.bin");
    write_file(&path, b"hello\x00world");

    assert_eq!(read_bounded(&path, 11).unwrap(), b"hello\x00world");
    assert_eq!(read_bounded(&path, 100).unwrap(), b"hello\x00world");
}

#[test]
fn reads_empty_files_even_with_a_zero_limit() {
    let temp = temp();
    let path = temp.path().join("empty");
    write_file(&path, b"");

    assert_eq!(read_bounded(&path, 0).unwrap(), Vec::<u8>::new());
    assert_eq!(read_bounded(&path, 1).unwrap(), Vec::<u8>::new());
}

#[test]
fn rejects_content_one_byte_over_the_limit_without_truncation() {
    let temp = temp();
    let path = temp.path().join("data");
    write_file(&path, b"0123456789");

    for limit in [0, 1, 5, 9] {
        let error = read_bounded(&path, limit).unwrap_err();
        assert!(
            matches!(error, KernelError::TooLarge { limit: reported, .. } if reported == limit),
            "limit {limit}: {error}"
        );
    }
    assert_eq!(fs::read(&path).unwrap(), b"0123456789");
}

#[test]
fn rejects_large_sparse_files_via_metadata_without_reading_them() {
    let temp = temp();
    let path = temp.path().join("sparse");
    let file = fs::File::create(&path).expect("create sparse");
    file.set_len(1024 * 1024 * 1024).expect("sparse length");
    drop(file);

    let error = read_bounded(&path, 1024).unwrap_err();
    assert!(matches!(error, KernelError::TooLarge { .. }));
}

#[test]
fn missing_file_and_directory_are_typed_io_errors() {
    let temp = temp();
    let missing = temp.path().join("missing");
    assert!(matches!(
        read_bounded(&missing, 10).unwrap_err(),
        KernelError::Io { .. }
    ));

    let directory = temp.path().join("directory");
    fs::create_dir(&directory).expect("directory");
    assert!(matches!(
        read_bounded(&directory, 10).unwrap_err(),
        KernelError::Io { .. }
    ));
}

#[test]
fn atomic_write_creates_a_file_and_reports_the_change() {
    let temp = temp();
    let path = temp.path().join("new-file");
    let changed = atomic_write(&path, b"first").unwrap();

    assert!(changed);
    assert_eq!(fs::read(&path).unwrap(), b"first");
}

#[test]
fn atomic_write_reports_false_and_does_not_replace_on_identical_bytes() {
    let temp = temp();
    let path = temp.path().join("stable");
    assert!(atomic_write(&path, b"same bytes").unwrap());
    let first = fs::metadata(&path).expect("metadata before");

    assert!(!atomic_write(&path, b"same bytes").unwrap());
    let second = fs::metadata(&path).expect("metadata after");

    assert_eq!(fs::read(&path).unwrap(), b"same bytes");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            first.ino(),
            second.ino(),
            "no-op must not replace the inode"
        );
        assert_eq!(first.modified().unwrap(), second.modified().unwrap());
    }
    #[cfg(not(unix))]
    {
        assert_eq!(first.modified().unwrap(), second.modified().unwrap());
    }
}

#[test]
fn atomic_write_replaces_changed_content() {
    let temp = temp();
    let path = temp.path().join("replace");
    write_file(&path, b"old");

    assert!(atomic_write(&path, b"new content").unwrap());
    assert_eq!(fs::read(&path).unwrap(), b"new content");
}

#[cfg(unix)]
#[test]
fn changed_write_uses_a_fresh_inode() {
    use std::os::unix::fs::MetadataExt;

    let temp = temp();
    let path = temp.path().join("replace");
    write_file(&path, b"old");
    let before = fs::metadata(&path).unwrap();

    assert!(atomic_write(&path, b"new").unwrap());
    let after = fs::metadata(&path).unwrap();
    assert_ne!(before.ino(), after.ino());
}

#[test]
fn atomic_write_leaves_no_temporary_files_behind() {
    let temp = temp();
    let path = temp.path().join("file");
    let untouched = temp.path().join("other");
    write_file(&untouched, b"other");

    assert!(atomic_write(&path, b"one").unwrap());
    assert!(atomic_write(&path, b"two").unwrap());
    assert!(!atomic_write(&path, b"two").unwrap());

    let mut names = fs::read_dir(temp.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(names, ["file", "other"]);
}

#[test]
fn atomic_write_handles_binary_and_empty_payloads() {
    let temp = temp();
    let path = temp.path().join("binary");
    let payload = vec![0_u8, 255, 1, 2, 0, 10, 13];

    assert!(atomic_write(&path, &payload).unwrap());
    assert_eq!(fs::read(&path).unwrap(), payload);
    assert!(atomic_write(&path, b"").unwrap());
    assert_eq!(fs::read(&path).unwrap(), Vec::<u8>::new());
    assert!(!atomic_write(&path, b"").unwrap());
}

#[test]
fn atomic_write_does_not_create_missing_parents() {
    let temp = temp();
    let path = temp.path().join("missing-parent/file");

    let error = atomic_write(&path, b"content").unwrap_err();
    assert!(matches!(error, KernelError::Io { .. }));
    assert!(!temp.path().join("missing-parent").exists());
}

#[test]
fn atomic_write_refuses_a_directory_destination() {
    let temp = temp();
    let path = temp.path().join("directory");
    fs::create_dir(&path).expect("directory");

    let error = atomic_write(&path, b"content").unwrap_err();
    assert!(matches!(error, KernelError::Io { .. }));
    assert!(path.is_dir());
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
}

#[test]
fn concurrent_atomic_writes_never_expose_a_partial_payload() {
    let temp = temp();
    let path = temp.path().join("shared");
    let phase_a = b"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_vec();
    let phase_b = b"BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB".to_vec();
    write_file(&path, &phase_a);

    let stop = Arc::new(AtomicBool::new(false));
    let writer_path = path.clone();
    let writer_phase_a = phase_a.clone();
    let writer_phase_b = phase_b.clone();
    let writer_stop = Arc::clone(&stop);
    let writer = std::thread::spawn(move || {
        for index in 0..200 {
            let payload = if index % 2 == 0 {
                &writer_phase_b
            } else {
                &writer_phase_a
            };
            atomic_write(&writer_path, payload).expect("writer");
        }
        writer_stop.store(true, Ordering::Relaxed);
    });

    while !stop.load(Ordering::Relaxed) {
        let observed = read_bounded(&path, 1024).expect("reader");
        assert!(
            observed == phase_a || observed == phase_b,
            "observed partial payload of {} bytes",
            observed.len()
        );
    }
    writer.join().expect("writer thread");
}
