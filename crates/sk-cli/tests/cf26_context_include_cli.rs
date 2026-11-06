//! CF26 focused smoke: `charter context --include` through the real `spec-kitty` binary.
//!
//! These assert the two output envelopes (verbatim text, and the reference's
//! `{"result": "success", "include": …, "context": …, "text": …}` JSON), the exit codes
//! of a reference refusal, and that `section:` keeps its existing rendering. They run
//! against a real project fixture and the pinned built-in pack, so the canonical
//! authorities — the `sk-charter` kind vocabulary, the identity index, the constructed
//! YAML read, the model dump and the profile repository — are all exercised end to end.
//!
//! Not a whole-command parity campaign: the deferred deep proof is enumerated in
//! `crates/sk-doctrine/tests/cf26_context_include.rs` and in the lane report.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

const CHARTER_MD: &str = "# Fixture Charter\n\n\
## Terminology Canon\n\n\
Use Mission, never Feature.\n\n\
## Policy Summary\n\n\
- Keep one canonical authority.\n";

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, contents).expect("fixture write");
}

/// A project fixture the CLI can resolve a root and a doctrine bundle from.
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

#[test]
fn directive_selector_prints_the_artifact_verbatim() {
    let temp = fixture();
    let output = run(
        temp.path(),
        &[
            "charter",
            "context",
            "--include",
            "directive:025-boy-scout-rule",
        ],
    );
    assert!(
        output.status.success(),
        "exit {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    // The authored `id: DIRECTIVE_025` wins over the file stem in the heading, and the
    // body comes from the real built-in pack.
    assert!(
        text.starts_with("Directive DIRECTIVE_025: Boy Scout Rule\n"),
        "{text}"
    );
    assert!(text.contains("    Intent: "), "{text}");
    assert!(text.contains("    Full artifact:\n"), "{text}");
    assert!(text.contains("\"id\": \"DIRECTIVE_025\""), "{text}");
    // Bracketed doctrine prose is printed verbatim (no Rich-markup loss).
    assert!(!text.contains("schema_version:"), "{text}");
}

#[test]
fn json_envelope_matches_the_reference_include_payload() {
    let temp = fixture();
    let output = run(
        temp.path(),
        &[
            "charter",
            "context",
            "--include",
            "tactic:adversarial-qa-handoff",
            "--json",
        ],
    );
    assert!(output.status.success(), "{output:?}");
    let payload = json(&output);
    assert_eq!(payload["result"], "success");
    assert_eq!(payload["success"], true);
    assert_eq!(payload["include"], "tactic:adversarial-qa-handoff");
    assert_eq!(payload["context"], payload["text"]);
    let text = payload["text"].as_str().expect("text");
    assert!(
        text.starts_with("Tactic adversarial-qa-handoff: "),
        "{text}"
    );
    // `--json` never writes first-load state (the include path does not mark loaded).
    assert!(
        !temp
            .path()
            .join(".kittify/charter/context-state.json")
            .exists()
    );
}

#[test]
fn agent_profile_selector_resolves_through_the_profile_repository() {
    let temp = fixture();
    let output = run(
        temp.path(),
        &[
            "charter",
            "context",
            "--include",
            "agent-profile:implementer-ivan",
        ],
    );
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.starts_with("Agent profile implementer-ivan: Implementer Ivan\n"),
        "{text}"
    );
    assert!(text.contains("    Roles: implementer"), "{text}");
    assert!(
        text.contains("\"profile-id\": \"implementer-ivan\""),
        "{text}"
    );
}

#[test]
fn section_selector_keeps_its_rendering_and_envelope() {
    let temp = fixture();
    let text_output = run(
        temp.path(),
        &[
            "charter",
            "context",
            "--include",
            "section:terminology-canon",
        ],
    );
    assert!(text_output.status.success(), "{text_output:?}");
    let text = String::from_utf8_lossy(&text_output.stdout);
    assert!(text.contains("Use Mission, never Feature."), "{text}");

    let unknown = run(
        temp.path(),
        &["charter", "context", "--include", "section:no-such-section"],
    );
    assert_eq!(unknown.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&unknown.stderr)
            .contains("No charter section found for selector 'section:no-such-section'."),
        "{}",
        String::from_utf8_lossy(&unknown.stderr)
    );
}

#[test]
fn missing_member_and_malformed_selector_refuse_with_exit_one() {
    let temp = fixture();
    let missing = run(
        temp.path(),
        &[
            "charter",
            "context",
            "--include",
            "directive:no-such-directive",
        ],
    );
    assert_eq!(missing.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&missing.stderr)
            .contains("Error: No directive found for selector 'directive:no-such-directive'."),
        "{}",
        String::from_utf8_lossy(&missing.stderr)
    );

    let malformed = run(
        temp.path(),
        &["charter", "context", "--include", "directive"],
    );
    assert_eq!(malformed.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&malformed.stderr)
            .contains("Error: Expected --include selector in '<kind>:<id>' form."),
        "{}",
        String::from_utf8_lossy(&malformed.stderr)
    );
}

#[test]
fn template_selector_resolves_a_supported_template() {
    let temp = fixture();
    let output = run(
        temp.path(),
        &[
            "charter",
            "context",
            "--include",
            "template:software-dev/spec-template.md",
            "--json",
        ],
    );
    assert!(output.status.success(), "{output:?}");
    let payload = json(&output);
    assert_eq!(payload["result"], "success");
    assert_eq!(payload["success"], true);
    assert_eq!(payload["include"], "template:software-dev/spec-template.md");
    assert_eq!(payload["context"], payload["text"]);
    assert!(payload.get("code").is_none(), "{payload}");
    let text = payload["text"].as_str().expect("text");
    // The canonical six-tier resolver reached the packaged default and the
    // reference heading names the Source tier spelling.
    assert!(
        text.starts_with("Template software-dev/spec-template.md (tier: package_default):\n"),
        "{text}"
    );
    assert!(!text.ends_with('\n'), "{text}");
    assert!(text.len() > 100, "{text}");
    // The template text is the authored template, never raw pack YAML.
    assert!(!text.contains("schema_version:"), "{text}");

    // Text mode prints the same rendered selector verbatim.
    let text_output = run(
        temp.path(),
        &[
            "charter",
            "context",
            "--include",
            "template:software-dev/spec-template.md",
        ],
    );
    assert!(text_output.status.success(), "{text_output:?}");
    assert_eq!(
        String::from_utf8_lossy(&text_output.stdout),
        format!("{text}\n")
    );
}
