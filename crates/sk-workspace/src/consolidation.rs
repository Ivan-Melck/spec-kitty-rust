//! Lane consolidation: the Source two-step landing, real merge commits, the
//! recorded baseline, and conditional teardown.
//!
//! # The Source shape
//!
//! Source lands a lane mission in two steps and this engine keeps that shape:
//!
//! 1. **lane → mission branch.** Every lane tip is merged into the mission's
//!    integration branch (`lanes.json.mission_branch`) with a *real merge
//!    commit*. This step is strategy-independent upstream and here too.
//! 2. **mission branch → target.** The integration branch is landed on the
//!    target with the resolved strategy: `merge` creates a real merge commit;
//!    `squash` (Source's default) creates **one single-parent commit** whose
//!    tree is the integration branch's tree.
//!
//! Strategy precedence is Source's (`consolidation/config.py` +
//! `_resolve_effective_merge_strategy`): explicit `--strategy` > persisted
//! strategy on `--resume` > `.kittify/config.yaml` `merge.strategy` > `squash`.
//! An invalid config value is refused (never silently defaulted), and a resume
//! that contradicts the persisted strategy is refused as a strategy flip.
//!
//! # Why the integration branch is a temporary worktree
//!
//! Merges need a checkout of the destination. The target checkout belongs to
//! the operator and must stay on the target, so the integration branch is
//! materialised in `.worktrees/<slug>-integration`, and removed again when the
//! run finishes. A conflict there leaves the **target completely untouched**:
//! the half-merge is aborted in the temporary worktree and the run refuses with
//! the real conflict paths.
//!
//! # Squash without a second writer
//!
//! The squash commit is built by the existing pinned-parent safe-commit route:
//! the integration tip's changed paths are materialised into the clean target
//! checkout, then one commit is created from the *observed* target tip over
//! exactly those paths (deletions included), and the branch is advanced by
//! compare-and-swap. Afterwards the resulting tree is compared with the
//! integration branch tree path by path; a mismatch refuses instead of claiming
//! a landing.
//!
//! # What is deliberately not invented
//!
//! * `rebase` is refused: replaying the mission commits needs a re-player the
//!   adapter does not have. The refusal names that gap; it is not a silent
//!   different landing.
//! * Nothing rewinds a landed target. `--abort` clears only a run that provably
//!   did not move the target.
//! * Readiness (work-package lanes) is supplied by the caller from the canonical
//!   status reducer; this crate has no status writer and does not guess.

pub mod state;

use std::path::{Path, PathBuf};

use sk_git::{
    BranchName, GitError, GitPath, GitRepository, MergeNoFfStatus, MergeStartStatus, Oid,
    ProtectedBranches, RebaseStatus, SquashMergeStatus, StartPoint, WorktreeDiscardPolicy,
    WorktreeStart,
};
use sk_mission::Mission;

use crate::allocation::LaneAllocator;
use crate::coord::{CoordJournal, CoordPhase, CoordRecord, CoordRecordKind};
use crate::error::WorkspaceError;
use crate::lanes::{ExecutionLane, LanesManifest};
use crate::protection::ProtectionPolicy;
use crate::workspace::MissionWorkspace;

/// The mission→target branch-integration strategy (Source `MergeStrategy`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MergeStrategy {
    /// A real merge commit (`merge`).
    Merge,
    /// One squashed single-parent commit (`squash`, Source's default).
    Squash,
    /// Replay the mission commits onto the target (`rebase`).
    Rebase,
}

impl MergeStrategy {
    /// The wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::Squash => "squash",
            Self::Rebase => "rebase",
        }
    }

    /// Parse the CLI/config spelling.
    pub fn parse(value: &str) -> Result<Self, ConsolidationError> {
        match value {
            "merge" => Ok(Self::Merge),
            "squash" => Ok(Self::Squash),
            "rebase" => Ok(Self::Rebase),
            other => Err(refused(
                "MERGE_STRATEGY_INVALID",
                format!(
                    "unknown merge strategy {other:?}: allowed values are merge, rebase, squash"
                ),
            )),
        }
    }

    /// Source's documented default.
    pub fn default_source() -> Self {
        Self::Squash
    }
}

/// Source `_resolve_effective_merge_strategy` precedence.
///
/// Fresh run: `explicit > config > squash`. Resume:
/// `explicit > persisted > config > squash`, and an explicit value that
/// contradicts the persisted one is a strategy flip, refused rather than
/// honoured.
pub fn resolve_effective_strategy(
    explicit: Option<MergeStrategy>,
    persisted: Option<MergeStrategy>,
    config: Option<MergeStrategy>,
    is_resume: bool,
) -> Result<MergeStrategy, ConsolidationError> {
    if is_resume {
        if let (Some(explicit), Some(persisted)) = (explicit, persisted)
            && explicit != persisted
        {
            return Err(refused(
                "MERGE_STRATEGY_FLIP",
                format!(
                    "--resume carries --strategy {} but the interrupted run persisted {}; a resumed merge must run the same strategy it started with",
                    explicit.as_str(),
                    persisted.as_str()
                ),
            ));
        }
        return Ok(explicit
            .or(persisted)
            .or(config)
            .unwrap_or_else(MergeStrategy::default_source));
    }
    Ok(explicit
        .or(config)
        .unwrap_or_else(MergeStrategy::default_source))
}

/// Read `merge.strategy` from `.kittify/config.yaml` (Source `load_merge_config`).
///
/// An absent config, an absent `merge` block, or an absent key is `None`; a
/// present but unreadable/undecodable config degrades to `None` (Source does the
/// same, so an unrelated YAML problem never blocks merging); a present but
/// invalid `merge.strategy` is a refusal.
pub fn load_merge_config(root: &Path) -> Result<Option<MergeStrategy>, ConsolidationError> {
    let path = root.join(".kittify/config.yaml");
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(refused(
                "MERGE_CONFIG_UNREADABLE",
                format!("could not read {}: {error}", path.display()),
            ));
        }
    };
    let Ok(value) = serde_yaml_ng::from_slice::<serde_yaml_ng::Value>(&bytes) else {
        return Ok(None);
    };
    let Some(merge) = value.get("merge").filter(|value| value.is_mapping()) else {
        return Ok(None);
    };
    let Some(raw) = merge.get("strategy").filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let text = match raw {
        serde_yaml_ng::Value::String(text) => text.clone(),
        other => format!("{other:?}"),
    };
    if !matches!(text.as_str(), "merge" | "squash" | "rebase") {
        return Err(refused(
            "MERGE_STRATEGY_INVALID",
            format!(
                "Invalid merge.strategy {text:?} in .kittify/config.yaml. Allowed values: merge, rebase, squash. No silent fallback — fix the config before merging."
            ),
        ));
    }
    MergeStrategy::parse(&text).map(Some)
}

/// One consolidation request.
#[derive(Clone, Debug, Default)]
pub struct ConsolidationRequest {
    /// Explicit `--target` override (defaults to the mission's target branch).
    pub target_branch: Option<String>,
    /// Explicit `--strategy` (Source resolves config then `squash`).
    pub strategy: Option<MergeStrategy>,
    /// `Some(true)`/`Some(false)` from `--delete-branch`/`--keep-branch`.
    pub delete_branch: Option<bool>,
    /// `Some(true)`/`Some(false)` from `--remove-worktree`/`--keep-worktree`.
    pub remove_worktree: Option<bool>,
    /// Plan only.
    pub dry_run: bool,
    /// Resume an interrupted run.
    pub resume: bool,
    /// Abort a run that provably did not move the target.
    pub abort: bool,
    /// Complete a mission whose lanes manifest is genuinely absent.
    pub skip_lanes: bool,
}

/// One lane's landing result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaneLanding {
    /// Lane id.
    pub lane_id: String,
    /// Lane branch.
    pub branch: String,
    /// `merged` | `already_integrated` | `skipped` | `planned`.
    pub outcome: String,
    /// The commit this lane's tip contributed, when it resolved.
    pub tip: Option<Oid>,
    /// The merge commit this lane created on the integration branch.
    pub merge_commit: Option<Oid>,
    /// Why a lane was skipped, when it was.
    pub reason: Option<String>,
}

/// One teardown action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TeardownAction {
    /// `worktree_removed` | `branch_deleted` | `worktree_retained` |
    /// `branch_retained` | `integration_worktree_removed` |
    /// `integration_branch_deleted` | `integration_branch_retained`.
    pub action: String,
    /// Lane id, branch name, or path.
    pub target: String,
    /// Why it was retained, when it was.
    pub reason: Option<String>,
}

/// A completed consolidation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConsolidationReport {
    /// Mission slug.
    pub mission_slug: String,
    /// The branch lanes landed on.
    pub target_branch: String,
    /// The strategy that ran.
    pub strategy: &'static str,
    /// The target tip observed before the landing.
    pub pre_target_tip: Option<Oid>,
    /// The recorded baseline (the target tip after the landing).
    pub baseline_commit: Oid,
    /// The mission integration branch the lanes were merged into.
    pub integration_branch: String,
    /// Whether this run created the integration branch.
    pub integration_created: bool,
    /// The integration branch tip after the lane merges.
    pub integration_commit: Option<Oid>,
    /// Lane results in merge order.
    pub lanes: Vec<LaneLanding>,
    /// Teardown actions (empty on a dry run).
    pub teardown: Vec<TeardownAction>,
    /// The path of the recorded baseline in `meta.json`.
    pub baseline_path: PathBuf,
    /// The coordination journal the run recorded into.
    pub journal_path: PathBuf,
    /// A journal fault recorded *after* the outcome was proven.
    pub journal_fault: Option<String>,
    /// True when this run only verified an already-recorded consolidation.
    pub already_consolidated: bool,
    /// True when the request was a dry run.
    pub dry_run: bool,
    /// The bookkeeping commit that carries the updated `meta.json`, when one
    /// was created (never the recorded baseline itself).
    pub metadata_commit: Option<Oid>,
}

