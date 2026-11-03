//! Durable advance: plan with `sk-runtime`, then commit under one cursor lock.
//!
//! Two calling styles are supported:
//!
//! * [`RunStore::advance`] plans and commits inside one lock hold (upstream
//!   `next_step`), so two overlapping writers cannot both plan from the same
//!   cursor.
//! * [`RunStore::plan_advance`] + [`RunStore::commit_advance`] let a coordinator
//!   resolve a workspace or prompt between planning and committing; the commit
//!   is a compare-and-swap on the live cursor and fails closed.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::json;
use sk_runtime::{
    ActorIdentity, ActorType, AdvancePlan, NextDecision, ResolvedRaciSnapshot, RunSnapshot,
    SignificanceEvaluated, StepResult, plan_advance_with_actor,
};

use crate::error::RunStoreError;
use crate::events::{
    DECISION_INPUT_REQUESTED, EventRecord, MISSION_RUN_COMPLETED, NEXT_STEP_AUTO_COMPLETED,
    NEXT_STEP_ISSUED, SIGNIFICANCE_EVALUATED, append_records,
};
use crate::model::{RunRef, RunState, actor_payload, request_payload, state_now_iso};
use crate::prompt::write_prompt;
use crate::recovery::{Mode, RecoveryReport, reconcile};
use crate::store::{RunStore, persist_state};

/// One advance request. `agent_id` is the actor identity upstream records as an
/// LLM actor for both issued and completed step events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdvanceRequest {
    pub agent_id: String,
    pub result: StepResult,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_issued_step: Option<String>,
}

/// A serializable plan that can be inspected, stored or handed to another
/// process before committing. This is a caller-owned value, not a persisted
/// run-store format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlannedAdvance {
    pub run: RunRef,
    pub agent_id: String,
    pub result: StepResult,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_issued_step: Option<String>,
    pub template_hash: String,
    pub source: RunSnapshot,
    pub snapshot: RunSnapshot,
    pub decision: NextDecision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_step_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub significance: Option<SignificanceEvaluated>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raci: Option<ResolvedRaciSnapshot>,
}

impl PlannedAdvance {
    fn runtime_plan(&self) -> AdvancePlan {
        AdvancePlan {
            source: self.source.clone(),
            snapshot: self.snapshot.clone(),
            decision: self.decision.clone(),
            completed_step_id: self.completed_step_id.clone(),
            significance: self.significance.clone(),
            raci: self.raci.clone(),
            expected_issued_step: self.expected_issued_step.clone(),
        }
    }
}

/// The durable result of a committed advance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdvanceOutcome {
    pub run: RunRef,
    pub decision: NextDecision,
    pub state: RunState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_step_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_file: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<EventRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub significance: Option<SignificanceEvaluated>,
    pub recovered: RecoveryReport,
}

impl RunStore {
    /// Plan an advance without writing anything.
    pub fn plan_advance(
        &self,
        run: &RunRef,
        request: &AdvanceRequest,
    ) -> Result<PlannedAdvance, RunStoreError> {
        let run = self.resolve_ref(run)?;
        let state = self.read_state(&run)?;
        self.verify_frozen_hash(&run, &state)?;
        self.plan_from_state(&run, &state, request)
    }

    /// Commit a previously computed plan under the cursor lock.
    ///
    /// Refuses with [`RunStoreError::StaleAdvancePlan`] when the live cursor is
    /// no longer the plan's source (or the caller-evaluated issued step changed)
    /// and writes nothing.
    pub fn commit_advance(&self, plan: &PlannedAdvance) -> Result<AdvanceOutcome, RunStoreError> {
        let run = self.resolve_ref(&plan.run)?;
        let _lock = self.lock_cursor(&run)?;
        let (state, recovered, _consistency) = reconcile(self, &run, Mode::Repair)?;
        self.commit_locked(&run, plan, &state, recovered)
    }

