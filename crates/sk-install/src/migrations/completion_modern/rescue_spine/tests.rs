//! Native regression fixtures for the four rescue migrations, grounded in
//! separately retained Source receipts. These tests do NOT execute Source or
//! prove per-fixture Native CWD/environment isolation; paired Source-first
//! executions live in dedicated integration tests and external evidence.
//!
//! Each fixture uses a fresh independent repository where Git is needed and
//! checks the carrier/bytes/untouched directories/modes/observed mtimes.

use super::*;

fn git(root: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_AUTHOR_NAME", "B7")
        .env("GIT_AUTHOR_EMAIL", "b7@example.invalid")
        .env("GIT_COMMITTER_NAME", "B7")
        .env("GIT_COMMITTER_EMAIL", "b7@example.invalid")
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_init(root: &Path) {
    git(root, &["init", "-q"]);
    git(root, &["config", "user.name", "B7"]);
    git(root, &["config", "user.email", "b7@example.invalid"]);
}

fn commit(root: &Path, message: &str) {
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", message]);
}

fn project(root: &Path) {
    git_init(root);
    std::fs::create_dir_all(root.join(".kittify")).expect("kittify");
    std::fs::write(
        root.join(".kittify/metadata.yaml"),
        "spec_kitty:\n  version: 0.9.0\n",
    )
    .expect("metadata");
    std::fs::write(root.join(".gitignore"), "*.pyc\n").expect("gitignore");
}

fn mission(root: &Path, slug: &str) -> PathBuf {
    let dir = root.join("kitty-specs").join(slug);
    std::fs::create_dir_all(dir.join("tasks")).expect("tasks");
    std::fs::write(
        dir.join("meta.json"),
        "{\n  \"created_at\": \"2026-01-02T03:04:05+00:00\",\n  \"mission_number\": \"042\",\n  \"slug\": \"mission-a\",\n  \"topology\": \"single_branch\"\n}\n",
    )
    .expect("meta");
    std::fs::write(
        dir.join("tasks/WP01-alpha.md"),
        "---\nlane: in_progress\nshell_pid: 4242\nagent: claude\nassignee: ana\ntracker_refs: [GH-1]\nprogress: 50\n---\n\n# WP01 alpha\n",
    )
    .expect("wp");
    std::fs::write(
        dir.join("tasks.md"),
        "## WP01 alpha\n- [x] T001 done\n- [ ] T002 todo\n",
    )
    .expect("tasks.md");
    std::fs::write(
        dir.join("status.json"),
        "{\n  \"materialized_at\": \"2026-01-02T03:05:00+00:00\",\n  \"work_packages\": {\"WP01\": {\"lane\": \"in_progress\"}}\n}\n",
    )
    .expect("status");
    dir
}

/// A `kitty-specs/` child that is not a Mission: no `meta.json`, only legacy
/// content. In git-aware mode this is residue.
fn residue_dir(root: &Path, slug: &str) {
    let dir = root.join("kitty-specs").join(slug);
    std::fs::create_dir_all(dir.join("tasks")).expect("tasks");
    std::fs::write(
        dir.join("tasks/WP02-beta.md"),
        "---\nlane: in_progress\n---\n\n# WP02 beta\n",
    )
    .expect("wp");
    std::fs::write(
        dir.join("status.json"),
        "{\n  \"materialized_at\": \"2026-01-02T03:05:00+00:00\",\n  \"work_packages\": {\"WP02\": {\"lane\": \"in_progress\"}}\n}\n",
    )
    .expect("status");
}

fn finder(id: &str) -> &'static NativeMigration {
    IMPLEMENTATIONS
        .iter()
        .find(|migration| migration.id == id)
        .expect("declared")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).expect("read")
}

fn mtime(path: &Path) -> std::time::SystemTime {
    std::fs::metadata(path)
        .expect("metadata")
        .modified()
        .expect("modified")
}

// ---------------------------------------------------------------------------
// F1 — Source-specific corpus selection (one classifier, per-view predicates)
// ---------------------------------------------------------------------------

