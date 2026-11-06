//! Python comparison and hashability in the default `json.loads` value domain.
//!
//! The status diary's fold orders and keys on *raw authored* values: an `at` or
//! `event_id` may be an exact string, an integer, a float (including `NaN` and
//! the infinities), a boolean, `null`, or even a list. Python compares those
//! values with its own rules — numeric across `bool`/`int`/`float` with exact
//! arbitrary-precision integers, code-point order for `str`, lexicographic order
//! for lists, and `TypeError` for cross-type rankings — and hashes only the
//! hashable kinds. A Rust `String` key would silently stringify all of that.
//!
//! This module is the ONE lower authority for those rules. It reuses the
//! existing numeric-equality and Python-type decisions in [`super::equality`] and
//! [`super::python_repr_value`]; it introduces no second value tree, parser,
//! encoder, numeric-equality table or hash table.
//!
//! Deliberate scope note: the stable sort is the pinned reference's list-sort
//! engine (natural runs, binary insertion, powersort merges with galloping), which
//! lives in [`super::sort`]. It applies Python's tuple `<` predicate, so a
//! non-transitive author such as `NaN` follows the reference's comparison and
//! error sequence instead of a total-order substitution.

use std::cmp::Ordering;
use std::fmt;

use super::{PythonJsonValue, python_repr_value};
use crate::PythonString;

/// A Python-level comparison or hashability refusal (`TypeError` text).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PythonJsonCompareError {
    message: String,
}

impl PythonJsonCompareError {
    fn type_error(message: String) -> Self {
        Self { message }
    }

    /// The CPython message text.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for PythonJsonCompareError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for PythonJsonCompareError {}

/// Python `type(value).__name__` for the JSON value domain.
pub fn python_json_type_name(value: &PythonJsonValue) -> &'static str {
    match value {
        PythonJsonValue::Null => "NoneType",
        PythonJsonValue::Bool(_) => "bool",
        PythonJsonValue::Number(token) => {
            if number_is_float_token(token) {
                "float"
            } else {
                "int"
            }
        }
        PythonJsonValue::Integer(_) => "int",
        PythonJsonValue::Float(_) => "float",
        PythonJsonValue::String(_) => "str",
        PythonJsonValue::Array(_) => "list",
        PythonJsonValue::Object(_) | PythonJsonValue::ExactObject(_) => "dict",
    }
}

fn number_is_float_token(token: &str) -> bool {
    token.bytes().any(|byte| matches!(byte, b'.' | b'e' | b'E'))
        || matches!(token, "NaN" | "Infinity" | "-Infinity")
}

/// Python truthiness (`bool(value)`) of a decoded JSON value.
pub fn python_json_truthy(value: &PythonJsonValue) -> bool {
    !super::is_falsy(value)
}

/// Python `str(value)` as exact code points.
///
/// A `str` is itself; every other value is its `repr` (which is what Python's
/// `str()` calls for builtins). Rendering reuses the one value-repr walker.
pub fn python_json_str(value: &PythonJsonValue) -> PythonString {
    match value {
        PythonJsonValue::String(text) => text.clone(),
        other => PythonString::from_scalars(&python_repr_value(other)),
    }
}

/// Python `hash()` eligibility: `list` and `dict` are unhashable.
pub fn python_json_hashable(value: &PythonJsonValue) -> Result<(), PythonJsonCompareError> {
    match value {
        PythonJsonValue::Array(_) => Err(PythonJsonCompareError::type_error(
            "unhashable type: 'list'".to_owned(),
        )),
        PythonJsonValue::Object(_) | PythonJsonValue::ExactObject(_) => Err(
            PythonJsonCompareError::type_error("unhashable type: 'dict'".to_owned()),
        ),
        _ => Ok(()),
    }
}

/// Hashability of a Python tuple: every member must be hashable.
pub fn python_json_tuple_hashable(
    values: &[PythonJsonValue],
) -> Result<(), PythonJsonCompareError> {
    values.iter().try_for_each(python_json_hashable)
}

/// Python `==` in the diary's JSON domain, including the shared `NaN` singleton.
///
/// `json.loads` maps every `NaN` literal to one float object, so two `NaN`
/// values carrying that provenance compare equal through CPython's identity
/// shortcut in containers and as dict keys (`nan == nan` for the *same*
/// object). This is the same rule [`super::python_json_dict_equal`] already
/// applies; it is borrowed here rather than restated.
pub fn python_json_key_equal(left: &PythonJsonValue, right: &PythonJsonValue) -> bool {
    super::equality::key_equal(left, right)
}

