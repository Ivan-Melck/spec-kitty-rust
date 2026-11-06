//! Read-only inspection of `.kittify/config.yaml` and org pack declarations.
//!
//! Only the canonical `charter_packs.org.packs[]` registry, the `charter:`
//! pointer and `charter_packs.project.skill_namespace` are read. Values are
//! preserved exactly as authored; `local_path`/`subdir` are resolved only when a
//! caller asks for an effective root. Nothing is reserialized.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::CharterError;
use crate::io::{
    FieldValue, field, is_ulid, parse_yaml_mapping, read_bounded, resolve_contained,
    validate_relative,
};
use crate::limits::Limits;

/// One `charter_packs.org.packs[]` entry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OrgPackConfig {
    /// Human-readable pack handle (unique within the registry).
    pub name: String,
    /// Stable ULID identity, when already minted.
    pub pack_id: Option<String>,
    /// Authored path, unexpanded and unresolved.
    pub local_path: PathBuf,
    /// Optional pack-relative subdirectory slice.
    pub subdir: Option<String>,
    /// Declared source kind (`git`, `https`, `artifactory`, `api`).
    pub source_type: Option<String>,
    /// Declared remote URL, if any.
    pub url: Option<String>,
    /// Declared remote ref, if any.
    #[serde(rename = "ref")]
    pub r#ref: Option<String>,
}

impl OrgPackConfig {
    /// Resolve the pack's effective root against `project_root`.
    ///
    /// `local_path` is expanded (`${VAR}`, `$VAR`, leading `~`) and joined onto
    /// the project root when relative. A `subdir` must stay inside that root;
    /// traversal and symlink escapes fail closed.
    pub fn effective_root(
        &self,
        project_root: &Path,
        expansion: &PathExpansion,
    ) -> Result<PathBuf, CharterError> {
        let base = self.local_path_root(project_root, expansion)?;
        match &self.subdir {
            Some(subdir) => resolve_contained(&base, subdir),
            None => Ok(base),
        }
    }

    /// Resolve `local_path` (expanded, project-joined) without applying `subdir`.
    pub fn local_path_root(
        &self,
        project_root: &Path,
        expansion: &PathExpansion,
    ) -> Result<PathBuf, CharterError> {
        let expanded = expansion.expand(&self.local_path)?;
        Ok(if expanded.is_absolute() {
            expanded
        } else {
            project_root.join(expanded)
        })
    }
}

/// The inspected project configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProjectConfig {
    /// `.kittify/config.yaml` location (as supplied, not canonicalized).
    pub path: PathBuf,
    /// Whether the file exists.
    pub present: bool,
    /// The top-level `charter:` pointer, when present.
    pub charter_pointer: Option<String>,
    /// `charter_packs.project.skill_namespace`, when present.
    pub project_skill_namespace: Option<String>,
    /// Org packs in declaration order (which is their precedence order).
    pub org_packs: Vec<OrgPackConfig>,
}

impl ProjectConfig {
    /// An absent-config value for a project without `.kittify/config.yaml`.
    pub fn absent(path: PathBuf) -> Self {
        Self {
            path,
            present: false,
            charter_pointer: None,
            project_skill_namespace: None,
            org_packs: Vec::new(),
        }
    }
}

/// Deterministic `${VAR}` / `$VAR` / `~` expansion.
///
/// Variables come from an explicit map so callers (and tests) never depend on
/// ambient process state unless they opt in with [`Self::from_env`].
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PathExpansion {
    vars: BTreeMap<String, String>,
    home: Option<PathBuf>,
}

impl PathExpansion {
    /// An expansion with no variables and no home directory.
    pub fn empty() -> Self {
        Self::default()
    }

