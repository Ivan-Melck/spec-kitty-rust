//! The project-level path-convention override reader (`.kittify/config.yaml` →
//! `project.path_conventions`).
//!
//! Source `specify_cli.config.path_conventions` is deliberately *narrow*: it
//! reads only that one subkey (never the rest of `project:`, which carries
//! identity fields), it is **remap-only**, and it fails closed on a malformed
//! *section* while staying lenient about an absent or unreadable file. The
//! artifact-routed key (`deliverables`) is excluded from the vocabulary with a
//! warning, because overriding it would flip the path's resolution surface from
//! the Mission's primary surface to the repository root.
//!
//! Diagnostics are reproduced exactly (`{key!r}`, `{value!r}`, the Python type
//! name, and the sorted known-key list), because operators copy them.

use std::path::{Path, PathBuf};

use sk_kernel::PythonString;

use crate::definition::dialect::SafeDocumentDecoder;
use crate::definition::model::VALID_PATH_KEYS;
use crate::definition::value::OrderedMap;

/// Source `ARTIFACT_ROUTED_KEYS`: keys whose mission default value is a mission
/// artifact token, and which therefore cannot be overridden.
pub const ARTIFACT_ROUTED_KEYS: [&str; 1] = ["deliverables"];

/// The `project.path_conventions` section key, as Source names it in messages.
pub const SECTION: &str = "project.path_conventions";

/// Source `PathConventionsConfigError`: the section is present but malformed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathConventionsConfigError {
    /// The rendered message.
    pub message: PythonString,
}

impl std::fmt::Display for PathConventionsConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&crate::definition::error::escape_codepoints(&self.message))
    }
}

impl std::error::Error for PathConventionsConfigError {}

/// A validated, remap-only project override.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectPathConventions {
    /// Key → directory, in authored order.
    pub overrides: OrderedMap<PythonString>,
    /// `warnings.warn` messages Source emitted while reading (artifact-routed
    /// keys ignored).
    pub warnings: Vec<PythonString>,
}

impl ProjectPathConventions {
    /// The override for `key`, if the project declared one.
    pub fn get(&self, key: &PythonString) -> Option<&PythonString> {
        self.overrides.get(key)
    }

    /// Whether nothing was declared.
    pub fn is_empty(&self) -> bool {
        self.overrides.is_empty()
    }
}

