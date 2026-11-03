//! Read-only branch, lane, and checkout occupancy facts.
//!
//! A coordination transaction must know, before it writes anything:
//!
//! * the tip of the mission target branch, the mission integration branch, and
//!   the coordination branch (never derived from the current checkout);
//! * which code-lane branches exist and at what tip;
//! * which checkout currently holds each branch, and whether it is dirty —
//!   because advancing a branch that is checked out elsewhere leaves that
//!   checkout behind its own HEAD;
//! * the lane-work-tip refs (`refs/spec-kitty/lane-tip/<branch>`), which are the
//!   only record that survives a destroyed lane.
//!
//! Everything here is a read except [`record_lane_tip`], which writes one
//! bookkeeping ref through a compare-and-swap plan. No function in this module
//! creates a worktree, checks out a branch, or touches a user file.

use std::path::{Path, PathBuf};

use sk_git::{BranchName, GitRepository, Oid, RefChange, RefName};
use sk_mission::routes_through_coordination;

use crate::error::WorkspaceError;
use crate::lanes::LanesManifest;
use crate::naming::PLANNING_LANE_ID;
use crate::workspace::{MissionWorkspace, SPECS_DIR_NAME};

/// Namespace of the hidden, never-pushed lane work-tip refs.
pub const LANE_TIP_REF_PREFIX: &str = "refs/spec-kitty/lane-tip/";

/// A branch and its current tip.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BranchTip {
    /// Short branch name.
    pub branch: String,
    /// Tip, or `None` when the branch does not exist.
    pub tip: Option<Oid>,
}

impl BranchTip {
    /// Does the branch exist?
    pub fn exists(&self) -> bool {
        self.tip.is_some()
    }
}

/// One code lane's branch state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaneBranchTip {
    /// Lane id from the manifest.
    pub lane_id: String,
    /// Lane branch name.
    pub branch: String,
    /// Live branch tip.
    pub tip: Option<Oid>,
    /// Recorded lane-work-tip ref, when one exists.
    pub recorded_tip: Option<Oid>,
}

impl LaneBranchTip {
    /// Is the recorded lane tip missing or behind the live branch tip?
    ///
    /// A `None` live tip with a recorded tip is the destroyed-lane shape: the
    /// work survives only in the ref, and it must never be mistaken for an
    /// empty lane.
    pub fn is_stale(&self) -> bool {
        match (&self.recorded_tip, &self.tip) {
            (Some(recorded), Some(tip)) => recorded != tip,
            (Some(_), None) => true,
            _ => false,
        }
    }

    /// Does a destroyed branch still have recorded work?
    pub fn is_destroyed_but_recorded(&self) -> bool {
        self.tip.is_none() && self.recorded_tip.is_some()
    }
}

/// Every branch a mission's write path depends on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MissionBranchTips {
    /// The mission's target branch.
    pub target: BranchTip,
    /// The stored `meta.json` `mission_branch`, when recorded.
    pub mission_branch: Option<BranchTip>,
    /// The declared coordination branch, when the topology routes through it.
    pub coordination: Option<BranchTip>,
    /// The recorded planning commit, resolved to an object id when readable.
    pub planning_commit: Option<Oid>,
    /// Code lanes, in manifest order.
    pub lanes: Vec<LaneBranchTip>,
}

impl MissionBranchTips {
    /// The lane entry for `lane_id`.
    pub fn lane(&self, lane_id: &str) -> Option<&LaneBranchTip> {
        self.lanes.iter().find(|lane| lane.lane_id == lane_id)
    }

    /// Lanes whose recorded work tip is missing or behind the live tip.
    pub fn stale_lanes(&self) -> Vec<&LaneBranchTip> {
        self.lanes.iter().filter(|lane| lane.is_stale()).collect()
    }

    /// Destroyed lane branches that still have recorded work (never "empty").
    pub fn destroyed_lanes(&self) -> Vec<&LaneBranchTip> {
        self.lanes
            .iter()
            .filter(|lane| lane.is_destroyed_but_recorded())
            .collect()
    }
}

