//! Pure composition of the CLI-facing `next` decision envelope.
//!
//! `plan_next` answers "which runtime step comes next for this cursor". The
//! upstream `spec-kitty next` command emits something wider: a decision
//! envelope that also carries the work-package board override, progress,
//! origin/tier, the mapped action and the guard diagnostics. This module is the
//! deterministic half of that composition, so the CLI adapter keeps every
//! filesystem read and write and this crate stays pure.
//!
//! What lives here:
//!
//! * [`DecisionEnvelope`] — the closed field set, its upstream JSON shape and
//!   the `kind="step"` prompt invariant (a step whose prompt does not resolve
//!   degrades to `blocked`, never to a step with a null `prompt_file`).
//! * [`finalized_board_step`] — the finalized-task-board override over
//!   already-read lane facts.
//! * [`wp_progress`] / [`weighted_percentage`] — the upstream lane-weighted
//!   progress object.
//! * [`state_alias`] — the mission-family state → command alias table.
//!
//! What does not: reading `tasks/`, the status log, `lanes.json`, the mission
//! metadata or any template. Callers pass those facts in. A decision envelope
//! is therefore reproducible from data alone.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

/// The five kinds the CLI decision contract exposes.
///
/// [`crate::DecisionKind`] models the *runtime* planner and has no `query`
/// member: a bare `next` call never advances, so query is a CLI-level envelope
/// kind, not a planner outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliDecisionKind {
    /// A bare, read-only call; state was not advanced.
    Query,
    /// A runnable step with a real prompt artifact.
    Step,
    /// Something must be decided before the run can continue.
    DecisionRequired,
    /// The run (or the board) cannot proceed without intervention.
    Blocked,
    /// Every step is complete.
    Terminal,
}

impl CliDecisionKind {
    /// Stable wire name used in the `kind` field.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Query => "query",
            Self::Step => "step",
            Self::DecisionRequired => "decision_required",
            Self::Blocked => "blocked",
            Self::Terminal => "terminal",
        }
    }

    /// The runtime planner kind this envelope kind corresponds to, when any.
    pub fn from_runtime(kind: crate::DecisionKind) -> Self {
        match kind {
            crate::DecisionKind::Step => Self::Step,
            crate::DecisionKind::DecisionRequired => Self::DecisionRequired,
            crate::DecisionKind::Blocked => Self::Blocked,
            crate::DecisionKind::Terminal => Self::Terminal,
        }
    }
}

impl std::fmt::Display for CliDecisionKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Where the mission type was resolved from (`Decision.origin`).
///
/// Upstream leaves `origin` as an empty object in query mode and fills the two
/// keys in advancing mode; an unresolvable tier is reported as `unknown`
/// rather than omitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Origin {
    /// Template tier that supplied `mission-runtime.yaml`.
    pub mission_tier: String,
    /// Directory the tier resolved to.
    pub mission_path: String,
}

impl Origin {
    /// The documented fallback for an unresolvable mission tier.
    pub fn unknown() -> Self {
        Self {
            mission_tier: "unknown".to_string(),
            mission_path: "unknown".to_string(),
        }
    }

    /// Build from a resolved `(tier, path)` pair.
    pub fn resolved(tier: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            mission_tier: tier.into(),
            mission_path: path.into(),
        }
    }
}

/// The lane-weighted progress object the CLI reports (`Decision.progress`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WpProgress {
    /// Every `WP*.md` file counted.
    pub total_wps: u64,
    /// Work packages whose lane is `done`.
    pub done_wps: u64,
    /// Work packages whose lane is `approved`.
    pub approved_wps: u64,
    /// Work packages in a non-blocked in-flight lane.
    pub in_progress_wps: u64,
    /// Work packages in `planned`.
    pub planned_wps: u64,
    /// Work packages awaiting review.
    pub for_review_wps: u64,
    /// Lane-weighted completion percentage, rounded to one decimal.
    pub weighted_percentage: f64,
}

