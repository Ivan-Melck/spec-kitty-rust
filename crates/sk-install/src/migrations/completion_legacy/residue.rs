//! The residue-safe successors of `0.9.0`, `0.9.1` and `2.0.6`.
//!
//! The three shipped predecessors are byte-frozen and are registered by the
//! pinned registry as recorded no-ops (`SupersededMigration`), because they
//! repair *every* `kitty-specs/` child — including a residue folder that is not
//! a Mission. These successors inherit the repair and only decide **which
//! folders count**:
//!
//! * `0.9.0_residue_safe_frontmatter_only_lanes` — flatten `tasks/<lane>/` dirs
//! * `0.9.1_residue_safe_complete_migration` — move the leftover lane files,
//!   clean worktrees, normalise lane frontmatter
//! * `2.0.6_residue_safe_consistency_sweep` — repair `meta.json`/`status.json`
//!   drift and clean legacy worktree assets
//!
//! A folder is residue exactly when the pinned write-seam predicate says so
//! (`is_kitty_specs_residue`): a `spec.md`/`meta.json` on disk always keeps it a
//! Mission, a git-aware walk asks `is_mission_dir`, and the file-presence
//! fallback asks whether any Mission content exists. Each successor also stays
//! inert while its predecessor's own repair is unproven (the pinned
//! `settled_in_selection_view` gate), so a successor never re-does finished work
//! and never renames a projection a later migration wrote.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use sk_status::{EVENTS_FILENAME, SNAPSHOT_FILENAME};

use crate::error::InstallError;
use crate::ledger::{MigrationCatalog, MigrationLedger};
use crate::migrations::completion_legacy::paths::{
    self, KITTIFY, KITTY_SPECS, LANE_DIRS, WORKTREES,
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

// ------------------------------------------------------------- residue probing

/// The `kitty-specs/` child content that keeps a folder out of residue
/// (`_MISSION_CONTENT`).
pub const MISSION_CONTENT: [&str; 9] = [
    "spec.md",
    "meta.json",
    "plan.md",
    "tasks.md",
    "tasks",
    "status.events.jsonl",
    "status.json",
    "lanes.json",
    "quickstart.md",
];

/// The marker files (`_MISSION_MARKER_FILES`).
pub const MISSION_MARKERS: [&str; 2] = ["spec.md", "meta.json"];

/// The `tracked=` probe for one checkout root (`tracked_mission_paths`).
///
/// `None` means "git could not answer" (not a work tree, or git unusable), which
/// makes the predicate fall back to file presence rather than declaring nothing
/// tracked. `Some(empty)` is a real answer: git runs here and tracks no
/// `kitty-specs/` path.
pub fn tracked_mission_paths(repo_root: &Path) -> Option<BTreeSet<String>> {
    let prefix = crate::migrations::git::run(
        repo_root,
        &["rev-parse", "--is-inside-work-tree", "--show-prefix"],
    )
    .ok()?;
    if !prefix.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&prefix.stdout).into_owned();
    let mut lines = text.split('\n');
    if lines.next().unwrap_or_default().trim() != "true" {
        return None;
    }
    let prefix = lines.next().unwrap_or_default().to_string();
    let listing =
        crate::migrations::git::run(repo_root, &["ls-files", "-z", "--", KITTY_SPECS]).ok()?;
    if !listing.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&listing.stdout).into_owned();
    let mut tracked = BTreeSet::new();
    for entry in raw.split('\0') {
        if entry.is_empty() {
            continue;
        }
        tracked.insert(
            entry
                .strip_prefix(prefix.as_str())
                .unwrap_or(entry)
                .to_string(),
        );
    }
    Some(tracked)
}

fn markers_exist(path: &Path) -> bool {
    MISSION_MARKERS.iter().any(|name| path.join(name).exists())
}

fn carries_identity(path: &Path) -> bool {
    let Ok(bytes) = std::fs::read(path.join("meta.json")) else {
        return false;
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return false;
    };
    value
        .get("mission_id")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|id| !id.trim().is_empty())
}

fn relative_marker(folder: &Path, repo_root: &Path, marker: &str) -> Option<String> {
    let root = std::fs::canonicalize(repo_root).ok()?;
    let target = std::fs::canonicalize(folder.join(marker)).ok()?;
    Some(
        target
            .strip_prefix(&root)
            .ok()?
            .to_string_lossy()
            .replace('\\', "/"),
    )
}

/// `is_mission_dir`: identity, else a git-tracked marker, else (outside git) the
/// marker's existence.
pub fn is_mission_dir(folder: &Path, repo_root: &Path, tracked: Option<&BTreeSet<String>>) -> bool {
    if carries_identity(folder) {
        return true;
    }
    match tracked {
        Some(tracked) => MISSION_MARKERS.iter().any(|marker| {
            relative_marker(folder, repo_root, marker)
                .map(|relative| tracked.contains(&relative))
                .unwrap_or(false)
        }),
        None => markers_exist(folder),
    }
}

