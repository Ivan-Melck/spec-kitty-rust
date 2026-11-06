//! Source-exact construction of YAML core-tag values for both Source dialects.
//!
//! Source loads YAML with a *safe* constructor that turns the parsed node graph
//! into Python objects before any model validation:
//!
//! * `yaml.safe_load` (PyYAML, YAML **1.1**) for `mission.yaml`;
//! * `ruamel.yaml.YAML(typ="safe")` (YAML **1.2**) for profiles, offerings,
//!   `project.path_conventions` and activation config.
//!
//! `serde_yaml_ng` keeps a tag as `Value::Tagged` and its accessors (`as_str`,
//! `as_i64`, …) silently untag it, so using it directly made `!!binary SGVsbG8=`
//! read as the *base64 text* `SGVsbG8=` instead of the bytes `Hello`, made
//! `!!set` look like a mapping, and accepted `!custom`/`!!timestamp` input the
//! reference refuses.
//!
//! This module is that missing construction phase — one place for both dialects,
//! applied to the single semantic tree the transport already built. The dialect
//! changes the *policy*, never the mechanism: the bool word table, integer and
//! float grammars, the `!!omap` result type and the duplicate-key/unhashable
//! rules. Every difference below is a probed receipt (see the L4 report).
//!
//! Two representative encodings keep the constructed *kind* visible to the one
//! model normalizer without a second value tree:
//!
//! * bytes are `!!binary`-tagged sequences of byte numbers;
//! * timestamps are `!!timestamp`-tagged authored scalars.
//!
//! Values the model dumps (a decoded string, a set's list, a mapping) are plain
//! scalars/sequences/mappings, exactly like the reference's `model_dump`.

use std::collections::BTreeMap;

use serde_yaml_ng::value::{Tag, TaggedValue};
use serde_yaml_ng::{Mapping, Number, Value};

use super::Dialect;
use super::error::{RuntimeCause, YamlError};
use crate::PythonString;

/// The YAML core tag prefix.
const TAG_PREFIX: &str = "tag:yaml.org,2002:";
/// The constructed-bytes tag (never authored; see module docs).
pub const BINARY_TAG: &str = "tag:yaml.org,2002:binary";
/// The constructed-timestamp tag.
pub const TIMESTAMP_TAG: &str = "tag:yaml.org,2002:timestamp";
/// The constructed big-integer tag (a Python `int` outside `i64`'s range).
pub const INTEGER_TAG: &str = "tag:yaml.org,2002:int";
/// The constructed-`set` tag (never authored; see module docs).
///
/// A `!!set` constructs to a Python `set`, which `str()` renders as `{…}` (or
/// `set()` when empty) — not as a list. The ordered keys stay inside this tag so
/// the one text authority can tell a `set` from an authored sequence while every
/// value accessor (`as_sequence`, …) still untags it transparently.
pub const SET_TAG: &str = "!sk-constructed-set";
/// The constructed `!!omap` tag: a Python `ordereddict` whose `repr` is
/// `ordereddict([…])`.
pub const OMAP_TAG: &str = "!sk-constructed-omap";
/// The constructed `!!pairs` tag: a Python `list` of `(key, value)` tuples.
pub const PAIRS_TAG: &str = "!sk-constructed-pairs";
/// The constructed tuple tag: a real Python `tuple`.
///
/// ruamel's safe constructor converts a **list mapping key** with
/// `tuple(key)` (`constructor.py`, `construct_mapping`), so `? [a, b]` is the
/// key `('a', 'b')` — hashable, Python-equal to another `('a', 'b')`, and
/// `repr()`ed as a tuple. The tag keeps that kind visible to the one text/hash
/// authority without a second value tree; every value accessor untags it.
pub const TUPLE_TAG: &str = "!sk-constructed-tuple";

