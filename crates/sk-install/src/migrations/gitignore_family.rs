//! The `.gitignore` entry migrations: backfills and stale-rule removals.
//!
//! One family, two shapes. The **backfills** ensure a frozen list of entries in
//! the project `.gitignore` (they never read the live state contract, so their
//! behaviour stays deterministic as that contract grows); the **removals** drop
//! a stale whole-directory rule that must not be tracked. Each native function
//! mirrors the pinned module's `detect`, `can_apply` and `apply` — including
//! which of them fail closed — and reproduces its `changes_made` wording where
//! the wording says something an operator acts on.
//!
//! Frozen entry lists (never derived at runtime):
//!
//! | migration | entries |
//! | --- | --- |
//! | `0.4.8_gitignore_agents` | the twelve pre-1.0 agent folders, as one commented block |
//! | `2.0.9_state_gitignore` | runtime, merge-state, events and both dossier patterns |
//! | `3.2.0rc35_kittify_runtime_git_hygiene` | sync state, encoding provenance, Op index |
//! | `3.2.3_encoding_provenance_gitignore_backfill` | delegates to the hygiene migration |
//! | `3.2.4_derived_mission_views_gitignore_backfill` | `.kittify/derived/` (+ slash-less form) |
//! | `3.2.4_runtime_dirs_gitignore_backfill` | migrations + logs dirs (+ slash-less forms) |
//! | `3.2.6rc3_lint_report_gitignore_backfill` | `.kittify/lint-report.json` |
//! | `3.2.6rc3_worktrees_gitignore_backfill` | `.worktrees/`, re-asserted after a negation |
//! | `0.12.1_remove_kitty_specs_from_gitignore` | removes whole-`kitty-specs` rules, keeps subpath ones |
//! | `2.0.0a5_remove_wp_status_gitignore_rule` | removes the retired WP-status rules and headings |

use std::path::Path;

use super::ignore::{self, Decode, IgnoreFile, Newlines};
use crate::error::InstallError;
use crate::upgrade::MigrationOutcome;

// ------------------------------------------------------------- 0.4.8 agents

/// The twelve agent folders the pre-1.0 layout ignored wholesale.
const LEGACY_AGENT_DIRECTORIES: [&str; 12] = [
    ".claude/",
    ".codex/",
    ".opencode/",
    ".windsurf/",
    ".gemini/",
    ".cursor/",
    ".qwen/",
    ".kilocode/",
    ".augment/",
    ".roo/",
    ".amazonq/",
    ".github/copilot/",
];

const LEGACY_AGENT_BLOCK: &str = "\n# AI Agent directories (added by Spec Kitty CLI)\n\
# These contain auth tokens and should NEVER be committed\n";

/// The missing entries for `0.4.8`, matched as substrings like the source.
fn missing_legacy_agent_directories(content: &str) -> Vec<&'static str> {
    LEGACY_AGENT_DIRECTORIES
        .iter()
        .copied()
        .filter(|entry| !content.contains(entry))
        .collect()
}

/// The `can_apply` every backfill that only requires an existing project uses.
pub fn can_apply_project_exists(project_root: &Path) -> Result<Option<String>, InstallError> {
    Ok(project_missing(project_root))
}

/// `0.4.8_gitignore_agents.detect`.
pub fn detect_legacy_agent_directories(project_root: &Path) -> Result<bool, InstallError> {
    // The source catches read failures and reports "needs migration", leaving
    // the refusal decision to `can_apply`.
    let content = IgnoreFile::new(project_root)
        .read(Decode::IgnoreInvalid, Newlines::Universal)
        .unwrap_or_default()
        .unwrap_or_default();
    Ok(!missing_legacy_agent_directories(&content).is_empty())
}

/// `0.4.8_gitignore_agents.can_apply`.
pub fn can_apply_legacy_agent_directories(
    project_root: &Path,
) -> Result<Option<String>, InstallError> {
    let file = IgnoreFile::new(project_root);
    if file.path().exists()
        && file
            .read(Decode::IgnoreInvalid, Newlines::Universal)
            .is_err()
    {
        return Ok(Some(".gitignore is not readable".to_string()));
    }
    Ok(None)
}

