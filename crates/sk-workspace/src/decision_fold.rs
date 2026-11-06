//! The ONE canonical event → `IndexEntry` fold for the Decision Moment ledger.
//!
//! # Source
//!
//! `specify_cli.decisions.index_fold` (pinned `9aed3a80`), the single source of
//! truth for interpreting `DecisionPointOpened` / `DecisionPointResolved`
//! envelopes as `IndexEntry` records. Source states the contract explicitly:
//! the forward write path and the reconciler share this fold so they cannot
//! drift into two independent interpretations of a decision event
//! (``contracts/decisions-doctor.md``, "Single fold, no second reducer").
//!
//! This module is that fold in Native. It builds the full `IndexEntry` — every
//! field, not just the status — and validates the OPEN-state document through
//! the canonical model ([`crate::decision_model::validate_entry_document`]), so
//! there is one event/entry model, one validator and one fold. The doctor's
//! former private fold is now a re-export of this module
//! ([`crate::doctor::decision_fold`]).
//!
//! # Reproduced Source behaviour
//!
//! * **Opening is required, and duplicates must agree.** Several openings fold
//!   only when their payloads are equal apart from `recorded_at` (#4848); the
//!   earliest `(recorded_at, event_id)` is used as a deterministic tie-break
//!   (the folded entry never reads `recorded_at`, so the winner cannot change
//!   the result).
//! * **Order independence (#4919).** The event-log merge driver re-sorts the
//!   whole log by `(at, event_id)`, so the fold accepts envelopes in ANY order
//!   and selects the terminal envelope by OUTCOME SET, not append position.
//! * **The reopen rule is one rule.** `deferred → resolved` (the
//!   `spec_kitty_events.decisionpoint.DECISION_POINT_TERMINAL_REOPEN` pair) is
//!   the only legal terminal-to-terminal transition, in either position; every
//!   other multi-terminal combination is unfoldable. It is exposed as
//!   [`is_allowed_terminal_reopen`] so a future write path enforces the same
//!   rule rather than re-spelling the pair.
//! * **Wire-schema limits are reproduced, not repaired.** The opening wire
//!   carries a single collapsed `step_id`, so the fold sets `slot_key = None`
//!   and `step_id = str(payload["step_id"])`; `summary_json` is never on the
//!   wire, so a folded terminal entry always records `summary_json = None`.
//! * **Two different validation phases.** The OPEN document is validated, since
//!   Source constructs it through `build_opened_entry(...) → IndexEntry(...)`.
//!   The terminal transition is applied the way Source's `apply_terminal` does
//!   — `entry.model_copy(update=...)` — which does **not** re-validate: only
//!   `status`, `resolved_at` (parsed through the canonical clock) and
//!   `resolved_by` (`str()`) are interpreted, while `final_answer`,
//!   `rationale`, `other_answer` and `summary_json` are copied verbatim. That
//!   distinction is reproduced deliberately; re-validating the terminal row
//!   would refuse input Source accepts.
//!
//! # Stated limits (disclosed, not waived)
//!
//! * Source's `str()` coercion of an exotic non-string payload scalar (a list or
//!   object) uses Python's `repr` spelling. Native uses the canonical Python
//!   `repr` renderer for those values, which matches for scalars and is
//!   approximate for containers.
//! * pydantic's `ValidationError` prose is not reproduced; a malformed payload
//!   is a typed [`FoldError`] with Native detail text.

use std::fmt;

use sk_kernel::PythonString;
use sk_kernel::python_json::{PythonJsonDict, PythonJsonValue, is_falsy, python_repr_value};

use crate::decision_model::{
    DecisionEntry, DecisionModelError, DecisionStatus, validate_entry_document,
};

/// The opening envelope's `event_type` (Source `DECISION_POINT_OPENED`).
pub const DECISION_OPENED_EVENT: &str = "DecisionPointOpened";

/// The terminal envelope's `event_type` (Source `DECISION_POINT_RESOLVED`).
pub const DECISION_RESOLVED_EVENT: &str = "DecisionPointResolved";

/// Source `OriginFlow` member values, in declaration order.
const ORIGIN_FLOWS: [&str; 3] = ["charter", "specify", "plan"];

