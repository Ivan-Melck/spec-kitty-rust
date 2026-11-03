//! Lane planning and the `lanes.json` manifest.
//!
//! # Planning
//!
//! Work packages collapse into one execution lane when their declared write
//! scopes overlap, or when they share a predicted surface and their ownership
//! is not provably disjoint. Dependency edges do **not** collapse lanes: they
//! become lane-level dependencies so disjoint upstream workstreams can run in
//! parallel.
//!
//! Planning-artifact work packages always live in the canonical
//! [`PLANNING_LANE_ID`] lane, which resolves to the repository-root checkout
//! and never to a worktree. A `single_branch` mission puts *every* work package
//! in that one lane.
//!
//! Lane identity is bound to its branch at creation, so a re-finalize reads a
//! surviving lane's id back from the previous manifest instead of re-lettering
//! it positionally onto a different work package's branch. Started work
//! packages (frozen membership) pin their recorded lane before any read-back,
//! and a binding that cannot be honoured refuses.
//!
//! # Persistence
//!
//! Reads are fail-closed: a corrupt manifest is an error, never "no lanes".
//! Writes take the **reference's** cross-process lock
//! ([`lanes_json_lock_path`]) and replace the file atomically, so a reader
//! observes a complete old or complete new document, and a native writer and an
//! upstream writer on the same manifest contend on the same lock file.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sk_kernel::{FileLock, MissionSlug, WpId};
use sk_mission::MissionTopology;

use crate::error::WorkspaceError;
use crate::naming::{PLANNING_LANE_ID, mission_branch_name};

/// The lane manifest filename.
pub const LANES_FILE_NAME: &str = "lanes.json";

/// Directory (under the Git common dir) holding the canonical `lanes.json`
/// locks, exactly as `specify_cli.lanes.persistence.lanes_json_lock` places
/// them.
pub const LANES_LOCK_DIRECTORY: &str = "spec-kitty-lanes-locks";

/// Largest manifest this crate will read.
pub const MAX_LANES_BYTES: u64 = 8 * 1024 * 1024;

/// How long a manifest writer waits for the cross-process lock.
///
/// Kept for callers that build their own [`FileLock`]; the store itself waits
/// **without** a deadline, exactly like the reference's
/// `machine_file_lock(blocking=True, timeout_s=None, reentrant=True)`.
pub const LANES_LOCK_TIMEOUT: Duration = Duration::from_secs(30);

/// The union-find merge rule labels recorded in a collapse report.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollapseRule {
    /// The work packages own overlapping files.
    WriteScopeOverlap,
    /// The work packages predict the same surface and ownership is not disjoint.
    SurfaceHeuristic,
    /// The work packages already shared a lane when work started.
    FrozenLaneMembership,
}

impl CollapseRule {
    /// The wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WriteScopeOverlap => "write_scope_overlap",
            Self::SurfaceHeuristic => "surface_heuristic",
            Self::FrozenLaneMembership => "frozen_lane_membership",
        }
    }
}

/// One union-find merge event.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CollapseEvent {
    /// First work package.
    pub wp_a: WpId,
    /// Second work package.
    pub wp_b: WpId,
    /// Why the merge happened.
    pub rule: CollapseRule,
    /// Human-readable evidence.
    pub evidence: String,
}

/// Summary of every collapse event during planning.
///
/// The wire form matches upstream: `events`, `total_merges`,
/// `independent_wps_collapsed`, and `by_rule` are all written, while only the
/// two stored fields are read back.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CollapseReport {
    /// Every merge event, in discovery order.
    pub events: Vec<CollapseEvent>,
    /// Merge events whose members have no direct or transitive dependency.
    pub independent_wps_collapsed: usize,
}

#[derive(Serialize)]
struct CollapseReportWire<'a> {
    events: &'a [CollapseEvent],
    total_merges: usize,
    independent_wps_collapsed: usize,
    by_rule: BTreeMap<&'static str, usize>,
}

#[derive(Deserialize)]
struct CollapseReportRead {
    #[serde(default)]
    events: Vec<CollapseEvent>,
    #[serde(default)]
    independent_wps_collapsed: usize,
}

impl Serialize for CollapseReport {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut by_rule: BTreeMap<&'static str, usize> = BTreeMap::new();
        for event in &self.events {
            *by_rule.entry(event.rule.as_str()).or_default() += 1;
        }
        CollapseReportWire {
            events: &self.events,
            total_merges: self.events.len(),
            independent_wps_collapsed: self.independent_wps_collapsed,
            by_rule,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for CollapseReport {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = CollapseReportRead::deserialize(deserializer)?;
        Ok(Self {
            events: wire.events,
            independent_wps_collapsed: wire.independent_wps_collapsed,
        })
    }
}

/// A group of work packages sharing one worktree and branch.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExecutionLane {
    /// Identifier such as `lane-a` or `lane-planning`.
    pub lane_id: String,
    /// Work packages, ordered by execution sequence within the lane.
    pub wp_ids: Vec<WpId>,
    /// Union of the owning work packages' `owned_files` globs.
    #[serde(default)]
    pub write_scope: Vec<String>,
    /// Surface tags predicted for this lane.
    #[serde(default)]
    pub predicted_surfaces: Vec<String>,
    /// Lanes that must complete before this one starts.
    #[serde(default)]
    pub depends_on_lanes: Vec<String>,
    /// Lanes with the same group can run in parallel.
    #[serde(default)]
    pub parallel_group: u32,
}

impl ExecutionLane {
    /// Is this the canonical repository-root planning lane?
    pub fn is_planning(&self) -> bool {
        self.lane_id == PLANNING_LANE_ID
    }

    /// Does this lane hold `wp`?
    pub fn contains(&self, wp: &WpId) -> bool {
        self.wp_ids.iter().any(|candidate| candidate == wp)
    }
}

/// The complete lane assignment persisted as `lanes.json`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LanesManifest {
    /// Schema version (currently 1).
    pub version: u32,
    /// Mission slug used in branch and worktree names.
    pub mission_slug: String,
    /// Canonical ULID, or `None` for a legacy mission.
    #[serde(default)]
    pub mission_id: Option<String>,
    /// The mission integration branch.
    pub mission_branch: String,
    /// The branch the mission merges into.
    pub target_branch: String,
    /// Lanes, ordered by `(parallel_group, lane_id)`.
    pub lanes: Vec<ExecutionLane>,
    /// ISO-8601 planning instant.
    pub computed_at: String,
    /// Description of the inputs used.
    pub computed_from: String,
    /// Derived view: work packages whose execution mode is planning-artifact.
    #[serde(default)]
    pub planning_artifact_wps: Vec<WpId>,
    /// The recorded finalize-tasks planning commit on `target_branch`.
    ///
    /// Upstream always writes this key (as `null` when absent), so the field is
    /// not skipped on serialization.
    #[serde(default)]
    pub planning_commit_sha: Option<String>,
    /// Collapse evidence, when any merge happened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collapse_report: Option<CollapseReport>,
}

impl LanesManifest {
    /// The schema version this crate writes.
    pub const VERSION: u32 = 1;

    /// The lane holding `wp`, if any.
    pub fn lane_for_wp(&self, wp: &WpId) -> Option<&ExecutionLane> {
        self.lanes.iter().find(|lane| lane.contains(wp))
    }

    /// Does the manifest contain at least one non-repo-root (code) lane?
    pub fn has_code_lanes(&self) -> bool {
        self.lanes.iter().any(|lane| !lane.is_planning())
    }

    /// Are all lanes the repository-root planning lane?
    pub fn is_planning_artifact_only(&self) -> bool {
        !self.lanes.is_empty() && self.lanes.iter().all(ExecutionLane::is_planning)
    }

    /// A copy with `planning_commit_sha` set to `commit`.
    ///
    /// The pin is the only durable link between a lane cut from the
    /// coordination branch and the planning lineage; it is written by the
    /// planning-commit owner after the commit lands, never predicted.
    pub fn with_planning_commit(&self, commit: &sk_git::Oid) -> Self {
        let mut manifest = self.clone();
        manifest.planning_commit_sha = Some(commit.to_string());
        manifest
    }

