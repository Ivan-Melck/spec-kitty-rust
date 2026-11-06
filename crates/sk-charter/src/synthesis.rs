//! Project charter-pack layout, artifact naming and the synthesis-manifest model.
//!
//! Ports the deterministic pieces of the pinned Source that four charter
//! synthesis/cutover effects share:
//!
//! * `kernel/charter_pack_paths.py` — the project pack root and its graph file;
//! * `charter/activation/synthesizer/artifact_naming.py` — `NNN-<slug>` directive
//!   filenames and the per-kind project-pack subdirectory;
//! * `charter/activation/synthesizer/provenance.py` — the sidecar path shape;
//! * `charter/activation/synthesizer/manifest.py` — the synthesis manifest model,
//!   its canonical bytes, `compute_manifest_hash` / `finalize_manifest` and the
//!   legacy raw-field verify fallback.
//!
//! The manifest is the authoritative "this bundle was committed" record: it is
//! written last, and its `manifest_hash` is the SHA-256 of the canonical YAML of
//! every other field. [`SynthesisManifest::finalize`] is the one finalizer every
//! manifest-writing caller uses, so no field can be silently left out of the
//! hashed payload.
//!
//! Only the manifest's own bytes are ever re-serialised here; user-authored
//! charter YAML is not reformatted by this module.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_yaml_ng::{Mapping, Value};

use crate::canonical_yaml::canonical_yaml;
use crate::error::CharterError;

/// The project charter-pack root, relative to the repository root.
pub const PROJECT_PACK_ROOT: &str = ".kittify/charter-packs";
/// [`PROJECT_PACK_ROOT`] in POSIX form, for prefix checks and manifests.
pub const PROJECT_PACK_ROOT_POSIX: &str = PROJECT_PACK_ROOT;
/// The project overlay graph file inside the project pack root.
pub const PROJECT_GRAPH_FILENAME: &str = "graph.yaml";
/// The synthesis manifest, relative to the repository root.
pub const SYNTHESIS_MANIFEST_PATH: &str = ".kittify/charter/synthesis-manifest.yaml";
/// The provenance sidecar directory, relative to the repository root.
pub const PROVENANCE_DIR: &str = ".kittify/charter/provenance";

/// `<project pack root>/<parts...>` for `repo_root`.
pub fn project_pack_path(repo_root: &Path, parts: &[&str]) -> PathBuf {
    let mut path = repo_root.join(PROJECT_PACK_ROOT);
    for part in parts {
        path = path.join(part);
    }
    path
}

/// The numeric directive segment of `artifact_id` (`PROJECT_001` → `"001"`).
///
/// Mirrors `extract_directive_number`: scan for an all-uppercase prefix followed
/// by `_` and at least one digit; no match yields `"000"`. Deliberately
/// backtracking-free, like the Source, because artifact ids come from external
/// input and persisted manifests.
pub fn extract_directive_number(artifact_id: Option<&str>) -> String {
    let Some(artifact_id) = artifact_id else {
        return "000".to_string();
    };
    if artifact_id.is_empty() {
        return "000".to_string();
    }
    let bytes: Vec<char> = artifact_id.chars().collect();
    let length = bytes.len();
    let mut index = 0usize;
    while index < length {
        let start = index;
        while index < length && bytes[index].is_uppercase() {
            index += 1;
        }
        if index == start || index >= length || bytes[index] != '_' {
            index = start + 1;
            continue;
        }
        let digits_start = index + 1;
        let mut digits_end = digits_start;
        while digits_end < length && bytes[digits_end].is_ascii_digit() {
            digits_end += 1;
        }
        if digits_end > digits_start {
            let digits: String = bytes[digits_start..digits_end].iter().collect();
            return format!("{digits:0>3}");
        }
        index = digits_start;
    }
    "000".to_string()
}

/// The repository-glob-matching filename for an artifact.
pub fn artifact_filename(
    kind: &str,
    slug: &str,
    artifact_id: Option<&str>,
) -> Result<String, CharterError> {
    match kind {
        "directive" => Ok(format!(
            "{}-{slug}.directive.yaml",
            extract_directive_number(artifact_id)
        )),
        "tactic" => Ok(format!("{slug}.tactic.yaml")),
        "styleguide" => Ok(format!("{slug}.styleguide.yaml")),
        other => Err(CharterError::UnsupportedNode {
            path: PathBuf::from("<artifact>"),
            detail: format!("unknown artifact kind: {other:?}"),
        }),
    }
}

