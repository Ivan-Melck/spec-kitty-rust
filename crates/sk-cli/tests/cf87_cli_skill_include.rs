//! CF87 — the real `spec-kitty` binary delivers a Source-valid pack skill through
//! `charter context --include skill:<id>`.
//!
//! This is the tiny operator-visible consumer of the delivery fixed in
//! `sk-doctrine`'s include dispatch: a project-tier `*.skill.yaml` plus its
//! sibling prompt body is served on the plain offering route, with the authored
//! model payload, never a structured miss and never raw YAML. The denied arm is
//! annotated, not refused, and the read writes nothing into the project.
//!
//! Not whole-command parity: the Source/Native codepoint comparison lives in
//! `crates/sk-doctrine/tests/cf87_skill_include_delivery.rs`; this target proves
//! only that the production binary actually reaches that route.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

const SKILL: &str = "fixture-skill";
const BODY: &str = "Fixture skill body.\n";
const DENIED: &str = "skill:fixture-skill is deactivated in this project \
                      (charter activate skill fixture-skill restores it)";

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, contents).expect("fixture write");
}

/// A project fixture with one Source-valid project-tier pack skill and its body.
fn fixture() -> tempfile::TempDir {
    let temp = tempfile::TempDir::new().expect("temp");
    let root = temp.path();
    write(
        &root.join(".kittify/charter/charter.md"),
        "# Fixture Charter\n",
    );
    write(
        &root.join(".kittify/config.yaml"),
        "# authored project config\ncustom_setting: keep-me\n",
    );
    let skills = root.join(".kittify/charter-packs/skills");
    write(
        &skills.join(format!("{SKILL}.skill.yaml")),
        &format!(
            "schema_version: '1.0'\n\
             id: {SKILL}\n\
             title: Fixture Skill\n\
             description: Fixture skill description.\n\
             form: prompt\n\
             triggers: [alpha trigger, beta trigger]\n\
             body_path: {SKILL}.skill.md\n"
        ),
    );
    write(&skills.join(format!("{SKILL}.skill.md")), BODY);
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

fn json(output: &Output) -> Value {
    serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap_or_else(|error| {
        panic!(
            "stdout is not JSON ({error}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

/// Every path below `root` with its bytes, for the no-write assertion.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn walk(base: &Path, directory: &Path, out: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        let Ok(entries) = fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let relative = path.strip_prefix(base).unwrap_or(&path).to_path_buf();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                out.insert(relative, None);
                walk(base, &path, out);
            } else {
                out.insert(relative, Some(fs::read(&path).unwrap_or_default()));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

#[test]
fn the_real_binary_delivers_a_source_valid_pack_skill() {
    let temp = fixture();
    let before = snapshot(temp.path());

    let output = run(
        temp.path(),
        &["charter", "context", "--include", "skill:fixture-skill"],
    );
    assert!(
        output.status.success(),
        "exit {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).expect("utf-8 stdout");
    assert!(
        text.starts_with("Skill fixture-skill: Fixture Skill\n"),
        "{text}"
    );
    assert!(
        text.contains("\n    Description: Fixture skill description.\n"),
        "{text}"
    );
    assert!(text.contains("\n    Form: prompt\n"), "{text}");
    assert!(
        text.contains("\n    Triggers: alpha trigger, beta trigger\n"),
        "{text}"
    );
    // The exact model dump (the reference's `Full artifact:` payload) keeps the
    // authored body path; the include phase does not invent inline body text.
    assert!(
        text.contains("\"body_path\": \"fixture-skill.skill.md\""),
        "{text}"
    );
    assert!(text.contains("    Full artifact:\n      {\n"), "{text}");
    assert!(!text.contains("schema_version:"), "no raw YAML: {text}");

    // `--json` carries the same text; it is a success envelope, not a refusal.
    let envelope = run(
        temp.path(),
        &[
            "charter",
            "context",
            "--include",
            "skill:fixture-skill",
            "--json",
        ],
    );
    assert!(
        envelope.status.success(),
        "exit {:?}: {}",
        envelope.status.code(),
        String::from_utf8_lossy(&envelope.stderr)
    );
    let payload = json(&envelope);
    assert_eq!(payload["result"], "success");
    assert_eq!(payload["success"], true);
    assert_eq!(
        payload["text"],
        Value::String(text.trim_end_matches('\n').to_owned())
    );
    assert_eq!(
        payload["context"],
        Value::String(text.trim_end_matches('\n').to_owned())
    );
    assert!(payload.get("code").is_none(), "{payload}");

    let after = snapshot(temp.path());
    assert_eq!(before, after, "the include path changed authored bytes");
    assert!(
        !temp
            .path()
            .join(".kittify/charter/context-state.json")
            .exists(),
        "an include must not record first-load state"
    );

    // The same route annotates a deactivated skill and still exits successfully.
    write(
        &temp.path().join(".kittify/charter/activation.yaml"),
        "deactivated_skills:\n  - fixture-skill\n",
    );
    let denied = run(
        temp.path(),
        &["charter", "context", "--include", "skill:fixture-skill"],
    );
    assert!(
        denied.status.success(),
        "exit {:?}: {}",
        denied.status.code(),
        String::from_utf8_lossy(&denied.stderr)
    );
    // The CLI applies its existing text-rail width to the annotation; the
    // reference sentence is asserted modulo that wrap, which is a separate
    // console-layout surface (not part of the include delivery slice).
    let denied_text = String::from_utf8_lossy(&denied.stdout);
    let normalized = denied_text.split_whitespace().collect::<Vec<_>>().join(" ");
    assert_eq!(normalized.as_str(), DENIED);
}
