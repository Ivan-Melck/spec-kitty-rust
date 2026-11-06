//! Canonical Source Mission progress/lifecycle/board projections.
//!
//! These are the derived views the runtime-state eviction and the lifecycle
//! normalization must regenerate. They are **output-only**: the event log stays
//! the sole authority. This module is the one projection authority —
//!
//! * [`compute_weighted_progress`] — Source `compute_weighted_progress`
//!   (lane-weighted readiness, exact weights and numeric rounding);
//! * [`derive_lifecycle`] — Source `derive_mission_lifecycle` (activity,
//!   terminality, re-open/follow-up post-mission facts, staleness bands);
//! * [`build_board_summary`] — Source `_build_board_summary`;
//! * [`write_mission_projection`] / [`project`] — the writer that emits
//!   `status.json`, `board-summary.json`, `progress.json` and `lifecycle.json`.
//!
//! Everything is computed from the canonical reducer/materializer
//! ([`crate::snapshot::materialize`]) and encoded with the one canonical JSON
//! encoder ([`crate::mutation_json::python_indented_json`]). There is no second
//! reducer, snapshot model, encoder or event writer here, and the tracked
//! `<mission>/status.json` write is Source `materialize`'s own documented
//! behaviour (it rewrites the file only when the rendered bytes differ).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::snapshot::{META_FILENAME, ReplayInput, Snapshot, materialize};

/// The derived-views root, relative to the repository root.
pub const DERIVED_VIEWS_DIR: &str = ".kittify/derived";

/// The tracked snapshot file name inside a mission directory.
pub const STATUS_FILENAME: &str = "status.json";

/// The derived progress view.
pub const PROGRESS_FILENAME: &str = "progress.json";

/// The derived lifecycle view.
pub const LIFECYCLE_FILENAME: &str = "lifecycle.json";

/// The derived board summary.
pub const BOARD_SUMMARY_FILENAME: &str = "board-summary.json";

/// The canonical event log inside a mission directory.
const EVENTS_FILENAME: &str = "status.events.jsonl";

/// The progress semantics stamp (`PROGRESS_SEMANTICS`).
pub const PROGRESS_SEMANTICS: &str = "weighted_readiness";

/// Source `DEFAULT_LANE_WEIGHTS` (all eleven lanes, `genesis`/`uninitialized`
/// pinned at 0 because they are non-display lanes).
pub const LANE_WEIGHTS: [(&str, f64); 11] = [
    ("genesis", 0.0),
    ("uninitialized", 0.0),
    ("planned", 0.0),
    ("claimed", 0.05),
    ("in_progress", 0.3),
    ("for_review", 0.6),
    ("in_review", 0.7),
    ("approved", 0.8),
    ("done", 1.0),
    ("blocked", 0.0),
    ("canceled", 0.0),
];

/// A mission completed within this many days is still surfaced.
pub const MISSION_RECENT_COMPLETION_WINDOW_DAYS: i64 = 3;

/// Active work older than this is stale.
pub const MISSION_STALE_THRESHOLD_DAYS: i64 = 14;

/// Active work older than this is abandoned.
pub const MISSION_ABANDONED_THRESHOLD_DAYS: i64 = 30;

/// Lanes that count as live work (`_ACTIVE_LANES`).
const ACTIVE_LANES: [&str; 6] = [
    "claimed",
    "in_progress",
    "for_review",
    "in_review",
    "approved",
    "blocked",
];

/// The two post-mission lifecycle event types.
pub const POST_MISSION_EVENT_TYPES: [&str; 2] = ["MissionReopened", "FollowUpRecorded"];

/// One day in microseconds (Source truncates `timedelta(days=1)` ratios).
const MICROS_PER_DAY: f64 = 86_400_000_000.0;

/// Why a projection could not be produced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectionError {
    /// `meta.json` is malformed or an identity value cannot be coerced.
    Identity(String),
    /// The event log or `meta.json` cannot be read.
    Unreadable(String),
    /// A derived view could not be written.
    Write(String),
}

impl std::fmt::Display for ProjectionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProjectionError::Identity(detail)
            | ProjectionError::Unreadable(detail)
            | ProjectionError::Write(detail) => formatter.write_str(detail),
        }
    }
}

// ---------------------------------------------------------------------------
// Encoders and numeric rules
// ---------------------------------------------------------------------------

/// Round like Python's `round(value, digits)` (correctly-rounded decimal,
/// half-to-even) by rendering and re-parsing the decimal.
fn round_to(value: f64, digits: usize) -> f64 {
    let rendered = match digits {
        1 => format!("{value:.1}"),
        _ => format!("{value:.4}"),
    };
    rendered.parse::<f64>().unwrap_or(value)
}

fn json_number(value: f64) -> Value {
    serde_json::Number::from_f64(value)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

fn json_string_or_null(value: Option<&str>) -> Value {
    value
        .map(|text| Value::String(text.to_owned()))
        .unwrap_or(Value::Null)
}

fn isoformat(instant: sk_kernel::clock::UtcDateTime<sk_kernel::clock::UtcZone>) -> String {
    if instant.timestamp_subsec_micros() == 0 {
        instant.format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
    } else {
        instant.format("%Y-%m-%dT%H:%M:%S%.6f+00:00").to_string()
    }
}

fn parse_instant(
    raw: Option<&Value>,
) -> Option<sk_kernel::clock::UtcDateTime<sk_kernel::clock::UtcZone>> {
    let text = raw?.as_str()?.trim();
    if text.is_empty() {
        return None;
    }
    sk_kernel::clock::parse_iso(text).ok()
}

/// A safe single-segment mission slug, or the trusted directory name.
fn safe_mission_slug(raw: Option<&Value>, fallback: &str) -> String {
    let Some(candidate) = raw.and_then(Value::as_str) else {
        return fallback.to_owned();
    };
    let candidate = candidate.trim();
    let unsafe_slug = candidate.is_empty()
        || candidate == "."
        || candidate == ".."
        || candidate.contains('/')
        || candidate.contains('\\')
        || candidate.chars().any(|character| character.is_control());
    if unsafe_slug {
        fallback.to_owned()
    } else {
        candidate.to_owned()
    }
}

// ---------------------------------------------------------------------------
// Progress
// ---------------------------------------------------------------------------

/// One work package's progress contribution.
#[derive(Clone, Debug, PartialEq)]
pub struct WpProgress {
    /// The work-package id.
    pub wp_id: String,
    /// Its current lane.
    pub lane: String,
    /// The lane's weight.
    pub lane_weight: f64,
    /// The WP's weight (default 1.0).
    pub wp_weight: f64,
    /// `lane_weight * wp_weight`.
    pub fractional_progress: f64,
}

impl WpProgress {
    fn to_json_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("wp_id".to_owned(), Value::String(self.wp_id.clone()));
        map.insert("lane".to_owned(), Value::String(self.lane.clone()));
        map.insert("lane_weight".to_owned(), json_number(self.lane_weight));
        map.insert("wp_weight".to_owned(), json_number(self.wp_weight));
        map.insert(
            "fractional_progress".to_owned(),
            json_number(self.fractional_progress),
        );
        Value::Object(map)
    }
}

