//! CF43 — the real `charter context --include` binary: exact emission phases.
//!
//! The doctrine-level paired proof (`cf43_exact_include_delivery.rs`) establishes
//! that the rendered text matches the pinned Source child. This file proves the two
//! *emission* phases of the CLI boundary:
//!
//! 1. `--json` prints the reference envelope (`result`, `success`, `include`,
//!    `context`, `text`, in that insertion order) through the ONE Kernel
//!    `json.dumps` encoder with `ensure_ascii=True`, so an authored lone surrogate
//!    reaches stdout as `\ud800` and non-ASCII text as `\uXXXX` — not as U+FFFD,
//!    not as a marker spelling, and not via a `serde_json` re-encoder of a
//!    scalar projection.
//! 2. Text mode writes the exact code points as UTF-8 (the reference's own late
//!    `str` → encode step). A value that step cannot encode refuses with a distinct
//!    message that names `--json`; it is neither a success nor a "missing artifact".
//!
//! The include read is a pure projection: no authored byte changes, and no
//! first-load state file is written.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const GLOSSARY: &str = "glossary-pack:spec-kitty-core";
const DIRECTIVE: &str = "directive:010-specification-fidelity-requirement";

fn write(path: &Path, contents: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, contents).expect("fixture write");
}

/// A project fixture whose project layer shadows a real built-in glossary pack with
/// a lone surrogate and a typed `NaN`.
fn fixture() -> tempfile::TempDir {
    let temp = tempfile::TempDir::new().expect("temp");
    let root = temp.path();
    write(
        &root.join(".kittify/charter/charter.md"),
        b"# Fixture Charter\n",
    );
    write(
        &root.join(".kittify/config.yaml"),
        b"# authored project config\ncustom_setting: keep-me\n",
    );
    // A single-backslash `\ud800` YAML escape is a real lone surrogate; the
    // documented read-only fallback needs the bytes, not a Rust `str`.
    write(
        &root.join(".kittify/charter-packs/glossary_packs/spec-kitty-core.glossary-pack.yaml"),
        b"id: spec-kitty-core\n\
          description: \"Overlay description \\ud800.\"\n\
          terms:\n\
          \x20 - surface: nan-term\n\
          \x20   definition: \"h\\u00e9llo \\u2603 nan\"\n\
          \x20   confidence: .nan\n\
          \x20   status: active\n",
    );
    temp
}

fn run(root: &Path, selector: &str, json_output: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_spec-kitty"));
    command
        .current_dir(root)
        .env("SPECIFY_REPO_ROOT", root)
        .env_remove("SPEC_KITTY_PACKS_ROOT")
        .env("COLUMNS", "80")
        .args(["charter", "context", "--include", selector]);
    if json_output {
        command.arg("--json");
    }
    command.output().expect("real binary")
}

/// Every file below `root/.kittify`, for the no-write assertion.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(directory: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let Ok(entries) = fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                walk(&path, out);
            } else {
                out.insert(path.clone(), fs::read(&path).unwrap_or_default());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(&root.join(".kittify"), &mut out);
    out
}

fn position(document: &str, needle: &str) -> usize {
    document
        .find(needle)
        .unwrap_or_else(|| panic!("missing {needle:?} in {document}"))
}