/// The one read-only branch/tip projection for a mission.
pub fn mission_branch_tips(
    workspace: &MissionWorkspace,
    manifest: Option<&LanesManifest>,
) -> Result<MissionBranchTips, WorkspaceError> {
    let repository = workspace.workspace().repository();
    let target_branch = workspace.target_branch()?;
    let target = BranchTip {
        tip: read_tip(repository, &target_branch)?,
        branch: target_branch,
    };
    let mission_branch = match workspace.declared_mission_branch() {
        Some(branch) => Some(BranchTip {
            tip: read_tip(repository, &branch)?,
            branch,
        }),
        None => None,
    };
    let coordination = match workspace.coordination_branch() {
        Some(branch) if routes_through_coordination(workspace.topology()) => Some(BranchTip {
            tip: read_tip(repository, &branch)?,
            branch,
        }),
        _ => None,
    };
    let planning_commit =
        match manifest.and_then(|manifest| manifest.planning_commit_sha.as_deref()) {
            Some(sha) => Some(Oid::parse(sha)?),
            None => None,
        };
    let mut lanes = Vec::new();
    if let Some(manifest) = manifest {
        for lane in &manifest.lanes {
            if lane.lane_id == PLANNING_LANE_ID {
                continue;
            }
            let branch =
                crate::naming::code_lane_branch_name(&manifest.mission_slug, &lane.lane_id)?;
            lanes.push(LaneBranchTip {
                lane_id: lane.lane_id.clone(),
                tip: read_tip(repository, &branch)?,
                recorded_tip: read_lane_tip(repository, &branch)?,
                branch,
            });
        }
    }
    Ok(MissionBranchTips {
        target,
        mission_branch,
        coordination,
        planning_commit,
        lanes,
    })
}

/// One checkout that holds a branch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckoutOccupancy {
    /// Checkout path.
    pub path: PathBuf,
    /// Whether this is the repository's primary checkout.
    pub is_primary: bool,
    /// Branch checked out here, when HEAD is not detached.
    pub branch: Option<String>,
    /// HEAD commit, when the checkout has one.
    pub head: Option<String>,
    /// Every status entry, as `XY path`.
    pub dirty_entries: Vec<String>,
}

impl CheckoutOccupancy {
    /// Is the checkout free of local state?
    pub fn is_clean(&self) -> bool {
        self.dirty_entries.is_empty()
    }
}

/// Occupancy of one lane's branch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaneOccupancy {
    /// Lane id.
    pub lane_id: String,
    /// Lane branch.
    pub branch: String,
    /// Live branch tip.
    pub tip: Option<Oid>,
    /// Recorded lane-work-tip ref.
    pub recorded_tip: Option<Oid>,
    /// Registered worktrees holding the branch.
    pub checkouts: Vec<CheckoutOccupancy>,
    /// A directory at the predicted lane path that Git does not register.
    pub stray_directory: Option<PathBuf>,
}

impl LaneOccupancy {
    /// Is the lane busy anywhere (checked out or occupied by a husk)?
    pub fn is_occupied(&self) -> bool {
        !self.checkouts.is_empty() || self.stray_directory.is_some()
    }

    /// Is every registered checkout clean?
    pub fn is_clean(&self) -> bool {
        self.checkouts.iter().all(CheckoutOccupancy::is_clean)
    }
}

/// Occupancy of the shared repository-root (planning) checkout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoRootOccupancy {
    /// Work packages the manifest assigns to the planning lane.
    pub planning_wps: Vec<String>,
    /// The primary checkout's branch.
    pub branch: Option<String>,
    /// The primary checkout's HEAD.
    pub head: Option<String>,
    /// Every status entry in the primary checkout, as `XY path`.
    pub dirty_entries: Vec<String>,
}

impl RepoRootOccupancy {
    /// Is the shared planning checkout clean?
    pub fn is_clean(&self) -> bool {
        self.dirty_entries.is_empty()
    }
}

/// A complete, read-only occupancy picture for one mission.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OccupancyReport {
    /// Target branch tip.
    pub target: BranchTip,
    /// Coordination branch tip and its registered checkout.
    pub coordination: Option<LaneOccupancy>,
    /// Code lanes.
    pub lanes: Vec<LaneOccupancy>,
    /// The shared repository-root checkout.
    pub repo_root: RepoRootOccupancy,
}

impl OccupancyReport {
    /// The lane occupancy for `lane_id`.
    pub fn lane(&self, lane_id: &str) -> Option<&LaneOccupancy> {
        self.lanes.iter().find(|lane| lane.lane_id == lane_id)
    }

    /// Every occupied lane.
    pub fn occupied_lanes(&self) -> Vec<&LaneOccupancy> {
        self.lanes
            .iter()
            .filter(|lane| lane.is_occupied())
            .collect()
    }
}

