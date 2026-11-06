//! Source-policy matrix for the Kernel writer operations.
//!
//! Every expectation here is the *observed* pinned-reference behaviour recorded in
//! the W1 Source probe (see `crates/sk-kernel/src/source_writer.rs` and the W1
//! report), not a mode table and not a convenience shape. The cases are the ones
//! the reference actually produces: fresh/writable/read-only/equal destinations,
//! symlink/broken-symlink/special/directory destinations, missing and read-only
//! parents, own and foreign PID artifacts, early and late encode failures,
//! publication and observer failures, retries, and the full physical footprint
//! (bytes + mode + which artifact survives).
//!
//! The umask-sensitive creation modes are asserted in
//! `source_writer_umask.rs`, which can set the process mask without racing other
//! tests; here a created mode is asserted against the mask *measured* from the
//! same process.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use sk_kernel::PythonString;
use sk_kernel::source_writer::{
    AtomicWrite, GeneratedWrite, SourcePayload, SourceWriteError, WriteListener,
    WriteListenerFailure, WriteObservation, WriterOperation, derived_json_temporary_path,
    generated_temporary_path, rebuild_log_temporary_path, write_source,
};

struct Fixture {
    directory: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self {
            directory: tempfile::tempdir().expect("temp dir"),
        }
    }

    fn path(&self) -> PathBuf {
        self.directory.path().join("output")
    }

    fn join(&self, name: &str) -> PathBuf {
        self.directory.path().join(name)
    }

    fn seed(&self, path: &Path, mode: u32, bytes: &[u8]) {
        fs::write(path, bytes).expect("seed");
        set_mode(path, mode);
    }

    /// Sorted `(name, kind, mode)` footprint, so a stray artifact is visible.
    fn footprint(&self) -> Vec<(String, &'static str, u32)> {
        let mut entries: Vec<_> = fs::read_dir(self.directory.path())
            .expect("read dir")
            .flatten()
            .map(|entry| {
                let path = entry.path();
                let metadata = fs::symlink_metadata(&path).expect("metadata");
                let kind = if metadata.file_type().is_symlink() {
                    "link"
                } else if metadata.is_dir() {
                    "dir"
                } else if metadata.file_type().is_fifo() {
                    "fifo"
                } else {
                    "file"
                };
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    kind,
                    mode_of(&path),
                )
            })
            .collect();
        entries.sort();
        entries
    }

    fn names(&self) -> Vec<String> {
        self.footprint().into_iter().map(|row| row.0).collect()
    }
}

fn mode_of(path: &Path) -> u32 {
    fs::symlink_metadata(path)
        .expect("metadata")
        .permissions()
        .mode()
        & 0o7777
}

fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("chmod");
}

/// The process umask, measured by asking the OS for 0o777 on a throwaway file.
fn measured_umask() -> u32 {
    let directory = tempfile::tempdir().expect("temp dir");
    let probe = directory.path().join("probe");
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o777)
        .open(&probe)
        .expect("probe");
    drop(file);
    0o777 & !mode_of(&probe)
}

#[derive(Default)]
struct Recorder {
    calls: Vec<(PathBuf, Vec<u8>)>,
    failure: Option<&'static str>,
}

impl WriteListener for Recorder {
    fn written(&mut self, path: &Path, bytes: &[u8]) -> Result<(), WriteListenerFailure> {
        self.calls.push((path.to_path_buf(), bytes.to_vec()));
        match self.failure {
            Some(message) => Err(WriteListenerFailure::new(message)),
            None => Ok(()),
        }
    }
}

fn atomic() -> WriterOperation {
    WriterOperation::atomic()
}

// ---------------------------------------------------------------------------
// Atomic operation: kernel.atomic.atomic_write
// ---------------------------------------------------------------------------

#[test]
fn atomic_fresh_destination_is_0600_and_notifies_the_published_bytes() {
    let fixture = Fixture::new();
    let path = fixture.path();
    let mut listener = Recorder::default();
    let receipt = write_source(
        atomic(),
        &path,
        SourcePayload::Bytes(b"new"),
        Some(&mut listener),
    )
    .expect("write");

    assert_eq!(fs::read(&path).expect("read"), b"new");
    assert_eq!(mode_of(&path), 0o600);
    assert_eq!(receipt.bytes, 3);
    assert_eq!(receipt.observation, WriteObservation::Notified);
    assert_eq!(listener.calls.len(), 1);
    assert_eq!(listener.calls[0].0, path);
    assert_eq!(listener.calls[0].1, b"new");
    // mkstemp sibling published, so nothing is left behind.
    assert_eq!(fixture.names(), vec!["output".to_string()]);
    let temporary = receipt.temporary.expect("temporary path recorded");
    assert!(
        temporary
            .file_name()
            .expect("name")
            .to_string_lossy()
            .starts_with(".atomic-"),
        "unexpected temporary name: {temporary:?}"
    );
    assert!(temporary.to_string_lossy().ends_with(".tmp"));
}

#[test]
fn atomic_replaces_a_writable_destination_without_preserving_its_mode() {
    let fixture = Fixture::new();
    let path = fixture.path();
    fixture.seed(&path, 0o664, b"old");
    let before_inode = fs::symlink_metadata(&path).expect("stat").ino();

    write_source(atomic(), &path, SourcePayload::Bytes(b"new"), None).expect("write");

    assert_eq!(fs::read(&path).expect("read"), b"new");
    assert_eq!(
        mode_of(&path),
        0o600,
        "a writable old mode is not preserved by the reference"
    );
    assert_ne!(
        fs::symlink_metadata(&path).expect("stat").ino(),
        before_inode,
        "the reference always replaces the inode"
    );
}

