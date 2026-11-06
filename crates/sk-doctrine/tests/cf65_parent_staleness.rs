use sk_doctrine::charter_bundle_staleness;
use std::fs;

#[test]
fn source_falsy_metadata_documents_are_stale_not_attribute_failures() {
    for text in ["false", "0", "[]", "{}", "''", ""] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("metadata.yaml");
        fs::write(&path, text).unwrap();
        let result = charter_bundle_staleness(None, &path, Some("Authored"));
        assert!(
            result.is_ok(),
            "Source accepts falsy metadata {text:?}: {result:?}"
        );
        assert!(result.unwrap().stale);
    }
}

#[test]
fn source_ruamel_metadata_byte_policy_accepts_utf16_bom() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("metadata.yaml");
    let text = format!("charter_hash: {}", sk_charter::hash_content("Authored"));
    let mut bytes = vec![0xff, 0xfe];
    for unit in text.encode_utf16() {
        bytes.extend(unit.to_le_bytes());
    }
    fs::write(&path, bytes).unwrap();
    let result = charter_bundle_staleness(None, &path, Some("Authored"));
    assert!(result.is_ok(), "Source accepts UTF16 YAML: {result:?}");
    assert!(!result.unwrap().stale);
}

#[test]
fn stored_hash_preserves_the_actual_requested_value() {
    use sk_kernel::{PythonString, python_value::PythonValue};
    let cases = [
        ("charter_hash: null", PythonValue::Null),
        (
            "charter_hash: [1, true]",
            PythonValue::Seq(vec![
                PythonValue::Int("1".to_owned()),
                PythonValue::Bool(true),
            ]),
        ),
        (
            r#"charter_hash: "\ud800""#,
            PythonValue::Str(PythonString::from_units(vec![0xd800]).unwrap()),
        ),
    ];
    for (text, expected) in cases {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("metadata.yaml");
        fs::write(&path, text).unwrap();
        let result = charter_bundle_staleness(None, &path, Some("Authored")).unwrap();
        assert!(result.stale);
        assert_eq!(result.stored_hash, expected);
    }
}

#[test]
fn unrelated_metadata_cycle_does_not_force_a_whole_root_projection() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("metadata.yaml");
    fs::write(
        &path,
        format!(
            "other: &cycle [*cycle]\ncharter_hash: {}",
            sk_charter::hash_content("Authored")
        ),
    )
    .unwrap();
    assert!(
        !charter_bundle_staleness(None, &path, Some("Authored"))
            .unwrap()
            .stale
    );
}

#[test]
fn source_surrogate_metadata_hash_is_data_not_a_yaml_refusal() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("metadata.yaml");
    fs::write(&path, r#"charter_hash: "\ud800""#).unwrap();
    let result = charter_bundle_staleness(None, &path, Some("Authored"));
    assert!(
        result.is_ok(),
        "Source accepts exact surrogate value: {result:?}"
    );
    assert!(result.unwrap().stale);
}
