//! Built-in root discovery, pack roots and `pack.yaml` descriptor inspection.
//!
//! Mirrors `src/charter/offering/pack_paths.py` and
//! `src/charter/offering/packs/pack_descriptor.py`. The built-in tier resolves
//! through `SPEC_KITTY_PACKS_ROOT` and a bounded ancestor walk; a pinned but
//! unsynced checkout is a distinct error. Every tier's descriptor is validated
//! and a declared tier that disagrees with its configured position fails closed.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Serialize;

use crate::config::{PathExpansion, ProjectConfig};
use crate::error::CharterError;
use crate::io::{FieldValue, field, is_ulid, parse_yaml_mapping, read_bounded, resolve_contained};
use crate::kind::ArtifactKind;
use crate::limits::Limits;
use crate::tier::PackTier;

/// The pack descriptor file name.
pub const PACK_DESCRIPTOR: &str = "pack.yaml";
const BUILT_IN_SIBLING: &str = "packs/built-in";
const BUILT_IN_PIN: &str = "packs/built-in.source.yaml";
const PROJECT_PACK_DIR: &str = ".kittify/charter-packs";

/// Locates the built-in pack tier.
///
/// Order: `SPEC_KITTY_PACKS_ROOT` + `built-in` when it exists, otherwise the
/// nearest ancestor of `anchor` containing `packs/built-in`, bounded by
/// `max_ancestor_hops`.
#[derive(Debug)]
pub struct BuiltInSearch {
    anchor: PathBuf,
    packs_root_env: Option<PathBuf>,
    warnings: Mutex<Vec<String>>,
    max_ancestor_hops: usize,
}

impl Clone for BuiltInSearch {
    fn clone(&self) -> Self {
        Self {
            anchor: self.anchor.clone(),
            packs_root_env: self.packs_root_env.clone(),
            warnings: Mutex::new(self.warnings()),
            max_ancestor_hops: self.max_ancestor_hops,
        }
    }
}

impl BuiltInSearch {
    /// Search from `anchor` without an environment override.
    pub fn new(anchor: impl Into<PathBuf>) -> Self {
        Self {
            anchor: anchor.into(),
            packs_root_env: None,
            warnings: Mutex::new(Vec::new()),
            max_ancestor_hops: Limits::default().max_ancestor_hops,
        }
    }

    /// Search using the process environment (`SPEC_KITTY_PACKS_ROOT`).
    pub fn from_env(anchor: impl Into<PathBuf>) -> Self {
        let packs_root_env = std::env::var_os("SPEC_KITTY_PACKS_ROOT").map(PathBuf::from);
        Self {
            packs_root_env,
            ..Self::new(anchor)
        }
    }

    /// Use an explicit `SPEC_KITTY_PACKS_ROOT` value (the parent of `built-in`).
    pub fn with_packs_root_env(mut self, value: impl Into<PathBuf>) -> Self {
        self.packs_root_env = Some(value.into());
        self
    }

    /// Bound the ancestor walk.
    pub fn with_max_ancestor_hops(mut self, hops: usize) -> Self {
        self.max_ancestor_hops = hops;
        self
    }

    /// Warnings emitted by the last [`Self::find_root`] call.
    pub fn warnings(&self) -> Vec<String> {
        self.warnings
            .lock()
            .map(|warnings| warnings.clone())
            .unwrap_or_default()
    }

    /// Resolve the built-in pack root, fail-closed.
    pub fn find_root(&self) -> Result<PathBuf, CharterError> {
        if let Ok(mut warnings) = self.warnings.lock() {
            warnings.clear();
        }
        if let Some(packs_root) = &self.packs_root_env {
            let candidate = packs_root.join("built-in");
            if candidate.is_dir() {
                return canonical(&candidate);
            }
            self.warn(format!(
                "SPEC_KITTY_PACKS_ROOT={:?} does not resolve to a directory (expected {}); \
                 ignoring the override and falling back to the installed built-in pack tree",
                packs_root,
                candidate.display()
            ));
        }

        for ancestor in self.anchor.ancestors().take(self.max_ancestor_hops + 1) {
            let candidate = ancestor.join(BUILT_IN_SIBLING);
            if candidate.is_dir() {
                if !candidate.join(PACK_DESCRIPTOR).is_file()
                    && ancestor.join(BUILT_IN_PIN).is_file()
                {
                    return Err(CharterError::BuiltInPackMissing);
                }
                return canonical(&candidate);
            }
        }

        for ancestor in self.anchor.ancestors().take(self.max_ancestor_hops + 1) {
            if ancestor.join(BUILT_IN_PIN).is_file() {
                return Err(CharterError::BuiltInPackMissing);
            }
        }
        Err(CharterError::PackRootNotFound {
            tier: PackTier::BuiltIn,
        })
    }

