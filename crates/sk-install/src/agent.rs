//! The configured agent surface registry.
//!
//! One owning registry replaces the three Python literals that had to stay in
//! step by convention (`core/config.py` `AI_CHOICES` / `AGENT_COMMAND_CONFIG` /
//! `AGENT_SKILL_CONFIG`, plus the skill-only roster in
//! `skills/_agent_roster.py`). Every consumer in this crate — rendering,
//! planning, initialization — reads surfaces from here, so an agent can never
//! be configured in one place and unroutable in another.
//!
//! Counts are part of the contract and are asserted by tests:
//!
//! - **17** configured agent keys (`AI_CHOICES` today);
//! - **12** slash-command surfaces, each with its own argument placeholder and
//!   file family;
//! - **16** skill-installable surfaces (every key except the wrapper-only `q`);
//! - **1** wrapper-only agent with no installable root.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::InstallError;

/// How a command file family serializes its argument placeholder.
///
/// The family is a property of the *harness*, not of the command: the same
/// canonical command renders as Markdown for Claude and TOML for Gemini.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RenderFamily {
    /// Markdown command file with YAML frontmatter (`spec-kitty.<cmd>.md`).
    Markdown,
    /// TOML command file with `description` + `prompt = """…"""` (`spec-kitty.<cmd>.toml`).
    Toml,
    /// GitHub Copilot prompt file (`spec-kitty.<cmd>.prompt.md`).
    PromptMarkdown,
    /// Agent Skills `SKILL.md` in a shared or native skill root.
    Skill,
}

impl RenderFamily {
    /// The file extension appended after `spec-kitty.<command>.`.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Markdown => "md",
            Self::Toml => "toml",
            Self::PromptMarkdown => "prompt.md",
            Self::Skill => "SKILL.md",
        }
    }
}

impl fmt::Display for RenderFamily {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Markdown => "markdown",
            Self::Toml => "toml",
            Self::PromptMarkdown => "prompt-markdown",
            Self::Skill => "skill",
        };
        formatter.write_str(text)
    }
}

/// A slash-command surface: the directory and argument placeholder a harness reads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CommandSurface {
    /// Project-relative directory the command files are written into (POSIX).
    pub directory: &'static str,
    /// The harness argument placeholder substituted for the template's `{ARGS}`.
    pub argument_placeholder: &'static str,
    /// The file family written into `directory`.
    pub family: RenderFamily,
}

/// Skill installation class, mirroring the Python placement taxonomy.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SkillClass {
    /// Installed into the cross-tool `.agents/skills/` root (shared-root capable).
    SharedRootCapable,
    /// Installed only into the harness' own skill root.
    NativeRootRequired,
    /// Wrapper-only: the harness has no installable root.
    WrapperOnly,
}

/// A skill surface: placement class plus its project-relative roots, in order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SkillSurface {
    /// Placement class.
    pub class: SkillClass,
    /// Project-relative skill roots, highest precedence first.
    pub roots: &'static [&'static str],
}

impl SkillSurface {
    /// The primary (highest-precedence) project skill root, if installable.
    pub fn primary_root(&self) -> Option<&'static str> {
        match self.class {
            SkillClass::WrapperOnly => None,
            _ => self.roots.first().copied(),
        }
    }
}

/// One configured agent key and everything an installer needs to know about it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentSurface {
    /// The stable configuration key (`claude`, `codex`, …).
    pub key: &'static str,
    /// Human-readable display name used by CLI output.
    pub display: &'static str,
    /// The project folder this agent may store private state in, if any.
    pub folder: Option<&'static str>,
    /// The slash-command surface, when the harness reads command files.
    pub command: Option<CommandSurface>,
    /// The skill surface.
    pub skill: SkillSurface,
}

impl AgentSurface {
    /// Whether the harness consumes Spec Kitty command *skills* (`SKILL.md`).
    ///
    /// These are the agents that share `.agents/skills/` rather than reading
    /// per-command files; the roster is `codex`, `vibe`, `pi`, `letta`,
    /// `antigravity` upstream (#6470).
    pub fn installs_command_skills(&self) -> bool {
        matches!(self.key, "codex" | "vibe" | "pi" | "letta" | "antigravity")
    }

    /// The slash-command surface, or a typed refusal for wrapper-only agents.
    pub fn require_command_surface(&self) -> Result<CommandSurface, InstallError> {
        self.command
            .ok_or_else(|| InstallError::AgentHasNoCommandSurface {
                key: self.key.to_string(),
            })
    }

    /// The primary skill root, or a typed refusal for wrapper-only agents.
    pub fn require_skill_root(&self) -> Result<&'static str, InstallError> {
        self.skill
            .primary_root()
            .ok_or_else(|| InstallError::AgentHasNoSkillRoot {
                key: self.key.to_string(),
            })
    }
}

