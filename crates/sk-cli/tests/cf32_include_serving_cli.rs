//! CF32 focused smoke: merged / annotated non-template include serving through the real
//! `spec-kitty` binary.
//!
//! These assert the operator-visible contract of the new serving path: an overlay record
//! is rendered merged, a denied selector is annotated (exit 0, never refused), the `--json`
//! envelope carries the same text, and the read writes nothing into the project.
//!
//! Not a whole-command parity campaign: the doctrine-level matrix lives in
//! `crates/sk-doctrine/tests/cf32_effective_include.rs`, and the deferred deep proof is
//! enumerated in the lane report.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

/// A real built-in tactic that ships in the pinned pack.
const TACTIC: &str = "moscow-scoping-lens";
const OVERLAY_NAME: &str = "Project Overlay Name";

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, contents).expect("fixture write");
}

/// A project fixture the binary can resolve a root and a pack from.
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

/// Every file below `root` with a digest of its bytes, for the no-write assertion.
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
                out.insert(
                    path.strip_prefix(directory.parent().unwrap_or(directory))
                        .unwrap_or(&path)
                        .to_path_buf(),
                    fs::read(&path).unwrap_or_default(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, &mut out);
    out
}

#[test]
fn a_project_overlay_renders_the_merged_record() {
    let temp = fixture();
    write(
        &temp.path().join(format!(
            ".kittify/charter-packs/tactic/{TACTIC}.tactic.yaml"
        )),
        &format!("id: {TACTIC}\nname: {OVERLAY_NAME}\n"),
    );
    let output = run(
        temp.path(),
        &[
            "charter",
            "context",
            "--include",
            &format!("tactic:{TACTIC}"),
        ],
    );
    assert!(
        output.status.success(),
        "exit {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.starts_with(&format!("Tactic {TACTIC}: {OVERLAY_NAME}\n")),
        "{text}"
    );
    // The built-in fields are inherited through the merge, not dropped.
    assert!(text.contains("    Purpose: "), "{text}");
    assert!(text.contains("    Steps:\n"), "{text}");
    assert!(text.contains("    Full artifact:\n"), "{text}");
    assert!(!text.contains("schema_version:"), "no raw YAML: {text}");
}

#[test]
fn a_denied_selector_is_annotated_and_never_refused() {
    let temp = fixture();
    write(
        &temp.path().join(".kittify/charter/activation.yaml"),
        &format!("deactivated_tactics:\n  - {TACTIC}\n"),
    );

    let text_output = run(
        temp.path(),
        &[
            "charter",
            "context",
            "--include",
            &format!("tactic:{TACTIC}"),
        ],
    );
    assert!(
        text_output.status.success(),
        "exit {:?}: {}",
        text_output.status.code(),
        String::from_utf8_lossy(&text_output.stderr)
    );
    let expected = format!(
        "tactic:{TACTIC} is deactivated in this project \
         (charter activate tactic {TACTIC} restores it)"
    );
    assert_eq!(
        String::from_utf8_lossy(&text_output.stdout),
        format!("{expected}\n")
    );

    // The generic probe is annotated too, naming the denied kind's restore token.
    let probe = run(
        temp.path(),
        &[
            "charter",
            "context",
            "--include",
            &format!("artifact:{TACTIC}"),
        ],
    );
    assert!(probe.status.success(), "{probe:?}");
    assert_eq!(
        String::from_utf8_lossy(&probe.stdout),
        format!(
            "artifact:{TACTIC} is deactivated in this project \
             (charter activate tactic {TACTIC} restores it)\n"
        )
    );

    // `--json` carries the same text and is not an error envelope.
    let envelope = run(
        temp.path(),
        &[
            "charter",
            "context",
            "--include",
            &format!("tactic:{TACTIC}"),
            "--json",
        ],
    );
    assert!(envelope.status.success(), "{envelope:?}");
    let payload = json(&envelope);
    assert_eq!(payload["result"], "success");
    assert_eq!(payload["success"], true);
    assert_eq!(payload["context"], Value::String(expected.clone()));
    assert_eq!(payload["text"], Value::String(expected));
    assert!(payload.get("code").is_none(), "{payload}");
}

#[test]
fn the_serving_path_writes_nothing_into_the_project() {
    let temp = fixture();
    write(
        &temp.path().join(format!(
            ".kittify/charter-packs/tactic/{TACTIC}.tactic.yaml"
        )),
        &format!("id: {TACTIC}\nname: {OVERLAY_NAME}\n"),
    );
    let before = snapshot(temp.path());
    for args in [
        vec![
            "charter",
            "context",
            "--include",
            &format!("tactic:{TACTIC}"),
        ],
        vec![
            "charter",
            "context",
            "--include",
            "agent-profile:implementer-ivan",
        ],
        vec![
            "charter",
            "context",
            "--include",
            "template:software-dev/spec-template.md",
        ],
    ] {
        let output = run(temp.path(), &args);
        assert!(
            output.status.success(),
            "{args:?} exit {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let after = snapshot(temp.path());
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        after.keys().collect::<Vec<_>>(),
        "the include path created or removed files"
    );
    assert_eq!(before, after, "the include path changed authored bytes");
    assert!(
        !temp
            .path()
            .join(".kittify/charter/context-state.json")
            .exists(),
        "an include must not record first-load state"
    );
}
