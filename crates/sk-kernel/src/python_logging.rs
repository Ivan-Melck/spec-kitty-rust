//! Pinned Source-default `logging` delivery for one diagnostic record.
//!
//! The reference (`src/charter/activation/resolver.py`) reports unresolved
//! activation tokens with `_LOGGER.warning("%s", warning)`. No `basicConfig` or
//! `dictConfig` runs anywhere in the repository's CLI or library entry points, so
//! the measured delivery path for that call is CPython 3.11.15's **default**
//! configuration, probed against the pinned interpreter:
//!
//! * `logging.getLogger("charter.activation.resolver")` and every ancestor,
//!   including the root logger, hold **no** handlers (`callHandlers` counts zero);
//! * `logging.lastResort` is `_StderrHandler(level=WARNING)` — a `StreamHandler`
//!   whose `stream` property is `sys.stderr` resolved at emit time;
//! * a handler with no `Formatter` formats through `logging.Formatter()`, whose
//!   default style is `%` and whose format string is `'%(message)s'`, and
//!   `StreamHandler.terminator` is `'\n'` — so the delivered text is exactly
//!   `record.getMessage() + "\n"`, with no level name, logger name, timestamp or
//!   source location;
//! * `Logger.isEnabledFor(WARNING)` is true while `logging.root.manager.disable`
//!   is `0`, and `callHandlers` delivers only when `record.levelno >=
//!   lastResort.level`;
//! * `logging.raiseExceptions` is true, so a failing `Handler.emit` (an
//!   `OSError`/`UnicodeEncodeError` from `stream.write` or `flush`) is swallowed
//!   by `handleError`, which itself tolerates a broken stderr; `Logger.warning`
//!   returns normally and the caller continues. With `raiseExceptions` false the
//!   failure is dropped silently. A `/dev/full` stderr still ends the **process**
//!   with CPython's shutdown-flush exit 120 — a process-level fact this library
//!   cannot reproduce and does not claim.
//!
//! This module models **only** that default path. Custom handler hierarchies,
//! `basicConfig`/`dictConfig`, `handleError`'s `--- Logging error ---` traceback
//! bytes, `logging.lastResort = None`'s "No handlers could be found" line, and
//! process-start codec capture are not implemented and are not implied.

use crate::PythonString;
use crate::python_stream::{StreamError, write_stderr_line};

/// CPython's `logging.WARNING` level number.
pub const WARNING: i32 = 30;
/// CPython's `logging.ERROR` level number.
pub const ERROR: i32 = 40;
/// CPython's `logging.CRITICAL` level number.
pub const CRITICAL: i32 = 50;
/// CPython's `logging.NOTSET` level number.
pub const NOTSET: i32 = 0;

/// The process-level `logging` state the default warning path consults.
///
/// The reference reads this state from module globals (`logging.disable`,
/// `logging.lastResort.level`, `logging.raiseExceptions`), which a program sets
/// once at start-up. Native has no logging configuration surface, so the state is
/// an explicit value: [`LoggingPolicy::source_default`] is the measured default,
/// and a caller that must reproduce an already-configured process passes a
/// different value instead of relying on a hidden global.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoggingPolicy {
    disabled_level: i32,
    last_resort_level: i32,
    raise_exceptions: bool,
}

impl Default for LoggingPolicy {
    fn default() -> Self {
        Self::source_default()
    }
}

impl LoggingPolicy {
    /// The measured CPython default: nothing disabled, `lastResort` at WARNING,
    /// `raiseExceptions` true.
    #[must_use]
    pub const fn source_default() -> Self {
        Self {
            disabled_level: NOTSET,
            last_resort_level: WARNING,
            raise_exceptions: true,
        }
    }

    /// `logging.disable(level)`: every record at or below `level` is refused
    /// before any handler runs.
    #[must_use]
    pub const fn with_disabled_level(mut self, level: i32) -> Self {
        self.disabled_level = level;
        self
    }

    /// `logging.lastResort.setLevel(level)`: `callHandlers` delivers only when the
    /// record's level is at least this value.
    #[must_use]
    pub const fn with_last_resort_level(mut self, level: i32) -> Self {
        self.last_resort_level = level;
        self
    }

    /// `logging.raiseExceptions`.
    #[must_use]
    pub const fn with_raise_exceptions(mut self, raise: bool) -> Self {
        self.raise_exceptions = raise;
        self
    }

    /// `Logger.isEnabledFor(level)`.
    #[must_use]
    pub const fn is_enabled_for(&self, level: i32) -> bool {
        level > self.disabled_level
    }

    /// Whether `callHandlers` reaches `lastResort` for this record.
    #[must_use]
    pub const fn last_resort_delivers(&self, level: i32) -> bool {
        level >= self.last_resort_level
    }

    /// `logging.raiseExceptions` as this policy holds it.
    #[must_use]
    pub const fn raise_exceptions(&self) -> bool {
        self.raise_exceptions
    }

    /// `logging.root.manager.disable` as this policy holds it.
    #[must_use]
    pub const fn disabled_level(&self) -> i32 {
        self.disabled_level
    }

    /// `logging.lastResort.level` as this policy holds it.
    #[must_use]
    pub const fn last_resort_level(&self) -> i32 {
        self.last_resort_level
    }
}

