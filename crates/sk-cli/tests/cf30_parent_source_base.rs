//! Actual Source caller-base controls, not paired child conformance.
use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::Value;
use sk_charter::template_catalog::{
    TemplateTier, render_source_template_include, render_template_include,
};

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn run(root: &Path) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_spec-kitty"))
        .current_dir(root)
        .env("SPECIFY_REPO_ROOT", root)
        .env("HOME", root.join("home"))
        .env("SPEC_KITTY_HOME", root.join("home"))
        .args([
            "charter",
            "context",
            "--include",
            "template:software-dev/spec-template.md",
            "--json",
        ])
        .output()
        .expect("real binary");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let payload: Value = serde_json::from_slice(&output.stdout).expect("one JSON document");
    assert_eq!(payload["result"], "success");
    assert_eq!(payload["success"], true);
    assert_eq!(payload["context"], payload["text"]);
    payload["text"].as_str().unwrap().to_owned()
}

#[test]
fn source_include_ignores_ordinary_repository_override_but_explicit_api_keeps_it() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        &root.join(".kittify/config.yaml"),
        "agents:\n  available: [codex]\n",
    );
    write(
        &root.join(".kittify/overrides/missions/software-dev/templates/spec-template.md"),
        "EXPLICIT ROOT BODY\n",
    );
    assert_eq!(sk_charter::resolve_source_project_layer(root), None);

    let explicit = render_template_include(
        root,
        "software-dev/spec-template.md",
        "template:software-dev/spec-template.md",
    )
    .unwrap();
    assert_eq!(explicit.tier, TemplateTier::Override);
    assert!(explicit.text.ends_with("EXPLICIT ROOT BODY"));
    let source = render_source_template_include(
        root,
        "software-dev/spec-template.md",
        "template:software-dev/spec-template.md",
    )
    .unwrap();
    assert_eq!(source.tier, TemplateTier::PackageDefault);
    assert!(!source.text.contains("EXPLICIT ROOT BODY"));
    let text = run(root);
    assert!(text.starts_with("Template software-dev/spec-template.md (tier: package_default):\n"));
    assert!(!text.contains("EXPLICIT ROOT BODY"));
}

#[test]
fn source_layer_directory_presence_and_legacy_fallback_choose_the_actual_base() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        &root.join(".kittify/config.yaml"),
        "agents:\n  available: [codex]\n",
    );
    let fallback = root.join("src/charter/offering");
    write(
        &fallback.join(".kittify/overrides/missions/software-dev/templates/spec-template.md"),
        "FALLBACK LAYER BODY\n",
    );
    assert_eq!(
        sk_charter::resolve_source_project_layer(root),
        Some(fallback)
    );
    assert_eq!(
        run(root),
        "Template software-dev/spec-template.md (tier: override):\nFALLBACK LAYER BODY"
    );

    let primary = root.join(".kittify/charter-packs");
    fs::create_dir_all(&primary).unwrap();
    assert_eq!(
        sk_charter::resolve_source_project_layer(root),
        Some(primary.clone())
    );
    let text = run(root);
    assert!(text.contains("(tier: package_default):"), "{text}");
    assert!(
        !text.contains("FALLBACK LAYER BODY"),
        "an EMPTY primary layer still wins"
    );

    write(
        &primary.join(".kittify/overrides/missions/software-dev/templates/spec-template.md"),
        "PRIMARY LAYER BODY\n",
    );
    assert_eq!(
        run(root),
        "Template software-dev/spec-template.md (tier: override):\nPRIMARY LAYER BODY"
    );
}
