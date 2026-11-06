//! Migration-local support for the modern (target `>= 3`) completion effects.
//!
//! Everything here is a *reading* aid or a thin adapter over an existing
//! canonical authority. No second ledger, catalog, manifest, renderer or state
//! machine lives in this module: [agent config] mirrors the pinned
//! `core/agent_config.load_agent_config` shape the migrated Source effects read,
//! [file writers] go through `sk_kernel`'s no-follow writers, `.gitignore`
//! edits go through [`crate::migrations::ignore`], and ownership proofs go
//! through [`crate::preserve`].
//!
//! Two Source reading rules are reproduced here because the modern effects
//! depend on them and the native entry points differ:
//!
//! * `agents.available` falls back to the legacy `tools.available` key (the
//!   2.0.1 rename), an absent section means "no configured agents" (never an
//!   error), and an unknown key is a refusal — exactly upstream's
//!   `load_agent_config`.
//! * the session-presence harness table is the pinned writer registry
//!   (`session_presence/writers/registry.py`), read-only. It is declared here
//!   only so a migration can *observe* presence; every write still goes through
//!   [`crate::session::apply_session_presence`].

use std::path::{Path, PathBuf};

use serde_yaml_ng::Value;

use crate::error::InstallError;
use crate::session::{SECTION_CLOSE, SECTION_OPEN};

/// The mission corpus root.
pub const KITTY_SPECS_DIR: &str = "kitty-specs";
/// The project config path.
pub const CONFIG_REL: &str = ".kittify/config.yaml";
/// The project metadata path.
pub const METADATA_REL: &str = ".kittify/metadata.yaml";
/// The bounded read size for the small project files the migrations inspect.
pub const MAX_SMALL_READ: u64 = 16 * 1024 * 1024;

/// The pinned `AgentConfig` projection the migrations need.
///
/// Source `core/agent_config.AgentConfig`: `auto_commit` defaults to **true**,
/// so a hand-written `Default` is required — a derived one would silently ship
/// `false` and turn every commit path off.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentConfig {
    /// The configured agent keys, in file order.
    pub available: Vec<String>,
    /// Whether agents should auto-commit their bookkeeping work.
    pub auto_commit: bool,
    /// Whether the file listed an explicit `available` key.
    pub explicit_agent_list: bool,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            available: Vec::new(),
            auto_commit: true,
            explicit_agent_list: false,
        }
    }
}

impl AgentConfig {
    /// Whether `key` is configured.
    pub fn has(&self, key: &str) -> bool {
        self.available.iter().any(|candidate| candidate == key)
    }
}

/// `agents.auto_commit`, else top-level `auto_commit`, else `true`.
///
/// A present non-boolean value is `true` (Source: no truthiness coercion, and no
/// second fallback once a value was seen).
fn read_auto_commit(
    section: Option<&serde_yaml_ng::Mapping>,
    root: &serde_yaml_ng::Mapping,
) -> bool {
    let from = |mapping: &serde_yaml_ng::Mapping, key: &str| mapping.get(key).cloned();
    // Source: an explicit `null` in the section is "not set", so the top level
    // is still consulted; the first non-null value decides, and a non-boolean
    // value is `true` (never truthiness-coerced, never a second fallback).
    let candidates = [
        section.and_then(|section| from(section, "auto_commit")),
        from(root, "auto_commit"),
    ];
    for candidate in candidates.into_iter().flatten() {
        if candidate.is_null() {
            continue;
        }
        // A present non-boolean value is `true` (Source: `isinstance(bool)` else
        // True) — `auto_commit: "false"`, `1` and `0` all mean enabled.
        return match candidate {
            Value::Bool(value) => value,
            _ => true,
        };
    }
    true
}

fn config_error(path: &Path, detail: impl Into<String>) -> InstallError {
    InstallError::ConfigInvalid {
        path: path.to_path_buf(),
        detail: detail.into(),
    }
}

