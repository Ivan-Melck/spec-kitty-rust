//! CF33 — session-presence decode/read phases over the one Kernel decoder.
//!
//! What the pinned Source actually does (observed with the lock-exact oracle;
//! see the lane report's receipts):
//!
//! * orientation is read with `Path.read_text(encoding="utf-8")`: strict UTF-8,
//!   BOM kept as a character, universal-newline translation, and a decode error
//!   escapes the provider;
//! * `.claude/settings.json` is read with `Path.read_bytes()` and decoded with
//!   `kernel.text_decode.decode_unambiguous`: a UTF-16/32 BOM is honoured, an
//!   unprovable payload raises `SettingsNotDecodableError` (caught -> unsafe),
//!   and any other read `OSError` escapes the provider.
//!
//! These tests assert the Native seam against that behaviour. They are
//! Native-only functional evidence: no Source child process runs here, so this
//! is not a paired differential proof.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use sk_install::session::{
    ClaudeSettingsShape, OrientationSectionState, SESSION_START_CMD, SESSION_STOP_CMD,
    assess_orientation, observe_claude_settings,
};
use sk_install::tool_surface_diagnostics::{
    SessionSurfacePhase, SurfaceAuditRequest, SurfaceKind, ToolSurfaceAuditFailure,
    audit_tool_surfaces,
};
use sk_install::{COMPAT_CORE_VERSION, OrientationContent};

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, contents).expect("fixture write");
}

