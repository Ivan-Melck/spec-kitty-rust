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

pub mod pack_skill;

pub use pack_skill::{
    InForceSkills, PackSkill, PackSkillRepository, PackTier, PreparedSkill, SkillExpansion,
    SkillForm, SkillInvocation, SkillParameter, explicit_activated_skills, pack_skills,
    pack_skills_matter, render_pack_skill, resolve_project_skill_catalog,
    resolve_project_skill_catalog_with, shipped_catalog,
};

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
    /// `builtin` for the shipped doctrine skills, `pack` for a charter-pack
    /// skill rendered into the staged root (ADR 2026-09-27-1). Pack skills are
    /// project-root only: they never feed a user-global asset batch.
    pub origin: String,
    /// Pack source reference (pack origin only), project-relative when inside
    /// the project and absolute otherwise.
    pub source_ref: String,
    /// Pack skill input hash (pack origin only).
    pub source_hash: String,
}

impl CanonicalSkill {
    /// A shipped (built-in) skill with no pack provenance.
    pub fn shipped(name: String, files: Vec<SkillFile>) -> Self {
        Self {
            name,
            files,
            origin: ORIGIN_BUILTIN.to_string(),
            source_ref: String::new(),
            source_hash: String::new(),
        }
    }

    /// Whether this skill came from a charter pack.
    pub fn is_pack(&self) -> bool {
        self.origin == ORIGIN_PACK
    }
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
            // Upstream's install order is the alphabetical source path
            // (`SKILL.md`, then `assets/`, `references/`, `scripts/`), which is
            // also the order its manifest records.
            files.sort_by(|left, right| left.relative.cmp(&right.relative));
            skills.push(CanonicalSkill::shipped(name, files));
        }
        Ok(Self {
            root: root.to_path_buf(),
            skills,
        })
    }

    /// A catalog with no skills, rooted at `root` (a project with no shipped tree).
    pub fn empty(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            skills: Vec::new(),
        }
    }

    /// Append one skill, keeping the catalog sorted by name.
    ///
    /// Additive: the staged pack skills are appended through this seam so the
    /// installer and every read surface see one catalog, never a second one.
    pub fn push_skill(&mut self, skill: CanonicalSkill) {
        self.skills.retain(|existing| existing.name != skill.name);
        self.skills.push(skill);
        self.skills
            .sort_by(|left, right| left.name.cmp(&right.name));
    }

    /// The catalog root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// A catalog narrowed to the skills `predicate` accepts, in the same order.
    ///
    /// Additive selection helper: the source tree, hashes and install rules are
    /// unchanged, so a migration that owns a name subset (the `spk-` pack)
    /// installs exactly that subset through the one canonical catalog instead of
    /// a second discovery path.
    pub fn filtered(&self, predicate: impl Fn(&CanonicalSkill) -> bool) -> Self {
        Self {
            root: self.root.clone(),
            skills: self
                .skills
                .iter()
                .filter(|skill| predicate(skill))
                .cloned()
                .collect(),
        }
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

/// Delivery mode for a project skill file that is a link into the user-global
/// canonical skill root (upstream `DELIVERY_SYMLINK`).
pub const DELIVERY_SYMLINK: &str = "symlink";

/// Delivery mode for an ordinary project copy (upstream `DELIVERY_COPY`).
pub const DELIVERY_COPY: &str = "copy";

/// The user-global canonical skill root for `agent_key`.
///
/// Mirrors `specify_cli.skills.paths.get_primary_global_skill_root`: the
/// project-relative primary skill root (`.claude/skills`, `.agents/skills`, …)
/// mirrored beneath the user's home directory. `None` for wrapper-only agents
/// and for an unresolvable home directory (the caller then treats a project
/// link as unverifiable rather than guessing).
pub fn global_skill_root(agent_key: &str) -> Option<std::path::PathBuf> {
    let root = agent_surface(agent_key).ok()?.skill.primary_root()?;
    let home = sk_kernel::paths::home_directory().ok()?;
    Some(home.join(root.trim_matches('/')))
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

    /// Build an entry for a *retained* user-global symlink.
    ///
    /// Upstream records `delivery_mode: symlink` for a project skill file that
    /// is (or was adopted as) a link into the user-global canonical root; the
    /// content hash is the linked canonical file's digest.
    pub fn symlinked(
        skill_name: &str,
        source_file: &str,
        installed_path: &str,
        installation_class: &str,
        agent_key: &str,
        content_hash: &str,
        installed_at: &str,
    ) -> Self {
        Self {
            delivery_mode: DELIVERY_SYMLINK.to_string(),
            ..Self::copied(
                skill_name,
                source_file,
                installed_path,
                installation_class,
                agent_key,
                content_hash,
                installed_at,
            )
        }
    }

    /// Whether this entry records a link into the user-global root.
    pub fn is_symlink_delivery(&self) -> bool {
        self.delivery_mode == DELIVERY_SYMLINK
    }

    /// Attach pack provenance (the pack source reference and input hash).
    pub fn with_pack_provenance(
        mut self,
        origin: &str,
        source_ref: &str,
        source_hash: &str,
    ) -> Self {
        self.origin = origin.to_string();
        self.source_ref = source_ref.to_string();
        self.source_hash = source_hash.to_string();
        self
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
        let path = managed_skill_destination(project_root, SKILLS_MANIFEST_REL)?;
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
                managed_entry_destination(project_root, &entry)?;
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
        let path = managed_skill_destination(project_root, SKILLS_MANIFEST_REL)?;
        for entry in &self.entries {
            managed_entry_destination(project_root, entry)?;
        }
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
        }
        // A first save keeps install order: upstream appends entries as it
        // installs them (native agents, then shared agents per catalog file).
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
    /// The running CLI version stamped into `spec_kitty_version`.
    ///
    /// Matches upstream: the field is only written by the native-root install
    /// path. A shared-root-only selection leaves the manifest's field at its
    /// default (empty), exactly like the pinned tool-surface repair does.
    pub spec_kitty_version: Option<String>,
}

impl SkillInstallOptions {
    /// Options stamped with the running CLI version.
    pub fn with_version(version: impl Into<String>) -> Self {
        Self {
            spec_kitty_version: Some(version.into()),
            ..Self::default()
        }
    }
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
    /// Non-fatal warnings for the CLI to render (skipped optional surfaces).
    pub warnings: Vec<String>,
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
///
/// Native-root agents install every catalog file in catalog order; shared-root
/// agents project the same catalog one file at a time across the selected
/// agents, which is the entry order a fresh upstream init records. An already
/// identical file is adopted, an owned drifted file is reported (and only
/// replaced with [`SkillInstallOptions::repair_drift`]), and an unowned
/// different file is a collision — never overwritten.
pub fn install_skills_for_agents(
    project_root: &Path,
    agent_keys: &[String],
    catalog: &SkillCatalog,
    options: &SkillInstallOptions,
) -> Result<SkillInstallReport, InstallError> {
    let now = options.now.clone().unwrap_or_else(now_utc_iso);
    let existing = ManagedSkillManifest::load_strict(project_root)?;
    let manifest_existed = existing.is_some();
    let mut manifest = existing.unwrap_or_else(|| ManagedSkillManifest::new("", &now));
    // Upstream stamps `spec_kitty_version` only in the per-agent loop that
    // installs native-class roots; a shared-root-only selection gets its
    // manifest from tool-surface repair, whose entries leave the field empty.
    // Preserve that provenance instead of stamping every selection.
    let stamps_version = agent_keys.iter().any(|key| {
        agent_surface(key).is_ok_and(|surface| {
            surface.skill.class == SkillClass::NativeRootRequired
                && surface.skill.primary_root().is_some()
        })
    });
    if stamps_version && let Some(version) = &options.spec_kitty_version {
        manifest.spec_kitty_version = version.clone();
    }
    let mut native_agents: Vec<&String> = Vec::new();
    let mut shared_agents: Vec<&String> = Vec::new();
    for key in agent_keys {
        let surface = agent_surface(key)?;
        match surface.skill.class {
            SkillClass::NativeRootRequired if surface.skill.primary_root().is_some() => {
                native_agents.push(key)
            }
            SkillClass::SharedRootCapable if surface.skill.primary_root().is_some() => {
                shared_agents.push(key)
            }
            _ => {}
        }
    }
    let mut report = SkillInstallReport::default();
    let result = (|| -> Result<(), InstallError> {
        for key in &native_agents {
            let surface = agent_surface(key)?;
            let Some(root) = surface.skill.primary_root() else {
                continue;
            };
            let class = installation_class(surface);
            for skill in catalog.skills() {
                for file in &skill.files {
                    let installed_path = format!("{}{}/{}", root, skill.name, file.relative);
                    install_managed_file(
                        project_root,
                        &mut manifest,
                        &mut report,
                        key,
                        skill,
                        &installed_path,
                        &class,
                        file,
                        &now,
                        options.repair_drift,
                    )?;
                }
            }
        }
        for skill in catalog.skills() {
            for file in &skill.files {
                for key in &shared_agents {
                    let surface = agent_surface(key)?;
                    let Some(root) = surface.skill.primary_root() else {
                        continue;
                    };
                    let class = installation_class(surface);
                    let installed_path = format!("{}{}/{}", root, skill.name, file.relative);
                    install_managed_file(
                        project_root,
                        &mut manifest,
                        &mut report,
                        key,
                        skill,
                        &installed_path,
                        &class,
                        file,
                        &now,
                        options.repair_drift,
                    )?;
                }
            }
        }
        retire_pack_skills(
            project_root,
            &mut manifest,
            &mut report,
            catalog,
            agent_keys,
        )?;
        Ok(())
    })();
    manifest.updated_at = now;
    // Persist ownership for every file written before a later collision or I/O
    // failure, so a retry adopts what landed instead of treating it as unowned
    // operator content. A brand-new manifest with no entries is not written: the
    // pinned oracle creates no `skills-manifest.json` when every optional
    // destination was skipped (for example a symlinked agent skill root).
    let saved = if manifest_existed || !report.entries.is_empty() {
        manifest.save(project_root).map(|_| ())
    } else {
        Ok(())
    };
    result?;
    saved?;
    Ok(report)
}

/// Retire pack-origin files that the resolved catalog no longer contains.
///
/// A pack skill that left the pack (or whose activation was removed) must not
/// survive as an installed copy: the catalog is the inventory authority, and a
/// manifest entry left behind would keep an obsolete skill in every agent root
/// (Source #5193). A file whose bytes still match its recorded hash is removed
/// with its (now empty) skill directory; a locally edited file is preserved and
/// reported so the operator can consent to losing the edit, exactly like every
/// other managed file this writer refuses to overwrite.
fn retire_pack_skills(
    project_root: &Path,
    manifest: &mut ManagedSkillManifest,
    report: &mut SkillInstallReport,
    catalog: &SkillCatalog,
    agent_keys: &[String],
) -> Result<(), InstallError> {
    let wanted: BTreeSet<(String, String)> = catalog
        .skills()
        .iter()
        .flat_map(|skill| {
            skill
                .files
                .iter()
                .map(move |file| (skill.name.clone(), file.relative.clone()))
        })
        .collect();
    let stale: Vec<ManagedFileEntry> = manifest
        .entries
        .iter()
        .filter(|entry| {
            entry.origin == ORIGIN_PACK
                && agent_keys.iter().any(|key| key == &entry.agent_key)
                && !wanted.contains(&(entry.skill_name.clone(), entry.source_file.clone()))
        })
        .cloned()
        .collect();
    if stale.is_empty() {
        return Ok(());
    }
    let mut retired_directories: BTreeSet<PathBuf> = BTreeSet::new();
    let mut retained: BTreeSet<String> = BTreeSet::new();
    for entry in &stale {
        let destination = match managed_skill_destination(project_root, &entry.installed_path) {
            Ok(destination) => destination,
            Err(error) => {
                report.warnings.push(format!(
                    "Preserved unverifiable pack skill {}: {error}",
                    entry.installed_path
                ));
                report.preserved.push(entry.installed_path.clone());
                retained.insert(entry.installed_path.clone());
                continue;
            }
        };
        match std::fs::symlink_metadata(&destination) {
            Ok(metadata) if metadata.file_type().is_file() => {
                let bytes = std::fs::read(&destination)
                    .map_err(|error| InstallError::io("read", &destination, error))?;
                let hash = format!("sha256:{:x}", Sha256::digest(&bytes));
                if hash != entry.content_hash {
                    report.warnings.push(format!(
                        "Preserved a locally modified pack skill {} for {}: it differs from the \
                         installed bytes, so retiring it needs explicit consent; delete it by hand \
                         once you no longer need the edit",
                        entry.installed_path, entry.agent_key
                    ));
                    report.preserved.push(entry.installed_path.clone());
                    retained.insert(entry.installed_path.clone());
                    continue;
                }
                std::fs::remove_file(&destination)
                    .map_err(|error| InstallError::io("remove", &destination, error))?;
                if let Some(parent) = destination.parent() {
                    retired_directories.insert(parent.to_path_buf());
                }
            }
            Ok(_) => {
                // A link or a directory is not this writer's file: leave it.
                report.preserved.push(entry.installed_path.clone());
                retained.insert(entry.installed_path.clone());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => return Err(InstallError::io("inspect", &destination, source)),
        }
    }
    // Remove skill directories that the retirement emptied, deepest first.
    let mut directories: Vec<PathBuf> = retired_directories.into_iter().collect();
    directories.sort_by_key(|directory| std::cmp::Reverse(directory.components().count()));
    for directory in directories {
        if std::fs::read_dir(&directory).is_ok_and(|mut entries| entries.next().is_none()) {
            let _ = std::fs::remove_dir(&directory);
        }
    }
    manifest.entries.retain(|entry| {
        !stale.iter().any(|item| {
            item.installed_path == entry.installed_path && item.agent_key == entry.agent_key
        }) || retained.contains(&entry.installed_path)
    });
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn install_managed_file(
    project_root: &Path,
    manifest: &mut ManagedSkillManifest,
    report: &mut SkillInstallReport,
    key: &str,
    skill: &CanonicalSkill,
    installed_path: &str,
    class: &str,
    file: &SkillFile,
    now: &str,
    repair_drift: bool,
) -> Result<(), InstallError> {
    let skill_name = skill.name.as_str();
    // A shipped skill is also written to the user-global canonical root, which
    // is what makes a retained `symlink` delivery verifiable and what a new
    // install links against (Source's global asset batch). A pack skill is
    // project-root only and never reaches a user-global root.
    if !skill.is_pack() {
        install_global_canonical(report, key, skill, file, installed_path)?;
    }
    // The installed path is catalog-derived, but the destination is still an
    // untrusted path under the project root: resolve it with the same
    // traversal/symlink-escape boundary the manifest-owned restores use, so a
    // symlinked agent root cannot write outside the project.
    //
    // A refused optional destination is preserved and reported, not fatal: the
    // pinned `init` skips a harness whose skill root is a symlink (it warns and
    // installs the remaining agents), so aborting here would be a new refusal of
    // a supported user layout. The strict refusal still applies to
    // manifest-owned restores (`run_surface_repair`), where the manifest names
    // the path and escaping it is a contract violation.
    let destination = match managed_skill_destination(project_root, installed_path) {
        Ok(destination) => destination,
        Err(error) => {
            report.warnings.push(format!(
                "Skipped charter skill {installed_path} for {key}: {error}"
            ));
            report.preserved.push(installed_path.to_string());
            return Ok(());
        }
    };
    let (desired_bytes, expected_hash) = projected_bytes(file, class)?;
    let owned = manifest
        .find(installed_path)
        .filter(|entry| entry.agent_key == key)
        .map(|entry| entry.content_hash.clone());
    match std::fs::symlink_metadata(&destination) {
        Ok(metadata) if metadata.file_type().is_file() => {
            let bytes = std::fs::read(&destination)
                .map_err(|error| InstallError::io("read", &destination, error))?;
            let hash = format!("sha256:{:x}", Sha256::digest(&bytes));
            if hash == expected_hash {
                report.adopted.push(installed_path.to_string());
            } else if owned.as_deref() == Some(hash.as_str()) {
                if repair_drift {
                    write_bytes(&destination, &desired_bytes)?;
                    report.drifted.push(installed_path.to_string());
                    report.written.push(installed_path.to_string());
                } else {
                    report.drifted.push(installed_path.to_string());
                    return Ok(());
                }
            } else {
                return Err(InstallError::SkillCollision { path: destination });
            }
        }
        Ok(metadata) if metadata.file_type().is_symlink() => {
            // Upstream keeps a project link into the user-global canonical
            // root: `_project_skill_file` returns `preserved` for any symlink
            // whose content identity it did not itself write, and
            // `_managed_link` recognises the verified case. A link is never a
            // collision here (that would refuse a supported 3.0.3 layout), and
            // it is never silently replaced with a copy.
            let verified = manifest
                .find(installed_path)
                .filter(|entry| entry.agent_key == key)
                .is_some_and(|entry| {
                    entry.is_symlink_delivery()
                        && link_matches_global_root(
                            &destination,
                            &entry.skill_name,
                            &entry.source_file,
                            key,
                        )
                        && std::fs::read(&destination).is_ok_and(|bytes| {
                            format!("sha256:{:x}", Sha256::digest(&bytes)) == entry.content_hash
                        })
                });
            if verified {
                report.adopted.push(installed_path.to_string());
                return Ok(());
            }
            report.preserved.push(installed_path.to_string());
            report.warnings.push(format!(
                "Preserved unverified skill link {installed_path} for {key}: its target is not the \
                 canonical user-global skill file; no ownership was taken"
            ));
            return Ok(());
        }
        Ok(_) => return Err(InstallError::SkillCollision { path: destination }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            write_bytes(&destination, &desired_bytes)?;
            report.written.push(installed_path.to_string());
        }
        Err(source) => {
            return Err(InstallError::io("inspect", &destination, source));
        }
    }
    let entry = ManagedFileEntry::copied(
        skill_name,
        &file.relative,
        installed_path,
        class,
        key,
        &expected_hash,
        now,
    )
    .with_pack_provenance(&skill.origin, &skill.source_ref, &skill.source_hash);
    manifest.add_entry(entry.clone());
    report.entries.push(entry);
    Ok(())
}

/// Write one shipped skill file into its user-global canonical root.
///
/// The global root mirrors the project skill root beneath the user's home
/// (Source's `global_assets` batch). Identical bytes are adopted; a file with
/// different bytes that this writer does not own is preserved and reported, and
/// an unreadable or absent home leaves the project install untouched.
fn install_global_canonical(
    report: &mut SkillInstallReport,
    key: &str,
    skill: &CanonicalSkill,
    file: &SkillFile,
    installed_path: &str,
) -> Result<(), InstallError> {
    let Some(global_root) = global_skill_root(key) else {
        return Ok(());
    };
    let destination = global_root.join(&skill.name).join(&file.relative);
    let source = std::fs::read(&file.source)
        .map_err(|error| InstallError::io("read", &file.source, error))?;
    match std::fs::symlink_metadata(&destination) {
        Ok(metadata) if metadata.file_type().is_file() => {
            let existing = std::fs::read(&destination)
                .map_err(|error| InstallError::io("read", &destination, error))?;
            if existing == source {
                return Ok(());
            }
            report.preserved.push(installed_path.to_string());
            report.warnings.push(format!(
                "Preserved a user-global skill file that differs from the shipped \
                 {installed_path} for {key}: {}. The project copy was installed; the global \
                 canonical file is user-owned and was not overwritten",
                destination.display()
            ));
            return Ok(());
        }
        Ok(_) => {
            report.preserved.push(installed_path.to_string());
            report.warnings.push(format!(
                "Preserved a non-file user-global skill path for {key}: {}",
                destination.display()
            ));
            return Ok(());
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => return Err(InstallError::io("inspect", &destination, source)),
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| InstallError::io("create directory", parent, error))?;
    }
    sk_kernel::atomic_write(&destination, &source)
        .map_err(|error| InstallError::io("write", &destination, std::io::Error::other(error)))?;
    Ok(())
}

/// Whether `destination` is a link to `global_skill_root(agent_key)/skill/file`.
///
/// Mirrors `installer._managed_link`: the *literal* link target is resolved
/// lexically against the link's parent (no filesystem canonicalization, so a
/// dangling link is still classified by its authored target) and compared with
/// the global canonical file path.
fn link_matches_global_root(
    destination: &Path,
    skill_name: &str,
    source_file: &str,
    agent_key: &str,
) -> bool {
    let Ok(target) = std::fs::read_link(destination) else {
        return false;
    };
    let Some(global_root) = global_skill_root(agent_key) else {
        return false;
    };
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let literal = lexical_normalize(&parent.join(target));
    literal == lexical_normalize(&global_root.join(skill_name).join(source_file))
}

/// Lexically normalize `.`/`..` components the way `os.path.normpath` does.
fn lexical_normalize(path: &Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut out = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
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
    /// Non-fatal warnings for the CLI to render (skipped optional surfaces).
    pub warnings: Vec<String>,
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
        let destination = match managed_skill_destination(project_root, &entry.installed_path) {
            Ok(destination) => destination,
            // The manifest owns this path, but the surrounding directory became
            // an escaping symlink. Refuse the write and report it instead of
            // failing the whole optional surface (upstream preserves it and
            // warns); the manifest entry is left untouched for a later repair.
            Err(error) => {
                summary.warnings.push(format!(
                    "Could not verify managed skill {}: {error}",
                    entry.installed_path
                ));
                summary.preserved.push(entry.installed_path.clone());
                continue;
            }
        };
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

fn managed_skill_destination(project_root: &Path, relative: &str) -> Result<PathBuf, InstallError> {
    let invalid = |detail: String| InstallError::SkillsManifestInvalid {
        path: project_root.join(SKILLS_MANIFEST_REL),
        detail,
    };
    crate::manifest::validate_relative_posix(relative, "managed skill destination")
        .map_err(|error| invalid(error.to_string()))?;
    sk_kernel::resolve_within(project_root, Path::new(relative))
        .map_err(|error| invalid(error.to_string()))
}

/// Resolve one manifest entry's `installed_path`, accepting the one supported
/// symlink shape: an entry whose `delivery_mode` is `symlink` and whose link
/// target is that agent's user-global canonical skill file.
///
/// Source keeps such entries in `skills-manifest.json` (`_valid_skill_entry` +
/// `_managed_link`) even though the link resolves outside the project. Every
/// other escaping path is still refused.
fn managed_entry_destination(
    project_root: &Path,
    entry: &ManagedFileEntry,
) -> Result<PathBuf, InstallError> {
    crate::manifest::validate_relative_posix(&entry.installed_path, "managed skill destination")
        .map_err(|error| InstallError::SkillsManifestInvalid {
            path: project_root.join(SKILLS_MANIFEST_REL),
            detail: error.to_string(),
        })?;
    let destination = project_root.join(&entry.installed_path);
    // A manifest entry records the mode a file was *written* with; the node on
    // disk may since have become the 3.0.3 user-global link (Source's
    // `_valid_skill_entry`/`_managed_link` accept that pair). Only the literal
    // target decides safety here: the agent's global skill root is allowed,
    // any other escape is refused by `resolve_within` below.
    if std::fs::symlink_metadata(&destination)
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
        && let Some(global_root) = global_skill_root(&entry.agent_key)
        && let Ok(target) = std::fs::read_link(&destination)
    {
        let parent = destination.parent().unwrap_or_else(|| Path::new("."));
        let literal = lexical_normalize(&parent.join(target));
        if literal.starts_with(lexical_normalize(&global_root)) {
            return Ok(destination);
        }
    }
    managed_skill_destination(project_root, &entry.installed_path)
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
            spec_kitty_version: None,
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

    #[cfg(unix)]
    #[test]
    fn a_symlinked_agent_root_cannot_escape_the_project() {
        use std::os::unix::fs::symlink;

        let project = tempfile::tempdir().expect("temp");
        let outside = tempfile::tempdir().expect("temp");
        let source = tempfile::tempdir().expect("temp");
        let catalog = catalog(source.path());
        symlink(outside.path(), project.path().join(".claude")).expect("symlink");
        // The optional catalog install is skipped with a warning (upstream
        // refuses only this surface and continues), never written through the
        // link, and a brand-new empty manifest is not created.
        let report = install_skills_for_agent(
            project.path(),
            "claude",
            &catalog,
            &SkillInstallOptions::default(),
        )
        .expect("optional surface is skipped, not fatal");
        assert!(report.written.is_empty());
        assert_eq!(report.preserved.len(), 3);
        assert!(
            report
                .warnings
                .iter()
                .all(|warning| warning.contains("resolves outside root")),
            "warnings: {:?}",
            report.warnings
        );
        assert!(report.entries.is_empty());
        assert!(!project.path().join(SKILLS_MANIFEST_REL).exists());
        assert!(
            std::fs::read_dir(outside.path())
                .expect("read outside")
                .next()
                .is_none(),
            "nothing may be written outside the project root"
        );
    }

    #[test]
    fn unsafe_manifest_destinations_refuse_before_surface_repair() {
        let sandbox = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir_in(sandbox.path()).unwrap();
        let source = tempfile::tempdir_in(sandbox.path()).unwrap();
        let catalog = catalog(source.path());
        let outside = source.path().join("escaped.md");
        for installed in [
            "../escaped.md".to_owned(),
            outside.to_string_lossy().into_owned(),
        ] {
            let mut manifest = ManagedSkillManifest::new("test", "now");
            manifest.add_entry(ManagedFileEntry::copied(
                "spec-kitty-demo",
                "SKILL.md",
                &installed,
                "native-root-required",
                "claude",
                "sha256:x",
                "now",
            ));
            let path = project.path().join(SKILLS_MANIFEST_REL);
            write(&path, &serde_json::to_string(&manifest).unwrap());
            let before = std::fs::read(&path).unwrap();
            assert!(ManagedSkillManifest::load_strict(project.path()).is_err());
            assert!(
                run_surface_repair(project.path(), &["claude".to_owned()], &catalog, true).is_err()
            );
            assert_eq!(std::fs::read(&path).unwrap(), before);
            assert!(!outside.exists());
            assert!(!sandbox.path().join("escaped.md").exists());
            std::fs::remove_file(&path).unwrap();
            assert!(manifest.save(project.path()).is_err());
            assert!(!path.exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_skill_parents_cannot_redirect_manifest_repair_outside_the_project() {
        let project = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), project.path().join(".claude")).unwrap();
        let mut manifest = ManagedSkillManifest::new("test", "now");
        manifest.add_entry(ManagedFileEntry::copied(
            "spec-kitty-demo",
            "SKILL.md",
            ".claude/skills/spec-kitty-demo/SKILL.md",
            "native-root-required",
            "claude",
            "sha256:x",
            "now",
        ));
        write(
            &project.path().join(SKILLS_MANIFEST_REL),
            &serde_json::to_string(&manifest).unwrap(),
        );
        assert!(ManagedSkillManifest::load_strict(project.path()).is_err());
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
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
