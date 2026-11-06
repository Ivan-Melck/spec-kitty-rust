//! Canonical status routes: `agent status validate` and
//! `agent status materialize`.
//!
//! Both routes are read/compute-first. `materialize` computes the snapshot with
//! the canonical reducer and writes the exact upstream `status.json` bytes
//! under the canonical Mission status lock; `validate` never writes.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sk_status::{Lane, Snapshot, TransitionError};

use super::support::{
    CompatError, MissionView, canonical_lanes, emit_json, events_path, project_root,
    resolve_mission_record,
};
use sk_mission::MissionRecord;

/// Upstream requires an explicit `--mission` for status routes.
pub fn require_mission(root: &Path, handle: Option<&str>) -> Result<MissionRecord, CompatError> {
    match handle.map(str::trim).filter(|value| !value.is_empty()) {
        Some(handle) => resolve_mission_record(root, Some(handle)),
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
        // Baseline `agent status materialize` takes `feature_status_lock` with
        // its unbounded default: a legitimate holder is waited out.
        sk_kernel::LockWait::Unbounded,
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
pub fn lanes(mission: &impl MissionView) -> Result<BTreeMap<String, String>, CompatError> {
    canonical_lanes(mission)
}

/// Diagnose a missing project root with the upstream message.
pub fn root_or_error() -> Result<PathBuf, CompatError> {
    project_root()
}

// ===========================================================================
// Mutation route: `agent status emit` + the canonical lifecycle read
// ===========================================================================

use clap::Args;

/// `agent status emit` options (the upstream Typer surface).
#[derive(Debug, Args)]
pub struct EmitArgs {
    /// Work package ID (positional).
    #[arg(value_name = "WP_ID")]
    pub wp_id: Option<String>,
    /// Work package ID (alternative to the positional ID).
    #[arg(long)]
    pub wp: Option<String>,
    /// Target lane.
    #[arg(long, value_name = "LANE")]
    pub to: String,
    /// Who is making this transition.
    #[arg(long)]
    pub actor: String,
    /// Mission slug (required in multi-mission repos).
    #[arg(long)]
    pub mission: Option<String>,
    /// Force the transition, bypassing guards.
    #[arg(long)]
    pub force: bool,
    /// Reason for a forced transition.
    #[arg(long)]
    pub reason: Option<String>,
    /// JSON done evidence.
    #[arg(long = "evidence-json")]
    pub evidence_json: Option<String>,
    /// Review feedback reference.
    #[arg(long = "review-ref")]
    pub review_ref: Option<String>,
    /// Structured JSON review outcome for transitions from `in_review`.
    #[arg(long = "review-result-json")]
    pub review_result_json: Option<String>,
    /// Workspace context identifier for `claimed -> in_progress`.
    #[arg(long = "workspace-context")]
    pub workspace_context: Option<String>,
    /// Whether required subtasks are complete for `in_progress -> for_review`.
    #[arg(long = "subtasks-complete")]
    pub subtasks_complete: Option<bool>,
    /// Whether implementation evidence exists for `in_progress -> for_review`.
    #[arg(long = "implementation-evidence-present")]
    pub implementation_evidence_present: Option<bool>,
    /// Execution mode (`worktree` or `direct_repo`).
    #[arg(long = "execution-mode", default_value = "worktree")]
    pub execution_mode: String,
    /// Emit JSON.
    #[arg(long)]
    pub json: bool,
}

/// The four compact `--agent` segments (`tool`, `model`, `profile`, `role`).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct ParsedAgent {
    pub tool: String,
    pub model: Option<String>,
    pub profile: Option<String>,
    pub role: Option<String>,
}

/// The compact `--agent` boundary parse.
///
/// Faithful to `status.models.parse_agent_boundary_string`: trailing segments
/// default to `None`, interior whitespace is stripped, an empty tool or more
/// than four segments is a refusal.
pub(super) fn parse_agent_boundary_string(raw: &str) -> Result<ParsedAgent, String> {
    let segments: Vec<&str> = raw.split(':').collect();
    if segments.len() > 4 {
        return Err(format!(
            "Too many segments in --agent value: {raw:?} (expected at most 4, got {})",
            segments.len()
        ));
    }
    let mut padded: Vec<String> = segments.iter().map(|seg| seg.trim().to_owned()).collect();
    while padded.len() < 4 {
        padded.push(String::new());
    }
    let tool = padded[0].clone();
    if tool.is_empty() {
        return Err(format!("Empty agent tool in --agent value: {raw:?}"));
    }
    let nonblank = |value: &String| (!value.is_empty()).then(|| value.clone());
    Ok(ParsedAgent {
        tool,
        model: nonblank(&padded[1]),
        profile: nonblank(&padded[2]),
        role: nonblank(&padded[3]),
    })
}

/// The actor role for a `from_lane -> to_lane` hop (`status.actor.role_for_hop`).
pub(super) fn role_for_hop(from_lane: Option<&str>, to_lane: &str) -> &'static str {
    let target = sk_status::resolve_lane_alias(to_lane);
    let reviewer = matches!(target.as_str(), "in_review" | "approved" | "done")
        || from_lane.map(sk_status::resolve_lane_alias).as_deref() == Some("in_review");
    if reviewer { "reviewer" } else { "implementer" }
}

/// `actor_from_option`: the lenient structured actor of a free-text identity.
pub(super) fn actor_from_option(
    actor: &str,
    role: &str,
    model: Option<&str>,
    profile: Option<&str>,
) -> sk_status::mutation::ActorValue {
    // A caller-supplied reconstruction seed is refused before anything is read.
    let seed_shaped = actor.starts_with(sk_status::MIGRATION_ACTOR_PREFIX);
    if !seed_shaped
        && actor.split(':').count() <= 4
        && let Ok(parsed) = parse_agent_boundary_string(actor)
    {
        return sk_status::mutation::ActorValue::Structured {
            role: Some(role.to_owned()),
            profile: profile.map(str::to_owned).or(parsed.profile),
            tool: Some(parsed.tool),
            model: model.map(str::to_owned).or(parsed.model),
        };
    }
    sk_status::mutation::ActorValue::Structured {
        role: Some(role.to_owned()),
        profile: profile.map(str::to_owned),
        tool: Some(actor.to_owned()),
        model: model.map(str::to_owned),
    }
}

