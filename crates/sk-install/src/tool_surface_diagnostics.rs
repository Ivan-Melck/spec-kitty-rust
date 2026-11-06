//! Read-only tool-surface health diagnostics for the three `doctor` leaves that
//! were refused before this module existed:
//!
//! * `spec-kitty doctor command-files`
//! * `spec-kitty doctor skills`
//! * `spec-kitty doctor tool-surfaces [--kind …] [--tool …]`
//!
//! The reference implementation of these surfaces is
//! `src/specify_cli/tool_surface/**` plus `cli/commands/_command_surface_doctor.py`
//! and `runtime/doctor.check_command_file_health` at the pinned baseline. This
//! module reproduces their **read-only** behaviour on the canonical native
//! owners; it never repairs anything and never writes to the project:
//!
//! | Surface | Canonical owner reused here |
//! | --- | --- |
//! | configured agent roster / command-file family / skill roots | [`crate::agent`] |
//! | command-skill ownership manifest (`.kittify/command-skills-manifest.json`) | [`crate::manifest`] |
//! | command-skill paths/names (`spec-kitty-<command>/SKILL.md`, legacy dotted form) | [`crate::render`] |
//! | managed charter-skill manifest (`.kittify/skills-manifest.json`) | [`crate::skills`] |
//! | agent-profile projection manifest | [`crate::profiles`] |
//! | Vibe native-config glue (`.vibe/config.toml` skill path) | [`crate::vibe`] |
//! | governed user-global command directories | this module (see [`user_global_command_dir`]) |
//!
//! # Read-only contract
//!
//! Every function here is an audit: it reads bytes, hashes them and reports.
//! No function creates, rewrites, prunes or locks anything — including for the
//! managed-skill seams: [`crate::skills::run_surface_repair`] takes a
//! `repair_drift` flag, but its `false` value is *"do not overwrite drifted
//! bytes"*, **not** "dry run": it still restores missing files and rewrites the
//! manifest. It is therefore deliberately not called from this module. Callers
//! that want a mutation must go through the CLI's explicit, separately-owned
//! repair path.
//!
//! # Deliberate gaps (this module does not claim them)
//!
//! * `plugin_manifest` (plugin bundles) is plan-registered so its `--kind` token
//!   resolves, but has no native probe yet; it reports the reference's own
//!   `research-gap-surface` finding instead of a health verdict.
//! * Session-presence kinds (`context_file`, `rule`, `hook`) ARE probed by
//!   [`session_presence`], which consumes the one writer roster and read-only
//!   assessment in [`crate::session`]; a harness whose upstream writer is a
//!   no-op (`qwen`, `kilocode`, `auggie`, `q`) still reports the reference's
//!   research gap for those kinds, never a health verdict.
//! * `agent_profile` is manifest-driven only: without
//!   `.kittify/agent_profiles_manifest.json` the expected set cannot be
//!   enumerated (that needs the profile catalog), so it reports a research gap
//!   rather than an empty all-clear.
//! * Command-skill *collisions* are detected from unowned content in a canonical
//!   folder; the reference additionally compares against today's canonical
//!   rendering digest, which needs the template/pack renderer.
//! * Legacy dotted leftovers are reported, but `_legacy_owned` (an unrecorded
//!   file whose bytes are exactly the legacy rendering) is not computed.
//! * Pack-skill findings cover recorded pack-origin manifest entries only; an
//!   in-force pack skill that was never installed needs the pack catalog.
//! * An `agents.available`-less legacy config is refused by
//!   [`crate::init::read_configured_agents`] instead of falling back to
//!   every-agent (the reference's #6501 legacy path).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use sk_kernel::COMPAT_CORE_VERSION;

pub mod session_presence;

use crate::agent::{
    AgentSurface, CANONICAL_COMMANDS, CLI_WRAPPER_COMMANDS, PROMPT_BACKED_COMMANDS, agent_keys,
    agent_surface, agents,
};
use crate::error::InstallError;
use crate::init::read_configured_agents;
use crate::manifest::{AssetManifest, ManifestEntry, content_hash};
use crate::profiles::{PROFILES_MANIFEST_REL, ProfileManifest};
use crate::render::{
    COMMAND_SKILLS_ROOT, LEGACY_COMMAND_SKILL_PREFIX, SKILL_FILENAME, command_skill_name,
    command_skill_rel_path, legacy_command_skill_rel_path,
};
use crate::skills::{ManagedSkillManifest, ORIGIN_PACK};
use crate::vibe::{VIBE_CONFIG_REL, skill_path_configured};

// ---------------------------------------------------------------------------
// Stable wire vocabulary (the public JSON API of the reference)
// ---------------------------------------------------------------------------

/// Surface states (stable JSON wire strings).
pub const STATE_PRESENT: &str = "present";
/// See [`STATE_PRESENT`].
pub const STATE_MISSING: &str = "missing";
/// See [`STATE_PRESENT`].
pub const STATE_DRIFTED: &str = "drifted";
/// See [`STATE_PRESENT`].
pub const STATE_STALE: &str = "stale";
/// See [`STATE_PRESENT`].
pub const STATE_ORPHANED: &str = "orphaned";
/// See [`STATE_PRESENT`].
pub const STATE_UNSAFE: &str = "unsafe";
/// See [`STATE_PRESENT`].
pub const STATE_UNSUPPORTED: &str = "unsupported";
/// See [`STATE_PRESENT`].
pub const STATE_NOT_APPLICABLE: &str = "not_applicable";

/// Finding severities (stable JSON wire strings).
pub const SEVERITY_ERROR: &str = "error";
/// See [`SEVERITY_ERROR`].
pub const SEVERITY_WARNING: &str = "warning";
/// See [`SEVERITY_ERROR`].
pub const SEVERITY_INFO: &str = "info";

/// A generated surface is absent.
pub const GENERATED_SURFACE_MISSING: &str = "generated-surface-missing";
/// A managed file's bytes no longer match the recorded hash.
pub const MANAGED_FILE_DRIFT: &str = "managed-file-drift";
/// A generated surface is present but out of date (version marker / retired name).
pub const STALE_GENERATED_SURFACE: &str = "stale-generated-surface";
/// An always-on context file is absent or carries no managed orientation block.
pub const CONTEXT_FILE_MISSING: &str = "context-file-missing";
/// A session-presence rule file or lifecycle hook entry is absent.
pub const SESSION_PRESENCE_INCOMPLETE: &str = "session-presence-incomplete";
/// A session-presence destination exists but cannot be decoded.
pub const SESSION_PRESENCE_UNDECODABLE: &str = "session-presence-undecodable";
/// A managed path escapes the project or is not a regular file.
pub const UNSAFE_MANAGED_PATH: &str = "unsafe-managed-path";
/// Content inside a managed root that no manifest owns.
pub const UNMANAGED_SPEC_KITTY_SURFACE: &str = "unmanaged-spec-kitty-surface";
/// A configured tool is not recorded as an owner of the shared surface.
pub const CONFIGURED_TOOL_SURFACE_UNINSTALLED: &str = "configured-tool-surface-uninstalled";
/// Native-config glue for a tool is absent.
pub const NATIVE_CONFIG_MISSING: &str = "native-config-missing";
/// A projected native agent profile is absent.
pub const NATIVE_AGENT_PROFILE_MISSING: &str = "native-agent-profile-missing";
/// A projected native agent profile no longer matches the manifest hash.
pub const NATIVE_AGENT_PROFILE_DRIFT: &str = "native-agent-profile-drift";
/// The surface is not implemented/known for the tool.
pub const RESEARCH_GAP_SURFACE: &str = "research-gap-surface";

/// The scheme version of the `doctor tool-surfaces --json` document.
pub const REPORT_SCHEMA_VERSION: u32 = 1;

/// Repair hint for command-file surfaces.
pub const COMMAND_FILE_REPAIR_HINT: &str =
    "spec-kitty doctor tool-surfaces --kind command-file --fix";
/// Repair hint for command-skill surfaces.
pub const COMMAND_SKILL_REPAIR_HINT: &str =
    "spec-kitty doctor tool-surfaces --kind command-skill --fix";
/// Drift-specific repair hint (an edited managed file is only replaced explicitly).
pub const DRIFT_REPAIR_HINT: &str = "spec-kitty doctor tool-surfaces --fix";
/// Repair hint for charter-skill surfaces.
pub const CHARTER_SKILL_REPAIR_HINT: &str =
    "spec-kitty doctor tool-surfaces --kind charter-skill --fix";
/// Repair hint for native-config surfaces.
pub const NATIVE_CONFIG_REPAIR_HINT: &str =
    "spec-kitty doctor tool-surfaces --kind native_config --fix";
/// Repair hint for agent-profile surfaces.
pub const AGENT_PROFILE_REPAIR_HINT: &str =
    "spec-kitty doctor tool-surfaces --kind agent-profile --fix";
/// Repair hint for session-presence context-file **and** rule surfaces.
///
/// The reference uses this single hint for both kinds (its `_REPAIR_HINT`), so a
/// missing rule file points at `--kind context_file`; reproduced as-is.
pub const SESSION_CONTEXT_REPAIR_HINT: &str =
    "spec-kitty doctor tool-surfaces --kind context_file --fix";
/// Repair hint for session-presence hook surfaces (the reference's
/// `_HOOK_REPAIR_HINT`).
pub const SESSION_HOOK_REPAIR_HINT: &str = "spec-kitty doctor tool-surfaces --kind hook --fix";

// ---------------------------------------------------------------------------
// Kinds, states, selections
// ---------------------------------------------------------------------------

/// The kind of surface a definition describes (the reference's `ToolSurfaceKind`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum SurfaceKind {
    /// Generated command skill (`.agents/skills/spec-kitty-<command>/SKILL.md`).
    CommandSkill,
    /// Managed charter/pack skill recorded in `.kittify/skills-manifest.json`.
    CharterSkill,
    /// Harness context file (session presence).
    ContextFile,
    /// Harness rule file (session presence).
    Rule,
    /// Harness hook (session presence).
    Hook,
    /// Projected native agent profile.
    AgentProfile,
    /// Plugin bundle manifest.
    PluginManifest,
    /// Tool-specific native config glue.
    NativeConfig,
    /// User-global slash-command file.
    CommandFile,
}

impl SurfaceKind {
    /// The stable JSON wire value (`command_skill`, …).
    pub fn wire(self) -> &'static str {
        match self {
            Self::CommandSkill => "command_skill",
            Self::CharterSkill => "charter_skill",
            Self::ContextFile => "context_file",
            Self::Rule => "rule",
            Self::Hook => "hook",
            Self::AgentProfile => "agent_profile",
            Self::PluginManifest => "plugin_manifest",
            Self::NativeConfig => "native_config",
            Self::CommandFile => "command_file",
        }
    }

    /// Every kind, in the reference's enum order.
    pub fn all() -> [SurfaceKind; 9] {
        [
            Self::CommandSkill,
            Self::CharterSkill,
            Self::ContextFile,
            Self::Rule,
            Self::Hook,
            Self::AgentProfile,
            Self::PluginManifest,
            Self::NativeConfig,
            Self::CommandFile,
        ]
    }
}

/// The operator-facing `--kind` tokens, kebab-case plus the underscore wire
/// spellings the reference also accepts for some kinds.
pub const KIND_TOKENS: [(&str, SurfaceKind); 12] = [
    ("command-skill", SurfaceKind::CommandSkill),
    ("charter-skill", SurfaceKind::CharterSkill),
    ("context-file", SurfaceKind::ContextFile),
    ("context_file", SurfaceKind::ContextFile),
    ("rule", SurfaceKind::Rule),
    ("hook", SurfaceKind::Hook),
    ("agent-profile", SurfaceKind::AgentProfile),
    ("plugin-manifest", SurfaceKind::PluginManifest),
    ("plugin_manifest", SurfaceKind::PluginManifest),
    ("native-config", SurfaceKind::NativeConfig),
    ("native_config", SurfaceKind::NativeConfig),
    ("command-file", SurfaceKind::CommandFile),
];

/// A refusal for an unknown `--kind` token (the reference's `UnknownSurfaceKind`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnknownSurfaceKind {
    /// The operator's token.
    pub token: String,
    /// The accepted tokens, sorted.
    pub known: Vec<&'static str>,
}

impl UnknownSurfaceKind {
    /// The operator-facing message (the reference's wording).
    pub fn message(&self) -> String {
        format!(
            "Unknown surface kind '{}'. Known kinds: {}.",
            self.token,
            self.known.join(", ")
        )
    }
}

