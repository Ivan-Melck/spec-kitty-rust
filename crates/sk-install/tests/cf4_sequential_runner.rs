//! CF4: the canonical *sequential* upgrade runner.
//!
//! Selection is the ledger diff; the guards are evaluated per migration, in catalogue
//! order, **after** the previous migration's effects. These tests use real file and git
//! effects in a temporary project — no callback injection — so a guard that reads another
//! migration's output is judged against the tree the run actually produced.
//!
//! Scope: sequential guards, the dry-run sequenced prediction (`sequenced_after`), main
//! stop on a failed step, detect failure vs apply failure ledger/metadata timing, version
//! and schema-stamp preservation, and rerun idempotency. It is not a failure matrix, not a
//! worktree fan-out proof (that seam is `sk-cli`'s `upgrade_worktrees`) and not an
//! exhaustive differential campaign.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use sk_install::{
    CURRENT_SCHEMA_VERSION, LedgerRecord, MigrationCatalog, MigrationLedger, ProjectMetadata,
    SETTLED_RESULT, UpgradeRequest, plan_upgrade, run_upgrade,
};

const GATE_DRIVERS: &str = "3.2.6_gate_artifact_merge_drivers";
const ISSUE_MATRIX_REPOINT: &str = "3.2.6_issue_matrix_driver_repoint";
const GATE_DRIVER_NAME: &str = "Spec Kitty issue matrix filled-side merge";
const REPOINT_DRIVER_NAME: &str = "Spec Kitty issue matrix row-aware merge";
const ISSUE_MATRIX_MD_ATTRIBUTE: &str =
    "kitty-specs/**/issue-matrix.md merge=spec-kitty-issue-matrix";
const ISSUE_MATRIX_JSON_ATTRIBUTE: &str =
    "kitty-specs/**/issue-matrix.json merge=spec-kitty-issue-matrix";

fn git(cwd: &Path, args: &[&str]) -> String {
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
    String::from_utf8(output.stdout).expect("utf8")
}

fn initialized(root: &Path, version: &str, schema_version: i64) {
    std::fs::create_dir_all(root.join(".kittify")).unwrap();
    std::fs::write(
        root.join(".kittify/config.yaml"),
        "agents:\n  available:\n  - claude\n",
    )
    .unwrap();
    std::fs::write(
        root.join(".kittify/metadata.yaml"),
        format!(
            "# Spec Kitty Project Metadata\nspec_kitty:\n  schema_version: {schema_version}\n  version: {version}\n  initialized_at: '2026-01-01T00:00:00+00:00'\n  last_upgraded_at:\n"
        ),
    )
    .unwrap();
}

fn git_repo(root: &Path) {
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["config", "user.email", "cf4@example.com"]);
    git(root, &["config", "user.name", "CF4"]);
}

/// Settle every declared migration except `unseeded`, in the authoritative partition.
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

fn result_for(root: &Path, id: &str) -> Option<String> {
    for clone_local in [false, true] {
        if let Some(record) = records(root, clone_local)
            .into_iter()
            .find(|record| record.id == id)
        {
            return Some(record.result);
        }
    }
    None
}

/// A tree that is *fully repointed* but lost the gate-driver record.
///
/// The issue-matrix driver already carries the row-aware name and the `.json` attribute,
/// so `3.2.6_issue_matrix_driver_repoint`'s `detect()` is **false** on the pre-run tree.
/// The earlier `3.2.6_gate_artifact_merge_drivers` migration rewrites that config value to
/// the filled-side name, so after it applies the repoint's `detect()` is **true** again and
/// the run must apply it in the same pass. A runner that executes a precomputed detect plan
/// leaves the shipped filled-side name and never repoints.
fn repointed_but_unrecorded(root: &Path) {
    initialized(root, "4.0.0rc7", 3);
    git_repo(root);
    std::fs::write(
        root.join(".gitattributes"),
        format!("{ISSUE_MATRIX_JSON_ATTRIBUTE}\n"),
    )
    .unwrap();
    git(
        root,
        &[
            "config",
            "merge.spec-kitty-issue-matrix.name",
            REPOINT_DRIVER_NAME,
        ],
    );
    git(
        root,
        &[
            "config",
            "merge.spec-kitty-issue-matrix.driver",
            "spec-kitty merge-driver-issue-matrix %O %A %B",
        ],
    );
    seed_all_except(root, "4.0.0rc7", &[GATE_DRIVERS, ISSUE_MATRIX_REPOINT]);
}

