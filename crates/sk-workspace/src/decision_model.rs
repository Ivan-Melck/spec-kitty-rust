//! The ONE canonical typed `DecisionIndex` model: validation, defaults,
//! coercion, the datetime representation and the JSON projection.
//!
//! # Why this module exists
//!
//! Source validates the whole `decisions/index.json` document through
//! `specify_cli.decisions.models.DecisionIndex` (`model_validate`, pydantic v2
//! lax mode). Two callers need exactly that model and nothing else:
//!
//! * the merge driver's typed projection (`merge_drivers::exact`), which unions
//!   the raw entries and then projects the union through the model; and
//! * the read-only store ([`crate::decision_store`]), which loads a stored
//!   index through the SAME model instead of re-parsing or raw-union reading.
//!
//! Before this module the model lived inside the merge driver, where the store
//! could not reuse it without also importing merge policy (union rules, conflict
//! precedence, `%A` publication). The typed authority is therefore promoted here
//! — one implementation, two callers. There is no second validator, parser or
//! projection: `exact.rs` keeps the RAW UNION policy and delegates every typed
//! decision to this module.
//!
//! # The contract (Source `specify_cli/decisions/models.py`)
//!
//! * `DecisionIndex` (`extra="forbid"`, frozen): `version: Literal[1] = 1`,
//!   `mission_id: str` (required, no length rule),
//!   `entries: tuple[IndexEntry, ...] = ()`.
//! * `IndexEntry` (`extra="forbid"`, frozen): the 18 fields in
//!   `DECISION_ENTRY_FIELDS`; `input_key`/`question` are `min_length=1`; the
//!   `_step_or_slot` model validator requires a truthy `step_id` or `slot_key`.
//! * `OriginFlow`/`DecisionStatus` are `StrEnum`s: exact member values only.
//! * `options: tuple[str, ...]` (JSON array of strings), `summary_json:
//!   dict[str, str] | None`, `other_answer: bool` with pydantic's lax boolean
//!   coercion, `created_at: datetime` (required), `resolved_at: datetime | None`.
//! * `model_dump(mode="json")` renders EVERY field (an omitted field takes its
//!   default, never a dropped key) and renders datetimes as ISO 8601 with a `Z`
//!   UTC suffix, a preserved offset, or no suffix when naive.
//!
//! The datetime domain is pydantic's, established from the pinned CPython
//! 3.11.15 / pydantic 2.13.3 oracle: ISO 8601 text (date only, `T`/`t`/space/`_`
//! separator, optional seconds, `.`/`,` fraction truncated to microseconds and
//! only after parsed seconds, `Z`/`z`/`±HH:MM`/`±HHMM` offsets), or an
//! integer/float Unix timestamp (seconds up to 2e10 inclusive, milliseconds
//! above that), with the observed pydantic handling of a negative fractional
//! timestamp. `DecisionMoment::python_isoformat` renders Python's
//! `datetime.isoformat()` — the driver's SORT key, which spells UTC as
//! `+00:00` where the JSON projection spells it `Z` — from the same value.
//!
//! # Deliberate non-claims
//!
//! * The authorized exception (unpaired-surrogate keys inside
//!   `IndexEntry.summary_json`) is a JSON-*mode serializer* effect in Source, so
//!   this model — like Source's raw union and `model_validate` — preserves every
//!   authored key and value here. Only the merge driver's serialization carries
//!   the approved, separately tested deviation.
//! * Pydantic's `ValidationError` wire text is NOT reproduced. A refusal is a
//!   typed [`DecisionModelError`] naming the document, the entry and the field;
//!   reproducing the pinned traceback prose is separate error-wire work (the H5
//!   F11 family) and is never fabricated here.
//! * No union, no conflict resolution and no publication live in this module:
//!   validating a document never mutates it, never picks a side and never
//!   writes bytes.

use sk_kernel::PythonString;
use sk_kernel::python_json::{PythonJsonDict, PythonJsonValue, python_repr_value};

/// The document these refusals name (Source's `decisions/index.json`).
pub const DECISION_INDEX_DOCUMENT: &str = "decisions/index.json";

/// A Python `str` identity key: exact code points, Python order and equality.
pub(crate) type Key = Vec<u32>;

/// Source's projection sort key: `(created_at.isoformat(), decision_id)`.
pub type DecisionSortKey = (Key, Key);

/// Source `DecisionStatus` members, in declaration order.
const DECISION_STATUSES: [&str; 4] = ["open", "resolved", "deferred", "canceled"];

/// Source `OriginFlow` members, in declaration order.
const DECISION_ORIGIN_FLOWS: [&str; 3] = ["charter", "specify", "plan"];

/// Source `DecisionIndex` required entry fields, in declaration order.
const DECISION_REQUIRED: [&str; 8] = [
    "decision_id",
    "origin_flow",
    "input_key",
    "question",
    "status",
    "created_at",
    "mission_id",
    "mission_slug",
];

/// Source `IndexEntry` fields in declaration order.
///
/// `model_dump(mode="json")` emits every field, so the projection does too: an
/// omitted field takes its model default (`null`, `[]`, `false`), never a
/// dropped key.
const DECISION_ENTRY_FIELDS: [&str; 18] = [
    "decision_id",
    "origin_flow",
    "step_id",
    "slot_key",
    "input_key",
    "question",
    "options",
    "status",
    "final_answer",
    "rationale",
    "other_answer",
    "summary_json",
    "created_at",
    "resolved_at",
    "resolved_by",
    "opened_by",
    "mission_id",
    "mission_slug",
];

const DECISION_TRUE_WORDS: [&str; 6] = ["true", "t", "yes", "y", "on", "1"];
const DECISION_FALSE_WORDS: [&str; 6] = ["false", "f", "no", "n", "off", "0"];

