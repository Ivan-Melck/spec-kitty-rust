//! Typed failures for workspace, placement, naming, and allocation operations.

use std::path::PathBuf;

use sk_git::GitError;
use sk_kernel::KernelError;
use sk_mission::MissionError;

/// Every failure this crate can report.
///
/// The variants deliberately separate *absent* data from *corrupt* data, a
/// refusal from an infrastructure failure, and an unwritable/invalid plan from
/// a Git process failure. A caller must never read "could not check" as "the
/// thing is absent".
#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    /// A durable read, lock, or atomic write failed.
    #[error(transparent)]
    Kernel(#[from] KernelError),

    /// A Git probe or planned mutation failed.
    #[error(transparent)]
    Git(#[from] GitError),

    /// Mission inventory/selection failed.
    #[error(transparent)]
    Mission(#[from] MissionError),

    /// A mission directory has no `meta.json`; placement and ownership facts
    /// fail closed rather than inventing identity from a directory name.
    #[error("mission {mission} has no readable meta.json at {path}")]
    MissingManifest {
        /// Mission selector or slug that was requested.
        mission: String,
        /// Expected manifest path.
        path: PathBuf,
    },

    /// A stored `meta.json` value contradicts the caller's assertion.
    #[error("mission {mission} stores {field}={stored:?}, not the asserted {expected:?}")]
    ManifestMismatch {
        /// Mission slug.
        mission: String,
        /// Field name.
        field: &'static str,
        /// Stored value (or `None` when absent).
        stored: Option<String>,
        /// Contract-required value.
        expected: String,
    },

    /// The stored `topology` is absent or unrecognised for an operation that
    /// requires a durable shape.
    #[error("mission {mission} has no recognised stored topology ({stored:?}); refusing to guess")]
    TopologyUnresolved {
        /// Mission slug.
        mission: String,
        /// Raw stored value, if any.
        stored: Option<String>,
    },

    /// A `SINGLE_BRANCH` mission's lane manifest contains code lanes
    /// (Invariant T-1). The writer refuses before any Git mutation.
    #[error(
        "mission {mission} is stamped topology=single_branch but its lane manifest has a code lane; run 'spec-kitty migrate backfill-topology --restamp-single-branch'"
    )]
    SingleBranchCodeLanes {
        /// Mission slug.
        mission: String,
    },

    /// The canonical branch/worktree name cannot be composed without
    /// inventing identity.
    #[error("cannot compose a canonical mission branch for {mission}: {detail}")]
    BranchIdentityUnresolved {
        /// Mission selector.
        mission: String,
        /// Why the identity is missing.
        detail: String,
    },

    /// A mission id is present but too short to derive a `mid8`.
    #[error("mission_id {mission_id:?} is invalid: a full ULID is at least 8 characters")]
    InvalidMissionIdentity {
        /// The corrupt value.
        mission_id: String,
    },

    /// An owned checkout could not be minted or is internally inconsistent.
    #[error("owned checkout refused ({code}): {message}")]
    OwnedRefused {
        /// Stable refusal code (`OWNED_*`).
        code: &'static str,
        /// Human explanation naming the violated invariant.
        message: String,
    },

    /// A path escapes the owned checkout's mission directory.
    #[error("path is outside the selected mission: {path}")]
    OwnedPathRefused {
        /// The offending path as supplied.
        path: PathBuf,
    },

    /// A surface has no resolved filesystem location for this call.
    #[error("no resolved location for surface {surface:?}; the caller must supply it")]
    SurfaceUnresolved {
        /// The requested surface.
        surface: crate::surface::TopologySurface,
    },

    /// The declared coordination branch was deleted from Git while the mission
    /// still declares coordination topology.
    #[error(
        "coordination branch {branch:?} for mission {mission} has been deleted from git; refusing to substitute the primary checkout"
    )]
    CoordinationBranchDeleted {
        /// Mission slug.
        mission: String,
        /// Declared branch.
        branch: String,
    },

    /// The coordination branch exists but its worktree has never been
    /// materialised (fresh clone / CI / removed worktree).
    #[error(
        "coordination worktree for mission {mission} ({branch:?}) is not materialised; materialise it instead of reading the primary checkout"
    )]
    CoordinationWorktreeUnmaterialized {
        /// Mission slug.
        mission: String,
        /// Declared branch.
        branch: String,
    },

    /// A lifecycle probe failed for a reason other than "absent".
    #[error("lifecycle probe failed (not absence): {detail}")]
    LifecycleProbeFailed {
        /// What was probed.
        probe: &'static str,
        /// Underlying detail.
        detail: String,
    },

    /// Lane planning cannot produce a valid assignment.
    #[error("lane planning error: {message}")]
    LanePlanning {
        /// Explanation.
        message: String,
    },

    /// The completed lane graph has a cycle.
    #[error("execution-lane dependency cycle detected: {}", path.join(" -> "))]
    LaneDependencyCycle {
        /// Deterministic cycle path, closed at both ends.
        path: Vec<String>,
    },

    /// The WP is not assigned to any lane in the manifest.
    #[error("work package {wp} is not assigned to any execution lane")]
    LaneNotFound {
        /// The WP id.
        wp: String,
    },

    /// A repo-root lane was asked for a worktree path.
    #[error("lane {lane} resolves to the repository-root checkout and has no worktree")]
    RepoRootLaneHasNoWorktree {
        /// The lane id.
        lane: String,
    },

    /// A lane worktree that would be reused carries uncommitted changes.
    #[error(
        "lane {lane} worktree at {path} has uncommitted changes; commit or stash before continuing"
    )]
    DirtyWorktree {
        /// Lane id.
        lane: String,
        /// Worktree path.
        path: PathBuf,
    },

    /// An explicit `--base` cannot be honoured on the active route.
    #[error("cannot honour base {base:?} on the {route} route for {wp}: {detail}")]
    UnhonorableBase {
        /// Supplied base ref.
        base: String,
        /// Route name.
        route: &'static str,
        /// Work package.
        wp: String,
        /// Why not.
        detail: String,
    },

    /// The allocation plan is stale: the observed Git state moved.
    #[error("allocation plan for {what} is stale: {detail}")]
    PlanStale {
        /// What was planned.
        what: String,
        /// Observed divergence.
        detail: String,
    },

    /// A recorded allocation intent cannot be reconciled automatically.
    #[error("interrupted allocation for {mission}/{lane} at {path} is inconsistent: {detail}")]
    JournalInconsistent {
        /// Mission slug.
        mission: String,
        /// Lane id.
        lane: String,
        /// Worktree path.
        path: PathBuf,
        /// Explanation of the inconsistent state.
        detail: String,
    },

    /// Retiring a lane would discard user state.
    #[error("refusing to retire lane {lane} at {path}: {reason}")]
    RetentionRefused {
        /// Lane id.
        lane: String,
        /// Worktree path.
        path: PathBuf,
        /// Why the state is retained.
        reason: String,
    },

    /// A lane manifest cannot be decoded.
    #[error("corrupt lanes.json at {path}: {detail}")]
    CorruptLanes {
        /// Manifest path.
        path: PathBuf,
        /// Decode failure.
        detail: String,
    },

    /// A manifest or YAML/JSON document does not satisfy the contract.
    #[error("invalid {artifact}: {detail}")]
    InvalidArtifact {
        /// Artifact name.
        artifact: &'static str,
        /// Detail.
        detail: String,
    },

    /// `.kittify/config.yaml` cannot be read as a protection policy. A
    /// malformed value is never silently replaced by a default.
    #[error("invalid protection configuration {path}: {detail}")]
    ProtectionConfig {
        /// Configuration path.
        path: PathBuf,
        /// What is malformed.
        detail: String,
    },

    /// A coordination operation refused before mutating anything.
    ///
    /// `code` is stable and machine-readable (`COORD_*`); the message names the
    /// violated invariant and the recovery step.
    #[error("coordination refused ({code}): {message}")]
    CoordRefused {
        /// Stable refusal code.
        code: &'static str,
        /// Human explanation.
        message: String,
    },

    /// A bootstrap attempt failed after it had minted the coordination branch,
    /// and the mint was rolled back (or deliberately left in place).
    ///
    /// The underlying failure stays inspectable through
    /// [`std::error::Error::source`], so a caller can still match on its code.
    #[error("{error}; coordination branch mint rollback: {rollback}")]
    CoordBootstrapRolledBack {
        /// The failure that ended the bootstrap.
        error: Box<WorkspaceError>,
        /// What happened to the branch this attempt minted.
        rollback: String,
    },

    /// The coordination journal cannot be decoded or reconciled.
    #[error("coordination journal {path} is unusable: {detail}")]
    CoordJournal {
        /// Journal document path.
        path: PathBuf,
        /// What is wrong.
        detail: String,
    },
}