impl ConsolidationReport {
    /// The `consolidate --json` payload.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "mission_slug": self.mission_slug,
            "target_branch": self.target_branch,
            "strategy": self.strategy,
            "pre_target_tip": self.pre_target_tip.as_ref().map(Oid::to_string),
            "baseline_commit": self.baseline_commit.to_string(),
            "integration_branch": self.integration_branch,
            "integration_created": self.integration_created,
            "integration_commit": self.integration_commit.as_ref().map(Oid::to_string),
            "lanes": self.lanes.iter().map(|lane| serde_json::json!({
                "lane_id": lane.lane_id,
                "branch": lane.branch,
                "outcome": lane.outcome,
                "tip": lane.tip.as_ref().map(Oid::to_string),
                "merge_commit": lane.merge_commit.as_ref().map(Oid::to_string),
                "reason": lane.reason,
            })).collect::<Vec<_>>(),
            "teardown": self.teardown.iter().map(|action| serde_json::json!({
                "action": action.action,
                "target": action.target,
                "reason": action.reason,
            })).collect::<Vec<_>>(),
            "baseline_path": self.baseline_path.to_string_lossy(),
            "journal_path": self.journal_path.to_string_lossy(),
            "journal_fault": self.journal_fault,
            "already_consolidated": self.already_consolidated,
            "dry_run": self.dry_run,
            "metadata_commit": self.metadata_commit.as_ref().map(Oid::to_string),
        })
    }
}

/// Consolidation refusals and failures.
#[derive(Debug)]
pub enum ConsolidationError {
    /// A precondition refused before (or instead of) a mutation.
    Refused {
        /// Stable error code.
        code: &'static str,
        /// Operator-facing detail naming what was observed.
        message: String,
    },
    /// The Mission metadata could not be read or updated.
    Mission(sk_mission::MissionError),
    /// A workspace (lane/ownership) operation failed.
    Workspace(Box<WorkspaceError>),
    /// A Git operation failed.
    Git(GitError),
    /// A journal or filesystem operation failed.
    Io(String),
}

impl std::fmt::Display for ConsolidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused { code, message } => write!(formatter, "{code}: {message}"),
            Self::Mission(error) => write!(formatter, "{error}"),
            Self::Workspace(error) => write!(formatter, "{error}"),
            Self::Git(error) => write!(formatter, "{error}"),
            Self::Io(detail) => formatter.write_str(detail),
        }
    }
}

impl std::error::Error for ConsolidationError {}

impl From<GitError> for ConsolidationError {
    fn from(error: GitError) -> Self {
        Self::Git(error)
    }
}

impl From<WorkspaceError> for ConsolidationError {
    fn from(error: WorkspaceError) -> Self {
        Self::Workspace(Box::new(error))
    }
}

impl From<sk_mission::MissionError> for ConsolidationError {
    fn from(error: sk_mission::MissionError) -> Self {
        Self::Mission(error)
    }
}

fn refused(code: &'static str, message: impl Into<String>) -> ConsolidationError {
    ConsolidationError::Refused {
        code,
        message: message.into(),
    }
}

/// Is `tip` reachable from `target` (or equal to it)?
fn integrated(repository: &GitRepository, tip: &Oid, target: &Oid) -> Result<bool, GitError> {
    if tip == target {
        return Ok(true);
    }
    repository.is_ancestor(tip, target)
}

/// The outcome of Source's mission-number bake, decided BEFORE the target moves.
///
/// Source `consolidation/mission_number/bake.py::_bake_mission_number_into_mission_branch`:
/// the number is computed from the TARGET branch's view, baked as its OWN commit
/// on the MISSION branch, and returned so the post-landing projection records it
/// on the target. A skipped or failed mission-branch write never discards the
/// number (`#4900`) - the target projection is the authority.
#[derive(Debug, Clone)]
pub struct MissionNumberAssignment {
    /// The number this run records on the target.
    pub number: i64,
    /// The mission-branch tip AFTER the bake commit, when one was written.
    pub branch_tip: Option<Oid>,
    /// Whether THIS call created the bake commit.
    pub baked: bool,
    /// Source's surfaced reason when the mission-branch write was skipped.
    pub note: Option<String>,
    /// The exact `meta.json` bytes this call wrote, captured from the real write.
    ///
    /// Present only when THIS call wrote the mission-branch `meta.json`; a bake
    /// skipped by idempotency (or by a failure before the write) has none. The
    /// capture is the compare-and-swap evidence a refused bookkeeping commit needs
    /// to undo its own bytes without erasing another writer's fields.
    pub meta_capture: Option<crate::planning::write_ledger::MetaBakeCapture>,
}

fn mission_number_assignment(
    number: i64,
    branch_tip: Option<Oid>,
    baked: bool,
    note: Option<String>,
) -> MissionNumberAssignment {
    MissionNumberAssignment {
        number,
        branch_tip,
        baked,
        note,
        meta_capture: None,
    }
}

fn mission_number_assignment_captured(
    number: i64,
    branch_tip: Oid,
    note: Option<String>,
    capture: crate::planning::write_ledger::MetaBakeCapture,
) -> MissionNumberAssignment {
    MissionNumberAssignment {
        number,
        branch_tip: Some(branch_tip),
        baked: true,
        note,
        meta_capture: Some(capture),
    }
}

/// Read an assigned (positive integer) `mission_number` from a `meta.json`.
fn read_assigned_number(path: &Path) -> Option<i64> {
    let bytes = std::fs::read(path).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    match value.get("mission_number")? {
        serde_json::Value::Number(number) => number.as_i64().filter(|value| *value >= 1),
        serde_json::Value::String(text) => {
            text.trim().parse::<i64>().ok().filter(|value| *value >= 1)
        }
        _ => None,
    }
}

/// Source `assign_next_mission_number`: max over the target view's missions + 1.
fn next_mission_number(specs_dir: &Path) -> i64 {
    let Ok(entries) = std::fs::read_dir(specs_dir) else {
        return 1;
    };
    let mut highest = 0;
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        if let Some(number) = read_assigned_number(&entry.path().join("meta.json")) {
            highest = highest.max(number);
        }
    }
    highest + 1
}

/// Source `_bake_mission_number_into_mission_branch` steps 1-2.
///
/// 1. Read the TARGET branch's view: when this mission already carries an
///    assigned number there, the bake is a no-op and that number is returned.
/// 2. Otherwise compute `max+1` from that view and write it as its OWN commit on
///    the MISSION branch: a detached scratch checkout of the mission branch, the
///    single `meta.json` field, a commit, and a fast-forward of the branch ref
///    (Source's silent `advance_branch_ref`).
///
/// Idempotent on retry: once the mission branch already carries exactly that
/// number, no second commit is created (Source's `_locked_assign_mission_number`
/// idempotency hit), so a failed landing followed by a retry never duplicates
/// the bake.
pub fn bake_mission_number_into_branch(
    repository: &GitRepository,
    slug: &str,
    mission_branch: &BranchName,
    target_branch: &BranchName,
) -> Result<MissionNumberAssignment, ConsolidationError> {
    let primary_root = repository.root().to_path_buf();
    let specs = Path::new("kitty-specs");

    // ---- step 1: the TARGET branch's view (its own detached checkout) ----
    let number = {
        let scan_root = primary_root
            .join(".worktrees")
            .join(format!("{slug}-numscan-{}", std::process::id()));
        let scan_head = repository
            .add_detached_worktree(
                &scan_root,
                &sk_git::StartPoint::Ref(target_branch.as_ref_name()),
            )
            .map_err(ConsolidationError::Git)?;
        let target_meta = scan_root.join(specs).join(slug).join("meta.json");
        let existing = read_assigned_number(&target_meta);
        let number = match existing {
            Some(number) => Some(number),
            None => Some(next_mission_number(&scan_root.join(specs))),
        };
        let cleanup = repository
            .remove_scratch_worktree(&scan_root, &scan_head)
            .err();
        if let Some(number) = existing {
            // The target already carries the number: the bake is a no-op.
            let _ = cleanup;
            return Ok(mission_number_assignment(number, None, false, None));
        }
        let number = number.expect("the scan produced a number");
        if let Some(error) = cleanup {
            return Ok(mission_number_assignment(
                number,
                None,
                false,
                Some(format!("the scan checkout could not be removed: {error}")),
            ));
        }
        number
    };

    // ---- step 2: the mission-branch bake ----
    let Some(old_tip) = repository
        .read_branch_tip(mission_branch)
        .map_err(ConsolidationError::Git)?
    else {
        return Ok(mission_number_assignment(
            number,
            None,
            false,
            Some(format!("branch {mission_branch} does not exist")),
        ));
    };
    match bake_number_on_branch(
        repository,
        &primary_root,
        slug,
        mission_branch,
        &old_tip,
        number,
    ) {
        Ok(Some((commit, capture))) => Ok(mission_number_assignment_captured(
            number, commit, None, capture,
        )),
        Ok(None) => Ok(mission_number_assignment(
            number,
            Some(old_tip),
            false,
            Some("the mission branch already carries this number".to_owned()),
        )),
        Err(note) => Ok(mission_number_assignment(number, None, false, Some(note))),
    }
}

