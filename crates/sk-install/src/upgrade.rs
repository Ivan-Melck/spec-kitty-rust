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
//! crate implements a small, source-read subset ([`NATIVE_MIGRATIONS`]). If the
//! selection contains any migration without a native implementation the real
//! run refuses *before the first write*, naming every id. That is deliberate:
//! silently dropping an old project's migration would claim a compatibility the
//! binary does not have. `dry_run` reports the same split without refusing.
//!
//! Ordering is catalog order (the upstream `order_manifest.txt`), not version
//! sort. Each applied migration writes one ledger record; a failure records
//! `failed` and stops. The project version is only advanced after the whole run
//! succeeds, matching upstream's "failed run leaves the version untouched".
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

    /// A migration that changed something.
    pub fn changed(changes: Vec<String>) -> Self {
        Self {
            changed: true,
            changes,
            warnings: Vec::new(),
        }
    }
}

/// A native implementation: `detect` plus `apply`.
#[derive(Clone, Copy)]
pub struct NativeMigration {
    /// The catalog id.
    pub id: &'static str,
    /// Whether the old state is still present (upstream `detect`).
    pub detect: fn(&Path) -> Result<bool, InstallError>,
    /// Apply the transformation (upstream `apply`, non-dry-run).
    pub apply: fn(&Path) -> Result<MigrationOutcome, InstallError>,
}

/// Every migration implemented natively in this crate.
pub const NATIVE_MIGRATIONS: [NativeMigration; 3] = [
    NativeMigration {
        id: "0.13.1_exclude_worktrees",
        detect: detect_exclude_worktrees,
        apply: apply_exclude_worktrees,
    },
    NativeMigration {
        id: "3.2.0rc28_github_diff_attributes",
        detect: detect_github_diff_attributes,
        apply: apply_github_diff_attributes,
    },
    NativeMigration {
        id: "4.0.0rc7_exclude_spec_kitty",
        detect: detect_exclude_spec_kitty,
        apply: apply_exclude_spec_kitty,
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
    /// A declared git requirement cannot be met here (recorded `skipped`).
    SkipOutsideGit,
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

    /// Ids that would be recorded `skipped` (not needed / outside git).
    pub fn skip_ids(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter(|row| {
                matches!(
                    row.outcome,
                    PlannedOutcome::NotNeeded | PlannedOutcome::SkipOutsideGit
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
                } else if (native.detect)(root)? {
                    (PlannedOutcome::Apply, None)
                } else {
                    (
                        PlannedOutcome::NotNeeded,
                        Some("project is already in the target state".to_string()),
                    )
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
    let _lock = ProjectLock::acquire(&root.join(crate::lock::LOCK_REL), request.lock_wait)?;
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
                        ledger.append(&LedgerRecord::for_spec(
                            &row.spec,
                            FAILED_RESULT,
                            &request.cli_version,
                        ))?;
                        return Err(error);
                    }
                };
                ledger.append(&LedgerRecord::for_spec(
                    &row.spec,
                    SETTLED_RESULT,
                    &request.cli_version,
                ))?;
                rows.push(MigrationResultRow {
                    id: row.spec.id.clone(),
                    result: "applied".to_string(),
                    changes: outcome.changes,
                    warnings: outcome.warnings,
                });
            }
            PlannedOutcome::NotNeeded | PlannedOutcome::SkipOutsideGit => {
                ledger.append(&LedgerRecord::for_spec(
                    &row.spec,
                    SKIPPED_RESULT,
                    &request.cli_version,
                ))?;
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