/// The `$ARGUMENTS` placeholder used by Markdown/Prompt families.
pub const ARGUMENTS_PLACEHOLDER: &str = "$ARGUMENTS";
/// The `{{args}}` placeholder used by the TOML family.
pub const TOML_ARGS_PLACEHOLDER: &str = "{{args}}";

const MARKDOWN_SURFACE: CommandSurface = CommandSurface {
    directory: ".claude/commands",
    argument_placeholder: ARGUMENTS_PLACEHOLDER,
    family: RenderFamily::Markdown,
};

// The registry. Order is the canonical configuration order (`AI_CHOICES`).
const AGENTS: [AgentSurface; 17] = [
    AgentSurface {
        key: "copilot",
        display: "GitHub Copilot",
        folder: Some(".github/"),
        command: Some(CommandSurface {
            directory: ".github/prompts",
            argument_placeholder: ARGUMENTS_PLACEHOLDER,
            family: RenderFamily::PromptMarkdown,
        }),
        skill: SkillSurface {
            class: SkillClass::SharedRootCapable,
            roots: &[".agents/skills/", ".github/skills/"],
        },
    },
    AgentSurface {
        key: "claude",
        display: "Claude Code",
        folder: Some(".claude/"),
        command: Some(CommandSurface {
            directory: ".claude/commands",
            ..MARKDOWN_SURFACE
        }),
        skill: SkillSurface {
            class: SkillClass::NativeRootRequired,
            roots: &[".claude/skills/"],
        },
    },
    AgentSurface {
        key: "gemini",
        display: "Gemini CLI",
        folder: Some(".gemini/"),
        command: Some(CommandSurface {
            directory: ".gemini/commands",
            argument_placeholder: TOML_ARGS_PLACEHOLDER,
            family: RenderFamily::Toml,
        }),
        skill: SkillSurface {
            class: SkillClass::SharedRootCapable,
            roots: &[".agents/skills/", ".gemini/skills/"],
        },
    },
    AgentSurface {
        key: "cursor",
        display: "Cursor",
        folder: Some(".cursor/"),
        command: Some(CommandSurface {
            directory: ".cursor/commands",
            ..MARKDOWN_SURFACE
        }),
        skill: SkillSurface {
            class: SkillClass::SharedRootCapable,
            roots: &[".agents/skills/", ".cursor/skills/"],
        },
    },
    AgentSurface {
        key: "qwen",
        display: "Qwen Code",
        folder: Some(".qwen/"),
        command: Some(CommandSurface {
            directory: ".qwen/commands",
            argument_placeholder: TOML_ARGS_PLACEHOLDER,
            family: RenderFamily::Toml,
        }),
        skill: SkillSurface {
            class: SkillClass::NativeRootRequired,
            roots: &[".qwen/skills/"],
        },
    },
    AgentSurface {
        key: "opencode",
        display: "opencode",
        folder: Some(".opencode/"),
        command: Some(CommandSurface {
            directory: ".opencode/command",
            ..MARKDOWN_SURFACE
        }),
        skill: SkillSurface {
            class: SkillClass::SharedRootCapable,
            roots: &[".agents/skills/", ".opencode/skills/"],
        },
    },
    AgentSurface {
        key: "codex",
        display: "Codex CLI",
        folder: Some(".agents/skills/"),
        command: None,
        skill: SkillSurface {
            class: SkillClass::SharedRootCapable,
            roots: &[".agents/skills/"],
        },
    },
    AgentSurface {
        key: "windsurf",
        display: "Windsurf",
        folder: Some(".windsurf/"),
        command: Some(CommandSurface {
            directory: ".windsurf/workflows",
            ..MARKDOWN_SURFACE
        }),
        skill: SkillSurface {
            class: SkillClass::SharedRootCapable,
            roots: &[".agents/skills/", ".windsurf/skills/"],
        },
    },
    AgentSurface {
        key: "kilocode",
        display: "Kilo Code",
        folder: Some(".kilocode/"),
        command: Some(CommandSurface {
            directory: ".kilocode/workflows",
            ..MARKDOWN_SURFACE
        }),
        skill: SkillSurface {
            class: SkillClass::NativeRootRequired,
            roots: &[".kilocode/skills/"],
        },
    },
    AgentSurface {
        key: "auggie",
        display: "Auggie CLI",
        folder: Some(".augment/"),
        command: Some(CommandSurface {
            directory: ".augment/commands",
            ..MARKDOWN_SURFACE
        }),
        skill: SkillSurface {
            class: SkillClass::SharedRootCapable,
            roots: &[".agents/skills/", ".augment/skills/"],
        },
    },
    AgentSurface {
        key: "q",
        display: "Amazon Q Developer CLI (legacy; use 'kiro')",
        folder: Some(".amazonq/"),
        command: Some(CommandSurface {
            directory: ".amazonq/prompts",
            ..MARKDOWN_SURFACE
        }),
        skill: SkillSurface {
            class: SkillClass::WrapperOnly,
            roots: &[],
        },
    },
    AgentSurface {
        key: "kiro",
        display: "Kiro CLI (formerly Amazon Q Developer CLI)",
        folder: Some(".kiro/"),
        command: Some(CommandSurface {
            directory: ".kiro/prompts",
            ..MARKDOWN_SURFACE
        }),
        skill: SkillSurface {
            class: SkillClass::SharedRootCapable,
            roots: &[".agents/skills/", ".kiro/skills/"],
        },
    },
    AgentSurface {
        key: "antigravity",
        display: "Google Antigravity",
        folder: Some(".agents/skills/"),
        command: None,
        skill: SkillSurface {
            class: SkillClass::SharedRootCapable,
            roots: &[".agents/skills/", ".agent/skills/"],
        },
    },
    AgentSurface {
        key: "vibe",
        display: "Mistral Vibe",
        folder: Some(".vibe/"),
        command: None,
        skill: SkillSurface {
            class: SkillClass::SharedRootCapable,
            roots: &[".agents/skills/"],
        },
    },
    AgentSurface {
        key: "pi",
        display: "Pi",
        folder: Some(".agents/skills/"),
        command: None,
        skill: SkillSurface {
            class: SkillClass::SharedRootCapable,
            roots: &[".agents/skills/", ".pi/skills/"],
        },
    },
    AgentSurface {
        key: "letta",
        display: "Letta Code",
        folder: Some(".agents/skills/"),
        command: None,
        skill: SkillSurface {
            class: SkillClass::SharedRootCapable,
            roots: &[".agents/skills/"],
        },
    },
    AgentSurface {
        key: "llxprt",
        display: "LLxprt Code",
        folder: Some(".llxprt/"),
        command: Some(CommandSurface {
            directory: ".llxprt/commands",
            argument_placeholder: TOML_ARGS_PLACEHOLDER,
            family: RenderFamily::Toml,
        }),
        skill: SkillSurface {
            class: SkillClass::SharedRootCapable,
            roots: &[".agents/skills/", ".llxprt/skills/"],
        },
    },
];