/// Source `ProgressResult`.
#[derive(Clone, Debug, PartialEq)]
pub struct ProgressProjection {
    /// Mission slug.
    pub mission_slug: String,
    /// Mission number as the payload's display string.
    pub mission_number: String,
    /// Mission type ("" when typeless).
    pub mission_type: String,
    /// The raw weighted percentage (unrounded).
    pub percentage: f64,
    /// WPs in `done`.
    pub done_count: usize,
    /// Total WPs.
    pub total_count: usize,
    /// Lane → count.
    pub per_lane_counts: BTreeMap<String, u64>,
    /// Per-WP breakdown, in `wp_id` order.
    pub per_wp: Vec<WpProgress>,
}

impl ProgressProjection {
    /// Done-only percentage.
    pub fn done_percentage(&self) -> f64 {
        if self.total_count == 0 {
            return 0.0;
        }
        self.done_count as f64 / self.total_count as f64 * 100.0
    }

    /// The exact `ProgressResult.to_dict()` payload.
    pub fn to_json_value(&self) -> Value {
        let mut map = Map::new();
        map.insert(
            "mission_slug".to_owned(),
            Value::String(self.mission_slug.clone()),
        );
        map.insert(
            "mission_number".to_owned(),
            Value::String(self.mission_number.clone()),
        );
        map.insert(
            "mission_type".to_owned(),
            Value::String(self.mission_type.clone()),
        );
        map.insert(
            "percentage".to_owned(),
            json_number(round_to(self.percentage, 4)),
        );
        map.insert(
            "progress_semantics".to_owned(),
            Value::String(PROGRESS_SEMANTICS.to_owned()),
        );
        map.insert(
            "weighted_percentage".to_owned(),
            json_number(round_to(self.percentage, 4)),
        );
        map.insert(
            "done_percentage".to_owned(),
            json_number(round_to(self.done_percentage(), 4)),
        );
        map.insert("done_count".to_owned(), Value::from(self.done_count));
        map.insert("total_count".to_owned(), Value::from(self.total_count));
        let mut lanes = Map::new();
        for (lane, count) in &self.per_lane_counts {
            lanes.insert(lane.clone(), Value::from(*count));
        }
        map.insert("per_lane_counts".to_owned(), Value::Object(lanes));
        map.insert(
            "per_wp".to_owned(),
            Value::Array(self.per_wp.iter().map(WpProgress::to_json_value).collect()),
        );
        Value::Object(map)
    }
}

fn lane_weight(lane: &str) -> f64 {
    LANE_WEIGHTS
        .iter()
        .find(|(name, _)| *name == lane)
        .map(|(_, weight)| *weight)
        .unwrap_or(0.0)
}

/// Compute lane-weighted progress from a snapshot (a pure function).
pub fn compute_weighted_progress(
    snapshot: &Snapshot,
    wp_weights: Option<&BTreeMap<String, f64>>,
    lane_weights: Option<&BTreeMap<String, f64>>,
) -> ProgressProjection {
    let status = snapshot.to_json_value();
    let display_number = status
        .get("mission_number")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let display_type = status
        .get("mission_type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if snapshot.work_packages.is_empty() {
        return ProgressProjection {
            mission_slug: snapshot.mission_slug.clone(),
            mission_number: display_number,
            mission_type: display_type,
            percentage: 0.0,
            done_count: 0,
            total_count: 0,
            per_lane_counts: BTreeMap::new(),
            per_wp: Vec::new(),
        };
    }
    let mut per_lane_counts: BTreeMap<String, u64> = BTreeMap::new();
    let mut per_wp: Vec<WpProgress> = Vec::with_capacity(snapshot.work_packages.len());
    let mut weighted_sum = 0.0_f64;
    let mut weight_total = 0.0_f64;
    let mut done_count = 0_usize;
    for (wp_id, state) in &snapshot.work_packages {
        let lane = state
            .get("lane")
            .and_then(Value::as_str)
            .unwrap_or("genesis")
            .to_owned();
        let wp_weight = wp_weights
            .and_then(|weights| weights.get(wp_id))
            .copied()
            .unwrap_or(1.0);
        let weight = lane_weights
            .and_then(|weights| weights.get(&lane))
            .copied()
            .unwrap_or_else(|| lane_weight(&lane));
        let fractional = wp_weight * weight;
        weighted_sum += fractional;
        weight_total += wp_weight;
        *per_lane_counts.entry(lane.clone()).or_insert(0) += 1;
        if lane == "done" {
            done_count += 1;
        }
        per_wp.push(WpProgress {
            wp_id: wp_id.clone(),
            lane,
            lane_weight: weight,
            wp_weight,
            fractional_progress: fractional,
        });
    }
    let percentage = if weight_total > 0.0 {
        weighted_sum / weight_total * 100.0
    } else {
        0.0
    };
    ProgressProjection {
        mission_slug: snapshot.mission_slug.clone(),
        mission_number: display_number,
        mission_type: display_type,
        percentage,
        done_count,
        total_count: snapshot.work_packages.len(),
        per_lane_counts,
        per_wp,
    }
}

