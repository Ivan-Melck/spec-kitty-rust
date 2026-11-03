//! Workspace, lane allocation, artifact placement, and ownership for the Rust
//! Spec Kitty implementation.
//!
//! # Scope
//!
//! This crate owns the bounded context that answers four questions:
//!
//! 1. **Where does an artifact live?** — [`artifact`] classifies a mission file
//!    to one of 23 artifact kinds and one of two partitions; [`surface`]
//!    translates the declared surface to a real path and resolves the
//!    materialization-aware read surface from the coordination state;
//!    [`coordination`] makes the stricter write-side decision, and [`policy`]
//!    is the pure protected-destination refusal.
//! 2. **Where in its lifecycle is the mission?** — [`lifecycle`] derives
//!    `pre_consolidation` / `consolidated` / `published` from durable evidence,
//!    keeping a failed probe distinct from an absent one.
//! 3. **Which checkout is owned, and what may it write?** — [`owned`] is a
//!    mint-only ownership fact backed by a real manifest, and [`workspace`]
//!    resolves roots and the mission write branch CWD-invariantly.
//! 4. **Which lane runs where, and how is it created without losing work?** —
//!    [`naming`] is the one name grammar, [`lanes`] plans and persists the
//!    lane manifest, [`journal`] records allocation intent, and [`allocation`]
//!    performs real, preconditioned, journaled lane/worktree allocation,
//!    reuse, crash recovery, and retention.
//! 5. **Where does a bookkeeping write actually land, and what proves it?** —
//!    [`coord`] is the coordination transaction store: read-only surface
//!    resolution (including the `EMPTY`/`UNMATERIALIZED` split that never mutates
//!    on a read), materialization/seeding with journals, claim-then-Mission
//!    locking, path-scoped commits with pinned bytes, typed landed/not-landed/
//!    indeterminate outcomes, and interrupted-journal recovery. [`planning`]
//!    adds the planning-commit partition/metadata/ancestry guards and
//!    fast-forward integration of planning commits and dependency lane tips.
//! 6. **What is protected, and which branch may this process write?** —
//!    [`protection`] resolves the live policy from `.kittify/config.yaml`
//!    (explicit list replaces defaults, `[]` unprotects, malformed fails closed),
//!    the remote default branch, the operator hatch, and a `single_branch`
//!    mission's own `commit_to_target`. [`topology`] supplies the branch/lane
//!    tips and checkout occupancy those decisions need.
//!
//! # Dependencies
//!
//! `sk-workspace` depends on `sk-mission` for Mission identity, metadata, and
//! topology, on `sk-kernel` for validated identifiers, bounded reads, atomic
//! writes, path confinement, and the cross-process file lock, and on `sk-git`
//! for read-only inspection and typed mutation plans. It never depends on a
//! higher layer, and it never shells out to Git itself: every Git operation goes
//! through the one `sk-git` seam. The canonical Mission lock is taken through
//! `sk-mission`'s door so the alias holds are never re-implemented here.
//!
//! # Placeholder ownership is impossible
//!
//! No function in this crate infers ownership, topology, or identity from a
//! guessed directory name. An [`owned::OwnedCheckout`] can only be minted from
//! a loaded `meta.json`, and a missing manifest is always an error.
//!
//! # Protected branches
//!
//! Allocation refuses to create a lane on a protected branch or on the
//! mission target branch, and a protected mission integration branch is
//! refused before creation. Coordination and planning commits resolve the live
//! policy with [`protection::ProtectionPolicy`] and pass the result to the Git
//! adapter; nothing here relies on the adapter's empty library default.
//! Nothing silently writes the default branch.

#![deny(missing_docs)]

pub mod allocation;
pub mod artifact;
pub mod coord;
pub mod coordination;
pub mod error;
pub mod journal;
pub mod lanes;
pub mod lifecycle;
pub mod naming;
pub mod owned;
pub mod planning;
pub mod policy;
pub mod protection;
pub mod surface;
pub mod topology;
pub mod workspace;

pub use error::WorkspaceError;

