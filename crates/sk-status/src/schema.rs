//! Schema auditing for the status diary and its derived snapshot.
//!
//! The auditor is intentionally structural and non-vacuous: it checks the
//! canonical display summary, every runtime slot a gate consumes, the schema
//! generation's versioned field, and (for raw rows) the event wire schema.
//! It reports findings; it never mutates or writes.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::lane::Lane;
use crate::replay::{DISPLAY_LANES, is_safe_path_segment};
use crate::snapshot::{
    IMPLEMENTER_OF_RECORD_SCHEMA_VERSION, IMPLEMENTER_OF_RECORD_SLOT,
    LEGACY_SNAPSHOT_SCHEMA_VERSION, Snapshot,
};

/// One audit finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaFinding {
    /// Stable machine-readable finding code.
    pub code: &'static str,
    /// Human-readable explanation.
    pub message: String,
}

/// The aggregate result of one audit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SchemaAudit {
    pub findings: Vec<SchemaFinding>,
}

impl SchemaAudit {
    fn push(&mut self, code: &'static str, message: impl Into<String>) {
        self.findings.push(SchemaFinding {
            code,
            message: message.into(),
        });
    }

    /// True when no finding was raised.
    pub fn is_clean(&self) -> bool {
        self.findings.is_empty()
    }

    /// All finding codes, in report order.
    pub fn codes(&self) -> Vec<&'static str> {
        self.findings.iter().map(|finding| finding.code).collect()
    }
}

/// The stateless schema auditor entry point.
///
/// The free functions [`audit_snapshot`] and [`audit_event_rows`] carry the
/// implementation; this wrapper exists so CLI integration can hold one named
/// auditor instead of importing two loose functions.
#[derive(Debug, Clone, Copy, Default)]
pub struct SchemaAuditor;

impl SchemaAuditor {
    /// Create an auditor.
    pub const fn new() -> Self {
        Self
    }

    /// Audit a reduced/materialized snapshot.
    pub fn audit_snapshot(&self, snapshot: &Snapshot) -> SchemaAudit {
        audit_snapshot(snapshot)
    }

    /// Audit raw diary rows against the event wire schema.
    pub fn audit_event_rows(&self, rows: &[Value]) -> SchemaAudit {
        audit_event_rows(rows)
    }
}

