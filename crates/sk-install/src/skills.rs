//! Charter **skill** installation: the managed-skills manifest, native-root
//! restore, and tool-surface repair.
//!
//! This is the second ownership authority upstream keeps beside the
//! command-skills manifest. `.kittify/skills-manifest.json` records every file
//! of every installed skill, per agent, with a `sha256:<hex>` content hash and
//! provenance. Its wire format is reproduced exactly:
//!
//! - top-level `version` (1), `created_at`, `updated_at`,
//!   `spec_kitty_version`, `entries`;
//! - two-space JSON indentation, a trailing newline, entry field order as
//!   declared upstream;
//! - provenance defaults (`origin: builtin`, empty `source_ref`/`source_hash`)
//!   omitted, so a pre-provenance manifest round-trips byte-stably;
//! - entries deduplicated by `(installed_path, agent_key)`, duplicate owners
//!   rejected on load.
//!
//! Installation is fail-closed: an existing file this writer does not own (or a
//! manifest-owned file whose bytes the operator changed) is never overwritten.
//! A missing managed file is restorable; a *drifted* one is reported and only
//! replaced when the caller explicitly asks for drift repair.
//!
//! The catalog source is explicit and verified by the caller. This crate does
//! not bundle the skill tree; it refuses an absent catalog rather than inventing
//! skills.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::agent::{AgentSurface, SkillClass, agent_surface};
use crate::error::InstallError;
use crate::plan::now_utc_iso;

/// Project-relative path of the managed-skills manifest.
pub const SKILLS_MANIFEST_REL: &str = ".kittify/skills-manifest.json";

/// The supported manifest schema version.
pub const SKILLS_MANIFEST_VERSION: u32 = 1;

/// Built-in (shipped) skill provenance.
pub const ORIGIN_BUILTIN: &str = "builtin";
/// Charter-pack skill provenance.
pub const ORIGIN_PACK: &str = "pack";

/// One source file of a skill.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillFile {
    /// Path relative to the skill directory, POSIX (`SKILL.md`,
    /// `references/x.md`, …).
    pub relative: String,
    /// Absolute source path.
    pub source: PathBuf,
    /// SHA-256 hex of the bytes.
    pub content_hash: String,
}

/// One discovered skill.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalSkill {
    /// The directory name (the skill name).
    pub name: String,
    /// Every file, `SKILL.md` first then sorted references/scripts/assets.
    pub files: Vec<SkillFile>,
}

/// A discovered skills catalog rooted at an explicit directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillCatalog {
    root: PathBuf,
    skills: Vec<CanonicalSkill>,
}

impl SkillCatalog {
    /// Discover `<root>/<skill>/SKILL.md` trees, refusing symlinks/special files.
    pub fn discover(root: &Path) -> Result<Self, InstallError> {
        if !root.is_dir() {
            return Err(InstallError::SkillsCatalogMissing {
                path: root.to_path_buf(),
                detail: "skills root is absent".to_string(),
            });
        }
        let mut names: Vec<String> = Vec::new();
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
                    detail: "skills catalog contains a symlink".to_string(),
                });
            }
            if !file_type.is_dir() {
                continue;
            }
            if entry.path().join("SKILL.md").is_file() {
                names.push(entry.file_name().to_string_lossy().to_string());
            }
        }
        names.sort();
        if names.is_empty() {
            return Err(InstallError::SkillsCatalogMissing {
                path: root.to_path_buf(),
                detail: "no skill directory contains SKILL.md".to_string(),
            });
        }
        let mut skills = Vec::new();
        for name in names {
            let directory = root.join(&name);
            let mut files = Vec::new();
            // Upstream installs SKILL.md plus the three canonical subdirectories;
            // other root files (README.md, notes) are not part of the skill.
            let skill_md = directory.join("SKILL.md");
            let bytes = std::fs::read(&skill_md)
                .map_err(|error| InstallError::io("read", &skill_md, error))?;
            files.push(SkillFile {
                relative: "SKILL.md".to_string(),
                source: skill_md,
                content_hash: format!("sha256:{:x}", Sha256::digest(&bytes)),
            });
            for subdirectory in ["references", "scripts", "assets"] {
                let path = directory.join(subdirectory);
                if path.is_dir() {
                    collect_skill_files(&directory, &path, &mut files)?;
                }
            }
            files.sort_by(|left, right| {
                let rank = |path: &str| {
                    if path == "SKILL.md" {
                        0
                    } else if path.starts_with("references/") {
                        1
                    } else if path.starts_with("scripts/") {
                        2
                    } else {
                        3
                    }
                };
                rank(&left.relative)
                    .cmp(&rank(&right.relative))
                    .then_with(|| left.relative.cmp(&right.relative))
            });
            skills.push(CanonicalSkill { name, files });
        }
        Ok(Self {
            root: root.to_path_buf(),
            skills,
        })
    }

    /// The catalog root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Every skill, sorted by name.
    pub fn skills(&self) -> &[CanonicalSkill] {
        &self.skills
    }

    /// The skill names, sorted.
    pub fn names(&self) -> Vec<String> {
        self.skills.iter().map(|skill| skill.name.clone()).collect()
    }

    /// One skill by name.
    pub fn skill(&self, name: &str) -> Option<&CanonicalSkill> {
        self.skills.iter().find(|skill| skill.name == name)
    }
}

