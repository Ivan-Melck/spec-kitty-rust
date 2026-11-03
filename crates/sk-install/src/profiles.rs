//! Native agent-profile projection into host-native agent files.
//!
//! Upstream projects every admitted charter agent profile into the native
//! named-agent file a harness understands, records the result in
//! `.kittify/agent_profiles_manifest.json`, and never overwrites an operator's
//! edit. This module is the native counterpart for the project-local formats:
//!
//! | tool key | format | output |
//! | --- | --- | --- |
//! | `claude` | `claude-agent` | `.claude/agents/<profile-id>.md` |
//! | `copilot`, `vscode` | `copilot-agent` | `.github/agents/<profile-id>.agent.md` |
//! | `codex`, `codex-cli` | `codex-agent` | `.codex/agents/<profile-id>.toml` |
//! | `auggie`, `augment` | `augment-agent` | `.augment/agents/<profile-id>.md` |
//!
//! The Amazon Q format (`q`, `amazon-q`) is **user-global** and deliberately
//! not written here: upstream never writes it during `init`/repair-scaffold, it
//! is inspected/repaired from the filesystem by `doctor tool-surfaces`. Writing
//! `~/.aws/amazonq/cli-agents` from a project init would mutate state outside the
//! project for a tool the user may not have installed.
//!
//! Rendering is byte-compatible with the pinned Python renderers:
//!
//! * Markdown formats share one frontmatter + body shape
//!   (`_render_helpers.render_markdown_agent`);
//! * Codex is a three-key TOML document (`name`, `description`,
//!   `developer_instructions`) rendered with the same basic-string escaping
//!   `tomli_w` produces for this data;
//! * the manifest is `json.dumps(..., indent=2, sort_keys=True)` with a trailing
//!   newline and entries ordered by output path.
//!
//! Sources are explicit and verified: the catalog is discovered from an
//! explicit pack/offering root, symlinks and unreadable profiles are refused,
//! and a destination whose recorded hash shows an operator edit is preserved
//! and reported, never silently rewritten.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use serde_json::{Map, Value};
use serde_yaml_ng::Value as YamlValue;
use sha2::{Digest, Sha256};

use crate::error::InstallError;
use crate::plan::now_utc_iso;

/// Project-relative path of the profile ownership manifest.
pub const PROFILES_MANIFEST_REL: &str = ".kittify/agent_profiles_manifest.json";

/// Manifest schema version (upstream `SCHEMA_VERSION`).
pub const PROFILES_MANIFEST_SCHEMA_VERSION: u32 = 1;

/// Projection generation recorded per entry (upstream `PROJECTION_VERSION`).
pub const PROJECTION_VERSION: u32 = 1;

/// The built-in token prefix used for portable source provenance.
pub const PACKS_ROOT_TOKEN: &str = "${SPEC_KITTY_PACKS_ROOT}";

/// One admitted agent profile, reduced to the fields the renderers use.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentProfile {
    /// `profile-id` — also the native filename stem.
    pub profile_id: String,
    /// Display `name`.
    pub name: String,
    /// `description` (may be empty; renderers fall back to `purpose`).
    pub description: String,
    /// Mandatory `purpose` paragraph.
    pub purpose: String,
    /// `roles`, in authored order.
    pub roles: Vec<String>,
    /// `specialization.primary-focus`.
    pub primary_focus: String,
    /// `specialization.avoidance-boundary` (may be empty).
    pub avoidance_boundary: String,
    /// A workflow routing signal, not an agent identity: never projected.
    pub sentinel: bool,
    /// The absolute source file.
    pub source_path: PathBuf,
}

impl AgentProfile {
    /// The DRG-style URN recorded in the manifest.
    pub fn urn(&self) -> String {
        format!("agent_profile:{}", self.profile_id)
    }
}

/// A discovered set of agent profiles rooted at one directory.
#[derive(Clone, Debug)]
pub struct ProfileCatalog {
    root: PathBuf,
    /// The pack root that owns `root`, when `root` is `<pack_root>/agent_profiles`.
    pack_root: Option<PathBuf>,
    profiles: Vec<AgentProfile>,
}

