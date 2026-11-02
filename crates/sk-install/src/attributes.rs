//! `.gitattributes` seed and the typed merge-driver wiring plan.
//!
//! Two halves make a merge driver work:
//!
//! 1. an **attribute mapping** in `.gitattributes` (tracked, travels with the
//!    repository), and
//! 2. a **`merge.<key>.name` / `merge.<key>.driver` value** in the per-clone
//!    git config (clone-local; a teammate's fresh clone has the mapping but no
//!    drivers until `upgrade`/init re-wires it).
//!
//! This crate owns the tracked half: [`ensure_attribute_lines`] appends the
//! canonical seed from the driver registry *and* the inert historical
//! `issue-matrix.md` line an upgrade migration still seeds, so the first upgrade
//! of a just-initialized project adds nothing. The seed and the diff attributes
//! are the same lists the Python initialization writes.
//!
//! The clone-local half is a **typed plan**, not a subprocess: git config reads
//! and writes belong to the Git owner (`sk-git`), which does not yet expose a
//! config seam. [`MergeDriverWiringPlan`] carries the exact keys, values and
//! accepted previous values so that owner (or the CLI's Git adapter) can apply
//! them without re-deriving this contract. A project that is not a git checkout
//! root gets the attribute half only, with the reason recorded — exactly the
//! best-effort behavior upstream has, minus an invented second git writer.

use std::path::{Path, PathBuf};

use crate::error::InstallError;

/// One merge driver: its config key, shipped values, and attribute patterns.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MergeDriver {
    /// The `merge.<key>` suffix.
    pub config_key: &'static str,
    /// The shipped display name.
    pub name: &'static str,
    /// The shipped driver command (with git's `%O %A %B` arguments).
    pub command: &'static str,
    /// The attribute patterns this driver covers.
    pub patterns: &'static [&'static str],
    /// Names a previous release shipped for this key (kept as "not user-customized").
    pub accepted_names: &'static [&'static str],
}

/// The canonical merge-driver registry (mirrors `lanes/consolidation._MERGE_DRIVERS`).
pub const MERGE_DRIVERS: [MergeDriver; 8] = [
    MergeDriver {
        config_key: "spec-kitty-event-log",
        name: "Spec Kitty event log union merge",
        command: "spec-kitty merge-driver-event-log %O %A %B",
        patterns: &[
            "kitty-specs/**/status.events.jsonl",
            "kitty-specs/**/decisions.events.jsonl",
        ],
        accepted_names: &[],
    },
    MergeDriver {
        config_key: "spec-kitty-meta",
        name: "Spec Kitty mission meta field merge",
        command: "spec-kitty merge-driver-meta %O %A %B",
        patterns: &["kitty-specs/**/meta.json"],
        accepted_names: &[],
    },
    MergeDriver {
        config_key: "spec-kitty-traces",
        name: "Spec Kitty mission traces union merge",
        command: "spec-kitty merge-driver-traces %O %A %B",
        patterns: &["kitty-specs/**/traces/*.md"],
        accepted_names: &[],
    },
    MergeDriver {
        config_key: "spec-kitty-acceptance-matrix",
        name: "Spec Kitty acceptance matrix filled-side merge",
        command: "spec-kitty merge-driver-acceptance-matrix %O %A %B",
        patterns: &["kitty-specs/**/acceptance-matrix.json"],
        accepted_names: &[],
    },
    MergeDriver {
        config_key: "spec-kitty-issue-matrix",
        name: "Spec Kitty issue matrix row-aware merge",
        command: "spec-kitty merge-driver-issue-matrix %O %A %B",
        patterns: &["kitty-specs/**/issue-matrix.json"],
        accepted_names: &["Spec Kitty issue matrix filled-side merge"],
    },
    MergeDriver {
        config_key: "spec-kitty-review-cycle",
        name: "Spec Kitty review-cycle verdict collision refusal",
        command: "spec-kitty merge-driver-review-cycle %O %A %B",
        patterns: &["kitty-specs/**/tasks/*/review-cycle-*.md"],
        accepted_names: &[],
    },
    MergeDriver {
        config_key: "spec-kitty-decision-index",
        name: "Spec Kitty decision-index entry union merge",
        command: "spec-kitty merge-driver-decision-index %O %A %B",
        patterns: &["kitty-specs/**/decisions/index.json"],
        accepted_names: &[],
    },
    MergeDriver {
        config_key: "spec-kitty-issue-matrix",
        name: "Spec Kitty issue matrix row-aware merge",
        command: "spec-kitty merge-driver-issue-matrix %O %A %B",
        // Historical inert mapping a pre-4.x upgrade migration still seeds.
        patterns: &["kitty-specs/**/issue-matrix.md"],
        accepted_names: &["Spec Kitty issue matrix filled-side merge"],
    },
];