    fn warn(&self, warning: String) {
        if let Ok(mut warnings) = self.warnings.lock() {
            warnings.push(warning);
        }
    }
}

/// One resolved org pack root, in configured precedence order.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OrgPackRoot {
    /// Configured pack name.
    pub name: String,
    /// Canonical effective root.
    pub root: PathBuf,
}

/// Every resolved pack root for a project.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PackRoots {
    /// Built-in pack root, when searched for and found.
    pub built_in: Option<PathBuf>,
    /// Org pack roots in config declaration order (later overrides earlier).
    pub organizations: Vec<OrgPackRoot>,
    /// Project pack overlay root, when present.
    pub project: Option<PathBuf>,
    /// Non-fatal resolution diagnostics (a rejected `SPEC_KITTY_PACKS_ROOT`, for example).
    pub warnings: Vec<String>,
}

impl PackRoots {
    /// Resolve all tiers for `project_root`.
    pub fn resolve(
        project_root: &Path,
        config: &ProjectConfig,
        expansion: &PathExpansion,
        built_in: Option<&BuiltInSearch>,
    ) -> Result<Self, CharterError> {
        let (built_in_root, warnings) = match built_in {
            Some(search) => (Some(search.find_root()?), search.warnings()),
            None => (None, Vec::new()),
        };

        let mut organizations = Vec::with_capacity(config.org_packs.len());
        for pack in &config.org_packs {
            let base = pack.local_path_root(project_root, expansion)?;
            if !base.is_dir() {
                return Err(CharterError::OrgPackMissing {
                    name: pack.name.clone(),
                    path: config.path.clone(),
                    root: base,
                });
            }
            let root = pack.effective_root(project_root, expansion)?;
            if !root.is_dir() {
                return Err(CharterError::OrgPackMissing {
                    name: pack.name.clone(),
                    path: config.path.clone(),
                    root,
                });
            }
            organizations.push(OrgPackRoot {
                name: pack.name.clone(),
                root: canonical(&root)?,
            });
        }

        let project_lexical = project_root.join(PROJECT_PACK_DIR);
        let project = if project_lexical.is_dir() {
            Some(resolve_contained(project_root, PROJECT_PACK_DIR)?)
        } else {
            None
        };

        Ok(Self {
            built_in: built_in_root,
            organizations,
            project,
            warnings,
        })
    }

    /// Return the `packs/built-in/<plural>/` content directory for `kind`.
    pub fn built_in_content_dir(&self, kind: ArtifactKind) -> Result<PathBuf, CharterError> {
        if !kind.has_built_in_content_dir() {
            return Err(CharterError::BuiltInContentDirNotAvailable { kind });
        }
        let root = self
            .built_in
            .as_ref()
            .ok_or(CharterError::PackRootNotFound {
                tier: PackTier::BuiltIn,
            })?;
        Ok(root.join(kind.plural()))
    }
}

/// The inspected `pack.yaml` descriptor.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PackDescriptor {
    /// The configured tier this descriptor was loaded for.
    pub tier: PackTier,
    /// The pack root directory (as resolved by [`PackRoots`]).
    pub root: PathBuf,
    /// The `pack.yaml` path.
    pub path: PathBuf,
    /// Stable ULID identity, when present.
    pub pack_id: Option<String>,
    /// Authored pack version, required for the built-in tier.
    pub pack_version: Option<String>,
    /// Parent pack ULID for lineage, when present.
    pub parent_pack: Option<String>,
    /// Human-readable pack name.
    pub name: String,
    /// The effective (declared or inferred) tier.
    pub pack_level: PackTier,
    /// Whether `pack_level` was absent and inferred from the configured position.
    pub level_inferred: bool,
    /// Optional PEP 440 range of supported core versions (not evaluated here).
    pub requires_core: Option<String>,
    /// Optional pack-relative active-charter path.
    pub active_charter: Option<String>,
    /// Optional pack-relative DRG source path.
    pub pack_drg: Option<String>,
}

impl fmt::Display for PackDescriptor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} ({})", self.name, self.tier)
    }
}