/// Audit the structural schema of a reduced/materialized snapshot.
pub fn audit_snapshot(snapshot: &Snapshot) -> SchemaAudit {
    let mut audit = SchemaAudit::default();

    if !is_safe_path_segment(&snapshot.mission_slug) && !snapshot.mission_slug.is_empty() {
        audit.push(
            "snapshot.mission_slug",
            format!("unsafe mission_slug: {:?}", snapshot.mission_slug),
        );
    }

    if snapshot.event_count == 0 && snapshot.last_event_id.is_some() {
        audit.push(
            "snapshot.last_event_id",
            "empty event stream must not carry a last_event_id",
        );
    }
    if snapshot.event_count > 0 && snapshot.last_event_id.is_none() {
        audit.push(
            "snapshot.last_event_id",
            "non-empty event stream must carry a last_event_id",
        );
    }

    let expected_lanes: BTreeSet<&str> = DISPLAY_LANES.iter().map(|lane| lane.as_str()).collect();
    let actual_lanes: BTreeSet<&str> = snapshot.summary.keys().map(String::as_str).collect();
    if actual_lanes != expected_lanes {
        audit.push(
            "summary.lanes",
            format!(
                "summary must contain exactly the nine display lanes; got {:?}",
                actual_lanes
            ),
        );
    }

    let mut recomputed: BTreeMap<&str, u64> = DISPLAY_LANES
        .iter()
        .map(|lane| (lane.as_str(), 0))
        .collect();
    for (wp_id, state) in &snapshot.work_packages {
        let Some(lane_text) = state.lane_string() else {
            audit.push("work_package.lane", format!("{wp_id}: missing lane string"));
            continue;
        };
        let lane = Lane::parse_canonical(lane_text);
        match lane {
            Ok(lane) if lane == Lane::Genesis || lane == Lane::Uninitialized => {
                // ``uninitialized`` is the legitimate runtime-only sentinel;
                // ``genesis`` is a seed source, never a materialized current lane.
                if lane == Lane::Genesis {
                    audit.push(
                        "work_package.lane",
                        format!("{wp_id}: genesis is not a current lane"),
                    );
                }
            }
            Ok(lane) => {
                *recomputed.entry(lane.as_str()).or_default() += 1;
            }
            Err(error) => audit.push("work_package.lane", format!("{wp_id}: {error}")),
        }

        if state.actor().is_none() {
            audit.push(
                "work_package.actor",
                format!("{wp_id}: actor must be a string"),
            );
        }
        if state.force_count().is_none() {
            audit.push(
                "work_package.force_count",
                format!("{wp_id}: force_count must be a non-negative integer"),
            );
        }
        if state
            .get("last_event_id")
            .is_some_and(|value| !value.is_null())
            && state.last_event_id().is_none()
        {
            audit.push(
                "work_package.last_event_id",
                format!("{wp_id}: last_event_id must be a string or null"),
            );
        }
        if state
            .get("last_transition_at")
            .is_some_and(|value| !value.is_null())
            && state.last_transition_at().is_none()
        {
            audit.push(
                "work_package.last_transition_at",
                format!("{wp_id}: last_transition_at must be a string or null"),
            );
        }
        if let Some(review) = state.review_result()
            && !review.is_null()
            && !is_complete_review_result(review)
        {
            audit.push(
                "work_package.review_result",
                format!("{wp_id}: review_result must carry reviewer/verdict/reference strings"),
            );
        }

        let has_cancellation =
            state.get("cancellation_reason").is_some() || state.get("reason_source").is_some();
        if has_cancellation {
            if state.lane() != Some(Lane::Canceled) {
                audit.push(
                    "work_package.cancellation",
                    format!("{wp_id}: cancellation provenance requires the canceled lane"),
                );
            }
            if state
                .reason_source()
                .is_some_and(|source| !matches!(source, "operator" | "synthetic"))
            {
                audit.push(
                    "work_package.cancellation",
                    format!("{wp_id}: reason_source must be operator or synthetic"),
                );
            }
        }

        if state.get(IMPLEMENTER_OF_RECORD_SLOT).is_some() {
            let generation = snapshot
                .schema_version
                .unwrap_or(LEGACY_SNAPSHOT_SCHEMA_VERSION);
            if generation < IMPLEMENTER_OF_RECORD_SCHEMA_VERSION {
                audit.push(
                    "schema.implementer_of_record",
                    format!(
                        "{wp_id}: implementer_of_record is a schema-{IMPLEMENTER_OF_RECORD_SCHEMA_VERSION} field"
                    ),
                );
            }
            if state.implementer_of_record().is_none() {
                audit.push(
                    "work_package.implementer_of_record",
                    format!("{wp_id}: implementer_of_record must be a string"),
                );
            }
        }
    }

    for lane in DISPLAY_LANES {
        let actual = snapshot.summary.get(lane.as_str()).copied().unwrap_or(0);
        let expected = recomputed.get(lane.as_str()).copied().unwrap_or(0);
        if actual != expected {
            audit.push(
                "summary.counts",
                format!("{}: summary={actual}, recomputed={expected}", lane.as_str()),
            );
        }
    }

    match snapshot.schema_version {
        Some(version) if version <= LEGACY_SNAPSHOT_SCHEMA_VERSION => audit.push(
            "schema.legacy_marker",
            "the legacy generation must have no schema_version key",
        ),
        Some(version) if version < 0 => audit.push(
            "schema.schema_version",
            format!("schema_version must be positive, got {version}"),
        ),
        _ => {}
    }

    audit
}

fn is_complete_review_result(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    ["reviewer", "verdict", "reference"].iter().all(|field| {
        object
            .get(*field)
            .and_then(Value::as_str)
            .is_some_and(|text| !text.trim().is_empty())
    })
}

