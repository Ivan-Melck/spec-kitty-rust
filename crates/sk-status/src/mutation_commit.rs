//! Auto-commit configuration, canonical review-cycle artifacts and the verdict
//! durability value.
//!
//! This is the **pure** half of the native primary/non-owned commit lifecycle:
//! it reads project configuration, renders a canonical `review-cycle-N.md`
//! artifact and describes the durability of a verdict write. It never runs Git
//! and never shells out — the Git adapter remains the only commit seam — so
//! `sk-status` keeps its bounded-context dependency edge.
//!
//! # Sources
//!
//! - `specify_cli/core/agent_config.py::load_agent_config` /
//!   `get_auto_commit_default` (the project default, `agents` then `tools`, then
//!   the top level, booleans only, missing file → `True`);
//! - `specify_cli/review/artifacts.py::ReviewCycleArtifact` (canonical
//!   `affected_files`, `cycle_number`, `mission_slug`, `reproduction_command`,
//!   `reviewed_at`, `reviewer_agent`, `wp_id` frontmatter, exclusive create);
//! - `specify_cli/review/cycle.py::create_rejected_review_cycle` (next-number
//!   allocation over every consulted sibling directory, `review-cycle://`
//!   pointer, `local_only`/`durable`/`persistence_failed` outcomes);
//! - `specify_cli/cli/commands/agent/tasks_verdict_persistence.py`
//!   (`VerdictPersistenceOutcome` invariants and the two documented commit
//!   messages).
//!
//! User-authored YAML is parsed with `serde_yaml_ng` and never reserialised.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// Repository-relative path of the project configuration.
pub const CONFIG_REL_PATH: &str = ".kittify/config.yaml";

/// The one bounded read limit for the project configuration (1 MiB).
pub const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

/// The filename prefix of a canonical review-cycle artifact.
pub const REVIEW_CYCLE_PREFIX: &str = "review-cycle-";

/// The canonical review-cycle pointer scheme.
pub const REVIEW_CYCLE_SCHEME: &str = "review-cycle://";

/// The pinned non-durable reason for `--no-auto-commit`.
pub const DURABILITY_REASON_NO_AUTO_COMMIT: &str = "no_auto_commit";

/// The pinned non-durable reason for a protected target branch.
pub const DURABILITY_REASON_PROTECTED_TARGET_BRANCH: &str = "protected_target_branch";

/// Why the project configuration could not be read.
#[derive(Debug, Error)]
pub enum CommitConfigError {
    /// The configured file exists but could not be read within the bound.
    #[error("could not read {path}: {source}")]
    Io {
        /// The configuration path.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: io::Error,
    },
    /// The file is not valid YAML.
    #[error("invalid YAML in {path}: {detail}")]
    InvalidYaml {
        /// The configuration path.
        path: PathBuf,
        /// The YAML parser diagnostic.
        detail: String,
    },
    /// The top level (or the `agents`/`tools` section) is not a mapping.
    #[error("invalid config shape in {path}: expected a YAML mapping, got {found}")]
    InvalidShape {
        /// The configuration path.
        path: PathBuf,
        /// A short description of the observed value.
        found: String,
    },
}

/// The `agents.auto_commit` project default.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectCommitConfig {
    /// Whether agents should auto-commit status changes.
    pub auto_commit: bool,
}

impl Default for ProjectCommitConfig {
    fn default() -> Self {
        // `load_agent_config`'s `AgentConfig` default and its missing-file arm
        // both answer `True`.
        Self { auto_commit: true }
    }
}

impl ProjectCommitConfig {
    /// Resolve the effective decision: an explicit flag wins over the project
    /// default (`get_auto_commit_default` if the flag is `None`).
    pub fn resolve(&self, flag: Option<bool>) -> bool {
        flag.unwrap_or(self.auto_commit)
    }
}