/// `0.4.8_gitignore_agents.apply`.
pub fn apply_legacy_agent_directories(
    project_root: &Path,
) -> Result<MigrationOutcome, InstallError> {
    let file = IgnoreFile::new(project_root);
    let content = file
        .read(Decode::IgnoreInvalid, Newlines::Universal)?
        .unwrap_or_default();
    let missing = missing_legacy_agent_directories(&content);
    if missing.is_empty() {
        return Ok(MigrationOutcome::unchanged(vec![
            "All agent directories already in .gitignore".to_string(),
        ]));
    }
    let mut block = LEGACY_AGENT_BLOCK.to_string();
    for entry in &missing {
        block.push_str(entry);
        block.push('\n');
    }
    file.append(&block)?;
    Ok(MigrationOutcome::changed(vec![format!(
        "Added {} agent directories to .gitignore",
        missing.len()
    )]))
}

// ------------------------------------------------------ 2.0.9 state surfaces

/// The five runtime state surfaces `2.0.9` backfills.
const RUNTIME_STATE_ENTRIES: [&str; 5] = [
    ".kittify/runtime/",
    ".kittify/merge-state.json",
    ".kittify/events/",
    ".kittify/dossiers/",
    "kitty-specs/**/.kittify/dossiers/",
];

/// `2.0.9_state_gitignore.detect` (substring presence, like the source).
pub fn detect_runtime_state_entries(project_root: &Path) -> Result<bool, InstallError> {
    let content =
        IgnoreFile::new(project_root).read_strict_or_empty(Decode::Strict, Newlines::Universal)?;
    Ok(RUNTIME_STATE_ENTRIES
        .iter()
        .any(|entry| !content.contains(entry)))
}

/// `2.0.9_state_gitignore.apply`.
pub fn apply_runtime_state_entries(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let modified = ignore::ensure_entries(project_root, &RUNTIME_STATE_ENTRIES, false)?;
    if modified {
        return Ok(MigrationOutcome::changed(vec![format!(
            "Added runtime state entries to .gitignore: {}",
            RUNTIME_STATE_ENTRIES.join(", ")
        )]));
    }
    Ok(MigrationOutcome::unchanged(vec![
        "All runtime state entries already present in .gitignore".to_string(),
    ]))
}

// ------------------------------------------ 3.2.0rc35 runtime git hygiene

const HYGIENE_ENTRIES: [&str; 3] = [
    ".kittify/sync-state.json",
    ".kittify/encoding-provenance/",
    "kitty-ops/ops-index.jsonl",
];

/// Machine-local runtime files that must not stay tracked.
const HYGIENE_TRACKED_PATHS: [&str; 3] = [
    ".kittify/charter/context-state.json",
    ".kittify/encoding-provenance/global.jsonl",
    "kitty-ops/ops-index.jsonl",
];

/// `3.2.0rc35_kittify_runtime_git_hygiene.detect`.
pub fn detect_runtime_git_hygiene(project_root: &Path) -> Result<bool, InstallError> {
    let entries = ignore::read_entry_set(project_root, Decode::Strict, Newlines::Universal)?;
    let entry_missing = HYGIENE_ENTRIES
        .iter()
        .any(|entry| !entries.contains(*entry));
    let tracked = HYGIENE_TRACKED_PATHS
        .iter()
        .any(|path| ignore::is_tracked(project_root, path));
    Ok(entry_missing || tracked)
}

/// `3.2.0rc35_kittify_runtime_git_hygiene.can_apply`.
pub fn can_apply_runtime_git_hygiene(project_root: &Path) -> Result<Option<String>, InstallError> {
    Ok(project_missing(project_root))
}

