//! `3_3_0_op_record_schema_v2`: rewrite legacy `kitty-ops/*.jsonl` records.
//!
//! Ports `specify_cli/upgrade/migrations/m_3_3_0_op_record_schema_v2.py` on top
//! of the v2 models that live in [`sk_invocation`] (`OpStartedEvent` /
//! `OpCompletedEvent`, their validators and `to_jsonl_line`). This module never
//! re-implements the record wire format: a mapped line is produced by the one
//! model writer, and a line the model cannot represent makes its file a
//! reported deletion instead of a silently lossy rewrite.
//!
//! Excluded files (`ops-index.jsonl`, `lifecycle.jsonl`,
//! `propagation-errors.jsonl`) are never opened: they carry different schemas.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use sk_invocation::{
    ClosedBy, InvocationId, ModeOfWork, OpCompletedEvent, OpCompletedFields, OpOutcome,
    OpStartedEvent, OpStartedFields,
};

use crate::error::InstallError;
use crate::upgrade::MigrationOutcome;

const OPS_DIR: &str = "kitty-ops";
const EXCLUDED_FILES: [&str; 3] = [
    "ops-index.jsonl",
    "lifecycle.jsonl",
    "propagation-errors.jsonl",
];

enum FilePlan {
    Skip,
    Rewrite {
        lines: Vec<String>,
        warnings: Vec<String>,
    },
    Delete {
        reason: String,
    },
}

fn eligible_files(project_root: &Path) -> Vec<PathBuf> {
    let directory = project_root.join(OPS_DIR);
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "jsonl")
                && path
                    .file_name()
                    .is_some_and(|name| !EXCLUDED_FILES.contains(&name.to_string_lossy().as_ref()))
        })
        .collect();
    files.sort();
    files
}

fn read_lines(path: &Path) -> Option<Vec<String>> {
    let text = crate::migrations::completion_legacy::paths::read_text(path)?;
    Some(
        text.lines()
            .filter(|line| !line.trim().is_empty())
            .map(str::to_string)
            .collect(),
    )
}

fn str_or_empty(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) => value.clone(),
        _ => String::new(),
    }
}

fn optional_str(value: Option<&Value>) -> Result<Option<String>, ()> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(()),
    }
}

fn is_v2_started(data: &Map<String, Value>) -> bool {
    OpStartedEvent::from_object(data).is_ok()
}

fn is_v2_completed(data: &Map<String, Value>) -> bool {
    OpCompletedEvent::from_object(data).is_ok()
}

fn map_started(data: &Map<String, Value>, invocation_id: &str) -> Option<OpStartedEvent> {
    let action = match data.get("action") {
        Some(Value::String(value)) if !value.is_empty() => value.clone(),
        _ => "unrecorded".to_string(),
    };
    let actor = match data.get("actor") {
        Some(Value::String(value)) if !value.is_empty() => value.clone(),
        _ => "unrecorded".to_string(),
    };
    let mode = match data.get("mode_of_work") {
        Some(Value::String(value)) if !value.is_empty() => ModeOfWork::parse(value)?,
        _ => ModeOfWork::parse("task_execution").expect("known mode"),
    };
    let governance_context_available = match data.get("governance_context_available") {
        None => true,
        Some(Value::Bool(value)) => *value,
        Some(_) => return None,
    };
    let router_confidence = match optional_str(data.get("router_confidence")) {
        Ok(value) => value,
        Err(()) => return None,
    };
    let mission_id = match optional_str(data.get("mission_id")) {
        Ok(value) => value,
        Err(()) => return None,
    };
    let wp_id = match optional_str(data.get("wp_id")) {
        Ok(value) => value,
        Err(()) => return None,
    };
    let model_id = match optional_str(data.get("model_id")) {
        Ok(value) => value,
        Err(()) => return None,
    };
    OpStartedEvent::new(OpStartedFields {
        invocation_id: InvocationId::parse(invocation_id).ok()?,
        profile_id: match data.get("profile_id") {
            Some(Value::String(value)) => value.clone(),
            _ => return None,
        },
        action,
        request_text: str_or_empty(data.get("request_text")),
        actor,
        mode_of_work: mode,
        governance_context_hash: str_or_empty(data.get("governance_context_hash")),
        governance_context_available,
        router_confidence,
        started_at: str_or_empty(data.get("started_at")),
        mission_id,
        wp_id,
        model_id,
    })
    .ok()
}

fn map_completed(
    data: &Map<String, Value>,
    file_name: &str,
    invocation_id: &str,
    started_at: &str,
    warnings: &mut Vec<String>,
) -> Option<OpCompletedEvent> {
    let mut completed_at = str_or_empty(data.get("completed_at"));
    if completed_at.is_empty() {
        completed_at = started_at.to_string();
        warnings.push(format!(
            "{file_name}: completed_at missing; fell back to started_at"
        ));
    }
    let outcome = match data.get("outcome") {
        Some(Value::String(value)) if matches!(value.as_str(), "done" | "failed" | "abandoned") => {
            OpOutcome::parse(value)?
        }
        _ => OpOutcome::parse("abandoned").expect("known outcome"),
    };
    let line_invocation = match data.get("invocation_id") {
        Some(Value::String(value)) if !value.is_empty() => value.clone(),
        _ => invocation_id.to_string(),
    };
    let evidence_ref = match optional_str(data.get("evidence_ref")) {
        Ok(value) => value,
        Err(()) => return None,
    };
    OpCompletedEvent::new(OpCompletedFields {
        invocation_id: InvocationId::parse(&line_invocation).ok()?,
        completed_at,
        outcome,
        closed_by: ClosedBy::parse("agent").expect("known closer"),
        evidence_ref,
    })
    .ok()
}