/// Replace the constructed-kind tags with the plain value their model layer sees.
///
/// `construct` keeps a constructed `set`/`!!omap`/`!!pairs` distinguishable so
/// the one text authority can spell Python's `str()` for it. The *model* layer
/// consumes the same value tree as a plain list/mapping (the reference's
/// Pydantic models do exactly that: a `set` iterates into a `list[str]`, an
/// `ordereddict` is a `dict`), so every model-facing consumer calls this once
/// after construction and sees the untagged tree it always saw.
pub fn untag_constructed(value: &mut Value) {
    match value {
        Value::Tagged(tagged) => {
            let constructed_kind = matches!(tagged.tag, ref tag
                if *tag == SET_TAG || *tag == OMAP_TAG || *tag == PAIRS_TAG);
            if !constructed_kind {
                // `!!binary`/`!!timestamp`/big `!!int` keep their tags: those are
                // the constructed Python kinds the model layer reads.
                untag_constructed(&mut tagged.value);
                return;
            }
            let tagged = std::mem::replace(
                tagged.as_mut(),
                TaggedValue {
                    tag: Tag::new(SET_TAG),
                    value: Value::Null,
                },
            );
            let mut inner = tagged.value;
            untag_constructed(&mut inner);
            *value = inner;
        }
        Value::Sequence(items) => {
            for item in items {
                untag_constructed(item);
            }
        }
        Value::Mapping(mapping) => {
            let entries: Vec<(Value, Value)> = mapping
                .iter()
                .map(|(key, item)| (key.clone(), item.clone()))
                .collect();
            let mut rebuilt = Mapping::new();
            for (mut key, mut item) in entries {
                untag_constructed(&mut key);
                untag_constructed(&mut item);
                rebuilt.insert(key, item);
            }
            *mapping = rebuilt;
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

/// Construct every tagged node in `value`, exactly as the dialect's safe
/// constructor does.
pub fn construct(
    value: &mut Value,
    markers: &BTreeMap<String, PythonString>,
    dialect: Dialect,
) -> Result<(), YamlError> {
    match value {
        Value::Tagged(_) => {
            // The identity splice marks anchor sites, alias sites and fresh
            // `!!float nan` objects *after* the transport has refused every
            // authored tag outside the core vocabulary, so a graph tag here is
            // always one this reader produced. The semantic constructor leaves
            // it in place; `graph::build` owns its meaning.
            if super::graph::graph_tag_name(value).is_some() {
                let Value::Tagged(tagged) = std::mem::replace(value, Value::Null) else {
                    unreachable!("the match arm proved the variant");
                };
                let tag = tagged.tag;
                let mut inner = tagged.value;
                construct(&mut inner, markers, dialect)?;
                *value = Value::Tagged(Box::new(TaggedValue { tag, value: inner }));
                return Ok(());
            }
            let Value::Tagged(tagged) = std::mem::replace(value, Value::Null) else {
                unreachable!("the match arm proved the variant");
            };
            *value = construct_tagged(*tagged, markers, dialect)?;
            Ok(())
        }
        Value::Sequence(items) => {
            for item in items {
                construct(item, markers, dialect)?;
            }
            Ok(())
        }
        Value::Mapping(mapping) => {
            let entries: Vec<(Value, Value)> = mapping
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            let mut constructed = Vec::with_capacity(entries.len());
            for (mut key, mut value) in entries {
                construct(&mut key, markers, dialect)?;
                // PyYAML's `construct_mapping` refuses a collection key *before*
                // it constructs the value; ruamel converts a list key first.
                let key = mapping_key(key, markers, dialect)?;
                construct(&mut value, markers, dialect)?;
                insert_python_entry(&mut constructed, key, value, markers, dialect)?;
            }
            *mapping = mapping_from(constructed);
            Ok(())
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => Ok(()),
    }
}

/// Python `dict[key] = value` (and ruamel's duplicate-key refusal).
///
/// A Python mapping has one entry per *equal* key, so `{1: a, true: b}` is one
/// entry: PyYAML keeps the last value in the first key position; ruamel's safe
/// loader raises `DuplicateKeyError` instead.
fn insert_python_entry(
    entries: &mut Vec<(Value, Value)>,
    key: Value,
    value: Value,
    markers: &BTreeMap<String, PythonString>,
    dialect: Dialect,
) -> Result<(), YamlError> {
    if dialect.refuses_duplicate_keys() {
        // ruamel's `mapping[key] = value` *hashes* the key. A tuple holding a
        // `list`/`dict` therefore raises the bare `TypeError` the reference does
        // not catch, after the value has been constructed.
        super::set_order::python_hash_of(&key, markers, dialect)?;
    }
    if let Some(position) = entries
        .iter()
        .position(|(existing, _)| super::set_order::python_value_equal(existing, &key, markers))
    {
        if dialect.refuses_duplicate_keys() {
            return Err(YamlError::Construct(format!(
                "found duplicate key {}",
                python_key_spelling(&entries[position].0, markers)
            )));
        }
        entries[position].1 = value;
        return Ok(());
    }
    entries.push((key, value));
    Ok(())
}

fn mapping_from(entries: Vec<(Value, Value)>) -> Mapping {
    let mut mapping = Mapping::new();
    for (key, value) in entries {
        mapping.insert(key, value);
    }
    mapping
}

/// Whether the constructed value is a Python object a `dict` can hash.
///
/// `list`, `dict`, `set`, `ordereddict` and a `!!pairs` list are unhashable;
/// every scalar, `bytes`, `date`/`datetime` and `tuple` is hashable (a tuple is
/// hashable as an *instance* even when a member is not — that failure happens
/// later, in `hash()`).
fn python_hashable(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => true,
        Value::Sequence(_) | Value::Mapping(_) => false,
        Value::Tagged(tagged) => matches!(tagged.tag, ref tag
            if *tag == BINARY_TAG
                || *tag == INTEGER_TAG
                || *tag == TIMESTAMP_TAG
                || *tag == TUPLE_TAG
                || super::graph::is_fresh_nan_tag(tag)),
    }
}

/// The dialect's mapping-key construction.
///
/// ruamel's `construct_mapping` is explicit: a key that is a `list` becomes
/// `tuple(key)` (shallow — a nested list stays a list), and a key that is still
/// unhashable (`dict`, `set`, …) is its `ConstructorError` "found unhashable
/// key". PyYAML never converts, so a list key is that same `ConstructorError`.
fn mapping_key(
    key: Value,
    _markers: &BTreeMap<String, PythonString>,
    dialect: Dialect,
) -> Result<Value, YamlError> {
    let key = match (dialect, key) {
        (Dialect::RuamelSafe12, Value::Sequence(items)) => tuple_value(items),
        (Dialect::RuamelSafe12, Value::Tagged(tagged)) if tagged.tag == PAIRS_TAG => {
            // The shallow list-key conversion also applies to the Python list
            // of real tuples produced by !!pairs.
            let Value::Sequence(items) = tagged.value else {
                unreachable!("the canonical pairs constructor stores list items");
            };
            tuple_value(items)
        }
        (_, key) => key,
    };
    if !python_hashable(&key) {
        return Err(constructor_error(format!(
            "while constructing a mapping: found unhashable key ({} value)",
            unhashable_type_name(&key)
        )));
    }
    Ok(key)
}

/// The Python type name Source uses for an unhashable key.
fn unhashable_type_name(value: &Value) -> &'static str {
    match value {
        Value::Sequence(_) => "list",
        Value::Mapping(_) => "dict",
        Value::Tagged(tagged) => match tagged.tag {
            ref tag if *tag == SET_TAG => "set",
            ref tag if *tag == OMAP_TAG => "ordereddict",
            ref tag if *tag == PAIRS_TAG => "list",
            _ => "object",
        },
        _ => "object",
    }
}

/// A constructed Python `tuple`.
pub fn tuple_value(items: Vec<Value>) -> Value {
    Value::Tagged(Box::new(TaggedValue {
        tag: Tag::new(TUPLE_TAG),
        value: Value::Sequence(items),
    }))
}

/// The items of a constructed Python `tuple`.
pub fn constructed_tuple(value: &Value) -> Option<&Vec<Value>> {
    match value {
        Value::Tagged(tagged) if tagged.tag == TUPLE_TAG => tagged.value.as_sequence(),
        _ => None,
    }
}

/// Python `repr()` of a duplicate key, as ruamel spells it in its message.
fn python_key_spelling(value: &Value, markers: &BTreeMap<String, PythonString>) -> String {
    match value {
        Value::String(text) => {
            let exact = markers
                .get(text)
                .cloned()
                .unwrap_or_else(|| PythonString::from_scalars(text));
            exact.repr().to_string()
        }
        other => super::set_order::python_repr_text(other, markers),
    }
}

/// The reference `SafeConstructor`'s tag table (as `ruamel.yaml` spells it).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CoreTag {
    Null,
    Bool,
    Int,
    Float,
    Binary,
    Timestamp,
    Omap,
    Pairs,
    Set,
    Str,
    Seq,
    Map,
}

fn core_tag(tag: &Tag) -> Option<CoreTag> {
    for (name, kind) in [
        ("null", CoreTag::Null),
        ("bool", CoreTag::Bool),
        ("int", CoreTag::Int),
        ("float", CoreTag::Float),
        ("binary", CoreTag::Binary),
        ("timestamp", CoreTag::Timestamp),
        ("omap", CoreTag::Omap),
        ("pairs", CoreTag::Pairs),
        ("set", CoreTag::Set),
        ("str", CoreTag::Str),
        ("seq", CoreTag::Seq),
        ("map", CoreTag::Map),
    ] {
        // Authored core tags are carried as the equivalent local tag by
        // `transport` (the semantic reader drops the `!!` core spelling), and
        // the reference's own resolver may also hand a full URI through.
        if tag == &format!("{TAG_PREFIX}{name}") || tag == name {
            return Some(kind);
        }
    }
    None
}

/// Render a tag the way the reference names it in its constructor error.
fn tag_spelling(tag: &Tag) -> String {
    let displayed = tag.to_string();
    match displayed.strip_prefix('!') {
        Some(uri) if uri.starts_with("tag:") => uri.to_owned(),
        _ => displayed,
    }
}

fn constructor_error(message: impl std::fmt::Display) -> YamlError {
    YamlError::Construct(message.to_string())
}

fn unknown_tag(tag: &Tag) -> YamlError {
    constructor_error(format!(
        "could not determine a constructor for the tag {}",
        PythonString::from_scalars(&tag_spelling(tag)).repr()
    ))
}

/// The reference's node-id wording for a wrong node kind.
fn node_kind(value: &Value) -> &'static str {
    match value {
        Value::Sequence(_) => "sequence",
        Value::Mapping(_) => "mapping",
        Value::Null => "scalar",
        _ => "scalar",
    }
}

fn scalar_text(value: &Value) -> Result<&str, YamlError> {
    match value {
        Value::String(text) => Ok(text),
        Value::Null => Ok(""),
        other => Err(constructor_error(format!(
            "expected a scalar node, but found {}",
            node_kind(other)
        ))),
    }
}

