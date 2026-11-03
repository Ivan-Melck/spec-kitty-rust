//! Work-package read routes: `agent tasks list-tasks`, `agent tasks status`
//! and `agent tasks list-dependents`.
//!
//! Lanes come from the canonical event log through `sk-status`; frontmatter is
//! read through `sk-mission`. An absent event log reports every WP as
//! `planned`, exactly as the upstream board does before the first event.

mod commit;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use sk_mission::{MissionRecord, WorkPackageGraph, list_missions};
use sk_status::Snapshot;

use super::status::require_mission;
use super::support::{
    CompatError, MissionView, StatusHeadCommittedProbe, canonical_lanes, emit_json,
    emit_json_pretty, events_path, resolve_mission, tasks_dir,
};

/// Upstream lane-weighted progress weights.
const LANE_WEIGHTS: &[(&str, f64)] = &[
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

fn lane_weight(lane: &str) -> f64 {
    LANE_WEIGHTS
        .iter()
        .find(|(name, _)| *name == lane)
        .map(|(_, weight)| *weight)
        .unwrap_or(0.0)
}

fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

/// Compute the upstream weighted-readiness percentage for a snapshot.
pub fn weighted_percentage(snapshot: Option<&Snapshot>) -> f64 {
    let Some(snapshot) = snapshot else {
        return 0.0;
    };
    if snapshot.work_packages.is_empty() {
        return 0.0;
    }
    let mut weighted = 0.0;
    for state in snapshot.work_packages.values() {
        let lane = state.lane_string().unwrap_or("genesis");
        weighted += lane_weight(lane);
    }
    round1(weighted / snapshot.work_packages.len() as f64 * 100.0)
}

fn canonical_snapshot(mission: &impl MissionView) -> Option<Snapshot> {
    let path = super::support::events_path(mission);
    let rows = sk_status::read_raw_events(&path).ok()?;
    if rows.is_empty() {
        return None;
    }
    sk_status::reduce_rows(&rows).ok()
}

fn frontmatter_string(package: &sk_mission::WorkPackage, key: &str) -> Option<String> {
    package
        .frontmatter_field(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// List tasks, optionally filtered by an exact lane token.
pub fn list_tasks(
    root: &Path,
    handle: Option<&str>,
    lane: Option<&str>,
    json_output: bool,
) -> Result<Value, CompatError> {
    let mission = require_mission(root, handle)?;
    let directory = tasks_dir(&mission);
    if !directory.is_dir() {
        return Err(CompatError::domain(format!(
            "Tasks directory not found: {}",
            directory.display()
        )));
    }
    let lanes = canonical_lanes(&mission)?;
    let packages = super::support::work_packages(&mission)?;

    let mut tasks = Vec::new();
    for package in &packages {
        let wp_id = package.id().as_str();
        let task_lane = lanes
            .get(wp_id)
            .cloned()
            .unwrap_or_else(|| "planned".to_owned());
        if let Some(filter) = lane
            && task_lane != filter
        {
            continue;
        }
        tasks.push(json!({
            "work_package_id": wp_id,
            "title": package.title().unwrap_or(""),
            "lane": task_lane,
            "path": super::support::path_string(package.source()),
        }));
    }

    let value = json!({"tasks": tasks, "count": tasks.len()});
    if json_output {
        emit_json(&value)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else if tasks.is_empty() {
        println!(
            "No tasks found{}",
            lane.map(|lane| format!(" in lane {lane}"))
                .unwrap_or_default()
        );
    } else {
        println!(
            "Tasks{}:",
            lane.map(|lane| format!(" in lane {lane}"))
                .unwrap_or_default()
        );
        for task in &tasks {
            println!(
                "  {}: {} [{}]",
                task["work_package_id"].as_str().unwrap_or(""),
                task["title"].as_str().unwrap_or(""),
                task["lane"].as_str().unwrap_or("")
            );
        }
    }
    Ok(value)
}

/// The kanban status board's machine payload.
pub fn status(root: &Path, handle: Option<&str>, json_output: bool) -> Result<Value, CompatError> {
    let mission = require_mission(root, handle)?;
    let directory = tasks_dir(&mission);
    if !directory.is_dir() {
        return Err(CompatError::domain(format!(
            "Tasks directory not found: {}",
            directory.display()
        )));
    }
    let lanes = canonical_lanes(&mission)?;
    let packages = super::support::work_packages(&mission)?;
    let snapshot = canonical_snapshot(&mission);

    let mut rows = Vec::new();
    let mut lane_counts: BTreeMap<String, u64> = BTreeMap::new();
    let mut done_count = 0u64;
    for package in &packages {
        let wp_id = package.id().as_str();
        let lane = lanes
            .get(wp_id)
            .cloned()
            .unwrap_or_else(|| "planned".to_owned());
        if lane == "done" {
            done_count += 1;
        }
        *lane_counts.entry(lane.clone()).or_default() += 1;

        let state = snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.work_packages.get(wp_id));
        let state_str = |key: &str| {
            state
                .and_then(|state| state.get(key))
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        let agent = frontmatter_string(package, "agent").or_else(|| state_str("actor"));
        let phase =
            frontmatter_string(package, "phase").unwrap_or_else(|| "Unknown Phase".to_owned());
        let file = package
            .source()
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();

        rows.push(json!({
            "id": wp_id,
            "title": package.title().unwrap_or(""),
            "lane": lane,
            "phase": phase,
            "file": file,
            "agent": agent,
            "agent_profile": frontmatter_string(package, "agent_profile").unwrap_or_default(),
            "resolved_agent_profile": state_str("agent_profile"),
            "resolved_role": state_str("role"),
            "resolved_model": state_str("model"),
            "shell_pid": state_str("shell_pid"),
            "shell_pid_created_at": state_str("shell_pid_created_at"),
            "execution_mode": frontmatter_string(package, "execution_mode")
                .unwrap_or_else(|| "worktree".to_owned()),
            "workspace_kind": "lane",
        }));
    }

    let total = rows.len() as f64;
    let done_percentage = if total <= 0.0 {
        0.0
    } else {
        round1(done_count as f64 / total * 100.0)
    };
    let progress = weighted_percentage(snapshot.as_ref());

    let identity = |mission: &MissionRecord| {
        (
            mission.slug().as_str().to_owned(),
            mission
                .mission_number()
                .map(Value::from)
                .unwrap_or(Value::Null),
            mission.mission_type().unwrap_or_default().to_owned(),
        )
    };
    let (slug, number, mission_type) = identity(&mission);

    let payload = json!({
        "mission_slug": slug,
        "mission_number": number,
        "mission_type": mission_type,
        "total_wps": rows.len(),
        "by_lane": lane_counts,
        "work_packages": rows,
        "progress_percentage": progress,
        "progress_semantics": "weighted_readiness",
        "weighted_percentage": progress,
        "done_count": done_count,
        "done_percentage": done_percentage,
        "stale_wps": 0,
        "stale_verdicts": [],
        "stalled_wps": [],
        "auto_commit": true,
    });

    if json_output {
        emit_json_pretty(&payload)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else if rows.is_empty() {
        println!("No work packages found in {}", directory.display());
    } else {
        println!(
            "Mission {}: {}/{} done ({done_percentage:.1}%), weighted {progress:.1}%",
            mission.slug().as_str(),
            done_count,
            rows.len()
        );
    }
    Ok(payload)
}

/// Downstream dependents and upstream dependencies of one WP.
pub fn list_dependents(
    root: &Path,
    handle: Option<&str>,
    wp_id: &str,
    json_output: bool,
) -> Result<Value, CompatError> {
    let mission = require_mission(root, handle)?;
    let directory = tasks_dir(&mission);
    if !directory.is_dir() {
        return Err(CompatError::domain(format!(
            "Mission directory not found: {}",
            mission.spec_dir().display()
        )));
    }
    let graph = WorkPackageGraph::from_tasks_dir(&directory).map_err(|error| {
        CompatError::domain(format!("could not build the work-package graph: {error}"))
    })?;
    let target: sk_kernel::WpId = wp_id
        .parse()
        .map_err(|error| CompatError::domain(format!("invalid work package id: {error}")))?;
    let depends_on: Vec<String> = graph
        .dependencies_of(&target)
        .map(|deps| deps.iter().map(ToString::to_string).collect())
        .unwrap_or_default();
    let dependents: Vec<String> = graph
        .dependents_of(&target)
        .map(|deps| deps.iter().map(ToString::to_string).collect())
        .unwrap_or_default();

    let value = json!({
        "wp_id": wp_id,
        "depends_on": depends_on,
        "dependents": dependents,
    });
    if json_output {
        emit_json(&value)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        println!(
            "{wp_id} depends on: {}",
            if depends_on.is_empty() {
                "(none)".to_owned()
            } else {
                depends_on.join(", ")
            }
        );
        println!(
            "{wp_id} depended on by: {}",
            if dependents.is_empty() {
                "(none)".to_owned()
            } else {
                dependents.join(", ")
            }
        );
    }
    Ok(value)
}

/// List the whole inventory (used by capability diagnostics and tests).
pub fn inventory(root: &Path) -> Result<Vec<String>, CompatError> {
    let missions = list_missions(root)
        .map_err(|error| CompatError::domain(format!("could not read missions: {error}")))?;
    let mut ids: Vec<String> = missions
        .iter()
        .map(|mission| mission.slug().as_str().to_owned())
        .collect();
    ids.sort();
    Ok(ids)
}

/// Resolve a mission for a status read (shared with the CLI dispatcher).
pub fn mission_for(root: &Path, handle: Option<&str>) -> Result<sk_mission::Mission, CompatError> {
    resolve_mission(root, handle)
}

/// Typed error shim kept for dispatcher symmetry.
pub fn machine_error(message: impl Into<String>) -> CompatError {
    CompatError::domain(message)
}

#[allow(dead_code)]
fn identity_fields(mission: &sk_mission::Mission) -> Map<String, Value> {
    let mut map = Map::new();
    map.insert("mission_slug".into(), json!(mission.slug().as_str()));
    map.insert(
        "mission_number".into(),
        mission
            .mission_number()
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    map.insert(
        "mission_type".into(),
        json!(mission.mission_type().unwrap_or_default()),
    );
    map
}

// ===========================================================================
// Mutation routes: `agent tasks move-task` and `agent tasks mark-status`
// ===========================================================================

use clap::Args;

use super::status::{
    actor_from_option, enforce_for_review_commit_gate, enforce_review_evidence_gate,
    normalize_wp_id, probe_lane_head, refuse_coordination, refuse_seed_shaped_identity,
    role_for_hop,
};

/// `agent tasks move-task` options (the upstream Typer surface).
#[derive(Debug, Args)]
pub struct MoveTaskArgs {
    /// Task ID (e.g. WP01).
    #[arg(value_name = "TASK_ID")]
    pub task_id: String,
    /// Target lane.
    #[arg(long, value_name = "LANE")]
    pub to: String,
    /// Mission slug.
    #[arg(long)]
    pub mission: Option<String>,
    /// Agent name (alias: --actor; the last occurrence wins).
    #[arg(long = "agent", alias = "actor")]
    pub agent: Option<String>,
    /// Dispatch-resolved model actual.
    #[arg(long)]
    pub model: Option<String>,
    /// Dispatch-resolved agent profile actual.
    #[arg(long)]
    pub profile: Option<String>,
    /// Authoritative dispatch Op record id.
    #[arg(long = "invocation-id")]
    pub invocation_id: Option<String>,
    /// Assignee recorded on this transition.
    #[arg(long)]
    pub assignee: Option<String>,
    /// Shell PID.
    #[arg(long = "shell-pid")]
    pub shell_pid: Option<String>,
    /// History note (alias: --reason; the last occurrence wins).
    #[arg(long = "note", alias = "reason")]
    pub note: Option<String>,
    /// Review feedback file (required on review-rejection edges).
    #[arg(long = "review-feedback-file", value_name = "PATH")]
    pub review_feedback_file: Option<std::path::PathBuf>,
    /// Approval reference for approval/done transitions.
    #[arg(long = "approval-ref")]
    pub approval_ref: Option<String>,
    /// Reviewer name (falls back to the acting identity).
    #[arg(long)]
    pub reviewer: Option<String>,
    /// Record that approval is a self-review fallback.
    #[arg(long = "self-review-fallback")]
    pub self_review_fallback: bool,
    /// Reviewer that should have reviewed this WP before fallback.
    #[arg(long = "intended-reviewer")]
    pub intended_reviewer: Option<String>,
    /// Reason the intended reviewer failed.
    #[arg(long = "reviewer-failure-reason")]
    pub reviewer_failure_reason: Option<String>,
    /// Required for `--to done` (merge ancestry is not verified natively).
    #[arg(long = "done-override-reason")]
    pub done_override_reason: Option<String>,
    /// Force even with unchecked subtasks.
    #[arg(long)]
    pub force: bool,
    /// External tracker reference (repeatable).
    #[arg(long = "tracker-ref")]
    pub tracker_ref: Vec<String>,
    /// Override a rejected latest review artifact when arbiter-approving.
    #[arg(long = "skip-review-artifact-check")]
    pub skip_review_artifact_check: bool,
    /// Commit the status/verdict artifacts (default: from project config).
    ///
    /// Upstream declares one Typer boolean pair
    /// (`typer.Option("--auto-commit/--no-auto-commit")`): a value is never
    /// accepted, and the last spelling wins. The count is presence-only (the
    /// negative option can therefore never resolve to an enabling `true`).
    #[arg(
        long = "auto-commit",
        overrides_with = "no_auto_commit",
        action = clap::ArgAction::Count
    )]
    pub auto_commit: Option<u8>,
    /// Disable automatic commits.
    #[arg(
        long = "no-auto-commit",
        overrides_with = "auto_commit",
        action = clap::ArgAction::Count
    )]
    pub no_auto_commit: Option<u8>,
    /// Skip the pre-review regression gate.
    #[arg(long = "skip-pre-review-gate")]
    pub skip_pre_review_gate: bool,
    /// Owned single-branch checkout (unsupported natively).
    #[arg(long = "owned-checkout", value_name = "PATH")]
    pub owned_checkout: Option<std::path::PathBuf>,
    /// Emit JSON.
    #[arg(long)]
    pub json: bool,
}

fn refusal_message(error: &sk_status::mutation::MutationError) -> String {
    error.to_string()
}

/// Resolve the explicit Typer boolean-pair decision.
///
/// Each side is presence-only, so a valued or inverted spelling can never turn
/// the negative option into an enabling `true`. Clap's `overrides_with` makes
/// the pair last-wins; if both were ever present (unreachable through the
/// parser), the negative side wins, because it must never enable a commit.
fn explicit_auto_commit(auto: Option<u8>, no_auto: Option<u8>) -> Option<bool> {
    let auto = auto.is_some_and(|count| count > 0);
    let no_auto = no_auto.is_some_and(|count| count > 0);
    match (auto, no_auto) {
        (true, false) => Some(true),
        (false, true) | (true, true) => Some(false),
        (false, false) => None,
    }
}

fn current_lane(mission: &impl MissionView, wp_id: &str) -> Result<String, CompatError> {
    Ok(canonical_lanes(mission)?
        .get(wp_id)
        .cloned()
        .unwrap_or_else(|| "genesis".to_owned()))
}

fn is_review_rejection_edge(old_lane: &str, target_lane: &str) -> bool {
    let old = sk_status::resolve_lane_alias(old_lane);
    let target = sk_status::resolve_lane_alias(target_lane);
    target == "planned" || (old == "in_review" && target == "in_progress")
}

fn is_approved_reopen(old_lane: &str, target_lane: &str) -> bool {
    sk_status::resolve_lane_alias(old_lane) == "approved"
        && sk_status::resolve_lane_alias(target_lane) == "in_progress"
}

/// `_self_review_fallback_option_error`: the option-set validation, in order.
fn self_review_fallback_option_error(args: &MoveTaskArgs, target: &str) -> Option<String> {
    let intended = args
        .intended_reviewer
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let failure = args
        .reviewer_failure_reason
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if !args.self_review_fallback {
        if intended.is_some() || failure.is_some() {
            return Some(
                "--intended-reviewer/--reviewer-failure-reason require --self-review-fallback."
                    .to_owned(),
            );
        }
        return None;
    }
    if !matches!(
        sk_status::resolve_lane_alias(target).as_str(),
        "approved" | "done"
    ) {
        return Some(
            "--self-review-fallback is only valid when approving or marking done.".to_owned(),
        );
    }
    if !args.force {
        return Some(
            "--self-review-fallback requires --force so force_count records the independence override."
                .to_owned(),
        );
    }
    if intended.is_none() {
        return Some("--self-review-fallback requires --intended-reviewer <agent>.".to_owned());
    }
    if failure.is_none() {
        return Some(
            "--self-review-fallback requires --reviewer-failure-reason <reason>.".to_owned(),
        );
    }
    None
}

/// `_feedback_file_no_effect_message`: a feedback file on a non-recordable edge (#6699).
fn feedback_file_no_effect_message(task_id: &str, old_lane: &str, target_lane: &str) -> String {
    let old = sk_status::resolve_lane_alias(old_lane);
    let target = sk_status::resolve_lane_alias(target_lane);
    format!(
        "Cannot move {task_id} from '{old}' to '{target}' with --review-feedback-file: \
         this edge writes no review cycle, so the file would be validated and then dropped.\n\n\
         The flag is only used on:\n\
         \x20 - review-rejection edges: --to planned (from any lane), and in_review -> in_progress\n\
         \x20 - approval edges: --to approved / --to done (recorded when a new approval review-cycle is written;\n\
         \x20   otherwise a warning on stderr says it was not recorded)\n\n\
         Remove --review-feedback-file from this move, or put the rationale in --note."
    )
}

/// `_planned_rollback_message`: the source-aware rejection refusal text.
fn planned_rollback_message(task_id: &str, old_lane: &str, target_lane: &str) -> String {
    let Ok(source) = sk_status::Lane::parse_input(old_lane) else {
        return format!("\u{274c} Moving {task_id} to '{target_lane}' requires review feedback.");
    };
    let Ok(target) = sk_status::Lane::parse_input(target_lane) else {
        return format!("\u{274c} Moving {task_id} to '{target_lane}' requires review feedback.");
    };
    if source.may_transition_to(sk_status::Lane::Planned) {
        return format!(
            "\u{274c} Moving {task_id} to '{target}' requires review feedback.\n\n\
             Please provide feedback:\n\
             \x201. Create feedback file: echo '**Issue**: Description' > feedback.md\n\
             \x202. Run: spec-kitty agent tasks move-task {task_id} \
             --to {target} --review-feedback-file feedback.md\n\n\
             This requirement cannot be bypassed with --force."
        );
    }
    let mut lines = vec![format!(
        "\u{274c} Cannot move {task_id} from '{source}' to 'planned': 'planned' is not reachable from '{source}'."
    )];
    let targets = source.allowed_targets();
    if targets.is_empty() {
        lines.push(format!(
            "\n\n'{source}' is a terminal lane with no onward transitions."
        ));
    } else {
        let legal = targets
            .iter()
            .map(|target| format!("--to {target}"))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("\n\nLegal targets from '{source}': {legal}."));
        if source == sk_status::Lane::Blocked
            && source.may_transition_to(sk_status::Lane::InProgress)
        {
            lines.push(format!(
                "\nTo resume this work package, run: spec-kitty agent tasks move-task {task_id} --to in_progress."
            ));
        }
    }
    lines.join("")
}

/// #6715: refuse an unforced `approved`/`done` move that is not an independent review.
///
/// Two cases refuse under the baseline's single code: an approval that would be
/// recorded as a generic placeholder (no `--agent`, or one that projects to
/// `user`/`unknown`/`implement-command`), and an approver whose full
/// `tool:model:profile` identity is not provably independent of the implementer
/// of record. `--force` bypasses both (operator ruling 3). An unknown
/// implementer passes here; accept/consolidate stay strict.
fn require_independent_approval(
    mission: &impl MissionView,
    wp_id: &str,
    target: &str,
    force: bool,
    acting: &str,
    actor: &sk_status::mutation::ActorValue,
) -> Result<(), CompatError> {
    if force || !matches!(target, "approved" | "done") {
        return Ok(());
    }
    let code = if mission.meta_field("topology").and_then(Value::as_str) == Some("single_branch") {
        "SINGLE_BRANCH_REVIEW_NOT_INDEPENDENT"
    } else {
        "REVIEW_NOT_INDEPENDENT"
    };
    let actor_value = actor.to_json_value();
    let key = sk_status::actor_key(&actor_value);
    if key
        .as_deref()
        .is_none_or(|key| sk_status::GENERIC_IMPLEMENTATION_ACTORS.contains(&key))
    {
        return Err(CompatError::coded(
            code,
            format!(
                "{code}: {wp_id} cannot be approved without a reviewer identity: the approval would be recorded as the placeholder '{}'. Pass --agent <your identity> (a reviewer other than the implementer).",
                key.as_deref().unwrap_or(sk_status::UNKNOWN_ACTOR)
            ),
        ));
    }
    let approver = sk_status::actor_full_identity(&actor_value);
    let rows =
        sk_status::read_raw_events(&super::support::events_path(mission)).unwrap_or_default();
    let implementer = sk_status::latest_implementer_actor(&rows, wp_id);
    if implementer.is_none()
        || sk_status::actor_identities_independent(implementer.as_deref(), approver.as_deref())
    {
        return Ok(());
    }
    let _ = acting;
    Err(CompatError::coded(
        code,
        format!(
            "{code}: {wp_id} cannot be approved by its implementer ({}): the approver ({}) is not provably another identity (same agent, and no model or profile recorded on both sides that differs). Have a reviewer with a different agent approve it, or pass the full --agent <tool>:<model>:<profile> of both implementer and reviewer.",
            implementer.unwrap_or_default(),
            approver.unwrap_or_default()
        ),
    ))
}

fn wp_file(mission: &impl MissionView, wp_id: &str) -> Option<std::path::PathBuf> {
    let directory = tasks_dir(mission);
    let mut matches: Vec<std::path::PathBuf> = std::fs::read_dir(&directory)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().and_then(|value| value.to_str()) == Some("md")
                && path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .map(|name| {
                        name.to_ascii_lowercase()
                            .starts_with(&wp_id.to_ascii_lowercase())
                    })
                    .unwrap_or(false)
                && !path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .is_some_and(|name| name.eq_ignore_ascii_case("readme.md"))
        })
        .collect();
    matches.sort();
    (matches.len() == 1).then(|| matches.remove(0))
}

