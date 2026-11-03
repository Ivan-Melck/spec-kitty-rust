//! Merge-driver seeding migrations.
//!
//! Seven declared migrations share one upstream engine
//! (`_merge_driver_seeding.MergeDriverSeedingMigration`). Each installs two
//! halves of a driver:
//!
//! 1. a tracked `.gitattributes` line, appended with `ensure_line` semantics
//!    (exact-line membership, file content otherwise preserved); and
//! 2. clone-local `merge.<key>.name` / `merge.<key>.driver` values through
//!    `git config --local`, when the project is a Git repository.
//!
//! A non-Git project still gets the tracked half and the exact upstream
//! warning. Detection is the idempotency guard: a project is pending while
//! either half is missing or differs. Both effects are positions in the
//! source, not inventions, and the migration records nothing itself.
//!
//! The driving specs are frozen here rather than read from the live driver
//! registry because a migration must keep writing what its release wrote even
//! after a later release renames a driver (the issue-matrix repoint is exactly
//! that case).

use std::path::Path;

use crate::error::InstallError;
use crate::upgrade::MigrationOutcome;

use super::git;

/// One driver entry a migration seeds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DriverSpec {
    /// `merge.<config_key>` suffix.
    pub config_key: &'static str,
    /// Display name written to `merge.<key>.name`.
    pub name: &'static str,
    /// Command written to `merge.<key>.driver`.
    pub command: &'static str,
    /// Attribute pattern.
    pub pattern: &'static str,
}

impl DriverSpec {
    /// The exact `.gitattributes` line this driver needs.
    pub fn attributes_entry(&self) -> String {
        format!("{} merge={}", self.pattern, self.config_key)
    }
}

const EVENT_LOG: DriverSpec = DriverSpec {
    config_key: "spec-kitty-event-log",
    name: "Spec Kitty event log union merge",
    command: "spec-kitty merge-driver-event-log %O %A %B",
    pattern: "kitty-specs/**/status.events.jsonl",
};

const DECISIONS_EVENT_LOG: DriverSpec = DriverSpec {
    config_key: "spec-kitty-event-log",
    name: "Spec Kitty event log union merge",
    command: "spec-kitty merge-driver-event-log %O %A %B",
    pattern: "kitty-specs/**/decisions.events.jsonl",
};

const GATE_ARTIFACTS: [DriverSpec; 2] = [
    DriverSpec {
        config_key: "spec-kitty-acceptance-matrix",
        name: "Spec Kitty acceptance matrix filled-side merge",
        command: "spec-kitty merge-driver-acceptance-matrix %O %A %B",
        pattern: "kitty-specs/**/acceptance-matrix.json",
    },
    DriverSpec {
        config_key: "spec-kitty-issue-matrix",
        name: "Spec Kitty issue matrix filled-side merge",
        command: "spec-kitty merge-driver-issue-matrix %O %A %B",
        pattern: "kitty-specs/**/issue-matrix.md",
    },
];

const ISSUE_MATRIX_REPOINT: [DriverSpec; 1] = [DriverSpec {
    config_key: "spec-kitty-issue-matrix",
    name: "Spec Kitty issue matrix row-aware merge",
    command: "spec-kitty merge-driver-issue-matrix %O %A %B",
    pattern: "kitty-specs/**/issue-matrix.json",
}];

const META_TRACES: [DriverSpec; 2] = [
    DriverSpec {
        config_key: "spec-kitty-meta",
        name: "Spec Kitty mission meta field merge",
        command: "spec-kitty merge-driver-meta %O %A %B",
        pattern: "kitty-specs/**/meta.json",
    },
    DriverSpec {
        config_key: "spec-kitty-traces",
        name: "Spec Kitty mission traces union merge",
        command: "spec-kitty merge-driver-traces %O %A %B",
        pattern: "kitty-specs/**/traces/*.md",
    },
];

const REVIEW_CYCLE: [DriverSpec; 1] = [DriverSpec {
    config_key: "spec-kitty-review-cycle",
    name: "Spec Kitty review-cycle verdict collision refusal",
    command: "spec-kitty merge-driver-review-cycle %O %A %B",
    pattern: "kitty-specs/**/tasks/*/review-cycle-*.md",
}];

const DECISION_INDEX: [DriverSpec; 1] = [DriverSpec {
    config_key: "spec-kitty-decision-index",
    name: "Spec Kitty decision-index entry union merge",
    command: "spec-kitty merge-driver-decision-index %O %A %B",
    pattern: "kitty-specs/**/decisions/index.json",
}];

const DRY_RUN_SUMMARIES: [(&str, &str); 7] = [
    (
        "3.1.1_event_log_merge_driver",
        "Would install status.events.jsonl merge driver and .gitattributes entry",
    ),
    (
        "3.2.6_decisions_event_log_merge_driver",
        "Would install the decisions.events.jsonl merge driver and .gitattributes entry",
    ),
    (
        "3.2.6_gate_artifact_merge_drivers",
        "Would install gate-artifact merge drivers and .gitattributes entries",
    ),
    (
        "3.2.6_issue_matrix_driver_repoint",
        "Would repoint the issue-matrix merge driver to issue-matrix.json",
    ),
    (
        "3.2.6_meta_traces_merge_drivers",
        "Would install meta.json + traces/*.md merge drivers and .gitattributes entries",
    ),
    (
        "3.2.7_review_cycle_merge_driver",
        "Would install the review-cycle-*.md merge driver and .gitattributes entry",
    ),
    (
        "4.0.0rc5_decision_index_merge_driver",
        "Would install the decisions/index.json merge driver and .gitattributes entry",
    ),
];

