//! Canonical Pydantic/model-JSON mapping-key projection.
//!
//! The reference builds an `--include` payload from a **validated Pydantic
//! model**: `model_dump(by_alias=True, mode="json", exclude_none=True)` and then
//! `_jsonable_artifact_value`. Inside an unstructured (`dict[str, Any]` /
//! `Any`) member, Pydantic's JSON serializer rewrites every mapping **key** into
//! a string — it does not leave a non-`str` key in place and (except for a
//! `frozenset`) it does not refuse one. This module owns that one key codec so
//! no consumer invents its own spelling.
//!
//! The rule, measured against the pinned interpreter (CPython 3.11.15 /
//! pydantic 2.13.3 / pydantic-core 2.46.3) rather than assumed from `str()` or
//! `json.dumps`:
//!
//! | Python key | JSON key text |
//! |---|---|
//! | `str` | the text itself, which must have a UTF-8 form |
//! | `None` | `None` |
//! | `bool` | `true` / `false` (JSON spelling, not Python's `True`/`False`) |
//! | `int` | its decimal digits (arbitrary precision) |
//! | `float` (finite) | `str(float)` ([`crate::python_json::python_float_repr`]) |
//! | `float` (NaN/±inf) | `None` — the JSON projection turns the scalar into `None` first |
//! | `bytes` | its strict UTF-8 decode (never a `b'…'` repr) |
//! | `date` | `YYYY-MM-DD` |
//! | `datetime` | [`crate::clock::model_json_datetime_text`] (`T` separator, `Z` for UTC) |
//! | `tuple` | its members' key texts joined with `,` (recursively, so a nested tuple flattens) |
//!
//! Two genuinely different objects, both CPython exceptions, are reported as
//! typed causes rather than a blanket "non-string key" refusal:
//!
//! * `bytes` that are not UTF-8 → `UnicodeDecodeError` on decode;
//! * text holding an unpaired surrogate → `UnicodeEncodeError`
//!   (`surrogates not allowed`) on encode — the asymmetry is real: the reference
//!   keeps such a surrogate in a **value** (`json.dumps` escapes it) but its key
//!   codec encodes to UTF-8 and refuses.
//!
//! A `frozenset` key makes Pydantic raise ``TypeError: `frozenset` not valid as
//! object key``; that is modelled by [`ModelJsonKeyError::NotAKey`] for
//! completeness, but a `frozenset` is not constructible from a YAML document, so
//! that branch is not reachable from the reference's own loader. Nothing here
//! guesses a key spelling: an object Python cannot key a mapping with is
//! refused with the loader's own `unhashable type: '<type>'` shape, which is the
//! phase that actually refused it.
//!
//! Typed mapping roots (`dict[str, str]`, `dict[str, Any]`) are **not** this
//! module's business: Pydantic *validates* those keys (`ValidationError` for a
//! non-`str` key) before any dump exists, so an admitted model never carries a
//! non-string key at a typed level. This projection is only for the
//! unstructured positions below such a root.

use std::borrow::Cow;

use serde_yaml_ng::Value;

use crate::PythonString;
use crate::clock::model_json_datetime_text;
use crate::python_json::python_float_repr;
use crate::python_utf8::{
    PythonUtf8DecodeError, PythonUtf8EncodeError, decode_utf8, encode_python_string,
};
use crate::python_yaml::{
    NodeRef, OMAP_TAG, Object, PAIRS_TAG, constructed_big_integer, constructed_bytes,
    constructed_set, constructed_timestamp_text, constructed_tuple, is_constructed_timestamp,
};

/// Why one Python object has no model-JSON key text.
///
/// Each variant is a real reference outcome with its own phase, never a generic
/// "unrepresentable" bucket.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModelJsonKeyError {
    /// `bytes` whose strict UTF-8 decode fails — CPython's `UnicodeDecodeError`.
    BytesNotUtf8(PythonUtf8DecodeError),
    /// Text with no UTF-8 form (an unpaired surrogate) — CPython's
    /// `UnicodeEncodeError` with `surrogates not allowed`.
    TextNotUtf8(PythonUtf8EncodeError),
    /// An object Python cannot use as a mapping key.
    ///
    /// The reference refuses it in the **loader's** constructor (`found
    /// unhashable key`, or `TypeError: unhashable type: '<type>'` for a member of
    /// a sequence key), before any model dump exists. `python_type_name` is the
    /// Python type name that refusal names.
    NotAKey {
        /// The Python type name (`list`, `dict`, `set`, …).
        python_type_name: &'static str,
    },
}

