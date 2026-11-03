//! `4.0.0rc8_narrow_agent_gitignore`: replace whole-folder agent ignores.
//!
//! Spec Kitty used to add every configured (and every *possible*) agent folder
//! to `.gitignore`, hiding files a team commits on purpose. This migration:
//!
//! 1. removes only the agent lines Spec Kitty provably wrote, in the run after
//!    its auto-managed marker or the 0.4.8 header (a matching line elsewhere is
//!    the operator's own and stays, with a warning);
//! 2. rewrites the delimited agent-files block for the configured agents; and
//! 3. warns about untracked files the removed lines used to hide.
//!
//! The legacy entry list and the header strings are frozen here so "what Spec
//! Kitty wrote" never moves with a later release. The new block is live: it
//! follows the project's configured agents, the shipped built-in skills, the
//! pack skills its manifest records and its projected agent profiles.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::Value as JsonValue;
use serde_yaml_ng::Value as YamlValue;

use crate::agent::{agent_keys, agent_surface};
use crate::error::InstallError;
use crate::gitignore::{
    AGENT_SECRET_ENTRIES, AGENT_STATE_DIR_ENTRIES, COMMAND_FILE_PATTERN, GITIGNORE_MARKER,
    RESERVED_SKILL_DIR_PATTERNS, SHARED_SKILL_ROOT, replace_agent_block,
};
use crate::upgrade::MigrationOutcome;

use super::git;

/// Every whole-folder or narrow agent entry an earlier Spec Kitty wrote.
pub const LEGACY_AGENT_ENTRIES: [&str; 22] = [
    ".claude/",
    ".codex/",
    ".vibe/",
    ".pi/",
    ".letta/",
    ".opencode/",
    ".windsurf/",
    ".gemini/",
    ".llxprt/",
    ".cursor/",
    ".cursor/rules/spec-kitty.mdc",
    ".cursor/commands/",
    ".cursor/skills/",
    ".qwen/",
    ".kilocode/",
    ".augment/",
    ".roo/",
    ".amazonq/",
    ".kiro/",
    ".agent/",
    ".github/copilot/",
    ".agents/skills/",
];

const LEGACY_HEADER: &str = "# AI Agent directories (added by Spec Kitty CLI)";
const LEGACY_HEADER_NOTE: &str = "# These contain auth tokens and should NEVER be committed";
const RUNTIME_PREFIXES: [&str; 4] = [".kittify/", "kitty-specs/", "kitty-ops/", ".worktrees/"];
const BACKFILL_APPENDED: [&str; 6] = [
    ".agents/skills/",
    ".cursor/rules/spec-kitty.mdc",
    ".cursor/commands/",
    ".cursor/skills/",
    ".pi/",
    ".letta/",
];
const CURSOR_ENTRY: &str = ".cursor/";
const CURSOR_NEIGHBOURS: [&str; 2] = [".gemini/", ".qwen/"];
const MAX_LISTED_PATHS: usize = 20;

/// Shipped built-in skill names at the pinned baseline.
///
/// Python reads the installed catalog here. This migration is pinned to the
/// baseline's release and its block is written once; the frozen list keeps a
/// later catalog addition from silently changing what this migration hides.
const BUILTIN_SKILL_NAMES: [&str; 45] = [
    "adversarial-squad",
    "spec-kitty",
    "spec-kitty-git-workflow",
    "spec-kitty-implement-review",
    "spec-kitty-mission-review",
    "spec-kitty-mission-system",
    "spec-kitty-orchestrator-api-operator",
    "spec-kitty-program-orchestrate",
    "spec-kitty-runtime-next",
    "spec-kitty-runtime-review",
    "spec-kitty-setup-doctor",
    "spk-admin-agent-config",
    "spk-admin-git-workflow",
    "spk-admin-setup-doctor",
    "spk-admin-upgrade",
    "spk-charter-glossary",
    "spk-charter-governance",
    "spk-charter-profile-load",
    "spk-charter-spdd-reasons",
    "spk-gate-accept",
    "spk-gate-consolidate",
    "spk-gate-mission-review",
    "spk-gate-retrospective",
    "spk-integrate-orchestrator-api",
    "spk-meta-skill-authoring",
    "spk-meta-skill-map",
    "spk-mission-documentation",
    "spk-mission-plan",
    "spk-mission-research",
    "spk-mission-specify",
    "spk-mission-tasks",
    "spk-mission-types",
    "spk-practice-bulk-edit",
    "spk-practice-semantic-compression",
    "spk-practice-show-me",
    "spk-run-blocked-recovery",
    "spk-run-implement-review",
    "spk-run-next",
    "spk-run-program-orchestrate",
    "spk-run-review-wp",
    "spk-run-verdict-capture",
    "spk-start-agent-surface",
    "spk-start-command-map",
    "spk-start-first-feature",
    "spk-start-here",
];