/// Every `.gitattributes` line init writes for merge drivers (registry + historical seed).
pub fn merge_attribute_seed_lines() -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for driver in MERGE_DRIVERS {
        for pattern in driver.patterns {
            let line = format!("{pattern} merge={}", driver.config_key);
            if !lines.contains(&line) {
                lines.push(line);
            }
        }
    }
    lines
}

/// GitHub diff-rendering attributes written next to the driver seed.
pub const GITHUB_DIFF_ATTRIBUTES: [&str; 17] = [
    "kitty-specs/**/status.json linguist-generated=true",
    "kitty-specs/**/status.events.jsonl linguist-generated=true",
    "kitty-specs/**/lanes.json linguist-generated=true",
    "kitty-specs/**/mission-events.jsonl linguist-generated=true",
    "kitty-specs/**/snapshot-latest.json linguist-generated=true",
    "kitty-specs/**/acceptance-matrix.json linguist-generated=true",
    "kitty-specs/**/occurrence_map.yaml linguist-generated=true",
    "kitty-specs/**/tasks/** linguist-generated=true",
    "kitty-specs/**/research/evidence-log.csv linguist-generated=true",
    "kitty-specs/**/research/source-register.csv linguist-generated=true",
    "kitty-specs/**/test-transcripts/** linguist-generated=true",
    "kitty-specs/**/baseline/** linguist-generated=true",
    "kitty-specs/**/canary-evidence/** linguist-generated=true",
    ".kittify/workspaces/** linguist-generated=true",
    ".kittify/workspaces/** -diff",
    ".kittify/migrations/** linguist-generated=true",
    ".kittify/migrations/** -diff",
];

/// One clone-local git config value the Git owner must apply.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MergeDriverConfigEntry {
    /// The full config key, for example `merge.spec-kitty-meta.name`.
    pub key: String,
    /// The shipped value.
    pub value: String,
    /// Previous values a release shipped; holding one of these is not a user customization.
    pub accepted_previous: Vec<String>,
}

/// The typed wiring plan half that belongs to the Git owner.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MergeDriverWiringPlan {
    /// The project root.
    pub project_root: PathBuf,
    /// Whether the root is a plain git checkout root (`.git` directory).
    pub git_checkout: bool,
    /// Why the clone-local half cannot be applied, when it cannot.
    pub git_skip_reason: Option<String>,
    /// Config entries to install (deduplicated by key, registry order).
    pub config_entries: Vec<MergeDriverConfigEntry>,
    /// `.gitattributes` lines the tracked half needs.
    pub attributes_lines: Vec<String>,
    /// `.gitattributes` lines this project already has.
    pub attributes_present: Vec<String>,
}

