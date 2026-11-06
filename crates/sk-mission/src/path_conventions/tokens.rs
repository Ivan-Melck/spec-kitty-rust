//! Path-convention tokens: the Source normalization and suggestion vocabulary.
//!
//! Everything here is deterministic text work over authored values. Tokens are
//! [`PythonString`]s so an authored path keeps its exact code points, and memory
//! comparison (the dedup/artifact checks) is therefore code-point exact rather
//! than a lossy UTF-8 proxy.

use std::path::Path;

use sk_kernel::PythonString;
use sk_kernel::python_text::is_whitespace;

use crate::definition::error::DefinitionError;
use crate::definition::model::MissionDefinition;
use crate::definition::value::OrderedMap;

/// Source `normalize_path_token`: strip surrounding whitespace, then slashes.
///
/// Both strips are exactly Python's `str.strip()` / `str.strip("/")`: the first
/// removes Unicode whitespace code points (not ASCII only), the second removes
/// every leading and trailing `/`.
pub fn normalize_path_token(token: &PythonString) -> PythonString {
    let units = token.codepoints();
    let start = units
        .iter()
        .position(|code| !is_whitespace(*code))
        .unwrap_or(units.len());
    let end = units
        .iter()
        .rposition(|code| !is_whitespace(*code))
        .map(|position| position + 1)
        .unwrap_or(start);
    let trimmed = &units[start..end];
    let first = trimmed.iter().position(|code| *code != u32::from(b'/'));
    let last = trimmed.iter().rposition(|code| *code != u32::from(b'/'));
    match (first, last) {
        (Some(first), Some(last)) => {
            PythonString::from_codepoints(&trimmed[first..=last]).expect("sliced code points")
        }
        _ => PythonString::from_scalars(""),
    }
}

/// The normalized tokens of a definition's declared artifacts (required then
/// optional).
pub fn artifact_tokens_for(definition: &MissionDefinition) -> Vec<PythonString> {
    let mut tokens: Vec<PythonString> = definition
        .artifact_names()
        .map(normalize_path_token)
        .collect();
    let mut seen: Vec<PythonString> = Vec::new();
    tokens.retain(|token| {
        if seen.contains(token) {
            false
        } else {
            seen.push(token.clone());
            true
        }
    });
    tokens
}

/// Whether `token` is one of `tokens` (code-point exact).
pub fn contains_token(tokens: &[PythonString], token: &PythonString) -> bool {
    tokens.iter().any(|candidate| candidate == token)
}

/// Source `_prefix_required_path`: place a declared path under `prefix`, keeping
/// a directory hint and leaving absolute declarations alone.
///
/// `PurePosixPath(prefix) / relative` normalizes `.` segments and repeated
/// slashes, so the join is reproduced component-wise rather than through
/// `PathBuf` string concatenation.
pub fn prefix_required_path(
    prefix: Option<&PythonString>,
    relative_path: &PythonString,
) -> Result<PythonString, DefinitionError> {
    let Some(prefix) = prefix else {
        return Ok(relative_path.clone());
    };
    if is_absolute_posix(relative_path) {
        return Ok(relative_path.clone());
    }
    let scalar_prefix =
        prefix
            .to_scalar_string()
            .map_err(|_| DefinitionError::UnencodableAuthoredPath {
                value: prefix.clone(),
            })?;
    let trimmed_prefix = strip_slashes(strip_whitespace(&scalar_prefix));
    if trimmed_prefix.is_empty() {
        return Ok(relative_path.clone());
    }
    let scalar_relative =
        relative_path
            .to_scalar_string()
            .map_err(|_| DefinitionError::UnencodableAuthoredPath {
                value: relative_path.clone(),
            })?;
    let relative_trimmed = strip_slashes(&scalar_relative);
    let mut joined = normalize_posix_parts(trimmed_prefix);
    let relative_parts = normalize_posix_parts(relative_trimmed);
    if !relative_parts.is_empty() {
        if !joined.is_empty() {
            joined.push('/');
        }
        joined.push_str(&relative_parts);
    }
    let hint = scalar_relative.ends_with('/');
    let mut text = joined;
    if hint && !text.is_empty() {
        text.push('/');
    }
    Ok(PythonString::from_scalars(&text))
}

