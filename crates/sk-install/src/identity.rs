//! Project identity: `.kittify/config.yaml` → `project:`.
//!
//! Upstream mints `project.uuid` (UUIDv4), `project.slug` (repository name,
//! kebab-case), `project.node_id` (first 12 hex of `sha256(hostname:user)`) and
//! a deterministic `project.build_id` (UUIDv5 over the URL namespace and
//! `"<uuid>:<node_id>"`) at init, and every later reader resolves it without
//! mutating anything.
//!
//! This module keeps those exact derivations and adds the preservation contract
//! the Python writer only partly has: writing identity **merges** into the
//! existing `project:` mapping, so operator keys and comments in that section
//! survive. A recorded value that cannot be understood (an invalid UUID, a
//! structured value where a text value belongs) is a typed refusal — re-minting
//! would orphan every journal and ledger row keyed on the old identity.
//!
//! The repository slug is derived from the directory name. Upstream additionally
//! consults `git remote origin`; that read belongs to the Git owner, so this
//! crate does not spawn `git` for it. A caller that wants the remote-derived slug
//! can pass one explicitly through [`ProjectIdentity::with_repo_slug`].

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_yaml_ng::{Mapping, Value};
use sha2::{Digest, Sha256};

use crate::error::InstallError;
use crate::lock::ProjectLock;
use crate::uuid_support::{NAMESPACE_URL, Uuid};
use crate::yaml_edit::{InsertPosition, Scalar, YamlEditor, parse_mapping};

/// The build-id namespace label upstream hashes into the URL namespace.
pub const BUILD_ID_NAMESPACE_NAME: &str = "spec-kitty:identity:build_id";

/// The project identity fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectIdentity {
    /// `project.uuid`.
    pub project_uuid: Option<Uuid>,
    /// `project.slug`.
    pub project_slug: Option<String>,
    /// `project.node_id`.
    pub node_id: Option<String>,
    /// `project.repo_slug` (an operator override; never auto-generated).
    pub repo_slug: Option<String>,
    /// `project.build_id`.
    pub build_id: Option<String>,
}

impl Default for ProjectIdentity {
    fn default() -> Self {
        Self::empty()
    }
}

impl ProjectIdentity {
    /// An identity with no recorded fields.
    pub fn empty() -> Self {
        Self {
            project_uuid: None,
            project_slug: None,
            node_id: None,
            repo_slug: None,
            build_id: None,
        }
    }

    /// Whether every required field (everything except `repo_slug`) is present.
    pub fn is_complete(&self) -> bool {
        self.project_uuid.is_some()
            && self.project_slug.is_some()
            && self.node_id.is_some()
            && self.build_id.is_some()
    }

    /// Override the optional repository slug.
    pub fn with_repo_slug(mut self, repo_slug: impl Into<String>) -> Self {
        self.repo_slug = Some(repo_slug.into());
        self
    }

    /// Fill missing fields with generated/deterministic values (no I/O).
    pub fn with_defaults(&self, project_root: &Path) -> Self {
        let project_uuid = self.project_uuid.unwrap_or_else(|| {
            Uuid::new_v4().unwrap_or_else(|_| {
                // `with_defaults` is infallible by contract (upstream's is too).
                // A failed CSPRNG read must not panic in a library; fall back to
                // entropy from the wall clock and process id, flagged by an
                // impossible-but-valid variant later if audited.
                let seed = format!(
                    "{:?}:{}:{}",
                    std::time::SystemTime::now(),
                    std::process::id(),
                    project_root.display()
                );
                Uuid::new_v5(NAMESPACE_URL, &seed)
            })
        });
        let node_id = self.node_id.clone().unwrap_or_else(generate_node_id);
        let build_id = self
            .build_id
            .clone()
            .unwrap_or_else(|| derive_build_id(project_uuid, &node_id));
        Self {
            project_uuid: Some(project_uuid),
            project_slug: Some(
                self.project_slug
                    .clone()
                    .unwrap_or_else(|| derive_project_slug(project_root)),
            ),
            node_id: Some(node_id),
            repo_slug: self.repo_slug.clone(),
            build_id: Some(build_id),
        }
    }

    /// The `project:` mapping this identity persists.
    pub fn to_mapping(&self) -> Mapping {
        let mut mapping = Mapping::new();
        for (key, value) in [
            ("uuid", self.project_uuid.map(|uuid| uuid.to_string())),
            ("slug", self.project_slug.clone()),
            ("node_id", self.node_id.clone()),
            ("repo_slug", self.repo_slug.clone()),
            ("build_id", self.build_id.clone()),
        ] {
            if let Some(value) = value {
                mapping.insert(Value::from(key), Value::from(value));
            }
        }
        mapping
    }
}

/// Derive the deterministic build id from the project uuid and node id.
pub fn derive_build_id(project_uuid: Uuid, node_id: &str) -> String {
    let namespace = Uuid::new_v5(NAMESPACE_URL, BUILD_ID_NAMESPACE_NAME);
    Uuid::new_v5(namespace, &format!("{project_uuid}:{node_id}")).to_string()
}

