//! Retirement migrations: removals the package performs on surfaces it shipped.
//!
//! Every function here mirrors one pinned module's `detect` / `can_apply` /
//! `apply`, and routes every destructive step through
//! [`crate::preserve::guard_destructive_removal`] so a retired *name* is never
//! treated as ownership proof:
//!
//! | Migration | Candidates | Proof |
//! | --- | --- | --- |
//! | `2.0.0_retire_git_hooks` | known Spec Kitty hook files under `.git/hooks` | a shipped signature set in the file (the source's own predicate) |
//! | `2.0.11_remove_clarify_command` | `.kittify/` command templates + every known agent command dir | command version marker |
//! | `2.1.2_remove_release_skill` | `.claude/skills/release/` | managed-skills manifest entry, archived when unproven |
//! | `3.2.0rc43_retire_profile_context_command` | configured agent command dirs | command version marker |
//! | `3.2.0rc45_retire_standalone_skill_surface` | every installable skill root × retired skill name | manifest entry, plus preserve-aware manifest pruning |
//! | `4.0.0rc5_retire_bundled_dashboard` | agent command dirs + two runtime files | marker / declared runtime path |
//! | `0_9_4_roo_deprecation` | `config.yaml` `agents.available` member, `.roo/` notice | n/a (config edit; directory preserved) |
//! | `4_0_0_retired_hosted_target` | machine `config.toml` `sync.server_url` | exact retired host, machine-scoped |
//!
//! Deliberate, documented deviation (bundled dashboard): upstream terminates a
//! dashboard server it can prove is running from the recorded PID. Native code
//! does not signal processes (the workspace forbids guessing at process
//! identity and no `psutil` equivalent exists here), so a *provably live*
//! dashboard server keeps `.kittify/.dashboard` with a warning naming its PID
//! and port — the source's own "could not stop it" branch, never a silent
//! orphaned file.

use std::path::{Path, PathBuf};

use crate::error::InstallError;
use crate::preserve::{Prover, RemovalVerdict, guard_destructive_removal};
use crate::skills::{ManagedSkillManifest, SKILLS_MANIFEST_REL};
use crate::upgrade::MigrationOutcome;

// ------------------------------------------------------- 2.0.0 git hooks

/// Hook names the pre-2.0 package installed.
pub const MANAGED_HOOK_NAMES: [&str; 5] = [
    "pre-commit",
    "commit-msg",
    "pre-commit-agent-check",
    "pre-commit-encoding-check",
    "pre-commit-markdown-check",
];

/// Signature sets that prove a hook file was written by Spec Kitty.
///
/// A file matches a hook when **all** markers of one of its signature tuples are
/// present; a custom hook that merely shares a name never matches.
pub fn managed_hook_signatures(hook_name: &str) -> &'static [&'static [&'static str]] {
    match hook_name {
        "pre-commit" => &[
            &["SPEC_KITTY_MANAGED_HOOK_SHIM=1"],
            &[
                "Main pre-commit hook that orchestrates all pre-commit checks",
                "pre-commit-encoding-check",
                "pre-commit-agent-check",
            ],
        ],
        "commit-msg" => &[
            &["SPEC_KITTY_MANAGED_HOOK_SHIM=1"],
            &[
                "commit-msg hook to enforce conventional commit messages via commitlint",
                "feat(doctrine): add markdown and commit quality gates",
            ],
        ],
        "pre-commit-agent-check" => &[&[
            "Pre-commit hook to prevent committing agent directories",
            ".github/copilot",
        ]],
        "pre-commit-encoding-check" => &[&[
            "Pre-commit hook to validate UTF-8 encoding in markdown files",
            "spec-kitty validate-encoding --all --fix",
        ]],
        "pre-commit-markdown-check" => &[&[
            "Pre-commit hook to validate markdown style on staged files",
            "markdownlint-cli2",
            "SPEC_KITTY_TEST_MODE",
        ]],
        _ => &[],
    }
}

/// Whether `path` matches a known Spec Kitty-managed hook signature.
pub fn is_managed_hook(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let Some(name) = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
    else {
        return false;
    };
    let signatures = managed_hook_signatures(&name);
    if signatures.is_empty() {
        return false;
    }
    // The pinned source reads with `errors="ignore"`, so invalid UTF-8 drops out
    // instead of failing the probe.
    let content = String::from_utf8_lossy(&std::fs::read(path).unwrap_or_default()).into_owned();
    signatures
        .iter()
        .any(|signature| signature.iter().all(|marker| content.contains(marker)))
}

/// `2.0.0_retire_git_hooks.detect`.
pub fn detect_retire_git_hooks(project_root: &Path) -> Result<bool, InstallError> {
    let hooks = project_root.join(".git/hooks");
    if !hooks.is_dir() {
        return Ok(false);
    }
    Ok(MANAGED_HOOK_NAMES
        .iter()
        .any(|name| is_managed_hook(&hooks.join(name))))
}

/// `2.0.0_retire_git_hooks.can_apply`.
pub fn can_apply_retire_git_hooks(project_root: &Path) -> Result<Option<String>, InstallError> {
    if project_root.exists() {
        Ok(None)
    } else {
        Ok(Some("Project path does not exist".to_string()))
    }
}

