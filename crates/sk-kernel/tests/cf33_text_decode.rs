//! CF33 — the one Kernel provable text decoder, checked against the pinned
//! Source's own observations.
//!
//! Every row below was recorded by running the reference's
//! `kernel.text_decode.decode_unambiguous` / `detect_bom` /
//! `normalize_newlines` under the lock-exact oracle (Python 3.11.15, 53/53
//! requirements verified). This is a Source-observed table, not a paired
//! subprocess differential: the Native assertions run here alone, and no claim
//! of full Unicode/codec parity follows from them.

use sk_kernel::text_decode::{Bom, decode_unambiguous, detect_bom, normalize_newlines};

fn decoded(data: &[u8]) -> Option<(String, &'static str)> {
    decode_unambiguous(data)
}

fn bom(data: &[u8]) -> Option<(&'static str, &'static str)> {
    detect_bom(data).map(|bom: Bom| (bom.codec(), bom.source_encoding()))
}

#[test]
fn utf32_boms_are_recognised_before_utf16() {
    // `FF FE 00 00` is the UTF-32-LE BOM and also a UTF-16-LE BOM plus a NUL
    // unit; the reference checks UTF-32 first. Reversing the order is the
    // historical mis-decode this module fixes.
    assert_eq!(bom(b"\xff\xfe\x00\x00"), Some(("utf-32", "utf-32-le")));
    assert_eq!(bom(b"\x00\x00\xfe\xff"), Some(("utf-32", "utf-32-be")));
    assert_eq!(bom(b"\xff\xfe"), Some(("utf-16", "utf-16-le")));
    assert_eq!(bom(b"\xfe\xff"), Some(("utf-16", "utf-16-be")));
    assert_eq!(bom(b"\xef\xbb\xbf"), Some(("utf-8-sig", "utf-8-sig")));
    assert_eq!(bom(b""), None);
    assert_eq!(bom(b"plain"), None);
    assert_eq!(bom(b"\xff"), None, "a lone FF is not a BOM");
    assert_eq!(bom(b"\xef\xbb"), None, "a truncated UTF-8 BOM is not a BOM");
}

#[test]
fn provable_decodes_keep_text_and_report_the_source_encoding() {
    let cases: &[(&[u8], &str, &str)] = &[
        (b"", "", "utf-8"),
        (b"hello", "hello", "utf-8"),
        (b"\xef\xbb\xbfhello", "hello", "utf-8-sig"),
        (b"\xff\xfeh\x00e\x00l\x00l\x00o\x00", "hello", "utf-16-le"),
        (b"\xfe\xff\x00h\x00e\x00l\x00l\x00o", "hello", "utf-16-be"),
        (
            b"\xff\xfe\x00\x00h\x00\x00\x00i\x00\x00\x00",
            "hi",
            "utf-32-le",
        ),
        (
            b"\x00\x00\xfe\xff\x00\x00\x00h\x00\x00\x00i",
            "hi",
            "utf-32-be",
        ),
        // A UTF-8 BOM followed by a NUL unit must decode as UTF-32-LE, not as
        // UTF-16-LE with an embedded NUL.
        (
            b"\xff\xfe\x00\x00a\x00\x00\x00\x00\x00\x00\x00",
            "a\u{0}",
            "utf-32-le",
        ),
    ];
    for (data, text, encoding) in cases {
        assert_eq!(
            decoded(data),
            Some(((*text).to_owned(), *encoding)),
            "{data:?}"
        );
    }
}

#[test]
fn bom_only_inputs_decode_to_empty_text() {
    for (data, encoding) in [
        (&b"\xef\xbb\xbf"[..], "utf-8-sig"),
        (b"\xff\xfe", "utf-16-le"),
        (b"\xfe\xff", "utf-16-be"),
        (b"\xff\xfe\x00\x00", "utf-32-le"),
        (b"\x00\x00\xfe\xff", "utf-32-be"),
    ] {
        assert_eq!(decoded(data), Some((String::new(), encoding)), "{data:?}");
    }
}

