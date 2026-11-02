//! Read-only parsing and loading of upstream glossary seed YAML.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde_yaml_ng::{Mapping, Value};

use crate::{
    GlossaryError, GlossaryScope, SeedFileError, SeedValidationError, TermMetadata, TermSense,
    TermStatus, TermSurface,
};

/// Maximum bytes accepted by the filesystem seed loader (8 MiB).
pub const MAX_SEED_BYTES: u64 = 8 * 1024 * 1024;

const ROOT_FIELDS: [&str; 1] = ["terms"];
const TERM_FIELDS: [&str; 7] = [
    "surface",
    "definition",
    "confidence",
    "status",
    "see_also",
    "synonyms_to_avoid",
    "introduced_in_mission",
];

/// Return the canonical seed path for a scope under a repository root.
pub fn seed_path(scope: GlossaryScope, repo_root: impl AsRef<Path>) -> PathBuf {
    repo_root
        .as_ref()
        .join(".kittify")
        .join("glossaries")
        .join(scope.filename())
}

/// Load one scope's seed file from `<repo_root>/.kittify/glossaries/`.
///
/// A missing file is a configured-but-empty scope and returns an empty vector.
/// The resolved seed path must stay inside `repo_root`; a symlink escape is
/// rejected with [`GlossaryError::PathEscape`]. Any other read failure, YAML
/// parse failure, or schema validation failure is returned as a typed error.
pub fn load_seed_file(
    scope: GlossaryScope,
    repo_root: impl AsRef<Path>,
) -> Result<Vec<TermSense>, GlossaryError> {
    let repo_root = repo_root.as_ref();
    let canonical_root = match std::fs::canonicalize(repo_root) {
        Ok(root) => root,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(GlossaryError::Io {
                action: "resolve glossary root",
                path: repo_root.to_path_buf(),
                source,
            });
        }
    };

    let path = seed_path(scope, repo_root);
    let canonical_seed = match std::fs::canonicalize(&path) {
        Ok(seed) => seed,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(GlossaryError::Io {
                action: "resolve glossary seed",
                path,
                source,
            });
        }
    };
    if !canonical_seed.starts_with(&canonical_root) {
        return Err(GlossaryError::PathEscape {
            root: canonical_root,
            path: canonical_seed,
        });
    }

    let bytes = sk_kernel::read_bounded(&canonical_seed, MAX_SEED_BYTES).map_err(|source| {
        GlossaryError::BoundedRead {
            path: path.clone(),
            source,
        }
    })?;

    let contents = match String::from_utf8(bytes) {
        Ok(contents) => contents,
        Err(error) => {
            let validation = SeedValidationError::new(
                path.clone(),
                None,
                None,
                None,
                format!("seed file must be valid UTF-8: {error}"),
            );
            return Err(SeedFileError::new(path, vec![validation]).into());
        }
    };

    parse_seed(scope, path, &contents).map_err(GlossaryError::from)
}

/// Parse seed YAML text for one scope.
///
/// The parser mirrors the upstream aggregate: a required `terms` key, a
/// `terms: null` value treated as an empty glossary, unknown fields rejected at
/// the root and term level, normalized surfaces, non-empty definitions,
/// numeric confidence in `0.0..=1.0`, the three-value status vocabulary, and
/// the accepted annotation fields (`see_also`, `synonyms_to_avoid`,
/// `introduced_in_mission`). All validation failures are collected.
pub fn parse_seed(
    scope: GlossaryScope,
    file_path: impl Into<PathBuf>,
    contents: &str,
) -> Result<Vec<TermSense>, SeedFileError> {
    let file_path = file_path.into();
    let value = match serde_yaml_ng::from_str::<Value>(contents) {
        Ok(value) => value,
        Err(error) => {
            let validation = SeedValidationError::new(
                file_path.clone(),
                None,
                None,
                None,
                format!("YAML parse error: {error}"),
            );
            return Err(SeedFileError::new(file_path, vec![validation]));
        }
    };

    let mut errors = Vec::new();
    let terms = match value.as_mapping() {
        Some(mapping) => parse_root(scope, &file_path, mapping, &mut errors),
        None => {
            errors.push(SeedValidationError::new(
                file_path.clone(),
                None,
                None,
                None,
                "seed file root must be a mapping",
            ));
            Vec::new()
        }
    };

    if errors.is_empty() {
        Ok(terms)
    } else {
        Err(SeedFileError::new(file_path, errors))
    }
}

