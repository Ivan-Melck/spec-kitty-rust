use sk_charter::ArtifactKind;
use sk_doctrine::{ActivationReadError, DoctrineError, load_activation_ledger_exact};
use sk_kernel::PythonString;
use std::fs;
fn root(doc: &str, legacy: bool) -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join(if legacy {
        ".kittify/config.yaml"
    } else {
        ".kittify/charter/activation.yaml"
    });
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, doc).unwrap();
    t
}
#[test]
fn escaped_surrogate_unknown_deny_key_is_data_not_repr_identifier() {
    let t = root("\"deactivated_\\ud800\": [\"\\udfff\"]\n", false);
    let ledger = load_activation_ledger_exact(t.path()).expect(
        "Source retains exact unknown deny key rather than rejecting its repr as unknown metadata",
    );
    assert_eq!(
        ledger.unknown_deny_keys[0],
        PythonString::from_units(
            "deactivated_"
                .chars()
                .map(u32::from)
                .chain([0xd800])
                .collect()
        )
        .unwrap()
    );
    assert_eq!(ledger.unknown_deny_values.len(), 1);
    assert_eq!(
        ledger.unknown_deny_values[0].1.as_ref().unwrap()[0].codepoints(),
        &[0xdfff]
    );
}
#[test]
fn all_surrogate_unknown_keys_and_values_remain_exact() {
    let mut doc = String::new();
    for cp in 0xd800..=0xdfff {
        doc.push_str(&format!("\"deactivated_\\u{cp:04x}\": [\"\\u{cp:04x}\"]\n"));
    }
    let t = root(&doc, false);
    let ledger = load_activation_ledger_exact(t.path()).unwrap();
    assert_eq!(ledger.unknown_deny_keys.len(), 2048);
    for (i, (key, values)) in ledger.unknown_deny_values.iter().enumerate() {
        let cp = 0xd800 + u32::try_from(i).unwrap();
        assert_eq!(key.codepoints().last(), Some(&cp));
        assert_eq!(values.as_ref().unwrap()[0].codepoints(), &[cp]);
    }
    assert!(ledger.to_scalar_view().is_err());
}
#[test]
fn empty_and_null_document_roots_are_empty_present_ledgers() {
    for doc in ["", "null\n", "~\n", "# comment\n"] {
        let t = root(doc, false);
        let ledger = load_activation_ledger_exact(t.path())
            .expect("Source accepts empty/null root as empty mapping");
        assert!(ledger.exists);
        assert!(ledger.activated_kinds.is_none());
    }
}
#[test]
fn unknown_null_key_has_no_unknown_value_entry() {
    let t = root("deactivated_mystery: null\n", false);
    let ledger = load_activation_ledger_exact(t.path()).unwrap();
    assert_eq!(ledger.unknown_deny_keys.len(), 1);
    assert!(ledger.unknown_deny_values.is_empty());
}
#[test]
fn known_field_value_failure_precedes_unknown_key_validation() {
    let t = root("activated_directives: 7\nsurprise: []\n", false);
    let error = load_activation_ledger_exact(t.path()).unwrap_err();
    assert!(
        matches!(error,ActivationReadError::Doctrine(DoctrineError::InvalidLedgerValue{ref key,..}) if key=="activated_directives"),
        "Source parses known fields before L1 key validation: {error:?}"
    );
}
#[test]
fn legacy_lists_use_source_python_str_coercion() {
    let t = root("activated_directives: [1, true, null, ' raw ']\n", true);
    let ledger = load_activation_ledger_exact(t.path())
        .expect("Source coerces legacy list items, only modern ledger ids are strict");
    assert_eq!(
        ledger.allow_for(ArtifactKind::Directive).unwrap(),
        &["1", "True", "None", " raw "].map(PythonString::from_scalars)
    );
}
