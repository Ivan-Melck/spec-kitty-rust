//! Command-template migrations (`0.7.3` … `0.14.0`).
//!
//! | id | effect |
//! | --- | --- |
//! | `0.7.3_update_scripts` | refresh `.kittify/scripts/bash/*` from the package |
//! | `0.10.0_python_only` | retire bash wrapper references in command templates; scripts are preserved in place |
//! | `0.10.1_populate_slash_commands` | copy mission command templates into empty agent command dirs |
//! | `0.10.2_update_slash_commands` | refresh generated command files from the mission templates, then sweep legacy `.kittify/commands/*.toml` |
//! | `0.10.6_workflow_simplification` | refresh `implement`/`review` commands from the mission templates |
//! | `0.10.14`/`0.11.1`/`0.11.2`/`0.13.0`/`0.13.5`/`0.14.0` | the pinned modules the 3.x registry keeps as permanently inert history |
//! | `0.11.3_workflow_agent_flag` | add `--agent <name>` to action commands in agent prompts |
//!
//! The inert ids are ported as the source declares them (`detect()` is
//! `False`, so the runner records the recorded no-op the pin itself records);
//! their `can_apply`/`apply` bodies are still the source's, never a fabricated
//! success.

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

fn project_missions(project_root: &Path) -> PathBuf {
    project_root.join(KITTIFY).join("missions")
}

/// The project's `software-dev` command-template directory.
pub fn project_software_dev_templates(project_root: &Path) -> PathBuf {
    project_missions(project_root)
        .join("software-dev")
        .join("command-templates")
}

/// The existing project mission `command-templates` directories, `software-dev`
/// first (`0.10.1`/`0.10.2`/`0.10.6` selection rule).
fn first_command_templates(project_root: &Path) -> Option<(PathBuf, String)> {
    let missions = project_missions(project_root);
    let preferred = missions.join("software-dev").join("command-templates");
    if preferred.exists() {
        return Some((preferred, "software-dev".to_string()));
    }
    for mission in paths::directories(&missions) {
        let templates = mission.join("command-templates");
        if templates.exists() {
            return Some((
                templates,
                mission
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            ));
        }
    }
    None
}

fn kittify_exists(project_root: &Path) -> bool {
    project_root.join(KITTIFY).exists()
}

// ------------------------------------------------------- 0.7.3 update scripts

fn script_path(project_root: &Path) -> PathBuf {
    project_root
        .join(KITTIFY)
        .join("scripts/bash/create-new-feature.sh")
}

/// `0.7.3_update_scripts.detect`: the old script lacks the `.worktrees/` scan.
pub fn detect_update_scripts(project_root: &Path) -> Result<bool, InstallError> {
    let script = script_path(project_root);
    if !script.exists() {
        return Ok(false);
    }
    let Some(content) = paths::read_text(&script) else {
        return Ok(false);
    };
    Ok(!content.contains("Also scan .worktrees/"))
}

/// `0.7.3_update_scripts.can_apply`.
pub fn can_apply_update_scripts(project_root: &Path) -> Result<Option<String>, InstallError> {
    if kittify_exists(project_root) {
        return Ok(None);
    }
    Ok(Some(
        "No .kittify directory (not a spec-kitty project)".to_string(),
    ))
}

/// `0.7.3_update_scripts.apply`: copy the packaged scripts when this build ships
/// them. The pinned source no longer does, so its own warning branch is the
/// observable behaviour here — never a claim that scripts were updated.
pub fn apply_update_scripts(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    const SCRIPTS: [&str; 2] = [
        "scripts/bash/create-new-feature.sh",
        "scripts/bash/common.sh",
    ];
    let Some(package) = assets::package_cli_root() else {
        return Ok(MigrationOutcome {
            changed: false,
            changes: Vec::new(),
            warnings: vec![
                "Bash scripts not found in package (removed in later version or never existed). \
                 If you need script updates, they may have been handled by migration 0.10.0 cleanup. \
                 This is not an error."
                    .to_string(),
            ],
            ..crate::upgrade::MigrationOutcome::default()
        });
    };
    if !SCRIPTS
        .iter()
        .any(|relative| package.join(relative).exists())
    {
        return Ok(MigrationOutcome {
            changed: false,
            changes: Vec::new(),
            warnings: vec![
                "Bash scripts not found in package (removed in later version or never existed). \
                 If you need script updates, they may have been handled by migration 0.10.0 cleanup. \
                 This is not an error."
                    .to_string(),
            ],
            ..crate::upgrade::MigrationOutcome::default()
        });
    }
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    for relative in SCRIPTS {
        let source = package.join(relative);
        let destination = project_root.join(KITTIFY).join(relative);
        if !source.exists() {
            warnings.push(format!("Template {relative} not found in package"));
            continue;
        }
        let bytes = std::fs::read(&source)
            .map_err(|error| InstallError::io("read script", &source, error))?;
        paths::write_bytes(&destination, &bytes)?;
        changes.push(format!("Updated .kittify/{relative}"));
    }
    Ok(outcome(!changes.is_empty(), changes, warnings))
}