impl MergeDriverWiringPlan {
    /// Build the plan from observed files (read-only; no git invocation).
    pub fn plan(project_root: &Path) -> Result<Self, InstallError> {
        let mut config_entries: Vec<MergeDriverConfigEntry> = Vec::new();
        for driver in MERGE_DRIVERS {
            for (suffix, value) in [("name", driver.name), ("driver", driver.command)] {
                let key = format!("merge.{}.{suffix}", driver.config_key);
                if config_entries.iter().any(|entry| entry.key == key) {
                    continue;
                }
                let accepted_previous = if suffix == "name" {
                    driver
                        .accepted_names
                        .iter()
                        .map(|name| (*name).to_string())
                        .collect()
                } else {
                    Vec::new()
                };
                config_entries.push(MergeDriverConfigEntry {
                    key,
                    value: value.to_string(),
                    accepted_previous,
                });
            }
        }
        let dot_git = project_root.join(".git");
        let (git_checkout, git_skip_reason) = match std::fs::symlink_metadata(&dot_git) {
            Ok(metadata) if metadata.file_type().is_dir() => (true, None),
            Ok(metadata) if metadata.file_type().is_file() => (
                false,
                Some(
                    "the project is a linked worktree or submodule (.git is a file); \
                     merge-driver config is common-dir scoped and needs the Git owner"
                        .to_string(),
                ),
            ),
            Ok(_) => (
                false,
                Some(".git is not a directory; not a git checkout root".to_string()),
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (
                false,
                Some("the project is not a git repository yet".to_string()),
            ),
            Err(source) => return Err(InstallError::io("inspect", &dot_git, source)),
        };
        let attributes_lines = merge_attribute_seed_lines();
        let existing = read_attributes(project_root)?;
        let attributes_present = attributes_lines
            .iter()
            .filter(|line| {
                existing
                    .iter()
                    .any(|present| present.trim() == line.as_str())
            })
            .cloned()
            .collect();
        Ok(Self {
            project_root: project_root.to_path_buf(),
            git_checkout,
            git_skip_reason,
            config_entries,
            attributes_lines,
            attributes_present,
        })
    }

    /// Whether the clone-local half still needs a writer.
    pub fn needs_git_config(&self) -> bool {
        self.git_checkout && !self.config_entries.is_empty()
    }
}

/// Ensure every seed attribute line is present, preserving all other content.
///
/// Returns `true` when the file changed.
pub fn ensure_attribute_lines(project_root: &Path) -> Result<bool, InstallError> {
    let path = project_root.join(".gitattributes");
    let path_meta = std::fs::symlink_metadata(&path);
    if let Ok(metadata) = &path_meta
        && (metadata.file_type().is_symlink() || !metadata.file_type().is_file())
    {
        return Err(InstallError::IgnoreFileUnsafe {
            path: path.clone(),
            detail: "attribute file is a symlink or not a regular file".to_string(),
        });
    }
    let existing: Vec<String> = read_attributes(project_root)?;
    let mut required: Vec<String> = merge_attribute_seed_lines();
    required.extend(
        GITHUB_DIFF_ATTRIBUTES
            .iter()
            .map(|line| (*line).to_string()),
    );
    let missing: Vec<String> = required
        .into_iter()
        .filter(|line| !existing.iter().any(|present| present.trim() == line))
        .collect();
    if missing.is_empty() {
        return Ok(false);
    }
    let mut lines = existing;
    lines.extend(missing);
    let text = format!("{}\n", lines.join("\n"));
    sk_kernel::atomic_write(&path, text.as_bytes()).map_err(|error| {
        InstallError::io("write .gitattributes", &path, std::io::Error::other(error))
    })?;
    Ok(true)
}

fn read_attributes(project_root: &Path) -> Result<Vec<String>, InstallError> {
    let path = project_root.join(".gitattributes");
    match std::fs::read(&path) {
        Ok(raw) => {
            let text = String::from_utf8(raw).map_err(|error| InstallError::IgnoreFileUnsafe {
                path: path.clone(),
                detail: error.to_string(),
            })?;
            Ok(text.lines().map(str::to_string).collect())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(source) => Err(InstallError::io("read", &path, source)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_lines_match_the_python_union_exactly() {
        let lines = merge_attribute_seed_lines();
        assert_eq!(lines.len(), 9);
        assert_eq!(
            lines,
            vec![
                "kitty-specs/**/status.events.jsonl merge=spec-kitty-event-log",
                "kitty-specs/**/decisions.events.jsonl merge=spec-kitty-event-log",
                "kitty-specs/**/meta.json merge=spec-kitty-meta",
                "kitty-specs/**/traces/*.md merge=spec-kitty-traces",
                "kitty-specs/**/acceptance-matrix.json merge=spec-kitty-acceptance-matrix",
                "kitty-specs/**/issue-matrix.json merge=spec-kitty-issue-matrix",
                "kitty-specs/**/tasks/*/review-cycle-*.md merge=spec-kitty-review-cycle",
                "kitty-specs/**/decisions/index.json merge=spec-kitty-decision-index",
                "kitty-specs/**/issue-matrix.md merge=spec-kitty-issue-matrix",
            ]
        );
    }

    #[test]
    fn writes_the_seed_once_and_preserves_operator_lines() {
        let directory = tempfile::tempdir().expect("temp");
        let path = directory.path().join(".gitattributes");
        std::fs::write(&path, "*.png binary\n").unwrap();
        assert!(ensure_attribute_lines(directory.path()).expect("write"));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("*.png binary\n"));
        assert!(text.contains("kitty-specs/**/status.events.jsonl merge=spec-kitty-event-log\n"));
        assert!(text.contains(".kittify/migrations/** -diff\n"));
        assert!(!ensure_attribute_lines(directory.path()).expect("no-op"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }

    #[test]
    fn plan_separates_tracked_and_clone_local_halves() {
        let directory = tempfile::tempdir().expect("temp");
        let plan = MergeDriverWiringPlan::plan(directory.path()).expect("plan");
        assert!(!plan.git_checkout);
        assert!(plan.git_skip_reason.is_some());
        assert!(!plan.needs_git_config());
        assert_eq!(plan.config_entries.len(), 14);
        assert!(plan.config_entries.iter().any(|entry| {
            entry.key == "merge.spec-kitty-issue-matrix.name"
                && entry
                    .accepted_previous
                    .contains(&"Spec Kitty issue matrix filled-side merge".to_string())
        }));

        std::fs::create_dir(directory.path().join(".git")).unwrap();
        let plan = MergeDriverWiringPlan::plan(directory.path()).expect("plan");
        assert!(plan.git_checkout);
        assert!(plan.needs_git_config());
        assert_eq!(plan.attributes_present.len(), 0);
    }

    #[test]
    fn refuses_a_symlinked_attribute_file() {
        let directory = tempfile::tempdir().expect("temp");
        let target = directory.path().join("elsewhere");
        std::fs::write(&target, "x\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, directory.path().join(".gitattributes")).unwrap();
        let error = ensure_attribute_lines(directory.path()).expect_err("refused");
        assert_eq!(error.code(), "ignore_file_unsafe");
    }
}
