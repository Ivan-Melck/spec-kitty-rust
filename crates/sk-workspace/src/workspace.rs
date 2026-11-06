//! Workspace roots, mission write branches, and coordination-surface probing.
//!
//! Every resolver here is CWD-invariant in the sense that matters: the answer
//! depends on the **repository** a path belongs to (its Git common directory
//! and its canonical primary checkout), not on which worktree the command
//! happens to run in. A lane worktree and the primary checkout therefore
//! resolve the same mission directory and the same write branch.
//!
//! # Missing manifests fail closed
//!
//! A mission is identified by `kitty-specs/<slug>/meta.json`. The loaders here
//! never infer a mission from a directory-name shape and never invent a default
//! topology. An absent manifest is [`WorkspaceError::MissingManifest`]; an
//! unrecognised stored topology is [`WorkspaceError::TopologyUnresolved`].

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use sk_git::{BranchName, GitRepository, ProtectedBranches};
use sk_mission::{
    Mission, MissionId, MissionTopology, list_missions, routes_through_coordination, select_mission,
};

use crate::error::WorkspaceError;
use crate::naming::{MissionIdentity, WORKTREES_DIRNAME, mission_dir_name};
use crate::surface::CoordState;

/// The `kitty-specs/` directory name.
pub const SPECS_DIR_NAME: &str = "kitty-specs";

/// The spec-kitty runtime state directory.
pub const RUNTIME_DIR_NAME: &str = ".spec-kitty";

/// A repository resolved from any path inside any of its worktrees.
#[derive(Clone, Debug)]
pub struct ProjectWorkspace {
    repository: GitRepository,
    primary_root: PathBuf,
    common_dir: PathBuf,
}

impl ProjectWorkspace {
    /// Open the repository containing `start` (a worktree path or a mission
    /// directory).
    pub fn open(start: &Path) -> Result<Self, WorkspaceError> {
        let repository = GitRepository::open(start)?;
        let dirs = repository.git_dirs()?;
        let common_dir = dirs.common_dir().to_path_buf();
        let primary_root = primary_root_of(&repository);
        Ok(Self {
            repository,
            primary_root,
            common_dir,
        })
    }

    /// The Git adapter for this repository.
    pub fn repository(&self) -> &GitRepository {
        &self.repository
    }

    /// The canonical primary checkout root, identical from every worktree.
    pub fn primary_root(&self) -> &Path {
        &self.primary_root
    }

    /// The shared Git common directory.
    pub fn common_dir(&self) -> &Path {
        &self.common_dir
    }

    /// The `kitty-specs/` directory in the primary checkout.
    pub fn specs_dir(&self) -> PathBuf {
        self.primary_root.join(SPECS_DIR_NAME)
    }

    /// The canonical lock root for a mission.
    ///
    /// Mis-routing a write lock is a concurrency defect: two processes anchored
    /// on the same mission through different worktrees must contend on one
    /// lock. This is therefore the primary checkout root, never the current
    /// worktree.
    pub fn status_lock_root(&self) -> &Path {
        &self.primary_root
    }

    /// List the primary checkout's missions, reading their manifests.
    pub fn missions(&self) -> Result<Vec<Mission>, WorkspaceError> {
        Ok(list_missions(self.primary_root())?)
    }

    /// Select and load one mission by slug, mid8, or full ULID.
    pub fn mission(&self, selector: &str) -> Result<MissionWorkspace, WorkspaceError> {
        let missions = self.missions()?;
        let mission = select_mission(&missions, selector)?;
        MissionWorkspace::from_mission(self.clone(), mission.clone())
    }
}

fn primary_root_of(repository: &GitRepository) -> PathBuf {
    if let Ok(worktrees) = repository.worktrees()
        && let Some(main) = worktrees.iter().find(|worktree| !worktree.is_bare())
    {
        return worktree_canonical_path(main.path());
    }
    // The common dir is normally `<primary>/.git`; a bare repository's common
    // dir is the repository itself. This fallback is only reached when the
    // listing cannot be read, and is documented rather than silent.
    repository
        .git_dirs()
        .ok()
        .and_then(|dirs| dirs.common_dir().parent().map(Path::to_path_buf))
        .unwrap_or_else(|| repository.root().to_path_buf())
}

fn worktree_canonical_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// A loaded mission with its durable placement facts.
#[derive(Clone, Debug)]
pub struct MissionWorkspace {
    workspace: ProjectWorkspace,
    mission: Mission,
    identity: MissionIdentity,
    topology: MissionTopology,
    mission_dir: PathBuf,
}

