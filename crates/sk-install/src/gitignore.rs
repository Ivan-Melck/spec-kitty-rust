//! `.gitignore` protection for runtime state and generated agent files.
//!
//! Upstream (`gitignore_manager.py` + `agent_utils/ignore_entries.py`) writes
//! three things:
//!
//! 1. the auto-managed runtime entries from the state contract (so a project
//!    never commits runtime state, dossiers, derived views or migration
//!    bookkeeping);
//! 2. a `.worktrees/` entry;
//! 3. one delimited **agent-files block**, rewritten in place from the
//!    configured agents. Whole agent folders are *not* ignored (a team commits
//!    its own `.claude/settings.json`); only Spec Kitty's narrow generated
//!    paths, credential files and state folders are.
//!
//! The block delimiters are part of the contract. A damaged block (a begin
//! line without its end, or two blocks) is a typed refusal, never a guess: a
//! wrong guess either stops protecting generated files or hides operator
//! content.
//!
//! Deliberately not implemented here: the state-contract derivation itself (the
//! runtime entry list is the canonical, checked-in projection of that
//! contract, asserted against the oracle fixture in `tests/gitignore.rs`) and
//! the informational "agent folder security" panel, whose entries are exposed
//! as data by [`agent_folder_notice`] for the CLI to render.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::agent::{AgentSurface, agent_keys, agent_surface};
use crate::error::InstallError;

/// The marker line written above Spec Kitty's runtime entries.
pub const GITIGNORE_MARKER: &str = "# Added by Spec Kitty CLI (auto-managed)";

/// Begin delimiter of the agent-files block.
pub const AGENT_BLOCK_BEGIN: &str =
    "# >>> Spec Kitty agent files (auto-managed; lines in this block are replaced) >>>";

/// End delimiter of the agent-files block.
pub const AGENT_BLOCK_END: &str = "# <<< Spec Kitty agent files <<<";

/// Runtime state entries (the canonical projection of `state/contract.py`).
pub const RUNTIME_ENTRIES: [&str; 16] = [
    ".kittify/charter/context-state.json",
    ".kittify/derived/",
    ".kittify/dossiers/",
    ".kittify/encoding-provenance/",
    ".kittify/events/",
    ".kittify/lint-report.json",
    ".kittify/logs/",
    ".kittify/merge-state.json",
    ".kittify/migrations/",
    ".kittify/missions/__pycache__/",
    ".kittify/runtime/",
    ".kittify/skills-manifest.json",
    ".kittify/sync-state.json",
    ".kittify/workspaces/",
    ".worktrees/",
    "kitty-ops/ops-index.jsonl",
];

/// Feature-nested runtime entry (kept separate to preserve the oracle order).
pub const FEATURE_RUNTIME_ENTRY: &str = "kitty-specs/*/.kittify/dossiers/";

/// Credential and local-only agent files, ignored for every project.
pub const AGENT_SECRET_ENTRIES: [&str; 8] = [
    ".codex/auth.json",
    ".codex/history.jsonl",
    ".codex/sessions/",
    ".codex/log/",
    ".claude/settings.local.json",
    ".gemini/.env",
    ".qwen/.env",
    ".llxprt/.env",
];

/// Agent runtime-state folders, ignored whole.
pub const AGENT_STATE_DIR_ENTRIES: [&str; 4] = [".vibe/", ".pi/", ".letta/", ".github/copilot/"];

/// Skill-folder names reserved for Spec Kitty inside a skill root.
pub const RESERVED_SKILL_DIR_PATTERNS: [&str; 4] =
    ["spec-kitty/", "spec-kitty-*/", "spec-kitty.*/", "spk-*/"];

/// The shared skills root.
pub const SHARED_SKILL_ROOT: &str = ".agents/skills/";

/// The generated command-file pattern inside a command directory.
pub const COMMAND_FILE_PATTERN: &str = "spec-kitty.*";

/// A protection write report.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProtectionResult {
    /// Whether the file changed.
    pub modified: bool,
    /// Entries added by this run.
    pub entries_added: Vec<String>,
    /// Entries already present.
    pub entries_skipped: Vec<String>,
    /// Non-fatal warnings (a manifest that could not be read, etc.).
    pub warnings: Vec<String>,
}