// ---------------------------------------------------------- 0.10.0 python only

/// The bash → Python command replacements (`COMMAND_REPLACEMENTS`).
///
/// Every pinned pattern is a literal path/command with only `\.` escapes, so a
/// literal replacement is byte-for-byte what `re.subn` produces for these rows.
pub const COMMAND_REPLACEMENTS: [(&str, &str); 32] = [
    (
        ".kittify/scripts/bash/create-new-feature.sh",
        "spec-kitty agent mission create-feature",
    ),
    (
        "scripts/bash/create-new-feature.sh",
        "spec-kitty agent mission create-feature",
    ),
    (
        ".kittify/scripts/bash/check-prerequisites.sh",
        "spec-kitty agent mission check-prerequisites",
    ),
    (
        "scripts/bash/check-prerequisites.sh",
        "spec-kitty agent mission check-prerequisites",
    ),
    (
        ".kittify/scripts/bash/setup-plan.sh",
        "spec-kitty agent setup-plan",
    ),
    ("scripts/bash/setup-plan.sh", "spec-kitty agent setup-plan"),
    (
        ".kittify/scripts/bash/update-agent-context.sh",
        "spec-kitty agent update-context",
    ),
    (
        "scripts/bash/update-agent-context.sh",
        "spec-kitty agent update-context",
    ),
    (
        ".kittify/scripts/bash/accept-feature.sh",
        "spec-kitty agent mission accept",
    ),
    (
        "scripts/bash/accept-feature.sh",
        "spec-kitty agent mission accept",
    ),
    (
        ".kittify/scripts/bash/merge-feature.sh",
        "spec-kitty agent mission merge",
    ),
    (
        "scripts/bash/merge-feature.sh",
        "spec-kitty agent mission merge",
    ),
    (
        ".kittify/scripts/bash/tasks-move-to-lane.sh",
        "spec-kitty agent move-task",
    ),
    (
        "scripts/bash/tasks-move-to-lane.sh",
        "spec-kitty agent move-task",
    ),
    (
        ".kittify/scripts/bash/tasks-list-lanes.sh",
        "spec-kitty agent list-tasks",
    ),
    (
        "scripts/bash/tasks-list-lanes.sh",
        "spec-kitty agent list-tasks",
    ),
    (
        ".kittify/scripts/bash/mark-task-status.sh",
        "spec-kitty agent mark-status",
    ),
    (
        "scripts/bash/mark-task-status.sh",
        "spec-kitty agent mark-status",
    ),
    (
        ".kittify/scripts/bash/tasks-add-history-entry.sh",
        "spec-kitty agent add-history",
    ),
    (
        "scripts/bash/tasks-add-history-entry.sh",
        "spec-kitty agent add-history",
    ),
    (
        ".kittify/scripts/bash/tasks-rollback-move.sh",
        "spec-kitty agent rollback-move",
    ),
    (
        "scripts/bash/tasks-rollback-move.sh",
        "spec-kitty agent rollback-move",
    ),
    (
        ".kittify/scripts/bash/validate-task-workflow.sh",
        "spec-kitty agent validate-workflow",
    ),
    (
        "scripts/bash/validate-task-workflow.sh",
        "spec-kitty agent validate-workflow",
    ),
    (
        ".kittify/scripts/bash/move-task-to-doing.sh",
        "spec-kitty agent move-task",
    ),
    (
        "scripts/bash/move-task-to-doing.sh",
        "spec-kitty agent move-task",
    ),
    ("tasks_cli.py move", "spec-kitty agent move-task"),
    ("tasks_cli.py list", "spec-kitty agent list-tasks"),
    ("tasks_cli.py mark", "spec-kitty agent mark-status"),
    ("tasks_cli.py history", "spec-kitty agent add-history"),
    ("tasks_cli.py rollback", "spec-kitty agent rollback-move"),
    (
        "tasks_cli.py validate",
        "spec-kitty agent validate-workflow",
    ),
];

/// The files of one script sweep (`*.sh` or `*.ps1`), sorted.
fn script_files(directory: &Path, extension: &str) -> Vec<PathBuf> {
    paths::entries(directory)
        .into_iter()
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .map(|ext| ext == extension)
                    .unwrap_or(false)
        })
        .collect()
}