/// Refuse a caller-supplied reconstruction-seed identity (`#6754`).
pub(super) fn refuse_seed_shaped_identity(value: &str, option: &str) -> Result<(), CompatError> {
    if value.starts_with(sk_status::MIGRATION_ACTOR_PREFIX) {
        return Err(CompatError::domain(format!(
            "{option} must not start with \"{}\": reconstruction seeds are written in-process by migrations only",
            sk_status::MIGRATION_ACTOR_PREFIX
        )));
    }
    Ok(())
}

/// `normalize_wp_id`: strip + upper-case, require `WP<digits>`.
pub(super) fn normalize_wp_id(value: &str) -> Result<String, CompatError> {
    let normalized = value.trim().to_ascii_uppercase();
    let digits = normalized.strip_prefix("WP").unwrap_or("");
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(CompatError::domain(format!(
            "Invalid work package ID '{value}': expected WP<digits> (e.g. WP01)"
        )));
    }
    Ok(normalized)
}

/// Parse `--review-result-json` through the canonical shape checks.
pub(super) fn parse_review_result_json(raw: &str) -> Result<Value, CompatError> {
    let parsed: Value = serde_json::from_str(raw).map_err(|error| {
        CompatError::domain(format!("Invalid JSON in --review-result-json: {error}"))
    })?;
    let Value::Object(object) = &parsed else {
        return Err(CompatError::domain(
            "--review-result-json must decode to a JSON object",
        ));
    };
    let nonblank = |key: &str| {
        object
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    if nonblank("reviewer").is_none()
        || nonblank("verdict").is_none()
        || nonblank("reference").is_none()
    {
        return Err(CompatError::domain(
            "--review-result-json requires non-empty reviewer, verdict, and reference strings",
        ));
    }
    match nonblank("verdict") {
        Some("approved") | Some("changes_requested") => {}
        _ => {
            return Err(CompatError::domain(
                "--review-result-json verdict must be 'approved' or 'changes_requested'",
            ));
        }
    }
    if let Some(feedback) = object.get("feedback_path")
        && !feedback.is_null()
        && !feedback.is_string()
    {
        return Err(CompatError::domain(
            "--review-result-json feedback_path must be a string",
        ));
    }
    Ok(parsed)
}

/// Refuse a coordination-routed mission on any primary status write.
pub(super) fn refuse_coordination(
    mission: &impl MissionView,
    action: &str,
) -> Result<(), CompatError> {
    if mission
        .meta_field("coordination_branch")
        .and_then(Value::as_str)
        .is_some_and(|branch| !branch.trim().is_empty())
    {
        return Err(CompatError::coded(
            "COORDINATION_SURFACE_UNAVAILABLE",
            format!(
                "coordination-routed {action} requires the canonical coordination write surface; refusing to write primary status silently instead"
            ),
        ));
    }
    Ok(())
}

/// Probe a WP's lane-branch HEAD for the `policy_metadata["lane_head"]` stamp.
///
/// Best-effort like `status.lane_head.probe_lane_head`: every absence, parse
/// failure and Git error resolves to "no stamp", never a refused transition.
pub(super) fn probe_lane_head(
    root: &Path,
    mission: &impl MissionView,
    wp_id: &str,
) -> Option<String> {
    let manifest_path = mission.spec_dir().join("lanes.json");
    let bytes = sk_kernel::read_bounded(&manifest_path, 4 * 1024 * 1024).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    let target_branch = value
        .get("target_branch")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let lane = value.get("lanes")?.as_array()?.iter().find(|lane| {
        lane.get("wp_ids")
            .and_then(Value::as_array)
            .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(wp_id)))
    })?;
    let lane_id = lane.get("lane_id").and_then(Value::as_str)?;
    let slug = value.get("mission_slug").and_then(Value::as_str)?;
    let branch = sk_workspace::lane_branch_name(slug, lane_id, target_branch).ok()?;
    let repository = sk_git::GitRepository::open(root).ok()?;
    let branch = sk_git::BranchName::parse(&branch).ok()?;
    repository
        .read_branch_tip(&branch)
        .ok()
        .flatten()
        .map(|oid| oid.to_string())
}

// ---------------------------------------------------------------------------
// Git-derived review-ward gates
// ---------------------------------------------------------------------------

/// Lanes from which a move counts as review-ward (`_REVIEW_WARD_LANES`).
fn review_ward(lane: &str) -> bool {
    matches!(
        sk_status::resolve_lane_alias(lane).as_str(),
        "for_review" | "in_review" | "approved" | "done"
    )
}

/// The status-only path prefixes that never count as implementation work.
///
/// The pinned baseline's one exclusion list (#6715 `status_only_excluded_prefixes`).
fn status_only_excluded_prefixes(slug: &str) -> Vec<String> {
    [
        format!("kitty-specs/{slug}/status."),
        format!("kitty-specs/{slug}/issue-matrix."),
        format!("kitty-specs/{slug}/meta.json"),
        format!("kitty-specs/{slug}/tasks/"),
        format!("kitty-specs/{slug}/lanes.json"),
        format!("kitty-specs/{slug}/acceptance-matrix.json"),
        format!("kitty-specs/{slug}/analysis-report.md"),
        format!("kitty-specs/{slug}/decisions/"),
        format!("kitty-specs/{slug}/decisions.events.jsonl"),
        format!("kitty-specs/{slug}/traces/"),
        ".kittify/".to_owned(),
    ]
    .into_iter()
    .collect()
}

