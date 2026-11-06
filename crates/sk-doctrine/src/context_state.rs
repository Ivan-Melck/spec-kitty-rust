//! The Source first-load context-state store.
//!
//! Ports `charter.activation.context_state` (Source
//! `9aed3a8012cb5eb22c0128b14d525039f95fea7a`,
//! `src/charter/activation/context_state.py`) onto the ONE existing Kernel
//! authority for each phase — there is no second state model, JSON
//! parser/renderer, string type, clock or writer in this module:
//!
//! * **read** — [`sk_kernel::paths::source_path_exists`] for the Source
//!   `Path.exists()` probe, then [`sk_kernel::python_json::parse`] for the Source
//!   `json.loads` grammar (Python dict order/duplicates, exact integers,
//!   non-finite floats, escaped lone surrogates, the integer digit ceiling and
//!   the frame budget).
//! * **write** — [`sk_kernel::python_json::PythonJsonValue::render_python_with`]
//!   with `indent=2, sort_keys=True` and the reference defaults
//!   (`ensure_ascii=True`, `allow_nan=True`), then
//!   [`sk_kernel::source_writer::write_source`] selecting the existing Atomic
//!   family with mkdir=True (not the stable/no-op inspection writer).
//! * **clock** — [`sk_kernel::clock::Clock`], injected for focused controls and
//!   defaulting to the canonical [`sk_kernel::clock::SystemClock`].
//!
//! ## The exact Source read/catch boundaries
//!
//! The reference is:
//!
//! ```python
//! if not path.exists():                 # BEFORE the try
//!     return DEFAULT
//! try:
//!     data = json.loads(path.read_text(encoding="utf-8"))
//! except (json.JSONDecodeError, UnicodeDecodeError, OSError):
//!     return DEFAULT
//! if not isinstance(data, dict):
//!     return DEFAULT
//! if not isinstance(data.get("actions"), dict):
//!     data["actions"] = {}
//! return data
//! ```
//!
//! [`load_context_state`] reproduces that order and that *specific* catch set:
//!
//! * the existence probe runs first, and a stat failure the interpreter does not
//!   ignore (for example `EACCES`, `ENAMETOOLONG`) is **escaped**, never treated
//!   as absence — `Path.exists()` only absorbs the interpreter's absent cases;
//! * every read `OSError` and every UTF-8 decode failure is **caught** and falls
//!   back to the model default, because `read_text` runs inside the `try`;
//! * a `JSONDecodeError` (any grammar/token/BOM refusal) is **caught**;
//! * the integer digit ceiling is a bare `ValueError`, **not** a
//!   `JSONDecodeError`, so it is **escaped** ([`ContextStateError::IntegerDigitLimit`]);
//! * `RecursionError` is **escaped**; the Kernel reader's operational frame
//!   budget is reported as [`ContextStateError::RecursionBudget`], labelled as a
//!   Native operational boundary rather than a Source format rule;
//! * a valid non-object document resets to the default, and an object keeps every
//!   authored key/value exactly, gaining a `{}` `actions` member only when that
//!   member is absent or is not an object.
//!
//! ## The prepared snapshot is what the mark writes
//!
//! Source `_prepare_context_state` returns a bundle holding the dict it just
//! decoded, and `_mark_action_loaded` mutates *that* dict and then writes it. No
//! second disk read happens at mark time, and there is no compare-and-swap or
//! foreign merge: a concurrent change to the file between prepare and mark is
//! simply overwritten, exactly like the reference. [`ContextStateBundle`] is that
//! ticket — callers must keep it and mark through it rather than re-reading.
//!
//! ## Depth
//!
//! Source `depth` is a Python `int | None` used as a DRG `suggests`-hop cap. This
//! API takes `Option<usize>`: that is the Native adapter of the finite CLI
//! interface the caller already has, and it never caps, clamps or discards a
//! value it was given. The file store itself holds no depth and loses no data.