// ---------------------------------------------------------------------------
// Per-lane lifecycle buckets (Source `wp_state_for(...).progress_bucket()`)
// ---------------------------------------------------------------------------

fn progress_bucket(lane: &str) -> &'static str {
    match lane {
        "for_review" | "in_review" | "approved" => "review",
        "done" | "canceled" => "terminal",
        "claimed" | "in_progress" | "blocked" => "in_flight",
        _ => "not_started",
    }
}

fn is_terminal_lane(lane: &str) -> bool {
    matches!(lane, "done" | "canceled")
}

fn is_blocked_lane(lane: &str) -> bool {
    lane == "blocked"
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

/// The classified lifecycle state and its two companion strings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LifecycleClassification {
    /// `reopened` / `active` / `stale` / `abandoned` / `recently_completed` /
    /// `archived` / `recoverable`.
    pub state: &'static str,
    /// The surface the mission belongs on, when it has one.
    pub surface_state: Option<&'static str>,
    /// The operator-facing reason.
    pub reason: &'static str,
}

/// Source `MissionLifecycleResult`.
#[derive(Clone, Debug, PartialEq)]
pub struct LifecycleProjection {
    /// Mission slug.
    pub mission_slug: String,
    /// Recorded canonical mission id, when the mission declares one.
    pub mission_id: Option<Value>,
    /// Mission number as an integer, when coerceable.
    pub mission_number: Option<i64>,
    /// Mission type ("" when typeless).
    pub mission_type: String,
    /// The classification.
    pub classification: LifecycleClassification,
    /// Last activity (transition, else `created_at`, else directory mtime).
    pub last_activity_at: Option<String>,
    /// Last transition from the reduced snapshot.
    pub last_transition_at: Option<String>,
    /// Whole days since `last_activity_at` (0 when absent/future).
    pub age_days: Option<i64>,
    /// The weighted completion percentage.
    pub completion_pct: f64,
    /// Events in the reduced log.
    pub event_count: usize,
    /// Total WPs.
    pub total_wps: usize,
    /// WPs in a live lane.
    pub active_wp_count: usize,
    /// WPs in `blocked`.
    pub blocked_wp_count: usize,
    /// WPs in a review bucket.
    pub review_wp_count: usize,
    /// WPs in a terminal lane.
    pub terminal_wp_count: usize,
    /// Whether the mission has an event log.
    pub has_event_log: bool,
    /// `MissionReopened` / `FollowUpRecorded` rows, `(timestamp, event_id)` order.
    pub post_mission_events: Vec<Value>,
    /// Timestamp of the most recent `FollowUpRecorded`.
    pub last_follow_up_at: Option<String>,
    /// The stale band.
    pub stale_after_days: i64,
    /// The abandoned band.
    pub abandoned_after_days: i64,
}

impl LifecycleProjection {
    /// The exact `MissionLifecycleResult.to_dict()` payload.
    pub fn to_json_value(&self) -> Value {
        let mut map = Map::new();
        map.insert(
            "mission_slug".to_owned(),
            Value::String(self.mission_slug.clone()),
        );
        map.insert(
            "mission_id".to_owned(),
            self.mission_id.clone().unwrap_or(Value::Null),
        );
        map.insert(
            "mission_number".to_owned(),
            self.mission_number.map(Value::from).unwrap_or(Value::Null),
        );
        map.insert(
            "mission_type".to_owned(),
            Value::String(self.mission_type.clone()),
        );
        map.insert(
            "state".to_owned(),
            Value::String(self.classification.state.to_owned()),
        );
        map.insert(
            "surface_state".to_owned(),
            json_string_or_null(self.classification.surface_state),
        );
        map.insert(
            "reason".to_owned(),
            Value::String(self.classification.reason.to_owned()),
        );
        map.insert(
            "last_activity_at".to_owned(),
            json_string_or_null(self.last_activity_at.as_deref()),
        );
        map.insert(
            "last_transition_at".to_owned(),
            json_string_or_null(self.last_transition_at.as_deref()),
        );
        map.insert(
            "age_days".to_owned(),
            self.age_days.map(Value::from).unwrap_or(Value::Null),
        );
        map.insert(
            "completion_pct".to_owned(),
            json_number(round_to(self.completion_pct, 1)),
        );
        map.insert("event_count".to_owned(), Value::from(self.event_count));
        map.insert("total_wps".to_owned(), Value::from(self.total_wps));
        map.insert(
            "active_wp_count".to_owned(),
            Value::from(self.active_wp_count),
        );
        map.insert(
            "blocked_wp_count".to_owned(),
            Value::from(self.blocked_wp_count),
        );
        map.insert(
            "review_wp_count".to_owned(),
            Value::from(self.review_wp_count),
        );
        map.insert(
            "terminal_wp_count".to_owned(),
            Value::from(self.terminal_wp_count),
        );
        map.insert("has_event_log".to_owned(), Value::Bool(self.has_event_log));
        map.insert(
            "post_mission_events".to_owned(),
            Value::Array(self.post_mission_events.clone()),
        );
        map.insert(
            "last_follow_up_at".to_owned(),
            json_string_or_null(self.last_follow_up_at.as_deref()),
        );
        map.insert(
            "stale_after_days".to_owned(),
            Value::from(self.stale_after_days),
        );
        map.insert(
            "abandoned_after_days".to_owned(),
            Value::from(self.abandoned_after_days),
        );
        Value::Object(map)
    }
}

