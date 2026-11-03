//! Real 4.x upgrade paths from realistic applied ledgers.
//!
//! The ledger is the selection authority (declared minus applied), so a
//! project's actual applied history is the input. These tests seed every
//! declared migration except the ones the path should select, then run the
//! native `run_upgrade` end to end against a temporary project.

use std::path::Path;
use std::process::Command;

use sk_install::{
    LedgerRecord, MigrationCatalog, MigrationLedger, ProjectMetadata, SETTLED_RESULT,
    UpgradeRequest, plan_upgrade, run_upgrade,
};

fn git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

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

fn git_repo(root: &Path) {
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["config", "user.email", "t@example.com"]);
    git(root, &["config", "user.name", "Test"]);
    std::fs::create_dir_all(root.join(".git/info")).unwrap();
    std::fs::write(root.join(".git/info/exclude"), "# git exclude\n").unwrap();
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

fn seed_tracked(root: &Path, version: &str) {
    let catalog = MigrationCatalog::embedded().expect("catalog");
    let records: Vec<LedgerRecord> = catalog
        .specs()
        .iter()
        .filter(|spec| !spec.clone_local)
        .map(|spec| LedgerRecord::for_spec(spec, SETTLED_RESULT, version))
        .collect();
    MigrationLedger::new(root)
        .append_many(&records)
        .expect("seed tracked");
}

fn rc6_to_rc7_project() -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path();
    initialized(root, "4.0.0rc6");
    git_repo(root);
    std::fs::create_dir_all(root.join(".kittify/charter-packs")).unwrap();
    std::fs::write(
        root.join(".kittify/charter-packs/graph.yaml"),
        "nodes: []\n",
    )
    .unwrap();
    std::fs::write(
        root.join(".gitignore"),
        ".kittify/charter-packs/**\n!.kittify/charter-packs/graph.yaml\n",
    )
    .unwrap();
    seed_all_except(
        root,
        "4.0.0rc6",
        &[
            "4.0.0rc7_exclude_spec_kitty",
            "4.0.0rc7_pack_level_backfill",
            "4.0.0rc7_track_charter_pack_yaml",
        ],
    );
    directory
}

#[test]
fn rc6_to_rc7_is_a_complete_native_path() {
    let directory = rc6_to_rc7_project();
    let root = directory.path();
    let report = run_upgrade(&UpgradeRequest::new(root, "4.0.0rc7")).expect("upgrade");
    assert!(report.success);
    assert_eq!(report.from_version, "4.0.0rc6");
    assert_eq!(report.to_version, "4.0.0rc7");
    assert_eq!(
        report.applied(),
        vec![
            "4.0.0rc7_exclude_spec_kitty".to_string(),
            "4.0.0rc7_pack_level_backfill".to_string(),
            "4.0.0rc7_track_charter_pack_yaml".to_string(),
        ]
    );
    assert!(report.skipped().is_empty());
    assert!(report.metadata_written);

    let exclude = std::fs::read_to_string(root.join(".git/info/exclude")).unwrap();
    assert!(exclude.starts_with("# git exclude\n"));
    assert!(exclude.contains(".spec-kitty/"));
    let pack = std::fs::read_to_string(root.join(".kittify/charter-packs/pack.yaml")).unwrap();
    assert!(pack.contains("pack_level: project"));
    let ignore = std::fs::read_to_string(root.join(".gitignore")).unwrap();
    assert!(ignore.contains("!.kittify/charter-packs/pack.yaml"));
    assert_eq!(
        ProjectMetadata::load(&root.join(".kittify"))
            .expect("metadata")
            .version,
        "4.0.0rc7"
    );

    let catalog = MigrationCatalog::embedded().unwrap();
    let ledger = MigrationLedger::new(root);
    for id in report.applied() {
        let spec = catalog.get(&id).expect("spec");
        assert!(
            ledger
                .records(spec.clone_local)
                .unwrap()
                .iter()
                .any(|record| record.id == id && record.result == SETTLED_RESULT),
            "{id} should be settled"
        );
    }
    let again = run_upgrade(&UpgradeRequest::new(root, "4.0.0rc7")).expect("second upgrade");
    assert!(again.applied().is_empty());
    assert!(again.skipped().is_empty());
}

#[test]
fn rc6_to_rc7_dry_run_reports_without_writing() {
    let directory = rc6_to_rc7_project();
    let root = directory.path();
    let records_before = MigrationLedger::new(root).records(false).unwrap().len();
    let report = run_upgrade(&UpgradeRequest::new(root, "4.0.0rc7").dry_run()).expect("dry run");
    assert!(report.success);
    assert!(
        report
            .rows
            .iter()
            .any(|row| row.id == "4.0.0rc7_pack_level_backfill" && row.result == "would_apply")
    );
    assert!(!root.join(".kittify/charter-packs/pack.yaml").exists());
    assert!(
        !std::fs::read_to_string(root.join(".gitignore"))
            .unwrap()
            .contains("!.kittify/charter-packs/pack.yaml")
    );
    assert!(
        !std::fs::read_to_string(root.join(".git/info/exclude"))
            .unwrap()
            .contains(".spec-kitty/")
    );
    assert_eq!(
        MigrationLedger::new(root).records(false).unwrap().len(),
        records_before
    );
    assert_eq!(
        ProjectMetadata::load(&root.join(".kittify"))
            .unwrap()
            .version,
        "4.0.0rc6"
    );
}

