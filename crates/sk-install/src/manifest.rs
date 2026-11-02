//! The managed-asset manifest: the single ownership authority for generated files.
//!
//! Wire format mirrors upstream `.kittify/command-skills-manifest.json`
//! (`skills/manifest_store.py`): `schema_version: 1`, entries sorted by `path`,
//! JSON object keys sorted, 2-space indent, trailing newline. That shape is a
//! shared contract with the Python CLI, so nothing is added to it here; the
//! recovery record and init receipt live in [`crate::init`] instead.
//!
//! Rules enforced at the boundary:
//!
//! - an absent manifest is an empty manifest (a fresh project), never an error;
//! - a corrupt, wrongly-versioned, malformed or duplicate-path manifest is a
//!   typed error, never silently repaired;
//! - `content_hash` is a lowercase hex SHA-256 (the deterministic ownership hash);
//! - `agents` is stored sorted and duplicate-free, so reference counting is
//!   deterministic across runs and agent orderings;
//! - serialization is byte-stable for equal manifests, so an idempotent run
//!   writes nothing.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::agent::agent_surface;
use crate::error::InstallError;

/// Project-relative path of the managed-asset manifest (POSIX).
pub const MANIFEST_REL: &str = ".kittify/command-skills-manifest.json";

/// The only supported on-disk schema version.
pub const SCHEMA_VERSION: u32 = 1;

/// The deterministic ownership hash of `bytes`: lowercase hex SHA-256.
pub fn content_hash(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// One owned path and the agents that reference it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestEntry {
    /// Project-relative POSIX path, e.g. `.agents/skills/spec-kitty-plan/SKILL.md`.
    pub path: String,
    /// Lowercase hex SHA-256 of the installed bytes at the last proven write.
    pub content_hash: String,
    /// Agent keys that installed this entry, sorted and duplicate-free.
    pub agents: Vec<String>,
    /// ISO-8601 UTC timestamp at which this entry was first written.
    pub installed_at: String,
    /// The version string that wrote the entry.
    pub spec_kitty_version: String,
}

impl ManifestEntry {
    /// Build a normalized entry (agents sorted/de-duplicated, hash validated).
    pub fn new(
        path: impl Into<String>,
        content_hash: impl Into<String>,
        agents: impl IntoIterator<Item = String>,
        installed_at: impl Into<String>,
        spec_kitty_version: impl Into<String>,
    ) -> Result<Self, InstallError> {
        let path = path.into();
        validate_relative_posix(&path, MANIFEST_REL)?;
        let content_hash = content_hash.into();
        if !is_lower_hex_sha256(&content_hash) {
            return Err(InstallError::ManifestInvalidEntry {
                path: PathBuf::from(MANIFEST_REL),
                detail: format!("content_hash for {path:?} is not a lowercase hex SHA-256"),
            });
        }
        let agents = normalize_agents(&path, agents)?;
        Ok(Self {
            path,
            content_hash,
            agents,
            installed_at: installed_at.into(),
            spec_kitty_version: spec_kitty_version.into(),
        })
    }

    /// Return a copy with `agent` added to the owner set (idempotent).
    pub fn with_agent_added(&self, agent: &str) -> Self {
        let mut agents: BTreeSet<String> = self.agents.iter().cloned().collect();
        agents.insert(agent.to_string());
        Self {
            agents: agents.into_iter().collect(),
            ..self.clone()
        }
    }

    /// Return a copy with `agent` removed from the owner set.
    pub fn with_agent_removed(&self, agent: &str) -> Self {
        Self {
            agents: self
                .agents
                .iter()
                .filter(|owner| owner.as_str() != agent)
                .cloned()
                .collect(),
            ..self.clone()
        }
    }

    /// Return a copy recording a refreshed content hash.
    pub fn with_content_hash(&self, content_hash: impl Into<String>) -> Self {
        Self {
            content_hash: content_hash.into(),
            ..self.clone()
        }
    }

    /// Whether `hash` matches both the recorded hash and the caller's expectation.
    pub fn owns_exactly(&self, hash: &str) -> bool {
        self.content_hash == hash
    }
}

/// The in-memory manifest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssetManifest {
    entries: Vec<ManifestEntry>,
}

impl Default for AssetManifest {
    fn default() -> Self {
        Self::empty()
    }
}

impl AssetManifest {
    /// An empty manifest (what an absent file means).
    pub fn empty() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// The entries, sorted by path.
    pub fn entries(&self) -> &[ManifestEntry] {
        &self.entries
    }

    /// Look up an entry by project-relative path.
    pub fn find(&self, path: &str) -> Option<&ManifestEntry> {
        self.entries.iter().find(|entry| entry.path == path)
    }

    /// Insert or replace the entry for `entry.path`, keeping the set sorted.
    pub fn upsert(&mut self, entry: ManifestEntry) {
        self.entries.retain(|existing| existing.path != entry.path);
        self.entries.push(entry);
        self.entries
            .sort_by(|left, right| left.path.cmp(&right.path));
    }

