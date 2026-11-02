//! Deterministic rendering of the managed command surfaces.
//!
//! Four renderer families are covered, matching the configured harnesses:
//! Markdown command files, TOML command files (Gemini/Qwen/LLxprt), GitHub
//! Copilot `*.prompt.md` files, and Agent Skills `SKILL.md` bodies in the shared
//! `.agents/skills/` root.
//!
//! Contracts (asserted by tests):
//!
//! 1. **Deterministic** — the same `(template, agent, version)` always produces
//!    byte-identical output. No timestamps, no random ordering.
//! 2. **No stray `$ARGUMENTS`** — a skill body never leaks the placeholder; a
//!    surviving token is a typed failure, not a silently shipped file.
//! 3. **Agent-independent skill bodies** — the shared root receives one body for
//!    every command-skill agent, so the installer can refuse differing bytes.
//! 4. **User content is not instrumentalized** — templates are read, never
//!    rewritten in place.
//!
//! Python reference (never invoked at runtime): `skills/command_renderer.py`,
//! `skills/_user_input_block.py`, `skills/skill_frontmatter.py`,
//! `template/asset_generator.py`, `agent_upgrade_prompt.py`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::agent::{
    ARGUMENTS_PLACEHOLDER, AgentSurface, CommandSurface, PROMPT_BACKED_COMMANDS, RenderFamily,
};
use crate::error::InstallError;

/// The Agent Skills file name.
pub const SKILL_FILENAME: &str = "SKILL.md";

/// The shared skills root prefix (project-relative POSIX, trailing slash).
pub const COMMAND_SKILLS_ROOT: &str = ".agents/skills/";

/// Prefix of generated command-skill names (Agent Skills grammar: `[a-z0-9-]`).
pub const COMMAND_SKILL_PREFIX: &str = "spec-kitty-";

/// Prefix of the retired pre-#6475 dotted names (read/retire only).
pub const LEGACY_COMMAND_SKILL_PREFIX: &str = "spec-kitty.";

/// Maximum description length carried in `SKILL.md` frontmatter.
pub const DESCRIPTION_MAX_LEN: usize = 140;

/// The User Input replacement block. Load-bearing text; a change is a version bump.
pub const REPLACEMENT_BLOCK: &str = "## User Input\n\n\
The content of the user's message that invoked this skill \
(everything after the skill invocation token, e.g. after \
`/spec-kitty-<command>` or `$spec-kitty-<command>`) is the User Input \
referenced elsewhere in these instructions.\n\n\
You **MUST** consider this user input before proceeding (if not empty).\n\n";

/// The startup upgrade-check block prepended to generated prompts.
pub const AGENT_UPGRADE_CHECK_BLOCK: &str = "## Startup Upgrade Check\n\n\
Run this at most once per active agent session before the first Spec Kitty command workflow.\n\
If you already ran `spec-kitty upgrade --agent-check --json` in this session, reuse that result and skip this block.\n\
Do not run or announce an upgrade check again for later Spec Kitty commands in the same session.\n\
Otherwise, before continuing, run:\n\n\
```bash\n\
spec-kitty upgrade --agent-check --json\n\
```\n\n\
If JSON `action` is `none`, continue.\n\
If `action` is `auto_upgrade`, run `upgrade_command` before continuing. If it fails, tell the user and continue with the current Spec Kitty version.\n\
If `action` is `guidance`, show `upgrade_note` briefly, then continue.\n\
If `action` is `prompt`, ask the user with the host-native question UI when available:\n\n\
`Spec Kitty {latest_version} is available. You are on {installed_version}. Upgrade now?`\n\n\
Use these choices:\n\n\
1. Upgrade now (recommended) - record `upgrade_now`, run `upgrade_command`, then continue.\n\
2. Always keep me up to date - record `always`, run `upgrade_command`, then continue.\n\
3. Not now - record `not_now`, then continue.\n\
4. Never ask again - record `never_ask`, then continue.\n\n\
Record the selected choice before continuing:\n\n\
```bash\n\
spec-kitty upgrade --agent-choice <upgrade_now|always|not_now|never_ask> --agent-latest <latest_version> --json\n\
```\n\n\
If no host-native question UI is available, present the same four choices in plain text and wait for the user.\n\
In non-interactive hosts, choose `not_now` and continue.\n";