/// `2.0.0_retire_git_hooks.apply`.
pub fn apply_retire_git_hooks(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let hooks = project_root.join(".git/hooks");
    if !hooks.is_dir() {
        return Ok(MigrationOutcome::unchanged(vec![
            "No .git/hooks directory found".to_string(),
        ]));
    }
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    let mut removed = 0usize;
    for name in MANAGED_HOOK_NAMES {
        let path = hooks.join(name);
        if !path.exists() {
            continue;
        }
        if !is_managed_hook(&path) {
            warnings.push(format!("Skipped custom hook: {name}"));
            continue;
        }
        std::fs::remove_file(&path)
            .map_err(|error| InstallError::io("remove hook", &path, error))?;
        removed += 1;
        changes.push(format!("Removed managed hook: {name}"));
    }
    if changes.is_empty() {
        changes.push("No managed git hooks found".to_string());
    }
    if removed > 0 {
        changes.insert(0, format!("Removed {removed} managed git hook(s)"));
    }
    Ok(MigrationOutcome {
        changed: removed > 0,
        changes,
        warnings,
        ..crate::upgrade::MigrationOutcome::default()
    })
}

// -------------------------------------------------- agent directory helpers

/// One project-relative command directory and the agent key that owns it.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct AgentDirectory {
    /// Project-relative directory, e.g. `.claude/commands` (no trailing slash).
    pub directory: String,
    /// The configuration key that names this surface.
    pub agent_key: String,
}

/// Every command directory the pinned `AGENT_DIRS` roster knows, in registry order.
pub fn known_agent_directories() -> Vec<AgentDirectory> {
    crate::agent::agents()
        .iter()
        .filter_map(|surface| {
            surface.command.map(|command| AgentDirectory {
                directory: command.directory.trim_end_matches('/').to_string(),
                agent_key: surface.key.to_string(),
            })
        })
        .collect()
}

/// The configured agent command directories for `project_root`.
///
/// Mirrors `get_agent_dirs_for_project`: an unreadable or agent-less
/// configuration yields **every** known directory (the source's explicit-list
/// fallback), otherwise only the configured agents' directories.
pub fn configured_agent_directories(project_root: &Path) -> Vec<AgentDirectory> {
    let known = known_agent_directories();
    let configured = match crate::init::read_configured_agents(project_root) {
        Ok(agents) if !agents.is_empty() => agents,
        // An unreadable or agent-less configuration is the source's
        // explicit-list fallback: every known directory is processed.
        _ => return known,
    };
    let selected: Vec<AgentDirectory> = known
        .iter()
        .filter(|directory| configured.iter().any(|agent| agent == &directory.agent_key))
        .cloned()
        .collect();
    if selected.is_empty() { known } else { selected }
}

/// The installable (non-wrapper) skill roots for the configured agents, sorted
/// and de-duplicated, without a trailing slash.
pub fn configured_skill_roots(project_root: &Path) -> Vec<String> {
    let configured = match crate::init::read_configured_agents(project_root) {
        Ok(agents) if !agents.is_empty() => agents,
        _ => crate::agent::agents()
            .iter()
            .map(|surface| surface.key.to_string())
            .collect(),
    };
    let mut roots: Vec<String> = Vec::new();
    for surface in crate::agent::agents() {
        if surface.skill.class == crate::agent::SkillClass::WrapperOnly {
            continue;
        }
        if !configured.iter().any(|agent| agent == surface.key) {
            continue;
        }
        for root in surface.skill.roots {
            let root = root.trim_end_matches('/').to_string();
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
    }
    roots.sort();
    roots
}

fn relative(root: &Path, path: &Path) -> String {
    crate::preserve::relative_display(path, root)
}

fn sorted_by_relative(root: &Path, mut paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths.sort_by_key(|path| relative(root, path));
    paths
}

fn dedupe(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen: Vec<PathBuf> = Vec::new();
    for path in paths {
        let resolved = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if !seen.contains(&resolved) {
            seen.push(resolved);
        }
    }
    seen
}

fn expand(pattern_root: &Path, pattern: &str) -> Vec<PathBuf> {
    // `*` is the only wildcard the retired-template scans use (`.merged-*` and
    // `*/command-templates/clarify.md`).
    let segments: Vec<&str> = pattern.split('/').collect();
    let mut current = vec![pattern_root.to_path_buf()];
    for segment in segments {
        let mut next = Vec::new();
        for base in current {
            if let Some(prefix) = segment.strip_suffix('*') {
                if let Ok(entries) = std::fs::read_dir(&base) {
                    let mut names: Vec<PathBuf> = entries
                        .flatten()
                        .map(|entry| entry.path())
                        .filter(|path| {
                            path.file_name()
                                .map(|name| name.to_string_lossy().starts_with(prefix))
                                .unwrap_or(false)
                        })
                        .collect();
                    names.sort();
                    next.extend(names);
                }
            } else {
                next.push(base.join(segment));
            }
        }
        current = next;
    }
    current
}

// ------------------------------------------- 2.0.11 remove clarify command

/// Every on-disk clarify artifact the source would consider.
pub fn clarify_command_targets(project_root: &Path) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    let templates = project_root.join(".kittify/templates");
    let missions = project_root.join(".kittify/missions");
    candidates.push(templates.join("command-templates/clarify.md"));
    if templates.exists() {
        candidates.extend(expand(&templates, ".merged-*/clarify.md"));
    }
    if missions.exists() {
        candidates.extend(expand(&missions, "*/command-templates/clarify.md"));
    }
    for directory in known_agent_directories() {
        let agent_dir = project_root.join(&directory.directory);
        if !agent_dir.is_dir() {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(&agent_dir) {
            let mut matches: Vec<PathBuf> = entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.is_file()
                        && path
                            .file_name()
                            .map(|name| name.to_string_lossy().starts_with("spec-kitty.clarify"))
                            .unwrap_or(false)
                })
                .collect();
            matches.sort();
            candidates.extend(matches);
        }
    }
    let existing: Vec<PathBuf> = candidates
        .into_iter()
        .filter(|path| path.exists())
        .collect();
    sorted_by_relative(project_root, dedupe(existing))
}

