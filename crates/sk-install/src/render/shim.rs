//! Canonical thin command-shim renderer family.
//!
//! Source: `src/specify_cli/shims/{registry,generator}.py` plus
//! `src/specify_cli/migration/rewrite_shims.py`. One renderer produces the
//! *thin* CLI-driven command files (frontmatter `description`, the
//! `<!-- spec-kitty-command-version: … -->` marker, the three instruction lines
//! and one canonical `spec-kitty …` call); the prompt-driven commands keep
//! using the one full-template renderer in [`crate::render`]. There is no
//! second workflow body here and no orphan shim pointing at a command the CLI
//! does not implement.
//!
//! # Command classification
//!
//! [`CONSUMER_SKILLS`] is the disjoint union of [`PROMPT_DRIVEN_COMMANDS`] and
//! [`CLI_DRIVEN_COMMANDS`]. A prompt-driven command gets a full rendered prompt
//! and never a shim; a CLI-driven command gets exactly one thin file per agent.
//!
//! # Files written by [`rewrite_agent_shims`]
//!
//! 1. the six CLI-driven shims for every configured agent command surface;
//! 2. the eight prompt-driven prompts, re-rendered through the canonical
//!    template renderer;
//! 3. stale `spec-kitty.*` files that are neither freshly written nor an
//!    expected prompt file, removed only when the version marker (or a managed
//!    manifest entry) proves package ownership — an unproven user-authored file
//!    such as `spec-kitty.custom.md` is preserved in place and reported.

use std::path::{Path, PathBuf};

use crate::agent::{RenderFamily, agent_surface};
use crate::error::InstallError;
use crate::migrations::completion_legacy::agent_commands::{
    PROMPT_DRIVEN_COMMANDS, command_directories, output_filename,
};
use crate::preserve::{Prover, guard_destructive_removal};
use crate::render::TemplateSource;

/// Skills a project team drives directly (`CONSUMER_SKILLS`).
pub const CONSUMER_SKILLS: [&str; 14] = [
    "accept",
    "analyze",
    "charter",
    "consolidate",
    "implement",
    "plan",
    "research",
    "review",
    "specify",
    "status",
    "tasks",
    "tasks-finalize",
    "tasks-outline",
    "tasks-packages",
];

/// Operator/developer-only skills (`INTERNAL_SKILLS`); never written to a
/// consumer agent directory.
pub const INTERNAL_SKILLS: [&str; 3] = ["debug", "doctor", "materialize"];

/// Consumer skills dispatched through a thin canonical CLI call
/// (`CLI_DRIVEN_COMMANDS`), sorted.
pub const CLI_DRIVEN_COMMANDS: [&str; 6] = [
    "accept",
    "consolidate",
    "implement",
    "review",
    "status",
    "tasks-finalize",
];

/// The upgrade migration id this renderer family implements.
pub const MIGRATION_ID: &str = "3.1.1_direct_canonical_commands";

/// The old shim marker this migration replaces (`_OLD_SHIM_MARKER`).
pub const OLD_SHIM_MARKER: &str = "spec-kitty agent shim";

/// Human-readable picker descriptions (`SHIM_DESCRIPTIONS`).
pub fn shim_description(command: &str) -> String {
    match command {
        "implement" => "Execute a work package implementation".to_string(),
        "review" => "Review a work package implementation".to_string(),
        "accept" => "Validate an approved mission before merge".to_string(),
        "consolidate" => "Consolidate an accepted mission".to_string(),
        "status" => "Show mission and work package status".to_string(),
        "tasks-finalize" => "Finalize a mission's work packages".to_string(),
        other => format!("spec-kitty {other}"),
    }
}

/// The canonical single-line CLI call for a CLI-driven command
/// (`_canonical_command`).
pub fn canonical_command(
    command: &str,
    agent_key: &str,
    arg_placeholder: &str,
) -> Result<String, InstallError> {
    let template = match command {
        "implement" => "spec-kitty agent action implement {args} --agent {agent}",
        "review" => "spec-kitty agent action review {args} --agent {agent}",
        "accept" => "spec-kitty agent mission accept {args}",
        "status" => "spec-kitty agent tasks status {args}",
        "consolidate" => "spec-kitty consolidate {args}",
        "tasks-finalize" => "spec-kitty agent mission finalize-tasks {args}",
        other => {
            return Err(InstallError::UpgradeRefused {
                detail: format!(
                    "Unknown CLI-driven command {other:?}; expected one of: {}",
                    CLI_DRIVEN_COMMANDS.join(", ")
                ),
            });
        }
    };
    Ok(template
        .replace("{args}", arg_placeholder)
        .replace("{agent}", agent_key))
}