/// Source `TerminalOutcome` member values mapped onto ledger statuses.
const TERMINAL_OUTCOMES: [(&str, DecisionStatus); 3] = [
    ("resolved", DecisionStatus::Resolved),
    ("deferred", DecisionStatus::Deferred),
    ("canceled", DecisionStatus::Canceled),
];

/// The one legal terminal-to-terminal reopen, as a `(current, target)` outcome
/// pair (`spec_kitty_events.decisionpoint.DECISION_POINT_TERMINAL_REOPEN`, #6412).
const ALLOWED_TERMINAL_REOPENS: [(&str, &str); 1] = [("deferred", "resolved")];

/// The one opening-payload field two openings of one decision may differ in.
const OPENED_VOLATILE_FIELD: &str = "recorded_at";

/// A decision's event group cannot be folded into a ledger entry.
///
/// Source raises `FoldError(ValueError)`; this is the typed equivalent. The
/// detail text is Native prose — Source's exception text is not a pinned wire
/// contract — but the numbered decisions it distinguishes are Source's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoldError {
    detail: String,
}

impl FoldError {
    fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
    }

    /// The diagnostic.
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for FoldError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(&self.detail)
    }
}

impl std::error::Error for FoldError {}

impl From<DecisionModelError> for FoldError {
    fn from(error: DecisionModelError) -> Self {
        Self::new(format!("malformed decision event payload ({error})"))
    }
}

/// One folded decision: its full `IndexEntry` document plus its status.
///
/// [`Self::entry`] is the `IndexEntry.model_dump(mode="json")`-shaped row the
/// ledger stores: every field present, datetimes normalised, `options` an
/// array. The OPEN row is model-validated; a terminal row carries Source's
/// `model_copy` semantics (see the module docs).
#[derive(Clone, Debug)]
pub struct FoldedDecision {
    entry: PythonJsonDict,
    status: DecisionStatus,
}

impl FoldedDecision {
    /// The folded entry document, in Source field-declaration order.
    pub fn entry(&self) -> &PythonJsonDict {
        &self.entry
    }

    /// The folded status.
    pub fn status(&self) -> DecisionStatus {
        self.status
    }

    /// One folded field, by its Source name.
    pub fn field(&self, name: &str) -> Option<&PythonJsonValue> {
        self.entry.get(name)
    }
}

/// True when a decision already in `current` may still transition to `target`.
///
/// Source `is_allowed_terminal_reopen`: the status-level view of
/// `DECISION_POINT_TERMINAL_REOPEN`, shared by the write path and this fold so
/// there is never a second, independently maintained outcome pair.
pub fn is_allowed_terminal_reopen(current: DecisionStatus, target: DecisionStatus) -> bool {
    ALLOWED_TERMINAL_REOPENS.iter().any(|(from, to)| {
        DecisionStatus::from_text(&python_text(from)) == Some(current)
            && DecisionStatus::from_text(&python_text(to)) == Some(target)
    })
}

/// Fold one decision's envelopes (in any order) into its full ledger entry.
///
/// Mirrors Source `index_fold.fold_events`. Every malformed input is a
/// [`FoldError`]; nothing is guessed and no field is invented.
pub fn fold_events(events: &[PythonJsonValue]) -> Result<FoldedDecision, FoldError> {
    let mut opened: Vec<&PythonJsonValue> = Vec::new();
    let mut resolved: Vec<&PythonJsonValue> = Vec::new();
    for event in events {
        let object = exact_object(event)
            .ok_or_else(|| FoldError::new("decision event envelope is not a JSON object"))?;
        match object.get("event_type") {
            Some(PythonJsonValue::String(event_type))
                if *event_type == python_text(DECISION_OPENED_EVENT) =>
            {
                opened.push(event)
            }
            Some(PythonJsonValue::String(event_type))
                if *event_type == python_text(DECISION_RESOLVED_EVENT) =>
            {
                resolved.push(event)
            }
            other => {
                return Err(FoldError::new(format!(
                    "unknown event type(s) in fold input: [{}]",
                    other
                        .map(python_repr_value)
                        .unwrap_or_else(|| "None".to_owned())
                )));
            }
        }
    }

    let opened_document = fold_opened(select_opened(&opened)?)?;
    match select_terminal(&resolved)? {
        None => Ok(FoldedDecision {
            entry: opened_document,
            status: DecisionStatus::Open,
        }),
        Some(payload) => fold_terminal(opened_document, payload),
    }
}

