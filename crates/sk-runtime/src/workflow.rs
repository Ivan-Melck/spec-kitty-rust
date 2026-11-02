//! Workflow sequence schema, validation, and a no-fallback registry.
//!
//! Workflows are the public action sequence (`specify -> plan -> ...`). They
//! are validated independently of mission templates because they have their
//! own invariants: linearity, reachability, acyclicity, and a terminal action.
//! Unknown workflow ids are always an error; this module never substitutes a
//! default workflow.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::RuntimeError;
use crate::template::read_bounded_utf8;

/// Hard upper bound for a workflow file.
pub const MAX_WORKFLOW_BYTES: u64 = 512 * 1024;

/// One action in a workflow sequence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionStep {
    pub action_name: String,
    #[serde(default)]
    pub next: Vec<String>,
    pub description: String,
    #[serde(default)]
    pub terminal: bool,
    #[serde(default)]
    pub agent_profile: Option<String>,
    #[serde(default)]
    pub human_in_the_loop: Option<String>,
    #[serde(default)]
    pub integration: Option<String>,
    #[serde(default)]
    pub outputs: BTreeMap<String, String>,
}

/// A validated, linear workflow graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowSequence {
    pub workflow_id: String,
    pub description: String,
    pub actions: Vec<ActionStep>,
    pub initial: String,
    pub version: u32,
    #[serde(default)]
    pub integrations: BTreeMap<String, Value>,
}

impl WorkflowSequence {
    /// Validate the workflow graph.
    ///
    /// Rejects empty workflows, version < 1, duplicate action names, multiple
    /// successors, terminal/next contradictions, unknown `initial`/`next`
    /// references, cycles and unreachable actions. A final action with no
    /// successors is valid even without an explicit `terminal: true` flag.
    pub fn validate(&self) -> Result<(), RuntimeError> {
        let workflow_id = self.workflow_id.clone();
        if workflow_id.is_empty()
            || !workflow_id
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            || workflow_id.starts_with('-')
        {
            return Err(RuntimeError::InvalidWorkflowAction {
                workflow_id,
                action: String::new(),
                message: "workflow_id must match [a-z0-9][a-z0-9-]*".to_string(),
            });
        }
        if self.version < 1 {
            return Err(RuntimeError::InvalidWorkflowVersion {
                workflow_id,
                version: self.version,
            });
        }
        if self.actions.is_empty() {
            return Err(RuntimeError::EmptyWorkflow { workflow_id });
        }

        let mut by_name: BTreeMap<&str, &ActionStep> = BTreeMap::new();
        for action in &self.actions {
            if action.action_name.trim().is_empty() {
                return Err(RuntimeError::InvalidWorkflowAction {
                    workflow_id: workflow_id.clone(),
                    action: action.action_name.clone(),
                    message: "action_name must be non-empty".to_string(),
                });
            }
            if by_name
                .insert(action.action_name.as_str(), action)
                .is_some()
            {
                return Err(RuntimeError::DuplicateWorkflowAction {
                    workflow_id: workflow_id.clone(),
                    action: action.action_name.clone(),
                });
            }
            if action.next.len() > 1 {
                return Err(RuntimeError::WorkflowNotLinear {
                    workflow_id: workflow_id.clone(),
                    action: action.action_name.clone(),
                    count: action.next.len(),
                });
            }
            if action.terminal && !action.next.is_empty() {
                return Err(RuntimeError::WorkflowTerminalHasNext {
                    workflow_id: workflow_id.clone(),
                    action: action.action_name.clone(),
                });
            }
        }

        if !by_name.contains_key(self.initial.as_str()) {
            return Err(RuntimeError::UnknownWorkflowInitial {
                workflow_id: workflow_id.clone(),
                initial: self.initial.clone(),
            });
        }

        for action in &self.actions {
            for next in &action.next {
                if !by_name.contains_key(next.as_str()) {
                    return Err(RuntimeError::UnknownWorkflowNext {
                        workflow_id: workflow_id.clone(),
                        action: action.action_name.clone(),
                        next: next.clone(),
                    });
                }
            }
        }

        detect_workflow_cycle(&workflow_id, &by_name, &self.initial)?;

        let mut visited = BTreeSet::new();
        mark_reachable(&by_name, &self.initial, &mut visited);
        let unreachable: Vec<String> = by_name
            .keys()
            .filter(|name| !visited.contains(**name))
            .map(|name| (*name).to_string())
            .collect();
        if !unreachable.is_empty() {
            return Err(RuntimeError::UnreachableWorkflowActions {
                workflow_id,
                actions: unreachable,
            });
        }

        Ok(())
    }

