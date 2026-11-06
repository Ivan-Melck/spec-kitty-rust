//! CF28 — NEW actual session-presence health probes (`context_file` / `rule` / `hook`).
//!
//! These exercise the real read-only diagnostics in
//! `sk_install::tool_surface_diagnostics` plus the writer-descriptor/assessment
//! authority in `sk_install::session`, on fresh `tempfile` fixtures.
//!
//! What is asserted here, and what is NOT:
//!
//! * real expansion per the canonical writer roster (Claude = one context file +
//!   two hook entries; Markdown family = one context-or-rule file; a `NullWriter`
//!   harness = the reference's research gap), never a hardcoded path table;
//! * ordinary presence/marker/staleness findings with the reference's codes,
//!   severities, states, messages and repair hints;
//! * the Claude settings assessment (absent / custom / malformed / undecodable /
//!   registered, plus the git-tracked preserve fact);
//! * the audit is read-only — a byte/mode/inode/mtime snapshot of the whole
//!   fixture is identical before and after every audit, and no writer ran
//!   (the fixtures are deliberately not canonical);
//! * NOT proven here: paired Source/Native differential output, platform path
//!   spellings, Rich-rendered human output, `--fix` repair. The UTF-16/32
//!   settings BOM case and the escaping read/decode phases that CF28 enumerated
//!   as debt are covered by the CF33 lane's own targets.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use sk_install::session::{
    ClaudeSettingsShape, SessionArtefactKind, SessionWriterMode, assess_claude_hooks,
    assess_orientation, observe_claude_settings, session_artefact_kind, session_artefacts,
    session_writer,
};
use sk_install::tool_surface_diagnostics::{
    self as diagnostics, SessionSurfacePhase, SurfaceAuditRequest, SurfaceKind,
    ToolSurfaceAuditFailure, audit_tool_surfaces,
};
use sk_install::{COMPAT_CORE_VERSION, OrientationContent, SECTION_OPEN};

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

fn audit(root: &Path, configured: &[&str], kinds: &[SurfaceKind]) -> diagnostics::SurfaceReport {
    let configured: Vec<String> = configured.iter().map(|key| (*key).to_string()).collect();
    audit_tool_surfaces(&SurfaceAuditRequest {
        project_root: root,
        configured_tools: &configured,
        tool_filter: None,
        kinds: Some(kinds),
    })
    .expect("audit")
}

fn orientation(version: &str, slug: &str) -> String {
    OrientationContent::healthy(version, slug).render()
}

fn claude_settings(session_start: Option<&str>, stop: Option<&str>, extra: &str) -> String {
    let entry = |command: &str| {
        format!(
            "      {{\n        \"hooks\": [\n          {{\"type\": \"command\", \"command\": \"{command}\"}}\n        ]\n      }}"
        )
    };
    let mut events = Vec::new();
    if let Some(command) = session_start {
        events.push(format!(
            "    \"SessionStart\": [\n{}\n    ]",
            entry(command)
        ));
    }
    if let Some(command) = stop {
        events.push(format!("    \"Stop\": [\n{}\n    ]", entry(command)));
    }
    format!(
        "{{\n  \"hooks\": {{\n{}\n  }}{extra}\n}}\n",
        events.join(",\n")
    )
}

