//! CF33 — CLI contract for the escaping session decode/read failure.
//!
//! `doctor tool-surfaces` is the one leaf whose provider can raise something the
//! reference never catches: `UnicodeDecodeError` from an orientation file and
//! `OSError` from `.claude/settings.json`. Upstream the exception escapes,
//! nothing is printed but the traceback, and the process exits 1. This file
//! pins the Native side of that contract at the CLI crate's own seam:
//!
//! * a Source-valid UTF-16/32-BOM settings file now produces the released
//!   `present` report (exit 0), not an invented `unsafe`/`missing` verdict;
//! * an escaping failure maps to a non-zero, report-free command result with the
//!   reference's own CPython/OS message.
//!
//! Not covered: running the binary (process-global CWD/HOME) and the Python
//! traceback wire itself — both are recorded as debt in the lane report.

use std::fs;
use std::path::Path;

use sk_cli::compat::doctor::tool_surfaces::session_surface_failure_error;
use sk_install::session::{SESSION_START_CMD, SESSION_STOP_CMD};
use sk_install::tool_surface_diagnostics::{
    SessionSurfacePhase, SurfaceAuditRequest, SurfaceKind, ToolSurfaceAuditFailure,
    audit_tool_surfaces,
};

fn write_bytes(path: &Path, contents: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, contents).expect("fixture write");
}

fn write(path: &Path, contents: &str) {
    write_bytes(path, contents.as_bytes());
}

fn configure(root: &Path, agents: &[&str]) {
    let list: Vec<String> = agents.iter().map(|key| format!("  - {key}")).collect();
    write(
        &root.join(".kittify/config.yaml"),
        &format!("agents:\n  available:\n{}\n", list.join("\n")),
    );
}

fn both_hooks_json() -> String {
    format!(
        "{{\n  \"hooks\": {{\n    \"SessionStart\": [{{\"hooks\": [{{\"type\": \"command\", \"command\": \"{SESSION_START_CMD}\"}}]}}],\n    \"Stop\": [{{\"hooks\": [{{\"type\": \"command\", \"command\": \"{SESSION_STOP_CMD}\"}}]}}]\n  }}\n}}\n"
    )
}

fn utf16_bom_le(text: &str) -> Vec<u8> {
    let mut bytes = vec![0xff, 0xfe];
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes
}

fn utf32_bom_le(text: &str) -> Vec<u8> {
    let mut bytes = vec![0xff, 0xfe, 0x00, 0x00];
    for character in text.chars() {
        bytes.extend_from_slice(&(character as u32).to_le_bytes());
    }
    bytes
}

fn audit(
    root: &Path,
    tools: &[&str],
    kinds: &[SurfaceKind],
) -> Result<sk_install::tool_surface_diagnostics::SurfaceReport, ToolSurfaceAuditFailure> {
    let configured: Vec<String> = tools.iter().map(|key| (*key).to_string()).collect();
    audit_tool_surfaces(&SurfaceAuditRequest {
        project_root: root,
        configured_tools: &configured,
        tool_filter: None,
        kinds: Some(kinds),
    })
}

#[test]
fn source_valid_utf16_and_utf32_settings_report_present_over_the_cli_seam() {
    for encoded in [
        utf16_bom_le(&both_hooks_json()),
        utf32_bom_le(&both_hooks_json()),
    ] {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();
        configure(root, &["claude"]);
        write_bytes(&root.join(".claude/settings.json"), &encoded);
        let report = audit(root, &["claude"], &[SurfaceKind::Hook])
            .expect("audit")
            .to_json();
        assert_eq!(report["ok"], serde_json::json!(true));
        assert_eq!(report["schema_version"], serde_json::json!(1));
        assert_eq!(report["summary"]["present"], serde_json::json!(2));
        assert_eq!(report["summary"]["errors"], serde_json::json!(0));
        assert_eq!(report["findings"], serde_json::json!([]));
        for surface in report["surfaces"].as_array().expect("surfaces") {
            assert_eq!(surface["state"], serde_json::json!("present"));
            assert_eq!(surface["provider"], serde_json::json!("session_presence"));
            assert_eq!(surface["repair_command"], serde_json::Value::Null);
        }
    }
}

#[test]
fn an_escaping_orientation_decode_maps_to_a_nonzero_report_free_error() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    configure(root, &["codex"]);
    write_bytes(&root.join("AGENTS.md"), b"caf\xe9\n");
    let failure = match audit(root, &["codex"], &[SurfaceKind::ContextFile]) {
        Ok(report) => panic!("expected an escaping failure, got {report:?}"),
        Err(ToolSurfaceAuditFailure::Session(session)) => session,
        Err(other) => panic!("expected the session failure, got {other}"),
    };
    assert_eq!(failure.phase(), SessionSurfacePhase::Decode);
    let error = session_surface_failure_error(&failure);
    assert_eq!(error.exit(), 1, "Source exits 1 when the exception escapes");
    assert_eq!(
        error.message(),
        format!(
            "{}: 'utf-8' codec can't decode byte 0xe9 in position 3: invalid continuation byte",
            root.join("AGENTS.md").display()
        )
    );
    // No health report is fabricated: the rendered document is an error object.
    let json = error.to_json();
    assert!(json.get("ok").is_none(), "{json}");
    assert_eq!(json["error"], serde_json::json!(error.message()));
    assert!(json.get("surfaces").is_none());
    assert!(json.get("summary").is_none());
}

#[test]
fn an_escaping_settings_read_maps_to_a_nonzero_report_free_error() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    configure(root, &["claude"]);
    fs::create_dir_all(root.join(".claude/settings.json")).expect("directory fixture");
    let failure = match audit(root, &["claude"], &[SurfaceKind::Hook]) {
        Ok(report) => panic!("expected an escaping failure, got {report:?}"),
        Err(ToolSurfaceAuditFailure::Session(session)) => session,
        Err(other) => panic!("expected the session failure, got {other}"),
    };
    assert_eq!(failure.phase(), SessionSurfacePhase::Read);
    let error = session_surface_failure_error(&failure);
    assert_eq!(error.exit(), 1);
    assert!(
        error.message().starts_with(&format!(
            "{}: ",
            root.join(".claude/settings.json").display()
        )),
        "{}",
        error.message()
    );
    assert!(
        error.message().to_lowercase().contains("directory")
            || error.message().contains("Is a directory"),
        "{}",
        error.message()
    );
    assert!(error.to_json().get("summary").is_none());
}
