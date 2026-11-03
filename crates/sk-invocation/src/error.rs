//! Typed failures for the Op-record domain.
//!
//! The reference has a Python exception hierarchy: `InvocationError` at the
//! base, with `InvocationWriteError`, `AlreadyClosedError` and
//! `LegacyRecordError` as subclasses, and plain `ValueError` for the id and
//! path boundaries. Rust has no subclassing, so the same facts are expressed
//! as variants of [`InvocationError`] plus the standalone
//! [`LegacyRecordError`] / [`RecordParseError`] pair a reader matches on.
//!
//! Operator-facing text is preserved where the reference treats it as a
//! contract (see the individual variants).

use thiserror::Error;

use crate::python_json::python_repr;

/// A value rejected by the canonical 26-character Crockford id boundary.
///
/// Mirrors `record.validate_invocation_id`'s `ValueError`:
/// `invocation_id must be a 26-char ULID, got <repr>`.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("invocation_id must be a 26-char ULID, got {}", python_repr(value))]
pub struct InvocationIdError {
    /// The rejected value, verbatim.
    pub value: String,
}

/// An entry command that `modes._ENTRY_COMMAND_MODE` does not register.
///
/// The reference raises `KeyError`; a mistyped entry command is a CLI-layer
/// programming error, not an operator error.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("entry command {entry:?} is not a registered ModeOfWork entry command")]
pub struct UnknownEntryCommand {
    /// The unregistered entry-command token.
    pub entry: String,
}

/// A Pydantic model-level validation failure, reduced to one field.
///
/// Pydantic reports every failing field in a multi-line `ValidationError`.
/// This port reports the first failing field in declaration order with the
/// same constraint vocabulary (`Field required`, `Input should be a valid
/// string`, `String should have at least 1 character`, `Input should be '…'`).
#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("{field}: {reason}")]
pub struct RecordValidationError {
    /// The declared field name.
    pub field: &'static str,
    /// The constraint that failed.
    pub reason: String,
}

impl RecordValidationError {
    pub(crate) fn new(field: &'static str, reason: impl Into<String>) -> Self {
        Self {
            field,
            reason: reason.into(),
        }
    }
}

/// A `kitty-ops` line written in the pre-v2 (legacy) record shape.
///
/// Raised by [`crate::record::parse_op_event`] when a completed line carries
/// no `closed_by`, or when a started/completed line is not v2-parseable.
/// Readers skip the record and point the operator at `spec-kitty upgrade`.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error(
    "Legacy Op record{}: {reason}. Run 'spec-kitty upgrade' to migrate kitty-ops records to the v2 schema.",
    legacy_id_suffix(invocation_id)
)]
pub struct LegacyRecordError {
    /// The line's `invocation_id` when it was a string, else `None`.
    pub invocation_id: Option<String>,
    /// Why the line could not be read as a v2 event.
    pub reason: String,
}

fn legacy_id_suffix(invocation_id: &Option<String>) -> String {
    match invocation_id {
        Some(value) => format!(" {value}"),
        None => String::new(),
    }
}

/// Why [`crate::record::parse_op_event`] could not dispatch a line.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum RecordParseError {
    /// The line is a pre-v2 record. Catchable, so readers and migrations can
    /// identify legacy lines deliberately.
    #[error("{0}")]
    Legacy(#[from] LegacyRecordError),
    /// The line is not an Op lifecycle event at all (`artifact_link`,
    /// `commit_link`, `glossary_checked`, …). The reference raises a plain
    /// `ValueError` here and readers do not treat it as a record.
    #[error("{0}")]
    NotLifecycle(String),
}

/// Failures of the Op-record write/read surface.
#[derive(Debug, Error)]
pub enum InvocationError {
    /// The id failed the canonical boundary before any path was composed.
    #[error(transparent)]
    InvalidInvocationId(#[from] InvocationIdError),

    /// A composed record path was refused (`ValueError` in the reference):
    /// the id was invalid, the path resolved outside `kitty-ops`, or the
    /// final component is a symbolic link.
    #[error("{0}")]
    PathRefused(String),

    /// `InvocationWriteError`: the record could not be written/opened safely.
    #[error("{0}")]
    Write(String),

    /// A generic `InvocationError` from the reference writer: a missing,
    /// unreadable, or identity-mismatched record.
    #[error("{0}")]
    Record(String),

    /// `AlreadyClosedError`: a second `completed` line would be a re-close.
    #[error("Invocation {invocation_id} is already closed.")]
    AlreadyClosed {
        /// The invocation id that is already closed.
        invocation_id: String,
    },

    /// A correlation link must carry exactly one of `ref` or `sha`
    /// (`ValueError` in `writer.append_correlation_link`).
    #[error("{0}")]
    InvalidCorrelationLink(String),
}
