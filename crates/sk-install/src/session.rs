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
//!
//! # Read-only decode phases (CF33)
//!
//! The diagnostics half below reuses the reference's two distinct read phases
//! instead of one private UTF-8 shortcut:
//!
//! * an orientation surface is read the way `Path.read_text(encoding="utf-8")`
//!   reads it — strict UTF-8 (a BOM stays a `U+FEFF` character) followed by
//!   universal-newline translation — and a decode failure escapes the probe;
//! * `.claude/settings.json` goes through the one provable decoder
//!   ([`sk_kernel::text_decode::decode_unambiguous`]), so a UTF-16/UTF-32 BOM is
//!   honoured and an unprovable payload is reported as unsafe, exactly as the
//!   reference's `SettingsNotDecodableError` catch does.
//!
//! A read failure that the reference lets escape (`EACCES`, `EIO`, a
//! non-regular settings destination) is propagated by the probe caller, never
//! converted into a `missing`/`unsafe` health finding.

use std::path::{Path, PathBuf};

use sk_kernel::PythonString;
use sk_kernel::python_utf8::PythonUtf8DecodeError;
use sk_kernel::text_decode;

use crate::error::InstallError;

pub mod claude_settings;

use claude_settings::{ClaudeSettingsDocument, ClaudeSettingsMerge};

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
pub enum SessionWriterMode {
    /// Append/replace a managed section in an existing-or-new file.
    Section,
    /// The file *is* the managed section.
    WholeFile,
    /// Append/replace a section and manage `.claude/settings.json` hooks.
    ClaudeCode,
}

/// Where one harness's session-presence artefacts live.
///
/// This is the single read-only projection of the harness roster: the
/// projection below ([`apply_session_presence`]) and the read-only diagnostics
/// (`crate::tool_surface_diagnostics::session_presence`) both consume THIS
/// descriptor, so the harness/path/mode list has exactly one owner and an audit
/// can never invent a second roster (upstream
/// `session_presence.writers.registry`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionWriterDescriptor {
    /// The harness key this descriptor belongs to.
    pub agent_key: &'static str,
    /// The managed Markdown/context/rules path, relative to the project root.
    pub path: &'static str,
    /// The harness root that must already exist before writing, when the rules
    /// file's own parent is not the right guard.
    pub check_dir: Option<&'static str>,
    /// The writer shape.
    pub mode: SessionWriterMode,
}

const fn section(agent_key: &'static str, path: &'static str) -> SessionWriterDescriptor {
    SessionWriterDescriptor {
        agent_key,
        path,
        check_dir: None,
        mode: SessionWriterMode::Section,
    }
}

const fn whole_file(
    agent_key: &'static str,
    path: &'static str,
    check_dir: &'static str,
) -> SessionWriterDescriptor {
    SessionWriterDescriptor {
        agent_key,
        path,
        check_dir: Some(check_dir),
        mode: SessionWriterMode::WholeFile,
    }
}