/// The project-pack subdirectory of an artifact kind (singular, as the
/// `.gitignore` whitelist spells it).
pub fn pack_kind_subdir(kind: &str) -> Result<&'static str, CharterError> {
    match kind {
        "directive" => Ok("directive"),
        "tactic" => Ok("tactic"),
        "styleguide" => Ok("styleguide"),
        other => Err(CharterError::UnsupportedNode {
            path: PathBuf::from("<artifact>"),
            detail: format!("unknown artifact kind: {other:?}"),
        }),
    }
}

/// `.kittify/charter/provenance/<kind>-<slug>.yaml`.
pub fn provenance_path_for(kind: &str, slug: &str) -> String {
    format!("{PROVENANCE_DIR}/{kind}-{slug}.yaml")
}

/// The synthesis manifest's artifact kinds (`ManifestArtifactEntry.kind`).
pub const MANIFEST_KINDS: [&str; 5] = [
    "directive",
    "tactic",
    "styleguide",
    "procedure",
    "agent_profile",
];

/// One synthesized artifact listed in the synthesis manifest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestArtifactEntry {
    /// `directive` / `tactic` / `styleguide` / `procedure` / `agent_profile`.
    pub kind: String,
    /// The artifact slug.
    pub slug: String,
    /// Repo-relative path to the artifact YAML under the project pack root.
    pub path: String,
    /// Repo-relative path to the provenance sidecar.
    pub provenance_path: String,
    /// SHA-256 hex digest of the artifact YAML bytes.
    pub content_hash: String,
}

impl ManifestArtifactEntry {
    fn to_value(&self) -> Value {
        let mut mapping = Mapping::new();
        insert_str(&mut mapping, "kind", &self.kind);
        insert_str(&mut mapping, "slug", &self.slug);
        insert_str(&mut mapping, "path", &self.path);
        insert_str(&mut mapping, "provenance_path", &self.provenance_path);
        insert_str(&mut mapping, "content_hash", &self.content_hash);
        Value::Mapping(mapping)
    }

    fn from_value(value: &Value) -> Result<Self, CharterError> {
        let mapping = require_mapping(value, "<artifact entry>")?;
        let kind = require_string(mapping, "kind", "<artifact entry>")?;
        if !MANIFEST_KINDS.contains(&kind.as_str()) {
            return Err(CharterError::UnsupportedNode {
                path: PathBuf::from("<artifact entry>"),
                detail: format!("unknown artifact kind {kind:?}"),
            });
        }
        Ok(Self {
            kind,
            slug: require_string(mapping, "slug", "<artifact entry>")?,
            path: require_string(mapping, "path", "<artifact entry>")?,
            provenance_path: require_string(mapping, "provenance_path", "<artifact entry>")?,
            content_hash: require_string(mapping, "content_hash", "<artifact entry>")?,
        })
    }
}

/// The synthesis manifest (schema 2 or 3).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SynthesisManifest {
    /// `"2"` or `"3"`; new writes default to `"3"`.
    pub schema_version: String,
    /// The Mission this bundle was synthesized for, when recorded.
    pub mission_id: Option<String>,
    /// ISO-8601 UTC creation stamp.
    pub created_at: String,
    /// The staging run id.
    pub run_id: String,
    /// Primary adapter id (empty for mixed-identity runs).
    pub adapter_id: String,
    /// Primary adapter version (empty for mixed-identity runs).
    pub adapter_version: String,
    /// CLI version that produced the manifest.
    pub synthesizer_version: String,
    /// SHA-256 of the canonical YAML of every other field.
    pub manifest_hash: String,
    /// One entry per committed artifact, in deterministic order.
    pub artifacts: Vec<ManifestArtifactEntry>,
    /// True when the synthesizer legitimately produced no project DRG.
    pub built_in_only: bool,
    /// Content-identity digest of the synced bundle files.
    pub bundle_content_hash: Option<String>,
    /// The field names the on-disk document actually carried (legacy verify fallback).
    pub raw_field_names: Option<BTreeSet<String>>,
}

