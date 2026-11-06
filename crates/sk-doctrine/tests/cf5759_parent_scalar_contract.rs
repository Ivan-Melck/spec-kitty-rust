use sk_doctrine::{AuthoredScalar, ExactText, PythonInt, load_org_charter_policy};
use sk_kernel::PythonString;

#[test]
fn finite_text_adapter_refuses_instead_of_returning_fake_null() {
    let text = PythonString::from_codepoints(&[0xd800]).unwrap();
    assert!(serde_json::to_value(ExactText::new(text)).is_err());
}

#[test]
fn finite_union_adapter_refuses_instead_of_returning_fake_null() {
    let text = PythonString::from_codepoints(&[0xd800]).unwrap();
    let value = AuthoredScalar::Text(text);
    assert!(serde_json::to_value(value).is_err());
}

#[test]
fn integer_text_does_not_use_str_strip_information_separators() {
    let value = PythonString::from_scalars("\u{1c}12\u{1f}");
    assert!(PythonInt::parse_text(&value, 4300).is_none());
    assert_eq!(
        PythonInt::parse_text(&PythonString::from_scalars("\u{a0}-١٢\u{a0}"), 4300)
            .unwrap()
            .as_decimal(),
        "-12"
    );
}

#[test]
fn typed_loader_refuses_the_source_invalid_schema_before_a_floor_can_include_it() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("org-charter.yaml");
    std::fs::write(
        &path,
        "schema_version: \"\\x1c12\\x1f\"\nrequired_directives: [BAD1]\n",
    )
    .unwrap();
    assert!(load_org_charter_policy(root.path()).is_err());
}

#[test]
fn integer_order_follows_the_canonical_lower_numeric_rule() {
    let parse = |s: &str| PythonInt::parse_text(&PythonString::from_scalars(s), 0).unwrap();
    assert!(
        parse("-1000000000000000000000000000000000") < parse("-999999999999999999999999999999999")
    );
    assert!(
        parse("999999999999999999999999999999999") < parse("1000000000000000000000000000000000")
    );
}