/// A byte/mode/inode/mtime snapshot of every entry below `root`.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, String> {
    let mut entries = BTreeMap::new();
    fn walk(base: &Path, path: &Path, entries: &mut BTreeMap<PathBuf, String>) {
        let metadata = fs::symlink_metadata(path).expect("metadata");
        #[cfg(unix)]
        let mode = {
            use std::os::unix::fs::PermissionsExt;
            metadata.permissions().mode()
        };
        #[cfg(not(unix))]
        let mode = 0u32;
        #[cfg(unix)]
        let inode = {
            use std::os::unix::fs::MetadataExt;
            metadata.ino()
        };
        #[cfg(not(unix))]
        let inode = 0u64;
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let contents = if metadata.is_file() {
            fs::read(path)
                .map(|bytes| format!("{}:{}", bytes.len(), fnv1a(&bytes)))
                .unwrap_or_else(|_| "unreadable".to_owned())
        } else {
            String::new()
        };
        entries.insert(
            path.strip_prefix(base).unwrap_or(path).to_path_buf(),
            format!("mode={mode} inode={inode} mtime={modified} {contents}"),
        );
        if metadata.is_dir() {
            let mut children: Vec<PathBuf> = fs::read_dir(path)
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

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The canonical configured-agent roster the writer table must cover.
fn session_roster() -> Vec<&'static str> {
    sk_install::agent::agent_keys().collect()
}

#[test]
fn every_harness_resolves_through_the_one_writer_descriptor_roster() {
    let claude = session_writer("claude").expect("claude descriptor");
    assert_eq!(claude.mode, SessionWriterMode::ClaudeCode);
    assert_eq!(claude.path, ".claude/CLAUDE.md");
    assert_eq!(claude.check_dir, Some(".claude"));

    for key in ["codex", "opencode", "antigravity", "pi", "vibe", "letta"] {
        let descriptor = session_writer(key).expect(key);
        assert_eq!(descriptor.path, "AGENTS.md", "{key}");
        assert_eq!(descriptor.mode, SessionWriterMode::Section, "{key}");
    }
    assert_eq!(session_writer("gemini").expect("gemini").path, "GEMINI.md");
    assert_eq!(session_writer("llxprt").expect("llxprt").path, "LLXPRT.md");
    let copilot = session_writer("copilot").expect("copilot");
    assert_eq!(copilot.path, ".github/copilot-instructions.md");
    assert_eq!(copilot.check_dir, Some(".github"));
    for (key, path) in [
        ("cursor", ".cursor/rules/spec-kitty.mdc"),
        ("windsurf", ".windsurf/rules/spec-kitty.md"),
        ("kiro", ".kiro/steering/spec-kitty.md"),
    ] {
        let descriptor = session_writer(key).expect(key);
        assert_eq!(descriptor.path, path, "{key}");
        assert_eq!(descriptor.mode, SessionWriterMode::WholeFile, "{key}");
    }

    for key in session_roster() {
        if matches!(key, "qwen" | "kilocode" | "auggie" | "q") {
            assert!(
                session_writer(key).is_none(),
                "{key} must stay a NullWriter harness"
            );
        } else {
            assert!(session_writer(key).is_some(), "{key} lost its descriptor");
        }
    }
    assert!(session_writer("not-a-harness").is_none());
}

#[test]
fn artefacts_and_kind_classification_follow_the_source_roster() {
    let claude: Vec<(SessionArtefactKind, &str)> = session_artefacts("claude")
        .iter()
        .map(|artefact| (artefact.kind, artefact.path))
        .collect();
    assert_eq!(
        claude,
        vec![
            (SessionArtefactKind::ContextFile, ".claude/CLAUDE.md"),
            (SessionArtefactKind::Hook, ".claude/settings.json"),
            (SessionArtefactKind::Hook, ".claude/settings.json"),
        ],
        "Claude plans one context file and one hook instance per lifecycle event"
    );
    assert_eq!(
        session_artefacts("cursor")[0].kind,
        SessionArtefactKind::Rule,
        ".mdc targets are rules"
    );
    assert_eq!(session_artefacts("kiro")[0].kind, SessionArtefactKind::Rule);
    assert_eq!(
        session_artefacts("copilot")[0].kind,
        SessionArtefactKind::ContextFile
    );
    assert_eq!(
        session_artefacts("codex")[0].kind,
        SessionArtefactKind::ContextFile
    );
    assert!(session_artefacts("qwen").is_empty());

    for (path, expected) in [
        (".cursor/rules/spec-kitty.mdc", SessionArtefactKind::Rule),
        (".kiro/steering/spec-kitty.md", SessionArtefactKind::Rule),
        (
            ".github/copilot-instructions.md",
            SessionArtefactKind::ContextFile,
        ),
        ("AGENTS.md", SessionArtefactKind::ContextFile),
        ("GEMINI.md", SessionArtefactKind::ContextFile),
    ] {
        assert_eq!(session_artefact_kind(path), expected, "{path}");
    }
}

#[test]
fn a_missing_context_file_rule_and_hook_entry_are_reported_per_kind() {
    let temp = tempfile::tempdir().expect("temp");
    configure(temp.path(), &["codex"]);

    let context = audit(temp.path(), &["codex"], &[SurfaceKind::ContextFile]);
    assert_eq!(context.summary.surfaces, 1);
    let status = &context.surfaces[0];
    assert_eq!(status.state, "missing");
    assert_eq!(status.instance.path, temp.path().join("AGENTS.md"));
    assert_eq!(context.findings[0].code, "context-file-missing");
    assert_eq!(context.findings[0].severity, "error");
    assert_eq!(
        context.findings[0].message,
        format!(
            "Always-on context file missing for codex: {}",
            temp.path().join("AGENTS.md").display()
        )
    );
    assert_eq!(
        context.findings[0].repair_command,
        Some("spec-kitty doctor tool-surfaces --kind context_file --fix")
    );
    assert_eq!(context.summary.errors, 1);
    assert!(!context.ok());
    let payload = context.to_json();
    assert_eq!(
        payload["surfaces"][0]["kind"],
        serde_json::json!("context_file")
    );
    assert_eq!(payload["ok"], serde_json::json!(false));

    let temp = tempfile::tempdir().expect("temp");
    configure(temp.path(), &["kiro"]);
    let rule = audit(temp.path(), &["kiro"], &[SurfaceKind::Rule]);
    assert_eq!(rule.summary.surfaces, 1);
    assert_eq!(rule.surfaces[0].state, "missing");
    assert_eq!(rule.findings[0].code, "session-presence-incomplete");
    assert!(
        rule.findings[0]
            .message
            .starts_with("Session-presence rule file missing for kiro: ")
    );
    // The reference shares one repair hint for context files and rules.
    assert_eq!(
        rule.findings[0].repair_command,
        Some("spec-kitty doctor tool-surfaces --kind context_file --fix")
    );

    let temp = tempfile::tempdir().expect("temp");
    configure(temp.path(), &["claude"]);
    let hook = audit(temp.path(), &["claude"], &[SurfaceKind::Hook]);
    assert_eq!(
        hook.summary.surfaces, 2,
        "the reference plans one hook instance per lifecycle event"
    );
    for status in &hook.surfaces {
        assert_eq!(status.state, "missing");
        assert_eq!(
            status.instance.path,
            temp.path().join(".claude/settings.json")
        );
        assert_eq!(status.instance.definition.kind, SurfaceKind::Hook);
    }
    for finding in &hook.findings {
        assert_eq!(finding.code, "session-presence-incomplete");
        assert_eq!(finding.severity, "error");
        assert!(
            finding
                .message
                .starts_with("Session-presence hook entry missing for claude: ")
        );
        assert_eq!(
            finding.repair_command,
            Some("spec-kitty doctor tool-surfaces --kind hook --fix")
        );
    }
}

#[test]
fn present_current_stale_markerless_and_unterminated_orientation_blocks() {
    let temp = tempfile::tempdir().expect("temp");
    configure(temp.path(), &["codex"]);
    write(
        &temp.path().join("AGENTS.md"),
        &orientation(COMPAT_CORE_VERSION, "demo"),
    );
    let current = audit(temp.path(), &["codex"], &[SurfaceKind::ContextFile]);
    assert_eq!(current.surfaces[0].state, "present");
    assert!(current.findings.is_empty(), "{:?}", current.findings);
    assert!(current.ok());

    // A stale stamp is a warning, never an error, and names the repair.
    write(
        &temp.path().join("AGENTS.md"),
        &orientation("0.0.1", "demo"),
    );
    let stale = audit(temp.path(), &["codex"], &[SurfaceKind::ContextFile]);
    assert_eq!(stale.surfaces[0].state, "stale");
    assert_eq!(stale.findings[0].code, "stale-generated-surface");
    assert_eq!(stale.findings[0].severity, "warning");
    assert_eq!(
        stale.findings[0].message,
        format!(
            "Orientation version stale for codex: {}",
            temp.path().join("AGENTS.md").display()
        )
    );
    assert_eq!(stale.summary.warnings, 1);
    assert_eq!(stale.summary.errors, 0);
    assert!(stale.ok(), "a stale surface is not an error");

    // A file without the markers is not a managed orientation surface.
    write(&temp.path().join("AGENTS.md"), "team notes only\n");
    let markerless = audit(temp.path(), &["codex"], &[SurfaceKind::ContextFile]);
    assert_eq!(markerless.surfaces[0].state, "missing");
    assert_eq!(markerless.findings[0].code, "context-file-missing");

    // A block whose closing marker was lost is still present (the reference
    // only asks whether the opening marker exists) and carries no parseable
    // stamp, so it is not churned into "stale".
    write(
        &temp.path().join("AGENTS.md"),
        &format!("{SECTION_OPEN}\n**Spec Kitty v{COMPAT_CORE_VERSION}** truncated\n"),
    );
    let unterminated = audit(temp.path(), &["codex"], &[SurfaceKind::ContextFile]);
    assert_eq!(unterminated.surfaces[0].state, "present");
    assert!(unterminated.findings.is_empty());
}

#[test]
fn the_orientation_assessment_reads_the_version_inside_the_managed_block_only() {
    let temp = tempfile::tempdir().expect("temp");
    let path = temp.path().join("AGENTS.md");
    write(
        &path,
        &format!(
            "pasted example: **Spec Kitty v9.9.9** — not the managed stamp\n\n{}",
            orientation(COMPAT_CORE_VERSION, "demo")
        ),
    );
    let assessment = assess_orientation(&path, COMPAT_CORE_VERSION);
    assert!(assessment.present());
    assert!(
        !assessment.stale,
        "a stamp outside the block must not count"
    );
    assert_eq!(assessment.expected_version, COMPAT_CORE_VERSION);

    let missing = assess_orientation(&temp.path().join("absent.md"), COMPAT_CORE_VERSION);
    assert!(!missing.present());
    assert!(!missing.stale);
}

#[test]
fn claude_hook_assessment_distinguishes_absent_custom_malformed_and_registered() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let absent = observe_claude_settings(root);
    assert_eq!(absent.shape, ClaudeSettingsShape::Absent);
    assert!(!absent.registered());
    assert!(!absent.foreign_entries);
    assert_eq!(absent.tracked, None, "an audit must not spawn git");

    write(&root.join(".claude/settings.json"), "{not json");
    let invalid = observe_claude_settings(root);
    assert_eq!(invalid.shape, ClaudeSettingsShape::InvalidJson);
    assert!(!invalid.registered());

    write(&root.join(".claude/settings.json"), "[]\n");
    let not_object = observe_claude_settings(root);
    assert_eq!(not_object.shape, ClaudeSettingsShape::NotObject);

    write(
        &root.join(".claude/settings.json"),
        &claude_settings(
            Some("some-other-tool session-start"),
            Some("spec-kitty session-stop"),
            ",\n  \"permissions\": {\"allow\": [\"Bash\"]}",
        ),
    );
    let custom = observe_claude_settings(root);
    assert_eq!(custom.shape, ClaudeSettingsShape::Object);
    assert!(!custom.session_start);
    assert!(custom.stop);
    assert!(!custom.registered());
    assert!(
        custom.foreign_entries,
        "a foreign SessionStart entry is recorded, never reported as ours"
    );

    write(
        &root.join(".claude/settings.json"),
        &claude_settings(
            Some("spec-kitty session-start"),
            Some("spec-kitty session-stop"),
            ",\n  \"permissions\": {\"allow\": [\"Bash\"]}",
        ),
    );
    let registered = observe_claude_settings(root);
    assert!(registered.registered());
    assert!(!registered.foreign_entries);
}