#[test]
fn canonical_context_never_writes_into_a_non_mission_directory() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    project(root);
    let mission = mission(root, "mission-a");
    residue_dir(root, "tracked-notes");
    residue_dir(root, "untracked-notes");
    // Commit only the Mission and the first notes folder: git now tracks
    // something under kitty-specs/, which is the git-aware residue mode.
    git(
        root,
        &["add", "kitty-specs/mission-a", "kitty-specs/tracked-notes"],
    );
    git(root, &["commit", "-q", "-m", "corpus"]);
    let residue_before: Vec<String> = ["tasks/WP02-beta.md", "status.json"]
        .iter()
        .map(|name| read(&root.join("kitty-specs/tracked-notes").join(name)))
        .collect();
    let untracked_before = read(&root.join("kitty-specs/untracked-notes/status.json"));

    let outcome = (finder("3.0.0_canonical_context").apply)(root).expect("apply");
    assert!(outcome.success, "{:?}", outcome.errors);
    assert!(
        outcome
            .changes
            .iter()
            .any(|change| change == "Migrated 1 feature(s), 1 WPs backfilled, 2 events generated"),
        "{:?}",
        outcome.changes
    );

    for slug in ["tracked-notes", "untracked-notes"] {
        let dir = root.join("kitty-specs").join(slug);
        assert!(!dir.join("meta.json").exists(), "{slug} got a meta.json");
        assert!(
            !dir.join("status.events.jsonl").exists(),
            "{slug} got an event log"
        );
        assert!(
            !root.join(".kittify/derived").join(slug).exists(),
            "{slug} got a derived view"
        );
    }
    let residue_after: Vec<String> = ["tasks/WP02-beta.md", "status.json"]
        .iter()
        .map(|name| read(&root.join("kitty-specs/tracked-notes").join(name)))
        .collect();
    assert_eq!(residue_before, residue_after, "residue bytes changed");
    assert_eq!(
        untracked_before,
        read(&root.join("kitty-specs/untracked-notes/status.json"))
    );
    // The real Mission was migrated.
    assert!(mission.join("status.events.jsonl").is_file());
}

#[test]
fn normalize_mission_lifecycle_reports_residue_with_sources_actionable_notice() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    project(root);
    let _mission = mission(root, "mission-a");
    residue_dir(root, "tracked-notes");
    git(
        root,
        &["add", "kitty-specs/mission-a", "kitty-specs/tracked-notes"],
    );
    git(root, &["commit", "-q", "-m", "corpus"]);

    let outcome = (finder("3.2.0a4_normalize_mission_lifecycle").apply)(root).expect("apply");
    assert!(
        outcome.warnings.iter().any(|warning| warning
            .starts_with("tracked-notes: not a Mission (residue directory ")
            && warning.contains("left untouched")),
        "{:?}",
        outcome.warnings
    );
    assert!(!root.join("kitty-specs/tracked-notes/meta.json").exists());
}

#[test]
fn runtime_cutover_reports_residue_and_never_creates_a_meta_stub() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    project(root);
    let mission = mission(root, "mission-a");
    std::fs::write(
        mission.join("meta.json"),
        "{\n  \"created_at\": \"2026-01-02T03:04:05+00:00\",\n  \"mission_id\": \"01KJ00000000000000000000MA\"\n}\n",
    )
    .expect("meta");
    residue_dir(root, "tracked-notes");
    git(
        root,
        &["add", "kitty-specs/mission-a", "kitty-specs/tracked-notes"],
    );
    git(root, &["commit", "-q", "-m", "corpus"]);

    let report = cutover_corpus(root, false).expect("corpus");
    assert_eq!(report.residue(), 1, "{report:?}");
    assert_eq!(report.flipped(), 1, "{report:?}");
    assert!(read(&root.join("kitty-specs/mission-a/status.events.jsonl")).contains("WP01"));
    let residue = root.join("kitty-specs/tracked-notes");
    assert!(
        !residue.join("meta.json").exists(),
        "residue got a meta stub"
    );
    assert!(
        !residue.join("status.events.jsonl").exists(),
        "residue got a seed log"
    );
}

