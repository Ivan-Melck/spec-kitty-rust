//! Planning-commit metadata/ancestry guards and dependency integration.
//!
//! A planning commit is the commit that carries `spec.md` / `plan.md` /
//! `tasks/` / `lanes.json` onto the mission target branch. Two things make it
//! dangerous:
//!
//! 1. **Routing.** It is a PRIMARY-partition commit. It must never transit the
//!    coordination branch (the add/add conflict that made `lanes.json` land in
//!    the wrong place), and it must never land on a protected target.
//! 2. **Ancestry and metadata.** Lanes are cut from the coordination branch,
//!    which is minted *before* planning exists, so the recorded planning commit
//!    is what later connects a lane to its planning lineage. A recorded pin that
//!    is orphaned or absent must be refused rather than merged, and an
//!    uncommitted `meta.json` that drops `coordination_branch` must never be
//!    swept into the commit (that would silently flatten the mission).
//!
//! # What is implemented
//!
//! * [`classify_recorded_pin`] — the shared four-way classification of a
//!   recorded `planning_commit_sha` against the target tip.
//! * [`PlanningCommitPlan`] — a fully validated, read-only plan; applying it is
//!   one path-scoped safe commit with a pinned parent and exact expected bytes.
//! * [`record_planning_commit`] — persist the landed commit into `lanes.json`.
//! * [`integrate_planning_commit`] / [`integrate_dependency_lanes`] — bring an
//!   already-approved planning commit or dependency lane tip into a lane.
//!
//! # Deliberate limitation
//!
//! Integration is fast-forward only. A fast-forward is exactly what a dependency
//! merge does when the destination has no unique commits (the normal fresh-lane
//! shape) and it is a real, verifyable ref advance. When both sides have moved,
//! a merge commit is required and `sk-git` has no three-way merge primitive yet;
//! this module returns [`IntegrationOutcome::MergeRequired`] naming both tips
//! and mutates nothing. See `COORDINATION-REPORT.md` for the exact patch.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use sk_git::{
    BranchAdvanceOptions, BranchName, CommitGuardCapability, ExpectedBlob, GitError, GitPath, Oid,
    ProtectedBranches, SafeCommitRequest,
};

use crate::artifact::{MissionArtifactKind, kind_for_mission_file};
use crate::coord::{CoordCommitOutcome, LandedCommit};
use crate::error::WorkspaceError;
use crate::lanes::{LanesManifest, LanesStore};
use crate::naming::PLANNING_LANE_ID;
use crate::protection::ProtectionPolicy;
use crate::workspace::{MissionWorkspace, SPECS_DIR_NAME};

/// Largest planning artifact this module will read for its exact-bytes pin.
pub const MAX_PLANNING_ARTIFACT_BYTES: u64 = 16 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Recorded planning pin
// ---------------------------------------------------------------------------

/// Classification of a recorded `planning_commit_sha` against a target tip.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PinClass {
    /// The recorded commit is an ancestor of (or equal to) the target tip.
    Advanced,
    /// The commit exists here but is not reachable from the target tip (a
    /// mid-mission rewrite). Merging it produces a dead base.
    Orphaned,
    /// The commit object is absent from this repository.
    Foreign,
    /// The question cannot be answered: no recorded pin, or no target tip.
    Indeterminate,
}

impl PinClass {
    /// Is this a pin a lane may merge?
    pub fn is_usable(self) -> bool {
        matches!(self, Self::Advanced | Self::Indeterminate)
    }

    /// The stable wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Advanced => "advanced",
            Self::Orphaned => "orphaned",
            Self::Foreign => "foreign",
            Self::Indeterminate => "indeterminate",
        }
    }
}