/// Move a work package between lanes (`agent tasks move-task`).
///
/// This is the canonical status mutation: one validated transition (with the
/// claim sidecar, approval evidence, verdict and lane-head stamp) plus the
/// off-axis runtime-state annotation the move implies. Git-derived gates the
/// baseline layers on top (pre-review regression gate, review-cycle artifacts,
/// auto-commit, done merge-ancestry, owned checkouts) are **not** silently
/// simulated here — `--to done` refuses without `--done-override-reason`, and
/// the accepted commit/review-artifact flags are recorded as not performed.
/// The latest event-sourced `review_result.verdict` for a work package.
///
/// The approval writer is a no-op when the current verdict is already an
/// approval (`_approval_write_skip_reason`); a rejection or an absent slot gets
/// a fresh approval artifact.
fn latest_review_verdict(mission: &impl MissionView, wp_id: &str) -> Option<String> {
    let rows = sk_status::read_raw_events(&events_path(mission)).ok()?;
    let mut verdict = None;
    for row in rows {
        if row["wp_id"].as_str() != Some(wp_id) {
            continue;
        }
        if let Some(value) = row["review_result"]["verdict"].as_str() {
            verdict = Some(value.to_owned());
        }
    }
    verdict
}

/// POSIX single-quote a value for the recorded reproduction command
/// (`shlex.join`'s shape for the inputs this route records).
fn shell_quote(value: &str) -> String {
    let safe = !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_-./:@+,=".contains(character));
    if safe {
        value.to_owned()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

/// Allocate the canonical review-cycle artifact for this move.
///
/// The caller writes it (`write_new`) under the Mission hold; this helper only
/// allocates the next number over the consulted directory and renders the
/// pinned model.
fn review_cycle_for_move(
    mission: &impl MissionView,
    wp_id: &str,
    reviewer: &str,
    body: String,
    reproduction_command: Option<String>,
) -> Result<(sk_status::mutation_commit::ReviewCycleArtifact, String), CompatError> {
    let wp_slug = wp_slug_for(mission, wp_id);
    let directory = sk_status::mutation_commit::review_cycle_dir(mission.spec_dir(), &wp_slug);
    let cycle_number =
        sk_status::mutation_commit::next_review_cycle_number(&[directory]).map_err(|error| {
            CompatError::domain(format!(
                "could not allocate the next review-cycle number: {error}"
            ))
        })?;
    let artifact = sk_status::mutation_commit::ReviewCycleArtifact {
        cycle_number,
        wp_id: wp_id.to_owned(),
        mission_slug: mission.slug_str().to_owned(),
        reviewer_agent: reviewer.to_owned(),
        reviewed_at: sk_kernel::clock::now_utc_iso(),
        affected_files: Vec::new(),
        reproduction_command,
        body,
        override_actor: None,
        override_reason: None,
    };
    Ok((artifact, wp_slug))
}

/// The repository-relative evidence reference for a path under `root`.
fn evidence_ref_from_root(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .ok()
        .and_then(|relative| relative.to_str())
        .map(|relative| relative.replace('\\', "/"))
        .unwrap_or_else(|| path.display().to_string())
}

/// The directory slug of a work package (`WP01` -> the WP file stem).
fn wp_slug_for(mission: &impl MissionView, wp_id: &str) -> String {
    wp_file(mission, wp_id)
        .and_then(|path| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| wp_id.to_owned())
}

/// What happened to the review-cycle artifact this call wrote.
enum EvidenceRetirement {
    /// The artifact was this call's own, provably uncommitted file and is gone.
    Removed,
    /// The artifact was kept, with the reason to report.
    Retained(String),
}

fn retire_failed_transition_evidence(
    authority: Option<&commit::CommitAuthority>,
    path: &Path,
    error: &sk_status::mutation::MutationError,
) -> EvidenceRetirement {
    retire_uncommitted_evidence(authority, path, !error.may_have_mutated())
}

/// Remove the review-cycle artifact only after proof, symmetric with the log.
///
/// Deletion requires BOTH:
///
/// 1. the status rows this call wrote are provably absent or were provably
///    uncommitted and cut (`status_proven_uncommitted`); and
/// 2. no blob for the artifact path exists at the destination branch — the
///    evidence analogue of the status log's committed-at-`HEAD` probe. An
///    unreadable destination retains the file instead of guessing.
///
/// A hook that commits the rows and artifact itself, or a refused rollback,
/// therefore keeps every byte and any `review_ref` that still points at it.
fn retire_uncommitted_evidence(
    authority: Option<&commit::CommitAuthority>,
    path: &Path,
    status_proven_uncommitted: bool,
) -> EvidenceRetirement {
    if !status_proven_uncommitted {
        return EvidenceRetirement::Retained(
            "the review-cycle artifact was retained because the status rows could not be proven uncommitted"
                .to_owned(),
        );
    }
    if let Some(authority) = authority {
        match authority.path_committed(path) {
            Ok(false) => {}
            Ok(true) => {
                return EvidenceRetirement::Retained(format!(
                    "the review-cycle artifact was retained because a copy is already committed at '{}'",
                    authority.target_name
                ));
            }
            Err(error) => {
                return EvidenceRetirement::Retained(format!(
                    "the review-cycle artifact was retained because it could not be proven uncommitted ({error})"
                ));
            }
        }
    }
    match std::fs::remove_file(path) {
        Ok(()) => EvidenceRetirement::Removed,
        Err(error) => EvidenceRetirement::Retained(format!(
            "the uncommitted review-cycle artifact could not be removed ({error}); it was retained"
        )),
    }
}

/// Move a work package between lanes (`agent tasks move-task`).
///
/// This is the canonical status mutation: one validated transition (with the
/// claim sidecar, approval evidence, verdict and lane-head stamp), the canonical
/// review-cycle artifact for a review-rejection/approval edge, and — when the
/// project config/flags resolve auto-commit on — one path-scoped safe commit of
/// the status artifacts and that evidence. Git-derived gates the baseline layers
/// on top (pre-review regression gate, done merge-ancestry, owned checkouts) are
/// **not** silently simulated; `--to done` still refuses without
/// `--done-override-reason` and an owned checkout still refuses.
///
/// The verdict is durable only when the commit landed and the exact artifact
/// bytes read back from the destination ref match. A provably **not landed**
/// commit failure rolls back this operation's own uncommitted status tail under
/// the same Mission hold, using the real committed-at-HEAD probe; a landed or
/// indeterminate outcome never truncates, rolls back or deletes anything.
pub fn move_task(root: &Path, args: &MoveTaskArgs) -> Result<Value, CompatError> {
    if let Some(path) = &args.owned_checkout {
        return Err(CompatError::unsupported(
            "OWNED_CHECKOUT_UNAVAILABLE",
            format!(
                "move-task --owned-checkout {} requires the owned-checkout lifecycle authority, which this native surface does not implement",
                path.display()
            ),
        ));
    }
    let wp_id = normalize_wp_id(&args.task_id)?;
    let mission = require_mission(root, args.mission.as_deref())?;
    refuse_coordination(&mission, "move-task")?;
    let auto_commit = commit::resolve_auto_commit(
        root,
        explicit_auto_commit(args.auto_commit, args.no_auto_commit),
    )?;
    let commit_authority = commit::prepare(root, &mission, auto_commit)?;
    // `move-task` keeps the reference's hard protected-primary refusal
    // (`_protected_branch_status_commit_error`) before any status read. Only
    // `mark-status` degrades this refusal (see `mark_status`).
    if let Some(authority) = &commit_authority
        && authority.protected_destination
    {
        return Err(authority.protected_refusal("spec-kitty agent tasks move-task"));
    }
    let old_lane = current_lane(&mission, &wp_id)?;
    let target_lane = sk_status::Lane::parse_input(&args.to)
        .map_err(|error| CompatError::domain(error.to_string()))?;
    let target = target_lane.as_str();

    if target == "done" && args.done_override_reason.is_none() {
        return Err(CompatError::coded(
            "DONE_ANCESTRY_UNVERIFIED",
            "move-task --to done verifies merge ancestry in the baseline (a Git contract this native surface does not implement); pass --done-override-reason \"<why>\" to record the documented override",
        ));
    }

    // Self-review fallback metadata is validated BEFORE anything is read or
    // written (`_self_review_fallback_option_error`).
    if let Some(message) = self_review_fallback_option_error(args, target) {
        return Err(CompatError::domain(message));
    }

    // `--review-feedback-file` is only meaningful on an edge that writes a
    // review cycle (#6699): the rejection family and the approval lanes. On any
    // other edge it would be validated and then dropped, so it is refused.
    let feedback_recordable =
        is_review_rejection_edge(&old_lane, target) || matches!(target, "approved" | "done");
    let mut feedback_pointer: Option<String> = None;
    let mut feedback_text: Option<String> = None;
    if let Some(path) = &args.review_feedback_file {
        if !feedback_recordable {
            return Err(CompatError::domain(feedback_file_no_effect_message(
                &wp_id, &old_lane, target,
            )));
        }
        match std::fs::metadata(path) {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => {
                return Err(CompatError::domain(format!(
                    "Review feedback path is not a file: {}",
                    path.display()
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(CompatError::domain(format!(
                    "Review feedback file not found: {}",
                    path.display()
                )));
            }
            Err(error) => {
                return Err(CompatError::domain(format!(
                    "could not inspect --review-feedback-file {}: {error}",
                    path.display()
                )));
            }
        }
        let text = std::fs::read_to_string(path).map_err(|error| {
            CompatError::domain(format!(
                "could not read --review-feedback-file {}: {error}",
                path.display()
            ))
        })?;
        if feedback_recordable && !text.trim().is_empty() {
            // #990/#2996(b): a prior review-cycle artifact is never fresh
            // feedback, by path identity or by content identity
            // (`_guard_feedback_source_provenance`). Both the rejection and the
            // approval/done edges pass a supplied file as `feedback_source`
            // (`_approval_feedback_source`); an empty approval file is dropped
            // before the guard, exactly like the reference.
            let wp_slug = wp_slug_for(&mission, &wp_id);
            let directory =
                sk_status::mutation_commit::review_cycle_dir(mission.spec_dir(), &wp_slug);
            if let Some(provenance) =
                sk_status::mutation_commit::review_feedback_provenance(path, &directory, &text)
            {
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default();
                return Err(CompatError::domain(provenance.refusal(name)));
            }
        }
        if is_approved_reopen(&old_lane, target) && text.trim().is_empty() {
            return Err(CompatError::domain(format!(
                "Review feedback file is empty: {}",
                path.display()
            )));
        }
        feedback_text = Some(text);
        feedback_pointer = Some(path.display().to_string());
    }
    // The review-rejection family MUST carry a substantive feedback file; a
    // `--note` never substitutes for it (`_guard_planned_rollback`, explicit
    // "cannot be bypassed with --force").
    if is_review_rejection_edge(&old_lane, target) {
        let Some(text) = feedback_text.as_deref() else {
            return Err(CompatError::domain(planned_rollback_message(
                &wp_id, &old_lane, target,
            )));
        };
        if text.trim().is_empty() {
            let source = args
                .review_feedback_file
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default();
            return Err(CompatError::domain(format!(
                "Review feedback file is empty: {source}"
            )));
        }
    }

    let acting = args
        .agent
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("user");
    refuse_seed_shaped_identity(acting, "--agent")?;
    let role = role_for_hop(Some(&old_lane), target);
    let actor = actor_from_option(acting, role, args.model.as_deref(), args.profile.as_deref());
    let reviewer = args
        .reviewer
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(acting)
        .to_owned();

    // #5939/#6715: a review-ward move must carry a committed implementation.
    // The shared commit gate covers `for_review`; the evidence window covers
    // every review-ward crossing from below (for_review/in_review/approved/done).
    // Both run before the canonical transition is written.
    // Baseline order: the implementation-evidence window runs first, then the
    // shared for_review commit gate.
    enforce_review_evidence_gate(root, &mission, &wp_id, &old_lane, target, args.force)?;
    enforce_for_review_commit_gate(root, &mission, &wp_id, target, args.force)?;

    // #6715: an unforced approval must be a provably independent review.
    require_independent_approval(&mission, &wp_id, target, args.force, acting, &actor)?;

    // The canonical review-cycle evidence this edge writes, if any. The
    // rejection family writes the reviewer's feedback; an approval/done hop
    // writes the synthesized approval record when the current verdict is not
    // already an approval.
    let mut review_cycle_artifact: Option<(
        sk_status::mutation_commit::ReviewCycleArtifact,
        String,
    )> = None;
    let mut review_cycle_verdict: Option<&str> = None;
    if is_review_rejection_edge(&old_lane, target) {
        let body = feedback_text.clone().unwrap_or_default();
        let (artifact, wp_slug) = review_cycle_for_move(&mission, &wp_id, &reviewer, body, None)?;
        review_cycle_verdict = Some("rejected");
        review_cycle_artifact = Some((artifact, wp_slug));
    } else if matches!(target, "approved" | "done")
        && latest_review_verdict(&mission, &wp_id).as_deref() != Some("approved")
    {
        let reference = args
            .approval_ref
            .clone()
            .unwrap_or_else(|| format!("approval:{wp_id}"));
        let body = format!("Approved by {reviewer}: {reference}\n");
        let reproduction_command = format!(
            "spec-kitty agent tasks move-task {} --to approved --mission {} --agent {}",
            shell_quote(&wp_id),
            shell_quote(mission.slug_str()),
            shell_quote(&reviewer)
        );
        let (artifact, wp_slug) = review_cycle_for_move(
            &mission,
            &wp_id,
            &reviewer,
            body,
            Some(reproduction_command),
        )?;
        review_cycle_verdict = Some("approved");
        review_cycle_artifact = Some((artifact, wp_slug));
    }
    let review_cycle_pointer = match &review_cycle_artifact {
        Some((artifact, wp_slug)) => Some(artifact.pointer(wp_slug).map_err(|error| {
            CompatError::domain(format!("could not build the review-cycle pointer: {error}"))
        })?),
        None => None,
    };

    let mut review_result = None;
    let mut evidence = None;
    if old_lane == "in_review" {
        let (verdict, reference) = if matches!(target, "approved" | "done") {
            (
                "approved",
                review_cycle_pointer
                    .clone()
                    .or_else(|| args.approval_ref.clone())
                    .unwrap_or_else(|| format!("approval:{wp_id}")),
            )
        } else {
            (
                "changes_requested",
                review_cycle_pointer
                    .clone()
                    .or_else(|| feedback_pointer.clone())
                    .unwrap_or_else(|| format!("review:{wp_id}")),
            )
        };
        review_result = Some(json!({
            "reviewer": reviewer,
            "verdict": verdict,
            "reference": reference,
        }));
    }
    if matches!(target, "approved" | "done") {
        let reference = review_cycle_pointer
            .clone()
            .or_else(|| args.approval_ref.clone())
            .unwrap_or_else(|| format!("approval:{wp_id}"));
        evidence = Some(json!({
            "review": {
                "reviewer": reviewer,
                "verdict": "approved",
                "reference": reference,
            }
        }));
    }

    // Only an edge with a real review cycle records an on-wire `review_ref`
    // (#6699): a rejection carries its canonical pointer, an operator rewind
    // out of `for_review` its sentinel, and everything else the structured
    // review reference.
    let mut review_ref: Option<String> = None;
    if let Some(pointer) = &review_cycle_pointer {
        review_ref = Some(pointer.clone());
    } else if is_approved_reopen(&old_lane, target) && feedback_pointer.is_some() {
        review_ref = feedback_pointer.clone();
    } else if old_lane == "for_review" && matches!(target, "in_progress" | "planned") && args.force
    {
        review_ref = Some("force-override".to_owned());
    } else if let Some(reference) = review_result
        .as_ref()
        .and_then(|result| result.get("reference"))
        .and_then(Value::as_str)
        .filter(|reference| !reference.trim().is_empty())
    {
        review_ref = Some(reference.to_owned());
    } else if matches!(target, "approved" | "done") {
        review_ref = evidence
            .as_ref()
            .and_then(|evidence| evidence.get("review"))
            .and_then(|review| review.get("reference"))
            .and_then(Value::as_str)
            .map(str::to_owned);
    }

    // FR-006: the approval/done hop records its gate-side decision evidence
    // sidecar unconditionally (`_mt_approval_policy_metadata`).
    let mut policy_metadata: Option<Value> = None;
    if matches!(target, "approved" | "done") {
        let mut sidecar = Map::new();
        sidecar.insert("tool".to_owned(), json!(reviewer));
        sidecar.insert(
            "profile".to_owned(),
            args.profile
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        sidecar.insert(
            "model".to_owned(),
            args.model.clone().map(Value::String).unwrap_or(Value::Null),
        );
        if let Some(raw_pid) = args
            .shell_pid
            .as_deref()
            .filter(|pid| !pid.trim().is_empty())
            && let Some(pid) = sk_status::python_int(&Value::String(raw_pid.to_owned()))
        {
            sidecar.insert("shell_pid".to_owned(), json!(pid));
        }
        policy_metadata = Some(Value::Object(sidecar));
    }
    if target == "claimed"
        && let Some(raw_pid) = args
            .shell_pid
            .as_deref()
            .filter(|pid| !pid.trim().is_empty())
    {
        let pid = sk_status::python_int(&Value::String(raw_pid.to_owned())).ok_or_else(|| {
            CompatError::domain(format!(
                "invalid --shell-pid {raw_pid:?}: expected an integer"
            ))
        })?;
        // `_mt_hop_policy_metadata` writes `baseline or ""`: the creation-time
        // baseline needs OS process introspection this surface does not perform,
        // so the pinned empty-string slot is recorded rather than fabricated.
        policy_metadata = Some(json!({
            "shell_pid": pid,
            "shell_pid_created_at": "",
            "agent": acting,
        }));
    }
    if target == "canceled" {
        policy_metadata = None;
    }

    // #5446: a bare forced exit out of `in_review`/`approved` reaches the
    // pipeline with NO synthesized reason, so the pipeline refuses it instead of
    // reading a manufactured note. Everything else gets the canonical default.
    let note_text = args
        .note
        .as_deref()
        .map(str::trim)
        .filter(|note| !note.is_empty())
        .map(str::to_owned);
    let mut reason = note_text.clone();
    if target == "done"
        && let Some(override_reason) = args
            .done_override_reason
            .as_deref()
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
    {
        let override_note = format!("Done override: {override_reason}");
        reason = Some(match note_text {
            Some(note) => format!("{note} | {override_note}"),
            None => override_note,
        });
    }
    let bare_review_exit_force = args.force
        && reason.is_none()
        && review_result.is_none()
        && matches!(old_lane.as_str(), "in_review" | "approved");
    if reason.is_none() && !bare_review_exit_force {
        reason = Some(if args.force {
            format!("Force move to {target}")
        } else {
            format!("move-task: {old_lane} -> {target}")
        });
    }

    let mut request = sk_status::mutation::TransitionRequest::new(
        mission.spec_dir(),
        mission.slug().as_str(),
        &wp_id,
        target,
        actor.clone(),
    );
    request.repo_root = Some(root.to_path_buf());
    request.force = args.force;
    request.reason = reason;
    request.reason_source = if target == "canceled" {
        let operator = args
            .note
            .as_deref()
            .is_some_and(|note| !note.trim().is_empty());
        Some(if operator { "operator" } else { "synthetic" }.to_owned())
    } else {
        None
    };
    request.evidence = evidence;
    request.review_ref = review_ref;
    request.review_result = review_result;
    request.policy_metadata = policy_metadata;
    request.lane_head = probe_lane_head(root, &mission, &wp_id);
    request.subtasks_complete = None;

    let mut annotation = sk_status::mutation::InnerStateDelta::new();
    if let Some(assignee) = args
        .assignee
        .clone()
        .filter(|value| !value.trim().is_empty())
    {
        annotation.assignee = Some(assignee);
    }
    if let Some(note) = args.note.clone() {
        annotation.note = Some(note);
    }
    if !args.tracker_ref.is_empty() {
        annotation.tracker_refs = Some(args.tracker_ref.clone());
    }
    if !annotation.is_empty() {
        request.annotation_delta = Some(annotation);
    }

    // Capture the Mission hold (and its rollback point) across the write, the
    // commit and any rollback. The status writer re-enters the same per-thread
    // lock, so the outer hold stays authoritative.
    let (mission_lock, rollback_point) = match &commit_authority {
        Some(_) => {
            let lock = sk_status::mutation::acquire_mission_lock(
                mission.spec_dir(),
                Some(root),
                sk_kernel::LockWait::Unbounded,
            )
            .map_err(|error| {
                CompatError::domain(format!(
                    "could not acquire the Mission write lock for auto-commit: {error}"
                ))
            })?;
            let point = sk_status::mutation::capture_rollback_point(&lock, mission.spec_dir())
                .map_err(|error| {
                    CompatError::domain(format!(
                        "could not capture the rollback point for auto-commit: {error}"
                    ))
                })?;
            (Some(lock), Some(point))
        }
        None => (None, None),
    };

    // Write the evidence before the transition that references it; a rejection
    // row's `review_ref` must be the durable pointer, not the raw draft path.
    let mut review_cycle_path: Option<PathBuf> = None;
    if let Some((artifact, wp_slug)) = &review_cycle_artifact {
        let directory = sk_status::mutation_commit::review_cycle_dir(mission.spec_dir(), wp_slug);
        let path = artifact.write_new(&directory).map_err(|error| {
            CompatError::domain(format!(
                "could not write the review-cycle artifact: {error}"
            ))
        })?;
        review_cycle_path = Some(path);
    }

    let outcome = match sk_status::mutation::emit_status_transition(&request) {
        Ok(outcome) => outcome,
        Err(error) => {
            // An emit error may occur after replacement or readback. Evidence
            // retirement must honor that mutation boundary, not assume no row.
            let mut evidence_note = String::new();
            if let Some(path) = &review_cycle_path
                && let EvidenceRetirement::Retained(reason) =
                    retire_failed_transition_evidence(commit_authority.as_ref(), path, &error)
            {
                evidence_note = format!(" {reason}.");
            }
            return Err(CompatError::domain(format!(
                "{}{evidence_note}",
                refusal_message(&error)
            )));
        }
    };
    let event = &outcome.event;

    // One path-scoped commit for the status artifacts (transactional topologies)
    // and the review-cycle evidence. A flat mission keeps the primary-uncommitted
    // status write; its evidence still commits when auto-commit is on.
    let mut verdict_outcome: Option<sk_status::mutation_commit::VerdictDurabilityOutcome> = None;
    // The destination this call's status commit landed on, traced even when the
    // edge writes no review-cycle artifact (F4: the resolved destination is the
    // respected current branch, never a stored `target_branch`).
    let mut commit_destination: Option<String> = None;
    if let Some(authority) = &commit_authority {
        let mut paths: Vec<PathBuf> = Vec::new();
        if commit::transactional_topology(&mission) {
            paths.extend(commit::status_artifact_paths(mission.spec_dir()));
        }
        if let Some(path) = &review_cycle_path {
            paths.push(path.clone());
        }
        // An alias-collapse/self-transition no-op writes no rows; with no
        // evidence either there is genuinely nothing to commit.
        if !paths.is_empty() && (outcome.persisted || review_cycle_path.is_some()) {
            let message = match (&review_cycle_artifact, review_cycle_verdict) {
                (Some((artifact, _)), Some(verdict)) => {
                    sk_status::mutation_commit::review_cycle_commit_message(
                        artifact.cycle_number,
                        verdict,
                        &wp_id,
                        mission.slug_str(),
                    )
                }
                _ => sk_status::mutation_commit::status_commit_message(mission.slug_str()),
            };
            match commit::commit_paths(authority, root, &paths, &message) {
                commit::CommitAttempt::Landed => {
                    commit_destination = Some(authority.target_name.clone());
                    if let Some(path) = &review_cycle_path {
                        let evidence_ref = authority
                            .evidence_ref(path)
                            .unwrap_or_else(|| evidence_ref_from_root(root, path));
                        let bytes = std::fs::read(path).unwrap_or_default();
                        if authority.readback_matches(path, &bytes) {
                            verdict_outcome = Some(
                                sk_status::mutation_commit::VerdictDurabilityOutcome::durable(
                                    evidence_ref,
                                    authority.target_name.clone(),
                                ),
                            );
                        } else {
                            verdict_outcome = Some(
                                sk_status::mutation_commit::VerdictDurabilityOutcome::persistence_failed(
                                    Some(evidence_ref),
                                    Some(authority.target_name.clone()),
                                    "destination_readback_mismatch",
                                    "the commit landed, but the exact review-cycle bytes were not verified at the destination ref; the retained files are left in place",
                                ),
                            );
                        }
                    }
                }
                commit::CommitAttempt::NotLanded { error } => {
                    // Only this operation's own, provably uncommitted tail may be
                    // cut. `rollback_status_artifacts` re-checks the multiset,
                    // torn-tail and committed-at-HEAD conditions and leaves every
                    // byte alone on any refusal.
                    let mut rolled_back = false;
                    let mut rollback_note = String::new();
                    if outcome.persisted
                        && let (Some(lock), Some(point)) = (&mission_lock, &rollback_point)
                    {
                        let mut expected = vec![event.event_id.clone()];
                        expected.extend(
                            outcome
                                .annotation
                                .iter()
                                .map(|draft| draft.event_id.clone()),
                        );
                        let probe = StatusHeadCommittedProbe::open(root).ok();
                        let probe_ref: Option<&dyn sk_status::HeadCommittedProbe> = probe
                            .as_ref()
                            .map(|probe| probe as &dyn sk_status::HeadCommittedProbe);
                        match sk_status::mutation::rollback_status_artifacts(
                            lock,
                            point,
                            Some(&expected),
                            probe_ref,
                        ) {
                            Ok(rollback) if rollback.rolled_back => {
                                rolled_back = true;
                                rollback_note =
                                    " the status transition rows were rolled back".to_owned();
                            }
                            Ok(rollback) => {
                                rollback_note = format!(" {}", rollback.message());
                            }
                            Err(rollback_error) => {
                                rollback_note = format!(
                                    " STATUS_ROLLBACK_REFUSED: the rollback could not run ({rollback_error}); inspect the status files"
                                );
                            }
                        }
                    }
                    // The evidence is retired only after the rows are proven
                    // absent/uncommitted AND the destination does not already
                    // carry a blob for it; otherwise a retained row's
                    // `review_ref` would dangle (F1).
                    let mut evidence_note = String::new();
                    if let Some(path) = &review_cycle_path
                        && let EvidenceRetirement::Retained(reason) = retire_uncommitted_evidence(
                            Some(authority),
                            path,
                            !outcome.persisted || rolled_back,
                        )
                    {
                        evidence_note = format!(" {reason}.");
                    }
                    return Err(CompatError::coded(
                        "VERDICT_PERSISTENCE_FAILED",
                        format!(
                            "the commit did not land ({error}); no verdict is durable.{rollback_note}{evidence_note}"
                        ),
                    ));
                }
                commit::CommitAttempt::Indeterminate { error } => {
                    // Possibly landed: never truncate, roll back or delete.
                    return Err(CompatError::coded(
                        "COMMIT_INDETERMINATE",
                        format!(
                            "the status transition was written, but the commit outcome is unknown ({error}); every file was left in place for inspection and no verdict durability is claimed"
                        ),
                    ));
                }
            }
        }
    } else if let Some(path) = &review_cycle_path {
        // `--no-auto-commit`: the evidence is local only, never a fake commit.
        verdict_outcome = Some(
            sk_status::mutation_commit::VerdictDurabilityOutcome::local_only(
                evidence_ref_from_root(root, path),
            ),
        );
    }

    if args.self_review_fallback {
        // The fallback is recorded as its own canonical lifecycle fact, after the
        // lane transition and under the Mission event log
        // (`emit_reviewer_self_approval`).
        let mut lifecycle = sk_status::mutation::LifecycleRequest::new(
            sk_status::mutation::mission_event_log_path(mission.spec_dir()),
            "ReviewerSelfApproval",
            json!({
                "mission_slug": mission.slug().as_str(),
                "wp_id": wp_id,
                "implementing_actor": acting,
                "intended_reviewer": args.intended_reviewer.as_deref().map(str::trim).unwrap_or(""),
                "failure_reason": args.reviewer_failure_reason.as_deref().map(str::trim).unwrap_or(""),
                "fallback_approved": true,
                "timestamp": sk_kernel::clock::now_utc_iso(),
            }),
            &wp_id,
            "WorkPackage",
        );
        lifecycle.mission_slug = Some(mission.slug().as_str().to_owned());
        lifecycle.repo_root = Some(root.to_path_buf());
        lifecycle
            .dedup_keys
            .insert("mission_slug".to_owned(), json!(mission.slug().as_str()));
        lifecycle
            .dedup_keys
            .insert("wp_id".to_owned(), json!(wp_id.clone()));
        lifecycle
            .dedup_keys
            .insert("implementing_actor".to_owned(), json!(acting));
        lifecycle.dedup_keys.insert(
            "intended_reviewer".to_owned(),
            json!(
                args.intended_reviewer
                    .as_deref()
                    .map(str::trim)
                    .unwrap_or("")
            ),
        );
        lifecycle.dedup_keys.insert(
            "failure_reason".to_owned(),
            json!(
                args.reviewer_failure_reason
                    .as_deref()
                    .map(str::trim)
                    .unwrap_or("")
            ),
        );
        sk_status::mutation::persist_lifecycle_event_local(&lifecycle).map_err(|error| {
            CompatError::domain(format!(
                "the lane transition was recorded, but the ReviewerSelfApproval lifecycle event could not be: {error}"
            ))
        })?;
    }

    let path = wp_file(&mission, &wp_id)
        .map(|path| path.display().to_string())
        .unwrap_or_default();

    let mut payload = serde_json::Map::new();
    payload.insert("result".to_owned(), json!("success"));
    payload.insert("transition_applied".to_owned(), json!(true));
    payload.insert("task_id".to_owned(), json!(wp_id));
    payload.insert("old_lane".to_owned(), json!(old_lane));
    payload.insert("new_lane".to_owned(), json!(target));
    payload.insert("path".to_owned(), json!(path));
    payload.insert("event_id".to_owned(), json!(event.event_id));
    payload.insert("work_package_id".to_owned(), json!(wp_id));
    payload.insert("to_lane".to_owned(), json!(target));
    payload.insert(
        "status_events_path".to_owned(),
        json!(events_path(&mission).display().to_string()),
    );
    payload.insert("retired_feedback_draft".to_owned(), Value::Null);
    if let Some(pointer) = review_cycle_pointer.as_ref().or(feedback_pointer.as_ref()) {
        payload.insert("review_feedback".to_owned(), json!(pointer));
    }
    match &verdict_outcome {
        Some(outcome) => {
            payload.insert(
                "verdict_durably_persisted".to_owned(),
                json!(outcome.verdict_durably_persisted),
            );
            payload.insert(
                "durability_classification".to_owned(),
                json!(outcome.classification.as_str()),
            );
            payload.insert("durability_reason".to_owned(), json!(outcome.reason));
            payload.insert("evidence_ref".to_owned(), json!(outcome.evidence_ref));
            payload.insert("destination_ref".to_owned(), json!(outcome.destination_ref));
        }
        None => {
            payload.insert("verdict_durably_persisted".to_owned(), json!(false));
            if let Some(destination) = commit_destination {
                payload.insert("destination_ref".to_owned(), json!(destination));
            }
        }
    }
    let payload = Value::Object(payload);

    if args.json {
        emit_json(&payload)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        println!("✓ Moved {wp_id} from {old_lane} to {target}");
    }
    Ok(payload)
}

/// `agent tasks mark-status` options.
#[derive(Debug, Args)]
pub struct MarkStatusArgs {
    /// Task ID(s) (space separated).
    #[arg(value_name = "TASK_ID", required = true)]
    pub task_ids: Vec<String>,
    /// Status: done/pending/skipped/not_applicable.
    #[arg(long)]
    pub status: String,
    /// Mission slug.
    #[arg(long)]
    pub mission: Option<String>,
    /// Work package containing the subtasks.
    #[arg(long)]
    pub wp: Option<String>,
    /// Agent name (compact tool:model:profile accepted).
    #[arg(long)]
    pub agent: Option<String>,
    /// Record status events when supported (default: from project config).
    ///
    /// Same Typer boolean-pair semantics as `move-task`: bare flags only, last
    /// spelling wins, and the negative option only ever selects `false`.
    #[arg(
        long = "auto-commit",
        overrides_with = "no_auto_commit",
        action = clap::ArgAction::Count
    )]
    pub auto_commit: Option<u8>,
    /// Disable automatic commits.
    #[arg(
        long = "no-auto-commit",
        overrides_with = "auto_commit",
        action = clap::ArgAction::Count
    )]
    pub no_auto_commit: Option<u8>,
    /// Explicit owned single-branch checkout root (unsupported natively).
    #[arg(long = "owned-checkout", value_name = "PATH")]
    pub owned_checkout: Option<std::path::PathBuf>,
    /// Emit JSON.
    #[arg(long)]
    pub json: bool,
}

fn requested_subtask_value(status: &str) -> Result<sk_status::SubtaskValue, CompatError> {
    match status {
        "done" => Ok(sk_status::SubtaskValue::Lane(sk_status::Lane::Done)),
        "pending" => Ok(sk_status::SubtaskValue::Lane(sk_status::Lane::Planned)),
        "skipped" | "not_applicable" => Ok(sk_status::SubtaskValue::Skipped),
        other => Err(CompatError::domain(format!(
            "Invalid status '{other}'. Must be 'done', 'pending', 'skipped', or 'not_applicable'."
        ))),
    }
}

/// `_normalize_task_id_input`: bare ids stay as given; a mission-qualified
/// `<slug>/T001` / `<slug>:T001` reduces to its upper-cased bare id.
fn normalize_task_id_input(raw: &str) -> String {
    let candidate = raw.trim();
    if candidate.is_empty() {
        return candidate.to_owned();
    }
    let Some(index) = candidate.rfind(['/', ':']) else {
        return candidate.to_owned();
    };
    let prefix = &candidate[..index];
    let bare = &candidate[index + 1..];
    let letters = bare.chars().take_while(char::is_ascii_alphabetic).count();
    if prefix.is_empty()
        || letters == 0
        || letters == bare.len()
        || !bare[letters..].chars().all(|c| c.is_ascii_digit())
        || !prefix
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        || !prefix
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return candidate.to_owned();
    }
    bare.to_ascii_uppercase()
}

/// Read the authored `subtasks:` roster of a WP file (block or inline list).
///
/// `Err` is a fail-closed unreadable roster (no frontmatter block or unreadable
/// file): skipping it could hide a second owner. A readable file with no
/// `subtasks:` key authors no roster (legacy WP), exactly like
/// `authored_subtask_roster`'s `SubtaskRosterMissingError` -> empty arm.
fn authored_roster(path: &Path) -> Result<Vec<String>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("{} is unreadable ({error})", path.display()))?;
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return Err(format!("{} has no YAML frontmatter block", path.display()));
    }
    let block: Vec<&str> = lines
        .by_ref()
        .take_while(|line| line.trim() != "---")
        .collect();
    let mut index = 0;
    while index < block.len() {
        if let Some(inline) = block[index].strip_prefix("subtasks:") {
            let inline = inline.trim();
            if inline.starts_with('[') {
                let inner = inline.trim_start_matches('[').trim_end_matches(']');
                return Ok(inner
                    .split(',')
                    .map(|item| item.trim().trim_matches(['"', '\'']).to_owned())
                    .filter(|item| !item.is_empty())
                    .collect());
            }
            let mut entries = Vec::new();
            index += 1;
            while index < block.len() {
                let trimmed = block[index].trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    index += 1;
                    continue;
                }
                let Some(item) = trimmed.strip_prefix("- ") else {
                    break;
                };
                let value = item.trim().trim_matches(['"', '\'']).to_owned();
                if !value.is_empty() {
                    entries.push(value);
                }
                index += 1;
            }
            return Ok(entries);
        }
        index += 1;
    }
    Ok(Vec::new())
}

