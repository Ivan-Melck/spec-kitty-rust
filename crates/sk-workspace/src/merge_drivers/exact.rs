//! The merge-driver MODEL, on J1's exact Python tree.
//!
//! Every driver body — event log, meta, issue matrix, acceptance matrix,
//! decision index — merges over [`PythonJsonValue`] / [`PythonJsonDict`] /
//! [`PythonString`], so a Source-valid document keeps its exact code points for
//! values AND keys (all 2048 surrogate units, NUL, PUA, noncharacters), its key
//! order, its duplicate-key semantics and its exact integer/float tokens.
//!
//! `serde_json` is NOT used for any model value, key, equality or merge decision:
//! the only conversions live in `merge_drivers.rs`'s legacy public wrappers, on
//! the boundary of APIs whose signature IS a serde map.
//!
//! Encoding happens once, at the real output write: the tree renders to a
//! [`PythonString`] (CPython's text phase) and only then is encoded; an
//! `ensure_ascii = false` body that still holds a lone surrogate fails THERE,
//! exactly as Source's `Path.write_text` raises `UnicodeEncodeError`.

use std::collections::{BTreeMap, BTreeSet};

use sk_kernel::PythonString;
use sk_kernel::python_json::{
    PythonJsonDict, PythonJsonRenderOptions, PythonJsonValue, is_falsy, parse,
    python_json_dict_equal, python_json_equal, python_json_optional_sequence_equal,
    python_repr_value,
};
use sk_kernel::python_text::{decimal_value, is_decimal, is_whitespace};

use super::MergeDriverError;
use crate::decision_model::{self, DecisionModelError, DecisionStatus};

/// The exact value tree the drivers merge.
pub(crate) type Exact = PythonJsonValue;

/// A row key: the exact code points of the row's identity text.
///
/// `Vec<u32>` is Python `str` order and equality (code points), so a
/// `BTreeMap` on it sorts exactly as Source's `sorted({...})` does — including
/// surrogate keys, which a Rust `String` could not carry.
pub(crate) type Key = Vec<u32>;

// ---------------------------------------------------------------------------
// exact accessors
// ---------------------------------------------------------------------------

/// The one exact object builder (insertion ordered, `dict` assignment rules).
pub(crate) fn object(entries: Vec<(PythonString, Exact)>) -> Exact {
    let mut object = PythonJsonDict::new();
    for (key, value) in entries {
        object.insert(key, value);
    }
    Exact::ExactObject(object)
}

/// An object must be the reader's `ExactObject`; the scalar-key convenience view
/// is a producer shape and is never a merge model here.
pub(crate) fn as_object(value: &Exact) -> Option<&PythonJsonDict> {
    match value {
        Exact::ExactObject(object) => Some(object),
        _ => None,
    }
}

/// The list view, or `None` for a non-list.
pub(crate) fn as_array(value: &Exact) -> Option<&[Exact]> {
    match value {
        Exact::Array(items) => Some(items),
        _ => None,
    }
}

/// The exact string view of a value.
pub(crate) fn as_string(value: &Exact) -> Option<&PythonString> {
    match value {
        Exact::String(text) => Some(text),
        _ => None,
    }
}

/// Whether an exact string equals Rust text (code-point comparison).
pub(crate) fn text_is(text: &PythonString, candidate: &str) -> bool {
    text.codepoints().len() == candidate.chars().count()
        && text
            .codepoints()
            .iter()
            .copied()
            .eq(candidate.chars().map(u32::from))
}

/// A scalar `String` view of exact text, only when every code point is a scalar.
pub(crate) fn text_scalar(text: &PythonString) -> Option<String> {
    text.to_scalar_string().ok()
}

/// The member of an exact object under Rust text.
pub(crate) fn get<'a>(object: &'a PythonJsonDict, key: &str) -> Option<&'a Exact> {
    object.get(key)
}

/// The exact member of an exact object under an exact key.
pub(crate) fn get_codepoints<'a>(object: &'a PythonJsonDict, key: &[u32]) -> Option<&'a Exact> {
    object.get_codepoints(key)
}

/// The exact string of a member, when present and a string.
pub(crate) fn get_text<'a>(object: &'a PythonJsonDict, key: &str) -> Option<&'a PythonString> {
    get(object, key).and_then(as_string)
}

/// The scalar text of a member (only when scalar).
pub(crate) fn get_scalar(object: &PythonJsonDict, key: &str) -> Option<String> {
    get_text(object, key).and_then(text_scalar)
}

/// Insert a member from Rust text (position preserved on replacement).
pub(crate) fn insert(object: &mut PythonJsonDict, key: &str, value: Exact) {
    object.insert(PythonString::from_scalars(key), value);
}
/// Whether a value is exact `null`.
pub(crate) fn is_null(value: &Exact) -> bool {
    matches!(value, Exact::Null)
}

// ---------------------------------------------------------------------------
// read / render
// ---------------------------------------------------------------------------

/// Parsed exact document from UTF-8 bytes (empty input is an empty object).
pub(crate) fn parse_document(
    bytes: Option<&[u8]>,
    side: &str,
    name: &str,
) -> Result<PythonJsonDict, MergeDriverError> {
    let Some(bytes) = bytes else {
        return Ok(PythonJsonDict::new());
    };
    let text = std::str::from_utf8(bytes).map_err(|error| {
        MergeDriverError::message(format!("{side}: {name} is not valid UTF-8 ({error})"))
    })?;
    if text
        .trim_matches(|character| is_whitespace(u32::from(character)))
        .is_empty()
    {
        return Ok(PythonJsonDict::new());
    }
    let parsed = parse(text).map_err(|error| {
        MergeDriverError::message(format!("{side}: {name} is not valid JSON ({error})"))
    })?;
    match parsed {
        Exact::ExactObject(object) => Ok(object),
        other => Err(MergeDriverError::message(format!(
            "{side}: {name} is not a JSON object ({})",
            render_text(&other).unwrap_or_else(|_| "<unrenderable>".to_owned())
        ))),
    }
}

/// Render an exact value through the ONE canonical encoder and encode it.
///
/// `sort_keys` is `json.dumps(sort_keys=...)`; the text phase is J1's; the
/// physical encode is where a lone surrogate (only reachable with
/// `ensure_ascii = false`) fails — never earlier, never replaced.
/// Render an exact value to the TEXT phase (no encoding).
///
/// Source's drivers serialize to a Python `str` and only then call
/// `Path.write_text`, which opens/truncates the destination BEFORE encoding can
/// raise. The rendered [`PythonString`] is therefore carried to the driver's real
/// open/truncate step; encoding happens there, never here.
pub(crate) fn render(
    value: &Exact,
    indent: Option<usize>,
    ensure_ascii: bool,
    sort_keys: bool,
) -> PythonString {
    let mut rendered = value.render_python_with(PythonJsonRenderOptions {
        indent,
        ensure_ascii,
        sort_keys,
        ..PythonJsonRenderOptions::default()
    });
    rendered.push_scalars("\n");
    rendered
}

/// Render an exact value as scalar text (diagnostics and tests).
pub(crate) fn render_text(value: &Exact) -> Result<String, MergeDriverError> {
    render(value, None, true, true)
        .to_scalar_string()
        .map_err(|unit| {
            MergeDriverError::message(format!(
                "the rendered document holds the lone surrogate U+{unit:04X}"
            ))
        })
}