impl WpProgress {
    /// The upstream JSON object (key set and rounding preserved).
    pub fn to_json(&self) -> Value {
        json!({
            "total_wps": self.total_wps,
            "done_wps": self.done_wps,
            "approved_wps": self.approved_wps,
            "in_progress_wps": self.in_progress_wps,
            "planned_wps": self.planned_wps,
            "for_review_wps": self.for_review_wps,
            "weighted_percentage": self.weighted_percentage,
        })
    }
}

/// The upstream default lane weights used by `compute_weighted_progress`.
pub const DEFAULT_LANE_WEIGHTS: [(&str, f64); 11] = [
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

/// The weight of one lane; an unknown lane contributes no forward progress.
pub fn lane_weight(lane: &str) -> f64 {
    DEFAULT_LANE_WEIGHTS
        .iter()
        .find(|(name, _)| *name == lane)
        .map(|(_, weight)| *weight)
        .unwrap_or(0.0)
}

/// The lane-weighted percentage over `lanes`, rounded to one decimal.
///
/// An empty iteration yields `0.0` (upstream's `weight_total > 0` guard).
/// Lanes are the reduced status lanes; a work package in a non-display lane
/// simply contributes zero.
pub fn weighted_percentage<'a>(lanes: impl IntoIterator<Item = &'a str>) -> f64 {
    let mut sum = 0.0;
    let mut total = 0.0;
    for lane in lanes {
        sum += lane_weight(lane);
        total += 1.0;
    }
    if total <= 0.0 {
        return 0.0;
    }
    round_half_even((sum / total) * 100.0, 1)
}

/// Round to `decimals` places, ties to even — the rule Python's `round` uses.
///
/// The upstream progress percentage is rounded with `round(percentage, 1)`, so
/// a tie such as exactly `0.25` becomes `0.2`, not `0.3`. Rust's `f64::round`
/// rounds ties away from zero, which would diverge on those boundaries.
fn round_half_even(value: f64, decimals: u32) -> f64 {
    let factor = 10f64.powi(i32::try_from(decimals).unwrap_or(0));
    let scaled = value * factor;
    if !scaled.is_finite() {
        return value;
    }
    let floor = scaled.floor();
    let tie = (scaled - floor - 0.5).abs() < 1e-9;
    let rounded = if tie {
        if (floor as i64) % 2 == 0 {
            floor
        } else {
            floor + 1.0
        }
    } else {
        scaled.round()
    };
    rounded / factor
}

/// The non-display lanes: `genesis` and `uninitialized` are not on the board.
pub fn is_display_lane(lane: &str) -> bool {
    !matches!(lane, "genesis" | "uninitialized")
}

/// Count the board from lane strings plus the weighted percentage.
///
/// Buckets follow `_compute_wp_progress` exactly: non-display lanes are counted
/// in `total_wps` but in no bucket; `done` and `approved` are their own buckets;
/// `for_review`/`in_review` are the review bucket; `claimed`/`in_progress` are
/// the in-flight bucket. A `blocked` work package is in an active lane but is
/// explicitly excluded from in-flight counting upstream, and `canceled` reaches
/// no bucket, so both contribute only to `total_wps` (and the weighted
/// percentage's denominator).
pub fn wp_progress<'a>(lanes: impl IntoIterator<Item = &'a str>) -> WpProgress {
    let mut progress = WpProgress {
        total_wps: 0,
        done_wps: 0,
        approved_wps: 0,
        in_progress_wps: 0,
        planned_wps: 0,
        for_review_wps: 0,
        weighted_percentage: 0.0,
    };
    let mut seen: Vec<&str> = Vec::new();
    for lane in lanes {
        progress.total_wps += 1;
        seen.push(lane);
        if !is_display_lane(lane) {
            continue;
        }
        match lane {
            "done" => progress.done_wps += 1,
            "approved" => progress.approved_wps += 1,
            "for_review" | "in_review" => progress.for_review_wps += 1,
            "claimed" | "in_progress" => progress.in_progress_wps += 1,
            "planned" => progress.planned_wps += 1,
            // `blocked` and `canceled` are in no bucket upstream.
            _ => {}
        }
    }
    progress.weighted_percentage = weighted_percentage(seen);
    progress
}

