//! Python-compatible JSON line rendering for Op records.
//!
//! The reference writes every Op line with `json.dumps(model_dump(
//! exclude_none=True))`, i.e. the library defaults: insertion (declared field)
//! order, `", "` between items, `": "` after a key, and `ensure_ascii=True`.
//! `serde_json`'s compact UTF-8 output is a different byte shape, so Op lines
//! are rendered here instead of through `serde_json::to_string`.
//!
//! Only two primitives are needed: a string literal encoder and an ordered
//! object builder. Values are always strings or booleans, so numbers, arrays
//! and nested objects never appear in a well-formed Op line.
//!
//! Escaping reproduces CPython's `py_encode_basestring_ascii`:
//! `"` and `\` are escaped, `\b \f \n \r \t` use their short forms, every
//! other code point below `U+0020` and every code point at or above `U+007F`
//! is written as `\uXXXX` (lowercase hex), and a supplementary-plane code
//! point becomes a UTF-16 surrogate pair (`\ud83d\ude00`).

use crate::python_unicode;

/// An ordered JSON object rendered the way Python's `json.dumps` renders a dict.
#[derive(Clone, Debug, Default)]
pub struct PythonJsonObject {
    buffer: String,
    empty: bool,
}

impl PythonJsonObject {
    /// Start an empty object.
    pub fn new() -> Self {
        Self {
            buffer: String::from("{"),
            empty: true,
        }
    }

    /// Append a string member.
    pub fn string(&mut self, key: &str, value: &str) {
        self.key(key);
        push_string(&mut self.buffer, value);
    }

    /// Append a boolean member.
    pub fn boolean(&mut self, key: &str, value: bool) {
        self.key(key);
        self.buffer.push_str(if value { "true" } else { "false" });
    }

    /// Append a string member only when the value is present (`exclude_none`).
    pub fn optional_string(&mut self, key: &str, value: Option<&str>) {
        if let Some(value) = value {
            self.string(key, value);
        }
    }

    /// Finish the object and return its JSON text (no trailing newline).
    pub fn finish(mut self) -> String {
        self.buffer.push('}');
        self.buffer
    }

    fn key(&mut self, key: &str) {
        if !self.empty {
            self.buffer.push_str(", ");
        }
        self.empty = false;
        push_string(&mut self.buffer, key);
        self.buffer.push_str(": ");
    }
}

fn push_string(target: &mut String, value: &str) {
    target.push('"');
    for character in value.chars() {
        match character {
            '"' => target.push_str("\\\""),
            '\\' => target.push_str("\\\\"),
            '\u{08}' => target.push_str("\\b"),
            '\u{0c}' => target.push_str("\\f"),
            '\n' => target.push_str("\\n"),
            '\r' => target.push_str("\\r"),
            '\t' => target.push_str("\\t"),
            character if (character as u32) < 0x20 || (character as u32) >= 0x7f => {
                push_ascii_escape(target, character);
            }
            character => target.push(character),
        }
    }
    target.push('"');
}

fn push_ascii_escape(target: &mut String, character: char) {
    let code = character as u32;
    if code <= 0xffff {
        target.push_str(&format!("\\u{code:04x}"));
        return;
    }
    let adjusted = code - 0x1_0000;
    let high = 0xd800 + (adjusted >> 10);
    let low = 0xdc00 + (adjusted & 0x3ff);
    target.push_str(&format!("\\u{high:04x}\\u{low:04x}"));
}

/// CPython `repr()` for a string, used by operator-facing refusal messages.
///
/// This is the `&str` wrapper over the one canonical formatter authority,
/// [`python_repr_codepoints`]. A `&str` can only hold Unicode scalars, so the
/// wrapper is used for text that already decoded strictly; the legacy Op reader
/// passes Python code points (including lone surrogate units) to the same
/// primitive.
pub fn python_repr(value: &str) -> String {
    let codepoints: Vec<u32> = value.chars().map(u32::from).collect();
    python_repr_codepoints(&codepoints)
}