#[test]
fn the_git_tracked_preserve_fact_is_a_separate_read_only_assessment() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    write(
        &root.join(".claude/settings.json"),
        &claude_settings(
            Some("spec-kitty session-start"),
            Some("spec-kitty session-stop"),
            "",
        ),
    );
    let untracked = assess_claude_hooks(root);
    assert_eq!(untracked.tracked, Some(false), "outside a repository");

    let init = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(root)
        .status()
        .expect("git init");
    assert!(init.success());
    let add = std::process::Command::new("git")
        .args(["add", "--", ".claude/settings.json"])
        .current_dir(root)
        .status()
        .expect("git add");
    assert!(add.success());
    let tracked = assess_claude_hooks(root);
    assert_eq!(tracked.tracked, Some(true));
    assert!(tracked.registered());
}

#[test]
fn an_undecodable_settings_file_is_unsafe_and_never_healthy() {
    let temp = tempfile::tempdir().expect("temp");
    configure(temp.path(), &["claude"]);
    // cp1252 bytes: not UTF-8 and no byte-order mark.
    write_bytes(
        &temp.path().join(".claude/settings.json"),
        b"{\"hooks\": {\"Stop\": [{\"hooks\": [{\"type\": \"command\", \"command\": \"caf\xe9\"}]}]}}",
    );
    let report = audit(temp.path(), &["claude"], &[SurfaceKind::Hook]);
    assert_eq!(report.summary.surfaces, 2);
    for status in &report.surfaces {
        assert_eq!(status.state, "unsafe");
    }
    assert_eq!(report.summary.errors, 2);
    assert!(!report.ok());
    for finding in &report.findings {
        assert_eq!(finding.code, "session-presence-undecodable");
        assert_eq!(finding.severity, "error");
        assert!(finding.message.starts_with("undecodable: re-save "));
        assert!(finding.message.ends_with(" as UTF-8"));
        assert_eq!(finding.repair_command, None);
    }

    // CF33 corrected this premise. The reference decodes an orientation file
    // with `read_text(encoding="utf-8")`, which RAISES `UnicodeDecodeError`; the
    // provider never catches it, so the audit aborts instead of reporting
    // `unsafe`. The escaping failure is asserted in
    // `cf33_session_decode_phases.rs`; here the audit must not return a report.
    let temp = tempfile::tempdir().expect("temp");
    configure(temp.path(), &["codex"]);
    write_bytes(&temp.path().join("AGENTS.md"), b"caf\xe9\n");
    let failure = audit_tool_surfaces(&SurfaceAuditRequest {
        project_root: temp.path(),
        configured_tools: &["codex".to_string()],
        tool_filter: None,
        kinds: Some(&[SurfaceKind::ContextFile]),
    })
    .expect_err("the reference lets the decode error escape the audit");
    match failure {
        ToolSurfaceAuditFailure::Session(session) => {
            assert_eq!(session.phase(), SessionSurfacePhase::Decode);
            assert_eq!(session.path(), temp.path().join("AGENTS.md").as_path());
            assert!(
                session
                    .detail()
                    .contains("'utf-8' codec can't decode byte 0xe9 in position 3"),
                "{}",
                session.detail()
            );
        }
        other => panic!("expected the session decode failure, got {other}"),
    }
}

