//! Status transition preparation, emission and off-axis annotations.
//!
//! This is the native flat/primary composition shell over the pinned baseline
//! `status.emit.emit_status_transition` + `status.transition_pipeline.prepare_transition`:
//!
//! 1. canonicalise the feature directory (a linked worktree path is rewritten to
//!    the main checkout's `kitty-specs/<slug>` unless it is a registered
//!    coordination worktree);
//! 2. take the Mission status lock (reentrant, `.holder` sidecar);
//! 3. read `meta.json` for the canonical `mission_id` (absent for legacy
//!    missions);
//! 4. read and reduce the log **once** — the WP's current lane and the
//!    implementation-evidence/subtask gates come from that one read;
//! 5. resolve the alias, run the review-gate inference, refuse an unnoted forced
//!    review-lane exit, build done evidence, and call the crate's single
//!    [`crate::transition::validate_transition`];
//! 6. append the event (and any batch members) as one verified whole-file
//!    replacement, verify its readback, then materialize `status.json`.
//!
//! Deliberate differences from the baseline, all recorded in `WRITER-REPORT.md`:
//!
//! - no `policy_metadata.lane_head` stamp (Git probe, and this crate never runs
//!   Git);
//! - no phase-1 frontmatter `lane` mirror (a user-authored Markdown rewrite);
//! - no fan-out (the transport seam is a separate owner);
//! - `materialize` failures are reported in [`EmitOutcome::materialize_error`]
//!   instead of being only logged — the persisted event remains authoritative,
//!   exactly as the baseline decides.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use sk_kernel::locks::LockWait;

use crate::guard::{self, DoneEvidence, GuardContext, ReviewApproval, ReviewResult};
use crate::lane::Lane;
use crate::mutation::MutationError;
use crate::mutation_lock::{acquire_mission_lock, canonicalize_feature_dir};
use crate::mutation_log::{
    AppendOptions, MaterializeOutcome, append_rows_atomic, events_path, materialize_snapshot_file,
    read_log_rows, verify_event_rows,
};
use crate::mutation_ulid::{is_ulid, new_ulid};
use crate::replay::{DiaryStream, parse_diary, reduce_diary};
use crate::snapshot::META_FILENAME;

/// Default bounded wait for an ordinary status write (baseline 10 s).
pub const DEFAULT_LOCK_WAIT_SECONDS: u64 = 10;

/// The `--force` review-lane note requirement (baseline text).
const FORCE_NOTE_REQUIRED: &str =
    "--force requires a non-blank --note explaining why the review lane is being left";
/// The operator spelling of the note requirement.
const FORCE_NOTE_HINT: &str = "`--force --note <why>`";

/// A status actor: a plain identity, or the four-key resolved binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActorValue {
    /// A plain scalar identity (every pre-#6400 event).
    Identity(String),
    /// The `{role, profile, tool, model}` resolved binding.
    Structured {
        role: Option<String>,
        profile: Option<String>,
        tool: Option<String>,
        model: Option<String>,
    },
}

impl ActorValue {
    /// A plain identity.
    pub fn identity(value: impl Into<String>) -> Self {
        ActorValue::Identity(value.into())
    }

    /// The display identity used by guards: `tool`, then `role`, then `""`.
    pub fn display_identity(&self) -> String {
        match self {
            ActorValue::Identity(value) => value.clone(),
            ActorValue::Structured { role, tool, .. } => {
                tool.clone().or_else(|| role.clone()).unwrap_or_default()
            }
        }
    }

    /// The wire value: a string or the four-key object.
    pub fn to_json_value(&self) -> Value {
        match self {
            ActorValue::Identity(value) => Value::String(value.clone()),
            ActorValue::Structured {
                role,
                profile,
                tool,
                model,
            } => json!({
                "role": role,
                "profile": profile,
                "tool": tool,
                "model": model,
            }),
        }
    }

    /// Decode a wire actor value (string, or exactly the four-key object).
    pub fn from_json(value: &Value) -> Result<Self, MutationError> {
        match value {
            Value::String(text) => Ok(ActorValue::Identity(text.clone())),
            Value::Object(object) => {
                let allowed = ["role", "profile", "tool", "model"];
                if object.len() != allowed.len()
                    || !allowed.iter().all(|key| object.contains_key(*key))
                {
                    return Err(MutationError::InvalidActor {
                        reason: format!(
                            "structured actor must contain exactly [\"model\", \"profile\", \"role\", \"tool\"]; got {:?}",
                            object.keys().collect::<Vec<_>>()
                        ),
                    });
                }
                let field = |name: &str| -> Result<Option<String>, MutationError> {
                    match object.get(name) {
                        None | Some(Value::Null) => Ok(None),
                        Some(Value::String(text)) => Ok(Some(text.clone())),
                        Some(_) => Err(MutationError::InvalidActor {
                            reason: format!(
                                "structured actor fields must be strings or null; {name:?} was not"
                            ),
                        }),
                    }
                };
                Ok(ActorValue::Structured {
                    role: field("role")?,
                    profile: field("profile")?,
                    tool: field("tool")?,
                    model: field("model")?,
                })
            }
            _ => Err(MutationError::InvalidActor {
                reason: "an actor must be a string or a structured object".to_owned(),
            }),
        }
    }
}