fn parse_root(
    scope: GlossaryScope,
    file_path: &Path,
    mapping: &Mapping,
    errors: &mut Vec<SeedValidationError>,
) -> Vec<TermSense> {
    for key in mapping.keys() {
        if !key.as_str().is_some_and(|name| ROOT_FIELDS.contains(&name)) {
            let field = field_label(key);
            errors.push(SeedValidationError::new(
                file_path.to_path_buf(),
                None,
                None,
                Some(field.clone()),
                format!("unknown field '{field}'"),
            ));
        }
    }

    let Some(terms_value) = mapping.get("terms") else {
        errors.push(SeedValidationError::new(
            file_path.to_path_buf(),
            None,
            None,
            Some("terms".to_owned()),
            "must have 'terms' key",
        ));
        return Vec::new();
    };

    if terms_value.is_null() {
        return Vec::new();
    }

    let Some(terms) = terms_value.as_sequence() else {
        errors.push(SeedValidationError::new(
            file_path.to_path_buf(),
            None,
            None,
            Some("terms".to_owned()),
            "terms must be a sequence",
        ));
        return Vec::new();
    };

    let mut senses = Vec::new();
    for (index, item) in terms.iter().enumerate() {
        if let Some(sense) = parse_term(scope, file_path, index, item, errors) {
            senses.push(sense);
        }
    }
    senses
}

fn parse_term(
    scope: GlossaryScope,
    file_path: &Path,
    index: usize,
    item: &Value,
    errors: &mut Vec<SeedValidationError>,
) -> Option<TermSense> {
    let Some(mapping) = item.as_mapping() else {
        push_term_error(
            errors,
            file_path,
            index,
            None,
            None,
            "term must be a mapping",
        );
        return None;
    };

    let errors_before = errors.len();
    let raw_surface = mapping
        .get("surface")
        .and_then(Value::as_str)
        .map(str::to_owned);

    for key in mapping.keys() {
        if !key.as_str().is_some_and(|name| TERM_FIELDS.contains(&name)) {
            let field = field_label(key);
            push_term_error(
                errors,
                file_path,
                index,
                raw_surface.as_deref(),
                Some(&field),
                format!("unknown field '{field}'"),
            );
        }
    }

    let surface = match mapping.get("surface") {
        None => {
            push_term_error(
                errors,
                file_path,
                index,
                None,
                Some("surface"),
                "must have 'surface' key",
            );
            None
        }
        Some(Value::String(value)) => match TermSurface::parse(value) {
            Ok(surface) => Some(surface),
            Err(error) => {
                push_term_error(
                    errors,
                    file_path,
                    index,
                    raw_surface.as_deref(),
                    Some("surface"),
                    error.to_string(),
                );
                None
            }
        },
        Some(_) => {
            push_term_error(
                errors,
                file_path,
                index,
                None,
                Some("surface"),
                "surface must be a string",
            );
            None
        }
    };

    let definition = match mapping.get("definition") {
        None => {
            push_term_error(
                errors,
                file_path,
                index,
                raw_surface.as_deref(),
                Some("definition"),
                "must have 'definition' key",
            );
            None
        }
        Some(Value::String(value)) if value.trim().is_empty() => {
            push_term_error(
                errors,
                file_path,
                index,
                raw_surface.as_deref(),
                Some("definition"),
                "definition must not be empty",
            );
            None
        }
        Some(Value::String(value)) => Some(value.clone()),
        Some(_) => {
            push_term_error(
                errors,
                file_path,
                index,
                raw_surface.as_deref(),
                Some("definition"),
                "definition must be a string",
            );
            None
        }
    };

    let confidence = match mapping.get("confidence") {
        None => Some(1.0),
        Some(raw) => match raw.as_f64() {
            Some(value) if (0.0..=1.0).contains(&value) => Some(value),
            Some(value) => {
                push_term_error(
                    errors,
                    file_path,
                    index,
                    raw_surface.as_deref(),
                    Some("confidence"),
                    format!("confidence must be 0.0..1.0, got {value}"),
                );
                None
            }
            None => {
                push_term_error(
                    errors,
                    file_path,
                    index,
                    raw_surface.as_deref(),
                    Some("confidence"),
                    "confidence must be a number",
                );
                None
            }
        },
    };

    let status = match mapping.get("status") {
        None => Some(TermStatus::Draft),
        Some(Value::String(value)) => match TermStatus::from_str(value) {
            Ok(status) => Some(status),
            Err(error) => {
                push_term_error(
                    errors,
                    file_path,
                    index,
                    raw_surface.as_deref(),
                    Some("status"),
                    error.to_string(),
                );
                None
            }
        },
        Some(_) => {
            push_term_error(
                errors,
                file_path,
                index,
                raw_surface.as_deref(),
                Some("status"),
                "status must be a string",
            );
            None
        }
    };

    let see_also = parse_see_also(
        mapping.get("see_also"),
        file_path,
        index,
        raw_surface.as_deref(),
        errors,
    );
    let synonyms_to_avoid = parse_string_list(
        mapping.get("synonyms_to_avoid"),
        "synonyms_to_avoid",
        file_path,
        index,
        raw_surface.as_deref(),
        errors,
    );
    let introduced_in_mission = parse_optional_string(
        mapping.get("introduced_in_mission"),
        "introduced_in_mission",
        file_path,
        index,
        raw_surface.as_deref(),
        errors,
    );

    if errors.len() != errors_before {
        return None;
    }

    let (Some(surface), Some(definition), Some(confidence), Some(status)) =
        (surface, definition, confidence, status)
    else {
        return None;
    };

    match TermSense::new(surface, scope, definition, confidence, status) {
        Ok(sense) => Some(sense.with_metadata(TermMetadata {
            see_also,
            synonyms_to_avoid,
            introduced_in_mission,
        })),
        Err(error) => {
            push_term_error(
                errors,
                file_path,
                index,
                raw_surface.as_deref(),
                None,
                error.to_string(),
            );
            None
        }
    }
}

