//! The one-way migration-ledger adoption backfill (`4.0.0rc8_backfill_migration_ledger`).
//!
//! Before the explicit two-partition ledger (ADR 2026-10-17-1 §6) a project
//! recorded its applied migrations only in `.kittify/metadata.yaml`'s
//! `spec_kitty.migrations:` list, and selection was a **version window**: a
//! migration declared below the recorded version was never selected, whether or
//! not the list named it. Ledger-diff selection reads no version, so adopting
//! the list alone would leave every legacy-settled migration pending and replay
//! it against a current project.
//!
//! This module is that adoption, ported from the pinned
//! `m_4_0_0rc8_backfill_migration_ledger`:
//!
//! - every id in `applied_migrations` (legacy aliases normalized) is written
//!   into its partition; a clone-local id only when its effect is verifiably
//!   present on **this** checkout, so a fresh clone leaves it pending;
//! - **on first adoption only** (until this migration itself holds a `success`),
//!   every declared migration whose `target_version` is below both the recorded
//!   version and `4.0.0rc8`, and that neither the list nor a ledger partition
//!   mentions, is stamped `success` with the same partition rule. That is the
//!   one-time counterpart of the init seed, not a revived selection window:
//!   once this migration is settled, nothing is stamped from the version;
//! - a recorded `skipped`/`failed` result is kept as recorded (it never settles,
//!   so it stays retriable), and a `success` wins over an earlier `skipped` for
//!   the same id (a retried migration carries both);
//! - one `append_many` per partition, so an interrupted adoption leaves no
//!   partial record set.
//!
//! Selection treats the ids this migration *will* settle as settled while it is
//! itself unsettled ([`crate::upgrade::select_candidates`]), so a preview and
//! the real run select the same set.
//!
//! **Deviation, honest:** the legacy metadata record carries no `applied_at`
//! (the pinned record does), so a backfilled record is stamped with the
//! project's own `last_upgraded_at`, else `initialized_at` — the project's own
//! statement of when that history happened, never "now".

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::error::InstallError;
use crate::ledger::{
    LEGACY_MIGRATION_ID_MAP, LedgerRecord, MigrationCatalog, MigrationLedger, MigrationSpec,
    SETTLED_RESULT, clone_local_effect_in_place,
};
use crate::metadata::ProjectMetadata;
use crate::upgrade::MigrationOutcome;

/// The adoption migration's id.
pub const BACKFILL_ID: &str = "4.0.0rc8_backfill_migration_ledger";

/// One record the backfill intends to write.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedRecord {
    /// The canonical migration id.
    pub id: String,
    /// The recorded result.
    pub result: String,
    /// When the project says that history happened.
    pub applied_at: String,
    /// Which partition the record belongs to.
    pub clone_local: bool,
}

/// The canonical id for a possibly-legacy id.
pub fn canonical_id(id: &str) -> &str {
    LEGACY_MIGRATION_ID_MAP
        .iter()
        .find_map(|(legacy, canonical)| (*legacy == id).then_some(*canonical))
        .unwrap_or(id)
}

fn recorded_at(metadata: &ProjectMetadata) -> String {
    metadata
        .last_upgraded_at
        .clone()
        .unwrap_or_else(|| metadata.initialized_at.clone())
}

/// The records the backfill would append, in catalog order.
pub fn plan_backfill(project_root: &Path) -> Result<Vec<PlannedRecord>, InstallError> {
    let Some(metadata) = ProjectMetadata::load(&project_root.join(".kittify")) else {
        return Ok(Vec::new());
    };
    let catalog = MigrationCatalog::embedded()?;
    let ledger = MigrationLedger::new(project_root);
    let tracked_recorded = ledger.recorded_ids(false)?;
    let clone_recorded = ledger.recorded_ids(true)?;

    // One record per canonical id, preferring a `success` (the pinned rule: a
    // retried migration carries `[skipped|failed, success]`).
    let mut settling: BTreeMap<String, String> = BTreeMap::new();
    for record in &metadata.applied_migrations {
        let canonical = canonical_id(&record.id).to_string();
        match settling.get(&canonical) {
            Some(existing) if existing == SETTLED_RESULT && record.result != SETTLED_RESULT => {}
            _ => {
                settling.insert(canonical, record.result.clone());
            }
        }
    }

    let stamp = recorded_at(&metadata);
    let mut plan: Vec<PlannedRecord> = Vec::new();
    let mut planned_ids: BTreeSet<String> = BTreeSet::new();
    for (id, result) in &settling {
        let Some(spec) = catalog.get(id) else {
            // A retired/renamed id with no declaration: record it faithfully in
            // the tracked partition with no drift hash (there is no source).
            if !tracked_recorded.contains(id) {
                plan.push(PlannedRecord {
                    id: id.clone(),
                    result: result.clone(),
                    applied_at: stamp.clone(),
                    clone_local: false,
                });
                planned_ids.insert(id.clone());
            }
            continue;
        };
        if spec.clone_local {
            if !clone_local_effect_in_place(project_root, spec) || clone_recorded.contains(id) {
                continue;
            }
        } else if tracked_recorded.contains(id) {
            continue;
        }
        plan.push(PlannedRecord {
            id: id.clone(),
            result: result.clone(),
            applied_at: stamp.clone(),
            clone_local: spec.clone_local,
        });
        planned_ids.insert(id.clone());
    }

    // The one-time adoption baseline, only until this migration is settled.
    let adoption_settled = catalog
        .get(BACKFILL_ID)
        .map(|spec| ledger.is_settled(spec))
        .transpose()?
        .unwrap_or(false);
    if !adoption_settled {
        let Some(ceiling) = version_ceiling(&metadata.version) else {
            return Ok(plan);
        };
        for spec in catalog.specs() {
            if spec.id == BACKFILL_ID
                || planned_ids.contains(&spec.id)
                || settling.contains_key(&spec.id)
                || tracked_recorded.contains(&spec.id)
                || clone_recorded.contains(&spec.id)
            {
                continue;
            }
            if version_at_least(&spec.target_version, &ceiling) {
                continue;
            }
            if spec.clone_local && !clone_local_effect_in_place(project_root, spec) {
                continue;
            }
            plan.push(PlannedRecord {
                id: spec.id.clone(),
                result: SETTLED_RESULT.to_string(),
                applied_at: stamp.clone(),
                clone_local: spec.clone_local,
            });
        }
    }
    Ok(plan)
}

