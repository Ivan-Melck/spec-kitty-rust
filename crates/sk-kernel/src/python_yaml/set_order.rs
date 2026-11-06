//! CPython `set` iteration order and hashing for constructed YAML `!!set` values.
//!
//! Shared by both Source dialects: CPython's set/hash algorithm is the same in
//! PyYAML and ruamel; only the *unhashable member* policy differs and is passed
//! in as the dialect.
//!
//! A YAML `!!set` constructs a Python `set`; Pydantic's `list[str]` then
//! iterates it, so the observable list order is the set's hash-table order.
//! That order is a pure function of CPython's hashes and the process's hash
//! secret (`PYTHONHASHSEED`), never of the authored key order — the pinned
//! oracle proves this: `!!set {Reviewer: null, Implementer: null}` lists
//! `Implementer, Reviewer` under seed 0 and `Reviewer, Implementer` under
//! seed 1.
//!
//! The reference behaviour implemented here, in one place:
//!
//! * `siphash13` (CPython's `Py_HashSecret`-seeded `pysiphash`), with the
//!   `lcg_urandom` key expansion for a numeric `PYTHONHASHSEED`, all-zero keys
//!   for seed `0`, and the empty-string zero hash;
//! * a string hashes its **in-memory code units** (latin-1/UTF-16/UTF-32,
//!   little-endian on this target), not its UTF-8 bytes;
//! * `bytes` hashes its bytes, `bool` its 0/1, `int` its exact decimal value
//!   modulo `2**61-1` (arbitrary precision), `float` through
//!   `_Py_HashDouble`, a tuple through the xxHash accumulator;
//! * insertion into CPython's open-addressing table (`LINEAR_PROBES = 9`,
//!   `PERTURB_SHIFT = 5`, growth at `fill*5 >= mask*3`, rehash in table order),
//!   with equal members collapsed by Python equality first (a YAML mapping's
//!   `1`/`1.0`/`True` are one key);
//! * a `list`/`dict` member is `TypeError: unhashable type`, which the
//!   reference's loader does not catch.
//!
//! **The secret is process-owned.** With no `PYTHONHASHSEED` (or
//! `PYTHONHASHSEED=random`) the reference draws the SipHash keys from the OS
//! per process, so *no* implementation — and no second reference process — can
//! reproduce a particular unseeded run. This module therefore draws its own
//! per-process secret from OS entropy once, instead of silently pretending the
//! all-zero seed is the Source behaviour. An explicitly invalid
//! `PYTHONHASHSEED` is CPython's fatal start-up error
//! ([`HashSeedError`]), never a silent fallback.
//!
//! `hash(None)` and `hash(float('nan'))` are pointer-derived in the reference
//! (CPython hashes the object, not a constant), so they are pointer-derived here
//! too: a process-stable value that is honest about being an address, never a
//! framed reproduction of another process's address.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use crate::PythonString;
use serde_yaml_ng::Value;

use super::error::RuntimeCause;
use super::error::YamlError;

/// CPython's `LINEAR_PROBES`.
const LINEAR_PROBES: usize = 9;
/// CPython's `PERTURB_SHIFT`.
const PERTURB_SHIFT: u32 = 5;
/// CPython's `PySet_MINSIZE`.
const MIN_SET_SIZE: usize = 8;
/// CPython's `_PyHASH_MODULUS` (`2**61 - 1`).
const HASH_MODULUS: u64 = (1 << 61) - 1;
/// CPython's `_PyHASH_BITS`.
const HASH_BITS: u32 = 61;
/// CPython's `_PyHASH_INF`.
const HASH_INF: i64 = 314_159;
/// CPython's `_PyHASH_XXPRIME_*` (the tuple/xxHash accumulator).
const XXPRIME_1: u64 = 11_400_714_785_074_694_791;
const XXPRIME_2: u64 = 14_029_467_366_897_019_727;
const XXPRIME_5: u64 = 2_870_177_450_012_600_261;
/// A stable process-local address stand-in for CPython's pointer hashes.
static POINTER_SENTINEL: u8 = 0;

/// A start-up fatal for `PYTHONHASHSEED`, byte-identical to CPython 3.11's.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HashSeedError;

/// CPython's fatal line for an unusable `PYTHONHASHSEED`.
pub const HASH_SEED_FATAL: &str = "Fatal Python error: config_init_hash_seed: PYTHONHASHSEED must be \"random\" or an integer in range [0; 4294967295]\nPython runtime state: preinitialized\n\n";

impl std::fmt::Display for HashSeedError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(HASH_SEED_FATAL)
    }
}

impl std::error::Error for HashSeedError {}

/// Validate the process hash seed the way CPython's start-up does.
///
/// The reference refuses to start at all for `PYTHONHASHSEED` values that are
/// neither `random` nor an integer in `[0; 4294967295]` (exit 1, no stdout,
/// the fatal line on stderr). A command that constructs a `!!set` calls this
/// first so an invalid seed can never silently become the all-zero secret.
pub fn validate_hash_seed_from_env() -> Result<(), HashSeedError> {
    hash_secret().map(|_| ())
}

/// Order constructed `!!set` keys exactly as CPython's `set` would.
pub fn set_order(
    keys: Vec<Value>,
    markers: &BTreeMap<String, PythonString>,
    dialect: super::Dialect,
) -> Result<Vec<Value>, YamlError> {
    set_order_with_secret(keys, markers, dialect, hash_secret()?)
}