impl ProfileCatalog {
    /// Discover `<root>/*.agent.yaml`, refusing symlinks and invalid ids.
    ///
    /// Returns `Ok(None)` when the directory is absent — the caller decides
    /// whether that is fatal, exactly like the skill catalog.
    pub fn discover(root: &Path) -> Result<Option<Self>, InstallError> {
        if !root.is_dir() {
            return Ok(None);
        }
        let mut sources: Vec<PathBuf> = Vec::new();
        for entry in std::fs::read_dir(root)
            .map_err(|error| InstallError::io("read directory", root, error))?
        {
            let entry =
                entry.map_err(|error| InstallError::io("read directory entry", root, error))?;
            let file_type = entry
                .file_type()
                .map_err(|error| InstallError::io("inspect", entry.path(), error))?;
            if file_type.is_symlink() {
                return Err(InstallError::TemplatePathUnsafe {
                    path: entry.path(),
                    detail: "agent profile tree contains a symlink".to_string(),
                });
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if file_type.is_file() && name.ends_with(".agent.yaml") {
                sources.push(entry.path());
            }
        }
        if sources.is_empty() {
            return Ok(None);
        }
        sources.sort();
        let mut profiles = Vec::with_capacity(sources.len());
        for source in sources {
            profiles.push(parse_profile(&source)?);
        }
        let pack_root = root
            .parent()
            .filter(|_| {
                root.file_name()
                    .is_some_and(|name| name == "agent_profiles")
            })
            .map(Path::to_path_buf);
        Ok(Some(Self {
            root: root.to_path_buf(),
            pack_root,
            profiles,
        }))
    }

    /// The catalog root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Every profile, sorted by profile id.
    pub fn profiles(&self) -> &[AgentProfile] {
        &self.profiles
    }

    /// Profiles that are projected (non-sentinel), sorted by profile id.
    pub fn projectable(&self) -> impl Iterator<Item = &AgentProfile> {
        self.profiles.iter().filter(|profile| !profile.sentinel)
    }

    /// The portable `source_path` recorded for `profile`.
    ///
    /// A path under `<pack_root>/agent_profiles` becomes
    /// `${SPEC_KITTY_PACKS_ROOT}/built-in/agent_profiles/<file>`; anything else
    /// stays an absolute, symlink-resolved path.
    pub fn source_token(&self, profile: &AgentProfile) -> Result<String, InstallError> {
        if let Some(pack_root) = &self.pack_root
            && let Ok(relative) = profile.source_path.strip_prefix(pack_root)
        {
            let relative = relative.to_string_lossy().replace('\\', "/");
            // `pack_root` is `<packs-root>/built-in`; the token prefix stands for
            // exactly that directory.
            return Ok(format!("{PACKS_ROOT_TOKEN}/built-in/{relative}"));
        }
        Ok(profile.source_path.to_string_lossy().replace('\\', "/"))
    }
}

fn parse_profile(path: &Path) -> Result<AgentProfile, InstallError> {
    let raw = sk_kernel::read_bounded(path, 1024 * 1024)
        .map_err(|error| InstallError::io("read", path, std::io::Error::other(error)))?;
    let text = std::str::from_utf8(&raw).map_err(|error| InstallError::UnsupportedNode {
        path: path.to_path_buf(),
        detail: error.to_string(),
    })?;
    let value: YamlValue =
        serde_yaml_ng::from_str(text).map_err(|error| InstallError::UnsupportedNode {
            path: path.to_path_buf(),
            detail: error.to_string(),
        })?;
    let mapping = value
        .as_mapping()
        .ok_or_else(|| InstallError::UnsupportedNode {
            path: path.to_path_buf(),
            detail: "profile root must be a mapping".to_string(),
        })?;
    let string = |key: &str| -> Result<String, InstallError> {
        match mapping.get(YamlValue::from(key)) {
            None | Some(YamlValue::Null) => Ok(String::new()),
            Some(YamlValue::String(value)) => Ok(value.clone()),
            Some(YamlValue::Number(value)) => Ok(value.to_string()),
            Some(YamlValue::Bool(value)) => Ok(value.to_string()),
            Some(_) => Err(InstallError::UnsupportedNode {
                path: path.to_path_buf(),
                detail: format!("{key} must be a scalar"),
            }),
        }
    };
    let profile_id = string("profile-id")?;
    if profile_id.is_empty() {
        return Err(InstallError::UnsupportedNode {
            path: path.to_path_buf(),
            detail: "profile-id is required".to_string(),
        });
    }
    if let Some(reason) = native_name_violation(&profile_id) {
        return Err(InstallError::UnsupportedNode {
            path: path.to_path_buf(),
            detail: format!("profile-id {profile_id:?} is not a safe filename stem: {reason}"),
        });
    }
    let name = string("name")?;
    if name.is_empty() {
        return Err(InstallError::UnsupportedNode {
            path: path.to_path_buf(),
            detail: "name is required".to_string(),
        });
    }
    let purpose = string("purpose")?;
    if purpose.trim().is_empty() {
        return Err(InstallError::UnsupportedNode {
            path: path.to_path_buf(),
            detail: "purpose is required".to_string(),
        });
    }
    let roles = match mapping.get(YamlValue::from("roles")) {
        Some(YamlValue::Sequence(sequence)) => sequence
            .iter()
            .map(|value| match value {
                YamlValue::String(value) => Ok(value.to_lowercase()),
                _ => Err(InstallError::UnsupportedNode {
                    path: path.to_path_buf(),
                    detail: "roles entries must be strings".to_string(),
                }),
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => {
            return Err(InstallError::UnsupportedNode {
                path: path.to_path_buf(),
                detail: "roles must be a list".to_string(),
            });
        }
        None => Vec::new(),
    };
    if roles.is_empty() {
        return Err(InstallError::UnsupportedNode {
            path: path.to_path_buf(),
            detail: "roles must declare at least one role".to_string(),
        });
    }
    let specialization = mapping
        .get(YamlValue::from("specialization"))
        .and_then(YamlValue::as_mapping)
        .ok_or_else(|| InstallError::UnsupportedNode {
            path: path.to_path_buf(),
            detail: "specialization must be a mapping".to_string(),
        })?;
    let nested = |key: &str| -> String {
        match specialization.get(YamlValue::from(key)) {
            Some(YamlValue::String(value)) => value.clone(),
            Some(YamlValue::Number(value)) => value.to_string(),
            Some(YamlValue::Bool(value)) => value.to_string(),
            _ => String::new(),
        }
    };
    let primary_focus = nested("primary-focus");
    if primary_focus.trim().is_empty() {
        return Err(InstallError::UnsupportedNode {
            path: path.to_path_buf(),
            detail: "specialization.primary-focus is required".to_string(),
        });
    }
    let sentinel = matches!(
        mapping.get(YamlValue::from("sentinel")),
        Some(YamlValue::Bool(true))
    );
    let resolved = sk_kernel::paths::resolve_path_non_strict(path).map_err(|error| {
        InstallError::UnsupportedNode {
            path: path.to_path_buf(),
            detail: format!("could not resolve source path: {error}"),
        }
    })?;
    Ok(AgentProfile {
        profile_id,
        name,
        description: string("description")?,
        purpose,
        roles,
        primary_focus,
        avoidance_boundary: nested("avoidance-boundary"),
        sentinel,
        source_path: resolved,
    })
}

/// The native formats this module can render.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum ProfileFormat {
    /// Claude Code project agents (`.claude/agents/<id>.md`).
    ClaudeAgent,
    /// Copilot / VS Code agents (`.github/agents/<id>.agent.md`).
    CopilotAgent,
    /// Codex CLI agents (`.codex/agents/<id>.toml`).
    CodexAgent,
    /// Augment Code agents (`.augment/agents/<id>.md`).
    AugmentAgent,
}

impl ProfileFormat {
    /// Stable manifest spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeAgent => "claude-agent",
            Self::CopilotAgent => "copilot-agent",
            Self::CodexAgent => "codex-agent",
            Self::AugmentAgent => "augment-agent",
        }
    }

