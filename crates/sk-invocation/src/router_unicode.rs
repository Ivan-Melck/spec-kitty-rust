//! Python-faithful lowercasing and word splitting for `ActionRouter`.
//!
//! The reference normalizes a request with
//! `re.split(r"[\s\W]+", text.lower())`. Two facts make that non-trivial:
//!
//! * `\w` for a `str` pattern is `str.isalnum() || '_'` -- an open Unicode
//!   class that keeps `_`, keeps non-ASCII letters/digits, and splits
//!   combining marks, emoji, punctuation and lone surrogates.
//! * `str.lower()` is the pinned interpreter's per-code-point mapping plus the
//!   **contextual** `Final_Sigma` rule; it is not `casefold` and not Rust's
//!   `char::to_lowercase()` (whose Unicode version differs from the reference).
//!   `U+0130` expands to two code points, and a capital sigma lowercases to
//!   `U+03C2` or `U+03C3` depending on context.
//!
//! Both fact tables are generated from the pinned CPython 3.11.15 / Unicode
//! 14.0.0 interpreter by `scripts/rust/generate_router_unicode.py` into the
//! private `router_unicode_data` module. Nothing here consults Python at runtime
//! and nothing approximates the classes with ASCII or Rust's own Unicode data.
//!
//! `Final_Sigma` is the standard Unicode rule, and both scans behave the same
//! way: skip case-ignorable code points; a *cased* one decides differently in
//! each direction, and a *non-case-ignorable, non-cased* one ends the scan.
//!
//! * after the sigma: a cased code point means "not final"; reaching the end of
//!   the string, or a non-ignorable code point, satisfies the "not followed by
//!   case-ignorable* cased" clause -- so `"ΑΣ."` stays final (`.` is ignorable)
//!   while `"ΑΣ.Α"` does not;
//! * before the sigma: a cased code point means "final"; a non-ignorable one
//!   means "not final", so `"Α5Σ"` is not final but `"Α:Σ"` is.
//!
//! This was corrected by the live oracle twice: a first model that skipped
//! every non-cased code point in the after-scan, and a second that returned
//! "final" as soon as the after-scan ended. The composed oracle cases pin the
//! difference (`"ΑΣ_Α"` is final, `"ΑΣ.Α"` is not).
//!
//! The context tables partition code points for these scans: decisive cased
//! (`Cased && !Case_Ignorable`), ignorable (including 200 also-Cased points),
//! and neither. They are not two disjoint Unicode-property definitions.
//! U+0345 has both Unicode properties and must be skipped by both scans.
//!
//! This module stays private. The in-crate oracle checks all 1,114,112 code
//! points without exposing a test-only public API.

use crate::router_unicode_data as facts;

/// Capital sigma: the only context-sensitive lower mapping in the reference.
const CAPITAL_SIGMA: u32 = 0x03a3;
/// Small final sigma.
const SMALL_FINAL_SIGMA: u32 = 0x03c2;

