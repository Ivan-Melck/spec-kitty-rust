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
//! [`NATIVE_MIGRATIONS`] (46 ids: the merge-driver family, run-index heal,
//! lane-tip hooks, the 4.0.0rc7 pack/gitignore pair, the narrow agent-ignore
//! rewrite, the stale mission-template fork repair, the ten frozen
//! `.gitignore` entry backfills/removals, the eighteen ids the pinned registry
//! itself registers as recorded no-ops, and the two pre-1.0 layout migrations). If the selection
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

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use crate::error::InstallError;
use crate::ledger::{
    FAILED_RESULT, LedgerRecord, MigrationCatalog, MigrationLedger, MigrationSpec, SETTLED_RESULT,
    SKIPPED_RESULT,
};
use crate::lock::ProjectLock;
use crate::metadata::{CURRENT_SCHEMA_VERSION, ProjectMetadata};
use crate::plan::now_utc_iso;
use crate::version::{Version, check_requires_core};

/// The outcome a native migration reports.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationOutcome {
    /// Whether the migration changed anything.
    pub changed: bool,
    /// Human-readable changes.
    pub changes: Vec<String>,
    /// Non-fatal warnings (preserved content, skipped optional work).
    pub warnings: Vec<String>,
}

impl MigrationOutcome {
    /// A migration that found nothing to do.
    pub fn skipped() -> Self {
        Self {
            changed: false,
            changes: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// A migration that found nothing to do but still reports why.
    pub fn unchanged(changes: Vec<String>) -> Self {
        Self {
            changed: false,
            changes,
            warnings: Vec::new(),
        }
    }

    /// A migration that changed something.
    pub fn changed(changes: Vec<String>) -> Self {
        Self {
            changed: true,
            changes,
            warnings: Vec::new(),
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
pub const NATIVE_MIGRATIONS: [NativeMigration; 46] = [
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
];

fn native_for(id: &str) -> Option<&'static NativeMigration> {
    NATIVE_MIGRATIONS
        .iter()
        .find(|migration| migration.id == id)
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
        }
    }

    /// Set an explicit `--target`.
    pub fn with_target(mut self, target: impl Into<String>) -> Self {
        self.target_version = Some(target.into());
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
    let catalog = MigrationCatalog::embedded()?;
    let ledger = MigrationLedger::new(root);
    let git = is_git_repository(root);
    let mut rows = Vec::new();
    for spec in catalog.specs() {
        let spec_version = match Version::parse(&spec.target_version) {
            Ok(version) => version,
            // An unparseable declared version caps nothing (upstream behavior).
            Err(_) => continue,
        };
        if spec_version > target {
            continue;
        }
        if ledger.is_settled(spec)? {
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
                        Ok(false) => (
                            PlannedOutcome::NotNeeded,
                            Some("project is already in the target state".to_string()),
                        ),
                        Ok(true) => match (native.can_apply)(root) {
                            Err(error) => (PlannedOutcome::Failed, Some(error.to_string())),
                            Ok(Some(reason)) => (PlannedOutcome::NotApplicable, Some(reason)),
                            Ok(None) => (PlannedOutcome::Apply, None),
                        },
                    }
                }
            }
        };
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
        return Ok(UpgradeReport {
            success: true,
            from_version: plan.from_version.clone(),
            to_version: plan.to_version.clone(),
            rows: plan
                .rows
                .iter()
                .map(|row| MigrationResultRow {
                    id: row.spec.id.clone(),
                    result: match row.outcome {
                        PlannedOutcome::Apply => "would_apply".to_string(),
                        PlannedOutcome::NotNeeded => "not_needed".to_string(),
                        PlannedOutcome::SkipOutsideGit => "would_skip_outside_git".to_string(),
                        PlannedOutcome::NotApplicable => "not_applicable".to_string(),
                        PlannedOutcome::Failed => "failed".to_string(),
                        PlannedOutcome::Unimplemented => "unimplemented".to_string(),
                    },
                    changes: row.detail.iter().cloned().collect(),
                    warnings: Vec::new(),
                })
                .collect(),
            dry_run: true,
            metadata_written: false,
        });
    }
    let unimplemented = plan.unimplemented_ids();
    if !unimplemented.is_empty() {
        return Err(InstallError::MigrationNotImplemented {
            count: unimplemented.len(),
            ids: unimplemented.join(", "),
        });
    }
    let root = &request.project_root;
    // Same canonical authority as init (machine state directory, keyed by the
    // resolved project root). A project-relative `.kittify/.install.lock` here
    // would split lock authority: upgrade and init would write the same managed
    // surfaces without excluding each other.
    let _lock = ProjectLock::for_project(root, request.lock_wait)?;
    // Re-plan under the lock: another process may have advanced the ledger.
    let plan = plan_upgrade(request)?;
    let unimplemented = plan.unimplemented_ids();
    if !unimplemented.is_empty() {
        return Err(InstallError::MigrationNotImplemented {
            count: unimplemented.len(),
            ids: unimplemented.join(", "),
        });
    }
    let ledger = MigrationLedger::new(root);
    let mut rows = Vec::new();
    for row in &plan.rows {
        let native = native_for(&row.spec.id).expect("plan only applies implemented migrations");
        match row.outcome {
            PlannedOutcome::Apply => {
                let outcome = match (native.apply)(root) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        record_result(&ledger, &row.spec, FAILED_RESULT, request)?;
                        return Err(error);
                    }
                };
                record_result(&ledger, &row.spec, SETTLED_RESULT, request)?;
                rows.push(MigrationResultRow {
                    id: row.spec.id.clone(),
                    result: "applied".to_string(),
                    changes: outcome.changes,
                    warnings: outcome.warnings,
                });
            }
            PlannedOutcome::NotNeeded => {
                record_result(&ledger, &row.spec, SKIPPED_RESULT, request)?;
                rows.push(MigrationResultRow {
                    id: row.spec.id.clone(),
                    result: "skipped".to_string(),
                    changes: row.detail.iter().cloned().collect(),
                    warnings: Vec::new(),
                });
            }
            PlannedOutcome::Failed => {
                // Re-run the failing hook under the held lock so the caller gets
                // the typed error, then record `failed` exactly as upstream does.
                let error = match (native.detect)(root) {
                    Err(error) => error,
                    Ok(_) => match (native.can_apply)(root) {
                        Err(error) => error,
                        Ok(_) => InstallError::UpgradeRefused {
                            detail: format!(
                                "{} no longer fails to evaluate the project",
                                row.spec.id
                            ),
                        },
                    },
                };
                record_result(&ledger, &row.spec, FAILED_RESULT, request)?;
                return Err(error);
            }
            // Upstream records nothing for "not applicable here": the next run
            // after the project changes must select it again.
            PlannedOutcome::SkipOutsideGit | PlannedOutcome::NotApplicable => {
                rows.push(MigrationResultRow {
                    id: row.spec.id.clone(),
                    result: "skipped".to_string(),
                    changes: row.detail.iter().cloned().collect(),
                    warnings: Vec::new(),
                });
            }
            PlannedOutcome::Unimplemented => unreachable!("refused before mutation"),
        }
    }
    let mut metadata = ProjectMetadata::load(&root.join(".kittify")).ok_or_else(|| {
        InstallError::UpgradeRefused {
            detail: "metadata.yaml disappeared during the upgrade".to_string(),
        }
    })?;
    metadata.version = plan.to_version.clone();
    metadata.last_upgraded_at = Some(now_utc_iso());
    let metadata_written = metadata.save(&root.join(".kittify"))?;
    Ok(UpgradeReport {
        success: true,
        from_version: plan.from_version,
        to_version: plan.to_version,
        rows,
        dry_run: false,
        metadata_written,
    })
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
fn record_result(
    ledger: &MigrationLedger,
    spec: &MigrationSpec,
    result: &str,
    request: &UpgradeRequest,
) -> Result<(), InstallError> {
    let held = ledger
        .records_for(spec)?
        .into_iter()
        .find(|record| record.id == spec.id)
        .map(|record| record.result);
    if held.as_deref() == Some(result)
        || (result == SKIPPED_RESULT && held.as_deref() == Some(SETTLED_RESULT))
    {
        return Ok(());
    }
    let mut record = LedgerRecord::for_spec(spec, result, &request.cli_version);
    record.superseded_by = spec.superseded_by.clone();
    ledger.append(&record)
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

fn exclude_path(project_root: &Path) -> Result<Option<PathBuf>, InstallError> {
    let dot_git = project_root.join(".git");
    match std::fs::symlink_metadata(&dot_git) {
        Ok(metadata) if metadata.file_type().is_dir() => {
            Ok(Some(dot_git.join("info").join("exclude")))
        }
        Ok(metadata) if metadata.file_type().is_file() => {
            Err(InstallError::GitDirectoryUnsupported {
                path: dot_git,
                detail: "a linked worktree's common-dir exclude is a Git-owner operation"
                    .to_string(),
            })
        }
        Ok(_) => Err(InstallError::GitDirectoryUnsupported {
            path: dot_git,
            detail: ".git is not a directory".to_string(),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(InstallError::io("inspect", &dot_git, source)),
    }
}

fn detect_exclude(project_root: &Path, entry: &str) -> Result<bool, InstallError> {
    let Some(path) = exclude_path(project_root)? else {
        return Ok(false);
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(source) => return Err(InstallError::io("read", &path, source)),
    };
    Ok(!text.lines().any(|line| line.trim() == entry))
}

fn apply_exclude(project_root: &Path, entry: &str) -> Result<MigrationOutcome, InstallError> {
    let Some(path) = exclude_path(project_root)? else {
        return Ok(MigrationOutcome::skipped());
    };
    let existing = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(source) => return Err(InstallError::io("read", &path, source)),
    };
    let mut text = String::from_utf8(existing).map_err(|error| InstallError::IgnoreFileUnsafe {
        path: path.clone(),
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
    sk_kernel::atomic_write(&path, text.as_bytes())
        .map_err(|error| InstallError::io("write exclude", &path, std::io::Error::other(error)))?;
    Ok(MigrationOutcome::changed(vec![format!(
        "Added {entry} to {}",
        path.display()
    )]))
}

fn detect_exclude_worktrees(project_root: &Path) -> Result<bool, InstallError> {
    detect_exclude(project_root, ".worktrees/")
}

fn apply_exclude_worktrees(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    apply_exclude(project_root, ".worktrees/")
}

fn detect_exclude_spec_kitty(project_root: &Path) -> Result<bool, InstallError> {
    detect_exclude(project_root, ".spec-kitty/")
}

fn apply_exclude_spec_kitty(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    apply_exclude(project_root, ".spec-kitty/")
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
    fn every_native_migration_id_is_declared_exactly_once() {
        let catalog = MigrationCatalog::embedded().expect("catalog");
        assert_eq!(NATIVE_MIGRATIONS.len(), 46);
        let mut ids = std::collections::BTreeSet::new();
        for migration in NATIVE_MIGRATIONS {
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
    fn a_freshly_seeded_project_plans_nothing() {
        let directory = tempfile::tempdir().expect("temp");
        initialized(directory.path(), "4.0.0rc8");
        seeded(directory.path());
        let plan = plan_upgrade(&UpgradeRequest::new(directory.path(), "4.0.0rc8")).expect("plan");
        // Only the clone-local migrations are unseeded (no git effects present).
        assert!(plan.rows.iter().all(|row| row.spec.clone_local));
        assert!(plan.is_blocked());
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
        initialized(directory.path(), "4.0.0rc7");
        seeded(directory.path());
        std::fs::create_dir_all(directory.path().join(".git/info")).unwrap();
        std::fs::write(
            directory.path().join(".git/info/exclude"),
            "# git exclude\n",
        )
        .unwrap();
        // Mark every implemented migration settled except the two excludes and
        // the diff attributes, so the run has exactly one implemented selection.
        let ledger = MigrationLedger::new(directory.path());
        let catalog = MigrationCatalog::embedded().expect("catalog");
        for id in [
            "0.13.1_exclude_worktrees",
            "3.2.0rc28_github_diff_attributes",
            "4.0.0rc7_exclude_spec_kitty",
        ] {
            // Remove any seeded success so all three are selected.
            let _ = id;
        }
        // All three are clone-local or tracked and unseeded; run refuses because
        // the catalog selection includes unimplemented migrations too.
        let error = run_upgrade(&UpgradeRequest::new(directory.path(), "4.0.0rc8"))
            .expect_err("blocked by unimplemented selection");
        assert_eq!(error.code(), "migration_not_implemented");

        // Directly exercise the native migrations.
        let outcome = apply_exclude_worktrees(directory.path()).expect("apply");
        assert!(outcome.changed);
        let outcome = apply_exclude_spec_kitty(directory.path()).expect("apply");
        assert!(outcome.changed);
        let outcome = apply_github_diff_attributes(directory.path()).expect("apply");
        assert!(outcome.changed);
        let exclude = std::fs::read_to_string(directory.path().join(".git/info/exclude")).unwrap();
        assert!(exclude.contains(".worktrees/"));
        assert!(exclude.contains(".spec-kitty/"));
        assert!(exclude.starts_with("# git exclude\n"));
        assert!(!detect_github_diff_attributes(directory.path()).expect("detect"));
        let _ = ledger;
        let _ = catalog;
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