fn lane_manifest(
    mission: &impl MissionView,
) -> Result<Option<sk_workspace::LanesManifest>, CompatError> {
    let store = sk_workspace::LanesStore::new(mission.spec_dir());
    store.read().map_err(|error| {
        CompatError::domain(format!(
            "could not read {}: {error}",
            mission.spec_dir().join("lanes.json").display()
        ))
    })
}

fn lane_for_wp<'a>(
    manifest: &'a sk_workspace::LanesManifest,
    wp_id: &str,
) -> Option<&'a sk_workspace::ExecutionLane> {
    let wp: sk_kernel::WpId = wp_id.parse().ok()?;
    manifest.lane_for_wp(&wp)
}

/// The ref a code lane was parented on. Coordination-routed missions are
/// refused before this gate, so the stored mission branch is the base.
fn lane_base_ref(manifest: &sk_workspace::LanesManifest) -> String {
    if manifest.mission_branch.trim().is_empty() {
        manifest.target_branch.clone()
    } else {
        manifest.mission_branch.clone()
    }
}

fn for_review_refusal(wp_id: &str, reason: String, lane_id: Option<&str>) -> CompatError {
    let mut error = CompatError::coded("FOR_REVIEW_GATE", reason).with("wp_id", json!(wp_id));
    if let Some(lane_id) = lane_id {
        error = error.with("lane_id", json!(lane_id));
    }
    error
}

/// The shared `in_progress -> for_review` commit gate (FR-011).
///
/// Mirrors `lanes/for_review_gate.evaluate_for_review_gate`: a no-op when the
/// alias-resolved target is not `for_review`, when `--force` bypasses it, when
/// there is no `lanes.json`, or when the WP is in no lane. A code lane needs a
/// real commit beyond its base in its lane worktree; a repository-root lane is
/// decided by its recorded claim base and a qualifying (non-bookkeeping) commit
/// since then. A missing lane worktree, unreadable commit or missing claim base
/// refuses; it never passes vacuously.
pub(super) fn enforce_for_review_commit_gate(
    root: &Path,
    mission: &impl MissionView,
    wp_id: &str,
    to_lane: &str,
    force: bool,
) -> Result<(), CompatError> {
    if sk_status::resolve_lane_alias(to_lane) != "for_review" || force {
        return Ok(());
    }
    let Some(manifest) = lane_manifest(mission)? else {
        // Legacy / non-lane mission: nothing to check.
        return Ok(());
    };
    let Some(lane) = lane_for_wp(&manifest, wp_id) else {
        return Ok(());
    };
    let slug = mission.slug_str();
    let repository = sk_git::GitRepository::open(root).map_err(|error| {
        CompatError::domain(format!(
            "the for_review commit gate needs a readable Git checkout at {}: {error}",
            root.display()
        ))
    })?;

    if lane.is_planning() {
        return planning_lane_commit_gate(&repository, wp_id, slug);
    }

    let worktree = sk_workspace::worktree_path(root, slug, &lane.lane_id).map_err(|error| {
        CompatError::domain(format!(
            "could not resolve the lane worktree for {}: {error}",
            lane.lane_id
        ))
    })?;
    let lane_branch = sk_workspace::lane_branch_name(slug, &lane.lane_id, &manifest.target_branch)
        .map_err(|error| {
            CompatError::domain(format!(
                "could not resolve the lane branch for {}: {error}",
                lane.lane_id
            ))
        })?;
    let base_ref = lane_base_ref(&manifest);
    let beyond = worktree
        .is_dir()
        .then(|| {
            let lane_repository = sk_git::GitRepository::open(&worktree).ok()?;
            let head = lane_repository.head_commit().ok()??;
            let head = sk_git::Oid::parse(&head).ok()?;
            let base = resolve_ref_commit(&lane_repository, &base_ref).ok()?;
            has_commit_beyond(&lane_repository, &head, &base).ok()
        })
        .flatten()
        .unwrap_or(false);
    if beyond {
        return Ok(());
    }
    Err(for_review_refusal(
        wp_id,
        format!(
            "{wp_id} cannot move to for_review: no implementation commit on lane {} ({lane_branch}) beyond {base_ref}. Commit the work in the lane worktree first, or pass --force if there is genuinely nothing to commit.",
            lane.lane_id
        ),
        Some(&lane.lane_id),
    ))
}

fn planning_lane_commit_gate(
    repository: &sk_git::GitRepository,
    wp_id: &str,
    slug: &str,
) -> Result<(), CompatError> {
    let claim_ref = sk_git::RefName::parse(&format!("refs/spec-kitty/wp-base/{slug}/{wp_id}"))
        .map_err(|error| CompatError::domain(format!("invalid claim-base ref: {error}")))?;
    let claim_base = repository.read_ref(&claim_ref).ok().flatten();
    let qualifying = claim_base.as_ref().is_some_and(|base| {
        (|| -> Option<bool> {
            let head = repository.head_commit().ok()??;
            let head = sk_git::Oid::parse(&head).ok()?;
            let paths = changed_paths_between(repository, base, &head, "").ok()?;
            let exclusions = status_only_excluded_prefixes(slug);
            Some(paths.iter().any(|path| {
                !exclusions
                    .iter()
                    .any(|prefix| path.starts_with(prefix.as_str()))
            }))
        })()
        .unwrap_or(false)
    });
    if qualifying {
        return Ok(());
    }
    let base = claim_base
        .map(|oid| oid.to_string())
        .unwrap_or_else(|| "<no recorded claim base>".to_owned());
    Err(for_review_refusal(
        wp_id,
        format!(
            "{wp_id} cannot move to for_review: no implementation commit since claim in the repository root checkout beyond {base}. Commit the work first, or pass --force if there is genuinely nothing to commit."
        ),
        None,
    ))
}