    /// Lanes grouped by parallel group, deterministic by group then lane id.
    pub fn parallel_groups(&self) -> BTreeMap<u32, Vec<&ExecutionLane>> {
        let mut groups: BTreeMap<u32, Vec<&ExecutionLane>> = BTreeMap::new();
        for lane in &self.lanes {
            groups.entry(lane.parallel_group).or_default().push(lane);
        }
        groups
    }
}

/// The declared write scope and execution mode of one work package.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WpPlan {
    /// The work package id.
    pub wp_id: WpId,
    /// Declared `owned_files` globs.
    pub owned_files: Vec<String>,
    /// `execution_mode == planning_artifact`.
    pub planning_artifact: bool,
}

impl WpPlan {
    /// Construct a code-change plan.
    pub fn code(wp_id: WpId, owned_files: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            wp_id,
            owned_files: owned_files.into_iter().map(Into::into).collect(),
            planning_artifact: false,
        }
    }

    /// Construct a planning-artifact plan.
    pub fn planning(wp_id: WpId, owned_files: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            wp_id,
            owned_files: owned_files.into_iter().map(Into::into).collect(),
            planning_artifact: true,
        }
    }
}

/// Everything lane planning needs, with no filesystem or Git access.
#[derive(Clone, Debug)]
pub struct LanePlannerInput {
    /// Mission slug.
    pub mission_slug: MissionSlug,
    /// Declared mission id, if any.
    pub mission_id: Option<sk_mission::MissionId>,
    /// The stored topology. It selects the `single_branch` short-circuit.
    pub topology: MissionTopology,
    /// The branch the mission merges into.
    pub target_branch: String,
    /// The already-resolved `meta.json` `mission_branch` (protected-target
    /// missions only; consulted for `single_branch`).
    pub mission_branch: Option<String>,
    /// Work package id → dependencies.
    pub dependencies: BTreeMap<WpId, Vec<WpId>>,
    /// Work package id → declared plan.
    pub work_packages: BTreeMap<WpId, WpPlan>,
    /// Optional prompt bodies, used for surface prediction.
    pub bodies: BTreeMap<WpId, String>,
    /// Prior manifest, used to read lane ids back.
    pub previous: Option<LanesManifest>,
    /// Started work packages and their recorded lane ids (#5573).
    pub frozen_lane_bindings: BTreeMap<WpId, String>,
}

impl LanePlannerInput {
    /// A planner input with no previous manifest, bodies, or frozen bindings.
    pub fn new(
        mission_slug: MissionSlug,
        topology: MissionTopology,
        target_branch: impl Into<String>,
    ) -> Self {
        Self {
            mission_slug,
            mission_id: None,
            topology,
            target_branch: target_branch.into(),
            mission_branch: None,
            dependencies: BTreeMap::new(),
            work_packages: BTreeMap::new(),
            bodies: BTreeMap::new(),
            previous: None,
            frozen_lane_bindings: BTreeMap::new(),
        }
    }

    /// Add a work package plan.
    pub fn with_work_package(mut self, plan: WpPlan) -> Self {
        self.dependencies.entry(plan.wp_id.clone()).or_default();
        self.work_packages.insert(plan.wp_id.clone(), plan);
        self
    }

    /// Declare dependencies for a work package.
    pub fn with_dependencies(mut self, wp: WpId, dependencies: Vec<WpId>) -> Self {
        self.dependencies.insert(wp, dependencies);
        self
    }

    /// Attach a previous manifest.
    pub fn with_previous(mut self, previous: LanesManifest) -> Self {
        self.previous = Some(previous);
        self
    }
}