/// Audit raw diary rows against the canonical event schema.
///
/// Every row first passes the reducer's decoder, so invalid annotations and
/// unknown kinds cannot be reported as clean. Valid annotations and non-lane
/// rows are exempt from the additional lane-transition schema rules.
pub fn audit_event_rows(rows: &[Value]) -> SchemaAudit {
    let mut audit = SchemaAudit::default();
    for (index, row) in rows.iter().enumerate() {
        let line = index + 1;
        let Some(object) = row.as_object() else {
            audit.push("event.row", format!("line {line}: expected a JSON object"));
            continue;
        };
        if let Err(error) = crate::replay::parse_diary(std::slice::from_ref(row)) {
            audit.push("event.decode", format!("line {line}: {error}"));
        }
        if object.get("kind").is_some_and(|value| !value.is_null()) {
            continue;
        }
        if is_non_lane(object) {
            continue;
        }

        let event_id = object.get("event_id");
        for field in [
            "event_id",
            "wp_id",
            "from_lane",
            "to_lane",
            "at",
            "actor",
            "force",
            "execution_mode",
        ] {
            if !object.contains_key(field) {
                audit.push(
                    "event.required_field",
                    format!("line {line}: missing required field {field:?}"),
                );
            }
        }
        if !object.contains_key("mission_slug") && !object.contains_key("feature_slug") {
            audit.push(
                "event.required_field",
                format!("line {line}: missing mission_slug (or legacy feature_slug)"),
            );
        }

        match event_id {
            Some(Value::String(event_id)) if is_valid_event_id(event_id) => {}
            Some(other) => audit.push(
                "event.event_id",
                format!("line {line}: invalid event id: {other}"),
            ),
            None => {}
        }

        for field in ["from_lane", "to_lane", "at", "execution_mode"] {
            if let Some(value) = object.get(field)
                && !value.is_null()
                && !value.is_string()
            {
                audit.push(
                    "event.field_type",
                    format!("line {line}: {field} must be a string"),
                );
            }
        }
        let from = object.get("from_lane").and_then(Value::as_str);
        let to = object.get("to_lane").and_then(Value::as_str);
        if let Some(from) = from
            && Lane::parse_canonical(from).is_err()
            && from != Lane::Genesis.as_str()
        {
            audit.push(
                "event.from_lane",
                format!("line {line}: from_lane is not canonical: {from}"),
            );
        }
        if let Some(to) = to
            && Lane::parse_canonical(to).is_err()
        {
            audit.push(
                "event.to_lane",
                format!("line {line}: to_lane is not canonical: {to}"),
            );
        }
        if to == Some(Lane::Genesis.as_str()) {
            audit.push(
                "event.to_lane",
                format!("line {line}: genesis is never a to_lane"),
            );
        }

        if let Some(at) = object.get("at").and_then(Value::as_str)
            && !is_iso8601(at)
        {
            audit.push(
                "event.at",
                format!("line {line}: invalid ISO 8601 timestamp: {at}"),
            );
        }

        if let Some(force) = object.get("force")
            && !force.is_boolean()
        {
            audit.push(
                "event.force",
                format!("line {line}: force must be a boolean"),
            );
        }
        if object.get("force").and_then(Value::as_bool) == Some(true)
            && !object.get("reason").is_some_and(crate::replay::is_truthy)
        {
            audit.push(
                "event.force_reason",
                format!("line {line}: force=true without a reason"),
            );
        }

        if let Some(mode) = object.get("execution_mode").and_then(Value::as_str)
            && !matches!(mode, "worktree" | "direct_repo")
        {
            audit.push(
                "event.execution_mode",
                format!("line {line}: unexpected execution_mode {mode:?}"),
            );
        }

        if from == Some(Lane::ForReview.as_str()) && to == Some(Lane::InProgress.as_str()) {
            let usable = object
                .get("review_ref")
                .and_then(Value::as_str)
                .is_some_and(|reference| !reference.trim().is_empty());
            if !usable {
                audit.push(
                    "event.review_ref",
                    format!("line {line}: for_review -> in_progress requires review_ref"),
                );
            }
        }
    }
    audit
}

