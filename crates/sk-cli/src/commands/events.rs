//! Read-only `status.events.jsonl` statistics.
//!
//! The inspector is deliberately not a reducer: it never reports a current
//! lane or a last-event-wins state. That authority belongs to the external
//! events package.

use std::path::Path;

use serde_json::{Value, json};
use sk_kernel::read_bounded;
use sk_status::{
    EventActor, EventStreamLimits, NonLaneCategory, StructuredActor, inspect_event_stream,
};

use crate::commands::path_string;
use crate::output::{CliError, CommandData};

/// Inspect one event stream and report typed statistics.
pub fn inspect(path: &Path) -> Result<CommandData, CliError> {
    let limits = EventStreamLimits::default();
    let bytes = read_bounded(path, limits.max_input_bytes as u64).map_err(|error| {
        CliError::domain(format!(
            "could not read the event stream {}: {error}",
            path.display()
        ))
    })?;
    let inspection = inspect_event_stream(&bytes).map_err(|error| {
        CliError::domain(format!(
            "could not inspect the event stream {}: {error}",
            path.display()
        ))
    })?;

    let events: Vec<Value> = inspection.events.iter().map(event_value).collect();
    let trailing = inspection.trailing_partial.map(|partial| {
        json!({
            "bytes": partial.bytes,
            "line": partial.line,
        })
    });

    Ok(CommandData::success(json!({
        "path": path_string(path),
        "statistics_only": true,
        "reduction": "none",
        "bytes_consumed": inspection.bytes_consumed,
        "counts": {
            "transitions": inspection.counts.transitions,
            "annotations": inspection.counts.annotations,
            "non_lane": inspection.counts.non_lane,
            "total": inspection.counts.total,
            "non_lane_by_type": inspection.counts.non_lane_by_type,
        },
        "trailing_partial": trailing,
        "events": events,
        "note": "This is a typed stream inspection, not a reducer. Current lane is owned by the external events package.",
    })))
}

fn event_value(event: &sk_status::InspectedEvent) -> Value {
    match event {
        sk_status::InspectedEvent::Transition(record) => json!({
            "kind": "transition",
            "event_id": &record.event_id,
            "mission_slug": &record.mission_slug,
            "mission_id": &record.mission_id,
            "wp_id": &record.wp_id,
            "from_lane": record.from_lane.as_str(),
            "to_lane": record.to_lane.as_str(),
            "at": &record.at,
            "actor": actor_value(&record.actor),
            "force": record.force,
            "execution_mode": &record.execution_mode,
            "reason": &record.reason,
            "review_ref": &record.review_ref,
            "review_result": record.review_result.as_ref().map(|result| json!({
                "reviewer": &result.reviewer,
                "verdict": &result.verdict,
                "reference": &result.reference,
                "feedback_path": &result.feedback_path,
            })),
        }),
        sk_status::InspectedEvent::Annotation(record) => json!({
            "kind": "annotation",
            "event_id": &record.event_id,
            "wp_id": &record.wp_id,
            "at": &record.at,
            "actor": actor_value(&record.actor),
        }),
        sk_status::InspectedEvent::NonLane(record) => json!({
            "kind": "non_lane",
            "category": category(record.category),
            "event_type": &record.event_type,
            "event_name": &record.event_name,
        }),
    }
}

fn actor_value(actor: &EventActor) -> Value {
    match actor {
        EventActor::Text(text) => json!({
            "kind": "text",
            "value": text,
        }),
        EventActor::Structured(StructuredActor {
            role,
            profile,
            tool,
            model,
            profile_source,
        }) => json!({
            "kind": "structured",
            "role": role,
            "profile": profile,
            "tool": tool,
            "model": model,
            "profile_source": profile_source,
        }),
    }
}

fn category(category: NonLaneCategory) -> &'static str {
    match category {
        NonLaneCategory::Lifecycle => "lifecycle",
        NonLaneCategory::DecisionMoment => "decision_moment",
        NonLaneCategory::RetrospectiveLifecycle => "retrospective_lifecycle",
        NonLaneCategory::RetrospectiveEventName => "retrospective_event_name",
        NonLaneCategory::UnknownExtension => "unknown_extension",
    }
}