/// Every work package the planner must account for: the union of declared
/// dependency-graph keys and planned work packages.
fn all_wp_ids(input: &LanePlannerInput) -> Vec<WpId> {
    input
        .dependencies
        .keys()
        .chain(input.work_packages.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Compute the lane assignment.
///
/// See the module documentation for the collapse rules and identity rules.
pub fn compute_lanes(input: &LanePlannerInput) -> Result<LanesManifest, WorkspaceError> {
    let slug = input.mission_slug.as_str();
    let mission_branch = match input.topology {
        MissionTopology::SingleBranch => input
            .mission_branch
            .clone()
            .unwrap_or_else(|| input.target_branch.clone()),
        _ => input
            .previous
            .as_ref()
            .map(|previous| previous.mission_branch.clone())
            .filter(|branch| !branch.is_empty())
            .unwrap_or_else(|| mission_branch_name(slug, input.mission_id.as_ref())),
    };

    validate_frozen_bindings(input)?;
    if let Some(previous) = &input.previous {
        crate::allocation::guard_topology_matches_manifest(Some(input.topology), previous)?;
    }
    if input.topology == MissionTopology::SingleBranch
        && input
            .frozen_lane_bindings
            .values()
            .any(|lane| lane != PLANNING_LANE_ID)
    {
        return Err(WorkspaceError::LanePlanning {
            message: "cannot move a started code-lane WP into lane-planning".to_owned(),
        });
    }

    let all_wp_ids = all_wp_ids(input);
    if all_wp_ids.is_empty() {
        return Ok(empty_manifest(input, mission_branch));
    }

    if input.topology == MissionTopology::SingleBranch {
        return single_branch_manifest(input, mission_branch);
    }

    let mut planning_wps = Vec::new();
    let mut code_wps = Vec::new();
    for wp in &all_wp_ids {
        match input.work_packages.get(wp) {
            Some(plan) if plan.planning_artifact => planning_wps.push(wp.clone()),
            Some(_) => code_wps.push(wp.clone()),
            None => {
                return Err(WorkspaceError::LanePlanning {
                    message: format!(
                        "executable WP {wp} has no ownership manifest; set owned_files and execution_mode in frontmatter, or run finalize-tasks"
                    ),
                });
            }
        }
    }

    if code_wps.is_empty() {
        let planning_lane = planning_lane(&planning_wps, &input.work_packages, Vec::new(), 0);
        return Ok(LanesManifest {
            version: LanesManifest::VERSION,
            mission_slug: slug.to_owned(),
            mission_id: input.mission_id.as_ref().map(ToString::to_string),
            mission_branch,
            target_branch: input.target_branch.clone(),
            planning_artifact_wps: planning_wps.clone(),
            lanes: vec![planning_lane],
            computed_at: sk_kernel::clock::now_utc_iso(),
            computed_from: "dependency_graph+ownership".to_owned(),
            planning_commit_sha: None,
            collapse_report: None,
        });
    }

    let mut union = UnionFind::new(code_wps.iter().cloned());
    let mut collapse_events = Vec::new();

    let code_plans: BTreeMap<WpId, &WpPlan> = code_wps
        .iter()
        .filter_map(|wp| input.work_packages.get(wp).map(|plan| (wp.clone(), plan)))
        .collect();

    for (wp_a, wp_b) in overlap_pairs(&code_plans) {
        if union.find(&wp_a) != union.find(&wp_b) {
            let evidence = overlap_evidence(&code_plans, &wp_a, &wp_b);
            collapse_events.push(CollapseEvent {
                wp_a: wp_a.clone(),
                wp_b: wp_b.clone(),
                rule: CollapseRule::WriteScopeOverlap,
                evidence,
            });
        }
        union.union(&wp_a, &wp_b);
    }

    // Surface prediction: a shared keyword is not enough when ownership is
    // provably disjoint.
    for (index, wp_a) in code_wps.iter().enumerate() {
        for wp_b in code_wps.iter().skip(index + 1) {
            let surfaces_a =
                infer_surfaces(input.bodies.get(wp_a).map(String::as_str).unwrap_or(""));
            let surfaces_b =
                infer_surfaces(input.bodies.get(wp_b).map(String::as_str).unwrap_or(""));
            let shared: BTreeSet<&str> = surfaces_a.intersection(&surfaces_b).copied().collect();
            if shared.is_empty() {
                continue;
            }
            let disjoint = match (code_plans.get(wp_a), code_plans.get(wp_b)) {
                (Some(plan_a), Some(plan_b)) => {
                    !globs_overlap_any(&plan_a.owned_files, &plan_b.owned_files)
                        && !plan_a.owned_files.is_empty()
                        && !plan_b.owned_files.is_empty()
                }
                _ => false,
            };
            if disjoint {
                continue;
            }
            if union.find(wp_a) != union.find(wp_b) {
                collapse_events.push(CollapseEvent {
                    wp_a: wp_a.clone(),
                    wp_b: wp_b.clone(),
                    rule: CollapseRule::SurfaceHeuristic,
                    evidence: format!("shared surfaces {shared:?} with non-disjoint ownership"),
                });
            }
            union.union(wp_a, wp_b);
        }
    }

    // Started lane-mates stay together (#5573).
    let mut by_lane: BTreeMap<String, Vec<WpId>> = BTreeMap::new();
    for wp in &code_wps {
        if let Some(lane_id) = input.frozen_lane_bindings.get(wp)
            && lane_id != PLANNING_LANE_ID
        {
            by_lane.entry(lane_id.clone()).or_default().push(wp.clone());
        }
    }
    for (lane_id, members) in by_lane {
        if members.len() < 2 {
            continue;
        }
        let anchor = members[0].clone();
        for other in &members[1..] {
            if union.find(&anchor) != union.find(other) {
                collapse_events.push(CollapseEvent {
                    wp_a: anchor.clone(),
                    wp_b: other.clone(),
                    rule: CollapseRule::FrozenLaneMembership,
                    evidence: format!("both started on {lane_id}"),
                });
            }
            union.union(&anchor, other);
        }
    }

    let mut groups = union.groups();
    groups.sort_by(|left, right| {
        left.iter()
            .min()
            .cmp(&right.iter().min())
            .then_with(|| left.cmp(right))
    });

    let lane_ids = assign_lane_ids(
        &groups,
        input.previous.as_ref(),
        &input.frozen_lane_bindings,
    )?;

    let mut lanes = Vec::with_capacity(groups.len());
    for (group, lane_id) in groups.iter().zip(lane_ids) {
        let group_set: BTreeSet<&WpId> = group.iter().collect();
        let sub_graph: BTreeMap<WpId, Vec<WpId>> = group
            .iter()
            .map(|wp| {
                let deps = input
                    .dependencies
                    .get(wp)
                    .map(|deps| {
                        deps.iter()
                            .filter(|dep| group_set.contains(dep))
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();
                (wp.clone(), deps)
            })
            .collect();
        let ordered = topological_sort(&sub_graph)?;

        let mut write_scope = BTreeSet::new();
        let mut surfaces = BTreeSet::new();
        for wp in &ordered {
            if let Some(plan) = input.work_packages.get(wp) {
                write_scope.extend(plan.owned_files.iter().cloned());
            }
            if let Some(body) = input.bodies.get(wp) {
                surfaces.extend(infer_surfaces(body).into_iter().map(str::to_owned));
            }
        }
        lanes.push(ExecutionLane {
            lane_id,
            wp_ids: ordered,
            write_scope: write_scope.into_iter().collect(),
            predicted_surfaces: surfaces.into_iter().collect(),
            depends_on_lanes: Vec::new(),
            parallel_group: 0,
        });
    }

    // Lane-level dependencies, including cross-mode planning edges.
    let mut wp_to_lane: BTreeMap<WpId, String> = BTreeMap::new();
    for lane in &lanes {
        for wp in &lane.wp_ids {
            wp_to_lane.insert(wp.clone(), lane.lane_id.clone());
        }
    }
    for wp in &planning_wps {
        wp_to_lane.insert(wp.clone(), PLANNING_LANE_ID.to_owned());
    }
    let mut lane_deps: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for lane in &lanes {
        lane_deps.entry(lane.lane_id.clone()).or_default();
    }
    if !planning_wps.is_empty() {
        lane_deps.entry(PLANNING_LANE_ID.to_owned()).or_default();
    }
    for wp in code_wps.iter().chain(planning_wps.iter()) {
        let Some(my_lane) = wp_to_lane.get(wp) else {
            continue;
        };
        for dep in input.dependencies.get(wp).into_iter().flatten() {
            let Some(dep_lane) = wp_to_lane.get(dep) else {
                continue;
            };
            if dep_lane != my_lane {
                lane_deps
                    .entry(my_lane.clone())
                    .or_default()
                    .insert(dep_lane.clone());
            }
        }
    }
    if let Some(cycle) = find_cycle(&lane_deps) {
        return Err(WorkspaceError::LaneDependencyCycle { path: cycle });
    }
    let depths = lane_depths(&lane_deps);
    for lane in &mut lanes {
        lane.depends_on_lanes = lane_deps
            .get(&lane.lane_id)
            .map(|deps| deps.iter().cloned().collect())
            .unwrap_or_default();
        lane.parallel_group = depths.get(&lane.lane_id).copied().unwrap_or(0);
    }

    let all_lanes = if planning_wps.is_empty() {
        lanes
    } else {
        let planning = planning_lane(
            &planning_wps,
            &input.work_packages,
            lane_deps
                .get(PLANNING_LANE_ID)
                .map(|deps| deps.iter().cloned().collect())
                .unwrap_or_default(),
            depths.get(PLANNING_LANE_ID).copied().unwrap_or(0),
        );
        let mut all = lanes;
        all.push(planning);
        all
    };
    let mut all_lanes = all_lanes;
    all_lanes.sort_by(|left, right| {
        left.parallel_group
            .cmp(&right.parallel_group)
            .then_with(|| left.lane_id.cmp(&right.lane_id))
    });

    let independent = count_independent_collapses(&collapse_events, &input.dependencies);
    let collapse_report = (!collapse_events.is_empty()).then_some(CollapseReport {
        events: collapse_events,
        independent_wps_collapsed: independent,
    });

    Ok(LanesManifest {
        version: LanesManifest::VERSION,
        mission_slug: slug.to_owned(),
        mission_id: input.mission_id.as_ref().map(ToString::to_string),
        mission_branch,
        target_branch: input.target_branch.clone(),
        lanes: all_lanes,
        computed_at: sk_kernel::clock::now_utc_iso(),
        computed_from: "dependency_graph+ownership".to_owned(),
        planning_artifact_wps: planning_wps,
        planning_commit_sha: None,
        collapse_report,
    })
}

fn empty_manifest(input: &LanePlannerInput, mission_branch: String) -> LanesManifest {
    LanesManifest {
        version: LanesManifest::VERSION,
        mission_slug: input.mission_slug.as_str().to_owned(),
        mission_id: input.mission_id.as_ref().map(ToString::to_string),
        mission_branch,
        target_branch: input.target_branch.clone(),
        lanes: Vec::new(),
        computed_at: sk_kernel::clock::now_utc_iso(),
        computed_from: "dependency_graph+ownership".to_owned(),
        planning_artifact_wps: Vec::new(),
        planning_commit_sha: None,
        collapse_report: None,
    }
}

fn single_branch_manifest(
    input: &LanePlannerInput,
    mission_branch: String,
) -> Result<LanesManifest, WorkspaceError> {
    let ids = all_wp_ids(input);
    let graph: BTreeMap<WpId, Vec<WpId>> = ids
        .iter()
        .map(|wp| {
            (
                wp.clone(),
                input.dependencies.get(wp).cloned().unwrap_or_default(),
            )
        })
        .collect();
    let ordered = topological_sort(&graph)?;
    let mut write_scope = BTreeSet::new();
    let mut planning = Vec::new();
    for wp in &ordered {
        let Some(plan) = input.work_packages.get(wp) else {
            return Err(WorkspaceError::LanePlanning {
                message: format!(
                    "executable WP {wp} has no ownership manifest; set owned_files and execution_mode in frontmatter, or run finalize-tasks"
                ),
            });
        };
        if plan.planning_artifact {
            planning.push(wp.clone());
        }
        write_scope.extend(plan.owned_files.iter().cloned());
    }
    Ok(LanesManifest {
        version: LanesManifest::VERSION,
        mission_slug: input.mission_slug.as_str().to_owned(),
        mission_id: input.mission_id.as_ref().map(ToString::to_string),
        mission_branch,
        target_branch: input.target_branch.clone(),
        lanes: vec![ExecutionLane {
            lane_id: PLANNING_LANE_ID.to_owned(),
            wp_ids: ordered,
            write_scope: write_scope.into_iter().collect(),
            predicted_surfaces: Vec::new(),
            depends_on_lanes: Vec::new(),
            parallel_group: 0,
        }],
        computed_at: sk_kernel::clock::now_utc_iso(),
        computed_from: "dependency_graph+ownership".to_owned(),
        planning_artifact_wps: planning,
        planning_commit_sha: None,
        collapse_report: None,
    })
}

fn planning_lane(
    planning_wps: &[WpId],
    plans: &BTreeMap<WpId, WpPlan>,
    depends_on_lanes: Vec<String>,
    parallel_group: u32,
) -> ExecutionLane {
    let mut write_scope = BTreeSet::new();
    for wp in planning_wps {
        if let Some(plan) = plans.get(wp) {
            write_scope.extend(plan.owned_files.iter().cloned());
        }
    }
    let mut sorted = planning_wps.to_vec();
    sorted.sort();
    ExecutionLane {
        lane_id: PLANNING_LANE_ID.to_owned(),
        wp_ids: sorted,
        write_scope: write_scope.into_iter().collect(),
        predicted_surfaces: vec!["planning".to_owned()],
        depends_on_lanes,
        parallel_group,
    }
}

fn validate_frozen_bindings(input: &LanePlannerInput) -> Result<(), WorkspaceError> {
    for (wp, lane_id) in &input.frozen_lane_bindings {
        let Some(plan) = input.work_packages.get(wp) else {
            return Err(WorkspaceError::LanePlanning {
                message: format!(
                    "cannot re-finalize: started work package {wp} is missing from the manifest inputs (started_wp_removed)"
                ),
            });
        };
        if (lane_id == PLANNING_LANE_ID) != plan.planning_artifact {
            return Err(WorkspaceError::LanePlanning {
                message: format!(
                    "cannot re-finalize: started work package {wp} changed kind relative to its recorded lane {lane_id} (started_wp_kind_changed)"
                ),
            });
        }
    }
    Ok(())
}

fn assign_lane_ids(
    groups: &[Vec<WpId>],
    previous: Option<&LanesManifest>,
    frozen: &BTreeMap<WpId, String>,
) -> Result<Vec<String>, WorkspaceError> {
    let mut assigned: BTreeMap<usize, String> = BTreeMap::new();
    let mut used: BTreeSet<String> = BTreeSet::new();

    // Pass 0: started work pins its recorded lane id.
    for (index, group) in groups.iter().enumerate() {
        let pins: BTreeSet<&String> = group
            .iter()
            .filter_map(|wp| frozen.get(wp))
            .filter(|lane_id| lane_id.as_str() != PLANNING_LANE_ID)
            .collect();
        match pins.len() {
            0 => {}
            1 => {
                let pin = pins.into_iter().next().expect("one pin").clone();
                used.insert(pin.clone());
                assigned.insert(index, pin);
            }
            _ => {
                return Err(WorkspaceError::LanePlanning {
                    message: format!(
                        "cannot re-finalize: started work packages from lanes {pins:?} would collapse into one lane (started_lanes_collapsed)"
                    ),
                });
            }
        }
    }

    // Pass 1: read back the unused prior lane id with the largest overlap.
    let prior: BTreeMap<&str, BTreeSet<&WpId>> = previous
        .map(|manifest| {
            manifest
                .lanes
                .iter()
                .filter(|lane| !lane.is_planning())
                .map(|lane| (lane.lane_id.as_str(), lane.wp_ids.iter().collect()))
                .collect()
        })
        .unwrap_or_default();
    for (index, group) in groups.iter().enumerate() {
        if assigned.contains_key(&index) {
            continue;
        }
        let members: BTreeSet<&WpId> = group.iter().collect();
        let mut best: Option<(&str, usize)> = None;
        for (lane_id, prior_members) in &prior {
            if used.contains(*lane_id) {
                continue;
            }
            let overlap = members.intersection(prior_members).count();
            if overlap > best.map(|(_, count)| count).unwrap_or(0) {
                best = Some((lane_id, overlap));
            }
        }
        if let Some((lane_id, _)) = best {
            used.insert(lane_id.to_owned());
            assigned.insert(index, lane_id.to_owned());
        }
    }

    // Pass 2: mint fresh positional ids, never reusing a reserved one.
    let reserved: BTreeSet<&str> = frozen.values().map(String::as_str).collect();
    for index in 0..groups.len() {
        if assigned.contains_key(&index) {
            continue;
        }
        let mut letter = b'a';
        let fresh = loop {
            let candidate = format!("lane-{}", letter as char);
            if !used.contains(&candidate) && !reserved.contains(candidate.as_str()) {
                break candidate;
            }
            if letter == b'z' {
                return Err(WorkspaceError::LanePlanning {
                    message: "more than 26 code lanes; extend the lane-id grammar".to_owned(),
                });
            }
            letter += 1;
        };
        used.insert(fresh.clone());
        assigned.insert(index, fresh);
    }

    Ok((0..groups.len())
        .map(|index| assigned.get(&index).cloned().expect("every group assigned"))
        .collect())
}

fn overlap_pairs(plans: &BTreeMap<WpId, &WpPlan>) -> Vec<(WpId, WpId)> {
    let ids: Vec<&WpId> = plans.keys().collect();
    let mut pairs = Vec::new();
    for (index, left) in ids.iter().enumerate() {
        for right in ids.iter().skip(index + 1) {
            let a = plans.get(*left).expect("present");
            let b = plans.get(*right).expect("present");
            if globs_overlap_any(&a.owned_files, &b.owned_files) {
                pairs.push(((*left).clone(), (*right).clone()));
            }
        }
    }
    pairs
}

fn overlap_evidence(plans: &BTreeMap<WpId, &WpPlan>, a: &WpId, b: &WpId) -> String {
    let mut examples = Vec::new();
    if let (Some(plan_a), Some(plan_b)) = (plans.get(a), plans.get(b)) {
        for left in &plan_a.owned_files {
            for right in &plan_b.owned_files {
                if globs_overlap(left, right) {
                    examples.push(format!("{left:?} vs {right:?}"));
                }
            }
        }
    }
    if examples.is_empty() {
        "write-scope overlap".to_owned()
    } else {
        format!("overlapping globs: {}", examples.join(", "))
    }
}

/// Do any of the two glob lists overlap?
pub fn globs_overlap_any(left: &[String], right: &[String]) -> bool {
    left.iter()
        .any(|a| right.iter().any(|b| globs_overlap(a, b)))
}

/// Can two path globs (with `*`, `?`, and `**` segments) match the same path?
///
/// The answer is a conservative intersection test, not a regex expansion: it
/// returns `true` exactly when some concrete path matches both patterns.
pub fn globs_overlap(left: &str, right: &str) -> bool {
    let a: Vec<&str> = left.split('/').collect();
    let b: Vec<&str> = right.split('/').collect();
    segments_overlap(&a, &b)
}

fn segments_overlap(a: &[&str], b: &[&str]) -> bool {
    if a.is_empty() && b.is_empty() {
        return true;
    }
    if a.first() == Some(&"**") {
        if segments_overlap(&a[1..], b) {
            return true;
        }
        return !b.is_empty() && segments_overlap(a, &b[1..]);
    }
    if b.first() == Some(&"**") {
        if segments_overlap(a, &b[1..]) {
            return true;
        }
        return !a.is_empty() && segments_overlap(&a[1..], b);
    }
    if a.is_empty() || b.is_empty() {
        return false;
    }
    segment_overlap(a[0], b[0]) && segments_overlap(&a[1..], &b[1..])
}

fn segment_overlap(a: &str, b: &str) -> bool {
    fn walk(a: &[u8], b: &[u8]) -> bool {
        match (a.first(), b.first()) {
            (None, None) => true,
            (Some(b'*'), _) => walk(&a[1..], b) || (!b.is_empty() && walk(a, &b[1..])),
            (_, Some(b'*')) => walk(a, &b[1..]) || (!a.is_empty() && walk(&a[1..], b)),
            (Some(left), Some(right)) if left == right || *left == b'?' || *right == b'?' => {
                walk(&a[1..], &b[1..])
            }
            _ => false,
        }
    }
    walk(a.as_bytes(), b.as_bytes())
}

/// Predict surface tags from a work-package body.
pub fn infer_surfaces(body: &str) -> BTreeSet<&'static str> {
    const KEYWORDS: &[(&str, &[&str])] = &[
        ("dashboard", &["dashboard", "landing page", "landing-page"]),
        ("workspace", &["workspace", "mission workspace"]),
        (
            "app-shell",
            &["app shell", "app-shell", "navigation", "sidebar", "layout"],
        ),
        (
            "legacy-cleanup",
            &["legacy", "cleanup", "deprecat", "remov"],
        ),
        (
            "tests",
            &[
                "test suite",
                "test infrastructure",
                "e2e test",
                "integration test",
            ],
        ),
        ("tracker-integration", &["tracker", "saas", "sync"]),
        ("artifact-rendering", &["artifact", "render", "template"]),
        ("api", &["api", "endpoint", "route", "view"]),
    ];
    let lowered = body.to_lowercase();
    KEYWORDS
        .iter()
        .filter(|(_, needles)| needles.iter().any(|needle| lowered.contains(needle)))
        .map(|(surface, _)| *surface)
        .collect()
}

fn topological_sort(graph: &BTreeMap<WpId, Vec<WpId>>) -> Result<Vec<WpId>, WorkspaceError> {
    let mut in_degree: BTreeMap<WpId, usize> = graph.keys().map(|wp| (wp.clone(), 0)).collect();
    let mut dependents: BTreeMap<WpId, Vec<WpId>> = BTreeMap::new();
    for (wp, deps) in graph {
        for dep in deps {
            if in_degree.contains_key(dep) {
                *in_degree.entry(wp.clone()).or_insert(0) += 1;
                dependents.entry(dep.clone()).or_default().push(wp.clone());
            }
        }
    }
    let mut ready: BTreeSet<WpId> = in_degree
        .iter()
        .filter(|(_, degree)| **degree == 0)
        .map(|(wp, _)| wp.clone())
        .collect();
    let mut ordered = Vec::new();
    while let Some(next) = ready.iter().next().cloned() {
        ready.remove(&next);
        ordered.push(next.clone());
        if let Some(children) = dependents.get(&next) {
            for child in children {
                if let Some(degree) = in_degree.get_mut(child) {
                    *degree -= 1;
                    if *degree == 0 {
                        ready.insert(child.clone());
                    }
                }
            }
        }
    }
    if ordered.len() != graph.len() {
        let cycle: Vec<String> = in_degree
            .iter()
            .filter(|(_, degree)| **degree > 0)
            .map(|(wp, _)| wp.to_string())
            .collect();
        return Err(WorkspaceError::LaneDependencyCycle { path: cycle });
    }
    Ok(ordered)
}

fn find_cycle(deps: &BTreeMap<String, BTreeSet<String>>) -> Option<Vec<String>> {
    let mut state: BTreeMap<&str, u8> = BTreeMap::new();
    let nodes: BTreeSet<&str> = deps
        .keys()
        .map(String::as_str)
        .chain(deps.values().flatten().map(String::as_str))
        .collect();
    for root in nodes {
        if state.get(root).copied().unwrap_or(0) != 0 {
            continue;
        }
        let mut stack: Vec<(&str, Vec<&str>, usize)> = vec![(
            root,
            deps.get(root)
                .map(|set| set.iter().map(String::as_str).collect())
                .unwrap_or_default(),
            0,
        )];
        let mut active: Vec<&str> = vec![root];
        state.insert(root, 1);
        while let Some((node, edges, index)) = stack.pop() {
            if index >= edges.len() {
                state.insert(node, 2);
                active.pop();
                continue;
            }
            stack.push((node, edges.clone(), index + 1));
            let dep = edges[index];
            match state.get(dep).copied().unwrap_or(0) {
                0 => {
                    state.insert(dep, 1);
                    active.push(dep);
                    stack.push((
                        dep,
                        deps.get(dep)
                            .map(|set| set.iter().map(String::as_str).collect())
                            .unwrap_or_default(),
                        0,
                    ));
                }
                1 => {
                    let start = active.iter().position(|node| *node == dep).unwrap_or(0);
                    let mut cycle: Vec<String> = active[start..]
                        .iter()
                        .map(|node| (*node).to_owned())
                        .collect();
                    let smallest = cycle
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, lane)| *lane)
                        .map(|(index, _)| index)
                        .unwrap_or(0);
                    cycle.rotate_left(smallest);
                    cycle.push(cycle[0].clone());
                    return Some(cycle);
                }
                _ => {}
            }
        }
    }
    None
}

fn lane_depths(deps: &BTreeMap<String, BTreeSet<String>>) -> BTreeMap<String, u32> {
    let mut depths: BTreeMap<String, u32> = BTreeMap::new();
    let mut visiting = BTreeSet::new();
    fn depth(
        lane: &str,
        deps: &BTreeMap<String, BTreeSet<String>>,
        depths: &mut BTreeMap<String, u32>,
        visiting: &mut BTreeSet<String>,
    ) -> u32 {
        if let Some(value) = depths.get(lane) {
            return *value;
        }
        if !visiting.insert(lane.to_owned()) {
            return 0;
        }
        let mut value = 0;
        if let Some(edges) = deps.get(lane) {
            for dep in edges {
                if dep == lane {
                    continue;
                }
                value = value.max(depth(dep, deps, depths, visiting) + 1);
            }
        }
        visiting.remove(lane);
        depths.insert(lane.to_owned(), value);
        value
    }
    for lane in deps.keys() {
        depth(lane, deps, &mut depths, &mut visiting);
    }
    depths
}

fn count_independent_collapses(
    events: &[CollapseEvent],
    dependencies: &BTreeMap<WpId, Vec<WpId>>,
) -> usize {
    fn reach(
        wp: &WpId,
        dependencies: &BTreeMap<WpId, Vec<WpId>>,
        cache: &mut BTreeMap<WpId, BTreeSet<WpId>>,
        visiting: &mut BTreeSet<WpId>,
    ) -> BTreeSet<WpId> {
        if let Some(cached) = cache.get(wp) {
            return cached.clone();
        }
        if !visiting.insert(wp.clone()) {
            return BTreeSet::new();
        }
        let mut result = BTreeSet::new();
        for dep in dependencies.get(wp).into_iter().flatten() {
            result.insert(dep.clone());
            result.extend(reach(dep, dependencies, cache, visiting));
        }
        visiting.remove(wp);
        cache.insert(wp.clone(), result.clone());
        result
    }

    let mut cache = BTreeMap::new();
    let mut visiting = BTreeSet::new();
    let transitive: BTreeMap<WpId, BTreeSet<WpId>> = dependencies
        .keys()
        .map(|wp| {
            (
                wp.clone(),
                reach(wp, dependencies, &mut cache, &mut visiting),
            )
        })
        .collect();

    events
        .iter()
        .filter(|event| event.rule != CollapseRule::FrozenLaneMembership)
        .filter(|event| {
            !transitive
                .get(&event.wp_a)
                .is_some_and(|set| set.contains(&event.wp_b))
                && !transitive
                    .get(&event.wp_b)
                    .is_some_and(|set| set.contains(&event.wp_a))
        })
        .count()
}

struct UnionFind {
    parent: BTreeMap<WpId, WpId>,
    rank: BTreeMap<WpId, u32>,
}

impl UnionFind {
    fn new(elements: impl IntoIterator<Item = WpId>) -> Self {
        let elements: Vec<WpId> = elements.into_iter().collect();
        Self {
            parent: elements.iter().map(|wp| (wp.clone(), wp.clone())).collect(),
            rank: elements.into_iter().map(|wp| (wp, 0)).collect(),
        }
    }

    fn find(&mut self, wp: &WpId) -> WpId {
        let mut current = wp.clone();
        while let Some(parent) = self.parent.get(&current)
            && parent != &current
        {
            let grandparent = self
                .parent
                .get(parent)
                .cloned()
                .unwrap_or_else(|| parent.clone());
            self.parent.insert(current.clone(), grandparent.clone());
            current = grandparent;
        }
        current
    }

    fn union(&mut self, a: &WpId, b: &WpId) {
        let mut root_a = self.find(a);
        let mut root_b = self.find(b);
        if root_a == root_b {
            return;
        }
        if self.rank.get(&root_a).copied().unwrap_or(0)
            < self.rank.get(&root_b).copied().unwrap_or(0)
        {
            std::mem::swap(&mut root_a, &mut root_b);
        }
        self.parent.insert(root_b.clone(), root_a.clone());
        if self.rank.get(&root_a).copied().unwrap_or(0)
            == self.rank.get(&root_b).copied().unwrap_or(0)
        {
            *self.rank.entry(root_a).or_insert(0) += 1;
        }
    }

    fn groups(&mut self) -> Vec<Vec<WpId>> {
        let keys: Vec<WpId> = self.parent.keys().cloned().collect();
        let mut grouped: BTreeMap<WpId, Vec<WpId>> = BTreeMap::new();
        for wp in keys {
            let root = self.find(&wp);
            grouped.entry(root).or_default().push(wp);
        }
        grouped.into_values().collect()
    }
}

/// Whether a lane may start, given the lanes that have already completed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LaneReadiness {
    /// Every dependency lane is complete.
    Ready,
    /// One or more dependency lanes are not complete, or the lane is unknown.
    Blocked {
        /// The lane asked about, or `None` when it is absent from the manifest.
        lane_id: String,
        /// Dependency lanes that are not in the satisfied set.
        missing: Vec<String>,
    },
}

