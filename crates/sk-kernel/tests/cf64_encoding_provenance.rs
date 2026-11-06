//! CF64 — Kernel encoding-provenance lifecycle controls.
//!
//! These are Native controls over the *reference-observed* record shape, routing
//! rule, phase order and caught-write policy (the Source observations are in the
//! lane report; the paired Source/Native business-read comparison lives in
//! `sk-doctrine`'s `cf64_encoding_provenance_reads`).
//!
//! Nothing here writes outside its own `tempfile` tree.

use std::path::Path;

use chrono::{DateTime, Utc};
use sk_kernel::PythonString;
use sk_kernel::clock::FrozenClock;
use sk_kernel::encoding_provenance::{
    ACTOR, AMBIGUOUS_CODE, EncodingProvenanceError, EncodingProvenanceJournal, EventIdSource,
    GLOBAL_LOG_RELATIVE, MISSION_LOG_FILENAME, ReadEncodingFacts, SPECS_DIR_NAME,
};
use sk_kernel::text_decode::{
    ProvenanceReadError, decode_unambiguous, detect_bom, normalize_newlines,
    read_text_with_provenance,
};

const FIXED_EVENT_ID: &str = "01M6QNZ0JDENFRX8NS45HBGPD0";

/// A deterministic identity source: the reference's event ids are opaque, so a
/// fixed one makes the rendered line comparable byte-for-byte.
struct FixedIds;

impl EventIdSource for FixedIds {
    fn next_event_id(&self, _millis: i64) -> Result<String, EncodingProvenanceError> {
        Ok(FIXED_EVENT_ID.to_owned())
    }
}

fn frozen_clock() -> FrozenClock {
    let instant = DateTime::parse_from_rfc3339("2026-11-06T05:40:34.509035+00:00")
        .expect("fixed instant")
        .with_timezone(&Utc);
    FrozenClock::new(instant)
}

fn journal(root: &Path) -> EncodingProvenanceJournal {
    EncodingProvenanceJournal::with_sources(root, Box::new(frozen_clock()), Box::new(FixedIds))
}

fn utf8_facts() -> ReadEncodingFacts<'static> {
    ReadEncodingFacts {
        source_encoding: "utf-8",
        confidence: 1.0,
        normalization_applied: false,
        bypass_used: false,
    }
}

fn write(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("fixture parent");
    }
    std::fs::write(path, bytes).expect("fixture write");
}

fn read_lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .expect("log readable")
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn record_line_matches_the_reference_rendering() {
    let temp = tempfile::tempdir().expect("tempdir");
    let document = temp.path().join(".kittify/charter/charter.md");
    write(&document, b"# Charter\n");

    let journal = journal(temp.path());
    let record = journal
        .record(Some(&document), utf8_facts())
        .expect("record minted");

    assert_eq!(record.event_id(), FIXED_EVENT_ID);
    assert_eq!(record.at(), "2026-11-06T05:40:34.509035+00:00");
    assert_eq!(record.actor(), ACTOR);
    assert_eq!(record.source_encoding(), "utf-8");
    assert_eq!(record.confidence(), 1.0);
    assert!(!record.normalization_applied());
    assert!(!record.bypass_used());
    assert_eq!(record.mission_id(), None);
    assert_eq!(
        record.file_path().to_scalar_string().expect("path text"),
        document.to_string_lossy()
    );

    // `json.dumps(record, sort_keys=True)` — sorted keys, `", "`/`": "`
    // separators, `ensure_ascii=True`, a Python float rendered `1.0`.
    let expected = format!(
        "{{\"actor\": \"spec-kitty charter\", \"at\": \"2026-11-06T05:40:34.509035+00:00\", \
\"bypass_used\": false, \"confidence\": 1.0, \"event_id\": \"{FIXED_EVENT_ID}\", \
\"file_path\": \"{}\", \"mission_id\": null, \"normalization_applied\": false, \
\"source_encoding\": \"utf-8\"}}",
        document.to_string_lossy()
    );
    assert_eq!(record.to_json_line(), expected);
}

