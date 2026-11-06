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
//! 14.0.0 interpreter by `scripts/rust/generate_router_unicode.py` into Kernel's
//! private `python_text_data` module. The shared algorithms live in Kernel too. Nothing here consults Python at runtime
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

#[cfg(test)]
pub(crate) use sk_kernel::python_text::is_word;
pub(crate) use sk_kernel::python_text::{lower_codepoints, split_word_runs};

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