#[test]
fn atomic_restores_only_the_observed_read_only_mode() {
    let fixture = Fixture::new();
    let path = fixture.path();
    fixture.seed(&path, 0o444, b"old");
    let before_inode = fs::symlink_metadata(&path).expect("stat").ino();

    write_source(atomic(), &path, SourcePayload::Bytes(b"new"), None).expect("write");

    assert_eq!(fs::read(&path).expect("read"), b"new");
    assert_eq!(mode_of(&path), 0o444);
    assert_ne!(
        fs::symlink_metadata(&path).expect("stat").ino(),
        before_inode
    );
}

#[test]
fn atomic_always_publishes_equal_bytes() {
    let fixture = Fixture::new();
    let path = fixture.path();
    fixture.seed(&path, 0o664, b"same");
    let before_inode = fs::symlink_metadata(&path).expect("stat").ino();

    write_source(atomic(), &path, SourcePayload::Bytes(b"same"), None).expect("write");

    assert_eq!(fs::read(&path).expect("read"), b"same");
    assert_ne!(
        fs::symlink_metadata(&path).expect("stat").ino(),
        before_inode,
        "the reference publishes even when the bytes are equal"
    );
    assert_eq!(mode_of(&path), 0o600);
}

#[test]
fn atomic_encodes_before_it_relaxes_the_destination_or_creates_a_temporary() {
    let fixture = Fixture::new();
    let path = fixture.path();
    fixture.seed(&path, 0o444, b"old");
    let before = fixture.footprint();
    let surrogate = PythonString::from_codepoints(&[b'x' as u32, 0xd800]).expect("python string");

    let error = write_source(atomic(), &path, SourcePayload::Python(&surrogate), None)
        .expect_err("encode failure");

    match &error {
        SourceWriteError::Encode { source, .. } => {
            assert_eq!(source.start, 1);
            assert_eq!(source.end, 2);
            assert_eq!(source.reason, "surrogates not allowed");
            assert_eq!(source.object(), &[b'x' as u32, 0xd800]);
            assert_eq!(
                source.to_string(),
                "'utf-8' codec can't encode character '\\ud800' in position 1: surrogates not allowed"
            );
        }
        other => panic!("unexpected error: {other:?}"),
    }
    assert!(!error.is_os_error());
    // Nothing at all changed: no relaxation, no temporary, no destination write.
    assert_eq!(fs::read(&path).expect("read"), b"old");
    assert_eq!(mode_of(&path), 0o444);
    assert_eq!(fixture.footprint(), before);
}

#[test]
fn atomic_encode_failure_on_a_fresh_destination_creates_nothing_even_with_mkdir() {
    let fixture = Fixture::new();
    let path = fixture.join("made").join("output");
    let surrogate = PythonString::from_codepoints(&[0xdcff]).expect("python string");

    let error = write_source(
        WriterOperation::atomic_creating_parents(),
        &path,
        SourcePayload::Python(&surrogate),
        None,
    )
    .expect_err("encode failure");

    assert!(matches!(error, SourceWriteError::Encode { .. }));
    // Source creates the parent first, then fails to encode.
    assert!(fixture.join("made").is_dir());
    assert_eq!(fs::read_dir(fixture.join("made")).expect("dir").count(), 0);
}

#[test]
fn atomic_replaces_a_destination_symlink_node_and_never_writes_through_it() {
    let fixture = Fixture::new();
    let path = fixture.path();
    let target = fixture.join("target");
    fixture.seed(&target, 0o664, b"keep");
    std::os::unix::fs::symlink(&target, &path).expect("symlink");

    write_source(atomic(), &path, SourcePayload::Bytes(b"new"), None).expect("write");

    assert_eq!(fs::read(&target).expect("target"), b"keep");
    assert_eq!(fs::read(&path).expect("path"), b"new");
    assert!(
        !fs::symlink_metadata(&path)
            .expect("stat")
            .file_type()
            .is_symlink()
    );
    assert_eq!(mode_of(&path), 0o600);
    assert_eq!(mode_of(&target), 0o664);
}

#[test]
fn atomic_replaces_a_broken_symlink_node_and_does_not_create_its_target() {
    let fixture = Fixture::new();
    let path = fixture.path();
    let absent_target = fixture.join("absent-target");
    std::os::unix::fs::symlink(&absent_target, &path).expect("symlink");

    write_source(atomic(), &path, SourcePayload::Bytes(b"new"), None).expect("write");

    assert_eq!(fs::read(&path).expect("path"), b"new");
    assert!(!absent_target.exists());
    assert_eq!(mode_of(&path), 0o600);
}

#[test]
fn atomic_replaces_a_fifo_destination() {
    let fixture = Fixture::new();
    let path = fixture.path();
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .expect("mkfifo")
            .success()
    );

    write_source(atomic(), &path, SourcePayload::Bytes(b"new"), None).expect("write");

    assert_eq!(fs::read(&path).expect("read"), b"new");
    assert_eq!(mode_of(&path), 0o600);
    assert!(
        !fs::symlink_metadata(&path)
            .expect("stat")
            .file_type()
            .is_fifo()
    );
}

#[test]
fn atomic_refuses_a_directory_destination_at_the_replace_and_cleans_the_temporary() {
    let fixture = Fixture::new();
    let path = fixture.path();
    fs::create_dir(&path).expect("dir");
    set_mode(&path, 0o775);

    let error = write_source(atomic(), &path, SourcePayload::Bytes(b"new"), None)
        .expect_err("directory destination");

    assert!(
        error.is_os_error(),
        "the reference raises an OSError subclass"
    );
    assert!(fs::symlink_metadata(&path).expect("stat").is_dir());
    assert_eq!(
        fixture.names(),
        vec!["output".to_string()],
        "the failed temporary is removed"
    );
    assert_eq!(mode_of(&path), 0o775);
}

