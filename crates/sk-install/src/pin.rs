//! The pack pin lockfile: which open-packs commit a tree came from.
//!
//! Mirrors `charter/offering/packs/pack_pin.py`. A pin is the *only* provenance
//! authority for a fetched external pack: the repository URL, the resolved
//! 40-hex commit (never a tag), the in-repository subtree, the declared-set
//! digest, the required full-tree digest, and an optional core-version range.
//!
//! Fail-closed rules enforced at parse time:
//!
//! - `repository` must be `https://` or `file://`;
//! - `commit` must be 40 lowercase hex characters (a tag is a resolution input,
//!   never a stored value);
//! - `subtree` must be a relative POSIX path with no `..`, no backslashes and no
//!   leading `-` (it is joined onto a cache path and handed to a transport);
//! - `tree_digest.algorithm` must be [`TREE_DIGEST_ALGORITHM`];
//! - `tree_digest.level` must be a known tier;
//! - `snapshot_sha256`, when present, must be a lowercase hex SHA-256.
//!
//! A pin without `snapshot_sha256` parses but never *verifies*: verification
//! needs both digests, and a partial pin is a configuration error, not a warning.

use std::path::{Component, Path};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::digest::TREE_DIGEST_ALGORITHM;
use crate::error::InstallError;

/// Allowed repository URL schemes (https for real fetches, file for fixtures).
const ALLOWED_SCHEMES: [&str; 2] = ["https://", "file://"];

/// A labelled content digest and the tier it was computed at.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TreeDigest {
    /// The digest algorithm token; only [`TREE_DIGEST_ALGORITHM`] verifies.
    pub algorithm: String,
    /// The digest value (lowercase hex).
    pub value: String,
    /// The tier label the digest was computed at.
    pub level: String,
}

/// The provenance pin of one pack.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackPin {
    /// Source repository (`https://…` or `file://…`).
    pub repository: String,
    /// Resolved 40-hex commit.
    pub commit: String,
    /// The pack's path inside the repository.
    pub subtree: String,
    /// Declared-set digest.
    pub tree_digest: TreeDigest,
    /// Optional PEP 440 range of core versions (parsed, not enforced here).
    pub requires_core: Option<String>,
    /// Required full-tree digest; a pin without it never verifies.
    pub snapshot_sha256: Option<String>,
}

impl PackPin {
    /// Read and validate a pin lockfile.
    pub fn load(path: &Path) -> Result<Self, InstallError> {
        let raw = sk_kernel::read_bounded(path, 256 * 1024).map_err(|error| {
            InstallError::PinInvalid {
                path: path.to_path_buf(),
                detail: format!("cannot read pin lockfile: {error}"),
            }
        })?;
        let text = std::str::from_utf8(&raw).map_err(|error| InstallError::PinInvalid {
            path: path.to_path_buf(),
            detail: error.to_string(),
        })?;
        let document: serde_yaml_ng::Value =
            serde_yaml_ng::from_str(text).map_err(|error| InstallError::PinInvalid {
                path: path.to_path_buf(),
                detail: error.to_string(),
            })?;
        let mapping = document
            .as_mapping()
            .ok_or_else(|| InstallError::PinInvalid {
                path: path.to_path_buf(),
                detail: "expected a mapping at the top level".to_string(),
            })?;

        let field = |key: &str| -> Option<&serde_yaml_ng::Value> { mapping.get(key) };

        let repository = required_string(path, field("repository"), "repository")?;
        let commit = required_string(path, field("commit"), "commit")?;
        let subtree = required_string(path, field("subtree"), "subtree")?;
        let requires_core = optional_string(path, field("requires_core"), "requires_core")?;
        let snapshot_sha256 = optional_string(path, field("snapshot_sha256"), "snapshot_sha256")?;

        let digest_value = field("tree_digest").ok_or_else(|| InstallError::PinInvalid {
            path: path.to_path_buf(),
            detail: "missing 'tree_digest' mapping".to_string(),
        })?;
        let digest_mapping = digest_value
            .as_mapping()
            .ok_or_else(|| InstallError::PinInvalid {
                path: path.to_path_buf(),
                detail: "'tree_digest' must be a mapping".to_string(),
            })?;
        let algorithm = required_string(
            path,
            digest_mapping.get("algorithm"),
            "tree_digest.algorithm",
        )?;
        let value = required_string(path, digest_mapping.get("value"), "tree_digest.value")?;
        let level = optional_string(path, digest_mapping.get("level"), "tree_digest.level")?
            .unwrap_or_else(|| "built-in".to_string());

        Self::from_parts(
            path,
            repository,
            commit,
            subtree,
            TreeDigest {
                algorithm,
                value,
                level,
            },
            requires_core,
            snapshot_sha256,
        )
    }