/// Classify a mission exactly like Source `_classify_state`.
pub fn classify_lifecycle(
    total_wps: usize,
    active_wp_count: usize,
    terminal_wp_count: usize,
    age_days: Option<i64>,
    is_reopened: bool,
) -> LifecycleClassification {
    let aged = |threshold: i64| age_days.is_some_and(|days| days >= threshold);
    if is_reopened {
        return LifecycleClassification {
            state: "reopened",
            surface_state: Some("reopened"),
            reason: "Mission was re-opened after merge and is actionable again.",
        };
    }
    if active_wp_count > 0 {
        if aged(MISSION_ABANDONED_THRESHOLD_DAYS) {
            return LifecycleClassification {
                state: "abandoned",
                surface_state: None,
                reason: "Active work appears abandoned and should stay off primary surfaces.",
            };
        }
        if aged(MISSION_STALE_THRESHOLD_DAYS) {
            return LifecycleClassification {
                state: "stale",
                surface_state: None,
                reason: "Active work has gone stale and should be recovered before it resurfaces.",
            };
        }
        return LifecycleClassification {
            state: "active",
            surface_state: Some("active"),
            reason: "Mission has live work in progress or review.",
        };
    }
    if total_wps > 0 && terminal_wp_count == total_wps {
        if age_days.is_some_and(|days| days <= MISSION_RECENT_COMPLETION_WINDOW_DAYS) {
            return LifecycleClassification {
                state: "recently_completed",
                surface_state: Some("recently_completed"),
                reason: "Mission completed recently and still deserves short-lived visibility.",
            };
        }
        return LifecycleClassification {
            state: "archived",
            surface_state: None,
            reason: "Mission is completed and now belongs in history rather than the default surface.",
        };
    }
    if aged(MISSION_ABANDONED_THRESHOLD_DAYS) {
        return LifecycleClassification {
            state: "abandoned",
            surface_state: None,
            reason: "Mission never reached an active terminal state and now looks abandoned.",
        };
    }
    if aged(MISSION_STALE_THRESHOLD_DAYS) {
        return LifecycleClassification {
            state: "stale",
            surface_state: None,
            reason: "Mission is recoverable but stale and should not pollute primary surfaces.",
        };
    }
    LifecycleClassification {
        state: "recoverable",
        surface_state: None,
        reason: "Mission is recoverable history and not active enough for the default surface.",
    }
}

/// Read the mission-level lifecycle rows (any object line with a string
/// `event_type`), tolerating blank/malformed lines exactly like Source.
fn read_lifecycle_rows(feature_dir: &Path) -> Vec<Value> {
    let path = feature_dir.join(EVENTS_FILENAME);
    let Ok(bytes) = std::fs::read(&path) else {
        return Vec::new();
    };
    let Ok(text) = String::from_utf8(bytes) else {
        return Vec::new();
    };
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|row| row.get("event_type").and_then(Value::as_str).is_some())
        .collect()
}

