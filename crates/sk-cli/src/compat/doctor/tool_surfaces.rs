//! `doctor command-files`, `doctor skills` and `doctor tool-surfaces` — the
//! read-only tool-surface diagnostics.
//!
//! These three leaves were refused with a named code before this adapter
//! existed. They now call the canonical diagnostics in
//! [`sk_install::tool_surface_diagnostics`], which owns the surface plan, the
//! read-only probes, the finding vocabulary and the JSON documents; this module
//! only resolves the project root, renders, and applies the exit contract.
//!
//! # Exit contract (the reference's, preserved)
//!
//! | command | clean | findings | not in a project | usage |
//! |---|---|---|---|---|
//! | `command-files` | 0 | 1 | **1** | — |
//! | `skills` | 0 | 1 | 2 | 2 (`--fix` refused) |
//! | `tool-surfaces` | 0 | 1 | 2 | 2 (unknown `--kind`, `--fix` refused) |
//!
//! `command-files` is the odd one out upstream: its root-resolution guard exits
//! 1, not 2, and it is preserved here rather than "fixed".
//!
//! # Escaping read/decode failure (CF33)
//!
//! `tool-surfaces` is the one leaf whose provider can raise something the
//! reference never catches: an undecodable orientation file
//! (`UnicodeDecodeError`) or an unreadable `.claude/settings.json` (`OSError`).
//! Upstream that exception escapes and the command exits 1 with no report; here
//! [`map_audit_failure`] preserves the non-zero, no-document outcome instead of
//! reporting a health verdict.
//!
//! # Read-only guarantee
//!
//! Every arm is an audit. `--fix` is refused **before** any read, hash or
//! write, with a named code and the remaining coverage, because the reference's
//! repair policy (legacy retirement, collision skips, `prune_stale`, the Vibe
//! config write, pack-skill reprojection) is not implemented natively yet. A
//! refusal is honest; a repair that silently skipped those rules would not be.
//!
//! # Not implemented here
//!
//! The parent owns root dispatch, the `IMPLEMENTED_LEAVES`/`REFUSED_LEAVES`
//! arrays and the capability matrix; this module exports the three runners for
//! that wiring and does not touch them.

use serde_json::json;
use sk_install::InstallError;
use sk_install::tool_surface_diagnostics::{
    CommandFileIssue, SkillsAudit, SurfaceAuditRequest, SurfaceReport, ToolSurfaceAuditFailure,
    audit_command_files, audit_skills, audit_tool_surfaces, configured_tools,
    surface_kind_from_token,
};

use crate::compat::support::{CompatError, emit_json_pretty, project_root};

/// The reference's not-in-project message (`_doctor_shared._NOT_IN_PROJECT_MESSAGE`).
pub const NOT_IN_PROJECT_MESSAGE: &str = "Not in a spec-kitty project";

/// The refusal code for `doctor skills --fix`.
pub const SKILLS_FIX_REFUSED: &str = "UNSUPPORTED_DOCTOR_SKILLS_FIX";
/// The refusal code for `doctor tool-surfaces --fix`.
pub const TOOL_SURFACES_FIX_REFUSED: &str = "UNSUPPORTED_DOCTOR_TOOL_SURFACES_FIX";

