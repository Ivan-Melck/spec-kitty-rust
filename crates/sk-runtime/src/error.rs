use std::io;
use std::path::PathBuf;

use thiserror::Error;

use crate::snapshot::ActorType;

/// Typed failures returned by the runtime library.
///
/// No public operation panics for malformed user input; every failure is one
/// of these variants. Error messages deliberately name the offending step or
/// action so an operator can repair the YAML without guessing.
#[derive(Debug, Error)]
pub enum RuntimeError {
    /// A template or workflow file could not be read.
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// The supplied path exists but is not a regular file.
    #[error("runtime input {path} is not a regular file")]
    NotAFile { path: PathBuf },

    /// A bounded parse rejected a file before reading it into memory.
    #[error("runtime input {path} is larger than the {limit}-byte limit")]
    TooLarge { path: PathBuf, limit: u64 },

    /// A template or workflow file is not valid UTF-8.
    #[error("runtime input {path} is not valid UTF-8")]
    NotUtf8 { path: PathBuf },

    /// YAML was syntactically invalid or did not match the schema.
    #[error("runtime input {path} is not a valid schema document: {message}")]
    Malformed { path: PathBuf, message: String },

    /// A mission template defines neither prompt steps nor audit steps.
    #[error("mission template has no steps")]
    EmptyTemplate,

    /// Mission metadata failed its non-empty-field checks.
    #[error("invalid mission metadata: {message}")]
    InvalidMission { message: String },

    /// A step failed a structural check.
    #[error("invalid step {step_id:?}: {message}")]
    InvalidStep { step_id: String, message: String },

    /// Step ids must be unique across `steps` and `audit_steps`.
    #[error("duplicate step id {id:?}")]
    DuplicateStepId { id: String },

    /// A `depends_on` list repeats the same dependency.
    #[error("step {step_id:?} repeats dependency {dependency:?}")]
    DuplicateDependency { step_id: String, dependency: String },

    /// A `depends_on` entry names a step that does not exist.
    #[error("step {step_id:?} depends on unknown step {dependency:?}")]
    UnknownDependency { step_id: String, dependency: String },

    /// The combined step/audit dependency graph contains a cycle.
    #[error("dependency cycle detected: {}", cycle.join(" -> "))]
    DependencyCycle { cycle: Vec<String> },

    /// A supplied RACI assignment makes a non-human accountable.
    #[error("step {step_id:?}: accountable must be human, got {actor_type}")]
    RaciAccountableNotHuman {
        step_id: String,
        actor_type: ActorType,
    },

    /// A RACI assignment and its override reason must be supplied together.
    #[error("step {step_id:?}: {message}")]
    RaciOverrideReason { step_id: String, message: String },

    /// A declared execution policy has not yet been implemented.
    #[error(
        "step {step_id:?} requires unsupported runtime policy {policy}; no decision was issued"
    )]
    UnsupportedPolicy {
        step_id: String,
        policy: &'static str,
    },

    /// An audit significance declaration is malformed.
    #[error("step {step_id:?}: invalid significance declaration: {message}")]
    InvalidSignificance { step_id: String, message: String },

    /// The run snapshot itself is internally inconsistent.
    #[error("invalid run snapshot: {message}")]
    InvalidRunSnapshot { message: String },

    /// A pending-decision map key does not equal the request's decision id.
    #[error("pending decision key {key:?} does not match request decision_id {decision_id:?}")]
    PendingDecisionMismatch { key: String, decision_id: String },

    /// A workflow id could not be resolved. Unknown ids never fall back.
    #[error("unknown workflow {workflow_id:?}; available workflows: {available:?}")]
    UnknownWorkflow {
        workflow_id: String,
        available: Vec<String>,
    },

    /// Two workflows share an id.
    #[error("duplicate workflow id {workflow_id:?}")]
    DuplicateWorkflow { workflow_id: String },

    /// A workflow contains no actions.
    #[error("workflow {workflow_id:?} has no actions")]
    EmptyWorkflow { workflow_id: String },

    /// A workflow version is below one.
    #[error("workflow {workflow_id:?} has invalid version {version}; expected >= 1")]
    InvalidWorkflowVersion { workflow_id: String, version: u32 },

    /// A workflow action failed a structural check.
    #[error("workflow {workflow_id:?}: invalid action {action:?}: {message}")]
    InvalidWorkflowAction {
        workflow_id: String,
        action: String,
        message: String,
    },

    /// A workflow repeats an action name.
    #[error("workflow {workflow_id:?} repeats action {action:?}")]
    DuplicateWorkflowAction { workflow_id: String, action: String },

    /// A workflow action names more than one successor.
    #[error("workflow {workflow_id:?} is not linear: action {action:?} has {count} successors")]
    WorkflowNotLinear {
        workflow_id: String,
        action: String,
        count: usize,
    },

    /// `terminal: true` and `next` were both supplied.
    #[error("workflow {workflow_id:?}: terminal action {action:?} must not name a successor")]
    WorkflowTerminalHasNext { workflow_id: String, action: String },

    /// A non-terminal action has no successor.
    #[error("workflow {workflow_id:?}: non-terminal action {action:?} has no successor")]
    WorkflowNonTerminalDeadEnd { workflow_id: String, action: String },

    /// A workflow `next` entry names an action that does not exist.
    #[error("workflow {workflow_id:?}: action {action:?} names unknown successor {next:?}")]
    UnknownWorkflowNext {
        workflow_id: String,
        action: String,
        next: String,
    },

    /// A workflow `initial` names an action that does not exist.
    #[error("workflow {workflow_id:?}: initial action {initial:?} does not exist")]
    UnknownWorkflowInitial {
        workflow_id: String,
        initial: String,
    },

    /// The workflow graph contains a cycle.
    #[error("workflow {workflow_id:?} contains a cycle: {}", cycle.join(" -> "))]
    WorkflowCycle {
        workflow_id: String,
        cycle: Vec<String>,
    },

    /// Some workflow actions are not reachable from `initial`.
    #[error("workflow {workflow_id:?} has unreachable actions: {actions:?}")]
    UnreachableWorkflowActions {
        workflow_id: String,
        actions: Vec<String>,
    },

    /// A workflow has no terminal action.
    #[error("workflow {workflow_id:?} has no terminal action")]
    WorkflowHasNoTerminal { workflow_id: String },

    /// A query named a current action that is not in the workflow.
    #[error("workflow {workflow_id:?} has no action {action:?}; available actions: {available:?}")]
    UnknownWorkflowAction {
        workflow_id: String,
        action: String,
        available: Vec<String>,
    },

    /// An advance was planned from a different issued step than the live run.
    #[error(
        "run {run_id:?} issued step changed (expected {expected:?}, found {found:?}); plan again"
    )]
    StaleIssuedStep {
        run_id: String,
        expected: Option<String>,
        found: Option<String>,
    },

    /// A planned advance no longer matches the live run snapshot.
    #[error("run {run_id:?} changed after the advance was planned; plan again")]
    StaleRunState { run_id: String },
}
