//! Canonical status routes: `agent status validate` and
//! `agent status materialize`.
//!
//! Both routes are read/compute-first. `materialize` computes the snapshot with
//! the canonical reducer and writes the exact upstream `status.json` bytes
//! under the canonical Mission status lock; `validate` never writes.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use sk_status::{Lane, Snapshot, TransitionError};

use super::support::{
    CompatError, canonical_lanes, emit_json, events_path, project_root, resolve_mission,
};
use sk_mission::Mission;

/// Upstream requires an explicit `--mission` for status routes.
pub fn require_mission(root: &Path, handle: Option<&str>) -> Result<Mission, CompatError> {
    match handle.map(str::trim).filter(|value| !value.is_empty()) {
        Some(handle) => resolve_mission(root, Some(handle)),
        None => Err(CompatError::domain("--mission <slug> is required")),
    }
}

fn snapshot_bytes(snapshot: &Snapshot) -> Result<String, CompatError> {
    sk_status::render_snapshot_json(snapshot)
        .map_err(|error| CompatError::domain(format!("could not render status.json: {error}")))
}

/// Materialize `status.json` from the canonical event log.
pub fn materialize(
    root: &Path,
    handle: Option<&str>,
    json_output: bool,
) -> Result<(), CompatError> {
    let mission = require_mission(root, handle)?;
    if mission
        .meta_field("coordination_branch")
        .and_then(Value::as_str)
        .is_some_and(|branch| !branch.trim().is_empty())
    {
        return Err(CompatError::coded(
            "COORDINATION_SURFACE_UNAVAILABLE",
            "coordination-routed materialization requires the canonical coordination write surface; refusing to write primary status.json",
        ));
    }
    let events = events_path(&mission);
    match std::fs::metadata(&events) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => {
            return Err(CompatError::domain(format!(
                "No event log found at {}\nRun 'spec-kitty agent status emit' to create the first event, or run a migration to initialize the event log.",
                events.display()
            )));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(CompatError::domain(format!(
                "No event log found at {}\nRun 'spec-kitty agent status emit' to create the first event, or run a migration to initialize the event log.",
                events.display()
            )));
        }
        Err(error) => {
            return Err(CompatError::domain(format!(
                "could not inspect {}: {error}",
                events.display()
            )));
        }
    }

    let lock = sk_status::mutation::acquire_mission_lock(
        mission.spec_dir(),
        Some(root),
        sk_kernel::LockWait::Bounded(Duration::from_secs(10)),
    )
    .map_err(|error| CompatError::domain(format!("could not acquire the status lock: {error}")))?;
    let snapshot = sk_status::materialize_dir(mission.spec_dir()).map_err(|error| {
        CompatError::domain(format!(
            "could not materialize {}: {error}",
            mission.spec_dir().display()
        ))
    })?;
    let bytes = snapshot_bytes(&snapshot)?;
    let destination = mission.spec_dir().join(sk_status::SNAPSHOT_FILENAME);
    let current = std::fs::read_to_string(&destination).ok();
    if current.as_deref() != Some(bytes.as_str()) {
        sk_kernel::atomic_write(&destination, bytes.as_bytes()).map_err(|error| {
            CompatError::domain(format!(
                "could not write {}: {error}",
                destination.display()
            ))
        })?;
    }
    drop(lock);

    if json_output {
        emit_json(&snapshot.to_json_value())
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        let lane_parts: Vec<String> = snapshot
            .summary
            .iter()
            .filter(|(_, count)| **count > 0)
            .map(|(lane, count)| format!("{lane}: {count}"))
            .collect();
        println!(
            "Materialized {}: {} events -> {} WPs",
            mission.slug().as_str(),
            snapshot.event_count,
            snapshot.work_packages.len()
        );
        if !lane_parts.is_empty() {
            println!("  {}", lane_parts.join(", "));
        }
    }
    Ok(())
}

/// The validation result upstream serializes.
#[derive(Default)]
struct Validation {
    errors: Vec<String>,
    warnings: Vec<String>,
}

