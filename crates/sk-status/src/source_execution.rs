//! The ONE typed Source store execution boundary: caught versus escaped.
//!
//! The reference's status store
//! (`specify_cli.status.store._partition_event_stream_from_text`) does **not**
//! flatten every failure into one refusal. It has three genuinely different
//! outcomes, and a caller can observe which one it got:
//!
//! | Reference phase | Reference outcome |
//! |---|---|
//! | `json.loads` per line (`JSONDecodeError`) | caught -> `StoreError("Invalid JSON on line N: …")` |
//! | annotation admission, `except (KeyError, ValueError, TypeError, AttributeError)` | caught -> `StoreError("Invalid event structure on line N: …")` |
//! | lane admission, same catch tuple (the `mission_id` resolver runs inside it) | caught -> `StoreError("Invalid event structure on line N: …")` |
//! | an exception class *outside* that tuple, e.g. `int(Infinity)`'s `OverflowError`, `json.loads`'s digit-ceiling `ValueError`, the metadata resolver's escaped `OSError`/`RecursionError` | **not caught** — it escapes to the caller |
//! | the fold (`reduce`), outside the store entirely | **not caught** — its `TypeError`/`AttributeError` escape |
//!
//! A single "refused" error cannot express that difference, and turning an
//! escape into an invalid-row refusal is a lie: `SourceError`-shaped refusal
//! text is what a caller uses to mean "the log is corrupt", while an escaping
//! `OverflowError` means the store crashed. This module owns the ONE typed
//! representation of the distinction plus the end-to-end execution entry point,
//! so no consumer can accidentally turn an escape into a healthy absence or a
//! refused row.
//!
//! Three outcomes, no more:
//!
//! * [`SourceExecutionError::Store`] — the reference caught it and raised
//!   `StoreError`; [`SourceEventLogError`] already owns that exact text.
//! * [`SourceExecutionError::Escaped`] — the reference did not catch it. The
//!   Python **class**, the **phase** and the message are preserved.
//! * [`SourceExecutionError::Operational`] — a native adapter limit with no
//!   Source counterpart (the J1 parser's operational frame budget, a platform
//!   float conversion, an invalid `PYTHONINTMAXSTRDIGITS` startup value). It is
//!   never reported as a Python class and never as healthy `None`/`[]`.
//!
//! Nothing here re-implements a decoder, path policy or fold: the entry points
//! reuse [`crate::source_event`]'s admission/fold and
//! [`crate::source_metadata`]'s resolver, which in turn reuse the canonical
//! Kernel JSON, integer-text, comparison, path and read owners.

use std::path::Path;

use crate::source_event::{
    SourceEventLogError, SourceEventStream, SourceFoldError, SourceSnapshot,
    read_source_event_stream_from_text, reduce_source_diary,
};

/// The Python exception class a Source phase raised.
///
/// Only the classes the pinned reference can actually raise on this path are
/// modelled. `OSError` is named at the family level: the reference raises the
/// OS-specific subclass (for example `PermissionError`) at that phase, and this
/// boundary does not guess one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourcePythonClass {
    /// `KeyError` — a missing mapping key (`obj["wp_id"]`, `data["delta"]`).
    KeyError,
    /// `ValueError` — an invalid lane, a rejected actor field, `int("abc")`, the
    /// integer digit ceiling from `json.loads`.
    ValueError,
    /// `TypeError` — an unhashable dict key, `obj[key]` on a sequence,
    /// `int([])`, `list(1)`.
    TypeError,
    /// `AttributeError` — `.get`/`.items`/`.strip` on a non-mapping/scalar.
    AttributeError,
    /// `OverflowError` — `int(infinity)`; **outside** the store's catch tuple.
    OverflowError,
    /// `RecursionError` — `json.loads` on a deeply nested document; outside the
    /// store's and `load_meta_fail_closed`'s catch tuples.
    RecursionError,
    /// The `OSError` family — `Path.exists()` on an unreadable entry,
    /// `Path.resolve()` on a symlink loop, a read failure that is not caught.
    OSError,
    /// `RuntimeError` — `pathlib`'s symlink-loop wrapper from `Path.resolve`.
    RuntimeError,
}