/// The `.gitignore` path the migration edits.
pub fn gitignore_path(project_root: &Path) -> PathBuf {
    project_root.join(".gitignore")
}

fn folder_forms(folder: &str) -> Vec<String> {
    let bare = folder.trim_end_matches('/');
    let mut forms = Vec::new();
    for prefix in ["", "/", "**/"] {
        for suffix in ["", "/", "/*", "/**"] {
            forms.push(format!("{prefix}{bare}{suffix}"));
        }
    }
    forms
}

fn operator_folder_forms() -> Vec<(String, String)> {
    let mut forms = Vec::new();
    for folder in LEGACY_AGENT_ENTRIES {
        if folder.ends_with('/') {
            for form in folder_forms(folder) {
                forms.push((form, folder.to_string()));
            }
        }
    }
    forms
}

fn is_run_entry(stripped: &str) -> bool {
    LEGACY_AGENT_ENTRIES.contains(&stripped)
        || RUNTIME_PREFIXES
            .iter()
            .any(|prefix| stripped.starts_with(prefix))
}

fn is_cursor_row_proven(lines: &[String], index: usize) -> bool {
    index > 0
        && index + 1 < lines.len()
        && lines[index - 1].trim() == CURSOR_NEIGHBOURS[0]
        && lines[index + 1].trim() == CURSOR_NEIGHBOURS[1]
}

fn is_owned(lines: &[String], index: usize, in_run: bool, after_marker: bool) -> bool {
    let stripped = lines[index].trim();
    if stripped == CURSOR_ENTRY {
        return in_run && is_cursor_row_proven(lines, index);
    }
    if in_run && LEGACY_AGENT_ENTRIES.contains(&stripped) {
        return true;
    }
    after_marker && BACKFILL_APPENDED.contains(&stripped)
}

fn is_header(stripped: &str) -> bool {
    stripped == GITIGNORE_MARKER || stripped == LEGACY_HEADER || stripped == LEGACY_HEADER_NOTE
}

fn classify(lines: &[String]) -> (BTreeSet<usize>, Vec<String>) {
    let mut owned = BTreeSet::new();
    let mut operator_rules = Vec::new();
    let forms = operator_folder_forms();
    let mut in_run = false;
    let mut after_marker = false;
    let mut header_indexes = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let stripped = line.trim();
        if is_header(stripped) {
            in_run = true;
            after_marker |= stripped == GITIGNORE_MARKER;
            if stripped != GITIGNORE_MARKER {
                header_indexes.push(index);
            }
            continue;
        }
        if stripped.is_empty() {
            continue;
        }
        if stripped.starts_with('#') || !is_run_entry(stripped) {
            in_run = false;
        }
        if is_owned(lines, index, in_run, after_marker) {
            owned.insert(index);
        } else if let Some((_, folder)) = forms.iter().find(|(form, _)| form == stripped)
            && !AGENT_STATE_DIR_ENTRIES.contains(&folder.as_str())
        {
            operator_rules.push(stripped.to_string());
        }
    }
    owned.extend(orphaned_headers(lines, &header_indexes, &owned));
    owned.extend(emptied_markers(lines, &owned));
    (owned, operator_rules)
}

fn orphaned_headers(
    lines: &[String],
    header_indexes: &[usize],
    owned: &BTreeSet<usize>,
) -> BTreeSet<usize> {
    let mut orphaned = BTreeSet::new();
    for index in header_indexes {
        let first_entry = (index + 1..lines.len()).find(|candidate| {
            let stripped = lines[*candidate].trim();
            !stripped.is_empty() && stripped != LEGACY_HEADER && stripped != LEGACY_HEADER_NOTE
        });
        if first_entry.is_none_or(|first| owned.contains(&first)) {
            orphaned.insert(*index);
        }
    }
    orphaned
}

fn emptied_markers(lines: &[String], owned: &BTreeSet<usize>) -> BTreeSet<usize> {
    let mut emptied = BTreeSet::new();
    for (index, line) in lines.iter().enumerate() {
        if line.trim() != GITIGNORE_MARKER {
            continue;
        }
        let next_kept = (index + 1..lines.len())
            .find(|candidate| !owned.contains(candidate) && !lines[*candidate].trim().is_empty());
        if next_kept.is_none_or(|next| lines[next].trim().starts_with('#')) {
            emptied.insert(index);
        }
    }
    emptied
}

