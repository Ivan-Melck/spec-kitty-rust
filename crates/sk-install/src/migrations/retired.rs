//! Migrations the pinned baseline itself declares as recorded no-ops.
//!
//! Two sub-shapes, both real source behaviour rather than a native shortcut:
//!
//! 1. **Superseded migrations.** `upgrade/supersession.py` registers a
//!    `SupersededMigration` substitute for an id a later migration replaced.
//!    The substitute keeps the id, identity, routing key and defining module
//!    (so its `content_hash` stays the shipped module's hash) but `detect()` is
//!    always `False`, so the runner records `skipped` and never applies it. The
//!    runner additionally stamps the record with the successor note, which is
//!    how a successor tells this no-op apart from a `skipped` the shipped
//!    migration recorded itself (ADR 2026-10-17-1, Amendment 2026-10-25).
//!    Because a `skipped` record does not settle an id, a superseded migration
//!    is re-selected and re-skipped on every run — idempotent by construction.
//!
//! 2. **Retired stubs.** Four pre-3.2 modules were replaced outright by
//!    `3.1.1_charter_rename` and reduced to a stub class with the same shape
//!    but no supersession note (so their records stay note-free). Their
//!    `can_apply` reason and `apply` warning are the original fixed strings.
//!
//! Regenerating any of this in native code would be a compatibility invention:
//! a project whose ledger holds the shipped migration's own `success` must not
//! be re-selected, and a project that never ran it must not suddenly mutate.

use std::path::Path;

use crate::error::InstallError;
use crate::upgrade::MigrationOutcome;

/// A migration the pinned registry registers as a recorded no-op.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetiredMigration {
    /// The declared catalog id.
    pub id: &'static str,
    /// The note stamped on its record, empty for the plain retired stubs.
    pub superseded_by: &'static str,
    /// The `can_apply` refusal text.
    pub refusal: &'static str,
    /// The `apply` warning text.
    pub warning: &'static str,
}

const CHARTER_RENAME_REFUSAL: &str = "Superseded by charter-rename migration";
const CHARTER_RENAME_WARNING: &str = "Superseded by charter-rename";
const CUTOVER_NOTE: &str = "Superseded by the charter-pack cutover";

