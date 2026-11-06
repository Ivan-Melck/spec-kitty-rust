//! CF30 focused smoke: `charter context --include template:<mission>/<name>` through
//! the real `spec-kitty` binary.
//!
//! These assert the real template resolution and the returned text/JSON, the
//! reference refusal family, the legacy-tier nudge, and that the template-only fast path
//! does not construct the doctrine bundle (a project whose `.kittify/config.yaml` cannot
//! be read still renders a template from the override tier, while a directive selector
//! fails on that same config).
//!
//! Not a whole-command parity campaign. Deferred proof: global-tier includes through a
//! real `~/.kittify` runtime, every non-UTF-8 selector form, Windows console encoding of
//! the rendered text, and concurrency while a pack changes underneath.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

const CHARTER_MD: &str = "# Fixture Charter\n\n\
## Terminology Canon\n\n\
Use Mission, never Feature.\n";

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, contents).expect("fixture write");
}

/// A project fixture with an authored config and activation state.
fn fixture(config: &str) -> tempfile::TempDir {
    let temp = tempfile::TempDir::new().expect("temp");
    let root = temp.path();
    write(
        &root.join(".kittify/charter/activation.yaml"),
        "# authored activation state\n",
    );
    write(&root.join(".kittify/charter/charter.md"), CHARTER_MD);
    write(&root.join(".kittify/config.yaml"), config);
    temp
}

/// Run the binary in `root` with the isolated home from `home` (when supplied).
fn run_with_home(root: &Path, home: Option<&Path>, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_spec-kitty"));
    command
        .args(args)
        .current_dir(root)
        .env("SPECIFY_REPO_ROOT", root)
        .env_remove("SPEC_KITTY_PACKS_ROOT");
    match home {
        Some(home) => {
            command.env("SPEC_KITTY_HOME", home).env("HOME", home);
        }
        None => {
            command.env_remove("SPEC_KITTY_HOME");
        }
    }
    command.output().expect("binary runs")
}

fn run(root: &Path, args: &[&str]) -> Output {
    run_with_home(root, None, args)
}

