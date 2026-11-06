//! `doctor invocation-pairing` — read-only orphan report over the local
//! profile-invocation lifecycle store.
//!
//! # Source
//!
//! `specify_cli.invocation.lifecycle.doctor_orphan_report` (#843, #7003): every
//! `started` record should eventually have exactly one paired `completed` or
//! `failed` record sharing the same
//! `(mission_id, canonical_action_id)`. An unpaired surplus of `started`
//! records is an *orphan* — the observable trace of a mid-cycle agent crash.
//!
//! # Store layout (pinned Source, #7003)
//!
//! * per-record files: `kitty-ops/lifecycle/<ulid>.json`, one record each,
//!   read after the legacy log and sorted by (`at` as UTC, ULID stem);
//! * the legacy log `kitty-ops/lifecycle.jsonl`, read first in insertion order,
//!   never written again and never migrated.
//!
//! Malformed lines/files are skipped, not fatal: Source treats a corrupt record
//! as a doctor signal rather than an aborted command, and this port keeps that
//! tolerance (`_parse_record_line`/`from_dict` failure ⇒ skip).
//!
//! # Ownership
//!
//! The lifecycle store's canonical owner (`specify_cli.invocation.lifecycle`)
//! is not ported yet, so this is the first native implementation of its
//! read-only half. It is deliberately doctor-scoped and must be promoted to
//! `sk_invocation::lifecycle` (one model, one reader) when the writer lands —
//! see the CF14 report's additive-API request. Nothing writes: no append, no
//! mkdir, no lock, no timestamps.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sk_kernel::PythonString;
use sk_kernel::python_json::{PythonJsonValue, parse};
use sk_kernel::python_text::{is_whitespace, splitlines};

use crate::doctor::DoctorError;

/// The legacy shared lifecycle log, relative to the repository root.
pub const LIFECYCLE_LOG_RELATIVE_PATH: [&str; 2] = ["kitty-ops", "lifecycle.jsonl"];

/// The directory of one-record-per-file lifecycle records.
pub const LIFECYCLE_RECORDS_RELATIVE_DIR: [&str; 2] = ["kitty-ops", "lifecycle"];

/// The record-file suffix.
pub const LIFECYCLE_RECORD_SUFFIX: &str = ".json";

/// The absolute path of the legacy lifecycle log under `root`.
pub fn lifecycle_log_path(root: &Path) -> PathBuf {
    root.join(LIFECYCLE_LOG_RELATIVE_PATH[0])
        .join(LIFECYCLE_LOG_RELATIVE_PATH[1])
}

/// The absolute path of the per-record lifecycle directory under `root`.
pub fn lifecycle_records_dir(root: &Path) -> PathBuf {
    root.join(LIFECYCLE_RECORDS_RELATIVE_DIR[0])
        .join(LIFECYCLE_RECORDS_RELATIVE_DIR[1])
}

/// One lifecycle record's phase (Source `ProfileInvocationPhase`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvocationPhase {
    /// An action started.
    Started,
    /// The action completed.
    Completed,
    /// The action failed.
    Failed,
}

impl InvocationPhase {
    /// The stored member value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    /// Parse a stored `phase` value; anything else is not a record.
    pub fn from_text(text: &PythonString) -> Option<Self> {
        [
            (Self::Started, "started"),
            (Self::Completed, "completed"),
            (Self::Failed, "failed"),
        ]
        .into_iter()
        .find(|(_, member)| *text == python_string(member))
        .map(|(phase, _)| phase)
    }
}

/// One paired profile-invocation lifecycle record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LifecycleRecord {
    /// The `mission_step::action` id shared by a start and its partner.
    pub canonical_action_id: PythonString,
    /// Which half of the pair this record is.
    pub phase: InvocationPhase,
    /// When the record was written, normalised to UTC on read.
    pub at: DateTime<Utc>,
    /// The agent that issued the action.
    pub agent: PythonString,
    /// The Mission the action belongs to.
    pub mission_id: PythonString,
    /// The work package, when the action names one.
    pub wp_id: Option<PythonString>,
}

impl LifecycleRecord {
    /// Source `_ensure_iso_utc(dt).isoformat()`: UTC spelled `+00:00`,
    /// microseconds only when non-zero.
    pub fn isoformat(&self) -> String {
        let mut text = self.at.format("%Y-%m-%dT%H:%M:%S").to_string();
        let micros = self.at.timestamp_subsec_micros();
        if micros != 0 {
            text.push_str(&format!(".{micros:06}"));
        }
        text.push_str("+00:00");
        text
    }
}

