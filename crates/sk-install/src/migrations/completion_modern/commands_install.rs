//! The command/skill installer family of modern migrations.
//!
//! Every effect here goes through the *existing* canonical installer seams:
//!
//! * the offering skill catalog ([`SkillCatalog::discover`]) plus the managed
//!   skills writer ([`install_skills_for_agents`]) for skill bodies;
//! * [`render_skill_asset`] plus [`InstallPlan`] for command skills — the same
//!   renderer, ownership manifest, collision/preservation rules and legacy
//!   retire rule `init` uses, so a migration can never delete user content the
//!   installer would keep;
//! * the shared façade ([`shared`]) for asset roots, command directories,
//!   template rendering and generated-file writes;
//! * the `.gitignore` writer for ignore backfills.
//!
//! Nothing here adds a second renderer, manifest or ownership authority.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::support;
use crate::agent::{AgentSurface, agent_surface, agents};
use crate::error::InstallError;
use crate::migrations::completion_legacy::agent_commands;
use crate::migrations::completion_legacy::shared;
use crate::migrations::retirements::AgentDirectory;
use crate::plan::{InstallPlan, PlanOptions};
use crate::preserve::{Prover, guard_destructive_removal};
use crate::render::{RenderedAsset, TemplateSource, render_skill_asset};
use crate::skills::{SkillCatalog, SkillInstallOptions, install_skills_for_agents};
use crate::upgrade::MigrationOutcome;

const VERSION: &str = sk_kernel::COMPAT_CORE_VERSION;
const LOCK_WAIT: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// Shared installer plumbing
// ---------------------------------------------------------------------------

fn configured_agents(project_root: &Path) -> Result<Vec<String>, InstallError> {
    Ok(support::load_agent_config(project_root)?.available)
}

fn configured_command_skill_agents(project_root: &Path) -> Vec<String> {
    let configured = configured_agents(project_root).unwrap_or_default();
    configured
        .iter()
        .filter(|key| agent_surface(key).is_ok_and(|surface| surface.installs_command_skills()))
        .cloned()
        .collect()
}

fn installable_agents(project_root: &Path) -> Vec<String> {
    let configured = configured_agents(project_root).unwrap_or_default();
    configured
        .iter()
        .filter(|key| {
            agent_surface(key).is_ok_and(|surface| {
                surface.skill.primary_root().is_some()
                    && surface.skill.class != crate::agent::SkillClass::WrapperOnly
            })
        })
        .cloned()
        .collect()
}

fn offering_catalog() -> Option<SkillCatalog> {
    let root = shared::offering_skills_root()?;
    SkillCatalog::discover(&root).ok()
}

fn template_source() -> Option<TemplateSource> {
    shared::asset_root().map(TemplateSource::new)
}

/// Render and commit the canonical command skills for `agents`.
///
/// `dotted` selects the pre-#6475 `spec-kitty.<cmd>` name; the current name is
/// `spec-kitty-<cmd>`. The bytes always come from the one renderer.
fn install_command_skills(
    project_root: &Path,
    agent_keys: &[String],
    dotted: bool,
) -> Result<crate::plan::ApplicationReport, InstallError> {
    let templates = template_source().map(|source| source.for_project(project_root));
    let mut rendered: Vec<RenderedAsset> = Vec::new();
    for key in agent_keys {
        let surface: &AgentSurface = agent_surface(key)?;
        if !surface.installs_command_skills() {
            continue;
        }
        for command in crate::agent::CANONICAL_COMMANDS {
            let mut asset = render_skill_asset(surface, command, VERSION, templates.as_ref())?;
            if dotted {
                let name = format!("spec-kitty.{command}");
                asset.path = format!(".agents/skills/{name}/SKILL.md");
            }
            rendered.push(asset);
        }
    }
    if rendered.is_empty() {
        return Ok(crate::plan::ApplicationReport::default());
    }
    let plan = InstallPlan::plan(project_root, VERSION, &rendered, &PlanOptions::new())?;
    plan.commit(LOCK_WAIT)
}

fn is_dotted_command_skill_name(name: &str) -> bool {
    name.starts_with("spec-kitty.")
}

fn manifest_is_dotted(project_root: &Path) -> bool {
    crate::manifest::AssetManifest::load(project_root)
        .ok()
        .flatten()
        .is_some_and(|manifest| {
            manifest
                .entries()
                .iter()
                .any(|entry| is_dotted_command_skill_name(&entry.path))
        })
}

fn dotted_command_folders_on_disk(project_root: &Path) -> Vec<String> {
    let mut folders = Vec::new();
    for command in crate::agent::CANONICAL_COMMANDS {
        let relative = format!(".agents/skills/spec-kitty.{command}");
        if project_root.join(&relative).exists() {
            folders.push(format!("{relative}/SKILL.md"));
        }
    }
    folders
}

fn describe_install_report(report: &crate::plan::ApplicationReport, changes: &mut Vec<String>) {
    for path in &report.deleted {
        if is_dotted_command_skill_name(path) {
            changes.push(format!("Removed legacy dotted command skill {path}"));
        }
    }
    for path in &report.written {
        changes.push(format!("Installed command skill {path}"));
    }
}

/// Install the whole offering catalog for `agents` and report Source-shaped lines.
fn install_catalog(
    project_root: &Path,
    agent_keys: &[String],
    catalog: &SkillCatalog,
    version: &str,
) -> Result<MigrationOutcome, InstallError> {
    let options = SkillInstallOptions::with_version(version);
    let report = install_skills_for_agents(project_root, agent_keys, catalog, &options)?;
    let mut changes = vec![format!(
        "Installed {} canonical skill(s) for {} agent(s) ({} managed files)",
        catalog.skills().len(),
        agent_keys.len(),
        report.entries.len()
    )];
    changes.extend(
        report
            .written
            .iter()
            .map(|path| format!("Installed {path}")),
    );
    changes.extend(report.adopted.iter().map(|path| format!("Adopted {path}")));
    let mut warnings = report.warnings.clone();
    warnings.extend(
        report
            .drifted
            .iter()
            .map(|path| format!("Skill file drifted from the manifest and was kept: {path}")),
    );
    warnings.extend(
        report
            .preserved
            .iter()
            .map(|path| format!("Preserved unowned skill file: {path}")),
    );
    Ok(MigrationOutcome {
        changed: !(report.written.is_empty() && report.entries.is_empty()),
        changes,
        warnings,
        ..crate::upgrade::MigrationOutcome::default()
    })
}

// ---------------------------------------------------------------------------
// 3.2.0rc35_spk_skill_pack
// ---------------------------------------------------------------------------

fn spk_catalog(catalog: &SkillCatalog) -> Option<SkillCatalog> {
    let filtered = catalog.filtered(|skill| skill.name.starts_with("spk-"));
    (!filtered.skills().is_empty()).then_some(filtered)
}

fn skill_files_present(project_root: &Path, agent_key: &str, catalog: &SkillCatalog) -> bool {
    let Some(surface) = agent_surface(agent_key).ok() else {
        return true;
    };
    let Some(root) = surface.skill.primary_root() else {
        return true;
    };
    for skill in catalog.skills() {
        for file in &skill.files {
            let path = project_root
                .join(root)
                .join(&skill.name)
                .join(&file.relative);
            if !path.exists() {
                return false;
            }
        }
    }
    true
}

