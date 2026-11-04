//! Contracts for the canonical `sk-kernel` Python-string and `repr` primitive.
//!
//! These are the exact type/API contracts of the relocated authority: the
//! existing full-table CPython oracle and JSON-pair regressions stay in their
//! original unchanged test files, so this file does not duplicate the
//! 1,112,064-scalar sweep.

use sk_kernel::{InvalidCodepoint, PythonString, python_repr, python_repr_codepoints};

fn text(codepoints: &[u32]) -> PythonString {
    PythonString::from_codepoints(codepoints).expect("fixture code points are valid")
}

#[test]
fn construction_accepts_every_python_code_unit_including_nul_and_surrogates() {
    for code in [
        0x0000, 0x0041, 0x00e9, 0xd800, 0xdc80, 0xdfff, 0x10000, 0x1f600, 0x10ffff,
    ] {
        let string = text(&[code]);
        assert_eq!(string.codepoints(), &[code]);
        assert!(!string.is_empty());
    }
    assert!(PythonString::default().is_empty());
    assert_eq!(PythonString::default().codepoints(), &[] as &[u32]);
}

#[test]
fn invalid_code_units_are_refused_with_the_first_offending_value() {
    let error: InvalidCodepoint =
        PythonString::from_codepoints(&[0x41, 0x110000, 0x41]).expect_err("out of range");
    assert_eq!(error.code(), 0x110000);
    assert_eq!(
        error.to_string(),
        "U+110000 is not a Python string code point (maximum U+10FFFF)"
    );
    let error = PythonString::from_units(vec![0x41, 0x110001]).expect_err("out of range");
    assert_eq!(error.code(), 0x110001);

    let mut string = PythonString::from_scalars("ab");
    let error = string.push_codepoint(0x110000).expect_err("out of range");
    assert_eq!(error.code(), 0x110000);
    assert_eq!(string.codepoints(), &[0x61, 0x62], "no partial append");
}

#[test]
fn owned_and_append_seams_preserve_exact_units() {
    let owned = PythonString::from_units(vec![0x61, 0xd800, 0x1f600]).expect("valid units");
    assert_eq!(owned.codepoints(), &[0x61, 0xd800, 0x1f600]);

    let mut composed = PythonString::from_scalars("a");
    composed.push_scalars("é𝄞");
    composed
        .push_codepoint(0xd83d)
        .expect("high surrogate is a code unit");
    composed
        .push_codepoint(0xde00)
        .expect("low surrogate is a code unit");
    composed.append(&owned);
    assert_eq!(
        composed.codepoints(),
        &[0x61, 0xe9, 0x1d11e, 0xd83d, 0xde00, 0x61, 0xd800, 0x1f600]
    );
}

#[test]
fn scalar_projection_names_the_first_surrogate_and_keeps_astral_text() {
    assert_eq!(
        text(&[0x41, 0xd800, 0xdc80]).to_scalar_string(),
        Err(0xd800)
    );
    assert_eq!(text(&[0x41, 0xdc80]).to_scalar_string(), Err(0xdc80));
    assert_eq!(text(&[0x41, 0x1f600]).to_scalar_string().unwrap(), "A😀");
}

#[test]
fn derived_traits_keep_their_old_shape() {
    let a = text(&[0x41]);
    let b = text(&[0x42]);
    let a_nul = text(&[0x41, 0x00]);
    assert_eq!(a, text(&[0x41]));
    assert_ne!(a, b);
    assert!(a < b);
    assert!(a < a_nul, "lexicographic code-point ordering");
    assert_eq!(format!("{a:?}"), "PythonString([65])");
    assert_eq!(
        format!(
            "{:?}",
            PythonString::from_codepoints(&[0x110000]).expect_err("out of range")
        ),
        "InvalidCodepoint { code: 1114112 }"
    );
    assert_eq!(text(&[0x41, 0xd800]).to_string(), "A\\ud800");
    assert_eq!(text(&[0x00]).to_string(), "\u{0}");
}

#[test]
fn repr_matches_cpython_quoting_and_escape_categories() {
    assert_eq!(python_repr("main"), "'main'");
    assert_eq!(python_repr("a'b"), "\"a'b\"");
    assert_eq!(python_repr("a\"b"), "'a\"b'");
    assert_eq!(python_repr("a'b\"c"), "'a\\'b\"c'");
    assert_eq!(python_repr("back\\slash"), "'back\\\\slash'");
    assert_eq!(python_repr("tab\there"), "'tab\\there'");
    assert_eq!(python_repr("\t\n\r"), "'\\t\\n\\r'");
    assert_eq!(python_repr("\u{0}\u{1f}"), "'\\x00\\x1f'");
    assert_eq!(python_repr("\u{7f}"), "'\\x7f'");
    assert_eq!(python_repr("\u{a0}"), "'\\xa0'");
    assert_eq!(python_repr("\u{200b}"), "'\\u200b'");
    assert_eq!(python_repr("\u{2029}"), "'\\u2029'");
    assert_eq!(python_repr("\u{1d173}"), "'\\U0001d173'");
    assert_eq!(python_repr("\u{10ffff}"), "'\\U0010ffff'");
    assert_eq!(python_repr("é α ☃ 😀 \u{10000}"), "'é α ☃ 😀 \u{10000}'");
    assert_eq!(python_repr(" "), "' '");
    assert_eq!(python_repr_codepoints(&[0x27, 0x22]), "'\\'\"'");
    assert_eq!(python_repr_codepoints(&[0x27]), "\"'\"");
}

#[test]
fn every_surrogate_code_unit_roundtrips_and_escapes_without_pairing() {
    for code in 0xd800..=0xdfff {
        let string = text(&[code]);
        assert_eq!(string.codepoints(), &[code]);
        assert_eq!(string.to_scalar_string(), Err(code));
        assert_eq!(string.repr(), format!("'\\u{code:04x}'"));
        assert_eq!(format!("{string:?}"), format!("PythonString([{code}])"));
    }

    // The primitive never pairs surrogate units: pairing is the legacy JSON
    // parser's rule (covered unchanged in `sk-invocation`).
    let pair = text(&[0xd83d, 0xde00]);
    assert_eq!(pair.codepoints(), &[0xd83d, 0xde00]);
    assert_eq!(pair.repr(), "'\\ud83d\\ude00'");
    assert_ne!(pair, text(&[0x1f600]));
    assert_eq!(text(&[0x1f600]).repr(), "'😀'");
    assert_eq!(pair.to_scalar_string(), Err(0xd83d));
}