/// Map a `--kind` token to its kind, refusing an unknown token.
pub fn surface_kind_from_token(token: &str) -> Result<SurfaceKind, UnknownSurfaceKind> {
    KIND_TOKENS
        .iter()
        .find(|(name, _)| *name == token)
        .map(|(_, kind)| *kind)
        .ok_or_else(|| {
            let known: Vec<&'static str> = KIND_TOKENS.iter().map(|(name, _)| *name).collect();
            let mut known = known;
            known.sort_unstable();
            UnknownSurfaceKind {
                token: token.to_string(),
                known,
            }
        })
}

// ---------------------------------------------------------------------------
// Findings
// ---------------------------------------------------------------------------

/// One diagnostic produced while probing a surface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SurfaceFinding {
    /// Stable kebab-case code (the public wire API).
    pub code: &'static str,
    /// `error`, `warning` or `info`.
    pub severity: &'static str,
    /// Operator-facing message.
    pub message: String,
    /// The tool key the finding belongs to.
    pub tool_key: Option<String>,
    /// The surface id the finding belongs to.
    pub surface_id: Option<String>,
    /// The inspected path, when the finding is about one.
    pub path: Option<PathBuf>,
    /// The command that can repair the surface, when one exists.
    pub repair_command: Option<&'static str>,
    /// Documentation reference, when the reference carries one.
    pub docs_ref: Option<String>,
    /// Extra machine-readable detail.
    pub details: BTreeMap<String, Value>,
}

impl SurfaceFinding {
    fn new(code: &'static str, severity: &'static str, message: String) -> Self {
        Self {
            code,
            severity,
            message,
            tool_key: None,
            surface_id: None,
            path: None,
            repair_command: None,
            docs_ref: None,
            details: BTreeMap::new(),
        }
    }

    fn with_tool(mut self, tool_key: &str) -> Self {
        self.tool_key = Some(tool_key.to_string());
        self
    }

    fn with_surface(mut self, surface_id: &str) -> Self {
        self.surface_id = Some(surface_id.to_string());
        self
    }

    fn with_path(mut self, path: &Path) -> Self {
        self.path = Some(path.to_path_buf());
        self
    }

    fn with_repair(mut self, command: &'static str) -> Self {
        self.repair_command = Some(command);
        self
    }

    fn with_detail(mut self, key: &str, value: Value) -> Self {
        self.details.insert(key.to_string(), value);
        self
    }

    /// The schema-conformant JSON shape.
    pub fn to_json(&self) -> Value {
        let mut details = Map::new();
        for (key, value) in &self.details {
            details.insert(key.clone(), value.clone());
        }
        json!({
            "code": self.code,
            "severity": self.severity,
            "message": self.message,
            "tool_key": self.tool_key,
            "surface_id": self.surface_id,
            "path": self.path.as_ref().map(|path| path_string(path)),
            "repair_command": self.repair_command,
            "docs_ref": self.docs_ref,
            "details": Value::Object(details),
        })
    }
}

fn path_string(path: &Path) -> String {
    path.display().to_string()
}

// ---------------------------------------------------------------------------
// Definitions / instances / statuses / report
// ---------------------------------------------------------------------------

/// The policy description of a surface that should exist for a tool.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SurfaceDefinition {
    /// Surface kind.
    pub kind: SurfaceKind,
    /// `checked_in`, `generated`, `user_global`, `package` or `plugin`.
    pub source_kind: &'static str,
    /// `project`, `user_global`, `team` or `plugin_bundle`.
    pub install_scope: &'static str,
    /// Display pattern of the surface path.
    pub path_pattern: &'static str,
    /// `required`, `repairable_required`, `optional` or `research_gap`.
    pub required_policy: &'static str,
    /// `always`, `glob`, `model_decision`, `manual`, `user_invoked`,
    /// `skills_invokable`, `event` or `disabled`.
    pub activation_mode: &'static str,
    /// The provider that owns probe/repair for this definition.
    pub provider_key: &'static str,
    /// The operator-facing repair hint.
    pub repair_hint: &'static str,
}

/// One concrete materialization of a definition at a path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SurfaceInstance {
    /// The owning definition.
    pub definition: SurfaceDefinition,
    /// Absolute path of the surface.
    pub path: PathBuf,
    /// The tool key the surface belongs to.
    pub owner: String,
    /// The manifest-recorded hash, when a manifest owns the surface.
    pub file_hash: Option<String>,
    /// An explicit stable surface id.
    pub surface_id: Option<String>,
}

impl SurfaceInstance {
    /// The stable surface id (explicit, else derived like the reference).
    fn id(&self) -> String {
        if let Some(id) = &self.surface_id {
            return id.clone();
        }
        let suffix = match (
            self.path.file_name().and_then(|name| name.to_str()),
            self.path
                .parent()
                .and_then(|parent| parent.file_name())
                .and_then(|name| name.to_str()),
        ) {
            (Some(name), Some(parent)) if name == SKILL_FILENAME => {
                format!("{parent}.{SKILL_FILENAME}")
            }
            (Some(name), _) => name.to_string(),
            (None, _) => self.path.display().to_string(),
        };
        format!("{}.{}.{suffix}", self.owner, self.definition.kind.wire())
    }
}

/// The probe result for one instance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SurfaceStatus {
    /// The probed instance.
    pub instance: SurfaceInstance,
    /// One of the `STATE_*` wire strings.
    pub state: &'static str,
    /// Findings raised by the probe.
    pub findings: Vec<SurfaceFinding>,
}

impl SurfaceStatus {
    fn healthy(instance: SurfaceInstance) -> Self {
        Self {
            instance,
            state: STATE_PRESENT,
            findings: Vec::new(),
        }
    }

    fn with(instance: SurfaceInstance, state: &'static str, finding: SurfaceFinding) -> Self {
        Self {
            instance,
            state,
            findings: vec![finding],
        }
    }

    /// The `SurfaceStatusEntry` JSON shape.
    pub fn to_json(&self) -> Value {
        let repair_command = self
            .findings
            .iter()
            .find_map(|finding| finding.repair_command);
        json!({
            "id": self.instance.id(),
            "tool": self.instance.owner,
            "kind": self.instance.definition.kind.wire(),
            "provider": self.instance.definition.provider_key,
            "path": path_string(&self.instance.path),
            "state": self.state,
            "source_kind": self.instance.definition.source_kind,
            "manifest": Value::Null,
            "repair_command": repair_command,
        })
    }
}

/// Aggregate counts over the probed surfaces.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SurfaceSummary {
    /// Every probed surface.
    pub surfaces: usize,
    /// Surfaces in state `present`.
    pub present: usize,
    /// Surfaces in state `missing`.
    pub missing: usize,
    /// Surfaces in state `drifted`.
    pub drifted: usize,
    /// `warning`-severity findings.
    pub warnings: usize,
    /// `error`-severity findings.
    pub errors: usize,
}

impl SurfaceSummary {
    /// The `SurfaceSummary` JSON shape.
    pub fn to_json(self) -> Value {
        json!({
            "surfaces": self.surfaces,
            "present": self.present,
            "missing": self.missing,
            "drifted": self.drifted,
            "warnings": self.warnings,
            "errors": self.errors,
        })
    }
}

/// One planned tool: its selected definitions and expanded instances.
#[derive(Clone, Debug, Default)]
struct SurfacePlan {
    instances: Vec<SurfaceInstance>,
}

/// The complete read-only outcome of a `doctor tool-surfaces` run.
#[derive(Clone, Debug)]
pub struct SurfaceReport {
    /// The project root the audit ran against.
    pub project_root: PathBuf,
    /// The configured tool roster the audit covers.
    pub configured_tools: Vec<String>,
    /// Aggregate counts.
    pub summary: SurfaceSummary,
    /// One status per probed instance, in plan order.
    pub surfaces: Vec<SurfaceStatus>,
    /// Every finding, in status order.
    pub findings: Vec<SurfaceFinding>,
}

impl SurfaceReport {
    /// Whether the audit is clean (no `error`-severity finding).
    pub fn ok(&self) -> bool {
        self.summary.errors == 0
    }

    /// The full `doctor tool-surfaces --json` shape.
    pub fn to_json(&self) -> Value {
        json!({
            "ok": self.ok(),
            "schema_version": REPORT_SCHEMA_VERSION,
            "project_root": self.project_root.display().to_string(),
            "configured_tools": self.configured_tools,
            "summary": self.summary.to_json(),
            "surfaces": self.surfaces.iter().map(SurfaceStatus::to_json).collect::<Vec<_>>(),
            "findings": self.findings.iter().map(SurfaceFinding::to_json).collect::<Vec<_>>(),
        })
    }
}

/// Request for a `doctor tool-surfaces` audit.
#[derive(Clone, Debug)]
pub struct SurfaceAuditRequest<'a> {
    /// The project root.
    pub project_root: &'a Path,
    /// The configured tool keys (the caller resolves `.kittify/config.yaml`).
    pub configured_tools: &'a [String],
    /// `--tool`: restrict the audit to one configured tool key.
    pub tool_filter: Option<&'a str>,
    /// `--kind`: restrict the audit to these kinds.
    pub kinds: Option<&'a [SurfaceKind]>,
}

// ---------------------------------------------------------------------------
// Plan / registry
// ---------------------------------------------------------------------------

fn command_skill_definition() -> SurfaceDefinition {
    SurfaceDefinition {
        kind: SurfaceKind::CommandSkill,
        source_kind: "generated",
        install_scope: "project",
        path_pattern: ".agents/skills/spec-kitty-{command}/SKILL.md",
        required_policy: "repairable_required",
        activation_mode: "skills_invokable",
        provider_key: "command_skills",
        repair_hint: COMMAND_SKILL_REPAIR_HINT,
    }
}

fn command_file_definition() -> SurfaceDefinition {
    SurfaceDefinition {
        kind: SurfaceKind::CommandFile,
        source_kind: "user_global",
        install_scope: "user_global",
        path_pattern: "<user-global>/spec-kitty.{command}",
        required_policy: "repairable_required",
        activation_mode: "user_invoked",
        provider_key: "slash_commands",
        repair_hint: COMMAND_FILE_REPAIR_HINT,
    }
}

fn session_presence_definition(kind: SurfaceKind) -> SurfaceDefinition {
    SurfaceDefinition {
        kind,
        source_kind: "generated",
        install_scope: "project",
        path_pattern: "<session-presence>",
        required_policy: "repairable_required",
        activation_mode: match kind {
            SurfaceKind::Hook => "event",
            SurfaceKind::Rule => "glob",
            _ => "always",
        },
        provider_key: "session_presence",
        repair_hint: match kind {
            SurfaceKind::Hook => SESSION_HOOK_REPAIR_HINT,
            _ => SESSION_CONTEXT_REPAIR_HINT,
        },
    }
}

fn native_config_definition() -> SurfaceDefinition {
    SurfaceDefinition {
        kind: SurfaceKind::NativeConfig,
        source_kind: "generated",
        install_scope: "project",
        path_pattern: VIBE_CONFIG_REL,
        required_policy: "repairable_required",
        activation_mode: "always",
        provider_key: "native_config",
        repair_hint: NATIVE_CONFIG_REPAIR_HINT,
    }
}

fn agent_profile_definition() -> SurfaceDefinition {
    SurfaceDefinition {
        kind: SurfaceKind::AgentProfile,
        source_kind: "generated",
        install_scope: "project",
        path_pattern: ".claude/agents/{profile_id}.md",
        required_policy: "repairable_required",
        activation_mode: "user_invoked",
        provider_key: "agent_profiles",
        repair_hint: AGENT_PROFILE_REPAIR_HINT,
    }
}

fn charter_skill_definition() -> SurfaceDefinition {
    SurfaceDefinition {
        kind: SurfaceKind::CharterSkill,
        source_kind: "generated",
        install_scope: "project",
        path_pattern: ".kittify/skills-manifest.json:{installed_path}",
        required_policy: "repairable_required",
        activation_mode: "skills_invokable",
        provider_key: "managed_skills",
        repair_hint: CHARTER_SKILL_REPAIR_HINT,
    }
}

fn plugin_manifest_definition() -> SurfaceDefinition {
    SurfaceDefinition {
        kind: SurfaceKind::PluginManifest,
        source_kind: "generated",
        install_scope: "plugin_bundle",
        path_pattern: "dist/spec-kitty-plugins/{target}/plugin.json",
        required_policy: "optional",
        activation_mode: "disabled",
        provider_key: "plugin_bundle",
        repair_hint: "spec-kitty doctor tool-surfaces --kind plugin-manifest --fix",
    }
}

/// The synthetic tool key the plugin-bundle provider is registered under.
pub const PLUGIN_BUNDLE_TOOL_KEY: &str = "plugin_bundle";

