#![cfg(unix)]
use sk_kernel::{
    PythonString,
    encoding_provenance::{EncodingProvenanceJournal, ReadEncodingFacts},
    python_os_encoding::OsEncodingContext,
};
use std::{ffi::OsStr, os::unix::ffi::OsStrExt, path::Path};

fn utf8() -> OsEncodingContext {
    OsEncodingContext::from_lookup(|key| (key == "PYTHONUTF8").then(|| "1".into()))
}

#[test]
fn path_spelling_matches_pathlib_not_raw_byte_decoding() {
    let codec = utf8();
    let cases: &[(&[u8], &str)] = &[
        (b"", "."),
        (b".", "."),
        (b"./", "."),
        (b"./a", "a"),
        (b"a//b/.", "a/b"),
        (b"a/./../b", "a/../b"),
        (b"//a//b/..", "//a/b/.."),
        (b"///a", "/a"),
        (b"/", "/"),
        (b"\\foo\\bar", "\\foo\\bar"),
        (b"\0//a", "\0/a"),
    ];
    let actual: Vec<_> = cases
        .iter()
        .map(|(bytes, _)| {
            codec
                .decode_path(Path::new(OsStr::from_bytes(bytes)))
                .unwrap()
        })
        .collect();
    let expected: Vec<_> = cases
        .iter()
        .map(|(_, text)| PythonString::from_scalars(text))
        .collect();
    assert_eq!(actual, expected);
    assert_eq!(
        codec.decode(b"a//b/.").unwrap(),
        PythonString::from_scalars("a//b/."),
        "raw fsdecode bytes must NOT be path-normalized"
    );
    assert_eq!(
        codec
            .decode_path(Path::new(OsStr::from_bytes(b"a//\xff/./..")))
            .unwrap()
            .codepoints(),
        &[97, 47, 0xdcff, 47, 46, 46]
    );
}

#[test]
fn provenance_some_empty_path_is_dot_and_distinct_from_no_path() {
    let root = tempfile::tempdir().unwrap();
    let journal = EncodingProvenanceJournal::new(root.path());
    let facts = ReadEncodingFacts {
        source_encoding: "utf-8",
        confidence: 1.0,
        normalization_applied: false,
        bypass_used: false,
    };
    assert_eq!(
        journal
            .record(Some(Path::new("")), facts)
            .unwrap()
            .file_path()
            .codepoints(),
        &[46]
    );
    assert!(journal.record(None, facts).unwrap().file_path().is_empty());
}