// ---------------------------------------------------------------------------
// F2 — a Source-valid backup failure is non-fatal
// ---------------------------------------------------------------------------

#[test]
fn a_backup_that_cannot_be_taken_warns_and_the_migration_still_runs() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    project(root);
    let mission = mission(root, "mission-a");
    let locked = root.join(".kittify/locked.bin");
    std::fs::write(&locked, b"secret").expect("locked");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).expect("chmod");
    }
    if std::fs::read(&locked).is_ok() {
        // A privileged runner reads through 0o000; this case binds the
        // unprivileged path only.
        return;
    }

    let outcome = (finder("3.0.0_canonical_context").apply)(root).expect("apply");
    assert!(outcome.success, "{:?}", outcome.errors);
    // Source's exact warning text (Source-first: `c300backup2`).
    assert!(
        outcome
            .warnings
            .iter()
            .any(|warning| warning == backup::BACKUP_WARNING),
        "{:?}",
        outcome.warnings
    );
    // The nine steps still ran.
    assert!(mission.join("status.events.jsonl").is_file());
    assert!(read(&root.join(".kittify/metadata.yaml")).contains("schema_version: 3"));
}

#[test]
fn a_stale_read_only_backup_is_cleared_and_the_run_completes() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    project(root);
    let stake = root.join(".kittify").join(backup::BACKUP_DIR_NAME);
    std::fs::create_dir_all(&stake).expect("stale");
    std::fs::write(stake.join("old.txt"), b"stale").expect("old");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            stake.join("old.txt"),
            std::fs::Permissions::from_mode(0o400),
        )
        .expect("chmod file");
        std::fs::set_permissions(&stake, std::fs::Permissions::from_mode(0o500))
            .expect("chmod dir");
    }
    let outcome = (finder("3.0.0_canonical_context").apply)(root).expect("apply");
    assert!(outcome.success, "{:?}", outcome.errors);
    assert!(
        !outcome
            .warnings
            .iter()
            .any(|warning| warning == backup::BACKUP_WARNING),
        "{:?}",
        outcome.warnings
    );
    assert!(!stake.exists(), "backup was cleaned up");
}

// ---------------------------------------------------------------------------
// F3 — 3.0.0 does not restore a tracked-but-absent meta.json
// ---------------------------------------------------------------------------

#[test]
fn canonical_context_skips_a_mission_whose_meta_json_is_only_tracked() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    project(root);
    let mission = mission(root, "mission-a");
    commit(root, "legacy corpus");
    std::fs::remove_file(mission.join("meta.json")).expect("delete meta");

    let outcome = (finder("3.0.0_canonical_context").apply)(root).expect("apply");
    assert!(outcome.success, "{:?}", outcome.errors);
    assert_eq!(
        outcome.changes.first().map(String::as_str),
        Some("Migrated 0 feature(s), 0 WPs backfilled, 0 events generated"),
        "{:?}",
        outcome.changes
    );
    assert!(
        !mission.join("meta.json").exists(),
        "3.0.0 must not restore the tracked copy"
    );
    assert!(!mission.join("status.events.jsonl").exists());
}

// ---------------------------------------------------------------------------
// F4 / F12 — the full failure carrier and the real rollback bytes/times
// ---------------------------------------------------------------------------