/// The single `meta.json` commit on a detached mission-branch checkout.
///
/// `Ok(None)` is Source's idempotency hit; `Err(note)` is a skipped/failed bake
/// that leaves the branch untouched (the number still flows to the target). The
/// `Ok(Some((commit, capture)))` arm carries the exact bytes the bake wrote —
/// captured from the real write, not re-rendered — so a caller can compare-and-swap
/// an undo without erasing a concurrent field.
fn bake_number_on_branch(
    repository: &GitRepository,
    primary_root: &Path,
    slug: &str,
    mission_branch: &BranchName,
    old_tip: &Oid,
    number: i64,
) -> Result<Option<(Oid, crate::planning::write_ledger::MetaBakeCapture)>, String> {
    let scratch_root = primary_root
        .join(".worktrees")
        .join(format!("{slug}-numbake-{}", std::process::id()));
    let head = repository
        .add_detached_worktree(
            &scratch_root,
            &sk_git::StartPoint::Ref(mission_branch.as_ref_name()),
        )
        .map_err(|error| format!("could not create the mission-branch checkout: {error}"))?;
    let result = (|| {
        let scratch = GitRepository::open(&scratch_root)
            .map_err(|error| format!("could not open the mission-branch checkout: {error}"))?;
        let feature_dir = scratch_root.join("kitty-specs").join(slug);
        let meta_path = feature_dir.join("meta.json");
        if !meta_path.exists() {
            return Ok(None);
        }
        if read_assigned_number(&meta_path) == Some(number) {
            return Ok(None);
        }
        let capture = crate::planning::write_ledger::bake_mission_number(&feature_dir, number)
            .map_err(|error| format!("could not record mission_number in meta.json: {error}"))?;
        let relative = format!("kitty-specs/{slug}/meta.json");
        let path = sk_git::GitPath::from_git_bytes(relative.as_bytes())
            .map_err(|error| format!("{relative} is not a usable git path: {error}"))?;
        scratch
            .stage_path(&path)
            .map_err(|error| format!("could not stage {relative}: {error}"))?;
        let message = format!("chore({slug}): assign mission_number={number}");
        let commit = scratch
            .commit_staged_merge(&head, &message)
            .map_err(|error| format!("could not commit the mission_number bake: {error}"))?;
        Ok(Some((commit, capture)))
    })();
    let commit_for_removal = match &result {
        Ok(Some((commit, _capture))) => commit.clone(),
        _ => head.clone(),
    };
    let cleanup = repository
        .remove_scratch_worktree(&scratch_root, &commit_for_removal)
        .err()
        .map(|error| format!("the mission_number checkout could not be removed: {error}"));
    match (result, cleanup) {
        (Ok(Some((commit, capture))), None) => repository
            .advance_branch_ref(mission_branch, old_tip, &commit)
            .map_err(|error| format!("could not fast-forward {mission_branch}: {error}"))
            .map(|_| Some((commit, capture))),
        // A failed advance leaves the branch where it was (the commit is
        // unreachable); the run continues with the number.
        (Ok(Some(_)), Some(cleanup)) => Err(cleanup),
        (Ok(None), None) => Ok(None),
        (Ok(None), Some(cleanup)) => Err(cleanup),
        (Err(error), _) => Err(error),
    }
}

/// Whether `lane`'s payload already landed on `integration` — Source's
/// `lane_integrated_by_tree_or_ancestry`: no commits absent from the integration
/// branch, OR the two refs expose the same tree right now.
fn lane_integrated(
    repository: &GitRepository,
    lane: &BranchName,
    integration: &BranchName,
) -> Result<bool, GitError> {
    let lane_ref = lane.as_ref_name();
    let integration_ref = integration.as_ref_name();
    let Some(lane_tip) = repository.read_ref(&lane_ref)? else {
        // A lane branch that does not exist has nothing to integrate.
        return Ok(true);
    };
    if let Some(integration_tip) = repository.read_ref(&integration_ref)?
        && integrated(repository, &lane_tip, &integration_tip)?
    {
        return Ok(true);
    }
    repository.trees_equal(&lane_ref, &integration_ref)
}

/// Every code lane in canonical `(parallel_group, lane_id)` order.
fn code_lanes(manifest: &LanesManifest) -> Vec<&ExecutionLane> {
    let mut lanes: Vec<&ExecutionLane> = manifest
        .lanes
        .iter()
        .filter(|lane| !lane.is_planning())
        .collect();
    lanes.sort_by(|left, right| {
        (left.parallel_group, left.lane_id.as_str())
            .cmp(&(right.parallel_group, right.lane_id.as_str()))
    });
    lanes
}

/// Read a boolean retention flag from `meta.json`.
fn retention_flag(mission: &Mission, key: &str) -> Option<bool> {
    mission.meta_field(key).and_then(serde_json::Value::as_bool)
}

/// The temporary worktree path the integration branch is materialised in.
fn integration_worktree_path(primary_root: &Path, slug: &str) -> PathBuf {
    primary_root
        .join(".worktrees")
        .join(format!("{slug}-integration"))
}

/// The persisted strategy of an interrupted run, read from the journal.
fn persisted_strategy(journal: &CoordJournal, slug: &str) -> Option<MergeStrategy> {
    let records = journal.records().ok()?;
    records
        .iter()
        .rev()
        .filter(|record| {
            record.kind == CoordRecordKind::Integration
                && record.operation == "consolidate_merge"
                && record.mission_slug == slug
        })
        .find_map(|record| {
            record.detail.as_deref().and_then(|detail| {
                detail
                    .split_whitespace()
                    .find_map(|token| token.strip_prefix("strategy="))
                    .and_then(|value| MergeStrategy::parse(value).ok())
            })
        })
}

