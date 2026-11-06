//! The workspace's one exact Python JSON read/render authority.
//!
//! The reference writes every Op line with `json.dumps(model_dump(
//! exclude_none=True))`, i.e. the library defaults: insertion (declared field)
//! order, `", "` between items, `": "` after a key, and `ensure_ascii=True`.
//! `serde_json`'s compact UTF-8 output is a different byte shape, so Op lines
//! are rendered here instead of through `serde_json::to_string`.
//!
//! One module, one value tree, one encoder and one reader:
//!
//! * [`PythonJsonValue`] is the tree. The reader ([`parse`], [`parse_python_string`])
//!   fills it from a document; [`PythonJsonObject`] builds scalar Op lines;
//!   [`PythonJsonValue::render_python`] renders any tree to exact Python code
//!   points and [`PythonJsonValue::render_with`] is the explicit scalar view.
//! * Object keys from the reader are [`PythonJsonDict`] entries, so they carry
//!   every Python code unit — the 2048 surrogate units included — as real string
//!   data, not tag namespaces or escaped pseudo-keys.
//! * Escaping reproduces CPython's `py_encode_basestring_ascii` for
//!   `ensure_ascii=True` (`"`/`\`, the five short control escapes, `\uXXXX` for
//!   every other code point below `U+0020` and at or above `U+007F`, surrogate
//!   pairs above the BMP) and `py_encode_basestring` for `ensure_ascii=False`
//!   (only `"`, `\`, the short escapes and `< U+0020` escape; everything else is
//!   the raw code point). There is exactly one formatter per mode and one
//!   printability table; the same engine renders the scalar Op builder.
//! * `json.dumps` and its physical encode are separate phases, exactly like the
//!   reference: [`PythonJsonValue::render_python`] returns a [`PythonString`]
//!   (so `ensure_ascii=False` returns lone surrogates instead of refusing), and
//!   the UTF-8 conversion is a later, explicitly fallible caller step.
//!
//! The canonical CPython `repr()` formatter lives with the Python-string
//! primitive in `sk-kernel`; this module re-exports it under the same callable
//! names so every existing `sk_invocation` path and Op writer behaviour is
//! unchanged. There is exactly one formatter and one printability table.

use crate::PythonString;
pub use crate::python_repr::python_repr;

mod equality;
mod order;
mod reader;
mod sort;
mod value;

pub use equality::{
    python_json_dict_equal, python_json_equal, python_json_optional_sequence_equal,
};
pub use order::{
    PythonJsonCompareError, python_json_hashable, python_json_key_equal, python_json_le,
    python_json_lt, python_json_stable_sort_by_key, python_json_str, python_json_truthy,
    python_json_tuple_hashable, python_json_tuple_lt, python_json_type_name,
};

pub use reader::{
    PythonJsonError, PythonJsonErrorKind, SOURCE_DEFAULT_INTEGER_DIGITS,
    SOURCE_DEFAULT_RECURSION_BUDGET, environment_integer_digits, parse, parse_python_string,
    parse_python_string_with_integer_digits, parse_str_with_integer_digits,
    source_integer_digit_limit,
};
pub use value::{PythonJsonDict, is_falsy, python_repr_value};

#[cfg(test)]
use crate::python_repr::python_repr_codepoints;

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
    push_json_codepoints(target, value.chars().map(u32::from));
}

/// A text sink for the one `ensure_ascii=True` escape engine.
///
/// `PythonString` receives exact code points; `String` receives the same bytes
/// for the scalar Op builder (the ascii escape output is always scalar).
trait JsonCodepointSink {
    fn push_text(&mut self, text: &str);
    fn push_code(&mut self, code: u32);
}

impl JsonCodepointSink for String {
    fn push_text(&mut self, text: &str) {
        self.push_str(text);
    }

    fn push_code(&mut self, code: u32) {
        self.push(char::from_u32(code).expect("the ascii encoder emits scalars"));
    }
}

impl JsonCodepointSink for PythonString {
    fn push_text(&mut self, text: &str) {
        self.push_scalars(text);
    }

    fn push_code(&mut self, code: u32) {
        self.push_codepoint(code)
            .expect("the ascii encoder emits valid Python code points");
    }
}