/// Fold one decision's envelopes into its status only.
///
/// The status is one projection of the full fold (Source's reconciler compares
/// statuses); this convenience never computes a second interpretation.
pub fn fold_status(events: &[PythonJsonValue]) -> Result<DecisionStatus, FoldError> {
    fold_events(events).map(|folded| folded.status)
}

/// Pick the single opening envelope (Source `_select_opened_event`).
fn select_opened<'a>(opened: &[&'a PythonJsonValue]) -> Result<&'a PythonJsonValue, FoldError> {
    let Some(first) = opened.first().copied() else {
        return Err(FoldError::new(
            "no DecisionPointOpened event in the fold input",
        ));
    };
    let first_identity = payload_without_recorded_at(payload_of(first)?);
    for candidate in &opened[1..] {
        if payload_without_recorded_at(payload_of(candidate)?) != first_identity {
            return Err(FoldError::new(
                "more than one DecisionPointOpened event for one decision_point_id with differing payloads",
            ));
        }
    }
    Ok(opened
        .iter()
        .copied()
        .min_by_key(|event| recording_key(event))
        .unwrap_or(first))
}

/// The `(recorded_at, event_id)` ordering key Source uses as a deterministic
/// tie-break between equal openings.
fn recording_key(event: &PythonJsonValue) -> (PythonString, PythonString) {
    let Some(object) = exact_object(event) else {
        return (PythonString::default(), PythonString::default());
    };
    let recorded_at = object
        .get("payload")
        .and_then(exact_object)
        .and_then(|payload| payload.get(OPENED_VOLATILE_FIELD))
        .map(python_str)
        .unwrap_or_default();
    let event_id = object.get("event_id").map(python_str).unwrap_or_default();
    (recorded_at, event_id)
}

/// Pick the terminal envelope, enforcing the reopen rule (Source
/// `_select_terminal_event`).
fn select_terminal<'a>(
    resolved: &[&'a PythonJsonValue],
) -> Result<Option<&'a PythonJsonDict>, FoldError> {
    match resolved {
        [] => Ok(None),
        [only] => Ok(Some(payload_of(only)?)),
        [first, second] => {
            let first_payload = payload_of(first)?;
            let second_payload = payload_of(second)?;
            let first_outcome = outcome_of(first_payload)?;
            let second_outcome = outcome_of(second_payload)?;
            if allowed_terminal_reopen(first_outcome, second_outcome) {
                return Ok(Some(second_payload));
            }
            if allowed_terminal_reopen(second_outcome, first_outcome) {
                return Ok(Some(first_payload));
            }
            Err(unfoldable(&[first_outcome, second_outcome]))
        }
        many => {
            let mut outcomes = Vec::with_capacity(many.len());
            for event in many {
                outcomes.push(outcome_of(payload_of(event)?)?);
            }
            Err(unfoldable(&outcomes))
        }
    }
}

/// The Source refusal text for an unfoldable terminal combination.
fn unfoldable(outcomes: &[&str]) -> FoldError {
    let mut outcomes: Vec<&str> = outcomes.to_vec();
    outcomes.sort_unstable();
    FoldError::new(format!(
        "unfoldable combination of {} DecisionPointResolved events for one decision_point_id: outcomes={outcomes:?}",
        outcomes.len()
    ))
}

/// Whether `current` may still transition to `target`, at outcome level.
fn allowed_terminal_reopen(current: &str, target: &str) -> bool {
    ALLOWED_TERMINAL_REOPENS
        .iter()
        .any(|(from, to)| *from == current && *to == target)
}