/// Run `doctor command-files` (read-only; 0 healthy / 1 issues / 1 not-in-project).
pub fn run_command_files(json_output: bool) -> Result<u8, CompatError> {
    let root = match project_root() {
        Ok(root) => root,
        Err(_) => return Err(not_in_project(1)),
    };
    let issues = audit_command_files(&root).map_err(map_install_error)?;
    if json_output {
        let rows: Vec<_> = issues.iter().map(CommandFileIssue::to_json).collect();
        emit_json_pretty(&serde_json::Value::Array(rows))
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else if issues.is_empty() {
        println!("Command Files: all files healthy");
    } else {
        render_command_files(&issues);
    }
    Ok(u8::from(!issues.is_empty()))
}

/// Run `doctor skills` (read-only; 0 ok / 1 findings / 2 not-in-project|config).
pub fn run_skills(fix: bool, json_output: bool) -> Result<u8, CompatError> {
    let root = match project_root() {
        Ok(root) => root,
        Err(_) => return Err(not_in_project(2)),
    };
    // The reference resolves the project first and then guards the mutating
    // mode, so a `--fix` outside a project is still the not-in-project refusal.
    if fix {
        return Err(skills_fix_refusal());
    }
    let audit = audit_skills(&root).map_err(config_or_manifest_refusal)?;
    if json_output {
        emit_json_pretty(&audit.to_json())
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        render_skills(&audit);
    }
    Ok(u8::from(!audit.ok()))
}

/// Run `doctor tool-surfaces [--kind …] [--tool …]` (read-only; 0 ok / 1 findings / 2).
pub fn run_tool_surfaces(
    kinds: &[String],
    tool: Option<&str>,
    fix: bool,
    json_output: bool,
) -> Result<u8, CompatError> {
    let root = match project_root() {
        Ok(root) => root,
        Err(_) => return Err(not_in_project(2)),
    };
    if fix {
        return Err(tool_surfaces_fix_refusal());
    }
    let selected = tool_surface_kinds(kinds)?;
    let configured = configured_tools(&root).map_err(config_or_manifest_refusal)?;
    let report = audit_tool_surfaces(&SurfaceAuditRequest {
        project_root: &root,
        configured_tools: &configured,
        tool_filter: tool,
        kinds: if selected.is_empty() {
            None
        } else {
            Some(&selected)
        },
    })
    .map_err(map_audit_failure)?;
    if json_output {
        emit_json_pretty(&report.to_json())
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        render_tool_surfaces(&report);
    }
    Ok(u8::from(!report.ok()))
}

/// Parse the operator's `--kind` tokens, refusing an unknown one with exit 2
/// and the reference's `unknown_kind` code (which names every accepted token).
pub fn tool_surface_kinds(
    kinds: &[String],
) -> Result<Vec<sk_install::tool_surface_diagnostics::SurfaceKind>, CompatError> {
    let mut selected = Vec::with_capacity(kinds.len());
    for token in kinds {
        match surface_kind_from_token(token) {
            Ok(kind) => selected.push(kind),
            Err(unknown) => {
                return Err(diagnostic_error(2, "unknown_kind", unknown.message()));
            }
        }
    }
    Ok(selected)
}

/// The typed `doctor skills --fix` refusal (exit 2, before any write).
pub fn skills_fix_refusal() -> CompatError {
    CompatError::unsupported(
        SKILLS_FIX_REFUSED,
        "`doctor skills --fix` reinstalls command skills, retires legacy dotted skills, prunes stale entries and repairs the Vibe skill path; this slice implements the read-only audit only",
    )
    .with(
        "coverage",
        json!([
            "command-skill manifest drift/gap/stale/orphan/legacy/collision audit (implemented, read-only)",
            "slash-command gap audit and tool-folder findings (implemented, read-only)",
            "`--fix` reinstall + prune_stale + legacy retirement + Vibe config write (missing; mutating)",
            "pack-skill projection/retirement on repair (missing; mutating)",
        ]),
    )
}

/// The typed `doctor tool-surfaces --fix` refusal (exit 2, before any write).
pub fn tool_surfaces_fix_refusal() -> CompatError {
    CompatError::unsupported(
        TOOL_SURFACES_FIX_REFUSED,
        "`doctor tool-surfaces --fix` repairs command files, managed command/charter skills, native config and agent profiles through per-provider repair batches; this slice implements the read-only audit only",
    )
    .with(
        "coverage",
        json!([
            "command-file / command-skill / charter-skill / native-config / agent-profile probes (implemented, read-only)",
            "session-presence (context_file/hook/rule) probes (implemented, read-only); plugin-manifest probes (missing)",
            "`--fix` provider repair batches through the install owner (missing; mutating)",
        ]),
    )
}

/// The reference's not-in-project refusal.
///
/// The document is shared, but `command-files` exits 1 and the other two
/// leaves exit 2. Keep both the Source shape and the command's actual exit.
fn not_in_project(exit: u8) -> CompatError {
    diagnostic_error(exit, "not_in_project", NOT_IN_PROJECT_MESSAGE)
}

/// Source's `cli.json_contract.json_error`, carried through one emission path.
fn diagnostic_error(exit: u8, code: &str, message: impl Into<String>) -> CompatError {
    let message = message.into();
    CompatError::envelope_with_exit(
        exit,
        json!({"ok": false, "error": {"code": code, "message": message}}),
        message,
    )
}

/// Translate a config/manifest read failure into the reference's exit-2 contract.
fn config_or_manifest_refusal(error: InstallError) -> CompatError {
    match &error {
        InstallError::ConfigInvalid { .. } => {
            diagnostic_error(2, "config_error", error.to_string())
        }
        _ => diagnostic_error(2, "manifest_error", error.to_string()),
    }
}

fn map_install_error(error: sk_install::InstallError) -> CompatError {
    CompatError::domain(error.to_string())
}

/// Propagate a whole-audit failure exactly where the reference lets it escape.
///
/// A session-presence read/decode failure has no handler upstream: the
/// `UnicodeDecodeError` / `OSError` propagates out of `doctor tool-surfaces`, so
/// the command exits non-zero (`1`) having printed no report. This maps it to
/// that same non-zero, no-document outcome instead of inventing a health
/// report or an `unsafe`/`missing` finding; a configuration/plan failure keeps
/// its pre-existing mapping. The reference prints a Python traceback here —
/// that wire difference is recorded as debt in the lane report, not hidden.
fn map_audit_failure(failure: ToolSurfaceAuditFailure) -> CompatError {
    match failure {
        ToolSurfaceAuditFailure::Session(session) => session_surface_failure_error(&session),
        ToolSurfaceAuditFailure::Install(error) => map_install_error(error),
    }
}

/// The one mapping from an escaping session read/decode failure to the command
/// result (exit 1, no report). Public so the CLI crate's own contract test can
/// assert it without a process-global CWD; it adds no route, flag or capability.
pub fn session_surface_failure_error(
    failure: &sk_install::tool_surface_diagnostics::SessionSurfaceFailure,
) -> CompatError {
    CompatError::domain(failure.to_string())
}

fn render_command_files(issues: &[CommandFileIssue]) {
    println!("\nCommand Files — {} issue(s) found\n", issues.len());
    println!(
        "{:<12} {:<18} {:<44} {:<8} Issue",
        "Agent", "Command", "File", "Severity"
    );
    for issue in issues {
        println!(
            "{:<12} {:<18} {:<44} {:<8} {}",
            issue.agent,
            issue.command,
            issue.file.display(),
            issue.severity,
            issue.issue
        );
    }
    println!();
}

fn render_skills(audit: &SkillsAudit) {
    if audit.ok() {
        println!(
            "Command Skills: all manifest entries healthy ({} file(s))",
            audit.entries
        );
    } else {
        println!("\nCommand Skills - issue(s) found\n");
        println!("  manifest entries      {}", audit.entries);
        println!("  drift                 {}", audit.drift.len());
        println!("  gaps                  {}", audit.gaps.len());
        println!("  orphans               {}", audit.orphans.len());
        println!("  stale                 {}", audit.stale.len());
        println!("  unsafe                {}", audit.unsafe_paths.len());
        println!("  legacy                {}", audit.legacy.len());
        println!("  collisions            {}", audit.collisions.len());
        println!("  uninstalled agents    {}", audit.uninstalled_agents.len());
        for (title, paths) in [
            (
                "Edited managed files (manual review required)",
                &audit.drift,
            ),
            ("Missing managed files", &audit.gaps),
            ("Unmanaged spec-kitty skill files", &audit.orphans),
            ("Stale managed files", &audit.stale),
            ("Unsafe managed paths", &audit.unsafe_paths),
            ("Legacy dotted command skills", &audit.legacy),
            (
                "Canonical-path collisions (user content kept)",
                &audit.collisions,
            ),
        ] {
            if paths.is_empty() {
                continue;
            }
            println!("\n{title}");
            for path in paths {
                println!("  ! {path}");
            }
        }
        if !audit.uninstalled_agents.is_empty() {
            println!("\nConfigured agents without command skills");
            for agent in &audit.uninstalled_agents {
                println!("  ! {agent}");
            }
        }
        if audit.vibe_config_missing {
            println!("\nVibe skill path is not configured in `.vibe/config.toml`");
        }
    }
    if !audit.slash_commands.configured_agents.is_empty() {
        if audit.slash_commands.ok() {
            println!(
                "✓ Slash Commands: all configured agents healthy ({} agent(s))",
                audit.slash_commands.configured_agents.len()
            );
        } else {
            println!("\nSlash Commands — gap(s) found");
            for gap in &audit.slash_commands.gaps {
                println!(
                    "  {}: {} → {}",
                    gap.agent_key,
                    gap.status,
                    gap.expected_path.display()
                );
            }
            for error in &audit.slash_commands.errors {
                println!("  ! {error}");
            }
        }
    }
    if !audit.pack_skills.is_empty() {
        println!("\nPack skills");
        for finding in &audit.pack_skills {
            println!("  ! [{}] {}", finding.kind, finding.message);
        }
    }
    for folder in &audit.tool_folders {
        println!("\n  ! [{}] {}", folder.kind, folder.message);
    }
    if !audit.ok() {
        println!(
            "\n`--fix` is refused by this build ({}); repair the findings above with the \
             canonical installer or wait for the repair slice.",
            SKILLS_FIX_REFUSED
        );
    }
}

fn render_tool_surfaces(report: &SurfaceReport) {
    if report.ok() && report.findings.is_empty() {
        println!(
            "Tool Surfaces: all surfaces healthy ({} checked)",
            report.summary.surfaces
        );
        return;
    }
    if report.ok() {
        // No error-severity finding: only research gaps and warnings. Say what
        // was actually observed instead of an unqualified "healthy".
        println!(
            "Tool Surfaces: {} surface(s) checked, {} warning(s), {} research gap(s)",
            report.summary.surfaces,
            report.summary.warnings,
            report.findings.len()
        );
    } else {
        println!("\nTool Surfaces - issue(s) found\n");
    }
    for finding in &report.findings {
        println!("  ! [{}] {}", finding.code, finding.message);
    }
    if !report.ok() {
        println!(
            "\n`--fix` is refused by this build ({}); repair the findings above with the \
             canonical installer or wait for the repair slice.",
            TOOL_SURFACES_FIX_REFUSED
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mutating_flags_are_refused_before_any_read() {
        let error = skills_fix_refusal();
        assert_eq!(error.exit(), 2);
        assert_eq!(error.to_json()["error_code"], json!(SKILLS_FIX_REFUSED));
        assert!(
            error.to_json()["coverage"]
                .as_array()
                .is_some_and(|items| items.len() >= 2)
        );
        let error = tool_surfaces_fix_refusal();
        assert_eq!(error.exit(), 2);
        assert_eq!(
            error.to_json()["error_code"],
            json!(TOOL_SURFACES_FIX_REFUSED)
        );
    }

    #[test]
    fn an_unknown_kind_is_a_typed_usage_refusal() {
        let error = tool_surface_kinds(&["not-a-kind".to_string()]).expect_err("refused");
        assert_eq!(error.exit(), 2);
        assert_eq!(error.to_json()["error"]["code"], json!("unknown_kind"));
        assert_eq!(error.to_json()["ok"], json!(false));
        assert!(error.message().contains("Known kinds"));
        let kinds = tool_surface_kinds(&["command-skill".to_string(), "command-file".to_string()])
            .expect("known");
        assert_eq!(kinds.len(), 2);
        assert!(tool_surface_kinds(&[]).expect("empty").is_empty());
    }

    #[test]
    fn the_exit_two_not_in_project_refusal_keeps_the_reference_code() {
        let error = not_in_project(2);
        assert_eq!(error.exit(), 2);
        assert_eq!(error.to_json()["error"]["code"], json!("not_in_project"));
        assert_eq!(error.to_json()["ok"], json!(false));
        assert_eq!(error.message(), NOT_IN_PROJECT_MESSAGE);
        let exit_one = not_in_project(1);
        assert_eq!(exit_one.exit(), 1);
        assert_eq!(exit_one.to_json()["error"]["code"], json!("not_in_project"));
        assert_eq!(exit_one.to_json()["ok"], json!(false));
    }

    #[test]
    fn the_not_in_project_message_matches_the_reference() {
        assert_eq!(NOT_IN_PROJECT_MESSAGE, "Not in a spec-kitty project");
    }
}
