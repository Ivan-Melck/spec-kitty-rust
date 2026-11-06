//! CF21 — the two withheld failure-carrier legs, exercised through the
//! PRE-EXISTING entry points (`preview_canonical_context`, the `3.0.0`
//! `NativeMigration::apply` row, `normalize_mission_lifecycle`, the
//! `rebuild_mission_event_log` seam and the `3.2.0a4` row).
//!
//! Source facts pinned here (read from the pinned reference before writing the
//! assertions):
//!
//! * `m_3_0_0_canonical_context.apply` wraps `run_migration` in a `try/except`
//!   and returns `MigrationResult(success=False,
//!   errors=["Migration failed with exception: <e>"])` — an uncaught exception
//!   arrives as a *reported* failure with **no** `changes_made`, **no**
//!   warnings (the in-progress report object is discarded) and **no** rollback
//!   and **no** backup cleanup (`_settle_failed_backup` runs only inside
//!   `_fail`).
//! * Source's `run_migration` enumerates `kitty-specs/` inside a `try` only for
//!   `backfill_mission_ids` (which catches and yields
//!   `[identity_backfill] mission ID backfill failed: <e>`); the later
//!   `for feature_dir in _discover_features(...)` walks sit OUTSIDE any `try`,
//!   so their enumeration error escapes to the `apply` wrapper.
//! * `rebuild_event_log` reports a log-write failure through
//!   `RebuildResult.errors` (`Failed to write event log for <slug>: <e>`) and
//!   returns the counts/warnings it already collected; it does not raise.
//!   `_normalize_event_log` then reports `status="error"` with
//!   `"; ".join(rebuild.errors)` and keeps `rebuild.warnings`; the runner's
//!   step 4 keeps going and appends `State rebuild warning for <slug>: <e>`.
//!
//! These are Native-only functional smokes on fresh fixtures. They are NOT
//! paired Source-isolation evidence and no equivalence/readiness claim follows.

use std::path::{Path, PathBuf};

use sk_install::migrations::completion_modern::rescue_spine::{
    IMPLEMENTATIONS, OWNED_IDS, preview_canonical_context,
};
use sk_install::upgrade::{MigrationOutcome, NativeMigration};
use sk_mission::migration_rescue::normalize_mission_lifecycle;

/// The owned row for `id`, looked up by id (never by index).
fn row(id: &str) -> &'static NativeMigration {
    let index = OWNED_IDS
        .iter()
        .position(|owned| *owned == id)
        .unwrap_or_else(|| panic!("{id} is owned by this module"));
    &IMPLEMENTATIONS[index]
}

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent");
    }
    std::fs::write(path, text).expect("write");
}

/// A schema-v2 project with one legacy mission whose WP frontmatter has `lane`.
fn legacy_project(root: &Path) -> PathBuf {
    write(
        &root.join(".kittify/metadata.yaml"),
        "spec_kitty:\n  schema_version: 2\n",
    );
    let mission = root.join("kitty-specs/001-legacy-mission");
    write(
        &mission.join("meta.json"),
        "{\n  \"mission_id\": \"01KNXQS9ATWWFXS3K5ZJ9E5002\",\n  \"slug\": \"001-legacy-mission\",\n  \"mission_number\": 1\n}\n",
    );
    write(&mission.join("spec.md"), "# spec\n");
    write(
        &mission.join("tasks/WP01-task.md"),
        "---\nwork_package_id: WP01\nlane: in_progress\ntitle: Task\n---\n\nbody\n",
    );
    mission
}

/// Make `kitty-specs/` unreadable and report whether that actually took effect.
fn unreadable_kitty_specs(root: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let specs = root.join("kitty-specs");
        std::fs::set_permissions(&specs, std::fs::Permissions::from_mode(0o000)).expect("chmod");
        // A privileged runner reads through 0o000; the assertions below bind the
        // unprivileged case only and are not weakened for it.
        std::fs::read_dir(&specs).is_err()
    }
    #[cfg(not(unix))]
    {
        let _ = root;
        false
    }
}

fn readable_again(root: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            root.join("kitty-specs"),
            std::fs::Permissions::from_mode(0o755),
        )
        .expect("restore permissions");
    }
    #[cfg(not(unix))]
    {
        let _ = root;
    }
}

#[test]
fn preview_reports_an_uncaught_enumeration_exception_as_a_carrier() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    legacy_project(root);
    if !unreadable_kitty_specs(root) {
        return;
    }

    // Source `apply(dry_run=True)`: the dry-run branch enumerates first and its
    // OSError is caught by the `apply` wrapper, not raised.
    let outcome: MigrationOutcome =
        preview_canonical_context(root).expect("Source returns a reported failure, not an error");
    readable_again(root);

    assert!(!outcome.success, "{outcome:?}");
    assert_eq!(outcome.errors.len(), 1, "{outcome:?}");
    assert!(
        outcome.errors[0].starts_with("Migration failed with exception: "),
        "{outcome:?}"
    );
    // The wrapper builds a FRESH result: the in-progress report is discarded.
    assert!(outcome.changes.is_empty(), "{outcome:?}");
    assert!(outcome.warnings.is_empty(), "{outcome:?}");
}

#[test]
fn apply_keeps_the_reference_step_identity_for_the_mission_id_enumeration() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    legacy_project(root);
    if !unreadable_kitty_specs(root) {
        return;
    }

    // `backfill_mission_ids` owns its own `try` in Source, so this is a RETURNED
    // failure: the step is `identity_backfill`, the message is the mission-ID
    // wording, and the earlier non-fatal backup warning survives.
    let outcome = (row("3.0.0_canonical_context").apply)(root).expect("returned carrier");
    readable_again(root);

    assert!(!outcome.success, "{outcome:?}");
    assert_eq!(outcome.errors.len(), 1, "{outcome:?}");
    assert!(
        outcome.errors[0].starts_with("[identity_backfill] mission ID backfill failed: "),
        "{outcome:?}"
    );
    assert_eq!(
        outcome.changes,
        vec!["Migration failed at step: identity_backfill".to_string()],
        "{outcome:?}"
    );
    assert!(
        outcome
            .warnings
            .iter()
            .any(|warning| warning == "Could not create backup — proceeding without safety net"),
        "{outcome:?}"
    );
}

