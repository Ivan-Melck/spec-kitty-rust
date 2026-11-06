//! Source's sparse scanner checks exists before its ordinary text read.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use sk_git::{GitError, GitRepository};

fn fixture() -> (tempfile::TempDir, GitRepository, PathBuf) {
    let temporary = tempfile::tempdir().unwrap();
    let output = Command::new("git")
        .args(["init", "-q"])
        .arg(temporary.path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let repository = GitRepository::open(temporary.path()).unwrap();
    let pattern = temporary.path().join(".git/info/sparse-checkout");
    (temporary, repository, pattern)
}

#[test]
fn missing_and_regular_source_sparse_files() {
    let (_temporary, repository, pattern) = fixture();
    assert_eq!(repository.sparse_pattern_file_text_source().unwrap(), None);
    fs::write(&pattern, b"/*\r\n!x\ry\n").unwrap();
    assert_eq!(
        repository
            .sparse_pattern_file_text_source()
            .unwrap()
            .as_deref(),
        Some("/*\n!x\ny\n")
    );
}

#[test]
fn a_non_directory_parent_is_absent_in_the_exists_phase() {
    let (_temporary, repository, pattern) = fixture();
    let parent = pattern.parent().unwrap();
    fs::rename(parent, parent.with_extension("saved")).unwrap();
    fs::write(parent, b"not a directory").unwrap();
    assert_eq!(repository.sparse_pattern_file_text_source().unwrap(), None);
}

#[cfg(unix)]
#[test]
fn a_symlink_loop_is_absent_in_the_exists_phase() {
    let (_temporary, repository, pattern) = fixture();
    std::os::unix::fs::symlink("sparse-checkout", pattern).unwrap();
    assert_eq!(repository.sparse_pattern_file_text_source().unwrap(), None);
}

#[test]
fn an_existing_directory_still_fails_at_the_read_phase() {
    let (_temporary, repository, pattern) = fixture();
    fs::create_dir(pattern).unwrap();
    assert!(matches!(
        repository.sparse_pattern_file_text_source(),
        Err(GitError::Io {
            context: "read the sparse-checkout pattern file (Source read)",
            ..
        })
    ));
}

#[test]
fn invalid_text_still_fails_at_the_decode_phase() {
    let (_temporary, repository, pattern) = fixture();
    fs::write(pattern, b"\xff").unwrap();
    assert!(matches!(
        repository.sparse_pattern_file_text_source(),
        Err(GitError::NonUtf8 { .. })
    ));
}