fn collect_post_mission_events(feature_dir: &Path) -> Vec<Value> {
    let mut relevant: Vec<Value> = read_lifecycle_rows(feature_dir)
        .into_iter()
        .filter(|row| {
            row.get("event_type")
                .and_then(Value::as_str)
                .is_some_and(|kind| POST_MISSION_EVENT_TYPES.contains(&kind))
        })
        .collect();
    relevant.sort_by(|left, right| {
        let key = |row: &Value| {
            (
                row.get("timestamp")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                row.get("event_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            )
        };
        key(left).cmp(&key(right))
    });
    relevant
}

/// The best-effort event time: the typed payload timestamp, else the envelope's.
fn latest_event_time(
    event: &Value,
) -> Option<sk_kernel::clock::UtcDateTime<sk_kernel::clock::UtcZone>> {
    if let Some(payload) = event.get("payload").and_then(Value::as_object) {
        for key in ["reopened_at", "recorded_at"] {
            if let Some(instant) = parse_instant(payload.get(key)) {
                return Some(instant);
            }
        }
    }
    parse_instant(event.get("timestamp"))
}

fn last_time_of_type(
    events: &[Value],
    event_type: &str,
) -> Option<sk_kernel::clock::UtcDateTime<sk_kernel::clock::UtcZone>> {
    events
        .iter()
        .filter(|event| event.get("event_type").and_then(Value::as_str) == Some(event_type))
        .filter_map(latest_event_time)
        .max()
}

fn meta_object(input: &ReplayInput) -> Result<Option<&Map<String, Value>>, ProjectionError> {
    match input.meta.as_ref() {
        None => Ok(None),
        Some(Value::Object(map)) => Ok(Some(map)),
        Some(other) => Err(ProjectionError::Identity(format!(
            "{META_FILENAME} must be a JSON object, observed {other}"
        ))),
    }
}

/// Directory mtime as an aware-UTC instant (Source `from_epoch(mtime)`).
fn directory_mtime(
    feature_dir: &Path,
) -> Option<sk_kernel::clock::UtcDateTime<sk_kernel::clock::UtcZone>> {
    let modified = std::fs::metadata(feature_dir).ok()?.modified().ok()?;
    let since = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    sk_kernel::clock::from_epoch(since.as_secs_f64()).ok()
}

/// The last transition instant carried by the reduced work packages.
fn last_transition_instant(
    snapshot: &Snapshot,
) -> Option<sk_kernel::clock::UtcDateTime<sk_kernel::clock::UtcZone>> {
    let mut latest: Option<sk_kernel::clock::UtcDateTime<sk_kernel::clock::UtcZone>> = None;
    for state in snapshot.work_packages.values() {
        if let Some(instant) = parse_instant(state.get("last_transition_at")) {
            latest = Some(match latest {
                Some(current) if current >= instant => current,
                _ => instant,
            });
        }
    }
    latest.or_else(|| {
        let raw = Value::String(snapshot.materialized_at.clone());
        parse_instant(Some(&raw))
    })
}

/// Derive the canonical lifecycle projection for one mission.
///
/// `snapshot` lets a caller share one materialization across the whole view set
/// (Source's `snapshot=` parameter); when omitted the snapshot is materialized
/// here, write-free.
pub fn derive_lifecycle(
    feature_dir: &Path,
    snapshot: Option<&Snapshot>,
    now: sk_kernel::clock::UtcDateTime<sk_kernel::clock::UtcZone>,
) -> Result<LifecycleProjection, ProjectionError> {
    let materialized;
    let snapshot: &Snapshot = match snapshot {
        Some(snapshot) => snapshot,
        None => {
            let input = ReplayInput::read_dir(feature_dir)
                .map_err(|error| ProjectionError::Unreadable(error.to_string()))?;
            materialized = materialize(&input)
                .map_err(|error| ProjectionError::Identity(error.to_string()))?;
            &materialized
        }
    };
    let input = ReplayInput::read_dir(feature_dir)
        .map_err(|error| ProjectionError::Unreadable(error.to_string()))?;
    let meta = meta_object(&input)?;
    let fallback_slug = feature_dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let identity_slug = safe_mission_slug(
        meta.and_then(|map| map.get("mission_slug").or_else(|| map.get("slug"))),
        &fallback_slug,
    );
    let mission_slug = if snapshot.mission_slug.trim().is_empty() {
        identity_slug
    } else {
        snapshot.mission_slug.trim().to_owned()
    };
    let status_payload = snapshot.to_json_value();
    let mission_type = status_payload
        .get("mission_type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let mission_number = status_payload
        .get("mission_number")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .and_then(|text| text.parse::<i64>().ok());
    let mission_id = meta.and_then(|map| map.get("mission_id")).cloned();
    let has_event_log = feature_dir.join(EVENTS_FILENAME).exists();

    let progress = compute_weighted_progress(snapshot, None, None);
    let total_wps = snapshot.work_packages.len();
    let mut active_wp_count = 0_usize;
    let mut blocked_wp_count = 0_usize;
    let mut review_wp_count = 0_usize;
    let mut terminal_wp_count = 0_usize;
    for state in snapshot.work_packages.values() {
        let lane = state
            .get("lane")
            .and_then(Value::as_str)
            .unwrap_or("genesis");
        if ACTIVE_LANES.contains(&lane) {
            active_wp_count += 1;
        }
        if is_blocked_lane(lane) {
            blocked_wp_count += 1;
        }
        if progress_bucket(lane) == "review" {
            review_wp_count += 1;
        }
        if is_terminal_lane(lane) {
            terminal_wp_count += 1;
        }
    }

    let last_transition_at = last_transition_instant(snapshot);
    let last_activity_at = last_transition_at.or_else(|| {
        meta.and_then(|map| parse_instant(map.get("created_at")))
            .or_else(|| directory_mtime(feature_dir))
    });
    let age_days = last_activity_at.map(|activity| {
        let micros = (now - activity)
            .num_microseconds()
            .unwrap_or_else(|| (now - activity).num_seconds().saturating_mul(1_000_000));
        let days = (micros as f64 / MICROS_PER_DAY).trunc() as i64;
        days.max(0)
    });

    let post_mission_events = collect_post_mission_events(feature_dir);
    let last_reopen_at = last_time_of_type(&post_mission_events, "MissionReopened");
    let last_follow_up_at = last_time_of_type(&post_mission_events, "FollowUpRecorded");
    let last_merge_at = meta.and_then(|map| parse_instant(map.get("merged_at")));
    let is_reopened = last_reopen_at.is_some_and(|reopened| match last_merge_at {
        None => true,
        Some(merged) => reopened > merged,
    });
    let classification = classify_lifecycle(
        total_wps,
        active_wp_count,
        terminal_wp_count,
        age_days,
        is_reopened,
    );

    Ok(LifecycleProjection {
        mission_slug,
        mission_id,
        mission_number,
        mission_type,
        classification,
        last_activity_at: last_activity_at.map(isoformat),
        last_transition_at: last_transition_at.map(isoformat),
        age_days,
        completion_pct: progress.percentage,
        event_count: snapshot.event_count,
        total_wps,
        active_wp_count,
        blocked_wp_count,
        review_wp_count,
        terminal_wp_count,
        has_event_log,
        post_mission_events,
        last_follow_up_at: last_follow_up_at.map(isoformat),
        stale_after_days: MISSION_STALE_THRESHOLD_DAYS,
        abandoned_after_days: MISSION_ABANDONED_THRESHOLD_DAYS,
    })
}

// ---------------------------------------------------------------------------
// Board summary
// ---------------------------------------------------------------------------

/// Source `_build_board_summary`.
pub fn build_board_summary(snapshot: &Snapshot) -> Value {
    let mut lanes: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (wp_id, state) in &snapshot.work_packages {
        let lane = state
            .get("lane")
            .and_then(Value::as_str)
            .unwrap_or("genesis")
            .to_owned();
        lanes.entry(lane).or_default().push(wp_id.clone());
    }
    let mut lane_map = Map::new();
    for (lane, wp_ids) in lanes {
        lane_map.insert(
            lane,
            Value::Array(wp_ids.into_iter().map(Value::String).collect()),
        );
    }
    let status_payload = snapshot.to_json_value();
    // `int(mission_number)` when the string is all digits (arbitrary precision,
    // exactly like Python's int()).
    let mission_number = match status_payload.get("mission_number").and_then(Value::as_str) {
        Some(text) if !text.is_empty() && text.chars().all(|c| c.is_ascii_digit()) => {
            serde_json::from_str::<Value>(text).unwrap_or(Value::String(text.to_owned()))
        }
        Some(_) => Value::Null,
        None => Value::Null,
    };
    let mut summary = Map::new();
    for (lane, count) in &snapshot.summary {
        summary.insert(lane.clone(), Value::from(*count));
    }
    let mut map = Map::new();
    map.insert(
        "mission_slug".to_owned(),
        Value::String(snapshot.mission_slug.clone()),
    );
    map.insert("mission_number".to_owned(), mission_number);
    map.insert(
        "mission_type".to_owned(),
        status_payload
            .get("mission_type")
            .cloned()
            .unwrap_or(Value::String(String::new())),
    );
    map.insert(
        "total_wps".to_owned(),
        Value::from(snapshot.work_packages.len()),
    );
    map.insert("summary".to_owned(), Value::Object(summary));
    map.insert("lanes".to_owned(), Value::Object(lane_map));
    map.insert(
        "materialized_at".to_owned(),
        Value::String(snapshot.materialized_at.clone()),
    );
    Value::Object(map)
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// One file the projection wrote.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WrittenView {
    /// The path written.
    pub path: PathBuf,
    /// The bytes written.
    pub bytes: usize,
    /// Whether the bytes differ from what was there before.
    pub changed: bool,
}

/// The four views one projection run produces, plus the tracked snapshot write.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProjectionReceipt {
    /// `<mission>/status.json` (Source `materialize`'s own rewrite).
    pub tracked_status: Option<WrittenView>,
    /// `.kittify/derived/<slug>/status.json`.
    pub derived_status: Option<WrittenView>,
    /// `.kittify/derived/<slug>/board-summary.json`.
    pub board: Option<WrittenView>,
    /// `.kittify/derived/<slug>/progress.json`.
    pub progress: Option<WrittenView>,
    /// `.kittify/derived/<slug>/lifecycle.json`.
    pub lifecycle: Option<WrittenView>,
}

impl ProjectionReceipt {
    /// Whether the run wrote nothing new.
    pub fn is_noop(&self) -> bool {
        [
            &self.tracked_status,
            &self.derived_status,
            &self.board,
            &self.progress,
            &self.lifecycle,
        ]
        .iter()
        .all(|view| view.as_ref().is_none_or(|view| !view.changed))
    }

    /// Every view that actually changed on disk.
    pub fn changed_views(&self) -> Vec<&WrittenView> {
        [
            &self.tracked_status,
            &self.derived_status,
            &self.board,
            &self.progress,
            &self.lifecycle,
        ]
        .into_iter()
        .flatten()
        .filter(|view| view.changed)
        .collect()
    }
}

fn is_safe_segment(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains('/')
        && !value.contains('\\')
        && !value.chars().any(|character| character.is_control())
}

/// The pure projection set: everything computed, nothing written.
#[derive(Clone, Debug)]
pub struct MissionProjection {
    /// The mission slug the views are keyed on.
    pub slug: String,
    /// The canonical snapshot payload (`status.json`).
    pub status: Value,
    /// The board summary payload.
    pub board: Value,
    /// The progress payload.
    pub progress: Value,
    /// The lifecycle payload.
    pub lifecycle: Value,
    /// The encoded tracked snapshot bytes.
    pub status_bytes: String,
}

/// Compute every derived view for one mission without writing anything.
pub fn project(
    feature_dir: &Path,
    now: sk_kernel::clock::UtcDateTime<sk_kernel::clock::UtcZone>,
) -> Result<MissionProjection, ProjectionError> {
    let input = ReplayInput::read_dir(feature_dir)
        .map_err(|error| ProjectionError::Unreadable(error.to_string()))?;
    let snapshot =
        materialize(&input).map_err(|error| ProjectionError::Identity(error.to_string()))?;
    let lifecycle = derive_lifecycle(feature_dir, Some(&snapshot), now)?;
    let progress = compute_weighted_progress(&snapshot, None, None);
    let board = build_board_summary(&snapshot);
    let status = snapshot.to_json_value();
    let status_bytes = crate::mutation_json::python_indented_json(&status);
    let slug = if snapshot.mission_slug.trim().is_empty() {
        feature_dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        snapshot.mission_slug.trim().to_owned()
    };
    let slug = if is_safe_segment(&slug) {
        slug
    } else {
        lifecycle.mission_slug.clone()
    };
    Ok(MissionProjection {
        slug,
        status,
        board,
        progress: progress.to_json_value(),
        lifecycle: lifecycle.to_json_value(),
        status_bytes,
    })
}

fn write_view(path: &Path, text: &str) -> Result<WrittenView, ProjectionError> {
    let previous = std::fs::read(path).ok();
    let bytes = text.as_bytes();
    let changed = previous.as_deref() != Some(bytes);
    if changed {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                ProjectionError::Write(format!("cannot create {}: {error}", parent.display()))
            })?;
        }
        // Source `status.views._atomic_write_json`: fixed `<stem>.json.tmp`
        // sibling, `Path.write_text` creation mode, rename, then the write
        // observer. A failure leaves that artifact behind, exactly as the
        // reference does.
        sk_kernel::source_writer::write_source(
            sk_kernel::source_writer::WriterOperation::DerivedJson,
            path,
            sk_kernel::source_writer::SourcePayload::Bytes(bytes),
            None,
        )
        .map_err(|error| {
            ProjectionError::Write(format!("cannot write {}: {error}", path.display()))
        })?;
    }
    Ok(WrittenView {
        path: path.to_path_buf(),
        bytes: bytes.len(),
        changed,
    })
}