/// The definitions a tool key gets, in the reference's provider order.
fn definitions_for(tool_key: &str) -> Vec<SurfaceDefinition> {
    let mut definitions = Vec::new();
    if tool_key == PLUGIN_BUNDLE_TOOL_KEY {
        definitions.push(plugin_manifest_definition());
        return definitions;
    }
    let Ok(surface) = agent_surface(tool_key) else {
        return definitions;
    };
    if surface.installs_command_skills() {
        definitions.push(command_skill_definition());
    }
    if surface.command.is_some() {
        definitions.push(command_file_definition());
    }
    for kind in [
        SurfaceKind::ContextFile,
        SurfaceKind::Hook,
        SurfaceKind::Rule,
    ] {
        definitions.push(session_presence_definition(kind));
    }
    definitions.push(native_config_definition());
    definitions.push(agent_profile_definition());
    if surface.skill.primary_root().is_some() {
        definitions.push(charter_skill_definition());
    }
    definitions
}

fn build_plan(
    tool_key: &str,
    project_root: &Path,
    kinds: Option<&[SurfaceKind]>,
) -> Result<SurfacePlan, InstallError> {
    let mut plan = SurfacePlan {
        instances: Vec::new(),
    };
    for definition in definitions_for(tool_key) {
        if kinds.is_some_and(|set| !set.contains(&definition.kind)) {
            continue;
        }
        expand(&definition, tool_key, project_root, &mut plan.instances)?;
    }
    Ok(plan)
}

fn expand(
    definition: &SurfaceDefinition,
    tool_key: &str,
    project_root: &Path,
    instances: &mut Vec<SurfaceInstance>,
) -> Result<(), InstallError> {
    match definition.kind {
        SurfaceKind::CommandSkill => {
            expand_command_skills(definition, tool_key, project_root, instances)
        }
        SurfaceKind::CommandFile => expand_command_files(definition, tool_key, instances),
        SurfaceKind::CharterSkill => {
            expand_charter_skills(definition, tool_key, project_root, instances)
        }
        SurfaceKind::NativeConfig => {
            expand_native_config(definition, tool_key, project_root, instances)
        }
        SurfaceKind::AgentProfile => {
            expand_agent_profiles(definition, tool_key, project_root, instances)
        }
        SurfaceKind::ContextFile | SurfaceKind::Rule | SurfaceKind::Hook => {
            session_presence::expand(definition, tool_key, project_root, instances);
            Ok(())
        }
        _ => {
            // No native probe yet: report the reference's own research-gap
            // instance so the kind is never silently absent or "healthy".
            instances.push(SurfaceInstance {
                definition: definition.clone(),
                path: PathBuf::from(RESEARCH_GAP_SENTINEL),
                owner: tool_key.to_string(),
                file_hash: None,
                surface_id: None,
            });
            Ok(())
        }
    }
}

const RESEARCH_GAP_SENTINEL: &str = "<unsupported>";

fn expand_command_files(
    definition: &SurfaceDefinition,
    tool_key: &str,
    instances: &mut Vec<SurfaceInstance>,
) -> Result<(), InstallError> {
    let surface = agent_surface(tool_key)?;
    let Some(_command) = surface.command else {
        return Ok(());
    };
    let directory = user_global_command_dir(tool_key)?;
    for command in sorted_commands() {
        instances.push(SurfaceInstance {
            definition: definition.clone(),
            path: directory.join(command_file_name(surface, command)),
            owner: tool_key.to_string(),
            file_hash: None,
            surface_id: None,
        });
    }
    Ok(())
}

fn expand_command_skills(
    definition: &SurfaceDefinition,
    tool_key: &str,
    project_root: &Path,
    instances: &mut Vec<SurfaceInstance>,
) -> Result<(), InstallError> {
    let surface = agent_surface(tool_key)?;
    if !surface.installs_command_skills() {
        return Ok(());
    }
    let manifest = AssetManifest::load(project_root)?;
    let mut expected: BTreeSet<String> = BTreeSet::new();
    for command in CANONICAL_COMMANDS {
        let rel = command_skill_rel_path(command);
        expected.insert(rel.clone());
        let entry = manifest.as_ref().and_then(|manifest| manifest.find(&rel));
        instances.push(SurfaceInstance {
            definition: definition.clone(),
            path: project_root.join(&rel),
            owner: tool_key.to_string(),
            file_hash: entry.map(|entry| entry.content_hash.clone()),
            surface_id: Some(format!(
                "{tool_key}.{}.{}",
                SurfaceKind::CommandSkill.wire(),
                command_skill_name(command)
            )),
        });
    }
    let extras = command_skill_tree(project_root)?.extra_rels(&expected);
    for rel in extras {
        instances.push(SurfaceInstance {
            definition: definition.clone(),
            path: project_root.join(&rel),
            owner: tool_key.to_string(),
            file_hash: None,
            surface_id: Some(format!(
                "{tool_key}.{}.{}",
                SurfaceKind::CommandSkill.wire(),
                rel.trim_start_matches(COMMAND_SKILLS_ROOT)
                    .replace('/', ".")
            )),
        });
    }
    Ok(())
}

fn expand_charter_skills(
    definition: &SurfaceDefinition,
    tool_key: &str,
    project_root: &Path,
    instances: &mut Vec<SurfaceInstance>,
) -> Result<(), InstallError> {
    let Some(manifest) = ManagedSkillManifest::load_strict(project_root)? else {
        return Ok(());
    };
    for entry in manifest
        .entries
        .iter()
        .filter(|entry| entry.agent_key == tool_key)
    {
        instances.push(SurfaceInstance {
            definition: definition.clone(),
            path: project_root.join(&entry.installed_path),
            owner: tool_key.to_string(),
            file_hash: Some(entry.content_hash.clone()),
            surface_id: Some(format!(
                "{tool_key}.{}.{}.{}",
                SurfaceKind::CharterSkill.wire(),
                entry.skill_name,
                entry.source_file.replace('/', ".")
            )),
        });
    }
    Ok(())
}

fn expand_native_config(
    definition: &SurfaceDefinition,
    tool_key: &str,
    project_root: &Path,
    instances: &mut Vec<SurfaceInstance>,
) -> Result<(), InstallError> {
    if tool_key != "vibe" {
        instances.push(SurfaceInstance {
            definition: definition.clone(),
            path: PathBuf::from(RESEARCH_GAP_SENTINEL),
            owner: tool_key.to_string(),
            file_hash: None,
            surface_id: None,
        });
        return Ok(());
    }
    instances.push(SurfaceInstance {
        definition: definition.clone(),
        path: project_root.join(VIBE_CONFIG_REL),
        owner: tool_key.to_string(),
        file_hash: None,
        surface_id: None,
    });
    Ok(())
}

fn expand_agent_profiles(
    definition: &SurfaceDefinition,
    tool_key: &str,
    project_root: &Path,
    instances: &mut Vec<SurfaceInstance>,
) -> Result<(), InstallError> {
    let manifest = ProfileManifest::load(project_root)?;
    let mut found = false;
    for entry in manifest
        .entries()
        .filter(|entry| entry.tool_key == tool_key)
    {
        found = true;
        instances.push(SurfaceInstance {
            definition: definition.clone(),
            path: project_root.join(&entry.output_path),
            owner: tool_key.to_string(),
            file_hash: Some(entry.file_hash.clone()),
            surface_id: Some(format!("{tool_key}.agent_profile.{}", entry.profile_urn)),
        });
    }
    if !found {
        // Without a recorded projection the expected set cannot be enumerated
        // here (that needs the profile catalog): report the gap, never an
        // empty all-clear.
        instances.push(SurfaceInstance {
            definition: definition.clone(),
            path: PathBuf::from(RESEARCH_GAP_SENTINEL),
            owner: tool_key.to_string(),
            file_hash: None,
            surface_id: None,
        });
    }
    Ok(())
}

fn sorted_commands() -> Vec<&'static str> {
    let mut commands: Vec<&'static str> = CANONICAL_COMMANDS.to_vec();
    commands.sort_unstable();
    commands
}

/// The project-relative command-file name for `command` on `surface`.
///
/// Markdown families use `spec-kitty.<command>.md`; the prompt-markdown family
/// `spec-kitty.<command>.prompt.md`; the TOML family
/// `spec-kitty.<command>.toml` — the reference's `_compute_output_filename`.
pub fn command_file_name(surface: &AgentSurface, command: &str) -> String {
    match surface.command {
        Some(command_surface) => format!(
            "spec-kitty.{command}.{}",
            command_surface.family.extension()
        ),
        None => format!("spec-kitty.{command}.md"),
    }
}

// ---------------------------------------------------------------------------
// User-global command directories
// ---------------------------------------------------------------------------

/// The user-global command directory for `agent_key`.
///
/// Mirrors the reference's `runtime.agent_commands.get_global_command_dir`: the
/// project-relative command directory mirrored beneath the user's home
/// directory, except for the two harnesses that resolve their own config root
/// (`opencode`, `llxprt`).
///
/// This is the only place in the tree that computes that location; it is
/// deliberately not exported from the crate root. An agent without a
/// command-file surface has no global command directory, which is a typed
/// refusal rather than a guessed path.
pub fn user_global_command_dir(agent_key: &str) -> Result<PathBuf, InstallError> {
    let surface = agent_surface(agent_key)?;
    let command = surface.require_command_surface()?;
    match agent_key {
        "opencode" => Ok(open_code_config_root().join("commands")),
        "llxprt" => Ok(llxprt_config_root().join("commands")),
        _ => Ok(sk_kernel::paths::home_directory()
            .map_err(|error| {
                InstallError::io(
                    "resolve home directory",
                    Path::new("."),
                    std::io::Error::other(error.to_string()),
                )
            })?
            .join(command.directory)),
    }
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn open_code_config_root() -> PathBuf {
    if let Some(root) = env_path("OPENCODE_CONFIG_DIR") {
        return root;
    }
    if let Some(root) = env_path("XDG_CONFIG_HOME") {
        return root.join("opencode");
    }
    sk_kernel::paths::home_directory()
        .unwrap_or_default()
        .join(".config")
        .join("opencode")
}

fn llxprt_config_root() -> PathBuf {
    if let Some(root) = env_path("LLXPRT_CONFIG_HOME") {
        return root;
    }
    if cfg!(target_os = "macos") {
        return sk_kernel::paths::home_directory()
            .unwrap_or_default()
            .join("Library")
            .join("Preferences")
            .join("llxprt-code");
    }
    if cfg!(windows) {
        let app_data = env_path("APPDATA").unwrap_or_else(|| {
            sk_kernel::paths::home_directory()
                .unwrap_or_default()
                .join("AppData")
                .join("Roaming")
        });
        return app_data.join("llxprt-code").join("Config");
    }
    if let Some(root) = env_path("XDG_CONFIG_HOME") {
        return root.join("llxprt-code");
    }
    sk_kernel::paths::home_directory()
        .unwrap_or_default()
        .join(".config")
        .join("llxprt-code")
}

// ---------------------------------------------------------------------------
// Probing
// ---------------------------------------------------------------------------

const VERSION_MARKER_PREFIX: &str = "<!-- spec-kitty-command-version:";
const VERSION_MARKER_HEAD_LINES: usize = 20;

/// The expected version marker line for the current compatibility version.
pub fn expected_version_marker() -> String {
    format!("{VERSION_MARKER_PREFIX} {COMPAT_CORE_VERSION} -->")
}

/// Whether `bytes` carry the expected version marker inside the file head.
///
/// The reference scans the first [`VERSION_MARKER_HEAD_LINES`] lines so the
/// post-rename layout (frontmatter on line 1, marker after the closing `---`)
/// counts as current.
pub fn has_current_version_marker(bytes: &[u8]) -> bool {
    let expected = expected_version_marker();
    let text = String::from_utf8_lossy(bytes);
    text.lines()
        .take(VERSION_MARKER_HEAD_LINES)
        .any(|line| line.trim() == expected)
}

// ---------------------------------------------------------------------------
// Session-presence failure propagation
// ---------------------------------------------------------------------------

/// Which Source read phase produced a [`SessionSurfaceFailure`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionSurfacePhase {
    /// A filesystem read (`Path.read_bytes` / `Path.read_text`) raised `OSError`.
    Read,
    /// The bytes were read but not decoded (`UnicodeDecodeError`).
    Decode,
    /// The bytes were decoded but `json.loads` raised a failure that is **not**
    /// a `JSONDecodeError` — CPython's integer-digit ceiling (`ValueError`) or
    /// its recursion ceiling (`RecursionError`). The reference `_load` catches
    /// only `JSONDecodeError`, so this escapes the provider exactly like a read
    /// failure does and must not become a health finding.
    JsonLimit,
}

