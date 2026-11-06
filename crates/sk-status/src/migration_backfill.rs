//! Source-compatible legacy status/runtime migration services.
//!
//! The modern migration wrappers live in `sk-install`; this module owns the
//! domain work they call and nothing that is already canonical elsewhere:
//!
//! * [`read_legacy_runtime`] — the pre-eviction runtime reader (task
//!   frontmatter + `tasks.md` subtask rows). This is the ground truth the
//!   runtime-state backfill compares its reduced snapshot against.
//! * [`stranded_verdict_findings`] / [`backfill_verdict_provenance`] — recover
//!   a terminal `review-cycle-N.md` verdict into the event authority as one
//!   hand-constructed `review_result` event, keyed by a deterministic ULID.
//! * [`restamp_single_branch_with_code_lanes`] — Invariant T-1 repair
//!   (`single_branch` + a code lane → `lanes`), one `meta.json` field.
//!
//! Every write routes through the existing canonical seam: the event log is
//! appended through [`crate::mutation_log::append_rows_atomic`], the snapshot is
//! refreshed through [`crate::mutation_log::materialize_snapshot_file`], and
//! `meta.json` is written atomically under the mission lock. No second reducer,
//! event writer or state engine exists here.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::lane::Lane;
use crate::mutation_delta::{InnerStateDelta, ReviewOverride as DeltaReviewOverride, SubtaskValue};
use crate::mutation_log::{AppendOptions, append_rows_atomic, materialize_snapshot_file};
use crate::replay::{Actor, ReviewOutcome, StatusEvent, StructuredActor};
use crate::snapshot::{Snapshot, read_raw_events, reduce_rows};

/// Actor identity stamped on every backfilled event (`migration:<module>`).
pub const VERDICT_BACKFILL_ACTOR: &str = "migration:verdict_provenance_backfill";

/// Actor identity stamped on runtime-state seeds.
pub const RUNTIME_BACKFILL_ACTOR: &str = "migration:backfill_runtime_state";

// ---------------------------------------------------------------------------
// Legacy runtime reader
// ---------------------------------------------------------------------------

/// One WP's pre-eviction runtime state.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LegacyWpRuntime {
    /// The `WPxx` code.
    pub wp_id: String,
    /// The recorded shell pid.
    pub shell_pid: Option<i64>,
    /// The shell pid creation timestamp.
    pub shell_pid_created_at: Option<String>,
    /// The recorded agent.
    pub agent: Option<String>,
    /// The recorded assignee.
    pub assignee: Option<String>,
    /// The tracker references.
    pub tracker_refs: Vec<String>,
    /// Subtask id → done.
    pub subtasks: BTreeMap<String, bool>,
    /// The review override quartet, when complete.
    pub review: Option<ReviewOverride>,
    /// Every frontmatter key seen.
    pub frontmatter_keys: Vec<String>,
}

/// A complete review-artifact override recorded in WP frontmatter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewOverride {
    /// Override timestamp.
    pub at: String,
    /// Override actor.
    pub actor: String,
    /// Override WP id.
    pub wp_id: String,
    /// Override reason.
    pub reason: String,
}

impl LegacyWpRuntime {
    /// Whether this WP carries any evictable runtime state.
    pub fn has_evictable_state(&self) -> bool {
        self.shell_pid.is_some()
            || self.shell_pid_created_at.is_some()
            || self.agent.is_some()
            || self.assignee.is_some()
            || !self.tracker_refs.is_empty()
            || !self.subtasks.is_empty()
            || self.review.is_some()
    }
}

fn split_frontmatter(text: &str) -> Option<(&str, &str)> {
    let rest = text.strip_prefix("---")?;
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    let close = rest.find("\n---")?;
    Some((&rest[..close], &rest[close + 4..]))
}

fn wp_code(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    if let Some(rest) = stem.strip_prefix("WP") {
        let digits: String = rest
            .chars()
            .scan((), |(), character| {
                character.is_ascii_digit().then_some(character)
            })
            .collect();
        if !digits.is_empty() {
            return format!("WP{digits}");
        }
    }
    stem
}

fn coerce_tracker_refs(value: Option<&Value>) -> Vec<String> {
    match value {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::String(text)) => text
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| match item {
                Value::String(text) => Some(text.trim().to_string()),
                Value::Number(number) => Some(number.to_string()),
                Value::Bool(value) => Some(value.to_string()),
                _ => None,
            })
            .filter(|text| !text.is_empty())
            .collect(),
        Some(_) => Vec::new(),
    }
}

fn review_from_frontmatter(data: &Map<String, Value>, wp_id: &str) -> Option<ReviewOverride> {
    let at = data
        .get("review_artifact_override_at")?
        .as_str()?
        .to_string();
    let actor = data
        .get("review_artifact_override_actor")?
        .as_str()?
        .to_string();
    let reason = data
        .get("review_artifact_override_reason")?
        .as_str()?
        .to_string();
    let override_wp = data
        .get("review_artifact_override_wp_id")
        .and_then(Value::as_str)
        .unwrap_or(wp_id)
        .to_string();
    if at.is_empty() || actor.is_empty() || reason.is_empty() {
        return None;
    }
    Some(ReviewOverride {
        at,
        actor,
        wp_id: override_wp,
        reason,
    })
}

fn is_heading(line: &str) -> bool {
    let hashes = line
        .chars()
        .take_while(|character| *character == '#')
        .count();
    (2..=4).contains(&hashes) && line.chars().nth(hashes).is_some_and(|c| c != '#')
}

fn heading_wp_token(line: &str) -> Option<String> {
    if !is_heading(line) {
        return None;
    }
    let bytes = line.as_bytes();
    let mut index = 0usize;
    while index + 2 < bytes.len() {
        if bytes[index] == b'W' && bytes[index + 1] == b'P' && bytes[index + 2].is_ascii_digit() {
            let start = index;
            let mut end = index + 2;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
            if end - (start + 2) >= 2 {
                return Some(line[start..end].to_string());
            }
            index = end;
            continue;
        }
        index += 1;
    }
    None
}

/// `(task_id, checked)` for `- [ ] T001 …` / `- [x] T001 …` rows.
fn subtask_row(stripped: &str) -> Option<(String, bool)> {
    let bytes = stripped.as_bytes();
    let mut index = 0usize;
    if *bytes.get(index)? != b'-' {
        return None;
    }
    index += 1;
    while bytes.get(index) == Some(&b' ') {
        index += 1;
    }
    if *bytes.get(index)? != b'[' {
        return None;
    }
    index += 1;
    let checked = match bytes.get(index)? {
        b' ' => false,
        b'x' | b'X' => true,
        _ => return None,
    };
    index += 1;
    if *bytes.get(index)? != b']' {
        return None;
    }
    index += 1;
    while bytes.get(index) == Some(&b' ') {
        index += 1;
    }
    if *bytes.get(index)? != b'T' {
        return None;
    }
    index += 1;
    let start = index;
    while bytes.get(index).is_some_and(u8::is_ascii_digit) {
        index += 1;
    }
    if index - start < 3 {
        return None;
    }
    if bytes
        .get(index)
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
    {
        return None;
    }
    Some((format!("T{}", &stripped[start..index]), checked))
}

fn subtasks_from_tasks_md(text: &str, wp_id: &str) -> BTreeMap<String, bool> {
    let mut result = BTreeMap::new();
    let mut in_section = false;
    let mut closed = false;
    let mut fence = false;
    for line in text.split('\n') {
        let stripped = line.trim();
        if stripped.starts_with("```") || stripped.starts_with("~~~") {
            fence = !fence;
            continue;
        }
        if fence {
            continue;
        }
        if let Some(token) = heading_wp_token(line) {
            if token == wp_id && !closed {
                in_section = true;
                continue;
            }
            if in_section {
                closed = true;
                in_section = false;
                continue;
            }
        }
        if in_section && let Some((task, checked)) = subtask_row(stripped) {
            result.insert(task, checked);
        }
    }
    result
}

/// Reconstruct every WP's pre-eviction runtime state (the OLD reader).
pub fn read_legacy_runtime(
    feature_dir: &Path,
) -> Result<BTreeMap<String, LegacyWpRuntime>, String> {
    let tasks_dir = feature_dir.join("tasks");
    if !tasks_dir.is_dir() {
        return Ok(BTreeMap::new());
    }
    let tasks_md = feature_dir.join("tasks.md");
    let tasks_md_text = std::fs::read_to_string(&tasks_md).unwrap_or_default();
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&tasks_dir)
        .map_err(|error| format!("cannot read {}: {error}", tasks_dir.display()))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().is_some_and(|extension| extension == "md")
                && path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("WP"))
        })
        .collect();
    entries.sort();
    let mut out = BTreeMap::new();
    for wp_file in entries {
        let text = std::fs::read_to_string(&wp_file).map_err(|error| {
            format!(
                "cannot read {} for legacy runtime: {error}",
                wp_file.display()
            )
        })?;
        let Some((frontmatter, _body)) = split_frontmatter(&text) else {
            return Err(format!(
                "cannot read {} for legacy runtime: no frontmatter",
                wp_file.display()
            ));
        };
        let data: Map<String, Value> = serde_yaml_ng::from_str(frontmatter).map_err(|error| {
            format!(
                "cannot read {} for legacy runtime: {error}",
                wp_file.display()
            )
        })?;
        let wp_id = wp_code(&wp_file);
        let shell_pid = data.get("shell_pid").and_then(|value| match value {
            Value::Number(number) => number.as_i64(),
            Value::String(text) => text.trim().parse::<i64>().ok(),
            _ => None,
        });
        let shell_pid_created_at = data
            .get("shell_pid_created_at")
            .and_then(Value::as_str)
            .map(str::to_string);
        let agent = data
            .get("agent")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string);
        let assignee = data
            .get("assignee")
            .and_then(Value::as_str)
            .map(str::to_string);
        let runtime = LegacyWpRuntime {
            wp_id: wp_id.clone(),
            shell_pid,
            shell_pid_created_at,
            agent,
            assignee,
            tracker_refs: coerce_tracker_refs(data.get("tracker_refs")),
            subtasks: subtasks_from_tasks_md(&tasks_md_text, &wp_id),
            review: review_from_frontmatter(&data, &wp_id),
            frontmatter_keys: data.keys().cloned().collect(),
        };
        if runtime.has_evictable_state() || !runtime.frontmatter_keys.is_empty() {
            out.insert(wp_id, runtime);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Deterministic ULID
// ---------------------------------------------------------------------------

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// A deterministic 26-char Crockford identifier from `seed`.
pub fn deterministic_ulid(seed: &str) -> String {
    let hex = sk_kernel::content_digest::sha256_hex(seed.as_bytes());
    let mut value = u128::from_str_radix(&hex[..32], 16).expect("sha256 hex prefix");

    let mut characters = [0u8; 26];
    for slot in characters.iter_mut().rev() {
        *slot = CROCKFORD[(value & 31) as usize];
        value >>= 5;
    }
    String::from_utf8(characters.to_vec()).expect("ASCII alphabet")
}

// ---------------------------------------------------------------------------
// Verdict provenance
// ---------------------------------------------------------------------------

/// One stranded-verdict finding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProvenanceFinding {
    /// The WP id.
    pub wp_id: String,
    /// Always `true` for a finding (kept to mirror the Source shape).
    pub has_md_verdict: bool,
    /// Always `false` for a finding.
    pub has_event_slot: bool,
}

