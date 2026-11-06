//! Native `upgrade`: version choice, `requires_core`-independent migration
//! selection, ordered execution and the ledger/metadata write-back.
//!
//! Selection is **declared minus applied**, never a version window
//! (ADR 2026-10-17-1 §3): a catalog migration is selected when the partition
//! authoritative for it has no `success` record, capped by an optional
//! `--target` upper bound. `detect()` is the *idempotency* guard, so an
//! already-target project records `skipped` rather than re-applying.
//!
//! **Fail-closed execution.** The catalog has 136 declared migrations; this
//! crate implements the source-read families registered in
//! [`native_migrations`], combining the original [`NATIVE_MIGRATIONS`] tranche
//! with the legacy and modern completion slices. Registration is not evidence
//! of complete Source workflow fidelity. If the selection
//! contains any migration without a native implementation the real run refuses
//! *before the first write*, naming every id. That is deliberate: silently
//! dropping an old project's migration would claim a compatibility the binary
//! does not have. `dry_run` reports the same split without refusing. A
//! migration whose `detect` is true but whose `can_apply` refuses this project
//! is a skip that is **not** recorded, so it is retried after the project
//! changes; a `detect`/`can_apply` that cannot evaluate the project at all is a
//! recorded failure that stops the run and leaves the version untouched.
//!
//! Ordering is catalog order (the upstream `order_manifest.txt`), not version
//! sort. Each applied migration writes one ledger record; a failure records
//! `failed` and stops. The project version is only advanced after the whole run
//! succeeds, matching upstream's "failed run leaves the version untouched".
//! Records are de-duplicated against the held result, so a re-run that reaches
//! the same verdict appends nothing; a superseded migration's record carries
//! its successor note, which is how a successor tells that no-op apart from the
//! shipped migration's own result.
//!
//! `--target` and `--agent-check`/`--agent-choice` are represented as typed
//! requests. The agent check is a pure decision surface: the CLI supplies the
//! already-fetched latest version and install safety, so this crate makes no
//! network call and writes no per-user nag cache.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use crate::error::InstallError;
use crate::ledger::{
    FAILED_RESULT, LedgerRecord, MigrationCatalog, MigrationLedger, MigrationSpec, SETTLED_RESULT,
    SKIPPED_RESULT,
};
use crate::lock::ProjectLock;
use crate::metadata::{
    CURRENT_SCHEMA_VERSION, ProjectMetadata, VersionStamp, stamp_schema_version,
};
use crate::plan::now_utc_iso;
use crate::version::{Version, check_requires_core};

/// Detect-input predecessors of SHIPPED migrations whose `detect()` reads state another
/// migration in the same run writes, declared without editing them (Source
/// `runner.SEQUENCED_AFTER`; a shipped module's bytes are its `content_hash`).
///
/// This is **not** ordering: the declared catalogue's order stays the single execution
/// authority. A declaration only tells a preview which migration's write a later
/// `detect()` reads, so the preview can report what the real, sequential run reaches
/// (#6552). `3.2.6_issue_matrix_driver_repoint` reads the merge-driver config the
/// `3.2.6_gate_artifact_merge_drivers` migration writes.
pub const SEQUENCED_AFTER: [(&str, &[&str]); 1] = [(
    "3.2.6_issue_matrix_driver_repoint",
    &["3.2.6_gate_artifact_merge_drivers"],
)];

/// Every predecessor migration `id` declares (Source `declared_predecessors`).
fn declared_predecessors(id: &str) -> &'static [&'static str] {
    SEQUENCED_AFTER
        .iter()
        .find_map(|(migration_id, predecessors)| (*migration_id == id).then_some(*predecessors))
        .unwrap_or(&[])
}

/// The declared predecessors of `id` that the caller has decided apply before it.
///
/// The ONE sequencing decision, shared by the read-only forecast and the dry-run loop: a
/// declared id the caller has not decided — not selected, skipped or unknown — wrote
/// nothing here, so it changes nothing.
fn sequenced_predecessors(id: &str, applied: &BTreeSet<String>) -> Vec<String> {
    declared_predecessors(id)
        .iter()
        .filter(|predecessor| applied.contains(**predecessor))
        .map(|predecessor| (*predecessor).to_string())
        .collect()
}

/// One real file persisted before a non-atomic migration aborted (Source FR-005).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MigrationPartialWrite {
    /// The mission to which the persisted file belongs.
    pub mission: String,
    /// The actual persisted path, not a proposed write or rollback guess.
    pub path: String,
}

/// The complete result of a migration, distinct from an uncaught exception.
///
/// `success = false` retains the report, errors and real partial writes. It is
/// never converted to a success-with-warning or discarded by a generic refusal.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MigrationOutcome {
    /// Source's result flag, independent of whether partial writes occurred.
    pub success: bool,
    /// Human-readable changes, including a migration's structured report JSON.
    #[serde(rename = "changes_made")]
    pub changes: Vec<String>,
    /// Source's migration errors; a failed result may legitimately have none.
    pub errors: Vec<String>,
    /// Non-fatal warnings (preserved content, skipped optional work).
    pub warnings: Vec<String>,
    /// Whether operator review is required even when the migration succeeded.
    pub manual_review_required: bool,
    /// Exact reported paths retained for review, never inferred from names.
    pub preserved_paths: Vec<String>,
    /// Declared predecessors used for a dry-run prediction, not a real detection.
    pub predicted_after: Vec<String>,
    /// Actual writes that survived a non-atomic abort.
    pub partial_writes: Vec<MigrationPartialWrite>,
    /// Whether the migration changed anything (Native convenience, not Source wire).
    #[serde(skip)]
    pub changed: bool,
}

impl Default for MigrationOutcome {
    /// The existing Native successful-no-change constructor contract.
    fn default() -> Self {
        Self {
            changed: false,
            changes: Vec::new(),
            warnings: Vec::new(),
            success: true,
            errors: Vec::new(),
            manual_review_required: false,
            preserved_paths: Vec::new(),
            predicted_after: Vec::new(),
            partial_writes: Vec::new(),
        }
    }
}

impl MigrationOutcome {
    /// A migration that found nothing to do.
    pub fn skipped() -> Self {
        Self::default()
    }

    /// A migration that found nothing to do but still reports why.
    pub fn unchanged(changes: Vec<String>) -> Self {
        Self {
            changes,
            ..Self::default()
        }
    }

    /// A migration that changed something.
    pub fn changed(changes: Vec<String>) -> Self {
        Self {
            changed: true,
            changes,
            ..Self::default()
        }
    }
}

/// A native implementation: `detect`, `can_apply` plus `apply`.
#[derive(Clone, Copy)]
pub struct NativeMigration {
    /// The catalog id.
    pub id: &'static str,
    /// Whether the old state is still present (upstream `detect`).
    pub detect: fn(&Path) -> Result<bool, InstallError>,
    /// The upstream `can_apply` refusal (a skip that records nothing), if any.
    pub can_apply: fn(&Path) -> Result<Option<String>, InstallError>,
    /// Apply the transformation (upstream `apply`, non-dry-run).
    pub apply: fn(&Path) -> Result<MigrationOutcome, InstallError>,
}

impl NativeMigration {
    /// Source's per-worktree execution flag, read from the ONE catalogue.
    ///
    /// Keeping this as an accessor preserves existing callback initializers and
    /// avoids a second field/table that could disagree with MigrationSpec.
    pub fn runs_on_worktrees(&self) -> Result<bool, InstallError> {
        MigrationCatalog::embedded()?
            .get(self.id)
            .map(|spec| spec.runs_on_worktrees)
            .ok_or_else(|| InstallError::MigrationCatalogInvalid {
                detail: format!(
                    "native migration {:?} has no declared execution metadata",
                    self.id
                ),
            })
    }
}

/// A migration that has no project-shape refusal beyond existence.
pub fn always_applicable(_project_root: &Path) -> Result<Option<String>, InstallError> {
    Ok(None)
}

macro_rules! merge_driver_native {
    ($detect:ident, $apply:ident, $id:literal) => {
        fn $detect(project_root: &Path) -> Result<bool, InstallError> {
            crate::migrations::merge_drivers::detect_for($id, project_root)
        }
        fn $apply(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
            crate::migrations::merge_drivers::apply_for($id, project_root)
        }
    };
}

merge_driver_native!(
    detect_event_log_merge_driver,
    apply_event_log_merge_driver,
    "3.1.1_event_log_merge_driver"
);
merge_driver_native!(
    detect_decisions_event_log_merge_driver,
    apply_decisions_event_log_merge_driver,
    "3.2.6_decisions_event_log_merge_driver"
);
merge_driver_native!(
    detect_gate_artifact_merge_drivers,
    apply_gate_artifact_merge_drivers,
    "3.2.6_gate_artifact_merge_drivers"
);
merge_driver_native!(
    detect_issue_matrix_driver_repoint,
    apply_issue_matrix_driver_repoint,
    "3.2.6_issue_matrix_driver_repoint"
);
merge_driver_native!(
    detect_meta_traces_merge_drivers,
    apply_meta_traces_merge_drivers,
    "3.2.6_meta_traces_merge_drivers"
);
merge_driver_native!(
    detect_review_cycle_merge_driver,
    apply_review_cycle_merge_driver,
    "3.2.7_review_cycle_merge_driver"
);
merge_driver_native!(
    detect_decision_index_merge_driver,
    apply_decision_index_merge_driver,
    "4.0.0rc5_decision_index_merge_driver"
);

macro_rules! retired_native {
    ($detect:ident, $can_apply:ident, $apply:ident, $id:literal) => {
        fn $detect(project_root: &Path) -> Result<bool, InstallError> {
            crate::migrations::retired::detect(project_root)
        }
        fn $can_apply(project_root: &Path) -> Result<Option<String>, InstallError> {
            crate::migrations::retired::can_apply_for($id, project_root)
        }
        fn $apply(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
            crate::migrations::retired::apply_for($id, project_root)
        }
    };
}

retired_native!(
    detect_retired_frontmatter_only_lanes,
    can_apply_retired_frontmatter_only_lanes,
    apply_retired_frontmatter_only_lanes,
    "0.9.0_frontmatter_only_lanes"
);
retired_native!(
    detect_retired_complete_migration,
    can_apply_retired_complete_migration,
    apply_retired_complete_migration,
    "0.9.1_complete_migration"
);
retired_native!(
    detect_retired_charter_cleanup,
    can_apply_retired_charter_cleanup,
    apply_retired_charter_cleanup,
    "0.10.12_charter_cleanup"
);
retired_native!(
    detect_retired_charter_directory,
    can_apply_retired_charter_directory,
    apply_retired_charter_directory,
    "2.0.0_charter_directory"
);
retired_native!(
    detect_retired_charter_context_bootstrap,
    can_apply_retired_charter_context_bootstrap,
    apply_retired_charter_context_bootstrap,
    "2.0.2_charter_context_bootstrap"
);
retired_native!(
    detect_retired_consistency_sweep,
    can_apply_retired_consistency_sweep,
    apply_retired_consistency_sweep,
    "2.0.6_consistency_sweep"
);
retired_native!(
    detect_retired_charter_doctrine_skill,
    can_apply_retired_charter_doctrine_skill,
    apply_retired_charter_doctrine_skill,
    "2.1.2_fix_charter_doctrine_skill"
);
retired_native!(
    detect_retired_glossary_context_skill,
    can_apply_retired_glossary_context_skill,
    apply_retired_glossary_context_skill,
    "2.1.2_fix_glossary_context_skill"
);
retired_native!(
    detect_retired_enforce_command_file_state,
    can_apply_retired_enforce_command_file_state,
    apply_retired_enforce_command_file_state,
    "2.1.4_enforce_command_file_state"
);
retired_native!(
    detect_retired_restore_prompt_commands,
    can_apply_retired_restore_prompt_commands,
    apply_retired_restore_prompt_commands,
    "3.0.2_restore_prompt_commands"
);
retired_native!(
    detect_retired_normalize_status_json,
    can_apply_retired_normalize_status_json,
    apply_retired_normalize_status_json,
    "3.1.1_normalize_status_json"
);
retired_native!(
    detect_retired_default_charter_pack,
    can_apply_retired_default_charter_pack,
    apply_retired_default_charter_pack,
    "3.2.0rc35_default_charter_pack"
);
retired_native!(
    detect_retired_agents_skills_backfill,
    can_apply_retired_agents_skills_backfill,
    apply_retired_agents_skills_backfill,
    "3.2.5_agents_skills_gitignore_backfill"
);
retired_native!(
    detect_retired_narrow_cursor_gitignore,
    can_apply_retired_narrow_cursor_gitignore,
    apply_retired_narrow_cursor_gitignore,
    "3.2.6rc3_narrow_cursor_gitignore"
);
retired_native!(
    detect_retired_normalize_activation_absence,
    can_apply_retired_normalize_activation_absence,
    apply_retired_normalize_activation_absence,
    "normalize_activation_absence"
);
retired_native!(
    detect_retired_unify_promote_answers,
    can_apply_retired_unify_promote_answers,
    apply_retired_unify_promote_answers,
    "unify_charter_activation_promote_answers"
);

retired_native!(
    detect_retired_historical_status,
    can_apply_retired_historical_status,
    apply_retired_historical_status,
    "2.0.0_historical_status_migration"
);
retired_native!(
    detect_retired_repair_templates,
    can_apply_retired_repair_templates,
    apply_retired_repair_templates,
    "0.10.9_repair_templates"
);

