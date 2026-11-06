//! A release-mode throughput measurement for the pure placement projections.
//!
//! Run with:
//!
//! ```sh
//! cargo run --release -p sk-workspace --example projection_bench
//! ```
//!
//! This measures the native Rust pure paths only: the complete
//! `(23 kinds × 4 topologies × 5 surfaces)` surface-resolution matrix and lane
//! planning for a mission with many work packages. It is deliberately not a
//! Python/Rust ratio: the aggregate differential harness lives with the
//! workspace benchmark tooling. The output reports raw nanoseconds per
//! operation plus a geometric mean so a coordinator can compare runs.
//!
//! It performs real work (no constant-output shortcut), checks the answer is
//! deterministic across repetitions, and never touches the filesystem or Git.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use sk_kernel::WpId;
use sk_mission::MissionTopology;
use sk_workspace::{
    CoordState, LanePlannerInput, LifecyclePhase, MissionArtifactKind, PlacementInputs,
    TopologySurface, WpPlan, compute_lanes, resolve_artifact_surface,
};

fn geometric_mean(samples: &[f64]) -> f64 {
    let sum: f64 = samples.iter().map(|value| value.ln()).sum();
    (sum / samples.len() as f64).exp()
}

fn bench<F: FnMut() -> u64>(label: &str, iterations: u64, mut body: F) -> f64 {
    let start = Instant::now();
    let mut checksum = 0u64;
    for _ in 0..iterations {
        checksum = checksum.wrapping_add(body());
    }
    let elapsed = start.elapsed();
    let per_op = elapsed.as_nanos() as f64 / iterations as f64;
    println!(
        "{label:<34} {iterations:>8} ops  {per_op:>10.1} ns/op  {:>12.0} ops/s  checksum={checksum}",
        iterations as f64 / elapsed.as_secs_f64()
    );
    per_op
}

fn main() {
    let all_states = [
        CoordState::None,
        CoordState::Empty,
        CoordState::Unmaterialized,
        CoordState::Materialized,
        CoordState::Deleted,
    ];
    let primary = PathBuf::from("/bench/primary");
    let coord = PathBuf::from("/bench/coord");

    let mut surface_samples = Vec::new();
    let per_matrix = bench("resolve 460-cell surface matrix", 2_000, || {
        let mut hits = 0u64;
        for topology in MissionTopology::ALL {
            for kind in MissionArtifactKind::ALL {
                for state in all_states {
                    let inputs = PlacementInputs {
                        kind,
                        mission_slug: "bench-mission",
                        topology,
                        primary_dir: &primary,
                        coordination_branch: Some("kitty/mission-bench-01KNXQS9"),
                        coord_state: state,
                        coord_dir: Some(&coord),
                        phase: LifecyclePhase::PreConsolidation,
                    };
                    match resolve_artifact_surface(&inputs) {
                        Ok(resolved) => {
                            hits += match resolved.surface_kind {
                                TopologySurface::Primary => 1,
                                TopologySurface::Coord => 2,
                                TopologySurface::Lane => 3,
                                TopologySurface::Consolidated => 4,
                                TopologySurface::Temp => 5,
                            }
                        }
                        Err(_) => hits += 0,
                    }
                }
            }
        }
        hits
    });
    println!(
        "  (per resolved cell: {:.1} ns)",
        per_matrix
            / (MissionArtifactKind::ALL.len() * MissionTopology::ALL.len() * all_states.len())
                as f64
    );
    surface_samples.push(
        per_matrix
            / (MissionArtifactKind::ALL.len() * MissionTopology::ALL.len() * all_states.len())
                as f64,
    );

    // 24 work packages with disjoint scopes: the union-find sees all pairs,
    // and the lane-id grammar (lane-a..lane-x) is not exhausted.
    let mut input = LanePlannerInput::new(
        "bench-mission".parse().unwrap(),
        MissionTopology::Lanes,
        "main",
    );
    let mut ids = Vec::new();
    for index in 0..24 {
        let id: WpId = format!("WP{index:02}").parse().unwrap();
        input.work_packages.insert(
            id.clone(),
            WpPlan::code(id.clone(), [format!("src/module-{index}/**")]),
        );
        input.dependencies.insert(id.clone(), Vec::new());
        ids.push(id);
    }
    // A dependency chain keeps lane-level ordering in the measured path.
    for pair in ids.windows(2) {
        input
            .dependencies
            .get_mut(&pair[1])
            .expect("dependency entry")
            .push(pair[0].clone());
    }
    let lane_samples = vec![bench("compute_lanes 24 WPs", 2_000, || {
        let manifest = compute_lanes(&input).expect("lane plan");
        1 + manifest.lanes.len() as u64
    })];

    // Classifier throughput over the complete basename table.
    let basenames: Vec<String> = sk_workspace::BASENAME_KIND
        .iter()
        .map(|(name, _)| format!("kitty-specs/bench-mission/{name}"))
        .collect();
    let per_table = bench("classify 28 basenames", 200_000, || {
        let mut hits = 0u64;
        for path in &basenames {
            if sk_workspace::kind_for_mission_file(path, Some("bench-mission")).is_some() {
                hits += 1;
            }
        }
        hits
    });
    println!(
        "  (per classified path: {:.1} ns)",
        per_table / basenames.len() as f64
    );
    let classifier_samples = vec![per_table / basenames.len() as f64];

    let mut all = surface_samples;
    all.extend(lane_samples);
    all.extend(classifier_samples);
    let geomean = geometric_mean(&all);
    println!(
        "\ngeometric mean across {} measured paths: {geomean:.1} ns/op",
        all.len()
    );

    // Determinism: the same input must always produce the same lane manifest.
    let first = compute_lanes(&input).unwrap();
    let second = compute_lanes(&input).unwrap();
    assert_eq!(
        first.lanes, second.lanes,
        "lane planning must be deterministic"
    );
    assert_eq!(first.planning_artifact_wps, Vec::<WpId>::new());
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for lane in &first.lanes {
        *counts.entry(lane.lane_id.clone()).or_default() += 1;
    }
    assert_eq!(counts.len(), first.lanes.len());
}