/// pydantic's timestamp unit boundary: seconds up to and including 2e10,
/// milliseconds above it (`float`/`int` epoch inputs).
const DECISION_EPOCH_SECONDS_LIMIT: f64 = 20_000_000_000.0;

/// The UTC microseconds range of a Python `datetime` (0001-01-01 .. 9999-12-31).
const DECISION_MIN_EPOCH_MICROS: i128 = -62_135_596_800_000_000;
const DECISION_MAX_EPOCH_MICROS: i128 = 253_402_300_799_999_999;

/// The exact-name view of one value.
type Exact = PythonJsonValue;

// ---------------------------------------------------------------------------
// the typed status (one enum authority for both callers)
// ---------------------------------------------------------------------------

/// Source `DecisionStatus` — the closed set of ledger states.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DecisionStatus {
    /// `open` — the question is still blocking.
    Open,
    /// `resolved` — terminal, answered.
    Resolved,
    /// `deferred` — terminal for acceptance, still recorded for verify drift.
    Deferred,
    /// `canceled` — terminal, withdrawn.
    Canceled,
}

impl DecisionStatus {
    /// Every member, in Source declaration order.
    pub const ALL: [Self; 4] = [Self::Open, Self::Resolved, Self::Deferred, Self::Canceled];

    /// The exact `StrEnum` value written to the ledger.
    pub fn as_str(self) -> &'static str {
        DECISION_STATUSES[self as usize]
    }

    /// The member whose exact value is `text`, or `None` (never a case-folded
    /// alias: Source compares `StrEnum` members by exact value).
    pub fn from_text(text: &PythonString) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|status| text_is(text, status.as_str()))
    }
}

// ---------------------------------------------------------------------------
// validated model types
// ---------------------------------------------------------------------------

/// One validated `IndexEntry`, with its exact JSON projection.
///
/// `fields` is the `model_dump(mode="json")` row: every field, defaults
/// included, datetimes normalised, `options` as an array, `summary_json`
/// preserving every authored key and value. It is a read-only view — a caller
/// that mutates its own copy cannot mutate this model, and the model never
/// writes bytes.
#[derive(Clone, Debug)]
pub struct DecisionEntry {
    decision_id: PythonString,
    status: DecisionStatus,
    sort_key: DecisionSortKey,
    fields: PythonJsonDict,
}

impl DecisionEntry {
    /// The exact authored `decision_id` (code points, surrogates included).
    pub fn decision_id(&self) -> &PythonString {
        &self.decision_id
    }

    /// The validated status.
    pub fn status(&self) -> DecisionStatus {
        self.status
    }

    /// Source's projection sort key for this entry.
    pub fn sort_key(&self) -> &DecisionSortKey {
        &self.sort_key
    }

    /// The whole projected row.
    pub fn fields(&self) -> &PythonJsonDict {
        &self.fields
    }

    /// One projected field, by its Source name.
    pub fn field(&self, name: &str) -> Option<&Exact> {
        self.fields.get(name)
    }
}

/// A validated `DecisionIndex` document.
///
/// `entries` keeps the DOCUMENT order (the store's and Source's `load_index`
/// order). [`Self::project_sorted`] is the projection the merge driver writes,
/// which Source sorts by `(created_at, decision_id)`.
#[derive(Clone, Debug)]
pub struct DecisionIndex {
    version: Exact,
    mission_id: PythonString,
    entries: Vec<DecisionEntry>,
}

impl DecisionIndex {
    /// Source's synthesized EMPTY index for a MISSING `index.json`:
    /// `DecisionIndex(mission_id="", entries=())` (so `version` takes its
    /// model default `1`). Never used for a present-but-unreadable file.
    pub fn empty() -> Self {
        Self {
            version: Exact::Integer("1".to_owned()),
            mission_id: PythonString::default(),
            entries: Vec::new(),
        }
    }

    /// The validated `mission_id` (empty for Source's synthesized empty index).
    pub fn mission_id(&self) -> &PythonString {
        &self.mission_id
    }

    /// The validated `version` — always the exact integer `1`.
    pub fn version(&self) -> &Exact {
        &self.version
    }

    /// The validated entries, in document order.
    pub fn entries(&self) -> &[DecisionEntry] {
        &self.entries
    }

    /// The whole document projected the way the merge driver writes it:
    /// `{"version": 1, "mission_id": <exact>, "entries": [...]}` with the
    /// entries sorted by [`DecisionEntry::sort_key`].
    pub fn project_sorted(&self) -> Exact {
        let mut ordered = PythonJsonDict::new();
        insert(&mut ordered, "version", self.version.clone());
        insert(
            &mut ordered,
            "mission_id",
            Exact::String(self.mission_id.clone()),
        );
        let mut sorted: Vec<&DecisionEntry> = self.entries.iter().collect();
        sorted.sort_by(|left, right| left.sort_key.cmp(&right.sort_key));
        insert(
            &mut ordered,
            "entries",
            Exact::Array(
                sorted
                    .into_iter()
                    .map(|entry| Exact::ExactObject(entry.fields.clone()))
                    .collect(),
            ),
        );
        Exact::ExactObject(ordered)
    }
}

// ---------------------------------------------------------------------------
// the typed, model-independent refusal
// ---------------------------------------------------------------------------

