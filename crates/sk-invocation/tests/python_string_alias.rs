//! Compatibility contracts for the relocated Python-string authority.
//!
//! `sk_invocation::{PythonString, InvalidCodepoint, python_repr}` must be the
//! SAME items as `sk_kernel`'s canonical primitive — re-exports, not wrappers,
//! conversions, bridges or a second nominal type. The exhaustive CPython
//! scalar-repr oracle and the JSON-pair parser regressions stay in their
//! original unchanged test files; this file proves the public identity and the
//! preserved callable surface without duplicating the 1,112,064-scalar sweep.

use std::any::TypeId;

use sk_invocation::{InvalidCodepoint, PythonString, python_repr};

#[test]
fn public_types_are_the_same_type_not_wrappers_or_bridges() {
    assert_eq!(
        TypeId::of::<sk_kernel::PythonString>(),
        TypeId::of::<sk_invocation::PythonString>()
    );
    assert_eq!(
        TypeId::of::<sk_kernel::InvalidCodepoint>(),
        TypeId::of::<sk_invocation::InvalidCodepoint>()
    );

    // Both directions must assign without conversion (a wrapper or a distinct
    // kernel type would be a type error here).
    fn takes_kernel(_: sk_kernel::PythonString) {}
    fn takes_invocation(_: sk_invocation::PythonString) {}
    let alias_value: sk_invocation::PythonString = sk_kernel::PythonString::from_scalars("alias");
    takes_kernel(alias_value.clone());
    let kernel_value: sk_kernel::PythonString = sk_invocation::PythonString::from_scalars("kernel");
    takes_invocation(kernel_value);

    // The public constructor and error identity are the same items too.
    let kernel_ctor: fn(&[u32]) -> Result<sk_kernel::PythonString, sk_kernel::InvalidCodepoint> =
        sk_kernel::PythonString::from_codepoints;
    let alias_ctor: fn(&[u32]) -> Result<sk_invocation::PythonString, sk_kernel::InvalidCodepoint> =
        sk_invocation::PythonString::from_codepoints;
    assert_eq!(kernel_ctor(&[0x41]).unwrap(), alias_ctor(&[0x41]).unwrap());
}

#[test]
fn old_public_constructors_getters_and_inherent_repr_remain_callable() {
    let string = PythonString::from_codepoints(&[0x61, 0x00, 0x1f600]).expect("valid codepoints");
    assert_eq!(string.codepoints(), &[0x61, 0x00, 0x1f600]);
    assert!(!string.is_empty());
    assert_eq!(string.to_scalar_string().unwrap(), "a\0😀");
    assert_eq!(string, PythonString::from_scalars("a\0😀"));
    assert_eq!(string.to_string(), "a\0😀");
    assert_eq!(string.repr(), "'a\\x00😀'");
    assert_eq!(python_repr("a\0😀"), "'a\\x00😀'");

    let owned = PythonString::from_units(vec![0x61, 0xd800]).expect("valid units");
    assert_eq!(owned.codepoints(), &[0x61, 0xd800]);
    assert_eq!(owned.to_scalar_string(), Err(0xd800));
}

#[test]
fn invalid_codepoint_alias_keeps_the_same_error_and_display() {
    let error: InvalidCodepoint =
        PythonString::from_codepoints(&[0x41, 0x110000]).expect_err("out of range");
    assert_eq!(error.code(), 0x110000);
    assert_eq!(
        error.to_string(),
        "U+110000 is not a Python string code point (maximum U+10FFFF)"
    );
    fn assert_std_error<E: std::error::Error>(_: &E) {}
    assert_std_error(&error);

    let mut string = PythonString::from_scalars("a");
    assert_eq!(string.push_codepoint(0x110000).err(), Some(error));
    assert_eq!(string.codepoints(), &[0x61], "no partial append");
}

#[test]
fn extra_append_seams_validate_ranges_and_preserve_units() {
    let mut composed = PythonString::from_scalars("op(");
    composed.push_scalars("): ");
    composed
        .push_codepoint(0x1f600)
        .expect("astral is a code unit");
    composed
        .push_codepoint(0xd83d)
        .expect("high surrogate is a code unit");
    composed
        .push_codepoint(0xde00)
        .expect("low surrogate is a code unit");
    let suffix = PythonString::from_units(vec![0x5d, 0x00]).expect("valid units");
    composed.append(&suffix);
    assert_eq!(
        composed.codepoints(),
        &[
            0x6f, 0x70, 0x28, 0x29, 0x3a, 0x20, 0x1f600, 0xd83d, 0xde00, 0x5d, 0x00
        ]
    );
    assert_eq!(composed.to_scalar_string(), Err(0xd83d));
    assert!(PythonString::from_units(vec![0x110000]).is_err());
}

#[test]
fn all_surrogates_roundtrip_through_the_alias_without_pairing() {
    for code in 0xd800..=0xdfff {
        let string = PythonString::from_codepoints(&[code]).expect("surrogate is a code unit");
        assert_eq!(string.codepoints(), &[code]);
        assert_eq!(string.repr(), format!("'\\u{code:04x}'"));
        assert_eq!(string.to_scalar_string(), Err(code));
        assert_eq!(format!("{string}"), format!("\\u{code:04x}"));
    }

    // The primitive keeps the two units; surrogate *pairing* is the legacy JSON
    // parser's rule and is covered by its unchanged in-crate pair regressions.
    let pair = PythonString::from_codepoints(&[0xd83d, 0xde00]).expect("pair units");
    assert_eq!(pair.codepoints(), &[0xd83d, 0xde00]);
    assert_eq!(pair.repr(), "'\\ud83d\\ude00'");
    assert_eq!(pair.to_scalar_string(), Err(0xd83d));
    assert_ne!(pair, PythonString::from_scalars("😀"));
    assert_eq!(PythonString::from_scalars("😀").repr(), "'😀'");
}

#[test]
fn nul_ordering_equality_display_and_default_match_the_old_surface() {
    let nul = PythonString::from_codepoints(&[0x00]).expect("NUL is a code unit");
    assert_eq!(nul.repr(), "'\\x00'");
    assert_eq!(nul.to_scalar_string().unwrap(), "\0");
    assert_eq!(nul.to_string(), "\0");
    assert!(PythonString::default().is_empty());
    assert!(nul < PythonString::from_codepoints(&[0x41]).unwrap());
    assert_eq!(
        PythonString::from_codepoints(&[0x41, 0x00])
            .unwrap()
            .codepoints(),
        &[0x41, 0x00]
    );
    assert_eq!(
        format!("{:?}", PythonString::from_scalars("A")),
        "PythonString([65])"
    );
}
