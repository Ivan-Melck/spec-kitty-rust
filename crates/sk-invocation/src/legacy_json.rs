//! Private CPython `json.loads` compatibility reader for raw legacy Op rows.
//!
//! The executor's best-effort commit reads the **first line** of an Op record
//! with `path.read_text(encoding="utf-8").splitlines()[0]` and then
//! `json.loads` that line (`executor.py::_read_started_event`). That reader is
//! deliberately looser than the strict Op claim/model readers and the wire
//! writer:
//!
//! * `NaN`, `Infinity` and `-Infinity` are accepted as bare constants
//!   (case-sensitive; `+Infinity` and `nan` are not).
//! * Object members keep Python dict order; a duplicate key keeps its **first
//!   insertion position** and takes the **last** value, including nested maps.
//! * Integer tokens are exact (`-0` decodes to `0`); float tokens (with `.`,
//!   `e` or `E`) decode like CPython `float`, so overflow becomes `±inf` and
//!   underflow becomes signed zero.
//!
//! `serde_json::Value` cannot carry non-finite values or an ordered map without
//! a workspace-wide `preserve_order` feature change, so this focused private
//! value tree exists instead. It is not a general JSON library, wire writer or
//! public capability.
//!
//! The whole first line is still validated: exactly one JSON value wrapped in
//! optional JSON whitespace (no trailing data), object keys must be strings,
//! and every nested member is parsed. Missing/empty/non-object/bad rows remain
//! the executor's `Invalid invocation record: <id>` failure.
//!
//! Genuine remaining gaps (not claimed as arbitrary-JSON equivalence): a lone
//! `\uD800`-style surrogate parses in Python but cannot be represented by Rust
//! `String` and is refused here; nested-string `repr` reuses
//! [`crate::python_json::python_repr`], whose `isprintable()` table is incomplete
//! (for example NBSP). Integer conversion follows the reference interpreter's
//! default 4300-digit ceiling (or its existing `PYTHONINTMAXSTRDIGITS` startup
//! setting, including zero for unlimited). A native operational budget of 1000 value frames
//! (the reference's default recursion budget) prevents process stack loss on
//! source-refused nesting. This is NOT a fixed JSON-format limit or exact
//! emulation of the source's caller-dependent ~992–994 acceptance ceiling.
//! Customized interpreter recursion settings, runtime Python integer-limit
//! mutations and the small acceptance band below this native budget remain
//! explicit compatibility differences.

use crate::python_json::{python_float_repr, python_repr};

// Verified against the pinned CPython 3.11.15 defaults. In particular the
// recursion budget is operational, not a claim about a JSON wire-format depth.
const SOURCE_DEFAULT_RECURSION_BUDGET: usize = 1000;
const SOURCE_DEFAULT_INTEGER_DIGITS: usize = 4300;

/// One decoded `json.loads` value.
#[derive(Clone, Debug)]
pub(crate) enum LegacyValue {
    /// The `null` constant.
    Null,
    /// `true` / `false`.
    Bool(bool),
    /// An exact integer token (`-0` is normalised to `0`).
    Integer(String),
    /// A float token, decoded like CPython `float` (`±inf`, signed zero, NaN).
    Float(f64),
    /// A decoded string (raw Unicode scalar values only).
    String(String),
    /// An ordered JSON array.
    Array(Vec<LegacyValue>),
    /// An ordered JSON object.
    Object(LegacyObject),
}

/// A Python-dict-ordered JSON object with duplicate-key semantics.
///
/// A duplicate key replaces the value in place, so the first insertion position
/// is kept, exactly like `dict` assignment during `json.loads`.
#[derive(Clone, Debug, Default)]
pub(crate) struct LegacyObject {
    entries: Vec<(String, LegacyValue)>,
}

impl LegacyObject {
    /// The value stored under `key`, if any.
    pub(crate) fn get(&self, key: &str) -> Option<&LegacyValue> {
        self.entries
            .iter()
            .find_map(|(name, value)| (name == key).then_some(value))
    }

    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn insert(&mut self, key: String, value: LegacyValue) {
        if let Some((_, existing)) = self.entries.iter_mut().find(|(name, _)| *name == key) {
            *existing = value;
        } else {
            self.entries.push((key, value));
        }
    }
}