    /// Build and validate a pin from its parts.
    #[allow(clippy::too_many_arguments)]
    pub fn from_parts(
        path: &Path,
        repository: String,
        commit: String,
        subtree: String,
        tree_digest: TreeDigest,
        requires_core: Option<String>,
        snapshot_sha256: Option<String>,
    ) -> Result<Self, InstallError> {
        if !ALLOWED_SCHEMES
            .iter()
            .any(|scheme| repository.starts_with(scheme))
        {
            return Err(InstallError::PinInvalid {
                path: path.to_path_buf(),
                detail: format!(
                    "repository must start with https:// or file://; got {repository:?}"
                ),
            });
        }
        if !is_lower_hex_commit(&commit) {
            return Err(InstallError::PinInvalid {
                path: path.to_path_buf(),
                detail: format!(
                    "commit must be a 40-hex lowercase SHA (never a tag); got {commit:?}"
                ),
            });
        }
        validate_subtree(&subtree).map_err(|detail| InstallError::PinInvalid {
            path: path.to_path_buf(),
            detail,
        })?;
        if tree_digest.algorithm != TREE_DIGEST_ALGORITHM {
            return Err(InstallError::PinInvalid {
                path: path.to_path_buf(),
                detail: format!(
                    "tree_digest.algorithm must be {TREE_DIGEST_ALGORITHM:?}; got {:?}",
                    tree_digest.algorithm
                ),
            });
        }
        if !is_lower_hex_sha(&tree_digest.value) {
            return Err(InstallError::PinInvalid {
                path: path.to_path_buf(),
                detail: "tree_digest.value must be a lowercase hex SHA-256".to_string(),
            });
        }
        if !matches!(tree_digest.level.as_str(), "built-in" | "org" | "project") {
            return Err(InstallError::PinInvalid {
                path: path.to_path_buf(),
                detail: format!(
                    "tree_digest.level must be built-in, org or project; got {:?}",
                    tree_digest.level
                ),
            });
        }
        if let Some(snapshot) = &snapshot_sha256
            && !is_lower_hex_sha(snapshot)
        {
            return Err(InstallError::PinInvalid {
                path: path.to_path_buf(),
                detail: "snapshot_sha256 must be a lowercase hex SHA-256".to_string(),
            });
        }
        Ok(Self {
            repository,
            commit,
            subtree,
            tree_digest,
            requires_core,
            snapshot_sha256,
        })
    }

    /// The tier label this pin's digest is computed at.
    pub fn tier(&self) -> sk_charter::PackTier {
        match self.tree_digest.level.as_str() {
            "org" => sk_charter::PackTier::Org,
            "project" => sk_charter::PackTier::Project,
            _ => sk_charter::PackTier::BuiltIn,
        }
    }

    /// Fail closed when a required verification input is missing.
    pub fn ensure_verifiable(&self, path: &Path) -> Result<(), InstallError> {
        if self.snapshot_sha256.is_none() {
            return Err(InstallError::PinUnverifiable {
                path: path.to_path_buf(),
                detail: format!(
                    "pin for {} has no snapshot_sha256; re-bump the pin to write it",
                    self.repository
                ),
            });
        }
        Ok(())
    }