#[test]
fn a_step_failure_returns_the_report_instead_of_a_refusal_and_restores_modes_and_times() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    git_init(root);
    // `.kittify` exists but metadata.yaml does not: the identity step fails.
    std::fs::create_dir_all(root.join(".kittify")).expect("kittify");
    std::fs::write(root.join(".kittify/keep.txt"), "keep\n").expect("keep");
    let mission = mission(root, "mission-a");
    let wp = mission.join("tasks/WP01-alpha.md");
    let before = std::fs::read(&wp).expect("wp");
    let before_mtime = mtime(&wp);
    let keep_mtime = mtime(&root.join(".kittify/keep.txt"));

    let outcome = (finder("3.0.0_canonical_context").apply)(root).expect("carrier");
    assert!(!outcome.success, "success must mirror the Source report");
    assert_eq!(
        outcome.changes,
        vec!["Migration failed at step: identity_backfill".to_string()]
    );
    assert_eq!(outcome.errors.len(), 1);
    assert!(
        outcome.errors[0]
            .starts_with("[identity_backfill] metadata.yaml missing: metadata.yaml not found: "),
        "{:?}",
        outcome.errors
    );
    // Source reports no positive rollback warning; a clean rollback leaves the
    // warning list empty (Source-first: `c300fail`).
    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
    assert_eq!(std::fs::read(&wp).expect("wp"), before);
    assert_eq!(mtime(&wp), before_mtime, "rollback must keep the mtime");
    assert_eq!(mtime(&root.join(".kittify/keep.txt")), keep_mtime);
    assert!(!root.join(".kittify").join(backup::BACKUP_DIR_NAME).exists());
}

#[test]
fn the_success_carrier_uses_sources_exact_change_lines() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    project(root);
    let _mission = mission(root, "mission-a");

    let outcome = (finder("3.0.0_canonical_context").apply)(root).expect("apply");
    assert_eq!(
        outcome.changes,
        vec![
            "Migrated 1 feature(s), 1 WPs backfilled, 2 events generated".to_string(),
            "Moved 1 derived file(s)".to_string(),
        ],
        "{:?}",
        outcome.changes
    );
    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
}

// ---------------------------------------------------------------------------
// F6 — Source frontmatter emission
// ---------------------------------------------------------------------------

#[test]
fn rewritten_frontmatter_carries_dependencies_in_source_order() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    project(root);
    let mission = mission(root, "mission-a");

    (finder("3.0.0_canonical_context").apply)(root).expect("apply");
    let wp = read(&mission.join("tasks/WP01-alpha.md"));
    assert!(wp.contains("dependencies: []"), "{wp}");
    // Source `WP_FIELD_ORDER`: work_package_id, title, dependencies, … then the
    // remaining keys alphabetically.
    let keys: Vec<&str> = wp
        .split("---\n")
        .nth(1)
        .expect("frontmatter")
        .lines()
        .filter(|line| {
            !line.is_empty() && !line.starts_with([' ', '\t', '#', '-']) && line.contains(':')
        })
        .map(|line| line.split(':').next().unwrap_or_default())
        .collect();
    assert_eq!(
        keys,
        [
            "work_package_id",
            "dependencies",
            "authoritative_surface",
            "execution_mode",
            "mission_id",
            "owned_files",
            "tracker_refs",
            "wp_code"
        ],
        "{wp}"
    );
}

// ---------------------------------------------------------------------------
// F9 / F13 — charter rename report shape and codec detail
// ---------------------------------------------------------------------------

#[test]
fn an_archive_then_remove_is_a_warning_not_a_change() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    std::fs::create_dir_all(root.join(".kittify/charter")).expect("charter");
    std::fs::write(root.join(".kittify/charter/charter.md"), "# charter\n").expect("charter");
    std::fs::create_dir_all(root.join(".kittify/missions/m1/constitution")).expect("mission");
    std::fs::write(
        root.join(".kittify/missions/m1/constitution/constitution.md"),
        "mission constitution body\n",
    )
    .expect("payload");

    let outcome = (finder("3.1.1_charter_rename").apply)(root).expect("apply");
    assert!(outcome.success, "{:?}", outcome.errors);
    assert!(
        !outcome
            .changes
            .iter()
            .any(|change| change.contains("mission-specific")),
        "a preserved payload must not be reported as removed: {:?}",
        outcome.changes
    );
    assert!(
        outcome
            .warnings
            .iter()
            .any(|warning| warning.contains("archived to")),
        "{:?}",
        outcome.warnings
    );
}