use std::path::{Path, PathBuf};

use sk_kernel::PythonString;
use sk_kernel::clock::{Clock, SystemClock};
use sk_kernel::python_json::{
    PythonJsonDict, PythonJsonErrorKind, PythonJsonRenderOptions, PythonJsonValue, parse,
};

/// The Source project directory name (`.kittify`).
pub const KITTIFY_DIRNAME: &str = ".kittify";

/// The state file name inside `<repo>/.kittify/charter/`.
pub const CONTEXT_STATE_FILE: &str = "context-state.json";

/// The `schema_version` the reference writes into a default document.
pub const CONTEXT_STATE_SCHEMA_VERSION: &str = "1.0.0";

/// Bootstrap depth: the first load delivers the summary plus references.
pub const MIN_EFFECTIVE_DEPTH: usize = 2;

/// A refused context-state phase.
///
/// Every variant here is a failure the reference does **not** absorb: the
/// caught cases (absence, unreadable bytes, decode errors, JSON grammar errors,
/// non-object documents) never produce an error at all, they produce
/// [`ContextStateValue::default_state`]. `Native` variants are operational limits
/// of this implementation and are labelled as such.
#[derive(Debug, thiserror::Error)]
pub enum ContextStateError {
    /// The Source existence probe failed with an error the interpreter does not
    /// ignore, so `_load_state` raises before any read.
    #[error("could not probe the context-state file: {path}: {source}")]
    Exists {
        /// The probed path.
        path: PathBuf,
        /// The stat failure (`EACCES` and friends, never a Source absence).
        #[source]
        source: std::io::Error,
    },

    /// `json.loads` raised the interpreter's integer digit ceiling: a bare
    /// `ValueError`, which `_load_state` does not catch.
    #[error("context-state {path} exceeds the integer digit ceiling: {message}")]
    IntegerDigitLimit {
        /// The state file.
        path: PathBuf,
        /// The reference `ValueError` text.
        message: String,
    },

    /// The Native operational frame budget was exhausted. Source may accept
    /// this input or raise at a different caller-dependent limit; this is not a
    /// fabricated Source exception or a JSON-format claim.
    #[error("context-state {path} exceeds the native operational JSON frame budget: {message}")]
    RecursionBudget {
        /// The state file.
        path: PathBuf,
        /// The Kernel reader's explanation.
        message: String,
    },

    /// The process' `PYTHONINTMAXSTRDIGITS` startup value cannot configure an
    /// interpreter that would have started. An explicit Native failure, never a
    /// healthy reset.
    #[error("cannot interpret the integer digit ceiling for context-state {path}: {message}")]
    IntegerDigitLimitSetting {
        /// The state file.
        path: PathBuf,
        /// The Kernel reader's explanation.
        message: String,
    },

    /// A grammar-valid float token the platform parser refused. An explicit
    /// Native failure, never a healthy reset.
    #[error("context-state {path} carries a float the platform parser refused: {message}")]
    NumberConversion {
        /// The state file.
        path: PathBuf,
        /// The Kernel reader's explanation.
        message: String,
    },

    /// The write or post-publication observer failed. Source lets its actual
    /// exception escape; the ONE writer's typed cause/publication fact survives.
    #[error("could not write the context-state file {path}: {source}")]
    Write {
        /// The state file.
        path: PathBuf,
        /// Existing Source writer failure, including observer class and whether
        /// publication preceded it; never collapsed into a message-only error.
        #[source]
        source: Box<sk_kernel::source_writer::SourceWriteError>,
    },
}

impl ContextStateError {
    /// Whether the reference reaches this failure as an escape (`true`) rather
    /// than absorbing it into the model default.
    ///
    /// Native operational boundaries are distinct from actual Source exceptions;
    /// neither kind is absorbed into a default state.
    #[must_use]
    pub fn is_escaped(&self) -> bool {
        !self.is_native_boundary()
    }