/// Resolve a branch name or fully-qualified ref to its commit.
fn resolve_ref_commit(
    repository: &sk_git::GitRepository,
    name: &str,
) -> Result<sk_git::Oid, String> {
    if name.starts_with("refs/") {
        let reference = sk_git::RefName::parse(name)
            .map_err(|error| format!("invalid ref {name:?}: {error}"))?;
        return repository
            .resolve_commit(&sk_git::StartPoint::Ref(reference))
            .map_err(|error| format!("could not resolve {name}: {error}"));
    }
    let branch = sk_git::BranchName::parse(name)
        .map_err(|error| format!("invalid branch {name:?}: {error}"))?;
    repository
        .read_branch_tip(&branch)
        .map_err(|error| format!("could not read {name}: {error}"))?
        .ok_or_else(|| format!("branch {name} does not exist"))
}

/// True when `head` carries at least one commit `base` does not.
fn has_commit_beyond(
    repository: &sk_git::GitRepository,
    head: &sk_git::Oid,
    base: &sk_git::Oid,
) -> Result<bool, sk_git::GitError> {
    if head == base {
        return Ok(false);
    }
    // `git rev-list base..HEAD` is non-empty unless HEAD is an ancestor of base
    // (a branch behind its base) or equal to it.
    Ok(!repository.is_ancestor(head, base)?)
}

/// The paths that differ between two trees under `prefix`.
///
/// `pub(crate)` for the lifecycle adapters (`review`, `accept`), which need the
/// same tree comparison and must not grow a second one.
///
/// The delivered Git adapter exposes tree paths and per-path tree entries, not a
/// bulk diff, so this enumerates the candidates under the one prefix the caller
/// cares about and compares object ids. Path membership changes (added/deleted)
/// are detected without a second call; a common path is compared by oid.
pub(crate) fn changed_paths_between(
    repository: &sk_git::GitRepository,
    base: &sk_git::Oid,
    tip: &sk_git::Oid,
    prefix: &str,
) -> Result<Vec<String>, String> {
    const MAX_CANDIDATES: usize = 20_000;
    let prefix_path = (!prefix.is_empty())
        .then(|| sk_git::GitPath::from_git_bytes(prefix.as_bytes()))
        .transpose()
        .map_err(|error| format!("invalid path prefix {prefix:?}: {error}"))?;
    let list = |revision: &sk_git::Oid| -> Result<Vec<String>, String> {
        repository
            .tree_paths_at(
                &sk_git::StartPoint::Oid(revision.clone()),
                prefix_path.as_ref(),
            )
            .map_err(|error| format!("could not list the tree at {revision}: {error}"))?
            .into_iter()
            .map(|path| {
                path.as_os_str()
                    .to_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("non-UTF-8 tree path under {prefix:?}"))
            })
            .collect()
    };
    let base_paths = list(base)?;
    let tip_paths = list(tip)?;
    let mut candidates: std::collections::BTreeSet<String> = base_paths.iter().cloned().collect();
    candidates.extend(tip_paths.iter().cloned());
    if candidates.len() > MAX_CANDIDATES {
        return Err(format!(
            "the window covers {} paths under {prefix:?}, more than the native gate's {MAX_CANDIDATES}-path bound",
            candidates.len()
        ));
    }
    let base_set: std::collections::BTreeSet<&String> = base_paths.iter().collect();
    let tip_set: std::collections::BTreeSet<&String> = tip_paths.iter().collect();
    let mut changed = Vec::new();
    for path in &candidates {
        let in_base = base_set.contains(path);
        let in_tip = tip_set.contains(path);
        if in_base != in_tip {
            changed.push(path.clone());
            continue;
        }
        let git_path = sk_git::GitPath::from_git_bytes(path.as_bytes())
            .map_err(|error| format!("invalid tree path {path:?}: {error}"))?;
        let entry = |revision: &sk_git::Oid| -> Result<Option<String>, String> {
            repository
                .tree_entry_at(&sk_git::StartPoint::Oid(revision.clone()), &git_path)
                .map_err(|error| format!("could not read {path:?} at {revision}: {error}"))
                .map(|entry| entry.map(|entry| entry.oid().to_string()))
        };
        if entry(base)? != entry(tip)? {
            changed.push(path.clone());
        }
    }
    Ok(changed)
}

/// Lifecycle-facing wrapper over [`changed_paths_between`].
pub(crate) fn changed_paths_for_lifecycle(
    repository: &sk_git::GitRepository,
    base: &sk_git::Oid,
    tip: &sk_git::Oid,
    prefix: &str,
) -> Result<Vec<String>, String> {
    changed_paths_between(repository, base, tip, prefix)
}

/// The literal path prefix of an `owned_files` glob, or `""` for a wildcard head.
fn owned_literal_prefix(pattern: &str) -> String {
    let end = pattern.find(['*', '?', '[']).unwrap_or(pattern.len());
    let literal = &pattern[..end];
    match literal.rfind('/') {
        Some(index) => literal[..=index].to_owned(),
        None => String::new(),
    }
}

/// A `fnmatch.fnmatch`-compatible matcher plus the baseline's prefix rule.
fn matches_owned(path: &str, patterns: &[String]) -> bool {
    let normalized = path.replace('\\', "/");
    let normalized = normalized.strip_prefix("./").unwrap_or(&normalized);
    patterns.iter().any(|pattern| {
        if wildcard_match(pattern, normalized) {
            return true;
        }
        let prefix = pattern
            .replace("/**", "")
            .replace("/*", "")
            .trim_end_matches('/')
            .to_owned();
        !prefix.is_empty() && normalized.starts_with(&format!("{prefix}/"))
    })
}