/// Encode a rendered text for the physical write, exactly as Source's
/// `Path.write_text(..., encoding="utf-8")` does when it hits a lone surrogate.
pub(crate) fn encode_text(text: &PythonString) -> Result<Vec<u8>, MergeDriverError> {
    text.to_scalar_string()
        .map(String::into_bytes)
        .map_err(|unit| {
            MergeDriverError::message(format!(
                "the rendered document holds the lone surrogate U+{unit:04X} and cannot be written \
             as UTF-8 (Source raises UnicodeEncodeError at the same physical write)"
            ))
        })
}

/// The exact code-point key of a Python string.
pub(crate) fn key_of_text(text: &PythonString) -> Key {
    text.codepoints().to_vec()
}

/// A key from Rust text.
pub(crate) fn key_of_scalar(text: &str) -> Key {
    text.chars().map(u32::from).collect()
}

/// Trim Python `str.strip()`-style whitespace, code point wise.
pub(crate) fn trim(text: &PythonString) -> PythonString {
    let units: Vec<u32> = text.codepoints().to_vec();
    let is_space = is_whitespace;
    let Some(start) = units.iter().position(|unit| !is_space(*unit)) else {
        return PythonString::default();
    };
    let end = units
        .iter()
        .rposition(|unit| !is_space(*unit))
        .map(|index| index + 1)
        .unwrap_or(start);
    PythonString::from_codepoints(&units[start..end]).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// shared row reconciliation (acceptance + issue matrix)
// ---------------------------------------------------------------------------

/// One row's identity key, computed from the row's own id field.
pub(crate) type KeyOf<'a> = &'a dyn Fn(&Key, &PythonJsonDict) -> Result<Key, MergeDriverError>;

/// Canonicalize one side's rows to `Key -> row`, rejecting an intra-side
/// collision of two DIFFERENT rows (Source's `RowMatrixMergeError` guard).
pub(crate) fn canonicalize_rows(
    rows: Option<&Exact>,
    key_of: KeyOf<'_>,
    name: &str,
) -> Result<BTreeMap<Key, PythonJsonDict>, MergeDriverError> {
    let mut canonical: BTreeMap<Key, PythonJsonDict> = BTreeMap::new();
    let Some(rows) = rows else {
        return Ok(canonical);
    };
    // Source iterates `rows or []`: strings yield only non-row characters,
    // and every falsy JSON value yields the empty iterable.
    if is_falsy(rows) || as_string(rows).is_some() {
        return Ok(canonical);
    }
    if let Some(items) = as_array(rows) {
        for (index, item) in items.iter().enumerate() {
            let Some(row) = as_object(item) else {
                continue;
            };
            let key = key_of(&key_of_scalar(&index.to_string()), row)?;
            insert_canonical(&mut canonical, key, row.clone())?;
        }
        return Ok(canonical);
    }
    if let Some(map) = as_object(rows) {
        for (raw_key, item) in map.entries() {
            let Some(row) = as_object(item) else {
                continue;
            };
            let key = key_of(&key_of_text(raw_key), row)?;
            insert_canonical(&mut canonical, key, row.clone())?;
        }
        return Ok(canonical);
    }
    Err(MergeDriverError::message(format!(
        "{name} rows are neither a list nor an object ({})",
        render_text(rows).unwrap_or_else(|_| "<unrenderable>".to_owned())
    )))
}

fn insert_canonical(
    canonical: &mut BTreeMap<Key, PythonJsonDict>,
    key: Key,
    row: PythonJsonDict,
) -> Result<(), MergeDriverError> {
    if let Some(existing) = canonical.get(&key)
        && !python_json_dict_equal(existing, &row)
    {
        let label = PythonString::from_codepoints(&key)
            .expect("row keys contain validated Python code points")
            .repr();
        return Err(MergeDriverError::message(format!(
            "intra-side duplicate row key {label}: two distinct rows on one side \
             normalize to the same canonical key — refusing to silently collapse either \
             (#2970-adjacent row-merge guard)"
        )));
    }
    canonical.insert(key, row);
    Ok(())
}

/// Source's per-field 3-way rule, on exact values.
pub(crate) fn merge_field(
    field: &str,
    base: Option<&Exact>,
    ours: Option<&Exact>,
    theirs: Option<&Exact>,
    sentinel: Option<&str>,
    row_key: &Key,
) -> Result<Exact, MergeDriverError> {
    let null = Exact::Null;
    let base = base.unwrap_or(&null);
    let ours = ours.unwrap_or(&null);
    let theirs = theirs.unwrap_or(&null);
    // ONE equality rule (Kernel Python `==` for decoded JSON values): bool/int/
    // float numeric equality, exact strings and containers, container NaN
    // identity. Never a private numeric policy.
    if python_json_equal(ours, theirs) {
        return Ok(ours.clone());
    }
    if python_json_equal(ours, base) {
        return Ok(theirs.clone());
    }
    if python_json_equal(theirs, base) {
        return Ok(ours.clone());
    }
    let Some(sentinel) = sentinel else {
        return Ok(ours.clone());
    };
    let sentinel_value = Exact::scalar(sentinel);
    if python_json_equal(theirs, &sentinel_value) && !python_json_equal(ours, &sentinel_value) {
        return Ok(ours.clone());
    }
    if python_json_equal(ours, &sentinel_value) && !python_json_equal(theirs, &sentinel_value) {
        return Ok(theirs.clone());
    }
    let label = PythonString::from_codepoints(row_key)
        .expect("row keys contain validated Python code points")
        .repr();
    Err(MergeDriverError::message(format!(
        "row {label}: verdict field {} diverged on both sides with no common \
         base value (target/ours={}, incoming/theirs={})",
        PythonString::from_scalars(field).repr(),
        python_repr_value(ours),
        python_repr_value(theirs)
    )))
}

/// Per-field merge of one row (Source `_merge_row_fields`), key order
/// `dict.fromkeys((*base, *ours, *theirs))`.
pub(crate) fn merge_row_fields(
    base: Option<&PythonJsonDict>,
    ours: &PythonJsonDict,
    theirs: &PythonJsonDict,
    sentinel: Option<(&str, &str)>,
    row_key: &Key,
) -> Result<PythonJsonDict, MergeDriverError> {
    let mut names: Vec<PythonString> = Vec::new();
    let mut push = |key: &PythonString| {
        if !names.iter().any(|existing| existing == key) {
            names.push(key.clone());
        }
    };
    if let Some(base) = base {
        for (key, _) in base.entries() {
            push(key);
        }
    }
    for (key, _) in ours.entries() {
        push(key);
    }
    for (key, _) in theirs.entries() {
        push(key);
    }
    let mut merged = PythonJsonDict::new();
    for name in names {
        let value = merge_field(
            &text_scalar(&name).unwrap_or_else(|| "<surrogate key>".to_owned()),
            base.and_then(|base| get_codepoints(base, name.codepoints())),
            get_codepoints(ours, name.codepoints()),
            get_codepoints(theirs, name.codepoints()),
            sentinel
                .filter(|(field, _)| text_is(&name, field))
                .map(|(_, value)| value),
            row_key,
        )?;
        merged.insert(name, value);
    }
    Ok(merged)
}

/// Source `_reconcile_row`: delete-vs-stale disambiguation + field merge.
pub(crate) fn reconcile_row(
    base: Option<&PythonJsonDict>,
    ours: Option<&PythonJsonDict>,
    theirs: Option<&PythonJsonDict>,
    sentinel: Option<(&str, &str)>,
    row_key: &Key,
) -> Result<Option<PythonJsonDict>, MergeDriverError> {
    let Some(base) = base else {
        return Ok(match (ours, theirs) {
            (None, None) => None,
            (None, Some(theirs)) => Some(theirs.clone()),
            (Some(ours), None) => Some(ours.clone()),
            (Some(ours), Some(theirs)) if python_json_dict_equal(ours, theirs) => {
                Some(ours.clone())
            }
            (Some(ours), Some(theirs)) => {
                Some(merge_row_fields(None, ours, theirs, sentinel, row_key)?)
            }
        });
    };
    Ok(match (ours, theirs) {
        (None, None) => None,
        (None, Some(theirs)) => (!python_json_dict_equal(theirs, base)).then(|| theirs.clone()),
        (Some(ours), None) => (!python_json_dict_equal(ours, base)).then(|| ours.clone()),
        (Some(ours), Some(theirs)) if python_json_dict_equal(ours, theirs) => Some(ours.clone()),
        (Some(ours), Some(theirs)) => Some(merge_row_fields(
            Some(base),
            ours,
            theirs,
            sentinel,
            row_key,
        )?),
    })
}

/// Source `_reconcile_keyed_rows`: merged rows in sorted-key order.
pub(crate) fn reconcile_keyed_rows(
    base: Option<&Exact>,
    ours: Option<&Exact>,
    theirs: Option<&Exact>,
    key_of: KeyOf<'_>,
    sentinel: Option<(&str, &str)>,
    name: &str,
) -> Result<Vec<(Key, PythonJsonDict)>, MergeDriverError> {
    let base = canonicalize_rows(base, key_of, name)?;
    let ours = canonicalize_rows(ours, key_of, name)?;
    let theirs = canonicalize_rows(theirs, key_of, name)?;
    let keys: Vec<&Key> = {
        let mut keys: Vec<&Key> = base
            .keys()
            .chain(ours.keys())
            .chain(theirs.keys())
            .collect();
        keys.sort();
        keys.dedup();
        keys
    };
    let mut merged = Vec::new();
    for key in keys {
        let row = reconcile_row(base.get(key), ours.get(key), theirs.get(key), sentinel, key)?;
        if let Some(row) = row {
            merged.push((key.clone(), row));
        }
    }
    Ok(merged)
}

/// Source `_canonicalize_issue_ref` on exact text: trim, then `#` + the trailing
/// run of decimal digits (an all-NUL/`surrogate` key without digits is returned
/// trimmed, unchanged).
pub(crate) fn canonicalize_issue_ref(raw: &PythonString) -> PythonString {
    // Source `_trailing_digits` uses `str.isdecimal()` (Unicode Nd), and the
    // canonical form keeps those digits' OWN code points (`#` + the raw run),
    // never an ASCII re-encoding. The Kernel text APIs are the one predicate.
    let trimmed = trim(raw);
    let units = trimmed.codepoints();
    let mut start = units.len();
    while start > 0 && is_decimal(units[start - 1]) {
        debug_assert!(decimal_value(units[start - 1]).is_some());
        start -= 1;
    }
    if start == units.len() {
        return trimmed;
    }
    let digits = &units[start..];
    let mut result: Vec<u32> = vec![u32::from(b'#')];
    result.extend_from_slice(digits);
    PythonString::from_codepoints(&result).unwrap_or(trimmed)
}

// ---------------------------------------------------------------------------
// meta.json
// ---------------------------------------------------------------------------

/// The exact keys of a document, in insertion order.
pub(crate) fn keys_of(object: &PythonJsonDict) -> Vec<Key> {
    object.keys().map(|key| key.codepoints().to_vec()).collect()
}

/// Source `_reconcile_meta_payloads` merge units, over exact keys.
pub(crate) fn merge_units(keys: &[Key], groups: &[&[&str]]) -> Vec<Vec<Key>> {
    let mut units: Vec<Vec<Key>> = Vec::new();
    let mut grouped: Vec<Key> = vec![key_of_scalar(super::ACCEPTANCE_HISTORY_FIELD)];
    for group in groups {
        if group
            .iter()
            .any(|key| keys.iter().any(|present| present == &key_of_scalar(key)))
        {
            let mut unit: Vec<Key> = group.iter().map(|key| key_of_scalar(key)).collect();
            unit.sort();
            grouped.extend(unit.iter().cloned());
            units.push(unit);
        }
    }
    for key in keys {
        if !grouped.iter().any(|known| known == key) {
            units.push(vec![key.clone()]);
        }
    }
    units
}

/// One unit's chosen value per member (Source `resolve_meta_unit`).
pub(crate) fn resolve_unit(
    unit: &[Key],
    base: &PythonJsonDict,
    ours: &PythonJsonDict,
    theirs: &PythonJsonDict,
    target_authoritative: &[Key],
) -> Vec<Option<Exact>> {
    let values = |document: &PythonJsonDict| -> Vec<Option<Exact>> {
        unit.iter()
            .map(|key| get_codepoints(document, key).cloned())
            .collect()
    };
    let base_values = values(base);
    let ours_values = values(ours);
    let theirs_values = values(theirs);
    // Source compares `dict.get(key, _MISSING)` tuples: a MISSING member
    // (`None`) is distinct from `Some(Null)`, and present members use the one
    // container equality rule.
    if python_json_optional_sequence_equal(&ours_values, &theirs_values)
        || python_json_optional_sequence_equal(&theirs_values, &base_values)
    {
        return ours_values;
    }
    if python_json_optional_sequence_equal(&ours_values, &base_values) {
        return theirs_values;
    }
    if unit
        .iter()
        .any(|key| target_authoritative.iter().any(|known| known == key))
    {
        ours_values
    } else {
        theirs_values
    }
}

/// `is_assigned_mission_number` on an exact value (a real positive int).
pub(crate) fn assigned_mission_number(value: Option<&Exact>) -> bool {
    // Source `is_assigned_mission_number`: a real `int` (NOT `bool`), value >= 1.
    // The reader's `Integer` token is a canonical Python int, so sign/zero is a
    // lexical check — no machine-width bound may reject a large positive int.
    match value {
        Some(Exact::Integer(digits)) => {
            let text = digits.as_str();
            if let Some(magnitude) = text.strip_prefix('-') {
                return magnitude.bytes().all(|byte| byte == b'0') && false;
            }
            let magnitude = text.strip_prefix('+').unwrap_or(text);
            !magnitude.is_empty()
                && magnitude.bytes().all(|byte| byte.is_ascii_digit())
                && magnitude.bytes().any(|byte| byte != b'0')
        }
        _ => false,
    }
}

/// Source `str(value)` sort key, with the caller's missing-member default `""`.
/// Strings retain exact code points; other values use the canonical repr walker.
fn str_key(value: Option<&Exact>) -> Key {
    match value {
        None => Vec::new(),
        Some(Exact::String(text)) => key_of_text(text),
        Some(value) => key_of_scalar(&python_repr_value(value)),
    }
}

/// Source history identity is `json.dumps(entry, sort_keys=True)` TEXT, NOT
/// Python value equality: `1`, `1.0`, and `True` must remain distinct entries.
pub(crate) fn union_acceptance_history(theirs: Option<&Exact>, ours: Option<&Exact>) -> Vec<Exact> {
    let mut combined: Vec<Exact> = Vec::new();
    let mut seen: BTreeSet<Key> = BTreeSet::new();
    for history in [theirs, ours] {
        let Some(items) = history.and_then(as_array) else {
            continue;
        };
        for entry in items {
            let identity = entry.render_python_with(PythonJsonRenderOptions {
                sort_keys: true,
                ..PythonJsonRenderOptions::default()
            });
            if seen.insert(key_of_text(&identity)) {
                combined.push(entry.clone());
            }
        }
    }
    combined.sort_by(|left, right| {
        let key = |value: &Exact, field: &str| -> Vec<u32> {
            str_key(as_object(value).and_then(|object| get(object, field)))
        };
        (key(left, "accepted_at"), key(left, "accepted_by"))
            .cmp(&(key(right, "accepted_at"), key(right, "accepted_by")))
    });
    combined
}

/// The newest history entry's `accept_commit` is not touched here: only the
/// union is a merge decision.
pub(crate) fn reconcile_meta(
    ours: &PythonJsonDict,
    theirs: &PythonJsonDict,
    base: Option<&PythonJsonDict>,
    groups: &[&[&str]],
    target_authoritative: &[&str],
) -> PythonJsonDict {
    let authoritative: Vec<Key> = target_authoritative
        .iter()
        .map(|key| key_of_scalar(key))
        .collect();
    let Some(base) = base.filter(|base| !base.is_empty()) else {
        // Two-way (Source's `_reconcile_meta_two_way`): theirs first, then the
        // target-authoritative fields from ours, then the history union.
        let mut result = theirs.clone();
        for key in target_authoritative {
            let Some(value) = get(ours, key) else {
                continue;
            };
            if *key == "mission_number" && !assigned_mission_number(Some(value)) {
                continue;
            }
            insert(&mut result, key, value.clone());
        }
        let history = union_acceptance_history(
            get(theirs, super::ACCEPTANCE_HISTORY_FIELD),
            get(ours, super::ACCEPTANCE_HISTORY_FIELD),
        );
        if !history.is_empty() {
            insert(
                &mut result,
                super::ACCEPTANCE_HISTORY_FIELD,
                Exact::Array(history),
            );
        }
        return result;
    };
    let keys = keys_of(base)
        .into_iter()
        .chain(keys_of(ours))
        .chain(keys_of(theirs))
        .collect::<Vec<Key>>();
    let keys = sorted_unique(keys);
    let mut result = PythonJsonDict::new();
    for unit in merge_units(&keys, groups) {
        let values = resolve_unit(&unit, base, ours, theirs, &authoritative);
        for (key, value) in unit.iter().zip(values) {
            if let Some(value) = value
                && let Ok(name) = PythonString::from_codepoints(key)
            {
                result.insert(name, value);
            }
        }
    }
    // `_apply_mission_number_guard`: an assigned number from either side wins.
    if !assigned_mission_number(get(&result, "mission_number")) {
        for side in [ours, theirs] {
            if assigned_mission_number(get(side, "mission_number"))
                && let Some(value) = get(side, "mission_number")
            {
                insert(&mut result, "mission_number", value.clone());
                break;
            }
        }
    }
    let history = union_acceptance_history(
        get(theirs, super::ACCEPTANCE_HISTORY_FIELD),
        get(ours, super::ACCEPTANCE_HISTORY_FIELD),
    );
    if !history.is_empty() {
        insert(
            &mut result,
            super::ACCEPTANCE_HISTORY_FIELD,
            Exact::Array(history),
        );
    }
    result
}

fn sorted_unique(mut keys: Vec<Key>) -> Vec<Key> {
    keys.sort();
    keys.dedup();
    keys
}

// ---------------------------------------------------------------------------
// acceptance-matrix.json
// ---------------------------------------------------------------------------

/// The criterion row identity key (`criterion_id`).
pub(crate) fn criterion_key(_raw: &Key, row: &PythonJsonDict) -> Result<Key, MergeDriverError> {
    Ok(get_text(row, "criterion_id")
        .map(key_of_text)
        .unwrap_or_default())
}

/// The invariant row identity key (`invariant_id`).
pub(crate) fn invariant_key(_raw: &Key, row: &PythonJsonDict) -> Result<Key, MergeDriverError> {
    Ok(get_text(row, "invariant_id")
        .map(key_of_text)
        .unwrap_or_default())
}

/// The document's identity fields, target (ours) authoritative.
pub(crate) fn acceptance_identity(
    base: &PythonJsonDict,
    ours: &PythonJsonDict,
    theirs: &PythonJsonDict,
) -> Vec<(PythonString, Exact)> {
    let mut entries = Vec::new();
    for key in ["mission_slug", "mission_number", "mission_type"] {
        let chosen = match get(ours, key) {
            Some(value) if !is_null(value) && !is_empty_text(value) => value.clone(),
            _ => get(theirs, key).cloned().unwrap_or(Exact::Null),
        };
        let chosen = match (&chosen, get(ours, key)) {
            (Exact::Null, Some(value)) => value.clone(),
            _ => chosen,
        };
        let chosen = if key == "mission_slug" && (is_null(&chosen) || is_empty_text(&chosen)) {
            get(base, key).cloned().unwrap_or_else(|| Exact::scalar(""))
        } else {
            chosen
        };
        // Source's `AcceptanceMatrix.to_dict()` writes the identity fields as
        // TEXT (`mission_number` included: `_identity_text` stringifies an int).
        entries.push((
            PythonString::from_scalars(key),
            identity_text(Some(&chosen)),
        ));
    }
    entries
}

fn is_empty_text(value: &Exact) -> bool {
    as_string(value).is_some_and(PythonString::is_empty)
}

/// The Source `AcceptanceMatrix.overall_verdict` dominance rule.
pub(crate) fn acceptance_verdict(document: &PythonJsonDict) -> &'static str {
    let texts = |field: &str, rows: &str| -> Vec<String> {
        get(document, rows)
            .and_then(as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(as_object)
                    .map(|row| get_scalar(row, field).unwrap_or_default())
                    .collect()
            })
            .unwrap_or_default()
    };
    let criterion_results = texts("pass_fail", "criteria");
    let invariant_results = texts("result", "negative_invariants");
    if criterion_results.is_empty() && invariant_results.is_empty() {
        return "pending";
    }
    let criterion_ok = |value: &str| matches!(value, "pass" | "fail" | "pending");
    // Source `TERMINAL_INVARIANT_RESULTS` + pending + the consolidation deferral.
    let invariant_ok = |value: &str| {
        sk_status::acceptance::TERMINAL_INVARIANT_RESULTS.contains(&value)
            || matches!(value, "pending" | "deferred_to_consolidation")
    };
    if criterion_results.iter().any(|value| !criterion_ok(value))
        || invariant_results.iter().any(|value| !invariant_ok(value))
    {
        return "fail";
    }
    if criterion_results.iter().any(|value| value == "fail")
        || invariant_results
            .iter()
            .any(|value| matches!(value.as_str(), "still_present" | "verification_error"))
    {
        return "fail";
    }
    let non_scaffold_results: Vec<String> = get(document, "criteria")
        .and_then(as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(as_object)
        .filter(|row| {
            !get_text(row, "description")
                .is_some_and(|text| text_is(text, "TODO: replace with a real acceptance criterion"))
        })
        .map(|row| get_scalar(row, "pass_fail").unwrap_or_default())
        .collect();
    let pending_dominant = if non_scaffold_results.is_empty() {
        &criterion_results
    } else {
        &non_scaffold_results
    };
    if pending_dominant.iter().any(|value| value == "pending")
        || invariant_results.iter().any(|value| value == "pending")
    {
        return "pending";
    }
    if invariant_results
        .iter()
        .any(|value| value == "deferred_to_consolidation")
    {
        return "pass_pending_consolidation";
    }
    "pass"
}

