//! The ONE Source-valued `InnerStateChanged` annotation model.
//!
//! The reference's CLI annotation model (`specify_cli.status.models.InnerStateChanged`
//! together with `WPInnerStateDelta`) keeps the *authored* delta values: only
//! `event_id` is constrained (a ULID `str`), `wp_id`/`at` are `str()`-coerced,
//! `shell_pid` is `int()`-coerced, `subtasks` is a lane vocabulary,
//! `tracker_refs*` is `list()`-coerced and every remaining scalar slot keeps
//! whatever JSON value was authored (`agent: 123`, `role: {"a": 1}`,
//! `note: NaN`). A finite Rust struct with `String`/`bool`/`i64` fields cannot
//! represent that: it stringifies numbers, refuses non-finite scalars and loses
//! containers.
//!
//! This module therefore owns ONE representation — the admitted raw row as the
//! canonical Kernel J1 [`PythonJsonDict`] plus the validated/coerced projection
//! the reference actually folds — and ONE fold consumer
//! ([`crate::source_event::reduce_source_diary`]). There is no second schema,
//! parser, serializer or sidecar: the typed legacy view
//! ([`crate::replay::InnerStateChanged`]) is a checked adapter over the same
//! fold, and the transitions-only/typed APIs keep their existing names.
//!
//! Admission phases (the reference's own order, exactly):
//!
//! 1. `data["event_id"]` — required; must be a `str` matching the reference
//!    `ULID_PATTERN` (`^[0-9A-HJKMNP-TV-Z]{26}$` under `re.match`, so a single
//!    trailing newline is accepted, as CPython's `$` does);
//! 2. `data.get("kind") == "annotation"`;
//! 3. `data["delta"]` — required;
//! 4. `str(data["wp_id"])`, `str(data["at"])`, `decode_actor(data["actor"])`;
//! 5. the delta body: mapping access (`subtasks`/`review`/`shell_pid`/
//!    `tracker_refs*`/scalar slots), then `subtasks.items()`, then
//!    `ReviewOverride.from_dict`, then `int(shell_pid)`, then `list(...)` for
//!    both tracker-ref channels, then the `""` -> absent scalar normalization.
//!
//! Read-only: nothing here writes or repairs.

use std::collections::BTreeMap;

use sk_kernel::PythonString;
use sk_kernel::python_json::{
    PythonJsonDict, PythonJsonValue, python_json_str, python_json_truthy, python_json_type_name,
};

use crate::source_event::{
    ANNOTATION_KIND, SourceActor, decode_actor, key, normalize_object, python_repr,
};
use crate::source_execution::SourcePythonFailure;

/// The pure `str | None` scalar slots (`WPInnerStateDelta._SCALAR_FIELDS`).
pub const DELTA_SCALAR_FIELDS: [&str; 8] = [
    "shell_pid_created_at",
    "agent",
    "assignee",
    "role",
    "agent_profile",
    "agent_profile_version",
    "model",
    "provider",
];

/// `WPInnerStateDelta.release_runtime_claim`'s claim triple.
pub const CLAIM_RELEASE_SLOTS: [&str; 3] = ["agent", "shell_pid", "shell_pid_created_at"];

/// The annotation's review-artifact override (`ReviewOverride.to_dict()`), exact
/// four `str()`-coerced fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceReviewOverride {
    /// `at`, `str()`-coerced.
    pub at: PythonString,
    /// `actor`, `str()`-coerced.
    pub actor: PythonString,
    /// `wp_id`, `str()`-coerced.
    pub wp_id: PythonString,
    /// `reason`, `str()`-coerced.
    pub reason: PythonString,
}

impl SourceReviewOverride {
    /// True only when all four fields are empty (`is_release_sentinel`).
    pub fn is_release_sentinel(&self) -> bool {
        self.at.is_empty()
            && self.actor.is_empty()
            && self.wp_id.is_empty()
            && self.reason.is_empty()
    }

    /// `ReviewOverride.to_dict()`.
    pub fn to_value(&self) -> PythonJsonValue {
        let mut object = PythonJsonDict::new();
        object.insert(key("at"), PythonJsonValue::String(self.at.clone()));
        object.insert(key("actor"), PythonJsonValue::String(self.actor.clone()));
        object.insert(key("wp_id"), PythonJsonValue::String(self.wp_id.clone()));
        object.insert(key("reason"), PythonJsonValue::String(self.reason.clone()));
        PythonJsonValue::ExactObject(object)
    }
}

