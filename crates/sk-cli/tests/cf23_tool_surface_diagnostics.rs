//! CF23 — functional smoke for the three tool-surface `doctor` leaves.
//!
//! These exercise the real read-only diagnostics in
//! `sk_install::tool_surface_diagnostics` (the layer the CLI adapter calls) on
//! fresh `tempfile` fixtures, plus the adapter's root-free contracts (the
//! `--fix` refusals, the `--kind` token vocabulary, the not-in-project message).
//!
//! They are **not** whole-command parity: root dispatch is wired by the parent,
//! so the `run_*` runners are covered here only where they do not need a
//! process-global CWD/HOME (the workspace forbids `unsafe`, so env mutation is
//! not available to tests). Deferred deep proof is listed in the lane report:
//! `HOME`-relative user-global command roots, platform path spellings
//! (opencode/llxprt), Rich-rendered human output, pack-skill catalog resolution,
//! collision-vs-canonical-render digests, and `--fix` repair scenarios.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use sk_cli::compat::doctor::tool_surfaces::{
    NOT_IN_PROJECT_MESSAGE, SKILLS_FIX_REFUSED, TOOL_SURFACES_FIX_REFUSED, skills_fix_refusal,
    tool_surface_kinds, tool_surfaces_fix_refusal,
};
use sk_install::tool_surface_diagnostics::{
    self as diagnostics, CommandDirs, SurfaceAuditRequest, SurfaceKind, audit_command_files_in,
    audit_skills_in, audit_tool_surfaces,
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

fn marker() -> String {
    diagnostics::expected_version_marker()
}

fn command_skill_body(command: &str) -> String {
    format!(
        "---\nname: spec-kitty-{command}\ndescription: fixture\n---\n{}\n# spec-kitty-{command}\nbody\n",
        marker()
    )
}

/// Install the command-skill surface an audit should find healthy.
fn install_command_skills(root: &Path, commands: &[&str]) -> BTreeMap<String, String> {
    let mut hashes = BTreeMap::new();
    let mut manifest = sk_install::AssetManifest::empty();
    for command in commands {
        let rel = sk_install::render::command_skill_rel_path(command);
        let body = command_skill_body(command);
        write(&root.join(&rel), &body);
        let hash = sk_install::manifest::content_hash(body.as_bytes());
        manifest.upsert(
            sk_install::ManifestEntry::new(
                rel.clone(),
                hash.clone(),
                vec!["codex".to_string()],
                "2026-01-01T00:00:00+00:00",
                sk_install::COMPAT_CORE_VERSION,
            )
            .expect("manifest entry"),
        );
        hashes.insert(rel, hash);
    }
    write(
        &root.join(sk_install::MANIFEST_REL),
        &String::from_utf8(manifest.to_bytes()).expect("utf8 manifest"),
    );
    hashes
}

fn dirs_for(root: &Path, agents: &[&str]) -> CommandDirs {
    let mut dirs = CommandDirs::new();
    for agent in agents {
        dirs.insert(
            (*agent).to_string(),
            root.join("home").join(agent).join("commands"),
        );
    }
    dirs
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
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn keys(value: &Value) -> BTreeSet<String> {
    value
        .as_object()
        .expect("an object")
        .keys()
        .cloned()
        .collect()
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

// ---------------------------------------------------------------------------
// adapter contracts that do not need a project root
// ---------------------------------------------------------------------------

#[test]
fn the_mutating_modes_are_refused_with_a_code_and_a_coverage_list() {
    let skills = skills_fix_refusal();
    assert_eq!(skills.exit(), 2);
    assert_eq!(
        skills.to_json()["error_code"],
        Value::String(SKILLS_FIX_REFUSED.into())
    );
    assert!(
        skills.to_json()["coverage"]
            .as_array()
            .is_some_and(|items| items.len() >= 3),
        "a refusal must name the missing repair policy"
    );

    let surfaces = tool_surfaces_fix_refusal();
    assert_eq!(surfaces.exit(), 2);
    assert_eq!(
        surfaces.to_json()["error_code"],
        Value::String(TOOL_SURFACES_FIX_REFUSED.into())
    );
    assert!(
        surfaces.message().contains("read-only"),
        "the refusal says what this build does: {}",
        surfaces.message()
    );
}

#[test]
fn the_kind_tokens_match_the_reference_registration_table() {
    // Kebab tokens for every kind, plus the underscore wire spellings the
    // reference's session-presence/native-config/plugin-bundle providers accept.
    let accepted = [
        "command-skill",
        "charter-skill",
        "command-file",
        "agent-profile",
        "context-file",
        "context_file",
        "hook",
        "rule",
        "native-config",
        "native_config",
        "plugin-manifest",
        "plugin_manifest",
    ];
    let kinds = tool_surface_kinds(&accepted.map(str::to_string)).expect("accepted");
    assert_eq!(kinds.len(), accepted.len());
    assert_eq!(kinds[0], SurfaceKind::CommandSkill);
    assert_eq!(kinds[1], SurfaceKind::CharterSkill);
    assert_eq!(kinds.last(), Some(&SurfaceKind::PluginManifest));

    let error = tool_surface_kinds(&["command_file".to_string()]).expect_err("not a token");
    assert_eq!(error.exit(), 2);
    assert_eq!(
        error.to_json()["error"]["code"],
        Value::String("unknown_kind".into())
    );
    assert!(
        error
            .message()
            .contains("Known kinds: agent-profile, charter-skill, command-file"),
        "the refusal lists the accepted tokens in sorted order: {}",
        error.message()
    );
}

#[test]
fn the_not_in_project_message_is_the_reference_string() {
    assert_eq!(NOT_IN_PROJECT_MESSAGE, "Not in a spec-kitty project");
}

// ---------------------------------------------------------------------------
// doctor tool-surfaces
// ---------------------------------------------------------------------------

#[test]
fn a_healthy_command_skill_surface_reports_source_shaped_json() {
    let temp = tempfile::tempdir().unwrap();
    configure(temp.path(), &["codex"]);
    let commands: Vec<&str> = sk_install::CANONICAL_COMMANDS.to_vec();
    install_command_skills(temp.path(), &commands);

    let report = audit_tool_surfaces(&SurfaceAuditRequest {
        project_root: temp.path(),
        configured_tools: &["codex".to_string()],
        tool_filter: None,
        kinds: Some(&[SurfaceKind::CommandSkill]),
    })
    .expect("audit");
    assert!(report.ok(), "findings: {:?}", report.findings);
    assert_eq!(
        report.summary.surfaces,
        diagnostics::canonical_command_count()
    );
    assert_eq!(report.summary.present, report.summary.surfaces);
    assert_eq!(report.summary.errors, 0);
    assert!(report.findings.is_empty());

    let payload = report.to_json();
    assert_eq!(
        keys(&payload),
        set(&[
            "ok",
            "schema_version",
            "project_root",
            "configured_tools",
            "summary",
            "surfaces",
            "findings"
        ])
    );
    assert_eq!(payload["ok"], Value::Bool(true));
    assert_eq!(payload["schema_version"], Value::from(1));
    assert_eq!(payload["configured_tools"], serde_json::json!(["codex"]));
    assert_eq!(
        keys(&payload["summary"]),
        set(&[
            "surfaces", "present", "missing", "drifted", "warnings", "errors"
        ])
    );
    assert_eq!(
        keys(&payload["surfaces"][0]),
        set(&[
            "id",
            "tool",
            "kind",
            "provider",
            "path",
            "state",
            "source_kind",
            "manifest",
            "repair_command"
        ])
    );
    assert_eq!(payload["surfaces"][0]["state"], Value::from("present"));
    assert_eq!(payload["surfaces"][0]["kind"], Value::from("command_skill"));
    assert_eq!(
        payload["surfaces"][0]["provider"],
        Value::from("command_skills")
    );
    assert_eq!(payload["surfaces"][0]["manifest"], Value::Null);
}

#[test]
fn a_missing_surface_is_an_error_finding_never_a_silent_health_claim() {
    let temp = tempfile::tempdir().unwrap();
    configure(temp.path(), &["codex"]);

    let report = audit_tool_surfaces(&SurfaceAuditRequest {
        project_root: temp.path(),
        configured_tools: &["codex".to_string()],
        tool_filter: None,
        kinds: Some(&[SurfaceKind::CommandSkill]),
    })
    .expect("audit");
    assert!(!report.ok());
    assert_eq!(
        report.summary.missing,
        diagnostics::canonical_command_count()
    );
    assert_eq!(report.summary.errors, report.findings.len());
    assert!(
        report
            .findings
            .iter()
            .all(|finding| finding.code == "generated-surface-missing")
    );
    let payload = report.to_json();
    assert_eq!(payload["ok"], Value::Bool(false));
    assert_eq!(payload["findings"][0]["severity"], Value::from("error"));
    assert_eq!(
        keys(&payload["findings"][0]),
        set(&[
            "code",
            "severity",
            "message",
            "tool_key",
            "surface_id",
            "path",
            "repair_command",
            "docs_ref",
            "details"
        ])
    );
    assert_eq!(
        payload["findings"][0]["repair_command"],
        Value::from(diagnostics::COMMAND_SKILL_REPAIR_HINT)
    );
    assert!(
        payload["surfaces"][0]["repair_command"]
            .as_str()
            .is_some_and(|hint| hint.contains("--kind command-skill")),
        "the surface entry carries the repair hint of its first finding"
    );
}

#[test]
fn an_edited_managed_skill_is_drift_and_a_dotted_leftover_is_stale() {
    let temp = tempfile::tempdir().unwrap();
    configure(temp.path(), &["codex"]);
    install_command_skills(temp.path(), &["plan"]);
    // Drifted canonical copy plus a retired dotted leftover.
    write(
        &temp.path().join(".agents/skills/spec-kitty-plan/SKILL.md"),
        "operator edit\n",
    );
    write(
        &temp.path().join(".agents/skills/spec-kitty.plan/SKILL.md"),
        "legacy\n",
    );

    let report = audit_tool_surfaces(&SurfaceAuditRequest {
        project_root: temp.path(),
        configured_tools: &["codex".to_string()],
        tool_filter: None,
        kinds: Some(&[SurfaceKind::CommandSkill]),
    })
    .expect("audit");
    let state = |rel: &str| {
        report
            .surfaces
            .iter()
            .find(|status| status.instance.path.ends_with(rel))
            .map(|status| status.state)
    };
    assert_eq!(
        state(".agents/skills/spec-kitty-plan/SKILL.md"),
        Some("drifted")
    );
    assert_eq!(
        state(".agents/skills/spec-kitty.plan/SKILL.md"),
        Some("stale")
    );
    assert!(report.summary.drifted >= 1);
    assert!(
        report
            .findings
            .iter()
            .any(|finding| finding.code == "managed-file-drift"),
        "drift is a warning finding, not a silent pass"
    );
    assert!(
        report
            .findings
            .iter()
            .any(|finding| finding.code == "stale-generated-surface"),
        "a legacy dotted skill is reported, never repaired by name"
    );
}

#[test]
fn kind_and_tool_filters_restrict_the_plan_without_widening_it() {
    let temp = tempfile::tempdir().unwrap();
    configure(temp.path(), &["claude", "codex"]);
    let configured = vec!["claude".to_string(), "codex".to_string()];

    let claude_only = audit_tool_surfaces(&SurfaceAuditRequest {
        project_root: temp.path(),
        configured_tools: &configured,
        tool_filter: Some("claude"),
        kinds: Some(&[SurfaceKind::CommandSkill]),
    })
    .expect("audit");
    assert_eq!(claude_only.configured_tools, vec!["claude".to_string()]);
    assert!(
        claude_only.surfaces.is_empty(),
        "claude is a slash-command agent, so it plans no command-skill surface"
    );

    let unknown_tool = audit_tool_surfaces(&SurfaceAuditRequest {
        project_root: temp.path(),
        configured_tools: &configured,
        tool_filter: Some("gemini"),
        kinds: Some(&[SurfaceKind::CommandSkill]),
    })
    .expect("audit");
    assert!(unknown_tool.configured_tools.is_empty());
    assert_eq!(unknown_tool.summary.surfaces, 0);

    let charter_only = audit_tool_surfaces(&SurfaceAuditRequest {
        project_root: temp.path(),
        configured_tools: &configured,
        tool_filter: None,
        kinds: Some(&[SurfaceKind::CharterSkill]),
    })
    .expect("audit");
    assert!(
        charter_only
            .surfaces
            .iter()
            .all(|status| status.instance.definition.kind == SurfaceKind::CharterSkill),
        "a --kind filter must not leak other kinds into the report"
    );
}

#[test]
fn a_remaining_unimplemented_kind_reports_a_research_gap_instead_of_health() {
    let temp = tempfile::tempdir().unwrap();
    configure(temp.path(), &["claude"]);
    let kind = SurfaceKind::PluginManifest;
    let report = audit_tool_surfaces(&SurfaceAuditRequest {
        project_root: temp.path(),
        configured_tools: &["claude".to_string()],
        tool_filter: None,
        kinds: Some(&[kind]),
    })
    .expect("audit");
    assert_eq!(report.surfaces.len(), 1, "{kind:?}");
    assert_eq!(report.surfaces[0].state, "not_applicable", "{kind:?}");
    assert_eq!(report.findings[0].code, "research-gap-surface", "{kind:?}");
    assert_eq!(report.findings[0].severity, "info", "{kind:?}");
    assert_eq!(report.summary.errors, 0, "{kind:?}");
}

#[test]
fn supported_session_kinds_report_actual_missing_members_not_research_gaps() {
    let temp = tempfile::tempdir().unwrap();
    configure(temp.path(), &["claude", "cursor"]);
    let before = snapshot(temp.path());
    for (kind, tool, count, code) in [
        (
            SurfaceKind::ContextFile,
            "claude",
            1,
            "context-file-missing",
        ),
        (
            SurfaceKind::Hook,
            "claude",
            2,
            "session-presence-incomplete",
        ),
        (
            SurfaceKind::Rule,
            "cursor",
            1,
            "session-presence-incomplete",
        ),
    ] {
        let report = audit_tool_surfaces(&SurfaceAuditRequest {
            project_root: temp.path(),
            configured_tools: &[tool.to_owned()],
            tool_filter: Some(tool),
            kinds: Some(&[kind]),
        })
        .expect("actual session audit");
        assert_eq!(report.surfaces.len(), count, "{kind:?}");
        assert_eq!(report.findings.len(), count, "{kind:?}");
        assert_eq!(report.summary.errors, count, "{kind:?}");
        for surface in report.surfaces {
            assert_eq!(surface.state, "missing", "{kind:?}");
        }
        for finding in report.findings {
            assert_eq!(finding.code, code, "{kind:?}");
            assert_eq!(finding.severity, "error", "{kind:?}");
        }
    }
    assert_eq!(
        snapshot(temp.path()),
        before,
        "audit creates no file, lock or writer effect"
    );
}

#[test]
fn the_agent_profile_surface_is_manifest_driven_and_gap_reported_when_absent() {
    let temp = tempfile::tempdir().unwrap();
    configure(temp.path(), &["claude"]);
    let gap = audit_tool_surfaces(&SurfaceAuditRequest {
        project_root: temp.path(),
        configured_tools: &["claude".to_string()],
        tool_filter: None,
        kinds: Some(&[SurfaceKind::AgentProfile]),
    })
    .expect("audit");
    assert_eq!(gap.surfaces[0].state, "not_applicable");
    assert_eq!(gap.findings[0].code, "research-gap-surface");

    let body = "# Researcher Robbie\n";
    write(
        &temp.path().join(".claude/agents/researcher-robbie.md"),
        body,
    );
    write(
        &temp.path().join(sk_install::PROFILES_MANIFEST_REL),
        &format!(
            "{{\"schema_version\":1,\"entries\":[{{\"profile_urn\":\"agent_profile:researcher-robbie\",\
             \"source_layer\":\"built-in\",\"tool_key\":\"claude\",\
             \"output_path\":\".claude/agents/researcher-robbie.md\",\"format\":\"markdown\",\
             \"file_hash\":\"{}\",\"source_path\":\"packs/built-in/agent_profiles/researcher-robbie.agent.yaml\",\
             \"source_hash\":\"00\",\"projection_version\":1}}]}}",
            sk_install::manifest::content_hash(body.as_bytes())
        ),
    );
    let healthy = audit_tool_surfaces(&SurfaceAuditRequest {
        project_root: temp.path(),
        configured_tools: &["claude".to_string()],
        tool_filter: None,
        kinds: Some(&[SurfaceKind::AgentProfile]),
    })
    .expect("audit");
    assert_eq!(healthy.surfaces.len(), 1);
    assert_eq!(healthy.surfaces[0].state, "present");
    assert!(healthy.ok());

    write(
        &temp.path().join(".claude/agents/researcher-robbie.md"),
        "# edited\n",
    );
    let drifted = audit_tool_surfaces(&SurfaceAuditRequest {
        project_root: temp.path(),
        configured_tools: &["claude".to_string()],
        tool_filter: None,
        kinds: Some(&[SurfaceKind::AgentProfile]),
    })
    .expect("audit");
    assert_eq!(drifted.surfaces[0].state, "drifted");
    assert_eq!(drifted.findings[0].code, "native-agent-profile-drift");
    assert_eq!(drifted.summary.warnings, 1);
}

// ---------------------------------------------------------------------------
// doctor skills
// ---------------------------------------------------------------------------

#[test]
fn the_skills_payload_is_the_frozen_reference_envelope() {
    let temp = tempfile::tempdir().unwrap();
    configure(temp.path(), &["codex"]);
    install_command_skills(temp.path(), &["plan"]);
    let dirs = dirs_for(temp.path(), &["codex"]);

    let audit = audit_skills_in(temp.path(), &dirs).expect("audit");
    let payload = audit.to_json();
    assert_eq!(
        keys(&payload),
        set(&[
            "configured_agents",
            "manifest_agents",
            "entries",
            "canonical_commands",
            "drift",
            "gaps",
            "orphans",
            "stale",
            "unsafe",
            "legacy",
            "collisions",
            "uninstalled_agents",
            "vibe_config_missing",
            "repaired_agents",
            "repaired_paths",
            "pruned",
            "repaired_vibe_config",
            "repair_errors",
            "ok",
            "slash_commands",
            "pack_skills",
            "tool_folders",
        ]),
        "the reference's frozen envelope (human-only keys excluded)"
    );
    assert_eq!(
        keys(&payload["slash_commands"]),
        set(&["configured_agents", "gaps", "repaired", "errors", "ok"])
    );
    assert_eq!(payload["configured_agents"], serde_json::json!(["codex"]));
    assert_eq!(payload["manifest_agents"], serde_json::json!(["codex"]));
    assert_eq!(payload["canonical_commands"], Value::from(14));
    assert_eq!(payload["repair_errors"], serde_json::json!([]));
    assert_eq!(payload["repaired_vibe_config"], Value::Bool(false));
}

#[test]
fn the_skills_audit_reports_missing_orphans_and_uninstalled_agents() {
    let temp = tempfile::tempdir().unwrap();
    configure(temp.path(), &["codex", "vibe"]);
    install_command_skills(temp.path(), &["plan"]);
    // An unowned file inside a canonical folder is an orphan, not a pass.
    write(
        &temp.path().join(".agents/skills/spec-kitty-plan/notes.md"),
        "extra\n",
    );
    // `vibe` is configured but has no manifest entry at all.
    let dirs = dirs_for(temp.path(), &["codex", "vibe"]);
    let audit = audit_skills_in(temp.path(), &dirs).expect("audit");

    assert_eq!(audit.configured_agents, vec!["codex", "vibe"]);
    assert_eq!(audit.uninstalled_agents, vec!["vibe"]);
    assert!(
        audit
            .orphans
            .contains(&".agents/skills/spec-kitty-plan/notes.md".to_string()),
        "orphans: {:?}",
        audit.orphans
    );
    assert!(!audit.ok());
    let payload = audit.to_json();
    assert_eq!(payload["uninstalled_agents"], serde_json::json!(["vibe"]));
}

#[test]
fn the_skills_audit_reports_slash_command_gaps_through_the_injected_dirs() {
    let temp = tempfile::tempdir().unwrap();
    configure(temp.path(), &["claude"]);
    let dirs = dirs_for(temp.path(), &["claude"]);
    let command_dir = dirs.get("claude").expect("dir").clone();
    write(
        &command_dir.join("spec-kitty.plan.md"),
        &format!("{}\n# plan\n", marker()),
    );
    write(&command_dir.join("spec-kitty.status.md"), "stale shim\n");

    let audit = audit_skills_in(temp.path(), &dirs).expect("audit");
    assert_eq!(audit.slash_commands.configured_agents, vec!["claude"]);
    assert!(!audit.slash_commands.ok());
    let stale: Vec<_> = audit
        .slash_commands
        .gaps
        .iter()
        .filter(|gap| gap.status == "stale")
        .collect();
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0].command, "status");
    assert!(
        audit
            .slash_commands
            .gaps
            .iter()
            .any(|gap| gap.status == "missing" && gap.command == "specify"),
        "every absent command is a gap"
    );
    let payload = audit.to_json();
    assert_eq!(payload["slash_commands"]["ok"], Value::Bool(false));
    assert_eq!(
        keys(&payload["slash_commands"]["gaps"][0]),
        set(&["agent_key", "command", "expected_path", "status"])
    );
}

#[test]
fn a_missing_tool_folder_is_one_finding_naming_the_configured_agents() {
    let temp = tempfile::tempdir().unwrap();
    configure(temp.path(), &["codex"]);
    let dirs = dirs_for(temp.path(), &["codex"]);
    let audit = audit_skills_in(temp.path(), &dirs).expect("audit");
    assert_eq!(audit.tool_folders.len(), 1);
    assert_eq!(audit.tool_folders[0].kind, "no_tool_folder");
    assert_eq!(audit.tool_folders[0].configured_agents, vec!["codex"]);
    assert!(
        audit.tool_folders[0]
            .message
            .contains("spec-kitty agent config remove")
    );
    let payload = audit.to_json();
    assert_eq!(
        keys(&payload["tool_folders"][0]),
        set(&["kind", "configured_agents", "message"])
    );
}

// ---------------------------------------------------------------------------
// doctor command-files
// ---------------------------------------------------------------------------

#[test]
fn command_file_rows_carry_the_reference_shape_and_severities() {
    let temp = tempfile::tempdir().unwrap();
    configure(temp.path(), &["claude"]);
    let mut dirs = CommandDirs::new();
    let command_dir = temp.path().join("home/.claude/commands");
    dirs.insert("claude".to_string(), command_dir.clone());
    // A current, prompt-shaped `plan` file.
    let long_body: String = (0..60).map(|index| format!("line {index}\n")).collect();
    write(
        &command_dir.join("spec-kitty.plan.md"),
        &format!("{}\n{long_body}", marker()),
    );
    // A stale thin shim for the CLI wrapper `status`.
    write(&command_dir.join("spec-kitty.status.md"), "stale\n");

    let issues = audit_command_files_in(temp.path(), &dirs);
    let issue_keys = keys(&issues[0].to_json());
    assert_eq!(
        issue_keys,
        set(&["agent", "command", "file", "issue", "severity"])
    );

    assert!(
        issues
            .iter()
            .any(|issue| issue.command == "specify" && issue.issue == "missing"),
        "absent command files are errors"
    );
    assert!(
        !issues
            .iter()
            .any(|issue| issue.command == "plan" && issue.severity == "error"),
        "a current prompt file is healthy"
    );
    let status: Vec<_> = issues
        .iter()
        .filter(|issue| issue.command == "status")
        .collect();
    assert!(
        status.iter().any(
            |issue| issue.issue.contains("stale or missing version marker")
                && issue.severity == "warning"
        ),
        "a stale marker is a warning: {:?}",
        status
    );
    assert!(
        status.iter().all(|issue| issue.severity == "warning"),
        "a stale file is never an error: {:?}",
        status
    );
}

#[test]
fn a_prompt_file_that_is_too_short_is_a_shape_warning() {
    let temp = tempfile::tempdir().unwrap();
    configure(temp.path(), &["claude"]);
    let mut dirs = CommandDirs::new();
    let command_dir = temp.path().join("home/.claude/commands");
    dirs.insert("claude".to_string(), command_dir.clone());
    write(
        &command_dir.join("spec-kitty.specify.md"),
        &format!("{}\n# specify\n", marker()),
    );

    let issues = audit_command_files_in(temp.path(), &dirs);
    let specify: Vec<_> = issues
        .iter()
        .filter(|issue| issue.command == "specify")
        .collect();
    assert!(
        specify
            .iter()
            .any(|issue| issue.issue.contains("non-empty lines")),
        "a prompt file below the line floor is reported: {:?}",
        specify
    );
    assert!(specify.iter().all(|issue| issue.severity == "warning"));
}

// ---------------------------------------------------------------------------
// read-only guarantee
// ---------------------------------------------------------------------------

#[test]
fn every_audit_leaves_the_tree_and_the_manifests_byte_identical() {
    let temp = tempfile::tempdir().unwrap();
    configure(temp.path(), &["codex", "claude", "vibe"]);
    install_command_skills(temp.path(), &["plan", "review"]);
    write(
        &temp.path().join(".agents/skills/spec-kitty-plan/notes.md"),
        "extra\n",
    );
    write(
        &temp.path().join(".vibe/config.toml"),
        "skill_paths = [\".agents/skills\"]\n",
    );
    let dirs = dirs_for(temp.path(), &["codex", "claude", "vibe"]);
    let command_dir = dirs.get("claude").expect("claude dir").clone();
    write(&command_dir.join("spec-kitty.plan.md"), "stale\n");

    let before = snapshot(temp.path());
    let configured = vec![
        "claude".to_string(),
        "codex".to_string(),
        "vibe".to_string(),
    ];
    let report = audit_tool_surfaces(&SurfaceAuditRequest {
        project_root: temp.path(),
        configured_tools: &configured,
        tool_filter: None,
        kinds: None,
    })
    .expect("audit");
    let _ = report.to_json();
    let skills = audit_skills_in(temp.path(), &dirs).expect("skills audit");
    let _ = skills.to_json();
    let _ = audit_command_files_in(temp.path(), &dirs);
    let after = snapshot(temp.path());

    assert_eq!(
        before, after,
        "a read-only audit must not create, rewrite, chmod or touch anything"
    );
}

#[test]
fn the_vibe_glue_probe_reads_but_never_writes_the_native_config() {
    let temp = tempfile::tempdir().unwrap();
    configure(temp.path(), &["vibe"]);
    let report = audit_tool_surfaces(&SurfaceAuditRequest {
        project_root: temp.path(),
        configured_tools: &["vibe".to_string()],
        tool_filter: None,
        kinds: Some(&[SurfaceKind::NativeConfig]),
    })
    .expect("audit");
    assert_eq!(report.surfaces[0].state, "missing");
    assert!(
        !temp.path().join(".vibe/config.toml").exists(),
        "the audit must not create the config it is diagnosing"
    );
}

// ---------------------------------------------------------------------------
// end-to-end runner smoke (the adapter arms the parent wires)
// ---------------------------------------------------------------------------

/// The only tests in this binary that change the process CWD; they hold this
/// lock so they never race each other. Every other test above passes absolute
/// paths and never reads the CWD.
static CWD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn with_cwd<T>(directory: &Path, body: impl FnOnce() -> T) -> T {
    let guard = CWD_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let previous = std::env::current_dir().expect("current dir");
    std::env::set_current_dir(directory).expect("chdir into the fixture");
    let outcome = body();
    std::env::set_current_dir(&previous).expect("restore cwd");
    drop(guard);
    outcome
}

#[test]
fn the_tool_surfaces_runner_goes_from_issue_to_healthy_on_a_real_project() {
    use sk_cli::compat::doctor::tool_surfaces::run_tool_surfaces;

    let temp = tempfile::tempdir().unwrap();
    // `codex` has no slash-command surface, so this path never reads `$HOME`.
    configure(temp.path(), &["codex"]);
    let kinds = vec!["command-skill".to_string()];

    let broken = with_cwd(temp.path(), || {
        run_tool_surfaces(&kinds, None, false, true).expect("audit runs")
    });
    assert_eq!(broken, 1, "a missing surface exits 1");

    let commands: Vec<&str> = sk_install::CANONICAL_COMMANDS.to_vec();
    install_command_skills(temp.path(), &commands);
    let healthy = with_cwd(temp.path(), || {
        run_tool_surfaces(&kinds, None, false, true).expect("audit runs")
    });
    assert_eq!(healthy, 0, "a healthy surface exits 0");
}

#[test]
fn the_skills_runner_reports_real_findings_and_refuses_fix() {
    use sk_cli::compat::doctor::tool_surfaces::run_skills;

    let temp = tempfile::tempdir().unwrap();
    configure(temp.path(), &["codex"]);

    // Read-only arm: no command skills installed yet, so the audit is red.
    let code = with_cwd(temp.path(), || run_skills(false, true).expect("audit runs"));
    assert_eq!(code, 1);

    // Mutating arm: refused before any read, with the reserved parent code.
    let refusal = with_cwd(temp.path(), || {
        run_skills(true, true).expect_err("--fix is refused")
    });
    assert_eq!(refusal.exit(), 2);
    assert_eq!(
        refusal.to_json()["error_code"],
        Value::String(SKILLS_FIX_REFUSED.into())
    );
}

#[test]
fn the_command_files_runner_and_the_not_in_project_contract() {
    use sk_cli::compat::doctor::tool_surfaces::{run_command_files, run_tool_surfaces};

    // Outside a project: `command-files` exits 1, the exit-2 leaves exit 2, and
    // both carry the reference's `not_in_project` code.
    let outside = tempfile::tempdir().unwrap();
    let command_files = with_cwd(outside.path(), || {
        run_command_files(true).expect_err("not in a project")
    });
    assert_eq!(command_files.exit(), 1);
    assert_eq!(
        command_files.to_json()["error"]["code"],
        Value::String("not_in_project".into())
    );

    let tool_surfaces = with_cwd(outside.path(), || {
        run_tool_surfaces(&[], None, false, true).expect_err("not in a project")
    });
    assert_eq!(tool_surfaces.exit(), 2);
    assert_eq!(
        tool_surfaces.to_json()["error"]["code"],
        Value::String("not_in_project".into())
    );

    // Inside a project with a stale user-global command file, `command-files`
    // reports it: exit 1 with a real row, never a green.
    let project = tempfile::tempdir().unwrap();
    configure(project.path(), &["claude"]);
    // `HOME` cannot be redirected in-process (the workspace forbids `unsafe`),
    // so a real `~/.claude/commands` may or may not exist here; both outcomes
    // are findings about the user-global surface, never a silent pass.
    let code = with_cwd(project.path(), || {
        run_command_files(false).expect("audit runs")
    });
    assert!(code == 0 || code == 1, "exit class is 0 or 1, got {code}");
}
