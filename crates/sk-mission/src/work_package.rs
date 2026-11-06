use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::Serialize;
use serde_json::{Map, Value};
use sk_kernel::{KernelError, WpId, read_bounded};

use crate::error::MissionError;
use crate::frontmatter::{FrontmatterError, split_frontmatter};

/// Default byte budget for one work-package Markdown file.
///
/// The largest work-package prompt in the repository's `kitty-specs` fixtures
/// is under 80 KiB; the cap leaves generous headroom while refusing hostile or
/// runaway files.
pub const DEFAULT_WORK_PACKAGE_BYTE_LIMIT: u64 = 1024 * 1024;

/// Read-only inspection model for one `WP*.md` work-package prompt.
///
/// The Markdown source is never rewritten. Frontmatter is decoded for
/// inspection, and only `work_package_id` and `dependencies` are interpreted.
/// In particular, the retired frontmatter `lane`/`history` values are preserved
/// verbatim but are never exposed as, or inferred into, authoritative state;
/// readiness comes exclusively from a caller-supplied state map (see
/// [`crate::dependency_readiness`]).
#[derive(Clone, Debug, Serialize)]
pub struct WorkPackage {
    id: WpId,
    title: Option<String>,
    dependencies: Vec<WpId>,
    source: PathBuf,
    /// All decoded frontmatter fields, including ones this crate does not
    /// interpret. Preserved exactly as decoded; non-authoritative for state.
    frontmatter: Map<String, Value>,
}

impl WorkPackage {
    /// Canonical `WP<digits>` id, validated by the kernel.
    pub fn id(&self) -> &WpId {
        &self.id
    }

    /// Frontmatter `title`, when present.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Declared dependencies, in declaration order.
    pub fn dependencies(&self) -> &[WpId] {
        &self.dependencies
    }

    /// Path this work package was read from.
    pub fn source(&self) -> &Path {
        &self.source
    }

    /// All decoded frontmatter fields.
    pub fn frontmatter(&self) -> &Map<String, Value> {
        &self.frontmatter
    }

    /// One raw frontmatter field by key.
    pub fn frontmatter_field(&self, key: &str) -> Option<&Value> {
        self.frontmatter.get(key)
    }

    /// Read and parse a work-package file under the default byte budget.
    pub fn from_file(path: &Path) -> Result<Self, MissionError> {
        Self::from_file_with_limit(path, DEFAULT_WORK_PACKAGE_BYTE_LIMIT)
    }

    /// Read and parse a work-package file under an explicit byte budget.
    pub fn from_file_with_limit(path: &Path, limit: u64) -> Result<Self, MissionError> {
        let bytes = read_bounded(path, limit).map_err(|source| match source {
            KernelError::TooLarge { limit, .. } => MissionError::WorkPackageTooLarge {
                path: path.to_path_buf(),
                limit,
            },
            other => MissionError::kernel(path, other),
        })?;
        let source =
            std::str::from_utf8(&bytes).map_err(|error| MissionError::InvalidFrontmatter {
                path: path.to_path_buf(),
                detail: format!("not valid UTF-8: {error}"),
            })?;
        Self::parse(path, source)
    }

    /// Parse work-package Markdown already held in memory.
    ///
    /// `path` supplies the filename id check and error context only; it is not
    /// read or modified.
    pub fn parse(path: &Path, source: &str) -> Result<Self, MissionError> {
        let frontmatter = split_frontmatter(source).map_err(|error| match error {
            FrontmatterError::MissingOpeningDelimiter => MissionError::MissingFrontmatter {
                path: path.to_path_buf(),
            },
            FrontmatterError::Unterminated => MissionError::UnterminatedFrontmatter {
                path: path.to_path_buf(),
            },
        })?;

        let decoded: Value = serde_yaml_ng::from_str(frontmatter.yaml).map_err(|error| {
            MissionError::InvalidFrontmatter {
                path: path.to_path_buf(),
                detail: error.to_string(),
            }
        })?;
        let Value::Object(fields) = decoded else {
            return Err(MissionError::InvalidFrontmatter {
                path: path.to_path_buf(),
                detail: "frontmatter must decode to a YAML mapping".to_owned(),
            });
        };

        let id = parse_work_package_id(path, &fields)?;
        check_filename_id(path, &id)?;
        let dependencies = parse_dependencies(path, &fields)?;
        let title = fields
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_owned);