#[test]
fn an_undecodable_file_keeps_the_codec_detail_and_the_failure_carrier() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    std::fs::create_dir_all(root.join(".kittify/charter")).expect("charter");
    std::fs::write(root.join(".kittify/charter/charter.md"), "# charter\n").expect("charter");
    std::fs::write(
        root.join(".kittify/charter/binary.md"),
        [0xff_u8, 0xfe, 0x00],
    )
    .expect("binary");

    let outcome = (finder("3.1.1_charter_rename").apply)(root).expect("carrier");
    assert!(!outcome.success);
    assert!(
        outcome
            .errors
            .iter()
            .any(|error| error == "Failed to read .kittify/charter/binary.md: 'utf-8' codec can't decode byte 0xff in position 0: invalid start byte"),
        "{:?}",
        outcome.errors
    );
}

#[test]
fn charter_rename_preview_reports_would_actions_and_writes_nothing() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    std::fs::create_dir_all(root.join(".kittify/constitution")).expect("dirs");
    std::fs::write(
        root.join(".kittify/constitution/constitution.md"),
        "# The constitution\nConstitution rules.\n",
    )
    .expect("constitution");

    let outcome = preview_charter_rename(root).expect("preview");
    assert!(outcome.success);
    assert!(
        outcome
            .changes
            .iter()
            .any(|change| change == "Would rename .kittify/constitution/ -> .kittify/charter/"),
        "{:?}",
        outcome.changes
    );
    assert!(root.join(".kittify/constitution/constitution.md").is_file());
    assert!(!root.join(".kittify/charter").exists());
}

// ---------------------------------------------------------------------------
// F7 — 3.2.0a4 detect predicate, action text
// ---------------------------------------------------------------------------

#[test]
fn a_missing_board_summary_alone_does_not_select_the_lifecycle_normalization() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    project(root);
    let mission = mission(root, "mission-a");
    std::fs::write(
        mission.join("meta.json"),
        "{\n  \"created_at\": \"2026-01-02T03:04:05+00:00\",\n  \"mission_id\": \"01KJ00000000000000000000MA\",\n  \"mission_number\": 42,\n  \"slug\": \"mission-a\"\n}\n",
    )
    .expect("meta");
    commit(root, "corpus");
    // First pass writes the canonical views.
    let first = (finder("3.2.0a4_normalize_mission_lifecycle").apply)(root).expect("apply");
    assert!(first.success, "{:?}", first.errors);
    let derived = root.join(".kittify/derived/mission-a");
    std::fs::remove_file(derived.join("board-summary.json")).expect("remove board");

    // Source's refresh predicate checks status/progress/lifecycle only.
    assert!(
        !(finder("3.2.0a4_normalize_mission_lifecycle").detect)(root).expect("detect"),
        "a missing board summary must not select the migration"
    );
    let second = preview_normalize_mission_lifecycle(root).expect("preview");
    assert!(second.changes.is_empty(), "{:?}", second.changes);
    assert!(!derived.join("board-summary.json").exists());
}

#[test]
fn the_lifecycle_action_text_carries_no_private_view_count() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    std::fs::create_dir_all(root.join(".kittify")).expect("kittify");
    git_init(root);
    let mission = root.join("kitty-specs/mission-b");
    std::fs::create_dir_all(mission.join("tasks")).expect("tasks");
    std::fs::write(
        mission.join("meta.json"),
        "{\n  \"created_at\": \"2026-02-03T04:05:06+00:00\",\n  \"mission_number\": \"007\",\n  \"slug\": \"mission-b\"\n}\n",
    )
    .expect("meta");
    std::fs::write(
        mission.join("tasks/WP01-beta.md"),
        "---\nlane: approved\n---\n\n# WP01 beta\n",
    )
    .expect("wp");

    let outcome = (finder("3.2.0a4_normalize_mission_lifecycle").apply)(root).expect("apply");
    for expected in [
        "mission-b: Backfilled mission identity metadata",
        "mission-b: Normalized legacy mission_number type",
        "mission-b: Backfilled missing WP identity fields",
        "mission-b: Regenerated canonical status/progress/lifecycle views",
    ] {
        assert!(
            outcome.changes.iter().any(|change| change == expected),
            "missing {expected:?} in {:?}",
            outcome.changes
        );
    }
}

// ---------------------------------------------------------------------------
// F4 / F10 / F14 — the cutover carrier, preview and clean no-op
// ---------------------------------------------------------------------------

