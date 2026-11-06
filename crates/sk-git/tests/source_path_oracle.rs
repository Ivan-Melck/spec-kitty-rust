//! Actual pinned-source controls for absolute preflight path semantics.
use sk_git::{GitPath, GitRepository, source_preflight_path};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn isolated(command: &mut Command, scratch: &Path) {
    command
        .current_dir(scratch.join("cwd"))
        .env("HOME", scratch.join("home"))
        .env("XDG_CONFIG_HOME", scratch.join("config"))
        .env("XDG_CACHE_HOME", scratch.join("cache"))
        .env("XDG_DATA_HOME", scratch.join("data"))
        .env("XDG_STATE_HOME", scratch.join("state"))
        .env("TMPDIR", scratch.join("tmp"))
        .env("PYTHONUTF8", "1")
        .env("GIT_CONFIG_NOSYSTEM", "1");
}
fn source_result(scratch: &Path, repo: &Path, path: &Path) -> String {
    let project = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let python = project.join(".venv-rust-oracle").join(if cfg!(windows) {
        "Scripts/python.exe"
    } else {
        "bin/python"
    });
    assert!(python.is_file(), "required pinned oracle missing");
    let mut verify = Command::new(&python);
    isolated(&mut verify, scratch);
    let out = verify
        .arg(project.join("scripts/rust/verify_oracle.py"))
        .arg("--root")
        .arg(&project)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "exact53 verifier failed: {:?} {:?}",
        out.stdout,
        out.stderr
    );
    let mut source = Command::new(python);
    isolated(&mut source, scratch);
    let out = source.env("PYTHONPATH",project.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT",project.join("packs"))
        .arg("-c").arg(r#"
import sys
from pathlib import Path
from mission_runtime import CommitTarget
from specify_cli.core.commit_guard import GuardCapability
from specify_cli.git.commit_helpers import preflight_commit
root, path = map(Path, sys.argv[1:])
try:
 preflight_commit(repo_root=root,worktree_root=root,target=CommitTarget(ref='main'),message='path control',paths=(path,),capability=GuardCapability.TEST_MODE)
 print('pass')
except Exception as exc:
 print(type(exc).__name__)
"#).arg(repo).arg(path).output().unwrap();
    assert!(
        out.status.success(),
        "source probe failed: {:?}",
        out.stderr
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}
fn fixture() -> (tempfile::TempDir, PathBuf, GitRepository) {
    let scratch = tempfile::tempdir().unwrap();
    for name in [
        "cwd", "home", "config", "cache", "data", "state", "tmp", "repo",
    ] {
        fs::create_dir_all(scratch.path().join(name)).unwrap();
    }
    let repo = scratch.path().join("repo");
    let mut init = Command::new("git");
    isolated(&mut init, scratch.path());
    assert!(
        init.arg("-C")
            .arg(&repo)
            .args(["init", "-q", "-b", "main"])
            .status()
            .unwrap()
            .success()
    );
    let mut seed = Command::new("git");
    isolated(&mut seed, scratch.path());
    assert!(
        seed.arg("-C")
            .arg(&repo)
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "seed"
            ])
            .status()
            .unwrap()
            .success()
    );
    let repository = GitRepository::open(&repo).unwrap();
    (scratch, repo, repository)
}
fn raw(path: &Path) -> GitPath {
    GitPath::from_git_bytes(path.as_os_str().as_encoded_bytes()).unwrap()
}
#[cfg(unix)]
#[test]
fn absolute_looping_leaf_refuses_like_the_actual_source() {
    let (scratch, root, repository) = fixture();
    std::os::unix::fs::symlink("loop", root.join("loop")).unwrap();
    let path = root.join("loop");
    assert_eq!(
        source_result(scratch.path(), &root, &path),
        "SafeCommitPathLoopRefused"
    );
    assert!(
        source_preflight_path(&repository, &raw(&path)).is_err(),
        "source rejects an absolute looping leaf before protection"
    );
}
#[cfg(unix)]
#[test]
fn absolute_symlink_parent_uses_the_source_normalized_staging_path_and_policy() {
    let (scratch, root, repository) = fixture();
    fs::create_dir_all(root.join("real-records")).unwrap();
    std::os::unix::fs::symlink("real-records", root.join("alias")).unwrap();
    let path = root.join("alias/record.jsonl");
    assert_eq!(source_result(scratch.path(), &root, &path), "pass");
    let plan = sk_git::plan_source_commit_path(&repository, &raw(&path)).unwrap();
    assert_eq!(
        plan.normalized().as_os_str(),
        std::ffi::OsStr::new("real-records/record.jsonl")
    );
    fs::create_dir_all(root.join(".worktrees/records")).unwrap();
    std::os::unix::fs::symlink(".worktrees/records", root.join("policy-alias")).unwrap();
    let policy_path = root.join("policy-alias/record.jsonl");
    assert_eq!(
        source_result(scratch.path(), &root, &policy_path),
        "SafeCommitPathPolicyError"
    );
    assert!(source_preflight_path(&repository, &raw(&policy_path)).is_err());
}