fn wildcard_match(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    let mut memo: std::collections::HashMap<(usize, usize), bool> =
        std::collections::HashMap::new();
    fn go(
        p: &[char],
        n: &[char],
        pi: usize,
        ni: usize,
        memo: &mut std::collections::HashMap<(usize, usize), bool>,
    ) -> bool {
        if let Some(answer) = memo.get(&(pi, ni)) {
            return *answer;
        }
        let answer = if pi == p.len() {
            ni == n.len()
        } else {
            match p[pi] {
                '*' => go(p, n, pi + 1, ni, memo) || (ni < n.len() && go(p, n, pi, ni + 1, memo)),
                '?' => ni < n.len() && go(p, n, pi + 1, ni + 1, memo),
                '[' => match char_class(p, pi, ni, n) {
                    Some((matched, next_pi)) => matched && go(p, n, next_pi, ni + 1, memo),
                    None => ni < n.len() && n[ni] == '[' && go(p, n, pi + 1, ni + 1, memo),
                },
                literal => ni < n.len() && n[ni] == literal && go(p, n, pi + 1, ni + 1, memo),
            }
        };
        memo.insert((pi, ni), answer);
        answer
    }
    go(&pattern, &name, 0, 0, &mut memo)
}

/// Match one `[...]` class; `None` when the class is unterminated.
fn char_class(p: &[char], pi: usize, ni: usize, n: &[char]) -> Option<(bool, usize)> {
    if ni >= n.len() {
        return Some((false, pi));
    }
    let mut index = pi + 1;
    let negated = index < p.len() && (p[index] == '!' || p[index] == '^');
    if negated {
        index += 1;
    }
    let mut matched = false;
    let mut first = true;
    while index < p.len() && (p[index] != ']' || first) {
        first = false;
        if index + 2 < p.len() && p[index + 1] == '-' && p[index + 2] != ']' {
            if p[index] <= n[ni] && n[ni] <= p[index + 2] {
                matched = true;
            }
            index += 3;
        } else {
            if p[index] == n[ni] {
                matched = true;
            }
            index += 1;
        }
    }
    if index >= p.len() {
        return None;
    }
    Some((matched != negated, index + 1))
}

/// The move-task-only implementation-evidence gate (#5939 / #6715).
///
/// Mirrors `_mt_require_implementation_evidence`: it fires on a review-ward
/// crossing from below, never under `--force`, and only when `lanes.json` is
/// readable (a missing/corrupt manifest keeps today's behaviour). The window is
/// `planning_commit..<lane branch>` (or `..<target branch>` for a planning lane);
/// the WP's declared `owned_files` must have changed inside it. The native gate
/// compares the concrete owned prefixes; see REMEDIATION-REPORT.md for the
/// dependency-lane widening gap.
pub(super) fn enforce_review_evidence_gate(
    root: &Path,
    mission: &impl MissionView,
    wp_id: &str,
    old_lane: &str,
    to_lane: &str,
    force: bool,
) -> Result<(), CompatError> {
    if force || !review_ward(to_lane) || review_ward(old_lane) {
        return Ok(());
    }
    let Ok(Some(manifest)) = lane_manifest(mission) else {
        return Ok(());
    };
    let single_branch =
        mission.meta_field("topology").and_then(Value::as_str) == Some("single_branch");
    if single_branch {
        return single_branch_evidence_gate(root, mission, wp_id, &manifest);
    }
    let lane = lane_for_wp(&manifest, wp_id);
    let Some(base) = manifest
        .planning_commit_sha
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    else {
        let where_ = match lane {
            None => "it is in no lane of lanes.json".to_owned(),
            Some(lane) => format!("{}: lanes.json records no planning commit", lane.lane_id),
        };
        return Err(CompatError::coded(
            "EVIDENCE_WINDOW_UNCOMPUTABLE",
            format!(
                "EVIDENCE_WINDOW_UNCOMPUTABLE: {wp_id} has no evidence window ({where_}), so its implementation cannot be verified. Run `spec-kitty agent mission finalize-tasks` to record the planning commit, then retry."
            ),
        ));
    };
    let Some(lane) = lane else {
        return Err(CompatError::coded(
            "EVIDENCE_WINDOW_UNCOMPUTABLE",
            format!(
                "EVIDENCE_WINDOW_UNCOMPUTABLE: {wp_id} has no evidence window (it is in no lane of lanes.json), so its implementation cannot be verified. Run `spec-kitty agent mission finalize-tasks` to record the planning commit, then retry."
            ),
        ));
    };
    let tip_ref = if lane.is_planning() {
        manifest.target_branch.clone()
    } else {
        sk_workspace::lane_branch_name(mission.slug_str(), &lane.lane_id, &manifest.target_branch)
            .map_err(|error| {
            CompatError::domain(format!("could not resolve the lane branch: {error}"))
        })?
    };
    match owned_change_in_window(root, mission, &manifest, base, &tip_ref, wp_id) {
        Ok(true) => Ok(()),
        Ok(false) => Err(CompatError::coded(
            "IMPLEMENTATION_MISSING",
            format!(
                "IMPLEMENTATION_MISSING: {wp_id} cannot move into review: no change to its owned_files in its evidence window {base}..{tip_ref} ({}). Commit the work package's implementation to its owned_files (on its lane branch, or on the target branch for a planning-artifact work package), then retry (ticked subtask checkboxes are not shipped code).",
                lane.lane_id
            ),
        )),
        Err(detail) => Err(CompatError::coded(
            "IMPLEMENTATION_UNREADABLE",
            format!(
                "IMPLEMENTATION_UNREADABLE: {wp_id}'s evidence window {base}..{tip_ref} ({}) could not be diffed ({detail}), so its implementation cannot be verified. Make the lane branch and the recorded planning commit readable, then retry.",
                lane.lane_id
            ),
        )),
    }
}