    /// Resolve the action after `current_action`.
    ///
    /// Returns `Ok(None)` at a terminal action. An unknown current action is a
    /// typed error (there is no implicit reset or fallback).
    pub fn next_action(&self, current_action: &str) -> Result<Option<&ActionStep>, RuntimeError> {
        self.validate()?;
        let action = self
            .actions
            .iter()
            .find(|action| action.action_name == current_action)
            .ok_or_else(|| RuntimeError::UnknownWorkflowAction {
                workflow_id: self.workflow_id.clone(),
                action: current_action.to_string(),
                available: self.action_names(),
            })?;
        if action.terminal || action.next.is_empty() {
            return Ok(None);
        }
        let next = action.next.first().expect("validated non-terminal action");
        self.actions
            .iter()
            .find(|candidate| &candidate.action_name == next)
            .map(Some)
            .ok_or_else(|| RuntimeError::UnknownWorkflowNext {
                workflow_id: self.workflow_id.clone(),
                action: action.action_name.clone(),
                next: next.clone(),
            })
    }

    /// All action names in template order.
    pub fn action_names(&self) -> Vec<String> {
        self.actions
            .iter()
            .map(|action| action.action_name.clone())
            .collect()
    }
}

/// Load and validate one workflow YAML file with a bounded read.
pub fn load_workflow(path: &Path) -> Result<WorkflowSequence, RuntimeError> {
    let text = read_bounded_utf8(path, MAX_WORKFLOW_BYTES)?;
    let workflow: WorkflowSequence =
        serde_yaml_ng::from_str(&text).map_err(|error| RuntimeError::Malformed {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
    workflow.validate()?;
    Ok(workflow)
}

/// An in-memory workflow registry with no default fallback.
#[derive(Debug, Clone, Default)]
pub struct WorkflowRegistry {
    workflows: BTreeMap<String, WorkflowSequence>,
}

impl WorkflowRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a registry from validated workflows, rejecting duplicate ids.
    pub fn from_sequences(
        workflows: impl IntoIterator<Item = WorkflowSequence>,
    ) -> Result<Self, RuntimeError> {
        let mut registry = Self::new();
        for workflow in workflows {
            registry.insert(workflow)?;
        }
        Ok(registry)
    }

    /// Validate and insert one workflow.
    pub fn insert(&mut self, workflow: WorkflowSequence) -> Result<(), RuntimeError> {
        workflow.validate()?;
        let workflow_id = workflow.workflow_id.clone();
        if self.workflows.contains_key(&workflow_id) {
            return Err(RuntimeError::DuplicateWorkflow { workflow_id });
        }
        self.workflows.insert(workflow_id, workflow);
        Ok(())
    }

    /// Resolve a workflow id. Unknown ids are an error, never a fallback.
    pub fn resolve(&self, workflow_id: &str) -> Result<&WorkflowSequence, RuntimeError> {
        self.workflows
            .get(workflow_id)
            .ok_or_else(|| RuntimeError::UnknownWorkflow {
                workflow_id: workflow_id.to_string(),
                available: self.available(),
            })
    }

    /// Sorted ids currently registered.
    pub fn available(&self) -> Vec<String> {
        self.workflows.keys().cloned().collect()
    }

    /// Number of registered workflows.
    pub fn len(&self) -> usize {
        self.workflows.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.workflows.is_empty()
    }
}

fn detect_workflow_cycle(
    workflow_id: &str,
    by_name: &BTreeMap<&str, &ActionStep>,
    initial: &str,
) -> Result<(), RuntimeError> {
    fn visit(
        workflow_id: &str,
        name: &str,
        by_name: &BTreeMap<&str, &ActionStep>,
        visiting: &mut BTreeSet<String>,
        visited: &mut BTreeSet<String>,
        stack: &mut Vec<String>,
    ) -> Result<(), RuntimeError> {
        if visited.contains(name) {
            return Ok(());
        }
        if !visiting.insert(name.to_string()) {
            let start = stack.iter().position(|item| item == name).unwrap_or(0);
            let mut cycle = stack[start..].to_vec();
            cycle.push(name.to_string());
            return Err(RuntimeError::WorkflowCycle {
                workflow_id: workflow_id.to_string(),
                cycle,
            });
        }
        stack.push(name.to_string());
        if let Some(action) = by_name.get(name) {
            for next in &action.next {
                if by_name.contains_key(next.as_str()) {
                    visit(workflow_id, next, by_name, visiting, visited, stack)?;
                }
            }
        }
        stack.pop();
        visiting.remove(name);
        visited.insert(name.to_string());
        Ok(())
    }

    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    let mut stack = Vec::new();
    visit(
        workflow_id,
        initial,
        by_name,
        &mut visiting,
        &mut visited,
        &mut stack,
    )
}

fn mark_reachable(
    by_name: &BTreeMap<&str, &ActionStep>,
    name: &str,
    visited: &mut BTreeSet<String>,
) {
    if !visited.insert(name.to_string()) {
        return;
    }
    if let Some(action) = by_name.get(name) {
        for next in &action.next {
            mark_reachable(by_name, next, visited);
        }
    }
}

/// The permanent default workflow id used when a mission declares none.
pub const DEFAULT_WORKFLOW_ID: &str = "software-dev-default";

/// The search roots for workflow discovery.
///
/// Project overrides live under `<project_root>/.kittify/overrides/workflows/`;
/// the shipped roots are supplied by the caller because pack-root resolution is
/// owned by `sk-charter`, not this crate. Candidates are, in order:
///
/// 1. `<project_root>/.kittify/overrides/workflows/<id>.workflow.yaml`
/// 2. `<project_root>/.kittify/overrides/workflows/<id>.yaml`
/// 3. `<shipped_root>/<id>.workflow.yaml` for each shipped root, in order
///
/// Unknown ids are always an error; there is no silent fallback.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkflowSearch {
    pub project_root: Option<std::path::PathBuf>,
    pub shipped_roots: Vec<std::path::PathBuf>,
}

