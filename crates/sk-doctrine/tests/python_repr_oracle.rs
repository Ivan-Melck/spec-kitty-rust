//! Source-first control: Doctrine's Python `repr` projection vs the oracle.
//!
//! Doctrine used to carry its own `repr` approximation (only quote, backslash
//! and `\n`/`\r`/`\t` escaped; every other non-printable code point emitted raw),
//! which diverged from the reference for exactly the strings this module renders.
//! It now consumes the one canonical `sk_kernel::python_repr` formatter.
//!
//! The matrix is authored, asserted against the pinned interpreter first, and
//! only then compared with Doctrine. The cases include the divergences that
//! motivated the fix: NUL, DEL, C1 controls, a private-use code point, a
//! noncharacter, a zero-width space, an emoji, both quote shapes, a backslash and
//! a real newline.

mod support;

use std::process::{Command, Stdio};

use serde_json::{Value, json};
use sk_doctrine::python_value_repr;

/// `(label, authored Python literal, expected `repr()` result)`.
///
/// The middle field is the Python expression evaluated by the oracle, so the
/// oracle compares the real interpreter against the authored expectation before
/// Doctrine is involved at all.
const CASES: &[(&str, &str, &str)] = &[
    ("plain", r#"'zig'"#, r#"'zig'"#),
    ("space", r#"'a b'"#, r#"'a b'"#),
    ("single-quote", r#""a'b""#, r#""a'b""#),
    ("both-quotes", r#""a'b\"c""#, r#"'a\'b"c'"#),
    ("double-quote", r#"'a"b'"#, r#"'a"b'"#),
    ("backslash", r#"'a\\b'"#, r#"'a\\b'"#),
    ("newline", r#"'a\nb'"#, r#"'a\nb'"#),
    ("tab", r#"'a\tb'"#, r#"'a\tb'"#),
    ("carriage", r#"'a\rb'"#, r#"'a\rb'"#),
    ("nul", r#"'a\x00b'"#, r#"'a\x00b'"#),
    ("del", r#"'a\x7fb'"#, r#"'a\x7fb'"#),
    ("c1-control", r#"'a\x85b'"#, r#"'a\x85b'"#),
    ("nbsp", r#"'a\u00a0b'"#, r#"'a\xa0b'"#),
    ("soft-hyphen", r#"'a\u00adb'"#, r#"'a\xadb'"#),
    ("private-use", r#"'a\ue000b'"#, r#"'a\ue000b'"#),
    ("noncharacter", r#"'a\ufdd0b'"#, r#"'a\ufdd0b'"#),
    ("zero-width-space", r#"'a\u200bb'"#, r#"'a\u200bb'"#),
    ("line-separator", r#"'a\u2028b'"#, r#"'a\u2028b'"#),
    ("bom", r#"'a\ufeffb'"#, r#"'a\ufeffb'"#),
    ("emoji", r#"'a\U0001f600b'"#, r#"'a😀b'"#),
    ("astral-printable", r#"'a\U00010000b'"#, r#"'a𐀀b'"#),
    ("accented", r#"'caf\u00e9'"#, r#"'café'"#),
    ("greek", r#"'\u03b1\u03c3'"#, r#"'ασ'"#),
    ("empty", r#"''"#, r#"''"#),
];

const SOURCE_SCRIPT: &str = r#"
import json, os
from pathlib import Path

root = Path(os.environ["SK_REPR_ROOT"]).resolve()
assert Path.cwd().resolve() == root, (Path.cwd(), root)
for key in ("HOME", "TMPDIR", "XDG_CACHE_HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"):
    path = Path(os.environ[key]).resolve()
    assert str(path).startswith(str(root) + os.sep), (key, path, root)

cases = json.loads(Path(os.environ["SK_REPR_CASES"]).read_text(encoding="utf-8"))
out = {}
for label, expression, expected in cases:
    value = eval(expression, {"__builtins__": {}}, {})
    rendered = repr(value)
    assert rendered == expected, (label, rendered, expected)
    out[label] = {"codepoints": [ord(character) for character in rendered]}
print(json.dumps(out))
"#;

fn scalars(text: &str) -> Vec<u32> {
    text.chars().map(u32::from).collect()
}

fn points(value: &Value) -> Vec<u32> {
    value
        .as_array()
        .expect("code point array")
        .iter()
        .map(|code| u32::try_from(code.as_u64().expect("integer")).expect("u32"))
        .collect()
}

#[test]
fn doctrine_repr_matches_the_pinned_interpreter() {
    let temp = tempfile::tempdir().expect("temporary root");
    let root = temp.path().canonicalize().expect("canonical root");
    for directory in ["home", "cache", "config", "data", "state", "tmp"] {
        std::fs::create_dir_all(root.join(directory)).expect("isolated directory");
    }
    let cases: Vec<Value> = CASES
        .iter()
        .map(|(label, expression, expected)| json!([label, expression, expected]))
        .collect();
    let cases_path = root.join("cases.json");
    std::fs::write(
        &cases_path,
        serde_json::to_vec(&cases).expect("serialise cases"),
    )
    .expect("write cases");
    let output = Command::new(support::python())
        .arg("-c")
        .arg(SOURCE_SCRIPT)
        .env_clear()
        .envs(vec![
            ("PATH".to_owned(), std::env::var("PATH").unwrap_or_default()),
            ("HOME".to_owned(), root.join("home").display().to_string()),
            (
                "XDG_CACHE_HOME".to_owned(),
                root.join("cache").display().to_string(),
            ),
            (
                "XDG_CONFIG_HOME".to_owned(),
                root.join("config").display().to_string(),
            ),
            (
                "XDG_DATA_HOME".to_owned(),
                root.join("data").display().to_string(),
            ),
            (
                "XDG_STATE_HOME".to_owned(),
                root.join("state").display().to_string(),
            ),
            ("TMPDIR".to_owned(), root.join("tmp").display().to_string()),
            ("PYTHONDONTWRITEBYTECODE".to_owned(), "1".to_owned()),
            ("SK_REPR_ROOT".to_owned(), root.display().to_string()),
            ("SK_REPR_CASES".to_owned(), cases_path.display().to_string()),
        ])
        .current_dir(&root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run source oracle");
    assert!(
        output.status.success(),
        "Source oracle failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let source: Value = serde_json::from_slice(&output.stdout).expect("oracle JSON");

    // The authored literals, decoded here so the comparison is a value
    // comparison (not a second hand-rolled Python parser).
    let values: &[(&str, serde_yaml_ng::Value)] = &[
        ("plain", serde_yaml_ng::Value::String("zig".to_owned())),
        ("space", serde_yaml_ng::Value::String("a b".to_owned())),
        (
            "single-quote",
            serde_yaml_ng::Value::String("a'b".to_owned()),
        ),
        (
            "both-quotes",
            serde_yaml_ng::Value::String("a'b\"c".to_owned()),
        ),
        (
            "double-quote",
            serde_yaml_ng::Value::String("a\"b".to_owned()),
        ),
        ("backslash", serde_yaml_ng::Value::String("a\\b".to_owned())),
        ("newline", serde_yaml_ng::Value::String("a\nb".to_owned())),
        ("tab", serde_yaml_ng::Value::String("a\tb".to_owned())),
        ("carriage", serde_yaml_ng::Value::String("a\rb".to_owned())),
        ("nul", serde_yaml_ng::Value::String("a\u{0}b".to_owned())),
        ("del", serde_yaml_ng::Value::String("a\u{7f}b".to_owned())),
        (
            "c1-control",
            serde_yaml_ng::Value::String("a\u{85}b".to_owned()),
        ),
        ("nbsp", serde_yaml_ng::Value::String("a\u{a0}b".to_owned())),
        (
            "soft-hyphen",
            serde_yaml_ng::Value::String("a\u{ad}b".to_owned()),
        ),
        (
            "private-use",
            serde_yaml_ng::Value::String("a\u{e000}b".to_owned()),
        ),
        (
            "noncharacter",
            serde_yaml_ng::Value::String("a\u{fdd0}b".to_owned()),
        ),
        (
            "zero-width-space",
            serde_yaml_ng::Value::String("a\u{200b}b".to_owned()),
        ),
        (
            "line-separator",
            serde_yaml_ng::Value::String("a\u{2028}b".to_owned()),
        ),
        ("bom", serde_yaml_ng::Value::String("a\u{feff}b".to_owned())),
        (
            "emoji",
            serde_yaml_ng::Value::String("a\u{1f600}b".to_owned()),
        ),
        (
            "astral-printable",
            serde_yaml_ng::Value::String("a\u{10000}b".to_owned()),
        ),
        ("accented", serde_yaml_ng::Value::String("café".to_owned())),
        ("greek", serde_yaml_ng::Value::String("ασ".to_owned())),
        ("empty", serde_yaml_ng::Value::String(String::new())),
    ];
    assert_eq!(values.len(), CASES.len());
    for ((label, value), (_, _, _)) in values.iter().zip(CASES) {
        let expected = source.get(*label).expect("oracle case");
        let native = python_value_repr(value);
        assert_eq!(
            scalars(&native),
            points(&expected["codepoints"]),
            "{label}: Doctrine repr {native:?} must equal the interpreter"
        );
    }

    // Composite values: `repr` of a list renders its strings through the same
    // formatter (this is the interview-answer shape Doctrine parses).
    let list = serde_yaml_ng::Value::Sequence(vec![
        serde_yaml_ng::Value::String("zig".to_owned()),
        serde_yaml_ng::Value::String("c".to_owned()),
    ]);
    assert_eq!(python_value_repr(&list), "['zig', 'c']");
    assert_eq!(python_value_repr(&serde_yaml_ng::Value::Null), "None");
    assert_eq!(python_value_repr(&serde_yaml_ng::Value::Bool(true)), "True");
    assert_eq!(
        python_value_repr(&serde_yaml_ng::Value::Number(3.into())),
        "3"
    );
}