/// One requested lane transition.
#[derive(Clone, Debug)]
pub struct TransitionRequest {
    pub feature_dir: PathBuf,
    pub mission_slug: String,
    /// Canonical ULID identity; `None` reads it from `meta.json`.
    pub mission_id: Option<String>,
    pub wp_id: String,
    pub to_lane: String,
    pub actor: ActorValue,
    pub force: bool,
    pub reason: Option<String>,
    pub reason_source: Option<String>,
    /// Raw done evidence (`{"review": {...}, "repos": [...], "verification": [...]}`).
    pub evidence: Option<Value>,
    pub review_ref: Option<String>,
    pub workspace_context: Option<String>,
    pub subtasks_complete: Option<bool>,
    pub implementation_evidence_present: Option<bool>,
    pub execution_mode: String,
    pub repo_root: Option<PathBuf>,
    pub policy_metadata: Option<Value>,
    pub review_result: Option<Value>,
    pub current_actor: Option<String>,
    /// In-lock dependency verdict; `None` fails open, as the baseline guard does.
    pub dependency_ready: Option<bool>,
    /// Explicit producer timestamp (fixtures pin this).
    pub at: Option<String>,
    /// Explicit event id (fixtures pin this).
    pub event_id: Option<String>,
    /// Synthesize `<execution_mode>:<root>` for a missing workspace context.
    pub default_workspace_context: bool,
    /// Lock wait policy; `None` uses the bounded 10 s default.
    pub lock_wait: Option<LockWait>,
}

impl TransitionRequest {
    /// A request with the baseline defaults filled in.
    pub fn new(
        feature_dir: impl Into<PathBuf>,
        mission_slug: impl Into<String>,
        wp_id: impl Into<String>,
        to_lane: impl Into<String>,
        actor: ActorValue,
    ) -> Self {
        Self {
            feature_dir: feature_dir.into(),
            mission_slug: mission_slug.into(),
            mission_id: None,
            wp_id: wp_id.into(),
            to_lane: to_lane.into(),
            actor,
            force: false,
            reason: None,
            reason_source: None,
            evidence: None,
            review_ref: None,
            workspace_context: None,
            subtasks_complete: None,
            implementation_evidence_present: None,
            execution_mode: "worktree".to_owned(),
            repo_root: None,
            policy_metadata: None,
            review_result: None,
            current_actor: None,
            dependency_ready: None,
            at: None,
            event_id: None,
            default_workspace_context: true,
            lock_wait: None,
        }
    }

    fn wait(&self) -> LockWait {
        self.lock_wait
            .unwrap_or(LockWait::Bounded(std::time::Duration::from_secs(
                DEFAULT_LOCK_WAIT_SECONDS,
            )))
    }
}

/// The exact `StatusEvent.to_dict()` wire shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusEventDraft {
    pub event_id: String,
    pub mission_slug: String,
    pub mission_id: Option<String>,
    pub wp_id: String,
    pub from_lane: Lane,
    pub to_lane: Lane,
    pub at: String,
    pub actor: ActorValue,
    pub force: bool,
    pub execution_mode: String,
    pub reason: Option<String>,
    pub reason_source: Option<String>,
    pub review_ref: Option<String>,
    /// Canonical done evidence, or `None`.
    pub evidence: Option<Value>,
    /// Canonical review result, or `None`.
    pub review_result: Option<Value>,
    pub policy_metadata: Option<Value>,
}