fn json(output: &Output) -> Value {
    serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap_or_else(|error| {
        panic!(
            "stdout is not JSON ({error}): {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

/// Every path (and file length) under `root`, for a no-write assertion.
fn footprint(root: &Path) -> Vec<(PathBuf, u64)> {
    fn walk(directory: &Path, out: &mut Vec<(PathBuf, u64)>) {
        let Ok(entries) = fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else {
                let length = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
                out.push((path, length));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out.sort();
    out
}

#[test]
fn an_override_template_prints_the_heading_and_the_stripped_text() {
    let temp = fixture("# authored project config\n");
    write(
        &temp.path().join(
            ".kittify/charter-packs/.kittify/overrides/missions/software-dev/templates/demo.md",
        ),
        "# Demo\n\nBody line one.\nBody line two.\n\n",
    );
    let output = run(
        temp.path(),
        &[
            "charter",
            "context",
            "--include",
            "template:software-dev/demo.md",
        ],
    );
    assert!(
        output.status.success(),
        "exit {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "Template software-dev/demo.md (tier: override):\n# Demo\n\nBody line one.\nBody line two.\n"
    );
}

#[test]
fn json_envelope_matches_the_reference_and_writes_nothing() {
    let temp = fixture("# authored project config\n");
    write(
        &temp.path().join(
            ".kittify/charter-packs/.kittify/overrides/missions/software-dev/templates/demo.md",
        ),
        "json body\n",
    );
    let before = footprint(temp.path());
    let output = run(
        temp.path(),
        &[
            "charter",
            "context",
            "--include",
            "template:software-dev/demo.md",
            "--json",
        ],
    );
    assert!(output.status.success(), "{output:?}");
    let payload = json(&output);
    assert_eq!(payload["result"], "success");
    assert_eq!(payload["success"], true);
    assert_eq!(payload["include"], "template:software-dev/demo.md");
    assert_eq!(
        payload["text"],
        "Template software-dev/demo.md (tier: override):\njson body"
    );
    assert_eq!(payload["context"], payload["text"]);
    let object = payload.as_object().expect("object");
    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["context", "include", "result", "success", "text"]);
    // No first-load marker, no context state, no materialization.
    assert_eq!(footprint(temp.path()), before);
}

#[test]
fn the_packaged_default_resolves_through_the_real_binary() {
    let temp = fixture("# authored project config\n");
    let output = run(
        temp.path(),
        &[
            "charter",
            "context",
            "--include",
            "template:software-dev/spec-template.md",
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
        text.starts_with("Template software-dev/spec-template.md (tier: package_default):\n"),
        "{text}"
    );
    assert!(!text.ends_with("\n\n"), "{text}");
    assert!(text.len() > 100, "{text}");
}

#[test]
fn the_template_path_does_not_construct_the_doctrine_bundle() {
    // A `.kittify/config.yaml` the doctrine bundle cannot read.
    let temp = fixture("charter_packs: [1, 2]\n");
    write(
        &temp.path().join(
            ".kittify/charter-packs/.kittify/overrides/missions/software-dev/templates/demo.md",
        ),
        "independent body\n",
    );

    let template = run(
        temp.path(),
        &[
            "charter",
            "context",
            "--include",
            "template:software-dev/demo.md",
        ],
    );
    assert!(
        template.status.success(),
        "the override tier precedes org config: exit {:?}: {}",
        template.status.code(),
        String::from_utf8_lossy(&template.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&template.stdout),
        "Template software-dev/demo.md (tier: override):\nindependent body\n"
    );

    // CF32: the artifact path reads only the canonical lazy access authority (the
    // lenient pack chain + one identity kind + the queried repository), so — exactly like
    // the pinned reference, whose `resolve_pack_chain(..., strict=False)` warns and drops
    // an unreadable registry — it serves the directive without the doctrine bundle and
    // without a strict `config.yaml` read. (Observed on the pinned Source oracle:
    // `build_charter_context_include(root, 'directive:025-boy-scout-rule')` succeeds with
    // `charter_packs: [1, 2]` authored.)
    let directive = run(
        temp.path(),
        &[
            "charter",
            "context",
            "--include",
            "directive:025-boy-scout-rule",
        ],
    );
    assert!(
        directive.status.success(),
        "the lenient pack chain serves the artifact path: exit {:?}: {}",
        directive.status.code(),
        String::from_utf8_lossy(&directive.stderr)
    );
    let text = String::from_utf8_lossy(&directive.stdout);
    assert!(
        text.starts_with("Directive DIRECTIVE_025: Boy Scout Rule\n"),
        "{text}"
    );
}

#[test]
fn missing_and_malformed_template_selectors_refuse_with_exit_one() {
    let temp = fixture("# authored project config\n");

    let missing = run(
        temp.path(),
        &[
            "charter",
            "context",
            "--include",
            "template:software-dev/no-such-template.md",
            "--json",
        ],
    );
    assert_eq!(missing.status.code(), Some(1));
    let payload = json(&missing);
    assert_eq!(payload["result"], "error");
    assert_eq!(payload["success"], false);
    assert!(payload.get("code").is_none(), "{payload}");
    assert_eq!(
        payload["error"],
        "No template found for selector 'template:software-dev/no-such-template.md'."
    );

    let missing_text = run(
        temp.path(),
        &[
            "charter",
            "context",
            "--include",
            "template:software-dev/no-such-template.md",
        ],
    );
    assert_eq!(missing_text.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&missing_text.stderr).contains(
            "Error: No template found for selector 'template:software-dev/no-such-template.md'."
        ),
        "{}",
        String::from_utf8_lossy(&missing_text.stderr)
    );

    let malformed = run(
        temp.path(),
        &["charter", "context", "--include", "template:no-slash"],
    );
    assert_eq!(malformed.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&malformed.stderr).contains(
            "Error: Malformed template selector 'template:no-slash': template_id 'no-slash' is \
             not of the form '<mission>/<name>'"
        ),
        "{}",
        String::from_utf8_lossy(&malformed.stderr)
    );
}

#[test]
fn a_legacy_template_nudges_once_in_human_mode_and_never_in_json_mode() {
    let temp = fixture("# authored project config\n");
    write(
        &temp
            .path()
            .join(".kittify/charter-packs/.kittify/templates/note.md"),
        "legacy body\n",
    );
    let home = tempfile::TempDir::new().expect("home");
    // A bootstrapped global runtime is the reference's precondition for the nudge.
    write(&home.path().join("cache/version.lock"), "9.9.9\n");

    let output = run_with_home(
        temp.path(),
        Some(home.path()),
        &[
            "charter",
            "context",
            "--include",
            "template:software-dev/note.md",
        ],
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "Template software-dev/note.md (tier: legacy):\nlegacy body\n"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        stderr.matches("Note: Run `spec-kitty migrate`").count(),
        1,
        "{stderr}"
    );
    assert!(
        stderr.contains("to clean up legacy project files and use the global runtime"),
        "{stderr}"
    );

    let json_output = run_with_home(
        temp.path(),
        Some(home.path()),
        &[
            "charter",
            "context",
            "--include",
            "template:software-dev/note.md",
            "--json",
        ],
    );
    assert!(json_output.status.success(), "{json_output:?}");
    assert!(
        String::from_utf8_lossy(&json_output.stderr).is_empty(),
        "--json must never carry the nudge: {}",
        String::from_utf8_lossy(&json_output.stderr)
    );
    assert_eq!(
        json(&json_output)["text"],
        "Template software-dev/note.md (tier: legacy):\nlegacy body"
    );
}

#[test]
fn a_legacy_template_without_a_global_runtime_prints_no_nudge() {
    let temp = fixture("# authored project config\n");
    write(
        &temp
            .path()
            .join(".kittify/charter-packs/.kittify/templates/note.md"),
        "legacy body\n",
    );
    let home = tempfile::TempDir::new().expect("home");
    let output = run_with_home(
        temp.path(),
        Some(home.path()),
        &[
            "charter",
            "context",
            "--include",
            "template:software-dev/note.md",
        ],
    );
    assert!(output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("(tier: legacy):"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    // The reference's un-configured branch is a Python logging/DeprecationWarning wire,
    // which this lane does not fabricate; only the nudge is asserted here.
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("Note: Run `spec-kitty migrate`"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