/// Order constructed `!!set` keys with an explicit `Py_HashSecret`.
///
/// The pinned `PYTHONHASHSEED=0` receipts use the all-zero key pair, so a test
/// can assert CPython's exact iteration order without touching the process
/// environment (which another test thread would share).
pub fn set_order_with_secret(
    keys: Vec<Value>,
    markers: &BTreeMap<String, PythonString>,
    dialect: super::Dialect,
    secret: (u64, u64),
) -> Result<Vec<Value>, YamlError> {
    // A YAML mapping is constructed first, so equal keys have already collapsed
    // to their first authored occurrence. Reproduce that with Python equality
    // (a `dict`'s keys, not YAML's structural identity): `1`, `1.0` and `True`
    // are one member.
    let mut members: Vec<Value> = Vec::with_capacity(keys.len());
    let mut hashes: Vec<i64> = Vec::with_capacity(keys.len());
    for key in keys {
        let hash = member_hash(&key, markers, dialect, secret)?;
        if let Some(position) = members
            .iter()
            .position(|existing| python_value_equal(existing, &key, markers))
        {
            let _ = position;
            continue;
        }
        members.push(key);
        hashes.push(hash);
    }
    let order = table_order(&hashes);
    let mut slots: Vec<Option<Value>> = members.into_iter().map(Some).collect();
    Ok(order
        .into_iter()
        .map(|index| slots[index].take().expect("each index is used once"))
        .collect())
}

/// The iteration order of a Python `frozenset[str]` built from `tokens`.
///
/// `tokens` is the insertion sequence the reference's `frozenset(<iterable>)`
/// consumes: a constructed YAML list in authored order, or the iteration order of
/// an already-built set. Equal tokens collapse to their first occurrence, exactly
/// like Python set membership. The result is the order CPython's open-addressing
/// table yields, through the **same** `Py_HashSecret` / `unicode_hash` / table
/// owner as the `!!set` constructor above — never a second hash and never a
/// sorted substitute.
///
/// # Errors
///
/// `PYTHONHASHSEED` is unusable, so CPython would refuse to start.
pub fn string_set_order(tokens: Vec<PythonString>) -> Result<Vec<PythonString>, HashSeedError> {
    let secret = hash_secret()?;
    Ok(string_set_order_with_secret(tokens, secret))
}

/// [`string_set_order`] with an explicit `Py_HashSecret`.
///
/// The pinned `PYTHONHASHSEED=0` receipts use the all-zero key pair, so a test can
/// assert CPython's exact iteration order without touching the process
/// environment (which another test thread would share).
#[must_use]
pub fn string_set_order_with_secret(
    tokens: Vec<PythonString>,
    secret: (u64, u64),
) -> Vec<PythonString> {
    let mut members: Vec<PythonString> = Vec::with_capacity(tokens.len());
    let mut hashes: Vec<i64> = Vec::with_capacity(tokens.len());
    for token in tokens {
        if members.contains(&token) {
            continue;
        }
        // `string_hash_with_secret` is total for a code-point sequence; the
        // `Result` is the shared signature of the hash family.
        let Ok(hash) = string_hash_with_secret(token.codepoints(), secret) else {
            continue;
        };
        members.push(token);
        hashes.push(hash);
    }
    let order = table_order(&hashes);
    let mut slots: Vec<Option<PythonString>> = members.into_iter().map(Some).collect();
    order
        .into_iter()
        .map(|index| slots[index].take().expect("each index is used once"))
        .collect()
}

/// Hash one constructed value exactly as Python's `hash()` would.
///
/// Used where the reference *hashes* a mapping key rather than only comparing
/// it: ruamel's `mapping[key] = value` / `assert key not in omap`. A tuple whose
/// members include a `list`/`dict` therefore raises Python's bare `TypeError`
/// here, in the same phase as Source.
pub(crate) fn python_hash_of(
    value: &Value,
    markers: &BTreeMap<String, PythonString>,
    dialect: super::Dialect,
) -> Result<i64, YamlError> {
    member_hash(value, markers, dialect, hash_secret()?)
}

/// The hash CPython stores for a `tuple`: the xxHash accumulator over its
/// members' hashes (an unhashable member raises, exactly as `hash(([],))` does).
fn tuple_hash(
    items: &[Value],
    markers: &BTreeMap<String, PythonString>,
    dialect: super::Dialect,
    secret: (u64, u64),
) -> Result<i64, YamlError> {
    let mut hashes: Vec<i64> = Vec::with_capacity(items.len());
    for item in items {
        hashes.push(member_hash(item, markers, dialect, secret)?);
    }
    Ok(xx_hash(&hashes))
}

/// The hash CPython stores for one set member.
fn member_hash(
    value: &Value,
    markers: &BTreeMap<String, PythonString>,
    dialect: super::Dialect,
    secret: (u64, u64),
) -> Result<i64, YamlError> {
    let hash = match value {
        Value::String(text) => {
            let codepoints: Vec<u32> = match markers.get(text) {
                Some(exact) => exact.codepoints().to_vec(),
                None => text.chars().map(u32::from).collect(),
            };
            string_hash_with_secret(&codepoints, secret)?
        }
        Value::Bool(flag) => i64::from(*flag),
        Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                integer_hash(integer)
            } else if let Some(unsigned) = number.as_u64() {
                decimal_hash(&unsigned.to_string())
            } else {
                float_hash(number.as_f64().unwrap_or(f64::NAN))
            }
        }
        Value::Null => pointer_hash(),
        Value::Sequence(_) | Value::Mapping(_) => {
            // PyYAML's `construct_mapping` reports a collection key as a
            // `ConstructorError` ("found unhashable key", a `YAMLError`, so the
            // reference skips that one file); ruamel's `construct_mapping`
            // converts a list KEY to a tuple (`tuple(key)`) and only then hashes
            // it, so a *nested* list/dict inside that tuple reaches here as the
            // bare `TypeError` the reference does not catch.
            return Err(if dialect.refuses_duplicate_keys() {
                YamlError::Runtime {
                    cause: RuntimeCause::Type,
                    message: format!("unhashable type: '{}'", python_type_name(value)),
                }
            } else {
                YamlError::Construct(format!(
                    "while constructing a mapping: found unhashable key ({} value)",
                    python_type_name(value)
                ))
            });
        }
        Value::Tagged(tagged) => {
            if super::graph::is_fresh_nan_tag(&tagged.tag) {
                // A fresh `!!float nan` object: distinct from every other float,
                // so its identity hash is its own node address (exactly the
                // `_Py_HashPointer` rule the canonical graph uses).
                pointer_hash_of(std::ptr::from_ref(&**tagged) as usize)
            } else if let Some(bytes) = super::construct::constructed_bytes(value) {
                bytes_hash_with_secret(&bytes, secret)?
            } else if let Some(digits) = super::construct::constructed_big_integer(value) {
                decimal_hash(digits)
            } else if super::construct::is_constructed_timestamp(value) {
                timestamp_hash(tagged.value.as_str().unwrap_or_default(), secret)?
            } else if let Some(items) = super::construct::constructed_tuple(value) {
                tuple_hash(items, markers, dialect, secret)?
            } else {
                return Err(YamlError::Runtime {
                    cause: RuntimeCause::Type,
                    message: format!("unhashable type: '{}'", python_type_name(value)),
                });
            }
        }
    };
    // CPython maps a -1 hash to -2 so it can use -1 as "uninitialised".
    Ok(if hash == -1 { -2 } else { hash })
}