    /// An expansion populated from the process environment and `HOME`.
    pub fn from_env() -> Self {
        let vars = std::env::vars().collect();
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from);
        Self { vars, home }
    }

    /// Add or replace one variable.
    pub fn with_var(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.vars.insert(name.into(), value.into());
        self
    }

    /// Set the home directory used for a leading `~`.
    pub fn with_home(mut self, home: impl Into<PathBuf>) -> Self {
        self.home = Some(home.into());
        self
    }

    /// Expand `raw`, failing closed on an unset or empty variable or a missing home.
    pub fn expand(&self, raw: &Path) -> Result<PathBuf, CharterError> {
        let text = raw.to_string_lossy();
        let unresolved = |token: String| CharterError::UnresolvedPathToken {
            path: raw.to_path_buf(),
            token,
        };

        let mut expanded = String::with_capacity(text.len());
        if text == "~" || text.starts_with("~/") || text.starts_with("~\\") {
            let home = self
                .home
                .as_ref()
                .ok_or_else(|| unresolved("~".to_owned()))?;
            expanded.push_str(&home.to_string_lossy());
            expanded.push_str(&text[1..]);
        } else {
            expanded.push_str(&text);
        }

        let chars: Vec<char> = expanded.chars().collect();
        let mut output = String::with_capacity(expanded.len());
        let mut index = 0;
        while index < chars.len() {
            if chars[index] != '$' {
                output.push(chars[index]);
                index += 1;
                continue;
            }
            if index + 1 < chars.len() && chars[index + 1] == '{' {
                let Some(end) = chars[index + 2..].iter().position(|ch| *ch == '}') else {
                    return Err(unresolved("${".to_owned()));
                };
                let name: String = chars[index + 2..index + 2 + end].iter().collect();
                let token: String = chars[index..=index + 2 + end].iter().collect();
                let value = self.vars.get(&name).filter(|value| !value.is_empty());
                let value = value.ok_or_else(|| unresolved(token))?;
                output.push_str(value);
                index += 3 + end;
                continue;
            }
            let mut end = index + 1;
            while end < chars.len() && (chars[end].is_ascii_alphanumeric() || chars[end] == '_') {
                end += 1;
            }
            if end == index + 1 {
                output.push('$');
                index += 1;
                continue;
            }
            let name: String = chars[index + 1..end].iter().collect();
            let token = format!("${name}");
            let value = self
                .vars
                .get(&name)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| unresolved(token))?;
            output.push_str(value);
            index = end;
        }
        Ok(PathBuf::from(output))
    }
}

/// Read only the org-pack registry from `.kittify/config.yaml`.
///
/// Unlike whole-project config validation, this read does not judge unrelated
/// `charter` pointers or project skill namespaces. Pack declarations still use
/// the exact same parser and path/identity rules as `load_project_config`.
/// Consumers implementing a lenient registry read may degrade an error to an
/// empty registry; path expansion remains a separate, fallible operation.
pub fn load_org_pack_registry(
    project_root: &Path,
    limits: &Limits,
) -> Result<Vec<OrgPackConfig>, CharterError> {
    let Some(mapping) = read_project_mapping(project_root, limits)? else {
        return Ok(Vec::new());
    };
    let path = project_root.join(".kittify/config.yaml");
    match field(&mapping, "charter_packs") {
        FieldValue::Absent | FieldValue::Null => Ok(Vec::new()),
        FieldValue::Other(serde_yaml_ng::Value::Mapping(packs)) => {
            parse_org_registry_block(packs, &path)
        }
        _ => Err(invalid(&path, "`charter_packs` must be a mapping")),
    }
}

fn read_project_mapping(
    project_root: &Path,
    limits: &Limits,
) -> Result<Option<serde_yaml_ng::Mapping>, CharterError> {
    let path = project_root.join(".kittify/config.yaml");
    if !path.exists() {
        return Ok(None);
    }
    if !path.is_file() {
        return Err(invalid(&path, "expected a regular file"));
    }
    let resolved = crate::io::resolve_contained(project_root, ".kittify/config.yaml")?;
    let bytes = read_bounded(&resolved, limits.max_config_bytes)?;
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
    parse_yaml_mapping(&path, bytes).map(Some)
}