#[test]
fn the_json_envelope_carries_the_exact_text_through_the_kernel_encoder() {
    let temp = fixture();
    let root = temp.path();
    let before = snapshot(root);
    let output = run(root, GLOSSARY, true);
    assert!(
        output.status.success(),
        "exit {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("ensure_ascii=True writes ASCII");
    assert!(
        !stdout.contains('\u{FFFD}'),
        "a replacement character must never be substituted"
    );
    assert!(
        !stdout.contains("CF43"),
        "no marker spelling leaks into the output"
    );

    // The reference's own key set, in the reference's insertion order.
    let keys = [
        "\"result\": \"success\"",
        "\"success\": true",
        "\"include\": \"glossary-pack:spec-kitty-core\"",
        "\"context\": ",
        "\"text\": ",
    ];
    let mut previous = 0;
    for key in keys {
        let at = position(&stdout, key);
        assert!(
            at >= previous,
            "envelope key order drifted at {key:?}: {stdout}"
        );
        previous = at;
    }

    // The lone surrogate is the reference `json.dumps` escape, and the typed NaN
    // is the reference's bare token (not `null`, not a string).
    assert!(
        stdout.contains("\\\\ud800") || stdout.contains("\\ud800"),
        "the authored surrogate is escaped as \\ud800: {stdout}"
    );
    assert!(
        stdout.contains("\\\"confidence\\\": NaN"),
        "the typed NaN survives the envelope (escaped inside the JSON string): {stdout}"
    );
    assert!(
        stdout.contains("h\\u00e9llo"),
        "non-ASCII text is escaped like the reference: {stdout}"
    );

    // The read wrote nothing and recorded no first-load state.
    assert_eq!(before, snapshot(root), "the include path wrote no file");
    assert!(!root.join(".kittify/charter/context-state.json").exists());
}

#[test]
fn text_mode_prints_the_exact_text_and_json_carries_the_same_value() {
    let temp = fixture();
    let root = temp.path();
    let text_output = run(root, DIRECTIVE, false);
    assert!(
        text_output.status.success(),
        "exit {:?}: {}",
        text_output.status.code(),
        String::from_utf8_lossy(&text_output.stderr)
    );
    let text = String::from_utf8(text_output.stdout).expect("the finite text is UTF-8");
    assert!(
        text.starts_with("Directive DIRECTIVE_010: Specification Fidelity Requirement\n"),
        "{text}"
    );
    assert!(text.contains("    Full artifact:\n"), "{text}");
    assert!(text.ends_with('\n'), "the reference's trailing newline");

    let json_output = run(root, DIRECTIVE, true);
    assert!(json_output.status.success(), "{json_output:?}");
    let envelope: serde_json::Value =
        serde_json::from_slice(&json_output.stdout).expect("one JSON document");
    // Source machine JSON carries the raw rendered doctrine; the human arm
    // applies CliConsole layout before encoding. They are not byte-identical.
    let raw = envelope["context"].as_str().expect("raw include text");
    let expected =
        sk_cli::compat::console_text::prepare(&sk_kernel::PythonString::from_scalars(raw), 80)
            .expect("positive console width")
            .to_scalar_string()
            .expect("finite fixture");
    assert_eq!(
        text,
        format!("{expected}\n"),
        "the human arm consumes the canonical Source layout"
    );
    assert_ne!(
        raw,
        text.trim_end_matches('\n'),
        "this Source fixture really wraps"
    );
    assert_eq!(
        envelope["text"], envelope["context"],
        "`text` repeats `context`, exactly like the reference"
    );
    assert_eq!(envelope["result"], "success");
    assert_eq!(envelope["success"], true);
    assert_eq!(envelope["include"], DIRECTIVE);
}

#[test]
fn a_non_encodable_text_phase_refuses_instead_of_losing_the_value() {
    let temp = fixture();
    let root = temp.path();
    let output = run(root, GLOSSARY, false);
    assert_eq!(
        output.status.code(),
        Some(1),
        "a physical encoding failure is not a success: {:?}",
        output.status
    );
    assert!(
        output.stdout.is_empty(),
        "nothing partial is printed: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot print the rendered text of --include") && stderr.contains("--json"),
        "the refusal names the phase and the route that carries the value: {stderr}"
    );
    assert!(
        !stderr.contains("No glossary_pack found")
            && !stderr.contains("No glossary-pack found")
            && !stderr.contains("CONTEXT_INCLUDE_PENDING"),
        "an encoding failure must not be reported as a missing artifact or a gap: {stderr}"
    );
}