/// `3.2.0rc35_kittify_runtime_git_hygiene.apply`.
pub fn apply_runtime_git_hygiene(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let entries = ignore::read_entry_set(project_root, Decode::Strict, Newlines::Universal)?;
    let missing: Vec<&str> = HYGIENE_ENTRIES
        .iter()
        .copied()
        .filter(|entry| !entries.contains(*entry))
        .collect();
    let modified = ignore::ensure_entries(project_root, &HYGIENE_ENTRIES, false)?;
    let mut changes = Vec::new();
    if modified && !missing.is_empty() {
        changes.push(format!("Added gitignore entries: {}", missing.join(", ")));
    } else if modified {
        changes.push("Updated .gitignore".to_string());
    } else {
        changes.push("gitignore entries already present".to_string());
    }
    for path in HYGIENE_TRACKED_PATHS {
        if !ignore::is_tracked(project_root, path) {
            continue;
        }
        if ignore::untrack(project_root, path) {
            changes.push(format!("Untracked local runtime file: {path}"));
        } else {
            return Err(InstallError::UpgradeRefused {
                detail: format!("Failed to untrack local runtime file: {path}"),
            });
        }
    }
    Ok(MigrationOutcome::changed(changes))
}

// --------------------------------------- 3.2.3 encoding provenance delegate

/// `3.2.3_encoding_provenance_gitignore_backfill` re-runs the hygiene repair
/// verbatim (upstream imports and delegates to it, so this is the same effect,
/// not an approximation).
pub fn detect_encoding_provenance(project_root: &Path) -> Result<bool, InstallError> {
    detect_runtime_git_hygiene(project_root)
}

/// `3.2.3_encoding_provenance_gitignore_backfill.can_apply`.
pub fn can_apply_encoding_provenance(project_root: &Path) -> Result<Option<String>, InstallError> {
    can_apply_runtime_git_hygiene(project_root)
}

/// `3.2.3_encoding_provenance_gitignore_backfill.apply`.
pub fn apply_encoding_provenance(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    apply_runtime_git_hygiene(project_root)
}

// ----------------------------------------- 3.2.4 derived mission views

const DERIVED_VIEWS_ENTRY: &str = ".kittify/derived/";
const DERIVED_VIEWS_EQUIVALENT: [&str; 2] = [".kittify/derived/", ".kittify/derived"];

/// `3.2.4_derived_mission_views_gitignore_backfill.detect`.
pub fn detect_derived_views_entry(project_root: &Path) -> Result<bool, InstallError> {
    let entries = ignore::read_entry_set(project_root, Decode::Strict, Newlines::Universal)?;
    Ok(!present(&entries, &DERIVED_VIEWS_EQUIVALENT))
}

/// `3.2.4_derived_mission_views_gitignore_backfill.apply`.
pub fn apply_derived_views_entry(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let entries = ignore::read_entry_set(project_root, Decode::Strict, Newlines::Universal)?;
    if present(&entries, &DERIVED_VIEWS_EQUIVALENT) {
        return Ok(MigrationOutcome::unchanged(vec![
            "gitignore entry already present".to_string(),
        ]));
    }
    if ignore::ensure_entries(project_root, &[DERIVED_VIEWS_ENTRY], false)? {
        return Ok(MigrationOutcome::changed(vec![format!(
            "Added gitignore entry: {DERIVED_VIEWS_ENTRY}"
        )]));
    }
    Ok(MigrationOutcome::unchanged(vec![
        "gitignore entry already present".to_string(),
    ]))
}

// ------------------------------------------- 3.2.4 runtime dirs backfill

const RUNTIME_DIR_ENTRIES: [&str; 2] = [".kittify/migrations/", ".kittify/logs/"];
const RUNTIME_DIR_EQUIVALENT: [&str; 4] = [
    ".kittify/migrations/",
    ".kittify/migrations",
    ".kittify/logs/",
    ".kittify/logs",
];

/// `3.2.4_runtime_dirs_gitignore_backfill.detect`.
pub fn detect_runtime_dirs_entries(project_root: &Path) -> Result<bool, InstallError> {
    let entries = ignore::read_entry_set(project_root, Decode::Strict, Newlines::Universal)?;
    Ok(RUNTIME_DIR_ENTRIES
        .iter()
        .any(|entry| !present(&entries, &equivalent_for(entry, &RUNTIME_DIR_EQUIVALENT))))
}