/// Python `a < b`.
pub fn python_json_lt(
    left: &PythonJsonValue,
    right: &PythonJsonValue,
) -> Result<bool, PythonJsonCompareError> {
    if let (Some(left), Some(right)) = (numeric(left), numeric(right)) {
        return Ok(match compare_numeric(left, right)? {
            Some(Ordering::Less) => true,
            Some(_) => false,
            None => false, // NaN: every `<` is false
        });
    }
    match (left, right) {
        (PythonJsonValue::String(left), PythonJsonValue::String(right)) => {
            Ok(left.codepoints().cmp(right.codepoints()) == Ordering::Less)
        }
        (PythonJsonValue::Array(left), PythonJsonValue::Array(right)) => {
            for (left, right) in left.iter().zip(right) {
                if python_json_key_equal(left, right) {
                    continue;
                }
                return python_json_lt(left, right);
            }
            Ok(left.len() < right.len())
        }
        _ => Err(incomparable(left, right)),
    }
}

/// Python `a <= b`: direct numerics do not take container identity shortcuts.
pub fn python_json_le(
    left: &PythonJsonValue,
    right: &PythonJsonValue,
) -> Result<bool, PythonJsonCompareError> {
    if let (Some(left), Some(right)) = (numeric(left), numeric(right)) {
        return Ok(matches!(
            compare_numeric(left, right)?,
            Some(Ordering::Less | Ordering::Equal)
        ));
    }
    Ok(python_json_lt(left, right)? || python_json_key_equal(left, right))
}

/// Lexicographic Python tuple comparison `left < right`.
///
/// Tuple comparison finds the first index whose members are not `==`, then
/// compares those two members with `<` — exactly CPython's rule.
pub fn python_json_tuple_lt(
    left: &[PythonJsonValue],
    right: &[PythonJsonValue],
) -> Result<bool, PythonJsonCompareError> {
    for (left, right) in left.iter().zip(right) {
        if python_json_key_equal(left, right) {
            continue;
        }
        return python_json_lt(left, right);
    }
    Ok(left.len() < right.len())
}

/// A stable sort that applies Python's tuple `<` predicate.
///
/// The sort key is computed once per item. Only `<` decides order, so no total
/// order is assumed and an incomparable pair is a typed `TypeError` (Python's
/// behaviour), never a panic. The engine is the pinned reference's list sort
/// (natural runs, binary insertion, powersort merges and galloping); equal keys
/// and `NaN`-broken pairs follow the reference's own comparison and stable
/// output sequence. On refusal the caller's items are untouched and nothing is
/// lost, matching `sorted()`'s input contract.
pub fn python_json_stable_sort_by_key<T>(
    items: &mut Vec<T>,
    key: impl Fn(&T) -> Vec<PythonJsonValue>,
) -> Result<(), PythonJsonCompareError> {
    super::sort::python_json_stable_sort_by_key(items, key)
}

fn incomparable(left: &PythonJsonValue, right: &PythonJsonValue) -> PythonJsonCompareError {
    PythonJsonCompareError::type_error(format!(
        "'<' not supported between instances of '{}' and '{}'",
        python_json_type_name(left),
        python_json_type_name(right)
    ))
}

enum Numeric<'a> {
    Integer(&'a str),
    Float(f64),
}

fn numeric(value: &PythonJsonValue) -> Option<Numeric<'_>> {
    match value {
        PythonJsonValue::Bool(flag) => Some(Numeric::Integer(if *flag { "1" } else { "0" })),
        PythonJsonValue::Integer(digits) => Some(Numeric::Integer(digits)),
        PythonJsonValue::Float(value) => Some(Numeric::Float(*value)),
        PythonJsonValue::Number(token) => {
            if number_is_float_token(token) {
                token.parse::<f64>().ok().map(Numeric::Float)
            } else {
                Some(Numeric::Integer(token))
            }
        }
        _ => None,
    }
}

fn compare_numeric(
    left: Numeric<'_>,
    right: Numeric<'_>,
) -> Result<Option<Ordering>, PythonJsonCompareError> {
    Ok(match (left, right) {
        (Numeric::Integer(left), Numeric::Integer(right)) => Some(compare_decimal(left, right)),
        (Numeric::Float(left), Numeric::Float(right)) => left.partial_cmp(&right),
        (Numeric::Integer(left), Numeric::Float(right)) => compare_integer_float(left, right),
        (Numeric::Float(left), Numeric::Integer(right)) => {
            compare_integer_float(right, left).map(Ordering::reverse)
        }
    })
}

/// Python `int < float` / `int == float`: exact, never routed through a lossy
/// `i64`/`f64` conversion of the integer.
fn compare_integer_float(integer: &str, value: f64) -> Option<Ordering> {
    if value.is_nan() {
        return None;
    }
    let (negative, magnitude) = split_sign(integer);
    if value.is_infinite() {
        return Some(if value.is_sign_positive() {
            Ordering::Less
        } else {
            Ordering::Greater
        });
    }
    if value == 0.0 {
        return Some(if magnitude == "0" {
            Ordering::Equal
        } else if negative {
            Ordering::Less
        } else {
            Ordering::Greater
        });
    }
    let float_negative = value.is_sign_negative();
    let float_magnitude = value.abs();
    let magnitude_order = compare_decimal_magnitude_to_float(magnitude, float_magnitude);
    Some(match (negative, float_negative) {
        (true, true) => magnitude_order.reverse(),
        (false, false) => magnitude_order,
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
    })
}

