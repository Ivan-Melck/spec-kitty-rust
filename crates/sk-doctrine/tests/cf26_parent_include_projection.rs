//! Bounded Source-cause regressions over the real include parser/renderer.
//! Native-only checks; not paired Source execution or exhaustive text proof.

use sk_doctrine::context_include::parse_include_selector;
use sk_doctrine::context_render::{include_body_lines, include_full_payload_lines};

#[test]
fn selector_strips_pythons_control_whitespace_from_both_parts() {
    for character in ['\u{1c}', '\u{1d}', '\u{1e}', '\u{1f}'] {
        let selector = format!("{character}Directive{character}:{character}fixture{character}");
        let parsed =
            parse_include_selector(&selector).expect("Source str.strip accepts these controls");
        assert_eq!(parsed.kind, "directive");
        assert_eq!(parsed.identifier, "fixture");
    }
}

#[test]
fn selector_lowercase_uses_the_pinned_ucd14_authority() {
    // U+1C89 was unassigned in Source's UCD14, so str.lower leaves it alone.
    // Newer Rust Unicode tables recognize a capital with a lowercase partner.
    let parsed = parse_include_selector("\u{1c89}:id").expect("grammar only");
    assert_eq!(parsed.kind, "\u{1c89}");
}

#[test]
fn full_payload_uses_python_ascii_escapes_and_float_spelling() {
    let lines = include_full_payload_lines(&serde_json::json!({
        "ratio": 1e20,
        "label": "é😀",
    }));
    assert_eq!(
        lines.join("\n"),
        concat!(
            "    Full artifact:\n",
            "      {\n",
            "        \"label\": \"\\u00e9\\ud83d\\ude00\",\n",
            "        \"ratio\": 1e+20\n",
            "      }"
        )
    );
}

#[test]
fn inline_body_uses_the_same_python_whitespace_contract() {
    assert_eq!(
        include_body_lines(
            "directive",
            &serde_json::json!({
                "intent": "\u{1c}keep this\u{1f}",
                "scope": "\u{1e}  \u{1d}",
            })
        ),
        vec!["    Intent: keep this".to_owned()]
    );
}