impl SourcePythonClass {
    /// The reference's class name.
    pub fn name(self) -> &'static str {
        match self {
            Self::KeyError => "KeyError",
            Self::ValueError => "ValueError",
            Self::TypeError => "TypeError",
            Self::AttributeError => "AttributeError",
            Self::OverflowError => "OverflowError",
            Self::RecursionError => "RecursionError",
            Self::OSError => "OSError",
            Self::RuntimeError => "RuntimeError",
        }
    }

    /// True when the reference's store-level
    /// `except (KeyError, ValueError, TypeError, AttributeError)` catches it.
    ///
    /// `OverflowError`, `RecursionError`, `OSError` and `RuntimeError` are
    /// outside that tuple, so they escape. The fold has no store catch at all:
    /// its `TypeError`/`AttributeError` escape even though the class is in the
    /// tuple, which is why [`SourceExecutionError::escaped`] exists beside
    /// [`SourceExecutionError::store_or_escape`].
    pub fn store_caught(self) -> bool {
        matches!(
            self,
            Self::KeyError | Self::ValueError | Self::TypeError | Self::AttributeError
        )
    }
}

/// Where in the reference's execution the failure arose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceExecutionPhase {
    /// `json.loads(stripped)` for one log line.
    LineDecode,
    /// `isinstance(obj, dict)` for one log line.
    RowShape,
    /// The `kind` discriminator partition.
    KindPartition,
    /// `mission_slug in self._cache` (a dict lookup, so the hash runs first).
    MissionIdCache,
    /// `assert_safe_path_segment(mission_slug)`.
    MissionIdGrammar,
    /// `ensure_within_any(meta_path, roots=[root])` (`resolve(strict=False)`).
    MissionIdContainment,
    /// `meta_path.exists()`.
    MissionIdExists,
    /// `load_meta_fail_closed` -> `Path.read_text(encoding="utf-8")`.
    MissionIdRead,
    /// `load_meta_fail_closed` -> `json.loads` + the object-only check.
    MissionIdDecode,
    /// `data.get("mission_id") or None`.
    MissionIdField,
    /// `InnerStateChanged.from_dict` for an annotation row.
    AnnotationAdmission,
    /// `StatusEvent.from_dict` for a lane row.
    TransitionAdmission,
    /// `reduce(transitions, annotations)` — outside the store's catch entirely.
    Fold,
}

impl SourceExecutionPhase {
    /// The phase name used in diagnostics and consumer errors.
    pub fn name(self) -> &'static str {
        match self {
            Self::LineDecode => "line_decode",
            Self::RowShape => "row_shape",
            Self::KindPartition => "kind_partition",
            Self::MissionIdCache => "mission_id_cache",
            Self::MissionIdGrammar => "mission_id_grammar",
            Self::MissionIdContainment => "mission_id_containment",
            Self::MissionIdExists => "mission_id_exists",
            Self::MissionIdRead => "mission_id_read",
            Self::MissionIdDecode => "mission_id_decode",
            Self::MissionIdField => "mission_id_field",
            Self::AnnotationAdmission => "annotation_admission",
            Self::TransitionAdmission => "transition_admission",
            Self::Fold => "fold",
        }
    }
}

/// A Python exception raised by a *leaf* decoder (class + message, no phase yet).
///
/// The leaf does not know the log line or the enclosing phase, so it reports
/// only what the reference's `raise` carries; the store boundary attaches the
/// phase and line with [`SourceRaised::at`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourcePythonFailure {
    class: SourcePythonClass,
    message: String,
}

impl SourcePythonFailure {
    /// One raised Python exception.
    pub fn new(class: SourcePythonClass, message: impl Into<String>) -> Self {
        Self {
            class,
            message: message.into(),
        }
    }

