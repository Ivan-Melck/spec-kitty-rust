//! CF10 — the CLI fan-out's inherited-scope wiring.
//!
//! Native fixture tests (no Source child execution). Source authority:
//! `upgrade/runner.py::_upgrade_worktrees` (early return, per-root continuation,
//! commit gating) and `upgrade/autocommit.py`.
//!
//! The recorded C6 golden (`tests/upgrade_fanout_c6.rs`) covers the empty inherited
//! scope's exact tree/commit bytes; these tests cover the selection-shaped edges the
//! golden cannot: a selection that has no worktree-scoped migration, and a failed
//! root whose churn must stay uncommitted.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use sk_cli::compat::upgrade::upgrade_worktrees::{
    FAILED_RUN_LEFT_UNCOMMITTED_WARNING, plan_worktree_upgrade, run_worktree_upgrade,
};
use sk_install::{MigrationCatalog, MigrationSpec};

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent");
    }
    std::fs::write(path, text).expect("write");
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).expect("read")
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "CF10")
        .env("GIT_AUTHOR_EMAIL", "cf10@example.test")
        .env("GIT_COMMITTER_NAME", "CF10")
        .env("GIT_COMMITTER_EMAIL", "cf10@example.test")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn spec(id: &str) -> MigrationSpec {
    MigrationCatalog::embedded()
        .expect("catalogue")
        .get(id)
        .unwrap_or_else(|| panic!("{id} is declared"))
        .clone()
}

fn metadata(version: &str, schema: i64) -> String {
    format!(
        "spec_kitty:\n  version: \"{version}\"\n  schema_version: {schema}\n\
         environment:\n  initialized_at: \"2025-01-01T00:00:00+00:00\"\n"
    )
}

/// A repository root with one non-integrating worktree on `feature/x`.
fn fixture() -> (tempfile::TempDir, PathBuf) {
    let sandbox = tempfile::tempdir().expect("tempdir");
    let project = sandbox.path().join("proj");
    write(
        &project.join(".kittify/config.yaml"),
        "agents:\n  available:\n    - claude\n  auto_commit: true\n",
    );
    write(
        &project.join(".kittify/metadata.yaml"),
        &metadata("3.2.0", 2),
    );
    write(&project.join("README.md"), "base\n");
    git(&project, &["init", "-q", "-b", "main"]);
    git(&project, &["add", "-A"]);
    git(&project, &["commit", "-qm", "base"]);
    // A non-integrating checkout with its own repository: Source treats a
    // `.worktrees/` entry without a Mission-namespace branch as upgradeable, and
    // a real `.git/info/exclude` is what the exclude migrations read.
    let lane = project.join(".worktrees/lane");
    std::fs::create_dir_all(&lane).expect("lane");
    write(
        &lane.join(".kittify/config.yaml"),
        "agents:\n  available:\n    - claude\n",
    );
    write(&lane.join(".kittify/metadata.yaml"), &metadata("3.2.0", 2));
    write(&lane.join("README.md"), "lane\n");
    git(&lane, &["init", "-q", "-b", "feature/x"]);
    git(&lane, &["add", "-A"]);
    git(&lane, &["commit", "-qm", "lane seed"]);
    (sandbox, project)
}

#[test]
fn a_project_only_selection_examines_no_worktree() {
    // Source `_upgrade_worktrees`: `if migrations and not worktree_migrations:
    // return result` — before discovery, stamping or committing anything.
    let (_sandbox, project) = fixture();
    let lane = project.join(".worktrees/lane");
    let lane_head = git(&lane, &["rev-parse", "HEAD"]);
    let lane_metadata = read(&lane.join(".kittify/metadata.yaml"));
    let inherited = [spec("3.2.0rc35_unified_bundle")];
    assert!(!inherited[0].runs_on_worktrees);

    let preview = plan_worktree_upgrade(&project, "4.0.0rc8", Some("4.0.0rc8"), true, &inherited)
        .expect("preview");
    assert!(
        preview.roots.is_empty(),
        "a project-only selection discovered roots: {:?}",
        preview.roots
    );

    let report = run_worktree_upgrade(
        &project,
        "4.0.0rc8",
        Some("4.0.0rc8"),
        true,
        Duration::from_secs(10),
        &inherited,
    )
    .expect("run");
    assert!(report.roots.is_empty());
    assert_eq!(git(&lane, &["rev-parse", "HEAD"]), lane_head);
    assert_eq!(git(&lane, &["status", "--porcelain"]), "");
    assert_eq!(
        read(&lane.join(".kittify/metadata.yaml")),
        lane_metadata,
        "a project-only selection stamped a worktree"
    );
}

#[test]
fn a_failed_worktree_root_is_never_committed() {
    // Source `_commit_worktree_churn(failed=worktree_failed or worktree_errored)`:
    // the failed root commits nothing, names the churn it left, and the fan-out
    // reports a fatal failure (the CLI then exits non-zero).
    let (_sandbox, project) = fixture();
    let lane = project.join(".worktrees/lane");
    // `0.13.1`'s detect() cannot read non-UTF-8 exclude bytes: a deterministic
    // caught detection error (Source `GitignorePathError`, #5746).
    std::fs::write(lane.join(".git/info/exclude"), [0xff, 0xfe, b'\n']).expect("invalid exclude");
    let lane_head = git(&lane, &["rev-parse", "HEAD"]);
    let inherited = [spec("0.13.1_exclude_worktrees")];

    let report = run_worktree_upgrade(
        &project,
        "4.0.0rc8",
        Some("4.0.0rc8"),
        true,
        Duration::from_secs(10),
        &inherited,
    )
    .expect("run");
    assert!(!report.success(), "a failed root must fail the fan-out");
    let root = report
        .roots
        .iter()
        .find(|root| root.root.name == "lane")
        .expect("lane root");
    assert!(root.worktree_errored);
    assert!(root.commit.is_none(), "the failed root was committed");
    assert_eq!(
        git(&lane, &["rev-parse", "HEAD"]),
        lane_head,
        "the failed root's HEAD moved"
    );
    let status = git(&lane, &["status", "--porcelain"]);
    assert!(
        !status.is_empty(),
        "the failed run left no churn for the no-commit notice: {status}"
    );
    assert!(
        root.warnings
            .iter()
            .any(|warning| warning.ends_with(FAILED_RUN_LEFT_UNCOMMITTED_WARNING)),
        "the failed-run notice is missing: {:?}",
        root.warnings
    );
    assert!(
        report
            .failures()
            .iter()
            .any(|failure| failure.contains("Cannot safely detect")),
        "the fatal channel is empty: {:?}",
        report.failures()
    );
}
