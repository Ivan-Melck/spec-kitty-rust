//! Project-surface repairs: stale generated prompts, config key rename, stale
//! overrides, terminology and the retired profile-context command.
//!
//! | id | effect |
//! | --- | --- |
//! | `2.0.1_fix_generated_command_templates` | deterministic text repairs in generated command prompts |
//! | `2.0.1_tool_config_key_rename` | rename `.kittify/config.yaml`'s legacy `agents:` key to `tools:` |
//! | `2.0.7_fix_stale_overrides` | drop `.kittify/overrides/` files byte-identical to the package default |
//! | `2.1.3_fix_planning_repository_terminology` | replace "planning repository" with "project root checkout" |
//! | `2.2.0_profile_context_deployment` | remove the retired `spec-kitty.profile-context` command file |
//!
//! The `.kittify/overrides/` sweep is the one place a *shared asset* is
//! deleted: ownership is exact byte identity with the immutable package
//! default (`Prover::CanonicalBytes`), never a name, a marker or a version
//! window. A genuine customisation differs and is preserved.

use std::path::{Path, PathBuf};

use crate::error::InstallError;
use crate::migrations::completion_legacy::agent_commands;
use crate::migrations::completion_legacy::assets;
use crate::migrations::completion_legacy::paths::{self, KITTIFY};
use crate::preserve::{Prover, RemovalVerdict, guard_destructive_removal};
use crate::upgrade::MigrationOutcome;

fn outcome(changed: bool, changes: Vec<String>, warnings: Vec<String>) -> MigrationOutcome {
    MigrationOutcome {
        changed,
        changes,
        warnings,
        ..crate::upgrade::MigrationOutcome::default()
    }
}

// ------------------------------- 2.0.1 fix generated command templates

/// The literal repairs (`REPLACEMENTS`), applied in declaration order.
pub const PROMPT_REPLACEMENTS: [(&str, &str); 5] = [
    (
        "spec-kitty agent check-prerequisites",
        "spec-kitty agent mission check-prerequisites",
    ),
    ("--require-tasks --include-tasks", "--include-tasks"),
    ("--require-tasks", "--include-tasks"),
    (
        "(Missing script command for sh)",
        "spec-kitty agent mission check-prerequisites --json --paths-only",
    ),
    ("main repository root", "primary repository checkout root"),
];

/// The stale sequential-merge phrasing the repair rewrites.
pub const STALE_MERGE_START: &str = "merges each ";
/// The single replacement the substring patterns produce.
pub const STALE_MERGE_REPLACEMENT: &str =
    "merges lane branches into the mission branch before landing the mission branch";

/// The `merges each …` sentence endings that mark stale wording.
pub const STALE_MERGE_ENDINGS: [&str; 3] = [
    " into main in sequence",
    " into the target branch in sequence",
    " lane branch into the mission branch before the target branch",
];

/// Whether `content` carries a stale marker (`detect`).
pub fn prompt_needs_repair(content: &str) -> bool {
    content.contains("spec-kitty agent check-prerequisites")
        || content.contains("--require-tasks")
        || content.contains("(Missing script command for sh)")
        || rewrite_stale_merge(content).1 > 0
}

/// Replace every `merges each … <ending>` span, greedily to the last ending on
/// the line (the pinned patterns' `.*` before the literal suffix).
fn rewrite_stale_merge(content: &str) -> (String, usize) {
    let mut rewrites = 0usize;
    let mut output = String::with_capacity(content.len());
    for line in content.split_inclusive('\n') {
        let trailing = line.ends_with('\n');
        let mut rewritten = line.trim_end_matches('\n').to_string();
        while let Some(start) = rewritten.find(STALE_MERGE_START) {
            let tail = &rewritten[start..];
            let best = STALE_MERGE_ENDINGS
                .iter()
                .filter_map(|ending| tail.rfind(ending).map(|index| (index, ending.len())))
                .max_by_key(|(index, _)| *index);
            let Some((index, length)) = best else {
                break;
            };
            let stop = start + index + length;
            rewritten = format!(
                "{}{}{}",
                &rewritten[..start],
                STALE_MERGE_REPLACEMENT,
                &rewritten[stop..]
            );
            rewrites += 1;
        }
        output.push_str(&rewritten);
        if trailing {
            output.push('\n');
        }
    }
    (output, rewrites)
}