/// A mission whose event-log write must fail with an ordinary `OSError`.
fn broken_log_mission(root: &Path, slug: &str, mission_id: &str) -> PathBuf {
    write(
        &root.join(".kittify/metadata.yaml"),
        "spec_kitty:\n  schema_version: 2\n",
    );
    let mission = root.join("kitty-specs").join(slug);
    write(
        &mission.join("meta.json"),
        &format!("{{\n  \"mission_id\": \"{mission_id}\",\n  \"slug\": \"{slug}\"\n}}\n"),
    );
    write(
        &mission.join("tasks/WP01-task.md"),
        "---\nwork_package_id: WP01\nlane: approved\ntitle: Task\n---\n\nbody\n",
    );
    // Source writes the fixed `<name>.tmp` sibling; a directory there makes the
    // `open("w")` fail with an ordinary OSError that Source REPORTS.
    std::fs::create_dir_all(mission.join("status.events.jsonl.tmp")).expect("tmp dir");
    mission
}

#[test]
fn a_rebuild_write_failure_is_reported_not_raised() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let slug = "002-broken-log";
    let mission = broken_log_mission(root, slug, "01KNXQS9ATWWFXS3K5ZJ9E5003");

    // The normalizer joins the reported rebuild errors with `"; "` and KEEPS the
    // rebuild's own warnings; the mission is `error`, so its actions never
    // become `changes_made`.
    let outcomes = normalize_mission_lifecycle(root, false);
    let outcome = outcomes
        .iter()
        .find(|outcome| outcome.slug == slug)
        .expect("mission visited");
    assert_eq!(outcome.status, "error", "{outcome:?}");
    let error = outcome.error.as_deref().unwrap_or_default();
    assert!(
        error.starts_with(&format!("Failed to write event log for {slug}: ")),
        "{outcome:?}"
    );
    assert!(!mission.join("status.events.jsonl").exists());

    let carrier = (row("3.2.0a4_normalize_mission_lifecycle").apply)(root).expect("carrier");
    assert!(!carrier.success, "{carrier:?}");
    assert_eq!(carrier.errors.len(), 1, "{carrier:?}");
    assert!(
        carrier.errors[0].starts_with(&format!("{slug}: Failed to write event log for {slug}: ")),
        "{carrier:?}"
    );
}

#[test]
fn a_controlled_charter_merge_failure_keeps_the_residual_directory() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    write(&root.join(".kittify/metadata.yaml"), "spec_kitty: {}\n");
    write(
        &root.join(".kittify/constitution/legacy.md"),
        "# legacy governance\n",
    );
    std::fs::create_dir_all(root.join(".kittify/charter")).expect("charter dir");
    // An unwritable destination directory is an ordinary OSError for the merge's
    // single `try` (Source: `shutil.move` -> PermissionError).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            root.join(".kittify/charter"),
            std::fs::Permissions::from_mode(0o500),
        )
        .expect("chmod charter");
    }
    // A privileged runner writes into 0o500; the assertions bind the unprivileged
    // case only and are not weakened for it.
    if std::fs::write(root.join(".kittify/charter/probe"), b"x").is_ok() {
        return;
    }

    let outcome = (row("3.1.1_charter_rename").apply)(root).expect("carrier");
    assert!(!outcome.success, "{outcome:?}");
    assert!(
        outcome
            .errors
            .iter()
            .any(|error| error.starts_with("Failed to merge constitution/ into charter/: ")),
        "{outcome:?}"
    );
    // The first failure leaves the residual directory in place: no silent success
    // and never a raw `rmtree` of an unmerged payload.
    assert!(root.join(".kittify/constitution/legacy.md").is_file());

    // A retry reports the same carrier (nothing was silently completed).
    let retry = (row("3.1.1_charter_rename").apply)(root).expect("carrier");
    assert!(!retry.success, "{retry:?}");
    assert!(
        retry
            .errors
            .iter()
            .any(|error| error.starts_with("Failed to merge constitution/ into charter/: ")),
        "{retry:?}"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            root.join(".kittify/charter"),
            std::fs::Permissions::from_mode(0o755),
        )
        .expect("restore charter");
    }
}

#[test]
fn the_runner_reports_a_rebuild_write_failure_as_a_warning_and_stays_successful() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let slug = "003-broken-log";
    broken_log_mission(root, slug, "01KNXQS9ATWWFXS3K5ZJ9E5004");

    let outcome = (row("3.0.0_canonical_context").apply)(root).expect("carrier");

    // Source step 4 turns a rebuild error into a WARNING
    // (`State rebuild warning for <slug>: <e>`); it is not a `_fail`, so the
    // run still succeeds and its summary is the ordinary one.
    assert!(outcome.success, "{outcome:?}");
    assert!(outcome.errors.is_empty(), "{outcome:?}");
    assert!(
        outcome
            .warnings
            .iter()
            .any(|warning| warning.starts_with(&format!(
                "State rebuild warning for {slug}: Failed to write event log for {slug}: "
            ))),
        "{outcome:?}"
    );
    assert!(
        outcome
            .changes
            .iter()
            .any(|change| change.starts_with("Migrated 1 feature(s)")),
        "{outcome:?}"
    );
}
