//! The canonical wall-clock door.
//!
//! Every wall-clock read in this crate — and every consumer that follows the
//! same contract — routes through this module rather than calling
//! `chrono::Utc::now` or `SystemTime::now` directly. The module owns one
//! producer per distinct serialization contract, mirroring upstream
//! `kernel.clock`:
//!
//! - [`now_utc_iso`]: aware-UTC ISO 8601 with `+00:00`, microseconds only
//!   when non-zero (Python `datetime.isoformat()` shape).
//! - [`now_utc_seconds`]: the same form truncated to whole seconds.
//! - [`now_utc_stamp`]: `%Y-%m-%dT%H:%M:%SZ` (the canonical second stamp).
//! - [`now_utc_compact_stamp`]: `%Y%m%dT%H%M%SZ` (no separators).
//! - [`now_utc`]: the `DateTime<Utc>` value.
//! - [`now_epoch`]: Unix epoch seconds.
//!
//! A logical (Lamport) clock and a monotonic duration clock are deliberately
//! different concepts and are not provided here.
//!
//! The [`Clock`] trait is the injection seam. [`SystemClock`] is the only real
//! implementation; [`FrozenClock`] is the deterministic test double. Free
//! functions read through a fresh `SystemClock`, exactly one real boundary.

use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};

use crate::KernelError;

pub use chrono::{DateTime as UtcDateTime, Utc as UtcZone};

/// The canonical second-precision UTC stamp format.
pub const UTC_SECOND_TIMESTAMP_FORMAT: &str = "%Y-%m-%dT%H:%M:%SZ";

/// Compact UTC stamp format (no separators).
pub const COMPACT_STAMP_FORMAT: &str = "%Y%m%dT%H%M%SZ";

/// Pydantic model-JSON presentation of an already validated canonical datetime.
///
/// This is a presentation adapter, NOT a datetime parser: the owning model/YAML
/// constructor already produced Python ISO text (civil fields, six-digit
/// nonzero microseconds, optional offset). JSON uses `T` and `Z` for UTC;
/// Python's own isoformat/sort-key spelling remains a separate contract.
pub fn model_json_datetime_text(canonical_iso: &str) -> String {
    let text = canonical_iso.replacen(' ', "T", 1);
    match text.strip_suffix("+00:00") {
        Some(civil) => format!("{civil}Z"),
        None => text,
    }
}

/// The injectable wall-clock seam.
///
/// Every default method derives from [`Clock::now`], so freezing one clock
/// freezes every producer shape uniformly.
pub trait Clock {
    /// The current aware-UTC instant.
    fn now(&self) -> DateTime<Utc>;

    /// The canonical aware-UTC ISO 8601 form.
    fn now_iso(&self) -> String {
        isoformat_utc(self.now())
    }

    /// The canonical aware-UTC ISO form truncated to seconds.
    fn now_utc_seconds(&self) -> String {
        isoformat_utc_seconds(self.now())
    }

    /// The canonical `%Y-%m-%dT%H:%M:%SZ` stamp.
    fn now_utc_stamp(&self) -> String {
        self.now().format(UTC_SECOND_TIMESTAMP_FORMAT).to_string()
    }

    /// The canonical compact `%Y%m%dT%H%M%SZ` stamp.
    fn now_utc_compact_stamp(&self) -> String {
        self.now().format(COMPACT_STAMP_FORMAT).to_string()
    }

    /// The current instant as Unix epoch seconds.
    fn now_epoch(&self) -> f64 {
        epoch_seconds(self.now())
    }
}

/// The single real wall-clock boundary.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }

    fn now_epoch(&self) -> f64 {
        match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.as_secs_f64(),
            Err(error) => -error.duration().as_secs_f64(),
        }
    }
}

/// A deterministic clock double: every method derives from one fixed instant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrozenClock {
    instant: DateTime<Utc>,
}

impl FrozenClock {
    /// Freeze a clock at `instant`.
    pub fn new(instant: DateTime<Utc>) -> Self {
        Self { instant }
    }

    /// The fixed instant.
    pub fn instant(&self) -> DateTime<Utc> {
        self.instant
    }
}

impl Clock for FrozenClock {
    fn now(&self) -> DateTime<Utc> {
        self.instant
    }
}

/// The current aware-UTC instant.
pub fn now_utc() -> DateTime<Utc> {
    SystemClock.now()
}

/// The canonical aware-UTC ISO 8601 string.
pub fn now_utc_iso() -> String {
    SystemClock.now_iso()
}

/// The canonical `%Y-%m-%dT%H:%M:%SZ` stamp.
pub fn now_utc_stamp() -> String {
    SystemClock.now_utc_stamp()
}