#[test]
fn atomic_missing_parent_fails_without_a_temp_and_creates_it_only_with_mkdir() {
    let fixture = Fixture::new();
    let path = fixture.join("absent").join("output");

    let error = write_source(atomic(), &path, SourcePayload::Bytes(b"new"), None)
        .expect_err("missing parent");
    assert!(error.is_os_error());
    assert!(!fixture.join("absent").exists());
    assert!(fixture.names().is_empty());

    write_source(
        WriterOperation::atomic_creating_parents(),
        &path,
        SourcePayload::Bytes(b"new"),
        None,
    )
    .expect("mkdir=True creates the parent");
    assert_eq!(fs::read(&path).expect("read"), b"new");
}

#[test]
fn atomic_read_only_parent_fails_before_any_temporary_exists() {
    let fixture = Fixture::new();
    let parent = fixture.join("locked");
    fs::create_dir(&parent).expect("dir");
    set_mode(&parent, 0o555);
    let path = parent.join("output");

    let error = write_source(atomic(), &path, SourcePayload::Bytes(b"new"), None)
        .expect_err("read-only parent");

    assert!(error.is_os_error());
    assert_eq!(fs::read_dir(&parent).expect("dir").count(), 0);
    set_mode(&parent, 0o755);
}

#[test]
fn atomic_observer_failure_propagates_after_the_publish_and_mode_restore() {
    let fixture = Fixture::new();
    let path = fixture.path();
    fixture.seed(&path, 0o444, b"old");
    let mut listener = Recorder {
        calls: Vec::new(),
        failure: Some("ledger refused"),
    };

    let error = write_source(
        atomic(),
        &path,
        SourcePayload::Bytes(b"new"),
        Some(&mut listener),
    )
    .expect_err("observer failure");

    assert!(matches!(error, SourceWriteError::Observer { .. }));
    assert!(error.published_before_error());
    assert_eq!(fs::read(&path).expect("read"), b"new");
    assert_eq!(mode_of(&path), 0o444, "the mode restore already ran");
    assert_eq!(listener.calls.len(), 1);
    assert_eq!(fixture.names(), vec!["output".to_string()]);
}

#[test]
fn atomic_without_a_listener_reports_no_listener_instead_of_pretending() {
    let fixture = Fixture::new();
    let path = fixture.path();
    let receipt = write_source(atomic(), &path, SourcePayload::Bytes(b"new"), None).expect("write");
    assert_eq!(receipt.observation, WriteObservation::NoListener);
}

#[test]
fn atomic_retries_after_an_encode_failure() {
    let fixture = Fixture::new();
    let path = fixture.path();
    let surrogate = PythonString::from_codepoints(&[0xd800]).expect("python string");
    write_source(atomic(), &path, SourcePayload::Python(&surrogate), None)
        .expect_err("encode failure");

    write_source(atomic(), &path, SourcePayload::Text("ok"), None).expect("retry");

    assert_eq!(fs::read(&path).expect("read"), b"ok");
    assert_eq!(mode_of(&path), 0o600);
}

#[test]
fn atomic_keeps_unrelated_siblings_and_index_files_untouched() {
    let fixture = Fixture::new();
    let path = fixture.path();
    let foreign = fixture.join("foreign.txt");
    fixture.seed(&foreign, 0o640, b"user content");
    let lock = fixture.join("index.lock");
    fixture.seed(&lock, 0o600, b"foreign lock");
    let before = fixture.footprint();

    write_source(atomic(), &path, SourcePayload::Bytes(b"new"), None).expect("write");

    assert_eq!(fs::read(&foreign).expect("foreign"), b"user content");
    assert_eq!(fs::read(&lock).expect("lock"), b"foreign lock");
    let mut expected = before;
    expected.push(("output".to_string(), "file", 0o600));
    expected.sort();
    assert_eq!(fixture.footprint(), expected);
}

// ---------------------------------------------------------------------------
// Generated operation: runtime.generated_writer.write_generated_file
// ---------------------------------------------------------------------------

#[test]
fn generated_uses_the_executing_pid_sibling_name() {
    let fixture = Fixture::new();
    let path = fixture.path();
    let expected = format!(".output.tmp{}", std::process::id());
    assert_eq!(
        generated_temporary_path(&path).file_name().expect("name"),
        expected.as_str()
    );

    let receipt = write_source(
        WriterOperation::generated(true),
        &path,
        SourcePayload::Bytes(b"new"),
        None,
    )
    .expect("write");

    assert_eq!(receipt.temporary_pid, Some(std::process::id()));
    assert_eq!(
        receipt
            .temporary
            .as_ref()
            .expect("temporary")
            .file_name()
            .expect("name"),
        expected.as_str()
    );
    assert_eq!(fs::read(&path).expect("read"), b"new");
    assert_eq!(fixture.names(), vec!["output".to_string()]);
}

#[test]
fn generated_read_only_lifecycle_matches_the_reference() {
    let fixture = Fixture::new();
    let path = fixture.path();
    let mask = measured_umask();

    write_source(
        WriterOperation::generated(false),
        &path,
        SourcePayload::Bytes(b"new"),
        None,
    )
    .expect("writable write");
    assert_eq!(mode_of(&path), 0o666 & !mask);

    write_source(
        WriterOperation::generated(true),
        &path,
        SourcePayload::Bytes(b"newer"),
        None,
    )
    .expect("read-only write");
    assert_eq!(mode_of(&path), (0o666 & !mask) & !0o222);
    assert_eq!(mode_of(&path) & 0o222, 0, "write bits are stripped");
}