const REQUIRED_FIELDS: [&str; 7] = [
    "created_at",
    "run_id",
    "adapter_id",
    "adapter_version",
    "synthesizer_version",
    "manifest_hash",
    "artifacts",
];

const KNOWN_FIELDS: [&str; 11] = [
    "schema_version",
    "mission_id",
    "created_at",
    "run_id",
    "adapter_id",
    "adapter_version",
    "synthesizer_version",
    "manifest_hash",
    "artifacts",
    "built_in_only",
    "bundle_content_hash",
];

impl SynthesisManifest {
    /// Parse a manifest document, validating it exactly like the Source model.
    pub fn from_value(value: &Value) -> Result<Self, CharterError> {
        let mapping = require_mapping(value, SYNTHESIS_MANIFEST_PATH)?;
        let mut names = BTreeSet::new();
        for (key, _) in mapping {
            let Some(key) = key.as_str() else {
                return Err(CharterError::UnsupportedNode {
                    path: PathBuf::from(SYNTHESIS_MANIFEST_PATH),
                    detail: "manifest keys must be strings".to_string(),
                });
            };
            names.insert(key.to_string());
            if !KNOWN_FIELDS.contains(&key) {
                return Err(CharterError::UnsupportedNode {
                    path: PathBuf::from(SYNTHESIS_MANIFEST_PATH),
                    detail: format!("extra field {key:?} is not allowed"),
                });
            }
        }
        for required in REQUIRED_FIELDS {
            if !names.contains(required) {
                return Err(CharterError::UnsupportedNode {
                    path: PathBuf::from(SYNTHESIS_MANIFEST_PATH),
                    detail: format!("missing required field {required:?}"),
                });
            }
        }
        let schema_version = match optional_string(mapping, "schema_version")? {
            Some(version) => version,
            None => "3".to_string(),
        };
        if schema_version != "2" && schema_version != "3" {
            return Err(CharterError::UnsupportedNode {
                path: PathBuf::from(SYNTHESIS_MANIFEST_PATH),
                detail: format!("unsupported schema_version {schema_version:?}"),
            });
        }
        let manifest_hash = require_string(mapping, "manifest_hash", SYNTHESIS_MANIFEST_PATH)?;
        if manifest_hash.chars().count() != 64 {
            return Err(CharterError::UnsupportedNode {
                path: PathBuf::from(SYNTHESIS_MANIFEST_PATH),
                detail: "manifest_hash must be 64 characters".to_string(),
            });
        }
        let synthesizer_version =
            require_string(mapping, "synthesizer_version", SYNTHESIS_MANIFEST_PATH)?;
        if synthesizer_version.is_empty() {
            return Err(CharterError::UnsupportedNode {
                path: PathBuf::from(SYNTHESIS_MANIFEST_PATH),
                detail: "synthesizer_version must not be empty".to_string(),
            });
        }
        let artifacts = match mapping.get(Value::String("artifacts".to_string())) {
            Some(Value::Sequence(entries)) => entries
                .iter()
                .map(ManifestArtifactEntry::from_value)
                .collect::<Result<Vec<_>, _>>()?,
            _ => {
                return Err(CharterError::UnsupportedNode {
                    path: PathBuf::from(SYNTHESIS_MANIFEST_PATH),
                    detail: "artifacts must be a list".to_string(),
                });
            }
        };
        Ok(Self {
            schema_version,
            mission_id: optional_string(mapping, "mission_id")?,
            created_at: require_string(mapping, "created_at", SYNTHESIS_MANIFEST_PATH)?,
            run_id: require_string(mapping, "run_id", SYNTHESIS_MANIFEST_PATH)?,
            adapter_id: require_string(mapping, "adapter_id", SYNTHESIS_MANIFEST_PATH)?,
            adapter_version: require_string(mapping, "adapter_version", SYNTHESIS_MANIFEST_PATH)?,
            synthesizer_version,
            manifest_hash,
            artifacts,
            built_in_only: match mapping.get(Value::String("built_in_only".to_string())) {
                None | Some(Value::Null) => false,
                Some(Value::Bool(value)) => *value,
                Some(_) => {
                    return Err(CharterError::UnsupportedNode {
                        path: PathBuf::from(SYNTHESIS_MANIFEST_PATH),
                        detail: "built_in_only must be a boolean".to_string(),
                    });
                }
            },
            bundle_content_hash: optional_string(mapping, "bundle_content_hash")?,
            raw_field_names: Some(names),
        })
    }