/// Build and validate the OPEN entry document (Source `_fold_opened` →
/// `build_opened_entry` → `IndexEntry(...)`, which validates).
fn fold_opened(event: &PythonJsonValue) -> Result<PythonJsonDict, FoldError> {
    let payload = payload_of(event)?;
    let origin_flow = origin_flow_of(payload)?;
    let options = options_of(payload)?;
    let created_at = parse_timestamp(payload, "state_entered_at")?;

    let mut document = PythonJsonDict::new();
    insert(
        &mut document,
        "decision_id",
        PythonJsonValue::String(python_str(required(payload, "decision_point_id")?)),
    );
    insert(
        &mut document,
        "origin_flow",
        PythonJsonValue::String(python_text(origin_flow)),
    );
    insert(
        &mut document,
        "step_id",
        PythonJsonValue::String(python_str(required(payload, "step_id")?)),
    );
    insert(&mut document, "slot_key", PythonJsonValue::Null);
    insert(
        &mut document,
        "input_key",
        PythonJsonValue::String(python_str(required(payload, "input_key")?)),
    );
    insert(
        &mut document,
        "question",
        PythonJsonValue::String(python_str(required(payload, "question")?)),
    );
    insert(&mut document, "options", options);
    insert(
        &mut document,
        "status",
        PythonJsonValue::String(python_text(DecisionStatus::Open.as_str())),
    );
    insert(&mut document, "final_answer", PythonJsonValue::Null);
    insert(&mut document, "rationale", PythonJsonValue::Null);
    insert(&mut document, "other_answer", PythonJsonValue::Bool(false));
    insert(&mut document, "summary_json", PythonJsonValue::Null);
    insert(&mut document, "created_at", created_at);
    insert(&mut document, "resolved_at", PythonJsonValue::Null);
    insert(&mut document, "resolved_by", PythonJsonValue::Null);
    insert(
        &mut document,
        "opened_by",
        PythonJsonValue::String(python_str(required(payload, "actor_id")?)),
    );
    insert(
        &mut document,
        "mission_id",
        PythonJsonValue::String(python_str(required(payload, "mission_id")?)),
    );
    insert(
        &mut document,
        "mission_slug",
        PythonJsonValue::String(python_str(required(payload, "mission_slug")?)),
    );

    // Source constructs `IndexEntry(...)` here, so the OPEN row is validated.
    validate_entry_document(&document)?;
    projected_entry(&document)
}

/// Apply the terminal transition with Source's `model_copy` semantics.
fn fold_terminal(
    opened_document: PythonJsonDict,
    payload: &PythonJsonDict,
) -> Result<FoldedDecision, FoldError> {
    let status = terminal_status(payload)?;
    let resolved_at = normalized_timestamp(&opened_document, payload, status)?;
    let resolved_by = python_str(required(payload, "resolved_by")?);

    let mut entry = opened_document;
    insert(
        &mut entry,
        "status",
        PythonJsonValue::String(python_text(status.as_str())),
    );
    insert(
        &mut entry,
        "final_answer",
        payload
            .get("final_answer")
            .cloned()
            .unwrap_or(PythonJsonValue::Null),
    );
    insert(
        &mut entry,
        "rationale",
        payload
            .get("rationale")
            .cloned()
            .unwrap_or(PythonJsonValue::Null),
    );
    insert(
        &mut entry,
        "other_answer",
        PythonJsonValue::Bool(!is_falsy(
            payload
                .get("other_answer")
                .unwrap_or(&PythonJsonValue::Null),
        )),
    );
    insert(&mut entry, "summary_json", PythonJsonValue::Null);
    insert(&mut entry, "resolved_at", resolved_at);
    insert(
        &mut entry,
        "resolved_by",
        PythonJsonValue::String(resolved_by.clone()),
    );
    Ok(FoldedDecision { entry, status })
}

/// The `resolved_at` text the model projects for this payload.
///
/// Source holds a real `datetime` in the copied field, so the stored document
/// carries its `isoformat()`. Rather than re-implement the model's timestamp
/// normalisation here (a second authority), one throwaway document — otherwise
/// an already-valid OPEN row — is validated and its projected `resolved_at`
/// reused.
fn normalized_timestamp(
    opened_document: &PythonJsonDict,
    payload: &PythonJsonDict,
    status: DecisionStatus,
) -> Result<PythonJsonValue, FoldError> {
    let mut probe = opened_document.clone();
    insert(
        &mut probe,
        "resolved_at",
        parse_timestamp(payload, "state_entered_at")?,
    );
    insert(
        &mut probe,
        "status",
        PythonJsonValue::String(python_text(status.as_str())),
    );
    let projected = projected_entry(&probe)?;
    Ok(projected
        .get("resolved_at")
        .cloned()
        .unwrap_or(PythonJsonValue::Null))
}