    /// The renderer for a tool key, or `None` when the harness has no
    /// project-local native named-agent primitive.
    pub fn for_tool(tool_key: &str) -> Option<Self> {
        match tool_key {
            "claude" => Some(Self::ClaudeAgent),
            "copilot" | "vscode" => Some(Self::CopilotAgent),
            "codex" | "codex-cli" => Some(Self::CodexAgent),
            "auggie" | "augment" => Some(Self::AugmentAgent),
            // Amazon Q (`q`/`amazon-q`) is user-global and never written by init.
            _ => None,
        }
    }

    /// The project-relative output path for `profile_id`.
    pub fn output_path(self, profile_id: &str) -> String {
        match self {
            Self::ClaudeAgent => format!(".claude/agents/{profile_id}.md"),
            Self::CopilotAgent => format!(".github/agents/{profile_id}.agent.md"),
            Self::CodexAgent => format!(".codex/agents/{profile_id}.toml"),
            Self::AugmentAgent => format!(".augment/agents/{profile_id}.md"),
        }
    }
}

/// Return a reason when `profile_id` is illegal as a native filename stem.
pub fn native_name_violation(profile_id: &str) -> Option<String> {
    if profile_id.is_empty() {
        return Some("empty profile id has no native filename stem".to_string());
    }
    if profile_id == "." || profile_id == ".." {
        return Some(format!(
            "profile id {profile_id:?} is a path-traversal segment"
        ));
    }
    if !profile_id
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
    {
        return Some(format!(
            "profile id {profile_id:?} contains characters illegal in a native agent filename"
        ));
    }
    None
}