pub fn detect_spk_skill_pack(project_root: &Path) -> Result<bool, InstallError> {
    if !project_root.join(".kittify").is_dir() {
        return Ok(false);
    }
    let Some(catalog) = offering_catalog() else {
        return Ok(false);
    };
    let Some(spk) = spk_catalog(&catalog) else {
        return Ok(false);
    };
    let agents = installable_agents(project_root);
    if agents.is_empty() {
        return Ok(false);
    }
    Ok(agents
        .iter()
        .any(|agent| !skill_files_present(project_root, agent, &spk)))
}

pub fn can_apply_spk_skill_pack(project_root: &Path) -> Result<Option<String>, InstallError> {
    if project_root.join(".kittify").is_dir() {
        Ok(None)
    } else {
        Ok(Some(
            ".kittify/ directory does not exist (not initialized)".to_string(),
        ))
    }
}

pub fn apply_spk_skill_pack(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let Some(catalog) = offering_catalog() else {
        return Err(InstallError::UpgradeRefused {
            detail: "No canonical skills discovered for spk skill-pack install".to_string(),
        });
    };
    let Some(spk) = spk_catalog(&catalog) else {
        return Err(InstallError::UpgradeRefused {
            detail: "No spk skills discovered for install".to_string(),
        });
    };
    let agents = installable_agents(project_root);
    if agents.is_empty() {
        return Ok(MigrationOutcome {
            changed: false,
            changes: Vec::new(),
            warnings: vec![
                "No skill-installing agents configured; skipping spk skill pack".to_string(),
            ],
            ..crate::upgrade::MigrationOutcome::default()
        });
    }
    let mut outcome = install_catalog(project_root, &agents, &spk, "3.2.0rc35")?;
    outcome.changes.insert(
        0,
        format!(
            "Installed {} spk skill(s) for {} agent(s)",
            spk.skills().len(),
            agents.len()
        ),
    );
    outcome
        .changes
        .push("Updated .kittify/skills-manifest.json with spk skill entries".to_string());
    Ok(outcome)
}

// ---------------------------------------------------------------------------
// 3.2.0rc30_fix_runtime_next_result_default
//    3.2.0rc35_fix_prompt_file_workaround
// ---------------------------------------------------------------------------

const RUNTIME_NEXT_SKILL: &str = "spec-kitty-runtime-next";

const RESULT_DEFAULT_MARKERS: [&str; 1] = ["Defaults to `success` if omitted."];

const PROMPT_FILE_MARKERS: [&str; 4] = [
    "Check `prompt_file` is not null, then read and execute",
    "Workaround #336: treat null prompt as blocked",
    "#336 — `prompt_file` can be `null` on `step` decisions",
    "Treat a null `prompt_file` as a blocked state",
];

fn file_contains_any(path: &Path, markers: &[&str]) -> bool {
    let Some(text) = shared::read_text(path) else {
        return false;
    };
    markers.iter().any(|marker| text.contains(marker))
}

fn detect_runtime_next_markers(project_root: &Path, markers: &[&str]) -> bool {
    shared::find_skill_files(project_root, RUNTIME_NEXT_SKILL, Some("SKILL.md"))
        .iter()
        .any(|path| file_contains_any(path, markers))
}

fn apply_runtime_next_markers(
    project_root: &Path,
    markers: &[&str],
) -> Result<MigrationOutcome, InstallError> {
    let Some(new_content) = shared::canonical_skill_text(RUNTIME_NEXT_SKILL, "SKILL.md") else {
        return Err(InstallError::UpgradeRefused {
            detail: "Cannot locate canonical SKILL.md for runtime-next".to_string(),
        });
    };
    let mut changes = Vec::new();
    for path in shared::find_skill_files(project_root, RUNTIME_NEXT_SKILL, Some("SKILL.md")) {
        if !file_contains_any(&path, markers) {
            continue;
        }
        let relative = shared::relative_display(project_root, &path);
        // The canonical skill body is generated content: write it through the
        // managed generated writer, which restores the read-only bit the
        // installer applied.
        support::write_generated_file(&path, &new_content, true)?;
        changes.push(format!("Replaced {relative}"));
    }
    if changes.is_empty() {
        changes.push("All runtime-next skill files already up to date".to_string());
    }
    Ok(MigrationOutcome {
        changed: !changes.is_empty(),
        changes,
        warnings: Vec::new(),
        ..crate::upgrade::MigrationOutcome::default()
    })
}

pub fn detect_runtime_next_result_default(project_root: &Path) -> Result<bool, InstallError> {
    Ok(detect_runtime_next_markers(
        project_root,
        &RESULT_DEFAULT_MARKERS,
    ))
}

pub fn apply_runtime_next_result_default(
    project_root: &Path,
) -> Result<MigrationOutcome, InstallError> {
    apply_runtime_next_markers(project_root, &RESULT_DEFAULT_MARKERS)
}

pub fn detect_prompt_file_workaround(project_root: &Path) -> Result<bool, InstallError> {
    Ok(detect_runtime_next_markers(
        project_root,
        &PROMPT_FILE_MARKERS,
    ))
}

pub fn apply_prompt_file_workaround(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    apply_runtime_next_markers(project_root, &PROMPT_FILE_MARKERS)
}

// ---------------------------------------------------------------------------
// 3.1.2_globalize_commands / 3.2.0a4_safe_globalize_commands
// ---------------------------------------------------------------------------

const VERSION_MARKER_PREFIX: &str = "<!-- spec-kitty-command-version:";
const VERSION_MARKER_HEAD_LINES: usize = 15;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GlobalizeAction {
    Hold,
    Keep,
    Remove,
}

fn globalize_directories(project_root: &Path) -> Vec<AgentDirectory> {
    let mut directories = shared::command_directories(project_root);
    let legacy_codex = project_root.join(".codex/prompts");
    if legacy_codex.is_dir()
        && !directories
            .iter()
            .any(|directory| directory.directory == ".codex/prompts")
    {
        directories.push(AgentDirectory {
            directory: ".codex/prompts".to_string(),
            agent_key: "codex".to_string(),
        });
    }
    directories
}

fn global_runtime_present_at(home: &Path) -> bool {
    let missions = home.join("missions");
    std::fs::read_dir(&missions)
        .map(|entries| entries.flatten().any(|entry| entry.path().is_dir()))
        .unwrap_or(false)
}

fn global_runtime_present() -> bool {
    shared::kittify_home()
        .map(|home| global_runtime_present_at(&home))
        .unwrap_or(false)
}

fn global_command_file_present(
    home: &Path,
    agent_root: &str,
    subdir: &str,
    filename: &str,
) -> bool {
    home.join(agent_root).join(subdir).join(filename).is_file()
}

fn is_generated_command_file(path: &Path) -> bool {
    let Some(text) = shared::read_text(path) else {
        return false;
    };
    text.lines()
        .take(VERSION_MARKER_HEAD_LINES)
        .any(|line| line.trim().starts_with(VERSION_MARKER_PREFIX))
}