impl LaneReadiness {
    /// Is the lane runnable?
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }
}

/// Evaluate lane-level dependency readiness.
///
/// `satisfied` holds lane ids that have completed (typically lanes whose work
/// packages are all `done`). A dependency lane that is not in the set blocks
/// the lane; an unknown lane is blocked rather than treated as ready.
pub fn lane_dependency_readiness(
    manifest: &LanesManifest,
    lane_id: &str,
    satisfied: &BTreeSet<String>,
) -> LaneReadiness {
    let Some(lane) = manifest.lanes.iter().find(|lane| lane.lane_id == lane_id) else {
        return LaneReadiness::Blocked {
            lane_id: lane_id.to_owned(),
            missing: vec![lane_id.to_owned()],
        };
    };
    let missing: Vec<String> = lane
        .depends_on_lanes
        .iter()
        .filter(|dependency| !satisfied.contains(dependency.as_str()))
        .cloned()
        .collect();
    if missing.is_empty() {
        LaneReadiness::Ready
    } else {
        LaneReadiness::Blocked {
            lane_id: lane_id.to_owned(),
            missing,
        }
    }
}

/// Read/write access to one mission's `lanes.json`.
#[derive(Clone, Debug)]
pub struct LanesStore {
    feature_dir: PathBuf,
    lock: LockAuthority,
}

