//! Exhaustive pure projections: artifact kinds, partitions, topologies, and
//! surfaces.
//!
//! These tests assert the *whole* vocabulary rather than representative
//! examples: all 23 artifact kinds, all 4 topologies, all 5 surfaces, and every
//! `(kind, topology, coord-state)` cell of the resolved-surface matrix.

use std::path::PathBuf;

use sk_mission::{MissionTopology, routes_through_coordination};
use sk_workspace::{
    BASENAME_KIND, CoordState, DIRECTORY_KIND, MissionArtifactKind, PlacementInputs,
    SurfaceLocations, TopologySurface, artifact_home_for, assert_partition_invariant,
    assert_surface_totality, declared_read_surface, kind_for_directory, kind_for_mission_file,
    kind_is_coordination_residue, resolve_artifact_surface, translate_surface,
};

const ALL_STATES: [CoordState; 5] = [
    CoordState::None,
    CoordState::Empty,
    CoordState::Unmaterialized,
    CoordState::Materialized,
    CoordState::Deleted,
];

#[test]
fn twenty_three_kinds_are_partitioned_disjointly_and_exhaustively() {
    assert_eq!(MissionArtifactKind::ALL.len(), 23);
    assert_partition_invariant().expect("partition invariant");

    let mut primary = 0;
    let mut coord = 0;
    for kind in MissionArtifactKind::ALL {
        let home = artifact_home_for(kind);
        assert_eq!(home.kind, kind);
        assert_eq!(home.partition, kind.partition());
        assert_eq!(home.read_surface, home.write_surface, "read/write symmetry");
        if kind.is_primary_partition() {
            primary += 1;
            assert_eq!(home.read_surface, TopologySurface::Primary);
        } else {
            coord += 1;
            assert_eq!(home.read_surface, TopologySurface::Coord);
        }
        assert_eq!(
            MissionArtifactKind::from_stored(kind.as_str()),
            Some(kind),
            "wire round-trip for {kind}"
        );
    }
    assert_eq!((primary, coord), (17, 6));
    assert_eq!(primary + coord, MissionArtifactKind::ALL.len());
}

#[test]
fn home_projection_is_total_over_the_four_topologies() {
    for topology in MissionTopology::ALL {
        for kind in MissionArtifactKind::ALL {
            let declared = declared_read_surface(kind, topology);
            let expected = if kind.is_primary_partition() {
                TopologySurface::Primary
            } else if routes_through_coordination(topology) {
                TopologySurface::Coord
            } else {
                TopologySurface::Primary
            };
            assert_eq!(declared, expected, "{kind} under {topology}");

            let residue = kind_is_coordination_residue(kind, topology);
            assert_eq!(
                residue,
                kind.is_coord_partition() && routes_through_coordination(topology),
                "residue projection for {kind} under {topology}"
            );
        }
    }
}

#[test]
fn surface_vocabulary_is_total_and_refuses_unresolved_locations() {
    assert_surface_totality().expect("surface totality");
    let full = SurfaceLocations::primary("/p")
        .with_coord("/c")
        .with_lane("/l")
        .with_consolidated("/z")
        .with_temp("/t");
    for surface in TopologySurface::ALL {
        assert!(translate_surface(surface, &full).is_ok(), "{surface}");
    }

    let primary_only = SurfaceLocations::primary("/p");
    assert_eq!(
        translate_surface(TopologySurface::Primary, &primary_only).unwrap(),
        std::path::Path::new("/p")
    );
    for surface in [
        TopologySurface::Coord,
        TopologySurface::Lane,
        TopologySurface::Consolidated,
        TopologySurface::Temp,
    ] {
        assert!(translate_surface(surface, &primary_only).is_err());
    }
}

