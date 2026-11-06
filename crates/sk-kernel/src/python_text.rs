//! Shared CPython 3.11.15/UCD14 text primitives.
//!
//! Profiles have half-open roles, so even non-ASCII custom roles need the same
//! full `str.lower()` as invocation routing. This module owns the existing
//! verified algorithms and private generated facts once, below both consumers.
//! No runtime Python, Rust-version Unicode substitution or casefold approximation.

use crate::python_text_data as facts;
use crate::python_text_upper_data as upper_facts;

/// Capital sigma: the only context-sensitive lower mapping in the reference.
const CAPITAL_SIGMA: u32 = 0x03a3;
/// Small final sigma.
const SMALL_FINAL_SIGMA: u32 = 0x03c2;

/// Whether CPython's `Final_Sigma` context holds at `index`.
fn final_sigma_context(codepoints: &[u32], index: usize) -> bool {
    for &code in &codepoints[index + 1..] {
        if facts::is_cased(code) {
            return false;
        }
        if !facts::is_case_ignorable(code) {
            break;
        }
    }
    for &code in codepoints[..index].iter().rev() {
        if facts::is_cased(code) {
            return true;
        }
        if !facts::is_case_ignorable(code) {
            return false;
        }
    }
    false
}

/// CPython `str.lower()` over exact Python code points, including surrogates.
///
/// Input units are `0..=U+10FFFF`, as validated by [`crate::PythonString`]. The
/// contextual Final Sigma rule and U+0130 expansion use the pinned Source facts.
pub fn lower_codepoints(codepoints: &[u32]) -> Vec<u32> {
    let mut lowered = Vec::with_capacity(codepoints.len());
    for (index, &code) in codepoints.iter().enumerate() {
        if code == CAPITAL_SIGMA && final_sigma_context(codepoints, index) {
            lowered.push(SMALL_FINAL_SIGMA);
        } else if let Some(mapped) = facts::lower_single(code) {
            lowered.push(mapped);
        } else if let Some(expansion) = facts::lower_expansion(code) {
            lowered.extend_from_slice(expansion);
        } else {
            lowered.push(code);
        }
    }
    lowered
}

/// CPython `str.upper()` over exact Python code points, including surrogates.
///
/// Input units are `0..=U+10FFFF`, as validated by [`crate::PythonString`].
/// `str.upper()` has no contextual rule, so every unit maps independently; the
/// full mappings of the pinned interpreter include 1:N expansions (`U+00DF` ->
/// `SS`, `U+1F80` -> `U+1F08 U+0399`). Surrogate units are identity in the
/// pinned data and are preserved, never dropped or replaced.
///
/// This is deliberately not Rust's `char::to_uppercase()`: that uses a newer
/// Unicode version, so code points such as `U+0264` and `U+019B` would gain a
/// capital the pinned Source never produces.
pub fn upper_codepoints(codepoints: &[u32]) -> Vec<u32> {
    let mut uppered = Vec::with_capacity(codepoints.len());
    for &code in codepoints {
        if let Some(mapped) = upper_facts::upper_single(code) {
            uppered.push(mapped);
        } else if let Some(expansion) = upper_facts::upper_expansion(code) {
            uppered.extend_from_slice(expansion);
        } else {
            uppered.push(code);
        }
    }
    uppered
}

/// TextIOWrapper's `newline=None` translation, without changing other Unicode
/// line separators. The common no-CR case borrows the original scalar text.
pub fn universal_newlines(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains('\r') {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut output = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\r' {
            output.push('\n');
            if characters.peek() == Some(&'\n') {
                characters.next();
            }
        } else {
            output.push(character);
        }
    }
    std::borrow::Cow::Owned(output)
}