/// Every migration implemented natively in this crate.
pub const NATIVE_MIGRATIONS: [NativeMigration; 61] = [
    NativeMigration {
        id: crate::migrations::ledger_adoption::BACKFILL_ID,
        detect: crate::migrations::ledger_adoption::detect,
        can_apply: always_applicable,
        apply: crate::migrations::ledger_adoption::apply,
    },
    NativeMigration {
        id: "2.0.0_retire_git_hooks",
        detect: crate::migrations::retirements::detect_retire_git_hooks,
        can_apply: crate::migrations::retirements::can_apply_retire_git_hooks,
        apply: crate::migrations::retirements::apply_retire_git_hooks,
    },
    NativeMigration {
        id: "2.0.11_remove_clarify_command",
        detect: crate::migrations::retirements::detect_remove_clarify_command,
        can_apply: always_applicable,
        apply: crate::migrations::retirements::apply_remove_clarify_command,
    },
    NativeMigration {
        id: "2.1.2_remove_release_skill",
        detect: crate::migrations::retirements::detect_remove_release_skill,
        can_apply: crate::migrations::retirements::can_apply_remove_release_skill,
        apply: crate::migrations::retirements::apply_remove_release_skill,
    },
    NativeMigration {
        id: "3.2.0rc43_retire_profile_context_command",
        detect: crate::migrations::retirements::detect_retire_profile_context_command,
        can_apply: always_applicable,
        apply: crate::migrations::retirements::apply_retire_profile_context_command,
    },
    NativeMigration {
        id: "3.2.0rc45_retire_standalone_skill_surface",
        detect: crate::migrations::retirements::detect_retire_standalone_skill_surface,
        can_apply: always_applicable,
        apply: crate::migrations::retirements::apply_retire_standalone_skill_surface,
    },
    NativeMigration {
        id: "4.0.0rc5_retire_bundled_dashboard",
        detect: crate::migrations::retirements::detect_retire_bundled_dashboard,
        can_apply: always_applicable,
        apply: crate::migrations::retirements::apply_retire_bundled_dashboard,
    },
    NativeMigration {
        id: "0_9_4_roo_deprecation",
        detect: crate::migrations::retirements::detect_roo_deprecation,
        can_apply: always_applicable,
        apply: crate::migrations::retirements::apply_roo_deprecation,
    },
    NativeMigration {
        id: "4_0_0_retired_hosted_target",
        detect: crate::migrations::retirements::detect_retired_hosted_target,
        can_apply: crate::migrations::retirements::can_apply_retired_hosted_target,
        apply: crate::migrations::retirements::apply_retired_hosted_target,
    },
    NativeMigration {
        id: "0.13.1_exclude_worktrees",
        detect: detect_exclude_worktrees,
        can_apply: always_applicable,
        apply: apply_exclude_worktrees,
    },
    NativeMigration {
        id: "3.1.1_event_log_merge_driver",
        detect: detect_event_log_merge_driver,
        can_apply: crate::migrations::merge_drivers::can_apply,
        apply: apply_event_log_merge_driver,
    },
    NativeMigration {
        id: "3.2.0rc28_github_diff_attributes",
        detect: detect_github_diff_attributes,
        can_apply: always_applicable,
        apply: apply_github_diff_attributes,
    },
    NativeMigration {
        id: "3.2.6_decisions_event_log_merge_driver",
        detect: detect_decisions_event_log_merge_driver,
        can_apply: crate::migrations::merge_drivers::can_apply,
        apply: apply_decisions_event_log_merge_driver,
    },
    NativeMigration {
        id: "3.2.6_gate_artifact_merge_drivers",
        detect: detect_gate_artifact_merge_drivers,
        can_apply: crate::migrations::merge_drivers::can_apply,
        apply: apply_gate_artifact_merge_drivers,
    },
    NativeMigration {
        id: "3.2.6_issue_matrix_driver_repoint",
        detect: detect_issue_matrix_driver_repoint,
        can_apply: crate::migrations::merge_drivers::can_apply,
        apply: apply_issue_matrix_driver_repoint,
    },
    NativeMigration {
        id: "3.2.6_meta_traces_merge_drivers",
        detect: detect_meta_traces_merge_drivers,
        can_apply: crate::migrations::merge_drivers::can_apply,
        apply: apply_meta_traces_merge_drivers,
    },
    NativeMigration {
        id: "3.2.7_review_cycle_merge_driver",
        detect: detect_review_cycle_merge_driver,
        can_apply: crate::migrations::merge_drivers::can_apply,
        apply: apply_review_cycle_merge_driver,
    },
    NativeMigration {
        id: "4.0.0rc5_decision_index_merge_driver",
        detect: detect_decision_index_merge_driver,
        can_apply: crate::migrations::merge_drivers::can_apply,
        apply: apply_decision_index_merge_driver,
    },
    NativeMigration {
        id: "4_0_0rc5_heal_run_index_paths",
        detect: crate::migrations::run_index::detect,
        can_apply: crate::migrations::run_index::can_apply,
        apply: crate::migrations::run_index::apply,
    },
    NativeMigration {
        id: "4_0_0rc5_install_lane_tip_recorder",
        detect: crate::migrations::lane_tip::detect,
        can_apply: crate::migrations::lane_tip::can_apply,
        apply: crate::migrations::lane_tip::apply,
    },
    NativeMigration {
        id: "4.0.0rc7_exclude_spec_kitty",
        detect: detect_exclude_spec_kitty,
        can_apply: always_applicable,
        apply: apply_exclude_spec_kitty,
    },
    NativeMigration {
        id: "4.0.0rc7_pack_level_backfill",
        detect: crate::migrations::pack_level::detect,
        can_apply: crate::migrations::pack_level::can_apply,
        apply: crate::migrations::pack_level::apply,
    },
    NativeMigration {
        id: "4.0.0rc7_track_charter_pack_yaml",
        detect: crate::migrations::charter_pack_gitignore::detect,
        can_apply: crate::migrations::charter_pack_gitignore::can_apply,
        apply: crate::migrations::charter_pack_gitignore::apply,
    },
    NativeMigration {
        id: "4.0.0rc8_narrow_agent_gitignore",
        detect: crate::migrations::narrow_agent_gitignore::detect,
        can_apply: crate::migrations::narrow_agent_gitignore::can_apply,
        apply: crate::migrations::narrow_agent_gitignore::apply,
    },
    NativeMigration {
        id: "4.0.0rc8_retire_stale_mission_template_forks",
        detect: crate::migrations::mission_template_forks::detect,
        can_apply: crate::migrations::mission_template_forks::can_apply,
        apply: crate::migrations::mission_template_forks::apply,
    },
    NativeMigration {
        id: "0.4.8_gitignore_agents",
        detect: crate::migrations::gitignore_family::detect_legacy_agent_directories,
        can_apply: crate::migrations::gitignore_family::can_apply_legacy_agent_directories,
        apply: crate::migrations::gitignore_family::apply_legacy_agent_directories,
    },
    NativeMigration {
        id: "0.12.1_remove_kitty_specs_from_gitignore",
        detect: crate::migrations::gitignore_family::detect_kitty_specs_blocking_rule,
        can_apply: crate::migrations::gitignore_family::can_apply_kitty_specs_blocking_rule,
        apply: crate::migrations::gitignore_family::apply_kitty_specs_blocking_rule,
    },
    NativeMigration {
        id: "2.0.0a5_remove_wp_status_gitignore_rule",
        detect: crate::migrations::gitignore_family::detect_wp_status_rule,
        can_apply: crate::migrations::gitignore_family::can_apply_wp_status_rule,
        apply: crate::migrations::gitignore_family::apply_wp_status_rule,
    },
    NativeMigration {
        id: "2.0.9_state_gitignore",
        detect: crate::migrations::gitignore_family::detect_runtime_state_entries,
        can_apply: crate::migrations::gitignore_family::can_apply_project_exists,
        apply: crate::migrations::gitignore_family::apply_runtime_state_entries,
    },
    NativeMigration {
        id: "3.2.0rc35_kittify_runtime_git_hygiene",
        detect: crate::migrations::gitignore_family::detect_runtime_git_hygiene,
        can_apply: crate::migrations::gitignore_family::can_apply_runtime_git_hygiene,
        apply: crate::migrations::gitignore_family::apply_runtime_git_hygiene,
    },
    NativeMigration {
        id: "3.2.3_encoding_provenance_gitignore_backfill",
        detect: crate::migrations::gitignore_family::detect_encoding_provenance,
        can_apply: crate::migrations::gitignore_family::can_apply_encoding_provenance,
        apply: crate::migrations::gitignore_family::apply_encoding_provenance,
    },
    NativeMigration {
        id: "3.2.4_derived_mission_views_gitignore_backfill",
        detect: crate::migrations::gitignore_family::detect_derived_views_entry,
        can_apply: crate::migrations::gitignore_family::can_apply_project_exists,
        apply: crate::migrations::gitignore_family::apply_derived_views_entry,
    },
    NativeMigration {
        id: "3.2.4_runtime_dirs_gitignore_backfill",
        detect: crate::migrations::gitignore_family::detect_runtime_dirs_entries,
        can_apply: crate::migrations::gitignore_family::can_apply_project_exists,
        apply: crate::migrations::gitignore_family::apply_runtime_dirs_entries,
    },
    NativeMigration {
        id: "3.2.6rc3_lint_report_gitignore_backfill",
        detect: crate::migrations::gitignore_family::detect_lint_report_entry,
        can_apply: crate::migrations::gitignore_family::can_apply_lint_report_entry,
        apply: crate::migrations::gitignore_family::apply_lint_report_entry,
    },
    NativeMigration {
        id: "3.2.6rc3_worktrees_gitignore_backfill",
        detect: crate::migrations::gitignore_family::detect_worktrees_entry,
        can_apply: crate::migrations::gitignore_family::can_apply_worktrees_entry,
        apply: crate::migrations::gitignore_family::apply_worktrees_entry,
    },
    NativeMigration {
        id: "0.9.0_frontmatter_only_lanes",
        detect: detect_retired_frontmatter_only_lanes,
        can_apply: can_apply_retired_frontmatter_only_lanes,
        apply: apply_retired_frontmatter_only_lanes,
    },
    NativeMigration {
        id: "0.9.1_complete_migration",
        detect: detect_retired_complete_migration,
        can_apply: can_apply_retired_complete_migration,
        apply: apply_retired_complete_migration,
    },
    NativeMigration {
        id: "0.10.12_charter_cleanup",
        detect: detect_retired_charter_cleanup,
        can_apply: can_apply_retired_charter_cleanup,
        apply: apply_retired_charter_cleanup,
    },
    NativeMigration {
        id: "2.0.0_charter_directory",
        detect: detect_retired_charter_directory,
        can_apply: can_apply_retired_charter_directory,
        apply: apply_retired_charter_directory,
    },
    NativeMigration {
        id: "2.0.2_charter_context_bootstrap",
        detect: detect_retired_charter_context_bootstrap,
        can_apply: can_apply_retired_charter_context_bootstrap,
        apply: apply_retired_charter_context_bootstrap,
    },
    NativeMigration {
        id: "2.0.6_consistency_sweep",
        detect: detect_retired_consistency_sweep,
        can_apply: can_apply_retired_consistency_sweep,
        apply: apply_retired_consistency_sweep,
    },
    NativeMigration {
        id: "2.1.2_fix_charter_doctrine_skill",
        detect: detect_retired_charter_doctrine_skill,
        can_apply: can_apply_retired_charter_doctrine_skill,
        apply: apply_retired_charter_doctrine_skill,
    },
    NativeMigration {
        id: "2.1.2_fix_glossary_context_skill",
        detect: detect_retired_glossary_context_skill,
        can_apply: can_apply_retired_glossary_context_skill,
        apply: apply_retired_glossary_context_skill,
    },
    NativeMigration {
        id: "2.1.4_enforce_command_file_state",
        detect: detect_retired_enforce_command_file_state,
        can_apply: can_apply_retired_enforce_command_file_state,
        apply: apply_retired_enforce_command_file_state,
    },
    NativeMigration {
        id: "3.0.2_restore_prompt_commands",
        detect: detect_retired_restore_prompt_commands,
        can_apply: can_apply_retired_restore_prompt_commands,
        apply: apply_retired_restore_prompt_commands,
    },
    NativeMigration {
        id: "3.1.1_normalize_status_json",
        detect: detect_retired_normalize_status_json,
        can_apply: can_apply_retired_normalize_status_json,
        apply: apply_retired_normalize_status_json,
    },
    NativeMigration {
        id: "3.2.0rc35_default_charter_pack",
        detect: detect_retired_default_charter_pack,
        can_apply: can_apply_retired_default_charter_pack,
        apply: apply_retired_default_charter_pack,
    },
    NativeMigration {
        id: "3.2.5_agents_skills_gitignore_backfill",
        detect: detect_retired_agents_skills_backfill,
        can_apply: can_apply_retired_agents_skills_backfill,
        apply: apply_retired_agents_skills_backfill,
    },
    NativeMigration {
        id: "3.2.6rc3_narrow_cursor_gitignore",
        detect: detect_retired_narrow_cursor_gitignore,
        can_apply: can_apply_retired_narrow_cursor_gitignore,
        apply: apply_retired_narrow_cursor_gitignore,
    },
    NativeMigration {
        id: "normalize_activation_absence",
        detect: detect_retired_normalize_activation_absence,
        can_apply: can_apply_retired_normalize_activation_absence,
        apply: apply_retired_normalize_activation_absence,
    },
    NativeMigration {
        id: "unify_charter_activation_promote_answers",
        detect: detect_retired_unify_promote_answers,
        can_apply: can_apply_retired_unify_promote_answers,
        apply: apply_retired_unify_promote_answers,
    },
    NativeMigration {
        id: "0.2.0_specify_to_kittify",
        detect: crate::migrations::legacy_layout::detect_specify_layout,
        can_apply: crate::migrations::legacy_layout::can_apply_specify_layout,
        apply: crate::migrations::legacy_layout::apply_specify_layout,
    },
    NativeMigration {
        id: "0.8.0_remove_active_mission",
        detect: crate::migrations::legacy_layout::detect_active_mission,
        can_apply: always_applicable,
        apply: crate::migrations::legacy_layout::apply_active_mission,
    },
    NativeMigration {
        id: "0.10.9_repair_templates",
        detect: detect_retired_repair_templates,
        can_apply: can_apply_retired_repair_templates,
        apply: apply_retired_repair_templates,
    },
    NativeMigration {
        id: "2.0.0_historical_status_migration",
        detect: detect_retired_historical_status,
        can_apply: can_apply_retired_historical_status,
        apply: apply_retired_historical_status,
    },
    crate::migrations::completion_modern::charter_synthesis::IMPLEMENTATIONS[0],
    crate::migrations::completion_modern::charter_synthesis::IMPLEMENTATIONS[1],
    crate::migrations::completion_modern::charter_synthesis::IMPLEMENTATIONS[2],
    crate::migrations::completion_modern::charter_synthesis::IMPLEMENTATIONS[3],
    crate::migrations::completion_modern::charter_synthesis::IMPLEMENTATIONS[4],
    crate::migrations::completion_modern::charter_synthesis::IMPLEMENTATIONS[5],
];

