//! Configured agent-surface plumbing shared by the pre-3.x command migrations.
//!
//! `get_agent_dirs_for_project` is the pinned authority for *which* agent
//! directories a migration may touch: the configured agents only, an explicit
//! fallback to the full roster when the configuration is absent. The native
//! roster ([`crate::agent::agents`]) and [`crate::migrations::retirements`]'s
//! config-aware directory list are the same authority, so these helpers reuse
//! them rather than inventing a second roster.

use std::path::{Path, PathBuf};

use crate::agent::{CommandSurface, RenderFamily, agent_surface, agents};
use crate::error::InstallError;
use crate::migrations::retirements::{AgentDirectory, configured_agent_directories};

/// The commands whose workflow is a *full prompt template* (`PROMPT_DRIVEN_COMMANDS`).
pub const PROMPT_DRIVEN_COMMANDS: [&str; 8] = [
    "analyze",
    "charter",
    "plan",
    "research",
    "specify",
    "tasks",
    "tasks-outline",
    "tasks-packages",
];

/// The `.md` shim marker the shim generator wrote (`SHIM_MARKER`).
pub const SHIM_MARKER: &str = "spec-kitty agent shim";

/// The `< 10` non-empty-line budget of a thin shim (`SHIM_LINE_THRESHOLD`).
pub const SHIM_LINE_THRESHOLD: usize = 10;

/// The configured agent command directories for `project_root`.
pub fn command_directories(project_root: &Path) -> Vec<AgentDirectory> {
    configured_agent_directories(project_root)
}

/// The slash-command surface of `agent_key`, when the harness reads one.
pub fn command_surface(agent_key: &str) -> Option<CommandSurface> {
    agent_surface(agent_key)
        .ok()
        .and_then(|surface| surface.command)
}

/// The agent key that owns a project-relative directory (`.claude/commands`).
pub fn agent_key_for_directory(directory: &str) -> Option<&'static str> {
    let normalized = directory.trim_end_matches('/');
    agents().iter().find_map(|surface| {
        surface
            .command
            .filter(|command| command.directory.trim_end_matches('/') == normalized)
            .map(|_| surface.key)
    })
}

/// The on-disk file name for `command` + `agent_key`
/// (`_compute_output_filename`).
///
/// The shim generator always wrote `.md`; a full prompt uses the agent's own
/// extension, and Codex replaces `-` with `_` in the stem.
pub fn output_filename(command: &str, agent_key: &str) -> String {
    let Some(surface) = command_surface(agent_key) else {
        return format!("spec-kitty.{command}.md");
    };
    let stem = if agent_key == "codex" {
        command.replace('-', "_")
    } else {
        command.to_string()
    };
    match surface.family.extension() {
        "" => format!("spec-kitty.{stem}"),
        extension => format!("spec-kitty.{stem}.{extension}"),
    }
}

/// The render family of `agent_key` (for generated-write shape checks).
pub fn render_family(agent_key: &str) -> Option<RenderFamily> {
    command_surface(agent_key).map(|surface| surface.family)
}

/// Whether `path` is a thin shim for a prompt-driven command
/// (`_is_thin_shim`: `< 10` non-empty lines *and* the shim marker).
pub fn is_thin_shim(path: &Path) -> bool {
    let Some(text) = crate::migrations::completion_legacy::paths::read_text(path) else {
        return false;
    };
    let non_empty = text.lines().filter(|line| !line.trim().is_empty()).count();
    non_empty < SHIM_LINE_THRESHOLD && text.contains(SHIM_MARKER)
}

/// The candidate paths a thin shim for `command` may occupy: the agent's
/// canonical file name, plus the legacy `.md` shim the generator wrote.
pub fn shim_candidates(agent_dir: &Path, command: &str, agent_key: &str) -> Vec<PathBuf> {
    let mut candidates = vec![agent_dir.join(output_filename(command, agent_key))];
    let legacy = agent_dir.join(format!("spec-kitty.{command}.md"));
    if !candidates.contains(&legacy) {
        candidates.push(legacy);
    }
    candidates
}

/// The command-prompt template roots, in the source's resolution order.
///
/// 1. `<checkout>/packs/built-in/missions/mission-steps/<mission>` — the
///    canonical mission steps (`MissionTemplateRepository.default_missions_root`).
/// 2. `<package missions>/<mission>/command-templates` — the legacy flat tree.
/// 3. `~/.kittify/missions/<mission>/command-templates` — the installed runtime.
pub fn runtime_command_templates(mission: &str) -> Option<PathBuf> {
    if let Some(checkout) = crate::migrations::completion_legacy::assets::checkout_root() {
        let steps = checkout
            .join("packs/built-in/missions/mission-steps")
            .join(mission);
        if steps.is_dir() {
            return Some(steps);
        }
    }
    if let Some(missions) = crate::migrations::completion_legacy::assets::package_missions_root() {
        let flat = missions.join(mission).join("command-templates");
        if flat.is_dir() {
            return Some(flat);
        }
    }
    let home = crate::migrations::completion_legacy::assets::kittify_home()?;
    let flat = home
        .join("missions")
        .join(mission)
        .join("command-templates");
    flat.is_dir().then_some(flat)
}

/// The template file for one command: the canonical mission step prompt first,
/// then the legacy flat `<command>.md` (`_resolve_template_path`).
pub fn template_path(templates_dir: &Path, command: &str) -> PathBuf {
    let prompt = templates_dir.join(command).join("prompt.md");
    if prompt.is_file() {
        return prompt;
    }
    templates_dir.join(format!("{command}.md"))
}