/// Read `.kittify/config.yaml` the way the pinned `load_agent_config` does.
///
/// * a missing file is an empty configuration (not an error);
/// * a non-mapping root, a non-mapping `agents`/`tools` section or a
///   non-list `available` is a refusal;
/// * `agents` wins while it carries content, then the legacy `tools` key;
/// * a scalar `available` is a one-element list;
/// * an unknown agent key is a refusal, naming the valid keys.
pub fn load_agent_config(project_root: &Path) -> Result<AgentConfig, InstallError> {
    let path = project_root.join(CONFIG_REL);
    let text = match std::fs::read(&path) {
        Ok(raw) => {
            String::from_utf8(raw).map_err(|error| config_error(&path, error.to_string()))?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(AgentConfig::default());
        }
        Err(source) => return Err(InstallError::io("read", &path, source)),
    };
    let value: Value = serde_yaml_ng::from_str(&text)
        .map_err(|error| config_error(&path, format!("invalid YAML: {error}")))?;
    let Value::Mapping(root) = value else {
        if value.is_null() {
            return Ok(AgentConfig::default());
        }
        return Err(config_error(
            &path,
            "expected a YAML mapping at the top level",
        ));
    };

    let mut section = root.get("agents");
    if section.is_none_or(|agents| {
        // Source: `agents_data is None or agents_data == {}` — an explicit null
        // is "no agents section", so the legacy `tools` key is consulted.
        agents.is_null()
            || agents
                .as_mapping()
                .is_some_and(serde_yaml_ng::Mapping::is_empty)
    }) {
        section = root.get("tools");
    }
    let section = match section {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_mapping()
                .ok_or_else(|| config_error(&path, "expected a YAML mapping for agents"))?,
        ),
    };

    // `auto_commit` is read from the chosen section, then the top level, and a
    // **non-null non-boolean** value is `true` (Source never falls back again
    // after seeing a value). It is settled before the `available` early return,
    // so a config carrying only `auto_commit` still reports it.
    let auto_commit = read_auto_commit(section, &root);

    let Some(section) = section else {
        return Ok(AgentConfig {
            auto_commit,
            ..AgentConfig::default()
        });
    };
    let Some(available) = section.get("available") else {
        return Ok(AgentConfig {
            auto_commit,
            ..AgentConfig::default()
        });
    };
    let explicit_agent_list = true;
    let raw: Vec<String> = match available {
        Value::String(single) => vec![single.clone()],
        Value::Sequence(items) => items
            .iter()
            .map(|item| match item {
                Value::String(key) => Ok(key.clone()),
                other => Err(config_error(
                    &path,
                    format!("agents.available entries must be strings, found {other:?}"),
                )),
            })
            .collect::<Result<_, _>>()?,
        // Source: `available: null` is not a list, so it is a refusal — not an
        // empty list (the recorded matrix fails this case).
        other => {
            return Err(config_error(
                &path,
                format!("agents.available must be a list, found {other:?}"),
            ));
        }
    };

    let mut unknown: Vec<&str> = Vec::new();
    for key in &raw {
        // Config membership is exact in Source; CLI selection aliases belong
        // to the separate Native `agent_surface` capability.
        if !crate::agent::agent_keys().any(|known| known == key) {
            unknown.push(key);
        }
    }
    if !unknown.is_empty() {
        return Err(config_error(
            &path,
            format!(
                "unknown agent key(s): {}. Valid agents: {}",
                unknown.join(", "),
                crate::agent::valid_keys()
            ),
        ));
    }

    Ok(AgentConfig {
        available: raw,
        auto_commit,
        explicit_agent_list,
    })
}

/// The configured agent keys (empty when the project has no config).
pub fn configured_agents(project_root: &Path) -> Result<Vec<String>, InstallError> {
    Ok(load_agent_config(project_root)?.available)
}

/// Every mission directory under `kitty-specs/`, sorted by name.
///
/// Mirrors the pinned `_iter_mission_dirs` (a direct-child enumeration, not a
/// recursive glob) used by every corpus-walking migration.
pub fn mission_dirs(project_root: &Path) -> Result<Vec<PathBuf>, InstallError> {
    let root = project_root.join(KITTY_SPECS_DIR);
    let entries = match std::fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(InstallError::io("read directory", &root, source)),
    };
    let mut missions: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| InstallError::io("read directory", &root, source))?;
        let path = entry.path();
        if path.is_dir() {
            missions.push(path);
        }
    }
    missions.sort();
    Ok(missions)
}

