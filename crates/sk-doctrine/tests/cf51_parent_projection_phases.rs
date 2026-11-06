//! Model admission and projection stages must not be reordered.

use std::fs;
use std::path::Path;

use sk_charter::ArtifactKind;
use sk_doctrine::context_include::{
    IncludeAccess, IncludeError, IncludeInputs, parse_include_selector, render_include_exact,
};
use sk_doctrine::exact_payload;
use sk_kernel::python_json::PythonJsonValue;
use sk_kernel::python_yaml::{Dialect, GraphDocument, decode_graph};

const BASE: &str =
    "id: parent-map\nschema_version: '1.0'\ntitle: Parent map\nscope: code\nprinciples: [p]\n";

fn fixture(document: &str) -> tempfile::TempDir {
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path().join(".kittify/charter-packs/styleguide");
    fs::create_dir_all(&directory).unwrap();
    fs::write(temporary.path().join(".kittify/config.yaml"), "# fixture\n").unwrap();
    fs::write(directory.join("fixture.styleguide.yaml"), document).unwrap();
    temporary
}

fn render(root: &Path, identifier: &str) -> Result<String, IncludeError> {
    let access = IncludeAccess::load(root).unwrap();
    let selector = format!("styleguide:{identifier}");
    let parsed = parse_include_selector(&selector).unwrap();
    render_include_exact(
        &IncludeInputs {
            project_root: root,
            access: &access,
        },
        &parsed,
        &selector,
    )
    .map(|text| text.to_scalar_string().unwrap())
}

#[test]
fn invalid_typed_root_keys_skip_the_document_at_model_admission() {
    let temporary = fixture(&format!("{BASE}structural_lint_config: {{1: invalid}}\n"));
    assert!(matches!(
        render(temporary.path(), "parent-map"),
        Err(IncludeError::Missing { .. })
    ));
}

#[test]
fn an_invalid_overlay_retains_the_valid_builtin_record() {
    let temporary = fixture("id: common-docs\nstructural_lint_config: {1: invalid}\n");
    let rendered = render(temporary.path(), "common-docs").unwrap();
    assert!(rendered.starts_with("Styleguide common-docs:"));
    assert!(!rendered.contains("\"1\": \"invalid\""));
}

#[test]
fn typed_string_map_accepts_utf8_bytes_keys_and_values() {
    let temporary = fixture(&format!(
        "{BASE}tooling:\n  !!binary a2V5: !!binary dmFsdWU=\n"
    ));
    let rendered = render(temporary.path(), "parent-map").unwrap();
    assert!(rendered.contains("\"key\": \"value\""), "{rendered}");
}

#[test]
fn typed_root_key_collisions_are_resolved_before_any_value_serialization() {
    let temporary = fixture(&format!(
        "{BASE}structural_lint_config:\n  !!binary a2V5: !!binary /w==\n  key: good\n"
    ));
    let rendered = render(temporary.path(), "parent-map").unwrap();
    assert!(rendered.contains("\"key\": \"good\""), "{rendered}");
}

fn projected(yaml: &str) -> PythonJsonValue {
    let document = GraphDocument::new(decode_graph(yaml, Dialect::RuamelSafe12).unwrap());
    exact_payload(ArtifactKind::Styleguide, document.root()).unwrap()
}

#[test]
fn a_last_null_after_key_projection_removes_the_overwritten_member() {
    let payload = projected(&format!(
        "{BASE}structural_lint_config:\n  nested:\n    1: first\n    '1': null\n"
    ));
    let nested = payload
        .field("structural_lint_config")
        .unwrap()
        .field("nested")
        .unwrap();
    let PythonJsonValue::ExactObject(object) = nested else {
        panic!("expected object");
    };
    assert!(object.is_empty(), "{object:?}");
}

#[test]
fn a_first_null_still_reserves_key_position_before_later_replacement() {
    let payload = projected(&format!(
        "{BASE}structural_lint_config:\n  nested:\n    before: b\n    1: null\n    after: a\n    '1': last\n"
    ));
    let nested = payload
        .field("structural_lint_config")
        .unwrap()
        .field("nested")
        .unwrap();
    let PythonJsonValue::ExactObject(object) = nested else {
        panic!("expected object");
    };
    let keys: Vec<String> = object
        .keys()
        .map(|key| key.to_scalar_string().unwrap())
        .collect();
    assert_eq!(keys, ["before", "1", "after"]);
}
