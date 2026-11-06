//! CF16: the four missing real dry-run doors plus typed failure-phase execution.
//!
//! Every assertion here is about behaviour that was previously absent or wrong:
//!
//! * the four effects whose `apply(dry_run=True)` had no native door (both
//!   provenance heals, the bundle fold, the project-intent rename) now preview
//!   through the SAME composition their real `apply` uses, and the change lines a
//!   preview reports are the ones the real run writes;
//! * the preview is reached through the canonical dispatch
//!   (`evaluate_step(dry_run = true)` -> `preview_for(id)` -> the migration's own
//!   door), and it writes nothing;
//! * `MigrationStateUnreadableError` is a typed native failure, and the runner's
//!   MAIN and WORKTREE phases keep Source's distinct catch/ledger rules: main
//!   returns a FAILED `MigrationResult` with `Cannot detect/apply {id}: ...` (a
//!   detect failure is recorded, an apply failure is NOT), while in a worktree a
//!   detect error is recorded and an apply error is not.
//!
//! Scope and honesty notes:
//!
//! * These are focused functional checks with hand-built fixtures. They are NOT a
//!   paired Source run (no isolated Python child is spawned here) and NOT a
//!   failure/Unicode/performance matrix.
//! * Parent registration now exposes the six existing callbacks through the ONE
//!   canonical registry. These per-step tests use that registered lookup;
//!   cf16_parent_registered_charter separately exercises run_upgrade reachability.
//!   Registration is not whole-effect or Source-equivalence approval.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use sk_install::migrations::completion_modern::charter_synthesis::{
    fold, legacy_rename, provenance,
};
use sk_install::upgrade::{StepStatus, evaluate_step, evaluate_worktree_step, preview_for};
use sk_install::{InstallError, MigrationCatalog, MigrationLedger, MigrationSpec, NativeMigration};

const HEAL: &str = "3.2.7_heal_provenance_paths";
const TEMPLATE_SET: &str = "4_0_0rc5_heal_template_set_provenance";
const RENAME: &str = "4.0.0rc8_project_intent_directive_rename";
const FOLD: &str = "consolidate_charter_bundle_fold";
const CUTOVER: &str = "charter_pack_cutover";

fn write(root: &Path, name: &str, text: &str) {
    let path = root.join(name);
    std::fs::create_dir_all(path.parent().expect("parent")).unwrap();
    std::fs::write(path, text).unwrap();
}

fn read(root: &Path, name: &str) -> String {
    std::fs::read_to_string(root.join(name)).unwrap()
}

/// A minimal initialized project: the config/metadata pair the runner requires,
/// plus a `.git` directory so repository-gated migrations are eligible.
fn project(version: &str) -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("temp dir");
    let root = directory.path();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    write(
        root,
        ".kittify/config.yaml",
        "agents:\n  available:\n  - claude\n",
    );
    write(
        root,
        ".kittify/metadata.yaml",
        &format!(
            "# Spec Kitty Project Metadata\nspec_kitty:\n  schema_version: 3\n  version: {version}\n  initialized_at: '2026-01-01T00:00:00+00:00'\n  last_upgraded_at:\n"
        ),
    );
    directory
}

/// A built-in pack root (`<root>/packs/built-in`) with one mission config, so the
/// provenance tokenizer derives the portable token from a real tier root.
fn built_in(root: &Path) -> PathBuf {
    let pack = root.join("packs/built-in");
    std::fs::create_dir_all(pack.join("missions/software-dev")).unwrap();
    std::fs::write(
        pack.join("missions/software-dev/mission.yaml"),
        "mission: software-dev\n",
    )
    .unwrap();
    std::fs::create_dir_all(pack.join("toolguide")).unwrap();
    std::fs::write(pack.join("toolguide/rtk.toolguide.yaml"), "id: rtk\n").unwrap();
    pack
}