/// The project's canonical slug (identity first, derived name as fallback).
pub fn project_slug(project_root: &Path) -> String {
    crate::identity::resolve_identity(project_root)
        .project_slug
        .unwrap_or_else(|| crate::identity::derive_project_slug(project_root))
}

// ---------------------------------------------------------------------------
// Text-file helpers
// ---------------------------------------------------------------------------

/// Read a small UTF-8 file, or `None` when it does not exist.
pub fn read_optional_text(path: &Path) -> Result<Option<String>, InstallError> {
    match sk_kernel::read_bounded(path, MAX_SMALL_READ) {
        Ok(bytes) => String::from_utf8(bytes)
            .map(Some)
            .map_err(|error| InstallError::io("decode", path, std::io::Error::other(error))),
        Err(sk_kernel::KernelError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            Ok(None)
        }
        Err(error) => Err(InstallError::io(
            "read",
            path,
            std::io::Error::other(error.to_string()),
        )),
    }
}

/// Read a small UTF-8 file without following a final symlink.
///
/// `Ok(None)` when the path does not exist; a symlink (including a dangling
/// one) is a typed refusal, so a migration never reads or clobbers whatever the
/// link points at.
pub fn read_optional_text_no_follow(path: &Path) -> Result<Option<String>, InstallError> {
    match sk_kernel::no_follow::read_text_no_follow(path, MAX_SMALL_READ) {
        Ok(text) => Ok(Some(text)),
        Err(sk_kernel::KernelError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            Ok(None)
        }
        Err(error) => Err(InstallError::UnsafePath {
            path: format!("{} ({error})", path.display()),
        }),
    }
}

/// Whether `text` contains any of `markers`.
pub fn contains_any(text: &str, markers: &[&str]) -> bool {
    markers.iter().any(|marker| text.contains(marker))
}

/// Write a package-generated file, restoring the read-only bit it had before.
///
/// The pinned generator leaves generated command files read-only; a migration
/// that rewrites one must restore the write bit for the write and leave the
/// file read-only again (`write_generated_file(..., read_only=True)`). A
/// planted symlink is refused by the kernel's no-follow writer.
pub fn write_generated_file(
    path: &Path,
    content: &str,
    read_only: bool,
) -> Result<(), InstallError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|source| InstallError::io("create directory", parent, source))?;
    }
    let mode = if read_only { 0o444 } else { 0o644 };
    // A read-only destination would fail the open; clear it first (the pinned
    // writer does the same, then re-applies the mode).
    if let Ok(metadata) = std::fs::symlink_metadata(path)
        && metadata.is_file()
    {
        let current = sk_kernel::no_follow::file_mode(path)
            .map_err(|error| InstallError::io("stat", path, std::io::Error::other(error)))?;
        if current & 0o200 == 0 {
            sk_kernel::no_follow::chmod_no_follow(path, current | 0o200)
                .map_err(|error| InstallError::io("chmod", path, std::io::Error::other(error)))?;
        }
    }
    sk_kernel::no_follow::write_text_no_follow(path, content, mode).map_err(
        |error| match error {
            sk_kernel::KernelError::NoFollow { path, .. } => InstallError::UnsafePath {
                path: path.display().to_string(),
            },
            other => InstallError::io("write", path, std::io::Error::other(other.to_string())),
        },
    )?;
    sk_kernel::no_follow::chmod_no_follow(path, mode)
        .map_err(|error| InstallError::io("chmod", path, std::io::Error::other(error)))
}

/// Whether `path` is proven package-owned by its command version marker.
pub fn proven_package_owned(path: &Path, project_root: &Path) -> bool {
    crate::preserve::Prover::CanonicalContent
        .prove(path, project_root)
        .is_some()
}

// ---------------------------------------------------------------------------
// Session-presence observation
// ---------------------------------------------------------------------------

/// The pinned session-presence writer shape for one harness key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PresenceWriter {
    /// `.claude/CLAUDE.md` section plus the two `settings.json` hooks.
    ClaudeCode,
    /// A managed section appended into a shared file.
    Append {
        /// Project-relative file path.
        path: &'static str,
        /// A harness root that must exist before the file is written.
        check_dir: Option<&'static str>,
    },
    /// The file *is* the managed section.
    Whole {
        /// Project-relative file path.
        path: &'static str,
        /// A harness root that must exist before the file is written.
        check_dir: Option<&'static str>,
    },
}