impl MissionWorkspace {
    fn from_mission(workspace: ProjectWorkspace, mission: Mission) -> Result<Self, WorkspaceError> {
        let Some(topology) = mission.topology() else {
            return Err(WorkspaceError::TopologyUnresolved {
                mission: mission.slug().as_str().to_owned(),
                stored: mission
                    .meta_field("topology")
                    .map(|value| value.to_string()),
            });
        };
        let identity = MissionIdentity::from_mission(&mission);
        let mission_dir = workspace
            .primary_root()
            .join(SPECS_DIR_NAME)
            .join(mission.slug().as_str());
        if !mission_dir.join("meta.json").is_file() {
            return Err(WorkspaceError::MissingManifest {
                mission: mission.slug().as_str().to_owned(),
                path: mission_dir.join("meta.json"),
            });
        }
        Ok(Self {
            workspace,
            mission,
            identity,
            topology,
            mission_dir,
        })
    }

    /// The owning project workspace.
    pub fn workspace(&self) -> &ProjectWorkspace {
        &self.workspace
    }

    /// The loaded mission.
    pub fn mission(&self) -> &Mission {
        &self.mission
    }

    /// Canonical mission identity.
    pub fn identity(&self) -> &MissionIdentity {
        &self.identity
    }

    /// The mission slug.
    pub fn slug(&self) -> &str {
        self.mission.slug().as_str()
    }

    /// The mission id.
    pub fn mission_id(&self) -> &MissionId {
        self.mission.id()
    }

    /// The stored topology.
    pub fn topology(&self) -> MissionTopology {
        self.topology
    }

    /// The canonical mission directory in the primary checkout.
    pub fn mission_dir(&self) -> &Path {
        &self.mission_dir
    }

    /// The mission's target branch.
    pub fn target_branch(&self) -> Result<String, WorkspaceError> {
        match self.mission.meta_field("target_branch") {
            Some(serde_json::Value::String(value)) if !value.trim().is_empty() => {
                Ok(value.trim().to_owned())
            }
            Some(other) => Err(WorkspaceError::InvalidArtifact {
                artifact: "target_branch",
                detail: format!("expected a non-empty string, observed {other}"),
            }),
            None => Err(WorkspaceError::InvalidArtifact {
                artifact: "target_branch",
                detail: "meta.json records no target_branch".to_owned(),
            }),
        }
    }

    /// The declared coordination branch, if the mission records one.
    pub fn coordination_branch(&self) -> Option<String> {
        match self.mission.meta_field("coordination_branch") {
            Some(serde_json::Value::String(value)) if !value.trim().is_empty() => {
                Some(value.trim().to_owned())
            }
            _ => None,
        }
    }

    /// The declared `meta.json` `mission_branch` (protected-target missions).
    pub fn declared_mission_branch(&self) -> Option<String> {
        match self.mission.meta_field("mission_branch") {
            Some(serde_json::Value::String(value)) if !value.trim().is_empty() => {
                Some(value.trim().to_owned())
            }
            _ => None,
        }
    }

    /// The branch an owned write to this mission lands on.
    ///
    /// The upstream rule (`mission_runtime.single_branch_write_ref`) is:
    /// a `single_branch` mission's minted `meta.json` `mission_branch` when one
    /// is recorded, otherwise its `target_branch`. Every other topology's
    /// durable write branch is its target branch; lane work never writes the
    /// primary checkout, so the mission integration branch is not used here.
    /// The protection policy is deliberately not consulted: `mission_branch`
    /// is the create-time record of that decision, and a later policy change
    /// must not move an existing mission's write branch.
    ///
    /// The one addition is a fail-closed guard: a `single_branch` mission whose
    /// target is on the supplied protected list but which records no
    /// `mission_branch` refuses, rather than returning a branch this process
    /// must not write behind the operator's back.
    pub fn write_branch(
        &self,
        protected: &ProtectedBranches,
    ) -> Result<BranchName, WorkspaceError> {
        let target = self.target_branch()?;
        if self.topology == MissionTopology::SingleBranch {
            if let Some(declared) = self.declared_mission_branch() {
                return Ok(BranchName::parse(&declared)?);
            }
            if protected_target(&target, protected)? {
                return Err(WorkspaceError::OwnedRefused {
                    code: "OWNED_BRANCH_REFUSED",
                    message: format!(
                        "target branch {target} is protected but the mission records no mission_branch; refusing to write the protected branch"
                    ),
                });
            }
        }
        Ok(BranchName::parse(&target)?)
    }

    /// The canonical mission directory name (`<slug>-<mid8>`).
    ///
    /// This is the **NNN-stripping** composer, used for names that must be
    /// idempotent with the `mission_id` path (the mission branch body, the
    /// composed primary-directory name). It is *not* the coordination home:
    /// see [`Self::coord_mission_dir_name`].
    pub fn canonical_mission_dir_name(&self) -> String {
        mission_dir_name(self.slug(), self.identity.mid8())
    }

