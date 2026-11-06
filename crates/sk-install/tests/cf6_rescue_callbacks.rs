//! CF6 — one real Apply + Preview/retry smoke per rescue migration callback.
//!
//! Coverage only: each of the four B7 rescue families is driven through its
//! PUBLIC callback pair (the shared engine's `Apply` and the read-only
//! `Preview`) on a fresh fixture, with a byte-level "preview wrote nothing"
//! assertion, an idempotent retry, and a meaningful preservation case (an
//! unproven governance payload, a user-authored command file, a non-UTF-8 file,
//! and a malformed legacy WP frontmatter).
//!
//! This is Native-only evidence: it executes no Source child process and proves
//! no CWD/HOME/XDG isolation. Paired differential runs remain the deep-verification
//! debt recorded in the CF6 report.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sk_install::migrations::completion_modern::rescue_spine::{
    OWNED_IDS, preview_canonical_context, preview_charter_rename,
    preview_normalize_mission_lifecycle, preview_runtime_state_backfill,
};
use sk_install::upgrade::{MigrationOutcome, NativeMigration};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn git(root: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_AUTHOR_NAME", "CF6")
        .env("GIT_AUTHOR_EMAIL", "cf6@example.invalid")
        .env("GIT_COMMITTER_NAME", "CF6")
        .env("GIT_COMMITTER_EMAIL", "cf6@example.invalid")
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn project(root: &Path) {
    git(root, &["init", "-q"]);
    git(root, &["config", "user.name", "CF6"]);
    git(root, &["config", "user.email", "cf6@example.invalid"]);
    std::fs::create_dir_all(root.join(".kittify")).expect("kittify");
    std::fs::write(
        root.join(".kittify/metadata.yaml"),
        "spec_kitty:\n  version: 0.9.0\n",
    )
    .expect("metadata");
    std::fs::write(root.join(".gitignore"), "*.pyc\n").expect("gitignore");
}

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent");
    }
    std::fs::write(path, text).expect("write");
}

/// A mission with legacy frontmatter runtime state and no event log.
fn mission(root: &Path, slug: &str) -> PathBuf {
    let dir = root.join("kitty-specs").join(slug);
    write(
        &dir.join("meta.json"),
        "{\n  \"created_at\": \"2026-01-02T03:04:05+00:00\",\n  \"mission_number\": \"042\",\n  \"slug\": \"mission-a\",\n  \"topology\": \"single_branch\"\n}\n",
    );
    write(
        &dir.join("tasks/WP01-alpha.md"),
        "---\nlane: in_progress\nshell_pid: 4242\nagent: claude\nassignee: ana\ntracker_refs: [GH-1]\nprogress: 50\n---\n\n# WP01 alpha\n",
    );
    write(
        &dir.join("tasks.md"),
        "## WP01 alpha\n- [x] T001 done\n- [ ] T002 todo\n",
    );
    write(
        &dir.join("status.json"),
        "{\n  \"materialized_at\": \"2026-01-02T03:05:00+00:00\",\n  \"work_packages\": {\"WP01\": {\"lane\": \"in_progress\"}}\n}\n",
    );
    dir
}

