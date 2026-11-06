//! Permanent Source-first differential for the thin-shim renderer family.
//!
//! `fixtures/shim_generator_oracle.json` was produced by the pinned Source
//! (`specify_cli.shims.generator.generate_all_shims` under `.venv-rust-oracle`,
//! CPython 3.11.15 / 53, sandboxed HOME/XDG/TMP/CWD) for a project configuring
//! `claude` and `gemini`. Both families (Markdown + TOML) are compared byte for
//! byte, plus a direct rendering call per family.

use std::path::PathBuf;

use serde_json::Value as Json;
use sk_install::render::shim::{
    CLI_DRIVEN_COMMANDS, generate_shim_content_for_agent, shim_file_name,
};

fn fixture() -> Json {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/shim_generator_oracle.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "shim generator oracle fixture is required but unreadable at {}: {error}",
            path.display()
        )
    });
    serde_json::from_str(&text).expect("fixture JSON")
}

#[test]
fn every_generated_shim_matches_the_pinned_source_bytes() {
    let fixture = fixture();
    let files = fixture
        .get("files")
        .and_then(Json::as_object)
        .expect("files");
    assert_eq!(files.len(), 12, "six commands x two agents");
    for agent in ["claude", "gemini"] {
        let directory = if agent == "claude" {
            ".claude/commands"
        } else {
            ".gemini/commands"
        };
        for command in CLI_DRIVEN_COMMANDS {
            let relative = format!("{directory}/{}", shim_file_name(command, agent));
            let expected = files
                .get(&relative)
                .and_then(Json::as_str)
                .unwrap_or_else(|| panic!("fixture is missing {relative}"));
            let actual = generate_shim_content_for_agent(command, agent, "4.0.0rc8")
                .unwrap_or_else(|error| panic!("render {relative}: {error}"));
            assert_eq!(actual, expected, "{relative} diverges from Source");
        }
    }
}

#[test]
fn the_recorded_direct_renderings_match() {
    let fixture = fixture();
    let markdown = fixture
        .get("markdown")
        .and_then(Json::as_str)
        .expect("markdown");
    let toml = fixture.get("toml").and_then(Json::as_str).expect("toml");
    assert_eq!(
        generate_shim_content_for_agent("implement", "claude", "4.0.0rc8").expect("render"),
        markdown
    );
    assert_eq!(
        generate_shim_content_for_agent("status", "gemini", "4.0.0rc8").expect("render"),
        toml
    );
}