/// The declared criterion fields, in Source's `to_dict()` order.
pub(crate) const CRITERION_FIELDS: [&str; 8] = [
    "criterion_id",
    "description",
    "proof_type",
    "evidence",
    "pass_fail",
    "verified_by",
    "verified_at",
    "notes",
];

/// The declared invariant fields, in Source's `to_dict()` order.
pub(crate) const INVARIANT_FIELDS: [&str; 6] = [
    "invariant_id",
    "description",
    "verification_method",
    "verification_command",
    "result",
    "evidence",
];

/// The invariant's optional fields, emitted only when the document carries them.
pub(crate) const INVARIANT_OPTIONALS: [&str; 6] = [
    "scope",
    "verified_ref",
    "verified_surface_kind",
    "deferred_reason",
    "deferred_to_phase",
    "provenance_origin",
];

/// Project one row onto Source's typed shape: declared fields in order (missing
/// ones take Source's defaults), then the unknown keys in the given order.
pub(crate) fn project_row(
    row: &PythonJsonDict,
    fields: &[&str],
    defaults: &dyn Fn(&str) -> Exact,
    order: Option<&[Key]>,
) -> Exact {
    let mut entries: Vec<(PythonString, Exact)> = Vec::new();
    for field in fields {
        let value = get(row, field).cloned().unwrap_or_else(|| defaults(field));
        entries.push((PythonString::from_scalars(field), value));
    }
    append_extras(&mut entries, row, fields, order);
    object(entries)
}

