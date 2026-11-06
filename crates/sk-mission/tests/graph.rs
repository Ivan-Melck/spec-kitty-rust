mod support;

use std::fs;
use std::path::Path;

use sk_mission::{MissionError, WorkPackage, WorkPackageGraph};
use support::wp_source;

fn package(id: &str, dependencies_yaml: &str) -> WorkPackage {
    let filename = format!("{id}-fixture.md");
    WorkPackage::parse(Path::new(&filename), &wp_source(id, dependencies_yaml)).expect("package")
}

fn ids(graph: &WorkPackageGraph) -> Vec<String> {
    graph
        .topological_order()
        .iter()
        .map(|id| id.as_str().to_owned())
        .collect()
}

#[test]
fn builds_from_a_tasks_directory_and_orders_dependencies_first() {
    let tasks = support::fixtures_project().join("kitty-specs/002-lightweight-pypi-release/tasks");
    let graph = WorkPackageGraph::from_tasks_dir(&tasks).expect("graph");

    assert_eq!(graph.len(), 3);
    assert_eq!(ids(&graph), vec!["WP01", "WP02", "WP03"]);

    let wp02 = graph.get(&"WP02".parse().expect("wp id")).expect("WP02");
    assert_eq!(wp02.title(), Some("PyPI Release Automation"));

    assert_eq!(
        graph
            .dependents_of(&"WP01".parse().expect("wp id"))
            .expect("dependents")
            .iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>(),
        vec!["WP02", "WP03"]
    );
    assert_eq!(
        graph
            .dependencies_of(&"WP03".parse().expect("wp id"))
            .expect("dependencies")
            .iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>(),
        vec!["WP01", "WP02"]
    );
}

#[test]
fn topological_order_is_independent_of_input_order() {
    let forward = vec![
        package("WP01", "[]"),
        package("WP02", "\n- WP01"),
        package("WP03", "\n- WP02"),
        package("WP04", "\n- WP01"),
    ];
    let mut backward = forward.clone();
    backward.reverse();

    let forward_graph = WorkPackageGraph::build(forward).expect("forward graph");
    let backward_graph = WorkPackageGraph::build(backward).expect("backward graph");
    assert_eq!(ids(&forward_graph), ids(&backward_graph));
    assert_eq!(ids(&forward_graph), vec!["WP01", "WP02", "WP03", "WP04"]);
}

#[test]
fn diamond_graph_order_is_deterministic() {
    let graph = WorkPackageGraph::build(vec![
        package("WP04", "\n- WP03\n- WP02"),
        package("WP03", "\n- WP01"),
        package("WP02", "\n- WP01"),
        package("WP01", "[]"),
    ])
    .expect("diamond graph");

    assert_eq!(ids(&graph), vec!["WP01", "WP02", "WP03", "WP04"]);
    assert_eq!(
        graph
            .dependents_of(&"WP04".parse().unwrap())
            .expect("dependents"),
        &[] as &[sk_mission::WpId]
    );
}

#[test]
fn unknown_dependency_is_refused() {
    let error =
        WorkPackageGraph::build(vec![package("WP02", "\n- WP09")]).expect_err("unknown dependency");
    match error {
        MissionError::UnknownDependency { wp, dependency } => {
            assert_eq!(wp.as_str(), "WP02");
            assert_eq!(dependency.as_str(), "WP09");
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn self_dependency_is_refused() {
    let error =
        WorkPackageGraph::build(vec![package("WP01", "\n- WP01")]).expect_err("self dependency");
    assert!(
        matches!(error, MissionError::SelfDependency { .. }),
        "unexpected error: {error}"
    );
}

#[test]
fn cycle_is_refused_with_a_deterministic_path() {
    let error = WorkPackageGraph::build(vec![
        package("WP08", "[]"),
        package("WP01", "\n- WP02"),
        package("WP02", "\n- WP03"),
        package("WP03", "\n- WP01"),
    ])
    .expect_err("cycle");
    match error {
        MissionError::DependencyCycle { cycle } => {
            assert_eq!(cycle, "WP01 -> WP02 -> WP03 -> WP01");
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn duplicate_work_packages_are_refused() {
    let error = WorkPackageGraph::build(vec![package("WP01", "[]"), package("WP01", "[]")])
        .expect_err("duplicate id");
    assert!(
        matches!(error, MissionError::DuplicateWorkPackage { .. }),
        "unexpected error: {error}"
    );
}

#[test]
fn absent_tasks_directory_is_an_empty_graph() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let graph =
        WorkPackageGraph::from_tasks_dir(&temp.path().join("missing-tasks")).expect("empty graph");
    assert!(graph.is_empty());
    assert_eq!(graph.len(), 0);
}

#[test]
fn tasks_scan_ignores_readmes_and_nested_directories() {
    let temp = tempfile::tempdir().expect("temporary directory");
    support::write(&temp.path().join("README.md"), "# Tasks overview\n");
    support::write(&temp.path().join("WP01-first.md"), &wp_source("WP01", "[]"));
    support::write(
        &temp.path().join("nested/WP09-ignored.md"),
        &wp_source("WP09", "[]"),
    );

    let graph = WorkPackageGraph::from_tasks_dir(temp.path()).expect("graph");
    assert_eq!(graph.len(), 1);
    assert_eq!(ids(&graph), vec!["WP01"]);
}

#[cfg(unix)]
#[test]
fn symlinked_work_package_file_is_refused() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().expect("temporary directory");
    let outside = tempfile::tempdir().expect("outside directory");
    let target = outside.path().join("WP01-outside.md");
    fs::write(&target, wp_source("WP01", "[]")).expect("write target");
    symlink(&target, temp.path().join("WP01-linked.md")).expect("symlink");

    let error = WorkPackageGraph::from_tasks_dir(temp.path()).expect_err("symlinked WP");
    assert!(
        matches!(error, MissionError::SymlinkedEntry { .. }),
        "unexpected error: {error}"
    );
}