/// The pinned writer table (`session_presence/writers/registry.py`).
///
/// `None` is the registry's `NullWriter`: the harness has no verified
/// orientation mechanism, so it neither has presence nor accepts a write.
pub fn presence_writer(agent_key: &str) -> Option<PresenceWriter> {
    match agent_key {
        "claude" => Some(PresenceWriter::ClaudeCode),
        "codex" | "opencode" | "antigravity" | "pi" | "vibe" | "letta" => {
            Some(PresenceWriter::Append {
                path: "AGENTS.md",
                check_dir: None,
            })
        }
        "gemini" => Some(PresenceWriter::Append {
            path: "GEMINI.md",
            check_dir: None,
        }),
        "llxprt" => Some(PresenceWriter::Append {
            path: "LLXPRT.md",
            check_dir: None,
        }),
        "copilot" => Some(PresenceWriter::Append {
            path: ".github/copilot-instructions.md",
            check_dir: Some(".github"),
        }),
        "cursor" => Some(PresenceWriter::Whole {
            path: ".cursor/rules/spec-kitty.mdc",
            check_dir: Some(".cursor"),
        }),
        "windsurf" => Some(PresenceWriter::Whole {
            path: ".windsurf/rules/spec-kitty.md",
            check_dir: Some(".windsurf"),
        }),
        "kiro" => Some(PresenceWriter::Whole {
            path: ".kiro/steering/spec-kitty.md",
            check_dir: Some(".kiro"),
        }),
        _ => None,
    }
}

fn writer_rules_path(writer: PresenceWriter) -> &'static str {
    match writer {
        PresenceWriter::ClaudeCode => ".claude/CLAUDE.md",
        PresenceWriter::Append { path, .. } | PresenceWriter::Whole { path, .. } => path,
    }
}

/// Whether the harness root for `writer` exists (`can_write`).
///
/// A writer with no `check_dir` guards on the rules file's real parent — the
/// project root for `AGENTS.md`/`GEMINI.md`/`LLXPRT.md`, which always exists.
pub fn presence_can_write(project_root: &Path, writer: PresenceWriter) -> bool {
    match writer {
        PresenceWriter::ClaudeCode => true,
        PresenceWriter::Append { check_dir, .. } | PresenceWriter::Whole { check_dir, .. } => {
            match check_dir {
                Some(dir) => project_root.join(dir).is_dir(),
                None => true,
            }
        }
    }
}

/// Whether the managed orientation section is present in `path`.
pub fn section_present(path: &Path) -> bool {
    read_optional_text(path)
        .map(|text| text.is_some_and(|text| text.contains(SECTION_OPEN)))
        .unwrap_or(false)
}

/// The `<!-- spec-kitty:orientation -->` … `<!-- /spec-kitty:orientation -->`
/// section of `path`, including a trailing newline when present.
pub fn extract_section(path: &Path) -> Option<String> {
    let text = read_optional_text(path).ok()??;
    let start = text.find(SECTION_OPEN)?;
    let mut end = text.find(SECTION_CLOSE).filter(|end| *end >= start)? + SECTION_CLOSE.len();
    if text[end..].starts_with('\n') {
        end += 1;
    }
    Some(text[start..end].to_string())
}

/// Whether the two pinned Claude Code hooks are registered in `settings.json`.
pub fn claude_hooks_registered(project_root: &Path) -> bool {
    let path = crate::session::claude_settings_path(project_root);
    let Ok(Some(text)) = read_optional_text(&path) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    let Some(hooks) = value.get("hooks").and_then(|hooks| hooks.as_object()) else {
        return false;
    };
    hook_registered(hooks, "SessionStart", crate::session::SESSION_START_CMD)
        && hook_registered(hooks, "Stop", crate::session::SESSION_STOP_CMD)
}

