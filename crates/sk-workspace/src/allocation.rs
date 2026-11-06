//! Real lane-worktree allocation, reuse, crash recovery, and retention.
//!
//! Every mutation goes through a typed `sk-git` plan: plan first (validating
//! preconditions against observed state), then apply (which re-reads the state
//! and refuses a stale plan). This crate never runs a Git process of its own.
//!
//! # Routes
//!
//! * **Reuse** — the worktree directory already exists. It must be clean, and
//!   an explicit `base` is refused (an existing lane cannot be re-parented).
//! * **Crash recovery** — the branch exists but the worktree directory is gone.
//!   The existing branch is attached; an explicit `base` is refused.
//! * **Fresh** — neither exists. The lane is cut from the topology-derived
//!   parent (coordination branch when present, else the mission integration
//!   branch), preferring `origin/<lane-branch>` when it exists and no explicit
//!   base was supplied.
//!
//! # Preconditions that refuse before any mutation
//!
//! * the work package must be in a code lane (the planning lane has no worktree);
//! * the lane branch must not be the protected/default branch;
//! * a `single_branch` mission must not have code lanes (Invariant T-1);
//! * an explicit base on a dependency-bearing lane, or on reuse/recovery, is
//!   unhonorable;
//! * an explicit base detached from the recorded planning commit is refused.
//!
//! # The journal
//!
//! An intent record is written before the worktree mutation. On success the
//! record is marked applied; on failure it is abandoned. [`LaneAllocator::recover_interrupted`]
//! reconciles a pending intent with observed Git state: it completes an intent
//! whose branch and worktree both exist, re-attaches a branch whose worktree is
//! missing, clears an intent that never mutated, and reports anything else as
//! inconsistent instead of guessing.
//!
//! # Retention
//!
//! Retiring a lane never discards user state. A checkout with any custom
//! (non-spec-kitty) status entry is refused, and a removal plan permits only
//! spec-kitty's own untracked runtime residue. An interrupted allocation whose
//! path already holds unregistered files is reported, not deleted.

use std::path::{Path, PathBuf};

use sk_git::{
    BranchAdvanceOptions, BranchDeletionOptions, BranchName, GitError, GitRepository, Oid,
    ProtectedBranches, RefName, StartPoint, StatusOptions, UntrackedMode, WorktreeAddPlan,
    WorktreeDiscardPolicy, WorktreeStart,
};
use sk_kernel::WpId;
use sk_mission::MissionTopology;

use crate::error::WorkspaceError;
use crate::journal::{AllocationJournal, AllocationRecord};
use crate::lanes::{ExecutionLane, LanesManifest};
use crate::naming::{PLANNING_LANE_ID, code_lane_branch_name, lane_branch_name, worktree_path};

/// The derived-parent source for a lane allocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LaneTopology {
    /// The mission declares a coordination branch.
    Coord,
    /// The mission uses the legacy mission integration branch.
    Legacy,
}

impl LaneTopology {
    /// The wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Coord => "coord",
            Self::Legacy => "legacy",
        }
    }
}

/// The closed set of routes that can allocate or recover a lane worktree.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AllocationRoute {
    /// Fresh cut from the coordination branch.
    FreshCoord,
    /// Fresh cut from the mission integration branch.
    FreshLegacy,
    /// The worktree already exists and is reused.
    Reuse,
    /// The branch exists but its worktree directory was lost.
    CrashRecovery,
}

impl AllocationRoute {
    /// The wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FreshCoord => "fresh_coord",
            Self::FreshLegacy => "fresh_legacy",
            Self::Reuse => "reuse",
            Self::CrashRecovery => "crash_recovery",
        }
    }
}

/// The resolved parent-ref decision for a lane allocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaneBaseDecision {
    /// The ref the lane worktree branches from (origin-aware on a fresh route).
    pub parent_ref: String,
    /// Was an explicit base supplied?
    pub base_honored: bool,
    /// The active route.
    pub route: AllocationRoute,
    /// The derived-parent source.
    pub topology: LaneTopology,
    /// The pre-override topology parent (the mission integration branch).
    pub topology_parent_ref: String,
}

