//! Fail-closed asset-manifest discovery and blob resolution.
//!
//! Mirrors `src/charter/offering/assets/repository.py`. Sidecar manifests
//! (`*.asset.yaml`) are loaded across built-in, org and project tiers. The
//! built-in anchor is the **parent** of the shipped `assets/` directory because
//! authored built-in paths carry the pack-relative `assets/` segment; org and
//! project anchors are the overlay's own `assets/` directory.
//!
//! Cross-tier precedence is explicit (built-in < org in config order < project)
//! and every shadowed entry is recorded. Duplicate ids at the *same* precedence
//! are ambiguous and fail closed. Blob paths are contained with the shared
//! primitive, so `..`, absolute and symlink escapes are refused.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::CharterError;
use crate::io::{FieldValue, field, parse_yaml_mapping, read_bounded, resolve_contained};
use crate::limits::Limits;
use crate::tier::PackTier;

/// One validated `*.asset.yaml` sidecar.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AssetManifest {
    /// Stable asset identifier.
    pub id: String,
    /// Declared MIME type.
    pub mime: String,
    /// Blob path relative to the tier-correct anchor.
    pub path: String,
    /// Optional display title.
    pub title: Option<String>,
}

/// One configured org asset root.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrgAssetRoot {
    /// Pack name (used as the provenance source label).
    pub name: String,
    /// The pack's `assets/` directory.
    pub assets_dir: PathBuf,
}

/// The asset roots to load, one per tier.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AssetRoots {
    /// Built-in `assets/` directory (`<built-in>/assets`).
    pub built_in: Option<PathBuf>,
    /// Org `assets/` directories in config order (later overrides earlier).
    pub organizations: Vec<OrgAssetRoot>,
    /// Project `assets/` directory.
    pub project: Option<PathBuf>,
}

/// One resolved asset entry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AssetEntry {
    /// Asset identifier.
    pub id: String,
    /// Declared MIME type.
    pub mime: String,
    /// Authored blob path.
    pub path: String,
    /// Optional display title.
    pub title: Option<String>,
    /// Winning tier.
    pub tier: PackTier,
    /// Provenance label (`built-in`, the org pack name, or `project`).
    pub source: String,
    /// Sidecar manifest that declared the winning entry.
    pub manifest_path: PathBuf,
    /// Containment anchor for the authored blob path.
    #[serde(skip)]
    pub anchor: PathBuf,
}

/// An entry that lost to a higher-precedence tier.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ShadowedAsset {
    /// Asset identifier.
    pub id: String,
    /// Tier that was overridden.
    pub tier: PackTier,
    /// Provenance label of the overridden entry.
    pub source: String,
    /// Sidecar manifest that declared the overridden entry.
    pub manifest_path: PathBuf,
}

/// A resolved, contained blob path.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ResolvedAsset {
    /// Asset identifier.
    pub id: String,
    /// Declared MIME type.
    pub mime: String,
    /// Authored blob path.
    pub authored_path: String,
    /// Contained absolute path (may not exist).
    pub path: PathBuf,
    /// Winning sidecar manifest.
    pub manifest_path: PathBuf,
    /// Winning tier.
    pub tier: PackTier,
    /// Whether the blob exists on disk. Inspection never opens the blob.
    pub exists: bool,
}

/// The loaded asset catalog.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct AssetCatalog {
    /// Winning entries, keyed and sorted by id.
    pub entries: BTreeMap<String, AssetEntry>,
    /// Entries shadowed by a higher-precedence tier, in override order.
    pub shadowed: Vec<ShadowedAsset>,
}

impl AssetCatalog {
    /// Load every sidecar under the supplied roots, fail-closed.
    pub fn load(roots: &AssetRoots, limits: &Limits) -> Result<Self, CharterError> {
        let mut catalog = Self::default();
        let mut count = 0_usize;

        if let Some(assets_dir) = &roots.built_in {
            if assets_dir.is_dir() {
                let anchor = assets_dir
                    .parent()
                    .ok_or_else(|| CharterError::RootNotDirectory {
                        root: assets_dir.clone(),
                    })?;
                catalog.load_layer(
                    PackTier::BuiltIn,
                    assets_dir,
                    anchor,
                    "built-in",
                    limits,
                    &mut count,
                )?;
            }
        }
        for org in &roots.organizations {
            if org.assets_dir.is_dir() {
                catalog.load_layer(
                    PackTier::Org,
                    &org.assets_dir,
                    &org.assets_dir,
                    &org.name,
                    limits,
                    &mut count,
                )?;
            }
        }
        if let Some(assets_dir) = &roots.project {
            if assets_dir.is_dir() {
                catalog.load_layer(
                    PackTier::Project,
                    assets_dir,
                    assets_dir,
                    "project",
                    limits,
                    &mut count,
                )?;
            }
        }
        Ok(catalog)
    }