/// Where [`LanesStore`] takes its cross-process lock.
#[derive(Clone, Debug)]
enum LockAuthority {
    /// The reference's canonical ``<common>/spec-kitty-lanes-locks/<rel>.lock``
    /// path, resolved from the feature directory when the lock is taken.
    Canonical,
    /// An explicit path supplied by the caller.
    Explicit(PathBuf),
}

impl LanesStore {
    /// A store anchored on `feature_dir` that uses the **canonical** lock.
    ///
    /// The lock is resolved at lock time by [`lanes_json_lock_path`], never
    /// placed beside the manifest: an in-tree `<mission_dir>/.lanes.json.lock`
    /// shares no lock with the reference writer (so a mixed lost update is
    /// possible) and leaves an untracked file inside the mission directory, which
    /// then makes a later `fast_forward`/rollback refuse the checkout as dirty
    /// (`INTEGRATION_CHECKOUT_DIRTY`).
    pub fn new(feature_dir: impl Into<PathBuf>) -> Self {
        Self {
            feature_dir: feature_dir.into(),
            lock: LockAuthority::Canonical,
        }
    }

    /// A store with an explicit lock path.
    ///
    /// This bypasses the canonical authority and is intended for tests and
    /// embedded callers that already own a serialization point; production
    /// writers must use [`Self::new`] so they contend with every other writer.
    pub fn with_lock_path(feature_dir: impl Into<PathBuf>, lock_path: impl Into<PathBuf>) -> Self {
        Self {
            feature_dir: feature_dir.into(),
            lock: LockAuthority::Explicit(lock_path.into()),
        }
    }

