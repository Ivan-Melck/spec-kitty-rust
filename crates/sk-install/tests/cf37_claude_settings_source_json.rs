//! CF37 — the one Source `json.loads`-backed `.claude/settings.json` model.
//!
//! The pinned Source reads `settings.json` with `json.loads` and asks only
//! "is this command registered for this event". `json.loads` therefore accepts
//! documents that `serde_json` refuses: bare `NaN`/`Infinity`/`-Infinity`
//! constants, arbitrarily large integers (up to the interpreter's
//! `sys.get_int_max_str_digits()` ceiling), lone-surrogate keys/values from
//! `\uXXXX` escapes, duplicate keys (last value wins at the first position) and
//! any member order. A `JSONDecodeError` (or a non-object value) means `_load`
//! returned `{}`; the integer ceiling and recursion depth are *not*
//! `JSONDecodeError`s, so they escape the provider instead.
//!
//! These tests assert the Native model against that behaviour: Source-valid
//! documents keep the hooks registered, the writer round-trips their exact
//! values through the canonical Python JSON renderer, and the two uncaught
//! limit phases stay failures instead of becoming "hooks missing".
//!
//! Native-only functional evidence: no Source child process runs here, so this
//! is not a paired differential proof (the lane report carries the separate
//! Source-only cause probes).

use std::fs;
use std::path::Path;

use sk_install::session::{
    ClaudeSettingsShape, SESSION_START_CMD, SESSION_STOP_CMD, apply_healthy_orientation,
    observe_claude_settings,
};
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

fn settings_path(root: &Path) -> std::path::PathBuf {
    root.join(".claude/settings.json")
}

fn audit(
    root: &Path,
) -> Result<sk_install::tool_surface_diagnostics::SurfaceReport, ToolSurfaceAuditFailure> {
    let configured = vec!["claude".to_string()];
    audit_tool_surfaces(&SurfaceAuditRequest {
        project_root: root,
        configured_tools: &configured,
        tool_filter: None,
        kinds: Some(&[SurfaceKind::Hook]),
    })
}

/// A document with both lifecycle hooks plus `extra` members, in `order`.
fn document(hooks_body: &str, extra: &str) -> String {
    format!("{{{hooks_body}{extra}}}\n")
}

#[test]
fn source_valid_nonfinite_numbers_keep_the_hooks_registered() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    write(
        &settings_path(root),
        &document(
            &format!(
                "\"zeta\": 1, \"hooks\": {{\"SessionStart\": [{}], \"Stop\": [{}]}}",
                event(SESSION_START_CMD),
                event(SESSION_STOP_CMD)
            ),
            ", \"unrelated\": [NaN, Infinity, -Infinity, -0.0, 1e400], \"alpha\": 2",
        ),
    );
    let assessment = observe_claude_settings(root);
    assert_eq!(assessment.shape, ClaudeSettingsShape::Object);
    assert!(assessment.session_start);
    assert!(assessment.stop);
    assert!(assessment.registered());
    assert!(!assessment.foreign_entries);

    let report = audit(root).expect("audit");
    assert!(
        report.findings.is_empty(),
        "Source-valid non-finite members must not become hook findings: {:?}",
        report.findings
    );
    assert!(report.ok());
    assert_eq!(report.summary.surfaces, 2);
}

#[test]
fn source_valid_large_integer_and_duplicate_members_keep_the_hooks_registered() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    // 4000 digits is inside CPython's default 4300-digit conversion ceiling.
    let big = "9".repeat(4000);
    write(
        &settings_path(root),
        &document(
            &format!(
                "\"hooks\": {{\"SessionStart\": [{}], \"Stop\": [{}]}}",
                event(SESSION_START_CMD),
                event(SESSION_STOP_CMD)
            ),
            &format!(", \"big\": {big}, \"dup\": 1, \"dup\": 2, \"flag\": true"),
        ),
    );
    let assessment = observe_claude_settings(root);
    assert_eq!(assessment.shape, ClaudeSettingsShape::Object);
    assert!(assessment.registered());
    assert!(!assessment.foreign_entries);
}

