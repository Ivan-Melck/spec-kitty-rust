//! Python-compatible JSON encoding and the canonical log-row sanitizer.
//!
//! The pinned baseline writes every status/lifecycle row through
//! `json.dumps(row, sort_keys=True)` (`store.serialize_event_line`) after
//! `events.sanitize_event_for_log`. That is **not** serde's compact form: Python
//! sorts keys, separates with `", "` / `": "`, and escapes every character
//! outside printable ASCII (including `0x7f` and all non-ASCII) as `\uXXXX`
//! (surrogate pairs above the BMP).
//!
//! Reproducing those bytes matters for two reasons: the rows are git-committed
//! and diffed, and the differential fixtures compare the native writer's output
//! byte-for-byte with the pinned oracle. [`python_compact_json`] is the single
//! encoder used for log rows.
//!
//! [`python_indented_json`] reproduces `json.dumps(obj, sort_keys=True,
//! indent=2, ensure_ascii=False)` for callers that need the `status.json`
//! shape; [`crate::snapshot::render_snapshot_json`] remains the snapshot
//! renderer (already pinned byte-identical by the replay fixtures).

use serde_json::{Map, Value};

/// PII keys the baseline strips at **every** nesting level.
pub const PII_FIELDS: [&str; 5] = [
    "machine_name",
    "hostname",
    "workspace_path",
    "developer_name",
    "developer_email",
];

/// Session timestamp keys replaced by `session_duration_s` (top level only).
const SESSION_STARTED: &str = "session_started_at";
const SESSION_ENDED: &str = "session_ended_at";
const SESSION_DURATION: &str = "session_duration_s";

/// Return a copy of `envelope` with baseline PII stripped.
///
/// Mirrors `specify_cli.events.sanitizer.sanitize_event_for_log`:
///
/// - `PII_FIELDS` are removed from every object, at every depth;
/// - arrays are recursed into;
/// - `session_started_at` / `session_ended_at` are consumed at the top level
///   only: both present and parseable becomes `session_duration_s`
///   (whole seconds, truncated toward zero); any unparseable or lone value is
///   removed without replacement;
/// - every other field, including `node_id`, `build_id`, `mission_id`,
///   `project_uuid` and `git_branch`, is preserved.
///
/// The input is never mutated.
pub fn sanitize_event_for_log(envelope: &Value) -> Value {
    let Value::Object(object) = envelope else {
        // The baseline takes a dict; a non-object has no PII keys to strip and
        // is returned unchanged (the writer refuses it later).
        return envelope.clone();
    };
    let mut stripped = strip_pii_recursive(envelope);
    if let Value::Object(map) = &mut stripped {
        apply_session_timestamps(object, map);
    }
    stripped
}

fn strip_pii_recursive(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut cleaned = Map::new();
            for (key, item) in object {
                if PII_FIELDS.contains(&key.as_str()) {
                    continue;
                }
                cleaned.insert(key.clone(), strip_pii_recursive(item));
            }
            Value::Object(cleaned)
        }
        Value::Array(items) => Value::Array(items.iter().map(strip_pii_recursive).collect()),
        other => other.clone(),
    }
}

fn apply_session_timestamps(original: &Map<String, Value>, cleaned: &mut Map<String, Value>) {
    let started = original.get(SESSION_STARTED).and_then(Value::as_str);
    let ended = original.get(SESSION_ENDED).and_then(Value::as_str);
    // The baseline pops both keys unconditionally, whatever their values.
    cleaned.remove(SESSION_STARTED);
    cleaned.remove(SESSION_ENDED);
    let (Some(started), Some(ended)) = (started, ended) else {
        return;
    };
    let (Ok(started), Ok(ended)) = (
        sk_kernel::clock::parse_iso(started),
        sk_kernel::clock::parse_iso(ended),
    ) else {
        return;
    };
    // Python `int((ended - started).total_seconds())` truncates toward zero.
    let millis = (ended - started).num_milliseconds();
    let seconds = millis / 1000;
    cleaned.insert(SESSION_DURATION.to_owned(), Value::from(seconds));
}

/// The canonical single-line JSON for one event-log row (no trailing newline).
///
/// Runs the baseline PII sanitizer and then encodes with Python's
/// `json.dumps(sanitized, sort_keys=True)` bytes.
pub fn serialize_event_line(row: &Value) -> String {
    python_compact_json(&sanitize_event_for_log(row))
}

/// Encode a value exactly as Python's `json.dumps(value, sort_keys=True)`
/// (default separators, `ensure_ascii=True`).
///
/// Finite floats use Python's fixed/scientific thresholds, signed and padded
/// exponents, and shortest round-trip digits. Arbitrary-precision integer
/// tokens are preserved exactly; non-standard NaN/Infinity literals are not
/// part of the accepted JSON grammar.
pub fn python_compact_json(value: &Value) -> String {
    let mut out = String::new();
    write_compact(value, &mut out);
    out
}