/// A session-presence read/decode/parse failure that the reference lets escape.
///
/// The reference `doctor tool-surfaces` path has no handler for these: a
/// non-UTF-8 orientation file (`UnicodeDecodeError` out of
/// `read_text(encoding="utf-8")`), an unreadable `.claude/settings.json`
/// (`OSError` out of `Path.read_bytes`), and a `json.loads` failure that is not
/// a `JSONDecodeError` (CPython's integer-digit `ValueError` / recursion
/// `RecursionError`) all propagate out of `provider.expand` /
/// `provider.probe` and abort the command. CF33 keeps the first two failure
/// phases; CF37 adds [`SessionSurfacePhase::JsonLimit`] for the third instead of
/// reporting a Source-valid-but-unparseable settings document as `missing`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionSurfaceFailure {
    path: PathBuf,
    phase: SessionSurfacePhase,
    detail: String,
}

impl SessionSurfaceFailure {
    /// A read failure (`OSError`) for `path`.
    pub fn read(path: impl Into<PathBuf>, detail: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            phase: SessionSurfacePhase::Read,
            detail: detail.into(),
        }
    }

    /// A decode failure (`UnicodeDecodeError`) for `path`.
    pub fn decode(path: impl Into<PathBuf>, detail: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            phase: SessionSurfacePhase::Decode,
            detail: detail.into(),
        }
    }

    /// A `json.loads` failure the reference never catches (the CPython integer
    /// or recursion ceiling) for `path`.
    pub fn json_limit(path: impl Into<PathBuf>, detail: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            phase: SessionSurfacePhase::JsonLimit,
            detail: detail.into(),
        }
    }

    /// The inspected destination.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The Source phase that failed.
    pub fn phase(&self) -> SessionSurfacePhase {
        self.phase
    }

    /// The CPython/OS detail verbatim (no traceback is invented).
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl std::fmt::Display for SessionSurfaceFailure {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "{}: {}", self.path.display(), self.detail)
    }
}

impl std::error::Error for SessionSurfaceFailure {}

/// A failed whole-surface audit.
///
/// The session variant exists because the reference aborts the command when a
/// provider read/decode fails; it is not a health verdict and must never be
/// rendered as a report.
#[derive(Debug)]
pub enum ToolSurfaceAuditFailure {
    /// A session-presence read/decode failure escaped the provider.
    Session(SessionSurfaceFailure),
    /// The pre-existing configuration / plan failure, unchanged.
    Install(InstallError),
}

impl std::fmt::Display for ToolSurfaceAuditFailure {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Session(failure) => write!(out, "{failure}"),
            Self::Install(error) => write!(out, "{error}"),
        }
    }
}

impl std::error::Error for ToolSurfaceAuditFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Session(failure) => Some(failure),
            Self::Install(error) => Some(error),
        }
    }
}

impl From<InstallError> for ToolSurfaceAuditFailure {
    fn from(error: InstallError) -> Self {
        Self::Install(error)
    }
}

fn probe(instance: &SurfaceInstance) -> Result<SurfaceStatus, SessionSurfaceFailure> {
    match instance.definition.kind {
        // Session-presence kinds own their own research-gap verdict: upstream's
        // provider checks the `<unsupported>` sentinel itself, so the generic
        // check below must not pre-empt it. They are also the only kinds whose
        // read/decode phases can fail instead of yielding a status (CF33).
        SurfaceKind::ContextFile | SurfaceKind::Rule | SurfaceKind::Hook => {
            session_presence::probe(instance)
        }
        _ if instance.path == Path::new(RESEARCH_GAP_SENTINEL) => Ok(research_gap(instance)),
        SurfaceKind::CommandFile => Ok(probe_command_file(instance)),
        SurfaceKind::CommandSkill => Ok(probe_command_skill(instance)),
        SurfaceKind::CharterSkill => Ok(probe_charter_skill(instance)),
        SurfaceKind::NativeConfig => Ok(probe_native_config(instance)),
        SurfaceKind::AgentProfile => Ok(probe_agent_profile(instance)),
        _ => Ok(research_gap(instance)),
    }
}

fn research_gap(instance: &SurfaceInstance) -> SurfaceStatus {
    let message = match instance.definition.kind {
        SurfaceKind::NativeConfig => {
            format!("No known native-config glue for {}.", instance.owner)
        }
        SurfaceKind::AgentProfile => format!(
            "No native agent-profile inventory for {}: `{}` is absent, so the expected \
             profile set cannot be enumerated (the profile catalog is not wired to this audit).",
            instance.owner, PROFILES_MANIFEST_REL
        ),
        kind => format!(
            "No native {} diagnostic for {}: the session-presence/plugin-bundle probe is not \
             implemented in this slice.",
            kind.wire(),
            instance.owner
        ),
    };
    let finding = SurfaceFinding::new(RESEARCH_GAP_SURFACE, SEVERITY_INFO, message)
        .with_tool(&instance.owner)
        .with_surface(&instance.id())
        .with_detail("native_probe", json!("missing"));
    SurfaceStatus::with(instance.clone(), STATE_NOT_APPLICABLE, finding)
}

fn probe_command_file(instance: &SurfaceInstance) -> SurfaceStatus {
    let id = instance.id();
    match fs::read(&instance.path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let finding = SurfaceFinding::new(
                GENERATED_SURFACE_MISSING,
                SEVERITY_ERROR,
                format!(
                    "User-global slash command missing: {}",
                    path_string(&instance.path)
                ),
            )
            .with_tool(&instance.owner)
            .with_surface(&id)
            .with_path(&instance.path)
            .with_repair(COMMAND_FILE_REPAIR_HINT);
            SurfaceStatus::with(instance.clone(), STATE_MISSING, finding)
        }
        Err(error) => {
            let finding = SurfaceFinding::new(
                GENERATED_SURFACE_MISSING,
                SEVERITY_ERROR,
                format!(
                    "User-global slash command is unreadable: {} ({error})",
                    path_string(&instance.path)
                ),
            )
            .with_tool(&instance.owner)
            .with_surface(&id)
            .with_path(&instance.path)
            .with_repair(COMMAND_FILE_REPAIR_HINT);
            SurfaceStatus::with(instance.clone(), STATE_MISSING, finding)
        }
        Ok(bytes) => {
            if has_current_version_marker(&bytes) {
                return SurfaceStatus::healthy(instance.clone());
            }
            let finding = SurfaceFinding::new(
                STALE_GENERATED_SURFACE,
                SEVERITY_WARNING,
                format!(
                    "User-global slash command is stale: {}",
                    path_string(&instance.path)
                ),
            )
            .with_tool(&instance.owner)
            .with_surface(&id)
            .with_path(&instance.path)
            .with_repair(COMMAND_FILE_REPAIR_HINT);
            SurfaceStatus::with(instance.clone(), STATE_STALE, finding)
        }
    }
}

fn probe_native_config(instance: &SurfaceInstance) -> SurfaceStatus {
    let project_root = project_of(instance, VIBE_CONFIG_REL);
    let configured = project_root.is_some_and(|root| skill_path_configured(&root));
    if configured {
        return SurfaceStatus::healthy(instance.clone());
    }
    let finding = SurfaceFinding::new(
        NATIVE_CONFIG_MISSING,
        SEVERITY_ERROR,
        format!(
            "Native-config glue missing for {}: {}",
            instance.owner,
            path_string(&instance.path)
        ),
    )
    .with_tool(&instance.owner)
    .with_surface(&instance.id())
    .with_path(&instance.path)
    .with_repair(NATIVE_CONFIG_REPAIR_HINT);
    SurfaceStatus::with(instance.clone(), STATE_MISSING, finding)
}

/// Recover the project root from a project-relative surface path, using the
/// project-relative suffix the caller planned the instance with.
fn project_of(instance: &SurfaceInstance, suffix: &str) -> Option<PathBuf> {
    let path = instance.path.to_str()?;
    let stripped = path.strip_suffix(suffix)?;
    let root = stripped.strip_suffix('/').unwrap_or(stripped);
    if root.is_empty() {
        return None;
    }
    Some(PathBuf::from(root))
}

fn probe_charter_skill(instance: &SurfaceInstance) -> SurfaceStatus {
    let id = instance.id();
    let metadata = fs::symlink_metadata(&instance.path);
    let missing = |suffix: &str| {
        let finding = SurfaceFinding::new(
            GENERATED_SURFACE_MISSING,
            SEVERITY_ERROR,
            format!(
                "Managed charter skill for {} is missing: {}",
                instance.owner,
                path_string(&instance.path)
            ),
        )
        .with_tool(&instance.owner)
        .with_surface(&id)
        .with_path(&instance.path)
        .with_repair(CHARTER_SKILL_REPAIR_HINT)
        .with_detail("state", json!(suffix));
        SurfaceStatus::with(instance.clone(), STATE_MISSING, finding)
    };
    let drifted = || {
        let finding = SurfaceFinding::new(
            MANAGED_FILE_DRIFT,
            SEVERITY_WARNING,
            format!(
                "Managed charter skill drifted from manifest hash: {}",
                path_string(&instance.path)
            ),
        )
        .with_tool(&instance.owner)
        .with_surface(&id)
        .with_path(&instance.path)
        .with_repair(CHARTER_SKILL_REPAIR_HINT);
        SurfaceStatus::with(instance.clone(), STATE_DRIFTED, finding)
    };
    let metadata = match metadata {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return missing("absent"),
        Err(error) => {
            let finding = SurfaceFinding::new(
                UNSAFE_MANAGED_PATH,
                SEVERITY_ERROR,
                format!(
                    "Managed charter skill is unreadable: {} ({error})",
                    path_string(&instance.path)
                ),
            )
            .with_tool(&instance.owner)
            .with_surface(&id)
            .with_path(&instance.path);
            return SurfaceStatus::with(instance.clone(), STATE_UNSAFE, finding);
        }
    };
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return drifted();
    }
    let Some(expected) = &instance.file_hash else {
        return SurfaceStatus::healthy(instance.clone());
    };
    match fs::read(&instance.path) {
        Ok(bytes) => {
            let observed = format!("sha256:{}", sha256_hex(&bytes));
            if &observed == expected {
                SurfaceStatus::healthy(instance.clone())
            } else {
                drifted()
            }
        }
        Err(_) => missing("unreadable"),
    }
}

fn probe_agent_profile(instance: &SurfaceInstance) -> SurfaceStatus {
    let id = instance.id();
    match fs::read(&instance.path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let finding = SurfaceFinding::new(
                NATIVE_AGENT_PROFILE_MISSING,
                SEVERITY_ERROR,
                format!(
                    "Native agent profile for {} is missing: {}",
                    instance.owner,
                    path_string(&instance.path)
                ),
            )
            .with_tool(&instance.owner)
            .with_surface(&id)
            .with_path(&instance.path)
            .with_repair(AGENT_PROFILE_REPAIR_HINT);
            SurfaceStatus::with(instance.clone(), STATE_MISSING, finding)
        }
        Err(_) => {
            let finding = SurfaceFinding::new(
                NATIVE_AGENT_PROFILE_DRIFT,
                SEVERITY_WARNING,
                format!(
                    "Native agent profile is unreadable: {}",
                    path_string(&instance.path)
                ),
            )
            .with_tool(&instance.owner)
            .with_surface(&id)
            .with_path(&instance.path)
            .with_repair(AGENT_PROFILE_REPAIR_HINT);
            SurfaceStatus::with(instance.clone(), STATE_DRIFTED, finding)
        }
        Ok(bytes) => {
            let matches = instance
                .file_hash
                .as_ref()
                .is_none_or(|expected| expected == &sha256_hex(&bytes));
            if matches {
                return SurfaceStatus::healthy(instance.clone());
            }
            let finding = SurfaceFinding::new(
                NATIVE_AGENT_PROFILE_DRIFT,
                SEVERITY_WARNING,
                format!(
                    "Native agent profile drifted from manifest hash: {}",
                    path_string(&instance.path)
                ),
            )
            .with_tool(&instance.owner)
            .with_surface(&id)
            .with_path(&instance.path)
            .with_repair(AGENT_PROFILE_REPAIR_HINT);
            SurfaceStatus::with(instance.clone(), STATE_DRIFTED, finding)
        }
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    content_hash(bytes)
}