    /// Verify the tree at `pack_root`: symlink-free, both digests must match.
    pub fn verify(&self, path: &Path, pack_root: &Path) -> Result<(), InstallError> {
        self.ensure_verifiable(path)?;
        crate::digest::refuse_symlinks(pack_root)?;
        let clean = crate::digest::clean_tree_digest(pack_root, self.tier())?;
        if clean != self.tree_digest.value {
            return Err(InstallError::PackDigestMismatch {
                path: pack_root.to_path_buf(),
                expected_clean: self.tree_digest.value.clone(),
                actual_clean: clean,
            });
        }
        let expected_full = self
            .snapshot_sha256
            .clone()
            .expect("ensure_verifiable checked");
        let actual_full = crate::digest::full_tree_digest(pack_root)?;
        if actual_full != expected_full {
            return Err(InstallError::PackDigestMismatch {
                path: pack_root.to_path_buf(),
                expected_clean: expected_full,
                actual_clean: actual_full,
            });
        }
        Ok(())
    }

    /// Whether `tree` is exactly the pinned tree (no error for a mismatch).
    pub fn matches(&self, path: &Path, tree: &Path) -> bool {
        self.verify(path, tree).is_ok()
    }

    /// Render the lockfile bytes in the canonical key order with the direction header.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut text = String::from(
            "# Pin lockfile: which spec-kitty-open-packs commit this pack was taken from.\n\
# Direction: open-packs -> core. (open-packs' own built-in.source.yaml records the\n\
# OPPOSITE anchor, core -> open-packs.) The tree is fetched from the pinned commit,\n\
# not vendored; tree_digest = clean_tree_digest(<subtree>, level=<tree_digest.level>).\n",
        );
        text.push_str(&format!("repository: {}\n", self.repository));
        text.push_str(&format!("commit: {}\n", self.commit));
        text.push_str(&format!("subtree: {}\n", self.subtree));
        text.push_str("tree_digest:\n");
        text.push_str(&format!("  algorithm: {}\n", self.tree_digest.algorithm));
        text.push_str(&format!("  value: {}\n", self.tree_digest.value));
        text.push_str(&format!("  level: {}\n", self.tree_digest.level));
        if let Some(requires_core) = &self.requires_core {
            text.push_str(&format!("requires_core: '{requires_core}'\n"));
        }
        if let Some(snapshot) = &self.snapshot_sha256 {
            text.push_str(&format!("snapshot_sha256: {snapshot}\n"));
        }
        text.into_bytes()
    }
}

/// Validate a pin subtree path.
pub fn validate_subtree(subtree: &str) -> Result<(), String> {
    if subtree.is_empty() {
        return Err("subtree must be a non-empty POSIX path".to_string());
    }
    if subtree.contains('\\') {
        return Err(format!(
            "subtree must be a POSIX path without backslashes; got {subtree:?}"
        ));
    }
    if subtree.starts_with('-') {
        return Err(format!("subtree must not start with '-'; got {subtree:?}"));
    }
    let path = Path::new(subtree);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::Prefix(_) | Component::RootDir | Component::ParentDir
            )
        })
    {
        return Err(format!(
            "subtree must be relative and must not contain '..'; got {subtree:?}"
        ));
    }
    Ok(())
}

/// Whether `value` is a lowercase 40-hex git commit id.
pub fn is_lower_hex_commit(value: &str) -> bool {
    value.len() == 40 && is_lower_hex(value)
}

/// Whether `value` is a lowercase hex SHA-256.
pub fn is_lower_hex_sha(value: &str) -> bool {
    value.len() == 64 && is_lower_hex(value)
}

fn is_lower_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The SHA-256 of `bytes` as lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn required_string(
    path: &Path,
    value: Option<&serde_yaml_ng::Value>,
    key: &str,
) -> Result<String, InstallError> {
    match value {
        Some(serde_yaml_ng::Value::String(text)) if !text.is_empty() => Ok(text.clone()),
        Some(serde_yaml_ng::Value::Number(number)) => Ok(number.to_string()),
        Some(_) => Err(InstallError::PinInvalid {
            path: path.to_path_buf(),
            detail: format!("field {key:?} must be a non-empty string"),
        }),
        None => Err(InstallError::PinInvalid {
            path: path.to_path_buf(),
            detail: format!("missing required field {key:?}"),
        }),
    }
}