/// A typed `DecisionIndex` refusal.
///
/// The error is model-independent on purpose: it names the document, the entry
/// label and the field, and carries the offending value's canonical Python
/// `repr` where Source would report one. It never mentions merge sides, `%A` or
/// conflict precedence — the merge driver adapts it at its own boundary, and the
/// store wraps it as a read failure. Its `Display` text is the exact text the
/// merge driver has always emitted for these refusals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecisionModelError {
    /// An unknown top-level field (`extra="forbid"`).
    IndexExtraField {
        /// Python `repr` of the offending key.
        key: String,
    },
    /// `version` is not the literal `1`.
    IndexVersionNotLiteral {
        /// Python `repr` of the offending value.
        value: String,
    },
    /// `mission_id` is absent.
    IndexMissingMissionId,
    /// `mission_id` is not a string.
    IndexMissionIdNotString {
        /// Python `repr` of the offending value.
        value: String,
    },
    /// `entries` is present but not a list (`tuple[IndexEntry, ...]`).
    IndexEntriesNotArray {
        /// Python `repr` of the offending value.
        value: String,
    },
    /// One `entries` item is not a JSON object.
    EntryNotObject {
        /// Zero-based position in `entries`.
        index: usize,
    },
    /// An unknown entry field (`extra="forbid"`).
    EntryExtraField {
        /// Entry label (its `decision_id`, or Source's fallback).
        entry: String,
        /// Python `repr` of the offending key.
        key: String,
    },
    /// A required entry field is absent.
    EntryMissingRequired {
        /// Entry label.
        entry: String,
        /// Field name.
        field: &'static str,
    },
    /// A `str` field holds a non-string.
    EntryFieldNotString {
        /// Entry label.
        entry: String,
        /// Field name.
        field: &'static str,
        /// Python `repr` of the offending value.
        value: String,
    },
    /// A `str | None` field holds neither.
    EntryFieldNotStringOrNull {
        /// Entry label.
        entry: String,
        /// Field name.
        field: &'static str,
        /// Python `repr` of the offending value.
        value: String,
    },
    /// A `StrEnum` field holds a non-member.
    EntryFieldNotEnumMember {
        /// Entry label.
        entry: String,
        /// Field name.
        field: &'static str,
        /// Enum name for the diagnostic.
        enum_name: &'static str,
        /// Python `repr` of the offending value.
        value: String,
    },
    /// `options` is not a list.
    EntryOptionsNotArray {
        /// Entry label.
        entry: String,
        /// Python `repr` of the offending value.
        value: String,
    },
    /// One `options` item is not a string.
    EntryOptionNotString {
        /// Entry label.
        entry: String,
        /// Python `repr` of the offending item.
        value: String,
    },
    /// `other_answer` is not a value pydantic's lax boolean accepts.
    EntryOtherAnswerNotBoolean {
        /// Entry label.
        entry: String,
        /// Python `repr` of the offending value.
        value: String,
    },
    /// `summary_json` is neither `null` nor an object.
    EntrySummaryNotObject {
        /// Entry label.
        entry: String,
        /// Python `repr` of the offending value.
        value: String,
    },
    /// One `summary_json` value is not a string (keys are never constrained).
    EntrySummaryValueNotString {
        /// Entry label.
        entry: String,
        /// Python `repr` of the offending key.
        key: String,
        /// Python `repr` of the offending value.
        value: String,
    },
    /// A datetime field is outside pydantic's domain.
    EntryFieldNotDatetime {
        /// Entry label.
        entry: String,
        /// Field name.
        field: &'static str,
        /// Python `repr` of the offending value.
        value: String,
    },
    /// A `min_length=1` field holds a non-scalar (unpaired-surrogate) string.
    EntryConstrainedNotScalar {
        /// Entry label.
        entry: String,
        /// Field name.
        field: &'static str,
    },
    /// A `min_length=1` field holds the empty string.
    EntryConstrainedEmpty {
        /// Entry label.
        entry: String,
        /// Field name.
        field: &'static str,
    },
    /// Neither `step_id` nor `slot_key` is truthy (`IndexEntry` model validator).
    EntryMissingStepOrSlot {
        /// Entry label.
        entry: String,
    },
    /// Internal invariant: the projected row lost its `decision_id`.
    EntryLostDecisionId {
        /// Entry label.
        entry: String,
    },
}

impl DecisionModelError {
    /// The entry this refusal is about, when it is about one entry.
    pub fn entry(&self) -> Option<&str> {
        match self {
            Self::IndexExtraField { .. }
            | Self::IndexVersionNotLiteral { .. }
            | Self::IndexMissingMissionId
            | Self::IndexMissionIdNotString { .. }
            | Self::IndexEntriesNotArray { .. }
            | Self::EntryNotObject { .. } => None,
            Self::EntryExtraField { entry, .. }
            | Self::EntryMissingRequired { entry, .. }
            | Self::EntryFieldNotString { entry, .. }
            | Self::EntryFieldNotStringOrNull { entry, .. }
            | Self::EntryFieldNotEnumMember { entry, .. }
            | Self::EntryOptionsNotArray { entry, .. }
            | Self::EntryOptionNotString { entry, .. }
            | Self::EntryOtherAnswerNotBoolean { entry, .. }
            | Self::EntrySummaryNotObject { entry, .. }
            | Self::EntrySummaryValueNotString { entry, .. }
            | Self::EntryFieldNotDatetime { entry, .. }
            | Self::EntryConstrainedNotScalar { entry, .. }
            | Self::EntryConstrainedEmpty { entry, .. }
            | Self::EntryMissingStepOrSlot { entry }
            | Self::EntryLostDecisionId { entry } => Some(entry),
        }
    }

    /// The field this refusal names, when it names one.
    pub fn field(&self) -> Option<&'static str> {
        match self {
            Self::EntryMissingRequired { field, .. }
            | Self::EntryFieldNotString { field, .. }
            | Self::EntryFieldNotStringOrNull { field, .. }
            | Self::EntryFieldNotEnumMember { field, .. }
            | Self::EntryFieldNotDatetime { field, .. }
            | Self::EntryConstrainedNotScalar { field, .. }
            | Self::EntryConstrainedEmpty { field, .. } => Some(field),
            Self::IndexExtraField { .. }
            | Self::IndexVersionNotLiteral { .. }
            | Self::IndexMissingMissionId
            | Self::IndexMissionIdNotString { .. }
            | Self::IndexEntriesNotArray { .. }
            | Self::EntryNotObject { .. }
            | Self::EntryExtraField { .. }
            | Self::EntryOptionsNotArray { .. }
            | Self::EntryOptionNotString { .. }
            | Self::EntryOtherAnswerNotBoolean { .. }
            | Self::EntrySummaryNotObject { .. }
            | Self::EntrySummaryValueNotString { .. }
            | Self::EntryMissingStepOrSlot { .. }
            | Self::EntryLostDecisionId { .. } => None,
        }
    }
}