    /// Whether this failure is a Native operational boundary the reference never
    /// reaches (a startup/parser limit rather than a Source phase).
    #[must_use]
    pub fn is_native_boundary(&self) -> bool {
        matches!(
            self,
            Self::RecursionBudget { .. }
                | Self::IntegerDigitLimitSetting { .. }
                | Self::NumberConversion { .. }
        )
    }
}

/// The decoded state document: a Python `dict` exactly as `json.loads` built it.
///
/// The value is always an object with an object `actions` member — the one shape
/// `_load_state` guarantees — and every other authored member is preserved
/// verbatim, including unknown keys, nesting, exact integers, non-finite floats
/// and escaped lone surrogates.
#[derive(Clone, Debug, PartialEq)]
pub struct ContextStateValue {
    object: PythonJsonDict,
}

impl ContextStateValue {
    /// The model default `{"schema_version": "1.0.0", "actions": {}}`.
    #[must_use]
    pub fn default_state() -> Self {
        let mut object = PythonJsonDict::new();
        object.insert(
            PythonString::from_scalars("schema_version"),
            PythonJsonValue::String(PythonString::from_scalars(CONTEXT_STATE_SCHEMA_VERSION)),
        );
        object.insert(
            PythonString::from_scalars("actions"),
            PythonJsonValue::ExactObject(PythonJsonDict::new()),
        );
        Self { object }
    }

    /// Take one decoded document through the reference's post-read normalisation.
    ///
    /// A non-object document is the model default (the caller never reaches this
    /// with one: [`load_context_state`] applies that rule at the same point the
    /// reference does). An object keeps every member and gains a `{}` `actions`
    /// when that member is absent or is not an object — `dict` assignment
    /// semantics, so an existing non-object `actions` keeps its key position.
    #[must_use]
    pub fn from_decoded(value: &PythonJsonValue) -> Self {
        let members = match value {
            PythonJsonValue::ExactObject(object) => object.clone(),
            PythonJsonValue::Object(entries) => {
                let mut object = PythonJsonDict::new();
                for (key, item) in entries {
                    object.insert(PythonString::from_scalars(key), item.clone());
                }
                object
            }
            _ => return Self::default_state(),
        };
        let mut state = Self { object: members };
        if !state.actions_is_object() {
            state.set_empty_actions();
        }
        state
    }

    /// The `actions` member as an object, when it is one.
    #[must_use]
    pub fn actions(&self) -> Option<&PythonJsonDict> {
        match self.object.get("actions") {
            Some(PythonJsonValue::ExactObject(object)) => Some(object),
            _ => None,
        }
    }

    /// Source `action in state["actions"]` (Python `str` equality, exact code
    /// points).
    #[must_use]
    pub fn contains_action(&self, action: &str) -> bool {
        self.actions()
            .is_some_and(|actions| actions.get(action).is_some())
    }

    /// The stored timestamp for `action`, when one is present.
    #[must_use]
    pub fn action_stamp(&self, action: &str) -> Option<&PythonString> {
        match self.actions()?.get(action) {
            Some(PythonJsonValue::String(stamp)) => Some(stamp),
            _ => None,
        }
    }

    /// The decoded document itself.
    #[must_use]
    pub fn value(&self) -> PythonJsonValue {
        PythonJsonValue::ExactObject(self.object.clone())
    }

    /// The exact bytes the reference writes for this document.
    ///
    /// `json.dumps(state, indent=2, sort_keys=True)` with the library defaults
    /// (`ensure_ascii=True`, `allow_nan=True`) and no trailing newline. The render
    /// is ASCII by construction — every non-ASCII code point, including a lone
    /// surrogate, is emitted as a `\uXXXX` escape — which is the invariant the
    /// existing doctrine render path also relies on.
    #[must_use]
    pub fn render(&self) -> String {
        self.value()
            .render_python_with(
                PythonJsonRenderOptions::new(Some(2))
                    .sort_keys(true)
                    .ensure_ascii(true),
            )
            .to_scalar_string()
            .expect("ensure_ascii=True renders only scalar ASCII")
    }

