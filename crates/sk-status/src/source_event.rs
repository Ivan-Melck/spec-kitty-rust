//! The ONE canonical Source-valued status-event model, admission and fold.
//!
//! The reference's CLI event model (`specify_cli.status.models.StatusEvent`)
//! preserves the *authored JSON values* of `event_id`, `wp_id`, `at`, `force`
//! and the actor/review payloads. Only `from_lane`/`to_lane` are constrained
//! (to the `Lane` enum); everything else reaches the fold as Python values. The
//! fold then keys, hashes, sorts and truth-tests those values with Python rules.
//!
//! A finite Rust struct with `String`/`bool` identities cannot represent that
//! domain: it stringifies numbers, refuses `NaN`/`Infinity`, loses exact
//! integers and changes ordering. This module therefore owns ONE representation
//! — the admitted row as [`PythonJsonValue`] (the canonical Kernel J1 JSON
//! value domain) plus its validated lane/actor views — and ONE fold that
//! consumes those values through the shared Kernel comparison/hashability
//! facade. The existing typed diary (`crate::replay::StatusEvent`) is retained
//! as a finite view whose adapter feeds the same fold; it is not a second
//! reducer.
//!
//! Phase fidelity (the reference's order):
//!
//! 1. line JSON decode (`json.loads` rules: duplicate keys, big integers,
//!    `NaN`/`Infinity`, exact strings);
//! 2. row object check;
//! 3. `kind` partition (annotation / unknown);
//! 4. non-lane rows skipped in place;
//! 5. legacy `mission_id` resolution (a *resolver* failure is an admission
//!    failure, exactly like the reference; the resolver itself is
//!    [`crate::source_metadata::SourceSlugResolver`]'s actual lazy `meta.json`
//!    execution);
//! 6. model admission (`StatusEvent.from_dict` validation for lane rows,
//!    [`crate::source_annotation::SourceAnnotation`] for annotations);
//! 7. the fold — where unhashable identities (`wp_id`, `event_id`, the
//!    `(wp_id, at)` approval key, an unhashable `tracker_refs_replace` member),
//!    mixed scalar ordering and non-mapping `policy_metadata` raise, exactly as
//!    the reference does. Annotations fold in a dedicated post-transition pass,
//!    ordered by `(at, event_id)`, through the same state owner.
//!
//! Read-only: nothing here writes, repairs or materialises.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use sk_kernel::PythonString;
use sk_kernel::python_json::{
    PythonJsonDict, PythonJsonError, PythonJsonErrorKind, PythonJsonValue, parse,
    python_json_hashable, python_json_key_equal, python_json_le, python_json_lt,
    python_json_stable_sort_by_key, python_json_str, python_json_truthy,
    python_json_tuple_hashable, python_json_tuple_lt, python_json_type_name, python_repr_value,
};

use crate::lane::Lane;
use crate::source_annotation::{
    CLAIM_RELEASE_SLOTS, DELTA_SCALAR_FIELDS, ReviewFold, SourceAnnotation, SourceDelta,
};
use crate::source_execution::{
    SourceExecutionError, SourceExecutionPhase, SourceOperational, SourcePythonClass,
    SourcePythonFailure, SourceRaised,
};
use crate::source_metadata::{SourceResolverFailure, SourceSlugResolver};

/// Wire discriminator for off-axis runtime-state annotation events.
pub const ANNOTATION_KIND: &str = "annotation";

const STRUCTURED_ACTOR_FIELDS: [&str; 4] = ["role", "profile", "tool", "model"];
const STATUS_ACTOR_FIELDS: [&str; 5] = ["role", "profile", "tool", "model", "profile_source"];
const PROFILE_SOURCE_FIELD: &str = "profile_source";
const PROFILE_SOURCE_VALUES: [&str; 4] = ["invocation", "flag", "agent_string", "wp_frontmatter"];
const RECONSTRUCTION_SEED_PREFIX: &str = "migration:";

// ---------------------------------------------------------------------------
// Admission refusals
// ---------------------------------------------------------------------------

/// Why the reference's event store refused a status-log row set.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SourceEventLogError {
    /// The line is not JSON at all (`Invalid JSON on line N: …`).
    #[error("Invalid JSON on line {line}: {message}")]
    InvalidJson {
        /// 1-based line number in the log text.
        line: usize,
        /// The JSON decoder's reason.
        message: String,
    },
    /// The line is valid JSON but not an object.
    #[error("Invalid event structure on line {line}: expected JSON object")]
    NonObjectRow {
        /// 1-based line number in the log text.
        line: usize,
    },
    /// The row's `kind` discriminator is neither absent, null nor `annotation`.
    #[error("Unknown event kind {kind} on line {line}")]
    UnknownKind {
        /// 1-based line number in the log text.
        line: usize,
        /// The discriminator as the reference renders it (`repr`).
        kind: String,
    },
    /// The row is an object but not an admissible event.
    ///
    /// The reference raises this from its store-level
    /// `except (KeyError, ValueError, TypeError, AttributeError)` block, which
    /// serves annotation admission, lane admission and the legacy `mission_id`
    /// resolver alike; `phase` records which one refused the row.
    #[error("Invalid event structure on line {line}: {detail}")]
    InvalidStructure {
        /// 1-based line number in the log text.
        line: usize,
        /// Why the row is not admissible.
        detail: String,
        /// Which admitted object refused it.
        phase: SourceExecutionPhase,
    },
}

/// A fold-phase refusal — the reference's own `TypeError`/`AttributeError`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SourceFoldError {
    /// `TypeError`: unhashable identity or incomparable ranking.
    #[error("{message}")]
    TypeError {
        /// CPython's message text.
        message: String,
    },
    /// `AttributeError`: e.g. a non-mapping `policy_metadata`.
    #[error("{message}")]
    AttributeError {
        /// CPython's message text.
        message: String,
    },
}

impl SourceFoldError {
    fn type_error(message: impl Into<String>) -> Self {
        Self::TypeError {
            message: message.into(),
        }
    }

    fn attribute_error(message: impl Into<String>) -> Self {
        Self::AttributeError {
            message: message.into(),
        }
    }
}

impl From<sk_kernel::python_json::PythonJsonCompareError> for SourceFoldError {
    fn from(error: sk_kernel::python_json::PythonJsonCompareError) -> Self {
        Self::type_error(error.message().to_owned())
    }
}

// ---------------------------------------------------------------------------
// Source-valued event model
// ---------------------------------------------------------------------------

