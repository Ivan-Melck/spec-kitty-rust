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
    pub fn canonical_mission_dir_name(&self) -> String {
        mission_dir_name(self.slug(), self.identity.mid8())
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
    /// * a coord-less topology is [`CoordState::None`];
    /// * a declared branch that no longer resolves is [`CoordState::Deleted`];
    /// * a registered worktree on that branch is [`CoordState::Materialized`];
    /// * a branch that exists without a registered worktree is
    ///   [`CoordState::Unmaterialized`] (materialize it; never silently read the
    ///   primary checkout).
    ///
    /// The upstream `EMPTY` versus `UNMATERIALIZED` distinction depends on
    /// coordination-seeding state that `sk-git` does not expose yet; this probe
    /// reports the fail-closed `Unmaterialized` answer for both rather than
    /// guessing.
    pub fn probe_coordination(&self) -> Result<CoordinationProbe, WorkspaceError> {
        if !routes_through_coordination(self.topology) {
            return Ok(CoordinationProbe {
                state: CoordState::None,
                branch: None,
                worktree: None,
            });
        }
        let Some(branch_text) = self.coordination_branch() else {
            // A coord-routing topology with no declared branch is not a
            // usable coordination surface. Fail closed with the delete-shaped
            // refusal rather than reading the primary checkout.
            return Ok(CoordinationProbe {
                state: CoordState::Deleted,
                branch: None,
                worktree: None,
            });
        };
        let branch = BranchName::parse(&branch_text)?;
        let tip = self.workspace.repository().read_branch_tip(&branch)?;
        if tip.is_none() {
            return Ok(CoordinationProbe {
                state: CoordState::Deleted,
                branch: Some(branch_text),
                worktree: None,
            });
        }
        let registered: Vec<PathBuf> = self
            .workspace
            .repository()
            .worktrees()?
            .into_iter()
            .filter(|worktree| worktree.branch() == Some(branch_text.as_str()))
            .map(|worktree| worktree.path().to_path_buf())
            .collect();
        match registered.into_iter().next() {
            Some(worktree) => Ok(CoordinationProbe {
                state: CoordState::Materialized,
                branch: Some(branch_text),
                worktree: Some(worktree),
            }),
            None => {
                // Name proposes, the worktree registry disposes: an existing
                // directory that git does not register is a husk, never a
                // materialized coordination surface.
                Ok(CoordinationProbe {
                    state: CoordState::Unmaterialized,
                    branch: Some(branch_text),
                    worktree: None,
                })
            }
        }
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
    /// The declared branch, when known.
    pub branch: Option<String>,
    /// The materialised worktree, when present.
    pub worktree: Option<PathBuf>,
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