fn without(lines: &[String], removed: &BTreeSet<usize>) -> Vec<String> {
    let mut kept = Vec::new();
    let mut just_removed = false;
    for (index, line) in lines.iter().enumerate() {
        if removed.contains(&index) {
            just_removed = true;
            continue;
        }
        if just_removed
            && line.trim().is_empty()
            && kept
                .last()
                .is_some_and(|last: &String| last.trim().is_empty())
        {
            continue;
        }
        just_removed = false;
        kept.push(line.clone());
    }
    kept
}

fn configured_agents(project_root: &Path) -> Result<Vec<String>, InstallError> {
    let path = project_root.join(".kittify/config.yaml");
    if !path.exists() {
        return Ok(agent_keys().map(str::to_string).collect());
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|source| InstallError::io("read config", &path, source))?;
    let value: YamlValue =
        serde_yaml_ng::from_str(&text).map_err(|error| InstallError::ConfigInvalid {
            path: path.clone(),
            detail: format!("Invalid YAML in {}: {error}", path.display()),
        })?;
    let Some(root) = value.as_mapping() else {
        return Err(InstallError::ConfigInvalid {
            path: path.clone(),
            detail: format!(
                "Invalid config shape in {}: expected a YAML mapping at the top level",
                path.display()
            ),
        });
    };
    let mut section = root.get(YamlValue::from("agents"));
    if section.is_none_or(|value| value.as_mapping().is_some_and(|mapping| mapping.is_empty())) {
        section = root.get(YamlValue::from("tools"));
    }
    let Some(section) = section else {
        return Ok(agent_keys().map(str::to_string).collect());
    };
    let Some(section) = section.as_mapping() else {
        return Err(InstallError::ConfigInvalid {
            path: path.clone(),
            detail: format!(
                "Invalid config shape in {}: expected a YAML mapping for agents",
                path.display()
            ),
        });
    };
    if !section.contains_key(YamlValue::from("available")) {
        // A legacy project with no stated list keeps every agent.
        return Ok(agent_keys().map(str::to_string).collect());
    }
    let available = section
        .get(YamlValue::from("available"))
        .expect("contains_key checked");
    let mut agents = Vec::new();
    if let Some(single) = available.as_str() {
        agents.push(single.to_string());
    } else if let Some(sequence) = available.as_sequence() {
        for entry in sequence {
            let Some(agent) = entry.as_str() else {
                return Err(InstallError::ConfigInvalid {
                    path: path.clone(),
                    detail:
                        "Invalid agents.available in config.yaml: expected a list of agent keys"
                            .to_string(),
                });
            };
            agents.push(agent.to_string());
        }
    } else {
        return Err(InstallError::ConfigInvalid {
            path: path.clone(),
            detail: "Invalid agents.available in config.yaml: expected a list of agent keys"
                .to_string(),
        });
    }
    for agent in &agents {
        if agent_surface(agent).is_err() {
            return Err(InstallError::ConfigInvalid {
                path: path.clone(),
                detail: format!("Unknown agent key(s) in config.yaml: {agent}"),
            });
        }
    }
    Ok(agents)
}

fn is_reserved_skill_name(name: &str) -> bool {
    name == "spec-kitty"
        || name.starts_with("spec-kitty-")
        || name.starts_with("spec-kitty.")
        || name.starts_with("spk-")
}

fn whole_session_file(agent: &str) -> Option<&'static str> {
    match agent {
        "cursor" => Some(".cursor/rules/spec-kitty.mdc"),
        "windsurf" => Some(".windsurf/rules/spec-kitty.md"),
        "kiro" => Some(".kiro/steering/spec-kitty.md"),
        _ => None,
    }
}

fn manifest_pack_skill_dirs(project_root: &Path, agents: &[String]) -> BTreeSet<String> {
    let mut directories = BTreeSet::new();
    let path = project_root.join(".kittify/skills-manifest.json");
    let Ok(raw) = std::fs::read(&path) else {
        return directories;
    };
    let Ok(value) = serde_json::from_slice::<JsonValue>(&raw) else {
        return directories;
    };
    let Some(entries) = value.get("entries").and_then(JsonValue::as_array) else {
        return directories;
    };
    for entry in entries {
        let Some(agent) = entry.get("agent_key").and_then(JsonValue::as_str) else {
            continue;
        };
        if !agents.iter().any(|configured| configured == agent)
            || entry.get("origin").and_then(JsonValue::as_str) != Some("pack")
        {
            continue;
        }
        let Some(installed_path) = entry.get("installed_path").and_then(JsonValue::as_str) else {
            continue;
        };
        let Ok(surface) = agent_surface(agent) else {
            continue;
        };
        let path = Path::new(installed_path);
        for root in surface.skill.roots {
            let root_path = Path::new(root);
            let Ok(relative) = path.strip_prefix(root_path) else {
                continue;
            };
            let parts: Vec<_> = relative.components().collect();
            if parts.len() > 1 {
                let first = parts[0].as_os_str().to_string_lossy();
                let root = root.trim_end_matches('/');
                directories.insert(format!("{root}/{first}/"));
            }
        }
    }
    directories
}