/// A completed allocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaneAllocation {
    /// Absolute worktree path.
    pub path: PathBuf,
    /// The lane branch.
    pub branch: BranchName,
    /// The route taken.
    pub route: AllocationRoute,
    /// The parent-ref decision.
    pub decision: LaneBaseDecision,
}

/// One automatic repair performed by recovery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecoveryRepair {
    /// The intent was already satisfied; it is now recorded as applied.
    Completed {
        /// Lane id.
        lane_id: String,
        /// Worktree path.
        path: PathBuf,
    },
    /// The branch existed and the worktree was re-attached.
    Reattached {
        /// Lane id.
        lane_id: String,
        /// Worktree path.
        path: PathBuf,
    },
    /// The intent never mutated anything and was cleared.
    Cleared {
        /// Lane id.
        lane_id: String,
        /// Worktree path.
        path: PathBuf,
    },
}

/// An interrupted intent that cannot be reconciled automatically.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryConflict {
    /// The pending record.
    pub record: AllocationRecord,
    /// Why it is inconsistent.
    pub detail: String,
}

/// The outcome of reconciliation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RecoveryReport {
    /// Automatic repairs.
    pub repairs: Vec<RecoveryRepair>,
    /// Intents that need an operator decision.
    pub conflicts: Vec<RecoveryConflict>,
}

/// How a checkout's status entries split between spec-kitty runtime residue and
/// user state.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RetentionReport {
    /// Paths owned by spec-kitty (safe to discard when retiring).
    pub owned_residue: Vec<String>,
    /// Paths that belong to the user or to tracked content.
    pub custom: Vec<String>,
}

impl RetentionReport {
    /// Is it safe to discard this checkout's local state?
    pub fn is_retainable(&self) -> bool {
        self.custom.is_empty()
    }
}

/// The inputs to a fresh lane allocation.
///
/// Grouping them keeps the route helper readable and keeps every value
/// explicit; nothing here is optional-by-omission except `base`.
struct FreshLane<'a> {
    manifest: &'a LanesManifest,
    topology: LaneTopology,
    coordination_branch: Option<&'a str>,
    lane: &'a ExecutionLane,
    path: PathBuf,
    branch: BranchName,
    base: Option<&'a str>,
}

/// Plans and applies lane allocations for one repository.
pub struct LaneAllocator<'a> {
    repository: &'a GitRepository,
    primary_root: PathBuf,
    protected: ProtectedBranches,
    journal: AllocationJournal,
}

impl<'a> LaneAllocator<'a> {
    /// Build an allocator.
    ///
    /// The journal is anchored under the repository's Git common directory, so
    /// every worktree of the repository shares one record of intent.
    pub fn new(
        repository: &'a GitRepository,
        protected: ProtectedBranches,
        mission_slug: &str,
    ) -> Result<Self, WorkspaceError> {
        let common_dir = repository.git_dirs()?.common_dir().to_path_buf();
        let workspace = crate::workspace::ProjectWorkspace::open(repository.root())?;
        Ok(Self {
            repository,
            primary_root: workspace.primary_root().to_path_buf(),
            protected,
            journal: AllocationJournal::new(&common_dir, mission_slug),
        })
    }

    /// The allocation journal.
    pub fn journal(&self) -> &AllocationJournal {
        &self.journal
    }

    /// Resolve the placement (path + branch) for `lane_id`, read-only.
    ///
    /// This is the ONE lane placement decision; the allocator and read-only
    /// mirrors share it so they cannot drift.
    pub fn predict(
        &self,
        mission_slug: &str,
        lane_id: &str,
    ) -> Result<(PathBuf, BranchName), WorkspaceError> {
        if lane_id == PLANNING_LANE_ID {
            return Err(WorkspaceError::RepoRootLaneHasNoWorktree {
                lane: lane_id.to_owned(),
            });
        }
        let path = worktree_path(&self.primary_root, mission_slug, lane_id)?;
        let branch = BranchName::parse(&code_lane_branch_name(mission_slug, lane_id)?)?;
        Ok((path, branch))
    }