#[test]
fn zero_confidence_renders_as_a_python_float() {
    let temp = tempfile::tempdir().expect("tempdir");
    let journal = journal(temp.path());
    let record = journal
        .record(
            None,
            ReadEncodingFacts {
                source_encoding: "cp1252",
                confidence: 0.0,
                normalization_applied: true,
                bypass_used: true,
            },
        )
        .expect("record minted");
    // The reference's unsafe-bypass record is exactly this shape (Source-probed).
    assert!(
        record.to_json_line().contains("\"confidence\": 0.0,"),
        "{}",
        record.to_json_line()
    );
    assert!(record.to_json_line().contains("\"bypass_used\": true,"));
    // Inline bytes carry an empty `file_path`.
    assert_eq!(record.file_path().to_scalar_string().expect("text"), "");
}

#[test]
fn routing_prefers_the_mission_log_and_never_writes_both() {
    let temp = tempfile::tempdir().expect("tempdir");
    let journal = journal(temp.path());

    let global = journal.route(Some(Path::new("docs/notes.md")));
    assert_eq!(global, temp.path().join(GLOBAL_LOG_RELATIVE));

    let mission = journal.route(Some(Path::new("kitty-specs/099-probe/plan.md")));
    // A relative source path yields the reference's relative route, which the
    // append anchors on the journal's project root (the reference anchors it on
    // the process CWD, which is the project root in the real CLI).
    assert_eq!(
        mission,
        Path::new(SPECS_DIR_NAME)
            .join("099-probe")
            .join(MISSION_LOG_FILENAME)
    );

    // An absolute source path keeps its own prefix, exactly as the reference
    // derives the mission directory from the path's components.
    let absolute = temp.path().join("kitty-specs/099-probe/spec.md");
    assert_eq!(
        journal.route(Some(&absolute)),
        temp.path()
            .join("kitty-specs")
            .join("099-probe")
            .join(MISSION_LOG_FILENAME)
    );

    // `kitty-specs` with nothing after it is not a mission directory.
    assert_eq!(
        journal.route(Some(Path::new("kitty-specs"))),
        temp.path().join(GLOBAL_LOG_RELATIVE)
    );
    // No path at all routes to the project log.
    assert_eq!(journal.route(None), temp.path().join(GLOBAL_LOG_RELATIVE));
}