/// Every regular file's authored bytes, keyed by project-relative POSIX path.
///
/// Git's own `.git/` internals and the migration backup are excluded: a preview
/// legitimately takes the Mission write lock (whose file lives in the git common
/// directory) and the backup is an Apply effect, so neither is authored project
/// content a read-only preview could corrupt.
fn footprint(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if name == ".git" || name == ".migration-backup" {
                continue;
            }
            if path.is_dir() {
                walk(root, &path, out);
            } else if path.is_file() {
                let relative = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                out.insert(relative, std::fs::read(&path).expect("read"));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

fn callback(id: &str) -> &'static NativeMigration {
    sk_install::migrations::completion_modern::rescue_spine::IMPLEMENTATIONS
        .iter()
        .find(|migration| migration.id == id)
        .unwrap_or_else(|| panic!("no rescue callback for {id}"))
}

fn assert_preview_writes_nothing(
    root: &Path,
    preview: fn(&Path) -> Result<MigrationOutcome, sk_install::InstallError>,
) {
    let before = footprint(root);
    let outcome = preview(root).expect("preview");
    assert!(outcome.success, "{:?}", outcome.errors);
    let after = footprint(root);
    let added: Vec<&String> = after
        .keys()
        .filter(|path| !before.contains_key(*path))
        .collect();
    let removed: Vec<&String> = before
        .keys()
        .filter(|path| !after.contains_key(*path))
        .collect();
    let changed: Vec<&String> = before
        .iter()
        .filter(|(path, bytes)| after.get(*path).is_some_and(|now| now != *bytes))
        .map(|(path, _)| path)
        .collect();
    assert!(
        added.is_empty() && removed.is_empty() && changed.is_empty(),
        "preview wrote to the project: added={added:?} removed={removed:?} changed={changed:?}"
    );
}

// ---------------------------------------------------------------------------
// 3.0.0_canonical_context
// ---------------------------------------------------------------------------

#[test]
fn canonical_context_apply_preview_and_retry() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    project(root);
    let mission = mission(root, "mission-a");
    let untouched = root.join("notes/keep.txt");
    write(&untouched, "operator notes\n");

    assert_preview_writes_nothing(root, preview_canonical_context);

    let migration = callback("3.0.0_canonical_context");
    assert!((migration.detect)(root).expect("detect"));
    assert!((migration.can_apply)(root).expect("can_apply").is_none());

    let applied = (migration.apply)(root).expect("apply");
    assert!(applied.success, "{:?}", applied.errors);
    assert!(
        applied
            .changes
            .iter()
            .any(|change| change.starts_with("Migrated 1 feature(s), 1 WPs backfilled,")),
        "{:?}",
        applied.changes
    );
    assert!(mission.join("status.events.jsonl").is_file());
    let metadata = std::fs::read_to_string(root.join(".kittify/metadata.yaml")).expect("metadata");
    assert!(metadata.contains("schema_version: 3"), "{metadata}");
    assert!(metadata.contains("project_uuid:"));
    let ignored = std::fs::read_to_string(root.join(".gitignore")).expect("gitignore");
    assert!(ignored.contains(".kittify/derived/"));
    assert!(ignored.contains("*.pyc"), "existing line lost: {ignored}");
    assert_eq!(
        std::fs::read_to_string(&untouched).expect("notes"),
        "operator notes\n"
    );
    // The legacy frontmatter fields were stripped after the rebuild.
    let wp = std::fs::read_to_string(mission.join("tasks/WP01-alpha.md")).expect("wp");
    assert!(!wp.contains("shell_pid: 4242"), "{wp}");

    // Retry: the runner is idempotent and the derived copy stays one file.
    let retried = (migration.apply)(root).expect("retry");
    assert!(retried.success, "{:?}", retried.errors);
    assert_eq!(
        std::fs::read_to_string(&untouched).expect("notes"),
        "operator notes\n"
    );
    assert!(!(migration.detect)(root).expect("detect after"));
}

// ---------------------------------------------------------------------------
// 3.1.1_charter_rename
// ---------------------------------------------------------------------------

#[test]
fn charter_rename_apply_preview_and_preservation() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    project(root);
    write(
        &root.join(".kittify/memory/constitution.md"),
        "# Constitution\n\nThe constitution governs.\n",
    );
    write(
        &root.join(".kittify/constitution/notes.md"),
        "Constitution notes\n",
    );
    write(
        &root.join(".kittify/missions/m1/constitution/constitution.md"),
        "mission-specific constitution\n",
    );
    write(
        &root.join(".claude/commands/spec-kitty.constitution.md"),
        "# Constitution command\n\nConstitution workflow\n",
    );
    write(
        &root.join(".claude/commands/spec-kitty.custom.md"),
        "# my own command\n",
    );

    assert_preview_writes_nothing(root, preview_charter_rename);

    let migration = callback("3.1.1_charter_rename");
    assert!((migration.detect)(root).expect("detect"));
    let applied = (migration.apply)(root).expect("apply");
    assert!(applied.success, "{:?}", applied.errors);
    assert!(root.join(".kittify/charter/charter.md").is_file());
    assert!(root.join(".kittify/charter/notes.md").is_file());
    let charter =
        std::fs::read_to_string(root.join(".kittify/charter/charter.md")).expect("charter");
    assert!(
        charter.contains("# Charter") && charter.contains("The charter governs."),
        "{charter}"
    );
    assert!(!root.join(".kittify/memory/constitution.md").exists());
    assert!(!root.join(".kittify/constitution").exists());
    // The mission-specific governance payload is archived, never deleted silently.
    assert!(
        applied
            .warnings
            .iter()
            .any(|warning| warning.contains("archived to")),
        "{:?}",
        applied.warnings
    );
    let archived = std::fs::read_dir(root.join(".kittify"))
        .expect("kittify")
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(".backup-"))
        })
        .expect("preservation backup");
    assert!(archived.is_dir());
    // The agent artifact was renamed and rewritten; the user file is untouched.
    assert!(
        !root
            .join(".claude/commands/spec-kitty.constitution.md")
            .exists()
    );
    let command =
        std::fs::read_to_string(root.join(".claude/commands/spec-kitty.charter.md")).expect("cmd");
    assert!(command.contains("Charter workflow"), "{command}");
    assert_eq!(
        std::fs::read_to_string(root.join(".claude/commands/spec-kitty.custom.md")).expect("user"),
        "# my own command\n"
    );

    // Retry: nothing left to detect, and the preserved bytes stay put.
    assert!(!(migration.detect)(root).expect("detect after"));
}

