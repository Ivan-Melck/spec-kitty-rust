//! Deterministic journal/cursor/index reconciliation.
//!
//! Upstream commits an advance in the order *events, then `state.json`* and an
//! answer in the order *`state.json`, then the answered event*. Either order
//! can be interrupted, so recovery must be able to roll the cursor forward from
//! the journal and append a missing answered/started event from the cursor.
//!
//! Recovery is explicit (or runs inside a mutating transaction under the
//! cursor lock). Query is non-mutating: it computes the same differences and
//! reports [`RunStoreError::RecoveryRequired`] instead of repairing.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::answer::{answered_event_payload, apply_pure_answer, map_answer_error};
use crate::error::RunStoreError;
use crate::events::{
    DECISION_AUTHORITY_DENIED, DECISION_INPUT_ANSWERED, DECISION_INPUT_REQUESTED, EventRecord,
    Journal, MISSION_RUN_COMPLETED, MISSION_RUN_STARTED, NEXT_STEP_AUTO_COMPLETED,
    NEXT_STEP_ISSUED, SIGNIFICANCE_EVALUATED, append_records, read_journal, truncate_torn_tail,
};
use crate::model::{ConsistencyReport, RunRef, RunState, now_iso, parse_actor, request_payload};
use crate::store::{RunStore, find_entry, persist_state};
use sk_runtime::{AnswerError, apply_decision_answer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Read-only: compute the differences, never repair.
    Analyze,
    /// Mutating transaction: reconcile derivable gaps, but refuse a torn tail
    /// that only an explicit operator recovery may cut.
    Repair,
    /// Explicit recovery: may truncate a torn event tail.
    Recover,
}

/// What an explicit recovery changed. A no-op report is a successful
/// consistency check.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryReport {
    pub torn_tail_truncated: u64,
    pub started_event_appended: bool,
    pub missing_events_appended: usize,
    pub state_rolled_forward: bool,
    pub index_repaired: bool,
    pub replayed_events: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<String>,
}

impl RecoveryReport {
    pub fn is_noop(&self) -> bool {
        self.torn_tail_truncated == 0
            && !self.started_event_appended
            && self.missing_events_appended == 0
            && !self.state_rolled_forward
            && !self.index_repaired
    }
}

impl RunStore {
    /// Explicitly reconcile one run under its cursor lock.
    pub fn recover(&self, run: &RunRef) -> Result<RecoveryReport, RunStoreError> {
        let run = self.resolve_ref(run)?;
        let _lock = self.lock_cursor(&run)?;
        let (_state, report, _consistency) = reconcile(self, &run, Mode::Recover)?;
        Ok(report)
    }
}