impl std::fmt::Display for ModelJsonKeyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BytesNotUtf8(error) => formatter.write_str(&model_json_bytes_error_text(error)),
            Self::TextNotUtf8(error) => error.fmt(formatter),
            Self::NotAKey { python_type_name } => {
                write!(formatter, "unhashable type: '{python_type_name}'")
            }
        }
    }
}

/// The reference's own `UnicodeDecodeError` text for `bytes` on the model-JSON path.
///
/// `mode="json"` decodes `bytes` (a value **or** a mapping key) with strict
/// `utf-8`, and pydantic-core reports the failure with its own fixed reason
/// `invalid utf-8` over the one offending byte. That is *not* CPython's
/// `bytes.decode` wording (`invalid start byte` / `unexpected end of data` /
/// `invalid continuation byte`), and the reference's include path propagates the
/// former. Measured on the pinned interpreter: `b"\xff"`, `b"\xc3"`,
/// `b"\xc3("`, `b"\x80"`, `b"ab\xff"`, `b"\xf0\x9f"` and `b"\xed\xa0\x80"`
/// all report `byte 0xNN in position <start>: invalid utf-8`, with the same
/// `position` [`PythonUtf8DecodeError::start`] yields. `error` is therefore the
/// one Kernel strict-decode authority's finding, presented the way the reference
/// presents it.
#[must_use]
pub fn model_json_bytes_error_text(error: &PythonUtf8DecodeError) -> String {
    let byte = error.object().get(error.start).copied().unwrap_or_default();
    format!(
        "'utf-8' codec can't decode byte 0x{byte:02x} in position {}: invalid utf-8",
        error.start
    )
}

impl std::error::Error for ModelJsonKeyError {}

/// One Python object's exact model-JSON mapping-key text.
///
/// This is the whole codec: `Ok` is the text Pydantic would emit, `Err` is the
/// typed reason the reference refuses. It is total over every graph object — an
/// object the loader would never accept as a key is refused, not spelled.
///
/// # Errors
///
/// [`ModelJsonKeyError::BytesNotUtf8`] / [`ModelJsonKeyError::TextNotUtf8`] when a
/// `bytes` or text member has no UTF-8 form, and
/// [`ModelJsonKeyError::NotAKey`] when the object is not a Python mapping key at
/// all (which the loader refuses first).
pub fn model_json_mapping_key(node: NodeRef<'_>) -> Result<PythonString, ModelJsonKeyError> {
    key_text(KeyShape::of_node(node))
}

/// The same codec over the graph-less `serde_yaml_ng` carrier.
///
/// A *constructed* tree is required: `!!tuple`/`!!binary`/big `!!int`/timestamp
/// shapes are what the construct phase produces. A bare `Value::Sequence` /
/// `Value::Mapping` is a Python `list` / `dict`, which cannot key a mapping.
///
/// # Errors
///
/// As [`model_json_mapping_key`].
pub fn model_json_mapping_key_value(value: &Value) -> Result<PythonString, ModelJsonKeyError> {
    key_text(KeyShape::of_value(value))
}