fn probe_command_skill(instance: &SurfaceInstance) -> SurfaceStatus {
    let id = instance.id();
    let Some(project_root) = command_skill_project_root(&instance.path) else {
        return missing_command_skill(instance, &id);
    };
    let Ok(tree) = command_skill_tree(&project_root) else {
        return missing_command_skill(instance, &id);
    };
    let rel = instance
        .path
        .strip_prefix(&project_root)
        .ok()
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    if tree.unsafe_paths.contains(&rel) {
        let finding = SurfaceFinding::new(
            UNSAFE_MANAGED_PATH,
            SEVERITY_ERROR,
            format!(
                "Command skill path is unsafe or escapes the project root: {}",
                path_string(&instance.path)
            ),
        )
        .with_tool(&instance.owner)
        .with_surface(&id)
        .with_path(&instance.path);
        return SurfaceStatus::with(instance.clone(), STATE_UNSAFE, finding);
    }
    if tree.legacy.contains(&rel) || tree.legacy.contains(&format!("{rel}/")) {
        let finding = SurfaceFinding::new(
            STALE_GENERATED_SURFACE,
            SEVERITY_WARNING,
            format!(
                "Legacy dotted command skill (renamed to spec-kitty-<command>): {}",
                path_string(&instance.path)
            ),
        )
        .with_tool(&instance.owner)
        .with_surface(&id)
        .with_path(&instance.path);
        return SurfaceStatus::with(instance.clone(), STATE_STALE, finding);
    }
    if tree.stale.contains(&rel) {
        let finding = SurfaceFinding::new(
            STALE_GENERATED_SURFACE,
            SEVERITY_WARNING,
            format!(
                "Command skill is no longer canonical: {}",
                path_string(&instance.path)
            ),
        )
        .with_tool(&instance.owner)
        .with_surface(&id)
        .with_path(&instance.path);
        return SurfaceStatus::with(instance.clone(), STATE_STALE, finding);
    }
    if tree.collisions.contains(&rel) {
        let finding = SurfaceFinding::new(
            UNMANAGED_SPEC_KITTY_SURFACE,
            SEVERITY_WARNING,
            format!(
                "Unowned content occupies the canonical command-skill path {}; it is never overwritten",
                path_string(&instance.path)
            ),
        )
        .with_tool(&instance.owner)
        .with_surface(&id)
        .with_path(&instance.path);
        return SurfaceStatus::with(instance.clone(), STATE_PRESENT, finding);
    }
    if tree.orphans.contains(&rel) {
        let finding = SurfaceFinding::new(
            UNMANAGED_SPEC_KITTY_SURFACE,
            SEVERITY_WARNING,
            format!(
                "Spec Kitty-looking command skill is not owned by the manifest: {}",
                path_string(&instance.path)
            ),
        )
        .with_tool(&instance.owner)
        .with_surface(&id)
        .with_path(&instance.path);
        return SurfaceStatus::with(instance.clone(), STATE_ORPHANED, finding);
    }
    if tree.gaps.contains(&rel) {
        let finding = SurfaceFinding::new(
            GENERATED_SURFACE_MISSING,
            SEVERITY_ERROR,
            format!(
                "Command skill for {} is missing: {}",
                instance.owner,
                path_string(&instance.path)
            ),
        )
        .with_tool(&instance.owner)
        .with_surface(&id)
        .with_path(&instance.path)
        .with_repair(COMMAND_SKILL_REPAIR_HINT);
        return SurfaceStatus::with(instance.clone(), STATE_MISSING, finding);
    }
    if tree.uninstalled.contains(&id) {
        let finding = SurfaceFinding::new(
            CONFIGURED_TOOL_SURFACE_UNINSTALLED,
            SEVERITY_ERROR,
            format!(
                "Configured tool is not recorded as an owner of command skill: {}",
                path_string(&instance.path)
            ),
        )
        .with_tool(&instance.owner)
        .with_surface(&id)
        .with_path(&instance.path)
        .with_repair(COMMAND_SKILL_REPAIR_HINT);
        return SurfaceStatus::with(instance.clone(), STATE_MISSING, finding);
    }
    if !instance.path.exists() {
        return missing_command_skill(instance, &id);
    }
    if tree.drift.contains(&rel) {
        let finding = SurfaceFinding::new(
            MANAGED_FILE_DRIFT,
            SEVERITY_WARNING,
            format!(
                "Command skill drifted from manifest hash: {}; run '{}' to restore it",
                path_string(&instance.path),
                DRIFT_REPAIR_HINT
            ),
        )
        .with_tool(&instance.owner)
        .with_surface(&id)
        .with_path(&instance.path)
        .with_repair(DRIFT_REPAIR_HINT);
        return SurfaceStatus::with(instance.clone(), STATE_DRIFTED, finding);
    }
    SurfaceStatus::healthy(instance.clone())
}

fn missing_command_skill(instance: &SurfaceInstance, id: &str) -> SurfaceStatus {
    let finding = SurfaceFinding::new(
        GENERATED_SURFACE_MISSING,
        SEVERITY_ERROR,
        format!(
            "Command skill for {} is missing: {}",
            instance.owner,
            path_string(&instance.path)
        ),
    )
    .with_tool(&instance.owner)
    .with_surface(id)
    .with_path(&instance.path)
    .with_repair(COMMAND_SKILL_REPAIR_HINT);
    SurfaceStatus::with(instance.clone(), STATE_MISSING, finding)
}

fn command_skill_project_root(path: &Path) -> Option<PathBuf> {
    let mut root = PathBuf::new();
    let parts: Vec<&std::ffi::OsStr> = path.iter().collect();
    let index = parts.iter().position(|part| *part == ".agents")?;
    for part in &parts[..index] {
        root.push(part);
    }
    if root.as_os_str().is_empty() {
        return None;
    }
    Some(root)
}

// ---------------------------------------------------------------------------
// Command-skill tree scan (the single read-only classification seam)
// ---------------------------------------------------------------------------

/// The read-only classification of the project's `.agents/skills` tree, in the
/// reference's `command_installer.verify` vocabulary.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CommandSkillTree {
    /// Manifest entries whose bytes no longer match the recorded hash.
    pub drift: BTreeSet<String>,
    /// Manifest entries whose file is absent.
    pub gaps: BTreeSet<String>,
    /// Files inside a canonical command-skill folder that no manifest entry owns.
    pub orphans: BTreeSet<String>,
    /// Manifest entries for names that are no longer canonical (pre-rename paths).
    pub stale: BTreeSet<String>,
    /// Manifest entries or orphan candidates that escape the project root.
    pub unsafe_paths: BTreeSet<String>,
    /// Legacy dotted paths: recorded entries, on-disk `SKILL.md`s and leftover folders.
    pub legacy: BTreeSet<String>,
    /// Unowned content occupying a canonical `SKILL.md` path.
    pub collisions: BTreeSet<String>,
    /// Surface ids of canonical surfaces whose manifest entry omits the tool.
    pub uninstalled: BTreeSet<String>,
    /// Unrecorded dotted `SKILL.md`s (the reference's human-only `legacy` extras).
    pub legacy_owned: BTreeSet<String>,
}

impl CommandSkillTree {
    /// Every unowned/invalid relative path worth an extra surface instance.
    pub fn extra_rels(&self, expected: &BTreeSet<String>) -> Vec<String> {
        let mut extra: BTreeSet<String> = BTreeSet::new();
        for set in [
            &self.orphans,
            &self.stale,
            &self.unsafe_paths,
            &self.legacy,
            &self.collisions,
            &self.gaps,
            &self.drift,
        ] {
            extra.extend(set.iter().cloned());
        }
        extra
            .into_iter()
            .filter(|rel| !expected.contains(rel))
            .filter(|rel| !rel.ends_with('/'))
            .collect()
    }
}

fn is_legacy_rel_path(rel: &str) -> bool {
    rel.starts_with(&format!(
        "{COMMAND_SKILLS_ROOT}{LEGACY_COMMAND_SKILL_PREFIX}"
    ))
}

fn canonical_skill_rel_paths() -> BTreeSet<String> {
    CANONICAL_COMMANDS
        .iter()
        .map(|command| command_skill_rel_path(command))
        .collect()
}

fn is_canonical_rel_path(rel: &str) -> bool {
    CANONICAL_COMMANDS.iter().any(|command| {
        rel.starts_with(&format!(
            "{COMMAND_SKILLS_ROOT}{}/",
            command_skill_name(command)
        ))
    })
}

/// Scan the project's command-skill tree without mutating anything.
pub fn command_skill_tree(project_root: &Path) -> Result<CommandSkillTree, InstallError> {
    let manifest = AssetManifest::load(project_root)?;
    let mut tree = CommandSkillTree::default();
    let mut recorded: BTreeSet<String> = BTreeSet::new();
    if let Some(manifest) = &manifest {
        for entry in manifest.entries() {
            recorded.insert(entry.path.clone());
            classify_entry(project_root, entry, &mut tree);
        }
    }
    scan_legacy_folders(project_root, &recorded, &mut tree);
    scan_canonical_folders(project_root, &recorded, &mut tree)?;
    Ok(tree)
}