/// Why the reference produced no bytes for a record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Suppression {
    /// `Logger.isEnabledFor` refused: `logging.disable` is at or above the level.
    Disabled,
    /// `lastResort`'s level is above the record's level.
    LastResortLevel,
}

/// What one `_LOGGER.warning` call did, in the reference's own terms.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WarningDelivery {
    /// `stream.write(message + "\n")` and `flush()` both succeeded.
    Delivered,
    /// The record never reached a handler; nothing was written.
    Suppressed(Suppression),
    /// `Handler.emit` raised and `handleError` swallowed it. `Logger.warning`
    /// returned normally in the reference, so the caller continues; `reported`
    /// records whether `raiseExceptions` would have attempted the
    /// `--- Logging error ---` report (which this module does not reproduce).
    Handled {
        /// The real stream/encoding failure.
        cause: StreamError,
        /// `logging.raiseExceptions` at the call.
        reported: bool,
    },
}

/// Deliver one `_LOGGER.warning("%s", message)` record under the default policy.
///
/// # Errors
///
/// The stderr stream policy cannot deliver at all: an invalid start-up value
/// (`PYTHONUTF8`, an undecodable variable) or a Source-valid codec/error handler
/// this native composition does not carry. Neither is a `logging` handler error and
/// neither is reported as delivered; the capability-gap class stays an explicit
/// unaccepted native gap, not a Source-invalid label.
pub fn deliver_warning(message: &PythonString) -> Result<WarningDelivery, StreamError> {
    deliver_warning_with(LoggingPolicy::source_default(), message)
}

/// [`deliver_warning`] under an explicit process logging policy.
///
/// # Errors
///
/// As [`deliver_warning`].
pub fn deliver_warning_with(
    policy: LoggingPolicy,
    message: &PythonString,
) -> Result<WarningDelivery, StreamError> {
    if !policy.is_enabled_for(WARNING) {
        return Ok(WarningDelivery::Suppressed(Suppression::Disabled));
    }
    if !policy.last_resort_delivers(WARNING) {
        return Ok(WarningDelivery::Suppressed(Suppression::LastResortLevel));
    }
    match write_stderr_line(message) {
        Ok(()) => Ok(WarningDelivery::Delivered),
        Err(cause) if cause.stops_warning_delivery() => Err(cause),
        Err(cause) => Ok(WarningDelivery::Handled {
            cause,
            reported: policy.raise_exceptions(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(text: &str) -> PythonString {
        PythonString::from_scalars(text)
    }

    #[test]
    fn the_default_policy_is_the_measured_cpython_configuration() {
        let policy = LoggingPolicy::source_default();
        assert_eq!(policy.disabled_level(), NOTSET);
        assert_eq!(policy.last_resort_level(), WARNING);
        assert!(policy.raise_exceptions());
        assert!(policy.is_enabled_for(WARNING));
        assert!(policy.last_resort_delivers(WARNING));
        assert!(!policy.last_resort_delivers(NOTSET));
    }

    #[test]
    fn a_disabled_level_suppresses_before_any_handler() {
        let policy = LoggingPolicy::source_default().with_disabled_level(WARNING);
        assert_eq!(
            deliver_warning_with(policy, &message("unresolved")).unwrap(),
            WarningDelivery::Suppressed(Suppression::Disabled)
        );
        let above = LoggingPolicy::source_default().with_disabled_level(ERROR);
        assert!(
            !above.is_enabled_for(WARNING),
            "`logging.disable(ERROR)` refuses WARNING (disable >= level)"
        );
        assert!(above.is_enabled_for(CRITICAL));
    }

    #[test]
    fn a_last_resort_level_above_warning_suppresses_delivery() {
        let policy = LoggingPolicy::source_default().with_last_resort_level(ERROR);
        assert_eq!(
            deliver_warning_with(policy, &message("unresolved")).unwrap(),
            WarningDelivery::Suppressed(Suppression::LastResortLevel)
        );
    }

    #[test]
    fn the_startup_class_is_the_one_outcome_that_is_not_a_handler_error() {
        use crate::python_stream::{StreamError, WriteClass};
        assert!(
            StreamError::InvalidStartup {
                detail: "PYTHONIOENCODING".to_owned()
            }
            .is_stream_startup_failure()
        );
        let gap = StreamError::UnsupportedEncoding {
            name: "cp1252".to_owned(),
            detail: String::new(),
        };
        assert!(
            !gap.is_stream_startup_failure(),
            "a Source-valid codec this native stream lacks is a capability gap, not Source-invalidity"
        );
        assert!(gap.is_native_capability_gap());
        assert!(gap.stops_warning_delivery());
        assert!(
            StreamError::UnsupportedHandler {
                name: "namereplace"
            }
            .is_native_capability_gap()
        );
        assert!(
            !StreamError::Io {
                class: WriteClass::Stderr,
                detail: "No space left on device".to_owned()
            }
            .is_stream_startup_failure(),
            "a physical write failure is caught by Handler.emit"
        );
        assert!(
            !StreamError::BrokenPipe {
                class: WriteClass::Stderr
            }
            .is_stream_startup_failure()
        );
        assert!(
            !StreamError::BrokenPipe {
                class: WriteClass::Stderr
            }
            .stops_warning_delivery()
        );
    }
}