fn classify_globalize(
    home: &Path,
    target: &Path,
    agent_root: &str,
    subdir: &str,
    runtime_present: bool,
) -> (GlobalizeAction, String) {
    if !is_generated_command_file(target) {
        return (
            GlobalizeAction::Hold,
            "no version marker (user-authored or unproven file)".to_string(),
        );
    }
    if !runtime_present {
        return (
            GlobalizeAction::Keep,
            "global runtime (~/.kittify/missions/) not found".to_string(),
        );
    }
    if !global_command_file_present(
        home,
        agent_root,
        subdir,
        &target.file_name().unwrap_or_default().to_string_lossy(),
    ) {
        return (
            GlobalizeAction::Keep,
            format!(
                "no global equivalent at ~/{agent_root}/{subdir}/{}",
                target.file_name().unwrap_or_default().to_string_lossy()
            ),
        );
    }
    (GlobalizeAction::Remove, String::new())
}

pub fn detect_globalize_commands(project_root: &Path) -> Result<bool, InstallError> {
    for directory in globalize_directories(project_root) {
        let agent_dir = project_root.join(&directory.directory);
        if !agent_dir.is_dir() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&agent_dir) else {
            continue;
        };
        if entries.flatten().any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("spec-kitty.")
        }) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn apply_globalize_commands(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    apply_globalize_commands_at(project_root, &home, global_runtime_present())
}

fn apply_globalize_commands_at(
    project_root: &Path,
    home: &Path,
    runtime_present: bool,
) -> Result<MigrationOutcome, InstallError> {
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    for directory in globalize_directories(project_root) {
        let agent_dir = project_root.join(&directory.directory);
        if !agent_dir.is_dir() {
            continue;
        }
        let mut candidates: Vec<PathBuf> = std::fs::read_dir(&agent_dir)
            .map_err(|error| InstallError::io("read directory", &agent_dir, error))?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("spec-kitty."))
            })
            .collect();
        candidates.sort();
        for target in candidates {
            let relative = shared::relative_display(project_root, &target);
            let (agent_root, subdir) = match directory.directory.rsplit_once('/') {
                Some((root, subdir)) => (root, subdir),
                None => (directory.directory.as_str(), ""),
            };
            let (action, reason) =
                classify_globalize(home, &target, agent_root, subdir, runtime_present);
            match action {
                GlobalizeAction::Hold => changes.push(format!("Preserved: {relative} ({reason})")),
                GlobalizeAction::Keep => {
                    changes.push(format!("Kept: {relative} (generated; {reason})"))
                }
                GlobalizeAction::Remove => {
                    if let Ok(metadata) = std::fs::symlink_metadata(&target)
                        && let Ok(mut permissions) =
                            Ok::<_, std::io::Error>(metadata.permissions().clone())
                    {
                        use std::os::unix::fs::PermissionsExt;
                        permissions.set_mode(metadata.permissions().mode() | 0o222);
                        let _ = std::fs::set_permissions(&target, permissions);
                    }
                    match std::fs::remove_file(&target) {
                        Ok(()) => changes.push(format!("Removed: {relative}")),
                        Err(error) => {
                            changes.push(format!(
                                "Kept: {relative} (generated; unlink failed: {error})"
                            ));
                            warnings.push(format!("Could not remove {relative}: {error}"));
                        }
                    }
                }
            }
        }
        if agent_dir.is_dir() && std::fs::remove_dir(&agent_dir).is_ok() {
            changes.push(format!(
                "Removed empty directory: {}",
                shared::relative_display(project_root, &agent_dir)
            ));
        }
    }
    Ok(MigrationOutcome {
        changed: !changes.is_empty(),
        changes,
        warnings,
        ..crate::upgrade::MigrationOutcome::default()
    })
}

// ---------------------------------------------------------------------------
// 3.2.0rc35_codex_to_skills
// ---------------------------------------------------------------------------

/// The pre-3.2 Codex prompt filenames (`_legacy_codex_hashes.LEGACY_CODEX_FILENAMES`).
const LEGACY_CODEX_FILENAMES: [&str; 11] = [
    "spec-kitty.analyze.md",
    "spec-kitty.charter.md",
    "spec-kitty.implement.md",
    "spec-kitty.plan.md",
    "spec-kitty.research.md",
    "spec-kitty.review.md",
    "spec-kitty.specify.md",
    "spec-kitty.tasks.md",
    "spec-kitty.tasks-finalize.md",
    "spec-kitty.tasks-outline.md",
    "spec-kitty.tasks-packages.md",
];

fn codex_prompt_files(project_root: &Path) -> Vec<PathBuf> {
    let prompts_dir = project_root.join(".codex/prompts");
    if !prompts_dir.is_dir() {
        return Vec::new();
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(&prompts_dir)
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default();
    files.sort();
    files
}

fn is_owned_codex_prompt(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| LEGACY_CODEX_FILENAMES.contains(&name.to_string_lossy().as_ref()))
}

pub fn detect_codex_to_skills(project_root: &Path) -> Result<bool, InstallError> {
    Ok(codex_prompt_files(project_root)
        .iter()
        .any(|path| path.is_file() && is_owned_codex_prompt(path)))
}

pub fn can_apply_codex_to_skills(project_root: &Path) -> Result<Option<String>, InstallError> {
    if project_root.join(".kittify").is_dir() {
        Ok(None)
    } else {
        Ok(Some(
            ".kittify/ directory does not exist (not initialized)".to_string(),
        ))
    }
}