/// `is_kitty_specs_residue`: the write-seam residue predicate.
pub fn is_kitty_specs_residue(
    folder: &Path,
    repo_root: &Path,
    tracked: Option<&BTreeSet<String>>,
) -> bool {
    let under_kitty_specs = folder
        .parent()
        .and_then(Path::file_name)
        .map(|name| name == KITTY_SPECS)
        .unwrap_or(false);
    if !under_kitty_specs || markers_exist(folder) {
        return false;
    }
    if tracked.map(|tracked| !tracked.is_empty()).unwrap_or(false) {
        return !is_mission_dir(folder, repo_root, tracked);
    }
    if MISSION_CONTENT
        .iter()
        .any(|name| folder.join(name).exists())
    {
        return false;
    }
    !is_mission_dir(folder, repo_root, tracked)
}

/// One checkout's Mission folders and its residue folders, sorted.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Partition {
    /// Mission folders this successor may repair.
    pub missions: Vec<PathBuf>,
    /// Residue folders it must leave untouched.
    pub residue: Vec<PathBuf>,
}

/// Split `kitty_specs` into `(missions, residue)` for one checkout root.
pub fn partition_kitty_specs(
    kitty_specs: &Path,
    checkout_root: &Path,
    tracked: Option<&BTreeSet<String>>,
) -> Partition {
    let mut partition = Partition::default();
    if !kitty_specs.is_dir() {
        return partition;
    }
    for child in paths::directories(kitty_specs) {
        if is_kitty_specs_residue(&child, checkout_root, tracked) {
            partition.residue.push(child);
        } else {
            partition.missions.push(child);
        }
    }
    partition
}

/// Split every `kitty-specs/` child in the main checkout and each worktree.
pub fn partition_checkout_specs(project_root: &Path) -> Partition {
    let mut combined = Partition::default();
    let main_tracked = tracked_mission_paths(project_root);
    let main = partition_kitty_specs(
        &project_root.join(KITTY_SPECS),
        project_root,
        main_tracked.as_ref(),
    );
    combined.missions.extend(main.missions);
    combined.residue.extend(main.residue);
    for worktree in paths::worktrees(project_root) {
        let tracked = tracked_mission_paths(&worktree);
        let worktree_partition =
            partition_kitty_specs(&worktree.join(KITTY_SPECS), &worktree, tracked.as_ref());
        combined.missions.extend(worktree_partition.missions);
        combined.residue.extend(worktree_partition.residue);
    }
    combined
}

/// One residue notice per folder, naming its project-relative POSIX path.
pub fn residue_warnings(residue: &[PathBuf], project_root: &Path) -> Vec<String> {
    residue
        .iter()
        .map(|folder| {
            format!(
                "Left kitty-specs/ residue folder untouched (not a Mission): {}",
                paths::relative_display(project_root, folder)
            )
        })
        .collect()
}

/// `settled_in_selection_view`: the id's authoritative partition records a
/// `success`, or a pending ledger adoption will record one.
pub fn settled_in_selection_view(project_root: &Path, migration_id: &str) -> bool {
    let Ok(catalog) = MigrationCatalog::embedded() else {
        return false;
    };
    let Some(spec) = catalog.get(migration_id) else {
        return false;
    };
    let ledger = MigrationLedger::new(project_root);
    if ledger.is_settled(spec).unwrap_or(false) {
        return true;
    }
    match crate::migrations::ledger_adoption::adoption_settled_ids(project_root) {
        Ok(adopting) => adopting.contains(migration_id),
        Err(_) => false,
    }
}

// ------------------------------------------------- 0.9.0 residue-safe flatten

fn legacy_lane_dir(feature: &Path) -> bool {
    let tasks = feature.join("tasks");
    if !tasks.exists() {
        return false;
    }
    LANE_DIRS.iter().any(|lane| {
        let lane_dir = tasks.join(lane);
        lane_dir.is_dir()
            && (!paths::real_contents(&lane_dir).is_empty()
                || !paths::entries(&lane_dir).is_empty())
    })
}

/// `0.9.0_residue_safe_frontmatter_only_lanes.detect`.
pub fn detect_residue_safe_frontmatter_lanes(project_root: &Path) -> Result<bool, InstallError> {
    if settled_in_selection_view(project_root, "0.9.0_frontmatter_only_lanes") {
        return Ok(false);
    }
    let partition = partition_checkout_specs(project_root);
    Ok(partition
        .missions
        .iter()
        .any(|mission| legacy_lane_dir(mission)))
}

/// `0.9.0_residue_safe_frontmatter_only_lanes.can_apply`: always safe.
pub fn can_apply_always(_project_root: &Path) -> Result<Option<String>, InstallError> {
    Ok(None)
}

