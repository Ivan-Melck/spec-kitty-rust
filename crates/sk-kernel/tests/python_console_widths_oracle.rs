//! Source-first parity for the console cell-width primitive.
//!
//! `sk_kernel::python_console` reproduces `rich.cells.get_character_cell_size`
//! and `rich.cells.cell_len` for the locked Rich release. This test measures the
//! **whole code-point space** and a set of composed strings in the pinned
//! reference first, then asserts the native primitive agrees exactly:
//!
//! * every width-≠-1 range over `0..=U+10FFFF` (so CJK, combining marks,
//!   noncharacters, private use, surrogates, unassigned planes are all covered);
//! * `cell_len` for composed strings exercising the ASCII fast path, the
//!   per-code-point sum, `U+FE0F` widening and the `U+200D` consumption rule.
//!
//! The oracle is required and lock-verified ([`oracle_support::python`]): a
//! missing or drifted interpreter panics, never a zero-comparison pass.

#[path = "support/oracle.rs"]
mod oracle_support;

use std::process::{Command, Stdio};

use serde_json::Value;

const SOURCE_SCRIPT: &str = r#"
import json
import rich
from rich import cells

table = cells.load_cell_table("auto")
widths = [cells.get_character_cell_size(chr(code)) for code in range(0x110000)]

ranges = []
start = None
current = None
for code, width in enumerate(widths):
    if width == 1:
        if start is not None:
            ranges.append([start, code - 1, current])
            start = None
            current = None
        continue
    if start is not None and width == current:
        continue
    if start is not None:
        ranges.append([start, code - 1, current])
    start = code
    current = width
if start is not None:
    ranges.append([start, 0x10FFFF, current])

composed = {
    "empty": "",
    "ascii": "Hello, world",
    "tabs": "a\tb",
    "cjk": "\u4e2d\u6587 \u5b57",
    "combining": "a\u0301e\u0300",
    "astral": "\U0001F600\U0001F469",
    "heart_vs16": "\u2764\uFE0F",
    "heart": "\u2764",
    "family_zwj": "\U0001F469\u200D\U0001F4BB",
    "zwj_tail": "a\u200Db",
    "double_zwj": "\u200D\u200D",
    "pua": "\uE000\uF8FF",
    "noncharacter": "\uFDD0\uFFFE",
    "nul": "a\u0000b",
    "controls": "\u0001\u001F\u007F\u009B",
    "soft_hyphen": "\u00ADx",
    "braille": "\u2800\u28FF",
    "box": "\u2500\u256D",
    "amharic": "\u1200\u137F",
    "hebrew": "\u05D0\u05EA",
}
print(json.dumps({
    "rich": getattr(rich, "VERSION", None) or "15.0.0",
    "unicode": table.unicode_version,
    "ranges": ranges,
    "cell_len": {name: cells.cell_len(text) for name, text in composed.items()},
}))
"#;