/// CPython `repr()` for a sequence of Python string code points.
///
/// This is the single `repr` formatting authority in the crate. CPython keeps
/// printable characters verbatim, writes `\t \n \r` in their short forms,
/// escapes the chosen quote and `\\`, and writes every other non-printable code
/// point as `\xNN` below `U+0100`, `\uNNNN` below `U+10000`, otherwise
/// `\UNNNNNNNN` (lowercase hex, like `unicode_repr`).
///
/// Printability is the verified [`crate::python_unicode`] `str.isprintable()`
/// table, not Rust's `char::is_control()` (which is `Cc` only). A value
/// containing a single quote but no double quote is double-quoted, exactly like
/// CPython. Lone surrogate code units are non-printable, so they always escape
/// and the rendered text is always valid UTF-8.
pub(crate) fn python_repr_codepoints(codepoints: &[u32]) -> String {
    let single_quote = u32::from(b'\'');
    let quote = if codepoints.contains(&single_quote) && !codepoints.contains(&u32::from(b'"')) {
        u32::from(b'"')
    } else {
        single_quote
    };
    let mut out = String::with_capacity(codepoints.len() + 2);
    out.push(char::from_u32(quote).expect("the repr quote is ASCII"));
    for &code in codepoints {
        match code {
            0x5c => out.push_str("\\\\"),
            0x09 => out.push_str("\\t"),
            0x0a => out.push_str("\\n"),
            0x0d => out.push_str("\\r"),
            code if code == quote => {
                out.push('\\');
                out.push(char::from_u32(code).expect("the repr quote is ASCII"));
            }
            code if (0x20..0x7f).contains(&code) => {
                out.push(char::from_u32(code).expect("ASCII printable code point"));
            }
            code => push_repr_escape(&mut out, code),
        }
    }
    out.push(char::from_u32(quote).expect("the repr quote is ASCII"));
    out
}

/// One non-ASCII/non-printable `repr` escape (`\xNN`, `\uNNNN`, `\UNNNNNNNN`).
fn push_repr_escape(target: &mut String, code: u32) {
    if python_unicode::is_printable(code)
        && let Some(character) = char::from_u32(code)
    {
        target.push(character);
        return;
    }
    if code < 0x100 {
        target.push_str(&format!("\\x{code:02x}"));
    } else if code < 0x10000 {
        target.push_str(&format!("\\u{code:04x}"));
    } else {
        target.push_str(&format!("\\U{code:08x}"));
    }
}

/// CPython `repr()` for an arbitrary JSON value, used in parse-refusal messages.
pub fn python_repr_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "None".to_owned(),
        serde_json::Value::Bool(true) => "True".to_owned(),
        serde_json::Value::Bool(false) => "False".to_owned(),
        // `json.loads` decodes a JSON float to a Python `float`, whose repr is
        // CPython's float repr (`1e+20`, `5.0`), not serde's source text.
        serde_json::Value::Number(number) if number.is_f64() => match number.as_f64() {
            Some(float) if float.is_finite() => python_float_repr(float),
            _ => number.to_string(),
        },
        serde_json::Value::Number(number) => {
            let integer = number.to_string();
            // json.loads parses integer -0 as 0; floating -0.0 is handled
            // above and retains its sign. Arbitrary-precision integers remain
            // exact decimal strings, never coerced through f64.
            if integer == "-0" {
                "0".to_owned()
            } else {
                integer
            }
        }
        serde_json::Value::String(text) => python_repr(text),
        serde_json::Value::Array(items) => {
            let rendered: Vec<String> = items.iter().map(python_repr_json).collect();
            format!("[{}]", rendered.join(", "))
        }
        serde_json::Value::Object(entries) => {
            let rendered: Vec<String> = entries
                .iter()
                .map(|(key, value)| format!("{}: {}", python_repr(key), python_repr_json(value)))
                .collect();
            format!("{{{}}}", rendered.join(", "))
        }
    }
}

