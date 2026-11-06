//! Bounded, classify-never-crash `meta.json` reads for the diagnostics.
//!
//! The upstream identity/topology audits read `meta.json` through a fail-closed
//! reader that distinguishes *absent* (a field-absent state) from *unreadable*
//! (a finding). That distinction is what this module reproduces, on top of the
//! canonical Kernel bounded read: a corrupt, non-object, non-UTF-8 or
//! oversized file is [`MetaRead::Unreadable`], never an empty object and never
//! `Ok(())` with missing keys.
//!
//! Reads are bounded at [`MAX_META_BYTES`]. The upstream reader is unbounded, so
//! an oversized `meta.json` is a *bounded-inspection* refusal here — a distinct
//! native capability, not a claim that the reference refuses it.

use std::path::Path;

use serde_json::{Map, Value};
use sk_kernel::{KernelError, read_bounded};

/// Bounded-inspection ceiling for one `meta.json` read (256 KiB).
pub const MAX_META_BYTES: u64 = 256 * 1024;

/// The `meta.json` filename inside one Mission directory.
pub const META_FILE_NAME: &str = "meta.json";

/// The outcome of reading one Mission's `meta.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetaRead {
    /// No `meta.json` on disk.
    Absent,
    /// The parsed JSON object, every key preserved.
    Present(Map<String, Value>),
    /// The file is present but cannot be used; the text is the diagnostic.
    Unreadable(String),
}

impl MetaRead {
    /// The parsed object, or `None` when absent or unreadable.
    pub fn object(&self) -> Option<&Map<String, Value>> {
        match self {
            Self::Present(meta) => Some(meta),
            Self::Absent | Self::Unreadable(_) => None,
        }
    }

    /// Whether a `meta.json` file exists on disk (readable or not).
    pub fn present_on_disk(&self) -> bool {
        !matches!(self, Self::Absent)
    }
}

/// Read `<dir>/meta.json` defensively.
///
/// A symlinked `meta.json` is followed by [`read_bounded`] (which refuses a
/// final symlink only where the caller asked for that); the audit is a
/// read-only reporter, and the Mission directory itself is resolved by the
/// caller. A directory entry where the file should be is `Unreadable`.
pub fn read_meta(dir: &Path) -> MetaRead {
    let path = dir.join(META_FILE_NAME);
    let bytes = match read_bounded(&path, MAX_META_BYTES) {
        Ok(bytes) => bytes,
        Err(KernelError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            return MetaRead::Absent;
        }
        Err(KernelError::TooLarge { limit, .. }) => {
            return MetaRead::Unreadable(format!(
                "{META_FILE_NAME} exceeds the {limit}-byte bounded-inspection limit"
            ));
        }
        Err(error) => return MetaRead::Unreadable(error.to_string()),
    };

    let text = match std::str::from_utf8(&bytes) {
        Ok(text) => text,
        Err(error) => {
            return MetaRead::Unreadable(format!("{META_FILE_NAME} is not valid UTF-8: {error}"));
        }
    };

    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(meta)) => MetaRead::Present(meta),
        Ok(other) => MetaRead::Unreadable(format!(
            "{META_FILE_NAME} is not a JSON object but {}",
            json_kind(&other)
        )),
        Err(error) => MetaRead::Unreadable(format!("{META_FILE_NAME} is not valid JSON: {error}")),
    }
}

/// Python's `bool(value)` for the decoded JSON value.
///
/// The upstream audit reads `meta.get("mission_id") or None`, so truthiness —
/// not the declared `str` type — decides presence: `0`, `0.0`, `""`, `[]` and
/// `{}` are `None`, while any other value (including a number or an object) is
/// kept verbatim.
pub fn python_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|value| value != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

/// Why a `mission_number` value cannot be coerced to a display number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumberCoercionError {
    /// The upstream-style diagnostic.
    pub detail: String,
}