fn classify_entry(project_root: &Path, entry: &ManifestEntry, tree: &mut CommandSkillTree) {
    if is_legacy_rel_path(&entry.path) {
        tree.legacy.insert(entry.path.clone());
        return;
    }
    if sk_kernel::resolve_within(project_root, Path::new(&entry.path)).is_err() {
        tree.unsafe_paths.insert(entry.path.clone());
        return;
    }
    if !is_canonical_rel_path(&entry.path) {
        tree.stale.insert(entry.path.clone());
    }
    let absolute = project_root.join(&entry.path);
    match fs::read(&absolute) {
        Ok(bytes) => {
            if sha256_hex(&bytes) != entry.content_hash {
                tree.drift.insert(entry.path.clone());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            tree.gaps.insert(entry.path.clone());
        }
        Err(_) => {
            tree.unsafe_paths.insert(entry.path.clone());
        }
    }
}

fn scan_legacy_folders(
    project_root: &Path,
    recorded: &BTreeSet<String>,
    tree: &mut CommandSkillTree,
) {
    for command in CANONICAL_COMMANDS {
        let rel = legacy_command_skill_rel_path(command);
        let skill_md = project_root.join(&rel);
        let folder = match skill_md.parent() {
            Some(folder) => folder.to_path_buf(),
            None => continue,
        };
        if fs::symlink_metadata(&skill_md).is_ok() {
            tree.legacy.insert(rel.clone());
            if !recorded.contains(&rel) {
                tree.legacy_owned.insert(rel);
            }
        } else if fs::symlink_metadata(&folder).is_ok() {
            let parent = Path::new(&rel)
                .parent()
                .map(|parent| parent.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            tree.legacy.insert(format!("{parent}/"));
        }
    }
}

fn scan_canonical_folders(
    project_root: &Path,
    recorded: &BTreeSet<String>,
    tree: &mut CommandSkillTree,
) -> Result<(), InstallError> {
    let canonical = canonical_skill_rel_paths();
    for rel in canonical {
        let skill_md = project_root.join(&rel);
        let folder = match skill_md.parent() {
            Some(folder) => folder.to_path_buf(),
            None => continue,
        };
        let folder_is_dir = fs::symlink_metadata(&folder)
            .map(|metadata| metadata.is_dir())
            .unwrap_or(false);
        if !folder_is_dir {
            continue;
        }
        if !recorded.contains(&rel) && folder_is_collision(&folder, &skill_md) {
            tree.collisions.insert(rel);
            continue;
        }
        collect_files(project_root, &folder, recorded, tree)?;
    }
    Ok(())
}

/// The reference's FR-019 collision shape, minus the canonical-rendering digest:
/// a foreign folder, or an unowned regular `SKILL.md` (or leftover files where
/// the `SKILL.md` is absent).
fn folder_is_collision(folder: &Path, skill_md: &Path) -> bool {
    if fs::symlink_metadata(folder).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return false;
    }
    match fs::symlink_metadata(skill_md) {
        Err(_) => directory_has_entries(folder),
        Ok(metadata) => metadata.file_type().is_symlink() || !metadata.file_type().is_file(),
    }
}

fn directory_has_entries(folder: &Path) -> bool {
    fs::read_dir(folder)
        .map(|entries| entries.flatten().next().is_some())
        .unwrap_or(false)
}

fn collect_files(
    project_root: &Path,
    folder: &Path,
    recorded: &BTreeSet<String>,
    tree: &mut CommandSkillTree,
) -> Result<(), InstallError> {
    let mut pending = vec![folder.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => return Err(InstallError::io("read dir", &directory, source)),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(_) => continue,
            };
            if metadata.is_dir() {
                pending.push(path);
                continue;
            }
            if !metadata.file_type().is_file() {
                continue;
            }
            let Ok(relative) = path.strip_prefix(project_root) else {
                continue;
            };
            let rel = relative.to_string_lossy().replace('\\', "/");
            if sk_kernel::resolve_within(project_root, relative).is_err() {
                tree.unsafe_paths.insert(rel);
                continue;
            }
            if !recorded.contains(&rel) {
                tree.orphans.insert(rel);
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Audit entry point
// ---------------------------------------------------------------------------

/// Audit every configured tool surface (read-only).
///
/// A session-presence read/decode failure the reference lets escape aborts the
/// audit with [`ToolSurfaceAuditFailure::Session`]; it is never folded into a
/// report. Every other outcome keeps the released schema and exit contract.
pub fn audit_tool_surfaces(
    request: &SurfaceAuditRequest<'_>,
) -> Result<SurfaceReport, ToolSurfaceAuditFailure> {
    let tools = selected_tools(request.configured_tools, request.tool_filter);
    let mut plan_tools: Vec<String> = tools.clone();
    // The plugin-bundle provider aggregates across tools, so it is planned under
    // its own synthetic key whenever no `--tool` filter is active.
    if !tools.is_empty() && request.tool_filter.is_none() {
        plan_tools.push(PLUGIN_BUNDLE_TOOL_KEY.to_string());
    }
    let mut surfaces: Vec<SurfaceStatus> = Vec::new();
    for tool_key in &plan_tools {
        let plan = build_plan(tool_key, request.project_root, request.kinds)?;
        for instance in plan.instances {
            surfaces.push(probe(&instance).map_err(ToolSurfaceAuditFailure::Session)?);
        }
    }
    let mut findings: Vec<SurfaceFinding> = Vec::new();
    let mut summary = SurfaceSummary {
        surfaces: surfaces.len(),
        ..SurfaceSummary::default()
    };
    for status in &surfaces {
        match status.state {
            STATE_PRESENT => summary.present += 1,
            STATE_MISSING => summary.missing += 1,
            STATE_DRIFTED => summary.drifted += 1,
            _ => {}
        }
        for finding in &status.findings {
            match finding.severity {
                SEVERITY_ERROR => summary.errors += 1,
                SEVERITY_WARNING => summary.warnings += 1,
                _ => {}
            }
            findings.push(finding.clone());
        }
    }
    Ok(SurfaceReport {
        project_root: request.project_root.to_path_buf(),
        configured_tools: tools,
        summary,
        surfaces,
        findings,
    })
}

fn selected_tools(configured: &[String], filter: Option<&str>) -> Vec<String> {
    match filter {
        None => configured.to_vec(),
        Some(tool) => configured
            .iter()
            .filter(|key| key.as_str() == tool)
            .cloned()
            .collect(),
    }
}

/// The configured tool roster of a project, refusing an unreadable/invalid
/// `.kittify/config.yaml`.
///
/// See the module note: an `agents.available`-less config is refused here rather
/// than widened to every agent.
pub fn configured_tools(project_root: &Path) -> Result<Vec<String>, InstallError> {
    let mut tools = read_configured_agents(project_root)?;
    tools.sort();
    tools.dedup();
    Ok(tools)
}

// ---------------------------------------------------------------------------
// `doctor command-files`
// ---------------------------------------------------------------------------

/// One `doctor command-files` issue row (the reference's dict shape).
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct CommandFileIssue {
    /// The agent key the file belongs to.
    pub agent: String,
    /// The command name.
    pub command: String,
    /// The inspected path.
    pub file: PathBuf,
    /// The issue text.
    pub issue: String,
    /// `error` or `warning`.
    pub severity: &'static str,
}

impl CommandFileIssue {
    /// The reference's JSON row shape.
    pub fn to_json(&self) -> Value {
        json!({
            "agent": self.agent,
            "command": self.command,
            "file": self.file.display().to_string(),
            "issue": self.issue,
            "severity": self.severity,
        })
    }
}

/// Audit every configured agent's user-global command files (read-only).
///
/// Rows are in the reference's order: configured agent, then command name, then
/// the marker check before the shape check.
pub fn audit_command_files(project_root: &Path) -> Result<Vec<CommandFileIssue>, InstallError> {
    let dirs = command_dirs(project_root)?;
    Ok(audit_command_files_in(project_root, &dirs))
}

/// The user-global command directory of every configured slash-command agent.
pub type CommandDirs = BTreeMap<String, PathBuf>;

/// Resolve the user-global command directories for the configured agents.
pub fn command_dirs(project_root: &Path) -> Result<CommandDirs, InstallError> {
    let configured = configured_tools(project_root)?;
    let mut dirs = CommandDirs::new();
    for key in &configured {
        let Ok(surface) = agent_surface(key) else {
            continue;
        };
        if surface.command.is_none() {
            continue;
        }
        dirs.insert(key.clone(), user_global_command_dir(key)?);
    }
    Ok(dirs)
}

/// Audit the configured agents' command files under already-resolved
/// directories (read-only). Kept separate so the resolution seam and the
/// inspection can be exercised independently.
pub fn audit_command_files_in(project_root: &Path, dirs: &CommandDirs) -> Vec<CommandFileIssue> {
    let mut issues = Vec::new();
    let configured = match configured_tools(project_root) {
        Ok(configured) => configured,
        Err(_) => dirs.keys().cloned().collect(),
    };
    for key in &configured {
        let Ok(surface) = agent_surface(key) else {
            continue;
        };
        if surface.command.is_none() {
            continue;
        }
        let Some(directory) = dirs.get(key) else {
            continue;
        };
        for command in sorted_commands() {
            let path = directory.join(command_file_name(surface, command));
            issues.extend(inspect_command_file(key, command, &path));
        }
    }
    issues
}

fn inspect_command_file(agent: &str, command: &str, path: &Path) -> Vec<CommandFileIssue> {
    let row = |issue: String, severity: &'static str| CommandFileIssue {
        agent: agent.to_string(),
        command: command.to_string(),
        file: path.to_path_buf(),
        issue,
        severity,
    };
    let bytes = match fs::read(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return vec![row("missing".to_string(), SEVERITY_ERROR)];
        }
        Err(error) => {
            return vec![row(format!("unreadable: {error}"), SEVERITY_ERROR)];
        }
        Ok(bytes) => bytes,
    };
    let text = match std::str::from_utf8(&bytes) {
        Ok(text) => text.to_string(),
        Err(error) => {
            return vec![row(format!("unreadable: {error}"), SEVERITY_ERROR)];
        }
    };
    let mut issues = Vec::new();
    if !has_current_version_marker(&bytes) {
        issues.push(row(
            format!(
                "stale or missing version marker (expected: {})",
                expected_version_marker()
            ),
            SEVERITY_WARNING,
        ));
    }
    let non_empty = text.lines().filter(|line| !line.trim().is_empty()).count();
    if is_prompt_backed(command) && non_empty < PROMPT_DRIVEN_MIN_LINES {
        issues.push(row(
            format!(
                "prompt-driven command has only {non_empty} non-empty lines (expected >{PROMPT_DRIVEN_MIN_LINES})"
            ),
            SEVERITY_WARNING,
        ));
    } else if !is_prompt_backed(command) && non_empty >= CLI_DRIVEN_MIN_LINES {
        issues.push(row(
            format!(
                "CLI-driven command has {non_empty} non-empty lines (expected <{CLI_DRIVEN_MIN_LINES} for thin shim)"
            ),
            SEVERITY_WARNING,
        ));
    }
    issues
}

/// The prompt-backed line floor (the reference's `>50 non-empty lines`).
pub const PROMPT_DRIVEN_MIN_LINES: usize = 50;
/// The thin-shim line ceiling (the reference's `<15 non-empty lines`).
pub const CLI_DRIVEN_MIN_LINES: usize = 15;

/// Whether `command` renders as a full prompt rather than a thin CLI shim.
///
/// The canonical native roster splits the command set into
/// [`PROMPT_BACKED_COMMANDS`] and [`CLI_WRAPPER_COMMANDS`].
pub fn is_prompt_backed(command: &str) -> bool {
    PROMPT_BACKED_COMMANDS.contains(&command)
}

/// Whether `command` is a thin CLI wrapper for the roster's classification.
pub fn is_cli_wrapper(command: &str) -> bool {
    CLI_WRAPPER_COMMANDS.contains(&command)
}

// ---------------------------------------------------------------------------
// `doctor skills`
// ---------------------------------------------------------------------------

/// One `doctor skills` slash-command gap row.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct SlashCommandGap {
    /// The agent key.
    pub agent_key: String,
    /// The command name.
    pub command: String,
    /// The expected path.
    pub expected_path: PathBuf,
    /// `missing` or `stale`.
    pub status: &'static str,
}

impl SlashCommandGap {
    /// The reference's JSON row shape.
    pub fn to_json(&self) -> Value {
        json!({
            "agent_key": self.agent_key,
            "command": self.command,
            "expected_path": self.expected_path.display().to_string(),
            "status": self.status,
        })
    }
}

/// The fused slash-command section of `doctor skills`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SlashCommandAudit {
    /// The configured slash-command agents.
    pub configured_agents: Vec<String>,
    /// Detected gaps.
    pub gaps: Vec<SlashCommandGap>,
    /// Errors raised while loading (the reference's `errors` array).
    pub errors: Vec<String>,
}

impl SlashCommandAudit {
    /// Whether the section is clean.
    pub fn ok(&self) -> bool {
        self.gaps.is_empty() && self.errors.is_empty()
    }

    /// The reference's JSON shape.
    pub fn to_json(&self) -> Value {
        json!({
            "configured_agents": self.configured_agents,
            "gaps": self.gaps.iter().map(SlashCommandGap::to_json).collect::<Vec<_>>(),
            "repaired": Vec::<String>::new(),
            "errors": self.errors,
            "ok": self.ok(),
        })
    }
}

/// One pack-skill finding row (`doctor skills`' `pack_skills`).
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct PackSkillFinding {
    /// `missing`, `drift`, `stale` or `unresolvable`.
    pub kind: &'static str,
    /// The skill name.
    pub skill_name: String,
    /// The installed path.
    pub installed_path: String,
    /// The pack source reference.
    pub source_ref: String,
    /// The finding message.
    pub message: String,
}

impl PackSkillFinding {
    /// The reference's JSON row shape.
    pub fn to_json(&self) -> Value {
        json!({
            "kind": self.kind,
            "skill_name": self.skill_name,
            "installed_path": self.installed_path,
            "source_ref": self.source_ref,
            "message": self.message,
        })
    }
}

/// One tool-folder finding row (the reference's `tool_folders`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolFolderFinding {
    /// The finding kind (`no_tool_folder`).
    pub kind: &'static str,
    /// The configured agents.
    pub configured_agents: Vec<String>,
    /// The operator-facing message.
    pub message: String,
}

impl ToolFolderFinding {
    /// The reference's JSON row shape.
    pub fn to_json(&self) -> Value {
        json!({
            "kind": self.kind,
            "configured_agents": self.configured_agents,
            "message": self.message,
        })
    }
}

/// The `doctor skills` payload (command-skill manifest drift plus the fused
/// slash-command, pack-skill and tool-folder sections).
#[derive(Clone, Debug, Default)]
pub struct SkillsAudit {
    /// Configured command-skill agents (the roster intersected with the config).
    pub configured_agents: Vec<String>,
    /// Agents recorded in the managed-skills manifest.
    pub manifest_agents: Vec<String>,
    /// Manifest entry count.
    pub entries: usize,
    /// The canonical command count.
    pub canonical_commands: usize,
    /// Edited managed files.
    pub drift: Vec<String>,
    /// Missing managed files.
    pub gaps: Vec<String>,
    /// Unmanaged files inside managed folders.
    pub orphans: Vec<String>,
    /// Managed files whose name is no longer canonical.
    pub stale: Vec<String>,
    /// Managed paths that escape the project.
    pub unsafe_paths: Vec<String>,
    /// Legacy dotted leftovers.
    pub legacy: Vec<String>,
    /// Legacy dotted leftovers the manifest still records (human-only in the reference).
    pub legacy_recorded: Vec<String>,
    /// Unowned content occupying a canonical path.
    pub collisions: Vec<String>,
    /// Configured command-skill agents with no manifest entry.
    pub uninstalled_agents: Vec<String>,
    /// Whether the configured `vibe` agent is missing its skill-path glue.
    pub vibe_config_missing: bool,
    /// The fused slash-command section.
    pub slash_commands: SlashCommandAudit,
    /// Recorded pack-skill findings.
    pub pack_skills: Vec<PackSkillFinding>,
    /// Tool-folder findings.
    pub tool_folders: Vec<ToolFolderFinding>,
}