/// Whether CPython's `Final_Sigma` context holds at `index`.
///
/// Both clauses are required: "not followed by case-ignorable* cased" and
/// "preceded by cased + case-ignorable*". The generated `is_case_ignorable`
/// class excludes cased code points, so the cased test always comes first.
fn final_sigma_context(codepoints: &[u32], index: usize) -> bool {
    // After-clause: a later cased code point (after any ignorable ones) vetoes
    // final sigma; anything not case-ignorable ends the scan in favour of it.
    for &code in &codepoints[index + 1..] {
        if facts::is_cased(code) {
            return false;
        }
        if !facts::is_case_ignorable(code) {
            break;
        }
    }
    // Before-clause: a cased code point wins; a non-case-ignorable one loses.
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

/// CPython `str.lower()` over an exact code-point sequence.
///
/// Surrogate code units are preserved (they have no case mapping). The result
/// may be longer than the input because of a special-casing expansion.
pub(crate) fn lower_codepoints(codepoints: &[u32]) -> Vec<u32> {
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

/// CPython `\w` for one code point.
pub(crate) fn is_word(code: u32) -> bool {
    facts::is_word(code)
}

/// Split a lowered code-point sequence into maximal `\w` runs.
///
/// This is `re.split(r"[\s\W]+", ...)` without the separator capture: every
/// non-word code point is a separator, including whitespace (whitespace is never
/// a word code point), combining marks, emoji and lone surrogates.
pub(crate) fn split_word_runs(lowered: &[u32]) -> Vec<&[u32]> {
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

    fn lower(text: &str) -> Vec<u32> {
        lower_codepoints(&text.chars().map(u32::from).collect::<Vec<_>>())
    }

    fn lower_text(text: &str) -> String {
        lower(text)
            .into_iter()
            .map(|code| char::from_u32(code).expect("scalar"))
            .collect()
    }

    #[test]
    fn lower_matches_pinned_python_for_contextual_sigma() {
        assert_eq!(lower_text("Σ"), "σ", "no preceding cased code point");
        assert_eq!(lower_text("ΑΣ"), "ας", "final sigma");
        assert_eq!(lower_text("ΑΣΑ"), "ασα", "cased code point follows");
        assert_eq!(lower_text("ΣΣ"), "σς", "after-scan sees the second sigma");
        assert_eq!(lower_text("ΑΣΣ"), "ασς");
        assert_eq!(
            lower_text("ΑΣ5"),
            "ας5",
            "a non-ignorable suffix stays final"
        );
        assert_eq!(
            lower_text("ΑΣ5Α"),
            "ας5α",
            "`5` blocks the `ignorable* cased` suffix"
        );
        assert_eq!(lower_text("ΑΣ_Α"), "ας_α", "`_` is not case-ignorable");
        assert_eq!(
            lower_text("ΑΣ.Α"),
            "ασ.α",
            "`.` is case-ignorable, so the alpha wins"
        );
        assert_eq!(lower_text("ΑΣ Α"), "ας α");
        assert_eq!(
            lower_text("Α5Σ"),
            "α5σ",
            "before-scan stops at a non-ignorable"
        );
        assert_eq!(lower_text("_ΣΑ"), "_σα");
        assert_eq!(lower_text("Α:Σ"), "α:ς", "colon is case-ignorable");
        assert_eq!(
            lower_text("\u{0301}Σ"),
            "\u{0301}σ",
            "no cased code point before"
        );
        assert_eq!(lower_text("ΑΣ\u{0301}Β"), "ασ\u{0301}β");
    }

    #[test]
    fn lower_keeps_expansions_and_surrogates() {
        assert_eq!(
            lower("İ"),
            vec![0x69, 0x307],
            "U+0130 expands to two code points"
        );
        assert_eq!(lower("ǅ"), vec![0x01c6], "titlecase maps to one code point");
        assert_eq!(
            lower_codepoints(&[0xd800, 0x41]),
            vec![0xd800, 0x61],
            "a lone surrogate is preserved verbatim"
        );
        assert_eq!(lower_text("Ⅷ"), "ⅷ");
    }

    #[test]
    fn word_runs_follow_python_word_membership() {
        let split = |text: &str| -> Vec<String> {
            split_word_runs(&lower(text))
                .into_iter()
                .map(|run| {
                    run.iter()
                        .map(|&c| char::from_u32(c).expect("scalar"))
                        .collect()
                })
                .collect()
        };
        assert_eq!(split("review_the_change"), vec!["review_the_change"]);
        assert_eq!(split("fix-bug"), vec!["fix", "bug"]);
        assert_eq!(split("café naïve"), vec!["café", "naïve"]);
        assert_eq!(split("emoji 🚀 deploy"), vec!["emoji", "deploy"]);
        assert_eq!(
            split("İstanbul"),
            vec!["i", "stanbul"],
            "the combining dot splits"
        );
        assert_eq!(split("foo\u{0301}bar"), vec!["foo", "bar"]);
        assert_eq!(split("123 456"), vec!["123", "456"]);
        assert_eq!(split("   "), Vec::<String>::new());
        assert_eq!(split("ἈΣ"), vec!["ἀς"]);
        assert!(is_word(0x5f) && is_word(0x3c9) && is_word(0x31));
        assert!(!is_word(0x2d) && !is_word(0x301) && !is_word(0xd800));
        assert!(!is_word(0x1f600), "an emoji separates tokens");
    }
}