/// The adoption ceiling: the lower of the recorded version and `4.0.0rc8`.
fn version_ceiling(recorded_version: &str) -> Option<crate::version::Version> {
    let recorded = crate::version::Version::parse(recorded_version).ok()?;
    let target = crate::version::Version::parse("4.0.0rc8").ok()?;
    Some(if recorded < target { recorded } else { target })
}

fn version_at_least(candidate: &str, ceiling: &crate::version::Version) -> bool {
    match crate::version::Version::parse(candidate) {
        Ok(version) => version >= *ceiling,
        // An unparseable declared version caps nothing (never a silent skip).
        Err(_) => false,
    }
}

/// The ids this backfill will settle as `success` right now.
pub fn adoption_settled_ids(project_root: &Path) -> Result<BTreeSet<String>, InstallError> {
    Ok(plan_backfill(project_root)?
        .into_iter()
        .filter(|record| record.result == SETTLED_RESULT)
        .map(|record| record.id)
        .collect())
}

/// `4.0.0rc8_backfill_migration_ledger.detect`.
pub fn detect(project_root: &Path) -> Result<bool, InstallError> {
    Ok(!plan_backfill(project_root)?.is_empty())
}

/// `4.0.0rc8_backfill_migration_ledger.apply`.
pub fn apply(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let plan = plan_backfill(project_root)?;
    if plan.is_empty() {
        return Ok(MigrationOutcome::unchanged(vec![
            "migration ledger already backfilled".to_string(),
        ]));
    }
    let catalog = MigrationCatalog::embedded()?;
    let records: Vec<LedgerRecord> = plan
        .iter()
        .map(|item| record_for(item, catalog, BACKFILL_ID))
        .collect();
    MigrationLedger::new(project_root).append_many(&records)?;
    let changes = plan
        .iter()
        .map(|item| {
            format!(
                "Backfilled {} into the {} ledger",
                item.id,
                if item.clone_local {
                    "clone-local"
                } else {
                    "tracked"
                }
            )
        })
        .collect();
    Ok(MigrationOutcome {
        changed: true,
        changes,
        warnings: Vec::new(),
        ..crate::upgrade::MigrationOutcome::default()
    })
}

fn record_for(item: &PlannedRecord, catalog: &MigrationCatalog, cli_version: &str) -> LedgerRecord {
    let (identity, content_hash, clone_local) = match catalog.get(&item.id) {
        Some(spec) => (
            spec.identity.clone(),
            spec.content_hash.clone(),
            spec.clone_local,
        ),
        None => (item.id.clone(), String::new(), item.clone_local),
    };
    LedgerRecord {
        id: item.id.clone(),
        applied_at: item.applied_at.clone(),
        result: item.result.clone(),
        identity,
        content_hash,
        cli_version: cli_version.to_string(),
        clone_local,
        superseded_by: String::new(),
    }
}

