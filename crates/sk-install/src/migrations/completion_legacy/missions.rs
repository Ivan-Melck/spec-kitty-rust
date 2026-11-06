//! Mission-asset migrations: bundled missions and their templates.
//!
//! | id | effect |
//! | --- | --- |
//! | `0.6.7_ensure_missions` | copy `software-dev`/`research` from the package into `.kittify/missions/` |
//! | `0.9.2_research_mission_templates` | add the research task-prompt template (and the legacy tasks command when the package ships one) |
//! | `0.12.0_documentation_mission` | install the documentation mission |
//!
//! Every copy is source-exact: `shutil.copytree` semantics (destination must be
//! absent, plain file modes, symlinks dereferenced). A mission directory the
//! project already holds is never overwritten; an *incomplete* required mission
//! is archived verbatim under `.kittify/` before it is rebuilt, because mission
//! content carries no ownership marker and may co-locate user files.

use std::path::{Path, PathBuf};

use crate::error::InstallError;
use crate::migrations::completion_legacy::assets;
use crate::migrations::completion_legacy::paths::{self, KITTIFY, KITTY_SPECS};
use crate::preserve::archive_into;
use crate::upgrade::MigrationOutcome;

/// The missions the 0.6.7 repair requires (`REQUIRED_MISSIONS`).
pub const REQUIRED_MISSIONS: [&str; 2] = ["software-dev", "research"];

fn missions_dir(project_root: &Path) -> PathBuf {
    project_root.join(KITTIFY).join("missions")
}

fn outcome(changed: bool, changes: Vec<String>, warnings: Vec<String>) -> MigrationOutcome {
    MigrationOutcome {
        changed,
        changes,
        warnings,
        ..crate::upgrade::MigrationOutcome::default()
    }
}

// ------------------------------------------------------- 0.6.7 ensure missions