pub fn apply_codex_to_skills(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let configured = configured_agents(project_root)?;
    if !configured.iter().any(|agent| agent == "codex") {
        return Ok(MigrationOutcome::skipped());
    }
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    install_command_skills(project_root, &["codex".to_string()], true)?;
    changes.push("Installed 11 Codex skill packages under .agents/skills/".to_string());

    let prompts_dir = project_root.join(".codex/prompts");
    let owned: Vec<PathBuf> = codex_prompt_files(project_root)
        .into_iter()
        .filter(|path| path.is_file() && is_owned_codex_prompt(path))
        .collect();
    let third_party: Vec<PathBuf> = codex_prompt_files(project_root)
        .into_iter()
        .filter(|path| path.is_file() && !is_owned_codex_prompt(path))
        .collect();
    if !owned.is_empty() {
        let superseded = project_root.join(".codex/prompts.superseded");
        std::fs::create_dir_all(&superseded)
            .map_err(|error| InstallError::io("create directory", &superseded, error))?;
        let mut moved = 0usize;
        for path in owned {
            let target = superseded.join(path.file_name().unwrap_or_default());
            if target.exists() {
                std::fs::remove_file(&path)
                    .map_err(|error| InstallError::io("remove", &path, error))?;
                continue;
            }
            match std::fs::rename(&path, &target) {
                Ok(()) => moved += 1,
                Err(error) => warnings.push(format!(
                    "{}: {error}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                )),
            }
        }
        if moved > 0 {
            changes.push(format!(
                "Moved {moved} superseded Codex prompt(s) from .codex/prompts/ to .codex/prompts.superseded/ (new skill packages live in .agents/skills/)"
            ));
        }
    }
    if !third_party.is_empty() {
        warnings.push(format!(
            "Preserved {} third-party file(s) in .codex/prompts/",
            third_party.len()
        ));
    }
    if prompts_dir.is_dir() {
        match std::fs::read_dir(&prompts_dir) {
            Ok(mut entries) => {
                if entries.next().is_none() && std::fs::remove_dir(&prompts_dir).is_ok() {
                    changes.push("Removed empty .codex/prompts/ directory".to_string());
                }
            }
            Err(error) => warnings.push(format!(
                "Could not remove .codex/prompts/ directory: {error}"
            )),
        }
    }
    Ok(MigrationOutcome {
        changed: !changes.is_empty(),
        changes,
        warnings,
        ..crate::upgrade::MigrationOutcome::default()
    })
}

// ---------------------------------------------------------------------------
// 3.2.0rc35_pi_letta_agent_backfill
// ---------------------------------------------------------------------------

const PI_LETTA_ENTRIES: [(&str, &str); 2] = [("pi", ".pi/"), ("letta", ".letta/")];

fn command_skill_paths_complete(project_root: &Path, dotted: bool) -> bool {
    crate::agent::CANONICAL_COMMANDS.iter().all(|command| {
        let name = if dotted {
            format!("spec-kitty.{command}")
        } else {
            format!("spec-kitty-{command}")
        };
        project_root
            .join(".agents/skills")
            .join(name)
            .join("SKILL.md")
            .exists()
    })
}

fn agent_skill_manifest_complete(project_root: &Path, agent_key: &str, dotted: bool) -> bool {
    let Ok(Some(manifest)) = crate::manifest::AssetManifest::load(project_root) else {
        return false;
    };
    crate::agent::CANONICAL_COMMANDS.iter().all(|command| {
        let name = if dotted {
            format!("spec-kitty.{command}")
        } else {
            format!("spec-kitty-{command}")
        };
        let relative = format!(".agents/skills/{name}/SKILL.md");
        manifest
            .find(&relative)
            .is_some_and(|entry| entry.agents.iter().any(|owner| owner == agent_key))
    })
}

fn agent_skills_complete(project_root: &Path, agent_key: &str) -> bool {
    let dotted = manifest_is_dotted(project_root)
        || !dotted_command_folders_on_disk(project_root).is_empty();
    command_skill_paths_complete(project_root, dotted)
        && agent_skill_manifest_complete(project_root, agent_key, dotted)
}

pub fn detect_pi_letta_backfill(project_root: &Path) -> Result<bool, InstallError> {
    if !project_root.join(".kittify").is_dir() {
        return Ok(false);
    }
    let Ok(configured) = configured_agents(project_root) else {
        return Ok(false);
    };
    let pi_letta: Vec<&str> = configured
        .iter()
        .filter(|key| PI_LETTA_ENTRIES.iter().any(|(agent, _)| agent == key))
        .map(String::as_str)
        .collect();
    if pi_letta.is_empty() {
        return Ok(false);
    }
    let gitignore = shared::read_text(&project_root.join(".gitignore")).unwrap_or_default();
    for (agent, entry) in PI_LETTA_ENTRIES {
        if pi_letta.contains(&agent) && !gitignore.contains(entry) {
            return Ok(true);
        }
    }
    Ok(pi_letta
        .iter()
        .any(|agent| !agent_skills_complete(project_root, agent)))
}

pub fn can_apply_pi_letta_backfill(project_root: &Path) -> Result<Option<String>, InstallError> {
    if project_root.exists() {
        Ok(None)
    } else {
        Ok(Some(format!(
            "Project path does not exist: {}",
            project_root.display()
        )))
    }
}

pub fn apply_pi_letta_backfill(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let configured = configured_agents(project_root)?;
    let dotted = manifest_is_dotted(project_root)
        || !dotted_command_folders_on_disk(project_root).is_empty();
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    for (agent, entry) in PI_LETTA_ENTRIES {
        if !configured.iter().any(|key| key == agent) {
            continue;
        }
        let gitignore = shared::read_text(&project_root.join(".gitignore")).unwrap_or_default();
        if !gitignore.contains(entry) {
            crate::migrations::ignore::ensure_entries(project_root, &[entry], false)?;
            changes.push(format!("Added {entry} to .gitignore"));
        }
    }
    for (agent, _) in PI_LETTA_ENTRIES {
        if !configured.iter().any(|key| key == agent) {
            continue;
        }
        if agent_skills_complete(project_root, agent) {
            continue;
        }
        match install_command_skills(project_root, &[agent.to_string()], dotted) {
            Ok(report) => {
                describe_install_report(&report, &mut changes);
                changes.push(format!("Repaired skill pack for {agent}"));
            }
            Err(error) => warnings.push(format!("Skill repair for {agent} skipped: {error}")),
        }
    }
    Ok(MigrationOutcome {
        changed: !changes.is_empty(),
        changes,
        warnings,
        ..crate::upgrade::MigrationOutcome::default()
    })
}

// ---------------------------------------------------------------------------
// 3.2.0rc35_update_planning_templates
// ---------------------------------------------------------------------------

const STALE_PLANNING_MARKER: &str = "Create `tasks.md`";
const PLANNING_COMMANDS: [&str; 2] = ["tasks-outline", "tasks-packages"];

fn planning_stale_file(agent_dir: &Path, command: &str) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(agent_dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name().is_some_and(|name| {
                let name = name.to_string_lossy();
                name.starts_with(&format!("spec-kitty.{command}."))
            })
        })
        .collect();
    candidates.sort();
    candidates
        .into_iter()
        .find(|path| file_contains_any(path, &[STALE_PLANNING_MARKER]))
}