    /// A `KeyError` for a missing mapping key.
    pub fn key_error(message: impl Into<String>) -> Self {
        Self::new(SourcePythonClass::KeyError, message)
    }

    /// A `ValueError`.
    pub fn value_error(message: impl Into<String>) -> Self {
        Self::new(SourcePythonClass::ValueError, message)
    }

    /// A `TypeError`.
    pub fn type_error(message: impl Into<String>) -> Self {
        Self::new(SourcePythonClass::TypeError, message)
    }

    /// An `AttributeError`.
    pub fn attribute_error(message: impl Into<String>) -> Self {
        Self::new(SourcePythonClass::AttributeError, message)
    }

    /// An `OverflowError`.
    pub fn overflow_error(message: impl Into<String>) -> Self {
        Self::new(SourcePythonClass::OverflowError, message)
    }

    /// The Python class.
    pub fn class(&self) -> SourcePythonClass {
        self.class
    }

    /// The reference's own message text (`str(exc)`).
    pub fn message(&self) -> &str {
        &self.message
    }

    /// This failure located at a phase (and a 1-based log line, when known).
    pub fn at(self, phase: SourceExecutionPhase, line: Option<usize>) -> SourceRaised {
        SourceRaised {
            class: self.class,
            phase,
            line,
            message: self.message,
        }
    }
}

impl std::fmt::Display for SourcePythonFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.class.name(), self.message)
    }
}

/// A Python exception the reference let escape, with its phase and line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRaised {
    class: SourcePythonClass,
    phase: SourceExecutionPhase,
    line: Option<usize>,
    message: String,
}

impl SourceRaised {
    /// One escaped exception at a phase.
    pub fn new(
        class: SourcePythonClass,
        phase: SourceExecutionPhase,
        message: impl Into<String>,
    ) -> Self {
        Self {
            class,
            phase,
            line: None,
            message: message.into(),
        }
    }

    /// One escaped exception from a leaf failure at a phase.
    pub fn at(
        phase: SourceExecutionPhase,
        line: Option<usize>,
        failure: SourcePythonFailure,
    ) -> Self {
        failure.at(phase, line)
    }

    /// The same failure located at a 1-based log line.
    pub fn with_line(mut self, line: usize) -> Self {
        self.line = Some(line);
        self
    }

    /// The Python class.
    pub fn class(&self) -> SourcePythonClass {
        self.class
    }

    /// The execution phase.
    pub fn phase(&self) -> SourceExecutionPhase {
        self.phase
    }

    /// The 1-based log line, when the phase is per-row.
    pub fn line(&self) -> Option<usize> {
        self.line
    }

    /// The reference's own message text (`str(exc)`).
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for SourceRaised {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.class.name(), self.message)
    }
}

/// A native adapter limit with no Source counterpart.
///
/// The reference cannot produce this failure: CPython would either accept the
/// input or raise a Python class. The label is deliberately *not* a Python
/// exception name — for example the J1 reader's operational frame budget sits
/// below CPython's own acceptance ceiling, so naming it `RecursionError` would
/// fabricate a Source phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceOperational {
    phase: SourceExecutionPhase,
    detail: String,
}

impl SourceOperational {
    /// One native-only failure at a phase.
    pub fn new(phase: SourceExecutionPhase, detail: impl Into<String>) -> Self {
        Self {
            phase,
            detail: detail.into(),
        }
    }

    /// The execution phase.
    pub fn phase(&self) -> SourceExecutionPhase {
        self.phase
    }

    /// What the native adapter refused.
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl std::fmt::Display for SourceOperational {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "native operational failure during {} (no Source counterpart): {}",
            self.phase.name(),
            self.detail
        )
    }
}

/// The outcome of one Source store execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceExecutionError {
    /// The reference caught the failure and raised `StoreError`.
    Store(SourceEventLogError),
    /// The reference did not catch it: the exception escapes to the caller.
    Escaped(SourceRaised),
    /// A native adapter limit the reference cannot reach.
    Operational(SourceOperational),
}