/// One rendered security-notice entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentFolderNotice {
    /// The agent display name.
    pub display: String,
    /// The project folder the agent may store private state in.
    pub folder: String,
}

/// Render the agent-folder security entries for `agents`.
pub fn agent_folder_notice(agents: &[&str]) -> Vec<AgentFolderNotice> {
    let mut notices = Vec::new();
    for key in agents {
        if let Ok(surface) = agent_surface(key)
            && let Some(folder) = surface.folder
        {
            notices.push(AgentFolderNotice {
                display: surface.display.to_string(),
                folder: folder.to_string(),
            });
        }
    }
    notices
}

/// Ensure Spec Kitty's runtime entries and agent block in `project_root/.gitignore`.
///
/// `agent_keys` is the selection (init passes it explicitly because the config
/// is saved later). `builtin_skill_names` are the shipped skill names whose
/// non-reserved names need their own ignore entry.
pub fn protect_all_agents(
    project_root: &Path,
    agent_keys: &[String],
    builtin_skill_names: &[String],
) -> Result<ProtectionResult, InstallError> {
    protect_all_agents_with(project_root, agent_keys, builtin_skill_names, true)
}

/// Variant used on a refresh that must not create a `.gitignore` the project
/// never had (`create=false`).
pub fn protect_all_agents_with(
    project_root: &Path,
    agent_keys: &[String],
    builtin_skill_names: &[String],
    create: bool,
) -> Result<ProtectionResult, InstallError> {
    let path = project_root.join(".gitignore");
    let existing = read_ignore_file(&path)?;
    let mut result = ProtectionResult::default();

    let mut required = Vec::new();
    required.extend(RUNTIME_ENTRIES.iter().map(|entry| (*entry).to_string()));
    required.push(FEATURE_RUNTIME_ENTRY.to_string());

    let entries = agent_ignore_entries(
        project_root,
        agent_keys,
        builtin_skill_names,
        &mut result.warnings,
    );
    let block = replace_agent_block(existing.as_deref(), &entries, create)?;

    let mut lines: Vec<String> = match &block {
        Some(text) => text.lines().map(str::to_string).collect(),
        None => existing
            .as_deref()
            .map(|text| text.lines().map(str::to_string).collect())
            .unwrap_or_default(),
    };
    let mut added = Vec::new();
    for entry in &required {
        if lines.iter().any(|line| line.trim() == entry) {
            result.entries_skipped.push(entry.clone());
        } else {
            if !lines.iter().any(|line| line.trim() == GITIGNORE_MARKER) {
                lines.push(GITIGNORE_MARKER.to_string());
            }
            lines.push(entry.clone());
            added.push(entry.clone());
            result.entries_added.push(entry.clone());
        }
    }
    // Keep the agent block last: move it after runtime entries if it was present.
    if !added.is_empty() {
        let begin = lines
            .iter()
            .position(|line| line.trim() == AGENT_BLOCK_BEGIN);
        let end = lines.iter().position(|line| line.trim() == AGENT_BLOCK_END);
        if let (Some(begin), Some(end)) = (begin, end)
            && begin < end
        {
            let block_lines: Vec<String> = lines.drain(begin..=end).collect();
            while lines.last().is_some_and(|line| line.trim().is_empty()) {
                lines.pop();
            }
            lines.push(String::new());
            lines.extend(block_lines);
        }
    }
    let text = if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    };
    let final_text = match (&existing, &block) {
        (None, None) if !create => String::new(),
        _ => text,
    };
    if let Some(before) = &existing
        && before == &final_text
    {
        result.entries_added.clear();
        return Ok(result);
    }
    if final_text.is_empty() {
        return Ok(result);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|source| InstallError::io("create directory", parent, source))?;
    }
    sk_kernel::atomic_write(&path, final_text.as_bytes()).map_err(|error| {
        InstallError::io("write .gitignore", &path, std::io::Error::other(error))
    })?;
    result.modified = true;
    Ok(result)
}