fn append_extras(
    entries: &mut Vec<(PythonString, Exact)>,
    row: &PythonJsonDict,
    known: &[&str],
    order: Option<&[Key]>,
) {
    let mut emitted: Vec<PythonString> = Vec::new();
    if let Some(order) = order {
        for key in order {
            let Some(name) = PythonString::from_codepoints(key).ok() else {
                continue;
            };
            if known.iter().any(|known| text_is(&name, known)) {
                continue;
            }
            if emitted.iter().any(|existing| existing == &name) {
                continue;
            }
            if let Some(value) = get_codepoints(row, key) {
                emitted.push(name.clone());
                entries.push((name, value.clone()));
            }
        }
    }
    for (name, value) in row.entries() {
        let scalar = text_scalar(name);
        if known
            .iter()
            .any(|known| scalar.as_deref() == Some(*known) || text_is(name, known))
        {
            continue;
        }
        if emitted.iter().any(|existing| existing == name) {
            continue;
        }
        emitted.push(name.clone());
        entries.push((name.clone(), value.clone()));
    }
}

/// Source's criterion defaults (`AcceptanceCriterion.from_dict`).
pub(crate) fn criterion_default(field: &str) -> Exact {
    match field {
        "criterion_id" | "description" | "proof_type" => Exact::scalar(""),
        "pass_fail" => Exact::scalar(VERDICT_PENDING),
        _ => Exact::Null,
    }
}

