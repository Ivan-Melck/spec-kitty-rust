//! Mission artifact kinds and their topology-independent homes.
//!
//! This module is the Rust projection of upstream
//! `src/mission_runtime/artifacts.py`. It is deliberately pure: no filesystem
//! I/O, no Git, no time. Every answer is a function of the artifact kind and
//! the mission's *stored* topology, so the same question asked from any
//! checkout returns the same answer.
//!
//! # The two partitions
//!
//! Every artifact kind belongs to exactly one partition:
//!
//! * **PRIMARY** — planning and identity artifacts. They live with the
//!   mission on the primary `target_branch` for *every* topology and never
//!   transit the coordination branch.
//! * **COORD** — coordination-owned bookkeeping. It resolves to the
//!   coordination surface only when the stored topology routes through
//!   coordination; on a coord-less topology its declared home *is* the
//!   primary surface.
//!
//! The partitions are disjoint and jointly exhaustive over
//! [`MissionArtifactKind::ALL`]. Rust cannot enforce that as a type invariant,
//! so [`assert_partition_invariant`] checks it and the crate's tests assert it
//! over the full 23-kind vocabulary.

use std::collections::BTreeSet;

use sk_mission::{MissionTopology, routes_through_coordination};

use crate::surface::TopologySurface;

/// The `kitty-specs/` directory name that anchors mission artifacts.
pub const SPECS_DIR_NAME: &str = "kitty-specs";

/// Every mission artifact category whose home can differ by topology.
///
/// The declaration order is the upstream enum order and is used by
/// [`MissionArtifactKind::ALL`] for exhaustive checks and deterministic output.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum MissionArtifactKind {
    /// `meta.json` — primary identity metadata.
    PrimaryMetadata,
    /// `plan.md` — the finalized execution plan.
    FinalizedExecutionPlan,
    /// `tasks.md` / `wps.yaml` — the task index and its structured source.
    TasksIndex,
    /// `tasks/WP*.md` and the per-WP baseline — the work-package prompt.
    WorkPackageTask,
    /// `lanes.json` — the lane manifest.
    LaneState,
    /// `acceptance-matrix.json` — accept-time verification.
    AcceptanceMatrix,
    /// `issue-matrix.{md,json}` — authored verdicts.
    IssueMatrix,
    /// `status.events.jsonl` / `status.json` — status state.
    StatusState,
    /// `analysis-report.md` (and audit/gap variants).
    AnalysisReport,
    /// `spec.md` — the mission specification.
    Spec,
    /// `data-model.md`.
    DataModel,
    /// `research.md` / `research/` — the sw-dev research note.
    Research,
    /// `checklists/` / `quickstart.md`.
    Checklist,
    /// `retrospective.yaml` — the terminal post-merge record.
    Retrospective,
    /// `decisions.events.jsonl` — the decision event log.
    DecisionLog,
    /// `traces/` — mission tracer files.
    TracerFile,
    /// `tasks/<wp>/review-cycle-<N>.md` — per-WP review bookkeeping.
    ReviewCycle,
    /// `decisions/index.json` and `decisions/DM-<ulid>.md` — the DM ledger.
    DecisionLedger,
    /// `contracts/**` — plan-output interface contracts.
    Contract,
    /// Research-mission deliverables (`findings.md`, `report.md`, …).
    ResearchOutput,
    /// `occurrence_map.yaml` — bulk-edit occurrence classification.
    OccurrenceMap,
    /// `goals.md` — the plan mission's goals.
    MissionGoals,
    /// `release.md` — the documentation mission's publish handoff.
    ReleaseRecord,
}