/// Classify `recorded` against `target_tip`.
///
/// `target_tip` must be the *planning target-branch tip*, never a lane HEAD:
/// a healthy fresh lane already contains the pin, so classifying against the
/// lane would report `Advanced` for every lane and hide an orphaned pin.
pub fn classify_recorded_pin(
    repository: &sk_git::GitRepository,
    recorded: Option<&str>,
    target_tip: Option<&Oid>,
) -> Result<PinClass, WorkspaceError> {
    let (Some(recorded), Some(target)) = (recorded, target_tip) else {
        return Ok(PinClass::Indeterminate);
    };
    let recorded = Oid::parse(recorded)?;
    if !repository.commit_exists(&recorded)? {
        return Ok(PinClass::Foreign);
    }
    if repository.is_ancestor(&recorded, target)? {
        Ok(PinClass::Advanced)
    } else {
        Ok(PinClass::Orphaned)
    }
}

// ---------------------------------------------------------------------------
// Planning commit
// ---------------------------------------------------------------------------

/// How a planning commit's `meta.json` demotion guard was satisfied.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MetaDemotionGuard {
    /// `meta.json` is not part of this commit.
    NotCommitted,
    /// No baseline was observable, so this is a first commit (allowed).
    NoBaseline,
    /// A baseline was supplied and the working copy keeps `coordination_branch`.
    Preserved,
}

/// A validated, read-only planning-commit plan.
#[derive(Clone, Debug)]
pub struct PlanningCommitPlan {
    destination: BranchName,
    message: String,
    paths: Vec<GitPath>,
    expected_bytes: Vec<ExpectedBlob>,
    target_tip: Oid,
    pin_class: PinClass,
    meta_guard: MetaDemotionGuard,
    protected: ProtectedBranches,
}

impl PlanningCommitPlan {
    /// The destination branch.
    pub fn destination(&self) -> &BranchName {
        &self.destination
    }

    /// The commit message.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The pinned parent (the target tip observed at plan time).
    pub fn target_tip(&self) -> &Oid {
        &self.target_tip
    }

    /// The repository-relative paths.
    pub fn paths(&self) -> &[GitPath] {
        &self.paths
    }

    /// The recorded pin classification.
    pub fn pin_class(&self) -> PinClass {
        self.pin_class
    }

    /// The demotion guard outcome.
    pub fn meta_guard(&self) -> MetaDemotionGuard {
        self.meta_guard
    }

    /// Build the plan for `workspace` without writing anything.
    ///
    /// `paths` are repository-relative POSIX paths (`kitty-specs/<dir>/...`).
    /// `baseline_meta` is the `meta.json` mapping the caller observed when it
    /// planned the commit; passing `None` means "no baseline" (a first commit).
    /// `recorded_planning_commit` is the manifest's current pin, if any.
    pub fn prepare(
        workspace: &MissionWorkspace,
        paths: &[String],
        message: &str,
        policy: &ProtectionPolicy,
        baseline_meta: Option<&Map<String, Value>>,
        recorded_planning_commit: Option<&str>,
    ) -> Result<Self, WorkspaceError> {
        let primary_root = workspace.workspace().primary_root().to_path_buf();
        let repository = workspace.workspace().repository();
        let destination = BranchName::parse(&workspace.target_branch()?)?;
        let target_tip = repository
            .read_branch_tip(&destination)?
            .ok_or_else(|| WorkspaceError::CoordRefused {
                code: "PLANNING_TARGET_MISSING",
                message: format!(
                    "planning commit destination {destination} does not exist; refusing to create a planning lineage on a branch the mission does not target"
                ),
            })?;
        let observed_head = repository.current_branch()?;
        if observed_head.as_deref() != Some(destination.as_str()) {
            return Err(WorkspaceError::CoordRefused {
                code: "PLANNING_COMMIT_HEAD_MISMATCH",
                message: format!(
                    "the primary checkout has {:?} checked out, not the planning target {destination}",
                    observed_head.unwrap_or_else(|| "<detached>".to_owned())
                ),
            });
        }
        let protected = policy.branch_set()?;
        if protected.contains(&destination) {
            return Err(WorkspaceError::CoordRefused {
                code: crate::policy::PROTECTED_BRANCH_REFUSED,
                message: format!(
                    "refusing the planning commit: destination {destination} is on the resolved protected-branch list"
                ),
            });
        }
        if paths.is_empty() {
            return Err(WorkspaceError::CoordRefused {
                code: "PLANNING_COMMIT_EMPTY",
                message: "a planning commit requires at least one path".to_owned(),
            });
        }

        // Artifact paths arrive with the mission's *stored* directory name
        // (`kitty-specs/<dir>/...`), which is not necessarily the composed
        // `<slug>-<mid8>` name. Classify against the directory that actually
        // appears on disk so the partition and meta guards see the real file.
        let mission_dir_name = workspace
            .mission_dir()
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
            .unwrap_or_else(|| workspace.canonical_mission_dir_name());
        let mut normalized = Vec::with_capacity(paths.len());
        let mut expected_bytes = Vec::new();
        let mut meta_guard = MetaDemotionGuard::NotCommitted;
        for supplied in paths {
            let copied = supplied.clone();
            let git_path = sk_git::normalize_safe_commit_path(&primary_root, Path::new(&copied))?;
            ensure_primary_partition(&git_path, &mission_dir_name)?;
            let absolute = primary_root.join(git_path.to_path_buf());
            let bytes = sk_kernel::read_bounded(&absolute, MAX_PLANNING_ARTIFACT_BYTES)?;
            if is_meta_json(&git_path, &mission_dir_name) {
                meta_guard = check_meta_demotion(&bytes, baseline_meta)?;
            }
            expected_bytes.push(ExpectedBlob::new(git_path.clone(), bytes));
            normalized.push(git_path);
        }

        let pin_class =
            classify_recorded_pin(repository, recorded_planning_commit, Some(&target_tip))?;
        if !pin_class.is_usable() {
            return Err(WorkspaceError::CoordRefused {
                code: "PLANNING_PIN_UNUSABLE",
                message: format!(
                    "recorded planning commit {} is {} against target {destination} ({target_tip}); refusing to build a planning commit on a dead pin",
                    recorded_planning_commit.unwrap_or("<none>"),
                    pin_class.as_str()
                ),
            });
        }

        Ok(Self {
            destination,
            message: message.to_owned(),
            paths: normalized,
            expected_bytes,
            target_tip,
            pin_class,
            meta_guard,
            protected,
        })
    }