fn construct_tagged(
    tagged: TaggedValue,
    markers: &BTreeMap<String, PythonString>,
    dialect: Dialect,
) -> Result<Value, YamlError> {
    let TaggedValue { tag, value } = tagged;
    let Some(kind) = core_tag(&tag) else {
        return Err(unknown_tag(&tag));
    };
    match kind {
        CoreTag::Null => {
            scalar_text(&value)?;
            Ok(Value::Null)
        }
        CoreTag::Bool => {
            let text = scalar_text(&value)?;
            let lowered = text.to_lowercase();
            match dialect.bool_word(&lowered) {
                Some(flag) => Ok(Value::Bool(flag)),
                // `self.bool_values[value.lower()]` raises a bare `KeyError`,
                // which the reference's loader does not catch: the whole
                // repository load aborts.
                None => Err(YamlError::Runtime {
                    cause: RuntimeCause::Key,
                    message: PythonString::from_scalars(&lowered).repr(),
                }),
            }
        }
        CoreTag::Int => {
            let text = scalar_text(&value)?;
            construct_int(text, dialect)
        }
        CoreTag::Float => {
            let text = scalar_text(&value)?;
            construct_float(text, dialect)
        }
        CoreTag::Binary => {
            let text = scalar_text(&value)?;
            let bytes = decode_base64_bytes(text)?;
            Ok(bytes_value(&bytes))
        }
        CoreTag::Timestamp => {
            let text = scalar_text(&value)?;
            validate_timestamp(text)?;
            Ok(Value::Tagged(Box::new(TaggedValue {
                tag: Tag::new(TIMESTAMP_TAG),
                value: Value::String(text.to_owned()),
            })))
        }
        CoreTag::Omap => {
            let items = value.as_sequence().ok_or_else(|| {
                constructor_error(format!(
                    "while constructing an ordered map: expected a sequence, but found {}",
                    node_kind(&value)
                ))
            })?;
            let entries = single_entry_mappings(items, "an ordered map")?;
            let mut ordered: Vec<(Value, Value)> = Vec::with_capacity(entries.len());
            for (mut key, mut value) in entries {
                construct(&mut key, markers, dialect)?;
                construct(&mut value, markers, dialect)?;
                if !dialect.refuses_duplicate_keys() {
                    // PyYAML's `construct_yaml_omap` appends to a Python list and
                    // never asserts; duplicates are ordinary pairs.
                    ordered.push((key, value));
                    continue;
                }
                // ruamel asserts `key not in omap`, which *hashes* the key: a
                // list key is the bare `TypeError` here (no tuple conversion —
                // `construct_yaml_omap` does not go through `construct_mapping`).
                super::set_order::python_hash_of(&key, markers, dialect)?;
                if ordered.iter().any(|(existing, _)| {
                    super::set_order::python_value_equal(existing, &key, markers)
                }) {
                    // The reference's `assert key not in omap` aborts the load.
                    return Err(YamlError::Runtime {
                        cause: RuntimeCause::Assertion,
                        message: String::new(),
                    });
                }
                ordered.push((key, value));
            }
            if dialect.refuses_duplicate_keys() {
                Ok(Value::Tagged(Box::new(TaggedValue {
                    tag: Tag::new(OMAP_TAG),
                    value: Value::Mapping(mapping_from(ordered)),
                })))
            } else {
                // PyYAML's `!!omap` is a Python `list` of `(key, value)` tuples.
                Ok(pairs_value(ordered))
            }
        }
        CoreTag::Pairs => {
            let items = value.as_sequence().ok_or_else(|| {
                constructor_error(format!(
                    "while constructing pairs: expected a sequence, but found {}",
                    node_kind(&value)
                ))
            })?;
            let entries = single_entry_mappings(items, "pairs")?;
            let mut pairs = Vec::with_capacity(entries.len());
            for (mut key, mut value) in entries {
                construct(&mut key, markers, dialect)?;
                construct(&mut value, markers, dialect)?;
                pairs.push((key, value));
            }
            Ok(pairs_value(pairs))
        }
        CoreTag::Set => {
            let mapping = value.as_mapping().ok_or_else(|| {
                constructor_error(format!(
                    "expected a mapping node, but found {}",
                    node_kind(&value)
                ))
            })?;
            let entries: Vec<(Value, Value)> = mapping
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            let mut members: Vec<(Value, Value)> = Vec::with_capacity(entries.len());
            for (mut key, mut value) in entries {
                construct(&mut key, markers, dialect)?;
                // `!!set` is `construct_mapping(node)` + `set.update(...)`: the
                // same key policy and the same unhashable-member outcome.
                let key = mapping_key(key, markers, dialect)?;
                construct(&mut value, markers, dialect)?;
                insert_python_entry(&mut members, key, value, markers, dialect)?;
            }
            let keys = members.into_iter().map(|(key, _)| key).collect();
            // `data.update(construct_mapping(node))` hashes each key. PyYAML's
            // `construct_mapping` reports a collection key as `ConstructorError`
            // ("found unhashable key", a `YAMLError` -> that file is skipped);
            // ruamel's safe constructor has its own (see `python_set_order`).
            let ordered = python_set_order(keys, markers, dialect)?;
            Ok(Value::Tagged(Box::new(TaggedValue {
                tag: Tag::new(SET_TAG),
                value: Value::Sequence(ordered),
            })))
        }
        CoreTag::Str => {
            let text = scalar_text(&value)?;
            Ok(Value::String(text.to_owned()))
        }
        CoreTag::Seq => {
            let items = value.as_sequence().ok_or_else(|| {
                constructor_error(format!(
                    "expected a sequence node, but found {}",
                    node_kind(&value)
                ))
            })?;
            let mut constructed = Vec::with_capacity(items.len());
            for mut item in items.iter().cloned() {
                construct(&mut item, markers, dialect)?;
                constructed.push(item);
            }
            Ok(Value::Sequence(constructed))
        }
        CoreTag::Map => {
            let mapping = value.as_mapping().ok_or_else(|| {
                constructor_error(format!(
                    "expected a mapping node, but found {}",
                    node_kind(&value)
                ))
            })?;
            let entries: Vec<(Value, Value)> = mapping
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            let mut constructed = Vec::with_capacity(entries.len());
            for (mut key, mut value) in entries {
                construct(&mut key, markers, dialect)?;
                let key = mapping_key(key, markers, dialect)?;
                construct(&mut value, markers, dialect)?;
                insert_python_entry(&mut constructed, key, value, markers, dialect)?;
            }
            Ok(Value::Mapping(mapping_from(constructed)))
        }
    }
}

/// The constructed value of a Python list of `(key, value)` tuples.
fn pairs_value(pairs: Vec<(Value, Value)>) -> Value {
    Value::Tagged(Box::new(TaggedValue {
        tag: Tag::new(PAIRS_TAG),
        value: Value::Sequence(
            pairs
                .into_iter()
                .map(|(key, value)| tuple_value(vec![key, value]))
                .collect(),
        ),
    }))
}

fn single_entry_mappings(items: &[Value], what: &str) -> Result<Vec<(Value, Value)>, YamlError> {
    let mut entries = Vec::with_capacity(items.len());
    for item in items {
        let Some(mapping) = item.as_mapping() else {
            return Err(constructor_error(format!(
                "while constructing {what}: expected a mapping of length 1, but found {}",
                node_kind(item)
            )));
        };
        if mapping.len() != 1 {
            return Err(constructor_error(format!(
                "while constructing {what}: expected a single mapping item, but found {} items",
                mapping.len()
            )));
        }
        let (key, value) = mapping
            .iter()
            .next()
            .expect("a length-1 mapping has one entry");
        entries.push((key.clone(), value.clone()));
    }
    Ok(entries)
}