/// Render the Markdown frontmatter + body shared by the Markdown formats.
pub fn render_markdown_agent(profile: &AgentProfile) -> String {
    let description = if profile.description.is_empty() {
        &profile.purpose
    } else {
        &profile.description
    };
    let avoidance = if profile.avoidance_boundary.is_empty() {
        "(none declared)".to_string()
    } else {
        profile.avoidance_boundary.clone()
    };
    format!(
        "---\nname: {id}\ndescription: {description}\nroles: [{roles}]\n---\n\n# {name}\n\n{purpose}\n\n## Specialization\n\n- Primary focus: {focus}\n- Avoidance boundary: {avoidance}\n\n_Projected from Spec Kitty agent profile `{id}`; do not edit by hand._\n",
        name = profile.name,
        description = yaml_scalar(description),
        roles = profile.roles.join(", "),
        purpose = profile.purpose,
        focus = profile.primary_focus,
        avoidance = avoidance,
        id = profile.profile_id,
    )
}

/// Render the Codex TOML agent document.
pub fn render_codex_agent(profile: &AgentProfile) -> String {
    let description = if profile.description.is_empty() {
        &profile.purpose
    } else {
        &profile.description
    };
    format!(
        "name = {}\ndescription = {}\ndeveloper_instructions = {}\n",
        toml_string(&profile.name),
        toml_string(description),
        toml_string(&profile.purpose),
    )
}

/// Render the file body for `format` and `profile`.
pub fn render_profile(profile: &AgentProfile, format: ProfileFormat) -> String {
    match format {
        ProfileFormat::CodexAgent => render_codex_agent(profile),
        ProfileFormat::ClaudeAgent | ProfileFormat::CopilotAgent | ProfileFormat::AugmentAgent => {
            render_markdown_agent(profile)
        }
    }
}

fn yaml_scalar(value: &str) -> String {
    // Upstream always double-quotes with internal quotes/backslashes escaped and
    // newlines collapsed to a single space (a folded scalar keeps its trailing
    // newline, so this preserves the trailing space the oracle has).
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace(['\n', '\r'], " ");
    format!("\"{escaped}\"")
}

fn toml_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            character if (character as u32) < 0x20 || character as u32 == 0x7f => {
                out.push_str(&format!("\\u{:04X}", character as u32));
            }
            character => out.push(character),
        }
    }
    out.push('"');
    out
}

/// One manifest entry (upstream 9-field shape).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileManifestEntry {
    /// The profile URN (`agent_profile:<id>`).
    pub profile_urn: String,
    /// `built-in`, `org` or `project`.
    pub source_layer: String,
    /// The selected tool key.
    pub tool_key: String,
    /// Project-relative POSIX output path.
    pub output_path: String,
    /// The native format identifier.
    pub format: String,
    /// SHA-256 hex of the rendered bytes.
    pub file_hash: String,
    /// Portable source provenance.
    pub source_path: String,
    /// SHA-256 hex of the source file bytes.
    pub source_hash: String,
    /// Projection generation.
    pub projection_version: u32,
}

/// The profile ownership manifest.
#[derive(Clone, Debug, Default)]
pub struct ProfileManifest {
    entries: BTreeMap<String, ProfileManifestEntry>,
}