/// Land every lane tip of `workspace`'s Mission on its target branch.
pub fn consolidate_mission(
    workspace: &MissionWorkspace,
    work_package_lanes: &[(String, String)],
    request: &ConsolidationRequest,
) -> Result<ConsolidationReport, ConsolidationError> {
    let mission = workspace.mission();
    let slug = mission.slug().as_str().to_owned();
    let primary_root = workspace.workspace().primary_root().to_path_buf();
    let common_dir = workspace.workspace().common_dir().to_path_buf();
    let repository = GitRepository::open(&primary_root)?;
    let journal = CoordJournal::new(&common_dir, &slug);

    if request.abort {
        return abort_consolidation(workspace, &journal);
    }

    let target_branch_name = match &request.target_branch {
        Some(value) => value.clone(),
        None => workspace.target_branch()?,
    };
    let target_branch = BranchName::parse(&target_branch_name).map_err(|error| {
        refused(
            "DESTINATION_REF_INVALID",
            format!("target branch {target_branch_name:?} is not usable: {error}"),
        )
    })?;
    let target_tip = repository.read_branch_tip(&target_branch)?.ok_or_else(|| {
        refused(
            "CONSOLIDATION_TARGET_MISSING",
            format!("target branch {target_branch} does not exist in this repository"),
        )
    })?;
    let baseline_path = mission.spec_dir().join(sk_mission::META_FILE_NAME);

    // A recorded baseline means the landing already happened.
    if let Some(recorded) = mission
        .meta_field("baseline_merge_commit")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let recorded_oid = Oid::parse(recorded).map_err(|error| {
            refused(
                "CONSOLIDATION_BASELINE_INVALID",
                format!("meta.json baseline_merge_commit {recorded:?} is not a commit id: {error}"),
            )
        })?;
        if request.resume {
            let manifest = load_manifest(workspace, request)?;
            let unlanded: Vec<String> = code_lanes(&manifest)
                .into_iter()
                .filter_map(|lane| {
                    let branch = crate::naming::lane_branch_name(
                        &manifest.mission_slug,
                        &lane.lane_id,
                        &manifest.target_branch,
                    )
                    .ok()?;
                    let name = BranchName::parse(&branch).ok()?;
                    let tip = repository.read_branch_tip(&name).ok().flatten()?;
                    match integrated(&repository, &tip, &target_tip) {
                        Ok(true) => None,
                        _ => Some(lane.lane_id.clone()),
                    }
                })
                .collect();
            if !unlanded.is_empty() {
                return Err(refused(
                    "CONSOLIDATION_RESUME_UNLANDED_LANES",
                    format!(
                        "baseline {recorded_oid} is recorded but these lane tips are not reachable from {target_branch}: {}. The recorded baseline is preserved; re-run without `--resume` only after confirming the landed history.",
                        unlanded.join(", ")
                    ),
                ));
            }
            return Ok(ConsolidationReport {
                mission_slug: slug,
                target_branch: target_branch.as_str().to_owned(),
                strategy: request
                    .strategy
                    .unwrap_or_else(MergeStrategy::default_source)
                    .as_str(),
                pre_target_tip: Some(target_tip.clone()),
                baseline_commit: recorded_oid,
                integration_branch: manifest.mission_branch.clone(),
                integration_created: false,
                integration_commit: None,
                lanes: Vec::new(),
                teardown: Vec::new(),
                baseline_path,
                journal_path: journal.path().to_path_buf(),
                journal_fault: None,
                already_consolidated: true,
                dry_run: request.dry_run,
                metadata_commit: None,
            });
        }
        return Err(refused(
            "MISSION_ALREADY_CONSOLIDATED",
            format!(
                "mission {slug} already records baseline_merge_commit {recorded_oid}; use `--resume` to verify a resumed run or `spec-kitty review` to audit the merged mission"
            ),
        ));
    }

    // Strategy resolution (explicit > persisted-on-resume > config > squash).
    let config = load_merge_config(&primary_root)?;
    let persisted = request
        .resume
        .then(|| persisted_strategy(&journal, &slug))
        .flatten();
    let strategy = resolve_effective_strategy(request.strategy, persisted, config, request.resume)?;

    // Merge readiness: every tracked work package must be terminal.
    if work_package_lanes.is_empty() {
        return Err(refused(
            "MISSION_NOT_MERGE_READY",
            "no work package is tracked in the status snapshot; nothing is ready to merge",
        ));
    }
    let non_terminal: Vec<String> = work_package_lanes
        .iter()
        .filter(|(_, lane)| !matches!(lane.as_str(), "approved" | "done"))
        .map(|(wp, lane)| format!("{wp}={lane}"))
        .collect();
    if !non_terminal.is_empty() {
        return Err(refused(
            "MISSION_NOT_MERGE_READY",
            format!(
                "not merge-ready: {} (every work package must be approved or done before consolidation)",
                non_terminal.join(", ")
            ),
        ));
    }

    let manifest = load_manifest(workspace, request)?;
    let integration_branch_name = manifest.mission_branch.clone();
    let integration_branch = BranchName::parse(&integration_branch_name).map_err(|error| {
        refused(
            "DESTINATION_REF_INVALID",
            format!(
                "mission integration branch {integration_branch_name:?} is not usable: {error}"
            ),
        )
    })?;
    if integration_branch == target_branch {
        return Err(refused(
            "CONSOLIDATION_INTEGRATION_BRANCH_INVALID",
            format!(
                "lanes.json records mission_branch {} which is the target branch itself; a lane mission needs a separate integration branch",
                integration_branch
            ),
        ));
    }

    // The target must be the checkout this run lands on.
    let observed = repository.current_branch()?;
    if observed.as_deref() != Some(target_branch.as_str()) {
        return Err(refused(
            "CONSOLIDATION_CHECKOUT_REQUIRED",
            format!(
                "consolidation lands on {target_branch}, but {} has {:?} checked out; check out {target_branch} in the primary checkout and retry",
                primary_root.display(),
                observed.unwrap_or_else(|| "<detached>".to_owned())
            ),
        ));
    }
    // Untracked paths (lane worktree registrations, operator scratch files) do
    // not block a merge; a tracked modification does, because the landing would
    // put it at risk.
    let dirty: Vec<String> = repository
        .status()?
        .into_iter()
        .filter(|entry| !entry.is_untracked())
        .map(|entry| entry.path().display_lossy())
        .collect();
    if !dirty.is_empty() {
        return Err(refused(
            "CONSOLIDATION_DIRTY_CHECKOUT",
            format!(
                "the primary checkout holds local changes: {}",
                dirty.join(", ")
            ),
        ));
    }

    // Resolve every lane tip before any mutation.
    let mut pending: Vec<(String, BranchName, Oid)> = Vec::new();
    let mut lanes: Vec<LaneLanding> = Vec::new();
    for lane in code_lanes(&manifest) {
        let branch_name = crate::naming::lane_branch_name(
            &manifest.mission_slug,
            &lane.lane_id,
            &manifest.target_branch,
        )?;
        let branch = match BranchName::parse(&branch_name) {
            Ok(branch) => branch,
            Err(error) => {
                lanes.push(LaneLanding {
                    lane_id: lane.lane_id.clone(),
                    branch: branch_name,
                    outcome: "skipped".to_owned(),
                    tip: None,
                    merge_commit: None,
                    reason: Some(format!("lane branch is not usable: {error}")),
                });
                continue;
            }
        };
        let Some(tip) = repository.read_branch_tip(&branch)? else {
            lanes.push(LaneLanding {
                lane_id: lane.lane_id.clone(),
                branch: branch.as_str().to_owned(),
                outcome: "skipped".to_owned(),
                tip: None,
                merge_commit: None,
                reason: Some(format!(
                    "lane branch {} does not resolve; it may have been merged and deleted",
                    branch.as_str()
                )),
            });
            continue;
        };
        if integrated(&repository, &tip, &target_tip)? {
            lanes.push(LaneLanding {
                lane_id: lane.lane_id.clone(),
                branch: branch.as_str().to_owned(),
                outcome: "already_integrated".to_owned(),
                tip: Some(tip),
                merge_commit: None,
                reason: None,
            });
            continue;
        }
        lanes.push(LaneLanding {
            lane_id: lane.lane_id.clone(),
            branch: branch.as_str().to_owned(),
            outcome: if request.dry_run { "planned" } else { "merged" }.to_owned(),
            tip: Some(tip.clone()),
            merge_commit: None,
            reason: None,
        });
        pending.push((lane.lane_id.clone(), branch, tip));
    }

    if request.dry_run {
        let baseline_commit = pending
            .last()
            .map(|(_, _, tip)| tip.clone())
            .unwrap_or_else(|| target_tip.clone());
        return Ok(ConsolidationReport {
            mission_slug: slug,
            target_branch: target_branch.as_str().to_owned(),
            strategy: strategy.as_str(),
            pre_target_tip: Some(target_tip),
            baseline_commit,
            integration_branch: integration_branch.as_str().to_owned(),
            integration_created: repository.read_branch_tip(&integration_branch)?.is_none(),
            integration_commit: None,
            lanes,
            teardown: Vec::new(),
            baseline_path,
            journal_path: journal.path().to_path_buf(),
            journal_fault: None,
            already_consolidated: false,
            dry_run: true,
            metadata_commit: None,
        });
    }

    // Journal the run before the first mutation.
    let mut intent = CoordRecord::intent(
        CoordRecordKind::Integration,
        &slug,
        "consolidate_merge",
        Some(target_branch.as_str()),
        Some(&primary_root),
        Some(&target_tip),
    );
    intent.detail = Some(format!("strategy={}", strategy.as_str()));
    journal.begin(intent.clone())?;

    // ---- step 1: lane tips -> integration branch (temporary worktree) ----
    let worktree_path = integration_worktree_path(&primary_root, &slug);
    let pre_integration = repository.read_branch_tip(&integration_branch)?;
    let (integration_created, integration_tip) = match merge_lanes_into_integration(
        workspace,
        &manifest,
        &integration_branch,
        &worktree_path,
        &pending,
        &mut lanes,
    ) {
        Ok(value) => value,
        Err(error) => {
            let fault = journal
                .resolve(
                    &intent,
                    CoordPhase::Abandoned,
                    Some(&error.to_string()),
                    Some(&target_tip),
                )
                .err()
                .map(|fault| fault.to_string());
            return Err(with_journal_fault(error, fault));
        }
    };

    // ---- step 1b: Source's mission-number bake (BEFORE the target moves) ----
    //
    // Source `consolidation/phase_advance.py::_phase_bake_and_pre_target_done`
    // runs `_bake_mission_number_into_mission_branch` after lane integration and
    // BEFORE the mission→target landing: the number is computed from the target's
    // view and written as its OWN commit on the MISSION branch, so a MERGE landing
    // carries it into the landed history. A skipped/failed branch write still
    // returns the number, which the post-landing projection records on the target.
    let assignment =
        bake_mission_number_into_branch(&repository, &slug, &integration_branch, &target_branch)?;
    if let Some(note) = &assignment.note {
        eprintln!("mission_number={} for {slug}: {note}", assignment.number);
    }
    let integration_tip = assignment.branch_tip.clone().or(integration_tip);

    // ---- step 2: integration branch -> target, by strategy ----
    let landing = match land_integration(
        &repository,
        &primary_root,
        &target_branch,
        &integration_branch,
        &worktree_path,
        &target_tip,
        integration_tip.as_ref(),
        strategy,
        &slug,
    ) {
        Ok(value) => value,
        Err(error) => {
            // The integration branch and its worktree are ours to clean up;
            // the target may or may not have moved. Both facts are recorded.
            let observed_tip = repository.read_branch_tip(&target_branch).ok().flatten();
            cleanup_integration(
                &repository,
                &worktree_path,
                integration_tip.as_ref(),
                &integration_branch,
                integration_created,
                &target_tip,
                false,
                false,
                false,
                "",
            );
            let fault = journal
                .resolve(
                    &intent,
                    CoordPhase::Indeterminate,
                    Some(&error.to_string()),
                    observed_tip.as_ref(),
                )
                .err()
                .map(|fault| fault.to_string());
            return Err(with_journal_fault(error, fault));
        }
    };

    // ---- step 3: reconciliation gate, bookkeeping projection, done recording ----
    //
    // Source order: the approved-work reconciliation gate runs before anything is
    // recorded or torn down; the done recording and the `meta.json` projection are
    // the bookkeeping the landing owns; the retrospective capture comes last. A
    // failure anywhere here is the rollback door: every branch this run moved is
    // restored to its pre-consolidation commit.
    let baseline = landing.baseline.clone();
    let mut teardown = Vec::new();
    let post = run_post_landing(
        workspace,
        &repository,
        &target_branch,
        &integration_branch,
        &manifest,
        work_package_lanes,
        &pending,
        &baseline,
        &target_tip,
        strategy,
        assignment.number,
    );
    let (baseline_path, metadata_commit, _retrospective_commit) = match post {
        Ok(recorded) => (
            recorded.baseline_path,
            recorded.metadata_commit,
            recorded.retrospective_commit,
        ),
        Err(error) => {
            let observed = repository.read_branch_tip(&target_branch).ok().flatten();
            let rollback = crate::lifecycle_protocol::rollback_to_pre_run(
                &repository,
                &target_branch,
                observed.as_ref(),
                &target_tip,
                pre_integration
                    .as_ref()
                    .map(|tip| (&integration_branch, tip)),
                &pending
                    .iter()
                    .map(|(_, branch, tip)| (branch.clone(), tip.clone()))
                    .collect::<Vec<_>>(),
            );
            let _ = discard_scratch_worktree(
                &repository,
                &integration_worktree_path(&primary_root, &slug),
            );
            let _ = sk_mission::record_baseline_merge_commit_clear(mission.spec_dir());
            let report = rollback
                .entries
                .iter()
                .map(|entry| {
                    let observed = entry.observed.as_deref().unwrap_or("absent");
                    match &entry.restored_to {
                        Some(target) => format!(
                            "{} {} {} -> {target}",
                            entry.outcome, entry.branch, observed
                        ),
                        None => format!("{} {}", entry.outcome, entry.branch),
                    }
                })
                .collect::<Vec<_>>()
                .join("; ");
            return Err(refused(
                "CONSOLIDATION_RECONCILIATION_FAILED",
                format!(
                    "{error}. Every branch this consolidation moved is being restored to its pre-consolidation commit: {report}. Nothing was torn down; the target keeps its pre-run tip."
                ),
            ));
        }
    };

    let retain_branches = retention_flag(mission, "retain_branches").unwrap_or(false);
    let delete_integration = request.delete_branch.unwrap_or(!retain_branches);
    teardown.extend(cleanup_integration(
        &repository,
        &worktree_path,
        integration_tip.as_ref(),
        &integration_branch,
        integration_created,
        &baseline,
        true,
        delete_integration,
        strategy == MergeStrategy::Squash,
        &squash_reason_for(baseline.as_str()),
    ));
    teardown.extend(teardown_lanes(
        workspace,
        &repository,
        &manifest,
        request,
        &target_branch,
        baseline.as_str(),
        strategy,
    )?);

    let journal_fault = journal
        .resolve(
            &intent,
            CoordPhase::Applied,
            Some("consolidation landed every lane tip"),
            Some(&baseline),
        )
        .err()
        .map(|fault| fault.to_string());

    Ok(ConsolidationReport {
        mission_slug: slug,
        target_branch: target_branch.as_str().to_owned(),
        strategy: strategy.as_str(),
        pre_target_tip: Some(target_tip),
        baseline_commit: baseline,
        integration_branch: integration_branch.as_str().to_owned(),
        integration_created,
        integration_commit: integration_tip,
        lanes,
        teardown,
        baseline_path,
        journal_path: journal.path().to_path_buf(),
        journal_fault,
        already_consolidated: false,
        dry_run: false,
        metadata_commit,
    })
}