#[test]
fn generated_relaxes_with_all_write_bits_before_replacing_a_read_only_file() {
    let fixture = Fixture::new();
    let path = fixture.path();
    fixture.seed(&path, 0o444, b"old");
    let mask = measured_umask();

    write_source(
        WriterOperation::generated(false),
        &path,
        SourcePayload::Bytes(b"new"),
        None,
    )
    .expect("write");

    assert_eq!(fs::read(&path).expect("read"), b"new");
    // The temporary's mode wins; the observed 0444 does not survive.
    assert_eq!(mode_of(&path), 0o666 & !mask);
}

#[test]
fn generated_refuses_a_symlink_destination_before_touching_anything() {
    let fixture = Fixture::new();
    let path = fixture.path();
    let target = fixture.join("target");
    fixture.seed(&target, 0o664, b"keep");
    std::os::unix::fs::symlink(&target, &path).expect("symlink");
    let before = fixture.footprint();

    let error = write_source(
        WriterOperation::generated(true),
        &path,
        SourcePayload::Bytes(b"new"),
        None,
    )
    .expect_err("symlink refusal");

    match &error {
        SourceWriteError::SymlinkRefused { path: refused } => assert_eq!(refused, &path),
        other => panic!("unexpected error: {other:?}"),
    }
    assert!(error.is_value_error());
    assert!(!error.is_os_error());
    assert_eq!(
        error.to_string(),
        format!(
            "write_generated_file refuses to write through a symlink: {} (the managed \
             generated-file tree is expected to contain only regular files; replace or \
             remove the symlink before writing)",
            path.display()
        )
    );
    assert_eq!(fs::read(&target).expect("target"), b"keep");
    assert_eq!(fixture.footprint(), before);
}

#[test]
fn generated_refuses_a_broken_symlink_destination() {
    let fixture = Fixture::new();
    let path = fixture.path();
    let absent_target = fixture.join("absent-target");
    std::os::unix::fs::symlink(&absent_target, &path).expect("symlink");

    let error = write_source(
        WriterOperation::generated(true),
        &path,
        SourcePayload::Bytes(b"new"),
        None,
    )
    .expect_err("symlink refusal");

    assert!(matches!(error, SourceWriteError::SymlinkRefused { .. }));
    assert!(!absent_target.exists());
    assert!(fs::symlink_metadata(&path).expect("stat").is_symlink());
}

#[test]
fn generated_refuses_an_own_pid_artifact_that_is_a_file() {
    let fixture = Fixture::new();
    let path = fixture.path();
    let artifact = generated_temporary_path(&path);
    fixture.seed(&artifact, 0o664, b"foreign");

    let error = write_source(
        WriterOperation::generated(true),
        &path,
        SourcePayload::Bytes(b"new"),
        None,
    )
    .expect_err("unproven artifact");

    match &error {
        SourceWriteError::UnprovenArtifact { path: refused } => assert_eq!(refused, &artifact),
        other => panic!("unexpected error: {other:?}"),
    }
    assert_eq!(
        error.to_string(),
        format!(
            "Refusing to replace an unproven atomic writer artifact: {}",
            artifact.display()
        )
    );
    assert_eq!(fs::read(&artifact).expect("artifact"), b"foreign");
    assert!(!path.exists());
}

#[test]
fn generated_refuses_an_own_pid_artifact_that_is_a_symlink() {
    let fixture = Fixture::new();
    let path = fixture.path();
    let artifact = generated_temporary_path(&path);
    let elsewhere = fixture.join("elsewhere");
    fixture.seed(&elsewhere, 0o664, b"foreign");
    std::os::unix::fs::symlink(&elsewhere, &artifact).expect("symlink");

    let error = write_source(
        WriterOperation::generated(true),
        &path,
        SourcePayload::Bytes(b"new"),
        None,
    )
    .expect_err("unproven artifact");

    assert!(matches!(error, SourceWriteError::UnprovenArtifact { .. }));
    assert_eq!(fs::read(&elsewhere).expect("elsewhere"), b"foreign");
    assert!(!path.exists());
}

#[test]
fn generated_leaves_a_foreign_pid_artifact_alone() {
    let fixture = Fixture::new();
    let path = fixture.path();
    let foreign = fixture.join(".output.tmp999999999");
    fixture.seed(&foreign, 0o664, b"other process");

    write_source(
        WriterOperation::generated(true),
        &path,
        SourcePayload::Bytes(b"new"),
        None,
    )
    .expect("write");

    assert_eq!(fs::read(&foreign).expect("foreign"), b"other process");
    assert_eq!(mode_of(&foreign), 0o664);
    assert_eq!(fs::read(&path).expect("read"), b"new");
}

#[test]
fn generated_encode_failure_still_runs_the_read_only_finally() {
    let fixture = Fixture::new();
    let path = fixture.path();
    fixture.seed(&path, 0o444, b"old");
    let surrogate =
        PythonString::from_codepoints(&[b'x' as u32, 0xd800, b'y' as u32]).expect("python string");

    let error = write_source(
        WriterOperation::generated(true),
        &path,
        SourcePayload::Python(&surrogate),
        None,
    )
    .expect_err("encode failure");

    assert!(matches!(error, SourceWriteError::Encode { .. }));
    assert_eq!(fs::read(&path).expect("read"), b"old");
    assert_eq!(mode_of(&path), 0o444, "the finally strip restores 0444");
    assert_eq!(
        fixture.names(),
        vec!["output".to_string()],
        "the temporary is removed"
    );
}