pub fn detect_update_planning_templates(project_root: &Path) -> Result<bool, InstallError> {
    for directory in shared::command_directories(project_root) {
        let agent_dir = project_root.join(&directory.directory);
        if !agent_dir.is_dir() {
            continue;
        }
        for command in PLANNING_COMMANDS {
            if planning_stale_file(&agent_dir, command).is_some() {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub fn can_apply_update_planning_templates(
    _project_root: &Path,
) -> Result<Option<String>, InstallError> {
    if agent_commands::runtime_command_templates("software-dev").is_some() {
        Ok(None)
    } else {
        Ok(Some(
            "Runtime command templates not found. Run 'spec-kitty upgrade' again after reinstalling spec-kitty-cli."
                .to_string(),
        ))
    }
}

pub fn apply_update_planning_templates(
    project_root: &Path,
) -> Result<MigrationOutcome, InstallError> {
    let Some(templates_dir) = agent_commands::runtime_command_templates("software-dev") else {
        return Err(InstallError::UpgradeRefused {
            detail: "Runtime command templates not found — cannot update planning templates"
                .to_string(),
        });
    };
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    for directory in shared::command_directories(project_root) {
        let agent_dir = project_root.join(&directory.directory);
        if !agent_dir.is_dir() {
            continue;
        }
        for command in PLANNING_COMMANDS {
            let template = agent_commands::template_path(&templates_dir, command);
            if !template.is_file() {
                warnings.push(format!(
                    "Template not found for command '{command}' in {} — skipping",
                    templates_dir.display()
                ));
                continue;
            }
            let Some(stale) = planning_stale_file(&agent_dir, command) else {
                continue;
            };
            let relative = shared::relative_display(project_root, &stale);
            let rendered = match agent_commands::render_prompt(&directory.agent_key, command) {
                Ok(rendered) => rendered,
                Err(_) => {
                    warnings.push(format!(
                        "Failed to render {command} for {}",
                        directory.agent_key
                    ));
                    continue;
                }
            };
            let output = agent_dir.join(shared::output_filename(command, &directory.agent_key));
            support::write_generated_file(&output, &rendered, false)?;
            changes.push(format!(
                "Updated: {}",
                shared::relative_display(project_root, &output)
            ));
            let _ = relative;
        }
    }
    Ok(MigrationOutcome {
        changed: !changes.is_empty(),
        changes,
        warnings,
        ..crate::upgrade::MigrationOutcome::default()
    })
}

// ---------------------------------------------------------------------------
// 3.2.0rc35_activate_builtin_mission_types
// ---------------------------------------------------------------------------

const MISSION_TYPES_KEY: &str = "mission_type_activations";
const ALREADY_PRESENT: &str = "mission_type_activations already present; no changes needed";

fn builtin_mission_type_ids() -> Vec<String> {
    let Some(checkout) = shared::checkout_root() else {
        return Vec::new();
    };
    let directory = checkout.join("packs/built-in/missions/mission_types");
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            entry
                .path()
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .collect();
    ids.sort();
    ids
}

/// The activation write target: the pointed `charter.yaml`, else `config.yaml`.
fn activation_write_target(project_root: &Path) -> PathBuf {
    let config_path = project_root.join(".kittify/config.yaml");
    let Some(text) = shared::read_text(&config_path) else {
        return config_path;
    };
    let Ok(document) = crate::yaml_edit::CstDocument::parse(&config_path, &text) else {
        return config_path;
    };
    if let Some(pointer) = document.root().get_string("charter") {
        let path = PathBuf::from(pointer);
        return if path.is_absolute() {
            path
        } else {
            project_root.join(path)
        };
    }
    config_path
}

fn mission_types_present_in_write_target(project_root: &Path) -> bool {
    let target = activation_write_target(project_root);
    let Some(text) = shared::read_text(&target) else {
        return false;
    };
    let Ok(document) = crate::yaml_edit::CstDocument::parse(&target, &text) else {
        return false;
    };
    document
        .root()
        .get_string_list(MISSION_TYPES_KEY)
        .is_some_and(|list| !list.is_empty())
}

pub fn detect_activate_builtin_mission_types(project_root: &Path) -> Result<bool, InstallError> {
    let config_file = project_root.join(".kittify/config.yaml");
    if !config_file.exists() || mission_types_present_in_write_target(project_root) {
        return Ok(false);
    }
    let Some(text) = shared::read_text(&config_file) else {
        return Ok(false);
    };
    let Ok(document) = crate::yaml_edit::CstDocument::parse(&config_file, &text) else {
        return Ok(false);
    };
    Ok(!document
        .root()
        .get_string_list(MISSION_TYPES_KEY)
        .is_some_and(|list| !list.is_empty()))
}

pub fn can_apply_activate_builtin_mission_types(
    project_root: &Path,
) -> Result<Option<String>, InstallError> {
    let config_file = project_root.join(".kittify/config.yaml");
    if !config_file.exists() {
        return Ok(None);
    }
    if !config_file.is_file() {
        return Ok(Some("config path exists but is not a file".to_string()));
    }
    match shared::read_text(&config_file) {
        Some(_) => Ok(None),
        None => Ok(Some("config file not readable".to_string())),
    }
}

pub fn apply_activate_builtin_mission_types(
    project_root: &Path,
) -> Result<MigrationOutcome, InstallError> {
    let config_file = project_root.join(".kittify/config.yaml");
    if !config_file.exists() {
        return Ok(MigrationOutcome::unchanged(vec![
            "No .kittify/config.yaml found; nothing to migrate".to_string(),
        ]));
    }
    if mission_types_present_in_write_target(project_root) {
        return Ok(MigrationOutcome::unchanged(vec![
            ALREADY_PRESENT.to_string(),
        ]));
    }
    let Some(text) = shared::read_text(&config_file) else {
        return Err(InstallError::UpgradeRefused {
            detail: "Invalid YAML: config.yaml is not readable".to_string(),
        });
    };
    let document = crate::yaml_edit::CstDocument::parse(&config_file, &text)?;
    if document
        .root()
        .get_string_list(MISSION_TYPES_KEY)
        .is_some_and(|list| !list.is_empty())
    {
        return Ok(MigrationOutcome::unchanged(vec![
            ALREADY_PRESENT.to_string(),
        ]));
    }
    let builtin = builtin_mission_type_ids();
    document.root().set_string_list(MISSION_TYPES_KEY, &builtin);
    crate::migrations::completion_legacy::paths::write_text(&config_file, &document.text())?;
    Ok(MigrationOutcome::changed(vec![format!(
        "Added mission_type_activations: {builtin:?}"
    )]))
}

// ---------------------------------------------------------------------------
// 4.0.0rc8_command_skill_hyphen_names
// ---------------------------------------------------------------------------

const DOCTOR_SKILLS: &str = "spec-kitty doctor skills";

pub fn detect_command_skill_hyphen_names(project_root: &Path) -> Result<bool, InstallError> {
    if !project_root.join(".kittify").is_dir() {
        return Ok(false);
    }
    let manifest_has_dotted = crate::manifest::AssetManifest::load(project_root)
        .map(|manifest| {
            manifest.is_some_and(|manifest| {
                manifest
                    .entries()
                    .iter()
                    .any(|entry| is_dotted_command_skill_name(&entry.path))
            })
        })
        .unwrap_or(true);
    Ok(manifest_has_dotted || !dotted_command_folders_on_disk(project_root).is_empty())
}

pub fn apply_command_skill_hyphen_names(
    project_root: &Path,
) -> Result<MigrationOutcome, InstallError> {
    let agents = configured_command_skill_agents(project_root);
    let mut warnings = Vec::new();
    let mut changes = Vec::new();
    if agents.is_empty() {
        let manifest = crate::manifest::AssetManifest::load(project_root)
            .ok()
            .flatten();
        if let Some(mut manifest) = manifest {
            let legacy: Vec<String> = manifest
                .entries()
                .iter()
                .filter(|entry| is_dotted_command_skill_name(&entry.path))
                .map(|entry| entry.path.clone())
                .collect();
            for path in legacy {
                if !project_root.join(&path).exists() {
                    manifest.remove_path(&path);
                    changes.push(format!(
                        "Dropped manifest entry of absent legacy command skill {path}"
                    ));
                } else {
                    warnings.push(format!(
                        "Legacy dotted command skill {path} is still recorded but no command-skill agent is configured; it was left in place — run `{DOCTOR_SKILLS}`"
                    ));
                }
            }
            sk_kernel::atomic_write(
                &project_root.join(crate::manifest::MANIFEST_REL),
                &manifest.to_bytes(),
            )
            .map_err(|error| {
                InstallError::io("write manifest", project_root, std::io::Error::other(error))
            })?;
        }
    } else {
        match install_command_skills(project_root, &agents, false) {
            Ok(report) => {
                describe_install_report(&report, &mut changes);
                warnings.extend(report.warnings.clone());
                for path in &report.preserved {
                    warnings.push(format!(
                        "Preserved {path}: command-skill install kept an unowned or edited path — run `{DOCTOR_SKILLS}`"
                    ));
                }
            }
            Err(error) => warnings.push(format!(
                "Command-skill install for {} was refused ({error}); legacy dotted command skills were left in place — run `{DOCTOR_SKILLS}`",
                agents.join(", ")
            )),
        }
    }
    for leftover in dotted_command_folders_on_disk(project_root) {
        warnings.push(format!(
            "Legacy dotted command skill {leftover} is still on disk — run `{DOCTOR_SKILLS}`"
        ));
    }
    warnings.dedup();
    Ok(MigrationOutcome {
        changed: !changes.is_empty(),
        changes,
        warnings,
        ..crate::upgrade::MigrationOutcome::default()
    })
}

// ---------------------------------------------------------------------------
// 4.0.0rc8_retire_antigravity_workflows
// ---------------------------------------------------------------------------

const ANTIGRAVITY: &str = "antigravity";
const WORKFLOWS_REL: &str = ".agent/workflows";
const ARCHIVE_REL: &str = ".kittify/backups/antigravity-workflows";
const MARKER_HEAD_LINES: usize = 20;

const HISTORICAL_COMMANDS: [&str; 20] = [
    "accept",
    "analyze",
    "charter",
    "consolidate",
    "implement",
    "plan",
    "research",
    "review",
    "specify",
    "status",
    "tasks",
    "tasks-finalize",
    "tasks-outline",
    "tasks-packages",
    "checklist",
    "clarify",
    "constitution",
    "dashboard",
    "merge",
    "profile-context",
];

fn historical_command_of(name: &str) -> Option<&str> {
    let command = name.strip_prefix("spec-kitty.")?.strip_suffix(".md")?;
    HISTORICAL_COMMANDS.contains(&command).then_some(command)
}

fn has_exact_marker_line(path: &Path) -> bool {
    let Some(text) = shared::read_text(path) else {
        return false;
    };
    text.lines().take(MARKER_HEAD_LINES).any(|line| {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        trimmed.starts_with("<!-- spec-kitty-command-version: ")
            && trimmed.ends_with(" -->")
            && trimmed.matches(' ').count() >= 2
            && !trimmed[4..trimmed.len() - 4].contains("-->")
    })
}

fn is_spec_kitty_workflow(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_file())
        .unwrap_or(false)
        && historical_command_of(&path.file_name().unwrap_or_default().to_string_lossy()).is_some()
        && has_exact_marker_line(path)
}

fn antigravity_refusal(project_root: &Path) -> Option<String> {
    for rel in [
        ".agent",
        WORKFLOWS_REL,
        ".kittify",
        ".kittify/backups",
        ARCHIVE_REL,
    ] {
        let path = project_root.join(rel);
        if path.is_symlink() {
            return Some(format!(
                "Antigravity workflow cleanup skipped: {rel} is a symlink; nothing was changed"
            ));
        }
        if std::fs::symlink_metadata(&path).is_ok() && !path.is_dir() {
            return Some(format!(
                "Antigravity workflow cleanup skipped: {rel} is not a directory; nothing was changed"
            ));
        }
    }
    None
}

/// `(process, configured, warning)`.
fn antigravity_gate(project_root: &Path, workflows_present: bool) -> (bool, bool, Option<String>) {
    let agents = match configured_agents(project_root) {
        Ok(agents) => agents,
        Err(error) => {
            return (
                false,
                false,
                Some(format!(
                    "Agent config could not be read ({error}); Antigravity workflow files were left in place"
                )),
            );
        }
    };
    if agents.iter().any(|agent| agent == ANTIGRAVITY) {
        return (workflows_present, true, None);
    }
    let legacy = !project_root.join(".kittify/config.yaml").exists() || agents.is_empty();
    (legacy && workflows_present, false, None)
}

fn antigravity_candidates(project_root: &Path) -> Vec<PathBuf> {
    let workflows = project_root.join(WORKFLOWS_REL);
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(&workflows)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .is_some_and(|name| name.to_string_lossy().starts_with("spec-kitty."))
                })
                .collect()
        })
        .unwrap_or_default();
    candidates.sort();
    candidates
}