/// Every registered native implementation, for lookup and coverage reporting.
///
/// Registration order is not execution order: selection and execution continue
/// to follow the canonical migration catalogue and applied ledger.
pub fn native_migrations() -> impl Iterator<Item = &'static NativeMigration> {
    NATIVE_MIGRATIONS
        .iter()
        .chain(crate::migrations::completion_legacy::IMPLEMENTATIONS.iter())
        .chain(crate::migrations::completion_modern::IMPLEMENTATIONS.iter())
}

fn native_for(id: &str) -> Option<&'static NativeMigration> {
    native_migrations().find(|migration| migration.id == id)
}

/// One migration's read-only dry-run door: Source's `apply(..., dry_run=True)`.
///
/// A door is never a second planner. Source passes `dry_run` into ONE `apply()` body,
/// so the preview and the real run share their parser, model, policy and change-line
/// composition; the preview only skips the writes. A would-do list derived from the
/// `detect()` verdict instead would be a fabrication (it could not say which entry
/// changes, or to what).
pub type PreviewFn = fn(&Path) -> Result<MigrationOutcome, InstallError>;

/// The dry-run door for a migration id, when this build has one.
///
/// The six charter-synthesis/cutover/provenance effects all have one: `promote` and
/// `cutover` shipped theirs with their engines, and the four the runner previously
/// reported without a write list (both provenance heals, the bundle fold, the
/// project-intent rename) are closed here through the same composition their `apply`
/// uses. Every other implemented migration has no door yet; the runner then reports
/// its verdict alone rather than pretending to preview it.
pub fn preview_for(id: &str) -> Option<PreviewFn> {
    use crate::migrations::completion_modern::charter_synthesis as synthesis;
    match id {
        "3.2.7_heal_provenance_paths" => Some(synthesis::provenance::preview),
        "4_0_0rc5_heal_template_set_provenance" => {
            Some(synthesis::provenance::preview_template_set)
        }
        "4.0.0rc8_project_intent_directive_rename" => Some(synthesis::legacy_rename::preview),
        "4.0.0rc8_promote_interview_selections" => Some(synthesis::promote::apply_dry_run),
        "consolidate_charter_bundle_fold" => Some(synthesis::fold::preview),
        "charter_pack_cutover" => Some(synthesis::cutover::dry_run),
        _ => None,
    }
}

/// Whether `error` is one of the two typed Source classes whose runner phases treat the
/// failure as a returned `MigrationResult` rather than a caught exception.
///
/// `GitignorePathError` (native [`InstallError::IgnoreFileUnsafe`]) and
/// `MigrationStateUnreadableError` (native
/// [`InstallError::MigrationStateUnreadable`]) are the ONLY classes `runner.py` catches
/// in both the `detect()` and the `apply()` phases of the main loop, and the `apply()`
/// catch of the worktree loop. Classification is by type, never by message text.
fn is_source_state_failure(error: &InstallError) -> bool {
    matches!(
        error,
        InstallError::IgnoreFileUnsafe { .. } | InstallError::MigrationStateUnreadable { .. }
    )
}

/// Run the native `detect` guard for `id`, or `None` when this build has no
/// implementation (so a caller can never read "not implemented" as "nothing to do").
pub fn native_migration_detect(
    id: &str,
    project_root: &Path,
) -> Option<Result<bool, InstallError>> {
    native_for(id).map(|migration| (migration.detect)(project_root))
}

/// The selected migrations for a target: **declared minus applied**, capped by
/// `target_version` (ADR 2026-10-17-1 §3).
///
/// This is the one selection authority shared by `plan_upgrade` and the
/// `migrate ledger` report: it reads only the catalog and the project's ledger,
/// so a project whose `metadata.yaml` is missing or corrupt can still be
/// reported on. An unparseable target caps nothing (upstream behavior).
pub fn select_candidates(
    project_root: &Path,
    target_version: &str,
) -> Result<Vec<MigrationSpec>, InstallError> {
    let catalog = MigrationCatalog::embedded()?;
    let ledger = MigrationLedger::new(project_root);
    let target = Version::parse(target_version).ok();
    // While the adoption backfill is unsettled, the ids it will settle count as
    // settled here: a preview and the real run then select the same set, and a
    // legacy project's already-settled history is never replayed.
    let adopting = match catalog.get(crate::migrations::ledger_adoption::BACKFILL_ID) {
        Some(spec) if !ledger.is_settled(spec)? => {
            crate::migrations::ledger_adoption::adoption_settled_ids(project_root)?
        }
        _ => BTreeSet::new(),
    };
    let mut selected = Vec::new();
    for spec in catalog.specs() {
        if let Some(target) = &target
            && let Ok(spec_version) = Version::parse(&spec.target_version)
            && spec_version > *target
        {
            continue;
        }
        if ledger.is_settled(spec)? {
            continue;
        }
        if adopting.contains(&spec.id) && spec.id != crate::migrations::ledger_adoption::BACKFILL_ID
        {
            continue;
        }
        selected.push(spec.clone());
    }
    Ok(selected)
}

/// What the plan will do with one migration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlannedOutcome {
    /// A native implementation will run it.
    Apply,
    /// `detect()` says the project is already in the target state.
    NotNeeded,
    /// A declared git requirement cannot be met here (not recorded).
    SkipOutsideGit,
    /// The migration's own `can_apply` refuses this project shape (not recorded).
    NotApplicable,
    /// `detect`/`can_apply` could not evaluate the project (recorded `failed`).
    Failed,
    /// No native implementation exists; the real run refuses.
    Unimplemented,
}

impl PlannedOutcome {
    /// Stable spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Apply => "apply",
            Self::NotNeeded => "not_needed",
            Self::SkipOutsideGit => "skipped_outside_git",
            Self::NotApplicable => "not_applicable",
            Self::Failed => "failed",
            Self::Unimplemented => "unimplemented",
        }
    }
}

/// One row of an upgrade plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationPlanRow {
    /// The migration spec.
    pub spec: MigrationSpec,
    /// The planned outcome.
    pub outcome: PlannedOutcome,
    /// A detail for `not_needed`, `skipped_outside_git` or `unimplemented`.
    pub detail: Option<String>,
}

/// A complete, read-only upgrade plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpgradePlan {
    /// The recorded project version.
    pub from_version: String,
    /// The version this run targets.
    pub to_version: String,
    /// Every selected migration in catalog order.
    pub rows: Vec<MigrationPlanRow>,
}

impl UpgradePlan {
    /// Ids with a native implementation that will run.
    pub fn apply_ids(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter(|row| row.outcome == PlannedOutcome::Apply)
            .map(|row| row.spec.id.clone())
            .collect()
    }

    /// Ids that would be recorded `skipped` (not needed).
    pub fn skip_ids(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter(|row| row.outcome == PlannedOutcome::NotNeeded)
            .map(|row| row.spec.id.clone())
            .collect()
    }

    /// Ids this run would not apply and would not record.
    pub fn not_applicable_ids(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter(|row| {
                matches!(
                    row.outcome,
                    PlannedOutcome::SkipOutsideGit | PlannedOutcome::NotApplicable
                )
            })
            .map(|row| row.spec.id.clone())
            .collect()
    }

    /// Ids with no native implementation.
    pub fn unimplemented_ids(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter(|row| row.outcome == PlannedOutcome::Unimplemented)
            .map(|row| row.spec.id.clone())
            .collect()
    }

    /// Whether any selected migration has no native implementation.
    pub fn is_blocked(&self) -> bool {
        !self.unimplemented_ids().is_empty()
    }

    /// Every selected specification, in catalogue order.
    ///
    /// This is the carrier the worktree pass inherits: selection is decided once,
    /// for the main root, and never re-derived per sibling.
    pub fn specs(&self) -> Vec<MigrationSpec> {
        self.rows.iter().map(|row| row.spec.clone()).collect()
    }
}

/// An upgrade request.
#[derive(Clone, Debug)]
pub struct UpgradeRequest {
    /// The project root (must contain `.kittify/config.yaml`).
    pub project_root: PathBuf,
    /// The running CLI version.
    pub cli_version: String,
    /// Optional `--target`; defaults to the CLI version.
    pub target_version: Option<String>,
    /// Preview only.
    pub dry_run: bool,
    /// Bounded wait for the project lock.
    pub lock_wait: Duration,
    /// Optional execution-only subset of the canonical selection.
    ///
    /// Excluded migrations are neither evaluated nor recorded. Callers derive
    /// worktree scopes from the catalogue's `runs_on_worktrees` metadata; this
    /// never changes target, ledger-partition or declared-minus-applied policy.
    pub execution_scope: Option<BTreeSet<String>>,
    /// Shared main/fallback stamp for a version-only worktree bump (#4972).
    ///
    /// Used only when no new ledger record was written and the version changes.
    /// Omit for freshly synthesized metadata; genuine changes keep a fresh clock.
    pub bookkeeping_stamp: Option<String>,
}

impl UpgradeRequest {
    /// Build a request with production defaults.
    pub fn new(project_root: impl Into<PathBuf>, cli_version: impl Into<String>) -> Self {
        Self {
            project_root: project_root.into(),
            cli_version: cli_version.into(),
            target_version: None,
            dry_run: false,
            lock_wait: Duration::from_secs(10),
            execution_scope: None,
            bookkeeping_stamp: None,
        }
    }

    /// Set an explicit `--target`.
    pub fn with_target(mut self, target: impl Into<String>) -> Self {
        self.target_version = Some(target.into());
        self
    }

    /// Execute only these ids from the canonical selection, in catalogue order.
    ///
    /// An empty set executes no callbacks and appends no migration records.
    /// Normal successful-run metadata bookkeeping still applies.
    pub fn with_scope(mut self, ids: BTreeSet<String>) -> Self {
        self.execution_scope = Some(ids);
        self
    }

    /// Align a bookkeeping-only version bump to an observed main/shared stamp.
    ///
    /// The fan-out caller obtains a main stamp or mints one real fallback once
    /// before visiting sibling roots. New records (including `skipped`) are
    /// material changes, not bookkeeping-only bumps.
    pub fn with_bookkeeping_stamp(mut self, stamp: impl Into<String>) -> Self {
        self.bookkeeping_stamp = Some(stamp.into());
        self
    }

    /// Preview without writing.
    pub fn dry_run(mut self) -> Self {
        self.dry_run = true;
        self
    }

    /// The resolved target version.
    pub fn resolved_target(&self) -> String {
        self.target_version
            .clone()
            .unwrap_or_else(|| self.cli_version.clone())
    }
}

/// One executed row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationResultRow {
    /// The migration id.
    pub id: String,
    /// `applied`, `skipped` or `failed`.
    pub result: String,
    /// Changes made.
    pub changes: Vec<String>,
    /// Warnings.
    pub warnings: Vec<String>,
}

/// The result of an upgrade run.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UpgradeReport {
    /// Whether the run completed (a dry run is always "successful").
    pub success: bool,
    /// The recorded project version.
    pub from_version: String,
    /// The target version.
    pub to_version: String,
    /// Executed rows.
    pub rows: Vec<MigrationResultRow>,
    /// Whether this was a dry run.
    pub dry_run: bool,
    /// Whether `metadata.yaml` was updated.
    pub metadata_written: bool,
    /// Full Source results, including failed and no-op payloads.
    pub migration_results: BTreeMap<String, MigrationOutcome>,
    /// Migration errors, not silently projected onto warnings.
    pub errors: Vec<String>,
    /// Aggregate migration warnings.
    pub warnings: Vec<String>,
    /// A caught detect/apply exception (distinct from a failed result).
    pub errored: bool,
    /// An apply result reported `success = false`.
    pub failed: bool,
    /// The migrations this run selected, in catalogue order.
    ///
    /// Captured **before** settlement, so the worktree pass inherits exactly the
    /// main run's selection (Source `_upgrade` passes its `migrations` list to
    /// `_upgrade_worktrees`), including every id selected after a main returned
    /// failure. A sibling root must never re-select for itself.
    pub selected: Vec<MigrationSpec>,
}

impl UpgradeReport {
    /// Applied ids.
    pub fn applied(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter(|row| row.result == "applied")
            .map(|row| row.id.clone())
            .collect()
    }

    /// Skipped ids.
    pub fn skipped(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter(|row| row.result == "skipped")
            .map(|row| row.id.clone())
            .collect()
    }
}