/// One rendered managed asset, ready to plan and commit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderedAsset {
    /// Project-relative POSIX destination path.
    pub path: String,
    /// The exact bytes to write.
    pub bytes: Vec<u8>,
    /// The family that produced the bytes.
    pub family: RenderFamily,
    /// The command the asset implements.
    pub command: String,
    /// The agent the asset was rendered for.
    pub agent: &'static str,
    /// SHA-256 of the source template at render time (`None` for synthetic bodies).
    pub source_hash: Option<String>,
    /// The source template path, when one was read.
    pub source_template: Option<PathBuf>,
}

/// A command template resolved from a missions root.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedTemplate {
    /// Absolute path of the template file.
    pub path: PathBuf,
    /// The normalized template text.
    pub text: String,
    /// SHA-256 of the normalized text.
    pub source_hash: String,
}

/// Where command templates are resolved from.
///
/// The canonical built-in layout is `<pack>/missions/mission-steps/<mission>/<command>/prompt.md`;
/// the pre-relocation layout is `<pack>/missions/<mission>/command-templates/<command>.md`.
/// Both are searched, canonical first. A missing template is a typed failure:
/// this crate never invents a template or falls back to a bundled copy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TemplateSource {
    missions_root: PathBuf,
    mission_type: String,
    /// Whether the SPDD/REASONS charter pack is active for the project.
    ///
    /// `true` (the default) keeps block content and strips only the marker
    /// lines; `false` removes the block entirely, producing the pre-feature
    /// template bytes.
    spdd_active: bool,
}

impl TemplateSource {
    /// Use `missions_root` (a pack's `missions/` directory) with the canonical mission type.
    pub fn new(missions_root: impl Into<PathBuf>) -> Self {
        Self {
            missions_root: missions_root.into(),
            mission_type: "software-dev".to_string(),
            // Upstream treats an unconfigured project as active: block content
            // stays, only the marker comment lines are stripped.
            spdd_active: true,
        }
    }

    /// Use a pack root (`packs/built-in`) whose `missions/` directory holds templates.
    pub fn from_pack_root(pack_root: impl Into<PathBuf>) -> Self {
        Self::new(pack_root.into().join("missions"))
    }

    /// Select another mission type (for example `research`).
    pub fn with_mission_type(mut self, mission_type: impl Into<String>) -> Self {
        self.mission_type = mission_type.into();
        self
    }

    /// Mark the SPDD/REASONS charter pack active for this render.
    pub fn with_spdd_active(mut self, active: bool) -> Self {
        self.spdd_active = active;
        self
    }

    /// The missions root this source reads from.
    pub fn missions_root(&self) -> &Path {
        &self.missions_root
    }

    /// Fail closed when the missions root itself is missing.
    pub fn ensure_available(&self) -> Result<(), InstallError> {
        if self.missions_root.is_dir() {
            Ok(())
        } else {
            Err(InstallError::TemplateSourceUnavailable {
                detail: format!(
                    "missions root {} does not exist (fetch the pinned pack first)",
                    self.missions_root.display()
                ),
            })
        }
    }

    /// The candidate template paths for `command`, in resolution order.
    pub fn candidate_paths(&self, command: &str) -> [PathBuf; 2] {
        [
            self.missions_root
                .join("mission-steps")
                .join(&self.mission_type)
                .join(command)
                .join("prompt.md"),
            self.missions_root
                .join(&self.mission_type)
                .join("command-templates")
                .join(format!("{command}.md")),
        ]
    }

    /// Every command that has a template in either layout, sorted.
    ///
    /// Slash-command files are template-driven upstream (`generate_agent_assets`
    /// walks the template directory), so the command set for those harnesses is
    /// discovered rather than assumed. Commands with no template are simply not
    /// generated; a prompt-backed *skill* still fails closed when its template is
    /// missing.
    pub fn available_commands(&self) -> Result<Vec<String>, InstallError> {
        self.ensure_available()?;
        let mut found: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let steps = self
            .missions_root
            .join("mission-steps")
            .join(&self.mission_type);
        if steps.is_dir() {
            for entry in std::fs::read_dir(&steps)
                .map_err(|source| InstallError::io("read directory", &steps, source))?
            {
                let entry = entry
                    .map_err(|source| InstallError::io("read directory entry", &steps, source))?;
                let path = entry.path();
                if path.is_dir()
                    && path.join("prompt.md").is_file()
                    && let Some(name) = path.file_name()
                {
                    found.insert(name.to_string_lossy().to_string());
                }
            }
        }
        let legacy = self
            .missions_root
            .join(&self.mission_type)
            .join("command-templates");
        if legacy.is_dir() {
            for entry in std::fs::read_dir(&legacy)
                .map_err(|source| InstallError::io("read directory", &legacy, source))?
            {
                let entry = entry
                    .map_err(|source| InstallError::io("read directory entry", &legacy, source))?;
                let path = entry.path();
                if path.is_file()
                    && path.extension().is_some_and(|extension| extension == "md")
                    && let Some(stem) = path.file_stem()
                {
                    let name = stem.to_string_lossy().to_string();
                    if name != "README" {
                        found.insert(name);
                    }
                }
            }
        }
        Ok(found.into_iter().collect())
    }