/// `3.2.4_runtime_dirs_gitignore_backfill.apply`.
pub fn apply_runtime_dirs_entries(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let entries = ignore::read_entry_set(project_root, Decode::Strict, Newlines::Universal)?;
    let missing: Vec<&str> = RUNTIME_DIR_ENTRIES
        .iter()
        .copied()
        .filter(|entry| !present(&entries, &equivalent_for(entry, &RUNTIME_DIR_EQUIVALENT)))
        .collect();
    if missing.is_empty() {
        return Ok(MigrationOutcome::unchanged(vec![
            "gitignore entries already present".to_string(),
        ]));
    }
    ignore::ensure_entries(project_root, &missing, false)?;
    Ok(MigrationOutcome::changed(vec![format!(
        "Added gitignore entries: {}",
        missing.join(", ")
    )]))
}

// --------------------------------------- 3.2.6rc3 lint report backfill

const LINT_REPORT_ENTRY: &str = ".kittify/lint-report.json";

/// `3.2.6rc3_lint_report_gitignore_backfill.detect`.
///
/// An unreadable/symlinked file reports `true` so `can_apply` turns it into a
/// refusal; the source routes it the same way rather than silently skipping.
pub fn detect_lint_report_entry(project_root: &Path) -> Result<bool, InstallError> {
    match ignore::read_entry_set(project_root, Decode::Strict, Newlines::Preserve) {
        Ok(entries) => Ok(!entries.contains(LINT_REPORT_ENTRY)),
        Err(_) => Ok(true),
    }
}

/// `3.2.6rc3_lint_report_gitignore_backfill.can_apply`.
pub fn can_apply_lint_report_entry(project_root: &Path) -> Result<Option<String>, InstallError> {
    if let Some(reason) = project_missing(project_root) {
        return Ok(Some(reason));
    }
    match ignore::read_entry_set(project_root, Decode::Strict, Newlines::Preserve) {
        Ok(_) => Ok(None),
        Err(error) => Ok(Some(refusal_reason(&error))),
    }
}

/// `3.2.6rc3_lint_report_gitignore_backfill.apply`.
pub fn apply_lint_report_entry(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let entries = ignore::read_entry_set(project_root, Decode::Strict, Newlines::Preserve)?;
    if entries.contains(LINT_REPORT_ENTRY) {
        return Ok(MigrationOutcome::unchanged(vec![
            "gitignore entry already present".to_string(),
        ]));
    }
    ignore::ensure_entries(project_root, &[LINT_REPORT_ENTRY], false)?;
    Ok(MigrationOutcome::changed(vec![format!(
        "Added gitignore entry: {LINT_REPORT_ENTRY}"
    )]))
}

// --------------------------------------- 3.2.6rc3 worktrees backfill

const WORKTREES_ENTRY: &str = ".worktrees/";
const WORKTREES_PROBE: &str = ".worktrees/.spec-kitty-ignore-probe";
const WORKTREES_EQUIVALENT: [&str; 4] =
    [".worktrees", ".worktrees/", "/.worktrees/", "/.worktrees"];

fn worktrees_need_backfill(project_root: &Path) -> Result<bool, InstallError> {
    Ok(!ignore::root_effectively_ignored(
        project_root,
        ".worktrees",
        &WORKTREES_EQUIVALENT,
        WORKTREES_PROBE,
    )?)
}

/// `3.2.6rc3_worktrees_gitignore_backfill.detect`.
pub fn detect_worktrees_entry(project_root: &Path) -> Result<bool, InstallError> {
    match worktrees_need_backfill(project_root) {
        Ok(needed) => Ok(needed),
        Err(_) => Ok(true),
    }
}

/// `3.2.6rc3_worktrees_gitignore_backfill.can_apply`.
pub fn can_apply_worktrees_entry(project_root: &Path) -> Result<Option<String>, InstallError> {
    if let Some(reason) = project_missing(project_root) {
        return Ok(Some(reason));
    }
    match worktrees_need_backfill(project_root) {
        Ok(_) => Ok(None),
        Err(error) => Ok(Some(refusal_reason(&error))),
    }
}

