//! Acceptance tests for the retirement migrations and the full default upgrade.
//!
//! Every case runs the real native effect against a temporary project, then
//! asserts the observable filesystem/ledger outcome. The fixtures are authored
//! from the pinned source's own preconditions (a marked command file, a managed
//! hook signature, a manifest-owned skill tree, a machine `config.toml`), never
//! from the native code's output.

use std::path::{Path, PathBuf};

use sk_install::{
    LedgerRecord, MigrationCatalog, MigrationLedger, ProjectMetadata, SETTLED_RESULT,
    UpgradeRequest, plan_upgrade, run_upgrade,
};

fn write(path: &Path, content: impl AsRef<[u8]>) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

fn initialized(root: &Path, version: &str) {
    write(
        &root.join(".kittify/config.yaml"),
        "agents:\n  available:\n  - claude\n",
    );
    write(
        &root.join(".kittify/metadata.yaml"),
        format!(
            "# Spec Kitty Project Metadata\nspec_kitty:\n  schema_version: 3\n  version: {version}\n  initialized_at: '2026-01-01T00:00:00+00:00'\n  last_upgraded_at:\n"
        ),
    );
}

/// Seed the ledger `success` for every declared migration except `open`.
fn seed_except(root: &Path, open: &[&str]) {
    std::fs::create_dir_all(root.join(".kittify")).unwrap();
    let catalog = MigrationCatalog::embedded().expect("catalog");
    let ledger = MigrationLedger::new(root);
    let mut tracked = Vec::new();
    let mut clone_local = Vec::new();
    for spec in catalog.specs() {
        if open.contains(&spec.id.as_str()) {
            continue;
        }
        let record = LedgerRecord::for_spec(spec, SETTLED_RESULT, "4.0.0rc8");
        if spec.clone_local {
            clone_local.push(record);
        } else {
            tracked.push(record);
        }
    }
    for (path, records) in [
        (ledger.tracked_path(), tracked),
        (ledger.clone_local_path(), clone_local),
    ] {
        if records.is_empty() {
            continue;
        }
        let text: Vec<String> = records.iter().map(LedgerRecord::to_line).collect();
        write(&path, format!("{}\n", text.join("\n")));
    }
}

fn outcomes(root: &Path) -> Vec<(String, String, bool)> {
    let ledger = MigrationLedger::new(root);
    let catalog = MigrationCatalog::embedded().expect("catalog");
    let mut rows = Vec::new();
    for spec in catalog.specs() {
        let records = ledger.records_for(spec).expect("records");
        if let Some(record) = records.iter().rev().find(|record| record.id == spec.id) {
            rows.push((spec.id.clone(), record.result.clone(), record.clone_local));
        }
    }
    rows
}

/// A 4.0.0rc5-shaped project: everything applied except the four retirement
/// migrations and the two runtime-only ones this fixture does not exercise.
fn project(root: &Path) {
    initialized(root, "4.0.0rc5");
    write(
        &root.join(".git/hooks/pre-commit"),
        b"#!/bin/sh\nSPEC_KITTY_MANAGED_HOOK_SHIM=1\nexec spec-kitty pre-commit\n",
    );
    write(
        &root.join(".claude/commands/spec-kitty.clarify.md"),
        b"<!-- spec-kitty-command-version: 2.0.11 -->\n# clarify\n",
    );
    write(
        &root.join(".claude/commands/spec-kitty.profile-context.md"),
        b"<!-- spec-kitty-command-version: 3.2.0 -->\n# profile context\n",
    );
    write(
        &root.join(".claude/commands/spec-kitty.dashboard.md"),
        b"<!-- spec-kitty-command-version: 4.0.0rc5 -->\n# dashboard\n",
    );
    write(
        &root.join(".kittify/preflight-warning.json"),
        b"{\"warning\": true}\n",
    );
    write(
        &root.join(".claude/skills/release/SKILL.md"),
        b"# release (shipped by mistake)\n",
    );
}

