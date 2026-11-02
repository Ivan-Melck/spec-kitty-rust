//! End-to-end, read-only charter inspection.
//!
//! [`inspect_project`] combines config, charter bundle, pack roots, descriptors
//! and the asset catalog into one serde-serializable value for a CLI. It never
//! writes, fetches, activates or falls back to a simulated result.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::asset::{AssetCatalog, AssetRoots, OrgAssetRoot};
use crate::charter::CharterBundle;
use crate::config::{PathExpansion, ProjectConfig, load_project_config};
use crate::error::CharterError;
use crate::limits::Limits;
use crate::pack::{BuiltInSearch, PackDescriptor, PackRoots, load_pack_descriptor};
use crate::tier::PackTier;

/// Options for [`inspect_project`].
#[derive(Clone, Debug)]
pub struct InspectionOptions {
    /// Read budgets.
    pub limits: Limits,
    /// Built-in root search; `None` skips the built-in tier.
    pub built_in: Option<BuiltInSearch>,
    /// Expansion used for org `local_path` values.
    pub path_expansion: PathExpansion,
}

impl Default for InspectionOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl InspectionOptions {
    /// Options with default budgets, no built-in search, and process-env expansion.
    pub fn new() -> Self {
        Self {
            limits: Limits::default(),
            built_in: None,
            path_expansion: PathExpansion::from_env(),
        }
    }

    /// Search for the built-in tier.
    pub fn with_built_in(mut self, search: BuiltInSearch) -> Self {
        self.built_in = Some(search);
        self
    }

    /// Replace the read budgets.
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// Replace the org `local_path` expansion.
    pub fn with_path_expansion(mut self, expansion: PathExpansion) -> Self {
        self.path_expansion = expansion;
        self
    }
}

/// The complete, serde-serializable inspection result.
#[derive(Clone, Debug, Serialize)]
pub struct CharterInspection {
    /// Project root as supplied.
    pub project_root: PathBuf,
    /// `.kittify/config.yaml` (or its absence).
    pub config: ProjectConfig,
    /// `.kittify/charter/` bundle files.
    pub charter: CharterBundle,
    /// Resolved tier roots.
    pub roots: PackRoots,
    /// Pack descriptors in precedence order: built-in, org (config order), project.
    pub packs: Vec<PackDescriptor>,
    /// Asset catalog with explicit precedence and recorded shadows.
    pub assets: AssetCatalog,
}

/// Inspect one project, fail-closed.
///
/// A declared org pack with no root, a missing or invalid pack descriptor, a
/// corrupt YAML document and an ambiguity at one precedence all become typed
/// errors rather than silent omissions.
pub fn inspect_project(
    project_root: &Path,
    options: &InspectionOptions,
) -> Result<CharterInspection, CharterError> {
    if !project_root.is_dir() {
        return Err(CharterError::ProjectRootNotDirectory {
            root: project_root.to_path_buf(),
        });
    }

    let config = load_project_config(project_root, &options.limits)?;
    let charter = CharterBundle::load(project_root, &options.limits)?;
    let roots = PackRoots::resolve(
        project_root,
        &config,
        &options.path_expansion,
        options.built_in.as_ref(),
    )?;

    let mut packs = Vec::new();
    if let Some(root) = &roots.built_in {
        packs.push(load_pack_descriptor(
            PackTier::BuiltIn,
            root,
            &options.limits,
        )?);
    }
    for org in &roots.organizations {
        packs.push(load_pack_descriptor(
            PackTier::Org,
            &org.root,
            &options.limits,
        )?);
    }
    if let Some(root) = &roots.project {
        packs.push(load_pack_descriptor(
            PackTier::Project,
            root,
            &options.limits,
        )?);
    }

    let assets = AssetCatalog::load(&asset_roots(&roots), &options.limits)?;

    Ok(CharterInspection {
        project_root: project_root.to_path_buf(),
        config,
        charter,
        roots,
        packs,
        assets,
    })
}

fn asset_roots(roots: &PackRoots) -> AssetRoots {
    let mut asset_roots = AssetRoots::default();
    if let Some(built_in) = &roots.built_in {
        let assets = built_in.join("assets");
        if assets.is_dir() {
            asset_roots.built_in = Some(assets);
        }
    }
    for org in &roots.organizations {
        let assets = org.root.join("assets");
        if assets.is_dir() {
            asset_roots.organizations.push(OrgAssetRoot {
                name: org.name.clone(),
                assets_dir: assets,
            });
        }
    }
    if let Some(project) = &roots.project {
        let assets = project.join("assets");
        if assets.is_dir() {
            asset_roots.project = Some(assets);
        }
    }
    asset_roots
}