#[test]
fn source_valid_lone_surrogate_keys_and_values_keep_the_hooks_registered() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    write(
        &settings_path(root),
        &document(
            &format!(
                "\"hooks\": {{\"SessionStart\": [{}], \"Stop\": [{}]}}",
                event(SESSION_START_CMD),
                event(SESSION_STOP_CMD)
            ),
            ", \"\\ud800-key\": \"\\udfff-value\", \"zeta\": null, \"alpha\": [1, {\"\\ud83d\\ude00\": 2}]",
        ),
    );
    let assessment = observe_claude_settings(root);
    assert_eq!(
        assessment.shape,
        ClaudeSettingsShape::Object,
        "a lone-surrogate key is a Python dict key, not a parse failure"
    );
    assert!(assessment.registered());
    assert!(!assessment.foreign_entries);
    assert!(audit(root).expect("audit").ok());
}

#[test]
fn command_matching_is_exact_python_string_equality() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    // Same text with a NUL suffix, a surrogate suffix, and a non-command type.
    for (session_start, stop) in [
        (
            format!("{SESSION_START_CMD}\\u0000"),
            SESSION_STOP_CMD.to_string(),
        ),
        (
            SESSION_START_CMD.to_string(),
            format!("{SESSION_STOP_CMD}\\ud800"),
        ),
    ] {
        write(
            &settings_path(root),
            &document(
                &format!(
                    "\"hooks\": {{\"SessionStart\": [{}], \"Stop\": [{}]}}",
                    event(&session_start),
                    event(&stop)
                ),
                "",
            ),
        );
        let assessment = observe_claude_settings(root);
        assert_eq!(assessment.shape, ClaudeSettingsShape::Object);
        assert!(
            !assessment.registered(),
            "an escaped suffix is a different Python string"
        );
        assert!(!assessment.session_start || !assessment.stop);
        assert!(assessment.foreign_entries);
    }

    write(
        &settings_path(root),
        &document(
            &format!(
                "\"hooks\": {{\"SessionStart\": [{{\"hooks\": [{{\"type\": \"prompt\", \"command\": \"{SESSION_START_CMD}\"}}]}}], \"Stop\": [{}]}}",
                event(SESSION_STOP_CMD)
            ),
            "",
        ),
    );
    let assessment = observe_claude_settings(root);
    assert!(!assessment.session_start, "only type == 'command' counts");
    assert!(assessment.stop);
    assert!(!assessment.registered());
    assert!(assessment.foreign_entries);
}

#[test]
fn malformed_hook_members_are_not_ours_and_never_panic() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    // (body, does the read-only assessment call it a foreign entry?)
    for (body, foreign) in [
        // The event value is not a list: absent for `_event_entries`, not foreign.
        (
            "\"hooks\": {\"SessionStart\": {\"hooks\": []}, \"Stop\": []}",
            false,
        ),
        // The entry is not an object.
        ("\"hooks\": {\"SessionStart\": [7], \"Stop\": []}", true),
        // The entry carries no hooks list.
        (
            "\"hooks\": {\"SessionStart\": [{\"hooks\": 3}], \"Stop\": []}",
            true,
        ),
        // The hook is not an object.
        (
            "\"hooks\": {\"SessionStart\": [{\"hooks\": [5]}], \"Stop\": []}",
            true,
        ),
        // `command` is not a string.
        (
            "\"hooks\": {\"SessionStart\": [{\"hooks\": [{\"type\": \"command\", \"command\": 5}]}], \"Stop\": []}",
            true,
        ),
        // `hooks` itself is not an object: absent, not foreign.
        ("\"hooks\": []", false),
    ] {
        write(&settings_path(root), &document(body, ""));
        let assessment = observe_claude_settings(root);
        assert_eq!(assessment.shape, ClaudeSettingsShape::Object, "{body}");
        assert!(!assessment.registered(), "{body}");
        assert_eq!(assessment.foreign_entries, foreign, "{body}");
        // The audit is a health verdict, never a crash.
        assert!(!audit(root).expect("audit").findings.is_empty(), "{body}");
    }
}

