//! CF10 — the worktree upgrade execution context.
//!
//! These are **Native fixture tests**, not a Source differential: Source behaviour
//! was read from `src/specify_cli/upgrade/runner.py` (`_upgrade`,
//! `_upgrade_worktrees`, `_apply_migration`, `_reconcile_worktree_bookkeeping`)
//! and `version_stamp_boundary.py`, and each assertion below names the observed
//! Source rule it pins. No Python process runs here.
//!
//! What is covered:
//! * the inherited selection: the MAIN run's selected list, filtered by
//!   `runs_on_worktrees` in catalogue order, never a per-root reselection;
//! * the empty inherited scope: version/schema bookkeeping only, no synthesized
//!   state from a Mission-looking tree;
//! * a non-empty inherited subset: only worktree-scoped ids run, project-only ids
//!   are never evaluated or recorded;
//! * per-step phase semantics: detect error vs returned failure vs apply error,
//!   and which of them append a `failed` ledger record;
//! * the sticky `worktree_failed` / `worktree_errored` flags, the schema-stamp
//!   guard, and the shared `last_upgraded_at` bookkeeping stamp.

use std::path::{Path, PathBuf};
use std::time::Duration;

use sk_install::metadata::CURRENT_SCHEMA_VERSION;
use sk_install::upgrade::{
    StepStatus, evaluate_worktree_step, run_worktree_migrations, worktree_has_upgradeable_state,
    worktree_migration_scope,
};
use sk_install::{
    InstallError, MigrationCatalog, MigrationLedger, MigrationOutcome, MigrationSpec,
    NativeMigration, ProjectMetadata,
};

const MAIN_STAMP: &str = "2026-01-01T00:00:00+00:00";
const FALLBACK_STAMP: &str = "2026-02-02T00:00:00+00:00";
const TARGET: &str = "4.0.0rc8";

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent");
    }
    std::fs::write(path, text).expect("write");
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).expect("read")
}

fn git(dir: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
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
}

/// A real catalogue specification (the ONE declared catalogue).
fn spec(id: &str) -> MigrationSpec {
    MigrationCatalog::embedded()
        .expect("catalogue")
        .get(id)
        .unwrap_or_else(|| panic!("{id} is declared"))
        .clone()
}

/// A worktree metadata fixture at a recorded version.
fn project(version: &str, schema: Option<i64>) -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("tempdir");
    write(
        &directory.path().join(".kittify/config.yaml"),
        "agents:\n  available:\n    - claude\n",
    );
    let mut metadata = format!("spec_kitty:\n  version: \"{version}\"\n");
    if let Some(schema) = schema {
        metadata.push_str(&format!("  schema_version: {schema}\n"));
    }
    metadata.push_str("environment:\n  initialized_at: \"2025-01-01T00:00:00+00:00\"\n");
    write(&directory.path().join(".kittify/metadata.yaml"), &metadata);
    directory
}

fn records(root: &Path, spec: &MigrationSpec) -> Vec<String> {
    MigrationLedger::new(root)
        .records_for(spec)
        .expect("ledger")
        .into_iter()
        .map(|record| record.result)
        .collect()
}

// ---------------------------------------------------------------------------
// Inherited selection
// ---------------------------------------------------------------------------

#[test]
fn the_scope_is_the_inherited_selection_filtered_in_catalogue_order() {
    // Source `_upgrade_worktrees`: `[m for m in migrations if m.runs_on_worktrees]`.
    // Order is the inherited catalogue order, and a project-only id is dropped —
    // not re-selected, not treated as "unknown".
    let inherited = vec![
        spec("0.2.0_specify_to_kittify"),
        spec("4.0.0rc8_backfill_migration_ledger"),
        spec("0.13.1_exclude_worktrees"),
        spec("3.2.0rc35_unified_bundle"),
    ];
    let scope = worktree_migration_scope(&inherited).expect("scope");
    assert_eq!(
        scope
            .iter()
            .map(|spec| spec.id.as_str())
            .collect::<Vec<_>>(),
        vec!["0.2.0_specify_to_kittify", "0.13.1_exclude_worktrees"]
    );
    // A selected id with no catalogue row cannot be classified: a typed refusal,
    // never a silent project-only classification.
    let mut unknown = spec("0.13.1_exclude_worktrees");
    unknown.id = "cf10.unknown.migration".to_string();
    assert!(matches!(
        worktree_migration_scope(&[unknown]),
        Err(InstallError::MigrationCatalogInvalid { .. })
    ));
}

