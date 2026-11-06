//! CPython base-10 integer text conversion over the one exact string carrier.
//!
//! This is not JSON number syntax or YAML radix/constructor policy. Callers keep
//! their own admission/catch phases, but never copy Python's decimal-text rules.
use crate::{
    PythonString,
    python_text::{decimal_value, is_whitespace},
};

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
            Self::DigitLimit { limit, digits } => write!(
                out,
                "Exceeds the limit ({limit} digits) for integer string conversion: value has {digits} digits; use sys.set_int_max_str_digits() to increase the limit"
            ),
        }
    }
}
impl std::error::Error for IntTextError {}

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