/// The three invariant instruction lines every shim carries.
const SHIM_INSTRUCTIONS: &str = "Run this exact command and treat its output as authoritative.\n\
Do not rediscover context from branches, files, prompt contents, or separate charter loads.\n\
When mission selection is required, pass --mission <handle> (mission_id, mid8, or mission_slug).\n";

/// Render the Markdown shim body (`generate_shim_content`).
pub fn generate_shim_content(
    command: &str,
    agent_key: &str,
    arg_placeholder: &str,
    version: &str,
) -> Result<String, InstallError> {
    let cli_call = canonical_command(command, agent_key, arg_placeholder)?;
    let description = shim_description(command);
    Ok(format!(
        "---\ndescription: {description}\n---\n\
<!-- spec-kitty-command-version: {version} -->\n\
{SHIM_INSTRUCTIONS}\n`{cli_call}`\n"
    ))
}

/// Render the TOML shim for agents whose command files are TOML
/// (`generate_shim_content_toml`).
pub fn generate_shim_content_toml(
    command: &str,
    agent_key: &str,
    arg_placeholder: &str,
    version: &str,
) -> Result<String, InstallError> {
    let cli_call = canonical_command(command, agent_key, arg_placeholder)?;
    let description = shim_description(command);
    let body = format!(
        "<!-- spec-kitty-command-version: {version} -->\n{SHIM_INSTRUCTIONS}\n`{cli_call}`\n"
    );
    let body = body.replace("\"\"\"", "\"\"\\\"");
    Ok(format!(
        "description = \"{description}\"\n\nprompt = \"\"\"\n{body}\"\"\"\n"
    ))
}

/// Render the shim for `agent_key`, choosing Markdown or TOML from the agent's
/// command surface (`generate_shim_content_for_agent`).
pub fn generate_shim_content_for_agent(
    command: &str,
    agent_key: &str,
    version: &str,
) -> Result<String, InstallError> {
    let surface = agent_surface(agent_key)?;
    let Some(command_surface) = surface.command else {
        return Err(InstallError::AgentHasNoCommandSurface {
            key: agent_key.to_string(),
        });
    };
    match command_surface.family {
        RenderFamily::Toml => generate_shim_content_toml(
            command,
            agent_key,
            command_surface.argument_placeholder,
            version,
        ),
        _ => generate_shim_content(
            command,
            agent_key,
            command_surface.argument_placeholder,
            version,
        ),
    }
}

/// The on-disk shim file name (`spec-kitty.<skill>.<ext>`).
pub fn shim_file_name(command: &str, agent_key: &str) -> String {
    output_filename(command, agent_key)
}

/// One rewrite pass (`RewriteResult`).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RewriteResult {
    /// Agent command directories examined.
    pub agents_processed: usize,
    /// Files created or overwritten, project-relative POSIX paths.
    pub files_written: Vec<String>,
    /// Stale package-generated files deleted, project-relative POSIX.
    pub files_deleted: Vec<String>,
    /// Unproven `spec-kitty.*` files left in place.
    pub files_preserved: Vec<String>,
    /// Non-fatal warnings.
    pub warnings: Vec<String>,
}