impl ProfileManifest {
    /// Load the manifest; absent is an empty manifest, malformed is a refusal.
    pub fn load(project_root: &Path) -> Result<Self, InstallError> {
        let path = managed_destination(project_root, PROFILES_MANIFEST_REL)?;
        let raw = match std::fs::read(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(source) => return Err(InstallError::io("read", &path, source)),
        };
        let value: Value =
            serde_json::from_slice(&raw).map_err(|error| InstallError::UnsupportedNode {
                path: path.clone(),
                detail: error.to_string(),
            })?;
        let object = value
            .as_object()
            .ok_or_else(|| InstallError::UnsupportedNode {
                path: path.clone(),
                detail: "manifest root must be an object".to_string(),
            })?;
        let schema = object
            .get("schema_version")
            .and_then(Value::as_u64)
            .ok_or_else(|| InstallError::UnsupportedNode {
                path: path.clone(),
                detail: "schema_version must be an integer".to_string(),
            })?;
        if schema != u64::from(PROFILES_MANIFEST_SCHEMA_VERSION) {
            return Err(InstallError::UnsupportedNode {
                path,
                detail: format!("unsupported schema_version {schema}"),
            });
        }
        let entries = object
            .get("entries")
            .and_then(Value::as_array)
            .ok_or_else(|| InstallError::UnsupportedNode {
                path: path.clone(),
                detail: "entries must be an array".to_string(),
            })?;
        let mut manifest = Self::default();
        for entry in entries {
            let map = entry
                .as_object()
                .ok_or_else(|| InstallError::UnsupportedNode {
                    path: path.clone(),
                    detail: "entry must be an object".to_string(),
                })?;
            let text = |key: &str| -> Result<String, InstallError> {
                map.get(key)
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .ok_or_else(|| InstallError::UnsupportedNode {
                        path: path.clone(),
                        detail: format!("entry field {key:?} must be a string"),
                    })
            };
            let output_path = text("output_path")?;
            managed_destination(project_root, &output_path)?;
            let entry = ProfileManifestEntry {
                profile_urn: text("profile_urn")?,
                source_layer: text("source_layer")?,
                tool_key: text("tool_key")?,
                output_path: output_path.clone(),
                format: text("format")?,
                file_hash: text("file_hash")?,
                source_path: text("source_path")?,
                source_hash: text("source_hash")?,
                projection_version: map
                    .get("projection_version")
                    .and_then(Value::as_u64)
                    .map(|value| value as u32)
                    .unwrap_or(PROJECTION_VERSION),
            };
            if manifest.entries.insert(output_path, entry).is_some() {
                return Err(InstallError::UnsupportedNode {
                    path,
                    detail: "duplicate manifest output path".to_string(),
                });
            }
        }
        Ok(manifest)
    }

    /// Every entry, ordered by output path.
    pub fn entries(&self) -> impl Iterator<Item = &ProfileManifestEntry> {
        self.entries.values()
    }

    /// The recorded hash for `output_path`.
    pub fn hash_for(&self, output_path: &str) -> Option<&str> {
        self.entries
            .get(output_path)
            .map(|entry| entry.file_hash.as_str())
    }

    /// Insert or replace an entry.
    pub fn record(&mut self, entry: ProfileManifestEntry) {
        self.entries.insert(entry.output_path.clone(), entry);
    }

    /// Remove an entry by output path.
    pub fn remove(&mut self, output_path: &str) {
        self.entries.remove(output_path);
    }

