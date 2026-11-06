//! Skill-pack migrations (`2.0.11` … `2.1.3`).
//!
//! | id | effect |
//! | --- | --- |
//! | `2.0.11_install_skills` | install the canonical skill pack for every configured agent |
//! | `2.1.1_repair_skill_pack` | rebuild missing managed skills after the 2.1.0 wheel defect |
//! | `2.1.2_fix_orchestrator_api_skill` / `2.1.2_fix_runtime_next_skill` | replace an out-of-date `SKILL.md` with the canonical one |
//! | `2.1.2_install_git_workflow_skill` / `2.1.2_install_mission_system_skill` | install a newly shipped skill |
//! | `2.1.3_restore_prompt_commands` | replace thin `spec-kitty agent shim` stubs with full prompts |
//!
//! The install/repair pair delegates to the crate's one skill authority
//! ([`crate::skills`]) rather than writing files itself: the managed manifest,
//! collision policy and entry ordering are the same bytes a fresh `init`
//! produces, so an upgraded project and an initialized one cannot diverge.

use std::path::Path;

use sha2::Digest;

use crate::error::InstallError;
use crate::migrations::completion_legacy::agent_commands;
use crate::migrations::completion_legacy::assets;
use crate::migrations::completion_legacy::paths::{self, KITTIFY};
use crate::skills::{
    ManagedSkillManifest, SKILLS_MANIFEST_REL, SkillCatalog, SkillInstallOptions,
    install_skills_for_agents,
};
use crate::upgrade::MigrationOutcome;

fn outcome(changed: bool, changes: Vec<String>, warnings: Vec<String>) -> MigrationOutcome {
    MigrationOutcome {
        changed,
        changes,
        warnings,
        ..crate::upgrade::MigrationOutcome::default()
    }
}

fn configured_agents(project_root: &Path) -> Result<Vec<String>, InstallError> {
    crate::init::read_configured_agents(project_root)
}

/// The installable (non-wrapper) configured agents.
fn installable_agents(project_root: &Path, agents: &[String]) -> Vec<String> {
    let _ = project_root;
    agents
        .iter()
        .filter(|key| {
            crate::agent::agent_surface(key).is_ok_and(|surface| {
                surface.skill.class != crate::agent::SkillClass::WrapperOnly
                    && surface.skill.primary_root().is_some()
            })
        })
        .cloned()
        .collect()
}