    /// Read and validate the manifest at `path`.
    pub fn load(path: &Path) -> Result<Self, CharterError> {
        let text = std::fs::read_to_string(path).map_err(|source| CharterError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let value: Value =
            serde_yaml_ng::from_str(&text).map_err(|error| CharterError::UnsupportedNode {
                path: path.to_path_buf(),
                detail: error.to_string(),
            })?;
        Self::from_value(&value)
    }

    /// The model's own field mapping (every field, defaults included).
    pub fn to_mapping(&self) -> Mapping {
        let mut mapping = Mapping::new();
        insert_str(&mut mapping, "schema_version", &self.schema_version);
        mapping.insert(
            Value::String("mission_id".to_string()),
            match &self.mission_id {
                Some(value) => Value::String(value.clone()),
                None => Value::Null,
            },
        );
        insert_str(&mut mapping, "created_at", &self.created_at);
        insert_str(&mut mapping, "run_id", &self.run_id);
        insert_str(&mut mapping, "adapter_id", &self.adapter_id);
        insert_str(&mut mapping, "adapter_version", &self.adapter_version);
        insert_str(
            &mut mapping,
            "synthesizer_version",
            &self.synthesizer_version,
        );
        insert_str(&mut mapping, "manifest_hash", &self.manifest_hash);
        mapping.insert(
            Value::String("artifacts".to_string()),
            Value::Sequence(
                self.artifacts
                    .iter()
                    .map(|entry| entry.to_value())
                    .collect(),
            ),
        );
        mapping.insert(
            Value::String("built_in_only".to_string()),
            Value::Bool(self.built_in_only),
        );
        mapping.insert(
            Value::String("bundle_content_hash".to_string()),
            match &self.bundle_content_hash {
                Some(value) => Value::String(value.clone()),
                None => Value::Null,
            },
        );
        mapping
    }

    /// `compute_manifest_hash`: canonical YAML of every field except `manifest_hash`.
    pub fn compute_manifest_hash(&self) -> String {
        canonical_manifest_hash(&self.to_mapping())
    }

    /// `verify_manifest_hash`: the stored hash must match, or the raw on-disk
    /// field subset must (the legacy fallback for a manifest written before a
    /// schema field existed).
    pub fn verify_manifest_hash(&self) -> Result<(), CharterError> {
        let computed = self.compute_manifest_hash();
        if computed == self.manifest_hash {
            return Ok(());
        }
        if let Some(names) = &self.raw_field_names {
            let mut subset = Mapping::new();
            for (key, value) in self.to_mapping() {
                let Some(name) = key.as_str().map(str::to_string) else {
                    continue;
                };
                if name != "manifest_hash" && names.contains(&name) {
                    subset.insert(key, value);
                }
            }
            let legacy =
                sk_kernel::content_digest::sha256_hex(&canonical_yaml(&Value::Mapping(subset)));
            if legacy == self.manifest_hash {
                return Ok(());
            }
        }
        Err(CharterError::UnsupportedNode {
            path: PathBuf::from(SYNTHESIS_MANIFEST_PATH),
            detail: format!(
                "manifest_hash mismatch (stored {}..., computed {}...)",
                self.manifest_hash.chars().take(12).collect::<String>(),
                computed.chars().take(12).collect::<String>()
            ),
        })
    }

    /// `finalize_manifest`: recompute `manifest_hash` from the full model.
    pub fn finalize(&self) -> Self {
        let mut finalized = self.clone();
        finalized.manifest_hash = finalized.compute_manifest_hash();
        finalized
    }

    /// The canonical bytes a manifest-writing caller persists.
    pub fn to_canonical_bytes(&self) -> Vec<u8> {
        canonical_yaml(&Value::Mapping(self.to_mapping()))
    }
}

fn canonical_manifest_hash(mapping: &Mapping) -> String {
    let mut fields = Mapping::new();
    for (key, value) in mapping {
        if key.as_str() == Some("manifest_hash") {
            continue;
        }
        fields.insert(key.clone(), value.clone());
    }
    sk_kernel::content_digest::sha256_hex(&canonical_yaml(&Value::Mapping(fields)))
}

fn insert_str(mapping: &mut Mapping, key: &str, value: &str) {
    mapping.insert(
        Value::String(key.to_string()),
        Value::String(value.to_string()),
    );
}

fn require_mapping<'a>(value: &'a Value, what: &str) -> Result<&'a Mapping, CharterError> {
    value
        .as_mapping()
        .ok_or_else(|| CharterError::UnsupportedNode {
            path: PathBuf::from(what),
            detail: "expected a mapping".to_string(),
        })
}