/// `2.0.11_remove_clarify_command.detect`.
pub fn detect_remove_clarify_command(project_root: &Path) -> Result<bool, InstallError> {
    Ok(!clarify_command_targets(project_root).is_empty())
}

/// `2.0.11_remove_clarify_command.apply`.
pub fn apply_remove_clarify_command(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let targets = clarify_command_targets(project_root);
    if targets.is_empty() {
        return Ok(MigrationOutcome::unchanged(vec![
            "No clarify command artifacts found".to_string(),
        ]));
    }
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    let mut preserved = 0usize;
    let mut removed = 0usize;
    for target in &targets {
        let verdict = guard_destructive_removal(
            target,
            project_root,
            &Prover::CanonicalContent,
            false,
            None,
            false,
        )?;
        match verdict {
            RemovalVerdict::Owned { .. } => {
                removed += 1;
                changes.push(format!("Removed: {}", relative(project_root, target)));
            }
            RemovalVerdict::Absent => {}
            RemovalVerdict::Preserved { diagnostic, .. }
            | RemovalVerdict::Archived { diagnostic, .. } => {
                preserved += 1;
                warnings.push(diagnostic);
            }
        }
    }
    changes.push(if preserved == 0 {
        format!("Removed {removed} clarify command artifacts")
    } else {
        format!("Removed {removed} clarify command artifacts, preserved {preserved}")
    });
    Ok(MigrationOutcome {
        changed: removed > 0,
        changes,
        warnings,
        ..crate::upgrade::MigrationOutcome::default()
    })
}

// ------------------------------------------------ 2.1.2 remove release skill

/// The release skill the package shipped by mistake (`2.0.0`..`2.1.1`).
pub const RELEASE_SKILL_REL: &str = ".claude/skills/release";

/// `2.1.2_remove_release_skill.detect`.
pub fn detect_remove_release_skill(project_root: &Path) -> Result<bool, InstallError> {
    Ok(project_root.join(RELEASE_SKILL_REL).is_dir())
}

/// `2.1.2_remove_release_skill.can_apply`.
pub fn can_apply_remove_release_skill(project_root: &Path) -> Result<Option<String>, InstallError> {
    if project_root.join(RELEASE_SKILL_REL).is_dir() {
        Ok(None)
    } else {
        Ok(Some(
            "No release skill found — nothing to remove".to_string(),
        ))
    }
}

/// `2.1.2_remove_release_skill.apply`.
pub fn apply_remove_release_skill(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let skill = project_root.join(RELEASE_SKILL_REL);
    if !skill.is_dir() {
        return Ok(MigrationOutcome::unchanged(vec![
            "No release skill found — nothing to do".to_string(),
        ]));
    }
    let verdict = guard_destructive_removal(
        &skill,
        project_root,
        &Prover::Manifest,
        true,
        Some(&project_root.join(".kittify")),
        false,
    )?;
    Ok(match verdict {
        RemovalVerdict::Owned { .. } => {
            MigrationOutcome::changed(vec![format!("Removed {RELEASE_SKILL_REL}/")])
        }
        RemovalVerdict::Absent => {
            MigrationOutcome::unchanged(vec![format!("{RELEASE_SKILL_REL}/ is absent")])
        }
        RemovalVerdict::Preserved { diagnostic, .. }
        | RemovalVerdict::Archived { diagnostic, .. } => MigrationOutcome {
            changed: true,
            changes: vec![diagnostic.clone()],
            warnings: vec![diagnostic],
            ..crate::upgrade::MigrationOutcome::default()
        },
    })
}

// -------------------------------------- 3.2.0rc43 profile-context command

/// Filename of the retired `profile-context` command.
pub const PROFILE_CONTEXT_FILENAME: &str = "spec-kitty.profile-context.md";

/// Configured agent dirs that still hold the retired command file.
pub fn profile_context_targets(project_root: &Path) -> Vec<PathBuf> {
    let mut targets = Vec::new();
    for directory in configured_agent_directories(project_root) {
        let candidate = project_root
            .join(&directory.directory)
            .join(PROFILE_CONTEXT_FILENAME);
        if candidate.exists() {
            targets.push(candidate);
        }
    }
    targets
}

/// `3.2.0rc43_retire_profile_context_command.detect`.
pub fn detect_retire_profile_context_command(project_root: &Path) -> Result<bool, InstallError> {
    Ok(!profile_context_targets(project_root).is_empty())
}