/// Reconcile a run. `Analyze` performs no writes; `Repair` may truncate a torn
/// tail, append derivable events, roll the cursor forward and repair the index.
pub(crate) fn reconcile(
    store: &RunStore,
    run: &RunRef,
    mode: Mode,
) -> Result<(RunState, RecoveryReport, ConsistencyReport), RunStoreError> {
    let state = store.read_state(run)?;
    // Corruption of the frozen artifact is never repaired by guessing.
    store.verify_frozen_hash(run, &state)?;

    let mut report = RecoveryReport::default();
    let mut journal = read_journal(&store.events_path(run))?;
    let mut details = Vec::new();

    if journal.torn_tail_bytes > 0 {
        match mode {
            Mode::Recover => {
                let removed = truncate_torn_tail(&store.events_path(run))?;
                report.torn_tail_truncated = removed;
                report
                    .actions
                    .push(format!("truncated {removed} torn event-tail byte(s)"));
                journal = read_journal(&store.events_path(run))?;
            }
            Mode::Repair => {
                return Err(RunStoreError::TornEventTail {
                    path: store.events_path(run),
                    torn_bytes: journal.torn_tail_bytes,
                });
            }
            Mode::Analyze => details.push(format!(
                "event journal has a {}-byte torn tail",
                journal.torn_tail_bytes
            )),
        }
    }
    report.replayed_events = journal.events.len();

    let mut started_missing = false;
    if journal.events.is_empty() {
        started_missing = true;
        details.push("MissionRunStarted event is missing".to_string());
    } else if let Some(first) = journal.events.first() {
        if first.event_type != MISSION_RUN_STARTED {
            return Err(RunStoreError::InconsistentRunJournal {
                run_id: run.run_id.clone(),
                details: vec![format!(
                    "first journal event is {:?}, expected {MISSION_RUN_STARTED:?}",
                    first.event_type
                )],
            });
        }
        let recorded = first.payload.get("run_id").and_then(Value::as_str);
        if recorded != Some(run.run_id.as_str()) {
            return Err(RunStoreError::InconsistentRunJournal {
                run_id: run.run_id.clone(),
                details: vec![format!(
                    "MissionRunStarted run_id {recorded:?} does not match {:?}",
                    run.run_id
                )],
            });
        }
    }

    let (working, mut missing_answered) = replay(&state, &journal)?;
    if !missing_answered.is_empty() {
        details.push(format!(
            "journal is missing {} answered event(s) present in the cursor",
            missing_answered.len()
        ));
    }
    let (target, state_changed) = roll_forward(&state, &working, &mut details)?;
    details.extend(index_differences(store, run, &target)?);
    if started_missing {
        // Recorded separately so Repair appends it before answered events.
        report.started_event_appended = mode != Mode::Analyze;
    }

    if mode == Mode::Analyze {
        let consistency = if details.is_empty() {
            ConsistencyReport::healthy()
        } else {
            ConsistencyReport::unhealthy(details)
        };
        return Ok((state, report, consistency));
    }

    // ---- repair phase -----------------------------------------------------
    if started_missing {
        let started = started_event(&state);
        append_records(&store.events_path(run), &[started])?;
        report
            .actions
            .push("appended the missing MissionRunStarted event".to_string());
    }
    if !missing_answered.is_empty() {
        missing_answered.sort_by(|left, right| {
            left.timestamp.cmp(&right.timestamp).then_with(|| {
                left.payload["decision_id"]
                    .to_string()
                    .cmp(&right.payload["decision_id"].to_string())
            })
        });
        let count = missing_answered.len();
        append_records(&store.events_path(run), &missing_answered)?;
        report.missing_events_appended = count;
        report.actions.push(format!(
            "appended {count} missing DecisionInputAnswered event(s)"
        ));
    }
    if state_changed {
        persist_state(&run.run_dir, &target)?;
        report.state_rolled_forward = true;
        report
            .actions
            .push("rolled the cursor forward from the journal".to_string());
    }
    if !index_present(store, run, &target)? {
        store.ensure_index_entry(run, &target)?;
        report.index_repaired = true;
        report
            .actions
            .push("repaired the missing feature-runs.json entry".to_string());
    }
    target.validate(Some(&run.run_id))?;

    let consistency = ConsistencyReport::healthy();
    Ok((target, report, consistency))
}