fn collect_skill_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<SkillFile>,
) -> Result<(), InstallError> {
    for entry in std::fs::read_dir(directory)
        .map_err(|error| InstallError::io("read directory", directory, error))?
    {
        let entry =
            entry.map_err(|error| InstallError::io("read directory entry", directory, error))?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| InstallError::io("inspect", &path, error))?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            return Err(InstallError::TemplatePathUnsafe {
                path,
                detail: "skill tree contains a symlink".to_string(),
            });
        }
        if file_type.is_dir() {
            collect_skill_files(root, &path, files)?;
            continue;
        }
        if !file_type.is_file() {
            return Err(InstallError::TemplatePathUnsafe {
                path,
                detail: "skill tree contains a special file".to_string(),
            });
        }
        if entry.file_name().to_string_lossy().starts_with('.') {
            // Dotfiles (`.gitkeep`) are placeholders, not skill content.
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| InstallError::TemplatePathUnsafe {
                path: path.clone(),
                detail: "skill file escaped its directory".to_string(),
            })?
            .to_string_lossy()
            .replace('\\', "/");
        let bytes = std::fs::read(&path).map_err(|error| InstallError::io("read", &path, error))?;
        files.push(SkillFile {
            relative,
            source: path,
            content_hash: format!("sha256:{:x}", Sha256::digest(&bytes)),
        });
    }
    Ok(())
}

/// One managed skill file, in the upstream wire shape.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ManagedFileEntry {
    /// The skill name.
    pub skill_name: String,
    /// The source file relative to the skill directory.
    pub source_file: String,
    /// The installed path, relative to the project root (POSIX).
    pub installed_path: String,
    /// `shared-root-capable`, `native-root-required` or `wrapper-only`.
    pub installation_class: String,
    /// The agent the file was installed for.
    pub agent_key: String,
    /// `sha256:<hex>`.
    pub content_hash: String,
    /// ISO-8601 UTC timestamp.
    pub installed_at: String,
    /// `copy` or `symlink`.
    #[serde(default = "default_delivery_mode")]
    pub delivery_mode: String,
    /// `builtin` or `pack`.
    #[serde(default = "default_origin", skip_serializing_if = "is_builtin_origin")]
    pub origin: String,
    /// Pack source reference (pack origin only).
    #[serde(default, skip_serializing_if = "is_empty_string")]
    pub source_ref: String,
    /// Pack skill input hash (pack origin only).
    #[serde(default, skip_serializing_if = "is_empty_string")]
    pub source_hash: String,
}

fn default_delivery_mode() -> String {
    "copy".to_string()
}

fn default_origin() -> String {
    ORIGIN_BUILTIN.to_string()
}

fn is_builtin_origin(value: &str) -> bool {
    value == ORIGIN_BUILTIN
}

fn is_empty_string(value: &str) -> bool {
    value.is_empty()
}

impl ManagedFileEntry {
    /// Build an entry for a copied file.
    pub fn copied(
        skill_name: &str,
        source_file: &str,
        installed_path: &str,
        installation_class: &str,
        agent_key: &str,
        content_hash: &str,
        installed_at: &str,
    ) -> Self {
        Self {
            skill_name: skill_name.to_string(),
            source_file: source_file.to_string(),
            installed_path: installed_path.to_string(),
            installation_class: installation_class.to_string(),
            agent_key: agent_key.to_string(),
            content_hash: content_hash.to_string(),
            installed_at: installed_at.to_string(),
            delivery_mode: "copy".to_string(),
            origin: ORIGIN_BUILTIN.to_string(),
            source_ref: String::new(),
            source_hash: String::new(),
        }
    }
}

/// The managed-skills manifest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ManagedSkillManifest {
    /// Schema version (1).
    pub version: u32,
    /// First-write timestamp.
    pub created_at: String,
    /// Last-write timestamp.
    pub updated_at: String,
    /// The CLI version that wrote it.
    pub spec_kitty_version: String,
    /// Every managed file.
    pub entries: Vec<ManagedFileEntry>,
}

impl ManagedSkillManifest {
    /// An empty manifest stamped with `now`.
    pub fn new(version: &str, now: &str) -> Self {
        Self {
            version: SKILLS_MANIFEST_VERSION,
            created_at: now.to_string(),
            updated_at: now.to_string(),
            spec_kitty_version: version.to_string(),
            entries: Vec::new(),
        }
    }