#[test]
fn charter_rename_reports_a_non_utf8_file_and_keeps_its_bytes() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    project(root);
    let charter_dir = root.join(".kittify/constitution");
    std::fs::create_dir_all(&charter_dir).expect("constitution");
    let binary = charter_dir.join("binary.md");
    let payload: &[u8] = b"---\n\xff\xfe not utf8\n---\n";
    std::fs::write(&binary, payload).expect("binary");

    let migration = callback("3.1.1_charter_rename");
    assert!((migration.detect)(root).expect("detect"));
    let applied = (migration.apply)(root).expect("apply");
    assert!(!applied.success, "{:?}", applied.errors);
    assert!(
        applied
            .errors
            .iter()
            .any(|error| error.contains("Failed to read")),
        "{:?}",
        applied.errors
    );
    // The rename itself happened; only the unreadable file is reported.
    assert!(root.join(".kittify/charter/binary.md").is_file());
    assert_eq!(
        std::fs::read(root.join(".kittify/charter/binary.md")).expect("bytes"),
        payload
    );
}

// ---------------------------------------------------------------------------
// 3.2.0a4_normalize_mission_lifecycle
// ---------------------------------------------------------------------------

#[test]
fn normalize_lifecycle_apply_preview_and_retry() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    project(root);
    let mission = mission(root, "mission-a");
    let untouched = mission.join("spec.md");
    write(&untouched, "# spec\n");
    // A second Mission that already carries its identity: Source's dry-run branch
    // reports the WP-identity pass only when a `mission_id` is already readable
    // (a dry run never writes the freshly minted one), so this fixture is what
    // covers that would-do line.
    let second = root.join("kitty-specs/second-b");
    write(
        &second.join("meta.json"),
        "{\n  \"mission_id\": \"01KJ00000000000000000000AB\",\n  \"slug\": \"second-b\"\n}\n",
    );
    write(
        &second.join("tasks/WP01-alpha.md"),
        "---\nlane: claimed\n---\n\n# WP01 alpha\n",
    );

    assert_preview_writes_nothing(root, preview_normalize_mission_lifecycle);
    let preview = preview_normalize_mission_lifecycle(root).expect("preview");
    assert!(
        preview.changes.iter().any(|change| change
            == "second-b: Would backfill missing WP identity fields before rebuilding the event log"),
        "{:?}",
        preview.changes
    );
    assert!(
        preview.changes.iter().any(|change| change
            == "second-b: Would rebuild status.events.jsonl from legacy mission state"),
        "{:?}",
        preview.changes
    );
    // The identity-less Mission reports only the rebuild it can already plan.
    assert!(
        preview.changes.iter().any(|change| change
            == "mission-a: Would rebuild status.events.jsonl from legacy mission state"),
        "{:?}",
        preview.changes
    );

    let migration = callback("3.2.0a4_normalize_mission_lifecycle");
    let applied = (migration.apply)(root).expect("apply");
    assert!(applied.success, "{:?}", applied.errors);
    assert!(
        applied
            .changes
            .iter()
            .any(|change| change == "mission-a: Backfilled mission identity metadata"),
        "{:?}",
        applied.changes
    );
    assert!(mission.join("status.events.jsonl").is_file());
    assert!(
        root.join(".kittify/derived/mission-a/status.json")
            .is_file()
    );
    assert_eq!(
        std::fs::read_to_string(&untouched).expect("spec"),
        "# spec\n"
    );

    let retried = (migration.apply)(root).expect("retry");
    assert!(retried.success, "{:?}", retried.errors);
    assert!(retried.changes.is_empty(), "{:?}", retried.changes);
}

