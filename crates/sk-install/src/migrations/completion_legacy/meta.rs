//! Mission-metadata migrations.
//!
//! | id | effect |
//! | --- | --- |
//! | `0.13.0_research_csv_schema_check` | report research CSV header mismatches (informational; no file is modified) |
//! | `0.13.8_target_branch` | add `target_branch` to `kitty-specs/NNN-*/meta.json` |
//!
//! The `meta.json` write is a field-add, never a reserialization of fields the
//! migration does not own: the file's existing key order is kept and only the
//! missing `target_branch` is appended, so comments-adjacent structure and any
//! extra user keys survive. A malformed file is reported per Mission exactly as
//! the source splits "Malformed JSON" from "Failed to update".

use std::path::{Path, PathBuf};

use crate::error::InstallError;
use crate::migrations::completion_legacy::paths::{self, KITTY_SPECS};
use crate::migrations::git;
use crate::upgrade::MigrationOutcome;

/// `EVIDENCE_REQUIRED_COLUMNS`.
pub const EVIDENCE_REQUIRED_COLUMNS: [&str; 6] = [
    "timestamp",
    "source_type",
    "citation",
    "key_finding",
    "confidence",
    "notes",
];

/// `SOURCE_REGISTER_REQUIRED_COLUMNS`.
pub const SOURCE_REGISTER_REQUIRED_COLUMNS: [&str; 6] = [
    "source_id",
    "citation",
    "url",
    "accessed_date",
    "relevance",
    "status",
];

/// The branch candidates the source's `_BRANCH_PATTERNS` extract.
const BRANCH_PATTERN_PREFIXES: [&str; 4] = [
    "**target branch**:",
    "**base branch**:",
    "target repo branch:",
    "branch:",
];

// ------------------------------------------- 0.13.0 research CSV schema check

/// The header row of a CSV document, with whitespace stripped and `#`-comment
/// cells dropped (`validate_csv_schema`).
pub fn csv_header(bytes: &[u8]) -> Option<Vec<String>> {
    let text = String::from_utf8_lossy(bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let first_row = text.split('\n').next()?;
    let mut cells: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut chars = first_row.trim_end_matches('\r').chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '"' if quoted => {
                if chars.peek() == Some(&'"') {
                    current.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            }
            '"' => quoted = true,
            ',' if !quoted => {
                cells.push(std::mem::take(&mut current));
            }
            other => current.push(other),
        }
    }
    cells.push(current);
    Some(
        cells
            .into_iter()
            .map(|cell| cell.trim().to_string())
            .filter(|cell| !cell.is_empty() && !cell.starts_with('#'))
            .collect(),
    )
}

/// Whether a CSV file's header matches `expected` exactly (names and order).
pub fn schema_matches(path: &Path, expected: &[&str]) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    csv_header(&bytes).is_some_and(|header| header == expected)
}

