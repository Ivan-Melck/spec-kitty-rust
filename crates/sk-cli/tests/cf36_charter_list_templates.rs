//! CF36 focused smoke: `charter list --all` publishes the Source-shaped template
//! availability array and the human `template` row, discovered through the canonical
//! CF30 catalog from real project/org/package tiers.
//!
//! These are not a whole-command parity campaign. Deferred proof is listed in the lane
//! report: a real `~/.kittify` global tier (the reference listing does not enumerate one,
//! so this lane deliberately has none), non-UTF-8 mission/template names, the Rich table
//! geometry and Windows console encoding of the row.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use sk_cli::compat::charter;

const CHARTER_MD: &str = "# Fixture Charter\n\n\
## Terminology Canon\n\n\
Use Mission, never Feature.\n";

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, contents).expect("fixture write");
}

fn packs_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packs/built-in")
        .canonicalize()
        .expect("built-in pack tree")
}

/// A project with a project pack and two declared, existing org packs.
///
/// The fixture deliberately declares no absent pack: the Native `charter list`
/// non-template rows (foreign, CF8-owned) resolve through `Roots::load` and refuse a
/// declared-but-unfetched org pack before any template work happens, while the pinned
/// Source listing drops it and degrades its required-floor read. That divergence is not
/// this lane's to fix; the lenient drop is covered where it IS this lane's code
/// (`list_templates` unit tests), and the exact integration request is in the lane
/// report.
fn fixture() -> tempfile::TempDir {
    let temp = tempfile::TempDir::new().expect("temp");
    let root = temp.path();
    write(
        &root.join(".kittify/config.yaml"),
        "charter_packs:\n  org:\n    packs:\n\
         \x20     - name: org-first\n        local_path: org-first\n\
         \x20     - name: org-second\n        local_path: org-second\n",
    );
    write(&root.join(".kittify/charter/charter.md"), CHARTER_MD);
    write(
        &root.join(".kittify/charter/activation.yaml"),
        "# authored\n",
    );
    // The project layer outranks the package for the same mission-qualified id.
    write(
        &root.join(".kittify/charter-packs/missions/software-dev/templates/spec-template.md"),
        "project spec\n",
    );
    write(
        &root.join(".kittify/charter-packs/missions/project-only/templates/only.md"),
        "project only\n",
    );
    // The same mission-qualified id in two org packs: the FIRST declared pack wins.
    write(
        &root.join("org-first/missions/shared/templates/same.md"),
        "org-first shared\n",
    );
    write(
        &root.join("org-second/missions/shared/templates/same.md"),
        "org-second shared\n",
    );
    write(
        &root.join("org-second/missions/beta/command-templates/implement.md"),
        "org command\n",
    );
    write(
        &root.join("org-second/missions/beta/templates/README.md"),
        "not a template\n",
    );
    write(
        &root.join("org-second/missions/beta/templates/nested/deep.md"),
        "nested\n",
    );
    temp
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_spec-kitty"))
        .current_dir(root)
        .env("HOME", root.join(".fixture-home"))
        .env("SPEC_KITTY_PACKS_ROOT", packs_root())
        .env_remove("SPECIFY_REPO_ROOT")
        .args(args)
        .output()
        .expect("binary runs")
}

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout is not JSON ({error}): {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn row<'a>(payload: &'a Value, template_id: &str) -> &'a Value {
    payload["templates"]
        .as_array()
        .expect("templates array")
        .iter()
        .find(|row| row["template_id"] == template_id)
        .unwrap_or_else(|| panic!("missing template row {template_id}: {payload:#}"))
}

/// Every fixture file's bytes, size, mode and modification time.
fn footprint(root: &Path) -> BTreeMap<PathBuf, (Vec<u8>, u64, u32, i64)> {
    let mut entries = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(&directory).expect("read dir") {
            let path = entry.expect("dir entry").path();
            let metadata = fs::symlink_metadata(&path).expect("metadata");
            if metadata.is_dir() {
                stack.push(path);
                continue;
            }
            let modified = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|delta| delta.as_nanos() as i64)
                .unwrap_or(-1);
            #[cfg(unix)]
            let mode = {
                use std::os::unix::fs::MetadataExt;
                metadata.mode()
            };
            #[cfg(not(unix))]
            let mode = 0;
            entries.insert(
                path.clone(),
                (
                    fs::read(&path).expect("bytes"),
                    metadata.len(),
                    mode,
                    modified,
                ),
            );
        }
    }
    entries
}