/// Decode the first row as a Python `dict`, or `None` for the invalid cases.
pub(crate) fn parse_object(text: &str) -> Option<LegacyObject> {
    let setting = match std::env::var("PYTHONINTMAXSTRDIGITS") {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => return None,
    };
    let integer_digits = integer_digit_limit(setting.as_deref()).ok()?;
    let mut reader = Reader {
        text,
        bytes: text.as_bytes(),
        index: 0,
        depth: 0,
        integer_digits,
    };
    let value = reader.value().ok()?;
    reader.skip_whitespace();
    if reader.index != reader.bytes.len() {
        return None;
    }
    match value {
        LegacyValue::Object(object) => Some(object),
        _ => None,
    }
}

// CPython startup uses a signed C int with leading ASCII whitespace accepted,
// but no trailing data; empty means default, 0 unlimited, otherwise >= 640.
fn integer_digit_limit(setting: Option<&str>) -> Result<usize, ()> {
    let Some(setting) = setting.filter(|value| !value.is_empty()) else {
        return Ok(SOURCE_DEFAULT_INTEGER_DIGITS);
    };
    let value = setting.trim_start_matches([' ', '\t', '\n', '\r', '\u{0b}', '\u{0c}']);
    let limit = value.parse::<i32>().map_err(|_| ())?;
    if limit == 0 || limit >= 640 {
        usize::try_from(limit).map_err(|_| ())
    } else {
        Err(())
    }
}

/// Python truthiness, as used by `started.get(key) or "unknown"`.
pub(crate) fn is_falsy(value: &LegacyValue) -> bool {
    match value {
        LegacyValue::Null => true,
        LegacyValue::Bool(value) => !value,
        LegacyValue::Integer(digits) => digits == "0",
        LegacyValue::Float(value) => *value == 0.0,
        LegacyValue::String(text) => text.is_empty(),
        LegacyValue::Array(items) => items.is_empty(),
        LegacyValue::Object(object) => object.is_empty(),
    }
}

/// Python `str(value)` for one decoded value.
///
/// A top-level string is verbatim; every other value uses its `repr`, because
/// `str(list)`/`str(dict)` delegate to element `repr` in CPython.
pub(crate) fn python_str(value: &LegacyValue) -> String {
    match value {
        LegacyValue::String(text) => text.clone(),
        other => python_repr_value(other),
    }
}

fn python_repr_value(value: &LegacyValue) -> String {
    match value {
        LegacyValue::Null => "None".to_owned(),
        LegacyValue::Bool(true) => "True".to_owned(),
        LegacyValue::Bool(false) => "False".to_owned(),
        LegacyValue::Integer(digits) => digits.clone(),
        LegacyValue::Float(value) => python_float_repr(*value),
        LegacyValue::String(text) => python_repr(text),
        LegacyValue::Array(items) => {
            let rendered: Vec<String> = items.iter().map(python_repr_value).collect();
            format!("[{}]", rendered.join(", "))
        }
        LegacyValue::Object(object) => {
            let rendered: Vec<String> = object
                .entries
                .iter()
                .map(|(key, value)| format!("{}: {}", python_repr(key), python_repr_value(value)))
                .collect();
            format!("{{{}}}", rendered.join(", "))
        }
    }
}

/// A recursive-descent reader for the CPython `json.loads` lexical grammar.
struct Reader<'a> {
    text: &'a str,
    bytes: &'a [u8],
    index: usize,
    depth: usize,
    integer_digits: usize,
}