fn spec(id: &str) -> MigrationSpec {
    MigrationCatalog::embedded()
        .expect("catalog")
        .get(id)
        .expect("declared")
        .clone()
}

/// The canonical registered `NativeMigration`, without an own-module fallback.
fn native(id: &str) -> &'static NativeMigration {
    sk_install::native_migrations()
        .find(|migration| migration.id == id)
        .unwrap_or_else(|| panic!("no registered native implementation for {id}"))
}

/// The ledger results recorded for `id`, in append order.
fn records(root: &Path, id: &str) -> Vec<String> {
    let ledger = MigrationLedger::new(root);
    let mut results = Vec::new();
    for clone_local in [false, true] {
        for record in ledger.records(clone_local).expect("records") {
            if record.id == id {
                results.push(record.result);
            }
        }
    }
    results
}

fn main_step(root: &Path, id: &str, dry_run: bool) -> sk_install::upgrade::StepResult {
    evaluate_step(
        root,
        &spec(id),
        native(id),
        &MigrationLedger::new(root),
        "4.0.0rc8",
        dry_run,
        &BTreeSet::new(),
    )
    .expect("evaluates the step")
}

// --------------------------------------------------------------------------- #
// 1. The two provenance heals: preview and apply share one composition.
// --------------------------------------------------------------------------- #

#[test]
fn the_provenance_preview_reports_the_lines_the_apply_writes() {
    let directory = project("4.0.0rc8");
    let root = directory.path();
    let pack = built_in(root);
    let absolute = pack.join("toolguide/rtk.toolguide.yaml");
    write(
        root,
        ".kittify/charter/charter.yaml",
        &format!(
            "catalog:\n  references:\n  - id: TOOLGUIDE:rtk\n    kind: toolguide\n    source_path: {}\n",
            absolute.display()
        ),
    );
    let before = read(root, ".kittify/charter/charter.yaml");

    let preview = provenance::preview_with(root, Some(&pack)).expect("preview");
    assert!(preview.changed);
    assert_eq!(preview.changes.len(), 1, "{:?}", preview.changes);
    let line = preview.changes[0].clone();
    assert!(
        line.starts_with("charter.yaml catalog[TOOLGUIDE:rtk].source_path: "),
        "{line}"
    );
    assert!(
        line.contains("-> ${SPEC_KITTY_PACKS_ROOT}/built-in/toolguide/rtk.toolguide.yaml"),
        "the preview must report the portable token, not a boolean: {line}"
    );
    // A preview writes nothing at all.
    assert_eq!(read(root, ".kittify/charter/charter.yaml"), before);

    let applied = provenance::apply_with(root, Some(&pack)).expect("apply");
    assert_eq!(applied.changes, vec![line.clone()]);
    let after = read(root, ".kittify/charter/charter.yaml");
    assert_ne!(after, before);
    assert!(
        after.contains("${SPEC_KITTY_PACKS_ROOT}/built-in/toolguide/rtk.toolguide.yaml"),
        "{after}"
    );
}

#[test]
fn the_template_set_preview_heals_the_row_3_2_7_left_alone() {
    let directory = project("4.0.0rc8");
    let root = directory.path();
    let pack = built_in(root);
    let absolute = pack.join("missions/software-dev/mission.yaml");
    write(
        root,
        ".kittify/charter/charter.yaml",
        &format!(
            "catalog:\n  mission: software-dev\n  template_set: software-dev-default\n  references:\n  - id: TEMPLATE_SET:software-dev-default\n    kind: template_set\n    source_path: {}\n",
            absolute.display()
        ),
    );
    let before = read(root, ".kittify/charter/charter.yaml");

    let preview = provenance::preview_template_set_with(root, Some(&pack)).expect("preview");
    assert!(preview.changed);
    assert_eq!(preview.changes.len(), 1, "{:?}", preview.changes);
    let line = preview.changes[0].clone();
    assert!(
        line.contains("-> ${SPEC_KITTY_PACKS_ROOT}/built-in/missions/software-dev/mission.yaml"),
        "{line}"
    );
    assert_eq!(read(root, ".kittify/charter/charter.yaml"), before);

    let applied = provenance::apply_template_set_with(root, Some(&pack)).expect("apply");
    assert_eq!(applied.changes, vec![line]);
    assert_ne!(read(root, ".kittify/charter/charter.yaml"), before);
}