    /// Allocate or reuse the worktree for the lane containing `wp_id`.
    ///
    /// See the module documentation for routes and preconditions.
    pub fn allocate(
        &self,
        manifest: &LanesManifest,
        stored_topology: Option<MissionTopology>,
        coordination_branch: Option<&str>,
        wp_id: &WpId,
        base: Option<&str>,
    ) -> Result<LaneAllocation, WorkspaceError> {
        let lane = manifest
            .lane_for_wp(wp_id)
            .ok_or_else(|| WorkspaceError::LaneNotFound {
                wp: wp_id.to_string(),
            })?;
        guard_topology_matches_manifest(stored_topology, manifest)?;
        if lane.is_planning() {
            return Err(WorkspaceError::RepoRootLaneHasNoWorktree {
                lane: lane.lane_id.clone(),
            });
        }

        let (path, branch) = self.predict(&manifest.mission_slug, &lane.lane_id)?;
        let lane_topology = topology_from_stored(stored_topology);
        if lane_topology == LaneTopology::Coord && coordination_branch.is_none() {
            return Err(WorkspaceError::InvalidArtifact {
                artifact: "coordination_branch",
                detail: format!(
                    "mission {} stores a coordination-routing topology but records no coordination_branch",
                    manifest.mission_slug
                ),
            });
        }
        // The branch is never the protected/default branch. Checking here, at
        // the single allocation entry point, means no route can be talked into
        // writing it.
        if self.protected.contains(&branch) {
            return Err(WorkspaceError::OwnedRefused {
                code: "OWNED_BRANCH_REFUSED",
                message: format!(
                    "lane branch {branch} is on the protected-branch list; refusing to allocate it"
                ),
            });
        }
        if branch.as_str() == manifest.target_branch {
            return Err(WorkspaceError::OwnedRefused {
                code: "OWNED_BRANCH_REFUSED",
                message: format!(
                    "lane branch {branch} is the mission target branch; refusing to allocate a lane on it"
                ),
            });
        }

        if path.exists() {
            return self.reuse(manifest, lane_topology, lane, path, branch, base);
        }

        if self.repository.read_branch_tip(&branch)?.is_some() {
            return self.recover_branch(manifest, lane_topology, lane, path, branch, base);
        }

        self.fresh(FreshLane {
            manifest,
            topology: lane_topology,
            coordination_branch,
            lane,
            path,
            branch,
            base,
        })
    }

    fn reuse(
        &self,
        manifest: &LanesManifest,
        lane_topology: LaneTopology,
        lane: &ExecutionLane,
        path: PathBuf,
        branch: BranchName,
        base: Option<&str>,
    ) -> Result<LaneAllocation, WorkspaceError> {
        if let Some(base) = base {
            return Err(WorkspaceError::UnhonorableBase {
                base: base.to_owned(),
                route: AllocationRoute::Reuse.as_str(),
                wp: lane.wp_ids[0].to_string(),
                detail: "an existing lane worktree cannot be re-parented".to_owned(),
            });
        }
        // A lane is clean when the only local state is spec-kitty's own
        // runtime residue. This mirrors upstream's excluded index residue while
        // still refusing any user file or tracked modification.
        let retention = self.classify_retention(&path)?;
        if !retention.is_retainable() {
            return Err(WorkspaceError::DirtyWorktree {
                lane: lane.lane_id.clone(),
                path,
            });
        }
        Ok(LaneAllocation {
            path,
            branch,
            route: AllocationRoute::Reuse,
            decision: LaneBaseDecision {
                parent_ref: manifest.mission_branch.clone(),
                base_honored: false,
                route: AllocationRoute::Reuse,
                topology: lane_topology,
                topology_parent_ref: manifest.mission_branch.clone(),
            },
        })
    }