/// Read and validate a pack's `pack.yaml`.
///
/// A missing file is [`CharterError::PackDescriptorMissing`]; corrupt YAML,
/// unknown fields, an invalid ULID and a level mismatch all fail closed.
pub fn load_pack_descriptor(
    tier: PackTier,
    pack_root: &Path,
    limits: &Limits,
) -> Result<PackDescriptor, CharterError> {
    let path = pack_root.join(PACK_DESCRIPTOR);
    if !path.is_file() {
        return Err(CharterError::PackDescriptorMissing {
            tier,
            root: pack_root.to_path_buf(),
        });
    }
    // Reject a symlinked descriptor that leaves the pack root. The lexical path
    // is kept in the result so `root`/`path` stay caller-stable.
    let resolved = resolve_contained(pack_root, PACK_DESCRIPTOR)?;
    let bytes = read_bounded(&resolved, limits.max_pack_descriptor_bytes)?;
    let mapping = parse_yaml_mapping(&path, &bytes)?;

    const FIELDS: [&str; 8] = [
        "pack_id",
        "pack_version",
        "parent_pack",
        "name",
        "pack_level",
        "requires_core",
        "active_charter",
        "pack_drg",
    ];
    for key in mapping.keys() {
        let Some(key) = key.as_str() else {
            return Err(invalid_descriptor(&path, "field names must be strings"));
        };
        if !FIELDS.contains(&key) {
            return Err(invalid_descriptor(&path, &format!("unknown field {key:?}")));
        }
    }

    let name = require_string(&mapping, "name", &path)?;
    if name.trim().is_empty() {
        return Err(invalid_descriptor(&path, "`name` must not be blank"));
    }
    let pack_id = optional_string(&mapping, "pack_id", &path)?;
    if let Some(value) = &pack_id {
        if !is_ulid(value) {
            return Err(CharterError::InvalidPackId {
                path: path.clone(),
                value: value.clone(),
            });
        }
    }
    let pack_version = optional_string(&mapping, "pack_version", &path)?;
    let parent_pack = optional_string(&mapping, "parent_pack", &path)?;
    let requires_core = optional_string(&mapping, "requires_core", &path)?;
    let active_charter = optional_string(&mapping, "active_charter", &path)?;
    let pack_drg = optional_string(&mapping, "pack_drg", &path)?;

    let declared = optional_string(&mapping, "pack_level", &path)?;
    let (pack_level, level_inferred) = match declared {
        Some(token) => {
            let declared = PackTier::from_token(&token)
                .map_err(|error| invalid_descriptor(&path, &error.to_string()))?;
            if declared != tier {
                return Err(CharterError::PackLevelMismatch {
                    path,
                    declared,
                    expected: tier,
                });
            }
            (declared, false)
        }
        None => (tier, true),
    };

    if pack_level == PackTier::BuiltIn && pack_version.is_none() {
        return Err(invalid_descriptor(
            &path,
            "`pack_version` is required when pack_level is 'built-in'",
        ));
    }

    Ok(PackDescriptor {
        tier,
        root: pack_root.to_path_buf(),
        path,
        pack_id,
        pack_version,
        parent_pack,
        name,
        pack_level,
        level_inferred,
        requires_core,
        active_charter,
        pack_drg,
    })
}

fn require_string(
    mapping: &serde_yaml_ng::Mapping,
    key: &str,
    path: &Path,
) -> Result<String, CharterError> {
    optional_string(mapping, key, path)?.ok_or_else(|| {
        invalid_descriptor(path, &format!("`{key}` is required and must be a string"))
    })
}

fn optional_string(
    mapping: &serde_yaml_ng::Mapping,
    key: &str,
    path: &Path,
) -> Result<Option<String>, CharterError> {
    match field(mapping, key) {
        FieldValue::Absent | FieldValue::Null => Ok(None),
        FieldValue::String(value) => Ok(Some(value.to_owned())),
        FieldValue::Other(_) => Err(invalid_descriptor(
            path,
            &format!("`{key}` must be a string"),
        )),
    }
}

fn invalid_descriptor(path: &Path, reason: &str) -> CharterError {
    CharterError::InvalidPackDescriptor {
        path: path.to_path_buf(),
        reason: reason.to_owned(),
    }
}

fn canonical(path: &Path) -> Result<PathBuf, CharterError> {
    std::fs::canonicalize(path).map_err(|source| CharterError::Io {
        path: path.to_path_buf(),
        source,
    })
}