/// Read the project's auto-commit default from `.kittify/config.yaml`.
///
/// Mirrors `load_agent_config`:
///
/// - a missing file answers the default (`auto_commit: true`);
/// - the document must be a mapping;
/// - the `agents` section wins, `tools` is the 2.0.1 fallback, and the top
///   level is consulted when neither carries `auto_commit`;
/// - only a YAML boolean counts; any other value falls back to `true`;
/// - a non-mapping `agents`/`tools` section is a typed configuration error.
pub fn read_auto_commit_config(repo_root: &Path) -> Result<ProjectCommitConfig, CommitConfigError> {
    let path = repo_root.join(CONFIG_REL_PATH);
    let bytes = match sk_kernel::read_bounded(&path, MAX_CONFIG_BYTES) {
        Ok(bytes) => bytes,
        Err(error) => {
            let source = io::Error::other(error.to_string());
            if !path.exists() {
                // A missing config is the documented default, not an error.
                return Ok(ProjectCommitConfig::default());
            }
            return Err(CommitConfigError::Io { path, source });
        }
    };
    let document: serde_yaml_ng::Value =
        serde_yaml_ng::from_slice(&bytes).map_err(|error| CommitConfigError::InvalidYaml {
            path: path.clone(),
            detail: error.to_string(),
        })?;
    let root = match document {
        serde_yaml_ng::Value::Null => serde_yaml_ng::Mapping::new(),
        serde_yaml_ng::Value::Mapping(mapping) => mapping,
        other => {
            return Err(CommitConfigError::InvalidShape {
                path,
                found: yaml_type_name(&other).to_owned(),
            });
        }
    };
    let mut section = "agents";
    let agents = root.get(serde_yaml_ng::Value::String(section.to_owned()));
    let mut section_value = agents;
    if section_value.is_none_or(|value| {
        matches!(value, serde_yaml_ng::Value::Null)
            || matches!(value, serde_yaml_ng::Value::Mapping(mapping) if mapping.is_empty())
    }) {
        section = "tools";
        section_value = root.get(serde_yaml_ng::Value::String(section.to_owned()));
    }
    let section_mapping = match section_value {
        None | Some(serde_yaml_ng::Value::Null) => None,
        Some(serde_yaml_ng::Value::Mapping(mapping)) => Some(mapping),
        Some(other) => {
            return Err(CommitConfigError::InvalidShape {
                path,
                found: format!("{section}: {}", yaml_type_name(other)),
            });
        }
    };
    let lookup = |mapping: &serde_yaml_ng::Mapping, key: &str| -> Option<bool> {
        match mapping.get(serde_yaml_ng::Value::String(key.to_owned())) {
            Some(serde_yaml_ng::Value::Bool(value)) => Some(*value),
            _ => None,
        }
    };
    let auto_commit = section_mapping
        .as_ref()
        .and_then(|mapping| lookup(mapping, "auto_commit"))
        .or_else(|| lookup(&root, "auto_commit"))
        .unwrap_or(true);
    Ok(ProjectCommitConfig { auto_commit })
}

fn yaml_type_name(value: &serde_yaml_ng::Value) -> &'static str {
    match value {
        serde_yaml_ng::Value::Null => "null",
        serde_yaml_ng::Value::Bool(_) => "a boolean",
        serde_yaml_ng::Value::Number(_) => "a number",
        serde_yaml_ng::Value::String(_) => "a string",
        serde_yaml_ng::Value::Sequence(_) => "a list",
        serde_yaml_ng::Value::Mapping(_) => "a mapping",
        serde_yaml_ng::Value::Tagged(_) => "a tagged value",
    }
}

/// One file a review cycle affected (`AffectedFile`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AffectedFile {
    /// Repository-relative path.
    pub path: String,
    /// An optional `start-end` line range.
    pub line_range: Option<String>,
}

/// A canonical `review-cycle-N.md` artifact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewCycleArtifact {
    /// The allocated cycle number (>= 1).
    pub cycle_number: u32,
    /// The work-package id (`WP01`).
    pub wp_id: String,
    /// The Mission slug.
    pub mission_slug: String,
    /// The reviewer identity recorded in the artifact.
    pub reviewer_agent: String,
    /// The ISO-8601 review timestamp.
    pub reviewed_at: String,
    /// Files the review touched.
    pub affected_files: Vec<AffectedFile>,
    /// The command that reproduces the decision, when the caller has one.
    pub reproduction_command: Option<String>,
    /// The markdown body (the reviewer's feedback, or the synthesized approval
    /// line).
    pub body: String,
    /// An operator/arbiter override actor, when one was stamped.
    pub override_actor: Option<String>,
    /// An operator/arbiter override reason, when one was stamped.
    pub override_reason: Option<String>,
}

/// Why a canonical review-cycle artifact could not be written.
#[derive(Debug, Error)]
pub enum ReviewCycleError {
    /// A path segment is not a safe single directory/file component.
    #[error("{field} is not a safe path segment: {value:?}")]
    UnsafeSegment {
        /// The offending field.
        field: &'static str,
        /// The offending value.
        value: String,
    },
    /// A sibling file matches the glob but not the `review-cycle-N.md` grammar.
    #[error(
        "cannot allocate the next review cycle: {name:?} in {directory} is not a review-cycle-N.md filename"
    )]
    UnparseableSibling {
        /// The directory holding the sibling.
        directory: PathBuf,
        /// The unparseable filename.
        name: String,
    },
    /// The cycle number space is exhausted.
    #[error("cannot allocate the next review cycle: cycle number overflow")]
    NumberOverflow,
    /// The artifact already exists and is never replaced.
    #[error("review-cycle artifact already exists: {path}")]
    Existing {
        /// The existing path.
        path: PathBuf,
    },
    /// The body is empty.
    #[error("Review feedback body is empty")]
    EmptyBody,
    /// A filesystem operation failed.
    #[error("could not write {path}: {source}")]
    Io {
        /// The path being written or read.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
}

/// The canonical per-work-package review-cycle directory
/// (`kitty-specs/<slug>/tasks/<wp_slug>`).
pub fn review_cycle_dir(feature_dir: &Path, wp_slug: &str) -> PathBuf {
    feature_dir.join("tasks").join(wp_slug)
}

