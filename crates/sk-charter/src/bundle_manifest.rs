//! Canonical charter-bundle inventory from pinned `charter.bundle`.
//!
//! This is the fixed canonical data, not a replacement schema/parser for arbitrary
//! user manifests. Every consumer of this inventory borrows the same declaration.
use crate::{ACTIVATION_LEDGER, CHARTER_MD, CHARTER_YAML};

/// The immutable canonical bundle inventory (Source schema version 2.0.0).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CanonicalBundleManifest {
    /// Manifest schema version, distinct from the application version.
    pub schema_version: &'static str,
    /// Required tracked/authored paths, relative to the repository root.
    pub tracked_files: &'static [&'static str],
    /// Required derived paths; empty after the Source prose-extraction retirement.
    pub derived_files: &'static [&'static str],
    /// Derived-path/source-path associations.
    pub derivation_sources: &'static [(&'static str, &'static str)],
    /// Content-hash members (not the derived-file inventory).
    pub content_hash_files: &'static [&'static str],
    /// Gitignore entries required by this manifest.
    pub gitignore_required_entries: &'static [&'static str],
    /// Optional tracked paths whose absence is not a bundle failure.
    pub optional_tracked_files: &'static [&'static str],
}

/// The ONE Native declaration of Source `CANONICAL_MANIFEST` data.
pub const CANONICAL_BUNDLE_MANIFEST: CanonicalBundleManifest = CanonicalBundleManifest {
    schema_version: "2.0.0",
    tracked_files: &[CHARTER_MD, CHARTER_YAML],
    derived_files: &[],
    derivation_sources: &[],
    content_hash_files: &[CHARTER_YAML],
    gitignore_required_entries: &[],
    optional_tracked_files: &[ACTIVATION_LEDGER],
};