        Ok(Self {
            id,
            title,
            dependencies,
            source: path.to_path_buf(),
            frontmatter: fields,
        })
    }
}

fn parse_work_package_id(path: &Path, fields: &Map<String, Value>) -> Result<WpId, MissionError> {
    match fields.get("work_package_id") {
        None | Some(Value::Null) => Err(MissionError::MissingWorkPackageId {
            path: path.to_path_buf(),
        }),
        Some(Value::String(value)) => {
            WpId::from_str(value).map_err(|source| MissionError::InvalidWorkPackageId {
                path: path.to_path_buf(),
                value: value.clone(),
                source,
            })
        }
        Some(other) => Err(MissionError::InvalidWorkPackageId {
            path: path.to_path_buf(),
            value: other.to_string(),
            source: KernelError::InvalidWpId {
                value: other.to_string(),
            },
        }),
    }
}

/// Extract the `WP<digits>` id a filename claims, if it claims one.
///
/// Mirrors the upstream `^(WP\d{2})` rule: only a `WP` prefix followed by at
/// least two digits is a claim. A file named `WP01-title.md` claims `WP01`.
pub(crate) fn filename_wp_id(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let rest = name.strip_prefix("WP")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.len() < 2 {
        return None;
    }
    Some(format!("WP{digits}"))
}

fn check_filename_id(path: &Path, frontmatter_id: &WpId) -> Result<(), MissionError> {
    let Some(filename_id) = filename_wp_id(path) else {
        return Ok(());
    };
    if filename_id != frontmatter_id.as_str() {
        return Err(MissionError::WorkPackageIdMismatch {
            path: path.to_path_buf(),
            filename: filename_id,
            frontmatter: frontmatter_id.as_str().to_owned(),
        });
    }
    Ok(())
}

fn parse_dependencies(path: &Path, fields: &Map<String, Value>) -> Result<Vec<WpId>, MissionError> {
    let invalid = |detail: String| MissionError::InvalidDependencies {
        path: path.to_path_buf(),
        detail,
    };

    let raw = match fields.get("dependencies") {
        None | Some(Value::Null) => return Ok(Vec::new()),
        // Legacy fixtures contain the quoted string "[]" where a list was
        // intended. Accept exactly that spelling; every other string is
        // refused rather than guessed at.
        Some(Value::String(value)) if value.trim() == "[]" => return Ok(Vec::new()),
        Some(Value::String(value)) => {
            return Err(invalid(format!(
                "expected a list of work-package ids, found string {value:?}"
            )));
        }
        Some(Value::Array(values)) => values,
        Some(other) => {
            return Err(invalid(format!(
                "expected a list of work-package ids, found {}",
                crate::meta::json_kind(other)
            )));
        }
    };

    let mut dependencies = Vec::with_capacity(raw.len());
    for (index, value) in raw.iter().enumerate() {
        let Value::String(value) = value else {
            return Err(invalid(format!(
                "dependency at index {index} is {}; expected a work-package id string",
                crate::meta::json_kind(value)
            )));
        };
        let dependency = WpId::from_str(value).map_err(|source| {
            invalid(format!(
                "dependency at index {index} is not a canonical work-package id: {source}"
            ))
        })?;
        if dependencies.contains(&dependency) {
            return Err(invalid(format!(
                "dependency {dependency} is declared more than once"
            )));
        }
        dependencies.push(dependency);
    }
    Ok(dependencies)
}