/// Replace (or append) the agent-files block in `content`.
///
/// Returns `None` when the file would not change, including "no block and
/// `create=false`".
pub fn replace_agent_block(
    content: Option<&str>,
    entries: &[String],
    create: bool,
) -> Result<Option<String>, InstallError> {
    let text = content.unwrap_or("");
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let ends_with_newline = text.ends_with('\n');
    let mut lines: Vec<String> = if text.is_empty() {
        Vec::new()
    } else {
        text.split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
            .collect()
    };
    if ends_with_newline {
        lines.pop();
    }
    let begins: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.trim() == AGENT_BLOCK_BEGIN)
        .map(|(index, _)| index)
        .collect();
    let ends: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.trim() == AGENT_BLOCK_END)
        .map(|(index, _)| index)
        .collect();
    let block: Vec<String> = std::iter::once(AGENT_BLOCK_BEGIN.to_string())
        .chain(entries.iter().cloned())
        .chain(std::iter::once(AGENT_BLOCK_END.to_string()))
        .collect();

    let mut updated_lines: Vec<String> = match (begins.len(), ends.len()) {
        (0, 0) => {
            if !create {
                return Ok(None);
            }
            let mut updated = lines.clone();
            if updated.last().is_some_and(|line| !line.trim().is_empty()) {
                updated.push(String::new());
            }
            updated.extend(block);
            updated
        }
        (1, 1) if begins[0] < ends[0] => {
            let mut updated = lines[..begins[0]].to_vec();
            updated.extend(block);
            updated.extend(lines[ends[0] + 1..].to_vec());
            updated
        }
        _ => {
            return Err(InstallError::IgnoreBlockDamaged {
                detail: format!(
                    ".gitignore has a damaged Spec Kitty agent-files block ({} begin and {} end line(s)); \
                     keep exactly one '{}' ... '{}' pair, or delete both lines, then re-run",
                    begins.len(),
                    ends.len(),
                    AGENT_BLOCK_BEGIN,
                    AGENT_BLOCK_END
                ),
            });
        }
    };
    // A block replacement must not accumulate trailing blank lines.
    while updated_lines.last().is_some_and(|line| line.is_empty()) {
        updated_lines.pop();
    }
    let updated = if updated_lines.is_empty() {
        String::new()
    } else {
        format!("{}{newline}", updated_lines.join(newline))
    };
    if updated == text {
        return Ok(None);
    }
    Ok(Some(updated))
}

/// The `.gitignore` patterns for the configured agents.
pub fn agent_ignore_entries(
    project_root: &Path,
    agent_keys: &[String],
    builtin_skill_names: &[String],
    warnings: &mut Vec<String>,
) -> Vec<String> {
    let known: Vec<&'static AgentSurface> = agent_keys
        .iter()
        .filter_map(|key| agent_surface(key).ok())
        .collect();
    let mut generated: BTreeSet<String> = BTreeSet::new();
    for pattern in RESERVED_SKILL_DIR_PATTERNS {
        generated.insert(format!("{SHARED_SKILL_ROOT}{pattern}"));
    }
    for surface in &known {
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
            for name in builtin_skill_names {
                if !is_reserved_skill_name(name) {
                    generated.insert(format!("{root}{name}/"));
                }
            }
        }
    }
    // Pack-skill folders the skills manifest records for these agents.
    if let Some(manifest) = load_skills_manifest_paths(project_root, &mut *warnings) {
        for (agent_key, installed_path, origin) in manifest {
            if !agent_keys.iter().any(|key| key == &agent_key) || origin != "pack" {
                continue;
            }
            if let Ok(surface) = agent_surface(&agent_key)
                && let Some(root) = surface.skill.primary_root()
            {
                let root = root.trim_end_matches('/');
                if let Some(rest) = installed_path.strip_prefix(&format!("{root}/"))
                    && let Some((skill, _)) = rest.split_once('/')
                {
                    generated.insert(format!("{root}/{skill}/"));
                }
            }
        }
    }
    // Projected agent-profile files.
    generated.extend(profile_paths(project_root, warnings));

    let mut entries: Vec<String> = AGENT_SECRET_ENTRIES
        .iter()
        .chain(AGENT_STATE_DIR_ENTRIES.iter())
        .map(|entry| (*entry).to_string())
        .collect();
    let fixed: BTreeSet<String> = entries.iter().cloned().collect();
    entries.extend(generated.into_iter().filter(|entry| !fixed.contains(entry)));
    entries
}

