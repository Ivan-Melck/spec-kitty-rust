//! Source-parity matrix for the one strict UTF-8 encoder.
//!
//! Every expectation is the pinned reference's actual `str.encode("utf-8")`
//! outcome (W1 encoder probes: `/tmp/sk-w1-enc.xZVbER/out.json` for the first
//! set and the range probe for the consecutive-unit cases). CPython refuses the
//! **maximal run of consecutive surrogate units**, so the reported `start`/`end`
//! and the singular/plural message form both have to match — not just the first
//! offending unit.

#![cfg(test)]

use sk_kernel::PythonString;
use sk_kernel::python_utf8::{PythonUtf8EncodeError, encode_python_string, encode_utf8_scalars};

fn python(units: &[u32]) -> PythonString {
    PythonString::from_codepoints(units).expect("valid code units")
}

fn failure(units: &[u32]) -> PythonUtf8EncodeError {
    encode_python_string(&python(units)).expect_err("surrogates have no UTF-8 form")
}

#[test]
fn every_encodable_code_unit_matches_the_reference_bytes() {
    for (units, expected_hex) in [
        (vec![0x61, 0x62, 0x63], "616263"),
        (vec![0x61, 0x00, 0x62], "610062"),
        (vec![0x68, 0xe9, 0x6c, 0x6c, 0x6f], "68c3a96c6c6f"),
        (vec![0x61, 0x1f600, 0x62], "61f09f988062"),
        (vec![0xfeff, 0x78], "efbbbf78"),
        (Vec::new(), ""),
    ] {
        let encoded = encode_python_string(&python(&units)).expect("encodable");
        let hex: String = encoded.iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(hex, expected_hex, "units {units:?}");
    }
    // The Rust-text path goes through the same encoder and cannot refuse.
    assert_eq!(encode_utf8_scalars("héllo ☃"), "héllo ☃".as_bytes());
}

#[test]
fn a_lone_surrogate_reports_cpythons_singular_message() {
    for (units, start, end, character) in [
        (vec![0xd800], 0usize, 1usize, "\\ud800"),
        (vec![0x78, 0xd800, 0x79], 1, 2, "\\ud800"),
        (vec![0x78, 0x79, 0xd800], 2, 3, "\\ud800"),
        (vec![0xdcff], 0, 1, "\\udcff"),
        (vec![0x1f600, 0xd800], 1, 2, "\\ud800"),
        (vec![0xd800, 0x61, 0xd800], 0, 1, "\\ud800"),
    ] {
        let error = failure(&units);
        assert_eq!(error.start, start, "units {units:?}");
        assert_eq!(error.end, end, "units {units:?}");
        assert_eq!(error.reason, "surrogates not allowed");
        assert_eq!(error.encoding(), "utf-8");
        assert_eq!(error.object(), units.as_slice());
        assert_eq!(
            error.to_string(),
            format!(
                "'utf-8' codec can't encode character '{character}' in position {start}: \
                 surrogates not allowed"
            ),
            "units {units:?}"
        );
    }
}

#[test]
fn consecutive_surrogates_report_cpythons_plural_range() {
    for (units, start, end) in [
        (vec![0xd800, 0xd800], 0usize, 2usize),
        (vec![0xd800, 0xd800, 0xd800], 0, 3),
        (vec![0xd83d, 0xde00], 0, 2), // paired units are still two surrogates
        (vec![0xd800, 0xdc00], 0, 2),
        (vec![0x78, 0xd800, 0xd801], 1, 3),
    ] {
        let error = failure(&units);
        assert_eq!(error.start, start, "units {units:?}");
        assert_eq!(error.end, end, "units {units:?}");
        assert_eq!(
            error.to_string(),
            format!(
                "'utf-8' codec can't encode characters in position {start}-{}: \
                 surrogates not allowed",
                end - 1
            ),
            "units {units:?}"
        );
    }
}

#[test]
fn a_long_prefix_keeps_the_exact_position() {
    let mut units = vec![b'a' as u32; 20_000];
    units.push(0xd800);
    let error = failure(&units);
    assert_eq!(error.start, 20_000);
    assert_eq!(error.end, 20_001);
    assert!(
        error
            .to_string()
            .ends_with("in position 20000: surrogates not allowed")
    );
    assert_eq!(error.object().len(), 20_001);
}

#[test]
fn the_encoder_never_pairs_surrogate_units() {
    // A high+low pair is not a scalar here: both units are refused, exactly as the
    // reference refuses them; nothing is silently combined or replaced.
    let error = failure(&[0xd83d, 0xde00]);
    assert_eq!((error.start, error.end), (0, 2));
    assert_eq!(error.object(), &[0xd83d, 0xde00]);
}