    /// Source `_mark_action_loaded`'s in-memory half: stamp `action`.
    pub fn mark_action(&mut self, action: &str, stamp: &str) {
        if !self.actions_is_object() {
            self.set_empty_actions();
        }
        let value = PythonJsonValue::String(PythonString::from_scalars(stamp));
        self.assign_action(action, value);
    }

    fn assign_action(&mut self, action: &str, value: PythonJsonValue) {
        // `dict` assignment on the existing object keeps its key position; the
        // `actions` member itself is replaced in place, never re-appended.
        let mut actions = self.actions().cloned().unwrap_or_default();
        actions.insert(PythonString::from_scalars(action), value);
        self.object.insert(
            PythonString::from_scalars("actions"),
            PythonJsonValue::ExactObject(actions),
        );
    }

    fn actions_is_object(&self) -> bool {
        self.actions().is_some()
    }

    fn set_empty_actions(&mut self) {
        self.object.insert(
            PythonString::from_scalars("actions"),
            PythonJsonValue::ExactObject(PythonJsonDict::new()),
        );
    }
}

/// The first-load state bundle Source `_prepare_context_state` returns.
///
/// This is the ticket a caller keeps: [`ContextStateBundle::mark_action_loaded`]
/// stamps the action into *this* snapshot and writes *this* snapshot, exactly
/// like the reference, so no second disk read, compare-and-swap or foreign merge
/// happens at mark time.
#[derive(Clone, Debug, PartialEq)]
pub struct ContextStateBundle {
    state_path: PathBuf,
    state: ContextStateValue,
    first_load: bool,
    effective_depth: usize,
}

impl ContextStateBundle {
    /// The state file this bundle was prepared from.
    #[must_use]
    pub fn state_path(&self) -> &Path {
        &self.state_path
    }

    /// The prepared snapshot.
    #[must_use]
    pub fn state(&self) -> &ContextStateValue {
        &self.state
    }

    /// Whether the action had never been loaded when the snapshot was read.
    #[must_use]
    pub fn first_load(&self) -> bool {
        self.first_load
    }

    /// The effective context depth the reference resolved.
    #[must_use]
    pub fn effective_depth(&self) -> usize {
        self.effective_depth
    }

    /// Source `_mark_action_loaded`: stamp `action` into this snapshot and write
    /// it back through the canonical atomic writer.
    pub fn mark_action_loaded(&mut self, action: &str) -> Result<(), ContextStateError> {
        self.mark_action_loaded_with(action, &SystemClock)
    }

    /// The same, with an injected clock (focused controls only).
    pub fn mark_action_loaded_with(
        &mut self,
        action: &str,
        clock: &dyn Clock,
    ) -> Result<(), ContextStateError> {
        let stamp = clock.now_utc_stamp();
        self.state.mark_action(action, &stamp);
        write_context_state(&self.state_path, &self.state)
    }
}

