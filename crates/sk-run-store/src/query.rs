//! Non-mutating run query.
//!
//! Query reads the durable cursor, validates the frozen artifact and the index,
//! reconciles the journal in memory and plans the next decision without writing
//! or locking. When the run is inconsistent it returns
//! [`RunStoreError::RecoveryRequired`] instead of presenting a misleading state.

use sk_runtime::MissionTemplate;

use crate::error::RunStoreError;
use crate::model::{IssuedStepView, MAX_PROMPT_BYTES, QueryResult, RunRef, pending_views};
use crate::recovery::{Mode, reconcile};
use crate::store::{RunStore, validate_mission_id, validate_mission_slug};

impl RunStore {
    /// Read the current state of one run without mutating anything.
    pub fn query(&self, run: &RunRef) -> Result<QueryResult, RunStoreError> {
        let run = self.resolve_ref(run)?;
        let (state, _report, consistency) = reconcile(self, &run, Mode::Analyze)?;
        if !consistency.healthy {
            return Err(RunStoreError::RecoveryRequired {
                run_id: run.run_id.clone(),
                details: consistency.differences,
            });
        }

        let template = self.load_frozen_template(&run)?;
        let live_hash = self.live_template_hash(&state)?;
        let snapshot = state.to_snapshot()?;
        let decision =
            sk_runtime::plan_next(&template, &snapshot, live_hash.as_deref().unwrap_or(""))?;
        let template_drift = live_hash
            .as_deref()
            .is_some_and(|hash| hash != state.template_hash);

        let issued_step = match &state.issued_step_id {
            Some(step_id) => Some(self.issued_step_view(&run, &template, step_id)?),
            None => None,
        };

        Ok(QueryResult {
            run,
            state: state.clone(),
            decision,
            issued_step,
            pending: pending_views(&state),
            template_hash: state.template_hash.clone(),
            live_template_hash: live_hash,
            template_drift,
            consistency,
        })
    }

    /// Read-only next-decision from the durable cursor alone.
    ///
    /// The pinned engine's *read* path tolerates a torn final journal line (it
    /// consumes complete lines); only a *mutation* must refuse until recovery.
    /// [`Self::query`] deliberately refuses a torn journal because it validates
    /// journal/cursor agreement, which is the right answer before a mutation.
    /// A read-only caller that must match the reference's tolerant read can use
    /// this method instead: it reads `state.json`, the run's frozen template and
    /// the recorded live template hash, plans the next decision, and returns
    /// both. It never writes, repairs or claims the journal is consistent.
    ///
    /// Callers must only use this after the strict query has established that
    /// the sole reason it refused is a torn journal tail; it is not a general
    /// substitute for [`Self::query`].
    pub fn plan_from_cursor(
        &self,
        run: &RunRef,
    ) -> Result<(crate::model::RunState, sk_runtime::NextDecision), RunStoreError> {
        let run = self.resolve_ref(run)?;
        let state = self.read_state(&run)?;
        self.verify_frozen_hash(&run, &state)?;
        let template = self.load_frozen_template(&run)?;
        let live_hash = self.live_template_hash(&state)?;
        let snapshot = state.to_snapshot()?;
        let decision =
            sk_runtime::plan_next(&template, &snapshot, live_hash.as_deref().unwrap_or(""))?;
        Ok((state, decision))
    }

    /// Look up the mission's indexed run and query it, without starting a run.
    pub fn query_mission(
        &self,
        mission_slug: &str,
        mission_id: Option<&str>,
    ) -> Result<QueryResult, RunStoreError> {
        validate_mission_slug(mission_slug)?;
        if let Some(mission_id) = mission_id {
            validate_mission_id(mission_id)?;
        }
        let selector = match mission_id {
            Some(mission_id) => format!("mission {mission_slug:?} ({mission_id})"),
            None => format!("mission {mission_slug:?}"),
        };
        let run = self
            .run_for_mission(mission_slug, mission_id)?
            .ok_or(RunStoreError::RunNotFound { selector })?;
        self.query(&run)
    }

    fn issued_step_view(
        &self,
        run: &RunRef,
        template: &MissionTemplate,
        step_id: &str,
    ) -> Result<IssuedStepView, RunStoreError> {
        let (title, prompt) = template
            .steps
            .iter()
            .find(|step| step.id == step_id)
            .map(|step| {
                (
                    step.title.clone(),
                    step.prompt
                        .clone()
                        .unwrap_or_else(|| format!("Execute step '{}': {}", step.id, step.title)),
                )
            })
            .or_else(|| {
                template
                    .audit_steps
                    .iter()
                    .find(|step| step.id == step_id)
                    .map(|step| {
                        (
                            step.title.clone(),
                            format!("Execute audit step '{}': {}", step.id, step.title),
                        )
                    })
            })
            .unwrap_or_else(|| (step_id.to_string(), format!("Execute step '{step_id}'")));

        let prompt_file = self.prompt_path(run, step_id)?;
        let prompt = sk_kernel::read_bounded(&prompt_file, MAX_PROMPT_BYTES)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .and_then(|text| {
                text.split_once("\n---\n\n")
                    .map(|(_header, body)| body.trim_end().to_string())
            })
            .unwrap_or(prompt);
        Ok(IssuedStepView {
            step_id: step_id.to_string(),
            title,
            prompt,
            prompt_file,
        })
    }
}