/// A constructed Python `bytes` value.
pub fn bytes_value(bytes: &[u8]) -> Value {
    Value::Tagged(Box::new(TaggedValue {
        tag: Tag::new(BINARY_TAG),
        value: Value::Sequence(
            bytes
                .iter()
                .map(|byte| Value::Number(Number::from(u64::from(*byte))))
                .collect(),
        ),
    }))
}

/// The bytes of a constructed `!!binary` value.
pub fn constructed_bytes(value: &Value) -> Option<Vec<u8>> {
    let Value::Tagged(tagged) = value else {
        return None;
    };
    if tagged.tag != BINARY_TAG {
        return None;
    }
    let Value::Sequence(items) = &tagged.value else {
        return None;
    };
    let mut bytes = Vec::with_capacity(items.len());
    for item in items {
        let Value::Number(number) = item else {
            return None;
        };
        let byte = number.as_u64()?;
        bytes.push(u8::try_from(byte).ok()?);
    }
    Some(bytes)
}

/// The ordered keys of a constructed Python `set`.
pub fn constructed_set(value: &Value) -> Option<&Vec<Value>> {
    match value {
        Value::Tagged(tagged) if tagged.tag == SET_TAG => tagged.value.as_sequence(),
        _ => None,
    }
}

/// The entries of a constructed `!!omap` (a Python `ordereddict`).
pub fn constructed_omap(value: &Value) -> Option<&Mapping> {
    match value {
        Value::Tagged(tagged) if tagged.tag == OMAP_TAG => tagged.value.as_mapping(),
        _ => None,
    }
}

/// The pairs of a constructed `!!pairs` list.
pub fn constructed_pairs(value: &Value) -> Option<&Vec<Value>> {
    match value {
        Value::Tagged(tagged) if tagged.tag == PAIRS_TAG => tagged.value.as_sequence(),
        _ => None,
    }
}

/// A constructed value read as a Python list (`list`, `set` keys or `pairs`).
pub fn constructed_sequence(value: &Value) -> Option<&Vec<Value>> {
    constructed_set(value)
        .or_else(|| constructed_pairs(value))
        .or_else(|| value.as_sequence())
}

/// A constructed value read as a Python mapping (`dict` or `ordereddict`).
pub fn constructed_mapping(value: &Value) -> Option<&Mapping> {
    constructed_omap(value).or_else(|| value.as_mapping())
}

/// Whether `value` is a constructed timestamp.
pub fn is_constructed_timestamp(value: &Value) -> bool {
    matches!(value, Value::Tagged(tagged) if tagged.tag == TIMESTAMP_TAG)
}

/// The digits (with optional sign) of a constructed big integer.
pub fn constructed_big_integer(value: &Value) -> Option<&str> {
    let Value::Tagged(tagged) = value else {
        return None;
    };
    if tagged.tag != INTEGER_TAG {
        return None;
    }
    tagged.value.as_str()
}

/// The constructed big integer value.
pub fn big_integer_value(digits: &str) -> Value {
    Value::Tagged(Box::new(TaggedValue {
        tag: Tag::new(INTEGER_TAG),
        value: Value::String(digits.to_owned()),
    }))
}

/// Construct one scalar from its resolved `kind` and authored payload.
///
/// Used by the transport's duplicate-key identity check (a plain key is typed by
/// the dialect's resolver before construction) and by the post-construction
/// collapse of a renamed duplicate key. Returns `None` when the dialect's
/// constructor refuses the payload — the caller is only comparing identities.
pub fn construct_scalar(kind: &str, payload: &str, dialect: Dialect) -> Option<Value> {
    let mut value = Value::Tagged(Box::new(TaggedValue {
        tag: Tag::new(format!("!{kind}")),
        value: Value::String(payload.to_owned()),
    }));
    construct(&mut value, &BTreeMap::new(), dialect).ok()?;
    Some(value)
}

/// Python `int()` for the dialect's `!!int` constructor.
///
/// PyYAML's (YAML 1.1) grammar also accepts a leading-zero octal spelling and a
/// base-60 sexagesimal spelling; ruamel's (YAML 1.2) does not.
fn construct_int(text: &str, dialect: Dialect) -> Result<Value, YamlError> {
    match dialect {
        Dialect::PyYamlSafe11 => construct_int_yaml11(text),
        Dialect::RuamelSafe12 => construct_int_yaml12(text),
    }
}

fn construct_int_yaml11(text: &str) -> Result<Value, YamlError> {
    let cleaned: String = text.chars().filter(|character| *character != '_').collect();
    let bytes = cleaned.as_bytes();
    let Some(first) = bytes.first() else {
        return Err(int_error(10, &cleaned));
    };
    let (negative, body) = match first {
        b'-' => (true, &cleaned[1..]),
        b'+' => (false, &cleaned[1..]),
        _ => (false, cleaned.as_str()),
    };
    if body.is_empty() {
        return Err(int_error(10, &cleaned));
    }
    if body == "0" {
        return Ok(Value::Number(Number::from(0u64)));
    }
    if let Some(digits) = body.strip_prefix("0b") {
        return int_from_radix(digits, 2, negative, &format!("0b{digits}"));
    }
    if let Some(digits) = body.strip_prefix("0x") {
        return int_from_radix(digits, 16, negative, &format!("0x{digits}"));
    }
    if body.starts_with('0') {
        // `int(value, 8)`: a leading zero means octal in YAML 1.1.
        return int_from_radix(body, 8, negative, body);
    }
    if body.contains(':') {
        // Sexagesimal: `1:30` is ninety.
        let mut total: i128 = 0;
        let mut base: i128 = 1;
        for part in body.split(':').rev() {
            let Some(value) = parse_decimal(part) else {
                return Err(int_error(10, part));
            };
            total = total.saturating_add(value.saturating_mul(base));
            base = base.saturating_mul(60);
        }
        return integer_value(if negative {
            decimal_from_small(-total)
        } else {
            decimal_from_small(total)
        });
    }
    let Some(magnitude) = parse_decimal(body) else {
        return Err(int_error(10, &cleaned));
    };
    integer_value(if negative {
        decimal_from_small(-magnitude)
    } else {
        decimal_from_small(magnitude)
    })
}

fn construct_int_yaml12(text: &str) -> Result<Value, YamlError> {
    let cleaned: String = text.chars().filter(|character| *character != '_').collect();
    let Some(first) = cleaned.as_bytes().first() else {
        return Err(int_error(10, &cleaned));
    };
    let (negative, body) = match first {
        b'-' => (true, &cleaned[1..]),
        b'+' => (false, &cleaned[1..]),
        _ => (false, cleaned.as_str()),
    };
    if body.is_empty() {
        return Err(int_error(10, &cleaned));
    }
    if body == "0" {
        return Ok(Value::Number(Number::from(0u64)));
    }
    let (radix, digits) = if let Some(digits) = body.strip_prefix("0b") {
        (2u32, digits)
    } else if let Some(digits) = body.strip_prefix("0x") {
        (16u32, digits)
    } else if let Some(digits) = body.strip_prefix("0o") {
        (8u32, digits)
    } else {
        (10u32, body)
    };
    if digits.is_empty() {
        return Err(int_error(radix, &cleaned));
    }
    let Some(magnitude) = parse_radix_magnitude(digits, radix) else {
        return Err(int_error(radix, &cleaned));
    };
    match magnitude {
        Magnitude::Small(value) => integer_value(if negative {
            format!("-{}", decimal_from_small(value))
        } else {
            decimal_from_small(value)
        }),
        Magnitude::Big(decimal) => {
            let exact = if negative {
                format!("-{decimal}")
            } else {
                decimal
            };
            Ok(big_integer_value(&exact))
        }
    }
}