/// Encode a value exactly as Python's
/// `json.dumps(value, sort_keys=True, indent=2, ensure_ascii=False)` (trailing
/// newline included).
pub fn python_indented_json(value: &Value) -> String {
    let mut out = String::new();
    write_indented(value, 0, &mut out);
    out.push('\n');
    out
}

fn write_compact(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(number) => out.push_str(&python_number(number)),
        Value::String(text) => write_python_string(text, out, true),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                write_compact(item, out);
            }
            out.push(']');
        }
        Value::Object(object) => {
            out.push('{');
            let mut keys: Vec<&String> = object.keys().collect();
            keys.sort();
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                write_python_string(key, out, true);
                out.push_str(": ");
                write_compact(&object[key], out);
            }
            out.push('}');
        }
    }
}

fn write_indented(value: &Value, depth: usize, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(number) => out.push_str(&python_number(number)),
        Value::String(text) => write_python_string(text, out, false),
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                newline_indent(depth + 1, out);
                write_indented(item, depth + 1, out);
            }
            newline_indent(depth, out);
            out.push(']');
        }
        Value::Object(object) => {
            if object.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            let mut keys: Vec<&String> = object.keys().collect();
            keys.sort();
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                newline_indent(depth + 1, out);
                write_python_string(key, out, false);
                out.push_str(": ");
                write_indented(&object[key], depth + 1, out);
            }
            newline_indent(depth, out);
            out.push('}');
        }
    }
}

/// JSON integer tokens may exceed f64, but a floating-point token must remain
/// finite. Keep the pre-arbitrary_precision overflow refusal at input boundaries.
pub(crate) fn has_unrepresentable_float(value: &Value) -> bool {
    match value {
        Value::Number(number) => {
            number.to_string().contains(['.', 'e', 'E']) && number.as_f64().is_none()
        }
        Value::Array(items) => items.iter().any(has_unrepresentable_float),
        Value::Object(object) => object.values().any(has_unrepresentable_float),
        _ => false,
    }
}

fn python_number(number: &serde_json::Number) -> String {
    let raw = number.to_string();
    if !raw.contains(['.', 'e', 'E']) {
        return if raw == "-0" { "0".to_owned() } else { raw };
    }
    let Some(value) = number.as_f64().filter(|value| value.is_finite()) else {
        return raw;
    };
    // serde_json's pinned shortest-roundtrip formatter handles decimal ties
    // like Python; Rust's Display/e formatter can pick the other shortest digit.
    let shortest = serde_json::Number::from_f64(value)
        .expect("finite float")
        .to_string();
    let (sign, unsigned) = shortest
        .strip_prefix('-')
        .map_or(("", shortest.as_str()), |digits| ("-", digits));
    let (mantissa, scale) = match unsigned.split_once('e') {
        Some((mantissa, scale)) => (mantissa, scale.parse::<i32>().expect("float exponent")),
        None => (unsigned, 0),
    };
    let decimal = mantissa.find('.').unwrap_or(mantissa.len()) as i32;
    let raw_digits = mantissa.replace('.', "");
    let leading = raw_digits
        .find(|digit: char| digit != '0')
        .unwrap_or(raw_digits.len());
    let mut digits = raw_digits[leading..].trim_end_matches('0').to_owned();
    let exponent = if digits.is_empty() {
        digits.push('0');
        0
    } else {
        scale + decimal - leading as i32 - 1
    };
    if !(-4..16).contains(&exponent) {
        let tail = if digits.len() > 1 {
            format!(".{}", &digits[1..])
        } else {
            String::new()
        };
        return format!("{sign}{}{tail}e{exponent:+03}", &digits[..1]);
    }
    let point = exponent + 1;
    if point <= 0 {
        return format!("{sign}0.{}{digits}", "0".repeat((-point) as usize));
    }
    let point = point as usize;
    if point >= digits.len() {
        return format!("{sign}{digits}{}.0", "0".repeat(point - digits.len()));
    }
    format!("{sign}{}.{}", &digits[..point], &digits[point..])
}

fn newline_indent(depth: usize, out: &mut String) {
    out.push('\n');
    for _ in 0..depth {
        out.push_str("  ");
    }
}

/// Python's `json.encoder.py_encode_basestring[_ascii]` escaping.
fn write_python_string(text: &str, out: &mut String, ascii_only: bool) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            character if !ascii_only && character >= ' ' => out.push(character),
            character if (' '..='~').contains(&character) => out.push(character),
            character => write_unicode_escape(character, out),
        }
    }
    out.push('"');
}

