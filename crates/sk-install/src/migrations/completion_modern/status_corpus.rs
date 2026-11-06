//! `kitty-specs/` corpus walks that reuse the canonical status authorities.
//!
//! | id | effect |
//! | --- | --- |
//! | [`LIFECYCLE_ENVELOPE_ID`] | rewrite legacy 9-key lifecycle envelopes to the strict 14-key shape |
//! | [`RESIDUE_SAFE_STATUS_ID`] | normalise `status.json`'s `materialized_at`, never in a residue folder |
//!
//! Both walk the same corpus the pinned migrations walk — the project-level
//! event log first, then every `kitty-specs/<mission>/` directory in name order
//! — and both delegate the write itself to the authoritative crate:
//! [`sk_status::migrate_lifecycle_envelope`] for the envelope rewrite and
//! [`sk_status::reduce_rows`] plus the Python-compatible indented JSON writer
//! for the snapshot normalisation. Nothing here re-implements the envelope
//! shape, the reducer, or the JSON encoder.

use std::path::{Path, PathBuf};

use sk_status::migrate_lifecycle_envelope;
use sk_status::mutation_json::python_indented_json;
use sk_status::snapshot::{EVENTS_FILENAME, SNAPSHOT_FILENAME, read_raw_events, reduce_rows};

use crate::error::InstallError;
use crate::upgrade::{MigrationOutcome, NativeMigration};

use super::support;

/// `migrate_lifecycle_envelope`.
pub const LIFECYCLE_ENVELOPE_ID: &str = "migrate_lifecycle_envelope";
/// `3.1.1_residue_safe_normalize_status_json`.
pub const RESIDUE_SAFE_STATUS_ID: &str = "3.1.1_residue_safe_normalize_status_json";
/// The shipped predecessor the residue-safe successor defers to.
pub const NORMALIZE_STATUS_PREDECESSOR_ID: &str = "3.1.1_normalize_status_json";

// ---------------------------------------------------------------------------
// migrate_lifecycle_envelope
// ---------------------------------------------------------------------------

/// Every lifecycle event log that currently exists, project log first.
///
/// A log that was never written is skipped: the rewrite would report zero rows
/// for it anyway, and skipping avoids acquiring its write lock for nothing.
fn lifecycle_log_paths(project_root: &Path) -> Result<Vec<PathBuf>, InstallError> {
    let mut paths = Vec::new();
    let project_log = project_root.join(crate::lifecycle::PROJECT_EVENTS_REL);
    if project_log.exists() {
        paths.push(project_log);
    }
    for mission in support::mission_dirs(project_root)? {
        let log = mission.join(EVENTS_FILENAME);
        if log.exists() {
            paths.push(log);
        }
    }
    Ok(paths)
}

fn envelope_needs_rewrite(path: &Path) -> Result<bool, InstallError> {
    let manifest = migrate_lifecycle_envelope(path, true, None, None).map_err(|error| {
        InstallError::LifecycleEventInvalid {
            path: path.to_path_buf(),
            detail: error.to_string(),
        }
    })?;
    Ok(manifest.migrated_count > 0)
}