#[test]
fn generated_encode_failure_without_the_read_only_lifecycle_leaves_the_file_writable() {
    let fixture = Fixture::new();
    let path = fixture.path();
    fixture.seed(&path, 0o444, b"old");
    let surrogate = PythonString::from_codepoints(&[0xd800]).expect("python string");

    write_source(
        WriterOperation::generated(false),
        &path,
        SourcePayload::Python(&surrogate),
        None,
    )
    .expect_err("encode failure");

    assert_eq!(fs::read(&path).expect("read"), b"old");
    assert_eq!(
        mode_of(&path),
        0o666,
        "the reference relaxes with 0o222 and does not strip it back"
    );
}

#[test]
fn generated_directory_destination_fails_and_strips_the_directory_write_bits() {
    let fixture = Fixture::new();
    let path = fixture.path();
    fs::create_dir(&path).expect("dir");
    set_mode(&path, 0o775);

    let error = write_source(
        WriterOperation::generated(true),
        &path,
        SourcePayload::Bytes(b"new"),
        None,
    )
    .expect_err("directory destination");

    assert!(error.is_os_error());
    // The reference's `finally` strips 0222 from whatever `path` is.
    assert_eq!(mode_of(&path), 0o555);
    assert_eq!(fixture.names(), vec!["output".to_string()]);
}

#[test]
fn generated_never_notifies_the_observer() {
    let fixture = Fixture::new();
    let path = fixture.path();
    let mut listener = Recorder::default();
    let receipt = write_source(
        WriterOperation::generated(true),
        &path,
        SourcePayload::Bytes(b"new"),
        Some(&mut listener),
    )
    .expect("write");
    assert_eq!(receipt.observation, WriteObservation::NotApplicable);
    assert!(listener.calls.is_empty());
}

#[test]
fn generated_retries_after_removing_the_unproven_artifact() {
    let fixture = Fixture::new();
    let path = fixture.path();
    let artifact = generated_temporary_path(&path);
    fixture.seed(&artifact, 0o664, b"foreign");
    write_source(
        WriterOperation::generated(true),
        &path,
        SourcePayload::Bytes(b"new"),
        None,
    )
    .expect_err("unproven artifact");

    fs::remove_file(&artifact).expect("remove artifact");
    write_source(
        WriterOperation::generated(true),
        &path,
        SourcePayload::Bytes(b"new"),
        None,
    )
    .expect("retry");

    assert_eq!(fs::read(&path).expect("read"), b"new");
    assert_eq!(mode_of(&path), 0o444);
}

// ---------------------------------------------------------------------------
// TextTruncate operation: Path.write_text
// ---------------------------------------------------------------------------

#[test]
fn text_creates_with_umask_mode_and_no_temporary() {
    let fixture = Fixture::new();
    let path = fixture.path();
    let mask = measured_umask();

    let receipt = write_source(
        WriterOperation::TextTruncate,
        &path,
        SourcePayload::Text("new"),
        None,
    )
    .expect("write");

    assert_eq!(fs::read(&path).expect("read"), b"new");
    assert_eq!(mode_of(&path), 0o666 & !mask);
    assert!(receipt.temporary.is_none());
    assert_eq!(receipt.observation, WriteObservation::NotApplicable);
    assert_eq!(fixture.names(), vec!["output".to_string()]);
}

#[test]
fn text_keeps_the_existing_inode_and_mode_and_rewrites_equal_bytes() {
    let fixture = Fixture::new();
    let path = fixture.path();
    fixture.seed(&path, 0o664, b"same");
    let before_inode = fs::symlink_metadata(&path).expect("stat").ino();

    write_source(
        WriterOperation::TextTruncate,
        &path,
        SourcePayload::Text("same"),
        None,
    )
    .expect("write");

    assert_eq!(fs::read(&path).expect("read"), b"same");
    assert_eq!(
        fs::symlink_metadata(&path).expect("stat").ino(),
        before_inode
    );
    assert_eq!(mode_of(&path), 0o664);
}

#[test]
fn text_creates_the_target_of_a_broken_symlink() {
    let fixture = Fixture::new();
    let path = fixture.path();
    let absent_target = fixture.join("absent-target");
    std::os::unix::fs::symlink(&absent_target, &path).expect("symlink");

    write_source(
        WriterOperation::TextTruncate,
        &path,
        SourcePayload::Text("new"),
        None,
    )
    .expect("write");

    assert_eq!(fs::read(&absent_target).expect("target"), b"new");
    assert!(fs::symlink_metadata(&path).expect("stat").is_symlink());
}

#[test]
fn text_refuses_a_read_only_destination_and_leaves_it_unchanged() {
    let fixture = Fixture::new();
    let path = fixture.path();
    fixture.seed(&path, 0o444, b"old");
    let before = fixture.footprint();

    let error = write_source(
        WriterOperation::TextTruncate,
        &path,
        SourcePayload::Text("new"),
        None,
    )
    .expect_err("permission denied");

    assert!(error.is_os_error());
    assert_eq!(fs::read(&path).expect("read"), b"old");
    assert_eq!(fixture.footprint(), before);
}

#[test]
fn text_follows_a_destination_symlink() {
    let fixture = Fixture::new();
    let path = fixture.path();
    let target = fixture.join("target");
    fixture.seed(&target, 0o664, b"keep");
    std::os::unix::fs::symlink(&target, &path).expect("symlink");

    write_source(
        WriterOperation::TextTruncate,
        &path,
        SourcePayload::Text("new"),
        None,
    )
    .expect("write");

    assert_eq!(fs::read(&target).expect("target"), b"new");
    assert_eq!(fs::read(&path).expect("path"), b"new");
    assert!(
        fs::symlink_metadata(&path)
            .expect("stat")
            .file_type()
            .is_symlink()
    );
    assert_eq!(mode_of(&target), 0o664);
}

