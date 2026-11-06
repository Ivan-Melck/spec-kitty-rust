//! CF50 — the real `charter context` (non-include) binary: exact merged delivery.
//!
//! The doctrine-level paired proof (`cf50_exact_action_delivery.rs`) establishes
//! that the action payload matches the pinned Source child. This file proves the two
//! CLI phases of the NON-INCLUDE surface:
//!
//! 1. `--json` prints the reference envelope in the reference's key order through the
//!    ONE Kernel `json.dumps` encoder (`ensure_ascii=True`, no `sort_keys`), so the
//!    merged record's body arrives with its authored code points escaped
//!    (`\u00e9`, `\u2603`, `\ud800`) — not U+FFFD, not a `serde_json` re-encoder,
//!    and not the declaring file's finite projection.
//! 2. Text mode prints the reference's `Action Doctrine` line
//!    (`- <id>: <title> — <summary>`) and never inlines the artifact body, and the
//!    writes are exactly the reference's: the first-load state publication
//!    (`_mark_action_loaded`) plus, because the fixture bundle has no
//!    `metadata.yaml`, the charter read's one encoding-provenance record.
//!
//! The `--include` fast path is untouched and proved separately (CF43).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The project overlay: replaces `title` (non-ASCII), `intent` (a lone surrogate)
/// and `procedures`; inherits `scope`, `integrity_rules` and the rest from the real
/// built-in `DIRECTIVE_001`.
const OVERLAY: &[u8] = b"schema_version: \"1.0\"\n\
id: DIRECTIVE_001\n\
title: \"Overlay integrity \\u00e9\\u2603 title\"\n\
intent: \"Overlay intent with a lone surrogate \\ud800 and a smile \\u263a.\\n\"\n\
procedures:\n\
\x20 - overlay procedure one\n";

fn write(path: &Path, contents: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, contents).expect("fixture write");
}