#[test]
fn an_empty_scope_counts_only_kittify_state() {
    // Source `has_upgradeable_state`: `.kittify/` always counts; Mission state only
    // while a worktree-scoped migration is actually in the inherited scope.
    let directory = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(directory.path().join("kitty-specs")).expect("kitty-specs");
    assert!(!worktree_has_upgradeable_state(directory.path(), &[]));
    assert!(worktree_has_upgradeable_state(
        directory.path(),
        &[spec("0.13.1_exclude_worktrees")]
    ));
    std::fs::create_dir_all(directory.path().join(".kittify")).expect("kittify");
    assert!(worktree_has_upgradeable_state(directory.path(), &[]));
}

// ---------------------------------------------------------------------------
// Empty inherited scope: bookkeeping only
// ---------------------------------------------------------------------------

#[test]
fn an_empty_inherited_scope_stamps_version_and_schema_only() {
    // Source `_upgrade_worktrees` with `migrations == []`: no migration reference
    // executed, no ledger record; `_reconcile_worktree_bookkeeping` advances the
    // version and aligns `last_upgraded_at` to the MAIN stamp, and (without a
    // returned failure) the schema stamp lands.
    let directory = project("3.2.0", Some(2));
    let execution = run_worktree_migrations(
        directory.path(),
        TARGET,
        TARGET,
        &[],
        false,
        Some(MAIN_STAMP),
        FALLBACK_STAMP,
        Duration::from_secs(5),
    )
    .expect("runs the empty scope");
    assert!(execution.rows.is_empty());
    assert!(!execution.failed());
    assert!(execution.metadata_written);
    assert!(
        !directory
            .path()
            .join(".kittify/migration-ledger.jsonl")
            .exists(),
        "the empty scope recorded a migration"
    );
    let metadata = ProjectMetadata::load(&directory.path().join(".kittify")).expect("metadata");
    assert_eq!(metadata.version, TARGET);
    assert_eq!(metadata.schema_version, Some(CURRENT_SCHEMA_VERSION));
    assert_eq!(metadata.last_upgraded_at.as_deref(), Some(MAIN_STAMP));
    assert!(read(&directory.path().join(".kittify/metadata.yaml")).contains("schema_capabilities"));

    // No main stamp: the ONE fallback minted before the sibling loop is used, so
    // sibling roots in one run cannot diverge on their bookkeeping stamp (#4972).
    let sibling = project("3.2.0", Some(2));
    run_worktree_migrations(
        sibling.path(),
        TARGET,
        TARGET,
        &[],
        false,
        None,
        FALLBACK_STAMP,
        Duration::from_secs(5),
    )
    .expect("runs the empty scope");
    let metadata = ProjectMetadata::load(&sibling.path().join(".kittify")).expect("metadata");
    assert_eq!(metadata.last_upgraded_at.as_deref(), Some(FALLBACK_STAMP));

    // A settled root is a byte-level no-op on a rerun.
    let before = std::fs::read(directory.path().join(".kittify/metadata.yaml")).expect("bytes");
    let rerun = run_worktree_migrations(
        directory.path(),
        TARGET,
        TARGET,
        &[],
        false,
        Some(MAIN_STAMP),
        FALLBACK_STAMP,
        Duration::from_secs(5),
    )
    .expect("rerun");
    assert!(!rerun.metadata_written);
    assert_eq!(
        std::fs::read(directory.path().join(".kittify/metadata.yaml")).expect("bytes"),
        before
    );
}

// ---------------------------------------------------------------------------
// Non-empty inherited subset
// ---------------------------------------------------------------------------