#[test]
fn a_guard_is_evaluated_after_the_previous_migrations_effects() {
    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path();
    repointed_but_unrecorded(root);

    // Pre-run: the later migration's own guard says "nothing to do". That is the answer a
    // precomputed detect plan would reuse for execution.
    let catalog = MigrationCatalog::embedded().expect("catalog");
    let repoint = catalog.get(ISSUE_MATRIX_REPOINT).expect("declared");
    assert!(
        !sk_install::native_migration_detect(ISSUE_MATRIX_REPOINT, root)
            .expect("implemented")
            .expect("detect"),
        "fixture must start with the repoint already in place"
    );
    assert!(
        !sk_install::MigrationLedger::new(root)
            .is_settled(repoint)
            .unwrap()
    );

    let report = run_upgrade(&UpgradeRequest::new(root, "4.0.0rc7")).expect("upgrade");
    assert!(report.success);
    assert_eq!(
        report.applied(),
        vec![GATE_DRIVERS.to_string(), ISSUE_MATRIX_REPOINT.to_string()],
        "the repoint must be re-evaluated after the gate drivers rewrote its input"
    );
    assert_eq!(
        result_for(root, GATE_DRIVERS).as_deref(),
        Some(SETTLED_RESULT)
    );
    assert_eq!(
        result_for(root, ISSUE_MATRIX_REPOINT).as_deref(),
        Some(SETTLED_RESULT)
    );

    let name = git(root, &["config", "merge.spec-kitty-issue-matrix.name"]);
    assert_eq!(name.trim(), REPOINT_DRIVER_NAME);
    let attributes = std::fs::read_to_string(root.join(".gitattributes")).unwrap();
    assert!(
        attributes.contains(ISSUE_MATRIX_MD_ATTRIBUTE),
        "{attributes}"
    );
    assert!(
        attributes.contains(ISSUE_MATRIX_JSON_ATTRIBUTE),
        "{attributes}"
    );
    // The gate driver's own name is not the shipped one any more.
    assert!(!name.contains(GATE_DRIVER_NAME));
}

#[test]
fn a_dry_run_predicts_the_sequenced_verdict_and_writes_nothing() {
    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path();
    repointed_but_unrecorded(root);
    let before_attributes = std::fs::read_to_string(root.join(".gitattributes")).unwrap();
    let before_metadata = std::fs::read_to_string(root.join(".kittify/metadata.yaml")).unwrap();
    let before_records = records(root, true).len();

    let plan = plan_upgrade(&UpgradeRequest::new(root, "4.0.0rc7")).expect("plan");
    let row = plan
        .rows
        .iter()
        .find(|row| row.spec.id == ISSUE_MATRIX_REPOINT)
        .expect("selected");
    assert_eq!(row.outcome.as_str(), "apply");
    let detail = row.detail.clone().unwrap_or_default();
    assert!(
        detail.contains("would apply after") && detail.contains(GATE_DRIVERS),
        "prediction must name its declared predecessor: {detail}"
    );

    let report = run_upgrade(&UpgradeRequest::new(root, "4.0.0rc7").dry_run()).expect("dry run");
    assert!(report.success && report.dry_run);
    let rows: Vec<(String, String)> = report
        .rows
        .iter()
        .map(|row| (row.id.clone(), row.result.clone()))
        .collect();
    assert_eq!(
        rows,
        vec![
            (GATE_DRIVERS.to_string(), "would_apply".to_string()),
            (ISSUE_MATRIX_REPOINT.to_string(), "would_apply".to_string()),
        ]
    );
    // Nothing was written: no attribute, no config, no ledger record, no version bump.
    assert_eq!(
        std::fs::read_to_string(root.join(".gitattributes")).unwrap(),
        before_attributes
    );
    assert_eq!(
        std::fs::read_to_string(root.join(".kittify/metadata.yaml")).unwrap(),
        before_metadata
    );
    assert_eq!(records(root, true).len(), before_records);
    let name = git(root, &["config", "merge.spec-kitty-issue-matrix.name"]);
    assert_eq!(name.trim(), REPOINT_DRIVER_NAME);
}

#[test]
fn a_dry_run_does_not_claim_a_target_state_it_did_not_simulate() {
    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path();
    initialized(root, "4.0.0rc7", 3);
    git_repo(root);
    seed_all_except(root, "4.0.0rc7", &["3.2.0rc28_github_diff_attributes"]);

    let report = run_upgrade(&UpgradeRequest::new(root, "4.0.0rc7").dry_run()).expect("dry run");
    let row = report
        .rows
        .iter()
        .find(|row| row.id == "3.2.0rc28_github_diff_attributes")
        .expect("row");
    assert_eq!(row.result, "would_apply");
    assert!(!root.join(".gitattributes").exists());
}