/// CPython `repr()` for an arbitrary JSON value, used in parse-refusal messages.
///
/// A `serde_json` capability view only: it projects through the canonical tree
/// ([`PythonJsonValue::from_serde`]) so there is one value-repr walker. The
/// serde map is key-sorted, which is exactly why it is not the Source read
/// authority.
pub fn python_repr_json(value: &serde_json::Value) -> String {
    python_repr_value(&PythonJsonValue::from_serde(value))
}

/// CPython `repr()`/`str()` for a float, including the non-finite values that
/// `json.loads` can produce.
///
/// Python uses shortest round-trip digits and switches to exponent form when
/// the decimal exponent is below `-4` or at least `16` (`1e-05`, `1e+20`),
/// with a two-digit minimum exponent and an always-present `.0` on integral
/// positional values. Rust's `Display` never uses exponents for `f64`, so the
/// threshold is applied here. `nan`/`inf`/`-inf` match CPython's lower-case
/// spellings. Used by operator-facing text and model-JSON mapping-key projection.
pub fn python_float_repr(value: f64) -> String {
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

// ---------------------------------------------------------------------------
// Exact Python `json.dumps` value tree
// ---------------------------------------------------------------------------

/// One value in Python's `json.dumps` domain.
///
/// Rust `String` cannot carry a lone surrogate and `serde_json::Map` keeps keys
/// sorted, so a payload that must be byte-identical to `json.dumps` is built as
/// this ordered tree and rendered by [`PythonJsonValue::render_python`] — the
/// same encoder (`ensure_ascii=True`, CPython's escapes) the Op writer uses.
///
/// The variants are one model, not competing authorities:
///
/// * [`PythonJsonValue::ExactObject`] (exact code-point keys),
///   [`PythonJsonValue::Integer`] and [`PythonJsonValue::Float`] are what the
///   canonical reader produces.
/// * [`PythonJsonValue::Object`] (scalar keys) and
///   [`PythonJsonValue::Number`] (an explicit pre-formatted token) are the
///   producer convenience views that existing callers already build. They render
///   through the same arms and never replace the reader's representation.
#[derive(Clone, Debug, PartialEq)]
pub enum PythonJsonValue {
    /// `null`
    Null,
    /// `true` / `false`
    Bool(bool),
    /// An explicit pre-formatted JSON number token (producer convenience view).
    ///
    /// The caller owns the token text; the reader does not produce this variant.
    Number(String),
    /// An exact Python `int` decoded from JSON (`-0` is canonicalised to `0`).
    Integer(String),
    /// An exact Python `float` decoded from JSON (`NaN`/`±inf`/signed zero).
    Float(f64),
    /// A Python `str`, exact code points including surrogates.
    String(PythonString),
    /// A Python list.
    Array(Vec<PythonJsonValue>),
    /// A Python dict with scalar keys, in insertion order (legacy convenience).
    Object(Vec<(String, PythonJsonValue)>),
    /// A Python dict with exact code-point keys (the reader's representation).
    ExactObject(PythonJsonDict),
}

/// How [`PythonJsonValue::render_python_with`] renders a document.
///
/// This is the ONE render-options API. It covers the three reference knobs a
/// caller needs: `sort_keys`, `separators` (including the compact pair) and the
/// `default`-projection contract. `ensure_ascii` and `indent` are carried too,
/// so a caller never needs a private encoder beside this one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PythonJsonRenderOptions {
    /// `None` for the reference default layout, `Some(width)` for `indent=width`.
    pub indent: Option<usize>,
    /// `json.dumps(..., ensure_ascii=<flag>)`.
    pub ensure_ascii: bool,
    /// `json.dumps(..., sort_keys=<flag>)`; caller-scoped, never a global map
    /// ordering change.
    pub sort_keys: bool,
    /// `json.dumps(..., separators=<pair>)`.
    ///
    /// `None` is the reference resolution: `(", ", ": ")` without `indent` and
    /// `(",", ": ")` with `indent` (CPython keeps `": "` and takes the item
    /// separator from the default `,`).
    pub separators: Option<PythonJsonSeparators>,
    /// The reference `json.dumps(..., default=...)` projection contract.
    pub default: PythonJsonDefault,
}