fn run(root: &Path) -> sk_install::UpgradeReport {
    let request = UpgradeRequest::new(root, "4.0.0rc8");
    run_upgrade(&request).expect("upgrade")
}

#[test]
fn a_default_upgrade_removes_the_retired_surfaces_and_records_each_step() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    project(root);
    seed_except(
        root,
        &[
            "2.0.0_retire_git_hooks",
            "2.0.11_remove_clarify_command",
            "2.1.2_remove_release_skill",
            "3.2.0rc43_retire_profile_context_command",
            "4.0.0rc5_retire_bundled_dashboard",
        ],
    );

    let plan = plan_upgrade(&UpgradeRequest::new(root, "4.0.0rc8")).expect("plan");
    let mut apply = plan.apply_ids();
    apply.sort();
    assert_eq!(
        apply,
        vec![
            "2.0.0_retire_git_hooks",
            "2.0.11_remove_clarify_command",
            "2.1.2_remove_release_skill",
            "3.2.0rc43_retire_profile_context_command",
            "4.0.0rc5_retire_bundled_dashboard",
        ],
        "the plan selects exactly the five retirement steps; nothing else is pending"
    );
    assert!(!plan.is_blocked(), "no unimplemented migration is selected");

    let report = run(root);
    assert!(report.success);
    assert!(report.metadata_written);
    let applied = report.applied();
    for expected in [
        "2.0.0_retire_git_hooks",
        "2.0.11_remove_clarify_command",
        "2.1.2_remove_release_skill",
        "3.2.0rc43_retire_profile_context_command",
        "4.0.0rc5_retire_bundled_dashboard",
    ] {
        assert!(
            applied.iter().any(|id| id == expected),
            "{expected} applied"
        );
    }

    // Effects, not just records.
    assert!(!root.join(".git/hooks/pre-commit").exists());
    assert!(!root.join(".claude/commands/spec-kitty.clarify.md").exists());
    assert!(
        !root
            .join(".claude/commands/spec-kitty.profile-context.md")
            .exists()
    );
    assert!(
        !root
            .join(".claude/commands/spec-kitty.dashboard.md")
            .exists()
    );
    assert!(!root.join(".kittify/preflight-warning.json").exists());
    assert!(!root.join(".claude/skills/release").exists());
    // The unproven release skill was archived, not destroyed.
    let archives: Vec<PathBuf> = std::fs::read_dir(root.join(".kittify"))
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(".backup-"))
        })
        .collect();
    assert_eq!(archives.len(), 1, "one preservation archive");
    assert!(
        archives[0]
            .join(".claude/skills/release/SKILL.md")
            .is_file()
    );

    // The project version advanced and the records are `success`.
    let metadata = ProjectMetadata::load(&root.join(".kittify")).expect("metadata");
    assert_eq!(metadata.version, "4.0.0rc8");
    let rows = outcomes(root);
    for id in [
        "2.0.0_retire_git_hooks",
        "2.0.11_remove_clarify_command",
        "2.1.2_remove_release_skill",
        "3.2.0rc43_retire_profile_context_command",
        "4.0.0rc5_retire_bundled_dashboard",
    ] {
        let row = rows
            .iter()
            .find(|(row_id, _, _)| row_id == id)
            .unwrap_or_else(|| panic!("{id} recorded"));
        assert_eq!(row.1, SETTLED_RESULT);
        assert!(!row.2, "{id} is a tracked step");
    }
}

#[test]
fn a_second_run_is_an_idempotent_no_op() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    project(root);
    seed_except(
        root,
        &[
            "2.0.0_retire_git_hooks",
            "2.0.11_remove_clarify_command",
            "2.1.2_remove_release_skill",
            "3.2.0rc43_retire_profile_context_command",
            "4.0.0rc5_retire_bundled_dashboard",
        ],
    );
    run(root);
    let before = std::fs::read_to_string(MigrationLedger::new(root).tracked_path()).unwrap();
    let second = run(root);
    assert!(second.success);
    assert!(
        second.applied().is_empty(),
        "nothing is applied twice: {:?}",
        second.applied()
    );
    let after = std::fs::read_to_string(MigrationLedger::new(root).tracked_path()).unwrap();
    assert_eq!(before, after, "a settled migration appends no record");
}

