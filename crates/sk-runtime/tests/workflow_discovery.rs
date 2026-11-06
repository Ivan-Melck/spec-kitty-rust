//! Workflow discovery, composition and action planning.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;
use sk_runtime::{
    DEFAULT_WORKFLOW_ID, WorkflowSearch, compose_template_with_workflow, list_available_workflows,
    load_workflow_by_id, plan_next_workflow_action, resolve_mission_workflow,
    resolve_workflow_path, validate_workflow_id, workflow_candidate_paths,
};

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, text).unwrap();
}

fn workflow_yaml(id: &str, actions: &[(&str, &str, bool)], initial: &str) -> String {
    let actions = actions
        .iter()
        .map(|(name, next, terminal)| {
            let next = if next.is_empty() {
                String::new()
            } else {
                format!("    next: [\"{next}\"]\n")
            };
            format!(
                "  - action_name: {name}\n    description: Do {name}.\n    terminal: {terminal}\n{next}"
            )
        })
        .collect::<String>();
    format!(
        "workflow_id: {id}\ndescription: A workflow.\ninitial: {initial}\nversion: 1\nactions:\n{actions}"
    )
}

struct Fixture {
    _root: tempfile::TempDir,
    search: WorkflowSearch,
}

fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let shipped = root.path().join("shipped");
    let project = root.path().join("project");
    write(
        &shipped.join("software-dev-default.workflow.yaml"),
        &workflow_yaml(
            DEFAULT_WORKFLOW_ID,
            &[("specify", "plan", false), ("plan", "", true)],
            "specify",
        ),
    );
    write(
        &shipped.join("broken.workflow.yaml"),
        "workflow_id: broken\nactions: []\n",
    );
    write(
        &project.join(".kittify/overrides/workflows/software-dev-default.workflow.yaml"),
        &workflow_yaml(DEFAULT_WORKFLOW_ID, &[("specify", "", true)], "specify"),
    );
    Fixture {
        search: WorkflowSearch::shipped([shipped]).with_project_root(&project),
        _root: root,
    }
}

#[test]
fn workflow_ids_reject_path_spellings() {
    for bad in ["../evil", "Upper", "with space", "-leading", "", "a/b"] {
        assert!(
            validate_workflow_id(bad).is_err(),
            "{bad:?} must be refused"
        );
    }
    for good in ["software-dev-default", "a1", "x-y"] {
        validate_workflow_id(good).unwrap();
    }
    assert_eq!(
        workflow_candidate_paths("../evil", &WorkflowSearch::default())
            .unwrap_err()
            .to_string(),
        "invalid workflow id \"../evil\": must match [a-z0-9][a-z0-9-]* (no path traversal, uppercase, spaces, or special characters)"
    );
}

#[test]
fn project_overrides_win_over_shipped_workflows() {
    let fixture = fixture();
    let workflow = load_workflow_by_id(DEFAULT_WORKFLOW_ID, &fixture.search).unwrap();
    assert_eq!(workflow.actions.len(), 1, "the override has one action");

    let plain = WorkflowSearch::shipped(fixture.search.shipped_roots.clone());
    let shipped = load_workflow_by_id(DEFAULT_WORKFLOW_ID, &plain).unwrap();
    assert_eq!(shipped.actions.len(), 2);

    let resolved = resolve_workflow_path(DEFAULT_WORKFLOW_ID, &fixture.search).unwrap();
    assert!(resolved.starts_with(fixture.search.override_root().unwrap()));
}

#[test]
fn a_plain_yaml_override_is_the_second_candidate() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    write(
        &project.join(".kittify/overrides/workflows/custom.yaml"),
        &workflow_yaml("custom", &[("only", "", true)], "only"),
    );
    let search = WorkflowSearch::shipped(Vec::new()).with_project_root(&project);
    let workflow = load_workflow_by_id("custom", &search).unwrap();
    assert_eq!(workflow.action_names(), vec!["only".to_string()]);
}