impl MissionArtifactKind {
    /// Every artifact kind, in declaration order.
    pub const ALL: [Self; 23] = [
        Self::PrimaryMetadata,
        Self::FinalizedExecutionPlan,
        Self::TasksIndex,
        Self::WorkPackageTask,
        Self::LaneState,
        Self::AcceptanceMatrix,
        Self::IssueMatrix,
        Self::StatusState,
        Self::AnalysisReport,
        Self::Spec,
        Self::DataModel,
        Self::Research,
        Self::Checklist,
        Self::Retrospective,
        Self::DecisionLog,
        Self::TracerFile,
        Self::ReviewCycle,
        Self::DecisionLedger,
        Self::Contract,
        Self::ResearchOutput,
        Self::OccurrenceMap,
        Self::MissionGoals,
        Self::ReleaseRecord,
    ];

    /// The canonical wire string (upstream enum value).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PrimaryMetadata => "primary_metadata",
            Self::FinalizedExecutionPlan => "finalized_execution_plan",
            Self::TasksIndex => "tasks_index",
            Self::WorkPackageTask => "work_package_task",
            Self::LaneState => "lane_state",
            Self::AcceptanceMatrix => "acceptance_matrix",
            Self::IssueMatrix => "issue_matrix",
            Self::StatusState => "status_state",
            Self::AnalysisReport => "analysis_report",
            Self::Spec => "spec",
            Self::DataModel => "data_model",
            Self::Research => "research",
            Self::Checklist => "checklist",
            Self::Retrospective => "retrospective",
            Self::DecisionLog => "decision_log",
            Self::TracerFile => "tracer_file",
            Self::ReviewCycle => "review_cycle",
            Self::DecisionLedger => "decision_ledger",
            Self::Contract => "contract",
            Self::ResearchOutput => "research_output",
            Self::OccurrenceMap => "occurrence_map",
            Self::MissionGoals => "mission_goals",
            Self::ReleaseRecord => "release_record",
        }
    }

    /// Parse a canonical wire string. Unknown values return `None`; they are
    /// never silently coerced to a default kind.
    pub fn from_stored(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == raw)
    }

    /// The artifact's partition.
    pub fn partition(self) -> ArtifactPartition {
        if Self::primary_kinds().contains(&self) {
            ArtifactPartition::Primary
        } else {
            ArtifactPartition::Coord
        }
    }

    /// Is this a PRIMARY-partition artifact (lands on `target_branch`)?
    pub fn is_primary_partition(self) -> bool {
        self.partition() == ArtifactPartition::Primary
    }

    /// Is this a COORD-partition artifact (coordination-owned bookkeeping)?
    pub fn is_coord_partition(self) -> bool {
        self.partition() == ArtifactPartition::Coord
    }

    /// The PRIMARY partition (upstream `_PRIMARY_ARTIFACT_KINDS`).
    pub fn primary_kinds() -> &'static [Self] {
        // 17 PRIMARY-partition kinds. Kept as one authoritative list; the
        // tests assert every member is unique and in `ALL`.
        &[
            Self::Spec,
            Self::DataModel,
            Self::Research,
            Self::Checklist,
            Self::Contract,
            Self::FinalizedExecutionPlan,
            Self::TasksIndex,
            Self::WorkPackageTask,
            Self::LaneState,
            Self::PrimaryMetadata,
            Self::Retrospective,
            Self::AnalysisReport,
            Self::DecisionLedger,
            Self::ResearchOutput,
            Self::OccurrenceMap,
            Self::MissionGoals,
            Self::ReleaseRecord,
        ]
    }

    /// The COORD partition (upstream `_PLACEMENT_ARTIFACT_KINDS`).
    pub fn coord_kinds() -> &'static [Self] {
        &[
            Self::AcceptanceMatrix,
            Self::IssueMatrix,
            Self::StatusState,
            Self::DecisionLog,
            Self::TracerFile,
            Self::ReviewCycle,
        ]
    }

    /// Basenames that classify to this kind (the inverse classifier leg).
    ///
    /// Directory-anchored kinds have no basename entry and yield an empty set;
    /// a kind with both legs (for example `Checklist`) yields only its
    /// basename entries, never the directory's files.
    pub fn basenames(self) -> BTreeSet<&'static str> {
        BASENAME_KIND
            .iter()
            .filter(|(_, kind)| *kind == self)
            .map(|(name, _)| *name)
            .collect()
    }
}