/// The model's projection of a validated entry document.
fn projected_entry(document: &PythonJsonDict) -> Result<PythonJsonDict, FoldError> {
    let entry: DecisionEntry = validate_entry_document(document)?;
    Ok(entry.fields().clone())
}

/// The terminal status of one resolved payload (Source `_fold_resolved`).
fn terminal_status(payload: &PythonJsonDict) -> Result<DecisionStatus, FoldError> {
    let outcome = outcome_of(payload)?;
    // Source also reads `state_entered_at` (resolved_at) and `resolved_by`; a
    // missing one is a `FoldError` there, so it is here.
    parse_timestamp(payload, "state_entered_at")?;
    required(payload, "resolved_by")?;
    TERMINAL_OUTCOMES
        .iter()
        .find(|(member, _)| *member == outcome)
        .map(|(_, status)| *status)
        .ok_or_else(|| FoldError::new(format!("unknown terminal_outcome {outcome:?}")))
}

/// The terminal outcome member of a resolved payload.
fn outcome_of(payload: &PythonJsonDict) -> Result<&'static str, FoldError> {
    match payload.get("terminal_outcome") {
        Some(PythonJsonValue::String(outcome)) => TERMINAL_OUTCOMES
            .iter()
            .map(|(member, _)| *member)
            .find(|member| *outcome == python_text(member))
            .ok_or_else(|| {
                FoldError::new(format!(
                    "malformed decision event payload (invalid terminal_outcome {})",
                    python_repr_value(&PythonJsonValue::String(outcome.clone()))
                ))
            }),
        other => Err(FoldError::new(format!(
            "malformed decision event payload (invalid terminal_outcome {})",
            other
                .map(python_repr_value)
                .unwrap_or_else(|| "None".to_owned())
        ))),
    }
}

/// The validated `origin_flow` member (Source `OriginFlow(payload[...])`).
fn origin_flow_of(payload: &PythonJsonDict) -> Result<&'static str, FoldError> {
    match payload.get("origin_flow") {
        Some(PythonJsonValue::String(flow)) => ORIGIN_FLOWS
            .iter()
            .copied()
            .find(|member| *flow == python_text(member))
            .ok_or_else(|| {
                FoldError::new(format!(
                    "malformed decision event payload (invalid origin_flow {})",
                    python_repr_value(&PythonJsonValue::String(flow.clone()))
                ))
            }),
        other => Err(FoldError::new(format!(
            "malformed decision event payload (invalid origin_flow {})",
            other
                .map(python_repr_value)
                .unwrap_or_else(|| "None".to_owned())
        ))),
    }
}

/// `tuple(payload.get("options", ()))` — including Python's refusal of `None`
/// and its per-code-point iteration of a bare string.
fn options_of(payload: &PythonJsonDict) -> Result<PythonJsonValue, FoldError> {
    match payload.get("options") {
        None => Ok(PythonJsonValue::Array(Vec::new())),
        Some(PythonJsonValue::Null) => Err(FoldError::new(
            "malformed decision event payload (TypeError: 'NoneType' object is not iterable)",
        )),
        Some(PythonJsonValue::String(text)) => Ok(PythonJsonValue::Array(
            text.codepoints()
                .iter()
                .copied()
                .map(|code| {
                    PythonJsonValue::String(
                        PythonString::from_units(vec![code])
                            .expect("a decoded code point is a valid Python code point"),
                    )
                })
                .collect(),
        )),
        Some(PythonJsonValue::Array(items)) => Ok(PythonJsonValue::Array(items.clone())),
        Some(other) => Err(FoldError::new(format!(
            "malformed decision event payload (invalid options {})",
            python_repr_value(other)
        ))),
    }
}

/// Require `key` to be present (Source raises `KeyError` on a missing key).
fn required<'a>(payload: &'a PythonJsonDict, key: &str) -> Result<&'a PythonJsonValue, FoldError> {
    payload.get(key).ok_or_else(|| {
        FoldError::new(format!(
            "malformed decision event payload (KeyError: 'missing {key}')"
        ))
    })
}

