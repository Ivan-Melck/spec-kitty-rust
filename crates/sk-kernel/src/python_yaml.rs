//! The workspace's one Python-safe YAML reader.
//!
//! Source reads authored YAML with two *different* safe loaders, and both are
//! reproduced here from one syntax/transport/construction mechanism with an
//! explicit [`Dialect`]:
//!
//! * [`Dialect::PyYamlSafe11`] — `yaml.safe_load` (PyYAML 6.0.3, YAML 1.1), the
//!   loader for `mission.yaml`;
//! * [`Dialect::RuamelSafe12`] — `ruamel.yaml.YAML(typ="safe")` (0.19.1, YAML
//!   1.2), the loader for profiles, offerings, `project.path_conventions` and
//!   activation config.
//!
//! There is deliberately **no** blanket "fidelity" switch: a ruamel reader can
//! not stand in for a Mission's PyYAML gate (it accepts `1e5`/`0o17` as numbers,
//! keeps `yes` a string and refuses duplicate keys), and the PyYAML policy must
//! not be imposed on profile artifacts. The caller states its dialect.
//!
//! # Pipeline (one mechanism, dialect policy)
//!
//! 1. **Read** ([`read_document_text`]) — Source's own byte/encoding policy:
//!    PyYAML receives a *text* stream (`open()` decoded it strictly as UTF-8 and
//!    translated universal newlines), ruamel's path loader sniffs a UTF-16/UTF-8
//!    BOM itself. Both strip exactly one leading `U+FEFF` BOM code point, and
//!    both refuse a NUL code point with `ReaderError`.
//! 2. **Transport** ([`transport`]) — the authored text is parsed by the
//!    lossless CST, lossy (surrogate-bearing) double-quoted scalars are encoded
//!    injectively into markers, genuine merge keys are marked, and every plain
//!    or core-tagged scalar is re-spelled with the dialect resolver's tag so the
//!    semantic reader cannot silently apply a *third* dialect.
//! 3. **Semantic parse** — the marked text is parsed once by `serde_yaml_ng` for
//!    structure, anchors/aliases, keys and numbers.
//! 4. **Merge** ([`resolve_merge_keys`]) and **construct** ([`construct`]) — the
//!    dialect's safe-constructor rules, including its duplicate-key and
//!    unhashable-member policy.
//! 5. **Value** ([`decode_text`]) — the constructed tree converted to the
//!    canonical [`PythonValue`] domain.
//!
//! # Known open cases (named, not hidden)
//!
//! A decoder that reproduces the exercised constructions can still be unfaithful
//! elsewhere, so the unclosed Source cases are a typed list ([`OPEN_CASES`])
//! rather than a boolean. They are:
//!
//! * **Recursive aliases.** Both loaders build a genuinely cyclic object
//!   (`a: &x [*x]` → `{'a': [[...]]}`, probed for PyYAML *and* ruamel);
//!   `serde_yaml_ng` cannot represent a cycle and reports a repetition/recursion
//!   limit error instead, so a Source-valid cyclic document is refused here.
//! * **Source's caret/mark prose.** Scanner/parser/constructor messages carry
//!   the class and the semantic reason, not libyaml's line/column caret block.
//! * **Constructed-object identity.** Explicit `!!float nan` creates a fresh
//!   float, unlike the loaders' `.nan` singleton. This tree currently conflates
//!   those objects, collapsing distinct PyYAML keys or falsely refusing ruamel
//!   input. None/NaN also share a pointer-hash stand-in, not distinct modeled
//!   objects. A canonical graph/identity construction is still required.
//! * **`!!python/*` and other non-core tags** are the reference's constructor
//!   error for both dialects (not constructed), which is asserted, not assumed.
//! * **Unpinned `PYTHONHASHSEED`.** A `!!set`'s iteration order is a property of
//!   the reference *process*; only a seeded run has one observable answer.
//! * **The reference's recursion budget.** Source's nesting ceiling is the
//!   interpreter's remaining recursion budget (measured: 200 nested lists load,
//!   500 raise `RecursionError`), while this reader refuses above 128 (transport)
//!   and 256 (semantic parser) — so a Source-valid document nested 129..~492
//!   levels is falsely refused here.
//!
//! Parent Source-first controls close the former timestamp-hash/pairs-key
//! cases using packed/timedelta hashing and genuine tuple pair members; that
//! does not close graph/identity/depth/probing/diagnostic domains.

mod construct;
mod dialect;
mod error;
mod graph;
mod set_order;
mod transport;
pub mod view;

use serde_yaml_ng::Value;