fn source_facts() -> Value {
    let python = oracle_support::python();
    let home = tempfile::tempdir().expect("temporary HOME");
    for directory in ["home", "cache", "config", "data", "state", "tmp"] {
        std::fs::create_dir_all(home.path().join(directory)).expect("isolated directory");
    }
    let cwd = tempfile::tempdir().expect("temporary CWD");
    let output = Command::new(&python)
        .arg("-c")
        .arg(SOURCE_SCRIPT)
        .current_dir(cwd.path())
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", home.path().join("home"))
        .env("XDG_CACHE_HOME", home.path().join("cache"))
        .env("XDG_CONFIG_HOME", home.path().join("config"))
        .env("XDG_DATA_HOME", home.path().join("data"))
        .env("XDG_STATE_HOME", home.path().join("state"))
        .env("TMPDIR", home.path().join("tmp"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("NO_COLOR", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run the pinned width probe");
    assert!(
        output.status.success(),
        "the pinned width probe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let facts: Value = serde_json::from_slice(&output.stdout).expect("probe JSON");
    assert_eq!(
        facts["rich"].as_str(),
        Some(sk_kernel::python_console::RICH_VERSION),
        "the pinned Rich release changed; regenerate the width table"
    );
    assert_eq!(
        facts["unicode"].as_str(),
        Some(sk_kernel::python_console::RICH_UNICODE_VERSION),
        "the pinned Rich width table version changed; regenerate the width table"
    );
    facts
}

/// The native width ranges over the whole code-point space.
fn native_ranges() -> Vec<(u32, u32, usize)> {
    let mut ranges: Vec<(u32, u32, usize)> = Vec::new();
    let mut current: Option<(u32, u32, usize)> = None;
    for code in 0..=0x10FFFFu32 {
        let width = sk_kernel::python_console::codepoint_cell_size(code);
        if width == 1 {
            if let Some(range) = current.take() {
                ranges.push(range);
            }
            continue;
        }
        match current {
            Some((start, _, existing)) if existing == width => {
                current = Some((start, code, width));
            }
            Some(range) => {
                ranges.push(range);
                current = Some((code, code, width));
            }
            None => current = Some((code, code, width)),
        }
    }
    if let Some(range) = current {
        ranges.push(range);
    }
    ranges
}

#[test]
fn every_code_point_width_matches_the_reference() {
    let facts = source_facts();
    let source: Vec<(u32, u32, usize)> = facts["ranges"]
        .as_array()
        .expect("range list")
        .iter()
        .map(|row| {
            let row = row.as_array().expect("range row");
            (
                row[0].as_u64().expect("start") as u32,
                row[1].as_u64().expect("end") as u32,
                row[2].as_u64().expect("width") as usize,
            )
        })
        .collect();
    assert!(!source.is_empty(), "the reference reported no ranges");
    let native = native_ranges();
    // Report the first divergence precisely instead of a giant vec diff.
    let mut mismatches: Vec<String> = Vec::new();
    for (index, (source_range, native_range)) in source.iter().zip(&native).enumerate() {
        if source_range != native_range {
            mismatches.push(format!(
                "range {index}: reference {source_range:?} != native {native_range:?}"
            ));
            if mismatches.len() >= 8 {
                break;
            }
        }
    }
    assert_eq!(
        source.len(),
        native.len(),
        "range count differs: reference {} native {} ({mismatches:?})",
        source.len(),
        native.len()
    );
    assert!(mismatches.is_empty(), "{mismatches:?}");
    // And spot-check the individual widths, not only the compressed ranges.
    for &(start, end, width) in &source {
        for code in [start, start + (end - start) / 2, end] {
            assert_eq!(
                sk_kernel::python_console::codepoint_cell_size(code) as u64,
                width as u64,
                "U+{code:04X}"
            );
        }
    }
}

#[test]
fn composed_string_cell_lengths_match_the_reference() {
    let facts = source_facts();
    let cases: &[(&str, &str)] = &[
        ("empty", ""),
        ("ascii", "Hello, world"),
        ("tabs", "a\tb"),
        ("cjk", "中文 字"),
        ("combining", "a\u{0301}e\u{0300}"),
        ("astral", "😀👩"),
        ("heart_vs16", "❤\u{FE0F}"),
        ("heart", "❤"),
        ("family_zwj", "👩\u{200D}💻"),
        ("zwj_tail", "a\u{200D}b"),
        ("double_zwj", "\u{200D}\u{200D}"),
        ("pua", "\u{E000}\u{F8FF}"),
        ("noncharacter", "\u{FDD0}\u{FFFE}"),
        ("nul", "a\u{0}b"),
        ("controls", "\u{1}\u{1F}\u{7F}\u{9B}"),
        ("soft_hyphen", "\u{AD}x"),
        ("braille", "\u{2800}\u{28FF}"),
        ("box", "\u{2500}\u{256D}"),
        ("amharic", "\u{1200}\u{137F}"),
        ("hebrew", "\u{5D0}\u{5EA}"),
    ];
    for (name, text) in cases {
        let expected = facts["cell_len"][*name]
            .as_u64()
            .expect("reference cell_len");
        assert_eq!(
            sk_kernel::python_console::cell_len(text) as u64,
            expected,
            "{name}: {text:?} in source {facts:?}"
        );
    }
    // An exact-code-point path for a lone surrogate (never a Rust `str`).
    assert_eq!(sk_kernel::python_console::codepoint_cell_len(&[0xD800]), 1);
    assert_eq!(
        sk_kernel::python_console::codepoint_cell_len(&[0x61, 0xD800, 0x62]),
        3
    );
}