/// Read and validate `.kittify/config.yaml`.
///
/// An absent file is absence, not an error. A present but corrupt, oversized or
/// malformed document fails closed.
pub fn load_project_config(
    project_root: &Path,
    limits: &Limits,
) -> Result<ProjectConfig, CharterError> {
    let path = project_root.join(".kittify").join("config.yaml");
    let Some(mapping) = read_project_mapping(project_root, limits)? else {
        return Ok(ProjectConfig::absent(path));
    };

    let charter_pointer = match field(&mapping, "charter") {
        FieldValue::Absent | FieldValue::Null => None,
        FieldValue::String(value) => Some(value.to_owned()),
        FieldValue::Other(_) => {
            return Err(invalid(&path, "`charter` must be a string pointer"));
        }
    };

    let (org_packs, project_skill_namespace) = match field(&mapping, "charter_packs") {
        FieldValue::Absent | FieldValue::Null => (Vec::new(), None),
        FieldValue::Other(serde_yaml_ng::Value::Mapping(charter_packs)) => {
            parse_charter_packs(charter_packs, &path)?
        }
        FieldValue::Other(_) | FieldValue::String(_) => {
            return Err(invalid(&path, "`charter_packs` must be a mapping"));
        }
    };

    Ok(ProjectConfig {
        path,
        present: true,
        charter_pointer,
        project_skill_namespace,
        org_packs,
    })
}

fn parse_charter_packs(
    charter_packs: &serde_yaml_ng::Mapping,
    path: &Path,
) -> Result<(Vec<OrgPackConfig>, Option<String>), CharterError> {
    let org_packs = parse_org_registry_block(charter_packs, path)?;

    let namespace = parse_project_namespace(charter_packs, path)?;
    Ok((org_packs, namespace))
}

fn parse_org_registry_block(
    charter_packs: &serde_yaml_ng::Mapping,
    path: &Path,
) -> Result<Vec<OrgPackConfig>, CharterError> {
    match field(charter_packs, "org") {
        FieldValue::Absent | FieldValue::Null => Ok(Vec::new()),
        FieldValue::Other(serde_yaml_ng::Value::Mapping(org)) => parse_org_block(org, path),
        FieldValue::Other(_) | FieldValue::String(_) => {
            Err(invalid(path, "`charter_packs.org` must be a mapping"))
        }
    }
}

fn parse_project_namespace(
    charter_packs: &serde_yaml_ng::Mapping,
    path: &Path,
) -> Result<Option<String>, CharterError> {
    let namespace = match field(charter_packs, "project") {
        FieldValue::Absent | FieldValue::Null => None,
        FieldValue::Other(serde_yaml_ng::Value::Mapping(project)) => {
            match field(project, "skill_namespace") {
                FieldValue::Absent | FieldValue::Null => None,
                FieldValue::String(value) if !value.trim().is_empty() => Some(value.to_owned()),
                FieldValue::String(_) => {
                    return Err(invalid(
                        path,
                        "`charter_packs.project.skill_namespace` must not be blank",
                    ));
                }
                FieldValue::Other(_) => {
                    return Err(invalid(
                        path,
                        "`charter_packs.project.skill_namespace` must be a string",
                    ));
                }
            }
        }
        FieldValue::Other(_) | FieldValue::String(_) => {
            return Err(invalid(path, "`charter_packs.project` must be a mapping"));
        }
    };

    Ok(namespace)
}

fn parse_org_block(
    org: &serde_yaml_ng::Mapping,
    path: &Path,
) -> Result<Vec<OrgPackConfig>, CharterError> {
    let packs = match field(org, "packs") {
        FieldValue::Absent | FieldValue::Null => {
            return if matches!(
                field(org, "local_path"),
                FieldValue::Absent | FieldValue::Null
            ) {
                Ok(Vec::new())
            } else {
                Err(invalid(
                    path,
                    "`charter_packs.org.local_path` is not a pack declaration; declare the pack \
                     as a named entry of `charter_packs.org.packs[]` (name, local_path)",
                ))
            };
        }
        FieldValue::Other(serde_yaml_ng::Value::Sequence(sequence)) => sequence,
        FieldValue::Other(_) | FieldValue::String(_) => {
            return Err(invalid(path, "`charter_packs.org.packs` must be a list"));
        }
    };

    let mut parsed = Vec::with_capacity(packs.len());
    let mut names = BTreeMap::new();
    for entry in packs {
        let serde_yaml_ng::Value::Mapping(entry) = entry else {
            return Err(invalid(
                path,
                "each `charter_packs.org.packs[]` entry must be a mapping",
            ));
        };
        let pack = parse_org_pack(entry, path)?;
        if names.insert(pack.name.clone(), ()).is_some() {
            return Err(invalid(
                path,
                &format!(
                    "duplicate pack name {:?} in charter_packs.org.packs",
                    pack.name
                ),
            ));
        }
        parsed.push(pack);
    }
    Ok(parsed)
}