#[test]
fn basename_and_directory_classifiers_cover_every_table_entry() {
    for (name, kind) in BASENAME_KIND {
        let path = format!("kitty-specs/my-mission/{name}");
        assert_eq!(
            kind_for_mission_file(&path, Some("my-mission")),
            Some(*kind),
            "{name}"
        );
        // A different mission's file is never classified.
        assert_eq!(kind_for_mission_file(&path, Some("other")), None);
    }
    for (name, kind) in DIRECTORY_KIND {
        let path = format!("kitty-specs/my-mission/{name}/nested/file.bin");
        assert_eq!(kind_for_mission_file(&path, None), Some(*kind), "{name}");
        assert_eq!(kind_for_directory(name), Some(*kind));
    }

    // Filename-anchored review cycle, not directory-anchored.
    assert_eq!(
        kind_for_mission_file(
            "kitty-specs/my-mission/tasks/WP01/review-cycle-2.md",
            Some("my-mission")
        ),
        Some(MissionArtifactKind::ReviewCycle)
    );
    assert_eq!(
        kind_for_mission_file(
            "kitty-specs/my-mission/tasks/WP01/baseline-tests.json",
            None
        ),
        Some(MissionArtifactKind::WorkPackageTask)
    );
    // Unrecognised and out-of-tree paths classify to None.
    assert_eq!(
        kind_for_mission_file("kitty-specs/my-mission/unknown.txt", None),
        None
    );
    assert_eq!(kind_for_mission_file("kitty-specs/my-mission", None), None);
    assert_eq!(kind_for_mission_file("src/lib.rs", None), None);
}

#[test]
fn resolved_surface_matrix_is_exhaustive_over_kind_topology_and_state() {
    let primary = PathBuf::from("/primary");
    let coord = PathBuf::from("/coord");

    for topology in MissionTopology::ALL {
        for kind in MissionArtifactKind::ALL {
            for state in ALL_STATES {
                let inputs = PlacementInputs {
                    kind,
                    mission_slug: "my-mission",
                    topology,
                    primary_dir: &primary,
                    coordination_branch: Some("kitty/mission-my-mission-01KNXQS9"),
                    coord_state: state,
                    coord_dir: Some(&coord),
                    phase: sk_workspace::LifecyclePhase::PreConsolidation,
                };
                let result = resolve_artifact_surface(&inputs);
                let routing = routes_through_coordination(topology);
                if kind.is_primary_partition() || !routing {
                    let resolved = result.expect("primary resolution");
                    assert_eq!(resolved.path, primary, "{kind}/{topology}/{state:?}");
                    assert_eq!(resolved.surface_kind, TopologySurface::Primary);
                } else {
                    match state {
                        CoordState::Deleted => {
                            assert!(result.is_err(), "{kind}/{topology}/{state:?}");
                        }
                        CoordState::Unmaterialized => {
                            assert!(result.is_err(), "{kind}/{topology}/{state:?}");
                        }
                        CoordState::Materialized => {
                            let resolved = result.expect("coord resolution");
                            assert_eq!(resolved.path, coord);
                            assert_eq!(resolved.surface_kind, TopologySurface::Coord);
                        }
                        CoordState::None | CoordState::Empty => {
                            let resolved = result.expect("declared create-window answer");
                            assert_eq!(resolved.path, primary);
                            assert_eq!(resolved.surface_kind, TopologySurface::Primary);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn consolidated_location_exists_only_after_consolidation() {
    let primary = PathBuf::from("/primary");
    assert_eq!(
        sk_workspace::consolidated_location(
            &primary,
            sk_workspace::LifecyclePhase::PreConsolidation
        ),
        None
    );
    for phase in [
        sk_workspace::LifecyclePhase::Consolidated,
        sk_workspace::LifecyclePhase::Published,
    ] {
        assert_eq!(
            sk_workspace::consolidated_location(&primary, phase),
            Some(primary.clone())
        );
    }
}

#[test]
fn kind_basenames_invert_the_classifier_without_directory_guessing() {
    let issue = MissionArtifactKind::IssueMatrix.basenames();
    assert!(issue.contains("issue-matrix.md"));
    assert!(issue.contains("issue-matrix.json"));
    let checklist = MissionArtifactKind::Checklist.basenames();
    assert_eq!(checklist, ["quickstart.md"].into_iter().collect());
    assert!(MissionArtifactKind::TracerFile.basenames().is_empty());
}