/// A side-effect-free preview of the work package `implement` would claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimablePreview {
    /// The dependency-ready `planned` work package, if one exists.
    pub wp_id: Option<String>,
    /// Why no work package is claimable (upstream vocabulary).
    pub selection_reason: Option<&'static str>,
}

/// No `tasks/` directory exists yet.
pub const NO_TASKS_DIR: &str = "no_tasks_dir";
/// No `WP*.md` files at all.
pub const NO_WP_FILES: &str = "no_wp_files";
/// Every `planned` work package is dependency-walled.
pub const DEPENDENCIES_NOT_SATISFIED: &str = "dependencies_not_satisfied";
/// An active (claimed/in-progress/for-review/in-review) work package exists.
pub const ALL_WPS_IN_PROGRESS: &str = "all_wps_in_progress";
/// A work package exists but has never been seeded out of `genesis`.
pub const NOT_FINALIZED: &str = "not_finalized";
/// Work packages exist but none is `planned`.
pub const NO_PLANNED_WPS: &str = "no_planned_wps";

/// Preview the work package `implement` would auto-claim.
///
/// Reproduces `runtime.next.discovery.preview_claimable_wp` over already-read
/// facts: the candidates in file order, their reduced lanes, and whether each
/// candidate's declared dependencies are satisfied. A `genesis` candidate is
/// never claimable and is reported distinctly from "nothing planned", so the
/// caller can tell "finalize tasks" apart from "all work packages advanced".
pub fn preview_claimable_wp(
    candidates: &[String],
    lanes: &BTreeMap<String, String>,
    dependency_ready: &BTreeMap<String, bool>,
) -> ClaimablePreview {
    if candidates.is_empty() {
        return ClaimablePreview {
            wp_id: None,
            selection_reason: Some(NO_WP_FILES),
        };
    }
    let mut has_dependency_blocked = false;
    let mut has_active = false;
    let mut has_genesis = false;
    for wp_id in candidates {
        let lane = lanes.get(wp_id).map(String::as_str).unwrap_or("genesis");
        if lane == "genesis" {
            has_genesis = true;
            continue;
        }
        if lane == "planned" {
            if !dependency_ready.get(wp_id).copied().unwrap_or(true) {
                has_dependency_blocked = true;
                continue;
            }
            return ClaimablePreview {
                wp_id: Some(wp_id.clone()),
                selection_reason: None,
            };
        }
        if matches!(lane, "claimed" | "in_progress" | "for_review" | "in_review") {
            has_active = true;
        }
    }
    let reason = if has_dependency_blocked {
        DEPENDENCIES_NOT_SATISFIED
    } else if has_active {
        ALL_WPS_IN_PROGRESS
    } else if has_genesis {
        NOT_FINALIZED
    } else {
        NO_PLANNED_WPS
    };
    ClaimablePreview {
        wp_id: None,
        selection_reason: Some(reason),
    }
}

/// Board facts the finalized-task-board override needs.
///
/// Every field is already derived from disk by the caller: the WP file count,
/// whether a `planned` WP is genuinely claimable (dependency-aware), and the
/// first WP id observed in each lane.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BoardFacts {
    /// Number of `WP*.md` files on the board.
    pub total_wps: u64,
    /// A dependency-ready `planned` work package exists.
    pub claimable_planned: bool,
    /// First WP in `claimed`.
    pub first_claimed: Option<String>,
    /// First WP in `in_progress`.
    pub first_in_progress: Option<String>,
    /// First WP in `for_review`.
    pub first_for_review: Option<String>,
    /// First WP in `in_review`.
    pub first_in_review: Option<String>,
    /// Work packages whose reduced lane is `done`.
    pub done_endings: u64,
    /// Work packages in an acceptable ending (`done` or `canceled`).
    pub acceptable_endings: u64,
}

/// The board's own answer for "what should happen next".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoardStep {
    /// A work package is (or can be made) claimable for implementation.
    Implement,
    /// A work package awaits review pickup.
    Review,
    /// Every work package reached an acceptable ending, but not all are done.
    Accept,
    /// Every work package is done.
    Done,
    /// The board cannot make progress; the payload is the sentinel reason
    /// (`review_in_progress` or `no_actionable_wp`).
    Blocked(&'static str),
}