pub use construct::{
    BINARY_TAG, INTEGER_TAG, OMAP_TAG, PAIRS_TAG, SET_TAG, TIMESTAMP_TAG, TUPLE_TAG,
    TimestampIdentity, big_integer_value, construct, construct_scalar, constructed_big_integer,
    constructed_bytes, constructed_mapping, constructed_omap, constructed_pairs,
    constructed_sequence, constructed_set, constructed_timestamp_text, constructed_tuple,
    decode_base64_bytes, is_constructed_timestamp, python_timestamp_text, timestamp_identity,
    tuple_value, untag_constructed,
};
pub use dialect::{Dialect, implicit_scalar_kind, is_timestamp_resolver};
pub use error::{ErrorClass, RuntimeCause, YamlError};
pub use graph::{
    ALIAS_TAG_PREFIX, ANCHOR_NULL_TAG_PREFIX, ANCHOR_TAG_PREFIX, FRESH_NAN_TAG_PREFIX,
    FloatProvenance, NodeId, Object, ObjectGraph, graph_tag_name, is_fresh_nan_tag, is_graph_tag,
};
pub use set_order::{
    HASH_SEED_FATAL, HashSeedError, python_repr_text, python_value_equal, set_order,
    set_order_with_secret, string_set_order, string_set_order_with_secret,
    validate_hash_seed_from_env,
};
pub use transport::{
    LosslessText, decode_double_quoted, is_marker_text, marker_for_exact, resolve_merge_keys,
    splice_identity_tags, transport, verify_markers,
};
pub use view::{GraphDocument, NodeRef, build_from_value, build_from_value_with_markers};

use crate::python_value::PythonValue;

/// One Source construction this reader does **not** reproduce, with the shape of
/// the divergence. Named so a caller can surface the gap instead of inheriting a
/// silent claim of parity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OpenCase {
    /// Stable identifier for the case.
    pub id: &'static str,
    /// One-line statement of what differs, and in which dialect.
    pub detail: &'static str,
}

/// The unclosed Source cases of this reader (see the module docs).
pub const OPEN_CASES: &[OpenCase] = &[
    OpenCase {
        id: "recursive-alias",
        detail: "the canonical object graph ([`decode_graph`] + [`NodeRef`] views) DOES \
                 construct the real cycle PyYAML and ruamel build for `a: &x [*x]` \
                 ({'a': [[...]]}), and the Source-compatible Mission/Doctrine readers \
                 now read that graph, so a cycle in a field Source ignores is accepted. \
                 The legacy whole-root tree projection ([`decode_text`]) still refuses a \
                 cycle by design; that is a remaining projection-API gap, not a \
                 consumer refusal",
    },
    OpenCase {
        id: "mark-prose",
        detail: "scanner/parser/constructor messages carry class and semantic reason, \
                 not the libyaml caret/line/column block",
    },
    OpenCase {
        id: "collection-key-duplicates",
        detail: "the graph now keeps distinct explicit `!!float nan` objects as distinct \
                 keys, collapses `.nan` through one process singleton, and hashes `None`/\
                 `.nan` from real process-level boxed objects (not a static stand-in). \
                 The remaining gap is the legacy tree projection's own set-position \
                 hashing, plus any document whose *transport* refuses before the graph \
                 is built",
    },
    OpenCase {
        id: "unpinned-hash-seed",
        detail: "a `!!set` iteration order is a property of the reference process; only \
                 a run with a pinned PYTHONHASHSEED has one observable answer",
    },
    OpenCase {
        id: "deep-recursion",
        detail: "Source's ceiling is the interpreter's remaining recursion budget at call \
                 time (measured: 200 nested lists load; 500 raise RecursionError \
                 'maximum recursion depth exceeded'; a bisect put the boundary at 492-494 \
                 in one frame and the same depth then loaded in a shallower one), while the \
                 parser dependencies refuse above 128 (serde_yaml_ng transport) and 256 \
                 (semantic parser). A Source-valid document \
                 nested 129..~492 levels is therefore FALSELY REFUSED here: this is a real \
                 false refusal owned by the parser dependency interface, not a house-style \
                 limit and not a declared cap",
    },
];

/// The outcome of reading one document plus the exact string markers it carries.
#[derive(Clone, Debug)]
pub struct DecodedDocument {
    /// The constructed value tree in the canonical Python value domain.
    pub value: PythonValue,
    /// The dialect that produced it.
    pub dialect: Dialect,
}