fn plan_file(path: &Path) -> FilePlan {
    let Some(lines) = read_lines(path) else {
        return FilePlan::Delete {
            reason: "unreadable or empty file".to_string(),
        };
    };
    if lines.is_empty() {
        return FilePlan::Delete {
            reason: "unreadable or empty file".to_string(),
        };
    }
    let mut parsed: Vec<Map<String, Value>> = Vec::with_capacity(lines.len());
    for line in &lines {
        let Ok(Value::Object(data)) = serde_json::from_str::<Value>(line) else {
            return FilePlan::Delete {
                reason: "unparseable JSON line".to_string(),
            };
        };
        parsed.push(data);
    }
    let Some(started_index) = parsed
        .iter()
        .position(|data| data.get("event").and_then(Value::as_str) == Some("started"))
    else {
        return FilePlan::Delete {
            reason: "missing started event".to_string(),
        };
    };
    let started = &parsed[started_index];
    let invocation_id = str_or_empty(started.get("invocation_id"));
    let profile_id = str_or_empty(started.get("profile_id"));
    if invocation_id.is_empty() || profile_id.is_empty() {
        return FilePlan::Delete {
            reason: "started event lacks invocation_id/profile_id".to_string(),
        };
    }
    let completed: Vec<&Map<String, Value>> = parsed
        .iter()
        .filter(|data| data.get("event").and_then(Value::as_str) == Some("completed"))
        .collect();
    if is_v2_started(started) && completed.iter().all(|data| is_v2_completed(data)) {
        return FilePlan::Skip;
    }
    let mut warnings = Vec::new();
    let started_at = str_or_empty(started.get("started_at"));
    let Some(started_event) = map_started(started, &invocation_id) else {
        return FilePlan::Delete {
            reason: "started event not representable as v2".to_string(),
        };
    };
    let mut new_lines = Vec::with_capacity(lines.len());
    for (index, (data, original)) in parsed.iter().zip(lines.iter()).enumerate() {
        match data.get("event").and_then(Value::as_str) {
            Some("started") if index == started_index => {
                new_lines.push(started_event.to_jsonl_line());
            }
            Some("completed") => {
                if is_v2_completed(data) {
                    new_lines.push(original.clone());
                    continue;
                }
                let Some(mapped) = map_completed(
                    data,
                    &path.file_name().unwrap_or_default().to_string_lossy(),
                    &invocation_id,
                    &started_at,
                    &mut warnings,
                ) else {
                    return FilePlan::Delete {
                        reason: "completed event not representable as v2".to_string(),
                    };
                };
                new_lines.push(mapped.to_jsonl_line());
            }
            _ => new_lines.push(original.clone()),
        }
    }
    FilePlan::Rewrite {
        lines: new_lines,
        warnings,
    }
}

/// `detect`: any per-op file needs rewriting or deletion.
pub fn detect(project_root: &Path) -> Result<bool, InstallError> {
    Ok(eligible_files(project_root)
        .iter()
        .any(|path| !matches!(plan_file(path), FilePlan::Skip)))
}

/// `can_apply`: the migration only needs a `kitty-ops/` directory.
pub fn can_apply(project_root: &Path) -> Result<Option<String>, InstallError> {
    if project_root.join(OPS_DIR).is_dir() {
        Ok(None)
    } else {
        Ok(Some("kitty-ops/ directory does not exist".to_string()))
    }
}

/// `apply`: rewrite or delete each eligible legacy file; v2 files are skipped.
pub fn apply(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    let mut deleted = Vec::new();
    for path in eligible_files(project_root) {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        match plan_file(&path) {
            FilePlan::Skip => {}
            FilePlan::Delete { reason } => {
                std::fs::remove_file(&path)
                    .or_else(|error| {
                        if error.kind() == std::io::ErrorKind::NotFound {
                            Ok(())
                        } else {
                            Err(error)
                        }
                    })
                    .map_err(|error| InstallError::io("delete", &path, error))?;
                deleted.push(name.clone());
                changes.push(format!("Deleted unsalvageable {name} ({reason})"));
            }
            FilePlan::Rewrite {
                lines,
                warnings: own,
            } => {
                let text: String = lines.into_iter().map(|line| format!("{line}\n")).collect();
                sk_kernel::atomic_write(&path, text.as_bytes()).map_err(|error| {
                    InstallError::io("write", &path, std::io::Error::other(error))
                })?;
                changes.push(format!("Rewrote {name} to v2 schema"));
                warnings.extend(own);
            }
        }
    }
    if !deleted.is_empty() {
        warnings.push(format!(
            "Deleted {} unsalvageable Op record file(s): {}",
            deleted.len(),
            deleted.join(", ")
        ));
    }
    Ok(MigrationOutcome {
        changed: !changes.is_empty(),
        changes,
        warnings,
        ..crate::upgrade::MigrationOutcome::default()
    })
}