/// One orphan `started` record in the report payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrphanEntry {
    /// The shared action id.
    pub canonical_action_id: String,
    /// The issuing agent.
    pub agent: String,
    /// The Mission id.
    pub mission_id: String,
    /// The work package, when recorded.
    pub wp_id: Option<String>,
    /// The start timestamp, Python `isoformat()` spelling.
    pub started_at: String,
}

/// The Source-shaped orphan report.
#[derive(Clone, Debug, PartialEq)]
pub struct PairingAudit {
    /// One entry per unpaired surplus `started` record.
    pub orphans: Vec<OrphanEntry>,
    /// The number of distinct `(mission_id, canonical_action_id)` groups.
    pub total_groups: usize,
    /// `paired / started`, or `1.0` when no action started (Source's rule).
    pub pairing_rate: f64,
}

impl PairingAudit {
    /// The number of orphan records (Source's `orphan_count`).
    pub fn orphan_count(&self) -> usize {
        self.orphans.len()
    }

    /// Whether no start is unpaired.
    pub fn healthy(&self) -> bool {
        self.orphans.is_empty()
    }

    /// Source's `json.dumps(report, indent=2, sort_keys=True)` payload: the
    /// keys are inserted in sorted order at every level.
    pub fn to_json(&self) -> Value {
        let mut report = serde_json::Map::new();
        report.insert("orphan_count".to_owned(), json!(self.orphan_count()));
        report.insert(
            "orphans".to_owned(),
            Value::Array(
                self.orphans
                    .iter()
                    .map(|entry| {
                        let mut item = serde_json::Map::new();
                        item.insert(
                            "canonical_action_id".to_owned(),
                            Value::String(entry.canonical_action_id.clone()),
                        );
                        item.insert("agent".to_owned(), Value::String(entry.agent.clone()));
                        item.insert(
                            "mission_id".to_owned(),
                            Value::String(entry.mission_id.clone()),
                        );
                        item.insert(
                            "started_at".to_owned(),
                            Value::String(entry.started_at.clone()),
                        );
                        item.insert(
                            "wp_id".to_owned(),
                            entry
                                .wp_id
                                .as_ref()
                                .map_or(Value::Null, |wp| Value::String(wp.clone())),
                        );
                        Value::Object(item)
                    })
                    .collect(),
            ),
        );
        report.insert("pairing_rate".to_owned(), json!(self.pairing_rate));
        report.insert("total_groups".to_owned(), json!(self.total_groups));
        Value::Object(report)
    }

    /// The pairing rate as Source's `f"{rate:.0%}"` (round half to even).
    pub fn pairing_percent(&self) -> i64 {
        (self.pairing_rate * 100.0).round_ties_even() as i64
    }
}

/// Read the whole lifecycle store: legacy log first, then per-record files.
pub fn read_lifecycle_records(root: &Path) -> Result<Vec<LifecycleRecord>, DoctorError> {
    let mut records = read_legacy_records(&lifecycle_log_path(root))?;
    records.extend(read_record_files(&lifecycle_records_dir(root))?);
    Ok(records)
}

/// Diagnose unpaired `started` records under `root`.
pub fn audit_invocation_pairing(root: &Path) -> Result<PairingAudit, DoctorError> {
    let records = read_lifecycle_records(root)?;
    Ok(pairing_report(&records))
}

/// Build the report from already-read records (Source's pure helpers:
/// `group_by_action`, `find_orphans`, `compute_pairing_rate`).
pub fn pairing_report(records: &[LifecycleRecord]) -> PairingAudit {
    let groups = group_by_action(records);
    let mut orphans = Vec::new();
    for group in &groups {
        let started = group
            .iter()
            .filter(|record| record.phase == InvocationPhase::Started)
            .collect::<Vec<_>>();
        let completions = group
            .iter()
            .filter(|record| record.phase != InvocationPhase::Started)
            .count();
        let deficit = started.len().saturating_sub(completions);
        for record in started.iter().skip(started.len() - deficit) {
            orphans.push(OrphanEntry {
                canonical_action_id: render(&record.canonical_action_id),
                agent: render(&record.agent),
                mission_id: render(&record.mission_id),
                wp_id: record.wp_id.as_ref().map(render),
                started_at: record.isoformat(),
            });
        }
    }

    let started_total = records
        .iter()
        .filter(|record| record.phase == InvocationPhase::Started)
        .count();
    let completion_total = records.len() - started_total;
    let pairing_rate = if started_total == 0 {
        1.0
    } else {
        started_total.min(completion_total) as f64 / started_total as f64
    };

    PairingAudit {
        orphans,
        total_groups: groups.len(),
        pairing_rate,
    }
}