/// Build a read-only upgrade plan.
pub fn plan_upgrade(request: &UpgradeRequest) -> Result<UpgradePlan, InstallError> {
    let root = &request.project_root;
    ensure_initialized(root)?;
    let metadata = ProjectMetadata::load(&root.join(".kittify")).ok_or_else(|| {
        InstallError::UpgradeRefused {
            detail: "metadata.yaml is missing or unreadable; cannot determine the project version"
                .to_string(),
        }
    })?;
    if let Some(schema) = metadata.schema_version
        && schema > CURRENT_SCHEMA_VERSION
    {
        return Err(InstallError::SchemaVersionNewer {
            found: schema,
            supported: CURRENT_SCHEMA_VERSION,
        });
    }
    let from = Version::parse(&metadata.version).map_err(|error| InstallError::VersionInvalid {
        value: metadata.version.clone(),
        detail: error.to_string(),
    })?;
    let cli =
        Version::parse(&request.cli_version).map_err(|error| InstallError::VersionInvalid {
            value: request.cli_version.clone(),
            detail: error.to_string(),
        })?;
    if from > cli {
        return Err(InstallError::ProjectVersionNewer {
            project: metadata.version.clone(),
            cli: request.cli_version.clone(),
        });
    }
    let target_text = request.resolved_target();
    let target = Version::parse(&target_text).map_err(|error| InstallError::VersionInvalid {
        value: target_text.clone(),
        detail: error.to_string(),
    })?;
    if target < from {
        return Err(InstallError::TargetVersionOlder {
            target: target_text,
            project: metadata.version.clone(),
        });
    }
    let git = is_git_repository(root);
    let mut rows = Vec::new();
    // The preview's sequenced view: the ids this forecast predicts it will apply, which is
    // what makes `sequenced_predecessors` meaningful for a later row (#6552). A forecast
    // cannot observe real writes, so this is a prediction only; the real run evaluates
    // every guard again, in order, after the previous migration's actual effects.
    let mut predicted_applied: BTreeSet<String> = BTreeSet::new();
    for spec in select_candidates(root, &target_text)? {
        if request
            .execution_scope
            .as_ref()
            .is_some_and(|scope| !scope.contains(&spec.id))
        {
            continue;
        }
        let (outcome, detail) = match native_for(&spec.id) {
            None => (
                PlannedOutcome::Unimplemented,
                Some("no native implementation in this build".to_string()),
            ),
            Some(native) => {
                if spec.requires_git_repository && !git {
                    (
                        PlannedOutcome::SkipOutsideGit,
                        Some("requires a git repository".to_string()),
                    )
                } else {
                    match (native.detect)(root) {
                        // Upstream records a `failed` result (and leaves the
                        // version untouched) when a guard cannot evaluate the
                        // project at all; it is never a silent skip.
                        Err(error) => (PlannedOutcome::Failed, Some(error.to_string())),
                        Ok(true) => match (native.can_apply)(root) {
                            Err(error) => (PlannedOutcome::Failed, Some(error.to_string())),
                            Ok(Some(reason)) => (PlannedOutcome::NotApplicable, Some(reason)),
                            Ok(None) => (PlannedOutcome::Apply, None),
                        },
                        Ok(false) => {
                            // A declared predecessor this forecast predicts it applies
                            // first writes the state this `detect()` reads, so a real run
                            // would apply this migration next; report that prediction and
                            // say it is one. Nothing is written: a prediction is a verdict.
                            let pending = sequenced_predecessors(&spec.id, &predicted_applied);
                            if pending.is_empty() {
                                (
                                    PlannedOutcome::NotNeeded,
                                    Some("project is already in the target state".to_string()),
                                )
                            } else {
                                (
                                    PlannedOutcome::Apply,
                                    Some(format!(
                                        "dry-run: would apply after {}",
                                        pending.join(", ")
                                    )),
                                )
                            }
                        }
                    }
                }
            }
        };
        if outcome == PlannedOutcome::Apply {
            predicted_applied.insert(spec.id.clone());
        }
        rows.push(MigrationPlanRow {
            spec: spec.clone(),
            outcome,
            detail,
        });
    }
    Ok(UpgradePlan {
        from_version: metadata.version,
        to_version: target_text,
        rows,
    })
}

/// Execute (or preview) an upgrade.
pub fn run_upgrade(request: &UpgradeRequest) -> Result<UpgradeReport, InstallError> {
    let plan = plan_upgrade(request)?;
    if request.dry_run {
        return dry_run_previews(request, &plan);
    }
    let unimplemented = plan.unimplemented_ids();
    if !unimplemented.is_empty() {
        return Err(InstallError::MigrationNotImplemented {
            count: unimplemented.len(),
            ids: unimplemented.join(", "),
        });
    }
    let root = &request.project_root;
    let kittify = root.join(".kittify");
    // Same canonical authority as init (machine state directory, keyed by the resolved
    // project root). A project-relative `.kittify/.install.lock` here would split lock
    // authority: upgrade and init would write the same managed surfaces without excluding
    // each other.
    let _lock = ProjectLock::for_project(root, request.lock_wait)?;
    // Re-plan under the lock: another process may have advanced the ledger, and the
    // selection this run executes must be the one the lock protects.
    let plan = plan_upgrade(request)?;
    let unimplemented = plan.unimplemented_ids();
    if !unimplemented.is_empty() {
        return Err(InstallError::MigrationNotImplemented {
            count: unimplemented.len(),
            ids: unimplemented.join(", "),
        });
    }
    let ledger = MigrationLedger::new(root);
    // FR-002/#3334: capture the on-disk version stamp BEFORE the loop. A failed run must
    // never advance — or erase — the schema the project actually satisfies, so the
    // captured stamp is restored instead of re-stamping the target onto a half-migrated
    // project.
    let pre_run_stamp = VersionStamp::capture(&kittify);
    let mut report = UpgradeReport {
        success: true,
        from_version: plan.from_version.clone(),
        to_version: plan.to_version.clone(),
        // The inherited worktree scope: this run's whole selection, in catalogue
        // order, captured before any step settles (Source `_upgrade`).
        selected: plan.specs(),
        ..UpgradeReport::default()
    };
    let mut ledger_changed = false;
    // The run's sequenced view: ids THIS run has applied. It grows in exactly one place,
    // so a dry run's later verdicts can consult what the run has reached (#6552).
    let mut sequenced_applied: BTreeSet<String> = BTreeSet::new();
    for row in &plan.rows {
        let native = native_for(&row.spec.id).expect("plan only applies implemented migrations");
        // Selection is the ledger diff; the guards are evaluated HERE, in catalogue order,
        // after every earlier migration of this run has written its effects.
        let step = evaluate_step(
            root,
            &row.spec,
            native,
            &ledger,
            &request.cli_version,
            false,
            &sequenced_applied,
        )?;
        ledger_changed |= step.recorded;
        report
            .warnings
            .extend(step.outcome.warnings.iter().cloned());
        let status = match step.status {
            StepStatus::Applied => "applied",
            StepStatus::Skipped => "skipped",
            StepStatus::Failed => "failed",
        };
        report.rows.push(MigrationResultRow {
            id: row.spec.id.clone(),
            result: status.to_string(),
            changes: step.outcome.changes.clone(),
            warnings: step.outcome.warnings.clone(),
        });
        // Record applied and skipped payloads alike, so an operator sees no-op and
        // refresh reports as well as the failed one.
        let is_failed = step.status == StepStatus::Failed;
        if step.status == StepStatus::Applied {
            sequenced_applied.insert(row.spec.id.clone());
        }
        if is_failed {
            report.errors.extend(step.outcome.errors.iter().cloned());
        }
        report
            .migration_results
            .insert(row.spec.id.clone(), step.outcome);
        if is_failed {
            // A returned `MigrationOutcome { success: false }` is a complete Source result,
            // not an uncaught exception: stop the main loop, keep the report, restore the
            // pre-run stamp, and let the caller continue with the worktree pass.
            report.success = false;
            report.failed = true;
            pre_run_stamp.restore(&kittify)?;
            return Ok(report);
        }
    }
    if plan.rows.is_empty() {
        report.warnings.push(format!(
            "No migrations needed from {} to {}",
            plan.from_version, plan.to_version
        ));
    }
    let mut metadata =
        ProjectMetadata::load(&kittify).ok_or_else(|| InstallError::UpgradeRefused {
            detail: "metadata.yaml disappeared during the upgrade".to_string(),
        })?;
    let bookkeeping_only_bump = !ledger_changed && metadata.version != plan.to_version;
    metadata.version = plan.to_version.clone();
    metadata.last_upgraded_at = Some(
        request
            .bookkeeping_stamp
            .as_ref()
            .filter(|_| bookkeeping_only_bump)
            .cloned()
            .unwrap_or_else(now_utc_iso),
    );
    let mut metadata_written = metadata.save(&kittify)?;
    // Source stamps the required schema AFTER `save()`: the loaded model may carry no
    // `schema_version` (a legacy file) and the stamp is the one step that settles the
    // value plus the canonical capability map, so the gate passes after an upgrade
    // (#705, #5229). A no-op stamp rewrites nothing.
    metadata_written |= stamp_schema_version(&kittify, CURRENT_SCHEMA_VERSION)?;
    report.metadata_written = metadata_written;
    Ok(report)
}

/// Execute the read-only preview doors for a plan (Source `upgrade(dry_run=True)`).
///
/// Source runs the SAME loop in both modes and only passes `dry_run` into `apply()`, so
/// the verdict, the change lines, the sequenced view and the stop-on-failure rule are
/// identical; only the writes are skipped. That is what this does: every row the plan
/// says will apply is handed to [`evaluate_step`] with `dry_run = true`, which reaches
/// the migration's own preview door. Nothing is recorded, stamped or written.
///
/// The row vocabulary stays the planner's (`would_apply` / `not_needed` / ...) so the
/// CLI's preview contract is unchanged; what the row gains is the real change list.
fn dry_run_previews(
    request: &UpgradeRequest,
    plan: &UpgradePlan,
) -> Result<UpgradeReport, InstallError> {
    let root = &request.project_root;
    let ledger = MigrationLedger::new(root);
    let mut report = UpgradeReport {
        success: true,
        from_version: plan.from_version.clone(),
        to_version: plan.to_version.clone(),
        dry_run: true,
        metadata_written: false,
        selected: plan.specs(),
        ..UpgradeReport::default()
    };
    // The preview's sequenced view grows exactly where the real run's does, so a later
    // prediction still names the predecessor this preview reached (#6552).
    let mut sequenced_applied: BTreeSet<String> = BTreeSet::new();
    for row in &plan.rows {
        let mut result_row = dry_run_row(row);
        let Some(native) = native_for(&row.spec.id) else {
            // The plan already reports the missing implementation; a real run refuses.
            report.rows.push(result_row);
            continue;
        };
        if row.outcome != PlannedOutcome::Apply {
            report.rows.push(result_row);
            continue;
        }
        let step = evaluate_step(
            root,
            &row.spec,
            native,
            &ledger,
            &request.cli_version,
            true,
            &sequenced_applied,
        )?;
        sequenced_applied.insert(row.spec.id.clone());
        report
            .warnings
            .extend(step.outcome.warnings.iter().cloned());
        if !step.outcome.changes.is_empty() {
            result_row.changes = step.outcome.changes.clone();
            result_row.warnings = step.outcome.warnings.clone();
        }
        let failed = step.status == StepStatus::Failed;
        if failed {
            // Source stops the dry-run loop on a failed result and keeps the payload.
            result_row.result = "failed".to_string();
            report.errors.extend(step.outcome.errors.iter().cloned());
            report.success = false;
            report.failed = true;
        }
        report
            .migration_results
            .insert(row.spec.id.clone(), step.outcome);
        report.rows.push(result_row);
        if failed {
            break;
        }
    }
    Ok(report)
}

/// The status of one sequential step (Source `_apply_migration`'s `(result, status)`).
///
/// Shared by the main loop and the worktree mode, so one vocabulary describes both.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StepStatus {
    /// The migration ran (or, in a dry run, is predicted to run).
    Applied,
    /// The migration was not needed, not applicable, or already applied.
    Skipped,
    /// `apply()` returned `success = false`: a complete, recorded Source result.
    Failed,
}

/// One row of the dry-run report, mapped from a read-only forecast row.
///
/// A dry run wrote nothing before this point, so a `not_needed` row may only claim the tree
/// in front of it, never the target state it has not simulated (#6552).
fn dry_run_row(row: &MigrationPlanRow) -> MigrationResultRow {
    MigrationResultRow {
        id: row.spec.id.clone(),
        result: match row.outcome {
            PlannedOutcome::Apply => "would_apply",
            PlannedOutcome::NotNeeded => "not_needed",
            PlannedOutcome::SkipOutsideGit => "would_skip_outside_git",
            PlannedOutcome::NotApplicable => "not_applicable",
            PlannedOutcome::Failed => "failed",
            PlannedOutcome::Unimplemented => "unimplemented",
        }
        .to_string(),
        changes: match row.outcome {
            PlannedOutcome::NotNeeded => {
                vec![format!(
                    "Migration {} not needed on the current tree",
                    row.spec.id
                )]
            }
            _ => row.detail.iter().cloned().collect(),
        },
        warnings: Vec::new(),
    }
}

/// One evaluated-and-executed migration step.
/// One evaluated-and-executed migration step.
///
/// Public because it is the main loop's per-migration seam: the dry-run door dispatch
/// and the typed failure phases it implements are exactly what a caller (and a test)
/// must be able to exercise without registering a callback first.
pub struct StepResult {
    /// The Source status this step reports.
    pub status: StepStatus,
    /// The complete `MigrationResult` payload, including real partial writes and errors.
    pub outcome: MigrationOutcome,
    /// Whether this step appended a NEW ledger record (Source's metadata dirty signal).
    pub recorded: bool,
}

fn step_skipped(message: String) -> StepResult {
    let mut outcome = MigrationOutcome::skipped();
    outcome.changes = vec![message];
    StepResult {
        status: StepStatus::Skipped,
        outcome,
        recorded: false,
    }
}