impl std::fmt::Display for DecisionModelError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IndexExtraField { key } => write!(
                out,
                "{DECISION_INDEX_DOCUMENT}: `DecisionIndex` forbids extra top-level field {key}"
            ),
            Self::IndexVersionNotLiteral { value } => write!(
                out,
                "{DECISION_INDEX_DOCUMENT}: `DecisionIndex` field 'version' must be 1 ({value})"
            ),
            Self::IndexMissingMissionId => write!(
                out,
                "{DECISION_INDEX_DOCUMENT}: `DecisionIndex` requires a string field 'mission_id'"
            ),
            Self::IndexMissionIdNotString { value } => write!(
                out,
                "{DECISION_INDEX_DOCUMENT}: `DecisionIndex` field 'mission_id' must be a string \
                 ({value})"
            ),
            Self::IndexEntriesNotArray { value } => write!(
                out,
                "{DECISION_INDEX_DOCUMENT}: `DecisionIndex` field 'entries' must be a list of \
                 entries ({value})"
            ),
            Self::EntryNotObject { index } => write!(
                out,
                "{DECISION_INDEX_DOCUMENT}: entry {index} is not a JSON object"
            ),
            Self::EntryExtraField { entry, key } => write!(
                out,
                "{DECISION_INDEX_DOCUMENT}: entry {entry:?} carries unknown field {key} \
                 (`DecisionIndex` forbids extras)"
            ),
            Self::EntryMissingRequired { entry, field } => write!(
                out,
                "{DECISION_INDEX_DOCUMENT}: entry {entry:?} is missing required field {field:?}"
            ),
            Self::EntryFieldNotString {
                entry,
                field,
                value,
            } => write!(
                out,
                "{DECISION_INDEX_DOCUMENT}: entry {entry:?} field {field:?} must be a string \
                 ({value})"
            ),
            Self::EntryFieldNotStringOrNull {
                entry,
                field,
                value,
            } => write!(
                out,
                "{DECISION_INDEX_DOCUMENT}: entry {entry:?} field {field:?} must be a string or \
                 null ({value})"
            ),
            Self::EntryFieldNotEnumMember {
                entry,
                field,
                enum_name,
                value,
            } => write!(
                out,
                "{DECISION_INDEX_DOCUMENT}: entry {entry:?} field {field:?} is not a {enum_name} \
                 value ({value})"
            ),
            Self::EntryOptionsNotArray { entry, value } => write!(
                out,
                "{DECISION_INDEX_DOCUMENT}: entry {entry:?} field 'options' must be an array of \
                 strings ({value})"
            ),
            Self::EntryOptionNotString { entry, value } => write!(
                out,
                "{DECISION_INDEX_DOCUMENT}: entry {entry:?} field 'options' item must be a \
                 string ({value})"
            ),
            Self::EntryOtherAnswerNotBoolean { entry, value } => write!(
                out,
                "{DECISION_INDEX_DOCUMENT}: entry {entry:?} field 'other_answer' is not a \
                 boolean value pydantic accepts ({value})"
            ),
            Self::EntrySummaryNotObject { entry, value } => write!(
                out,
                "{DECISION_INDEX_DOCUMENT}: entry {entry:?} field 'summary_json' must be null or \
                 an object of string values ({value})"
            ),
            Self::EntrySummaryValueNotString { entry, key, value } => write!(
                out,
                "{DECISION_INDEX_DOCUMENT}: entry {entry:?} field 'summary_json' value for {key} \
                 must be a string ({value})"
            ),
            Self::EntryFieldNotDatetime {
                entry,
                field,
                value,
            } => write!(
                out,
                "{DECISION_INDEX_DOCUMENT}: entry {entry:?} field {field:?} is not a datetime \
                 pydantic accepts ({value})"
            ),
            Self::EntryConstrainedNotScalar { entry, field } => write!(
                out,
                "{DECISION_INDEX_DOCUMENT}: entry {entry:?} field {field:?} must be a valid \
                 unicode string"
            ),
            Self::EntryConstrainedEmpty { entry, field } => write!(
                out,
                "{DECISION_INDEX_DOCUMENT}: entry {entry:?} field {field:?} must have at least 1 \
                 character"
            ),
            Self::EntryMissingStepOrSlot { entry } => write!(
                out,
                "{DECISION_INDEX_DOCUMENT}: entry {entry:?} requires 'step_id' or 'slot_key' \
                 (`IndexEntry` model validator)"
            ),
            Self::EntryLostDecisionId { entry } => write!(
                out,
                "{DECISION_INDEX_DOCUMENT}: entry {entry:?} lost its 'decision_id' during \
                 projection"
            ),
        }
    }
}

impl std::error::Error for DecisionModelError {}

// ---------------------------------------------------------------------------
// validation
// ---------------------------------------------------------------------------