    /// Resolve the template for `command`, failing closed when neither layout exists.
    pub fn resolve(&self, command: &str) -> Result<ResolvedTemplate, InstallError> {
        let candidates = self.candidate_paths(command);
        for candidate in &candidates {
            match sk_kernel::read_bounded(candidate, 4 * 1024 * 1024) {
                Ok(raw) => {
                    let text = normalize_newlines(std::str::from_utf8(&raw).map_err(|error| {
                        InstallError::io(
                            "decode template",
                            candidate,
                            std::io::Error::new(std::io::ErrorKind::InvalidData, error),
                        )
                    })?);
                    let text = process_spdd_blocks(&text, self.spdd_active)?;
                    let source_hash = crate::manifest::content_hash(text.as_bytes());
                    return Ok(ResolvedTemplate {
                        path: candidate.clone(),
                        text,
                        source_hash,
                    });
                }
                Err(sk_kernel::KernelError::Io { source, .. })
                    if source.kind() == std::io::ErrorKind::NotFound =>
                {
                    continue;
                }
                Err(error) => {
                    return Err(InstallError::io(
                        "read template",
                        candidate,
                        std::io::Error::other(error),
                    ));
                }
            }
        }
        Err(InstallError::TemplateNotFound {
            command: command.to_string(),
            root: self.missions_root.clone(),
        })
    }
}

/// Marker that opens an SPDD/REASONS conditional block.
pub const REASONS_BLOCK_START: &str = "<!-- spdd:reasons-block:start -->";
/// Marker that closes an SPDD/REASONS conditional block.
pub const REASONS_BLOCK_END: &str = "<!-- spdd:reasons-block:end -->";

/// Render SPDD/REASONS blocks for the active/inactive charter state.
///
/// `active` keeps the block content and strips only the marker lines; inactive
/// removes the whole block (plus the blank line author convention places before
/// it), leaving the pre-feature bytes. Unbalanced or nested markers are a typed
/// failure, never a silent truncation.
pub fn process_spdd_blocks(text: &str, active: bool) -> Result<String, InstallError> {
    if !text.contains(REASONS_BLOCK_START) && !text.contains(REASONS_BLOCK_END) {
        return Ok(text.to_string());
    }
    let mut depth = 0_i32;
    for line in text.lines() {
        let stripped = line.trim();
        if stripped == REASONS_BLOCK_START {
            if depth != 0 {
                return Err(InstallError::SpddBlockMalformed {
                    detail: "nested spdd:reasons-block start marker; flat blocks only".to_string(),
                });
            }
            depth += 1;
        } else if stripped == REASONS_BLOCK_END {
            if depth == 0 {
                return Err(InstallError::SpddBlockMalformed {
                    detail: "spdd:reasons-block end marker has no matching start".to_string(),
                });
            }
            depth -= 1;
        }
    }
    if depth != 0 {
        return Err(InstallError::SpddBlockMalformed {
            detail: "spdd:reasons-block start marker has no matching end".to_string(),
        });
    }
    let trailing = text.ends_with('\n');
    let lines: Vec<&str> = text.lines().collect();
    let mut output: Vec<&str> = Vec::new();
    if active {
        for line in lines {
            let stripped = line.trim();
            if stripped == REASONS_BLOCK_START || stripped == REASONS_BLOCK_END {
                continue;
            }
            output.push(line);
        }
    } else {
        let mut index = 0;
        while index < lines.len() {
            if lines[index].trim() == REASONS_BLOCK_START {
                if output.last().is_some_and(|line| line.is_empty()) {
                    output.pop();
                }
                index += 1;
                while index < lines.len() && lines[index].trim() != REASONS_BLOCK_END {
                    index += 1;
                }
                if index < lines.len() {
                    index += 1;
                }
                continue;
            }
            output.push(lines[index]);
            index += 1;
        }
    }
    let mut rendered = output.join("\n");
    if trailing {
        rendered.push('\n');
    }
    Ok(rendered)
}