fn is_absolute_posix(value: &PythonString) -> bool {
    value.codepoints().first() == Some(&u32::from(b'/'))
}

fn strip_whitespace(text: &str) -> &str {
    text.trim_matches(|character: char| is_whitespace(u32::from(character)))
}

fn strip_slashes(text: &str) -> &str {
    text.trim_matches('/')
}

/// Drop empty and `.` segments, keeping every other segment verbatim.
fn normalize_posix_parts(text: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in text.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        parts.push(part);
    }
    parts.join("/")
}

/// Source `suggest_directory_creation`: shell-friendly remediation hints, in the
/// declared order, with the "one go" hint first when two or more directory
/// hints exist.
pub fn suggest_directory_creation(missing_paths: &[PythonString]) -> Vec<PythonString> {
    let mut suggestions: Vec<PythonString> = Vec::new();
    for path in missing_paths {
        let Some(text) = path.to_scalar_string().ok() else {
            // A path with an unpaired surrogate cannot be advised in shell text;
            // the missing-path warning above still names it exactly.
            continue;
        };
        if text.ends_with('/') {
            suggestions.push(PythonString::from_scalars(&format!("mkdir -p {text}")));
            continue;
        }
        let name = text.rsplit('/').next().unwrap_or(text.as_str());
        if name.contains('.') {
            match text.rsplit_once('/') {
                Some((parent, _)) if !parent.is_empty() && parent != "." => {
                    suggestions.push(PythonString::from_scalars(&format!(
                        "mkdir -p {parent} && touch {text}"
                    )));
                }
                _ => suggestions.push(PythonString::from_scalars(&format!("touch {text}"))),
            }
        } else {
            suggestions.push(PythonString::from_scalars(&format!("mkdir -p {text}")));
        }
    }

    let directory_paths: Vec<&PythonString> = missing_paths
        .iter()
        .filter(|path| path.codepoints().last() == Some(&u32::from(b'/')))
        .collect();
    if directory_paths.len() > 1 {
        let joined = directory_paths
            .iter()
            .filter_map(|path| path.to_scalar_string().ok())
            .collect::<Vec<String>>()
            .join(" ");
        suggestions.insert(
            0,
            PythonString::from_scalars(&format!("Create directories in one go: mkdir -p {joined}")),
        );
    }
    suggestions
}

/// The declared path map as an ordered list of pairs.
pub fn declared_pairs(paths: &OrderedMap<PythonString>) -> Vec<(PythonString, PythonString)> {
    paths
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// `str(PurePosixPath(value))` for a **relative** authored path.
///
/// Source builds artifact paths as `feature_dir / token` and renders the result
/// with `str(p.relative_to(feature_dir))`, i.e. through `pathlib`, which drops
/// empty and `.` segments and every surrounding/trailing slash but keeps every
/// other segment (including `..` and whitespace) verbatim. Callers reject an
/// absolute token first; this helper does not restore a leading `/`.
pub fn relative_posix_text(value: &str) -> String {
    normalize_posix_parts(strip_slashes(value))
}

/// Convert an authored path value into a filesystem path.
///
/// Authored values are UTF-8 text; a value carrying an unpaired surrogate has no
/// filesystem form here, which Source also fails to probe (`UnicodeEncodeError`).
/// The typed error keeps that boundary visible instead of lossily rewriting the
/// authored value.
pub fn authored_path(value: &PythonString) -> Result<std::path::PathBuf, DefinitionError> {
    let text = value
        .to_scalar_string()
        .map_err(|_| DefinitionError::UnencodableAuthoredPath {
            value: value.clone(),
        })?;
    Ok(std::path::PathBuf::from(text))
}

/// The Source-shaped text of a path (`str(Path)` on POSIX).
pub fn path_text(path: &Path) -> Result<PythonString, DefinitionError> {
    match path.to_str() {
        Some(text) => Ok(PythonString::from_scalars(text)),
        None => Err(DefinitionError::NonUtf8Path {
            path: path.to_path_buf(),
        }),
    }
}