impl WorkflowSearch {
    /// A search over shipped roots only (no project overrides).
    pub fn shipped(roots: impl IntoIterator<Item = std::path::PathBuf>) -> Self {
        Self {
            project_root: None,
            shipped_roots: roots.into_iter().collect(),
        }
    }

    /// The same search with a project root for overrides.
    pub fn with_project_root(mut self, project_root: impl Into<std::path::PathBuf>) -> Self {
        self.project_root = Some(project_root.into());
        self
    }

    /// Override directory, when a project root is configured.
    pub fn override_root(&self) -> Option<std::path::PathBuf> {
        self.project_root
            .as_ref()
            .map(|root| root.join(".kittify").join("overrides").join("workflows"))
    }
}

/// Reject workflow ids that cannot be safely interpolated into a path.
pub fn validate_workflow_id(workflow_id: &str) -> Result<(), RuntimeError> {
    let valid = !workflow_id.is_empty()
        && !workflow_id.starts_with('-')
        && workflow_id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if valid {
        Ok(())
    } else {
        Err(RuntimeError::InvalidWorkflowId {
            workflow_id: workflow_id.to_string(),
        })
    }
}

/// Candidate file paths for a workflow id, in precedence order.
pub fn workflow_candidate_paths(
    workflow_id: &str,
    search: &WorkflowSearch,
) -> Result<Vec<std::path::PathBuf>, RuntimeError> {
    validate_workflow_id(workflow_id)?;
    let mut candidates = Vec::new();
    if let Some(override_root) = search.override_root() {
        candidates.push(override_root.join(format!("{workflow_id}.workflow.yaml")));
        candidates.push(override_root.join(format!("{workflow_id}.yaml")));
    }
    for root in &search.shipped_roots {
        candidates.push(root.join(format!("{workflow_id}.workflow.yaml")));
    }
    Ok(candidates)
}