/// `3.2.6rc3_worktrees_gitignore_backfill.apply`.
///
/// The re-assertion uses `force_append`: a later negation can neutralize an
/// earlier canonical rule, so appending at EOF is what restores the guarantee
/// without deleting the operator's own rules.
pub fn apply_worktrees_entry(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    if !worktrees_need_backfill(project_root)? {
        return Ok(MigrationOutcome::unchanged(vec![
            "gitignore entry already present".to_string(),
        ]));
    }
    ignore::ensure_entries(project_root, &[WORKTREES_ENTRY], true)?;
    if worktrees_need_backfill(project_root)? {
        return Err(InstallError::UpgradeRefused {
            detail: ".worktrees/ is still not effectively gitignored after backfill".to_string(),
        });
    }
    Ok(MigrationOutcome::changed(vec![format!(
        "Added gitignore entry: {WORKTREES_ENTRY}"
    )]))
}

// ------------------------------- 0.12.1 remove kitty-specs whole-dir rules

/// The whole-directory rules `0.12.1` removes (kept as exact spellings).
const KITTY_SPECS_BLOCKING: [&str; 4] = [
    "kitty-specs",
    "kitty-specs/",
    "/kitty-specs",
    "/kitty-specs/",
];

fn blocks_kitty_specs(line: &str) -> bool {
    let stripped = line.trim();
    if stripped.is_empty() || stripped.starts_with('#') {
        return false;
    }
    // The removal patterns are anchored whole-line matches, so a worktree-scoped
    // subpath rule (`kitty-specs/**/tasks/*.md`) is never in this set: only the
    // four exhaustive spellings of the whole directory are removed.
    KITTY_SPECS_BLOCKING.contains(&stripped)
}

/// `0.12.1_remove_kitty_specs_from_gitignore.detect`.
///
/// An unsafe/unreadable file reports `true`, the source's fail-closed choice:
/// `can_apply` then refuses it, so the run skips instead of silently treating
/// an unevaluated file as already migrated.
pub fn detect_kitty_specs_blocking_rule(project_root: &Path) -> Result<bool, InstallError> {
    let file = IgnoreFile::new(project_root);
    match file.read(Decode::IgnoreInvalid, Newlines::Universal) {
        Ok(Some(content)) => Ok(content.lines().any(blocks_kitty_specs)),
        Ok(None) => Ok(false),
        Err(_) => Ok(true),
    }
}

/// `0.12.1_remove_kitty_specs_from_gitignore.can_apply`.
///
/// A symlinked `.gitignore` is a refusal here (the source's explicit
/// `_gitignore_symlink_refusal`), not a silent skip.
pub fn can_apply_kitty_specs_blocking_rule(
    project_root: &Path,
) -> Result<Option<String>, InstallError> {
    let file = IgnoreFile::new(project_root);
    if let Ok(metadata) = std::fs::symlink_metadata(file.path())
        && metadata.file_type().is_symlink()
    {
        return Ok(Some(
            ".gitignore is a symbolic link; refusing to follow it".to_string(),
        ));
    }
    if !file.path().exists() {
        return Ok(None);
    }
    match file.read(Decode::IgnoreInvalid, Newlines::Universal) {
        Ok(_) => Ok(None),
        Err(error) => Ok(Some(format!(".gitignore is not readable: {error}"))),
    }
}

/// `0.12.1_remove_kitty_specs_from_gitignore.apply`.
pub fn apply_kitty_specs_blocking_rule(
    project_root: &Path,
) -> Result<MigrationOutcome, InstallError> {
    let file = IgnoreFile::new(project_root);
    if !file.path().exists() {
        return Ok(MigrationOutcome::unchanged(vec![
            "No .gitignore file found".to_string(),
        ]));
    }
    let removed = ignore::remove_lines(&file, blocks_kitty_specs)?;
    if removed.is_empty() {
        return Ok(MigrationOutcome::unchanged(vec![
            "No blocking kitty-specs/ entries found in .gitignore".to_string(),
        ]));
    }
    let mut changes = vec![format!(
        "Removed {} blocking entries from .gitignore",
        removed.len()
    )];
    changes.extend(
        removed
            .iter()
            .map(|line| format!("Line {}: Removed '{}'", line.number, line.text)),
    );
    Ok(MigrationOutcome::changed(changes))
}