/// Rebuild the cursor the journal proves, seeded with non-cursor metadata
/// (policy/inputs/significance/RACI/soft-gate records) from the durable state.
fn replay(
    state: &RunState,
    journal: &Journal,
) -> Result<(RunState, Vec<EventRecord>), RunStoreError> {
    let mut working = state.clone();
    working.completed_steps.clear();
    working.issued_step_id = None;
    working.blocked_reason = None;
    working.pending_decisions.clear();
    working.decisions.retain(|key, _| {
        key.starts_with("significance:")
            || key.starts_with("raci:")
            || key.starts_with("soft_gate:")
    });

    for event in &journal.events {
        match event.event_type.as_str() {
            MISSION_RUN_STARTED | MISSION_RUN_COMPLETED | DECISION_AUTHORITY_DENIED => {}
            NEXT_STEP_ISSUED => {
                let step_id = required_str(event, "step_id")?;
                if working.completed_steps.iter().any(|step| step == step_id) {
                    return Err(inconsistent(
                        state,
                        format!("journal issues already-completed step {step_id:?}"),
                    ));
                }
                working.issued_step_id = Some(step_id.to_string());
            }
            NEXT_STEP_AUTO_COMPLETED => {
                let step_id = required_str(event, "step_id")?;
                let result = required_str(event, "result")?;
                if working.issued_step_id.as_deref() != Some(step_id) {
                    return Err(inconsistent(
                        state,
                        format!(
                            "journal completes step {step_id:?} but the issued step is {:?}",
                            working.issued_step_id
                        ),
                    ));
                }
                working.issued_step_id = None;
                match result {
                    "success" => {
                        if !working.completed_steps.iter().any(|step| step == step_id) {
                            working.completed_steps.push(step_id.to_string());
                        }
                    }
                    "failed" => {
                        working.blocked_reason = Some(format!(
                            "Previous step '{step_id}' failed; manual intervention required."
                        ));
                    }
                    "blocked" => {
                        working.blocked_reason =
                            Some(format!("Previous step '{step_id}' reported blocked state."));
                    }
                    other => {
                        return Err(inconsistent(
                            state,
                            format!("unknown step result {other:?} for {step_id:?}"),
                        ));
                    }
                }
            }
            DECISION_INPUT_REQUESTED => {
                let decision_id = required_str(event, "decision_id")?;
                if working.pending_decisions.contains_key(decision_id) {
                    continue;
                }
                let actor = event
                    .payload
                    .get("actor")
                    .and_then(parse_actor)
                    .ok_or_else(|| {
                        inconsistent(state, format!("request {decision_id:?} has no actor"))
                    })?;
                let options = event
                    .payload
                    .get("options")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                working.pending_decisions.insert(
                    decision_id.to_string(),
                    request_payload(
                        decision_id,
                        required_str(event, "step_id")?,
                        required_str(event, "question")?,
                        &options,
                        &actor,
                        &event.timestamp,
                    ),
                );
            }
            DECISION_INPUT_ANSWERED => {
                let decision_id = required_str(event, "decision_id")?;
                let answer = required_str(event, "answer")?;
                let actor = event
                    .payload
                    .get("actor")
                    .and_then(parse_actor)
                    .ok_or_else(|| {
                        inconsistent(state, format!("answer {decision_id:?} has no actor"))
                    })?;
                // A soft gate can remain pending after defer/open_stand_up;
                // its next answer replaces the prior record, just like the live
                // engine. A completed decision still refuses as non-pending.
                if !working.pending_decisions.contains_key(decision_id) {
                    return Err(inconsistent(
                        state,
                        format!("journal answers non-pending decision {decision_id:?}"),
                    ));
                }
                // Replay through the same pure policy the live path uses, so
                // mediation/completion cannot drift from a normal answer.
                let snapshot = working.to_snapshot()?;
                let applied =
                    apply_decision_answer(&snapshot, &actor, decision_id, answer, &event.timestamp)
                        .map_err(|error| match error {
                            AnswerError::AuthorityDenied(denial) => inconsistent(
                                state,
                                format!(
                                    "journal answer {decision_id:?} is no longer authorized: {}",
                                    denial.reason
                                ),
                            ),
                            other => map_answer_error(other, decision_id, answer),
                        })?;
                apply_pure_answer(&mut working, &applied);
            }
            SIGNIFICANCE_EVALUATED => {
                if let Some(decision_id) = event.payload.get("decision_id").and_then(Value::as_str)
                {
                    let score = event
                        .payload
                        .get("significance_score")
                        .cloned()
                        .unwrap_or_else(|| event.payload.clone());
                    working
                        .decisions
                        .insert(format!("significance:{decision_id}"), score);
                }
                // A LOW gate auto-proceeds: upstream folds the audit step into
                // `completed_steps` during planning and emits no separate
                // NextStepAutoCompleted for it. The significance event is the
                // durable proof of that completion.
                let low =
                    event.payload.get("effective_band").and_then(Value::as_str) == Some("low");
                if low
                    && let Some(step_id) = event.payload.get("step_id").and_then(Value::as_str)
                    && !working.completed_steps.iter().any(|step| step == step_id)
                {
                    working.completed_steps.push(step_id.to_string());
                }
            }
            other => {
                return Err(inconsistent(
                    state,
                    format!("unknown journal event type {other:?}"),
                ));
            }
        }
    }

    // A cursor answer with no answered event is a state-ahead crash window:
    // the event is derivable from the persisted answer record.
    let mut missing_answered = Vec::new();
    for (decision_id, record) in &state.decisions {
        if record.get("answer").and_then(Value::as_str).is_none() {
            continue;
        }
        if working.decisions.contains_key(decision_id) {
            continue;
        }
        missing_answered.push(answered_event(&state.run_id, decision_id, record)?);
    }

    Ok((working, missing_answered))
}