/// Python `str()` of a decoded JSON value, preserving exact code points.
///
/// The fold and the decision-stream fork detector both need Source's `str()`
/// coercion (event ids, decision ids, `step_id`/`resolved_by` payload members).
/// One helper owns it, so an id is never re-rendered by a second spelling.
/// A Python string is returned with its code points untouched; scalars use
/// Python's own spellings (`None`, `True`, `False`); containers fall back to the
/// canonical Python `repr` renderer, which is exact for scalars and approximate
/// for containers (disclosed in the module docs).
pub fn python_str(value: &PythonJsonValue) -> PythonString {
    match value {
        PythonJsonValue::String(text) => text.clone(),
        other => python_text(&str_of(other)),
    }
}

/// Validate a timestamp payload member with the canonical clock.
fn parse_timestamp(payload: &PythonJsonDict, key: &str) -> Result<PythonJsonValue, FoldError> {
    let text = python_str(required(payload, key)?);
    let scalar = text.to_scalar_string().map_err(|code| {
        FoldError::new(format!("malformed {key} (unpaired surrogate U+{code:04X})"))
    })?;
    sk_kernel::clock::parse_iso(&scalar).map_err(|error| {
        FoldError::new(format!("malformed decision event payload ({key}: {error})"))
    })?;
    Ok(PythonJsonValue::String(text))
}

/// The `payload` member of an envelope, which must be a decoded object.
fn payload_of(event: &PythonJsonValue) -> Result<&PythonJsonDict, FoldError> {
    exact_object(event)
        .and_then(|object| object.get("payload"))
        .and_then(exact_object)
        .ok_or_else(|| FoldError::new("decision event envelope has no object payload"))
}

/// The canonical reader's object view (the only object representation a
/// decoded document carries).
fn exact_object(value: &PythonJsonValue) -> Option<&PythonJsonDict> {
    match value {
        PythonJsonValue::ExactObject(object) => Some(object),
        _ => None,
    }
}

/// The payload members other than `recorded_at`, in decoded order.
fn payload_without_recorded_at(payload: &PythonJsonDict) -> Vec<(PythonString, PythonJsonValue)> {
    payload
        .iter()
        .filter(|(key, _)| *key != python_text(OPENED_VOLATILE_FIELD))
        .cloned()
        .collect()
}

/// Python `str()` for the scalar cases the fold coerces or reports.
fn str_of(value: &PythonJsonValue) -> String {
    match value {
        PythonJsonValue::String(text) => text.to_scalar_string().unwrap_or_default(),
        PythonJsonValue::Null => "None".to_owned(),
        PythonJsonValue::Bool(true) => "True".to_owned(),
        PythonJsonValue::Bool(false) => "False".to_owned(),
        PythonJsonValue::Integer(token) | PythonJsonValue::Number(token) => token.clone(),
        PythonJsonValue::Float(number) => sk_kernel::python_json::python_float_repr(*number),
        other => python_repr_value(other),
    }
}

/// Insert a field with an ASCII name.
fn insert(object: &mut PythonJsonDict, key: &str, value: PythonJsonValue) {
    object.insert(python_text(key), value);
}