/// Resolve a workflow id to its source YAML path.
///
/// Raises [`RuntimeError::UnknownWorkflow`] naming the id and the currently
/// available ids; it never falls back to the default workflow.
pub fn resolve_workflow_path(
    workflow_id: &str,
    search: &WorkflowSearch,
) -> Result<std::path::PathBuf, RuntimeError> {
    for candidate in workflow_candidate_paths(workflow_id, search)? {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(RuntimeError::UnknownWorkflow {
        workflow_id: workflow_id.to_string(),
        available: list_available_workflows(search),
    })
}

/// Resolve, load and validate a workflow by id.
///
/// The file must declare the requested id; a file that declares a different id
/// is a typed mismatch rather than a silently accepted alias.
pub fn load_workflow_by_id(
    workflow_id: &str,
    search: &WorkflowSearch,
) -> Result<WorkflowSequence, RuntimeError> {
    let path = resolve_workflow_path(workflow_id, search)?;
    let workflow = load_workflow(&path)?;
    if workflow.workflow_id != workflow_id {
        return Err(RuntimeError::WorkflowIdMismatch {
            path,
            declared: workflow.workflow_id,
            requested: workflow_id.to_string(),
        });
    }
    Ok(workflow)
}

/// Sorted ids of every workflow resolvable from the search roots.
///
/// An unreadable or invalid file is simply not listed; discovery never fails
/// the whole lookup because one unrelated file is malformed.
pub fn list_available_workflows(search: &WorkflowSearch) -> Vec<String> {
    let mut roots: Vec<&std::path::Path> = Vec::new();
    let override_root;
    if let Some(root) = search.override_root() {
        override_root = root;
        roots.push(&override_root);
    }
    for root in &search.shipped_roots {
        roots.push(root);
    }

    let mut available: Vec<String> = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !name.ends_with(".yaml") {
                continue;
            }
            let id = name
                .strip_suffix(".workflow.yaml")
                .or_else(|| name.strip_suffix(".yaml"))
                .unwrap_or(name);
            if validate_workflow_id(id).is_err() {
                continue;
            }
            match load_workflow(&path) {
                Ok(workflow) if workflow.workflow_id == id => available.push(id.to_string()),
                _ => continue,
            }
        }
    }
    available.sort_unstable();
    available.dedup();
    available
}

/// Resolve the workflow a mission declares, defaulting only when it declares
/// none.
///
/// `declared = None` selects [`DEFAULT_WORKFLOW_ID`] (the upstream NEW-2
/// permanent default). A declared but unknown id propagates the typed
/// unknown-workflow error.
pub fn resolve_mission_workflow(
    declared: Option<&str>,
    search: &WorkflowSearch,
) -> Result<WorkflowSequence, RuntimeError> {
    load_workflow_by_id(declared.unwrap_or(DEFAULT_WORKFLOW_ID), search)
}

/// The resolved next action of a workflow sequence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowActionPlan {
    pub current_action: String,
    pub next_action: Option<String>,
    pub workflow_id: String,
}