/// Source's invariant defaults (`NegativeInvariant.from_dict`).
pub(crate) fn invariant_default(field: &str) -> Exact {
    match field {
        "invariant_id" | "description" | "verification_method" => Exact::scalar(""),
        "result" => Exact::scalar(VERDICT_PENDING),
        "provenance_origin" => Exact::scalar(PROVENANCE_LEGACY_UNRECORDED),
        _ => Exact::Null,
    }
}

/// Source `VERDICT_PENDING`.
pub(crate) const VERDICT_PENDING: &str = "pending";
/// Source's unrecorded invariant provenance sentinel.
pub(crate) const PROVENANCE_LEGACY_UNRECORDED: &str = "legacy_unrecorded";

/// Source `mission_identity_text`: a string as-is, an int by its exact token,
/// anything else the empty string.
fn identity_text(value: Option<&Exact>) -> Exact {
    match value {
        Some(Exact::String(text)) => Exact::String(text.clone()),
        Some(Exact::Integer(token)) => Exact::scalar(token),
        Some(Exact::Number(token)) => Exact::scalar(token),
        _ => Exact::scalar(""),
    }
}

/// Dataclass construction requires presence, not string coercion/validation.
/// Validate MERGED rows only: an invalid discarded side is not a new refusal.
fn required_acceptance_fields(
    rows: &[(Key, PythonJsonDict)],
    section: &str,
    class: &str,
    fields: &[&str],
) -> Result<(), MergeDriverError> {
    for (index, (_, row)) in rows.iter().enumerate() {
        let missing: Vec<String> = fields
            .iter()
            .filter(|field| get(row, field).is_none())
            .map(|field| PythonString::from_scalars(field).repr())
            .collect();
        let Some(last) = missing.last() else {
            continue;
        };
        let names = match missing.len() {
            1 => last.clone(),
            2 => format!("{} and {last}", missing[0]),
            _ => format!("{}, and {last}", missing[..missing.len() - 1].join(", ")),
        };
        let plural = if missing.len() == 1 { "" } else { "s" };
        return Err(MergeDriverError::message(format!(
            "{section}[{index}]: malformed shape ({class}.__init__() missing {} required positional argument{plural}: {names})",
            missing.len()
        )));
    }
    Ok(())
}

fn reject_acceptance_markers(
    rows: &[(Key, PythonJsonDict)],
    section: &str,
    id: &str,
    fields: &[&str],
) -> Result<(), MergeDriverError> {
    for (index, (_, row)) in rows.iter().enumerate() {
        for field in fields {
            let Some(text) = get_text(row, field) else {
                continue;
            };
            for marker in ["<<<<<<<", "=======", ">>>>>>>"] {
                let units: Vec<u32> = marker.chars().map(u32::from).collect();
                if text
                    .codepoints()
                    .windows(units.len())
                    .any(|part| part == units)
                {
                    let key = get(row, id).map(python_repr_value).unwrap_or_default();
                    return Err(MergeDriverError::message(format!(
                        "{section}[{index}]: malformed shape (field {} on row {key} contains an unresolved conflict marker {})",
                        PythonString::from_scalars(field).repr(),
                        PythonString::from_scalars(marker).repr()
                    )));
                }
            }
        }
    }
    Ok(())
}