/// Record subtask status for one or more tasks (`agent tasks mark-status`).
///
/// Canonical completion is event-sourced: one `InnerStateChanged` delta per
/// owning WP (grouped, never one event per task id), exactly as the baseline's
/// `_ms_emit_subtask_state`. `tasks.md` is never written or committed.
pub fn mark_status(root: &Path, args: &MarkStatusArgs) -> Result<Value, CompatError> {
    if let Some(path) = &args.owned_checkout {
        return Err(CompatError::unsupported(
            "OWNED_CHECKOUT_UNAVAILABLE",
            format!(
                "mark-status --owned-checkout {} requires the owned-checkout lifecycle authority, which this native surface does not implement",
                path.display()
            ),
        ));
    }
    let requested = requested_subtask_value(&args.status)?;
    if args.task_ids.is_empty() {
        return Err(CompatError::domain("At least one task ID is required"));
    }
    let mission = require_mission(root, args.mission.as_deref())?;
    refuse_coordination(&mission, "mark-status")?;
    // The project config supplies the default; the explicit flag overrides it.
    // The destination is the respected current branch (F4); a stored
    // `target_branch` is not a precondition.
    let auto_commit = commit::resolve_auto_commit(
        root,
        explicit_auto_commit(args.auto_commit, args.no_auto_commit),
    )?;
    let prepared_authority = commit::prepare(root, &mission, auto_commit)?;
    // F3: the reference commits an annotation only for `single_branch`
    // (`st.resolved_auto_commit and is_single_branch(resolve_topology(...))`).
    // F2: it applies NO protected-primary refusal here — a `single_branch`
    // annotation that would commit on a protected branch degrades to the
    // uncommitted write with the canonical warning (`_annotation_degrades_uncommitted`).
    let annotation_topology = commit::annotation_commit_topology(&mission);
    let protected_degraded_destination = prepared_authority
        .as_ref()
        .filter(|_| annotation_topology)
        .filter(|authority| authority.protected_destination)
        .map(|authority| authority.target_name.clone());
    let commit_authority = if annotation_topology && protected_degraded_destination.is_none() {
        prepared_authority
    } else {
        None
    };
    let explicit_wp = args.wp.as_deref().map(normalize_wp_id).transpose()?;

    // `tasks.md` is the task-roster index this command resolves against. Its
    // presence is required before any read or write (#2816 keeps it read-only).
    let tasks_md = mission.spec_dir().join("tasks.md");
    if !tasks_md.is_file() {
        return Err(CompatError::domain(format!(
            "tasks.md not found: {}",
            tasks_md.display()
        )));
    }

    // Accept bare and mission-qualified ids (`T001`, `<slug>/T001`, `<slug>:T001`).
    let task_ids: Vec<String> = args
        .task_ids
        .iter()
        .map(|raw| normalize_task_id_input(raw))
        .collect();

    // Authored roster ownership, keyed case-insensitively: `t002` and `T002`
    // name the same subtask (the authored map upper-cases every id).
    let packages = super::support::work_packages(&mission)?;
    let mut owners: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for package in &packages {
        let wp = package.id().as_str().to_owned();
        let roster = authored_roster(package.source()).map_err(|error| {
            CompatError::domain(format!(
                "Cannot determine which work package owns the subtasks: {error}"
            ))
        })?;
        for task_id in roster {
            owners
                .entry(task_id.to_ascii_uppercase())
                .or_default()
                .push(wp.clone());
        }
    }

    let snapshot = canonical_snapshot(&mission);
    let recorded = |wp: &str, task_id: &str| -> Option<String> {
        snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.work_packages.get(wp))
            .and_then(|state| state.get("subtasks"))
            .and_then(|value| value.get(task_id))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };

    let mission_slug = mission.slug().as_str();
    let mut results: Vec<Value> = Vec::new();
    let mut updated_by_wp: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut resolved = 0usize;
    for task_id in &task_ids {
        let mut candidates = owners
            .get(&task_id.to_ascii_uppercase())
            .cloned()
            .unwrap_or_default();
        candidates.sort();
        candidates.dedup();
        let owner = match &explicit_wp {
            Some(wp) => {
                if !candidates.iter().any(|candidate| candidate == wp) {
                    let message = if candidates.is_empty() {
                        format!(
                            "{task_id} is not a subtask of {wp} in Mission {mission_slug}; no work package lists it."
                        )
                    } else {
                        format!(
                            "{task_id} is not a subtask of {wp} in Mission {mission_slug} (owners: {}). Re-run with {}.",
                            candidates.join(", "),
                            candidates
                                .iter()
                                .map(|owner| format!("--wp {owner}"))
                                .collect::<Vec<_>>()
                                .join(" or ")
                        )
                    };
                    return Err(CompatError::domain(message));
                }
                Some(wp.clone())
            }
            None => match candidates.as_slice() {
                [] => None,
                [only] => Some(only.clone()),
                many => {
                    return Err(CompatError::domain(format!(
                        "{task_id} is a subtask of {} in Mission {mission_slug}; pass --wp to select one.",
                        many.join(", ")
                    )));
                }
            },
        };
        let Some(wp) = owner else {
            results.push(json!({
                "id": task_id,
                "outcome": "not_found",
                "format": Value::Null,
                "message": format!("{task_id} was not found in any supported task format."),
            }));
            continue;
        };
        resolved += 1;
        let desired = requested.as_str();
        if recorded(&wp, task_id).as_deref() == Some(desired) {
            results.push(json!({
                "id": task_id,
                "outcome": "already_satisfied",
                "format": "authored_roster",
                "message": format!("{task_id} in {wp} is already {desired}; no event written."),
            }));
            continue;
        }
        updated_by_wp
            .entry(wp.clone())
            .or_default()
            .push(task_id.clone());
        results.push(json!({
            "id": task_id,
            "outcome": "updated",
            "format": "authored_roster",
            "message": format!("Recorded {desired} for {task_id} in {wp}."),
        }));
    }

    let summary = json!({
        "updated": results.iter().filter(|row| row["outcome"] == json!("updated")).count(),
        "already_satisfied": results.iter().filter(|row| row["outcome"] == json!("already_satisfied")).count(),
        "not_found": results.iter().filter(|row| row["outcome"] == json!("not_found")).count(),
    });

    // Nothing resolved is a failure, not a success with warnings: a driver
    // keyed on the exit status must not read "nothing recorded" as recorded.
    if resolved == 0 {
        // The contracted failure payload is the result envelope itself (the
        // dispatcher emits `to_json()` on `--json`) with exit class 1.
        return Err(CompatError::result_envelope(
            json!({"results": results, "summary": summary}),
            format!("No task IDs found in tasks.md: {}", task_ids.join(", ")),
        ));
    }

    let acting = args
        .agent
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("user");
    let actor = actor_from_option(acting, "implementer", None, None);

    // The transactional status write commits the annotation rows; the hold and
    // its rollback point cover the write, the commit and any rollback.
    let transactional = commit_authority.is_some();
    let (mission_lock, rollback_point) = if transactional {
        let lock = sk_status::mutation::acquire_mission_lock(
            mission.spec_dir(),
            Some(root),
            sk_kernel::LockWait::Unbounded,
        )
        .map_err(|error| {
            CompatError::domain(format!(
                "could not acquire the Mission write lock for auto-commit: {error}"
            ))
        })?;
        let point = sk_status::mutation::capture_rollback_point(&lock, mission.spec_dir())
            .map_err(|error| {
                CompatError::domain(format!(
                    "could not capture the rollback point for auto-commit: {error}"
                ))
            })?;
        (Some(lock), Some(point))
    } else {
        (None, None)
    };

    let mut applied: Vec<String> = Vec::new();
    for (wp, task_ids) in &updated_by_wp {
        let mut delta = sk_status::mutation::InnerStateDelta::new();
        for task_id in task_ids {
            delta = delta.with_subtask(task_id.clone(), requested);
        }
        let request = sk_status::mutation::AnnotationRequest {
            feature_dir: mission.spec_dir().to_path_buf(),
            wp_id: wp.clone(),
            actor: actor.clone(),
            delta: delta.to_json_value(),
            at: None,
            event_id: None,
            repo_root: Some(root.to_path_buf()),
            lock_wait: None,
        };
        let outcome = sk_status::mutation::emit_inner_state_changed(&request)
            .map_err(|error| CompatError::domain(error.to_string()))?;
        applied.push(outcome.event_id);
        if protected_degraded_destination.is_some() {
            // The reference logs exactly this on the degrade arm.
            eprintln!(
                "Status annotation for {} {} was written but not committed: the mission records directly on a protected branch.",
                mission.slug_str(),
                wp
            );
        }
    }

    if transactional && !applied.is_empty() {
        let Some(authority) = &commit_authority else {
            unreachable!("transactional implies an auto-commit authority");
        };
        let paths = commit::status_artifact_paths(mission.spec_dir());
        let message = sk_status::mutation_commit::status_commit_message(mission.slug_str());
        match commit::commit_paths(authority, root, &paths, &message) {
            commit::CommitAttempt::Landed => {}
            commit::CommitAttempt::NotLanded { error } => {
                let mut rollback_note = String::new();
                if let (Some(lock), Some(point)) = (&mission_lock, &rollback_point) {
                    let probe = StatusHeadCommittedProbe::open(root).ok();
                    let probe_ref: Option<&dyn sk_status::HeadCommittedProbe> = probe
                        .as_ref()
                        .map(|probe| probe as &dyn sk_status::HeadCommittedProbe);
                    match sk_status::mutation::rollback_status_artifacts(
                        lock,
                        point,
                        Some(&applied),
                        probe_ref,
                    ) {
                        Ok(rollback) if rollback.rolled_back => {
                            rollback_note = " the annotation rows were rolled back".to_owned();
                        }
                        Ok(rollback) => rollback_note = format!(" {}", rollback.message()),
                        Err(rollback_error) => {
                            rollback_note = format!(
                                " STATUS_ROLLBACK_REFUSED: the rollback could not run ({rollback_error}); inspect the status files"
                            );
                        }
                    }
                }
                return Err(CompatError::coded(
                    "COMMIT_NOT_LANDED",
                    format!(
                        "the subtask annotation commit did not land ({error}); nothing is reported as committed.{rollback_note}"
                    ),
                ));
            }
            commit::CommitAttempt::Indeterminate { error } => {
                return Err(CompatError::coded(
                    "COMMIT_INDETERMINATE",
                    format!(
                        "the subtask annotation was written, but the commit outcome is unknown ({error}); every file was left in place for inspection"
                    ),
                ));
            }
        }
    }

    let mut payload = json!({"results": results, "summary": summary});
    if let Some(destination) = &protected_degraded_destination {
        // Additive native durability trace for the canonical degrade arm; the
        // reference's only signal here is the warning above.
        let object = payload
            .as_object_mut()
            .expect("the mark-status payload is an object");
        object.insert(
            "durability_classification".to_owned(),
            json!("persistence_failed"),
        );
        object.insert(
            "durability_reason".to_owned(),
            json!(sk_status::mutation_commit::DURABILITY_REASON_PROTECTED_TARGET_BRANCH),
        );
        object.insert("destination_ref".to_owned(), json!(destination));
    }
    if args.json {
        emit_json(&payload)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        let updated = payload["summary"]["updated"].as_u64().unwrap_or(0);
        if updated == 0 {
            println!(
                "✓ Requested status already satisfied for: {}",
                task_ids.join(", ")
            );
        } else {
            println!(
                "✓ Marked {updated} subtask(s) as {} ({} annotation event(s))",
                args.status,
                applied.len()
            );
        }
    }
    Ok(payload)
}