fn cutover_corpus_fixture(root: &Path) -> (PathBuf, PathBuf) {
    project(root);
    let mut dirs = Vec::new();
    for (slug, mission_id) in [
        ("mission-e", "01KJ00000000000000000000ME"),
        ("mission-z-corrupt", "01KJ00000000000000000000MZ"),
    ] {
        let dir = root.join("kitty-specs").join(slug);
        std::fs::create_dir_all(dir.join("tasks")).expect("tasks");
        std::fs::write(
            dir.join("meta.json"),
            format!(
                "{{\n  \"created_at\": \"2026-04-05T06:07:08+00:00\",\n  \"mission_id\": \"{mission_id}\"\n}}\n"
            ),
        )
        .expect("meta");
        std::fs::write(
            dir.join("tasks/WP01-delta.md"),
            "---\nshell_pid: 4242\nshell_pid_created_at: '2026-04-05T06:00:00+00:00'\nagent: claude\n---\n\nbody\n",
        )
        .expect("wp");
        dirs.push(dir);
    }
    let claim_id =
        sk_status::migration_backfill::deterministic_ulid("01KJ00000000000000000000MZ|WP01|claim");
    std::fs::write(
        root.join("kitty-specs/mission-z-corrupt/status.events.jsonl"),
        format!(
            "{{\"actor\": \"migration:backfill_runtime_state\", \"at\": \"2026-04-05T06:00:00+00:00\", \"event_id\": \"{claim_id}\", \"evidence\": null, \"execution_mode\": \"worktree\", \"force\": false, \"from_lane\": \"planned\", \"mission_id\": \"01KJ00000000000000000000MZ\", \"mission_slug\": \"mission-z-corrupt\", \"policy_metadata\": {{\"agent\": \"claude\", \"shell_pid\": 9999, \"shell_pid_created_at\": \"2026-04-05T06:00:00+00:00\"}}, \"reason\": null, \"review_ref\": null, \"to_lane\": \"claimed\", \"wp_id\": \"WP01\"}}\n"
        ),
    )
    .expect("tampered");
    (dirs[0].clone(), dirs[1].clone())
}

#[test]
fn a_cutover_abort_returns_the_structured_partial_writes() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let (flipped, corrupt) = cutover_corpus_fixture(root);

    let outcome = (finder("runtime_state_backfill").apply)(root).expect("carrier");
    assert!(!outcome.success, "a Source abort is a returned failure");
    assert_eq!(outcome.errors.len(), 1);
    let abort = &outcome.errors[0];
    assert!(
        abort.starts_with(
            "Runtime-state cutover aborted: mission 'mission-z-corrupt' failed fail-closed verify ("
        ),
        "{abort}"
    );
    assert!(
        abort.contains("--mission mission-z-corrupt --dry-run"),
        "{abort}"
    );
    assert!(!abort.contains("persisted before abort"), "{abort}");
    let residue: Vec<(String, String)> = outcome
        .partial_writes
        .iter()
        .map(|write| {
            (
                write.mission.clone(),
                Path::new(&write.path)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            )
        })
        .collect();
    assert_eq!(
        residue,
        vec![
            ("mission-e".to_string(), "status.events.jsonl".to_string()),
            ("mission-e".to_string(), "meta.json".to_string()),
        ]
    );
    let meta: serde_json::Value =
        serde_json::from_str(&read(&flipped.join("meta.json"))).expect("json");
    assert_eq!(meta["status_phase"], "1");
    let meta: serde_json::Value =
        serde_json::from_str(&read(&corrupt.join("meta.json"))).expect("json");
    assert!(meta.get("status_phase").is_none());
}

