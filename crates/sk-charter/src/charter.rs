//! Bounded, read-only inspection of the `.kittify/charter/` bundle.
//!
//! The bundle is presence + size + digest for the known companion files, plus a
//! YAML well-formedness check for the machine-readable ones. The curated
//! `charter.md` is never parsed, and no file is ever written.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::CharterError;
use crate::io::{parse_yaml_mapping, read_bounded, resolve_contained, sha256_hex};
use crate::limits::Limits;

/// Relative path of the curated charter prose.
pub const CHARTER_MD: &str = ".kittify/charter/charter.md";
/// Relative path of the compiled charter YAML.
pub const CHARTER_YAML: &str = ".kittify/charter/charter.yaml";
/// Relative path of the activation ledger.
pub const ACTIVATION_LEDGER: &str = ".kittify/charter/activation.yaml";
/// Relative path of the synthesis manifest.
pub const SYNTHESIS_MANIFEST: &str = ".kittify/charter/synthesis-manifest.yaml";

const BUNDLE_DIR: &str = ".kittify/charter";

/// One inspected bundle file.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BundleFile {
    /// Absolute on-disk path (symlinks resolved after containment).
    pub path: PathBuf,
    /// Project-relative path using forward slashes.
    pub relative_path: String,
    /// Byte length that was read.
    pub size: u64,
    /// Hex-encoded SHA-256 of the bytes.
    pub sha256: String,
}

/// The inspected `.kittify/charter/` bundle.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct CharterBundle {
    /// Curated `charter.md`, when present.
    pub charter_md: Option<BundleFile>,
    /// Compiled `charter.yaml`, when present.
    pub charter_yaml: Option<BundleFile>,
    /// `activation.yaml` ledger, when present.
    pub activation_ledger: Option<BundleFile>,
    /// `synthesis-manifest.yaml`, when present.
    pub synthesis_manifest: Option<BundleFile>,
}

impl CharterBundle {
    /// Inspect the bundle, fail-closed.
    pub fn load(project_root: &Path, limits: &Limits) -> Result<Self, CharterError> {
        let bundle_dir = project_root.join(BUNDLE_DIR);
        if !bundle_dir.exists() {
            return Ok(Self::default());
        }
        // Containing the bundle directory itself under the project root catches
        // a `.kittify/charter` symlink that redirects outside the project.
        let bundle_dir = resolve_contained(project_root, BUNDLE_DIR)?;

        Ok(Self {
            charter_md: inspect_file(&bundle_dir, "charter.md", false, limits)?,
            charter_yaml: inspect_file(&bundle_dir, "charter.yaml", true, limits)?,
            activation_ledger: inspect_file(&bundle_dir, "activation.yaml", true, limits)?,
            synthesis_manifest: inspect_file(&bundle_dir, "synthesis-manifest.yaml", true, limits)?,
        })
    }

    /// Whether any known bundle file exists.
    pub fn is_present(&self) -> bool {
        self.files().next().is_some()
    }

    /// Iterate the present bundle files in declaration order.
    pub fn files(&self) -> impl Iterator<Item = &BundleFile> {
        [
            self.charter_md.as_ref(),
            self.charter_yaml.as_ref(),
            self.activation_ledger.as_ref(),
            self.synthesis_manifest.as_ref(),
        ]
        .into_iter()
        .flatten()
    }
}

fn inspect_file(
    bundle_dir: &Path,
    file_name: &str,
    yaml: bool,
    limits: &Limits,
) -> Result<Option<BundleFile>, CharterError> {
    let lexical = bundle_dir.join(file_name);
    if !lexical.exists() {
        return Ok(None);
    }
    let resolved = resolve_contained(bundle_dir, file_name)?;
    let bytes = read_bounded(&resolved, limits.max_charter_file_bytes)?;
    if yaml {
        parse_yaml_mapping(&resolved, &bytes)?;
    }
    Ok(Some(BundleFile {
        path: resolved,
        relative_path: format!("{BUNDLE_DIR}/{file_name}"),
        size: bytes.len() as u64,
        sha256: sha256_hex(&bytes),
    }))
}
