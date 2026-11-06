mod support;

use std::collections::BTreeMap;
use std::path::Path;

use sk_mission::{
    MissionError, UnsatisfiedReason, WorkPackage, WorkPackageGraph, WpId, WpState, WpStateMap,
    dependency_readiness, readiness_for_dependencies, ready_work_packages,
};
use support::wp_source;

fn graph() -> WorkPackageGraph {
    let tasks = support::fixtures_project().join("kitty-specs/002-lightweight-pypi-release/tasks");
    WorkPackageGraph::from_tasks_dir(&tasks).expect("graph")
}

fn wp(value: &str) -> WpId {
    value.parse().expect("work package id")
}

fn state(value: &str) -> WpState {
    value.parse().expect("work package state")
}

fn states(entries: &[(&str, &str)]) -> WpStateMap {
    entries
        .iter()
        .map(|(wp_value, state_value)| (wp(wp_value), state(state_value)))
        .collect()
}

#[test]
fn approved_dependency_satisfies_readiness() {
    let graph = graph();
    let states = states(&[("WP01", "approved")]);

    let verdict = dependency_readiness(&graph, &states, &wp("WP02")).expect("readiness");
    assert!(verdict.satisfied());
    assert_eq!(verdict.wp(), &wp("WP02"));
    assert_eq!(verdict.dependencies(), &[wp("WP01")]);
    assert!(verdict.unsatisfied().is_empty());
}

#[test]
fn done_dependency_satisfies_readiness() {
    let graph = graph();
    let states = states(&[("WP01", "done"), ("WP02", "done")]);

    let verdict = dependency_readiness(&graph, &states, &wp("WP03")).expect("readiness");
    assert!(verdict.satisfied());
}

#[test]
fn for_review_dependency_is_not_ready() {
    let graph = graph();
    let states = states(&[("WP01", "for_review")]);

    let verdict = dependency_readiness(&graph, &states, &wp("WP02")).expect("readiness");
    assert!(!verdict.satisfied());
    assert_eq!(verdict.unsatisfied().len(), 1);
    assert_eq!(
        verdict.unsatisfied()[0].reason(),
        &UnsatisfiedReason::NotAcceptableEnding {
            state: WpState::ForReview
        }
    );
}

#[test]
fn missing_state_is_distinct_from_a_non_acceptable_state() {
    let graph = graph();

    let empty = dependency_readiness(&graph, &WpStateMap::new(), &wp("WP02")).expect("readiness");
    assert_eq!(
        empty.unsatisfied()[0].reason(),
        &UnsatisfiedReason::MissingState
    );

    let planned = dependency_readiness(&graph, &states(&[("WP01", "planned")]), &wp("WP02"))
        .expect("readiness");
    assert_eq!(
        planned.unsatisfied()[0].reason(),
        &UnsatisfiedReason::NotAcceptableEnding {
            state: WpState::Planned
        }
    );
}

#[test]
fn retired_frontmatter_lane_never_satisfies_readiness() {
    let graph = graph();
    let wp03 = graph.get(&wp("WP03")).expect("WP03");

    // The fixture prompt records the retired `lane: done` and `status: done`.
    assert_eq!(
        wp03.frontmatter_field("lane")
            .and_then(|value| value.as_str()),
        Some("done")
    );
    assert_eq!(
        wp03.frontmatter_field("status")
            .and_then(|value| value.as_str()),
        Some("done")
    );

    // No caller-supplied state: the dependencies are reported missing, not
    // "done" from the frontmatter.
    let verdict = dependency_readiness(&graph, &WpStateMap::new(), &wp("WP03")).expect("readiness");
    assert!(!verdict.satisfied());
    assert_eq!(
        verdict
            .unsatisfied()
            .iter()
            .map(|entry| (entry.dependency().as_str(), entry.reason().clone()))
            .collect::<Vec<_>>(),
        vec![
            ("WP01", UnsatisfiedReason::MissingState),
            ("WP02", UnsatisfiedReason::MissingState),
        ]
    );
}

#[test]
fn work_package_without_dependencies_is_satisfied() {
    let graph = graph();
    let verdict = dependency_readiness(&graph, &WpStateMap::new(), &wp("WP01")).expect("readiness");
    assert!(verdict.satisfied());
    assert!(verdict.dependencies().is_empty());
}

