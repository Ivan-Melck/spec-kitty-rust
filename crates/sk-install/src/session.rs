//! Session-presence projection: the orientation block and Claude Code hooks.
//!
//! Upstream keeps one orientation text (`SessionPresenceContent.render()`) and
//! injects it into whichever file each harness reads at session start:
//!
//! | harness | artefact | mode |
//! | --- | --- | --- |
//! | `claude` | `.claude/CLAUDE.md` + `.claude/settings.json` hooks | append section |
//! | `codex`, `opencode`, `antigravity`, `pi`, `vibe`, `letta` | `AGENTS.md` | append section |
//! | `gemini` / `llxprt` | `GEMINI.md` / `LLXPRT.md` | append section |
//! | `copilot` | `.github/copilot-instructions.md` | append section |
//! | `cursor` / `windsurf` / `kiro` | `.cursor/rules/spec-kitty.mdc` etc. | whole file |
//!
//! The projection is idempotent: a canonical block is replaced in place, while
//! an operator-edited block is preserved and reported. A harness whose root
//! directory does not exist is skipped — init never invents a harness directory
//! merely to write an orientation block. `qwen`, `kilocode`, `auggie` and `q`
//! have no verified orientation mechanism upstream (their writer is a no-op), so
//! no file is written for them.
//!
//! `.claude/settings.json` is only ever merged, never replaced: unrelated keys,
//! permissions and hook entries survive, invalid JSON is preserved with a
//! warning, and a settings file Git already tracks is left untouched
//! (upstream #6474: a committed team file is not ours to edit).

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::error::InstallError;

/// The opening marker of the managed orientation section.
pub const SECTION_OPEN: &str = "<!-- spec-kitty:orientation -->";
/// The closing marker of the managed orientation section.
pub const SECTION_CLOSE: &str = "<!-- /spec-kitty:orientation -->";
/// The Claude settings path.
pub const CLAUDE_SETTINGS_REL: &str = ".claude/settings.json";
/// The `SessionStart` hook command.
pub const SESSION_START_CMD: &str = "spec-kitty session-start";
/// The `Stop` hook command.
pub const SESSION_STOP_CMD: &str = "spec-kitty session-stop";
/// The canonical mission-start invocation sentence (upstream
/// `invocation.orientation_invocation()` for the frozen agent roster).
pub const ORIENTATION_INVOCATION: &str = "`/spec-kitty.specify` (slash-command agents), `$spec-kitty-specify` (codex), `/skill:spec-kitty-specify` (pi) or `/spec-kitty-specify` (antigravity, letta, vibe)";

/// How the project's metadata compares with the running CLI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrientationHealth {
    /// The project is current.
    Healthy,
    /// A newer CLI is available.
    UpgradeAvailable,
    /// The project needs an upgrade before missions work.
    MigrationRequired,
}

impl OrientationHealth {
    /// Stable spelling used inside the block.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::UpgradeAvailable => "upgrade-available",
            Self::MigrationRequired => "migration-required",
        }
    }
}

/// The exact orientation block text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrientationContent {
    /// The running CLI version.
    pub version: String,
    /// The project slug.
    pub project_slug: String,
    /// Project health.
    pub health: OrientationHealth,
    /// A newer available version, when `health` is `UpgradeAvailable`.
    pub available_version: Option<String>,
}

impl OrientationContent {
    /// A healthy block for a freshly initialized project.
    pub fn healthy(version: impl Into<String>, project_slug: impl Into<String>) -> Self {
        Self {
            version: version.into(),
            project_slug: project_slug.into(),
            health: OrientationHealth::Healthy,
            available_version: None,
        }
    }