/// Resolve the action that follows `current_action` in `workflow`.
///
/// `next_action` is `None` at a terminal action. An unknown current action is a
/// typed error; there is no implicit reset.
pub fn plan_next_workflow_action(
    workflow: &WorkflowSequence,
    current_action: &str,
) -> Result<WorkflowActionPlan, RuntimeError> {
    let next = workflow.next_action(current_action)?;
    Ok(WorkflowActionPlan {
        current_action: current_action.to_string(),
        next_action: next.map(|action| action.action_name.clone()),
        workflow_id: workflow.workflow_id.clone(),
    })
}

/// Return a runtime template whose steps follow `workflow`.
///
/// The workflow artifact owns public action ordering; the runtime needs a
/// mission-template DAG. Existing step metadata is preserved when an action
/// matches a template step. A leading `discovery` step is kept when the
/// workflow starts at `specify` and does not name `discovery` itself, exactly
/// as upstream.
pub fn compose_template_with_workflow(
    template: &crate::template::MissionTemplate,
    workflow: &WorkflowSequence,
) -> crate::template::MissionTemplate {
    use std::collections::BTreeMap;

    let by_id: BTreeMap<&str, &crate::template::PromptStep> = template
        .steps
        .iter()
        .map(|step| (step.id.as_str(), step))
        .collect();
    let action_names: Vec<&str> = workflow
        .actions
        .iter()
        .map(|action| action.action_name.as_str())
        .collect();

    let mut composed: Vec<crate::template::PromptStep> = Vec::new();
    let mut previous_id: Option<String> = None;

    if workflow.initial == "specify"
        && let Some(discovery) = by_id.get("discovery")
        && !action_names.contains(&"discovery")
    {
        composed.push(copy_prompt_step(discovery, Some(Vec::new())));
        previous_id = Some("discovery".to_string());
    }

    for action in &workflow.actions {
        let depends_on: Vec<String> = previous_id.iter().cloned().collect();
        let step = match by_id.get(action.action_name.as_str()) {
            Some(source) => {
                let mut step = copy_prompt_step(source, Some(depends_on));
                step.description = if action.description.is_empty() {
                    source.description.clone()
                } else {
                    action.description.clone()
                };
                step.agent_profile = action
                    .agent_profile
                    .clone()
                    .filter(|profile| !profile.is_empty())
                    .or_else(|| source.agent_profile.clone());
                step
            }
            None => crate::template::PromptStep {
                id: action.action_name.clone(),
                title: title_case(&action.action_name),
                description: action.description.clone(),
                prompt: None,
                prompt_template: Some(format!("{}.md", action.action_name)),
                expected_output: None,
                requires_inputs: action
                    .human_in_the_loop
                    .iter()
                    .cloned()
                    .collect::<Vec<String>>(),
                depends_on,
                raci: None,
                raci_override_reason: None,
                agent_profile: action.agent_profile.clone(),
                contract_ref: None,
            },
        };
        composed.push(step);
        previous_id = Some(action.action_name.clone());
    }

    crate::template::MissionTemplate {
        mission: template.mission.clone(),
        steps: composed,
        audit_steps: template.audit_steps.clone(),
    }
}

fn copy_prompt_step(
    source: &crate::template::PromptStep,
    depends_on: Option<Vec<String>>,
) -> crate::template::PromptStep {
    crate::template::PromptStep {
        id: source.id.clone(),
        title: source.title.clone(),
        description: source.description.clone(),
        prompt: source.prompt.clone(),
        prompt_template: source.prompt_template.clone(),
        expected_output: source.expected_output.clone(),
        requires_inputs: source.requires_inputs.clone(),
        depends_on: depends_on.unwrap_or_else(|| source.depends_on.clone()),
        raci: source.raci.clone(),
        raci_override_reason: source.raci_override_reason.clone(),
        agent_profile: source.agent_profile.clone(),
        contract_ref: source.contract_ref.clone(),
    }
}

fn title_case(name: &str) -> String {
    // Upstream: ``action_name.replace("-", " ").title()``.
    name.split('-')
        .map(|part| {
            let mut characters = part.chars();
            match characters.next() {
                Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<String>>()
        .join(" ")
}
