//! Project-surface repair, config cleanup and generated-terminology effects.
//!
//! | id | effect |
//! | --- | --- |
//! | [`SURFACE_WIRING_ID`] | write the `.kittify/surface_repair_wired` marker (no mutation beyond it) |
//! | [`STRIP_SELECTION_ID`] | drop the orphaned `selection` block from `.kittify/config.yaml` |
//! | [`TERMINOLOGY_ID`] | rewrite `project root checkout`/`main repo` phrasing in *proven* generated command files |
//!
//! The terminology effect is the one that touches operator-visible bytes. It
//! is deliberately narrow, exactly like the source: a matched phrase is not
//! ownership proof, so the in-place rewrite is gated on the canonical-content
//! prover and an unproven file is preserved with the source's diagnostic.

use std::path::{Path, PathBuf};

use crate::agent::agent_surface;
use crate::error::InstallError;
use crate::upgrade::{MigrationOutcome, NativeMigration};

use super::support;

/// `0_9_3_surface_repair_wiring`.
pub const SURFACE_WIRING_ID: &str = "0_9_3_surface_repair_wiring";
/// `3.2.0rc35_strip_selection_config`.
pub const STRIP_SELECTION_ID: &str = "3.2.0rc35_strip_selection_config";
/// `3.2.0rc35_repository_root_checkout_terminology`.
pub const TERMINOLOGY_ID: &str = "3.2.0rc35_repository_root_checkout_terminology";

/// The wiring marker the sentinel writes.
pub const SURFACE_WIRING_MARKER_REL: &str = ".kittify/surface_repair_wired";

// ---------------------------------------------------------------------------
// 0_9_3_surface_repair_wiring
// ---------------------------------------------------------------------------

/// `detect`: an initialized project that has not yet recorded the probe.
pub fn detect_surface_wiring(project_root: &Path) -> Result<bool, InstallError> {
    if !project_root.join(".kittify").exists() {
        return Ok(false);
    }
    Ok(!project_root.join(SURFACE_WIRING_MARKER_REL).exists())
}

/// `can_apply`: the project must be initialized.
pub fn can_apply_surface_wiring(project_root: &Path) -> Result<Option<String>, InstallError> {
    if !project_root.join(".kittify").exists() {
        return Ok(Some(
            "No .kittify directory found; project is not initialized".to_string(),
        ));
    }
    Ok(None)
}

/// `apply`: write the sentinel and report the observed configured-surface count.
///
/// The probe is read-only and best-effort, matching the source (a probe failure
/// never blocks the migration). Native reports the configured agent surfaces it
/// can resolve, which is the surface inventory this build owns; the source's
/// status service also folds in its plugin-bundle surface, which has no native
/// counterpart.
pub fn apply_surface_wiring(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let marker = project_root.join(SURFACE_WIRING_MARKER_REL);
    support::write_generated_file(&marker, "wired\n", false)?;
    let count = configured_surface_count(project_root);
    Ok(MigrationOutcome::changed(vec![format!(
        "surface_repair_wiring: probed {count} tool surface(s)"
    )]))
}

fn configured_surface_count(project_root: &Path) -> usize {
    support::configured_agents(project_root)
        .map(|keys| {
            let mut seen: Vec<String> = Vec::new();
            for key in keys {
                if agent_surface(&key).is_ok() && !seen.iter().any(|known| known == &key) {
                    seen.push(key);
                }
            }
            seen.len()
        })
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// 3.2.0rc35_strip_selection_config
// ---------------------------------------------------------------------------

fn config_path(project_root: &Path) -> PathBuf {
    project_root.join(support::CONFIG_REL)
}

/// `detect`: either the `agents` or the legacy `tools` section carries a
/// `selection` block.
///
/// A config that cannot be parsed is not "already clean": the source swallows a
/// parse error here and reports `False`, leaving the repair to a later run once
/// the operator fixes the file. The same tolerant read is used.
pub fn detect_strip_selection(project_root: &Path) -> Result<bool, InstallError> {
    let path = config_path(project_root);
    let Some(text) = support::read_optional_text(&path)? else {
        return Ok(false);
    };
    let Ok(value) = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&text) else {
        return Ok(false);
    };
    let Some(root) = value.as_mapping() else {
        return Ok(false);
    };
    Ok(["agents", "tools"].iter().any(|key| {
        root.get(*key)
            .and_then(|section| section.as_mapping())
            .is_some_and(|section| section.contains_key("selection"))
    }))
}

