//! Shared fixtures for `sk-workspace` integration tests.
//!
//! Every fixture is a real Git repository under a `tempfile::TempDir`. Nothing
//! touches the fork, a remote, or the user's global config: repositories are
//! initialised with `git init`, commit identity is set locally, and no test
//! pushes or fetches.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

/// A disposable repository fixture.
pub struct Fixture {
    _temp: TempDir,
    root: PathBuf,
}

impl Fixture {
    /// The repository root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// A path under the repository root.
    pub fn path(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.root.join(relative)
    }
}

/// Run git in `cwd` and return trimmed stdout, panicking on failure.
pub fn git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(cwd)
        // Isolate the fixture from the user's environment and hooks.
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Spec Kitty Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Spec Kitty Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("failed to run git {args:?}: {error}"));
    assert!(
        output.status.success(),
        "git {args:?} failed in {}:\nstdout: {}\nstderr: {}",
        cwd.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// Initialise a repository on `main` with one commit.
pub fn init_repo() -> Fixture {
    let temp = TempDir::new().expect("temp dir");
    let root = temp.path().to_path_buf();
    git(&root, &["init", "-b", "main"]);
    git(&root, &["config", "user.name", "Spec Kitty Test"]);
    git(&root, &["config", "user.email", "test@example.invalid"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    write(&root.join("README.md"), "# fixture\n");
    git(&root, &["add", "README.md"]);
    git(&root, &["commit", "-m", "initial"]);
    Fixture { _temp: temp, root }
}

/// Write a file, creating parent directories.
pub fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent dir");
    }
    std::fs::write(path, contents).expect("write file");
}

/// Commit every change in `cwd`.
pub fn commit_all(cwd: &Path, message: &str) {
    git(cwd, &["add", "-A"]);
    git(cwd, &["commit", "-m", message]);
}

/// Commit one file (and only that file) in `cwd`.
pub fn commit_file(cwd: &Path, file: &str, message: &str) {
    git(cwd, &["add", "--", file]);
    git(cwd, &["commit", "-m", message]);
}

/// A valid 26-character ULID-shaped mission id with the given numeric tail.
pub fn mission_id(tail: u8) -> String {
    format!("01KNXQS9ATWWFXS3K5ZJ9E50{tail:02}")
}

/// Write a mission manifest under `<root>/kitty-specs/<slug>/meta.json`.
pub fn write_mission(
    root: &Path,
    slug: &str,
    mission_id: &str,
    topology: &str,
    target_branch: &str,
) -> PathBuf {
    let dir = root.join("kitty-specs").join(slug);
    let meta = serde_json::json!({
        "mission_id": mission_id,
        "mission_slug": slug,
        "topology": topology,
        "target_branch": target_branch,
        "mission_type": "software-dev",
    });
    write(
        &dir.join("meta.json"),
        &format!("{}\n", serde_json::to_string_pretty(&meta).unwrap()),
    );
    dir
}

/// The current tip of `branch`.
pub fn branch_tip(root: &Path, branch: &str) -> String {
    git(root, &["rev-parse", &format!("refs/heads/{branch}")])
}

/// Write a mission manifest with extra `meta.json` keys.
pub fn write_mission_with(
    root: &Path,
    slug: &str,
    mission_id: &str,
    topology: &str,
    target_branch: &str,
    extra: serde_json::Value,
) -> PathBuf {
    let dir = root.join("kitty-specs").join(slug);
    let mut meta = serde_json::json!({
        "mission_id": mission_id,
        "mission_slug": slug,
        "topology": topology,
        "target_branch": target_branch,
        "mission_type": "software-dev",
    });
    if let (Some(meta), Some(extra)) = (meta.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            meta.insert(key.clone(), value.clone());
        }
    }
    write(
        &dir.join("meta.json"),
        &format!("{}\n", serde_json::to_string_pretty(&meta).unwrap()),
    );
    dir
}

/// The canonical `<slug>-<mid8>` mission directory name for a fixture.
pub fn canonical_dir_name(slug: &str, mission_id: &str) -> String {
    format!("{slug}-{}", &mission_id[..8])
}

/// Create a branch at the current `HEAD` and return its tip.
pub fn create_branch(root: &Path, branch: &str) -> String {
    git(root, &["branch", branch]);
    branch_tip(root, branch)
}

/// Read a path's content at a commit (`git show <rev>:<path>`).
pub fn show_at(root: &Path, rev: &str, path: &str) -> String {
    git(root, &["show", &format!("{rev}:{path}")])
}

/// Count commits reachable from `branch` but from no other branch.
pub fn unique_commit_count(root: &Path, branch: &str) -> usize {
    let output = git(
        root,
        &[
            "rev-list",
            branch,
            "--not",
            "--branches",
            "--remotes",
            "--tags",
        ],
    );
    output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count()
}

/// The commit trailer value for `key` in `rev`'s message.
pub fn trailer(root: &Path, rev: &str, key: &str) -> String {
    git(
        root,
        &[
            "log",
            "-1",
            &format!("--format=%(trailers:key={key},valueonly)"),
            rev,
        ],
    )
    .trim()
    .to_owned()
}