/// Normalize CRLF/CR line endings to LF (upstream #4998).
pub fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// The canonical command-skill name for `command`.
pub fn command_skill_name(command: &str) -> String {
    format!("{COMMAND_SKILL_PREFIX}{command}")
}

/// The project-relative `SKILL.md` path of `command`'s canonical command skill.
pub fn command_skill_rel_path(command: &str) -> String {
    format!(
        "{COMMAND_SKILLS_ROOT}{}/{SKILL_FILENAME}",
        command_skill_name(command)
    )
}

/// The retired dotted `SKILL.md` path of `command` (retire-only).
pub fn legacy_command_skill_rel_path(command: &str) -> String {
    format!(
        "{COMMAND_SKILLS_ROOT}{}{command}/{SKILL_FILENAME}",
        LEGACY_COMMAND_SKILL_PREFIX
    )
}

/// The project-relative command-file path of `command` for `surface`.
pub fn command_file_rel_path(surface: &CommandSurface, command: &str) -> String {
    format!(
        "{}/spec-kitty.{command}.{}",
        surface.directory,
        surface.family.extension()
    )
}

/// Prepend the startup upgrade-check block unless it is already present.
pub fn prepend_agent_upgrade_check(body: &str) -> String {
    if body.contains("spec-kitty upgrade --agent-check --json") {
        return body.to_string();
    }
    if body.is_empty() {
        return AGENT_UPGRADE_CHECK_BLOCK.to_string();
    }
    let separator = if body.starts_with('\n') { "\n" } else { "\n\n" };
    format!("{AGENT_UPGRADE_CHECK_BLOCK}{separator}{body}")
}

/// Render one managed asset for `agent` and `command`.
///
/// `templates` is required for prompt-backed commands and ignored for the thin
/// CLI wrappers.
pub fn render_asset(
    agent: &AgentSurface,
    command: &str,
    version: &str,
    templates: Option<&TemplateSource>,
) -> Result<RenderedAsset, InstallError> {
    if agent.installs_command_skills() {
        render_skill_asset(agent, command, version, templates)
    } else {
        let surface = agent.require_command_surface()?;
        render_command_file(agent, command, version, &surface, templates)
    }
}

/// Render the shared-root `SKILL.md` asset for a command-skill agent.
pub fn render_skill_asset(
    agent: &AgentSurface,
    command: &str,
    version: &str,
    templates: Option<&TemplateSource>,
) -> Result<RenderedAsset, InstallError> {
    if !agent.installs_command_skills() {
        return Err(InstallError::UnknownAgent {
            key: format!("{} (not a command-skill agent)", agent.key),
            valid: crate::agent::valid_keys(),
        });
    }
    let _ = version;
    let (bytes, source_hash, source_template) = if PROMPT_BACKED_COMMANDS.contains(&command) {
        let source = templates.ok_or_else(|| InstallError::TemplateSourceUnavailable {
            detail: format!(
                "rendering command skill {command:?} requires a command template source"
            ),
        })?;
        let template = source.resolve(command)?;
        let rendered = render_skill_body(&template.path, &template.text, command)?;
        (
            rendered.into_bytes(),
            Some(template.source_hash),
            Some(template.path),
        )
    } else {
        (render_cli_wrapper_skill(command)?.into_bytes(), None, None)
    };

    Ok(RenderedAsset {
        path: command_skill_rel_path(command),
        bytes,
        family: RenderFamily::Skill,
        command: command.to_string(),
        agent: agent.key,
        source_hash,
        source_template,
    })
}

/// Render the slash-command file for `agent` and `command`.
pub fn render_command_file(
    agent: &AgentSurface,
    command: &str,
    version: &str,
    surface: &CommandSurface,
    templates: Option<&TemplateSource>,
) -> Result<RenderedAsset, InstallError> {
    let source = templates.ok_or_else(|| InstallError::TemplateSourceUnavailable {
        detail: format!(
            "rendering command file for {command:?} requires a command template source"
        ),
    })?;
    let template = source.resolve(command)?;
    let bytes = render_command_template(&template.text, agent.key, surface, version)?.into_bytes();
    Ok(RenderedAsset {
        path: command_file_rel_path(surface, command),
        bytes,
        family: surface.family,
        command: command.to_string(),
        agent: agent.key,
        source_hash: Some(template.source_hash),
        source_template: Some(template.path),
    })
}