#[test]
fn null_writer_harnesses_report_the_source_research_gap_for_every_session_kind() {
    let temp = tempfile::tempdir().expect("temp");
    configure(temp.path(), &["qwen"]);
    for kind in [
        SurfaceKind::ContextFile,
        SurfaceKind::Rule,
        SurfaceKind::Hook,
    ] {
        let report = audit(temp.path(), &["qwen"], &[kind]);
        assert_eq!(report.summary.surfaces, 1, "{kind:?}");
        let status = &report.surfaces[0];
        assert_eq!(status.state, "not_applicable", "{kind:?}");
        // The reference's research-gap instance always carries the
        // context_file definition, even when the filter asked for another kind.
        assert_eq!(status.instance.definition.kind, SurfaceKind::ContextFile);
        assert_eq!(report.findings[0].code, "research-gap-surface");
        assert_eq!(report.findings[0].severity, "info");
        assert_eq!(
            report.findings[0].message,
            "No known session-presence mechanism for qwen."
        );
        assert_eq!(report.summary.errors, 0, "{kind:?}");
        assert!(report.ok(), "{kind:?}");
    }
}

#[test]
fn tool_and_kind_filters_keep_session_kinds_exact() {
    let temp = tempfile::tempdir().expect("temp");
    configure(temp.path(), &["claude", "codex"]);
    let configured: Vec<String> = ["claude".to_string(), "codex".to_string()].to_vec();

    let hooks = audit_tool_surfaces(&SurfaceAuditRequest {
        project_root: temp.path(),
        configured_tools: &configured,
        tool_filter: Some("codex"),
        kinds: Some(&[SurfaceKind::Hook]),
    })
    .expect("audit");
    assert_eq!(
        hooks.summary.surfaces, 0,
        "a harness with no hook mechanism yields no hook instance"
    );

    let hooks = audit(temp.path(), &["claude", "codex"], &[SurfaceKind::Hook]);
    assert_eq!(hooks.summary.surfaces, 2);
    assert!(
        hooks
            .surfaces
            .iter()
            .all(|status| status.instance.owner == "claude")
    );

    let rules = audit_tool_surfaces(&SurfaceAuditRequest {
        project_root: temp.path(),
        configured_tools: &configured,
        tool_filter: None,
        kinds: Some(&[SurfaceKind::Rule]),
    })
    .expect("audit");
    assert_eq!(
        rules.summary.surfaces, 0,
        "neither claude nor codex manages a path/glob-activated rule"
    );

    let contexts = audit(
        temp.path(),
        &["claude", "codex"],
        &[SurfaceKind::ContextFile],
    );
    assert_eq!(contexts.summary.surfaces, 2);
    let owners: Vec<&str> = contexts
        .surfaces
        .iter()
        .map(|status| status.instance.owner.as_str())
        .collect();
    assert!(owners.contains(&"claude"));
    assert!(owners.contains(&"codex"));
}