    fn recover_branch(
        &self,
        manifest: &LanesManifest,
        lane_topology: LaneTopology,
        lane: &ExecutionLane,
        path: PathBuf,
        branch: BranchName,
        base: Option<&str>,
    ) -> Result<LaneAllocation, WorkspaceError> {
        if let Some(base) = base {
            return Err(WorkspaceError::UnhonorableBase {
                base: base.to_owned(),
                route: AllocationRoute::CrashRecovery.as_str(),
                wp: lane.wp_ids[0].to_string(),
                detail: "a branch that already exists cannot be re-parented by re-attaching"
                    .to_owned(),
            });
        }
        self.journal.begin(AllocationRecord::intent(
            &manifest.mission_slug,
            &lane.lane_id,
            branch.as_str(),
            &path,
            &manifest.mission_branch,
        ))?;
        let plan =
            self.repository
                .plan_worktree_add(path.clone(), &branch, WorktreeStart::ExistingBranch);
        let plan = match plan {
            Ok(plan) => plan,
            Err(error) => {
                self.journal
                    .abandon(&lane.lane_id, &path, &error.to_string())?;
                return Err(WorkspaceError::Git(error));
            }
        };
        match self.repository.apply_worktree_add(&plan) {
            Ok(_) => {
                self.journal
                    .applied(&lane.lane_id, &path, branch.as_str())?;
                Ok(LaneAllocation {
                    path,
                    branch,
                    route: AllocationRoute::CrashRecovery,
                    decision: LaneBaseDecision {
                        parent_ref: manifest.mission_branch.clone(),
                        base_honored: false,
                        route: AllocationRoute::CrashRecovery,
                        topology: lane_topology,
                        topology_parent_ref: manifest.mission_branch.clone(),
                    },
                })
            }
            Err(error) => {
                self.journal
                    .abandon(&lane.lane_id, &path, &error.to_string())?;
                Err(WorkspaceError::Git(error))
            }
        }
    }

    fn fresh(&self, request: FreshLane<'_>) -> Result<LaneAllocation, WorkspaceError> {
        let FreshLane {
            manifest,
            topology,
            coordination_branch,
            lane,
            path,
            branch,
            base,
        } = request;
        let coordination_branch = match topology {
            LaneTopology::Coord => {
                Some(coordination_branch.expect("checked by allocate").to_owned())
            }
            LaneTopology::Legacy => None,
        };
        let topology_parent_ref = base.map(str::to_owned).unwrap_or_else(|| {
            coordination_branch
                .clone()
                .unwrap_or_else(|| manifest.mission_branch.clone())
        });

        // Preconditions that must refuse BEFORE any branch or worktree exists.
        if base.is_some() && !lane.depends_on_lanes.is_empty() {
            return Err(WorkspaceError::UnhonorableBase {
                base: base.unwrap_or_default().to_owned(),
                route: if coordination_branch.is_some() {
                    AllocationRoute::FreshCoord.as_str()
                } else {
                    AllocationRoute::FreshLegacy.as_str()
                },
                wp: lane.wp_ids[0].to_string(),
                detail: "a lane with dependency lanes cannot take an explicit base".to_owned(),
            });
        }
        if let (Some(base), Some(planning_sha)) = (base, manifest.planning_commit_sha.as_deref()) {
            self.guard_planning_ancestry(base, planning_sha, lane)?;
        }

        let route = if coordination_branch.is_some() {
            AllocationRoute::FreshCoord
        } else {
            AllocationRoute::FreshLegacy
        };
        // Ensure the mission integration branch exists before cutting the lane.
        // Creating a branch from the target is not a write to the target.
        let integration_ref = coordination_branch
            .clone()
            .unwrap_or_else(|| manifest.mission_branch.clone());
        self.ensure_integration_branch(&integration_ref, &manifest.target_branch)?;

        let parent_ref = self.resolve_parent_ref(base, &branch, &topology_parent_ref)?;
        let parent_oid = self.resolve_ref_oid(&parent_ref)?;

        self.journal.begin(AllocationRecord::intent(
            &manifest.mission_slug,
            &lane.lane_id,
            branch.as_str(),
            &path,
            &parent_ref,
        ))?;

        let plan: Result<WorktreeAddPlan, GitError> = self.repository.plan_worktree_add(
            path.clone(),
            &branch,
            WorktreeStart::NewBranch(StartPoint::Oid(parent_oid)),
        );
        let plan = match plan {
            Ok(plan) => plan,
            Err(error) => {
                self.journal
                    .abandon(&lane.lane_id, &path, &error.to_string())?;
                return Err(WorkspaceError::Git(error));
            }
        };
        match self.repository.apply_worktree_add(&plan) {
            Ok(_) => {
                self.journal
                    .applied(&lane.lane_id, &path, branch.as_str())?;
                Ok(LaneAllocation {
                    path,
                    branch,
                    route,
                    decision: LaneBaseDecision {
                        parent_ref,
                        base_honored: base.is_some(),
                        route,
                        topology,
                        topology_parent_ref,
                    },
                })
            }
            Err(error) => {
                // The worktree add may have created the branch before failing;
                // recovery reconciles that from the journal on the next run.
                self.journal
                    .abandon(&lane.lane_id, &path, &error.to_string())?;
                Err(WorkspaceError::Git(error))
            }
        }
    }

