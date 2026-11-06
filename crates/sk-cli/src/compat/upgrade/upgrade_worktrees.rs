//! Per-worktree upgrade execution: discovery, Source integration-skip, per-root
//! migrations, and the canonical churn commit (#2385).
//!
//! Source: `src/specify_cli/upgrade/runner.py` (`_is_integrating_worktree`,
//! `_worktrees_to_upgrade`, `_upgrade_worktrees`, `_reconcile_worktree_bookkeeping`),
//! `src/specify_cli/upgrade/autocommit.py` (`capture_upgrade_baseline`,
//! `prepare_upgrade_commit_files`, `expand_upgrade_commit_path`,
//! `is_upgrade_commit_eligible`, `commit_touched_checkout`).
//!
//! What this module composes, and what it never re-implements:
//!
//! * **selection + planning** — [`sk_install::plan_upgrade`] per root, so the
//!   ledger-diff selection, target bound and per-root version come from the one
//!   planner;
//! * **execution scope** — [`sk_install::NativeMigration::runs_on_worktrees`]
//!   read from the one embedded catalogue. The 22 project-only migrations never
//!   execute in a worktree and are reported as such, with no writes at that root;
//! * **ledger** — [`sk_install::MigrationLedger`] records per root, through the
//!   same `(id, result)` idempotency rule the runner uses;
//! * **commit** — one `sk_git` safe commit per touched root with
//!   [`sk_git::CommitGuardCapability::UpgradeBookkeeping`], the commit set being
//!   the status diff against the pre-run baseline;
//! * **lock** — [`sk_install::ProjectLock`] per root (keyed on that root's
//!   resolved path), so no outer lock and no lock bypass is introduced.
//!
//! The preview ([`plan_worktree_upgrade`]) and the execution
//! ([`run_worktree_upgrade`]) share the discovery, the scope partition and the
//! commit-policy computation, so a preview can never describe an execution the
//! run does not perform.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use sk_git::{
    BranchName, CommitGuardCapability, GitPath, GitRepository, ProtectedBranches,
    SafeCommitRequest, StatusOptions, UntrackedMode,
};

use sk_install::CURRENT_SCHEMA_VERSION;
use sk_install::{
    InstallError, MigrationSpec, ProjectMetadata, now_utc_iso,
    upgrade::{
        pending_worktree_migrations, run_worktree_migrations, worktree_has_upgradeable_state,
        worktree_migration_scope,
    },
};

/// The Source worktree directory, relative to the project root.
pub const WORKTREES_DIRNAME: &str = ".worktrees";

/// Source `autocommit.UPGRADE_COMMIT_SKIP_WARNING`.
pub const UPGRADE_COMMIT_SKIP_WARNING: &str =
    "Could not auto-commit upgrade changes; please review and commit manually.";
/// Source `autocommit.DETACHED_HEAD_WARNING`.
pub const DETACHED_HEAD_WARNING: &str = "Checkout is on a detached HEAD; skipped auto-committing\
     upgrade changes — please review and commit manually.";
/// Source `autocommit.BRANCH_DETECTION_FAILED_WARNING`.
pub const BRANCH_DETECTION_FAILED_WARNING: &str = "Could not determine the current branch; skipped\
     auto-committing upgrade changes — please review and commit manually.";
/// Source `autocommit.BASELINE_UNAVAILABLE_WARNING`.
pub const BASELINE_UNAVAILABLE_WARNING: &str = "Could not read the git status before the upgrade;\
     nothing was committed — review the changes and commit them yourself.";
/// Source `autocommit.FAILED_RUN_LEFT_UNCOMMITTED_WARNING`.
pub const FAILED_RUN_LEFT_UNCOMMITTED_WARNING: &str = "The upgrade failed, so its changes were left uncommitted — review them, then re-run `spec-kitty upgrade` or commit them yourself.";
/// Source `autocommit.METADATA_DIRTY_AT_BASELINE_WARNING`.
pub const METADATA_DIRTY_AT_BASELINE_WARNING: &str = ".kittify/metadata.yaml had uncommitted changes before the upgrade; its new schema stamp was left uncommitted — commit it yourself.";
/// Source `autocommit.HELD_FOR_REVIEW_NOT_COMMITTED_WARNING`.
pub const HELD_FOR_REVIEW_NOT_COMMITTED_WARNING: &str =
    "Held for manual review (not committed): {paths}";