/// The driver list for one migration id, or `None` when it is not this family.
pub fn drivers_for(id: &str) -> Option<&'static [DriverSpec]> {
    match id {
        "3.1.1_event_log_merge_driver" => Some(std::slice::from_ref(&EVENT_LOG)),
        "3.2.6_decisions_event_log_merge_driver" => {
            Some(std::slice::from_ref(&DECISIONS_EVENT_LOG))
        }
        "3.2.6_gate_artifact_merge_drivers" => Some(&GATE_ARTIFACTS),
        "3.2.6_issue_matrix_driver_repoint" => Some(&ISSUE_MATRIX_REPOINT),
        "3.2.6_meta_traces_merge_drivers" => Some(&META_TRACES),
        "3.2.7_review_cycle_merge_driver" => Some(&REVIEW_CYCLE),
        "4.0.0rc5_decision_index_merge_driver" => Some(&DECISION_INDEX),
        _ => None,
    }
}

/// The dry-run summary upstream declares for one migration id.
pub fn dry_run_summary(id: &str) -> Option<&'static str> {
    DRY_RUN_SUMMARIES
        .iter()
        .find_map(|(candidate, summary)| (*candidate == id).then_some(*summary))
}

fn attributes_missing(project_root: &Path, drivers: &[DriverSpec]) -> Result<bool, InstallError> {
    let path = project_root.join(".gitattributes");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(source) => return Err(InstallError::io("read .gitattributes", &path, source)),
    };
    Ok(drivers
        .iter()
        .any(|driver| !text.contains(&driver.attributes_entry())))
}

fn config_missing(project_root: &Path, drivers: &[DriverSpec]) -> Result<bool, InstallError> {
    if !project_root.join(".git").exists() {
        return Ok(false);
    }
    for driver in drivers {
        let name_key = format!("merge.{}.name", driver.config_key);
        let command_key = format!("merge.{}.driver", driver.config_key);
        if git::config_get(project_root, &name_key)?.as_deref() != Some(driver.name) {
            return Ok(true);
        }
        if git::config_get(project_root, &command_key)?.as_deref() != Some(driver.command) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Detection for a driver set: either half absent or differing.
pub fn detect_drivers(project_root: &Path, drivers: &[DriverSpec]) -> Result<bool, InstallError> {
    Ok(attributes_missing(project_root, drivers)? || config_missing(project_root, drivers)?)
}

fn ensure_line(path: &Path, line: &str) -> Result<bool, InstallError> {
    let mut lines: Vec<String> = match std::fs::read_to_string(path) {
        Ok(text) => text.lines().map(str::to_string).collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(source) => return Err(InstallError::io("read .gitattributes", path, source)),
    };
    if lines.iter().any(|existing| existing == line) {
        return Ok(false);
    }
    lines.push(line.to_string());
    let text = format!("{}\n", lines.join("\n").trim_end());
    sk_kernel::atomic_write(path, text.as_bytes()).map_err(|error| {
        InstallError::io("write .gitattributes", path, std::io::Error::other(error))
    })?;
    Ok(true)
}

/// Apply a driver set.
pub fn apply_drivers(
    project_root: &Path,
    drivers: &[DriverSpec],
) -> Result<MigrationOutcome, InstallError> {
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    let attributes_path = project_root.join(".gitattributes");
    let is_git_repo = project_root.join(".git").exists();
    for driver in drivers {
        let entry = driver.attributes_entry();
        if ensure_line(&attributes_path, &entry)? {
            changes.push(format!("Added .gitattributes entry: {entry}"));
        }
        if !is_git_repo {
            continue;
        }
        let name_key = format!("merge.{}.name", driver.config_key);
        if git::config_get(project_root, &name_key)?.as_deref() != Some(driver.name) {
            git::config_set(project_root, &name_key, driver.name)?;
            changes.push(format!("Configured git merge.{}.name", driver.config_key));
        }
        let command_key = format!("merge.{}.driver", driver.config_key);
        if git::config_get(project_root, &command_key)?.as_deref() != Some(driver.command) {
            git::config_set(project_root, &command_key, driver.command)?;
            changes.push(format!("Configured git merge.{}.driver", driver.config_key));
        }
    }
    if !is_git_repo {
        warnings.push(
            "Skipped local git merge-driver config because this project is not a git repository"
                .to_string(),
        );
    }
    Ok(MigrationOutcome {
        changed: !changes.is_empty(),
        changes,
        warnings,
    })
}

/// `can_apply` for every driver migration: the project must exist.
pub fn can_apply(project_root: &Path) -> Result<Option<String>, InstallError> {
    if !project_root.exists() {
        return Ok(Some(format!(
            "Project path does not exist: {}",
            project_root.display()
        )));
    }
    Ok(None)
}

/// Detect a named driver migration.
pub fn detect_for(id: &str, project_root: &Path) -> Result<bool, InstallError> {
    let Some(drivers) = drivers_for(id) else {
        return Err(InstallError::UpgradeRefused {
            detail: format!("{id} is not a merge-driver migration"),
        });
    };
    detect_drivers(project_root, drivers)
}

/// Apply a named driver migration.
pub fn apply_for(id: &str, project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let Some(drivers) = drivers_for(id) else {
        return Err(InstallError::UpgradeRefused {
            detail: format!("{id} is not a merge-driver migration"),
        });
    };
    apply_drivers(project_root, drivers)
}