/// Read the full occupancy picture without mutating anything.
pub fn mission_occupancy(
    workspace: &MissionWorkspace,
    manifest: Option<&LanesManifest>,
) -> Result<OccupancyReport, WorkspaceError> {
    let repository = workspace.workspace().repository();
    let worktrees = repository.worktrees()?;
    let primary = workspace.workspace().primary_root().to_path_buf();
    let target_branch = workspace.target_branch()?;
    let target = BranchTip {
        tip: read_tip(repository, &target_branch)?,
        branch: target_branch,
    };

    let coordination = workspace
        .coordination_branch()
        .map(|branch| -> Result<LaneOccupancy, WorkspaceError> {
            let predicted = workspace.coord_worktree_path();
            Ok(LaneOccupancy {
                recorded_tip: read_lane_tip(repository, &branch)?,
                tip: read_tip(repository, &branch)?,
                checkouts: checkouts_of(&worktrees, &primary, &branch)?,
                stray_directory: stray_directory(predicted.as_deref(), &worktrees),
                lane_id: "coordination".to_owned(),
                branch,
            })
        })
        .transpose()?;

    let mut lanes = Vec::new();
    let mut planning_wps = Vec::new();
    if let Some(manifest) = manifest {
        for lane in &manifest.lanes {
            if lane.lane_id == PLANNING_LANE_ID {
                planning_wps.extend(lane.wp_ids.iter().map(ToString::to_string));
                continue;
            }
            let branch =
                crate::naming::code_lane_branch_name(&manifest.mission_slug, &lane.lane_id)?;
            let predicted =
                crate::naming::worktree_path(&primary, &manifest.mission_slug, &lane.lane_id).ok();
            lanes.push(LaneOccupancy {
                recorded_tip: read_lane_tip(repository, &branch)?,
                tip: read_tip(repository, &branch)?,
                checkouts: checkouts_of(&worktrees, &primary, &branch)?,
                stray_directory: stray_directory(predicted.as_deref(), &worktrees),
                lane_id: lane.lane_id.clone(),
                branch,
            });
        }
    }

    let primary_checkout = GitRepository::open(&primary)?;
    let repo_root = RepoRootOccupancy {
        planning_wps,
        branch: primary_checkout.current_branch()?,
        head: primary_checkout.head_commit()?,
        dirty_entries: status_lines(&primary_checkout)?,
    };

    Ok(OccupancyReport {
        target,
        coordination,
        lanes,
        repo_root,
    })
}

fn read_tip(repository: &GitRepository, branch: &str) -> Result<Option<Oid>, WorkspaceError> {
    let branch = BranchName::parse(branch)?;
    Ok(repository.read_branch_tip(&branch)?)
}

fn checkouts_of(
    worktrees: &[sk_git::Worktree],
    primary: &Path,
    branch: &str,
) -> Result<Vec<CheckoutOccupancy>, WorkspaceError> {
    let mut checkouts = Vec::new();
    for worktree in worktrees {
        if worktree.branch() != Some(branch) {
            continue;
        }
        let path = worktree.path().to_path_buf();
        let checkout = GitRepository::open(&path)?;
        checkouts.push(CheckoutOccupancy {
            is_primary: same_path(&path, primary),
            branch: checkout.current_branch()?,
            head: checkout.head_commit()?,
            dirty_entries: status_lines(&checkout)?,
            path,
        });
    }
    Ok(checkouts)
}

fn status_lines(repository: &GitRepository) -> Result<Vec<String>, WorkspaceError> {
    let entries = repository.status()?;
    Ok(entries
        .iter()
        .map(|entry| {
            format!(
                "{}{} {}",
                entry.index(),
                entry.worktree(),
                entry.path().display_lossy()
            )
        })
        .collect())
}

fn stray_directory(predicted: Option<&Path>, worktrees: &[sk_git::Worktree]) -> Option<PathBuf> {
    let predicted = predicted?;
    if !predicted.exists() {
        return None;
    }
    let registered = worktrees.iter().any(|worktree| {
        std::fs::canonicalize(worktree.path()).unwrap_or_else(|_| worktree.path().to_path_buf())
            == std::fs::canonicalize(predicted).unwrap_or_else(|_| predicted.to_path_buf())
    });
    if registered {
        None
    } else {
        Some(predicted.to_path_buf())
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    std::fs::canonicalize(left).unwrap_or_else(|_| left.to_path_buf())
        == std::fs::canonicalize(right).unwrap_or_else(|_| right.to_path_buf())
}

/// The lane-tip ref name for `branch`.
pub fn lane_tip_ref(branch: &str) -> Result<RefName, WorkspaceError> {
    Ok(RefName::parse(&format!("{LANE_TIP_REF_PREFIX}{branch}"))?)
}

/// Read the recorded lane work tip for `branch`.
pub fn read_lane_tip(
    repository: &GitRepository,
    branch: &str,
) -> Result<Option<Oid>, WorkspaceError> {
    Ok(repository.read_ref(&lane_tip_ref(branch)?)?)
}

/// How a lane-tip recording resolved.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LaneTipOutcome {
    /// The ref did not exist and was created.
    Created,
    /// The ref advanced to a newer tip.
    Advanced,
    /// The ref already pointed at this commit; nothing was written.
    Unchanged,
    /// The recorded tip is ahead of the observed one; the newer record is kept.
    KeptNewerRecord,
}

