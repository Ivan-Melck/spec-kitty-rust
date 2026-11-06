//! `3_3_1_context_sources_consolidation`: retire the profile `context-sources` surface.
//!
//! Agent profiles that still author `context-sources` fail to load once the
//! schema drops the block (`extra="forbid"`), so every consumer-authored
//! `*.agent.yaml` is rewritten in lockstep: each id is *set-merged* onto the
//! matching `*-references` field (deduplicated by id, never appended twice),
//! non-edge keys (`doctrine-layers`, `additional`) are dropped with a named
//! note, and the block itself is removed.
//!
//! The edit is a round-trip CST edit ([`CstDocument`]): comments, quotes, key
//! order and layout survive; no profile is reserialized through a lossy
//! mapping. A malformed profile is skipped at scan time (never fatal), while a
//! read failure during `apply` is reported — exactly the Source posture.

use std::path::{Path, PathBuf};

use crate::error::InstallError;
use crate::upgrade::MigrationOutcome;
use crate::yaml_edit::{CstDocument, CstMapping, CstSequence, CstValue};

/// Rationale stamped onto a reference synthesised from a bare id.
const MIGRATED_RATIONALE: &str = "Migrated from the retired context-sources surface (mission doctrine-drg-silent-drop-boundary-01M0PE7E).";

/// `context-sources` key → (canonical `*-references` field, id key).
const MERGE_MAP: [(&str, &str, &str); 4] = [
    ("directives", "directive-references", "code"),
    ("tactics", "tactic-references", "id"),
    ("toolguides", "toolguide-references", "id"),
    ("styleguides", "styleguide-references", "id"),
];

/// `context-sources` keys with no DRG edge shape — dropped with a note.
const NON_EDGE_KEYS: [&str; 2] = ["doctrine-layers", "additional"];

const PROFILE_GLOB_SUFFIX: &str = ".agent.yaml";
const SKIP_DIRS: [&str; 4] = [".git", ".venv", "node_modules", ".worktrees"];

/// The result of consolidating one profile.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConsolidationOutcome {
    /// Whether the profile changed.
    pub changed: bool,
    /// `*-references` field → ids newly moved off `context-sources`.
    pub merged: Vec<(String, Vec<String>)>,
    /// `context-sources` non-edge entries dropped (`key:value`).
    pub dropped: Vec<String>,
}

impl ConsolidationOutcome {
    fn summary(&self) -> String {
        let merged: usize = self.merged.iter().map(|(_, ids)| ids.len()).sum();
        if self.dropped.is_empty() {
            return format!("merged {merged} refs, dropped 0 non-edge entries");
        }
        let mut dropped = self.dropped.clone();
        dropped.sort();
        format!(
            "merged {merged} refs, dropped {} non-edge entries ({})",
            self.dropped.len(),
            dropped.join(", ")
        )
    }
}

/// Set-merge one `context-sources` list onto its canonical `*-references` list.
fn merge_one(
    root: &CstMapping,
    sources: &CstMapping,
    cs_key: &str,
    ref_field: &str,
    id_key: &str,
) -> Vec<String> {
    let Some(raw_ids) = sources.get_string_list(cs_key) else {
        return Vec::new();
    };
    if raw_ids.is_empty() {
        return Vec::new();
    }
    let target = root.get_sequence(ref_field);
    let mut existing: Vec<String> = target
        .as_ref()
        .map(|sequence| reference_ids(sequence, id_key))
        .unwrap_or_default();
    let mut newly = Vec::new();
    let mut absent_yaml = String::new();
    for id in raw_ids {
        if existing.iter().any(|known| known == &id) {
            continue;
        }
        match &target {
            Some(sequence) => sequence.push_value(synthesise_entry(id_key, &id)),
            None => absent_yaml.push_str(&entry_yaml(id_key, &id)),
        }
        existing.push(id.clone());
        newly.push(id);
    }
    if !newly.is_empty() && target.is_none() {
        root.set_raw(ref_field, &absent_yaml);
    }
    newly
}

fn reference_ids(sequence: &CstSequence, id_key: &str) -> Vec<String> {
    sequence
        .values()
        .iter()
        .filter_map(|value| match value {
            CstValue::Mapping(mapping, _) => mapping.get_string(id_key),
            _ => None,
        })
        .collect()
}

fn entry_fields(id_key: &str, id: &str) -> Vec<(&'static str, String)> {
    if id_key == "code" {
        vec![
            ("code", id.to_string()),
            ("name", id.to_string()),
            ("rationale", MIGRATED_RATIONALE.to_string()),
        ]
    } else {
        vec![
            ("id", id.to_string()),
            ("rationale", MIGRATED_RATIONALE.to_string()),
        ]
    }
}