/// CPython's `invalid literal for int()` message.
fn int_error(radix: u32, argument: &str) -> YamlError {
    YamlError::Runtime {
        cause: RuntimeCause::Value,
        message: format!(
            "invalid literal for int() with base {radix}: {}",
            PythonString::from_scalars(argument).repr()
        ),
    }
}

/// Turn a decimal magnitude into the narrowest exact integer value.
fn integer_value(decimal: String) -> Result<Value, YamlError> {
    let (negative, body) = match decimal.strip_prefix('-') {
        Some(body) => (true, body),
        None => (false, decimal.as_str()),
    };
    if let Ok(small) = body.parse::<u64>() {
        if negative {
            if small <= i64::MAX as u64 {
                return Ok(Value::Number(Number::from(-(small as i64))));
            }
            if small == i64::MAX as u64 + 1 {
                return Ok(Value::Number(Number::from(i64::MIN)));
            }
        } else if small <= i64::MAX as u64 {
            return Ok(Value::Number(Number::from(small as i64)));
        } else {
            return Ok(Value::Number(Number::from(small)));
        }
    }
    Ok(big_integer_value(&decimal))
}

fn parse_decimal(digits: &str) -> Option<i128> {
    let mut value: i128 = 0;
    if digits.is_empty() {
        return None;
    }
    for character in digits.chars() {
        let digit = character.to_digit(10)?;
        value = value.checked_mul(10)?.checked_add(i128::from(digit))?;
    }
    Some(value)
}

fn int_from_radix(
    digits: &str,
    radix: u32,
    negative: bool,
    argument: &str,
) -> Result<Value, YamlError> {
    let Some(magnitude) = parse_radix_magnitude(digits, radix) else {
        return Err(int_error(radix, argument));
    };
    match magnitude {
        Magnitude::Small(value) => integer_value(if negative {
            format!("-{}", decimal_from_small(value))
        } else {
            decimal_from_small(value)
        }),
        Magnitude::Big(decimal) => {
            let exact = if negative {
                format!("-{decimal}")
            } else {
                decimal
            };
            Ok(big_integer_value(&exact))
        }
    }
}

enum Magnitude {
    Small(i128),
    Big(String),
}

/// The decimal spelling of an `i128` (used on the rare path where a crafted
/// magnitude exceeds `i64`/`u64`).
fn decimal_from_small(value: i128) -> String {
    let mut value = value.unsigned_abs();
    if value == 0 {
        return "0".to_owned();
    }
    let mut digits = Vec::new();
    while value > 0 {
        digits.push(b'0' + (value % 10) as u8);
        value /= 10;
    }
    digits.reverse();
    String::from_utf8(digits).expect("ascii digits")
}

/// Parse an unsigned digit string in `radix`, exact for any length.
fn parse_radix_magnitude(digits: &str, radix: u32) -> Option<Magnitude> {
    let mut value: i128 = 0;
    let mut overflowed = false;
    for character in digits.chars() {
        let digit = character.to_digit(radix)?;
        if !overflowed {
            match value
                .checked_mul(i128::from(radix))
                .and_then(|scaled| scaled.checked_add(i128::from(digit)))
            {
                Some(next) => value = next,
                None => overflowed = true,
            }
        }
    }
    if !overflowed {
        return Some(Magnitude::Small(value));
    }
    // Re-encode exactly with `u128`-free long multiplication in base 10.
    let mut decimal: Vec<u32> = vec![0];
    for character in digits.chars() {
        let digit = character.to_digit(radix)?;
        let mut carry = u64::from(digit);
        for place in decimal.iter_mut() {
            let product = u64::from(*place) * u64::from(radix) + carry;
            *place = (product % 10) as u32;
            carry = product / 10;
        }
        while carry > 0 {
            decimal.push((carry % 10) as u32);
            carry /= 10;
        }
    }
    while decimal.len() > 1 && decimal.last() == Some(&0) {
        decimal.pop();
    }
    let text: String = decimal
        .iter()
        .rev()
        .map(|digit| digit.to_string())
        .collect();
    Some(Magnitude::Big(text))
}

/// Python `float()` for the dialect's `!!float` constructor.
fn construct_float(text: &str, dialect: Dialect) -> Result<Value, YamlError> {
    match dialect {
        Dialect::PyYamlSafe11 => construct_float_yaml11(text),
        Dialect::RuamelSafe12 => construct_float_yaml12(text),
    }
}

fn construct_float_yaml11(text: &str) -> Result<Value, YamlError> {
    let cleaned: String = text.chars().filter(|character| *character != '_').collect();
    let lowered = cleaned.to_lowercase();
    let Some(first) = lowered.as_bytes().first() else {
        return Err(float_error(&cleaned));
    };
    let (negative, body) = match first {
        b'-' => (true, &lowered[1..]),
        b'+' => (false, &lowered[1..]),
        _ => (false, lowered.as_str()),
    };
    let value = match body {
        ".inf" => {
            if negative {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            }
        }
        ".nan" => f64::NAN,
        _ => {
            if body.contains(':') {
                // Sexagesimal: `1:30.5` is ninety and a half.
                let mut total: f64 = 0.0;
                let mut base: f64 = 1.0;
                for part in body.split(':').rev() {
                    let Ok(value) = part.trim().parse::<f64>() else {
                        return Err(float_error(&cleaned));
                    };
                    total += value * base;
                    base *= 60.0;
                }
                if negative { -total } else { total }
            } else {
                let Ok(parsed) = body.trim().parse::<f64>() else {
                    return Err(float_error(&cleaned));
                };
                if negative { -parsed } else { parsed }
            }
        }
    };
    Ok(Value::Number(Number::from(value)))
}

fn construct_float_yaml12(text: &str) -> Result<Value, YamlError> {
    let cleaned: String = text.chars().filter(|character| *character != '_').collect();
    let lowered = cleaned.to_lowercase();
    let (negative, body) = match lowered.as_bytes().first() {
        Some(b'-') => (true, &lowered[1..]),
        Some(b'+') => (false, &lowered[1..]),
        _ => (false, lowered.as_str()),
    };
    let value = match body {
        ".inf" => f64::INFINITY,
        ".nan" => f64::NAN,
        _ => {
            let trimmed = body.trim();
            trimmed.parse::<f64>().map_err(|_| float_error(&cleaned))?
        }
    };
    let value = if negative { -value } else { value };
    Ok(Value::Number(Number::from(value)))
}

/// CPython's `could not convert string to float` message.
fn float_error(argument: &str) -> YamlError {
    YamlError::Runtime {
        cause: RuntimeCause::Value,
        message: format!(
            "could not convert string to float: {}",
            PythonString::from_scalars(argument).repr()
        ),
    }
}