fn with_journal_fault(error: ConsolidationError, fault: Option<String>) -> ConsolidationError {
    match fault {
        Some(fault) => refused(
            "CONSOLIDATION_FAILED_WITH_JOURNAL_FAULT",
            format!("{error}; the run journal could not record the abandoned run either: {fault}"),
        ),
        None => error,
    }
}

/// Merge every pending lane tip into the integration branch.
///
/// The integration branch is materialised in its own temporary worktree so the
/// operator's target checkout is never touched before the landing. A conflict
/// aborts the half-merge, removes the temporary worktree (and the branch when
/// this run created it), and refuses with the real conflict paths; the target
/// keeps its exact pre-run tip.
fn merge_lanes_into_integration(
    workspace: &MissionWorkspace,
    manifest: &LanesManifest,
    integration_branch: &BranchName,
    worktree_path: &Path,
    pending: &[(String, BranchName, Oid)],
    lanes: &mut [LaneLanding],
) -> Result<(bool, Option<Oid>), ConsolidationError> {
    let primary_root = workspace.workspace().primary_root().to_path_buf();
    let repository = GitRepository::open(&primary_root)?;
    let existing_tip = repository.read_branch_tip(integration_branch)?;
    let created = existing_tip.is_none();

    // Materialise (or reuse) the integration worktree.
    if !worktree_path.join(".git").exists() {
        let start = match &existing_tip {
            None => WorktreeStart::NewBranch(StartPoint::Oid(
                repository
                    .read_branch_tip(&BranchName::parse(&manifest.target_branch)?)?
                    .ok_or_else(|| {
                        refused(
                            "CONSOLIDATION_TARGET_MISSING",
                            format!("target branch {} does not exist", manifest.target_branch),
                        )
                    })?,
            )),
            Some(_) => WorktreeStart::ExistingBranch,
        };
        let plan = repository
            .plan_worktree_add(worktree_path.to_path_buf(), integration_branch, start)
            .map_err(|error| {
                refused(
                    "CONSOLIDATION_INTEGRATION_WORKTREE_FAILED",
                    format!(
                        "could not materialise the integration branch {integration_branch} at {}: {error}",
                        worktree_path.display()
                    ),
                )
            })?;
        repository.apply_worktree_add(&plan).map_err(|error| {
            refused(
                "CONSOLIDATION_INTEGRATION_WORKTREE_FAILED",
                format!(
                    "could not materialise the integration branch {integration_branch} at {}: {error}",
                    worktree_path.display()
                ),
            )
        })?;
    }

    let integration = GitRepository::open(worktree_path)?;
    let mut tip = integration
        .read_branch_tip(integration_branch)?
        .ok_or_else(|| {
            refused(
                "CONSOLIDATION_INTEGRATION_BRANCH_MISSING",
                format!("integration branch {integration_branch} disappeared"),
            )
        })?;

    // Source activates the registered drivers for the whole lane→mission stride
    // (`_merge_branch_into`, all strategies). The lane merges stay base-aware:
    // their real ancestry makes `%O` a true merge base, so the two-way meta
    // opt-out is NOT set here.
    let activation = repository.activate_merge_drivers(false)?;

    for (lane_id, branch, lane_tip) in pending {
        // Source's `lane_integrated_by_tree_or_ancestry`: ancestry OR an
        // identical tree. A squash-style payload already present byte-for-byte
        // must not be re-merged into a no-op merge commit.
        if lane_integrated(&integration, branch, integration_branch)? {
            if let Some(entry) = lanes.iter_mut().find(|entry| &entry.lane_id == lane_id) {
                entry.outcome = "already_integrated".to_owned();
            }
            continue;
        }
        // Source's lane merge is `git merge --no-edit -m "Merge <lane> into
        // <mission>"` with NO `--no-ff` (`lanes/consolidation.py::_merge_branch_into`),
        // so a lane tip that already descends from the integration tip
        // FAST-FORWARDS: the branch moves to the lane commit and NO merge commit
        // is created. Only a genuine divergence produces one. (Source-first
        // evidence: `h4r-causal2-seed{0,1}` - lane-a fast-forwards, lane-b and
        // lane-c merge, identically under two hash seeds and two fixtures.)
        if integrated(&integration, &tip, lane_tip)? {
            integration.advance_branch_ref(integration_branch, &tip, lane_tip)?;
            tip = lane_tip.clone();
            if let Some(entry) = lanes.iter_mut().find(|entry| &entry.lane_id == lane_id) {
                entry.outcome = "fast_forwarded".to_owned();
                entry.merge_commit = Some(lane_tip.clone());
            }
            continue;
        }
        match merge_commit_into(&integration, integration_branch, lane_tip, branch.as_str()) {
            Ok(commit) => {
                tip = commit.clone();
                if let Some(entry) = lanes.iter_mut().find(|entry| &entry.lane_id == lane_id) {
                    entry.outcome = "merged".to_owned();
                    entry.merge_commit = Some(commit);
                }
            }
            Err(error) => {
                // The lane conflict left the target untouched; remove what this
                // run materialised so a retry starts clean.
                let _ = cleanup_integration(
                    &repository,
                    worktree_path,
                    Some(&tip),
                    integration_branch,
                    created,
                    &tip,
                    false,
                    false,
                    false,
                    "",
                );
                return Err(error);
            }
        }
    }
    activation.finish()?;
    Ok((created, Some(tip)))
}

/// One real merge commit of `tip` into the checked-out `branch`.
///
/// The registered merge drivers are active (the caller seeds them) so a
/// `kitty-specs/**` bookkeeping conflict is reconciled by the same body Source
/// uses. When git could not start a driver (a native binary not on PATH), the
/// same body is applied to git's own stage-1/2/3 blobs; anything still
/// unresolved aborts the half-merge and refuses with the real paths.
fn merge_commit_into(
    repository: &GitRepository,
    branch: &BranchName,
    tip: &Oid,
    source_label: &str,
) -> Result<Oid, ConsolidationError> {
    // Source's lane->mission merge message (`lanes/consolidation.py`,
    // `_merge_branch_into`, MERGE strategy):
    // `git merge --no-ff --no-edit -m "Merge <lane> into <mission>"`.
    let message = format!("Merge {source_label} into {branch}");
    let plan = repository.plan_merge_start(&StartPoint::Oid(tip.clone()))?;
    let start = repository.apply_merge_start(&plan)?;
    match start.status() {
        MergeStartStatus::AlreadyUpToDate => Ok(start.head().clone()),
        MergeStartStatus::Conflicted => {
            let paths: Vec<GitPath> = start
                .conflicts()
                .iter()
                .map(|conflict| conflict.path().clone())
                .collect();
            let unresolved = resolve_registered_driver_conflicts(repository, &paths, false)?;
            if !unresolved.is_empty() {
                let rendered: Vec<String> = unresolved.iter().map(GitPath::display_lossy).collect();
                let _ = repository.merge_abort();
                return Err(refused(
                    "CONSOLIDATION_MERGE_CONFLICT",
                    format!(
                        "merging {source_label} into {branch} conflicts on {}; the half-merge was aborted and the target branch was not touched",
                        rendered.join(", ")
                    ),
                ));
            }
            conclude_merge(repository, start.head(), &message, source_label, branch)
        }
        MergeStartStatus::Stopped => {
            conclude_merge(repository, start.head(), &message, source_label, branch)
        }
    }
}

fn conclude_merge(
    repository: &GitRepository,
    head: &Oid,
    message: &str,
    source_label: &str,
    branch: &BranchName,
) -> Result<Oid, ConsolidationError> {
    let conclusion = repository.plan_merge_conclusion(head, Some(message))?;
    match repository.apply_merge_conclusion(&conclusion) {
        Ok(concluded) => Ok(concluded.new_head().clone()),
        Err(GitError::MergeConclusionUnconfirmed(evidence)) => {
            let payload = *evidence;
            Err(refused(
                "CONSOLIDATION_MERGE_UNCONFIRMED",
                format!(
                    "merging {source_label} into {branch} stopped at an unconfirmed conclusion: created commit {:?}, observed HEAD {:?}, operation marker consumed: {}. Nothing was aborted or rewound.",
                    payload.candidate, payload.observed_head, payload.operation_consumed
                ),
            ))
        }
        Err(error) => {
            let _ = repository.merge_abort();
            Err(ConsolidationError::Git(error))
        }
    }
}

/// The landed target tip.
struct Landing {
    baseline: Oid,
}

/// The unique scratch worktree this run lands in.
///
/// Source materialises a temporary checkout (`tempfile.mkdtemp("kitty-merge-")`)
/// so the operator's target checkout is never the place a merge is resolved.
/// The native run keeps that property with a tool-owned path under
/// `.worktrees/`; the process id keeps two concurrent runs from colliding.
fn scratch_worktree_path(primary_root: &Path, slug: &str) -> PathBuf {
    primary_root
        .join(".worktrees")
        .join(format!("{slug}-landing-{}", std::process::id()))
}

/// Remove a scratch worktree, returning a diagnostic when removal failed.
fn discard_scratch_worktree(repository: &GitRepository, path: &Path) -> Option<String> {
    if !path.exists() {
        return None;
    }
    let plan =
        repository.plan_worktree_removal(path, None, None, sk_git::WorktreeDiscardPolicy::AllowAll);
    match plan.and_then(|plan| repository.apply_worktree_removal(&plan).map(|_| ())) {
        Ok(()) => None,
        Err(error) => Some(format!(
            "could not remove the scratch worktree {}: {error}",
            path.display()
        )),
    }
}