/// The decoded `actor` view: a plain scalar `str`, or the structured binding.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SourceActor {
    Text(PythonString),
    Structured(Vec<(&'static str, Option<PythonString>)>),
}

impl SourceActor {
    pub(crate) fn identity(&self) -> PythonString {
        match self {
            Self::Text(text) => text.clone(),
            Self::Structured(fields) => fields
                .iter()
                .find(|(name, _)| *name == "tool")
                .and_then(|(_, value)| value.clone())
                .or_else(|| {
                    fields
                        .iter()
                        .find(|(name, _)| *name == "role")
                        .and_then(|(_, value)| value.clone())
                })
                .unwrap_or_default(),
        }
    }

    pub(crate) fn to_value(&self) -> PythonJsonValue {
        match self {
            Self::Text(text) => PythonJsonValue::String(text.clone()),
            Self::Structured(fields) => {
                let mut object = PythonJsonDict::new();
                for (name, value) in fields {
                    object.insert(
                        PythonString::from_scalars(name),
                        value
                            .clone()
                            .map(PythonJsonValue::String)
                            .unwrap_or(PythonJsonValue::Null),
                    );
                }
                PythonJsonValue::ExactObject(object)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct SourceReviewApproval {
    reviewer: PythonJsonValue,
    verdict: PythonJsonValue,
    reference: PythonJsonValue,
}

#[derive(Debug, Clone, PartialEq)]
struct SourceRepoEvidence {
    repo: PythonJsonValue,
    branch: PythonJsonValue,
    commit: PythonJsonValue,
    files_touched: Option<PythonJsonValue>,
}

#[derive(Debug, Clone, PartialEq)]
struct SourceVerification {
    command: PythonJsonValue,
    result: PythonJsonValue,
    summary: PythonJsonValue,
}

#[derive(Debug, Clone, PartialEq)]
struct SourceDoneEvidence {
    review: SourceReviewApproval,
    repos: Vec<SourceRepoEvidence>,
    verification: Vec<SourceVerification>,
}

#[derive(Debug, Clone, PartialEq)]
struct SourceReviewResult {
    reviewer: PythonJsonValue,
    verdict: PythonJsonValue,
    reference: PythonJsonValue,
    feedback_path: Option<PythonJsonValue>,
}

impl SourceDoneEvidence {
    fn to_value(&self) -> PythonJsonValue {
        let mut root = PythonJsonDict::new();
        root.insert(key("review"), self.review.to_value());
        if !self.repos.is_empty() {
            root.insert(
                key("repos"),
                PythonJsonValue::Array(
                    self.repos
                        .iter()
                        .map(SourceRepoEvidence::to_value)
                        .collect(),
                ),
            );
        }
        if !self.verification.is_empty() {
            root.insert(
                key("verification"),
                PythonJsonValue::Array(
                    self.verification
                        .iter()
                        .map(SourceVerification::to_value)
                        .collect(),
                ),
            );
        }
        PythonJsonValue::ExactObject(root)
    }
}

impl SourceReviewApproval {
    fn to_value(&self) -> PythonJsonValue {
        let mut object = PythonJsonDict::new();
        object.insert(key("reviewer"), self.reviewer.clone());
        object.insert(key("verdict"), self.verdict.clone());
        object.insert(key("reference"), self.reference.clone());
        PythonJsonValue::ExactObject(object)
    }
}

impl SourceRepoEvidence {
    fn to_value(&self) -> PythonJsonValue {
        let mut object = PythonJsonDict::new();
        object.insert(key("repo"), self.repo.clone());
        object.insert(key("branch"), self.branch.clone());
        object.insert(key("commit"), self.commit.clone());
        if let Some(files) = &self.files_touched
            && python_json_truthy(files)
        {
            object.insert(key("files_touched"), files.clone());
        }
        PythonJsonValue::ExactObject(object)
    }
}

impl SourceVerification {
    fn to_value(&self) -> PythonJsonValue {
        let mut object = PythonJsonDict::new();
        object.insert(key("command"), self.command.clone());
        object.insert(key("result"), self.result.clone());
        object.insert(key("summary"), self.summary.clone());
        PythonJsonValue::ExactObject(object)
    }
}

impl SourceReviewResult {
    fn to_value(&self) -> PythonJsonValue {
        let mut object = PythonJsonDict::new();
        object.insert(key("reviewer"), self.reviewer.clone());
        object.insert(key("verdict"), self.verdict.clone());
        object.insert(key("reference"), self.reference.clone());
        if let Some(path) = &self.feedback_path
            && !is_python_null(path)
        {
            object.insert(key("feedback_path"), path.clone());
        }
        PythonJsonValue::ExactObject(object)
    }
}

/// The ONE Source-valued lane transition.
///
/// `row` is the admitted raw JSON object (authored keys/values, duplicate-key
/// dict semantics already applied by the Kernel parser). Lanes and actor are
/// validated views over it; nothing is stringified or coerced for storage.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceStatusEvent {
    row: PythonJsonDict,
    from_lane: Lane,
    to_lane: Lane,
    actor: SourceActor,
    evidence: Option<SourceDoneEvidence>,
    review_result: Option<SourceReviewResult>,
}

impl SourceStatusEvent {
    /// The admitted authored row.
    pub fn raw(&self) -> &PythonJsonDict {
        &self.row
    }

    /// One authored field.
    pub fn field(&self, name: &str) -> Option<&PythonJsonValue> {
        self.row.get(name)
    }

    /// `event_id` exactly as authored (present by admission).
    pub fn event_id(&self) -> &PythonJsonValue {
        self.row
            .get("event_id")
            .expect("admission requires event_id")
    }

    /// `wp_id` exactly as authored.
    pub fn wp_id(&self) -> &PythonJsonValue {
        self.row.get("wp_id").expect("admission requires wp_id")
    }

    /// `at` exactly as authored.
    pub fn at(&self) -> &PythonJsonValue {
        self.row.get("at").expect("admission requires at")
    }

    /// `force` exactly as authored (raw; the fold truth-tests it).
    pub fn force(&self) -> &PythonJsonValue {
        self.row.get("force").expect("admission requires force")
    }

    /// The validated from-lane.
    pub fn from_lane(&self) -> Lane {
        self.from_lane
    }

    /// The validated to-lane.
    pub fn to_lane(&self) -> Lane {
        self.to_lane
    }

    /// The actor's display identity (`tool`, then `role`, then `""` for a
    /// structured actor; the exact Python `str()` for a scalar).
    pub fn actor_identity(&self) -> PythonString {
        self.actor.identity()
    }

    /// The reference's `mission_slug` projection (`mission_slug or feature_slug or ""`).
    pub fn mission_slug(&self) -> PythonJsonValue {
        self.row
            .get("mission_slug")
            .filter(|value| python_json_truthy(value))
            .cloned()
            .unwrap_or_else(|| {
                self.row
                    .get("feature_slug")
                    .cloned()
                    .unwrap_or_else(|| PythonJsonValue::scalar(""))
            })
    }

    /// The reference's `StatusEvent.to_dict()` projection (used only as the
    /// reconstruction-seed canonical tie-break key).
    pub fn to_dict_value(&self) -> PythonJsonValue {
        let mut object = PythonJsonDict::new();
        object.insert(key("event_id"), self.event_id().clone());
        object.insert(key("mission_slug"), self.mission_slug());
        object.insert(key("wp_id"), self.wp_id().clone());
        object.insert(
            key("from_lane"),
            PythonJsonValue::scalar(self.from_lane.as_str()),
        );
        object.insert(
            key("to_lane"),
            PythonJsonValue::scalar(self.to_lane.as_str()),
        );
        object.insert(key("at"), self.at().clone());
        object.insert(key("actor"), self.actor.to_value());
        object.insert(key("force"), self.force().clone());
        object.insert(
            key("execution_mode"),
            self.field("execution_mode")
                .cloned()
                .unwrap_or(PythonJsonValue::Null),
        );
        object.insert(
            key("reason"),
            self.field("reason")
                .cloned()
                .unwrap_or(PythonJsonValue::Null),
        );
        object.insert(
            key("review_ref"),
            self.field("review_ref")
                .cloned()
                .unwrap_or(PythonJsonValue::Null),
        );
        object.insert(
            key("evidence"),
            self.evidence
                .as_ref()
                .map(SourceDoneEvidence::to_value)
                .unwrap_or(PythonJsonValue::Null),
        );
        object.insert(
            key("policy_metadata"),
            self.field("policy_metadata")
                .cloned()
                .unwrap_or(PythonJsonValue::Null),
        );
        if let Some(result) = &self.review_result {
            object.insert(key("review_result"), result.to_value());
        }
        if let Some(source) = self
            .field("reason_source")
            .filter(|value| !is_python_null(value))
        {
            object.insert(key("reason_source"), source.clone());
        }
        if let Some(mission_id) = self
            .field("mission_id")
            .filter(|value| !is_python_null(value))
        {
            object.insert(key("mission_id"), mission_id.clone());
        }
        PythonJsonValue::ExactObject(object)
    }

    /// One admitted lane stream plus the off-axis annotations.
    ///
    /// Admit one already-parsed row exactly as `StatusEvent.from_dict` does.
    fn admit(row: PythonJsonDict) -> Result<Self, String> {
        required(&row, "event_id")?;
        required(&row, "wp_id")?;
        let from_lane = decode_lane(required(&row, "from_lane")?)?;
        let to_lane = decode_lane(required(&row, "to_lane")?)?;
        required(&row, "at")?;
        let actor = decode_actor(required(&row, "actor")?)?;
        required(&row, "force")?;
        required(&row, "execution_mode")?;
        let evidence = decode_evidence(row.get("evidence"))?;
        let review_result = decode_review_result(row.get("review_result"))?;
        Ok(Self {
            row,
            from_lane,
            to_lane,
            actor,
            evidence,
            review_result,
        })
    }
}

fn required<'a>(row: &'a PythonJsonDict, name: &str) -> Result<&'a PythonJsonValue, String> {
    row.get(name).ok_or_else(|| format!("'{name}'"))
}

pub(crate) fn key(name: &str) -> PythonString {
    PythonString::from_scalars(name)
}

fn string_value(value: &PythonJsonValue) -> Option<&PythonString> {
    match value {
        PythonJsonValue::String(text) => Some(text),
        _ => None,
    }
}

fn sequence_value(value: &PythonJsonValue) -> Option<&[PythonJsonValue]> {
    match value {
        PythonJsonValue::Array(items) => Some(items),
        _ => None,
    }
}

fn is_python_null(value: &PythonJsonValue) -> bool {
    matches!(value, PythonJsonValue::Null)
}

fn dict_contains_key(object: &PythonJsonDict, name: &str) -> bool {
    object.get(name).is_some()
}

// ---------------------------------------------------------------------------
// Read / admission
// ---------------------------------------------------------------------------

/// One admitted status-log stream.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SourceEventStream {
    /// Lane transitions, in log order (the fold deduplicates and orders).
    pub transitions: Vec<SourceStatusEvent>,
    /// Off-axis `kind == "annotation"` rows, in log order, as the ONE Source
    /// annotation model ([`SourceAnnotation`]).
    pub annotations: Vec<SourceAnnotation>,
    /// 1-based line numbers skipped in place as non-lane records.
    pub non_lane_lines: Vec<usize>,
}

/// Read a status log through the Source-valued store-admission capability.
///
/// The error is the typed [`SourceExecutionError`]: the reference's caught
/// `StoreError` phases, the Python exceptions it lets escape (an annotation's
/// `int(Infinity)` `OverflowError`, `json.loads`'s digit-ceiling `ValueError`, the
/// metadata resolver's escaped `OSError`/`RecursionError`), and the native
/// adapter limits that have no Source counterpart.
pub fn read_source_event_stream_from_text(
    feature_dir: &Path,
    content: &str,
) -> Result<SourceEventStream, SourceExecutionError> {
    let mut stream = SourceEventStream::default();
    let mut resolver = SourceSlugResolver::new(feature_dir);
    for (index, line) in sk_kernel::python_text::splitlines(content)
        .into_iter()
        .enumerate()
    {
        let line_number = index + 1;
        let stripped = line.trim_matches(|character: char| {
            sk_kernel::python_text::is_whitespace(character as u32)
        });
        if stripped.is_empty() {
            continue;
        }
        let value: PythonJsonValue = parse(stripped)
            .map_err(|error: PythonJsonError| line_decode_error(line_number, &error))?;
        let Some(object) = normalize_object(&value) else {
            return Err(SourceEventLogError::NonObjectRow { line: line_number }.into());
        };

        if let Some(kind) = object.get("kind").filter(|value| !is_python_null(value)) {
            if string_value(kind)
                .and_then(|text| text.to_scalar_string().ok())
                .as_deref()
                == Some(ANNOTATION_KIND)
            {
                let annotation = admit_annotation(&object).map_err(|failure| {
                    SourceExecutionError::store_or_escape(
                        failure.at(SourceExecutionPhase::AnnotationAdmission, Some(line_number)),
                    )
                })?;
                stream.annotations.push(annotation);
                continue;
            }
            return Err(SourceEventLogError::UnknownKind {
                line: line_number,
                kind: python_repr(kind),
            }
            .into());
        }
        if is_non_lane_row(&object) {
            stream.non_lane_lines.push(line_number);
            continue;
        }

        let mut row = object;
        // The reference resolves `mission_id` from the raw dict before model
        // decode; its resolver raises inside the admission try-block. It runs
        // even when a falsy `mission_id` key is present (the key is then never
        // overwritten), so a resolver failure keeps the admission phase.
        let resolved = resolver
            .resolve_for_row(&row)
            .map_err(|failure| match failure {
                SourceResolverFailure::Raised(raised) => {
                    SourceExecutionError::store_or_escape(raised.with_line(line_number))
                }
                SourceResolverFailure::Operational(operational) => {
                    SourceExecutionError::Operational(operational)
                }
            })?;
        if !dict_contains_key(&row, "mission_id")
            && let Some(mission_id) = resolved
        {
            row.insert(key("mission_id"), mission_id);
        }
        let event = SourceStatusEvent::admit(row).map_err(|detail| {
            SourceEventLogError::InvalidStructure {
                line: line_number,
                detail,
                phase: SourceExecutionPhase::TransitionAdmission,
            }
        })?;
        stream.transitions.push(event);
    }
    Ok(stream)
}

/// Classify the J1 decoder's refusal of one line the way CPython does.
///
/// `json.loads` raises `JSONDecodeError` for a syntax error (caught by the store),
/// but a bare `ValueError` for the integer digit ceiling — outside the store's
/// `except json.JSONDecodeError` and therefore **escaping**. The J1 reader's own
/// operational limits have no Source counterpart and are labelled as such.
fn line_decode_error(line: usize, error: &PythonJsonError) -> SourceExecutionError {
    match error.kind() {
        PythonJsonErrorKind::IntegerDigitLimit => SourceExecutionError::Escaped(
            SourceRaised::new(
                SourcePythonClass::ValueError,
                SourceExecutionPhase::LineDecode,
                error.message(),
            )
            .with_line(line),
        ),
        PythonJsonErrorKind::IntegerDigitLimitSetting
        | PythonJsonErrorKind::RecursionBudget
        | PythonJsonErrorKind::NumberConversion => SourceExecutionError::Operational(
            SourceOperational::new(SourceExecutionPhase::LineDecode, error.message()),
        ),
        _ => SourceEventLogError::InvalidJson {
            line,
            message: error.to_string(),
        }
        .into(),
    }
}

/// The transitions-only view of [`read_source_event_stream_from_text`].
pub fn read_source_transitions_from_text(
    feature_dir: &Path,
    content: &str,
) -> Result<Vec<SourceStatusEvent>, SourceExecutionError> {
    Ok(read_source_event_stream_from_text(feature_dir, content)?.transitions)
}

pub(crate) fn normalize_object(value: &PythonJsonValue) -> Option<PythonJsonDict> {
    match value {
        PythonJsonValue::ExactObject(object) => Some(object.clone()),
        PythonJsonValue::Object(entries) => {
            let mut object = PythonJsonDict::new();
            for (key, value) in entries {
                object.insert(PythonString::from_scalars(key), value.clone());
            }
            Some(object)
        }
        _ => None,
    }
}

/// Python's `repr` for the values the reference interpolates with `!r`.
pub(crate) fn python_repr(value: &PythonJsonValue) -> String {
    match value {
        PythonJsonValue::String(text) => text.repr(),
        other => python_repr_value(other),
    }
}

fn is_non_lane_row(object: &PythonJsonDict) -> bool {
    if let Some(PythonJsonValue::String(name)) = object.get("event_name")
        && starts_with(name, "retrospective.")
    {
        return true;
    }
    if let Some(PythonJsonValue::String(event_type)) = object.get("type")
        && matches!(
            event_type.to_scalar_string().ok().as_deref(),
            Some("RetrospectiveCaptured" | "RetrospectiveCaptureFailed" | "RetrospectiveSkipped")
        )
    {
        return true;
    }
    dict_contains_key(object, "event_type")
}

fn starts_with(value: &PythonString, prefix: &str) -> bool {
    let prefix: Vec<u32> = prefix.chars().map(u32::from).collect();
    value.codepoints().starts_with(&prefix)
}

/// The row's own truthful `mission_slug`, else `feature_slug`.
pub(crate) fn row_mission_slug(object: &PythonJsonDict) -> Option<&PythonJsonValue> {
    object
        .get("mission_slug")
        .filter(|value| python_json_truthy(value))
        .or_else(|| {
            object
                .get("feature_slug")
                .filter(|value| python_json_truthy(value))
        })
}

fn decode_lane(value: &PythonJsonValue) -> Result<Lane, String> {
    match value {
        PythonJsonValue::String(text) => match text.to_scalar_string() {
            Ok(text) => Lane::parse_canonical(&text)
                .map_err(|_| format!("{} is not a valid Lane", python_repr(value))),
            Err(_) => Err(format!("{} is not a valid Lane", python_repr(value))),
        },
        PythonJsonValue::Array(_) => Err("unhashable type: 'list'".to_owned()),
        PythonJsonValue::Object(_) | PythonJsonValue::ExactObject(_) => {
            Err("unhashable type: 'dict'".to_owned())
        }
        other => Err(format!("{} is not a valid Lane", python_repr(other))),
    }
}

pub(crate) fn decode_actor(value: &PythonJsonValue) -> Result<SourceActor, String> {
    let Some(object) = normalize_object(value) else {
        return Ok(SourceActor::Text(python_json_str(value)));
    };
    let keys: HashSet<String> = object
        .keys()
        .filter_map(|key| key.to_scalar_string().ok())
        .collect();
    let four: HashSet<String> = STRUCTURED_ACTOR_FIELDS
        .iter()
        .map(|key| (*key).to_owned())
        .collect();
    let five: HashSet<String> = STATUS_ACTOR_FIELDS
        .iter()
        .map(|key| (*key).to_owned())
        .collect();
    if keys != four && keys != five {
        return Err(format!(
            "structured actor must contain exactly {} or {}; got {}",
            python_sorted_list(&STRUCTURED_ACTOR_FIELDS),
            python_sorted_list(&STATUS_ACTOR_FIELDS),
            python_sorted_set(&keys)
        ));
    }
    let mut decoded: Vec<(&'static str, Option<PythonString>)> = Vec::new();
    for name in STATUS_ACTOR_FIELDS {
        let Some(value) = object.get(name) else {
            continue;
        };
        let decoded_value = match value {
            PythonJsonValue::Null => None,
            PythonJsonValue::String(text) => Some(text.clone()),
            other => {
                return Err(format!(
                    "structured actor fields must be strings or null; '{name}' was {}",
                    python_json_type_name(other)
                ));
            }
        };
        decoded.push((name, decoded_value));
    }
    let profile = decoded
        .iter()
        .find(|(name, _)| *name == "profile")
        .and_then(|(_, value)| value.clone());
    if let Some((_, source)) = decoded
        .iter()
        .find(|(name, _)| *name == PROFILE_SOURCE_FIELD)
    {
        validate_profile_source(source.clone(), profile)?;
    }
    Ok(SourceActor::Structured(decoded))
}

fn validate_profile_source(
    source: Option<PythonString>,
    profile: Option<PythonString>,
) -> Result<(), String> {
    if let Some(source) = &source {
        let known = source
            .to_scalar_string()
            .ok()
            .is_some_and(|text| PROFILE_SOURCE_VALUES.contains(&text.as_str()));
        if !known {
            let rendered: Vec<String> = {
                let mut values: Vec<String> = PROFILE_SOURCE_VALUES
                    .iter()
                    .map(|value| format!("'{value}'"))
                    .collect();
                values.sort();
                values
            };
            // The reference interpolates the value with `!r`: CPython `repr`
            // (single-quoted for a str), never Rust's `Debug` quoting.
            let got = source.repr();
            // The reference's message; admission catches this ValueError.
            return Err(format!(
                "structured actor profile_source must be one of [{}] or null; got {got}",
                rendered.join(", ")
            ));
        }
    }
    if (source.is_none()) != (profile.is_none()) {
        return Err(
            "structured actor profile_source must be null exactly when profile is null".to_owned(),
        );
    }
    Ok(())
}

fn decode_evidence(value: Option<&PythonJsonValue>) -> Result<Option<SourceDoneEvidence>, String> {
    let Some(value) = value.filter(|value| python_json_truthy(value)) else {
        return Ok(None);
    };
    let object = normalize_object(value).ok_or_else(|| subscript_error(value))?;
    let review_value = object.get("review").ok_or_else(|| "'review'".to_owned())?;
    let review_object = normalize_object(review_value)
        .ok_or_else(|| format!("evidence review: {}", subscript_error(review_value)))?;
    let review = SourceReviewApproval {
        reviewer: required(&review_object, "reviewer")?.clone(),
        verdict: required(&review_object, "verdict")?.clone(),
        reference: required(&review_object, "reference")?.clone(),
    };
    let repos = match object.get("repos") {
        None => Vec::new(),
        Some(value) => sequence_items(value, "evidence repos")?
            .into_iter()
            .map(|item| {
                let entry = normalize_object(item)
                    .ok_or_else(|| format!("evidence repos: {}", subscript_error(item)))?;
                Ok(SourceRepoEvidence {
                    repo: required(&entry, "repo")?.clone(),
                    branch: required(&entry, "branch")?.clone(),
                    commit: required(&entry, "commit")?.clone(),
                    files_touched: entry.get("files_touched").cloned(),
                })
            })
            .collect::<Result<Vec<_>, String>>()?,
    };
    let verification = match object.get("verification") {
        None => Vec::new(),
        Some(value) => sequence_items(value, "evidence verification")?
            .into_iter()
            .map(|item| {
                let entry = normalize_object(item)
                    .ok_or_else(|| format!("evidence verification: {}", subscript_error(item)))?;
                Ok(SourceVerification {
                    command: required(&entry, "command")?.clone(),
                    result: required(&entry, "result")?.clone(),
                    summary: required(&entry, "summary")?.clone(),
                })
            })
            .collect::<Result<Vec<_>, String>>()?,
    };
    Ok(Some(SourceDoneEvidence {
        review,
        repos,
        verification,
    }))
}

fn decode_review_result(
    value: Option<&PythonJsonValue>,
) -> Result<Option<SourceReviewResult>, String> {
    let Some(value) = value.filter(|value| python_json_truthy(value)) else {
        return Ok(None);
    };
    let object = normalize_object(value).ok_or_else(|| subscript_error(value))?;
    Ok(Some(SourceReviewResult {
        reviewer: required(&object, "reviewer")?.clone(),
        verdict: required(&object, "verdict")?.clone(),
        reference: required(&object, "reference")?.clone(),
        feedback_path: object.get("feedback_path").cloned(),
    }))
}

fn sequence_items<'a>(
    value: &'a PythonJsonValue,
    field: &str,
) -> Result<Vec<&'a PythonJsonValue>, String> {
    match value {
        PythonJsonValue::Array(items) => Ok(items.iter().collect()),
        other => Err(format!(
            "{field}: '{}' object is not iterable",
            python_json_type_name(other)
        )),
    }
}