    /// Remove the entry for `path` (no-op when absent).
    pub fn remove_path(&mut self, path: &str) -> Option<ManifestEntry> {
        let position = self.entries.iter().position(|entry| entry.path == path)?;
        Some(self.entries.remove(position))
    }

    /// Remove and return every entry owned by `agent`.
    pub fn remove_entries_for_agent(&mut self, agent: &str) -> Vec<ManifestEntry> {
        let mut removed = Vec::new();
        self.entries.retain(|entry| {
            if entry.agents.iter().any(|owner| owner == agent) {
                removed.push(entry.clone());
                false
            } else {
                true
            }
        });
        removed
    }

    /// Every owner agent recorded across all entries, sorted.
    pub fn owners(&self) -> BTreeSet<String> {
        self.entries
            .iter()
            .flat_map(|entry| entry.agents.iter().cloned())
            .collect()
    }

    /// Whether two manifests agree once volatile timestamps are ignored.
    ///
    /// `installed_at` is a per-invocation wall-clock stamp; comparing manifests
    /// with it neutralized keeps a cross-invocation difference from planning a
    /// phantom rewrite (upstream #4134).
    pub fn equals_ignoring_timestamps(&self, other: &Self) -> bool {
        if self.entries.len() != other.entries.len() {
            return false;
        }
        self.entries
            .iter()
            .zip(other.entries.iter())
            .all(|(left, right)| {
                left.path == right.path
                    && left.content_hash == right.content_hash
                    && left.agents == right.agents
                    && left.spec_kitty_version == right.spec_kitty_version
            })
    }

    /// Serialize to the canonical wire bytes (deterministic).
    ///
    /// Object keys are emitted alphabetically (upstream `sort_keys=True`), the
    /// indent is two spaces, and the document ends with one newline. Entries are
    /// sorted by path. A `serde_json::Map` is used rather than a struct so key
    /// order is the map's lexical order instead of declaration order.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut entries: Vec<&ManifestEntry> = self.entries.iter().collect();
        entries.sort_by(|left, right| left.path.cmp(&right.path));

        let entry_values: Vec<serde_json::Value> = entries
            .into_iter()
            .map(|entry| {
                let mut object = serde_json::Map::new();
                object.insert(
                    "agents".to_string(),
                    serde_json::Value::Array(
                        entry
                            .agents
                            .iter()
                            .map(|agent| serde_json::Value::from(agent.clone()))
                            .collect(),
                    ),
                );
                object.insert(
                    "content_hash".to_string(),
                    serde_json::Value::from(entry.content_hash.clone()),
                );
                object.insert(
                    "installed_at".to_string(),
                    serde_json::Value::from(entry.installed_at.clone()),
                );
                object.insert(
                    "path".to_string(),
                    serde_json::Value::from(entry.path.clone()),
                );
                object.insert(
                    "spec_kitty_version".to_string(),
                    serde_json::Value::from(entry.spec_kitty_version.clone()),
                );
                serde_json::Value::Object(object)
            })
            .collect();

        let mut document = serde_json::Map::new();
        document.insert(
            "entries".to_string(),
            serde_json::Value::Array(entry_values),
        );
        document.insert(
            "schema_version".to_string(),
            serde_json::Value::from(u64::from(SCHEMA_VERSION)),
        );
        let mut bytes = serde_json::to_vec_pretty(&serde_json::Value::Object(document))
            .expect("manifest serialization cannot fail");
        bytes.push(b'\n');
        bytes
    }

    /// Load the manifest for `project_root`, or `Ok(None)` when it is absent.
    pub fn load(project_root: &Path) -> Result<Option<Self>, InstallError> {
        let path = project_root.join(MANIFEST_REL);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                Err(InstallError::ManifestNotRegularFile { path })
            }
            Ok(metadata) if !metadata.file_type().is_file() => {
                Err(InstallError::ManifestNotRegularFile { path })
            }
            Ok(_) => {
                let raw = sk_kernel::read_bounded(&path, 8 * 1024 * 1024).map_err(|error| {
                    InstallError::io("read", &path, std::io::Error::other(error))
                })?;
                Self::from_bytes(&path, &raw).map(Some)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(InstallError::io("inspect", &path, source)),
        }
    }

    /// Parse and validate manifest bytes read from `path`.
    pub fn from_bytes(path: &Path, raw: &[u8]) -> Result<Self, InstallError> {
        let text = std::str::from_utf8(raw).map_err(|error| InstallError::ManifestCorrupt {
            path: path.to_path_buf(),
            detail: error.to_string(),
        })?;
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|error| InstallError::ManifestCorrupt {
                path: path.to_path_buf(),
                detail: error.to_string(),
            })?;
        let object = value
            .as_object()
            .ok_or_else(|| InstallError::ManifestCorrupt {
                path: path.to_path_buf(),
                detail: "manifest root must be a JSON object".to_string(),
            })?;

        // Unknown future top-level fields are tolerated and dropped on the next
        // save (upstream forward-compatibility rule).
        let version = object.get("schema_version").ok_or_else(|| {
            InstallError::ManifestUnsupportedVersion {
                path: path.to_path_buf(),
                found: "missing".to_string(),
            }
        })?;
        match version.as_u64() {
            Some(found) if found == u64::from(SCHEMA_VERSION) => {}
            other => {
                return Err(InstallError::ManifestUnsupportedVersion {
                    path: path.to_path_buf(),
                    found: other.map_or_else(|| version.to_string(), |value| value.to_string()),
                });
            }
        }

        let raw_entries = object
            .get("entries")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| InstallError::ManifestInvalidEntry {
                path: path.to_path_buf(),
                detail: "'entries' must be an array".to_string(),
            })?;

        let mut manifest = Self::empty();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for raw_entry in raw_entries {
            let entry = parse_entry(path, raw_entry)?;
            if !seen.insert(entry.path.clone()) {
                return Err(InstallError::ManifestDuplicatePath {
                    path: path.to_path_buf(),
                    entry: entry.path,
                });
            }
            manifest.entries.push(entry);
        }
        manifest
            .entries
            .sort_by(|left, right| left.path.cmp(&right.path));
        Ok(manifest)
    }
}

