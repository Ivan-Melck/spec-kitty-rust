//! Step 7 of the charter-pack cutover: installed copies of removed skills.
//!
//! For each project skill root of each configured agent (deduplicated; a root
//! that does not exist is skipped, never created), every `<root>/<removed name>/`
//! is removed through [`guard_destructive_removal`] when its ownership is proven,
//! either
//!
//! * by the managed-skills manifest ([`Prover::Manifest`]: every file has an
//!   entry whose hash matches), or
//! * by [`Prover::TreeHashes`]: the tree holds exactly the files a shipped source
//!   held and each one hashes to [`SHIPPED_SKILL_HASHES`].
//!
//! Anything else (an edited copy, an extra file, a symlink) stays in place and is
//! reported once (`skills_kept`). A settled copy — removed or kept — leaves the
//! skills manifest: a removed one so no orphan entry remains, a kept one so it
//! becomes user-owned and a later upgrade never offers to overwrite it. A name
//! the installed CLI still ships is left alone (the finalizer would reinstall it).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::CutoverReport;
use super::skill_data::{REMOVED_SKILL_NAMES, SHIPPED_SKILL_HASHES};
use crate::preserve::{Prover, guard_destructive_removal, relative_display};
use crate::skills::ManagedSkillManifest;

/// The shipped per-file hashes of one removed skill, raw hex (the prover adds
/// the `sha256:` prefix itself).
fn shipped_hashes(name: &str) -> BTreeMap<String, String> {
    SHIPPED_SKILL_HASHES
        .iter()
        .find(|(skill, _)| *skill == name)
        .map(|(_, members)| {
            members
                .iter()
                .map(|(member, digest)| ((*member).to_string(), (*digest).to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// The names the installed CLI still ships, so removing them would only churn.
///
/// The shipped catalog is the checkout's `src/charter/offering/skills` when this
/// is a development tree; an absent catalog yields no names (every removed name
/// is live once its source is deleted, which is the reference's own state at the
/// pin).
fn still_shipped_names() -> Vec<String> {
    let mut names = Vec::new();
    for candidate in [
        PathBuf::from("src/charter/offering/skills"),
        PathBuf::from("packs/built-in/skills"),
    ] {
        if !candidate.is_dir() {
            continue;
        }
        if let Ok(read) = std::fs::read_dir(&candidate) {
            for entry in read.flatten() {
                if entry.path().join("SKILL.md").is_file() {
                    names.push(entry.file_name().to_string_lossy().to_string());
                }
            }
        }
    }
    names.sort();
    names.dedup();
    names
}

/// The skill roots of the CONFIGURED agents, in the project's own agent order.
///
/// The reference reads `AGENT_SKILL_CONFIG[agent]["skill_roots"]` while iterating
/// `load_agent_config(project).available`, so the removal order (and therefore the
/// report order) follows the project's agent list, not the registry's. The shared
/// `configured_skill_roots` helper iterates the registry instead, which is right
/// for its callers but produced a different report order here.
fn configured_roots_in_project_order(project: &Path) -> Vec<String> {
    let configured = match crate::init::read_configured_agents(project) {
        Ok(agents) if !agents.is_empty() => agents,
        _ => crate::agent::agents()
            .iter()
            .map(|surface| surface.key.to_string())
            .collect(),
    };
    let mut roots: Vec<String> = Vec::new();
    for agent in &configured {
        let Some(surface) = crate::agent::agents()
            .iter()
            .find(|surface| surface.key == agent)
        else {
            continue;
        };
        if surface.skill.class == crate::agent::SkillClass::WrapperOnly {
            continue;
        }
        for root in surface.skill.roots {
            let root = root.trim_end_matches('/').to_string();
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
    }
    roots
}

/// Every `<configured skill root>/<removed name>` present in `project` (nothing
/// is written).
pub fn find_removed_skill_copies(project: &Path) -> Vec<(PathBuf, String)> {
    let shipped = still_shipped_names();
    let names: Vec<&str> = REMOVED_SKILL_NAMES
        .iter()
        .copied()
        .filter(|name| !shipped.iter().any(|shipping| shipping == name))
        .collect();
    let mut copies = Vec::new();
    if names.is_empty() {
        return copies;
    }
    for root in configured_roots_in_project_order(project) {
        let base = project.join(&root);
        if !base.is_dir() {
            continue;
        }
        for name in &names {
            let candidate = base.join(name);
            if std::fs::symlink_metadata(&candidate).is_ok() {
                copies.push((candidate, (*name).to_string()));
            }
        }
    }
    copies
}

/// Drop the manifest entries of every settled directory (project-relative paths).
fn drop_manifest_entries(
    project: &Path,
    settled: &[String],
) -> Result<(), crate::error::InstallError> {
    let Some(mut manifest) = ManagedSkillManifest::load_strict(project)? else {
        return Ok(());
    };
    let prefixes: Vec<String> = settled.iter().map(|rel| format!("{rel}/")).collect();
    let before = manifest.entries.len();
    manifest.entries.retain(|entry| {
        !prefixes
            .iter()
            .any(|prefix| entry.installed_path.starts_with(prefix))
    });
    if manifest.entries.len() != before {
        manifest.save(project)?;
    }
    Ok(())
}

/// Remove every proven copy of a removed skill (unless `dry_run`); keep and
/// report the rest.
pub fn run(project: &Path, dry_run: bool) -> CutoverReport {
    let mut report = CutoverReport::default();
    let mut settled = Vec::new();
    for (path, name) in find_removed_skill_copies(project) {
        let rel = relative_display(&path, project);
        let prover = Prover::Any(vec![
            Prover::Manifest,
            Prover::TreeHashes(shipped_hashes(&name)),
        ]);
        // No archive parent: the reference keeps an unprovable copy IN PLACE
        // (it never archives a kept skill), so a kept copy must survive here.
        let verdict = guard_destructive_removal(&path, project, &prover, true, None, dry_run);
        match verdict {
            // The guard reports `Owned` for a proved path under a dry run too
            // (it removes nothing), and the reference reports the same line.
            Ok(crate::preserve::RemovalVerdict::Owned { proof }) => {
                report
                    .skills_removed
                    .push(format!("{rel} ({name}; package-owned ({}))", proof.kind));
                settled.push(rel);
            }
            Ok(_) => {
                report.skills_kept.push(format!(
                    "{rel} ({name}; edited or not a shipped copy, left in place and no longer managed by spec-kitty; remove it by hand once you no longer need it)"
                ));
                report.preserved_paths.push(rel.clone());
                settled.push(rel);
            }
            Err(error) => {
                report.errors.push(format!(
                    "{rel} could not be removed ({error}); close any program holding it, then run `spec-kitty upgrade` again"
                ));
            }
        }
    }
    if !settled.is_empty()
        && !dry_run
        && let Err(error) = drop_manifest_entries(project, &settled)
    {
        report.errors.push(format!(
            ".kittify/skills-manifest.json could not be updated ({error}); run `spec-kitty upgrade` again"
        ));
    }
    report
}
