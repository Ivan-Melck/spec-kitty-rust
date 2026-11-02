use std::path::PathBuf;

use sk_runtime::{ActionStep, RuntimeError, WorkflowRegistry, WorkflowSequence, load_workflow};

fn repo_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

fn action(name: &str, next: &[&str], terminal: bool) -> ActionStep {
    ActionStep {
        action_name: name.to_string(),
        next: next.iter().map(|value| value.to_string()).collect(),
        description: format!("Do {name}"),
        terminal,
        agent_profile: None,
        human_in_the_loop: None,
        integration: None,
        outputs: Default::default(),
    }
}

fn workflow(actions: Vec<ActionStep>, initial: &str, version: u32) -> WorkflowSequence {
    WorkflowSequence {
        workflow_id: "test-flow".to_string(),
        description: "Test flow".to_string(),
        actions,
        initial: initial.to_string(),
        version,
        integrations: Default::default(),
    }
}

#[test]
fn loads_and_walks_the_upstream_default_workflow() {
    let path = repo_path("src/charter/offering/workflows/software-dev-default.workflow.yaml");
    let workflow = load_workflow(&path).expect("upstream default workflow should load");

    assert_eq!(workflow.workflow_id, "software-dev-default");
    assert_eq!(workflow.version, 1);
    assert_eq!(workflow.initial, "specify");

    let mut steps = Vec::new();
    let mut current = workflow.initial.clone();
    loop {
        let next = workflow.next_action(&current).expect("walk workflow");
        steps.push(current);
        match next {
            Some(next) => current = next.action_name.clone(),
            None => break,
        }
    }

    assert_eq!(
        steps,
        vec!["specify", "plan", "tasks", "implement", "review", "merge"]
    );
    assert_eq!(steps.len(), workflow.actions.len());
}

