//! End-to-end administrative routes through the **actual** `spec-kitty` binary.
//!
//! These tests exercise the two surfaces this continuation completed on the CLI
//! side: the compat-planner preview contract (`upgrade --plan-json` and
//! `upgrade --json --dry-run`) and the centralised-layout `migrate` command,
//! each with an isolated `HOME`/XDG/TMP so nothing touches real user state.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_spec-kitty")
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, contents).expect("write fixture");
}

/// A project with schema 3 metadata and (optionally) a fully settled ledger.
fn project(version: &str, schema: i64) -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path();
    write(
        &root.join(".kittify/config.yaml"),
        "agents:\n  available:\n  - claude\n",
    );
    write(
        &root.join(".kittify/metadata.yaml"),
        &format!(
            "# Spec Kitty Project Metadata\nspec_kitty:\n  schema_version: {schema}\n  version: {version}\n  initialized_at: '2026-01-01T00:00:00+00:00'\n"
        ),
    );
    directory
}

fn run(root: &Path, home: &Path, args: &[&str]) -> Output {
    Command::new(binary())
        .current_dir(root)
        .env("HOME", home)
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("TMPDIR", home.join("tmp"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("SPECIFY_REPO_ROOT")
        .env_remove("CI")
        .args(args)
        .output()
        .expect("run spec-kitty")
}

/// Mark every declared migration settled, the state of an already-upgraded project.
fn settle_ledger(root: &Path) {
    let catalog = sk_install::MigrationCatalog::embedded().expect("catalog");
    let ledger = sk_install::MigrationLedger::new(root);
    let version = env!("CARGO_PKG_VERSION");
    let records: Vec<sk_install::LedgerRecord> = catalog
        .specs()
        .iter()
        .map(|spec| sk_install::LedgerRecord::for_spec(spec, sk_install::SETTLED_RESULT, version))
        .collect();
    ledger.append_many(&records).expect("seed ledger");
}

fn home(directory: &tempfile::TempDir) -> PathBuf {
    let home = directory.path().join("home");
    fs::create_dir_all(home.join("tmp")).expect("home");
    home
}

fn stdout_json(output: &Output) -> Value {
    let text = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&text).unwrap_or_else(|error| {
        panic!(
            "stdout is not one JSON document ({error}):\nstdout:\n{text}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

#[test]
fn upgrade_plan_json_emits_the_full_preview_envelope() {
    let directory = project("4.0.0rc8", 3);
    let root = directory.path();
    let home = home(&directory);
    let output = run(root, &home, &["upgrade", "--plan-json"]);
    let payload = stdout_json(&output);
    assert_eq!(payload["schema_version"], 1, "{payload}");
    assert_eq!(payload["mode"], "preview");
    assert_eq!(payload["intent"]["include_worktrees"], true);
    assert_eq!(payload["target"]["valid"], true);
    assert_eq!(payload["roots"][0]["id"], "project");
    assert_eq!(
        payload["roots"][0]["path"],
        root.display().to_string(),
        "the envelope names the real project root"
    );
    // The compat-planner compatibility block is embedded, with its own state.
    assert_eq!(payload["compatibility"]["schema_version"], 1);
    assert_eq!(payload["compatibility"]["project"]["state"], "compatible");
    assert!(payload["migrations"].is_array());
    assert!(payload["effects"].is_array());
    assert!(payload["dispositions"].is_array());
    // The fixture config declares no `auto_commit`, so the canonical default
    // (`true`) is the policy the run applies — no longer a disabled placeholder.
    assert_eq!(payload["commit_policy"]["project_enabled"], true);
    // An unseeded project is schema-compatible, so the layout assessment and the
    // migration list are both real; whether the plan is *ready* depends only on
    // whether every selected migration has a native implementation.
    assert!(
        matches!(payload["decision"].as_str(), Some("ready" | "incomplete")),
        "{payload}"
    );
    // Nothing was written.
    assert!(!root.join(".kittify/migration-ledger.jsonl").exists());
}

#[test]
fn upgrade_plan_json_reports_the_layout_effects_it_can_observe() {
    let directory = project("4.0.0rc8", 3);
    let root = directory.path();
    let home = home(&directory);
    write(&root.join(".kittify/templates/mine.md"), "# mine\n");
    let output = run(root, &home, &["upgrade", "--plan-json"]);
    let payload = stdout_json(&output);
    let effects = payload["effects"].as_array().expect("effects");
    let moved = effects
        .iter()
        .find(|effect| effect["action"] == "move")
        .unwrap_or_else(|| panic!("a user-created template is a move effect: {payload}"));
    assert!(
        moved["path"]
            .as_str()
            .expect("path")
            .ends_with(".kittify/overrides/templates/mine.md"),
        "{moved}"
    );
    assert_eq!(moved["before"]["kind"], "file");
    assert_eq!(moved["after"]["kind"], "absent");
    let dispositions = payload["dispositions"].as_array().expect("dispositions");
    assert!(
        dispositions
            .iter()
            .any(|disposition| disposition["state"] == "customized"),
        "{payload}"
    );
    // The preview wrote nothing.
    assert!(root.join(".kittify/templates/mine.md").is_file());
    assert!(!root.join(".kittify/overrides").exists());
}

#[test]
fn upgrade_plan_json_refuses_a_downgrade_target() {
    let directory = project("4.0.0rc8", 3);
    let home = home(&directory);
    let output = run(
        directory.path(),
        &home,
        &["upgrade", "--plan-json", "--target", "4.0.0rc7"],
    );
    let payload = stdout_json(&output);
    assert_eq!(payload["target"]["valid"], false);
    assert_eq!(payload["decision"], "blocked");
    assert_eq!(output.status.code(), Some(2));
    let diagnostics = payload["diagnostics"].as_array().expect("diagnostics");
    assert!(
        diagnostics
            .iter()
            .any(|item| item["code"] == "invalid_target"),
        "{payload}"
    );
}

#[test]
fn upgrade_json_dry_run_emits_the_compat_planner_contract() {
    let directory = project("4.0.0rc8", 3);
    let root = directory.path();
    let home = home(&directory);
    let output = run(root, &home, &["upgrade", "--json", "--dry-run"]);
    let payload = stdout_json(&output);
    assert_eq!(payload["schema_version"], 1);
    assert_eq!(payload["case"], "none");
    assert_eq!(payload["decision"], "ALLOW");
    assert_eq!(
        payload["cli"]["installed_version"],
        sk_kernel::COMPAT_CORE_VERSION
    );
    assert_eq!(payload["cli"]["latest_version"], Value::Null);
    assert_eq!(payload["cli"]["latest_source"], "none");
    assert_eq!(payload["project"]["state"], "compatible");
    assert_eq!(payload["project"]["schema_version"], 3);
    assert_eq!(payload["project"]["min_supported"], 3);
    assert_eq!(payload["project"]["max_supported"], 3);
    assert_eq!(payload["safety"], "unsafe");
    let pending = payload["pending_migrations"].as_array().expect("pending");
    assert!(
        !pending.is_empty(),
        "an unseeded project selects migrations"
    );
    for row in pending {
        assert!(row["migration_id"].is_string(), "{row}");
        assert!(row["target_schema_version"].is_number(), "{row}");
        assert!(row["description"].is_string(), "{row}");
    }
    assert!(payload["rendered_human"].is_string());
    // Adoption has virtually settled older history. All actually pending ids
    // now have real callbacks, including the formerly withheld charter rename.
    for row in pending {
        let id = row["migration_id"].as_str().expect("migration id");
        assert!(
            sk_install::native_migrations().any(|migration| migration.id == id),
            "pending callback is genuinely registered: {id}"
        );
    }
    assert!(
        pending
            .iter()
            .any(|row| row["migration_id"] == "4.0.0rc8_project_intent_directive_rename")
    );
    assert_eq!(payload["exit_code"], 0, "{payload}");
    assert_eq!(output.status.code(), Some(0));
    // A dry run never writes.
    assert!(!root.join(".kittify/migration-ledger.jsonl").exists());
}

#[test]
fn a_settled_project_previews_a_clean_zero() {
    let directory = project("4.0.0rc8", 3);
    let root = directory.path();
    settle_ledger(root);
    let home = home(&directory);
    let output = run(root, &home, &["upgrade", "--json", "--dry-run"]);
    let payload = stdout_json(&output);
    assert_eq!(payload["decision"], "ALLOW");
    assert_eq!(payload["exit_code"], 0, "{payload}");
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        payload["pending_migrations"].as_array().map(Vec::len),
        Some(0)
    );
}

#[test]
fn a_legacy_project_is_a_blocked_migration_in_the_contract() {
    let directory = project("4.0.0rc8", 3);
    let root = directory.path();
    let home = home(&directory);
    write(
        &root.join(".kittify/metadata.yaml"),
        "spec_kitty:\n  version: 4.0.0rc8\n",
    );
    let output = run(root, &home, &["upgrade", "--json", "--project"]);
    let payload = stdout_json(&output);
    assert_eq!(payload["project"]["state"], "legacy");
    assert_eq!(payload["decision"], "BLOCK_PROJECT_MIGRATION");
    assert_eq!(payload["case"], "project_migration_needed");
    assert_eq!(output.status.code(), Some(4));
}

#[test]
fn a_project_too_new_for_this_cli_blocks_the_cli_upgrade() {
    let directory = project("4.0.0rc8", 99);
    let home = home(&directory);
    let output = run(directory.path(), &home, &["upgrade", "--json", "--project"]);
    let payload = stdout_json(&output);
    assert_eq!(payload["project"]["state"], "too_new");
    assert_eq!(payload["decision"], "BLOCK_CLI_UPGRADE");
    assert_eq!(output.status.code(), Some(5));
}

#[test]
fn migrate_moves_a_user_created_asset_into_overrides_and_keeps_the_config() {
    let directory = project("4.0.0rc8", 3);
    let root = directory.path();
    let home = home(&directory);
    write(&root.join(".kittify/templates/mine.md"), "# mine\n");
    let output = run(root, &home, &["migrate"]);
    assert!(
        output.status.success(),
        "migrate failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(root.join(".kittify/overrides/templates/mine.md").is_file());
    assert!(!root.join(".kittify/templates/mine.md").exists());
    assert!(root.join(".kittify/config.yaml").is_file());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Step 2: Per-project .kittify/ cleanup"),
        "{stdout}"
    );
    assert!(stdout.contains("1 files customized"), "{stdout}");
}

#[test]
fn migrate_dry_run_writes_nothing() {
    let directory = project("4.0.0rc8", 3);
    let root = directory.path();
    let home = home(&directory);
    write(&root.join(".kittify/templates/mine.md"), "# mine\n");
    let output = run(root, &home, &["migrate", "--dry-run"]);
    assert!(output.status.success());
    assert!(root.join(".kittify/templates/mine.md").is_file());
    assert!(!root.join(".kittify/overrides").exists());
    assert!(String::from_utf8_lossy(&output.stdout).contains("Dry run -- no changes made"));
}

#[test]
fn migrate_refuses_a_group_flag_the_subcommand_does_not_declare() {
    let directory = project("4.0.0rc8", 3);
    let home = home(&directory);
    let output = run(directory.path(), &home, &["migrate", "--dry-run", "ledger"]);
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--dry-run"), "{stderr}");
    assert!(stderr.contains("migrate ledger"), "{stderr}");
}

#[test]
fn migrate_ledger_still_reports_the_two_partitions() {
    let directory = project("4.0.0rc8", 3);
    let home = home(&directory);
    let output = run(directory.path(), &home, &["migrate", "ledger", "--json"]);
    let payload = stdout_json(&output);
    assert!(
        payload["summary"]["total"].as_u64().unwrap_or(0) >= 100,
        "{payload}"
    );
    assert!(payload["steps"].is_array());
}
