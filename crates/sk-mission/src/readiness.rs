use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use sk_kernel::WpId;

use crate::error::MissionError;
use crate::graph::WorkPackageGraph;

/// Caller-supplied work-package states, keyed by canonical id.
///
/// This crate never reconstructs state from an event log, from a
/// `status.json`, or from the retired frontmatter `lane`. The caller is the
/// authority for the snapshot it passes here.
pub type WpStateMap = BTreeMap<WpId, WpState>;

/// Canonical work-package lifecycle state.
///
/// `Genesis` and `Uninitialized` are non-display sentinels preserved from the
/// upstream model so a caller can distinguish "seeded but not planned" and
/// "absent from the snapshot" from a real lane. Neither is ever an acceptable
/// ending.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WpState {
    Genesis,
    Uninitialized,
    Planned,
    Claimed,
    InProgress,
    ForReview,
    InReview,
    Approved,
    Done,
    Blocked,
    Canceled,
}

impl WpState {
    /// Every state, in lifecycle order.
    pub const ALL: [Self; 11] = [
        Self::Genesis,
        Self::Uninitialized,
        Self::Planned,
        Self::Claimed,
        Self::InProgress,
        Self::ForReview,
        Self::InReview,
        Self::Approved,
        Self::Done,
        Self::Blocked,
        Self::Canceled,
    ];

    /// The canonical lowercase string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Genesis => "genesis",
            Self::Uninitialized => "uninitialized",
            Self::Planned => "planned",
            Self::Claimed => "claimed",
            Self::InProgress => "in_progress",
            Self::ForReview => "for_review",
            Self::InReview => "in_review",
            Self::Approved => "approved",
            Self::Done => "done",
            Self::Blocked => "blocked",
            Self::Canceled => "canceled",
        }
    }

    /// Whether this state satisfies a dependency.
    ///
    /// Only `approved` and `done` do. `approved` is required for intra-mission
    /// dependency ordering (a dependency cannot be `done` before the mission
    /// merge). `canceled` is *not* accepted here: upstream additionally accepts
    /// a cancellation carrying operator-authored provenance, which this
    /// read-only crate cannot verify from a state map alone.
    pub fn is_acceptable_ending(self) -> bool {
        matches!(self, Self::Approved | Self::Done)
    }
}

impl fmt::Display for WpState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for WpState {
    type Err = MissionError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|state| state.as_str() == value)
            .ok_or_else(|| MissionError::UnknownWorkPackageState {
                value: value.to_owned(),
            })
    }
}

/// Why one dependency does not satisfy readiness.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UnsatisfiedReason {
    /// The caller's state map has no entry for this dependency. This is
    /// distinct from an entry in a non-acceptable state.
    MissingState,
    /// The dependency is present but has not reached an acceptable ending.
    NotAcceptableEnding { state: WpState },
}

/// One unsatisfied dependency and the reason.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct UnsatisfiedDependency {
    dependency: WpId,
    reason: UnsatisfiedReason,
}

impl UnsatisfiedDependency {
    /// The dependency that is not satisfied.
    pub fn dependency(&self) -> &WpId {
        &self.dependency
    }

    /// Why it is not satisfied.
    pub fn reason(&self) -> &UnsatisfiedReason {
        &self.reason
    }
}

/// Dependency verdict for one work package.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DependencyReadiness {
    wp: WpId,
    dependencies: Vec<WpId>,
    unsatisfied: Vec<UnsatisfiedDependency>,
}

impl DependencyReadiness {
    /// The work package the verdict is about.
    pub fn wp(&self) -> &WpId {
        &self.wp
    }

    /// Declared dependencies, in declaration order.
    pub fn dependencies(&self) -> &[WpId] {
        &self.dependencies
    }

    /// Dependencies that are not satisfied, in declaration order.
    pub fn unsatisfied(&self) -> &[UnsatisfiedDependency] {
        &self.unsatisfied
    }

    /// Whether every declared dependency has reached an acceptable ending.
    pub fn satisfied(&self) -> bool {
        self.unsatisfied.is_empty()
    }

    fn evaluate(wp: &WpId, dependencies: &[WpId], states: &WpStateMap) -> Self {
        let unsatisfied = dependencies
            .iter()
            .filter_map(|dependency| match states.get(dependency) {
                None => Some(UnsatisfiedDependency {
                    dependency: dependency.clone(),
                    reason: UnsatisfiedReason::MissingState,
                }),
                Some(state) if !state.is_acceptable_ending() => Some(UnsatisfiedDependency {
                    dependency: dependency.clone(),
                    reason: UnsatisfiedReason::NotAcceptableEnding { state: *state },
                }),
                Some(_) => None,
            })
            .collect();

        Self {
            wp: wp.clone(),
            dependencies: dependencies.to_vec(),
            unsatisfied,
        }
    }
}

/// Return the dependency verdict for one work package against an explicit
/// caller-supplied state map.
///
/// This is the pure core: it needs only the declared dependencies, so a caller
/// can evaluate a work package that is not in a graph.
pub fn readiness_for_dependencies(
    wp: &WpId,
    dependencies: &[WpId],
    states: &WpStateMap,
) -> DependencyReadiness {
    DependencyReadiness::evaluate(wp, dependencies, states)
}

/// Return the dependency verdict for a work package in `graph`.
///
/// Fails when `wp` is not part of the graph. A work package with no declared
/// dependencies is satisfied (vacuously), never "unknown".
pub fn dependency_readiness(
    graph: &WorkPackageGraph,
    states: &WpStateMap,
    wp: &WpId,
) -> Result<DependencyReadiness, MissionError> {
    let dependencies = graph
        .dependencies_of(wp)
        .ok_or_else(|| MissionError::UnknownWorkPackage { wp: wp.clone() })?;
    Ok(readiness_for_dependencies(wp, dependencies, states))
}

/// Every work package whose declared dependencies are satisfied, in graph
/// topological order.
///
/// This answers "what can start?"; it deliberately does not filter by the
/// work package's own state, which belongs to the caller.
pub fn ready_work_packages(
    graph: &WorkPackageGraph,
    states: &WpStateMap,
) -> Result<Vec<WpId>, MissionError> {
    let mut ready = Vec::new();
    for wp in graph.topological_order() {
        let dependencies = graph
            .dependencies_of(wp)
            .ok_or_else(|| MissionError::UnknownWorkPackage { wp: wp.clone() })?;
        if readiness_for_dependencies(wp, dependencies, states).satisfied() {
            ready.push(wp.clone());
        }
    }
    Ok(ready)
}