impl Default for PythonJsonRenderOptions {
    fn default() -> Self {
        Self {
            indent: None,
            ensure_ascii: true,
            sort_keys: false,
            separators: None,
            default: PythonJsonDefault::Reject,
        }
    }
}

impl PythonJsonRenderOptions {
    /// The reference defaults with a chosen `indent`.
    pub fn new(indent: Option<usize>) -> Self {
        Self {
            indent,
            ..Self::default()
        }
    }

    /// Set `ensure_ascii`.
    pub fn ensure_ascii(mut self, ensure_ascii: bool) -> Self {
        self.ensure_ascii = ensure_ascii;
        self
    }

    /// Set `sort_keys`.
    pub fn sort_keys(mut self, sort_keys: bool) -> Self {
        self.sort_keys = sort_keys;
        self
    }

    /// Set an explicit `separators` pair (or `None` for the reference resolution).
    pub fn separators(mut self, separators: Option<PythonJsonSeparators>) -> Self {
        self.separators = separators;
        self
    }

    /// Declare how non-JSON Python values were projected.
    pub fn default_projection(mut self, default: PythonJsonDefault) -> Self {
        self.default = default;
        self
    }

    /// The separators the reference encoder would use for this option set.
    pub fn resolved_separators(&self) -> PythonJsonSeparators {
        self.separators.unwrap_or(if self.indent.is_none() {
            PythonJsonSeparators::DEFAULT
        } else {
            PythonJsonSeparators::INDENTED
        })
    }
}

/// The reference `json.dumps(separators=(item, key))` pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PythonJsonSeparators {
    /// Between object members / array items.
    pub item: &'static str,
    /// After an object key.
    pub key: &'static str,
}

impl PythonJsonSeparators {
    /// `json.dumps` defaults without `indent`: `(", ", ": ")`.
    pub const DEFAULT: Self = Self {
        item: ", ",
        key: ": ",
    };

    /// `separators=(",", ":")`, the compact wire form.
    pub const COMPACT: Self = Self {
        item: ",",
        key: ":",
    };

    /// The reference resolution when `indent` is set without separators.
    pub const INDENTED: Self = Self {
        item: ",",
        key: ": ",
    };

    /// An explicit pair.
    pub const fn new(item: &'static str, key: &'static str) -> Self {
        Self { item, key }
    }
}

/// The reference `json.dumps(default=...)` projection contract for a typed tree.
///
/// The typed tree has no non-JSON values: a caller that needs the reference
/// `default=str` projects the non-JSON object **upstream** through the Kernel's
/// one Python `str()` primitive and stores the result as
/// [`PythonJsonValue::String`]. Dumping that projection produces exactly the
/// bytes of `json.dumps(value, default=str)`, because CPython feeds the
/// projection's return value back through the normal string encoder.
///
/// The selector therefore records caller intent instead of adding a codec-side
/// `default` hook: a hook would be a second, caller-supplied formatter, which
/// this single-authority module refuses to introduce. Both variants render
/// identically; `Reject` says the document was already JSON-only.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PythonJsonDefault {
    /// The document is JSON-only; the reference `default` is never called.
    #[default]
    Reject,
    /// Non-JSON values were projected with `str()` upstream of the encoder.
    PythonStr,
}

/// A borrowed object key for the shared member renderer.
enum JsonKeyRef<'a> {
    Scalar(&'a str),
    Exact(&'a PythonString),
}

impl JsonKeyRef<'_> {
    /// Python `str` comparison order (code points).
    fn cmp_codepoints(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            (Self::Scalar(left), Self::Scalar(right)) => left.cmp(right),
            (Self::Exact(left), Self::Exact(right)) => left.codepoints().cmp(right.codepoints()),
            (Self::Scalar(left), Self::Exact(right)) => left
                .chars()
                .map(u32::from)
                .cmp(right.codepoints().iter().copied()),
            (Self::Exact(left), Self::Scalar(right)) => left
                .codepoints()
                .iter()
                .copied()
                .cmp(right.chars().map(u32::from)),
        }
    }

    fn push(&self, out: &mut PythonString, ensure_ascii: bool) {
        match (self, ensure_ascii) {
            (Self::Scalar(text), true) => push_json_codepoints(out, text.chars().map(u32::from)),
            (Self::Scalar(text), false) => {
                push_json_codepoints_utf8(out, text.chars().map(u32::from));
            }
            (Self::Exact(value), true) => {
                push_json_codepoints(out, value.codepoints().iter().copied());
            }
            (Self::Exact(value), false) => {
                push_json_codepoints_utf8(out, value.codepoints().iter().copied());
            }
        }
    }
}