/// Python `repr()` text of one *scalar* constructed value.
///
/// Used for the duplicate-key message ruamel spells with `repr(key)`. Only the
/// scalar kinds a mapping key can carry are rendered exactly; a container key
/// falls back to a named placeholder because its full `repr()` belongs to the
/// model-facing text authority, not to this error path.
pub fn python_repr_text(value: &Value, markers: &BTreeMap<String, PythonString>) -> String {
    match value {
        Value::String(text) => exact_string(text, markers).repr().to_string(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::Null => "None".to_owned(),
        Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                integer.to_string()
            } else if let Some(unsigned) = number.as_u64() {
                unsigned.to_string()
            } else {
                crate::python_json::python_float_repr(number.as_f64().unwrap_or(f64::NAN))
            }
        }
        Value::Tagged(tagged) if super::graph::is_fresh_nan_tag(&tagged.tag) => "nan".to_owned(),
        Value::Tagged(_) => {
            if let Some(bytes) = super::construct::constructed_bytes(value) {
                let mut rendered = String::from("b'");
                for byte in bytes {
                    match byte {
                        b'\\' => rendered.push_str("\\\\"),
                        b'\'' => rendered.push_str("\\'"),
                        0x20..=0x7e => rendered.push(byte as char),
                        other => rendered.push_str(&format!("\\x{other:02x}")),
                    }
                }
                rendered.push('\'');
                rendered
            } else if let Some(digits) = super::construct::constructed_big_integer(value) {
                digits.to_owned()
            } else if let Some(items) = super::construct::constructed_tuple(value) {
                // Python's `repr()`/`str()` of a tuple, e.g. `('a',)` / `()`.
                let mut rendered = String::from("(");
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        rendered.push_str(", ");
                    }
                    rendered.push_str(&python_repr_text(item, markers));
                }
                if items.len() == 1 {
                    rendered.push(',');
                }
                rendered.push(')');
                rendered
            } else if let Some(text) = super::construct::constructed_timestamp_text(value) {
                // `str(date)`/`str(datetime)`; `repr()` is the `datetime.*` form.
                text
            } else {
                "<tagged>".to_owned()
            }
        }
        Value::Sequence(_) | Value::Mapping(_) => "<container>".to_owned(),
    }
}
/// The exact Python string of a transported scalar (marker, else its text).
fn exact_string(text: &str, markers: &BTreeMap<String, PythonString>) -> PythonString {
    match markers.get(text) {
        Some(exact) => exact.clone(),
        None => PythonString::from_scalars(text),
    }
}

/// The Python type name of an unhashable constructed value.
fn python_type_name(value: &Value) -> &'static str {
    match value {
        Value::Sequence(_) => "list",
        Value::Mapping(_) => "dict",
        _ => "object",
    }
}

/// Python `==` between two constructed set members.
pub fn python_value_equal(
    left: &Value,
    right: &Value,
    markers: &BTreeMap<String, PythonString>,
) -> bool {
    match (left, right) {
        (Value::String(left), Value::String(right)) => {
            exact_text(left, markers) == exact_text(right, markers)
        }
        (Value::Bool(left), Value::Bool(right)) => left == right,
        (Value::Null, Value::Null) => true,
        (Value::Sequence(left), Value::Sequence(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(a, b)| python_value_equal(a, b, markers))
        }
        (Value::Mapping(left), Value::Mapping(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, value)| {
                    right.iter().any(|(other_key, other)| {
                        python_value_equal(key, other_key, markers)
                            && python_value_equal(value, other, markers)
                    })
                })
        }
        (Value::Tagged(left), Value::Tagged(right)) => {
            // A fresh `!!float nan` object is never equal to another object;
            // the loader's `.nan` singleton is not tagged and keeps its own
            // (same-object) collapse.
            if super::graph::is_fresh_nan_tag(&left.tag)
                || super::graph::is_fresh_nan_tag(&right.tag)
            {
                return false;
            }
            let left_value = Value::Tagged(left.clone());
            let right_value = Value::Tagged(right.clone());
            let left_bytes = super::construct::constructed_bytes(&left_value);
            let right_bytes = super::construct::constructed_bytes(&right_value);
            match (left_bytes, right_bytes) {
                (Some(left), Some(right)) => return left == right,
                (None, None) => {}
                // `bytes` is never equal to another constructed kind.
                _ => return false,
            }
            let left_tuple = super::construct::constructed_tuple(&left_value);
            let right_tuple = super::construct::constructed_tuple(&right_value);
            match (left_tuple, right_tuple) {
                (Some(left), Some(right)) => {
                    return left.len() == right.len()
                        && left
                            .iter()
                            .zip(right)
                            .all(|(a, b)| python_value_equal(a, b, markers));
                }
                (None, None) => {}
                // A tuple is never equal to another constructed kind.
                _ => return false,
            }
            let left_timestamp = super::construct::is_constructed_timestamp(&left_value);
            let right_timestamp = super::construct::is_constructed_timestamp(&right_value);
            match (left_timestamp, right_timestamp) {
                (true, true) => {
                    // Python compares `date`/`datetime` objects, not their text:
                    // a `date` never equals a `datetime`, a naive `datetime` never
                    // equals an aware one, and two aware datetimes are equal at the
                    // same instant whatever offset they were authored with.
                    let left_text = left.value.as_str().unwrap_or_default();
                    let right_text = right.value.as_str().unwrap_or_default();
                    return match (
                        super::construct::timestamp_identity(left_text),
                        super::construct::timestamp_identity(right_text),
                    ) {
                        (Some(left), Some(right)) => left == right,
                        _ => left_text == right_text,
                    };
                }
                (false, false) => {}
                _ => return false,
            }
            python_value_equal(&left.value, &right.value, markers)
        }
        _ => numeric_equal(left, right),
    }
}