// ------------------------------ 2.0.0a5 remove stale WP status rules

/// The retired WP-status rules and headings `2.0.0a5` removes.
const WP_STATUS_RULES: [&str; 2] = ["kitty-specs/**/tasks/*.md", "kitty-specs/*/tasks/*.md"];
const WP_STATUS_HEADINGS: [&str; 2] = [
    "# Block WP status files",
    "# Research artifacts in kitty-specs/**/research/ are allowed",
];

fn is_wp_status_rule(line: &str) -> bool {
    let stripped = line.trim();
    if stripped.is_empty() {
        return false;
    }
    WP_STATUS_RULES.contains(&stripped)
        || WP_STATUS_HEADINGS
            .iter()
            .any(|heading| stripped.starts_with(heading))
}

/// `2.0.0a5_remove_wp_status_gitignore_rule.detect`.
pub fn detect_wp_status_rule(project_root: &Path) -> Result<bool, InstallError> {
    match IgnoreFile::new(project_root).read(Decode::IgnoreInvalid, Newlines::Universal) {
        Ok(Some(content)) => Ok(content.lines().any(is_wp_status_rule)),
        Ok(None) => Ok(false),
        // Fail closed exactly as the source does: `can_apply` refuses it.
        Err(_) => Ok(true),
    }
}

/// `2.0.0a5_remove_wp_status_gitignore_rule.can_apply`.
pub fn can_apply_wp_status_rule(project_root: &Path) -> Result<Option<String>, InstallError> {
    can_apply_kitty_specs_blocking_rule(project_root)
}

/// `2.0.0a5_remove_wp_status_gitignore_rule.apply`.
pub fn apply_wp_status_rule(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let file = IgnoreFile::new(project_root);
    if !file.path().exists() {
        return Ok(MigrationOutcome::unchanged(vec![
            "No .gitignore file found".to_string(),
        ]));
    }
    let removed = ignore::remove_lines(&file, is_wp_status_rule)?;
    if removed.is_empty() {
        return Ok(MigrationOutcome::unchanged(vec![
            "No stale WP status ignore entries found in .gitignore".to_string(),
        ]));
    }
    let mut changes = vec![format!(
        "Removed {} stale WP status ignore entries from .gitignore",
        removed.len()
    )];
    changes.extend(
        removed
            .iter()
            .map(|line| format!("Line {}: Removed '{}'", line.number, line.text)),
    );
    Ok(MigrationOutcome::changed(changes))
}

// ---------------------------------------------------------------- helpers

/// The bare message an operator sees for a guard refusal.
///
/// Upstream turns a `GitignorePathError` into `str(exc)`, which is the message
/// alone. `InstallError`'s `Display` adds its own prefix, so a `can_apply`
/// refusal must unwrap the variants that model that error rather than print
/// the whole error.
fn refusal_reason(error: &InstallError) -> String {
    match error {
        InstallError::UpgradeRefused { detail } => detail.clone(),
        InstallError::IgnoreFileUnsafe { detail, .. } => detail.clone(),
        other => other.to_string(),
    }
}

fn project_missing(project_root: &Path) -> Option<String> {
    if project_root.exists() {
        None
    } else {
        Some(format!(
            "Project path does not exist: {}",
            project_root.display()
        ))
    }
}

fn present(entries: &std::collections::BTreeSet<String>, equivalents: &[&str]) -> bool {
    equivalents.iter().any(|entry| entries.contains(*entry))
}