/// Source `autocommit.HELD_FOR_REVIEW_SKIPPED_COMMIT_WARNING`.
pub const HELD_FOR_REVIEW_SKIPPED_COMMIT_WARNING: &str =
    "Skipped auto-commit of {count} file(s) held for manual review: {paths}";

/// `autocommit._METADATA_PATH`.
const METADATA_REL: &str = ".kittify/metadata.yaml";

/// The mission/lane/coordination branch prefix (`lanes.branch_naming._MISSION_PREFIX`).
const MISSION_BRANCH_PREFIX: &str = "kitty/mission-";

/// Why a `.worktrees/` entry is not upgraded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorktreeDisposition {
    /// Upgraded by this run (or, in a preview, upgraded by the described run).
    Upgrade,
    /// An integrating mission/lane/coordination checkout: it receives project-
    /// global state through integration, so nothing is written, stamped or
    /// committed there (#5457).
    SkipIntegrating,
    /// No `.kittify/` and no upgradeable mission state: nothing to do.
    SkipNoState,
}

impl WorktreeDisposition {
    /// The stable machine-readable name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Upgrade => "upgrade",
            Self::SkipIntegrating => "skip:integrating",
            Self::SkipNoState => "skip:no-state",
        }
    }
}

/// Whether `branch` names a mission, lane or coordination checkout.
///
/// Source `parse_mission_slug_from_branch(branch) is not None`: the grammar is
/// anchored on `kitty/mission-`, so the classification is exactly "the branch is
/// in the Mission namespace".
pub fn is_mission_branch(branch: &str) -> bool {
    branch.starts_with(MISSION_BRANCH_PREFIX) && branch.len() > MISSION_BRANCH_PREFIX.len()
}

/// One `.worktrees/` entry and why it takes part (or not).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorktreeRoot {
    /// The directory name (`Worktree <name>: …` in Source's messages).
    pub name: String,
    /// The checkout path.
    pub path: PathBuf,
    /// The disposition this run gives it.
    pub disposition: WorktreeDisposition,
    /// The branch checked out there, when it could be read.
    pub branch: Option<String>,
}

/// One root's preview.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorktreeRootPlan {
    /// The root.
    pub root: WorktreeRoot,
    /// The version recorded in that root's metadata (`unknown` when absent).
    pub from_version: String,
    /// The target this run reaches.
    pub to_version: String,
    /// The scoped migration ids this root would execute, in catalog order.
    pub scope: Vec<String>,
    /// The ids excluded by `runs_on_worktrees = false`, in catalog order.
    pub project_only: Vec<String>,
    /// Whether that root's `metadata.yaml` would be written.
    pub metadata_written: bool,
}

/// The commit policy a run actually applies.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CommitPolicy {
    /// Whether the per-root churn commit runs.
    pub auto_commit: bool,
    /// Why, in operator terms.
    pub reason: &'static str,
}

/// A preview of the whole fan-out.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorktreeUpgradePlan {
    /// The repository root checkout.
    pub project_root: PathBuf,
    /// Every discovered `.worktrees/` entry, in name order.
    pub roots: Vec<WorktreeRootPlan>,
    /// The commit policy the matching run applies.
    pub commit_policy: CommitPolicy,
}

/// What one root's execution did.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorktreeRootReport {
    /// The root.
    pub root: WorktreeRoot,
    /// The version the root started at.
    pub from_version: String,
    /// The target reached.
    pub to_version: String,
    /// Executed rows: `(id, result)` with `result` ∈ `applied`, `skipped`.
    pub rows: Vec<(String, String)>,
    /// Ids skipped because they are project-only (`runs_on_worktrees = false`).
    pub project_only: Vec<String>,
    /// Failures recorded at this root (they make the fan-out fail).
    pub failures: Vec<String>,
    /// Warnings, each prefixed with `Worktree <name>: ` like Source.
    pub warnings: Vec<String>,
    /// Whether this root's `metadata.yaml` was written.
    pub metadata_written: bool,
    /// A returned unsuccessful migration result (suppresses the schema stamp).
    pub worktree_failed: bool,
    /// A caught execution exception, independently sticky from failed results.
    pub worktree_errored: bool,
    /// Paths a migration held for manual review in this root (named, never
    /// committed, and never suppressing the rest of the commit set).
    pub held_paths: std::collections::BTreeSet<String>,
    /// Full migration payloads, including review paths and partial writes.
    pub migration_results: BTreeMap<String, sk_install::MigrationOutcome>,
    /// The churn commit, when one was attempted.
    pub commit: Option<CommitOutcome>,
}