    /// Add/replace an entry keyed by `(installed_path, agent_key)`.
    pub fn add_entry(&mut self, entry: ManagedFileEntry) {
        self.entries.retain(|existing| {
            !(existing.installed_path == entry.installed_path
                && existing.agent_key == entry.agent_key)
        });
        self.entries.push(entry);
    }

    /// Find an entry by installed path.
    pub fn find(&self, installed_path: &str) -> Option<&ManagedFileEntry> {
        self.entries
            .iter()
            .find(|entry| entry.installed_path == installed_path)
    }

    /// Load the manifest; absent or unparseable returns `None` (upstream tolerant).
    pub fn load(project_root: &Path) -> Option<Self> {
        Self::load_strict(project_root).ok().flatten()
    }

    /// Load the manifest strictly: absent is `Ok(None)`, malformed is an error.
    pub fn load_strict(project_root: &Path) -> Result<Option<Self>, InstallError> {
        let path = project_root.join(SKILLS_MANIFEST_REL);
        let raw = match std::fs::read(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(InstallError::io("read", &path, source)),
        };
        let value: serde_json::Value =
            serde_json::from_slice(&raw).map_err(|error| InstallError::SkillsManifestInvalid {
                path: path.clone(),
                detail: error.to_string(),
            })?;
        let object = value
            .as_object()
            .ok_or_else(|| InstallError::SkillsManifestInvalid {
                path: path.clone(),
                detail: "manifest root must be an object".to_string(),
            })?;
        const FIELDS: [&str; 5] = [
            "version",
            "created_at",
            "updated_at",
            "spec_kitty_version",
            "entries",
        ];
        for key in object.keys() {
            if !FIELDS.contains(&key.as_str()) {
                return Err(InstallError::SkillsManifestInvalid {
                    path: path.clone(),
                    detail: format!("unknown manifest field {key:?}"),
                });
            }
        }
        let version = object
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(1);
        if version != u64::from(SKILLS_MANIFEST_VERSION) {
            return Err(InstallError::SkillsManifestInvalid {
                path: path.clone(),
                detail: format!("unsupported manifest version {version}"),
            });
        }
        let mut entries: Vec<ManagedFileEntry> = Vec::new();
        if let Some(records) = object.get("entries") {
            let records =
                records
                    .as_array()
                    .ok_or_else(|| InstallError::SkillsManifestInvalid {
                        path: path.clone(),
                        detail: "entries must be an array".to_string(),
                    })?;
            for record in records {
                let map =
                    record
                        .as_object()
                        .ok_or_else(|| InstallError::SkillsManifestInvalid {
                            path: path.clone(),
                            detail: "entry must be an object".to_string(),
                        })?;
                for (key, value) in map {
                    if !value.is_string() {
                        return Err(InstallError::SkillsManifestInvalid {
                            path: path.clone(),
                            detail: format!("entry field {key:?} must be a string"),
                        });
                    }
                }
                let entry: ManagedFileEntry =
                    serde_json::from_value(record.clone()).map_err(|error| {
                        InstallError::SkillsManifestInvalid {
                            path: path.clone(),
                            detail: error.to_string(),
                        }
                    })?;
                if !matches!(entry.delivery_mode.as_str(), "copy" | "symlink") {
                    return Err(InstallError::SkillsManifestInvalid {
                        path: path.clone(),
                        detail: format!("invalid delivery_mode {:?}", entry.delivery_mode),
                    });
                }
                entries.push(entry);
            }
        }
        let owners: BTreeSet<(String, String)> = entries
            .iter()
            .map(|entry| (entry.installed_path.clone(), entry.agent_key.clone()))
            .collect();
        if owners.len() != entries.len() {
            return Err(InstallError::SkillsManifestInvalid {
                path,
                detail: "duplicate skills manifest owner".to_string(),
            });
        }
        Ok(Some(Self {
            version: SKILLS_MANIFEST_VERSION,
            created_at: object
                .get("created_at")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            updated_at: object
                .get("updated_at")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            spec_kitty_version: object
                .get("spec_kitty_version")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            entries,
        }))
    }