fn write_bytes(path: &Path, contents: &[u8]) {
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

fn hook_settings(session_start: bool, stop: bool) -> String {
    let entry = |command: &str| {
        format!(
            "      {{\n        \"hooks\": [\n          {{\"type\": \"command\", \"command\": \"{command}\"}}\n        ]\n      }}"
        )
    };
    let mut events = Vec::new();
    if session_start {
        events.push(format!(
            "    \"SessionStart\": [\n{}\n    ]",
            entry(SESSION_START_CMD)
        ));
    }
    if stop {
        events.push(format!(
            "    \"Stop\": [\n{}\n    ]",
            entry(SESSION_STOP_CMD)
        ));
    }
    format!(
        "{{\n  \"hooks\": {{\n{}\n  }},\n  \"permissions\": {{\"allow\": [\"Bash\"]}}\n}}\n",
        events.join(",\n")
    )
}

fn utf16_bom(text: &str, big_endian: bool) -> Vec<u8> {
    let mut bytes = if big_endian {
        vec![0xfe, 0xff]
    } else {
        vec![0xff, 0xfe]
    };
    for unit in text.encode_utf16() {
        let encoded = if big_endian {
            unit.to_be_bytes()
        } else {
            unit.to_le_bytes()
        };
        bytes.extend_from_slice(&encoded);
    }
    bytes
}

fn utf32_bom(text: &str, big_endian: bool) -> Vec<u8> {
    let mut bytes = if big_endian {
        vec![0x00, 0x00, 0xfe, 0xff]
    } else {
        vec![0xff, 0xfe, 0x00, 0x00]
    };
    for character in text.chars() {
        let scalar = character as u32;
        let encoded = if big_endian {
            scalar.to_be_bytes()
        } else {
            scalar.to_le_bytes()
        };
        bytes.extend_from_slice(&encoded);
    }
    bytes
}

fn audit(
    root: &Path,
    configured: &[&str],
    kinds: &[SurfaceKind],
) -> Result<sk_install::tool_surface_diagnostics::SurfaceReport, ToolSurfaceAuditFailure> {
    let tools: Vec<String> = configured.iter().map(|key| (*key).to_string()).collect();
    audit_tool_surfaces(&SurfaceAuditRequest {
        project_root: root,
        configured_tools: &tools,
        tool_filter: None,
        kinds: Some(kinds),
    })
}

fn session_failure(
    failure: ToolSurfaceAuditFailure,
) -> sk_install::tool_surface_diagnostics::SessionSurfaceFailure {
    match failure {
        ToolSurfaceAuditFailure::Session(session) => session,
        other => panic!("expected the session failure, got {other}"),
    }
}

fn snapshot(root: &Path) -> BTreeMap<std::path::PathBuf, String> {
    let mut entries = BTreeMap::new();
    fn walk(base: &Path, path: &Path, entries: &mut BTreeMap<std::path::PathBuf, String>) {
        let metadata = fs::symlink_metadata(path).expect("metadata");
        #[cfg(unix)]
        let (mode, inode) = {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            (metadata.permissions().mode(), metadata.ino())
        };
        #[cfg(not(unix))]
        let (mode, inode) = (0u32, 0u64);
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let contents = if metadata.is_file() {
            fs::read(path).map(|bytes| bytes.len()).unwrap_or(0)
        } else {
            0
        };
        entries.insert(
            path.strip_prefix(base).unwrap_or(path).to_path_buf(),
            format!("mode={mode} inode={inode} mtime={modified} len={contents}"),
        );
        if metadata.is_dir() {
            let mut children: Vec<std::path::PathBuf> = fs::read_dir(path)
                .expect("read dir")
                .flatten()
                .map(|entry| entry.path())
                .collect();
            children.sort();
            for child in children {
                walk(base, &child, entries);
            }
        }
    }
    walk(root, root, &mut entries);
    entries
}

#[test]
fn utf16_and_utf32_bom_settings_are_present_not_undecodable() {
    fn utf8(text: &str) -> Vec<u8> {
        text.as_bytes().to_vec()
    }
    fn utf8_sig(text: &str) -> Vec<u8> {
        let mut bytes = b"\xef\xbb\xbf".to_vec();
        bytes.extend_from_slice(text.as_bytes());
        bytes
    }
    fn utf16le(text: &str) -> Vec<u8> {
        utf16_bom(text, false)
    }
    fn utf16be(text: &str) -> Vec<u8> {
        utf16_bom(text, true)
    }
    fn utf32le(text: &str) -> Vec<u8> {
        utf32_bom(text, false)
    }
    fn utf32be(text: &str) -> Vec<u8> {
        utf32_bom(text, true)
    }

    let settings = hook_settings(true, true);
    let partial = hook_settings(true, false);
    for (label, encode) in [
        ("utf-8", utf8 as fn(&str) -> Vec<u8>),
        ("utf-8-sig", utf8_sig),
        ("utf-16-le", utf16le),
        ("utf-16-be", utf16be),
        ("utf-32-le", utf32le),
        ("utf-32-be", utf32be),
    ] {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();
        configure(root, &["claude"]);
        write_bytes(&root.join(".claude/settings.json"), &encode(&settings));
        let report = audit(root, &["claude"], &[SurfaceKind::Hook]).expect("audit");
        assert_eq!(report.summary.surfaces, 2, "{label}");
        assert_eq!(report.summary.present, 2, "{label}");
        assert_eq!(report.summary.missing, 0, "{label}");
        assert!(report.findings.is_empty(), "{label}: ok={}", report.ok());
        // The decoded document is really read: dropping one event must be
        // reported as missing rather than "present by encoding".
        write_bytes(&root.join(".claude/settings.json"), &encode(&partial));
        let report = audit(root, &["claude"], &[SurfaceKind::Hook]).expect("audit");
        assert_eq!(report.summary.missing, 2, "{label} partial");
    }
}

#[test]
fn a_bom_proven_payload_its_codec_rejects_is_unsafe_not_missing() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    configure(root, &["claude"]);
    // BOM proves UTF-16-LE, then an odd trailing byte: `decode_unambiguous`
    // returns None, so the reference raises `SettingsNotDecodableError`, which
    // the provider catches as an unsafe finding.
    write_bytes(
        &root.join(".claude/settings.json"),
        b"\xff\xfe{\x00}\x00\x00",
    );
    let report = audit(root, &["claude"], &[SurfaceKind::Hook]).expect("audit");
    assert_eq!(report.summary.surfaces, 2);
    assert_eq!(report.summary.missing, 0);
    assert_eq!(report.summary.errors, 2);
    for status in &report.surfaces {
        assert_eq!(status.state, "unsafe");
    }
    for finding in &report.findings {
        assert_eq!(finding.code, "session-presence-undecodable");
        assert!(finding.message.starts_with("undecodable: re-save "));
        assert!(finding.message.ends_with(" as UTF-8"));
    }
}