/// Resolve a harness key to its writer descriptor.
///
/// `None` means upstream's `NullWriter` path (`qwen`, `kilocode`, `auggie`, `q`
/// and any unregistered key): no artefact is managed and no orientation text is
/// written for it.
pub fn session_writer(agent_key: &str) -> Option<SessionWriterDescriptor> {
    match agent_key {
        "claude" => Some(SessionWriterDescriptor {
            agent_key: "claude",
            path: ".claude/CLAUDE.md",
            check_dir: Some(".claude"),
            mode: SessionWriterMode::ClaudeCode,
        }),
        "codex" => Some(section("codex", "AGENTS.md")),
        "opencode" => Some(section("opencode", "AGENTS.md")),
        "antigravity" => Some(section("antigravity", "AGENTS.md")),
        "pi" => Some(section("pi", "AGENTS.md")),
        "vibe" => Some(section("vibe", "AGENTS.md")),
        "letta" => Some(section("letta", "AGENTS.md")),
        "gemini" => Some(section("gemini", "GEMINI.md")),
        "llxprt" => Some(section("llxprt", "LLXPRT.md")),
        // Upstream guards copilot on the harness root (`.github`), not on the
        // rules file's parent, which init only creates later when profiles are
        // projected. A copilot-only fresh init therefore writes no
        // `.github/copilot-instructions.md` in the pinned oracle.
        "copilot" => Some(SessionWriterDescriptor {
            agent_key: "copilot",
            path: ".github/copilot-instructions.md",
            check_dir: Some(".github"),
            mode: SessionWriterMode::Section,
        }),
        "cursor" => Some(whole_file(
            "cursor",
            ".cursor/rules/spec-kitty.mdc",
            ".cursor",
        )),
        "windsurf" => Some(whole_file(
            "windsurf",
            ".windsurf/rules/spec-kitty.md",
            ".windsurf",
        )),
        "kiro" => Some(whole_file("kiro", ".kiro/steering/spec-kitty.md", ".kiro")),
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
            let Some(descriptor) = session_writer(tool) else {
                report.not_applicable.push(tool.to_string());
                return Ok(());
            };
            // repeated tools that write the same target (codex + vibe) are
            // skipped once the target's content is current; the write itself is
            // idempotent.
            match descriptor.mode {
                SessionWriterMode::Section | SessionWriterMode::WholeFile => {
                    if !harness_root_exists(project_root, descriptor.path, descriptor.check_dir) {
                        report.not_applicable.push(tool.to_string());
                        return Ok(());
                    }
                    record_section(
                        &mut report,
                        tool,
                        descriptor.path,
                        write_section(
                            project_root,
                            descriptor.path,
                            content,
                            descriptor.mode == SessionWriterMode::Section,
                        ),
                    );
                }
                SessionWriterMode::ClaudeCode => {
                    let check_dir = descriptor.check_dir.unwrap_or(".claude");
                    // A symlinked `.claude` root is an unowned layout: refuse both
                    // the orientation file and the hook settings instead of
                    // writing through the link (upstream warns and skips).
                    if let Some(link) = first_symlink_component(project_root, check_dir) {
                        report.preserved.push(descriptor.path.to_string());
                        report.warnings.push(format!(
                            "Failed to write orientation for {tool}: Refusing unowned symlink: {}",
                            display_path(project_root, &link)
                        ));
                        return Ok(());
                    }
                    if !project_root.join(check_dir).is_dir() {
                        report.not_applicable.push(tool.to_string());
                        return Ok(());
                    }
                    record_section(
                        &mut report,
                        tool,
                        descriptor.path,
                        write_section(project_root, descriptor.path, content, true),
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

    let decoded = existing
        .as_deref()
        .and_then(|raw| text_decode::decode_unambiguous(raw));
    let document = decoded.as_ref().map(|(text, _encoding)| {
        claude_settings::parse_settings(&PythonString::from_scalars(text))
    });
    // Both hooks already registered is the reference's `changed == False`: no
    // write happens at all, so an operator's bytes (and any comment-free
    // formatting) survive untouched.
    if let Some(ClaudeSettingsDocument::Object(document)) = &document
        && claude_settings::registered_all(document, &REQUIRED_HOOKS)
    {
        return Ok(HookOutcome::Kept);
    }
    if existing.is_some() {
        if is_git_tracked(project_root, CLAUDE_SETTINGS_REL) {
            return Ok(HookOutcome::Preserved(
                ".claude/settings.json is committed, so Spec Kitty did not edit it; add SessionStart/Stop hooks running `spec-kitty session-start` / `spec-kitty session-stop` yourself".to_string(),
            ));
        }
        return match document {
            Some(ClaudeSettingsDocument::Object(document)) => {
                match claude_settings::merge_required(document, &REQUIRED_HOOKS) {
                    // Nothing to add.
                    ClaudeSettingsMerge::Unchanged => Ok(HookOutcome::Kept),
                    // `hooks`, or an event value, is not the shape the reference
                    // replaces; the file is preserved rather than repaired.
                    ClaudeSettingsMerge::NotMergeable => Ok(HookOutcome::Kept),
                    ClaudeSettingsMerge::Merged(merged) => {
                        let bytes = claude_settings::render_settings(&merged);
                        write_settings(&destination, bytes.as_bytes())?;
                        Ok(HookOutcome::Written)
                    }
                }
            }
            // Absent source encoding (an unprovable payload) is preserved, as
            // before: the reference refuses this document too, it just refuses
            // it with `SettingsNotDecodableError`.
            None => Ok(HookOutcome::Preserved(
                ".claude/settings.json is not a JSON object; Spec Kitty left it untouched"
                    .to_string(),
            )),
            Some(ClaudeSettingsDocument::Uncaught(failure)) => Ok(HookOutcome::Preserved(format!(
                "{CLAUDE_SETTINGS_REL} could not be parsed ({}) so Spec Kitty left it untouched",
                failure.detail()
            ))),
            Some(ClaudeSettingsDocument::InvalidJson(_))
            | Some(ClaudeSettingsDocument::NotObject) => Ok(HookOutcome::Preserved(
                ".claude/settings.json is not a JSON object; Spec Kitty left it untouched"
                    .to_string(),
            )),
        };
    }
    write_settings(&destination, canonical_settings())?;
    Ok(HookOutcome::Written)
}

/// The two lifecycle hook commands this writer manages.
const REQUIRED_HOOKS: [(&str, &str); 2] = [
    (SESSION_START_EVENT, SESSION_START_CMD),
    (SESSION_STOP_EVENT, SESSION_STOP_CMD),
];

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

// ---------------------------------------------------------------------------
// Read-only session-presence assessment (the diagnostics seam)
// ---------------------------------------------------------------------------
//
// `doctor tool-surfaces --kind context_file|rule|hook` must report what is on
// disk for the artefacts this module's writer owns WITHOUT ever running that
// writer. Everything below is therefore read-only: it inspects metadata and
// bytes and returns typed observations. The projection above stays the only
// mutator of these paths.

/// The `SessionStart` lifecycle hook event key in `.claude/settings.json`.
pub const SESSION_START_EVENT: &str = "SessionStart";
/// The `Stop` lifecycle hook event key in `.claude/settings.json`.
pub const SESSION_STOP_EVENT: &str = "Stop";

/// The bound the *writer* path applies to a presence destination.
///
/// The read-only assessment below does not use it: the reference's probe reads
/// through `Path.read_text` / `Path.read_bytes` with no size cap, and CF33
/// preserves that source read phase instead of turning an oversized
/// Source-valid file into a `missing` finding. The constant stays public
/// because the projection writer's own bound is unchanged.
pub const PRESENCE_READ_LIMIT: u64 = 4 * 1024 * 1024;

/// The prefix upstream stamps immediately before the orientation version.
const VERSION_PREFIX: &str = "**Spec Kitty v";
/// The suffix that ends the stamped version (and the header line).
const VERSION_SUFFIX: &str = "**";

/// The kind of a managed session-presence artefact (upstream
/// `ToolSurfaceKind`'s session-presence subset).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionArtefactKind {
    /// Always-on orientation context file (`CLAUDE.md`, `AGENTS.md`, `GEMINI.md`, …).
    ContextFile,
    /// Path/glob-activated rule or steering file (`.cursor/rules/…`, `.kiro/steering/…`).
    Rule,
    /// Lifecycle hook entry inside the harness settings file.
    Hook,
}

/// One managed session-presence artefact of a harness.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionArtefact {
    /// What the artefact is.
    pub kind: SessionArtefactKind,
    /// Path relative to the project root.
    pub path: &'static str,
}

/// Classify a Markdown writer target the way upstream `_markdown_kind` does.
///
/// A path/glob-activated *rule* carries one of the rule markers; everything
/// else a Markdown-family writer manages is an always-on context file.
pub fn session_artefact_kind(path: &str) -> SessionArtefactKind {
    let lowered = path.to_ascii_lowercase();
    if ["/rules/", "/steering/", ".mdc"]
        .iter()
        .any(|marker| lowered.contains(marker))
    {
        SessionArtefactKind::Rule
    } else {
        SessionArtefactKind::ContextFile
    }
}

/// The artefacts `agent_key`'s writer manages, in upstream order.
///
/// An empty result means upstream's `NullWriter`: the harness has no verified
/// orientation mechanism, so a read-only audit must report a research gap
/// rather than a health verdict.
pub fn session_artefacts(agent_key: &str) -> Vec<SessionArtefact> {
    let Some(descriptor) = session_writer(agent_key) else {
        return Vec::new();
    };
    if descriptor.mode == SessionWriterMode::ClaudeCode {
        // Upstream `_managed_surfaces` emits the settings path once per
        // lifecycle event, so two identical HOOK instances are planned and
        // probed independently.
        return vec![
            SessionArtefact {
                kind: SessionArtefactKind::ContextFile,
                path: descriptor.path,
            },
            SessionArtefact {
                kind: SessionArtefactKind::Hook,
                path: CLAUDE_SETTINGS_REL,
            },
            SessionArtefact {
                kind: SessionArtefactKind::Hook,
                path: CLAUDE_SETTINGS_REL,
            },
        ];
    }
    vec![SessionArtefact {
        kind: session_artefact_kind(descriptor.path),
        path: descriptor.path,
    }]
}

/// The observed state of an orientation-bearing Markdown surface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OrientationSectionState {
    /// The destination file does not exist.
    FileAbsent,
    /// Legacy bounded-inspection state; the Source reader imposes no regular-file precondition.
    NotRegularFile,
    /// `Path.exists()` failed before Source's read-error catch, so this escapes the audit.
    InspectionFailed(String),
    /// The bytes could not be read; the reference catches this `OSError` as
    /// "not present", so the surface is reported missing.
    Unreadable(String),
    /// The bytes are not decodable as strict UTF-8. The reference's
    /// `read_text(encoding="utf-8")` raises `UnicodeDecodeError` out of the
    /// read-only probe, so a caller must propagate this instead of inventing a
    /// health verdict (CF33).
    Undecodable(PythonUtf8DecodeError),
    /// The file exists but carries no opening orientation marker.
    NoSection,
    /// An opening marker without a closing marker.
    Unterminated {
        /// The version stamp inside the block, when parseable.
        version: Option<String>,
    },
    /// A marker-delimited orientation section.
    Section {
        /// The version stamp inside the block, when parseable.
        version: Option<String>,
    },
}

/// A read-only assessment of one orientation-bearing Markdown surface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrientationAssessment {
    /// The inspected destination path.
    pub path: String,
    /// What was found.
    pub state: OrientationSectionState,
    /// The version the installed content stamps.
    pub expected_version: String,
    /// Whether a present, parseable stamp differs from `expected_version`.
    pub stale: bool,
}