    fn guard_planning_ancestry(
        &self,
        base: &str,
        planning_sha: &str,
        lane: &ExecutionLane,
    ) -> Result<(), WorkspaceError> {
        let base_oid = match self.resolve_ref_oid(base) {
            Ok(oid) => oid,
            Err(_) => {
                return Err(WorkspaceError::UnhonorableBase {
                    base: base.to_owned(),
                    route: "detached_base",
                    wp: lane.wp_ids[0].to_string(),
                    detail: "the supplied base does not resolve".to_owned(),
                });
            }
        };
        let planning_oid =
            Oid::parse(planning_sha).map_err(|error| WorkspaceError::InvalidArtifact {
                artifact: "planning_commit_sha",
                detail: error.to_string(),
            })?;
        // Fail-closed common-ancestor approximation: the two commits must be
        // directly related. `sk-git` does not expose `merge-base` yet, so a
        // shared-but-unrelated ancestor is refused rather than guessed at.
        let related = self.repository.is_ancestor(&base_oid, &planning_oid)?
            || self.repository.is_ancestor(&planning_oid, &base_oid)?;
        if !related {
            return Err(WorkspaceError::UnhonorableBase {
                base: base.to_owned(),
                route: "detached_base",
                wp: lane.wp_ids[0].to_string(),
                detail: format!(
                    "base {base} shares no direct ancestry with the recorded planning commit {planning_sha}"
                ),
            });
        }
        Ok(())
    }

    fn ensure_integration_branch(
        &self,
        branch_name: &str,
        target_branch: &str,
    ) -> Result<(), WorkspaceError> {
        let branch = BranchName::parse(branch_name)?;
        if self.protected.contains(&branch) {
            return Err(WorkspaceError::OwnedRefused {
                code: "OWNED_BRANCH_REFUSED",
                message: format!(
                    "mission integration branch {branch} is protected; refusing to create or write it"
                ),
            });
        }
        if self.repository.read_branch_tip(&branch)?.is_some() {
            return Ok(());
        }
        let target = BranchName::parse(target_branch)?;
        let target_tip = self.repository.read_branch_tip(&target)?.ok_or_else(|| {
            WorkspaceError::InvalidArtifact {
                artifact: "target branch",
                detail: format!("target branch {target_branch} does not resolve locally"),
            }
        })?;
        let plan = match self.repository.plan_branch_creation(
            &branch,
            StartPoint::Oid(target_tip),
            "spec-kitty: create mission integration branch",
        ) {
            Ok(plan) => plan,
            Err(GitError::BranchAlreadyExists { .. }) => return Ok(()),
            Err(error) => return Err(WorkspaceError::Git(error)),
        };
        match self.repository.apply_branch_creation(&plan) {
            Ok(_) => Ok(()),
            // A concurrent allocator may have won the race; treat the branch's
            // existence as success, but never as an overwrite.
            Err(GitError::PlanStale { .. })
                if self.repository.read_branch_tip(&branch)?.is_some() =>
            {
                Ok(())
            }
            Err(error) => Err(WorkspaceError::Git(error)),
        }
    }