fn migrate_lane_files(
    feature: &Path,
    location_label: &str,
    any_extension: bool,
    changes: &mut Vec<String>,
    warnings: &mut Vec<String>,
) -> (usize, usize) {
    let tasks = feature.join("tasks");
    let feature_name = feature
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    changes.push(format!("[{location_label}] {feature_name}:"));
    let mut migrated = 0usize;
    let mut skipped = 0usize;
    for lane in LANE_DIRS {
        let lane_dir = tasks.join(lane);
        if !lane_dir.is_dir() {
            continue;
        }
        for item in paths::entries(&lane_dir) {
            let Some(name) = item
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
            else {
                continue;
            };
            if any_extension {
                if name == ".gitkeep" {
                    continue;
                }
                if item.is_dir() {
                    warnings.push(format!(
                        "  Warning: Nested directory {lane}/{name}/ found - please check manually"
                    ));
                    continue;
                }
            } else {
                if name == "README.md" || !name.ends_with(".md") || !item.is_file() {
                    continue;
                }
            }
            let target = tasks.join(&name);
            if target.exists() {
                warnings.push(format!("  Skip: {name} already exists in tasks/"));
                skipped += 1;
                continue;
            }
            match std::fs::read(&item) {
                Ok(bytes) => {
                    let content = String::from_utf8_lossy(&bytes).into_owned();
                    let updated = paths::ensure_lane_in_frontmatter(
                        content.strip_prefix('\u{feff}').unwrap_or(&content),
                        lane,
                    );
                    if let Err(error) = paths::write_text(&target, &updated) {
                        warnings.push(format!("  Error migrating {name}: {error}"));
                        continue;
                    }
                    if let Err(error) = std::fs::remove_file(&item) {
                        warnings.push(format!("  Error removing {lane}/{name}: {error}"));
                        continue;
                    }
                    changes.push(format!("  Moved: {lane}/{name} → tasks/{name}"));
                    migrated += 1;
                }
                Err(error) => warnings.push(format!("  Error migrating {name}: {error}")),
            }
        }
        // Clean up a lane directory that holds nothing but OS noise.
        if paths::real_contents(&lane_dir).is_empty()
            && paths::remove_tool_owned_tree(&lane_dir, &lane_dir, "empty lane directory", false)
                .unwrap_or(false)
        {
            changes.push(format!("  Removed empty: {lane}/"));
        }
    }
    (migrated, skipped)
}

/// `0.9.0_residue_safe_frontmatter_only_lanes.apply`.
pub fn apply_residue_safe_frontmatter_lanes(
    project_root: &Path,
) -> Result<MigrationOutcome, InstallError> {
    let partition = partition_checkout_specs(project_root);
    let mut changes = Vec::new();
    let mut warnings = residue_warnings(&partition.residue, project_root);
    let features: Vec<&PathBuf> = partition
        .missions
        .iter()
        .filter(|mission| legacy_lane_dir(mission))
        .collect();
    if features.is_empty() {
        warnings.push("No features need migration - all already use flat structure".to_string());
        return Ok(MigrationOutcome {
            changed: false,
            changes,
            warnings,
            ..crate::upgrade::MigrationOutcome::default()
        });
    }
    let mut total_migrated = 0usize;
    let mut total_skipped = 0usize;
    for feature in &features {
        let label = if feature.starts_with(project_root.join(WORKTREES)) {
            feature
                .strip_prefix(project_root.join(WORKTREES))
                .ok()
                .and_then(|rest| rest.components().next())
                .map(|component| format!("worktree:{}", component.as_os_str().to_string_lossy()))
                .unwrap_or_else(|| "worktree".to_string())
        } else {
            "main".to_string()
        };
        let (migrated, skipped) =
            migrate_lane_files(feature, &label, false, &mut changes, &mut warnings);
        total_migrated += migrated;
        total_skipped += skipped;
    }
    changes.push(format!(
        "Migrated {total_migrated} WP files across {} features",
        features.len()
    ));
    if total_skipped > 0 {
        warnings.push(format!(
            "Skipped {total_skipped} files (already exist or conflicts)"
        ));
    }
    Ok(outcome(total_migrated > 0, changes, warnings))
}

// ------------------------------------- 0.9.1 residue-safe complete migration

fn has_remaining_lane_dirs(feature: &Path) -> bool {
    let tasks = feature.join("tasks");
    if !tasks.exists() {
        return false;
    }
    LANE_DIRS.iter().any(|lane| {
        let lane_dir = tasks.join(lane);
        lane_dir.is_dir()
            && (!paths::real_contents(&lane_dir).is_empty()
                || !paths::entries(&lane_dir).is_empty())
    })
}