    /// Render the block, ending with the closing marker and a newline.
    pub fn render(&self) -> String {
        let upgrade_line = if self.health == OrientationHealth::UpgradeAvailable {
            format!(
                "\n⚠ Upgrade available: {} — run `spec-kitty upgrade --cli` to update.",
                self.available_version
                    .as_deref()
                    .unwrap_or("a newer release")
            )
        } else {
            String::new()
        };
        let migration_line = if self.health == OrientationHealth::MigrationRequired {
            "\n⚠ Project migration required — run `spec-kitty upgrade` before using missions."
                .to_string()
        } else {
            String::new()
        };
        format!(
            "{SECTION_OPEN}\n**Spec Kitty v{version}** — project: {slug} ({health}){upgrade_line}{migration_line}\n\nTwo usage patterns:\n- **Full mission** (spec → plan → tasks → implement → review → merge):\n  trigger: \"spec out\", \"create a mission\", \"write a spec\", \"plan this\"\n  → run {invocation}\n- **Lightweight dispatch** (ad-hoc fix, question, or advice — no mission created):\n  trigger: \"hey spec kitty\", \"use spec kitty to\", \"spec kitty <anything>\"\n  → **ALWAYS run `spec-kitty dispatch \"<request verbatim>\"` — do NOT answer directly.**\n  If you know the right profile, pass it to skip routing:\n  `spec-kitty dispatch \"<request verbatim>\" --profile <profile-id>`\n  Reason: `spec-kitty dispatch` loads governance context, routes the request,\n  and opens the Op. Skipping it produces ungoverned, untracked responses.\n  After finishing the work, close the Op with the command printed in the capsule\n  (`spec-kitty profile-invocation complete --invocation-id <id> --outcome <done|failed|abandoned>`).\n{SECTION_CLOSE}\n",
            version = self.version,
            slug = self.project_slug,
            health = self.health.as_str(),
            invocation = ORIENTATION_INVOCATION,
        )
    }
}

/// The writer shape a harness uses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WriterKind {
    /// Append/replace a managed section in an existing-or-new file.
    MarkdownAppend {
        path: &'static str,
        check_dir: Option<&'static str>,
    },
    /// The file *is* the managed section.
    MarkdownWhole {
        path: &'static str,
        check_dir: Option<&'static str>,
    },
    /// Append/replace a section and manage `.claude/settings.json` hooks.
    ClaudeCode,
}

const fn append(path: &'static str) -> WriterKind {
    WriterKind::MarkdownAppend {
        path,
        check_dir: None,
    }
}

const fn whole(path: &'static str, check_dir: Option<&'static str>) -> WriterKind {
    WriterKind::MarkdownWhole { path, check_dir }
}

fn writer_for(agent_key: &str) -> Option<WriterKind> {
    match agent_key {
        "claude" => Some(WriterKind::ClaudeCode),
        "codex" | "opencode" | "antigravity" | "pi" | "vibe" | "letta" => Some(append("AGENTS.md")),
        "gemini" => Some(append("GEMINI.md")),
        "llxprt" => Some(append("LLXPRT.md")),
        // Upstream guards copilot on the harness root (`.github`), not on the
        // rules file's parent, which init only creates later when profiles are
        // projected. A copilot-only fresh init therefore writes no
        // `.github/copilot-instructions.md` in the pinned oracle.
        "copilot" => Some(WriterKind::MarkdownAppend {
            path: ".github/copilot-instructions.md",
            check_dir: Some(".github"),
        }),
        "cursor" => Some(whole(".cursor/rules/spec-kitty.mdc", Some(".cursor"))),
        "windsurf" => Some(whole(".windsurf/rules/spec-kitty.md", Some(".windsurf"))),
        "kiro" => Some(whole(".kiro/steering/spec-kitty.md", Some(".kiro"))),
        _ => None,
    }
}

/// What one session-presence pass did.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SessionPresenceReport {
    /// Files written.
    pub written: Vec<String>,
    /// Files already current.
    pub kept: Vec<String>,
    /// Files preserved because an operator edited them or they are unsafe.
    pub preserved: Vec<String>,
    /// Operators that have no verified orientation mechanism.
    pub not_applicable: Vec<String>,
    /// Warnings for the CLI to render.
    pub warnings: Vec<String>,
}