#[test]
fn every_session_audit_is_read_only_and_runs_no_projection_writer() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    configure(root, &["claude", "codex", "cursor", "kiro", "qwen"]);
    // Deliberately non-canonical content: any projection writer would rewrite
    // these files, so an unchanged snapshot proves the audit never ran one.
    write(
        &root.join("AGENTS.md"),
        "team notes, no orientation block\n",
    );
    write(
        &root.join(".claude/CLAUDE.md"),
        &orientation("0.0.1", "demo"),
    );
    write(
        &root.join(".claude/settings.json"),
        &claude_settings(Some("spec-kitty session-stop"), None, ""),
    );
    write(&root.join(".cursor/rules/spec-kitty.mdc"), "stale rules\n");
    write(
        &root.join(".kiro/steering/spec-kitty.md"),
        "stale steering\n",
    );
    write(
        &root.join(".kittify/config.yaml"),
        "agents:\n  available:\n  - claude\n  - codex\n  - cursor\n  - kiro\n  - qwen\n",
    );

    let before = snapshot(root);
    let configured = ["claude", "codex", "cursor", "kiro", "qwen"];
    for kind in [
        SurfaceKind::ContextFile,
        SurfaceKind::Rule,
        SurfaceKind::Hook,
    ] {
        let report = audit(root, &configured, &[kind]);
        assert!(!report.surfaces.is_empty(), "{kind:?} planned no instance");
    }
    let after = snapshot(root);
    assert_eq!(
        before, after,
        "a read-only audit must not change bytes, modes, inodes or mtimes"
    );
    // The same guarantee for the assessment seam alone.
    let before = snapshot(root);
    let _ = observe_claude_settings(root);
    let _ = assess_orientation(&root.join(".claude/CLAUDE.md"), COMPAT_CORE_VERSION);
    for key in configured {
        for artefact in session_artefacts(key) {
            let _ = artefact.kind;
        }
    }
    assert_eq!(before, snapshot(root));
}