impl BoardStep {
    /// The runtime step id the board maps onto (`implement`, `review`, ...).
    pub fn step_id(&self) -> &'static str {
        match self {
            Self::Implement => "implement",
            Self::Review => "review",
            Self::Accept => "accept",
            Self::Done => "done",
            Self::Blocked(reason) => reason,
        }
    }

    /// The upstream `blocked:` sentinel text, when this is a blocked board.
    pub fn blocked_reason(&self) -> Option<&'static str> {
        match self {
            Self::Blocked(reason) => Some(reason),
            _ => None,
        }
    }
}

/// Derive the finalized-task-board override step.
///
/// Returns `None` when the board is not finalized yet (no work packages), so
/// the caller keeps the DAG's own progression. A dependency-walled `planned`
/// WP does not pin the board to `implement`: a sibling awaiting review wins,
/// which is the upstream `#5669` rule.
pub fn finalized_board_step(facts: &BoardFacts) -> Option<BoardStep> {
    if facts.total_wps == 0 {
        return None;
    }
    if facts.claimable_planned {
        return Some(BoardStep::Implement);
    }
    if facts.first_claimed.is_some() {
        return Some(BoardStep::Implement);
    }
    if facts.first_in_progress.is_some() {
        return Some(BoardStep::Implement);
    }
    if facts.first_for_review.is_some() {
        return Some(BoardStep::Review);
    }
    if facts.first_in_review.is_some() {
        return Some(BoardStep::Blocked("review_in_progress"));
    }
    if facts.acceptable_endings == facts.total_wps {
        return Some(if facts.done_endings == facts.total_wps {
            BoardStep::Done
        } else {
            BoardStep::Accept
        });
    }
    Some(BoardStep::Blocked("no_actionable_wp"))
}

/// The mission-family state → command alias table.
///
/// Upstream resolves `<state>.md` first and only then falls back to these
/// aliases, so a mission that ships its own template keeps it.
pub const STATE_ALIASES: [(&str, &str); 12] = [
    ("discovery", "research"),
    ("scoping", "specify"),
    ("methodology", "plan"),
    ("tasks_outline", "tasks-outline"),
    ("tasks_packages", "tasks-packages"),
    ("tasks_finalize", "tasks-finalize"),
    ("gathering", "implement"),
    ("synthesis", "review"),
    ("output", "accept"),
    ("goals", "specify"),
    ("structure", "plan"),
    ("draft", "plan"),
];

/// The alias for a mission state, if the table defines one.
pub fn state_alias(state: &str) -> Option<&'static str> {
    STATE_ALIASES
        .iter()
        .find(|(name, _)| *name == state)
        .map(|(_, alias)| *alias)
}

/// A fully-resolved CLI decision, before it is rendered.
///
/// Field set and JSON shape mirror upstream `Decision.to_dict()`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DecisionEnvelope {
    /// Envelope kind.
    pub kind: Option<CliDecisionKind>,
    /// Agent identity carried from the invocation.
    pub agent: Option<String>,
    /// Canonical mission slug.
    pub mission_slug: String,
    /// Display mission number (empty string when the mission has none).
    pub mission_number: String,
    /// Mission type (neutral `""` for a typeless legacy mission).
    pub mission_type: String,
    /// Current mission state (a step id, `not_started`, `blocked`, `done`).
    pub mission_state: String,
    /// Wall-clock decision stamp.
    pub timestamp: String,
    /// Mapped action (`research`, `specify`, `implement`, ...).
    pub action: Option<String>,
    /// Work package the action applies to.
    pub wp_id: Option<String>,
    /// Resolved lane worktree for a WP action.
    pub workspace_path: Option<String>,
    /// Real, on-disk prompt artifact for a step action.
    pub prompt_file: Option<String>,
    /// Why the decision is what it is.
    pub reason: Option<String>,
    /// Guard failures that hold the run in place.
    pub guard_failures: Vec<String>,
    /// Artifact tag → path the guard actually read.
    pub guard_failure_paths: BTreeMap<String, String>,
    /// Lane-weighted board progress.
    pub progress: Option<WpProgress>,
    /// Mission template tier/path (advancing mode only).
    pub origin: Option<Origin>,
    /// Durable run id, when a run exists.
    pub run_id: Option<String>,
    /// Runtime step id.
    pub step_id: Option<String>,
    /// Pending decision id.
    pub decision_id: Option<String>,
    /// Input key for an input-keyed decision.
    pub input_key: Option<String>,
    /// Question text for a decision-required outcome.
    pub question: Option<String>,
    /// Selectable options for a decision-required outcome.
    pub options: Option<Vec<String>>,
    /// True only for a bare query call.
    pub is_query: bool,
    /// Step a query call would issue.
    pub preview_step: Option<String>,
    /// Typed refusal code carried by a blocked decision.
    pub error_code: Option<String>,
}