fn hook_registered(
    hooks: &serde_json::Map<String, serde_json::Value>,
    event: &str,
    command: &str,
) -> bool {
    let Some(groups) = hooks.get(event).and_then(|groups| groups.as_array()) else {
        return false;
    };
    groups.iter().any(|group| {
        group
            .get("hooks")
            .and_then(|hooks| hooks.as_array())
            .is_some_and(|hooks| {
                hooks.iter().any(|hook| {
                    hook.get("command").and_then(|value| value.as_str()) == Some(command)
                })
            })
    })
}

/// Whether `agent_key` already has the session-presence artefacts.
pub fn has_presence(project_root: &Path, agent_key: &str) -> bool {
    let Some(writer) = presence_writer(agent_key) else {
        return false;
    };
    let rules = project_root.join(writer_rules_path(writer));
    if !section_present(&rules) {
        return false;
    }
    if writer == PresenceWriter::ClaudeCode {
        return claude_hooks_registered(project_root);
    }
    true
}

// ---------------------------------------------------------------------------
// Ledger settlement and kitty-specs residue
// ---------------------------------------------------------------------------

/// The ledger-adoption backfill id (the only `ledger_adoption` migration).
pub const LEDGER_ADOPTION_ID: &str = crate::migrations::ledger_adoption::BACKFILL_ID;

/// Whether selection would treat `migration_id` as already settled.
///
/// The same predicate the upgrade selector uses: a `success` record in the
/// authoritative partition, or a pending ledger-adoption backfill that will
/// settle the id. A residue-safe successor asks this about its predecessor
/// before re-walking the project (the predecessor's own outcome decides whether
/// the repair still needs to run).
pub fn settled_in_selection_view(
    project_root: &Path,
    migration_id: &str,
) -> Result<bool, InstallError> {
    let catalog = crate::ledger::MigrationCatalog::embedded()?;
    let Some(spec) = catalog.get(migration_id) else {
        return Ok(false);
    };
    let ledger = crate::ledger::MigrationLedger::new(project_root);
    if ledger.is_settled(spec)? {
        return Ok(true);
    }
    let adopting = match catalog.get(LEDGER_ADOPTION_ID) {
        Some(spec) if migration_id != LEDGER_ADOPTION_ID && !ledger.is_settled(spec)? => {
            crate::migrations::ledger_adoption::adoption_settled_ids(project_root)?
        }
        _ => std::collections::BTreeSet::new(),
    };
    Ok(adopting.contains(migration_id))
}

/// Mission marker files whose existence keeps a `kitty-specs/` child a Mission.
const MISSION_MARKER_FILES: [&str; 2] = ["spec.md", "meta.json"];

/// Mission content whose presence keeps a child out of residue in file-presence mode.
const MISSION_CONTENT: [&str; 8] = [
    "spec.md",
    "meta.json",
    "plan.md",
    "tasks.md",
    "tasks",
    "status.events.jsonl",
    "status.json",
    "lanes.json",
];

/// The operator-facing residue notice (`RESIDUE_NOTICE` in the pinned resolver).
pub fn residue_notice(relative_posix: &str) -> String {
    format!(
        "not a Mission (residue directory {relative_posix}: no mission_id, and no spec.md or meta.json \
         on disk or in git); left untouched. To make it a Mission, add a meta.json with a mission_id \
         (for example, re-create it with `spec-kitty agent mission create`), or remove the folder."
    )
}

/// The `tracked=` probe: project-relative paths git tracks under `kitty-specs/`.
///
/// `None` means "probed, no usable git" (the pinned helper degrades a broken
/// index to the file-presence mode rather than crashing the walk).
pub fn tracked_mission_paths(project_root: &Path) -> Option<std::collections::BTreeSet<String>> {
    let output = crate::migrations::git::run(
        project_root,
        &[
            "--literal-pathspecs",
            "ls-files",
            "-z",
            "--",
            KITTY_SPECS_DIR,
        ],
    )
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut tracked = std::collections::BTreeSet::new();
    for entry in output.stdout.split(|byte| *byte == 0) {
        if entry.is_empty() {
            continue;
        }
        tracked.insert(String::from_utf8_lossy(entry).into_owned());
    }
    Some(tracked)
}

fn markers_exist(folder: &Path) -> bool {
    MISSION_MARKER_FILES
        .iter()
        .any(|name| folder.join(name).exists())
}