/// The declared spec for a backfilled id, when the catalog still carries it.
pub fn spec_for(id: &str) -> Option<&'static MigrationSpec> {
    MigrationCatalog::embedded()
        .ok()
        .and_then(|catalog| catalog.get(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, content: impl AsRef<[u8]>) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    fn project(version: &str, migrations: &[(&str, &str)]) -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        write(
            &directory.path().join(".kittify/config.yaml"),
            "agents:\n  available:\n  - claude\n",
        );
        let mut block = String::new();
        if !migrations.is_empty() {
            block.push_str("migrations:\n  applied:\n");
            for (id, result) in migrations {
                block.push_str(&format!("  - id: {id}\n    result: {result}\n"));
            }
        }
        write(
            &directory.path().join(".kittify/metadata.yaml"),
            format!(
                "spec_kitty:\n  schema_version: 3\n  version: {version}\n  initialized_at: '2025-01-01T00:00:00+00:00'\n  last_upgraded_at: '2025-06-01T00:00:00+00:00'\n{block}"
            ),
        );
        directory
    }

    #[test]
    fn listed_ids_are_backfilled_with_the_projects_own_timestamp() {
        let directory = project(
            "2.0.11",
            &[
                ("2.0.11_remove_clarify_command", SETTLED_RESULT),
                ("0.10.12_constitution_cleanup", SETTLED_RESULT),
            ],
        );
        let plan = plan_backfill(directory.path()).expect("plan");
        let clarify = plan
            .iter()
            .find(|record| record.id == "2.0.11_remove_clarify_command")
            .expect("listed id");
        assert_eq!(clarify.result, SETTLED_RESULT);
        assert_eq!(clarify.applied_at, "2025-06-01T00:00:00+00:00");
        assert!(!clarify.clone_local);
        // A legacy constitution-era id is recorded under its canonical name.
        assert!(
            plan.iter()
                .any(|record| record.id == "0.10.12_charter_cleanup"),
            "{plan:?}"
        );
    }

    #[test]
    fn first_adoption_stamps_the_pre_ledger_version_window_and_nothing_at_or_above_it() {
        let directory = project("2.0.11", &[]);
        let plan = plan_backfill(directory.path()).expect("plan");
        let ids: Vec<&str> = plan.iter().map(|record| record.id.as_str()).collect();
        assert!(ids.contains(&"0.6.5_commands_rename"), "{ids:?}");
        assert!(
            !ids.contains(&"2.0.11_remove_clarify_command"),
            "a migration AT the recorded version stays pending"
        );
        assert!(
            plan.iter()
                .all(|record| record.applied_at == "2025-06-01T00:00:00+00:00")
        );
    }

    #[test]
    fn a_clone_local_record_needs_its_effect_present_on_this_checkout() {
        let directory = project("4.0.0rc7", &[("0.13.1_exclude_worktrees", SETTLED_RESULT)]);
        let plan = plan_backfill(directory.path()).expect("plan");
        assert!(
            !plan
                .iter()
                .any(|record| record.id == "0.13.1_exclude_worktrees"),
            "no git repository means the effect cannot be in place"
        );
        write(&directory.path().join(".git/info/exclude"), ".worktrees/\n");
        let plan = plan_backfill(directory.path()).expect("plan");
        let record = plan
            .iter()
            .find(|record| record.id == "0.13.1_exclude_worktrees")
            .expect("effect present");
        assert!(record.clone_local);
    }

    #[test]
    fn applying_writes_one_record_per_partition_and_is_idempotent() {
        let directory = project(
            "4.0.0rc7",
            &[("2.0.11_remove_clarify_command", SETTLED_RESULT)],
        );
        write(&directory.path().join(".git/info/exclude"), ".worktrees/\n");
        let outcome = apply(directory.path()).expect("apply");
        assert!(outcome.changed);
        let ledger = MigrationLedger::new(directory.path());
        let tracked = ledger.records(false).expect("tracked");
        assert!(
            tracked
                .iter()
                .any(|record| record.id == "2.0.11_remove_clarify_command"
                    && record.result == SETTLED_RESULT)
        );
        let clone_local = ledger.records(true).expect("clone-local");
        assert!(
            clone_local
                .iter()
                .any(|record| record.id == "0.13.1_exclude_worktrees" && record.clone_local)
        );
        // A second apply finds the adoption baseline already recorded, so it is a
        // no-op: the baseline is stamped once, not on every run.
        let second = apply(directory.path()).expect("second apply");
        assert!(!second.changed, "the adoption baseline is stamped once");
        let ids: BTreeSet<String> = ledger
            .records(false)
            .unwrap()
            .into_iter()
            .map(|record| record.id)
            .collect();
        assert!(ids.contains("2.0.11_remove_clarify_command"));
    }

    #[test]
    fn a_recorded_success_wins_over_an_earlier_skipped() {
        let directory = project(
            "2.0.11",
            &[
                ("2.0.11_remove_clarify_command", "skipped"),
                ("2.0.11_remove_clarify_command", SETTLED_RESULT),
            ],
        );
        let plan = plan_backfill(directory.path()).expect("plan");
        let record = plan
            .iter()
            .find(|record| record.id == "2.0.11_remove_clarify_command")
            .expect("record");
        assert_eq!(record.result, SETTLED_RESULT);
    }

    #[test]
    fn a_recorded_skipped_is_kept_and_never_settles() {
        let directory = project("4.0.0rc7", &[("3.2.0rc35_default_charter_pack", "skipped")]);
        let plan = plan_backfill(directory.path()).expect("plan");
        let record = plan
            .iter()
            .find(|record| record.id == "3.2.0rc35_default_charter_pack")
            .expect("record");
        assert_eq!(record.result, "skipped");
        let settled = adoption_settled_ids(directory.path()).expect("settled ids");
        assert!(!settled.contains("3.2.0rc35_default_charter_pack"));
    }
}
