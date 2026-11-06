//! The mission/status spine modern migrations.
//!
//! The domain work lives in [`sk_status::migration_backfill`]; this module only
//! walks `kitty-specs/` and translates the typed per-mission outcomes into a
//! [`MigrationOutcome`], exactly like the Source migration modules. No second
//! event writer, reducer or meta writer lives here.

use std::path::Path;

use crate::error::InstallError;
use crate::migrations::completion_modern::support;
use crate::upgrade::MigrationOutcome;

fn mission_dirs(project_root: &Path) -> Result<Vec<std::path::PathBuf>, InstallError> {
    support::mission_dirs(project_root)
}

// ---------------------------------------------------------------------------
// verdict_provenance_backfill
// ---------------------------------------------------------------------------

/// `detect`: at least one mission still carries a stranded verdict.
pub fn detect_verdict_provenance_backfill(project_root: &Path) -> Result<bool, InstallError> {
    Ok(mission_dirs(project_root)?.iter().any(|mission| {
        !sk_status::migration_backfill::stranded_verdict_findings(mission).is_empty()
    }))
}

/// `can_apply`: only when a stranded verdict exists.
pub fn can_apply_verdict_provenance_backfill(
    project_root: &Path,
) -> Result<Option<String>, InstallError> {
    if detect_verdict_provenance_backfill(project_root)? {
        Ok(None)
    } else {
        Ok(Some(
            "no stranded verdict provenance found under kitty-specs/".to_string(),
        ))
    }
}

/// `apply`: backfill every mission in sorted order.
pub fn apply_verdict_provenance_backfill(
    project_root: &Path,
) -> Result<MigrationOutcome, InstallError> {
    let missions = mission_dirs(project_root)?;
    if missions.is_empty() {
        return Ok(MigrationOutcome::skipped());
    }
    let mut seeded = 0usize;
    let mut warnings = Vec::new();
    for mission in &missions {
        let outcome = sk_status::migration_backfill::backfill_verdict_provenance(mission)
            .map_err(|error| InstallError::UpgradeRefused { detail: error })?;
        seeded += outcome.appended_wp_ids.len();
        if let Some(error) = outcome.refresh_error {
            let slug = mission
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            warnings.push(format!(
                "{slug}: {error}; heal with `spec-kitty agent status materialize --mission {slug}`"
            ));
        }
    }
    if seeded == 0 {
        return Ok(MigrationOutcome {
            changed: false,
            changes: Vec::new(),
            warnings,
            ..crate::upgrade::MigrationOutcome::default()
        });
    }
    Ok(MigrationOutcome {
        changed: true,
        changes: vec![format!(
            "Backfilled {seeded} stranded verdict event(s) across {} mission(s) scanned",
            missions.len()
        )],
        warnings,
        ..crate::upgrade::MigrationOutcome::default()
    })
}

// ---------------------------------------------------------------------------
// 4_0_0rc5_single_branch_code_lanes_restamp
// ---------------------------------------------------------------------------

/// `detect`: a non-terminal `single_branch` mission with a code lane.
pub fn detect_single_branch_restamp(project_root: &Path) -> Result<bool, InstallError> {
    Ok(
        sk_status::migration_backfill::restamp_single_branch_with_code_lanes(project_root, true)
            .iter()
            .any(|result| result.action == "restamped"),
    )
}

/// `can_apply`: only when a mission is selectable.
pub fn can_apply_single_branch_restamp(
    project_root: &Path,
) -> Result<Option<String>, InstallError> {
    if detect_single_branch_restamp(project_root)? {
        Ok(None)
    } else {
        Ok(Some(
            "no single_branch mission with a code lane found".to_string(),
        ))
    }
}

/// `apply`: re-stamp each selectable mission's stored `topology`.
pub fn apply_single_branch_restamp(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let results =
        sk_status::migration_backfill::restamp_single_branch_with_code_lanes(project_root, false);
    let changes: Vec<String> = results
        .iter()
        .filter(|result| result.action == "restamped")
        .map(|result| format!("{}: topology single_branch -> lanes", result.slug))
        .collect();
    let errors: Vec<String> = results
        .iter()
        .filter(|result| result.action == "error")
        .map(|result| {
            format!(
                "{}: {}",
                result.slug,
                result.reason.as_deref().unwrap_or("unknown error")
            )
        })
        .collect();
    if !errors.is_empty() {
        return Err(InstallError::UpgradeRefused {
            detail: errors.join("; "),
        });
    }
    Ok(MigrationOutcome {
        changed: !changes.is_empty(),
        changes,
        warnings: Vec::new(),
        ..crate::upgrade::MigrationOutcome::default()
    })
}

/// The mission/status effects implemented by this module.
pub const IMPLEMENTATIONS: [crate::upgrade::NativeMigration; 2] = [
    crate::upgrade::NativeMigration {
        id: "verdict_provenance_backfill",
        detect: detect_verdict_provenance_backfill,
        can_apply: can_apply_verdict_provenance_backfill,
        apply: apply_verdict_provenance_backfill,
    },
    crate::upgrade::NativeMigration {
        id: "4_0_0rc5_single_branch_code_lanes_restamp",
        detect: detect_single_branch_restamp,
        can_apply: can_apply_single_branch_restamp,
        apply: apply_single_branch_restamp,
    },
];