/// Numeric equality across `bool`/`int`/`float` (Python's numeric tower).
fn numeric_equal(left: &Value, right: &Value) -> bool {
    let Some(left) = numeric_value(left) else {
        return false;
    };
    let Some(right) = numeric_value(right) else {
        return false;
    };
    match (left, right) {
        (Numeric::Int(left), Numeric::Int(right)) => left == right,
        // `float('nan')` is never equal to itself in Python, but both Source
        // loaders construct `.nan` from **one** `nan_value` singleton
        // (`construct_yaml_float`), so two `.nan` keys in one document are the
        // *same object* and a dict keeps one entry (probed: PyYAML accepts
        // `{.nan: a, .nan: b}` as `{nan: 'b'}`). Inside one decoded document
        // every NaN therefore denotes that singleton.
        (Numeric::Float(left), Numeric::Float(right)) => {
            left == right || (left.is_nan() && right.is_nan())
        }
        (Numeric::Int(left), Numeric::Float(right))
        | (Numeric::Float(right), Numeric::Int(left)) => match left.parse::<f64>() {
            Ok(left) => left == right,
            Err(_) => false,
        },
    }
}

enum Numeric {
    Int(String),
    Float(f64),
}

fn numeric_value(value: &Value) -> Option<Numeric> {
    match value {
        Value::Bool(flag) => Some(Numeric::Int(i64::from(*flag).to_string())),
        Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                Some(Numeric::Int(normalize_decimal(&integer.to_string())))
            } else if let Some(unsigned) = number.as_u64() {
                Some(Numeric::Int(normalize_decimal(&unsigned.to_string())))
            } else {
                number.as_f64().map(Numeric::Float)
            }
        }
        Value::Tagged(_) => super::construct::constructed_big_integer(value)
            .map(|digits| Numeric::Int(normalize_decimal(digits))),
        _ => None,
    }
}

fn normalize_decimal(digits: &str) -> String {
    match digits.strip_prefix('-') {
        Some(body) if body.trim_start_matches('0').is_empty() => "0".to_owned(),
        Some(body) => format!("-{}", body.trim_start_matches('0')),
        None => {
            let body = digits.trim_start_matches('0');
            if body.is_empty() {
                "0".to_owned()
            } else {
                body.to_owned()
            }
        }
    }
}

fn exact_text(value: &str, markers: &BTreeMap<String, PythonString>) -> Vec<u32> {
    match markers.get(value) {
        Some(exact) => exact.codepoints().to_vec(),
        None => value.chars().map(u32::from).collect(),
    }
}

/// The order in which CPython's set table yields the inserted members.
/// CPython's set table layout: the iteration order of one insertion sequence.
///
/// The table never fails to place a key (it grows instead), so this is total; the
/// `Result` it used to return was always `Ok`.
fn table_order(hashes: &[i64]) -> Vec<usize> {
    let mut table = SetTable::new();
    for (index, hash) in hashes.iter().enumerate() {
        table.insert(*hash, index);
    }
    table.iter_order()
}

struct SetTable {
    slots: Vec<Option<(usize, i64)>>,
    fill: usize,
    used: usize,
}

impl SetTable {
    fn new() -> Self {
        Self {
            slots: vec![None; MIN_SET_SIZE],
            fill: 0,
            used: 0,
        }
    }

    fn mask(&self) -> usize {
        self.slots.len() - 1
    }

    fn insert(&mut self, hash: i64, key_index: usize) {
        // CPython: `if ((size_t)so->fill*5 < mask*3) insert else resize+insert`.
        if self.fill * 5 >= self.mask() * 3 {
            self.resize();
        }
        self.insert_clean(hash, key_index);
    }

    fn resize(&mut self) {
        let target = if self.used > 50_000 {
            self.used * 2
        } else {
            self.used * 4
        };
        let mut new_size = MIN_SET_SIZE;
        while new_size <= target {
            new_size <<= 1;
        }
        let old_slots = std::mem::replace(&mut self.slots, vec![None; new_size]);
        self.fill = 0;
        self.used = 0;
        for (index, hash) in old_slots.into_iter().flatten() {
            self.insert_clean(hash, index);
        }
    }

    fn insert_clean(&mut self, hash: i64, key_index: usize) {
        let mask = self.mask();
        let mut probe = (hash as u64 as usize) & mask;
        let mut perturb = hash as u64;
        loop {
            if self.slots[probe].is_none() {
                self.slots[probe] = Some((key_index, hash));
                self.fill += 1;
                self.used += 1;
                return;
            }
            if probe + LINEAR_PROBES <= mask {
                for offset in 1..=LINEAR_PROBES {
                    let slot = probe + offset;
                    if self.slots[slot].is_none() {
                        self.slots[slot] = Some((key_index, hash));
                        self.fill += 1;
                        self.used += 1;
                        return;
                    }
                }
            }
            perturb >>= PERTURB_SHIFT;
            probe = (probe
                .wrapping_mul(5)
                .wrapping_add(1)
                .wrapping_add(perturb as usize))
                & mask;
        }
    }