/// `0.6.7_ensure_missions.detect`.
pub fn detect_ensure_missions(project_root: &Path) -> Result<bool, InstallError> {
    if assets::uses_centralized_runtime(project_root) {
        return Ok(false);
    }
    let missions = missions_dir(project_root);
    if !missions.exists() {
        return Ok(true);
    }
    for name in REQUIRED_MISSIONS {
        let mission = missions.join(name);
        if !mission.exists() || !mission.join("mission.yaml").exists() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `0.6.7_ensure_missions.can_apply`: the package must be able to supply every
/// required mission. A development/test environment without the bundled tree is
/// the source's own "not applicable here" skip, never a fabricated copy.
pub fn can_apply_ensure_missions(project_root: &Path) -> Result<Option<String>, InstallError> {
    let Some(package) = assets::package_missions_root() else {
        return Ok(Some(
            "Could not locate package missions to copy from. Run 'spec-kitty init --force' to repair missions manually."
                .to_string(),
        ));
    };
    let missing: Vec<&str> = REQUIRED_MISSIONS
        .iter()
        .copied()
        .filter(|name| !package.join(name).exists())
        .collect();
    if !missing.is_empty() {
        return Ok(Some(format!(
            "Package is missing missions: {}. Please upgrade spec-kitty-cli to the latest version.",
            missing.join(", ")
        )));
    }
    let _ = project_root;
    Ok(None)
}

/// `0.6.7_ensure_missions.apply`.
pub fn apply_ensure_missions(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let Some(package) = assets::package_missions_root() else {
        return Err(InstallError::UpgradeRefused {
            detail: "Could not locate package missions".to_string(),
        });
    };
    let missions = missions_dir(project_root);
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    if !missions.exists() {
        std::fs::create_dir_all(&missions)
            .map_err(|error| InstallError::io("create directory", &missions, error))?;
        changes.push("Created .kittify/missions/ directory".to_string());
    }
    for name in REQUIRED_MISSIONS {
        let destination = missions.join(name);
        let source = package.join(name);
        if destination.exists() {
            if destination.join("mission.yaml").exists() {
                continue;
            }
            // An incomplete required mission may hold user files beside package
            // remnants, and mission content has no manifest/version marker: the
            // whole directory is archived verbatim before it is torn down.
            let archived = archive_into(&destination, project_root, &project_root.join(KITTIFY))?;
            warnings.push(format!(
                "Archived incomplete mission {name} to {}",
                paths::relative_display(project_root, &archived)
            ));
            paths::remove_tool_owned_tree(
                &destination,
                &missions,
                "incomplete bundled mission",
                false,
            )?;
            assets::copy_tree(&source, &destination)?;
            changes.push(format!("Repaired incomplete mission: {name}"));
            continue;
        }
        assets::copy_tree(&source, &destination)?;
        changes.push(format!("Copied missing mission: {name}"));
    }
    for worktree in paths::worktrees(project_root) {
        let wt_missions = worktree.join(KITTIFY).join("missions");
        if !wt_missions.exists() {
            continue;
        }
        for name in REQUIRED_MISSIONS {
            let destination = wt_missions.join(name);
            if destination.exists() {
                continue;
            }
            let worktree_name = worktree
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            match assets::copy_tree(&package.join(name), &destination) {
                Ok(_) => changes.push(format!(
                    "Copied missing mission to worktree {worktree_name}: {name}"
                )),
                Err(error) => warnings.push(format!(
                    "Could not copy mission to worktree {worktree_name}: {error}"
                )),
            }
        }
    }
    Ok(outcome(!changes.is_empty(), changes, warnings))
}

// --------------------------------------------- 0.9.2 research mission templates

/// Files every supported layout needs (`REQUIRED_FILES`).
const RESEARCH_REQUIRED: [(&str, &str); 1] = [("templates", "task-prompt-template.md")];
/// Files only older layouts need (`LEGACY_REQUIRED_FILES`).
const RESEARCH_LEGACY: [(&str, &str); 1] = [("command-templates", "tasks.md")];

fn research_required_files(package_research: Option<&Path>) -> Vec<(&'static str, &'static str)> {
    let mut required = RESEARCH_REQUIRED.to_vec();
    if let Some(package) = package_research {
        for (subdir, filename) in RESEARCH_LEGACY {
            if package.join(subdir).join(filename).exists() {
                required.push((subdir, filename));
            }
        }
    }
    required
}

fn research_mission(project_root: &Path) -> PathBuf {
    missions_dir(project_root).join("research")
}

/// `0.9.2_research_mission_templates.detect`.
pub fn detect_research_mission_templates(project_root: &Path) -> Result<bool, InstallError> {
    let research = research_mission(project_root);
    if !research.exists() {
        return Ok(false);
    }
    let package = assets::package_mission("research");
    Ok(research_required_files(package.as_deref())
        .into_iter()
        .any(|(subdir, filename)| !research.join(subdir).join(filename).exists()))
}

/// `0.9.2_research_mission_templates.can_apply`.
pub fn can_apply_research_mission_templates(
    project_root: &Path,
) -> Result<Option<String>, InstallError> {
    let _ = project_root;
    let Some(package) = assets::package_mission("research") else {
        return Ok(Some(
            "Could not locate package research mission to copy templates from. Run 'spec-kitty init --force' to repair missions manually."
                .to_string(),
        ));
    };
    let missing: Vec<String> = research_required_files(Some(&package))
        .into_iter()
        .filter(|(subdir, filename)| !package.join(subdir).join(filename).exists())
        .map(|(subdir, filename)| format!("{subdir}/{filename}"))
        .collect();
    if !missing.is_empty() {
        return Ok(Some(format!(
            "Package research mission is missing: {}. Please upgrade spec-kitty-cli to the latest version.",
            missing.join(", ")
        )));
    }
    Ok(None)
}

fn copy_missing_files(
    source: &Path,
    destination: &Path,
    label: &str,
    changes: &mut Vec<String>,
    warnings: &mut Vec<String>,
) -> Result<(), InstallError> {
    for (subdir, filename) in research_required_files(Some(source)) {
        let target = destination.join(subdir).join(filename);
        if target.exists() {
            continue;
        }
        let bytes = std::fs::read(source.join(subdir).join(filename))
            .map_err(|error| InstallError::io("read template", &target, error))?;
        match paths::write_bytes(&target, &bytes) {
            Ok(_) => changes.push(format!("Added {label}{subdir}/{filename}")),
            Err(error) => warnings.push(format!("Could not copy {subdir}/{filename}: {error}")),
        }
    }
    Ok(())
}

/// `0.9.2_research_mission_templates.apply`.
pub fn apply_research_mission_templates(
    project_root: &Path,
) -> Result<MigrationOutcome, InstallError> {
    let research = research_mission(project_root);
    let Some(package) = assets::package_mission("research") else {
        return Err(InstallError::UpgradeRefused {
            detail: "Could not locate package research mission".to_string(),
        });
    };
    if !research.exists() {
        return Ok(MigrationOutcome::unchanged(vec![
            "Research mission not present, skipping".to_string(),
        ]));
    }
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    copy_missing_files(
        &package,
        &research,
        "research/",
        &mut changes,
        &mut warnings,
    )?;
    for worktree in paths::worktrees(project_root) {
        let wt_research = worktree.join(KITTIFY).join("missions").join("research");
        if !wt_research.exists() {
            continue;
        }
        let mut worktree_changes = Vec::new();
        let mut worktree_warnings = Vec::new();
        copy_missing_files(
            &package,
            &wt_research,
            "",
            &mut worktree_changes,
            &mut worktree_warnings,
        )?;
        let name = worktree
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        changes.extend(
            worktree_changes
                .into_iter()
                .map(|line| format!("{line} (worktree {name})")),
        );
        warnings.extend(
            worktree_warnings
                .into_iter()
                .map(|line| format!("{line} (worktree {name})")),
        );
    }
    Ok(outcome(!changes.is_empty(), changes, warnings))
}

// -------------------------------------------------- 0.12.0 documentation mission

fn documentation_mission(project_root: &Path) -> PathBuf {
    missions_dir(project_root).join("documentation")
}

/// `0.12.0_documentation_mission.detect`.
pub fn detect_documentation_mission(project_root: &Path) -> Result<bool, InstallError> {
    if assets::uses_centralized_runtime(project_root) {
        return Ok(false);
    }
    if !project_root.join(KITTIFY).exists() {
        return Ok(false);
    }
    let missions = missions_dir(project_root);
    if !missions.exists() {
        return Ok(true);
    }
    let documentation = missions.join("documentation");
    Ok(!(documentation.exists() && documentation.join("mission.yaml").exists()))
}

/// `0.12.0_documentation_mission.can_apply`.
pub fn can_apply_documentation_mission(
    project_root: &Path,
) -> Result<Option<String>, InstallError> {
    let _ = project_root;
    let found = assets::package_mission("documentation")
        .filter(|mission| mission.join("mission.yaml").exists());
    if found.is_none() {
        return Ok(Some(
            "Could not locate documentation mission source in spec-kitty installation. This may indicate an incomplete installation."
                .to_string(),
        ));
    }
    Ok(None)
}

/// `0.12.0_documentation_mission.apply`.
pub fn apply_documentation_mission(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let Some(source) = assets::package_mission("documentation")
        .filter(|mission| mission.join("mission.yaml").exists())
    else {
        return Err(InstallError::UpgradeRefused {
            detail: "Could not find documentation mission source in spec-kitty installation"
                .to_string(),
        });
    };
    let missions = missions_dir(project_root);
    let destination = documentation_mission(project_root);
    if destination.exists() && destination.join("mission.yaml").exists() {
        return Ok(MigrationOutcome::unchanged(vec![
            "Documentation mission already installed (skipped)".to_string(),
        ]));
    }
    let mut changes = Vec::new();
    if !missions.exists() {
        std::fs::create_dir_all(&missions)
            .map_err(|error| InstallError::io("create directory", &missions, error))?;
        changes.push("Created .kittify/missions/ directory".to_string());
    }
    let files = assets::copy_tree(&source, &destination)?;
    changes.push(format!("Copied documentation mission ({files} files)"));
    Ok(outcome(true, changes, Vec::new()))
}

/// `.kittify/` existence is what makes a legacy layout a project at all; the
/// corpus check keeps a `kitty-specs/`-only tree out of the mission repairs.
pub fn is_spec_project(project_root: &Path) -> bool {
    project_root.join(KITTIFY).exists() || project_root.join(KITTY_SPECS).exists()
}