    /// The canonical coordination Mission directory name (`<slug>-<mid8>`,
    /// VERBATIM).
    ///
    /// The coordination read/seed/transaction path reconstructs the directory
    /// the reference actually created, so it must not strip a legacy `NNN-`
    /// prefix: upstream composes it with
    /// `lanes.branch_naming.coord_mission_dir_name`, whose docstring names this
    /// exact hazard ("the canonical, NNN-stripping `mission_dir_name` … would
    /// drift a legacy `NNN-` slug to a name never created"). A coordination-
    /// routed mission stored as `057-name` therefore lives at
    /// `kitty-specs/057-name-<mid8>` inside its coordination worktree, while its
    /// mission branch body drops the prefix.
    pub fn coord_mission_dir_name(&self) -> String {
        crate::naming::coord_mission_dir_name(self.slug(), self.identity.mid8())
    }

    /// The expected coordination worktree path, when the topology routes
    /// through coordination.
    pub fn coord_worktree_path(&self) -> Option<PathBuf> {
        if !routes_through_coordination(self.topology) {
            return None;
        }
        Some(
            self.workspace
                .primary_root()
                .join(WORKTREES_DIRNAME)
                .join(self.identity.coord_dir_name()),
        )
    }

    /// Probe the coordination surface's materialization state.
    ///
    /// The four-way answer mirrors upstream's shared `probe_coord_state`:
    ///
    /// * a coord-less topology is [`CoordState::None`];
    /// * the canonical coordination worktree is registered on the declared
    ///   branch and its Mission directory exists → [`CoordState::Materialized`];
    /// * that worktree is registered but its Mission directory is absent →
    ///   [`CoordState::Empty`] (a seed ticket, never a silent primary read);
    /// * no worktree is registered but the branch still exists locally or on a
    ///   remote → [`CoordState::Unmaterialized`];
    /// * the declared branch is gone everywhere → [`CoordState::Deleted`].
    ///
    /// This probe is strictly read-only: it creates no worktree and writes no
    /// file, so a read of an `Empty` or `Unmaterialized` surface cannot mutate
    /// the repository. A registered worktree whose checkout is on a different
    /// branch is a typed refusal (never auto-corrected): switching a checkout
    /// back would discard whatever the operator has in flight there.
    ///
    /// Only the `Deleted` question consults remotes, and only after the local
    /// head and the `refs/remotes/` view both miss — the same last arm upstream
    /// uses. `sk-git` bounds each remote contact with its own timeout, so this
    /// cannot hang indefinitely; a remote that answers nothing or errors keeps
    /// the branch `Unmaterialized` rather than reporting data loss.
    ///
    /// # A lost `coordination_branch` record
    ///
    /// `meta.json`'s `coordination_branch` is only a *record*. When it is absent
    /// on a coordination-routed topology, this probe derives the same canonical
    /// name the mint uses ([`crate::naming::coord_reconstruct_branch`]) and
    /// probes **that** branch, setting [`CoordinationProbe::derived`]. The
    /// reference does exactly this (`_derive_branch_for_undeclared`): a lost
    /// record over a live canonical branch must proceed as if declared instead
    /// of being labelled data loss, and only a derived branch that is genuinely
    /// absent is `COORD_BRANCH_UNDECLARED_AND_ABSENT` on the write path.
    pub fn probe_coordination(&self) -> Result<CoordinationProbe, WorkspaceError> {
        if !routes_through_coordination(self.topology) {
            return Ok(CoordinationProbe {
                state: CoordState::None,
                branch: None,
                derived: false,
                worktree: None,
                mission_dir: None,
                local_head: false,
                stray_directory: None,
                tip: None,
            });
        }
        let (branch_text, derived) = match self.coordination_branch() {
            Some(branch_text) => (branch_text, false),
            None => (
                crate::naming::coord_reconstruct_branch(self.slug(), self.identity.mid8()),
                true,
            ),
        };
        let branch = BranchName::parse(&branch_text)?;
        let repository = self.workspace.repository();
        let local_tip = repository.read_branch_tip(&branch)?;
        let coord_root = self.coord_worktree_path();
        let mission_dir = coord_root.as_ref().map(|root| {
            root.join(SPECS_DIR_NAME)
                .join(self.coord_mission_dir_name())
        });
        let worktrees = repository.worktrees()?;

        if let Some(root) = &coord_root
            && root.exists()
        {
            let registered = worktrees
                .iter()
                .find(|worktree| same_path(worktree.path(), root));
            return match registered {
                Some(worktree) => {
                    if worktree.branch() != Some(branch_text.as_str()) {
                        return Err(WorkspaceError::CoordRefused {
                            code: "COORDINATION_WORKTREE_BRANCH_MISMATCH",
                            message: format!(
                                "coordination worktree {} is on {:?}, expected {branch_text:?}; refusing to switch it automatically",
                                root.display(),
                                worktree.branch().unwrap_or("<detached>")
                            ),
                        });
                    }
                    let state = if mission_dir.as_ref().is_some_and(|dir| dir.is_dir()) {
                        CoordState::Materialized
                    } else {
                        CoordState::Empty
                    };
                    Ok(CoordinationProbe {
                        state,
                        branch: Some(branch_text),
                        derived,
                        worktree: Some(root.clone()),
                        mission_dir,
                        local_head: local_tip.is_some(),
                        stray_directory: None,
                        tip: local_tip,
                    })
                }
                None => Ok(CoordinationProbe {
                    // Name proposes, the registry disposes: a directory Git does
                    // not register is a husk, never a materialized surface.
                    state: CoordState::Unmaterialized,
                    branch: Some(branch_text),
                    derived,
                    worktree: None,
                    mission_dir,
                    local_head: local_tip.is_some(),
                    stray_directory: Some(root.clone()),
                    tip: local_tip,
                }),
            };
        }

        if local_tip.is_some() {
            return Ok(CoordinationProbe {
                state: CoordState::Unmaterialized,
                branch: Some(branch_text),
                derived,
                worktree: None,
                mission_dir,
                local_head: true,
                stray_directory: None,
                tip: local_tip,
            });
        }
        // No local head. A branch that lives only on a remote is present, not
        // deleted: a fresh clone, a single-branch checkout, or a pruned
        // tracking ref must never be reported as data loss.
        let remote = repository.remote_branch_lookup(&branch)?;
        let present_on_remote = matches!(
            remote,
            sk_git::RemoteBranchLookup::Hit { .. } | sk_git::RemoteBranchLookup::Error { .. }
        );
        Ok(CoordinationProbe {
            state: if present_on_remote {
                CoordState::Unmaterialized
            } else {
                CoordState::Deleted
            },
            branch: Some(branch_text),
            derived,
            worktree: None,
            mission_dir,
            local_head: false,
            stray_directory: None,
            tip: None,
        })
    }
}