fn write_unicode_escape(character: char, out: &mut String) {
    let code = character as u32;
    if code <= 0xffff {
        push_escape(code as u16, out);
        return;
    }
    let adjusted = code - 0x1_0000;
    let high = 0xd800 + ((adjusted >> 10) as u16);
    let low = 0xdc00 + ((adjusted & 0x3ff) as u16);
    push_escape(high, out);
    push_escape(low, out);
}

fn push_escape(unit: u16, out: &mut String) {
    out.push_str("\\u");
    out.push_str(&format!("{unit:04x}"));
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn compact_encoding_matches_python_default_separators_and_ascii_escapes() {
        let value = json!({"b": [1, true, null], "a": "caf\u{e9}\n\u{7f}", "c": 2.5});
        assert_eq!(
            python_compact_json(&value),
            r#"{"a": "caf\u00e9\n\u007f", "b": [1, true, null], "c": 2.5}"#
        );
    }

    #[test]
    fn numeric_wire_encoding_matches_python_thresholds_and_big_integers() {
        for (input, expected) in [
            ("810553441865041.25", "810553441865041.2"),
            ("-0", "0"),
            ("1e30", "1e+30"),
            ("1e-5", "1e-05"),
            ("1e-4", "0.0001"),
            ("1e16", "1e+16"),
            ("1e15", "1000000000000000.0"),
            ("-0.0", "-0.0"),
            ("12345678901234567890123", "12345678901234567890123"),
        ] {
            let value: Value = serde_json::from_str(input).unwrap();
            assert_eq!(python_compact_json(&value), expected);
        }
        assert!(has_unrepresentable_float(
            &serde_json::from_str("1e400").unwrap()
        ));
    }

    #[test]
    fn finite_ieee_vectors_match_the_verified_python_oracle() {
        let fixture: Value =
            serde_json::from_str(include_str!("../tests/fixtures/mutation/edge-cases.json"))
                .unwrap();
        assert_eq!(
            fixture["baseline"],
            "9aed3a8012cb5eb22c0128b14d525039f95fea7a"
        );
        let cases = fixture["floats"].as_array().unwrap();
        assert_eq!(cases.len(), 2056);
        for case in cases {
            let bits = u64::from_str_radix(case["bits"].as_str().unwrap(), 16).unwrap();
            let number = serde_json::Number::from_f64(f64::from_bits(bits)).unwrap();
            assert_eq!(
                python_number(&number),
                case["expected"].as_str().unwrap(),
                "bits: {bits:016x}"
            );
        }
    }

    #[test]
    fn astral_characters_become_surrogate_pairs() {
        assert_eq!(
            python_compact_json(&json!("\u{1f600}")),
            r#""\ud83d\ude00""#
        );
    }

    #[test]
    fn empty_containers_and_quotes_are_python_shaped() {
        assert_eq!(
            python_compact_json(&json!({"a": {}, "b": [], "c": "\"q\""})),
            r#"{"a": {}, "b": [], "c": "\"q\""}"#
        );
    }

    #[test]
    fn sanitizer_strips_pii_at_every_depth() {
        let row = json!({
            "hostname": "h",
            "payload": {"developer_email": "e", "keep": 1, "nested": [{"workspace_path": "/x"}]},
            "mission_id": "01ABC",
        });
        assert_eq!(
            sanitize_event_for_log(&row),
            json!({"payload": {"keep": 1, "nested": [{}]}, "mission_id": "01ABC"})
        );
    }

    #[test]
    fn sanitizer_replaces_session_timestamps_with_whole_seconds() {
        let row = json!({
            "session_started_at": "2026-01-01T00:00:00+00:00",
            "session_ended_at": "2026-01-01T00:00:02.750000+00:00",
        });
        assert_eq!(
            sanitize_event_for_log(&row),
            json!({"session_duration_s": 2})
        );
        let lone = json!({"session_started_at": "2026-01-01T00:00:00+00:00", "a": 1});
        assert_eq!(sanitize_event_for_log(&lone), json!({"a": 1}));
        let malformed = json!({"session_started_at": "not-a-time", "session_ended_at": "x"});
        assert_eq!(sanitize_event_for_log(&malformed), json!({}));
    }

    #[test]
    fn indented_rendering_matches_python_indent_two() {
        let value = json!({"b": [1, 2], "a": {"nested": true, "empty": {}}});
        assert_eq!(
            python_indented_json(&value),
            "{\n  \"a\": {\n    \"empty\": {},\n    \"nested\": true\n  },\n  \"b\": [\n    1,\n    2\n  ]\n}\n"
        );
    }
}
