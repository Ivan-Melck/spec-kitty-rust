//! Work-package read routes: `agent tasks list-tasks`, `agent tasks status`
//! and `agent tasks list-dependents`.
//!
//! Lanes come from the canonical event log through `sk-status`; frontmatter is
//! read through `sk-mission`. An absent event log reports every WP as
//! `planned`, exactly as the upstream board does before the first event.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value, json};
use sk_mission::{WorkPackageGraph, list_missions};
use sk_status::Snapshot;

use super::status::require_mission;
use super::support::{
    CompatError, canonical_lanes, emit_json, emit_json_pretty, resolve_mission, tasks_dir,
};

/// Upstream lane-weighted progress weights.
const LANE_WEIGHTS: &[(&str, f64)] = &[
    ("genesis", 0.0),
    ("uninitialized", 0.0),
    ("planned", 0.0),
    ("claimed", 0.05),
    ("in_progress", 0.3),
    ("for_review", 0.6),
    ("in_review", 0.7),
    ("approved", 0.8),
    ("done", 1.0),
    ("blocked", 0.0),
    ("canceled", 0.0),
];

fn lane_weight(lane: &str) -> f64 {
    LANE_WEIGHTS
        .iter()
        .find(|(name, _)| *name == lane)
        .map(|(_, weight)| *weight)
        .unwrap_or(0.0)
}

fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

/// Compute the upstream weighted-readiness percentage for a snapshot.
pub fn weighted_percentage(snapshot: Option<&Snapshot>) -> f64 {
    let Some(snapshot) = snapshot else {
        return 0.0;
    };
    if snapshot.work_packages.is_empty() {
        return 0.0;
    }
    let mut weighted = 0.0;
    for state in snapshot.work_packages.values() {
        let lane = state.lane_string().unwrap_or("genesis");
        weighted += lane_weight(lane);
    }
    round1(weighted / snapshot.work_packages.len() as f64 * 100.0)
}

fn canonical_snapshot(mission: &sk_mission::Mission) -> Option<Snapshot> {
    let path = super::support::events_path(mission);
    let rows = sk_status::read_raw_events(&path).ok()?;
    if rows.is_empty() {
        return None;
    }
    sk_status::reduce_rows(&rows).ok()
}