/// Every declared no-op id, in catalog order.
pub const RETIRED_MIGRATIONS: [RetiredMigration; 16] = [
    RetiredMigration {
        id: "0.9.0_frontmatter_only_lanes",
        superseded_by: "Superseded by 0.9.0_residue_safe_frontmatter_only_lanes",
        refusal: "Superseded by 0.9.0_residue_safe_frontmatter_only_lanes",
        warning: "Superseded by 0.9.0_residue_safe_frontmatter_only_lanes",
    },
    RetiredMigration {
        id: "0.9.1_complete_migration",
        superseded_by: "Superseded by 0.9.1_residue_safe_complete_migration",
        refusal: "Superseded by 0.9.1_residue_safe_complete_migration",
        warning: "Superseded by 0.9.1_residue_safe_complete_migration",
    },
    RetiredMigration {
        id: "0.10.12_charter_cleanup",
        superseded_by: "",
        refusal: CHARTER_RENAME_REFUSAL,
        warning: CHARTER_RENAME_WARNING,
    },
    RetiredMigration {
        id: "2.0.0_charter_directory",
        superseded_by: "",
        refusal: CHARTER_RENAME_REFUSAL,
        warning: CHARTER_RENAME_WARNING,
    },
    RetiredMigration {
        id: "2.0.2_charter_context_bootstrap",
        superseded_by: "",
        refusal: CHARTER_RENAME_REFUSAL,
        warning: CHARTER_RENAME_WARNING,
    },
    RetiredMigration {
        id: "2.0.6_consistency_sweep",
        superseded_by: "Superseded by 2.0.6_residue_safe_consistency_sweep",
        refusal: "Superseded by 2.0.6_residue_safe_consistency_sweep",
        warning: "Superseded by 2.0.6_residue_safe_consistency_sweep",
    },
    RetiredMigration {
        id: "2.1.2_fix_charter_doctrine_skill",
        superseded_by: "",
        refusal: CHARTER_RENAME_REFUSAL,
        warning: CHARTER_RENAME_WARNING,
    },
    RetiredMigration {
        id: "2.1.2_fix_glossary_context_skill",
        superseded_by: CUTOVER_NOTE,
        refusal: CUTOVER_NOTE,
        warning: CUTOVER_NOTE,
    },
    RetiredMigration {
        id: "2.1.4_enforce_command_file_state",
        superseded_by: "Superseded by 3.2.0a4_safe_globalize_commands",
        refusal: "Superseded by 3.2.0a4_safe_globalize_commands",
        warning: "Superseded by 3.2.0a4_safe_globalize_commands",
    },
    RetiredMigration {
        id: "3.0.2_restore_prompt_commands",
        superseded_by: "Superseded by 3.2.0a4_safe_globalize_commands",
        refusal: "Superseded by 3.2.0a4_safe_globalize_commands",
        warning: "Superseded by 3.2.0a4_safe_globalize_commands",
    },
    RetiredMigration {
        id: "3.1.1_normalize_status_json",
        superseded_by: "Superseded by 3.1.1_residue_safe_normalize_status_json",
        refusal: "Superseded by 3.1.1_residue_safe_normalize_status_json",
        warning: "Superseded by 3.1.1_residue_safe_normalize_status_json",
    },
    RetiredMigration {
        id: "3.2.0rc35_default_charter_pack",
        superseded_by: CUTOVER_NOTE,
        refusal: CUTOVER_NOTE,
        warning: CUTOVER_NOTE,
    },
    RetiredMigration {
        id: "3.2.5_agents_skills_gitignore_backfill",
        superseded_by: "Superseded by 4.0.0rc8_narrow_agent_gitignore",
        refusal: "Superseded by 4.0.0rc8_narrow_agent_gitignore",
        warning: "Superseded by 4.0.0rc8_narrow_agent_gitignore",
    },
    RetiredMigration {
        id: "3.2.6rc3_narrow_cursor_gitignore",
        superseded_by: "Superseded by 4.0.0rc8_narrow_agent_gitignore",
        refusal: "Superseded by 4.0.0rc8_narrow_agent_gitignore",
        warning: "Superseded by 4.0.0rc8_narrow_agent_gitignore",
    },
    RetiredMigration {
        id: "normalize_activation_absence",
        superseded_by: CUTOVER_NOTE,
        refusal: CUTOVER_NOTE,
        warning: CUTOVER_NOTE,
    },
    RetiredMigration {
        id: "unify_charter_activation_promote_answers",
        superseded_by: "Superseded by 4.0.0rc8_promote_interview_selections",
        refusal: "Superseded by 4.0.0rc8_promote_interview_selections",
        warning: "Superseded by 4.0.0rc8_promote_interview_selections",
    },
];

/// A migration the pinned source keeps registered but implements as inert.
///
/// Unlike the [`RetiredMigration`] table these are not supersession
/// substitutes: the source module itself declares `detect()` = `False` and a
/// fixed `apply()` result. One of them even keeps an allowing `can_apply`, so
/// the refusal column is optional rather than a note.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InertMigration {
    /// The declared catalog id.
    pub id: &'static str,
    /// The `can_apply` refusal text; `None` means "always allowed".
    pub refusal: Option<&'static str>,
    /// The fixed `changes_made` the source reports.
    pub changes: &'static [&'static str],
}

/// `2.0.0_historical_status_migration`: the event log is the sole authority, so
/// the migration is complete by definition and still allows `can_apply`.
const HISTORICAL_STATUS_CHANGES: [&str; 1] = ["Already complete (no-op)"];
/// `0.10.9_repair_templates`: command templates were removed in WP10.
const REPAIR_TEMPLATE_CHANGES: [&str; 1] =
    ["Skipped: command templates removed in WP10; shim generation replaces them"];
const REPAIR_TEMPLATE_REFUSAL: &str =
    "WP10: Command templates were removed. Shim generation replaces template-based commands.";

/// Every declared inert id.
pub const INERT_MIGRATIONS: [InertMigration; 2] = [
    InertMigration {
        id: "0.10.9_repair_templates",
        refusal: Some(REPAIR_TEMPLATE_REFUSAL),
        changes: &REPAIR_TEMPLATE_CHANGES,
    },
    InertMigration {
        id: "2.0.0_historical_status_migration",
        refusal: None,
        changes: &HISTORICAL_STATUS_CHANGES,
    },
];

