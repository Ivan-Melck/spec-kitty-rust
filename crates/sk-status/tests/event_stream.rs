//! Read-only typed event-stream inspection contracts.
//!
//! These tests deliberately assert *inspection* facts (kinds, counts, preserved
//! payloads, line errors, torn-tail behaviour). They never assert a current
//! lane: the canonical reducer lives outside this crate.

use serde_json::json;
use sk_status::event_stream::{
    EventActor, EventKind, EventStreamError, EventStreamLimits, InspectedEvent, NonLaneCategory,
    inspect_event_stream, inspect_event_stream_with_limits,
};

const ULID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

fn transition_line(wp: &str, from: &str, to: &str, extra: &str) -> String {
    let trailing = if extra.is_empty() {
        String::new()
    } else {
        format!(",{extra}")
    };
    format!(
        r#"{{"event_id":"{ULID}","mission_slug":"001-mission","wp_id":"{wp}","from_lane":"{from}","to_lane":"{to}","at":"2026-11-02T00:00:00Z","actor":"agent-1","force":false,"execution_mode":"worktree"{trailing}}}"#
    )
}

fn annotation_line() -> String {
    format!(
        r#"{{"event_id":"{ULID}","kind":"annotation","wp_id":"WP01","at":"2026-11-02T00:00:00Z","actor":{{"role":"implementer","profile":null,"tool":"claude","model":null}},"delta":{{"shell_pid":42,"future_delta":{{"a":true}}}},"extra_top":{{"keep":true}}}}"#
    )
}

fn non_lane_lines() -> Vec<String> {
    vec![
        r#"{"event_type":"DecisionPointOpened","payload":{"id":"D1"},"extra":"x"}"#.to_owned(),
        r#"{"event_type":"SpecifyStarted","mission_slug":"001-mission"}"#.to_owned(),
        r#"{"type":"RetrospectiveCaptured","mission_slug":"001-mission"}"#.to_owned(),
        r#"{"event_name":"retrospective.captured","mission_slug":"001-mission"}"#.to_owned(),
        r#"{"event_type":"FutureExtensionType","payload":{"future":true}}"#.to_owned(),
        r#"{"event_type":null,"payload":{"future":true}}"#.to_owned(),
    ]
}