    /// Serialize in the upstream wire format (2-space indent, provenance defaults omitted).
    pub fn to_bytes(&self) -> Result<Vec<u8>, InstallError> {
        let mut bytes = serde_json::to_vec_pretty(self).map_err(|error| {
            InstallError::SkillsManifestInvalid {
                path: PathBuf::from("<manifest>"),
                detail: error.to_string(),
            }
        })?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    /// Save the manifest, retaining previous timestamps where identity is unchanged.
    ///
    /// Returns `true` when the file changed.
    pub fn save(&self, project_root: &Path) -> Result<bool, InstallError> {
        let path = project_root.join(SKILLS_MANIFEST_REL);
        let previous = Self::load_strict(project_root)?;
        let mut desired = self.clone();
        if let Some(previous) = &previous {
            desired.created_at = previous.created_at.clone();
            let prior: BTreeMap<(String, String), &ManagedFileEntry> = previous
                .entries
                .iter()
                .map(|entry| {
                    (
                        (entry.installed_path.clone(), entry.agent_key.clone()),
                        entry,
                    )
                })
                .collect();
            let order: BTreeMap<(String, String), usize> = previous
                .entries
                .iter()
                .enumerate()
                .map(|(index, entry)| {
                    (
                        (entry.installed_path.clone(), entry.agent_key.clone()),
                        index,
                    )
                })
                .collect();
            for entry in &mut desired.entries {
                let key = (entry.installed_path.clone(), entry.agent_key.clone());
                if let Some(existing) = prior.get(&key)
                    && existing.skill_name == entry.skill_name
                    && existing.source_file == entry.source_file
                    && existing.installation_class == entry.installation_class
                {
                    entry.installed_at = existing.installed_at.clone();
                }
            }
            desired.entries.sort_by(|left, right| {
                let left_key = (left.installed_path.clone(), left.agent_key.clone());
                let right_key = (right.installed_path.clone(), right.agent_key.clone());
                order
                    .get(&left_key)
                    .copied()
                    .unwrap_or(usize::MAX)
                    .cmp(&order.get(&right_key).copied().unwrap_or(usize::MAX))
                    .then_with(|| left.installed_path.cmp(&right.installed_path))
                    .then_with(|| left.agent_key.cmp(&right.agent_key))
            });
            let comparable = |manifest: &Self| {
                let mut copy = manifest.clone();
                copy.updated_at = String::new();
                copy
            };
            if comparable(&desired) == comparable(previous) {
                return Ok(false);
            }
        } else {
            desired.entries.sort_by(|left, right| {
                left.installed_path
                    .cmp(&right.installed_path)
                    .then_with(|| left.agent_key.cmp(&right.agent_key))
            });
        }
        let bytes = desired.to_bytes()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|source| InstallError::io("create directory", parent, source))?;
        }
        sk_kernel::atomic_write(&path, &bytes).map_err(|error| {
            InstallError::io("write skills manifest", &path, std::io::Error::other(error))
        })?;
        Ok(true)
    }
}

/// The Agent Skills specification's allowed top-level frontmatter fields.
pub const ALLOWED_SKILL_FIELDS: [&str; 6] = [
    "name",
    "description",
    "license",
    "compatibility",
    "metadata",
    "allowed-tools",
];