/// Render a command-file template for one agent surface.
///
/// Substitutions mirror upstream `template/asset_generator.py`: `{ARGS}` is the
/// surface's argument placeholder, `__AGENT__` the agent key, `{SCRIPT}` and
/// `{AGENT_SCRIPT}` are empty (no script provider is involved in a project
/// install), and the TOML family rewrites `$ARGUMENTS` to `{{args}}`.
pub fn render_command_template(
    template_text: &str,
    agent_key: &str,
    surface: &CommandSurface,
    version: &str,
) -> Result<String, InstallError> {
    let text = normalize_newlines(template_text);
    let (frontmatter, stripped) = split_frontmatter(&text);
    let substituted = substitute(stripped, agent_key, surface.argument_placeholder);
    let mut body = prepend_agent_upgrade_check(&substituted);
    let description = frontmatter_description(frontmatter.as_deref().unwrap_or_default())
        .unwrap_or_else(|| format!("Spec Kitty {agent_key} command"));
    let version_marker = format!("<!-- spec-kitty-command-version: {version} -->\n");

    let mut output = String::new();
    match surface.family {
        RenderFamily::Toml => {
            body = body.replace(ARGUMENTS_PLACEHOLDER, surface.argument_placeholder);
            if !body.ends_with('\n') {
                body.push('\n');
            }
            let escaped = body.replace('\\', "\\\\").replace("\"\"\"", "\"\"\\\"");
            let _ = write!(
                output,
                "description = {}\n\nprompt = \"\"\"\n{version_marker}{escaped}\"\"\"\n",
                toml_basic_string(&description)
            );
        }
        RenderFamily::Markdown | RenderFamily::PromptMarkdown => {
            match frontmatter {
                Some(block) => {
                    let _ = write!(output, "---\n{block}\n---\n{version_marker}\n{body}");
                }
                None => {
                    let _ = write!(output, "{version_marker}\n{body}");
                }
            }
            if !output.ends_with('\n') {
                output.push('\n');
            }
        }
        RenderFamily::Skill => {
            return Err(InstallError::UnsupportedNode {
                path: PathBuf::from(command_skill_rel_path(agent_key)),
                detail: "the skill family is rendered by render_skill_body".to_string(),
            });
        }
    }
    Ok(output)
}

/// Render a prompt-backed command into an Agent Skills `SKILL.md` document.
pub fn render_skill_body(
    template_path: &Path,
    template_text: &str,
    command: &str,
) -> Result<String, InstallError> {
    let text = normalize_newlines(template_text);
    let (frontmatter, body_after_frontmatter) = split_frontmatter(&text);
    let stripped = body_after_frontmatter.to_string();

    let rewritten =
        rewrite_user_input_block(&stripped).ok_or_else(|| InstallError::MissingUserInputBlock {
            path: template_path.to_path_buf(),
        })?;

    let rewritten = rewrite_command_invocations(&rewritten);
    for (index, line) in rewritten.lines().enumerate() {
        if line.contains(ARGUMENTS_PLACEHOLDER) {
            return Err(InstallError::StrayArgumentsToken {
                path: template_path.to_path_buf(),
                line: index + 1,
            });
        }
    }

    // Description priority: template frontmatter, `## Purpose` sentence, first
    // paragraph, then a canonical fallback derived from the command name.
    let description = frontmatter_description(frontmatter.as_deref().unwrap_or_default())
        .or_else(|| purpose_description(&stripped))
        .or_else(|| first_paragraph(&stripped))
        .unwrap_or_else(|| format!("Spec Kitty {command} workflow"));

    let name = command_skill_name(command);
    let front = render_command_skill_frontmatter(&name, &description);
    Ok(format!(
        "{front}\n{}",
        prepend_agent_upgrade_check(&rewritten)
    ))
}