impl StatusEventDraft {
    /// Render the row exactly as the baseline `StatusEvent.to_dict()` does.
    pub fn to_json_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("event_id".to_owned(), Value::String(self.event_id.clone()));
        map.insert(
            "mission_slug".to_owned(),
            Value::String(self.mission_slug.clone()),
        );
        map.insert("wp_id".to_owned(), Value::String(self.wp_id.clone()));
        map.insert(
            "from_lane".to_owned(),
            Value::String(self.from_lane.as_str().to_owned()),
        );
        map.insert(
            "to_lane".to_owned(),
            Value::String(self.to_lane.as_str().to_owned()),
        );
        map.insert("at".to_owned(), Value::String(self.at.clone()));
        map.insert("actor".to_owned(), self.actor.to_json_value());
        map.insert("force".to_owned(), Value::Bool(self.force));
        map.insert(
            "execution_mode".to_owned(),
            Value::String(self.execution_mode.clone()),
        );
        map.insert(
            "reason".to_owned(),
            self.reason
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        map.insert(
            "review_ref".to_owned(),
            self.review_ref
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        map.insert(
            "evidence".to_owned(),
            self.evidence.clone().unwrap_or(Value::Null),
        );
        map.insert(
            "policy_metadata".to_owned(),
            self.policy_metadata.clone().unwrap_or(Value::Null),
        );
        if let Some(result) = &self.review_result {
            map.insert("review_result".to_owned(), result.clone());
        }
        if let Some(reason_source) = &self.reason_source {
            map.insert(
                "reason_source".to_owned(),
                Value::String(reason_source.clone()),
            );
        }
        if let Some(mission_id) = &self.mission_id {
            map.insert("mission_id".to_owned(), Value::String(mission_id.clone()));
        }
        Value::Object(map)
    }
}

/// The pipeline's value-level result.
#[derive(Clone, Debug)]
pub struct PreparedTransition {
    /// `None` is the alias-collapse no-op arm: nothing is appended.
    pub event: Option<StatusEventDraft>,
    pub resolved_lane: Lane,
    pub mirror_frontmatter_lane: bool,
}

/// The outcome of one emit.
#[derive(Clone, Debug)]
pub struct EmitOutcome {
    /// The event (persisted, or the synthetic alias-collapse value).
    pub event: StatusEventDraft,
    /// Whether the event was appended to the log.
    pub persisted: bool,
    /// The log path.
    pub log_path: PathBuf,
    /// The snapshot written after a successful append.
    pub snapshot: Option<MaterializeOutcome>,
    /// A materialization failure, recorded without failing the emit (baseline
    /// keeps the persisted event authoritative).
    pub materialize_error: Option<String>,
    /// The lock file the write ran under.
    pub lock_path: PathBuf,
    /// The Mission lock key.
    pub mission_lock_key: String,
}

/// Read the mission event stream (transitions + annotations), failing closed on
/// a malformed row.
pub fn read_event_stream(feature_dir: &Path) -> Result<DiaryStream, MutationError> {
    let path = events_path(feature_dir);
    let rows = read_log_rows(&path)?;
    parse_diary(&rows).map_err(|error| MutationError::InvalidRow {
        reason: format!("{}: {error}", path.display()),
    })
}

/// The WP's current lane from the reduced log; an absent WP is `genesis`.
pub fn derive_from_lane(feature_dir: &Path, wp_id: &str) -> Result<Lane, MutationError> {
    let stream = read_event_stream(feature_dir)?;
    Ok(derive_from_lane_from_stream(&stream, wp_id))
}

fn derive_from_lane_from_stream(stream: &DiaryStream, wp_id: &str) -> Lane {
    let snapshot = reduce_diary(&stream.transitions, &stream.annotations);
    snapshot
        .work_packages
        .get(wp_id)
        .and_then(|state| state.lane())
        .unwrap_or(Lane::Genesis)
}

/// Build and validate a transition from an already-read stream (pure apart from
/// the WP-file read the subtask gate performs, exactly like the baseline).
pub fn prepare_transition(
    request: &TransitionRequest,
    feature_dir: &Path,
    mission_slug: &str,
    mission_id: Option<&str>,
    from_lane: Lane,
    stream: &DiaryStream,
) -> Result<PreparedTransition, MutationError> {
    if request.wp_id.trim().is_empty() || request.to_lane.trim().is_empty() {
        return Err(MutationError::InvalidRequest {
            reason: "each transition requires wp_id and to_lane".to_owned(),
        });
    }
    let raw_to_lane = request.to_lane.trim().to_lowercase();
    let resolved_lane =
        Lane::parse_input(&request.to_lane).map_err(crate::transition::TransitionError::from)?;

    let mut workspace_context = request.workspace_context.clone();
    if workspace_context.is_none() && request.default_workspace_context {
        let context_root = request.repo_root.as_deref().unwrap_or(feature_dir);
        workspace_context = Some(format!(
            "{}:{}",
            request.execution_mode,
            context_root.display()
        ));
    }

    let review_handoff = from_lane == Lane::InProgress && resolved_lane == Lane::ForReview;
    let mut subtasks_complete = request.subtasks_complete;
    if !request.force && review_handoff {
        subtasks_complete = Some(match subtasks_complete {
            Some(value) => value,
            None => infer_subtasks_complete(feature_dir, &request.wp_id, stream)?,
        });
    }
    let implementation_evidence_present = match request.implementation_evidence_present {
        Some(value) => Some(value),
        None if review_handoff => Some(
            stream
                .transitions
                .iter()
                .any(|event| event.wp_id == request.wp_id),
        ),
        None => None,
    };

    if raw_to_lane != resolved_lane.as_str() && resolved_lane == from_lane {
        return Ok(PreparedTransition {
            event: None,
            resolved_lane,
            mirror_frontmatter_lane: true,
        });
    }

    let actor_identity = request.actor.display_identity();
    refuse_unnoted_review_exit(request, from_lane, &actor_identity)?;

    let evidence = match &request.evidence {
        Some(raw) => Some(build_done_evidence(raw)?),
        None => None,
    };
    let review_result = match &request.review_result {
        Some(raw) => Some(build_review_result(raw)?),
        None => None,
    };

    let guard_evidence = evidence.as_ref().map(guard_done_evidence);
    let guard_review_result = review_result.as_ref().map(|value| ReviewResult {
        reviewer: string_field(value, "reviewer").unwrap_or_default(),
        verdict: string_field(value, "verdict").unwrap_or_default(),
        reference: string_field(value, "reference").unwrap_or_default(),
        feedback_path: string_field(value, "feedback_path"),
    });
    let context = GuardContext {
        actor: Some(actor_identity.clone()),
        workspace_context,
        subtasks_complete,
        implementation_evidence_present,
        reason: request.reason.clone(),
        review_ref: request.review_ref.clone(),
        evidence: guard_evidence,
        force: request.force,
        review_result: guard_review_result,
        current_actor: request.current_actor.clone(),
        dependency_ready: request.dependency_ready,
    };
    crate::transition::validate_transition(from_lane, resolved_lane, &context)?;

    let event_id = match &request.event_id {
        Some(value) => {
            if !is_ulid(value) {
                return Err(MutationError::InvalidRow {
                    reason: format!("event_id {value:?} is not a canonical ULID"),
                });
            }
            value.clone()
        }
        None => new_ulid(),
    };
    let event = StatusEventDraft {
        event_id,
        mission_slug: mission_slug.to_owned(),
        mission_id: mission_id.map(str::to_owned),
        wp_id: request.wp_id.clone(),
        from_lane,
        to_lane: resolved_lane,
        at: request
            .at
            .clone()
            .unwrap_or_else(sk_kernel::clock::now_utc_iso),
        actor: request.actor.clone(),
        force: request.force,
        execution_mode: request.execution_mode.clone(),
        reason: request.reason.clone(),
        reason_source: request.reason_source.clone(),
        review_ref: request.review_ref.clone(),
        evidence,
        review_result,
        policy_metadata: request.policy_metadata.clone(),
    };
    Ok(PreparedTransition {
        event: Some(event),
        resolved_lane,
        mirror_frontmatter_lane: true,
    })
}

fn refuse_unnoted_review_exit(
    request: &TransitionRequest,
    from_lane: Lane,
    actor_identity: &str,
) -> Result<(), MutationError> {
    const VERDICT_GATED: [Lane; 2] = [Lane::InReview, Lane::Approved];
    if !request.force
        || !VERDICT_GATED.contains(&from_lane)
        || request.review_result.is_some()
        || guard::is_migration_actor(Some(actor_identity))
    {
        return Ok(());
    }
    if request
        .reason
        .as_deref()
        .is_some_and(|reason| !reason.trim().is_empty())
    {
        return Ok(());
    }
    Err(MutationError::TransitionRefused {
        reason: format!(
            "{} cannot leave {} by force without a note. {FORCE_NOTE_REQUIRED}; as an operator pass {FORCE_NOTE_HINT} (`--reason <why>` on `agent status emit`).",
            request.wp_id,
            from_lane.as_str()
        ),
    })
}

/// Build the canonical `DoneEvidence.to_dict()` shape from raw input.
fn build_done_evidence(raw: &Value) -> Result<Value, MutationError> {
    let review = raw
        .get("review")
        .and_then(Value::as_object)
        .ok_or_else(|| MutationError::TransitionRefused {
            reason:
                "Moving to done requires evidence with review.reviewer review.verdict, and review.reference"
                    .to_owned(),
        })?;
    let mut review_out = Map::new();
    for field in ["reviewer", "verdict", "reference"] {
        let value = review.get(field).and_then(Value::as_str).unwrap_or("");
        if value.is_empty() {
            return Err(MutationError::TransitionRefused {
                reason:
                    "Moving to done requires evidence with review.reviewer review.verdict, and review.reference"
                        .to_owned(),
            });
        }
        review_out.insert(field.to_owned(), Value::String(value.to_owned()));
    }
    if review_out
        .get("reference")
        .and_then(Value::as_str)
        .is_some_and(|reference| reference.trim().is_empty())
    {
        return Err(MutationError::TransitionRefused {
            reason:
                "Moving to done requires evidence with review.reviewer review.verdict, and review.reference"
                    .to_owned(),
        });
    }

    let mut evidence = Map::new();
    evidence.insert("review".to_owned(), Value::Object(review_out));

    let repos = raw.get("repos").and_then(Value::as_array);
    if let Some(repos) = repos.filter(|repos| !repos.is_empty()) {
        let mut out = Vec::with_capacity(repos.len());
        for repo in repos {
            let object = repo.as_object().ok_or_else(|| MutationError::InvalidRow {
                reason: "done evidence repos entries must be objects".to_owned(),
            })?;
            let mut entry = Map::new();
            for field in ["repo", "branch", "commit"] {
                let value = object.get(field).and_then(Value::as_str).ok_or_else(|| {
                    MutationError::InvalidRow {
                        reason: format!("done evidence repo entry is missing {field:?}"),
                    }
                })?;
                entry.insert(field.to_owned(), Value::String(value.to_owned()));
            }
            if let Some(files) = object.get("files_touched").and_then(Value::as_array)
                && !files.is_empty()
            {
                entry.insert("files_touched".to_owned(), Value::Array(files.clone()));
            }
            out.push(Value::Object(entry));
        }
        evidence.insert("repos".to_owned(), Value::Array(out));
    }

    let verification = raw.get("verification").and_then(Value::as_array);
    if let Some(verification) = verification.filter(|items| !items.is_empty()) {
        let mut out = Vec::with_capacity(verification.len());
        for item in verification {
            let object = item.as_object().ok_or_else(|| MutationError::InvalidRow {
                reason: "done evidence verification entries must be objects".to_owned(),
            })?;
            let mut entry = Map::new();
            for field in ["command", "result", "summary"] {
                let value = object.get(field).and_then(Value::as_str).ok_or_else(|| {
                    MutationError::InvalidRow {
                        reason: format!("done evidence verification entry is missing {field:?}"),
                    }
                })?;
                entry.insert(field.to_owned(), Value::String(value.to_owned()));
            }
            out.push(Value::Object(entry));
        }
        evidence.insert("verification".to_owned(), Value::Array(out));
    }
    Ok(Value::Object(evidence))
}

/// Build the canonical `ReviewResult.to_dict()` shape.
fn build_review_result(raw: &Value) -> Result<Value, MutationError> {
    let object = raw.as_object().ok_or_else(|| MutationError::InvalidRow {
        reason: "review_result must be an object".to_owned(),
    })?;
    let mut out = Map::new();
    for field in ["reviewer", "verdict", "reference"] {
        let value =
            object
                .get(field)
                .and_then(Value::as_str)
                .ok_or_else(|| MutationError::InvalidRow {
                    reason: format!("review_result is missing {field:?}"),
                })?;
        out.insert(field.to_owned(), Value::String(value.to_owned()));
    }
    if let Some(path) = object.get("feedback_path").and_then(Value::as_str) {
        out.insert("feedback_path".to_owned(), Value::String(path.to_owned()));
    }
    Ok(Value::Object(out))
}

fn guard_done_evidence(evidence: &Value) -> DoneEvidence {
    let review = evidence.get("review").and_then(Value::as_object);
    DoneEvidence {
        review: review.map(|review| ReviewApproval {
            reviewer: string_field_value(review.get("reviewer")).unwrap_or_default(),
            verdict: string_field_value(review.get("verdict")).unwrap_or_default(),
            reference: string_field_value(review.get("reference")).unwrap_or_default(),
        }),
    }
}

fn string_field(value: &Value, field: &str) -> Option<String> {
    string_field_value(value.get(field))
}

fn string_field_value(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_owned)
}