impl DecisionEnvelope {
    /// A query-mode envelope for `mission_slug`.
    pub fn query(mission_slug: &str, mission_type: &str, timestamp: &str) -> Self {
        Self {
            kind: Some(CliDecisionKind::Query),
            mission_slug: mission_slug.to_string(),
            mission_type: mission_type.to_string(),
            timestamp: timestamp.to_string(),
            is_query: true,
            ..Self::default()
        }
    }

    /// A blocked envelope with a reason.
    pub fn blocked(
        mission_slug: &str,
        mission_type: &str,
        mission_state: &str,
        timestamp: &str,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            kind: Some(CliDecisionKind::Blocked),
            mission_slug: mission_slug.to_string(),
            mission_type: mission_type.to_string(),
            mission_state: mission_state.to_string(),
            timestamp: timestamp.to_string(),
            reason: Some(reason.into()),
            ..Self::default()
        }
    }

    /// The envelope kind, or `query` for an unset envelope.
    pub fn kind(&self) -> CliDecisionKind {
        self.kind.unwrap_or(CliDecisionKind::Query)
    }

    /// Enforce the `kind="step"` prompt contract.
    ///
    /// A step whose prompt artifact does not resolve becomes a blocked
    /// decision carrying the caller's reason. This is the upstream
    /// `step_or_blocked` invariant, expressed once, without I/O: the caller
    /// supplies whether the prompt exists.
    pub fn materialize(mut self, prompt_exists: bool, reason: impl Into<String>) -> Self {
        if self.kind() != CliDecisionKind::Step {
            return self;
        }
        let resolves = self
            .prompt_file
            .as_deref()
            .is_some_and(|path| !path.is_empty())
            && prompt_exists;
        if resolves {
            return self;
        }
        self.kind = Some(CliDecisionKind::Blocked);
        self.reason = Some(reason.into());
        self.prompt_file = None;
        self
    }

    /// The upstream JSON object.
    pub fn to_json(&self) -> Value {
        let mut map = Map::new();
        map.insert(
            "kind".into(),
            Value::String(self.kind().as_str().to_string()),
        );
        map.insert("agent".into(), option_string(&self.agent));
        map.insert(
            "mission_slug".into(),
            Value::String(self.mission_slug.clone()),
        );
        map.insert(
            "mission_number".into(),
            Value::String(self.mission_number.clone()),
        );
        map.insert(
            "mission_type".into(),
            Value::String(self.mission_type.clone()),
        );
        map.insert("mission".into(), Value::String(self.mission_type.clone()));
        map.insert(
            "mission_state".into(),
            Value::String(self.mission_state.clone()),
        );
        map.insert("timestamp".into(), Value::String(self.timestamp.clone()));
        map.insert("action".into(), option_string(&self.action));
        map.insert("wp_id".into(), option_string(&self.wp_id));
        map.insert("workspace_path".into(), option_string(&self.workspace_path));
        map.insert("prompt_file".into(), option_string(&self.prompt_file));
        map.insert("reason".into(), option_string(&self.reason));
        map.insert("guard_failures".into(), json!(self.guard_failures));
        map.insert(
            "guard_failure_paths".into(),
            Value::Object(
                self.guard_failure_paths
                    .iter()
                    .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                    .collect(),
            ),
        );
        map.insert(
            "progress".into(),
            self.progress
                .as_ref()
                .map(WpProgress::to_json)
                .unwrap_or(Value::Null),
        );
        map.insert(
            "origin".into(),
            match &self.origin {
                Some(origin) => json!({
                    "mission_tier": origin.mission_tier,
                    "mission_path": origin.mission_path,
                }),
                None => Value::Object(Map::new()),
            },
        );
        map.insert("run_id".into(), option_string(&self.run_id));
        map.insert("step_id".into(), option_string(&self.step_id));
        map.insert("decision_id".into(), option_string(&self.decision_id));
        map.insert("input_key".into(), option_string(&self.input_key));
        map.insert("question".into(), option_string(&self.question));
        map.insert(
            "options".into(),
            match &self.options {
                Some(options) => json!(options),
                None => Value::Null,
            },
        );
        map.insert("is_query".into(), Value::Bool(self.is_query));
        map.insert("preview_step".into(), option_string(&self.preview_step));
        if let Some(code) = &self.error_code {
            map.insert("error_code".into(), Value::String(code.clone()));
        }
        Value::Object(map)
    }
}