    /// Plan and commit one advance under a single cursor-lock hold.
    pub fn advance(
        &self,
        run: &RunRef,
        request: &AdvanceRequest,
    ) -> Result<AdvanceOutcome, RunStoreError> {
        let run = self.resolve_ref(run)?;
        let _lock = self.lock_cursor(&run)?;
        let (state, recovered, _consistency) = reconcile(self, &run, Mode::Repair)?;
        let plan = self.plan_from_state(&run, &state, request)?;
        self.commit_locked(&run, &plan, &state, recovered)
    }

    fn plan_from_state(
        &self,
        run: &RunRef,
        state: &RunState,
        request: &AdvanceRequest,
    ) -> Result<PlannedAdvance, RunStoreError> {
        let template = self.load_frozen_template(run)?;
        let live_hash = self.live_template_hash(state)?;
        let snapshot = state.to_snapshot()?;
        let plan = plan_advance_with_actor(
            &template,
            &snapshot,
            live_hash.as_deref().unwrap_or(""),
            request.expected_issued_step.as_deref(),
            request.result,
            Some(&request.agent_id),
        )?;
        Ok(PlannedAdvance {
            run: run.clone(),
            agent_id: request.agent_id.clone(),
            result: request.result,
            expected_issued_step: request.expected_issued_step.clone(),
            template_hash: state.template_hash.clone(),
            source: snapshot,
            snapshot: plan.snapshot,
            decision: plan.decision,
            completed_step_id: plan.completed_step_id,
            significance: plan.significance,
            raci: plan.raci,
        })
    }