fn split_sign(digits: &str) -> (bool, &str) {
    match digits.strip_prefix('-') {
        Some(rest) if rest.bytes().any(|byte| byte != b'0') => (true, rest),
        Some(_) => (false, "0"),
        None => (false, digits),
    }
}

/// Compare a non-negative decimal integer with a positive finite `f64` exactly.
fn compare_decimal_magnitude_to_float(magnitude: &str, value: f64) -> Ordering {
    let (mantissa, exponent) = decompose_float(value);
    if exponent >= 0 {
        let scaled = multiply_decimal_by_power_of_two(mantissa, exponent as u32);
        compare_decimal(magnitude, &scaled)
    } else {
        let scaled = multiply_decimal_by_power_of_two(
            strip_leading_zeros(magnitude).to_owned(),
            (-exponent) as u32,
        );
        compare_decimal(&scaled, &mantissa)
    }
}

fn decompose_float(value: f64) -> (String, i32) {
    let bits = value.to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i32;
    let fraction = bits & ((1_u64 << 52) - 1);
    if exponent == 0 {
        (fraction.to_string(), -1074)
    } else {
        ((fraction | (1_u64 << 52)).to_string(), exponent - 1075)
    }
}

fn strip_leading_zeros(digits: &str) -> &str {
    let trimmed = digits.trim_start_matches('0');
    if trimmed.is_empty() { "0" } else { trimmed }
}

fn multiply_decimal_by_power_of_two(mut digits: String, times: u32) -> String {
    for _ in 0..times {
        digits = multiply_decimal_by_two(&digits);
    }
    digits
}

fn multiply_decimal_by_two(digits: &str) -> String {
    let mut output: Vec<u8> = Vec::with_capacity(digits.len() + 1);
    let mut carry = 0_u8;
    for byte in digits.bytes().rev() {
        let digit = byte - b'0';
        let doubled = digit * 2 + carry;
        output.push(b'0' + (doubled % 10));
        carry = doubled / 10;
    }
    if carry > 0 {
        output.push(b'0' + carry);
    }
    output.reverse();
    String::from_utf8(output).expect("decimal digits stay ASCII")
}

fn compare_decimal(left: &str, right: &str) -> Ordering {
    let (left_negative, left_magnitude) = split_sign(left);
    let (right_negative, right_magnitude) = split_sign(right);
    match (left_negative, right_negative) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => compare_decimal_magnitudes(left_magnitude, right_magnitude),
        (true, true) => compare_decimal_magnitudes(right_magnitude, left_magnitude),
    }
}

fn compare_decimal_magnitudes(left: &str, right: &str) -> Ordering {
    let left = strip_leading_zeros(left);
    let right = strip_leading_zeros(right);
    left.len()
        .cmp(&right.len())
        .then_with(|| left.as_bytes().cmp(right.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn int(digits: &str) -> PythonJsonValue {
        PythonJsonValue::Integer(digits.to_owned())
    }

    fn text(value: &str) -> PythonJsonValue {
        PythonJsonValue::scalar(value)
    }

    #[test]
    fn integer_float_comparison_is_exact() {
        assert_eq!(
            compare_integer_float("9007199254740993", 9007199254740992.0),
            Some(Ordering::Greater)
        );
        assert_eq!(
            compare_integer_float("9007199254740992", 9007199254740992.0),
            Some(Ordering::Equal)
        );
        // 1e30's exact binary value is 1000000000000000019884624838656.
        assert_eq!(
            compare_integer_float("1000000000000000000000000000000", 1e30),
            Some(Ordering::Less)
        );
        assert_eq!(compare_integer_float("-1", 0.0), Some(Ordering::Less));
        assert_eq!(
            compare_integer_float("1", f64::INFINITY),
            Some(Ordering::Less)
        );
        assert_eq!(
            compare_integer_float("1", f64::NEG_INFINITY),
            Some(Ordering::Greater)
        );
    }

    #[test]
    fn mixed_scalars_refuse_and_strings_sort_by_code_point() {
        let error = python_json_lt(&int("5"), &text("5")).expect_err("mixed ranking");
        assert_eq!(
            error.message(),
            "'<' not supported between instances of 'int' and 'str'"
        );
        assert!(python_json_lt(&text("10"), &text("2")).expect("strings order"));
    }

    #[test]
    fn unhashable_kinds_name_the_python_type() {
        assert_eq!(
            python_json_hashable(&PythonJsonValue::Array(Vec::new()))
                .expect_err("list unhashable")
                .message(),
            "unhashable type: 'list'"
        );
    }

    #[test]
    fn stable_sort_keeps_nan_pairs_in_input_order() {
        let mut items = vec![PythonJsonValue::Float(f64::NAN), int("2")];
        python_json_stable_sort_by_key(&mut items, |value| vec![value.clone()])
            .expect("NaN does not raise in a sort");
        assert!(matches!(items[0], PythonJsonValue::Float(value) if value.is_nan()));
        assert_eq!(items[1], int("2"));
    }
}