#[test]
fn text_truncates_before_it_encodes() {
    let fixture = Fixture::new();
    let path = fixture.path();
    fixture.seed(&path, 0o664, b"0123456789");
    let surrogate =
        PythonString::from_codepoints(&[b'x' as u32, 0xd800, b'y' as u32]).expect("python string");

    let error = write_source(
        WriterOperation::TextTruncate,
        &path,
        SourcePayload::Python(&surrogate),
        None,
    )
    .expect_err("encode failure");

    assert!(matches!(error, SourceWriteError::Encode { .. }));
    assert_eq!(
        fs::read(&path).expect("read"),
        b"",
        "the destination was opened and truncated before the encode"
    );
    assert_eq!(mode_of(&path), 0o664);
}

#[test]
fn text_missing_parent_and_directory_destination_are_os_errors() {
    let fixture = Fixture::new();
    let missing = fixture.join("absent").join("output");
    let error = write_source(
        WriterOperation::TextTruncate,
        &missing,
        SourcePayload::Text("new"),
        None,
    )
    .expect_err("missing parent");
    assert!(error.is_os_error());
    assert!(!fixture.join("absent").exists());

    let directory = fixture.path();
    fs::create_dir(&directory).expect("dir");
    let error = write_source(
        WriterOperation::TextTruncate,
        &directory,
        SourcePayload::Text("new"),
        None,
    )
    .expect_err("directory destination");
    assert!(error.is_os_error());
    assert!(fs::symlink_metadata(&directory).expect("stat").is_dir());
}

#[test]
fn text_retries_after_the_destination_becomes_writable() {
    let fixture = Fixture::new();
    let path = fixture.path();
    fixture.seed(&path, 0o444, b"old");
    write_source(
        WriterOperation::TextTruncate,
        &path,
        SourcePayload::Text("new"),
        None,
    )
    .expect_err("permission denied");

    set_mode(&path, 0o644);
    write_source(
        WriterOperation::TextTruncate,
        &path,
        SourcePayload::Text("new"),
        None,
    )
    .expect("retry");

    assert_eq!(fs::read(&path).expect("read"), b"new");
    assert_eq!(mode_of(&path), 0o644);
}

#[test]
fn text_write_error_on_a_full_device_is_a_genuine_os_error() {
    // A real ENOSPC on /dev/full: the open succeeds and the write fails.
    let error = write_source(
        WriterOperation::TextTruncate,
        Path::new("/dev/full"),
        SourcePayload::Text("new"),
        None,
    )
    .expect_err("ENOSPC");
    assert!(error.is_os_error());
    assert!(
        error.to_string().contains("No space left on device")
            || error.to_string().contains("os error 28"),
        "unexpected error text: {error}"
    );
}

// ---------------------------------------------------------------------------
// DerivedJson operation: status.views._atomic_write_json
// ---------------------------------------------------------------------------

#[test]
fn derived_json_uses_the_fixed_sibling_name_and_notifies_after_publish() {
    let fixture = Fixture::new();
    let path = fixture.path();
    assert_eq!(
        derived_json_temporary_path(&path)
            .file_name()
            .expect("name"),
        "output.json.tmp"
    );
    let mask = measured_umask();
    let mut listener = Recorder::default();

    let receipt = write_source(
        WriterOperation::DerivedJson,
        &path,
        SourcePayload::Text("{\n  \"a\": 1\n}\n"),
        Some(&mut listener),
    )
    .expect("write");

    assert_eq!(fs::read(&path).expect("read"), b"{\n  \"a\": 1\n}\n");
    assert_eq!(mode_of(&path), 0o666 & !mask);
    assert_eq!(receipt.observation, WriteObservation::Notified);
    assert_eq!(listener.calls.len(), 1);
    assert_eq!(listener.calls[0].1, b"{\n  \"a\": 1\n}\n");
    assert_eq!(fixture.names(), vec!["output".to_string()]);
}

#[test]
fn derived_json_replaces_a_read_only_destination_without_relaxing_it() {
    let fixture = Fixture::new();
    let path = fixture.path();
    fixture.seed(&path, 0o444, b"old");
    let mask = measured_umask();

    write_source(
        WriterOperation::DerivedJson,
        &path,
        SourcePayload::Text("{}"),
        None,
    )
    .expect("write");

    assert_eq!(fs::read(&path).expect("read"), b"{}");
    assert_eq!(mode_of(&path), 0o666 & !mask);
}

#[test]
fn derived_json_truncates_and_reuses_an_existing_temporary() {
    let fixture = Fixture::new();
    let path = fixture.path();
    let temporary = derived_json_temporary_path(&path);
    fixture.seed(&temporary, 0o600, b"stale artifact that is longer");

    write_source(
        WriterOperation::DerivedJson,
        &path,
        SourcePayload::Text("{}"),
        None,
    )
    .expect("write");

    assert_eq!(fs::read(&path).expect("read"), b"{}");
    assert!(!temporary.exists());
}

#[test]
fn derived_json_replaces_a_destination_symlink_node() {
    let fixture = Fixture::new();
    let path = fixture.path();
    let target = fixture.join("target");
    fixture.seed(&target, 0o664, b"keep");
    std::os::unix::fs::symlink(&target, &path).expect("symlink");

    write_source(
        WriterOperation::DerivedJson,
        &path,
        SourcePayload::Text("{}"),
        None,
    )
    .expect("write");

    assert_eq!(fs::read(&target).expect("target"), b"keep");
    assert_eq!(fs::read(&path).expect("path"), b"{}");
    assert!(
        !fs::symlink_metadata(&path)
            .expect("stat")
            .file_type()
            .is_symlink()
    );
}