/// Decode one authored **text** document into the canonical object graph.
///
/// This is the construction/identity/hash/text authority for the
/// Source-compatible loader. It runs the same transport, dialect resolver, tag
/// constructors, merge-key flattening and duplicate policy as [`decode_text`];
/// the only addition is that anchor/alias sites and fresh-`nan` objects are
/// marked in the marked text, so sharing and cycles survive construction
/// instead of being expanded or refused by the semantic tree. [`decode_text`]
/// remains the acyclic projection those typed consumers still read.
pub fn decode_graph(text: &str, dialect: Dialect) -> Result<ObjectGraph, YamlError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let lossless = transport(text, dialect)?;
    let spliced = splice_identity_tags(lossless.shadow())?;
    let mut value: Value =
        serde_yaml_ng::from_str(&spliced).map_err(|error| YamlError::Syntax(error.to_string()))?;
    verify_markers(&value, lossless.markers())?;
    resolve_merge_keys(&mut value, dialect)?;
    construct::construct(&mut value, lossless.markers(), dialect)?;
    resolve_duplicate_keys(&mut value, &lossless, dialect)?;
    graph::build(&value, lossless.markers(), dialect)
}

/// Decode one authored **text** document into the canonical Python value domain.
///
/// `text` must be the code points the loader's scanner would see (see
/// [`read_document_text`] for the byte-level policy). A leading BOM is stripped.
pub fn decode_text(text: &str, dialect: Dialect) -> Result<DecodedDocument, YamlError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let lossless = transport(text, dialect)?;
    let mut value: Value = serde_yaml_ng::from_str(lossless.shadow())
        .map_err(|error| YamlError::Syntax(error.to_string()))?;
    verify_markers(&value, lossless.markers())?;
    resolve_merge_keys(&mut value, dialect)?;
    construct::construct(&mut value, lossless.markers(), dialect)?;
    resolve_duplicate_keys(&mut value, &lossless, dialect)?;
    let value = to_python_value(&value, lossless.markers());
    Ok(DecodedDocument { value, dialect })
}

/// Decode one authored document into a constructed `serde_yaml_ng` tree.
///
/// This is the semantic tree the model-facing consumers (profiles, offerings)
/// already read; it carries the constructed-kind tags, so a caller that only
/// wants model values must call [`untag_constructed`].
pub fn decode_document(text: &str, dialect: Dialect) -> Result<Value, YamlError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let lossless = transport(text, dialect)?;
    let mut value: Value = serde_yaml_ng::from_str(lossless.shadow())
        .map_err(|error| YamlError::Syntax(error.to_string()))?;
    verify_markers(&value, lossless.markers())?;
    resolve_merge_keys(&mut value, dialect)?;
    construct::construct(&mut value, lossless.markers(), dialect)?;
    resolve_duplicate_keys(&mut value, &lossless, dialect)?;
    Ok(value)
}

/// Collapse the duplicate keys PyYAML accepts but the semantic reader refuses.
///
/// The transport renamed every repeated scalar key aside to a registered marker;
/// here each marker is replaced by the *first* key's identity with the *last*
/// value, which is `dict[key] = value` over the authored order (PyYAML's
/// "last value wins, first position kept"). A `!!set`'s members are collapsed
/// the same way and the CPython table order is recomputed for the final set.
fn resolve_duplicate_keys(
    value: &mut Value,
    lossless: &LosslessText,
    dialect: Dialect,
) -> Result<(), YamlError> {
    if lossless.duplicate_keys().is_empty() {
        return Ok(());
    }
    collapse_duplicates(
        value,
        lossless.duplicate_keys(),
        lossless.markers(),
        dialect,
    )
}