/// `can_apply`: an existing config must be a readable file.
pub fn can_apply_strip_selection(project_root: &Path) -> Result<Option<String>, InstallError> {
    let path = config_path(project_root);
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(InstallError::io("inspect", &path, source)),
        Ok(metadata) if !metadata.is_file() => {
            Ok(Some("config path exists but is not a file".to_string()))
        }
        Ok(_) => match std::fs::read(&path) {
            Ok(_) => Ok(None),
            Err(source) => Ok(Some(format!("config file not readable: {source}"))),
        },
    }
}

/// `apply`: delete `agents.selection` and/or `tools.selection` from the config.
pub fn apply_strip_selection(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let path = config_path(project_root);
    let Some(text) = support::read_optional_text(&path)? else {
        return Ok(MigrationOutcome::unchanged(vec![
            "No config.yaml found".to_string(),
        ]));
    };

    let mut current = text;
    let mut changes = Vec::new();
    for root_key in ["agents", "tools"] {
        let Some(edited) = support::delete_nested_key(&path, &current, root_key, "selection")?
        else {
            continue;
        };
        current = edited;
        changes.push(format!("Removed: {root_key}.selection"));
    }

    if changes.is_empty() {
        return Ok(MigrationOutcome::unchanged(vec![
            "No selection block found".to_string(),
        ]));
    }

    sk_kernel::no_follow::write_text_no_follow(&path, &current, 0o644).map_err(|error| {
        InstallError::io("write", &path, std::io::Error::other(error.to_string()))
    })?;
    Ok(MigrationOutcome::changed(changes))
}

// ---------------------------------------------------------------------------
// 3.2.0rc35_repository_root_checkout_terminology
// ---------------------------------------------------------------------------

const REPLACEMENTS: &[(&str, &str)] = &[
    (
        "Planning happens in main for ALL missions.",
        "Planning happens in the repository root checkout for all missions. The mission target branch comes from branch-context.",
    ),
    (
        "Task generation happens in main for ALL missions.",
        "Task generation happens in the repository root checkout for all missions and commits to the mission's target branch.",
    ),
    ("project root checkout root", "repository root checkout"),
    ("Project root checkout root", "Repository root checkout"),
    ("main repository root", "repository root checkout"),
    ("Main repository root", "Repository root checkout"),
    ("project root checkout", "repository root checkout"),
    ("Project root checkout", "Repository root checkout"),
    ("main repository", "repository root checkout"),
    ("Main repository", "Repository root checkout"),
    ("main repo", "repository root checkout"),
    ("Main repo", "Repository root checkout"),
];

const DETECTION_MARKERS: &[&str] = &[
    "project root checkout",
    "main repository root",
    "main repository",
    "main repo",
    "Planning happens in main for ALL missions.",
    "Task generation happens in main for ALL missions.",
];