#[test]
fn unprovable_bytes_are_refused_rather_than_guessed() {
    for data in [
        // cp1252 (the classic single-byte code page) has no BOM and is not UTF-8.
        &b"caf\xe9"[..],
        // Invalid UTF-8 continuation / start byte.
        b"\xff",
        b"\xd8\x00",
        // A BOM that proves a codec, then a payload that codec rejects: the
        // reference returns None instead of falling through to strict UTF-8.
        b"\xef\xbb\xbf\xff",
        // Truncated UTF-16 unit.
        b"\xff\xfeh\x00i",
        // Unpaired UTF-16 surrogates (high, low, big-endian high).
        b"\xff\xfe\x00\xd8",
        b"\xff\xfe\x00\xdc",
        b"\xfe\xff\xd8\x00",
        // Truncated UTF-32 unit.
        b"\xff\xfe\x00\x00a\x00\x00",
        // UTF-32 surrogate code points (both endiannesses).
        b"\xff\xfe\x00\x00\x00\xd8\x00\x00",
        b"\x00\x00\xfe\xff\x00\x00\xd8\x00",
        // UTF-32 above U+10FFFF.
        b"\x00\x00\xfe\xff\x00\x11\x00\x00",
        b"\xff\xfe\x00\x00\x00\x00\x11\x00",
    ] {
        assert_eq!(decoded(data), None, "{data:?}");
    }
}

#[test]
fn exact_code_points_survive_including_nul_noncharacters_and_bom_after_bom() {
    // U+FFFF and U+FFFE are valid UTF-8 (noncharacters, not decode errors).
    assert_eq!(
        decoded(b"\xef\xbf\xbf"),
        Some(("\u{ffff}".to_owned(), "utf-8"))
    );
    assert_eq!(
        decoded(b"\xef\xbf\xbe"),
        Some(("\u{fffe}".to_owned(), "utf-8"))
    );
    // NUL is preserved, never used as a string terminator.
    assert_eq!(decoded(b"a\x00b"), Some(("a\u{0}b".to_owned(), "utf-8")));
    // Four zero bytes are valid UTF-8 NULs, not a UTF-32 BOM.
    assert_eq!(
        decoded(b"\x00\x00\x00\x00"),
        Some(("\u{0}\u{0}\u{0}\u{0}".to_owned(), "utf-8"))
    );
    // A second U+FEFF authored after the BOM is data, not the BOM.
    assert_eq!(
        decoded(b"\xef\xbb\xbf\xef\xbb\xbfx"),
        Some(("\u{feff}x".to_owned(), "utf-8-sig"))
    );
    // Surrogate code units are not decodable, so no `PythonString` surrogate
    // can enter through this decoder.
    assert_eq!(decoded(b"\xed\xa0\x80"), None, "UTF-8 surrogate");
    assert_eq!(
        decoded(b"\xff\xfe\x00\xd8\x00\xdc"),
        Some(("\u{10000}".to_owned(), "utf-16-le")),
        "a real surrogate pair is one astral scalar"
    );
}

#[test]
fn newlines_are_normalised_separately_and_never_touch_a_bom() {
    assert_eq!(normalize_newlines("a\r\nb\rc\nd"), "a\nb\nc\nd");
    assert_eq!(normalize_newlines("\r\r\n"), "\n\n");
    assert_eq!(normalize_newlines("\r\n"), "\n");
    assert_eq!(
        normalize_newlines("a\nb"),
        "a\nb",
        "LF-only text is unchanged"
    );
    assert_eq!(
        normalize_newlines("\u{feff}a\r\n"),
        "\u{feff}a\n",
        "a BOM is not stripped by newline normalisation"
    );
    // Decoding itself never translates newlines: that is a TextIO phase.
    assert_eq!(
        decoded(b"a\r\nb\rc\nd"),
        Some(("a\r\nb\rc\nd".to_owned(), "utf-8"))
    );
    assert_eq!(
        decoded(b"\xff\xfea\x00\r\x00\n\x00b\x00"),
        Some(("a\r\nb".to_owned(), "utf-16-le"))
    );
    assert_eq!(
        decoded(b"\xff\xfe\x00\x00a\x00\x00\x00\r\x00\x00\x00\n\x00\x00\x00"),
        Some(("a\r\n".to_owned(), "utf-32-le"))
    );
}