/// Render the thin skill body of a CLI-wrapper command (`consolidate`, `status`).
pub fn render_cli_wrapper_skill(command: &str) -> Result<String, InstallError> {
    let (description, cli_command) = match command {
        "consolidate" => ("Consolidate an accepted mission", "spec-kitty consolidate"),
        "status" => (
            "Show mission and work package status",
            "spec-kitty agent tasks status",
        ),
        other => {
            return Err(InstallError::TemplateNotFound {
                command: other.to_string(),
                root: PathBuf::from("<cli-wrapper>"),
            });
        }
    };
    let name = command_skill_name(command);
    let body = format!(
        "# /{name} - {description}\n\n\
## Purpose\n\n\
Run the canonical Spec Kitty CLI command for this workflow and treat its output as authoritative.\n\n\
Do not rediscover mission context from branches, files, prompt contents, or separate charter loads. \
If mission selection is required, pass `--mission <handle>` where `<handle>` is a mission_id, mid8, or mission_slug.\n\n\
## User Input\n\n\
The content of the user's message that invoked this skill is the User Input. \
Consider it before proceeding. If it contains CLI arguments, append them to the command below.\n\n\
## Steps\n\n\
Run this command from the repository root:\n\n\
```bash\n\
{cli_command} <user-provided-args-if-any>\n\
```\n\n\
Report the command output and follow any next-step instructions it prints.\n"
    );
    let body = prepend_agent_upgrade_check(&body);
    let front = render_command_skill_frontmatter(&name, description);
    Ok(format!("{front}\n\n{body}"))
}

/// Render the spec-only frontmatter block of a command skill (no trailing newline).
pub fn render_command_skill_frontmatter(name: &str, description: &str) -> String {
    let description = truncate_chars(description, DESCRIPTION_MAX_LEN);
    format!(
        "---\nname: {name}\ndescription: {}\nmetadata:\n  user-invocable: \"true\"\n---",
        yaml_scalar(&description)
    )
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// Split a leading YAML frontmatter block from the body.
///
/// Returns `(Some(inner_block), body)` when the document starts with `---` and
/// the block is closed; `(None, whole_text)` otherwise.
fn split_frontmatter(text: &str) -> (Option<String>, &str) {
    let rest = match text.strip_prefix("---\n") {
        Some(rest) => rest,
        None => return (None, text),
    };
    let mut offset = 0usize;
    for line in rest.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if trimmed == "---" {
            let inner = &rest[..offset];
            let body = &rest[offset + line.len()..];
            let inner = inner.strip_suffix('\n').unwrap_or(inner);
            return (Some(inner.to_string()), body);
        }
        offset += line.len();
    }
    (None, text)
}

fn substitute(body: &str, agent_key: &str, argument_placeholder: &str) -> String {
    body.replace("{ARGS}", argument_placeholder)
        .replace("__AGENT__", agent_key)
        .replace("{AGENT_SCRIPT}", "")
        .replace("{SCRIPT}", "")
}

fn frontmatter_description(block: &str) -> Option<String> {
    for line in block.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("description:") {
            let value = rest.trim();
            let value = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .unwrap_or(value);
            let value = value
                .strip_prefix('\'')
                .and_then(|v| v.strip_suffix('\''))
                .unwrap_or(value);
            let value = truncate_chars(value, DESCRIPTION_MAX_LEN);
            return if value.is_empty() { None } else { Some(value) };
        }
    }
    None
}

fn purpose_description(body: &str) -> Option<String> {
    let mut lines = body.lines();
    let mut found = false;
    let mut section = String::new();
    for line in lines.by_ref() {
        if found {
            if line.starts_with('#') {
                break;
            }
            section.push_str(line);
            section.push('\n');
        } else if line.trim_end() == "## Purpose" || line.trim_end() == "##  Purpose" {
            found = true;
        }
    }
    let section = section.trim();
    if section.is_empty() {
        return None;
    }
    let end = section
        .find(['.', '?', '!', '\n'])
        .map_or(section.len(), |index| index + 1);
    let sentence = section[..end].trim().trim_end_matches('.').trim();
    if sentence.is_empty() {
        None
    } else {
        Some(truncate_chars(sentence, DESCRIPTION_MAX_LEN))
    }
}