/// The canonical `review-cycle://<slug>/<wp_slug>/<filename>` pointer.
pub fn review_cycle_pointer(
    mission_slug: &str,
    wp_slug: &str,
    filename: &str,
) -> Result<String, ReviewCycleError> {
    validate_segment("mission_slug", mission_slug)?;
    validate_segment("wp_slug", wp_slug)?;
    validate_segment("filename", filename)?;
    if parse_cycle_number(filename).is_none() {
        return Err(ReviewCycleError::UnsafeSegment {
            field: "filename",
            value: filename.to_owned(),
        });
    }
    Ok(format!(
        "{REVIEW_CYCLE_SCHEME}{mission_slug}/{wp_slug}/{filename}"
    ))
}

/// The next free cycle number across every consulted directory.
///
/// Mirrors `ReviewCycleArtifact.next_cycle_number`: the number is
/// `max(parsed) + 1` over the union, and a `review-cycle-*.md` sibling whose
/// number cannot be parsed refuses instead of inflating a count.
pub fn next_review_cycle_number(directories: &[PathBuf]) -> Result<u32, ReviewCycleError> {
    let mut numbers: Vec<u32> = Vec::new();
    for directory in directories {
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(ReviewCycleError::Io {
                    path: directory.clone(),
                    source,
                });
            }
        };
        for entry in entries {
            let entry = entry.map_err(|source| ReviewCycleError::Io {
                path: directory.clone(),
                source,
            })?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if !name.starts_with(REVIEW_CYCLE_PREFIX) || !name.ends_with(".md") {
                continue;
            }
            let Some(number) = parse_cycle_number(name) else {
                return Err(ReviewCycleError::UnparseableSibling {
                    directory: directory.clone(),
                    name: name.to_owned(),
                });
            };
            numbers.push(number);
        }
    }
    match numbers.into_iter().max() {
        None => Ok(1),
        Some(max) => max.checked_add(1).ok_or(ReviewCycleError::NumberOverflow),
    }
}

fn parse_cycle_number(name: &str) -> Option<u32> {
    let middle = name
        .strip_prefix(REVIEW_CYCLE_PREFIX)?
        .strip_suffix(".md")?;
    if middle.is_empty() || !middle.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    middle.parse::<u32>().ok().filter(|number| *number >= 1)
}

/// The strict artifact filename grammar (`_REVIEW_CYCLE_FILE_RE`:
/// `^review-cycle-(?P<cycle>[1-9][0-9]*)\.md$`).
///
/// Distinct from `parse_cycle_number`, which also tolerates a leading zero
/// for allocation compatibility; the provenance guard uses the reference's
/// stricter spelling.
pub fn is_review_cycle_filename(name: &str) -> bool {
    let Some(middle) = name
        .strip_prefix(REVIEW_CYCLE_PREFIX)
        .and_then(|rest| rest.strip_suffix(".md"))
    else {
        return false;
    };
    let mut bytes = middle.bytes();
    match bytes.next() {
        Some(first) if first.is_ascii_digit() && first != b'0' => {}
        _ => return false,
    }
    bytes.all(|byte| byte.is_ascii_digit())
}

/// Why a reviewer-supplied `--review-feedback-file` is one of this WP's own
/// prior review-cycle artifacts (`_guard_feedback_source_provenance`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FeedbackSourceProvenance {
    /// The source lives in the WP's review-cycle directory under the artifact
    /// filename grammar (path identity).
    PriorArtifactPath,
    /// The source's content parses as a `ReviewCycleArtifact` (content identity).
    PriorArtifactContent,
}

impl FeedbackSourceProvenance {
    /// The canonical refusal text for `source_name`.
    pub fn refusal(self, source_name: &str) -> String {
        match self {
            FeedbackSourceProvenance::PriorArtifactPath => format!(
                "feedback_source is this WP's own review-cycle artifact ({source_name}); pass the underlying reviewer feedback instead of a prior review-cycle artifact."
            ),
            FeedbackSourceProvenance::PriorArtifactContent => format!(
                "feedback_source content parses as a review-cycle artifact ({source_name}); pass distinct reviewer feedback instead of a prior review-cycle artifact's content."
            ),
        }
    }
}

/// Check `feedback_source` against the two independent provenance legs.
///
/// Mirrors `review/cycle.py::_guard_feedback_source_provenance`:
///
/// - **path identity** — the resolved source's parent is the resolved
///   `sub_artifact_dir` and its filename matches the strict
///   `review-cycle-N.md` grammar (leading zero refused, like the reference);
/// - **content identity** — the file text parses as a `ReviewCycleArtifact`
///   (frontmatter mapping with the required artifact fields). Plain reviewer
///   prose never parses, so an honest re-report of the same words is admitted.
///
/// The legs are independent: a hand-edited artifact at an artifact-shaped path
/// is still refused by the path leg, and an artifact copy at another path is
/// refused by the content leg. An unreadable source is not a provenance
/// refusal (the caller's own read already reported it).
pub fn review_feedback_provenance(
    feedback_source: &Path,
    sub_artifact_dir: &Path,
    feedback_text: &str,
) -> Option<FeedbackSourceProvenance> {
    if let Some(parent) = feedback_source.parent() {
        let source_parent = normalized_absolute(parent)?;
        let directory = normalized_absolute(sub_artifact_dir)?;
        let name = feedback_source.file_name().and_then(|name| name.to_str());
        if source_parent == directory && name.is_some_and(is_review_cycle_filename) {
            return Some(FeedbackSourceProvenance::PriorArtifactPath);
        }
    }
    parses_as_review_cycle_artifact(feedback_text)
        .then_some(FeedbackSourceProvenance::PriorArtifactContent)
}