fn require_string(mapping: &Mapping, key: &str, what: &str) -> Result<String, CharterError> {
    match mapping.get(Value::String(key.to_string())) {
        Some(Value::String(value)) => Ok(value.clone()),
        Some(Value::Null) | None => Err(CharterError::UnsupportedNode {
            path: PathBuf::from(what),
            detail: format!("required field {key:?} is absent"),
        }),
        Some(_) => Err(CharterError::UnsupportedNode {
            path: PathBuf::from(what),
            detail: format!("field {key:?} must be a string"),
        }),
    }
}

fn optional_string(mapping: &Mapping, key: &str) -> Result<Option<String>, CharterError> {
    match mapping.get(Value::String(key.to_string())) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(CharterError::UnsupportedNode {
            path: PathBuf::from(SYNTHESIS_MANIFEST_PATH),
            detail: format!("field {key:?} must be a string"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directive_numbers_follow_the_source_scan() {
        assert_eq!(extract_directive_number(Some("PROJECT_001")), "001");
        assert_eq!(extract_directive_number(Some("PROJECT_12")), "012");
        assert_eq!(extract_directive_number(Some("project_001")), "000");
        assert_eq!(extract_directive_number(Some("DIRECTIVE_044")), "044");
        assert_eq!(extract_directive_number(None), "000");
        assert_eq!(extract_directive_number(Some("")), "000");
        assert_eq!(
            artifact_filename("directive", "x", Some("PROJECT_7")).unwrap(),
            "007-x.directive.yaml"
        );
    }

    #[test]
    fn the_provenance_path_shape_is_kind_dash_slug() {
        assert_eq!(
            provenance_path_for("directive", "project-intent-directive"),
            ".kittify/charter/provenance/directive-project-intent-directive.yaml"
        );
    }

    #[test]
    fn a_finalized_manifest_verifies() {
        let manifest = SynthesisManifest {
            schema_version: "3".to_string(),
            mission_id: None,
            created_at: "2026-01-01T00:00:00+00:00".to_string(),
            run_id: "01J00000000000000000000000".to_string(),
            adapter_id: "a".to_string(),
            adapter_version: "1".to_string(),
            synthesizer_version: "4.0.0".to_string(),
            manifest_hash: "0".repeat(64),
            artifacts: vec![ManifestArtifactEntry {
                kind: "directive".to_string(),
                slug: "s".to_string(),
                path: ".kittify/charter-packs/directive/001-s.directive.yaml".to_string(),
                provenance_path: provenance_path_for("directive", "s"),
                content_hash: "abc".to_string(),
            }],
            built_in_only: false,
            bundle_content_hash: None,
            raw_field_names: None,
        };
        let finalized = manifest.finalize();
        assert_ne!(finalized.manifest_hash, manifest.manifest_hash);
        assert!(finalized.verify_manifest_hash().is_ok());
        let reparsed = SynthesisManifest::from_value(
            &serde_yaml_ng::from_str(&String::from_utf8(finalized.to_canonical_bytes()).unwrap())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(reparsed.manifest_hash, finalized.manifest_hash);
        assert!(reparsed.verify_manifest_hash().is_ok());
    }

    #[test]
    fn an_extra_manifest_field_is_refused_like_the_source_model() {
        let text = "created_at: x\nrun_id: y\nadapter_id: a\nadapter_version: 1\nsynthesizer_version: s\nmanifest_hash: 0000000000000000000000000000000000000000000000000000000000000000\nartifacts: []\nextra: 1\n";
        let value: Value = serde_yaml_ng::from_str(text).unwrap();
        assert!(SynthesisManifest::from_value(&value).is_err());
    }
}
