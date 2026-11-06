//! `lanes.json` lock authority (review finding R4).
//!
//! The reference locks a manifest at
//! `<git-common-dir>/spec-kitty-lanes-locks/<rel>/lanes.json.lock`
//! (`specify_cli.lanes.persistence.lanes_json_lock`). A native store that
//! instead locks `<mission_dir>/.lanes.json.lock` shares nothing with the
//! reference writer and leaves an untracked file inside the mission directory,
//! which later makes a `fast_forward`/rollback refuse the checkout as dirty.
//!
//! These tests bind the native path to the reference's own computed path, and
//! prove real cross-language contention in **both** directions over the same
//! lock file.

mod support;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use sk_kernel::FileLock;
use sk_workspace::{LANES_FILE_NAME, LanesStore, lanes_json_lock_path};

use support::{commit_all, init_repo, write};

#[path = "support/oracle.rs"]
mod oracle_support;

/// The reference's own `lanes_json_lock(...).lock_path` for `feature_dir`.
fn oracle_lanes_lock_path(feature_dir: &Path) -> String {
    oracle_support::run(
        r#"
import sys
from pathlib import Path
from specify_cli.lanes.persistence import lanes_json_lock

print(lanes_json_lock(Path(sys.argv[1])).lock_path)
"#,
        &[feature_dir.to_str().unwrap()],
    )
}

