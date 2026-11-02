use sk_kernel::KernelError;
use sk_kernel::event_log_append::{
    EventLogAppendOptions, append_event_log_lines, event_log_lock_path,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repository(root: &Path) {
    fs::create_dir_all(root.join(".git/objects")).unwrap();
    fs::create_dir_all(root.join(".git/refs")).unwrap();
    fs::write(root.join(".git/HEAD"), b"ref: refs/heads/main\n").unwrap();
}

#[test]
fn a_partial_nested_repository_cannot_select_a_different_lock_authority() {
    let temp = tempfile::tempdir().unwrap();
    repository(temp.path());
    let nested = temp.path().join("nested");
    fs::create_dir_all(nested.join(".git")).unwrap();
    fs::write(nested.join(".git/HEAD"), b"ref: refs/heads/main\n").unwrap();
    let log = nested.join("events.jsonl");
    assert!(matches!(
        append_event_log_lines(&log, &["event"], &EventLogAppendOptions::default()),
        Err(KernelError::LockUnavailable { .. })
    ));
    assert!(!log.exists());
    assert!(!nested.join(".git/spec-kitty-locks").exists());
}

#[cfg(unix)]
#[test]
fn a_symlinked_git_directory_uses_the_same_canonical_common_directory() {
    let temp = tempfile::tempdir().unwrap();
    repository(temp.path());
    fs::rename(temp.path().join(".git"), temp.path().join(".git.real")).unwrap();
    std::os::unix::fs::symlink(".git.real", temp.path().join(".git")).unwrap();
    let lock = event_log_lock_path(&temp.path().join("events.jsonl")).unwrap();
    assert_eq!(
        lock.parent().unwrap(),
        temp.path().join(".git.real/spec-kitty-locks/event-logs")
    );
}

const ENV_CASE: &str = "SK_KERNEL_LOCK_AUTHORITY_CASE";
const ENV_ROOT: &str = "SK_KERNEL_LOCK_AUTHORITY_ROOT";

#[test]
fn env_override_child() {
    let Some(case) = std::env::var_os(ENV_CASE) else {
        return;
    };
    let root = PathBuf::from(std::env::var_os(ENV_ROOT).expect("root"));
    let log = root.join("events.jsonl");
    match case.to_string_lossy().as_ref() {
        "git_dir" => assert_eq!(
            event_log_lock_path(&log).unwrap().parent().unwrap(),
            root.join("other.git/spec-kitty-locks/event-logs")
        ),
        "git_common_dir" => assert_eq!(
            event_log_lock_path(&log).unwrap().parent().unwrap(),
            root.join("shared.git/spec-kitty-locks/event-logs")
        ),
        "git_object_directory" => assert_eq!(
            event_log_lock_path(&log).unwrap().parent().unwrap(),
            root.join(".git/spec-kitty-locks/event-logs")
        ),
        "invalid_git_dir" | "invalid_git_common_dir" => assert!(matches!(
            event_log_lock_path(&log),
            Err(KernelError::LockUnavailable { .. })
        )),
        other => panic!("unknown lock-authority child case: {other}"),
    }
}

fn run_child(case: &str, root: &Path, environment: &[(&str, PathBuf)]) -> std::process::Output {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "env_override_child", "--nocapture"])
        .env_remove("GIT_DIR")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env(ENV_CASE, case)
        .env(ENV_ROOT, root);
    for (name, value) in environment {
        command.env(name, value);
    }
    command.output().unwrap()
}

fn assert_child_passed(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn other_repository(root: &Path) {
    fs::create_dir_all(root.join("other.git/objects")).unwrap();
    fs::create_dir_all(root.join("other.git/refs")).unwrap();
    fs::write(root.join("other.git/HEAD"), b"ref: refs/heads/main\n").unwrap();
}

#[test]
fn git_dir_env_selects_the_same_authority_as_git() {
    let temp = tempfile::tempdir().unwrap();
    repository(temp.path());
    other_repository(temp.path());
    let output = run_child(
        "git_dir",
        temp.path(),
        &[("GIT_DIR", temp.path().join("other.git"))],
    );
    assert_child_passed(&output);
}

#[test]
fn git_common_dir_env_selects_the_shared_common_directory() {
    let temp = tempfile::tempdir().unwrap();
    repository(temp.path());
    other_repository(temp.path());
    fs::rename(
        temp.path().join("other.git"),
        temp.path().join("shared.git"),
    )
    .unwrap();
    let output = run_child(
        "git_common_dir",
        temp.path(),
        &[
            ("GIT_DIR", temp.path().join(".git")),
            ("GIT_COMMON_DIR", temp.path().join("shared.git")),
        ],
    );
    assert_child_passed(&output);
}

#[test]
fn git_object_directory_does_not_change_lock_authority() {
    let temp = tempfile::tempdir().unwrap();
    repository(temp.path());
    let output = run_child(
        "git_object_directory",
        temp.path(),
        &[("GIT_OBJECT_DIRECTORY", temp.path().join(".git/objects"))],
    );
    assert_child_passed(&output);
}

#[test]
fn invalid_git_environment_overrides_are_refused_not_silently_ignored() {
    let temp = tempfile::tempdir().unwrap();
    repository(temp.path());
    for case in ["invalid_git_dir", "invalid_git_common_dir"] {
        let variable = if case == "invalid_git_dir" {
            "GIT_DIR"
        } else {
            "GIT_COMMON_DIR"
        };
        let output = run_child(
            case,
            temp.path(),
            &[(variable, temp.path().join("missing.git"))],
        );
        assert_child_passed(&output);
    }
}