/// Every configured agent surface, in canonical configuration order.
pub fn agents() -> &'static [AgentSurface] {
    &AGENTS
}

/// Every configured agent key, in canonical order.
pub fn agent_keys() -> impl Iterator<Item = &'static str> {
    AGENTS.iter().map(|agent| agent.key)
}

/// The comma-separated valid-key list used in diagnostics.
pub fn valid_keys() -> String {
    agent_keys().collect::<Vec<_>>().join(", ")
}

/// Resolve one agent key to its surface, failing closed on an unknown key.
pub fn agent_surface(key: &str) -> Result<&'static AgentSurface, InstallError> {
    let normalized = key.trim().to_ascii_lowercase();
    AGENTS
        .iter()
        .find(|agent| agent.key == normalized)
        .ok_or(InstallError::UnknownAgent {
            key: key.to_string(),
            valid: valid_keys(),
        })
}

/// Parse a `--ai` selection: comma/semicolon separated, case-insensitive,
/// order-preserving, de-duplicated, failing closed on any unknown key.
pub fn parse_agent_selection(raw: &str) -> Result<Vec<&'static AgentSurface>, InstallError> {
    let mut selected: Vec<&'static AgentSurface> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    for part in raw
        .replace(';', ",")
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        let surface = match agent_surface(part) {
            Ok(surface) => surface,
            Err(_) => {
                unknown.push(part.to_ascii_lowercase());
                continue;
            }
        };
        if !selected.iter().any(|known| known.key == surface.key) {
            selected.push(surface);
        }
    }
    if !unknown.is_empty() {
        return Err(InstallError::UnknownAgent {
            key: unknown.join(", "),
            valid: valid_keys(),
        });
    }
    if selected.is_empty() {
        return Err(InstallError::NoAgentsSelected);
    }
    Ok(selected)
}

/// The canonical command-skill set (prompt-backed + thin CLI wrappers).
///
/// Mirrors `skills/command_installer.py` `CANONICAL_COMMANDS`, which is
/// required to cover every consumer command shipped in `shims/registry.py`.
pub const PROMPT_BACKED_COMMANDS: [&str; 12] = [
    "accept",
    "analyze",
    "charter",
    "implement",
    "plan",
    "research",
    "review",
    "specify",
    "tasks",
    "tasks-finalize",
    "tasks-outline",
    "tasks-packages",
];

/// Thin command skills that delegate to the canonical CLI rather than a prompt.
pub const CLI_WRAPPER_COMMANDS: [&str; 2] = ["consolidate", "status"];

/// Every canonical command, sorted (the installer's canonical order).
pub const CANONICAL_COMMANDS: [&str; 14] = [
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

/// Whether `command` is part of the canonical command set.
pub fn is_canonical_command(command: &str) -> bool {
    CANONICAL_COMMANDS.contains(&command)
}

/// Whether `command` renders from a prompt template (rather than a CLI wrapper).
pub fn is_prompt_backed(command: &str) -> bool {
    PROMPT_BACKED_COMMANDS.contains(&command)
}