fn subscript_error(value: &PythonJsonValue) -> String {
    match value {
        PythonJsonValue::Array(_) => "list indices must be integers or slices, not str".to_owned(),
        PythonJsonValue::Object(_) | PythonJsonValue::ExactObject(_) => {
            "dictionary keys must be strings".to_owned()
        }
        PythonJsonValue::String(_) => "string indices must be integers, not 'str'".to_owned(),
        other => format!(
            "'{}' object is not subscriptable",
            python_json_type_name(other)
        ),
    }
}

fn python_sorted_list(values: &[&str]) -> String {
    let mut owned: Vec<&str> = values.to_vec();
    owned.sort_unstable();
    format!(
        "[{}]",
        owned
            .iter()
            .map(|value| format!("'{value}'"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn python_sorted_set(values: &HashSet<String>) -> String {
    let mut owned: Vec<&str> = values.iter().map(String::as_str).collect();
    owned.sort_unstable();
    format!(
        "[{}]",
        owned
            .iter()
            .map(|value| format!("'{value}'"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// Admit one `kind == "annotation"` row through the ONE Source annotation model.
fn admit_annotation(object: &PythonJsonDict) -> Result<SourceAnnotation, SourcePythonFailure> {
    SourceAnnotation::admit(object)
}

// ---------------------------------------------------------------------------
// The ONE fold
// ---------------------------------------------------------------------------

/// A per-WP Source-valued state: snapshot slots carrying exact Python values.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SourceWpState {
    slots: BTreeMap<String, PythonJsonValue>,
}

impl SourceWpState {
    /// One snapshot slot.
    pub fn get(&self, name: &str) -> Option<&PythonJsonValue> {
        self.slots.get(name)
    }

    /// Every slot in name order.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &PythonJsonValue)> {
        self.slots.iter()
    }

    /// The reduced lane (a canonical lane string), when present.
    pub fn lane(&self) -> Option<Lane> {
        match self.slots.get("lane")? {
            PythonJsonValue::String(text) => text
                .to_scalar_string()
                .ok()
                .and_then(|text| Lane::parse_canonical(&text).ok()),
            _ => None,
        }
    }

    fn insert(&mut self, name: impl Into<String>, value: PythonJsonValue) {
        self.slots.insert(name.into(), value);
    }

    fn remove(&mut self, name: &str) {
        self.slots.remove(name);
    }
}

/// A Python-dict-ordered map keyed by Source values.
///
/// Insertion/lookup use Kernel Python key equality (numeric tower, exact strings,
/// shared `NaN` singleton) and refuse an unhashable key with Python's own
/// message. It is a thin adapter over the canonical equality/hashability
/// authority, not a second hash implementation.
#[derive(Debug, Clone, PartialEq)]
struct SourceKeyMap<V> {
    entries: Vec<(PythonJsonValue, V)>,
}

impl<V> Default for SourceKeyMap<V> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}

impl<V> SourceKeyMap<V> {
    fn get(&self, key: &PythonJsonValue) -> Option<&V> {
        self.entries
            .iter()
            .find(|(candidate, _)| python_json_key_equal(candidate, key))
            .map(|(_, value)| value)
    }

    fn get_mut(&mut self, key: &PythonJsonValue) -> Option<&mut V> {
        self.entries
            .iter_mut()
            .find(|(candidate, _)| python_json_key_equal(candidate, key))
            .map(|(_, value)| value)
    }

    fn insert(&mut self, key: PythonJsonValue, value: V) -> Result<(), SourceFoldError> {
        python_json_hashable(&key)?;
        if let Some(slot) = self.get_mut(&key) {
            *slot = value;
        } else {
            self.entries.push((key, value));
        }
        Ok(())
    }

    fn contains(&self, key: &PythonJsonValue) -> Result<bool, SourceFoldError> {
        python_json_hashable(key)?;
        Ok(self.get(key).is_some())
    }
}

/// The reduced result for Source-valued events.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceSnapshot {
    /// The reference's sanitized mission slug projection.
    pub mission_slug: PythonJsonValue,
    /// `last_transition.at` of the last ordered transition.
    pub materialized_at: PythonJsonValue,
    /// Number of folded transitions (after dedupe).
    pub event_count: usize,
    /// `last_event_id` of the last ordered transition.
    pub last_event_id: Option<PythonJsonValue>,
    /// WP id -> reduced state, in first-insertion order.
    pub work_packages: Vec<(PythonJsonValue, SourceWpState)>,
    /// Display-lane counts.
    pub summary: BTreeMap<String, u64>,
}

impl SourceSnapshot {
    /// One reduced WP by Source identity (Python key equality + hashability).
    pub fn work_package(
        &self,
        wp_id: &PythonJsonValue,
    ) -> Result<Option<&SourceWpState>, SourceFoldError> {
        python_json_hashable(wp_id)?;
        Ok(self
            .work_packages
            .iter()
            .find(|(candidate, _)| python_json_key_equal(candidate, wp_id))
            .map(|(_, state)| state))
    }

    /// One reduced WP by its plain Rust id.
    pub fn work_package_by_str(&self, wp_id: &str) -> Option<&SourceWpState> {
        self.work_package(&PythonJsonValue::scalar(wp_id))
            .ok()
            .flatten()
    }
}

trait DiaryFoldEvent: Clone {
    fn event_id_fold(&self) -> PythonJsonValue;
    fn wp_id_fold(&self) -> PythonJsonValue;
    fn at_fold(&self) -> PythonJsonValue;
    fn mission_slug_fold(&self) -> PythonJsonValue;
    fn force_truthy_fold(&self) -> bool;
    fn source_from_lane(&self) -> Lane;
    fn source_to_lane(&self) -> Lane;
    fn review_ref_present_fold(&self) -> bool;
    fn actor_identity_fold(&self) -> PythonString;
    fn claim_writes_fold(&self) -> Result<BTreeMap<String, PythonJsonValue>, SourceFoldError>;
    fn review_result_present_fold(&self) -> bool;
    fn review_result_value_fold(&self) -> Option<PythonJsonValue>;
    fn canonical_key_fold(&self) -> String;
}

impl DiaryFoldEvent for SourceStatusEvent {
    fn event_id_fold(&self) -> PythonJsonValue {
        self.event_id().clone()
    }
    fn wp_id_fold(&self) -> PythonJsonValue {
        self.wp_id().clone()
    }
    fn at_fold(&self) -> PythonJsonValue {
        self.at().clone()
    }
    fn mission_slug_fold(&self) -> PythonJsonValue {
        self.mission_slug()
    }
    fn force_truthy_fold(&self) -> bool {
        python_json_truthy(self.force())
    }
    fn source_from_lane(&self) -> Lane {
        self.from_lane
    }
    fn source_to_lane(&self) -> Lane {
        self.to_lane
    }
    fn review_ref_present_fold(&self) -> bool {
        self.field("review_ref")
            .is_some_and(|value| !is_python_null(value))
    }
    fn actor_identity_fold(&self) -> PythonString {
        self.actor.identity()
    }
    fn claim_writes_fold(&self) -> Result<BTreeMap<String, PythonJsonValue>, SourceFoldError> {
        claim_sidecar_writes(self.from_lane, self.to_lane, self.field("policy_metadata"))
    }
    fn review_result_present_fold(&self) -> bool {
        self.review_result.is_some()
    }
    fn review_result_value_fold(&self) -> Option<PythonJsonValue> {
        self.review_result
            .as_ref()
            .map(SourceReviewResult::to_value)
    }
    fn canonical_key_fold(&self) -> String {
        canonical_key(&self.to_dict_value())
    }
}

impl DiaryFoldEvent for crate::replay::StatusEvent {
    fn event_id_fold(&self) -> PythonJsonValue {
        PythonJsonValue::scalar(&self.event_id)
    }
    fn wp_id_fold(&self) -> PythonJsonValue {
        PythonJsonValue::scalar(&self.wp_id)
    }
    fn at_fold(&self) -> PythonJsonValue {
        PythonJsonValue::scalar(&self.at)
    }
    fn mission_slug_fold(&self) -> PythonJsonValue {
        PythonJsonValue::scalar(&self.mission_slug)
    }
    fn force_truthy_fold(&self) -> bool {
        self.force
    }
    fn source_from_lane(&self) -> Lane {
        self.from_lane
    }
    fn source_to_lane(&self) -> Lane {
        self.to_lane
    }
    fn review_ref_present_fold(&self) -> bool {
        self.review_ref.is_some()
    }
    fn actor_identity_fold(&self) -> PythonString {
        PythonString::from_scalars(&self.actor.identity())
    }
    fn claim_writes_fold(&self) -> Result<BTreeMap<String, PythonJsonValue>, SourceFoldError> {
        let mut writes = BTreeMap::new();
        if self.from_lane != Lane::Planned || self.to_lane != Lane::Claimed {
            return Ok(writes);
        }
        let Some(serde_json::Value::Object(meta)) = self.policy_metadata.as_ref() else {
            return Ok(writes);
        };
        for slot in ["shell_pid", "shell_pid_created_at"] {
            if let Some(value) = meta.get(slot).filter(|value| !value.is_null()) {
                writes.insert(slot.to_owned(), PythonJsonValue::from_serde(value));
            }
        }
        if let Some(agent) = meta
            .get("agent")
            .filter(|value| crate::replay::is_truthy(value))
        {
            writes.insert("agent".to_owned(), PythonJsonValue::from_serde(agent));
        }
        Ok(writes)
    }
    fn review_result_present_fold(&self) -> bool {
        self.review_result.is_some()
    }
    fn review_result_value_fold(&self) -> Option<PythonJsonValue> {
        self.review_result
            .as_ref()
            .map(|result| PythonJsonValue::from_serde(&result.to_json_value()))
    }
    fn canonical_key_fold(&self) -> String {
        canonical_key(&typed_to_dict_value(self))
    }
}

fn claim_sidecar_writes(
    from_lane: Lane,
    to_lane: Lane,
    policy_metadata: Option<&PythonJsonValue>,
) -> Result<BTreeMap<String, PythonJsonValue>, SourceFoldError> {
    let mut writes = BTreeMap::new();
    if from_lane != Lane::Planned || to_lane != Lane::Claimed {
        return Ok(writes);
    }
    let metadata = policy_metadata.cloned().unwrap_or(PythonJsonValue::Null);
    if !python_json_truthy(&metadata) {
        return Ok(writes);
    }
    let Some(object) = normalize_object(&metadata) else {
        return Err(SourceFoldError::attribute_error(format!(
            "'{}' object has no attribute 'get'",
            python_json_type_name(&metadata)
        )));
    };
    for slot in ["shell_pid", "shell_pid_created_at"] {
        if let Some(value) = object.get(slot).filter(|value| !is_python_null(value)) {
            writes.insert(slot.to_owned(), value.clone());
        }
    }
    if let Some(agent) = object
        .get("agent")
        .filter(|value| python_json_truthy(value))
    {
        writes.insert("agent".to_owned(), agent.clone());
    }
    Ok(writes)
}

fn canonical_key(value: &PythonJsonValue) -> String {
    value
        .render_python_with(
            sk_kernel::python_json::PythonJsonRenderOptions::default().sort_keys(true),
        )
        .to_scalar_string()
        .unwrap_or_else(|_| python_repr_value(value))
}

fn typed_to_dict_value(event: &crate::replay::StatusEvent) -> PythonJsonValue {
    let mut object = PythonJsonDict::new();
    object.insert(key("event_id"), PythonJsonValue::scalar(&event.event_id));
    object.insert(
        key("mission_slug"),
        PythonJsonValue::scalar(&event.mission_slug),
    );
    object.insert(key("wp_id"), PythonJsonValue::scalar(&event.wp_id));
    object.insert(
        key("from_lane"),
        PythonJsonValue::scalar(event.from_lane.as_str()),
    );
    object.insert(
        key("to_lane"),
        PythonJsonValue::scalar(event.to_lane.as_str()),
    );
    object.insert(key("at"), PythonJsonValue::scalar(&event.at));
    object.insert(
        key("actor"),
        PythonJsonValue::from_serde(&event.actor.to_json_value()),
    );
    object.insert(key("force"), PythonJsonValue::Bool(event.force));
    object.insert(
        key("execution_mode"),
        PythonJsonValue::scalar(&event.execution_mode),
    );
    object.insert(
        key("reason"),
        event
            .reason
            .as_ref()
            .map(|text| PythonJsonValue::scalar(text))
            .unwrap_or(PythonJsonValue::Null),
    );
    object.insert(
        key("review_ref"),
        event
            .review_ref
            .as_ref()
            .map(|text| PythonJsonValue::scalar(text))
            .unwrap_or(PythonJsonValue::Null),
    );
    object.insert(
        key("evidence"),
        event
            .evidence
            .as_ref()
            .map(|evidence| PythonJsonValue::from_serde(&evidence.to_json_value()))
            .unwrap_or(PythonJsonValue::Null),
    );
    object.insert(
        key("policy_metadata"),
        event
            .policy_metadata
            .as_ref()
            .map(PythonJsonValue::from_serde)
            .unwrap_or(PythonJsonValue::Null),
    );
    if let Some(result) = &event.review_result {
        object.insert(
            key("review_result"),
            PythonJsonValue::from_serde(&result.to_json_value()),
        );
    }
    if let Some(source) = &event.reason_source {
        object.insert(key("reason_source"), PythonJsonValue::scalar(source));
    }
    if let Some(mission_id) = &event.mission_id {
        object.insert(key("mission_id"), PythonJsonValue::scalar(mission_id));
    }
    PythonJsonValue::ExactObject(object)
}

/// Reduce a Source-valued transition stream (annotations are off-axis).
pub fn reduce_source_diary(
    transitions: &[SourceStatusEvent],
    annotations: &[SourceAnnotation],
) -> Result<SourceSnapshot, SourceFoldError> {
    fold_diary(transitions, annotations)
}

/// Reduce the finite typed diary view through the SAME fold.
pub(crate) fn reduce_typed_diary(
    transitions: &[crate::replay::StatusEvent],
    annotations: &[crate::replay::InnerStateChanged],
) -> crate::snapshot::Snapshot {
    let snapshot = fold_diary(transitions, annotations)
        .expect("the finite typed diary adapter cannot produce a Source fold refusal");
    project_snapshot(snapshot)
}

fn fold_diary<E: DiaryFoldEvent, A: FoldAnnotation>(
    transitions: &[E],
    annotations: &[A],
) -> Result<SourceSnapshot, SourceFoldError> {
    let unique = dedup_transitions(transitions)?;
    let sorted = order_transitions(&unique)?;
    let mut work_packages: SourceKeyMap<SourceWpState> = SourceKeyMap::default();
    let mut provenance: SourceKeyMap<SlotProvenance> = SourceKeyMap::default();
    for event in &sorted {
        let wp_id = event.wp_id_fold();
        python_json_hashable(&wp_id)?;
        let current = work_packages.get(&wp_id).cloned();
        if !provenance.contains(&wp_id)? {
            provenance.insert(wp_id.clone(), SlotProvenance::default())?;
        }
        let verdict = arbitrate_event(current.as_ref(), event, &sorted)?;
        match verdict {
            Arbitration::Apply => {
                let mut next = state_from_event(event, current.as_ref())?;
                let prov = provenance.get_mut(&wp_id).expect("provenance seeded above");
                record_applied_event(event, &mut next, prov)?;
                work_packages.insert(wp_id, next)?;
            }
            Arbitration::LostOnContinuity if current.is_some() && is_reconstruction_seed(event) => {
                let prov = provenance.get_mut(&wp_id).expect("provenance seeded above");
                let mut next = current.expect("checked");
                fold_losing_seed(event, &mut next, prov)?;
                work_packages.insert(wp_id, next)?;
            }
            Arbitration::LostOnContinuity | Arbitration::LostToRollback => {}
        }
    }

    let mut annotation_refs: Vec<&A> = annotations.iter().collect();
    python_json_stable_sort_by_key(&mut annotation_refs, |annotation| {
        vec![
            PythonJsonValue::String(annotation.annotation_at()),
            PythonJsonValue::String(annotation.annotation_event_id()),
        ]
    })?;
    for annotation in annotation_refs {
        let wp_id = PythonJsonValue::String(annotation.annotation_wp_id());
        if work_packages.get(&wp_id).is_none() {
            work_packages.insert(
                wp_id.clone(),
                runtime_only_wp_state(&annotation.annotation_actor_identity()),
            )?;
        }
        let state = work_packages
            .get_mut(&wp_id)
            .expect("runtime state seeded above");
        annotation.apply_annotation(state)?;
    }

    let mut summary: BTreeMap<String, u64> = crate::replay::DISPLAY_LANES
        .iter()
        .map(|lane| (lane.as_str().to_owned(), 0))
        .collect();
    for (_, state) in &work_packages.entries {
        if let Some(lane) = state.lane()
            && let Some(count) = summary.get_mut(lane.as_str())
        {
            *count += 1;
        }
    }

    let mission_slug = match sorted.first() {
        Some(event) => source_safe_mission_slug(&event.mission_slug_fold(), "")?,
        None => PythonJsonValue::scalar(""),
    };
    let last = sorted.last();
    Ok(SourceSnapshot {
        mission_slug,
        materialized_at: last
            .map(DiaryFoldEvent::at_fold)
            .unwrap_or_else(|| PythonJsonValue::scalar("")),
        event_count: sorted.len(),
        last_event_id: last.map(DiaryFoldEvent::event_id_fold),
        work_packages: work_packages.entries,
        summary,
    })
}

fn source_safe_mission_slug(
    slug: &PythonJsonValue,
    fallback: &str,
) -> Result<PythonJsonValue, SourceFoldError> {
    if !python_json_truthy(slug) {
        return Ok(PythonJsonValue::scalar(fallback));
    }
    match slug {
        PythonJsonValue::String(text) => {
            let safe = text
                .to_scalar_string()
                .ok()
                .is_some_and(|text| crate::replay::is_safe_path_segment(&text));
            if safe {
                Ok(slug.clone())
            } else {
                Ok(PythonJsonValue::scalar(fallback))
            }
        }
        other => Err(SourceFoldError::attribute_error(format!(
            "'{}' object has no attribute 'strip'",
            python_json_type_name(other)
        ))),
    }
}

fn dedup_transitions<E: DiaryFoldEvent>(transitions: &[E]) -> Result<Vec<E>, SourceFoldError> {
    let mut order: Vec<PythonJsonValue> = Vec::new();
    let mut groups: SourceKeyMap<Vec<E>> = SourceKeyMap::default();
    for event in transitions {
        let event_id = event.event_id_fold();
        if groups.get(&event_id).is_none() {
            order.push(event_id.clone());
        }
        if let Some(group) = groups.get_mut(&event_id) {
            group.push(event.clone());
        } else {
            groups.insert(event_id, vec![event.clone()])?;
        }
    }
    let mut unique = Vec::with_capacity(order.len());
    for event_id in order {
        let rows = groups
            .entries
            .iter()
            .find(|(candidate, _)| python_json_key_equal(candidate, &event_id))
            .map(|(_, rows)| rows.clone())
            .unwrap_or_default();
        if rows.len() > 1 && rows.iter().all(is_reconstruction_seed) {
            let mut winner = &rows[0];
            for row in &rows[1..] {
                if row.canonical_key_fold() < winner.canonical_key_fold() {
                    winner = row;
                }
            }
            unique.push(winner.clone());
        } else if let Some(first) = rows.first() {
            unique.push(first.clone());
        }
    }
    Ok(unique)
}

fn order_transitions<E: DiaryFoldEvent>(events: &[E]) -> Result<Vec<E>, SourceFoldError> {
    let mut approvals: Vec<(PythonJsonValue, PythonJsonValue)> = Vec::new();
    for event in events {
        if event.source_to_lane() == Lane::Approved
            && event.source_from_lane() != Lane::Approved
            && !is_reconstruction_seed(event)
        {
            let wp_id = event.wp_id_fold();
            let at = event.at_fold();
            python_json_tuple_hashable(&[wp_id.clone(), at.clone()])?;
            approvals.push((wp_id, at));
        }
    }
    let lifted = approved_departure_sort_at(events)?;
    let (effective, seq) = carry_lifted_sort_at(events, &lifted)?;

    // Python's sort key evaluates `e.event_id in seq or (e.from_lane == APPROVED
    // and not seed and (e.wp_id, e.at) in approvals)`. The tuple membership
    // hashes `(wp_id, at)` whenever the first clause is false and the event
    // leaves APPROVED — even for an empty `approvals` set. Reproduce that hash
    // phase here instead of short-circuiting it away.
    let mut second: SourceKeyMap<bool> = SourceKeyMap::default();
    for event in events {
        let event_id = event.event_id_fold();
        let in_seq = seq.contains(&event_id)?;
        if in_seq {
            second.insert(event_id, true)?;
            continue;
        }
        let mut value = false;
        if event.source_from_lane() == Lane::Approved && !is_reconstruction_seed(event) {
            let wp_id = event.wp_id_fold();
            let at = event.at_fold();
            python_json_tuple_hashable(&[wp_id.clone(), at.clone()])?;
            value = approvals.iter().any(|(approval_wp, approval_at)| {
                python_json_key_equal(approval_wp, &wp_id)
                    && python_json_key_equal(approval_at, &at)
            });
        }
        second.insert(event_id, value)?;
    }

    let mut ordered: Vec<E> = events.to_vec();
    python_json_stable_sort_by_key(&mut ordered, |event| {
        let event_id = event.event_id_fold();
        let at = effective
            .get(&event_id)
            .cloned()
            .unwrap_or_else(|| event.at_fold());
        let is_second = second.get(&event_id).copied().unwrap_or(false);
        let sequence = seq
            .get(&event_id)
            .map(|value| PythonJsonValue::Integer(value.to_string()))
            .unwrap_or_else(|| PythonJsonValue::Integer("0".to_owned()));
        vec![at, PythonJsonValue::Bool(is_second), sequence, event_id]
    })?;
    Ok(ordered)
}

fn approved_departure_sort_at<E: DiaryFoldEvent>(
    events: &[E],
) -> Result<SourceKeyMap<PythonJsonValue>, SourceFoldError> {
    let mut arrivals: SourceKeyMap<Vec<E>> = SourceKeyMap::default();
    let mut departures: SourceKeyMap<Vec<E>> = SourceKeyMap::default();
    for event in events {
        if is_reconstruction_seed(event) {
            continue;
        }
        let wp_id = event.wp_id_fold();
        if event.source_to_lane() == Lane::Approved && event.source_from_lane() != Lane::Approved {
            if let Some(group) = arrivals.get_mut(&wp_id) {
                group.push(event.clone());
            } else {
                arrivals.insert(wp_id, vec![event.clone()])?;
            }
        } else if event.source_from_lane() == Lane::Approved
            && event.source_to_lane() != Lane::Approved
        {
            if let Some(group) = departures.get_mut(&wp_id) {
                group.push(event.clone());
            } else {
                departures.insert(wp_id, vec![event.clone()])?;
            }
        }
    }

    let mut lifted: SourceKeyMap<PythonJsonValue> = SourceKeyMap::default();
    for (wp_id, wp_departures) in departures.entries {
        let mut unbound = arrivals
            .entries
            .iter()
            .find(|(candidate, _)| python_json_key_equal(candidate, &wp_id))
            .map(|(_, rows)| rows.clone())
            .unwrap_or_default();
        sort_events(&mut unbound)?;
        let mut ordered = wp_departures;
        sort_events(&mut ordered)?;

        let mut skewed: Vec<E> = Vec::new();
        for departure in ordered {
            let departure_at = departure.at_fold();
            let mut earlier = None;
            for (index, arrival) in unbound.iter().enumerate() {
                if python_json_le(&arrival.at_fold(), &departure_at)? {
                    earlier = Some(index);
                }
            }
            if let Some(index) = earlier {
                unbound.remove(index);
            } else {
                skewed.push(departure);
            }
        }
        for departure in skewed {
            let departure_at = departure.at_fold();
            let mut later = None;
            for (index, arrival) in unbound.iter().enumerate() {
                if python_json_lt(&departure_at, &arrival.at_fold())? {
                    later = Some(index);
                    break;
                }
            }
            if let Some(index) = later {
                let arrival = unbound.remove(index);
                lifted.insert(departure.event_id_fold(), arrival.at_fold())?;
            }
        }
    }
    Ok(lifted)
}

fn sort_events<E: DiaryFoldEvent>(events: &mut Vec<E>) -> Result<(), SourceFoldError> {
    Ok(python_json_stable_sort_by_key(events, |event| {
        vec![event.at_fold(), event.event_id_fold()]
    })?)
}

fn carry_lifted_sort_at<E: DiaryFoldEvent>(
    events: &[E],
    lifted: &SourceKeyMap<PythonJsonValue>,
) -> Result<(SourceKeyMap<PythonJsonValue>, SourceKeyMap<usize>), SourceFoldError> {
    if lifted.entries.is_empty() {
        return Ok((SourceKeyMap::default(), SourceKeyMap::default()));
    }
    let mut effective: SourceKeyMap<PythonJsonValue> = SourceKeyMap {
        entries: lifted.entries.clone(),
    };
    let mut seq: SourceKeyMap<usize> = SourceKeyMap::default();
    let mut floors: SourceKeyMap<PythonJsonValue> = SourceKeyMap::default();
    let mut ordered: Vec<E> = events.to_vec();
    sort_events(&mut ordered)?;
    for (position, event) in ordered.iter().enumerate() {
        let wp_id = event.wp_id_fold();
        let event_id = event.event_id_fold();
        let floor = floors.get(&wp_id).cloned();
        if let Some(lifted_at) = lifted.get(&event_id) {
            let next = match floor {
                Some(floor) if python_json_lt(&floor, lifted_at)? => lifted_at.clone(),
                Some(floor) => floor,
                None => lifted_at.clone(),
            };
            floors.insert(wp_id, next)?;
            seq.insert(event_id, position + 1)?;
        } else if let Some(floor) = floor
            && python_json_lt(&event.at_fold(), &floor)?
        {
            effective.insert(event_id.clone(), floor)?;
            seq.insert(event_id, position + 1)?;
        }
    }
    Ok((effective, seq))
}

#[derive(Debug, Default, Clone)]
struct SlotProvenance {
    real: HashSet<String>,
    seed: HashSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arbitration {
    Apply,
    LostToRollback,
    LostOnContinuity,
}

fn is_reconstruction_seed<E: DiaryFoldEvent>(event: &E) -> bool {
    let identity = event.actor_identity_fold();
    starts_with(&identity, RECONSTRUCTION_SEED_PREFIX)
}

fn is_rollback_event<E: DiaryFoldEvent>(event: &E) -> bool {
    const REJECTION_TARGETS: [Lane; 3] = [Lane::Planned, Lane::Claimed, Lane::InProgress];
    if event.source_from_lane() == Lane::InReview {
        return REJECTION_TARGETS.contains(&event.source_to_lane());
    }
    if event.source_from_lane() == Lane::ForReview && event.review_ref_present_fold() {
        return REJECTION_TARGETS.contains(&event.source_to_lane());
    }
    false
}

fn is_rollback_seed<E: DiaryFoldEvent>(event: &E) -> bool {
    is_reconstruction_seed(event) && is_rollback_event(event)
}

fn is_bare_sibling<E: DiaryFoldEvent>(
    approval: &E,
    seed: &E,
    all_events: &[E],
) -> Result<bool, SourceFoldError> {
    if is_rollback_event(approval)
        || approval.source_to_lane() != Lane::Approved
        || approval.source_from_lane() != seed.source_from_lane()
    {
        return Ok(false);
    }
    let key = vec![approval.at_fold(), approval.event_id_fold()];
    let mut later = false;
    for event in all_events {
        let candidate = vec![event.at_fold(), event.event_id_fold()];
        if python_json_key_equal(&event.wp_id_fold(), &approval.wp_id_fold())
            && event.source_from_lane() == Lane::Approved
            && !is_reconstruction_seed(event)
            && python_json_tuple_lt(&key, &candidate)?
        {
            later = true;
            break;
        }
    }
    Ok(!later)
}

fn yields_to_real<E: DiaryFoldEvent>(
    setter: &E,
    new_event: &E,
    all_events: &[E],
) -> Result<bool, SourceFoldError> {
    Ok(is_rollback_seed(setter)
        && !is_reconstruction_seed(new_event)
        && !is_bare_sibling(new_event, setter, all_events)?)
}

fn arbitrate_event<E: DiaryFoldEvent>(
    current: Option<&SourceWpState>,
    new_event: &E,
    all_events: &[E],
) -> Result<Arbitration, SourceFoldError> {
    let Some(current) = current else {
        return Ok(Arbitration::Apply);
    };
    // Python's `_find_event_by_id(None)` treats a `None` id as "no setter";
    // a JSON `null` event id is that same `None` on the reference's typed event.
    let current_event_id = current
        .get("last_event_id")
        .filter(|event_id| !is_python_null(event_id));
    let current_setter = current_event_id.and_then(|event_id| {
        all_events
            .iter()
            .find(|event| python_json_key_equal(&event.event_id_fold(), event_id))
    });
    let current_timestamp = current.get("last_transition_at");
    let current_lane = current.lane();

    if current_timestamp
        .is_some_and(|timestamp| python_json_key_equal(timestamp, &new_event.at_fold()))
        && let Some(verdict) = same_timestamp_verdict(new_event, current_setter, all_events)?
    {
        return Ok(verdict);
    }

    if let Some(setter) = current_setter
        && !new_event.force_truthy_fold()
        && current_lane != Some(new_event.source_from_lane())
        && !continuity_exempt(new_event, setter, all_events)?
    {
        return Ok(Arbitration::LostOnContinuity);
    }
    Ok(Arbitration::Apply)
}

fn same_timestamp_verdict<E: DiaryFoldEvent>(
    new_event: &E,
    current_setter: Option<&E>,
    all_events: &[E],
) -> Result<Option<Arbitration>, SourceFoldError> {
    let Some(current_setter) = current_setter else {
        return Ok(None);
    };
    if is_rollback_event(new_event) {
        if is_rollback_event(current_setter) {
            return Ok(None);
        }
        if !is_reconstruction_seed(new_event)
            || is_bare_sibling(current_setter, new_event, all_events)?
        {
            return Ok(Some(Arbitration::Apply));
        }
        return Ok(None);
    }
    if is_rollback_event(current_setter) && !yields_to_real(current_setter, new_event, all_events)?
    {
        return Ok(Some(Arbitration::LostToRollback));
    }
    Ok(None)
}

fn continuity_exempt<E: DiaryFoldEvent>(
    new_event: &E,
    current_setter: &E,
    all_events: &[E],
) -> Result<bool, SourceFoldError> {
    if is_rollback_event(new_event) {
        if !is_reconstruction_seed(new_event) {
            return Ok(true);
        }
        return Ok(is_rollback_seed(current_setter)
            || is_bare_sibling(current_setter, new_event, all_events)?);
    }
    if is_reconstruction_seed(new_event) {
        return Ok(false);
    }
    if is_reconstruction_seed(current_setter) && !is_rollback_event(current_setter) {
        return Ok(true);
    }
    yields_to_real(current_setter, new_event, all_events)
}

fn state_from_event<E: DiaryFoldEvent>(
    event: &E,
    previous: Option<&SourceWpState>,
) -> Result<SourceWpState, SourceFoldError> {
    const RUNTIME_SLOTS: [&str; 13] = [
        "shell_pid",
        "shell_pid_created_at",
        "subtasks",
        "notes",
        "tracker_refs",
        "agent",
        "assignee",
        "review",
        "role",
        "agent_profile",
        "agent_profile_version",
        "model",
        "provider",
    ];
    let prior_force_count = previous
        .and_then(|state| state.get("force_count"))
        .and_then(json_integer)
        .unwrap_or(0);
    let mut state = SourceWpState::default();
    state.insert(
        "lane",
        PythonJsonValue::scalar(event.source_to_lane().as_str()),
    );
    state.insert(
        "actor",
        PythonJsonValue::String(event.actor_identity_fold()),
    );
    state.insert("last_transition_at", event.at_fold());
    state.insert("last_event_id", event.event_id_fold());
    state.insert(
        "force_count",
        PythonJsonValue::Integer(
            (prior_force_count + i64::from(event.force_truthy_fold())).to_string(),
        ),
    );
    if let Some(previous) = previous {
        for slot in RUNTIME_SLOTS {
            if let Some(value) = previous.get(slot) {
                state.insert(slot, value.clone());
            }
        }
    }
    for (slot, value) in event.claim_writes_fold()? {
        state.insert(slot, value);
    }
    if event.review_result_present_fold() || event.source_from_lane() == Lane::InReview {
        state.insert(
            "review_result",
            event
                .review_result_value_fold()
                .unwrap_or(PythonJsonValue::Null),
        );
    } else if let Some(value) = previous.and_then(|state| state.get("review_result")) {
        state.insert("review_result", value.clone());
    }
    Ok(state)
}

fn json_integer(value: &PythonJsonValue) -> Option<i64> {
    match value {
        PythonJsonValue::Integer(digits) => digits.parse().ok(),
        PythonJsonValue::Bool(flag) => Some(i64::from(*flag)),
        PythonJsonValue::Float(number) => Some(*number as i64),
        _ => None,
    }
}

fn record_applied_event<E: DiaryFoldEvent>(
    event: &E,
    state: &mut SourceWpState,
    provenance: &mut SlotProvenance,
) -> Result<(), SourceFoldError> {
    let writes = event.claim_writes_fold()?;
    let mut wrote: HashSet<String> = writes.keys().cloned().collect();
    if event.review_result_present_fold() || event.source_from_lane() == Lane::InReview {
        wrote.insert("review_result".to_owned());
    }
    if is_reconstruction_seed(event) && !is_rollback_event(event) {
        provenance.seed.extend(wrote);
        return Ok(());
    }
    if writes.contains_key("shell_pid")
        && !writes.contains_key("shell_pid_created_at")
        && provenance.seed.contains("shell_pid_created_at")
    {
        state.remove("shell_pid_created_at");
        provenance.seed.remove("shell_pid_created_at");
    }
    provenance.real.extend(wrote.iter().cloned());
    for slot in wrote {
        provenance.seed.remove(&slot);
    }
    Ok(())
}

fn fold_losing_seed<E: DiaryFoldEvent>(
    event: &E,
    state: &mut SourceWpState,
    provenance: &mut SlotProvenance,
) -> Result<(), SourceFoldError> {
    for (slot, value) in event.claim_writes_fold()? {
        if provenance.real.contains(&slot) {
            continue;
        }
        if slot == "shell_pid_created_at" && provenance.real.contains("shell_pid") {
            continue;
        }
        state.insert(&slot, value);
        provenance.seed.insert(slot);
    }
    if let Some(result) = event.review_result_value_fold()
        && !provenance.real.contains("review_result")
    {
        state.insert("review_result", result);
        provenance.seed.insert("review_result".to_owned());
    }
    Ok(())
}

/// One annotation as the fold consumes it (`(at, event_id)` order, per-WP key,
/// the `uninitialized` seed's actor identity, and its delta).
///
/// Two implementations exist — [`SourceAnnotation`] (the Source-valued row) and
/// [`crate::replay::InnerStateChanged`] (the finite typed diary view) — and both
/// funnel into the ONE [`apply_annotation_delta`] body below.
pub(crate) trait FoldAnnotation {
    fn annotation_at(&self) -> PythonString;
    fn annotation_event_id(&self) -> PythonString;
    fn annotation_wp_id(&self) -> PythonString;
    fn annotation_actor_identity(&self) -> PythonString;
    fn apply_annotation(&self, state: &mut SourceWpState) -> Result<(), SourceFoldError>;
}

/// One delta's slots, as the fold reads them.
pub(crate) trait FoldDelta {
    fn release_runtime_claim(&self) -> bool;
    fn replace_slot(&self, name: &str) -> Option<PythonJsonValue>;
    fn subtasks(&self) -> Option<Vec<(PythonString, PythonString)>>;
    fn note(&self) -> Option<PythonJsonValue>;
    fn tracker_refs(&self) -> Option<Vec<PythonJsonValue>>;
    fn tracker_refs_replace(&self) -> Option<Vec<PythonJsonValue>>;
    fn review(&self) -> Option<ReviewFold>;
}

/// How the replace channel de-duplicates: the reference's `set` (hash), or the
/// finite typed view's equality-only rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefsDedup {
    PythonHash,
    Equality,
}

impl FoldAnnotation for SourceAnnotation {
    fn annotation_at(&self) -> PythonString {
        self.at().clone()
    }

    fn annotation_event_id(&self) -> PythonString {
        self.event_id().clone()
    }

    fn annotation_wp_id(&self) -> PythonString {
        self.wp_id().clone()
    }

    fn annotation_actor_identity(&self) -> PythonString {
        self.actor_identity()
    }

    fn apply_annotation(&self, state: &mut SourceWpState) -> Result<(), SourceFoldError> {
        apply_annotation_delta(state, self.delta(), RefsDedup::PythonHash)
    }
}

impl FoldDelta for SourceDelta {
    fn release_runtime_claim(&self) -> bool {
        self.release_runtime_claim
    }

    fn replace_slot(&self, name: &str) -> Option<PythonJsonValue> {
        if name == "shell_pid" {
            return self.shell_pid.clone();
        }
        self.scalars.get(name).cloned()
    }

    fn subtasks(&self) -> Option<Vec<(PythonString, PythonString)>> {
        self.subtasks.as_ref().map(|subtasks| {
            subtasks
                .iter()
                .filter_map(|(id, status)| match status {
                    PythonJsonValue::String(text) => Some((id.clone(), text.clone())),
                    _ => None,
                })
                .collect()
        })
    }

    fn note(&self) -> Option<PythonJsonValue> {
        self.note.clone()
    }

    fn tracker_refs(&self) -> Option<Vec<PythonJsonValue>> {
        self.tracker_refs.clone()
    }

    fn tracker_refs_replace(&self) -> Option<Vec<PythonJsonValue>> {
        self.tracker_refs_replace.clone()
    }

    fn review(&self) -> Option<ReviewFold> {
        self.review.as_ref().map(|review| {
            if review.is_release_sentinel() {
                ReviewFold::Clear
            } else {
                ReviewFold::Value(review.to_value())
            }
        })
    }
}

/// The typed diary view projected onto the same slots.
struct TypedAnnotationDelta<'a>(&'a crate::replay::InnerStateDelta);

impl FoldAnnotation for crate::replay::InnerStateChanged {
    fn annotation_at(&self) -> PythonString {
        PythonString::from_scalars(&self.at)
    }

    fn annotation_event_id(&self) -> PythonString {
        PythonString::from_scalars(&self.event_id)
    }

    fn annotation_wp_id(&self) -> PythonString {
        PythonString::from_scalars(&self.wp_id)
    }

    fn annotation_actor_identity(&self) -> PythonString {
        PythonString::from_scalars(&self.actor.identity())
    }

    fn apply_annotation(&self, state: &mut SourceWpState) -> Result<(), SourceFoldError> {
        // The typed view is a finite capability (its own decoder already
        // normalized these values), so its replace channel keeps its
        // equality-only de-duplication: an unhashable member must not turn a
        // total typed reducer into a refusal.
        apply_annotation_delta(
            state,
            &TypedAnnotationDelta(&self.delta),
            RefsDedup::Equality,
        )
    }
}

impl FoldDelta for TypedAnnotationDelta<'_> {
    fn release_runtime_claim(&self) -> bool {
        self.0.release_runtime_claim
    }

    fn replace_slot(&self, name: &str) -> Option<PythonJsonValue> {
        let value = if name == "shell_pid" {
            self.0.shell_pid.as_ref()
        } else {
            self.0.scalar(name)
        };
        value
            .filter(|value| !matches!(value, serde_json::Value::String(text) if text.is_empty()))
            .map(PythonJsonValue::from_serde)
    }

    fn subtasks(&self) -> Option<Vec<(PythonString, PythonString)>> {
        self.0.subtasks.as_ref().map(|subtasks| {
            subtasks
                .iter()
                .map(|(id, status)| {
                    (
                        PythonString::from_scalars(id),
                        PythonString::from_scalars(status),
                    )
                })
                .collect()
        })
    }

    fn note(&self) -> Option<PythonJsonValue> {
        self.0.note.as_ref().map(PythonJsonValue::from_serde)
    }

    fn tracker_refs(&self) -> Option<Vec<PythonJsonValue>> {
        self.0
            .tracker_refs
            .as_ref()
            .map(|refs| refs.iter().map(PythonJsonValue::from_serde).collect())
    }

    fn tracker_refs_replace(&self) -> Option<Vec<PythonJsonValue>> {
        self.0
            .tracker_refs_replace
            .as_ref()
            .map(|refs| refs.iter().map(PythonJsonValue::from_serde).collect())
    }

    fn review(&self) -> Option<ReviewFold> {
        self.0.review.as_ref().map(|review| {
            if review.is_release_sentinel() {
                ReviewFold::Clear
            } else {
                ReviewFold::Value(PythonJsonValue::from_serde(&review.to_json_value()))
            }
        })
    }
}

/// Seed a per-WP snapshot entry for an annotation with no prior transition.
///
/// Such a WP never traversed the FSM, so it sits in the non-display
/// `uninitialized` lane (excluded from the board summary); the delta then folds
/// its runtime slots on top. The actor is projected to its string identity, so a
/// structured annotation actor never lands as a raw dict in a display sink.
fn runtime_only_wp_state(actor_identity: &PythonString) -> SourceWpState {
    let mut state = SourceWpState::default();
    state.insert(
        "lane",
        PythonJsonValue::scalar(Lane::Uninitialized.as_str()),
    );
    state.insert("actor", PythonJsonValue::String(actor_identity.clone()));
    state.insert("last_transition_at", PythonJsonValue::Null);
    state.insert("last_event_id", PythonJsonValue::Null);
    state.insert("force_count", PythonJsonValue::Integer("0".to_owned()));
    state
}

/// Fold one annotation delta into a per-WP snapshot state — the ONE body.
///
/// Reference order (never increments `force_count`):
///
/// 1. `release_runtime_claim` clears the claim triple FIRST, so a concrete value
///    carried in the same delta still wins;
/// 2. every present replace slot folds latest-wins, and an empty string is a
///    no-op (it must never clobber a real recorded value);
/// 3. `subtasks` merges per subtask id;
/// 4. `note` appends to the `notes` list (a slot-name mismatch by design);
/// 5. `tracker_refs_replace` wholesale-replaces (dedup preserving first-seen
///    order) and takes precedence over the additive `tracker_refs` union;
/// 6. `review` replaces, except the all-empty release sentinel, which clears.
fn apply_annotation_delta(
    state: &mut SourceWpState,
    delta: &dyn FoldDelta,
    dedup: RefsDedup,
) -> Result<(), SourceFoldError> {
    if delta.release_runtime_claim() {
        for slot in CLAIM_RELEASE_SLOTS {
            state.insert(slot, PythonJsonValue::Null);
        }
    }
    for slot in ["shell_pid"].into_iter().chain(DELTA_SCALAR_FIELDS) {
        if let Some(value) = delta.replace_slot(slot) {
            let blank = matches!(&value, PythonJsonValue::String(text) if text.is_empty());
            if !blank {
                state.insert(slot, value);
            }
        }
    }
    if let Some(subtasks) = delta.subtasks() {
        let mut current = state
            .get("subtasks")
            .and_then(normalize_object)
            .unwrap_or_default();
        for (id, status) in subtasks {
            current.insert(id, PythonJsonValue::String(status));
        }
        state.insert("subtasks", PythonJsonValue::ExactObject(current));
    }
    if let Some(note) = delta.note() {
        let mut notes = state
            .get("notes")
            .and_then(sequence_value)
            .map(<[PythonJsonValue]>::to_vec)
            .unwrap_or_default();
        notes.push(note);
        state.insert("notes", PythonJsonValue::Array(notes));
    }
    if let Some(replacement) = delta.tracker_refs_replace() {
        state.insert(
            "tracker_refs",
            PythonJsonValue::Array(dedup_preserve_order(&replacement, dedup)?),
        );
    } else if let Some(additive) = delta.tracker_refs() {
        let mut merged = state
            .get("tracker_refs")
            .and_then(sequence_value)
            .map(<[PythonJsonValue]>::to_vec)
            .unwrap_or_default();
        for reference in additive {
            if !merged
                .iter()
                .any(|held| python_json_key_equal(held, &reference))
            {
                merged.push(reference);
            }
        }
        state.insert("tracker_refs", PythonJsonValue::Array(merged));
    }
    if let Some(review) = delta.review() {
        let value = match review {
            ReviewFold::Clear => PythonJsonValue::Null,
            ReviewFold::Value(value) => value,
        };
        state.insert("review", value);
    }
    Ok(())
}

/// De-duplicate preserving first-seen order.
///
/// The reference's `_dedup_preserve_order` accumulates into a `set`, so an
/// unhashable member raises `TypeError: unhashable type: '…'`; equality alone
/// would silently accept it. The Source path reproduces that hash step, the
/// finite typed view keeps its equality-only rule (see [`RefsDedup`]).
fn dedup_preserve_order(
    refs: &[PythonJsonValue],
    dedup: RefsDedup,
) -> Result<Vec<PythonJsonValue>, SourceFoldError> {
    let mut result: Vec<PythonJsonValue> = Vec::new();
    for reference in refs {
        if dedup == RefsDedup::PythonHash {
            python_json_hashable(reference)?;
        }
        if !result
            .iter()
            .any(|held| python_json_key_equal(held, reference))
        {
            result.push(reference.clone());
        }
    }
    Ok(result)
}

fn project_snapshot(snapshot: SourceSnapshot) -> crate::snapshot::Snapshot {
    let work_packages = snapshot
        .work_packages
        .into_iter()
        .map(|(key, state)| {
            let name = scalar_identity(&key).expect("the finite typed diary keeps scalar ids");
            let mut snapshot_state = crate::snapshot::WorkPackageState::default();
            for (slot, value) in state.iter() {
                if let Some(value) = value.to_serde() {
                    snapshot_state.as_map_mut().insert(slot.clone(), value);
                }
            }
            (name, snapshot_state)
        })
        .collect();
    crate::snapshot::Snapshot {
        mission_slug: scalar_identity(&snapshot.mission_slug).unwrap_or_default(),
        materialized_at: scalar_identity(&snapshot.materialized_at).unwrap_or_default(),
        event_count: snapshot.event_count,
        last_event_id: snapshot.last_event_id.as_ref().and_then(scalar_identity),
        work_packages,
        summary: snapshot.summary,
        mission_number: None,
        mission_type: None,
        retrospective: None,
        schema_version: None,
    }
}

fn scalar_identity(value: &PythonJsonValue) -> Option<String> {
    match value {
        PythonJsonValue::String(text) => text.to_scalar_string().ok(),
        PythonJsonValue::Integer(digits) => Some(digits.clone()),
        PythonJsonValue::Bool(flag) => Some(if *flag { "True" } else { "False" }.to_owned()),
        PythonJsonValue::Null => Some("None".to_owned()),
        PythonJsonValue::Float(number) => Some(sk_kernel::python_json::python_float_repr(*number)),
        _ => None,
    }
}