impl PythonJsonValue {
    /// A string from exact code points.
    pub fn string(value: PythonString) -> Self {
        Self::String(value)
    }

    /// A string from a Rust scalar string.
    pub fn scalar(value: &str) -> Self {
        Self::String(PythonString::from_scalars(value))
    }

    /// An optional string (`None` becomes `null`).
    pub fn optional_scalar(value: Option<&str>) -> Self {
        match value {
            Some(value) => Self::scalar(value),
            None => Self::Null,
        }
    }

    /// Convert an arbitrary JSON value. Strings are scalar Python strings; an
    /// object's key order is whatever `serde_json` retained.
    ///
    /// This is a capability view, not the Source read authority: the serde map
    /// is key-sorted (unless the workspace opts into `preserve_order`, which it
    /// does not) and cannot carry a lone surrogate. Numeric tokens become the
    /// canonical [`PythonJsonValue::Integer`]/[`PythonJsonValue::Float`] so the
    /// whole crate has one number model.
    pub fn from_serde(value: &serde_json::Value) -> Self {
        match value {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::Bool(flag) => Self::Bool(*flag),
            serde_json::Value::Number(number) => {
                // The token has already been validated by serde_json. Its
                // lexical kind, not whether it fits a machine integer, selects
                // Python's int/float semantics. Never round a large int via f64.
                let token = number.to_string();
                let is_float = token.bytes().any(|byte| matches!(byte, b'.' | b'e' | b'E'));
                if !is_float {
                    Self::Integer(if token == "-0" { "0".to_owned() } else { token })
                } else {
                    let float = token.parse::<f64>().expect("validated JSON float token");
                    Self::Float(float)
                }
            }
            serde_json::Value::String(text) => Self::scalar(text),
            serde_json::Value::Array(items) => {
                Self::Array(items.iter().map(Self::from_serde).collect())
            }
            serde_json::Value::Object(entries) => Self::Object(
                entries
                    .iter()
                    .map(|(key, value)| (key.clone(), Self::from_serde(value)))
                    .collect(),
            ),
        }
    }

    /// The scalar JSON projection, or `None` when exact data has no scalar
    /// form (a lone surrogate string/key, or a non-finite float).
    pub fn to_serde(&self) -> Option<serde_json::Value> {
        Some(match self {
            Self::Null => serde_json::Value::Null,
            Self::Bool(flag) => serde_json::Value::Bool(*flag),
            Self::Number(token) | Self::Integer(token) => serde_json::from_str(token).ok()?,
            Self::Float(number) => {
                serde_json::Value::Number(serde_json::Number::from_f64(*number)?)
            }
            Self::String(value) => serde_json::Value::String(value.to_scalar_string().ok()?),
            Self::Array(items) => serde_json::Value::Array(
                items
                    .iter()
                    .map(Self::to_serde)
                    .collect::<Option<Vec<_>>>()?,
            ),
            Self::Object(entries) => serde_json::Value::Object(
                entries
                    .iter()
                    .map(|(key, value)| Some((key.clone(), value.to_serde()?)))
                    .collect::<Option<serde_json::Map<_, _>>>()?,
            ),
            Self::ExactObject(object) => serde_json::Value::Object(
                object
                    .entries()
                    .iter()
                    .map(|(key, value)| Some((key.to_scalar_string().ok()?, value.to_serde()?)))
                    .collect::<Option<serde_json::Map<_, _>>>()?,
            ),
        })
    }