/// Route one script through the ownership guard.
///
/// Scripts carry no version marker and the package ships no canonical to
/// byte-match, so the canonical prover proves nothing and the guard preserves
/// every script in place with a diagnostic (charter: unprovable ⇒ preserve +
/// warn). Deleting a user's script on a name match is exactly what the pinned
/// migration stopped doing.
fn preserve_scripts(
    scripts: Vec<PathBuf>,
    project_root: &Path,
    changes: &mut Vec<String>,
    warnings: &mut Vec<String>,
) -> Result<usize, InstallError> {
    let mut preserved = 0usize;
    for script in scripts {
        let verdict = guard_destructive_removal(
            &script,
            project_root,
            &Prover::CanonicalContent,
            false,
            None,
            false,
        )?;
        match verdict {
            RemovalVerdict::Owned { .. } => changes.push(format!(
                "Removed {}",
                paths::relative_display(project_root, &script)
            )),
            RemovalVerdict::Absent => {}
            RemovalVerdict::Preserved { diagnostic, .. }
            | RemovalVerdict::Archived { diagnostic, .. } => {
                preserved += 1;
                warnings.push(diagnostic);
            }
        }
    }
    Ok(preserved)
}

fn update_template_text(content: &str) -> (String, usize) {
    let mut updated = content.to_string();
    let mut replacements = 0usize;
    for (pattern, replacement) in COMMAND_REPLACEMENTS {
        let hits = updated.matches(pattern).count();
        if hits > 0 {
            updated = updated.replace(pattern, replacement);
            replacements += hits;
        }
    }
    (updated, replacements)
}

/// `0.10.0_python_only.detect`: any `.sh` under `.kittify/scripts/bash`.
pub fn detect_python_only(project_root: &Path) -> Result<bool, InstallError> {
    let bash = project_root.join(KITTIFY).join("scripts").join("bash");
    if !bash.exists() {
        return Ok(false);
    }
    Ok(!script_files(&bash, "sh").is_empty())
}

/// `0.10.0_python_only.can_apply`.
pub fn can_apply_python_only(_project_root: &Path) -> Result<Option<String>, InstallError> {
    Ok(None)
}

/// `0.10.0_python_only.apply`.
pub fn apply_python_only(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let mut changes = Vec::new();
    let mut warnings = Vec::new();

    let kittify_bash = project_root.join(KITTIFY).join("scripts").join("bash");
    if !kittify_bash.exists() {
        warnings.push("No .kittify/scripts/bash/ directory found - already migrated?".to_string());
    } else {
        let preserved = preserve_scripts(
            script_files(&kittify_bash, "sh"),
            project_root,
            &mut changes,
            &mut warnings,
        )?;
        let kittify_ps = project_root
            .join(KITTIFY)
            .join("scripts")
            .join("powershell");
        let mut preserved = preserved;
        if kittify_ps.exists() {
            preserved += preserve_scripts(
                script_files(&kittify_ps, "ps1"),
                project_root,
                &mut changes,
                &mut warnings,
            )?;
        }
        if paths::remove_if_empty(&kittify_bash) {
            changes.push("Removed empty: .kittify/scripts/bash/".to_string());
        }
        if paths::remove_if_empty(&kittify_ps) {
            changes.push("Removed empty: .kittify/scripts/powershell/".to_string());
        }
        if preserved > 0 {
            warnings.push(format!(
                "Preserved {preserved} unprovable script(s); left in place (no ownership signal)"
            ));
        } else {
            warnings.push("No bash scripts found to remove - already migrated?".to_string());
        }
    }

    for worktree in paths::worktrees(project_root) {
        let wt_bash = worktree.join(KITTIFY).join("scripts").join("bash");
        if !wt_bash.exists() {
            continue;
        }
        let preserved = preserve_scripts(
            script_files(&wt_bash, "sh"),
            project_root,
            &mut changes,
            &mut warnings,
        )?;
        paths::remove_if_empty(&wt_bash);
        if preserved > 0 {
            let name = worktree
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            warnings.push(format!(
                "Preserved {preserved} unprovable script(s) in worktree: {name}"
            ));
        }
    }

    let tasks_dir = project_root.join(KITTIFY).join("scripts").join("tasks");
    if tasks_dir.exists() {
        let verdict = guard_destructive_removal(
            &tasks_dir,
            project_root,
            &Prover::CanonicalContent,
            true,
            None,
            false,
        )?;
        match verdict {
            RemovalVerdict::Owned { .. } => {
                changes.push("Removed .kittify/scripts/tasks/".to_string())
            }
            RemovalVerdict::Absent => {}
            RemovalVerdict::Preserved { diagnostic, .. }
            | RemovalVerdict::Archived { diagnostic, .. } => warnings.push(diagnostic),
        }
    }

    let templates_dir = project_root
        .join(KITTIFY)
        .join("templates")
        .join("command-templates");
    if !templates_dir.exists() {
        changes.push(
            "Templates directory not found at .kittify/templates/command-templates/. \
             This is expected for projects initialized with older package versions. \
             Run 'spec-kitty upgrade' after upgrading to v0.10.9+ to repair templates."
                .to_string(),
        );
    } else {
        let mut updated = 0usize;
        for template in markdown_children(&templates_dir) {
            let Some(content) = paths::read_text(&template) else {
                continue;
            };
            let (updated_content, replacements) = update_template_text(&content);
            if replacements == 0 {
                continue;
            }
            paths::write_text(&template, &updated_content)?;
            updated += 1;
            let name = template
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            changes.push(format!("Updated: {name} ({replacements} replacements)"));
        }
        if updated > 0 {
            changes.push(format!("Total templates updated: {updated}"));
        }
    }

    Ok(outcome(!changes.is_empty(), changes, warnings))
}