/// CPython `repr()`/`str()` for a float, including the non-finite values that
/// `json.loads` can produce.
///
/// Python uses shortest round-trip digits and switches to exponent form when
/// the decimal exponent is below `-4` or at least `16` (`1e-05`, `1e+20`),
/// with a two-digit minimum exponent and an always-present `.0` on integral
/// positional values. Rust's `Display` never uses exponents for `f64`, so the
/// threshold is applied here. `nan`/`inf`/`-inf` match CPython's lower-case
/// spellings. Used only in operator-facing message text.
pub(crate) fn python_float_repr(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_owned();
    }
    if value.is_infinite() {
        return if value.is_sign_negative() {
            "-inf".to_owned()
        } else {
            "inf".to_owned()
        };
    }
    if value == 0.0 {
        return if value.is_sign_negative() {
            "-0.0".to_owned()
        } else {
            "0.0".to_owned()
        };
    }
    let exponential = format!("{value:e}");
    let Some((mantissa, exponent)) = exponential.split_once('e') else {
        return exponential;
    };
    let Ok(exponent) = exponent.parse::<i32>() else {
        return exponential;
    };
    if !(-4..16).contains(&exponent) {
        let sign = if exponent < 0 { '-' } else { '+' };
        return format!("{mantissa}e{sign}{:02}", exponent.abs());
    }
    let positional = format!("{value}");
    if positional.contains('.') {
        positional
    } else {
        format!("{positional}.0")
    }
}