pub use coord::{
    COORD_JOURNAL_DIRECTORY, COORD_SEED_TRAILER, COORD_TRANSACTION_LOCK_TIMEOUT,
    CoordBootstrapReceipt, CoordCommitOutcome, CoordEstablishment, CoordJournal, CoordPhase,
    CoordReceipt, CoordRecord, CoordRecordKind, CoordRecoveryConflict, CoordRecoveryRepair,
    CoordRecoveryReport, CoordSeedReceipt, CoordSurfaceRead, CoordTransaction, LandedCommit,
    MissionHold, OwnershipProof, bootstrap_coordination, read_coordination_surface,
    recover_interrupted,
};
pub use planning::{
    DependencyIntegration, IntegrationError, IntegrationOutcome, IntegrationReport,
    IntegrationRollback, LandedAdvance, MAX_PLANNING_ARTIFACT_BYTES, MetaDemotionGuard, PinClass,
    PlanningCommitPlan, PlanningCommitReceipt, PlanningLockReceipt, PlanningPinMode,
    SkippedDependency, classify_recorded_pin, integrate_dependency_lanes,
    integrate_planning_commit, is_planning_kind, lane_contains_planning_commit, planning_paths,
    record_planning_commit, record_planning_commit_with, recorded_pin,
};
pub use protection::{
    CONFIG_REL_PATH, DEFAULT_PROTECTED_BRANCHES, PROTECTION_HATCH_ENV, ProtectionPolicy,
};
pub use topology::{
    BranchTip, CheckoutOccupancy, LANE_TIP_REF_PREFIX, LaneBranchTip, LaneOccupancy,
    LaneTipOutcome, LaneTipRecord, MissionBranchTips, OccupancyReport, RepoRootOccupancy,
    coordination_mission_dir, coordination_worktree_path, lane_tip_ref, mission_branch_tips,
    mission_occupancy, read_lane_tip, record_lane_tip, record_lane_tip_at,
};

pub use allocation::{
    AllocationRoute, LaneAllocation, LaneAllocator, LaneBaseDecision, LaneTopology,
    RecoveryConflict, RecoveryRepair, RecoveryReport, RetentionReport,
    guard_topology_matches_manifest,
};
pub use artifact::{
    ArtifactHome, ArtifactPartition, BASENAME_KIND, DIRECTORY_KIND, MissionArtifactKind,
    PartitionViolation, artifact_home_for, assert_partition_invariant, declared_read_surface,
    kind_for_basename, kind_for_directory, kind_for_mission_file, kind_is_coordination_residue,
};
pub use coordination::{
    PUBLISHED_WRITE_SHORT_CIRCUIT_KINDS, coordination_write_surface,
    is_published_write_short_circuit_kind,
};
pub use journal::{AllocationJournal, AllocationRecord, JournalPhase};
pub use lanes::{
    CollapseEvent, CollapseReport, CollapseRule, ExecutionLane, LANES_FILE_NAME,
    LANES_LOCK_DIRECTORY, LanePlannerInput, LaneReadiness, LanesManifest, LanesStore, WpPlan,
    compute_lanes, globs_overlap, infer_surfaces, lane_dependency_readiness, lanes_json_lock_path,
};
pub use lifecycle::{
    LifecycleEvidence, LifecyclePhase, Probe, baseline_merge_commit_from_meta,
    baseline_merge_commit_from_mission, derive_lifecycle_phase, probe_target_ref,
    terminal_completion,
};
pub use naming::{
    MissionIdentity, PLANNING_LANE_ID, code_lane_branch_name, coord_branch_name, coord_dir_name,
    coord_mission_dir_name, coord_reconstruct_branch, is_mid8_shape, lane_branch_name,
    legacy_branch_body, mid8_from_slug, mission_branch_name, mission_branch_name_required,
    mission_dir_name, strip_numeric_prefix, worktree_dir_name, worktree_path,
};
pub use owned::OwnedCheckout;
pub use policy::{
    DESTINATION_REF_INVALID, MutationPolicy, PROTECTED_BRANCH_REFUSED, PolicyVerdict,
};
pub use surface::{
    CommitTarget, CoordState, PlacementInputs, PlacementRef, ResolvedSurface, SurfaceLocationField,
    SurfaceLocations, SurfaceTotalityViolation, TopologySurface, assert_surface_totality,
    consolidated_location, coord_read_surface, resolve_artifact_surface, translate_surface,
};
pub use workspace::{
    CoordinationProbe, MissionWorkspace, ProjectWorkspace, mission_dir_from, repo_root_lane_wps,
};
