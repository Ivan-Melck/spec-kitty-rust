//! CF8 focused smoke: the native `charter` adapters against a real project
//! fixture and the pinned built-in pack.
//!
//! These exercise the actual canonical authorities (context payload build,
//! lossless activation writes, cascade planners). They are not a whole-command
//! parity campaign: charter status, presets, the catalog compiler/synthesizer
//! and pack-skill projection remain pending and are reported as such.

use std::fs;
use std::path::Path;

use serde_json::Value;
use sk_cli::compat::charter;

const CHARTER_MD: &str = "# Fixture Charter\n\n\
## Policy Summary\n\n\
- Keep one canonical authority.\n\n\
## Terminology Canon\n\n\
Use Mission, never Feature.\n\n\
## Code Review Checklist\n\n\
- tests added\n\n\
## Regression Vigilance (2026-04-06)\n\n\
- grep the old term\n";

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, contents).expect("fixture write");
}

/// A project fixture whose activation ledger already carries authored state.
fn fixture() -> tempfile::TempDir {
    let temp = tempfile::TempDir::new().expect("temp");
    let root = temp.path();
    write(
        &root.join(".kittify/charter/activation.yaml"),
        "# authored activation state\n\
         activated_directives:\n\
         - 001-architectural-integrity-standard\n\
         deactivated_tactics:\n\
         - adversarial-qa-handoff\n",
    );
    write(&root.join(".kittify/charter/charter.md"), CHARTER_MD);
    // `charter context` runs the Source bundle-freshness gate before it reads the
    // prose, so the fixture must be a real Git project (the reference refuses a
    // root it cannot resolve) with a *fresh* bundle: the recorded digest matches
    // the authored prose, so no encoding-provenance read is appended either.
    write(
        &root.join(".kittify/charter/metadata.yaml"),
        &format!(
            "charter_hash: {}\ntimestamp_utc: '2026-01-01T00:00:00+00:00'\n",
            sk_charter::hash_content(CHARTER_MD)
        ),
    );
    let status = std::process::Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .arg(root)
        .status()
        .expect("git is available");
    assert!(status.success(), "fixture git init failed");
    write(
        &root.join(".kittify/config.yaml"),
        "# authored project config\n\
         custom_setting: keep-me\n\
         charter_packs:\n\
         \x20 project:\n\
         \x20   enabled: true\n",
    );
    temp
}

fn kinds(payload: &Value, kind: &str) -> Value {
    payload["kinds"]
        .as_array()
        .expect("kinds")
        .iter()
        .find(|row| row["kind"] == kind)
        .cloned()
        .expect("row")
}

#[test]
fn context_payload_carries_the_contract_and_the_rendered_text() {
    let temp = fixture();
    let outcome =
        charter::context_payload(temp.path(), "implement", Some("software-dev"), None, false)
            .expect("context");
    assert_eq!(outcome.payload["result"], "success");
    assert_eq!(outcome.payload["success"], true);
    assert_eq!(outcome.payload["action"], "implement");
    assert_eq!(outcome.payload["mode"], "bootstrap");
    assert_eq!(outcome.payload["first_load"], true);
    assert_eq!(
        outcome.payload["context_schema_version"],
        sk_doctrine::CONTEXT_SCHEMA_VERSION
    );
    for key in [
        "directives",
        "all_directives",
        "diagnostics",
        "directives_source",
        "tactics",
        "styleguides",
        "toolguides",
        "references",
        "governance_references",
        "project_charter",
        "org_charter",
        "references_count",
        "context",
        "text",
    ] {
        assert!(outcome.payload.get(key).is_some(), "missing key {key}");
    }
    assert_eq!(outcome.payload["context"], outcome.payload["text"]);
    let text = outcome
        .text
        .to_scalar_string()
        .expect("this fixture has finite text");
    assert!(text.starts_with("Charter Context (Bootstrap):"));
    assert!(text.contains("  - Keep one canonical authority."));
    assert!(text.contains("Action-Critical Charter Sections (implement):"));
    assert!(text.contains("### Terminology Canon"));
    assert!(text.contains("When you rename or introduce a term in the diff"));
    assert!(text.contains("Action Doctrine (implement):"), "{text}");
}

#[test]
fn list_reads_the_activation_ledger() {
    let temp = fixture();
    let payload = charter::list_payload(temp.path(), false, false).expect("list");
    assert_eq!(payload["result"], "success");
    assert!(payload["templates"].is_null());
    let directives = kinds(&payload, "directive");
    assert_eq!(
        directives["activated"],
        serde_json::json!(["001-architectural-integrity-standard"])
    );
    assert_eq!(directives["available"], Value::Null);
    let tactics = kinds(&payload, "tactic");
    assert_eq!(
        tactics["deactivated"],
        serde_json::json!(["adversarial-qa-handoff"])
    );
    let mission_types = kinds(&payload, "mission-type");
    assert!(mission_types["deactivated"].is_null());

    let layered = charter::list_payload(temp.path(), false, true).expect("list all");
    let directives = kinds(&layered, "directive");
    let available = directives["available"].as_array().expect("available");
    assert!(available.iter().any(|entry| {
        entry["artifact_id"] == "024-locality-of-change"
            && entry["layer"] == "built-in"
            && entry["declared_id"] == "DIRECTIVE_024"
    }));
}