    /// The manifest path.
    pub fn path(&self) -> PathBuf {
        self.feature_dir.join(LANES_FILE_NAME)
    }

    /// The cross-process lock path [`Self::write`] and [`Self::update`] take.
    ///
    /// For a canonical store this is resolved on demand, so it reflects the
    /// repository the feature directory currently belongs to.
    pub fn lock_path(&self) -> Result<PathBuf, WorkspaceError> {
        match &self.lock {
            LockAuthority::Canonical => lanes_json_lock_path(&self.feature_dir),
            LockAuthority::Explicit(path) => Ok(path.clone()),
        }
    }

    /// The mission directory this store is anchored on.
    pub fn feature_dir(&self) -> &Path {
        &self.feature_dir
    }

    /// Take the canonical cross-process lock (unbounded and reentrant, matching
    /// `machine_file_lock(blocking=True, timeout_s=None, reentrant=True)`).
    fn lock(&self) -> Result<FileLock, WorkspaceError> {
        let path = self.lock_path()?;
        Ok(FileLock::acquire_unbounded(&path)?)
    }

    /// Read the manifest, or `Ok(None)` when it is genuinely absent.
    ///
    /// A corrupt document is [`WorkspaceError::CorruptLanes`]; it is never
    /// downgraded to "no lanes".
    pub fn read(&self) -> Result<Option<LanesManifest>, WorkspaceError> {
        let path = self.path();
        let bytes = match sk_kernel::read_bounded(&path, MAX_LANES_BYTES) {
            Ok(bytes) => bytes,
            Err(sk_kernel::KernelError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                return Ok(None);
            }
            Err(error) => return Err(WorkspaceError::Kernel(error)),
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| WorkspaceError::CorruptLanes {
                path,
                detail: error.to_string(),
            })
    }

    /// Read, mutate and write the manifest under **one** cross-process lock.
    ///
    /// This is the only correct way to change one field of an existing manifest:
    /// a read followed by a separate [`Self::write`] loses any update another
    /// process makes in between, and a blind [`Self::write`] of a stale manifest
    /// discards it. Because the same lock file serialises every listed writer
    /// ([`Self::write`] takes it too), holding it across the read-modify-write is
    /// the compare-and-swap: no other holder can interleave.
    ///
    /// `Ok(None)` means the manifest is genuinely absent; the closure is then not
    /// called and nothing is written.
    pub fn update<F>(&self, mutate: F) -> Result<Option<LanesManifest>, WorkspaceError>
    where
        F: FnOnce(&LanesManifest) -> Result<LanesManifest, WorkspaceError>,
    {
        let _lock = self.lock()?;
        let path = self.path();
        let current = match sk_kernel::read_bounded(&path, MAX_LANES_BYTES) {
            Ok(bytes) => bytes,
            Err(sk_kernel::KernelError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                return Ok(None);
            }
            Err(error) => return Err(WorkspaceError::Kernel(error)),
        };
        let manifest: LanesManifest =
            serde_json::from_slice(&current).map_err(|error| WorkspaceError::CorruptLanes {
                path: path.clone(),
                detail: error.to_string(),
            })?;
        let updated = mutate(&manifest)?;
        let mut bytes = serde_json::to_vec_pretty(&updated).map_err(|error| {
            WorkspaceError::InvalidArtifact {
                artifact: "lanes.json",
                detail: error.to_string(),
            }
        })?;
        bytes.push(b'\n');
        sk_kernel::atomic_write(&path, &bytes)?;
        Ok(Some(updated))
    }