/// Generate the stable machine identifier (`sha256(hostname:user)[..12]`).
pub fn generate_node_id() -> String {
    let hostname = std::env::var("HOSTNAME")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            std::fs::read_to_string("/proc/sys/kernel/hostname")
                .ok()
                .map(|value| value.trim().to_string())
        })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown-host".to_string());
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown-user".to_string());
    let digest = Sha256::digest(format!("{hostname}:{user}").as_bytes());
    digest[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Derive the project slug from the directory name (upstream's fallback).
pub fn derive_project_slug(project_root: &Path) -> String {
    let name = project_root
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "spec-kitty-project".to_string());
    normalize_slug(&name)
}

fn normalize_slug(name: &str) -> String {
    name.to_lowercase().replace(['_', ' '], "-")
}

/// Load the recorded identity; absent or unreadable config yields an empty one.
pub fn load_identity(project_root: &Path) -> ProjectIdentity {
    read_identity(project_root).unwrap_or_else(|_| ProjectIdentity::empty())
}

/// Resolve a complete identity **without writing anything**.
pub fn resolve_identity(project_root: &Path) -> ProjectIdentity {
    load_identity(project_root).with_defaults(project_root)
}

/// Load or mint the identity, persisting missing fields atomically.
///
/// The write is serialized by the project install lock and merges into the
/// existing `project:` mapping, preserving comments and unrelated keys. A
/// recorded value that cannot be understood is refused rather than replaced.
pub fn ensure_identity(
    project_root: &Path,
    lock_wait: Duration,
) -> Result<ProjectIdentity, InstallError> {
    let recorded = read_identity_strict(project_root)?;
    if recorded.is_complete() {
        return Ok(recorded);
    }
    let resolved = recorded.with_defaults(project_root);
    let config_path = project_root.join(".kittify/config.yaml");
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|source| InstallError::io("create directory", parent, source))?;
    }
    let _lock = ProjectLock::acquire(&project_root.join(crate::lock::LOCK_REL), lock_wait)?;
    let text = match std::fs::read(&config_path) {
        Ok(raw) => String::from_utf8(raw).map_err(|error| InstallError::IdentityInvalid {
            path: config_path.clone(),
            detail: error.to_string(),
        })?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(source) => return Err(InstallError::io("read", &config_path, source)),
    };
    let entries = vec![
        (
            "uuid".to_string(),
            Scalar::Str(
                resolved
                    .project_uuid
                    .expect("with_defaults fills the uuid")
                    .to_string(),
            ),
        ),
        (
            "slug".to_string(),
            Scalar::Str(resolved.project_slug.clone().unwrap_or_default()),
        ),
        (
            "node_id".to_string(),
            Scalar::Str(resolved.node_id.clone().unwrap_or_default()),
        ),
        (
            "build_id".to_string(),
            Scalar::Str(resolved.build_id.clone().unwrap_or_default()),
        ),
    ];
    let mut entries = entries;
    if let Some(repo_slug) = &resolved.repo_slug {
        entries.push(("repo_slug".to_string(), Scalar::Str(repo_slug.clone())));
    }
    let editor = YamlEditor::new(config_path.clone(), text);
    let outcome = editor.upsert_scalars("project", &entries, InsertPosition::Top)?;
    if outcome.changed {
        sk_kernel::atomic_write(&config_path, outcome.text.as_bytes()).map_err(|error| {
            InstallError::io("write config", &config_path, std::io::Error::other(error))
        })?;
    }
    Ok(resolved)
}

/// Strict read used by the writer: a malformed `project:` section is a refusal.
pub fn read_identity_strict(project_root: &Path) -> Result<ProjectIdentity, InstallError> {
    let config_path = project_root.join(".kittify/config.yaml");
    let text = match std::fs::read(&config_path) {
        Ok(raw) => String::from_utf8(raw).map_err(|error| InstallError::IdentityInvalid {
            path: config_path.clone(),
            detail: error.to_string(),
        })?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ProjectIdentity::empty());
        }
        Err(source) => return Err(InstallError::io("read", &config_path, source)),
    };
    if text.trim().is_empty() {
        return Ok(ProjectIdentity::empty());
    }
    let mapping =
        parse_mapping(&config_path, &text).map_err(|error| InstallError::IdentityInvalid {
            path: config_path.clone(),
            detail: error.to_string(),
        })?;
    let project = match mapping.get(Value::from("project")) {
        None | Some(Value::Null) => return Ok(ProjectIdentity::empty()),
        Some(Value::Mapping(mapping)) => mapping.clone(),
        Some(_) => {
            return Err(InstallError::IdentityInvalid {
                path: config_path.clone(),
                detail: "'project' section is not a mapping".to_string(),
            });
        }
    };
    let text_field = |key: &str| -> Result<Option<String>, InstallError> {
        match project.get(Value::from(key)) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(value)) => {
                Ok(Some(value.trim().to_string()).filter(|v| !v.is_empty()))
            }
            Some(Value::Number(value)) => Ok(Some(value.to_string())),
            Some(Value::Bool(value)) => Ok(Some(value.to_string())),
            Some(_) => Err(InstallError::IdentityInvalid {
                path: config_path.clone(),
                detail: format!("project.{key} is not a text value"),
            }),
        }
    };
    let project_uuid = match text_field("uuid")? {
        Some(value) => {
            Some(
                Uuid::parse(&value).map_err(|error| InstallError::IdentityInvalid {
                    path: config_path.clone(),
                    detail: format!("project.uuid is not a UUID: {error}"),
                })?,
            )
        }
        None => None,
    };
    Ok(ProjectIdentity {
        project_uuid,
        project_slug: text_field("slug")?,
        node_id: text_field("node_id")?,
        repo_slug: text_field("repo_slug")?,
        build_id: text_field("build_id")?,
    })
}