fn single_branch_evidence_gate(
    root: &Path,
    mission: &impl MissionView,
    wp_id: &str,
    manifest: &sk_workspace::LanesManifest,
) -> Result<(), CompatError> {
    let Some(base) = manifest
        .planning_commit_sha
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    else {
        return Err(CompatError::coded(
            "SINGLE_BRANCH_EVIDENCE_WINDOW_UNCOMPUTABLE",
            format!(
                "Cannot verify a committed implementation for {wp_id}: this single_branch mission records no planning commit, so the implementation-evidence window cannot be computed. Run `spec-kitty agent mission finalize-tasks` to record the planning commit, then retry."
            ),
        ));
    };
    let tip = if manifest.mission_branch.trim().is_empty() {
        manifest.target_branch.clone()
    } else {
        manifest.mission_branch.clone()
    };
    match owned_change_in_window(root, mission, manifest, base, &tip, wp_id) {
        Ok(true) => Ok(()),
        Ok(false) => Err(CompatError::coded(
            "SINGLE_BRANCH_IMPLEMENTATION_MISSING",
            format!(
                "{wp_id} cannot move into review: no implementation commit touching its owned_files on the single_branch mission branch. Commit the work package's implementation to its owned_files, then retry (ticked subtask checkboxes are not shipped code)."
            ),
        )),
        Err(detail) => Err(CompatError::coded(
            "SINGLE_BRANCH_IMPLEMENTATION_UNREADABLE",
            format!(
                "Could not read the single_branch implementation window {base}..{tip} in {}; cannot verify {wp_id} has a committed implementation. Make sure the mission branch is checked out and its planning commit is reachable, then retry ({detail}).",
                root.display()
            ),
        )),
    }
}

/// Does the window hold a change to one of the WP's declared `owned_files`?
fn owned_change_in_window(
    root: &Path,
    mission: &impl MissionView,
    _manifest: &sk_workspace::LanesManifest,
    base: &str,
    tip: &str,
    wp_id: &str,
) -> Result<bool, String> {
    let owned = declared_owned_files(mission, wp_id)?;
    let repository = sk_git::GitRepository::open(root)
        .map_err(|error| format!("could not open {}: {error}", root.display()))?;
    let base = repository
        .resolve_commit(&sk_git::StartPoint::Oid(sk_git::Oid::parse(base).map_err(
            |error| format!("invalid planning commit {base:?}: {error}"),
        )?))
        .map_err(|error| format!("could not resolve planning commit {base}: {error}"))?;
    let tip = resolve_ref_commit(&repository, tip)?;
    let mut prefixes: Vec<String> = owned
        .iter()
        .map(|pattern| owned_literal_prefix(pattern))
        .collect();
    prefixes.sort();
    prefixes.dedup();
    for prefix in prefixes {
        let changed = changed_paths_between(&repository, &base, &tip, &prefix)?;
        if changed.iter().any(|path| matches_owned(path, &owned)) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// A WP's declared `owned_files` patterns from its prompt frontmatter.
fn declared_owned_files(mission: &impl MissionView, wp_id: &str) -> Result<Vec<String>, String> {
    let directory = mission.spec_dir().join("tasks");
    let entries = std::fs::read_dir(&directory)
        .map_err(|error| format!("could not read {}: {error}", directory.display()))?;
    let mut matches = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        if name.eq_ignore_ascii_case("readme.md") {
            continue;
        }
        if name
            .strip_prefix(wp_id)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(['-', '_', '.']))
        {
            matches.push(path);
        }
    }
    matches.sort();
    let Some(path) = (matches.len() == 1).then(|| matches.remove(0)) else {
        return Err(format!(
            "{} does not resolve exactly one WP prompt file",
            wp_id
        ));
    };
    let package = sk_mission::WorkPackage::from_file(&path)
        .map_err(|error| format!("could not parse {}: {error}", path.display()))?;
    let mut patterns = Vec::new();
    if let Some(value) = package.frontmatter_field("owned_files") {
        let Some(items) = value.as_array() else {
            return Err(format!("{} owned_files is not a list", path.display()));
        };
        for item in items {
            let Some(pattern) = item.as_str() else {
                return Err(format!(
                    "{} owned_files contains a non-string entry",
                    path.display()
                ));
            };
            if !pattern.trim().is_empty() {
                patterns.push(pattern.trim().to_owned());
            }
        }
    }
    Ok(patterns)
}