impl std::fmt::Display for MissionArtifactKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The two swappable artifact partitions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArtifactPartition {
    /// Planning and identity artifacts: primary surface for every topology.
    Primary,
    /// Coordination-owned artifacts: coordination surface only when routed.
    Coord,
}

impl ArtifactPartition {
    /// The surface this partition declares, before topology is considered.
    pub fn declared_surface(self) -> TopologySurface {
        match self {
            Self::Primary => TopologySurface::Primary,
            Self::Coord => TopologySurface::Coord,
        }
    }

    /// Is this partition routed through coordination under `topology`?
    pub fn routes_through_coordination(self, topology: MissionTopology) -> bool {
        self == Self::Coord && routes_through_coordination(topology)
    }
}

/// The declared read/write home for one artifact kind.
///
/// The commit target is always the placement seam's single destination ref
/// (upstream `MissionArtifactHome.commit_target` is the carried
/// `placement_ref` for both partitions), so it is not a per-kind branch name
/// here. Callers obtain the ref from [`crate::surface::PlacementRef`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArtifactHome {
    /// The artifact kind.
    pub kind: MissionArtifactKind,
    /// The partition the kind belongs to.
    pub partition: ArtifactPartition,
    /// Where a read resolves (declared, materialization-blind).
    pub read_surface: TopologySurface,
    /// Where a write resolves (declared, before coordination materialization).
    pub write_surface: TopologySurface,
}

/// Resolve the artifact-home contract for `kind`.
///
/// This is total: every [`MissionArtifactKind`] has exactly one answer.
pub fn artifact_home_for(kind: MissionArtifactKind) -> ArtifactHome {
    let partition = kind.partition();
    ArtifactHome {
        kind,
        partition,
        read_surface: partition.declared_surface(),
        write_surface: partition.declared_surface(),
    }
}

/// The declared, materialization-blind read surface for `kind`.
///
/// * PRIMARY-partition kinds declare `PRIMARY` for every topology.
/// * COORD-partition kinds declare `COORD` only when the stored topology
///   routes through coordination; otherwise they declare `PRIMARY`, which is
///   their real home on a coord-less topology, not a fallback.
pub fn declared_read_surface(
    kind: MissionArtifactKind,
    topology: MissionTopology,
) -> TopologySurface {
    if kind.is_primary_partition() {
        return TopologySurface::Primary;
    }
    if routes_through_coordination(topology) {
        TopologySurface::Coord
    } else {
        TopologySurface::Primary
    }
}

/// Is `kind` coordination residue under `topology`?
///
/// Residue is a *stored-topology* projection: a COORD-partition kind is
/// residue only when the mission routes through coordination. A coord-less
/// topology has no primary/coordination split, so nothing is residue there.
pub fn kind_is_coordination_residue(kind: MissionArtifactKind, topology: MissionTopology) -> bool {
    routes_through_coordination(topology) && kind.is_coord_partition()
}

/// A violated partition invariant.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PartitionViolation {
    /// A kind appears in both partitions.
    #[error("kind(s) classified in BOTH partitions: {0:?}")]
    Overlap(Vec<MissionArtifactKind>),
    /// A kind appears in neither partition.
    #[error("kind(s) classified in NEITHER partition: {0:?}")]
    Uncovered(Vec<MissionArtifactKind>),
    /// A kind appears twice inside one partition.
    #[error("kind(s) duplicated inside one partition: {0:?}")]
    Duplicate(Vec<MissionArtifactKind>),
}