fn relative(project_root: &Path, path: &Path) -> String {
    path.strip_prefix(project_root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Regenerate the CLI shims + prompt-driven prompts, then retire only proven
/// stale command files (`rewrite_agent_shims`).
pub fn rewrite_agent_shims(
    project_root: &Path,
    version: &str,
    templates: Option<&TemplateSource>,
    dry_run: bool,
) -> Result<RewriteResult, InstallError> {
    let mut result = RewriteResult::default();
    let directories: Vec<_> = command_directories(project_root)
        .into_iter()
        .map(|directory| (directory.agent_key, directory.directory))
        .collect();
    // Source `_generate_prompt_templates` passes `repo_root=repo_root` into every
    // template render, so the SPDD/REASONS block visibility is the project's own
    // canonical activation answer — never the pure-render capability default and
    // never a hardcoded boolean. Binding here (one place) keeps every caller of
    // this rewrite on the same seam; an explicit `with_spdd_active` value wins.
    let templates = templates.map(|source| source.clone().for_project(project_root));

    // Step 1 + 2: render every expected file. The write set is collected before
    // any stale sweep so a partially rendered command is never deleted.
    let mut expected_prompts: Vec<PathBuf> = Vec::new();
    let mut written: Vec<PathBuf> = Vec::new();
    for (agent_key, directory) in &directories {
        let surface = agent_surface(agent_key)?;
        let Some(command_surface) = surface.command else {
            continue;
        };
        let agent_dir = project_root.join(directory);
        for command in CLI_DRIVEN_COMMANDS {
            let content = generate_shim_content_for_agent(command, agent_key, version)?;
            let path = agent_dir.join(shim_file_name(command, agent_key));
            written.push(path.clone());
            if dry_run {
                continue;
            }
            write_command_file(&path, content.as_bytes())?;
            result.files_written.push(relative(project_root, &path));
        }
        for command in PROMPT_DRIVEN_COMMANDS {
            let path = agent_dir.join(output_filename(command, agent_key));
            expected_prompts.push(path.clone());
        }
        // The TOML family rewrites `$ARGUMENTS` and the Markdown family keeps the
        // surface placeholder, which is why rendering stays in `crate::render`.
        let Some(templates) = templates.as_ref() else {
            result.warnings.push(
                "Command templates directory not found — skipping prompt-driven templates"
                    .to_string(),
            );
            continue;
        };
        for command in PROMPT_DRIVEN_COMMANDS {
            let asset = crate::render::render_command_file(
                surface,
                command,
                version,
                &command_surface,
                Some(templates),
            );
            let asset = match asset {
                Ok(asset) => asset,
                Err(error) => {
                    // Source logs a warning and continues: a missing template must
                    // not destroy the shims that were just written.
                    result.warnings.push(format!(
                        "Failed to render {command} for {agent_key}: {error}"
                    ));
                    continue;
                }
            };
            let path = agent_dir.join(output_filename(command, agent_key));
            written.push(path.clone());
            if dry_run {
                continue;
            }
            write_command_file(&path, &asset.bytes)?;
            result.files_written.push(relative(project_root, &path));
        }
    }

    // Step 3/4: sweep stale `spec-kitty.*` files, keeping unproven ones.
    let mut seen = Vec::new();
    for (agent_key, directory) in &directories {
        let _ = agent_key;
        let agent_dir = project_root.join(directory);
        if !agent_dir.is_dir() || seen.contains(&agent_dir) {
            continue;
        }
        seen.push(agent_dir.clone());
        result.agents_processed += 1;
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&agent_dir)
            .map_err(|error| InstallError::io("read", &agent_dir, error))?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("spec-kitty."))
            })
            .collect();
        entries.sort();
        for stale in entries {
            if written.contains(&stale) || expected_prompts.contains(&stale) {
                continue;
            }
            let verdict = guard_destructive_removal(
                &stale,
                project_root,
                &Prover::CanonicalContent,
                false,
                None,
                dry_run,
            )?;
            if verdict.removed() {
                result.files_deleted.push(relative(project_root, &stale));
            } else if matches!(verdict, crate::preserve::RemovalVerdict::Preserved { .. }) {
                result.files_preserved.push(relative(project_root, &stale));
                if let Some(diagnostic) = verdict.diagnostic() {
                    result.warnings.push(diagnostic.to_string());
                }
            }
        }
    }
    Ok(result)
}

