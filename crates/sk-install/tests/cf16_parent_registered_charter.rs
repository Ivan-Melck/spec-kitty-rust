//! Canonical registration/runner reachability; Native smoke, not Source pairing.

use sk_install::{
    MigrationCatalog, MigrationLedger, UpgradeRequest, native_migrations, run_upgrade,
};
use std::collections::BTreeSet;

const FOLD: &str = "consolidate_charter_bundle_fold";

#[test]
fn six_charter_callbacks_are_registered_once_with_catalogue_owned_scope() {
    let catalog = MigrationCatalog::embedded().expect("catalogue");
    let all: Vec<_> = native_migrations().collect();
    assert_eq!(all.len(), 132, "callable subset, not equivalence approval");
    for migration in sk_install::migrations::completion_modern::charter_synthesis::IMPLEMENTATIONS {
        assert_eq!(all.iter().filter(|item| item.id == migration.id).count(), 1);
        assert_eq!(
            migration.runs_on_worktrees().expect("scope"),
            catalog
                .get(migration.id)
                .expect("declared")
                .runs_on_worktrees
        );
    }
}

#[test]
fn real_runner_reaches_registered_fold_preview_then_apply() {
    let project = tempfile::tempdir().expect("project");
    let root = project.path();
    let git = std::process::Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(root)
        .output()
        .expect("real Git");
    assert!(git.status.success());
    std::fs::create_dir_all(root.join(".kittify/charter")).expect("charter dir");
    let config = root.join(".kittify/config.yaml");
    let metadata = root.join(".kittify/metadata.yaml");
    let directives = root.join(".kittify/charter/directives.yaml");
    let config_before = "agents:\n  available:\n  - claude\nactivated_tactics:\n- local-first\n";
    // The unsettled adoption backfill virtually settles history through the
    // recorded version. Keep this fixture BEFORE the fold target so it is
    // genuinely pending, rather than incorrectly expecting a settled id to run.
    let metadata_before = "spec_kitty:\n  schema_version: 3\n  version: 3.2.5\n  initialized_at: '2026-01-01T00:00:00+00:00'\n  last_upgraded_at:\n";
    std::fs::write(&config, config_before).expect("config");
    std::fs::write(&metadata, metadata_before).expect("metadata");
    std::fs::write(&directives, "directives: []\n").expect("legacy bundle");
    let request =
        UpgradeRequest::new(root, "4.0.0rc8").with_scope(BTreeSet::from([FOLD.to_string()]));
    let preview = run_upgrade(&request.clone().dry_run()).expect("registered actual preview");
    assert!(preview.success);
    assert_eq!(preview.selected.len(), 1);
    let changes = &preview
        .migration_results
        .get(FOLD)
        .expect("real callback outcome")
        .changes;
    assert_eq!(changes.len(), 3, "not an empty detect-only forecast");
    assert!(changes[0].starts_with("dry-run: would compose "));
    assert_eq!(
        std::fs::read_to_string(&config).expect("config"),
        config_before
    );
    assert_eq!(
        std::fs::read_to_string(&metadata).expect("metadata"),
        metadata_before
    );
    assert_eq!(
        std::fs::read_to_string(&directives).expect("bundle"),
        "directives: []\n"
    );
    assert!(!root.join(".kittify/charter/charter.yaml").exists());
    let ledger = MigrationLedger::new(root);
    assert!(ledger.records(false).expect("ledger").is_empty());
    assert!(ledger.records(true).expect("local ledger").is_empty());
    let applied = run_upgrade(&request).expect("registered actual apply");
    assert!(applied.success);
    assert!(root.join(".kittify/charter/charter.yaml").is_file());
    assert!(!directives.exists());
    assert_eq!(
        std::fs::read_to_string(config).expect("repointed config"),
        "agents:\n  available:\n  - claude\ncharter: .kittify/charter/charter.yaml\n"
    );
    assert!(
        ledger
            .records(false)
            .expect("recorded")
            .iter()
            .any(|row| row.id == FOLD && row.result == "success")
    );
}
