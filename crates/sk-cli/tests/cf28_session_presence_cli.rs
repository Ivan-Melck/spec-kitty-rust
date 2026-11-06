//! CF28 — CLI-crate surface for the NEW session-presence probes.
//!
//! The read-only probes themselves live in `sk_install::session` +
//! `sk_install::tool_surface_diagnostics::session_presence` and are covered by
//! `crates/sk-install/tests/cf28_session_presence_probes.rs`. This file covers
//! the CLI crate's own contract around them: the `--kind` token table the
//! adapter resolves, and the released `doctor tool-surfaces --json` entry shape
//! for session-presence surfaces (provider/source_kind/kind/repair_command).
//!
//! Not covered here: root dispatch through the binary (it needs a process-global
//! CWD/HOME, which the workspace's `unsafe`-free tests cannot set; CF23 covered
//! the adapter's root-free contracts), Rich-rendered human output, and `--fix`
//! repair.

use std::fs;
use std::path::Path;

use sk_cli::compat::doctor::tool_surfaces::tool_surface_kinds;
use sk_install::session::SESSION_START_CMD;
use sk_install::tool_surface_diagnostics::{SurfaceAuditRequest, SurfaceKind, audit_tool_surfaces};
use sk_install::{COMPAT_CORE_VERSION, OrientationContent};

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

#[test]
fn the_cli_resolves_the_three_session_kind_tokens() {
    for (token, expected) in [
        ("context-file", SurfaceKind::ContextFile),
        ("context_file", SurfaceKind::ContextFile),
        ("rule", SurfaceKind::Rule),
        ("hook", SurfaceKind::Hook),
    ] {
        let selected = tool_surface_kinds(&[token.to_string()]).expect(token);
        assert_eq!(selected, vec![expected], "{token}");
    }
}

#[test]
fn the_session_presence_report_keeps_the_released_json_entry_shape() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    configure(root, &["claude"]);
    write(
        &root.join(".claude/CLAUDE.md"),
        &OrientationContent::healthy(COMPAT_CORE_VERSION, "demo").render(),
    );
    write(
        &root.join(".claude/settings.json"),
        &format!(
            "{{\n  \"hooks\": {{\n    \"SessionStart\": [\n      {{\"hooks\": [{{\"type\": \"command\", \"command\": \"{SESSION_START_CMD}\"}}]}}\n    ],\n    \"Stop\": [\n      {{\"hooks\": [{{\"type\": \"command\", \"command\": \"spec-kitty session-stop\"}}]}}\n    ]\n  }},\n  \"permissions\": {{\"allow\": [\"Bash\"]}}\n}}\n"
        ),
    );
    let configured = vec!["claude".to_string()];
    let report = audit_tool_surfaces(&SurfaceAuditRequest {
        project_root: root,
        configured_tools: &configured,
        tool_filter: None,
        kinds: Some(&[SurfaceKind::ContextFile, SurfaceKind::Hook]),
    })
    .expect("audit")
    .to_json();
    assert_eq!(report["ok"], serde_json::json!(true));
    assert_eq!(report["schema_version"], serde_json::json!(1));
    let surfaces = report["surfaces"].as_array().expect("surfaces array");
    assert_eq!(
        surfaces.len(),
        3,
        "one context file plus two hook instances"
    );
    for surface in surfaces {
        assert_eq!(surface["provider"], serde_json::json!("session_presence"));
        assert_eq!(surface["source_kind"], serde_json::json!("generated"));
        assert_eq!(surface["state"], serde_json::json!("present"));
        assert_eq!(surface["manifest"], serde_json::Value::Null);
        assert_eq!(surface["tool"], serde_json::json!("claude"));
    }
    assert_eq!(report["summary"]["present"], serde_json::json!(3));
    assert_eq!(report["findings"], serde_json::json!([]));
}

#[test]
fn a_missing_hook_surface_carries_the_hook_repair_command() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    configure(root, &["claude"]);
    let configured = vec!["claude".to_string()];
    let report = audit_tool_surfaces(&SurfaceAuditRequest {
        project_root: root,
        configured_tools: &configured,
        tool_filter: None,
        kinds: Some(&[SurfaceKind::Hook]),
    })
    .expect("audit")
    .to_json();
    assert_eq!(report["ok"], serde_json::json!(false));
    let finding = &report["findings"][0];
    assert_eq!(
        finding["code"],
        serde_json::json!("session-presence-incomplete")
    );
    assert_eq!(
        finding["repair_command"],
        serde_json::json!("spec-kitty doctor tool-surfaces --kind hook --fix")
    );
    assert_eq!(
        finding["surface_id"],
        serde_json::json!("claude.hook.settings.json")
    );
    assert_eq!(
        report["surfaces"][0]["repair_command"],
        finding["repair_command"]
    );
}