/// The declared no-op for `id`, when this family owns it.
pub fn retired(id: &str) -> Option<&'static RetiredMigration> {
    RETIRED_MIGRATIONS
        .iter()
        .find(|migration| migration.id == id)
}

/// The declared inert migration for `id`, when this family owns it.
pub fn inert(id: &str) -> Option<&'static InertMigration> {
    INERT_MIGRATIONS.iter().find(|migration| migration.id == id)
}

/// `detect` for every retired id: the old state is never present.
///
/// The substitute's contract is content-independent (`SupersededMigration.detect`
/// ignores the project), so this needs no project inspection at all.
pub fn detect(_project_root: &Path) -> Result<bool, InstallError> {
    Ok(false)
}

/// `can_apply` for a specific retired/inert id (the refusal carries its note).
pub fn can_apply_for(id: &str, _project_root: &Path) -> Result<Option<String>, InstallError> {
    if let Some(migration) = retired(id) {
        return Ok(Some(migration.refusal.to_string()));
    }
    Ok(inert(id).and_then(|migration| migration.refusal.map(str::to_string)))
}

/// `apply` for a specific retired id (the warning carries its note).
pub fn apply_for(id: &str, _project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    if let Some(migration) = retired(id) {
        let mut outcome = MigrationOutcome::unchanged(Vec::new());
        outcome.warnings.push(migration.warning.to_string());
        return Ok(outcome);
    }
    if let Some(migration) = inert(id) {
        return Ok(MigrationOutcome::unchanged(
            migration
                .changes
                .iter()
                .map(|change| (*change).to_string())
                .collect(),
        ));
    }
    Err(InstallError::MigrationNotImplemented {
        count: 1,
        ids: id.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::MigrationCatalog;

    #[test]
    fn every_retired_id_is_declared_and_its_note_matches_the_catalog() {
        let catalog = MigrationCatalog::embedded().expect("catalog");
        for migration in RETIRED_MIGRATIONS {
            let spec = catalog
                .get(migration.id)
                .unwrap_or_else(|| panic!("{} is not declared", migration.id));
            assert_eq!(
                spec.superseded_by, migration.superseded_by,
                "{} widened or lost its supersession note",
                migration.id
            );
        }
    }

    #[test]
    fn every_inert_id_is_declared_and_carries_no_supersession_note() {
        let catalog = MigrationCatalog::embedded().expect("catalog");
        for migration in INERT_MIGRATIONS {
            let spec = catalog
                .get(migration.id)
                .unwrap_or_else(|| panic!("{} is not declared", migration.id));
            assert_eq!(
                spec.superseded_by, "",
                "{} is inert, not superseded",
                migration.id
            );
        }
    }

    #[test]
    fn inert_ids_report_the_source_result_verbatim() {
        let directory = tempfile::tempdir().expect("temp");
        for migration in INERT_MIGRATIONS {
            assert!(!detect(directory.path()).unwrap(), "{}", migration.id);
            assert_eq!(
                can_apply_for(migration.id, directory.path()).unwrap(),
                migration.refusal.map(str::to_string)
            );
            let outcome = apply_for(migration.id, directory.path()).unwrap();
            assert!(!outcome.changed);
            assert_eq!(
                outcome.changes,
                migration
                    .changes
                    .iter()
                    .map(|change| (*change).to_string())
                    .collect::<Vec<_>>()
            );
            assert!(outcome.warnings.is_empty());
        }
    }

    #[test]
    fn retired_ids_never_report_old_state_and_always_refuse() {
        let directory = tempfile::tempdir().expect("temp");
        for migration in RETIRED_MIGRATIONS {
            assert!(!detect(directory.path()).unwrap(), "{}", migration.id);
            assert_eq!(
                can_apply_for(migration.id, directory.path()).unwrap(),
                Some(migration.refusal.to_string())
            );
            let outcome = apply_for(migration.id, directory.path()).unwrap();
            assert!(!outcome.changed);
            assert_eq!(outcome.warnings, vec![migration.warning.to_string()]);
        }
    }
}