/// The merged acceptance matrix, projected onto Source's `to_dict()` shape.
///
/// Every merged row's unknown keys are emitted in Source's `dict.fromkeys((*base,
/// *ours, *theirs))` order, read from the exact side documents.
pub(crate) fn acceptance_document(
    base: &PythonJsonDict,
    ours: &PythonJsonDict,
    theirs: &PythonJsonDict,
) -> Result<Exact, MergeDriverError> {
    let criteria = reconcile_keyed_rows(
        get(base, "criteria"),
        get(ours, "criteria"),
        get(theirs, "criteria"),
        &criterion_key,
        Some(("pass_fail", VERDICT_PENDING)),
        "criteria",
    )?;
    let invariants = reconcile_keyed_rows(
        get(base, "negative_invariants"),
        get(ours, "negative_invariants"),
        get(theirs, "negative_invariants"),
        &invariant_key,
        Some(("result", VERDICT_PENDING)),
        "negative_invariants",
    )?;
    let base_criteria = canonicalize_rows(get(base, "criteria"), &criterion_key, "criteria")?;
    let ours_criteria = canonicalize_rows(get(ours, "criteria"), &criterion_key, "criteria")?;
    let theirs_criteria = canonicalize_rows(get(theirs, "criteria"), &criterion_key, "criteria")?;
    let base_invariants = canonicalize_rows(
        get(base, "negative_invariants"),
        &invariant_key,
        "negative_invariants",
    )?;
    let ours_invariants = canonicalize_rows(
        get(ours, "negative_invariants"),
        &invariant_key,
        "negative_invariants",
    )?;
    let theirs_invariants = canonicalize_rows(
        get(theirs, "negative_invariants"),
        &invariant_key,
        "negative_invariants",
    )?;

    // Source constructs ALL criteria, then ALL invariants, and only then runs
    // the declared-field marker guards (extras are authored and not scanned).
    required_acceptance_fields(
        &criteria,
        "criteria",
        "AcceptanceCriterion",
        &CRITERION_FIELDS[..3],
    )?;
    required_acceptance_fields(
        &invariants,
        "negative_invariants",
        "NegativeInvariant",
        &INVARIANT_FIELDS[..3],
    )?;
    reject_acceptance_markers(&criteria, "criteria", "criterion_id", &CRITERION_FIELDS)?;
    reject_acceptance_markers(
        &invariants,
        "negative_invariants",
        "invariant_id",
        &[&INVARIANT_FIELDS[..], &INVARIANT_OPTIONALS[..]].concat(),
    )?;

    // `overall_verdict` is computed from the reconciled rows, never merged.
    let mut rows = PythonJsonDict::new();
    let criterion_values: Vec<Exact> = criteria
        .iter()
        .map(|(key, row)| {
            let order = row_extra_order(
                key,
                [
                    base_criteria.get(key),
                    ours_criteria.get(key),
                    theirs_criteria.get(key),
                ],
            );
            project_criterion(row, order.as_deref())
        })
        .collect();
    let invariant_values: Vec<Exact> = invariants
        .iter()
        .map(|(key, row)| {
            let order = row_extra_order(
                key,
                [
                    base_invariants.get(key),
                    ours_invariants.get(key),
                    theirs_invariants.get(key),
                ],
            );
            project_invariant(row, order.as_deref())
        })
        .collect();
    rows.insert(
        PythonString::from_scalars("criteria"),
        Exact::Array(criterion_values),
    );
    rows.insert(
        PythonString::from_scalars("negative_invariants"),
        Exact::Array(invariant_values),
    );

    let mut document = PythonJsonDict::new();
    for (key, value) in acceptance_identity(base, ours, theirs) {
        document.insert(key, value);
    }
    let verdict = acceptance_verdict(&rows);
    document.insert(
        PythonString::from_scalars("overall_verdict"),
        Exact::scalar(verdict),
    );
    document.insert(
        PythonString::from_scalars("criteria"),
        get(&rows, "criteria").cloned().unwrap_or(Exact::Null),
    );
    document.insert(
        PythonString::from_scalars("negative_invariants"),
        get(&rows, "negative_invariants")
            .cloned()
            .unwrap_or(Exact::Null),
    );
    Ok(Exact::ExactObject(document))
}

/// `dict.fromkeys((*base, *ours, *theirs))` order of one merged row's keys.
fn row_extra_order(key: &Key, sides: [Option<&PythonJsonDict>; 3]) -> Option<Vec<Key>> {
    let mut order: Vec<Key> = Vec::new();
    for side in sides.into_iter().flatten() {
        for (name, _) in side.entries() {
            let candidate = key_of_text(name);
            if !order.iter().any(|existing| existing == &candidate) {
                order.push(candidate);
            }
        }
    }
    if order.is_empty() {
        None
    } else {
        let _ = key;
        Some(order)
    }
}

/// Source's criterion row projection: the eight declared fields, then extras.
fn project_criterion(row: &PythonJsonDict, order: Option<&[Key]>) -> Exact {
    project_row(row, &CRITERION_FIELDS, &criterion_default, order)
}

/// Source's invariant row projection: the six declared fields, the optionals it
/// actually carries, and the unknown keys.
pub(crate) fn project_invariant(row: &PythonJsonDict, order: Option<&[Key]>) -> Exact {
    let mut entries: Vec<(PythonString, Exact)> = Vec::new();
    for field in INVARIANT_FIELDS {
        let value = get(row, field)
            .cloned()
            .unwrap_or_else(|| invariant_default(field));
        entries.push((PythonString::from_scalars(field), value));
    }
    for field in INVARIANT_OPTIONALS {
        match get(row, field) {
            None => {}
            Some(Exact::Null) if field != "provenance_origin" => {}
            Some(value) => {
                if field == "provenance_origin"
                    && as_string(value)
                        .is_some_and(|text| text_is(text, PROVENANCE_LEGACY_UNRECORDED))
                {
                    continue;
                }
                entries.push((PythonString::from_scalars(field), value.clone()));
            }
        }
    }
    append_extras(
        &mut entries,
        row,
        &[&INVARIANT_FIELDS[..], &INVARIANT_OPTIONALS[..]].concat(),
        order,
    );
    object(entries)
}

// ---------------------------------------------------------------------------
// decisions/index.json
// ---------------------------------------------------------------------------
//
// Source `run_decision_index_driver` does not merge two raw documents and
// publish them: it unions the entries, then validates the WHOLE merged
// document through `specify_cli.decisions.models.DecisionIndex`
// (`DecisionIndex.model_validate`, pydantic v2 lax mode), sorts the validated
// entries by `(created_at.isoformat(), decision_id)` and serializes
// `model_dump(mode="json")` with `json.dumps(sort_keys=True, indent=2)`.
//
// This section owns the RAW UNION only: entry identity, both-sides precedence,
// the terminal-beats-open rule and the top-level fallback. The typed MODEL
// contract (schema, defaults, coercion, enums, the pydantic datetime domain and
// the JSON projection) is ONE implementation in [`crate::decision_model`], which
// the read-only store shares; `project_decision_index` below delegates to it and
// adapts its typed refusal into this driver's message error.
//
// NOT claimed here: pydantic's `ValidationError` wire text. A refusal names the
// document, entry and field natively; reproducing the pinned traceback prose is
// separate error-wire work (H5 F11 family), never a fabricated message.
// ---------------------------------------------------------------------------

const TERMINAL_DECISION_STATUSES: [&str; 3] = ["resolved", "deferred", "canceled"];

fn decision_entries(
    document: &PythonJsonDict,
    side: &str,
) -> Result<BTreeMap<Key, PythonJsonDict>, MergeDriverError> {
    let mut by_id = BTreeMap::new();
    let Some(entries) = get(document, "entries") else {
        return Ok(by_id);
    };
    let Some(items) = as_array(entries) else {
        return Err(MergeDriverError::message(format!(
            "{side}: 'entries' is not a list ({})",
            render_text(entries).unwrap_or_else(|_| "<unrenderable>".to_owned())
        )));
    };
    for entry in items {
        let Some(entry) = as_object(entry) else {
            return Err(MergeDriverError::message(format!(
                "{side}: entry is not a JSON object ({})",
                render_text(entry).unwrap_or_else(|_| "<unrenderable>".to_owned())
            )));
        };
        let Some(decision_id) = get_text(entry, "decision_id") else {
            return Err(MergeDriverError::message(format!(
                "{side}: entry missing 'decision_id'"
            )));
        };
        if decision_id.is_empty() {
            return Err(MergeDriverError::message(format!(
                "{side}: entry missing 'decision_id'"
            )));
        }
        by_id.insert(key_of_text(decision_id), entry.clone());
    }
    Ok(by_id)
}

fn decision_status(
    entry: &PythonJsonDict,
    side: &str,
    id: &str,
) -> Result<String, MergeDriverError> {
    let Some(status) = get_scalar(entry, "status") else {
        return Err(MergeDriverError::message(format!(
            "{side}: decision {id:?} has an invalid 'status' (null)"
        )));
    };
    // Union precedence reads the SAME status enum as the typed model: one
    // member table, never a second list of accepted values.
    if !DecisionStatus::ALL
        .iter()
        .any(|member| member.as_str() == status)
    {
        return Err(MergeDriverError::message(format!(
            "{side}: decision {id:?} has an invalid 'status' ({status:?})"
        )));
    }
    Ok(status)
}

