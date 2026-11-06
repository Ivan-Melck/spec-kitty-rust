//! Session-presence orientation migrations (target `>= 3`).
//!
//! | id | effect |
//! | --- | --- |
//! | [`CLAUDE_CODE_ID`] | write `.claude/CLAUDE.md` orientation + both hooks for a configured `claude` |
//! | [`ALL_HARNESSES_ID`] | write the orientation block for every *other* configured harness |
//! | [`REFRESH_ID`] | replace a stale installed block with the current rendering |
//!
//! All three delegate the write to [`crate::session::apply_session_presence`],
//! which owns the harness table, the atomic section replacement, the
//! operator-edit preservation and the `settings.json` merge. This module adds
//! only the pinned `detect` / `can_apply` / report shape:
//!
//! * the Claude Code effect reports its three artefacts and, like the source,
//!   does not turn a preserved (`git`-tracked) `settings.json` into a failure;
//! * the all-harnesses effect only *observes* configured keys, so a harness
//!   directory that exists on disk without being configured is ignored (C-005);
//! * the refresh effect never installs an absent block — that stays the two
//!   install effects' job.

use std::path::Path;

use crate::error::InstallError;
use crate::session::OrientationContent;
use crate::upgrade::{MigrationOutcome, NativeMigration};

use super::support::{self, PresenceWriter};

/// `3_3_0_session_presence_claude_code`.
pub const CLAUDE_CODE_ID: &str = "3_3_0_session_presence_claude_code";
/// `3_3_0_session_presence_all_harnesses`.
pub const ALL_HARNESSES_ID: &str = "3_3_0_session_presence_all_harnesses";
/// `3_2_0rc39_refresh_orientation_block`.
pub const REFRESH_ID: &str = "3_2_0rc39_refresh_orientation_block";

/// The harness the Phase-1 observation effect owns.
const CLAUDE: &str = "claude";

/// The orientation content a migration pass renders.
///
/// Upstream's `SessionPresenceManager._build_content()` reads the installed CLI
/// version, the canonical project slug and a best-effort health verdict. The
/// migration pass writes the healthy block for the running compatibility
/// version: an upgrade immediately re-stamps the project at that version, and
/// the health probe is explicitly best-effort (it falls back to `healthy` when
/// the compat plan raises).
pub(crate) fn orientation_content(project_root: &Path) -> OrientationContent {
    OrientationContent::healthy(
        super::COMPAT_CLI_VERSION,
        support::project_slug(project_root),
    )
}

fn kittify_exists(project_root: &Path) -> bool {
    project_root.join(".kittify").is_dir()
}

// ---------------------------------------------------------------------------
// 3_3_0_session_presence_claude_code
// ---------------------------------------------------------------------------

/// `detect`: an initialized project that configures `claude` without presence.
pub fn detect_claude_code(project_root: &Path) -> Result<bool, InstallError> {
    if !kittify_exists(project_root) {
        return Ok(false);
    }
    if !support::configured_agents(project_root)?
        .iter()
        .any(|key| key == CLAUDE)
    {
        return Ok(false);
    }
    Ok(!support::has_presence(project_root, CLAUDE))
}

/// `can_apply`: the project must be initialized.
pub fn can_apply_claude_code(project_root: &Path) -> Result<Option<String>, InstallError> {
    if !kittify_exists(project_root) {
        return Ok(Some(
            ".kittify/ directory does not exist (not initialized)".to_string(),
        ));
    }
    Ok(None)
}

/// `apply`: write the section and register both hooks.
pub fn apply_claude_code(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let content = orientation_content(project_root);
    let report =
        crate::session::apply_session_presence(project_root, &[CLAUDE.to_string()], &content)?;
    let mut outcome = MigrationOutcome::changed(vec![
        "Wrote spec-kitty orientation to .claude/CLAUDE.md".to_string(),
        "Registered spec-kitty session-start SessionStart hook".to_string(),
        "Registered spec-kitty session-stop Stop hook".to_string(),
    ]);
    outcome.warnings = report.warnings;
    Ok(outcome)
}

// ---------------------------------------------------------------------------
// 3_3_0_session_presence_all_harnesses
// ---------------------------------------------------------------------------

/// Configured harness keys outside the Phase-1 (`claude`) effect.
fn pending_non_claude(project_root: &Path) -> Result<Vec<String>, InstallError> {
    let mut pending = Vec::new();
    for key in support::configured_agents(project_root)? {
        if key == CLAUDE {
            continue;
        }
        let Some(writer) = support::presence_writer(&key) else {
            // The registry's NullWriter: no verified orientation mechanism.
            continue;
        };
        if support::presence_can_write(project_root, writer)
            && !support::has_presence(project_root, &key)
        {
            pending.push(key);
        }
    }
    Ok(pending)
}