#[test]
fn registry_never_falls_back_to_a_default_workflow() {
    let default = workflow(vec![action("start", &[], true)], "start", 1);
    let registry = WorkflowRegistry::from_sequences([default]).expect("valid registry");

    let error = registry
        .resolve("solo-fast")
        .expect_err("unknown id must fail, even with a default present");
    match error {
        RuntimeError::UnknownWorkflow {
            workflow_id,
            available,
        } => {
            assert_eq!(workflow_id, "solo-fast");
            assert_eq!(available, vec!["test-flow"]);
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn registry_rejects_duplicate_workflow_ids() {
    let first = workflow(vec![action("start", &[], true)], "start", 1);
    let mut second = first.clone();
    second.description = "Other".to_string();

    let error = WorkflowRegistry::from_sequences([first, second]).expect_err("duplicates fail");
    assert!(
        matches!(error, RuntimeError::DuplicateWorkflow { .. }),
        "{error:?}"
    );
}

#[test]
fn duplicate_action_names_are_rejected() {
    let flow = WorkflowSequence {
        actions: vec![action("start", &[], true), action("start", &[], true)],
        ..workflow(vec![], "start", 1)
    };
    let error = flow.validate().expect_err("duplicate action");
    assert!(
        matches!(error, RuntimeError::DuplicateWorkflowAction { ref action, .. } if action == "start"),
        "{error:?}"
    );
}

#[test]
fn unknown_next_is_rejected() {
    let flow = workflow(vec![action("start", &["missing"], false)], "start", 1);
    let error = flow.validate().expect_err("unknown next");
    assert!(
        matches!(error, RuntimeError::UnknownWorkflowNext { ref next, .. } if next == "missing"),
        "{error:?}"
    );
}

#[test]
fn unknown_initial_is_rejected() {
    let flow = workflow(vec![action("start", &[], true)], "other", 1);
    let error = flow.validate().expect_err("unknown initial");
    assert!(
        matches!(error, RuntimeError::UnknownWorkflowInitial { ref initial, .. } if initial == "other"),
        "{error:?}"
    );
}

#[test]
fn branching_is_rejected_as_non_linear() {
    let flow = workflow(
        vec![
            action("start", &["a", "b"], false),
            action("a", &[], true),
            action("b", &[], true),
        ],
        "start",
        1,
    );
    let error = flow.validate().expect_err("branching is not v1 linear");
    assert!(
        matches!(error, RuntimeError::WorkflowNotLinear { ref action, count: 2, .. } if action == "start"),
        "{error:?}"
    );
}

#[test]
fn terminal_action_with_a_successor_is_rejected() {
    let flow = workflow(
        vec![action("start", &["end"], true), action("end", &[], true)],
        "start",
        1,
    );
    let error = flow.validate().expect_err("terminal+next");
    assert!(
        matches!(error, RuntimeError::WorkflowTerminalHasNext { ref action, .. } if action == "start"),
        "{error:?}"
    );
}

#[test]
fn implicit_terminal_and_empty_description_match_upstream() {
    let mut flow = workflow(vec![action("start", &[], false)], "start", 1);
    flow.actions[0].description.clear();
    flow.validate().unwrap();
    assert!(flow.next_action("start").unwrap().is_none());
}

#[test]
fn workflow_ids_cannot_be_paths() {
    for id in ["../../evil", "a/b", "Uppercase", "-leading", ""] {
        let mut flow = workflow(vec![action("start", &[], false)], "start", 1);
        flow.workflow_id = id.to_owned();
        assert!(flow.validate().is_err(), "{id}");
    }
}

#[test]
fn unreachable_actions_are_rejected() {
    let flow = workflow(
        vec![action("start", &[], true), action("island", &[], true)],
        "start",
        1,
    );
    let error = flow.validate().expect_err("unreachable island");
    match error {
        RuntimeError::UnreachableWorkflowActions { actions, .. } => {
            assert_eq!(actions, vec!["island"]);
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn cycle_is_rejected() {
    let flow = workflow(
        vec![
            action("start", &["next"], false),
            action("next", &["start"], false),
        ],
        "start",
        1,
    );
    let error = flow.validate().expect_err("cycle");
    assert!(
        matches!(error, RuntimeError::WorkflowCycle { .. }),
        "{error:?}"
    );
}

#[test]
fn empty_workflow_is_rejected() {
    let flow = workflow(vec![], "start", 1);
    let error = flow.validate().expect_err("empty workflow");
    assert!(
        matches!(error, RuntimeError::EmptyWorkflow { .. }),
        "{error:?}"
    );
}

#[test]
fn invalid_version_is_rejected() {
    let flow = workflow(vec![action("start", &[], true)], "start", 0);
    let error = flow.validate().expect_err("version zero");
    assert!(
        matches!(
            error,
            RuntimeError::InvalidWorkflowVersion { version: 0, .. }
        ),
        "{error:?}"
    );
}

#[test]
fn querying_an_unknown_current_action_is_an_error() {
    let flow = workflow(vec![action("start", &[], true)], "start", 1);
    let error = flow.next_action("not-there").expect_err("unknown action");
    assert!(
        matches!(error, RuntimeError::UnknownWorkflowAction { ref action, .. } if action == "not-there"),
        "{error:?}"
    );
}

#[test]
fn terminal_action_resolves_to_no_next_action() {
    let flow = workflow(
        vec![action("start", &["end"], false), action("end", &[], true)],
        "start",
        1,
    );
    assert_eq!(
        flow.next_action("start").unwrap().unwrap().action_name,
        "end"
    );
    assert!(flow.next_action("end").unwrap().is_none());
}

#[test]
fn workflow_file_loading_is_bounded_and_fail_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bad = dir.path().join("bad.workflow.yaml");
    std::fs::write(&bad, "workflow_id: broken\n").unwrap();
    let error = load_workflow(&bad).expect_err("incomplete workflow");
    assert!(matches!(error, RuntimeError::Malformed { .. }), "{error:?}");

    let oversized = dir.path().join("large.workflow.yaml");
    std::fs::write(
        &oversized,
        vec![b'#'; sk_runtime::MAX_WORKFLOW_BYTES as usize + 1],
    )
    .unwrap();
    let error = load_workflow(&oversized).expect_err("oversized workflow");
    assert!(matches!(error, RuntimeError::TooLarge { .. }), "{error:?}");
}
