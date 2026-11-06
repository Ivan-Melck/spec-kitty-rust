//! CPython base-10 integer text conversion over the one exact string carrier.
//!
//! This is not JSON number syntax or YAML radix/constructor policy. Callers keep
//! their own admission/catch phases, but never copy Python's decimal-text rules.
use crate::{
    PythonString,
    python_text::{decimal_value, is_whitespace},
};

/// CPython's "no decimal conversion ceiling" value (`PYTHONINTMAXSTRDIGITS=0`).
///
/// The same `0` disables the ceiling in [`parse_text`] and [`format_decimal`], so a
/// caller cannot spell "unlimited" two different ways.
pub const UNLIMITED_INTEGER_DIGITS: usize = 0;

/// A Python ValueError from `int(str)`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IntTextError {
    /// The original argument failed base-10 syntax.
    InvalidLiteral(PythonString),
    /// CPython's configured decimal string-conversion ceiling was exceeded.
    DigitLimit { limit: usize, digits: usize },
}

impl std::fmt::Display for IntTextError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidLiteral(argument) => {
                // CPython formats this as %.200R: precision applies to repr,
                // not to the original authored argument before escaping.
                let repr: String = argument.repr().chars().take(200).collect();
                write!(out, "invalid literal for int() with base 10: {repr}")
            }
            Self::DigitLimit { limit, digits } => {
                write!(out, "{}", parse_digit_limit_message(*limit, *digits))
            }
        }
    }
}
impl std::error::Error for IntTextError {}

/// A Python ValueError from `str(int)` / `repr(int)` decimal formatting.
///
/// This is the **integer-to-text** direction, and the reference's message really
/// differs from the text-to-integer one: measured on the pinned CPython 3.11.15,
/// `str(2**14285)` reports no observed digit count, while `int("1" + "0" * 4300)`
/// appends `: value has 4301 digits`. The two messages are rendered by
/// [`parse_digit_limit_message`] and [`decimal_digit_limit_message`] in this one
/// module so neither phase can borrow the other's wording.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntDecimalError {
    /// CPython's configured decimal string-conversion ceiling was exceeded.
    ///
    /// `digits` is the observed magnitude width. CPython 3.11.15 does not print it
    /// for this direction; it is retained as structured evidence, never as text.
    DigitLimit { limit: usize, digits: usize },
}

impl std::fmt::Display for IntDecimalError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DigitLimit { limit, .. } => decimal_digit_limit_message(*limit).fmt(out),
        }
    }
}
impl std::error::Error for IntDecimalError {}

/// `str(int)` for canonical decimal text, under the interpreter's ceiling.
///
/// `digits` is the canonical decimal spelling the object model carries (an
/// optional `-` and no leading zeros); the sign is not a decimal digit. A
/// `digit_limit` of [`UNLIMITED_INTEGER_DIGITS`] disables the ceiling. The check
/// matches the pinned interpreter's boundary exactly: a magnitude of `limit`
/// digits renders and `limit + 1` raises.
///
/// # Errors
///
/// [`IntDecimalError::DigitLimit`] when the spelling would exceed the ceiling.
pub fn format_decimal(digits: &str, digit_limit: usize) -> Result<String, IntDecimalError> {
    let magnitude = digits.strip_prefix('-').unwrap_or(digits);
    let count = magnitude.len();
    if digit_limit != UNLIMITED_INTEGER_DIGITS && count > digit_limit {
        return Err(IntDecimalError::DigitLimit {
            limit: digit_limit,
            digits: count,
        });
    }
    Ok(digits.to_owned())
}

/// The one text-to-integer ceiling message (`int(str)`).
fn parse_digit_limit_message(limit: usize, digits: usize) -> String {
    format!(
        "Exceeds the limit ({limit} digits) for integer string conversion: value has {digits} digits; use sys.set_int_max_str_digits() to increase the limit"
    )
}

/// The one integer-to-text ceiling message (`str(int)`); no observed count.
fn decimal_digit_limit_message(limit: usize) -> String {
    format!(
        "Exceeds the limit ({limit} digits) for integer string conversion; use sys.set_int_max_str_digits() to increase the limit"
    )
}