    /// Apply the plan: one path-scoped safe commit on the target branch.
    pub fn apply(
        self,
        workspace: &MissionWorkspace,
    ) -> Result<PlanningCommitReceipt, WorkspaceError> {
        let repository = workspace.workspace().repository();
        let request = SafeCommitRequest::new(
            self.destination.clone(),
            self.message.clone(),
            self.paths.clone(),
        )
        .with_protection(self.protected.clone(), CommitGuardCapability::Standard)
        .with_expected_parent(self.target_tip.clone())
        .with_expected_bytes(self.expected_bytes.clone());
        let outcome = match repository.safe_commit(&request) {
            Ok(outcome) => CoordCommitOutcome::Landed(LandedCommit {
                commit: outcome.commit().clone(),
                destination: outcome.destination().as_str().to_owned(),
                worktree_root: outcome.worktree_root().to_path_buf(),
                committed_paths: outcome
                    .committed_paths()
                    .iter()
                    .map(GitPath::display_lossy)
                    .collect(),
                observed_tip: Some(self.target_tip.clone()),
                index_repair: None,
            }),
            Err(GitError::SafeCommitNothingToCommit { .. }) => CoordCommitOutcome::NoOp {
                head: self.target_tip.clone(),
                destination: self.destination.as_str().to_owned(),
            },
            Err(GitError::SafeCommitLanded {
                destination,
                worktree_root,
                detail,
                recovery,
                ..
            }) => CoordCommitOutcome::Landed(LandedCommit {
                commit: recovery.commit().clone(),
                destination,
                worktree_root,
                committed_paths: recovery
                    .affected_paths()
                    .iter()
                    .map(GitPath::display_lossy)
                    .collect(),
                observed_tip: Some(self.target_tip.clone()),
                index_repair: Some(detail),
            }),
            Err(GitError::SafeCommitAdvanceUnconfirmed {
                candidate, detail, ..
            }) => {
                let current = repository.read_branch_tip(&self.destination)?;
                if current.as_ref() == Some(&candidate) {
                    CoordCommitOutcome::Landed(LandedCommit {
                        commit: candidate,
                        destination: self.destination.as_str().to_owned(),
                        worktree_root: workspace.workspace().primary_root().to_path_buf(),
                        committed_paths: self.paths.iter().map(GitPath::display_lossy).collect(),
                        observed_tip: Some(self.target_tip.clone()),
                        index_repair: Some(detail),
                    })
                } else if current.as_ref() == Some(&self.target_tip) {
                    CoordCommitOutcome::NotLanded {
                        candidate,
                        observed_tip: current,
                        detail,
                    }
                } else {
                    CoordCommitOutcome::Indeterminate {
                        candidate,
                        observed_tip: current,
                        detail,
                    }
                }
            }
            Err(error) => return Err(WorkspaceError::Git(error)),
        };
        Ok(PlanningCommitReceipt {
            outcome,
            destination: self.destination,
            target_tip: self.target_tip,
            pin_class: self.pin_class,
            meta_guard: self.meta_guard,
            paths: self.paths,
        })
    }
}