impl Default for WorktreeRootReport {
    fn default() -> Self {
        Self {
            root: WorktreeRoot {
                name: String::new(),
                path: PathBuf::new(),
                disposition: WorktreeDisposition::SkipNoState,
                branch: None,
            },
            from_version: String::new(),
            to_version: String::new(),
            rows: Vec::new(),
            project_only: Vec::new(),
            failures: Vec::new(),
            warnings: Vec::new(),
            metadata_written: false,
            worktree_failed: false,
            worktree_errored: false,
            held_paths: std::collections::BTreeSet::new(),
            migration_results: BTreeMap::new(),
            commit: None,
        }
    }
}

impl WorktreeRootReport {
    /// Whether this root recorded any failure.
    pub fn failed(&self) -> bool {
        self.worktree_failed || self.worktree_errored || !self.failures.is_empty()
    }
}

/// The result of one churn commit attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommitOutcome {
    /// Whether a commit landed.
    pub committed: bool,
    /// The paths the commit set contains (checkout-relative POSIX).
    pub paths: Vec<String>,
    /// The commit id, when one landed.
    pub commit: Option<String>,
    /// The branch the commit landed on (or would have).
    pub branch: Option<String>,
    /// The one no-commit reason, when nothing landed.
    pub reason: Option<String>,
}

/// The whole fan-out's result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorktreeUpgradeReport {
    /// The repository root checkout.
    pub project_root: PathBuf,
    /// Every executed root, in name order.
    pub roots: Vec<WorktreeRootReport>,
    /// The commit policy that was applied.
    pub commit_policy: CommitPolicy,
}

impl WorktreeUpgradeReport {
    /// Whether every executed root succeeded.
    pub fn success(&self) -> bool {
        self.roots.iter().all(|root| !root.failed())
    }

