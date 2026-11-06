use sk_kernel::PythonString;
use sk_kernel::python_json::PythonJsonValue;
use sk_status::replay::read_source_event_stream_from_text;
use sk_status::source_annotation::python_int_value;

#[test]
fn source_annotation_integer_text_uses_the_same_lexical_rule() {
    let root = tempfile::tempdir().unwrap();
    let row = serde_json::json!({
        "kind": "annotation", "event_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV",
        "wp_id": "WP-A", "at": "now", "actor": "claude",
        "delta": {"shell_pid": "\u{1c}12\u{1f}"}
    });
    let error = read_source_event_stream_from_text(root.path(), &row.to_string()).unwrap_err();
    assert!(
        error.to_string().contains("invalid literal for int()"),
        "{error}"
    );
}

#[test]
fn source_annotation_integer_string_obeys_the_shared_interpreter_ceiling() {
    let limit = sk_kernel::python_json::environment_integer_digits().unwrap();
    if limit == 0 {
        return;
    }
    let value = PythonJsonValue::String(PythonString::from_scalars(&"1".repeat(limit + 1)));
    let error = python_int_value(&value).unwrap_err();
    assert!(error.contains("Exceeds the limit"), "{error}");
}