impl SkillsAudit {
    /// Whether the audit is clean (the reference's `ok`).
    pub fn ok(&self) -> bool {
        self.drift.is_empty()
            && self.gaps.is_empty()
            && self.orphans.is_empty()
            && self.stale.is_empty()
            && self.unsafe_paths.is_empty()
            && self.legacy.is_empty()
            && self.collisions.is_empty()
            && self.uninstalled_agents.is_empty()
            && !self.vibe_config_missing
            && self.slash_commands.ok()
            && self.pack_skills.is_empty()
            && self.tool_folders.is_empty()
    }

    /// The frozen `doctor skills --json` envelope (human-only keys excluded).
    pub fn to_json(&self) -> Value {
        json!({
            "configured_agents": self.configured_agents,
            "manifest_agents": self.manifest_agents,
            "entries": self.entries,
            "canonical_commands": self.canonical_commands,
            "drift": self.drift,
            "gaps": self.gaps,
            "orphans": self.orphans,
            "stale": self.stale,
            "unsafe": self.unsafe_paths,
            "legacy": self.legacy,
            "collisions": self.collisions,
            "uninstalled_agents": self.uninstalled_agents,
            "vibe_config_missing": self.vibe_config_missing,
            "repaired_agents": Vec::<String>::new(),
            "repaired_paths": Vec::<String>::new(),
            "pruned": Vec::<String>::new(),
            "repaired_vibe_config": false,
            "repair_errors": Vec::<String>::new(),
            "ok": self.ok(),
            "slash_commands": self.slash_commands.to_json(),
            "pack_skills": self.pack_skills.iter().map(PackSkillFinding::to_json).collect::<Vec<_>>(),
            "tool_folders": self.tool_folders.iter().map(ToolFolderFinding::to_json).collect::<Vec<_>>(),
        })
    }
}

/// The number of canonical commands the command-skill surface covers.
pub fn canonical_command_count() -> usize {
    CANONICAL_COMMANDS.len()
}

/// Whether `agent_key` installs shared-root command skills.
pub fn installs_command_skills(agent_key: &str) -> bool {
    agent_surface(agent_key).is_ok_and(|surface| surface.installs_command_skills())
}

/// Audit the project's command-skill surfaces (read-only).
pub fn audit_skills(project_root: &Path) -> Result<SkillsAudit, InstallError> {
    let dirs = command_dirs(project_root)?;
    audit_skills_in(project_root, &dirs)
}

/// Audit the project's command-skill surfaces with already-resolved user-global
/// command directories (read-only).
pub fn audit_skills_in(
    project_root: &Path,
    dirs: &CommandDirs,
) -> Result<SkillsAudit, InstallError> {
    let configured = configured_tools(project_root)?;
    let configured_skill_agents: Vec<String> = configured
        .iter()
        .filter(|key| installs_command_skills(key))
        .cloned()
        .collect();
    // `doctor skills` reports the **command-skill** ownership manifest
    // (`.kittify/command-skills-manifest.json`); the managed charter-skill
    // manifest is a different surface (`doctor tool-surfaces --kind charter-skill`).
    let command_manifest = AssetManifest::load(project_root)?;
    let mut audit = SkillsAudit {
        configured_agents: configured_skill_agents.clone(),
        canonical_commands: CANONICAL_COMMANDS.len(),
        ..SkillsAudit::default()
    };
    if let Some(manifest) = &command_manifest {
        audit.entries = manifest.entries().len();
        audit.manifest_agents = manifest.owners().into_iter().collect();
    }
    audit.uninstalled_agents = configured_skill_agents
        .iter()
        .filter(|key| !audit.manifest_agents.contains(key))
        .cloned()
        .collect();
    audit.vibe_config_missing = configured_skill_agents.iter().any(|key| key == "vibe")
        && !skill_path_configured(project_root);
    collect_command_skill_findings(project_root, command_manifest.as_ref(), &mut audit)?;
    audit.slash_commands = audit_slash_commands(dirs, &configured)?;
    audit.pack_skills = pack_skill_findings(project_root)?;
    audit.tool_folders = tool_folder_findings(project_root, &configured)?;
    Ok(audit)
}

/// Fold the read-only command-skill tree classification into the payload.
///
/// Every list comes from one [`command_skill_tree`] scan so the `doctor skills`
/// payload and the `command-skill` tool-surface probe can never disagree.
fn collect_command_skill_findings(
    project_root: &Path,
    command_manifest: Option<&AssetManifest>,
    audit: &mut SkillsAudit,
) -> Result<(), InstallError> {
    let skill_tree = command_skill_tree(project_root)?;
    audit.drift = skill_tree.drift.iter().cloned().collect();
    audit.gaps = skill_tree.gaps.iter().cloned().collect();
    audit.orphans = skill_tree.orphans.iter().cloned().collect();
    audit.stale = skill_tree.stale.iter().cloned().collect();
    audit.unsafe_paths = skill_tree.unsafe_paths.iter().cloned().collect();
    audit.legacy = skill_tree.legacy.iter().cloned().collect();
    audit.collisions = skill_tree.collisions.iter().cloned().collect();
    audit.legacy_recorded = match command_manifest {
        Some(manifest) => audit
            .legacy
            .iter()
            .filter(|rel| manifest.find(rel).is_some())
            .cloned()
            .collect(),
        None => Vec::new(),
    };
    Ok(())
}

fn audit_slash_commands(
    dirs: &CommandDirs,
    configured: &[String],
) -> Result<SlashCommandAudit, InstallError> {
    let mut audit = SlashCommandAudit::default();
    let mut agents: Vec<String> = Vec::new();
    for key in configured {
        if agent_surface(key).is_ok_and(|surface| surface.command.is_some()) {
            agents.push(key.clone());
        }
    }
    agents.sort();
    for key in &agents {
        let Ok(surface) = agent_surface(key) else {
            continue;
        };
        let directory = dirs.get(key).cloned();
        let directory = match directory {
            Some(directory) => directory,
            None => match user_global_command_dir(key) {
                Ok(directory) => directory,
                Err(error) => {
                    audit.errors.push(format!("{key}: {error}"));
                    continue;
                }
            },
        };
        for command in sorted_commands() {
            let path = directory.join(command_file_name(surface, command));
            match fs::read(&path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    audit.gaps.push(SlashCommandGap {
                        agent_key: key.clone(),
                        command: command.to_string(),
                        expected_path: path,
                        status: "missing",
                    });
                }
                Err(error) => audit.errors.push(format!("{}: {error}", path.display())),
                Ok(bytes) => {
                    if !has_current_version_marker(&bytes) {
                        audit.gaps.push(SlashCommandGap {
                            agent_key: key.clone(),
                            command: command.to_string(),
                            expected_path: path,
                            status: "stale",
                        });
                    }
                }
            }
        }
    }
    audit.configured_agents = agents;
    Ok(audit)
}

fn pack_skill_findings(project_root: &Path) -> Result<Vec<PackSkillFinding>, InstallError> {
    let Some(manifest) = ManagedSkillManifest::load_strict(project_root)? else {
        return Ok(Vec::new());
    };
    let mut findings = Vec::new();
    for entry in manifest
        .entries
        .iter()
        .filter(|entry| entry.origin == ORIGIN_PACK)
    {
        let path = project_root.join(&entry.installed_path);
        match fs::read(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                findings.push(PackSkillFinding {
                    kind: "missing",
                    skill_name: entry.skill_name.clone(),
                    installed_path: entry.installed_path.clone(),
                    source_ref: entry.source_ref.clone(),
                    message: format!(
                        "Pack skill {} is recorded but absent: {}",
                        entry.skill_name, entry.installed_path
                    ),
                });
            }
            Err(error) => findings.push(PackSkillFinding {
                kind: "drift",
                skill_name: entry.skill_name.clone(),
                installed_path: entry.installed_path.clone(),
                source_ref: entry.source_ref.clone(),
                message: format!(
                    "Pack skill {} is unreadable: {} ({error})",
                    entry.skill_name, entry.installed_path
                ),
            }),
            Ok(bytes) => {
                let observed = format!("sha256:{}", sha256_hex(&bytes));
                if observed != entry.content_hash {
                    findings.push(PackSkillFinding {
                        kind: "drift",
                        skill_name: entry.skill_name.clone(),
                        installed_path: entry.installed_path.clone(),
                        source_ref: entry.source_ref.clone(),
                        message: format!(
                            "Pack skill {} drifted from its recorded hash: {}",
                            entry.skill_name, entry.installed_path
                        ),
                    });
                }
            }
        }
    }
    Ok(findings)
}

fn tool_folder_findings(
    project_root: &Path,
    configured: &[String],
) -> Result<Vec<ToolFolderFinding>, InstallError> {
    let mut roots: BTreeSet<&'static str> = BTreeSet::new();
    for key in configured {
        if let Ok(surface) = agent_surface(key)
            && let Some(folder) = surface.folder
        {
            roots.insert(folder.trim_end_matches('/'));
        }
    }
    if roots.is_empty() || roots.iter().any(|root| project_root.join(root).is_dir()) {
        return Ok(Vec::new());
    }
    Ok(vec![ToolFolderFinding {
        kind: "no_tool_folder",
        configured_agents: configured.to_vec(),
        message: format!(
            "none of the configured tool folders exists ({}) for agents {}; run `spec-kitty upgrade` to recreate configured folders, or `spec-kitty agent config remove <agent>` to drop a tool",
            roots.iter().copied().collect::<Vec<_>>().join(", "),
            configured.join(", ")
        ),
    }])
}

/// The configured agent keys in canonical roster order (used by callers that
/// want the reference's `_configured_tool_keys` ordering).
pub fn configured_tool_keys(project_root: &Path) -> Result<Vec<String>, InstallError> {
    let configured = configured_tools(project_root)?;
    let mut ordered: Vec<String> = agent_keys()
        .filter(|key| configured.iter().any(|configured| configured == key))
        .map(str::to_string)
        .collect();
    ordered.sort();
    Ok(ordered)
}