#[test]
fn cp1252_settings_stay_unsafe_and_invalid_json_stays_missing() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    configure(root, &["claude"]);
    write_bytes(
        &root.join(".claude/settings.json"),
        b"{\"hooks\": {\"Stop\": [{\"hooks\": [{\"type\": \"command\", \"command\": \"caf\xe9\"}]}]}}",
    );
    let report = audit(root, &["claude"], &[SurfaceKind::Hook]).expect("audit");
    assert_eq!(report.summary.errors, 2);
    assert!(
        report
            .surfaces
            .iter()
            .all(|status| status.state == "unsafe")
    );

    // Invalid JSON is not a decode failure: the reference's `_load` returns `{}`
    // and the hooks are reported missing.
    for payload in [b"{not json".as_slice(), b"[]\n".as_slice()] {
        write_bytes(&root.join(".claude/settings.json"), payload);
        let report = audit(root, &["claude"], &[SurfaceKind::Hook]).expect("audit");
        assert_eq!(report.summary.missing, 2, "{payload:?}");
        assert_eq!(report.summary.errors, 2, "{payload:?}");
    }

    // Absent settings are missing, never unsafe.
    fs::remove_file(root.join(".claude/settings.json")).expect("remove");
    let report = audit(root, &["claude"], &[SurfaceKind::Hook]).expect("audit");
    assert_eq!(report.summary.missing, 2);
    assert!(
        report
            .findings
            .iter()
            .all(|f| f.code == "session-presence-incomplete")
    );
}

#[test]
fn an_undecodable_orientation_escapes_with_the_cpython_message() {
    for payload in [
        // cp1252, no BOM.
        b"caf\xe9\n".as_slice(),
        // A UTF-16LE BOM does not make orientation decodable: the reference
        // reads orientation as strict UTF-8.
        b"\xff\xfea\x00\n\x00",
    ] {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();
        configure(root, &["codex"]);
        write_bytes(&root.join("AGENTS.md"), payload);
        let failure = session_failure(
            audit(root, &["codex"], &[SurfaceKind::ContextFile])
                .expect_err("the decode error must escape the audit"),
        );
        assert_eq!(failure.phase(), SessionSurfacePhase::Decode);
        assert_eq!(failure.path(), root.join("AGENTS.md").as_path());
        assert!(
            failure
                .detail()
                .starts_with("'utf-8' codec can't decode byte 0x"),
            "{}",
            failure.detail()
        );
        // The display line keeps the inspected path and the reference message.
        let line = failure.to_string();
        assert!(line.contains(&root.join("AGENTS.md").display().to_string()));
        assert!(line.contains("'utf-8' codec can't decode"));
    }
}

#[test]
fn utf8_bom_orientation_is_present_and_newlines_are_translated() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    configure(root, &["codex"]);
    let block = OrientationContent::healthy(COMPAT_CORE_VERSION, "demo").render();
    let mut bytes = b"\xef\xbb\xbf".to_vec();
    bytes.extend_from_slice(block.replace('\n', "\r\n").as_bytes());
    write_bytes(&root.join("AGENTS.md"), &bytes);
    // `read_text(encoding="utf-8")` keeps the BOM as U+FEFF and translates CRLF;
    // the managed block is found and the version is current.
    let assessment = assess_orientation(&root.join("AGENTS.md"), COMPAT_CORE_VERSION);
    assert!(assessment.present(), "{:?}", assessment.state);
    assert!(!assessment.stale);
    let report = audit(root, &["codex"], &[SurfaceKind::ContextFile]).expect("audit");
    assert_eq!(report.summary.present, 1);
    assert!(report.ok());
}

