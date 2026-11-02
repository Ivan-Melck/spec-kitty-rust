//! Deterministic, pure mission-runtime planning and policy for the Rust Spec
//! Kitty fork.
//!
//! This crate is deliberately narrower than the Python `spec-kitty next`
//! command. It owns:
//!
//! * mission-template and workflow schemas with fail-closed validation,
//! * bounded template/workflow file loading and workflow discovery,
//! * the deterministic [`plan_next`] query over a frozen template and run
//!   snapshot,
//! * a pure [`apply_result`] prelude and a stale-state-checked
//!   [`plan_advance`] / [`commit_advance`] pair,
//! * audit significance policy (six dimensions, three bands, five hard
//!   triggers) and its LOW/MEDIUM/HIGH gate decisions,
//! * deterministic RACI inference/resolution with human accountability,
//! * pure decision-answer authorization and application
//!   ([`authorize_decision_answer`] / [`apply_decision_answer`]),
//! * a pure projection of the retrospective terminus lifecycle.
//!
//! It owns no run store, no event ledger, no locks, no prompt-file emission,
//! and no CLI presentation. A caller that needs the full governed
//! `spec-kitty next` workflow (run creation, persistence, event emission,
//! prompt files, workspace resolution, retrospective record writing) must use
//! the command surface, not this library. This crate answers the planning and
//! policy questions only.
//!
//! # Scope boundary
//!
//! [`plan_next`] is a *query*. It never writes files and never advances a run.
//! Advancing a run is modelled by [`plan_advance`] (pure, including significance
//! and RACI policy) followed by [`commit_advance`], which refuses a plan whose
//! source snapshot is no longer the live snapshot. Persisting the returned
//! snapshot belongs to the caller.
//!
//! [`apply_decision_answer`] is likewise pure: it returns the new snapshot, the
//! persisted decision record, the progression effect and the
//! `DecisionInputAnswered` event body. Appending the event and writing the
//! snapshot atomically is the run-store owner's transaction.
//!
//! # Python compatibility
//!
//! The planner preserves the upstream decision priority and decision JSON
//! shape for step, input-required, audit, pending, blocked and terminal
//! decisions. [`plan_advance`] reproduces the upstream engine's significance
//! and RACI folding. Answer semantics reproduce `provide_decision_answer` for
//! input-keyed, LLM-delegated and audit decisions, including the MODERATE
//! soft gate and authority denials. Golden fixtures captured from the pinned
//! Python interpreter cover these paths.
//!
//! Deliberate fail-closed differences from Python: malformed RACI/significance
//! records are typed errors instead of being silently ignored, and a template
//! with duplicate/unknown/cyclic dependencies refuses to load rather than
//! returning a later blocked decision. A MEDIUM re-evaluation of an
//! already-pending audit decision keeps `reason: "pending_decision"`, where
//! upstream rebuilds the decision and loses the reason.
//!
//! # Not owned here
//!
//! Run-store persistence and event appends, run-cursor locking, prompt-file
//! emission, Mission metadata (`meta.json`) reading for workflow selection,
//! retrospective record writing and operator prompting, context-contract
//! registries, `TimeoutPolicy`/`DimensionScoreOverride` audit records, and
//! facilitator exception handling all belong to other bounded contexts. Their
//! absence here is a scope boundary, not a claimed implementation.

mod advance;
mod answer;
mod decision;
mod error;
mod planner;
mod raci;
mod retrospective;
mod significance;
mod snapshot;
mod template;
mod workflow;

pub use advance::{
    AdvancePlan, AppliedResult, StepResult, apply_result, commit_advance, plan_advance,
    plan_advance_with_actor,
};
pub use answer::{
    AUDIT_PREFIX, AnswerEffect, AnswerError, AppliedAnswer, AuthorityDenial, DecisionAnswered,
    DecisionAuthority, INPUT_PREFIX, apply_decision_answer, authorize_decision_answer,
};
pub use decision::{
    BlockedDecision, DecisionKind, DecisionRequired, DecisionRequirement, NextDecision,
    StepContextBundle, StepDecision, TerminalDecision,
};
pub use error::RuntimeError;
pub use planner::{
    TEMPLATE_DRIFT_REASON, TERMINAL_REASON, UNSCHEDULABLE_REASON, find_step, plan_next,
};
pub use raci::{
    HUMAN_IN_CHARGE_ACTOR_ID, RaciRoleBinding, RaciRule, RaciSource, ResolvedRaciSnapshot, StepRef,
    UNKNOWN_ACTOR_ID, bind_actor, infer_raci, resolve_raci, validate_raci_assignment,
};
pub use retrospective::{
    DEFAULT_SKIP_REASON, DEFERRED_ACTION_ID, DEFERRED_FACILITATOR_ID, HicChoice, RequesterActor,
    RetrospectiveMode, TERMINUS_STEP_ID, TerminusPlan, TerminusStep, plan_terminus,
};
pub use significance::{
    BandCutoffs, DimensionName, HardTriggerClass, HardTriggerId, MAX_COMPOSITE_SCORE,
    MAX_DIMENSION_SCORE, RoutingBand, RoutingBandName, SignificanceDimension,
    SignificanceEvaluated, SignificanceScore, SoftGateAction, SoftGateDecision, default_bands,
    evaluate_significance, parse_band_cutoffs_from_policy, validate_band_cutoffs,
    validate_dimension_scores,
};
pub use snapshot::{
    ActorIdentity, ActorType, DecisionRequest, MissionPolicySnapshot, RunSnapshot, Strictness,
};
pub use template::{
    AuditConfig, AuditEnforcement, AuditStep, MAX_TEMPLATE_BYTES, MissionMeta, MissionTemplate,
    PromptStep, RaciAssignment, RaciRoleSpec, SignificanceBlock, TriggerMode, load_template,
};
pub use workflow::{
    ActionStep, DEFAULT_WORKFLOW_ID, MAX_WORKFLOW_BYTES, WorkflowActionPlan, WorkflowRegistry,
    WorkflowSearch, WorkflowSequence, compose_template_with_workflow, list_available_workflows,
    load_workflow, load_workflow_by_id, plan_next_workflow_action, resolve_mission_workflow,
    resolve_workflow_path, validate_workflow_id, workflow_candidate_paths,
};
