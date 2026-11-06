//! Read-only store for `decisions/index.json`.
//!
//! # What this is
//!
//! Source (`specify_cli.decisions.store.load_index`) resolves
//! `kitty-specs/<mission>/decisions/index.json` in exactly three steps:
//!
//! 1. `path.exists()` — a MISSING file is the benign EMPTY index
//!    (`DecisionIndex(mission_id="", entries=())`), never an error;
//! 2. `path.read_bytes()` + the canonical metadata byte decoder
//!    (`kernel.meta_decode.decode_meta(on_malformed="raise")`, i.e. explicit
//!    strict UTF-8 decode and then one `json.loads`) — a read failure, invalid
//!    UTF-8, malformed JSON or a non-object top level is a `DecisionIndexReadError`;
//! 3. `DecisionIndex.model_validate` — a schema failure is the same
//!    `DecisionIndexReadError`, carrying the pydantic cause.
//!
//! This module is that path, natively, and nothing more. It parses with the
//! Kernel's canonical Python `json.loads` reader, decodes bytes with the Kernel's
//! canonical strict UTF-8 decoder, and validates with the ONE typed model in
//! [`crate::decision_model`] — the same model the merge driver projects through.
//! It does NOT call the merge driver, the raw union or any serializer, and it is
//! NOT a second validator.
//!
//! # Read-only by construction
//!
//! Nothing here writes: no `mkdir`, no temp file, no lock, no timestamp touch, no
//! repair, no `save_index`. A read of an absent or corrupt index leaves the tree
//! byte-identical, which the paired tests assert against a physical footprint.
//!
//! # Deliberate non-claims
//!
//! * Official parity is claimed only for what is Source-deterministic here: the
//!   three-step phase order, the missing-is-empty rule, the read/schema failure
//!   boundary and its typed cause classes, the exact entries and their order, and
//!   the status values. The NON-UTF-8 and JSON failure TEXT is CPython-exact
//!   (both come from Kernel authorities that reproduce the interpreter text); the
//!   `OSError` text and the pydantic `ValidationError` traceback prose are NOT
//!   reproduced (F11 error-wire family, still open) and are disclosed as Native
//!   text, never faked.
//! * `Path.exists()` ignores `ENOENT`/`ENOTDIR`/`EBADF`/`ELOOP`, plus its
//!   separately caught ValueError for an invalid NUL path. `OSError(EINVAL)`
//!   is NOT absent. The shared Kernel stat policy uses platform errno constants
//!   and propagates other failures before the corrupt-index boundary.
//! * Source reads `index.json` to EOF with no size bound or regular-file
//!   precondition. The shared Source byte-stream reader therefore follows
//!   symlinks and may block on a FIFO, just like Source; bounded regular-file
//!   inspection remains a separate Kernel capability.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sk_kernel::PythonString;
use sk_kernel::python_json::{PythonJsonError, PythonJsonValue, parse};
use sk_kernel::python_utf8::{PythonUtf8DecodeError, decode_utf8};
use sk_kernel::{KernelError, read_source_bytes};

use crate::decision_model::{
    DecisionEntry, DecisionIndex, DecisionModelError, DecisionStatus, validate_index,
};

/// The ledger directory name inside a Mission directory.
const DECISIONS_DIRECTORY: &str = "decisions";

/// The ledger index file name.
const INDEX_FILE_NAME: &str = "index.json";

/// The Mission's decisions directory (Source `decisions_dir`; never created).
pub fn decisions_dir(mission_dir: &Path) -> PathBuf {
    mission_dir.join(DECISIONS_DIRECTORY)
}

/// The Mission's decision index path (Source `index_path`).
pub fn index_path(mission_dir: &Path) -> PathBuf {
    decisions_dir(mission_dir).join(INDEX_FILE_NAME)
}

// ---------------------------------------------------------------------------
// the validated, read-only view
// ---------------------------------------------------------------------------

/// A loaded `decisions/index.json`: the validated index plus its status view.
///
/// The entries keep DOCUMENT order exactly as Source's `load_index` returns them
/// (Source sorts only when it SAVES). This type has no mutating method.
#[derive(Clone, Debug)]
pub struct DecisionIndexStore {
    index: DecisionIndex,
    statuses: BTreeMap<PythonString, DecisionStatus>,
}

impl DecisionIndexStore {
    /// Wrap an already validated index, building the lookup view.
    fn new(index: DecisionIndex) -> Self {
        // Source's ACCEPTANCE and VERIFY consumers build
        // `{entry.decision_id: entry for entry in index.entries}` — a dict
        // comprehension, so a duplicate `decision_id` keeps the LAST entry.
        let mut statuses = BTreeMap::new();
        for entry in index.entries() {
            statuses.insert(entry.decision_id().clone(), entry.status());
        }
        Self { index, statuses }
    }

    /// Source's missing-file index: `mission_id=""`, no entries.
    fn empty() -> Self {
        Self::new(DecisionIndex::empty())
    }

    /// The validated `mission_id` (`""` for the synthesized empty index).
    pub fn mission_id(&self) -> &PythonString {
        self.index.mission_id()
    }

