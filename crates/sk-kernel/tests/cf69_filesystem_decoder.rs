//! CF69 — Source filesystem spelling through the existing Kernel provenance API.
//!
//! The first control is deliberately written against the pre-CF69 public API:
//! it uses only `EncodingProvenanceJournal::record`, so running it under an
//! actual installed Latin-1 startup locale reproduces the pre-fix
//! `U+DCE9` (hardcoded UTF-8 surrogateescape) spelling where Source
//! `os.fsdecode` yields `U+00E9`. The matching default-UTF-8 control is a
//! regression, not a pre-fix red.
//!
//! These tests select their fixture by the requested startup environment and
//! skip when that environment is not active; the explicit runs are recorded in
//! the lane report with their exact environment.
//!
//! Not a whole-command or cross-platform parity claim.

#![cfg(unix)]

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use sk_kernel::clock::FrozenClock;
use sk_kernel::encoding_provenance::{
    EncodingProvenanceError, EncodingProvenanceJournal, EventIdSource, ReadEncodingFacts,
};
use sk_kernel::python_os_encoding::OsEncodingContext;

const FIXED_EVENT_ID: &str = "01M6QNZ0JDENFRX8NS45HBGPD0";

struct FixedIds;

impl EventIdSource for FixedIds {
    fn next_event_id(&self, _millis: i64) -> Result<String, EncodingProvenanceError> {
        Ok(FIXED_EVENT_ID.to_owned())
    }
}

fn journal(root: &Path) -> EncodingProvenanceJournal {
    let instant = chrono::DateTime::parse_from_rfc3339("2026-11-06T05:40:34.509035+00:00")
        .expect("fixed instant")
        .with_timezone(&chrono::Utc);
    EncodingProvenanceJournal::with_sources(
        root,
        Box::new(FrozenClock::new(instant)),
        Box::new(FixedIds),
    )
}

fn raw_path(root: &Path, bytes: &[u8]) -> PathBuf {
    root.join(OsStr::from_bytes(bytes))
}

fn facts() -> ReadEncodingFacts<'static> {
    ReadEncodingFacts {
        source_encoding: "utf-8",
        confidence: 1.0,
        normalization_applied: false,
        bypass_used: false,
    }
}

fn lookup(
    pairs: &'static [(&'static str, &'static str)],
) -> impl Fn(&str) -> Option<std::ffi::OsString> {
    move |name| {
        pairs
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| std::ffi::OsString::from((*value).to_owned()))
    }
}

#[test]
fn captured_lookup_uses_actual_installed_locale_data() {
    use sk_kernel::os_locale::{LocaleFact, resolve};

    let latin1_lookup = lookup(&[
        ("LC_ALL", "en_US.ISO-8859-1"),
        ("PYTHONUTF8", "0"),
        ("PYTHONCOERCECLOCALE", "0"),
    ]);
    if matches!(
        resolve(&latin1_lookup, Some("en_US.ISO-8859-1")),
        LocaleFact::Resolved { .. }
    ) {
        let captured = OsEncodingContext::from_lookup(latin1_lookup);
        assert_eq!(captured.label(), "iso-8859-1, surrogateescape");
        assert_eq!(
            captured.decode(b"caf\xe9").expect("decode").codepoints(),
            &[0x63, 0x61, 0x66, 0x00e9]
        );
    } else {
        eprintln!("skip: no installed en_US.ISO-8859-1 host data");
    }

    let latin9_lookup = lookup(&[
        ("LC_ALL", "en_US.iso885915"),
        ("PYTHONUTF8", "0"),
        ("PYTHONCOERCECLOCALE", "0"),
    ]);
    if matches!(
        resolve(&latin9_lookup, Some("en_US.iso885915")),
        LocaleFact::Resolved { .. }
    ) {
        let captured = OsEncodingContext::from_lookup(latin9_lookup);
        assert_eq!(captured.label(), "iso-8859-15, surrogateescape");
        assert_eq!(
            captured.decode(b"\xe9\xa4").expect("decode").codepoints(),
            &[0x00e9, 0x20ac]
        );
    } else {
        eprintln!("skip: no installed en_US.iso885915 host data");
    }

    let ascii = OsEncodingContext::from_lookup(lookup(&[
        ("LC_ALL", "C"),
        ("PYTHONUTF8", "0"),
        ("PYTHONCOERCECLOCALE", "0"),
    ]));
    assert_eq!(ascii.label(), "ascii, surrogateescape");
    assert_eq!(
        ascii.decode(b"caf\xe9").expect("decode").codepoints(),
        &[0x63, 0x61, 0x66, 0xdce9]
    );

    let utf8_override = OsEncodingContext::from_lookup(lookup(&[
        ("LC_ALL", "en_US.ISO-8859-1"),
        ("PYTHONUTF8", "1"),
    ]));
    assert_eq!(utf8_override.label(), "utf-8, surrogateescape");
    assert_eq!(
        utf8_override
            .decode(b"caf\xe9")
            .expect("decode")
            .codepoints(),
        &[0x63, 0x61, 0x66, 0xdce9]
    );
}