    fn iter_order(&self) -> Vec<usize> {
        self.slots
            .iter()
            .filter_map(|slot| slot.map(|(index, _hash)| index))
            .collect()
    }
}

/// CPython's `unicode_hash` for an exact code-point sequence.
/// CPython's `unicode_hash` with an explicit `Py_HashSecret`.
pub fn string_hash_with_secret(
    codepoints: &[u32],
    secret: (u64, u64),
) -> Result<i64, super::error::YamlError> {
    if codepoints.is_empty() {
        return Ok(0);
    }
    // CPython 3 hashes *every* non-empty string with SipHash-1-3; the
    // `ord(ch)` shortcut for a length-1 string is a Python 2 behaviour and gets
    // `{'a','b','c'}` wrong (probed: `hash('a') == 4644417185603328019`).
    let kind = if codepoints.iter().all(|code| *code <= 0xff) {
        1
    } else if codepoints.iter().all(|code| *code <= 0xffff) {
        2
    } else {
        4
    };
    let mut data = Vec::with_capacity(codepoints.len() * kind);
    for code in codepoints {
        match kind {
            1 => data.push(*code as u8),
            2 => data.extend_from_slice(&(*code as u16).to_le_bytes()),
            _ => data.extend_from_slice(&code.to_le_bytes()),
        }
    }
    let (k0, k1) = secret;
    Ok(fix_negative_one(siphash13(k0, k1, &data) as i64))
}

/// CPython's `bytes_hash`.
/// CPython's `bytes_hash` with an explicit `Py_HashSecret`.
pub fn bytes_hash_with_secret(
    bytes: &[u8],
    secret: (u64, u64),
) -> Result<i64, super::error::YamlError> {
    if bytes.is_empty() {
        return Ok(0);
    }
    // No length-1 shortcut: `hash(b'a') == hash('a')` for ASCII, via SipHash.
    let (k0, k1) = secret;
    Ok(fix_negative_one(siphash13(k0, k1, bytes) as i64))
}

fn fix_negative_one(hash: i64) -> i64 {
    if hash == -1 { -2 } else { hash }
}

/// CPython's `long_hash` (an `i64` value).
pub(crate) fn integer_hash(value: i64) -> i64 {
    decimal_hash(&value.to_string())
}

/// CPython's `long_hash` for an exact decimal spelling.
///
/// `x = (x * 10 + digit) mod (2**61 - 1)` over the magnitude, then the sign —
/// exact for any number of digits.
pub(crate) fn decimal_hash(decimal: &str) -> i64 {
    let (negative, body) = match decimal.strip_prefix('-') {
        Some(body) => (true, body),
        None => (false, decimal.strip_prefix('+').unwrap_or(decimal)),
    };
    let mut hash: u64 = 0;
    for character in body.chars() {
        let Some(digit) = character.to_digit(10) else {
            // A non-decimal spelling cannot come from the constructor; the
            // fallback keeps the hash deterministic instead of panicking.
            return pointer_hash();
        };
        hash = ((u128::from(hash) * 10 + u128::from(digit)) % u128::from(HASH_MODULUS)) as u64;
    }
    if negative && hash != 0 {
        fix_negative_one(-(hash as i64))
    } else {
        fix_negative_one(hash as i64)
    }
}

/// CPython's `_Py_HashDouble`.
pub(crate) fn float_hash(value: f64) -> i64 {
    if !value.is_finite() {
        return if value.is_infinite() {
            if value > 0.0 { HASH_INF } else { -HASH_INF }
        } else {
            pointer_hash()
        };
    }
    let (mut mantissa, mut exponent) = frexp(value);
    let sign = if mantissa < 0.0 {
        mantissa = -mantissa;
        -1i64
    } else {
        1i64
    };
    // Process 28 bits at a time, exactly like `_Py_HashDouble`.
    let mut x: u64 = 0;
    while mantissa != 0.0 {
        x = ((x << 28) & HASH_MODULUS) | (x >> (HASH_BITS - 28));
        mantissa *= 268_435_456.0; // 2**28
        exponent -= 28;
        let part = mantissa as u64; // truncation, exactly like C
        mantissa -= part as f64;
        x = x.wrapping_add(part);
        if x >= HASH_MODULUS {
            x -= HASH_MODULUS;
        }
    }
    // Reduce the exponent modulo `_PyHASH_BITS`.
    let shift = if exponent >= 0 {
        (exponent as u32) % HASH_BITS
    } else {
        HASH_BITS - 1 - (((-1 - exponent) as u32) % HASH_BITS)
    };
    x = ((x << shift) & HASH_MODULUS) | (x >> (HASH_BITS - shift));
    let value = x as i64;
    fix_negative_one(if sign < 0 {
        value.wrapping_neg()
    } else {
        value
    })
}

/// `math.frexp`: `value = mantissa * 2**exponent` with `0.5 <= |mantissa| < 1`.
fn frexp(value: f64) -> (f64, i32) {
    if value == 0.0 || !value.is_finite() {
        return (value, 0);
    }
    let bits = value.to_bits();
    let raw = ((bits >> 52) & 0x7ff) as i32;
    if raw == 0 {
        // Subnormal: normalize by exact doublings (at most 52 of them).
        let mut scaled = value;
        let mut exponent = 0i32;
        while scaled.abs() < 0.5 {
            scaled *= 2.0;
            exponent -= 1;
        }
        return (scaled, exponent);
    }
    let mantissa_bits = (bits & !(0x7ffu64 << 52)) | (1022u64 << 52);
    (f64::from_bits(mantissa_bits), raw - 1022)
}