/// Resolve the canonical skill catalog through the one project seam.
///
/// The seam merges the shipped doctrine skills with the project's charter-pack
/// skills in force (and stages the pack set when `stage`), so a migration never
/// sees a built-in-only inventory that would retire installed pack skills.
fn skill_catalog(project_root: &Path, stage: bool) -> Result<Option<SkillCatalog>, InstallError> {
    match crate::skills::resolve_project_skill_catalog(project_root, stage) {
        Ok(catalog) if catalog.skills().is_empty() => Ok(None),
        Ok(catalog) => Ok(Some(catalog)),
        Err(InstallError::SkillsCatalogMissing { .. }) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Whether every manifest entry still exists as a matching regular file
/// (`verify_installed_skills`).
pub fn verify_installed_skills(project_root: &Path) -> Result<bool, InstallError> {
    let Some(manifest) = ManagedSkillManifest::load_strict(project_root)? else {
        return Ok(true);
    };
    for entry in &manifest.entries {
        let installed = project_root.join(&entry.installed_path);
        let metadata = match std::fs::symlink_metadata(&installed) {
            Ok(metadata) => metadata,
            Err(_) => return Ok(false),
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Ok(false);
        }
        let bytes = match std::fs::read(&installed) {
            Ok(bytes) => bytes,
            Err(_) => return Ok(false),
        };
        let observed = format!("sha256:{:x}", sha2::Sha256::digest(&bytes));
        if observed != entry.content_hash {
            return Ok(false);
        }
    }
    Ok(true)
}

// ------------------------------------------------------ 2.0.11 install skills

/// `2.0.11_install_skills.detect`: no skills manifest yet.
pub fn detect_install_skills(project_root: &Path) -> Result<bool, InstallError> {
    Ok(!project_root.join(SKILLS_MANIFEST_REL).exists())
}

/// `2.0.11_install_skills.can_apply`.
pub fn can_apply_install_skills(project_root: &Path) -> Result<Option<String>, InstallError> {
    if project_root.join(KITTIFY).is_dir() {
        return Ok(None);
    }
    Ok(Some(
        ".kittify/ directory does not exist (not initialized)".to_string(),
    ))
}

/// `2.0.11_install_skills.apply`.
pub fn apply_install_skills(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let catalog = match skill_catalog(project_root, true)? {
        Some(catalog) => catalog,
        None => {
            return Ok(MigrationOutcome {
                changed: false,
                changes: Vec::new(),
                warnings: vec!["No skills found to install".to_string()],
                ..crate::upgrade::MigrationOutcome::default()
            });
        }
    };
    if catalog.skills().is_empty() {
        return Ok(MigrationOutcome {
            changed: false,
            changes: Vec::new(),
            warnings: vec!["No skills found to install".to_string()],
            ..crate::upgrade::MigrationOutcome::default()
        });
    }
    let agents = configured_agents(project_root)?;
    if agents.is_empty() {
        return Ok(MigrationOutcome {
            changed: false,
            changes: Vec::new(),
            warnings: vec!["No agents configured; skipping skill installation".to_string()],
            ..crate::upgrade::MigrationOutcome::default()
        });
    }
    let options = SkillInstallOptions::with_version("2.0.11");
    let report = install_skills_for_agents(project_root, &agents, &catalog, &options)?;
    if report.entries.is_empty() {
        return Err(InstallError::UpgradeRefused {
            detail: "No skill files were installed for any configured agent".to_string(),
        });
    }
    let mut changes = vec![
        format!(
            "Installed {} skill(s) for {} agent(s) ({} managed files)",
            catalog.skills().len(),
            agents.len(),
            report.entries.len()
        ),
        "Created .kittify/skills-manifest.json".to_string(),
    ];
    changes.extend(
        report
            .written
            .into_iter()
            .map(|path| format!("Wrote {path}")),
    );
    Ok(outcome(true, changes, report.warnings))
}

// -------------------------------------------------- 2.1.1 repair skill pack

/// `2.1.1_repair_skill_pack.detect`.
pub fn detect_repair_skill_pack(project_root: &Path) -> Result<bool, InstallError> {
    if !project_root.join(KITTIFY).is_dir() {
        return Ok(false);
    }
    let agents = match configured_agents(project_root) {
        Ok(agents) => agents,
        Err(_) => return Ok(true),
    };
    let installable = installable_agents(project_root, &agents);
    if installable.is_empty() {
        return Ok(false);
    }
    let catalog = match skill_catalog(project_root, false) {
        Ok(Some(catalog)) => catalog,
        Ok(None) => return Ok(true),
        Err(_) => return Ok(true),
    };
    if catalog.skills().is_empty() {
        return Ok(true);
    }
    let manifest = match ManagedSkillManifest::load_strict(project_root) {
        Ok(Some(manifest)) if !manifest.entries.is_empty() => manifest,
        Ok(_) => return Ok(true),
        Err(_) => return Ok(true),
    };
    let actual: Vec<(String, String)> = manifest
        .entries
        .iter()
        .filter(|entry| entry.source_file == "SKILL.md")
        .map(|entry| (entry.agent_key.clone(), entry.skill_name.clone()))
        .collect();
    for agent in &installable {
        for skill in catalog.skills() {
            if !actual.contains(&(agent.clone(), skill.name.clone())) {
                return Ok(true);
            }
        }
    }
    Ok(!verify_installed_skills(project_root)?)
}

/// `2.1.1_repair_skill_pack.can_apply`.
pub fn can_apply_repair_skill_pack(project_root: &Path) -> Result<Option<String>, InstallError> {
    if project_root.join(KITTIFY).is_dir() {
        return Ok(None);
    }
    Ok(Some(
        ".kittify/ directory does not exist (not initialized)".to_string(),
    ))
}

/// `2.1.1_repair_skill_pack.apply`.
pub fn apply_repair_skill_pack(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let Some(catalog) = skill_catalog(project_root, true)? else {
        return Err(InstallError::UpgradeRefused {
            detail: "No bundled skills found in the installed package".to_string(),
        });
    };
    if catalog.skills().is_empty() {
        return Err(InstallError::UpgradeRefused {
            detail: "No canonical skills discovered for repair".to_string(),
        });
    }
    let agents = configured_agents(project_root)?;
    let installable = installable_agents(project_root, &agents);
    if installable.is_empty() {
        return Ok(MigrationOutcome {
            changed: false,
            changes: Vec::new(),
            warnings: vec!["No skill-installing agents configured; skipping repair".to_string()],
            ..crate::upgrade::MigrationOutcome::default()
        });
    }
    let options = SkillInstallOptions {
        repair_drift: true,
        ..SkillInstallOptions::with_version("2.1.1")
    };
    let report = install_skills_for_agents(project_root, &installable, &catalog, &options)?;
    if report.entries.is_empty() {
        return Err(InstallError::UpgradeRefused {
            detail: "No skill files were installed for any configured agent".to_string(),
        });
    }
    let mut changes = vec![
        format!(
            "Installed {} skill(s) for {} agent(s) ({} managed files)",
            catalog.skills().len(),
            installable.len(),
            report.entries.len()
        ),
        "Rebuilt .kittify/skills-manifest.json".to_string(),
    ];
    changes.extend(
        report
            .written
            .into_iter()
            .map(|path| format!("Wrote {path}")),
    );
    Ok(outcome(true, changes, report.warnings))
}

// ------------------------------------------- 2.1.2 skill content refresh

fn skill_contains(path: &Path, markers: &[&str]) -> bool {
    match paths::read_text(path) {
        Some(content) => markers.iter().any(|marker| content.contains(marker)),
        None => false,
    }
}

/// Replace an out-of-date `SKILL.md` with the canonical file.
fn refresh_skill(
    project_root: &Path,
    skill: &str,
    label: &str,
    old_markers: &[&str],
) -> Result<MigrationOutcome, InstallError> {
    let Some(files) = assets::canonical_skill_files(skill) else {
        return Err(InstallError::UpgradeRefused {
            detail: format!("Cannot locate canonical SKILL.md for {label}"),
        });
    };
    let Some((_, canonical)) = files.iter().find(|(path, _)| path == "SKILL.md") else {
        return Err(InstallError::UpgradeRefused {
            detail: format!("Cannot locate canonical SKILL.md for {label}"),
        });
    };
    let mut changes = Vec::new();
    let mut replaced = 0usize;
    for path in agent_commands::find_skill_files(project_root, skill, Some("SKILL.md")) {
        if !skill_contains(&path, old_markers) {
            continue;
        }
        paths::write_bytes(&path, canonical)?;
        changes.push(format!(
            "Replaced {}",
            paths::relative_display(project_root, &path)
        ));
        replaced += 1;
    }
    if replaced == 0 {
        changes.push(format!("All {label} skill files already up to date"));
    }
    Ok(outcome(replaced > 0, changes, Vec::new()))
}

/// The orchestrator-api `SKILL.md` marker the repair replaces.
pub const ORCHESTRATOR_API_OLD_MARKERS: [&str; 1] =
    ["## Step 1: Verify the API Contract\n\n```bash\nspec-kitty orchestrator-api contract-version"];

/// The runtime-next `SKILL.md` marker the repair replaces.
pub const RUNTIME_NEXT_OLD_MARKERS: [&str; 1] =
    ["## Step 1: Load Runtime Context\n\nBefore invoking the runtime, gather"];

/// `2.1.2_fix_orchestrator_api_skill.detect`.
pub fn detect_fix_orchestrator_api_skill(project_root: &Path) -> Result<bool, InstallError> {
    Ok(agent_commands::find_skill_files(
        project_root,
        "spec-kitty-orchestrator-api-operator",
        Some("SKILL.md"),
    )
    .iter()
    .any(|path| skill_contains(path, &ORCHESTRATOR_API_OLD_MARKERS)))
}

/// `2.1.2_fix_runtime_next_skill.detect`.
pub fn detect_fix_runtime_next_skill(project_root: &Path) -> Result<bool, InstallError> {
    Ok(
        agent_commands::find_skill_files(project_root, "spec-kitty-runtime-next", Some("SKILL.md"))
            .iter()
            .any(|path| skill_contains(path, &RUNTIME_NEXT_OLD_MARKERS)),
    )
}

/// `2.1.2_fix_orchestrator_api_skill.apply`.
pub fn apply_fix_orchestrator_api_skill(
    project_root: &Path,
) -> Result<MigrationOutcome, InstallError> {
    refresh_skill(
        project_root,
        "spec-kitty-orchestrator-api-operator",
        "orchestrator-api",
        &ORCHESTRATOR_API_OLD_MARKERS,
    )
}

/// `2.1.2_fix_runtime_next_skill.apply`.
pub fn apply_fix_runtime_next_skill(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    refresh_skill(
        project_root,
        "spec-kitty-runtime-next",
        "runtime-next",
        &RUNTIME_NEXT_OLD_MARKERS,
    )
}

/// Install a skill that a release newly introduced.
fn install_new_skill(
    project_root: &Path,
    skill: &str,
    label: &str,
    extra_roots: bool,
) -> Result<MigrationOutcome, InstallError> {
    let Some(files) = assets::canonical_skill_files(skill) else {
        return Err(InstallError::UpgradeRefused {
            detail: format!("Cannot locate canonical SKILL.md for {label}"),
        });
    };
    let mut roots: Vec<String> = vec![".claude/skills".to_string(), ".agents/skills".to_string()];
    if extra_roots {
        for root in agent_commands::SKILL_ROOTS {
            let directory = project_root.join(root);
            if directory.is_dir() && !roots.iter().any(|existing| existing == root) {
                roots.push(root.to_string());
            }
        }
    }
    let mut changes = Vec::new();
    let mut installed = 0usize;
    for root in roots {
        for (relative, bytes) in &files {
            let destination = project_root.join(&root).join(skill).join(relative);
            if destination.exists() {
                continue;
            }
            paths::write_bytes(&destination, bytes)?;
            changes.push(format!(
                "Installed {}",
                paths::relative_display(project_root, &destination)
            ));
            installed += 1;
        }
    }
    if installed == 0 {
        changes.push(format!("{label} skill already installed"));
    }
    Ok(outcome(installed > 0, changes, Vec::new()))
}

/// `2.1.2_install_git_workflow_skill.detect`.
pub fn detect_install_git_workflow_skill(project_root: &Path) -> Result<bool, InstallError> {
    Ok(
        agent_commands::find_skill_files(project_root, "spec-kitty-git-workflow", Some("SKILL.md"))
            .is_empty(),
    )
}

/// `2.1.2_install_git_workflow_skill.apply`.
///
/// The pinned module installs into `.claude/skills` + `.agents/skills` and then
/// every other known root that already holds skills; the canonical file set is
/// `SKILL.md` plus `references/git-operations-matrix.md`.
pub fn apply_install_git_workflow_skill(
    project_root: &Path,
) -> Result<MigrationOutcome, InstallError> {
    install_new_skill(
        project_root,
        "spec-kitty-git-workflow",
        "git-workflow",
        true,
    )
}

/// `2.1.2_install_mission_system_skill.detect`.
pub fn detect_install_mission_system_skill(project_root: &Path) -> Result<bool, InstallError> {
    Ok(agent_commands::find_skill_files(
        project_root,
        "spec-kitty-mission-system",
        Some("SKILL.md"),
    )
    .is_empty())
}

/// `2.1.2_install_mission_system_skill.apply`.
pub fn apply_install_mission_system_skill(
    project_root: &Path,
) -> Result<MigrationOutcome, InstallError> {
    install_new_skill(
        project_root,
        "spec-kitty-mission-system",
        "mission-system",
        true,
    )
}

// ------------------------------------------- 2.1.3 restore prompt commands

/// `2.1.3_restore_prompt_commands.can_apply`: the runtime command templates
/// must be resolvable for the rendering the restore performs.
pub fn can_apply_restore_prompt_commands(
    project_root: &Path,
) -> Result<Option<String>, InstallError> {
    let _ = project_root;
    if agent_commands::runtime_command_templates("software-dev").is_some() {
        return Ok(None);
    }
    Ok(Some(
        "Runtime command templates not found. Run 'spec-kitty upgrade' again after reinstalling spec-kitty-cli."
            .to_string(),
    ))
}

/// `2.1.3_restore_prompt_commands.detect`: any prompt-driven command file is a
/// thin shim.
pub fn detect_restore_prompt_commands(project_root: &Path) -> Result<bool, InstallError> {
    for directory in agent_commands::command_directories(project_root) {
        let agent_dir = project_root.join(&directory.directory);
        if !agent_dir.is_dir() {
            continue;
        }
        for command in agent_commands::PROMPT_DRIVEN_COMMANDS {
            for candidate in
                agent_commands::shim_candidates(&agent_dir, command, &directory.agent_key)
            {
                if candidate.exists() && agent_commands::is_thin_shim(&candidate) {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

/// `2.1.3_restore_prompt_commands.apply`.
pub fn apply_restore_prompt_commands(
    project_root: &Path,
) -> Result<MigrationOutcome, InstallError> {
    let Some(templates_dir) = agent_commands::runtime_command_templates("software-dev") else {
        return Err(InstallError::UpgradeRefused {
            detail: "Runtime command templates not found — cannot restore full prompts".to_string(),
        });
    };
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    let mut restored = 0usize;
    let mut commands: Vec<&str> = agent_commands::PROMPT_DRIVEN_COMMANDS.to_vec();
    commands.sort_unstable();
    for directory in agent_commands::command_directories(project_root) {
        let agent_dir = project_root.join(&directory.directory);
        if !agent_dir.is_dir() {
            continue;
        }
        let agent_key = directory.agent_key.clone();
        for command in &commands {
            let template = agent_commands::template_path(&templates_dir, command);
            if !template.is_file() {
                warnings.push(format!(
                    "Template not found for command '{command}' in {} — skipping",
                    templates_dir.display()
                ));
                continue;
            }
            let candidates = agent_commands::shim_candidates(&agent_dir, command, &agent_key);
            let Some(shim) = candidates
                .into_iter()
                .find(|candidate| candidate.exists() && agent_commands::is_thin_shim(candidate))
            else {
                continue;
            };
            let output_path = agent_dir.join(agent_commands::output_filename(command, &agent_key));
            let rendered = match agent_commands::render_prompt_for_project(
                &agent_key,
                command,
                project_root,
            ) {
                Ok(rendered) => rendered,
                Err(_) => {
                    warnings.push(format!("Failed to render {command} for {agent_key}"));
                    continue;
                }
            };
            paths::write_text(&output_path, &rendered)?;
            restored += 1;
            if shim != output_path {
                match std::fs::remove_file(&shim) {
                    Ok(()) => changes.push(format!(
                        "Restored: {} -> {} (removed stale shim)",
                        paths::relative_display(project_root, &shim),
                        paths::relative_display(project_root, &output_path)
                    )),
                    Err(error) => {
                        warnings.push(format!(
                            "Could not remove stale shim {}: {error}",
                            paths::relative_display(project_root, &shim)
                        ));
                        changes.push(format!(
                            "Restored: {}",
                            paths::relative_display(project_root, &output_path)
                        ));
                    }
                }
            } else {
                changes.push(format!(
                    "Restored: {}",
                    paths::relative_display(project_root, &output_path)
                ));
            }
        }
    }
    if changes.is_empty() && warnings.is_empty() {
        changes.push("No thin shims found for prompt-driven commands — nothing to do".to_string());
    }
    Ok(outcome(restored > 0, changes, warnings))
}