fn parse_entry(
    manifest_path: &Path,
    value: &serde_json::Value,
) -> Result<ManifestEntry, InstallError> {
    let object = value
        .as_object()
        .ok_or_else(|| InstallError::ManifestInvalidEntry {
            path: manifest_path.to_path_buf(),
            detail: "each entry must be an object".to_string(),
        })?;
    let required_string = |key: &str| -> Result<String, InstallError> {
        object
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| InstallError::ManifestInvalidEntry {
                path: manifest_path.to_path_buf(),
                detail: format!("entry field {key:?} must be a string"),
            })
    };
    let path = required_string("path")?;
    let content_hash = required_string("content_hash")?;
    let installed_at = required_string("installed_at")?;
    let spec_kitty_version = required_string("spec_kitty_version")?;
    let raw_agents = object
        .get("agents")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| InstallError::ManifestInvalidEntry {
            path: manifest_path.to_path_buf(),
            detail: "entry field 'agents' must be an array".to_string(),
        })?;
    let mut agents = Vec::with_capacity(raw_agents.len());
    for raw_agent in raw_agents {
        agents.push(
            raw_agent
                .as_str()
                .ok_or_else(|| InstallError::ManifestInvalidEntry {
                    path: manifest_path.to_path_buf(),
                    detail: "every 'agents' element must be a string".to_string(),
                })?
                .to_string(),
        );
    }

    // Normalization is shared with `ManifestEntry::new`; errors are relocated
    // to the manifest file so diagnostics name the document, not a constant.
    ManifestEntry::new(path, content_hash, agents, installed_at, spec_kitty_version).map_err(
        |error| match error {
            InstallError::ManifestInvalidEntry { detail, .. } => {
                InstallError::ManifestInvalidEntry {
                    path: manifest_path.to_path_buf(),
                    detail,
                }
            }
            other => other,
        },
    )
}

fn normalize_agents(
    path: &str,
    agents: impl IntoIterator<Item = String>,
) -> Result<Vec<String>, InstallError> {
    let mut unique: BTreeSet<String> = BTreeSet::new();
    for agent in agents {
        if agent.trim().is_empty() {
            return Err(InstallError::ManifestInvalidEntry {
                path: PathBuf::from(MANIFEST_REL),
                detail: format!("entry {path:?} has an empty agent key"),
            });
        }
        if agent_surface(&agent).is_err() {
            return Err(InstallError::ManifestInvalidEntry {
                path: PathBuf::from(MANIFEST_REL),
                detail: format!("entry {path:?} names unknown agent {agent:?}"),
            });
        }
        unique.insert(agent);
    }
    if unique.is_empty() {
        return Err(InstallError::ManifestInvalidEntry {
            path: PathBuf::from(MANIFEST_REL),
            detail: format!("entry {path:?} has no owning agent"),
        });
    }
    Ok(unique.into_iter().collect())
}

fn is_lower_hex_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Validate a project-relative POSIX path for use as a manifest key.
pub fn validate_relative_posix(path: &str, context: &str) -> Result<(), InstallError> {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains('\0')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || path.starts_with('-')
    {
        return Err(InstallError::UnsafePath {
            path: format!("{context}: {path}"),
        });
    }
    Ok(())
}

/// A parsed, validated manifest entry pair used by drift/verify reporting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DriftState {
    /// The file is absent though the manifest owns it (a gap).
    Gap,
    /// The file is present with different bytes (drift).
    Drifted,
    /// The file matches the recorded ownership hash.
    Intact,
}