fn normalized_absolute(path: &Path) -> Option<PathBuf> {
    match fs::canonicalize(path) {
        Ok(path) => Some(path),
        // `Path.resolve()` in the reference is non-strict: a not-yet-created
        // directory still yields its absolute spelling.
        Err(error) if error.kind() == io::ErrorKind::NotFound => std::path::absolute(path).ok(),
        Err(_) => None,
    }
}

/// True when `text` parses as `ReviewCycleArtifact.from_file` would parse it.
///
/// The required frontmatter fields and their types are the pinned
/// `ReviewCycleArtifact.from_dict` contract; an absent `affected_files` defaults
/// to an empty list and an absent `reproduction_command` to `None`.
pub fn parses_as_review_cycle_artifact(text: &str) -> bool {
    let Some(rest) = text.strip_prefix("---") else {
        return false;
    };
    // `from_file`: an optional newline after the opening delimiter.
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    let Some(closing) = rest.find("\n---") else {
        return false;
    };
    let frontmatter = &rest[..closing];
    let Ok(serde_yaml_ng::Value::Mapping(mapping)) =
        serde_yaml_ng::from_str::<serde_yaml_ng::Value>(frontmatter)
    else {
        return false;
    };
    let field = |key: &str| mapping.get(serde_yaml_ng::Value::String(key.to_owned()));
    let non_empty_string = |key: &str| matches!(field(key), Some(serde_yaml_ng::Value::String(value)) if !value.is_empty());
    let cycle_number_ok = match field("cycle_number") {
        Some(serde_yaml_ng::Value::Number(number)) => {
            number.as_u64().is_some_and(|number| number >= 1)
        }
        Some(serde_yaml_ng::Value::String(value)) => {
            value.parse::<u64>().is_ok_and(|number| number >= 1)
        }
        _ => false,
    };
    if !cycle_number_ok
        || !non_empty_string("wp_id")
        || !non_empty_string("mission_slug")
        || !non_empty_string("reviewer_agent")
        || !non_empty_string("reviewed_at")
    {
        return false;
    }
    match field("reproduction_command") {
        None | Some(serde_yaml_ng::Value::Null | serde_yaml_ng::Value::String(_)) => {}
        Some(_) => return false,
    }
    match field("affected_files") {
        None => true,
        Some(serde_yaml_ng::Value::Sequence(entries)) => entries.iter().all(|entry| {
            let serde_yaml_ng::Value::Mapping(entry) = entry else {
                return false;
            };
            let path = entry.get(serde_yaml_ng::Value::String("path".to_owned()));
            if !matches!(path, Some(serde_yaml_ng::Value::String(value)) if !value.is_empty()) {
                return false;
            }
            matches!(
                entry.get(serde_yaml_ng::Value::String("line_range".to_owned())),
                None | Some(serde_yaml_ng::Value::Null | serde_yaml_ng::Value::String(_))
            )
        }),
        Some(_) => false,
    }
}

fn validate_segment(field: &'static str, value: &str) -> Result<(), ReviewCycleError> {
    let safe = !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains(['/', '\\'])
        && !value.chars().any(|character| character.is_control());
    if safe {
        Ok(())
    } else {
        Err(ReviewCycleError::UnsafeSegment {
            field,
            value: value.to_owned(),
        })
    }
}

impl ReviewCycleArtifact {
    /// The canonical filename (`review-cycle-<n>.md`).
    pub fn filename(&self) -> String {
        format!("{REVIEW_CYCLE_PREFIX}{}.md", self.cycle_number)
    }

    /// The canonical `review-cycle://` pointer for this artifact.
    pub fn pointer(&self, wp_slug: &str) -> Result<String, ReviewCycleError> {
        review_cycle_pointer(&self.mission_slug, wp_slug, &self.filename())
    }