#[test]
fn invalid_json_and_non_object_documents_are_absent_not_limit_failures() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    write(&settings_path(root), "{\"hooks\": ");
    let invalid = observe_claude_settings(root);
    assert_eq!(invalid.shape, ClaudeSettingsShape::InvalidJson);
    assert!(!invalid.registered());
    assert!(
        !audit(root).expect("audit").findings.is_empty(),
        "invalid JSON is a missing-hook finding, not an escaping failure"
    );

    write(&settings_path(root), "[1, 2, 3]\n");
    let not_object = observe_claude_settings(root);
    assert_eq!(not_object.shape, ClaudeSettingsShape::NotObject);
    assert!(!not_object.registered());

    fs::remove_file(settings_path(root)).expect("remove");
    let absent = observe_claude_settings(root);
    assert_eq!(absent.shape, ClaudeSettingsShape::Absent);
    assert!(!absent.registered());
}

#[test]
fn the_integer_digit_ceiling_is_an_uncaught_source_phase() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    // One digit past CPython's default 4300-digit conversion ceiling. The
    // pinned Source raises `ValueError` from `json.loads`; `_load` catches only
    // `JSONDecodeError`, so the provider has no verdict to report.
    let big = "9".repeat(4301);
    write(
        &settings_path(root),
        &document(
            &format!(
                "\"hooks\": {{\"SessionStart\": [{}], \"Stop\": [{}]}}",
                event(SESSION_START_CMD),
                event(SESSION_STOP_CMD)
            ),
            &format!(", \"big\": {big}"),
        ),
    );
    let assessment = observe_claude_settings(root);
    assert_eq!(
        assessment.shape,
        ClaudeSettingsShape::UncaughtJson,
        "the integer ceiling is a ValueError, not a JSONDecodeError"
    );
    assert!(assessment.detail.is_some());
    assert!(!assessment.registered());

    match audit(root) {
        Err(ToolSurfaceAuditFailure::Session(failure)) => {
            assert_eq!(failure.phase(), SessionSurfacePhase::JsonLimit);
            assert!(
                failure.detail().contains("int_max_str_digits"),
                "detail: {}",
                failure.detail()
            );
            assert!(failure.path().ends_with("settings.json"));
        }
        other => panic!("expected an escaping limit failure, got {other:?}"),
    }
}

#[test]
fn the_recursion_ceiling_is_an_uncaught_source_phase() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let deep = format!("{}{}", "[".repeat(1500), "]".repeat(1500));
    write(&settings_path(root), &format!("{{\"hooks\": {deep}}}\n"));
    let assessment = observe_claude_settings(root);
    assert_eq!(assessment.shape, ClaudeSettingsShape::UncaughtJson);
    assert!(!assessment.registered());
    match audit(root) {
        Err(ToolSurfaceAuditFailure::Session(failure)) => {
            assert_eq!(failure.phase(), SessionSurfacePhase::JsonLimit);
        }
        other => panic!("expected an escaping limit failure, got {other:?}"),
    }
}