impl SourceExecutionError {
    /// Classify a failure that arose inside the reference's *store* `try`.
    ///
    /// This is the reference's own policy, applied once:
    /// `except (KeyError, ValueError, TypeError, AttributeError)` -> `StoreError`
    /// with the row's `Invalid event structure on line N: <exc>` text; anything
    /// else escapes unchanged. Callers inside the store block (annotation
    /// admission, the `mission_id` resolver) must use this, not
    /// [`Self::escaped`].
    pub fn store_or_escape(raised: SourceRaised) -> Self {
        match (raised.class.store_caught(), raised.line) {
            (true, Some(line)) => Self::Store(SourceEventLogError::InvalidStructure {
                line,
                detail: raised.message,
                phase: raised.phase,
            }),
            _ => Self::Escaped(raised),
        }
    }

    /// Classify a failure that arose *outside* any store catch (the fold).
    ///
    /// The reference's fold runs after `read_events*` returns, so even a
    /// `TypeError`/`AttributeError` escapes its caller rather than becoming a
    /// `StoreError`.
    pub fn escaped(raised: SourceRaised) -> Self {
        Self::Escaped(raised)
    }

    /// The caught `StoreError`, when the reference caught it.
    pub fn store_error(&self) -> Option<&SourceEventLogError> {
        match self {
            Self::Store(error) => Some(error),
            _ => None,
        }
    }

    /// The escaped Python exception, when the reference let it out.
    pub fn raised(&self) -> Option<&SourceRaised> {
        match self {
            Self::Escaped(raised) => Some(raised),
            _ => None,
        }
    }

    /// The native-only limit, when that is what happened.
    pub fn operational(&self) -> Option<&SourceOperational> {
        match self {
            Self::Operational(operational) => Some(operational),
            _ => None,
        }
    }

    /// True only for the reference's own caught `StoreError` phase.
    pub fn is_caught_store_error(&self) -> bool {
        matches!(self, Self::Store(_))
    }

    /// The execution phase this outcome arose in.
    pub fn phase(&self) -> SourceExecutionPhase {
        match self {
            Self::Store(error) => error.phase(),
            Self::Escaped(raised) => raised.phase,
            Self::Operational(operational) => operational.phase,
        }
    }
}

impl std::fmt::Display for SourceExecutionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(error) => error.fmt(formatter),
            Self::Escaped(raised) => raised.fmt(formatter),
            Self::Operational(operational) => operational.fmt(formatter),
        }
    }
}

impl std::error::Error for SourceExecutionError {}

impl From<SourceEventLogError> for SourceExecutionError {
    fn from(error: SourceEventLogError) -> Self {
        Self::Store(error)
    }
}

impl From<SourceFoldError> for SourceExecutionError {
    /// A fold refusal is the reference's own `TypeError`/`AttributeError`
    /// escaping the caller — never a `StoreError`.
    fn from(error: SourceFoldError) -> Self {
        Self::Escaped(error.into_raised())
    }
}

/// Whether the annotation partition is folded.
///
/// `read_events_from_text` (the committed consumer's view) admits annotations
/// and then folds none of them; `reduce(transitions, annotations)` folds both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceAnnotationPass {
    /// Fold annotations in the dedicated post-transition pass.
    Fold,
    /// Admit annotations (so a malformed one still refuses) but do not fold them.
    Ignore,
}

/// One completed Source execution: the admitted stream and its reduced snapshot.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceExecution {
    /// The admitted stream (transitions in log order, annotations in log order).
    pub stream: SourceEventStream,
    /// The reduced snapshot.
    pub snapshot: SourceSnapshot,
}

/// The ONE end-to-end Source execution boundary: raw text -> admitted stream ->
/// fold, with the reference's caught/escaped/operational outcome preserved.
///
/// `pass` mirrors the caller the reference has: its store read is
/// transitions-only for the committed consumer and annotation-aware for
/// `reduce`.
pub fn execute_source_diary(
    feature_dir: &Path,
    content: &str,
    pass: SourceAnnotationPass,
) -> Result<SourceExecution, SourceExecutionError> {
    let stream = read_source_event_stream_from_text(feature_dir, content)?;
    let annotations: &[crate::source_annotation::SourceAnnotation] = match pass {
        SourceAnnotationPass::Fold => &stream.annotations,
        SourceAnnotationPass::Ignore => &[],
    };
    let snapshot = reduce_source_diary(&stream.transitions, annotations)?;
    Ok(SourceExecution { stream, snapshot })
}

