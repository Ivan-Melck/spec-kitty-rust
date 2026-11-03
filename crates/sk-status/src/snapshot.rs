//! Typed snapshot materialization for `status.events.jsonl`.
//!
//! The authoritative input is the append-only diary. This module produces the
//! exact derived `status.json` shape the baseline CLI writes, including:
//!
//! - the external diary fold ([`crate::replay::reduce_diary`]);
//! - cancellation provenance and the `implementer_of_record` read-root slot;
//! - the mission identity fields resolved from `meta.json`;
//! - retrospective rows attached after reduction;
//! - the materialized-snapshot schema generation, keyed off the existing
//!   on-disk `status.json` so an archive replays at *its own* schema.
//!
//! It reads and renders; it never appends, renames or writes an event log or a
//! snapshot. A future writer must use the shared kernel lock and the baseline
//! append protocol rather than introducing a second writer here.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io;
use std::path::Path;

use serde_json::{Map, Number, Value};

use crate::lane::Lane;
use crate::replay::{DiaryError, StatusEvent, is_truthy, parse_diary, python_str, reduce_diary};

/// File name of the authoritative append-only diary.
pub const EVENTS_FILENAME: &str = "status.events.jsonl";
/// File name of the derived, human-readable snapshot.
pub const SNAPSHOT_FILENAME: &str = "status.json";
/// File name of mission metadata.
pub const META_FILENAME: &str = "meta.json";

/// Default file budget for replay input (64 MiB).
pub const MAX_DIARY_BYTES: u64 = 64 * 1024 * 1024;
/// Default budget for an existing derived snapshot (32 MiB).
pub const MAX_SNAPSHOT_BYTES: u64 = 32 * 1024 * 1024;
/// Default mission metadata budget (1 MiB).
pub const MAX_META_BYTES: u64 = 1024 * 1024;

/// The implicit schema of every snapshot with no `schema_version` key.
pub const LEGACY_SNAPSHOT_SCHEMA_VERSION: i64 = 1;
/// The current materialized-snapshot schema generation.
pub const CURRENT_SNAPSHOT_SCHEMA_VERSION: i64 = 2;
/// The generation at which `implementer_of_record` was introduced.
pub const IMPLEMENTER_OF_RECORD_SCHEMA_VERSION: i64 = 2;
/// The durable implementer-claim provenance slot.
pub const IMPLEMENTER_OF_RECORD_SLOT: &str = "implementer_of_record";

const SYNTHETIC_REASON_PREFIXES: [&str; 2] = ["Force move to ", "move-task: "];

