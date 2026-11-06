//! Worktree/layout migrations.
//!
//! | id | effect |
//! | --- | --- |
//! | `0.6.5_commands_rename` | rename `commands/` to `command-templates/` in templates and missions |
//! | `0.7.2_worktree_commands_dedup` | drop a worktree's own `.claude/commands/` (it inherits the main checkout's) |
//! | `0.8.0_worktree_agents_symlink` | link each worktree's `.kittify/AGENTS.md` to the main checkout's |
//! | `0.10.8_fix_memory_structure` | move a root-level `memory/` and `AGENTS.md` into `.kittify/`, repair broken links |
//!
//! Every removal names its owned root (`kernel.tree_removal`'s contract), so a
//! path that resolves outside the tree, or a directory holding a git checkout,
//! is refused instead of deleted. Symlinks are handled as links: `exists()` is
//! false for a dangling link, so the checks test `is_symlink()` first, exactly
//! as the pinned modules do.

use std::path::{Path, PathBuf};

use crate::error::InstallError;
use crate::migrations::completion_legacy::paths::{self, KITTIFY, WORKTREES};
use crate::upgrade::MigrationOutcome;

fn outcome(changed: bool, changes: Vec<String>, warnings: Vec<String>) -> MigrationOutcome {
    MigrationOutcome {
        changed,
        changes,
        warnings,
        ..crate::upgrade::MigrationOutcome::default()
    }
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

// --------------------------------------------------- 0.6.5 commands/ rename

fn has_old_commands(location: &Path) -> bool {
    let kittify = location.join(KITTIFY);
    if kittify.join("templates/commands").exists() {
        return true;
    }
    paths::directories(&kittify.join("missions"))
        .into_iter()
        .any(|mission| mission.join("commands").exists())
}

/// `0.6.5_commands_rename.detect`.
pub fn detect_commands_rename(project_root: &Path) -> Result<bool, InstallError> {
    if has_old_commands(project_root) {
        return Ok(true);
    }
    Ok(paths::worktrees(project_root)
        .into_iter()
        .any(|worktree| has_old_commands(&worktree)))
}

/// `0.6.5_commands_rename.can_apply`: a directory holding both the old and the
/// new name is the source's manual-merge refusal.
pub fn can_apply_commands_rename(project_root: &Path) -> Result<Option<String>, InstallError> {
    let kittify = project_root.join(KITTIFY);
    let templates = kittify.join("templates");
    if templates.exists()
        && templates.join("commands").exists()
        && templates.join("command-templates").exists()
    {
        return Ok(Some(
            "Both commands/ and command-templates/ exist in templates - manual merge required"
                .to_string(),
        ));
    }
    for mission in paths::directories(&kittify.join("missions")) {
        if mission.join("commands").exists() && mission.join("command-templates").exists() {
            return Ok(Some(format!(
                "Both directories exist in mission {} - manual merge required",
                name_of(&mission)
            )));
        }
    }
    Ok(None)
}

fn rename_dir(old: &Path, new: &Path, context: &str, changes: &mut Vec<String>) {
    if old.exists() && !new.exists() {
        match std::fs::rename(old, new) {
            Ok(()) => changes.push(format!(
                "Renamed {context}: commands/ -> command-templates/"
            )),
            Err(error) => changes.push(format!("Failed to rename {context}: {error}")),
        }
    }
}

/// `0.6.5_commands_rename.apply`.
pub fn apply_commands_rename(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let kittify = project_root.join(KITTIFY);
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    let templates = kittify.join("templates");
    if templates.exists() {
        rename_dir(
            &templates.join("commands"),
            &templates.join("command-templates"),
            ".kittify/templates",
            &mut changes,
        );
    }
    for mission in paths::directories(&kittify.join("missions")) {
        rename_dir(
            &mission.join("commands"),
            &mission.join("command-templates"),
            &format!(".kittify/missions/{}", name_of(&mission)),
            &mut changes,
        );
    }
    for worktree in paths::worktrees(project_root) {
        let worktree_name = name_of(&worktree);
        let wt_kittify = worktree.join(KITTIFY);
        let wt_templates_commands = wt_kittify.join("templates/commands");
        if wt_templates_commands.exists() {
            match paths::remove_tool_owned_tree(
                &wt_templates_commands,
                &wt_kittify,
                "worktree legacy templates/commands",
                false,
            ) {
                Ok(_) => changes.push(format!(
                    "Removed old commands/ from worktree {worktree_name}"
                )),
                Err(error) => warnings.push(format!(
                    "Could not remove old commands/ from worktree {worktree_name}: {error}"
                )),
            }
        }
        for mission in paths::directories(&wt_kittify.join("missions")) {
            rename_dir(
                &mission.join("commands"),
                &mission.join("command-templates"),
                &format!(
                    ".worktrees/{worktree_name}/.kittify/missions/{}",
                    name_of(&mission)
                ),
                &mut changes,
            );
        }
    }
    Ok(outcome(!changes.is_empty(), changes, warnings))
}

// ------------------------------------------------- 0.7.2 worktree command dedup

fn worktree_claude_commands(project_root: &Path) -> Vec<PathBuf> {
    paths::worktrees(project_root)
        .into_iter()
        .map(|worktree| worktree.join(".claude").join("commands"))
        .filter(|path| path.exists() || path.is_symlink())
        .collect()
}

/// `0.7.2_worktree_commands_dedup.detect`.
pub fn detect_worktree_commands_dedup(project_root: &Path) -> Result<bool, InstallError> {
    Ok(!worktree_claude_commands(project_root).is_empty())
}

/// `0.7.2_worktree_commands_dedup.can_apply`: the main checkout must hold the
/// commands the worktrees inherit.
pub fn can_apply_worktree_commands_dedup(
    project_root: &Path,
) -> Result<Option<String>, InstallError> {
    if project_root.join(".claude").join("commands").exists() {
        return Ok(None);
    }
    Ok(Some(
        "Main repo .claude/commands/ must exist before removing from worktrees".to_string(),
    ))
}

/// `0.7.2_worktree_commands_dedup.apply`.
pub fn apply_worktree_commands_dedup(
    project_root: &Path,
) -> Result<MigrationOutcome, InstallError> {
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    for worktree in paths::worktrees(project_root) {
        let commands = worktree.join(".claude").join("commands");
        if !(commands.exists() || commands.is_symlink()) {
            continue;
        }
        let worktree_name = name_of(&worktree);
        match paths::remove_tool_owned_tree(
            &commands,
            &worktree,
            "worktree .claude/commands dedup",
            false,
        ) {
            Ok(_) => changes.push(format!(
                "Removed .claude/commands/ from worktree {worktree_name} (inherits from main repo)"
            )),
            Err(error) => warnings.push(format!(
                "Failed to remove .claude/commands/ from {worktree_name}: {error}"
            )),
        }
        paths::remove_if_empty(&worktree.join(".claude"));
    }
    Ok(outcome(!changes.is_empty(), changes, warnings))
}

// ------------------------------------------------ 0.8.0 worktree AGENTS symlink

fn worktree_agents_targets(project_root: &Path) -> Vec<PathBuf> {
    paths::worktrees(project_root)
        .into_iter()
        .filter(|worktree| !name_of(worktree).starts_with('.'))
        .map(|worktree| worktree.join(KITTIFY).join("AGENTS.md"))
        .collect()
}

/// `0.8.0_worktree_agents_symlink.detect`: only a worktree whose link is missing
/// (or dangling) selects the repair, and only when the main checkout has the
/// file to link to.
pub fn detect_worktree_agents_symlink(project_root: &Path) -> Result<bool, InstallError> {
    let main_agents = project_root.join(KITTIFY).join("AGENTS.md");
    if !main_agents.exists() {
        return Ok(false);
    }
    if !project_root.join(WORKTREES).exists() {
        return Ok(false);
    }
    Ok(worktree_agents_targets(project_root)
        .into_iter()
        .any(|target| !target.exists()))
}

/// `0.8.0_worktree_agents_symlink.can_apply`.
pub fn can_apply_worktree_agents_symlink(
    project_root: &Path,
) -> Result<Option<String>, InstallError> {
    if project_root.join(KITTIFY).join("AGENTS.md").exists() {
        return Ok(None);
    }
    Ok(Some(
        "Main repo .kittify/AGENTS.md must exist before creating symlinks".to_string(),
    ))
}

/// `0.8.0_worktree_agents_symlink.apply`.
///
/// The link is created relative (`../../../.kittify/AGENTS.md`), so a checkout
/// stays relocatable; when the platform refuses a symlink the source copies the
/// file instead, and so does this.
pub fn apply_worktree_agents_symlink(
    project_root: &Path,
) -> Result<MigrationOutcome, InstallError> {
    let main_agents = project_root.join(KITTIFY).join("AGENTS.md");
    if !main_agents.exists() {
        return Ok(MigrationOutcome {
            changed: false,
            changes: Vec::new(),
            warnings: vec!["Main repo .kittify/AGENTS.md not found, skipping".to_string()],
            ..crate::upgrade::MigrationOutcome::default()
        });
    }
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    for worktree in paths::worktrees(project_root) {
        if name_of(&worktree).starts_with('.') {
            continue;
        }
        let wt_kittify = worktree.join(KITTIFY);
        let wt_agents = wt_kittify.join("AGENTS.md");
        if wt_agents.exists() && !wt_agents.is_symlink() {
            warnings.push(format!(
                "Worktree {} has non-symlink AGENTS.md, skipping",
                name_of(&worktree)
            ));
            continue;
        }
        if wt_agents.is_symlink() && wt_agents.exists() {
            continue;
        }
        let relative = Path::new("../../../.kittify/AGENTS.md");
        if wt_agents.is_symlink() {
            std::fs::remove_file(&wt_agents)
                .map_err(|error| InstallError::io("remove symlink", &wt_agents, error))?;
        }
        if let Err(error) = std::fs::create_dir_all(&wt_kittify) {
            warnings.push(format!(
                "Failed to create {}: {error}",
                paths::relative_display(project_root, &wt_kittify)
            ));
            continue;
        }
        // A relative link needs the link's own directory as the resolution base,
        // so the raw `symlink` call is made with the link path (never by changing
        // the process CWD, which is not reentrant).
        match paths::symlink(relative, &wt_agents) {
            Ok(()) => changes.push(format!(
                "Created .kittify/AGENTS.md symlink in worktree {}",
                name_of(&worktree)
            )),
            Err(_) => match std::fs::copy(&main_agents, &wt_agents) {
                Ok(_) => changes.push(format!(
                    "Copied .kittify/AGENTS.md to worktree {} (symlink failed)",
                    name_of(&worktree)
                )),
                Err(copy_error) => warnings.push(format!(
                    "Failed to create AGENTS.md in {}: copy failed: {copy_error}",
                    name_of(&worktree)
                )),
            },
        }
    }
    Ok(outcome(!changes.is_empty(), changes, warnings))
}

// ------------------------------------------------ 0.10.8 memory/AGENTS structure

fn is_broken_link(path: &Path) -> bool {
    path.is_symlink() && std::fs::canonicalize(path).is_err()
}

/// `0.10.8_fix_memory_structure.detect`.
pub fn detect_memory_structure(project_root: &Path) -> Result<bool, InstallError> {
    let root_memory = project_root.join("memory");
    let kittify_memory = project_root.join(KITTIFY).join("memory");
    if root_memory.is_dir() && (!kittify_memory.exists() || is_broken_link(&kittify_memory)) {
        return Ok(true);
    }
    if is_broken_link(&kittify_memory) {
        return Ok(true);
    }
    Ok(is_broken_link(
        &project_root.join(KITTIFY).join("AGENTS.md"),
    ))
}

/// `0.10.8_fix_memory_structure.can_apply`: `.kittify/` must exist first.
pub fn can_apply_memory_structure(project_root: &Path) -> Result<Option<String>, InstallError> {
    if project_root.join(KITTIFY).exists() {
        return Ok(None);
    }
    Ok(Some(
        ".kittify directory must exist before migration".to_string(),
    ))
}

/// `0.10.8_fix_memory_structure.apply`.
pub fn apply_memory_structure(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let kittify = project_root.join(KITTIFY);
    let kittify_memory = kittify.join("memory");
    let kittify_agents = kittify.join("AGENTS.md");
    let templates_agents = kittify.join("templates").join("AGENTS.md");
    let mut changes = Vec::new();
    let mut warnings = Vec::new();

    if kittify_memory.is_symlink() {
        std::fs::remove_file(&kittify_memory)
            .map_err(|error| InstallError::io("remove symlink", &kittify_memory, error))?;
        changes.push(format!(
            "Removed broken symlink: {}",
            paths::relative_display(project_root, &kittify_memory)
        ));
    }

    let root_memory = project_root.join("memory");
    if root_memory.is_dir() {
        if !kittify_memory.exists() {
            match std::fs::rename(&root_memory, &kittify_memory) {
                Ok(()) => changes.push(format!(
                    "Moved {} -> {}",
                    paths::relative_display(project_root, &root_memory),
                    paths::relative_display(project_root, &kittify_memory)
                )),
                Err(error) => match crate::migrations::completion_legacy::assets::copy_tree(
                    &root_memory,
                    &kittify_memory,
                ) {
                    Ok(_) => {
                        changes.push(format!(
                            "Copied {} -> {} (move failed: {error})",
                            paths::relative_display(project_root, &root_memory),
                            paths::relative_display(project_root, &kittify_memory)
                        ));
                        warnings.push(format!("Could not move (copied instead): {error}"));
                    }
                    Err(copy_error) => {
                        return Err(InstallError::UpgradeRefused {
                            detail: format!("Failed to move or copy memory/: {copy_error}"),
                        });
                    }
                },
            }
        } else {
            warnings.push(format!(
                "{} already exists, skipping root memory/ migration",
                paths::relative_display(project_root, &kittify_memory)
            ));
        }
    }

    if !kittify_memory.exists() {
        let mut template_charter = None;
        let mission = kittify.join("missions").join("software-dev");
        if mission.exists() {
            let candidate = mission.join("command-templates").join("charter.md");
            let candidate = if candidate.exists() {
                candidate
            } else {
                kittify.join("templates/command-templates/charter.md")
            };
            if candidate.exists() {
                template_charter = Some(candidate);
            }
        }
        match template_charter {
            Some(template) => {
                std::fs::create_dir_all(&kittify_memory).map_err(|error| {
                    InstallError::io("create directory", &kittify_memory, error)
                })?;
                std::fs::copy(&template, kittify_memory.join("charter.md"))
                    .map_err(|error| InstallError::io("copy charter template", &template, error))?;
                changes.push("Created .kittify/memory/ from template".to_string());
            }
            None => {
                warnings.push(".kittify/memory/ doesn't exist and no template found".to_string())
            }
        }
    }

    if kittify_agents.is_symlink() {
        std::fs::remove_file(&kittify_agents)
            .map_err(|error| InstallError::io("remove symlink", &kittify_agents, error))?;
        changes.push("Removed broken symlink: .kittify/AGENTS.md".to_string());
    }
    if !kittify_agents.exists() || kittify_agents.is_symlink() {
        if templates_agents.exists() {
            std::fs::copy(&templates_agents, &kittify_agents)
                .map_err(|error| InstallError::io("copy AGENTS.md", &templates_agents, error))?;
            changes.push("Copied .kittify/templates/AGENTS.md -> .kittify/AGENTS.md".to_string());
        } else {
            warnings
                .push("No AGENTS.md template found at .kittify/templates/AGENTS.md".to_string());
        }
    }

    for worktree in paths::worktrees(project_root) {
        let wt_kittify = worktree.join(KITTIFY);
        if !wt_kittify.exists() {
            continue;
        }
        let wt_memory = wt_kittify.join("memory");
        let wt_agents = wt_kittify.join("AGENTS.md");
        if is_broken_link(&wt_memory) {
            std::fs::remove_file(&wt_memory)
                .map_err(|error| InstallError::io("remove symlink", &wt_memory, error))?;
            changes.push(format!(
                "Removed broken worktree symlink: {}",
                paths::relative_display(project_root, &wt_memory)
            ));
        }
        if !(wt_memory.exists() || wt_memory.is_symlink()) && kittify_memory.exists() {
            let relative = Path::new("../../../.kittify/memory");
            match paths::symlink(relative, &wt_memory) {
                Ok(()) => changes.push(format!(
                    "Created worktree symlink: {} -> {}",
                    paths::relative_display(project_root, &wt_memory),
                    relative.display()
                )),
                Err(_) => {
                    crate::migrations::completion_legacy::assets::copy_tree(
                        &kittify_memory,
                        &wt_memory,
                    )?;
                    changes.push(format!(
                        "Copied to worktree (symlink failed): {}",
                        paths::relative_display(project_root, &wt_memory)
                    ));
                }
            }
        }
        if is_broken_link(&wt_agents) {
            std::fs::remove_file(&wt_agents)
                .map_err(|error| InstallError::io("remove symlink", &wt_agents, error))?;
        }
        if !(wt_agents.exists() || wt_agents.is_symlink()) && kittify_agents.exists() {
            let relative = Path::new("../../../.kittify/AGENTS.md");
            match paths::symlink(relative, &wt_agents) {
                Ok(()) => changes.push(format!(
                    "Created worktree symlink: {} -> {}",
                    paths::relative_display(project_root, &wt_agents),
                    relative.display()
                )),
                Err(_) => {
                    std::fs::copy(&kittify_agents, &wt_agents)
                        .map_err(|error| InstallError::io("copy AGENTS.md", &wt_agents, error))?;
                    changes.push(format!(
                        "Copied to worktree (symlink failed): {}",
                        paths::relative_display(project_root, &wt_agents)
                    ));
                }
            }
        }
    }

    Ok(outcome(!changes.is_empty(), changes, warnings))
}