/// Guard P-1: the partitions are disjoint and jointly cover every kind.
///
/// This mirrors upstream `assert_partition_invariant` but returns a typed
/// error instead of raising, so a construction path can fail closed without
/// panicking. It is cheap set arithmetic; call it at plan construction.
pub fn assert_partition_invariant() -> Result<(), PartitionViolation> {
    let primary = MissionArtifactKind::primary_kinds();
    let coord = MissionArtifactKind::coord_kinds();

    let primary_set: BTreeSet<_> = primary.iter().copied().collect();
    if primary_set.len() != primary.len() {
        let mut seen = BTreeSet::new();
        let duplicates: Vec<_> = primary
            .iter()
            .copied()
            .filter(|kind| !seen.insert(*kind))
            .collect();
        return Err(PartitionViolation::Duplicate(duplicates));
    }
    let coord_set: BTreeSet<_> = coord.iter().copied().collect();
    if coord_set.len() != coord.len() {
        let mut seen = BTreeSet::new();
        let duplicates: Vec<_> = coord
            .iter()
            .copied()
            .filter(|kind| !seen.insert(*kind))
            .collect();
        return Err(PartitionViolation::Duplicate(duplicates));
    }

    let overlap: Vec<_> = primary_set.intersection(&coord_set).copied().collect();
    if !overlap.is_empty() {
        return Err(PartitionViolation::Overlap(overlap));
    }
    let all: BTreeSet<_> = MissionArtifactKind::ALL.into_iter().collect();
    let covered: BTreeSet<_> = primary_set.union(&coord_set).copied().collect();
    if covered != all {
        return Err(PartitionViolation::Uncovered(
            all.difference(&covered).copied().collect(),
        ));
    }
    if covered.len() != MissionArtifactKind::ALL.len() {
        // Covered equals ALL as a set, so this can only differ if ALL itself
        // repeats a member.
        let mut seen = BTreeSet::new();
        let duplicates: Vec<_> = MissionArtifactKind::ALL
            .into_iter()
            .filter(|kind| !seen.insert(*kind))
            .collect();
        if !duplicates.is_empty() {
            return Err(PartitionViolation::Duplicate(duplicates));
        }
    }
    Ok(())
}

/// The single basename → kind classifier table.
///
/// Every entry is a *classification*: it says "this basename maps to this
/// kind", never "this kind is coordination residue". The partition question is
/// answered separately by the kind's partition.
pub const BASENAME_KIND: &[(&str, MissionArtifactKind)] = &[
    ("plan.md", MissionArtifactKind::FinalizedExecutionPlan),
    ("tasks.md", MissionArtifactKind::TasksIndex),
    ("wps.yaml", MissionArtifactKind::TasksIndex),
    ("lanes.json", MissionArtifactKind::LaneState),
    (
        "acceptance-matrix.json",
        MissionArtifactKind::AcceptanceMatrix,
    ),
    ("issue-matrix.md", MissionArtifactKind::IssueMatrix),
    ("issue-matrix.json", MissionArtifactKind::IssueMatrix),
    ("status.events.jsonl", MissionArtifactKind::StatusState),
    ("status.json", MissionArtifactKind::StatusState),
    ("analysis-report.md", MissionArtifactKind::AnalysisReport),
    ("spec.md", MissionArtifactKind::Spec),
    ("data-model.md", MissionArtifactKind::DataModel),
    ("research.md", MissionArtifactKind::Research),
    ("quickstart.md", MissionArtifactKind::Checklist),
    ("retrospective.yaml", MissionArtifactKind::Retrospective),
    ("baseline-tests.json", MissionArtifactKind::WorkPackageTask),
    ("decisions.events.jsonl", MissionArtifactKind::DecisionLog),
    ("findings.md", MissionArtifactKind::ResearchOutput),
    ("report.md", MissionArtifactKind::ResearchOutput),
    ("methodology.md", MissionArtifactKind::ResearchOutput),
    ("synthesis.md", MissionArtifactKind::ResearchOutput),
    ("literature-review.md", MissionArtifactKind::ResearchOutput),
    ("gap-analysis.md", MissionArtifactKind::AnalysisReport),
    ("audit-report.md", MissionArtifactKind::AnalysisReport),
    ("occurrence_map.yaml", MissionArtifactKind::OccurrenceMap),
    ("goals.md", MissionArtifactKind::MissionGoals),
    ("release.md", MissionArtifactKind::ReleaseRecord),
    ("meta.json", MissionArtifactKind::PrimaryMetadata),
];