/// Source `_load_state`: decode the bookkeeping document, or the model default.
///
/// See the module documentation for the exact read/catch order and which
/// failures are absorbed versus escaped.
pub fn load_context_state(path: &Path) -> Result<ContextStateValue, ContextStateError> {
    let exists =
        sk_kernel::paths::source_path_exists(path).map_err(|source| ContextStateError::Exists {
            path: path.to_path_buf(),
            source,
        })?;
    if !exists {
        return Ok(ContextStateValue::default_state());
    }

    // `read_text(encoding="utf-8")` runs inside the reference's `try`, so both an
    // OS read failure and an invalid-UTF-8 body are caught, not escaped.
    let Ok(bytes) = std::fs::read(path) else {
        return Ok(ContextStateValue::default_state());
    };
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return Ok(ContextStateValue::default_state());
    };

    let text = sk_kernel::python_text::universal_newlines(text);
    let decoded = match parse(&text) {
        Ok(value) => value,
        Err(error) => match error.kind() {
            // `json.JSONDecodeError` — the reference catches every one of these.
            PythonJsonErrorKind::ExpectingValue
            | PythonJsonErrorKind::ExpectingDelimiter
            | PythonJsonErrorKind::ExpectingColon
            | PythonJsonErrorKind::ExpectingPropertyName
            | PythonJsonErrorKind::ExtraData
            | PythonJsonErrorKind::UnterminatedString
            | PythonJsonErrorKind::InvalidEscape
            | PythonJsonErrorKind::InvalidUnicodeEscape
            | PythonJsonErrorKind::InvalidControlCharacter
            | PythonJsonErrorKind::UnexpectedBom => return Ok(ContextStateValue::default_state()),
            // A bare `ValueError` from the integer conversion ceiling: escaped.
            PythonJsonErrorKind::IntegerDigitLimit => {
                return Err(ContextStateError::IntegerDigitLimit {
                    path: path.to_path_buf(),
                    message: error.to_string(),
                });
            }
            // `RecursionError`: escaped (reported as the Native frame boundary).
            PythonJsonErrorKind::RecursionBudget => {
                return Err(ContextStateError::RecursionBudget {
                    path: path.to_path_buf(),
                    message: error.to_string(),
                });
            }
            // Startup/platform limits the reference never reaches.
            PythonJsonErrorKind::IntegerDigitLimitSetting => {
                return Err(ContextStateError::IntegerDigitLimitSetting {
                    path: path.to_path_buf(),
                    message: error.to_string(),
                });
            }
            PythonJsonErrorKind::NumberConversion => {
                return Err(ContextStateError::NumberConversion {
                    path: path.to_path_buf(),
                    message: error.to_string(),
                });
            }
        },
    };

    Ok(ContextStateValue::from_decoded(&decoded))
}

/// Source `_write_state`: `atomic_write(path, json.dumps(...), mkdir=True)`.
pub fn write_context_state(
    path: &Path,
    state: &ContextStateValue,
) -> Result<(), ContextStateError> {
    let bytes = state.render();
    sk_kernel::source_writer::write_source(
        sk_kernel::source_writer::WriterOperation::atomic_creating_parents(),
        path,
        sk_kernel::source_writer::SourcePayload::Text(&bytes),
        None,
    )
    .map(|_| ())
    .map_err(|source| ContextStateError::Write {
        path: path.to_path_buf(),
        source: Box::new(source),
    })
}

/// Source `_prepare_context_state`: resolve the first-load state and the
/// effective context depth for `repo_root` and one already-normalized action.
///
/// `action` must be the Source-normalized key
/// ([`crate::normalize_context_action`]); the reference receives the same
/// normalized spelling here and uses it for both the membership test and the mark.
///
/// `depth` is the Native adapter of the reference's `int | None`: `Some` is used
/// verbatim (no cap, no clamp), `None` resolves to
/// [`MIN_EFFECTIVE_DEPTH`] on a first load and `1` afterwards.
pub fn prepare_context_state(
    repo_root: &Path,
    action: &str,
    depth: Option<usize>,
) -> Result<ContextStateBundle, ContextStateError> {
    let state_path = repo_root
        .join(KITTIFY_DIRNAME)
        .join("charter")
        .join(CONTEXT_STATE_FILE);
    let state = load_context_state(&state_path)?;
    // `_load_state` guarantees an object `actions` member, so the reference's
    // `isinstance(actions_val, dict)` fallback is already resolved here.
    let first_load = !state.contains_action(action);
    let effective_depth = match depth {
        Some(depth) => depth,
        None if first_load => MIN_EFFECTIVE_DEPTH,
        None => 1,
    };
    Ok(ContextStateBundle {
        state_path,
        state,
        first_load,
        effective_depth,
    })
}