    /// The exact bytes the pinned `ReviewCycleArtifact.write` produces.
    ///
    /// The layout is authored here (not reserialised from user YAML): a
    /// `---`-delimited frontmatter block with the pinned key order, followed by
    /// an empty line and the body when one is present.
    pub fn render(&self) -> String {
        let mut frontmatter = String::new();
        if self.affected_files.is_empty() {
            frontmatter.push_str("affected_files: []\n");
        } else {
            frontmatter.push_str("affected_files:\n");
            for affected in &self.affected_files {
                frontmatter.push_str("- path: ");
                frontmatter.push_str(&yaml_scalar(&affected.path));
                frontmatter.push('\n');
                if let Some(line_range) = &affected.line_range {
                    frontmatter.push_str("  line_range: ");
                    frontmatter.push_str(&yaml_scalar(line_range));
                    frontmatter.push('\n');
                }
            }
        }
        frontmatter.push_str(&format!("cycle_number: {}\n", self.cycle_number));
        frontmatter.push_str("mission_slug: ");
        frontmatter.push_str(&yaml_scalar(&self.mission_slug));
        frontmatter.push('\n');
        frontmatter.push_str("reproduction_command:");
        // ruamel's round-trip dump renders `None` as an empty value.
        if let Some(command) = &self.reproduction_command {
            frontmatter.push(' ');
            frontmatter.push_str(&yaml_scalar(command));
        }
        frontmatter.push('\n');
        frontmatter.push_str("reviewed_at: ");
        frontmatter.push_str(&yaml_scalar(&self.reviewed_at));
        frontmatter.push('\n');
        frontmatter.push_str("reviewer_agent: ");
        frontmatter.push_str(&yaml_scalar(&self.reviewer_agent));
        frontmatter.push('\n');
        frontmatter.push_str("wp_id: ");
        frontmatter.push_str(&yaml_scalar(&self.wp_id));
        frontmatter.push('\n');
        if let Some(actor) = &self.override_actor {
            frontmatter.push_str("review_artifact_override_actor: ");
            frontmatter.push_str(&yaml_scalar(actor));
            frontmatter.push('\n');
        }
        if let Some(reason) = &self.override_reason {
            frontmatter.push_str("review_artifact_override_reason: ");
            frontmatter.push_str(&yaml_scalar(reason));
            frontmatter.push('\n');
        }
        let mut content = format!("---\n{frontmatter}---\n");
        if !self.body.is_empty() {
            content.push('\n');
            content.push_str(&self.body);
        }
        content
    }

    /// Write this artifact into `directory` with an exclusive create.
    ///
    /// A file that already exists is never replaced (the baseline
    /// `ReviewCycleArtifact.write` opens with `"xb"`); a write failure removes
    /// the just-created file so a later allocation cannot mistake it for a
    /// legitimate prior cycle.
    pub fn write_new(&self, directory: &Path) -> Result<PathBuf, ReviewCycleError> {
        if self.body.trim().is_empty() {
            return Err(ReviewCycleError::EmptyBody);
        }
        validate_segment("mission_slug", &self.mission_slug)?;
        validate_segment("wp_id", &self.wp_id)?;
        fs::create_dir_all(directory).map_err(|source| ReviewCycleError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
        let path = directory.join(self.filename());
        let content = self.render();
        use std::io::Write;
        // A failed exclusive create owns nothing. In particular, an Existing
        // error must never flow through cleanup of another writer's artifact.
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|source| {
                if source.kind() == io::ErrorKind::AlreadyExists {
                    ReviewCycleError::Existing { path: path.clone() }
                } else {
                    ReviewCycleError::Io {
                        path: path.clone(),
                        source,
                    }
                }
            })?;
        let result = file
            .write_all(content.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|source| ReviewCycleError::Io {
                path: path.clone(),
                source,
            });
        drop(file);
        if result.is_err() {
            // Only this call's partially written file is ours to unwind.
            // Callers hold the Mission lock across allocation and this write.
            let _ = fs::remove_file(&path);
        }
        result.map(|()| path)
    }
}

/// Render one YAML scalar with the pinned round-trip spelling.
///
/// Ordinary identities (`alice`, `WP01`, `034-demo`) stay plain; a value that
/// would change meaning as a plain scalar (a leading indicator, `": "`, `" #"`,
/// a boolean/null spelling, a numeric token, or a control character) is
/// single-quoted with `''` escaping. This renders only the artifact this module
/// authors; it never rewrites user YAML.
fn yaml_scalar(value: &str) -> String {
    if needs_quotes(value) {
        if value.chars().any(|character| character.is_control()) {
            let mut escaped = String::with_capacity(value.len() + 2);
            escaped.push('"');
            for character in value.chars() {
                match character {
                    '"' => escaped.push_str("\\\""),
                    '\\' => escaped.push_str("\\\\"),
                    '\n' => escaped.push_str("\\n"),
                    '\r' => escaped.push_str("\\r"),
                    '\t' => escaped.push_str("\\t"),
                    other => escaped.push(other),
                }
            }
            escaped.push('"');
            escaped
        } else {
            format!("'{}'", value.replace('\'', "''"))
        }
    } else {
        value.to_owned()
    }
}