    /// Serialize in the upstream wire format (sorted keys, two-space indent).
    pub fn to_bytes(&self) -> Result<Vec<u8>, InstallError> {
        let mut entries = Vec::with_capacity(self.entries.len());
        for entry in self.entries.values() {
            let mut object = Map::new();
            object.insert(
                "profile_urn".to_string(),
                Value::String(entry.profile_urn.clone()),
            );
            object.insert(
                "source_layer".to_string(),
                Value::String(entry.source_layer.clone()),
            );
            object.insert(
                "tool_key".to_string(),
                Value::String(entry.tool_key.clone()),
            );
            object.insert(
                "output_path".to_string(),
                Value::String(entry.output_path.clone()),
            );
            object.insert("format".to_string(), Value::String(entry.format.clone()));
            object.insert(
                "file_hash".to_string(),
                Value::String(entry.file_hash.clone()),
            );
            object.insert(
                "source_path".to_string(),
                Value::String(entry.source_path.clone()),
            );
            object.insert(
                "source_hash".to_string(),
                Value::String(entry.source_hash.clone()),
            );
            object.insert(
                "projection_version".to_string(),
                Value::Number(u64::from(entry.projection_version).into()),
            );
            entries.push(Value::Object(object));
        }
        let mut root = Map::new();
        root.insert("entries".to_string(), Value::Array(entries));
        root.insert(
            "schema_version".to_string(),
            Value::Number(u64::from(PROFILES_MANIFEST_SCHEMA_VERSION).into()),
        );
        let mut bytes = serde_json::to_vec_pretty(&Value::Object(root)).map_err(|error| {
            InstallError::UnsupportedNode {
                path: PathBuf::from("<manifest>"),
                detail: error.to_string(),
            }
        })?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    /// Save the manifest; returns whether the file changed.
    pub fn save(&self, project_root: &Path) -> Result<bool, InstallError> {
        let path = managed_destination(project_root, PROFILES_MANIFEST_REL)?;
        let bytes = self.to_bytes()?;
        match std::fs::read(&path) {
            Ok(existing) if existing == bytes => return Ok(false),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => return Err(InstallError::io("read", &path, source)),
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|source| InstallError::io("create directory", parent, source))?;
        }
        sk_kernel::atomic_write(&path, &bytes).map_err(|error| {
            InstallError::io(
                "write profile manifest",
                &path,
                std::io::Error::other(error),
            )
        })?;
        Ok(true)
    }
}

/// What one projection pass did.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProfileProjectionReport {
    /// Files written (created or refreshed from an owned prior render).
    pub written: Vec<String>,
    /// Files already holding the canonical render.
    pub kept: Vec<String>,
    /// Files preserved because they are unowned or operator-edited.
    pub preserved: Vec<String>,
    /// Manifest-owned files pruned because the profile no longer projects.
    pub pruned: Vec<String>,
    /// Non-fatal warnings for the CLI to render (skipped optional surfaces).
    pub warnings: Vec<String>,
    /// The manifest entries produced.
    pub entries: Vec<ProfileManifestEntry>,
}