    fn resolve_parent_ref(
        &self,
        base: Option<&str>,
        lane_branch: &BranchName,
        topology_parent_ref: &str,
    ) -> Result<String, WorkspaceError> {
        if let Some(base) = base {
            return Ok(base.to_owned());
        }
        // Origin preference on a fresh cut: an approved lane pushed by a
        // teammate but absent locally must root the cut, not be shadowed by the
        // local mission branch.
        let origin = format!("refs/remotes/origin/{}", lane_branch.as_str());
        if let Ok(reference) = RefName::parse(&origin)
            && self.repository.read_ref(&reference)?.is_some()
        {
            return Ok(origin);
        }
        Ok(topology_parent_ref.to_owned())
    }

    /// Resolve a ref spelling to an object id.
    ///
    /// Accepts a fully-qualified ref, a remote-tracking ref (`origin/x`), or a
    /// short local branch name. A missing ref is an error, never a fallback to
    /// another branch.
    pub fn resolve_ref_oid(&self, reference: &str) -> Result<Oid, WorkspaceError> {
        if reference.starts_with("refs/") {
            let name = RefName::parse(reference)?;
            return self.repository.read_ref(&name)?.ok_or_else(|| {
                WorkspaceError::InvalidArtifact {
                    artifact: "base ref",
                    detail: format!("{reference:?} does not resolve to a commit"),
                }
            });
        }
        if let Ok(branch) = BranchName::parse(reference)
            && let Some(oid) = self.repository.read_branch_tip(&branch)?
        {
            return Ok(oid);
        }
        if let Some((remote, branch)) = reference.split_once('/') {
            let name = RefName::parse(&format!("refs/remotes/{remote}/{branch}"))?;
            if let Some(oid) = self.repository.read_ref(&name)? {
                return Ok(oid);
            }
        }
        Err(WorkspaceError::InvalidArtifact {
            artifact: "base ref",
            detail: format!("{reference:?} does not resolve to a commit"),
        })
    }

    /// Reconcile every pending journal intent with observed Git state.
    ///
    /// See the module documentation for the four outcomes. Nothing is deleted
    /// here: an unexpected on-disk path is reported, never wiped.
    pub fn recover_interrupted(&self) -> Result<RecoveryReport, WorkspaceError> {
        let registered = self.registered_worktrees()?;
        let mut report = RecoveryReport::default();
        for record in self.journal.pending()? {
            let branch = BranchName::parse(&record.branch)?;
            let branch_tip = self.repository.read_branch_tip(&branch)?;
            let registered_branch = registered
                .iter()
                .find(|(path, _)| path == &record.path)
                .map(|(_, branch)| branch.clone());

            match (branch_tip, registered_branch, record.path.exists()) {
                (Some(_), Some(branch_name), _) if branch_name == record.branch => {
                    self.journal
                        .applied(&record.lane_id, &record.path, &record.branch)?;
                    report.repairs.push(RecoveryRepair::Completed {
                        lane_id: record.lane_id,
                        path: record.path,
                    });
                }
                (Some(_), None, false) => {
                    let plan = self.repository.plan_worktree_add(
                        record.path.clone(),
                        &branch,
                        WorktreeStart::ExistingBranch,
                    )?;
                    self.repository.apply_worktree_add(&plan)?;
                    self.journal
                        .applied(&record.lane_id, &record.path, &record.branch)?;
                    report.repairs.push(RecoveryRepair::Reattached {
                        lane_id: record.lane_id,
                        path: record.path,
                    });
                }
                (None, None, false) => {
                    self.journal.abandon(
                        &record.lane_id,
                        &record.path,
                        "recovery found neither the branch nor the worktree; the intent never mutated",
                    )?;
                    report.repairs.push(RecoveryRepair::Cleared {
                        lane_id: record.lane_id,
                        path: record.path,
                    });
                }
                (Some(_), Some(_), _) => report.conflicts.push(RecoveryConflict {
                    record,
                    detail: "the path is registered to a different branch".to_owned(),
                }),
                (None, _, true) => report.conflicts.push(RecoveryConflict {
                    record,
                    detail:
                        "the path exists but its branch is missing; refusing to delete unregistered content"
                            .to_owned(),
                }),
                (Some(_), None, true) => report.conflicts.push(RecoveryConflict {
                    record,
                    detail: "the branch exists and the unregistered path is non-empty; refusing to reuse it"
                        .to_owned(),
                }),
                (None, Some(_), _) => report.conflicts.push(RecoveryConflict {
                    record,
                    detail: "a worktree is registered but its branch is missing".to_owned(),
                }),
            }
        }
        Ok(report)
    }