/// The folded form of the delta's `review` slot.
#[derive(Debug, Clone, PartialEq)]
pub enum ReviewFold {
    /// The all-empty release sentinel: the slot is cleared to `None`.
    Clear,
    /// The override's `to_dict()` value.
    Value(PythonJsonValue),
}

/// The typed `WPInnerStateDelta` view over Source-authored values.
///
/// Every slot is `Option`-shaped because an absent (or `None`) field leaves the
/// reduced slot untouched. Values are the reference's coerced forms, not a second
/// decode of the row: `shell_pid` is an exact Python `int`, `subtasks` holds the
/// canonical status strings, and the scalar slots keep the authored value.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SourceDelta {
    /// `bool(data.get("release_runtime_claim", False))`.
    pub release_runtime_claim: bool,
    /// `int(shell_pid)`.
    pub shell_pid: Option<PythonJsonValue>,
    /// The present, non-blank `str | None` scalar slots.
    pub scalars: BTreeMap<&'static str, PythonJsonValue>,
    /// `{str(sid): str(Status)}`, insertion ordered.
    pub subtasks: Option<PythonJsonDict>,
    /// The authored `note` value (the fold appends it to `notes`).
    pub note: Option<PythonJsonValue>,
    /// The additive tracker-ref channel.
    pub tracker_refs: Option<Vec<PythonJsonValue>>,
    /// The replace tracker-ref channel.
    pub tracker_refs_replace: Option<Vec<PythonJsonValue>>,
    /// The review-artifact override.
    pub review: Option<SourceReviewOverride>,
}

impl SourceDelta {
    /// Admit one `delta` value exactly as `WPInnerStateDelta.from_dict` does.
    pub fn admit(value: &PythonJsonValue) -> Result<Self, SourcePythonFailure> {
        let object = normalize_object(value).ok_or_else(|| {
            SourcePythonFailure::attribute_error(format!(
                "'{}' object has no attribute 'get'",
                python_json_type_name(value)
            ))
        })?;
        let subtasks = match object.get("subtasks").filter(|value| !is_null(value)) {
            Some(value) => Some(admit_subtasks(value)?),
            None => None,
        };
        let review = match object.get("review").filter(|value| !is_null(value)) {
            Some(value) => Some(admit_review_override(value)?),
            None => None,
        };
        let shell_pid = match object.get("shell_pid").filter(|value| !is_null(value)) {
            Some(value) => Some(python_int_coercion(value)?),
            None => None,
        };
        let tracker_refs = match object.get("tracker_refs").filter(|value| !is_null(value)) {
            Some(value) => Some(python_list_coercion(value)?),
            None => None,
        };
        let tracker_refs_replace = match object
            .get("tracker_refs_replace")
            .filter(|value| !is_null(value))
        {
            Some(value) => Some(python_list_coercion(value)?),
            None => None,
        };
        let mut scalars = BTreeMap::new();
        for name in DELTA_SCALAR_FIELDS {
            if let Some(value) = object.get(name).filter(|value| !is_null(value)) {
                // `__post_init__` normalizes `""` -> `None`: a blanking delta is
                // never recorded, and the fold never sees it.
                if matches!(value, PythonJsonValue::String(text) if text.is_empty()) {
                    continue;
                }
                scalars.insert(name, value.clone());
            }
        }
        let release_runtime_claim = object
            .get("release_runtime_claim")
            .is_some_and(python_json_truthy);
        Ok(Self {
            release_runtime_claim,
            shell_pid,
            scalars,
            subtasks,
            note: object.get("note").filter(|value| !is_null(value)).cloned(),
            tracker_refs,
            tracker_refs_replace,
            review,
        })
    }

    /// `WPInnerStateDelta.to_dict()` — only present slots, in reference order.
    pub fn to_value(&self) -> PythonJsonValue {
        let mut object = PythonJsonDict::new();
        if let Some(shell_pid) = &self.shell_pid {
            object.insert(key("shell_pid"), shell_pid.clone());
        }
        if let Some(subtasks) = &self.subtasks {
            object.insert(
                key("subtasks"),
                PythonJsonValue::ExactObject(subtasks.clone()),
            );
        }
        if let Some(note) = &self.note {
            object.insert(key("note"), note.clone());
        }
        if let Some(refs) = &self.tracker_refs {
            object.insert(key("tracker_refs"), PythonJsonValue::Array(refs.clone()));
        }
        if let Some(refs) = &self.tracker_refs_replace {
            object.insert(
                key("tracker_refs_replace"),
                PythonJsonValue::Array(refs.clone()),
            );
        }
        if let Some(review) = &self.review {
            object.insert(key("review"), review.to_value());
        }
        for name in DELTA_SCALAR_FIELDS {
            if let Some(value) = self.scalars.get(name) {
                object.insert(key(name), value.clone());
            }
        }
        if self.release_runtime_claim {
            object.insert(key("release_runtime_claim"), PythonJsonValue::Bool(true));
        }
        PythonJsonValue::ExactObject(object)
    }
}