#[cfg(unix)]
#[test]
fn a_detect_failure_stops_the_loop_and_preserves_the_version_stamp() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path();
    initialized(root, "4.0.0rc7", 2);
    let failing = "2.0.9_state_gitignore";
    let later = "4.0.0rc7_exclude_spec_kitty";
    seed_all_except(root, "4.0.0rc7", &[failing, later]);
    std::fs::create_dir_all(root.join(".git/info")).unwrap();
    std::fs::write(root.join(".git/info/exclude"), "# git exclude\n").unwrap();
    std::fs::write(root.join("elsewhere"), "keep me\n").unwrap();
    // The strict ignore reader refuses a symlinked ignore file: `detect()` cannot evaluate
    // the project, which is a recorded failure, never a skip.
    symlink("elsewhere", root.join(".gitignore")).unwrap();
    let metadata_before = std::fs::read_to_string(root.join(".kittify/metadata.yaml")).unwrap();

    // CF16: Source `_detect_failure` RETURNS a failed result for this guard class, so
    // the run ends with a report (stamp restored) rather than a raised error.
    let report = run_upgrade(&UpgradeRequest::new(root, "4.0.0rc7")).expect("report");
    assert!(!report.success && report.failed);
    assert!(
        report
            .errors
            .iter()
            .any(|error| error.starts_with("Cannot safely detect 2.0.9_state_gitignore: ")),
        "{:?}",
        report.errors
    );

    assert_eq!(result_for(root, failing).as_deref(), Some("failed"));
    assert_eq!(
        result_for(root, later),
        None,
        "the run stopped at the failure; no later migration ran or was recorded"
    );
    let exclude = std::fs::read_to_string(root.join(".git/info/exclude")).unwrap();
    assert!(!exclude.contains(".spec-kitty/"));
    assert_eq!(
        std::fs::read_to_string(root.join("elsewhere")).unwrap(),
        "keep me\n"
    );

    let metadata = ProjectMetadata::load(&root.join(".kittify")).expect("metadata");
    assert_eq!(
        metadata.version, "4.0.0rc7",
        "a failed run must not advance"
    );
    assert_eq!(
        metadata.schema_version,
        Some(2),
        "a failed run must not advance the schema stamp"
    );
    assert_eq!(
        std::fs::read_to_string(root.join(".kittify/metadata.yaml")).unwrap(),
        metadata_before,
        "the captured pre-run stamp is left exactly as it was found"
    );
}

#[cfg(unix)]
#[test]
fn an_apply_failure_is_recorded_at_apply_time_and_stops_the_loop() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path();
    initialized(root, "4.0.0rc7", 3);
    git_repo(root);
    let failing = "2.0.9_state_gitignore";
    seed_all_except(root, "4.0.0rc7", &[failing]);
    // Isolate the one migration so the failure is the apply, not an earlier write.
    let scope: BTreeSet<String> = BTreeSet::from([failing.to_string()]);
    let request = UpgradeRequest::new(root, "4.0.0rc7").with_scope(scope);
    // `detect()` reads `.gitignore` (absent -> pending); `apply()` cannot write it because
    // the project directory is read-only.
    let original = std::fs::metadata(root).unwrap().permissions();
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o555)).unwrap();
    let outcome = run_upgrade(&request);
    std::fs::set_permissions(root, original).unwrap();
    let error = outcome.expect_err("write refused");
    assert_eq!(error.code(), "io_failed");
    assert_eq!(result_for(root, failing).as_deref(), Some("failed"));
    assert!(!root.join(".gitignore").exists());
    let metadata = ProjectMetadata::load(&root.join(".kittify")).expect("metadata");
    assert_eq!(metadata.version, "4.0.0rc7");
}