    /// Iterate winning entries in id order.
    pub fn entries(&self) -> impl Iterator<Item = &AssetEntry> {
        self.entries.values()
    }

    /// Entries shadowed by higher-precedence tiers.
    pub fn shadowed(&self) -> &[ShadowedAsset] {
        &self.shadowed
    }

    /// Look up an entry without resolving its blob.
    pub fn get(&self, id: &str) -> Option<&AssetEntry> {
        self.entries.get(id)
    }

    /// Resolve an asset id to a contained blob path.
    pub fn resolve(&self, id: &str) -> Result<ResolvedAsset, CharterError> {
        let entry = self
            .entries
            .get(id)
            .ok_or_else(|| CharterError::AssetNotFound { id: id.to_owned() })?;
        let path = resolve_contained(&entry.anchor, &entry.path).map_err(|error| match error {
            CharterError::AbsolutePath { .. }
            | CharterError::ParentTraversal { .. }
            | CharterError::NulInPath { .. }
            | CharterError::EmptyPath { .. }
            | CharterError::PathEscape { .. }
            | CharterError::RootNotDirectory { .. } => CharterError::AssetPathEscape {
                id: entry.id.clone(),
                blob_path: entry.path.clone(),
                root: entry.anchor.clone(),
            },
            other => other,
        })?;
        let exists = path.is_file();
        Ok(ResolvedAsset {
            id: entry.id.clone(),
            mime: entry.mime.clone(),
            authored_path: entry.path.clone(),
            path,
            manifest_path: entry.manifest_path.clone(),
            tier: entry.tier,
            exists,
        })
    }

    fn load_layer(
        &mut self,
        tier: PackTier,
        scan_dir: &Path,
        anchor: &Path,
        source: &str,
        limits: &Limits,
        count: &mut usize,
    ) -> Result<(), CharterError> {
        // Prove the assets directory is still inside its owning pack before
        // adopting it as a blob anchor. Canonicalizing the anchor alone would
        // bless a symlink that redirects the whole catalog outside the pack.
        let parent = scan_dir
            .parent()
            .ok_or_else(|| CharterError::RootNotDirectory {
                root: scan_dir.to_path_buf(),
            })?;
        let name = scan_dir
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| CharterError::ContainmentRefused {
                path: scan_dir.to_path_buf(),
                reason: "asset directory name is not UTF-8".to_owned(),
            })?;
        resolve_contained(parent, name)?;
        let mut manifests = Vec::new();
        collect_manifests(scan_dir, limits, &mut manifests)?;
        manifests.sort();

        for manifest_path in manifests {
            *count += 1;
            if *count > limits.max_asset_manifests {
                return Err(CharterError::TooManyAssets {
                    limit: limits.max_asset_manifests,
                });
            }
            let manifest = load_manifest(&manifest_path, limits, &self.entries)?;
            let entry = AssetEntry {
                id: manifest.id.clone(),
                mime: manifest.mime,
                path: manifest.path,
                title: manifest.title,
                tier,
                source: source.to_owned(),
                manifest_path: manifest_path.clone(),
                anchor: anchor.to_path_buf(),
            };

            match self.entries.get(&entry.id) {
                None => {
                    self.entries.insert(entry.id.clone(), entry);
                }
                Some(existing) if existing.tier == tier && existing.source == source => {
                    return Err(CharterError::AmbiguousAsset {
                        id: entry.id.clone(),
                        sources: vec![existing.manifest_path.clone(), manifest_path],
                    });
                }
                Some(existing) => {
                    self.shadowed.push(ShadowedAsset {
                        id: existing.id.clone(),
                        tier: existing.tier,
                        source: existing.source.clone(),
                        manifest_path: existing.manifest_path.clone(),
                    });
                    self.entries.insert(entry.id.clone(), entry);
                }
            }
        }
        Ok(())
    }
}