/// `3.2.0rc43_retire_profile_context_command.apply`.
pub fn apply_retire_profile_context_command(
    project_root: &Path,
) -> Result<MigrationOutcome, InstallError> {
    let targets = profile_context_targets(project_root);
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    let mut removed = 0usize;
    for target in &targets {
        let verdict = guard_destructive_removal(
            target,
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
                    relative(project_root, target)
                ));
            }
            RemovalVerdict::Absent => {}
            RemovalVerdict::Preserved { diagnostic, .. }
            | RemovalVerdict::Archived { diagnostic, .. } => warnings.push(diagnostic),
        }
    }
    if changes.is_empty() && warnings.is_empty() {
        changes.push(format!(
            "{PROFILE_CONTEXT_FILENAME} absent from all configured agent dirs"
        ));
    }
    Ok(MigrationOutcome {
        changed: removed > 0,
        changes,
        warnings,
        ..crate::upgrade::MigrationOutcome::default()
    })
}

// ------------------------------- 3.2.0rc45 retire standalone skill surface

/// Standalone governance skill packages the package no longer ships.
pub const RETIRED_STANDALONE_SKILL_NAMES: [&str; 1] = ["spec-kitty.advise"];

fn path_contains_retired_skill(path: &str) -> bool {
    path.split('/')
        .any(|part| RETIRED_STANDALONE_SKILL_NAMES.contains(&part))
}

/// Existing retired-skill directories across the configured skill roots.
pub fn retired_skill_targets(project_root: &Path) -> Vec<PathBuf> {
    let mut targets = Vec::new();
    for root in configured_skill_roots(project_root) {
        for name in RETIRED_STANDALONE_SKILL_NAMES {
            let candidate = project_root.join(&root).join(name);
            if candidate.exists() || candidate.symlink_metadata().is_ok() {
                targets.push(candidate);
            }
        }
    }
    targets
}

fn managed_manifest_has_retired_entries(project_root: &Path) -> bool {
    ManagedSkillManifest::load(project_root).is_some_and(|manifest| {
        manifest.entries.iter().any(|entry| {
            RETIRED_STANDALONE_SKILL_NAMES.contains(&entry.skill_name.as_str())
                || path_contains_retired_skill(&entry.installed_path)
        })
    })
}

fn command_manifest_has_retired_entries(project_root: &Path) -> bool {
    crate::manifest::AssetManifest::load(project_root)
        .ok()
        .flatten()
        .is_some_and(|manifest| {
            manifest
                .entries()
                .iter()
                .any(|entry| path_contains_retired_skill(&entry.path))
        })
}

/// `3.2.0rc45_retire_standalone_skill_surface.detect`.
pub fn detect_retire_standalone_skill_surface(project_root: &Path) -> Result<bool, InstallError> {
    Ok(!retired_skill_targets(project_root).is_empty()
        || managed_manifest_has_retired_entries(project_root)
        || command_manifest_has_retired_entries(project_root))
}

/// `3.2.0rc45_retire_standalone_skill_surface.apply`.
pub fn apply_retire_standalone_skill_surface(
    project_root: &Path,
) -> Result<MigrationOutcome, InstallError> {
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let mut mutated = false;

    for target in retired_skill_targets(project_root) {
        let is_tree = target.is_dir() && !target.is_symlink();
        let verdict = guard_destructive_removal(
            &target,
            project_root,
            &Prover::Manifest,
            is_tree,
            None,
            false,
        )?;
        match verdict {
            RemovalVerdict::Owned { .. } => {
                mutated = true;
                changes.push(format!(
                    "Removed retired skill surface {}",
                    relative(project_root, &target)
                ));
            }
            RemovalVerdict::Absent => {}
            RemovalVerdict::Preserved { diagnostic, .. }
            | RemovalVerdict::Archived { diagnostic, .. } => warnings.push(diagnostic),
        }
    }

    // Preserve-aware pruning: only a *gone* file's entry is pruned, so a
    // preserved user collision keeps its manifest record.
    if let Some(mut manifest) = ManagedSkillManifest::load(project_root) {
        let retired: Vec<&crate::skills::ManagedFileEntry> = manifest
            .entries
            .iter()
            .filter(|entry| {
                (RETIRED_STANDALONE_SKILL_NAMES.contains(&entry.skill_name.as_str())
                    || path_contains_retired_skill(&entry.installed_path))
                    && !project_root.join(&entry.installed_path).exists()
            })
            .collect();
        if !retired.is_empty() {
            let removed: Vec<String> = retired
                .iter()
                .map(|entry| entry.installed_path.clone())
                .collect();
            manifest.entries.retain(|entry| {
                !(RETIRED_STANDALONE_SKILL_NAMES.contains(&entry.skill_name.as_str())
                    || path_contains_retired_skill(&entry.installed_path))
                    || project_root.join(&entry.installed_path).exists()
            });
            match manifest.save(project_root) {
                Ok(_) => {
                    mutated = true;
                    for path in removed {
                        changes.push(format!("Pruned retired skill manifest entry {path}"));
                    }
                }
                Err(error) => {
                    errors.push(format!("Failed to update {SKILLS_MANIFEST_REL}: {error}"))
                }
            }
        }
    }

    if let Ok(Some(mut manifest)) = crate::manifest::AssetManifest::load(project_root) {
        let removed: Vec<String> = manifest
            .entries()
            .iter()
            .filter(|entry| {
                path_contains_retired_skill(&entry.path) && !project_root.join(&entry.path).exists()
            })
            .map(|entry| entry.path.clone())
            .collect();
        if !removed.is_empty() {
            for path in &removed {
                manifest.remove_path(path);
            }
            let target = project_root.join(crate::manifest::MANIFEST_REL);
            match sk_kernel::atomic_write(&target, &manifest.to_bytes()) {
                Ok(_) => {
                    mutated = true;
                    for path in removed {
                        changes.push(format!(
                            "Pruned retired command skills manifest entry {path}"
                        ));
                    }
                }
                Err(error) => {
                    warnings.push(format!("Could not prune command skills manifest: {error}"))
                }
            }
        }
    }

    if changes.is_empty() && warnings.is_empty() && errors.is_empty() {
        changes.push("Retired standalone governance skill surfaces absent".to_string());
    }
    Ok(MigrationOutcome {
        changed: mutated,
        changes,
        warnings: warnings.into_iter().chain(errors).collect(),
        ..crate::upgrade::MigrationOutcome::default()
    })
}