fn parse_org_pack(
    entry: &serde_yaml_ng::Mapping,
    path: &Path,
) -> Result<OrgPackConfig, CharterError> {
    const FIELDS: [&str; 7] = [
        "name",
        "pack_id",
        "local_path",
        "subdir",
        "source_type",
        "url",
        "ref",
    ];
    for key in entry.keys() {
        let Some(key) = key.as_str() else {
            return Err(invalid(path, "pack field names must be strings"));
        };
        if !FIELDS.contains(&key) {
            return Err(invalid(path, &format!("unknown org pack field {key:?}")));
        }
    }

    let name = required_string(entry, "name", path)?;
    if name.trim().is_empty() {
        return Err(invalid(path, "pack `name` must be a non-empty string"));
    }
    let local_path = required_string(entry, "local_path", path)?;
    if local_path.trim().is_empty() {
        return Err(invalid(
            path,
            "pack `local_path` must be a non-empty string",
        ));
    }
    let subdir = optional_string(entry, "subdir", path)?;
    let subdir = match subdir.as_deref().map(str::trim) {
        None | Some("") | Some(".") => None,
        Some(subdir) => {
            validate_relative(subdir)
                .map_err(|error| invalid(path, &format!("invalid pack `subdir`: {error}")))?;
            Some(subdir.to_owned())
        }
    };

    let pack_id = optional_string(entry, "pack_id", path)?;
    if let Some(value) = &pack_id
        && !is_ulid(value)
    {
        return Err(invalid(
            path,
            &format!("pack `pack_id` must be a valid 26-char ULID, got {value:?}"),
        ));
    }
    let source_type = optional_string(entry, "source_type", path)?;
    if let Some(value) = &source_type {
        const SOURCE_TYPES: [&str; 4] = ["git", "https", "artifactory", "api"];
        if !SOURCE_TYPES.contains(&value.as_str()) {
            return Err(invalid(
                path,
                &format!(
                    "pack `source_type` must be one of git, https, artifactory, api; got {value:?}"
                ),
            ));
        }
    }

    Ok(OrgPackConfig {
        name,
        pack_id,
        local_path: PathBuf::from(local_path),
        subdir,
        source_type,
        url: optional_string(entry, "url", path)?,
        r#ref: optional_string(entry, "ref", path)?,
    })
}

fn required_string(
    mapping: &serde_yaml_ng::Mapping,
    key: &str,
    path: &Path,
) -> Result<String, CharterError> {
    optional_string(mapping, key, path)?.ok_or_else(|| {
        invalid(
            path,
            &format!("pack field `{key}` is required and must be a string"),
        )
    })
}

fn optional_string(
    mapping: &serde_yaml_ng::Mapping,
    key: &str,
    path: &Path,
) -> Result<Option<String>, CharterError> {
    match field(mapping, key) {
        FieldValue::Absent | FieldValue::Null => Ok(None),
        FieldValue::String(value) => Ok(Some(value.to_owned())),
        FieldValue::Other(_) => Err(invalid(
            path,
            &format!("pack field `{key}` must be a string"),
        )),
    }
}

fn invalid(path: &Path, reason: &str) -> CharterError {
    CharterError::InvalidConfig {
        path: path.to_path_buf(),
        reason: reason.to_owned(),
    }
}