/// `0.9.1_residue_safe_complete_migration.detect`.
pub fn detect_residue_safe_complete_migration(project_root: &Path) -> Result<bool, InstallError> {
    if settled_in_selection_view(project_root, "0.9.1_complete_migration") {
        return Ok(false);
    }
    let partition = partition_checkout_specs(project_root);
    if partition
        .missions
        .iter()
        .any(|mission| has_remaining_lane_dirs(mission))
    {
        return Ok(true);
    }
    for worktree in paths::worktrees(project_root) {
        for directory in crate::migrations::retirements::known_agent_directories() {
            if worktree.join(&directory.directory).exists() {
                return Ok(true);
            }
        }
        if worktree.join(KITTIFY).join("scripts").exists() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `0.9.1_residue_safe_complete_migration.apply`.
pub fn apply_residue_safe_complete_migration(
    project_root: &Path,
) -> Result<MigrationOutcome, InstallError> {
    let partition = partition_checkout_specs(project_root);
    let mut changes = vec!["=== Part 1: Complete Lane Migration ===".to_string()];
    let mut warnings = residue_warnings(&partition.residue, project_root);
    let mut migrated_total = 0usize;
    for feature in &partition.missions {
        if !has_remaining_lane_dirs(feature) {
            continue;
        }
        let label = if feature.starts_with(project_root.join(WORKTREES)) {
            feature
                .strip_prefix(project_root.join(WORKTREES))
                .ok()
                .and_then(|rest| rest.components().next())
                .map(|component| format!("worktree:{}", component.as_os_str().to_string_lossy()))
                .unwrap_or_else(|| "worktree".to_string())
        } else {
            "main".to_string()
        };
        let (migrated, _skipped) =
            migrate_lane_files(feature, &label, true, &mut changes, &mut warnings);
        migrated_total += migrated;
    }
    changes.push(format!(
        "Migrated {migrated_total} files out of lane directories"
    ));

    changes.push(String::new());
    changes.push("=== Part 2: Worktree Cleanup ===".to_string());
    let mut cleaned = 0usize;
    for worktree in paths::worktrees(project_root) {
        let mut worktree_cleaned = false;
        for directory in crate::migrations::retirements::known_agent_directories() {
            let commands = worktree.join(&directory.directory);
            if !(commands.exists() || commands.is_symlink()) {
                continue;
            }
            match paths::remove_tool_owned_tree(
                &commands,
                &worktree,
                "worktree agent commands dir",
                false,
            ) {
                Ok(_) => {
                    changes.push(format!(
                        "[{}] Removed {}/ (inherits from main)",
                        worktree
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        directory.directory
                    ));
                    if let Some(parent) = commands.parent() {
                        paths::remove_if_empty(parent);
                    }
                    worktree_cleaned = true;
                }
                Err(error) => warnings.push(format!(
                    "[{}] Failed to remove {}/: {error}",
                    worktree
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    directory.directory
                )),
            }
        }
        let scripts = worktree.join(KITTIFY).join("scripts");
        if scripts.exists() || scripts.is_symlink() {
            match paths::remove_tool_owned_tree(
                &scripts,
                &worktree,
                "worktree .kittify/scripts",
                false,
            ) {
                Ok(_) => {
                    changes.push(format!(
                        "[{}] Removed .kittify/scripts/ (inherits from main)",
                        worktree
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    ));
                    worktree_cleaned = true;
                }
                Err(error) => warnings.push(format!(
                    "[{}] Failed to remove .kittify/scripts/: {error}",
                    worktree
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default()
                )),
            }
        }
        if worktree_cleaned {
            cleaned += 1;
        }
    }
    changes.push(if cleaned > 0 {
        format!("Cleaned up {cleaned} worktrees")
    } else {
        "No worktrees needed cleanup".to_string()
    });

    changes.push(String::new());
    changes.push("=== Part 3: Normalize Frontmatter ===".to_string());
    let mut normalized = 0usize;
    let mut skipped = 0usize;
    let mut markdown: Vec<PathBuf> = Vec::new();
    for mission in &partition.missions {
        markdown.extend(paths::markdown_files(mission));
    }
    if markdown.is_empty() {
        changes.push("No markdown files found".to_string());
    }
    for file in markdown {
        if !file
            .components()
            .any(|component| component.as_os_str() == "tasks")
        {
            continue;
        }
        let Some(content) = paths::read_text(&file) else {
            skipped += 1;
            continue;
        };
        if paths::split_frontmatter(&content).is_none() {
            warnings.push(format!(
                "Skip (no frontmatter): {}",
                paths::relative_display(project_root, &file)
            ));
            skipped += 1;
            continue;
        }
        let lane = paths::split_frontmatter(&content)
            .and_then(|(frontmatter, _)| paths::frontmatter_value(&frontmatter, "lane"))
            .map(|lane| sk_status::resolve_lane_alias(&lane))
            .unwrap_or_else(|| "planned".to_string());
        let Some(rendered) = paths::normalized_lane_document(&content, &lane) else {
            skipped += 1;
            continue;
        };
        if rendered == content {
            skipped += 1;
            continue;
        }
        paths::write_text(&file, &rendered)?;
        changes.push(format!(
            "Normalized: {}",
            paths::relative_display(project_root, &file)
        ));
        normalized += 1;
    }
    changes.push(format!("Normalized {normalized} files, skipped {skipped}"));
    Ok(outcome(
        migrated_total > 0 || cleaned > 0 || normalized > 0,
        changes,
        warnings,
    ))
}

// ------------------------------------- 2.0.6 residue-safe consistency sweep

/// `check_stale_legacy_assets`: legacy runtime assets that must be migrated.
fn has_stale_legacy_assets(project_root: &Path) -> bool {
    let kittify = project_root.join(KITTIFY);
    for directory in paths::directories(&kittify) {
        let relative = directory.strip_prefix(&kittify).unwrap_or(&directory);
        let top_level = relative
            .components()
            .next()
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .unwrap_or_default();
        if paths::SHARED_ASSET_DIRS.contains(&top_level.as_str()) {
            return true;
        }
    }
    false
}

fn legacy_worktree_assets(project_root: &Path) -> bool {
    let roots: Vec<PathBuf> = if project_root
        .components()
        .any(|component| component.as_os_str() == WORKTREES)
    {
        vec![project_root.to_path_buf()]
    } else {
        paths::worktrees(project_root)
    };
    roots.into_iter().any(|root| {
        crate::migrations::retirements::known_agent_directories()
            .iter()
            .any(|directory| root.join(&directory.directory).exists())
            || root.join(KITTIFY).join("scripts").exists()
    })
}

fn baseline_slug_fields(feature_name: &str) -> Vec<(&'static str, String)> {
    let (number, tail) = match feature_name.split_once('-') {
        Some((number, tail)) => (number, tail),
        None => (feature_name, ""),
    };
    let friendly = if tail.is_empty() {
        feature_name.to_string()
    } else {
        tail.replace('-', " ").trim().to_string()
    };
    vec![
        (
            "mission_number",
            if number.chars().all(|character| character.is_ascii_digit()) {
                number.to_string()
            } else {
                String::new()
            },
        ),
        ("slug", feature_name.to_string()),
        ("mission_slug", feature_name.to_string()),
        (
            "friendly_name",
            if friendly.is_empty() {
                feature_name.to_string()
            } else {
                friendly
            },
        ),
    ]
}

fn infer_mission(feature: &Path, existing: &serde_json::Map<String, serde_json::Value>) -> String {
    if let Some(mission) = existing
        .get("mission_type")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|mission| !mission.is_empty())
    {
        return mission.to_string();
    }
    if feature.join("research").exists() {
        "research".to_string()
    } else {
        "software-dev".to_string()
    }
}

fn infer_created_at(feature: &Path) -> String {
    let mut earliest: Option<std::time::SystemTime> = None;
    let mut stack = vec![feature.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in paths::entries(&directory) {
            let Ok(metadata) = std::fs::symlink_metadata(&entry) else {
                continue;
            };
            if metadata.is_dir() {
                stack.push(entry);
                continue;
            }
            if let Ok(modified) = metadata.modified() {
                earliest = Some(match earliest {
                    Some(current) if current <= modified => current,
                    _ => modified,
                });
            }
        }
    }
    let stamp = earliest.unwrap_or_else(std::time::SystemTime::now);
    let datetime: chrono::DateTime<chrono::Utc> = stamp.into();
    datetime.to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}

/// `build_baseline_feature_meta`: the minimum viable `meta.json` payload.
///
/// A residue folder gets no baseline (its `existing` comes back unchanged), and
/// a Mission whose tracked `meta.json` git holds but cannot return is refused by
/// returning `None`, so no identity is fabricated over an unreadable one.
pub fn build_baseline_feature_meta(
    feature: &Path,
    repo_root: &Path,
    existing: Option<serde_json::Value>,
) -> Option<serde_json::Value> {
    if is_kitty_specs_residue(feature, repo_root, None) {
        return existing;
    }
    let tracked = crate::migrations::completion_legacy::assets::read_tracked_meta(feature);
    let has_id = existing
        .as_ref()
        .and_then(|value| value.get("mission_id"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|id| !id.trim().is_empty());
    if feature.join("meta.json").exists() && has_id {
        // A healthy Mission costs no git read.
    } else if tracked.unavailable {
        return None;
    } else if !feature.join("meta.json").exists()
        && let Some(tracked_meta) = tracked.meta.clone()
    {
        let mut seeded = tracked_meta;
        apply_baseline_fields(feature, repo_root, &mut seeded);
        return Some(seeded);
    } else if !has_id
        && let Some(tracked_id) = tracked
            .meta
            .as_ref()
            .and_then(|value| value.get("mission_id"))
            .and_then(serde_json::Value::as_str)
            .filter(|id| !id.trim().is_empty())
    {
        let mut seeded = existing
            .clone()
            .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
        if let Some(map) = seeded.as_object_mut() {
            map.insert(
                "mission_id".to_string(),
                serde_json::Value::String(tracked_id.to_string()),
            );
        }
        apply_baseline_fields(feature, repo_root, &mut seeded);
        return Some(seeded);
    }
    let mut meta = existing.unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
    apply_baseline_fields(feature, repo_root, &mut meta);
    Some(meta)
}

fn apply_baseline_fields(feature: &Path, repo_root: &Path, meta: &mut serde_json::Value) {
    let Some(map) = meta.as_object_mut() else {
        return;
    };
    let feature_name = feature
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    for (key, value) in baseline_slug_fields(&feature_name) {
        set_if_blank(map, key, serde_json::Value::String(value));
    }
    let mission = infer_mission(feature, map);
    set_if_blank(map, "mission_type", serde_json::Value::String(mission));
    let target_branch =
        crate::migrations::completion_legacy::meta::infer_target_branch(feature, repo_root);
    set_if_blank(
        map,
        "target_branch",
        serde_json::Value::String(target_branch),
    );
    let created_at = infer_created_at(feature);
    set_if_blank(map, "created_at", serde_json::Value::String(created_at));
}

fn set_if_blank(
    map: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    value: serde_json::Value,
) {
    match map.get(key) {
        None => {
            map.insert(key.to_string(), value);
        }
        Some(serde_json::Value::String(text)) if text.trim().is_empty() => {
            map.insert(key.to_string(), value);
        }
        _ => {}
    }
}

fn write_meta(feature: &Path, meta: &serde_json::Value) -> Result<(), InstallError> {
    let mut text =
        serde_json::to_string_pretty(meta).map_err(|error| InstallError::UpgradeRefused {
            detail: format!("could not serialize meta.json: {error}"),
        })?;
    text.push('\n');
    paths::write_text(&feature.join("meta.json"), &text)?;
    Ok(())
}

fn wp_frontmatter_needs_normalization(feature: &Path) -> bool {
    let tasks = feature.join("tasks");
    if !tasks.exists() {
        return false;
    }
    for file in paths::entries(&tasks) {
        let Some(name) = file
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
        else {
            continue;
        };
        if !name.starts_with("WP") || !name.ends_with(".md") || !file.is_file() {
            continue;
        }
        let Some(content) = paths::read_text(&file) else {
            return true;
        };
        let Some((frontmatter, _body)) = paths::split_frontmatter(&content) else {
            return true;
        };
        for line in &frontmatter {
            if let Some(rest) = line.strip_prefix("lane:") {
                let value = rest.trim();
                if value.starts_with('"') || value.starts_with('\'') || value == "doing" {
                    return true;
                }
            }
        }
        match paths::frontmatter_value(&frontmatter, "lane") {
            None => return true,
            Some(value) if value.trim().is_empty() => return true,
            Some(_) => {}
        }
    }
    false
}

fn orphan_status_snapshot(feature: &Path) -> bool {
    let status_path = feature.join(SNAPSHOT_FILENAME);
    let events_path = feature.join(EVENTS_FILENAME);
    if !status_path.exists() || events_path.exists() {
        return false;
    }
    let Ok(bytes) = std::fs::read(&status_path) else {
        return false;
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return false;
    };
    let feature_name = feature
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let event_count_zero = value
        .get("event_count")
        .and_then(serde_json::Value::as_i64)
        .is_some_and(|count| count == 0);
    let no_packages = value
        .get("work_packages")
        .and_then(serde_json::Value::as_object)
        .is_some_and(serde_json::Map::is_empty);
    let slug = value
        .get("feature_slug")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    event_count_zero && no_packages && (slug.is_empty() || slug == feature_name)
}

fn tasks_md_has_legacy_prompt_refs(feature: &Path) -> bool {
    let Some(content) = paths::read_text(&feature.join("tasks.md")) else {
        return false;
    };
    content.lines().any(|line| {
        line.starts_with("**Prompt**:")
            && [
                "tasks/planned/",
                "tasks/doing/",
                "tasks/for_review/",
                "tasks/done/",
            ]
            .iter()
            .any(|segment| line.contains(segment))
    })
}

fn has_event_log(feature: &Path) -> bool {
    paths::read_text(&feature.join(EVENTS_FILENAME))
        .map(|text| !text.trim().is_empty())
        .unwrap_or(false)
}

fn status_drift(feature: &Path) -> bool {
    if !has_event_log(feature) {
        return false;
    }
    let Ok(input) = sk_status::ReplayInput::read_dir(feature) else {
        return true;
    };
    let Ok(snapshot) = sk_status::materialize(&input) else {
        return true;
    };
    let Ok(rendered) = sk_status::render_snapshot_json(&snapshot) else {
        return true;
    };
    match std::fs::read_to_string(feature.join(SNAPSHOT_FILENAME)) {
        Ok(current) => current != rendered,
        Err(_) => true,
    }
}

fn feature_requires_repair(feature: &Path, project_root: &Path) -> bool {
    let existing = crate::migrations::completion_legacy::meta::load_meta(feature);
    let desired = build_baseline_feature_meta(feature, project_root, existing.clone());
    if existing != desired {
        return true;
    }
    tasks_md_has_legacy_prompt_refs(feature)
        || orphan_status_snapshot(feature)
        || wp_frontmatter_needs_normalization(feature)
        || status_drift(feature)
}

/// `2.0.6_residue_safe_consistency_sweep.detect`.
pub fn detect_residue_safe_consistency_sweep(project_root: &Path) -> Result<bool, InstallError> {
    if settled_in_selection_view(project_root, "2.0.6_consistency_sweep") {
        return Ok(false);
    }
    if has_stale_legacy_assets(project_root) || legacy_worktree_assets(project_root) {
        return Ok(true);
    }
    let tracked = tracked_mission_paths(project_root);
    let partition = partition_kitty_specs(
        &project_root.join(KITTY_SPECS),
        project_root,
        tracked.as_ref(),
    );
    Ok(partition
        .missions
        .iter()
        .any(|mission| feature_requires_repair(mission, project_root)))
}

/// `2.0.6_residue_safe_consistency_sweep.can_apply`.
pub fn can_apply_consistency_sweep(project_root: &Path) -> Result<Option<String>, InstallError> {
    if project_root.join(KITTIFY).exists() || project_root.join(KITTY_SPECS).exists() {
        return Ok(None);
    }
    Ok(Some(
        "No .kittify/ or kitty-specs/ directory found".to_string(),
    ))
}

fn normalize_wp_frontmatter(feature: &Path) -> Result<usize, InstallError> {
    let tasks = feature.join("tasks");
    if !tasks.exists() {
        return Ok(0);
    }
    let mut normalized = 0usize;
    for file in paths::entries(&tasks) {
        let Some(name) = file
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
        else {
            continue;
        };
        if !name.starts_with("WP") || !name.ends_with(".md") || !file.is_file() {
            continue;
        }
        let Some(content) = paths::read_text(&file) else {
            continue;
        };
        let Some((frontmatter, _body)) = paths::split_frontmatter(&content) else {
            continue;
        };
        let raw_lane =
            paths::frontmatter_value(&frontmatter, "lane").unwrap_or_else(|| "planned".to_string());
        let lane = sk_status::resolve_lane_alias(&raw_lane);
        let Some(rendered) = paths::normalized_lane_document(&content, &lane) else {
            continue;
        };
        if rendered == content {
            continue;
        }
        paths::write_text(&file, &rendered)?;
        normalized += 1;
    }
    Ok(normalized)
}

fn rewrite_tasks_prompt_refs(feature: &Path, dry_run: bool) -> Result<usize, InstallError> {
    let tasks_md = feature.join("tasks.md");
    let tasks = feature.join("tasks");
    if !tasks_md.exists() || !tasks.exists() {
        return Ok(0);
    }
    let Some(content) = paths::read_text(&tasks_md) else {
        return Ok(0);
    };
    let mut rewrites = 0usize;
    let mut lines: Vec<String> = Vec::new();
    for line in content.split_inclusive('\n') {
        let trailing = line.ends_with('\n');
        let body = line.trim_end_matches('\n');
        let mut rewritten = body.to_string();
        if let Some(value) = body.strip_prefix("**Prompt**: ")
            && value.contains("tasks/")
            && let Some(wp) = wp_id_from_prompt(value)
        {
            let candidates: Vec<PathBuf> = paths::entries(&tasks)
                .into_iter()
                .filter(|path| {
                    path.file_name()
                        .map(|name| {
                            let name = name.to_string_lossy();
                            name.starts_with(&wp) && name.ends_with(".md")
                        })
                        .unwrap_or(false)
                })
                .collect();
            if let Some(candidate) = candidates.first()
                && let Some(index) = value.find("tasks/")
            {
                let name = candidate
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let new_value = format!("{}tasks/{name}", &value[..index]);
                if new_value != value {
                    rewritten = format!("**Prompt**: {new_value}");
                    rewrites += 1;
                }
            }
        }
        lines.push(rewritten);
        if trailing {
            lines.push("\n".to_string());
        }
    }
    if rewrites > 0 && !dry_run {
        paths::write_text(&tasks_md, &lines.concat())?;
    }
    Ok(rewrites)
}

fn wp_id_from_prompt(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut index = 0usize;
    while index + 1 < bytes.len() {
        if bytes[index] == b'W' && bytes[index + 1] == b'P' {
            let digits: String = value[index + 2..]
                .chars()
                .take_while(|character| character.is_ascii_digit())
                .collect();
            if !digits.is_empty() {
                return Some(format!("WP{digits}"));
            }
        }
        index += 1;
    }
    None
}

fn cleanup_worktree_assets(
    project_root: &Path,
    changes: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    let roots: Vec<PathBuf> = if project_root
        .components()
        .any(|component| component.as_os_str() == WORKTREES)
    {
        vec![project_root.to_path_buf()]
    } else {
        paths::worktrees(project_root)
    };
    for root in roots {
        let name = root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        for directory in crate::migrations::retirements::known_agent_directories() {
            let commands = root.join(&directory.directory);
            if !(commands.exists() || commands.is_symlink()) {
                continue;
            }
            match paths::remove_tool_owned_tree(
                &commands,
                &root,
                "worktree agent commands dir",
                false,
            ) {
                Ok(_) => {
                    changes.push(format!("[{name}] removed {}/", directory.directory));
                    if let Some(parent) = commands.parent() {
                        paths::remove_if_empty(parent);
                    }
                }
                Err(error) => warnings.push(format!(
                    "[{name}] failed to remove {}/: {error}",
                    directory.directory
                )),
            }
        }
        let scripts = root.join(KITTIFY).join("scripts");
        if scripts.exists() || scripts.is_symlink() {
            match paths::remove_tool_owned_tree(&scripts, &root, "worktree .kittify/scripts", false)
            {
                Ok(_) => changes.push(format!("[{name}] removed .kittify/scripts/")),
                Err(error) => warnings.push(format!(
                    "[{name}] failed to remove .kittify/scripts/: {error}"
                )),
            }
        }
    }
}

/// `2.0.6_residue_safe_consistency_sweep.apply`.
pub fn apply_residue_safe_consistency_sweep(
    project_root: &Path,
) -> Result<MigrationOutcome, InstallError> {
    let tracked = tracked_mission_paths(project_root);
    let partition = partition_kitty_specs(
        &project_root.join(KITTY_SPECS),
        project_root,
        tracked.as_ref(),
    );
    let mut changes = Vec::new();
    let mut warnings = residue_warnings(&partition.residue, project_root);

    if project_root.join(KITTIFY).exists() {
        match crate::migrations::completion_legacy::layout::execute_migration(
            project_root,
            false,
            "software-dev",
        ) {
            Ok(report) => {
                let moved = report.moved.len();
                let removed = report.removed.len();
                if moved > 0 || removed > 0 {
                    changes.push(format!(
                        "migrated {removed} identical and {moved} customized legacy runtime asset(s)"
                    ));
                }
                if !report.conflicts.is_empty() {
                    warnings.push(format!(
                        "{} customized legacy runtime asset(s) not moved: an override already exists, a non-folder blocks the overrides path, or the file is a symbolic link; review the legacy copies",
                        report.conflicts.len()
                    ));
                }
            }
            Err(error) => warnings.push(format!("runtime asset cleanup skipped: {error}")),
        }
    }

    cleanup_worktree_assets(project_root, &mut changes, &mut warnings);

    for mission in &partition.missions {
        let missing_tracked =
            crate::migrations::completion_legacy::assets::read_tracked_meta(mission);
        if missing_tracked.unavailable && !mission.join("meta.json").exists() {
            warnings.push(format!(
                "{}: tracked meta.json is unavailable; repair nothing here so no identity is fabricated",
                paths::relative_display(project_root, mission)
            ));
            continue;
        }
        let existing = crate::migrations::completion_legacy::meta::load_meta(mission);
        let desired = build_baseline_feature_meta(mission, project_root, existing.clone());
        if existing != desired
            && let Some(desired) = &desired
        {
            write_meta(mission, desired)?;
            changes.push(format!(
                "{}: wrote baseline meta.json",
                paths::relative_display(project_root, mission)
            ));
        }

        let normalized = normalize_wp_frontmatter(mission)?;
        if normalized > 0 {
            changes.push(format!(
                "{}: normalized {normalized} work package files",
                paths::relative_display(project_root, mission)
            ));
        }

        if orphan_status_snapshot(mission) {
            let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
            let backup = format!("{SNAPSHOT_FILENAME}.orphan.bak.{stamp}");
            std::fs::rename(mission.join(SNAPSHOT_FILENAME), mission.join(&backup))
                .map_err(|error| InstallError::io("archive orphan status.json", mission, error))?;
            changes.push(format!(
                "{}: archived orphan {SNAPSHOT_FILENAME} to {backup}",
                paths::relative_display(project_root, mission)
            ));
        }

        let events_path = mission.join(EVENTS_FILENAME);
        let unreadable = events_path.exists()
            && paths::read_text(&events_path)
                .map(|text| !text.trim().is_empty())
                .unwrap_or(false)
            && sk_status::read_raw_events(&events_path)
                .map(|rows| sk_status::parse_diary(&rows).is_err())
                .unwrap_or(true);
        if unreadable {
            let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
            let backup = format!("{EVENTS_FILENAME}.unreadable.bak.{stamp}");
            std::fs::rename(&events_path, mission.join(&backup)).map_err(|error| {
                InstallError::io("archive unreadable event log", &events_path, error)
            })?;
            changes.push(format!(
                "{}: archived unreadable {EVENTS_FILENAME} to {backup}",
                paths::relative_display(project_root, mission)
            ));
            warnings.push(format!(
                "{}: status.events.jsonl was unreadable and has been quarantined",
                paths::relative_display(project_root, mission)
            ));
        }

        if has_event_log(mission) || unreadable {
            let drifted = status_drift(mission);
            match sk_status::materialize_snapshot_file(mission) {
                Ok(_) => {
                    if drifted || unreadable {
                        changes.push(format!(
                            "{}: regenerated status.json",
                            paths::relative_display(project_root, mission)
                        ));
                    }
                }
                Err(error) => warnings.push(format!(
                    "{}: could not regenerate status.json: {error}",
                    paths::relative_display(project_root, mission)
                )),
            }
        }

        let rewrites = rewrite_tasks_prompt_refs(mission, false)?;
        if rewrites > 0 {
            changes.push(format!(
                "{}: rewrote {rewrites} legacy prompt reference(s)",
                paths::relative_display(project_root, mission)
            ));
        }
    }

    if changes.is_empty() && warnings.is_empty() {
        changes.push("Project already consistent for 2.0.6".to_string());
    }
    Ok(outcome(!changes.is_empty(), changes, warnings))
}