    /// Every failure message, prefixed with its root name (the fatal channel).
    pub fn failures(&self) -> Vec<String> {
        self.roots
            .iter()
            .flat_map(|root| root.failures.iter().cloned())
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

/// Whether `worktree` integrates back into its mission's target branch (#5457).
///
/// Source `_is_integrating_worktree`: no `.git` entry means "not a git
/// worktree", so it keeps participating; an unreadable branch (detached HEAD)
/// fails safe as integrating; otherwise the Mission-namespace branch rule decides.
pub fn is_integrating_worktree(worktree: &Path) -> Result<bool, InstallError> {
    if !worktree.join(".git").exists() {
        return Ok(false);
    }
    let repository =
        GitRepository::open(worktree).map_err(|error| InstallError::UpgradeRefused {
            detail: format!(
                "cannot read the checkout at {}: {error}",
                worktree.display()
            ),
        })?;
    let branch = repository
        .current_branch()
        .map_err(|error| InstallError::UpgradeRefused {
            detail: format!("cannot read the branch of {}: {error}", worktree.display()),
        })?;
    Ok(branch.is_none_or(|branch| is_mission_branch(&branch)))
}

/// Every `.worktrees/` entry, in name order, classified (Source `_worktrees_to_upgrade`).
///
/// `worktree_scope` is the MAIN run's inherited selection filtered by
/// `runs_on_worktrees`; it decides the Mission-state half of "has upgradeable
/// state" and is never re-derived here.
pub fn discover_worktrees(
    project_root: &Path,
    worktree_scope: &[MigrationSpec],
) -> Result<Vec<WorktreeRoot>, InstallError> {
    let directory = project_root.join(WORKTREES_DIRNAME);
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    let mut entries: Vec<(String, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(&directory)
        .map_err(|error| InstallError::io("read directory", &directory, error))?
    {
        let entry =
            entry.map_err(|error| InstallError::io("read directory entry", &directory, error))?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| InstallError::io("inspect", &path, error))?;
        if !metadata.is_dir() {
            continue;
        }
        entries.push((entry.file_name().to_string_lossy().to_string(), path));
    }
    entries.sort();
    let mut roots = Vec::new();
    for (name, path) in entries {
        let branch = GitRepository::open(&path)
            .ok()
            .and_then(|repository| repository.current_branch().ok().flatten());
        let integrating = is_integrating_worktree(&path)?;
        let has_state = has_upgradeable_state(&path, worktree_scope);
        let disposition = if integrating {
            WorktreeDisposition::SkipIntegrating
        } else if has_state {
            WorktreeDisposition::Upgrade
        } else {
            WorktreeDisposition::SkipNoState
        };
        roots.push(WorktreeRoot {
            name,
            path,
            disposition,
            branch,
        });
    }
    Ok(roots)
}

/// Whether a checkout has state an upgrade pass can act on.
///
/// Source: `.kittify/` exists, or worktree-scoped migrations are in the inherited
/// scope and the checkout carries `kitty-specs/`/`.specify/`. An empty inherited
/// scope never synthesizes state from a Mission-looking tree.
fn has_upgradeable_state(worktree: &Path, worktree_scope: &[MigrationSpec]) -> bool {
    worktree_has_upgradeable_state(worktree, worktree_scope)
}

// ---------------------------------------------------------------------------
// Planning (preview)
// ---------------------------------------------------------------------------

/// The commit policy for a run.
///
/// Source gates the churn commit on the project's configured `auto_commit`
/// default and never on dry-run. This build's `UpgradeRequest` has no
/// `auto_commit` field yet, so the policy is reported as the *decided* value for
/// the requested run and the exact reason is carried in the report — never a
/// blanket "disabled" claim for a feature that exists.
fn commit_policy(project_root: &Path, dry_run: bool, auto_commit: bool) -> CommitPolicy {
    let _ = project_root;
    if dry_run {
        CommitPolicy {
            auto_commit: false,
            reason: "preview only: no commit is made by a dry run",
        }
    } else if auto_commit {
        CommitPolicy {
            auto_commit: true,
            reason: "auto-commit requested: each touched non-integrating worktree commits its churn",
        }
    } else {
        CommitPolicy {
            auto_commit: false,
            reason: "auto-commit not requested: worktree churn is left for the operator to commit",
        }
    }
}

/// The ids the inherited selection marks project-only, in catalogue order.
fn project_only_scope(inherited: &[MigrationSpec]) -> Vec<String> {
    inherited
        .iter()
        .filter(|spec| !spec.runs_on_worktrees)
        .map(|spec| spec.id.clone())
        .collect()
}

fn root_metadata(worktree: &Path) -> Option<ProjectMetadata> {
    ProjectMetadata::load(&worktree.join(".kittify"))
}

/// Read-only preview of the whole fan-out.
///
/// `inherited` is the MAIN run's selected migrations, in catalogue order
/// (Source `_upgrade` passes its own `migrations` list to `_upgrade_worktrees`).
/// The preview filters and evaluates that SAME inherited scope, so it can never
/// describe a per-root reselection the real run would not perform.
pub fn plan_worktree_upgrade(
    project_root: &Path,
    cli_version: &str,
    target: Option<&str>,
    auto_commit: bool,
    inherited: &[MigrationSpec],
) -> Result<WorktreeUpgradePlan, InstallError> {
    let to_version = target.unwrap_or(cli_version).to_string();
    let worktree_scope = worktree_migration_scope(inherited)?;
    let mut roots = Vec::new();
    // Source returns BEFORE discovery when the selection is non-empty but nothing
    // in it runs on worktrees: no sibling is examined, stamped or committed.
    if inherited.is_empty() || !worktree_scope.is_empty() {
        for root in discover_worktrees(project_root, &worktree_scope)? {
            if root.disposition != WorktreeDisposition::Upgrade {
                roots.push(WorktreeRootPlan {
                    root,
                    from_version: String::new(),
                    to_version: to_version.clone(),
                    scope: Vec::new(),
                    project_only: Vec::new(),
                    metadata_written: false,
                });
                continue;
            }
            let metadata = root_metadata(&root.path);
            let from_version = metadata
                .as_ref()
                .map(|metadata| metadata.version.clone())
                .unwrap_or_default();
            let pending = pending_worktree_migrations(&root.path, &worktree_scope)?;
            let scope: Vec<String> = pending.iter().map(|spec| spec.id.clone()).collect();
            // A dry run writes nothing; this is the prediction of what the run
            // would write (a settled target version and schema write nothing).
            let metadata_written = metadata.as_ref().is_some_and(|metadata| {
                metadata.version != to_version
                    || metadata.schema_version != Some(i64::from(CURRENT_SCHEMA_VERSION))
            }) || !pending.is_empty();
            roots.push(WorktreeRootPlan {
                from_version,
                to_version: to_version.clone(),
                scope,
                project_only: project_only_scope(inherited),
                metadata_written,
                root,
            });
        }
    }
    Ok(WorktreeUpgradePlan {
        project_root: project_root.to_path_buf(),
        roots,
        // The preview reports the policy the matching RUN applies, so it is
        // computed exactly as the run computes it (not as a "dry run").
        commit_policy: commit_policy(project_root, false, auto_commit),
    })
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

/// Execute the upgrade in every non-integrating `.worktrees/` checkout.
///
/// Each root is planned by [`sk_install::plan_upgrade`], executed under its
/// own [`ProjectLock`], recorded in its own ledger, and finally committed (when
/// the policy asks) through one `sk_git` safe commit. Files the run did not write
/// (already dirty at the pre-run baseline) are never swept into that commit.
pub fn run_worktree_upgrade(
    project_root: &Path,
    cli_version: &str,
    target: Option<&str>,
    auto_commit: bool,
    lock_wait: std::time::Duration,
    inherited: &[MigrationSpec],
) -> Result<WorktreeUpgradeReport, InstallError> {
    let to_version = target.unwrap_or(cli_version).to_string();
    let policy = commit_policy(project_root, false, auto_commit);
    let worktree_scope = worktree_migration_scope(inherited)?;
    let mut roots = Vec::new();
    // Source returns BEFORE discovery when the selection is non-empty but none of
    // it runs on worktrees: nothing is examined, stamped or committed anywhere.
    if !inherited.is_empty() && worktree_scope.is_empty() {
        return Ok(WorktreeUpgradeReport {
            project_root: project_root.to_path_buf(),
            roots,
            commit_policy: policy,
        });
    }
    let main_metadata = root_metadata(project_root);
    // The main checkout's already-settled stamp is the shared value a
    // bookkeeping-only worktree bump aligns to (#4972).
    let main_stamp = main_metadata
        .as_ref()
        .and_then(|metadata| metadata.last_upgraded_at.clone());
    // Minted once, before the loop: every bookkeeping-only version bump in this
    // run aligns to the same fallback stamp (#4972).
    let fallback_stamp = now_utc_iso();
    for root in discover_worktrees(project_root, &worktree_scope)? {
        if root.disposition != WorktreeDisposition::Upgrade {
            roots.push(WorktreeRootReport {
                root,
                to_version: to_version.clone(),
                ..WorktreeRootReport::default()
            });
            continue;
        }
        let baseline = if policy.auto_commit {
            capture_baseline(&root.path)
        } else {
            None
        };
        let prefix = format!("Worktree {}: ", root.name);
        // The canonical runner owns the per-root lock, the inherited-order step
        // evaluation, the ledger records and the metadata/schema reconciliation.
        let execution = match run_worktree_migrations(
            &root.path,
            cli_version,
            &to_version,
            &worktree_scope,
            false,
            main_stamp.as_deref(),
            &fallback_stamp,
            lock_wait,
        ) {
            Ok(execution) => execution,
            Err(error) => {
                // A root-level refusal (unreadable metadata, a selected id with no
                // native implementation, the root lock) is fatal for that root and
                // is never reported as a success or a silent skip.
                roots.push(WorktreeRootReport {
                    root,
                    to_version: to_version.clone(),
                    failures: vec![format!("{prefix}{error}")],
                    worktree_errored: true,
                    ..WorktreeRootReport::default()
                });
                continue;
            }
        };
        let mut report = WorktreeRootReport {
            root,
            from_version: execution.from_version,
            to_version: to_version.clone(),
            rows: execution
                .rows
                .into_iter()
                .map(|row| (row.id, row.result))
                .collect(),
            project_only: project_only_scope(inherited),
            failures: execution
                .failures
                .iter()
                .map(|failure| format!("{prefix}{failure}"))
                .collect(),
            warnings: execution
                .warnings
                .iter()
                .map(|warning| format!("{prefix}{warning}"))
                .collect(),
            metadata_written: execution.metadata_written,
            worktree_failed: execution.worktree_failed,
            worktree_errored: execution.worktree_errored,
            held_paths: execution.held_paths,
            migration_results: execution.migration_results,
            commit: None,
        };
        finish_root(&mut report, &policy, baseline, &prefix)?;
        roots.push(report);
    }
    Ok(WorktreeUpgradeReport {
        project_root: project_root.to_path_buf(),
        roots,
        commit_policy: policy,
    })
}

fn finish_root(
    report: &mut WorktreeRootReport,
    policy: &CommitPolicy,
    baseline: Option<BTreeSet<String>>,
    prefix: &str,
) -> Result<(), InstallError> {
    if !policy.auto_commit {
        return Ok(());
    }
    // Source `_commit_worktree_churn` checks the baseline first: without one the
    // run cannot know what it introduced, so it commits nothing and says so.
    let Some(baseline) = baseline else {
        report
            .warnings
            .push(format!("{prefix}{BASELINE_UNAVAILABLE_WARNING}"));
        return Ok(());
    };
    if report.failed() {
        // Source: a failed root's churn is never committed; the warning appears
        // only when the failed run actually left changes there.
        if !pending_commit_paths(&report.root.path, &baseline).is_empty() {
            report
                .warnings
                .push(format!("{prefix}{FAILED_RUN_LEFT_UNCOMMITTED_WARNING}"));
        }
        return Ok(());
    }
    let outcome = commit_touched_checkout(
        &report.root.path,
        &baseline,
        &report.from_version,
        &report.to_version,
    )?;
    if let Some(reason) = &outcome.reason {
        report.warnings.push(format!("{prefix}{reason}"));
    }
    if !report.held_paths.is_empty() {
        let joined = report
            .held_paths
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        let held = if outcome.committed {
            HELD_FOR_REVIEW_NOT_COMMITTED_WARNING.replace("{paths}", &joined)
        } else {
            HELD_FOR_REVIEW_SKIPPED_COMMIT_WARNING
                .replace("{count}", &report.held_paths.len().to_string())
                .replace("{paths}", &joined)
        };
        report.warnings.push(format!("{prefix}{held}"));
    }
    report.commit = Some(outcome);
    Ok(())
}

// ---------------------------------------------------------------------------
// Churn commit (Source `autocommit.py`)
// ---------------------------------------------------------------------------

/// The checkout's dirty paths before this run (Source `capture_upgrade_baseline`).
///
/// `None` means "could not read git status": the run then commits nothing and
/// says so, rather than reading a failed probe as a clean checkout.
pub fn capture_baseline(checkout: &Path) -> Option<BTreeSet<String>> {
    status_paths(checkout)
}

fn status_options() -> StatusOptions {
    StatusOptions {
        untracked: UntrackedMode::All,
        ignored: false,
    }
}

fn status_paths(checkout: &Path) -> Option<BTreeSet<String>> {
    let repository = GitRepository::open(checkout).ok()?;
    let entries = repository.status_with(&status_options()).ok()?;
    Some(
        entries
            .iter()
            .map(|entry| entry.path().display_lossy())
            .collect(),
    )
}

/// Paths the run introduced: dirty after minus dirty at baseline, eligible per
/// Source's one rule (the baseline is the ownership signal).
fn pending_commit_paths(checkout: &Path, baseline: &BTreeSet<String>) -> Vec<String> {
    let Some(current) = status_paths(checkout) else {
        return Vec::new();
    };
    current
        .into_iter()
        .filter(|path| !baseline.contains(path))
        .filter(|path| is_upgrade_commit_eligible(path, checkout))
        .collect()
}

/// Whether a newly changed path belongs in the upgrade commit.
///
/// Source `is_upgrade_commit_eligible`: never a path outside the checkout; in the
/// `$HOME` hazard case neither `.kittify/` nor root-level files. Path depth is
/// deliberately not a rule (the baseline decides ownership).
pub fn is_upgrade_commit_eligible(path: &str, checkout: &Path) -> bool {
    if path.is_empty() || path.starts_with("../") {
        return false;
    }
    let Some(home) = sk_kernel::paths::home_directory().ok() else {
        return true;
    };
    let resolved_checkout = checkout
        .canonicalize()
        .unwrap_or_else(|_| checkout.to_path_buf());
    let resolved_home = home.canonicalize().unwrap_or(home);
    if resolved_checkout == resolved_home {
        return !(path.starts_with(".kittify/") || !path.contains('/'));
    }
    true
}

/// Expand a status path into the concrete files git will stage.
///
/// `git status --porcelain` reports an untracked directory as one path; the commit
/// set needs the files inside it (Source `expand_upgrade_commit_path`).
pub fn expand_commit_path(checkout: &Path, relative: &str) -> Vec<String> {
    let absolute = checkout.join(relative);
    match std::fs::symlink_metadata(&absolute) {
        Ok(metadata) if metadata.is_dir() => {
            let mut files = Vec::new();
            collect_files(&absolute, checkout, &mut files);
            files.sort();
            files
        }
        _ => vec![relative.to_string()],
    }
}

fn collect_files(directory: &Path, checkout: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            collect_files(&path, checkout, out);
        } else if let Ok(relative) = path.strip_prefix(checkout) {
            out.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
}

/// Commit the churn a run introduced in `checkout`.
///
/// The commit set is the status diff against `baseline`, expanded to files, and
/// committed through the one `sk_git` safe commit with the upgrade-bookkeeping
/// capability. A detached HEAD or an unreadable branch never guesses a ref.
pub fn commit_touched_checkout(
    checkout: &Path,
    baseline: &BTreeSet<String>,
    from_version: &str,
    to_version: &str,
) -> Result<CommitOutcome, InstallError> {
    let mut paths: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for path in pending_commit_paths(checkout, baseline) {
        for expanded in expand_commit_path(checkout, &path) {
            if seen.insert(expanded.clone()) {
                paths.push(expanded);
            }
        }
    }
    if paths.is_empty() {
        return Ok(CommitOutcome {
            committed: false,
            paths,
            commit: None,
            branch: None,
            reason: None,
        });
    }
    let repository =
        GitRepository::open(checkout).map_err(|error| InstallError::UpgradeRefused {
            detail: format!("cannot open {}: {error}", checkout.display()),
        })?;
    let branch = repository
        .current_branch()
        .map_err(|error| InstallError::UpgradeRefused {
            detail: format!("cannot read the branch of {}: {error}", checkout.display()),
        })?;
    let Some(branch) = branch else {
        return Ok(CommitOutcome {
            committed: false,
            paths,
            commit: None,
            branch: None,
            reason: Some(DETACHED_HEAD_WARNING.to_string()),
        });
    };
    let Some(destination) = BranchName::parse(&branch).ok() else {
        return Ok(CommitOutcome {
            committed: false,
            paths,
            commit: None,
            branch: Some(branch),
            reason: Some(BRANCH_DETECTION_FAILED_WARNING.to_string()),
        });
    };
    let message =
        format!("chore: apply spec-kitty upgrade changes ({from_version} -> {to_version})");
    let git_paths: Vec<GitPath> = paths
        .iter()
        .filter_map(|path| GitPath::from_git_bytes(path.as_bytes()).ok())
        .collect();
    if git_paths.is_empty() {
        return Ok(CommitOutcome {
            committed: false,
            paths,
            commit: None,
            branch: Some(destination.as_str().to_string()),
            reason: Some(UPGRADE_COMMIT_SKIP_WARNING.to_string()),
        });
    }
    let request = SafeCommitRequest::new(destination, message, git_paths).with_protection(
        ProtectedBranches::default(),
        CommitGuardCapability::UpgradeBookkeeping,
    );
    let landed =
        repository
            .safe_commit(&request)
            .map_err(|error| InstallError::UpgradeRefused {
                detail: format!(
                    "cannot commit the upgrade churn in {}: {error}",
                    checkout.display()
                ),
            })?;
    let commit = landed.commit().as_str().to_string();
    let branch = landed.destination().as_str().to_string();
    let reason = if baseline.contains(METADATA_REL) {
        Some(METADATA_DIRTY_AT_BASELINE_WARNING.to_string())
    } else {
        None
    };
    Ok(CommitOutcome {
        committed: true,
        paths,
        commit: Some(commit),
        branch: Some(branch),
        reason,
    })
}

// ---------------------------------------------------------------------------
// Terminal Mission finalizer and surface repair
// ---------------------------------------------------------------------------

/// One artifact the finalizer rewrote.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FinalizerArtifact {
    /// Project-relative POSIX path.
    pub path: String,
    /// What changed.
    pub change: String,
}

/// What the terminal finalizer did.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FinalizerReport {
    /// Artifacts written.
    pub artifacts: Vec<FinalizerArtifact>,
    /// Warnings that did not fail the run.
    pub warnings: Vec<String>,
}

/// Finalize a project after an upgrade run: stamp the version and settle the
/// managed skill/command surfaces, through the canonical writers.
///
/// Source `upgrade/finalize.py` + the CLI's post-run surface repair: the terminal
/// step restores absent managed skill files and reports drift, and records the
/// version the run reached. It composes the existing installer and surface-repair
/// seams; it does not write status, event or metadata engines itself.
pub fn finalize_project(
    project_root: &Path,
    reached_version: &str,
    agent_keys: &[String],
    repair_drift: bool,
) -> Result<FinalizerReport, InstallError> {
    let mut report = FinalizerReport::default();
    let catalog =
        sk_install::skills::pack_skill::resolve_project_skill_catalog(project_root, true)?;
    if !agent_keys.is_empty() {
        let summary = sk_install::skills::run_surface_repair(
            project_root,
            agent_keys,
            &catalog,
            repair_drift,
        )?;
        for path in &summary.restored {
            report.artifacts.push(FinalizerArtifact {
                path: path.clone(),
                change: "restored managed skill file".to_string(),
            });
        }
        for path in &summary.drifted {
            report.warnings.push(format!(
                "Managed skill file {path} drifted from its recorded bytes; review and commit it"
            ));
        }
        report.warnings.extend(summary.warnings.iter().cloned());
    }
    let kittify = project_root.join(".kittify");
    if let Some(mut metadata) = ProjectMetadata::load(&kittify)
        && metadata.version != reached_version
    {
        metadata.version = reached_version.to_string();
        metadata.last_upgraded_at = Some(now_utc_iso());
        if metadata.save(&kittify)? {
            report.artifacts.push(FinalizerArtifact {
                path: ".kittify/metadata.yaml".to_string(),
                change: format!("version stamped {reached_version}"),
            });
        }
    }
    Ok(report)
}

/// One repair action the planner proposes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepairAction {
    /// Project-relative POSIX path.
    pub path: String,
    /// `restore`, `preserve` or `report-drift`.
    pub action: &'static str,
    /// Why.
    pub reason: String,
}