/// One admitted off-axis annotation: the raw row plus its validated projection.
///
/// `row` is the admitted authored JSON object (duplicate-key semantics already
/// applied by the Kernel parser). `wp_id`/`at` are the reference's `str()`
/// coercions, `actor` its validated actor view and `delta` its coerced delta
/// view; nothing is stringified into a separate schema.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceAnnotation {
    row: PythonJsonDict,
    event_id: PythonString,
    wp_id: PythonString,
    at: PythonString,
    actor: SourceActor,
    delta: SourceDelta,
}

impl SourceAnnotation {
    /// The admitted authored row.
    pub fn raw(&self) -> &PythonJsonDict {
        &self.row
    }

    /// One authored field.
    pub fn field(&self, name: &str) -> Option<&PythonJsonValue> {
        self.row.get(name)
    }

    /// The `event_id` ULID, exactly as authored (a `str` by admission).
    pub fn event_id(&self) -> &PythonString {
        &self.event_id
    }

    /// The `str()`-coerced `wp_id` (the fold's per-WP key).
    pub fn wp_id(&self) -> &PythonString {
        &self.wp_id
    }

    /// The `str()`-coerced `at` (the fold's ordering key).
    pub fn at(&self) -> &PythonString {
        &self.at
    }

    /// The actor's display identity (`tool`, then `role`, then `""` for a
    /// structured actor; the exact Python `str()` for a scalar).
    pub fn actor_identity(&self) -> PythonString {
        self.actor.identity()
    }

    /// The decoded actor value (`InnerStateChanged.actor`).
    pub fn actor_value(&self) -> PythonJsonValue {
        self.actor.to_value()
    }

    /// The coerced delta.
    pub fn delta(&self) -> &SourceDelta {
        &self.delta
    }

    /// `InnerStateChanged.to_dict()`.
    pub fn to_dict_value(&self) -> PythonJsonValue {
        let mut object = PythonJsonDict::new();
        object.insert(
            key("event_id"),
            PythonJsonValue::String(self.event_id.clone()),
        );
        object.insert(key("kind"), PythonJsonValue::String(key(ANNOTATION_KIND)));
        object.insert(key("wp_id"), PythonJsonValue::String(self.wp_id.clone()));
        object.insert(key("at"), PythonJsonValue::String(self.at.clone()));
        object.insert(key("actor"), self.actor.to_value());
        object.insert(key("delta"), self.delta.to_value());
        PythonJsonValue::ExactObject(object)
    }

    /// Admit one parsed row exactly as `InnerStateChanged.from_dict` does.
    pub fn admit(object: &PythonJsonDict) -> Result<Self, SourcePythonFailure> {
        let event_id_value = object
            .get("event_id")
            .ok_or_else(|| SourcePythonFailure::key_error("'event_id'"))?;
        let event_id = match event_id_value {
            PythonJsonValue::String(text) if is_ulid_pattern(text) => text.clone(),
            other => {
                return Err(SourcePythonFailure::value_error(format!(
                    "InnerStateChanged.event_id is not a valid ULID: {}",
                    python_repr(other)
                )));
            }
        };
        match object.get("kind") {
            Some(PythonJsonValue::String(text)) if is_annotation_kind(text) => {}
            other => {
                return Err(SourcePythonFailure::value_error(format!(
                    "InnerStateChanged requires kind == '{ANNOTATION_KIND}', got {}",
                    other.map(python_repr).unwrap_or_else(|| "None".to_owned())
                )));
            }
        }
        let delta_value = object
            .get("delta")
            .ok_or_else(|| SourcePythonFailure::key_error("'delta'"))?;
        let wp_id = python_json_str(
            object
                .get("wp_id")
                .ok_or_else(|| SourcePythonFailure::key_error("'wp_id'"))?,
        );
        let at = python_json_str(
            object
                .get("at")
                .ok_or_else(|| SourcePythonFailure::key_error("'at'"))?,
        );
        let actor = decode_actor(
            object
                .get("actor")
                .ok_or_else(|| SourcePythonFailure::key_error("'actor'"))?,
        )
        .map_err(SourcePythonFailure::value_error)?;
        let delta = SourceDelta::admit(delta_value)?;
        Ok(Self {
            row: object.clone(),
            event_id,
            wp_id,
            at,
            actor,
            delta,
        })
    }
}