#[test]
fn a_non_empty_inherited_subset_runs_only_worktree_scoped_ids() {
    let directory = project("3.2.0", Some(2));
    git(directory.path(), &["init", "-q", "-b", "main"]);
    write(&directory.path().join(".git/info/exclude"), "# operator\n");
    // The caller passes the filtered worktree scope; a project-only id stays out
    // of it and is therefore never evaluated or recorded in a worktree.
    let scope = [spec("0.13.1_exclude_worktrees")];
    let execution = run_worktree_migrations(
        directory.path(),
        TARGET,
        TARGET,
        &scope,
        false,
        Some(MAIN_STAMP),
        FALLBACK_STAMP,
        Duration::from_secs(10),
    )
    .expect("runs the subset");
    assert_eq!(
        execution
            .rows
            .iter()
            .map(|row| (row.id.as_str(), row.result.as_str()))
            .collect::<Vec<_>>(),
        vec![("0.13.1_exclude_worktrees", "applied")]
    );
    assert!(!execution.failed());
    let exclude = read(&directory.path().join(".git/info/exclude"));
    assert!(exclude.contains(".worktrees/"), "{exclude}");
    assert!(exclude.starts_with("# operator\n"), "{exclude}");
    assert_eq!(
        records(directory.path(), &spec("0.13.1_exclude_worktrees")),
        vec!["success"]
    );
    assert!(
        records(
            directory.path(),
            &spec("4.0.0rc8_backfill_migration_ledger")
        )
        .is_empty(),
        "a project-only id was evaluated in a worktree"
    );
    let metadata = ProjectMetadata::load(&directory.path().join(".kittify")).expect("metadata");
    assert_eq!(metadata.version, TARGET);
    assert_eq!(metadata.schema_version, Some(CURRENT_SCHEMA_VERSION));
    // A genuine change (a migration applied) mints a fresh stamp rather than
    // aligning to the main one.
    assert_ne!(metadata.last_upgraded_at.as_deref(), Some(MAIN_STAMP));

    // Running it again records the settled skip state without re-applying: the
    // second run reports `skipped` and still leaves the exclude file untouched.
    let second = run_worktree_migrations(
        directory.path(),
        TARGET,
        TARGET,
        &scope,
        false,
        Some(MAIN_STAMP),
        FALLBACK_STAMP,
        Duration::from_secs(10),
    )
    .expect("second run");
    assert_eq!(second.rows[0].result, "skipped");
    assert_eq!(read(&directory.path().join(".git/info/exclude")), exclude);
}

// ---------------------------------------------------------------------------
// Per-step phase semantics (shared evaluator, worktree mode)
// ---------------------------------------------------------------------------

fn fixture_spec(id: &str) -> MigrationSpec {
    MigrationSpec {
        id: id.to_string(),
        identity: id.to_string(),
        module: "cf10_test_fixture".to_string(),
        target_version: TARGET.to_string(),
        clone_local: false,
        requires_git_repository: false,
        runs_on_worktrees: true,
        superseded_by: String::new(),
        content_hash: "sha256:cf10-test".to_string(),
    }
}

fn native(
    id: &'static str,
    detect: fn(&Path) -> Result<bool, InstallError>,
    apply: fn(&Path) -> Result<MigrationOutcome, InstallError>,
) -> NativeMigration {
    fn no_refusal(_project_root: &Path) -> Result<Option<String>, InstallError> {
        Ok(None)
    }
    NativeMigration {
        id,
        detect,
        can_apply: no_refusal,
        apply,
    }
}

fn detect_needed(_root: &Path) -> Result<bool, InstallError> {
    Ok(true)
}

fn detect_error(_root: &Path) -> Result<bool, InstallError> {
    Err(InstallError::IgnoreFileUnsafe {
        path: PathBuf::from(".gitignore"),
        detail: "is a symlink".to_string(),
    })
}

fn apply_ok(_root: &Path) -> Result<MigrationOutcome, InstallError> {
    Ok(MigrationOutcome::changed(vec!["did the work".to_string()]))
}

fn apply_returned_failure(_root: &Path) -> Result<MigrationOutcome, InstallError> {
    Ok(MigrationOutcome {
        success: false,
        errors: vec!["the work could not be completed".to_string()],
        ..MigrationOutcome::default()
    })
}

fn apply_ignore_error(_root: &Path) -> Result<MigrationOutcome, InstallError> {
    Err(InstallError::IgnoreFileUnsafe {
        path: PathBuf::from(".gitignore"),
        detail: "is a symlink".to_string(),
    })
}

fn apply_other_error(_root: &Path) -> Result<MigrationOutcome, InstallError> {
    Err(InstallError::UpgradeRefused {
        detail: "a git probe failed".to_string(),
    })
}

fn step(
    root: &Path,
    spec: &MigrationSpec,
    native: &NativeMigration,
    dry_run: bool,
) -> (sk_install::upgrade::WorktreeStep, Vec<String>) {
    let ledger = MigrationLedger::new(root);
    let step = evaluate_worktree_step(root, spec, native, &ledger, TARGET, dry_run)
        .expect("evaluates the step");
    (step, records(root, spec))
}

