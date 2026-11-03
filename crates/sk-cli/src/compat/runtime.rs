//! Persistent governed-run routes over `sk-run-store`.
//!
//! `next` is the upstream-shaped loop: query, advance, answer. The `runtime`
//! group exposes the substrate contracts directly (a native extension, not an
//! upstream route) so operators and tests can inspect durable run state.
//!
//! The composed upstream `next` decision carries additional mission-board
//! fields (workspace resolution, guard evaluation, progress). This slice fills
//! every field the durable run and canonical status log can supply and reports
//! the remainder as `null`; the capability matrix records the gap.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use sk_run_store::{
    ActorIdentity, ActorType, AdvanceRequest, AnswerRequest, NextDecisionExt, QueryResult, RunRef,
    RunState, RunStore, StartRequest, StepResult,
};
use sk_runtime::{DecisionKind, NextDecision};

use super::support::{
    CompatError, emit_json, emit_json_pretty, inject_branch_contract, path_string, project_root,
    resolve_mission,
};

fn map_store_error(error: sk_run_store::RunStoreError) -> CompatError {
    CompatError::domain(format!("{error}"))
}

fn parse_result(value: &str) -> Result<StepResult, CompatError> {
    match value.trim() {
        "success" => Ok(StepResult::Success),
        "failed" => Ok(StepResult::Failed),
        "blocked" => Ok(StepResult::Blocked),
        other => Err(CompatError::usage(format!(
            "invalid --result {other:?}: expected success, failed, or blocked"
        ))),
    }
}

/// The canonical runtime template for a mission type.
pub fn runtime_template(root: &Path, mission_type: &str) -> Result<PathBuf, CompatError> {
    let mut candidates = vec![
        root.join(".kittify")
            .join("overrides")
            .join("missions")
            .join(mission_type)
            .join("mission-runtime.yaml"),
        root.join(".kittify")
            .join("missions")
            .join(mission_type)
            .join("mission-runtime.yaml"),
    ];
    if let Some(pack) = super::support::pack_root() {
        candidates.push(
            pack.join("missions")
                .join(mission_type)
                .join("mission-runtime.yaml"),
        );
    }
    for candidate in candidates {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(CompatError::coded(
        "TEMPLATE_NOT_FOUND",
        format!(
            "could not resolve mission-runtime.yaml for mission type '{mission_type}'; the built-in pack must be present"
        ),
    ))
}

fn mission_type_of(mission: &sk_mission::Mission) -> String {
    mission
        .mission_type()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("software-dev")
        .to_owned()
}

fn open_store(root: &Path) -> Result<RunStore, CompatError> {
    RunStore::open(root).map_err(map_store_error)
}

fn existing_run(
    root: &Path,
    mission: &sk_mission::Mission,
) -> Result<(RunStore, RunRef), CompatError> {
    let store = open_store(root)?;
    let run = store.run_for_mission(mission.slug().as_str(), Some(mission.id().as_str()))
        .map_err(map_store_error)?
        .ok_or_else(|| CompatError::coded("RUN_NOT_FOUND", "no durable run exists; query does not start one (full no-run mission preview is not implemented)"))?;
    Ok((store, run))
}

/// Start or resolve the mission's durable run.
pub fn ensure_run(
    root: &Path,
    mission: &sk_mission::Mission,
) -> Result<(RunStore, RunRef), CompatError> {
    let store = open_store(root)?;
    if let Some(run) = store
        .run_for_mission(mission.slug().as_str(), Some(mission.id().as_str()))
        .map_err(map_store_error)?
    {
        return Ok((store, run));
    }
    let template = runtime_template(root, &mission_type_of(mission))?;
    let request = StartRequest {
        mission_slug: mission.slug().as_str().to_owned(),
        mission_id: Some(mission.id().as_str().to_owned()),
        template_path: template,
        inputs: Default::default(),
        policy_snapshot: Default::default(),
    };
    let outcome = store.start(&request).map_err(map_store_error)?;
    Ok((store, outcome.run))
}

fn decision_kind(decision: &NextDecision) -> &'static str {
    decision.kind().as_str()
}

fn mission_state(state: &RunState) -> String {
    if state
        .completed_steps
        .iter()
        .any(|step| step == "retrospect")
    {
        "completed".to_owned()
    } else {
        "active".to_owned()
    }
}