/// `2.0.1_fix_generated_command_templates.detect`.
pub fn detect_fix_generated_command_templates(project_root: &Path) -> Result<bool, InstallError> {
    for file in agent_commands::generated_prompt_files(project_root, true) {
        if let Some(content) = paths::read_text(&file)
            && prompt_needs_repair(&content)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `2.0.1_fix_generated_command_templates.can_apply`: always safe.
pub fn can_apply_fix_generated_command_templates(
    _project_root: &Path,
) -> Result<Option<String>, InstallError> {
    Ok(None)
}

/// `2.0.1_fix_generated_command_templates.apply`.
pub fn apply_fix_generated_command_templates(
    project_root: &Path,
) -> Result<MigrationOutcome, InstallError> {
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    for file in agent_commands::generated_prompt_files(project_root, true) {
        let Some(original) = paths::read_text(&file) else {
            warnings.push(format!(
                "Skipped unreadable file {}",
                paths::relative_display(project_root, &file)
            ));
            continue;
        };
        let mut updated = original.clone();
        for (old, new) in PROMPT_REPLACEMENTS {
            updated = updated.replace(old, new);
        }
        let (rewritten, _) = rewrite_stale_merge(&updated);
        if rewritten == original {
            continue;
        }
        paths::write_text(&file, &rewritten)?;
        changes.push(format!(
            "Updated: {}",
            paths::relative_display(project_root, &file)
        ));
    }
    if changes.is_empty() {
        changes.push("No generated prompt files needed repair".to_string());
    }
    Ok(outcome(!changes.is_empty(), changes, warnings))
}

// ------------------------------------- 2.0.1 tool config key rename

fn config_path(project_root: &Path) -> PathBuf {
    project_root.join(KITTIFY).join("config.yaml")
}

fn parse_config_mapping(text: &str) -> Option<serde_yaml_ng::Mapping> {
    match serde_yaml_ng::from_str::<serde_yaml_ng::Value>(text) {
        Ok(serde_yaml_ng::Value::Mapping(mapping)) => Some(mapping),
        _ => None,
    }
}

/// `2.0.1_tool_config_key_rename.detect`.
pub fn detect_tool_config_key_rename(project_root: &Path) -> Result<bool, InstallError> {
    let path = config_path(project_root);
    let Some(text) = paths::read_text(&path) else {
        return Ok(false);
    };
    let Some(mapping) = parse_config_mapping(&text) else {
        return Ok(false);
    };
    let has_agents = mapping.contains_key(serde_yaml_ng::Value::from("agents"));
    let has_tools = mapping.contains_key(serde_yaml_ng::Value::from("tools"));
    Ok(has_agents && !has_tools)
}

/// `2.0.1_tool_config_key_rename.can_apply`: an unreadable config is a
/// "not applicable here" skip, not an apply failure.
pub fn can_apply_tool_config_key_rename(
    project_root: &Path,
) -> Result<Option<String>, InstallError> {
    let path = config_path(project_root);
    if !path.exists() {
        return Ok(None);
    }
    if !path.is_file() {
        return Ok(Some("config path exists but is not a file".to_string()));
    }
    match std::fs::read_to_string(&path) {
        Ok(_) => Ok(None),
        Err(error) => Ok(Some(format!("config file not readable: {error}"))),
    }
}

/// `2.0.1_tool_config_key_rename.apply`.
///
/// Source does `data["tools"] = data.pop("agents")`: the legacy key is removed
/// and its value is appended under the canonical name at the end of the
/// mapping. The moved block (key line, trailing key comment, nested entries)
/// keeps its own bytes; every other line stays where it is. The result is
/// re-parsed and must equal the original mapping with the one key renamed.
///
/// Divergence from ruamel, disclosed: ruamel re-dumps the whole document, so it
/// normalises a hand-edited document's indentation/sequence style. This editor
/// preserves the operator's layout instead. For a document already written in
/// the crate's own layout the bytes match Source exactly (see
/// `tests/migrations_tool_config_rename.rs`).
pub fn apply_tool_config_key_rename(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let path = config_path(project_root);
    if !path.exists() {
        return Ok(MigrationOutcome::unchanged(vec![
            "No config.yaml found".to_string(),
        ]));
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|error| InstallError::io("read config", &path, error))?;
    let Some(mapping) = parse_config_mapping(&text) else {
        return Err(InstallError::UpgradeRefused {
            detail: "Invalid YAML in config.yaml: root must be a mapping".to_string(),
        });
    };
    if mapping.contains_key(serde_yaml_ng::Value::from("tools")) {
        return Ok(MigrationOutcome::unchanged(vec![
            "Config already uses canonical `tools` key".to_string(),
        ]));
    }
    if !mapping.contains_key(serde_yaml_ng::Value::from("agents")) {
        return Ok(MigrationOutcome::unchanged(vec![
            "No legacy `agents` key found".to_string(),
        ]));
    }
    // Source (`data["tools"] = data.pop("agents")`) removes the legacy key and
    // appends it under the canonical name at the end of the mapping. The moved
    // block keeps its own bytes, including the trailing key comment.
    let Some(updated) = crate::yaml_edit::move_root_key_to_end(&path, &text, "agents", "tools")?
    else {
        return Ok(MigrationOutcome::unchanged(vec![
            "No legacy `agents` key found".to_string(),
        ]));
    };
    let Some(updated_mapping) = parse_config_mapping(&updated) else {
        return Err(InstallError::UpgradeRefused {
            detail: "Renaming `agents` to `tools` produced an unparseable config.yaml".to_string(),
        });
    };
    let mut expected = serde_yaml_ng::Mapping::new();
    for (key, value) in &mapping {
        if key == &serde_yaml_ng::Value::from("agents") {
            expected.insert(serde_yaml_ng::Value::from("tools"), value.clone());
        } else {
            expected.insert(key.clone(), value.clone());
        }
    }
    if updated_mapping != expected {
        return Err(InstallError::UpgradeRefused {
            detail: "Refusing to rewrite config.yaml: the renamed document does not round-trip"
                .to_string(),
        });
    }
    paths::write_text(&path, &updated)?;
    Ok(outcome(
        true,
        vec!["Renamed .kittify/config.yaml key `agents` -> `tools`".to_string()],
        Vec::new(),
    ))
}

// ------------------------------------------- 2.0.7 stale overrides

fn overrides_dir(project_root: &Path) -> PathBuf {
    project_root.join(KITTIFY).join("overrides")
}

/// The package counterpart of an overrides file: the mission-specific path
/// first, then the direct path under the package asset root.
fn package_counterpart(relative: &Path, package_root: &Path, mission: &str) -> Option<PathBuf> {
    let mission_specific = package_root.join(mission).join(relative);
    if mission_specific.is_file() {
        return Some(mission_specific);
    }
    let direct = package_root.join(relative);
    direct.is_file().then_some(direct)
}

fn override_files(project_root: &Path) -> Vec<(PathBuf, PathBuf)> {
    let overrides = overrides_dir(project_root);
    if !overrides.is_dir() {
        return Vec::new();
    }
    let mut found = Vec::new();
    let mut stack = vec![overrides.clone()];
    while let Some(directory) = stack.pop() {
        for entry in paths::entries(&directory) {
            let Ok(metadata) = std::fs::symlink_metadata(&entry) else {
                continue;
            };
            if metadata.is_dir() {
                stack.push(entry);
                continue;
            }
            if !metadata.is_file() {
                continue;
            }
            let Ok(relative) = entry.strip_prefix(&overrides) else {
                continue;
            };
            if paths::is_shared_asset(relative) {
                found.push((entry.clone(), relative.to_path_buf()));
            }
        }
    }
    found.sort();
    found
}

/// `2.0.7_fix_stale_overrides.detect`: at least one override is byte-identical
/// to the current package default.
pub fn detect_fix_stale_overrides(project_root: &Path) -> Result<bool, InstallError> {
    if !overrides_dir(project_root).exists() {
        return Ok(false);
    }
    let Some(package_root) = assets::package_asset_root() else {
        return Ok(false);
    };
    for (path, relative) in override_files(project_root) {
        let Some(counterpart) = package_counterpart(&relative, &package_root, "software-dev")
        else {
            continue;
        };
        let Ok(a) = std::fs::read(&path) else {
            continue;
        };
        let Ok(b) = std::fs::read(&counterpart) else {
            continue;
        };
        if a == b {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `2.0.7_fix_stale_overrides.can_apply`.
pub fn can_apply_fix_stale_overrides(project_root: &Path) -> Result<Option<String>, InstallError> {
    if overrides_dir(project_root).exists() {
        return Ok(None);
    }
    Ok(Some("No .kittify/overrides/ directory found".to_string()))
}

/// `2.0.7_fix_stale_overrides.apply`.
pub fn apply_fix_stale_overrides(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    if !overrides_dir(project_root).exists() {
        return Ok(MigrationOutcome::unchanged(vec![
            "No overrides directory — nothing to do".to_string(),
        ]));
    }
    let Some(package_root) = assets::package_asset_root() else {
        return Err(InstallError::UpgradeRefused {
            detail: "Cannot locate package assets — unable to compare overrides".to_string(),
        });
    };
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    for (path, relative) in override_files(project_root) {
        let Some(counterpart) = package_counterpart(&relative, &package_root, "software-dev")
        else {
            continue;
        };
        let Ok(canonical) = std::fs::read(&counterpart) else {
            continue;
        };
        let verdict = guard_destructive_removal(
            &path,
            project_root,
            &Prover::CanonicalBytes(canonical),
            false,
            None,
            false,
        )?;
        match verdict {
            RemovalVerdict::Owned { .. } => changes.push(format!(
                "Removed stale override: overrides/{}",
                relative.to_string_lossy()
            )),
            RemovalVerdict::Absent => {}
            RemovalVerdict::Preserved { diagnostic, .. }
            | RemovalVerdict::Archived { diagnostic, .. } => warnings.push(diagnostic),
        }
    }
    cleanup_empty_override_dirs(project_root);
    if changes.is_empty() {
        changes.push("No stale overrides found — nothing to do".to_string());
    }
    Ok(outcome(!changes.is_empty(), changes, warnings))
}

fn cleanup_empty_override_dirs(project_root: &Path) {
    let overrides = overrides_dir(project_root);
    let mut depth_first: Vec<PathBuf> = Vec::new();
    let mut stack = vec![overrides.clone()];
    while let Some(directory) = stack.pop() {
        depth_first.push(directory.clone());
        for child in paths::directories(&directory) {
            stack.push(child);
        }
    }
    depth_first.sort();
    for directory in depth_first.into_iter().rev() {
        paths::remove_if_empty(&directory);
    }
}

// ----------------------------- 2.1.3 planning repository terminology

/// The replacement pairs (`_OLD_TERMS`).
pub const TERMINOLOGY_REPLACEMENTS: [(&str, &str); 4] = [
    ("planning repository", "project root checkout"),
    ("planning repo", "project root"),
    ("Planning repository", "Project root checkout"),
    ("Planning repo", "Project root"),
];

fn terminology_files(project_root: &Path) -> Vec<PathBuf> {
    let mut hits = Vec::new();
    for directory in agent_commands::command_directories(project_root) {
        let agent_dir = project_root.join(&directory.directory);
        if !agent_dir.is_dir() {
            continue;
        }
        for file in paths::entries(&agent_dir) {
            let Some(name) = file
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
            else {
                continue;
            };
            if !(name.starts_with("spec-kitty.") && name.ends_with(".md")) || !file.is_file() {
                continue;
            }
            let Some(content) = paths::read_text(&file) else {
                continue;
            };
            if content.to_lowercase().contains("planning repository") {
                hits.push(file);
            }
        }
    }
    hits.sort();
    hits
}

/// `2.1.3_fix_planning_repository_terminology.detect`.
pub fn detect_planning_repository_terminology(project_root: &Path) -> Result<bool, InstallError> {
    Ok(!terminology_files(project_root).is_empty())
}

/// `2.1.3_fix_planning_repository_terminology.can_apply`.
pub fn can_apply_planning_repository_terminology(
    project_root: &Path,
) -> Result<Option<String>, InstallError> {
    if terminology_files(project_root).is_empty() {
        return Ok(Some(
            "No agent command files contain 'planning repository'".to_string(),
        ));
    }
    Ok(None)
}

/// `2.1.3_fix_planning_repository_terminology.apply`.
pub fn apply_planning_repository_terminology(
    project_root: &Path,
) -> Result<MigrationOutcome, InstallError> {
    let hits = terminology_files(project_root);
    if hits.is_empty() {
        return Ok(MigrationOutcome::unchanged(vec![
            "No files needed updating".to_string(),
        ]));
    }
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    for file in hits {
        let Some(original) = paths::read_text(&file) else {
            warnings.push(format!(
                "Could not update {}",
                paths::relative_display(project_root, &file)
            ));
            continue;
        };
        let mut content = original.clone();
        for (old, new) in TERMINOLOGY_REPLACEMENTS {
            content = content.replace(old, new);
        }
        if content == original {
            continue;
        }
        paths::write_text(&file, &content)?;
        changes.push(paths::relative_display(project_root, &file));
    }
    if changes.is_empty() {
        changes.push(
            "Updated 0 file(s): 'planning repository' -> 'project root checkout'".to_string(),
        );
    }
    Ok(outcome(true, changes, warnings))
}

// ------------------------------------- 2.2.0 profile-context deployment

/// The retired command file (`_DEST_FILENAME`).
pub const PROFILE_CONTEXT_COMMAND: &str = "spec-kitty.profile-context.md";

/// `2.2.0_profile_context_deployment.detect`.
pub fn detect_profile_context_deployment(project_root: &Path) -> Result<bool, InstallError> {
    for directory in agent_commands::command_directories(project_root) {
        let agent_dir = project_root.join(&directory.directory);
        if agent_dir.exists() && agent_dir.join(PROFILE_CONTEXT_COMMAND).exists() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `2.2.0_profile_context_deployment.can_apply`: always safe.
pub fn can_apply_profile_context_deployment(
    _project_root: &Path,
) -> Result<Option<String>, InstallError> {
    Ok(None)
}

/// `2.2.0_profile_context_deployment.apply`.
pub fn apply_profile_context_deployment(
    project_root: &Path,
) -> Result<MigrationOutcome, InstallError> {
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    let mut removed = 0usize;
    for directory in agent_commands::command_directories(project_root) {
        let agent_dir = project_root.join(&directory.directory);
        if !agent_dir.exists() {
            continue;
        }
        let destination = agent_dir.join(PROFILE_CONTEXT_COMMAND);
        if !destination.exists() {
            continue;
        }
        let verdict = guard_destructive_removal(
            &destination,
            project_root,
            &Prover::CanonicalContent,
            false,
            None,
            false,
        )?;
        match verdict {
            RemovalVerdict::Owned { .. } => {
                removed += 1;
                changes.push(format!(
                    "Removed retired {}",
                    paths::relative_display(project_root, &destination)
                ));
            }
            RemovalVerdict::Absent => {}
            RemovalVerdict::Preserved { diagnostic, .. }
            | RemovalVerdict::Archived { diagnostic, .. } => warnings.push(diagnostic),
        }
    }
    if removed == 0 && warnings.is_empty() {
        changes.push(format!(
            "{PROFILE_CONTEXT_COMMAND} absent from all configured agent dirs"
        ));
    }
    Ok(outcome(removed > 0, changes, warnings))
}