#[test]
fn a_detect_error_is_a_recorded_failure_and_is_flagged_errored() {
    let directory = project("3.2.0", Some(2));
    let spec = fixture_spec("cf10.test.detect_error");
    let (step, records) = step(
        directory.path(),
        &spec,
        &native("cf10.test.detect_error", detect_error, apply_ok),
        false,
    );
    assert_eq!(step.status, StepStatus::Failed);
    assert!(step.errored);
    assert!(step.recorded);
    assert!(
        step.failure
            .expect("failure line")
            .contains("Cannot safely detect")
    );
    // Source's detect-error branch DOES record a `failed` result (#5746).
    assert_eq!(records, vec!["failed"]);
}

#[test]
fn a_returned_failure_is_recorded_but_not_flagged_errored() {
    let directory = project("3.2.0", Some(2));
    let spec = fixture_spec("cf10.test.returned_failure");
    let (step, records) = step(
        directory.path(),
        &spec,
        &native(
            "cf10.test.returned_failure",
            detect_needed,
            apply_returned_failure,
        ),
        false,
    );
    assert_eq!(step.status, StepStatus::Failed);
    assert!(
        !step.errored,
        "a returned failure is not a caught exception"
    );
    assert!(step.recorded);
    assert_eq!(
        step.failure.as_deref(),
        Some("the work could not be completed")
    );
    assert_eq!(records, vec!["failed"]);
}

#[test]
fn an_apply_ignore_path_error_is_errored_and_appends_no_failed_record() {
    // Source's apply() catch for `GitignorePathError` / `MigrationStateUnreadableError`
    // returns before the record step: sticky `errored`, no ledger record.
    let directory = project("3.2.0", Some(2));
    let spec = fixture_spec("cf10.test.apply_ignore_error");
    let (step, records) = step(
        directory.path(),
        &spec,
        &native(
            "cf10.test.apply_ignore_error",
            detect_needed,
            apply_ignore_error,
        ),
        false,
    );
    assert_eq!(step.status, StepStatus::Failed);
    assert!(step.errored);
    assert!(!step.recorded);
    assert!(
        records.is_empty(),
        "an ignore-path error recorded {records:?}"
    );
}

#[test]
fn another_apply_error_takes_the_recorded_git_error_path() {
    // Source converts `GitCommandError` into a returned failed result and records it;
    // Native's single error enum cannot prove that class, so every other apply error
    // stays a recorded, retryable failure (never a silent skip).
    let directory = project("3.2.0", Some(2));
    let spec = fixture_spec("cf10.test.apply_other_error");
    let (step, records) = step(
        directory.path(),
        &spec,
        &native(
            "cf10.test.apply_other_error",
            detect_needed,
            apply_other_error,
        ),
        false,
    );
    assert_eq!(step.status, StepStatus::Failed);
    assert!(!step.errored);
    assert!(step.recorded);
    assert_eq!(records, vec!["failed"]);
}

#[test]
fn a_dry_run_reports_the_verdict_and_records_nothing() {
    let directory = project("3.2.0", Some(2));
    let spec = fixture_spec("cf10.test.dry_run");
    let (step, records) = step(
        directory.path(),
        &spec,
        &native("cf10.test.dry_run", detect_needed, apply_ok),
        true,
    );
    assert_eq!(step.status, StepStatus::Applied);
    assert!(!step.recorded);
    assert!(records.is_empty());
}

#[test]
fn a_successful_step_records_success_and_keeps_review_paths() {
    let directory = project("3.2.0", Some(2));
    let spec = fixture_spec("cf10.test.success");
    let (step, records) = step(
        directory.path(),
        &spec,
        &native("cf10.test.success", detect_needed, apply_ok),
        false,
    );
    assert_eq!(step.status, StepStatus::Applied);
    assert!(!step.errored);
    assert!(step.recorded);
    assert_eq!(records, vec!["success"]);
}

// ---------------------------------------------------------------------------
// Root-level flags and continuation
// ---------------------------------------------------------------------------