/// What one backfill did.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BackfillOutcome {
    /// WPs whose verdict was appended this run.
    pub appended_wp_ids: Vec<String>,
    /// Whether an existing `status.json` was re-materialized.
    pub status_json_refreshed: bool,
    /// The refresh failure, when the append committed but the refresh did not.
    pub refresh_error: Option<String>,
}

fn cycle_number(path: &Path) -> u64 {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    name.strip_prefix("review-cycle-")
        .and_then(|rest| rest.strip_suffix(".md"))
        .and_then(|digits| digits.parse().ok())
        .unwrap_or(0)
}

fn legacy_frontmatter_verdict(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let (frontmatter, _) = split_frontmatter(&text)?;
    let data: Map<String, Value> = serde_yaml_ng::from_str(frontmatter).ok()?;
    data.get("verdict")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn terminal_review_artifact(feature_dir: &Path, wp_id: &str) -> Option<(PathBuf, String, u64)> {
    let tasks_dir = feature_dir.join("tasks");
    if !tasks_dir.is_dir() {
        return None;
    }
    let mut best: Option<(PathBuf, String, u64)> = None;
    for entry in std::fs::read_dir(&tasks_dir).ok()?.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(wp_id) {
            continue;
        }
        let mut cycles: Vec<PathBuf> = std::fs::read_dir(&path)
            .ok()?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("review-cycle-"))
            })
            .collect();
        cycles.sort_by_key(|path| cycle_number(path));
        let Some(terminal) = cycles.last() else {
            continue;
        };
        let Some(verdict) = legacy_frontmatter_verdict(terminal) else {
            continue;
        };
        let number = cycle_number(terminal);
        if best
            .as_ref()
            .is_none_or(|(_, _, best_number)| number >= *best_number)
        {
            best = Some((terminal.clone(), verdict, number));
        }
    }
    best
}

fn discover_wp_ids_with_review_cycles(feature_dir: &Path) -> Vec<String> {
    let tasks_dir = feature_dir.join("tasks");
    let Ok(entries) = std::fs::read_dir(&tasks_dir) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with("WP") {
                return None;
            }
            let digits: String = name
                .trim_start_matches("WP")
                .chars()
                .scan((), |(), character| {
                    character.is_ascii_digit().then_some(character)
                })
                .collect();
            let has_cycle = std::fs::read_dir(entry.path()).ok().is_some_and(|files| {
                files.flatten().any(|file| {
                    file.file_name()
                        .to_string_lossy()
                        .starts_with("review-cycle-")
                })
            });
            has_cycle.then(|| format!("WP{digits}"))
        })
        .collect();
    ids.sort();
    ids
}

fn review_result_slot_present(snapshot: &Snapshot, wp_id: &str) -> bool {
    snapshot
        .work_packages
        .get(wp_id)
        .is_some_and(|state| state.get("review_result").is_some())
}

fn reduce_feature(feature_dir: &Path) -> Option<Snapshot> {
    let rows = read_raw_events(&crate::mutation_log::events_path(feature_dir)).ok()?;
    reduce_rows(&rows).ok()
}

fn event_verdict(artifact_verdict: &str) -> Option<&'static str> {
    match artifact_verdict {
        "approved" => Some("approved"),
        "rejected" => Some("changes_requested"),
        _ => None,
    }
}

/// Pure predicate: every WP with a terminal `.md` verdict and no event slot.
pub fn stranded_verdict_findings(feature_dir: &Path) -> Vec<ProvenanceFinding> {
    let snapshot = reduce_feature(feature_dir);
    let mut findings = Vec::new();
    for wp_id in discover_wp_ids_with_review_cycles(feature_dir) {
        let Some((_, verdict, _)) = terminal_review_artifact(feature_dir, &wp_id) else {
            continue;
        };
        if event_verdict(&verdict).is_none() {
            continue;
        }
        if snapshot
            .as_ref()
            .is_some_and(|snapshot| review_result_slot_present(snapshot, &wp_id))
        {
            continue;
        }
        findings.push(ProvenanceFinding {
            wp_id,
            has_md_verdict: true,
            has_event_slot: false,
        });
    }
    findings
}

fn artifact_fields(path: &Path) -> Option<(String, u64)> {
    let text = std::fs::read_to_string(path).ok()?;
    let (frontmatter, _) = split_frontmatter(&text)?;
    let data: Map<String, Value> = serde_yaml_ng::from_str(frontmatter).ok()?;
    let reviewer = data.get("reviewer_agent")?.as_str()?.to_string();
    if reviewer.is_empty() {
        return None;
    }
    let reviewed_at = data.get("reviewed_at")?.as_str()?.to_string();
    if reviewed_at.is_empty() {
        return None;
    }
    let cycle = data.get("cycle_number")?.as_u64()?;
    if cycle == 0 {
        return None;
    }
    Some((reviewed_at, cycle))
}

fn build_review_result_event(
    feature_dir: &Path,
    wp_id: &str,
    artifact: &Path,
    legacy_verdict: &str,
) -> Option<StatusEvent> {
    let (reviewed_at, cycle) = artifact_fields(artifact)?;
    let text = std::fs::read_to_string(artifact).ok()?;
    let (frontmatter, _) = split_frontmatter(&text)?;
    let data: Map<String, Value> = serde_yaml_ng::from_str(frontmatter).ok()?;
    let reviewer = data.get("reviewer_agent")?.as_str()?.to_string();
    let mission_slug = feature_dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mission_id = std::fs::read_to_string(feature_dir.join("meta.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|meta| {
            meta.get("mission_id")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    let bridged = event_verdict(legacy_verdict)?;
    let reference = format!(
        "review-cycle://{mission_slug}/{}/{}",
        artifact.parent()?.file_name()?.to_string_lossy(),
        artifact.file_name()?.to_string_lossy()
    );
    let seed = format!(
        "{}|{wp_id}|review_result|{legacy_verdict}|{cycle}",
        mission_id.as_deref().unwrap_or(&mission_slug)
    );
    let feedback = (bridged == "changes_requested").then(|| artifact.display().to_string());
    Some(StatusEvent {
        event_id: deterministic_ulid(&seed),
        mission_slug,
        mission_id,
        wp_id: wp_id.to_string(),
        from_lane: Lane::InReview,
        to_lane: if bridged == "changes_requested" {
            Lane::InProgress
        } else {
            Lane::Approved
        },
        at: reviewed_at,
        actor: Actor::Structured(StructuredActor {
            role: Some("system".to_string()),
            profile: None,
            tool: Some(VERDICT_BACKFILL_ACTOR.to_string()),
            model: None,
        }),
        force: false,
        execution_mode: "worktree".to_string(),
        reason: None,
        reason_source: None,
        review_ref: None,
        evidence: None,
        review_result: Some(ReviewOutcome {
            reviewer,
            verdict: bridged.to_string(),
            reference,
            feedback_path: feedback,
        }),
        policy_metadata: None,
    })
}

/// Idempotently backfill `status.events.jsonl` from terminal `.md` verdicts.
pub fn backfill_verdict_provenance(feature_dir: &Path) -> Result<BackfillOutcome, String> {
    let findings = stranded_verdict_findings(feature_dir);
    let mut outcome = BackfillOutcome::default();
    if findings.is_empty() {
        return Ok(outcome);
    }
    let mut events = Vec::new();
    for finding in &findings {
        let Some((artifact, verdict, _)) = terminal_review_artifact(feature_dir, &finding.wp_id)
        else {
            continue;
        };
        let Some(event) =
            build_review_result_event(feature_dir, &finding.wp_id, &artifact, &verdict)
        else {
            continue;
        };
        events.push(event.to_json_value());
        outcome.appended_wp_ids.push(finding.wp_id.clone());
    }
    if events.is_empty() {
        return Ok(outcome);
    }
    let path = crate::mutation_log::events_path(feature_dir);
    let lock = crate::mutation_lock::acquire_mission_lock(
        feature_dir,
        None,
        sk_kernel::LockWait::Bounded(sk_status_lock_timeout()),
    )
    .map_err(|error| error.to_string())?;
    append_rows_atomic(&path, &events, &AppendOptions::default())
        .map_err(|error| error.to_string())?;
    drop(lock);
    if feature_dir
        .join(crate::snapshot::SNAPSHOT_FILENAME)
        .exists()
    {
        match materialize_snapshot_file(feature_dir) {
            Ok(_) => outcome.status_json_refreshed = true,
            Err(error) => outcome.refresh_error = Some(error.to_string()),
        }
    }
    Ok(outcome)
}

fn sk_status_lock_timeout() -> std::time::Duration {
    std::time::Duration::from_secs(30)
}

// ---------------------------------------------------------------------------
// Single-branch topology restamp
// ---------------------------------------------------------------------------

/// One mission's restamp result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestampResult {
    /// The mission slug.
    pub slug: String,
    /// `restamped`, `skip` or `error`.
    pub action: &'static str,
    /// The reason for a skip/error.
    pub reason: Option<String>,
}

/// Whether `feature_dir` is terminal (merged, or every WP is approved/done).
pub fn mission_is_terminal(feature_dir: &Path) -> bool {
    if let Ok(text) = std::fs::read_to_string(feature_dir.join("meta.json"))
        && let Ok(meta) = serde_json::from_str::<Value>(&text)
        && meta.get("merged_at").is_some_and(|value| !value.is_null())
    {
        return true;
    }
    let Ok(text) = std::fs::read_to_string(feature_dir.join(crate::snapshot::SNAPSHOT_FILENAME))
    else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return false;
    };
    let Some(packages) = value.get("work_packages").and_then(Value::as_object) else {
        return false;
    };
    !packages.is_empty()
        && packages.values().all(|state| {
            state
                .get("lane")
                .and_then(Value::as_str)
                .is_some_and(|lane| matches!(lane, "approved" | "done"))
        })
}

fn mission_dirs(repo_root: &Path) -> Vec<PathBuf> {
    let kitty = repo_root.join("kitty-specs");
    let Ok(entries) = std::fs::read_dir(&kitty) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    dirs
}

fn has_code_lane(feature_dir: &Path) -> Result<bool, String> {
    let path = feature_dir.join("lanes.json");
    if !path.exists() {
        return Ok(false);
    }
    let text = std::fs::read_to_string(&path).map_err(|error| error.to_string())?;
    let value: Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    let Some(lanes) = value.get("lanes").and_then(Value::as_array) else {
        return Ok(false);
    };
    Ok(lanes.iter().any(|lane| {
        let name = lane.get("lane_id").and_then(Value::as_str);
        !name.is_some_and(|name| name == "lane-planning" || name == "planning")
    }))
}