fn frontmatter_string(package: &sk_mission::WorkPackage, key: &str) -> Option<String> {
    package
        .frontmatter_field(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// List tasks, optionally filtered by an exact lane token.
pub fn list_tasks(
    root: &Path,
    handle: Option<&str>,
    lane: Option<&str>,
    json_output: bool,
) -> Result<Value, CompatError> {
    let mission = require_mission(root, handle)?;
    let directory = tasks_dir(&mission);
    if !directory.is_dir() {
        return Err(CompatError::domain(format!(
            "Tasks directory not found: {}",
            directory.display()
        )));
    }
    let lanes = canonical_lanes(&mission)?;
    let packages = super::support::work_packages(&mission)?;

    let mut tasks = Vec::new();
    for package in &packages {
        let wp_id = package.id().as_str();
        let task_lane = lanes
            .get(wp_id)
            .cloned()
            .unwrap_or_else(|| "planned".to_owned());
        if let Some(filter) = lane
            && task_lane != filter
        {
            continue;
        }
        tasks.push(json!({
            "work_package_id": wp_id,
            "title": package.title().unwrap_or(""),
            "lane": task_lane,
            "path": super::support::path_string(package.source()),
        }));
    }

    let value = json!({"tasks": tasks, "count": tasks.len()});
    if json_output {
        emit_json(&value)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else if tasks.is_empty() {
        println!(
            "No tasks found{}",
            lane.map(|lane| format!(" in lane {lane}"))
                .unwrap_or_default()
        );
    } else {
        println!(
            "Tasks{}:",
            lane.map(|lane| format!(" in lane {lane}"))
                .unwrap_or_default()
        );
        for task in &tasks {
            println!(
                "  {}: {} [{}]",
                task["work_package_id"].as_str().unwrap_or(""),
                task["title"].as_str().unwrap_or(""),
                task["lane"].as_str().unwrap_or("")
            );
        }
    }
    Ok(value)
}

/// The kanban status board's machine payload.
pub fn status(root: &Path, handle: Option<&str>, json_output: bool) -> Result<Value, CompatError> {
    let mission = require_mission(root, handle)?;
    let directory = tasks_dir(&mission);
    if !directory.is_dir() {
        return Err(CompatError::domain(format!(
            "Tasks directory not found: {}",
            directory.display()
        )));
    }
    let lanes = canonical_lanes(&mission)?;
    let packages = super::support::work_packages(&mission)?;
    let snapshot = canonical_snapshot(&mission);

    let mut rows = Vec::new();
    let mut lane_counts: BTreeMap<String, u64> = BTreeMap::new();
    let mut done_count = 0u64;
    for package in &packages {
        let wp_id = package.id().as_str();
        let lane = lanes
            .get(wp_id)
            .cloned()
            .unwrap_or_else(|| "planned".to_owned());
        if lane == "done" {
            done_count += 1;
        }
        *lane_counts.entry(lane.clone()).or_default() += 1;

        let state = snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.work_packages.get(wp_id));
        let state_str = |key: &str| {
            state
                .and_then(|state| state.get(key))
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        let agent = frontmatter_string(package, "agent").or_else(|| state_str("actor"));
        let phase =
            frontmatter_string(package, "phase").unwrap_or_else(|| "Unknown Phase".to_owned());
        let file = package
            .source()
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();

        rows.push(json!({
            "id": wp_id,
            "title": package.title().unwrap_or(""),
            "lane": lane,
            "phase": phase,
            "file": file,
            "agent": agent,
            "agent_profile": frontmatter_string(package, "agent_profile").unwrap_or_default(),
            "resolved_agent_profile": state_str("agent_profile"),
            "resolved_role": state_str("role"),
            "resolved_model": state_str("model"),
            "shell_pid": state_str("shell_pid"),
            "shell_pid_created_at": state_str("shell_pid_created_at"),
            "execution_mode": frontmatter_string(package, "execution_mode")
                .unwrap_or_else(|| "worktree".to_owned()),
            "workspace_kind": "lane",
        }));
    }

    let total = rows.len() as f64;
    let done_percentage = if total <= 0.0 {
        0.0
    } else {
        round1(done_count as f64 / total * 100.0)
    };
    let progress = weighted_percentage(snapshot.as_ref());

    let identity = |mission: &sk_mission::Mission| {
        (
            mission.slug().as_str().to_owned(),
            mission
                .mission_number()
                .map(Value::from)
                .unwrap_or(Value::Null),
            mission.mission_type().unwrap_or_default().to_owned(),
        )
    };
    let (slug, number, mission_type) = identity(&mission);

    let payload = json!({
        "mission_slug": slug,
        "mission_number": number,
        "mission_type": mission_type,
        "total_wps": rows.len(),
        "by_lane": lane_counts,
        "work_packages": rows,
        "progress_percentage": progress,
        "progress_semantics": "weighted_readiness",
        "weighted_percentage": progress,
        "done_count": done_count,
        "done_percentage": done_percentage,
        "stale_wps": 0,
        "stale_verdicts": [],
        "stalled_wps": [],
        "auto_commit": true,
    });

    if json_output {
        emit_json_pretty(&payload)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else if rows.is_empty() {
        println!("No work packages found in {}", directory.display());
    } else {
        println!(
            "Mission {}: {}/{} done ({done_percentage:.1}%), weighted {progress:.1}%",
            mission.slug().as_str(),
            done_count,
            rows.len()
        );
    }
    Ok(payload)
}

/// Downstream dependents and upstream dependencies of one WP.
pub fn list_dependents(
    root: &Path,
    handle: Option<&str>,
    wp_id: &str,
    json_output: bool,
) -> Result<Value, CompatError> {
    let mission = require_mission(root, handle)?;
    let directory = tasks_dir(&mission);
    if !directory.is_dir() {
        return Err(CompatError::domain(format!(
            "Mission directory not found: {}",
            mission.spec_dir().display()
        )));
    }
    let graph = WorkPackageGraph::from_tasks_dir(&directory).map_err(|error| {
        CompatError::domain(format!("could not build the work-package graph: {error}"))
    })?;
    let target: sk_kernel::WpId = wp_id
        .parse()
        .map_err(|error| CompatError::domain(format!("invalid work package id: {error}")))?;
    let depends_on: Vec<String> = graph
        .dependencies_of(&target)
        .map(|deps| deps.iter().map(ToString::to_string).collect())
        .unwrap_or_default();
    let dependents: Vec<String> = graph
        .dependents_of(&target)
        .map(|deps| deps.iter().map(ToString::to_string).collect())
        .unwrap_or_default();

    let value = json!({
        "wp_id": wp_id,
        "depends_on": depends_on,
        "dependents": dependents,
    });
    if json_output {
        emit_json(&value)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        println!(
            "{wp_id} depends on: {}",
            if depends_on.is_empty() {
                "(none)".to_owned()
            } else {
                depends_on.join(", ")
            }
        );
        println!(
            "{wp_id} depended on by: {}",
            if dependents.is_empty() {
                "(none)".to_owned()
            } else {
                dependents.join(", ")
            }
        );
    }
    Ok(value)
}

/// List the whole inventory (used by capability diagnostics and tests).
pub fn inventory(root: &Path) -> Result<Vec<String>, CompatError> {
    let missions = list_missions(root)
        .map_err(|error| CompatError::domain(format!("could not read missions: {error}")))?;
    let mut ids: Vec<String> = missions
        .iter()
        .map(|mission| mission.slug().as_str().to_owned())
        .collect();
    ids.sort();
    Ok(ids)
}

/// Resolve a mission for a status read (shared with the CLI dispatcher).
pub fn mission_for(root: &Path, handle: Option<&str>) -> Result<sk_mission::Mission, CompatError> {
    resolve_mission(root, handle)
}

/// Typed error shim kept for dispatcher symmetry.
pub fn machine_error(message: impl Into<String>) -> CompatError {
    CompatError::domain(message)
}

#[allow(dead_code)]
fn identity_fields(mission: &sk_mission::Mission) -> Map<String, Value> {
    let mut map = Map::new();
    map.insert("mission_slug".into(), json!(mission.slug().as_str()));
    map.insert(
        "mission_number".into(),
        mission
            .mission_number()
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    map.insert(
        "mission_type".into(),
        json!(mission.mission_type().unwrap_or_default()),
    );
    map
}