// --------------------------------------------------------------------------- #
// 2. The fold and the rename: real doors, reached through the dispatch.
// --------------------------------------------------------------------------- #

/// Every id the preview dispatch knows, and the ones it deliberately does not.
#[test]
fn the_preview_dispatch_covers_exactly_the_six_effects() {
    for id in [
        HEAL,
        TEMPLATE_SET,
        RENAME,
        FOLD,
        CUTOVER,
        "4.0.0rc8_promote_interview_selections",
    ] {
        assert!(preview_for(id).is_some(), "{id} must have a dry-run door");
    }
    // A migration without a door reports its verdict alone; it must not be given a
    // fabricated one.
    assert!(preview_for("2.0.9_state_gitignore").is_none());
    assert!(preview_for("no-such-migration").is_none());
}

#[test]
fn the_bundle_fold_preview_runs_through_the_dispatch_and_writes_nothing() {
    let directory = project("4.0.0rc8");
    let root = directory.path();
    write(root, ".kittify/charter/directives.yaml", "directives: []\n");
    write(
        root,
        ".kittify/config.yaml",
        "agents:\n  available:\n  - claude\nactivated_tactics:\n- local-first\n",
    );
    let config_before = read(root, ".kittify/config.yaml");
    let directive_before = read(root, ".kittify/charter/directives.yaml");

    let step = main_step(root, FOLD, true);
    assert_eq!(step.status, StepStatus::Applied);
    assert!(!step.recorded);
    let charter_yaml = root.join(".kittify/charter/charter.yaml");
    let directives = root.join(".kittify/charter/directives.yaml");
    assert_eq!(
        step.outcome.changes,
        vec![
            format!(
                "dry-run: would compose {} from legacy bundle files",
                charter_yaml.display()
            ),
            format!("dry-run: would delete {}", directives.display()),
            "dry-run: would relocate activated_* keys from config.yaml onto charter.yaml and add the charter: pointer"
                .to_string(),
        ],
        "the preview must be the reference's own dry-run summary"
    );

    // Nothing was composed, deleted, re-pointed or recorded.
    assert!(!charter_yaml.exists());
    assert_eq!(
        read(root, ".kittify/charter/directives.yaml"),
        directive_before
    );
    assert_eq!(read(root, ".kittify/config.yaml"), config_before);
    assert!(records(root, FOLD).is_empty());

    // The real run then writes exactly what the preview reported.
    let step = main_step(root, FOLD, false);
    assert_eq!(step.status, StepStatus::Applied);
    assert!(charter_yaml.is_file());
    assert!(!directives.exists());
    assert_eq!(
        read(root, ".kittify/config.yaml"),
        "agents:\n  available:\n  - claude\ncharter: .kittify/charter/charter.yaml\n"
    );
    assert_eq!(records(root, FOLD), vec!["success".to_string()]);
}

#[test]
fn the_rename_preview_prefixes_would_and_renames_nothing() {
    let directory = project("4.0.0rc8");
    let root = directory.path();
    let legacy = ".kittify/charter-packs/directive/001-mission-type-scope-directive.directive.yaml";
    write(root, legacy, "id: mission-type-scope-directive\n");
    let source = root.join(legacy);

    let step = main_step(root, RENAME, true);
    assert_eq!(step.status, StepStatus::Applied);
    assert!(!step.recorded);
    assert!(!step.outcome.changes.is_empty());
    for change in &step.outcome.changes {
        assert!(change.starts_with("Would "), "{change}");
    }
    assert!(
        step.outcome
            .changes
            .iter()
            .any(|change| change.contains("project-intent-directive")),
        "{:?}",
        step.outcome.changes
    );
    // The preview renamed nothing and wrote nothing.
    assert!(source.is_file());
    let mut siblings: Vec<String> = std::fs::read_dir(source.parent().unwrap())
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect();
    siblings.sort();
    assert_eq!(
        siblings,
        vec!["001-mission-type-scope-directive.directive.yaml".to_string()]
    );
}