/// `detect`: at least one event log still carries a legacy-shaped row.
pub fn detect_lifecycle_envelope(project_root: &Path) -> Result<bool, InstallError> {
    for path in lifecycle_log_paths(project_root)? {
        if envelope_needs_rewrite(&path)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `can_apply`: only when a legacy row survives.
pub fn can_apply_lifecycle_envelope(project_root: &Path) -> Result<Option<String>, InstallError> {
    if detect_lifecycle_envelope(project_root)? {
        return Ok(None);
    }
    Ok(Some(
        "no legacy-shaped lifecycle envelope rows found in any event log".to_string(),
    ))
}

/// `apply`: rewrite every legacy row across the corpus.
///
/// A per-file refusal (a stale `.pre-migration.bak` from an interrupted earlier
/// run) does not abort the remaining logs — the rewrite's refusal scope is the
/// file — but the aggregate outcome fails and names the exact path, so the run
/// is never recorded as settled while a legacy log survives.
pub fn apply_lifecycle_envelope(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let paths = lifecycle_log_paths(project_root)?;
    if paths.is_empty() {
        return Ok(MigrationOutcome::skipped());
    }
    let scanned = paths.len();
    let mut migrated_total = 0usize;
    let mut refusals = Vec::new();
    for path in &paths {
        let manifest = migrate_lifecycle_envelope(path, false, None, None).map_err(|error| {
            InstallError::LifecycleEventInvalid {
                path: path.clone(),
                detail: error.to_string(),
            }
        })?;
        if let Some(reason) = manifest.refused_reason {
            refusals.push(format!("{}: {reason}", path.display()));
            continue;
        }
        migrated_total += manifest.migrated_count;
    }
    if !refusals.is_empty() {
        return Err(InstallError::LifecycleEventInvalid {
            path: project_root.to_path_buf(),
            detail: refusals.join("; "),
        });
    }
    if migrated_total == 0 {
        return Ok(MigrationOutcome::skipped());
    }
    Ok(MigrationOutcome::changed(vec![format!(
        "Rewrote {migrated_total} legacy-shaped lifecycle envelope row(s) to F1's strict shape across {scanned} event log(s) scanned"
    )]))
}

// ---------------------------------------------------------------------------
// 3.1.1_residue_safe_normalize_status_json
// ---------------------------------------------------------------------------

/// The snapshot's expected deterministic `materialized_at` for one mission.
fn expected_materialized_at(mission: &Path) -> Result<String, InstallError> {
    let events = mission.join(EVENTS_FILENAME);
    let rows = read_raw_events(&events).map_err(|error| InstallError::MetadataInvalid {
        path: events.clone(),
        detail: error.to_string(),
    })?;
    let snapshot = reduce_rows(&rows).map_err(|error| InstallError::MetadataInvalid {
        path: events,
        detail: error.to_string(),
    })?;
    Ok(snapshot.materialized_at)
}

/// Whether one mission's `status.json` carries a stale `materialized_at`.
///
/// Read-only, exactly like the source check: a snapshot that cannot be read or
/// reduced is "nothing to do" here, so `detect`/dry-run never mutate and never
/// hide a stale file behind an error.
fn status_json_needs_normalisation(mission: &Path) -> bool {
    let status_path = mission.join(SNAPSHOT_FILENAME);
    if !status_path.exists() {
        return false;
    }
    let Ok(text) = std::fs::read_to_string(&status_path) else {
        return false;
    };
    let Ok(serde_json::Value::Object(current)) = serde_json::from_str(&text) else {
        return false;
    };
    let Ok(expected) = expected_materialized_at(mission) else {
        return false;
    };
    current
        .get("materialized_at")
        .and_then(|value| value.as_str())
        != Some(expected.as_str())
}

/// `detect`: some mission's `status.json` is stale and the predecessor is unsettled.
pub fn detect_residue_safe_status(project_root: &Path) -> Result<bool, InstallError> {
    if support::settled_in_selection_view(project_root, NORMALIZE_STATUS_PREDECESSOR_ID)? {
        return Ok(false);
    }
    let (missions, _residue) = support::partition_kitty_specs(project_root)?;
    Ok(missions
        .iter()
        .any(|mission| status_json_needs_normalisation(mission)))
}

/// `can_apply`: the project must look like a Spec Kitty project.
pub fn can_apply_residue_safe_status(project_root: &Path) -> Result<Option<String>, InstallError> {
    if project_root.join(support::KITTY_SPECS_DIR).exists()
        || project_root.join(".kittify").exists()
    {
        return Ok(None);
    }
    Ok(Some("No kitty-specs/ directory found".to_string()))
}

/// `apply`: normalise each Mission's `status.json`, leaving residue untouched.
pub fn apply_residue_safe_status(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let (missions, residue) = support::partition_kitty_specs(project_root)?;
    let mut changes = Vec::new();
    let mut errors = Vec::new();
    for mission in &missions {
        let name = mission
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string();
        if !status_json_needs_normalisation(mission) {
            continue;
        }
        match normalise_one(mission) {
            Ok(()) => changes.push(format!("{name}: normalised status.json")),
            Err(detail) => errors.push(format!("{name}: {detail}")),
        }
    }
    if !errors.is_empty() {
        return Err(InstallError::MetadataInvalid {
            path: project_root.to_path_buf(),
            detail: errors.join("; "),
        });
    }
    let warnings = support::residue_warnings(&residue, project_root);
    Ok(MigrationOutcome {
        changed: !changes.is_empty(),
        changes,
        warnings,
        ..crate::upgrade::MigrationOutcome::default()
    })
}

fn normalise_one(mission: &Path) -> Result<(), String> {
    let status_path = mission.join(SNAPSHOT_FILENAME);
    let text = std::fs::read_to_string(&status_path).map_err(|error| error.to_string())?;
    let mut current: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let expected = expected_materialized_at(mission).map_err(|error| error.to_string())?;
    let Some(object) = current.as_object_mut() else {
        return Err("status.json root is not an object".to_string());
    };
    object.insert(
        "materialized_at".to_string(),
        serde_json::Value::String(expected),
    );
    let rendered = python_indented_json(&current);
    sk_kernel::no_follow::write_text_no_follow(&status_path, &rendered, 0o644)
        .map_err(|error| error.to_string())
}

/// The two corpus implementations.
pub const IMPLEMENTATIONS: &[NativeMigration] = &[
    NativeMigration {
        id: LIFECYCLE_ENVELOPE_ID,
        detect: detect_lifecycle_envelope,
        can_apply: can_apply_lifecycle_envelope,
        apply: apply_lifecycle_envelope,
    },
    NativeMigration {
        id: RESIDUE_SAFE_STATUS_ID,
        detect: detect_residue_safe_status,
        can_apply: can_apply_residue_safe_status,
        apply: apply_residue_safe_status,
    },
];