/// Project the orientation block (and Claude hooks) for `agent_keys`.
pub fn apply_session_presence(
    project_root: &Path,
    agent_keys: &[String],
    content: &OrientationContent,
) -> Result<SessionPresenceReport, InstallError> {
    let mut report = SessionPresenceReport::default();
    let mut tools: Vec<&str> = Vec::new();
    for key in agent_keys {
        if !tools.contains(&key.as_str()) {
            tools.push(key);
        }
    }
    tools.sort_unstable();
    for tool in tools {
        // Upstream wraps every harness write in a warn-only handler: an optional
        // orientation surface never fails init. A refused write is reported and
        // the remaining harnesses still run.
        let result: Result<(), InstallError> = (|| {
            let Some(kind) = writer_for(tool) else {
                report.not_applicable.push(tool.to_string());
                return Ok(());
            };
            // repeated tools that write the same target (codex + vibe) are
            // skipped once the target's content is current; the write itself is
            // idempotent.
            match kind {
                WriterKind::MarkdownAppend { path, check_dir } => {
                    if !harness_root_exists(project_root, path, check_dir) {
                        report.not_applicable.push(tool.to_string());
                        return Ok(());
                    }
                    record_section(
                        &mut report,
                        tool,
                        path,
                        write_section(project_root, path, content, true),
                    );
                }
                WriterKind::MarkdownWhole { path, check_dir } => {
                    if !harness_root_exists(project_root, path, check_dir) {
                        report.not_applicable.push(tool.to_string());
                        return Ok(());
                    }
                    record_section(
                        &mut report,
                        tool,
                        path,
                        write_section(project_root, path, content, false),
                    );
                }
                WriterKind::ClaudeCode => {
                    // A symlinked `.claude` root is an unowned layout: refuse both
                    // the orientation file and the hook settings instead of
                    // writing through the link (upstream warns and skips).
                    if let Some(link) = first_symlink_component(project_root, ".claude") {
                        report.preserved.push(".claude/CLAUDE.md".to_string());
                        report.warnings.push(format!(
                            "Failed to write orientation for {tool}: Refusing unowned symlink: {}",
                            display_path(project_root, &link)
                        ));
                        return Ok(());
                    }
                    if !project_root.join(".claude").is_dir() {
                        report.not_applicable.push(tool.to_string());
                        return Ok(());
                    }
                    record_section(
                        &mut report,
                        tool,
                        ".claude/CLAUDE.md",
                        write_section(project_root, ".claude/CLAUDE.md", content, true),
                    );
                    match merge_claude_hooks(project_root) {
                        Ok(HookOutcome::Written) => {
                            report.written.push(CLAUDE_SETTINGS_REL.to_string())
                        }
                        Ok(HookOutcome::Kept) => report.kept.push(CLAUDE_SETTINGS_REL.to_string()),
                        Ok(HookOutcome::Preserved(reason)) => {
                            report.preserved.push(CLAUDE_SETTINGS_REL.to_string());
                            report.warnings.push(reason);
                        }
                        Err(error) => report
                            .warnings
                            .push(format!("Failed to write orientation for {tool}: {error}")),
                    }
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            report
                .warnings
                .push(format!("Failed to write orientation for {tool}: {error}"));
        }
    }
    Ok(report)
}

/// Whether the harness root (or the rules file's real parent) is present.
fn harness_root_exists(project_root: &Path, path: &str, check_dir: Option<&str>) -> bool {
    match check_dir {
        Some(check_dir) => project_root.join(check_dir).exists(),
        None => project_root.join(path).parent().is_some_and(Path::exists),
    }
}

fn record_section(
    report: &mut SessionPresenceReport,
    tool: &str,
    path: &str,
    outcome: Result<SectionOutcome, InstallError>,
) {
    match outcome {
        Ok(SectionOutcome::Written) => report.written.push(path.to_string()),
        Ok(SectionOutcome::Kept) => report.kept.push(path.to_string()),
        Ok(SectionOutcome::Preserved(reason)) => {
            report.preserved.push(path.to_string());
            report
                .warnings
                .push(format!("Failed to write orientation for {tool}: {reason}"));
        }
        Err(error) => report
            .warnings
            .push(format!("Failed to write orientation for {tool}: {error}")),
    }
}

enum SectionOutcome {
    Written,
    Kept,
    Preserved(String),
}

/// The first symlinked component of `relative` under `project_root`, if any.
///
/// Mirrors upstream `observe_presence_path`: every component is inspected with
/// `lstat`, so a symlinked directory is refused even when its target stays
/// inside the project. An absent component ends the walk (the write may create
/// it).
fn first_symlink_component(project_root: &Path, relative: &str) -> Option<PathBuf> {
    let mut current = project_root.to_path_buf();
    for component in Path::new(relative).components() {
        current.push(component);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => return Some(current),
            Ok(_) => {}
            Err(_) => return None,
        }
    }
    None
}

fn display_path(project_root: &Path, path: &Path) -> String {
    path.strip_prefix(project_root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn write_section(
    project_root: &Path,
    relative: &str,
    content: &OrientationContent,
    append_mode: bool,
) -> Result<SectionOutcome, InstallError> {
    let destination = project_root.join(relative);
    // Refuse to write through any symlinked component, including a parent such
    // as `.claude/` or `.github/` (upstream `observe_presence_path`). The managed
    // region is preserved and the refusal is reported, never written through.
    if let Some(link) = first_symlink_component(project_root, relative) {
        return Ok(SectionOutcome::Preserved(format!(
            "Refusing unowned symlink: {}",
            display_path(project_root, &link)
        )));
    }
    let metadata = match std::fs::symlink_metadata(&destination) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => return Err(InstallError::io("inspect", &destination, source)),
    };
    if let Some(metadata) = &metadata
        && !metadata.file_type().is_file()
    {
        return Ok(SectionOutcome::Preserved(
            "orientation destination is not a regular file".to_string(),
        ));
    }
    let existing = match &metadata {
        Some(_) => sk_kernel::read_bounded(&destination, 4 * 1024 * 1024).map_err(|error| {
            InstallError::io("read", &destination, std::io::Error::other(error))
        })?,
        None => Vec::new(),
    };
    let text = match String::from_utf8(existing) {
        Ok(text) => text,
        Err(error) => {
            return Ok(SectionOutcome::Preserved(format!(
                "orientation destination is not UTF-8: {error}"
            )));
        }
    };
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let rendered = content.render().replace('\n', newline);
    let desired = if let Some(start) = text.find(SECTION_OPEN) {
        let Some(close_offset) = text[start..].find(SECTION_CLOSE) else {
            return Ok(SectionOutcome::Preserved(
                "managed orientation section is unterminated".to_string(),
            ));
        };
        let end = start + close_offset + SECTION_CLOSE.len();
        if !section_is_canonical(&text[start..end]) {
            return Ok(SectionOutcome::Preserved(
                "managed orientation was edited by the operator".to_string(),
            ));
        }
        if text[start..end].contains(&format!("**Spec Kitty v{}**", content.version))
            && text[start..end].contains(ORIENTATION_INVOCATION)
        {
            return Ok(SectionOutcome::Kept);
        }
        let mut desired = String::with_capacity(text.len() + rendered.len());
        desired.push_str(&text[..start]);
        desired.push_str(rendered.trim_end_matches('\n'));
        desired.push_str(&text[end..]);
        desired
    } else if !text.is_empty() && !append_mode {
        return Ok(SectionOutcome::Preserved(
            "whole-file rules are operator owned".to_string(),
        ));
    } else {
        let mut desired = text.clone();
        if !desired.is_empty() {
            desired.push_str(newline);
            desired.push_str(newline);
        }
        desired.push_str(&rendered);
        desired
    };
    if desired == text {
        return Ok(SectionOutcome::Kept);
    }
    let path = project_root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|source| InstallError::io("create directory", parent, source))?;
    }
    sk_kernel::atomic_write(&path, desired.as_bytes()).map_err(|error| {
        InstallError::io("write orientation", &path, std::io::Error::other(error))
    })?;
    Ok(SectionOutcome::Written)
}

/// True when an existing section is exactly one of the block shapes this tool
/// writes (current or the pre-#6475 legacy invocation). Anything else is an
/// operator edit and is preserved.
fn section_is_canonical(block: &str) -> bool {
    let body = block
        .trim_start_matches(SECTION_OPEN)
        .trim_end_matches(SECTION_CLOSE)
        .trim();
    body.starts_with("**Spec Kitty v")
        && body.contains("Two usage patterns:")
        && (body.contains(ORIENTATION_INVOCATION) || body.contains("`/spec-kitty.specify`"))
        && body.contains("Lightweight dispatch")
}

enum HookOutcome {
    Written,
    Kept,
    Preserved(String),
}

fn merge_claude_hooks(project_root: &Path) -> Result<HookOutcome, InstallError> {
    let destination = project_root.join(CLAUDE_SETTINGS_REL);
    if let Some(link) = first_symlink_component(project_root, CLAUDE_SETTINGS_REL) {
        return Ok(HookOutcome::Preserved(format!(
            "Refusing unowned symlink: {}",
            display_path(project_root, &link)
        )));
    }
    let existing = match std::fs::symlink_metadata(&destination) {
        Ok(metadata) => {
            if !metadata.file_type().is_file() {
                return Ok(HookOutcome::Preserved(
                    ".claude/settings.json is not a regular file; Spec Kitty left it untouched"
                        .to_string(),
                ));
            }
            Some(
                sk_kernel::read_bounded(&destination, 4 * 1024 * 1024).map_err(|error| {
                    InstallError::io("read", &destination, std::io::Error::other(error))
                })?,
            )
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => return Err(InstallError::io("inspect", &destination, source)),
    };

    if let Some(raw) = &existing
        && settings_have_required_hooks(raw)
    {
        return Ok(HookOutcome::Kept);
    }
    if let Some(raw) = &existing {
        if is_git_tracked(project_root, CLAUDE_SETTINGS_REL) {
            return Ok(HookOutcome::Preserved(
                ".claude/settings.json is committed, so Spec Kitty did not edit it; add SessionStart/Stop hooks running `spec-kitty session-start` / `spec-kitty session-stop` yourself".to_string(),
            ));
        }
        match serde_json::from_slice::<Value>(raw) {
            Ok(Value::Object(mut object)) => {
                if !merge_hook_object(&mut object) {
                    return Ok(HookOutcome::Kept);
                }
                let mut bytes =
                    serde_json::to_vec_pretty(&Value::Object(object)).map_err(|error| {
                        InstallError::UnsupportedNode {
                            path: destination.clone(),
                            detail: error.to_string(),
                        }
                    })?;
                bytes.push(b'\n');
                write_settings(&destination, &bytes)?;
                return Ok(HookOutcome::Written);
            }
            _ => {
                return Ok(HookOutcome::Preserved(
                    ".claude/settings.json is not a JSON object; Spec Kitty left it untouched"
                        .to_string(),
                ));
            }
        }
    }
    write_settings(&destination, canonical_settings())?;
    Ok(HookOutcome::Written)
}

fn settings_have_required_hooks(raw: &[u8]) -> bool {
    let Ok(Value::Object(object)) = serde_json::from_slice::<Value>(raw) else {
        return false;
    };
    hook_present(&object, "SessionStart", SESSION_START_CMD)
        && hook_present(&object, "Stop", SESSION_STOP_CMD)
}

fn hook_present(object: &Map<String, Value>, event: &str, command: &str) -> bool {
    object
        .get("hooks")
        .and_then(Value::as_object)
        .and_then(|hooks| hooks.get(event))
        .and_then(Value::as_array)
        .is_some_and(|entries| {
            entries.iter().any(|entry| {
                entry
                    .get("hooks")
                    .and_then(Value::as_array)
                    .is_some_and(|hooks| {
                        hooks.iter().any(|hook| {
                            hook.get("type").and_then(Value::as_str) == Some("command")
                                && hook.get("command").and_then(Value::as_str) == Some(command)
                        })
                    })
            })
        })
}

fn merge_hook_object(object: &mut Map<String, Value>) -> bool {
    let hooks = object
        .entry("hooks".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(hooks) = hooks.as_object_mut() else {
        return false;
    };
    let mut changed = false;
    for (event, command) in [
        ("SessionStart", SESSION_START_CMD),
        ("Stop", SESSION_STOP_CMD),
    ] {
        let entries = hooks
            .entry(event.to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
        let Some(entries) = entries.as_array_mut() else {
            return false;
        };
        let present = entries.iter().any(|entry| {
            entry
                .get("hooks")
                .and_then(Value::as_array)
                .is_some_and(|hooks| {
                    hooks.iter().any(|hook| {
                        hook.get("type").and_then(Value::as_str) == Some("command")
                            && hook.get("command").and_then(Value::as_str) == Some(command)
                    })
                })
        });
        if !present {
            entries.push(serde_json::json!({
                "hooks": [{"type": "command", "command": command}]
            }));
            changed = true;
        }
    }
    changed
}

fn canonical_settings() -> &'static [u8] {
    b"{\n  \"hooks\": {\n    \"SessionStart\": [\n      {\n        \"hooks\": [\n          {\n            \"type\": \"command\",\n            \"command\": \"spec-kitty session-start\"\n          }\n        ]\n      }\n    ],\n    \"Stop\": [\n      {\n        \"hooks\": [\n          {\n            \"type\": \"command\",\n            \"command\": \"spec-kitty session-stop\"\n          }\n        ]\n      }\n    ]\n  }\n}\n"
}

fn write_settings(destination: &Path, bytes: &[u8]) -> Result<(), InstallError> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|source| InstallError::io("create directory", parent, source))?;
    }
    sk_kernel::atomic_write(destination, bytes).map_err(|error| {
        InstallError::io(
            "write Claude settings",
            destination,
            std::io::Error::other(error),
        )
    })?;
    Ok(())
}