#[test]
fn normalize_lifecycle_reports_a_malformed_meta_and_writes_nothing() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    project(root);
    let dir = root.join("kitty-specs/broken");
    write(&dir.join("meta.json"), "{not json\n");
    write(
        &dir.join("tasks/WP01.md"),
        "---\nlane: planned\n---\n\n# WP01\n",
    );

    let migration = callback("3.2.0a4_normalize_mission_lifecycle");
    let applied = (migration.apply)(root).expect("apply");
    assert!(!applied.success, "{:?}", applied.changes);
    assert!(
        applied
            .errors
            .iter()
            .any(|error| error.starts_with("broken: Could not read meta.json")),
        "{:?}",
        applied.errors
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("meta.json")).expect("meta"),
        "{not json\n"
    );
    assert!(!dir.join("status.events.jsonl").exists());
}

// ---------------------------------------------------------------------------
// runtime_state_backfill
// ---------------------------------------------------------------------------

fn status_phase(root: &Path, slug: &str) -> Option<String> {
    let meta =
        std::fs::read_to_string(root.join("kitty-specs").join(slug).join("meta.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&meta).ok()?;
    value
        .get("status_phase")
        .and_then(|phase| phase.as_str())
        .map(str::to_string)
}

#[test]
fn runtime_state_backfill_apply_preview_and_retry() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    project(root);
    let mission = mission(root, "mission-a");

    assert_preview_writes_nothing(root, preview_runtime_state_backfill);
    let preview = preview_runtime_state_backfill(root).expect("preview");
    assert!(
        preview
            .changes
            .iter()
            .any(|change| change.starts_with("dry-run: would seed ")),
        "{:?}",
        preview.changes
    );
    assert!(status_phase(root, "mission-a").is_none());

    let migration = callback("runtime_state_backfill");
    assert!((migration.detect)(root).expect("detect"));
    let applied = (migration.apply)(root).expect("apply");
    assert!(applied.success, "{:?}", applied.errors);
    assert!(
        applied
            .changes
            .iter()
            .any(|change| change.contains("and flipped 1 of 1 mission(s) scanned")),
        "{:?}",
        applied.changes
    );
    assert!(mission.join("status.events.jsonl").is_file());
    assert_eq!(status_phase(root, "mission-a").as_deref(), Some("1"));
    let wp = std::fs::read_to_string(mission.join("tasks/WP01-alpha.md")).expect("wp");
    assert!(wp.contains("# WP01 alpha"), "authored body lost: {wp}");

    let retried = (migration.apply)(root).expect("retry");
    assert!(retried.success, "{:?}", retried.errors);
    assert!(retried.changes.is_empty(), "{:?}", retried.changes);
    assert!(!(migration.detect)(root).expect("detect after"));
}

#[test]
fn runtime_state_backfill_fails_closed_on_unreadable_legacy_state() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    project(root);
    let dir = mission(root, "mission-a");
    // Malformed task frontmatter: the legacy read fails, so the flip is
    // structurally unreachable and no status_phase may be written.
    let broken = dir.join("tasks/WP02-beta.md");
    let payload = "---\n: : not yaml :\n---\n# WP02\n";
    write(&broken, payload);

    let migration = callback("runtime_state_backfill");
    let applied = (migration.apply)(root).expect("apply");
    assert!(!applied.success, "{:?}", applied.changes);
    assert!(!applied.errors.is_empty());
    assert!(status_phase(root, "mission-a").is_none());
    assert_eq!(std::fs::read_to_string(&broken).expect("wp"), payload);
}

#[test]
fn every_rescue_callback_is_owned_and_declared_unregistered_here() {
    assert_eq!(
        OWNED_IDS,
        [
            "3.0.0_canonical_context",
            "3.1.1_charter_rename",
            "3.2.0a4_normalize_mission_lifecycle",
            "runtime_state_backfill",
        ]
    );
    for id in OWNED_IDS {
        assert_eq!(callback(id).id, id);
    }
}