impl SourceEventLogError {
    /// The execution phase a caught store refusal arose in.
    ///
    /// `InvalidStructure` carries the phase it was raised from (annotation versus
    /// lane admission versus the legacy `mission_id` resolver), because the same
    /// `StoreError` text serves all three in the reference.
    pub fn phase(&self) -> SourceExecutionPhase {
        match self {
            Self::InvalidJson { .. } => SourceExecutionPhase::LineDecode,
            Self::NonObjectRow { .. } => SourceExecutionPhase::RowShape,
            Self::UnknownKind { .. } => SourceExecutionPhase::KindPartition,
            Self::InvalidStructure { phase, .. } => *phase,
        }
    }
}

impl SourceFoldError {
    /// The Python class the reference's fold raised.
    pub fn class(&self) -> SourcePythonClass {
        match self {
            Self::TypeError { .. } => SourcePythonClass::TypeError,
            Self::AttributeError { .. } => SourcePythonClass::AttributeError,
        }
    }

    /// The reference's fold refusal as an escaped exception.
    pub fn into_raised(self) -> SourceRaised {
        let class = self.class();
        let message = match self {
            Self::TypeError { message } | Self::AttributeError { message } => message,
        };
        SourceRaised::new(class, SourceExecutionPhase::Fold, message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reference_catch_tuple_is_the_only_caught_set() {
        for (class, caught) in [
            (SourcePythonClass::KeyError, true),
            (SourcePythonClass::ValueError, true),
            (SourcePythonClass::TypeError, true),
            (SourcePythonClass::AttributeError, true),
            (SourcePythonClass::OverflowError, false),
            (SourcePythonClass::RecursionError, false),
            (SourcePythonClass::OSError, false),
            (SourcePythonClass::RuntimeError, false),
        ] {
            assert_eq!(class.store_caught(), caught, "{class:?}");
        }
    }

    #[test]
    fn a_caught_class_becomes_the_reference_store_error_text() {
        let error = SourceExecutionError::store_or_escape(
            SourcePythonFailure::value_error("cannot convert float NaN to integer")
                .at(SourceExecutionPhase::AnnotationAdmission, Some(2)),
        );
        assert_eq!(
            error.to_string(),
            "Invalid event structure on line 2: cannot convert float NaN to integer"
        );
        assert!(error.is_caught_store_error());
        assert_eq!(error.phase(), SourceExecutionPhase::AnnotationAdmission);
    }

    #[test]
    fn an_overflow_escapes_instead_of_becoming_a_store_error() {
        let error = SourceExecutionError::store_or_escape(
            SourcePythonFailure::overflow_error("cannot convert float infinity to integer")
                .at(SourceExecutionPhase::AnnotationAdmission, Some(1)),
        );
        assert_eq!(
            error.to_string(),
            "OverflowError: cannot convert float infinity to integer"
        );
        assert!(!error.is_caught_store_error());
        assert_eq!(error.phase(), SourceExecutionPhase::AnnotationAdmission);
        assert_eq!(
            error.raised().and_then(SourceRaised::line),
            Some(1),
            "the row is preserved"
        );
    }

    #[test]
    fn the_fold_class_escapes_even_though_the_store_catch_tuple_names_it() {
        let error = SourceExecutionError::from(SourceFoldError::TypeError {
            message: "unhashable type: 'list'".to_owned(),
        });
        assert!(!error.is_caught_store_error());
        assert_eq!(error.phase(), SourceExecutionPhase::Fold);
        assert_eq!(error.to_string(), "TypeError: unhashable type: 'list'");
    }
}