    /// The validated `version` — always the exact integer `1`.
    pub fn version(&self) -> &PythonJsonValue {
        self.index.version()
    }

    /// The validated entries, in DOCUMENT order (duplicates included).
    pub fn entries(&self) -> &[DecisionEntry] {
        self.index.entries()
    }

    /// Whether the index has no entries (Source's `DecisionIndex(entries=())`).
    pub fn is_empty(&self) -> bool {
        self.entries().is_empty()
    }

    /// The status of `decision_id` as Source's acceptance/verify consumers read
    /// it: a dict comprehension over `entries`, so the LAST duplicate wins.
    ///
    /// Source's `decisions/service.py` instead uses
    /// `next((e for e in index.entries if e.decision_id == ...), None)` (FIRST
    /// wins); [`Self::first_status_of`] is that distinct view. Neither rule is
    /// invented here — both are read off the pinned consumers.
    pub fn status_of(&self, decision_id: &PythonString) -> Option<DecisionStatus> {
        self.statuses.get(decision_id).copied()
    }

    /// The status of `decision_id` as Source's `decisions/service.py` reads it:
    /// the FIRST entry with that id wins.
    pub fn first_status_of(&self, decision_id: &PythonString) -> Option<DecisionStatus> {
        self.entries()
            .iter()
            .find(|entry| entry.decision_id() == decision_id)
            .map(DecisionEntry::status)
    }

    /// The whole `decision_id -> status` lookup view (last duplicate wins).
    pub fn statuses(&self) -> &BTreeMap<PythonString, DecisionStatus> {
        &self.statuses
    }

    /// The validated index this store exposes.
    pub fn index(&self) -> &DecisionIndex {
        &self.index
    }
}

// ---------------------------------------------------------------------------
// errors
// ---------------------------------------------------------------------------

/// Why `index.json` could not be decoded — the typed form of Source's
/// `cause` (a `MetaDecodeError`, an `OSError`, or a pydantic `ValidationError`).
#[derive(Debug)]
pub enum DecisionIndexReadCause {
    /// The read itself failed (`OSError` in Source).
    Io(std::io::Error),
    /// The bytes are not strict UTF-8 (`UnicodeDecodeError` in Source).
    NonUtf8(PythonUtf8DecodeError),
    /// The text is not one JSON document (`json.JSONDecodeError` in Source).
    Json(PythonJsonError),
    /// The JSON top level is not an object; the value is Source's
    /// `type(value).__name__` (CPython's `Expected JSON object, got <name>`).
    NotObject {
        /// The Python type name of the decoded top-level value.
        python_type: &'static str,
    },
    /// The document is valid JSON but not a valid `DecisionIndex`.
    Model(DecisionModelError),
}

impl std::fmt::Display for DecisionIndexReadCause {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // Source renders `str(OSError)`, e.g. `[Errno 21] Is a directory:
            // '…'`; Rust's `io::Error` text is the Native spelling of the same
            // failure (disclosed, not faked).
            Self::Io(error) => write!(out, "{error}"),
            // CPython-exact: Kernel owns the `UnicodeDecodeError` text.
            Self::NonUtf8(error) => write!(out, "{error}"),
            // CPython-exact: Kernel owns `str(JSONDecodeError)`.
            Self::Json(error) => write!(out, "{error}"),
            Self::NotObject { python_type } => {
                write!(out, "Expected JSON object, got {python_type}")
            }
            // Native model text; Source's pydantic traceback prose is separate
            // error-wire work (F11) and is never fabricated.
            Self::Model(error) => write!(out, "{error}"),
        }
    }
}

/// Source's `DecisionIndexReadError`: `index.json` EXISTS but cannot be decoded.
///
/// Mirrors `specify_cli.decisions.store.DecisionIndexReadError` — a fail-closed
/// signal that keeps a genuine read failure distinct from the benign missing-file
/// case. The message text is Source's wrapper text with this crate's truthful
/// native cause inside it.
#[derive(Debug)]
pub struct DecisionIndexReadError {
    path: PathBuf,
    cause: DecisionIndexReadCause,
}

impl DecisionIndexReadError {
    fn new(path: &Path, cause: DecisionIndexReadCause) -> Self {
        Self {
            path: path.to_path_buf(),
            cause,
        }
    }

    /// The `index.json` that could not be decoded.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The typed cause.
    pub fn cause(&self) -> &DecisionIndexReadCause {
        &self.cause
    }
}

impl std::fmt::Display for DecisionIndexReadError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            out,
            "Cannot read {}: {} \u{2014} fail-closed (index.json exists but is corrupt or \
             unreadable); run: spec-kitty doctor",
            self.path.display(),
            self.cause
        )
    }
}

impl std::error::Error for DecisionIndexReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.cause {
            DecisionIndexReadCause::Io(error) => Some(error),
            DecisionIndexReadCause::NonUtf8(error) => Some(error),
            DecisionIndexReadCause::Json(error) => Some(error),
            DecisionIndexReadCause::Model(error) => Some(error),
            DecisionIndexReadCause::NotObject { .. } => None,
        }
    }
}