/// Project one `SKILL.md`'s bytes for a shared root.
///
/// A shared root (` .agents/skills/`) accepts only the Agent Skills
/// specification fields; every other top-level key is moved under `metadata` as
/// a string, which is exactly what upstream's `project_for_root` does for the
/// shared root (and why the same skill has a different hash there than in a
/// native root). Native roots keep the source bytes unchanged.
pub fn project_skill_bytes(
    source: &[u8],
    shared: bool,
    _skill_name: &str,
) -> Result<Vec<u8>, InstallError> {
    if !shared {
        return Ok(source.to_vec());
    }
    let text = std::str::from_utf8(source).map_err(|error| InstallError::SkillsCatalogMissing {
        path: PathBuf::from("<skill>"),
        detail: error.to_string(),
    })?;
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    if !text.starts_with("---\n") {
        return Ok(source.to_vec());
    }
    let Some(close) = text[4..].find("\n---") else {
        return Ok(source.to_vec());
    };
    let inner = &text[4..4 + close];
    let rest = &text[4 + close + 4..];
    let chunks = frontmatter_chunks(inner);
    let moved: Vec<(String, serde_yaml_ng::Value)> = chunks
        .iter()
        .filter_map(|chunk| match &chunk.key {
            Some(key) if !ALLOWED_SKILL_FIELDS.contains(&key.as_str()) => {
                parse_chunk_value(key, &chunk.lines)
                    .ok()
                    .map(|value| (key.clone(), value))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    if moved.is_empty() {
        return Ok(source.to_vec());
    }
    let mut metadata: Vec<(String, serde_yaml_ng::Value)> = moved;
    let mut kept: Vec<&FrontmatterChunk> = Vec::new();
    for chunk in &chunks {
        match &chunk.key {
            Some(key) if ALLOWED_SKILL_FIELDS.contains(&key.as_str()) => {
                if key == "metadata" {
                    if let Ok(serde_yaml_ng::Value::Mapping(existing)) =
                        parse_chunk_value(key, &chunk.lines)
                    {
                        for (key, value) in existing {
                            if let Some(key) = key.as_str() {
                                // An explicit metadata entry wins over a
                                // top-level key of the same name.
                                metadata.retain(|(existing, _)| existing != key);
                                metadata.push((key.to_string(), value));
                            }
                        }
                    }
                } else {
                    kept.push(chunk);
                }
            }
            Some(_) => {}
            None => kept.push(chunk),
        }
    }
    let mut lines: Vec<String> = kept
        .iter()
        .flat_map(|chunk| chunk.lines.iter().cloned())
        .collect();
    lines.push("metadata:".to_string());
    for (key, value) in &metadata {
        lines.push(format!(
            "  {}: {}",
            yaml_key(key),
            yaml_double_quote(&metadata_string(value))
        ));
    }
    let projected = format!("---\n{}\n---{}", lines.join("\n"), rest);
    Ok(projected.into_bytes())
}

struct FrontmatterChunk {
    key: Option<String>,
    lines: Vec<String>,
}

fn frontmatter_chunks(inner: &str) -> Vec<FrontmatterChunk> {
    let mut chunks = vec![FrontmatterChunk {
        key: None,
        lines: Vec::new(),
    }];
    for line in inner.lines() {
        let first = line.chars().next();
        let top_level = matches!(first, Some(character) if !character.is_whitespace() && character != '#' && character != '-')
            && line.contains(':');
        if top_level {
            let key = line
                .split_once(':')
                .map(|(key, _)| key.trim().trim_matches(['"', '\'']).to_string());
            chunks.push(FrontmatterChunk {
                key,
                lines: vec![line.to_string()],
            });
        } else {
            chunks
                .last_mut()
                .expect("initial chunk")
                .lines
                .push(line.to_string());
        }
    }
    chunks
}

fn parse_chunk_value(key: &str, lines: &[String]) -> Result<serde_yaml_ng::Value, InstallError> {
    let value: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&lines.join("\n")).map_err(|error| {
            InstallError::SkillsManifestInvalid {
                path: PathBuf::from("<skill>"),
                detail: format!("frontmatter key {key:?} is not valid YAML: {error}"),
            }
        })?;
    match value {
        serde_yaml_ng::Value::Mapping(mapping) => mapping
            .into_iter()
            .next()
            .map(|(_, value)| value)
            .ok_or_else(|| InstallError::SkillsManifestInvalid {
                path: PathBuf::from("<skill>"),
                detail: format!("frontmatter key {key:?} is empty"),
            }),
        _ => Ok(value),
    }
}

fn metadata_string(value: &serde_yaml_ng::Value) -> String {
    match value {
        serde_yaml_ng::Value::Bool(value) => value.to_string(),
        serde_yaml_ng::Value::Null => String::new(),
        serde_yaml_ng::Value::String(value) => value.clone(),
        serde_yaml_ng::Value::Number(value) => value.to_string(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

fn yaml_key(key: &str) -> String {
    if key.is_empty()
        || !key
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_.-".contains(character))
        || key
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_digit())
    {
        format!("\"{}\"", key.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        key.to_string()
    }
}

fn yaml_double_quote(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

fn projected_bytes(
    file: &SkillFile,
    installation_class: &str,
) -> Result<(Vec<u8>, String), InstallError> {
    let source = std::fs::read(&file.source)
        .map_err(|error| InstallError::io("read", &file.source, error))?;
    let shared = installation_class == "shared-root-capable";
    let bytes = project_skill_bytes(&source, shared && file.relative == "SKILL.md", "")?;
    let hash = format!("sha256:{:x}", Sha256::digest(&bytes));
    Ok((bytes, hash))
}

/// Options controlling a skill install.
#[derive(Clone, Debug, Default)]
pub struct SkillInstallOptions {
    /// Timestamp for new entries (tests); `None` uses the wall clock.
    pub now: Option<String>,
    /// Replace a manifest-owned file whose bytes drifted (default false: report only).
    pub repair_drift: bool,
}

/// What one agent install/repair did.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SkillInstallReport {
    /// Files written.
    pub written: Vec<String>,
    /// Files whose identical bytes were adopted (already present).
    pub adopted: Vec<String>,
    /// Manifest-owned files whose bytes drifted (reported, or repaired).
    pub drifted: Vec<String>,
    /// Paths preserved because they are unowned/unsafe.
    pub preserved: Vec<String>,
    /// Entries produced for the manifest.
    pub entries: Vec<ManagedFileEntry>,
}

/// Install every catalog skill for `agent_key`.
pub fn install_skills_for_agent(
    project_root: &Path,
    agent_key: &str,
    catalog: &SkillCatalog,
    options: &SkillInstallOptions,
) -> Result<SkillInstallReport, InstallError> {
    install_skills_for_agents(project_root, &[agent_key.to_string()], catalog, options)
}

/// Install the catalog for several agents, refreshing the shared manifest.
pub fn install_skills_for_agents(
    project_root: &Path,
    agent_keys: &[String],
    catalog: &SkillCatalog,
    options: &SkillInstallOptions,
) -> Result<SkillInstallReport, InstallError> {
    let now = options.now.clone().unwrap_or_else(now_utc_iso);
    let mut manifest = ManagedSkillManifest::load_strict(project_root)?
        .unwrap_or_else(|| ManagedSkillManifest::new("unknown", &now));
    let mut report = SkillInstallReport::default();
    let result = (|| -> Result<(), InstallError> {
        for key in agent_keys {
            let surface = agent_surface(key)?;
            let Some(root) = surface.skill.primary_root() else {
                continue;
            };
            let class = installation_class(surface);
            for skill in catalog.skills() {
                for file in &skill.files {
                    let installed_path = format!("{}{}/{}", root, skill.name, file.relative);
                    let destination = project_root.join(&installed_path);
                    let (desired_bytes, expected_hash) = projected_bytes(file, &class)?;
                    let owned = manifest
                        .find(&installed_path)
                        .filter(|entry| entry.agent_key == *key)
                        .map(|entry| entry.content_hash.clone());
                    match std::fs::symlink_metadata(&destination) {
                        Ok(metadata) if metadata.file_type().is_file() => {
                            let bytes = std::fs::read(&destination)
                                .map_err(|error| InstallError::io("read", &destination, error))?;
                            let hash = format!("sha256:{:x}", Sha256::digest(&bytes));
                            if hash == expected_hash {
                                report.adopted.push(installed_path.clone());
                            } else if owned.as_deref() == Some(hash.as_str()) {
                                if options.repair_drift {
                                    write_bytes(&destination, &desired_bytes)?;
                                    report.drifted.push(installed_path.clone());
                                    report.written.push(installed_path.clone());
                                } else {
                                    report.drifted.push(installed_path.clone());
                                    continue;
                                }
                            } else {
                                return Err(InstallError::SkillCollision { path: destination });
                            }
                        }
                        Ok(metadata) if metadata.file_type().is_symlink() => {
                            return Err(InstallError::SkillCollision { path: destination });
                        }
                        Ok(_) => return Err(InstallError::SkillCollision { path: destination }),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            write_bytes(&destination, &desired_bytes)?;
                            report.written.push(installed_path.clone());
                        }
                        Err(source) => {
                            return Err(InstallError::io("inspect", &destination, source));
                        }
                    }
                    let entry = ManagedFileEntry::copied(
                        &skill.name,
                        &file.relative,
                        &installed_path,
                        &class,
                        key,
                        &expected_hash,
                        &now,
                    );
                    manifest.add_entry(entry.clone());
                    report.entries.push(entry);
                }
            }
        }
        Ok(())
    })();
    manifest.updated_at = now;
    // Persist ownership for every file written before a later collision or I/O
    // failure, so a retry adopts what landed instead of treating it as unowned
    // operator content.
    let saved = manifest.save(project_root);
    result?;
    saved?;
    Ok(report)
}

/// Restore absent native/shared skill files for `agent_keys` (additive only).
pub fn restore_native_project_skills(
    project_root: &Path,
    agent_keys: &[String],
    catalog: &SkillCatalog,
    options: &SkillInstallOptions,
) -> Result<SkillInstallReport, InstallError> {
    install_skills_for_agents(project_root, agent_keys, catalog, options)
}

/// A tool-surface repair summary.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SurfaceRepairSummary {
    /// Files restored from the catalog.
    pub restored: Vec<String>,
    /// Managed files with drifted bytes (kept unless `repair_drift`).
    pub drifted: Vec<String>,
    /// Paths left untouched because they are absent from the catalog or unowned.
    pub preserved: Vec<String>,
    /// Manifest entries whose node is not a regular file.
    pub unusable: Vec<String>,
}

impl SurfaceRepairSummary {
    /// Whether the repair changed anything.
    pub fn changed(&self) -> bool {
        !self.restored.is_empty()
    }
}

/// Verify and repair the managed skill surface for the configured agents.
pub fn run_surface_repair(
    project_root: &Path,
    agent_keys: &[String],
    catalog: &SkillCatalog,
    repair_drift: bool,
) -> Result<SurfaceRepairSummary, InstallError> {
    let Some(manifest) = ManagedSkillManifest::load_strict(project_root)? else {
        return Ok(SurfaceRepairSummary::default());
    };
    let mut summary = SurfaceRepairSummary::default();
    let mut repairs: Vec<ManagedFileEntry> = Vec::new();
    for entry in &manifest.entries {
        if !agent_keys.iter().any(|key| key == &entry.agent_key) {
            continue;
        }
        if !matches!(
            agent_surface(&entry.agent_key).map(|surface| surface.skill.class),
            Ok(SkillClass::SharedRootCapable | SkillClass::NativeRootRequired)
        ) {
            continue;
        }
        let destination = project_root.join(&entry.installed_path);
        match std::fs::symlink_metadata(&destination) {
            Ok(metadata) if metadata.file_type().is_file() => {
                let bytes = std::fs::read(&destination)
                    .map_err(|error| InstallError::io("read", &destination, error))?;
                let hash = format!("sha256:{:x}", Sha256::digest(&bytes));
                if hash != entry.content_hash {
                    summary.drifted.push(entry.installed_path.clone());
                    if repair_drift
                        && let Some(skill) = catalog.skill(&entry.skill_name)
                        && let Some(file) = skill
                            .files
                            .iter()
                            .find(|file| file.relative == entry.source_file)
                    {
                        let (bytes, _) = projected_bytes(file, &entry.installation_class)?;
                        write_bytes(&destination, &bytes)?;
                        summary.restored.push(entry.installed_path.clone());
                        repairs.push(entry.clone());
                    }
                }
            }
            Ok(_) => summary.unusable.push(entry.installed_path.clone()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if let Some(skill) = catalog.skill(&entry.skill_name)
                    && let Some(file) = skill
                        .files
                        .iter()
                        .find(|file| file.relative == entry.source_file)
                {
                    let (bytes, _) = projected_bytes(file, &entry.installation_class)?;
                    write_bytes(&destination, &bytes)?;
                    summary.restored.push(entry.installed_path.clone());
                    repairs.push(entry.clone());
                } else {
                    summary.preserved.push(entry.installed_path.clone());
                }
            }
            Err(source) => return Err(InstallError::io("inspect", &destination, source)),
        }
    }
    if repairs.is_empty() {
        return Ok(summary);
    }
    let mut updated = manifest.clone();
    for entry in repairs {
        updated.add_entry(entry);
    }
    updated.updated_at = now_utc_iso();
    updated.save(project_root)?;
    Ok(summary)
}

fn write_bytes(destination: &Path, bytes: &[u8]) -> Result<(), InstallError> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| InstallError::io("create directory", parent, error))?;
    }
    sk_kernel::atomic_write(destination, bytes).map_err(|error| {
        InstallError::io("write skill", destination, std::io::Error::other(error))
    })?;
    Ok(())
}