/// CPython `str.splitlines(keepends=False)` over scalar text, borrowing each
/// line. CRLF is one boundary; a final boundary does not add an empty line.
/// This differs from both Rust `lines()` and splitting only on LF.
pub fn splitlines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut characters = text.char_indices().peekable();
    while let Some((index, character)) = characters.next() {
        if is_line_break(u32::from(character)) {
            lines.push(&text[start..index]);
            start = index + character.len_utf8();
            if character == '\r' && characters.peek().is_some_and(|(_, next)| *next == '\n') {
                characters.next();
                start += 1;
            }
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// The ONE CPython str.splitlines boundary roster (distinct from whitespace
/// and from TextIOWrapper universal newlines). CRLF is combined by the views.
pub fn is_line_break(code: u32) -> bool {
    matches!(code, 0x0a..=0x0d | 0x1c..=0x1e | 0x85 | 0x2028 | 0x2029)
}

/// Exact borrowed str.splitlines(keepends=False) view over validated Python
/// code points, including lone surrogates. A final boundary adds no empty line.
pub fn splitlines_codepoints(units: &[u32]) -> Vec<&[u32]> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while index < units.len() {
        if is_line_break(units[index]) {
            lines.push(&units[start..index]);
            index += if units[index] == 0x0d && units.get(index + 1) == Some(&0x0a) {
                2
            } else {
                1
            };
            start = index;
        } else {
            index += 1;
        }
    }
    if start < units.len() {
        lines.push(&units[start..]);
    }
    lines
}

/// CPython 3.11.15/UCD14 `chr(code).isspace()` membership.
///
/// These are the pinned Source's 29 code points, including U+001C..U+001F
/// (which Rust's Unicode White_Space property omits). Surrogates are not space.
pub fn is_whitespace(code: u32) -> bool {
    matches!(code,
        0x0009..=0x000d | 0x001c..=0x0020 | 0x0085 | 0x00a0 | 0x1680 |
        0x2000..=0x200a | 0x2028..=0x2029 | 0x202f | 0x205f | 0x3000)
}

/// CPython/UCD14 decimal value for one code point (`Nd`), never `isnumeric`.
pub fn decimal_value(code: u32) -> Option<u8> {
    let ranges = crate::python_decimal_data::DECIMAL_RANGES;
    let index = ranges
        .partition_point(|&(start, _)| start <= code)
        .checked_sub(1)?;
    let (start, end) = ranges[index];
    (code <= end).then_some(((code - start) % 10) as u8)
}

/// CPython `chr(code).isdecimal()` for a valid Python code point.
pub fn is_decimal(code: u32) -> bool {
    decimal_value(code).is_some()
}

/// CPython/UCD14 `chr(code).isdigit()`, distinct from decimal and numeric.
///
/// Reuses the one Nd authority and the pinned nondecimal digit facts.
pub fn is_digit(code: u32) -> bool {
    is_decimal(code)
        || crate::python_digit_data::EXTRA_DIGIT_RANGES
            .iter()
            .any(|(start, end)| (*start..=*end).contains(&code))
}

/// CPython `str.isdecimal()` over exact code points, including the empty rule.
pub fn is_decimal_codepoints(codepoints: &[u32]) -> bool {
    !codepoints.is_empty() && codepoints.iter().copied().all(is_decimal)
}

/// CPython Unicode-regex `\w` membership (`str.isalnum() || '_'`).
pub fn is_word(code: u32) -> bool {
    facts::is_word(code)
}

/// Equivalence key for one Unicode literal under CPython `re.IGNORECASE`.
///
/// This is not contextual `str.lower()` or full casefolding, and is not a
/// general regex engine. It reuses the one pinned lowercase authority's first
/// single-character unit, plus the reference regex compiler's extra classes.
/// Word boundaries must still be evaluated on the original input code points.
pub fn regex_ignore_case_key(code: u32) -> u32 {
    let lowered = facts::lower_single(code)
        .or_else(|| facts::lower_expansion(code).and_then(|units| units.first().copied()))
        .unwrap_or(code);
    let classes = crate::python_regex_case_data::EXTRA_CASE_KEYS;
    match classes.binary_search_by_key(&lowered, |&(member, _)| member) {
        Ok(index) => classes[index].1,
        Err(_) => lowered,
    }
}

/// Split a lowered sequence into maximal CPython `\w` runs.
pub fn split_word_runs(lowered: &[u32]) -> Vec<&[u32]> {
    let mut runs = Vec::new();
    let mut start = 0usize;
    let mut in_run = false;
    for (index, &code) in lowered.iter().enumerate() {
        if is_word(code) {
            if !in_run {
                start = index;
                in_run = true;
            }
        } else if in_run {
            runs.push(&lowered[start..index]);
            in_run = false;
        }
    }
    if in_run {
        runs.push(&lowered[start..]);
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upper_expands_like_the_pinned_interpreter() {
        assert_eq!(upper_codepoints(&[0x00df]), vec![0x0053, 0x0053], "sharp s");
        assert_eq!(
            upper_codepoints(&[0xfb01]),
            vec![0x0046, 0x0049],
            "fi ligature"
        );
        assert_eq!(
            upper_codepoints(&[0x1f80]),
            vec![0x1f08, 0x0399],
            "Greek dasia"
        );
        assert_eq!(
            upper_codepoints(&[0x01f0]),
            vec![0x004a, 0x030c],
            "j with caron"
        );
        assert_eq!(
            upper_codepoints(&[0x0390]),
            vec![0x0399, 0x0308, 0x0301],
            "Greek dialytika tonos"
        );
    }

    #[test]
    fn upper_is_identity_where_the_pinned_ucd_has_no_capital() {
        // Rust's own char::to_uppercase() returns U+A7CB / U+2C6B for these two
        // (later Unicode versions). The pinned Source does not.
        assert_eq!(upper_codepoints(&[0x0264]), vec![0x0264], "rams horn");
        assert_eq!(upper_codepoints(&[0x019b]), vec![0x019b], "l with bar");
        assert_eq!(upper_codepoints(&[0x0130]), vec![0x0130], "dotted I");
        assert_eq!(upper_codepoints(&[0x0378]), vec![0x0378], "unassigned");
        assert_eq!(upper_codepoints(&[0x0000]), vec![0x0000], "NUL");
        assert_eq!(upper_codepoints(&[0xe000]), vec![0xe000], "private use");
        assert_eq!(upper_codepoints(&[]), Vec::<u32>::new(), "empty string");
    }

    #[test]
    fn upper_preserves_all_surrogate_units() {
        for code in 0xd800..=0xdfff {
            assert_eq!(upper_codepoints(&[code]), vec![code], "U+{code:04X}");
        }
        let mixed = [0x0061, 0xd800, 0x00df, 0x002d, 0x0062];
        assert_eq!(
            upper_codepoints(&mixed),
            vec![0x0041, 0xd800, 0x0053, 0x0053, 0x002d, 0x0042],
            "surrogates mixed with expansions keep their exact position"
        );
        // Adjacent surrogate units stay two units: nothing pairs or normalizes.
        assert_eq!(upper_codepoints(&[0xd83d, 0xde00]), vec![0xd83d, 0xde00]);
    }

    #[test]
    fn upper_covers_every_code_point_with_valid_units() {
        for code in 0..0x110000u32 {
            let mapped = upper_codepoints(&[code]);
            assert!(!mapped.is_empty(), "U+{code:04X} maps to nothing");
            assert!(
                mapped.iter().all(|&item| item <= 0x10ffff),
                "U+{code:04X} leaves the Python code-point range"
            );
            if mapped == [code] {
                continue;
            }
            assert!(
                crate::python_text_upper_data::upper_single(code).is_some()
                    || crate::python_text_upper_data::upper_expansion(code).is_some(),
                "U+{code:04X} maps without a recorded fact"
            );
        }
    }

    #[test]
    fn python_string_upper_keeps_exact_units() {
        let text =
            crate::PythonString::from_codepoints(&[0x00df, 0x002d, 0xd800, 0x0031, 0x0264, 0x0061])
                .expect("valid units");
        let uppered = text.upper();
        assert_eq!(
            uppered.codepoints(),
            &[0x0053, 0x0053, 0x002d, 0xd800, 0x0031, 0x0264, 0x0041]
        );
        assert_eq!(
            crate::PythonString::from_scalars("use-c4-model-techniques")
                .upper()
                .to_scalar_string()
                .expect("scalar uppercase"),
            "USE-C4-MODEL-TECHNIQUES"
        );
    }
}