/// Python's lenient `base64.decodebytes` (`binascii.a2b_base64`).
pub fn decode_base64_bytes(text: &str) -> Result<Vec<u8>, YamlError> {
    if !text.is_ascii() {
        let character = text.chars().find(|character| !character.is_ascii());
        let detail = character.map_or_else(
            || "non-ascii data".to_owned(),
            |character| {
                format!(
                    "'ascii' codec can't encode character '\\u{{{:x}}}' in position {}: ordinal not in range(128)",
                    u32::from(character),
                    text.chars().position(|item| item == character).unwrap_or(0)
                )
            },
        );
        return Err(constructor_error(format!(
            "failed to convert base64 data into ascii: {detail}"
        )));
    }
    let mut sextets: Vec<u8> = Vec::with_capacity(text.len());
    let mut padding = 0usize;
    let mut padded = false;
    for byte in text.bytes() {
        if byte == b'=' {
            padded = true;
            padding += 1;
            continue;
        }
        let Some(value) = base64_value(byte) else {
            // `a2b_base64` skips characters outside the standard alphabet.
            continue;
        };
        if !padded {
            sextets.push(value);
        }
    }
    let remainder = sextets.len() % 4;
    match remainder {
        0 => {}
        1 => {
            return Err(constructor_error(format!(
                "failed to decode base64 data: Invalid base64-encoded string: number of data \
                 characters ({}) cannot be 1 more than a multiple of 4",
                sextets.len()
            )));
        }
        2 if padding < 2 => {
            return Err(constructor_error(
                "failed to decode base64 data: Incorrect padding",
            ));
        }
        3 if padding < 1 => {
            return Err(constructor_error(
                "failed to decode base64 data: Incorrect padding",
            ));
        }
        _ => {}
    }
    let mut bytes = Vec::with_capacity(sextets.len() / 4 * 3 + 2);
    let full_quads = sextets.len() - remainder;
    for position in (0..full_quads).step_by(4) {
        let quad = &sextets[position..position + 4];
        let combined = (u32::from(quad[0]) << 18)
            | (u32::from(quad[1]) << 12)
            | (u32::from(quad[2]) << 6)
            | u32::from(quad[3]);
        bytes.push((combined >> 16) as u8);
        bytes.push((combined >> 8) as u8);
        bytes.push(combined as u8);
    }
    match remainder {
        2 => {
            let combined = (u32::from(sextets[sextets.len() - 2]) << 6)
                | u32::from(sextets[sextets.len() - 1]);
            bytes.push((combined >> 4) as u8);
        }
        3 => {
            let combined = (u32::from(sextets[sextets.len() - 3]) << 12)
                | (u32::from(sextets[sextets.len() - 2]) << 6)
                | u32::from(sextets[sextets.len() - 1]);
            bytes.push((combined >> 10) as u8);
            bytes.push((combined >> 2) as u8);
        }
        _ => {}
    }
    Ok(bytes)
}

fn base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

struct TimestampParts {
    year: u32,
    month: u32,
    day: u32,
    time: Option<TimestampTime>,
}

struct TimestampTime {
    hour: u32,
    minute: u32,
    second: u32,
    /// Digits after the decimal point, exactly as authored.
    fraction: Option<String>,
    /// Offset in minutes east of UTC, exactly as authored (`Z` is `+00:00`).
    offset: Option<i32>,
    /// `true` when the authored text spelled the UTC marker `Z`.
    utc_z: bool,
}

/// The reference's `timestamp_regexp` (`ruamel/yaml/util.py` / PyYAML).
fn timestamp_parts(text: &str) -> Option<TimestampParts> {
    let bytes = text.as_bytes();
    let mut index = 0usize;
    let digits = |index: &mut usize, min: usize, max: usize| -> Option<u32> {
        let start = *index;
        while *index < bytes.len() && *index - start < max && bytes[*index].is_ascii_digit() {
            *index += 1;
        }
        let length = *index - start;
        if length < min || length > max {
            return None;
        }
        text[start..*index].parse().ok()
    };
    let expected = |index: &mut usize, byte: u8| -> bool {
        if bytes.get(*index) == Some(&byte) {
            *index += 1;
            true
        } else {
            false
        }
    };
    let year = digits(&mut index, 4, 4)?;
    if !expected(&mut index, b'-') {
        return None;
    }
    let month = digits(&mut index, 1, 2)?;
    if !expected(&mut index, b'-') {
        return None;
    }
    let day = digits(&mut index, 1, 2)?;
    let mut time = None;
    if index < bytes.len() {
        if bytes[index] == b'T' || bytes[index] == b't' {
            index += 1;
        } else if bytes[index] == b' ' || bytes[index] == b'\t' {
            while bytes.get(index) == Some(&b' ') || bytes.get(index) == Some(&b'\t') {
                index += 1;
            }
        } else {
            return None;
        }
        let hour = digits(&mut index, 1, 2)?;
        if !expected(&mut index, b':') {
            return None;
        }
        let minute = digits(&mut index, 2, 2)?;
        if !expected(&mut index, b':') {
            return None;
        }
        let second = digits(&mut index, 2, 2)?;
        let mut fraction = None;
        if expected(&mut index, b'.') {
            let start = index;
            while index < bytes.len() && bytes[index].is_ascii_digit() {
                index += 1;
            }
            fraction = Some(text[start..index].to_owned());
        }
        while bytes.get(index) == Some(&b' ') || bytes.get(index) == Some(&b'\t') {
            index += 1;
        }
        let mut offset = None;
        let mut utc_z = false;
        if bytes.get(index) == Some(&b'Z') {
            index += 1;
            utc_z = true;
            offset = Some(0);
        } else if matches!(bytes.get(index), Some(b'+') | Some(b'-')) {
            let negative = bytes[index] == b'-';
            index += 1;
            let hours = digits(&mut index, 1, 2)?;
            let mut minutes = 0;
            if expected(&mut index, b':') {
                minutes = digits(&mut index, 2, 2)?;
            }
            let magnitude = (hours * 60 + minutes) as i32;
            offset = Some(if negative { -magnitude } else { magnitude });
        }
        if index != bytes.len() {
            return None;
        }
        time = Some(TimestampTime {
            hour,
            minute,
            second,
            fraction,
            offset,
            utc_z,
        });
    }
    Some(TimestampParts {
        year,
        month,
        day,
        time,
    })
}

/// Validate a constructed timestamp like `datetime.date`/`datetime.datetime`.
///
/// A value the regex does not match is the reference's `ConstructorError`
/// (skipped file). A value that matches but names an impossible date/time
/// raises `ValueError` from `datetime.date(...)`/`datetime.datetime(...)`,
/// which the loader does not catch — so the message is CPython's, exactly.
fn validate_timestamp(text: &str) -> Result<(), YamlError> {
    let Some(parts) = timestamp_parts(text) else {
        return Err(constructor_error(format!(
            "failed to construct timestamp from {}",
            PythonString::from_scalars(text).repr()
        )));
    };
    if !(1..=9999).contains(&parts.year) {
        return Err(YamlError::Runtime {
            cause: RuntimeCause::Value,
            message: format!("year {} is out of range", parts.year),
        });
    }
    if !(1..=12).contains(&parts.month) {
        return Err(YamlError::Runtime {
            cause: RuntimeCause::Value,
            message: "month must be in 1..12".to_owned(),
        });
    }
    if day_of_month_invalid(parts.year, parts.month, parts.day) {
        return Err(YamlError::Runtime {
            cause: RuntimeCause::Value,
            message: "day is out of range for month".to_owned(),
        });
    }
    if let Some(time) = &parts.time {
        if time.hour > 23 {
            return Err(YamlError::Runtime {
                cause: RuntimeCause::Value,
                message: "hour must be in 0..23".to_owned(),
            });
        }
        if time.minute > 59 {
            return Err(YamlError::Runtime {
                cause: RuntimeCause::Value,
                message: "minute must be in 0..59".to_owned(),
            });
        }
        if time.second > 59 {
            return Err(YamlError::Runtime {
                cause: RuntimeCause::Value,
                message: "second must be in 0..59".to_owned(),
            });
        }
    }
    Ok(())
}