pub fn detect_retire_antigravity_workflows(project_root: &Path) -> Result<bool, InstallError> {
    let workflows_present = std::fs::symlink_metadata(project_root.join(WORKFLOWS_REL)).is_ok();
    let (process, _configured, warning) = antigravity_gate(project_root, workflows_present);
    if warning.is_some() {
        return Ok(true);
    }
    if !process {
        return Ok(false);
    }
    Ok(antigravity_refusal(project_root).is_some()
        || !antigravity_candidates(project_root).is_empty())
}

pub fn apply_retire_antigravity_workflows(
    project_root: &Path,
) -> Result<MigrationOutcome, InstallError> {
    let workflows_present = std::fs::symlink_metadata(project_root.join(WORKFLOWS_REL)).is_ok();
    let (process, configured, warning) = antigravity_gate(project_root, workflows_present);
    let mut changes = Vec::new();
    let mut warnings: Vec<String> = warning.into_iter().collect();
    if process {
        if let Some(refusal) = antigravity_refusal(project_root) {
            warnings.push(refusal);
        } else {
            for path in antigravity_candidates(project_root) {
                let rel = format!(
                    "{WORKFLOWS_REL}/{}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                );
                if !is_spec_kitty_workflow(&path) {
                    warnings.push(format!(
                        "Kept {rel}: not provably generated by Spec Kitty (needs a known command name, a regular file and the exact version-marker line); Antigravity no longer reads .agent/workflows — review and remove it yourself"
                    ));
                    continue;
                }
                let archive_parent = project_root.join(ARCHIVE_REL);
                let verdict = guard_destructive_removal(
                    &path,
                    project_root,
                    // Ownership was proven above; the guard's proven branch
                    // deletes, and the ruling is archive-never-delete, so route
                    // it through the verbatim archive branch.
                    &Prover::ManagedPath(vec!["__antigravity_archive__".to_string()]),
                    false,
                    Some(&archive_parent),
                    false,
                )?;
                match verdict {
                    crate::preserve::RemovalVerdict::Archived { path: archived, .. } => {
                        changes.push(format!(
                            "Archived {rel} to {}",
                            shared::relative_display(project_root, &archived)
                        ));
                    }
                    other => warnings.push(format!(
                        "Antigravity workflow {rel} was not archived: {other:?}"
                    )),
                }
            }
        }
    }
    if configured && process {
        match install_command_skills(project_root, &[ANTIGRAVITY.to_string()], false) {
            Ok(report) => {
                for path in &report.written {
                    changes.push(format!("Installed command skill {path}"));
                }
                warnings.extend(report.warnings.clone());
            }
            Err(error) => warnings.push(format!(
                "Command-skill install for {ANTIGRAVITY} was refused ({error}) — run `{DOCTOR_SKILLS}`"
            )),
        }
    }
    Ok(MigrationOutcome {
        changed: !changes.is_empty(),
        changes,
        warnings,
        ..crate::upgrade::MigrationOutcome::default()
    })
}

// ---------------------------------------------------------------------------
// 3.2.0rc35_kittify_profile_handoff
// ---------------------------------------------------------------------------

const HANDOFF_SENTINEL: &str = "Prepare for Review Hand-off";
const PROFILE_LOAD_SENTINEL: &str = "Load Agent Profile";
const DO_THIS_FIRST_SENTINEL: &str = "⚡ Do This First";

const IMPLEMENT_HANDOFF_BLOCK: &str = r#"

### 6. Prepare for Review Hand-off

Before moving this WP to `for_review`, update the `agent_profile` field in the WP
prompt frontmatter to a reviewer profile so the reviewing agent loads the correct
persona automatically.

1. Identify the appropriate reviewer profile:
   ```bash
   spec-kitty agent profile list --json | grep reviewer
   ```
   The default reviewer profile is `reviewer-renata`. Use it unless the mission or
   charter specifies a different reviewer.

2. Update the WP frontmatter:
   ```yaml
   agent_profile: "reviewer-renata"
   role: "reviewer"
   ```

3. Commit the updated frontmatter together with your implementation changes **before**
   running `spec-kitty agent tasks move-task WPxx --to for_review`.

The reviewer will then use `/spk-charter-profile-load` with the reviewer profile and apply
its self-review gates automatically.
"#;

const REVIEW_PROFILE_LOAD_BLOCK: &str = r#"

### 2a. Load Agent Profile

Before proceeding with the review, load the agent profile from the WP frontmatter
using the `/spk-charter-profile-load` skill (or `spec-kitty agent profile list` to browse
available profiles). Apply the profile's reviewer guidance and self-review gates for
the rest of this review session.

The WP frontmatter should already have `agent_profile` set to a reviewer profile
(e.g., `reviewer-renata`) by the implementing agent before it moved the WP to
`for_review`. If `agent_profile` is still set to an implementer profile, load the
implementer profile anyway and note the oversight in your review comments.
"#;

const TASK_PROMPT_DO_THIS_FIRST_BLOCK: &str = r#"

## ⚡ Do This First: Load Agent Profile

Use the `/spk-charter-profile-load` skill to load the agent profile specified in the
frontmatter, and behave according to its guidance before parsing the rest of this
prompt.

If no profile is specified, run `spec-kitty agent profile list` and select the best
match for this work package's `task_type` and `authoritative_surface`.

---
"#;

fn patch_implement(content: &str) -> (String, Vec<String>) {
    let mut changes = Vec::new();
    let mut content = content.to_string();
    for (from, to, message) in [
        (
            "- `profile`",
            "- `agent_profile`",
            "fixed stale field: `profile` → `agent_profile`",
        ),
        (
            "- `tool`",
            "- `agent`",
            "fixed stale field: `tool` → `agent`",
        ),
        (
            "If `profile` is empty",
            "If `agent_profile` is empty",
            "fixed stale Section-2a fallback sentence",
        ),
    ] {
        if content.contains(from) {
            content = content.replace(from, to);
            changes.push(message.to_string());
        }
    }
    if !content.contains(HANDOFF_SENTINEL) {
        content = if content.contains("## Output") {
            content.replacen(
                "## Output",
                &format!("{IMPLEMENT_HANDOFF_BLOCK}## Output"),
                1,
            )
        } else {
            format!("{content}{IMPLEMENT_HANDOFF_BLOCK}")
        };
        changes.push("inserted Section 6 'Prepare for Review Hand-off'".to_string());
    }
    (content, changes)
}

fn patch_review(content: &str) -> (String, Vec<String>) {
    let mut changes = Vec::new();
    let mut content = content.to_string();
    if !content.contains(PROFILE_LOAD_SENTINEL) {
        content = if content.contains("### 3.") {
            content.replacen("### 3.", &format!("{REVIEW_PROFILE_LOAD_BLOCK}### 3."), 1)
        } else {
            format!("{content}{REVIEW_PROFILE_LOAD_BLOCK}")
        };
        changes.push("inserted Section 2a 'Load Agent Profile'".to_string());
    }
    (content, changes)
}

fn patch_task_prompt_template(content: &str) -> (String, Vec<String>) {
    let mut changes = Vec::new();
    let mut content = content.to_string();
    if content.starts_with("---")
        && let Some(end) = content[3..].find("---")
    {
        let end = end + 3;
        let frontmatter = &content[3..end];
        let mut new_frontmatter = frontmatter.to_string();
        if !frontmatter.contains("agent_profile:") {
            new_frontmatter = format!("{}\nagent_profile: \"\"\n", new_frontmatter.trim_end());
            changes.push("added agent_profile field to frontmatter".to_string());
        }
        if !frontmatter.contains("role:") {
            new_frontmatter = format!("{}\nrole: \"\"\n", new_frontmatter.trim_end());
            changes.push("added role field to frontmatter".to_string());
        }
        if new_frontmatter != frontmatter {
            content = format!("---{new_frontmatter}---{}", &content[end + 3..]);
        }
    }
    if !content.contains(DO_THIS_FIRST_SENTINEL) {
        let lines: Vec<&str> = content.split('\n').collect();
        let mut insert_at = None;
        for (index, line) in lines.iter().enumerate() {
            if line.starts_with("# ") && index > 0 {
                insert_at = Some(index + 1);
                break;
            }
        }
        content = match insert_at {
            Some(index) => {
                let mut owned: Vec<String> = lines.iter().map(|line| (*line).to_string()).collect();
                owned.insert(index, TASK_PROMPT_DO_THIS_FIRST_BLOCK.to_string());
                owned.join("\n")
            }
            None => format!("{content}{TASK_PROMPT_DO_THIS_FIRST_BLOCK}"),
        };
        changes.push("inserted '⚡ Do This First: Load Agent Profile' section".to_string());
    }
    (content, changes)
}

fn patch_file(
    path: &Path,
    patch: fn(&str) -> (String, Vec<String>),
    read_only: bool,
    changes: &mut Vec<String>,
) -> Result<(), InstallError> {
    if !path.exists() {
        return Ok(());
    }
    let Some(content) = shared::read_text(path) else {
        return Ok(());
    };
    let (new_content, applied) = patch(&content);
    if applied.is_empty() {
        return Ok(());
    }
    support::write_generated_file(path, &new_content, read_only)?;
    changes.extend(applied.into_iter().map(|change| {
        format!(
            "{}: {change}",
            path.strip_prefix("/").unwrap_or(path).display()
        )
    }));
    Ok(())
}

pub fn detect_kittify_profile_handoff(project_root: &Path) -> Result<bool, InstallError> {
    let implement =
        project_root.join(".kittify/missions/software-dev/command-templates/implement.md");
    if implement.exists()
        && shared::read_text(&implement).is_some_and(|text| !text.contains(HANDOFF_SENTINEL))
    {
        return Ok(true);
    }
    for skill in ["spec-kitty.implement", "spec-kitty.review"] {
        let path = project_root
            .join(".agents/skills")
            .join(skill)
            .join("SKILL.md");
        if path.exists()
            && shared::read_text(&path).is_some_and(|text| !text.contains(HANDOFF_SENTINEL))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn apply_kittify_profile_handoff(
    project_root: &Path,
) -> Result<MigrationOutcome, InstallError> {
    let mut changes = Vec::new();
    patch_file(
        &project_root.join(".kittify/missions/software-dev/command-templates/implement.md"),
        patch_implement,
        false,
        &mut changes,
    )?;
    patch_file(
        &project_root.join(".kittify/missions/software-dev/command-templates/review.md"),
        patch_review,
        false,
        &mut changes,
    )?;
    patch_file(
        &project_root
            .join(".kittify/overrides/missions/software-dev/templates/task-prompt-template.md"),
        patch_task_prompt_template,
        false,
        &mut changes,
    )?;
    for (skill, patch, read_only) in [
        (
            "spec-kitty.implement",
            patch_implement as fn(&str) -> (String, Vec<String>),
            true,
        ),
        (
            "spec-kitty.review",
            patch_review as fn(&str) -> (String, Vec<String>),
            true,
        ),
    ] {
        patch_file(
            &project_root
                .join(".agents/skills")
                .join(skill)
                .join("SKILL.md"),
            patch,
            read_only,
            &mut changes,
        )?;
    }
    Ok(MigrationOutcome {
        changed: !changes.is_empty(),
        changes,
        warnings: Vec::new(),
        ..crate::upgrade::MigrationOutcome::default()
    })
}

/// The agents roster helper, kept here so the module owns its own surface list.
pub fn antigravity_configured(project_root: &Path) -> bool {
    agents().iter().any(|surface| {
        surface.key == ANTIGRAVITY
            && configured_agents(project_root).is_ok_and(|a| a.iter().any(|k| k == surface.key))
    })
}

/// The command/skill installer effects implemented by this module.
pub const IMPLEMENTATIONS: [crate::upgrade::NativeMigration; 12] = [
    crate::upgrade::NativeMigration {
        id: "3.2.0rc35_spk_skill_pack",
        detect: detect_spk_skill_pack,
        can_apply: can_apply_spk_skill_pack,
        apply: apply_spk_skill_pack,
    },
    crate::upgrade::NativeMigration {
        id: "3.2.0rc30_fix_runtime_next_result_default",
        detect: detect_runtime_next_result_default,
        can_apply: crate::upgrade::always_applicable,
        apply: apply_runtime_next_result_default,
    },
    crate::upgrade::NativeMigration {
        id: "3.2.0rc35_fix_prompt_file_workaround",
        detect: detect_prompt_file_workaround,
        can_apply: crate::upgrade::always_applicable,
        apply: apply_prompt_file_workaround,
    },
    crate::upgrade::NativeMigration {
        id: "3.1.2_globalize_commands",
        detect: detect_globalize_commands,
        can_apply: crate::upgrade::always_applicable,
        apply: apply_globalize_commands,
    },
    crate::upgrade::NativeMigration {
        id: "3.2.0a4_safe_globalize_commands",
        detect: detect_globalize_commands,
        can_apply: crate::upgrade::always_applicable,
        apply: apply_globalize_commands,
    },
    crate::upgrade::NativeMigration {
        id: "3.2.0rc35_codex_to_skills",
        detect: detect_codex_to_skills,
        can_apply: can_apply_codex_to_skills,
        apply: apply_codex_to_skills,
    },
    crate::upgrade::NativeMigration {
        id: "3.2.0rc35_pi_letta_agent_backfill",
        detect: detect_pi_letta_backfill,
        can_apply: can_apply_pi_letta_backfill,
        apply: apply_pi_letta_backfill,
    },
    crate::upgrade::NativeMigration {
        id: "3.2.0rc35_update_planning_templates",
        detect: detect_update_planning_templates,
        can_apply: can_apply_update_planning_templates,
        apply: apply_update_planning_templates,
    },
    crate::upgrade::NativeMigration {
        id: "3.2.0rc35_activate_builtin_mission_types",
        detect: detect_activate_builtin_mission_types,
        can_apply: can_apply_activate_builtin_mission_types,
        apply: apply_activate_builtin_mission_types,
    },
    crate::upgrade::NativeMigration {
        id: "4.0.0rc8_command_skill_hyphen_names",
        detect: detect_command_skill_hyphen_names,
        can_apply: crate::upgrade::always_applicable,
        apply: apply_command_skill_hyphen_names,
    },
    crate::upgrade::NativeMigration {
        id: "4.0.0rc8_retire_antigravity_workflows",
        detect: detect_retire_antigravity_workflows,
        can_apply: crate::upgrade::always_applicable,
        apply: apply_retire_antigravity_workflows,
    },
    crate::upgrade::NativeMigration {
        id: "3.2.0rc35_kittify_profile_handoff",
        detect: detect_kittify_profile_handoff,
        can_apply: crate::upgrade::always_applicable,
        apply: apply_kittify_profile_handoff,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globalize_with_an_injected_home_keeps_unproven_files() {
        let project = tempfile::tempdir().expect("project");
        let home = tempfile::tempdir().expect("home");
        let root = project.path();
        support::write_generated_file(
            &root.join(".kittify/config.yaml"),
            "agents:\n  available:\n    - claude\n",
            false,
        )
        .expect("config");
        let generated = root.join(".claude/commands/spec-kitty.specify.md");
        support::write_generated_file(
            &generated,
            "<!-- spec-kitty-command-version: 4.0.0rc8 -->\nold\n",
            false,
        )
        .expect("generated");
        let custom = root.join(".claude/commands/spec-kitty.custom.md");
        support::write_generated_file(&custom, "custom\n", false).expect("custom");

        // Runtime present, no exact global replacement: the generated file is kept.
        std::fs::create_dir_all(home.path().join("missions/software-dev")).expect("runtime");
        let first = apply_globalize_commands_at(root, home.path(), true).expect("first");
        assert!(generated.exists(), "{:?}", first.changes);
        assert!(
            first
                .changes
                .iter()
                .any(|line| line.contains("no global equivalent")),
            "{:?}",
            first.changes
        );

        std::fs::create_dir_all(home.path().join(".claude/commands")).expect("dir");
        support::write_generated_file(
            &home.path().join(".claude/commands/spec-kitty.specify.md"),
            "global\n",
            false,
        )
        .expect("global");
        let second = apply_globalize_commands_at(root, home.path(), true).expect("second");
        assert!(!generated.exists());
        assert_eq!(
            second.changes,
            vec![
                "Preserved: .claude/commands/spec-kitty.custom.md (no version marker (user-authored or unproven file))",
                "Removed: .claude/commands/spec-kitty.specify.md",
            ]
        );
        assert!(custom.exists());
        assert!(root.join(".claude/commands").is_dir());
    }
}