fn is_allowed_terminal_reopen(from: &str, to: &str) -> bool {
    from == "deferred" && to == "resolved"
}

fn resolve_decision_entry(
    id: &str,
    ours: Option<&PythonJsonDict>,
    theirs: Option<&PythonJsonDict>,
) -> Result<PythonJsonDict, MergeDriverError> {
    let (ours_entry, theirs_entry) = match (ours, theirs) {
        (None, None) => {
            return Err(MergeDriverError::message(format!(
                "decision {id:?} resolved with no entry on either side"
            )));
        }
        (None, Some(theirs)) => return Ok(theirs.clone()),
        (Some(ours), None) => return Ok(ours.clone()),
        (Some(ours), Some(theirs)) => (ours, theirs),
    };
    if python_json_dict_equal(ours_entry, theirs_entry) {
        return Ok(ours_entry.clone());
    }
    let ours_status = decision_status(ours_entry, "ours", id)?;
    let theirs_status = decision_status(theirs_entry, "theirs", id)?;
    let ours_terminal = TERMINAL_DECISION_STATUSES.contains(&ours_status.as_str());
    let theirs_terminal = TERMINAL_DECISION_STATUSES.contains(&theirs_status.as_str());
    if ours_terminal && !theirs_terminal {
        return Ok(ours_entry.clone());
    }
    if theirs_terminal && !ours_terminal {
        return Ok(theirs_entry.clone());
    }
    if ours_terminal && theirs_terminal && ours_status != theirs_status {
        if is_allowed_terminal_reopen(&ours_status, &theirs_status) {
            return Ok(theirs_entry.clone());
        }
        if is_allowed_terminal_reopen(&theirs_status, &ours_status) {
            return Ok(ours_entry.clone());
        }
    }
    Err(MergeDriverError::message(format!(
        "decision {id:?} diverged on both sides with no safe precedence (ours \
         status={ours_status:?}, theirs status={theirs_status:?}) -- refusing to silently \
         pick a side"
    )))
}

/// The unioned `decisions/index.json` entries (no `DecisionIndex` projection).
pub(crate) fn union_decision_entries(
    ours: &PythonJsonDict,
    theirs: &PythonJsonDict,
) -> Result<Exact, MergeDriverError> {
    let merged_entries = merged_decision_entries(ours, theirs)?;
    let mut document = decision_document(ours, theirs);
    let entries: Vec<Exact> = merged_entries
        .into_iter()
        .map(|(_, entry)| Exact::ExactObject(entry))
        .collect();
    insert(&mut document, "entries", Exact::Array(entries));
    Ok(Exact::ExactObject(document))
}

/// Union `decisions/index.json` onto Source's `DecisionIndex` shape.
pub(crate) fn union_decision_index(
    ours: &PythonJsonDict,
    theirs: &PythonJsonDict,
) -> Result<Exact, MergeDriverError> {
    let merged_entries = merged_decision_entries(ours, theirs)?;
    let document = decision_document(ours, theirs);
    project_decision_index(&document, merged_entries)
}

fn merged_decision_entries(
    ours: &PythonJsonDict,
    theirs: &PythonJsonDict,
) -> Result<Vec<(Key, PythonJsonDict)>, MergeDriverError> {
    let ours_entries = decision_entries(ours, "ours")?;
    let theirs_entries = decision_entries(theirs, "theirs")?;
    let ids = sorted_unique(
        ours_entries
            .keys()
            .chain(theirs_entries.keys())
            .cloned()
            .collect(),
    );
    let mut merged_entries: Vec<(Key, PythonJsonDict)> = Vec::new();
    for id in ids {
        let label = PythonString::from_codepoints(&id)
            .ok()
            .and_then(|text| text_scalar(&text))
            .unwrap_or_else(|| "<surrogate key>".to_owned());
        let entry = resolve_decision_entry(&label, ours_entries.get(&id), theirs_entries.get(&id))?;
        merged_entries.push((id, entry));
    }
    merged_entries.sort_by(|left, right| {
        let key = |entry: &PythonJsonDict| -> (Key, Key) {
            (
                str_key(get(entry, "created_at")),
                get_text(entry, "decision_id")
                    .map(key_of_text)
                    .unwrap_or_default(),
            )
        };
        key(&left.1).cmp(&key(&right.1))
    });
    Ok(merged_entries)
}

/// Raw union fields: the typed DRIVER projection, not this union, owns defaults.
fn decision_document(ours: &PythonJsonDict, theirs: &PythonJsonDict) -> PythonJsonDict {
    let mut names: Vec<PythonString> = Vec::new();
    for (name, _) in ours.entries() {
        if !text_is(name, "entries") {
            names.push(name.clone());
        }
    }
    for (name, _) in theirs.entries() {
        if !text_is(name, "entries") && !names.iter().any(|known| known == name) {
            names.push(name.clone());
        }
    }
    let mut ordered = PythonJsonDict::new();
    for name in names {
        let ours_value = get_codepoints(ours, name.codepoints()).cloned();
        let chosen = match &ours_value {
            Some(value) if !is_null(value) && !is_empty_text(value) => value.clone(),
            _ => get_codepoints(theirs, name.codepoints())
                .cloned()
                .unwrap_or(Exact::Null),
        };
        ordered.insert(name, chosen);
    }
    ordered
}

// ---------------------------------------------------------------------------
// typed `DecisionIndex` model (one authority, shared with the read-only store)
// ---------------------------------------------------------------------------

/// Validate the merged document through the canonical typed model and project
/// its JSON form.
///
/// The union above -- not this function and not the model -- owns entry identity,
/// conflict precedence and the merged entry ORDER. The typed model lives in
/// [`crate::decision_model`] so that this projection and the read-only index
/// store ([`crate::decision_store`]) validate and project through ONE
/// implementation instead of two.
fn project_decision_index(
    document: &PythonJsonDict,
    merged_entries: Vec<(Key, PythonJsonDict)>,
) -> Result<Exact, MergeDriverError> {
    let mut merged = document.clone();
    insert(
        &mut merged,
        "entries",
        Exact::Array(
            merged_entries
                .into_iter()
                .map(|(_, entry)| Exact::ExactObject(entry))
                .collect(),
        ),
    );
    decision_model::validate_index(&merged)
        .map(|index| index.project_sorted())
        .map_err(merge_decision_model_error)
}

/// The merge-side adapter: a typed model refusal becomes this driver's message
/// error with exactly the text the driver has always emitted for it. The model
/// never depends on merge errors; the dependency points only this way.
fn merge_decision_model_error(error: DecisionModelError) -> MergeDriverError {
    MergeDriverError::message(error.to_string())
}

// ---------------------------------------------------------------------------
// event log (JSONL)
// ---------------------------------------------------------------------------