/// Project `catalog` for `agent_keys` into the project and record ownership.
///
/// `source_layer` is the provenance layer (`built-in` for the shipped pack).
/// `only_tools` limits projection to the selected agents that have a
/// project-local renderer; an agent without one is skipped (upstream reports a
/// research gap rather than writing an invented file).
pub fn project_agent_profiles(
    project_root: &Path,
    agent_keys: &[String],
    catalog: &ProfileCatalog,
    source_layer: &str,
) -> Result<ProfileProjectionReport, InstallError> {
    let manifest_existed = project_root.join(PROFILES_MANIFEST_REL).is_file();
    let mut manifest = ProfileManifest::load(project_root)?;
    let mut report = ProfileProjectionReport::default();
    let mut desired_outputs: BTreeSet<String> = BTreeSet::new();
    let mut tools: Vec<&str> = Vec::new();
    for key in agent_keys {
        if ProfileFormat::for_tool(key).is_some() && !tools.contains(&key.as_str()) {
            tools.push(key);
        }
    }
    for tool in &tools {
        let format = ProfileFormat::for_tool(tool).expect("checked");
        for profile in catalog.projectable() {
            let output_path = format.output_path(&profile.profile_id);
            // A desired output is never stale, even when an operator edit makes
            // us preserve it instead of writing.
            desired_outputs.insert(output_path.clone());
            // A refused optional destination is preserved and reported, never
            // fatal: the pinned init skips a harness whose root is a symlink and
            // completes with a warning, instead of aborting a supported layout.
            let destination = match managed_destination(project_root, &output_path) {
                Ok(destination) => destination,
                Err(error) => {
                    report.warnings.push(format!(
                        "Skipped agent profile {output_path} for {tool}: {error}"
                    ));
                    report.preserved.push(output_path.clone());
                    continue;
                }
            };
            let rendered = render_profile(profile, format);
            let file_hash = hex(&Sha256::digest(rendered.as_bytes()));
            let source_bytes = std::fs::read(&profile.source_path)
                .map_err(|error| InstallError::io("read", &profile.source_path, error))?;
            let source_hash = hex(&Sha256::digest(&source_bytes));
            let recorded = manifest.hash_for(&output_path).map(str::to_string);
            match std::fs::symlink_metadata(&destination) {
                Ok(metadata) if metadata.file_type().is_file() => {
                    let existing = std::fs::read(&destination)
                        .map_err(|error| InstallError::io("read", &destination, error))?;
                    let existing_hash = hex(&Sha256::digest(&existing));
                    if existing_hash == file_hash {
                        report.kept.push(output_path.clone());
                    } else if recorded.as_deref() == Some(existing_hash.as_str()) {
                        write_file(&destination, rendered.as_bytes())?;
                        report.written.push(output_path.clone());
                    } else {
                        // Unowned or operator-edited: preserve and do not claim it.
                        report.preserved.push(output_path.clone());
                        continue;
                    }
                }
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    report.warnings.push(format!(
                        "Skipped agent profile {output_path} for {tool}: destination is a symlink"
                    ));
                    report.preserved.push(output_path);
                    continue;
                }
                Ok(_) => {
                    report.warnings.push(format!(
                        "Skipped agent profile {output_path} for {tool}: destination is not a regular file"
                    ));
                    report.preserved.push(output_path);
                    continue;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    write_file(&destination, rendered.as_bytes())?;
                    report.written.push(output_path.clone());
                }
                Err(source) => return Err(InstallError::io("inspect", &destination, source)),
            }
            let entry = ProfileManifestEntry {
                profile_urn: profile.urn(),
                source_layer: source_layer.to_string(),
                tool_key: (*tool).to_string(),
                output_path: output_path.clone(),
                format: format.as_str().to_string(),
                file_hash,
                source_path: catalog.source_token(profile)?,
                source_hash,
                projection_version: PROJECTION_VERSION,
            };
            manifest.record(entry.clone());
            report.entries.push(entry);
        }
    }
    // Prune files this manifest owns for the tools we just projected when the
    // profile is gone from the source. An operator-edited file is preserved.
    let stale: Vec<(String, String)> = manifest
        .entries()
        .filter(|entry| {
            tools.iter().any(|tool| *tool == entry.tool_key)
                && !desired_outputs.contains(&entry.output_path)
        })
        .map(|entry| (entry.output_path.clone(), entry.file_hash.clone()))
        .collect();
    for (output_path, recorded_hash) in stale {
        let destination = managed_destination(project_root, &output_path)?;
        match std::fs::read(&destination) {
            Ok(existing) if hex(&Sha256::digest(&existing)) == recorded_hash => {
                std::fs::remove_file(&destination).map_err(|source| {
                    InstallError::io("remove stale profile", &destination, source)
                })?;
                report.pruned.push(output_path.clone());
                manifest.remove(&output_path);
            }
            Ok(_) => {
                report.preserved.push(output_path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                manifest.remove(&output_path);
                report.pruned.push(output_path);
            }
            Err(source) => return Err(InstallError::io("read", &destination, source)),
        }
    }
    // Only create a new manifest when something was actually claimed. The
    // pinned oracle writes no `.kittify/agent_profiles_manifest.json` for an
    // agent set with no project-local profile renderer (11 of the 17 keys), or
    // when every optional destination was refused; an existing manifest is
    // still refreshed/pruned normally.
    if manifest_existed || !report.entries.is_empty() {
        manifest.save(project_root)?;
    }
    Ok(report)
}

/// The current UTC timestamp in the shape the profile manifest helpers use.
pub fn timestamp() -> String {
    now_utc_iso()
}

fn write_file(destination: &Path, bytes: &[u8]) -> Result<(), InstallError> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|source| InstallError::io("create directory", parent, source))?;
    }
    sk_kernel::atomic_write(destination, bytes).map_err(|error| {
        InstallError::io("write profile", destination, std::io::Error::other(error))
    })?;
    Ok(())
}