#[test]
fn templates_is_explicit_null_without_all_even_with_show_available() {
    let temp = fixture();
    let plain = charter::list_payload(temp.path(), false, false).expect("list");
    assert_eq!(plain["result"], "success");
    assert!(
        plain.get("templates").is_some(),
        "the templates key must be present, not absent"
    );
    assert!(plain["templates"].is_null());

    let available_only =
        charter::list_payload(temp.path(), true, false).expect("list show-available");
    assert!(available_only["templates"].is_null());
    // `--show-available` keeps its own (non-layer) availability rows.
    let directives = available_only["kinds"]
        .as_array()
        .expect("kinds")
        .iter()
        .find(|row| row["kind"] == "directive")
        .expect("directive row");
    assert!(directives["available"].is_array());
}

#[test]
fn all_publishes_the_source_shaped_template_rows() {
    let temp = fixture();
    let payload = charter::list_payload(temp.path(), false, true).expect("list --all");
    let templates = payload["templates"].as_array().expect("templates array");
    assert!(!templates.is_empty());

    // The four reference keys, and nothing else.
    for row in templates {
        let object = row.as_object().expect("row object");
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["mission", "name", "template_id", "tier"]);
    }

    // A project-layer template is an `override`.
    let only = row(&payload, "project-only/only.md");
    assert_eq!(only["mission"], "project-only");
    assert_eq!(only["name"], "only.md");
    assert_eq!(only["tier"], "override");

    // The project layer wins the duplicate mission-qualified id against the package.
    assert_eq!(
        row(&payload, "software-dev/spec-template.md")["tier"],
        "override"
    );

    // The org chain wins against the package and deduplicates across org packs.
    assert_eq!(row(&payload, "shared/same.md")["tier"], "org");
    let shared = templates
        .iter()
        .filter(|row| row["template_id"] == "shared/same.md")
        .count();
    assert_eq!(shared, 1, "one row per mission-qualified id");

    // Command templates are discovered and recursively nested templates flatten to the
    // file name, exactly like the reference catalog.
    assert_eq!(row(&payload, "beta/implement.md")["tier"], "org");
    assert_eq!(row(&payload, "beta/deep.md")["tier"], "org");
    assert!(
        !templates.iter().any(|row| row["template_id"]
            .as_str()
            .unwrap_or_default()
            .ends_with("README.md")),
        "README.md is not a template"
    );

    // Package-default rows come from the installed built-in pack.
    assert_eq!(
        row(&payload, "software-dev/plan-template.md")["tier"],
        "package_default"
    );
    assert_eq!(
        row(&payload, "software-dev/plan-template.md")["mission"],
        "software-dev"
    );
    assert_eq!(
        row(&payload, "software-dev/plan-template.md")["name"],
        "plan-template.md"
    );

    // Tiers are the Source enum value spellings; the listing enumerates no global tier.
    for row in templates {
        let tier = row["tier"].as_str().expect("tier string");
        assert!(
            ["override", "org", "package_default"].contains(&tier),
            "unexpected tier {tier}"
        );
    }

    // Rows are sorted by (template_id, tier) like the reference.
    let mut sorted: Vec<(String, String)> = templates
        .iter()
        .map(|row| {
            (
                row["template_id"].as_str().unwrap_or_default().to_owned(),
                row["tier"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    let observed = sorted.clone();
    sorted.sort();
    assert_eq!(observed, sorted);
}

#[test]
fn the_real_binary_matches_the_payload_and_writes_nothing() {
    let temp = fixture();
    let before = footprint(temp.path());
    let in_process = charter::list_payload(temp.path(), false, true).expect("list --all");

    let output = run(temp.path(), &["charter", "list", "--all", "--json"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload = stdout_json(&output);
    assert_eq!(payload["result"], "success");
    assert_eq!(
        payload["templates"], in_process["templates"],
        "the binary and the in-process payload discover the same templates"
    );

    // Without `--all` the same binary keeps the explicit null.
    let plain = run(temp.path(), &["charter", "list", "--json"]);
    assert!(plain.status.success());
    assert!(stdout_json(&plain)["templates"].is_null());

    // The human `--all` view carries the template row and its availability line.
    let text = run(temp.path(), &["charter", "list", "--all"]);
    assert!(text.status.success());
    let text = String::from_utf8(text.stdout).expect("stdout is UTF-8");
    assert!(
        text.contains("template: (mission-scoped — not separately activated)"),
        "{text}"
    );
    assert!(text.contains("  available: beta/deep.md (org)"), "{text}");
    assert!(text.contains("project-only/only.md (override)"), "{text}");
    assert!(
        text.contains("software-dev/spec-template.md (override)"),
        "{text}"
    );

    // A read-only listing leaves every fixture file byte-identical.
    assert_eq!(
        before,
        footprint(temp.path()),
        "charter list wrote to the project"
    );
}