/// Re-stamp every `single_branch` mission whose `lanes.json` has a code lane.
pub fn restamp_single_branch_with_code_lanes(
    repo_root: &Path,
    dry_run: bool,
) -> Vec<RestampResult> {
    let mut results = Vec::new();
    for feature_dir in mission_dirs(repo_root) {
        let slug = feature_dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let Ok(text) = std::fs::read_to_string(feature_dir.join("meta.json")) else {
            results.push(RestampResult {
                slug,
                action: "skip",
                reason: Some("no meta.json".to_string()),
            });
            continue;
        };
        let Ok(mut meta) = serde_json::from_str::<Value>(&text) else {
            results.push(RestampResult {
                slug,
                action: "error",
                reason: Some("corrupt meta.json".to_string()),
            });
            continue;
        };
        if meta.get("topology").and_then(Value::as_str) != Some("single_branch") {
            results.push(RestampResult {
                slug,
                action: "skip",
                reason: Some("topology is not single_branch".to_string()),
            });
            continue;
        }
        match has_code_lane(&feature_dir) {
            Err(error) => {
                results.push(RestampResult {
                    slug,
                    action: "skip",
                    reason: Some(format!("lanes.json unreadable: {error}")),
                });
                continue;
            }
            Ok(false) => {
                results.push(RestampResult {
                    slug,
                    action: "skip",
                    reason: Some("no code lanes".to_string()),
                });
                continue;
            }
            Ok(true) => {}
        }
        if mission_is_terminal(&feature_dir) {
            results.push(RestampResult {
                slug,
                action: "skip",
                reason: Some("terminal (archived) mission — nothing runs it".to_string()),
            });
            continue;
        }
        if !dry_run && let Some(object) = meta.as_object_mut() {
            object.insert("topology".to_string(), json!("lanes"));
            let bytes = serde_json::to_vec_pretty(&meta).map_err(|error| error.to_string());
            let Ok(mut bytes) = bytes else {
                results.push(RestampResult {
                    slug,
                    action: "error",
                    reason: Some("cannot serialise meta.json".to_string()),
                });
                continue;
            };
            bytes.push(b'\n');
            // Source `mission_metadata.write_meta` -> `kernel.atomic.atomic_write`.
            let written = sk_kernel::source_writer::write_source(
                sk_kernel::source_writer::WriterOperation::atomic(),
                &feature_dir.join("meta.json"),
                sk_kernel::source_writer::SourcePayload::Bytes(&bytes),
                None,
            );
            if written.is_err() {
                results.push(RestampResult {
                    slug,
                    action: "error",
                    reason: Some("cannot write meta.json".to_string()),
                });
                continue;
            }
        }
        results.push(RestampResult {
            slug,
            action: "restamped",
            reason: None,
        });
    }
    results
}

// ---------------------------------------------------------------------------
// Runtime-state seed / fail-closed verify (Source `backfill_runtime_state`)
// ---------------------------------------------------------------------------
//
// The event-log half of the runtime-state cutover: reconstruct the legacy
// frontmatter/checkbox runtime state as deterministic seed events, then prove
// — fail-closed — that every legacy value survives in its exact seed row.
//
// Nothing here is a second reducer, event writer or snapshot engine: seeds are
// ordinary `StatusEvent`/annotation rows appended through
// [`append_rows_atomic`] under the one Mission lock, the wire payloads are the
// canonical [`crate::replay::StatusEvent::to_json_value`] and
// [`crate::mutation_delta::InnerStateDelta`] shapes, and every reduced read goes
// through [`reduce_rows`]. The `meta.json` `status_phase` flip lives with the
// Mission metadata authority (`sk_mission::migration_rescue`), not here.

/// Snapshot slots the seed `planned -> claimed` carrier populates through its
/// `policy_metadata` sidecar (Source `_CLAIM_SLOTS`).
const CLAIM_SLOTS: [&str; 3] = ["shell_pid", "shell_pid_created_at", "agent"];

/// The concrete `review_artifact_override_*` frontmatter keys (Source
/// `_REVIEW_OVERRIDE_KEYS`), enumerated — never glob-guessed.
const REVIEW_OVERRIDE_KEYS: [&str; 4] = [
    "review_artifact_override_at",
    "review_artifact_override_actor",
    "review_artifact_override_wp_id",
    "review_artifact_override_reason",
];

/// Reduced-snapshot slots that count as "this WP carries runtime state"
/// (Source `_has_snapshot_runtime`).
const SNAPSHOT_RUNTIME_SLOTS: [&str; 13] = [
    "shell_pid",
    "shell_pid_created_at",
    "agent",
    "assignee",
    "tracker_refs",
    "subtasks",
    "review",
    "role",
    "agent_profile",
    "agent_profile_version",
    "model",
    "provider",
    "notes",
];

/// The canonical `status_phase` key the cutover flip writes.
pub const STATUS_PHASE_KEY: &str = "status_phase";

/// The snapshot-authority `status_phase` value (contract step 5).
pub const SNAPSHOT_AUTHORITY_PHASE: &str = "1";

/// What [`backfill_runtime_state`] did for one mission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeBackfillAction {
    /// One or more seeds were appended.
    Wrote,
    /// Nothing to seed, or every seed was already on disk.
    Skip,
    /// An unrecoverable per-mission error.
    Error,
}

impl RuntimeBackfillAction {
    /// The Source action string.
    pub fn as_str(self) -> &'static str {
        match self {
            RuntimeBackfillAction::Wrote => "wrote",
            RuntimeBackfillAction::Skip => "skip",
            RuntimeBackfillAction::Error => "error",
        }
    }
}

/// Per-mission result of [`backfill_runtime_state`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeBackfillOutcome {
    /// The mission slug (directory name).
    pub slug: String,
    /// What the seed phase did.
    pub action: RuntimeBackfillAction,
    /// NEW seed events appended this run (0 on a converged re-run or a dry run).
    pub seeded_count: usize,
    /// Source-shaped explanation for a skip/error.
    pub reason: Option<String>,
    /// Non-fatal per-WP warnings (never-claimed WP skipped, anchor synthesized).
    pub warnings: Vec<String>,
}

/// Fail-closed result of [`verify_runtime_state`].
///
/// `ok` is true only when every legacy-derived deterministic seed is present
/// with its exact payload and the WP/count integrity guards pass. Any
/// `mismatches` entry is terminal: the caller must not advance to the flip.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeVerifyOutcome {
    /// True only when every check passed.
    pub ok: bool,
    /// Number of legacy WPs the run is responsible for seeding.
    pub wp_count: usize,
    /// One human-readable line per divergence.
    pub mismatches: Vec<String>,
}

impl RuntimeVerifyOutcome {
    fn failed(mismatches: Vec<String>) -> Self {
        Self {
            ok: false,
            wp_count: 0,
            mismatches,
        }
    }
}

/// Why the runtime-state seed/verify spine refused to continue.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuntimeStateError {
    /// Verify was asked to run against already-stripped frontmatter, or a seed
    /// ordering time cannot be represented strictly.
    ///
    /// The pinned order is `backfill -> verify(pre-strip) -> cutover -> strip`;
    /// seeing a stripped key the reduced state still carries would make the OLD
    /// reader read empty and yield a vacuous false green, so it is fail-closed.
    Ordering {
        /// The WP the refusal belongs to (`""` for a mission-level refusal).
        wp_id: String,
        /// The exact reason.
        detail: String,
    },
    /// A WP artifact or the event log cannot be read.
    Unreadable(String),
    /// The canonical writer refused the append.
    Store(String),
}

impl RuntimeStateError {
    fn ordering(wp_id: &str, detail: impl Into<String>) -> Self {
        RuntimeStateError::Ordering {
            wp_id: wp_id.to_string(),
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for RuntimeStateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuntimeStateError::Ordering { wp_id, detail } => {
                if wp_id.is_empty() {
                    write!(formatter, "{detail}")
                } else {
                    write!(formatter, "{wp_id}: {detail}")
                }
            }
            RuntimeStateError::Unreadable(detail) | RuntimeStateError::Store(detail) => {
                formatter.write_str(detail)
            }
        }
    }
}

/// The kernel clock's aware-UTC instant.
type Instant = sk_kernel::clock::UtcDateTime<sk_kernel::clock::UtcZone>;

// -- seed identity ----------------------------------------------------------

/// ``(seed_namespace, declared_mission_id)`` from ONE `meta.json` read.
///
/// The two values are deliberately different and must never be conflated
/// (Source `_mission_identity`, #6689):
///
/// * the **namespace** is the deterministic seed-id root: the declared
///   `mission_id`, or the directory name when `meta.json` declares none. A
///   legacy mission without a minted id degrades to its directory name, which
///   is still stable per corpus — all seed determinism requires;
/// * the **declared** value is the envelope `mission_id` the seed rows put on
///   disk, and `None` when `meta.json` declares none. A mission that declares
///   nothing must keep omitting it: the reader re-derives a `None` `mission_id`
///   from the slug, and a written fallback would put an identity on the row
///   that no reader would ever have produced.
fn mission_identity(read_dir: &Path) -> (String, Option<String>) {
    let declared = read_meta_value(read_dir).and_then(|meta| {
        meta.get("mission_id")
            .filter(|value| !value.is_null())
            .map(|value| match value {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            })
            .filter(|value| !value.is_empty())
    });
    let slug = read_dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    (declared.clone().unwrap_or(slug), declared)
}

fn read_meta_value(feature_dir: &Path) -> Option<Value> {
    std::fs::read_to_string(feature_dir.join("meta.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
}

/// The deterministic namespaced seed ULID for one `(wp, field)`.
fn seed_id(seed_namespace: &str, wp_id: &str, field_name: &str) -> String {
    deterministic_ulid(&format!("{seed_namespace}|{wp_id}|{field_name}"))
}

fn seed_actor() -> Actor {
    Actor::Structured(StructuredActor {
        role: Some("system".to_string()),
        profile: None,
        tool: Some(RUNTIME_BACKFILL_ACTOR.to_string()),
        model: None,
    })
}

fn is_our_seed(actor: &Actor) -> bool {
    actor.identity() == RUNTIME_BACKFILL_ACTOR
}

/// Source's absent-slot rule: `value not in (None, "", [], {})`.
fn is_absent(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(text) => text.is_empty(),
        Value::Array(items) => items.is_empty(),
        Value::Object(map) => map.is_empty(),
        _ => false,
    }
}

// -- canonical JSON projections (comparison only) ---------------------------

/// Project a canonical event/annotation row's `actor` to its identity string.
///
/// This is the same projection [`Actor::identity`] performs (tool, then role,
/// then the plain string), used only to tolerate the pre-#6722 plain-string
/// seed actor against today's structured form of the same identity.
fn actor_identity_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Object(map) => map
            .get("tool")
            .and_then(Value::as_str)
            .or_else(|| map.get("role").and_then(Value::as_str))
            .unwrap_or_default()
            .to_string(),
        _ => String::new(),
    }
}

/// `row` with `at` dropped and/or `actor` projected to its identity string.
fn project_row(row: &Value, drop_at: bool, actor_as_identity: bool) -> Value {
    let Value::Object(map) = row else {
        return row.clone();
    };
    let mut projected = map.clone();
    if drop_at {
        projected.remove("at");
    }
    if actor_as_identity && let Some(actor) = projected.get("actor") {
        let identity = Value::String(actor_identity_value(actor));
        projected.insert("actor".to_string(), identity);
    }
    Value::Object(projected)
}

/// Two canonical seed rows describe the same seed, allowing the legitimately
/// shaped older-writer differences: an old (pre-floor) `at` with an otherwise
/// identical payload, and the actor's other shape.
fn same_seed_payload(expected: &Value, actual: &Value) -> bool {
    if expected == actual {
        return true;
    }
    if project_row(expected, true, false) == project_row(actual, true, false) {
        return true;
    }
    project_row(expected, true, true) == project_row(actual, true, true)
}

// -- ordering timestamps ----------------------------------------------------

