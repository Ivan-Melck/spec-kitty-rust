//! `3.0.3_globalize_skill_pack` — the user-global canonical skill relink.
//!
//! Source: `src/specify_cli/upgrade/migrations/m_3_0_3_globalize_skill_pack.py`.
//!
//! The pinned effect is **not** a migration-only install path: it re-runs the
//! one canonical skill installer for every installable configured agent and
//! then reconciles `.kittify/skills-manifest.json`. Projects that already hold
//! the 3.0.3 layout have project skill files that are *links* into the
//! user-global canonical root (`~/.claude/skills/…`, `~/.agents/skills/…`); the
//! installer's job here is to recognise and retain those links
//! (`delivery_mode: symlink`), not to refuse them as collisions and not to
//! rewrite them into copies. Missing files are installed by the same installer
//! every other skill migration uses.
//!
//! Family ownership:
//!
//! * [`crate::skills`] — catalog, ownership manifest, symlink delivery mode,
//!   global-root verification.
//! * this module — the two typed effect bodies and the Source-shaped report.

use std::path::{Path, PathBuf};

use crate::error::InstallError;
use crate::migrations::completion_legacy::shared;
use crate::migrations::completion_modern::support;
use crate::skills::{
    DELIVERY_SYMLINK, ManagedSkillManifest, SkillCatalog, SkillInstallOptions,
    install_skills_for_agents,
};
use crate::upgrade::MigrationOutcome;

/// Source `migration_id`.
pub const MIGRATION_ID: &str = "3.0.3_globalize_skill_pack";

/// Source stamps this version into `skills-manifest.json`.
const GLOBALIZE_VERSION: &str = "3.0.3";

/// Migration ids this module implements (one).
pub const IMPLEMENTATIONS: [&str; 1] = [MIGRATION_ID];

fn offering_catalog() -> Option<SkillCatalog> {
    let root = shared::offering_skills_root()?;
    SkillCatalog::discover(&root).ok()
}

/// Resolve the project's skill catalog (built-in plus pack skills) through the
/// one seam. `detect` passes `stage = false` (read-only); only `apply` stages.
fn project_catalog(project_root: &Path, stage: bool) -> Result<SkillCatalog, InstallError> {
    match crate::skills::resolve_project_skill_catalog(project_root, stage) {
        Ok(catalog) => Ok(catalog),
        // A project with no shipped skills and no pack sources resolves to the
        // empty shipped catalog, exactly the state the pure offering read had.
        Err(InstallError::SkillsCatalogMissing { .. }) => {
            Ok(offering_catalog().unwrap_or_else(|| SkillCatalog::empty(PathBuf::new())))
        }
        Err(error) => Err(error),
    }
}

/// Every configured agent with an installable skill root (Source
/// `_installable_agents`).
fn installable_agents(project_root: &Path) -> Vec<String> {
    let configured = support::load_agent_config(project_root)
        .map(|config| config.available)
        .unwrap_or_default();
    configured
        .iter()
        .filter(|key| {
            crate::agent::agent_surface(key).is_ok_and(|surface| {
                surface.skill.primary_root().is_some()
                    && surface.skill.class != crate::agent::SkillClass::WrapperOnly
            })
        })
        .cloned()
        .collect()
}

fn builtin_catalog_names(catalog: &SkillCatalog) -> Vec<String> {
    catalog.names()
}

/// Source `detect`.
pub fn detect(project_root: &Path) -> Result<bool, InstallError> {
    if !project_root.join(".kittify").is_dir() {
        return Ok(false);
    }
    let catalog = match project_catalog(project_root, false) {
        Ok(catalog) => catalog,
        // Fail closed: `apply` reports the broken pack as a migration error.
        Err(InstallError::PackSkillCatalog { .. }) => return Ok(true),
        Err(error) => return Err(error),
    };
    if catalog.skills().is_empty() {
        return Ok(false);
    }
    let agents = installable_agents(project_root);
    if agents.is_empty() {
        return Ok(false);
    }

    if let Some(manifest) = ManagedSkillManifest::load(project_root) {
        for entry in &manifest.entries {
            if entry.skill_name.starts_with("spec-kitty-") && !entry.is_symlink_delivery() {
                return Ok(true);
            }
        }
    }

    for key in &agents {
        let Ok(surface) = crate::agent::agent_surface(key) else {
            continue;
        };
        let Some(root) = surface.skill.primary_root() else {
            continue;
        };
        for skill in catalog.skills() {
            if skill.is_pack() {
                // Pack skills are copies by design, never user-global links.
                continue;
            }
            let skill_file = project_root.join(root).join(&skill.name).join("SKILL.md");
            if !skill_file.exists() {
                return Ok(true);
            }
            if !skill_file.is_symlink() {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Source `can_apply`.
pub fn can_apply(project_root: &Path) -> Result<Option<String>, InstallError> {
    if project_root.join(".kittify").is_dir() {
        Ok(None)
    } else {
        Ok(Some(
            ".kittify/ directory does not exist (not initialized)".to_string(),
        ))
    }
}

/// Source `apply`.
pub fn apply(project_root: &Path, dry_run: bool) -> Result<MigrationOutcome, InstallError> {
    let catalog = project_catalog(project_root, !dry_run)?;
    if catalog.skills().is_empty() {
        return Err(InstallError::UpgradeRefused {
            detail: "No canonical skills discovered for relinking".to_string(),
        });
    }
    let agents = installable_agents(project_root);
    if agents.is_empty() {
        return Ok(MigrationOutcome {
            changed: false,
            changes: Vec::new(),
            warnings: vec!["No skill-installing agents configured; skipping relink".to_string()],
            ..crate::upgrade::MigrationOutcome::default()
        });
    }
    let skills = builtin_catalog_names(&catalog);

    if dry_run {
        return Ok(MigrationOutcome::changed(vec![format!(
            "Would relink {} canonical skill(s) for {} agent(s)",
            skills.len(),
            agents.len()
        )]));
    }

    let options = SkillInstallOptions {
        spec_kitty_version: Some(GLOBALIZE_VERSION.to_string()),
        ..SkillInstallOptions::default()
    };
    let report = install_skills_for_agents(project_root, &agents, &catalog, &options)?;

    let manifest_entries = ManagedSkillManifest::load(project_root)
        .map(|manifest| manifest.entries.len())
        .unwrap_or(0);
    if manifest_entries == 0 && report.preserved.is_empty() {
        return Err(InstallError::UpgradeRefused {
            detail: "No managed skill files were linked for any configured agent".to_string(),
        });
    }

    let mut outcome = MigrationOutcome {
        changed: true,
        changes: Vec::new(),
        warnings: report.warnings.clone(),
        ..crate::upgrade::MigrationOutcome::default()
    };
    outcome.changes.push(format!(
        "Relinked {} canonical skill(s) for {} agent(s) ({} managed files)",
        skills.len(),
        agents.len(),
        manifest_entries
    ));
    outcome
        .changes
        .push("Updated .kittify/skills-manifest.json for global canonical skill links".to_string());
    for path in &report.preserved {
        outcome.changes.push(format!(
            "Preserved unverified skill link for manual review: {path}"
        ));
    }
    Ok(outcome)
}

/// The Source constant, re-exported for callers that assert the delivery mode
/// recorded by this migration.
pub const RELINK_DELIVERY_MODE: &str = DELIVERY_SYMLINK;