fn protected_target(target: &str, protected: &ProtectedBranches) -> Result<bool, WorkspaceError> {
    let branch = BranchName::parse(target)?;
    Ok(protected.contains(&branch))
}

/// The probed coordination surface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoordinationProbe {
    /// The materialization state.
    pub state: CoordState,
    /// The declared or derived coordination branch, when known.
    pub branch: Option<String>,
    /// `true` when `meta.json` lost `coordination_branch` and this is the name
    /// the canonical grammar derived. A derived branch that is absent is a
    /// typed undeclared-and-absent refusal, never a data-loss `Deleted` label.
    pub derived: bool,
    /// The materialised worktree, when present and registered.
    pub worktree: Option<PathBuf>,
    /// The canonical coordination Mission directory
    /// (`<worktree>/kitty-specs/<slug>-<mid8>`, VERBATIM), whether or not it
    /// exists.
    pub mission_dir: Option<PathBuf>,
    /// Does the declared branch resolve as a local head?
    pub local_head: bool,
    /// A directory at the canonical worktree path that Git does not register.
    /// It is reported, never adopted and never deleted.
    pub stray_directory: Option<PathBuf>,
    /// The local branch tip, when the branch exists locally.
    pub tip: Option<sk_git::Oid>,
}

fn same_path(left: &Path, right: &Path) -> bool {
    std::fs::canonicalize(left).unwrap_or_else(|_| left.to_path_buf())
        == std::fs::canonicalize(right).unwrap_or_else(|_| right.to_path_buf())
}

/// Resolve a mission's canonical directory from any checkout path.
///
/// This is a thin convenience over [`ProjectWorkspace::mission`] that returns
/// only the directory, for callers that already know the slug is valid.
pub fn mission_dir_from(start: &Path, selector: &str) -> Result<PathBuf, WorkspaceError> {
    let workspace = ProjectWorkspace::open(start)?;
    Ok(workspace.mission(selector)?.mission_dir().to_path_buf())
}

/// The repo-root-lane WP ids of a manifest (used by occupancy checks).
pub fn repo_root_lane_wps(manifest: &crate::lanes::LanesManifest) -> BTreeSet<String> {
    manifest
        .lanes
        .iter()
        .filter(|lane| lane.is_planning())
        .flat_map(|lane| lane.wp_ids.iter().map(ToString::to_string))
        .collect()
}