/// Read `project.path_conventions` from `<repo_root>/.kittify/config.yaml`.
///
/// Absent file, unreadable file, a non-mapping document, or a non-mapping
/// `project:` block all yield the empty override (Source's lenient posture). A
/// present but malformed *section* yields [`PathConventionsConfigError`].
///
/// The file is decoded through the injected [`SafeDocumentDecoder`] into the ONE
/// canonical object graph; only `project.path_conventions` is ever traversed, so
/// an unrelated cyclic field elsewhere in `config.yaml` is not projected. Decoding
/// failure is one of Source's lenient cases (`{}`), so a dialect mismatch cannot
/// turn a readable project into a hard failure here.
pub fn load_project_path_conventions(
    repo_root: &Path,
    decoder: &dyn SafeDocumentDecoder,
) -> Result<ProjectPathConventions, PathConventionsConfigError> {
    let config_path: PathBuf = repo_root.join(".kittify").join("config.yaml");
    if !config_path.is_file() {
        return Ok(ProjectPathConventions::default());
    }
    // Source is lenient about every read/decode failure on this file.
    let Some(text) = read_text(&config_path) else {
        return Ok(ProjectPathConventions::default());
    };
    let Ok(document) = decoder.decode(&config_path, &text) else {
        return Ok(ProjectPathConventions::default());
    };
    let root = document.root();
    if root.as_mapping().is_none() {
        return Ok(ProjectPathConventions::default());
    }
    let Some(project) = root.lookup("project") else {
        return Ok(ProjectPathConventions::default());
    };
    if project.as_mapping().is_none() {
        return Ok(ProjectPathConventions::default());
    }
    let Some(section) = project.lookup("path_conventions") else {
        return Ok(ProjectPathConventions::default());
    };
    let Some(entries) = section.as_mapping() else {
        return Err(PathConventionsConfigError {
            message: python(&format!(
                "{SECTION} must be a mapping of path-convention key to directory, got {}.",
                section.python_type_name()
            )),
        });
    };

    let mut overrides = OrderedMap::new();
    let mut warnings = Vec::new();
    for (key, value) in entries {
        let key = section.view(*key);
        let value = section.view(*value);
        let Some(key_text) = key.as_str() else {
            return Err(PathConventionsConfigError {
                message: python(&format!(
                    "Unknown path-convention key {} in {SECTION}. Known keys: {}.",
                    key.repr(),
                    known_keys()
                )),
            });
        };
        let key_scalar = match key_text.to_scalar_string() {
            Ok(scalar) => scalar,
            Err(_) => {
                return Err(PathConventionsConfigError {
                    message: python(&format!(
                        "Unknown path-convention key {} in {SECTION}. Known keys: {}.",
                        key_text.repr(),
                        known_keys()
                    )),
                });
            }
        };
        if !VALID_PATH_KEYS.iter().any(|valid| *valid == key_scalar) {
            return Err(PathConventionsConfigError {
                message: python(&format!(
                    "Unknown path-convention key {} in {SECTION}. Known keys: {}.",
                    key_text.repr(),
                    known_keys()
                )),
            });
        }
        if ARTIFACT_ROUTED_KEYS
            .iter()
            .any(|routed| *routed == key_scalar)
        {
            warnings.push(python(&format!(
                "{SECTION}.{key_scalar} is ignored: {} is artifact-routed and cannot be \
                 overridden (it would flip the artifact-resolution surface). See ADR \
                 project-path-convention-override-precedes-doctrine.",
                key_text.repr()
            )));
            continue;
        }
        let Some(value_text) = value.as_str() else {
            return Err(PathConventionsConfigError {
                message: python(&format!(
                    "{SECTION}.{key_scalar} must be a string directory, got {}.",
                    value.python_type_name()
                )),
            });
        };
        let Some(raw) = value_text.to_scalar_string().ok() else {
            return Err(PathConventionsConfigError {
                message: python(&format!(
                    "{SECTION}.{key_scalar} must be a string directory, got str."
                )),
            });
        };
        let normalized = raw.trim();
        if normalized.is_empty() {
            return Err(PathConventionsConfigError {
                message: python(&format!(
                    "{SECTION}.{key_scalar} must not be empty or blank."
                )),
            });
        }
        let candidate = Path::new(normalized);
        let absolute = candidate.is_absolute() || is_windows_absolute(normalized);
        let traversal = normalized.split(['/', '\\']).any(|part| part == "..");
        if absolute || traversal {
            return Err(PathConventionsConfigError {
                message: python(&format!(
                    "{SECTION}.{key_scalar} must be a repo-relative path with no '..' traversal, \
                     got {}.",
                    value_text.repr()
                )),
            });
        }
        overrides.insert(key_text.clone(), python(normalized));
    }
    Ok(ProjectPathConventions {
        overrides,
        warnings,
    })
}

fn is_windows_absolute(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\')
}

fn read_text(path: &Path) -> Option<String> {
    let bytes = sk_kernel::read_bounded(path, 16 * 1024 * 1024).ok()?;
    let text = std::str::from_utf8(&bytes).ok()?;
    Some(text.strip_prefix('\u{feff}').unwrap_or(text).to_owned())
}

/// The last entry for a string key (Python `dict` semantics after duplicate
/// keys collapse: the last authored value wins) is provided by the ONE graph view
/// ([`sk_kernel::python_yaml::NodeRef::lookup`]); this module carries no private
/// walker or scalar `repr`.
fn known_keys() -> String {
    let mut keys: Vec<PythonString> = VALID_PATH_KEYS.iter().map(|key| python(key)).collect();
    crate::definition::model::sort_python_strings(&mut keys);
    crate::definition::model::repr_list(&keys)
}

fn python(text: &str) -> PythonString {
    PythonString::from_scalars(text)
}