#[test]
fn a_fresh_clone_with_only_clone_local_state_missing_applies_the_whole_family() {
    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path();
    initialized(root, "4.0.0rc7");
    git_repo(root);
    seed_tracked(root, "4.0.0rc7");
    let catalog = MigrationCatalog::embedded().unwrap();
    let expected: Vec<String> = catalog
        .specs()
        .iter()
        .filter(|spec| spec.clone_local && spec.target_version.as_str() <= "4.0.0rc7")
        .map(|spec| spec.id.clone())
        .collect();
    assert_eq!(expected.len(), 10, "{expected:?}");

    let report = run_upgrade(&UpgradeRequest::new(root, "4.0.0rc7")).expect("upgrade");
    assert!(report.success);
    assert_eq!(report.applied(), expected);
    assert!(
        report
            .rows
            .iter()
            .all(|row| matches!(row.result.as_str(), "applied" | "skipped"))
    );
    let tracked = MigrationLedger::new(root).records(false).unwrap();
    assert_eq!(
        tracked
            .iter()
            .filter(|record| record.result == SETTLED_RESULT)
            .count(),
        124
    );
    let clone = MigrationLedger::new(root).records(true).unwrap();
    for id in report.applied() {
        assert!(
            clone
                .iter()
                .any(|record| record.id == id && record.result == SETTLED_RESULT),
            "{id}"
        );
    }
    let again = run_upgrade(&UpgradeRequest::new(root, "4.0.0rc7")).expect("second upgrade");
    assert!(again.applied().is_empty());
    assert!(again.skipped().is_empty());
}

#[test]
fn a_realistic_rc8_ledger_still_refuses_every_unimplemented_id_before_mutation() {
    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path();
    initialized(root, "4.0.0rc7");
    git_repo(root);
    let rc8: Vec<String> = MigrationCatalog::embedded()
        .unwrap()
        .specs()
        .iter()
        .filter(|spec| spec.target_version == "4.0.0rc8")
        .map(|spec| spec.id.clone())
        .collect();
    let unseeded: Vec<&str> = rc8.iter().map(String::as_str).collect();
    seed_all_except(root, "4.0.0rc7", &unseeded);
    let ledger_before = MigrationLedger::new(root).records(false).unwrap().len();

    let error = run_upgrade(&UpgradeRequest::new(root, "4.0.0rc8")).expect_err("unsupported");
    assert_eq!(error.code(), "migration_not_implemented");
    let message = error.to_string();
    for id in [
        "4.0.0rc8_backfill_migration_ledger",
        "4.0.0rc8_project_intent_directive_rename",
        "4.0.0rc8_activation_ledger",
        "4.0.0rc8_promote_interview_selections",
        "4.0.0rc8_command_skill_hyphen_names",
        "4.0.0rc8_retire_antigravity_workflows",
    ] {
        assert!(message.contains(id), "{message}");
    }
    assert!(!message.contains("4.0.0rc8_narrow_agent_gitignore"));
    assert!(!message.contains("4.0.0rc8_retire_stale_mission_template_forks"));
    assert_eq!(
        MigrationLedger::new(root).records(false).unwrap().len(),
        ledger_before
    );
    assert_eq!(
        ProjectMetadata::load(&root.join(".kittify"))
            .unwrap()
            .version,
        "4.0.0rc7"
    );
    assert!(!root.join(".kittify/charter-packs/pack.yaml").exists());
    assert!(
        !std::fs::read_to_string(root.join(".gitattributes"))
            .unwrap_or_default()
            .contains("merge=")
    );
}

#[test]
fn a_can_apply_refusal_is_a_skip_that_is_not_recorded_and_is_retried() {
    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path();
    initialized(root, "4.0.0rc6");
    git_repo(root);
    std::fs::create_dir_all(root.join(".kittify/charter-packs")).unwrap();
    let descriptor = root.join(".kittify/charter-packs/pack.yaml");
    std::fs::write(&descriptor, "pack_id: [unterminated\n").unwrap();
    seed_all_except(root, "4.0.0rc6", &["4.0.0rc7_pack_level_backfill"]);

    let plan = plan_upgrade(&UpgradeRequest::new(root, "4.0.0rc7")).expect("plan");
    assert_eq!(plan.apply_ids(), Vec::<String>::new());
    assert_eq!(
        plan.rows
            .iter()
            .filter(|row| row.spec.id == "4.0.0rc7_pack_level_backfill")
            .map(|row| row.outcome.as_str())
            .collect::<Vec<_>>(),
        vec!["not_applicable"]
    );
    let report = run_upgrade(&UpgradeRequest::new(root, "4.0.0rc7")).expect("upgrade");
    assert!(report.applied().is_empty());
    assert_eq!(
        report.skipped(),
        vec!["4.0.0rc7_pack_level_backfill".to_string()]
    );
    assert_eq!(
        std::fs::read_to_string(&descriptor).unwrap(),
        "pack_id: [unterminated\n"
    );
    let records = MigrationLedger::new(root).records(false).unwrap();
    assert!(
        !records
            .iter()
            .any(|record| record.id == "4.0.0rc7_pack_level_backfill")
    );

    let again = run_upgrade(&UpgradeRequest::new(root, "4.0.0rc7")).expect("retry");
    assert_eq!(
        again.skipped(),
        vec!["4.0.0rc7_pack_level_backfill".to_string()]
    );
}
