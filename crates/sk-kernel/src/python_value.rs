//! The workspace's one canonical Python value domain.
//!
//! Source's YAML readers produce **Python objects**, not JSON: a document may
//! contain `None`, `bool`, arbitrary-precision `int`, `float`, `str` (including
//! unpaired surrogates), `bytes` (`!!binary`), `date`/`datetime` (`!!timestamp`),
//! `set` (`!!set`), `list`, `dict`, and (for `!!pairs`/`!!omap`) a list of
//! tuples or an `ordereddict`. Validation then judges those *Python* values and
//! its diagnostics name the Python type, so the domain has to be explicit: a
//! narrowed Rust struct would silently change acceptance.
//!
//! There is exactly one such type. `sk-mission` re-exports it under its old
//! names ([`PythonValue`] as `SourceValue`, [`OrderedMap`] as itself), so the
//! definition model, the path-convention reader and the Kernel decoder share one
//! within-build TypeId — no second DTO, no lossy bridge.
//!
//! Strings are [`PythonString`] (code points, never bytes) because a CPython
//! `str` may hold a lone surrogate; integers are canonical decimal text so
//! arbitrary precision survives; dates and datetimes carry CPython's `str()`
//! spelling (`2026-11-05`, `2001-12-15 02:59:43.100000-05:00`).

use crate::PythonString;

/// One value produced by a Python-compatible YAML safe loader.
#[derive(Clone, Debug, PartialEq)]
pub enum PythonValue {
    /// `None`
    Null,
    /// `bool`
    Bool(bool),
    /// `int` — canonical decimal text, so arbitrary precision survives.
    Int(String),
    /// `float`
    Float(f64),
    /// `str` — code points, including unpaired surrogates.
    Str(PythonString),
    /// `bytes` (`!!binary`)
    Bytes(Vec<u8>),
    /// `datetime.date` — CPython `str()` spelling.
    Date(String),
    /// `datetime.datetime` — CPython `str()` spelling.
    DateTime(String),
    /// `set` — members in CPython's hash-table iteration order.
    Set(Vec<PythonValue>),
    /// `list`
    Seq(Vec<PythonValue>),
    /// `tuple` — a real Python tuple, not a formatted string.
    ///
    /// ruamel's safe constructor converts a **list mapping key** with
    /// `tuple(key)` (`constructor.py`, `construct_mapping`), so a key such as
    /// `? [a, b]` is the tuple `('a', 'b')`: it has Python's tuple hash and
    /// equality, and it `repr()`s as `('a', 'b')`. The members are the loader's
    /// own constructed values; nothing here is a business identifier, an
    /// escaped spelling or a transport artifact.
    Tuple(Vec<PythonValue>),
    /// `dict` — authored insertion order, keys in their constructed value form.
    Map(Vec<(PythonValue, PythonValue)>),
    /// A Python `list` of `(key, value)` tuples (`!!pairs`, PyYAML's `!!omap`).
    Pairs(Vec<(PythonValue, PythonValue)>),
    /// A Python `ordereddict` (ruamel's `!!omap`).
    OrderedDict(Vec<(PythonValue, PythonValue)>),
}

