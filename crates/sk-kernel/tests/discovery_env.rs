//! Environment-sensitive discovery tests.
//!
//! `std::env::set_var` is unsafe in edition 2024, and this workspace forbids
//! `unsafe_code`, so the cases run in a child process of this test binary with
//! a controlled environment instead of mutating the parent's environment.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sk_kernel::{KernelError, discover_project_root};
use tempfile::TempDir;

const CASE_VAR: &str = "SK_KERNEL_DISCOVERY_CASE";
const START_VAR: &str = "SK_KERNEL_DISCOVERY_START";
const EXPECT_VAR: &str = "SK_KERNEL_DISCOVERY_EXPECT";

fn temp() -> TempDir {
    tempfile::tempdir().expect("temporary directory")
}

fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).expect("canonical path")
}

fn write_git_head(repo: &Path) {
    fs::create_dir_all(repo.join(".git")).expect("mkdir .git");
    fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").expect("write HEAD");
}

fn run_case(case: &str, start: &Path, expected: &Path, environment: &[(&str, OsString)]) -> Output {
    let mut command = Command::new(std::env::current_exe().expect("current test binary"));
    command
        .args(["--exact", "env_discovery_child", "--nocapture"])
        .env_remove("SPECIFY_REPO_ROOT")
        .env_remove("GIT_CEILING_DIRECTORIES")
        .env(CASE_VAR, case)
        .env(START_VAR, start)
        .env(EXPECT_VAR, expected);

    for (name, value) in environment {
        command.env(name, value);
    }

    command.output().expect("run child discovery test")
}

fn assert_child_passed(output: &Output) {
    assert!(
        output.status.success(),
        "child discovery test failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn env_discovery_child() {
    let Some(case) = std::env::var_os(CASE_VAR) else {
        return;
    };
    let start = PathBuf::from(std::env::var_os(START_VAR).expect("start path"));
    let expected = PathBuf::from(std::env::var_os(EXPECT_VAR).expect("expected path"));

    match case.to_string_lossy().as_ref() {
        "env_root_wins_without_marker"
        | "env_root_takes_precedence"
        | "env_worktree_pointer"
        | "env_missing_falls_through"
        | "env_file_falls_through"
        | "ceiling_project_inside"
        | "relative_ceiling_ignored" => {
            assert_eq!(discover_project_root(&start).unwrap(), canonical(&expected));
        }
        "ceiling_stops_walk" => match discover_project_root(&start) {
            Err(KernelError::ProjectRootNotFound { .. }) => {}
            other => panic!("expected ProjectRootNotFound, got {other:?}"),
        },
        other => panic!("unknown child discovery case: {other}"),
    }
}

#[test]
fn existing_env_root_is_authoritative_even_without_kittify() {
    let temp = temp();
    let ambient = temp.path().join("ambient");
    let explicit = temp.path().join("explicit");
    fs::create_dir_all(ambient.join(".kittify")).expect("ambient marker");
    write_git_head(&ambient);
    fs::create_dir_all(&explicit).expect("explicit root");

    let output = run_case(
        "env_root_wins_without_marker",
        &ambient,
        &explicit,
        &[("SPECIFY_REPO_ROOT", explicit.clone().into_os_string())],
    );
    assert_child_passed(&output);
}

#[test]
fn env_root_beats_ambient_kittify_project() {
    let temp = temp();
    let ambient = temp.path().join("ambient");
    let explicit = temp.path().join("explicit");
    fs::create_dir_all(ambient.join(".kittify")).expect("ambient marker");
    write_git_head(&ambient);
    fs::create_dir_all(explicit.join(".kittify")).expect("explicit marker");
    write_git_head(&explicit);

    let output = run_case(
        "env_root_takes_precedence",
        &ambient,
        &explicit,
        &[("SPECIFY_REPO_ROOT", explicit.clone().into_os_string())],
    );
    assert_child_passed(&output);
}

#[test]
fn missing_env_root_falls_through() {
    let temp = temp();
    let ambient = temp.path().join("ambient");
    let missing = temp.path().join("missing");
    fs::create_dir_all(ambient.join(".kittify")).expect("ambient marker");
    write_git_head(&ambient);

    let output = run_case(
        "env_missing_falls_through",
        &ambient,
        &ambient,
        &[("SPECIFY_REPO_ROOT", missing.into_os_string())],
    );
    assert_child_passed(&output);
}

#[test]
fn file_valued_env_root_falls_through() {
    let temp = temp();
    let ambient = temp.path().join("ambient");
    let marker = temp.path().join("not-a-directory");
    fs::create_dir_all(ambient.join(".kittify")).expect("ambient marker");
    write_git_head(&ambient);
    fs::write(&marker, "not a root\n").expect("marker file");

    let output = run_case(
        "env_file_falls_through",
        &ambient,
        &ambient,
        &[("SPECIFY_REPO_ROOT", marker.into_os_string())],
    );
    assert_child_passed(&output);
}

#[test]
fn env_root_worktree_pointer_resolves_primary_without_marker() {
    let temp = temp();
    let primary = temp.path().join("primary");
    let worktree = temp.path().join("worktrees/feature");
    fs::create_dir_all(primary.join(".git/worktrees/feature")).expect("worktree gitdir");
    fs::create_dir_all(&worktree).expect("worktree");
    fs::write(
        worktree.join(".git"),
        format!(
            "gitdir: {}\n",
            primary.join(".git/worktrees/feature").display()
        ),
    )
    .expect("worktree pointer");

    let output = run_case(
        "env_worktree_pointer",
        &worktree,
        &primary,
        &[("SPECIFY_REPO_ROOT", worktree.clone().into_os_string())],
    );
    assert_child_passed(&output);
}

#[test]
fn ceiling_stops_the_walk_above_it() {
    let temp = temp();
    let ambient = temp.path().join("ambient");
    let ceiling = ambient.join("ceiling");
    let start = ceiling.join("sandbox/deep");
    write_git_head(&ambient);
    fs::create_dir_all(&start).expect("start");

    let output = run_case(
        "ceiling_stops_walk",
        &start,
        &start,
        &[("GIT_CEILING_DIRECTORIES", ceiling.into_os_string())],
    );
    assert_child_passed(&output);
}

#[test]
fn ceiling_still_allows_a_project_inside_it() {
    let temp = temp();
    let ambient = temp.path().join("ambient");
    let ceiling = ambient.join("ceiling");
    let start = ceiling.join("sandbox/deep");
    write_git_head(&ambient);
    fs::create_dir_all(ceiling.join(".kittify")).expect("ceiling marker");
    fs::create_dir_all(&start).expect("start");

    let output = run_case(
        "ceiling_project_inside",
        &start,
        &ceiling,
        &[("GIT_CEILING_DIRECTORIES", ceiling.clone().into_os_string())],
    );
    assert_child_passed(&output);
}

#[test]
fn relative_ceiling_entries_are_ignored_like_git() {
    let temp = temp();
    let ambient = temp.path().join("ambient");
    let start = ambient.join("nested/deep");
    write_git_head(&ambient);
    fs::create_dir_all(&start).expect("start");

    let output = run_case(
        "relative_ceiling_ignored",
        &start,
        &ambient,
        &[("GIT_CEILING_DIRECTORIES", OsString::from("."))],
    );
    assert_child_passed(&output);
}