/// Emit one canonical status transition (`agent status emit`).
pub fn emit(root: &Path, args: &EmitArgs) -> Result<Value, CompatError> {
    refuse_seed_shaped_identity(&args.actor, "--actor")?;
    let positional = args.wp_id.as_deref().map(normalize_wp_id).transpose()?;
    let flagged = args.wp.as_deref().map(normalize_wp_id).transpose()?;
    let wp_id = match (positional, flagged) {
        (Some(left), Some(right)) if left != right => {
            return Err(CompatError::domain(format!(
                "Positional WP ID '{left}' and --wp '{right}' must match when both are supplied"
            )));
        }
        (Some(value), _) | (None, Some(value)) => value,
        (None, None) => {
            return Err(CompatError::domain(
                "A work package ID is required (positional or --wp)",
            ));
        }
    };

    let mission = require_mission(root, args.mission.as_deref())?;
    refuse_coordination(&mission, "status emission")?;
    if !mission.spec_dir().is_dir() {
        return Err(CompatError::domain(format!(
            "Mission directory not found: {}",
            mission.spec_dir().display()
        )));
    }

    let evidence = match &args.evidence_json {
        None => None,
        Some(raw) => serde_json::from_str::<Value>(raw).map(Some).map_err(|error| {
            CompatError::domain(format!(
                "Invalid JSON in --evidence-json: {error}\nExpected valid JSON object, e.g.: '{{\"review\": {{\"reviewer\": \"alice\", \"verdict\": \"approved\", \"reference\": \"PR#1\"}}}}'"
            ))
        })?,
    };
    let review_result = match &args.review_result_json {
        None => None,
        Some(raw) => Some(parse_review_result_json(raw)?),
    };

    // FR-011: the shared, topology-aware for_review commit gate, placed
    // immediately before the transition is emitted (the same placement the
    // orchestrator transition surface uses).
    enforce_for_review_commit_gate(root, &mission, &wp_id, &args.to, args.force)?;

    let current_lane = canonical_lanes(&mission)?
        .get(&wp_id)
        .cloned()
        .unwrap_or_else(|| "genesis".to_owned());
    let role = role_for_hop(Some(&current_lane), &args.to);
    let actor = actor_from_option(&args.actor, role, None, None);
    let lane_head = probe_lane_head(root, &mission, &wp_id);

    let mut request = sk_status::mutation::TransitionRequest::new(
        mission.spec_dir(),
        mission.slug().as_str(),
        &wp_id,
        args.to.clone(),
        actor,
    );
    request.repo_root = Some(root.to_path_buf());
    request.force = args.force;
    request.reason = args.reason.clone();
    request.evidence = evidence;
    request.review_ref = args.review_ref.clone();
    request.review_result = review_result;
    request.workspace_context = args.workspace_context.clone();
    request.subtasks_complete = args.subtasks_complete;
    request.implementation_evidence_present = args.implementation_evidence_present;
    request.execution_mode = args.execution_mode.clone();
    request.lane_head = lane_head;

    let outcome = sk_status::mutation::emit_status_transition(&request)
        .map_err(|error| CompatError::domain(error.to_string()))?;

    let payload = json!({
        "event_id": outcome.event.event_id,
        "wp_id": outcome.event.wp_id,
        "work_package_id": outcome.event.wp_id,
        "from_lane": outcome.event.from_lane.as_str(),
        "to_lane": outcome.event.to_lane.as_str(),
        "status_events_path": events_path(&mission).display().to_string(),
        "actor": args.actor,
    });

    if args.json {
        emit_json(&payload)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        let prefix: String = payload["event_id"]
            .as_str()
            .unwrap_or("")
            .chars()
            .take(12)
            .collect();
        println!(
            "OK {}: {} -> {} (event: {prefix}...)",
            payload["wp_id"].as_str().unwrap_or(""),
            payload["from_lane"].as_str().unwrap_or(""),
            payload["to_lane"].as_str().unwrap_or("")
        );
        if let Some(error) = &outcome.materialize_error {
            eprintln!("warning: status.json was not materialized: {error}");
        }
    }
    Ok(payload)
}

// ---------------------------------------------------------------------------
// Canonical lifecycle read (`agent status lifecycle`)
// ---------------------------------------------------------------------------

fn parse_dt(value: Option<&Value>) -> Option<chrono::DateTime<chrono::Utc>> {
    let text = value?.as_str()?;
    sk_kernel::clock::parse_iso(text).ok()
}

/// Render exactly like Python's `datetime.isoformat()` on an aware-UTC value:
/// no fractional part when the microsecond is zero, six digits otherwise, and a
/// `+00:00` offset (Python does not render `Z`).
fn iso(dt: Option<chrono::DateTime<chrono::Utc>>) -> Value {
    dt.map(|value| {
        let mut rendered = value.format("%Y-%m-%dT%H:%M:%S").to_string();
        let micros = value.timestamp_subsec_micros();
        if micros != 0 {
            rendered.push_str(&format!(".{micros:06}"));
        }
        rendered.push_str("+00:00");
        Value::String(rendered)
    })
    .unwrap_or(Value::Null)
}

