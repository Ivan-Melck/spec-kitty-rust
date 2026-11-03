//! Bounded, read-only inspection of Spec Kitty charter, pack and asset state.
//!
//! This crate owns the read side of the charter domain for the Rust
//! implementation: the typed artifact-kind taxonomy, Charter Pack tier
//! precedence, `.kittify/config.yaml` org-pack declarations, the
//! `.kittify/charter/` bundle, `pack.yaml` descriptors, and fail-closed asset
//! sidecar resolution.
//!
//! Contract mirrors (Python reference, never invoked at runtime):
//!
//! - `src/charter/offering/artifact_kinds.py`
//! - `src/charter/offering/pack_paths.py`
//! - `src/charter/offering/assets/{models,repository}.py`
//! - `src/charter/offering/packs/pack_descriptor.py`
//! - `src/kernel/pack_tiers.py`, `src/kernel/paths.py`
//!
//! Boundaries:
//!
//! - No activation writes, no ledger append, no pack fetching, no synthesis.
//! - User-authored YAML/Markdown is never reserialized.
//! - Every read is bounded by [`Limits`].
//! - Every user/pack-supplied path is contained lexically and against symlinks.
//! - Missing or ambiguous state is an error, never a silent empty result.
//!
//! Containment and bounded regular-file reads always use `sk-kernel`;
//! disabling default Cargo features cannot weaken the safety policy.

mod asset;
mod charter;
mod config;
mod error;
mod inspect;
mod io;
mod kind;
mod limits;
mod pack;
mod tier;

pub use asset::{
    AssetCatalog, AssetEntry, AssetManifest, AssetRoots, OrgAssetRoot, ResolvedAsset, ShadowedAsset,
};
pub use charter::{
    ACTIVATION_LEDGER, BundleFile, CHARTER_MD, CHARTER_YAML, CharterBundle, SYNTHESIS_MANIFEST,
};
pub use config::{
    OrgPackConfig, PathExpansion, ProjectConfig, load_org_pack_registry, load_project_config,
};
pub use error::CharterError;
pub use inspect::{CharterInspection, InspectionOptions, inspect_project};
pub use kind::{ArtifactKind, CHARTER_KIND_TOKENS, CharterKind, MISSION_TYPE_TOKEN};
pub use limits::Limits;
pub use pack::{
    BuiltInSearch, OrgPackRoot, PACK_DESCRIPTOR, PackDescriptor, PackRoots, load_pack_descriptor,
};
pub use tier::PackTier;
