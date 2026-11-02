//! Read budgets for bounded inspection.
//!
//! Every file or directory walk is capped so a malformed or hostile tree cannot
//! make inspection unbounded. The defaults are generous for real charter packs
//! and small enough to keep a CLI responsive.

use serde::{Deserialize, Serialize};

/// Byte and entry budgets applied to every inspection read.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    /// Maximum bytes read from `.kittify/config.yaml`.
    pub max_config_bytes: u64,
    /// Maximum bytes read from any single `.kittify/charter/` file.
    pub max_charter_file_bytes: u64,
    /// Maximum bytes read from a pack `pack.yaml`.
    pub max_pack_descriptor_bytes: u64,
    /// Maximum bytes read from one `*.asset.yaml` sidecar.
    pub max_asset_manifest_bytes: u64,
    /// Maximum number of asset sidecars loaded across all tiers.
    pub max_asset_manifests: usize,
    /// Maximum ancestor hops walked while locating `packs/built-in`.
    pub max_ancestor_hops: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_config_bytes: 1024 * 1024,
            max_charter_file_bytes: 4 * 1024 * 1024,
            max_pack_descriptor_bytes: 256 * 1024,
            max_asset_manifest_bytes: 64 * 1024,
            max_asset_manifests: 4096,
            max_ancestor_hops: 64,
        }
    }
}

impl Limits {
    /// Replace the config byte budget.
    pub fn with_max_config_bytes(mut self, limit: u64) -> Self {
        self.max_config_bytes = limit;
        self
    }

    /// Replace the charter-file byte budget.
    pub fn with_max_charter_file_bytes(mut self, limit: u64) -> Self {
        self.max_charter_file_bytes = limit;
        self
    }

    /// Replace the pack-descriptor byte budget.
    pub fn with_max_pack_descriptor_bytes(mut self, limit: u64) -> Self {
        self.max_pack_descriptor_bytes = limit;
        self
    }

    /// Replace the asset-manifest byte budget.
    pub fn with_max_asset_manifest_bytes(mut self, limit: u64) -> Self {
        self.max_asset_manifest_bytes = limit;
        self
    }

    /// Replace the asset-manifest count budget.
    pub fn with_max_asset_manifests(mut self, limit: usize) -> Self {
        self.max_asset_manifests = limit;
        self
    }

    /// Replace the ancestor-walk budget.
    pub fn with_max_ancestor_hops(mut self, limit: usize) -> Self {
        self.max_ancestor_hops = limit;
        self
    }
}