fn post_mission_events(mission: &impl MissionView) -> Vec<Value> {
    let path = sk_status::mutation::mission_event_log_path(mission.spec_dir());
    let Ok(events) = sk_status::mutation::read_lifecycle_events(&path) else {
        return Vec::new();
    };
    let mut relevant: Vec<Value> = events
        .into_iter()
        .filter(|event| {
            matches!(
                event.get("event_type").and_then(Value::as_str),
                Some("MissionReopened") | Some("FollowUpRecorded")
            )
        })
        .collect();
    relevant.sort_by_key(|event| {
        (
            event
                .get("timestamp")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            event
                .get("event_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
        )
    });
    relevant
}

fn latest_event_time(event: &Value) -> Option<chrono::DateTime<chrono::Utc>> {
    if let Some(payload) = event.get("payload") {
        for key in ["reopened_at", "recorded_at"] {
            if let Some(parsed) = parse_dt(payload.get(key)) {
                return Some(parsed);
            }
        }
    }
    parse_dt(event.get("timestamp"))
}

/// Derive the canonical mission lifecycle state (baseline
/// `status.lifecycle.derive_mission_lifecycle`).
pub fn lifecycle(
    root: &Path,
    handle: Option<&str>,
    json_output: bool,
) -> Result<Value, CompatError> {
    lifecycle_at(root, handle, json_output, sk_kernel::clock::now_utc())
}

/// Derive the lifecycle state against an explicit `now` (deterministic tests).
pub fn lifecycle_at(
    root: &Path,
    handle: Option<&str>,
    json_output: bool,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Value, CompatError> {
    let mission = require_mission(root, handle)?;
    let snapshot = sk_status::materialize_dir(mission.spec_dir()).ok();
    let has_event_log = events_path(&mission).is_file();

    let lanes = canonical_lanes(&mission)?;
    let total_wps = lanes.len();
    let mut active = 0u64;
    let mut blocked = 0u64;
    let mut review = 0u64;
    let mut terminal = 0u64;
    for lane in lanes.values() {
        match lane.as_str() {
            "claimed" | "in_progress" | "for_review" | "in_review" | "approved" | "blocked" => {
                active += 1
            }
            _ => {}
        }
        match lane.as_str() {
            "blocked" => blocked += 1,
            "for_review" | "in_review" | "approved" => review += 1,
            "done" | "canceled" => terminal += 1,
            _ => {}
        }
    }

    let mut last_transition_at: Option<chrono::DateTime<chrono::Utc>> = None;
    if let Some(snapshot) = &snapshot {
        for state in snapshot.work_packages.values() {
            if let Some(parsed) = parse_dt(state.get("last_transition_at")) {
                last_transition_at =
                    Some(last_transition_at.map_or(parsed, |current| current.max(parsed)));
            }
        }
        if last_transition_at.is_none() {
            last_transition_at = parse_dt(Some(&Value::String(snapshot.materialized_at.clone())));
        }
    }
    let last_activity_at = last_transition_at.or_else(|| {
        let created = mission
            .meta_field("created_at")
            .and_then(|value| match value {
                Value::String(text) => sk_kernel::clock::parse_iso(text).ok(),
                _ => None,
            });
        created.or_else(|| {
            std::fs::metadata(mission.spec_dir())
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .map(chrono::DateTime::<chrono::Utc>::from)
        })
    });
    let age_days = last_activity_at.map(|activity| {
        let delta = now.signed_duration_since(activity);
        delta.num_days().max(0)
    });

    let events = post_mission_events(&mission);
    let last_reopen_at = events
        .iter()
        .filter(|event| event.get("event_type").and_then(Value::as_str) == Some("MissionReopened"))
        .filter_map(latest_event_time)
        .max();
    let last_follow_up_at = events
        .iter()
        .filter(|event| event.get("event_type").and_then(Value::as_str) == Some("FollowUpRecorded"))
        .filter_map(latest_event_time)
        .max();
    let last_merge_at = mission
        .meta_field("merged_at")
        .and_then(|value| match value {
            Value::String(text) => sk_kernel::clock::parse_iso(text).ok(),
            _ => None,
        });
    let is_reopened = match (last_reopen_at, last_merge_at) {
        (None, _) => false,
        (Some(_), None) => true,
        (Some(reopen), Some(merge)) => reopen > merge,
    };

    const STALE_AFTER_DAYS: i64 = 14;
    const ABANDONED_AFTER_DAYS: i64 = 30;
    const RECENT_COMPLETION_WINDOW_DAYS: i64 = 3;
    let (state, surface_state, reason) = if is_reopened {
        (
            "reopened",
            Some("reopened"),
            "Mission was re-opened after merge and is actionable again.",
        )
    } else if active > 0 {
        match age_days {
            Some(days) if days >= ABANDONED_AFTER_DAYS => (
                "abandoned",
                None,
                "Active work appears abandoned and should stay off primary surfaces.",
            ),
            Some(days) if days >= STALE_AFTER_DAYS => (
                "stale",
                None,
                "Active work has gone stale and should be recovered before it resurfaces.",
            ),
            _ => (
                "active",
                Some("active"),
                "Mission has live work in progress or review.",
            ),
        }
    } else if total_wps > 0 && terminal as usize == total_wps {
        match age_days {
            Some(days) if days <= RECENT_COMPLETION_WINDOW_DAYS => (
                "recently_completed",
                Some("recently_completed"),
                "Mission completed recently and still deserves short-lived visibility.",
            ),
            _ => (
                "archived",
                None,
                "Mission is completed and now belongs in history rather than the default surface.",
            ),
        }
    } else if age_days.is_some_and(|days| days >= ABANDONED_AFTER_DAYS) {
        (
            "abandoned",
            None,
            "Mission never reached an active terminal state and now looks abandoned.",
        )
    } else if age_days.is_some_and(|days| days >= STALE_AFTER_DAYS) {
        (
            "stale",
            None,
            "Mission is recoverable but stale and should not pollute primary surfaces.",
        )
    } else {
        (
            "recoverable",
            None,
            "Mission is recoverable history and not active enough for the default surface.",
        )
    };

    let completion_pct = super::tasks::weighted_percentage(snapshot.as_ref());
    let mission_number = mission
        .mission_number()
        .map(Value::from)
        .unwrap_or(Value::Null);
    let payload = json!({
        "mission_slug": mission.slug().as_str(),
        "mission_id": mission.mission_id_str().map(Value::from).unwrap_or(Value::Null),
        "mission_number": mission_number,
        "mission_type": mission.mission_type().map(Value::from).unwrap_or(Value::Null),
        "state": state,
        "surface_state": surface_state,
        "reason": reason,
        "last_activity_at": iso(last_activity_at),
        "last_transition_at": iso(last_transition_at),
        "age_days": age_days.map(Value::from).unwrap_or(Value::Null),
        "completion_pct": (completion_pct * 10.0).round() / 10.0,
        "event_count": snapshot.as_ref().map(|snapshot| snapshot.event_count).unwrap_or(0),
        "total_wps": total_wps,
        "active_wp_count": active,
        "blocked_wp_count": blocked,
        "review_wp_count": review,
        "terminal_wp_count": terminal,
        "has_event_log": has_event_log,
        "post_mission_events": events,
        "last_follow_up_at": iso(last_follow_up_at),
        "stale_after_days": STALE_AFTER_DAYS,
        "abandoned_after_days": ABANDONED_AFTER_DAYS,
    });

    if json_output {
        emit_json(&payload)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        println!();
        println!("Mission Lifecycle: {}", mission.slug().as_str());
        println!(
            "  Mission            {}",
            payload["mission_slug"].as_str().unwrap_or("")
        );
        println!("  State              {state}");
        println!(
            "  Default Surface    {}",
            payload["surface_state"].as_str().unwrap_or("off")
        );
        println!("  Reason             {reason}");
        println!("  Completion         {completion_pct:.1}%");
        println!("  Work Packages      {total_wps}");
        println!("  Active WPs         {active}");
        println!("  Review WPs         {review}");
        println!("  Blocked WPs        {blocked}");
        println!("  Terminal WPs       {terminal}");
        println!(
            "  Event Log          {}",
            if has_event_log { "present" } else { "missing" }
        );
        println!("  Age                {}d", age_days.unwrap_or(0));
    }
    Ok(payload)
}