#[test]
fn a_utf16_orientation_is_reported_undecodable_with_the_exact_error() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let text = OrientationContent::healthy(COMPAT_CORE_VERSION, "demo").render();
    let path = root.join("AGENTS.md");
    write_bytes(&path, &utf16_bom(&text, false));
    let assessment = assess_orientation(&path, COMPAT_CORE_VERSION);
    assert!(!assessment.present());
    match &assessment.state {
        OrientationSectionState::Undecodable(error) => {
            assert_eq!(error.start, 0);
            assert_eq!(error.encoding(), "utf-8");
            assert!(error.to_string().contains("invalid start byte"));
        }
        other => panic!("expected Undecodable, got {other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn orientation_read_errors_stay_missing_but_settings_read_errors_escape() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    configure(root, &["claude", "codex"]);
    write(
        &root.join("AGENTS.md"),
        &OrientationContent::healthy(COMPAT_CORE_VERSION, "demo").render(),
    );
    write(
        &root.join(".claude/settings.json"),
        &hook_settings(true, true),
    );
    fs::set_permissions(root.join("AGENTS.md"), fs::Permissions::from_mode(0o000)).expect("chmod");
    fs::set_permissions(
        root.join(".claude/settings.json"),
        fs::Permissions::from_mode(0o000),
    )
    .expect("chmod");

    // The reference catches the orientation read `OSError` -> missing.
    let report = audit(root, &["codex"], &[SurfaceKind::ContextFile]).expect("audit");
    assert_eq!(report.summary.missing, 1);
    assert_eq!(report.findings[0].code, "context-file-missing");

    // The settings read `OSError` has no handler upstream -> it escapes.
    let failure = session_failure(
        audit(root, &["claude"], &[SurfaceKind::Hook])
            .expect_err("the settings read error must escape the audit"),
    );
    assert_eq!(failure.phase(), SessionSurfacePhase::Read);
    assert_eq!(failure.path(), root.join(".claude/settings.json").as_path());
    assert!(
        failure.to_string().contains("Permission denied"),
        "{}",
        failure
    );

    fs::set_permissions(root.join("AGENTS.md"), fs::Permissions::from_mode(0o644)).expect("chmod");
    fs::set_permissions(
        root.join(".claude/settings.json"),
        fs::Permissions::from_mode(0o644),
    )
    .expect("chmod");
}

#[test]
fn a_non_regular_settings_destination_escapes_as_a_read_failure() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    configure(root, &["claude"]);
    fs::create_dir_all(root.join(".claude/settings.json")).expect("directory fixture");
    let failure = session_failure(
        audit(root, &["claude"], &[SurfaceKind::Hook])
            .expect_err("reading a directory escapes `Path.read_bytes`"),
    );
    assert_eq!(failure.phase(), SessionSurfacePhase::Read);
    assert_eq!(failure.path(), root.join(".claude/settings.json").as_path());
}

#[test]
fn the_decode_corrected_audit_is_read_only_for_bytes_modes_inodes_and_mtimes() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    configure(root, &["claude", "codex"]);
    // Deliberately non-canonical bytes; any writer would change them.
    write(
        &root.join("AGENTS.md"),
        "team notes, no orientation block\r\n",
    );
    write_bytes(&root.join(".claude/CLAUDE.md"), &{
        let mut bytes = b"\xef\xbb\xbf".to_vec();
        bytes.extend_from_slice(b"stale, not canonical\r\n");
        bytes
    });
    write_bytes(
        &root.join(".claude/settings.json"),
        &utf16_bom(&hook_settings(true, false), true),
    );

    let before = snapshot(root);
    for (kind, minimum) in [
        (SurfaceKind::ContextFile, 2),
        (SurfaceKind::Hook, 2),
        // Neither claude nor codex manages a path/glob-activated rule.
        (SurfaceKind::Rule, 0),
    ] {
        let report = audit(root, &["claude", "codex"], &[kind]).expect("audit");
        assert_eq!(report.summary.surfaces, minimum, "{kind:?}");
    }
    assert_eq!(
        before,
        snapshot(root),
        "the audit must not change bytes, modes, inodes or mtimes"
    );

    // The assessment seam alone has the same guarantee, including a failing
    // decode (which is a read, not a write).
    let before = snapshot(root);
    let _ = assess_orientation(&root.join(".claude/CLAUDE.md"), COMPAT_CORE_VERSION);
    assert!(matches!(
        observe_claude_settings(root).shape,
        ClaudeSettingsShape::Object
    ));
    assert_eq!(before, snapshot(root));
}