fn has_explicit_offset(raw: &str) -> bool {
    let Some((_, time_part)) = raw.split_once('T').or_else(|| raw.split_once(' ')) else {
        return false;
    };
    time_part.contains('Z') || time_part.contains('+') || time_part.contains('-')
}

fn parse_ordering_timestamp(raw: &str, wp_id: &str) -> Result<Instant, RuntimeStateError> {
    if !has_explicit_offset(raw) {
        return Err(RuntimeStateError::ordering(
            wp_id,
            format!(
                "cannot represent a strict seed history floor below timezone-naive event timestamp {raw:?}"
            ),
        ));
    }
    sk_kernel::clock::parse_iso(raw).map_err(|_| {
        RuntimeStateError::ordering(
            wp_id,
            format!(
                "cannot represent a strict seed history floor below malformed event timestamp {raw:?}"
            ),
        )
    })
}

/// Render `instant` the way Python's aware-datetime `isoformat()` does.
fn python_isoformat(instant: Instant) -> String {
    if instant.timestamp_subsec_micros() == 0 {
        instant.format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
    } else {
        instant.format("%Y-%m-%dT%H:%M:%S%.6f+00:00").to_string()
    }
}

/// Move `instant` by `offset_micros`, or `None` on overflow.
///
/// The shift goes through `SystemTime` so this crate never names a `chrono`
/// duration type while still using the kernel's one clock authority.
fn shift_instant(instant: Instant, offset_micros: i64) -> Option<Instant> {
    let system: std::time::SystemTime = instant.into();
    let shifted = if offset_micros >= 0 {
        system.checked_add(std::time::Duration::from_micros(
            offset_micros.unsigned_abs(),
        ))?
    } else {
        system.checked_sub(std::time::Duration::from_micros(
            offset_micros.unsigned_abs(),
        ))?
    };
    Some(shifted.into())
}

/// Render `instant` with the same UTC designator `like` carries.
///
/// Preserving the designator is load-bearing: the reducer folds on the RAW
/// `(at, event_id)` string tuple, so every ordering guard compares ISO strings
/// lexically.
fn encode_ordering_timestamp(instant: Instant, like: &str) -> String {
    let encoded = python_isoformat(instant);
    if like.ends_with('Z') && encoded.ends_with("+00:00") {
        return format!("{}Z", &encoded[..encoded.len() - "+00:00".len()]);
    }
    encoded
}

/// Move `raw` one ordering tick earlier, failing closed.
///
/// The postcondition is *lexical* (the reducer's fold key is the raw string).
/// One microsecond satisfies it for `+00:00` stamps, but not for a whole-second
/// `Z` stamp whose predecessor would keep the same second: the shift then
/// escalates to the neighbouring whole second, which differs at or left of the
/// seconds field and therefore dominates any suffix.
fn shift_ordering_timestamp(raw: &str, wp_id: &str) -> Result<String, RuntimeStateError> {
    let overflow = || {
        RuntimeStateError::ordering(
            wp_id,
            format!("cannot represent a strict seed history floor before event timestamp {raw:?}"),
        )
    };
    let parsed = parse_ordering_timestamp(raw, wp_id)?;
    if let Some(tick) = shift_instant(parsed, -1) {
        let encoded = encode_ordering_timestamp(tick, raw);
        if encoded.as_str() < raw {
            return Ok(encoded);
        }
    }
    let truncated =
        sk_kernel::clock::from_epoch(parsed.timestamp() as f64).map_err(|_| overflow())?;
    shift_instant(truncated, -1_000_000)
        .map(|shifted| encode_ordering_timestamp(shifted, raw))
        .ok_or_else(overflow)
}

// -- event-log reads --------------------------------------------------------

fn read_rows(feature_dir: &Path) -> Result<Vec<Value>, RuntimeStateError> {
    let path = crate::mutation_log::events_path(feature_dir);
    crate::snapshot::read_raw_events(&path)
        .map_err(|error| RuntimeStateError::Unreadable(format!("event log unreadable: {error}")))
}

fn parse_rows(rows: &[Value]) -> Result<crate::replay::DiaryStream, RuntimeStateError> {
    crate::replay::parse_diary(rows)
        .map_err(|error| RuntimeStateError::Unreadable(format!("event log unreadable: {error}")))
}

/// Every raw row that is NOT this backfill's own seed.
///
/// "Our own output" is decided by the canonical parser's actor projection, so
/// the filter can never disagree with the reader about what an actor is. A
/// previously written seed is this module's own output from an earlier run:
/// folding it back in would make the next run's seed a function of the last run
/// rather than of the corpus, breaking byte-stable idempotency and silently
/// dissolving the tamper proof (the expectation would vanish the moment the seed
/// it checks exists).
fn authentic_rows(rows: &[Value]) -> Result<Vec<Value>, RuntimeStateError> {
    // One whole-stream parse first: a corrupt row is a terminal read error, not
    // something to filter around.
    parse_rows(rows)?;
    let mut kept = Vec::with_capacity(rows.len());
    for row in rows {
        let stream = parse_rows(std::slice::from_ref(row))?;
        let actor = stream
            .transitions
            .first()
            .map(|event| &event.actor)
            .or_else(|| stream.annotations.first().map(|event| &event.actor));
        if actor.is_some_and(is_our_seed) {
            continue;
        }
        kept.push(row.clone());
    }
    Ok(kept)
}

fn reduce_authentic(rows: &[Value]) -> Result<Snapshot, RuntimeStateError> {
    let kept = authentic_rows(rows)?;
    reduce_rows(&kept)
        .map_err(|error| RuntimeStateError::Unreadable(format!("event log unreadable: {error}")))
}

/// Per-WP earliest *authentic* transition `at`.
fn earliest_transition_ats(stream: &crate::replay::DiaryStream) -> BTreeMap<String, String> {
    let mut earliest: BTreeMap<String, String> = BTreeMap::new();
    for event in stream
        .transitions
        .iter()
        .filter(|event| !is_our_seed(&event.actor))
    {
        earliest
            .entry(event.wp_id.clone())
            .and_modify(|current| {
                if event.at < *current {
                    *current = event.at.clone();
                }
            })
            .or_insert_with(|| event.at.clone());
    }
    earliest
}

/// Each WP's `claimed` anchor from the existing event log, falling back to its
/// earliest transition.
fn claim_anchors(feature_dir: &Path) -> Result<BTreeMap<String, String>, RuntimeStateError> {
    let rows = read_rows(feature_dir)?;
    let stream = parse_rows(&rows)?;
    let earliest = earliest_transition_ats(&stream);
    let mut claimed: BTreeMap<String, String> = BTreeMap::new();
    for event in stream
        .transitions
        .iter()
        .filter(|event| event.to_lane == Lane::Claimed && !is_our_seed(&event.actor))
    {
        claimed
            .entry(event.wp_id.clone())
            .and_modify(|current| {
                if event.at < *current {
                    *current = event.at.clone();
                }
            })
            .or_insert_with(|| event.at.clone());
    }
    Ok(earliest
        .into_iter()
        .map(|(wp_id, earliest_at)| {
            let anchor = claimed.get(&wp_id).cloned().unwrap_or(earliest_at);
            (wp_id, anchor)
        })
        .collect())
}

/// A timestamp strictly below every legitimate (non-seed) event for `wp_id`.
fn wp_history_floor(
    stream: &crate::replay::DiaryStream,
    wp_id: &str,
) -> Result<Option<String>, RuntimeStateError> {
    let mut history: Vec<(&str, &str)> = Vec::new();
    for event in &stream.transitions {
        if event.wp_id == wp_id && !is_our_seed(&event.actor) {
            history.push((event.at.as_str(), event.event_id.as_str()));
        }
    }
    for event in &stream.annotations {
        if event.wp_id == wp_id && !is_our_seed(&event.actor) {
            history.push((event.at.as_str(), event.event_id.as_str()));
        }
    }
    if history.is_empty() {
        return Ok(None);
    }
    for (at, _) in &history {
        parse_ordering_timestamp(at, wp_id)?;
    }
    let earliest_at = history.iter().map(|(at, _)| *at).min().unwrap_or_default();
    let floor = shift_ordering_timestamp(earliest_at, wp_id)?;
    for (at, event_id) in &history {
        if !((floor.as_str(), "") < (*at, *event_id)) {
            return Err(RuntimeStateError::ordering(
                wp_id,
                format!(
                    "cannot represent a strict seed history floor below the reducer key {:?}",
                    (*at, *event_id)
                ),
            ));
        }
    }
    Ok(Some(floor))
}

// -- eligibility ------------------------------------------------------------

impl LegacyWpRuntime {
    /// True when the WP file carries claim state (`agent`/`shell_pid`/`shell_pid_created_at`).
    pub fn has_claim_state(&self) -> bool {
        self.shell_pid.is_some() || self.shell_pid_created_at.is_some() || self.agent.is_some()
    }

    /// True when the WP FILE carries runtime state (claim, assignee, tracker
    /// refs, a completed review) — narrower than
    /// [`Self::has_evictable_state`], which also counts `tasks.md` subtasks.
    pub fn has_frontmatter_runtime(&self) -> bool {
        self.has_claim_state()
            || self.assignee.is_some()
            || !self.tracker_refs.is_empty()
            || self.review.is_some()
    }

    fn claim_slots(&self) -> Vec<(&'static str, Value)> {
        let mut slots = Vec::new();
        if let Some(pid) = self.shell_pid {
            slots.push(("shell_pid", Value::from(pid)));
        }
        if let Some(created_at) = &self.shell_pid_created_at {
            slots.push(("shell_pid_created_at", Value::String(created_at.clone())));
        }
        if let Some(agent) = &self.agent {
            slots.push(("agent", Value::String(agent.clone())));
        }
        slots
    }
}

/// Coerce a legacy timestamp to ISO-8601 UTC, or `None` when unparseable.
///
/// `shell_pid_created_at` is written as raw epoch seconds; a genuine ISO-8601
/// string is tolerated too. Never raises — an unparseable value is a signal to
/// fall back, not an error.
fn parse_epoch_or_iso(raw: Option<&str>) -> Option<String> {
    let text = raw?.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(seconds) = text.parse::<f64>()
        && let Ok(instant) = sk_kernel::clock::from_epoch(seconds)
    {
        return Some(python_isoformat(instant));
    }
    sk_kernel::clock::parse_iso(text).ok().map(python_isoformat)
}

/// Synthesize a deterministic claim anchor from frontmatter, or `None`.
///
/// Used only when the event log carries no transition for this WP at all: a
/// missing/truncated log must not silently drop a real claim. Sources, in order:
/// `shell_pid_created_at` (the process-start baseline), then the mission's
/// `meta.json` `created_at`. `None` when neither yields a timestamp — claim
/// fields with no honest time stay unseeded (no fabricated time).
fn synthesize_claim_anchor(read_dir: &Path, runtime: &LegacyWpRuntime) -> Option<String> {
    if let Some(anchor) = parse_epoch_or_iso(runtime.shell_pid_created_at.as_deref()) {
        return Some(anchor);
    }
    read_meta_value(read_dir).and_then(|meta| {
        meta.get("created_at")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
    })
}

/// `(anchor, was_synthesized)` for one WP.
fn resolve_anchor(
    read_dir: &Path,
    wp_id: &str,
    runtime: &LegacyWpRuntime,
    anchors: &BTreeMap<String, String>,
) -> (Option<String>, bool) {
    if let Some(anchor) = anchors.get(wp_id) {
        return (Some(anchor.clone()), false);
    }
    if !runtime.has_claim_state() {
        return (None, false);
    }
    let synthesized = synthesize_claim_anchor(read_dir, runtime);
    (synthesized.clone(), synthesized.is_some())
}