/// The hash of a constructed timestamp (a Python `date`/`datetime`).
///
/// CPython v3.11.15 `_datetimemodule.c::{date_hash,datetime_hash,delta_hash}`:
/// dates and naive datetimes hash their packed object data with `_Py_HashBytes`;
/// aware datetimes hash `(ordinal_days, seconds, microseconds)` after subtracting
/// the fixed timezone offset and normalizing the resulting timedelta.
pub(crate) fn timestamp_hash(text: &str, secret: (u64, u64)) -> Result<i64, YamlError> {
    use super::construct::{TimestampIdentity, timestamp_identity, timestamp_storage_bytes};
    match timestamp_identity(text) {
        Some(TimestampIdentity::Date { .. } | TimestampIdentity::Naive { .. }) => {
            let bytes = timestamp_storage_bytes(text).ok_or_else(|| {
                YamlError::Construct("invalid constructed timestamp storage".to_owned())
            })?;
            bytes_hash_with_secret(&bytes, secret)
        }
        Some(TimestampIdentity::Aware { seconds, micro }) => {
            // Python's ordinal of 1970-01-01 is 719163. Euclidean division
            // preserves normalization even for year1 plus an eastward offset.
            let days = seconds.div_euclid(86_400) + 719_163;
            let seconds = seconds.rem_euclid(86_400);
            Ok(xx_hash(&[
                integer_hash(days),
                integer_hash(seconds),
                i64::from(micro),
            ]))
        }
        None => Err(YamlError::Construct(
            "invalid constructed timestamp".to_owned(),
        )),
    }
}

/// The xxHash accumulator CPython uses for tuples.
pub(crate) fn xx_hash(components: &[i64]) -> i64 {
    let mut accumulator: u64 = XXPRIME_5;
    for lane in components {
        accumulator = accumulator.wrapping_add((*lane as u64).wrapping_mul(XXPRIME_2));
        accumulator = accumulator.rotate_left(31);
        accumulator = accumulator.wrapping_mul(XXPRIME_1);
    }
    accumulator = accumulator.wrapping_add((components.len() as u64) ^ (XXPRIME_5 ^ 3_527_539));
    fix_negative_one(accumulator as i64)
}

/// CPython's pointer hash, from a stable process-local address.
///
/// `_Py_HashPointer` rotates the object address right by four bits and maps
/// `-1` to `-2`. The tree projection still hashes its synthesized `None`/`nan`
/// scalar with one process-static address (a named open case); the canonical
/// object graph uses [`pointer_hash_of`] with the **real** address of the object
/// it is hashing.
fn pointer_hash() -> i64 {
    let address = std::ptr::addr_of!(POINTER_SENTINEL) as usize;
    fix_negative_one((address >> 4) as i64)
}

/// CPython v3.11.15 `_Py_HashPointer` for one object address.
///
/// ```c
/// uintptr_t x = (uintptr_t)p;
/// x = (x >> 4) | (x << (8 * sizeof(void *) - 4));
/// if (x == (uintptr_t)-1) x = (uintptr_t)-2;
/// ```
/// The rotation (not an arithmetic shift) is load-bearing for the low bits a
/// `dict`/`set` probe consumes, and the `-1` remap keeps `-1` meaning
/// "uninitialised" to CPython. Addresses and their derived order are properties
/// of **this** process; no other process's addresses are invented or compared.
pub(crate) fn pointer_hash_of(address: usize) -> i64 {
    let bits = usize::BITS;
    let rotated = (address >> 4) | (address << (bits - 4));
    fix_negative_one(rotated as i64)
}

/// The `Py_HashSecret` SipHash keys this process uses.
pub(crate) fn hash_secret() -> Result<(u64, u64), HashSeedError> {
    match std::env::var_os("PYTHONHASHSEED") {
        None => Ok(*secret_cell().get_or_init(secret_from_os)),
        Some(value) => {
            let Ok(text) = value.into_string() else {
                // A non-UTF-8 value is not `random` and not a decimal integer.
                return Err(HashSeedError);
            };
            if text == "random" {
                return Ok(*secret_cell().get_or_init(secret_from_os));
            }
            let seed = parse_hash_seed(&text).ok_or(HashSeedError)?;
            Ok(if seed == 0 { (0, 0) } else { lcg_urandom(seed) })
        }
    }
}

/// The process-owned secret, initialised once (CPython's secret is per process).
fn secret_cell() -> &'static OnceLock<(u64, u64)> {
    static SECRET: OnceLock<(u64, u64)> = OnceLock::new();
    &SECRET
}

/// 16 bytes of OS entropy, derived through the standard library's keyed hasher.
///
/// `RandomState` seeds itself from the operating system's entropy source once
/// per process; hashing two domain-separated values yields the two SipHash keys
/// without a platform-specific `/dev/urandom` read or a new dependency.
fn secret_from_os() -> (u64, u64) {
    use std::hash::BuildHasher;
    let state = std::collections::hash_map::RandomState::new();
    (state.hash_one(0u8), state.hash_one(1u8))
}

/// CPython's `strtoul`-based `PYTHONHASHSEED` parse.
///
/// Leading white space is skipped and a sign is accepted (a negative value
/// wraps to a huge unsigned value, which is out of range unless it is `-0`);
/// the rest of the string must be decimal digits, and the value must fit in
/// `u32`.
fn parse_hash_seed(text: &str) -> Option<u32> {
    let trimmed = text.trim_start_matches([' ', '\t', '\n', '\r', '\x0b', '\x0c']);
    let (negative, body) = match trimmed.as_bytes().first() {
        Some(b'-') => (true, &trimmed[1..]),
        Some(b'+') => (false, &trimmed[1..]),
        _ => (false, trimmed),
    };
    if body.is_empty() || !body.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let magnitude = body.parse::<u128>().ok()?;
    if negative {
        // `strtoul` negates the unsigned magnitude; only -0 stays in range.
        return if magnitude == 0 { Some(0) } else { None };
    }
    u32::try_from(magnitude).ok()
}