#[test]
fn parses_transitions_annotations_and_known_non_lane_rows() {
    let input = format!(
        "{}\n{}\n{}\n",
        transition_line(
            "WP01",
            "planned",
            "claimed",
            r#""future_extension":{"z":1}"#
        ),
        annotation_line(),
        non_lane_lines().join("\n")
    );

    let inspection = inspect_event_stream(input.as_bytes()).expect("stream should inspect");
    assert_eq!(inspection.counts.transitions, 1);
    assert_eq!(inspection.counts.annotations, 1);
    assert_eq!(inspection.counts.non_lane, 6);
    assert_eq!(inspection.counts.total, 8);
    assert_eq!(inspection.bytes_consumed, input.len());
    assert!(inspection.trailing_partial.is_none());

    let transition = match &inspection.events[0] {
        InspectedEvent::Transition(record) => record,
        other => panic!("expected transition, got {other:?}"),
    };
    assert_eq!(transition.wp_id, "WP01");
    assert_eq!(transition.from_lane.as_str(), "planned");
    assert_eq!(transition.to_lane.as_str(), "claimed");
    assert_eq!(transition.actor, EventActor::Text("agent-1".to_owned()));
    assert!(!transition.force);
    assert_eq!(transition.execution_mode, "worktree");
    assert_eq!(transition.mission_slug.as_deref(), Some("001-mission"));
    assert!(
        transition.extensions.contains_key("future_extension"),
        "unknown extension payload must survive"
    );
    assert!(!transition.extensions.contains_key("event_id"));
    assert_eq!(
        transition.raw.get("future_extension"),
        Some(&json!({"z": 1}))
    );

    let annotation = match &inspection.events[1] {
        InspectedEvent::Annotation(record) => record,
        other => panic!("expected annotation, got {other:?}"),
    };
    assert_eq!(annotation.wp_id, "WP01");
    assert_eq!(
        annotation.actor,
        EventActor::Structured(sk_status::event_stream::StructuredActor {
            role: Some("implementer".to_owned()),
            profile: None,
            tool: Some("claude".to_owned()),
            model: None,
            profile_source: None,
        })
    );
    assert_eq!(annotation.delta.get("shell_pid"), Some(&json!(42)));
    assert_eq!(
        annotation.delta.get("future_delta"),
        Some(&json!({"a": true})),
        "unknown delta payload must survive"
    );
    assert_eq!(
        annotation.extensions.get("extra_top"),
        Some(&json!({"keep": true}))
    );
    assert_eq!(annotation.kind(), EventKind::Annotation);

    let non_lane: Vec<&sk_status::event_stream::NonLaneRecord> = inspection
        .events
        .iter()
        .filter_map(|event| match event {
            InspectedEvent::NonLane(record) => Some(record),
            _ => None,
        })
        .collect();
    assert_eq!(non_lane.len(), 6);
    assert_eq!(non_lane[0].category, NonLaneCategory::DecisionMoment);
    assert_eq!(non_lane[1].category, NonLaneCategory::Lifecycle);
    assert_eq!(
        non_lane[2].category,
        NonLaneCategory::RetrospectiveLifecycle
    );
    assert_eq!(
        non_lane[3].category,
        NonLaneCategory::RetrospectiveEventName
    );
    assert_eq!(non_lane[4].category, NonLaneCategory::UnknownExtension);
    assert_eq!(non_lane[5].category, NonLaneCategory::UnknownExtension);
    assert_eq!(
        non_lane[0].event_type.as_deref(),
        Some("DecisionPointOpened")
    );
    assert_eq!(
        non_lane[3].event_name.as_deref(),
        Some("retrospective.captured")
    );
    assert!(non_lane[0].raw.contains_key("payload"));
    assert_eq!(non_lane[0].raw.get("extra"), Some(&json!("x")));

    let by_type = &inspection.counts.non_lane_by_type;
    assert_eq!(by_type.get("DecisionPointOpened"), Some(&1));
    assert_eq!(by_type.get("SpecifyStarted"), Some(&1));
    assert_eq!(by_type.get("RetrospectiveCaptured"), Some(&1));
    assert_eq!(by_type.get("retrospective.captured"), Some(&1));
    assert_eq!(by_type.get("FutureExtensionType"), Some(&1));
    assert_eq!(by_type.get("<non-string>"), Some(&1));
}