/// The seed ordering time for one WP: `(anchor, was_synthesized)`.
///
/// This is the migration's eligibility contract, expressed independently of the
/// builder so the verify witness never derives its denominator from the rows the
/// builder emits.
fn resolve_seed_anchor(
    stream: &crate::replay::DiaryStream,
    read_dir: &Path,
    wp_id: &str,
    runtime: &LegacyWpRuntime,
    anchors: &BTreeMap<String, String>,
) -> Result<(Option<String>, bool), RuntimeStateError> {
    if let Some(floor) = wp_history_floor(stream, wp_id)? {
        return Ok((Some(floor), false));
    }
    Ok(resolve_anchor(read_dir, wp_id, runtime, anchors))
}

/// The seed claim transition's `at`, forced strictly before recorded history.
///
/// The seed `planned -> claimed` transition is a *carrier* for pre-eviction
/// claim metadata, never a statement about the WP's current lane. The reducer
/// folds transitions in `(at, event_id)` order and the last one wins, so a seed
/// that ties with — or outlives — the WP's real history would silently regress
/// the reduced lane (a `done` WP reappearing as `claimed`).
fn retro_claim_at(anchor: &str, earliest_at: Option<&str>) -> String {
    let Some(earliest_at) = earliest_at else {
        return anchor.to_string();
    };
    if anchor < earliest_at {
        return anchor.to_string();
    }
    match sk_kernel::clock::parse_iso(earliest_at)
        .ok()
        .and_then(|parsed| shift_instant(parsed, -1))
    {
        Some(shifted) => python_isoformat(shifted),
        None => anchor.to_string(),
    }
}

// -- seed building ----------------------------------------------------------

#[derive(Clone, Debug)]
struct AnnotationSeed {
    event_id: String,
    wp_id: String,
    at: String,
    delta: InnerStateDelta,
}

impl AnnotationSeed {
    /// The canonical `kind: "annotation"` wire row (the one annotation writer's
    /// shape: `event_id`, `kind`, `wp_id`, `at`, `actor`, `delta`).
    fn to_row(&self) -> Value {
        let mut row = Map::new();
        row.insert("event_id".to_string(), Value::String(self.event_id.clone()));
        row.insert("kind".to_string(), Value::String("annotation".to_string()));
        row.insert("wp_id".to_string(), Value::String(self.wp_id.clone()));
        row.insert("at".to_string(), Value::String(self.at.clone()));
        row.insert("actor".to_string(), seed_actor().to_json_value());
        row.insert("delta".to_string(), self.delta.to_json_value());
        Value::Object(row)
    }
}

#[derive(Clone, Debug, Default)]
struct SeedSet {
    transitions: Vec<StatusEvent>,
    annotations: Vec<AnnotationSeed>,
}

fn claim_carrier(
    slug: &str,
    seed_namespace: &str,
    mission_id: Option<&str>,
    wp_id: &str,
    at: &str,
    policy_metadata: Map<String, Value>,
) -> StatusEvent {
    StatusEvent {
        event_id: seed_id(seed_namespace, wp_id, "claim"),
        mission_slug: slug.to_string(),
        mission_id: mission_id.map(str::to_string),
        wp_id: wp_id.to_string(),
        from_lane: Lane::Planned,
        to_lane: Lane::Claimed,
        at: at.to_string(),
        actor: seed_actor(),
        force: false,
        execution_mode: "worktree".to_string(),
        reason: None,
        reason_source: None,
        review_ref: None,
        evidence: None,
        review_result: None,
        policy_metadata: Some(Value::Object(policy_metadata)),
    }
}

/// Claim slot VALUES authentic history already carries, per WP.
///
/// The backfill's contract is "no legacy frontmatter value is lost", not "every
/// legacy value is re-stated": a claim slot the canonical model already holds
/// with the same value has nothing left to migrate, so minting a lane-shaped
/// carrier for it would be pure write amplification.
fn snapshot_claim_slots(
    rows: &[Value],
) -> Result<BTreeMap<String, Map<String, Value>>, RuntimeStateError> {
    let snapshot = reduce_authentic(rows)?;
    let mut slots: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
    for (wp_id, state) in &snapshot.work_packages {
        let mut present = Map::new();
        for slot in CLAIM_SLOTS {
            if let Some(value) = state.get(slot)
                && !is_absent(value)
            {
                present.insert(slot.to_string(), value.clone());
            }
        }
        slots.insert(wp_id.clone(), present);
    }
    Ok(slots)
}

/// The claim `policy_metadata` payload still worth seeding.
fn unmigrated_claim_slots(
    runtime: &LegacyWpRuntime,
    present: Option<&Map<String, Value>>,
) -> Map<String, Value> {
    let mut payload = Map::new();
    for (slot, value) in runtime.claim_slots() {
        if present.and_then(|map| map.get(slot)) != Some(&value) {
            payload.insert(slot.to_string(), value);
        }
    }
    payload
}

fn subtask_values(subtasks: &BTreeMap<String, bool>) -> BTreeMap<String, SubtaskValue> {
    subtasks
        .iter()
        .map(|(task, done)| {
            (
                task.clone(),
                if *done {
                    SubtaskValue::Lane(Lane::Done)
                } else {
                    SubtaskValue::Lane(Lane::Planned)
                },
            )
        })
        .collect()
}

fn push_annotation(
    annotations: &mut Vec<AnnotationSeed>,
    seed_namespace: &str,
    wp_id: &str,
    at: &str,
    field_name: &str,
    delta: InnerStateDelta,
) {
    if delta.is_empty() {
        return;
    }
    annotations.push(AnnotationSeed {
        event_id: seed_id(seed_namespace, wp_id, field_name),
        wp_id: wp_id.to_string(),
        at: at.to_string(),
        delta,
    });
}

/// Build the seed rows for a corpus (Source `_build_seed_events`).
///
/// Two ordering rules compose, and both must hold:
///
/// 1. a shared per-WP floor strictly below the earliest raw reducer key whenever
///    the WP already carries authentic transition *or* annotation history; with
///    no history the claimed/synthesized anchor applies to every seed, so their
///    fold ordering stays deterministic;
/// 2. the retroactive claim pin: the claim *transition*'s `at` is additionally
///    pinned strictly before the WP's earliest recorded transition so a
///    retroactive metadata carrier can never win the reducer fold and regress a
///    finished WP back to `claimed`.
fn build_seed_events(
    feature_dir: &Path,
    read_dir: &Path,
    legacy: &BTreeMap<String, LegacyWpRuntime>,
    anchors: &BTreeMap<String, String>,
    warnings: &mut Vec<String>,
) -> Result<SeedSet, RuntimeStateError> {
    let slug = mission_slug(feature_dir);
    let (seed_namespace, declared_mission_id) = mission_identity(read_dir);
    let rows = read_rows(feature_dir)?;
    let stream = parse_rows(&rows)?;
    let earliest_ats = earliest_transition_ats(&stream);
    let present_claim_slots = snapshot_claim_slots(&rows)?;
    let mut seeds = SeedSet::default();
    for (wp_id, runtime) in legacy {
        let (anchor, synthesized) =
            resolve_seed_anchor(&stream, read_dir, wp_id, runtime, anchors)?;
        let Some(anchor) = anchor else {
            if runtime.has_evictable_state() {
                warnings.push(format!(
                    "{wp_id}: no claim anchor (never-claimed WP) — runtime seed skipped"
                ));
            }
            continue;
        };
        if synthesized {
            warnings.push(format!(
                "{wp_id}: no claim anchor in event log — synthesized from frontmatter claim state"
            ));
        }
        let payload = unmigrated_claim_slots(runtime, present_claim_slots.get(wp_id));
        if !payload.is_empty() {
            let at = retro_claim_at(&anchor, earliest_ats.get(wp_id).map(String::as_str));
            seeds.transitions.push(claim_carrier(
                &slug,
                &seed_namespace,
                declared_mission_id.as_deref(),
                wp_id,
                &at,
                payload,
            ));
        }
        if let Some(assignee) = &runtime.assignee {
            push_annotation(
                &mut seeds.annotations,
                &seed_namespace,
                wp_id,
                &anchor,
                "assignee",
                InnerStateDelta {
                    assignee: Some(assignee.clone()),
                    ..InnerStateDelta::default()
                },
            );
        }
        if !runtime.tracker_refs.is_empty() {
            push_annotation(
                &mut seeds.annotations,
                &seed_namespace,
                wp_id,
                &anchor,
                "tracker_refs",
                InnerStateDelta {
                    tracker_refs: Some(runtime.tracker_refs.clone()),
                    ..InnerStateDelta::default()
                },
            );
        }
        if !runtime.subtasks.is_empty() {
            push_annotation(
                &mut seeds.annotations,
                &seed_namespace,
                wp_id,
                &anchor,
                "subtasks",
                InnerStateDelta {
                    subtasks: Some(subtask_values(&runtime.subtasks)),
                    ..InnerStateDelta::default()
                },
            );
        }
        if let Some(review) = &runtime.review {
            push_annotation(
                &mut seeds.annotations,
                &seed_namespace,
                wp_id,
                &anchor,
                "review",
                InnerStateDelta {
                    review: Some(DeltaReviewOverride {
                        at: review.at.clone(),
                        actor: review.actor.clone(),
                        wp_id: review.wp_id.clone(),
                        reason: review.reason.clone(),
                    }),
                    ..InnerStateDelta::default()
                },
            );
        }
    }
    Ok(seeds)
}

// -- backfill ---------------------------------------------------------------