/// Group records by `(mission_id, canonical_action_id)` in first-seen order.
fn group_by_action(records: &[LifecycleRecord]) -> Vec<Vec<&LifecycleRecord>> {
    let mut order: Vec<(PythonString, PythonString)> = Vec::new();
    let mut buckets: BTreeMap<(PythonString, PythonString), Vec<&LifecycleRecord>> =
        BTreeMap::new();
    for record in records {
        let key = (
            record.mission_id.clone(),
            record.canonical_action_id.clone(),
        );
        if !buckets.contains_key(&key) {
            order.push(key.clone());
        }
        buckets.entry(key).or_default().push(record);
    }
    order
        .into_iter()
        .map(|key| buckets.remove(&key).unwrap_or_default())
        .collect()
}

/// Read the legacy JSONL log; `[]` when it is absent or unreadable as text.
///
/// Source `_read_legacy_records`: `path.is_file()` then `read_text`, and an
/// `OSError`/`UnicodeDecodeError` degrades to no records (a corrupt legacy log
/// is a doctor signal, not a crash).
fn read_legacy_records(path: &Path) -> Result<Vec<LifecycleRecord>, DoctorError> {
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let raw = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => return Ok(Vec::new()),
    };
    let Ok(text) = std::str::from_utf8(&raw) else {
        return Ok(Vec::new());
    };
    Ok(splitlines(text)
        .into_iter()
        .filter_map(parse_record_line)
        .collect())
}

/// Read the per-record directory, sorted by (`at` as UTC, ULID stem).
fn read_record_files(directory: &Path) -> Result<Vec<LifecycleRecord>, DoctorError> {
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(directory).map_err(|source| DoctorError::Io {
        context: "list the lifecycle record directory",
        path: directory.to_path_buf(),
        source,
    })?;
    let mut keyed: Vec<(DateTime<Utc>, Vec<u8>, LifecycleRecord)> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| DoctorError::Io {
            context: "list the lifecycle record directory",
            path: directory.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let name = entry.file_name();
        if !name
            .as_encoded_bytes()
            .ends_with(LIFECYCLE_RECORD_SUFFIX.as_bytes())
        {
            continue;
        }
        if !path.is_file() {
            continue;
        }
        let Ok(raw) = std::fs::read(&path) else {
            continue;
        };
        let Ok(text) = std::str::from_utf8(&raw) else {
            continue;
        };
        let Some(record) = parse_record_line(text) else {
            continue;
        };
        let stem = name
            .as_encoded_bytes()
            .strip_suffix(LIFECYCLE_RECORD_SUFFIX.as_bytes())
            .unwrap_or_default()
            .to_vec();
        keyed.push((record.at, stem, record));
    }
    keyed.sort_by(|left, right| (left.0, &left.1).cmp(&(right.0, &right.1)));
    Ok(keyed.into_iter().map(|(_, _, record)| record).collect())
}

/// Parse one JSONL line as a lifecycle record; `None` for a blank or
/// malformed line (Source's skip-don't-crash rule).
fn parse_record_line(line: &str) -> Option<LifecycleRecord> {
    let line = trim_python_whitespace(line);
    if line.is_empty() {
        return None;
    }
    let value = parse(line).ok()?;
    parse_record(&value)
}