/// Render one **resolved template file** for `agent_key`.
///
/// This is the raw-template seam: it substitutes the surface placeholders and
/// writes the family envelope, but it does not resolve templates from the pack
/// and does not render the SPDD/REASONS conditional block, so the marker lines
/// stay in the body. Prefer [`render_prompt`] for a command prompt; this form
/// exists for a caller that already holds the exact template bytes.
pub fn render_raw_template(
    template: &Path,
    agent_key: &str,
    version: &str,
) -> Result<String, InstallError> {
    let text = std::fs::read_to_string(template)
        .map_err(|error| InstallError::io("read template", template, error))?;
    let surface =
        command_surface(agent_key).ok_or_else(|| InstallError::AgentHasNoCommandSurface {
            key: agent_key.to_string(),
        })?;
    crate::render::render_command_template(&text, agent_key, &surface, version)
}

/// Render one command prompt through the crate's **canonical** renderer.
///
/// This is the same pipeline `init` uses ([`crate::render::render_asset`] with a
/// [`crate::render::TemplateSource`]): the template is resolved from the pack's
/// `mission-steps` (falling back to the legacy `command-templates`), the
/// SPDD/REASONS conditional block is inactive (Source's omitted-project default), and
/// the per-family body is produced for the agent's surface. A caller that
/// renders "the full prompt for command X" wants this, not
/// [`render_raw_template`].
///
/// The `spec-kitty-command-version` marker carries
/// [`super::LEGACY_RENDER_VERSION`] — the Source compatibility identity — so a
/// restored file byte-matches the release the Python CLI of the same version
/// writes.
pub fn render_prompt(agent_key: &str, command: &str) -> Result<String, InstallError> {
    render_prompt_with_project(agent_key, command, None)
}

/// Render a restored prompt with the actual project's activation decision.
///
/// Unlike the no-project Source helper, this consults the canonical selector for
/// marker-bearing templates. Neither assets nor CWD stand in for the project.
pub fn render_prompt_for_project(
    agent_key: &str,
    command: &str,
    project_root: &Path,
) -> Result<String, InstallError> {
    render_prompt_with_project(agent_key, command, Some(project_root))
}

fn render_prompt_with_project(
    agent_key: &str,
    command: &str,
    project_root: Option<&Path>,
) -> Result<String, InstallError> {
    let missions_root = crate::migrations::completion_legacy::assets::pack_missions_root()
        .or_else(crate::migrations::completion_legacy::assets::package_missions_root)
        .ok_or_else(|| InstallError::TemplateSourceUnavailable {
            detail: "the pinned missions tree is not resolvable".to_string(),
        })?;
    let source = crate::render::TemplateSource::new(missions_root);
    let source = match project_root {
        Some(root) => source.for_project(root),
        // Source _render_full_prompt(..., repo_root=None) is inactive, including
        // update-planning-templates which deliberately omits the project argument.
        None => source.with_spdd_active(false),
    };
    let agent = agent_surface(agent_key)?;
    let rendered = crate::render::render_asset(
        agent,
        command,
        crate::migrations::completion_legacy::LEGACY_RENDER_VERSION,
        Some(&source),
    )?;
    String::from_utf8(rendered.bytes).map_err(|error| InstallError::UpgradeRefused {
        detail: format!("rendered prompt for {command} is not UTF-8: {error}"),
    })
}

/// The generated command files of every configured agent, sorted, matching
/// `spec-kitty.*.md` / `spec-kitty.*.toml` (`FILE_GLOBS`).
pub fn generated_prompt_files(project_root: &Path, legacy_codex: bool) -> Vec<PathBuf> {
    let mut directories: Vec<PathBuf> = command_directories(project_root)
        .into_iter()
        .map(|directory| project_root.join(directory.directory))
        .collect();
    if legacy_codex {
        let codex = project_root.join(".codex/prompts");
        if codex.exists() {
            directories.push(codex);
        }
    }
    let mut files: Vec<PathBuf> = Vec::new();
    for directory in directories {
        if !directory.is_dir() {
            continue;
        }
        for entry in crate::migrations::completion_legacy::paths::entries(&directory) {
            let Some(name) = entry
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
            else {
                continue;
            };
            if !name.starts_with("spec-kitty.") {
                continue;
            }
            if !(name.ends_with(".md") || name.ends_with(".toml")) {
                continue;
            }
            if entry.is_file() && !files.contains(&entry) {
                files.push(entry);
            }
        }
    }
    files.sort();
    files
}

/// The skill roots a migration may install into (`SKILL_ROOTS`).
pub const SKILL_ROOTS: [&str; 14] = [
    ".claude/skills",
    ".agents/skills",
    ".qwen/skills",
    ".kilocode/skills",
    ".github/skills",
    ".gemini/skills",
    ".llxprt/skills",
    ".cursor/skills",
    ".opencode/skills",
    ".windsurf/skills",
    ".augment/skills",
    ".roo/skills",
    ".agent/skills",
    ".codex/skills",
];

/// Every installed copy of one skill file across the known roots
/// (`find_skill_files`), sorted.
pub fn find_skill_files(project_root: &Path, skill: &str, relative: Option<&str>) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for root in SKILL_ROOTS {
        let skill_dir = project_root.join(root).join(skill);
        if !skill_dir.is_dir() {
            continue;
        }
        match relative {
            Some(relative) => {
                let path = skill_dir.join(relative);
                if path.is_file() {
                    found.push(path);
                }
            }
            None => {
                for path in crate::migrations::completion_legacy::paths::markdown_files(&skill_dir)
                {
                    found.push(path);
                }
            }
        }
    }
    found.sort();
    found
}