fn carries_identity(folder: &Path) -> bool {
    let path = folder.join("meta.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Ok(serde_json::Value::Object(object)) = serde_json::from_str(&text) else {
        return false;
    };
    object
        .get("mission_id")
        .and_then(|value| value.as_str())
        .is_some_and(|value| !value.trim().is_empty())
}

fn markers_tracked(
    folder: &Path,
    project_root: &Path,
    tracked: Option<&std::collections::BTreeSet<String>>,
) -> bool {
    let Some(tracked) = tracked else {
        return markers_exist(folder);
    };
    MISSION_MARKER_FILES.iter().any(|name| {
        crate::preserve::relative_posix(&folder.join(name), project_root)
            .is_some_and(|relative| tracked.contains(&relative))
    })
}

fn is_mission_dir(
    folder: &Path,
    project_root: &Path,
    tracked: Option<&std::collections::BTreeSet<String>>,
) -> bool {
    carries_identity(folder) || markers_tracked(folder, project_root, tracked)
}

/// Whether a `kitty-specs/` child is residue the walk must leave untouched.
pub fn is_kitty_specs_residue(
    folder: &Path,
    project_root: &Path,
    tracked: Option<&std::collections::BTreeSet<String>>,
) -> bool {
    if folder
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        != Some(KITTY_SPECS_DIR)
        || markers_exist(folder)
    {
        return false;
    }
    if tracked.is_some_and(|tracked| !tracked.is_empty()) {
        return !is_mission_dir(folder, project_root, tracked);
    }
    if MISSION_CONTENT
        .iter()
        .any(|name| folder.join(name).exists())
    {
        return false;
    }
    !is_mission_dir(folder, project_root, tracked)
}

/// Split the `kitty-specs/` children into `(missions, residue)`, both sorted.
pub fn partition_kitty_specs(
    project_root: &Path,
) -> Result<(Vec<PathBuf>, Vec<PathBuf>), InstallError> {
    let tracked = tracked_mission_paths(project_root);
    let mut missions = Vec::new();
    let mut residue = Vec::new();
    for folder in mission_dirs(project_root)? {
        if is_kitty_specs_residue(&folder, project_root, tracked.as_ref()) {
            residue.push(folder);
        } else {
            missions.push(folder);
        }
    }
    Ok((missions, residue))
}

/// One residue notice per folder, project-relative POSIX spelling.
pub fn residue_warnings(residue: &[PathBuf], project_root: &Path) -> Vec<String> {
    residue
        .iter()
        .filter_map(|folder| crate::preserve::relative_posix(folder, project_root))
        .map(|relative| residue_notice(&relative))
        .collect()
}

// ---------------------------------------------------------------------------
// Comment-preserving YAML key deletion
// ---------------------------------------------------------------------------

