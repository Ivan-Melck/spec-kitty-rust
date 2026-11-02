//! Differential goldens for workflow discovery and template/workflow composition.
//!
//! Captured from the pinned Python runtime by
//! `tests/oracle/generate_goldens.py`. These cover the reader-owned workflow
//! registry and the pure `compose_template_with_workflow` adapter, which are
//! separate from the planning/answer goldens in `golden_differential.rs`.

use std::path::PathBuf;

use serde_json::{Value, json};
use sk_runtime::{
    MissionTemplate, WorkflowSearch, WorkflowSequence, compose_template_with_workflow,
    list_available_workflows, load_workflow_by_id, resolve_workflow_path,
};

fn golden(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
        .join(format!("{name}.json"));
    serde_json::from_str(
        &std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display())),
    )
    .expect("golden is JSON")
}

fn repo_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

fn project_step(step: &sk_runtime::PromptStep) -> Value {
    json!({
        "id": step.id,
        "title": step.title,
        "description": step.description,
        "prompt": step.prompt,
        "prompt_template": step.prompt_template,
        "expected_output": step.expected_output,
        "requires_inputs": step.requires_inputs,
        "depends_on": step.depends_on,
        "raci": step.raci,
        "raci_override_reason": step.raci_override_reason,
        "agent_profile": step.agent_profile,
        "contract_ref": step.contract_ref,
    })
}

fn compose_matches(name: &str) {
    let golden = golden(name);
    let scenario = &golden["scenario"];
    let expected = &golden["expected"];

    let template: MissionTemplate =
        serde_json::from_value(scenario["template"].clone()).expect("template deserializes");
    let workflow: WorkflowSequence =
        serde_json::from_value(scenario["workflow"].clone()).expect("workflow deserializes");

    let composed = compose_template_with_workflow(&template, &workflow);
    composed.validate().expect("composed template is valid");

    let actual_steps: Vec<Value> = composed.steps.iter().map(project_step).collect();
    assert_eq!(
        Value::Array(actual_steps),
        expected["steps"],
        "composed steps drift in {name}"
    );
    assert_eq!(
        json!(composed.audit_steps),
        expected["audit_steps"],
        "audit steps drift in {name}"
    );
    assert_eq!(
        serde_json::to_value(&composed.mission).unwrap(),
        expected["mission"],
        "mission metadata drift in {name}"
    );
}

#[test]
fn composition_goldens_match_the_python_reference() {
    compose_matches("compose_virtual_discovery_prefix");
    compose_matches("compose_named_discovery_action");
}

#[test]
fn shipped_workflow_discovery_matches_the_python_reference() {
    let golden = golden("workflow_shipped_default");
    let expected = &golden["expected"];
    let workflows_root = repo_path("src/charter/offering/workflows");
    let search =
        WorkflowSearch::shipped([workflows_root.join("_fixtures"), workflows_root.clone()]);

    assert_eq!(
        json!(list_available_workflows(&search)),
        expected["available"],
        "available workflow ids drift"
    );

    let workflow = load_workflow_by_id("software-dev-default", &search).expect("default workflow");
    assert_eq!(
        serde_json::to_value(&workflow).unwrap(),
        expected["workflow"],
        "shipped default workflow drift"
    );

    let resolved = resolve_workflow_path("software-dev-default", &search).unwrap();
    let suffix: Vec<String> = resolved
        .components()
        .rev()
        .take(2)
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<String>>()
        .into_iter()
        .rev()
        .collect();
    assert_eq!(
        suffix.join("/"),
        expected["path_suffix"].as_str().unwrap(),
        "resolved path precedence drift"
    );
}