/// Validate one parsed `decisions/index.json` document as `DecisionIndex`.
///
/// Source `DecisionIndex.model_validate`: top-level extras, `version`,
/// `mission_id`, then every entry (in document order — no union, no sorting, no
/// mutation). A document without `entries` has the model default `()`, exactly
/// like Source.
pub fn validate_index(document: &PythonJsonDict) -> Result<DecisionIndex, DecisionModelError> {
    for (name, _) in document.entries() {
        if !text_is(name, "version") && !text_is(name, "mission_id") && !text_is(name, "entries") {
            return Err(DecisionModelError::IndexExtraField { key: name.repr() });
        }
    }
    let version = match get(document, "version") {
        None => Exact::Integer("1".to_owned()),
        Some(value) => decision_version(value)?,
    };
    let Some(mission_id) = get(document, "mission_id") else {
        return Err(DecisionModelError::IndexMissingMissionId);
    };
    let Some(mission_id) = as_string(mission_id) else {
        return Err(DecisionModelError::IndexMissionIdNotString {
            value: python_repr_value(mission_id),
        });
    };
    let mut entries = Vec::new();
    match get(document, "entries") {
        None => {}
        Some(value) => {
            let Some(items) = as_array(value) else {
                return Err(DecisionModelError::IndexEntriesNotArray {
                    value: python_repr_value(value),
                });
            };
            for (index, item) in items.iter().enumerate() {
                let Some(entry) = as_object(item) else {
                    return Err(DecisionModelError::EntryNotObject { index });
                };
                entries.push(validate_entry(entry)?);
            }
        }
    }
    Ok(DecisionIndex {
        version,
        mission_id: mission_id.clone(),
        entries,
    })
}

/// Validate one `IndexEntry` row exactly as `IndexEntry.model_validate` does.
///
/// The minimal additive seam the canonical fold ([`crate::decision_fold`])
/// needs: one entry document in, the same validated [`DecisionEntry`]
/// projection [`validate_index`] produces for the same row, out. It is a new
/// entry point to the existing validator — never a second model or a second
/// validation rule.
pub fn validate_entry_document(
    entry: &PythonJsonDict,
) -> Result<DecisionEntry, DecisionModelError> {
    validate_entry(entry)
}

/// `version: Literal[1]` — the exact int 1; `1.0` and `True` are 1 to pydantic,
/// a string `"1"` is not.
fn decision_version(value: &Exact) -> Result<Exact, DecisionModelError> {
    let accepted = match value {
        Exact::Integer(token) => token.as_str() == "1",
        Exact::Float(number) => *number == 1.0,
        Exact::Bool(flag) => *flag,
        _ => false,
    };
    if accepted {
        Ok(Exact::Integer("1".to_owned()))
    } else {
        Err(DecisionModelError::IndexVersionNotLiteral {
            value: python_repr_value(value),
        })
    }
}

/// Validate one entry and project it to `IndexEntry.model_dump(mode="json")`.
fn validate_entry(entry: &PythonJsonDict) -> Result<DecisionEntry, DecisionModelError> {
    let label = get_scalar(entry, "decision_id").unwrap_or_else(|| "<no decision_id>".to_owned());
    for (name, _) in entry.entries() {
        if !DECISION_ENTRY_FIELDS
            .iter()
            .any(|field| text_is(name, field))
        {
            return Err(DecisionModelError::EntryExtraField {
                entry: label,
                key: name.repr(),
            });
        }
    }
    for field in DECISION_REQUIRED {
        if get(entry, field).is_none() {
            return Err(DecisionModelError::EntryMissingRequired {
                entry: label,
                field,
            });
        }
    }
    let created_at = decision_moment_field(entry, "created_at", &label)?;
    let mut row = PythonJsonDict::new();
    for field in DECISION_ENTRY_FIELDS {
        let value = match field {
            "decision_id" | "mission_id" | "mission_slug" => {
                Exact::String(decision_text(entry, field, &label)?.clone())
            }
            "origin_flow" => {
                decision_member(entry, field, &label, &DECISION_ORIGIN_FLOWS, "OriginFlow")?
            }
            "status" => {
                decision_member(entry, field, &label, &DECISION_STATUSES, "DecisionStatus")?
            }
            "input_key" | "question" => {
                let text = decision_text(entry, field, &label)?;
                // Pydantic's constrained str validator requires scalar Unicode;
                // unconstrained str fields and the approved summary-key view
                // retain their separate exact-code-point contracts.
                if text.to_scalar_string().is_err() {
                    return Err(DecisionModelError::EntryConstrainedNotScalar {
                        entry: label,
                        field,
                    });
                }
                if text.is_empty() {
                    return Err(DecisionModelError::EntryConstrainedEmpty {
                        entry: label,
                        field,
                    });
                }
                Exact::String(text.clone())
            }
            "step_id" | "slot_key" | "final_answer" | "rationale" | "resolved_by" | "opened_by" => {
                decision_optional_text(entry, field, &label)?
            }
            "options" => decision_options(entry, &label)?,
            "other_answer" => decision_boolean(entry, &label)?,
            "summary_json" => decision_summary(entry, &label)?,
            "created_at" => Exact::String(created_at.json_text()),
            "resolved_at" => match get(entry, field) {
                None | Some(Exact::Null) => Exact::Null,
                Some(value) => {
                    Exact::String(decision_moment_value(value, field, &label)?.json_text())
                }
            },
            _ => {
                return Err(DecisionModelError::EntryMissingRequired {
                    entry: label,
                    field: "decision_id",
                });
            }
        };
        insert(&mut row, field, value);
    }
    let steps_set = get_text(&row, "step_id").is_some_and(|text| !text.is_empty());
    let slots_set = get_text(&row, "slot_key").is_some_and(|text| !text.is_empty());
    if !steps_set && !slots_set {
        return Err(DecisionModelError::EntryMissingStepOrSlot { entry: label });
    }
    let Some(status) = get_text(&row, "status").and_then(DecisionStatus::from_text) else {
        return Err(DecisionModelError::EntryFieldNotEnumMember {
            entry: label,
            field: "status",
            enum_name: "DecisionStatus",
            value: "<projected status>".to_owned(),
        });
    };
    let Some(decision_id) = get_text(&row, "decision_id") else {
        return Err(DecisionModelError::EntryLostDecisionId { entry: label });
    };
    let sort_key = (
        key_of_scalar(&created_at.python_isoformat()),
        key_of_text(decision_id),
    );
    Ok(DecisionEntry {
        decision_id: decision_id.clone(),
        status,
        sort_key,
        fields: row,
    })
}