    /// Write the manifest under the cross-process lock, atomically.
    pub fn write(&self, manifest: &LanesManifest) -> Result<PathBuf, WorkspaceError> {
        let _lock = self.lock()?;
        let path = self.path();
        let mut bytes = serde_json::to_vec_pretty(manifest).map_err(|error| {
            WorkspaceError::InvalidArtifact {
                artifact: "lanes.json",
                detail: error.to_string(),
            }
        })?;
        bytes.push(b'\n');
        sk_kernel::atomic_write(&path, &bytes)?;
        Ok(path)
    }
}

/// The reference's canonical `lanes.json` lock path.
///
/// Mirrors `specify_cli.lanes.persistence.lanes_json_lock` exactly:
///
/// * inside a Git work tree:
///   `<git-common-dir>/spec-kitty-lanes-locks/<the lanes.json path relative to
///   the worktree root>.lock` — one lock per manifest, shared by every worktree
///   and (crucially) by every implementation;
/// * outside any repository (`NotAGitRepositoryError` upstream): a stable name
///   under `<tempdir>/spec-kitty-lanes-locks/` derived from the resolved
///   manifest path, exactly like `_non_git_lanes_lock_path`;
/// * any other topology failure, or a manifest that is not inside its own
///   worktree: a typed refusal (`LANES_LOCK_TOPOLOGY_UNRESOLVED` /
///   `LANES_LOCK_OUTSIDE_WORKTREE`). There is **no** fallback to an in-tree
///   lock, because that is exactly the split-authority defect this resolver
///   replaces: a native `<mission_dir>/.lanes.json.lock` neither contends with
///   the reference writer nor stops dirtying the checkout it lives in.
pub fn lanes_json_lock_path(feature_dir: &Path) -> Result<PathBuf, WorkspaceError> {
    let lanes_path = resolved_lanes_path(feature_dir);
    let common_dir = match sk_kernel::git_topology::git_common_dir(feature_dir) {
        Ok(common_dir) => common_dir,
        Err(sk_kernel::KernelError::NotAGitRepository { .. }) => {
            return Ok(non_git_lanes_lock_path(&lanes_path));
        }
        Err(error) => return Err(lanes_lock_refusal(feature_dir, &error)),
    };
    // `git rev-parse --show-toplevel`: a bare repository or a git-directory
    // interior has a common dir but no work tree, which upstream refuses.
    let worktree_root = match sk_kernel::git_topology::git_toplevel(feature_dir) {
        Ok(root) => root,
        Err(error) => return Err(lanes_lock_refusal(feature_dir, &error)),
    };
    let relative = lanes_path.strip_prefix(&worktree_root).map_err(|_| {
        WorkspaceError::CoordRefused {
            code: "LANES_LOCK_OUTSIDE_WORKTREE",
            message: format!(
                "lanes.json is outside its Git worktree: {} is not under {}; refusing to pick a lock that no other writer shares",
                lanes_path.display(),
                worktree_root.display()
            ),
        }
    })?;
    let name = relative
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| LANES_FILE_NAME.to_owned());
    Ok(common_dir
        .join(LANES_LOCK_DIRECTORY)
        .join(relative)
        .with_file_name(format!("{name}.lock")))
}

fn lanes_lock_refusal(feature_dir: &Path, error: &sk_kernel::KernelError) -> WorkspaceError {
    WorkspaceError::CoordRefused {
        code: "LANES_LOCK_TOPOLOGY_UNRESOLVED",
        message: format!(
            "cannot safely lock lanes.json for {}: {error}; refusing to fall back to an in-tree lock that no other writer shares",
            feature_dir.display()
        ),
    }
}

/// `_non_git_lanes_lock_path`: a stable temp-root name for a non-repository.
fn non_git_lanes_lock_path(lanes_path: &Path) -> PathBuf {
    // Python: `json.dumps(lanes_path.as_posix(), ensure_ascii=True,
    // separators=(",", ":"))`, then the bare SHA-256 hex of those bytes.
    let identity = python_json_path(lanes_path);
    let digest = sk_kernel::content_digest::sha256_hex(identity.as_bytes());
    std::env::temp_dir()
        .join(LANES_LOCK_DIRECTORY)
        .join(format!("{digest}.lock"))
}

/// `resolve_lanes_dir(feature_dir).resolve()`: the manifest path, resolved once.
fn resolved_lanes_path(feature_dir: &Path) -> PathBuf {
    let path = feature_dir.join(LANES_FILE_NAME);
    if let Ok(resolved) = std::fs::canonicalize(&path) {
        return resolved;
    }
    match std::fs::canonicalize(feature_dir) {
        Ok(dir) => dir.join(LANES_FILE_NAME),
        Err(_) => absolutize(&path),
    }
}

fn absolutize(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return normalize_lexically(path);
    }
    match std::env::current_dir() {
        Ok(cwd) => normalize_lexically(&cwd.join(path)),
        Err(_) => path.to_path_buf(),
    }
}

fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let has_normal = out
                    .components()
                    .any(|component| matches!(component, Component::Normal(_)));
                if has_normal {
                    out.pop();
                } else if !out.has_root() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(unix)]
fn python_json_path(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut out = String::from("\"");
    for chunk in path.as_os_str().as_bytes().utf8_chunks() {
        let valid = python_json_string(chunk.valid());
        out.push_str(&valid[1..valid.len() - 1]);
        // Python's filesystem decoder uses surrogateescape for these bytes;
        // ensure_ascii then renders each surrogate as its literal JSON escape.
        for byte in chunk.invalid() {
            out.push_str(&format!("\\udc{byte:02x}"));
        }
    }
    out.push('"');
    out
}

#[cfg(not(unix))]
fn python_json_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    #[cfg(windows)]
    let text = text.replace('\\', "/");
    python_json_string(&text)
}