/// A typed snapshot read/materialization failure.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    #[error(transparent)]
    Diary(#[from] DiaryError),

    #[error("Could not read the status surface: {0}")]
    Io(#[from] io::Error),

    #[error("Could not read replay input {path}: {source}")]
    InputRead {
        path: std::path::PathBuf,
        #[source]
        source: sk_kernel::KernelError,
    },

    #[error("Invalid JSON on line {line}: {message}")]
    InvalidJson { line: u64, message: String },

    #[error("Invalid event structure on line {line}: expected JSON object")]
    InvalidStructure { line: u64 },

    #[error("Could not decode meta.json: {detail}")]
    Meta { detail: String },

    #[error("Could not decode an existing status.json: {detail}")]
    ExistingSnapshot { detail: String },

    #[error("Could not serialize the snapshot: {0}")]
    Serialize(#[from] serde_json::Error),
}

/// One work package's reduced runtime state.
///
/// The value is a JSON object on purpose: the snapshot is a read-root that
/// carries additive runtime slots, and a typed enum would silently drop a
/// slot a newer writer emitted. Typed accessors cover the slots gates consume.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WorkPackageState(BTreeMap<String, Value>);

impl WorkPackageState {
    /// An empty state object.
    pub fn new() -> Self {
        Self::default()
    }

    /// Borrow the raw slot map.
    pub fn as_map(&self) -> &BTreeMap<String, Value> {
        &self.0
    }

    /// Mutably borrow the raw slot map.
    pub fn as_map_mut(&mut self) -> &mut BTreeMap<String, Value> {
        &mut self.0
    }

    /// Read one raw slot.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.0.get(name)
    }

    /// Insert one raw slot.
    pub fn insert(&mut self, name: impl Into<String>, value: Value) -> Option<Value> {
        self.0.insert(name.into(), value)
    }

    /// Remove one raw slot.
    pub fn remove(&mut self, name: &str) -> Option<Value> {
        self.0.remove(name)
    }

    /// The current lane, when the slot is a canonical lane value.
    pub fn lane(&self) -> Option<Lane> {
        self.lane_string()
            .and_then(|lane| Lane::parse_canonical(lane).ok())
    }

    /// The current lane as its wire string, even if it is not canonical.
    pub fn lane_string(&self) -> Option<&str> {
        self.get("lane").and_then(Value::as_str)
    }

    /// The projected display actor identity.
    pub fn actor(&self) -> Option<&str> {
        self.get("actor").and_then(Value::as_str)
    }

    /// The non-negative force counter.
    pub fn force_count(&self) -> Option<u64> {
        self.get("force_count").and_then(Value::as_u64)
    }

    /// The last applied transition's event id (`None` for a runtime-only WP).
    pub fn last_event_id(&self) -> Option<&str> {
        self.get("last_event_id").and_then(Value::as_str)
    }

    /// The last applied transition's timestamp (`None` for a runtime-only WP).
    pub fn last_transition_at(&self) -> Option<&str> {
        self.get("last_transition_at").and_then(Value::as_str)
    }

    /// The durable implementer-of-record attribution.
    pub fn implementer_of_record(&self) -> Option<&str> {
        self.get(IMPLEMENTER_OF_RECORD_SLOT).and_then(Value::as_str)
    }

    /// The cancellation reason on a finally-canceled WP.
    pub fn cancellation_reason(&self) -> Option<&str> {
        self.get("cancellation_reason").and_then(Value::as_str)
    }

    /// The cancellation provenance discriminator.
    pub fn reason_source(&self) -> Option<&str> {
        self.get("reason_source").and_then(Value::as_str)
    }

    /// The event-sourced review verdict slot.
    pub fn review_result(&self) -> Option<&Value> {
        self.get("review_result")
    }

    /// The JSON object for this state.
    pub fn to_json_value(&self) -> Value {
        Value::Object(
            self.0
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        )
    }
}

/// The retrospective attachment derived from `retrospective.*` rows.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RetrospectiveSnapshot {
    pub status: String,
    pub mode: Option<Value>,
    pub record_path: Option<String>,
    pub proposals_total: u64,
    pub proposals_applied: u64,
    pub proposals_rejected: u64,
    pub proposals_pending: u64,
}

impl RetrospectiveSnapshot {
    fn to_json_value(&self) -> Value {
        json_object([
            ("status", Value::String(self.status.clone())),
            ("mode", self.mode.clone().unwrap_or(Value::Null)),
            (
                "record_path",
                self.record_path
                    .clone()
                    .map(Value::String)
                    .unwrap_or(Value::Null),
            ),
            (
                "proposals_total",
                Value::Number(Number::from(self.proposals_total)),
            ),
            (
                "proposals_applied",
                Value::Number(Number::from(self.proposals_applied)),
            ),
            (
                "proposals_rejected",
                Value::Number(Number::from(self.proposals_rejected)),
            ),
            (
                "proposals_pending",
                Value::Number(Number::from(self.proposals_pending)),
            ),
        ])
    }
}

/// The derived mission snapshot.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub mission_slug: String,
    pub materialized_at: String,
    pub event_count: usize,
    pub last_event_id: Option<String>,
    pub work_packages: BTreeMap<String, WorkPackageState>,
    pub summary: BTreeMap<String, u64>,
    pub mission_number: Option<String>,
    pub mission_type: Option<String>,
    pub retrospective: Option<RetrospectiveSnapshot>,
    pub schema_version: Option<i64>,
}