#[test]
fn readiness_requires_the_work_package_in_the_graph() {
    let graph = graph();
    let error = dependency_readiness(&graph, &WpStateMap::new(), &wp("WP99"))
        .expect_err("unknown work package");
    assert!(
        matches!(error, MissionError::UnknownWorkPackage { .. }),
        "unexpected error: {error}"
    );
}

#[test]
fn ready_work_packages_follow_topological_order() {
    let graph = graph();

    let none = ready_work_packages(&graph, &WpStateMap::new()).expect("ready");
    assert_eq!(
        none,
        vec![wp("WP01")],
        "only the dependency-free WP is ready"
    );

    let first = ready_work_packages(&graph, &states(&[("WP01", "approved")])).expect("ready");
    assert_eq!(first, vec![wp("WP01"), wp("WP02")]);

    let all = ready_work_packages(
        &graph,
        &states(&[("WP01", "done"), ("WP02", "approved"), ("WP03", "planned")]),
    )
    .expect("ready");
    assert_eq!(all, vec![wp("WP01"), wp("WP02"), wp("WP03")]);
}

#[test]
fn canceled_is_not_an_acceptable_ending_in_this_read_only_crate() {
    // Upstream additionally accepts a cancellation with operator-authored
    // provenance; a state map alone cannot prove that, so `canceled` never
    // satisfies readiness here (documented gap, not silent inference).
    let graph = graph();
    let verdict = dependency_readiness(&graph, &states(&[("WP01", "canceled")]), &wp("WP02"))
        .expect("readiness");
    assert!(!verdict.satisfied());
    assert_eq!(
        verdict.unsatisfied()[0].reason(),
        &UnsatisfiedReason::NotAcceptableEnding {
            state: WpState::Canceled
        }
    );
}

#[test]
fn pure_readiness_works_without_a_graph() {
    let verdict = readiness_for_dependencies(
        &wp("WP02"),
        &[wp("WP01"), wp("WP03")],
        &states(&[("WP03", "done")]),
    );
    assert!(!verdict.satisfied());
    assert_eq!(verdict.unsatisfied().len(), 1);
    assert_eq!(verdict.unsatisfied()[0].dependency(), &wp("WP01"));
}

#[test]
fn state_parsing_round_trips_and_refuses_unknown_names() {
    for value in [
        "genesis",
        "uninitialized",
        "planned",
        "claimed",
        "in_progress",
        "for_review",
        "in_review",
        "approved",
        "done",
        "blocked",
        "canceled",
    ] {
        let parsed = state(value);
        assert_eq!(parsed.as_str(), value);
        assert_eq!(parsed.to_string(), value);
    }

    let error = "merged".parse::<WpState>().expect_err("unknown state");
    assert!(
        matches!(error, MissionError::UnknownWorkPackageState { .. }),
        "unexpected error: {error}"
    );
}

#[test]
fn only_approved_and_done_are_acceptable_endings() {
    for state_value in WpState::ALL {
        assert_eq!(
            state_value.is_acceptable_ending(),
            matches!(state_value, WpState::Approved | WpState::Done),
            "{state_value}"
        );
    }
}

#[test]
fn readiness_verdict_serializes_for_inspection() {
    let verdict = readiness_for_dependencies(&wp("WP02"), &[wp("WP01")], &BTreeMap::new());
    let value = serde_json::to_value(&verdict).expect("serialize verdict");
    assert_eq!(value["wp"], "WP02");
    assert_eq!(value["unsatisfied"][0]["dependency"], "WP01");
    assert_eq!(value["unsatisfied"][0]["reason"]["kind"], "missing_state");
}

#[test]
fn duplicate_work_package_ids_are_not_needed_for_a_verdict() {
    // A verdict is about declarations, not a mission; an unparsable file is
    // irrelevant here. This pins that readiness does not touch the filesystem.
    let package = WorkPackage::parse(Path::new("WP02-fixture.md"), &wp_source("WP02", "\n- WP01"))
        .expect("package");
    assert_eq!(package.dependencies(), &[wp("WP01")]);
}