// ---------------------------------------------------------------------------
// Subtask gate (frontmatter roster subset)
// ---------------------------------------------------------------------------

/// Infer the `in_progress -> for_review` subtask gate from the authored roster
/// and the reduced annotation slots.
///
/// The roster is the WP file's top-level `subtasks:` list (block or inline YAML
/// forms); completion is the reduced `subtasks` slot folded from
/// `InnerStateChanged` annotations. A roster id whose slot is not `done` or
/// `skipped` blocks — an unprovable state never fails open.
fn infer_subtasks_complete(
    feature_dir: &Path,
    wp_id: &str,
    stream: &DiaryStream,
) -> Result<bool, MutationError> {
    let roster = authored_subtask_roster(feature_dir, wp_id)?;
    if roster.is_empty() {
        return Ok(true);
    }
    let snapshot = reduce_diary(&stream.transitions, &stream.annotations);
    let mut statuses: BTreeMap<String, String> = BTreeMap::new();
    if let Some(state) = snapshot.work_packages.get(wp_id)
        && let Some(Value::Object(slots)) = state.get("subtasks")
    {
        for (id, value) in slots {
            if let Some(text) = value.as_str() {
                statuses.insert(id.clone(), text.to_owned());
            }
        }
    }
    let complete = roster.iter().all(|id| {
        statuses
            .get(id)
            .is_some_and(|status| status == "done" || status == "skipped")
    });
    Ok(complete)
}