impl OrientationAssessment {
    /// Whether the managed orientation block is present (a stale block is still
    /// present: upstream reports it as `stale`, never as missing).
    pub fn present(&self) -> bool {
        matches!(
            self.state,
            OrientationSectionState::Section { .. } | OrientationSectionState::Unterminated { .. }
        )
    }
}

/// The decoded shape of `.claude/settings.json`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClaudeSettingsShape {
    /// The file does not exist.
    Absent,
    /// The bytes could not be read; the reference lets this `OSError` escape the
    /// provider (`EACCES`, `EIO`, or a non-regular destination), so the caller
    /// must propagate it rather than report a health finding (CF33).
    Unreadable,
    /// The bytes cannot be proven to be a supported encoding
    /// (`decode_unambiguous` returned `None`); the reference catches
    /// `SettingsNotDecodableError` here and reports the hook as unsafe.
    Undecodable,
    /// The text is not valid JSON (the reference's `_load` returns `{}`).
    InvalidJson,
    /// The JSON value is not an object (the reference's `_load` returns `{}`).
    NotObject,
    /// `json.loads` raised a failure that is **not** a `JSONDecodeError`
    /// (CPython's integer-digit `ValueError` or recursion `RecursionError`), so
    /// the reference `_load` has no catch and the failure escapes the provider.
    /// It is never a `missing`/`unsafe` verdict.
    UncaughtJson,
    /// The JSON value is an object (the hook entries may still be missing).
    Object,
}