fn collapse_duplicates(
    value: &mut Value,
    duplicates: &std::collections::BTreeMap<String, (&'static str, String)>,
    markers: &std::collections::BTreeMap<String, crate::PythonString>,
    dialect: Dialect,
) -> Result<(), YamlError> {
    match value {
        Value::Mapping(mapping) => {
            let entries: Vec<(Value, Value)> = mapping
                .iter()
                .map(|(key, item)| (key.clone(), item.clone()))
                .collect();
            let mut resolved: Vec<(Value, Value)> = Vec::with_capacity(entries.len());
            for (mut key, mut item) in entries {
                collapse_duplicates(&mut key, duplicates, markers, dialect)?;
                collapse_duplicates(&mut item, duplicates, markers, dialect)?;
                if let Some((kind, payload)) = renamed_duplicate(&key, duplicates) {
                    let original =
                        construct::construct_scalar(kind, payload, dialect).ok_or_else(|| {
                            YamlError::Syntax(
                                "a renamed duplicate key cannot be reconstructed".to_owned(),
                            )
                        })?;
                    match resolved
                        .iter()
                        .position(|(existing, _)| python_value_equal(existing, &original, markers))
                    {
                        Some(position) => resolved[position].1 = item,
                        None => resolved.push((original, item)),
                    }
                    continue;
                }
                resolved.push((key, item));
            }
            let mut rebuilt = serde_yaml_ng::Mapping::new();
            for (key, item) in resolved {
                rebuilt.insert(key, item);
            }
            *mapping = rebuilt;
            Ok(())
        }
        Value::Sequence(items) => {
            for item in items {
                collapse_duplicates(item, duplicates, markers, dialect)?;
            }
            Ok(())
        }
        Value::Tagged(tagged) => {
            if tagged.tag == serde_yaml_ng::value::Tag::new(construct::SET_TAG) {
                let Value::Sequence(items) = &mut tagged.value else {
                    return Ok(());
                };
                let mut members: Vec<Value> = Vec::with_capacity(items.len());
                for item in items.drain(..) {
                    let member = match renamed_duplicate(&item, duplicates) {
                        Some((kind, payload)) => construct::construct_scalar(
                            kind, payload, dialect,
                        )
                        .ok_or_else(|| {
                            YamlError::Syntax(
                                "a renamed duplicate key cannot be reconstructed".to_owned(),
                            )
                        })?,
                        None => item,
                    };
                    if members
                        .iter()
                        .any(|existing| python_value_equal(existing, &member, markers))
                    {
                        continue;
                    }
                    members.push(member);
                }
                *items = set_order::set_order(members, markers, dialect)?;
                return Ok(());
            }
            collapse_duplicates(&mut tagged.value, duplicates, markers, dialect)
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => Ok(()),
    }
}

/// The resolved identity of a renamed duplicate key marker.
fn renamed_duplicate<'a>(
    key: &Value,
    duplicates: &'a std::collections::BTreeMap<String, (&'static str, String)>,
) -> Option<(&'static str, &'a str)> {
    let Value::String(text) = key else {
        return None;
    };
    duplicates
        .get(text)
        .map(|(kind, payload)| (*kind, payload.as_str()))
}

/// Read authored **bytes** exactly as the dialect's loader does, then decode.
///
/// * [`Dialect::PyYamlSafe11`]: strict UTF-8 (CPython `open()`'s default codec),
///   then TextIO universal-newline translation — the text `yaml.safe_load(file)`
///   receives. An undecodable byte is CPython's `UnicodeDecodeError` (a
///   `ValueError`, raised before YAML sees the data).
/// * [`Dialect::RuamelSafe12`]: BOM-driven decoding exactly like
///   `YAML(typ="safe").load(path)` — UTF-16LE/BE per its BOM, UTF-8 per its BOM
///   or by default; an invalid sequence is the reference's `ReaderError`
///   (`unacceptable character`), which is a `YAMLError`.
///
/// Both then strip one leading BOM code point and refuse a NUL character with
/// the reference's `ReaderError`.
pub fn decode_bytes(bytes: &[u8], dialect: Dialect) -> Result<DecodedDocument, YamlError> {
    let text = read_document_text(bytes, dialect)?;
    decode_text(&text, dialect)
}

/// The exact code points a dialect's scanner sees for these authored bytes.
pub fn read_document_text(bytes: &[u8], dialect: Dialect) -> Result<String, YamlError> {
    let text = match dialect {
        Dialect::PyYamlSafe11 => {
            let decoded =
                crate::python_utf8::decode_utf8(bytes).map_err(|error| YamlError::Runtime {
                    cause: RuntimeCause::Value,
                    message: error.to_string(),
                })?;
            // TextIO with `newline=None`: `\r\n` and a lone `\r` become `\n`.
            crate::python_text::universal_newlines(decoded).into_owned()
        }
        Dialect::RuamelSafe12 => decode_ruamel_path_bytes(bytes)?,
    };
    // Both readers strip exactly one leading BOM code point.
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text).to_owned();
    // PyYAML's Reader refuses a NUL character; so does ruamel's.
    if let Some(position) = text.chars().position(|character| character == '\u{0}') {
        return Err(YamlError::Syntax(format!(
            "unacceptable character #x0000: special characters are not allowed (position {position})"
        )));
    }
    Ok(text)
}