fn profile_files(project_root: &Path) -> BTreeSet<String> {
    let mut files = BTreeSet::new();
    let path = project_root.join(".kittify/agent_profiles_manifest.json");
    let Ok(raw) = std::fs::read(&path) else {
        return files;
    };
    let Ok(value) = serde_json::from_slice::<JsonValue>(&raw) else {
        return files;
    };
    let Some(entries) = value.get("entries").and_then(JsonValue::as_array) else {
        return files;
    };
    for entry in entries {
        let Some(output) = entry.get("output_path").and_then(JsonValue::as_str) else {
            continue;
        };
        let output = Path::new(output);
        let absolute = if output.is_absolute() {
            output.to_path_buf()
        } else {
            project_root.join(output)
        };
        if let Ok(relative) = absolute.strip_prefix(project_root) {
            files.insert(relative.to_string_lossy().replace('\\', "/"));
        }
    }
    files
}

fn agent_entries(project_root: &Path) -> Result<Vec<String>, InstallError> {
    let agents = configured_agents(project_root)?;
    let builtin_names = BUILTIN_SKILL_NAMES;
    let mut generated: BTreeSet<String> = BTreeSet::new();
    for pattern in RESERVED_SKILL_DIR_PATTERNS {
        generated.insert(format!("{SHARED_SKILL_ROOT}{pattern}"));
    }
    for agent in &agents {
        let Ok(surface) = agent_surface(agent) else {
            continue;
        };
        if let Some(command) = &surface.command {
            generated.insert(format!("{}/{COMMAND_FILE_PATTERN}", command.directory));
        }
        if let Some(root) = surface.skill.primary_root() {
            let root = if root.ends_with('/') {
                root.to_string()
            } else {
                format!("{root}/")
            };
            for pattern in RESERVED_SKILL_DIR_PATTERNS {
                generated.insert(format!("{root}{pattern}"));
            }
            for name in builtin_names {
                if !is_reserved_skill_name(name) {
                    generated.insert(format!("{root}{name}/"));
                }
            }
        }
        if let Some(session_file) = whole_session_file(agent) {
            generated.insert(session_file.to_string());
        }
    }
    generated.extend(manifest_pack_skill_dirs(project_root, &agents));
    generated.extend(profile_files(project_root));
    let mut entries: Vec<String> = AGENT_SECRET_ENTRIES
        .iter()
        .chain(AGENT_STATE_DIR_ENTRIES.iter())
        .map(|entry| (*entry).to_string())
        .collect();
    let fixed: BTreeSet<String> = entries.iter().cloned().collect();
    entries.extend(generated.into_iter().filter(|entry| !fixed.contains(entry)));
    Ok(entries)
}