/// A read-only assessment of the Claude Code lifecycle hooks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaudeHookAssessment {
    /// Project-relative settings path that was inspected.
    pub settings_path: String,
    /// The decoded file shape.
    pub shape: ClaudeSettingsShape,
    /// A `SessionStart` hook runs [`SESSION_START_CMD`].
    pub session_start: bool,
    /// A `Stop` hook runs [`SESSION_STOP_CMD`].
    pub stop: bool,
    /// The lifecycle event keys carry hook entries that are not ours (they are
    /// preserved as-is by the write path).
    pub foreign_entries: bool,
    /// Whether git tracks the settings file; `None` when that was not probed.
    pub tracked: Option<bool>,
    /// The decode/parse problem, when [`shape`](Self::shape) is not
    /// [`ClaudeSettingsShape::Object`].
    pub detail: Option<String>,
}

impl ClaudeHookAssessment {
    /// Whether both lifecycle hooks are registered (upstream
    /// `_claude_hooks_present`).
    pub fn registered(&self) -> bool {
        self.session_start && self.stop
    }
}

/// Assess one orientation-bearing surface against the installed content version.
///
/// `path` is the destination file (absolute, as the surface plan resolved it).
/// Read-only: the destination is `read_text`-read (strict UTF-8, newline
/// translation) and nothing is created, rewritten, chmodded or locked. A
/// non-UTF-8 destination is reported as
/// [`OrientationSectionState::Undecodable`] carrying the reference's exact
/// `UnicodeDecodeError`; a read `OSError` is `Unreadable` (the reference catches
/// it as "not present").
pub fn assess_orientation(path: &Path, expected_version: &str) -> OrientationAssessment {
    let state = orientation_section_state(path);
    let on_disk = match &state {
        OrientationSectionState::Section { version }
        | OrientationSectionState::Unterminated { version } => version.clone(),
        _ => None,
    };
    let stale = on_disk
        .as_deref()
        .is_some_and(|version| version != expected_version);
    OrientationAssessment {
        path: path.display().to_string(),
        state,
        expected_version: expected_version.to_string(),
        stale,
    }
}