/// `ruamel.yaml`'s path loader: BOM-sniffing decode of the authored bytes.
fn decode_ruamel_path_bytes(bytes: &[u8]) -> Result<String, YamlError> {
    if let Some(rest) = bytes.strip_prefix(&[0xff, 0xfe]) {
        return decode_utf16(rest, true);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xfe, 0xff]) {
        return decode_utf16(rest, false);
    }
    let rest = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    match crate::python_utf8::decode_utf8(rest) {
        Ok(text) => Ok(text.to_owned()),
        Err(error) => Err(YamlError::Syntax(format!(
            "unacceptable character #x{:04x}: {}",
            error.object().get(error.start).copied().unwrap_or_default(),
            error.reason
        ))),
    }
}

/// Decode UTF-16 code units (ruamel's BOM-selected decoder).
fn decode_utf16(bytes: &[u8], little_endian: bool) -> Result<String, YamlError> {
    if !bytes.len().is_multiple_of(2) {
        return Err(YamlError::Syntax(
            "unacceptable character: truncated UTF-16 byte sequence".to_owned(),
        ));
    }
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            if little_endian {
                u16::from_le_bytes([pair[0], pair[1]])
            } else {
                u16::from_be_bytes([pair[0], pair[1]])
            }
        })
        .collect();
    String::from_utf16(&units)
        .map_err(|_| YamlError::Syntax("unacceptable character: invalid UTF-16 data".to_owned()))
}