fn synthesise_entry(id_key: &str, id: &str) -> yaml_edit::YamlValue {
    let mut mapping = std::collections::BTreeMap::new();
    for (key, value) in entry_fields(id_key, id) {
        mapping.insert(
            key.to_string(),
            yaml_edit::YamlValue::Scalar(yaml_edit::ScalarValue::string(value)),
        );
    }
    yaml_edit::YamlValue::Mapping(mapping)
}

/// One block-sequence entry, rendered from typed scalars so a marker-shaped id
/// (`true`, `001`) stays the *string* Source wrote.
fn entry_yaml(id_key: &str, id: &str) -> String {
    let mut text = String::new();
    for (index, (key, value)) in entry_fields(id_key, id).iter().enumerate() {
        let rendered = yaml_edit::ScalarValue::string(value.clone()).to_yaml_string();
        if index == 0 {
            text.push_str(&format!("- {key}: {rendered}\n"));
        } else {
            text.push_str(&format!("  {key}: {rendered}\n"));
        }
    }
    text
}

/// Consolidate `context-sources` on `root` in place.
pub fn consolidate_profile(root: &CstMapping) -> ConsolidationOutcome {
    let Some(sources) = root.get_mapping("context-sources") else {
        return ConsolidationOutcome::default();
    };
    let mut outcome = ConsolidationOutcome {
        changed: true,
        ..ConsolidationOutcome::default()
    };
    for (cs_key, ref_field, id_key) in MERGE_MAP {
        let newly = merge_one(root, &sources, cs_key, ref_field, id_key);
        if !newly.is_empty() {
            outcome.merged.push((ref_field.to_string(), newly));
        }
    }
    for key in NON_EDGE_KEYS {
        if let Some(values) = sources.get_string_list(key) {
            for value in values {
                outcome.dropped.push(format!("{key}:{value}"));
            }
        }
    }
    root.remove("context-sources");
    outcome
}

/// Every consumer-authored `*.agent.yaml`, sorted.
pub fn profile_paths(project_root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    walk(project_root, &mut paths);
    paths.sort();
    paths
}

fn walk(directory: &Path, paths: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            if SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            walk(&path, paths);
        } else if file_type.is_file() && name.ends_with(PROFILE_GLOB_SUFFIX) {
            paths.push(path);
        }
    }
}

/// `detect`: any consumer profile still authors a `context-sources` mapping.
pub fn detect(project_root: &Path) -> Result<bool, InstallError> {
    for path in profile_paths(project_root) {
        let Some(text) = crate::migrations::completion_legacy::paths::read_text(&path) else {
            continue;
        };
        let Ok(document) = CstDocument::parse(&path, &text) else {
            continue;
        };
        if document.root().get_mapping("context-sources").is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `apply`: consolidate every profile that still authors `context-sources`.
pub fn apply(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let mut changes = Vec::new();
    let mut errors = Vec::new();
    for path in profile_paths(project_root) {
        let relative = path
            .strip_prefix(project_root)
            .map(|relative| relative.to_string_lossy().into_owned())
            .unwrap_or_else(|_| path.to_string_lossy().into_owned());
        let Some(text) = crate::migrations::completion_legacy::paths::read_text(&path) else {
            errors.push(format!("Failed to read {relative}"));
            continue;
        };
        let document = match CstDocument::parse(&path, &text) {
            Ok(document) => document,
            Err(error) => {
                errors.push(format!("Failed to read {relative}: {error}"));
                continue;
            }
        };
        if document.root().get_mapping("context-sources").is_none() {
            continue;
        }
        let outcome = consolidate_profile(&document.root());
        if !outcome.changed {
            continue;
        }
        let text = document.text();
        crate::migrations::completion_legacy::paths::write_text(&path, &text)?;
        changes.push(format!(
            "Consolidated context-sources in {relative} ({})",
            outcome.summary()
        ));
    }
    if changes.is_empty() && errors.is_empty() {
        changes.push("No consumer profile authored context-sources".to_string());
    }
    if !errors.is_empty() {
        // The runner treats a returned Err as a recorded failure; keep the
        // success path returning the changes and surface the errors as a
        // refusal, matching Source's `success=False`.
        return Err(InstallError::UpgradeRefused {
            detail: errors.join("; "),
        });
    }
    Ok(MigrationOutcome {
        changed: !changes.is_empty(),
        changes,
        warnings: Vec::new(),
        ..crate::upgrade::MigrationOutcome::default()
    })
}