#[cfg(test)]
mod evidence_retirement_tests {
    use std::fs;

    use super::*;
    use sk_status::mutation::MutationError;

    #[test]
    fn failed_emit_retains_evidence_when_the_status_write_may_have_landed() {
        let path = PathBuf::from("status.events.jsonl");
        let failures = [
            MutationError::CommitIndeterminate {
                path: path.clone(),
                detail: "replacement may have landed".to_owned(),
            },
            MutationError::PostCommitVerification {
                path: path.clone(),
                source: Box::new(MutationError::InvalidRow {
                    reason: "readback failed".to_owned(),
                }),
            },
            MutationError::EventMissing {
                path,
                event_id: "already-published-event".to_owned(),
            },
        ];
        for error in failures {
            let temp = tempfile::tempdir().unwrap();
            let evidence = temp.path().join("review-cycle-1.md");
            fs::write(&evidence, "authored review evidence\n").unwrap();
            assert!(
                matches!(
                    retire_failed_transition_evidence(None, &evidence, &error),
                    EvidenceRetirement::Retained(_)
                ),
                "{error}"
            );
            assert_eq!(fs::read(&evidence).unwrap(), b"authored review evidence\n");
        }
        let temp = tempfile::tempdir().unwrap();
        let evidence = temp.path().join("review-cycle-1.md");
        fs::write(&evidence, "uncommitted evidence\n").unwrap();
        let refusal = MutationError::InvalidRow {
            reason: "refused before a write".to_owned(),
        };
        assert!(matches!(
            retire_failed_transition_evidence(None, &evidence, &refusal),
            EvidenceRetirement::Removed
        ));
        assert!(!evidence.exists());
    }
}