/// CPython `str()` of a constructed timestamp: `2026-11-05` for a date, and
/// `2026-11-05 01:02:03[.ffffff][+HH:MM]` for a datetime.
///
/// Returns `None` when the value is not a constructible timestamp (the
/// validator has already produced the error in that case).
pub fn python_timestamp_text(text: &str) -> Option<(bool, String)> {
    let parts = timestamp_parts(text)?;
    let mut rendered = format!("{:04}-{:02}-{:02}", parts.year, parts.month, parts.day);
    let Some(time) = parts.time else {
        return Some((false, rendered));
    };
    rendered.push_str(&format!(
        " {:02}:{:02}:{:02}",
        time.hour, time.minute, time.second
    ));
    if let Some(fraction) = &time.fraction {
        // CPython keeps microseconds: `int(frac[:6])`, right-padded with zeros.
        let mut micros: String = fraction.chars().take(6).collect();
        while micros.len() < 6 {
            micros.push('0');
        }
        if micros != "000000" {
            rendered.push_str(&format!(".{micros}"));
        }
    }
    if let Some(offset) = time.offset {
        let sign = if offset < 0 { '-' } else { '+' };
        let magnitude = offset.abs();
        rendered.push_str(&format!(
            "{sign}{:02}:{:02}",
            magnitude / 60,
            magnitude % 60
        ));
    }
    let _ = time.utc_z;
    Some((true, rendered))
}

/// The comparable Python identity of a constructed timestamp.
///
/// Source's loaders construct `datetime.date` / `datetime.datetime` objects and
/// Python compares those objects, not their text: a `date` is never equal to a
/// `datetime`, a naive `datetime` is never equal to an aware one, and two aware
/// datetimes are equal when they name the same instant whatever offset they were
/// authored with (`{...T00:00:00+00:00: a, ...T01:00:00+01:00: b}` is ONE dict
/// entry in PyYAML — probed).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimestampIdentity {
    /// `datetime.date`: the proleptic-Gregorian calendar day.
    Date {
        /// Year.
        year: u32,
        /// Month.
        month: u32,
        /// Day.
        day: u32,
    },
    /// `datetime.datetime` without `tzinfo`: local fields, comparable only with
    /// another naive datetime.
    Naive {
        /// Seconds from the civil epoch (`1970-01-01`) of the local fields.
        seconds: i64,
        /// Microseconds.
        micro: u32,
    },
    /// `datetime.datetime` with a fixed offset: the UTC instant.
    Aware {
        /// Seconds from the civil epoch, normalised to UTC.
        seconds: i64,
        /// Microseconds.
        micro: u32,
    },
}

/// The identity Python's `==`/`hash()` use for a constructed timestamp.
///
/// Returns `None` for text the timestamp grammar does not accept (which the
/// validator reports as a `ConstructorError` before any comparison happens).
pub fn timestamp_identity(text: &str) -> Option<TimestampIdentity> {
    let parts = timestamp_parts(text)?;
    let Some(time) = parts.time else {
        return Some(TimestampIdentity::Date {
            year: parts.year,
            month: parts.month,
            day: parts.day,
        });
    };
    // CPython keeps `int(frac[:6])`, right-padded to six digits.
    let micro = match time.fraction.as_deref() {
        Some(fraction) => {
            let mut micros: u32 = 0;
            let mut digits = 0u32;
            for character in fraction.chars().take(6) {
                micros = micros * 10 + character.to_digit(10).unwrap_or(0);
                digits += 1;
            }
            for _ in digits..6 {
                micros *= 10;
            }
            micros
        }
        None => 0,
    };
    let days = days_from_civil(i64::from(parts.year), parts.month, parts.day);
    let local = days * 86_400
        + i64::from(time.hour) * 3_600
        + i64::from(time.minute) * 60
        + i64::from(time.second);
    match time.offset {
        None => Some(TimestampIdentity::Naive {
            seconds: local,
            micro,
        }),
        Some(offset_minutes) => Some(TimestampIdentity::Aware {
            seconds: local - i64::from(offset_minutes) * 60,
            micro,
        }),
    }
}

/// CPython's packed `date`/naive `datetime` storage, from the SAME timestamp
/// parser and microsecond construction as object equality. Aware datetimes hash
/// a normalized timedelta instead; they have no raw-storage hash input.
///
/// CPython v3.11.15 `Include/datetime.h` specifies big-endian year (2 bytes),
/// month/day and (for datetime) hour/minute/second/microseconds (3 bytes).
pub(super) fn timestamp_storage_bytes(text: &str) -> Option<Vec<u8>> {
    let parts = timestamp_parts(text)?;
    let mut bytes = vec![
        (parts.year >> 8) as u8,
        parts.year as u8,
        parts.month as u8,
        parts.day as u8,
    ];
    if let Some(time) = parts.time {
        let TimestampIdentity::Naive { micro, .. } = timestamp_identity(text)? else {
            return None;
        };
        bytes.extend_from_slice(&[
            time.hour as u8,
            time.minute as u8,
            time.second as u8,
            (micro >> 16) as u8,
            (micro >> 8) as u8,
            micro as u8,
        ]);
    }
    Some(bytes)
}

/// Days since `1970-01-01` in the proleptic Gregorian calendar (Hinnant's
/// `days_from_civil`, which is what CPython's datetime arithmetic uses).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_index = i64::from(month);
    let day_of_year =
        (153 * (if month_index > 2 {
            month_index - 3
        } else {
            month_index + 9
        }) + 2)
            / 5
            + i64::from(day)
            - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// `str()` of a constructed timestamp (`2026-11-05`, `2026-11-05 01:02:03+00:00`).
pub fn constructed_timestamp_text(value: &Value) -> Option<String> {
    let Value::Tagged(tagged) = value else {
        return None;
    };
    if tagged.tag != TIMESTAMP_TAG {
        return None;
    }
    let authored = tagged.value.as_str()?;
    Some(
        python_timestamp_text(authored)
            .map(|(_, rendered)| rendered)
            .unwrap_or_else(|| authored.to_owned()),
    )
}

fn day_of_month_invalid(year: u32, month: u32, day: u32) -> bool {
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            let leap =
                (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400);
            if leap { 29 } else { 28 }
        }
        _ => return true,
    };
    day > days
}