fn authored_subtask_roster(feature_dir: &Path, wp_id: &str) -> Result<Vec<String>, MutationError> {
    let tasks_dir = feature_dir.join("tasks");
    let mut matches = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&tasks_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("md") {
                continue;
            }
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if name.eq_ignore_ascii_case("readme.md") {
                continue;
            }
            let lower = name.to_ascii_lowercase();
            let prefix = wp_id.to_ascii_lowercase();
            let Some(rest) = lower.strip_prefix(&prefix) else {
                continue;
            };
            if rest.is_empty() || rest.starts_with(['-', '_', '.']) {
                matches.push(path);
            }
        }
    }
    matches.sort();
    match matches.len() {
        0 => Err(MutationError::TransitionRefused {
            reason: format!(
                "Cannot resolve subtask roster for {wp_id}: no work package file in {}",
                tasks_dir.display()
            ),
        }),
        1 => {
            let text = std::fs::read_to_string(&matches[0]).map_err(|error| {
                MutationError::TransitionRefused {
                    reason: format!(
                        "Cannot resolve subtask roster for {wp_id}: {} is unreadable ({error})",
                        matches[0].display()
                    ),
                }
            })?;
            let subtasks =
                frontmatter_subtasks(&text).map_err(|reason| MutationError::TransitionRefused {
                    reason: format!(
                        "Cannot resolve subtask roster for {wp_id}: {} {reason}",
                        matches[0].display()
                    ),
                })?;
            Ok(normalize_roster(subtasks))
        }
        count => {
            let names = matches
                .iter()
                .filter_map(|path| path.file_name().and_then(|value| value.to_str()))
                .collect::<Vec<_>>()
                .join(", ");
            Err(MutationError::TransitionRefused {
                reason: format!(
                    "Cannot resolve the declared dependencies of {wp_id}: ambiguous match ({count} files in {}: {names})",
                    tasks_dir.display()
                ),
            })
        }
    }
}