/// Convert a constructed tree into the canonical Python value domain.
fn to_python_value(
    value: &Value,
    markers: &std::collections::BTreeMap<String, crate::PythonString>,
) -> PythonValue {
    // A `!!float nan` is a fresh float object: the value domain sees a `float`
    // (its identity is a graph concern, and the tree only needs the value).
    if let Value::Tagged(tagged) = value
        && is_fresh_nan_tag(&tagged.tag)
    {
        return PythonValue::Float(f64::NAN);
    }
    if let Some(bytes) = constructed_bytes(value) {
        return PythonValue::Bytes(bytes);
    }
    if let Some(digits) = constructed_big_integer(value) {
        return PythonValue::Int(digits.to_owned());
    }
    if is_constructed_timestamp(value) {
        if let Value::Tagged(tagged) = value
            && let Some(text) = tagged.value.as_str()
            && let Some((is_datetime, rendered)) = python_timestamp_text(text)
        {
            return if is_datetime {
                PythonValue::DateTime(rendered)
            } else {
                PythonValue::Date(rendered)
            };
        }
        return PythonValue::Str(crate::PythonString::from_scalars(""));
    }
    if let Some(keys) = constructed_set(value) {
        return PythonValue::Set(
            keys.iter()
                .map(|key| to_python_value(key, markers))
                .collect(),
        );
    }
    if let Some(items) = constructed_tuple(value) {
        return PythonValue::Tuple(
            items
                .iter()
                .map(|item| to_python_value(item, markers))
                .collect(),
        );
    }
    if let Some(entries) = constructed_omap(value) {
        return PythonValue::OrderedDict(
            entries
                .iter()
                .map(|(key, item)| {
                    (
                        to_python_value(key, markers),
                        to_python_value(item, markers),
                    )
                })
                .collect(),
        );
    }
    if let Some(pairs) = constructed_pairs(value) {
        return PythonValue::Pairs(
            pairs
                .iter()
                .filter_map(|pair| {
                    let pair = pair.as_sequence()?;
                    Some((
                        to_python_value(pair.first()?, markers),
                        to_python_value(pair.get(1)?, markers),
                    ))
                })
                .collect(),
        );
    }
    match value {
        Value::Null => PythonValue::Null,
        Value::Bool(flag) => PythonValue::Bool(*flag),
        Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                PythonValue::Int(integer.to_string())
            } else if let Some(unsigned) = number.as_u64() {
                PythonValue::Int(unsigned.to_string())
            } else {
                PythonValue::Float(number.as_f64().unwrap_or(f64::NAN))
            }
        }
        Value::String(text) => PythonValue::Str(
            markers
                .get(text)
                .cloned()
                .unwrap_or_else(|| crate::PythonString::from_scalars(text)),
        ),
        Value::Sequence(items) => PythonValue::Seq(
            items
                .iter()
                .map(|item| to_python_value(item, markers))
                .collect(),
        ),
        Value::Mapping(mapping) => PythonValue::Map(
            mapping
                .iter()
                .map(|(key, item)| {
                    (
                        to_python_value(key, markers),
                        to_python_value(item, markers),
                    )
                })
                .collect(),
        ),
        Value::Tagged(tagged) => to_python_value(&tagged.value, markers),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pyyaml(text: &str) -> Result<PythonValue, YamlError> {
        decode_text(text, Dialect::PyYamlSafe11).map(|decoded| decoded.value)
    }

    fn ruamel(text: &str) -> Result<PythonValue, YamlError> {
        decode_text(text, Dialect::RuamelSafe12).map(|decoded| decoded.value)
    }

    fn entry<'a>(value: &'a PythonValue, key: &str) -> &'a PythonValue {
        value
            .mapping_entry(key)
            .unwrap_or_else(|| panic!("missing key {key} in {value:?}"))
    }

    #[test]
    fn dialect_resolution_differs_where_source_differs() {
        // YAML 1.1 booleans and the octal/sexagesimal grammars.
        assert_eq!(
            entry(&pyyaml("name: yes\n").unwrap(), "name"),
            &PythonValue::Bool(true)
        );
        assert_eq!(
            entry(&ruamel("name: yes\n").unwrap(), "name"),
            &PythonValue::Str(crate::PythonString::from_scalars("yes"))
        );
        assert_eq!(
            entry(&pyyaml("v: 017\n").unwrap(), "v"),
            &PythonValue::Int("15".to_owned())
        );
        assert_eq!(
            entry(&ruamel("v: 017\n").unwrap(), "v"),
            &PythonValue::Int("17".to_owned())
        );
        assert_eq!(
            entry(&ruamel("v: 0o17\n").unwrap(), "v"),
            &PythonValue::Int("15".to_owned())
        );
        assert_eq!(
            entry(&pyyaml("v: 1:30\n").unwrap(), "v"),
            &PythonValue::Int("90".to_owned())
        );
        assert_eq!(
            entry(&ruamel("v: 1:30\n").unwrap(), "v"),
            &PythonValue::Str(crate::PythonString::from_scalars("1:30"))
        );
        // `1e5` is a string in 1.1 and a float in 1.2.
        assert_eq!(
            entry(&pyyaml("v: 1e5\n").unwrap(), "v"),
            &PythonValue::Str(crate::PythonString::from_scalars("1e5"))
        );
        assert_eq!(
            entry(&ruamel("v: 1e5\n").unwrap(), "v"),
            &PythonValue::Float(100_000.0)
        );
    }

    #[test]
    fn implicit_timestamps_are_dates_and_datetimes() {
        assert_eq!(
            entry(&pyyaml("v: 2026-11-05\n").unwrap(), "v"),
            &PythonValue::Date("2026-11-05".to_owned())
        );
        assert_eq!(
            entry(&ruamel("v: 2026-11-05\n").unwrap(), "v"),
            &PythonValue::Date("2026-11-05".to_owned())
        );
        assert_eq!(
            entry(&pyyaml("v: 2026-11-05T01:02:03Z\n").unwrap(), "v"),
            &PythonValue::DateTime("2026-11-05 01:02:03+00:00".to_owned())
        );
        assert_eq!(
            entry(&pyyaml("v: 2001-12-15 2:59:43.1 -5\n").unwrap(), "v"),
            &PythonValue::DateTime("2001-12-15 02:59:43.100000-05:00".to_owned())
        );
        assert_eq!(
            entry(&ruamel("v: 2001-12-15T02:59:43.10-05:00\n").unwrap(), "v"),
            &PythonValue::DateTime("2001-12-15 02:59:43.100000-05:00".to_owned())
        );
    }

    #[test]
    fn duplicate_keys_follow_each_dialect() {
        // PyYAML keeps the last value in the first key position.
        let value = pyyaml("a: 1\nb: 2\na: 3\n").unwrap();
        let PythonValue::Map(entries) = &value else {
            panic!("expected a mapping")
        };
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].1, PythonValue::Int("3".to_owned()));
        // ruamel refuses it (DuplicateKeyError is a YAMLError).
        let error = ruamel("a: 1\nb: 2\na: 3\n").unwrap_err();
        assert_eq!(error.class(), ErrorClass::Yaml);
        assert!(matches!(error, YamlError::Construct(_)));
    }

    #[test]
    fn merge_keys_and_aliases_resolve() {
        let value = pyyaml("base: &b {x: 1, y: 2}\nv:\n  <<: *b\n  y: 3\n").unwrap();
        let merged = entry(&value, "v");
        assert_eq!(entry(merged, "x"), &PythonValue::Int("1".to_owned()));
        assert_eq!(entry(merged, "y"), &PythonValue::Int("3".to_owned()));
        // A sequence merge is applied in reverse, so the first element wins.
        let value = pyyaml("a: &a {x: 1}\nb: &b {y: 2}\nv: {<<: [*a, *b]}\n").unwrap();
        let entries = entry(&value, "v").as_mapping().unwrap().to_vec();
        assert_eq!(
            entries[0].0,
            PythonValue::Str(crate::PythonString::from_scalars("y"))
        );
        assert_eq!(
            entries[1].0,
            PythonValue::Str(crate::PythonString::from_scalars("x"))
        );
    }

    #[test]
    fn runtime_and_yaml_error_classes_are_kept_apart() {
        // A bare `KeyError` from `!!bool` aborts the load in both dialects.
        assert_eq!(
            pyyaml("v: !!bool notabool\n").unwrap_err().class(),
            ErrorClass::Runtime
        );
        // An impossible date is a `ValueError`.
        assert_eq!(
            pyyaml("v: 2026-13-05\n").unwrap_err().class(),
            ErrorClass::Runtime
        );
        // An unknown tag is the reference's `ConstructorError`.
        assert_eq!(
            ruamel("v: !custom hello\n").unwrap_err().class(),
            ErrorClass::Yaml
        );
        // PyYAML reports an unhashable `!!set` member as a ConstructorError...
        assert_eq!(
            pyyaml("v: !!set {? [a], b: null}\n").unwrap_err().class(),
            ErrorClass::Yaml
        );
        // ...while ruamel's safe constructor converts the list key to a real
        // tuple and constructs the set (Source-equal; this used to be recorded
        // as a declared divergence and is now compared against Source).
        let value = ruamel("v: !!set {? [a], b: null}\n").unwrap();
        let PythonValue::Set(members) = entry(&value, "v") else {
            panic!("expected a set")
        };
        assert!(
            members
                .iter()
                .any(|member| matches!(member, PythonValue::Tuple(items) if items.len() == 1))
        );
        // A member that is still unhashable after that conversion is the bare
        // `TypeError` the reference does not catch.
        assert_eq!(
            ruamel("v: !!set {? [[a]], b: null}\n").unwrap_err().class(),
            ErrorClass::Runtime
        );
    }

    #[test]
    fn bytes_and_bom_policy_follow_each_loader() {
        // PyYAML gets UTF-8 text; a UTF-16 BOM is an undecodable byte for it.
        let mut utf16 = vec![0xff, 0xfe];
        for unit in "a: 1\n".encode_utf16() {
            utf16.extend_from_slice(&unit.to_le_bytes());
        }
        let error = decode_bytes(&utf16, Dialect::PyYamlSafe11).unwrap_err();
        assert_eq!(error.class(), ErrorClass::Runtime);
        // ruamel's path loader decodes it.
        let value = decode_bytes(&utf16, Dialect::RuamelSafe12).unwrap();
        assert_eq!(entry(&value.value, "a"), &PythonValue::Int("1".to_owned()));
        // UTF-8 BOM is stripped by both.
        let bom = b"\xef\xbb\xbfa: 1\n";
        for dialect in [Dialect::PyYamlSafe11, Dialect::RuamelSafe12] {
            let value = decode_bytes(bom, dialect).unwrap();
            assert_eq!(entry(&value.value, "a"), &PythonValue::Int("1".to_owned()));
        }
        // A NUL character is a ReaderError (a YAMLError) for both.
        for dialect in [Dialect::PyYamlSafe11, Dialect::RuamelSafe12] {
            let error = decode_bytes(b"a: \x00\n", dialect).unwrap_err();
            assert_eq!(error.class(), ErrorClass::Yaml);
        }
        // CRLF is a line break for both.
        for dialect in [Dialect::PyYamlSafe11, Dialect::RuamelSafe12] {
            let value = decode_bytes(b"a: 1\r\nb: 2\r\n", dialect).unwrap();
            assert_eq!(entry(&value.value, "b"), &PythonValue::Int("2".to_owned()));
        }
    }

    #[test]
    fn constructed_kinds_are_distinguished() {
        let value = pyyaml(
            "v: !!binary aGk=\ns: !!set {a: null}\no: !!omap [{a: b}]\np: !!pairs [{a: b}]\n",
        )
        .unwrap();
        assert_eq!(entry(&value, "v"), &PythonValue::Bytes(b"hi".to_vec()));
        assert!(matches!(entry(&value, "s"), PythonValue::Set(items) if items.len() == 1));
        assert!(matches!(entry(&value, "o"), PythonValue::Pairs(pairs) if pairs.len() == 1));
        assert!(matches!(entry(&value, "p"), PythonValue::Pairs(pairs) if pairs.len() == 1));
        // ruamel's `!!omap` is an ordereddict instead.
        let value = ruamel("o: !!omap [{a: b}]\n").unwrap();
        assert!(matches!(
            entry(&value, "o"),
            PythonValue::OrderedDict(entries) if entries.len() == 1
        ));
    }

    #[test]
    fn arbitrary_precision_and_nonfinite_floats_survive() {
        assert_eq!(
            entry(&pyyaml("v: 123456789012345678901234567890\n").unwrap(), "v"),
            &PythonValue::Int("123456789012345678901234567890".to_owned())
        );
        assert_eq!(
            entry(&pyyaml("v: .inf\n").unwrap(), "v"),
            &PythonValue::Float(f64::INFINITY)
        );
        match entry(&pyyaml("v: .NaN\n").unwrap(), "v") {
            PythonValue::Float(value) => assert!(value.is_nan()),
            other => panic!("expected a float, got {other:?}"),
        }
    }

    #[test]
    fn ruamel_list_keys_become_python_tuples() {
        // The value domain carries a real tuple, not a formatted string: the
        // same document is `{('a', 'b'): 1}` for ruamel and a refusal for PyYAML.
        let value = ruamel("? [a, b]\n: 1\n").unwrap();
        let PythonValue::Map(entries) = &value else {
            panic!("expected a mapping")
        };
        let PythonValue::Tuple(items) = &entries[0].0 else {
            panic!("expected a tuple key, got {:?}", entries[0].0)
        };
        assert_eq!(
            items,
            &vec![
                PythonValue::Str(crate::PythonString::from_scalars("a")),
                PythonValue::Str(crate::PythonString::from_scalars("b")),
            ]
        );
        assert_eq!(entries[0].1, PythonValue::Int("1".to_owned()));
        assert_eq!(entries[0].0.python_type_name(), "tuple");
        // PyYAML never converts a collection key.
        assert_eq!(
            pyyaml("? [a, b]\n: 1\n").unwrap_err().class(),
            ErrorClass::Yaml
        );
        // A nested list stays a list, so hashing the tuple raises TypeError.
        assert_eq!(
            ruamel("v: !!set {? [[a]], b: null}\n").unwrap_err().class(),
            ErrorClass::Runtime
        );
    }

    #[test]
    fn duplicate_collection_keys_follow_python_equality() {
        // `2026-11-05` is a date and `2026-11-05T00:00:00` a datetime: two keys.
        let value = pyyaml("v: {2026-11-05: a, 2026-11-05T00:00:00: b}\n").unwrap();
        assert_eq!(entry(&value, "v").as_mapping().unwrap().len(), 2);
        // Two aware datetimes at the same instant are ONE key; the first key
        // object is kept and the last value wins.
        let value =
            pyyaml("v: {2026-11-05T00:00:00+00:00: a, 2026-11-05T01:00:00+01:00: b}\n").unwrap();
        let entries = entry(&value, "v").as_mapping().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].0,
            PythonValue::DateTime("2026-11-05 00:00:00+00:00".to_owned())
        );
        assert_eq!(
            entries[0].1,
            PythonValue::Str(crate::PythonString::from_scalars("b"))
        );
        // ruamel refuses the same duplicate instead.
        assert_eq!(
            ruamel("v: {2026-11-05T00:00:00+00:00: a, 2026-11-05T01:00:00+01:00: b}\n")
                .unwrap_err()
                .class(),
            ErrorClass::Yaml
        );
        // `.nan` is one loader singleton, so two `.nan` keys are one entry.
        let value = pyyaml("v: {.nan: a, .nan: b}\n").unwrap();
        let entries = entry(&value, "v").as_mapping().unwrap();
        assert_eq!(entries.len(), 1);
        assert!(matches!(entries[0].0, PythonValue::Float(number) if number.is_nan()));
    }

    #[test]
    fn deep_nesting_never_aborts_the_process() {
        // Source's ceiling is the interpreter's recursion budget (200 nested
        // lists load, 500 raise `RecursionError`), which is a declared open case
        // here. Whatever this reader's own ceiling is, exceeding it must be a
        // typed refusal — never a stack abort that takes the process down.
        for depth in [200_usize, 500, 4000] {
            let text = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
            match decode_text(&text, Dialect::PyYamlSafe11) {
                Ok(_) => println!("depth {depth}: accepted"),
                Err(error) => {
                    assert!(
                        matches!(error.class(), ErrorClass::Yaml | ErrorClass::Runtime),
                        "depth {depth} must fail typed, got {error:?}"
                    );
                    println!("depth {depth}: {:?} {error}", error.class());
                }
            }
        }
    }

    #[test]
    fn open_cases_are_declared() {
        assert!(OPEN_CASES.iter().any(|case| case.id == "recursive-alias"));
        // The recursive-alias divergence is real, not theoretical.
        assert!(pyyaml("a: &x [*x]\n").is_err());
    }
}