/// Parse a decoded JSON value as a lifecycle record.
///
/// Source `ProfileInvocationRecord.from_dict`: `at` must be a parseable
/// timestamp, `phase` must be a declared member, and
/// `canonical_action_id`/`agent`/`mission_id` must be present (each is
/// `str()`-coerced). `wp_id`/`reason`/`metadata` are optional.
fn parse_record(value: &PythonJsonValue) -> Option<LifecycleRecord> {
    let PythonJsonValue::ExactObject(object) = value else {
        return None;
    };
    let at = match object.get("at") {
        Some(PythonJsonValue::String(text)) if !text.is_empty() => {
            let text = text.to_scalar_string().ok()?;
            sk_kernel::clock::parse_iso(&text).ok()?
        }
        _ => return None,
    };
    let phase = match object.get("phase") {
        Some(PythonJsonValue::String(text)) => InvocationPhase::from_text(text)?,
        _ => return None,
    };
    let field = |name: &str| -> Option<PythonString> {
        let value = object.get(name)?;
        match value {
            PythonJsonValue::String(text) => Some(text.clone()),
            other => scoped_str(other),
        }
    };
    let canonical_action_id = field("canonical_action_id")?;
    let agent = field("agent")?;
    let mission_id = field("mission_id")?;
    let wp_id = match object.get("wp_id") {
        None | Some(PythonJsonValue::Null) => None,
        Some(_) => field("wp_id"),
    };
    Some(LifecycleRecord {
        canonical_action_id,
        phase,
        at,
        agent,
        mission_id,
        wp_id,
    })
}

/// Python `str()` for a decoded JSON value, as a code-point string.
fn scoped_str(value: &PythonJsonValue) -> Option<PythonString> {
    let text = match value {
        PythonJsonValue::String(text) => return Some(text.clone()),
        PythonJsonValue::Null => "None".to_owned(),
        PythonJsonValue::Bool(true) => "True".to_owned(),
        PythonJsonValue::Bool(false) => "False".to_owned(),
        other => other.render(None),
    };
    Some(PythonString::from_scalars(&text))
}

/// Render a code-point id as scalar text for the report (a lone surrogate has
/// no scalar form; it is reported through the Python `repr` instead of being
/// silently replaced).
fn render(value: &PythonString) -> String {
    value.to_scalar_string().unwrap_or_else(|_| value.repr())
}

/// An ASCII literal as an exact code-point Python string.
fn python_string(text: &str) -> PythonString {
    PythonString::from_scalars(text)
}