/// Apply the registered merge-driver bodies to git's own conflict stages.
///
/// Git invokes the drivers itself when `spec-kitty` is on PATH. When it is not
/// (a native binary that was not installed globally), git leaves the registered
/// path unmerged. Source has exactly this second caller —
/// `consolidation/git_probes.py::_resolve_registered_driver_callable` — and it
/// applies the SAME body to the SAME stage-1/2/3 blobs. Returning the still
/// unresolved paths keeps a genuine conflict visible instead of inventing a
/// resolution.
fn resolve_registered_driver_conflicts(
    worktree: &GitRepository,
    paths: &[GitPath],
    two_way_meta: bool,
) -> Result<Vec<GitPath>, ConsolidationError> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let conflicts = worktree.unmerged_paths()?;
    let mut unresolved = Vec::new();
    for path in paths {
        let rendered = path.display_lossy();
        let Some(spec) = crate::merge_drivers::driver_spec_for_path(&rendered) else {
            unresolved.push(path.clone());
            continue;
        };
        let Some(driver) = spec.driver_name() else {
            unresolved.push(path.clone());
            continue;
        };
        let Some(conflict) = conflicts.iter().find(|conflict| conflict.path() == path) else {
            // Already resolved (for example staged by a real driver invocation).
            continue;
        };
        let stage_oid = |number: u8| {
            conflict
                .stages()
                .iter()
                .find(|stage| stage.stage() == number)
                .map(|stage| stage.oid().clone())
        };
        let read = |number: u8| -> Result<Option<Vec<u8>>, ConsolidationError> {
            match stage_oid(number) {
                Some(oid) => Ok(Some(worktree.read_blob_object(&oid)?)),
                None => Ok(None),
            }
        };
        let base = if two_way_meta && driver == "merge-driver-meta" {
            None
        } else {
            read(1)?
        };
        let ours = read(2)?;
        let theirs = read(3)?;
        crate::merge_drivers::run_driver_into(
            driver,
            crate::merge_drivers::MergeDriverInputs {
                base: base.as_deref(),
                ours: ours.as_deref(),
                theirs: theirs.as_deref(),
            },
            &worktree.root().join(path.as_os_str()),
        )
        .map_err(|error| {
            refused(
                "CONSOLIDATION_MERGE_DRIVER_FAILED",
                format!("{rendered}: {error}"),
            )
        })?;
        // Publication already happened through the shared physical writer.
        // Stage exactly that file, never a second write of a prepared payload.
        worktree.stage_path(path)?;
    }
    for conflict in worktree.unmerged_paths()? {
        unresolved.push(conflict.path().clone());
    }
    unresolved.sort_by_key(GitPath::display_lossy);
    unresolved.dedup();
    Ok(unresolved)
}

/// The Source squash message (`feat(<mission branch>): squash merge of mission`).
fn squash_message(integration_branch: &BranchName) -> String {
    format!(
        "feat({}): squash merge of mission",
        integration_branch.as_str()
    )
}

/// The Source merge message (`Merge <mission branch> into <target>`).
fn merge_message(integration_branch: &BranchName, target: &BranchName) -> String {
    format!(
        "Merge {} into {}",
        integration_branch.as_str(),
        target.as_str()
    )
}

/// Land the integration branch on the target with `strategy`.
///
/// Every stride runs in a detached scratch worktree at the target tip with the
/// registered merge drivers activated for the duration, exactly as Source's
/// `_merge_branch_into` does. The target ref is advanced only after the stride
/// produced a real commit; a conflict leaves the target exactly where it was.
#[allow(clippy::too_many_arguments)]
fn land_integration(
    repository: &GitRepository,
    primary_root: &Path,
    target: &BranchName,
    integration_branch: &BranchName,
    worktree_path: &Path,
    pre_target_tip: &Oid,
    integration_tip: Option<&Oid>,
    strategy: MergeStrategy,
    slug: &str,
) -> Result<Landing, ConsolidationError> {
    let _ = worktree_path;
    let Some(integration_tip) = integration_tip else {
        return Ok(Landing {
            baseline: pre_target_tip.clone(),
        });
    };
    if integrated(repository, integration_tip, pre_target_tip)? {
        return Ok(Landing {
            baseline: pre_target_tip.clone(),
        });
    }

    let scratch_path = scratch_worktree_path(primary_root, slug);
    if let Some(fault) = discard_scratch_worktree(repository, &scratch_path) {
        return Err(refused("CONSOLIDATION_SCRATCH_WORKTREE_STALE", fault));
    }
    let activation = repository.activate_merge_drivers(false)?;
    let start = match strategy {
        MergeStrategy::Rebase => StartPoint::Oid(integration_tip.clone()),
        MergeStrategy::Merge | MergeStrategy::Squash => StartPoint::Oid(pre_target_tip.clone()),
    };
    let outcome = (|| {
        repository.add_detached_worktree(&scratch_path, &start)?;
        let worktree = GitRepository::open(&scratch_path)?;
        match strategy {
            MergeStrategy::Squash => land_squash(
                repository,
                &worktree,
                target,
                integration_branch,
                pre_target_tip,
            ),
            MergeStrategy::Merge => land_merge(
                repository,
                &worktree,
                target,
                integration_branch,
                pre_target_tip,
            ),
            MergeStrategy::Rebase => land_rebase(repository, &worktree, target, pre_target_tip),
        }
    })();
    let cleanup_fault = discard_scratch_worktree(repository, &scratch_path);
    let activation_fault = activation.finish().err().map(|error| error.to_string());
    match (outcome, cleanup_fault, activation_fault) {
        (Ok(landing), None, None) => Ok(landing),
        (Err(error), None, None) => Err(error),
        (Ok(landing), cleanup, activation) => {
            let mut notes = Vec::new();
            if let Some(fault) = cleanup {
                notes.push(fault);
            }
            if let Some(fault) = activation {
                notes.push(format!("merge-driver teardown failed: {fault}"));
            }
            if notes.is_empty() {
                return Ok(landing);
            }
            Err(refused(
                "CONSOLIDATION_POST_LANDING_CLEANUP",
                format!(
                    "the landing commit {} is on {target}; cleanup after it failed: {}",
                    landing.baseline,
                    notes.join("; ")
                ),
            ))
        }
        (Err(error), cleanup, activation) => {
            let mut suffix = Vec::new();
            if let Some(fault) = cleanup {
                suffix.push(fault);
            }
            if let Some(fault) = activation {
                suffix.push(format!("merge-driver teardown failed: {fault}"));
            }
            if suffix.is_empty() {
                Err(error)
            } else {
                Err(refused(
                    "CONSOLIDATION_FAILED_WITH_CLEANUP_FAULT",
                    format!("{error}; {}", suffix.join("; ")),
                ))
            }
        }
    }
}