impl Snapshot {
    /// The exact `StatusSnapshot.to_dict()` shape, with sorted JSON keys.
    pub fn to_json_value(&self) -> Value {
        let mut map = Map::new();
        map.insert(
            "mission_slug".to_owned(),
            Value::String(self.mission_slug.trim().to_owned()),
        );
        map.insert(
            "mission_number".to_owned(),
            Value::String(self.mission_number_for_output()),
        );
        map.insert(
            "mission_type".to_owned(),
            Value::String(self.mission_type_for_output()),
        );
        map.insert(
            "materialized_at".to_owned(),
            Value::String(self.materialized_at.clone()),
        );
        map.insert(
            "event_count".to_owned(),
            Value::Number(Number::from(self.event_count)),
        );
        map.insert(
            "last_event_id".to_owned(),
            self.last_event_id
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        map.insert(
            "work_packages".to_owned(),
            Value::Object(
                self.work_packages
                    .iter()
                    .map(|(wp_id, state)| (wp_id.clone(), state.to_json_value()))
                    .collect(),
            ),
        );
        map.insert(
            "summary".to_owned(),
            Value::Object(
                self.summary
                    .iter()
                    .map(|(lane, count)| (lane.clone(), Value::Number(Number::from(*count))))
                    .collect(),
            ),
        );
        if let Some(retrospective) = &self.retrospective {
            map.insert("retrospective".to_owned(), retrospective.to_json_value());
        }
        if let Some(version) = self.schema_version {
            map.insert(
                "schema_version".to_owned(),
                Value::Number(Number::from(version)),
            );
        }
        Value::Object(map)
    }

    fn mission_number_for_output(&self) -> String {
        if let Some(number) = &self.mission_number {
            let trimmed = number.trim();
            if !trimmed.is_empty() {
                return trimmed.to_owned();
            }
        }
        mission_number_from_slug(&self.mission_slug)
            .map(|number| number.to_string())
            .unwrap_or_default()
    }

    fn mission_type_for_output(&self) -> String {
        self.mission_type
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("")
            .to_owned()
    }
}

/// The read inputs for one materialization.
#[derive(Debug, Clone, Default)]
pub struct ReplayInput {
    /// Raw JSON objects, in file order.
    pub rows: Vec<Value>,
    /// Parsed `meta.json`, when present.
    pub meta: Option<Value>,
    /// Existing `status.json`: `None` means absent; `Some(Null)` also represents
    /// present but malformed JSON. Presence must survive parsing failure so
    /// archive replay never silently promotes a legacy schema generation.
    pub existing_snapshot: Option<Value>,
}

impl ReplayInput {
    /// Read `status.events.jsonl`, `meta.json`, and the existing `status.json`.
    ///
    /// A missing diary is an empty stream. A missing `meta.json` is `None`. An
    /// malformed existing `status.json` becomes `Some(Null)`, preserving its
    /// presence for the baseline legacy-schema resolver. Unsafe, oversized or unreadable files
    /// fail closed instead of being silently reclassified as a legacy schema.
    pub fn read_dir(feature_dir: &Path) -> Result<Self, SnapshotError> {
        let rows = read_raw_events(&feature_dir.join(EVENTS_FILENAME))?;
        let meta = read_optional_json_strict(&feature_dir.join(META_FILENAME))?;
        let existing_snapshot = read_optional_json_lenient(&feature_dir.join(SNAPSHOT_FILENAME))?;
        Ok(Self {
            rows,
            meta,
            existing_snapshot,
        })
    }
}

/// Split Python text-file universal newlines without copying the whole input.
pub(crate) fn universal_lines(mut text: &str) -> impl Iterator<Item = &str> {
    std::iter::from_fn(move || {
        if text.is_empty() {
            return None;
        }
        let Some(end) = text.find(['\r', '\n']) else {
            return Some(std::mem::take(&mut text));
        };
        let line = &text[..end];
        let width = if text.as_bytes()[end] == b'\r' && text.as_bytes().get(end + 1) == Some(&b'\n')
        {
            2
        } else {
            1
        };
        text = &text[end + width..];
        Some(line)
    })
}

/// Read raw diary rows, accepting LF, CRLF and CR separators.
///
/// Missing files are empty streams. Blank lines are skipped; malformed JSON,
/// non-object rows and non-finite floats fail with a 1-based line number.
pub fn read_raw_events(path: &Path) -> Result<Vec<Value>, SnapshotError> {
    let Some(bytes) = read_optional_bytes(path, MAX_DIARY_BYTES)? else {
        return Ok(Vec::new());
    };
    let text = String::from_utf8(bytes).map_err(|error| SnapshotError::InputRead {
        path: path.to_path_buf(),
        source: sk_kernel::KernelError::Io {
            action: "decode UTF-8 replay input",
            path: path.to_path_buf(),
            source: io::Error::new(io::ErrorKind::InvalidData, error),
        },
    })?;
    let mut rows = Vec::new();
    for (index, line) in universal_lines(&text).enumerate() {
        let line_number = index as u64 + 1;
        let stripped = line.trim();
        if stripped.is_empty() {
            continue;
        }
        let value: Value =
            serde_json::from_str(stripped).map_err(|error| SnapshotError::InvalidJson {
                line: line_number,
                message: error.to_string(),
            })?;
        if crate::mutation_json::has_unrepresentable_float(&value) {
            return Err(SnapshotError::InvalidJson {
                line: line_number,
                message: "non-finite JSON floats are unsupported".to_owned(),
            });
        }
        if !value.is_object() {
            return Err(SnapshotError::InvalidStructure { line: line_number });
        }
        rows.push(value);
    }
    Ok(rows)
}

/// Reduce raw rows through the diary fold only (no CLI projections).
///
/// This is not a materialized CLI snapshot: schema generation, mission identity,
/// cancellation provenance, attribution and retrospective projection require
/// [`materialize`]. Do not use this result as a replacement for `status.json`.
pub fn reduce_rows(rows: &[Value]) -> Result<Snapshot, SnapshotError> {
    let stream = parse_diary(rows)?;
    Ok(reduce_diary(&stream.transitions, &stream.annotations))
}

/// Materialize the full CLI snapshot from already-read inputs.
pub fn materialize(input: &ReplayInput) -> Result<Snapshot, SnapshotError> {
    let stream = parse_diary(&input.rows)?;
    let mut snapshot = reduce_diary(&stream.transitions, &stream.annotations);

    project_cancellation_provenance(&stream.transitions, &mut snapshot);

    let target = target_schema_version(input.existing_snapshot.as_ref());
    if target >= IMPLEMENTER_OF_RECORD_SCHEMA_VERSION {
        project_implementer_attribution(&stream.transitions, &mut snapshot);
    }
    if target > LEGACY_SNAPSHOT_SCHEMA_VERSION {
        snapshot.schema_version = Some(target);
    }

    let identity = mission_identity(input.meta.as_ref())?;
    snapshot.mission_number = identity.mission_number;
    snapshot.mission_type = identity.mission_type;

    if let Some(retrospective) = reduce_retrospective(&input.rows) {
        snapshot.retrospective = Some(retrospective);
    }

    Ok(snapshot)
}

/// Read a feature directory and materialize its snapshot.
pub fn materialize_dir(feature_dir: &Path) -> Result<Snapshot, SnapshotError> {
    let input = ReplayInput::read_dir(feature_dir)?;
    materialize(&input)
}

/// Render a snapshot as deterministic, human-readable JSON with a trailing LF.
pub fn render_snapshot_json(snapshot: &Snapshot) -> Result<String, serde_json::Error> {
    Ok(crate::mutation_json::python_indented_json(
        &snapshot.to_json_value(),
    ))
}

/// Resolve the schema generation this materialization emits at.
///
/// A brand-new mission (no existing snapshot) materializes at the current
/// generation. An existing snapshot keys replay off its own recorded
/// `schema_version`, whatever that is. A snapshot with no marker (including
/// every pre-version archive) replays at the legacy generation forever.
pub fn target_schema_version(existing_snapshot: Option<&Value>) -> i64 {
    if existing_snapshot.is_none() {
        return CURRENT_SNAPSHOT_SCHEMA_VERSION;
    }
    existing_schema_version(existing_snapshot).unwrap_or(LEGACY_SNAPSHOT_SCHEMA_VERSION)
}

/// The integer `schema_version` recorded in an existing snapshot object.
pub fn existing_schema_version(existing_snapshot: Option<&Value>) -> Option<i64> {
    let object = existing_snapshot?.as_object()?;
    match object.get("schema_version") {
        Some(Value::Number(number)) => number.as_i64(),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Projections
// ---------------------------------------------------------------------------

/// Project `cancellation_reason` / `reason_source` onto canceled WPs.
///
/// Exposed crate-internally so the writer's dependency-readiness verdict can
/// read the same provenance the materialized snapshot carries (the diary fold
/// alone does not project it). The reader pipeline is unchanged.
pub(crate) fn project_cancellation_provenance(
    transitions: &[StatusEvent],
    snapshot: &mut Snapshot,
) {
    for event in unique_by_first_occurrence(transitions) {
        if event.to_lane != Lane::Canceled {
            continue;
        }
        let Some(state) = snapshot.work_packages.get_mut(&event.wp_id) else {
            continue;
        };
        if state.lane() != Some(Lane::Canceled) {
            continue;
        }
        state.insert(
            "cancellation_reason",
            event
                .reason
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        state.insert(
            "reason_source",
            Value::String(cancellation_reason_source(event)),
        );
    }
}

fn cancellation_reason_source(event: &StatusEvent) -> String {
    if let Some(source) = &event.reason_source {
        return source.clone();
    }
    let Some(reason) = event
        .reason
        .as_deref()
        .filter(|reason| !reason.trim().is_empty())
    else {
        return "synthetic".to_owned();
    };
    if SYNTHETIC_REASON_PREFIXES
        .iter()
        .any(|prefix| reason.starts_with(prefix))
    {
        return "synthetic".to_owned();
    }
    "operator".to_owned()
}

fn project_implementer_attribution(transitions: &[StatusEvent], snapshot: &mut Snapshot) {
    let events = unique_by_first_occurrence(transitions);
    let claims: Vec<&StatusEvent> = events
        .iter()
        .filter(|event| is_claimant_bearing_claim(event))
        .copied()
        .collect();

    let mut latest_real_claim_at: HashMap<String, String> = HashMap::new();
    for event in &claims {
        if event.is_reconstruction_seed() {
            continue;
        }
        let current = latest_real_claim_at
            .get(&event.wp_id)
            .map(String::as_str)
            .unwrap_or("");
        if event.at.as_str() > current {
            latest_real_claim_at.insert(event.wp_id.clone(), event.at.clone());
        }
    }

    for event in claims {
        if event.is_reconstruction_seed()
            && latest_real_claim_at
                .get(&event.wp_id)
                .is_some_and(|latest| latest.as_str() >= event.at.as_str())
        {
            continue;
        }
        let Some(claimant) = event
            .policy_metadata
            .as_ref()
            .and_then(Value::as_object)
            .and_then(|meta| meta.get("agent"))
            .filter(|value| is_truthy(value))
        else {
            continue;
        };
        let Some(state) = snapshot.work_packages.get_mut(&event.wp_id) else {
            continue;
        };
        state.insert(
            IMPLEMENTER_OF_RECORD_SLOT,
            Value::String(python_str(claimant)),
        );
    }
}

fn is_claimant_bearing_claim(event: &StatusEvent) -> bool {
    event.from_lane == Lane::Planned
        && event.to_lane == Lane::Claimed
        && event
            .policy_metadata
            .as_ref()
            .and_then(Value::as_object)
            .and_then(|meta| meta.get("agent"))
            .is_some_and(is_truthy)
}

fn unique_by_first_occurrence(transitions: &[StatusEvent]) -> Vec<&StatusEvent> {
    let mut sorted: Vec<&StatusEvent> = transitions.iter().collect();
    sorted.sort_by(|left, right| {
        (left.at.as_str(), left.event_id.as_str())
            .cmp(&(right.at.as_str(), right.event_id.as_str()))
    });
    let mut seen = HashSet::new();
    sorted
        .into_iter()
        .filter(|event| seen.insert(event.event_id.clone()))
        .collect()
}

// ---------------------------------------------------------------------------
// Retrospective attachment
// ---------------------------------------------------------------------------

fn reduce_retrospective(rows: &[Value]) -> Option<RetrospectiveSnapshot> {
    let mut retro: Vec<&Map<String, Value>> = rows
        .iter()
        .filter_map(Value::as_object)
        .filter(|row| {
            row.get("event_name")
                .and_then(Value::as_str)
                .is_some_and(|name| name.starts_with("retrospective."))
        })
        .collect();
    if retro.is_empty() {
        return None;
    }
    retro.sort_by(|left, right| {
        (
            python_str(left.get("at").unwrap_or(&Value::Null)),
            python_str(left.get("event_id").unwrap_or(&Value::Null)),
        )
            .cmp(&(
                python_str(right.get("at").unwrap_or(&Value::Null)),
                python_str(right.get("event_id").unwrap_or(&Value::Null)),
            ))
    });

    let requested: Vec<&&Map<String, Value>> = retro
        .iter()
        .filter(|row| {
            row.get("event_name").and_then(Value::as_str) == Some("retrospective.requested")
        })
        .collect();
    let mode = requested
        .last()
        .and_then(|row| row.get("payload"))
        .and_then(Value::as_object)
        .and_then(|payload| payload.get("mode"))
        .filter(|value| !value.is_null())
        .and_then(validate_mode);

    const TERMINAL: [&str; 3] = [
        "retrospective.completed",
        "retrospective.skipped",
        "retrospective.failed",
    ];
    let latest_terminal = retro
        .iter()
        .rev()
        .find(|row| {
            row.get("event_name")
                .and_then(Value::as_str)
                .is_some_and(|name| TERMINAL.contains(&name))
        })
        .copied();
    let (status, record_path) = if let Some(event) = latest_terminal {
        let status = match event.get("event_name").and_then(Value::as_str) {
            Some("retrospective.completed") => "completed",
            Some("retrospective.skipped") => "skipped",
            _ => "failed",
        };
        let record_path = event
            .get("payload")
            .and_then(Value::as_object)
            .and_then(|payload| payload.get("record_path"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        (status.to_owned(), record_path)
    } else {
        ("pending".to_owned(), None)
    };

    let count = |name: &str| {
        retro
            .iter()
            .filter(|row| row.get("event_name").and_then(Value::as_str) == Some(name))
            .count() as u64
    };
    let proposals_total = count("retrospective.proposal.generated");
    let proposals_applied = count("retrospective.proposal.applied");
    let proposals_rejected = count("retrospective.proposal.rejected");
    let proposals_pending = proposals_total.saturating_sub(proposals_applied + proposals_rejected);

    Some(RetrospectiveSnapshot {
        status,
        mode,
        record_path,
        proposals_total,
        proposals_applied,
        proposals_rejected,
        proposals_pending,
    })
}

fn validate_mode(value: &Value) -> Option<Value> {
    let object = value.as_object()?;
    if object.len() != 2 {
        return None;
    }
    let mode = object.get("value")?.as_str()?;
    if !matches!(mode, "autonomous" | "human_in_command") {
        return None;
    }
    let signal = object.get("source_signal")?.as_object()?;
    if signal.len() != 2 {
        return None;
    }
    let kind = signal.get("kind")?.as_str()?;
    if !matches!(
        kind,
        "charter_override" | "explicit_flag" | "environment" | "parent_process"
    ) {
        return None;
    }
    let evidence = signal.get("evidence")?.as_str()?;
    Some(json_object([
        ("value", Value::String(mode.to_owned())),
        (
            "source_signal",
            json_object([
                ("kind", Value::String(kind.to_owned())),
                ("evidence", Value::String(evidence.to_owned())),
            ]),
        ),
    ]))
}

// ---------------------------------------------------------------------------
// Mission identity
// ---------------------------------------------------------------------------

struct MissionIdentity {
    mission_number: Option<String>,
    mission_type: Option<String>,
}

fn mission_identity(meta: Option<&Value>) -> Result<MissionIdentity, SnapshotError> {
    let Some(meta) = meta else {
        return Ok(MissionIdentity {
            mission_number: None,
            mission_type: Some(String::new()),
        });
    };
    let Some(object) = meta.as_object() else {
        return Err(SnapshotError::Meta {
            detail: "meta.json must be a JSON object".to_owned(),
        });
    };
    let mission_number = coerce_mission_number(object.get("mission_number"))?;
    let mission_type = object
        .get("mission_type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("")
        .to_owned();
    Ok(MissionIdentity {
        mission_number,
        mission_type: Some(mission_type),
    })
}

fn coerce_mission_number(raw: Option<&Value>) -> Result<Option<String>, SnapshotError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    match raw {
        Value::Null => Ok(None),
        Value::Bool(_) => Err(SnapshotError::Meta {
            detail: "mission_number must be int, str, or null".to_owned(),
        }),
        Value::Number(number) => number
            .as_i64()
            .map(|value| Some(value.to_string()))
            .ok_or_else(|| SnapshotError::Meta {
                detail: "mission_number must be an integer".to_owned(),
            }),
        Value::String(text) => {
            let stripped = text.trim();
            if stripped.is_empty() {
                return Ok(None);
            }
            if matches!(stripped, "pending" | "unassigned" | "TBD") {
                return Err(SnapshotError::Meta {
                    detail: format!("mission_number must be int or null, got {text:?}"),
                });
            }
            let digits = stripped.trim_start_matches('0');
            let normalized = if digits.is_empty() { "0" } else { digits };
            normalized
                .parse::<i64>()
                .map(|value| Some(value.to_string()))
                .map_err(|_| SnapshotError::Meta {
                    detail: format!("mission_number must be int or null, got {text:?}"),
                })
        }
        _ => Err(SnapshotError::Meta {
            detail: "mission_number must be int, str, or null".to_owned(),
        }),
    }
}

fn mission_number_from_slug(mission_slug: &str) -> Option<u64> {
    let slug = mission_slug.trim();
    let (prefix, _) = slug.split_once('-')?;
    if prefix.is_empty() || !prefix.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    prefix.parse::<u64>().ok()
}

// ---------------------------------------------------------------------------
// File helpers
// ---------------------------------------------------------------------------

fn read_optional_bytes(path: &Path, limit: u64) -> Result<Option<Vec<u8>>, SnapshotError> {
    match sk_kernel::read_bounded(path, limit) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(sk_kernel::KernelError::Io { source, .. })
            if source.kind() == io::ErrorKind::NotFound =>
        {
            Ok(None)
        }
        Err(source) => Err(SnapshotError::InputRead {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn read_optional_json_strict(path: &Path) -> Result<Option<Value>, SnapshotError> {
    let Some(bytes) = read_optional_bytes(path, MAX_META_BYTES)? else {
        return Ok(None);
    };
    let value: Value = serde_json::from_slice(&bytes).map_err(|error| SnapshotError::Meta {
        detail: error.to_string(),
    })?;
    if !value.is_object() {
        return Err(SnapshotError::Meta {
            detail: "meta.json must be a JSON object".to_owned(),
        });
    }
    Ok(Some(value))
}

fn read_optional_json_lenient(path: &Path) -> Result<Option<Value>, SnapshotError> {
    let Some(bytes) = read_optional_bytes(path, MAX_SNAPSHOT_BYTES)? else {
        return Ok(None);
    };
    // The Python resolver checks existence before parsing. Even malformed JSON
    // and valid non-object JSON must remain *present*, selecting legacy replay.
    Ok(Some(serde_json::from_slice(&bytes).unwrap_or(Value::Null)))
}

fn json_object<const N: usize>(entries: [(&str, Value); N]) -> Value {
    let mut map = Map::new();
    for (key, value) in entries {
        map.insert(key.to_owned(), value);
    }
    Value::Object(map)
}