impl PythonValue {
    /// The Python type name Pydantic uses in its diagnostics.
    pub fn python_type_name(&self) -> &'static str {
        match self {
            Self::Null => "NoneType",
            Self::Bool(_) => "bool",
            Self::Int(_) => "int",
            Self::Float(_) => "float",
            Self::Str(_) => "str",
            Self::Bytes(_) => "bytes",
            Self::Date(_) => "date",
            Self::DateTime(_) => "datetime",
            Self::Set(_) => "set",
            Self::Seq(_) => "list",
            Self::Tuple(_) => "tuple",
            Self::Map(_) | Self::OrderedDict(_) => "dict",
            Self::Pairs(_) => "list",
        }
    }

    /// The value as an authored Python `str`, when it is one.
    pub fn as_str(&self) -> Option<&PythonString> {
        match self {
            Self::Str(text) => Some(text),
            _ => None,
        }
    }

    /// The entries of a Python mapping (`dict` or `ordereddict`).
    pub fn as_mapping(&self) -> Option<&[(PythonValue, PythonValue)]> {
        match self {
            Self::Map(entries) | Self::OrderedDict(entries) => Some(entries),
            _ => None,
        }
    }

    /// One mapping entry by exact Python string key.
    pub fn mapping_entry(&self, key: &str) -> Option<&PythonValue> {
        self.as_mapping()?
            .iter()
            .find_map(|(candidate, value)| match candidate {
                Self::Str(text) if text.to_scalar_string().ok().as_deref() == Some(key) => {
                    Some(value)
                }
                _ => None,
            })
    }

    /// pydantic's lax `bool` interpretation (probed against Source): the real
    /// booleans, the integers `0`/`1`, and the strings CPython's `bool()`
    /// vocabulary accepts case-insensitively.
    ///
    /// The rule itself lives once in `python_yaml::view::lax_bool`; this
    /// is the legacy-domain adapter over it.
    pub fn as_bool_lax(&self) -> Option<bool> {
        crate::python_yaml::view::lax_bool(
            match self {
                Self::Bool(value) => Some(*value),
                _ => None,
            },
            match self {
                Self::Int(text) => Some(text.as_str()),
                _ => None,
            },
            match self {
                Self::Float(value) => Some(*value),
                _ => None,
            },
            match self {
                Self::Str(text) => text.to_scalar_string().ok(),
                _ => None,
            },
        )
    }

    /// The authored text of a `str` or `bytes` value, following pydantic's lax
    /// `str` decoding of `bytes` (strict UTF-8; a failure is not a `str`).
    ///
    /// The rule itself lives once in `python_yaml::view::lax_str`.
    pub fn as_lax_str(&self) -> Option<PythonString> {
        crate::python_yaml::view::lax_str(
            match self {
                Self::Str(text) => Some(text),
                _ => None,
            },
            match self {
                Self::Bytes(bytes) => Some(bytes.as_slice()),
                _ => None,
            },
        )
    }

    /// Python truthiness (`bool(value)`), through the one shared rule.
    pub fn is_falsy(&self) -> bool {
        use crate::python_yaml::view::Truthiness;
        crate::python_yaml::view::falsy(match self {
            Self::Null => Truthiness::Null,
            Self::Bool(flag) => Truthiness::Bool(*flag),
            Self::Int(text) => Truthiness::Int(text),
            Self::Float(value) => Truthiness::Float(*value),
            Self::Str(text) => Truthiness::Str(text),
            Self::Bytes(bytes) => Truthiness::Bytes(bytes),
            Self::Date(_) | Self::DateTime(_) => Truthiness::AlwaysTrue,
            Self::Set(items) | Self::Seq(items) | Self::Tuple(items) => {
                Truthiness::Sized(items.len())
            }
            Self::Map(entries) | Self::OrderedDict(entries) | Self::Pairs(entries) => {
                Truthiness::Sized(entries.len())
            }
        })
    }

    /// The items of a `list`, when the value is one.
    pub fn as_seq(&self) -> Option<&[PythonValue]> {
        match self {
            Self::Seq(items) => Some(items),
            _ => None,
        }
    }
}

/// An ordered map with Python `dict` semantics for string keys.
///
/// Overwriting an existing key replaces its value in place, keeping the
/// position the key was first authored at — exactly what `dict[key] = value`
/// does, which is what the validator's declaration order depends on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OrderedMap<V> {
    entries: Vec<(PythonString, V)>,
}

impl<V> OrderedMap<V> {
    /// An empty map.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Insert (or overwrite in place).
    pub fn insert(&mut self, key: PythonString, value: V) {
        match self
            .entries
            .iter_mut()
            .find(|(existing, _)| *existing == key)
        {
            Some(slot) => slot.1 = value,
            None => self.entries.push((key, value)),
        }
    }

    /// Look one key up.
    pub fn get(&self, key: &PythonString) -> Option<&V> {
        self.entries
            .iter()
            .find(|(existing, _)| existing == key)
            .map(|(_, value)| value)
    }

    /// Whether the key is present.
    pub fn contains_key(&self, key: &PythonString) -> bool {
        self.get(key).is_some()
    }

    /// Entries in authored order.
    pub fn iter(&self) -> impl Iterator<Item = (&PythonString, &V)> {
        self.entries.iter().map(|(key, value)| (key, value))
    }

    /// The entries as a slice (authored order).
    pub fn entries(&self) -> &[(PythonString, V)] {
        &self.entries
    }

    /// Keys in authored order.
    pub fn keys(&self) -> impl Iterator<Item = &PythonString> {
        self.entries.iter().map(|(key, _)| key)
    }

    /// Values in authored order.
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.entries.iter().map(|(_, value)| value)
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the map is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl OrderedMap<PythonString> {
    /// Build from ordered pairs (later duplicates overwrite in place).
    pub fn from_pairs(pairs: impl IntoIterator<Item = (PythonString, PythonString)>) -> Self {
        let mut map = Self::new();
        for (key, value) in pairs {
            map.insert(key, value);
        }
        map
    }
}
