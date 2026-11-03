//! Python-compatible JSON line rendering for Op records.
//!
//! The reference writes every Op line with `json.dumps(model_dump(
//! exclude_none=True))`, i.e. the library defaults: insertion (declared field)
//! order, `", "` between items, `": "` after a key, and `ensure_ascii=True`.
//! `serde_json`'s compact UTF-8 output is a different byte shape, so Op lines
//! are rendered here instead of through `serde_json::to_string`.
//!
//! Only two primitives are needed: a string literal encoder and an ordered
//! object builder. Values are always strings or booleans, so numbers, arrays
//! and nested objects never appear in a well-formed Op line.
//!
//! Escaping reproduces CPython's `py_encode_basestring_ascii`:
//! `"` and `\` are escaped, `\b \f \n \r \t` use their short forms, every
//! other code point below `U+0020` and every code point at or above `U+007F`
//! is written as `\uXXXX` (lowercase hex), and a supplementary-plane code
//! point becomes a UTF-16 surrogate pair (`\ud83d\ude00`).

/// An ordered JSON object rendered the way Python's `json.dumps` renders a dict.
#[derive(Clone, Debug, Default)]
pub struct PythonJsonObject {
    buffer: String,
    empty: bool,
}

impl PythonJsonObject {
    /// Start an empty object.
    pub fn new() -> Self {
        Self {
            buffer: String::from("{"),
            empty: true,
        }
    }

    /// Append a string member.
    pub fn string(&mut self, key: &str, value: &str) {
        self.key(key);
        push_string(&mut self.buffer, value);
    }

    /// Append a boolean member.
    pub fn boolean(&mut self, key: &str, value: bool) {
        self.key(key);
        self.buffer.push_str(if value { "true" } else { "false" });
    }

    /// Append a string member only when the value is present (`exclude_none`).
    pub fn optional_string(&mut self, key: &str, value: Option<&str>) {
        if let Some(value) = value {
            self.string(key, value);
        }
    }

    /// Finish the object and return its JSON text (no trailing newline).
    pub fn finish(mut self) -> String {
        self.buffer.push('}');
        self.buffer
    }

    fn key(&mut self, key: &str) {
        if !self.empty {
            self.buffer.push_str(", ");
        }
        self.empty = false;
        push_string(&mut self.buffer, key);
        self.buffer.push_str(": ");
    }
}

fn push_string(target: &mut String, value: &str) {
    target.push('"');
    for character in value.chars() {
        match character {
            '"' => target.push_str("\\\""),
            '\\' => target.push_str("\\\\"),
            '\u{08}' => target.push_str("\\b"),
            '\u{0c}' => target.push_str("\\f"),
            '\n' => target.push_str("\\n"),
            '\r' => target.push_str("\\r"),
            '\t' => target.push_str("\\t"),
            character if (character as u32) < 0x20 || (character as u32) >= 0x7f => {
                push_ascii_escape(target, character);
            }
            character => target.push(character),
        }
    }
    target.push('"');
}

fn push_ascii_escape(target: &mut String, character: char) {
    let code = character as u32;
    if code <= 0xffff {
        target.push_str(&format!("\\u{code:04x}"));
        return;
    }
    let adjusted = code - 0x1_0000;
    let high = 0xd800 + (adjusted >> 10);
    let low = 0xdc00 + (adjusted & 0x3ff);
    target.push_str(&format!("\\u{high:04x}\\u{low:04x}"));
}