/// Evidence of one lane-tip recording.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaneTipRecord {
    /// Branch whose tip was recorded.
    pub branch: String,
    /// The bookkeeping ref that holds the record.
    pub reference: String,
    /// The value observed before writing.
    pub previous: Option<Oid>,
    /// The tip recorded now.
    pub recorded: Oid,
    /// What the write did.
    pub outcome: LaneTipOutcome,
}

/// Record `branch`'s current tip, refusing to move a record sideways.
///
/// Returns `Ok(None)` when `branch` does not resolve: there is nothing to
/// record. A recorded tip that is not related to the observed one is a typed
/// refusal — that shape means the branch history was rewritten, and silently
/// overwriting the only surviving record of the old work would lose it.
pub fn record_lane_tip(
    repository: &GitRepository,
    branch: &str,
) -> Result<Option<LaneTipRecord>, WorkspaceError> {
    let branch_name = BranchName::parse(branch)?;
    let Some(tip) = repository.read_branch_tip(&branch_name)? else {
        return Ok(None);
    };
    Ok(Some(record_lane_tip_at(repository, branch, &tip)?))
}

/// Record an explicit commit as `branch`'s lane work tip.
pub fn record_lane_tip_at(
    repository: &GitRepository,
    branch: &str,
    tip: &Oid,
) -> Result<LaneTipRecord, WorkspaceError> {
    let reference = lane_tip_ref(branch)?;
    let observed = repository.read_ref(&reference)?;
    let outcome = match &observed {
        None => {
            let plan = repository.plan_ref_transaction(
                "spec-kitty: record lane work tip",
                vec![RefChange::create(reference.clone(), tip.clone())],
            )?;
            repository.apply_ref_transaction(&plan)?;
            LaneTipOutcome::Created
        }
        Some(previous) if previous == tip => LaneTipOutcome::Unchanged,
        Some(previous) => {
            if repository.is_ancestor(previous, tip)? {
                let plan = repository.plan_ref_transaction(
                    "spec-kitty: advance lane work tip",
                    vec![RefChange::set(
                        reference.clone(),
                        tip.clone(),
                        Some(previous.clone()),
                    )],
                )?;
                repository.apply_ref_transaction(&plan)?;
                LaneTipOutcome::Advanced
            } else if repository.is_ancestor(tip, previous)? {
                LaneTipOutcome::KeptNewerRecord
            } else {
                return Err(WorkspaceError::CoordRefused {
                    code: "LANE_TIP_HISTORY_REWRITTEN",
                    message: format!(
                        "the recorded lane work tip {previous} for {branch} is unrelated to the observed tip {tip}; refusing to overwrite the only record of the earlier work"
                    ),
                });
            }
        }
    };
    Ok(LaneTipRecord {
        branch: branch.to_owned(),
        reference: reference.as_str().to_owned(),
        previous: observed,
        recorded: tip.clone(),
        outcome,
    })
}

/// The canonical coordination worktree path for a mission, if it routes through
/// coordination.
pub fn coordination_worktree_path(workspace: &MissionWorkspace) -> Option<PathBuf> {
    workspace.coord_worktree_path()
}

/// The canonical coordination Mission directory inside its worktree.
///
/// Composed with the VERBATIM coordination grammar
/// ([`MissionWorkspace::coord_mission_dir_name`]), never the NNN-stripping
/// primary composer: a legacy `NNN-` slug's coordination home keeps its prefix.
pub fn coordination_mission_dir(workspace: &MissionWorkspace) -> Option<PathBuf> {
    workspace.coord_worktree_path().map(|root| {
        root.join(SPECS_DIR_NAME)
            .join(workspace.coord_mission_dir_name())
    })
}