/// `int(text, 10)` as canonical ASCII decimal digits, without a width bound.
///
/// Unicode Nd, surrounding integer whitespace, sign and single underscores
/// follow CPython. ASCII U+001C..U+001F are str.isspace but NOT int whitespace.
/// `digit_limit == 0` disables the decimal conversion ceiling.
///
/// # Errors
///
/// [`IntTextError`] preserves the original argument and the ValueError message.
pub fn parse_text(text: &PythonString, digit_limit: usize) -> Result<String, IntTextError> {
    let points = text.codepoints();
    let invalid = || IntTextError::InvalidLiteral(text.clone());
    let start = points
        .iter()
        .position(|code| !integer_space(*code))
        .unwrap_or(points.len());
    let end = points
        .iter()
        .rposition(|code| !integer_space(*code))
        .map_or(start, |at| at + 1);
    let (negative, body) = match &points[start..end] {
        [0x2d, rest @ ..] => (true, rest),
        [0x2b, rest @ ..] => (false, rest),
        rest => (false, rest),
    };
    let mut digits = String::with_capacity(body.len());
    let mut previous_digit = false;
    let mut invalid_suffix = false;
    for code in body {
        if *code == 0x5f {
            if !previous_digit {
                return Err(invalid());
            }
            previous_digit = false;
        } else if let Some(digit) = decimal_value(*code) {
            digits.push(char::from(b'0' + digit));
            previous_digit = true;
        } else {
            // CPython checks the scanned digit prefix's ceiling before its
            // trailing-data check. Bad underscore placement is earlier.
            invalid_suffix = true;
            break;
        }
    }
    if !previous_digit {
        return Err(invalid());
    }
    if digit_limit != 0 && digits.len() > digit_limit {
        return Err(IntTextError::DigitLimit {
            limit: digit_limit,
            digits: digits.len(),
        });
    }
    if invalid_suffix {
        return Err(invalid());
    }
    let magnitude = digits.trim_start_matches('0');
    Ok(if magnitude.is_empty() {
        "0".to_owned()
    } else if negative {
        format!("-{magnitude}")
    } else {
        magnitude.to_owned()
    })
}

fn integer_space(code: u32) -> bool {
    is_whitespace(code) && !(0x1c..=0x1f).contains(&code)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn integer_formatting_uses_its_own_ceiling_message() {
        // The measured pinned-interpreter boundary: `limit` digits render and
        // `limit + 1` raise. `str(-10**n)` counts magnitude digits only.
        assert_eq!(format_decimal(&"1".repeat(4300), 4300).unwrap().len(), 4300);
        let error = format_decimal(&"1".repeat(4301), 4300).unwrap_err();
        assert_eq!(
            error,
            IntDecimalError::DigitLimit {
                limit: 4300,
                digits: 4301
            }
        );
        assert_eq!(
            error.to_string(),
            "Exceeds the limit (4300 digits) for integer string conversion; use sys.set_int_max_str_digits() to increase the limit"
        );
        assert_eq!(
            format_decimal(&format!("-{}", "1".repeat(4301)), 0)
                .unwrap()
                .len(),
            4302
        );
        assert!(matches!(
            format_decimal(&format!("-{}", "1".repeat(641)), 640),
            Err(IntDecimalError::DigitLimit {
                limit: 640,
                digits: 641
            })
        ));
        assert_eq!(format_decimal("0", 640).unwrap(), "0");
        // The int(str) direction keeps its distinct message bytes.
        assert_eq!(
            parse_text(&PythonString::from_scalars(&"1".repeat(4301)), 4300)
                .unwrap_err()
                .to_string(),
            "Exceeds the limit (4300 digits) for integer string conversion: value has 4301 digits; use sys.set_int_max_str_digits() to increase the limit"
        );
    }

    #[test]
    fn ceiling_checks_the_valid_prefix_before_a_bad_suffix_but_not_bad_underscores() {
        let prefix = "1".repeat(4301);
        for suffix in ["x", " x", "\0", "é"] {
            assert!(matches!(
                parse_text(
                    &PythonString::from_scalars(&format!("{prefix}{suffix}")),
                    4300
                ),
                Err(IntTextError::DigitLimit {
                    limit: 4300,
                    digits: 4301
                })
            ));
        }
        for value in [
            format!("{prefix}_"),
            format!("{prefix}__2"),
            format!("{prefix}_x"),
            format!("x{prefix}"),
        ] {
            assert!(matches!(
                parse_text(&PythonString::from_scalars(&value), 4300),
                Err(IntTextError::InvalidLiteral(_))
            ));
        }
    }

    #[test]
    fn invalid_literal_precision_applies_to_the_exact_repr() {
        for (value, repr_prefix) in [
            (
                PythonString::from_scalars(&"a".repeat(300)),
                format!("'{}", "a".repeat(199)),
            ),
            (
                PythonString::from_units(vec![0xd800; 300]).unwrap(),
                format!("'{}\\", "\\ud800".repeat(33)),
            ),
        ] {
            assert_eq!(
                parse_text(&value, 4300).unwrap_err().to_string(),
                format!("invalid literal for int() with base 10: {repr_prefix}")
            );
        }
    }

    #[test]
    fn source_integer_text_policy_is_distinct_from_strip_and_json() {
        let parse = |text: &str, limit| parse_text(&PythonString::from_scalars(text), limit);
        assert_eq!(parse("\u{a0}-١_２\u{a0}", 4300).unwrap(), "-12");
        assert_eq!(parse("+000", 4300).unwrap(), "0");
        for text in ["\u{1c}12\u{1f}", "＋１２", "1__0", "_1", "1_", "", "0x10"] {
            assert!(parse(text, 4300).is_err(), "{text:?}");
        }
        assert!(matches!(
            parse(&"0".repeat(4301), 4300),
            Err(IntTextError::DigitLimit { .. })
        ));
        assert_eq!(parse(&"0".repeat(4301), 0).unwrap(), "0");
    }
}