/// Python's `set(dict_keys)` iteration order.
///
/// A YAML `!!set` has no authored order: the reference constructs a Python
/// `set`, and Pydantic's `list[str]` then iterates that set's hash table. The
/// order is therefore a function of CPython's string/bytes hash (and the
/// `PYTHONHASHSEED` it was started with), never of the authored key order.
fn python_set_order(
    keys: Vec<Value>,
    markers: &BTreeMap<String, PythonString>,
    dialect: Dialect,
) -> Result<Vec<Value>, YamlError> {
    super::set_order::set_order(keys, markers, dialect)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::python_yaml::{Dialect, decode_document};

    fn construct_text(text: &str, dialect: Dialect) -> Result<Value, YamlError> {
        decode_document(text, dialect)
    }

    #[test]
    fn ruamel_converts_a_list_mapping_key_to_a_real_tuple() {
        // Probed: `? [a, b]\n: 1` is `{('a', 'b'): 1}` under ruamel's safe
        // loader (`construct_mapping`: `if isinstance(key, list): key = tuple(key)`).
        let mapping = construct_text("? [a, b]\n: 1\n", Dialect::RuamelSafe12).unwrap();
        let entries: Vec<(Value, Value)> = mapping
            .as_mapping()
            .expect("a mapping")
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let (key, value) = &entries[0];
        assert_eq!(value.as_i64(), Some(1));
        let items = constructed_tuple(key).expect("the key is a constructed tuple");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].as_str(), Some("a"));
        assert_eq!(items[1].as_str(), Some("b"));
        // The empty list key is `()`.
        let empty = construct_text("? []\n: 1\n", Dialect::RuamelSafe12).unwrap();
        let key = empty.as_mapping().unwrap().iter().next().unwrap().0.clone();
        assert_eq!(constructed_tuple(&key).map(Vec::len), Some(0));
    }

    #[test]
    fn collection_key_policy_follows_the_dialect() {
        // PyYAML never converts: a list key is `found unhashable key`.
        for text in ["? [a, b]\n: 1\n", "v: !!set {? [a], b: null}\n"] {
            let error = construct_text(text, Dialect::PyYamlSafe11).unwrap_err();
            assert_eq!(
                error.class(),
                crate::python_yaml::ErrorClass::Yaml,
                "{text}"
            );
        }
        // ruamel converts the outermost list but a nested list/dict stays
        // unhashable, which fails when the mapping hashes the key.
        for (text, kind) in [
            ("v: !!set {? [[a]], b: null}\n", "list"),
            ("? [a, {b: c}]\n: 1\n", "dict"),
        ] {
            match construct_text(text, Dialect::RuamelSafe12) {
                Err(YamlError::Runtime { cause, message }) => {
                    assert_eq!(cause, RuntimeCause::Type, "{text}");
                    assert_eq!(message, format!("unhashable type: '{kind}'"), "{text}");
                }
                other => panic!("expected a TypeError for {text}, got {other:?}"),
            }
        }
        // `!!omap` does not go through `construct_mapping`: ruamel hashes the
        // list key as-is (`assert key not in omap`), PyYAML keeps the list.
        assert!(matches!(
            construct_text("!!omap [{? [a]: 1}]\n", Dialect::RuamelSafe12),
            Err(YamlError::Runtime {
                cause: RuntimeCause::Type,
                ..
            })
        ));
        let pyyaml = construct_text("!!omap [{? [a]: 1}]\n", Dialect::PyYamlSafe11).unwrap();
        assert_eq!(constructed_pairs(&pyyaml).map(Vec::len), Some(1));
        // `!!pairs` never hashes, so both dialects keep the list key.
        for dialect in [Dialect::PyYamlSafe11, Dialect::RuamelSafe12] {
            let pairs = construct_text("!!pairs [{? [a]: 1}]\n", dialect).unwrap();
            assert_eq!(constructed_pairs(&pairs).map(Vec::len), Some(1));
        }
    }

    #[test]
    fn a_duplicate_tuple_key_follows_the_dialect() {
        // ruamel: `check_mapping_key` sees the equal tuple and refuses.
        let error =
            construct_text("? &k [a]\n: 1\n? *k\n: 2\n", Dialect::RuamelSafe12).unwrap_err();
        assert_eq!(error.class(), crate::python_yaml::ErrorClass::Yaml);
    }

    #[test]
    fn base64_matches_python_lenient_rules() {
        let cases: &[(&str, &str)] = &[
            ("", ""),
            ("SGVs", "48656c"),
            ("SGVsbA==", "48656c6c"),
            ("SGVsbA===", "48656c6c"),
            ("SGVs bG8=", "48656c6c6f"),
            ("SGV!sbG8=", "48656c6c6f"),
            ("AAAA", "000000"),
            ("AA==", "00"),
            ("ABCD", "001083"),
            ("AB==", "00"),
            ("====", ""),
            ("SGVsbG8=extra", "48656c6c6f"),
            ("_-", ""),
            ("SGVsbG8==", "48656c6c6f"),
        ];
        for (text, expected) in cases {
            assert_eq!(
                decode_base64_bytes(text)
                    .expect(text)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>(),
                *expected,
                "{text}"
            );
        }
        for text in [
            "SGV", "SGVsbA", "SGVsbA=", "A", "AB", "ABC", "AB=", "A===", "+/", "SGVsbG8",
            "SGVsbG8*",
        ] {
            assert!(decode_base64_bytes(text).is_err(), "{text}");
        }
    }

    #[test]
    fn tags_construct_to_python_kinds() {
        assert_eq!(
            constructed_bytes(&construct_text("!!binary SGVsbG8=", Dialect::RuamelSafe12).unwrap()),
            Some(b"Hello".to_vec())
        );
        assert_eq!(
            construct_text("!!int \"0x10\"", Dialect::RuamelSafe12)
                .unwrap()
                .as_i64(),
            Some(16)
        );
        assert_eq!(
            construct_text("!!int 0o17", Dialect::RuamelSafe12)
                .unwrap()
                .as_i64(),
            Some(15)
        );
        assert_eq!(
            construct_text("!!int 1_0", Dialect::RuamelSafe12)
                .unwrap()
                .as_i64(),
            Some(10)
        );
        assert_eq!(
            construct_text("!!float 5.0e1", Dialect::RuamelSafe12)
                .unwrap()
                .as_f64(),
            Some(50.0)
        );
        assert_eq!(
            construct_text("!!bool \"on\"", Dialect::RuamelSafe12).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            construct_text("!!str 7", Dialect::RuamelSafe12).unwrap(),
            Value::String("7".into())
        );
        assert!(is_constructed_timestamp(
            &construct_text("!!timestamp 2020-01-01", Dialect::RuamelSafe12).unwrap()
        ));
        // A `!!set` constructs to a Python `set`: its ordered keys live inside the
        // constructed-set tag so `str()` can spell `{…}` (while `as_sequence()`
        // still untags it for every list-typed model field).
        let set = construct_text("!!set {a: null, b: null}", Dialect::RuamelSafe12).unwrap();
        assert_eq!(
            constructed_set(&set).map(Vec::len),
            Some(2),
            "the constructed set keeps its two keys"
        );
        assert_eq!(set.as_sequence().map(Vec::len), Some(2));
        // An `!!omap` is a Python `ordereddict` under ruamel's loader: its
        // entries live inside the constructed-omap tag so `repr()` can spell
        // `ordereddict([…])`, while `as_mapping()` still untags it.
        let omap = construct_text("!!omap [{a: b}]", Dialect::RuamelSafe12).unwrap();
        assert_eq!(
            constructed_omap(&omap).map(serde_yaml_ng::Mapping::len),
            Some(1)
        );
        assert_eq!(omap.as_mapping().map(serde_yaml_ng::Mapping::len), Some(1));
        // A `!!pairs` list is a list of Python tuples: the pairs live inside the
        // constructed-pairs tag so `repr()` can spell `[('a', 'b')]`.
        let pairs = construct_text("!!pairs [{a: b}]", Dialect::RuamelSafe12).unwrap();
        assert!(matches!(
            constructed_pairs(&pairs).map(Vec::as_slice),
            Some([pair]) if constructed_tuple(pair).is_some_and(|items| items.len() == 2)
        ));
        // PyYAML's `!!omap` is a Python list of tuples, not an ordereddict.
        let pyyaml_omap = construct_text("!!omap [{a: b}]", Dialect::PyYamlSafe11).unwrap();
        assert!(constructed_omap(&pyyaml_omap).is_none());
        assert!(matches!(
            constructed_pairs(&pyyaml_omap).map(Vec::as_slice),
            Some([pair]) if constructed_tuple(pair).is_some_and(|items| items.len() == 2)
        ));
    }

    #[test]
    fn unknown_and_invalid_tags_are_constructor_errors() {
        for dialect in [Dialect::RuamelSafe12, Dialect::PyYamlSafe11] {
            for text in [
                "!custom hello",
                "!!timestamp not-a-date",
                "!!timestamp 2020-13-01",
                "!!bool maybe",
                "!!int abc",
                "!!float abc",
                "!!set [a]",
                "!!seq {a: b}",
                "!!map [a]",
            ] {
                assert!(construct_text(text, dialect).is_err(), "{text}");
            }
        }
    }
}