fn mission_slug(feature_dir: &Path) -> String {
    feature_dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn unreadable_outcome(
    slug: &str,
    warnings: Vec<String>,
    error: &RuntimeStateError,
) -> RuntimeBackfillOutcome {
    RuntimeBackfillOutcome {
        slug: slug.to_string(),
        action: RuntimeBackfillAction::Error,
        seeded_count: 0,
        reason: Some(format!("event log unreadable: {error}")),
        warnings,
    }
}

/// Idempotently seed one mission's frontmatter/checkbox runtime state as events.
///
/// Determinism + idempotency: seed ids are content-namespaced ULIDs and any seed
/// whose id is already on disk is skipped, so a second run seeds nothing. The
/// claim-anchor read, the idempotency read and the single atomic append all run
/// under ONE hold of the canonical Mission status lock.
pub fn backfill_runtime_state(
    feature_dir: &Path,
    dry_run: bool,
) -> Result<RuntimeBackfillOutcome, RuntimeStateError> {
    backfill_runtime_state_with_legs(feature_dir, feature_dir, dry_run)
}

/// Idempotently seed one mission's legacy runtime state, with the Source
/// two-leg partition.
///
/// Source `cutover_mission` threads two directories: `feature_dir` is the
/// PRIMARY leg (the legacy `tasks/` frontmatter read anchor and the seed-id
/// namespace), `status_feature_dir` is the COORD leg the seed **events** are
/// appended against. The two collapse to one directory for a flat /
/// single-branch mission, which is why every single-leg caller is unchanged.
/// This is not a leg swap: only the frontmatter read and the identity read move
/// to `read_dir`; the event write and verify anchor stay on `status_dir`.
pub fn backfill_runtime_state_with_legs(
    read_dir: &Path,
    status_dir: &Path,
    dry_run: bool,
) -> Result<RuntimeBackfillOutcome, RuntimeStateError> {
    backfill_runtime_state_with_legs_and_root(read_dir, status_dir, dry_run, None)
}

/// Idempotently seed one mission's legacy runtime state with the Source owned
/// lock root.
///
/// Source `backfill_runtime_state` takes its Mission status lock with
/// `resolve_status_lock_root(feature_dir, owned.repository_root)` — the OWNED
/// leg's lock namespace is the ownership fact's repository root, not whatever
/// root the mission directory would resolve for itself. This sibling threads
/// that root through to [`sk_status::mutation_lock::acquire_mission_lock`];
/// `None` is the unowned leg and resolves exactly like
/// [`backfill_runtime_state_with_legs`] always has, which is why that wrapper
/// (and every single-leg caller) is unchanged.
///
/// This is the seed half of the one canonical lock-root composition the
/// runtime cutover uses: the flip takes its lock with the same root
/// (`migration_rescue::flip_status_phase`). No second lock model, no second
/// writer, no new refusal.
pub fn backfill_runtime_state_with_legs_and_root(
    read_dir: &Path,
    status_dir: &Path,
    dry_run: bool,
    repo_root: Option<&Path>,
) -> Result<RuntimeBackfillOutcome, RuntimeStateError> {
    let slug = mission_slug(status_dir);
    if !read_dir.join("tasks").is_dir() {
        return Ok(RuntimeBackfillOutcome {
            slug,
            action: RuntimeBackfillAction::Skip,
            seeded_count: 0,
            reason: Some("no tasks/ directory".to_string()),
            warnings: Vec::new(),
        });
    }
    let lock = crate::mutation_lock::acquire_mission_lock(
        status_dir,
        repo_root,
        sk_kernel::LockWait::Bounded(sk_status_lock_timeout()),
    )
    .map_err(|error| RuntimeStateError::Store(error.to_string()))?;
    let outcome = backfill_runtime_state_locked(read_dir, status_dir, &slug, dry_run);
    drop(lock);
    outcome
}

fn backfill_runtime_state_locked(
    read_dir: &Path,
    status_dir: &Path,
    slug: &str,
    dry_run: bool,
) -> Result<RuntimeBackfillOutcome, RuntimeStateError> {
    let mut warnings = Vec::new();
    let legacy = match read_legacy_runtime(read_dir) {
        Ok(legacy) => legacy,
        Err(error) => {
            let error = RuntimeStateError::Unreadable(error);
            return Ok(unreadable_outcome(slug, warnings, &error));
        }
    };
    let anchors = match claim_anchors(status_dir) {
        Ok(anchors) => anchors,
        Err(error) => return Ok(unreadable_outcome(slug, warnings, &error)),
    };
    let seeds = match build_seed_events(status_dir, read_dir, &legacy, &anchors, &mut warnings) {
        Ok(seeds) => seeds,
        Err(error @ RuntimeStateError::Ordering { .. }) => return Err(error),
        Err(error) => return Ok(unreadable_outcome(slug, warnings, &error)),
    };
    let rows = read_rows(status_dir)?;
    let stream = parse_rows(&rows)?;
    let existing: std::collections::BTreeSet<String> = stream
        .transitions
        .iter()
        .map(|event| event.event_id.clone())
        .chain(
            stream
                .annotations
                .iter()
                .map(|event| event.event_id.clone()),
        )
        .collect();
    let new_transitions: Vec<StatusEvent> = seeds
        .transitions
        .into_iter()
        .filter(|event| !existing.contains(&event.event_id))
        .collect();
    let new_annotations: Vec<AnnotationSeed> = seeds
        .annotations
        .into_iter()
        .filter(|seed| !existing.contains(&seed.event_id))
        .collect();
    let seeded_count = new_transitions.len() + new_annotations.len();
    if seeded_count == 0 {
        if !dry_run {
            // Seeds that landed before the snapshot refresh existed leave a stale
            // status.json behind; heal it, warning (never failing) if that fails.
            if let Some(error) = refresh_snapshot_if_present(status_dir) {
                warnings.push(error);
            }
        }
        return Ok(RuntimeBackfillOutcome {
            slug: slug.to_string(),
            action: RuntimeBackfillAction::Skip,
            seeded_count: 0,
            reason: Some("nothing new to seed (idempotent)".to_string()),
            warnings,
        });
    }
    if dry_run {
        return Ok(RuntimeBackfillOutcome {
            slug: slug.to_string(),
            action: RuntimeBackfillAction::Wrote,
            seeded_count,
            reason: Some("dry-run (no write)".to_string()),
            warnings,
        });
    }
    let mut rows_to_append: Vec<Value> = new_transitions
        .iter()
        .map(StatusEvent::to_json_value)
        .collect();
    rows_to_append.extend(new_annotations.iter().map(AnnotationSeed::to_row));
    let path = crate::mutation_log::events_path(status_dir);
    append_rows_atomic(&path, &rows_to_append, &AppendOptions::default())
        .map_err(|error| RuntimeStateError::Store(error.to_string()))?;
    if let Some(error) = refresh_snapshot_if_present(status_dir) {
        warnings.push(error);
    }
    Ok(RuntimeBackfillOutcome {
        slug: slug.to_string(),
        action: RuntimeBackfillAction::Wrote,
        seeded_count,
        reason: None,
        warnings,
    })
}

/// Regenerate a persisted `status.json`; `None` on success, the warning on failure.
fn refresh_snapshot_if_present(feature_dir: &Path) -> Option<String> {
    if !feature_dir
        .join(crate::snapshot::SNAPSHOT_FILENAME)
        .exists()
    {
        return None;
    }
    materialize_snapshot_file(feature_dir)
        .err()
        .map(|error| format!("status.json refresh failed: {error}"))
}

// -- fail-closed verify -----------------------------------------------------

fn has_snapshot_runtime(state: &crate::snapshot::WorkPackageState) -> bool {
    SNAPSHOT_RUNTIME_SLOTS
        .iter()
        .any(|slot| state.get(slot).is_some_and(|value| !is_absent(value)))
}

/// Canonical row for one raw annotation row (delta through the one codec).
fn canonical_annotation_row(row: &Value) -> Option<Value> {
    let object = row.as_object()?;
    let delta = object.get("delta")?;
    let parsed = InnerStateDelta::from_json(delta).ok()?.to_json_value();
    let mut canonical = Map::new();
    canonical.insert("event_id".to_string(), object.get("event_id")?.clone());
    canonical.insert("kind".to_string(), Value::String("annotation".to_string()));
    canonical.insert("wp_id".to_string(), object.get("wp_id")?.clone());
    canonical.insert("at".to_string(), object.get("at")?.clone());
    canonical.insert("actor".to_string(), object.get("actor")?.clone());
    canonical.insert("delta".to_string(), parsed);
    Some(Value::Object(canonical))
}

fn actual_transition_rows(stream: &crate::replay::DiaryStream) -> BTreeMap<String, Value> {
    stream
        .transitions
        .iter()
        .map(|event| (event.event_id.clone(), event.to_json_value()))
        .collect()
}

fn actual_annotation_rows(rows: &[Value]) -> BTreeMap<String, Value> {
    rows.iter()
        .filter(|row| row.get("kind").and_then(Value::as_str) == Some("annotation"))
        .filter_map(|row| {
            let canonical = canonical_annotation_row(row)?;
            let event_id = canonical.get("event_id")?.as_str()?.to_string();
            Some((event_id, canonical))
        })
        .collect()
}

fn seed_field_label(seed: &AnnotationSeed) -> String {
    match seed.delta.to_json_value().as_object() {
        Some(map) => map
            .iter()
            .find(|(_, value)| !value.is_null())
            .map(|(name, _)| name.clone())
            .unwrap_or_else(|| "annotation".to_string()),
        None => "annotation".to_string(),
    }
}

/// Accept an immutable `planned` subtask seed only when real history proves a
/// later legitimate completion.
fn subtask_seed_is_superseded(
    expected: &Value,
    actual: &Value,
    rows: &[Value],
    wp_id: &str,
) -> bool {
    let (Some(expected_subtasks), Some(actual_subtasks)) = (
        expected
            .pointer("/delta/subtasks")
            .and_then(Value::as_object),
        actual.pointer("/delta/subtasks").and_then(Value::as_object),
    ) else {
        return false;
    };
    if expected_subtasks.keys().ne(actual_subtasks.keys()) {
        return false;
    }
    let mut changed = false;
    for (task, expected_status) in expected_subtasks {
        let Some(actual_status) = actual_subtasks.get(task) else {
            return false;
        };
        if actual_status == expected_status {
            continue;
        }
        if actual_status.as_str() != Some("planned") || expected_status.as_str() != Some("done") {
            return false;
        }
        changed = true;
    }
    if !changed {
        return false;
    }
    let Some(seed_instant) = actual
        .get("at")
        .and_then(Value::as_str)
        .and_then(|at| sk_kernel::clock::parse_iso(at).ok())
    else {
        return false;
    };
    let Ok(authentic) = authentic_rows(rows) else {
        return false;
    };
    let mut witnessed = false;
    for row in &authentic {
        if row.get("kind").and_then(Value::as_str) != Some("annotation")
            || row.get("wp_id").and_then(Value::as_str) != Some(wp_id)
        {
            continue;
        }
        let Some(at) = row.get("at").and_then(Value::as_str) else {
            continue;
        };
        let later = matches!(
            sk_kernel::clock::parse_iso(at),
            Ok(instant) if instant > seed_instant
        );
        if later && row.pointer("/delta/subtasks") == expected.pointer("/delta/subtasks") {
            witnessed = true;
            break;
        }
    }
    if !witnessed {
        return false;
    }
    let Ok(snapshot) = reduce_authentic(rows) else {
        return false;
    };
    snapshot
        .work_packages
        .get(wp_id)
        .and_then(|state| state.get("subtasks"))
        == expected.pointer("/delta/subtasks")
}

/// Which frontmatter slots have deterministic seeds (the strip-order guard seat).
fn seeded_frontmatter_slots(
    read_dir: &Path,
    stream: &crate::replay::DiaryStream,
    wp_ids: &std::collections::BTreeSet<String>,
) -> BTreeMap<String, std::collections::BTreeSet<String>> {
    let (seed_namespace, _) = mission_identity(read_dir);
    let transitions: BTreeMap<String, &StatusEvent> = stream
        .transitions
        .iter()
        .map(|event| (event.event_id.clone(), event))
        .collect();
    let annotations: std::collections::BTreeSet<String> = stream
        .annotations
        .iter()
        .map(|event| event.event_id.clone())
        .collect();
    let mut slots: BTreeMap<String, std::collections::BTreeSet<String>> = BTreeMap::new();
    for wp_id in wp_ids {
        let mut found = std::collections::BTreeSet::new();
        if let Some(claim) = transitions.get(&seed_id(&seed_namespace, wp_id, "claim"))
            && let Some(Value::Object(meta)) = claim.policy_metadata.as_ref()
        {
            for slot in CLAIM_SLOTS {
                if meta.contains_key(slot) {
                    found.insert(slot.to_string());
                }
            }
        }
        for (field_name, slot) in [
            ("assignee", "assignee"),
            ("tracker_refs", "tracker_refs"),
            ("review", "review"),
        ] {
            if annotations.contains(&seed_id(&seed_namespace, wp_id, field_name)) {
                found.insert(slot.to_string());
            }
        }
        slots.insert(wp_id.clone(), found);
    }
    slots
}

fn frontmatter_key_for_slot(slot: &str) -> &str {
    if slot == "review" {
        REVIEW_OVERRIDE_KEYS[0]
    } else {
        slot
    }
}

/// Fail-closed proof that OLD-reader values survive in deterministic seeds.
///
/// Rebuilds the expected deterministic rows from the OLD frontmatter/checkbox
/// reader and compares each exact typed payload to the raw event stream. The
/// current reduced snapshot may legitimately be newer (runtime slots are
/// latest-wins), so the no-data-loss proof pins the seed row itself.
pub fn verify_runtime_state(feature_dir: &Path) -> Result<RuntimeVerifyOutcome, RuntimeStateError> {
    verify_runtime_state_with_legs(feature_dir, feature_dir)
}

/// Fail-closed verify with the Source two-leg partition (`read_dir` = PRIMARY,
/// `status_dir` = COORD where the event log and its anchors live).
pub fn verify_runtime_state_with_legs(
    read_dir: &Path,
    status_dir: &Path,
) -> Result<RuntimeVerifyOutcome, RuntimeStateError> {
    let legacy = match read_legacy_runtime(read_dir) {
        Ok(legacy) => legacy,
        Err(error) => {
            return Ok(RuntimeVerifyOutcome::failed(vec![format!(
                "legacy runtime unreadable: {error}"
            )]));
        }
    };
    let rows = match read_rows(status_dir) {
        Ok(rows) => rows,
        Err(error) => return Ok(RuntimeVerifyOutcome::failed(vec![error.to_string()])),
    };
    let stream = match parse_rows(&rows) {
        Ok(stream) => stream,
        Err(error) => return Ok(RuntimeVerifyOutcome::failed(vec![error.to_string()])),
    };
    let snapshot = match reduce_rows(&rows) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return Ok(RuntimeVerifyOutcome::failed(vec![format!(
                "event log unreadable: {error}"
            )]));
        }
    };
    let anchors = claim_anchors(status_dir)?;
    let mut mismatches: Vec<String> = Vec::new();

    // Count parity, DATA-LOSS direction: every seeded WP must carry runtime.
    let mut seeded_wps: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for (wp_id, runtime) in &legacy {
        if runtime.has_evictable_state()
            && resolve_anchor(read_dir, wp_id, runtime, &anchors)
                .0
                .is_some()
        {
            seeded_wps.insert(wp_id.clone());
        }
    }
    let snapshot_runtime_wps: std::collections::BTreeSet<String> = snapshot
        .work_packages
        .iter()
        .filter(|(_, state)| has_snapshot_runtime(state))
        .map(|(wp_id, _)| wp_id.clone())
        .collect();
    for wp_id in seeded_wps.difference(&snapshot_runtime_wps) {
        mismatches.push(format!(
            "{wp_id}: legacy carries runtime state but snapshot has none (count mismatch)"
        ));
    }
    // Reverse direction is tolerant of the already-migrated state, but a
    // snapshot WP with NO legacy row at all is a phantom / injected entry.
    let legacy_wp_ids: std::collections::BTreeSet<String> = legacy.keys().cloned().collect();
    for wp_id in snapshot_runtime_wps.difference(&legacy_wp_ids) {
        mismatches.push(format!(
            "{wp_id}: snapshot carries runtime state but no legacy WP row exists (phantom / injected)"
        ));
    }

    // Every legacy-derived value must exist exactly in its deterministic seed row.
    let mut ignored_warnings = Vec::new();
    let seeds = build_seed_events(
        status_dir,
        read_dir,
        &legacy,
        &anchors,
        &mut ignored_warnings,
    )?;
    let actual_transitions = actual_transition_rows(&stream);
    let actual_annotations = actual_annotation_rows(&rows);
    let (seed_namespace, declared_mission_id) = mission_identity(read_dir);
    for expected in &seeds.transitions {
        let expected_row = expected.to_json_value();
        let actual = actual_transitions.get(&expected.event_id);
        let Some(actual) = actual else {
            mismatches.push(format!(
                "{}: claim mismatch (deterministic seed missing)",
                expected.wp_id
            ));
            continue;
        };
        let mut matched = same_seed_payload(&expected_row, actual);
        if !matched && let Some(runtime) = legacy.get(&expected.wp_id) {
            // Pre-#2985 contract: every non-null legacy slot on one carrier.
            let mut full = Map::new();
            for (slot, value) in runtime.claim_slots() {
                full.insert(slot.to_string(), value);
            }
            let reference = claim_carrier(
                &mission_slug(status_dir),
                &seed_namespace,
                declared_mission_id.as_deref(),
                &expected.wp_id,
                "",
                full,
            )
            .to_json_value();
            matched = project_row(&reference, true, true) == project_row(actual, true, true);
        }
        if !matched {
            mismatches.push(format!(
                "{}: claim mismatch (deterministic seed payload diverged)",
                expected.wp_id
            ));
        }
    }
    for seed in &seeds.annotations {
        let expected_row = seed.to_row();
        let label = seed_field_label(seed);
        let Some(actual) = actual_annotations.get(&seed.event_id) else {
            mismatches.push(format!(
                "{}: {label} mismatch (deterministic seed missing)",
                seed.wp_id
            ));
            continue;
        };
        if same_seed_payload(&expected_row, actual) {
            continue;
        }
        if label == "subtasks"
            && subtask_seed_is_superseded(&expected_row, actual, &rows, &seed.wp_id)
        {
            continue;
        }
        mismatches.push(format!(
            "{}: {label} mismatch (deterministic seed payload diverged)",
            seed.wp_id
        ));
    }

    // Independent claim-slot witness: the denominator comes from the OLD reader
    // plus the eligibility contract, never from the rows the builder emitted.
    let authentic_snapshot = reduce_authentic(&rows)?;
    let present_claim_slots = snapshot_claim_slots(&rows)?;
    for (wp_id, runtime) in &legacy {
        let claim_slots = runtime.claim_slots();
        if claim_slots.is_empty() {
            continue;
        }
        if resolve_seed_anchor(&stream, read_dir, wp_id, runtime, &anchors)?
            .0
            .is_none()
        {
            continue;
        }
        let carrier_slots = unmigrated_claim_slots(runtime, present_claim_slots.get(wp_id));
        let claim = actual_transitions.get(&seed_id(&seed_namespace, wp_id, "claim"));
        let raw: Map<String, Value> = claim
            .and_then(|row| row.get("policy_metadata"))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let legitimate = authentic_snapshot.work_packages.get(wp_id);
        let actual_state = snapshot.work_packages.get(wp_id);
        for (slot, legacy_value) in claim_slots {
            if carrier_slots.contains_key(slot) {
                match claim {
                    None => mismatches.push(format!(
                        "{wp_id}: raw claim-slot witness missing for {slot} (deterministic claim seed absent)"
                    )),
                    Some(_) => {
                        if raw.get(slot) != Some(&legacy_value) {
                            mismatches.push(format!(
                                "{wp_id}: raw claim-slot witness for {slot} diverged"
                            ));
                        }
                    }
                }
            }
            let later_value = legitimate.and_then(|state| state.get(slot));
            let expected_value = later_value.unwrap_or(&legacy_value);
            let observed = actual_state.and_then(|state| state.get(slot));
            if observed != Some(expected_value) {
                let owner = if later_value.is_some() {
                    "later legitimate writer"
                } else {
                    "legacy seed"
                };
                mismatches.push(format!(
                    "{wp_id}: reduced claim-slot witness for {slot} does not match {owner}"
                ));
            }
        }
    }

    // Preserve the strip-order guard using deterministic seed provenance.
    let seeded_slots = seeded_frontmatter_slots(read_dir, &stream, &legacy_wp_ids);
    for wp_id in &legacy_wp_ids {
        let runtime = &legacy[wp_id];
        for slot in seeded_slots.get(wp_id).into_iter().flatten() {
            let key = frontmatter_key_for_slot(slot);
            if !runtime.frontmatter_keys.iter().any(|name| name == key) {
                return Err(RuntimeStateError::ordering(
                    wp_id,
                    format!(
                        "deterministic seed carries {slot:?} but frontmatter key {key:?} is absent — \
                         strip_mutable_fields ran before verify (pinned order is backfill -> verify(pre-strip) -> cutover -> strip)"
                    ),
                ));
            }
        }
    }

    Ok(RuntimeVerifyOutcome {
        ok: mismatches.is_empty(),
        wp_count: seeded_wps.len(),
        mismatches,
    })
}

