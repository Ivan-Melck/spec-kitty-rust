//! Process-mask matrix for the Kernel writer operations.
//!
//! The pinned reference's 48-case writer matrix varies the process umask
//! (`0o002`, `0o027`, `0o077`) and shows that only the *creation* modes move:
//! the atomic operation is always `0600`, while the text, generated and derived
//! operations are `0666 & ~umask` (and the generated read-only operation strips
//! `0222` from that).
//!
//! `umask` is process-wide, so this file never changes the mask of the test
//! process: it re-executes itself under `sh` with a real `umask` and reads the
//! modes the child measured back from disk. That is a genuine cause, not an
//! injected mode value.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use sk_kernel::source_writer::{SourcePayload, WriterOperation, write_source};

const CHILD_DIR: &str = "SK_WRITER_UMASK_CHILD_DIR";

fn mode_of(path: &Path) -> u32 {
    fs::symlink_metadata(path)
        .expect("metadata")
        .permissions()
        .mode()
        & 0o7777
}

#[test]
fn creation_modes_follow_the_reference_for_every_umask() {
    let exe = std::env::current_exe().expect("current exe");
    let mut rows = Vec::new();
    for mask in ["002", "027", "077"] {
        let directory = tempfile::tempdir().expect("temp dir");
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!(
                "umask {mask}; exec \"$0\" --exact child_umask_matrix --nocapture"
            ))
            .arg(&exe)
            .env(CHILD_DIR, directory.path())
            .status()
            .expect("spawn child");
        assert!(status.success(), "child for umask {mask} failed");
        let record = fs::read_to_string(directory.path().join("modes")).expect("child record");
        rows.push(record.trim().to_string());
    }

    // The reference's fresh-file rows (`.h32bls` 48-case matrix): only the
    // atomic operation is mask-independent.
    assert_eq!(
        rows,
        vec![
            "0o2 atomic=0o600 text=0o664 generated_writable=0o664 generated_read_only=0o444 \
             derived=0o664 relaxed=0o664"
                .to_string(),
            "0o27 atomic=0o600 text=0o640 generated_writable=0o640 generated_read_only=0o440 \
             derived=0o640 relaxed=0o640"
                .to_string(),
            "0o77 atomic=0o600 text=0o600 generated_writable=0o600 generated_read_only=0o400 \
             derived=0o600 relaxed=0o600"
                .to_string(),
        ]
    );
}

/// The child half: runs under a real `umask` and records the modes it measured.
#[test]
fn child_umask_matrix() {
    let Ok(directory) = std::env::var(CHILD_DIR) else {
        return;
    };
    let directory = Path::new(&directory).to_path_buf();
    let write = |name: &str, operation: WriterOperation| {
        let path = directory.join(name);
        write_source(operation, &path, SourcePayload::Bytes(b"new"), None).expect(name);
        mode_of(&path)
    };

    let atomic = write("atomic", WriterOperation::atomic());
    let text = write("text", WriterOperation::TextTruncate);
    let generated_writable = write("generated_writable", WriterOperation::generated(false));
    let generated_read_only = write("generated_read_only", WriterOperation::generated(true));
    let derived = write("derived", WriterOperation::DerivedJson);

    // An existing read-only destination is relaxed with 0o222 and replaced by the
    // temporary, whose mode is the creation mode.
    let relaxed_path = directory.join("relaxed");
    fs::write(&relaxed_path, b"old").expect("seed");
    fs::set_permissions(&relaxed_path, fs::Permissions::from_mode(0o444)).expect("chmod");
    write_source(
        WriterOperation::generated(false),
        &relaxed_path,
        SourcePayload::Bytes(b"new"),
        None,
    )
    .expect("generated over read-only");
    let relaxed = mode_of(&relaxed_path);

    // Measure the mask the shell installed, rather than assuming which one ran:
    // ask the OS for 0o777 on a throwaway file and read back what it granted.
    let mask = {
        let probe = directory.join("probe");
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o777)
            .open(&probe)
            .expect("probe");
        drop(file);
        0o777 & !mode_of(&probe)
    };
    let record = format!(
        "0o{mask:o} atomic=0o{atomic:o} text=0o{text:o} generated_writable=0o{generated_writable:o} \
         generated_read_only=0o{generated_read_only:o} derived=0o{derived:o} relaxed=0o{relaxed:o}"
    );
    fs::write(directory.join("modes"), record).expect("record");
}