fn is_reserved_skill_name(name: &str) -> bool {
    name == "spec-kitty"
        || name.starts_with("spec-kitty-")
        || name.starts_with("spec-kitty.")
        || name.starts_with("spk-")
}

fn load_skills_manifest_paths(
    project_root: &Path,
    warnings: &mut Vec<String>,
) -> Option<Vec<(String, String, String)>> {
    let path = project_root.join(".kittify/skills-manifest.json");
    let raw = match std::fs::read(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            warnings.push(format!("Could not read {}: {error}", path.display()));
            return None;
        }
    };
    let value: Value = match serde_json::from_slice(&raw) {
        Ok(value) => value,
        Err(error) => {
            warnings.push(format!("Could not parse {}: {error}", path.display()));
            return None;
        }
    };
    let entries = value.get("entries").and_then(Value::as_array)?;
    let mut paths = Vec::new();
    for entry in entries {
        let agent = entry.get("agent_key").and_then(Value::as_str)?;
        let installed = entry.get("installed_path").and_then(Value::as_str)?;
        let origin = entry
            .get("origin")
            .and_then(Value::as_str)
            .unwrap_or("builtin");
        paths.push((agent.to_string(), installed.to_string(), origin.to_string()));
    }
    Some(paths)
}

fn profile_paths(project_root: &Path, warnings: &mut Vec<String>) -> BTreeSet<String> {
    let path = project_root.join(".kittify/agent_profiles_manifest.json");
    let mut paths = BTreeSet::new();
    let raw = match std::fs::read(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return paths,
        Err(error) => {
            warnings.push(format!("Could not read {}: {error}", path.display()));
            return paths;
        }
    };
    let value: Value = match serde_json::from_slice(&raw) {
        Ok(value) => value,
        Err(error) => {
            warnings.push(format!("Could not parse {}: {error}", path.display()));
            return paths;
        }
    };
    if let Some(entries) = value.get("entries").and_then(Value::as_array) {
        for entry in entries {
            if let Some(output) = entry.get("output_path").and_then(Value::as_str) {
                paths.insert(output.to_string());
            }
        }
    }
    paths
}

fn read_ignore_file(path: &Path) -> Result<Option<String>, InstallError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(InstallError::IgnoreFileUnsafe {
            path: path.to_path_buf(),
            detail: "ignore file is a symbolic link; refusing to follow it".to_string(),
        }),
        Ok(metadata) if !metadata.file_type().is_file() => Err(InstallError::IgnoreFileUnsafe {
            path: path.to_path_buf(),
            detail: "ignore file is not a regular file".to_string(),
        }),
        Ok(_) => {
            let raw = sk_kernel::read_bounded(path, 4 * 1024 * 1024)
                .map_err(|error| InstallError::io("read", path, std::io::Error::other(error)))?;
            let text = String::from_utf8(raw).map_err(|error| InstallError::IgnoreFileUnsafe {
                path: path.to_path_buf(),
                detail: error.to_string(),
            })?;
            Ok(Some(text))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(InstallError::io("inspect", path, source)),
    }
}

/// Every configured agent key, for callers that need a full default selection.
pub fn all_agent_keys() -> Vec<String> {
    agent_keys().map(str::to_string).collect()
}