/// `detect`: any configured non-Claude harness still needs orientation.
pub fn detect_all_harnesses(project_root: &Path) -> Result<bool, InstallError> {
    if !kittify_exists(project_root) {
        return Ok(false);
    }
    Ok(!pending_non_claude(project_root)?.is_empty())
}

/// `can_apply`: the project must be initialized.
pub fn can_apply_all_harnesses(project_root: &Path) -> Result<Option<String>, InstallError> {
    if !kittify_exists(project_root) {
        return Ok(Some(
            ".kittify/ directory does not exist (not initialized)".to_string(),
        ));
    }
    Ok(None)
}

/// `apply`: write the orientation block for each pending harness.
pub fn apply_all_harnesses(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let pending = pending_non_claude(project_root)?;
    if pending.is_empty() {
        return Ok(MigrationOutcome::skipped());
    }
    let content = orientation_content(project_root);
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    for key in pending {
        // Re-check inside the write loop: a concurrent pass may have written it.
        if support::has_presence(project_root, &key) {
            continue;
        }
        let report = crate::session::apply_session_presence(
            project_root,
            std::slice::from_ref(&key),
            &content,
        )?;
        warnings.extend(report.warnings);
        changes.push(format!(
            "Wrote session presence orientation for harness: {key}"
        ));
    }
    Ok(MigrationOutcome {
        changed: !changes.is_empty(),
        changes,
        warnings,
        ..crate::upgrade::MigrationOutcome::default()
    })
}

// ---------------------------------------------------------------------------
// 3_2_0rc39_refresh_orientation_block
// ---------------------------------------------------------------------------

/// Configured harnesses whose installed block differs from the current render.
fn stale_harnesses(project_root: &Path, rendered: &str) -> Result<Vec<String>, InstallError> {
    let mut stale = Vec::new();
    for key in support::configured_agents(project_root)? {
        let Some(writer) = support::presence_writer(&key) else {
            continue;
        };
        // Only an installed block is refreshed; an absent one is the install
        // effects' responsibility.
        if !support::has_presence(project_root, &key) {
            continue;
        }
        let path = project_root.join(match writer {
            PresenceWriter::ClaudeCode => ".claude/CLAUDE.md",
            PresenceWriter::Append { path, .. } | PresenceWriter::Whole { path, .. } => path,
        });
        if support::extract_section(&path).as_deref() != Some(rendered) {
            stale.push(key);
        }
    }
    Ok(stale)
}

/// `detect`: at least one configured harness has a stale installed block.
pub fn detect_refresh(project_root: &Path) -> Result<bool, InstallError> {
    if !kittify_exists(project_root) {
        return Ok(false);
    }
    let content = orientation_content(project_root);
    Ok(!stale_harnesses(project_root, &content.render())?.is_empty())
}

/// `can_apply`: the project must be initialized.
pub fn can_apply_refresh(project_root: &Path) -> Result<Option<String>, InstallError> {
    if !kittify_exists(project_root) {
        return Ok(Some(
            ".kittify/ directory does not exist (not initialized)".to_string(),
        ));
    }
    Ok(None)
}

/// `apply`: replace each stale block with the current rendering.
///
/// A block that is present but **not** provably canonical (an operator edited
/// the managed section) is preserved: the source's writer refuses that write
/// too, and the migration still reports the refresh it attempted, so the
/// report shape is identical while the operator's bytes survive.
pub fn apply_refresh(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let content = orientation_content(project_root);
    let stale = stale_harnesses(project_root, &content.render())?;
    if stale.is_empty() {
        return Ok(MigrationOutcome::skipped());
    }
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    for key in stale {
        let rules_path = rules_path_for(project_root, &key);
        let canonical = rules_path
            .as_deref()
            .and_then(support::extract_section)
            .is_some_and(|block| is_canonical_orientation_block(&block, &content));
        if canonical {
            let report = crate::session::apply_session_presence(
                project_root,
                std::slice::from_ref(&key),
                &content,
            )?;
            warnings.extend(report.warnings);
        }
        changes.push(format!("Refreshed orientation block for harness: {key}"));
    }
    Ok(MigrationOutcome {
        changed: !changes.is_empty(),
        changes,
        warnings,
        ..crate::upgrade::MigrationOutcome::default()
    })
}

fn rules_path_for(project_root: &Path, agent_key: &str) -> Option<std::path::PathBuf> {
    let writer = support::presence_writer(agent_key)?;
    let relative = match writer {
        PresenceWriter::ClaudeCode => ".claude/CLAUDE.md",
        PresenceWriter::Append { path, .. } | PresenceWriter::Whole { path, .. } => path,
    };
    Some(project_root.join(relative))
}