/// Plan the surface repair without writing anything.
///
/// Source's `doctor skills` repair preview: every manifest-owned managed skill
/// file is classified by what the catalog would do — restore an absent file,
/// report drift, or leave an unowned/unverifiable node alone. The plan is derived
/// from the same manifest and catalog the repair applies, so a preview can never
/// promise a write the repair will not make.
pub fn plan_surface_repair(
    project_root: &Path,
    agent_keys: &[String],
) -> Result<Vec<RepairAction>, InstallError> {
    let Some(manifest) = sk_install::ManagedSkillManifest::load_strict(project_root)? else {
        return Ok(Vec::new());
    };
    let catalog =
        sk_install::skills::pack_skill::resolve_project_skill_catalog(project_root, false)?;
    let mut actions = Vec::new();
    for entry in &manifest.entries {
        if !agent_keys.iter().any(|key| key == &entry.agent_key) {
            continue;
        }
        let destination = project_root.join(&entry.installed_path);
        match std::fs::symlink_metadata(&destination) {
            Ok(metadata) if metadata.file_type().is_file() => {
                let bytes = std::fs::read(&destination)
                    .map_err(|error| InstallError::io("read", &destination, error))?;
                let hash = sk_install::skills::pack_skill::file_digest(&bytes);
                if hash != entry.content_hash {
                    let restorable = catalog.skill(&entry.skill_name).is_some_and(|skill| {
                        skill
                            .files
                            .iter()
                            .any(|file| file.relative == entry.source_file)
                    });
                    actions.push(RepairAction {
                        path: entry.installed_path.clone(),
                        action: if restorable {
                            "report-drift"
                        } else {
                            "preserve"
                        },
                        reason: if restorable {
                            "bytes differ from the recorded hash; the catalog still ships this file"
                                .to_string()
                        } else {
                            "bytes differ and the catalog no longer ships this file; left in place"
                                .to_string()
                        },
                    });
                }
            }
            Ok(_) => actions.push(RepairAction {
                path: entry.installed_path.clone(),
                action: "preserve",
                reason: "not a regular file; repaired by hand".to_string(),
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let restorable = catalog.skill(&entry.skill_name).is_some_and(|skill| {
                    skill
                        .files
                        .iter()
                        .any(|file| file.relative == entry.source_file)
                });
                actions.push(RepairAction {
                    path: entry.installed_path.clone(),
                    action: if restorable { "restore" } else { "preserve" },
                    reason: if restorable {
                        "absent; the catalog ships this file".to_string()
                    } else {
                        "absent and the catalog no longer ships it; nothing to restore".to_string()
                    },
                });
            }
            Err(source) => return Err(InstallError::io("inspect", &destination, source)),
        }
    }
    Ok(actions)
}