/// CPython's `lcg_urandom` key expansion.
fn lcg_urandom(seed: u32) -> (u64, u64) {
    let mut state = seed;
    let mut bytes = [0u8; 16];
    for byte in &mut bytes {
        state = state.wrapping_mul(214_013).wrapping_add(2_531_011);
        *byte = ((state >> 16) & 0xff) as u8;
    }
    let mut first = [0u8; 8];
    let mut second = [0u8; 8];
    first.copy_from_slice(&bytes[..8]);
    second.copy_from_slice(&bytes[8..]);
    (u64::from_le_bytes(first), u64::from_le_bytes(second))
}

/// CPython's `pysiphash` (SipHash-1-3).
fn siphash13(k0: u64, k1: u64, data: &[u8]) -> u64 {
    let mut v0 = k0 ^ 0x736f_6d65_7073_6575;
    let mut v1 = k1 ^ 0x646f_7261_6e64_6f6d;
    let mut v2 = k0 ^ 0x6c79_6765_6e65_7261;
    let mut v3 = k1 ^ 0x7465_6462_7974_6573;

    let mut b = (data.len() as u64) << 56;
    let mut offset = 0usize;
    while offset + 8 <= data.len() {
        let word = u64::from_le_bytes(
            data[offset..offset + 8]
                .try_into()
                .expect("eight-byte word"),
        );
        v3 ^= word;
        sip_round(&mut v0, &mut v1, &mut v2, &mut v3);
        v0 ^= word;
        offset += 8;
    }
    for (position, byte) in data[offset..].iter().enumerate() {
        b |= u64::from(*byte) << (8 * position);
    }
    v3 ^= b;
    sip_round(&mut v0, &mut v1, &mut v2, &mut v3);
    v0 ^= b;
    v2 ^= 0xff;
    sip_round(&mut v0, &mut v1, &mut v2, &mut v3);
    sip_round(&mut v0, &mut v1, &mut v2, &mut v3);
    sip_round(&mut v0, &mut v1, &mut v2, &mut v3);
    v0 ^ v1 ^ v2 ^ v3
}

