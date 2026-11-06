//! A small Python-shaped JSON encoder for migration write-back.
//!
//! Python's `json.dumps(..., indent=2, sort_keys=True)` is the wire writer for
//! the run index and for the legacy `run_dir` preview. `serde_json`'s pretty
//! printer differs in two observable ways: it keeps non-ASCII characters raw
//! (Python escapes them by default) and its object order follows the value.
//! This module reproduces the Python shape for the value types these
//! migrations actually carry (objects, arrays, strings, numbers, booleans and
//! null) without adding a dependency.
//!
//! The encoder is deliberately local to the migration tranche: it is not a
//! general JSON implementation and it is never used to read user content.

use serde_json::Value;

/// Render `value` as Python's `json.dumps(value, indent=2, sort_keys=True)`
/// with the default `ensure_ascii=True`.
pub fn python_indented_ascii(value: &Value) -> String {
    let mut out = String::new();
    write_value(value, 0, true, &mut out);
    out
}

fn write_value(value: &Value, depth: usize, ascii_only: bool, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(number) => out.push_str(&python_number(number)),
        Value::String(text) => write_string(text, ascii_only, out),
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
                write_value(item, depth + 1, ascii_only, out);
            }
            newline_indent(depth, out);
            out.push(']');
        }
        Value::Object(mapping) => {
            if mapping.is_empty() {
                out.push_str("{}");
                return;
            }
            // `serde_json::Map` is a BTreeMap by default, so iteration is
            // already lexicographic; copy into a Vec to make that explicit and
            // robust if the feature set ever changes.
            let mut entries: Vec<(&String, &Value)> = mapping.iter().collect();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            out.push('{');
            for (index, (key, item)) in entries.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                newline_indent(depth + 1, out);
                write_string(key, ascii_only, out);
                out.push_str(": ");
                write_value(item, depth + 1, ascii_only, out);
            }
            newline_indent(depth, out);
            out.push('}');
        }
    }
}

fn newline_indent(depth: usize, out: &mut String) {
    out.push('\n');
    for _ in 0..depth {
        out.push_str("  ");
    }
}

fn python_number(number: &serde_json::Number) -> String {
    let raw = number.to_string();
    if !raw.contains(['.', 'e', 'E']) {
        return raw;
    }
    let Some(value) = number.as_f64().filter(|value| value.is_finite()) else {
        return raw;
    };
    // Python's repr switches to scientific notation outside [1e-4, 1e16) and
    // uses a two-digit exponent with an explicit sign.
    let scientific = format!("{value:e}");
    let (mantissa, exponent) = scientific.split_once('e').expect("finite scientific float");
    let exponent: i32 = exponent.parse().expect("scientific exponent");
    if !(-4..16).contains(&exponent) {
        return format!("{mantissa}e{exponent:+03}");
    }
    let (sign, mantissa) = mantissa
        .strip_prefix('-')
        .map_or(("", mantissa), |digits| ("-", digits));
    let digits = mantissa.replace('.', "");
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

fn write_string(text: &str, ascii_only: bool, out: &mut String) {
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
            character if (character as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", character as u32));
            }
            character if ascii_only && !(' '..='~').contains(&character) => {
                let code = character as u32;
                if code <= 0xffff {
                    out.push_str(&format!("\\u{code:04x}"));
                } else {
                    let adjusted = code - 0x1_0000;
                    let high = 0xd800 + (adjusted >> 10);
                    let low = 0xdc00 + (adjusted & 0x3ff);
                    out.push_str(&format!("\\u{high:04x}\\u{low:04x}"));
                }
            }
            character => out.push(character),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matches_python_sorted_indented_ascii_shape() {
        let value = json!({"b": [1, 2], "a": {"x": "é", "y": "\u{1f600}"}});
        assert_eq!(
            python_indented_ascii(&value),
            "{\n  \"a\": {\n    \"x\": \"\\u00e9\",\n    \"y\": \"\\ud83d\\ude00\"\n  },\n  \"b\": [\n    1,\n    2\n  ]\n}"
        );
    }

    #[test]
    fn python_float_formatting_matches_repr() {
        assert_eq!(python_indented_ascii(&json!(1.0)), "1.0");
        assert_eq!(python_indented_ascii(&json!(1.5)), "1.5");
        assert_eq!(python_indented_ascii(&json!(1e-5)), "1e-05");
        assert_eq!(python_indented_ascii(&json!(1e16)), "1e+16");
        assert_eq!(python_indented_ascii(&json!("\u{7f}")), "\"\\u007f\"");
    }

    #[test]
    fn empty_containers_match_python() {
        assert_eq!(python_indented_ascii(&json!({})), "{}");
        assert_eq!(python_indented_ascii(&json!([])), "[]");
    }
}