#[test]
fn relative_directory_probe_value_error_is_not_a_safe_commit_refusal() {
    let (scratch, root, repository) = fixture();
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::create_dir_all(root.join("dir")).unwrap();
    let relative = Path::new("sub/../dir");
    assert_eq!(source_result(scratch.path(), &root, relative), "ValueError");
    assert!(
        matches!(
            source_preflight_path(&repository, &raw(relative)),
            Err(sk_git::SourcePathRefusal::Unreadable { .. })
        ),
        "Source GitPath.parse propagates a non-SafeCommit ValueError before probing the directory"
    );
    let missing = Path::new("sub/../missing");
    assert_eq!(source_result(scratch.path(), &root, missing), "pass");
    assert!(source_preflight_path(&repository, &raw(missing)).is_ok());
    for spelling in ["./dir", "dir//", "sub/../dir"] {
        let absolute = root.join(spelling);
        assert_eq!(
            source_result(scratch.path(), &root, &absolute),
            "SafeCommitDirectoryArgument"
        );
        assert!(matches!(
            source_preflight_path(&repository, &raw(&absolute)),
            Err(sk_git::SourcePathRefusal::Source { .. })
        ));
    }
    for spelling in ["./dir", "dir//"] {
        assert_eq!(
            source_result(scratch.path(), &root, Path::new(spelling)),
            "SafeCommitDirectoryArgument"
        );
        assert!(matches!(
            source_preflight_path(&repository, &raw(Path::new(spelling))),
            Err(sk_git::SourcePathRefusal::Source { .. })
        ));
    }
    for spelling in [".", ""] {
        assert_eq!(
            source_result(scratch.path(), &root, Path::new(spelling)),
            "ValueError"
        );
        assert!(matches!(
            source_preflight_path(&repository, &raw(Path::new(spelling))),
            Err(sk_git::SourcePathRefusal::Unreadable { .. })
        ));
    }
    let outside = scratch.path().join("outside-dir");
    fs::create_dir_all(&outside).unwrap();
    assert_eq!(source_result(scratch.path(), &root, &outside), "ValueError");
    assert!(matches!(
        source_preflight_path(&repository, &raw(&outside)),
        Err(sk_git::SourcePathRefusal::Unreadable { .. })
    ));
}

#[test]
fn absolute_missing_parent_then_dotdot_resolves_like_the_actual_source() {
    let (scratch, root, repository) = fixture();
    let path = root.join("missing/../record.jsonl");
    assert_eq!(source_result(scratch.path(), &root, &path), "pass");
    assert!(
        source_preflight_path(&repository, &raw(&path)).is_ok(),
        "source non-strict resolution collapses the missing-parent tail"
    );
}