/// The research mission of `feature_dir`, resolved through the canonical
/// `meta.json` reader (a malformed file is skipped, not an error).
fn research_mission_name(feature_dir: &Path) -> Option<String> {
    let bytes = std::fs::read(feature_dir.join("meta.json")).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    value
        .get("mission")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

fn research_features(project_root: &Path) -> Vec<PathBuf> {
    let specs = project_root.join(KITTY_SPECS);
    if !specs.is_dir() {
        return Vec::new();
    }
    paths::directories(&specs)
        .into_iter()
        .filter(|feature| feature.join("meta.json").is_file())
        .filter(|feature| research_mission_name(feature).as_deref() == Some("research"))
        .collect()
}

fn research_csvs() -> [(&'static str, &'static [&'static str]); 2] {
    [
        ("evidence-log.csv", &EVIDENCE_REQUIRED_COLUMNS),
        ("source-register.csv", &SOURCE_REGISTER_REQUIRED_COLUMNS),
    ]
}

/// `0.13.0_research_csv_schema_check.detect`.
pub fn detect_research_csv_schema_check(project_root: &Path) -> Result<bool, InstallError> {
    for feature in research_features(project_root) {
        for (name, expected) in research_csvs() {
            let path = feature.join("research").join(name);
            if path.exists() && !schema_matches(&path, expected) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// `0.13.0_research_csv_schema_check.can_apply`: informational, always safe.
pub fn can_apply_research_csv_schema_check(
    _project_root: &Path,
) -> Result<Option<String>, InstallError> {
    Ok(None)
}

/// `0.13.0_research_csv_schema_check.apply` — reports, modifies nothing.
pub fn apply_research_csv_schema_check(
    project_root: &Path,
) -> Result<MigrationOutcome, InstallError> {
    if !project_root.join(KITTY_SPECS).is_dir() {
        return Ok(MigrationOutcome::unchanged(vec![
            "No kitty-specs/ directory found - nothing to check".to_string(),
        ]));
    }
    let mut mismatches: Vec<String> = Vec::new();
    for feature in research_features(project_root) {
        for (name, expected) in research_csvs() {
            let path = feature.join("research").join(name);
            if !path.exists() || schema_matches(&path, expected) {
                continue;
            }
            let actual = std::fs::read(&path)
                .ok()
                .and_then(|bytes| csv_header(&bytes))
                .unwrap_or_default();
            let feature_name = feature
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            mismatches.push(format!(
                "{feature_name}/research/{name}: expected [{}], actual [{}]",
                expected.join(","),
                if actual.is_empty() {
                    "<missing>".to_string()
                } else {
                    actual.join(",")
                }
            ));
        }
    }
    if mismatches.is_empty() {
        return Ok(MigrationOutcome::unchanged(vec![
            "All research CSV schemas are correct".to_string(),
        ]));
    }
    let mut warnings = mismatches;
    warnings.push(
        "Research CSV schema mismatches are informational only - no files were modified. \
         Use LLM agents to help migrate data to the canonical schemas."
            .to_string(),
    );
    Ok(MigrationOutcome {
        changed: false,
        changes: vec![format!(
            "Detected {} schema mismatch(es)",
            warnings.len() - 1
        )],
        warnings,
        ..crate::upgrade::MigrationOutcome::default()
    })
}

// -------------------------------------------------------- 0.13.8 target branch

/// The numbered Mission folders (`kitty-specs/[0-9][0-9][0-9]-*/`).
fn numbered_missions(project_root: &Path) -> Vec<PathBuf> {
    let specs = project_root.join(KITTY_SPECS);
    if !specs.is_dir() {
        return Vec::new();
    }
    paths::directories(&specs)
        .into_iter()
        .filter(|feature| {
            let name = feature
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let bytes = name.as_bytes();
            bytes.len() >= 4 && bytes[..3].iter().all(u8::is_ascii_digit) && bytes[3] == b'-'
        })
        .collect()
}

/// `0.13.8_target_branch.detect`.
pub fn detect_target_branch(project_root: &Path) -> Result<bool, InstallError> {
    for feature in numbered_missions(project_root) {
        let meta_file = feature.join("meta.json");
        if !meta_file.exists() {
            continue;
        }
        if let Ok(bytes) = std::fs::read(&meta_file)
            && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes)
            && value.is_object()
            && value.get("target_branch").is_none()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `0.13.8_target_branch.can_apply`: always safe (fields are only added).
pub fn can_apply_target_branch(_project_root: &Path) -> Result<Option<String>, InstallError> {
    Ok(None)
}

/// `resolve_primary_branch`: `origin/HEAD`, then the checked-out branch, then
/// the common-branch ladder, then `main`.
pub fn resolve_primary_branch(repo_root: &Path) -> String {
    if let Some(text) =
        git::success_stdout(repo_root, &["symbolic-ref", "refs/remotes/origin/HEAD"])
            .ok()
            .flatten()
        && let Some(name) = text.rsplit('/').next()
        && !name.is_empty()
    {
        return name.to_string();
    }
    if let Some(current) = git::success_stdout(repo_root, &["branch", "--show-current"])
        .ok()
        .flatten()
        && !current.is_empty()
    {
        return current;
    }
    for branch in ["main", "master", "develop"] {
        let exists = git::run(repo_root, &["rev-parse", "--verify", branch])
            .map(|output| output.status.success())
            .unwrap_or(false);
        if exists {
            return branch.to_string();
        }
    }
    "main".to_string()
}

fn branch_candidates(feature_dir: &Path) -> Vec<String> {
    let mut candidates: Vec<String> = Vec::new();
    for name in ["spec.md", "plan.md", "tasks.md", "quickstart.md"] {
        let document = feature_dir.join(name);
        let Ok(bytes) = std::fs::read(&document) else {
            continue;
        };
        let content = String::from_utf8_lossy(&bytes).into_owned();
        for line in content.lines() {
            let trimmed = line.trim();
            let lowered = trimmed.to_lowercase();
            let raw = BRANCH_PATTERN_PREFIXES
                .iter()
                .find_map(|prefix| {
                    lowered
                        .starts_with(prefix)
                        .then(|| trimmed[prefix.len()..].trim().to_string())
                })
                .or_else(|| {
                    // The narrative patterns ("must be done on ... `branch`",
                    // "merge back to `branch`"): the first backticked token on a
                    // line that talks about a branch.
                    if !lowered.contains("branch") {
                        return None;
                    }
                    let start = trimmed.find('`')?;
                    let rest = &trimmed[start + 1..];
                    let end = rest.find('`')?;
                    Some(rest[..end].to_string())
                });
            let Some(raw) = raw else {
                continue;
            };
            if let Some(candidate) = normalize_branch_candidate(&raw)
                && !candidates.contains(&candidate)
            {
                candidates.push(candidate);
            }
        }
    }
    candidates
}

fn normalize_branch_candidate(value: &str) -> Option<String> {
    let cleaned = value.trim();
    if cleaned.is_empty() || cleaned.to_lowercase().contains(" or ") {
        return None;
    }
    let cleaned = cleaned.trim_matches(|character: char| {
        matches!(
            character,
            '`' | '\'' | '"' | '*' | '[' | ']' | '(' | ')' | ' '
        )
    });
    let allowed = |character: char| {
        character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '/' | '-')
    };
    let candidate: String = cleaned
        .chars()
        .take_while(|character| allowed(*character))
        .collect();
    if candidate.is_empty() {
        None
    } else {
        Some(candidate)
    }
}

/// `feature_meta.infer_target_branch`: the single documented branch, else the
/// primary branch (a document naming several branches only auto-detects when
/// the primary branch is one of them).
pub fn infer_target_branch(feature_dir: &Path, repo_root: &Path) -> String {
    let fallback = resolve_primary_branch(repo_root);
    let candidates = branch_candidates(feature_dir);
    if candidates.len() == 1 {
        return candidates[0].clone();
    }
    if candidates.len() > 1 && candidates.contains(&fallback) {
        return fallback;
    }
    fallback
}

/// Add `target_branch` to a `meta.json` object without touching any other field.
fn add_target_branch(value: &mut serde_json::Value, target_branch: &str) -> bool {
    let Some(map) = value.as_object_mut() else {
        return false;
    };
    if map.contains_key("target_branch") {
        return false;
    }
    map.insert(
        "target_branch".to_string(),
        serde_json::Value::String(target_branch.to_string()),
    );
    true
}

/// `0.13.8_target_branch.apply`.
pub fn apply_target_branch(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    if !project_root.join(KITTY_SPECS).exists() {
        return Ok(MigrationOutcome::unchanged(vec![
            "No features found, skipping target_branch addition".to_string(),
        ]));
    }
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    for feature in numbered_missions(project_root) {
        let meta_file = feature.join("meta.json");
        if !meta_file.exists() {
            continue;
        }
        let feature_name = feature
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let bytes = match std::fs::read(&meta_file) {
            Ok(bytes) => bytes,
            Err(error) => {
                errors.push(format!(
                    "Failed to update {feature_name}/meta.json: {error}"
                ));
                continue;
            }
        };
        let mut value: serde_json::Value = match serde_json::from_slice::<serde_json::Value>(&bytes)
        {
            Ok(value) if value.is_object() => value,
            Ok(_) => {
                errors.push(format!(
                    "Malformed JSON in {feature_name}/meta.json: top level is not an object"
                ));
                continue;
            }
            Err(error) => {
                errors.push(format!(
                    "Malformed JSON in {feature_name}/meta.json: {error}"
                ));
                continue;
            }
        };
        if value.get("target_branch").is_some() {
            continue;
        }
        let target_branch = infer_target_branch(&feature, project_root);
        if target_branch != "main" {
            warnings.push(format!(
                "{feature_name} auto-detected target_branch={target_branch}"
            ));
        }
        add_target_branch(&mut value, &target_branch);
        let mut text =
            serde_json::to_string_pretty(&value).map_err(|error| InstallError::UpgradeRefused {
                detail: format!("could not serialize {feature_name}/meta.json: {error}"),
            })?;
        text.push('\n');
        paths::write_text(&meta_file, &text)?;
        changes.push(format!(
            "Added target_branch={target_branch} to {feature_name}"
        ));
    }
    Ok(MigrationOutcome {
        changed: !changes.is_empty(),
        changes,
        warnings: warnings.into_iter().chain(errors).collect(),
        ..crate::upgrade::MigrationOutcome::default()
    })
}

/// The metadata reader shared with the consistency sweep: an object, or `None`.
pub fn load_meta(feature_dir: &Path) -> Option<serde_json::Value> {
    let bytes = std::fs::read(feature_dir.join("meta.json")).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    value.is_object().then_some(value)
}