/// CPython `repr()` for a string, used by operator-facing refusal messages.
///
/// ASCII printable characters are kept, `\t \n \r` use their short forms, and
/// other control characters use `\xNN`/`\uNNNN`/`\UNNNNNNNN`. Printable
/// non-ASCII characters are kept verbatim (Python's `isprintable()` rule is
/// Unicode-table driven; this port keeps every non-control scalar, which
/// matches the branch names and ids those messages quote in practice).
/// A value containing a single quote but no double quote is double-quoted,
/// exactly like CPython.
pub fn python_repr(value: &str) -> String {
    let quote = if value.contains('\'') && !value.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(value.len() + 2);
    out.push(quote);
    for character in value.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            character if character == quote => {
                out.push('\\');
                out.push(character);
            }
            character if character.is_control() => {
                let code = character as u32;
                if code <= 0xff {
                    out.push_str(&format!("\\x{code:02x}"));
                } else if code <= 0xffff {
                    out.push_str(&format!("\\u{code:04x}"));
                } else {
                    out.push_str(&format!("\\U{code:08x}"));
                }
            }
            character => out.push(character),
        }
    }
    out.push(quote);
    out
}

/// CPython `repr()` for an arbitrary JSON value, used in parse-refusal messages.
pub fn python_repr_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "None".to_owned(),
        serde_json::Value::Bool(true) => "True".to_owned(),
        serde_json::Value::Bool(false) => "False".to_owned(),
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::String(text) => python_repr(text),
        serde_json::Value::Array(items) => {
            let rendered: Vec<String> = items.iter().map(python_repr_json).collect();
            format!("[{}]", rendered.join(", "))
        }
        serde_json::Value::Object(entries) => {
            let rendered: Vec<String> = entries
                .iter()
                .map(|(key, value)| format!("{}: {}", python_repr(key), python_repr_json(value)))
                .collect();
            format!("{{{}}}", rendered.join(", "))
        }
    }
}

/// Render a `Duration` the way Python renders the float `timeout_s`.
pub fn python_seconds(duration: std::time::Duration) -> String {
    let seconds = duration.as_secs_f64();
    if seconds.fract() == 0.0 {
        format!("{seconds:.1}")
    } else {
        format!("{seconds}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_separators_and_member_order_match_python() {
        let mut object = PythonJsonObject::new();
        object.string("invocation_id", "01KTK5JBD69FQ8XVRFV1J630MJ");
        object.string("profile_id", "p");
        object.string("started_at", "2026-06-10T20:00:00+00:00");
        assert_eq!(
            object.finish(),
            r#"{"invocation_id": "01KTK5JBD69FQ8XVRFV1J630MJ", "profile_id": "p", "started_at": "2026-06-10T20:00:00+00:00"}"#
        );
    }

    #[test]
    fn ascii_escaping_matches_python_for_controls_and_unicode() {
        let mut object = PythonJsonObject::new();
        object.string("k", "héllo ☃ 𝄞 \u{1} \u{7f} \"q\" \\ tab\tnl\nend");
        assert_eq!(
            object.finish(),
            r#"{"k": "h\u00e9llo \u2603 \ud834\udd1e \u0001 \u007f \"q\" \\ tab\tnl\nend"}"#
        );
    }

    #[test]
    fn optional_member_is_omitted_when_absent() {
        let mut object = PythonJsonObject::new();
        object.string("a", "1");
        object.optional_string("b", None);
        object.boolean("c", false);
        assert_eq!(object.finish(), r#"{"a": "1", "c": false}"#);
    }

    #[test]
    fn python_repr_quotes_like_cpython() {
        assert_eq!(python_repr("main"), "'main'");
        assert_eq!(python_repr("a'b"), "\"a'b\"");
        assert_eq!(python_repr("a\"b"), "'a\"b'");
        assert_eq!(python_repr("a'b\"c"), "'a\\'b\"c'");
        assert_eq!(python_repr("back\\slash"), "'back\\\\slash'");
        assert_eq!(python_repr("tab\there"), "'tab\\there'");
        assert_eq!(python_repr("é"), "'é'");
        assert_eq!(python_repr("\u{0}\u{1f}"), "'\\x00\\x1f'");
        assert_eq!(python_repr("\u{7f}"), "'\\x7f'");
    }

    #[test]
    fn python_seconds_matches_float_rendering() {
        assert_eq!(python_seconds(std::time::Duration::from_secs(10)), "10.0");
        assert_eq!(python_seconds(std::time::Duration::from_millis(200)), "0.2");
    }
}