fn first_paragraph(body: &str) -> Option<String> {
    let mut in_fence = false;
    for line in body.lines() {
        let stripped = line.trim();
        if stripped.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence
            || stripped.is_empty()
            || stripped.starts_with('#')
            || (stripped.len() >= 3 && stripped.chars().all(|c| matches!(c, '-' | '*' | '_')))
        {
            continue;
        }
        let cleaned = stripped.trim_start_matches(['*', '_']).trim();
        if !cleaned.is_empty() {
            return Some(truncate_chars(cleaned, DESCRIPTION_MAX_LEN));
        }
    }
    None
}

/// Replace the `## User Input` block with [`REPLACEMENT_BLOCK`].
///
/// Returns `None` when no level-2 `## User Input` heading is present, so the
/// caller fails closed rather than generating a skill with no user input.
pub fn rewrite_user_input_block(body: &str) -> Option<String> {
    let mut search_offset = 0usize;
    let mut start = None;
    let mut heading_end = 0usize;
    while search_offset <= body.len() {
        let rest = &body[search_offset..];
        let line_end = rest.find('\n').map_or(rest.len(), |index| index + 1);
        let line = rest[..line_end].trim_end_matches(['\n', '\r']);
        if is_user_input_heading(line) {
            start = Some(search_offset);
            heading_end = search_offset + line_end;
            break;
        }
        if line_end == 0 || search_offset + line_end >= body.len() {
            break;
        }
        search_offset += line_end;
    }
    let start = start?;
    // The block ends at the next level-1/level-2 heading (exclusive of that heading).
    let mut end = body.len();
    let mut cursor = heading_end;
    while cursor < body.len() {
        let rest = &body[cursor..];
        let line_end = rest.find('\n').map_or(rest.len(), |index| index + 1);
        let line = rest[..line_end].trim_end_matches(['\n', '\r']);
        if is_level_one_or_two_heading(line) {
            end = cursor;
            break;
        }
        cursor += line_end;
    }
    Some(format!(
        "{}{}{}",
        &body[..start],
        REPLACEMENT_BLOCK,
        &body[end..]
    ))
}

fn is_user_input_heading(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("## ") else {
        return false;
    };
    rest.trim_end() == "User Input"
}

fn is_level_one_or_two_heading(line: &str) -> bool {
    let hashes = line
        .chars()
        .take_while(|character| *character == '#')
        .count();
    (1..=2).contains(&hashes) && line.chars().nth(hashes) == Some(' ')
}

/// Rewrite `spec-kitty.<cmd>` invocation tokens to the canonical skill name.
pub fn rewrite_command_invocations(text: &str) -> String {
    // Longest command first so `tasks` never claims the start of `tasks-outline`.
    let mut commands: Vec<&str> = crate::agent::CANONICAL_COMMANDS.to_vec();
    commands.sort_by_key(|command| std::cmp::Reverse(command.len()));

    let mut output = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if let Some((sigil, consumed)) = match_sigil(&text[index..]) {
            let after = index + consumed;
            if let Some(command) = commands.iter().find(|command| {
                text[after..].starts_with(&format!("{LEGACY_COMMAND_SKILL_PREFIX}{command}"))
            }) {
                let token = format!("{LEGACY_COMMAND_SKILL_PREFIX}{command}");
                let token_end = after + token.len();
                let boundary_before_ok =
                    index == 0 || !is_invocation_preceding_byte(bytes[index - 1]);
                let boundary_after_ok = boundary_after(text, token_end);
                if boundary_before_ok && boundary_after_ok {
                    output.push_str(sigil);
                    output.push_str(COMMAND_SKILL_PREFIX);
                    output.push_str(command);
                    index = token_end;
                    continue;
                }
            }
        }
        let character = text[index..]
            .chars()
            .next()
            .expect("index is on a char boundary");
        output.push(character);
        index += character.len_utf8();
    }
    output
}

fn match_sigil(text: &str) -> Option<(&'static str, usize)> {
    if text.starts_with("/skill:") {
        Some(("/skill:", 7))
    } else if text.starts_with('/') {
        Some(("/", 1))
    } else if text.starts_with('$') {
        Some(("$", 1))
    } else {
        None
    }
}

fn is_invocation_preceding_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b':' | b'/' | b'$' | b'-' | b'_')
}

fn boundary_after(text: &str, index: usize) -> bool {
    let rest = &text[index..];
    match rest.chars().next() {
        None => true,
        Some(character) if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') => {
            false
        }
        Some('.') => {
            // A file extension (`.md`) keeps the token untouched; a sentence-ending
            // period followed by whitespace does not.
            !rest[1..]
                .chars()
                .next()
                .is_some_and(|next| next.is_ascii_alphanumeric())
        }
        Some(_) => true,
    }
}