#[test]
fn a_user_file_reusing_a_retired_name_survives_the_upgrade() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    initialized(root, "4.0.0rc5");
    write(
        &root.join(".claude/commands/spec-kitty.clarify.md"),
        b"# our own clarify command, hand written\n",
    );
    seed_except(root, &["2.0.11_remove_clarify_command"]);

    let report = run(root);
    assert!(report.success);
    assert!(
        root.join(".claude/commands/spec-kitty.clarify.md").exists(),
        "an unmarked user file is never deleted by name"
    );
    let warning = report
        .rows
        .iter()
        .find(|row| row.id == "2.0.11_remove_clarify_command")
        .and_then(|row| row.warnings.first())
        .expect("preservation warning");
    assert!(warning.contains("Preserved"), "{warning}");
}

#[test]
fn a_custom_git_hook_is_kept_and_reported() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    initialized(root, "4.0.0rc5");
    // One managed hook (so `detect` is true) and one custom hook that shares the
    // managed *name* but not its signature.
    write(
        &root.join(".git/hooks/pre-commit"),
        b"#!/bin/sh\nSPEC_KITTY_MANAGED_HOOK_SHIM=1\nexec spec-kitty pre-commit\n",
    );
    write(
        &root.join(".git/hooks/commit-msg"),
        b"#!/bin/sh\necho my own hook\n",
    );
    seed_except(root, &["2.0.0_retire_git_hooks"]);
    let report = run(root);
    assert!(report.success);
    assert!(!root.join(".git/hooks/pre-commit").exists());
    assert!(root.join(".git/hooks/commit-msg").exists());
    assert!(
        report
            .rows
            .iter()
            .any(|row| row.id == "2.0.0_retire_git_hooks"
                && row
                    .warnings
                    .iter()
                    .any(|w| w.contains("Skipped custom hook"))),
        "{:?}",
        report.rows
    );
}

#[test]
fn a_project_with_only_custom_hooks_is_not_touched() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    initialized(root, "4.0.0rc5");
    write(
        &root.join(".git/hooks/pre-commit"),
        b"#!/bin/sh\necho my own hook\n",
    );
    seed_except(root, &["2.0.0_retire_git_hooks"]);
    let plan = plan_upgrade(&UpgradeRequest::new(root, "4.0.0rc8")).expect("plan");
    let row = plan
        .rows
        .iter()
        .find(|row| row.spec.id == "2.0.0_retire_git_hooks")
        .expect("row");
    assert_eq!(
        row.outcome.as_str(),
        "not_needed",
        "detect() finds no managed hook, so nothing is applied"
    );
    let report = run(root);
    assert!(report.success);
    assert!(root.join(".git/hooks/pre-commit").exists());
}

#[test]
fn a_clone_local_retirement_routes_to_the_clone_local_partition() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    initialized(root, "4.0.0rc8");
    write(&root.join(".git/info/exclude"), "# git exclude\n");
    write(
        &root.join(".kittify/config.yaml"),
        "agents:\n  available:\n  - antigravity\n",
    );
    write(
        &root.join(".agent/workflows/spec-kitty.plan.md"),
        b"# legacy antigravity workflow\n",
    );
    // Derive the still-unimplemented id from the canonical iterator instead of
    // naming an effect that later landed: `4.0.0rc8_retire_antigravity_workflows`
    // is implemented now, so the clone-local retirement under test is applied
    // and recorded rather than refused.
    let unimplemented = unimplemented_id(root);
    seed_except(root, &[unimplemented.as_str()]);

    let request = UpgradeRequest::new(root, "4.0.0rc8");
    let error = run_upgrade(&request).expect_err("unimplemented selection refuses");
    let text = error.to_string();
    assert!(text.contains(&unimplemented), "{text}");
    // Nothing was written by the refused run: the retired workflow is intact
    // and no partition holds a record for any selected id.
    assert!(root.join(".agent/workflows/spec-kitty.plan.md").exists());
    let ledger = MigrationLedger::new(root);
    assert!(
        !ledger
            .records(true)
            .unwrap()
            .iter()
            .any(|record| record.id == unimplemented),
        "a refusal records nothing"
    );
}