#[test]
fn the_rename_preview_reports_a_refusal_as_a_failed_result() {
    let directory = project("4.0.0rc8");
    let root = directory.path();
    write(
        root,
        ".kittify/charter-packs/directive/001-mission-type-scope-directive.directive.yaml",
        "id: mission-type-scope-directive\n",
    );
    // The target name already exists, so the rename must refuse.
    write(
        root,
        ".kittify/charter-packs/directive/001-project-intent-directive.directive.yaml",
        "id: project-intent-directive\n",
    );

    let preview = legacy_rename::preview(root).expect("a refusal is a result, not an error");
    assert!(!preview.success, "a refusal is never a successful preview");
    assert!(preview.changes.is_empty());
    assert_eq!(preview.errors.len(), 1, "{:?}", preview.errors);
    assert!(
        preview.errors[0].contains("Refused to rename the legacy 'mission-type-scope-directive'"),
        "{:?}",
        preview.errors
    );
    assert!(
        root.join(
            ".kittify/charter-packs/directive/001-mission-type-scope-directive.directive.yaml"
        )
        .is_file()
    );
    // A refusal is also a FAILED step, not a crash and not a success.
    let step = main_step(root, RENAME, false);
    assert_eq!(step.status, StepStatus::Failed);
    assert!(!step.outcome.success);
    assert!(!step.outcome.errors.is_empty());
}

// --------------------------------------------------------------------------- #
// 3. Typed failure phases: main returns a failed result, and the ledger differs.
// --------------------------------------------------------------------------- #

#[test]
fn an_unreadable_state_premise_is_a_recorded_main_failure() {
    let directory = project("4.0.0rc8");
    let root = directory.path();
    // An unclosed flow sequence: not parseable YAML.
    write(
        root,
        ".kittify/charter/charter.yaml",
        "catalog: [unclosed\n",
    );

    // The typed failure is a RESULT (main stops with the report), never a raised error.
    let step = main_step(root, TEMPLATE_SET, false);
    assert_eq!(step.status, StepStatus::Failed);
    assert!(!step.outcome.success);
    assert_eq!(step.outcome.errors.len(), 1, "{:?}", step.outcome.errors);
    let error = &step.outcome.errors[0];
    assert!(
        error.starts_with(&format!("Cannot safely detect {TEMPLATE_SET}: ")),
        "{error}"
    );
    assert!(
        error.contains("could not be read (YAMLError); provenance was not evaluated"),
        "the original typed cause must survive: {error}"
    );
    // Source records the detect failure (it stays retryable).
    assert!(step.recorded);
    assert_eq!(records(root, TEMPLATE_SET), vec!["failed".to_string()]);
}

#[test]
fn an_unreadable_apply_premise_fails_without_a_ledger_record() {
    let directory = project("4.0.0rc6");
    let root = directory.path();
    // The cutover's apply reads this layout first; an unparseable config is the
    // typed state-unreadable premise.
    write(root, ".kittify/config.yaml", "agents: [unclosed\n");

    let step = main_step(root, CUTOVER, false);
    assert_eq!(step.status, StepStatus::Failed);
    assert!(!step.outcome.success);
    assert_eq!(step.outcome.errors.len(), 1, "{:?}", step.outcome.errors);
    let error = &step.outcome.errors[0];
    assert!(
        error.starts_with(&format!("Cannot apply {CUTOVER}: ")),
        "{error}"
    );
    // Source has two shapes for this premise and both are `MigrationStateUnreadableError`:
    // the layout preflight's "{file} is not readable YAML" and `_unreadable`'s
    // "{file} could not be read ({exc})". The config reader reaches the second first.
    assert!(error.contains(".kittify/config.yaml"), "{error}");
    assert!(
        error.contains("could not be read") || error.contains("is not readable YAML"),
        "{error}"
    );
    assert!(
        error.contains("fix the file, then run `spec-kitty upgrade` again"),
        "{error}"
    );
    // Source's apply() catch returns BEFORE the record step: no ledger record, so
    // the next run retries instead of settling the migration.
    assert!(!step.recorded);
    assert!(records(root, CUTOVER).is_empty());
}

