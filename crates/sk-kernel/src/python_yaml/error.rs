//! Error model of the canonical Python-safe YAML reader.
//!
//! Source's two loaders distinguish, and this module keeps, the four outcome
//! classes that their *callers* branch on:
//!
//! * a **YAML error** (`yaml.YAMLError`: scanner/parser/composer/reader,
//!   `ConstructorError`, ruamel's `DuplicateKeyError`) — the reference's
//!   per-file readers catch these and skip that one file;
//! * a **Python runtime exception the loader does not catch** (`KeyError` from
//!   an invalid `!!bool` spelling, `ValueError` from `int()`/`float()`/an
//!   impossible date, `TypeError` from an unhashable set member, `AssertionError`
//!   from a duplicate `!!omap` key) — these abort the whole load;
//! * a **transport** failure of the lossless carrier (a quoted scalar that
//!   cannot be decoded exactly, an unregistered internal marker);
//! * a **process** fatal (`PYTHONHASHSEED`), which CPython raises before main.
//!
//! The exact Python class is carried by the variant; the *text* of a syntax or
//! constructor error is the semantic reason with the class name, not a
//! byte-identical reproduction of libyaml's caret/mark block (that prose is
//! produced by the C scanner and is listed as an open case in the L4 report).

use crate::python_yaml::set_order::HashSeedError;

/// A Python exception the reference's loader does **not** catch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeCause {
    /// `KeyError` (an invalid `!!bool` spelling).
    Key,
    /// `ValueError` (`int()`, `float()`, an impossible `datetime`, bad bytes).
    Value,
    /// `TypeError` (an unhashable value where Source hashes one).
    Type,
    /// `AssertionError` (a duplicate `!!omap` key).
    Assertion,
}

impl RuntimeCause {
    /// The Python exception name the reference's traceback ends with.
    pub fn python_name(self) -> &'static str {
        match self {
            Self::Key => "KeyError",
            Self::Value => "ValueError",
            Self::Type => "TypeError",
            Self::Assertion => "AssertionError",
        }
    }
}

/// A failure of the canonical Python-safe YAML reader.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum YamlError {
    /// The reader/scanner/parser/composer refused the document (`yaml.YAMLError`
    /// outside construction: `ScannerError`, `ParserError`, `ComposerError`,
    /// `ReaderError` — an input-encoding or control-character refusal).
    Syntax(String),
    /// A double-quoted scalar could not be decoded exactly.
    Decode(String),
    /// A transport marker was not registered by the scalar transport.
    ForeignMarker(String),
    /// A merge-key declaration the reference's constructor refuses.
    Merge(String),
    /// The reference's safe constructor refuses this tag/node
    /// (`ConstructorError`), or ruamel refuses a duplicate key
    /// (`DuplicateKeyError`, a `YAMLError` subclass).
    Construct(String),
    /// A Python exception the loader does not catch; it aborts the whole load.
    Runtime {
        /// The Python exception class.
        cause: RuntimeCause,
        /// The exception's message.
        message: String,
    },
    /// `PYTHONHASHSEED` is unusable, so CPython would refuse to start.
    HashSeed(HashSeedError),
}

/// The coarse outcome class a caller branches on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorClass {
    /// A `yaml.YAMLError` (skip that file).
    Yaml,
    /// A Python exception outside the loader's `except` clause (abort).
    Runtime,
    /// The lossless carrier could not carry the authored text (skip).
    Transport,
    /// A process-level fatal (CPython start-up).
    Process,
}

impl YamlError {
    /// The outcome class Source's callers branch on.
    pub fn class(&self) -> ErrorClass {
        match self {
            Self::Syntax(_) | Self::Merge(_) | Self::Construct(_) => ErrorClass::Yaml,
            Self::Decode(_) | Self::ForeignMarker(_) => ErrorClass::Transport,
            Self::Runtime { .. } => ErrorClass::Runtime,
            Self::HashSeed(_) => ErrorClass::Process,
        }
    }

    /// The Python exception class name, when the outcome names one.
    pub fn python_class(&self) -> &'static str {
        match self {
            Self::Syntax(_) => "YAMLError",
            Self::Merge(_) | Self::Construct(_) => "ConstructorError",
            Self::Decode(_) | Self::ForeignMarker(_) => "YAMLError",
            Self::Runtime { cause, .. } => cause.python_name(),
            Self::HashSeed(_) => "Fatal Python error",
        }
    }
}

impl From<HashSeedError> for YamlError {
    fn from(error: HashSeedError) -> Self {
        Self::HashSeed(error)
    }
}

impl std::fmt::Display for YamlError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Syntax(message) => write!(formatter, "YAML syntax error: {message}"),
            Self::Decode(message) => write!(formatter, "quoted scalar error: {message}"),
            Self::ForeignMarker(text) => write!(
                formatter,
                "unregistered internal profile transport marker {}",
                crate::PythonString::from_scalars(text).repr()
            ),
            Self::Merge(message) => formatter.write_str(message),
            Self::Construct(message) => formatter.write_str(message),
            Self::Runtime { cause, message } => {
                write!(formatter, "{}: {message}", cause.python_name())
            }
            Self::HashSeed(error) => std::fmt::Display::fmt(error, formatter),
        }
    }
}

impl std::error::Error for YamlError {}