fn roll_forward(
    state: &RunState,
    working: &RunState,
    details: &mut Vec<String>,
) -> Result<(RunState, bool), RunStoreError> {
    let mut target = state.clone();
    let mut changed = false;

    let common = state
        .completed_steps
        .len()
        .min(working.completed_steps.len());
    if state.completed_steps[..common] != working.completed_steps[..common] {
        return Err(inconsistent(
            state,
            "completed-step history differs from the journal".to_string(),
        ));
    }
    if state.completed_steps.len() > working.completed_steps.len() {
        return Err(inconsistent(
            state,
            "cursor records completed steps the journal does not prove".to_string(),
        ));
    }
    let completions_grew = working.completed_steps.len() > state.completed_steps.len();
    if completions_grew {
        details.push(format!(
            "cursor is behind {} journal completion(s)",
            working.completed_steps.len() - state.completed_steps.len()
        ));
        target.completed_steps = working.completed_steps.clone();
        changed = true;
    }

    if state.issued_step_id != working.issued_step_id {
        let state_was_completed = state
            .issued_step_id
            .as_ref()
            .is_some_and(|step| completions_grew && working.completed_steps.contains(step));
        if state.issued_step_id.is_none() || state_was_completed {
            details.push(format!(
                "cursor issued step {:?} -> {:?}",
                state.issued_step_id, working.issued_step_id
            ));
            target.issued_step_id = working.issued_step_id.clone();
            changed = true;
        } else {
            return Err(inconsistent(
                state,
                format!(
                    "cursor issued step {:?} disagrees with journal {:?}",
                    state.issued_step_id, working.issued_step_id
                ),
            ));
        }
    }

    if state.blocked_reason != working.blocked_reason {
        if state.blocked_reason.is_none() || completions_grew {
            details.push("cursor blocked reason is behind the journal".to_string());
            target.blocked_reason = working.blocked_reason.clone();
            changed = true;
        } else {
            return Err(inconsistent(
                state,
                "cursor blocked reason disagrees with the journal".to_string(),
            ));
        }
    }

    for (key, request) in &working.pending_decisions {
        if !target.pending_decisions.contains_key(key) {
            // A cursor answer with no answered event is the state-ahead crash
            // window: the decision is no longer pending even though the journal
            // replay still sees its request.
            if state.decisions.contains_key(key) {
                continue;
            }
            details.push(format!(
                "pending decision {key:?} is missing from the cursor"
            ));
            target
                .pending_decisions
                .insert(key.clone(), request.clone());
            changed = true;
        }
    }
    for key in state.pending_decisions.keys() {
        if !working.pending_decisions.contains_key(key) {
            let answered_in_journal = working
                .decisions
                .get(key)
                .is_some_and(|record| record.get("answer").is_some());
            if state.decisions.contains_key(key) || answered_in_journal {
                details.push(format!("cursor still holds answered decision {key:?}"));
                target.pending_decisions.remove(key);
                changed = true;
            } else {
                return Err(inconsistent(
                    state,
                    format!("cursor pending decision {key:?} has no request event"),
                ));
            }
        }
    }

    for (decision_id, record) in &working.decisions {
        let is_answer = record.get("answer").and_then(Value::as_str).is_some();
        let is_significance = decision_id.starts_with("significance:");
        if !is_answer && !is_significance {
            continue;
        }
        match state.decisions.get(decision_id) {
            None => {
                details.push(if is_answer {
                    format!("cursor is missing answer {decision_id:?}")
                } else {
                    format!("cursor is missing significance record {decision_id:?}")
                });
                target.decisions.insert(decision_id.clone(), record.clone());
                changed = true;
            }
            Some(existing) => {
                if is_answer {
                    let left = existing.get("answer").and_then(Value::as_str);
                    let right = record.get("answer").and_then(Value::as_str);
                    if left != right {
                        return Err(inconsistent(
                            state,
                            format!("answer {decision_id:?} differs between cursor and journal"),
                        ));
                    }
                }
            }
        }
    }

    for (key, value) in &working.inputs {
        match state.inputs.get(key) {
            None => {
                details.push(format!("cursor is missing input {key:?}"));
                target.inputs.insert(key.clone(), value.clone());
                changed = true;
            }
            Some(existing) if existing != value => {
                return Err(inconsistent(
                    state,
                    format!("input {key:?} differs between cursor and journal"),
                ));
            }
            Some(_) => {}
        }
    }

    Ok((target, changed))
}