/// Read the top-level `subtasks:` list from a Markdown frontmatter block.
///
/// Supports the canonical authored forms: `subtasks:` followed by `- ID` block
/// entries, and the inline `subtasks: []` / `subtasks: [T001, T002]` form.
/// Anything else is a refusal rather than a silently empty roster.
fn frontmatter_subtasks(text: &str) -> Result<Vec<String>, String> {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return Err("has no YAML frontmatter block".to_owned());
    }
    let mut block: Vec<&str> = Vec::new();
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        block.push(line);
    }
    let mut index = 0;
    while index < block.len() {
        let line = block[index];
        if let Some(inline) = line.strip_prefix("subtasks:") {
            let inline = inline.trim();
            if inline.starts_with('[') {
                if !inline.ends_with(']') {
                    return Err("declares a malformed inline `subtasks` list".to_owned());
                }
                let inner = &inline[1..inline.len() - 1];
                if inner.trim().is_empty() {
                    return Ok(Vec::new());
                }
                return Ok(inner.split(',').map(|item| unquote(item.trim())).collect());
            }
            if !inline.is_empty() {
                return Err("declares a malformed `subtasks` value".to_owned());
            }
            let mut entries = Vec::new();
            index += 1;
            while index < block.len() {
                let entry = block[index];
                let trimmed = entry.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    index += 1;
                    continue;
                }
                let Some(item) = trimmed.strip_prefix("- ") else {
                    break;
                };
                entries.push(unquote(item.trim()));
                index += 1;
            }
            return Ok(entries);
        }
        index += 1;
    }
    Err("has no `subtasks` key".to_owned())
}

fn unquote(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        if (bytes[0] == b'"' && bytes[value.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[value.len() - 1] == b'\'')
        {
            return value[1..value.len() - 1].to_owned();
        }
    }
    value.to_owned()
}

fn normalize_roster(entries: Vec<String>) -> Vec<String> {
    let mut seen = Vec::new();
    for entry in entries {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !seen.iter().any(|existing| existing == trimmed) {
            seen.push(trimmed.to_owned());
        }
    }
    seen
}

// ---------------------------------------------------------------------------
// Emission
// ---------------------------------------------------------------------------