fn needs_quotes(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    let first = value.chars().next().unwrap_or(' ');
    if !first.is_ascii_alphanumeric() {
        return true;
    }
    if value
        .chars()
        .any(|character| character.is_control() || character == '\u{7f}')
    {
        return true;
    }
    if value.ends_with(char::is_whitespace) {
        return true;
    }
    if value.contains(": ") || value.ends_with(':') || value.contains(" #") {
        return true;
    }
    let lowered = value.to_ascii_lowercase();
    if matches!(
        lowered.as_str(),
        "null" | "~" | "true" | "false" | "yes" | "no" | "on" | "off"
    ) {
        return true;
    }
    if value.parse::<i64>().is_ok() || value.parse::<f64>().is_ok() {
        return true;
    }
    // Date-like tokens (`2026-11-03T00:00:00Z`) are quoted by the reference
    // dumper to keep them strings; a cycle line range such as `1-3` stays
    // plain.
    let bytes = value.as_bytes();
    bytes.len() > 4 && bytes[..4].iter().all(u8::is_ascii_digit) && bytes[4] == b'-'
}

/// A verdict write's durability classification (`VerdictPersistenceOutcome`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DurabilityClassification {
    /// The evidence was committed and verified.
    Durable,
    /// The evidence was written locally without a commit (`--no-auto-commit`).
    LocalOnly,
    /// A commit was attempted and did not verifiably land.
    PersistenceFailed,
}

impl DurabilityClassification {
    /// The pinned wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            DurabilityClassification::Durable => "durable",
            DurabilityClassification::LocalOnly => "local_only",
            DurabilityClassification::PersistenceFailed => "persistence_failed",
        }
    }
}

/// The typed durability of one review-cycle verdict write.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerdictDurabilityOutcome {
    /// The classification.
    pub classification: DurabilityClassification,
    /// `true` only for a verified durable commit.
    pub verdict_durably_persisted: bool,
    /// The artifact path relative to the committing checkout.
    pub evidence_ref: Option<String>,
    /// The destination ref a durable write landed on.
    pub destination_ref: Option<String>,
    /// The stable non-durable reason, absent for a durable outcome.
    pub reason: Option<String>,
    /// The operator-facing explanation.
    pub message: String,
}

impl VerdictDurabilityOutcome {
    /// The `--no-auto-commit` outcome: written locally, never committed.
    pub fn local_only(evidence_ref: impl Into<String>) -> Self {
        Self {
            classification: DurabilityClassification::LocalOnly,
            verdict_durably_persisted: false,
            evidence_ref: Some(evidence_ref.into()),
            destination_ref: None,
            reason: Some(DURABILITY_REASON_NO_AUTO_COMMIT.to_owned()),
            message: "Review-cycle evidence was written locally without auto-commit.".to_owned(),
        }
    }

    /// A verified durable outcome (commit landed and readback matched).
    pub fn durable(evidence_ref: impl Into<String>, destination_ref: impl Into<String>) -> Self {
        Self {
            classification: DurabilityClassification::Durable,
            verdict_durably_persisted: true,
            evidence_ref: Some(evidence_ref.into()),
            destination_ref: Some(destination_ref.into()),
            reason: None,
            message: "Review-cycle evidence is committed and verified.".to_owned(),
        }
    }

    /// A persisted-locally-but-not-verified outcome.
    pub fn persistence_failed(
        evidence_ref: Option<String>,
        destination_ref: Option<String>,
        reason: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            classification: DurabilityClassification::PersistenceFailed,
            verdict_durably_persisted: false,
            evidence_ref,
            destination_ref,
            reason: Some(reason.into()),
            message: message.into(),
        }
    }

    /// True only for a verified durable commit.
    pub fn is_durable(&self) -> bool {
        self.classification == DurabilityClassification::Durable && self.verdict_durably_persisted
    }
}

/// The canonical status-search commit message for a status-only commit.
pub fn status_commit_message(mission_dir_name: &str) -> String {
    format!("chore(spec-kitty): status transition {mission_dir_name}")
}

