//! Bounded inspection of Spec Kitty charter, pack and asset state, plus the
//! deterministic byte-level charter authorities the writers share.
//!
//! This crate owns the charter domain for the Rust implementation: the typed
//! artifact-kind taxonomy, Charter Pack tier precedence, `.kittify/config.yaml`
//! org-pack declarations, the `.kittify/charter/` bundle, `pack.yaml`
//! descriptors, fail-closed asset sidecar resolution, and the five deterministic
//! authorities every charter writer must route through:
//!
//! - [`canonical_yaml`] — the one YAML serialization authority (manifest and
//!   sidecar hashes, and the manifests themselves);
//! - [`hash_content`] / [`compute_bundle_content_hash`] — the one bundle
//!   content-identity recipe (`charter/bundle.py`, `charter/hasher.py`);
//! - [`provenance::to_portable_source_path`] — the one `source_path` normalizer
//!   (`charter/offering/provenance.py`);
//! - [`synthesis::SynthesisManifest`] — the synthesis-manifest model, its
//!   canonical bytes and its `manifest_hash` finalizer;
//! - [`bundle_restamp::restamp_bundle_hash`] — the one manifest bundle-hash
//!   re-stamp every activation write ends with (`bundle_restamp.py`).
//!
//! The run-wide charter synthesis lock lives beside the activation authority in
//! `sk-doctrine` (`sk_doctrine::acquire_synthesis_lock`), which is the one
//! cross-language lock owner; this crate does not fork it.
//!
//! Contract mirrors (Python reference, never invoked at runtime):
//!
//! - `src/charter/offering/artifact_kinds.py`
//! - `src/charter/offering/pack_paths.py`
//! - `src/charter/offering/provenance.py`
//! - `src/charter/offering/assets/{models,repository}.py`
//! - `src/charter/offering/packs/pack_descriptor.py`
//! - `src/charter/activation/synthesizer/{artifact_naming,provenance,manifest}.py`
//! - `src/charter/bundle.py`, `src/charter/hasher.py`
//! - `src/kernel/pack_tiers.py`, `src/kernel/paths.py`, `src/kernel/charter_pack_paths.py`
//!
//! Boundaries:
//!
//! - No activation writes, no ledger append, no pack fetching, no synthesis run.
//!   This crate spells the *rules* the writers apply; the writers themselves
//!   live in their owning crate.
//! - User-authored YAML/Markdown is never reserialized by the read surface.
//!   [`canonical_yaml`] is the one serialization authority, and only the
//!   Source's own `canonical_yaml` call sites (manifest/sidecar hashes) use it.
//! - Every read is bounded by [`Limits`].
//! - Every user/pack-supplied path is contained lexically and against symlinks.
//! - Missing or ambiguous state is an error, never a silent empty result.
//!
//! Containment and bounded regular-file reads always use `sk-kernel`;
//! disabling default Cargo features cannot weaken the safety policy.

mod asset;
pub mod bundle_compat;
pub mod bundle_manifest;
pub mod bundle_restamp;
mod canonical_yaml;
mod charter;
mod config;
mod error;
mod hash;
mod inspect;
mod io;
mod kind;
mod limits;
mod pack;
pub mod presets;
pub mod provenance;
pub mod synthesis;
pub mod template_catalog;
mod tier;

pub use asset::{
    AssetCatalog, AssetEntry, AssetManifest, AssetRoots, OrgAssetRoot, ResolvedAsset, ShadowedAsset,
};
pub use bundle_manifest::{CANONICAL_BUNDLE_MANIFEST, CanonicalBundleManifest};
pub use canonical_yaml::{
    canonical_yaml, canonical_yaml_sha256, canonical_yaml_sha256_without, ordered_yaml,
    ordered_yaml_string_lists, ordered_yaml_width, ordered_yaml_width_normalized,
};
pub use charter::{
    ACTIVATION_LEDGER, BundleFile, CHARTER_MD, CHARTER_YAML, CharterBundle, SYNTHESIS_MANIFEST,
};
pub use config::{
    OrgPackConfig, PathExpansion, ProjectConfig, load_org_pack_registry, load_project_config,
};
pub use error::CharterError;
pub use hash::{
    BUNDLE_CONTENT_HASH_FILES, BUNDLE_OPTIONAL_CONTENT_HASH_FILES, bundle_content_hash_of,
    compute_bundle_content_hash, hash_content,
};
pub use inspect::{CharterInspection, InspectionOptions, inspect_project};
pub use kind::{ArtifactKind, CHARTER_KIND_TOKENS, CharterKind, MISSION_TYPE_TOKEN};
pub use limits::Limits;
pub use pack::{
    BuiltInSearch, OrgPackRoot, PACK_DESCRIPTOR, PackDescriptor, PackRoots, load_pack_descriptor,
    resolve_source_project_layer,
};
pub use tier::PackTier;