/// Compare non-Git authority with the SAME isolated TMPDIR in both processes.
/// Source's tempfile.gettempdir() is an input, not a value to normalize away.
fn paired_non_git_lanes_lock_path(feature_dir: &Path) -> PathBuf {
    let sandbox = tempfile::tempdir().unwrap();
    for name in ["cwd", "home", "tmp", "cache", "config", "data", "state"] {
        std::fs::create_dir_all(sandbox.path().join(name)).unwrap();
    }
    let environment = [
        ("HOME", "home"),
        ("TMPDIR", "tmp"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_STATE_HOME", "state"),
    ];
    let mut source = Command::new(oracle_support::python());
    source.arg("-c").arg(r#"
import os,sys,tempfile
from pathlib import Path
from specify_cli.lanes.persistence import _non_git_lanes_lock_path, resolve_lanes_dir
base=Path(os.environ['HOME']).parent
assert Path.cwd()==base/'cwd'
assert all(Path(os.environ[k]).parent==base for k in ['HOME','TMPDIR','XDG_CACHE_HOME','XDG_CONFIG_HOME','XDG_DATA_HOME','XDG_STATE_HOME'])
root=Path(os.fsdecode(bytes.fromhex(sys.argv[1])))
result=_non_git_lanes_lock_path(resolve_lanes_dir(root).resolve())
assert result.parent==Path(tempfile.gettempdir())/'spec-kitty-lanes-locks'
print(result)
"#).arg(feature_dir.as_os_str().as_encoded_bytes().iter().map(|b| format!("{b:02x}")).collect::<String>());
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    source
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("PYTHONPATH", repo.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .current_dir(sandbox.path().join("cwd"));
    for (key, name) in environment {
        source.env(key, sandbox.path().join(name));
    }
    let expected = source.output().unwrap();
    assert!(
        expected.status.success(),
        "Source failed: {}",
        String::from_utf8_lossy(&expected.stderr)
    );
    let expected = PathBuf::from(String::from_utf8(expected.stdout).unwrap().trim());
    assert!(expected.is_absolute());
    let output_path = sandbox.path().join("native-path");
    let mut native = Command::new(std::env::current_exe().unwrap());
    native
        .args([
            "--exact",
            "non_git_lock_path_helper",
            "--ignored",
            "--nocapture",
        ])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("SK_NATIVE_FEATURE_DIR", feature_dir)
        .env("SK_NATIVE_LOCK_PATH_OUT", &output_path)
        .current_dir(sandbox.path().join("cwd"));
    for (key, name) in environment {
        native.env(key, sandbox.path().join(name));
    }
    let actual = native.output().unwrap();
    assert!(
        actual.status.success(),
        "Native failed: {}\n{}",
        String::from_utf8_lossy(&actual.stdout),
        String::from_utf8_lossy(&actual.stderr)
    );
    let actual = PathBuf::from(std::fs::read_to_string(output_path).unwrap());
    assert_eq!(
        actual, expected,
        "exact Source/native non-Git lock authority"
    );
    actual
}

#[test]
#[ignore = "Source-first parent tests exercise this with matching isolated roots"]
fn non_git_lock_path_helper() {
    let base = PathBuf::from(std::env::var_os("HOME").unwrap())
        .parent()
        .unwrap()
        .to_path_buf();
    assert_eq!(std::env::current_dir().unwrap(), base.join("cwd"));
    for key in [
        "HOME",
        "TMPDIR",
        "XDG_CACHE_HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
    ] {
        assert_eq!(
            PathBuf::from(std::env::var_os(key).unwrap()).parent(),
            Some(base.as_path())
        );
    }
    let feature = PathBuf::from(std::env::var_os("SK_NATIVE_FEATURE_DIR").unwrap());
    let path = lanes_json_lock_path(&feature).unwrap();
    std::fs::write(
        std::env::var_os("SK_NATIVE_LOCK_PATH_OUT").unwrap(),
        path.to_str().unwrap(),
    )
    .unwrap();
}

fn manifest_json() -> serde_json::Value {
    serde_json::json!({
        "version": 1,
        "mission_slug": "demo",
        "mission_id": "01KNXQS9ATWWFXS3K5ZJ9E5001",
        "mission_branch": "kitty/mission-demo",
        "target_branch": "main",
        "lanes": [],
        "computed_at": "2026-11-02T00:00:00Z",
        "computed_from": "fixture",
        "planning_artifact_wps": [],
        "planning_commit_sha": null,
    })
}

fn manifest() -> sk_workspace::LanesManifest {
    serde_json::from_value(manifest_json()).unwrap()
}

#[test]
fn the_lanes_lock_is_the_reference_common_dir_path() {
    let fixture = init_repo();
    let feature_dir = fixture.root().join("kitty-specs").join("demo");
    write(
        &feature_dir.join(LANES_FILE_NAME),
        &format!("{}\n", manifest_json()),
    );
    commit_all(fixture.root(), "scaffold");

    let expected = fixture
        .root()
        .join(".git")
        .join("spec-kitty-lanes-locks")
        .join("kitty-specs")
        .join("demo")
        .join("lanes.json.lock");
    assert_eq!(lanes_json_lock_path(&feature_dir).unwrap(), expected);
    assert_eq!(
        oracle_lanes_lock_path(&feature_dir),
        expected.to_str().unwrap(),
        "the native lock path is the reference's own path"
    );
    let store = LanesStore::new(&feature_dir);
    assert_eq!(store.lock_path().unwrap(), expected);
}

#[test]
fn a_lane_worktree_locks_the_same_manifest_the_primary_checkout_does() {
    // Cross-worktree authority: `lanes.json` is written from the primary
    // checkout and from a lane worktree; both must name one lock in the shared
    // common dir, or a mixed pair of writers loses an update.
    let fixture = init_repo();
    let feature_dir = fixture.root().join("kitty-specs").join("demo");
    write(
        &feature_dir.join(LANES_FILE_NAME),
        &format!("{}\n", manifest_json()),
    );
    commit_all(fixture.root(), "scaffold");
    let worktree = fixture.root().join(".worktrees").join("demo-lane-a");
    support::git(
        fixture.root(),
        &[
            "worktree",
            "add",
            "-b",
            "kitty/mission-demo-lane-a",
            worktree.to_str().unwrap(),
        ],
    );
    let from_worktree = lanes_json_lock_path(&worktree.join("kitty-specs").join("demo")).unwrap();
    assert_eq!(from_worktree, lanes_json_lock_path(&feature_dir).unwrap());
    assert_eq!(
        from_worktree,
        fixture
            .root()
            .join(".git")
            .join("spec-kitty-lanes-locks")
            .join("kitty-specs")
            .join("demo")
            .join("lanes.json.lock")
    );
}

#[test]
fn a_non_git_project_locks_the_reference_temp_root_name() {
    let temp = tempfile::TempDir::new().unwrap();
    let feature_dir = temp.path().join("café").join("demo");
    std::fs::create_dir_all(&feature_dir).unwrap();
    write(
        &feature_dir.join(LANES_FILE_NAME),
        &format!("{}\n", manifest_json()),
    );
    let native = paired_non_git_lanes_lock_path(&feature_dir);
    assert_eq!(
        native.parent().unwrap().file_name().unwrap(),
        "spec-kitty-lanes-locks"
    );
    assert_eq!(native.extension().unwrap(), "lock");
    // Full path and digest were compared before returning the paired result.
}

#[cfg(unix)]
#[test]
fn non_git_lock_identity_preserves_undecodable_path_bytes() {
    use std::os::unix::ffi::OsStringExt;
    let temp = tempfile::tempdir().unwrap();
    let name = std::ffi::OsString::from_vec(b"mission-\xff-\xc3\xa9-\xed\xa0\x80".to_vec());
    let feature_dir = temp.path().join(name);
    std::fs::create_dir_all(&feature_dir).unwrap();
    paired_non_git_lanes_lock_path(&feature_dir);
}

#[test]
fn a_manifest_outside_its_worktree_is_refused_not_locked_in_tree() {
    let fixture = init_repo();
    let feature_dir = fixture.root().join("kitty-specs").join("demo");
    std::fs::create_dir_all(&feature_dir).unwrap();
    let outside = fixture
        .root()
        .parent()
        .unwrap()
        .join(format!("sk-lanes-outside-{}.json", std::process::id()));
    write(&outside, &format!("{}\n", manifest_json()));
    std::os::unix::fs::symlink(&outside, feature_dir.join(LANES_FILE_NAME)).unwrap();
    let error = lanes_json_lock_path(&feature_dir).expect_err("refusal");
    assert!(
        matches!(
            error,
            sk_workspace::WorkspaceError::CoordRefused {
                code: "LANES_LOCK_OUTSIDE_WORKTREE",
                ..
            }
        ),
        "{error}"
    );
    let _ = std::fs::remove_file(&outside);
}

#[test]
fn a_store_write_leaves_the_checkout_clean_and_no_in_tree_lock() {
    let fixture = init_repo();
    let feature_dir = fixture.root().join("kitty-specs").join("demo");
    std::fs::create_dir_all(&feature_dir).unwrap();
    let store = LanesStore::new(&feature_dir);
    let path = store.write(&manifest()).unwrap();
    commit_all(fixture.root(), "lanes");
    // A second write of the same content must not dirty the checkout: the lock
    // lives under the Git common dir, never beside the manifest.
    store.update(|current| Ok(current.clone())).unwrap();
    assert_eq!(
        support::git(fixture.root(), &["status", "--porcelain"]),
        "",
        "the canonical lock leaves the worktree clean"
    );
    assert!(
        !feature_dir.join(".lanes.json.lock").exists(),
        "no in-tree lock residue: {}",
        path.display()
    );
    assert!(store.lock_path().unwrap().exists());
}

#[test]
fn a_python_writer_holds_the_lock_the_native_writer_waits_for() {
    let fixture = init_repo();
    let feature_dir = fixture.root().join("kitty-specs").join("demo");
    std::fs::create_dir_all(&feature_dir).unwrap();
    let store = LanesStore::new(&feature_dir);
    store.write(&manifest()).unwrap();
    commit_all(fixture.root(), "lanes");
    let ready = fixture.root().join("python-ready");
    let released = fixture.root().join("python-released");
    let script = r#"
import sys, time
from pathlib import Path
from specify_cli.lanes.persistence import lanes_json_lock

feature_dir, ready, released = Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3])
with lanes_json_lock(feature_dir):
    ready.write_text("ready")
    time.sleep(1.5)
    # Written INSIDE the hold: the native writer can only have made progress
    # after the release if this file already exists when it returns.
    released.write_text("released")
"#;
    let child = Command::new(oracle_support::python())
        .arg("-c")
        .arg(script)
        .arg(&feature_dir)
        .arg(&ready)
        .arg(&released)
        .env("PYTHONPATH", {
            let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            root.join("src")
        })
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the reference writer");
    // Wait for the reference to actually hold it (bounded, no busy loop).
    for _ in 0..600 {
        if ready.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(ready.exists(), "the reference never took the lock");
    // This blocks until the reference releases; it can only return afterwards.
    store.update(|current| Ok(current.clone())).unwrap();
    // Observed BEFORE waiting on the child, so a non-contending native writer
    // (which would return mid-hold) fails here instead of passing on the child's
    // later exit.
    let progressed_after_release = released.exists();
    let output = child.wait_with_output().expect("wait for the reference");
    assert!(
        output.status.success(),
        "the reference writer failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        progressed_after_release,
        "the native writer made progress only after the reference released"
    );
}

#[test]
fn a_native_holder_contends_the_python_writer() {
    let fixture = init_repo();
    let feature_dir = fixture.root().join("kitty-specs").join("demo");
    write(
        &feature_dir.join(LANES_FILE_NAME),
        &format!("{}\n", manifest_json()),
    );
    commit_all(fixture.root(), "scaffold");
    let lock_path = lanes_json_lock_path(&feature_dir).unwrap();
    let script = r#"
import sys
from pathlib import Path
from kernel.locks import LockNotAcquired, machine_file_lock

try:
    with machine_file_lock(Path(sys.argv[1])):
        print("acquired")
except LockNotAcquired:
    print("contended")
"#;
    let run_python = || {
        let output = Command::new(oracle_support::python())
            .arg("-c")
            .arg(script)
            .arg(&lock_path)
            .env("PYTHONPATH", {
                let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
                root.join("src")
            })
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .output()
            .expect("run the reference contender");
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    };
    {
        let _held: FileLock = FileLock::acquire_unbounded(&lock_path).unwrap();
        assert_eq!(run_python(), "contended");
    }
    assert_eq!(run_python(), "acquired");
}
