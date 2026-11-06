//! End-to-end upgrade paths for the `.gitignore` family and the declared no-ops.
//!
//! The differential cases in `migrations_gitignore_oracle.rs` pin each
//! migration's own verdicts. These tests pin the *runner* contracts those
//! migrations rely on:
//!
//! - a superseded id is recorded `skipped` **once**, carrying its successor
//!   note, and a re-run appends nothing (a `skipped` record does not settle an
//!   id, so it is selected again on every run — that must stay idempotent);
//! - a guard that cannot evaluate the project is a recorded `failed` result
//!   that stops the run and leaves the version untouched, never a silent skip;
//! - a selection made only of implemented ids applies in catalog order,
//!   advances the project version, and settles both partitions correctly.

use std::path::Path;

use sk_install::{
    LedgerRecord, MigrationCatalog, MigrationLedger, ProjectMetadata, SETTLED_RESULT,
    UpgradeRequest, run_upgrade,
};

const REPORTED_FAMILY: [&str; 10] = [
    "0.4.8_gitignore_agents",
    "0.12.1_remove_kitty_specs_from_gitignore",
    "2.0.0a5_remove_wp_status_gitignore_rule",
    "2.0.9_state_gitignore",
    "3.2.0rc35_kittify_runtime_git_hygiene",
    "3.2.3_encoding_provenance_gitignore_backfill",
    "3.2.4_derived_mission_views_gitignore_backfill",
    "3.2.4_runtime_dirs_gitignore_backfill",
    "3.2.6rc3_lint_report_gitignore_backfill",
    "3.2.6rc3_worktrees_gitignore_backfill",
];

const RETIRED_FAMILY: [&str; 16] = [
    "0.9.0_frontmatter_only_lanes",
    "0.9.1_complete_migration",
    "0.10.12_charter_cleanup",
    "2.0.0_charter_directory",
    "2.0.2_charter_context_bootstrap",
    "2.0.6_consistency_sweep",
    "2.1.2_fix_charter_doctrine_skill",
    "2.1.2_fix_glossary_context_skill",
    "2.1.4_enforce_command_file_state",
    "3.0.2_restore_prompt_commands",
    "3.1.1_normalize_status_json",
    "3.2.0rc35_default_charter_pack",
    "3.2.5_agents_skills_gitignore_backfill",
    "3.2.6rc3_narrow_cursor_gitignore",
    "normalize_activation_absence",
    "unify_charter_activation_promote_answers",
];

fn initialized(root: &Path, version: &str) {
    std::fs::create_dir_all(root.join(".kittify")).unwrap();
    std::fs::write(
        root.join(".kittify/config.yaml"),
        "agents:\n  available:\n  - claude\n",
    )
    .unwrap();
    std::fs::write(
        root.join(".kittify/metadata.yaml"),
        format!(
            "# Spec Kitty Project Metadata\nspec_kitty:\n  schema_version: 3\n  version: {version}\n  initialized_at: '2026-01-01T00:00:00+00:00'\n  last_upgraded_at:\n"
        ),
    )
    .unwrap();
}

fn seed_all_except(root: &Path, version: &str, unseeded: &[&str]) {
    let catalog = MigrationCatalog::embedded().expect("catalog");
    let records: Vec<LedgerRecord> = catalog
        .specs()
        .iter()
        .filter(|spec| !unseeded.contains(&spec.id.as_str()))
        .map(|spec| LedgerRecord::for_spec(spec, SETTLED_RESULT, version))
        .collect();
    MigrationLedger::new(root)
        .append_many(&records)
        .expect("seed");
}

fn records(root: &Path, clone_local: bool) -> Vec<LedgerRecord> {
    MigrationLedger::new(root)
        .records(clone_local)
        .expect("records")
}

#[test]
fn the_whole_reported_family_has_a_native_implementation() {
    let catalog = MigrationCatalog::embedded().expect("catalog");
    for id in REPORTED_FAMILY.iter().chain(RETIRED_FAMILY.iter()) {
        assert!(catalog.get(id).is_some(), "{id} is not declared");
        assert!(
            sk_install::NATIVE_MIGRATIONS
                .iter()
                .any(|migration| migration.id == *id),
            "{id} is not native"
        );
    }
}

