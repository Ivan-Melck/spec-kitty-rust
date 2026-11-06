//! CF42: `charter context --include directive:<id>` through the real binary.
//!
//! The layered Directive repository is served by the CLI adapter end to end: this
//! project fixture overrides one shipped built-in directive, so the printed record must
//! be the **merged** one (project `title`/`intent`, inherited built-in fields) with the
//! CF38 model dump — and the include read must not write anything.
//!
//! Not a whole-command parity campaign: `crates/sk-doctrine/tests/cf42_layered_directive.rs`
//! owns the paired Source/Native comparison; the remaining debt is enumerated in the lane
//! report.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const CHARTER_MD: &str =
    "# Fixture Charter\n\n## Policy Summary\n\n- Keep one canonical authority.\n";

/// The project-tier pack root spelling the reference uses for directives (singular).
const PROJECT_OVERRIDE: &str = "schema_version: \"1.0\"\n\
id: DIRECTIVE_024\n\
title: Project Locality Title\n\
intent: Project-authored locality intent.\n";

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, contents).expect("fixture write");
}

/// A project fixture the CLI resolves a root and a doctrine bundle from.
fn fixture() -> tempfile::TempDir {
    let temp = tempfile::TempDir::new().expect("temp");
    let root = temp.path();
    write(
        &root.join(".kittify/charter/activation.yaml"),
        "# authored activation state\n",
    );
    write(&root.join(".kittify/charter/charter.md"), CHARTER_MD);
    write(
        &root.join(".kittify/config.yaml"),
        "# authored project config\ncustom_setting: keep-me\n",
    );
    write(
        &root.join(".kittify/charter-packs/directive/900-project-locality.directive.yaml"),
        PROJECT_OVERRIDE,
    );
    temp
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_spec-kitty"))
        .args(args)
        .current_dir(root)
        .env("SPECIFY_REPO_ROOT", root)
        .env_remove("SPEC_KITTY_PACKS_ROOT")
        .output()
        .expect("binary runs")
}

/// Every file under `root`, with its bytes — the no-write footprint.
fn footprint(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(directory: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let Ok(entries) = fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => walk(&path, files),
                Ok(_) => {
                    let bytes = fs::read(&path).expect("fixture file");
                    files.insert(path, bytes);
                }
                Err(error) => panic!("fixture entry type: {error}"),
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(root, &mut files);
    files
}

#[test]
fn a_project_override_is_served_merged_and_the_read_writes_nothing() {
    let temp = fixture();
    let root = temp.path();
    let before = footprint(root);
    let output = run(
        root,
        &[
            "charter",
            "context",
            "--include",
            "directive:024-locality-of-change",
        ],
    );
    assert!(
        output.status.success(),
        "exit {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    // The heading and the replaced fields are the project layer's; the rest is the
    // built-in record the overlay inherited.
    assert!(
        text.starts_with("Directive DIRECTIVE_024: Project Locality Title\n"),
        "{text}"
    );
    assert!(
        text.contains("    Intent: Project-authored locality intent."),
        "{text}"
    );
    assert!(
        text.contains("    Scope: Applies to code, tests, and documentation"),
        "the built-in scope is inherited, not dropped: {text}"
    );
    assert!(
        text.contains("\"title\": \"Project Locality Title\""),
        "the full payload is the merged model dump: {text}"
    );
    assert!(
        text.contains("\"scope\": \"Applies to code, tests, and documentation"),
        "{text}"
    );
    // No raw authored YAML is printed as the payload.
    assert!(!text.contains("schema_version:"), "{text}");

    // The include path is a read: nothing on disk changed and no state file appeared.
    assert_eq!(footprint(root), before, "the include wrote to the project");
    assert!(
        !root.join(".kittify/charter/context-state.json").exists(),
        "the include path must not mark first-load state"
    );
}

#[test]
fn the_declared_id_and_the_numeric_shorthand_render_the_same_record() {
    let temp = fixture();
    let root = temp.path();
    let by_stem = run(
        root,
        &[
            "charter",
            "context",
            "--include",
            "directive:024-locality-of-change",
        ],
    );
    let by_id = run(
        root,
        &["charter", "context", "--include", "directive:DIRECTIVE_024"],
    );
    let by_number = run(root, &["charter", "context", "--include", "directive:24"]);
    for output in [&by_id, &by_number] {
        assert_eq!(
            output.status.code(),
            by_stem.status.code(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, by_stem.stdout);
    }
}

#[test]
fn an_unadmitted_directive_is_the_reference_refusal() {
    let temp = fixture();
    let root = temp.path();
    // A document the Source `Directive` model rejects (no `enforcement`) is skipped by
    // the repository, so the selector is a plain miss — never a rendered raw document.
    write(
        &root.join(".kittify/charter-packs/directive/901-invalid.directive.yaml"),
        "schema_version: \"1.0\"\nid: DIRECTIVE_901\ntitle: Missing Enforcement\nintent: Invalid.\n",
    );
    let output = run(
        root,
        &["charter", "context", "--include", "directive:901-invalid"],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("Error: No directive found for selector 'directive:901-invalid'."),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