/// The canonical compact `%Y%m%dT%H%M%SZ` stamp.
pub fn now_utc_compact_stamp() -> String {
    SystemClock.now_utc_compact_stamp()
}

/// The aware-UTC ISO form truncated to whole seconds.
pub fn now_utc_seconds() -> String {
    SystemClock.now_utc_seconds()
}

/// Current Unix epoch seconds.
pub fn now_epoch() -> f64 {
    SystemClock.now_epoch()
}

/// Parse an ISO 8601 / RFC 3339 string into an aware-UTC instant.
///
/// Offsets are accepted and normalised to UTC. A bare local-style timestamp
/// without an offset is interpreted as UTC; that extension exists so callers
/// holding Python `datetime.isoformat()` values from a naive context still get
/// a deterministic result.
pub fn parse_iso(value: &str) -> Result<DateTime<Utc>, KernelError> {
    if let Ok(parsed) = DateTime::parse_from_rfc3339(value) {
        return Ok(parsed.with_timezone(&Utc));
    }
    // Python `datetime.fromisoformat` also accepts a numeric offset without a
    // colon, a date-only value, and a space separator.
    for format in ["%Y-%m-%dT%H:%M:%S%.f%z", "%Y-%m-%d %H:%M:%S%.f%z"] {
        if let Ok(parsed) = DateTime::parse_from_str(value, format) {
            return Ok(parsed.with_timezone(&Utc));
        }
    }
    for format in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f"] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(value, format) {
            return Ok(Utc.from_utc_datetime(&naive));
        }
    }
    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d")
        && let Some(naive) = date.and_hms_opt(0, 0, 0)
    {
        return Ok(Utc.from_utc_datetime(&naive));
    }
    Err(KernelError::InvalidTimestamp {
        value: value.to_owned(),
        reason: "expected an ISO 8601 timestamp".to_owned(),
    })
}

/// Parse a stamp with an explicit `chrono` format string.
pub fn parse_stamp(value: &str, format: &str) -> Result<DateTime<Utc>, KernelError> {
    if let Ok(parsed) = DateTime::parse_from_str(value, format) {
        return Ok(parsed.with_timezone(&Utc));
    }
    if let Ok(naive) = NaiveDateTime::parse_from_str(value, format) {
        return Ok(Utc.from_utc_datetime(&naive));
    }
    Err(KernelError::InvalidTimestamp {
        value: value.to_owned(),
        reason: format!("does not match format {format:?}"),
    })
}

/// Format an instant with an explicit `chrono` format string.
pub fn format_stamp(value: DateTime<Utc>, format: &str) -> String {
    value.format(format).to_string()
}

/// Convert Unix epoch seconds into an aware-UTC instant.
pub fn from_epoch(value: f64) -> Result<DateTime<Utc>, KernelError> {
    if !value.is_finite() {
        return Err(KernelError::InvalidTimestamp {
            value: value.to_string(),
            reason: "epoch seconds must be finite".to_owned(),
        });
    }
    // Python's `datetime.fromtimestamp` has microsecond resolution. Round to
    // the nearest microsecond and carry, so a value that came from Python
    // round-trips to the same instant rather than accumulating f64 noise.
    let seconds = value.floor();
    let micros = ((value - seconds) * 1_000_000.0).round();
    let (seconds, micros) = if micros >= 1_000_000.0 {
        (seconds + 1.0, 0)
    } else {
        (seconds, micros as u32)
    };
    let nanos = micros * 1_000;
    if seconds < i64::MIN as f64 || seconds > i64::MAX as f64 {
        return Err(KernelError::InvalidTimestamp {
            value: value.to_string(),
            reason: "epoch seconds are outside the representable range".to_owned(),
        });
    }
    DateTime::from_timestamp(seconds as i64, nanos).ok_or_else(|| KernelError::InvalidTimestamp {
        value: value.to_string(),
        reason: "epoch seconds are outside the representable range".to_owned(),
    })
}

fn epoch_seconds(instant: DateTime<Utc>) -> f64 {
    instant.timestamp() as f64 + f64::from(instant.timestamp_subsec_nanos()) / 1_000_000_000.0
}

/// Python-compatible `datetime.isoformat()` value for an aware-UTC instant.
fn isoformat_utc(instant: DateTime<Utc>) -> String {
    let base = instant.format("%Y-%m-%dT%H:%M:%S").to_string();
    let micros = instant.timestamp_subsec_micros();
    if micros == 0 {
        format!("{base}+00:00")
    } else {
        format!("{base}.{micros:06}+00:00")
    }
}

fn isoformat_utc_seconds(instant: DateTime<Utc>) -> String {
    format!("{}+00:00", instant.format("%Y-%m-%dT%H:%M:%S"))
}