/// Required-field presence, with the entry's `decision_id` as the label.
fn decision_field<'a>(
    entry: &'a PythonJsonDict,
    field: &'static str,
    label: &str,
) -> Result<&'a Exact, DecisionModelError> {
    get(entry, field).ok_or_else(|| DecisionModelError::EntryMissingRequired {
        entry: label.to_owned(),
        field,
    })
}

/// A required `str` field (`DecisionIndex` never coerces a non-string).
fn decision_text<'a>(
    entry: &'a PythonJsonDict,
    field: &'static str,
    label: &str,
) -> Result<&'a PythonString, DecisionModelError> {
    let value = decision_field(entry, field, label)?;
    as_string(value).ok_or_else(|| DecisionModelError::EntryFieldNotString {
        entry: label.to_owned(),
        field,
        value: python_repr_value(value),
    })
}

/// A `str | None` field: absent and `null` both project as `null`.
fn decision_optional_text(
    entry: &PythonJsonDict,
    field: &'static str,
    label: &str,
) -> Result<Exact, DecisionModelError> {
    match get(entry, field) {
        None | Some(Exact::Null) => Ok(Exact::Null),
        Some(Exact::String(text)) => Ok(Exact::String(text.clone())),
        Some(other) => Err(DecisionModelError::EntryFieldNotStringOrNull {
            entry: label.to_owned(),
            field,
            value: python_repr_value(other),
        }),
    }
}

/// A `StrEnum` member: the exact member value, never a case-folded alias.
fn decision_member(
    entry: &PythonJsonDict,
    field: &'static str,
    label: &str,
    members: &[&str],
    enum_name: &'static str,
) -> Result<Exact, DecisionModelError> {
    let text = decision_text(entry, field, label)?;
    if members.iter().any(|member| text_is(text, member)) {
        return Ok(Exact::String(text.clone()));
    }
    Err(DecisionModelError::EntryFieldNotEnumMember {
        entry: label.to_owned(),
        field,
        enum_name,
        value: text.repr(),
    })
}

/// `options: tuple[str, ...]` — a JSON array whose items are all strings.
fn decision_options(entry: &PythonJsonDict, label: &str) -> Result<Exact, DecisionModelError> {
    let Some(value) = get(entry, "options") else {
        return Ok(Exact::Array(Vec::new()));
    };
    let Some(items) = as_array(value) else {
        return Err(DecisionModelError::EntryOptionsNotArray {
            entry: label.to_owned(),
            value: python_repr_value(value),
        });
    };
    let mut options = Vec::with_capacity(items.len());
    for item in items {
        let Some(text) = as_string(item) else {
            return Err(DecisionModelError::EntryOptionNotString {
                entry: label.to_owned(),
                value: python_repr_value(item),
            });
        };
        options.push(Exact::String(text.clone()));
    }
    Ok(Exact::Array(options))
}

/// `other_answer: bool` with pydantic's lax coercion.
fn decision_boolean(entry: &PythonJsonDict, label: &str) -> Result<Exact, DecisionModelError> {
    let Some(value) = get(entry, "other_answer") else {
        return Ok(Exact::Bool(false));
    };
    let coerced = match value {
        Exact::Bool(flag) => Some(*flag),
        Exact::Integer(token) => token.parse::<i128>().ok().and_then(|number| match number {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }),
        Exact::Float(number) => {
            if *number == 0.0 {
                Some(false)
            } else if *number == 1.0 {
                Some(true)
            } else {
                None
            }
        }
        Exact::String(text) => ascii_lower(text).and_then(|word| {
            if DECISION_TRUE_WORDS.contains(&word.as_str()) {
                Some(true)
            } else if DECISION_FALSE_WORDS.contains(&word.as_str()) {
                Some(false)
            } else {
                None
            }
        }),
        _ => None,
    };
    coerced
        .map(Exact::Bool)
        .ok_or_else(|| DecisionModelError::EntryOtherAnswerNotBoolean {
            entry: label.to_owned(),
            value: python_repr_value(value),
        })
}

/// `summary_json: dict[str, str] | None`.
///
/// Keys are NOT constrained — an unpaired-surrogate key is a valid Python `str`
/// and is preserved exactly, like Source's raw union and `model_validate`.
fn decision_summary(entry: &PythonJsonDict, label: &str) -> Result<Exact, DecisionModelError> {
    match get(entry, "summary_json") {
        None | Some(Exact::Null) => Ok(Exact::Null),
        Some(Exact::ExactObject(object)) => {
            for (key, value) in object.entries() {
                if !matches!(value, Exact::String(_)) {
                    return Err(DecisionModelError::EntrySummaryValueNotString {
                        entry: label.to_owned(),
                        key: key.repr(),
                        value: python_repr_value(value),
                    });
                }
            }
            Ok(Exact::ExactObject(object.clone()))
        }
        Some(other) => Err(DecisionModelError::EntrySummaryNotObject {
            entry: label.to_owned(),
            value: python_repr_value(other),
        }),
    }
}

/// `created_at: datetime` / `resolved_at: datetime | None`.
fn decision_moment_field(
    entry: &PythonJsonDict,
    field: &'static str,
    label: &str,
) -> Result<DecisionMoment, DecisionModelError> {
    let value = decision_field(entry, field, label)?;
    decision_moment_value(value, field, label)
}