/// A git-backed project fixture whose project layer shadows the built-in record.
fn fixture() -> tempfile::TempDir {
    let temp = tempfile::TempDir::new().expect("temp");
    let root = temp.path();
    write(
        &root.join(".kittify/config.yaml"),
        b"# authored project config\ncustom_setting: keep-me\n",
    );
    write(
        &root.join(".kittify/charter/charter.yaml"),
        b"schema_version: \"1.1.0\"\ncatalog:\n  references: []\ngovernance:\n  authority_paths: [docs/context]\n",
    );
    write(
        &root.join(".kittify/charter/charter.md"),
        b"# Fixture Charter\n\n## Policy Summary\n\n- one rule\n",
    );
    write(
        &root.join(
            ".kittify/charter-packs/directive/001-architectural-integrity-standard.directive.yaml",
        ),
        OVERLAY,
    );
    let output = Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(root)
        .output()
        .expect("run git init");
    assert!(
        output.status.success(),
        "git init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    temp
}

fn run(root: &Path, json_output: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_spec-kitty"));
    command
        .current_dir(root)
        .env("SPECIFY_REPO_ROOT", root)
        .env_remove("SPEC_KITTY_PACKS_ROOT")
        .env_remove("PYTHONIOENCODING")
        .env("PYTHONUTF8", "1")
        .args([
            "charter",
            "context",
            "--action",
            "implement",
            "--mission-type",
            "software-dev",
        ]);
    if json_output {
        command.arg("--json");
    }
    command.output().expect("real binary")
}

/// Every file below `root/.kittify`, for the write assertion.
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
fn the_json_payload_carries_the_merged_body_through_the_kernel_encoder() {
    let temp = fixture();
    let root = temp.path();
    let output = run(root, true);
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

    // The reference CLI's envelope keys, in the reference's insertion order. The
    // markers carry the `indent=2` prefix, so a nested entry key can never satisfy
    // one of them.
    let keys = [
        "\n  \"result\": \"success\"",
        "\n  \"success\": true",
        "\n  \"context_schema_version\": \"1.4.0\"",
        "\n  \"action\": \"implement\"",
        "\n  \"mode\": \"bootstrap\"",
        "\n  \"first_load\": true",
        "\n  \"references_count\": ",
        "\n  \"context\": ",
        "\n  \"text\": ",
        "\n  \"directives\": [",
        "\n  \"all_directives\": [",
        "\n  \"diagnostics\": ",
        "\n  \"directives_source\": ",
        "\n  \"tactics\": ",
        "\n  \"styleguides\": ",
        "\n  \"toolguides\": ",
        "\n  \"references\": [",
        "\n  \"governance_references\": ",
        "\n  \"project_charter\": ",
        "\n  \"org_charter\": ",
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

    // The merged record: the OVERLAY title/summary/procedures and the INHERITED
    // fields, with the authored code points escaped exactly like `json.dumps`.
    assert!(
        stdout.contains("\"title\": \"Overlay integrity \\u00e9\\u2603 title\""),
        "the overlaid title is the delivered one: {stdout}"
    );
    assert!(
        stdout.contains("\\ud800"),
        "the authored lone surrogate survives as \\ud800: {stdout}"
    );
    assert!(
        stdout.contains("\"scope\": \"Applies to all design proposals"),
        "an overridden-away field is inherited from the built-in record: {stdout}"
    );
    assert!(
        stdout.contains("\"procedures\": [\n"),
        "the overridden list is delivered: {stdout}"
    );
    assert!(
        stdout.contains("\"delivery\": \"inline\""),
        "the requires closure is inline: {stdout}"
    );
    assert!(
        stdout.contains("\"source\": \"project\""),
        "the entry's provenance is the overlay layer: {stdout}"
    );
    // The body carries no reopened declaring-file title from the built-in record.
    assert!(
        !stdout.contains("\"title\": \"Architectural Integrity Standard\""),
        "the declaring file's title must not win over the overlay: {stdout}"
    );
}

#[test]
fn unencodable_text_preserves_the_source_summary_and_prior_first_load_write() {
    let temp = fixture();
    let root = temp.path();
    let before = snapshot(root);
    let output = run(root, false);
    assert_eq!(
        output.status.code(),
        Some(1),
        "Source cannot encode the authored high surrogate"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("surrogates not allowed"));
    let text = String::from_utf8(output.stdout).expect("only the ASCII header was written");
    assert_eq!(text, "Action: implement (bootstrap)\n");
    assert!(
        !text.contains("Intent: "),
        "the bootstrap text never inlines an artifact body: {text}"
    );
    assert!(
        !text.contains('\u{FFFD}'),
        "a replacement character must never be substituted"
    );

    // The reference writes twice on this path: the first-load state publication and
    // the charter read's encoding-provenance record (its bundle-freshness gate runs
    // before the render, and this fixture has no `metadata.yaml`, so the bundle is
    // stale and the read is ingested).
    let after = snapshot(root);
    let state = root.join(".kittify/charter/context-state.json");
    assert!(state.exists(), "the first-load state file is published");
    let published = fs::read_to_string(&state).expect("state file");
    assert!(
        published.contains("\"implement\""),
        "the action is recorded: {published}"
    );
    let journal = root.join(".kittify/encoding-provenance/global.jsonl");
    let journal_text = fs::read_to_string(&journal).expect("provenance journal");
    let records: Vec<&str> = journal_text
        .lines()
        .filter(|line| !line.is_empty())
        .collect();
    assert_eq!(records.len(), 1, "one read, one record: {records:?}");
    assert!(
        records[0].contains(&format!(
            "\"file_path\": \"{}\"",
            root.join(".kittify/charter/charter.md").display()
        )),
        "the record names the charter it read: {}",
        records[0]
    );
    assert!(
        records[0].contains("\"source_encoding\": \"utf-8\""),
        "the record carries the proven encoding: {}",
        records[0]
    );
    let mut expected = before;
    expected.insert(state.clone(), fs::read(&state).expect("state bytes"));
    expected.insert(journal.clone(), fs::read(&journal).expect("journal bytes"));
    assert_eq!(
        after, expected,
        "no other authored byte below .kittify changed"
    );
}