#[allow(dead_code)]
fn write_bounded(source: &Path, destination: &Path) -> Result<(), InstallError> {
    let metadata = std::fs::symlink_metadata(source)
        .map_err(|error| InstallError::io("inspect", source, error))?;
    if !metadata.file_type().is_file() {
        return Err(InstallError::TemplatePathUnsafe {
            path: source.to_path_buf(),
            detail: "skill source is not a regular file".to_string(),
        });
    }
    let bytes = std::fs::read(source).map_err(|error| InstallError::io("read", source, error))?;
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| InstallError::io("create directory", parent, error))?;
    }
    sk_kernel::atomic_write(destination, &bytes).map_err(|error| {
        InstallError::io("write skill", destination, std::io::Error::other(error))
    })?;
    Ok(())
}

fn installation_class(surface: &AgentSurface) -> String {
    match surface.skill.class {
        SkillClass::SharedRootCapable => "shared-root-capable".to_string(),
        SkillClass::NativeRootRequired => "native-root-required".to_string(),
        SkillClass::WrapperOnly => "wrapper-only".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn catalog(root: &Path) -> SkillCatalog {
        write(&root.join("spec-kitty-demo/SKILL.md"), "# demo\n");
        write(&root.join("spec-kitty-demo/references/ref.md"), "ref\n");
        write(&root.join("adversarial-squad/SKILL.md"), "# squad\n");
        SkillCatalog::discover(root).expect("catalog")
    }

    #[test]
    fn discovers_skills_and_orders_files() {
        let directory = tempfile::tempdir().expect("temp");
        let catalog = catalog(directory.path());
        assert_eq!(
            catalog.names(),
            vec!["adversarial-squad", "spec-kitty-demo"]
        );
        let skill = catalog.skill("spec-kitty-demo").expect("skill");
        assert_eq!(skill.files[0].relative, "SKILL.md");
        assert_eq!(skill.files[1].relative, "references/ref.md");
        assert!(skill.files[0].content_hash.starts_with("sha256:"));
    }

    #[test]
    fn installs_into_the_native_root_and_writes_the_manifest_shape() {
        let project = tempfile::tempdir().expect("temp");
        let source = tempfile::tempdir().expect("temp");
        let catalog = catalog(source.path());
        let options = SkillInstallOptions {
            now: Some("2026-11-02T19:52:03.738333+00:00".to_string()),
            repair_drift: false,
        };
        let report = install_skills_for_agent(project.path(), "claude", &catalog, &options)
            .expect("install");
        assert_eq!(report.written.len(), 3);
        assert!(
            project
                .path()
                .join(".claude/skills/spec-kitty-demo/SKILL.md")
                .is_file()
        );
        let manifest = ManagedSkillManifest::load_strict(project.path())
            .expect("load")
            .expect("manifest");
        assert_eq!(manifest.version, 1);
        assert_eq!(manifest.entries.len(), 3);
        let text = std::fs::read_to_string(project.path().join(SKILLS_MANIFEST_REL)).unwrap();
        assert!(text.contains("\"skill_name\": \"spec-kitty-demo\""));
        assert!(!text.contains("\"origin\""));
        assert!(text.ends_with("}\n"));
        // A repeat install adopts identical bytes and is a no-op.
        let report = install_skills_for_agent(project.path(), "claude", &catalog, &options)
            .expect("install");
        assert_eq!(report.written.len(), 0);
        assert_eq!(report.adopted.len(), 3);
    }

    #[test]
    fn refuses_to_overwrite_unowned_content() {
        let project = tempfile::tempdir().expect("temp");
        let source = tempfile::tempdir().expect("temp");
        let catalog = catalog(source.path());
        write(
            &project
                .path()
                .join(".claude/skills/spec-kitty-demo/SKILL.md"),
            "operator content\n",
        );
        let error = install_skills_for_agent(
            project.path(),
            "claude",
            &catalog,
            &SkillInstallOptions::default(),
        )
        .expect_err("collision");
        assert_eq!(error.code(), "skill_collision");
        assert_eq!(
            std::fs::read_to_string(
                project
                    .path()
                    .join(".claude/skills/spec-kitty-demo/SKILL.md")
            )
            .unwrap(),
            "operator content\n"
        );
    }

    #[test]
    fn a_collision_still_records_the_files_that_landed() {
        let project = tempfile::tempdir().expect("temp");
        let source = tempfile::tempdir().expect("temp");
        let catalog = catalog(source.path());
        // Alphabetically first `adversarial-squad` lands; the later
        // `spec-kitty-demo` destination is unowned operator content.
        write(
            &project
                .path()
                .join(".claude/skills/spec-kitty-demo/SKILL.md"),
            "operator\n",
        );
        let error = install_skills_for_agent(
            project.path(),
            "claude",
            &catalog,
            &SkillInstallOptions::default(),
        )
        .expect_err("collision");
        assert_eq!(error.code(), "skill_collision");
        let manifest = ManagedSkillManifest::load_strict(project.path())
            .expect("load")
            .expect("manifest");
        assert!(
            manifest
                .entries
                .iter()
                .any(|entry| entry.installed_path.ends_with("adversarial-squad/SKILL.md"))
        );
        assert!(
            !manifest
                .entries
                .iter()
                .any(|entry| entry.installed_path.ends_with("spec-kitty-demo/SKILL.md"))
        );
    }

    #[test]
    fn repair_restores_missing_and_reports_drift() {
        let project = tempfile::tempdir().expect("temp");
        let source = tempfile::tempdir().expect("temp");
        let catalog = catalog(source.path());
        let options = SkillInstallOptions::default();
        install_skills_for_agent(project.path(), "claude", &catalog, &options).expect("install");
        std::fs::remove_file(
            project
                .path()
                .join(".claude/skills/spec-kitty-demo/SKILL.md"),
        )
        .unwrap();
        std::fs::write(
            project
                .path()
                .join(".claude/skills/adversarial-squad/SKILL.md"),
            "drifted\n",
        )
        .unwrap();
        let summary = run_surface_repair(project.path(), &["claude".to_string()], &catalog, false)
            .expect("repair");
        assert_eq!(
            summary.restored,
            vec![".claude/skills/spec-kitty-demo/SKILL.md"]
        );
        assert_eq!(
            summary.drifted,
            vec![".claude/skills/adversarial-squad/SKILL.md"]
        );
        assert_eq!(
            std::fs::read_to_string(
                project
                    .path()
                    .join(".claude/skills/adversarial-squad/SKILL.md")
            )
            .unwrap(),
            "drifted\n"
        );
    }

    #[test]
    fn manifest_rejects_unknown_fields_and_duplicate_owners() {
        let project = tempfile::tempdir().expect("temp");
        write(
            &project.path().join(SKILLS_MANIFEST_REL),
            "{\"version\":1,\"extra\":true,\"entries\":[]}\n",
        );
        assert_eq!(
            ManagedSkillManifest::load_strict(project.path())
                .expect_err("unknown field")
                .code(),
            "skills_manifest_invalid"
        );
        write(
            &project.path().join(SKILLS_MANIFEST_REL),
            "{\"version\":1,\"entries\":[{\"skill_name\":\"a\",\"source_file\":\"SKILL.md\",\"installed_path\":\"p\",\"installation_class\":\"native-root-required\",\"agent_key\":\"claude\",\"content_hash\":\"sha256:x\",\"installed_at\":\"t\"},{\"skill_name\":\"a\",\"source_file\":\"SKILL.md\",\"installed_path\":\"p\",\"installation_class\":\"native-root-required\",\"agent_key\":\"claude\",\"content_hash\":\"sha256:x\",\"installed_at\":\"t\"}]}\n",
        );
        assert_eq!(
            ManagedSkillManifest::load_strict(project.path())
                .expect_err("duplicate")
                .code(),
            "skills_manifest_invalid"
        );
    }
}