#[test]
fn a_complete_native_path_applies_the_gitignore_family_in_catalog_order() {
    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path();
    initialized(root, "4.0.0rc7");
    let unseeded: Vec<&str> = REPORTED_FAMILY
        .iter()
        .chain(RETIRED_FAMILY.iter())
        .copied()
        .collect();
    seed_all_except(root, "4.0.0rc7", &unseeded);
    // A stale whole-directory rule and a retired WP-status rule, so both
    // removal migrations have real work: the first drops `kitty-specs/`, the
    // second drops the worktree-scoped `tasks/*.md` rule the first must keep.
    std::fs::write(
        root.join(".gitignore"),
        "node_modules/\nkitty-specs/\nkitty-specs/**/tasks/*.md\n",
    )
    .unwrap();

    let report = run_upgrade(&UpgradeRequest::new(root, "4.0.0rc8")).expect("upgrade");
    assert!(report.success);
    assert_eq!(report.from_version, "4.0.0rc7");
    assert_eq!(report.to_version, "4.0.0rc8");
    // The runner evaluates each guard in catalogue order, AFTER the previous migration's
    // effects (Source `_apply_migration`). `3.2.3_encoding_provenance_gitignore_backfill`
    // is a verbatim delegate of `3.2.0rc35_kittify_runtime_git_hygiene` (same `detect()`
    // and same `apply()`), so once the earlier migration has written those entries the
    // delegate's `detect()` is false at its own turn and Source records it `skipped`
    // ("Not applicable") instead of re-applying an identical repair. This expectation is
    // the sequential Source verdict, not a relaxation: the effect assertions below and
    // the per-record assertions still prove the project really received the repair.
    let delegated = "3.2.3_encoding_provenance_gitignore_backfill";
    let expected: Vec<String> = MigrationCatalog::embedded()
        .unwrap()
        .specs()
        .iter()
        .filter(|spec| REPORTED_FAMILY.contains(&spec.id.as_str()) && spec.id.as_str() != delegated)
        .map(|spec| spec.id.clone())
        .collect();
    assert_eq!(report.applied(), expected, "catalog order preserved");
    let mut skipped = report.skipped();
    skipped.sort();
    let mut expected_skipped: Vec<String> = RETIRED_FAMILY
        .iter()
        .map(|id| (*id).to_string())
        .chain(std::iter::once(delegated.to_string()))
        .collect();
    expected_skipped.sort();
    assert_eq!(skipped, expected_skipped);
    assert_eq!(
        ProjectMetadata::load(&root.join(".kittify"))
            .expect("metadata")
            .version,
        "4.0.0rc8"
    );

    // The project really did receive the effects, not just ledger records.
    let ignore = std::fs::read_to_string(root.join(".gitignore")).expect(".gitignore");
    assert!(ignore.contains(".kittify/runtime/"));
    assert!(ignore.contains(".kittify/lint-report.json"));
    assert!(ignore.contains(".worktrees/"));
    assert!(ignore.contains(".claude/"));
    assert!(
        !ignore.lines().any(|line| line.trim() == "kitty-specs/"),
        "the whole-directory rule is gone: {ignore}"
    );
    assert!(
        !ignore.contains("kitty-specs/**/tasks/*.md"),
        "the retired WP-status rule is gone: {ignore}"
    );
    assert!(ignore.starts_with("node_modules/\n"), "{ignore}");

    let before = records(root, false).len();
    let again = run_upgrade(&UpgradeRequest::new(root, "4.0.0rc8")).expect("second upgrade");
    assert!(again.applied().is_empty());
    assert_eq!(
        records(root, false).len(),
        before,
        "a re-run must not grow the ledger"
    );
    assert!(
        std::fs::read_to_string(root.join(".gitignore"))
            .unwrap()
            .contains(".kittify/runtime/")
    );
}