fn transition_findings(rows: &[Value], validation: &mut Validation) {
    let mut current: BTreeMap<String, Lane> = BTreeMap::new();
    let mut last_at: BTreeMap<String, String> = BTreeMap::new();
    for (index, row) in rows.iter().enumerate() {
        let line = index + 1;
        let Some(object) = row.as_object() else {
            continue;
        };
        if object.get("kind").is_some_and(|value| !value.is_null()) {
            continue;
        }
        let Some(wp_id) = object.get("wp_id").and_then(Value::as_str) else {
            continue;
        };
        let Some(to) = object.get("to_lane").and_then(Value::as_str) else {
            continue;
        };
        let Ok(target) = Lane::parse_canonical(&sk_status::resolve_lane_alias(to)) else {
            // The schema auditor already reports a non-canonical lane.
            continue;
        };
        let from = current.get(wp_id).copied().unwrap_or(Lane::Planned);
        let actor = object.get("actor").and_then(Value::as_str);
        let reason = object.get("reason").and_then(Value::as_str);
        let review_ref = object.get("review_ref").and_then(Value::as_str);
        let review = object
            .get("evidence")
            .and_then(|evidence| evidence.get("review"))
            .map(|review| sk_status::ReviewApproval {
                reviewer: review
                    .get("reviewer")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                verdict: review
                    .get("verdict")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                reference: review
                    .get("reference")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            });
        let guard = sk_status::GuardContext {
            actor: actor.map(str::to_owned),
            workspace_context: object
                .get("workspace_context")
                .and_then(Value::as_str)
                .map(str::to_owned),
            subtasks_complete: object.get("subtasks_complete").and_then(Value::as_bool),
            implementation_evidence_present: object
                .get("implementation_evidence_present")
                .and_then(Value::as_bool),
            reason: reason.map(str::to_owned),
            review_ref: review_ref.map(str::to_owned),
            evidence: review.map(|review| sk_status::DoneEvidence {
                review: Some(review),
            }),
            force: object
                .get("force")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            review_result: None,
            current_actor: None,
            dependency_ready: None,
        };
        if let Err(error) = sk_status::validate_transition(from, target, &guard) {
            // A review result is stored as a nested object on the row; retry
            // with it before declaring the edge illegal.
            let structured = object
                .get("review_result")
                .and_then(Value::as_object)
                .map(|value| sk_status::ReviewResult {
                    reviewer: value
                        .get("reviewer")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    verdict: value
                        .get("verdict")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    reference: value
                        .get("reference")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    feedback_path: value
                        .get("feedback_path")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                });
            if structured.is_none() {
                validation.errors.push(format!(
                    "line {line}: {wp_id}: {}",
                    display_transition(&error, from, target)
                ));
            } else {
                let retry = sk_status::GuardContext {
                    review_result: structured,
                    ..guard
                };
                if let Err(error) = sk_status::validate_transition(from, target, &retry) {
                    validation.errors.push(format!(
                        "line {line}: {wp_id}: {}",
                        display_transition(&error, from, target)
                    ));
                }
            }
        }

        if let Some(at) = object.get("at").and_then(Value::as_str) {
            if let Some(previous) = last_at.get(wp_id)
                && at < previous.as_str()
            {
                validation.warnings.push(format!(
                    "line {line}: {wp_id}: timestamp {at} is earlier than the previous event {previous} (clock skew; the reducer re-orders causally)"
                ));
            }
            last_at.insert(wp_id.to_owned(), at.to_owned());
        }
        current.insert(wp_id.to_owned(), target);
    }
}

fn display_transition(error: &TransitionError, from: Lane, to: Lane) -> String {
    match error {
        TransitionError::IllegalEdge { .. } => format!(
            "illegal transition {from} -> {to} (for the recorded state); {}",
            error
        ),
        _ => error.to_string(),
    }
}

/// Validate canonical status model integrity.
pub fn validate(root: &Path, handle: Option<&str>, json_output: bool) -> Result<bool, CompatError> {
    let mission = require_mission(root, handle)?;
    if !mission.spec_dir().is_dir() {
        return Err(CompatError::domain(format!(
            "Mission directory not found: {}",
            mission.spec_dir().display()
        )));
    }

    let mut validation = Validation::default();
    let events = events_path(&mission);
    let rows = match sk_status::read_raw_events(&events) {
        Ok(rows) => rows,
        Err(sk_status::SnapshotError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
            Vec::new()
        }
        Err(error) => {
            validation
                .errors
                .push(format!("status.events.jsonl is corrupt: {error}"));
            Vec::new()
        }
    };

    let has_events = !rows.is_empty();
    if has_events {
        let audit = sk_status::audit_event_rows(&rows);
        for finding in &audit.findings {
            validation
                .errors
                .push(format!("{}: {}", finding.code, finding.message));
        }
        transition_findings(&rows, &mut validation);

        // Materialization drift: the event log is the sole authority, so a
        // missing or divergent status.json is an error.
        match sk_status::materialize_dir(mission.spec_dir()) {
            Ok(snapshot) => {
                let expected = snapshot_bytes(&snapshot)?;
                let snapshot_path = mission.spec_dir().join(sk_status::SNAPSHOT_FILENAME);
                match std::fs::read_to_string(&snapshot_path) {
                    Ok(current) if current == expected => {}
                    Ok(_) => validation.errors.push(format!(
                        "{} is stale relative to status.events.jsonl; run 'spec-kitty agent status materialize'",
                        snapshot_path.display()
                    )),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        validation.errors.push(format!(
                            "{} is missing; run 'spec-kitty agent status materialize'",
                            snapshot_path.display()
                        ));
                    }
                    Err(error) => validation.errors.push(format!(
                        "could not read {}: {error}",
                        snapshot_path.display()
                    )),
                }
            }
            Err(error) => validation.errors.push(format!(
                "could not materialize the status snapshot: {error}"
            )),
        }
    }

    let passed = validation.errors.is_empty();
    if json_output {
        emit_json(&json!({
            "mission_slug": mission.slug().as_str(),
            "passed": passed,
            "errors": validation.errors,
            "warnings": validation.warnings,
            "error_count": validation.errors.len(),
            "warning_count": validation.warnings.len(),
        }))
        .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        println!("Status Validation: {}", mission.slug().as_str());
        if validation.errors.is_empty() && validation.warnings.is_empty() {
            println!("No events to validate.");
        }
        for error in &validation.errors {
            println!("  error: {error}");
        }
        for warning in &validation.warnings {
            println!("  warning: {warning}");
        }
        println!("Result: {}", if passed { "PASS" } else { "FAIL" });
    }
    Ok(passed)
}

/// The canonical lane map used by task reads.
pub fn lanes(mission: &Mission) -> Result<BTreeMap<String, String>, CompatError> {
    canonical_lanes(mission)
}

/// Diagnose a missing project root with the upstream message.
pub fn root_or_error() -> Result<PathBuf, CompatError> {
    project_root()
}