#[test]
fn kind_null_is_not_a_kind_and_does_not_shadow_a_transition() {
    let line = transition_line("WP01", "planned", "claimed", r#""kind":null"#);
    let inspection = inspect_event_stream(format!("{line}\n").as_bytes()).unwrap();
    assert_eq!(inspection.counts.transitions, 1);
    assert_eq!(inspection.counts.non_lane, 0);
}

#[test]
fn blank_lines_are_skipped_without_changing_counts_or_offsets() {
    let line = transition_line("WP01", "planned", "claimed", "");
    let input = format!("\n  \n{line}\n\n");
    let inspection = inspect_event_stream(input.as_bytes()).unwrap();
    assert_eq!(inspection.counts.total, 1);
    assert_eq!(inspection.bytes_consumed, input.len());
    assert!(inspection.trailing_partial.is_none());
}

#[test]
fn missing_file_equivalent_empty_input_is_an_empty_inspection() {
    let inspection = inspect_event_stream(b"").unwrap();
    assert_eq!(
        inspection.counts,
        sk_status::event_stream::EventCounts::default()
    );
    assert!(inspection.events.is_empty());
    assert!(inspection.trailing_partial.is_none());
}

#[test]
fn torn_tail_is_left_unconsumed_and_retried_not_reported_as_an_error() {
    let first = transition_line("WP01", "planned", "claimed", "");
    let second = transition_line("WP02", "claimed", "in_progress", "");
    let partial =
        r#"{"event_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV","mission_slug":"001-mission","wp_id":"WP03""#;

    let torn = format!("{first}\n{second}\n{partial}");
    let inspection = inspect_event_stream(torn.as_bytes()).unwrap();
    assert_eq!(inspection.counts.transitions, 2);
    assert_eq!(
        inspection.trailing_partial,
        Some(sk_status::event_stream::TrailingPartial {
            bytes: partial.len(),
            line: 3,
        })
    );
    assert_eq!(inspection.bytes_consumed, torn.len() - partial.len());

    let complete = format!(
        "{torn},\"from_lane\":\"planned\",\"to_lane\":\"claimed\",\"at\":\"2026-11-02T00:00:00Z\",\"actor\":\"agent-1\",\"force\":false,\"execution_mode\":\"worktree\"}}\n"
    );
    let inspection = inspect_event_stream(complete.as_bytes()).unwrap();
    assert_eq!(inspection.counts.transitions, 3);
    assert!(inspection.trailing_partial.is_none());
}

#[test]
fn complete_final_event_without_lf_is_not_dropped_as_a_torn_tail() {
    let line = transition_line("WP01", "planned", "claimed", "");
    let without_lf = inspect_event_stream(line.as_bytes()).unwrap();
    let with_lf = inspect_event_stream(format!("{line}\n").as_bytes()).unwrap();
    assert_eq!(without_lf.counts.total, 1);
    assert_eq!(without_lf.counts, with_lf.counts);
    assert_eq!(without_lf.bytes_consumed, line.len());
    assert!(without_lf.trailing_partial.is_none());
}

#[test]
fn malformed_complete_tail_is_not_mistaken_for_an_incomplete_tail() {
    assert!(matches!(
        inspect_event_stream(b"not json"),
        Err(EventStreamError::InvalidJson { .. })
    ));
    assert!(matches!(
        inspect_event_stream(b"42"),
        Err(EventStreamError::InvalidStructure { .. })
    ));
}

#[test]
fn unknown_kind_is_a_hard_error_naming_the_line() {
    let input = format!(
        "{}\n{{\"kind\":\"future-change\",\"wp_id\":\"WP01\"}}\n",
        transition_line("WP01", "planned", "claimed", "")
    );
    let error = inspect_event_stream(input.as_bytes()).unwrap_err();
    match &error {
        EventStreamError::UnknownEventKind { line, kind } => {
            assert_eq!(*line, 2);
            assert!(kind.contains("future-change"), "kind was {kind:?}");
        }
        other => panic!("expected unknown-kind error, got {other:?}"),
    }
    assert_eq!(
        error.to_string(),
        "Unknown event kind \"future-change\" on line 2"
    );
}

#[test]
fn invalid_json_and_non_object_lines_are_line_errors() {
    let valid = transition_line("WP01", "planned", "claimed", "");
    let error = inspect_event_stream(format!("{valid}\n{{oops\n").as_bytes()).unwrap_err();
    match &error {
        EventStreamError::InvalidJson { line, message } => {
            assert_eq!(*line, 2);
            assert!(!message.is_empty());
            assert!(
                error.to_string().contains("line 2"),
                "message was {}",
                error
            );
        }
        other => panic!("expected invalid-JSON error, got {other:?}"),
    }

    let error = inspect_event_stream(format!("{valid}\n[1,2,3]\n").as_bytes()).unwrap_err();
    match &error {
        EventStreamError::InvalidStructure { line, detail } => {
            assert_eq!(*line, 2);
            assert!(
                detail.contains("expected JSON object"),
                "detail was {detail}"
            );
        }
        other => panic!("expected structure error, got {other:?}"),
    }
}

#[test]
fn malformed_transition_rows_are_line_errors() {
    let cases = [
        // missing required from_lane
        (
            r#"{"event_id":"x","wp_id":"WP01","to_lane":"planned","at":"t","actor":"a","force":false,"execution_mode":"worktree"}"#,
            "from_lane",
        ),
        // persisted `doing` is not a canonical lane
        (
            r#"{"event_id":"x","wp_id":"WP01","from_lane":"doing","to_lane":"planned","at":"t","actor":"a","force":false,"execution_mode":"worktree"}"#,
            "Unknown lane: doing",
        ),
        // force must be a boolean
        (
            r#"{"event_id":"x","wp_id":"WP01","from_lane":"planned","to_lane":"claimed","at":"t","actor":"a","force":"yes","execution_mode":"worktree"}"#,
            "force",
        ),
        // review_result must be a complete object when present
        (
            r#"{"event_id":"x","wp_id":"WP01","from_lane":"in_review","to_lane":"planned","at":"t","actor":"a","force":false,"execution_mode":"worktree","review_result":{"reviewer":"r","verdict":"approved"}}"#,
            "reference",
        ),
    ];

    for (line, expected_fragment) in cases {
        let error = inspect_event_stream(format!("{line}\n").as_bytes()).unwrap_err();
        match error {
            EventStreamError::InvalidStructure { line, detail } => {
                assert_eq!(line, 1);
                assert!(
                    detail.contains(expected_fragment),
                    "detail {detail:?} did not contain {expected_fragment:?}"
                );
            }
            other => panic!("expected structure error, got {other:?}"),
        }
    }
}

#[test]
fn structured_actor_is_validated_but_unknown_top_level_fields_survive() {
    let five_key = format!(
        r#"{{"event_id":"{ULID}","kind":"annotation","wp_id":"WP01","at":"t","actor":{{"tool":"claude","role":"implementer","profile":"reviewer-renata","profile_source":"flag","model":null}},"delta":{{"note":"x"}},"future":1}}"#
    );
    let inspection = inspect_event_stream(format!("{five_key}\n").as_bytes()).unwrap();
    let record = match &inspection.events[0] {
        InspectedEvent::Annotation(record) => record,
        other => panic!("expected annotation, got {other:?}"),
    };
    assert_eq!(
        record.actor,
        EventActor::Structured(sk_status::event_stream::StructuredActor {
            role: Some("implementer".to_owned()),
            profile: Some("reviewer-renata".to_owned()),
            tool: Some("claude".to_owned()),
            model: None,
            profile_source: Some("flag".to_owned()),
        })
    );
    assert_eq!(record.extensions.get("future"), Some(&json!(1)));

    let bad_actor = format!(
        r#"{{"event_id":"{ULID}","kind":"annotation","wp_id":"WP01","at":"t","actor":{{"tool":"claude","role":"implementer","extra":"no"}},"delta":{{"note":"x"}}}}"#
    );
    match inspect_event_stream(format!("{bad_actor}\n").as_bytes()).unwrap_err() {
        EventStreamError::InvalidStructure { line: 1, detail } => {
            assert!(detail.contains("actor"), "detail was {detail}");
        }
        other => panic!("expected structure error, got {other:?}"),
    }
}

#[test]
fn annotation_event_id_must_be_a_ulid() {
    let line = r#"{"event_id":"not-a-ulid","kind":"annotation","wp_id":"WP01","at":"t","actor":"a","delta":{"note":"x"}}"#;
    match inspect_event_stream(format!("{line}\n").as_bytes()).unwrap_err() {
        EventStreamError::InvalidStructure { line: 1, detail } => {
            assert!(detail.contains("ULID"), "detail was {detail}");
        }
        other => panic!("expected structure error, got {other:?}"),
    }
}

#[test]
fn input_line_and_event_bounds_are_enforced() {
    let line = transition_line("WP01", "planned", "claimed", "");

    let too_small_for_input = EventStreamLimits {
        max_input_bytes: line.len() - 1,
        ..EventStreamLimits::default()
    };
    match inspect_event_stream_with_limits(format!("{line}\n").as_bytes(), &too_small_for_input)
        .unwrap_err()
    {
        EventStreamError::InputTooLarge { bytes, limit } => {
            assert!(bytes > limit);
            assert_eq!(limit, line.len() - 1);
        }
        other => panic!("expected input-too-large, got {other:?}"),
    }

    let too_small_for_line = EventStreamLimits {
        max_line_bytes: line.len() - 1,
        ..EventStreamLimits::default()
    };
    match inspect_event_stream_with_limits(format!("{line}\n").as_bytes(), &too_small_for_line)
        .unwrap_err()
    {
        EventStreamError::LineTooLong {
            line: 1,
            bytes,
            limit,
        } => {
            assert_eq!(bytes, line.len());
            assert_eq!(limit, line.len() - 1);
        }
        other => panic!("expected line-too-long, got {other:?}"),
    }

    let only_one = EventStreamLimits {
        max_events: 1,
        ..EventStreamLimits::default()
    };
    match inspect_event_stream_with_limits(format!("{line}\n{line}\n").as_bytes(), &only_one)
        .unwrap_err()
    {
        EventStreamError::TooManyEvents { limit } => assert_eq!(limit, 1),
        other => panic!("expected too-many-events, got {other:?}"),
    }

    // Exactly at the bound is allowed.
    let inspection =
        inspect_event_stream_with_limits(format!("{line}\n").as_bytes(), &only_one).unwrap();
    assert_eq!(inspection.counts.total, 1);
}