fn land_squash(
    repository: &GitRepository,
    worktree: &GitRepository,
    target: &BranchName,
    integration_branch: &BranchName,
    pre_target_tip: &Oid,
) -> Result<Landing, ConsolidationError> {
    let message = squash_message(integration_branch);
    let status = repository.squash_merge(worktree.root(), integration_branch)?;
    let conflicts = match status {
        SquashMergeStatus::NoChanges => {
            return Ok(Landing {
                baseline: pre_target_tip.clone(),
            });
        }
        SquashMergeStatus::Staged => Vec::new(),
        SquashMergeStatus::Conflicts(paths) => paths,
    };
    let unresolved = resolve_registered_driver_conflicts(worktree, &conflicts, true)?;
    if !unresolved.is_empty() {
        return Err(refused(
            "CONSOLIDATION_MERGE_CONFLICT",
            format!(
                "squash merge of {} into {} left unresolved content conflicts: {}; the target keeps its pre-run tip",
                integration_branch.as_str(),
                target.as_str(),
                unresolved
                    .iter()
                    .map(GitPath::display_lossy)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }
    if worktree.staged_state()? == SquashMergeStatus::NoChanges {
        // Every conflict resolved to the target copy: a genuine no-op.
        return Ok(Landing {
            baseline: pre_target_tip.clone(),
        });
    }
    let commit = worktree.commit_staged_merge(pre_target_tip, &message)?;
    repository.advance_branch_ref(target, pre_target_tip, &commit)?;
    Ok(Landing { baseline: commit })
}

fn land_merge(
    repository: &GitRepository,
    worktree: &GitRepository,
    target: &BranchName,
    integration_branch: &BranchName,
    pre_target_tip: &Oid,
) -> Result<Landing, ConsolidationError> {
    let message = merge_message(integration_branch, target);
    match worktree.merge_no_ff(integration_branch, &message, true)? {
        MergeNoFfStatus::AlreadyUpToDate => Ok(Landing {
            baseline: pre_target_tip.clone(),
        }),
        MergeNoFfStatus::Merged { commit } => {
            repository.advance_branch_ref(target, pre_target_tip, &commit)?;
            Ok(Landing { baseline: commit })
        }
        MergeNoFfStatus::Conflicts(paths) => {
            let unresolved = resolve_registered_driver_conflicts(worktree, &paths, true)?;
            if !unresolved.is_empty() {
                let _ = worktree.merge_abort();
                return Err(refused(
                    "CONSOLIDATION_MERGE_CONFLICT",
                    format!(
                        "landing {} on {} conflicts on {}; the half-merge was aborted and the target keeps its pre-run tip",
                        integration_branch.as_str(),
                        target.as_str(),
                        unresolved
                            .iter()
                            .map(GitPath::display_lossy)
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ));
            }
            match worktree.conclude_merge_in_progress(pre_target_tip, &message) {
                Ok(commit) => {
                    repository.advance_branch_ref(target, pre_target_tip, &commit)?;
                    Ok(Landing { baseline: commit })
                }
                Err(error) => {
                    let _ = worktree.merge_abort();
                    Err(ConsolidationError::Git(error))
                }
            }
        }
    }
}

fn land_rebase(
    repository: &GitRepository,
    worktree: &GitRepository,
    target: &BranchName,
    pre_target_tip: &Oid,
) -> Result<Landing, ConsolidationError> {
    let RebaseStatus::Rebased { new_tip, .. } = worktree.rebase_onto(target)?;
    repository.advance_branch_ref(target, pre_target_tip, &new_tip)?;
    Ok(Landing { baseline: new_tip })
}

#[allow(clippy::too_many_arguments)]
fn cleanup_integration(
    repository: &GitRepository,
    worktree_path: &Path,
    integration_tip: Option<&Oid>,
    integration_branch: &BranchName,
    _created: bool,
    landed_tip: &Oid,
    record_actions: bool,
    delete_branch: bool,
    content_landed: bool,
    squash_reason: &str,
) -> Vec<TeardownAction> {
    let mut actions = Vec::new();
    if worktree_path.join(".git").exists() {
        let head = GitRepository::open(worktree_path)
            .ok()
            .and_then(|open| open.head_commit().ok().flatten())
            .and_then(|text| Oid::parse(&text).ok());
        let plan = repository.plan_worktree_removal(
            worktree_path,
            head.as_ref(),
            None,
            WorktreeDiscardPolicy::AllowAll,
        );
        match plan.and_then(|plan| repository.apply_worktree_removal(&plan)) {
            Ok(_) => {
                if record_actions {
                    actions.push(TeardownAction {
                        action: "integration_worktree_removed".to_owned(),
                        target: worktree_path.display().to_string(),
                        reason: None,
                    });
                }
            }
            Err(error) => {
                if record_actions {
                    actions.push(TeardownAction {
                        action: "integration_worktree_removed".to_owned(),
                        target: worktree_path.display().to_string(),
                        reason: Some(format!("guarded removal refused: {error}")),
                    });
                }
            }
        }
    }
    // Source tears the Mission's integration branch down once its content landed,
    // whether or not this run created it; the reachability proof below is what
    // makes the deletion safe, not the branch's provenance.
    if delete_branch {
        let tip = repository
            .read_branch_tip(integration_branch)
            .ok()
            .flatten()
            .or_else(|| integration_tip.cloned());
        let reachable = content_landed
            || match &tip {
                Some(tip) => integrated(repository, tip, landed_tip).unwrap_or(false),
                None => true,
            };
        if !reachable {
            if record_actions {
                actions.push(TeardownAction {
                    action: "integration_branch_retained".to_owned(),
                    target: integration_branch.as_str().to_owned(),
                    reason: Some(format!(
                        "the integration tip is not reachable from {landed_tip}; it is kept"
                    )),
                });
            }
            return actions;
        }
        // A squash landing carries the integration *content* without making the
        // integration commits ancestors, so the guarded analysis needs the
        // caller's recorded reason there.
        let options = if content_landed {
            sk_git::BranchDeletionOptions::default().allow_unique_commits(squash_reason)
        } else {
            sk_git::BranchDeletionOptions::default()
        };
        match repository.plan_branch_deletion(
            integration_branch,
            &ProtectedBranches::default(),
            integration_tip,
            options,
            "spec-kitty: retire the landed integration branch",
        ) {
            Ok(plan) => match repository.apply_branch_deletion(&plan) {
                Ok(_) => {
                    if record_actions {
                        actions.push(TeardownAction {
                            action: "integration_branch_deleted".to_owned(),
                            target: integration_branch.as_str().to_owned(),
                            reason: None,
                        });
                    }
                }
                Err(error) => {
                    if record_actions {
                        actions.push(TeardownAction {
                            action: "integration_branch_retained".to_owned(),
                            target: integration_branch.as_str().to_owned(),
                            reason: Some(format!("guarded deletion refused: {error}")),
                        });
                    }
                }
            },
            Err(error) => {
                if record_actions {
                    actions.push(TeardownAction {
                        action: "integration_branch_retained".to_owned(),
                        target: integration_branch.as_str().to_owned(),
                        reason: Some(format!("guarded deletion refused: {error}")),
                    });
                }
            }
        }
    } else if record_actions {
        actions.push(TeardownAction {
            action: "integration_branch_retained".to_owned(),
            target: integration_branch.as_str().to_owned(),
            reason: Some("the mission's retention policy keeps branches".to_owned()),
        });
    }
    actions
}

/// The recorded reason a squash landing authorises a guarded branch deletion.
fn squash_reason_for(baseline: &str) -> String {
    format!(
        "the content landed in the squashed baseline commit {baseline} (recorded in meta.json baseline_merge_commit)"
    )
}

/// Retire lane worktrees and branches according to the request and the
/// Mission's persisted retention policy.
fn teardown_lanes(
    workspace: &MissionWorkspace,
    repository: &GitRepository,
    manifest: &LanesManifest,
    request: &ConsolidationRequest,
    target: &BranchName,
    baseline: &str,
    strategy: MergeStrategy,
) -> Result<Vec<TeardownAction>, ConsolidationError> {
    let mission = workspace.mission();
    let retain_branches = retention_flag(mission, "retain_branches").unwrap_or(false);
    let retain_worktrees = retention_flag(mission, "retain_worktrees").unwrap_or(false);
    let delete_branch = request.delete_branch.unwrap_or(!retain_branches);
    let remove_worktree = request.remove_worktree.unwrap_or(!retain_worktrees);
    if !delete_branch && !remove_worktree {
        return Ok(Vec::new());
    }
    let protected = ProtectionPolicy::resolve(workspace.workspace().primary_root())
        .and_then(|policy| policy.branch_set())
        .unwrap_or_default();
    let allocator = LaneAllocator::new(repository, protected, &manifest.mission_slug)?;
    let primary_root = workspace.workspace().primary_root().to_path_buf();
    let mut actions = Vec::new();
    for lane in code_lanes(manifest) {
        let branch_name = match crate::naming::lane_branch_name(
            &manifest.mission_slug,
            &lane.lane_id,
            &manifest.target_branch,
        ) {
            Ok(value) => value,
            Err(error) => {
                actions.push(TeardownAction {
                    action: "branch_retained".to_owned(),
                    target: lane.lane_id.clone(),
                    reason: Some(error.to_string()),
                });
                continue;
            }
        };
        let branch = match BranchName::parse(&branch_name) {
            Ok(branch) => branch,
            Err(error) => {
                actions.push(TeardownAction {
                    action: "branch_retained".to_owned(),
                    target: branch_name,
                    reason: Some(error.to_string()),
                });
                continue;
            }
        };
        let Some(tip) = repository.read_branch_tip(&branch)? else {
            continue;
        };
        let Some(target_tip) = repository.read_branch_tip(target)? else {
            actions.push(TeardownAction {
                action: "branch_retained".to_owned(),
                target: branch.as_str().to_owned(),
                reason: Some(format!("target branch {target} does not exist")),
            });
            continue;
        };
        // A lane that did not land is never torn down. A squash landing carries
        // the lane content in the baseline without making the lane commits
        // ancestors, so that landing is proven by the recorded baseline instead.
        if strategy != MergeStrategy::Squash && !integrated(repository, &tip, &target_tip)? {
            actions.push(TeardownAction {
                action: "branch_retained".to_owned(),
                target: branch.as_str().to_owned(),
                reason: Some(format!(
                    "tip {tip} is not reachable from {target}; the lane did not land"
                )),
            });
            continue;
        }
        let worktree =
            crate::naming::worktree_path(&primary_root, &manifest.mission_slug, &lane.lane_id)?;
        if remove_worktree && worktree.exists() {
            match allocator.retire_worktree(&lane.lane_id, &worktree) {
                Ok(()) => actions.push(TeardownAction {
                    action: "worktree_removed".to_owned(),
                    target: worktree.display().to_string(),
                    reason: None,
                }),
                Err(WorkspaceError::RetentionRefused { reason, .. }) => {
                    actions.push(TeardownAction {
                        action: "worktree_retained".to_owned(),
                        target: worktree.display().to_string(),
                        reason: Some(reason),
                    });
                }
                Err(error) => return Err(error.into()),
            }
        }
        if delete_branch {
            // A squash landing proves the lane *content* landed in the baseline
            // commit without making the lane commits ancestors, so the guarded
            // unique-commit analysis is overridden with that recorded reason.
            let squash_reason = squash_reason_for(baseline);
            let deletion = if strategy == MergeStrategy::Squash {
                allocator.retire_branch_allowing_unique_commits(&branch, None, &squash_reason)
            } else {
                allocator.retire_branch(&branch, None)
            };
            match deletion {
                Ok(()) => actions.push(TeardownAction {
                    action: "branch_deleted".to_owned(),
                    target: branch.as_str().to_owned(),
                    reason: None,
                }),
                Err(error) => actions.push(TeardownAction {
                    action: "branch_retained".to_owned(),
                    target: branch.as_str().to_owned(),
                    reason: Some(format!("guarded deletion refused: {error}")),
                }),
            }
        }
    }
    Ok(actions)
}

/// What the post-landing lifecycle recorded.
struct PostLanding {
    baseline_path: PathBuf,
    metadata_commit: Option<Oid>,
    retrospective_commit: Option<Oid>,
}

/// The canonical status rows of a Mission (absent log → no rows).
fn read_status_rows(mission: &Mission) -> Result<Vec<serde_json::Value>, ConsolidationError> {
    let path = mission.spec_dir().join("status.events.jsonl");
    match sk_status::read_raw_events(&path) {
        Ok(rows) => Ok(rows),
        Err(sk_status::SnapshotError::Io(error))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            Ok(Vec::new())
        }
        Err(error) => Err(refused(
            "CONSOLIDATION_STATUS_UNREADABLE",
            format!("could not read {}: {error}", path.display()),
        )),
    }
}

/// Run Source's post-landing phases: the reconciliation gate, the Mission-number
/// bake with the `meta.json` projection, the per-work-package `done` recording,
/// the coordination alias fold and the retrospective capture.
///
/// Teardown and the journal's applied phase run after this returns; a failure here
/// is the caller's rollback door.
#[allow(clippy::too_many_arguments)]
fn run_post_landing(
    workspace: &MissionWorkspace,
    repository: &GitRepository,
    target: &BranchName,
    integration_branch: &BranchName,
    manifest: &LanesManifest,
    work_package_lanes: &[(String, String)],
    pending: &[(String, BranchName, Oid)],
    landed: &Oid,
    pre_target_tip: &Oid,
    strategy: MergeStrategy,
    mission_number: i64,
) -> Result<PostLanding, ConsolidationError> {
    use crate::lifecycle_protocol::{
        ConsolidationMetadata, MergedWorkPackage, capture_retrospective, coordination_alias_files,
        fold_coordination_alias, record_done_transitions, reviewers_of_record,
    };

    let _ = integration_branch;
    let mission = workspace.mission();

    // ---- reconciliation gate (Source: approved work is never dropped) ----
    // A merge/rebase landing keeps the approved lane commits reachable. A squash
    // landing carries their *content* instead, so Source defers the per-commit
    // reachability there; the attribution is verified by the reconciliation
    // report instead.
    if strategy != MergeStrategy::Squash {
        let mut dropped: Vec<String> = Vec::new();
        for (wp_id, branch, tip) in pending {
            if !integrated(repository, tip, landed)? {
                dropped.push(format!(
                    "approved WP {wp_id} commit {} (on {}) is NOT reachable from the target",
                    tip.short(),
                    branch.as_str()
                ));
            }
        }
        if !dropped.is_empty() {
            return Err(refused(
                "CONSOLIDATION_RECONCILIATION_FAILED",
                format!(
                    "Reconciliation FAILED: {} - approved work would be dropped",
                    dropped.join("; ")
                ),
            ));
        }
    }

    // ---- Mission-number bake + the consolidation metadata projection ----
    let merged_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, false);
    let rows = read_status_rows(mission)?;
    let reviewers = reviewers_of_record(&rows);
    let lane_tips: std::collections::BTreeMap<String, Oid> = pending
        .iter()
        .map(|(_, branch, tip)| (branch.as_str().to_owned(), tip.clone()))
        .collect();
    let mut packages: Vec<MergedWorkPackage> = Vec::new();
    for (wp_id, lane_name) in work_package_lanes {
        if !matches!(lane_name.as_str(), "approved" | "done") {
            continue;
        }
        let lane_head = manifest
            .lanes
            .iter()
            .find(|lane| {
                lane.wp_ids
                    .iter()
                    .any(|candidate| candidate.as_str() == wp_id)
            })
            .and_then(|lane| {
                let branch = crate::naming::lane_branch_name(
                    &manifest.mission_slug,
                    &lane.lane_id,
                    &manifest.target_branch,
                )
                .ok()?;
                lane_tips.get(&branch).cloned()
            });
        packages.push(MergedWorkPackage {
            reviewer: reviewers
                .get(wp_id)
                .cloned()
                .unwrap_or_else(|| "unknown".to_owned()),
            wp_id: wp_id.clone(),
            lane_head,
        });
    }
    packages.sort_by(|left, right| left.wp_id.cmp(&right.wp_id));

    let metadata = ConsolidationMetadata {
        baseline: pre_target_tip.as_str(),
        merged_commit: landed.as_str(),
        merged_at: &merged_at,
        mission_number,
    };
    let recording = record_done_transitions(mission, repository, target, &packages, &metadata)
        .map_err(|error| {
            refused(
                "CONSOLIDATION_BOOKKEEPING_FAILED",
                format!("the landing stands, but the done bookkeeping failed: {error}"),
            )
        })?;

    // ---- coordination alias fold (bare-slug coordination Missions only) ----
    let primary_root = workspace.workspace().primary_root().to_path_buf();
    if let Some(mid8) = mission
        .meta_field("mid8")
        .and_then(serde_json::Value::as_str)
    {
        let composed = primary_root.join(sk_mission::SPECS_DIR_NAME).join(format!(
            "{}-{}",
            mission.slug().as_str(),
            mid8.trim()
        ));
        if composed.is_dir()
            && let Ok(Some(fold)) =
                coordination_alias_files(&primary_root, &composed, mission.spec_dir())
        {
            fold_coordination_alias(repository, &fold, Some(pre_target_tip)).map_err(|error| {
                refused(
                    "CONSOLIDATION_ALIAS_FOLD_REFUSED",
                    format!("the landing stands, but the coordination alias fold refused: {error}"),
                )
            })?;
        }
    }

    // ---- retrospective capture (fail-open record, committed bookkeeping) ----
    let retrospective_commit = capture_retrospective(mission, repository, target, &merged_at)
        .map_err(|error| {
            refused(
                "CONSOLIDATION_RETROSPECTIVE_FAILED",
                format!("the landing stands, but the retrospective capture failed: {error}"),
            )
        })?;

    Ok(PostLanding {
        baseline_path: mission.spec_dir().join(sk_mission::META_FILE_NAME),
        metadata_commit: recording.metadata_commit,
        retrospective_commit,
    })
}

/// Abort a run that provably did not move the target.
pub fn abort_consolidation(
    workspace: &MissionWorkspace,
    journal: &CoordJournal,
) -> Result<ConsolidationReport, ConsolidationError> {
    let mission = workspace.mission();
    let slug = mission.slug().as_str().to_owned();
    let target_branch_name = workspace.target_branch()?;
    let target_branch = BranchName::parse(&target_branch_name)?;
    let repository = GitRepository::open(workspace.workspace().primary_root())?;
    let current = repository.read_branch_tip(&target_branch)?;
    let pending: Vec<CoordRecord> = journal
        .pending()?
        .into_iter()
        .filter(|record| {
            record.kind == CoordRecordKind::Integration
                && record.operation == "consolidate_merge"
                && record.mission_slug == slug
        })
        .collect();
    let Some(record) = pending.last() else {
        return Err(refused(
            "NOTHING_TO_ABORT",
            format!("no pending consolidation run is recorded for mission {slug}"),
        ));
    };
    let observed_before = record
        .observed_tip
        .as_deref()
        .map(Oid::parse)
        .transpose()
        .map_err(|error| {
            refused(
                "CONSOLIDATION_JOURNAL_CORRUPT",
                format!("the recorded pre-run target tip is not a commit id: {error}"),
            )
        })?;
    if current != observed_before {
        return Err(refused(
            "CONSOLIDATION_ALREADY_LANDED",
            format!(
                "the recorded run moved {target_branch} from {:?} to {:?}; abort never rewinds a landed target. Inspect the landing and, if it must be discarded, do so deliberately with Git.",
                observed_before, current
            ),
        ));
    }
    journal.resolve(
        record,
        CoordPhase::Abandoned,
        Some("aborted before any landing was proven"),
        current.as_ref(),
    )?;
    let baseline_path = mission.spec_dir().join(sk_mission::META_FILE_NAME);
    Ok(ConsolidationReport {
        mission_slug: slug,
        target_branch: target_branch.as_str().to_owned(),
        strategy: MergeStrategy::default_source().as_str(),
        pre_target_tip: observed_before.clone(),
        baseline_commit: current.ok_or_else(|| {
            refused(
                "CONSOLIDATION_TARGET_MISSING",
                format!("target branch {target_branch} has no commit to report"),
            )
        })?,
        integration_branch: String::new(),
        integration_created: false,
        integration_commit: None,
        lanes: Vec::new(),
        teardown: Vec::new(),
        baseline_path,
        journal_path: journal.path().to_path_buf(),
        journal_fault: None,
        already_consolidated: false,
        dry_run: false,
        metadata_commit: None,
    })
}

/// Load the lane manifest, optionally synthesizing the no-lane shape.
fn load_manifest(
    workspace: &MissionWorkspace,
    request: &ConsolidationRequest,
) -> Result<LanesManifest, ConsolidationError> {
    let store = crate::lanes::LanesStore::new(workspace.mission().spec_dir());
    match store.read()? {
        Some(manifest) => Ok(manifest),
        None if request.skip_lanes => Ok(synthesize_no_lane_manifest(workspace)?),
        None => Err(refused(
            "MISSING_LANES",
            format!(
                "lanes.json not found at {}; run tasks and finalize-tasks, or pass --skip-lanes for a mission that genuinely has no lanes",
                store.path().display()
            ),
        )),
    }
}

/// The manifest shape of a mission with no lanes: a single planning lane that
/// writes the repository-root checkout.
fn synthesize_no_lane_manifest(
    workspace: &MissionWorkspace,
) -> Result<LanesManifest, ConsolidationError> {
    let mission = workspace.mission();
    let target_branch = workspace.target_branch()?;
    Ok(LanesManifest {
        version: LanesManifest::VERSION,
        mission_slug: mission.slug().as_str().to_owned(),
        mission_id: Some(mission.id().as_str().to_owned()),
        mission_branch: workspace
            .declared_mission_branch()
            .unwrap_or_else(|| format!("kitty/mission-{}", mission.slug().as_str())),
        target_branch,
        lanes: vec![ExecutionLane {
            lane_id: crate::naming::PLANNING_LANE_ID.to_owned(),
            wp_ids: Vec::new(),
            write_scope: vec![format!("kitty-specs/{}/**", mission.slug().as_str())],
            predicted_surfaces: Vec::new(),
            depends_on_lanes: Vec::new(),
            parallel_group: 0,
        }],
        computed_at: sk_kernel::clock::now_utc_iso(),
        computed_from: "synthesized_no_lanes".to_owned(),
        planning_artifact_wps: Vec::new(),
        planning_commit_sha: None,
        collapse_report: None,
    })
}

/// The Mission's slug for diagnostics.
pub fn consolidation_slug(mission: &Mission) -> String {
    mission.slug().as_str().to_owned()
}

/// The primary checkout path a run writes.
pub fn consolidation_checkout(workspace: &MissionWorkspace) -> &Path {
    workspace.workspace().primary_root()
}
