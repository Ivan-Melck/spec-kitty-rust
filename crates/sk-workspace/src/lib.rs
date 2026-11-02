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
//!
//! # Dependencies
//!
//! `sk-workspace` depends on `sk-mission` for Mission identity, metadata, and
//! topology, on `sk-kernel` for validated identifiers, bounded reads, atomic
//! writes, and the cross-process file lock, and on `sk-git` for read-only
//! inspection and typed mutation plans. It never depends on a higher layer, and
//! it never shells out to Git itself: every Git operation goes through the one
//! `sk-git` seam.
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
//! refused before creation. Nothing here silently writes the default branch.

#![deny(missing_docs)]

pub mod allocation;
pub mod artifact;
pub mod coordination;
pub mod error;
pub mod journal;
pub mod lanes;
pub mod lifecycle;
pub mod naming;
pub mod owned;
pub mod policy;
pub mod surface;
pub mod workspace;

pub use error::WorkspaceError;

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
    CollapseEvent, CollapseReport, CollapseRule, ExecutionLane, LANES_FILE_NAME, LanePlannerInput,
    LaneReadiness, LanesManifest, LanesStore, WpPlan, compute_lanes, globs_overlap, infer_surfaces,
    lane_dependency_readiness,
};
pub use lifecycle::{
    LifecycleEvidence, LifecyclePhase, Probe, baseline_merge_commit_from_meta,
    baseline_merge_commit_from_mission, derive_lifecycle_phase, probe_target_ref,
    terminal_completion,
};
pub use naming::{
    MissionIdentity, PLANNING_LANE_ID, code_lane_branch_name, coord_branch_name, coord_dir_name,
    coord_mission_dir_name, coord_reconstruct_branch, is_mid8_shape, lane_branch_name,
    mid8_from_slug, mission_branch_name, mission_branch_name_required, mission_dir_name,
    strip_numeric_prefix, worktree_dir_name, worktree_path,
};
pub use owned::OwnedCheckout;
pub use policy::{
    DESTINATION_REF_INVALID, MutationPolicy, PROTECTED_BRANCH_REFUSED, PolicyVerdict,
};
pub use surface::{
    CommitTarget, CoordState, PlacementInputs, PlacementRef, ResolvedSurface, SurfaceLocationField,
    SurfaceLocations, SurfaceTotalityViolation, TopologySurface, assert_surface_totality,
    consolidated_location, resolve_artifact_surface, translate_surface,
};
pub use workspace::{
    CoordinationProbe, MissionWorkspace, ProjectWorkspace, mission_dir_from, repo_root_lane_wps,
};