fn parse_see_also(
    value: Option<&Value>,
    file_path: &Path,
    index: usize,
    surface: Option<&str>,
    errors: &mut Vec<SeedValidationError>,
) -> Vec<BTreeMap<String, String>> {
    let Some(value) = value else {
        return Vec::new();
    };
    if value.is_null() {
        return Vec::new();
    }
    let Some(items) = value.as_sequence() else {
        push_term_error(
            errors,
            file_path,
            index,
            surface,
            Some("see_also"),
            "see_also must be a sequence of string maps",
        );
        return Vec::new();
    };

    let mut result = Vec::new();
    for item in items {
        let Some(mapping) = item.as_mapping() else {
            push_term_error(
                errors,
                file_path,
                index,
                surface,
                Some("see_also"),
                "see_also entries must be string maps",
            );
            continue;
        };

        let mut entries = BTreeMap::new();
        let mut valid = true;
        for (key, value) in mapping {
            let (Some(key), Some(value)) = (key.as_str(), value.as_str()) else {
                push_term_error(
                    errors,
                    file_path,
                    index,
                    surface,
                    Some("see_also"),
                    "see_also entries must map strings to strings",
                );
                valid = false;
                continue;
            };
            entries.insert(key.to_owned(), value.to_owned());
        }
        if valid {
            result.push(entries);
        }
    }
    result
}

fn parse_string_list(
    value: Option<&Value>,
    field: &str,
    file_path: &Path,
    index: usize,
    surface: Option<&str>,
    errors: &mut Vec<SeedValidationError>,
) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };
    if value.is_null() {
        return Vec::new();
    }
    let Some(items) = value.as_sequence() else {
        push_term_error(
            errors,
            file_path,
            index,
            surface,
            Some(field),
            format!("{field} must be a sequence of strings"),
        );
        return Vec::new();
    };

    let mut result = Vec::new();
    for item in items {
        match item.as_str() {
            Some(value) => result.push(value.to_owned()),
            None => push_term_error(
                errors,
                file_path,
                index,
                surface,
                Some(field),
                format!("{field} entries must be strings"),
            ),
        }
    }
    result
}

fn parse_optional_string(
    value: Option<&Value>,
    field: &str,
    file_path: &Path,
    index: usize,
    surface: Option<&str>,
    errors: &mut Vec<SeedValidationError>,
) -> Option<String> {
    let value = value?;
    if value.is_null() {
        return None;
    }
    match value.as_str() {
        Some(value) => Some(value.to_owned()),
        None => {
            push_term_error(
                errors,
                file_path,
                index,
                surface,
                Some(field),
                format!("{field} must be a string"),
            );
            None
        }
    }
}

fn push_term_error(
    errors: &mut Vec<SeedValidationError>,
    file_path: &Path,
    index: usize,
    surface: Option<&str>,
    field: Option<&str>,
    message: impl Into<String>,
) {
    errors.push(SeedValidationError::new(
        file_path.to_path_buf(),
        Some(index),
        surface.map(str::to_owned),
        field.map(str::to_owned),
        message,
    ));
}

fn field_label(key: &Value) -> String {
    key.as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{key:?}"))
}