#[test]
fn activation_is_idempotent_and_preserves_authored_state() {
    let temp = fixture();
    let root = temp.path();
    let ledger_path = root.join(".kittify/charter/activation.yaml");
    let config_path = root.join(".kittify/config.yaml");
    let config_before = fs::read(&config_path).expect("config before");

    let first = charter::activate(root, "directive", "DIRECTIVE_024", false).expect("activate");
    assert_eq!(first.changed, vec!["024-locality-of-change".to_owned()]);

    let after_first = fs::read(&ledger_path).expect("ledger after first");
    let text = String::from_utf8(after_first.clone()).expect("utf8");
    assert!(text.contains("# authored activation state"));
    assert!(text.contains("024-locality-of-change"));
    assert!(text.contains("001-architectural-integrity-standard"));
    // An unrelated authored project setting is untouched: the write targets
    // the activation ledger, never the project config.
    assert_eq!(fs::read(&config_path).expect("config after"), config_before);

    let second = charter::activate(root, "directive", "DIRECTIVE_024", false).expect("re-activate");
    assert!(second.changed.is_empty());
    assert!(
        second
            .warnings
            .iter()
            .any(|warning| warning.contains("already activated")),
        "{:?}",
        second.warnings
    );
    assert_eq!(
        fs::read(&ledger_path).expect("ledger after second"),
        after_first,
        "an idempotent retry must not rewrite the authored document"
    );

    let listed = charter::list_payload(root, false, false).expect("list");
    let activated = kinds(&listed, "directive")["activated"].clone();
    assert!(
        activated
            .as_array()
            .expect("activated")
            .iter()
            .any(|id| id == "024-locality-of-change")
    );
}

#[test]
fn deactivation_records_an_exclusion_and_is_idempotent() {
    let temp = fixture();
    let root = temp.path();
    let ledger_path = root.join(".kittify/charter/activation.yaml");
    charter::activate(root, "directive", "DIRECTIVE_024", false).expect("activate");
    let before = fs::read(&ledger_path).expect("ledger before deactivate");

    let first = charter::deactivate(root, "directive", "DIRECTIVE_024").expect("deactivate");
    assert_eq!(first.changed, vec!["024-locality-of-change".to_owned()]);
    let after_first = fs::read(&ledger_path).expect("ledger after deactivate");
    let text = String::from_utf8(after_first.clone()).expect("utf8");
    assert!(text.contains("deactivated_directives"));
    assert!(text.contains("024-locality-of-change"));
    assert_ne!(before, after_first);

    let second = charter::deactivate(root, "directive", "DIRECTIVE_024").expect("re-deactivate");
    assert!(second.changed.is_empty());
    assert!(
        second
            .warnings
            .iter()
            .any(|warning| warning.contains("already deactivated")),
        "{:?}",
        second.warnings
    );
    assert_eq!(
        fs::read(&ledger_path).expect("ledger after second"),
        after_first
    );

    let listed = charter::list_payload(root, false, false).expect("list");
    let directives = kinds(&listed, "directive");
    assert!(
        directives["deactivated"]
            .as_array()
            .expect("deactivated")
            .iter()
            .any(|id| id == "024-locality-of-change")
    );
}

#[test]
fn cascade_planners_run_over_the_merged_graph() {
    let temp = fixture();
    let root = temp.path();
    charter::activate(root, "directive", "DIRECTIVE_024", false).expect("activate");
    let cascade = charter::cascade_activation_after(
        root,
        "directive",
        "DIRECTIVE_024",
        Some("tactic"),
        false,
    )
    .expect("cascade report")
    .expect("scoped cascade");
    assert_eq!(cascade["scope"], serde_json::json!(["tactic"]));
    for key in [
        "cascaded",
        "skipped_out_of_scope",
        "skipped_deactivated",
        "unreached_behind_deactivated",
        "skipped_kind_filtered",
    ] {
        assert!(cascade[key].is_array(), "missing cascade key {key}");
    }
    assert!(
        cascade["source_urn"]
            .as_str()
            .unwrap_or_default()
            .starts_with("directive:")
    );

    let deactivation =
        charter::cascade_deactivation_after(root, "directive", "DIRECTIVE_024", Some("tactic"))
            .expect("deactivation report")
            .expect("scoped cascade");
    for key in ["cascaded", "skipped_shared", "skipped_kind_filtered"] {
        assert!(deactivation[key].is_array(), "missing key {key}");
    }
}

#[test]
fn unknown_kinds_are_refused_before_any_write() {
    let temp = fixture();
    let root = temp.path();
    let ledger_path = root.join(".kittify/charter/activation.yaml");
    let before = fs::read(&ledger_path).expect("ledger");
    let error = charter::activate(root, "not-a-kind", "x", false).expect_err("refusal");
    assert_eq!(
        error.to_json()["error_code"],
        serde_json::json!("ACTIVATION_KIND_UNKNOWN")
    );
    assert_eq!(fs::read(&ledger_path).expect("ledger after"), before);
}