#[test]
fn derived_json_replaces_a_broken_symlink_node() {
    let fixture = Fixture::new();
    let path = fixture.path();
    let absent_target = fixture.join("absent-target");
    std::os::unix::fs::symlink(&absent_target, &path).expect("symlink");

    write_source(
        WriterOperation::DerivedJson,
        &path,
        SourcePayload::Text("{}"),
        None,
    )
    .expect("write");

    assert_eq!(fs::read(&path).expect("path"), b"{}");
    assert!(!absent_target.exists());
}

#[test]
fn derived_json_encode_failure_leaves_the_zero_byte_temporary_behind() {
    let fixture = Fixture::new();
    let path = fixture.path();
    let surrogate = PythonString::from_codepoints(&[0xd800]).expect("python string");

    let error = write_source(
        WriterOperation::DerivedJson,
        &path,
        SourcePayload::Python(&surrogate),
        None,
    )
    .expect_err("encode failure");

    assert!(matches!(error, SourceWriteError::Encode { .. }));
    assert!(!path.exists());
    assert_eq!(
        fs::read(derived_json_temporary_path(&path)).expect("temporary"),
        b"",
        "_atomic_write_json has no cleanup path"
    );
}

#[test]
fn derived_json_replace_failure_keeps_the_written_temporary() {
    let fixture = Fixture::new();
    let path = fixture.path();
    fs::create_dir(&path).expect("dir");

    let error = write_source(
        WriterOperation::DerivedJson,
        &path,
        SourcePayload::Text("{}"),
        None,
    )
    .expect_err("directory destination");

    assert!(error.is_os_error());
    assert_eq!(
        fs::read(derived_json_temporary_path(&path)).expect("temporary"),
        b"{}"
    );
}

#[test]
fn derived_json_observer_failure_propagates_after_publication() {
    let fixture = Fixture::new();
    let path = fixture.path();
    let mut listener = Recorder {
        calls: Vec::new(),
        failure: Some("ledger refused"),
    };

    let error = write_source(
        WriterOperation::DerivedJson,
        &path,
        SourcePayload::Text("{}"),
        Some(&mut listener),
    )
    .expect_err("observer failure");

    assert!(matches!(error, SourceWriteError::Observer { .. }));
    assert_eq!(fs::read(&path).expect("read"), b"{}");
    assert_eq!(listener.calls.len(), 1);
}

// ---------------------------------------------------------------------------
// RebuildLog operation: migration.rebuild_state.rebuild_event_log
// ---------------------------------------------------------------------------

#[test]
fn rebuild_log_uses_the_fixed_tmp_sibling_and_never_notifies() {
    let fixture = Fixture::new();
    let path = fixture.path();
    assert_eq!(
        rebuild_log_temporary_path(&path).file_name().expect("name"),
        "output.tmp"
    );
    let mask = measured_umask();
    let mut listener = Recorder::default();

    let receipt = write_source(
        WriterOperation::RebuildLog,
        &path,
        SourcePayload::Text("line\n"),
        Some(&mut listener),
    )
    .expect("write");

    assert_eq!(fs::read(&path).expect("read"), b"line\n");
    assert_eq!(mode_of(&path), 0o666 & !mask);
    assert_eq!(receipt.observation, WriteObservation::NotApplicable);
    assert!(listener.calls.is_empty());
    assert_eq!(fixture.names(), vec!["output".to_string()]);
}

#[test]
fn rebuild_log_replaces_a_read_only_destination() {
    let fixture = Fixture::new();
    let path = fixture.path();
    fixture.seed(&path, 0o444, b"old");
    let mask = measured_umask();

    write_source(
        WriterOperation::RebuildLog,
        &path,
        SourcePayload::Bytes(b"new"),
        None,
    )
    .expect("write");

    assert_eq!(fs::read(&path).expect("read"), b"new");
    assert_eq!(mode_of(&path), 0o666 & !mask);
}

#[test]
fn rebuild_log_encode_failure_removes_the_temporary() {
    let fixture = Fixture::new();
    let path = fixture.path();
    fixture.seed(&path, 0o664, b"old");
    let surrogate = PythonString::from_codepoints(&[0xd800]).expect("python string");

    let error = write_source(
        WriterOperation::RebuildLog,
        &path,
        SourcePayload::Python(&surrogate),
        None,
    )
    .expect_err("encode failure");

    assert!(matches!(error, SourceWriteError::Encode { .. }));
    assert_eq!(fs::read(&path).expect("read"), b"old");
    assert_eq!(
        fixture.names(),
        vec!["output".to_string()],
        "rebuild_event_log removes the temporary in a finally"
    );
}

#[test]
fn rebuild_log_replace_failure_removes_the_temporary() {
    let fixture = Fixture::new();
    let path = fixture.path();
    fs::create_dir(&path).expect("dir");

    let error = write_source(
        WriterOperation::RebuildLog,
        &path,
        SourcePayload::Bytes(b"rows\n"),
        None,
    )
    .expect_err("directory destination");

    assert!(error.is_os_error());
    assert_eq!(fixture.names(), vec!["output".to_string()]);
}

// ---------------------------------------------------------------------------
// Shared mechanism and policy identity
// ---------------------------------------------------------------------------

