//! Real binary charter include error documents, not a Source paired campaign.
use std::fs;
use std::process::Command;

use serde_json::{Value, json};

#[test]
fn malformed_missing_and_section_refusals_use_the_charter_error_family() {
    let temp = tempfile::tempdir().expect("fixture");
    let root = temp.path();
    fs::create_dir_all(root.join(".kittify/charter")).unwrap();
    fs::write(
        root.join(".kittify/config.yaml"),
        "agents:\n  available: [codex]\n",
    )
    .unwrap();
    fs::write(
        root.join(".kittify/charter/charter.md"),
        "# Charter\n\n## Existing\n\nKeep this.\n",
    )
    .unwrap();
    for (selector, message) in [
        (
            "directive",
            "Expected --include selector in '<kind>:<id>' form.",
        ),
        (
            "directive:no-such-directive",
            "No directive found for selector 'directive:no-such-directive'.",
        ),
        (
            "section:no-such-section",
            "No charter section found for selector 'section:no-such-section'.",
        ),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_spec-kitty"))
            .current_dir(root)
            .env("SPECIFY_REPO_ROOT", root)
            .args(["charter", "context", "--include", selector, "--json"])
            .output()
            .expect("real binary");
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        let payload: Value = serde_json::from_slice(&output.stdout).expect("one JSON document");
        assert_eq!(
            payload,
            json!({"result": "error", "success": false, "error": message}),
            "selector={selector}"
        );
    }
    assert!(!root.join(".kittify/charter/context-state.json").exists());
    assert_eq!(
        fs::read_to_string(root.join(".kittify/config.yaml")).unwrap(),
        "agents:\n  available: [codex]\n"
    );
}