/// Delete the nested key `parent.key` from a YAML document, preserving every
/// other byte.
///
/// The pinned migration uses a `ruamel.yaml` round-trip load/`del`/dump, which
/// keeps unrelated keys, ordering and comments. `serde_yaml_ng` cannot
/// round-trip comments, so this edits lines and then *proves* the edit: the
/// re-parsed document must equal the original mapping with exactly that nested
/// key removed. Anything else (a flow mapping that shares a line with sibling
/// keys, an unexpected indent shape) is a typed refusal rather than a
/// silently altered config.
pub fn delete_nested_key(
    path: &Path,
    text: &str,
    parent: &str,
    key: &str,
) -> Result<Option<String>, InstallError> {
    let error = |detail: String| InstallError::YamlEdit {
        path: path.to_path_buf(),
        detail,
    };

    let value: Value =
        serde_yaml_ng::from_str(text).map_err(|source| error(format!("invalid YAML: {source}")))?;
    let Value::Mapping(root) = &value else {
        return Err(error("root must be a mapping".to_string()));
    };
    let Some(Value::Mapping(section)) = root.get(parent) else {
        return Ok(None);
    };
    if !section.contains_key(key) {
        return Ok(None);
    }
    let mut expected = root.clone();
    if let Some(Value::Mapping(section)) = expected.get_mut(parent) {
        section.remove(key);
    }
    let expected_value = Value::Mapping(expected);

    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let parent_index = lines
        .iter()
        .position(|line| top_level_key(line, parent))
        .ok_or_else(|| error(format!("could not locate the top-level {parent:?} key")))?;

    // The child block runs until the next line that starts at or before the
    // parent's own indentation (the next top-level key).
    let mut child_indent: Option<usize> = None;
    let mut key_index: Option<usize> = None;
    for (index, line) in lines.iter().enumerate().skip(parent_index + 1) {
        let stripped = line.trim_end_matches(['\n', '\r']);
        if stripped.trim().is_empty() {
            continue;
        }
        let indent = indent_of(stripped);
        if indent == 0 {
            break;
        }
        if child_indent.is_none() {
            child_indent = Some(indent);
        }
        if indent == child_indent.unwrap_or(0) && yaml_key_of(stripped.trim_start()) == Some(key) {
            key_index = Some(index);
            break;
        }
    }
    let key_index = key_index.ok_or_else(|| {
        error(format!(
            "could not locate {parent}.{key} as its own YAML block; refusing a partial edit"
        ))
    })?;
    let child_indent = child_indent.unwrap_or(0);

    let mut end = key_index + 1;
    while end < lines.len() {
        let stripped = lines[end].trim_end_matches(['\n', '\r']);
        if stripped.trim().is_empty() {
            // A blank line belongs to the block only when a deeper line follows.
            let mut lookahead = end + 1;
            while lookahead < lines.len() && lines[lookahead].trim().is_empty() {
                lookahead += 1;
            }
            let deeper = lines
                .get(lookahead)
                .is_some_and(|line| indent_of(line.trim_end_matches(['\n', '\r'])) > child_indent);
            if !deeper {
                break;
            }
            end = lookahead;
            continue;
        }
        if indent_of(stripped) <= child_indent {
            break;
        }
        end += 1;
    }

    let mut edited = String::with_capacity(text.len());
    // Whether any sibling of the removed key survives in the parent block.
    let mut siblings = 0usize;
    for (index, line) in lines.iter().enumerate() {
        if index >= key_index && index < end {
            continue;
        }
        if index > parent_index && index < lines.len() {
            let stripped = line.trim_end_matches(['\n', '\r']);
            if !stripped.trim().is_empty() && indent_of(stripped) > 0 {
                siblings += 1;
            }
        }
        edited.push_str(line);
    }
    if siblings == 0 {
        // An emptied mapping serializes as `parent: {}` (what a round-trip
        // dumper writes), never as `parent:` (which would read back as null).
        let mut collapsed = String::with_capacity(edited.len() + 3);
        for (index, line) in edited.split_inclusive('\n').enumerate() {
            if index == parent_index {
                let newline = if line.ends_with('\n') { "\n" } else { "" };
                collapsed.push_str(&format!("{parent}: {{}}{newline}"));
                continue;
            }
            collapsed.push_str(line);
        }
        edited = collapsed;
    }

    let reparsed: Value = serde_yaml_ng::from_str(&edited)
        .map_err(|source| error(format!("edited document no longer parses: {source}")))?;
    if reparsed != expected_value {
        return Err(error(format!(
            "refusing a line edit that would not equal removing {parent}.{key}"
        )));
    }
    Ok(Some(edited))
}

