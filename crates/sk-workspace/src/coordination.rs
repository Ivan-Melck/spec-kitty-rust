//! The write-side coordination decision: which surface a write targets.
//!
//! The read side ([`crate::surface::resolve_artifact_surface`]) answers "where
//! can this artifact be read right now". The write side is stricter: it never
//! substitutes the primary checkout for a coordination write that must land in
//! the coordination worktree, and it honours the post-consolidation
//! short-circuit for the bookkeeping kinds that are written after the
//! coordination surface has been torn down.
//!
//! This is the pure decision. Actually materializing or seeding a coordination
//! worktree belongs to the coordination transaction writer; this module refuses
//! an unmaterialized coordination surface rather than pretending the write
//! landed.

use sk_mission::{MissionTopology, routes_through_coordination};

use crate::artifact::MissionArtifactKind;
use crate::error::WorkspaceError;
use crate::lifecycle::LifecyclePhase;
use crate::surface::{CoordState, TopologySurface};

/// Bookkeeping kinds that are written to the primary Mission directory once the
/// mission is `published` (E2), even though they are COORD-partition kinds.
///
/// Upstream `PlacementSeam.write_dir` checks this set before probing the
/// coordination surface, so a published mission whose coordination branch was
/// torn down by consolidation can never raise `CoordinationBranchDeleted` here
/// and is never written into a torn-down worktree.
pub const PUBLISHED_WRITE_SHORT_CIRCUIT_KINDS: [MissionArtifactKind; 5] = [
    MissionArtifactKind::AcceptanceMatrix,
    MissionArtifactKind::IssueMatrix,
    MissionArtifactKind::StatusState,
    MissionArtifactKind::TracerFile,
    MissionArtifactKind::ReviewCycle,
];

/// Is this kind eligible for the published/E2 primary short-circuit?
///
/// `DecisionLog` is deliberately absent: it is in-mission-only by design
/// (ADR 2026-07-30-1 Decision 1 §6) and must not move to the primary checkout
/// after consolidation.
pub fn is_published_write_short_circuit_kind(kind: MissionArtifactKind) -> bool {
    PUBLISHED_WRITE_SHORT_CIRCUIT_KINDS.contains(&kind)
}

/// The surface a write of `kind` must target.
///
/// * a PRIMARY-partition kind, or any kind on a coord-less topology, targets
///   the primary Mission directory — no coordination probe, no side effect;
/// * a published short-circuit kind on a `published` mission targets the
///   primary directory without probing coordination;
/// * any other COORD-partition kind on a coord-routing topology requires a
///   materialized coordination surface: `MATERIALIZED` targets `COORD`, while
///   `DELETED` and `UNMATERIALIZED` refuse, and `NONE`/`EMPTY` refuse because
///   this crate does not seed a coordination surface.
pub fn coordination_write_surface(
    kind: MissionArtifactKind,
    topology: MissionTopology,
    coord_state: CoordState,
    phase: LifecyclePhase,
    mission_slug: &str,
    coordination_branch: Option<&str>,
) -> Result<TopologySurface, WorkspaceError> {
    if kind.is_primary_partition() || !routes_through_coordination(topology) {
        return Ok(TopologySurface::Primary);
    }
    if phase == LifecyclePhase::Published && is_published_write_short_circuit_kind(kind) {
        return Ok(TopologySurface::Primary);
    }
    match coord_state {
        CoordState::Materialized => Ok(TopologySurface::Coord),
        CoordState::Deleted => Err(WorkspaceError::CoordinationBranchDeleted {
            mission: mission_slug.to_owned(),
            branch: coordination_branch.unwrap_or("<declared>").to_owned(),
        }),
        CoordState::None | CoordState::Empty | CoordState::Unmaterialized => {
            Err(WorkspaceError::CoordinationWorktreeUnmaterialized {
                mission: mission_slug.to_owned(),
                branch: coordination_branch.unwrap_or("<declared>").to_owned(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SLUG: &str = "my-mission";
    const COORD: Option<&str> = Some("kitty/mission-my-mission-01KNXQS9");

    #[test]
    fn published_short_circuit_is_exact() {
        for kind in MissionArtifactKind::ALL {
            let expected = matches!(
                kind,
                MissionArtifactKind::AcceptanceMatrix
                    | MissionArtifactKind::IssueMatrix
                    | MissionArtifactKind::StatusState
                    | MissionArtifactKind::TracerFile
                    | MissionArtifactKind::ReviewCycle
            );
            assert_eq!(
                is_published_write_short_circuit_kind(kind),
                expected,
                "{kind}"
            );
            assert!(
                !is_published_write_short_circuit_kind(MissionArtifactKind::DecisionLog),
                "the decision log stays in-mission"
            );
        }
    }

    #[test]
    fn primary_partition_never_probes_coordination() {
        for state in [
            CoordState::None,
            CoordState::Empty,
            CoordState::Unmaterialized,
            CoordState::Materialized,
            CoordState::Deleted,
        ] {
            assert_eq!(
                coordination_write_surface(
                    MissionArtifactKind::Spec,
                    MissionTopology::LanesWithCoord,
                    state,
                    LifecyclePhase::PreConsolidation,
                    SLUG,
                    COORD,
                )
                .unwrap(),
                TopologySurface::Primary
            );
        }
    }

    #[test]
    fn coord_less_topology_writes_primary_affirmatively() {
        for kind in MissionArtifactKind::ALL {
            assert_eq!(
                coordination_write_surface(
                    kind,
                    MissionTopology::Lanes,
                    CoordState::None,
                    LifecyclePhase::PreConsolidation,
                    SLUG,
                    None,
                )
                .unwrap(),
                TopologySurface::Primary
            );
        }
    }

    #[test]
    fn published_short_circuit_beats_a_torn_down_surface() {
        for kind in PUBLISHED_WRITE_SHORT_CIRCUIT_KINDS {
            assert_eq!(
                coordination_write_surface(
                    kind,
                    MissionTopology::Coord,
                    CoordState::Deleted,
                    LifecyclePhase::Published,
                    SLUG,
                    COORD,
                )
                .unwrap(),
                TopologySurface::Primary
            );
        }
        // DecisionLog is not eligible: it still refuses.
        assert!(
            coordination_write_surface(
                MissionArtifactKind::DecisionLog,
                MissionTopology::Coord,
                CoordState::Deleted,
                LifecyclePhase::Published,
                SLUG,
                COORD,
            )
            .is_err()
        );
    }

    #[test]
    fn coordination_write_requires_materialization() {
        for kind in [
            MissionArtifactKind::StatusState,
            MissionArtifactKind::DecisionLog,
            MissionArtifactKind::TracerFile,
        ] {
            assert_eq!(
                coordination_write_surface(
                    kind,
                    MissionTopology::Coord,
                    CoordState::Materialized,
                    LifecyclePhase::PreConsolidation,
                    SLUG,
                    COORD,
                )
                .unwrap(),
                TopologySurface::Coord
            );
            for state in [
                CoordState::Deleted,
                CoordState::Unmaterialized,
                CoordState::Empty,
                CoordState::None,
            ] {
                assert!(
                    coordination_write_surface(
                        kind,
                        MissionTopology::Coord,
                        state,
                        LifecyclePhase::PreConsolidation,
                        SLUG,
                        COORD,
                    )
                    .is_err(),
                    "{kind} must refuse {state:?}"
                );
            }
        }
    }
}