fn started_event(state: &RunState) -> EventRecord {
    EventRecord {
        event_type: MISSION_RUN_STARTED.to_string(),
        timestamp: now_iso(),
        payload: json!({
            "run_id": state.run_id,
            "mission_type": state.mission_key,
            "actor": {
                "actor_id": "system",
                "actor_type": "service",
                "display_name": "",
                "provider": null,
                "model": null,
                "tool": null,
            },
            "mission_id": state.mission_id,
            "mission_slug": state.mission_slug,
        }),
    }
}

fn answered_event(
    run_id: &str,
    decision_id: &str,
    record: &Value,
) -> Result<EventRecord, RunStoreError> {
    let answer = record
        .get("answer")
        .and_then(Value::as_str)
        .ok_or_else(|| RunStoreError::StateCorrupt {
            path: Path::new("state.json").to_path_buf(),
            message: format!("answer record {decision_id:?} has no answer"),
        })?;
    let actor = record
        .get("answered_by")
        .and_then(parse_actor)
        .ok_or_else(|| RunStoreError::StateCorrupt {
            path: Path::new("state.json").to_path_buf(),
            message: format!("answer record {decision_id:?} has no decodable answered_by actor"),
        })?;
    Ok(EventRecord {
        event_type: DECISION_INPUT_ANSWERED.to_string(),
        timestamp: record
            .get("answered_at")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(now_iso),
        payload: answered_event_payload(
            run_id,
            decision_id,
            answer,
            &actor,
            record.get("mission_id").and_then(Value::as_str),
            record.get("mission_slug").and_then(Value::as_str),
        )?,
    })
}

fn index_differences(
    store: &RunStore,
    run: &RunRef,
    state: &RunState,
) -> Result<Vec<String>, RunStoreError> {
    Ok(if index_present(store, run, state)? {
        Vec::new()
    } else {
        vec!["feature-runs.json has no live entry for this run".to_string()]
    })
}

fn index_present(store: &RunStore, run: &RunRef, state: &RunState) -> Result<bool, RunStoreError> {
    let (slug, id) = crate::store::resolved_run_identity(run, state)?;
    let (index, _rekeyed) = store.read_index_canonical()?;
    let Some((_key, entry)) = find_entry(&index, slug, id)? else {
        return Ok(false);
    };
    let indexed = store.run_ref_from_entry(entry)?;
    if indexed.run_id == run.run_id {
        Ok(true)
    } else {
        Err(RunStoreError::IdentityMismatch {
            message: format!(
                "mission {slug:?} is indexed to run {:?}, not {:?}",
                indexed.run_id, run.run_id
            ),
        })
    }
}

fn required_str<'a>(event: &'a EventRecord, field: &str) -> Result<&'a str, RunStoreError> {
    event
        .payload
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| RunStoreError::EventLogCorrupt {
            path: Path::new("run.events.jsonl").to_path_buf(),
            line: 0,
            message: format!(
                "{} payload is missing string field {field:?}",
                event.event_type
            ),
        })
}

fn inconsistent(state: &RunState, detail: String) -> RunStoreError {
    RunStoreError::InconsistentRunJournal {
        run_id: state.run_id.clone(),
        details: vec![detail],
    }
}