fn is_annotation_kind(text: &PythonString) -> bool {
    text.to_scalar_string().ok().as_deref() == Some(ANNOTATION_KIND)
}

fn is_null(value: &PythonJsonValue) -> bool {
    matches!(value, PythonJsonValue::Null)
}

/// CPython `ULID_PATTERN.match(value)` with the reference's anchored pattern.
///
/// `re.match(r"^[0-9A-HJKMNP-TV-Z]{26}$", value)` accepts exactly 26 alphabet
/// characters followed by end-of-string **or** by a single trailing newline,
/// because `$` also matches just before a final newline.
fn is_ulid_pattern(value: &PythonString) -> bool {
    let codepoints = value.codepoints();
    let body = match codepoints {
        [rest @ .., 0x0a] => rest,
        rest => rest,
    };
    body.len() == 26 && body.iter().all(|code| is_ulid_code(*code))
}

fn is_ulid_code(code: u32) -> bool {
    matches!(code, 0x30..=0x39)                       // 0-9
        || matches!(code, 0x41..=0x48)                // A-H
        || matches!(code, 0x4a | 0x4b | 0x4d | 0x4e)  // J K M N
        || matches!(code, 0x50..=0x54)                // P-T
        || matches!(code, 0x56..=0x5a) // V-Z
}

fn admit_subtasks(value: &PythonJsonValue) -> Result<PythonJsonDict, SourcePythonFailure> {
    let object = normalize_object(value).ok_or_else(|| {
        SourcePythonFailure::attribute_error(format!(
            "'{}' object has no attribute 'items'",
            python_json_type_name(value)
        ))
    })?;
    let mut subtasks = PythonJsonDict::new();
    for (subtask_id, status) in object.iter() {
        // `not_applicable` is an input alias of `skipped`; anything else must be a
        // lane value or the reference's own `ValueError` text applies.
        let canonical = match status {
            PythonJsonValue::String(text)
                if matches!(
                    text.to_scalar_string().ok().as_deref(),
                    Some("skipped" | "not_applicable")
                ) =>
            {
                "skipped".to_owned()
            }
            PythonJsonValue::String(text) => match text.to_scalar_string().ok() {
                Some(lane) if crate::lane::Lane::parse_canonical(&lane).is_ok() => lane,
                _ => {
                    return Err(SourcePythonFailure::value_error(format!(
                        "{} is not a valid Lane",
                        python_repr(status)
                    )));
                }
            },
            other => {
                return Err(SourcePythonFailure::value_error(format!(
                    "{} is not a valid Lane",
                    python_repr(other)
                )));
            }
        };
        subtasks.insert(
            subtask_id.clone(),
            PythonJsonValue::String(PythonString::from_scalars(&canonical)),
        );
    }
    Ok(subtasks)
}

fn admit_review_override(
    value: &PythonJsonValue,
) -> Result<SourceReviewOverride, SourcePythonFailure> {
    let object = normalize_object(value).ok_or_else(|| subscript_error(value))?;
    let field = |name: &str| -> Result<PythonString, SourcePythonFailure> {
        object
            .get(name)
            .map(python_json_str)
            .ok_or_else(|| SourcePythonFailure::key_error(format!("'{name}'")))
    };
    Ok(SourceReviewOverride {
        at: field("at")?,
        actor: field("actor")?,
        wp_id: field("wp_id")?,
        reason: field("reason")?,
    })
}

/// The reference's `obj[key]` refusal for a non-mapping `obj` (a `TypeError`).
fn subscript_error(value: &PythonJsonValue) -> SourcePythonFailure {
    match value {
        PythonJsonValue::Array(_) => {
            SourcePythonFailure::type_error("list indices must be integers or slices, not str")
        }
        PythonJsonValue::String(_) => {
            SourcePythonFailure::type_error("string indices must be integers, not 'str'")
        }
        other => SourcePythonFailure::type_error(format!(
            "'{}' object is not subscriptable",
            python_json_type_name(other)
        )),
    }
}

/// Python `int(value)` as an exact integer value (`int` is unbounded).
///
/// The typed form is [`python_int_coercion`]; this wrapper keeps the historical
/// `String` error for existing callers. The distinction that matters for the
/// store boundary is preserved there: `int(infinity)` is an `OverflowError`,
/// which is **outside** the reference's catch tuple, while `int(NaN)`, an invalid
/// literal and the digit ceiling are `ValueError`.
pub fn python_int_value(value: &PythonJsonValue) -> Result<PythonJsonValue, String> {
    python_int_coercion(value).map_err(|failure| failure.message().to_owned())
}