#[test]
fn mission_id_uses_source_str_on_the_meta_json_value() {
    let temp = tempfile::tempdir().expect("tempdir");
    let journal = journal(temp.path());
    let mission = temp.path().join("kitty-specs/099-probe");
    std::fs::create_dir_all(&mission).expect("mission dir");

    write(
        &mission.join("meta.json"),
        br#"{"mission_id": "01M6QNZ0JDENFRX8NS45HBGPD0"}"#,
    );
    assert_eq!(
        journal.mission_id(Some(&mission.join("plan.md"))).unwrap(),
        Some(PythonString::from_scalars("01M6QNZ0JDENFRX8NS45HBGPD0"))
    );

    // Source applies str() even to a non-string value; authored data is unchanged.
    write(&mission.join("meta.json"), br#"{"mission_id": 42}"#);
    assert_eq!(
        journal.mission_id(Some(&mission.join("plan.md"))).unwrap(),
        Some(PythonString::from_scalars("42"))
    );

    // Absent `meta.json`, absent key, and a non-mission path all read as absent.
    std::fs::remove_file(mission.join("meta.json")).expect("remove meta");
    assert_eq!(
        journal.mission_id(Some(&mission.join("plan.md"))).unwrap(),
        None
    );
    write(&mission.join("meta.json"), br#"{"slug": "099-probe"}"#);
    assert_eq!(
        journal.mission_id(Some(&mission.join("plan.md"))).unwrap(),
        None
    );
    assert_eq!(journal.mission_id(Some(temp.path())).unwrap(), None);
    assert_eq!(journal.mission_id(None).unwrap(), None);
}

#[test]
fn every_successful_read_appends_and_nothing_is_deduplicated() {
    let temp = tempfile::tempdir().expect("tempdir");
    let document = temp.path().join(".kittify/charter/charter.md");
    write(&document, b"# Charter\n");

    let journal = journal(temp.path());
    let first = read_text_with_provenance(&document, &journal).expect("first read");
    let second = read_text_with_provenance(&document, &journal).expect("second read");

    assert_eq!(
        first.text().to_scalar_string().expect("text"),
        "# Charter\n"
    );
    assert_eq!(
        second.text().to_scalar_string().expect("text"),
        "# Charter\n"
    );
    assert!(first.warnings().is_empty());
    assert!(second.warnings().is_empty());

    let log = temp.path().join(GLOBAL_LOG_RELATIVE);
    assert_eq!(first.log_path(), Some(log.as_path()));
    let lines = read_lines(&log);
    assert_eq!(lines.len(), 2, "one record per read, no deduplication");
    assert_eq!(lines[0], lines[1]);
    // The two records are byte-identical here only because the clock and the
    // identity are frozen; a real journal mints a fresh pair per read (probed).
    assert!(lines[0].contains(FIXED_EVENT_ID));
}

#[test]
fn a_caught_write_failure_keeps_the_read_and_reports_the_reference_warning() {
    let temp = tempfile::tempdir().expect("tempdir");
    let document = temp.path().join(".kittify/charter/charter.md");
    write(&document, b"# Charter\n");
    // Make the provenance *directory* a regular file so `mkdir -p` fails.
    let blocker = temp.path().join(".kittify/encoding-provenance");
    write(&blocker, b"not a directory\n");

    let journal = journal(temp.path());
    let read = read_text_with_provenance(&document, &journal).expect("the read still succeeds");
    assert_eq!(read.text().to_scalar_string().expect("text"), "# Charter\n");
    assert_eq!(read.warnings().len(), 1);
    let warning = &read.warnings()[0];
    assert_eq!(
        warning.message(),
        format!(
            "Failed to write encoding provenance to {}",
            temp.path().join(GLOBAL_LOG_RELATIVE).display()
        )
    );
    assert!(warning.source().is_caught_by_reference());
    // The failed path is untouched: no partial log, no directory swap.
    assert_eq!(
        std::fs::read_to_string(&blocker).expect("blocker intact"),
        "not a directory\n"
    );
}

#[test]
fn an_unprovable_encoding_refuses_before_any_record() {
    let temp = tempfile::tempdir().expect("tempdir");
    let document = temp.path().join(".kittify/charter/charter.md");
    write(&document, b"# caf\xe9 not utf-8\n");

    let journal = journal(temp.path());
    let error = read_text_with_provenance(&document, &journal).expect_err("cp1252 refuses");
    match error {
        ProvenanceReadError::Unprovable { code, path, .. } => {
            assert_eq!(code, AMBIGUOUS_CODE);
            assert_eq!(path, document);
        }
        other => panic!("expected the ambiguity refusal, got {other:?}"),
    }
    assert!(
        !temp.path().join(GLOBAL_LOG_RELATIVE).exists(),
        "a refused read appends nothing"
    );
}

#[test]
fn absent_file_fails_without_a_record() {
    let temp = tempfile::tempdir().expect("tempdir");
    let journal = journal(temp.path());
    let missing = temp.path().join(".kittify/charter/absent.md");
    let error = read_text_with_provenance(&missing, &journal).expect_err("absent refuses");
    assert!(
        matches!(error, ProvenanceReadError::Read { .. }),
        "{error:?}"
    );
    assert!(!temp.path().join(GLOBAL_LOG_RELATIVE).exists());
}

#[test]
fn a_utf8_bom_read_reports_sig_and_normalization() {
    let temp = tempfile::tempdir().expect("tempdir");
    let document = temp.path().join(".kittify/charter/charter.md");
    write(&document, b"\xef\xbb\xbf# Charter\n");

    let journal = journal(temp.path());
    let read = read_text_with_provenance(&document, &journal).expect("bom read");
    assert_eq!(read.source_encoding(), "utf-8-sig");
    assert!(read.normalization_applied());
    assert!(!read.bypass_used());
    assert_eq!(read.text().to_scalar_string().expect("text"), "# Charter\n");
    let line = read_lines(&temp.path().join(GLOBAL_LOG_RELATIVE)).remove(0);
    assert!(line.contains("\"normalization_applied\": true,"), "{line}");
    assert!(
        line.contains("\"source_encoding\": \"utf-8-sig\""),
        "{line}"
    );
}

#[test]
fn a_mission_document_routes_to_the_mission_log_with_its_mission_id() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mission = temp.path().join("kitty-specs/099-probe");
    let document = mission.join("plan.md");
    write(&document, b"# Plan\n");
    write(
        &mission.join("meta.json"),
        br#"{"mission_id": "01M6QNZ0JDENFRX8NS45HBGPD0"}"#,
    );

    let journal = journal(temp.path());
    let read = read_text_with_provenance(&document, &journal).expect("mission read");
    let mission_log = mission.join(MISSION_LOG_FILENAME);
    assert_eq!(read.log_path(), Some(mission_log.as_path()));
    assert!(
        !temp.path().join(GLOBAL_LOG_RELATIVE).exists(),
        "a mission record never lands in the global log"
    );
    let line = read_lines(&mission_log).remove(0);
    assert!(
        line.contains("\"mission_id\": \"01M6QNZ0JDENFRX8NS45HBGPD0\","),
        "{line}"
    );
}

#[cfg(unix)]
#[test]
fn a_non_utf8_file_name_is_reported_with_surrogateescape_units() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let temp = tempfile::tempdir().expect("tempdir");
    let name = OsStr::from_bytes(b"caf\xff.md");
    let document = temp.path().join(name);
    write(&document, b"# notes\n");

    let journal = journal(temp.path());
    let read = read_text_with_provenance(&document, &journal).expect("read");
    assert_eq!(read.text().to_scalar_string().expect("text"), "# notes\n");
    let line = read_lines(&temp.path().join(GLOBAL_LOG_RELATIVE)).remove(0);
    // `os.fsdecode` maps the single raw byte to U+DCFF; `ensure_ascii` renders it
    // as the `\udcff` escape, never U+FFFD and never a lossy replacement.
    assert!(line.contains("\\udcff"), "{line}");
    assert!(!line.contains('\u{fffd}'), "{line}");
}

#[test]
fn the_pure_codecs_stay_pure() {
    let temp = tempfile::tempdir().expect("tempdir");
    let data = b"\xef\xbb\xbf# Charter\r\n";

    let bom = detect_bom(data).expect("a recognised BOM");
    assert_eq!(bom.codec(), "utf-8-sig");
    assert_eq!(bom.source_encoding(), "utf-8-sig");

    let (text, encoding) = decode_unambiguous(data).expect("proven decode");
    assert_eq!(encoding, "utf-8-sig");
    assert_eq!(normalize_newlines(&text), "# Charter\n");

    // A BOM-only payload decodes to empty text under its own encoding, and an
    // unprovable byte decodes to nothing at all.
    assert_eq!(
        decode_unambiguous(b"\xff\xfe"),
        Some((String::new(), "utf-16-le"))
    );
    assert_eq!(decode_unambiguous(b"\x80"), None);

    // Decoding is I/O-free: nothing exists in the fixture tree afterwards.
    assert!(
        std::fs::read_dir(temp.path())
            .expect("fixture dir")
            .next()
            .is_none()
    );
}