/// Whether Git tracks `relative` in `project_root` (false outside a repository).
fn is_git_tracked(project_root: &Path, relative: &str) -> bool {
    let Ok(output) = std::process::Command::new("git")
        .args(["ls-files", "--error-unmatch", "--", relative])
        .current_dir(project_root)
        .output()
    else {
        return false;
    };
    output.status.success()
}

/// A convenience wrapper for tests and callers that only have a version/slug.
pub fn apply_healthy_orientation(
    project_root: &Path,
    agent_keys: &[String],
    version: &str,
    project_slug: &str,
) -> Result<SessionPresenceReport, InstallError> {
    apply_session_presence(
        project_root,
        agent_keys,
        &OrientationContent::healthy(version, project_slug),
    )
}

/// The settings path for callers that report it.
pub fn claude_settings_path(project_root: &Path) -> PathBuf {
    project_root.join(CLAUDE_SETTINGS_REL)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn orientation_block_matches_the_frozen_contract() {
        let rendered = OrientationContent::healthy("4.0.0rc8", "demo").render();
        assert!(rendered.starts_with(SECTION_OPEN));
        assert!(rendered.contains("**Spec Kitty v4.0.0rc8** — project: demo (healthy)"));
        assert!(rendered.contains(ORIENTATION_INVOCATION));
        assert!(rendered.ends_with(&format!("{SECTION_CLOSE}\n")));
    }

    #[test]
    fn appends_replaces_and_preserves_operator_edits() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir_all(root.join(".claude")).unwrap();
        let content = OrientationContent::healthy("4.0.0rc8", "demo");
        let report = apply_session_presence(root, &["claude".to_string()], &content).unwrap();
        assert_eq!(
            report.written,
            vec![".claude/CLAUDE.md", CLAUDE_SETTINGS_REL]
        );
        let first = std::fs::read_to_string(root.join(".claude/CLAUDE.md")).unwrap();
        assert!(first.starts_with(SECTION_OPEN));
        assert_eq!(
            std::fs::read_to_string(root.join(CLAUDE_SETTINGS_REL)).unwrap(),
            String::from_utf8(canonical_settings().to_vec()).unwrap()
        );
        // A repeat is a no-op.
        let report = apply_session_presence(root, &["claude".to_string()], &content).unwrap();
        assert!(report.written.is_empty());

        // Operator content around the block survives; a newer version replaces it.
        write(
            &root.join(".claude/CLAUDE.md"),
            &format!("team notes\n\n{}extra\n", first),
        );
        let newer = OrientationContent::healthy("4.0.1", "demo");
        let report = apply_session_presence(root, &["claude".to_string()], &newer).unwrap();
        assert_eq!(report.written, vec![".claude/CLAUDE.md"]);
        let text = std::fs::read_to_string(root.join(".claude/CLAUDE.md")).unwrap();
        assert!(text.starts_with("team notes\n\n"));
        assert!(text.contains("**Spec Kitty v4.0.1**"));
        assert!(text.ends_with("extra\n"));

        // An edited block is preserved.
        write(
            &root.join(".claude/CLAUDE.md"),
            &format!("{}\noperator edit\n{SECTION_CLOSE}\n", SECTION_OPEN),
        );
        let report = apply_session_presence(root, &["claude".to_string()], &newer).unwrap();
        assert_eq!(report.preserved, vec![".claude/CLAUDE.md".to_string()]);
    }

    #[test]
    fn codex_and_vibe_share_one_agents_block_and_unverified_harnesses_are_skipped() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let content = OrientationContent::healthy("4.0.0rc8", "demo");
        let report = apply_session_presence(
            root,
            &["codex".to_string(), "vibe".to_string(), "qwen".to_string()],
            &content,
        )
        .unwrap();
        assert_eq!(report.written, vec!["AGENTS.md"]);
        assert_eq!(report.not_applicable, vec!["qwen".to_string()]);
        let text = std::fs::read_to_string(root.join("AGENTS.md")).unwrap();
        assert_eq!(text.matches(SECTION_OPEN).count(), 1);
    }

    #[test]
    fn an_invalid_settings_file_is_preserved_with_a_warning() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write(&root.join(CLAUDE_SETTINGS_REL), "{not json");
        let report =
            apply_healthy_orientation(root, &["claude".to_string()], "4.0.0rc8", "demo").unwrap();
        assert!(report.preserved.contains(&CLAUDE_SETTINGS_REL.to_string()));
        assert_eq!(
            std::fs::read_to_string(root.join(CLAUDE_SETTINGS_REL)).unwrap(),
            "{not json"
        );
    }

    #[test]
    fn existing_settings_keys_and_hooks_survive() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write(
            &root.join(CLAUDE_SETTINGS_REL),
            "{\n  \"hooks\": {\n    \"Stop\": [\n      {\n        \"hooks\": [\n          {\n            \"type\": \"command\",\n            \"command\": \"spec-kitty session-stop\"\n          }\n        ]\n      }\n    ]\n  },\n  \"permissions\": {\"allow\": [\"Bash\"]}\n}\n",
        );
        apply_healthy_orientation(root, &["claude".to_string()], "4.0.0rc8", "demo").unwrap();
        let value: Value =
            serde_json::from_slice(&std::fs::read(root.join(CLAUDE_SETTINGS_REL)).unwrap())
                .unwrap();
        assert_eq!(value["permissions"]["allow"][0], "Bash");
        assert!(hook_present(
            value.as_object().unwrap(),
            "SessionStart",
            SESSION_START_CMD
        ));
        assert!(hook_present(
            value.as_object().unwrap(),
            "Stop",
            SESSION_STOP_CMD
        ));
    }
}