impl Reader<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.index).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.index += 1;
        Some(byte)
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.index += 1;
        }
    }

    fn expect(&mut self, expected: &[u8]) -> Result<(), ()> {
        if self.bytes.get(self.index..self.index + expected.len()) == Some(expected) {
            self.index += expected.len();
            Ok(())
        } else {
            Err(())
        }
    }

    fn value(&mut self) -> Result<LegacyValue, ()> {
        self.skip_whitespace();
        if self.depth >= SOURCE_DEFAULT_RECURSION_BUDGET {
            return Err(());
        }
        self.depth += 1;
        // Any error discards the entire reader; only successful returns need
        // to restore the counter before a sibling value is decoded.
        let result = match self.peek().ok_or(())? {
            b'n' => {
                self.expect(b"null")?;
                Ok(LegacyValue::Null)
            }
            b't' => {
                self.expect(b"true")?;
                Ok(LegacyValue::Bool(true))
            }
            b'f' => {
                self.expect(b"false")?;
                Ok(LegacyValue::Bool(false))
            }
            b'N' => {
                self.expect(b"NaN")?;
                Ok(LegacyValue::Float(f64::NAN))
            }
            b'I' => {
                self.expect(b"Infinity")?;
                Ok(LegacyValue::Float(f64::INFINITY))
            }
            b'-' if self.bytes[self.index..].starts_with(b"-Infinity") => {
                self.index += b"-Infinity".len();
                Ok(LegacyValue::Float(f64::NEG_INFINITY))
            }
            b'-' | b'0'..=b'9' => self.number(),
            b'[' => self.array(),
            b'{' => self.object(),
            b'"' => self.string().map(LegacyValue::String),
            _ => Err(()),
        };
        self.depth -= 1;
        result
    }

    fn number(&mut self) -> Result<LegacyValue, ()> {
        let start = self.index;
        if self.peek() == Some(b'-') {
            self.index += 1;
        }
        match self.peek().ok_or(())? {
            b'0' => self.index += 1,
            b'1'..=b'9' => {
                self.index += 1;
                while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                    self.index += 1;
                }
            }
            _ => return Err(()),
        }
        let mut is_float = false;
        if self.peek() == Some(b'.') {
            is_float = true;
            self.index += 1;
            if !self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                return Err(());
            }
            while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                self.index += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            is_float = true;
            self.index += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.index += 1;
            }
            if !self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                return Err(());
            }
            while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                self.index += 1;
            }
        }
        let token = &self.text[start..self.index];
        if is_float {
            // CPython `float` and Rust `f64` share the JSON number grammar and
            // correctly-rounded parsing: overflow is `±inf`, underflow keeps a
            // signed zero. Integer tokens never pass through `f64`.
            token.parse().map(LegacyValue::Float).map_err(|_| ())
        } else if self.integer_digits != 0
            && token.strip_prefix('-').unwrap_or(token).len() > self.integer_digits
        {
            // CPython's default int conversion counts digits, not the sign.
            // Float mantissas/exponents do not have this integer-only ceiling.
            Err(())
        } else if token == "-0" {
            Ok(LegacyValue::Integer("0".to_owned()))
        } else {
            Ok(LegacyValue::Integer(token.to_owned()))
        }
    }

    fn string(&mut self) -> Result<String, ()> {
        self.expect(b"\"")?;
        let mut out = String::new();
        let mut chunk_start = self.index;
        loop {
            match self.peek().ok_or(())? {
                b'"' => {
                    out.push_str(self.text.get(chunk_start..self.index).ok_or(())?);
                    self.index += 1;
                    return Ok(out);
                }
                b'\\' => {
                    out.push_str(self.text.get(chunk_start..self.index).ok_or(())?);
                    self.index += 1;
                    self.escape(&mut out)?;
                    chunk_start = self.index;
                }
                byte if byte < 0x20 => return Err(()),
                _ => self.index += 1,
            }
        }
    }

    fn escape(&mut self, out: &mut String) -> Result<(), ()> {
        match self.bump().ok_or(())? {
            b'"' => out.push('"'),
            b'\\' => out.push('\\'),
            b'/' => out.push('/'),
            b'b' => out.push('\u{08}'),
            b'f' => out.push('\u{0c}'),
            b'n' => out.push('\n'),
            b'r' => out.push('\r'),
            b't' => out.push('\t'),
            b'u' => self.unicode_escape(out)?,
            _ => return Err(()),
        }
        Ok(())
    }

    fn unicode_escape(&mut self, out: &mut String) -> Result<(), ()> {
        let first = self.hex4()?;
        if (0xdc00..=0xdfff).contains(&first) {
            // Python keeps a lone low surrogate; Rust `String` cannot.
            return Err(());
        }
        if (0xd800..=0xdbff).contains(&first) {
            self.expect(b"\\u")?;
            let second = self.hex4()?;
            if !(0xdc00..=0xdfff).contains(&second) {
                return Err(());
            }
            let combined = 0x1_0000 + ((first - 0xd800) << 10) + (second - 0xdc00);
            out.push(char::from_u32(combined).ok_or(())?);
        } else {
            out.push(char::from_u32(first).ok_or(())?);
        }
        Ok(())
    }

    fn hex4(&mut self) -> Result<u32, ()> {
        let mut value = 0u32;
        for _ in 0..4 {
            let byte = self.bump().ok_or(())?;
            let digit = match byte {
                b'0'..=b'9' => u32::from(byte - b'0'),
                b'a'..=b'f' => u32::from(byte - b'a') + 10,
                b'A'..=b'F' => u32::from(byte - b'A') + 10,
                _ => return Err(()),
            };
            value = value * 16 + digit;
        }
        Ok(value)
    }

    fn array(&mut self) -> Result<LegacyValue, ()> {
        self.expect(b"[")?;
        self.skip_whitespace();
        let mut items = Vec::new();
        if self.peek() == Some(b']') {
            self.index += 1;
            return Ok(LegacyValue::Array(items));
        }
        loop {
            items.push(self.value()?);
            self.skip_whitespace();
            match self.bump().ok_or(())? {
                b',' => self.skip_whitespace(),
                b']' => return Ok(LegacyValue::Array(items)),
                _ => return Err(()),
            }
        }
    }

    fn object(&mut self) -> Result<LegacyValue, ()> {
        self.expect(b"{")?;
        self.skip_whitespace();
        let mut object = LegacyObject::default();
        if self.peek() == Some(b'}') {
            self.index += 1;
            return Ok(LegacyValue::Object(object));
        }
        loop {
            self.skip_whitespace();
            // JSON object keys are strings; Python rejects anything else.
            let key = self.string()?;
            self.skip_whitespace();
            self.expect(b":")?;
            let value = self.value()?;
            object.insert(key, value);
            self.skip_whitespace();
            match self.bump().ok_or(())? {
                b',' => self.skip_whitespace(),
                b'}' => return Ok(LegacyValue::Object(object)),
                _ => return Err(()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(text: &str, key: &str) -> String {
        let object = parse_object(text).expect("valid object");
        let value = object.get(key).expect("field is present");
        assert!(!is_falsy(value), "the fixture field is truthy");
        python_str(value)
    }

    #[test]
    fn integer_startup_configuration_matches_pinned_cpython_facts() {
        assert_eq!(integer_digit_limit(None), Ok(4300));
        for value in ["", "0", "640", "+640", "-0", " 640", "000640"] {
            let expected = if value.is_empty() {
                4300
            } else if value == "0" || value == "-0" {
                0
            } else {
                640
            };
            assert_eq!(integer_digit_limit(Some(value)), Ok(expected), "{value:?}");
        }
        for value in ["640 ", "639", "bad", "2147483648"] {
            assert!(integer_digit_limit(Some(value)).is_err(), "{value:?}");
        }
    }

    #[test]
    fn duplicate_keys_keep_the_first_position_and_the_last_value() {
        assert_eq!(
            field(r#"{"p": {"a": 1, "b": 2, "a": 3}}"#, "p"),
            "{'a': 3, 'b': 2}"
        );
        assert_eq!(field(r#"{"p": 1, "q": 2, "p": 3}"#, "p"), "3");
    }

    #[test]
    fn nested_maps_and_arrays_keep_python_insertion_order() {
        assert_eq!(field(r#"{"p": {"b": 1, "a": 2}}"#, "p"), "{'b': 1, 'a': 2}");
        assert_eq!(
            field(
                r#"{"p": [1, NaN, {"z": 1, "y": 2}, true, null, [], {}]}"#,
                "p"
            ),
            "[1, nan, {'z': 1, 'y': 2}, True, None, [], {}]"
        );
    }

    #[test]
    fn nonfinite_constants_and_number_overflow_use_python_float_rendering() {
        assert_eq!(field(r#"{"p": NaN}"#, "p"), "nan");
        assert_eq!(field(r#"{"p": Infinity}"#, "p"), "inf");
        assert_eq!(field(r#"{"p": -Infinity}"#, "p"), "-inf");
        assert_eq!(field(r#"{"p": 1e400}"#, "p"), "inf");
        assert_eq!(field(r#"{"p": -1e400}"#, "p"), "-inf");
        assert_eq!(field(r#"{"p": 1e300}"#, "p"), "1e+300");
        assert!(!is_falsy(&LegacyValue::Float(f64::NAN)));
        assert!(!is_falsy(&LegacyValue::Float(f64::INFINITY)));
    }

    #[test]
    fn underflow_and_signed_zero_are_falsy_but_keep_python_repr() {
        for text in [r#"{"p": 1e-400}"#, r#"{"p": -1e-400}"#, r#"{"p": 0.0}"#] {
            let object = parse_object(text).expect("valid object");
            assert!(is_falsy(object.get("p").unwrap()), "{text}");
        }
        assert_eq!(
            python_str(parse_object(r#"{"p": -1e-400}"#).unwrap().get("p").unwrap()),
            "-0.0"
        );
        assert_eq!(
            python_str(parse_object(r#"{"p": -0.0}"#).unwrap().get("p").unwrap()),
            "-0.0"
        );
        assert_eq!(
            python_str(parse_object(r#"{"p": -1e-400}"#).unwrap().get("p").unwrap()),
            "-0.0"
        );
    }

    #[test]
    fn exact_integers_are_never_coerced_through_f64() {
        assert_eq!(
            field(r#"{"p": 123456789012345678901234567890}"#, "p"),
            "123456789012345678901234567890"
        );
        let object = parse_object(r#"{"p": -0}"#).unwrap();
        assert_eq!(python_str(object.get("p").unwrap()), "0");
        assert!(is_falsy(object.get("p").unwrap()));
    }

    #[test]
    fn authored_strings_are_verbatim_and_nested_strings_use_python_repr() {
        assert_eq!(field(r#"{"p": "NaN"}"#, "p"), "NaN");
        assert_eq!(field(r#"{"p": "Infinity"}"#, "p"), "Infinity");
        assert_eq!(field(r#"{"p": {"k": "NaN"}}"#, "p"), "{'k': 'NaN'}");
        assert_eq!(
            field(r#"{"p": "line\nit's"}"#, "p"),
            "line\nit's",
            "a top-level string is verbatim"
        );
        assert_eq!(field(r#"{"p": [{"k": "it's"}]}"#, "p"), "[{'k': \"it's\"}]");
    }

    #[test]
    fn unicode_escapes_decode_raw_scalars() {
        assert_eq!(
            field(r#"{"p": "\u00e9 \ud83d\ude00 \u0000"}"#, "p"),
            "\u{e9} \u{1f600} \u{0}"
        );
    }

    #[test]
    fn optional_json_whitespace_around_one_object_is_accepted() {
        assert!(parse_object("  {\"p\": 1}  ").is_some());
        assert!(parse_object("\t{\"p\": 1}\t").is_some());
        assert!(parse_object("{}").is_some());
    }

    #[test]
    fn bad_syntax_and_non_objects_are_rejected() {
        for text in [
            "",
            " ",
            "NaN",
            "null",
            "[]",
            "[NaN]",
            r#"{"p": 1} x"#,
            r#"{"p": 1}{"q": 2}"#,
            r#"{"p": +Infinity}"#,
            r#"{"p": nan}"#,
            r#"{"p": -NaN}"#,
            r#"{"p": Inf}"#,
            r#"{"p": 1,}"#,
            r#"{"p": 01}"#,
            r#"{"p": .5}"#,
            r#"{"p": 1.}"#,
            r#"{"p": 1e}"#,
            r#"{"p": [1,]}"#,
            r#"{"p": {"a": 1,}}"#,
            r#"{"p" 1}"#,
            r#"{p: 1}"#,
            r#"{1: 2}"#,
            r#"{"p": "unterminated}"#,
            r#"{"p": "bad\qescape"}"#,
            r#"{"p": "raw
control"}"#,
            // A lone surrogate parses in Python but not in Rust `String`; this
            // disclosed gap is refused rather than silently substituted.
            r#"{"p": "\ud800"}"#,
            // A UTF-8 BOM is an explicit `json.loads` error.
            "\u{feff}{\"p\": 1}",
        ] {
            assert!(parse_object(text).is_none(), "must reject {text:?}");
        }
    }
}
