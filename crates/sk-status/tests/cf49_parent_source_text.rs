//! Source splits Python text lines before parsing JSON, not only CR/LF lines.

use sk_status::replay::read_source_event_stream_from_text;

#[test]
fn every_python_line_separator_preserves_source_row_numbers() {
    let temporary = tempfile::tempdir().unwrap();
    for separator in [
        '\n', '\r', '\u{000b}', '\u{000c}', '\u{001c}', '\u{001d}', '\u{001e}', '\u{0085}',
        '\u{2028}', '\u{2029}',
    ] {
        let text = format!("{{\"event_type\":\"one\"}}{separator}{{\"event_type\":\"two\"}}");
        let stream = read_source_event_stream_from_text(temporary.path(), &text)
            .unwrap_or_else(|error| panic!("U+{:04X}: {error}", separator as u32));
        assert_eq!(stream.non_lane_lines, [1, 2]);
    }
}

#[test]
fn source_strip_includes_unit_separator_which_is_not_a_line_break() {
    let temporary = tempfile::tempdir().unwrap();
    let stream = read_source_event_stream_from_text(
        temporary.path(),
        "\u{001f}{\"event_type\":\"one\"}\u{001f}",
    )
    .unwrap();
    assert_eq!(stream.non_lane_lines, [1]);
}

#[test]
fn unknown_kind_uses_python_quote_choice_and_control_escaping() {
    let temporary = tempfile::tempdir().unwrap();
    let text = serde_json::json!({"kind": "line\n'quoted'"}).to_string();
    let error = read_source_event_stream_from_text(temporary.path(), &text).unwrap_err();
    assert_eq!(
        error.to_string(),
        r#"Unknown event kind "line\n'quoted'" on line 1"#
    );
}