    fn registered_worktrees(&self) -> Result<Vec<(PathBuf, String)>, WorkspaceError> {
        let mut registered = Vec::new();
        for worktree in self.repository.worktrees()? {
            if let Some(branch) = worktree.branch() {
                registered.push((worktree.path().to_path_buf(), branch.to_owned()));
            }
        }
        Ok(registered)
    }

    /// Classify a checkout's local state into owned residue and custom files.
    pub fn classify_retention(&self, path: &Path) -> Result<RetentionReport, WorkspaceError> {
        let handle = GitRepository::open(path)?;
        let entries = handle.status_with(&StatusOptions {
            untracked: UntrackedMode::All,
            ignored: true,
        })?;
        let mut report = RetentionReport::default();
        for entry in entries {
            let rendered = entry.path().to_string();
            if is_spec_kitty_residue(&rendered) && matches!(entry.index(), '?' | '!') {
                report.owned_residue.push(rendered);
            } else {
                report.custom.push(rendered);
            }
        }
        Ok(report)
    }

    /// Retire a lane worktree, retaining user state.
    ///
    /// Refuses when any custom status entry is present. Otherwise removes the
    /// worktree registration with [`WorktreeDiscardPolicy::AllowAll`] *after* the
    /// retention screen proved every local entry is spec-kitty residue. The lane
    /// branch is retained: deleting it requires a separate, guarded decision
    /// through [`Self::retire_branch`].
    pub fn retire_worktree(&self, lane_id: &str, path: &Path) -> Result<(), WorkspaceError> {
        let retention = self.classify_retention(path)?;
        if !retention.is_retainable() {
            return Err(WorkspaceError::RetentionRefused {
                lane: lane_id.to_owned(),
                path: path.to_path_buf(),
                reason: format!("custom state present: {}", retention.custom.join(", ")),
            });
        }
        let head = GitRepository::open(path)?.head_commit()?;
        let expected_head = head.as_deref().map(Oid::parse).transpose()?;
        let plan = self.repository.plan_worktree_removal(
            path,
            expected_head.as_ref(),
            None,
            WorktreeDiscardPolicy::AllowAll,
        )?;
        self.repository.apply_worktree_removal(&plan)?;
        Ok(())
    }