fn read_gitignore(path: &Path) -> Result<Option<String>, InstallError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(InstallError::IgnoreFileUnsafe {
            path: path.to_path_buf(),
            detail: "Refusing to read symlinked .gitignore".to_string(),
        }),
        Ok(metadata) if !metadata.file_type().is_file() => Err(InstallError::IgnoreFileUnsafe {
            path: path.to_path_buf(),
            detail: "Could not safely open .gitignore: not a regular file".to_string(),
        }),
        Ok(_) => {
            let raw = std::fs::read(path)
                .map_err(|source| InstallError::io("read .gitignore", path, source))?;
            let text = String::from_utf8(raw).map_err(|error| InstallError::IgnoreFileUnsafe {
                path: path.to_path_buf(),
                detail: format!(".gitignore is not valid UTF-8: {error}"),
            })?;
            Ok(Some(
                text.strip_prefix('\u{feff}').unwrap_or(&text).to_string(),
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(InstallError::io("inspect .gitignore", path, source)),
    }
}

struct Plan {
    content: Option<String>,
    updated: String,
    removed: Vec<String>,
    operator_rules: Vec<String>,
}

fn plan(project_root: &Path) -> Result<Plan, InstallError> {
    let path = gitignore_path(project_root);
    let content = read_gitignore(&path)?;
    let text = content.clone().unwrap_or_default();
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    let (owned, operator_rules) = classify(&lines);
    let kept = without(&lines, &owned);
    let narrowed = if kept.is_empty() {
        String::new()
    } else {
        format!("{}{newline}", kept.join(newline))
    };
    let entries = agent_entries(project_root)?;
    let updated = replace_agent_block(Some(&narrowed), &entries, true)?.unwrap_or(narrowed);
    let removed = owned
        .iter()
        .filter_map(|index| {
            let stripped = lines[*index].trim();
            stripped.starts_with('.').then(|| stripped.to_string())
        })
        .collect();
    Ok(Plan {
        content,
        updated,
        removed,
        operator_rules,
    })
}

fn visibility_warning(project_root: &Path, removed: &[String]) -> Option<String> {
    if removed.is_empty() {
        return None;
    }
    let mut pathspecs: Vec<&str> = removed.iter().map(String::as_str).collect();
    pathspecs.sort_unstable();
    pathspecs.dedup();
    let mut args = vec![
        "status",
        "--porcelain=v1",
        "-z",
        "--untracked-files=all",
        "--",
    ];
    args.extend(pathspecs.iter().copied());
    let output = git::run(project_root, &args).ok()?;
    if !output.status.success() {
        return None;
    }
    let mut paths: Vec<String> = output
        .stdout
        .split(|byte| *byte == 0)
        .filter_map(|entry| {
            let entry = String::from_utf8_lossy(entry);
            entry
                .strip_prefix("?? ")
                .map(|path| path.trim().to_string())
        })
        .filter(|path| !path.is_empty())
        .collect();
    paths.sort();
    paths.dedup();
    if paths.is_empty() {
        return None;
    }
    let listed = paths
        .iter()
        .take(MAX_LISTED_PATHS)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let more = if paths.len() > MAX_LISTED_PATHS {
        format!(" and {} more", paths.len() - MAX_LISTED_PATHS)
    } else {
        String::new()
    };
    Some(format!(
        "Git now shows {} file(s) that the old agent ignore rules hid: {listed}{more}. Review them, then commit the ones your team shares or add your own ignore rule for the rest.",
        paths.len()
    ))
}

fn operator_rule_warnings(rules: &[String]) -> Vec<String> {
    let forms = operator_folder_forms();
    let unique: BTreeSet<String> = rules.iter().cloned().collect();
    let mut warnings = Vec::new();
    for rule in unique {
        let folder = forms
            .iter()
            .find(|(form, _)| form == &rule)
            .map(|(_, folder)| folder.clone())
            .unwrap_or_else(|| rule.clone());
        warnings.push(format!(
            "Kept your own ignore rule '{rule}' (Spec Kitty did not write it); remove it if your team commits files under {folder}"
        ));
    }
    warnings
}

/// Detection: the project's `.gitignore` would change.
pub fn detect(project_root: &Path) -> Result<bool, InstallError> {
    match plan(project_root) {
        Ok(plan) => Ok(plan.content.as_deref() != Some(plan.updated.as_str())),
        Err(_) => Ok(true),
    }
}

/// `can_apply`: a readable config and a safely readable `.gitignore`.
pub fn can_apply(project_root: &Path) -> Result<Option<String>, InstallError> {
    if !project_root.exists() {
        return Ok(Some(format!(
            "Project path does not exist: {}",
            project_root.display()
        )));
    }
    match plan(project_root) {
        Ok(_) => Ok(None),
        Err(error) => Ok(Some(format!(".gitignore cannot be narrowed: {error}"))),
    }
}

/// Apply the narrowing.
pub fn apply(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let plan = plan(project_root)?;
    let mut changes: Vec<String> = plan
        .removed
        .iter()
        .map(|line| format!("Removed agent ignore line: '{line}'"))
        .collect();
    let block_changed = plan.content.as_deref() != Some(plan.updated.as_str());
    let mut warnings = operator_rule_warnings(&plan.operator_rules);
    if block_changed {
        changes.push("Wrote the Spec Kitty agent-files block in .gitignore".to_string());
        let path = gitignore_path(project_root);
        sk_kernel::atomic_write(&path, plan.updated.as_bytes()).map_err(|error| {
            InstallError::io("write .gitignore", &path, std::io::Error::other(error))
        })?;
        if let Some(visibility) = visibility_warning(project_root, &plan.removed) {
            warnings.push(visibility);
        }
    }
    let changed = !plan.removed.is_empty() || block_changed;
    if changes.is_empty() {
        changes.push("Agent ignore rules already narrow".to_string());
    }
    Ok(MigrationOutcome {
        changed,
        changes,
        warnings,
    })
}