fn sip_round(v0: &mut u64, v1: &mut u64, v2: &mut u64, v3: &mut u64) {
    *v0 = v0.wrapping_add(*v1);
    *v1 = v1.rotate_left(13);
    *v1 ^= *v0;
    *v0 = v0.rotate_left(32);
    *v2 = v2.wrapping_add(*v3);
    *v3 = v3.rotate_left(16);
    *v3 ^= *v2;
    *v0 = v0.wrapping_add(*v3);
    *v3 = v3.rotate_left(21);
    *v3 ^= *v0;
    *v2 = v2.wrapping_add(*v1);
    *v1 = v1.rotate_left(17);
    *v1 ^= *v2;
    *v2 = v2.rotate_left(32);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_and_float_hashes_follow_cpython() {
        // Published facts from the pinned CPython 3.11.15 oracle.
        assert_eq!(decimal_hash("0"), 0);
        assert_eq!(decimal_hash("100"), 100);
        assert_eq!(decimal_hash("-1"), -2);
        assert_eq!(decimal_hash("2305843009213693951"), 0, "the modulus");
        assert_eq!(decimal_hash("2305843009213693952"), 1);
        assert_eq!(decimal_hash("-2305843009213693952"), -2);
        assert_eq!(
            decimal_hash("1000000000000000000000000000000"),
            465_258_685_558_744_706
        );
        assert_eq!(float_hash(0.0), 0);
        assert_eq!(float_hash(1.5), 1_152_921_504_606_846_977);
        assert_eq!(float_hash(-1.5), -1_152_921_504_606_846_977);
        assert_eq!(float_hash(3.0), 3);
        assert_eq!(float_hash(1e300), 1_224_995_262_755_759_164);
        assert_eq!(float_hash(0.1), 230_584_300_921_369_408);
        assert_eq!(float_hash(2f64.powi(61)), 1);
        assert_eq!(float_hash(f64::INFINITY), HASH_INF);
        assert_eq!(float_hash(f64::NEG_INFINITY), -HASH_INF);
    }

    #[test]
    fn unseeded_secret_is_never_the_all_zero_seed() {
        // The process secret is OS entropy; seed 0 is the all-zero key pair. The
        // two must not be conflated (that was the old silent fallback).
        let secret = secret_from_os();
        assert_ne!(secret, (0, 0));
    }

    #[test]
    fn invalid_hash_seeds_are_refused_like_cpython() {
        assert_eq!(parse_hash_seed("abc"), None);
        assert_eq!(parse_hash_seed("-1"), None);
        assert_eq!(parse_hash_seed("4294967296"), None);
        assert_eq!(parse_hash_seed(""), None);
        assert_eq!(parse_hash_seed("-0"), Some(0));
        assert_eq!(
            parse_hash_seed(" 12 "),
            None,
            "trailing space is strtoul's end"
        );
        assert_eq!(parse_hash_seed(" 12"), Some(12));
        assert_eq!(parse_hash_seed("0"), Some(0));
        assert_eq!(parse_hash_seed("4294967295"), Some(4_294_967_295));
    }

    #[test]
    fn unhashable_members_report_the_dialect_outcome() {
        let markers = BTreeMap::new();
        for (value, expected) in [
            (
                Value::Sequence(vec![Value::Null]),
                "unhashable type: 'list'",
            ),
            (
                Value::Mapping(Default::default()),
                "unhashable type: 'dict'",
            ),
        ] {
            // ruamel's safe constructor hashes the member directly: a bare
            // `TypeError` the reference does not catch.
            match member_hash(
                &value.clone(),
                &markers,
                super::super::Dialect::RuamelSafe12,
                (0, 0),
            ) {
                Err(YamlError::Runtime { cause, message }) => {
                    assert_eq!(cause, RuntimeCause::Type);
                    assert_eq!(message, expected);
                }
                other => panic!("expected a TypeError, got {other:?}"),
            }
            // PyYAML reports it as a `ConstructorError` instead.
            assert!(matches!(
                member_hash(
                    &value.clone(),
                    &markers,
                    super::super::Dialect::PyYamlSafe11,
                    (0, 0)
                ),
                Err(YamlError::Construct(_))
            ));
        }
    }

    #[test]
    fn tuple_hashes_match_pinned_cpython_receipts() {
        // `PYTHONHASHSEED=0` facts from the pinned oracle (probe
        // `hashfacts`): `hash(())`, `hash(('a',))` and `hash((('a',),))`.
        let secret = (0, 0);
        let markers = BTreeMap::new();
        let string = |text: &str| Value::String(text.to_owned());
        let one = super::super::construct::tuple_value(vec![string("a")]);
        let empty = super::super::construct::tuple_value(vec![]);
        let nested = super::super::construct::tuple_value(vec![one.clone()]);
        for (value, expected) in [
            (empty, 5_740_354_900_026_072_187_i64),
            (one, 7_319_529_274_390_396_360),
            (nested, 3_245_582_265_640_510_847),
        ] {
            assert_eq!(
                member_hash(
                    &value,
                    &markers,
                    super::super::Dialect::RuamelSafe12,
                    secret
                )
                .unwrap(),
                expected,
                "{value:?}"
            );
        }
    }

    #[test]
    fn timestamp_hashes_match_actual_cpython_seed_zero_facts() {
        for (text, hash) in [
            ("2026-11-05", 7_311_123_894_698_728_974_i64),
            ("2026-11-05 00:00:00", 7_281_516_447_428_217_606),
            ("2026-11-05 00:00:00+00:00", -2_210_446_007_498_203_512),
            ("2026-11-05 01:00:00+01:00", -2_210_446_007_498_203_512),
        ] {
            assert_eq!(timestamp_hash(text, (0, 0)).unwrap(), hash, "{text}");
        }
    }

    #[test]
    fn the_loader_nan_is_one_object() {
        // Both loaders construct `.nan` from a single `nan_value`, so two `.nan`
        // keys in one document are the same object and a dict keeps one entry.
        let markers = BTreeMap::new();
        assert!(python_value_equal(
            &Value::Number(serde_yaml_ng::Number::from(f64::NAN)),
            &Value::Number(serde_yaml_ng::Number::from(f64::NAN)),
            &markers,
        ));
        // ...but a NaN is never equal to an ordinary float.
        assert!(!python_value_equal(
            &Value::Number(serde_yaml_ng::Number::from(f64::NAN)),
            &Value::Number(serde_yaml_ng::Number::from(1.0)),
            &markers,
        ));
    }

    #[test]
    fn timestamps_compare_as_python_objects() {
        use super::super::construct::{TimestampIdentity, timestamp_identity};
        // A `date` is never equal to a `datetime` on the same day.
        assert_eq!(
            timestamp_identity("2026-11-05"),
            Some(TimestampIdentity::Date {
                year: 2026,
                month: 11,
                day: 5
            })
        );
        assert_ne!(
            timestamp_identity("2026-11-05"),
            timestamp_identity("2026-11-05T00:00:00")
        );
        // Two aware datetimes at the same instant are equal whatever the offset.
        assert_eq!(
            timestamp_identity("2026-11-05T00:00:00+00:00"),
            timestamp_identity("2026-11-05T01:00:00+01:00")
        );
        // A naive datetime is never equal to an aware one.
        assert_ne!(
            timestamp_identity("2026-11-05T00:00:00"),
            timestamp_identity("2026-11-05T00:00:00Z")
        );
    }

    #[test]
    fn constructed_set_order_matches_a_pinned_cpython_receipt() {
        // `PYTHONHASHSEED=0` receipt (evidence 09): `!!set {a: null, b: null,
        // c: null}` iterates c, a, b in CPython 3.11.15.
        let markers = BTreeMap::new();
        let keys = vec![
            Value::String("a".to_owned()),
            Value::String("b".to_owned()),
            Value::String("c".to_owned()),
        ];
        let ordered =
            set_order_with_secret(keys, &markers, super::super::Dialect::PyYamlSafe11, (0, 0))
                .unwrap();
        assert_eq!(
            ordered
                .iter()
                .map(|value| value.as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["c", "a", "b"]
        );
    }

    /// The same table owner orders a `frozenset[str]` built from an activation list.
    ///
    /// Both receipts were measured from the pinned Source (`frozenset(<authored
    /// list>)` iterated by `_gated`) before this native capability existed, so the
    /// order is CPython's table order — not the authored order and not sorted.
    #[test]
    fn string_set_order_matches_pinned_source_receipts() {
        let tokens = |seed: u32| {
            let mut keys: Vec<PythonString> = vec![PythonString::from_scalars("alpha")];
            for number in 1..=12 {
                keys.push(PythonString::from_scalars(&format!("t{number:02}")));
            }
            let secret = if seed == 0 { (0, 0) } else { lcg_urandom(seed) };
            string_set_order_with_secret(keys, secret)
                .iter()
                .map(|token| token.to_scalar_string().unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            tokens(0),
            vec![
                "t07", "alpha", "t04", "t02", "t01", "t05", "t12", "t03", "t06", "t10", "t09",
                "t11", "t08"
            ]
        );
        assert_eq!(
            tokens(42),
            vec![
                "t04", "t09", "t08", "t10", "t02", "t03", "t01", "t11", "t06", "t07", "t05", "t12",
                "alpha"
            ]
        );
        let mut sorted = tokens(0);
        sorted.sort();
        assert_ne!(tokens(0), sorted, "never a sorted substitute");
        assert_ne!(tokens(0), tokens(42), "the secret decides the order");
    }
}