fn orientation_section_state(path: &Path) -> OrientationSectionState {
    // Source checks exists OUTSIDE its read-error catch. Reuse the canonical
    // ignored-stat-error policy, then read the actual stream (FIFO/device too).
    match sk_kernel::paths::source_path_exists(path) {
        Ok(false) => return OrientationSectionState::FileAbsent,
        Err(error) => return OrientationSectionState::InspectionFailed(error.to_string()),
        Ok(true) => {}
    }
    let bytes = match sk_kernel::read_source_bytes(path) {
        Ok(bytes) => bytes,
        Err(error) => return OrientationSectionState::Unreadable(error.to_string()),
    };
    // Strict UTF-8 with a BOM kept as U+FEFF, then the same universal-newline
    // translation Python's text mode applies. A decode failure is NOT a health
    // state: it escapes the probe like the reference.
    let text = match sk_kernel::python_utf8::decode_utf8(&bytes) {
        Ok(text) => text_decode::normalize_newlines(text),
        Err(error) => return OrientationSectionState::Undecodable(error),
    };
    let Some(open_at) = text.find(SECTION_OPEN) else {
        return OrientationSectionState::NoSection;
    };
    let rest = &text[open_at..];
    match rest.find(SECTION_CLOSE) {
        None => OrientationSectionState::Unterminated {
            version: orientation_version(rest),
        },
        Some(offset) => OrientationSectionState::Section {
            version: orientation_version(&rest[..offset + SECTION_CLOSE.len()]),
        },
    }
}

/// The version stamped inside an orientation block, when parseable.
///
/// Scoped to the block (the same delimiters the writer uses), so a stray stamp
/// elsewhere in the file is never mistaken for the managed version.
fn orientation_version(section: &str) -> Option<String> {
    let start = section.find(VERSION_PREFIX)? + VERSION_PREFIX.len();
    let end = section[start..].find(VERSION_SUFFIX)? + start;
    let stamped = section[start..end].trim();
    if stamped.is_empty() {
        None
    } else {
        Some(stamped.to_string())
    }
}