/// Whether `block` is exactly a block this tool renders for some header triple.
///
/// The pinned writer proves the whole section with its own renderer rather than
/// matching a suffix: a block whose *only* difference is the version, slug,
/// health or available-version header is canonical (and refreshable), while any
/// other edit is the operator's. The pre-#6475 slash-only invocation is accepted
/// as canonical too, so an older block is still refreshed instead of frozen.
///
/// `_content` is the *target* rendering, deliberately unused: a block written
/// for an older version/slug/health is canonical (and refreshable), so the
/// comparison rebuilds the block from its own parsed header instead.
fn is_canonical_orientation_block(block: &str, _content: &OrientationContent) -> bool {
    let trimmed = block.trim_end_matches('\n');
    if !trimmed.starts_with(crate::session::SECTION_OPEN)
        || !trimmed.ends_with(crate::session::SECTION_CLOSE)
    {
        return false;
    }
    let inner = trimmed
        .trim_start_matches(crate::session::SECTION_OPEN)
        .trim_end_matches(crate::session::SECTION_CLOSE)
        .trim_matches('\n');
    let mut lines = inner.lines();
    let Some(header) = lines.next() else {
        return false;
    };
    let Some((version, slug, health)) = parse_orientation_header(header) else {
        return false;
    };
    let mut rest = lines.collect::<Vec<_>>();
    let available_version = if health == crate::session::OrientationHealth::UpgradeAvailable {
        let Some(line) = rest.first().copied() else {
            return false;
        };
        const PREFIX: &str = "⚠ Upgrade available: ";
        const SUFFIX: &str = " — run `spec-kitty upgrade --cli` to update.";
        let Some(available) = line
            .strip_prefix(PREFIX)
            .and_then(|rest| rest.strip_suffix(SUFFIX))
        else {
            return false;
        };
        if available.is_empty() {
            return false;
        }
        rest.remove(0);
        Some(available.to_string())
    } else {
        None
    };
    let block_body = rest.join("\n");
    let candidate = OrientationContent {
        version,
        project_slug: slug,
        health,
        available_version,
    };
    let mut rendered = candidate.render();
    rendered = rendered.trim_end_matches('\n').to_string();
    let rendered_inner = rendered
        .trim_start_matches(crate::session::SECTION_OPEN)
        .trim_end_matches(crate::session::SECTION_CLOSE)
        .trim_matches('\n');
    let mut rendered_lines = rendered_inner.lines();
    rendered_lines.next();
    let rendered_body: Vec<&str> = rendered_lines
        .filter(|line| !line.starts_with("⚠ Upgrade available: "))
        .collect();
    let block_lines: Vec<&str> = block_body
        .lines()
        .filter(|line| !line.starts_with("⚠ Upgrade available: "))
        .collect();
    for legacy in [false, true] {
        let expected: Vec<String> = if legacy {
            rendered_body
                .iter()
                .map(|line| {
                    line.replace(
                        crate::session::ORIENTATION_INVOCATION,
                        "`/spec-kitty.specify`",
                    )
                })
                .collect()
        } else {
            rendered_body
                .iter()
                .map(|line| (*line).to_string())
                .collect()
        };
        let actual: Vec<String> = block_lines.iter().map(|line| (*line).to_string()).collect();
        if expected == actual {
            return true;
        }
    }
    false
}

fn parse_orientation_header(
    header: &str,
) -> Option<(String, String, crate::session::OrientationHealth)> {
    let rest = header.strip_prefix("**Spec Kitty v")?;
    let (version, rest) = rest.split_once("** — project: ")?;
    let (slug, rest) = rest.rsplit_once(" (")?;
    let health = rest.strip_suffix(')')?;
    if version.is_empty() || slug.is_empty() {
        return None;
    }
    let health = match health {
        "healthy" => crate::session::OrientationHealth::Healthy,
        "upgrade-available" => crate::session::OrientationHealth::UpgradeAvailable,
        "migration-required" => crate::session::OrientationHealth::MigrationRequired,
        _ => return None,
    };
    Some((version.to_string(), slug.to_string(), health))
}

/// The three session-presence implementations, in catalog order.
pub const IMPLEMENTATIONS: &[NativeMigration] = &[
    NativeMigration {
        id: CLAUDE_CODE_ID,
        detect: detect_claude_code,
        can_apply: can_apply_claude_code,
        apply: apply_claude_code,
    },
    NativeMigration {
        id: ALL_HARNESSES_ID,
        detect: detect_all_harnesses,
        can_apply: can_apply_all_harnesses,
        apply: apply_all_harnesses,
    },
    NativeMigration {
        id: REFRESH_ID,
        detect: detect_refresh,
        can_apply: can_apply_refresh,
        apply: apply_refresh,
    },
];