fn option_string(value: &Option<String>) -> Value {
    value
        .as_ref()
        .map(|value| Value::String(value.clone()))
        .unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_without_a_resolvable_prompt_is_blocked() {
        let envelope = DecisionEnvelope {
            kind: Some(CliDecisionKind::Step),
            mission_slug: "demo".into(),
            mission_type: "software-dev".into(),
            mission_state: "plan".into(),
            timestamp: "t".into(),
            action: Some("plan".into()),
            prompt_file: Some("/missing/plan.md".into()),
            ..DecisionEnvelope::default()
        };
        let blocked = envelope.materialize(false, "prompt_file_not_resolvable");
        assert_eq!(blocked.kind(), CliDecisionKind::Blocked);
        assert_eq!(
            blocked.reason.as_deref(),
            Some("prompt_file_not_resolvable")
        );
        assert!(blocked.prompt_file.is_none());
        let value = blocked.to_json();
        assert_eq!(value["kind"], json!("blocked"));
        assert_eq!(value["prompt_file"], Value::Null);
        assert_eq!(value["options"], Value::Null);
        assert_eq!(value["origin"], json!({}));
        assert_eq!(value["mission_number"], json!(""));
    }

    #[test]
    fn a_step_with_a_resolvable_prompt_keeps_its_kind() {
        let envelope = DecisionEnvelope {
            kind: Some(CliDecisionKind::Step),
            prompt_file: Some("/tmp/plan.md".into()),
            ..DecisionEnvelope::default()
        };
        assert_eq!(
            envelope.materialize(true, "x").kind(),
            CliDecisionKind::Step
        );
    }

    #[test]
    fn claimable_preview_reports_the_upstream_selection_reasons() {
        let planned =
            |ids: &[&str]| -> Vec<String> { ids.iter().map(|id| id.to_string()).collect() };
        let lanes = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
            pairs
                .iter()
                .map(|(id, lane)| (id.to_string(), lane.to_string()))
                .collect()
        };
        let ready = |pairs: &[(&str, bool)]| -> BTreeMap<String, bool> {
            pairs.iter().map(|(id, ok)| (id.to_string(), *ok)).collect()
        };

        let candidates = planned(&["WP01"]);
        assert_eq!(
            preview_claimable_wp(&candidates, &lanes(&[("WP01", "genesis")]), &ready(&[]))
                .selection_reason,
            Some(NOT_FINALIZED)
        );
        assert_eq!(
            preview_claimable_wp(&candidates, &lanes(&[("WP01", "in_progress")]), &ready(&[]))
                .selection_reason,
            Some(ALL_WPS_IN_PROGRESS)
        );
        assert_eq!(
            preview_claimable_wp(
                &candidates,
                &lanes(&[("WP01", "planned")]),
                &ready(&[("WP01", false)])
            )
            .selection_reason,
            Some(DEPENDENCIES_NOT_SATISFIED)
        );
        assert_eq!(
            preview_claimable_wp(&candidates, &lanes(&[("WP01", "done")]), &ready(&[]))
                .selection_reason,
            Some(NO_PLANNED_WPS)
        );
        assert_eq!(
            preview_claimable_wp(&[], &lanes(&[]), &ready(&[])).selection_reason,
            Some(NO_WP_FILES)
        );
        let preview = preview_claimable_wp(
            &candidates,
            &lanes(&[("WP01", "planned")]),
            &ready(&[("WP01", true)]),
        );
        assert_eq!(preview.wp_id.as_deref(), Some("WP01"));
        assert_eq!(preview.selection_reason, None);
    }

    #[test]
    fn board_override_prefers_the_claimable_work_package_over_a_walled_one() {
        let blocked_planned = BoardFacts {
            total_wps: 2,
            claimable_planned: false,
            first_for_review: Some("WP02".into()),
            ..BoardFacts::default()
        };
        assert_eq!(
            finalized_board_step(&blocked_planned),
            Some(BoardStep::Review)
        );

        let claimable = BoardFacts {
            total_wps: 2,
            claimable_planned: true,
            first_for_review: Some("WP02".into()),
            ..BoardFacts::default()
        };
        assert_eq!(finalized_board_step(&claimable), Some(BoardStep::Implement));
    }

    #[test]
    fn board_override_is_absent_without_work_packages() {
        assert_eq!(finalized_board_step(&BoardFacts::default()), None);
    }

    #[test]
    fn board_override_reports_every_terminal_shape() {
        let review = BoardFacts {
            total_wps: 1,
            first_in_review: Some("WP01".into()),
            ..BoardFacts::default()
        };
        assert_eq!(
            finalized_board_step(&review),
            Some(BoardStep::Blocked("review_in_progress"))
        );

        let all_done = BoardFacts {
            total_wps: 2,
            done_endings: 2,
            acceptable_endings: 2,
            ..BoardFacts::default()
        };
        assert_eq!(finalized_board_step(&all_done), Some(BoardStep::Done));

        let canceled = BoardFacts {
            total_wps: 2,
            done_endings: 1,
            acceptable_endings: 2,
            ..BoardFacts::default()
        };
        assert_eq!(finalized_board_step(&canceled), Some(BoardStep::Accept));

        let stuck = BoardFacts {
            total_wps: 2,
            done_endings: 0,
            acceptable_endings: 1,
            ..BoardFacts::default()
        };
        assert_eq!(
            finalized_board_step(&stuck),
            Some(BoardStep::Blocked("no_actionable_wp"))
        );
    }

    #[test]
    fn weighted_progress_matches_the_upstream_lane_weights() {
        assert_eq!(weighted_percentage(["in_progress", "planned"]), 15.0);
        assert_eq!(weighted_percentage(["done", "done"]), 100.0);
        assert_eq!(weighted_percentage(std::iter::empty()), 0.0);
        assert_eq!(weighted_percentage(["genesis"]), 0.0);
        assert_eq!(weighted_percentage(["for_review", "approved"]), 70.0);
        // A single claimed work package among twenty is exactly 0.25%, which
        // Python's `round(..., 1)` ties to 0.2.
        let mut twenty = vec!["planned"; 19];
        twenty.push("claimed");
        assert_eq!(weighted_percentage(twenty), 0.2);
    }

    #[test]
    fn progress_counts_skip_non_display_lanes_and_bucket_review_lanes() {
        let lanes = [
            "genesis",
            "planned",
            "in_progress",
            "for_review",
            "approved",
            "done",
            "blocked",
            "canceled",
        ];
        let progress = wp_progress(lanes);
        assert_eq!(progress.total_wps, 8);
        assert_eq!(progress.done_wps, 1);
        assert_eq!(progress.approved_wps, 1);
        assert_eq!(progress.in_progress_wps, 1);
        assert_eq!(progress.for_review_wps, 1);
        assert_eq!(progress.planned_wps, 1);
        assert_eq!(progress.weighted_percentage, weighted_percentage(lanes));
    }

    #[test]
    fn state_aliases_cover_the_documented_mission_families() {
        assert_eq!(state_alias("discovery"), Some("research"));
        assert_eq!(state_alias("gathering"), Some("implement"));
        assert_eq!(state_alias("output"), Some("accept"));
        assert_eq!(state_alias("specify"), None);
    }
}