#[test]
fn a_clean_cutover_is_a_successful_empty_change_list() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    project(root);
    let mission = mission(root, "mission-c");
    std::fs::write(
        mission.join("meta.json"),
        "{\n  \"created_at\": \"2026-03-04T05:06:07+00:00\",\n  \"mission_id\": \"01KJ00000000000000000000MC\"\n}\n",
    )
    .expect("meta");
    std::fs::write(
        mission.join("tasks/WP01-gamma.md"),
        "---\nshell_pid: 4242\nshell_pid_created_at: '2026-03-04T05:00:00+00:00'\nagent: claude\nassignee: ana\n---\n\nbody\n",
    )
    .expect("wp");
    let outcome = (finder("runtime_state_backfill").apply)(root).expect("apply");
    assert!(outcome.success);
    assert_eq!(outcome.changes.len(), 1);
    assert!(
        outcome.changes[0].starts_with("Seeded ")
            && outcome.changes[0].ends_with("and flipped 1 of 1 mission(s) scanned"),
        "{:?}",
        outcome.changes
    );

    // Second pass: genuinely nothing to do, and Source still reports success
    // with an empty change list.
    let again = (finder("runtime_state_backfill").apply)(root).expect("again");
    assert!(again.success);
    assert!(again.changes.is_empty(), "{:?}", again.changes);
    assert!(again.partial_writes.is_empty());
}

#[test]
fn cutover_previews_the_would_seed_count_and_writes_nothing() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    project(root);
    let mission = mission(root, "mission-c");
    std::fs::write(
        mission.join("meta.json"),
        "{\n  \"created_at\": \"2026-03-04T05:06:07+00:00\",\n  \"mission_id\": \"01KJ00000000000000000000MC\"\n}\n",
    )
    .expect("meta");
    std::fs::write(
        mission.join("tasks/WP01-gamma.md"),
        "---\nshell_pid: 4242\nshell_pid_created_at: '2026-03-04T05:00:00+00:00'\nagent: claude\n---\n\nbody\n",
    )
    .expect("wp");

    let outcome = preview_runtime_state_backfill(root).expect("preview");
    assert!(outcome.success);
    assert_eq!(
        outcome.changes,
        vec!["dry-run: would seed 2 event(s) across 1 mission(s) scanned".to_string()]
    );
    assert!(!mission.join("status.events.jsonl").exists());
    let meta: serde_json::Value =
        serde_json::from_str(&read(&mission.join("meta.json"))).expect("json");
    assert!(meta.get("status_phase").is_none());
    // `detect` still reports the work.
    assert!((finder("runtime_state_backfill").detect)(root).expect("detect"));
}

#[test]
fn canonical_context_preview_reports_the_dry_run_summary_and_writes_nothing() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    project(root);
    let mission = mission(root, "mission-a");
    let before = read(&root.join(".kittify/metadata.yaml"));

    let outcome = preview_canonical_context(root).expect("preview");
    assert!(outcome.success);
    assert_eq!(
        outcome.changes,
        vec!["[DRY RUN] Migrated 1 feature(s), 0 WPs backfilled, 0 events generated".to_string()]
    );
    assert_eq!(
        outcome.warnings,
        vec!["DRY RUN: would migrate 1 feature(s) to schema v3".to_string()]
    );
    assert_eq!(read(&root.join(".kittify/metadata.yaml")), before);
    assert!(!mission.join("status.events.jsonl").exists());
    assert!(!root.join(".kittify/derived").exists());
    assert!(!root.join(".kittify").join(backup::BACKUP_DIR_NAME).exists());
}

// ---------------------------------------------------------------------------
// F5 — the authored-branch git file list
// ---------------------------------------------------------------------------

