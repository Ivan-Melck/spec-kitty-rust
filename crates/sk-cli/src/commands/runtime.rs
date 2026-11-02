//! Pure mission-runtime planner query.
//!
//! This is not `spec-kitty next`: it creates no run, persists no snapshot,
//! emits no prompt, resolves no workspace and applies no gate. It answers the
//! deterministic planning question for an explicit frozen template and run
//! snapshot.

use std::path::Path;

use serde_json::{Value, json};
use sk_kernel::read_bounded;
use sk_runtime::{DecisionKind, RunSnapshot, load_template, plan_next};

use crate::commands::path_string;
use crate::output::{CliError, CommandData, EXIT_BLOCKED};

const MAX_STATE_BYTES: u64 = 4 * 1024 * 1024;

/// Plan the next decision for an explicit template and frozen snapshot.
pub fn query(
    template_path: &Path,
    state_path: &Path,
    current_template_hash: Option<&str>,
) -> Result<CommandData, CliError> {
    let template = load_template(template_path).map_err(|error| {
        CliError::domain(format!(
            "could not load the mission template {}: {error}",
            template_path.display()
        ))
    })?;

    let bytes = read_bounded(state_path, MAX_STATE_BYTES).map_err(|error| {
        CliError::domain(format!(
            "could not read the run snapshot {}: {error}",
            state_path.display()
        ))
    })?;
    let text = std::str::from_utf8(&bytes).map_err(|error| {
        CliError::domain(format!(
            "the run snapshot {} is not valid UTF-8: {error}",
            state_path.display()
        ))
    })?;
    let snapshot: RunSnapshot = serde_json::from_str(text).map_err(|error| {
        CliError::domain(format!(
            "the run snapshot {} is not a valid RunSnapshot: {error}",
            state_path.display()
        ))
    })?;

    let hash = current_template_hash.unwrap_or("");
    let decision = plan_next(&template, &snapshot, hash)
        .map_err(|error| CliError::domain(format!("the planner refused the query: {error}")))?;
    let decision_value: Value = serde_json::to_value(&decision).map_err(|error| {
        CliError::internal(format!("could not serialize the planner decision: {error}"))
    })?;
    let blocked = decision.kind() == DecisionKind::Blocked;

    let data = json!({
        "mode": "pure_planner_query",
        "decision_kind": decision.kind().as_str(),
        "blocked": blocked,
        "advances_run": false,
        "writes": false,
        "creates_run": false,
        "prompt_emitted": false,
        "workspace_resolved": false,
        "template": path_string(template_path),
        "state": path_string(state_path),
        "current_template_hash": current_template_hash,
        "decision": decision_value,
    });

    Ok(if blocked {
        CommandData::with_exit(data, EXIT_BLOCKED)
    } else {
        CommandData::success(data)
    })
}