fn markdown_children(directory: &Path) -> Vec<PathBuf> {
    paths::entries(directory)
        .into_iter()
        .filter(|path| {
            path.is_file() && path.extension().is_some_and(|extension| extension == "md")
        })
        .collect()
}

// --------------------------------------------- 0.10.1 populate slash commands

/// `0.10.1_populate_slash_commands.detect`.
pub fn detect_populate_slash_commands(project_root: &Path) -> Result<bool, InstallError> {
    let claude = project_root.join(".claude").join("commands");
    if !claude.exists() {
        return Ok(true);
    }
    let slash: Vec<PathBuf> = paths::entries(&claude)
        .into_iter()
        .filter(|path| {
            path.file_name()
                .map(|name| {
                    let name = name.to_string_lossy();
                    name.starts_with("spec-kitty.") && name.ends_with(".md")
                })
                .unwrap_or(false)
        })
        .collect();
    Ok(slash.len() < 8)
}

/// `0.10.1_populate_slash_commands.can_apply`.
pub fn can_apply_populate_slash_commands(
    project_root: &Path,
) -> Result<Option<String>, InstallError> {
    let missions = project_missions(project_root);
    if !missions.exists() {
        return Ok(Some("No missions directory found".to_string()));
    }
    if first_command_templates(project_root).is_some() {
        return Ok(None);
    }
    Ok(Some("No mission command templates found".to_string()))
}

/// `0.10.1_populate_slash_commands.apply`.
pub fn apply_populate_slash_commands(
    project_root: &Path,
) -> Result<MigrationOutcome, InstallError> {
    let Some((templates, mission_name)) = first_command_templates(project_root) else {
        return Err(InstallError::UpgradeRefused {
            detail: "No mission command templates found".to_string(),
        });
    };
    let mut changes = Vec::new();
    let mut total_created = 0usize;
    for directory in agent_commands::command_directories(project_root) {
        let agent_dir = project_root.join(&directory.directory);
        // Only agents that exist in this project: `get_agent_dirs_for_project`
        // answers the configured set, and a configured-but-deleted agent root
        // must not be recreated.
        if !agent_dir
            .parent()
            .map(|parent| parent.exists())
            .unwrap_or(false)
        {
            continue;
        }
        if !agent_dir.exists() {
            std::fs::create_dir_all(&agent_dir)
                .map_err(|error| InstallError::io("create directory", &agent_dir, error))?;
            changes.push(format!(
                "Created: {}",
                paths::relative_display(project_root, &agent_dir)
            ));
        }
        let mut created = 0usize;
        for template in markdown_children(&templates) {
            let stem = template
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default();
            let destination = agent_dir.join(format!("spec-kitty.{stem}.md"));
            if destination.exists() {
                continue;
            }
            let bytes = std::fs::read(&template)
                .map_err(|error| InstallError::io("read template", &template, error))?;
            paths::write_bytes(&destination, &bytes)?;
            changes.push(format!(
                "Created: {}",
                paths::relative_display(project_root, &destination)
            ));
            created += 1;
        }
        if created > 0 {
            let agent_name = directory.agent_key.clone();
            changes.push(format!(
                "Created {created} slash commands for {agent_name} from {mission_name}"
            ));
            total_created += created;
        }
    }
    if total_created > 0 {
        changes.push(format!("Total: created {total_created} slash command(s)"));
    }
    Ok(outcome(!changes.is_empty(), changes, Vec::new()))
}

// ----------------------------------------------- 0.10.2 update slash commands

