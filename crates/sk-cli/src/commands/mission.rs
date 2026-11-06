//! Mission inventory, selection, and dependency graph.

use std::path::Path;

use serde_json::{Value, json};
use sk_mission::{Mission, WorkPackage, WorkPackageGraph, list_missions, select_mission};

use crate::commands::path_string;
use crate::commands::project::resolve_root;
use crate::output::{CliError, CommandData};

fn mission_error(context: &str, error: sk_mission::MissionError) -> CliError {
    CliError::domain(format!("{context}: {error}"))
}

fn summary(mission: &Mission) -> Value {
    json!({
        "slug": mission.slug().as_str(),
        "mission_id": mission.id().as_str(),
        "mid8": mission.mid8(),
        "mission_number": mission.mission_number(),
        "mission_number_display": mission.mission_number_display(),
        "mission_type": mission.mission_type(),
        "friendly_name": mission.friendly_name(),
        "display_name": mission.display_name(),
        "topology": mission.topology().map(|topology| topology.as_str()),
        "flattened": mission.flattened(),
        "spec_dir": path_string(mission.spec_dir()),
        "meta": mission.meta(),
    })
}

fn package_value(package: &WorkPackage, graph: &WorkPackageGraph) -> Value {
    let dependents: Vec<String> = graph
        .dependents_of(package.id())
        .map(|ids| ids.iter().map(ToString::to_string).collect())
        .unwrap_or_default();
    json!({
        "id": package.id().as_str(),
        "title": package.title(),
        "dependencies": package.dependencies(),
        "dependents": dependents,
        "source": path_string(package.source()),
    })
}

fn graph_value(mission: &Mission, graph: &WorkPackageGraph) -> Value {
    let packages: Vec<Value> = graph
        .work_packages()
        .map(|package| package_value(package, graph))
        .collect();
    let edges: Vec<Value> = graph
        .work_packages()
        .flat_map(|package| {
            package.dependencies().iter().map(move |dependency| {
                json!({
                    "dependency": dependency.as_str(),
                    "dependent": package.id().as_str(),
                })
            })
        })
        .collect();

    json!({
        "work_package_count": graph.len(),
        "empty": graph.is_empty(),
        "topological_order": graph.topological_order(),
        "packages": packages,
        "edges": edges,
        "tasks_dir": path_string(&mission.spec_dir().join("tasks")),
    })
}

fn select(root: &Path, selector: &str) -> Result<(Mission, WorkPackageGraph), CliError> {
    let missions =
        list_missions(root).map_err(|error| mission_error("could not list Missions", error))?;
    let mission = select_mission(&missions, selector)
        .map_err(|error| mission_error(&format!("selector {selector:?} was refused"), error))?
        .clone();
    let graph = WorkPackageGraph::from_tasks_dir(&mission.spec_dir().join("tasks"))
        .map_err(|error| mission_error("could not read the work-package graph", error))?;
    Ok((mission, graph))
}

/// List every Mission under the project root.
pub fn list(explicit: Option<&Path>) -> Result<CommandData, CliError> {
    let root = resolve_root(explicit)?;
    let missions =
        list_missions(&root).map_err(|error| mission_error("could not list Missions", error))?;
    let summaries: Vec<Value> = missions.iter().map(summary).collect();
    Ok(CommandData::success(json!({
        "count": summaries.len(),
        "missions": summaries,
        "read_only": true,
    })))
}

/// Show one Mission and its parsed work packages.
pub fn show(explicit: Option<&Path>, selector: &str) -> Result<CommandData, CliError> {
    let root = resolve_root(explicit)?;
    let (mission, graph) = select(&root, selector)?;
    let packages: Vec<Value> = graph
        .work_packages()
        .map(|package| package_value(package, &graph))
        .collect();
    Ok(CommandData::success(json!({
        "mission": summary(&mission),
        "work_packages": {
            "count": packages.len(),
            "packages": packages,
        },
        "read_only": true,
        "state_reconstructed": false,
    })))
}

/// Show one Mission's dependency graph.
pub fn graph(explicit: Option<&Path>, selector: &str) -> Result<CommandData, CliError> {
    let root = resolve_root(explicit)?;
    let (mission, graph) = select(&root, selector)?;
    Ok(CommandData::success(json!({
        "mission": summary(&mission),
        "graph": graph_value(&mission, &graph),
        "read_only": true,
        "state_reconstructed": false,
        "readiness_evaluated": false,
    })))
}