#[test]
fn a_superseded_record_carries_its_note_and_is_written_once() {
    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path();
    initialized(root, "4.0.0rc7");
    let id = "3.2.5_agents_skills_gitignore_backfill";
    seed_all_except(root, "4.0.0rc7", &[id]);

    let report = run_upgrade(&UpgradeRequest::new(root, "4.0.0rc8")).expect("upgrade");
    assert!(report.applied().is_empty());
    assert_eq!(report.skipped(), vec![id.to_string()]);
    let ledger_path = MigrationLedger::new(root).tracked_path();
    let text = std::fs::read_to_string(&ledger_path).expect("ledger");
    let line = text
        .lines()
        .find(|line| line.contains(id))
        .expect("superseded record");
    assert!(line.contains("\"result\": \"skipped\""), "{line}");
    assert!(
        line.contains("\"superseded_by\": \"Superseded by 4.0.0rc8_narrow_agent_gitignore\""),
        "{line}"
    );
    let count = records(root, false).len();
    // The no-op is re-selected on every run (a `skipped` record never settles
    // an id) and must record nothing the second time.
    let again = run_upgrade(&UpgradeRequest::new(root, "4.0.0rc8")).expect("second upgrade");
    assert_eq!(again.skipped(), vec![id.to_string()]);
    assert_eq!(records(root, false).len(), count);

    // A settled migration's record is note-free.
    let settled = text
        .lines()
        .find(|line| line.contains("\"4.0.0rc8_narrow_agent_gitignore\""))
        .expect("settled record");
    assert!(!settled.contains("superseded_by"), "{settled}");
}

#[test]
fn an_ordinary_skip_is_recorded_once_without_a_note() {
    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path();
    initialized(root, "4.0.0rc7");
    // Everything but one already-satisfied backfill is settled.
    let id = "2.0.9_state_gitignore";
    seed_all_except(root, "4.0.0rc7", &[id]);
    std::fs::write(
        root.join(".gitignore"),
        ".kittify/runtime/\n.kittify/merge-state.json\n.kittify/events/\n.kittify/dossiers/\nkitty-specs/**/.kittify/dossiers/\n",
    )
    .unwrap();
    let report = run_upgrade(&UpgradeRequest::new(root, "4.0.0rc8")).expect("upgrade");
    assert_eq!(report.skipped(), vec![id.to_string()]);
    let text = std::fs::read_to_string(MigrationLedger::new(root).tracked_path()).unwrap();
    let line = text.lines().find(|line| line.contains(id)).expect("record");
    assert!(line.contains("\"result\": \"skipped\""), "{line}");
    assert!(!line.contains("superseded_by"), "{line}");
    let count = records(root, false).len();
    run_upgrade(&UpgradeRequest::new(root, "4.0.0rc8")).expect("rerun");
    assert_eq!(records(root, false).len(), count);
}

#[cfg(unix)]
#[test]
fn an_unevaluable_project_is_a_recorded_failure_that_does_not_advance_the_version() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path();
    initialized(root, "4.0.0rc7");
    // `2.0.9_state_gitignore` reads through the strict helper, which refuses a
    // symlinked ignore file. Upstream records that as a failure and leaves the
    // project version alone, so the next run retries.
    let id = "2.0.9_state_gitignore";
    seed_all_except(root, "4.0.0rc7", &[id]);
    std::fs::write(root.join("elsewhere"), "keep me\n").unwrap();
    symlink("elsewhere", root.join(".gitignore")).unwrap();

    // CF16: Source `_detect_failure` RETURNS a failed result for this guard class, so
    // the run ends with a report (stamp restored) rather than a raised error.
    let report = run_upgrade(&UpgradeRequest::new(root, "4.0.0rc8")).expect("report");
    assert!(!report.success && report.failed);
    assert!(
        report
            .errors
            .iter()
            .any(|error| error.starts_with("Cannot safely detect 2.0.9_state_gitignore: ")),
        "{:?}",
        report.errors
    );
    assert_eq!(
        std::fs::read_to_string(root.join("elsewhere")).unwrap(),
        "keep me\n",
        "the symlink target was never written through"
    );
    assert_eq!(
        ProjectMetadata::load(&root.join(".kittify"))
            .expect("metadata")
            .version,
        "4.0.0rc7",
        "a failed run must not advance the version"
    );
    let record = records(root, false)
        .into_iter()
        .find(|record| record.id == id)
        .expect("failure record");
    assert_eq!(record.result, "failed");
}

#[test]
fn an_unimplemented_selection_still_refuses_before_any_family_write() {
    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path();
    initialized(root, "0.1.0");
    let error = run_upgrade(&UpgradeRequest::new(root, "4.0.0rc8")).expect_err("refused");
    assert_eq!(error.code(), "migration_not_implemented");
    assert!(!root.join(".gitignore").exists());
    assert!(records(root, false).is_empty());
}