fn decision_moment_value(
    value: &Exact,
    field: &'static str,
    label: &str,
) -> Result<DecisionMoment, DecisionModelError> {
    let parsed = match value {
        Exact::String(text) => DecisionMoment::from_string(text),
        Exact::Integer(token) => DecisionMoment::from_integer_token(token),
        Exact::Float(number) => DecisionMoment::from_epoch_float(*number),
        _ => None,
    };
    parsed.ok_or_else(|| DecisionModelError::EntryFieldNotDatetime {
        entry: label.to_owned(),
        field,
        value: python_repr_value(value),
    })
}

// ---------------------------------------------------------------------------
// exact-tree accessors (Kernel `PythonJsonValue`; no private parser)
// ---------------------------------------------------------------------------

/// Whether an exact string equals Rust text (code-point comparison).
fn text_is(text: &PythonString, candidate: &str) -> bool {
    text.codepoints().len() == candidate.chars().count()
        && text
            .codepoints()
            .iter()
            .copied()
            .eq(candidate.chars().map(u32::from))
}

/// The member of an exact object under Rust text.
fn get<'a>(object: &'a PythonJsonDict, key: &str) -> Option<&'a Exact> {
    object.get(key)
}

/// The exact string of a member, when present and a string.
fn get_text<'a>(object: &'a PythonJsonDict, key: &str) -> Option<&'a PythonString> {
    get(object, key).and_then(as_string)
}

/// The scalar text of a member (only when scalar).
fn get_scalar(object: &PythonJsonDict, key: &str) -> Option<String> {
    get_text(object, key).and_then(|text| text.to_scalar_string().ok())
}

/// Insert a member from Rust text (position preserved on replacement).
fn insert(object: &mut PythonJsonDict, key: &str, value: Exact) {
    object.insert(PythonString::from_scalars(key), value);
}

/// The object view (`dict`), never the scalar-key producer convenience view.
fn as_object(value: &Exact) -> Option<&PythonJsonDict> {
    match value {
        Exact::ExactObject(object) => Some(object),
        _ => None,
    }
}

/// The list view, or `None` for a non-list.
fn as_array(value: &Exact) -> Option<&[Exact]> {
    match value {
        Exact::Array(items) => Some(items),
        _ => None,
    }
}

/// The exact string view of a value.
fn as_string(value: &Exact) -> Option<&PythonString> {
    match value {
        Exact::String(text) => Some(text),
        _ => None,
    }
}

/// An exact code-point key (Python `str` order and equality).
fn key_of_text(text: &PythonString) -> Key {
    text.codepoints().to_vec()
}

/// A code-point key from Rust text.
fn key_of_scalar(text: &str) -> Key {
    text.chars().map(u32::from).collect()
}

/// ASCII lowercase, or `None` when the text holds a non-ASCII code point
/// (pydantic's boolean words are ASCII-only and case-insensitive).
fn ascii_lower(text: &PythonString) -> Option<String> {
    let mut lowered = String::with_capacity(text.codepoints().len());
    for unit in text.codepoints() {
        if *unit > 0x7F {
            return None;
        }
        lowered.push(char::from(u8::try_from(*unit).ok()?).to_ascii_lowercase());
    }
    Some(lowered)
}

// ---------------------------------------------------------------------------
// the datetime domain
// ---------------------------------------------------------------------------

/// One `datetime` in pydantic's domain: civil fields, microseconds, and an
/// optional UTC offset in seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecisionMoment {
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    micros: u32,
    offset_seconds: Option<i32>,
}

impl DecisionMoment {
    /// Validate the civil fields (`0001-01-01` .. `9999-12-31`, second < 60).
    fn validated(self) -> Option<Self> {
        if !(1..=9999).contains(&self.year) {
            return None;
        }
        if !(1..=12).contains(&self.month) {
            return None;
        }
        if self.day == 0 || self.day > days_in_month(self.year, self.month) {
            return None;
        }
        if self.hour > 23 || self.minute > 59 || self.second > 59 || self.micros > 999_999 {
            return None;
        }
        Some(self)
    }

    /// pydantic `model_dump(mode="json")` text: `Z` for UTC, offset kept.
    fn json_text(self) -> PythonString {
        PythonString::from_scalars(&sk_kernel::clock::model_json_datetime_text(
            &self.python_isoformat(),
        ))
    }

    /// Python `datetime.isoformat()` text: UTC spelled `+00:00` (sort key).
    fn python_isoformat(self) -> String {
        self.rendered()
    }