fn step_of_decision(decision: &NextDecision) -> Option<String> {
    match decision {
        NextDecision::Step(step) => Some(step.step_id.clone()),
        NextDecision::DecisionRequired(required) => Some(match &required.requirement {
            sk_runtime::DecisionRequirement::PendingDecision { step_id, .. }
            | sk_runtime::DecisionRequirement::MissingRequiredInput { step_id, .. }
            | sk_runtime::DecisionRequirement::AuditCheckpoint { step_id, .. } => step_id.clone(),
        }),
        NextDecision::Blocked(_) => None,
        NextDecision::Terminal(_) => None,
    }
}

fn decision_id_of(decision: &NextDecision) -> Option<String> {
    decision.decision_id().map(str::to_owned)
}

/// Build the upstream-shaped `next` decision payload.
#[allow(clippy::too_many_arguments)]
pub fn decision_payload(
    mission: &sk_mission::Mission,
    state: &RunState,
    decision: &NextDecision,
    run: &RunRef,
    agent: Option<&str>,
    is_query: bool,
    prompt_file: Option<&Path>,
    answered: Option<&str>,
    answer: Option<&str>,
) -> Map<String, Value> {
    let mission_type = mission_type_of(mission);
    let step = step_of_decision(decision);
    // A bare call is structurally `kind: query`, never a runnable `step`
    // (the upstream `DecisionKind.query` invariant). It carries the preview
    // step and no prompt file.
    let kind = if is_query {
        "query".to_owned()
    } else {
        decision_kind(decision).to_owned()
    };
    let is_step = matches!(decision, NextDecision::Step(_));
    let mut payload = Map::new();
    payload.insert("kind".into(), json!(kind));
    payload.insert(
        "agent".into(),
        agent.map(|value| json!(value)).unwrap_or(Value::Null),
    );
    payload.insert("mission_slug".into(), json!(mission.slug().as_str()));
    payload.insert(
        "mission_number".into(),
        mission
            .mission_number()
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    payload.insert("mission_type".into(), json!(mission_type));
    payload.insert("mission".into(), json!(mission_type));
    let query_state = if state.issued_step_id.is_none() && state.completed_steps.is_empty() {
        "not_started".to_owned()
    } else {
        step.clone().unwrap_or_else(|| mission_state(state))
    };
    payload.insert(
        "mission_state".into(),
        json!(if is_query {
            query_state
        } else {
            mission_state(state)
        }),
    );
    payload.insert("timestamp".into(), json!(sk_kernel::clock::now_utc_iso()));
    payload.insert(
        "action".into(),
        if !is_query && is_step {
            step.clone().map(Value::String).unwrap_or(Value::Null)
        } else {
            Value::Null
        },
    );
    payload.insert("wp_id".into(), Value::Null);
    payload.insert("workspace_path".into(), Value::Null);
    payload.insert(
        "prompt_file".into(),
        if !is_query && is_step {
            prompt_file
                .map(|path| json!(path_string(path)))
                .unwrap_or(Value::Null)
        } else {
            Value::Null
        },
    );
    payload.insert(
        "reason".into(),
        match decision {
            NextDecision::Blocked(blocked) => json!(blocked.reason),
            NextDecision::Terminal(terminal) => json!(terminal.reason),
            _ => Value::Null,
        },
    );
    payload.insert("guard_failures".into(), json!([]));
    payload.insert("guard_failure_paths".into(), json!([]));
    payload.insert("progress".into(), Value::Null);
    payload.insert("origin".into(), json!("runtime"));
    payload.insert("run_id".into(), json!(run.run_id));
    payload.insert(
        "step_id".into(),
        if is_query {
            Value::Null
        } else {
            step.clone().map(Value::String).unwrap_or(Value::Null)
        },
    );
    payload.insert(
        "decision_id".into(),
        decision_id_of(decision)
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    payload.insert(
        "input_key".into(),
        decision
            .input_key()
            .map(|value| json!(value))
            .unwrap_or(Value::Null),
    );
    let (question, options) = match decision {
        NextDecision::DecisionRequired(required) => match &required.requirement {
            sk_runtime::DecisionRequirement::PendingDecision {
                question, options, ..
            } => (Some(question.clone()), options.clone()),
            sk_runtime::DecisionRequirement::MissingRequiredInput { question, .. } => {
                (Some(question.clone()), Vec::new())
            }
            sk_runtime::DecisionRequirement::AuditCheckpoint {
                question, options, ..
            } => (Some(question.clone()), options.clone()),
        },
        _ => (None, Vec::new()),
    };
    payload.insert(
        "question".into(),
        question.map(Value::String).unwrap_or(Value::Null),
    );
    payload.insert("options".into(), json!(options));
    payload.insert("is_query".into(), json!(is_query));
    payload.insert(
        "preview_step".into(),
        if is_query {
            step.map(Value::String).unwrap_or(Value::Null)
        } else {
            Value::Null
        },
    );
    if let Some(answered) = answered {
        payload.insert("answered".into(), json!(answered));
        payload.insert(
            "answer".into(),
            answer.map(|value| json!(value)).unwrap_or(Value::Null),
        );
    }
    payload
}

#[allow(clippy::too_many_arguments)]
fn emit_decision(
    mission: &sk_mission::Mission,
    run: &RunRef,
    query: &QueryResult,
    agent: Option<&str>,
    is_query: bool,
    answered: Option<&str>,
    answer: Option<&str>,
    json_output: bool,
) -> Result<Value, CompatError> {
    let mut payload = decision_payload(
        mission,
        &query.state,
        &query.decision,
        run,
        agent,
        is_query,
        query
            .issued_step
            .as_ref()
            .map(|step| step.prompt_file.as_path()),
        answered,
        answer,
    );
    // Additive branch contract for the mission's target branch (the upstream
    // next decision joins the same aliases through `_inject_branch_contract`).
    let target = mission
        .meta_field("target_branch")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    if !target.is_empty() {
        let current = super::support::current_branch(
            mission
                .spec_dir()
                .ancestors()
                .find(|path| path.join(".git").exists())
                .unwrap_or(mission.spec_dir()),
        )
        .unwrap_or_else(|| target.clone());
        inject_branch_contract(&mut payload, &target, &current, &target);
    }
    let value = Value::Object(payload);
    if json_output {
        emit_json_pretty(&value)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        println!(
            "{}: {}",
            value["kind"].as_str().unwrap_or("decision"),
            value["action"].as_str().unwrap_or("(no action)")
        );
        if let Some(reason) = value["reason"].as_str() {
            println!("  reason: {reason}");
        }
        if let Some(prompt) = value["prompt_file"].as_str() {
            println!("  prompt: {prompt}");
        }
    }
    Ok(value)
}

/// The `next` command.
#[allow(clippy::too_many_arguments)]
pub fn next(
    handle: Option<&str>,
    agent: Option<&str>,
    result: Option<&str>,
    answer: Option<&str>,
    decision_id: Option<&str>,
    json_output: bool,
) -> Result<i32, CompatError> {
    let root = project_root()?;
    let mission = resolve_mission(&root, handle)?;

    if answer.is_some() && result.is_none() {
        return Err(CompatError::domain(
            "--answer requires --result because query mode is read-only",
        ));
    }
    if answer.is_some() && agent.is_none() {
        return Err(CompatError::usage("--agent is required with --answer"));
    }
    if result.is_some() && agent.is_none() {
        return Err(CompatError::domain(
            "--agent is required when --result is provided",
        ));
    }
    let step_result = result.map(parse_result).transpose()?;
    let (store, run) = if step_result.is_some() {
        ensure_run(&root, &mission)?
    } else {
        existing_run(&root, &mission)?
    };

    let mut answered = None;
    if let Some(answer) = answer {
        let decision = match decision_id {
            Some(decision_id) => decision_id.to_owned(),
            None => {
                let query = store.query(&run).map_err(map_store_error)?;
                match query.pending.as_slice() {
                    [] => {
                        return Err(CompatError::domain(
                            "no pending decision to answer in this run",
                        ));
                    }
                    [only] => only.decision_id.clone(),
                    many => {
                        return Err(CompatError::coded(
                            "DECISION_SELECTION_REQUIRED",
                            "multiple decisions are pending; pass --decision-id",
                        )
                        .with(
                            "pending",
                            json!(
                                many.iter()
                                    .map(|decision| decision.decision_id.clone())
                                    .collect::<Vec<_>>()
                            ),
                        ));
                    }
                }
            }
        };
        let actor = ActorIdentity::new(agent.expect("checked above"), ActorType::Human);
        store
            .answer(
                &run,
                &AnswerRequest {
                    decision_id: decision.clone(),
                    answer: answer.to_owned(),
                    actor,
                },
            )
            .map_err(map_store_error)?;
        answered = Some(decision);
    }

    let query = if let Some(step_result) = step_result {
        store
            .advance(
                &run,
                &AdvanceRequest {
                    agent_id: agent.unwrap_or_default().to_owned(),
                    result: step_result,
                    expected_issued_step: None,
                },
            )
            .map_err(map_store_error)?;
        // Re-read the durable cursor so the reported issued step is the real
        // prompt artifact the advance wrote.
        store.query(&run).map_err(map_store_error)?
    } else {
        store.query(&run).map_err(map_store_error)?
    };

    let is_query = step_result.is_none();
    let value = emit_decision(
        &mission,
        &run,
        &query,
        agent,
        is_query,
        answered.as_deref(),
        answer,
        json_output,
    )?;
    let blocked = !is_query && value["kind"].as_str() == Some("blocked");
    Ok(if blocked { 1 } else { 0 })
}

/// Native extension: start a governed run.
pub fn start(
    handle: Option<&str>,
    template: Option<&Path>,
    json_output: bool,
) -> Result<Value, CompatError> {
    let root = project_root()?;
    let mission = resolve_mission(&root, handle)?;
    let store = open_store(&root)?;
    let template = match template {
        Some(path) => path.to_path_buf(),
        None => runtime_template(&root, &mission_type_of(&mission))?,
    };
    let outcome = store
        .start(&StartRequest {
            mission_slug: mission.slug().as_str().to_owned(),
            mission_id: Some(mission.id().as_str().to_owned()),
            template_path: template,
            inputs: Default::default(),
            policy_snapshot: Default::default(),
        })
        .map_err(map_store_error)?;
    let value = serde_json::to_value(&outcome)
        .map_err(|error| CompatError::domain(format!("could not serialize run: {error}")))?;
    if json_output {
        emit_json(&value)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        println!(
            "{} run {} for {}",
            if outcome.started { "Started" } else { "Reused" },
            outcome.run.run_id,
            mission.slug().as_str()
        );
    }
    Ok(value)
}

/// Native extension: query a governed run.
pub fn query(handle: Option<&str>, json_output: bool) -> Result<Value, CompatError> {
    let root = project_root()?;
    let mission = resolve_mission(&root, handle)?;
    let (store, run) = existing_run(&root, &mission)?;
    let result = store.query(&run).map_err(map_store_error)?;
    let value = serde_json::to_value(&result)
        .map_err(|error| CompatError::domain(format!("could not serialize run: {error}")))?;
    if json_output {
        emit_json(&value)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        println!("run {}: {:?}", run.run_id, result.decision.kind());
    }
    Ok(value)
}

/// Native extension: advance a governed run.
pub fn advance(
    handle: Option<&str>,
    agent: &str,
    result: &str,
    expected_issued_step: Option<&str>,
    json_output: bool,
) -> Result<Value, CompatError> {
    let root = project_root()?;
    let mission = resolve_mission(&root, handle)?;
    let (store, run) = ensure_run(&root, &mission)?;
    let outcome = store
        .advance(
            &run,
            &AdvanceRequest {
                agent_id: agent.to_owned(),
                result: parse_result(result)?,
                expected_issued_step: expected_issued_step.map(str::to_owned),
            },
        )
        .map_err(map_store_error)?;
    let value = serde_json::to_value(&outcome)
        .map_err(|error| CompatError::domain(format!("could not serialize run: {error}")))?;
    if json_output {
        emit_json(&value)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        println!("advanced run {}: {:?}", run.run_id, outcome.decision.kind());
    }
    Ok(value)
}

/// Native extension: answer a pending decision.
pub fn answer(
    handle: Option<&str>,
    decision_id: &str,
    answer: &str,
    actor: &str,
    json_output: bool,
) -> Result<Value, CompatError> {
    let root = project_root()?;
    let mission = resolve_mission(&root, handle)?;
    let (store, run) = ensure_run(&root, &mission)?;
    let outcome = store
        .answer(
            &run,
            &AnswerRequest {
                decision_id: decision_id.to_owned(),
                answer: answer.to_owned(),
                actor: ActorIdentity::new(actor, ActorType::Human),
            },
        )
        .map_err(map_store_error)?;
    let value = serde_json::to_value(&outcome)
        .map_err(|error| CompatError::domain(format!("could not serialize run: {error}")))?;
    if json_output {
        emit_json(&value)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        println!("answered {decision_id}");
    }
    Ok(value)
}

/// Native extension: reconcile a run's cursor and journal.
pub fn recover(handle: Option<&str>, json_output: bool) -> Result<Value, CompatError> {
    let root = project_root()?;
    let mission = resolve_mission(&root, handle)?;
    let (store, run) = ensure_run(&root, &mission)?;
    let report = store.recover(&run).map_err(map_store_error)?;
    let value = serde_json::to_value(&report)
        .map_err(|error| CompatError::domain(format!("could not serialize recovery: {error}")))?;
    if json_output {
        emit_json(&value)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        println!("run {} recovered", run.run_id);
    }
    Ok(value)
}

/// True when the decision is runnable (a step or an open decision).
pub fn decision_is_runnable(decision: &NextDecision) -> bool {
    matches!(
        decision.kind(),
        DecisionKind::Step | DecisionKind::DecisionRequired
    )
}
