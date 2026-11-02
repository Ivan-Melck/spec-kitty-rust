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