/// Coerce a raw `mission_number` value the way upstream does.
///
/// Accepted: JSON `null`, an integer, a numeric string (leading zeros
/// stripped), an empty/blank string (`None`). Refused: booleans, floats,
/// arrays, objects, sentinel strings (`pending`/`unassigned`/`TBD`) and
/// non-numeric strings. The audit treats a refusal as "no number" while
/// keeping the coercion error available to the caller.
///
/// Integers outside the signed 64-bit range are a refusal here (the reference's
/// `int` is unbounded); the audit reports that as an `error` note rather than
/// inventing a number.
pub fn coerce_mission_number(raw: &Value) -> Result<Option<i64>, NumberCoercionError> {
    let refuse = |detail: String| Err(NumberCoercionError { detail });
    match raw {
        Value::Null => Ok(None),
        Value::Bool(flag) => refuse(format!(
            "meta.json mission_number must be int or null, got bool {flag}"
        )),
        Value::Number(number) => match number.as_i64() {
            Some(value) => Ok(Some(value)),
            None => refuse(format!(
                "meta.json mission_number {number} is outside the native inspection range"
            )),
        },
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            if matches!(trimmed, "pending" | "unassigned" | "TBD") {
                return refuse(format!(
                    "meta.json mission_number must be int or null, got {text:?}; run `spec-kitty migrate backfill-identity` first"
                ));
            }
            if !trimmed.bytes().all(|byte| byte.is_ascii_digit()) {
                return refuse(format!(
                    "meta.json mission_number must be int or null, got {text:?}"
                ));
            }
            let stripped = trimmed.trim_start_matches('0');
            let digits = if stripped.is_empty() { "0" } else { stripped };
            match digits.parse::<i64>() {
                Ok(value) => Ok(Some(value)),
                Err(_) => refuse(format!(
                    "meta.json mission_number {text:?} is outside the native inspection range"
                )),
            }
        }
        other => refuse(format!(
            "meta.json mission_number must be int, str, or null, got {}",
            json_kind(other)
        )),
    }
}

fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn absent_and_unreadable_meta_are_distinct() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_meta(dir.path()), MetaRead::Absent);

        std::fs::write(dir.path().join(META_FILE_NAME), b"{[not json").unwrap();
        assert!(matches!(read_meta(dir.path()), MetaRead::Unreadable(_)));

        std::fs::write(dir.path().join(META_FILE_NAME), b"[]").unwrap();
        assert!(matches!(read_meta(dir.path()), MetaRead::Unreadable(_)));

        std::fs::write(dir.path().join(META_FILE_NAME), b"{\"mission_id\":\"X\"}").unwrap();
        assert_eq!(
            read_meta(dir.path()).object().and_then(|meta| {
                meta.get("mission_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            }),
            Some("X".to_owned())
        );
    }

    #[test]
    fn mission_number_coercion_matches_the_documented_matrix() {
        for (raw, expected) in [
            (json!(null), Ok(None)),
            (json!(7), Ok(Some(7))),
            (json!("042"), Ok(Some(42))),
            (json!(" 8 "), Ok(Some(8))),
            (json!("000"), Ok(Some(0))),
            (json!(""), Ok(None)),
            (json!("   "), Ok(None)),
        ] {
            assert_eq!(coerce_mission_number(&raw), expected, "raw={raw}");
        }
        for raw in [
            json!(true),
            json!(1.5),
            json!("pending"),
            json!("TBD"),
            json!("abc"),
            json!([1]),
            json!({"n": 1}),
        ] {
            assert!(
                coerce_mission_number(&raw).is_err(),
                "expected a refusal for {raw}"
            );
        }
    }

    #[test]
    fn python_truthiness_decides_identity_presence() {
        assert!(!python_truthy(&json!(null)));
        assert!(!python_truthy(&json!("")));
        assert!(!python_truthy(&json!(0)));
        assert!(!python_truthy(&json!([])));
        assert!(!python_truthy(&json!({})));
        assert!(!python_truthy(&json!(false)));
        assert!(python_truthy(&json!("01ABC")));
        assert!(python_truthy(&json!(5)));
        assert!(python_truthy(&json!({"a": 1})));
    }
}
