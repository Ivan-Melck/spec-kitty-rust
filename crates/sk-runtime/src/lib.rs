//! Deterministic, pure mission-runtime planning for the Rust Spec Kitty fork.
//!
//! This crate is deliberately narrower than the Python `spec-kitty next`
//! command. It owns:
//!
//! * mission-template and workflow schemas with fail-closed validation,
//! * bounded template/workflow file loading,
//! * the deterministic [`plan_next`] query over a frozen template and run
//!   snapshot,
//! * a pure [`apply_result`] prelude and a stale-state-checked
//!   [`plan_advance`] / [`commit_advance`] pair.
//!
//! It owns no run store, no event ledger, no locks, no prompt-file emission,
//! and no CLI presentation. A caller that needs the full governed
//! `spec-kitty next` workflow (run creation, persistence, event emission,
//! prompt files, workspace resolution, retrospective gates) must use the
//! command surface, not this library. This crate answers the planning question
//! only.
//!
//! # Scope boundary
//!
//! [`plan_next`] is a *query*. It never writes files and never advances a run.
//! Advancing a run is modelled by [`plan_advance`] (pure) followed by
//! [`commit_advance`], which refuses a plan whose source snapshot is no longer
//! the live snapshot. Persisting the returned snapshot belongs to the caller.
//!
//! # Python compatibility
//!
//! The planner preserves the upstream decision priority and decision JSON
//! shape for step and input-required decisions. It does not reproduce the
//! engine's run-store writes or its full `next` command behaviour. See the
//! crate tests for the fixtures that are and are not applicable.

mod advance;
mod decision;
mod error;
mod planner;
mod snapshot;
mod template;
mod workflow;

pub use advance::{
    AdvancePlan, AppliedResult, StepResult, apply_result, commit_advance, plan_advance,
};
pub use decision::{
    BlockedDecision, DecisionKind, DecisionRequired, DecisionRequirement, NextDecision,
    StepContextBundle, StepDecision, TerminalDecision,
};
pub use error::RuntimeError;
pub use planner::{TEMPLATE_DRIFT_REASON, TERMINAL_REASON, UNSCHEDULABLE_REASON, plan_next};
pub use snapshot::{
    ActorIdentity, ActorType, DecisionRequest, MissionPolicySnapshot, RunSnapshot, Strictness,
};
pub use template::{
    AuditConfig, AuditEnforcement, AuditStep, MAX_TEMPLATE_BYTES, MissionMeta, MissionTemplate,
    PromptStep, RaciAssignment, RaciRoleSpec, SignificanceBlock, TriggerMode, load_template,
};
pub use workflow::{
    ActionStep, MAX_WORKFLOW_BYTES, WorkflowRegistry, WorkflowSequence, load_workflow,
};