#[test]
fn the_writer_preserves_source_valid_values_order_and_duplicates() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let big = "9".repeat(4000);
    let source = format!(
        "{{\"alpha\": 1, \"hooks\": {{\"Stop\": [{}]}}, \"zeta\": NaN, \"big\": {big}, \"k\": \"\\ud800\", \"dup\": 1, \"dup\": 2, \"permissions\": {{\"allow\": [\"Bash\", \"é\"]}}}}\n",
        event(SESSION_STOP_CMD)
    );
    write(&settings_path(root), &source);

    apply_healthy_orientation(root, &["claude".to_string()], "4.0.0rc8", "demo").expect("apply");

    let installed = fs::read_to_string(settings_path(root)).expect("read");
    let expected = format!(
        concat!(
            "{{\n",
            "  \"alpha\": 1,\n",
            "  \"hooks\": {{\n",
            "    \"Stop\": [\n",
            "      {{\n",
            "        \"hooks\": [\n",
            "          {{\n",
            "            \"type\": \"command\",\n",
            "            \"command\": \"spec-kitty session-stop\"\n",
            "          }}\n",
            "        ]\n",
            "      }}\n",
            "    ],\n",
            "    \"SessionStart\": [\n",
            "      {{\n",
            "        \"hooks\": [\n",
            "          {{\n",
            "            \"type\": \"command\",\n",
            "            \"command\": \"spec-kitty session-start\"\n",
            "          }}\n",
            "        ]\n",
            "      }}\n",
            "    ]\n",
            "  }},\n",
            "  \"zeta\": NaN,\n",
            "  \"big\": {big},\n",
            "  \"k\": \"\\ud800\",\n",
            "  \"dup\": 2,\n",
            "  \"permissions\": {{\n",
            "    \"allow\": [\n",
            "      \"Bash\",\n",
            "      \"\\u00e9\"\n",
            "    ]\n",
            "  }}\n",
            "}}\n"
        ),
        big = big
    );
    assert_eq!(installed, expected);

    // A second pass is a no-op: both hooks are already registered.
    apply_healthy_orientation(root, &["claude".to_string()], "4.0.0rc8", "demo").expect("apply");
    assert_eq!(
        fs::read_to_string(settings_path(root)).expect("read"),
        expected,
        "an already-registered document must not be rewritten"
    );
}

#[test]
fn the_writer_leaves_a_source_valid_registered_document_untouched() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let source = document(
        &format!(
            "\"zeta\": 3, \"hooks\": {{\"SessionStart\": [{}], \"Stop\": [{}]}}",
            event(SESSION_START_CMD),
            event(SESSION_STOP_CMD)
        ),
        ", \"unrelated\": [NaN, Infinity], \"\\ud800\": 1",
    );
    write(&settings_path(root), &source);
    apply_healthy_orientation(root, &["claude".to_string()], "4.0.0rc8", "demo").expect("apply");
    assert_eq!(
        fs::read_to_string(settings_path(root)).expect("read"),
        source,
        "both hooks present: the reference writes nothing"
    );
}

#[test]
fn a_fresh_settings_file_is_created_with_the_canonical_bytes() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    fs::create_dir_all(root.join(".claude")).expect("claude root");
    apply_healthy_orientation(root, &["claude".to_string()], "4.0.0rc8", "demo").expect("apply");
    let installed = fs::read_to_string(settings_path(root)).expect("read");
    assert!(installed.contains("\"SessionStart\""));
    assert!(installed.contains("\"Stop\""));
    assert!(installed.ends_with("}\n"));
    assert!(observe_claude_settings(root).registered());
}

#[test]
fn a_configured_claude_project_with_source_valid_settings_audits_clean() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    configure(root, &["claude"]);
    write(
        &root.join(".claude/CLAUDE.md"),
        &sk_install::OrientationContent::healthy(sk_install::COMPAT_CORE_VERSION, "demo").render(),
    );
    write(
        &settings_path(root),
        &document(
            &format!(
                "\"hooks\": {{\"SessionStart\": [{}], \"Stop\": [{}]}}",
                event(SESSION_START_CMD),
                event(SESSION_STOP_CMD)
            ),
            ", \"env\": {\"A\": NaN}, \"big\": 123456789012345678901234567890",
        ),
    );
    let report = audit(root).expect("audit");
    assert!(report.ok());
    assert_eq!(report.summary.surfaces, 2);
    assert!(report.findings.is_empty());
}