/// An ASCII literal as an exact code-point Python string.
fn python_text(text: &str) -> PythonString {
    PythonString::from_scalars(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sk_kernel::python_json::parse;

    fn events(lines: &[&str]) -> Vec<PythonJsonValue> {
        lines
            .iter()
            .map(|line| parse(line).expect("valid JSON fixture"))
            .collect()
    }

    fn opened(id: &str, at: &str) -> String {
        format!(
            r#"{{"event_id":"e1","event_type":"DecisionPointOpened","payload":{{"decision_point_id":"{id}","origin_flow":"specify","step_id":"s","input_key":"k","question":"q","options":["a"],"state_entered_at":"{at}","actor_id":"claude","mission_id":"m","mission_slug":"001-x","recorded_at":"{at}"}}}}"#
        )
    }

    fn resolved(id: &str, outcome: &str, at: &str) -> String {
        format!(
            r#"{{"event_id":"e2","event_type":"DecisionPointResolved","payload":{{"decision_point_id":"{id}","terminal_outcome":"{outcome}","state_entered_at":"{at}","resolved_by":"operator","final_answer":"yes"}}}}"#
        )
    }

    fn text_of(field: Option<&PythonJsonValue>) -> Option<String> {
        match field {
            Some(PythonJsonValue::String(text)) => text.to_scalar_string().ok(),
            _ => None,
        }
    }

    #[test]
    fn an_opening_alone_folds_to_the_full_open_entry() {
        let log = events(&[&opened("d1", "2026-01-01T00:00:00+00:00")]);
        let folded = fold_events(&log).expect("fold");
        assert_eq!(folded.status(), DecisionStatus::Open);
        // The model projection: every field present, slot_key collapsed away.
        assert_eq!(folded.entry().len(), 18);
        assert_eq!(text_of(folded.field("decision_id")).as_deref(), Some("d1"));
        assert_eq!(
            text_of(folded.field("origin_flow")).as_deref(),
            Some("specify")
        );
        assert_eq!(text_of(folded.field("step_id")).as_deref(), Some("s"));
        assert_eq!(folded.field("slot_key"), Some(&PythonJsonValue::Null));
        assert_eq!(text_of(folded.field("status")).as_deref(), Some("open"));
        assert_eq!(
            text_of(folded.field("opened_by")).as_deref(),
            Some("claude")
        );
        assert_eq!(
            text_of(folded.field("created_at")).as_deref(),
            Some("2026-01-01T00:00:00Z")
        );
        assert_eq!(folded.field("resolved_at"), Some(&PythonJsonValue::Null));
        assert_eq!(folded.field("summary_json"), Some(&PythonJsonValue::Null));
    }

    #[test]
    fn a_single_terminal_folds_to_its_outcome_and_terminal_fields() {
        for (outcome, status) in [
            ("resolved", DecisionStatus::Resolved),
            ("deferred", DecisionStatus::Deferred),
            ("canceled", DecisionStatus::Canceled),
        ] {
            let log = events(&[
                &opened("d1", "2026-01-01T00:00:00+00:00"),
                &resolved("d1", outcome, "2026-01-02T00:00:00+00:00"),
            ]);
            let folded = fold_events(&log).expect("fold");
            assert_eq!(folded.status(), status, "{outcome}");
            assert_eq!(
                text_of(folded.field("status")).as_deref(),
                Some(status.as_str())
            );
            assert_eq!(
                text_of(folded.field("final_answer")).as_deref(),
                Some("yes")
            );
            assert_eq!(
                text_of(folded.field("resolved_by")).as_deref(),
                Some("operator")
            );
            assert_eq!(
                text_of(folded.field("resolved_at")).as_deref(),
                Some("2026-01-02T00:00:00Z")
            );
            assert_eq!(folded.field("summary_json"), Some(&PythonJsonValue::Null));
        }
    }

    #[test]
    fn the_deferred_then_resolved_reopen_folds_to_resolved_in_either_order() {
        let defer = resolved("d1", "deferred", "2026-01-02T00:00:00+00:00");
        let resolve = resolved("d1", "resolved", "2026-01-03T00:00:00+00:00");
        let open = opened("d1", "2026-01-01T00:00:00+00:00");
        for log in [
            events(&[&open, &defer, &resolve]),
            events(&[&open, &resolve, &defer]),
        ] {
            let folded = fold_events(&log).expect("fold");
            assert_eq!(folded.status(), DecisionStatus::Resolved);
            assert_eq!(
                text_of(folded.field("resolved_at")).as_deref(),
                Some("2026-01-03T00:00:00Z")
            );
        }
    }

    #[test]
    fn the_reopen_rule_is_the_one_source_pair() {
        assert!(is_allowed_terminal_reopen(
            DecisionStatus::Deferred,
            DecisionStatus::Resolved
        ));
        for (current, target) in [
            (DecisionStatus::Resolved, DecisionStatus::Deferred),
            (DecisionStatus::Resolved, DecisionStatus::Resolved),
            (DecisionStatus::Canceled, DecisionStatus::Resolved),
            (DecisionStatus::Open, DecisionStatus::Resolved),
        ] {
            assert!(
                !is_allowed_terminal_reopen(current, target),
                "{current:?}/{target:?}"
            );
        }
    }

    #[test]
    fn unfoldable_terminal_combinations_are_refused() {
        let open = opened("d1", "2026-01-01T00:00:00+00:00");
        for pair in [
            ("resolved", "resolved"),
            ("resolved", "canceled"),
            ("canceled", "deferred"),
        ] {
            let first = resolved("d1", pair.0, "2026-01-02T00:00:00+00:00");
            let second = resolved("d1", pair.1, "2026-01-03T00:00:00+00:00");
            let error = fold_events(&events(&[&open, &first, &second])).expect_err("refusal");
            assert!(error.detail().contains("unfoldable combination"), "{error}");
        }
    }

    #[test]
    fn a_missing_opening_or_conflicting_openings_are_refused() {
        assert!(fold_events(&[]).is_err());
        let a = opened("d1", "2026-01-01T00:00:00+00:00");
        let b = a.replace(r#""question":"q""#, r#""question":"different""#);
        assert!(fold_events(&events(&[&a, &b])).is_err());
    }

    #[test]
    fn equal_openings_apart_from_recorded_at_fold_once() {
        let a = opened("d1", "2026-01-01T00:00:00+00:00");
        let b = a.replace(
            r#""recorded_at":"2026-01-01T00:00:00+00:00""#,
            r#""recorded_at":"2026-01-05T00:00:00+00:00""#,
        );
        assert_eq!(fold_status(&events(&[&a, &b])), Ok(DecisionStatus::Open));
    }

    #[test]
    fn malformed_opened_payloads_are_refused_not_folded() {
        let good = opened("d1", "2026-01-01T00:00:00+00:00");
        for (needle, replacement) in [
            (r#""origin_flow":"specify""#, r#""origin_flow":"bogus""#),
            (r#""input_key":"k""#, r#""input_key":"""#),
            (r#""options":["a"]"#, r#""options":null"#),
            (r#""options":["a"]"#, r#""options":[1]"#),
            (
                r#""state_entered_at":"2026-01-01T00:00:00+00:00""#,
                r#""state_entered_at":"not-a-time""#,
            ),
            (r#""step_id":"s","#, ""),
            (r#""mission_id":"m","#, ""),
        ] {
            let broken = good.replace(needle, replacement);
            assert!(
                fold_events(&events(&[&broken])).is_err(),
                "expected a refusal for {needle} -> {replacement}"
            );
        }
    }

    #[test]
    fn a_terminal_without_its_required_fields_is_refused() {
        let open = opened("d1", "2026-01-01T00:00:00+00:00");
        let no_resolver = resolved("d1", "resolved", "2026-01-02T00:00:00+00:00")
            .replace(r#","resolved_by":"operator""#, "");
        assert!(fold_events(&events(&[&open, &no_resolver])).is_err());
        let no_time = resolved("d1", "resolved", "2026-01-02T00:00:00+00:00")
            .replace(r#""state_entered_at":"2026-01-02T00:00:00+00:00","#, "");
        assert!(fold_events(&events(&[&open, &no_time])).is_err());
    }

    #[test]
    fn terminal_fields_keep_source_copy_semantics_without_revalidation() {
        // Source `apply_terminal` is `model_copy(update=...)`, which does not
        // validate: a non-string `final_answer` is copied through verbatim.
        let open = opened("d1", "2026-01-01T00:00:00+00:00");
        let numeric = resolved("d1", "resolved", "2026-01-02T00:00:00+00:00")
            .replace(r#""final_answer":"yes""#, r#""final_answer":3"#);
        let folded = fold_events(&events(&[&open, &numeric])).expect("fold");
        assert_eq!(folded.status(), DecisionStatus::Resolved);
        assert_eq!(
            folded.field("final_answer"),
            Some(&PythonJsonValue::Integer("3".to_owned()))
        );
    }

    #[test]
    fn a_bare_string_options_payload_iterates_by_code_point_like_python() {
        let line = opened("d1", "2026-01-01T00:00:00+00:00")
            .replace(r#""options":["a"]"#, r#""options":"ab""#);
        let folded = fold_events(&events(&[&line])).expect("fold");
        assert_eq!(
            folded.field("options"),
            Some(&PythonJsonValue::Array(vec![
                PythonJsonValue::String(python_text("a")),
                PythonJsonValue::String(python_text("b")),
            ]))
        );
    }

    #[test]
    fn an_unknown_event_type_is_refused_with_its_repr() {
        let log = events(&[r#"{"event_type":"SomethingElse","payload":{}}"#]);
        let error = fold_events(&log).expect_err("refusal");
        assert!(error.detail().contains("unknown event type"), "{error}");
    }
}
