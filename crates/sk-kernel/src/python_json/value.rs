//! Exact-code-point object storage and Python value text for the canonical tree.
//!
//! [`PythonJsonDict`] is the reader's object representation: keys are exact
//! [`PythonString`] code points (all 2048 surrogate units included), insertion
//! ordered, with Python `dict` assignment semantics (a duplicate key keeps its
//! first position and takes the last value). It is not a second value tree: the
//! reader, the renderer and the legacy Op adapter all share
//! [`PythonJsonValue`](super::PythonJsonValue).
//!
//! The value-text walkers here are the single `str()`/`repr()` and truthiness
//! authority for a decoded JSON value. `C`/`repr` formatting itself stays in
//! [`crate::python_repr`]; no second Unicode or float formatter exists.

use super::PythonJsonValue;
use crate::PythonString;

/// A Python `dict` decoded from JSON, keyed by exact string code points.
///
/// Keys may hold lone surrogate units (`json.loads('{"\\ud800": 1}')`), which a
/// Rust `String` cannot carry, so the canonical reader uses this type instead
/// of tag namespaces, escaped pseudo-keys or a lossy projection.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PythonJsonDict {
    entries: Vec<(PythonString, PythonJsonValue)>,
}

impl PythonJsonDict {
    /// An empty object.
    pub fn new() -> Self {
        Self::default()
    }

    /// The number of distinct keys.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the object holds no members (`{}`).
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The value stored under a Rust scalar key, if any.
    ///
    /// Comparison is code-point equality, so a surrogate key is reachable
    /// through [`PythonJsonDict::get_codepoints`] instead.
    pub fn get(&self, key: &str) -> Option<&PythonJsonValue> {
        self.entries
            .iter()
            .find_map(|(name, value)| codepoints_eq_str(name.codepoints(), key).then_some(value))
    }

    /// The value stored under exact code-point key, if any.
    pub fn get_codepoints(&self, key: &[u32]) -> Option<&PythonJsonValue> {
        self.entries
            .iter()
            .find_map(|(name, value)| (name.codepoints() == key).then_some(value))
    }

    /// Python `dict` assignment: replace in place, otherwise append.
    pub fn insert(&mut self, key: PythonString, value: PythonJsonValue) {
        Self::assign_entry(&mut self.entries, key, value);
    }

    /// The same exact-string dictionary assignment for phase-specific values.
    ///
    /// Model validation can normalize keys before serializing borrowed values;
    /// it must retain the same first-position/last-value rule as this JSON dict.
    pub fn assign_entry<T>(entries: &mut Vec<(PythonString, T)>, key: PythonString, value: T) {
        match entries.iter().position(|(name, _)| name == &key) {
            Some(position) => entries[position].1 = value,
            None => entries.push((key, value)),
        }
    }

    /// Retain members in place without changing surviving key positions.
    pub fn retain(&mut self, mut keep: impl FnMut(&PythonString, &PythonJsonValue) -> bool) {
        self.entries.retain(|(key, value)| keep(key, value));
    }

    /// The members in decoded order.
    pub fn entries(&self) -> &[(PythonString, PythonJsonValue)] {
        &self.entries
    }

    /// The members in decoded order.
    pub fn iter(&self) -> std::slice::Iter<'_, (PythonString, PythonJsonValue)> {
        self.entries.iter()
    }

    /// The keys in decoded order.
    pub fn keys(&self) -> impl Iterator<Item = &PythonString> {
        self.entries.iter().map(|(key, _)| key)
    }
}

/// Code-point equality between a Python string and Rust text.
fn codepoints_eq_str(units: &[u32], text: &str) -> bool {
    units.len() == text.chars().count() && units.iter().copied().eq(text.chars().map(u32::from))
}

/// Python truthiness of a decoded JSON value.
///
/// Used by the legacy Op reader for `started.get(key) or "unknown"`: `None`,
/// `False`, zero, empty text and empty collections are falsy. `NaN` and the
/// infinities are truthy, exactly like Python.
pub fn is_falsy(value: &PythonJsonValue) -> bool {
    match value {
        PythonJsonValue::Null => true,
        PythonJsonValue::Bool(flag) => !flag,
        PythonJsonValue::Number(token) => number_token_is_falsy(token),
        PythonJsonValue::Integer(digits) => digits == "0",
        PythonJsonValue::Float(number) => *number == 0.0,
        PythonJsonValue::String(text) => text.is_empty(),
        PythonJsonValue::Array(items) => items.is_empty(),
        PythonJsonValue::Object(entries) => entries.is_empty(),
        PythonJsonValue::ExactObject(object) => object.is_empty(),
    }
}

/// Truthiness of an explicit pre-formatted number token.
fn number_token_is_falsy(token: &str) -> bool {
    if token.bytes().any(|byte| matches!(byte, b'.' | b'e' | b'E')) {
        token
            .parse::<f64>()
            .map(|value| value == 0.0)
            .unwrap_or(false)
    } else {
        token
            .strip_prefix('-')
            .unwrap_or(token)
            .bytes()
            .all(|b| b == b'0')
    }
}

/// Python `repr()` of a decoded JSON value (`None`, `True`, `[..]`, `{..}`).
///
/// This is the single value-repr walker; a top-level string caller that needs
/// `str()` (verbatim code points, not `repr`) branches before calling it, as the
/// legacy Op adapter does.
pub fn python_repr_value(value: &PythonJsonValue) -> String {
    match value {
        PythonJsonValue::Null => "None".to_owned(),
        PythonJsonValue::Bool(true) => "True".to_owned(),
        PythonJsonValue::Bool(false) => "False".to_owned(),
        PythonJsonValue::Number(token) => token.clone(),
        PythonJsonValue::Integer(digits) => digits.clone(),
        PythonJsonValue::Float(number) => super::python_float_repr(*number),
        PythonJsonValue::String(text) => text.repr(),
        PythonJsonValue::Array(items) => {
            let rendered: Vec<String> = items.iter().map(python_repr_value).collect();
            format!("[{}]", rendered.join(", "))
        }
        PythonJsonValue::Object(entries) => {
            let rendered: Vec<String> = entries
                .iter()
                .map(|(key, value)| {
                    format!("{}: {}", crate::python_repr(key), python_repr_value(value))
                })
                .collect();
            format!("{{{}}}", rendered.join(", "))
        }
        PythonJsonValue::ExactObject(object) => {
            let rendered: Vec<String> = object
                .entries()
                .iter()
                .map(|(key, value)| format!("{}: {}", key.repr(), python_repr_value(value)))
                .collect();
            format!("{{{}}}", rendered.join(", "))
        }
    }
}