/// Evidence of one planning commit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanningCommitReceipt {
    /// The commit outcome.
    pub outcome: CoordCommitOutcome,
    /// The target branch.
    pub destination: BranchName,
    /// The parent the commit was pinned to.
    pub target_tip: Oid,
    /// The pin classification observed at plan time.
    pub pin_class: PinClass,
    /// The demotion guard outcome.
    pub meta_guard: MetaDemotionGuard,
    /// The committed paths.
    pub paths: Vec<GitPath>,
}

impl PlanningCommitReceipt {
    /// The landed commit, when one landed.
    pub fn landed(&self) -> Option<&LandedCommit> {
        self.outcome.landed()
    }
}

fn ensure_primary_partition(path: &GitPath, mission_dir_name: &str) -> Result<(), WorkspaceError> {
    let lookup = path.display_lossy();
    // `meta.json` and unrecognised paths are fail-safe toward PRIMARY, exactly
    // like the upstream classifier.
    if let Some(kind) = kind_for_mission_file(&lookup, Some(mission_dir_name))
        && kind.is_coord_partition()
    {
        return Err(WorkspaceError::CoordRefused {
            code: "PLANNING_PARTITION_REFUSED",
            message: format!(
                "COORD-partition artifact {lookup} reached the planning commit; coordination kinds must commit to the coordination branch"
            ),
        });
    }
    Ok(())
}

fn is_meta_json(path: &GitPath, mission_dir_name: &str) -> bool {
    let display = path.display_lossy();
    display == format!("{SPECS_DIR_NAME}/{mission_dir_name}/meta.json")
}

fn check_meta_demotion(
    working: &[u8],
    baseline: Option<&Map<String, Value>>,
) -> Result<MetaDemotionGuard, WorkspaceError> {
    let Some(baseline) = baseline else {
        return Ok(MetaDemotionGuard::NoBaseline);
    };
    let working: Value =
        serde_json::from_slice(working).map_err(|error| WorkspaceError::CoordRefused {
            code: "META_DEMOTION_GUARD_CORRUPT",
            message: format!("the working meta.json is not valid JSON: {error}"),
        })?;
    let working = working
        .as_object()
        .ok_or_else(|| WorkspaceError::CoordRefused {
            code: "META_DEMOTION_GUARD_CORRUPT",
            message: "the working meta.json is not a JSON object".to_owned(),
        })?;
    let baseline_branch = baseline.get("coordination_branch");
    let working_branch = working.get("coordination_branch");
    let baseline_routed = baseline_branch.is_some_and(|value| !value.is_null());
    let working_routed = working_branch.is_some_and(|value| !value.is_null());
    if baseline_routed && !working_routed {
        return Err(WorkspaceError::CoordRefused {
            code: "META_DEMOTION_REFUSED",
            message: format!(
                "the uncommitted meta.json drops coordination_branch (was {:?}); refusing to carry a whole-team routing change into the planning commit. Restore the key, or commit the flattened meta.json deliberately",
                baseline_branch.unwrap_or(&Value::Null)
            ),
        });
    }
    Ok(MetaDemotionGuard::Preserved)
}