#[test]
fn unknown_workflow_ids_never_fall_back() {
    let fixture = fixture();
    let error = load_workflow_by_id("unknown-flow", &fixture.search).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "unknown workflow \"unknown-flow\"; available workflows: [\"{DEFAULT_WORKFLOW_ID}\"]"
        )
    );

    let error = resolve_mission_workflow(Some("missing-flow"), &fixture.search).unwrap_err();
    assert!(matches!(
        error,
        sk_runtime::RuntimeError::UnknownWorkflow { .. }
    ));
}

#[test]
fn a_declared_default_is_used_only_when_no_workflow_is_declared() {
    let fixture = fixture();
    assert_eq!(
        resolve_mission_workflow(None, &fixture.search)
            .unwrap()
            .workflow_id,
        DEFAULT_WORKFLOW_ID
    );
    assert_eq!(
        resolve_mission_workflow(Some(DEFAULT_WORKFLOW_ID), &fixture.search)
            .unwrap()
            .workflow_id,
        DEFAULT_WORKFLOW_ID
    );
}

#[test]
fn a_workflow_file_declaring_a_different_id_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let shipped = root.path().join("shipped");
    write(
        &shipped.join("requested.workflow.yaml"),
        &workflow_yaml("different", &[("only", "", true)], "only"),
    );
    let search = WorkflowSearch::shipped([shipped]);
    let error = load_workflow_by_id("requested", &search).unwrap_err();
    assert!(matches!(
        error,
        sk_runtime::RuntimeError::WorkflowIdMismatch { .. }
    ));
}

#[test]
fn discovery_lists_only_valid_workflows_and_skips_malformed_files() {
    let fixture = fixture();
    // The override replaces the shipped default (same id) and `broken` is
    // malformed, so only the default id is listed.
    assert_eq!(
        list_available_workflows(&fixture.search),
        vec![DEFAULT_WORKFLOW_ID.to_string()]
    );
}

#[test]
fn action_planning_is_terminal_at_the_last_action() {
    let fixture = fixture();
    let workflow = load_workflow_by_id(
        DEFAULT_WORKFLOW_ID,
        &WorkflowSearch::shipped(fixture.search.shipped_roots.clone()),
    )
    .unwrap();

    let plan = plan_next_workflow_action(&workflow, "specify").unwrap();
    assert_eq!(plan.next_action.as_deref(), Some("plan"));
    assert_eq!(plan.workflow_id, DEFAULT_WORKFLOW_ID);

    let plan = plan_next_workflow_action(&workflow, "plan").unwrap();
    assert_eq!(plan.next_action, None);

    let error = plan_next_workflow_action(&workflow, "review").unwrap_err();
    assert!(
        error.to_string().contains("has no action \"review\""),
        "{error}"
    );
}

#[test]
fn template_composition_follows_the_workflow_and_keeps_metadata() {
    let template: sk_runtime::MissionTemplate = serde_json::from_value(json!({
        "mission": {"key": "m", "name": "M", "version": "1.0.0", "description": ""},
        "steps": [
            {"id": "discovery", "title": "Discovery", "description": "d", "prompt": "discover",
             "requires_inputs": ["topic"], "depends_on": ["legacy"]},
            {"id": "specify", "title": "Specify", "description": "old", "prompt": "spec",
             "agent_profile": "source-profile"}
        ],
        "audit_steps": []
    }))
    .unwrap();
    let workflow: sk_runtime::WorkflowSequence = serde_yaml_ng::from_str(&workflow_yaml(
        "software-dev-default",
        &[
            ("specify", "plan", false),
            ("plan", "tasks", false),
            ("tasks", "", true),
        ],
        "specify",
    ))
    .unwrap();

    let composed = compose_template_with_workflow(&template, &workflow);
    let ids: Vec<&str> = composed.steps.iter().map(|step| step.id.as_str()).collect();
    assert_eq!(ids, vec!["discovery", "specify", "plan", "tasks"]);

    let discovery = &composed.steps[0];
    assert_eq!(discovery.depends_on, Vec::<String>::new());
    assert_eq!(discovery.requires_inputs, vec!["topic".to_string()]);

    let specify = &composed.steps[1];
    assert_eq!(specify.depends_on, vec!["discovery".to_string()]);
    assert_eq!(specify.prompt.as_deref(), Some("spec"));
    // The workflow's action description wins when non-empty.
    assert_eq!(specify.description, "Do specify.");
    assert_eq!(specify.agent_profile.as_deref(), Some("source-profile"));

    let plan = &composed.steps[2];
    assert_eq!(plan.title, "Plan");
    assert_eq!(tasks_title(&composed), "Tasks");
    assert_eq!(plan.prompt_template.as_deref(), Some("plan.md"));
    assert_eq!(plan.depends_on, vec!["specify".to_string()]);

    let tasks = &composed.steps[3];
    assert_eq!(tasks.depends_on, vec!["plan".to_string()]);
    composed.validate().unwrap();
}