/// The two Source-distinct failure classes of [`load_index`].
///
/// Source raises the `exists()` probe's `OSError` BEFORE it enters the decode
/// boundary, and only the read/decode/schema failures become
/// `DecisionIndexReadError`. Keeping that split in the type stops a probe failure
/// from being reported as a corrupt index (and stops an unreachable directory from
/// being reported as Source's benign empty index).
#[derive(Debug)]
pub enum DecisionIndexLoadError {
    /// The existence probe itself failed (Source re-raises this `OSError`).
    Probe {
        /// The probed path.
        path: PathBuf,
        /// The probe's failure.
        source: std::io::Error,
    },
    /// The file exists but cannot be decoded (Source's `DecisionIndexReadError`).
    Read(DecisionIndexReadError),
}

impl DecisionIndexLoadError {
    /// The read failure, when this is one.
    pub fn read(&self) -> Option<&DecisionIndexReadError> {
        match self {
            Self::Read(error) => Some(error),
            Self::Probe { .. } => None,
        }
    }
}

impl std::fmt::Display for DecisionIndexLoadError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Probe { path, source } => write!(
                out,
                "Cannot inspect {}: {source} (the existence probe failed; Source raises this \
                 OSError before its corrupt-index boundary)",
                path.display()
            ),
            Self::Read(error) => write!(out, "{error}"),
        }
    }
}

impl std::error::Error for DecisionIndexLoadError {}

// ---------------------------------------------------------------------------
// loading
// ---------------------------------------------------------------------------

/// Load and validate `decisions/index.json` (Source `load_index`).
///
/// A missing index — including a broken symlink, which CPython's `Path.exists()`
/// reports as absent — is the validated EMPTY index (`mission_id=""`,
/// `entries=()`). A present one goes through the canonical byte decoder and the
/// SAME typed model the merge driver projects through. Nothing is written,
/// created, locked, repaired or re-serialized.
pub fn load_index(mission_dir: &Path) -> Result<DecisionIndexStore, DecisionIndexLoadError> {
    let path = index_path(mission_dir);
    match probe_exists(&path).map_err(|source| DecisionIndexLoadError::Probe {
        path: path.clone(),
        source,
    })? {
        false => Ok(DecisionIndexStore::empty()),
        true => decode_index(&path)
            .map(DecisionIndexStore::new)
            .map_err(DecisionIndexLoadError::Read),
    }
}

/// Source `Path.exists()`: `stat`, with CPython 3.11's ignored error set treated
/// as ABSENT and every other failure re-raised.
///
/// Follows symlinks (a broken link is `ENOENT` → absent), exactly like
/// `Path.stat()`'s default.
fn probe_exists(path: &Path) -> std::io::Result<bool> {
    sk_kernel::paths::source_path_exists(path)
}

/// Source `_decode_index`: one read, one strict UTF-8 decode, one JSON read, one
/// non-object check, then the model.
fn decode_index(path: &Path) -> Result<DecisionIndex, DecisionIndexReadError> {
    let bytes = read_source_bytes(path)
        .map_err(|error| DecisionIndexReadError::new(path, read_cause(error)))?;
    let text = decode_utf8(&bytes).map_err(|error| {
        DecisionIndexReadError::new(path, DecisionIndexReadCause::NonUtf8(error))
    })?;
    let value = parse(text)
        .map_err(|error| DecisionIndexReadError::new(path, DecisionIndexReadCause::Json(error)))?;
    let PythonJsonValue::ExactObject(document) = value else {
        return Err(DecisionIndexReadError::new(
            path,
            DecisionIndexReadCause::NotObject {
                python_type: python_type_name(&value),
            },
        ));
    };
    validate_index(&document)
        .map_err(|error| DecisionIndexReadError::new(path, DecisionIndexReadCause::Model(error)))
}

/// The canonical reader's typed failure, as Source's `OSError` cause.
fn read_cause(error: KernelError) -> DecisionIndexReadCause {
    match error {
        KernelError::Io { source, .. } => DecisionIndexReadCause::Io(source),
        other => DecisionIndexReadCause::Io(std::io::Error::other(other.to_string())),
    }
}

/// CPython `type(value).__name__` for a decoded JSON value.
fn python_type_name(value: &PythonJsonValue) -> &'static str {
    match value {
        PythonJsonValue::Null => "NoneType",
        PythonJsonValue::Bool(_) => "bool",
        // The reader never produces `Number` (a producer convenience token); a
        // token carrying a fraction or exponent would have been a `float`.
        PythonJsonValue::Number(token) => {
            if token.contains(['.', 'e', 'E']) {
                "float"
            } else {
                "int"
            }
        }
        PythonJsonValue::Integer(_) => "int",
        PythonJsonValue::Float(_) => "float",
        PythonJsonValue::String(_) => "str",
        PythonJsonValue::Array(_) => "list",
        PythonJsonValue::Object(_) | PythonJsonValue::ExactObject(_) => "dict",
    }
}