// -------------------------------------- 4.0.0rc5 retire bundled dashboard

/// Retired dashboard command filenames (all agent families).
pub const DASHBOARD_COMMAND_FILENAMES: [&str; 3] = [
    "spec-kitty.dashboard.md",
    "spec-kitty.dashboard.prompt.md",
    "spec-kitty.dashboard.toml",
];

/// The dashboard's daemon metadata file.
pub const DASHBOARD_META_REL: &str = ".kittify/.dashboard";
/// The persisted preflight banner the bundled dashboard wrote.
pub const DASHBOARD_PREFLIGHT_REL: &str = ".kittify/preflight-warning.json";

fn dashboard_prover() -> Prover {
    Prover::Any(vec![
        Prover::CanonicalContent,
        Prover::ManagedPath(vec![
            DASHBOARD_META_REL.to_string(),
            DASHBOARD_PREFLIGHT_REL.to_string(),
        ]),
    ])
}

/// Every retired dashboard candidate that exists on disk.
pub fn dashboard_targets(project_root: &Path) -> Vec<PathBuf> {
    let mut targets = Vec::new();
    for directory in configured_agent_directories(project_root) {
        let agent_dir = project_root.join(&directory.directory);
        if !agent_dir.is_dir() {
            continue;
        }
        for name in DASHBOARD_COMMAND_FILENAMES {
            let candidate = agent_dir.join(name);
            if candidate.is_file() {
                targets.push(candidate);
            }
        }
    }
    for rel in [DASHBOARD_META_REL, DASHBOARD_PREFLIGHT_REL] {
        let candidate = project_root.join(rel);
        if candidate.is_file() {
            targets.push(candidate);
        }
    }
    targets
}

/// The dashboard PID recorded on line 4 of `.kittify/.dashboard`.
pub fn recorded_dashboard_pid(meta_path: &Path) -> Option<u32> {
    let text = std::fs::read_to_string(meta_path).ok()?;
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    lines.get(3)?.parse().ok()
}

fn recorded_dashboard_port(meta_path: &Path) -> String {
    std::fs::read_to_string(meta_path)
        .ok()
        .and_then(|text| {
            let lines: Vec<String> = text
                .lines()
                .map(|line| line.trim().to_string())
                .filter(|line| !line.is_empty())
                .collect();
            lines.get(1).cloned()
        })
        .unwrap_or_else(|| "unknown".to_string())
}

/// Whether the recorded PID really is the retired dashboard server.
///
/// Linux reads `/proc/<pid>/cmdline`; other platforms cannot prove the process
/// identity without a psutil equivalent, so they answer `false` (the metadata
/// file is then removable, exactly as when the PID belongs to another program).
fn dashboard_server_running(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
        let joined = String::from_utf8_lossy(&cmdline).replace('\0', " ");
        joined.contains("specify_cli.dashboard")
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        false
    }
}

/// `4.0.0rc5_retire_bundled_dashboard.detect`.
///
/// Only a candidate the package can prove it owns counts, so a preserved user
/// file never re-triggers the migration.
pub fn detect_retire_bundled_dashboard(project_root: &Path) -> Result<bool, InstallError> {
    let prover = dashboard_prover();
    Ok(dashboard_targets(project_root)
        .iter()
        .any(|path| prover.prove(path, project_root).is_some()))
}

/// `4.0.0rc5_retire_bundled_dashboard.apply`.
pub fn apply_retire_bundled_dashboard(
    project_root: &Path,
) -> Result<MigrationOutcome, InstallError> {
    let prover = dashboard_prover();
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    let mut preserved = 0usize;
    let mut removed = 0usize;

    for path in dashboard_targets(project_root) {
        let rel = relative(project_root, &path);
        if rel == DASHBOARD_META_REL
            && let Some(pid) = recorded_dashboard_pid(&path)
            && dashboard_server_running(pid)
        {
            warnings.push(format!(
                "A dashboard server from an older CLI may still be running (PID {pid}, port {}); \
                 stop it by hand. Kept {DASHBOARD_META_REL} as its only record.",
                recorded_dashboard_port(&path)
            ));
            preserved += 1;
            continue;
        }
        match guard_destructive_removal(&path, project_root, &prover, false, None, false)? {
            RemovalVerdict::Owned { .. } => {
                removed += 1;
                changes.push(format!("Removed retired {rel}"));
            }
            RemovalVerdict::Absent => {}
            RemovalVerdict::Preserved { diagnostic, .. }
            | RemovalVerdict::Archived { diagnostic, .. } => {
                warnings.push(diagnostic);
                preserved += 1;
            }
        }
    }

    if removed == 0 && preserved == 0 {
        changes.push("No retired dashboard files present".to_string());
    }
    Ok(MigrationOutcome {
        changed: removed > 0,
        changes,
        warnings,
        ..crate::upgrade::MigrationOutcome::default()
    })
}