/// Observe `.claude/settings.json` and its two lifecycle hooks (read-only).
///
/// [`tracked`](ClaudeHookAssessment::tracked) stays `None` here because proving
/// git ownership spawns `git ls-files`; a read-only surface audit deliberately
/// does not do that. Use [`assess_claude_hooks`] when the preserve policy needs
/// the answer.
pub fn observe_claude_settings(project_root: &Path) -> ClaudeHookAssessment {
    let path = project_root.join(CLAUDE_SETTINGS_REL);
    let mut assessment = ClaudeHookAssessment {
        settings_path: CLAUDE_SETTINGS_REL.to_string(),
        shape: ClaudeSettingsShape::Absent,
        session_start: false,
        stop: false,
        foreign_entries: false,
        tracked: None,
        detail: None,
    };
    // Registrar._load checks exists before _read_settings_text. In particular
    // ENOTDIR/ELOOP at this stage mean absent, not an escaping read failure.
    match sk_kernel::paths::source_path_exists(&path) {
        Ok(false) => return assessment,
        Err(error) => {
            assessment.shape = ClaudeSettingsShape::Unreadable;
            assessment.detail = Some(error.to_string());
            return assessment;
        }
        Ok(true) => {}
    }
    let bytes = match sk_kernel::read_source_bytes(&path) {
        Ok(bytes) => bytes,
        Err(sk_kernel::KernelError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            // Source `Path.exists()` false (including a vanished file or a
            // broken link): the document is absent, not a failure.
            return assessment;
        }
        Err(error) => {
            // Every other read failure escapes the reference provider
            // (`_read_settings_text` only catches FileNotFoundError), so the
            // caller must propagate it (CF33).
            assessment.shape = ClaudeSettingsShape::Unreadable;
            assessment.detail = Some(error.to_string());
            return assessment;
        }
    };
    // Source `kernel.text_decode.decode_unambiguous`: a BOM or strict UTF-8 must
    // prove the encoding, otherwise the reference raises
    // `SettingsNotDecodableError` (reported as unsafe by the provider).
    let Some((text, _source_encoding)) = text_decode::decode_unambiguous(&bytes) else {
        assessment.shape = ClaudeSettingsShape::Undecodable;
        assessment.detail = Some(format!(
            "{CLAUDE_SETTINGS_REL} is not valid UTF-8 and has no byte-order mark"
        ));
        return assessment;
    };
    match claude_settings::parse_settings(&PythonString::from_scalars(&text)) {
        ClaudeSettingsDocument::Object(document) => {
            assessment.shape = ClaudeSettingsShape::Object;
            assessment.session_start =
                claude_settings::registered(&document, SESSION_START_EVENT, SESSION_START_CMD);
            assessment.stop =
                claude_settings::registered(&document, SESSION_STOP_EVENT, SESSION_STOP_CMD);
            assessment.foreign_entries =
                claude_settings::foreign_entries(&document, &REQUIRED_HOOKS);
        }
        ClaudeSettingsDocument::NotObject => {
            assessment.shape = ClaudeSettingsShape::NotObject;
            assessment.detail = Some("expected a JSON object".to_string());
        }
        ClaudeSettingsDocument::InvalidJson(detail) => {
            assessment.shape = ClaudeSettingsShape::InvalidJson;
            assessment.detail = Some(detail);
        }
        ClaudeSettingsDocument::Uncaught(failure) => {
            assessment.shape = ClaudeSettingsShape::UncaughtJson;
            assessment.detail = Some(failure.detail().to_string());
        }
    }
    assessment
}

/// The full hook assessment, including the git-tracked fact the preserve policy
/// uses (upstream `_settings_committed`: a committed team file is never edited).
pub fn assess_claude_hooks(project_root: &Path) -> ClaudeHookAssessment {
    let mut assessment = observe_claude_settings(project_root);
    assessment.tracked = Some(is_git_tracked(project_root, CLAUDE_SETTINGS_REL));
    assessment
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
        let installed = std::fs::read_to_string(root.join(CLAUDE_SETTINGS_REL)).unwrap();
        assert!(installed.contains("\"permissions\""));
        assert!(installed.contains("\"Bash\""));
        let assessment = observe_claude_settings(root);
        assert_eq!(assessment.shape, ClaudeSettingsShape::Object);
        assert!(assessment.session_start);
        assert!(assessment.stop);
    }
}