fn truncate_chars(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

/// Render a YAML scalar: bare when safe, double-quoted otherwise.
fn yaml_scalar(text: &str) -> String {
    if is_safe_bare_scalar(text) {
        return text.to_string();
    }
    double_quoted(text)
}

fn is_safe_bare_scalar(text: &str) -> bool {
    if text.is_empty() || text.trim() != text {
        return false;
    }
    const KEYWORDS: [&str; 15] = [
        "true", "True", "TRUE", "false", "False", "FALSE", "yes", "Yes", "YES", "no", "No", "NO",
        "on", "Off", "~",
    ];
    if KEYWORDS.contains(&text) {
        return false;
    }
    if text.chars().any(|character| {
        matches!(
            character,
            ':' | '#'
                | '['
                | ']'
                | '{'
                | '}'
                | ','
                | '&'
                | '*'
                | '?'
                | '|'
                | '<'
                | '>'
                | '='
                | '!'
                | '%'
                | '@'
                | '`'
                | '"'
                | '\''
                | '\n'
                | '\r'
                | '\t'
        ) || character.is_control()
    }) {
        return false;
    }
    !text
        .chars()
        .next()
        .is_some_and(|first| "-+.0123456789".contains(first))
}

fn double_quoted(text: &str) -> String {
    let mut quoted = serde_json::to_string(text).expect("string serialization cannot fail");
    // Characters YAML cannot carry raw but JSON leaves unescaped.
    let mut escaped = String::with_capacity(quoted.len());
    for character in quoted.chars() {
        match character {
            '\u{85}' | '\u{2028}' | '\u{2029}' | '\u{feff}' | '\u{fffe}' | '\u{ffff}' => {
                let _ = write!(escaped, "\\u{:04x}", character as u32);
            }
            other => escaped.push(other),
        }
    }
    quoted = escaped;
    quoted
}

fn toml_basic_string(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len() + 2);
    escaped.push('"');
    for character in text.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            control if control.is_control() => {
                let _ = write!(escaped, "\\u{:04X}", control as u32);
            }
            other => escaped.push(other),
        }
    }
    escaped.push('"');
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_frontmatter_reads_block_and_body() {
        let (block, body) = split_frontmatter("---\ndescription: hi\n---\n\nBody\n");
        assert_eq!(block.as_deref(), Some("description: hi"));
        assert_eq!(body, "\nBody\n");
    }

    #[test]
    fn split_frontmatter_tolerates_absent_block() {
        let (block, body) = split_frontmatter("# Heading\n");
        assert!(block.is_none());
        assert_eq!(body, "# Heading\n");
    }

    #[test]
    fn user_input_block_is_replaced_between_headings() {
        let body = "# Title\n\n## User Input\n\n$ARGUMENTS\n\n## Steps\n\nDo it.\n";
        let rewritten = rewrite_user_input_block(body).expect("block present");
        assert!(!rewritten.contains("$ARGUMENTS"));
        assert!(rewritten.contains(REPLACEMENT_BLOCK));
        assert!(rewritten.ends_with("## Steps\n\nDo it.\n"));
    }

    #[test]
    fn user_input_block_missing_is_none() {
        assert!(rewrite_user_input_block("# Title\n").is_none());
    }

    #[test]
    fn invocation_rewrite_keeps_sigil_and_boundary() {
        assert_eq!(
            rewrite_command_invocations("run /spec-kitty.plan now"),
            "run /spec-kitty-plan now"
        );
        assert_eq!(
            rewrite_command_invocations("$spec-kitty.tasks-outline and /skill:spec-kitty.tasks"),
            "$spec-kitty-tasks-outline and /skill:spec-kitty-tasks"
        );
        // File names and unrelated dotted names are untouched.
        assert_eq!(
            rewrite_command_invocations("see /spec-kitty.plan.md and spec-kitty.ai"),
            "see /spec-kitty.plan.md and spec-kitty.ai"
        );
    }

    #[test]
    fn yaml_scalar_quotes_unsafe_values() {
        assert_eq!(yaml_scalar("plain text"), "plain text");
        assert_eq!(yaml_scalar("has: colon"), "\"has: colon\"");
        assert_eq!(yaml_scalar("true"), "\"true\"");
    }
}