    /// Delete a retired lane's local branch, refusing unique commits.
    ///
    /// This is a guarded deletion: a branch with commits not reachable from its
    /// creation base is refused, and the branch must not be checked out. The
    /// caller supplies the creation base it observed; passing it makes the
    /// unique-commit analysis explicit rather than inferred.
    pub fn retire_branch(
        &self,
        branch: &BranchName,
        creation_base: Option<&Oid>,
    ) -> Result<(), WorkspaceError> {
        if self.protected.contains(branch) {
            return Err(WorkspaceError::OwnedRefused {
                code: "OWNED_BRANCH_REFUSED",
                message: format!("refusing to delete protected branch {branch}"),
            });
        }
        let plan = self.repository.plan_branch_deletion(
            branch,
            &self.protected,
            creation_base,
            BranchDeletionOptions::default(),
            "spec-kitty: retire lane branch",
        )?;
        self.repository.apply_branch_deletion(&plan)?;
        Ok(())
    }

    /// Delete a retired lane's local branch, allowing its unique commits for a
    /// recorded reason.
    ///
    /// A squash landing carries the lane *content* without carrying the lane
    /// commits as ancestors, so the guarded unique-commit analysis legitimately
    /// reports them as unique. The caller that proved the content landed (with a
    /// real baseline commit) supplies that reason explicitly; nothing else may.
    pub fn retire_branch_allowing_unique_commits(
        &self,
        branch: &BranchName,
        creation_base: Option<&Oid>,
        reason: &str,
    ) -> Result<(), WorkspaceError> {
        if self.protected.contains(branch) {
            return Err(WorkspaceError::OwnedRefused {
                code: "OWNED_BRANCH_REFUSED",
                message: format!("refusing to delete protected branch {branch}"),
            });
        }
        let options = BranchDeletionOptions::default().allow_unique_commits(reason);
        let plan = self.repository.plan_branch_deletion(
            branch,
            &self.protected,
            creation_base,
            options,
            "spec-kitty: retire lane branch",
        )?;
        self.repository.apply_branch_deletion(&plan)?;
        Ok(())
    }

    /// Plan (but do not apply) a guarded fast-forward of a mission branch.
    ///
    /// Present so callers have one place that names the protection and
    /// fast-forward policy; this crate never advances the target/default
    /// branch. A protected target refuses here unless an explicit human reason
    /// is supplied through [`BranchAdvanceOptions`].
    pub fn plan_branch_advance(
        &self,
        branch: &BranchName,
        new_tip: Oid,
        expected_old: Option<Oid>,
        options: &BranchAdvanceOptions,
    ) -> Result<sk_git::BranchAdvancePlan, WorkspaceError> {
        Ok(self.repository.plan_branch_advance(
            branch,
            new_tip,
            expected_old,
            &self.protected,
            "spec-kitty: advance lane branch",
            options,
        )?)
    }
}

fn topology_from_stored(stored: Option<MissionTopology>) -> LaneTopology {
    match stored {
        Some(topology) if sk_mission::routes_through_coordination(topology) => LaneTopology::Coord,
        _ => LaneTopology::Legacy,
    }
}

/// Refuse a `single_branch` mission whose manifest has code lanes (T-1).
///
/// Runs before any allocation side effect. `stored_topology = None` means the
/// mission has no stored topology at all; there is no durable shape to violate,
/// so the guard is skipped rather than enforcing a manufactured default.
pub fn guard_topology_matches_manifest(
    stored_topology: Option<MissionTopology>,
    manifest: &LanesManifest,
) -> Result<(), WorkspaceError> {
    if stored_topology == Some(MissionTopology::SingleBranch) && manifest.has_code_lanes() {
        return Err(WorkspaceError::SingleBranchCodeLanes {
            mission: manifest.mission_slug.clone(),
        });
    }
    Ok(())
}

/// Is `path` spec-kitty's own runtime residue?
fn is_spec_kitty_residue(path: &str) -> bool {
    path.starts_with(".spec-kitty/") || path == ".spec-kitty"
}

/// The branch a lane executes on, re-exported for allocation callers.
pub fn lane_branch(
    manifest: &LanesManifest,
    lane: &ExecutionLane,
) -> Result<String, WorkspaceError> {
    lane_branch_name(
        &manifest.mission_slug,
        &lane.lane_id,
        &manifest.target_branch,
    )
}