#[test]
fn a_detect_error_is_recorded_and_the_next_worktree_migration_still_runs() {
    // Source `_upgrade_worktrees`: a caught detection error appends `failed` and
    // CONTINUES with the next inherited id; the root's later success does not clear
    // the sticky `errored` flag, and only `worktree_failed` (a returned failure)
    // suppresses the schema stamp.
    let directory = project("3.2.0", Some(2));
    git(directory.path(), &["init", "-q", "-b", "main"]);
    write(&directory.path().join(".gitignore"), "target/\n");
    // Non-UTF-8 exclude bytes: `0.13.1`'s detect() cannot read it, the deterministic
    // stand-in for Source's `GitignorePathError`.
    std::fs::write(
        directory.path().join(".git/info/exclude"),
        [0xff, 0xfe, b'\n'],
    )
    .expect("invalid exclude");
    let scope = vec![
        spec("0.13.1_exclude_worktrees"),
        spec("3.2.6rc3_worktrees_gitignore_backfill"),
    ];
    let execution = run_worktree_migrations(
        directory.path(),
        TARGET,
        TARGET,
        &scope,
        false,
        Some(MAIN_STAMP),
        FALLBACK_STAMP,
        Duration::from_secs(10),
    )
    .expect("runs the root");
    assert_eq!(
        execution
            .rows
            .iter()
            .map(|row| (row.id.as_str(), row.result.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("0.13.1_exclude_worktrees", "failed"),
            ("3.2.6rc3_worktrees_gitignore_backfill", "applied"),
        ]
    );
    assert!(execution.worktree_errored);
    assert!(!execution.worktree_failed);
    assert!(
        execution
            .failures
            .iter()
            .any(|failure| failure.contains("Cannot safely detect 0.13.1_exclude_worktrees")),
        "failures: {:?}",
        execution.failures
    );
    assert_eq!(
        records(directory.path(), &spec("0.13.1_exclude_worktrees")),
        vec!["failed"]
    );
    assert_eq!(
        records(
            directory.path(),
            &spec("3.2.6rc3_worktrees_gitignore_backfill")
        ),
        vec!["success"]
    );
    assert!(read(&directory.path().join(".gitignore")).contains(".worktrees/"));
    let metadata = ProjectMetadata::load(&directory.path().join(".kittify")).expect("metadata");
    assert_eq!(metadata.version, TARGET);
    // errored alone follows Source's schema rule: the stamp still lands.
    assert_eq!(metadata.schema_version, Some(CURRENT_SCHEMA_VERSION));
}

#[test]
fn a_dry_run_root_reports_the_prediction_and_writes_nothing() {
    // Source's worktree dry run evaluates the same guards and writes nothing; the
    // preview path (CLI `plan_worktree_upgrade`) uses this mode so it can never
    // describe a mutation it does not perform.
    let directory = project("3.2.0", Some(2));
    git(directory.path(), &["init", "-q", "-b", "main"]);
    write(&directory.path().join(".git/info/exclude"), "# operator\n");
    let metadata_path = directory.path().join(".kittify/metadata.yaml");
    let before = read(&metadata_path);
    let exclude_path = directory.path().join(".git/info/exclude");
    let exclude_before = read(&exclude_path);
    let scope = [spec("0.13.1_exclude_worktrees")];
    let execution = run_worktree_migrations(
        directory.path(),
        TARGET,
        TARGET,
        &scope,
        true,
        Some(MAIN_STAMP),
        FALLBACK_STAMP,
        Duration::from_secs(5),
    )
    .expect("previews the root");
    assert_eq!(
        execution
            .rows
            .iter()
            .map(|row| (row.id.as_str(), row.result.as_str()))
            .collect::<Vec<_>>(),
        vec![("0.13.1_exclude_worktrees", "applied")]
    );
    assert!(
        execution.metadata_written,
        "the prediction missed the stamp"
    );
    assert_eq!(read(&metadata_path), before, "the preview wrote metadata");
    assert_eq!(
        read(&exclude_path),
        exclude_before,
        "the preview wrote a file"
    );
    assert!(
        !directory.path().join(".kittify/migrations").exists(),
        "the preview recorded a ledger entry"
    );
    assert!(records(directory.path(), &scope[0]).is_empty());
}

#[test]
fn a_project_only_id_cannot_be_smuggled_into_the_worktree_scope() {
    let directory = project("3.2.0", Some(2));
    let error = run_worktree_migrations(
        directory.path(),
        TARGET,
        TARGET,
        &[spec("3.2.0rc35_unified_bundle")],
        false,
        None,
        FALLBACK_STAMP,
        Duration::from_secs(5),
    )
    .expect_err("project-only id refused");
    assert!(
        error.to_string().contains("project-only migration"),
        "{error}"
    );
}