/// The canonical review-cycle commit message.
pub fn review_cycle_commit_message(
    cycle_number: u32,
    verdict: &str,
    wp_id: &str,
    mission_slug: &str,
) -> String {
    format!("chore: Record review-cycle-{cycle_number} ({verdict}) for {wp_id} on {mission_slug}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact(body: &str) -> ReviewCycleArtifact {
        ReviewCycleArtifact {
            cycle_number: 1,
            wp_id: "WP01".to_owned(),
            mission_slug: "034-cli-mutation-fixture".to_owned(),
            reviewer_agent: "alice".to_owned(),
            reviewed_at: "2026-11-03T00:00:00Z".to_owned(),
            affected_files: Vec::new(),
            reproduction_command: None,
            body: body.to_owned(),
            override_actor: None,
            override_reason: None,
        }
    }

    #[test]
    fn the_rendered_artifact_matches_the_pinned_reader_shape() {
        // Oracle: `ReviewCycleArtifact(cycle_number=1, wp_id="WP01",
        // mission_slug="034-cli-mutation-fixture", reviewer_agent="alice",
        // reviewed_at="2026-11-03T00:00:00Z", affected_files=[],
        // reproduction_command=None, body="**Issue**: broken\n").write(...)`
        // produced exactly these bytes under the pinned `.venv-rust-oracle`.
        let rendered = artifact("**Issue**: broken\n").render();
        assert_eq!(
            rendered,
            "---\naffected_files: []\ncycle_number: 1\nmission_slug: 034-cli-mutation-fixture\nreproduction_command:\nreviewed_at: '2026-11-03T00:00:00Z'\nreviewer_agent: alice\nwp_id: WP01\n---\n\n**Issue**: broken\n"
        );
    }

    #[test]
    fn affected_files_and_reproduction_command_follow_the_pinned_order() {
        let mut value = artifact("Approved by Jane Doe (model-x): approval:WP01");
        value.cycle_number = 2;
        value.reviewer_agent = "Jane Doe (model-x)".to_owned();
        value.affected_files = vec![AffectedFile {
            path: "src/a.rs".to_owned(),
            line_range: Some("1-3".to_owned()),
        }];
        value.reproduction_command = Some(
            "spec-kitty agent tasks move-task WP01 --to approved --mission 034-cli-mutation-fixture --agent \"Jane Doe (model-x)\""
                .to_owned(),
        );
        let rendered = value.render();
        assert!(
            rendered.contains(
                "affected_files:\n- path: src/a.rs\n  line_range: 1-3\ncycle_number: 2\n"
            )
        );
        assert!(rendered.contains(
            "mission_slug: 034-cli-mutation-fixture\nreproduction_command: spec-kitty agent tasks move-task WP01 --to approved --mission 034-cli-mutation-fixture --agent \"Jane Doe (model-x)\"\nreviewed_at: '2026-11-03T00:00:00Z'\n"
        ));
    }

    #[test]
    fn allocation_takes_the_max_over_every_consulted_directory() {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        fs::write(a.join("review-cycle-1.md"), "").unwrap();
        fs::write(b.join("review-cycle-4.md"), "").unwrap();
        assert_eq!(
            next_review_cycle_number(&[a.clone(), b.clone()]).unwrap(),
            5
        );
        // An absent directory contributes nothing (the baseline's tolerance for
        // a not-yet-created coordination surface).
        assert_eq!(
            next_review_cycle_number(&[temp.path().join("absent")]).unwrap(),
            1
        );
        fs::write(b.join("review-cycle-final.md"), "").unwrap();
        assert!(matches!(
            next_review_cycle_number(&[a, b]),
            Err(ReviewCycleError::UnparseableSibling { .. })
        ));
    }

    #[test]
    fn write_new_is_exclusive_and_unwinds_a_failed_body() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("tasks/WP01");
        let path = artifact("feedback\n").write_new(&dir).unwrap();
        assert!(path.ends_with("tasks/WP01/review-cycle-1.md"));
        let original = fs::read(&path).unwrap();
        assert!(matches!(
            artifact("different feedback\n").write_new(&dir),
            Err(ReviewCycleError::Existing { .. })
        ));
        assert_eq!(
            fs::read(&path).unwrap(),
            original,
            "a losing create owns no file to remove"
        );
        let empty = artifact("").write_new(&dir).unwrap_err();
        assert!(matches!(empty, ReviewCycleError::EmptyBody));
    }

    #[test]
    fn the_pointer_is_canonical_and_refuses_unsafe_segments() {
        let value = artifact("x");
        assert_eq!(
            value.pointer("WP01").unwrap(),
            "review-cycle://034-cli-mutation-fixture/WP01/review-cycle-1.md"
        );
        assert!(matches!(
            review_cycle_pointer("../escape", "WP01", "review-cycle-1.md"),
            Err(ReviewCycleError::UnsafeSegment { .. })
        ));
        assert!(matches!(
            review_cycle_pointer("034", "WP01", "review-cycle-final.md"),
            Err(ReviewCycleError::UnsafeSegment { .. })
        ));
    }

    #[test]
    fn config_resolution_matches_the_pinned_precedence() {
        let temp = tempfile::tempdir().unwrap();
        // Missing file → the documented default.
        assert_eq!(
            read_auto_commit_config(temp.path()).unwrap(),
            ProjectCommitConfig { auto_commit: true }
        );

        let write = |text: &str| {
            fs::create_dir_all(temp.path().join(".kittify")).unwrap();
            fs::write(temp.path().join(CONFIG_REL_PATH), text).unwrap();
        };
        write("agents: {auto_commit: false}\n");
        assert!(!read_auto_commit_config(temp.path()).unwrap().auto_commit);
        // `tools` is the 2.0.1 fallback when `agents` is absent/empty.
        write("agents: {}\ntools: {auto_commit: false}\n");
        assert!(!read_auto_commit_config(temp.path()).unwrap().auto_commit);
        // The top level is consulted only when neither section carries it.
        write("auto_commit: false\n");
        assert!(!read_auto_commit_config(temp.path()).unwrap().auto_commit);
        // A non-boolean value falls back to `true`.
        write("agents: {auto_commit: \"false\"}\n");
        assert!(read_auto_commit_config(temp.path()).unwrap().auto_commit);
        // The explicit flag always wins.
        write("agents: {auto_commit: true}\n");
        let config = read_auto_commit_config(temp.path()).unwrap();
        assert!(!config.resolve(Some(false)));
        assert!(config.resolve(None));
    }

    #[test]
    fn a_non_mapping_config_is_a_typed_refusal() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join(".kittify")).unwrap();
        fs::write(temp.path().join(CONFIG_REL_PATH), "- just\n- a list\n").unwrap();
        assert!(matches!(
            read_auto_commit_config(temp.path()),
            Err(CommitConfigError::InvalidShape { .. })
        ));
        fs::write(temp.path().join(CONFIG_REL_PATH), "agents: [1, 2]\n").unwrap();
        assert!(matches!(
            read_auto_commit_config(temp.path()),
            Err(CommitConfigError::InvalidShape { .. })
        ));
        fs::write(temp.path().join(CONFIG_REL_PATH), "a: [\n").unwrap();
        assert!(matches!(
            read_auto_commit_config(temp.path()),
            Err(CommitConfigError::InvalidYaml { .. })
        ));
    }

    #[test]
    fn review_cycle_filename_grammar_is_the_strict_reference_spelling() {
        assert!(is_review_cycle_filename("review-cycle-1.md"));
        assert!(is_review_cycle_filename("review-cycle-12.md"));
        assert!(!is_review_cycle_filename("review-cycle-0.md"));
        assert!(!is_review_cycle_filename("review-cycle-01.md"));
        assert!(!is_review_cycle_filename("review-cycle-.md"));
        assert!(!is_review_cycle_filename("review-cycle-1.txt"));
        assert!(!is_review_cycle_filename("review-cycle-final.md"));
    }

    #[test]
    fn a_canonical_artifact_parses_and_plain_prose_does_not() {
        let rendered = artifact("The prior review finding.\n").render();
        assert!(parses_as_review_cycle_artifact(&rendered));
        // The reference's FR-004 narrowing: prose never parses, even when it is
        // byte-identical to a prior artifact's body.
        assert!(!parses_as_review_cycle_artifact(
            "The prior review finding.\n"
        ));
        // A frontmatter mapping missing a required field is not an artifact.
        assert!(!parses_as_review_cycle_artifact(
            "---\naffected_files: []\ncycle_number: 1\n---\n\nbody\n"
        ));
        // A non-mapping frontmatter is not an artifact.
        assert!(!parses_as_review_cycle_artifact(
            "---\n- one\n- two\n---\n\nbody\n"
        ));
    }

    #[test]
    fn the_provenance_guard_covers_path_and_content_identity() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("tasks/WP01");
        let prior = artifact("prior finding\n").write_new(&directory).unwrap();
        let text = fs::read_to_string(&prior).unwrap();
        assert_eq!(
            review_feedback_provenance(&prior, &directory, &text),
            Some(FeedbackSourceProvenance::PriorArtifactPath)
        );
        // Content identity alone: a copy outside the artifact directory.
        let copy = temp.path().join("review-cycle-copy.md");
        fs::write(&copy, &text).unwrap();
        assert_eq!(
            review_feedback_provenance(&copy, &directory, &text),
            Some(FeedbackSourceProvenance::PriorArtifactContent)
        );
        // Plain prose at a non-artifact path is admitted.
        let prose = temp.path().join("feedback.md");
        fs::write(&prose, "prior finding\n").unwrap();
        assert_eq!(
            review_feedback_provenance(&prose, &directory, "prior finding\n"),
            None
        );
        // Path identity alone: an artifact-shaped name whose bytes were edited
        // to prose is still refused by the path leg.
        let edited = directory.join("review-cycle-2.md");
        fs::write(&edited, "hand-edited prose\n").unwrap();
        assert_eq!(
            review_feedback_provenance(&edited, &directory, "hand-edited prose\n"),
            Some(FeedbackSourceProvenance::PriorArtifactPath)
        );
    }

    #[test]
    fn the_durability_invariants_match_the_pinned_model() {
        let local = VerdictDurabilityOutcome::local_only("kitty-specs/x/review-cycle-1.md");
        assert!(!local.is_durable());
        assert_eq!(
            local.reason.as_deref(),
            Some(DURABILITY_REASON_NO_AUTO_COMMIT)
        );
        let durable = VerdictDurabilityOutcome::durable("a.md", "main");
        assert!(durable.is_durable());
        assert_eq!(durable.reason, None);
        let failed = VerdictDurabilityOutcome::persistence_failed(
            Some("a.md".to_owned()),
            Some("main".to_owned()),
            "commit_error",
            "boom",
        );
        assert!(!failed.is_durable());
        assert_eq!(failed.classification.as_str(), "persistence_failed");
    }

    #[test]
    fn commit_messages_match_the_pinned_spelling() {
        assert_eq!(
            review_cycle_commit_message(2, "rejected", "WP01", "034-demo"),
            "chore: Record review-cycle-2 (rejected) for WP01 on 034-demo"
        );
        assert_eq!(
            status_commit_message("034-demo"),
            "chore(spec-kitty): status transition 034-demo"
        );
    }
}