/// Render a `Duration` the way Python renders the float `timeout_s`.
pub fn python_seconds(duration: std::time::Duration) -> String {
    python_float_repr(duration.as_secs_f64())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_separators_and_member_order_match_python() {
        let mut object = PythonJsonObject::new();
        object.string("invocation_id", "01KTK5JBD69FQ8XVRFV1J630MJ");
        object.string("profile_id", "p");
        object.string("started_at", "2026-06-10T20:00:00+00:00");
        assert_eq!(
            object.finish(),
            r#"{"invocation_id": "01KTK5JBD69FQ8XVRFV1J630MJ", "profile_id": "p", "started_at": "2026-06-10T20:00:00+00:00"}"#
        );
    }

    #[test]
    fn ascii_escaping_matches_python_for_controls_and_unicode() {
        let mut object = PythonJsonObject::new();
        object.string("k", "héllo ☃ 𝄞 \u{1} \u{7f} \"q\" \\ tab\tnl\nend");
        assert_eq!(
            object.finish(),
            r#"{"k": "h\u00e9llo \u2603 \ud834\udd1e \u0001 \u007f \"q\" \\ tab\tnl\nend"}"#
        );
    }

    #[test]
    fn optional_member_is_omitted_when_absent() {
        let mut object = PythonJsonObject::new();
        object.string("a", "1");
        object.optional_string("b", None);
        object.boolean("c", false);
        assert_eq!(object.finish(), r#"{"a": "1", "c": false}"#);
    }

    #[test]
    fn python_repr_quotes_like_cpython() {
        assert_eq!(python_repr("main"), "'main'");
        assert_eq!(python_repr("a'b"), "\"a'b\"");
        assert_eq!(python_repr("a\"b"), "'a\"b'");
        assert_eq!(python_repr("a'b\"c"), "'a\\'b\"c'");
        assert_eq!(python_repr("back\\slash"), "'back\\\\slash'");
        assert_eq!(python_repr("tab\there"), "'tab\\there'");
        assert_eq!(python_repr("é"), "'é'");
        assert_eq!(python_repr("\u{0}\u{1f}"), "'\\x00\\x1f'");
        assert_eq!(python_repr("\u{7f}"), "'\\x7f'");
    }

    // Expected strings are CPython 3.11.15 `repr()` output measured from the
    // pinned oracle. They cover every non-printable category (`Zs`, `Cf`, `Zl`,
    // `Zp`, `Cs`, `Co`, `Cn`), not only Rust's `Cc`-wide `is_control()`.
    #[test]
    fn python_repr_follows_cpython_isprintable_categories() {
        assert_eq!(python_repr("a\u{a0}b"), "'a\\xa0b'");
        assert_eq!(python_repr("\u{ad}"), "'\\xad'");
        assert_eq!(python_repr("a\u{200b}b"), "'a\\u200bb'");
        assert_eq!(python_repr("\u{2028}"), "'\\u2028'");
        assert_eq!(python_repr("\u{2029}"), "'\\u2029'");
        assert_eq!(python_repr("\u{378}"), "'\\u0378'");
        assert_eq!(python_repr("\u{e000}"), "'\\ue000'");
        assert_eq!(python_repr("\u{feff}"), "'\\ufeff'");
        assert_eq!(python_repr("\u{1d173}"), "'\\U0001d173'");
        assert_eq!(python_repr("\u{e0001}"), "'\\U000e0001'");
        assert_eq!(python_repr("\u{10ffff}"), "'\\U0010ffff'");
        assert_eq!(python_repr("\u{10000}"), "'\u{10000}'");
        assert_eq!(python_repr("\u{1f600}"), "'\u{1f600}'");
        assert_eq!(python_repr(" "), "' '");
    }

    #[test]
    fn codepoint_formatter_is_the_single_authority_including_surrogates() {
        for text in [
            "main",
            "a'b",
            "a\"b",
            "a'b\"c",
            "back\\slash",
            "\t\n\r",
            "é",
            "\u{a0}\u{200b}",
        ] {
            let codepoints: Vec<u32> = text.chars().map(u32::from).collect();
            assert_eq!(
                python_repr(text),
                python_repr_codepoints(&codepoints),
                "{text:?}"
            );
        }
        assert_eq!(python_repr_codepoints(&[0xd800]), "'\\ud800'");
        assert_eq!(python_repr_codepoints(&[0xd800, 0x41]), "'\\ud800A'");
        assert_eq!(python_repr_codepoints(&[0xdc80]), "'\\udc80'");
        assert_eq!(python_repr_codepoints(&[0x1f600]), "'\u{1f600}'");
        assert_eq!(python_repr_codepoints(&[0x27, 0x22]), "'\\'\"'");
    }

    #[test]
    fn integer_negative_zero_is_canonical_but_float_negative_zero_keeps_its_sign() {
        let integer: serde_json::Value = serde_json::from_str("-0").unwrap();
        let float: serde_json::Value = serde_json::from_str("-0.0").unwrap();
        let huge: serde_json::Value =
            serde_json::from_str("123456789012345678901234567890").unwrap();
        assert_eq!(python_repr_json(&integer), "0");
        assert_eq!(python_repr_json(&float), "-0.0");
        assert_eq!(python_repr_json(&huge), "123456789012345678901234567890");
    }

    #[test]
    fn python_seconds_matches_float_rendering() {
        assert_eq!(python_seconds(std::time::Duration::from_secs(10)), "10.0");
        assert_eq!(python_seconds(std::time::Duration::from_millis(200)), "0.2");
        // Sub-millisecond timeouts are caller-supplied; Python's f-string uses
        // `repr(float)`, which switches to exponent form below 1e-4.
        assert_eq!(python_seconds(std::time::Duration::from_micros(1)), "1e-06");
        assert_eq!(
            python_seconds(std::time::Duration::from_nanos(100)),
            "1e-07"
        );
    }

    // Expected strings below are CPython 3.11 `repr()` output, produced by the
    // pinned oracle (`/tmp/sk-op-float-repr-probe.log`).
    #[test]
    fn python_float_repr_renders_nonfinite_like_cpython() {
        assert_eq!(python_float_repr(f64::NAN), "nan");
        assert_eq!(python_float_repr(f64::INFINITY), "inf");
        assert_eq!(python_float_repr(f64::NEG_INFINITY), "-inf");
    }

    #[test]
    fn python_float_repr_uses_cpython_thresholds() {
        for (value, expected) in [
            (10.0_f64, "10.0"),
            (0.2, "0.2"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (0.000001, "1e-06"),
            (0.0000001, "1e-07"),
            (1.5e-7, "1.5e-07"),
            (1234.5678, "1234.5678"),
            (1.2345678901234568e-5, "1.2345678901234568e-05"),
            (1e15, "1000000000000000.0"),
            (1e16, "1e+16"),
            (1e20, "1e+20"),
            (1e300, "1e+300"),
            (-0.0, "-0.0"),
        ] {
            assert_eq!(python_float_repr(value), expected, "repr({value:?})");
        }
    }

    #[test]
    fn python_repr_json_renders_json_floats_like_python() {
        use serde_json::json;
        assert_eq!(python_repr_json(&json!(1e20)), "1e+20");
        assert_eq!(python_repr_json(&json!(5.0)), "5.0");
        assert_eq!(python_repr_json(&json!(5)), "5");
        assert_eq!(python_repr_json(&json!(-0.0)), "-0.0");
        assert_eq!(
            python_repr_json(&json!([1e20, true, null])),
            "[1e+20, True, None]"
        );
    }
}