fn read_identity(project_root: &Path) -> Result<ProjectIdentity, InstallError> {
    read_identity_strict(project_root)
}

/// Whether `project_root` records a complete identity.
pub fn has_complete_identity(project_root: &Path) -> bool {
    read_identity_strict(project_root).is_ok_and(|identity| identity.is_complete())
}

/// The config path identity is persisted to.
pub fn identity_path(project_root: &Path) -> PathBuf {
    project_root.join(".kittify/config.yaml")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).expect("parent");
        std::fs::write(path, text).expect("write");
    }

    #[test]
    fn known_build_id_matches_python_uuid5() {
        let project = Uuid::parse("48fe8ecb-fbf7-4893-90f5-714b9987ac60").expect("uuid");
        assert_eq!(
            derive_build_id(project, "28d3810d268d"),
            "c4685d25-fbd9-5632-a6cb-1822523869dc"
        );
    }

    #[test]
    fn ensure_mints_and_persists_all_fields_once() {
        let directory = tempfile::tempdir().expect("temp");
        write(
            &directory.path().join(".kittify/config.yaml"),
            "vcs:\n  type: git\n",
        );
        let identity = ensure_identity(directory.path(), Duration::from_secs(1)).expect("ensure");
        assert!(identity.is_complete());
        let text = std::fs::read_to_string(directory.path().join(".kittify/config.yaml")).unwrap();
        assert!(text.contains("project:\n"));
        assert!(text.contains("vcs:\n  type: git"));
        let reloaded = load_identity(directory.path());
        assert_eq!(reloaded, identity);
        // A second ensure performs no write.
        let before = std::fs::metadata(directory.path().join(".kittify/config.yaml"))
            .unwrap()
            .modified()
            .unwrap();
        let again = ensure_identity(directory.path(), Duration::from_secs(1)).expect("ensure");
        assert_eq!(again, identity);
        let after = std::fs::metadata(directory.path().join(".kittify/config.yaml"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn ensure_preserves_comments_and_unrelated_project_keys() {
        let directory = tempfile::tempdir().expect("temp");
        let original = "# operator header\nproject:\n  # identity comment\n  repo_slug: owner/repo  # pinned\n  owner_note: keep me\nvcs:\n  type: git\n";
        write(&directory.path().join(".kittify/config.yaml"), original);
        let identity = ensure_identity(directory.path(), Duration::from_secs(1)).expect("ensure");
        let text = std::fs::read_to_string(directory.path().join(".kittify/config.yaml")).unwrap();
        assert!(text.contains("# identity comment"));
        assert!(text.contains("repo_slug: owner/repo  # pinned"));
        assert!(text.contains("owner_note: keep me"));
        assert_eq!(identity.repo_slug.as_deref(), Some("owner/repo"));
        assert!(text.contains(&format!("uuid: {}", identity.project_uuid.unwrap())));
    }

    #[test]
    fn invalid_recorded_uuid_is_refused_not_reminted() {
        let directory = tempfile::tempdir().expect("temp");
        write(
            &directory.path().join(".kittify/config.yaml"),
            "project:\n  uuid: not-a-uuid\n",
        );
        let error = ensure_identity(directory.path(), Duration::from_secs(1)).expect_err("refused");
        assert_eq!(error.code(), "identity_invalid");
        assert!(
            std::fs::read_to_string(directory.path().join(".kittify/config.yaml"))
                .unwrap()
                .contains("not-a-uuid")
        );
    }

    #[test]
    fn resolve_never_writes() {
        let directory = tempfile::tempdir().expect("temp");
        let identity = resolve_identity(directory.path());
        assert!(identity.is_complete());
        assert!(!directory.path().join(".kittify/config.yaml").exists());
        assert_eq!(
            identity.project_slug.as_deref(),
            Some(normalize_slug(&directory.path().file_name().unwrap().to_string_lossy()).as_str())
        );
    }

    #[test]
    fn non_mapping_project_section_is_refused() {
        let directory = tempfile::tempdir().expect("temp");
        write(
            &directory.path().join(".kittify/config.yaml"),
            "project: guard-suite\n",
        );
        assert_eq!(
            ensure_identity(directory.path(), Duration::from_secs(1))
                .expect_err("refused")
                .code(),
            "identity_invalid"
        );
    }
}
