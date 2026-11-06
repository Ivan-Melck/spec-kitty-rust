//! Differential parity for anchored lock names and the status families.
//!
//! The oracle is the pinned upstream Python interpreter, imported from this
//! checkout's `src/` tree. These tests are development oracle tests, not a
//! runtime dependency: the Rust kernel never invokes Python.
//!
//! Run with:
//! `CARGO_BUILD_JOBS=2 cargo test -p sk-kernel --test lock_path_parity`

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use sk_kernel::lock_families::{
    checkout_claim_lock_path, feature_status_lock_path, project_event_log_lock_path,
};
use sk_kernel::locks::{
    AnchorDigestAlgorithm, AnchoredLockName, anchored_lock_path, anchored_lock_path_with,
};

#[path = "support/oracle.rs"]
mod oracle_support;

fn python() -> Option<PathBuf> {
    // Missing or drifted oracles panic; None must never certify zero comparisons.
    Some(oracle_support::python())
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("workspace root")
}

fn python_paths(root: &Path, common: &Path, checkout: &Path, anchor: &Path) -> Option<Value> {
    let python = python()?;
    let script = r#"
import json, sys
from pathlib import Path
from kernel.locks import LOCK_DIRECTORY, anchored_lock_path

root, common, checkout, anchor = map(Path, sys.argv[1:5])
out = {}
out["default"] = str(anchored_lock_path(anchor, common_dir=common))
out["subdirectory"] = str(anchored_lock_path(anchor, common_dir=common, subdirectory="event-logs"))
out["checkout"] = str(anchored_lock_path(
    checkout,
    common_dir=common,
    prefix="__checkout-",
    suffix="__.status.lock",
    digest_algorithm="sha1",
    digest_hex_length=16,
))
out["lengths"] = [
    str(anchored_lock_path(anchor, common_dir=common, digest_hex_length=length))
    for length in (1, 16, 32, 40, 64)
]
out["status"] = str(common / LOCK_DIRECTORY / "mission-01ABCDEF.status.lock")
out["project"] = str(common / LOCK_DIRECTORY / "__project__.status.lock")
out["checkout_formula"] = str(
    common / LOCK_DIRECTORY
    / f"__checkout-{__import__('hashlib').sha1(__import__('os').path.normcase(str(checkout.resolve())).encode(), usedforsecurity=False).hexdigest()[:16]}__.status.lock"
)
print(json.dumps(out))
"#;
    let output = Command::new(python)
        .arg("-c")
        .arg(script)
        .arg(root)
        .arg(common)
        .arg(checkout)
        .arg(anchor)
        .env("PYTHONPATH", repo_root().join("src"))
        .output()
        .expect("run pinned python");
    assert!(
        output.status.success(),
        "python oracle failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Some(serde_json::from_slice(&output.stdout).expect("python JSON"))
}

#[test]
fn anchored_and_family_lock_names_match_the_python_baseline() {
    let Some(_) = python() else {
        eprintln!("pinned Python oracle is not installed; skipping differential lock-path test");
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let repo = root.join("repo");
    let checkout = root.join("checkout");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&checkout).unwrap();
    std::fs::write(repo.join("log.jsonl"), b"").unwrap();
    let common = root.join(".git");
    std::fs::create_dir_all(&common).unwrap();

    let anchor = repo.join("log.jsonl");
    #[cfg(unix)]
    let symlink_anchor = {
        let link = root.join("link.jsonl");
        std::os::unix::fs::symlink(&anchor, &link).unwrap();
        link
    };
    #[cfg(not(unix))]
    let symlink_anchor = anchor.clone();

    let expected = python_paths(root, &common, &checkout, &symlink_anchor).expect("oracle");

    let mut lengths = Vec::new();
    for length in [1_usize, 16, 32, 40, 64] {
        lengths.push(
            anchored_lock_path_with(
                &symlink_anchor,
                &common,
                None,
                &AnchoredLockName::new().with_digest_hex_length(length),
            )
            .unwrap()
            .display()
            .to_string(),
        );
    }
    let actual = serde_json::json!({
        "default": anchored_lock_path(&symlink_anchor, &common, None).unwrap().display().to_string(),
        "subdirectory": anchored_lock_path(&symlink_anchor, &common, Some(Path::new("event-logs"))).unwrap().display().to_string(),
        "checkout": checkout_claim_lock_path(&checkout, &common).unwrap().display().to_string(),
        "lengths": lengths,
        "status": feature_status_lock_path(&common, "mission-01ABCDEF").unwrap().display().to_string(),
        "project": project_event_log_lock_path(&common).display().to_string(),
        "checkout_formula": checkout_claim_lock_path(&checkout, &common).unwrap().display().to_string(),
    });

    assert_eq!(actual, expected);
}

#[test]
fn status_family_paths_and_common_dir_degrade_match_python() {
    let Some(_) = python() else {
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let status = Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(&repo)
        .status()
        .unwrap();
    assert!(status.success());
    let non_git_kittify = temp.path().join("kittify-project");
    std::fs::create_dir_all(non_git_kittify.join(".kittify")).unwrap();
    let bare_tree = temp.path().join("bare-tree");
    std::fs::create_dir_all(&bare_tree).unwrap();

    let script = r#"
import json, sys
from pathlib import Path
from specify_cli.status.locking import (
    LOCK_DIRECTORY,
    _checkout_claim_lock_path,
    _git_common_dir,
    feature_status_lock_path,
)
repo, kittify, bare = map(Path, sys.argv[1:4])
out = {}
for name, path in (("repo", repo), ("kittify", kittify), ("bare", bare)):
    out[name] = str(_git_common_dir(path))
out["feature"] = str(feature_status_lock_path(repo, "mission-01ABCDEF"))
out["checkout"] = str(_checkout_claim_lock_path(repo))
out["project"] = str(_git_common_dir(repo) / LOCK_DIRECTORY / "__project__.status.lock")
print(json.dumps(out, sort_keys=True))
"#;
    let output = Command::new(python().unwrap())
        .arg("-c")
        .arg(script)
        .arg(&repo)
        .arg(&non_git_kittify)
        .arg(&bare_tree)
        .env("PYTHONPATH", repo_root().join("src"))
        .current_dir(repo_root())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let expected: Value = serde_json::from_slice(&output.stdout).unwrap();

    let repo_common = sk_kernel::lock_families::status_lock_common_dir(&repo).unwrap();
    let actual = serde_json::json!({
        "repo": repo_common.display().to_string(),
        "kittify": sk_kernel::lock_families::status_lock_common_dir(&non_git_kittify).unwrap().display().to_string(),
        "bare": sk_kernel::lock_families::status_lock_common_dir(&bare_tree).unwrap().display().to_string(),
        "feature": feature_status_lock_path(&repo_common, "mission-01ABCDEF").unwrap().display().to_string(),
        "checkout": checkout_claim_lock_path(&repo, &repo_common).unwrap().display().to_string(),
        "project": project_event_log_lock_path(&repo_common).display().to_string(),
    });
    assert_eq!(actual, expected);
}

#[test]
fn sha1_checkout_name_is_the_historical_shape() {
    let temp = tempfile::tempdir().unwrap();
    let path = checkout_claim_lock_path(temp.path(), Path::new("/repo/.git")).unwrap();
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.starts_with("__checkout-"));
    assert!(name.ends_with("__.status.lock"));
    assert_eq!(
        name.len(),
        "__checkout-".len() + 16 + "__.status.lock".len()
    );
    assert_eq!(
        anchored_lock_path_with(
            temp.path(),
            Path::new("/repo/.git"),
            None,
            &AnchoredLockName::new()
                .with_prefix("__checkout-")
                .with_suffix("__.status.lock")
                .with_sha1()
                .with_digest_hex_length(16),
        )
        .unwrap(),
        checkout_claim_lock_path(temp.path(), Path::new("/repo/.git")).unwrap()
    );
    assert_eq!(
        checkout_claim_lock_path(temp.path(), Path::new("/repo/.git"))
            .unwrap()
            .extension()
            .unwrap(),
        "lock"
    );
    let _ = AnchorDigestAlgorithm::Sha1;
}