#[test]
fn a_successful_run_advances_the_schema_stamp_and_a_rerun_changes_nothing() {
    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path();
    initialized(root, "4.0.0rc7", 2);
    git_repo(root);
    seed_all_except(root, "4.0.0rc7", &["3.2.0rc28_github_diff_attributes"]);

    let report = run_upgrade(&UpgradeRequest::new(root, "4.0.0rc7")).expect("upgrade");
    assert!(report.success);
    assert_eq!(
        report.applied(),
        vec!["3.2.0rc28_github_diff_attributes".to_string()]
    );
    assert!(report.metadata_written);
    let metadata = ProjectMetadata::load(&root.join(".kittify")).expect("metadata");
    assert_eq!(metadata.version, "4.0.0rc7");
    assert_eq!(
        metadata.schema_version,
        Some(i64::from(CURRENT_SCHEMA_VERSION)),
        "a successful run stamps the required schema version"
    );
    let text = std::fs::read_to_string(root.join(".kittify/metadata.yaml")).unwrap();
    assert!(text.contains("schema_capabilities:"), "{text}");
    assert!(text.contains("canonical_context: true"), "{text}");

    let after_first = std::fs::read_to_string(root.join(".kittify/metadata.yaml")).unwrap();
    let ledger_after_first = records(root, false).len();
    let again = run_upgrade(&UpgradeRequest::new(root, "4.0.0rc7")).expect("rerun");
    assert!(again.success);
    assert!(again.rows.is_empty(), "{:?}", again.rows);
    assert!(
        again
            .warnings
            .iter()
            .any(|warning| warning.starts_with("No migrations needed from")),
        "{:?}",
        again.warnings
    );
    let after_second = std::fs::read_to_string(root.join(".kittify/metadata.yaml")).unwrap();
    assert!(
        !again.metadata_written,
        "a no-op rerun must not rewrite\n{after_first}\n---\n{after_second}"
    );
    assert_eq!(after_second, after_first);
    assert_eq!(records(root, false).len(), ledger_after_first);
}

#[cfg(unix)]
#[test]
fn a_linked_worktree_skips_the_spelled_exclude_and_writes_the_shared_one() {
    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path().join("proj");
    initialized(&root, "4.0.0rc7", 3);
    git_repo(&root);
    std::fs::write(root.join("README.md"), "base\n").unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "base"]);
    // Only the two exclude migrations are pending in this fixture.
    seed_all_except(
        &root,
        "4.0.0rc7",
        &["0.13.1_exclude_worktrees", "4.0.0rc7_exclude_spec_kitty"],
    );
    git(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            ".worktrees/lane",
            "-b",
            "feature/x",
        ],
    );
    let lane = root.join(".worktrees/lane");
    assert!(
        lane.join(".git").is_file(),
        "a linked worktree's .git must be a file for this test to mean anything"
    );
    std::fs::write(
        lane.join(".kittify/metadata.yaml"),
        "spec_kitty:\n  version: 3.2.0\n  schema_version: 2\n",
    )
    .unwrap();
    git(&lane, &["add", "-A"]);
    git(&lane, &["commit", "-qm", "lane seed"]);

    let scope: BTreeSet<String> = BTreeSet::from([
        "0.13.1_exclude_worktrees".to_string(),
        "4.0.0rc7_exclude_spec_kitty".to_string(),
    ]);
    let report = run_upgrade(&UpgradeRequest::new(&lane, "4.0.0rc7").with_scope(scope))
        .expect("the worktree run continues");
    assert!(report.success, "{:?}", report.errors);
    assert_eq!(
        report.applied(),
        vec!["4.0.0rc7_exclude_spec_kitty".to_string()],
        "the common-dir migration owns the write in a linked worktree"
    );
    assert_eq!(
        report.skipped(),
        vec!["0.13.1_exclude_worktrees".to_string()],
        "the spelled `.git/info/exclude` does not exist in a linked worktree"
    );
    assert!(!lane.join(".git/info/exclude").exists());
    let shared = std::fs::read_to_string(root.join(".git/info/exclude")).unwrap();
    assert!(shared.contains(".spec-kitty/"), "{shared}");
    assert!(!shared.contains("Added by spec-kitty") || shared.contains(".spec-kitty/"));
    assert!(!lane.join(".worktrees").exists());
}

#[test]
fn an_operator_capability_map_is_never_replaced_by_the_stamp() {
    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path();
    initialized(root, "4.0.0rc7", 3);
    git_repo(root);
    std::fs::write(
        root.join(".kittify/metadata.yaml"),
        "# Spec Kitty Project Metadata\nspec_kitty:\n  schema_version: 3\n  version: 4.0.0rc7\n  initialized_at: '2026-01-01T00:00:00+00:00'\n  last_upgraded_at:\n  schema_capabilities:\n    canonical_context: false\n    my_operator_flag: true\nproject:\n  uuid: keep-me\n",
    )
    .unwrap();
    seed_all_except(root, "4.0.0rc7", &["3.2.0rc28_github_diff_attributes"]);

    let report = run_upgrade(&UpgradeRequest::new(root, "4.0.0rc7")).expect("upgrade");
    assert!(report.success);
    let text = std::fs::read_to_string(root.join(".kittify/metadata.yaml")).unwrap();
    assert!(text.contains("canonical_context: false"), "{text}");
    assert!(text.contains("my_operator_flag: true"), "{text}");
    assert!(!text.contains("event_log_authority"), "{text}");
    assert!(text.contains("uuid: keep-me"), "{text}");
}