fn load_mission_id(feature_dir: &Path) -> Option<String> {
    let path = feature_dir.join(META_FILENAME);
    let bytes = sk_kernel::read_bounded(&path, 1024 * 1024).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    value
        .get("mission_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn synthetic_alias_event(
    request: &TransitionRequest,
    mission_slug: &str,
    mission_id: Option<&str>,
    from_lane: Lane,
    resolved_lane: Lane,
) -> StatusEventDraft {
    StatusEventDraft {
        event_id: request.event_id.clone().unwrap_or_else(new_ulid),
        mission_slug: mission_slug.to_owned(),
        mission_id: mission_id.map(str::to_owned),
        wp_id: request.wp_id.clone(),
        from_lane,
        to_lane: resolved_lane,
        at: request
            .at
            .clone()
            .unwrap_or_else(sk_kernel::clock::now_utc_iso),
        actor: request.actor.clone(),
        force: request.force,
        execution_mode: request.execution_mode.clone(),
        reason: request.reason.clone(),
        reason_source: request.reason_source.clone(),
        review_ref: request.review_ref.clone(),
        evidence: None,
        review_result: request.review_result.clone(),
        policy_metadata: request.policy_metadata.clone(),
    }
}

/// Emit one status transition (flat/primary shell).
///
/// Acquires the Mission lock, derives the current lane, validates and builds the
/// event, appends it with readback verification, then materializes `status.json`.
/// A validation refusal happens before any write.
pub fn emit_status_transition(request: &TransitionRequest) -> Result<EmitOutcome, MutationError> {
    let feature_dir = canonicalize_feature_dir(&request.feature_dir);
    let mission_id = match &request.mission_id {
        Some(value) => Some(value.clone()),
        None => load_mission_id(&feature_dir),
    };
    let log_path = events_path(&feature_dir);
    let defaults = AppendOptions::default();

    let lock = acquire_mission_lock(&feature_dir, request.repo_root.as_deref(), request.wait())?;
    // The stream is observed *inside* the lock: another writer may have appended
    // between any pre-lock read and the acquire. This is the write surface's ONE
    // full-log read (baseline NFR-004).
    let stream = read_event_stream(&feature_dir)?;
    let from_lane = derive_from_lane_from_stream(&stream, &request.wp_id);
    let prepared = prepare_transition(
        request,
        &feature_dir,
        &request.mission_slug,
        mission_id.as_deref(),
        from_lane,
        &stream,
    )?;

    let Some(event) = prepared.event else {
        let event = synthetic_alias_event(
            request,
            &request.mission_slug,
            mission_id.as_deref(),
            from_lane,
            prepared.resolved_lane,
        );
        return Ok(EmitOutcome {
            event,
            persisted: false,
            log_path,
            snapshot: None,
            materialize_error: None,
            lock_path: lock.path().to_path_buf(),
            mission_lock_key: lock.key().to_owned(),
        });
    };

    let row = event.to_json_value();
    append_rows_atomic(&log_path, std::slice::from_ref(&row), &defaults)?;
    verify_event_rows(&log_path, std::slice::from_ref(&row))
        .map_err(|error| error.after_commit(&log_path))?;
    let (snapshot, materialize_error) = materialize_after_write(&feature_dir);

    Ok(EmitOutcome {
        event,
        persisted: true,
        log_path,
        snapshot,
        materialize_error,
        lock_path: lock.path().to_path_buf(),
        mission_lock_key: lock.key().to_owned(),
    })
}

fn materialize_after_write(feature_dir: &Path) -> (Option<MaterializeOutcome>, Option<String>) {
    match materialize_snapshot_file(feature_dir) {
        Ok(outcome) => (Some(outcome), None),
        Err(error) => (None, Some(error.to_string())),
    }
}

/// A batch member's request plus its expected identity.
///
/// Every batch member must target the same feature directory, mission and WP;
/// the batch is all-or-nothing and lands as one verified replacement.
pub fn emit_status_transition_batch(
    requests: &[TransitionRequest],
) -> Result<Vec<EmitOutcome>, MutationError> {
    if requests.is_empty() {
        return Ok(Vec::new());
    }
    let first = &requests[0];
    let feature_dir = canonicalize_feature_dir(&first.feature_dir);
    for request in requests {
        if canonicalize_feature_dir(&request.feature_dir) != feature_dir
            || request.mission_slug != first.mission_slug
            || request.wp_id != first.wp_id
        {
            return Err(MutationError::InvalidRequest {
                reason: "emit_status_transition_batch only supports one mission/WP per batch"
                    .to_owned(),
            });
        }
    }
    let mission_id = match &first.mission_id {
        Some(value) => Some(value.clone()),
        None => load_mission_id(&feature_dir),
    };
    let log_path = events_path(&feature_dir);
    let defaults = AppendOptions::default();

    let lock = acquire_mission_lock(&feature_dir, first.repo_root.as_deref(), first.wait())?;
    let stream = read_event_stream(&feature_dir)?;
    let mut from_lane = derive_from_lane_from_stream(&stream, &first.wp_id);
    let mut rows = Vec::new();
    let mut outcomes = Vec::new();
    for request in requests {
        let mut request = request.clone();
        // The baseline batch door never manufactures a workspace context.
        request.default_workspace_context = false;
        let prepared = prepare_transition(
            &request,
            &feature_dir,
            &request.mission_slug,
            mission_id.as_deref(),
            from_lane,
            &stream,
        )?;
        let Some(event) = prepared.event else {
            continue;
        };
        from_lane = prepared.resolved_lane;
        rows.push(event.to_json_value());
        outcomes.push(event);
    }
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    append_rows_atomic(&log_path, &rows, &defaults)?;
    verify_event_rows(&log_path, &rows).map_err(|error| error.after_commit(&log_path))?;
    let (snapshot, materialize_error) = materialize_after_write(&feature_dir);

    Ok(outcomes
        .into_iter()
        .map(|event| EmitOutcome {
            event,
            persisted: true,
            log_path: log_path.clone(),
            snapshot: snapshot.clone(),
            materialize_error: materialize_error.clone(),
            lock_path: lock.path().to_path_buf(),
            mission_lock_key: lock.key().to_owned(),
        })
        .collect())
}

/// A request to persist one off-axis `InnerStateChanged` annotation.
#[derive(Clone, Debug)]
pub struct AnnotationRequest {
    pub feature_dir: PathBuf,
    pub wp_id: String,
    pub actor: ActorValue,
    /// The delta object. Supply only populated slots (the typed
    /// `WPInnerStateDelta` builder is not ported); an empty object is refused.
    pub delta: Value,
    pub at: Option<String>,
    pub event_id: Option<String>,
    pub repo_root: Option<PathBuf>,
    pub lock_wait: Option<LockWait>,
}

/// The outcome of an annotation write.
#[derive(Clone, Debug)]
pub struct AnnotationOutcome {
    pub event_id: String,
    pub log_path: PathBuf,
    pub snapshot: Option<MaterializeOutcome>,
    pub materialize_error: Option<String>,
}

/// Persist one `InnerStateChanged` annotation and verify its readback.
///
/// This is the baseline `emit_inner_state_changed` persistence half: no FSM
/// edge, no `force_count`, one atomic append under the Mission lock, then a
/// best-effort materialize.
pub fn emit_inner_state_changed(
    request: &AnnotationRequest,
) -> Result<AnnotationOutcome, MutationError> {
    let wp_id = request.wp_id.trim().to_ascii_uppercase();
    let canonical: sk_kernel::WpId = wp_id.parse().map_err(|_| MutationError::InvalidRequest {
        reason: format!(
            "annotate() refuses wp_id not matching the WP-id pattern: {:?}",
            request.wp_id
        ),
    })?;
    let Value::Object(delta) = &request.delta else {
        return Err(MutationError::InvalidRow {
            reason: "an annotation delta must be a JSON object".to_owned(),
        });
    };
    if delta.is_empty() {
        return Err(MutationError::InvalidRow {
            reason: "annotate() refuses an empty delta (it folds to a no-op)".to_owned(),
        });
    }
    let event_id = match &request.event_id {
        Some(value) if is_ulid(value) => value.clone(),
        Some(value) => {
            return Err(MutationError::InvalidRow {
                reason: format!("event_id {value:?} is not a canonical ULID"),
            });
        }
        None => new_ulid(),
    };
    let feature_dir = canonicalize_feature_dir(&request.feature_dir);
    let log_path = events_path(&feature_dir);
    let wait = request
        .lock_wait
        .unwrap_or(LockWait::Bounded(std::time::Duration::from_secs(
            DEFAULT_LOCK_WAIT_SECONDS,
        )));
    let _lock = acquire_mission_lock(&feature_dir, request.repo_root.as_deref(), wait)?;

    let mut row = Map::new();
    row.insert("event_id".to_owned(), Value::String(event_id.clone()));
    row.insert("kind".to_owned(), Value::String("annotation".to_owned()));
    row.insert(
        "wp_id".to_owned(),
        Value::String(canonical.as_str().to_owned()),
    );
    row.insert(
        "at".to_owned(),
        Value::String(
            request
                .at
                .clone()
                .unwrap_or_else(sk_kernel::clock::now_utc_iso),
        ),
    );
    row.insert("actor".to_owned(), request.actor.to_json_value());
    row.insert("delta".to_owned(), Value::Object(delta.clone()));
    let row = Value::Object(row);

    append_rows_atomic(
        &log_path,
        std::slice::from_ref(&row),
        &AppendOptions::default(),
    )?;
    verify_event_rows(&log_path, std::slice::from_ref(&row))
        .map_err(|error| error.after_commit(&log_path))?;
    let (snapshot, materialize_error) = materialize_after_write(&feature_dir);
    Ok(AnnotationOutcome {
        event_id,
        log_path,
        snapshot,
        materialize_error,
    })
}
