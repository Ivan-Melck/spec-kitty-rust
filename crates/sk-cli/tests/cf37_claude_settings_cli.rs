//! CF37 — CLI-crate wire for the Source JSON-backed Claude settings model.
//!
//! The model itself lives in `sk_install::session` and is covered by
//! `crates/sk-install/tests/cf37_claude_settings_source_json.rs`. This file
//! covers the CLI crate's own contract: the released `doctor tool-surfaces`
//! JSON shape must stay healthy for a Source-valid settings document, and the
//! two uncaught `json.loads` limit phases must still map to the ordinary
//! escaping-failure wire (exit 1, no report) instead of an invented verdict.
//!
//! Native-only functional evidence: no Source child process runs here.

use std::fs;
use std::path::Path;

use sk_cli::compat::doctor::tool_surfaces::session_surface_failure_error;
use sk_install::session::{SESSION_START_CMD, SESSION_STOP_CMD};
use sk_install::tool_surface_diagnostics::{
    SessionSurfacePhase, SurfaceAuditRequest, SurfaceKind, ToolSurfaceAuditFailure,
    audit_tool_surfaces,
};

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, contents).expect("fixture write");
}

fn configure(root: &Path, agents: &[&str]) {
    let list: Vec<String> = agents.iter().map(|key| format!("  - {key}")).collect();
    write(
        &root.join(".kittify/config.yaml"),
        &format!("agents:\n  available:\n{}\n", list.join("\n")),
    );
}

fn event(command: &str) -> String {
    format!("{{\"hooks\": [{{\"type\": \"command\", \"command\": \"{command}\"}}]}}")
}

#[test]
fn a_source_valid_settings_document_reports_present_hook_surfaces() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    configure(root, &["claude"]);
    write(
        &root.join(".claude/settings.json"),
        &format!(
            "{{\n  \"unrelated\": [NaN, Infinity, -Infinity],\n  \"hooks\": {{\n    \"SessionStart\": [{}],\n    \"Stop\": [{}]\n  }},\n  \"big\": {},\n  \"\\ud800\": \"\\udfff\"\n}}\n",
            event(SESSION_START_CMD),
            event(SESSION_STOP_CMD),
            "9".repeat(4000)
        ),
    );
    let configured = vec!["claude".to_string()];
    let report = audit_tool_surfaces(&SurfaceAuditRequest {
        project_root: root,
        configured_tools: &configured,
        tool_filter: None,
        kinds: Some(&[SurfaceKind::Hook]),
    })
    .expect("audit")
    .to_json();
    assert_eq!(report["ok"], serde_json::json!(true));
    let surfaces = report["surfaces"].as_array().expect("surfaces array");
    assert_eq!(surfaces.len(), 2, "one instance per lifecycle event");
    for surface in surfaces {
        assert_eq!(surface["provider"], serde_json::json!("session_presence"));
        assert_eq!(surface["state"], serde_json::json!("present"));
        assert_eq!(surface["tool"], serde_json::json!("claude"));
    }
    assert_eq!(report["findings"], serde_json::json!([]));
}

#[test]
fn an_uncaught_json_limit_keeps_the_escaping_failure_wire() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    configure(root, &["claude"]);
    write(
        &root.join(".claude/settings.json"),
        &format!(
            "{{\"hooks\": {{\"SessionStart\": [{}], \"Stop\": [{}]}}, \"big\": {}}}\n",
            event(SESSION_START_CMD),
            event(SESSION_STOP_CMD),
            "9".repeat(4301)
        ),
    );
    let configured = vec!["claude".to_string()];
    let failure = audit_tool_surfaces(&SurfaceAuditRequest {
        project_root: root,
        configured_tools: &configured,
        tool_filter: None,
        kinds: Some(&[SurfaceKind::Hook]),
    })
    .expect_err("the integer ceiling escapes the provider");
    let ToolSurfaceAuditFailure::Session(failure) = &failure else {
        panic!("expected the session failure carrier, got {failure:?}");
    };
    assert_eq!(failure.phase(), SessionSurfacePhase::JsonLimit);

    // The CLI mapping is unchanged: a domain refusal (exit 1, no report).
    let error = session_surface_failure_error(failure);
    assert!(error.message().contains("int_max_str_digits"));
    assert!(
        error.message().contains("settings.json"),
        "the failure names the inspected destination"
    );
    assert_eq!(error.to_json()["error"], serde_json::json!(error.message()));
}