/// The first catalog id with no native implementation, from the one canonical
/// iterator (never a hand-written list).
fn unimplemented_id(project_root: &std::path::Path) -> String {
    let request = UpgradeRequest::new(project_root, "4.0.0rc8");
    let plan = plan_upgrade(&request).expect("plan");
    plan.unimplemented_ids()
        .into_iter()
        .next()
        .expect("the catalog still declares at least one unimplemented migration")
}

#[test]
fn the_roo_agent_is_dropped_from_config_without_touching_the_directory() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    initialized(root, "4.0.0rc5");
    write(
        &root.join(".kittify/config.yaml"),
        "# project config\nagents:\n  available:\n  - claude\n  - roo\n  - codex\n  other: 1\n",
    );
    write(&root.join(".roo/rules.md"), b"# roo rules\n");
    seed_except(root, &["0_9_4_roo_deprecation"]);

    let report = run(root);
    assert!(report.success);
    let config = std::fs::read_to_string(root.join(".kittify/config.yaml")).unwrap();
    assert_eq!(
        config, "# project config\nagents:\n  available:\n  - claude\n  - codex\n  other: 1\n",
        "only the roo member is dropped; comments and siblings survive"
    );
    assert!(root.join(".roo/rules.md").exists(), ".roo/ is preserved");
}

#[test]
fn the_retired_hosted_target_is_deleted_only_for_the_exact_host() {
    // Machine-scoped effect exercised through its path-parameterised seam, so no
    // process environment is mutated.
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.toml");
    write(
        &config,
        "[sync]\nserver_url = \"https://app.spec-kitty.ai\"\ntoken = \"keep-me\"\n[other]\nkey = 1\n",
    );
    let outcome = sk_install::migrations::retirements::apply_retired_hosted_target_at(&config)
        .expect("apply");
    assert!(outcome.changed);
    assert!(outcome.changes[0].contains("app.spec-kitty.ai"));
    let text = std::fs::read_to_string(&config).unwrap();
    assert!(!text.contains("app.spec-kitty.ai"), "{text}");
    assert!(text.contains("token = \"keep-me\""), "{text}");
    assert!(text.contains("[other]"), "{text}");
}

#[test]
fn a_custom_hosted_endpoint_is_never_touched() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.toml");
    write(
        &config,
        "[sync]\nserver_url = \"https://app.spec-kitty.ai.example.org\"\n",
    );
    let outcome = sk_install::migrations::retirements::apply_retired_hosted_target_at(&config)
        .expect("apply");
    assert!(
        !outcome.changed,
        "a look-alike host is somebody else's config"
    );
    let text = std::fs::read_to_string(&config).unwrap();
    assert!(text.contains("app.spec-kitty.ai.example.org"));
}

#[test]
fn an_unimplemented_selection_refuses_before_the_first_write() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    initialized(root, "3.2.0");
    // No ledger: every declared migration is selected, so the run must refuse
    // and leave the project untouched.
    let before = std::fs::read_to_string(root.join(".kittify/metadata.yaml")).unwrap();
    let request = UpgradeRequest::new(root, "4.0.0rc8");
    let error = run_upgrade(&request).expect_err("refused");
    assert!(matches!(
        error,
        sk_install::InstallError::MigrationNotImplemented { .. }
    ));
    let after = std::fs::read_to_string(root.join(".kittify/metadata.yaml")).unwrap();
    assert_eq!(before, after, "a refusal writes no version stamp");
    assert!(
        !MigrationLedger::new(root).tracked_path().exists(),
        "a refusal appends no ledger record"
    );
}