fn collect_manifests(
    dir: &Path,
    limits: &Limits,
    manifests: &mut Vec<PathBuf>,
) -> Result<(), CharterError> {
    let mut stack = vec![(dir.to_path_buf(), 0_usize)];
    while let Some((current, depth)) = stack.pop() {
        // Bound walk depth so a pathological tree cannot recurse forever.
        if depth > 32 {
            return Err(CharterError::TooManyAssets {
                limit: limits.max_asset_manifests,
            });
        }
        let entries = fs::read_dir(&current).map_err(|source| CharterError::Io {
            path: current.clone(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| CharterError::Io {
                path: current.clone(),
                source,
            })?;
            let file_type = entry.file_type().map_err(|source| CharterError::Io {
                path: entry.path(),
                source,
            })?;
            // Never follow a symlink during discovery. A symlinked sidecar is
            // reported rather than silently skipped, so a hidden manifest cannot
            // change the catalog without an error.
            if file_type.is_symlink() {
                if entry.file_name().to_string_lossy().ends_with(".asset.yaml") {
                    return Err(invalid_manifest(
                        &entry.path(),
                        "symlinked asset manifests are not followed",
                    ));
                }
                continue;
            }
            if file_type.is_dir() {
                stack.push((entry.path(), depth + 1));
            } else if file_type.is_file()
                && entry.file_name().to_string_lossy().ends_with(".asset.yaml")
            {
                manifests.push(entry.path());
                if manifests.len() > limits.max_asset_manifests {
                    return Err(CharterError::TooManyAssets {
                        limit: limits.max_asset_manifests,
                    });
                }
            }
        }
    }
    Ok(())
}

fn load_manifest(
    path: &Path,
    limits: &Limits,
    inherited: &BTreeMap<String, AssetEntry>,
) -> Result<AssetManifest, CharterError> {
    let bytes = read_bounded(path, limits.max_asset_manifest_bytes)?;
    let mapping = parse_yaml_mapping(path, &bytes)?;

    const FIELDS: [&str; 4] = ["id", "mime", "path", "title"];
    for key in mapping.keys() {
        let Some(key) = key.as_str() else {
            return Err(invalid_manifest(path, "field names must be strings"));
        };
        if !FIELDS.contains(&key) {
            return Err(invalid_manifest(path, &format!("unknown field {key:?}")));
        }
    }

    let id = require_non_blank(&mapping, "id", path)?;
    let previous = inherited.get(&id);
    let mime = match (field(&mapping, "mime"), previous) {
        (FieldValue::Absent, Some(previous)) => previous.mime.clone(),
        _ => require_non_blank(&mapping, "mime", path)?,
    };
    let blob_path = match (field(&mapping, "path"), previous) {
        (FieldValue::Absent, Some(previous)) => previous.path.clone(),
        _ => require_non_blank(&mapping, "path", path)?,
    };
    let title = match field(&mapping, "title") {
        FieldValue::Absent => previous.and_then(|entry| entry.title.clone()),
        FieldValue::Null => None,
        FieldValue::String(value) => Some(value.to_owned()),
        FieldValue::Other(_) => return Err(invalid_manifest(path, "`title` must be a string")),
    };

    Ok(AssetManifest {
        id,
        mime,
        path: blob_path,
        title,
    })
}

fn require_non_blank(
    mapping: &serde_yaml_ng::Mapping,
    key: &str,
    path: &Path,
) -> Result<String, CharterError> {
    match field(mapping, key) {
        FieldValue::String(value) if !value.trim().is_empty() => Ok(value.to_owned()),
        FieldValue::Absent | FieldValue::Null => Err(invalid_manifest(
            path,
            &format!("`{key}` is required and must be a non-empty string"),
        )),
        FieldValue::String(_) => Err(invalid_manifest(
            path,
            &format!("`{key}` must not be blank"),
        )),
        FieldValue::Other(_) => Err(invalid_manifest(path, &format!("`{key}` must be a string"))),
    }
}

fn invalid_manifest(path: &Path, reason: &str) -> CharterError {
    CharterError::InvalidAssetManifest {
        path: path.to_path_buf(),
        reason: reason.to_owned(),
    }
}