    /// An object member by key (either object representation).
    pub fn field(&self, key: &str) -> Option<&PythonJsonValue> {
        match self {
            Self::Object(entries) => entries
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value),
            Self::ExactObject(object) => object.get(key),
            _ => None,
        }
    }

    /// Insert or replace an object member, preserving an existing position.
    pub fn set(&mut self, key: &str, value: PythonJsonValue) {
        match self {
            Self::Object(entries) => {
                if let Some((_, existing)) = entries.iter_mut().find(|(name, _)| name == key) {
                    *existing = value;
                    return;
                }
                entries.push((key.to_owned(), value));
            }
            Self::ExactObject(object) => {
                object.insert(PythonString::from_scalars(key), value);
            }
            _ => {}
        }
    }

    /// Render exactly as `json.dumps(value, indent=indent, ensure_ascii=True)`.
    ///
    /// `None` uses the default `", "` / `": "` separators; `Some(n)` uses a
    /// newline plus `n` spaces per level and `": "` after a key, matching CPython.
    pub fn render(&self, indent: Option<usize>) -> String {
        self.render_with(indent, true)
            .expect("ensure_ascii=True never fails")
    }

    /// Render as `json.dumps(value, indent=indent, ensure_ascii=<flag>)` when the
    /// result is representable as a Rust scalar string.
    ///
    /// This is the **explicit scalar capability view**, not the parity
    /// authority: with `ensure_ascii=False` a lone surrogate has no UTF-8
    /// encoding, so the exact writer reports the boundary as `None`. Use
    /// [`PythonJsonValue::render_python`] for the reference `json.dumps` text
    /// phase, which returns the surrogate instead of refusing.
    pub fn render_with(&self, indent: Option<usize>, ensure_ascii: bool) -> Option<String> {
        self.render_python(indent, ensure_ascii)
            .to_scalar_string()
            .ok()
    }

    /// Render exactly as `json.dumps(value, indent=indent, ensure_ascii=<flag>)`
    /// into exact Python code points.
    ///
    /// This is the reference text phase: it never refuses a value the reference
    /// `json.dumps` renders. A physical UTF-8 encode is a separate late step
    /// (for example [`PythonString::to_scalar_string`] or the caller's
    /// filesystem encoder), exactly like the reference's `str` then `encode`.
    pub fn render_python(&self, indent: Option<usize>, ensure_ascii: bool) -> PythonString {
        self.render_python_with(PythonJsonRenderOptions {
            indent,
            ensure_ascii,
            ..PythonJsonRenderOptions::default()
        })
    }

    /// Render with the full `json.dumps` option set into exact code points.
    pub fn render_python_with(&self, options: PythonJsonRenderOptions) -> PythonString {
        let mut out = PythonString::default();
        self.render_into(&mut out, options, 0);
        out
    }

    fn render_into(&self, out: &mut PythonString, options: PythonJsonRenderOptions, depth: usize) {
        match self {
            Self::Null => out.push_scalars("null"),
            Self::Bool(true) => out.push_scalars("true"),
            Self::Bool(false) => out.push_scalars("false"),
            Self::Number(token) | Self::Integer(token) => out.push_scalars(token),
            Self::Float(number) => push_python_float(out, *number),
            Self::String(value) => {
                if options.ensure_ascii {
                    push_json_codepoints(out, value.codepoints().iter().copied());
                } else {
                    push_json_codepoints_utf8(out, value.codepoints().iter().copied());
                }
            }
            Self::Array(items) => {
                if items.is_empty() {
                    out.push_scalars("[]");
                    return;
                }
                out.push_scalars("[");
                for (position, item) in items.iter().enumerate() {
                    push_item_prefix(out, options, depth, position);
                    item.render_into(out, options, depth + 1);
                }
                push_container_suffix(out, options, depth, "]");
            }
            Self::Object(entries) => {
                let members: Vec<(JsonKeyRef<'_>, &PythonJsonValue)> = entries
                    .iter()
                    .map(|(key, value)| (JsonKeyRef::Scalar(key.as_str()), value))
                    .collect();
                render_members(out, options, depth, members);
            }
            Self::ExactObject(object) => {
                let members: Vec<(JsonKeyRef<'_>, &PythonJsonValue)> = object
                    .entries()
                    .iter()
                    .map(|(key, value)| (JsonKeyRef::Exact(key), value))
                    .collect();
                render_members(out, options, depth, members);
            }
        }
    }
}

/// The reference `json.dumps` text for one float.
///
/// Finite values use CPython's `float.__repr__` (the same function the encoder
/// calls); the reference's default `allow_nan=True` writes `NaN`, `Infinity`
/// and `-Infinity` for the non-finite values.
fn push_python_float(out: &mut PythonString, value: f64) {
    if value.is_nan() {
        out.push_scalars("NaN");
    } else if value.is_infinite() {
        out.push_scalars(if value.is_sign_negative() {
            "-Infinity"
        } else {
            "Infinity"
        });
    } else {
        out.push_scalars(&python_float_repr(value));
    }
}

/// One object with the shared separators, indentation and `sort_keys` rule.
fn render_members(
    out: &mut PythonString,
    options: PythonJsonRenderOptions,
    depth: usize,
    mut members: Vec<(JsonKeyRef<'_>, &PythonJsonValue)>,
) {
    if members.is_empty() {
        out.push_scalars("{}");
        return;
    }
    if options.sort_keys {
        members.sort_by(|left, right| left.0.cmp_codepoints(&right.0));
    }
    let separators = options.resolved_separators();
    out.push_scalars("{");
    for (position, (key, value)) in members.iter().enumerate() {
        push_item_prefix(out, options, depth, position);
        key.push(out, options.ensure_ascii);
        out.push_scalars(separators.key);
        value.render_into(out, options, depth + 1);
    }
    push_container_suffix(out, options, depth, "}");
}

/// `item_separator` (plus the indent newline) before an item, exactly like
/// CPython's encoder: with `indent` the separator is followed by a newline and
/// the padding, and an explicit item separator is still honoured.
fn push_item_prefix(
    out: &mut PythonString,
    options: PythonJsonRenderOptions,
    depth: usize,
    position: usize,
) {
    if position > 0 {
        out.push_scalars(options.resolved_separators().item);
    }
    if let Some(width) = options.indent {
        out.push_scalars("\n");
        push_padding_into(out, width * (depth + 1));
    }
}

/// Closing newline/padding of a container; CPython writes nothing for empty ones.
fn push_container_suffix(
    out: &mut PythonString,
    options: PythonJsonRenderOptions,
    depth: usize,
    close: &str,
) {
    if let Some(width) = options.indent {
        out.push_scalars("\n");
        push_padding_into(out, width * depth);
    }
    out.push_scalars(close);
}

fn push_padding_into(out: &mut PythonString, count: usize) {
    for _ in 0..count {
        out.push_scalars(" ");
    }
}

/// CPython's `py_encode_basestring_ascii` over exact code points.
///
/// A lone surrogate is emitted as its own `\uXXXX` escape (the reference's
/// `json.dumps` does exactly that); a supplementary-plane code point becomes a
/// UTF-16 surrogate pair, matching `json.dumps("😀") == "\"\\ud83d\\ude00\""`.
pub fn push_python_string(out: &mut String, codepoints: &[u32]) {
    push_json_codepoints(out, codepoints.iter().copied());
}

/// CPython's `py_encode_basestring` over exact code points (`ensure_ascii=False`).
///
/// Only `"`, `\\`, the five short control escapes and the remaining code points
/// below `U+0020` are escaped; every other code point is kept, including a lone
/// surrogate. The reference's `json.dumps(..., ensure_ascii=False)` returns a
/// `str` that holds that unit; the encode step is a separate caller phase, so
/// this writer never substitutes U+FFFD and never refuses.
fn push_json_codepoints_utf8(out: &mut PythonString, codepoints: impl IntoIterator<Item = u32>) {
    out.push_scalars("\"");
    for code in codepoints {
        match code {
            0x22 => out.push_scalars("\\\""),
            0x5c => out.push_scalars("\\\\"),
            0x08 => out.push_scalars("\\b"),
            0x0c => out.push_scalars("\\f"),
            0x0a => out.push_scalars("\\n"),
            0x0d => out.push_scalars("\\r"),
            0x09 => out.push_scalars("\\t"),
            0x00..=0x1f => out.push_scalars(&format!("\\u{code:04x}")),
            other => out
                .push_codepoint(other)
                .expect("rendered code points are valid"),
        }
    }
    out.push_scalars("\"");
}

/// The one `ensure_ascii=True` encoder, shared by the scalar Op builder, the
/// scalar string helper and the exact document renderer.
fn push_json_codepoints<S: JsonCodepointSink>(
    out: &mut S,
    codepoints: impl IntoIterator<Item = u32>,
) {
    out.push_code(u32::from(b'"'));
    for code in codepoints {
        match code {
            0x22 => out.push_text("\\\""),
            0x5c => out.push_text("\\\\"),
            0x08 => out.push_text("\\b"),
            0x0c => out.push_text("\\f"),
            0x0a => out.push_text("\\n"),
            0x0d => out.push_text("\\r"),
            0x09 => out.push_text("\\t"),
            0x20..=0x7e => out.push_code(code),
            0x1_0000..=0x10_ffff => {
                let adjusted = code - 0x1_0000;
                let high = 0xd800 + (adjusted >> 10);
                let low = 0xdc00 + (adjusted & 0x3ff);
                out.push_text(&format!("\\u{high:04x}\\u{low:04x}"));
            }
            _ => out.push_text(&format!("\\u{code:04x}")),
        }
    }
    out.push_code(u32::from(b'"'));
}

#[cfg(test)]
mod python_json_value_tests {
    use super::*;

    #[test]
    fn rendering_matches_python_separators_and_escapes() {
        let value = PythonJsonValue::Object(vec![
            (
                "b".to_owned(),
                PythonJsonValue::Array(vec![
                    PythonJsonValue::Number("1".to_owned()),
                    PythonJsonValue::scalar("x"),
                ]),
            ),
            ("a".to_owned(), PythonJsonValue::Null),
        ]);
        assert_eq!(value.render(None), r#"{"b": [1, "x"], "a": null}"#);
        assert_eq!(
            value.render(Some(2)),
            "{\n  \"b\": [\n    1,\n    \"x\"\n  ],\n  \"a\": null\n}"
        );
        let astral = PythonString::from_codepoints(&[0x1f600]).unwrap();
        assert_eq!(
            PythonJsonValue::String(astral).render(None),
            r#""\ud83d\ude00""#
        );
        let surrogate = PythonString::from_codepoints(&[0xd800, 0x0]).unwrap();
        assert_eq!(
            PythonJsonValue::String(surrogate).render(None),
            r#""\ud800\u0000""#
        );
    }

    #[test]
    fn render_with_binds_both_ensure_ascii_modes() {
        let value = PythonJsonValue::Object(vec![
            (
                "k".to_owned(),
                PythonJsonValue::scalar("h\u{e9}llo \u{2603} \u{1}\t\"q\"/\\"),
            ),
            (
                "n".to_owned(),
                PythonJsonValue::Array(vec![
                    PythonJsonValue::Number("1".to_owned()),
                    PythonJsonValue::Null,
                ]),
            ),
        ]);
        assert_eq!(
            value.render_with(None, true).expect("ascii"),
            "{\"k\": \"h\\u00e9llo \\u2603 \\u0001\\t\\\"q\\\"/\\\\\", \"n\": [1, null]}"
        );
        assert_eq!(
            value.render_with(None, false).expect("utf8"),
            "{\"k\": \"h\u{e9}llo \u{2603} \\u0001\\t\\\"q\\\"/\\\\\", \"n\": [1, null]}"
        );
        assert_eq!(
            value.render_with(Some(2), false).expect("utf8 indent"),
            "{\n  \"k\": \"h\u{e9}llo \u{2603} \\u0001\\t\\\"q\\\"/\\\\\",\n  \"n\": [\n    1,\n    null\n  ]\n}"
        );
    }

    #[test]
    fn raw_utf8_rendering_reports_the_lone_surrogate_boundary_instead_of_faking_bytes() {
        let surrogate = PythonString::from_codepoints(&[0x61, 0xd800]).expect("python string");
        let value = PythonJsonValue::String(surrogate);
        assert_eq!(
            value.render_with(None, true).as_deref(),
            Some(r#""a\ud800""#)
        );
        assert_eq!(value.render_with(None, false), None);
    }
}