/// The derived directory for one mission slug under a project root's
/// `.kittify/derived/`.
pub fn derived_dir_for(project_root: &Path, slug: &str) -> PathBuf {
    project_root.join(DERIVED_VIEWS_DIR).join(slug)
}

/// Write every derived view for one mission (Source `write_derived_views` +
/// `generate_progress_json` + `generate_lifecycle_json` in one authority).
///
/// The tracked `<mission>/status.json` is rewritten only when the rendered bytes
/// differ, exactly like Source `materialize`; the four derived files are written
/// under `derived_root/<slug>/`.
pub fn write_mission_projection(
    feature_dir: &Path,
    derived_root: &Path,
    now: sk_kernel::clock::UtcDateTime<sk_kernel::clock::UtcZone>,
) -> Result<ProjectionReceipt, ProjectionError> {
    let projection = project(feature_dir, now)?;
    let output = derived_root.join(&projection.slug);
    let receipt = ProjectionReceipt {
        tracked_status: Some(write_view(
            &feature_dir.join(STATUS_FILENAME),
            &projection.status_bytes,
        )?),
        derived_status: Some(write_view(
            &output.join(STATUS_FILENAME),
            &crate::mutation_json::python_indented_json(&projection.status),
        )?),
        board: Some(write_view(
            &output.join(BOARD_SUMMARY_FILENAME),
            &crate::mutation_json::python_indented_json(&projection.board),
        )?),
        progress: Some(write_view(
            &output.join(PROGRESS_FILENAME),
            &crate::mutation_json::python_indented_json(&projection.progress),
        )?),
        lifecycle: Some(write_view(
            &output.join(LIFECYCLE_FILENAME),
            &crate::mutation_json::python_indented_json(&projection.lifecycle),
        )?),
    };
    Ok(receipt)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mission(root: &Path, slug: &str) -> PathBuf {
        let dir = root.join("kitty-specs").join(slug);
        std::fs::create_dir_all(dir.join("tasks")).expect("dirs");
        dir
    }

    fn write(path: &Path, text: &str) {
        std::fs::write(path, text).expect("write");
    }

    fn event_row(event_id: &str, wp_id: &str, from: &str, to: &str, at: &str) -> String {
        format!(
            "{{\"actor\": \"claude\", \"at\": \"{at}\", \"event_id\": \"{event_id}\", \"evidence\": null, \"execution_mode\": \"worktree\", \"force\": false, \"from_lane\": \"{from}\", \"mission_slug\": \"mission-p\", \"reason\": null, \"review_ref\": null, \"to_lane\": \"{to}\", \"wp_id\": \"{wp_id}\"}}\n"
        )
    }

    #[test]
    fn progress_weights_and_rounding_match_the_source_map() {
        let temp = tempfile::tempdir().expect("temp");
        let dir = mission(temp.path(), "mission-p");
        write(
            &dir.join("meta.json"),
            "{\n  \"mission_number\": \"007\",\n  \"mission_type\": \"software-dev\",\n  \"slug\": \"mission-p\"\n}\n",
        );
        write(
            &dir.join(EVENTS_STATUS),
            &[
                event_row(
                    "01KJ0000000000000000000A01",
                    "WP01",
                    "planned",
                    "claimed",
                    "2026-01-01T00:00:00+00:00",
                ),
                event_row(
                    "01KJ0000000000000000000A02",
                    "WP01",
                    "claimed",
                    "in_progress",
                    "2026-01-01T00:10:00+00:00",
                ),
                event_row(
                    "01KJ0000000000000000000A03",
                    "WP02",
                    "planned",
                    "claimed",
                    "2026-01-01T00:00:00+00:00",
                ),
                event_row(
                    "01KJ0000000000000000000A04",
                    "WP02",
                    "claimed",
                    "in_progress",
                    "2026-01-01T00:10:00+00:00",
                ),
                event_row(
                    "01KJ0000000000000000000A05",
                    "WP02",
                    "in_progress",
                    "for_review",
                    "2026-01-01T00:20:00+00:00",
                ),
                event_row(
                    "01KJ0000000000000000000A06",
                    "WP03",
                    "planned",
                    "claimed",
                    "2026-01-01T00:00:00+00:00",
                ),
                event_row(
                    "01KJ0000000000000000000A07",
                    "WP03",
                    "claimed",
                    "in_progress",
                    "2026-01-01T00:10:00+00:00",
                ),
                event_row(
                    "01KJ0000000000000000000A08",
                    "WP03",
                    "in_progress",
                    "for_review",
                    "2026-01-01T00:20:00+00:00",
                ),
                event_row(
                    "01KJ0000000000000000000A09",
                    "WP03",
                    "for_review",
                    "done",
                    "2026-01-01T00:30:00+00:00",
                ),
            ]
            .concat(),
        );
        let projection = project(&dir, fixed_now("2026-01-02T00:00:00+00:00")).expect("project");
        let progress = &projection.progress;
        // (0.3 + 0.6 + 1.0) / 3 * 100 = 63.3333...
        assert_eq!(progress["percentage"], json(63.3333));
        assert_eq!(progress["weighted_percentage"], json(63.3333));
        assert_eq!(progress["done_percentage"], json(33.3333));
        assert_eq!(progress["done_count"], 1);
        assert_eq!(progress["total_count"], 3);
        assert_eq!(progress["mission_number"], "7");
        assert_eq!(progress["mission_type"], "software-dev");
        assert_eq!(progress["progress_semantics"], "weighted_readiness");
        assert_eq!(progress["per_lane_counts"]["in_progress"], 1);
        assert_eq!(progress["per_lane_counts"]["for_review"], 1);
        assert_eq!(progress["per_lane_counts"]["done"], 1);
        assert_eq!(progress["per_wp"][0]["wp_id"], "WP01");
        assert_eq!(progress["per_wp"][0]["lane_weight"], json(0.3));
        assert_eq!(progress["per_wp"][0]["wp_weight"], json(1.0));
        assert_eq!(progress["per_wp"][2]["fractional_progress"], json(1.0));
    }

    #[test]
    fn lifecycle_classifies_activity_and_writes_all_five_files() {
        let temp = tempfile::tempdir().expect("temp");
        let dir = mission(temp.path(), "mission-p");
        write(
            &dir.join("meta.json"),
            "{\n  \"created_at\": \"2026-01-01T00:00:00+00:00\",\n  \"mission_id\": \"01KJ0000000000000000000MP\",\n  \"mission_number\": 7,\n  \"mission_type\": \"software-dev\",\n  \"slug\": \"mission-p\"\n}\n",
        );
        write(
            &dir.join(EVENTS_STATUS),
            &event_row(
                "01KJ0000000000000000000A01",
                "WP01",
                "planned",
                "in_progress",
                "2026-01-01T00:00:00+00:00",
            ),
        );
        let now = fixed_now("2026-01-01T12:00:00+00:00");
        let receipt = write_mission_projection(&dir, &temp.path().join(DERIVED_VIEWS_DIR), now)
            .expect("write");
        for view in [
            &receipt.tracked_status,
            &receipt.derived_status,
            &receipt.board,
            &receipt.progress,
            &receipt.lifecycle,
        ] {
            let view = view.as_ref().expect("view");
            assert!(view.path.is_file(), "{} missing", view.path.display());
        }
        let derived = temp.path().join(DERIVED_VIEWS_DIR).join("mission-p");
        assert_eq!(
            std::fs::read_to_string(derived.join(LIFECYCLE_FILENAME)).expect("lifecycle"),
            std::fs::read_to_string(&receipt.lifecycle.as_ref().expect("view").path).expect("read")
        );
        assert_eq!(
            std::fs::read_to_string(dir.join(STATUS_FILENAME)).expect("status"),
            std::fs::read_to_string(derived.join(STATUS_FILENAME)).expect("derived")
        );
        let lifecycle: Value = serde_json::from_str(
            &std::fs::read_to_string(derived.join(LIFECYCLE_FILENAME)).expect("read"),
        )
        .expect("json");
        assert_eq!(lifecycle["state"], "active");
        assert_eq!(lifecycle["surface_state"], "active");
        assert_eq!(lifecycle["age_days"], 0);
        assert_eq!(lifecycle["mission_number"], 7);
        assert_eq!(lifecycle["mission_id"], "01KJ0000000000000000000MP");
        assert_eq!(lifecycle["completion_pct"], json(30.0));
        assert_eq!(lifecycle["event_count"], 1);
        // A second run changes nothing.
        let again = write_mission_projection(&dir, &temp.path().join(DERIVED_VIEWS_DIR), now)
            .expect("again");
        assert!(again.is_noop());
    }

    #[test]
    fn completed_and_abandoned_bands_follow_the_source_thresholds() {
        let completed = classify_lifecycle(3, 0, 3, Some(2), false);
        assert_eq!(completed.state, "recently_completed");
        assert_eq!(completed.surface_state, Some("recently_completed"));
        let archived = classify_lifecycle(3, 0, 3, Some(4), false);
        assert_eq!(archived.state, "archived");
        assert_eq!(archived.surface_state, None);
        let stale = classify_lifecycle(3, 1, 0, Some(14), false);
        assert_eq!(stale.state, "stale");
        let abandoned = classify_lifecycle(3, 1, 0, Some(30), false);
        assert_eq!(abandoned.state, "abandoned");
        let reopened = classify_lifecycle(3, 0, 3, Some(1), true);
        assert_eq!(reopened.state, "reopened");
        assert_eq!(reopened.surface_state, Some("reopened"));
    }

    #[test]
    fn post_mission_events_are_filtered_and_sorted_and_reopen_wins() {
        let temp = tempfile::tempdir().expect("temp");
        let dir = mission(temp.path(), "mission-p");
        write(
            &dir.join("meta.json"),
            "{\n  \"mission_number\": 7,\n  \"slug\": \"mission-p\"\n}\n",
        );
        let rows = [
            event_row("01KJ0000000000000000000A01", "WP01", "planned", "done", "2026-01-01T00:00:00+00:00"),
            "{\"event_id\": \"01KJ0000000000000000000E02\", \"event_type\": \"MissionReopened\", \"payload\": {\"reopened_at\": \"2026-02-01T00:00:00+00:00\", \"reopened_by\": \"ana\", \"reason\": \"late fix\"}, \"timestamp\": \"2026-02-01T00:00:00+00:00\"}\n".to_string(),
            "{\"event_id\": \"01KJ0000000000000000000E01\", \"event_type\": \"FollowUpRecorded\", \"payload\": {\"recorded_at\": \"2026-02-02T00:00:00+00:00\", \"follow_up_type\": \"commit\", \"commit_sha\": \"abc123\"}, \"timestamp\": \"2026-02-02T00:00:00+00:00\"}\n".to_string(),
            "{\"event_id\": \"01KJ0000000000000000000E03\", \"event_type\": \"SomethingElse\", \"timestamp\": \"2026-02-03T00:00:00+00:00\"}\n".to_string(),
        ]
        .concat();
        write(&dir.join(EVENTS_STATUS), &rows);
        let projection = project(&dir, fixed_now("2026-02-03T00:00:00+00:00")).expect("project");
        let events = projection.lifecycle["post_mission_events"]
            .as_array()
            .expect("array");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["event_type"], "MissionReopened");
        assert_eq!(events[1]["event_type"], "FollowUpRecorded");
        assert_eq!(projection.lifecycle["state"], "reopened");
        assert_eq!(
            projection.lifecycle["last_follow_up_at"],
            "2026-02-02T00:00:00+00:00"
        );
    }

    #[test]
    fn a_merged_then_reopened_mission_is_reopened_only_after_the_marker() {
        let temp = tempfile::tempdir().expect("temp");
        let dir = mission(temp.path(), "mission-p");
        write(
            &dir.join("meta.json"),
            "{\n  \"merged_at\": \"2026-02-05T00:00:00+00:00\",\n  \"mission_number\": 7,\n  \"slug\": \"mission-p\"\n}\n",
        );
        let reopen = "{\"event_id\": \"01KJ0000000000000000000E02\", \"event_type\": \"MissionReopened\", \"payload\": {\"reopened_at\": \"2026-02-01T00:00:00+00:00\"}, \"timestamp\": \"2026-02-01T00:00:00+00:00\"}\n";
        write(&dir.join(EVENTS_STATUS), reopen);
        let projection = project(&dir, fixed_now("2026-02-06T00:00:00+00:00")).expect("project");
        // Re-open predates the merge marker: the merge wins, no active WP, no WPs.
        assert_ne!(projection.lifecycle["state"], "reopened");
    }

    #[test]
    fn a_corrupt_meta_json_fails_closed() {
        let temp = tempfile::tempdir().expect("temp");
        let dir = mission(temp.path(), "mission-p");
        write(&dir.join("meta.json"), "{\n  \"broken\": ");
        let error = project(&dir, fixed_now("2026-01-01T00:00:00+00:00")).expect_err("refusal");
        assert!(
            matches!(
                error,
                ProjectionError::Unreadable(_) | ProjectionError::Identity(_)
            ),
            "unexpected {error:?}"
        );
    }

    const EVENTS_STATUS: &str = "status.events.jsonl";

    fn fixed_now(text: &str) -> sk_kernel::clock::UtcDateTime<sk_kernel::clock::UtcZone> {
        sk_kernel::clock::parse_iso(text).expect("instant")
    }

    fn json(value: f64) -> Value {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}
