//! Python equality in the default `json.loads` value domain.
use super::{PythonJsonDict, PythonJsonValue};
use crate::PythonString;

/// Compare decoded JSON values with Python's numeric/container semantics.
///
/// Bool is an int subtype; arbitrary integers are never rounded through f64.
/// Dict order is irrelevant and exact string keys retain every code point.
/// Default `json.loads` shares its NaN literal singleton: scalar NaN equality is
/// false, but containers apply Python's identity-or-equality shortcut to it.
/// This models that JSON domain, not identities of arbitrary independently
/// constructed Python float objects. Existing Rust `PartialEq` is unchanged;
/// caller-dependent Python recursion/stack behavior is not claimed here.
pub fn python_json_equal(left: &PythonJsonValue, right: &PythonJsonValue) -> bool {
    equal(left, right, false)
}

/// Borrowed dict view of the same equality rule (key order is irrelevant).
pub fn python_json_dict_equal(left: &PythonJsonDict, right: &PythonJsonDict) -> bool {
    left.len() == right.len()
        && left.iter().all(|(key, value)| {
            right
                .get_codepoints(key.codepoints())
                .is_some_and(|other| equal(value, other, true))
        })
}

/// Source tuple/list comparison of `dict.get(key, missing)` projections.
///
/// `None` is an absent member, distinct from `Some(Null)`. Present members use
/// the same container identity-or-equality rule; no serializable sentinel or
/// alternate numeric policy is introduced.
pub fn python_json_optional_sequence_equal(
    left: &[Option<PythonJsonValue>],
    right: &[Option<PythonJsonValue>],
) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| match (left, right) {
                (None, None) => true,
                (Some(left), Some(right)) => equal(left, right, true),
                _ => false,
            })
}

/// Python `==` for a dict key / container member in this JSON domain.
///
/// Identical to the private container rule: numeric-tower equality (never a
/// lossy float round-trip for integers), exact string code points, order-free
/// dict comparison, and the shared-`NaN`-singleton identity shortcut.
/// `json.loads` produces one `NaN` object per document, so the shortcut models
/// the reference's value domain rather than arbitrary float identities.
pub(super) fn key_equal(left: &PythonJsonValue, right: &PythonJsonValue) -> bool {
    equal(left, right, true)
}

fn equal(left: &PythonJsonValue, right: &PythonJsonValue, contained: bool) -> bool {
    if let (Some(left), Some(right)) = (numeric(left), numeric(right)) {
        return match (left, right) {
            (Numeric::Integer(a), Numeric::Integer(b)) => integer_equal(a, b),
            (Numeric::Float(a), Numeric::Float(b)) => {
                a == b || (contained && a.is_nan() && b.is_nan())
            }
            (Numeric::Integer(a), Numeric::Float(b)) | (Numeric::Float(b), Numeric::Integer(a)) => {
                b.is_finite() && b.fract() == 0.0 && integer_equal(a, &format!("{b:.0}"))
            }
        };
    }
    match (left, right) {
        (PythonJsonValue::ExactObject(a), PythonJsonValue::ExactObject(b)) => {
            python_json_dict_equal(a, b)
        }
        (PythonJsonValue::Null, PythonJsonValue::Null) => true,
        (PythonJsonValue::String(a), PythonJsonValue::String(b)) => a == b,
        (PythonJsonValue::Array(a), PythonJsonValue::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| equal(a, b, true))
        }
        (a, b) => match (object_entries(a), object_entries(b)) {
            (Some(a), Some(b)) => {
                a.len() == b.len()
                    && a.iter().all(|(key, value)| {
                        b.iter()
                            .find(|(other, _)| other == key)
                            .is_some_and(|(_, other)| equal(value, other, true))
                    })
            }
            _ => false,
        },
    }
}

enum Numeric<'a> {
    Integer(&'a str),
    Float(f64),
}
fn numeric(value: &PythonJsonValue) -> Option<Numeric<'_>> {
    match value {
        PythonJsonValue::Bool(flag) => Some(Numeric::Integer(if *flag { "1" } else { "0" })),
        PythonJsonValue::Integer(value) => Some(Numeric::Integer(value)),
        PythonJsonValue::Float(value) => Some(Numeric::Float(*value)),
        // Number is the existing producer convenience for a valid raw token.
        PythonJsonValue::Number(token) => {
            if token.bytes().any(|byte| matches!(byte, b'.' | b'e' | b'E'))
                || matches!(token.as_str(), "NaN" | "Infinity" | "-Infinity")
            {
                token.parse::<f64>().ok().map(Numeric::Float)
            } else {
                Some(Numeric::Integer(token))
            }
        }
        _ => None,
    }
}
fn integer_equal(left: &str, right: &str) -> bool {
    fn parts(token: &str) -> (bool, &str) {
        let negative = token.starts_with('-');
        let digits = token
            .strip_prefix('-')
            .unwrap_or(token)
            .trim_start_matches('0');
        (negative && !digits.is_empty(), digits)
    }
    parts(left) == parts(right)
}
fn object_entries(value: &PythonJsonValue) -> Option<Vec<(PythonString, &PythonJsonValue)>> {
    match value {
        PythonJsonValue::ExactObject(object) => Some(
            object
                .iter()
                .map(|(key, value)| (key.clone(), value))
                .collect(),
        ),
        PythonJsonValue::Object(entries) => {
            let mut result: Vec<(PythonString, &PythonJsonValue)> =
                Vec::with_capacity(entries.len());
            for (key, value) in entries {
                let key = PythonString::from_scalars(key);
                if let Some((_, held)) = result.iter_mut().find(|(held, _)| held == &key) {
                    *held = value;
                } else {
                    result.push((key, value));
                }
            }
            Some(result)
        }
        _ => None,
    }
}