/// Persist a landed planning commit into `lanes.json`.
///
/// The manifest's `planning_commit_sha` is the durable pin every lane consumer
/// reads. This writes the manifest through [`LanesStore`] (locked + atomic), so
/// a concurrent finalize cannot interleave.
pub fn record_planning_commit(
    store: &LanesStore,
    commit: &Oid,
) -> Result<LanesManifest, WorkspaceError> {
    let Some(manifest) = store.read()? else {
        return Err(WorkspaceError::InvalidArtifact {
            artifact: "lanes.json",
            detail: format!("no manifest at {}", store.path().display()),
        });
    };
    let updated = manifest.with_planning_commit(commit);
    store.write(&updated)?;
    Ok(updated)
}

/// Does `lane_branch` contain `planning_commit`?
pub fn lane_contains_planning_commit(
    repository: &sk_git::GitRepository,
    lane_branch: &str,
    planning_commit: &Oid,
) -> Result<bool, WorkspaceError> {
    let lane_branch = BranchName::parse(lane_branch)?;
    let Some(tip) = repository.read_branch_tip(&lane_branch)? else {
        return Ok(false);
    };
    Ok(repository.is_ancestor(planning_commit, &tip)?)
}

// ---------------------------------------------------------------------------
// Integration (fast-forward) of planning commits and dependency lane tips
// ---------------------------------------------------------------------------

/// A real, applied ref advance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LandedAdvance {
    /// The branch that moved.
    pub branch: String,
    /// Its previous tip.
    pub previous_tip: Oid,
    /// Its new tip.
    pub new_tip: Oid,
    /// Checked-out worktrees that were resynchronised.
    pub resynced: Vec<PathBuf>,
}

/// What integrating one source tip into a destination did.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IntegrationOutcome {
    /// The destination moved forward to the source tip.
    FastForwarded(LandedAdvance),
    /// The source was already reachable from the destination.
    AlreadyIntegrated {
        /// The destination tip.
        destination_tip: Oid,
        /// The source tip.
        source_tip: Oid,
    },
    /// Both sides moved: a merge commit is required. Nothing was changed.
    MergeRequired {
        /// The destination tip.
        destination_tip: Oid,
        /// The source tip.
        source_tip: Oid,
        /// Why a fast-forward was impossible, and what is needed.
        detail: String,
    },
}

impl IntegrationOutcome {
    /// Did this integration mutate anything?
    pub fn advanced(&self) -> Option<&LandedAdvance> {
        match self {
            Self::FastForwarded(advance) => Some(advance),
            _ => None,
        }
    }
}

/// One dependency lane's integration result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DependencyIntegration {
    /// The dependency lane id.
    pub lane_id: String,
    /// Its branch.
    pub branch: String,
    /// The outcome.
    pub outcome: IntegrationOutcome,
}

/// The result of integrating a lane's dependency tips.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IntegrationReport {
    /// Integrations in execution order.
    pub integrations: Vec<DependencyIntegration>,
}

impl IntegrationReport {
    /// Every dependency whose source was still unreachable at the end.
    pub fn unintegrated(&self) -> Vec<&DependencyIntegration> {
        self.integrations
            .iter()
            .filter(|entry| matches!(entry.outcome, IntegrationOutcome::MergeRequired { .. }))
            .collect()
    }

    /// Did every dependency integrate cleanly?
    pub fn is_complete(&self) -> bool {
        self.unintegrated().is_empty()
    }
}