// ------------------------------------------------ 0.9.4 Roo deprecation

/// The retired Roo Code project directory (never deleted).
pub const ROO_DIR: &str = ".roo";
/// The retired Roo Code agent key.
pub const ROO_AGENT_KEY: &str = "roo";
/// Roo Code's shutdown date.
pub const ROO_DEPRECATION_DATE: &str = "2026-05-15";

/// The raw `agents.available` (or legacy `tools.available`) member list.
///
/// Read straight from the YAML without validating agent keys (the source reads
/// the file with its own YAML reader for exactly this reason): a stale `roo`
/// member must still be visible to the migration that retires it, even though
/// `roo` is no longer a valid agent key.
pub fn configured_available_agents(project_root: &Path) -> Vec<String> {
    let path = project_root.join(".kittify/config.yaml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(value) = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&text) else {
        return Vec::new();
    };
    let Some(mapping) = value.as_mapping() else {
        return Vec::new();
    };
    let agents = mapping
        .get(serde_yaml_ng::Value::from("agents"))
        .or_else(|| mapping.get(serde_yaml_ng::Value::from("tools")));
    agents
        .and_then(|agents| agents.as_mapping())
        .and_then(|agents| agents.get(serde_yaml_ng::Value::from("available")))
        .and_then(|available| available.as_sequence())
        .map(|members| {
            members
                .iter()
                .filter_map(|member| member.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `roo` is still listed in `.kittify/config.yaml`.
pub fn roo_in_config(project_root: &Path) -> bool {
    configured_available_agents(project_root)
        .iter()
        .any(|agent| agent == ROO_AGENT_KEY)
}

/// `0_9_4_roo_deprecation.detect`.
pub fn detect_roo_deprecation(project_root: &Path) -> Result<bool, InstallError> {
    Ok(project_root.join(ROO_DIR).exists() || roo_in_config(project_root))
}

/// `0_9_4_roo_deprecation.apply`.
pub fn apply_roo_deprecation(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    if project_root.join(ROO_DIR).exists() {
        changes.push(format!(
            "Roo Code deprecation notice emitted ({ROO_DIR}/ preserved at {})",
            project_root.join(ROO_DIR).display()
        ));
        eprintln!(
            "Roo Code has been discontinued as of {ROO_DEPRECATION_DATE}.\n\
             The {ROO_DIR}/ directory has been preserved but Roo Code will no longer be maintained \
             or generated by spec-kitty.\n\
             You may safely delete {ROO_DIR}/ if you are not using Roo Code.\n\n\
             Remove the agent from your project config:\n  spec-kitty agent config remove roo"
        );
    }
    if roo_in_config(project_root) {
        match remove_roo_from_config(project_root)? {
            true => changes.push(
                "Removed 'roo' from .kittify/config.yaml (agent no longer supported)".to_string(),
            ),
            false => warnings.push(
                "Could not remove 'roo' from config.yaml; manual cleanup may be required"
                    .to_string(),
            ),
        }
    }
    let changed = changes
        .iter()
        .any(|change| change.starts_with("Removed 'roo'"));
    Ok(MigrationOutcome {
        changed,
        changes,
        warnings,
        ..crate::upgrade::MigrationOutcome::default()
    })
}

/// Remove `roo` from the `available` sequence, preserving every other byte.
///
/// The line-oriented edit drops only a `- roo` member line inside the
/// `agents:`/`tools:` → `available:` block, so comments, ordering, quoting and
/// unrelated keys survive untouched (the source round-trips the whole document
/// through ruamel; this is stricter about authored bytes).
pub fn remove_roo_from_config(project_root: &Path) -> Result<bool, InstallError> {
    let path = project_root.join(".kittify/config.yaml");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(source) => return Err(InstallError::io("read", &path, source)),
    };
    let Some(updated) = strip_roo_member(&text) else {
        return Ok(true);
    };
    sk_kernel::atomic_write(&path, updated.as_bytes())
        .map_err(|error| InstallError::io("write", &path, std::io::Error::other(error)))?;
    Ok(true)
}

/// The byte-preserving `roo` removal; `None` when the member is not present in a
/// recognized `available:` sequence.
///
/// Only a `- roo` member line inside the `agents:`/`tools:` → `available:` block
/// is dropped. The member indent is taken from the first sequence entry (YAML
/// allows the dash at the parent key's indentation), so comments, blank lines,
/// ordering, quoting and unrelated keys survive byte-for-byte.
pub fn strip_roo_member(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut in_agent_block = false;
    let mut agent_indent = 0usize;
    let mut in_available = false;
    let mut member_indent: Option<usize> = None;
    let mut removed = false;
    let mut output = String::with_capacity(text.len());
    for line in lines {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        let indent = trimmed.len() - trimmed.trim_start().len();
        let body = trimmed.trim_start();
        let blank = body.is_empty();
        let comment = body.starts_with('#');

        if !in_agent_block {
            if indent == 0 && (body.starts_with("agents:") || body.starts_with("tools:")) {
                in_agent_block = true;
                agent_indent = indent;
            }
            output.push_str(line);
            continue;
        }

        if !in_available {
            if !blank && !comment {
                if body.starts_with("available:") {
                    in_available = true;
                } else if indent <= agent_indent {
                    in_agent_block = false;
                }
            }
            output.push_str(line);
            continue;
        }

        // Inside the `available:` sequence.
        let is_member = body.starts_with("- ") || body == "-";
        if !blank && !comment && is_member {
            let index = *member_indent.get_or_insert(indent);
            if indent == index {
                let value = body.trim_start_matches('-').trim();
                let bare = value.trim_matches(['\'', '"']);
                if bare == ROO_AGENT_KEY {
                    removed = true;
                    continue;
                }
            }
            output.push_str(line);
            continue;
        }
        if !blank && !comment {
            // The sequence ended (a sibling key or the block's end).
            in_available = false;
            member_indent = None;
            if indent <= agent_indent {
                in_agent_block = false;
            }
        }
        output.push_str(line);
    }
    removed.then_some(output)
}

// -------------------------------------- 4.0.0rc1 retired hosted target

/// The retired first-party hosted endpoint (exact host component match).
pub const RETIRED_HOSTED_HOSTNAME: &str = "app.spec-kitty.ai";

/// The machine-scoped runtime `config.toml`.
///
/// Resolved through the canonical runtime state root at call time (no I/O, no
/// directory creation), so `SPEC_KITTY_HOME` drives it exactly like every other
/// runtime-root consumer.
pub fn home_config_path() -> Result<PathBuf, InstallError> {
    let root = sk_kernel::paths::get_runtime_state_root().map_err(|error| {
        InstallError::io(
            "resolve runtime root",
            Path::new("."),
            std::io::Error::other(error),
        )
    })?;
    Ok(root.join("config.toml"))
}

/// The hostname of `url`, or `None` when it is not an absolute URL.
pub fn url_hostname(url: &str) -> Option<String> {
    let rest = url.trim().split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if authority.is_empty() {
        return None;
    }
    let host = if let Some(stripped) = authority.strip_prefix('[') {
        // IPv6 literal: `[::1]:8080`.
        stripped.split(']').next()?.to_string()
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) => host.to_string(),
            _ => authority.to_string(),
        }
    };
    Some(host.to_ascii_lowercase())
}

/// Whether `url`'s host is exactly the retired first-party endpoint.
pub fn is_retired_first_party_url(url: &str) -> bool {
    url_hostname(url).as_deref() == Some(RETIRED_HOSTED_HOSTNAME)
}

fn load_home_config(path: &Path) -> Option<toml_edit::DocumentMut> {
    if !path.is_file() {
        return None;
    }
    std::fs::read_to_string(path).ok()?.parse().ok()
}

/// The retired `[sync].server_url` value, when the machine still carries it.
pub fn retired_server_url(path: &Path) -> Option<String> {
    let document = load_home_config(path)?;
    let value = document.get("sync")?.get("server_url")?.as_str()?;
    is_retired_first_party_url(value).then(|| value.to_string())
}

/// `4_0_0_retired_hosted_target.detect` (machine-scoped; ignores the project).
pub fn detect_retired_hosted_target(_project_root: &Path) -> Result<bool, InstallError> {
    Ok(home_config_path()
        .ok()
        .and_then(|path| retired_server_url(&path))
        .is_some())
}

/// `4_0_0_retired_hosted_target.can_apply`.
pub fn can_apply_retired_hosted_target(
    _project_root: &Path,
) -> Result<Option<String>, InstallError> {
    if detect_retired_hosted_target(Path::new("."))? {
        Ok(None)
    } else {
        Ok(Some(
            "no retired first-party server_url in config.toml (already canonical, unset, or custom)"
                .to_string(),
        ))
    }
}

/// `4_0_0_retired_hosted_target.apply` at a caller-supplied `config.toml`.
///
/// The effect is path-parameterised so it can be exercised without mutating the
/// process environment; `apply_retired_hosted_target` is the machine-scoped
/// wrapper that resolves the canonical runtime root.
pub fn apply_retired_hosted_target_at(
    config_path: &Path,
) -> Result<MigrationOutcome, InstallError> {
    let Some(mut document) = load_home_config(config_path) else {
        return Ok(MigrationOutcome::unchanged(vec![
            "no retired first-party server_url in config.toml (already canonical, unset, or custom)"
                .to_string(),
        ]));
    };
    let Some(stale) = document
        .get("sync")
        .and_then(|sync| sync.get("server_url"))
        .and_then(|value| value.as_str())
        .filter(|value| is_retired_first_party_url(value))
        .map(str::to_string)
    else {
        return Ok(MigrationOutcome::unchanged(vec![
            "no retired first-party server_url in config.toml (already canonical, unset, or custom)"
                .to_string(),
        ]));
    };
    if let Some(sync) = document
        .get_mut("sync")
        .and_then(|sync| sync.as_table_mut())
    {
        // Only the retired key is removed; every other table, key and value is
        // carried through, and an empty `[sync]` table stays in place.
        sync.remove("server_url");
    }
    sk_kernel::atomic_write(config_path, document.to_string().as_bytes())
        .map_err(|error| InstallError::io("write", config_path, std::io::Error::other(error)))?;
    Ok(MigrationOutcome::changed(vec![format!(
        "removed retired config.toml sync.server_url {stale}"
    )]))
}

/// `4_0_0_retired_hosted_target.apply` (machine-scoped).
pub fn apply_retired_hosted_target(_project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let path = home_config_path()?;
    apply_retired_hosted_target_at(&path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> tempfile::TempDir {
        tempfile::tempdir().expect("temp")
    }

    fn write(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, bytes).unwrap();
    }

    fn configured(project: &Path) {
        write(
            &project.join(".kittify/config.yaml"),
            b"agents:\n  available:\n  - claude\n",
        );
    }

    #[test]
    fn a_managed_hook_is_removed_and_a_custom_one_is_kept() {
        let project = temp();
        write(
            &project.path().join(".git/hooks/pre-commit"),
            b"#!/bin/sh\nSPEC_KITTY_MANAGED_HOOK_SHIM=1\nexec spec-kitty pre-commit\n",
        );
        write(
            &project.path().join(".git/hooks/commit-msg"),
            b"#!/bin/sh\nmy own commit-msg check\n",
        );
        assert!(detect_retire_git_hooks(project.path()).unwrap());
        let outcome = apply_retire_git_hooks(project.path()).unwrap();
        assert!(outcome.changed);
        assert!(!project.path().join(".git/hooks/pre-commit").exists());
        assert!(project.path().join(".git/hooks/commit-msg").exists());
        assert!(
            outcome
                .warnings
                .iter()
                .any(|w| w.contains("Skipped custom hook"))
        );
        assert!(!detect_retire_git_hooks(project.path()).unwrap());
    }

    #[test]
    fn clarify_removal_proves_ownership_and_preserves_a_user_file() {
        let project = temp();
        configured(project.path());
        write(
            &project
                .path()
                .join(".claude/commands/spec-kitty.clarify.md"),
            b"<!-- spec-kitty-command-version: 2.0.11 -->\n# clarify\n",
        );
        write(
            &project
                .path()
                .join(".cursor/commands/spec-kitty.clarify.md"),
            b"# my own clarify command\n",
        );
        assert!(detect_remove_clarify_command(project.path()).unwrap());
        let outcome = apply_remove_clarify_command(project.path()).unwrap();
        assert!(
            !project
                .path()
                .join(".claude/commands/spec-kitty.clarify.md")
                .exists()
        );
        assert!(
            project
                .path()
                .join(".cursor/commands/spec-kitty.clarify.md")
                .exists()
        );
        assert!(outcome.warnings.iter().any(|w| w.contains("Preserved")));
    }

    #[test]
    fn the_release_skill_is_archived_when_unproven_and_removed_when_manifest_owned() {
        let project = temp();
        let skill = project.path().join(RELEASE_SKILL_REL);
        write(&skill.join("SKILL.md"), b"# release\n");
        assert!(detect_remove_release_skill(project.path()).unwrap());
        let outcome = apply_remove_release_skill(project.path()).unwrap();
        assert!(!skill.exists());
        assert!(outcome.changes.iter().any(|c| c.contains("archived to")));
        let archived = std::fs::read_dir(project.path().join(".kittify"))
            .unwrap()
            .flatten()
            .find(|entry| entry.file_name().to_string_lossy().starts_with(".backup-"))
            .expect("archive directory");
        assert!(
            archived
                .path()
                .join(RELEASE_SKILL_REL)
                .join("SKILL.md")
                .exists()
        );
        assert!(!detect_remove_release_skill(project.path()).unwrap());
    }

    #[test]
    fn roo_member_stripping_preserves_every_other_byte() {
        let text =
            "# my config\nagents:\n  available:\n  - claude\n  - roo\n  - codex\n  other: 1\n";
        let updated = strip_roo_member(text).expect("member removed");
        assert_eq!(
            updated,
            "# my config\nagents:\n  available:\n  - claude\n  - codex\n  other: 1\n"
        );
        assert!(strip_roo_member("agents:\n  available:\n  - claude\n").is_none());
    }

    #[test]
    fn the_retired_hosted_url_match_is_component_wise() {
        assert!(is_retired_first_party_url("https://app.spec-kitty.ai"));
        assert!(is_retired_first_party_url(
            "https://app.spec-kitty.ai:443/x"
        ));
        assert!(is_retired_first_party_url("  https://App.Spec-Kitty.AI  "));
        assert!(!is_retired_first_party_url(
            "https://app.spec-kitty.ai.evil.example"
        ));
        assert!(!is_retired_first_party_url("https://eu.app.spec-kitty.ai"));
        assert!(!is_retired_first_party_url("app.spec-kitty.ai"));
        assert!(!is_retired_first_party_url(""));
    }

    #[test]
    fn dashboard_meta_is_kept_when_a_server_is_provably_running() {
        let project = temp();
        configured(project.path());
        // A live PID whose command line is *not* the dashboard: removable.
        write(
            &project.path().join(DASHBOARD_META_REL),
            b"http://127.0.0.1:8765\n8765\ntoken\n1\n",
        );
        write(
            &project.path().join(DASHBOARD_PREFLIGHT_REL),
            b"{\"warning\": true}\n",
        );
        write(
            &project
                .path()
                .join(".claude/commands/spec-kitty.dashboard.md"),
            b"<!-- spec-kitty-command-version: 4.0.0rc5 -->\n# dashboard\n",
        );
        assert!(detect_retire_bundled_dashboard(project.path()).unwrap());
        let outcome = apply_retire_bundled_dashboard(project.path()).unwrap();
        assert!(!project.path().join(DASHBOARD_META_REL).exists());
        assert!(!project.path().join(DASHBOARD_PREFLIGHT_REL).exists());
        assert!(
            !project
                .path()
                .join(".claude/commands/spec-kitty.dashboard.md")
                .exists()
        );
        assert!(!detect_retire_bundled_dashboard(project.path()).unwrap());
        let _ = outcome;
    }
}