/// The skip warning for a migration `detect()` reports nothing to do for.
///
/// A real run does reach the target state, so it keeps that claim. A dry run applied
/// nothing before this point, so it can only claim the tree in front of it; it must not
/// assert a target state it has not simulated (#6552).
fn not_needed_message(id: &str, dry_run: bool) -> String {
    if dry_run {
        format!("Migration {id} not needed on the current tree")
    } else {
        format!("Migration {id} not needed (project already in target state)")
    }
}

/// Evaluate one migration's guards and run it — in order, against the CURRENT tree.
///
/// Two authorities meet here. The LEDGER decides whether this migration is selected at
/// all (`is_settled`); `detect()` is the pre-apply idempotency guard, and it is read NOW,
/// after every earlier migration of this run has written its effects. Reusing a plan's
/// pre-run verdicts for execution would judge a migration whose `detect()` reads an
/// earlier migration's output against the tree that existed before the run.
///
/// A `detect`/`can_apply` hook that cannot evaluate the project is a recorded `failed`
/// (the migration stays retryable). The two typed Source state-failure classes
/// (`IgnoreFileUnsafe` = `GitignorePathError`, `MigrationStateUnreadable` =
/// `MigrationStateUnreadableError`) are RETURNED as a failed result with Source's
/// `Cannot safely detect {id}: ...` / `Cannot apply {id}: ...` payload — the main loop
/// stops with the report, exactly as `runner.py` does — while any other class keeps the
/// typed-abort contract. `apply()` errors of a returned-failure class append NO ledger
/// record (Source's `apply()` catch returns before the record step); a returned
/// `success = false` outcome is not an error at all — it becomes a `Failed` step whose
/// complete payload the caller reports.
///
/// Under `dry_run` the step reaches the migration's own preview door
/// ([`preview_for`]) and writes nothing.
pub fn evaluate_step(
    root: &Path,
    spec: &MigrationSpec,
    native: &NativeMigration,
    ledger: &MigrationLedger,
    cli_version: &str,
    dry_run: bool,
    sequenced_applied: &BTreeSet<String>,
) -> Result<StepResult, InstallError> {
    if ledger.is_settled(spec)? {
        return Ok(step_skipped(format!(
            "Migration {} already applied, skipping",
            spec.id
        )));
    }
    // A migration that needs a git repository is not applicable outside one. Decide
    // before `detect()` (which may itself resolve the repo root) and record nothing, so a
    // run inside a repository still owns it.
    if spec.requires_git_repository && !is_git_repository(root) {
        return Ok(step_skipped(format!(
            "Migration {} not applicable here (not inside a git repository); skipping",
            spec.id
        )));
    }
    let needed = match (native.detect)(root) {
        Ok(needed) => needed,
        Err(error) => {
            // Source `_detect_failure`: a `GitignorePathError` / state-unreadable guard
            // is a RECORDED failure returned as a complete `MigrationResult` (the run
            // stops with the report and the stamp restored), never a caught exception
            // that drops the run. Every other class keeps the typed-abort contract.
            if is_source_state_failure(&error) {
                let recorded = if dry_run {
                    false
                } else {
                    record_scoped_result(ledger, spec, FAILED_RESULT, cli_version)?
                };
                return Ok(StepResult {
                    status: StepStatus::Failed,
                    outcome: MigrationOutcome {
                        success: false,
                        errors: vec![format!("Cannot safely detect {}: {error}", spec.id)],
                        ..MigrationOutcome::default()
                    },
                    recorded,
                });
            }
            record_failure(ledger, spec, cli_version, dry_run)?;
            return Err(error);
        }
    };
    // The sequenced view a dry run cannot observe for itself (#6552): a declared
    // predecessor this run has reached has written the state this `detect()` just read, so
    // its false is a pre-run answer, not the run's verdict.
    let pending = if dry_run && !needed {
        sequenced_predecessors(&spec.id, sequenced_applied)
    } else {
        Vec::new()
    };
    if !needed && pending.is_empty() {
        let mut outcome = MigrationOutcome::skipped();
        outcome.changes = vec![not_needed_message(&spec.id, dry_run)];
        let recorded = if dry_run {
            false
        } else {
            record_scoped_result(ledger, spec, SKIPPED_RESULT, cli_version)?
        };
        return Ok(StepResult {
            status: StepStatus::Skipped,
            outcome,
            recorded,
        });
    }
    // A migration that does not fit THIS project is a skip that records nothing, so it is
    // re-evaluated when the project state changes. A genuine inability to complete work is
    // an `apply()` failure, which still fails the run.
    match (native.can_apply)(root) {
        Ok(None) => {}
        Ok(Some(reason)) => {
            return Ok(step_skipped(format!(
                "Migration {} not applicable here ({reason}); skipping",
                spec.id
            )));
        }
        Err(error) => {
            record_failure(ledger, spec, cli_version, dry_run)?;
            return Err(error);
        }
    }
    if !pending.is_empty() {
        let mut outcome = MigrationOutcome::skipped();
        outcome.changes = vec![format!("dry-run: would apply after {}", pending.join(", "))];
        outcome.predicted_after = pending;
        return Ok(StepResult {
            status: StepStatus::Applied,
            outcome,
            recorded: false,
        });
    }
    if dry_run {
        // Source's `migration.apply(project_path, dry_run=True)`: the real door, in
        // preview mode. Nothing is written, and the change lines are the ones the same
        // composition would produce for real.
        let Some(preview) = preview_for(&spec.id) else {
            // No door in this build: report the verdict only. Inventing a would-do list
            // from the detect verdict would claim detail the build cannot prove.
            return Ok(StepResult {
                status: StepStatus::Applied,
                outcome: MigrationOutcome::skipped(),
                recorded: false,
            });
        };
        return Ok(match preview(root) {
            Ok(outcome) => StepResult {
                status: if outcome.success {
                    StepStatus::Applied
                } else {
                    StepStatus::Failed
                },
                outcome,
                recorded: false,
            },
            Err(error) => StepResult {
                status: StepStatus::Failed,
                outcome: MigrationOutcome {
                    success: false,
                    errors: vec![format!("Cannot apply {}: {error}", spec.id)],
                    ..MigrationOutcome::default()
                },
                recorded: false,
            },
        });
    }
    match (native.apply)(root) {
        Ok(outcome) => {
            let result = if outcome.success {
                SETTLED_RESULT
            } else {
                FAILED_RESULT
            };
            let recorded = record_scoped_result(ledger, spec, result, cli_version)?;
            Ok(StepResult {
                status: if outcome.success {
                    StepStatus::Applied
                } else {
                    StepStatus::Failed
                },
                outcome,
                recorded,
            })
        }
        Err(error) => {
            // Source's `apply()` catch for `GitignorePathError` /
            // `MigrationStateUnreadableError` RETURNS `MigrationResult(success=False,
            // errors=["Cannot apply {id}: {exc}"])` BEFORE the record step: the run stops
            // with that payload, and no ledger record is appended (the migration stays
            // retryable). Every other class keeps the recorded, typed-abort contract.
            if is_source_state_failure(&error) {
                return Ok(StepResult {
                    status: StepStatus::Failed,
                    outcome: MigrationOutcome {
                        success: false,
                        errors: vec![format!("Cannot apply {}: {error}", spec.id)],
                        ..MigrationOutcome::default()
                    },
                    recorded: false,
                });
            }
            // The guards were satisfiable but the work could not be completed. Source
            // records that as `failed` and stops the run, so the migration is retried and
            // the version is never advanced past unfinished work.
            record_failure(ledger, spec, cli_version, dry_run)?;
            Err(error)
        }
    }
}

/// Record a `failed` result for a step that could not proceed (`dry_run` records nothing).
fn record_failure(
    ledger: &MigrationLedger,
    spec: &MigrationSpec,
    cli_version: &str,
    dry_run: bool,
) -> Result<(), InstallError> {
    if dry_run {
        return Ok(());
    }
    record_scoped_result(ledger, spec, FAILED_RESULT, cli_version)?;
    Ok(())
}

/// Whether the project is a git repository (a `.git` directory or gitfile at the
/// project root). Worktree/submodule topologies are not treated as a repository
/// here; a migration that needs the common dir refuses instead of guessing.
pub fn is_git_repository(project_root: &Path) -> bool {
    project_root.join(".git").exists()
}

/// Append a ledger record unless the held result already says the same thing.
///
/// Upstream's `_record_ledger_result` skips the append when the authoritative
/// partition already holds this result, and never downgrades a held `success`
/// to `skipped`. Without that guard a re-run of a `skipped` (non-settling)
/// migration — every superseded no-op, for example — would grow the ledger on
/// every invocation. A superseded migration's record carries its successor
/// note so a successor can tell that no-op from the shipped migration's own
/// `skipped`.
/// Record a result through the runner's canonical partition/deduplication rule.
///
/// Identical held results append nothing, and `skipped` never downgrades held
/// `success`. Supersession provenance comes from the catalogue specification.
/// This does not select or execute a migration, nor does it acquire a lock;
/// callers must already hold the same project lock as the upgrade runner.
/// Returns whether a NEW record was appended, the Source metadata dirty signal.
pub fn record_scoped_result(
    ledger: &MigrationLedger,
    spec: &MigrationSpec,
    result: &str,
    cli_version: &str,
) -> Result<bool, InstallError> {
    let held = ledger
        .records_for(spec)?
        .into_iter()
        .find(|record| record.id == spec.id)
        .map(|record| record.result);
    if held.as_deref() == Some(result)
        || (result == SKIPPED_RESULT && held.as_deref() == Some(SETTLED_RESULT))
    {
        return Ok(false);
    }
    let mut record = LedgerRecord::for_spec(spec, result, cli_version);
    record.superseded_by = spec.superseded_by.clone();
    ledger.append(&record)?;
    Ok(true)
}

// ------------------------------------------------- worktree execution context

/// Filter the main run's selection to the migrations a worktree executes.
///
/// Source `_upgrade_worktrees` first line:
/// `worktree_migrations = [migration for migration in migrations if migration.runs_on_worktrees]`.
/// The order is the inherited catalogue order, never a per-root selection.
///
/// A selected id with no catalogue row cannot be classified and is a typed
/// refusal, never a silent "project-only" or "nothing to do".
pub fn worktree_migration_scope(
    selected: &[MigrationSpec],
) -> Result<Vec<MigrationSpec>, InstallError> {
    let catalog = MigrationCatalog::embedded()?;
    let mut scope = Vec::new();
    for spec in selected {
        let declared = catalog.get(&spec.id).ok_or_else(|| {
            InstallError::MigrationCatalogInvalid {
                detail: format!(
                    "selected migration {:?} has no catalogue row, so its worktree scope is unknown",
                    spec.id
                ),
            }
        })?;
        if declared.runs_on_worktrees {
            scope.push(spec.clone());
        }
    }
    Ok(scope)
}

/// The inherited scope entries that are still pending at `root` (not settled there).
///
/// A worktree evaluates only the ids its own ledger has not settled; that is a
/// per-root *skip*, never a re-selection: the candidate set still comes from the
/// main run's inherited list.
pub fn pending_worktree_migrations(
    root: &Path,
    scope: &[MigrationSpec],
) -> Result<Vec<MigrationSpec>, InstallError> {
    let ledger = MigrationLedger::new(root);
    let mut pending = Vec::new();
    for spec in scope {
        if !ledger.is_settled(spec)? {
            pending.push(spec.clone());
        }
    }
    Ok(pending)
}

/// Whether a `.worktrees/` entry has state this pass can act on (Source
/// `_upgrade_worktrees`).
///
/// `.kittify/` is always enough. Mission state (`kitty-specs/`, `.specify/`) only
/// counts while worktree-scoped migrations are actually in the inherited scope:
/// an empty inherited list must never synthesize upgrade state from a tree that
/// merely looks like a Mission checkout.
pub fn worktree_has_upgradeable_state(worktree: &Path, worktree_scope: &[MigrationSpec]) -> bool {
    worktree.join(".kittify").exists()
        || (!worktree_scope.is_empty()
            && (worktree.join("kitty-specs").exists() || worktree.join(".specify").exists()))
}

/// One executed worktree migration row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorktreeRow {
    /// The migration id.
    pub id: String,
    /// `applied`, `skipped` or `failed` (the Source status vocabulary).
    pub result: String,
}

/// One root's worktree migration pass (Source `_upgrade_worktrees` body for one root).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WorktreeRootExecution {
    /// The version the root's metadata recorded before the pass.
    pub from_version: String,
    /// Executed rows, in catalogue order.
    pub rows: Vec<WorktreeRow>,
    /// Full migration payloads, including failed and no-op results.
    pub migration_results: BTreeMap<String, MigrationOutcome>,
    /// Unprefixed warnings from migrations.
    pub warnings: Vec<String>,
    /// Unprefixed fatal failure messages (errors + `worktree_failures`).
    pub failures: Vec<String>,
    /// Paths a migration held for manual review in this root.
    pub held_paths: BTreeSet<String>,
    /// Sticky: an apply result reported `success = false`.
    pub worktree_failed: bool,
    /// Sticky: a guard or apply error was caught; distinct from `worktree_failed`.
    pub worktree_errored: bool,
    /// Whether this pass wrote `.kittify/metadata.yaml` (in a dry run, would write).
    pub metadata_written: bool,
    /// Whether any new ledger record was appended (a metadata dirty signal).
    pub recorded_any: bool,
}

impl WorktreeRootExecution {
    /// Whether anything in this root failed (a commit must then stay uncommitted).
    pub fn failed(&self) -> bool {
        self.worktree_failed || self.worktree_errored || !self.failures.is_empty()
    }
}

/// One evaluated worktree migration step.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorktreeStep {
    /// The Source status this step reports.
    pub status: StepStatus,
    /// The complete result payload (real partial writes, errors, review paths).
    pub outcome: MigrationOutcome,
    /// Whether this step appended a NEW ledger record (Source's dirty signal).
    pub recorded: bool,
    /// A caught detect/apply error (sticky, distinct from a returned failure).
    pub errored: bool,
    /// The fatal failure line this step contributes, when it failed.
    pub failure: Option<String>,
}