/// `json.dumps(value, ensure_ascii=True, separators=(",", ":"))` for one string.
fn python_json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            character if !('\u{20}'..='\u{7e}').contains(&character) => {
                let code = u32::from(character);
                if code > 0xFFFF {
                    let scalar = code - 0x10000;
                    let high = 0xD800 + (scalar >> 10);
                    let low = 0xDC00 + (scalar & 0x3FF);
                    out.push_str(&format!("\\u{high:04x}\\u{low:04x}"));
                } else {
                    out.push_str(&format!("\\u{code:04x}"));
                }
            }
            character => out.push(character),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wp(value: &str) -> WpId {
        value.parse().unwrap()
    }

    fn input() -> LanePlannerInput {
        LanePlannerInput::new(
            "my-mission".parse().unwrap(),
            MissionTopology::Lanes,
            "main",
        )
    }

    #[test]
    fn disjoint_ownership_produces_parallel_lanes() {
        let mut input = input();
        input
            .work_packages
            .insert(wp("WP01").clone(), WpPlan::code(wp("WP01"), ["src/a/**"]));
        input
            .work_packages
            .insert(wp("WP02").clone(), WpPlan::code(wp("WP02"), ["src/b/**"]));
        input.dependencies.insert(wp("WP01"), vec![]);
        input.dependencies.insert(wp("WP02"), vec![]);
        let manifest = compute_lanes(&input).unwrap();
        assert_eq!(manifest.lanes.len(), 2);
        assert_eq!(manifest.lanes[0].parallel_group, 0);
        assert_eq!(manifest.lanes[1].parallel_group, 0);
    }

    #[test]
    fn overlapping_ownership_collapses_and_records_evidence() {
        let mut input = input();
        input.work_packages.insert(
            wp("WP01").clone(),
            WpPlan::code(wp("WP01"), ["src/shared/*.rs"]),
        );
        input.work_packages.insert(
            wp("WP02").clone(),
            WpPlan::code(wp("WP02"), ["src/shared/mod.rs"]),
        );
        input.dependencies.insert(wp("WP01"), vec![]);
        input.dependencies.insert(wp("WP02"), vec![]);
        let manifest = compute_lanes(&input).unwrap();
        assert_eq!(manifest.lanes.len(), 1);
        assert_eq!(manifest.lanes[0].wp_ids, vec![wp("WP01"), wp("WP02")]);
        let report = manifest.collapse_report.expect("collapse evidence");
        assert_eq!(report.events[0].rule, CollapseRule::WriteScopeOverlap);
        assert_eq!(report.independent_wps_collapsed, 1);
    }

    #[test]
    fn dependency_edges_become_lane_dependencies_not_collapses() {
        let mut input = input();
        input
            .work_packages
            .insert(wp("WP01").clone(), WpPlan::code(wp("WP01"), ["src/a/**"]));
        input
            .work_packages
            .insert(wp("WP02").clone(), WpPlan::code(wp("WP02"), ["src/b/**"]));
        input.dependencies.insert(wp("WP01"), vec![]);
        input.dependencies.insert(wp("WP02"), vec![wp("WP01")]);
        let manifest = compute_lanes(&input).unwrap();
        assert_eq!(manifest.lanes.len(), 2);
        let lane_b = manifest
            .lanes
            .iter()
            .find(|lane| lane.contains(&wp("WP02")))
            .unwrap();
        let lane_a = manifest
            .lanes
            .iter()
            .find(|lane| lane.contains(&wp("WP01")))
            .unwrap();
        assert_eq!(lane_b.depends_on_lanes, vec![lane_a.lane_id.clone()]);
        assert_eq!(lane_b.parallel_group, lane_a.parallel_group + 1);
    }

    #[test]
    fn planning_artifacts_get_the_repo_root_lane() {
        let mut input = input();
        input.work_packages.insert(
            wp("WP01").clone(),
            WpPlan::planning(wp("WP01"), ["kitty-specs/**"]),
        );
        input
            .work_packages
            .insert(wp("WP02").clone(), WpPlan::code(wp("WP02"), ["src/a/**"]));
        input.dependencies.insert(wp("WP01"), vec![]);
        input.dependencies.insert(wp("WP02"), vec![wp("WP01")]);
        let manifest = compute_lanes(&input).unwrap();
        let planning = manifest
            .lanes
            .iter()
            .find(|lane| lane.lane_id == PLANNING_LANE_ID)
            .unwrap();
        assert_eq!(planning.wp_ids, vec![wp("WP01")]);
        assert_eq!(manifest.planning_artifact_wps, vec![wp("WP01")]);
        let code = manifest
            .lanes
            .iter()
            .find(|lane| lane.lane_id != PLANNING_LANE_ID)
            .unwrap();
        assert_eq!(code.depends_on_lanes, vec![PLANNING_LANE_ID]);
    }

    #[test]
    fn single_branch_puts_every_wp_in_the_planning_lane() {
        let mut input = LanePlannerInput::new(
            "my-mission".parse().unwrap(),
            MissionTopology::SingleBranch,
            "main",
        );
        input
            .work_packages
            .insert(wp("WP01").clone(), WpPlan::code(wp("WP01"), ["src/a/**"]));
        input.work_packages.insert(
            wp("WP02").clone(),
            WpPlan::planning(wp("WP02"), ["kitty-specs/**"]),
        );
        input.dependencies.insert(wp("WP01"), vec![]);
        input.dependencies.insert(wp("WP02"), vec![wp("WP01")]);
        let manifest = compute_lanes(&input).unwrap();
        assert_eq!(manifest.lanes.len(), 1);
        assert_eq!(manifest.lanes[0].lane_id, PLANNING_LANE_ID);
        assert_eq!(manifest.lanes[0].wp_ids, vec![wp("WP01"), wp("WP02")]);
        assert_eq!(manifest.mission_branch, "main");
    }

    #[test]
    fn re_finalize_reads_back_a_surviving_lane_id() {
        let mut first = input();
        first
            .work_packages
            .insert(wp("WP01").clone(), WpPlan::code(wp("WP01"), ["src/a/**"]));
        first
            .work_packages
            .insert(wp("WP02").clone(), WpPlan::code(wp("WP02"), ["src/b/**"]));
        first.dependencies.insert(wp("WP01"), vec![]);
        first.dependencies.insert(wp("WP02"), vec![]);
        let previous = compute_lanes(&first).unwrap();
        let lane_of_b = previous
            .lane_for_wp(&wp("WP02"))
            .expect("previous lane")
            .lane_id
            .clone();

        // Remove WP01; WP02 must keep its previous id instead of being
        // re-lettered onto the freed positional slot.
        let mut second = input();
        second
            .work_packages
            .insert(wp("WP02").clone(), WpPlan::code(wp("WP02"), ["src/b/**"]));
        second.dependencies.insert(wp("WP02"), vec![]);
        second.previous = Some(previous);
        let manifest = compute_lanes(&second).unwrap();
        assert_eq!(
            manifest.lane_for_wp(&wp("WP02")).unwrap().lane_id,
            lane_of_b
        );
    }

    #[test]
    fn frozen_started_lane_mates_stay_together_and_pin_their_id() {
        let mut input = input();
        input
            .work_packages
            .insert(wp("WP01").clone(), WpPlan::code(wp("WP01"), ["src/a/**"]));
        input
            .work_packages
            .insert(wp("WP02").clone(), WpPlan::code(wp("WP02"), ["src/b/**"]));
        input.dependencies.insert(wp("WP01"), vec![]);
        input.dependencies.insert(wp("WP02"), vec![]);
        input
            .frozen_lane_bindings
            .insert(wp("WP01"), "lane-c".to_owned());
        input
            .frozen_lane_bindings
            .insert(wp("WP02"), "lane-c".to_owned());
        let manifest = compute_lanes(&input).unwrap();
        assert_eq!(manifest.lanes.len(), 1);
        assert_eq!(manifest.lanes[0].lane_id, "lane-c");
    }

    #[test]
    fn frozen_work_package_removal_refuses() {
        let mut input = input();
        input.dependencies.insert(wp("WP01"), vec![]);
        input
            .work_packages
            .insert(wp("WP01").clone(), WpPlan::code(wp("WP01"), ["src/a/**"]));
        input
            .frozen_lane_bindings
            .insert(wp("WP09"), "lane-a".to_owned());
        let error = compute_lanes(&input).unwrap_err();
        assert!(matches!(error, WorkspaceError::LanePlanning { .. }));
    }

    #[test]
    fn glob_intersection_is_conservative_and_correct() {
        assert!(globs_overlap("src/**", "src/shared/*.rs"));
        assert!(globs_overlap("src/*/mod.rs", "src/**"));
        assert!(globs_overlap("src/a.rs", "src/?.rs"));
        assert!(!globs_overlap("src/a/**", "src/b/**"));
        assert!(!globs_overlap("src/a.rs", "src/b.rs"));
    }

    #[test]
    fn lane_cycle_is_refused() {
        let mut input = input();
        input
            .work_packages
            .insert(wp("WP01").clone(), WpPlan::code(wp("WP01"), ["src/a/**"]));
        input
            .work_packages
            .insert(wp("WP02").clone(), WpPlan::code(wp("WP02"), ["src/b/**"]));
        input.dependencies.insert(wp("WP01"), vec![wp("WP02")]);
        input.dependencies.insert(wp("WP02"), vec![wp("WP01")]);
        let error = compute_lanes(&input).unwrap_err();
        assert!(matches!(error, WorkspaceError::LaneDependencyCycle { .. }));
    }
}
