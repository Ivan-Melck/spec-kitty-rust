//! CF25 — the owned repository-root composition of the runtime-state seed lock.
//!
//! Source `backfill_runtime_state` takes its Mission status lock with
//! `resolve_status_lock_root(feature_dir, owned.repository_root)`: on the OWNED
//! leg the lock namespace is the ownership fact's repository root, not whichever
//! root the mission directory would resolve for itself. CF21's report recorded
//! the gap (seed lock resolved from the mission directory while only the flip
//! took the fact's root); this pins the additive
//! `backfill_runtime_state_with_legs_and_root` sibling that closes it, and proves
//! the old wrapper still resolves the unowned root and produces the same seeds.
//!
//! Native functional smokes only (fresh `mktemp` repositories, no oracle child,
//! no pairing claim). The cross-process lock-namespace proof remains listed as
//! deep verification debt in the CF25 report.

use std::path::{Path, PathBuf};
use std::process::Command;

use sk_status::migration_backfill::{
    RuntimeBackfillAction, backfill_runtime_state, backfill_runtime_state_with_legs,
    backfill_runtime_state_with_legs_and_root,
};
use sk_status::mutation_lock::{mission_lock_path, resolve_lock_root};

fn git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("failed to run git {args:?}: {error}"));
    assert!(
        output.status.success(),
        "git {args:?} failed in {}: {}",
        cwd.display(),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent dir");
    }
    std::fs::write(path, text).expect("write file");
}

fn init_repo(root: &Path) {
    std::fs::create_dir_all(root).expect("root");
    git(root, &["init", "-b", "main"]);
}

/// One legacy mission with evictable runtime state (five deterministic seeds).
fn legacy_mission(root: &Path, slug: &str) -> PathBuf {
    let dir = root.join("kitty-specs").join(slug);
    write(
        &dir.join("meta.json"),
        &format!(
            "{{\n  \"created_at\": \"2026-03-04T05:06:07+00:00\",\n  \"mission_id\": \"01KJ00000000000000000000MC\",\n  \"slug\": \"{slug}\"\n}}\n"
        ),
    );
    write(
        &dir.join("tasks/WP01-gamma.md"),
        "---\nshell_pid: 4242\nshell_pid_created_at: '2026-03-04T05:00:00+00:00'\nagent: claude\nassignee: ana\ntracker_refs: [GH-1, GH-2]\nreview_artifact_override_at: '2026-03-04T05:30:00+00:00'\nreview_artifact_override_actor: renata\nreview_artifact_override_wp_id: WP01\nreview_artifact_override_reason: manual approval\n---\n\n# WP01 gamma\n",
    );
    write(
        &dir.join("tasks.md"),
        "## WP01 gamma\n- [x] T001 done\n- [ ] T002 todo\n",
    );
    dir
}

fn events(dir: &Path) -> Vec<u8> {
    std::fs::read(dir.join("status.events.jsonl")).expect("seeded event log")
}

#[test]
fn the_owned_sibling_takes_the_fact_repository_root_as_its_lock_root() {
    let temp = tempfile::tempdir().expect("temp");
    let mission_repo = temp.path().join("mission-repo");
    init_repo(&mission_repo);
    let dir = legacy_mission(&mission_repo, "mission-c");

    // A SECOND, independent repository stands in for the ownership fact's
    // repository root. Source's owned leg is exactly this: the fact's root, not
    // the mission directory's own resolution.
    let owned_repo = temp.path().join("owned-repo");
    init_repo(&owned_repo);

    assert_eq!(resolve_lock_root(&dir, None), mission_repo);
    assert_eq!(resolve_lock_root(&dir, Some(&owned_repo)), owned_repo);

    let unowned_lock = mission_lock_path(&dir, None).expect("unowned lock path");
    let owned_lock = mission_lock_path(&dir, Some(&owned_repo)).expect("owned lock path");
    assert_ne!(
        unowned_lock, owned_lock,
        "the two legs must not share one lock namespace"
    );
    assert!(
        owned_lock.starts_with(owned_repo.join(".git")),
        "owned lock lives under the fact's repository: {}",
        owned_lock.display()
    );

    // The sibling seeds successfully through the owned root and produces the
    // same deterministic rows the unowned wrapper does.
    let owned_outcome =
        backfill_runtime_state_with_legs_and_root(&dir, &dir, false, Some(&owned_repo))
            .expect("owned seed");
    assert_eq!(owned_outcome.action, RuntimeBackfillAction::Wrote);
    assert_eq!(owned_outcome.seeded_count, 5);
    let owned_rows = events(&dir);

    let temp_unowned = tempfile::tempdir().expect("temp");
    let unowned_repo = temp_unowned.path().join("mission-repo");
    init_repo(&unowned_repo);
    let unowned_dir = legacy_mission(&unowned_repo, "mission-c");
    let unowned_outcome =
        backfill_runtime_state_with_legs(&unowned_dir, &unowned_dir, false).expect("unowned seed");
    assert_eq!(unowned_outcome.action, RuntimeBackfillAction::Wrote);
    assert_eq!(unowned_outcome.seeded_count, 5);
    let unowned_rows = events(&unowned_dir);
    assert_eq!(
        owned_rows, unowned_rows,
        "the owned root must not change the seeded data"
    );
}

#[test]
fn the_old_wrappers_keep_their_unowned_lock_root_and_the_owned_leg_takes_priority() {
    let temp = tempfile::tempdir().expect("temp");
    let mission_repo = temp.path().join("mission-repo");
    init_repo(&mission_repo);
    let dir = legacy_mission(&mission_repo, "mission-c");

    // The original single-leg wrapper is byte-compatible: it resolves the
    // mission directory's own root (no owned context exists on that path).
    assert_eq!(resolve_lock_root(&dir, None), mission_repo);
    let outcome = backfill_runtime_state(&dir, false).expect("legacy seed");
    assert_eq!(outcome.action, RuntimeBackfillAction::Wrote);
    assert_eq!(outcome.seeded_count, 5);

    // A second mission seeded with `Some(root)` uses the given root even when the
    // mission directory would resolve a different one.
    let owned_repo = temp.path().join("owned-repo");
    init_repo(&owned_repo);
    let other = legacy_mission(&mission_repo, "mission-d");
    let outcome =
        backfill_runtime_state_with_legs_and_root(&other, &other, false, Some(&owned_repo))
            .expect("owned seed");
    assert_eq!(outcome.action, RuntimeBackfillAction::Wrote);
    assert_eq!(resolve_lock_root(&other, Some(&owned_repo)), owned_repo);
}