#[cfg(test)]
mod flag_tests {
    //! The `--auto-commit/--no-auto-commit` pair is upstream's
    //! `bool | None` Typer option: a value is never accepted, absent means "use
    //! the project default", and the last spelling wins.

    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct MoveCli {
        #[command(flatten)]
        args: MoveTaskArgs,
    }

    fn resolved(parts: &[&str]) -> Option<bool> {
        let parsed = MoveCli::parse_from(parts);
        explicit_auto_commit(parsed.args.auto_commit, parsed.args.no_auto_commit)
    }

    #[test]
    fn the_auto_commit_flags_are_tri_state_and_last_wins() {
        assert_eq!(resolved(&["x", "WP01", "--to", "planned"]), None);
        assert_eq!(
            resolved(&["x", "WP01", "--to", "planned", "--auto-commit"]),
            Some(true)
        );
        assert_eq!(
            resolved(&["x", "WP01", "--to", "planned", "--no-auto-commit"]),
            Some(false)
        );

        // A bare flag must not swallow the following positional/option.
        let before_positional = MoveCli::parse_from([
            "x",
            "--auto-commit",
            "WP01",
            "--to",
            "planned",
            "--mission",
            "034-demo",
        ]);
        assert_eq!(
            explicit_auto_commit(
                before_positional.args.auto_commit,
                before_positional.args.no_auto_commit
            ),
            Some(true)
        );
        assert_eq!(before_positional.args.task_id, "WP01");
        assert_eq!(before_positional.args.mission.as_deref(), Some("034-demo"));

        assert_eq!(
            resolved(&[
                "x",
                "WP01",
                "--to",
                "planned",
                "--auto-commit",
                "--no-auto-commit"
            ]),
            Some(false)
        );
        assert_eq!(
            resolved(&[
                "x",
                "WP01",
                "--to",
                "planned",
                "--no-auto-commit",
                "--auto-commit"
            ]),
            Some(true)
        );
        // Repeats are last-wins (the reference's boolean flag accepts them).
        assert_eq!(
            resolved(&[
                "x",
                "WP01",
                "--to",
                "planned",
                "--auto-commit",
                "--auto-commit"
            ]),
            Some(true)
        );
        assert_eq!(
            resolved(&[
                "x",
                "WP01",
                "--to",
                "planned",
                "--no-auto-commit",
                "--no-auto-commit"
            ]),
            Some(false)
        );
    }

    #[test]
    fn the_auto_commit_flags_reject_every_valued_spelling() {
        for spelling in [
            "--auto-commit=true",
            "--auto-commit=false",
            "--no-auto-commit=true",
            "--no-auto-commit=false",
        ] {
            let parsed = MoveCli::try_parse_from(["x", "WP01", "--to", "planned", spelling]);
            assert!(parsed.is_err(), "{spelling} must be rejected");
        }
    }
}
