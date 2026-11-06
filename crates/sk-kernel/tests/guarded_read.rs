//! Acceptance tests for the guarded-read and no-follow read primitives.

use std::fs;

use sk_kernel::guarded_read::{read_guarded, read_guarded_text};
use sk_kernel::no_follow::read_no_follow;
use sk_kernel::{KernelError, read_bounded};
use tempfile::TempDir;

fn temp() -> TempDir {
    tempfile::tempdir().expect("temporary directory")
}

#[test]
fn guarded_read_parses_exactly_once_from_bounded_bytes() {
    let temp = temp();
    let path = temp.path().join("data");
    fs::write(&path, b"42").unwrap();

    let mut calls = 0;
    let value: i32 = read_guarded(&path, 16, |bytes| {
        calls += 1;
        std::str::from_utf8(bytes)
            .map_err(|error| error.to_string())?
            .parse::<i32>()
            .map_err(|error| error.to_string())
    })
    .expect("parse");

    assert_eq!(value, 42);
    assert_eq!(calls, 1);
}

#[test]
fn guarded_read_collapses_declared_parse_failures() {
    let temp = temp();
    let path = temp.path().join("data");
    fs::write(&path, b"not-a-number").unwrap();

    let error = read_guarded::<i32, _, _>(&path, 16, |_| Err("invalid integer")).unwrap_err();
    match error {
        KernelError::GuardedRead {
            path: reported,
            reason,
        } => {
            assert_eq!(reported, path);
            assert!(reason.contains("invalid integer"));
        }
        other => panic!("expected GuardedRead, got {other:?}"),
    }
}

#[test]
fn guarded_text_read_rejects_invalid_utf8_before_parsing() {
    let temp = temp();
    let path = temp.path().join("data");
    fs::write(&path, [0xff, 0xfe]).unwrap();

    let error =
        read_guarded_text::<String, _, _>(&path, 16, |text| Ok::<_, String>(text.to_owned()))
            .unwrap_err();
    assert!(matches!(error, KernelError::InvalidUtf8 { .. }), "{error}");
}

#[test]
fn guarded_read_applies_the_bounded_read_limit() {
    let temp = temp();
    let path = temp.path().join("data");
    fs::write(&path, b"0123456789").unwrap();
    let error =
        read_guarded::<usize, _, _>(&path, 9, |bytes| Ok::<_, String>(bytes.len())).unwrap_err();
    assert!(
        matches!(error, KernelError::TooLarge { limit: 9, .. }),
        "{error}"
    );
}

#[test]
fn guarded_read_keeps_missing_file_as_io() {
    let temp = temp();
    let path = temp.path().join("missing");
    let error =
        read_guarded::<usize, _, _>(&path, 16, |bytes| Ok::<_, String>(bytes.len())).unwrap_err();
    assert!(matches!(error, KernelError::Io { .. }), "{error}");
}

#[test]
fn no_follow_read_refuses_a_final_symlink() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let temp = temp();
        let target = temp.path().join("target");
        fs::write(&target, b"target bytes").unwrap();
        let link = temp.path().join("link");
        symlink(&target, &link).unwrap();

        let error = read_no_follow(&link, 64).unwrap_err();
        assert!(matches!(error, KernelError::NoFollow { .. }), "{error}");

        // The ordinary bounded read keeps its documented follow-symlink
        // behaviour; no-follow is opt-in at the owning boundary.
        assert_eq!(read_bounded(&link, 64).unwrap(), b"target bytes");
    }
}

#[test]
fn no_follow_read_reads_a_regular_file() {
    let temp = temp();
    let path = temp.path().join("regular");
    fs::write(&path, b"content").unwrap();
    assert_eq!(read_no_follow(&path, 64).unwrap(), b"content");
}

#[cfg(unix)]
#[test]
fn no_follow_read_rejects_a_fifo_without_blocking() {
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
        let _ = sender.send(read_no_follow(&path, 64));
    });
    let result = receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("nonblocking");
    assert!(matches!(result, Err(KernelError::Io { .. })), "{result:?}");
}