    /// Python ISO civil/offset text; model-JSON presentation belongs to Kernel.
    fn rendered(self) -> String {
        let mut text = format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        );
        if self.micros != 0 {
            text.push_str(&format!(".{:06}", self.micros));
        }
        match self.offset_seconds {
            None => {}
            Some(offset) => {
                let sign = if offset < 0 { '-' } else { '+' };
                let magnitude = offset.abs();
                text.push_str(&format!(
                    "{sign}{:02}:{:02}",
                    magnitude / 3600,
                    (magnitude % 3600) / 60
                ));
            }
        }
        text
    }

    /// A JSON string in pydantic's datetime domain: ISO 8601 first, then the
    /// numeric-timestamp fallback (an `i64`, or an `f64` only when the text
    /// holds a `.`, exactly as pydantic decides it).
    fn from_string(text: &PythonString) -> Option<Self> {
        if let Some(moment) = Self::from_text(text) {
            return Some(moment);
        }
        let scalar = text.to_scalar_string().ok()?;
        if scalar.parse::<i64>().is_ok() {
            return Self::from_integer_token(&scalar);
        }
        if scalar.contains('.')
            && let Ok(value) = scalar.parse::<f64>()
        {
            return Self::from_epoch_float(value);
        }
        None
    }

    /// ISO 8601 text (pydantic's `datetime` string domain).
    ///
    /// Accepted: `YYYY-MM-DD`, an optional `T`/`t`/space/`_` separator, `HH:MM`
    /// with optional `:SS`, a `.`/`,` fraction (only after parsed seconds), and
    /// `Z`/`z`/`±HH:MM`/`±HHMM`. Any other text is not a datetime, which is
    /// where pydantic's integer/float timestamp fallback takes over.
    fn from_text(text: &PythonString) -> Option<Self> {
        let units = text.codepoints();
        if units.iter().any(|unit| *unit > 0x7F) {
            return None;
        }
        let bytes: Vec<u8> = units.iter().map(|unit| *unit as u8).collect();
        Self::from_ascii(&bytes)
    }

    fn from_ascii(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 10 || bytes[4] != b'-' || bytes[7] != b'-' {
            return None;
        }
        let mut moment = Self {
            year: digits(bytes, 0, 4)? as i32,
            month: digits(bytes, 5, 2)?,
            day: digits(bytes, 8, 2)?,
            hour: 0,
            minute: 0,
            second: 0,
            micros: 0,
            offset_seconds: None,
        };
        let mut rest = &bytes[10..];
        if rest.is_empty() {
            return moment.validated();
        }
        if !matches!(rest[0], b'T' | b't' | b' ' | b'_') {
            return None;
        }
        rest = &rest[1..];
        if rest.len() < 5 || rest[2] != b':' {
            return None;
        }
        moment.hour = digits(rest, 0, 2)?;
        moment.minute = digits(rest, 3, 2)?;
        rest = &rest[5..];
        let has_seconds = rest.first() == Some(&b':');
        if has_seconds {
            if rest.len() < 3 {
                return None;
            }
            moment.second = digits(rest, 1, 2)?;
            rest = &rest[3..];
        }
        if matches!(rest.first(), Some(b'.' | b',')) {
            if !has_seconds {
                return None;
            }
            rest = &rest[1..];
            let count = rest.iter().take_while(|byte| byte.is_ascii_digit()).count();
            if count == 0 {
                return None;
            }
            let mut micros = 0u32;
            let mut scale = 100_000u32;
            for byte in &rest[..count.min(6)] {
                micros += u32::from(byte - b'0') * scale;
                scale /= 10;
            }
            moment.micros = micros;
            rest = &rest[count..];
        }
        if let Some(first) = rest.first() {
            match first {
                b'Z' | b'z' => {
                    if rest.len() != 1 {
                        return None;
                    }
                    moment.offset_seconds = Some(0);
                }
                b'+' | b'-' => {
                    let tail = &rest[1..];
                    let (hours, minutes, used) = if tail.len() == 5 && tail[2] == b':' {
                        (digits(tail, 0, 2)?, digits(tail, 3, 2)?, 5)
                    } else if tail.len() == 4 {
                        (digits(tail, 0, 2)?, digits(tail, 2, 2)?, 4)
                    } else {
                        return None;
                    };
                    if tail.len() != used || hours > 23 || minutes > 59 {
                        return None;
                    }
                    let seconds = (hours * 3600 + minutes * 60) as i32;
                    moment.offset_seconds = Some(if *first == b'-' { -seconds } else { seconds });
                }
                _ => return None,
            }
        }
        moment.validated()
    }

    /// An exact integer JSON token as a Unix timestamp.
    fn from_integer_token(token: &str) -> Option<Self> {
        let value: i128 = token.parse().ok()?;
        if value.unsigned_abs() <= DECISION_EPOCH_SECONDS_LIMIT as u128 {
            return Self::from_epoch_micros(value * 1_000_000);
        }
        if value.unsigned_abs() > 253_402_300_799_999 {
            return None;
        }
        Self::from_epoch_micros(value * 1_000)
    }

    /// A JSON float as a Unix timestamp, with pydantic's unit choice (seconds up
    /// to 2e10, milliseconds above) and its observed negative-fraction
    /// handling: the fraction is complemented within the chosen unit.
    fn from_epoch_float(value: f64) -> Option<Self> {
        if !value.is_finite() {
            return None;
        }
        let unit_micros: i128 = if value.abs() <= DECISION_EPOCH_SECONDS_LIMIT {
            1_000_000
        } else {
            1_000
        };
        let trunc = value.trunc();
        if trunc.abs() > 1e18 {
            return None;
        }
        let whole = trunc as i128 * unit_micros;
        if value.fract() == 0.0 {
            return Self::from_epoch_micros(whole);
        }
        let fraction = (value - trunc).abs();
        let micros = (fraction * unit_micros as f64).round() as i128;
        let total = if value > 0.0 {
            whole + micros
        } else {
            whole - (unit_micros - micros)
        };
        Self::from_epoch_micros(total)
    }

    /// An aware-UTC moment from Unix microseconds, or `None` outside Python's
    /// `datetime` range (pydantic refuses those).
    fn from_epoch_micros(total: i128) -> Option<Self> {
        if !(DECISION_MIN_EPOCH_MICROS..=DECISION_MAX_EPOCH_MICROS).contains(&total) {
            return None;
        }
        let seconds = total.div_euclid(1_000_000);
        let micros = total.rem_euclid(1_000_000) as u32;
        let instant = chrono::DateTime::from_timestamp(seconds as i64, micros * 1_000)?;
        Self {
            year: chrono::Datelike::year(&instant),
            month: chrono::Datelike::month(&instant),
            day: chrono::Datelike::day(&instant),
            hour: chrono::Timelike::hour(&instant),
            minute: chrono::Timelike::minute(&instant),
            second: chrono::Timelike::second(&instant),
            micros,
            offset_seconds: Some(0),
        }
        .validated()
    }
}

/// Fixed-width ASCII digits at `start`, or `None`.
fn digits(bytes: &[u8], start: usize, width: usize) -> Option<u32> {
    let slice = bytes.get(start..start + width)?;
    let mut value = 0u32;
    for byte in slice {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value * 10 + u32::from(byte - b'0');
    }
    Some(value)
}

/// Days in a proleptic-Gregorian month (Python's `datetime` calendar).
fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        _ => 0,
    }
}