/// Fast-forward `destination` to `source_tip` when that is the only mutation
/// needed; otherwise report what a real merge requires.
pub fn integrate_tip(
    repository: &sk_git::GitRepository,
    destination: &BranchName,
    source_tip: &Oid,
    source_label: &str,
    protected: &ProtectedBranches,
    message: &str,
) -> Result<IntegrationOutcome, WorkspaceError> {
    let Some(destination_tip) = repository.read_branch_tip(destination)? else {
        return Err(WorkspaceError::CoordRefused {
            code: "INTEGRATION_DESTINATION_MISSING",
            message: format!(
                "cannot integrate {source_label} into {destination}: the destination branch does not exist"
            ),
        });
    };
    if source_tip == &destination_tip || repository.is_ancestor(source_tip, &destination_tip)? {
        return Ok(IntegrationOutcome::AlreadyIntegrated {
            destination_tip,
            source_tip: source_tip.clone(),
        });
    }
    if !repository.is_ancestor(&destination_tip, source_tip)? {
        return Ok(IntegrationOutcome::MergeRequired {
            destination_tip,
            source_tip: source_tip.clone(),
            detail: format!(
                "{destination} and {source_label} have diverged; a merge commit is required and the Git adapter exposes no three-way merge primitive yet"
            ),
        });
    }
    let options = BranchAdvanceOptions::default().reset_clean_checkouts();
    let plan = match repository.plan_branch_advance(
        destination,
        source_tip.clone(),
        Some(destination_tip.clone()),
        protected,
        message,
        &options,
    ) {
        Ok(plan) => plan,
        Err(GitError::ProtectedTarget { .. }) => {
            return Err(WorkspaceError::CoordRefused {
                code: crate::policy::PROTECTED_BRANCH_REFUSED,
                message: format!(
                    "refusing to integrate {source_label}: destination {destination} is protected"
                ),
            });
        }
        Err(GitError::BranchCheckedOut { worktrees, .. }) => {
            return Err(WorkspaceError::CoordRefused {
                code: "INTEGRATION_CHECKOUT_DIRTY",
                message: format!(
                    "refusing to integrate {source_label}: {destination} is checked out in a checkout with local changes ({}); commit or stash first",
                    worktrees
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            });
        }
        Err(error) => return Err(WorkspaceError::Git(error)),
    };
    let outcome = repository.apply_branch_advance(&plan)?;
    Ok(IntegrationOutcome::FastForwarded(LandedAdvance {
        branch: outcome.branch().as_str().to_owned(),
        previous_tip: outcome
            .previous()
            .cloned()
            .unwrap_or_else(|| destination_tip.clone()),
        new_tip: outcome.new_tip().clone(),
        resynced: outcome.resynced().to_vec(),
    }))
}

/// Integrate the recorded planning commit into a lane worktree's branch.
///
/// A lane cut from the coordination branch predates planning, so this fast-
/// forwards the lane to the recorded planning commit when the lane has no unique
/// work yet. A divergent lane is reported as [`IntegrationOutcome::MergeRequired`].
pub fn integrate_planning_commit(
    repository: &sk_git::GitRepository,
    lane_branch: &BranchName,
    planning_commit: &Oid,
    protected: &ProtectedBranches,
) -> Result<IntegrationOutcome, WorkspaceError> {
    integrate_tip(
        repository,
        lane_branch,
        planning_commit,
        "the recorded planning commit",
        protected,
        "chore(spec-kitty): merge recorded planning commit",
    )
}

/// Integrate every approved dependency lane tip into `lane_id`.
///
/// Dependencies run in `(parallel_group, lane_id)` order, matching lane
/// planning. The first dependency that needs a merge commit stops the run: the
/// already-completed fast-forwards are real and reported, and nothing is
/// attempted that would need a merge primitive this adapter does not have.
pub fn integrate_dependency_lanes(
    workspace: &MissionWorkspace,
    manifest: &LanesManifest,
    lane_id: &str,
    protected: &ProtectedBranches,
) -> Result<IntegrationReport, WorkspaceError> {
    let lane = manifest
        .lanes
        .iter()
        .find(|lane| lane.lane_id == lane_id)
        .ok_or_else(|| WorkspaceError::LaneNotFound {
            wp: lane_id.to_owned(),
        })?;
    let mut report = IntegrationReport::default();
    if lane.depends_on_lanes.is_empty() {
        return Ok(report);
    }
    if lane.is_planning() && lane_id == PLANNING_LANE_ID {
        // The planning lane writes the shared primary checkout, whose branch is
        // the mission target branch.
        let primary_root = workspace.workspace().primary_root().to_path_buf();
        let repository = sk_git::GitRepository::open(&primary_root)?;
        let destination = BranchName::parse(&workspace.target_branch()?)?;
        let mut dependencies: Vec<&str> =
            lane.depends_on_lanes.iter().map(String::as_str).collect();
        dependencies.sort_unstable();
        for dependency in dependencies {
            let branch = crate::naming::code_lane_branch_name(&manifest.mission_slug, dependency)?;
            let branch_name = BranchName::parse(&branch)?;
            let Some(source_tip) = repository.read_branch_tip(&branch_name)? else {
                return Err(WorkspaceError::CoordRefused {
                    code: "DEPENDENCY_BRANCH_MISSING",
                    message: format!(
                        "dependency lane {dependency} has no branch {branch}; refusing to report an integration that cannot exist"
                    ),
                });
            };
            let outcome = integrate_tip(
                &repository,
                &destination,
                &source_tip,
                &branch,
                protected,
                &format!("chore(spec-kitty): merge dependency lane {dependency}"),
            )?;
            let needs_merge = matches!(outcome, IntegrationOutcome::MergeRequired { .. });
            report.integrations.push(DependencyIntegration {
                lane_id: dependency.to_owned(),
                branch,
                outcome,
            });
            if needs_merge {
                break;
            }
        }
        return Ok(report);
    }

    let primary_root = workspace.workspace().primary_root().to_path_buf();
    let worktree = crate::naming::worktree_path(&primary_root, &manifest.mission_slug, lane_id)?;
    if !worktree.exists() {
        return Err(WorkspaceError::CoordRefused {
            code: "LANE_WORKTREE_MISSING",
            message: format!(
                "cannot integrate dependencies into {lane_id}: {} is not materialized",
                worktree.display()
            ),
        });
    }
    let repository = sk_git::GitRepository::open(&worktree)?;
    let destination = BranchName::parse(&crate::naming::code_lane_branch_name(
        &manifest.mission_slug,
        lane_id,
    )?)?;
    let mut dependencies: Vec<&crate::lanes::ExecutionLane> = lane
        .depends_on_lanes
        .iter()
        .filter_map(|id| manifest.lanes.iter().find(|lane| &lane.lane_id == id))
        .collect();
    dependencies.sort_by(|left, right| {
        (left.parallel_group, left.lane_id.as_str())
            .cmp(&(right.parallel_group, right.lane_id.as_str()))
    });
    for dependency in dependencies {
        let branch =
            crate::naming::code_lane_branch_name(&manifest.mission_slug, &dependency.lane_id)?;
        let branch_name = BranchName::parse(&branch)?;
        let Some(source_tip) = repository.read_branch_tip(&branch_name)? else {
            return Err(WorkspaceError::CoordRefused {
                code: "DEPENDENCY_BRANCH_MISSING",
                message: format!(
                    "dependency lane {} has no branch {branch}; refusing to report an integration that cannot exist",
                    dependency.lane_id
                ),
            });
        };
        let outcome = integrate_tip(
            &repository,
            &destination,
            &source_tip,
            &branch,
            protected,
            &format!(
                "chore(spec-kitty): merge dependency lane {}",
                dependency.lane_id
            ),
        )?;
        let needs_merge = matches!(outcome, IntegrationOutcome::MergeRequired { .. });
        report.integrations.push(DependencyIntegration {
            lane_id: dependency.lane_id.clone(),
            branch,
            outcome,
        });
        if needs_merge {
            break;
        }
    }
    Ok(report)
}

/// A planning manifest's recorded pin, resolved for consumers.
pub fn recorded_pin(manifest: &LanesManifest) -> Result<Option<Oid>, WorkspaceError> {
    match manifest.planning_commit_sha.as_deref() {
        Some(sha) => Ok(Some(Oid::parse(sha)?)),
        None => Ok(None),
    }
}

/// Paths a planning commit may carry, in repository-relative POSIX form.
pub fn planning_paths(directory: &Path, entries: &[&str]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| {
            let joined = directory.join(entry);
            joined
                .components()
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/")
        })
        .collect()
}

/// The kinds a planning commit is allowed to carry (PRIMARY partition).
pub fn is_planning_kind(kind: MissionArtifactKind) -> bool {
    kind.is_primary_partition()
}