/// The full agent roster (all 17 keys), for callers that render a roster table.
pub fn all_agents() -> &'static [AgentSurface] {
    agents()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::MANIFEST_REL;
    use crate::skills::SKILLS_MANIFEST_REL;

    fn write(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent");
        }
        fs::write(path, text).expect("write");
    }

    fn config(root: &Path, agents: &[&str]) {
        let list: Vec<String> = agents.iter().map(|key| format!("  - {key}")).collect();
        write(
            &root.join(".kittify/config.yaml"),
            &format!("agents:\n  available:\n{}\n", list.join("\n")),
        );
    }

    fn command_skill_bytes() -> String {
        format!(
            "---\nname: spec-kitty-plan\n---\n{} {{}}\nbody\n",
            expected_version_marker()
        )
    }

    #[test]
    fn kind_tokens_cover_every_documented_spelling() {
        assert_eq!(
            surface_kind_from_token("command-skill").expect("known"),
            SurfaceKind::CommandSkill
        );
        assert_eq!(
            surface_kind_from_token("native_config").expect("known"),
            SurfaceKind::NativeConfig
        );
        let error = surface_kind_from_token("nope").expect_err("unknown");
        assert!(error.message().starts_with("Unknown surface kind 'nope'"));
        assert!(error.message().contains("command-skill"));
    }

    #[test]
    fn version_marker_scan_reads_the_head_not_only_line_one() {
        let text = format!(
            "---\ndescription: x\n---\n{}\nbody\n",
            expected_version_marker()
        );
        assert!(has_current_version_marker(text.as_bytes()));
        let stale = "---\n---\n<!-- spec-kitty-command-version: 1.0.0 -->\n";
        assert!(!has_current_version_marker(stale.as_bytes()));
    }

    #[test]
    fn command_files_audit_reports_missing_stale_and_shape_findings() {
        let temp = tempfile::tempdir().expect("temp");
        config(temp.path(), &["claude"]);
        let home = temp.path().join("home");
        let dir = home.join(".claude/commands");
        // A current, prompt-shaped plan file.
        let long_body: String = (0..60)
            .map(|index| format!("line {index}\n"))
            .collect::<String>();
        write(
            &dir.join("spec-kitty.plan.md"),
            &format!("{}\n{long_body}", expected_version_marker()),
        );
        // A stale thin shim for the CLI wrapper `status`.
        write(&dir.join("spec-kitty.status.md"), "stale\n");

        let mut dirs = CommandDirs::new();
        dirs.insert("claude".to_string(), dir);
        let issues = audit_command_files_in(temp.path(), &dirs);
        let missing: Vec<_> = issues
            .iter()
            .filter(|issue| issue.issue == "missing")
            .collect();
        assert!(
            missing.iter().any(|issue| issue.command == "specify"),
            "every absent command is reported"
        );
        assert!(
            !issues
                .iter()
                .any(|issue| issue.command == "plan" && issue.severity == SEVERITY_ERROR),
            "a current prompt file is healthy"
        );
        let status: Vec<_> = issues
            .iter()
            .filter(|issue| issue.command == "status")
            .collect();
        assert!(
            status
                .iter()
                .any(|issue| issue.issue.contains("stale or missing version marker"))
        );
    }

    #[test]
    fn command_skill_tree_classifies_drift_gaps_orphans_and_legacy() {
        let temp = tempfile::tempdir().expect("temp");
        config(temp.path(), &["codex"]);
        let owner = temp.path().join(".agents/skills/spec-kitty-plan/SKILL.md");
        let bytes = command_skill_bytes();
        write(&owner, &bytes);
        let mut manifest = AssetManifest::empty();
        let hash = crate::manifest::content_hash(bytes.as_bytes());
        manifest.upsert(
            ManifestEntry::new(
                ".agents/skills/spec-kitty-plan/SKILL.md",
                hash.clone(),
                vec!["codex".to_string()],
                "2026-01-01T00:00:00+00:00",
                COMPAT_CORE_VERSION,
            )
            .expect("entry"),
        );
        manifest.upsert(
            ManifestEntry::new(
                ".agents/skills/spec-kitty-review/SKILL.md",
                hash,
                vec!["codex".to_string()],
                "2026-01-01T00:00:00+00:00",
                COMPAT_CORE_VERSION,
            )
            .expect("entry"),
        );
        write(
            &temp.path().join(".kittify/command-skills-manifest.json"),
            String::from_utf8(manifest.to_bytes())
                .expect("utf8")
                .as_str(),
        );
        // Drifted canonical file, an unowned extra file, and a dotted leftover.
        write(&owner, "edited by the operator\n");
        write(
            &temp.path().join(".agents/skills/spec-kitty-plan/extra.md"),
            "extra\n",
        );
        write(
            &temp
                .path()
                .join(".agents/skills/spec-kitty.review/SKILL.md"),
            "legacy\n",
        );

        let tree = command_skill_tree(temp.path()).expect("tree");
        assert!(
            tree.drift
                .contains(".agents/skills/spec-kitty-plan/SKILL.md")
        );
        assert!(
            tree.gaps
                .contains(".agents/skills/spec-kitty-review/SKILL.md")
        );
        assert!(
            tree.orphans
                .contains(".agents/skills/spec-kitty-plan/extra.md")
        );
        assert!(
            tree.legacy
                .contains(".agents/skills/spec-kitty.review/SKILL.md")
        );
    }

    #[test]
    fn tool_surface_audit_reports_missing_skill_and_keeps_the_tree_untouched() {
        let temp = tempfile::tempdir().expect("temp");
        config(temp.path(), &["codex"]);
        let report = audit_tool_surfaces(&SurfaceAuditRequest {
            project_root: temp.path(),
            configured_tools: &["codex".to_string()],
            tool_filter: None,
            kinds: Some(&[SurfaceKind::CommandSkill]),
        })
        .expect("audit");
        assert!(!report.ok());
        assert_eq!(report.summary.surfaces, CANONICAL_COMMANDS.len());
        assert_eq!(report.summary.missing, CANONICAL_COMMANDS.len());
        assert!(
            report
                .findings
                .iter()
                .all(|finding| finding.code == GENERATED_SURFACE_MISSING)
        );
        let payload = report.to_json();
        assert_eq!(payload["ok"], json!(false));
        assert_eq!(payload["schema_version"], json!(REPORT_SCHEMA_VERSION));
        assert_eq!(payload["surfaces"][0]["kind"], json!("command_skill"));
        assert_eq!(
            payload["surfaces"][0]["state"],
            json!("missing"),
            "a missing surface is never reported healthy"
        );
    }

    #[test]
    fn tool_filter_and_kind_filter_restrict_the_plan() {
        let temp = tempfile::tempdir().expect("temp");
        config(temp.path(), &["codex", "claude"]);
        let configured = vec!["claude".to_string(), "codex".to_string()];
        let filtered = audit_tool_surfaces(&SurfaceAuditRequest {
            project_root: temp.path(),
            configured_tools: &configured,
            tool_filter: Some("codex"),
            kinds: Some(&[SurfaceKind::CommandSkill]),
        })
        .expect("audit");
        assert_eq!(filtered.configured_tools, vec!["codex".to_string()]);
        assert!(
            filtered
                .surfaces
                .iter()
                .all(|status| status.instance.owner == "codex")
        );
        let unrelated = audit_tool_surfaces(&SurfaceAuditRequest {
            project_root: temp.path(),
            configured_tools: &configured,
            tool_filter: Some("gemini"),
            kinds: Some(&[SurfaceKind::CommandSkill]),
        })
        .expect("audit");
        assert_eq!(unrelated.summary.surfaces, 0);
        assert!(unrelated.configured_tools.is_empty());
    }

    #[test]
    fn a_remaining_unimplemented_kind_is_a_research_gap_not_a_health_claim() {
        // Superseded premise (CF28): session-presence kinds are now real probes,
        // so the original assertion (a configured `claude` has a research-gap
        // `context_file`) no longer holds. `plugin_manifest` is still a genuine
        // research gap and keeps the same state/severity contract pinned.
        let temp = tempfile::tempdir().expect("temp");
        config(temp.path(), &["claude"]);
        let report = audit_tool_surfaces(&SurfaceAuditRequest {
            project_root: temp.path(),
            configured_tools: &["claude".to_string()],
            tool_filter: None,
            kinds: Some(&[SurfaceKind::PluginManifest]),
        })
        .expect("audit");
        assert!(
            report
                .surfaces
                .iter()
                .all(|status| status.state == STATE_NOT_APPLICABLE),
            "a research gap must not be reported healthy"
        );
        assert!(report.findings.iter().all(
            |finding| finding.code == RESEARCH_GAP_SURFACE && finding.severity == SEVERITY_INFO
        ));
    }

    #[test]
    fn skills_audit_reports_missing_command_skills_and_no_slash_agents() {
        let temp = tempfile::tempdir().expect("temp");
        config(temp.path(), &["codex"]);
        let audit = audit_skills(temp.path()).expect("audit");
        assert_eq!(audit.configured_agents, vec!["codex".to_string()]);
        assert_eq!(audit.uninstalled_agents, vec!["codex".to_string()]);
        assert!(audit.slash_commands.configured_agents.is_empty());
        assert!(!audit.ok());
        let payload = audit.to_json();
        assert_eq!(
            payload["canonical_commands"],
            json!(CANONICAL_COMMANDS.len())
        );
        assert_eq!(payload["ok"], json!(false));
        assert_eq!(payload["uninstalled_agents"], json!(["codex"]));
    }

    #[test]
    fn skills_audit_is_healthy_when_every_manifest_entry_matches() {
        let temp = tempfile::tempdir().expect("temp");
        config(temp.path(), &["codex"]);
        let bytes = command_skill_bytes();
        let rel = ".agents/skills/spec-kitty-plan/SKILL.md";
        write(&temp.path().join(rel), &bytes);
        let mut manifest = AssetManifest::empty();
        manifest.upsert(
            ManifestEntry::new(
                rel,
                sha256_hex(bytes.as_bytes()),
                vec!["codex".to_string()],
                "2026-01-01T00:00:00+00:00",
                COMPAT_CORE_VERSION,
            )
            .expect("entry"),
        );
        write(
            &temp.path().join(MANIFEST_REL),
            String::from_utf8(manifest.to_bytes())
                .expect("utf8")
                .as_str(),
        );
        let mut dirs = CommandDirs::new();
        dirs.insert("codex".to_string(), temp.path().join("home/.agents/skills"));
        let audit = audit_skills_in(temp.path(), &dirs).expect("audit");
        assert!(audit.gaps.is_empty());
        assert!(audit.ok(), "audit: {audit:?}");
    }

    #[test]
    fn charter_skill_drift_is_detected_from_the_managed_manifest() {
        let temp = tempfile::tempdir().expect("temp");
        config(temp.path(), &["claude"]);
        let bytes = "# shipped\n";
        let mut manifest =
            ManagedSkillManifest::new(COMPAT_CORE_VERSION, "2026-01-01T00:00:00+00:00");
        manifest.add_entry(crate::skills::ManagedFileEntry::copied(
            "spec-kitty-implement-review",
            "SKILL.md",
            ".claude/skills/spec-kitty-implement-review/SKILL.md",
            "native-root-required",
            "claude",
            &format!("sha256:{}", sha256_hex(bytes.as_bytes())),
            "2026-01-01T00:00:00+00:00",
        ));
        write(
            &temp
                .path()
                .join(".claude/skills/spec-kitty-implement-review/SKILL.md"),
            "edited\n",
        );
        write(
            &temp.path().join(SKILLS_MANIFEST_REL),
            String::from_utf8(manifest.to_bytes().expect("bytes"))
                .expect("utf8")
                .as_str(),
        );
        let report = audit_tool_surfaces(&SurfaceAuditRequest {
            project_root: temp.path(),
            configured_tools: &["claude".to_string()],
            tool_filter: None,
            kinds: Some(&[SurfaceKind::CharterSkill]),
        })
        .expect("audit");
        assert_eq!(report.summary.surfaces, 1);
        assert_eq!(report.surfaces[0].state, STATE_DRIFTED);
        assert_eq!(report.findings[0].code, MANAGED_FILE_DRIFT);
        assert_eq!(report.summary.warnings, 1);
        assert_eq!(report.summary.errors, 0);
    }

    #[test]
    fn vibe_native_config_glue_is_probed_through_the_canonical_owner() {
        let temp = tempfile::tempdir().expect("temp");
        config(temp.path(), &["vibe"]);
        let missing = audit_tool_surfaces(&SurfaceAuditRequest {
            project_root: temp.path(),
            configured_tools: &["vibe".to_string()],
            tool_filter: None,
            kinds: Some(&[SurfaceKind::NativeConfig]),
        })
        .expect("audit");
        assert_eq!(missing.surfaces[0].state, STATE_MISSING);
        assert_eq!(missing.findings[0].code, NATIVE_CONFIG_MISSING);

        write(
            &temp.path().join(VIBE_CONFIG_REL),
            "skill_paths = [\".agents/skills\"]\n",
        );
        let configured = audit_tool_surfaces(&SurfaceAuditRequest {
            project_root: temp.path(),
            configured_tools: &["vibe".to_string()],
            tool_filter: None,
            kinds: Some(&[SurfaceKind::NativeConfig]),
        })
        .expect("audit");
        assert_eq!(configured.surfaces[0].state, STATE_PRESENT);
    }

    #[test]
    fn a_non_vibe_tool_reports_the_native_config_research_gap() {
        let temp = tempfile::tempdir().expect("temp");
        let report = audit_tool_surfaces(&SurfaceAuditRequest {
            project_root: temp.path(),
            configured_tools: &["claude".to_string()],
            tool_filter: None,
            kinds: Some(&[SurfaceKind::NativeConfig]),
        })
        .expect("audit");
        assert_eq!(report.surfaces[0].state, STATE_NOT_APPLICABLE);
        assert_eq!(report.findings[0].code, RESEARCH_GAP_SURFACE);
    }

    #[test]
    fn the_plugin_bundle_key_is_planned_only_without_a_tool_filter() {
        let temp = tempfile::tempdir().expect("temp");
        let configured = vec!["claude".to_string()];
        let with_bundle = audit_tool_surfaces(&SurfaceAuditRequest {
            project_root: temp.path(),
            configured_tools: &configured,
            tool_filter: None,
            kinds: Some(&[SurfaceKind::PluginManifest]),
        })
        .expect("audit");
        assert_eq!(with_bundle.summary.surfaces, 1);
        assert_eq!(
            with_bundle.surfaces[0].instance.owner,
            PLUGIN_BUNDLE_TOOL_KEY
        );

        let filtered = audit_tool_surfaces(&SurfaceAuditRequest {
            project_root: temp.path(),
            configured_tools: &configured,
            tool_filter: Some("claude"),
            kinds: Some(&[SurfaceKind::PluginManifest]),
        })
        .expect("audit");
        assert_eq!(filtered.summary.surfaces, 0);
    }

    #[test]
    fn an_unreadable_config_is_a_typed_refusal() {
        let temp = tempfile::tempdir().expect("temp");
        write(
            &temp.path().join(".kittify/config.yaml"),
            "agents: [not a mapping]\n",
        );
        assert!(configured_tools(temp.path()).is_err());
    }
}