/// A convenience path helper for callers that plan around the file.
pub fn gitignore_path(project_root: &Path) -> PathBuf {
    project_root.join(".gitignore")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries() -> Vec<String> {
        vec!["a".to_string(), "b".to_string()]
    }

    #[test]
    fn inserts_a_block_and_is_idempotent() {
        let content = "node_modules/\n";
        let once = replace_agent_block(Some(content), &entries(), true)
            .expect("edit")
            .expect("changed");
        assert!(once.starts_with("node_modules/\n\n"));
        assert!(once.contains(AGENT_BLOCK_BEGIN));
        assert!(once.ends_with(&format!("{AGENT_BLOCK_END}\n")));
        assert_eq!(
            replace_agent_block(Some(&once), &entries(), true).expect("no-op"),
            None
        );
    }

    #[test]
    fn replaces_in_place_and_preserves_crlf() {
        let content =
            format!("keep\r\n{AGENT_BLOCK_BEGIN}\r\nold\r\n{AGENT_BLOCK_END}\r\ntail\r\n");
        let updated = replace_agent_block(Some(&content), &entries(), true)
            .expect("edit")
            .expect("changed");
        assert!(updated.contains(&format!(
            "{AGENT_BLOCK_BEGIN}\r\na\r\nb\r\n{AGENT_BLOCK_END}"
        )));
        assert!(updated.ends_with("tail\r\n"));
        assert!(!updated.contains("old"));
    }

    #[test]
    fn refuses_a_damaged_block() {
        let damaged = format!("{AGENT_BLOCK_BEGIN}\na\n");
        let error = replace_agent_block(Some(&damaged), &entries(), true).expect_err("refused");
        assert_eq!(error.code(), "gitignore_block_damaged");
    }

    #[test]
    fn create_false_leaves_a_blockless_file_alone() {
        assert_eq!(
            replace_agent_block(Some("x\n"), &entries(), false).expect("no-op"),
            None
        );
    }

    #[test]
    fn protection_matches_the_oracle_runtime_and_agent_surface() {
        let directory = tempfile::tempdir().expect("temp");
        std::fs::write(directory.path().join(".gitignore"), "node_modules/\n").unwrap();
        std::fs::create_dir_all(directory.path().join(".kittify")).unwrap();
        std::fs::write(
            directory
                .path()
                .join(".kittify/agent_profiles_manifest.json"),
            r#"{"entries":[{"output_path":".claude/agents/architect-alphonso.md"}]}"#,
        )
        .unwrap();
        let result = protect_all_agents(
            directory.path(),
            &["claude".to_string()],
            &["adversarial-squad".to_string()],
        )
        .expect("protect");
        assert!(result.modified);
        let text = std::fs::read_to_string(directory.path().join(".gitignore")).unwrap();
        for entry in RUNTIME_ENTRIES {
            assert!(text.lines().any(|line| line.trim() == entry), "{entry}");
        }
        assert!(text.contains(FEATURE_RUNTIME_ENTRY));
        assert!(text.contains(".claude/commands/spec-kitty.*"));
        assert!(text.contains(".claude/agents/architect-alphonso.md"));
        assert!(text.contains(".claude/skills/adversarial-squad/"));
        assert!(text.contains(".claude/skills/spec-kitty-*/"));
        assert!(!text.contains(".claude/\n"));
        // A second run adds nothing.
        let second = protect_all_agents(
            directory.path(),
            &["claude".to_string()],
            &["adversarial-squad".to_string()],
        )
        .expect("protect");
        assert!(!second.modified);
        assert!(second.entries_added.is_empty());
    }

    #[test]
    fn refuses_a_symlinked_gitignore() {
        let directory = tempfile::tempdir().expect("temp");
        let target = directory.path().join("elsewhere");
        std::fs::write(&target, "x\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, directory.path().join(".gitignore")).unwrap();
        let error = protect_all_agents(directory.path(), &["claude".to_string()], &[])
            .expect_err("refused");
        assert_eq!(error.code(), "ignore_file_unsafe");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "x\n");
    }

    #[test]
    fn security_notice_uses_agent_folders() {
        let notices = agent_folder_notice(&["claude", "codex", "vibe"]);
        assert!(notices.iter().any(|notice| notice.folder == ".claude/"));
        assert!(
            notices
                .iter()
                .any(|notice| notice.folder == ".agents/skills/")
        );
        assert!(notices.iter().any(|notice| notice.folder == ".vibe/"));
    }
}