/// Python `str.strip()` — strip code points where `str.isspace()` is true.
fn trim_python_whitespace(text: &str) -> &str {
    let start = text
        .char_indices()
        .find(|(_, character)| !is_whitespace(u32::from(*character)))
        .map(|(index, _)| index)
        .unwrap_or(text.len());
    let end = text
        .char_indices()
        .rev()
        .find(|(_, character)| !is_whitespace(u32::from(*character)))
        .map(|(index, character)| index + character.len_utf8())
        .unwrap_or(start);
    &text[start..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("fixture parent");
        }
        std::fs::write(path, contents).expect("fixture write");
    }

    fn record(action: &str, phase: &str, at: &str, wp: Option<&str>) -> String {
        match wp {
            Some(wp) => format!(
                r#"{{"canonical_action_id":"{action}","phase":"{phase}","at":"{at}","agent":"claude","mission_id":"m1","wp_id":"{wp}"}}"#
            ),
            None => format!(
                r#"{{"canonical_action_id":"{action}","phase":"{phase}","at":"{at}","agent":"claude","mission_id":"m1"}}"#
            ),
        }
    }

    #[test]
    fn an_absent_store_is_vacuously_paired() {
        let root = tempfile::tempdir().expect("tempdir");
        let report = audit_invocation_pairing(root.path()).expect("audit");
        assert!(report.healthy());
        assert_eq!(report.pairing_rate, 1.0);
        assert_eq!(report.total_groups, 0);
        assert_eq!(report.pairing_percent(), 100);
    }

    #[test]
    fn an_unpaired_start_from_the_legacy_log_is_an_orphan() {
        let root = tempfile::tempdir().expect("tempdir");
        write(
            &lifecycle_log_path(root.path()),
            &format!(
                "{}\n{}\n",
                record(
                    "plan::specify",
                    "started",
                    "2026-01-01T00:00:00+00:00",
                    Some("WP01")
                ),
                record(
                    "plan::specify",
                    "started",
                    "2026-01-02T00:00:00+00:00",
                    None
                )
                .replace("\"m1\"", "\"m2\"")
            ),
        );
        let report = audit_invocation_pairing(root.path()).expect("audit");
        assert_eq!(report.orphan_count(), 2);
        assert_eq!(report.total_groups, 2);
        assert_eq!(report.pairing_rate, 0.0);
        let payload = report.to_json();
        assert_eq!(payload["orphan_count"], json!(2));
        assert_eq!(payload["orphans"][0]["wp_id"], json!("WP01"));
        assert_eq!(
            payload["orphans"][0]["started_at"],
            json!("2026-01-01T00:00:00+00:00")
        );
        // Sorted keys, like `json.dumps(sort_keys=True)`.
        let keys: Vec<_> = payload
            .as_object()
            .expect("object")
            .keys()
            .cloned()
            .collect();
        assert_eq!(
            keys,
            vec!["orphan_count", "orphans", "pairing_rate", "total_groups"]
        );
    }

    #[test]
    fn a_paired_completion_removes_the_orphan() {
        let root = tempfile::tempdir().expect("tempdir");
        write(
            &lifecycle_log_path(root.path()),
            &format!(
                "{}\n{}\n",
                record(
                    "plan::specify",
                    "started",
                    "2026-01-01T00:00:00+00:00",
                    None
                ),
                record("plan::specify", "failed", "2026-01-01T01:00:00+00:00", None)
            ),
        );
        let report = audit_invocation_pairing(root.path()).expect("audit");
        assert!(report.healthy());
        assert_eq!(report.pairing_rate, 1.0);
    }

    #[test]
    fn the_same_action_in_two_missions_cannot_cross_pair() {
        let root = tempfile::tempdir().expect("tempdir");
        let started = record(
            "plan::specify",
            "started",
            "2026-01-01T00:00:00+00:00",
            None,
        );
        let completed = record(
            "plan::specify",
            "completed",
            "2026-01-01T01:00:00+00:00",
            None,
        )
        .replace("\"m1\"", "\"m2\"");
        write(
            &lifecycle_log_path(root.path()),
            &format!("{started}\n{completed}\n"),
        );
        let report = audit_invocation_pairing(root.path()).expect("audit");
        assert_eq!(report.orphan_count(), 1);
        assert_eq!(report.total_groups, 2);
    }

    #[test]
    fn per_record_files_are_read_after_the_legacy_log_and_ordered_by_at() {
        let root = tempfile::tempdir().expect("tempdir");
        write(
            &lifecycle_log_path(root.path()),
            &format!(
                "{}\n",
                record(
                    "plan::specify",
                    "started",
                    "2026-01-05T00:00:00+00:00",
                    None
                )
            ),
        );
        let dir = lifecycle_records_dir(root.path());
        write(
            &dir.join("01AAA.json"),
            &record(
                "plan::specify",
                "completed",
                "2026-01-02T00:00:00+00:00",
                None,
            ),
        );
        write(
            &dir.join("01BBB.json"),
            &record(
                "plan::specify",
                "started",
                "2026-01-09T00:00:00+00:00",
                None,
            ),
        );
        let records = read_lifecycle_records(root.path()).expect("records");
        let phases: Vec<_> = records.iter().map(|record| record.phase).collect();
        // Legacy first (started), then the two files by `at` (completed, started).
        assert_eq!(
            phases,
            vec![
                InvocationPhase::Started,
                InvocationPhase::Completed,
                InvocationPhase::Started
            ]
        );
    }

    #[test]
    fn malformed_lines_and_files_are_skipped_not_fatal() {
        let root = tempfile::tempdir().expect("tempdir");
        write(
            &lifecycle_log_path(root.path()),
            "{not json\n{\"phase\":\"started\"}\n{\"at\":\"2026-01-01T00:00:00Z\",\"phase\":\"bogus\",\"canonical_action_id\":\"a\",\"agent\":\"x\",\"mission_id\":\"m\"}\n\n",
        );
        let dir = lifecycle_records_dir(root.path());
        write(&dir.join("01BAD.json"), "not json at all");
        let report = audit_invocation_pairing(root.path()).expect("audit");
        assert!(report.healthy());
        assert_eq!(report.total_groups, 0);
    }

    #[test]
    fn a_naive_timestamp_is_read_as_utc_and_keeps_microseconds() {
        let root = tempfile::tempdir().expect("tempdir");
        write(
            &lifecycle_log_path(root.path()),
            &format!(
                "{}\n",
                record("a::b", "started", "2026-01-01T00:00:00.500000", None)
            ),
        );
        let report = audit_invocation_pairing(root.path()).expect("audit");
        assert_eq!(
            report.orphans[0].started_at,
            "2026-01-01T00:00:00.500000+00:00"
        );
    }
}