#[test]
fn operations_do_not_infer_their_policy_from_the_path() {
    // The same destination name gets four different physical results.
    let fixture = Fixture::new();
    let path = fixture.path();
    let mask = measured_umask();

    write_source(atomic(), &path, SourcePayload::Bytes(b"a"), None).expect("atomic");
    assert_eq!(mode_of(&path), 0o600);

    write_source(
        WriterOperation::TextTruncate,
        &path,
        SourcePayload::Bytes(b"b"),
        None,
    )
    .expect("text");
    assert_eq!(mode_of(&path), 0o600, "text keeps the existing mode");

    write_source(
        WriterOperation::generated(true),
        &path,
        SourcePayload::Bytes(b"c"),
        None,
    )
    .expect("generated");
    assert_eq!(mode_of(&path), (0o666 & !mask) & !0o222);

    write_source(
        WriterOperation::DerivedJson,
        &path,
        SourcePayload::Bytes(b"d"),
        None,
    )
    .expect("derived");
    assert_eq!(mode_of(&path), 0o666 & !mask);
    assert_eq!(fs::read(&path).expect("read"), b"d");
}

#[test]
fn operation_metadata_names_the_source_function() {
    assert_eq!(
        WriterOperation::atomic().source_function(),
        "kernel.atomic.atomic_write"
    );
    assert_eq!(
        WriterOperation::generated(true).source_function(),
        "specify_cli.runtime.generated_writer.write_generated_file"
    );
    assert_eq!(
        WriterOperation::TextTruncate.source_function(),
        "pathlib.Path.write_text"
    );
    assert_eq!(
        WriterOperation::DerivedJson.source_function(),
        "specify_cli.status.views._atomic_write_json"
    );
    assert_eq!(
        WriterOperation::RebuildLog.source_function(),
        "specify_cli.migration.rebuild_state.rebuild_event_log"
    );
    assert!(!WriterOperation::RebuildLog.notifies_observer());
    assert!(!WriterOperation::TextTruncate.uses_temporary());
    assert!(WriterOperation::atomic().uses_temporary());
    assert!(!WriterOperation::Generated(GeneratedWrite { read_only: true }).notifies_observer());
    assert!(WriterOperation::Atomic(AtomicWrite::default()).notifies_observer());
}

#[test]
fn encoder_is_the_one_strict_authority_for_all_text_payloads() {
    let fixture = Fixture::new();
    let path = fixture.path();
    write_source(
        WriterOperation::TextTruncate,
        &path,
        SourcePayload::Text("héllo ☃"),
        None,
    )
    .expect("write");
    assert_eq!(
        fs::read(&path).expect("read"),
        "héllo ☃".as_bytes(),
        "no BOM, no newline translation, no replacement"
    );
}

// ---------------------------------------------------------------------------
// Canonical ambient write-observer context (W2): dispatch and receipt states
// ---------------------------------------------------------------------------

#[test]
fn ambient_context_is_notified_for_a_none_call_and_before_an_explicit_listener() {
    use sk_kernel::source_writer::write_observer::{
        WriteContext, observe_writes, stop_observing_writes,
    };
    use std::sync::{Arc, Mutex};

    let fixture = Fixture::new();
    let path = fixture.path();
    let context = WriteContext::new();
    let recorded = Arc::new(Mutex::new(Vec::<(PathBuf, Vec<u8>)>::new()));
    let guard = context.enter().expect("context entry");
    let sink = Arc::clone(&recorded);
    let token = observe_writes(move |path: &Path, bytes: &[u8]| {
        sink.lock()
            .expect("sink")
            .push((path.to_path_buf(), bytes.to_vec()));
        Ok(())
    });

    // No per-call listener: the canonical ambient context still hears the write.
    let receipt =
        write_source(atomic(), &path, SourcePayload::Bytes(b"first"), None).expect("write");
    assert_eq!(receipt.observation, WriteObservation::Notified);
    assert_eq!(
        *recorded.lock().expect("sink"),
        vec![(path.clone(), b"first".to_vec())]
    );

    // Both capabilities: ambient outer first, then the explicit per-call listener.
    recorded.lock().expect("sink").clear();
    let mut listener = Recorder::default();
    let receipt = write_source(
        atomic(),
        &path,
        SourcePayload::Bytes(b"second"),
        Some(&mut listener),
    )
    .expect("write");
    assert_eq!(receipt.observation, WriteObservation::Notified);
    assert_eq!(
        *recorded.lock().expect("sink"),
        vec![(path.clone(), b"second".to_vec())]
    );
    assert_eq!(listener.calls, vec![(path.clone(), b"second".to_vec())]);

    stop_observing_writes(&token).expect("stop ambient observer");
    drop(guard);

    // Outside the explicit context there is no listener again.
    let receipt =
        write_source(atomic(), &path, SourcePayload::Bytes(b"third"), None).expect("write");
    assert_eq!(receipt.observation, WriteObservation::NoListener);
}

#[test]
fn ambient_observer_os_error_is_classified_and_propagates_after_publication() {
    use sk_kernel::source_writer::write_observer::{
        WriteContext, WriteObserverError, observe_writes, stop_observing_writes,
    };

    let fixture = Fixture::new();
    let path = fixture.path();
    fixture.seed(&path, 0o444, b"old");
    let context = WriteContext::new();
    let guard = context.enter().expect("context entry");
    let token = observe_writes(|_: &Path, _: &[u8]| {
        Err(WriteObserverError::os(std::io::Error::other("observer os")))
    });

    let error = write_source(atomic(), &path, SourcePayload::Bytes(b"new"), None)
        .expect_err("observer raised");

    assert!(error.is_os_error(), "a typed OSError stays OSError-class");
    assert!(!error.is_value_error());
    assert!(error.published_before_error());
    assert_eq!(fs::read(&path).expect("read"), b"new");
    assert_eq!(mode_of(&path), 0o444, "the mode restore already ran");
    assert!(
        error
            .observer_cause()
            .is_some_and(WriteObserverError::is_os_error)
    );

    stop_observing_writes(&token).expect("stop ambient observer");
    drop(guard);
}