#[test]
fn decode_path_spells_native_bytes_without_loss() {
    let captured = OsEncodingContext::from_lookup(lookup(&[("LC_ALL", "C.UTF-8")]));
    let path = raw_path(Path::new(""), b"a\x00b\xef\xbf\xbe\xee\x80\x80");
    assert_eq!(
        captured.decode_path(&path).expect("decode").codepoints(),
        &[0x61, 0x00, 0x62, 0xfffe, 0xe000],
        "NUL, a noncharacter and a private-use scalar survive exactly"
    );
}

fn requested_latin1() -> bool {
    std::env::var("LC_ALL").as_deref() == Ok("en_US.ISO-8859-1")
        && !matches!(std::env::var("PYTHONUTF8").as_deref(), Ok("1"))
}

fn requested_default_utf8() -> bool {
    let explicit_utf8 = matches!(std::env::var("PYTHONUTF8").as_deref(), Ok("1"));
    let latin1 = std::env::var("LC_ALL")
        .map(|value| value.to_ascii_lowercase().contains("8859-1"))
        .unwrap_or(false);
    explicit_utf8 || !latin1
}

#[test]
fn latin1_raw_filename_uses_source_fsdecode_spelling() {
    if !requested_latin1() {
        return;
    }
    assert_eq!(
        OsEncodingContext::from_environment().label(),
        "iso-8859-1, surrogateescape",
        "the actual started child is in the requested Latin-1 filesystem context"
    );
    let temp = tempfile::tempdir().expect("tempdir");
    let path = raw_path(temp.path(), b"caf\xe9.txt");
    let record = journal(temp.path())
        .record(Some(&path), facts())
        .expect("record minted");
    assert_eq!(
        &record.file_path().codepoints()[record.file_path().codepoints().len() - 8..],
        &[0x63, 0x61, 0x66, 0x00e9, 0x2e, 0x74, 0x78, 0x74],
        "an actual ISO-8859-1 startup context decodes byte 0xE9 as U+00E9"
    );
}

#[test]
fn default_utf8_raw_filename_uses_surrogateescape() {
    if !requested_default_utf8() {
        return;
    }
    assert_eq!(
        OsEncodingContext::from_environment().label(),
        "utf-8, surrogateescape",
        "the actual started child is in the requested UTF-8 filesystem context"
    );
    let temp = tempfile::tempdir().expect("tempdir");
    let path = raw_path(temp.path(), b"caf\xe9.txt");
    let record = journal(temp.path())
        .record(Some(&path), facts())
        .expect("record minted");
    assert_eq!(
        &record.file_path().codepoints()[record.file_path().codepoints().len() - 8..],
        &[0x63, 0x61, 0x66, 0xdce9, 0x2e, 0x74, 0x78, 0x74],
        "a UTF-8 startup context maps the raw byte to U+DCE9"
    );
}