/// A fast skip hint: does any WP still carry un-migrated legacy runtime state?
///
/// Not the idempotency authority (`verify`/`backfill` are): it short-circuits on
/// an already-authoritative `status_phase` and otherwise falls back to the
/// read-only legacy scan.
///
/// The scan's read failure is RETURNED, never folded into `false`: Source
/// `_mission_needs_cutover` lets `LegacyRuntimeReadError` escape `detect()` and
/// `can_apply()`, so an unreadable WP artifact is a loud error, not "nothing to
/// do" (which would report the corpus as finished).
pub fn runtime_state_needs_cutover(feature_dir: &Path) -> Result<bool, RuntimeStateError> {
    if is_snapshot_authority(feature_dir) {
        return Ok(false);
    }
    read_legacy_runtime(feature_dir)
        .map(|legacy| legacy.values().any(LegacyWpRuntime::has_evictable_state))
        .map_err(RuntimeStateError::Unreadable)
}

/// True when this mission's `meta.json` declares a snapshot-authority phase.
pub fn is_snapshot_authority(feature_dir: &Path) -> bool {
    crate::mutation_mirror::read_status_phase(feature_dir).is_some_and(|phase| phase >= 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_ulid_is_26_crockford_chars_and_stable() {
        let id = deterministic_ulid("mission|WP01|review_result|approved|2");
        assert_eq!(id.len(), 26);
        assert!(
            id.chars()
                .all(|character| character.is_ascii_uppercase() || character.is_ascii_digit())
        );
        assert_eq!(
            id,
            deterministic_ulid("mission|WP01|review_result|approved|2")
        );
        assert_ne!(
            id,
            deterministic_ulid("mission|WP01|review_result|approved|3")
        );
    }

    #[test]
    fn subtask_rows_respect_the_first_wp_heading_and_fences() {
        let text = "## WP01 — one (depends: WP02)\n- [x] T001 done\n- [ ] T002 todo\n### Notes\n```\n- [x] T009 fenced\n```\n## WP02\n- [x] T003 other\n";
        let rows = subtasks_from_tasks_md(text, "WP01");
        assert_eq!(rows.get("T001"), Some(&true));
        assert_eq!(rows.get("T002"), Some(&false));
        assert!(!rows.contains_key("T009"));
        assert!(!rows.contains_key("T003"));
    }
}

#[cfg(test)]
mod legacy_runtime_tests {
    use super::*;

    #[test]
    fn reads_frontmatter_and_subtask_rows() {
        let directory = tempfile::tempdir().expect("temp");
        let mission = directory.path().join("kitty-specs/mission-a");
        std::fs::create_dir_all(mission.join("tasks")).expect("dirs");
        std::fs::write(
            mission.join("tasks/WP01-alpha.md"),
            "---\nshell_pid: 4242\nshell_pid_created_at: '2026-01-01T00:00:00Z'\nagent: claude\nassignee: ana\ntracker_refs: [GH-1, GH-2]\n---\n\nbody\n",
        )
        .expect("wp");
        std::fs::write(
            mission.join("tasks.md"),
            "## WP01 alpha\n- [x] T001 done\n- [ ] T002 todo\n## WP02 beta\n- [x] T003 other\n",
        )
        .expect("tasks");
        let runtime = read_legacy_runtime(&mission).expect("read");
        let wp = runtime.get("WP01").expect("WP01");
        assert_eq!(wp.shell_pid, Some(4242));
        assert_eq!(
            wp.shell_pid_created_at.as_deref(),
            Some("2026-01-01T00:00:00Z")
        );
        assert_eq!(wp.agent.as_deref(), Some("claude"));
        assert_eq!(wp.assignee.as_deref(), Some("ana"));
        assert_eq!(
            wp.tracker_refs,
            vec!["GH-1".to_string(), "GH-2".to_string()]
        );
        assert_eq!(wp.subtasks.get("T001"), Some(&true));
        assert_eq!(wp.subtasks.get("T002"), Some(&false));
        assert!(!wp.subtasks.contains_key("T003"));
        assert!(wp.has_evictable_state());
    }

    #[test]
    fn absent_tasks_directory_is_an_empty_read() {
        let directory = tempfile::tempdir().expect("temp");
        assert!(
            read_legacy_runtime(directory.path())
                .expect("read")
                .is_empty()
        );
    }
}

#[cfg(test)]
mod runtime_state_tests {
    use super::*;

    fn mission(root: &std::path::Path, slug: &str) -> PathBuf {
        let dir = root.join("kitty-specs").join(slug);
        std::fs::create_dir_all(dir.join("tasks")).expect("dirs");
        dir
    }

    fn write(path: &std::path::Path, text: &str) {
        std::fs::write(path, text).expect("write");
    }

    /// The oracle fixture: one claim, assignee, two tracker refs, two subtasks
    /// and a complete review override.
    fn legacy_mission(root: &std::path::Path) -> PathBuf {
        let dir = mission(root, "mission-c");
        write(
            &dir.join("meta.json"),
            "{\n  \"created_at\": \"2026-03-04T05:06:07+00:00\",\n  \"mission_id\": \"01KJ00000000000000000000MC\",\n  \"slug\": \"mission-c\"\n}\n",
        );
        write(
            &dir.join("tasks/WP01-gamma.md"),
            "---\nshell_pid: 4242\nshell_pid_created_at: '2026-03-04T05:00:00+00:00'\nagent: claude\nassignee: ana\ntracker_refs: [GH-1, GH-2]\nreview_artifact_override_at: '2026-03-04T05:30:00+00:00'\nreview_artifact_override_actor: renata\nreview_artifact_override_wp_id: WP01\nreview_artifact_override_reason: manual approval\n---\n\n# WP01 gamma\n",
        );
        write(
            &dir.join("tasks.md"),
            "## WP01 gamma\n- [x] T001 done\n- [ ] T002 todo\n",
        );
        dir
    }

    fn event_rows(dir: &std::path::Path) -> Vec<Value> {
        std::fs::read_to_string(dir.join("status.events.jsonl"))
            .map(|text| {
                text.lines()
                    .map(|line| serde_json::from_str::<Value>(line).expect("row"))
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn seeds_one_claim_carrier_and_four_deterministic_annotations() {
        let temp = tempfile::tempdir().expect("temp");
        let dir = legacy_mission(temp.path());
        let outcome = backfill_runtime_state(&dir, false).expect("seed");
        assert_eq!(outcome.action, RuntimeBackfillAction::Wrote);
        assert_eq!(outcome.seeded_count, 5);
        let rows = event_rows(&dir);
        assert_eq!(rows.len(), 5);
        // The claim carrier is first, annotate order follows Source.
        let expected_ids: Vec<String> = ["claim", "assignee", "tracker_refs", "subtasks", "review"]
            .iter()
            .map(|field| seed_id("01KJ00000000000000000000MC", "WP01", field))
            .collect();
        let ids: Vec<String> = rows
            .iter()
            .map(|row| row["event_id"].as_str().expect("id").to_string())
            .collect();
        assert_eq!(ids, expected_ids);
        let claim = &rows[0];
        assert_eq!(claim["to_lane"], "claimed");
        assert_eq!(claim["mission_id"], "01KJ00000000000000000000MC");
        assert_eq!(claim["at"], "2026-03-04T05:00:00+00:00");
        assert_eq!(
            claim["policy_metadata"],
            json!({
                "agent": "claude",
                "shell_pid": 4242,
                "shell_pid_created_at": "2026-03-04T05:00:00+00:00",
            })
        );
        assert_eq!(
            rows[3]["delta"]["subtasks"],
            json!({"T001": "done", "T002": "planned"})
        );
        assert_eq!(rows[4]["delta"]["review"]["reason"], "manual approval");
    }

    #[test]
    fn verify_is_ok_then_a_second_run_seeds_nothing() {
        let temp = tempfile::tempdir().expect("temp");
        let dir = legacy_mission(temp.path());
        backfill_runtime_state(&dir, false).expect("seed");
        let verify = verify_runtime_state(&dir).expect("verify");
        assert!(verify.ok, "mismatches: {:?}", verify.mismatches);
        assert_eq!(verify.wp_count, 1);
        let before = std::fs::read(dir.join("status.events.jsonl")).expect("read");
        let second = backfill_runtime_state(&dir, false).expect("reseed");
        assert_eq!(second.action, RuntimeBackfillAction::Skip);
        assert_eq!(second.seeded_count, 0);
        assert_eq!(
            std::fs::read(dir.join("status.events.jsonl")).expect("read"),
            before
        );
        // A seeded-but-unflipped mission still needs the cutover: `detect` is a
        // skip hint on the `status_phase` flip, never on the seed count.
        assert!(runtime_state_needs_cutover(&dir).expect("readable"));
    }

    #[test]
    fn the_reduced_snapshot_carries_every_seeded_slot() {
        let temp = tempfile::tempdir().expect("temp");
        let dir = legacy_mission(temp.path());
        backfill_runtime_state(&dir, false).expect("seed");
        let rows = event_rows(&dir);
        let snapshot = reduce_rows(&rows).expect("reduce");
        let wp01 = snapshot.work_packages.get("WP01").expect("WP01");
        assert_eq!(wp01.get("shell_pid"), Some(&json!(4242)));
        assert_eq!(wp01.get("agent"), Some(&json!("claude")));
        assert_eq!(wp01.get("assignee"), Some(&json!("ana")));
        assert_eq!(wp01.get("tracker_refs"), Some(&json!(["GH-1", "GH-2"])));
        assert_eq!(
            wp01.get("subtasks"),
            Some(&json!({"T001": "done", "T002": "planned"}))
        );
        assert_eq!(wp01.get("lane"), Some(&json!("claimed")));
    }

    #[test]
    fn a_diverged_seed_row_is_a_fail_closed_mismatch() {
        let temp = tempfile::tempdir().expect("temp");
        let dir = legacy_mission(temp.path());
        backfill_runtime_state(&dir, false).expect("seed");
        let mut rows = event_rows(&dir);
        rows[0]["policy_metadata"]["shell_pid"] = json!(9999);
        let mut text = String::new();
        for row in &rows {
            text.push_str(&sk_status_json_line(row));
            text.push('\n');
        }
        write(&dir.join("status.events.jsonl"), &text);
        let verify = verify_runtime_state(&dir).expect("verify");
        assert!(!verify.ok);
        assert!(
            verify
                .mismatches
                .iter()
                .any(|mismatch| mismatch
                    == "WP01: claim mismatch (deterministic seed payload diverged)"),
            "mismatches: {:?}",
            verify.mismatches
        );
        assert!(
            verify
                .mismatches
                .iter()
                .any(|mismatch| mismatch == "WP01: raw claim-slot witness for shell_pid diverged")
        );
    }

    fn sk_status_json_line(row: &Value) -> String {
        crate::mutation_json::serialize_event_line(row)
    }

    #[test]
    fn verify_refuses_after_the_frontmatter_was_stripped() {
        let temp = tempfile::tempdir().expect("temp");
        let dir = legacy_mission(temp.path());
        backfill_runtime_state(&dir, false).expect("seed");
        // Simulate `strip_mutable_fields` running before verify.
        write(
            &dir.join("tasks/WP01-gamma.md"),
            "---\nwork_package_id: WP01\n---\n\n# WP01 gamma\n",
        );
        let error = verify_runtime_state(&dir).expect_err("ordering refusal");
        match error {
            RuntimeStateError::Ordering { wp_id, detail } => {
                assert_eq!(wp_id, "WP01");
                assert!(
                    detail.contains("strip_mutable_fields ran before verify"),
                    "{detail}"
                );
            }
            other => panic!("expected an ordering refusal, got {other}"),
        }
    }

    #[test]
    fn seeds_land_strictly_below_existing_history() {
        let temp = tempfile::tempdir().expect("temp");
        let dir = legacy_mission(temp.path());
        // A real transition already recorded at the claim anchor.
        write(
            &dir.join("status.events.jsonl"),
            "{\"at\": \"2026-03-04T05:00:00+00:00\", \"event_id\": \"01KJ0000000000000000000AAA\", \"wp_id\": \"WP01\", \"from_lane\": \"planned\", \"to_lane\": \"in_progress\", \"actor\": \"claude\", \"force\": false, \"execution_mode\": \"worktree\", \"mission_slug\": \"mission-c\", \"reason\": null, \"review_ref\": null, \"evidence\": null}\n",
        );
        backfill_runtime_state(&dir, false).expect("seed");
        let rows = event_rows(&dir);
        let seed_ats: Vec<&str> = rows[1..]
            .iter()
            .filter_map(|row| row.get("at").and_then(Value::as_str))
            .collect();
        assert!(!seed_ats.is_empty());
        let floor = seed_ats[0];
        assert!(floor < "2026-03-04T05:00:00+00:00", "seed at {floor}");
        let verify = verify_runtime_state(&dir).expect("verify");
        assert!(verify.ok, "mismatches: {:?}", verify.mismatches);
    }

    #[test]
    fn a_missing_meta_mission_id_degrades_the_seed_namespace_to_the_slug() {
        let temp = tempfile::tempdir().expect("temp");
        let dir = mission(temp.path(), "mission-no-id");
        write(&dir.join("meta.json"), "{\"slug\": \"mission-no-id\"}\n");
        write(
            &dir.join("tasks/WP01-alpha.md"),
            "---\nshell_pid: 7\nshell_pid_created_at: '1700000000.0'\nagent: claude\n---\n\nbody\n",
        );
        let outcome = backfill_runtime_state(&dir, false).expect("seed");
        assert_eq!(outcome.seeded_count, 1);
        let rows = event_rows(&dir);
        assert_eq!(
            rows[0]["event_id"],
            seed_id("mission-no-id", "WP01", "claim")
        );
        // The namespace fell back to the slug, but the envelope stays absent:
        // a written fallback would put an identity on the row no reader produces.
        assert!(rows[0].get("mission_id").is_none());
        assert_eq!(rows[0]["at"], "2023-11-14T22:13:20+00:00");
    }
}