/// Insert or replace one **top-level** scalar key, preserving every other byte.
///
/// `crate::yaml_edit` edits one level *below* a named parent; `.kittify/config.yaml`
/// also carries top-level keys (`env_file:`), so this is the same narrow
/// line-edit-plus-semantic-proof approach applied at the document root: the
/// re-parsed document must equal the original mapping with exactly that key set.
/// A present, non-empty value is returned unchanged (`changed == false`): an
/// authored pointer is never overwritten.
pub fn upsert_top_level_scalar(
    path: &Path,
    text: &str,
    key: &str,
    value: &str,
) -> Result<(String, bool), InstallError> {
    let error = |detail: String| InstallError::YamlEdit {
        path: path.to_path_buf(),
        detail,
    };
    let parsed_value: Value =
        serde_yaml_ng::from_str(text).map_err(|source| error(format!("invalid YAML: {source}")))?;
    let parsed = if parsed_value.is_null() {
        Value::Mapping(serde_yaml_ng::Mapping::new())
    } else {
        parsed_value
    };
    if !parsed.is_mapping() {
        return Err(error("root must be a mapping".to_string()));
    }
    let mut expected = parsed.clone();
    {
        let mapping = expected.as_mapping_mut().expect("checked mapping");
        if mapping
            .get(key)
            .is_some_and(|existing| !existing.is_null() && existing.as_str() != Some(""))
        {
            return Ok((text.to_string(), false));
        }
        mapping.insert(
            Value::String(key.to_string()),
            Value::String(value.to_string()),
        );
    }

    let rendered = format!("{key}: {value}");
    let edited = if let Some(line_index) = text
        .split_inclusive('\n')
        .position(|line| top_level_key(line, key))
    {
        let mut lines: Vec<String> = text.split_inclusive('\n').map(str::to_string).collect();
        let had_newline = lines[line_index].ends_with('\n');
        lines[line_index] = if had_newline {
            format!("{rendered}\n")
        } else {
            rendered
        };
        lines.concat()
    } else {
        let mut edited = text.to_string();
        if !edited.is_empty() && !edited.ends_with('\n') {
            edited.push('\n');
        }
        edited.push_str(&rendered);
        edited.push('\n');
        edited
    };

    let reparsed: Value = serde_yaml_ng::from_str(&edited)
        .map_err(|source| error(format!("edited document no longer parses: {source}")))?;
    if reparsed != expected {
        return Err(error(format!(
            "refusing a line edit that would not equal setting {key:?}"
        )));
    }
    Ok((edited, true))
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

fn top_level_key(line: &str, key: &str) -> bool {
    let Some(rest) = line.strip_prefix(key) else {
        return false;
    };
    if indent_of(line) != 0 {
        return false;
    }
    rest.starts_with(':')
}

fn yaml_key_of(trimmed: &str) -> Option<&str> {
    let (key, _) = trimmed.split_once(':')?;
    let key = key.trim();
    if key.is_empty() || key.starts_with('-') || key.starts_with('#') {
        return None;
    }
    Some(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn missing_config_is_an_empty_agent_list() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(
            load_agent_config(directory.path()).unwrap(),
            AgentConfig::default()
        );
    }

    #[test]
    fn tools_key_is_the_legacy_fallback_and_scalar_is_one_agent() {
        let directory = tempfile::tempdir().unwrap();
        write(
            &directory.path().join(CONFIG_REL),
            "tools:\n  available: gemini\n",
        );
        let config = load_agent_config(directory.path()).unwrap();
        assert_eq!(config.available, vec!["gemini".to_string()]);
        assert!(config.explicit_agent_list);
    }

    #[test]
    fn unknown_agent_keys_are_refused() {
        let directory = tempfile::tempdir().unwrap();
        write(
            &directory.path().join(CONFIG_REL),
            "agents:\n  available:\n    - not-an-agent\n",
        );
        let error = load_agent_config(directory.path()).unwrap_err();
        assert_eq!(error.code(), "config_invalid");
    }

    #[test]
    fn nested_key_deletion_preserves_other_keys_and_proves_the_result() {
        let text = "agents:\n  available:\n    - claude\n  auto_commit: true\n  selection:\n    preferred_implementer: a\n    preferred_reviewer: b\n  lint_on_edit: false\nvcs:\n  type: git\n";
        let edited = delete_nested_key(Path::new("config.yaml"), text, "agents", "selection")
            .unwrap()
            .expect("edit");
        assert!(edited.contains("  auto_commit: true\n"));
        assert!(edited.contains("  lint_on_edit: false\n"));
        assert!(!edited.contains("selection"));
        let value: Value = serde_yaml_ng::from_str(&edited).unwrap();
        assert!(value.get("agents").unwrap().get("selection").is_none());
    }

    #[test]
    fn nested_key_deletion_is_a_noop_when_absent() {
        let text = "agents:\n  available:\n    - claude\n";
        assert_eq!(
            delete_nested_key(Path::new("config.yaml"), text, "agents", "selection").unwrap(),
            None
        );
    }
}
