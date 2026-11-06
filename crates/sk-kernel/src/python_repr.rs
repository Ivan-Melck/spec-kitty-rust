//! The workspace's one canonical CPython `repr()` string formatter.
//!
//! CPython keeps printable characters verbatim, writes `\t \n \r` in their
//! short forms, escapes the chosen quote and `\\`, and writes every other
//! non-printable code point as `\xNN` below `U+0100`, `\uNNNN` below `U+10000`,
//! otherwise `\UNNNNNNNN` (lowercase hex, like `unicode_repr`).
//!
//! Printability is the verified `crate::python_unicode` `str.isprintable()`
//! table, not Rust's `char::is_control()` (which is `Cc` only). A value
//! containing a single quote but no double quote is double-quoted, exactly like
//! CPython. Lone surrogate code units are non-printable, so they always escape
//! and the rendered text is always valid UTF-8.
//!
//! The formatter lives with [`crate::PythonString`] so the inherent
//! `PythonString::repr()` method can remain on the type after the primitive
//! moved down to this crate (Rust cannot add inherent methods to a foreign
//! type). Callers keep their old paths through `sk-invocation` re-exports; this
//! module is the single implementation and there is no second Unicode engine.

/// CPython `repr()` for a string, used by operator-facing refusal messages.
///
/// A `&str` can only hold Unicode scalars, so this wrapper is used for text
/// that already decoded strictly; the legacy Op reader passes Python code
/// points (including lone surrogate units) to
/// [`python_repr_codepoints`] directly.
pub fn python_repr(value: &str) -> String {
    let codepoints: Vec<u32> = value.chars().map(u32::from).collect();
    python_repr_codepoints(&codepoints)
}

/// CPython `repr()` for a sequence of Python string code points.
///
/// This is the single `repr` formatting authority in the workspace. Each input
/// unit is a Python code point (`0..=U+10FFFF`, including surrogates). For checked
/// construction use [`crate::PythonString`] and its inherent `repr()` method.
/// See the module documentation for the exact quoting and escape rules.
pub fn python_repr_codepoints(codepoints: &[u32]) -> String {
    let single_quote = u32::from(b'\'');
    let quote = if codepoints.contains(&single_quote) && !codepoints.contains(&u32::from(b'"')) {
        u32::from(b'"')
    } else {
        single_quote
    };
    let mut out = String::with_capacity(codepoints.len() + 2);
    out.push(char::from_u32(quote).expect("the repr quote is ASCII"));
    for &code in codepoints {
        match code {
            0x5c => out.push_str("\\\\"),
            0x09 => out.push_str("\\t"),
            0x0a => out.push_str("\\n"),
            0x0d => out.push_str("\\r"),
            code if code == quote => {
                out.push('\\');
                out.push(char::from_u32(code).expect("the repr quote is ASCII"));
            }
            code if (0x20..0x7f).contains(&code) => {
                out.push(char::from_u32(code).expect("ASCII printable code point"));
            }
            code => push_repr_escape(&mut out, code),
        }
    }
    out.push(char::from_u32(quote).expect("the repr quote is ASCII"));
    out
}

/// One non-ASCII/non-printable `repr` escape (`\xNN`, `\uNNNN`, `\UNNNNNNNN`).
fn push_repr_escape(target: &mut String, code: u32) {
    if crate::python_unicode::is_printable(code)
        && let Some(character) = char::from_u32(code)
    {
        target.push(character);
        return;
    }
    if code < 0x100 {
        target.push_str(&format!("\\x{code:02x}"));
    } else if code < 0x10000 {
        target.push_str(&format!("\\u{code:04x}"));
    } else {
        target.push_str(&format!("\\U{code:08x}"));
    }
}