/// One key candidate, independent of the carrier it came from.
///
/// Deliberately small: it carries only what the key codec reads, so the two
/// carriers share one rule instead of two dispatch tables.
enum KeyShape<'a> {
    Null,
    Bool(bool),
    Integer(Cow<'a, str>),
    Float(f64),
    Text(Cow<'a, PythonString>),
    Bytes(Cow<'a, [u8]>),
    Date(Cow<'a, str>),
    DateTime(Cow<'a, str>),
    Tuple(Vec<KeyShape<'a>>),
    /// A Python object that cannot key a mapping (`list`, `dict`, `set`, …).
    NotAKey(&'static str),
}

impl<'a> KeyShape<'a> {
    fn of_node(node: NodeRef<'a>) -> Self {
        match node.object() {
            Object::Null => Self::Null,
            Object::Bool(flag) => Self::Bool(*flag),
            Object::Int(digits) => Self::Integer(Cow::Borrowed(digits)),
            Object::Float { value, .. } => Self::Float(*value),
            Object::Str(text) => Self::Text(Cow::Borrowed(text)),
            Object::Bytes(bytes) => Self::Bytes(Cow::Borrowed(bytes)),
            Object::Date(text) => Self::Date(Cow::Borrowed(text)),
            Object::DateTime(text) => Self::DateTime(Cow::Borrowed(text)),
            // A constructed tuple is a real Python `tuple` (ruamel's safe
            // constructor converts a list mapping key with `tuple(key)`), so its
            // members are projected in turn. The loader refuses an unhashable
            // member when the mapping hashes the key, so a `list` / `dict` / `set`
            // member is refused here.
            Object::Tuple(items) => Self::Tuple(
                items
                    .iter()
                    .map(|id| Self::of_node(node.member(*id)))
                    .collect(),
            ),
            other => Self::NotAKey(other.python_type_name()),
        }
    }

    fn of_value(value: &'a Value) -> Self {
        match value {
            Value::Null => Self::Null,
            Value::Bool(flag) => Self::Bool(*flag),
            Value::Number(number) => {
                if let Some(value) = number.as_i64() {
                    return Self::Integer(Cow::Owned(value.to_string()));
                }
                if let Some(value) = number.as_u64() {
                    return Self::Integer(Cow::Owned(value.to_string()));
                }
                Self::Float(number.as_f64().unwrap_or_default())
            }
            Value::String(text) => Self::Text(Cow::Owned(PythonString::from_scalars(text))),
            // A bare sequence/mapping in the constructed tree is a list/dict, which
            // Source never accepts as a key.
            Value::Sequence(_) => Self::NotAKey("list"),
            Value::Mapping(_) => Self::NotAKey("dict"),
            Value::Tagged(tagged) => {
                if let Some(digits) = constructed_big_integer(value) {
                    return Self::Integer(Cow::Borrowed(digits));
                }
                if let Some(bytes) = constructed_bytes(value) {
                    return Self::Bytes(Cow::Owned(bytes));
                }
                if is_constructed_timestamp(value) {
                    let Some(text) = constructed_timestamp_text(value) else {
                        return Self::NotAKey("datetime");
                    };
                    return if text.contains(' ') && text.contains(':') {
                        Self::DateTime(Cow::Owned(text))
                    } else {
                        Self::Date(Cow::Owned(text))
                    };
                }
                if let Some(items) = constructed_tuple(value) {
                    return Self::Tuple(items.iter().map(Self::of_value).collect());
                }
                if constructed_set(value).is_some() {
                    return Self::NotAKey("set");
                }
                if tagged.tag == OMAP_TAG {
                    return Self::NotAKey("ordereddict");
                }
                if tagged.tag == PAIRS_TAG {
                    return Self::NotAKey("list");
                }
                Self::of_value(&tagged.value)
            }
        }
    }
}

/// Render one key candidate exactly as the reference's JSON key codec does.
fn key_text(shape: KeyShape<'_>) -> Result<PythonString, ModelJsonKeyError> {
    match shape {
        KeyShape::Null => Ok(PythonString::from_scalars("None")),
        KeyShape::Bool(true) => Ok(PythonString::from_scalars("true")),
        KeyShape::Bool(false) => Ok(PythonString::from_scalars("false")),
        KeyShape::Integer(digits) => Ok(PythonString::from_scalars(digits.as_ref())),
        // `mode="json"` projects a non-finite float to `None` *before* the key is
        // spelled, so its key text is `None`; a finite float spells `str(float)`.
        KeyShape::Float(value) => {
            let text = if value.is_finite() {
                python_float_repr(value)
            } else {
                "None".to_owned()
            };
            Ok(PythonString::from_scalars(&text))
        }
        // A `str` key is the authored text, but the reference's codec encodes it to
        // UTF-8 first: an unpaired surrogate is a real `UnicodeEncodeError` here
        // even though the same surrogate survives as a *value*.
        KeyShape::Text(text) => {
            let text = text.into_owned();
            encode_python_string(&text).map_err(ModelJsonKeyError::TextNotUtf8)?;
            Ok(text)
        }
        KeyShape::Bytes(bytes) => Ok(PythonString::from_scalars(
            decode_utf8(bytes.as_ref()).map_err(ModelJsonKeyError::BytesNotUtf8)?,
        )),
        KeyShape::Date(text) => Ok(PythonString::from_scalars(text.as_ref())),
        // Pydantic's key presentation of a datetime is the same model-JSON form the
        // payload uses for it; Python's own `str()`/`isoformat` spelling stays a
        // separate contract.
        KeyShape::DateTime(text) => Ok(PythonString::from_scalars(&model_json_datetime_text(
            text.as_ref(),
        ))),
        KeyShape::Tuple(items) => {
            let mut text = PythonString::default();
            for (position, item) in items.into_iter().enumerate() {
                if position > 0 {
                    text.push_scalars(",");
                }
                text.append(&key_text(item)?);
            }
            Ok(text)
        }
        KeyShape::NotAKey(python_type_name) => Err(ModelJsonKeyError::NotAKey { python_type_name }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::python_yaml::{Dialect, GraphDocument, TUPLE_TAG, decode_document, decode_graph};
    use serde_yaml_ng::value::{Tag, TaggedValue};

    /// The single mapping key of one authored document, projected from the graph.
    fn key_of(text: &str) -> Result<PythonString, ModelJsonKeyError> {
        let graph = decode_graph(text, Dialect::RuamelSafe12).expect("document constructs");
        let document = GraphDocument::new(graph);
        let entries = document.root().as_mapping().expect("mapping root");
        assert_eq!(entries.len(), 1, "one entry: {entries:?}");
        model_json_mapping_key(document.root().member(entries[0].0))
    }

    /// The same, from the constructed bare tree (the graph-less carrier).
    fn key_of_tree(text: &str) -> Result<PythonString, ModelJsonKeyError> {
        let tree = decode_document(text, Dialect::RuamelSafe12).expect("document constructs");
        let entries = tree.as_mapping().expect("mapping root");
        assert_eq!(entries.len(), 1, "one entry");
        let (key, _) = entries.iter().next().expect("one entry");
        model_json_mapping_key_value(key)
    }

    /// The projected text of one authored scalar mapping key.
    fn text_of(key: &str) -> String {
        key_of(&format!("{{{key}: v}}"))
            .expect("key projects")
            .to_scalar_string()
            .expect("authored text is scalar")
    }

    fn text(value: Result<PythonString, ModelJsonKeyError>) -> String {
        value
            .expect("key projects")
            .to_scalar_string()
            .expect("scalar text")
    }

    /// One constructed `!!python/tuple` value, the shape a sequence key becomes.
    fn tuple_tree(members: Vec<Value>) -> Value {
        Value::Tagged(Box::new(TaggedValue {
            tag: Tag::new(TUPLE_TAG),
            value: Value::Sequence(members),
        }))
    }

    #[test]
    fn a_text_key_keeps_its_exact_code_points() {
        assert_eq!(text_of("plain"), "plain");
        assert_eq!(text_of("'caf\u{e9}'"), "caf\u{e9}");
        assert_eq!(text_of("'\u{1f600}'"), "\u{1f600}");
        assert_eq!(text_of("'a,b'"), "a,b");
        assert_eq!(text_of("\"\""), "");
        // NUL survives; it is not a separator or an escape.
        assert_eq!(text_of("\"a\\0b\""), "a\0b");
    }

    #[test]
    fn non_text_scalar_keys_take_pydantic_json_key_text() {
        // Every expected text below was measured on the pinned Source child
        // (CPython 3.11.15 / pydantic 2.13.3): `model_dump(mode="json")` over a
        // `dict[str, Any]` member.
        assert_eq!(text_of("1"), "1");
        assert_eq!(text_of("-7"), "-7");
        assert_eq!(
            text_of("123456789012345678901234567890"),
            "123456789012345678901234567890"
        );
        // JSON spelling, NOT Python's `True`/`False`.
        assert_eq!(text_of("true"), "true");
        assert_eq!(text_of("false"), "false");
        assert_eq!(text_of("~"), "None");
        assert_eq!(text_of("1.5"), "1.5");
        assert_eq!(text_of("1.0"), "1.0");
        assert_eq!(text_of("-0.0"), "-0.0");
        assert_eq!(text_of("1e20"), "1e+20");
        assert_eq!(text_of("1e-7"), "1e-07");
        // A non-finite float is projected to `None` BEFORE the key is spelled.
        assert_eq!(text_of(".nan"), "None");
        assert_eq!(text_of(".inf"), "None");
        assert_eq!(text_of("-.inf"), "None");
        // `bytes` decode to their text, never to a `b'…'` repr.
        assert_eq!(text(key_of("? !!binary eHk=\n: v\n")), "xy");
        assert_eq!(text(key_of("? !!binary YQBi\n: v\n")), "a\0b");
        // Dates and datetimes use the model-JSON presentation.
        assert_eq!(text_of("2020-01-02"), "2020-01-02");
        assert_eq!(text_of("0001-01-02"), "0001-01-02");
        assert_eq!(text_of("2020-01-02T03:04:05Z"), "2020-01-02T03:04:05Z");
        assert_eq!(
            text_of("2020-01-02T03:04:05+05:30"),
            "2020-01-02T03:04:05+05:30"
        );
        assert_eq!(text_of("2020-01-02T03:04:05"), "2020-01-02T03:04:05");
        assert_eq!(
            text_of("2020-01-02T03:04:05.000006"),
            "2020-01-02T03:04:05.000006"
        );
    }

    #[test]
    fn a_sequence_key_is_a_tuple_joined_with_commas() {
        // ruamel's safe constructor converts a list mapping key with `tuple(key)`,
        // so `? [1, 2]` is the Python key `(1, 2)`.
        assert_eq!(text(key_of("? [1, 2]\n: v\n")), "1,2");
        assert_eq!(text(key_of("? [a, b]\n: v\n")), "a,b");
        assert_eq!(text(key_of("? []\n: v\n")), "");
        // Members take the same codec, so a bool spells `true` and null `None`.
        assert_eq!(text(key_of("? [1, a, null, true]\n: v\n")), "1,a,None,true");
        // The same rule over the bare constructed carrier: a nested tuple flattens
        // (the join of joins), exactly like the reference's recursive key codec.
        assert_eq!(
            text(model_json_mapping_key_value(&tuple_tree(vec![
                tuple_tree(vec![Value::from(1), Value::from(2)]),
                Value::from(3),
            ]))),
            "1,2,3"
        );
        // A *list* member inside a sequence key is unhashable, and the reference's
        // loader refuses it before any model dump exists.
        let error = decode_graph("? [[1, 2], 3]\n: v\n", Dialect::RuamelSafe12)
            .expect_err("a list member is unhashable");
        assert!(
            error.to_string().contains("unhashable type: 'list'"),
            "{error}"
        );
    }

    #[test]
    fn a_non_utf8_binary_key_is_cpythons_decode_error() {
        let error = key_of("? !!binary /w==\n: v\n").expect_err("not UTF-8");
        assert!(
            matches!(&error, ModelJsonKeyError::BytesNotUtf8(inner) if inner.start == 0),
            "{error:?}"
        );
        assert_eq!(
            error.to_string(),
            "'utf-8' codec can't decode byte 0xff in position 0: invalid utf-8"
        );
    }

    #[test]
    fn an_unpaired_surrogate_key_is_cpythons_encode_error() {
        // The same surrogate survives as a *value*; only the key codec encodes it.
        // Double-quoted YAML is the escape form: the transport marker restores the
        // authored surrogate unit into the canonical graph.
        let error = key_of("{\"\\ud800\": v}").expect_err("no UTF-8 form");
        assert!(
            matches!(error, ModelJsonKeyError::TextNotUtf8(_)),
            "{error:?}"
        );
        assert_eq!(
            error.to_string(),
            "'utf-8' codec can't encode character '\\ud800' in position 0: surrogates not allowed"
        );
    }

    #[test]
    fn unhashable_objects_are_refused_not_spelled() {
        let mapping = Value::Mapping(
            [(Value::Sequence(vec![Value::from(1)]), Value::from("v"))]
                .into_iter()
                .collect(),
        );
        let entries = mapping.as_mapping().expect("mapping");
        let (key, _) = entries.iter().next().expect("one entry");
        assert_eq!(
            model_json_mapping_key_value(key).expect_err("list key"),
            ModelJsonKeyError::NotAKey {
                python_type_name: "list"
            }
        );
    }

    #[test]
    fn the_bare_tree_carrier_projects_the_same_key_texts() {
        for (document, expected) in [
            ("{~: v}\n", "None"),
            ("{true: v}\n", "true"),
            ("{false: v}\n", "false"),
            ("{1: v}\n", "1"),
            ("{-7: v}\n", "-7"),
            ("{1.5: v}\n", "1.5"),
            ("{-0.0: v}\n", "-0.0"),
            ("{1e20: v}\n", "1e+20"),
            ("{plain: v}\n", "plain"),
            ("? !!binary eHk=\n: v\n", "xy"),
            ("? [a, 1, null]\n: v\n", "a,1,None"),
            ("{2020-01-02: v}\n", "2020-01-02"),
            ("{2020-01-02T03:04:05Z: v}\n", "2020-01-02T03:04:05Z"),
        ] {
            assert_eq!(text(key_of_tree(document)), expected, "{document:?}");
        }
        // The bare tree refuses a marker string and a non-UTF-8 binary the same way
        // the graph does (the caller owns the marker check; the codec owns bytes).
        let error = key_of_tree("? !!binary /w==\n: v\n").expect_err("not UTF-8");
        assert!(
            matches!(error, ModelJsonKeyError::BytesNotUtf8(_)),
            "{error:?}"
        );
    }
}