fn equivalent_for<'a>(entry: &str, pool: &'a [&'a str]) -> Vec<&'a str> {
    let slashless = entry.trim_end_matches('/');
    pool.iter()
        .copied()
        .filter(|candidate| *candidate == entry || *candidate == slashless)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> tempfile::TempDir {
        tempfile::tempdir().expect("temp")
    }

    #[test]
    fn legacy_agent_block_is_appended_once_with_its_own_comment() {
        let directory = project();
        let root = directory.path();
        std::fs::write(root.join(".gitignore"), "node_modules/").unwrap();
        assert!(detect_legacy_agent_directories(root).unwrap());
        let outcome = apply_legacy_agent_directories(root).unwrap();
        assert!(outcome.changed);
        let text = std::fs::read_to_string(root.join(".gitignore")).unwrap();
        assert!(text.starts_with("node_modules/\n\n# AI Agent directories"));
        assert!(text.contains("# These contain auth tokens"));
        assert!(text.ends_with(".github/copilot/\n"));
        assert!(!detect_legacy_agent_directories(root).unwrap());
    }

    #[test]
    fn state_entries_use_the_managed_marker() {
        let directory = project();
        let root = directory.path();
        assert!(detect_runtime_state_entries(root).unwrap());
        let outcome = apply_runtime_state_entries(root).unwrap();
        assert!(outcome.changed);
        let text = std::fs::read_to_string(root.join(".gitignore")).unwrap();
        assert!(text.starts_with(ignore::MARKER));
        for entry in RUNTIME_STATE_ENTRIES {
            assert!(text.lines().any(|line| line == entry), "{entry}");
        }
        assert!(!detect_runtime_state_entries(root).unwrap());
    }

    #[test]
    fn runtime_dirs_accepts_the_slash_less_variant() {
        let directory = project();
        let root = directory.path();
        std::fs::write(
            root.join(".gitignore"),
            ".kittify/migrations\n.kittify/logs/\n",
        )
        .unwrap();
        assert!(!detect_runtime_dirs_entries(root).unwrap());
        let outcome = apply_runtime_dirs_entries(root).unwrap();
        assert!(!outcome.changed);
        assert_eq!(
            std::fs::read_to_string(root.join(".gitignore")).unwrap(),
            ".kittify/migrations\n.kittify/logs/\n"
        );
    }

    #[test]
    fn worktrees_reassertion_survives_a_later_negation() {
        let directory = project();
        let root = directory.path();
        std::fs::write(root.join(".gitignore"), ".worktrees/\n!.worktrees/keep/\n").unwrap();
        assert!(detect_worktrees_entry(root).unwrap());
        let outcome = apply_worktrees_entry(root).unwrap();
        assert!(outcome.changed);
        let text = std::fs::read_to_string(root.join(".gitignore")).unwrap();
        assert!(text.ends_with(&format!("{WORKTREES_ENTRY}\n")));
        assert!(text.contains("!.worktrees/keep/"));
        assert!(!detect_worktrees_entry(root).unwrap());
    }

    #[test]
    fn kitty_specs_keeps_the_subpath_rules() {
        let directory = project();
        let root = directory.path();
        std::fs::write(
            root.join(".gitignore"),
            "kitty-specs/\nkitty-specs/**/tasks/*.md\nnode_modules/\n",
        )
        .unwrap();
        assert!(detect_kitty_specs_blocking_rule(root).unwrap());
        let outcome = apply_kitty_specs_blocking_rule(root).unwrap();
        assert!(outcome.changed);
        assert_eq!(
            std::fs::read_to_string(root.join(".gitignore")).unwrap(),
            "kitty-specs/**/tasks/*.md\nnode_modules/\n"
        );
        assert!(!detect_kitty_specs_blocking_rule(root).unwrap());
    }

    #[test]
    fn wp_status_rules_and_headings_are_removed_together() {
        let directory = project();
        let root = directory.path();
        std::fs::write(
            root.join(".gitignore"),
            "# Block WP status files during merges\nkitty-specs/**/tasks/*.md\nkeep/\n\n\n\n",
        )
        .unwrap();
        assert!(detect_wp_status_rule(root).unwrap());
        let outcome = apply_wp_status_rule(root).unwrap();
        assert!(outcome.changed);
        // The source collapses a run of >= 3 newlines to exactly two.
        assert_eq!(
            std::fs::read_to_string(root.join(".gitignore")).unwrap(),
            "keep/\n\n"
        );
        assert!(!detect_wp_status_rule(root).unwrap());
    }

    #[test]
    fn a_missing_project_is_a_can_apply_refusal() {
        let directory = project();
        let missing = directory.path().join("absent");
        assert!(
            can_apply_lint_report_entry(&missing)
                .unwrap()
                .is_some_and(|reason| reason.contains("does not exist"))
        );
    }
}