fn optional_string(
    path: &Path,
    value: Option<&serde_yaml_ng::Value>,
    key: &str,
) -> Result<Option<String>, InstallError> {
    match value {
        None | Some(serde_yaml_ng::Value::Null) => Ok(None),
        Some(serde_yaml_ng::Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(InstallError::PinInvalid {
            path: path.to_path_buf(),
            detail: format!("field {key:?} must be a string or null"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "a79c36318b7a506ca2ab26d74705c5e3e406004f";
    const DIGEST: &str = "3b4cbd97a28efcce3b07f9751ccdde4a976bd206b5049056a9c347b9fb73f839";
    const SNAPSHOT: &str = "c21c5903af73181e26879da0ba0cc7e48dff32906148bb6f11e9d6ff575db8d7";

    #[test]
    fn refuses_tag_instead_of_sha() {
        let pin = PackPin::from_parts(
            Path::new("pin"),
            "https://example.invalid/repo.git".to_string(),
            "v1.2.3".to_string(),
            "packs/built-in".to_string(),
            TreeDigest {
                algorithm: TREE_DIGEST_ALGORITHM.to_string(),
                value: DIGEST.to_string(),
                level: "built-in".to_string(),
            },
            None,
            Some(SNAPSHOT.to_string()),
        );
        assert!(pin.is_err());
    }

    #[test]
    fn refuses_parent_traversal_subtree() {
        assert!(validate_subtree("../etc").is_err());
        assert!(validate_subtree("packs/../../etc").is_err());
        assert!(validate_subtree("/abs").is_err());
        assert!(validate_subtree("packs\\built-in").is_err());
        assert!(validate_subtree("packs/built-in").is_ok());
    }

    #[test]
    fn accepts_a_real_pin_shape() {
        let pin = PackPin::from_parts(
            Path::new("pin"),
            "https://example.invalid/repo.git".to_string(),
            SHA.to_string(),
            "packs/built-in".to_string(),
            TreeDigest {
                algorithm: TREE_DIGEST_ALGORITHM.to_string(),
                value: DIGEST.to_string(),
                level: "built-in".to_string(),
            },
            Some(">=4.0.0rc7,<5".to_string()),
            Some(SNAPSHOT.to_string()),
        )
        .expect("pin is valid");
        assert!(pin.ensure_verifiable(Path::new("pin")).is_ok());
        assert_eq!(pin.tier(), sk_charter::PackTier::BuiltIn);
    }

    #[test]
    fn a_pin_lockfile_round_trips_through_its_canonical_bytes() {
        let directory = tempfile::tempdir().expect("temp dir");
        let pin = PackPin::from_parts(
            Path::new("pin"),
            "https://example.invalid/repo.git".to_string(),
            SHA.to_string(),
            "packs/built-in".to_string(),
            TreeDigest {
                algorithm: TREE_DIGEST_ALGORITHM.to_string(),
                value: DIGEST.to_string(),
                level: "built-in".to_string(),
            },
            Some(">=4.0.0rc7,<5".to_string()),
            Some(SNAPSHOT.to_string()),
        )
        .expect("pin");
        let path = directory.path().join("built-in.source.yaml");
        std::fs::write(&path, pin.to_bytes()).expect("write pin");
        let loaded = PackPin::load(&path).expect("reload pin");
        assert_eq!(loaded, pin);
        assert_eq!(loaded.to_bytes(), pin.to_bytes(), "byte-stable");
        // The direction header is part of the file, not the parsed model.
        assert!(
            String::from_utf8(pin.to_bytes())
                .expect("utf-8")
                .starts_with("# Pin lockfile: which spec-kitty-open-packs commit")
        );
    }

    #[test]
    fn missing_snapshot_digest_never_verifies() {
        let pin = PackPin::from_parts(
            Path::new("pin"),
            "https://example.invalid/repo.git".to_string(),
            SHA.to_string(),
            "packs/built-in".to_string(),
            TreeDigest {
                algorithm: TREE_DIGEST_ALGORITHM.to_string(),
                value: DIGEST.to_string(),
                level: "built-in".to_string(),
            },
            None,
            None,
        )
        .expect("pin parses");
        let directory = tempfile::tempdir().expect("temp dir");
        let error = pin
            .verify(Path::new("pin"), directory.path())
            .expect_err("never verifies");
        assert_eq!(error.code(), "PACK_PIN_INVALID");
    }
}