    fn commit_locked(
        &self,
        run: &RunRef,
        plan: &PlannedAdvance,
        state: &RunState,
        recovered: RecoveryReport,
    ) -> Result<AdvanceOutcome, RunStoreError> {
        if plan.run.run_id != run.run_id {
            return Err(RunStoreError::StaleAdvancePlan {
                run_id: plan.run.run_id.clone(),
                reason: "the plan names a different run".to_string(),
            });
        }
        if plan.template_hash != state.template_hash {
            return Err(RunStoreError::StaleAdvancePlan {
                run_id: run.run_id.clone(),
                reason: format!(
                    "the frozen template hash changed (planned {}, live {})",
                    plan.template_hash, state.template_hash
                ),
            });
        }
        let live_snapshot = state.to_snapshot()?;
        if let Some(expected) = &plan.expected_issued_step
            && live_snapshot.issued_step_id.as_deref() != Some(expected.as_str())
        {
            return Err(RunStoreError::StaleAdvancePlan {
                run_id: run.run_id.clone(),
                reason: format!(
                    "expected issued step {expected:?}, found {:?}",
                    live_snapshot.issued_step_id
                ),
            });
        }
        let committed = sk_runtime::commit_advance(&plan.runtime_plan(), &live_snapshot)?;
        if committed != plan.snapshot {
            return Err(RunStoreError::StaleAdvancePlan {
                run_id: run.run_id.clone(),
                reason: "the committed snapshot differs from the planned snapshot".to_string(),
            });
        }

        let actor = ActorIdentity {
            actor_id: plan.agent_id.clone(),
            actor_type: ActorType::Llm,
            display_name: String::new(),
            provider: None,
            model: None,
            tool: None,
        };
        let mut events = Vec::new();
        if let Some(step_id) = &plan.completed_step_id {
            events.push(EventRecord::new(
                NEXT_STEP_AUTO_COMPLETED,
                json!({
                    "run_id": run.run_id,
                    "step_id": step_id,
                    "agent_id": plan.agent_id,
                    "result": plan.result.as_str(),
                    "actor": actor_payload(&actor),
                    "mission_id": state.mission_id,
                    "mission_slug": state.mission_slug,
                }),
            ));
        }

        let mut next_state = state.clone();
        next_state.completed_steps = plan.snapshot.completed_steps.clone();
        next_state.blocked_reason = plan.snapshot.blocked_reason.clone();
        next_state.issued_step_id = None;
        // The pure plan may add `significance:` and `raci:` records.
        next_state.decisions = plan.snapshot.decisions.clone();
        next_state.inputs = plan.snapshot.inputs.clone();
        let mut prompt_file = None;

        if let Some(significance) = &plan.significance {
            events.push(EventRecord::new(
                SIGNIFICANCE_EVALUATED,
                serde_json::to_value(significance).map_err(|error| {
                    RunStoreError::TemplateInvalid {
                        message: format!("significance payload is not serializable: {error}"),
                    }
                })?,
            ));
        }

        match &plan.decision {
            NextDecision::Step(step) => {
                // Write the real prompt artifact before any journal append. A
                // prompt failure aborts the transaction with nothing committed.
                prompt_file = Some(write_prompt(
                    &run.run_dir,
                    &step.step_id,
                    &step.step_title,
                    &step.prompt,
                    &run.run_id,
                    &state.mission_key,
                    &state.template_hash,
                )?);
                next_state.issued_step_id = Some(step.step_id.clone());
                events.push(EventRecord::new(
                    NEXT_STEP_ISSUED,
                    json!({
                        "run_id": run.run_id,
                        "step_id": step.step_id,
                        "agent_id": plan.agent_id,
                        "actor": actor_payload(&actor),
                        "mission_id": state.mission_id,
                        "mission_slug": state.mission_slug,
                    }),
                ));
            }
            NextDecision::DecisionRequired(required) => {
                let (decision_id, step_id, question, options, input_key) =
                    match &required.requirement {
                        sk_runtime::DecisionRequirement::PendingDecision {
                            decision_id,
                            step_id,
                            input_key,
                            question,
                            options,
                        } => (
                            decision_id.clone(),
                            step_id.clone(),
                            question.clone(),
                            options.clone(),
                            input_key.clone(),
                        ),
                        sk_runtime::DecisionRequirement::MissingRequiredInput {
                            decision_id,
                            step_id,
                            input_key,
                            question,
                        } => (
                            decision_id.clone(),
                            step_id.clone(),
                            question.clone(),
                            Vec::new(),
                            Some(input_key.clone()),
                        ),
                        sk_runtime::DecisionRequirement::AuditCheckpoint {
                            decision_id,
                            step_id,
                            question,
                            options,
                            ..
                        } => (
                            decision_id.clone(),
                            step_id.clone(),
                            question.clone(),
                            options.clone(),
                            None,
                        ),
                    };
                if !next_state.pending_decisions.contains_key(&decision_id) {
                    // `requested_at` is a cursor datetime, so it uses the
                    // Pydantic `Z` shape rather than the journal shape.
                    let requested_at = state_now_iso();
                    next_state.pending_decisions.insert(
                        decision_id.clone(),
                        request_payload(
                            &decision_id,
                            &step_id,
                            &question,
                            &options,
                            &actor,
                            &requested_at,
                        ),
                    );
                    let payload = json!({
                        "run_id": run.run_id,
                        "decision_id": decision_id,
                        "step_id": step_id,
                        "question": question,
                        "options": options,
                        "input_key": input_key,
                        "actor": actor_payload(&actor),
                        "mission_id": state.mission_id,
                        "mission_slug": state.mission_slug,
                    });
                    events.push(EventRecord::new(DECISION_INPUT_REQUESTED, payload));
                }
            }
            NextDecision::Terminal(_) => {
                if plan.completed_step_id.is_some() {
                    events.push(EventRecord::new(
                        MISSION_RUN_COMPLETED,
                        json!({
                            "run_id": run.run_id,
                            "mission_type": state.mission_key,
                            "actor": actor_payload(&actor),
                            "mission_id": state.mission_id,
                            "mission_slug": state.mission_slug,
                        }),
                    ));
                }
            }
            NextDecision::Blocked(_) => {}
        }

        next_state.validate(Some(&run.run_id))?;
        if !events.is_empty() {
            append_records(&self.events_path(run), &events)?;
        }
        persist_state(&run.run_dir, &next_state)?;

        Ok(AdvanceOutcome {
            run: run.clone(),
            decision: plan.decision.clone(),
            state: next_state,
            completed_step_id: plan.completed_step_id.clone(),
            prompt_file,
            events,
            significance: plan.significance.clone(),
            recovered,
        })
    }
}