/// Evaluate ONE migration in a worktree, against that worktree's current tree.
///
/// This is the worktree mode of the shared evaluator: the same ledger
/// settlement and the same `record_scoped_result` de-duplication rule the main
/// loop uses, with the Source worktree loop's distinct phase semantics
/// (`runner.py::_upgrade_worktrees`):
///
/// * a `detect()` error is a **recorded** `failed` plus the sticky `errored`
///   flag, and the caller continues with the next migration;
/// * a `can_apply` refusal is a warning-only skip that records nothing;
/// * a returned `success = false` is recorded `failed` (the sticky `failed`
///   flag) and adds no metadata dirt: it must not mint `last_upgraded_at`;
/// * an `apply()` `IgnoreFileUnsafe` error (Source `GitignorePathError`) is the
///   sticky `errored` flag with **no** ledger record, because Source's
///   `apply()` catch for that class returns before the record step; every other
///   apply error is recorded `failed` like Source's `GitCommandError`
///   conversion. Native's single error enum cannot prove Source's uncaught
///   exception classes, so every other class is a recorded, fatal, retryable
///   failure rather than a silent skip.
///
/// `dry_run` never writes: it reports the guard verdict only, because native
/// apply callbacks have no dry-run mode.
pub fn evaluate_worktree_step(
    root: &Path,
    spec: &MigrationSpec,
    native: &NativeMigration,
    ledger: &MigrationLedger,
    cli_version: &str,
    dry_run: bool,
) -> Result<WorktreeStep, InstallError> {
    if ledger.is_settled(spec)? {
        let mut outcome = MigrationOutcome::skipped();
        outcome.changes = vec![format!("Migration {} already applied, skipping", spec.id)];
        return Ok(WorktreeStep {
            status: StepStatus::Skipped,
            outcome,
            recorded: false,
            errored: false,
            failure: None,
        });
    }
    let needed = match (native.detect)(root) {
        Ok(needed) => needed,
        Err(error) => {
            let recorded = if dry_run {
                false
            } else {
                record_scoped_result(ledger, spec, FAILED_RESULT, cli_version)?
            };
            let reason = error.to_string();
            return Ok(WorktreeStep {
                status: StepStatus::Failed,
                outcome: MigrationOutcome {
                    success: false,
                    errors: vec![format!("Cannot safely detect: {reason}")],
                    ..MigrationOutcome::default()
                },
                recorded,
                errored: true,
                failure: Some(format!("Cannot safely detect {}: {reason}", spec.id)),
            });
        }
    };
    if !needed {
        let recorded = if dry_run {
            false
        } else {
            record_scoped_result(ledger, spec, SKIPPED_RESULT, cli_version)?
        };
        let mut outcome = MigrationOutcome::skipped();
        outcome.changes = vec![not_needed_message(&spec.id, dry_run)];
        return Ok(WorktreeStep {
            status: StepStatus::Skipped,
            outcome,
            recorded,
            errored: false,
            failure: None,
        });
    }
    match (native.can_apply)(root) {
        Ok(None) => {}
        Ok(Some(reason)) => {
            let mut outcome = MigrationOutcome::skipped();
            outcome.warnings = vec![format!("Cannot apply {}: {reason}", spec.id)];
            return Ok(WorktreeStep {
                status: StepStatus::Skipped,
                outcome,
                recorded: false,
                errored: false,
                failure: None,
            });
        }
        Err(error) => {
            let recorded = if dry_run {
                false
            } else {
                record_scoped_result(ledger, spec, FAILED_RESULT, cli_version)?
            };
            let reason = error.to_string();
            return Ok(WorktreeStep {
                status: StepStatus::Failed,
                outcome: MigrationOutcome {
                    success: false,
                    errors: vec![format!("Cannot apply {}: {reason}", spec.id)],
                    ..MigrationOutcome::default()
                },
                recorded,
                errored: true,
                failure: Some(format!("Cannot apply {}: {reason}", spec.id)),
            });
        }
    }
    if dry_run {
        // Source's worktree loop passes `dry_run` into `apply()`; the preview door is
        // that same composition, so the change lines are the real ones and nothing is
        // written or recorded.
        let Some(preview) = preview_for(&spec.id) else {
            return Ok(WorktreeStep {
                status: StepStatus::Applied,
                outcome: MigrationOutcome::skipped(),
                recorded: false,
                errored: false,
                failure: None,
            });
        };
        return Ok(match preview(root) {
            Ok(outcome) => {
                let failure = if outcome.success {
                    None
                } else {
                    Some(
                        outcome
                            .errors
                            .first()
                            .cloned()
                            .unwrap_or_else(|| format!("Migration {} failed", spec.id)),
                    )
                };
                WorktreeStep {
                    status: if outcome.success {
                        StepStatus::Applied
                    } else {
                        StepStatus::Failed
                    },
                    outcome,
                    recorded: false,
                    errored: false,
                    failure,
                }
            }
            Err(error) => {
                let reason = error.to_string();
                // A source state failure is the sticky `errored` flag with no record; any
                // other class is a failed result. Nothing is recorded in a dry run.
                WorktreeStep {
                    status: StepStatus::Failed,
                    outcome: MigrationOutcome {
                        success: false,
                        errors: vec![format!("Cannot apply {}: {reason}", spec.id)],
                        ..MigrationOutcome::default()
                    },
                    recorded: false,
                    errored: is_source_state_failure(&error),
                    failure: Some(format!("Cannot apply {}: {reason}", spec.id)),
                }
            }
        });
    }
    match (native.apply)(root) {
        Ok(outcome) => {
            let result = if outcome.success {
                SETTLED_RESULT
            } else {
                FAILED_RESULT
            };
            let recorded = record_scoped_result(ledger, spec, result, cli_version)?;
            let failure = if outcome.success {
                None
            } else {
                // A failure that gave no error of its own still needs one line,
                // or no failure is recorded (#5746).
                Some(
                    outcome
                        .errors
                        .first()
                        .cloned()
                        .unwrap_or_else(|| format!("Migration {} failed", spec.id)),
                )
            };
            Ok(WorktreeStep {
                status: if outcome.success {
                    StepStatus::Applied
                } else {
                    StepStatus::Failed
                },
                outcome,
                recorded,
                errored: false,
                failure,
            })
        }
        Err(error) => {
            let reason = error.to_string();
            // Source's apply() catch: GitignorePathError / state-unreadable return
            // before the ledger record and set only the sticky `errored` flag.
            if is_source_state_failure(&error) {
                return Ok(WorktreeStep {
                    status: StepStatus::Failed,
                    outcome: MigrationOutcome {
                        success: false,
                        errors: vec![format!("Cannot apply {}: {reason}", spec.id)],
                        ..MigrationOutcome::default()
                    },
                    recorded: false,
                    errored: true,
                    failure: Some(format!("Cannot apply {}: {reason}", spec.id)),
                });
            }
            // Source converts a GitCommandError into a returned failed result and
            // records it; every other native error takes the same recorded path.
            let recorded = record_scoped_result(ledger, spec, FAILED_RESULT, cli_version)?;
            Ok(WorktreeStep {
                status: StepStatus::Failed,
                outcome: MigrationOutcome {
                    success: false,
                    errors: vec![format!("Cannot apply {}: {reason}", spec.id)],
                    ..MigrationOutcome::default()
                },
                recorded,
                errored: false,
                failure: Some(format!("Cannot apply {}: {reason}", spec.id)),
            })
        }
    }
}

/// Execute the inherited worktree scope in ONE root and settle its bookkeeping.
///
/// Composition, not a second runner: the candidate set is the inherited scope
/// (already filtered by [`worktree_migration_scope`]), each step goes through
/// [`evaluate_worktree_step`], records go through `record_scoped_result`, and the
/// metadata/schema writers are the same [`ProjectMetadata`] /
/// [`stamp_schema_version`] authorities the main run uses. Discovery and the
/// churn commit stay with the caller (they are Git/CLI concerns).
///
/// Journal rules, all Source `runner.py::_upgrade_worktrees`:
/// * a failing step never stops the root or the fan-out: it is recorded and the
///   loop continues with the next inherited id;
/// * `worktree_failed` (a returned failure) suppresses the schema stamp;
///   `worktree_errored` alone does not;
/// * a returned failure records nothing dirty, but the final version
///   reconciliation still advances `version` (no per-worktree pre-run capture),
///   aligning `last_upgraded_at` to the shared main/fallback stamp;
/// * `last_upgraded_at` is minted fresh only for a genuine change; the
///   bookkeeping-only version bump aligns to `main_bookkeeping_stamp` or the
///   single `fallback_stamp` the caller minted before the sibling loop.
#[allow(clippy::too_many_arguments)]
pub fn run_worktree_migrations(
    root: &Path,
    cli_version: &str,
    target_version: &str,
    scope: &[MigrationSpec],
    dry_run: bool,
    main_bookkeeping_stamp: Option<&str>,
    fallback_stamp: &str,
    lock_wait: Duration,
) -> Result<WorktreeRootExecution, InstallError> {
    for spec in scope {
        if !spec.runs_on_worktrees {
            return Err(InstallError::UpgradeRefused {
                detail: format!(
                    "worktree scope contains the project-only migration {:?}",
                    spec.id
                ),
            });
        }
    }
    let kittify = root.join(".kittify");
    let _lock = if dry_run {
        None
    } else {
        Some(ProjectLock::for_project(root, lock_wait)?)
    };
    let mut metadata = ProjectMetadata::load(&kittify).ok_or_else(|| InstallError::UpgradeRefused {
        detail: format!(
            "{} has no readable .kittify/metadata.yaml; the worktree upgrade cannot determine its version",
            root.display()
        ),
    })?;
    let mut report = WorktreeRootExecution {
        from_version: metadata.version.clone(),
        ..WorktreeRootExecution::default()
    };
    // Source `worktree_metadata_dirty`: starts false for a loaded metadata file;
    // every NEW ledger record (skipped/applied) makes it true. A returned failure
    // is recorded but intentionally does NOT dirty this flag, so it never mints a
    // fresh `last_upgraded_at` of its own.
    let mut dirty = false;
    let ledger = MigrationLedger::new(root);
    for spec in scope {
        let Some(native) = native_for(&spec.id) else {
            return Err(InstallError::MigrationNotImplemented {
                count: 1,
                ids: spec.id.clone(),
            });
        };
        let step = evaluate_worktree_step(root, spec, native, &ledger, cli_version, dry_run)?;
        report.recorded_any |= step.recorded;
        report
            .warnings
            .extend(step.outcome.warnings.iter().cloned());
        if step.outcome.manual_review_required {
            report
                .held_paths
                .extend(step.outcome.preserved_paths.iter().cloned());
        }
        match step.status {
            StepStatus::Applied => {
                dirty |= step.recorded;
                report.rows.push(WorktreeRow {
                    id: spec.id.clone(),
                    result: "applied".to_string(),
                });
            }
            StepStatus::Skipped => {
                dirty |= step.recorded;
                report.rows.push(WorktreeRow {
                    id: spec.id.clone(),
                    result: "skipped".to_string(),
                });
            }
            StepStatus::Failed => {
                report.worktree_errored |= step.errored;
                if step.errored {
                    // Source's detect-error branch dirties the metadata when its
                    // failure record is new; the returned-failure branch never does.
                    dirty |= step.recorded;
                } else {
                    report.worktree_failed = true;
                }
                if let Some(failure) = &step.failure {
                    report.failures.push(failure.clone());
                }
                report.rows.push(WorktreeRow {
                    id: spec.id.clone(),
                    result: "failed".to_string(),
                });
            }
        }
        report
            .migration_results
            .insert(spec.id.clone(), step.outcome);
    }
    if dry_run {
        // Prediction only: the same reconciliation would run, and nothing is written.
        report.metadata_written = metadata.version != target_version
            || metadata.schema_version != Some(CURRENT_SCHEMA_VERSION)
            || report.recorded_any
            || !report.rows.is_empty();
        return Ok(report);
    }
    // Source `_reconcile_worktree_bookkeeping` + schema stamp, in that order.
    let bookkeeping_only_bump = !dirty && metadata.version != target_version;
    if metadata.version != target_version {
        metadata.version = target_version.to_string();
        dirty = true;
    }
    if dirty {
        metadata.last_upgraded_at = Some(if bookkeeping_only_bump {
            main_bookkeeping_stamp.unwrap_or(fallback_stamp).to_string()
        } else {
            now_utc_iso()
        });
        report.metadata_written |= metadata.save(&kittify)?;
    }
    // Source: guarded by `worktree_failed` only. A caught error (errored) still
    // settles the schema stamp; a returned failure suppresses it.
    if !report.worktree_failed {
        report.metadata_written |= stamp_schema_version(&kittify, CURRENT_SCHEMA_VERSION)?;
    }
    Ok(report)
}

fn ensure_initialized(project_root: &Path) -> Result<(), InstallError> {
    if !project_root.join(".kittify/config.yaml").is_file() {
        return Err(InstallError::UpgradeRefused {
            detail: format!(
                "{} is not an initialized Spec Kitty project (no .kittify/config.yaml)",
                project_root.display()
            ),
        });
    }
    Ok(())
}

// ---------------------------------------------------------------- migrations