fn managed_destination(project_root: &Path, relative: &str) -> Result<PathBuf, InstallError> {
    let invalid = |detail: String| InstallError::UnsupportedNode {
        path: project_root.join(PROFILES_MANIFEST_REL),
        detail,
    };
    let path = Path::new(relative);
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::RootDir))
    {
        return Err(invalid(format!("unsafe profile destination {relative:?}")));
    }
    sk_kernel::resolve_within(project_root, path).map_err(|error| invalid(error.to_string()))
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn catalog(root: &Path) -> ProfileCatalog {
        write(
            &root.join("agent_profiles/demo.agent.yaml"),
            "profile-id: demo\nname: Demo Agent\ndescription: >\n  A demo agent.\nroles:\n- implementer\n- reviewer\npurpose: >\n  Do demo work.\nspecialization:\n  primary-focus: >\n    Demo focus.\n  avoidance-boundary: >\n    Nothing.\n",
        );
        write(
            &root.join("agent_profiles/sentinel.agent.yaml"),
            "profile-id: sentinel\nname: Sentinel\nroles:\n- manager\npurpose: route\nsentinel: true\nspecialization:\n  primary-focus: routing\n",
        );
        ProfileCatalog::discover(&root.join("agent_profiles"))
            .expect("catalog")
            .expect("present")
    }

    #[test]
    fn markdown_and_codex_renders_match_the_pinned_shape() {
        let directory = tempfile::tempdir().unwrap();
        let catalog = catalog(directory.path());
        let profile = catalog.projectable().next().unwrap();
        let markdown = render_profile(profile, ProfileFormat::ClaudeAgent);
        assert!(markdown.starts_with("---\nname: demo\ndescription: \"A demo agent. \"\nroles: [implementer, reviewer]\n---\n\n# Demo Agent\n\n"));
        assert!(
            markdown.ends_with(
                "_Projected from Spec Kitty agent profile `demo`; do not edit by hand._\n"
            )
        );
        assert!(!markdown.contains("routing"));
        let toml = render_profile(profile, ProfileFormat::CodexAgent);
        assert_eq!(
            toml,
            "name = \"Demo Agent\"\ndescription = \"A demo agent.\\n\"\ndeveloper_instructions = \"Do demo work.\\n\"\n"
        );
    }

    #[test]
    fn projection_writes_the_manifest_and_preserves_operator_edits() {
        let project = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let catalog = catalog(source.path());
        let report = project_agent_profiles(
            project.path(),
            &["claude".to_string(), "codex".to_string()],
            &catalog,
            "built-in",
        )
        .expect("project");
        assert_eq!(report.written.len(), 2);
        assert!(project.path().join(".claude/agents/demo.md").is_file());
        assert!(project.path().join(".codex/agents/demo.toml").is_file());
        let manifest = ProfileManifest::load(project.path()).expect("manifest");
        let entries: Vec<&ProfileManifestEntry> = manifest.entries().collect();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].output_path, ".claude/agents/demo.md");
        assert_eq!(entries[0].source_layer, "built-in");
        assert_eq!(entries[0].projection_version, 1);
        assert!(
            entries[0]
                .source_path
                .ends_with("/agent_profiles/demo.agent.yaml")
        );

        // A repeat is a no-op.
        let report = project_agent_profiles(
            project.path(),
            &["claude".to_string(), "codex".to_string()],
            &catalog,
            "built-in",
        )
        .expect("project");
        assert_eq!(report.written.len(), 0);
        assert_eq!(report.kept.len(), 2);

        // An operator edit is never overwritten and not claimed as kept.
        let path = project.path().join(".claude/agents/demo.md");
        std::fs::write(&path, "operator edit\n").unwrap();
        let report = project_agent_profiles(
            project.path(),
            &["claude".to_string()],
            &catalog,
            "built-in",
        )
        .expect("project");
        assert_eq!(report.preserved, vec![".claude/agents/demo.md".to_string()]);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "operator edit\n");
    }

    #[test]
    fn unsafe_manifest_paths_are_refused() {
        let project = tempfile::tempdir().unwrap();
        let path = project.path().join(PROFILES_MANIFEST_REL);
        write(
            &path,
            "{\n  \"entries\": [\n    {\n      \"format\": \"claude-agent\",\n      \"file_hash\": \"x\",\n      \"output_path\": \"../escape.md\",\n      \"profile_urn\": \"agent_profile:x\",\n      \"projection_version\": 1,\n      \"source_hash\": \"x\",\n      \"source_layer\": \"built-in\",\n      \"source_path\": \"x\",\n      \"tool_key\": \"claude\"\n    }\n  ],\n  \"schema_version\": 1\n}\n",
        );
        let error = ProfileManifest::load(project.path()).expect_err("refused");
        assert!(error.to_string().contains("unsafe profile destination"));
    }
}
