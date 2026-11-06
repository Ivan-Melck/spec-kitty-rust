use sk_kernel::encoding_provenance::EncodingProvenanceJournal;
use sk_kernel::text_decode::read_text_with_provenance;
use sk_kernel::{
    PythonString,
    python_json::{self, PythonJsonValue},
};
use std::fs;

#[test]
fn provenance_keeps_source_str_coercion_and_exact_mission_id_units() {
    let cases: Vec<(&[u8], PythonString)> = vec![
        (
            br#"{"mission_id":null}"#,
            PythonString::from_scalars("None"),
        ),
        (
            br#"{"mission_id":true}"#,
            PythonString::from_scalars("True"),
        ),
        (br#"{"mission_id":123}"#, PythonString::from_scalars("123")),
        (br#"{"mission_id":NaN}"#, PythonString::from_scalars("nan")),
        (
            br#"{"mission_id":[1,true]}"#,
            PythonString::from_scalars("[1, True]"),
        ),
        (
            br#"{"mission_id":"\ud800"}"#,
            PythonString::from_units(vec![0xd800]).unwrap(),
        ),
    ];
    for (raw, expected) in cases {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("kitty-specs/m");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("meta.json"), raw).unwrap();
        let path = dir.join("plan.md");
        fs::write(&path, "Authored text").unwrap();
        let journal = EncodingProvenanceJournal::new(root.path());
        let read = read_text_with_provenance(&path, &journal).unwrap();
        let log = fs::read_to_string(read.log_path().unwrap()).unwrap();
        let record = python_json::parse(log.trim()).unwrap();
        assert_eq!(
            record.field("mission_id"),
            Some(&PythonJsonValue::String(expected)),
            "raw={raw:?}"
        );
    }
}

#[test]
fn uncaught_metadata_errors_do_not_become_successful_null_records() {
    let huge = format!("{{\"mission_id\":{}}}", "1".repeat(4301));
    for raw in [
        b"null".as_slice(),
        br#"["mission_id"]"#,
        br#""mission_id""#,
        b"\xff",
        huge.as_bytes(),
    ] {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("kitty-specs/m");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("meta.json"), raw).unwrap();
        let path = dir.join("plan.md");
        fs::write(&path, "Authored text").unwrap();
        let journal = EncodingProvenanceJournal::new(root.path());
        assert!(
            read_text_with_provenance(&path, &journal).is_err(),
            "raw prefix={:?}",
            &raw[..raw.len().min(40)]
        );
        assert!(!dir.join(".encoding-provenance.jsonl").exists());
    }
}

#[test]
fn source_caught_json_syntax_still_records_a_null_mission_id() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("kitty-specs/m");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("meta.json"), b"{").unwrap();
    let path = dir.join("plan.md");
    fs::write(&path, "Authored text").unwrap();
    let read =
        read_text_with_provenance(&path, &EncodingProvenanceJournal::new(root.path())).unwrap();
    let log = fs::read_to_string(read.log_path().unwrap()).unwrap();
    assert_eq!(
        python_json::parse(log.trim()).unwrap().field("mission_id"),
        Some(&PythonJsonValue::Null)
    );
}