/// The `info/exclude` file git honours for this checkout, or `None` outside a repository.
///
/// Git reads `info/exclude` from the COMMON git directory for the main checkout and every
/// linked worktree alike (Source `git_ops.git_exclude_file`); a linked worktree's `.git` is
/// a file, so the shared directory is resolved through the Git owner's captured repository
/// state instead of being spelled from the checkout. This is a read-only path resolution:
/// the file write below stays this crate's `atomic_write`.
fn git_exclude_file(project_root: &Path) -> Option<PathBuf> {
    let dot_git = project_root.join(".git");
    if dot_git.is_dir() {
        return Some(dot_git.join("info").join("exclude"));
    }
    if !dot_git.exists() {
        return None;
    }
    let repository = sk_git::GitRepository::open(project_root).ok()?;
    repository
        .git_dirs()
        .ok()
        .map(|dirs| dirs.common_dir().join("info").join("exclude"))
}

/// The *spelled* `.git/info/exclude` of this checkout.
///
/// Source `ExcludeWorktreesMigration` reads this exact path, so in a linked worktree (where
/// `.git` is a file) the migration is a recorded no-op and the later
/// `4.0.0rc7_exclude_spec_kitty` migration, which resolves the common dir, owns the write.
fn spelled_exclude_file(project_root: &Path) -> PathBuf {
    project_root.join(".git").join("info").join("exclude")
}

fn detect_exclude_at(path: &Path, entry: &str) -> Result<bool, InstallError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        // Source tests `exclude_file.exists()`, which is also false when a parent component is
        // a file (a linked worktree's `.git`): the migration is a recorded no-op, not a crash.
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(false);
        }
        Err(source) => return Err(InstallError::io("read", path, source)),
    };
    Ok(!text.lines().any(|line| line.trim() == entry))
}

fn apply_exclude_at(path: &Path, entry: &str) -> Result<MigrationOutcome, InstallError> {
    if !path.exists() {
        // Source `exclude_from_git_index` returns early when the file is absent.
        return Ok(MigrationOutcome::skipped());
    }
    let existing = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(source) => return Err(InstallError::io("read", path, source)),
    };
    let mut text = String::from_utf8(existing).map_err(|error| InstallError::IgnoreFileUnsafe {
        path: path.to_path_buf(),
        detail: error.to_string(),
    })?;
    if text.lines().any(|line| line.trim() == entry) {
        return Ok(MigrationOutcome::skipped());
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(entry);
    text.push('\n');
    sk_kernel::atomic_write(path, text.as_bytes())
        .map_err(|error| InstallError::io("write exclude", path, std::io::Error::other(error)))?;
    Ok(MigrationOutcome::changed(vec![format!(
        "Added {entry} to {}",
        path.display()
    )]))
}

fn detect_exclude(project_root: &Path, entry: &str) -> Result<bool, InstallError> {
    detect_exclude_at(&spelled_exclude_file(project_root), entry)
}

fn apply_exclude(project_root: &Path, entry: &str) -> Result<MigrationOutcome, InstallError> {
    apply_exclude_at(&spelled_exclude_file(project_root), entry)
}

/// The common-dir variant used by `4.0.0rc7_exclude_spec_kitty` (Source
/// `ExcludeSpecKittyMigration._exclude_file`): the file must exist, or the migration is a
/// recorded no-op.
fn detect_common_exclude(project_root: &Path, entry: &str) -> Result<bool, InstallError> {
    let Some(path) = git_exclude_file(project_root) else {
        return Ok(false);
    };
    detect_exclude_at(&path, entry)
}

fn apply_common_exclude(
    project_root: &Path,
    entry: &str,
) -> Result<MigrationOutcome, InstallError> {
    let Some(path) = git_exclude_file(project_root) else {
        return Ok(MigrationOutcome::skipped());
    };
    apply_exclude_at(&path, entry)
}

fn detect_exclude_worktrees(project_root: &Path) -> Result<bool, InstallError> {
    detect_exclude(project_root, ".worktrees/")
}

fn apply_exclude_worktrees(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    apply_exclude(project_root, ".worktrees/")
}

fn detect_exclude_spec_kitty(project_root: &Path) -> Result<bool, InstallError> {
    detect_common_exclude(project_root, ".spec-kitty/")
}

fn apply_exclude_spec_kitty(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    apply_common_exclude(project_root, ".spec-kitty/")
}

fn detect_github_diff_attributes(project_root: &Path) -> Result<bool, InstallError> {
    let path = project_root.join(".gitattributes");
    let existing = match std::fs::read_to_string(&path) {
        Ok(text) => text.lines().map(str::to_string).collect::<Vec<_>>(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(source) => return Err(InstallError::io("read", &path, source)),
    };
    Ok(crate::attributes::GITHUB_DIFF_ATTRIBUTES
        .iter()
        .any(|entry| !existing.iter().any(|line| line == entry)))
}

fn apply_github_diff_attributes(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let path = project_root.join(".gitattributes");
    let mut lines: Vec<String> = match std::fs::read_to_string(&path) {
        Ok(text) => text.lines().map(str::to_string).collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(source) => return Err(InstallError::io("read", &path, source)),
    };
    let mut added = Vec::new();
    for entry in crate::attributes::GITHUB_DIFF_ATTRIBUTES {
        if !lines.iter().any(|line| line == entry) {
            lines.push(entry.to_string());
            added.push(format!("Added .gitattributes entry: {entry}"));
        }
    }
    if added.is_empty() {
        return Ok(MigrationOutcome::skipped());
    }
    let text = format!("{}\n", lines.join("\n").trim_end());
    sk_kernel::atomic_write(&path, text.as_bytes()).map_err(|error| {
        InstallError::io("write .gitattributes", &path, std::io::Error::other(error))
    })?;
    Ok(MigrationOutcome::changed(added))
}

// ------------------------------------------------------------- agent choices

/// The four agent-host upgrade choices.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentUpgradeChoice {
    /// Upgrade now.
    UpgradeNow,
    /// Always keep up to date.
    Always,
    /// Not now.
    NotNow,
    /// Never ask again.
    NeverAsk,
}

impl AgentUpgradeChoice {
    /// Parse the CLI spelling.
    pub fn parse(raw: &str) -> Result<Self, InstallError> {
        match raw {
            "upgrade_now" => Ok(Self::UpgradeNow),
            "always" => Ok(Self::Always),
            "not_now" => Ok(Self::NotNow),
            "never_ask" => Ok(Self::NeverAsk),
            other => Err(InstallError::AgentChoiceInvalid {
                choice: other.to_string(),
                detail: "expected upgrade_now, always, not_now or never_ask".to_string(),
            }),
        }
    }

    /// The CLI spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UpgradeNow => "upgrade_now",
            Self::Always => "always",
            Self::NotNow => "not_now",
            Self::NeverAsk => "never_ask",
        }
    }

    /// Whether this choice asks for an upgrade to run now.
    pub fn runs_upgrade(self) -> bool {
        matches!(self, Self::UpgradeNow | Self::Always)
    }

    /// Whether this choice suppresses future prompts.
    pub fn suppresses_future_prompts(self) -> bool {
        matches!(self, Self::NeverAsk)
    }
}

/// The `upgrade --agent-check --json` payload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AgentCheckPayload {
    /// Schema version (1).
    pub schema_version: u32,
    /// `none`, `auto_upgrade`, `guidance` or `prompt`.
    pub action: String,
    /// The installed version.
    pub installed_version: String,
    /// The latest known version, when known.
    pub latest_version: Option<String>,
    /// Where the latest version came from.
    pub latest_source: Option<String>,
    /// How the CLI was installed.
    pub install_method: String,
    /// The command an agent should run to upgrade.
    pub upgrade_command: Option<String>,
    /// A short note for `guidance`.
    pub upgrade_note: Option<String>,
}

/// The pure decision inputs for an agent upgrade check.
#[derive(Clone, Debug)]
pub struct AgentCheckInput {
    /// The installed CLI version.
    pub installed_version: String,
    /// The latest available version, when the caller fetched one.
    pub latest_version: Option<String>,
    /// The version source (for example `pypi`).
    pub latest_source: Option<String>,
    /// The install method (for example `uv-tool`, `pipx`, `pip`).
    pub install_method: String,
    /// Whether this install method is safe to auto-upgrade.
    pub auto_upgrade_safe: bool,
    /// A previously recorded choice, if any.
    pub choice: Option<AgentUpgradeChoice>,
    /// The upgrade command for this install method.
    pub upgrade_command: Option<String>,
    /// A guidance note when auto-upgrade is unsafe.
    pub guidance_note: Option<String>,
}

/// Decide the agent-check payload without network or disk access.
pub fn plan_agent_check(input: &AgentCheckInput) -> Result<AgentCheckPayload, InstallError> {
    let installed =
        Version::parse(&input.installed_version).map_err(|error| InstallError::VersionInvalid {
            value: input.installed_version.clone(),
            detail: error.to_string(),
        })?;
    let mut payload = AgentCheckPayload {
        schema_version: 1,
        action: "none".to_string(),
        installed_version: input.installed_version.clone(),
        latest_version: input.latest_version.clone(),
        latest_source: input.latest_source.clone(),
        install_method: input.install_method.clone(),
        upgrade_command: input.upgrade_command.clone(),
        upgrade_note: None,
    };
    let Some(latest_text) = &input.latest_version else {
        return Ok(payload);
    };
    let latest = Version::parse(latest_text).map_err(|error| InstallError::VersionInvalid {
        value: latest_text.clone(),
        detail: error.to_string(),
    })?;
    if latest <= installed {
        return Ok(payload);
    }
    match input.choice {
        Some(choice) if choice.suppresses_future_prompts() => Ok(payload),
        Some(choice) if choice.runs_upgrade() && input.auto_upgrade_safe => {
            payload.action = "auto_upgrade".to_string();
            Ok(payload)
        }
        Some(choice) if !choice.runs_upgrade() => Ok(payload),
        _ if input.auto_upgrade_safe && input.choice == Some(AgentUpgradeChoice::Always) => {
            payload.action = "auto_upgrade".to_string();
            Ok(payload)
        }
        _ => {
            if input.auto_upgrade_safe {
                payload.action = "prompt".to_string();
            } else {
                payload.action = "guidance".to_string();
                payload.upgrade_note = input.guidance_note.clone();
            }
            Ok(payload)
        }
    }
}