fn tasks_title(composed: &sk_runtime::MissionTemplate) -> String {
    composed
        .steps
        .iter()
        .find(|step| step.id == "tasks")
        .expect("tasks step")
        .title
        .clone()
}

#[test]
fn composition_omits_the_discovery_prefix_when_the_workflow_names_it() {
    let template: sk_runtime::MissionTemplate = serde_json::from_value(json!({
        "mission": {"key": "m", "name": "M", "version": "1.0.0", "description": ""},
        "steps": [{"id": "discovery", "title": "Discovery", "description": "d"}],
        "audit_steps": []
    }))
    .unwrap();
    let workflow: sk_runtime::WorkflowSequence = serde_yaml_ng::from_str(&workflow_yaml(
        "custom",
        &[("discovery", "specify", false), ("specify", "", true)],
        "discovery",
    ))
    .unwrap();

    let composed = compose_template_with_workflow(&template, &workflow);
    let ids: Vec<&str> = composed.steps.iter().map(|step| step.id.as_str()).collect();
    assert_eq!(ids, vec!["discovery", "specify"]);
    assert_eq!(composed.steps[0].depends_on, Vec::<String>::new());
}

#[test]
fn hyphenated_action_titles_use_spaces_like_upstream() {
    let template: sk_runtime::MissionTemplate = serde_json::from_value(json!({
        "mission": {"key": "m", "name": "M", "version": "1.0.0", "description": ""},
        "steps": [],
        "audit_steps": []
    }))
    .unwrap();
    let workflow: sk_runtime::WorkflowSequence = serde_yaml_ng::from_str(&workflow_yaml(
        "custom",
        &[("mission-review", "", true)],
        "mission-review",
    ))
    .unwrap();
    let composed = compose_template_with_workflow(&template, &workflow);
    assert_eq!(composed.steps[0].title, "Mission Review");
}

#[test]
fn composition_uses_human_in_the_loop_as_a_required_input() {
    let template: sk_runtime::MissionTemplate = serde_json::from_value(json!({
        "mission": {"key": "m", "name": "M", "version": "1.0.0", "description": ""},
        "steps": [],
        "audit_steps": []
    }))
    .unwrap();
    let yaml = r#"
workflow_id: custom
description: A workflow.
initial: accept
version: 1
actions:
  - action_name: accept
    description: Accept.
    terminal: true
    human_in_the_loop: operator_approval
"#;
    let workflow: sk_runtime::WorkflowSequence = serde_yaml_ng::from_str(yaml).unwrap();
    let composed = compose_template_with_workflow(&template, &workflow);
    assert_eq!(
        composed.steps[0].requires_inputs,
        vec!["operator_approval".to_string()]
    );
}

#[test]
fn a_missing_shipped_root_is_an_unknown_workflow_not_a_panic() {
    let search = WorkflowSearch::shipped([PathBuf::from("/nonexistent/workflows")]);
    let error = load_workflow_by_id(DEFAULT_WORKFLOW_ID, &search).unwrap_err();
    assert!(matches!(
        error,
        sk_runtime::RuntimeError::UnknownWorkflow { .. }
    ));
    assert!(list_available_workflows(&search).is_empty());
}