/// `0.10.2_update_slash_commands.detect`.
pub fn detect_update_slash_commands(project_root: &Path) -> Result<bool, InstallError> {
    for directory in agent_commands::command_directories(project_root) {
        let agent_dir = project_root.join(&directory.directory);
        if !agent_dir.exists() {
            continue;
        }
        for command in markdown_children(&agent_dir) {
            let Some(name) = command
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
            else {
                continue;
            };
            if !name.starts_with("spec-kitty.") {
                continue;
            }
            let Some(content) = paths::read_text(&command) else {
                continue;
            };
            if content.contains(".kittify/scripts/bash/") || content.contains("scripts/bash/") {
                return Ok(true);
            }
            if content.contains(".kittify/scripts/powershell/")
                || content.contains("scripts/powershell/")
            {
                return Ok(true);
            }
            if (content.contains("tasks/planned/") || content.contains("tasks/doing/"))
                && (!content.contains("WRONG") || content.matches("tasks/planned/").count() > 2)
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// `0.10.2_update_slash_commands.can_apply`.
pub fn can_apply_update_slash_commands(
    project_root: &Path,
) -> Result<Option<String>, InstallError> {
    can_apply_populate_slash_commands(project_root)
}

/// The outcome of the canonical-overwrite gate (`guard_destructive_overwrite`).
enum Overwrite {
    Proceed,
    Preserve(String),
}

/// A generated command file may only be overwritten when it is absent or
/// package-generated (a command version marker, or a managed-manifest entry).
/// A hand-authored file that reuses the canonical name is preserved.
fn overwrite_verdict(path: &Path, project_root: &Path) -> Result<Overwrite, InstallError> {
    if !path.exists() {
        return Ok(Overwrite::Proceed);
    }
    if Prover::CanonicalContent.prove(path, project_root).is_some() {
        return Ok(Overwrite::Proceed);
    }
    Ok(Overwrite::Preserve(format!(
        "Preserved {}; not package-generated — a user-authored file is never overwritten",
        paths::relative_display(project_root, path)
    )))
}

/// `0.10.2_update_slash_commands.apply`.
pub fn apply_update_slash_commands(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let Some((templates, mission_name)) = first_command_templates(project_root) else {
        return Err(InstallError::UpgradeRefused {
            detail: "No mission command templates found".to_string(),
        });
    };
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    let mut total_updated = 0usize;
    for directory in agent_commands::command_directories(project_root) {
        let agent_dir = project_root.join(&directory.directory);
        if !agent_dir.exists() {
            continue;
        }
        let mut updated = 0usize;
        for template in markdown_children(&templates) {
            let stem = template
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default();
            let destination = agent_dir.join(format!("spec-kitty.{stem}.md"));
            let bytes = std::fs::read(&template)
                .map_err(|error| InstallError::io("read template", &template, error))?;
            match overwrite_verdict(&destination, project_root)? {
                Overwrite::Proceed => {}
                Overwrite::Preserve(diagnostic) => {
                    warnings.push(diagnostic);
                    continue;
                }
            }
            paths::write_bytes(&destination, &bytes)?;
            updated += 1;
        }
        if updated > 0 {
            changes.push(format!(
                "Updated {updated} slash commands for {}",
                directory.agent_key
            ));
            total_updated += updated;
        }
    }
    if total_updated > 0 {
        changes.push(format!(
            "Total: Updated {total_updated} slash commands from {mission_name} mission"
        ));
    }

    let commands_dir = project_root.join(KITTIFY).join("commands");
    if commands_dir.exists() {
        let prover = Prover::Any(vec![Prover::Manifest, Prover::CanonicalContent]);
        let mut removed = 0usize;
        for toml in paths::entries(&commands_dir).into_iter().filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_some_and(|extension| extension == "toml")
        }) {
            let verdict =
                guard_destructive_removal(&toml, project_root, &prover, false, None, false)?;
            match verdict {
                RemovalVerdict::Owned { .. } => {
                    removed += 1;
                    changes.push(format!(
                        "Removed legacy {}",
                        toml.file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    ));
                }
                RemovalVerdict::Absent => {}
                RemovalVerdict::Preserved { diagnostic, .. }
                | RemovalVerdict::Archived { diagnostic, .. } => warnings.push(diagnostic),
            }
        }
        let _ = removed;
        if paths::remove_if_empty(&commands_dir) {
            changes.push("Removed empty .kittify/commands/ directory".to_string());
        }
    }

    Ok(outcome(!changes.is_empty(), changes, warnings))
}

// ------------------------------------------- 0.10.6 / 0.11.x workflow refresh

/// The `implement`/`review` refresh shared by `0.10.6`, `0.11.1` and `0.11.2`.
fn refresh_implement_review(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    let templates = project_software_dev_templates(project_root);
    match assets::package_mission("software-dev") {
        Some(package) => {
            let package_templates = package.join("command-templates");
            if package_templates.exists() {
                std::fs::create_dir_all(&templates)
                    .map_err(|error| InstallError::io("create directory", &templates, error))?;
                for name in ["implement.md", "review.md"] {
                    let source = package_templates.join(name);
                    if !source.exists() {
                        warnings.push(format!("Package template missing: {name}"));
                        continue;
                    }
                    let bytes = std::fs::read(&source)
                        .map_err(|error| InstallError::io("read template", &source, error))?;
                    paths::write_bytes(&templates.join(name), &bytes)?;
                    changes.push(format!("Updated mission template: software-dev/{name}"));
                }
            } else {
                warnings.push(
                    "Mission templates not found in package. Slash commands may already be updated or require manual repair."
                        .to_string(),
                );
            }
        }
        None => warnings.push(
            "Mission templates not found in package. Slash commands may already be updated or require manual repair."
                .to_string(),
        ),
    }

    let mut total_updated = 0usize;
    for directory in agent_commands::command_directories(project_root) {
        let agent_dir = project_root.join(&directory.directory);
        if !agent_dir.exists() {
            continue;
        }
        let mut updated = 0usize;
        for name in ["implement.md", "review.md"] {
            let source = templates.join(name);
            if !source.exists() {
                continue;
            }
            let destination = agent_dir.join(format!("spec-kitty.{name}"));
            let bytes = std::fs::read(&source)
                .map_err(|error| InstallError::io("read template", &source, error))?;
            paths::write_bytes(&destination, &bytes)?;
            updated += 1;
        }
        if updated > 0 {
            changes.push(format!(
                "Updated {updated} templates for {}",
                directory.agent_key
            ));
            total_updated += updated;
        }
    }
    if total_updated > 0 {
        changes.push(format!(
            "Total: Updated {total_updated} slash command templates"
        ));
    } else if changes.is_empty() {
        warnings.push(
            "No templates were updated (already updated or mission templates missing)".to_string(),
        );
    }
    Ok(outcome(!changes.is_empty(), changes, warnings))
}

/// `0.10.6_workflow_simplification.detect`.
pub fn detect_workflow_simplification(project_root: &Path) -> Result<bool, InstallError> {
    for directory in agent_commands::command_directories(project_root) {
        let agent_dir = project_root.join(&directory.directory);
        if !agent_dir.exists() {
            continue;
        }
        let implement = agent_dir.join("spec-kitty.implement.md");
        if let Some(content) = paths::read_text(&implement) {
            if content.contains("Work Package Selection")
                || content.contains("Setup (Do This First)")
            {
                return Ok(true);
            }
            if !content.contains("spec-kitty agent action implement")
                && !content.contains("spec-kitty agent workflow implement")
            {
                return Ok(true);
            }
        }
        let review = agent_dir.join("spec-kitty.review.md");
        if let Some(content) = paths::read_text(&review) {
            if content.contains("Location Pre-flight Check")
                || content.contains("Conduct the review:")
            {
                return Ok(true);
            }
            if !content.contains("spec-kitty agent action review")
                && !content.contains("spec-kitty agent workflow review")
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// `0.10.6_workflow_simplification.can_apply`.
pub fn can_apply_workflow_simplification(
    project_root: &Path,
) -> Result<Option<String>, InstallError> {
    if kittify_exists(project_root) {
        return Ok(None);
    }
    Ok(Some(
        "No .kittify directory (not a spec-kitty project)".to_string(),
    ))
}

/// `0.10.6_workflow_simplification.apply`.
pub fn apply_workflow_simplification(
    project_root: &Path,
) -> Result<MigrationOutcome, InstallError> {
    refresh_implement_review(project_root)
}

/// `0.11.1_improved_workflow_templates.detect`: permanently inert in the pin.
pub fn detect_never(_project_root: &Path) -> Result<bool, InstallError> {
    Ok(false)
}

/// The inert modules' `can_apply` refusal (`command templates removed in WP10`).
pub fn can_apply_inert(_project_root: &Path) -> Result<Option<String>, InstallError> {
    Ok(Some(
        "Command templates were removed in WP10 (canonical context architecture). \
         Shim generation replaces template-based commands."
            .to_string(),
    ))
}

/// `0.11.1_improved_workflow_templates.apply` / `0.11.2` — the same refresh the
/// 0.10.6 body performs; unreachable from the runner because `detect()` is False.
pub fn apply_improved_workflow_templates(
    project_root: &Path,
) -> Result<MigrationOutcome, InstallError> {
    refresh_implement_review(project_root)
}

// ------------------------------------------- 0.11.3 workflow `--agent` flag

/// `0.11.3_workflow_agent_flag` implementation-prefixes.
pub const IMPLEMENT_PREFIXES: [&str; 2] = [
    "spec-kitty agent workflow implement",
    "spec-kitty agent action implement",
];

/// `0.11.3_workflow_agent_flag` review-prefixes.
pub const REVIEW_PREFIXES: [&str; 2] = [
    "spec-kitty agent workflow review",
    "spec-kitty agent action review",
];

/// The agent display name for a command-directory key (`AGENT_NAME_MAP`).
pub fn agent_display_name(agent_key: &str) -> String {
    match agent_key {
        "copilot" => "copilot".to_string(),
        "auggie" => "auggie".to_string(),
        "q" => "q".to_string(),
        other => other.trim_start_matches('.').to_string(),
    }
}

fn patched_lines(content: &str, agent_name: &str) -> (String, bool) {
    let mut updated = false;
    let mut lines: Vec<String> = Vec::new();
    for line in content.split_inclusive('\n') {
        let trailing = line.ends_with('\n');
        let mut current = line.trim_end_matches('\n').to_string();
        if current.contains("__AGENT__") {
            current = current.replace("__AGENT__", agent_name);
            updated = true;
        }
        let has_implement = IMPLEMENT_PREFIXES
            .iter()
            .any(|prefix| current.contains(prefix));
        let has_review = REVIEW_PREFIXES
            .iter()
            .any(|prefix| current.contains(prefix));
        if (has_implement || has_review) && !current.contains("--agent") {
            current = format!("{current} --agent {agent_name}");
            updated = true;
        }
        if trailing {
            current.push('\n');
        }
        lines.push(current);
    }
    (lines.concat(), updated)
}

fn action_prompt_files(project_root: &Path, agent_dir: &Path) -> Vec<PathBuf> {
    let _ = project_root;
    ["spec-kitty.implement.md", "spec-kitty.review.md"]
        .iter()
        .map(|name| agent_dir.join(name))
        .filter(|path| path.exists())
        .collect()
}

/// `0.11.3_workflow_agent_flag.detect`.
pub fn detect_workflow_agent_flag(project_root: &Path) -> Result<bool, InstallError> {
    for directory in agent_commands::command_directories(project_root) {
        let agent_dir = project_root.join(&directory.directory);
        if !agent_dir.exists() {
            continue;
        }
        for path in action_prompt_files(project_root, &agent_dir) {
            let Some(content) = paths::read_text(&path) else {
                continue;
            };
            for line in content.lines() {
                if line.contains("__AGENT__") {
                    return Ok(true);
                }
                if IMPLEMENT_PREFIXES
                    .iter()
                    .any(|prefix| line.contains(prefix))
                    && !line.contains("--agent")
                {
                    return Ok(true);
                }
                if REVIEW_PREFIXES.iter().any(|prefix| line.contains(prefix))
                    && !line.contains("--agent")
                {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

/// `0.11.3_workflow_agent_flag.can_apply`.
pub fn can_apply_workflow_agent_flag(project_root: &Path) -> Result<Option<String>, InstallError> {
    if kittify_exists(project_root) {
        return Ok(None);
    }
    Ok(Some(
        "No .kittify directory (not a spec-kitty project)".to_string(),
    ))
}

/// `0.11.3_workflow_agent_flag.apply`.
pub fn apply_workflow_agent_flag(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    for directory in agent_commands::command_directories(project_root) {
        let agent_dir = project_root.join(&directory.directory);
        if !agent_dir.exists() {
            continue;
        }
        let agent_name = agent_display_name(&directory.agent_key);
        let mut updated_count = 0usize;
        for path in action_prompt_files(project_root, &agent_dir) {
            let Some(content) = paths::read_text(&path) else {
                continue;
            };
            let (patched, updated) = patched_lines(&content, &agent_name);
            if !updated {
                continue;
            }
            paths::write_text(&path, &patched)?;
            updated_count += 1;
        }
        if updated_count > 0 {
            changes.push(format!(
                "Updated {updated_count} action prompts for {agent_name}"
            ));
        }
    }
    if changes.is_empty() {
        warnings.push("No action prompts required updates".to_string());
    }
    Ok(outcome(!changes.is_empty(), changes, warnings))
}

// -------------------------------------------- 0.10.14 / 0.13.0 / 0.13.5 / 0.14.0

/// `0.10.14_update_implement_slash_command.can_apply`: the packaged template,
/// falling back to the project's legacy copy.
pub fn can_apply_update_implement_0_10_14(
    project_root: &Path,
) -> Result<Option<String>, InstallError> {
    if packaged_template(project_root, "software-dev", "implement.md").is_some() {
        return Ok(None);
    }
    Ok(Some(
        "Template not found in packaged missions or legacy location".to_string(),
    ))
}

fn packaged_template(project_root: &Path, mission: &str, name: &str) -> Option<PathBuf> {
    if let Some(package) = assets::package_mission(mission) {
        let candidate = package.join("command-templates").join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    let legacy = project_missions(project_root)
        .join(mission)
        .join("command-templates")
        .join(name);
    legacy.exists().then_some(legacy)
}

/// The single-template/single-command refresh used by `0.10.14` and `0.13.0`.
fn refresh_one_command(
    project_root: &Path,
    mission: &str,
    template_name: &str,
    command_file: &str,
    selector: Option<&str>,
) -> Result<MigrationOutcome, InstallError> {
    let Some(template) = packaged_template(project_root, mission, template_name) else {
        return Err(InstallError::UpgradeRefused {
            detail: format!(
                "Template not found in packaged missions or legacy location ({mission}/{template_name})"
            ),
        });
    };
    let template_content = std::fs::read(&template)
        .map_err(|error| InstallError::io("read template", &template, error))?;
    let template_content = String::from_utf8_lossy(&template_content).into_owned();
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    let mut updated_agents = 0usize;
    for directory in agent_commands::command_directories(project_root) {
        let agent_dir = project_root.join(&directory.directory);
        if !agent_dir.exists() {
            continue;
        }
        let destination = agent_dir.join(command_file);
        if let Some(selector) = selector {
            let Some(current) = paths::read_text(&destination) else {
                continue;
            };
            if !current.contains(selector) {
                continue;
            }
            if current == template_content {
                continue;
            }
        }
        paths::write_text(&destination, &template_content)?;
        changes.push(format!("Updated: {}/{}", directory.directory, command_file));
        updated_agents += 1;
    }
    if updated_agents == 0 {
        changes.push("No agent directories needed updates".to_string());
    }
    let _ = &mut warnings;
    Ok(outcome(!changes.is_empty(), changes, warnings))
}

/// `0.10.14_update_implement_slash_command.apply`.
pub fn apply_update_implement_0_10_14(
    project_root: &Path,
) -> Result<MigrationOutcome, InstallError> {
    refresh_one_command(
        project_root,
        "software-dev",
        "implement.md",
        "spec-kitty.implement.md",
        None,
    )
}

/// `0.11.1_update_implement_slash_command.can_apply`.
pub fn can_apply_update_implement_0_11_1(
    project_root: &Path,
) -> Result<Option<String>, InstallError> {
    let _ = project_root;
    Ok(Some(
        "Command templates were removed in WP10 (canonical context architecture). \
         Shim generation replaces template-based commands."
            .to_string(),
    ))
}

/// `0.11.1_update_implement_slash_command.apply`.
pub fn apply_update_implement_0_11_1(
    project_root: &Path,
) -> Result<MigrationOutcome, InstallError> {
    refresh_one_command(
        project_root,
        "software-dev",
        "implement.md",
        "spec-kitty.implement.md",
        None,
    )
}

/// `0.13.0_update_research_implement_templates.apply`.
pub fn apply_update_research_implement(
    project_root: &Path,
) -> Result<MigrationOutcome, InstallError> {
    refresh_one_command(
        project_root,
        "research",
        "implement.md",
        "spec-kitty.implement.md",
        Some("Sprint Planning Artifacts"),
    )
}

/// `0.13.5_add_commit_workflow_to_templates.apply`.
///
/// The mission to refresh comes from `.kittify/meta.json`'s `mission_name`
/// (`software-dev` when the file is absent), falling back to both known
/// missions when it cannot be read — the source's own warn-and-continue arm.
pub fn apply_add_commit_workflow(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let meta = project_root.join(KITTIFY).join("meta.json");
    let mut warnings = Vec::new();
    let missions: Vec<String> = if meta.exists() {
        match std::fs::read(&meta)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        {
            Some(value) => {
                let name = value
                    .get("mission_name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("software-dev")
                    .to_string();
                vec![name]
            }
            None => {
                warnings.push("Cannot parse meta.json".to_string());
                vec!["software-dev".to_string(), "documentation".to_string()]
            }
        }
    } else {
        warnings.push("No meta.json found - cannot determine mission type".to_string());
        vec!["software-dev".to_string(), "documentation".to_string()]
    };

    let mut changes = Vec::new();
    for mission in missions {
        let Some(template) = packaged_template(project_root, &mission, "implement.md") else {
            warnings.push(format!("Template not found for mission: {mission}"));
            continue;
        };
        let template_content = std::fs::read_to_string(&template)
            .map_err(|error| InstallError::io("read template", &template, error))?;
        let mut updated_agents = 0usize;
        for directory in agent_commands::command_directories(project_root) {
            let agent_dir = project_root.join(&directory.directory);
            if !agent_dir.exists() {
                continue;
            }
            let destination = agent_dir.join("spec-kitty.implement.md");
            let Some(current) = paths::read_text(&destination) else {
                continue;
            };
            if current == template_content || current.contains("Commit Workflow") {
                continue;
            }
            paths::write_text(&destination, &template_content)?;
            changes.push(format!(
                "Updated: {}/spec-kitty.implement.md",
                directory.directory
            ));
            updated_agents += 1;
        }
        if updated_agents > 0 {
            changes.push(format!(
                "Updated {updated_agents} agent implement templates ({mission})"
            ));
        }
    }
    Ok(outcome(!changes.is_empty(), changes, warnings))
}

/// `0.14.0_centralized_feature_detection.apply`.
pub fn apply_centralized_feature_detection(
    project_root: &Path,
) -> Result<MigrationOutcome, InstallError> {
    let Some(template) = packaged_template(project_root, "software-dev", "plan.md") else {
        return Err(InstallError::UpgradeRefused {
            detail: "Could not locate package plan.md template".to_string(),
        });
    };
    let bytes = std::fs::read(&template)
        .map_err(|error| InstallError::io("read template", &template, error))?;
    let mut changes = Vec::new();
    for directory in agent_commands::command_directories(project_root) {
        let agent_dir = project_root.join(&directory.directory);
        if !agent_dir.exists() {
            continue;
        }
        std::fs::create_dir_all(&agent_dir)
            .map_err(|error| InstallError::io("create directory", &agent_dir, error))?;
        let destination = agent_dir.join("spec-kitty.plan.md");
        paths::write_bytes(&destination, &bytes)?;
        changes.push(format!(
            "Updated {}/spec-kitty.plan.md",
            directory.directory
        ));
    }
    if changes.is_empty() {
        changes.push("No agents configured, skipping template update".to_string());
    }
    Ok(outcome(!changes.is_empty(), changes, Vec::new()))
}