/// Check a pack pin's `requires_core` against the running core version.
///
/// This is the reusable predicate pack acquisition and init call before placing
/// a tree: an incompatible pin is refused, never partially installed.
pub fn enforce_requires_core(
    pin_path: &Path,
    requires_core: Option<&str>,
    core_version: &str,
) -> Result<(), InstallError> {
    let satisfied = check_requires_core(requires_core, core_version).map_err(|error| {
        InstallError::VersionInvalid {
            value: requires_core.unwrap_or_default().to_string(),
            detail: error.to_string(),
        }
    })?;
    if satisfied {
        return Ok(());
    }
    Err(InstallError::RequiresCoreUnsatisfied {
        pin: pin_path.to_path_buf(),
        requires_core: requires_core.unwrap_or_default().to_string(),
        core_version: core_version.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn seeded(root: &Path) {
        let catalog = MigrationCatalog::embedded().expect("catalog");
        MigrationLedger::new(root)
            .seed_for_init(catalog, "4.0.0rc8", |_spec| false)
            .expect("seed");
    }

    #[test]
    fn execution_scope_preserves_canonical_selection_and_catalogue_order() {
        let directory = tempfile::tempdir().expect("temp");
        initialized(directory.path(), "0.1.0");
        let selected = select_candidates(directory.path(), "4.0.0rc8").expect("selection");
        let scope: BTreeSet<String> = native_migrations()
            .take(2)
            .map(|migration| migration.id.to_owned())
            .collect();
        let expected: Vec<_> = selected
            .iter()
            .filter(|spec| scope.contains(&spec.id))
            .map(|spec| spec.id.clone())
            .collect();
        assert_eq!(expected.len(), 2);
        let request = UpgradeRequest::new(directory.path(), "4.0.0rc8").with_scope(scope);
        let plan = plan_upgrade(&request).expect("scoped plan");
        assert_eq!(
            plan.rows
                .iter()
                .map(|row| row.spec.id.clone())
                .collect::<Vec<_>>(),
            expected
        );
        assert_eq!(
            select_candidates(directory.path(), "4.0.0rc8").expect("unchanged selection"),
            selected
        );
        assert!(
            UpgradeRequest::new(directory.path(), "4.0.0rc8")
                .execution_scope
                .is_none()
        );
    }

    #[test]
    fn empty_execution_scope_reuses_the_held_lock_and_records_nothing() {
        let directory = tempfile::tempdir().expect("temp");
        initialized(directory.path(), "0.1.0");
        let _held = ProjectLock::for_project(directory.path(), Duration::ZERO).expect("outer lock");
        let mut request =
            UpgradeRequest::new(directory.path(), "4.0.0rc8").with_scope(BTreeSet::new());
        request.lock_wait = Duration::ZERO;
        assert!(plan_upgrade(&request).expect("plan").rows.is_empty());
        let report = run_upgrade(&request).expect("canonical reentrant runner");
        assert!(report.success);
        assert!(report.rows.is_empty());
        assert!(report.metadata_written);
        let ledger = MigrationLedger::new(directory.path());
        assert!(ledger.records(false).expect("legacy records").is_empty());
        assert!(ledger.records(true).expect("modern records").is_empty());
        assert_eq!(
            ProjectMetadata::load(&directory.path().join(".kittify"))
                .expect("metadata")
                .version,
            "4.0.0rc8"
        );
    }

    #[test]
    fn every_native_migration_id_is_declared_exactly_once() {
        let catalog = MigrationCatalog::embedded().expect("catalog");
        // The implemented subset grows as families land; the invariant is that
        // every native id is declared exactly once, never that the count is
        // frozen at a tranche boundary.
        let implementations: Vec<_> = native_migrations().collect();
        assert_eq!(
            implementations.len(),
            NATIVE_MIGRATIONS.len()
                + crate::migrations::completion_legacy::IMPLEMENTATIONS.len()
                + crate::migrations::completion_modern::IMPLEMENTATIONS.len()
        );
        assert!(implementations.len() <= catalog.len());
        let mut ids = std::collections::BTreeSet::new();
        for migration in implementations {
            assert!(
                ids.insert(migration.id),
                "duplicate native id {}",
                migration.id
            );
            assert!(
                catalog.get(migration.id).is_some(),
                "{} is not in the declared catalog",
                migration.id
            );
        }
    }

    #[test]
    fn upgrade_takes_the_same_canonical_lock_as_init() {
        let directory = tempfile::tempdir().expect("temp");
        initialized(directory.path(), "4.0.0rc8");
        let catalog = MigrationCatalog::embedded().expect("catalog");
        // Settle both partitions so the run reaches the lock (nothing to apply).
        MigrationLedger::new(directory.path())
            .seed_for_init(catalog, "4.0.0rc8", |_spec| true)
            .expect("seed");
        let lock_path =
            crate::lock::project_lock_path(directory.path()).expect("canonical lock path");
        let (ready_sender, ready_receiver) = std::sync::mpsc::channel();
        let (release_sender, release_receiver) = std::sync::mpsc::channel();
        let holder = std::thread::spawn(move || {
            let _lock =
                ProjectLock::acquire(&lock_path, Duration::from_secs(2)).expect("holder lock");
            ready_sender.send(()).expect("ready");
            release_receiver.recv().expect("release");
        });
        ready_receiver.recv().expect("ready");
        let mut request = UpgradeRequest::new(directory.path(), "4.0.0rc8");
        request.lock_wait = Duration::from_millis(100);
        // Release the holder even when the contention expectation fails, so a
        // red run reports the assertion rather than a deadlocked helper thread.
        let result = run_upgrade(&request);
        release_sender.send(()).expect("release");
        holder.join().expect("join");
        let error = result.expect_err("canonical lock is contended");
        assert_eq!(error.code(), "lock_unavailable");
    }

    #[test]
    fn fresh_init_seeding_leaves_only_clone_local_migrations_pending() {
        let directory = tempfile::tempdir().expect("temp");
        initialized(directory.path(), "4.0.0rc8");
        seeded(directory.path());
        let plan = plan_upgrade(&UpgradeRequest::new(directory.path(), "4.0.0rc8")).expect("plan");
        // Only the clone-local migrations are unseeded (no git effects present).
        let catalog = MigrationCatalog::embedded().expect("catalog");
        let expected: Vec<_> = catalog
            .specs()
            .iter()
            .filter(|spec| spec.clone_local)
            .map(|spec| spec.id.as_str())
            .collect();
        assert_eq!(
            plan.rows
                .iter()
                .map(|row| row.spec.id.as_str())
                .collect::<Vec<_>>(),
            expected
        );
    }

    #[test]
    fn unimplemented_selection_is_refused_before_mutation() {
        let directory = tempfile::tempdir().expect("temp");
        initialized(directory.path(), "0.1.0");
        let error = run_upgrade(&UpgradeRequest::new(directory.path(), "4.0.0rc8"))
            .expect_err("unimplemented");
        assert_eq!(error.code(), "migration_not_implemented");
        assert!(!directory.path().join(".gitattributes").exists());
        // Nothing was recorded.
        assert!(
            MigrationLedger::new(directory.path())
                .records(false)
                .expect("records")
                .is_empty()
        );
    }

    #[test]
    fn dry_run_reports_the_split_without_writing() {
        let project = tempfile::tempdir().expect("temp");
        initialized(project.path(), "0.1.0");
        let report = run_upgrade(&UpgradeRequest::new(project.path(), "4.0.0rc8").dry_run())
            .expect("dry run");
        assert!(report.dry_run);
        assert!(report.success);
        assert!(report.rows.iter().any(|row| row.result == "unimplemented"));
        assert!(
            report
                .rows
                .iter()
                .any(|row| row.result == "would_apply"
                    && row.id == "3.2.0rc28_github_diff_attributes")
        );
        assert!(!project.path().join(".gitattributes").exists());
        assert!(
            MigrationLedger::new(project.path())
                .records(false)
                .expect("records")
                .is_empty()
        );
    }

    #[test]
    fn a_target_caps_the_upper_bound() {
        let directory = tempfile::tempdir().expect("temp");
        initialized(directory.path(), "0.1.0");
        let plan =
            plan_upgrade(&UpgradeRequest::new(directory.path(), "4.0.0rc8").with_target("0.13.5"))
                .expect("plan");
        assert!(
            plan.rows
                .iter()
                .all(|row| Version::parse(&row.spec.target_version).unwrap()
                    <= Version::parse("0.13.5").unwrap())
        );
        assert!(
            !plan
                .rows
                .iter()
                .any(|row| row.spec.id == "3.2.0rc28_github_diff_attributes")
        );
    }

    #[test]
    fn downgrades_and_newer_projects_are_refused() {
        let directory = tempfile::tempdir().expect("temp");
        initialized(directory.path(), "4.1.0");
        let error =
            plan_upgrade(&UpgradeRequest::new(directory.path(), "4.0.0rc8")).expect_err("newer");
        assert_eq!(error.code(), "project_version_newer");

        initialized(directory.path(), "4.0.0rc8");
        let error = plan_upgrade(
            &UpgradeRequest::new(directory.path(), "4.0.0rc8").with_target("4.0.0rc7"),
        )
        .expect_err("older target");
        assert_eq!(error.code(), "target_version_older");
    }

    #[test]
    fn implemented_migrations_apply_record_and_advance_metadata() {
        let directory = tempfile::tempdir().expect("temp");
        // Isolate the three actual filesystem effects, independent of how many
        // other families have since been ported. Exercise the real runner, not
        // a stale refusal followed by direct callback invocations.
        git_project(directory.path(), "4.0.0rc7");
        let report = run_upgrade(&UpgradeRequest::new(directory.path(), "4.0.0rc8"))
            .expect("implemented selection");
        assert!(report.success);
        assert_eq!(report.applied().len(), 3);
        let ledger = MigrationLedger::new(directory.path());
        let catalog = MigrationCatalog::embedded().expect("catalog");
        for id in report.applied() {
            let spec = catalog.get(&id).expect("declared");
            assert!(
                ledger
                    .records(spec.clone_local)
                    .expect("records")
                    .iter()
                    .any(|record| record.id == id && record.result == SETTLED_RESULT)
            );
        }
        assert_eq!(
            ProjectMetadata::load(&directory.path().join(".kittify"))
                .expect("metadata")
                .version,
            "4.0.0rc8"
        );
        let exclude = std::fs::read_to_string(directory.path().join(".git/info/exclude")).unwrap();
        assert!(exclude.contains(".worktrees/"));
        assert!(exclude.contains(".spec-kitty/"));
        assert!(exclude.starts_with("# exclude\n"));
        assert!(!detect_github_diff_attributes(directory.path()).expect("detect"));
        assert!(is_git_repository(directory.path()));
    }

    fn git_project(root: &Path, version: &str) {
        initialized(root, version);
        std::fs::create_dir_all(root.join(".git/info")).unwrap();
        std::fs::write(root.join(".git/info/exclude"), "# exclude\n").unwrap();
        std::fs::write(
            root.join(".git/config"),
            "[core]\n\trepositoryformatversion = 0\n",
        )
        .unwrap();
        seeded(root);
        // Keep only the three implemented migrations un-settled, so the run has a
        // fully implemented selection (the catalog's other 133 are settled).
        let ledger = MigrationLedger::new(root);
        let implemented = [
            "0.13.1_exclude_worktrees",
            "3.2.0rc28_github_diff_attributes",
            "4.0.0rc7_exclude_spec_kitty",
        ];
        let catalog = MigrationCatalog::embedded().unwrap();
        for spec in catalog.specs().iter().filter(|spec| spec.clone_local) {
            if !implemented.contains(&spec.id.as_str()) {
                ledger
                    .append(&LedgerRecord::for_spec(spec, SETTLED_RESULT, "4.0.0rc8"))
                    .unwrap();
            }
        }
        for partition in [false, true] {
            let path = ledger.path_for(partition);
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let kept: Vec<&str> = text
                .lines()
                .filter(|line| {
                    let id = serde_json::from_str::<serde_json::Value>(line)
                        .ok()
                        .and_then(|value| {
                            value
                                .get("id")
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_string)
                        });
                    !id.is_some_and(|id| implemented.contains(&id.as_str()))
                })
                .collect();
            std::fs::write(&path, format!("{}\n", kept.join("\n"))).unwrap();
        }
    }

    #[test]
    fn an_interrupted_run_resumes_only_unrecorded_migrations() {
        let directory = tempfile::tempdir().expect("temp");
        git_project(directory.path(), "4.0.0rc8");
        // Simulate a crash after the first migration wrote its ledger record.
        apply_exclude_worktrees(directory.path()).expect("first migration");
        MigrationLedger::new(directory.path())
            .append(&LedgerRecord::for_spec(
                MigrationCatalog::embedded()
                    .unwrap()
                    .get("0.13.1_exclude_worktrees")
                    .unwrap(),
                SETTLED_RESULT,
                "4.0.0rc8",
            ))
            .expect("record");
        let report =
            run_upgrade(&UpgradeRequest::new(directory.path(), "4.0.0rc8")).expect("resume");
        assert_eq!(report.applied().len(), 2);
        assert!(
            !report
                .applied()
                .iter()
                .any(|id| id == "0.13.1_exclude_worktrees")
        );
        // A third run selects nothing.
        let report =
            run_upgrade(&UpgradeRequest::new(directory.path(), "4.0.0rc8")).expect("no-op");
        assert!(report.applied().is_empty());
        assert!(report.skipped().is_empty());
    }

    #[test]
    fn a_fully_implemented_selection_applies_in_catalog_order() {
        let directory = tempfile::tempdir().expect("temp");
        git_project(directory.path(), "4.0.0rc8");
        let report =
            run_upgrade(&UpgradeRequest::new(directory.path(), "4.0.0rc8")).expect("upgrade");
        assert_eq!(
            report.applied(),
            vec![
                "0.13.1_exclude_worktrees".to_string(),
                "3.2.0rc28_github_diff_attributes".to_string(),
                "4.0.0rc7_exclude_spec_kitty".to_string(),
            ]
        );
        let exclude = std::fs::read_to_string(directory.path().join(".git/info/exclude")).unwrap();
        assert!(exclude.starts_with("# exclude\n"));
        assert!(exclude.contains(".worktrees/"));
        assert!(exclude.contains(".spec-kitty/"));
        let attributes = std::fs::read_to_string(directory.path().join(".gitattributes")).unwrap();
        assert!(attributes.contains("kitty-specs/**/status.json linguist-generated=true"));
        assert!(!detect_github_diff_attributes(directory.path()).unwrap());
        let catalog = MigrationCatalog::embedded().unwrap();
        let ledger = MigrationLedger::new(directory.path());
        for id in report.applied() {
            let spec = catalog.get(&id).expect("spec");
            assert!(
                ledger
                    .records(spec.clone_local)
                    .unwrap()
                    .iter()
                    .any(|record| record.id == id && record.result == SETTLED_RESULT)
            );
        }
    }

    #[test]
    fn a_newer_schema_is_refused() {
        let directory = tempfile::tempdir().expect("temp");
        initialized(directory.path(), "4.0.0rc8");
        let metadata_path = directory.path().join(".kittify/metadata.yaml");
        let text = std::fs::read_to_string(&metadata_path).unwrap();
        std::fs::write(
            &metadata_path,
            text.replace("schema_version: 3", "schema_version: 99"),
        )
        .unwrap();
        let error = plan_upgrade(&UpgradeRequest::new(directory.path(), "4.0.0rc8"))
            .expect_err("schema newer");
        assert_eq!(error.code(), "schema_version_newer");
    }

    #[test]
    fn agent_choice_decisions_are_pure_and_validated() {
        let base = AgentCheckInput {
            installed_version: "4.0.0rc7".to_string(),
            latest_version: Some("4.0.0rc8".to_string()),
            latest_source: Some("pypi".to_string()),
            install_method: "uv-tool".to_string(),
            auto_upgrade_safe: true,
            choice: None,
            upgrade_command: Some("uv tool upgrade spec-kitty-cli".to_string()),
            guidance_note: Some("use your installer".to_string()),
        };
        assert_eq!(plan_agent_check(&base).expect("check").action, "prompt");
        let mut input = base.clone();
        input.choice = Some(AgentUpgradeChoice::parse("always").expect("choice"));
        assert_eq!(
            plan_agent_check(&input).expect("check").action,
            "auto_upgrade"
        );
        input.choice = Some(AgentUpgradeChoice::parse("never_ask").expect("choice"));
        assert_eq!(plan_agent_check(&input).expect("check").action, "none");
        input.choice = None;
        input.auto_upgrade_safe = false;
        assert_eq!(plan_agent_check(&input).expect("check").action, "guidance");
        input.latest_version = Some("4.0.0rc8".to_string());
        input.installed_version = "4.0.0rc8".to_string();
        assert_eq!(plan_agent_check(&input).expect("check").action, "none");
        assert_eq!(
            AgentUpgradeChoice::parse("sometimes")
                .expect_err("invalid")
                .code(),
            "invalid_agent_choice"
        );
        assert!(
            AgentUpgradeChoice::parse("upgrade_now")
                .expect("choice")
                .runs_upgrade()
        );
    }

    #[test]
    fn requires_core_is_enforced() {
        let pin = Path::new("packs/built-in.source.yaml");
        enforce_requires_core(pin, Some(">=4.0.0rc7,<5"), "4.0.0rc8").expect("compatible");
        let error =
            enforce_requires_core(pin, Some(">=4.0.0rc7,<5"), "5.0.0").expect_err("incompatible");
        assert_eq!(error.code(), "PACK_REQUIRES_CORE");
        enforce_requires_core(pin, None, "0.0.1").expect("no constraint");
    }
}