/// The directory → kind table (first mission-relative component only).
pub const DIRECTORY_KIND: &[(&str, MissionArtifactKind)] = &[
    ("tasks", MissionArtifactKind::WorkPackageTask),
    ("checklists", MissionArtifactKind::Checklist),
    ("traces", MissionArtifactKind::TracerFile),
    ("decisions", MissionArtifactKind::DecisionLedger),
    ("contracts", MissionArtifactKind::Contract),
    ("research", MissionArtifactKind::Research),
];

/// Filename-anchored review-cycle glob (upstream `review-cycle-*.md`).
pub const REVIEW_CYCLE_FILENAME_GLOB: &str = "review-cycle-*.md";

/// Look up a bare mission-file basename.
pub fn kind_for_basename(name: &str) -> Option<MissionArtifactKind> {
    BASENAME_KIND
        .iter()
        .find(|(candidate, _)| *candidate == name)
        .map(|(_, kind)| *kind)
}

/// Look up a mission-relative first component.
pub fn kind_for_directory(name: &str) -> Option<MissionArtifactKind> {
    DIRECTORY_KIND
        .iter()
        .find(|(candidate, _)| *candidate == name)
        .map(|(_, kind)| *kind)
}

/// Normalise a path to POSIX separators without resolving it.
///
/// Backslashes are treated as separators only on Windows, matching the
/// upstream `to_posix` contract. Trailing separators are removed (a bare `/`
/// stays `/`).
pub fn normalize_posix(path: &str) -> String {
    let mut normalized = if cfg!(windows) {
        path.replace('\\', "/")
    } else {
        path.to_owned()
    };
    while normalized.len() > 1 && normalized.ends_with('/') {
        normalized.pop();
    }
    normalized
}

/// Classify a `kitty-specs/<slug>/...` path to its artifact kind.
///
/// Returns `None` for an unrecognised path, a path outside a `kitty-specs/`
/// directory, a bare mission directory, or another mission's artifact when
/// `mission_slug` is supplied. Classification is by basename where the file
/// carries a known name, then by directory for the remaining mission-relative
/// shapes. It never guesses from an arbitrary 8-character tail.
pub fn kind_for_mission_file(
    path: &str,
    mission_slug: Option<&str>,
) -> Option<MissionArtifactKind> {
    let normalized = normalize_posix(path);
    let parts: Vec<&str> = normalized
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    let specs_index = parts.iter().position(|part| *part == SPECS_DIR_NAME)?;
    let mission_index = specs_index + 1;
    let rel_index = mission_index + 1;
    if rel_index >= parts.len() {
        return None;
    }
    let path_mission = parts[mission_index];
    if let Some(expected) = mission_slug
        && path_mission != expected
    {
        return None;
    }
    let mission_rel = &parts[rel_index..];
    if mission_rel.len() == 1 {
        let name = mission_rel[0];
        return kind_for_basename(name).or_else(|| kind_for_directory(name));
    }
    if mission_rel[0] == "tasks"
        && glob_matches(
            mission_rel[mission_rel.len() - 1],
            REVIEW_CYCLE_FILENAME_GLOB,
        )
    {
        return Some(MissionArtifactKind::ReviewCycle);
    }
    kind_for_directory(mission_rel[0])
}

/// Minimal single-segment shell glob (`*`, `?`, literals).
fn glob_matches(value: &str, pattern: &str) -> bool {
    let value = value.as_bytes();
    let pattern = pattern.as_bytes();
    fn walk(value: &[u8], pattern: &[u8]) -> bool {
        match pattern.split_first() {
            None => value.is_empty(),
            Some((b'*', rest)) => {
                walk(value, rest) || (!value.is_empty() && walk(&value[1..], pattern))
            }
            Some((b'?', rest)) => !value.is_empty() && walk(&value[1..], rest),
            Some((byte, rest)) => value.first() == Some(byte) && walk(&value[1..], rest),
        }
    }
    walk(value, pattern)
}
