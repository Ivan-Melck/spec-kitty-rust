//! The shared-install seam every migration family may call.
//!
//! The pre-3.x family owns the installer-side primitives: package/asset root
//! resolution, the configured agent-surface roster, generated-file writing and
//! the ownership guards. A later (`target_version` major >= 3) migration needs
//! the *same* primitives — a command-skill rename must retire the old
//! generated file through the same proof a 2.x migration used, and a template
//! refresh must read the same shipped asset tree — so this module is the one
//! documented façade over them. It is deliberately thin: re-exports plus the
//! handful of "retire one named surface" operations that were previously
//! spelled out inline in a single migration.
//!
//! Two rules hold for every caller:
//!
//! - **Ownership is proven, never assumed.** A retired name is not proof: a
//!   file is removed only through [`crate::preserve::guard_destructive_removal`]
//!   with a marker, manifest or exact-byte proof, and an unprovable file is
//!   preserved in place with a diagnostic.
//! - **Assets come from the pinned source.** [`asset_root`] and
//!   [`canonical_skill_files`] read the shipped checkout/pack tree; a missing
//!   asset reports *absent* so the caller's `can_apply` skips, and nothing is
//!   invented from the running binary.

use std::path::{Path, PathBuf};

use crate::error::InstallError;
use crate::migrations::retirements::AgentDirectory;
use crate::preserve::{Prover, RemovalVerdict, guard_destructive_removal};
use crate::upgrade::MigrationOutcome;

pub use super::agent_commands::{
    PROMPT_DRIVEN_COMMANDS, SKILL_ROOTS, agent_key_for_directory, command_directories,
    command_surface, find_skill_files, is_thin_shim, output_filename, render_prompt,
    render_raw_template, runtime_command_templates, shim_candidates, template_path,
};
pub use super::assets::{
    TrackedMeta, canonical_skill_files, checkout_root, copy_tree, global_runtime_configured,
    kittify_home, offering_skills_root, pack_missions_root, package_asset_root, package_cli_root,
    package_mission, package_missions_root, read_tracked_meta, uses_centralized_runtime,
};
pub use super::layout::{
    AssetDisposition, ConflictReason, MigrationConflict, MigrationReport, classify_asset,
    execute_migration,
};
pub use super::paths::{
    KITTIFY, KITTY_SPECS, LANE_DIRS, WORKTREES, ensure_lane_in_frontmatter,
    normalized_lane_document, read_text, relative_display, remove_if_empty, remove_tool_owned_tree,
    should_ignore_file, split_frontmatter, worktrees, write_bytes, write_text,
};

/// The resolved package asset root (`get_package_asset_root()`), fail-closed.
///
/// This is the one place a migration asks "what did the release ship?". The
/// answer is the immutable built-in `missions` tree — never the mutable
/// `~/.kittify/` home, which a previous upgrade may already have rewritten.
pub fn asset_root() -> Option<PathBuf> {
    package_asset_root()
}

/// Write one generated file through the crate's canonical writer.
///
/// The write is atomic and refuses a symlink at the destination, so a managed
/// tree never grows a link that a later migration would follow.
pub fn write_generated(
    project_root: &Path,
    relative: &str,
    bytes: &[u8],
) -> Result<bool, InstallError> {
    write_bytes(&project_root.join(relative), bytes)
}

/// Retire one generated command file across every agent command surface.
///
/// `filename` is the project-relative file name inside each command directory
/// (for example `spec-kitty.profile-context.md`). Only a file that a marker,
/// a managed manifest entry or exact canonical bytes prove package-generated is
/// removed; a user-authored file that reuses the name is preserved and named in
/// a warning. Each directory is reported once, in configured order.
pub fn retire_command_file(
    project_root: &Path,
    filename: &str,
) -> Result<MigrationOutcome, InstallError> {
    let mut directories: Vec<AgentDirectory> = command_directories(project_root);
    for known in crate::migrations::retirements::known_agent_directories() {
        if !directories
            .iter()
            .any(|directory| directory.directory == known.directory)
        {
            directories.push(known);
        }
    }
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    let mut removed = 0usize;
    for directory in directories {
        let agent_dir = project_root.join(&directory.directory);
        if !agent_dir.exists() {
            continue;
        }
        let destination = agent_dir.join(filename);
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
                    relative_display(project_root, &destination)
                ));
            }
            RemovalVerdict::Absent => {}
            RemovalVerdict::Preserved { diagnostic, .. }
            | RemovalVerdict::Archived { diagnostic, .. } => warnings.push(diagnostic),
        }
    }
    if removed == 0 && warnings.is_empty() {
        changes.push(format!("{filename} absent from all configured agent dirs"));
    }
    Ok(MigrationOutcome {
        changed: removed > 0,
        changes,
        warnings,
        ..crate::upgrade::MigrationOutcome::default()
    })
}

/// Retire one installed skill directory from every skill root.
///
/// A provenance archive (`backup_parent`) keeps an unprovable tree instead of
/// deleting it, which is what a retired *standalone* skill surface needs: the
/// bytes stay recoverable, and the manifest is not rewritten here (the skill
/// installer owns that).
pub fn retire_skill_dir(
    project_root: &Path,
    skill: &str,
    backup_parent: Option<&Path>,
) -> Result<MigrationOutcome, InstallError> {
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    let mut removed = 0usize;
    let mut archived = 0usize;
    let mut roots: Vec<String> = SKILL_ROOTS.iter().map(|root| root.to_string()).collect();
    for root in crate::migrations::retirements::configured_skill_roots(project_root) {
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    for root in roots {
        let directory = project_root.join(&root).join(skill);
        if !directory.is_dir() {
            continue;
        }
        let verdict = guard_destructive_removal(
            &directory,
            project_root,
            &Prover::Manifest,
            true,
            backup_parent,
            false,
        )?;
        match verdict {
            RemovalVerdict::Owned { .. } => {
                removed += 1;
                changes.push(format!(
                    "Removed {}",
                    relative_display(project_root, &directory)
                ));
            }
            RemovalVerdict::Absent => {}
            RemovalVerdict::Preserved { diagnostic, .. } => warnings.push(diagnostic),
            RemovalVerdict::Archived { diagnostic, .. } => {
                // The bytes moved out of the live tree into the provenance
                // archive: the surface is retired, and the operator can still
                // recover it. That is a change, and it is reported as one.
                archived += 1;
                warnings.push(diagnostic);
            }
        }
        remove_if_empty(&project_root.join(&root));
    }
    if removed == 0 && archived == 0 && warnings.is_empty() {
        changes.push(format!("{skill} absent from all skill roots"));
    }
    Ok(MigrationOutcome {
        changed: removed > 0 || archived > 0,
        changes,
        warnings,
        ..crate::upgrade::MigrationOutcome::default()
    })
}

/// The canonical text of one skill file (`<skill>/SKILL.md`, …), when the
/// shipped offering tree provides it.
pub fn canonical_skill_text(skill: &str, relative: &str) -> Option<String> {
    canonical_skill_files(skill).and_then(|files| {
        files
            .into_iter()
            .find(|(path, _)| path == relative)
            .map(|(_, bytes)| String::from_utf8_lossy(&bytes).into_owned())
    })
}