fn is_non_lane(object: &serde_json::Map<String, Value>) -> bool {
    if let Some(name) = object.get("event_name").and_then(Value::as_str)
        && name.starts_with("retrospective.")
    {
        return true;
    }
    if let Some(event_type) = object.get("type").and_then(Value::as_str)
        && matches!(
            event_type,
            "RetrospectiveCaptured" | "RetrospectiveCaptureFailed" | "RetrospectiveSkipped"
        )
    {
        return true;
    }
    object.contains_key("event_type")
}

fn is_valid_event_id(value: &str) -> bool {
    crate::replay::is_ulid(value) || is_uuid(value)
}

fn is_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    for (index, byte) in bytes.iter().enumerate() {
        if matches!(index, 8 | 13 | 18 | 23) {
            if *byte != b'-' {
                return false;
            }
        } else if !byte.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

fn is_iso8601(value: &str) -> bool {
    // The common native parser validates calendar dates too. Its RFC3339
    // parser accepts lowercase z and leap seconds, unlike Python datetime;
    // exclude those before using it. Python also accepts hour/minute precision.
    if value.ends_with('z') || value.get(17..19) == Some("60") {
        return false;
    }
    let expanded = match value.len() {
        13 => format!("{value}:00:00"),
        16 => format!("{value}:00"),
        _ => value.to_owned(),
    };
    if sk_kernel::clock::parse_iso(&expanded).is_err() {
        return false;
    }
    let value = expanded.as_str();
    let bytes = value.as_bytes();
    if bytes.len() < 10 {
        return false;
    }
    if !(bytes[0..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(u8::is_ascii_digit))
    {
        return false;
    }
    if bytes.len() == 10 {
        return true;
    }
    if bytes[10] != b'T' && bytes[10] != b' ' {
        return false;
    }
    if bytes.len() < 19 {
        return false;
    }
    if !(bytes[11..13].iter().all(u8::is_ascii_digit)
        && bytes[13] == b':'
        && bytes[14..16].iter().all(u8::is_ascii_digit)
        && bytes[16] == b':'
        && bytes[17..19].iter().all(u8::is_ascii_digit))
    {
        return false;
    }
    let hour = value[11..13].parse::<u8>().unwrap_or(99);
    let minute = value[14..16].parse::<u8>().unwrap_or(99);
    let second = value[17..19].parse::<u8>().unwrap_or(99);
    if hour > 23 || minute > 59 || second > 60 {
        return false;
    }

    let mut index = 19;
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        if index == start {
            return false;
        }
    }
    if index == bytes.len() {
        return true;
    }
    let rest = &value[index..];
    if rest == "Z" || rest == "z" {
        return true;
    }
    let sign = rest.as_bytes().first().copied();
    if !matches!(sign, Some(b'+') | Some(b'-')) {
        return false;
    }
    let offset = &rest[1..];
    let (hours, minutes) = match offset.len() {
        2 => (offset, None),
        4 => (&offset[..2], Some(&offset[2..])),
        5 if offset.as_bytes()[2] == b':' => (&offset[..2], Some(&offset[3..])),
        _ => return false,
    };
    if !hours.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    let hours = hours.parse::<u8>().unwrap_or(99);
    if hours > 23 {
        return false;
    }
    match minutes {
        None => true,
        Some(minutes) => {
            minutes.len() == 2
                && minutes.bytes().all(|byte| byte.is_ascii_digit())
                && minutes.parse::<u8>().is_ok_and(|minutes| minutes <= 59)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_parser_accepts_the_shapes_the_baseline_accepts() {
        assert!(is_iso8601("2026-01-01T00:00:01Z"));
        assert!(is_iso8601("2026-01-01T00:00:01.123456+00:00"));
        assert!(is_iso8601("2026-01-01T00:00:01"));
        assert!(is_iso8601("2026-01-01 00:00:01"));
        assert!(is_iso8601("2026-01-01T00:00"));
        assert!(!is_iso8601("2026-13-45T00:00:01Z"));
        assert!(!is_iso8601("2026-01-01T00:00:01z"));
        assert!(!is_iso8601("2026-01-01T00:00:60Z"));
        assert!(!is_iso8601("not-a-timestamp"));
    }
}