#[test]
fn a_real_lane_branch_diff_supplies_owned_files() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    git_init(root);
    git(root, &["checkout", "-q", "-b", "main"]);
    std::fs::write(root.join("README.md"), "base\n").expect("readme");
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "base"]);
    git(
        root,
        &["checkout", "-q", "-b", "kitty/mission-013-mission-y-lane-a"],
    );
    std::fs::create_dir_all(root.join("src")).expect("src");
    std::fs::write(root.join("src/feature.py"), "print('x')\n").expect("feature");
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "lane work"]);
    git(root, &["checkout", "-q", "main"]);

    let mission = root.join("kitty-specs/013-mission-y");
    std::fs::create_dir_all(mission.join("tasks")).expect("tasks");
    std::fs::write(
        mission.join("meta.json"),
        "{\n  \"mission_id\": \"01KJ0000000000000000000MY\",\n  \"mission_number\": 13,\n  \"slug\": \"013-mission-y\"\n}\n",
    )
    .expect("meta");
    std::fs::write(
        mission.join("lanes.json"),
        "{\n  \"version\": 1,\n  \"mission_slug\": \"013-mission-y\",\n  \"mission_branch\": \"kitty/mission-013-mission-y\",\n  \"target_branch\": \"main\",\n  \"lanes\": [\n    {\n      \"lane_id\": \"lane-a\",\n      \"wp_ids\": [\"WP01\"],\n      \"write_scope\": [\"src/**\"],\n      \"predicted_surfaces\": [],\n      \"depends_on_lanes\": [],\n      \"parallel_group\": 0\n    }\n  ],\n  \"computed_at\": \"2026-01-01T00:00:00+00:00\",\n  \"computed_from\": \"tasks.md\"\n}\n",
    )
    .expect("lanes");
    let wp = mission.join("tasks/WP01-alpha.md");
    std::fs::write(
        &wp,
        "---\nplanning_base_branch: main\nwp_code: WP01\n---\n\n# WP01\n\nImplement the feature.\n",
    )
    .expect("wp");

    let changed = lane_changed_files(&mission, "013-mission-y");
    assert_eq!(
        changed.get("WP01"),
        Some(&vec!["src/feature.py".to_string()]),
        "lane diff: {changed:?}"
    );
    let written = sk_mission::migration_rescue::backfill_ownership_with_files(
        &mission,
        "013-mission-y",
        &changed,
        false,
    )
    .expect("ownership");
    assert_eq!(written, 1);
    let text = read(&wp);
    assert!(text.contains("execution_mode: code_change"), "{text}");
    assert!(text.contains("- src/feature.py"), "{text}");
    assert!(text.contains("authoritative_surface: src/"), "{text}");
}

#[test]
fn without_an_authored_base_branch_there_is_no_diff_and_no_override() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    git_init(root);
    git(root, &["checkout", "-q", "-b", "main"]);
    std::fs::write(root.join("README.md"), "base\n").expect("readme");
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "base"]);

    let mission = root.join("kitty-specs/014-mission-z");
    std::fs::create_dir_all(mission.join("tasks")).expect("tasks");
    let wp = mission.join("tasks/WP01-alpha.md");
    std::fs::write(
        &wp,
        "---\nwp_code: WP01\n---\n\n# WP01\n\nNo path-like text here.\n",
    )
    .expect("wp");
    let changed = lane_changed_files(&mission, "014-mission-z");
    assert!(changed.is_empty(), "{changed:?}");
    sk_mission::migration_rescue::backfill_ownership_with_files(
        &mission,
        "014-mission-z",
        &changed,
        false,
    )
    .expect("ownership");
    let text = read(&wp);
    assert!(text.contains("execution_mode: code_change"), "{text}");
    assert!(text.contains("- src/**"), "{text}");
}

// ---------------------------------------------------------------------------
// F8 — a detect/read failure propagates instead of reporting "finished"
// ---------------------------------------------------------------------------

#[test]
fn cutover_detect_propagates_an_unreadable_wp_artifact() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    project(root);
    let mission = mission(root, "mission-a");
    let wp = mission.join("tasks/WP01-alpha.md");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&wp, std::fs::Permissions::from_mode(0o000)).expect("chmod");
    }
    if std::fs::read_to_string(&wp).is_ok() {
        // A privileged runner reads through 0o000; this case binds the
        // unprivileged path only.
        return;
    }
    // Source lets `LegacyRuntimeReadError` escape `detect()`/`can_apply()`.
    let error = (finder("runtime_state_backfill").detect)(root).expect_err("read error");
    assert!(
        error
            .to_string()
            .contains("cannot read mission-a for legacy runtime"),
        "{error}"
    );
    let refusal = (finder("runtime_state_backfill").can_apply)(root);
    assert!(refusal.is_err(), "{refusal:?}");
}