/// Parse exact JSONL event rows, requiring a non-empty `event_id` per line.
pub(crate) fn read_event_rows(
    bytes: Option<&[u8]>,
    source: &str,
) -> Result<Vec<PythonJsonDict>, MergeDriverError> {
    let text = match bytes {
        None => "",
        Some(bytes) => std::str::from_utf8(bytes).map_err(|error| {
            MergeDriverError::message(format!("{source} is not valid UTF-8 ({error})"))
        })?,
    };
    let mut rows = Vec::new();
    for (index, raw) in text.split('\n').enumerate() {
        let stripped = raw.trim();
        if stripped.is_empty() {
            continue;
        }
        let parsed = parse(stripped).map_err(|error| {
            MergeDriverError::message(format!(
                "{source}: invalid JSON on line {}: {error}",
                index + 1
            ))
        })?;
        let Some(row) = as_object(&parsed) else {
            return Err(MergeDriverError::message(format!(
                "{source}: line {} is not a JSON object",
                index + 1
            )));
        };
        match get_text(row, "event_id") {
            Some(id) if !trim(id).is_empty() => rows.push(row.clone()),
            _ => {
                return Err(MergeDriverError::message(format!(
                    "{source}: line {} is missing a valid event_id",
                    index + 1
                )));
            }
        }
    }
    Ok(rows)
}

fn event_sort_key(row: &PythonJsonDict) -> (Key, Key) {
    let at = str_key(
        get(row, "at")
            .filter(|value| !is_falsy(value))
            .or_else(|| get(row, "timestamp")),
    );
    let id = get_text(row, "event_id")
        .map(key_of_text)
        .unwrap_or_default();
    (at, id)
}

/// Union event payloads, dedupe by exact `event_id`, sort by `(at|timestamp, id)`.
pub(crate) fn merge_event_rows(
    groups: impl IntoIterator<Item = Vec<PythonJsonDict>>,
) -> Result<Vec<PythonJsonDict>, MergeDriverError> {
    let mut merged: BTreeMap<Key, PythonJsonDict> = BTreeMap::new();
    for group in groups {
        for row in group {
            let id = get_text(&row, "event_id")
                .map(key_of_text)
                .unwrap_or_default();
            if let Some(existing) = merged.get(&id)
                && !python_json_dict_equal(existing, &row)
            {
                let label = PythonString::from_codepoints(&id)
                    .ok()
                    .and_then(|text| text_scalar(&text))
                    .unwrap_or_else(|| "<surrogate id>".to_owned());
                return Err(MergeDriverError::message(format!(
                    "Conflicting payloads found for event_id {label:?}"
                )));
            }
            merged.insert(id, row);
        }
    }
    let mut rows: Vec<PythonJsonDict> = merged.into_values().collect();
    rows.sort_by_key(event_sort_key);
    Ok(rows)
}

/// Render event rows one JSON object per line (`json.dumps(row, sort_keys=True)`
/// + `\\n`, ASCII-escaped like Source's event-log writer).
pub(crate) fn render_event_rows(rows: &[PythonJsonDict]) -> PythonString {
    let mut out = PythonString::default();
    for row in rows {
        out.push_scalars("");
        let line = render(&Exact::ExactObject(row.clone()), None, true, true);
        for code in line.codepoints() {
            out.push_codepoint(*code)
                .expect("a rendered code point is always a valid Python string unit");
        }
    }
    out
}

// ---------------------------------------------------------------------------
// issue-matrix.json
// ---------------------------------------------------------------------------

/// Source `_canonicalize_issue_ref` as the issue row key.
pub(crate) fn issue_row_key(raw: &Key, _row: &PythonJsonDict) -> Result<Key, MergeDriverError> {
    let text = PythonString::from_codepoints(raw).unwrap_or_default();
    Ok(key_of_text(&canonicalize_issue_ref(&text)))
}

/// The merged `issue-matrix.json` document (rows keyed by canonical issue ref).
pub(crate) fn union_issue_matrix(
    base: &PythonJsonDict,
    ours: &PythonJsonDict,
    theirs: &PythonJsonDict,
) -> Result<Exact, MergeDriverError> {
    let rows = reconcile_keyed_rows(
        get(base, "rows"),
        get(ours, "rows"),
        get(theirs, "rows"),
        &issue_row_key,
        Some(("verdict", "unknown")),
        "rows",
    )?;
    let mut merged = PythonJsonDict::new();
    insert(
        &mut merged,
        "schema_version",
        Exact::Integer("1".to_owned()),
    );
    let mut projected = PythonJsonDict::new();
    for (key, row) in rows {
        if let Ok(name) = PythonString::from_codepoints(&key) {
            projected.insert(name, Exact::ExactObject(row));
        }
    }
    insert(&mut merged, "rows", Exact::ExactObject(projected));
    Ok(Exact::ExactObject(merged))
}

// ---------------------------------------------------------------------------
// concurrent-fork detector (event log), on exact rows
// ---------------------------------------------------------------------------

/// Exact counterpart of the concurrent-fork detector: a cancel/rejection made in
/// two clones from the same lane is reported, a purely forward fork is not.
pub(crate) fn detect_concurrent_fork(
    base: &[PythonJsonDict],
    ours: &[PythonJsonDict],
    theirs: &[PythonJsonDict],
) -> Option<String> {
    type Destinations = BTreeMap<Key, BTreeMap<Key, std::collections::BTreeSet<Key>>>;
    let base_ids: std::collections::BTreeSet<Key> = base
        .iter()
        .filter_map(|row| get_text(row, "event_id"))
        .map(key_of_text)
        .collect();
    let departures = |rows: &[PythonJsonDict]| -> Destinations {
        let mut by_wp: Destinations = BTreeMap::new();
        for row in rows {
            let Some(id) = get_text(row, "event_id").map(key_of_text) else {
                continue;
            };
            if base_ids.contains(&id) {
                continue;
            }
            let (Some(from), Some(to), Some(wp)) = (
                get_text(row, "from_lane"),
                get_text(row, "to_lane"),
                get_text(row, "wp_id"),
            ) else {
                continue;
            };
            by_wp
                .entry(key_of_text(wp))
                .or_default()
                .entry(key_of_text(from))
                .or_default()
                .insert(key_of_text(to));
        }
        by_wp
    };
    let ours = departures(ours);
    let theirs = departures(theirs);
    for (wp, ours_from) in &ours {
        let Some(theirs_from) = theirs.get(wp) else {
            continue;
        };
        for (from_lane, ours_to) in ours_from {
            let Some(theirs_to) = theirs_from.get(from_lane) else {
                continue;
            };
            let destinations: std::collections::BTreeSet<&Key> = ours_to.union(theirs_to).collect();
            if destinations.len() > 1 {
                let from_text = PythonString::from_codepoints(from_lane).unwrap_or_default();
                let decision = destinations.iter().any(|destination| {
                    let to_text = PythonString::from_codepoints(destination).unwrap_or_default();
                    let (Some(from), Some(to)) = (text_scalar(&from_text), text_scalar(&to_text))
                    else {
                        return false;
                    };
                    super::is_decision_destination(&from, &to)
                });
                if decision {
                    let wp_text = PythonString::from_codepoints(wp).unwrap_or_default();
                    let wp =
                        text_scalar(&wp_text).unwrap_or_else(|| "<surrogate wp_id>".to_owned());
                    let from = text_scalar(&from_text).unwrap_or_else(|| "<surrogate>".to_owned());
                    let rendered = Exact::Array(
                        destinations
                            .iter()
                            .map(|destination| {
                                Exact::String(
                                    PythonString::from_codepoints(destination)
                                        .expect("lane keys contain validated Python code points"),
                                )
                            })
                            .collect(),
                    );
                    return Some(format!(
                        "work package {wp}: concurrent transitions from '{from}' to \
                         {} were made in two clones without a pull between them; a \
                         union merge would silently drop one. Pull, reconcile the divergence by \
                         hand (keep the cancel/rejection, redo the other), then re-run.",
                        python_repr_value(&rendered)
                    ));
                }
            }
        }
    }
    None
}