#[test]
fn the_worktree_phases_keep_the_sources_record_split() {
    // A worktree detect failure IS recorded and flagged errored.
    let detected = project("4.0.0rc8");
    let detected_root = detected.path();
    write(
        detected_root,
        ".kittify/charter/charter.yaml",
        "catalog: [unclosed\n",
    );
    let step = evaluate_worktree_step(
        detected_root,
        &spec(TEMPLATE_SET),
        native(TEMPLATE_SET),
        &MigrationLedger::new(detected_root),
        "4.0.0rc8",
        false,
    )
    .expect("evaluates");
    assert_eq!(step.status, StepStatus::Failed);
    assert!(step.errored);
    assert!(step.recorded);
    assert!(
        step.failure
            .as_deref()
            .expect("failure line")
            .contains("Cannot safely detect")
    );
    assert_eq!(
        records(detected_root, TEMPLATE_SET),
        vec!["failed".to_string()]
    );

    // A worktree apply failure is NOT recorded, only flagged errored.
    let applied = project("4.0.0rc6");
    let applied_root = applied.path();
    write(applied_root, ".kittify/config.yaml", "agents: [unclosed\n");
    let step = evaluate_worktree_step(
        applied_root,
        &spec(CUTOVER),
        native(CUTOVER),
        &MigrationLedger::new(applied_root),
        "4.0.0rc8",
        false,
    )
    .expect("evaluates");
    assert_eq!(step.status, StepStatus::Failed);
    assert!(step.errored);
    assert!(!step.recorded, "Source appends no failed record here");
    assert!(records(applied_root, CUTOVER).is_empty());
}

// --------------------------------------------------------------------------- #
// 4. The typed failure carries its own stable code.
// --------------------------------------------------------------------------- #

#[test]
fn the_state_unreadable_failure_has_its_own_stable_code() {
    let failure = InstallError::MigrationStateUnreadable {
        path: Some(PathBuf::from(".kittify/charter/charter.yaml")),
        detail: "could not be read (YAMLError); provenance was not evaluated".to_string(),
    };
    assert_eq!(failure.code(), "migration_state_unreadable");
    assert!(
        failure.to_string().contains("provenance was not evaluated"),
        "the retained cause is the message: {failure}"
    );
    // The unrelated refusal keeps its own code: the two classes never collapse.
    let refusal = InstallError::UpgradeRefused {
        detail: "no stale built-in template_set source_path found".to_string(),
    };
    assert_eq!(refusal.code(), "upgrade_refused");
}

#[test]
fn the_fold_door_is_the_one_the_dispatch_calls() {
    let directory = project("4.0.0rc8");
    let root = directory.path();
    write(root, ".kittify/charter/references.yaml", "references: []\n");
    let direct = fold::preview(root).expect("preview");
    assert!(direct.changed);
    let dispatched = preview_for(FOLD).expect("door")(root).expect("preview");
    assert_eq!(direct.changes, dispatched.changes);
    assert_eq!(
        direct.changes,
        vec![
            format!(
                "dry-run: would compose {} from legacy bundle files",
                root.join(".kittify/charter/charter.yaml").display()
            ),
            format!(
                "dry-run: would delete {}",
                root.join(".kittify/charter/references.yaml").display()
            ),
        ]
    );
}