/// `detect()`: any CLI-driven command file still carries the old shim marker.
pub fn detect_direct_canonical_commands(project_root: &Path) -> Result<bool, InstallError> {
    for directory in command_directories(project_root) {
        let agent_dir = project_root.join(&directory.directory);
        if !agent_dir.is_dir() {
            continue;
        }
        for command in CLI_DRIVEN_COMMANDS {
            let prefix = format!("spec-kitty.{command}.");
            let mut matches: Vec<PathBuf> = std::fs::read_dir(&agent_dir)
                .map_err(|error| InstallError::io("read", &agent_dir, error))?
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_stem()
                        .and_then(|stem| stem.to_str())
                        .is_some_and(|stem| stem == format!("spec-kitty.{command}"))
                        || path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| name.starts_with(&prefix))
                })
                .collect();
            matches.sort();
            for candidate in matches {
                let Ok(content) = std::fs::read_to_string(&candidate) else {
                    continue;
                };
                if content.contains(OLD_SHIM_MARKER) {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

/// `apply()`: regenerate every command file with direct canonical commands.
pub fn apply_direct_canonical_commands(
    project_root: &Path,
    dry_run: bool,
) -> Result<crate::upgrade::MigrationOutcome, InstallError> {
    let version = sk_kernel::COMPAT_CORE_VERSION;
    if dry_run {
        return Ok(crate::upgrade::MigrationOutcome::changed(vec![
            "Would regenerate all CLI-driven command files to use direct canonical commands"
                .to_string(),
        ]));
    }
    let templates =
        crate::migrations::completion_legacy::shared::pack_missions_root().map(TemplateSource::new);
    let result = rewrite_agent_shims(project_root, version, templates.as_ref(), false)?;
    let mut changes: Vec<String> = result
        .files_written
        .iter()
        .map(|path| format!("Regenerated: {path}"))
        .collect();
    changes.extend(
        result
            .files_deleted
            .iter()
            .map(|path| format!("Deleted stale: {path}")),
    );
    if changes.is_empty() {
        changes.push("No command files needed updating".to_string());
    }
    Ok(crate::upgrade::MigrationOutcome {
        changed: !result.files_written.is_empty() || !result.files_deleted.is_empty(),
        changes,
        warnings: result.warnings,
        ..crate::upgrade::MigrationOutcome::default()
    })
}

/// Write one generated command file, creating its directory.
///
/// Source writes these files with `Path.write_text` (both the CLI shims through
/// `specify_cli.shims.generator` and the prompt templates through
/// `specify_cli.migration.rewrite_shims`), so the operation is the
/// text-truncate policy: the destination is opened and truncated in place, an
/// existing inode and mode are kept, a new file is `0666 & ~umask`, and a
/// destination symlink is followed. That last point is why the previous
/// path-name symlink refusal is gone: it was a Native-only guard, and refusing
/// here would have hidden a file the reference rewrites. `create_dir_all` stays
/// because Source's generator creates the agent directory before writing.
fn write_command_file(path: &Path, bytes: &[u8]) -> Result<(), InstallError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| InstallError::io("create", parent, error))?;
    }
    sk_kernel::source_writer::write_source(
        sk_kernel::source_writer::WriterOperation::TextTruncate,
        path,
        sk_kernel::source_writer::SourcePayload::Bytes(bytes),
        None,
    )
    .map(|_receipt| ())
    .map_err(|error| InstallError::io("write command file", path, std::io::Error::other(error)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_is_disjoint_and_complete() {
        let mut union: Vec<&str> = Vec::new();
        union.extend(PROMPT_DRIVEN_COMMANDS.iter().copied());
        union.extend(CLI_DRIVEN_COMMANDS.iter().copied());
        union.sort_unstable();
        let mut consumer = CONSUMER_SKILLS.to_vec();
        consumer.sort_unstable();
        assert_eq!(union, consumer);
        for command in CLI_DRIVEN_COMMANDS {
            assert!(!PROMPT_DRIVEN_COMMANDS.contains(&command));
        }
    }

    #[test]
    fn claude_shim_matches_the_pinned_shape() {
        let content = generate_shim_content("implement", "claude", "$ARGUMENTS", "4.0.0rc8")
            .expect("renders");
        assert_eq!(
            content,
            "---\ndescription: Execute a work package implementation\n---\n\
<!-- spec-kitty-command-version: 4.0.0rc8 -->\n\
Run this exact command and treat its output as authoritative.\n\
Do not rediscover context from branches, files, prompt contents, or separate charter loads.\n\
When mission selection is required, pass --mission <handle> (mission_id, mid8, or mission_slug).\n\
\n`spec-kitty agent action implement $ARGUMENTS --agent claude`\n"
        );
    }

    #[test]
    fn toml_shim_wraps_prompt_in_triple_quotes() {
        let content = generate_shim_content_toml("status", "gemini", "{{args}}", "4.0.0rc8")
            .expect("renders");
        assert!(content.starts_with(
            "description = \"Show mission and work package status\"\n\nprompt = \"\"\"\n"
        ));
        assert!(content.contains("`spec-kitty agent tasks status {{args}}`"));
        assert!(content.ends_with("\"\"\"\n"));
    }
}