/// Python `int(value)` with the reference's own exception classes.
pub fn python_int_coercion(
    value: &PythonJsonValue,
) -> Result<PythonJsonValue, SourcePythonFailure> {
    match value {
        PythonJsonValue::Integer(digits) => Ok(PythonJsonValue::Integer(digits.clone())),
        PythonJsonValue::Number(token) => {
            if let Ok(parsed) = sk_kernel::python_json::parse(token) {
                match &parsed {
                    PythonJsonValue::Integer(_) | PythonJsonValue::Float(_) => {
                        return python_int_coercion(&parsed);
                    }
                    _ => {}
                }
            }
            Err(int_literal_error(&PythonString::from_scalars(token)))
        }
        PythonJsonValue::Bool(flag) => Ok(PythonJsonValue::Integer(
            if *flag { "1" } else { "0" }.to_owned(),
        )),
        PythonJsonValue::Float(number) => {
            if number.is_nan() {
                // `ValueError: cannot convert float NaN to integer`
                return Err(SourcePythonFailure::value_error(
                    "cannot convert float NaN to integer",
                ));
            }
            if number.is_infinite() {
                // `OverflowError: cannot convert float infinity to integer` — the
                // reference's store does NOT catch this, so it escapes.
                return Err(SourcePythonFailure::overflow_error(
                    "cannot convert float infinity to integer",
                ));
            }
            Ok(PythonJsonValue::Integer(exact_f64_integer(*number)))
        }
        PythonJsonValue::String(text) => parse_int_string(text),
        other => Err(SourcePythonFailure::type_error(format!(
            "int() argument must be a string, a bytes-like object or a real number, not '{}'",
            python_json_type_name(other)
        ))),
    }
}

/// `int(f)`: truncate toward zero and render the exact decimal digits.
fn exact_f64_integer(number: f64) -> String {
    let truncated = number.trunc();
    // Exact fixed-point rendering of the binary value (no exponent, no rounding).
    let rendered = format!("{truncated:.0}");
    if rendered == "-0" {
        "0".to_owned()
    } else {
        rendered
    }
}

fn int_literal_error(argument: &PythonString) -> SourcePythonFailure {
    SourcePythonFailure::value_error(
        sk_kernel::python_int::IntTextError::InvalidLiteral(argument.clone()).to_string(),
    )
}

/// `int(str)` with CPython's base-10 grammar over exact code points.
fn parse_int_string(text: &PythonString) -> Result<PythonJsonValue, SourcePythonFailure> {
    let limit = sk_kernel::python_json::environment_integer_digits()
        .unwrap_or(sk_kernel::python_json::SOURCE_DEFAULT_INTEGER_DIGITS);
    sk_kernel::python_int::parse_text(text, limit)
        .map(PythonJsonValue::Integer)
        .map_err(|error| SourcePythonFailure::value_error(error.to_string()))
}

/// Python `list(value)` over the J1 value domain.
///
/// The typed form is [`python_list_coercion`]; this wrapper keeps the historical
/// `String` error for existing callers.
pub fn python_list_value(value: &PythonJsonValue) -> Result<Vec<PythonJsonValue>, String> {
    python_list_coercion(value).map_err(|failure| failure.message().to_owned())
}

/// Python `list(value)` with the reference's own exception class (`TypeError`).
pub fn python_list_coercion(
    value: &PythonJsonValue,
) -> Result<Vec<PythonJsonValue>, SourcePythonFailure> {
    match value {
        PythonJsonValue::Array(items) => Ok(items.clone()),
        PythonJsonValue::String(text) => Ok(text
            .codepoints()
            .iter()
            .map(|code| {
                PythonJsonValue::String(
                    PythonString::from_codepoints(&[*code]).expect("a decoded code point"),
                )
            })
            .collect()),
        PythonJsonValue::Object(entries) => Ok(entries
            .iter()
            .map(|(name, _)| PythonJsonValue::String(PythonString::from_scalars(name)))
            .collect()),
        PythonJsonValue::ExactObject(object) => Ok(object
            .keys()
            .map(|name| PythonJsonValue::String(name.clone()))
            .collect()),
        other => Err(SourcePythonFailure::type_error(format!(
            "'{}' object is not iterable",
            python_json_type_name(other)
        ))),
    }
}