/// Every generated command file the configured slash-command agents own.
fn agent_command_files(project_root: &Path) -> Result<Vec<PathBuf>, InstallError> {
    let mut files = Vec::new();
    for key in support::configured_agents(project_root)? {
        let Ok(surface) = agent_surface(&key) else {
            continue;
        };
        let Some(command) = surface.command else {
            continue;
        };
        let directory = project_root.join(command.directory);
        if !directory.is_dir() {
            continue;
        }
        let entries = std::fs::read_dir(&directory)
            .map_err(|source| InstallError::io("read directory", &directory, source))?;
        let mut candidates: Vec<PathBuf> = Vec::new();
        for entry in entries {
            let entry =
                entry.map_err(|source| InstallError::io("read directory", &directory, source))?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            // The source glob is `spec-kitty.*.md`: a `<stem>.md` name whose
            // stem starts with `spec-kitty.`.
            if !name.ends_with(".md") {
                continue;
            }
            let stem = &name[..name.len() - ".md".len()];
            if stem.starts_with("spec-kitty.") {
                candidates.push(path);
            }
        }
        candidates.sort();
        files.extend(candidates);
    }
    Ok(files)
}

fn needs_fix(project_root: &Path) -> Result<Vec<PathBuf>, InstallError> {
    let mut hits = Vec::new();
    for path in agent_command_files(project_root)? {
        let Some(text) = support::read_optional_text(&path)? else {
            continue;
        };
        let lowered = text.to_lowercase();
        if DETECTION_MARKERS
            .iter()
            .any(|marker| lowered.contains(&marker.to_lowercase()))
        {
            hits.push(path);
        }
    }
    Ok(hits)
}

/// `detect`: a generated command file still carries the stale phrasing.
pub fn detect_terminology(project_root: &Path) -> Result<bool, InstallError> {
    Ok(!needs_fix(project_root)?.is_empty())
}

/// `can_apply`: at least one file still needs the update.
pub fn can_apply_terminology(project_root: &Path) -> Result<Option<String>, InstallError> {
    if needs_fix(project_root)?.is_empty() {
        return Ok(Some(
            "No generated command files need repository-root terminology updates".to_string(),
        ));
    }
    Ok(None)
}

/// `apply`: rewrite the proven generated files, preserving unproven ones.
pub fn apply_terminology(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let hits = needs_fix(project_root)?;
    if hits.is_empty() {
        return Ok(MigrationOutcome::unchanged(vec![
            "No files needed updating".to_string(),
        ]));
    }
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    let mut preserved = 0usize;
    for path in hits {
        let Some(original) = support::read_optional_text(&path)? else {
            continue;
        };
        let mut updated = original.clone();
        for (old, new) in REPLACEMENTS {
            updated = updated.replace(old, new);
        }
        if updated == original {
            continue;
        }
        let relative = crate::preserve::relative_display(&path, project_root);
        // A matched phrase is not ownership proof: only a file whose command
        // version marker proves it package-generated is rewritten in place.
        if !support::proven_package_owned(&path, project_root) {
            preserved += 1;
            warnings.push(format!(
                "Refused to overwrite {relative}; not proven package-owned and not authorized"
            ));
            continue;
        }
        support::write_generated_file(&path, &updated, true)?;
        changes.push(relative);
    }
    let summary = "Updated generated command files to use 'repository root checkout' terminology";
    Ok(MigrationOutcome {
        changed: !changes.is_empty(),
        changes: if changes.is_empty() {
            vec![summary.to_string()]
        } else {
            changes
        },
        warnings: {
            let mut warnings = warnings;
            if preserved > 0 {
                warnings.push(format!(
                    "Preserved {preserved} unproven command file(s); review them by hand"
                ));
            }
            warnings
        },
        ..crate::upgrade::MigrationOutcome::default()
    })
}

/// The three surface implementations.
pub const IMPLEMENTATIONS: &[NativeMigration] = &[
    NativeMigration {
        id: SURFACE_WIRING_ID,
        detect: detect_surface_wiring,
        can_apply: can_apply_surface_wiring,
        apply: apply_surface_wiring,
    },
    NativeMigration {
        id: STRIP_SELECTION_ID,
        detect: detect_strip_selection,
        can_apply: can_apply_strip_selection,
        apply: apply_strip_selection,
    },
    NativeMigration {
        id: TERMINOLOGY_ID,
        detect: detect_terminology,
        can_apply: can_apply_terminology,
        apply: apply_terminology,
    },
];
