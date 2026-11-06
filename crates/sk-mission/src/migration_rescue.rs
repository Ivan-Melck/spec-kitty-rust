//! Source-compatible legacy Mission rescue and identity/ownership normalization.
//!
//! This is the Mission domain half of the four rescue/cutover upgrade effects
//! (`3.0.0_canonical_context`, `3.2.0a4_normalize_mission_lifecycle` and the
//! `runtime_state_backfill` cutover flip). It owns only work that belongs to
//! Mission identity and `meta.json` authority:
//!
//! * [`backfill_mission_identity`] — the idempotent `mission_id` mint plus the
//!   legacy `mission_number` type coercion;
//! * [`backfill_wp_identity`] — `mission_id` / `wp_code` / `work_package_id`
//!   onto every `tasks/WP*.md` frontmatter;
//! * [`backfill_ownership`] — the Source inference for `execution_mode` /
//!   `owned_files` / `authoritative_surface`, written only where absent;
//! * [`strip_mutable_fields`] — the runtime field eviction, recording the
//!   pre-strip `lane` values the rebuild needs;
//! * [`rebuild_mission_event_log`] — the reconciled legacy → event-log rebuild;
//! * [`normalize_mission_lifecycle`] — the per-mission lifecycle normalization;
//! * [`cutover_mission`] / [`cutover_repo`] — seed → fail-closed verify → the
//!   `status_phase` flip, the sole writer of that `meta.json` key.
//!
//! Every write reuses a canonical authority: the status event log is appended
//! through `sk_status` (one reducer, one lock, one writer), `meta.json` is
//! written atomically through the kernel with the same canonical bytes the
//! Mission creator writes, and WP frontmatter is edited in place with the body
//! and untouched keys preserved, then re-parsed and semantically verified.
//! No second parser, codec, reducer, lock or event writer exists here.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::topology::MissionTopology;
use serde_json::{Map as JsonMap, Value};
use sk_status::migration_backfill::{
    RuntimeBackfillAction, RuntimeVerifyOutcome, SNAPSHOT_AUTHORITY_PHASE, STATUS_PHASE_KEY,
};

pub mod corpus;
pub mod tracked_meta;
pub mod wp_frontmatter;

/// Runtime-state frontmatter fields the eviction removes (Source `MUTABLE_FIELDS`).
pub const MUTABLE_FIELDS: [&str; 15] = [
    "lane",
    "review_status",
    "reviewed_by",
    "review_feedback",
    "progress",
    "shell_pid",
    "assignee",
    "agent",
    "shell_pid_created_at",
    "review_artifact_override_at",
    "review_artifact_override_actor",
    "review_artifact_override_wp_id",
    "review_artifact_override_reason",
    "reviewer_shell_pid",
    "history",
];

/// The canonical `tasks/` WP prompt directory name.
pub const TASKS_DIR_NAME: &str = "tasks";

/// The canonical event-log file name (Source `status.events.jsonl`).
pub const EVENTS_FILE_NAME: &str = "status.events.jsonl";

/// The legacy derived snapshot name.
pub const STATUS_FILE_NAME: &str = "status.json";

/// The derived-views root under `.kittify/`.
pub const DERIVED_DIR_NAME: &str = "derived";

/// A lock wait bound for migration writes.
const RESCUE_LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Why a rescue step refused to continue.
#[derive(Debug, thiserror::Error)]
pub enum RescueError {
    /// A Mission artifact cannot be read or parsed.
    #[error("{0}")]
    Unreadable(String),
    /// `meta.json` is malformed.
    #[error("corrupt json: {0}")]
    CorruptMeta(String),
    /// A legacy value cannot be coerced without guessing.
    #[error("{0}")]
    Uncoercible(String),
    /// A frontmatter edit would have changed the document beyond the intent.
    #[error("{0}")]
    FrontmatterEdit(String),
    /// The canonical lock or writer refused the operation.
    #[error("{0}")]
    Writer(String),
    /// The status domain reported a typed refusal.
    #[error("{0}")]
    Status(String),
    /// Git could not answer the tracked-copy question.
    #[error("{0}")]
    Git(String),
}

fn kernel(path: &Path, error: sk_kernel::KernelError) -> RescueError {
    RescueError::Unreadable(format!("{}: {error}", path.display()))
}

// ---------------------------------------------------------------------------
// Canonical frontmatter editor (in-place, body-preserving, verified)
// ---------------------------------------------------------------------------

/// One parsed `tasks/WP*.md` document, split for an in-place frontmatter edit.
struct FrontmatterDoc {
    yaml: String,
    body: String,
}

impl FrontmatterDoc {
    fn parse(text: &str) -> Result<Self, RescueError> {
        // Source reads a WP document with `utf-8-sig`: a leading BOM is encoding
        // metadata, not frontmatter content, and a write drops it.
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let split = crate::frontmatter::split_frontmatter(text)
            .map_err(|error| RescueError::Unreadable(error.to_string()))?;
        Ok(Self {
            yaml: split.yaml.to_owned(),
            body: split.body.to_owned(),
        })
    }

    /// Render this document through the ONE Source-compatible emitter.
    ///
    /// `is_work_package` is true for a `tasks/WP*.md` file (Source's `read`
    /// injects `dependencies: []` for those only). Key order, the injected
    /// empty sequence and every authored comment/style come from
    /// [`wp_frontmatter::render_normalized`], never from a private formatter.
    fn render(&self, is_work_package: bool) -> String {
        let mut yaml = self.yaml.clone();
        if !yaml.is_empty() && !yaml.ends_with('\n') {
            yaml.push('\n');
        }
        let yaml = wp_frontmatter::render_normalized(&yaml, is_work_package);
        format!("---\n{yaml}---\n{}", self.body)
    }

    fn mapping(&self) -> Result<serde_yaml_ng::Mapping, RescueError> {
        let parsed: serde_yaml_ng::Value =
            serde_yaml_ng::from_str(&self.yaml).map_err(|error| {
                RescueError::Unreadable(format!("frontmatter is not valid YAML: {error}"))
            })?;
        match parsed {
            serde_yaml_ng::Value::Mapping(mapping) => Ok(mapping),
            serde_yaml_ng::Value::Null => Ok(serde_yaml_ng::Mapping::new()),
            _ => Err(RescueError::Unreadable(
                "frontmatter is not a mapping".to_string(),
            )),
        }
    }

    /// Set one top-level scalar, replacing any existing value block.
    ///
    /// A trailing `# comment` authored on the key's own line survives the
    /// replacement: the touched line keeps everything from the `#` on. Nothing
    /// else on the line block is rewritten, so comments, quoting and key order
    /// of every other line are untouched, and the caller re-parses the result
    /// and proves only the intended keys changed.
    fn set_scalar(yaml: &str, key: &str, rendered: &str) -> String {
        match top_level_block(yaml, key) {
            Some((start, end)) => {
                let trailing = trailing_comment(&yaml[start..end]);
                let mut out = String::with_capacity(yaml.len());
                out.push_str(&yaml[..start]);
                out.push_str(key);
                out.push_str(": ");
                out.push_str(rendered);
                if let Some(comment) = trailing {
                    out.push_str(&comment);
                }
                out.push('\n');
                out.push_str(&yaml[end..]);
                out
            }
            None => {
                let mut out = yaml.to_string();
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(key);
                out.push_str(": ");
                out.push_str(rendered);
                out.push('\n');
                out
            }
        }
    }

    /// Set one top-level block sequence.
    fn set_sequence(yaml: &str, key: &str, items: &[String]) -> String {
        let mut rendered = String::new();
        rendered.push_str(key);
        rendered.push_str(":\n");
        for item in items {
            rendered.push_str("- ");
            rendered.push_str(&scalar(item));
            rendered.push('\n');
        }
        match top_level_block(yaml, key) {
            Some((start, end)) => {
                let mut out = String::with_capacity(yaml.len() + rendered.len());
                out.push_str(&yaml[..start]);
                out.push_str(&rendered);
                out.push_str(&yaml[end..]);
                out
            }
            None => {
                let mut out = yaml.to_string();
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(&rendered);
                out
            }
        }
    }

    /// Remove one top-level key and its value block.
    fn remove_key(yaml: &str, key: &str) -> String {
        match top_level_block(yaml, key) {
            Some((start, end)) => {
                let mut out = String::with_capacity(yaml.len());
                out.push_str(&yaml[..start]);
                out.push_str(&yaml[end..]);
                out
            }
            None => yaml.to_string(),
        }
    }
}

/// `(start, end)` byte span of the top-level `key:` block inside `yaml`.
///
/// The block is the key line plus every following line that continues it: an
/// indented line, a column-0 block-sequence item (`- value`, the style both the
/// ruamel and `serde_yaml_ng` writers emit for `indent(offset=0)`), or a blank
/// line. The block ends at the next top-level key or column-0 comment, so a
/// neighbouring key's leading comment is never removed.
fn top_level_block(yaml: &str, key: &str) -> Option<(usize, usize)> {
    let mut offset = 0usize;
    let mut start: Option<usize> = None;
    for line in yaml.split_inclusive('\n') {
        let line_start = offset;
        offset += line.len();
        let content = line.trim_end_matches(['\r', '\n']);
        if let Some(block_start) = start {
            let continues = content.is_empty()
                || content.starts_with(' ')
                || content.starts_with('\t')
                || content.starts_with("- ")
                || content == "-";
            if continues {
                continue;
            }
            return Some((block_start, line_start));
        }
        if top_level_key_of(content) == Some(key) {
            start = Some(line_start);
        }
    }
    start.map(|block_start| (block_start, yaml.len()))
}

/// The authored trailing `# comment` of a key's own line, including the `#`.
///
/// A YAML comment starts at a `#` that begins the value or follows whitespace;
/// a `#` inside a scalar (`key: a#b`, `key: 'a # b'`) is data, so those are
/// left alone. A quoted value is skipped entirely for that reason.
fn trailing_comment(block: &str) -> Option<String> {
    let first = block.lines().next()?;
    let (_, value) = first.split_once(':')?;
    if value.contains('\'') || value.contains('"') {
        return None;
    }
    let bytes = value.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'#' {
            continue;
        }
        let starts_comment = index == 0
            || value[..index]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        if starts_comment {
            // Keep the authored whitespace run before the `#`, so the comment
            // column is stable rather than re-spaced.
            let mut start = index;
            while start > 0 && value.as_bytes()[start - 1].is_ascii_whitespace() {
                start -= 1;
            }
            return Some(value[start..].trim_end().to_string());
        }
    }
    None
}

pub(crate) fn top_level_key_of(line: &str) -> Option<&str> {
    if line.starts_with(' ')
        || line.starts_with('\t')
        || line.starts_with('#')
        || line.starts_with('-')
        || line.starts_with("---")
    {
        return None;
    }
    let (key, _) = line.split_once(':')?;
    if key.is_empty()
        || !key.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '_' || character == '-'
        })
    {
        return None;
    }
    Some(key)
}

/// Render a plain scalar when it is unambiguously plain, else a quoted string.
fn scalar(value: &str) -> String {
    let plain = !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_./*'\"".contains(character))
        && !value.starts_with(['-', '*', '\'', '"']);
    if plain {
        value.to_string()
    } else {
        serde_json::Value::String(value.to_string()).to_string()
    }
}

/// Prove an edit changed exactly what was intended, then return the new text.
///
/// `is_work_package` selects Source's `dependencies: []` injection (a `WP*`
/// document only) and the canonical field emission for that class of file.
fn verified_edit(
    original: &str,
    edited_yaml: &str,
    sets: &[(&str, Option<serde_yaml_ng::Value>)],
    removes: &[&str],
    is_work_package: bool,
) -> Result<String, RescueError> {
    let before = FrontmatterDoc::parse(original)?.mapping()?;
    let doc = FrontmatterDoc {
        yaml: edited_yaml.to_string(),
        body: FrontmatterDoc::parse(original)?.body,
    };
    let after = doc.mapping()?;
    for key in removes {
        if after.contains_key(serde_yaml_ng::Value::String((*key).to_string())) {
            return Err(RescueError::FrontmatterEdit(format!(
                "removing {key:?} did not take effect"
            )));
        }
    }
    for (key, expected) in sets {
        let observed = after.get(serde_yaml_ng::Value::String((*key).to_string()));
        match expected {
            Some(expected) => {
                if observed != Some(expected) {
                    return Err(RescueError::FrontmatterEdit(format!(
                        "{key:?} did not become the intended value"
                    )));
                }
            }
            None => {
                if observed.is_some() {
                    return Err(RescueError::FrontmatterEdit(format!(
                        "{key:?} should be absent"
                    )));
                }
            }
        }
    }
    let touched: Vec<&str> = sets
        .iter()
        .map(|(key, _)| *key)
        .chain(removes.iter().copied())
        .collect();
    for (key, value) in &before {
        let Some(name) = key.as_str() else { continue };
        if touched.contains(&name) {
            continue;
        }
        if after.get(key) != Some(value) {
            return Err(RescueError::FrontmatterEdit(format!(
                "unrelated key {name:?} changed"
            )));
        }
    }
    Ok(doc.render(is_work_package))
}

fn yaml_scalar(value: &str) -> serde_yaml_ng::Value {
    serde_yaml_ng::Value::String(value.to_string())
}

/// Write `bytes` with one explicit Source operation, then verify the readback.
///
/// The operation is a parameter because this one helper serves Source writers
/// that are genuinely different — the classification is per call site, never
/// inferred from the path:
///
/// * `write_meta_object` -> Source `mission_metadata.write_meta` ->
///   `kernel.atomic.atomic_write`;
/// * WP identity backfill -> Source `frontmatter.FrontmatterManager.write` ->
///   `Path.write_text`;
/// * ownership / strip -> Source `frontmatter.locked_update_frontmatter` ->
///   `kernel.atomic.atomic_write`;
/// * event-log rebuild -> Source `migration.rebuild_state.rebuild_event_log` ->
///   fixed `<name>.tmp` sibling + replace.
///
/// The readback is a Native assertion about the bytes that landed; the reference
/// has no equivalent, so it is never presented as Source behaviour.
fn write_verified(
    operation: sk_kernel::source_writer::WriterOperation,
    path: &Path,
    bytes: &[u8],
) -> Result<(), RescueError> {
    sk_kernel::source_writer::write_source(
        operation,
        path,
        sk_kernel::source_writer::SourcePayload::Bytes(bytes),
        None,
    )
    .map(|_receipt| ())
    .map_err(|error| RescueError::Unreadable(format!("{}: {error}", path.display())))?;
    let landed = sk_kernel::read_bounded(path, 1 << 22).map_err(|error| kernel(path, error))?;
    if landed != bytes {
        return Err(RescueError::FrontmatterEdit(format!(
            "{} readback does not match the intended bytes",
            path.display()
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// WP discovery helpers
// ---------------------------------------------------------------------------

fn mission_slug(feature_dir: &Path) -> String {
    feature_dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Sorted `tasks/WP*.md` files of one mission.
fn wp_files(feature_dir: &Path) -> Result<Vec<PathBuf>, RescueError> {
    let tasks_dir = feature_dir.join(TASKS_DIR_NAME);
    if !tasks_dir.is_dir() {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(&tasks_dir).map_err(|error| {
        RescueError::Unreadable(format!("cannot read {}: {error}", tasks_dir.display()))
    })?;
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path.extension().is_some_and(|extension| extension == "md")
                && path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("WP"))
        })
        .collect();
    files.sort();
    Ok(files)
}

/// `WPxx` code from a WP filename stem (Source `_WP_CODE_RE` = `^(WP\d{2,})`).
fn wp_code(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_string_lossy().into_owned();
    let rest = stem.strip_prefix("WP")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.len() < 2 {
        return None;
    }
    Some(format!("WP{digits}"))
}

fn load_meta_object(feature_dir: &Path) -> Result<Option<JsonMap<String, Value>>, RescueError> {
    let path = feature_dir.join(crate::meta::META_FILE_NAME);
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path).map_err(|error| {
        RescueError::Unreadable(format!("cannot read {}: {error}", path.display()))
    })?;
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => Ok(Some(map)),
        Ok(_) => Err(RescueError::CorruptMeta(format!(
            "{} is not a JSON object",
            path.display()
        ))),
        Err(error) => Err(RescueError::CorruptMeta(error.to_string())),
    }
}

/// Write `meta.json` with the canonical bytes (sorted keys, two-space, one LF).
fn write_meta_object(feature_dir: &Path, meta: &JsonMap<String, Value>) -> Result<(), RescueError> {
    let path = feature_dir.join(crate::meta::META_FILE_NAME);
    let mut text = serde_json::to_string_pretty(meta)
        .map_err(|error| RescueError::Writer(format!("cannot serialise meta.json: {error}")))?;
    text.push('\n');
    let bytes = text.into_bytes();
    write_verified(
        sk_kernel::source_writer::WriterOperation::atomic(),
        &path,
        &bytes,
    )
}

// ---------------------------------------------------------------------------
// Identity backfill
// ---------------------------------------------------------------------------

/// What [`backfill_mission_identity`] did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdentityAction {
    /// An identity was minted or a legacy number coerced.
    Wrote,
    /// Both values were already canonical.
    Skip,
    /// The mission could not be given an identity.
    Refused,
}

/// Per-mission identity backfill outcome.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MissionIdentityOutcome {
    /// The mission slug.
    pub slug: String,
    /// What happened.
    pub action: IdentityAction,
    /// The recorded (or existing) `mission_id`.
    pub mission_id: Option<String>,
    /// Whether a string `mission_number` was rewritten as an integer.
    pub number_coerced: bool,
    /// Why the backfill skipped/refused.
    pub reason: Option<String>,
}

/// The Source `mission_number` coercion matrix (`_coerce_mission_number`).
fn coerce_mission_number(raw: &Value) -> Result<Option<Value>, RescueError> {
    match raw {
        Value::Null => Ok(None),
        Value::Bool(value) => Err(RescueError::Uncoercible(format!(
            "meta.json mission_number must be int or null, got bool {value}."
        ))),
        Value::Number(number) => {
            if let Some(value) = number.as_i64() {
                Ok(Some(Value::from(value)))
            } else {
                Err(RescueError::Uncoercible(format!(
                    "meta.json mission_number must be int or null, got {number}."
                )))
            }
        }
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            if ["pending", "unassigned", "TBD"].contains(&trimmed) {
                return Err(RescueError::Uncoercible(format!(
                    "meta.json mission_number must be int or null, got {text:?}. Run `spec-kitty migrate backfill-identity` to migrate."
                )));
            }
            let stripped = trimmed.trim_start_matches('0');
            if stripped.is_empty() {
                return Ok(Some(Value::from(0)));
            }
            match stripped.parse::<i64>() {
                Ok(value) if value >= 0 => Ok(Some(Value::from(value))),
                _ => Err(RescueError::Uncoercible(format!(
                    "meta.json mission_number must be int or null, got {text:?}. Run `spec-kitty migrate backfill-identity` to migrate."
                ))),
            }
        }
        other => Err(RescueError::Uncoercible(format!(
            "meta.json mission_number must be int or null, got {other}."
        ))),
    }
}

/// Write a restored tracked `meta.json` with the canonical bytes.
pub(crate) fn write_restored_meta(
    feature_dir: &Path,
    meta: &JsonMap<String, Value>,
) -> Result<(), RescueError> {
    write_meta_object(feature_dir, meta)
}

fn mint_mission_id() -> String {
    ulid::Ulid::new().to_string()
}

/// Idempotently write `mission_id` into `<feature_dir>/meta.json`.
///
/// If `mission_id` is already present and non-empty this is a no-op. A mission
/// whose `meta.json` is absent is REFUSED rather than given a fresh identity:
/// minting over a copy the operator's Git already tracks (or that was deleted)
/// would fork the Mission, and the tracked-copy restore seam belongs to the
/// Mission metadata authority, not to a migration walk.
pub fn backfill_mission_identity(
    feature_dir: &Path,
    dry_run: bool,
) -> Result<MissionIdentityOutcome, RescueError> {
    backfill_identity(feature_dir, dry_run, true)
}

/// Idempotently write ONLY `mission_id` into `<feature_dir>/meta.json`.
///
/// This is Source `backfill_mission_ids`: the `3.0.0_canonical_context` runner
/// mints the identity but deliberately leaves a legacy `mission_number` value
/// (e.g. `"042"`) untouched. The type coercion belongs to
/// [`backfill_mission_identity`] (Source `backfill_mission`, the
/// `3.2.0a4_normalize_mission_lifecycle` seam) — two different Source functions
/// with two different contracts, kept distinct here for the same reason.
pub fn backfill_mission_id(
    feature_dir: &Path,
    dry_run: bool,
) -> Result<MissionIdentityOutcome, RescueError> {
    backfill_identity(feature_dir, dry_run, false)
}

fn backfill_identity(
    feature_dir: &Path,
    dry_run: bool,
    coerce_number: bool,
) -> Result<MissionIdentityOutcome, RescueError> {
    // Source holds the Mission write lock across the read, the decisions and the
    // write (dry runs included), so a concurrent writer's identity is never
    // overwritten on a stale copy. The lock nests re-entrantly per thread, which
    // is what lets the tracked-copy restore take its own birth lock.
    let lock = sk_status::mutation::acquire_mission_lock_with(
        feature_dir,
        None,
        sk_kernel::LockWait::Bounded(RESCUE_LOCK_TIMEOUT),
        true,
        false,
    )
    .map_err(|error| RescueError::Writer(error.to_string()))?;
    let outcome = backfill_identity_locked(feature_dir, dry_run, coerce_number);
    drop(lock);
    outcome
}

fn backfill_identity_locked(
    feature_dir: &Path,
    dry_run: bool,
    coerce_number: bool,
) -> Result<MissionIdentityOutcome, RescueError> {
    let slug = mission_slug(feature_dir);
    let meta_path = feature_dir.join(crate::meta::META_FILE_NAME);
    let mut restored_from_git = false;
    if !meta_path.exists() {
        // Source restores the tracked copy BEFORE deciding identity, so a Mission
        // whose working-tree meta.json was lost keeps the id its history uses.
        match tracked_meta::read_tracked_meta(feature_dir) {
            Ok(tracked) if tracked.is_restorable() => {
                if dry_run {
                    return Ok(MissionIdentityOutcome {
                        slug,
                        action: IdentityAction::Refused,
                        mission_id: None,
                        number_coerced: false,
                        reason: Some("would restore the tracked meta.json from git".to_string()),
                    });
                }
                if !tracked_meta::restore_tracked_meta(feature_dir)? {
                    return Ok(MissionIdentityOutcome {
                        slug,
                        action: IdentityAction::Refused,
                        mission_id: None,
                        number_coerced: false,
                        reason: Some("tracked meta.json could not be restored".to_string()),
                    });
                }
                restored_from_git = true;
            }
            Ok(_) => {
                let reason = tracked_meta::tracked_meta_notice(feature_dir)?
                    .map(|notice| notice.notice)
                    .unwrap_or_else(|| "meta.json not found".to_string());
                return Ok(MissionIdentityOutcome {
                    slug,
                    action: IdentityAction::Refused,
                    mission_id: None,
                    number_coerced: false,
                    reason: Some(reason),
                });
            }
            Err(error) => {
                return Ok(MissionIdentityOutcome {
                    slug,
                    action: IdentityAction::Refused,
                    mission_id: None,
                    number_coerced: false,
                    reason: Some(error.to_string()),
                });
            }
        }
    }
    let Some(mut meta) = load_meta_object(feature_dir)? else {
        return Ok(MissionIdentityOutcome {
            slug,
            action: IdentityAction::Refused,
            mission_id: None,
            number_coerced: false,
            reason: Some("meta.json not found".to_string()),
        });
    };
    let mut changed = restored_from_git;
    let existing = meta
        .get("mission_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let mut identity_from_git = false;
    let mut recorded = existing;
    if recorded.is_none() {
        // The present copy lost its id: take the tracked one back instead of
        // minting a fresh identity over a copy git does hold.
        match tracked_meta::read_tracked_meta(feature_dir) {
            Ok(tracked) if tracked.unavailable => {
                let reason = tracked_meta::tracked_meta_notice(feature_dir)?
                    .map(|notice| notice.notice)
                    .unwrap_or_else(|| "tracked meta.json is unusable".to_string());
                return Ok(MissionIdentityOutcome {
                    slug,
                    action: IdentityAction::Refused,
                    mission_id: None,
                    number_coerced: false,
                    reason: Some(reason),
                });
            }
            Ok(tracked) => {
                if let Some(id) = tracked
                    .meta
                    .as_ref()
                    .and_then(|value| value.get("mission_id"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    meta.insert("mission_id".to_string(), Value::String(id.to_string()));
                    recorded = Some(id.to_string());
                    identity_from_git = true;
                    changed = true;
                }
            }
            Err(error) => {
                return Ok(MissionIdentityOutcome {
                    slug,
                    action: IdentityAction::Refused,
                    mission_id: None,
                    number_coerced: false,
                    reason: Some(error.to_string()),
                });
            }
        }
    }
    let mission_id = match recorded {
        Some(id) => id,
        None => {
            let minted = mint_mission_id();
            meta.insert("mission_id".to_string(), Value::String(minted.clone()));
            changed = true;
            minted
        }
    };
    let mut number_coerced = false;
    if coerce_number && let Some(raw) = meta.get("mission_number").cloned() {
        match coerce_mission_number(&raw)? {
            Some(coerced) if coerced != raw => {
                meta.insert("mission_number".to_string(), coerced);
                number_coerced = true;
                changed = true;
            }
            _ => {}
        }
    }
    if changed && !dry_run {
        write_meta_object(feature_dir, &meta)?;
    }
    if !changed {
        return Ok(MissionIdentityOutcome {
            slug,
            action: IdentityAction::Skip,
            mission_id: Some(mission_id),
            number_coerced: false,
            reason: Some("mission_id already present".to_string()),
        });
    }
    let _ = identity_from_git;
    Ok(MissionIdentityOutcome {
        slug,
        action: IdentityAction::Wrote,
        mission_id: Some(mission_id),
        number_coerced,
        reason: None,
    })
}

/// Assign `mission_id`, `wp_code` and `work_package_id` to every WP frontmatter.
///
/// `mission_id` is always propagated (it may already be correct), `wp_code` is
/// set when absent, and `work_package_id` is minted only when absent — an
/// existing immutable identity is never overwritten.
pub fn backfill_wp_identity(
    feature_dir: &Path,
    mission_id: &str,
    dry_run: bool,
) -> Result<BTreeMap<String, String>, RescueError> {
    let mut mapping = BTreeMap::new();
    for path in wp_files(feature_dir)? {
        let Some(code) = wp_code(&path) else {
            continue;
        };
        let text = std::fs::read_to_string(&path).map_err(|error| {
            RescueError::Unreadable(format!("cannot read {}: {error}", path.display()))
        })?;
        let doc = FrontmatterDoc::parse(&text)?;
        let mapping_before = doc.mapping()?;
        let mut yaml = doc.yaml.clone();
        let mut sets: Vec<(&str, Option<serde_yaml_ng::Value>)> = Vec::new();

        let recorded_id = mapping_before
            .get(serde_yaml_ng::Value::String("mission_id".to_string()))
            .and_then(|value| value.as_str());
        if recorded_id != Some(mission_id) {
            yaml = FrontmatterDoc::set_scalar(&yaml, "mission_id", &scalar(mission_id));
            sets.push(("mission_id", Some(yaml_scalar(mission_id))));
        }
        if !mapping_before.contains_key(serde_yaml_ng::Value::String("wp_code".to_string())) {
            yaml = FrontmatterDoc::set_scalar(&yaml, "wp_code", &scalar(&code));
            sets.push(("wp_code", Some(yaml_scalar(&code))));
        }
        let existing_wp_id = mapping_before
            .get(serde_yaml_ng::Value::String("work_package_id".to_string()))
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let wp_id = match existing_wp_id {
            Some(id) => id,
            None => {
                let minted = ulid::Ulid::new().to_string();
                yaml = FrontmatterDoc::set_scalar(&yaml, "work_package_id", &scalar(&minted));
                sets.push(("work_package_id", Some(yaml_scalar(&minted))));
                minted
            }
        };
        if !sets.is_empty() && !dry_run {
            let edited = verified_edit(&text, &yaml, &sets, &[], true)?;
            // Source `frontmatter.FrontmatterManager.write` -> `Path.write_text`:
            // the WP identity backfill truncates in place (keeps the inode and an
            // existing mode) instead of replacing the file.
            write_verified(
                sk_kernel::source_writer::WriterOperation::TextTruncate,
                &path,
                edited.as_bytes(),
            )?;
        }
        mapping.insert(code, wp_id);
    }
    Ok(mapping)
}

// ---------------------------------------------------------------------------
// Ownership inference (Source `ownership.inference`)
// ---------------------------------------------------------------------------

const SRC_FALLBACK_GLOB: &str = "src/**";

/// `code_change` or `planning_artifact` (Source `WorkProductKind`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkProductKind {
    /// Source code, tests and their docs.
    CodeChange,
    /// Planning artifacts under `kitty-specs/`.
    PlanningArtifact,
}

impl WorkProductKind {
    /// The persisted `execution_mode` string.
    pub fn as_str(self) -> &'static str {
        match self {
            WorkProductKind::CodeChange => "code_change",
            WorkProductKind::PlanningArtifact => "planning_artifact",
        }
    }
}

const PLANNING_SIGNALS: [&str; 6] = [
    "kitty-specs/",
    "spec.md",
    "plan.md",
    "tasks.md",
    "data-model.md",
    "quickstart.md",
];

const CODE_SIGNALS: [&str; 5] = ["src/", "tests/", "test/", ".py", ".ts"];

fn contains_any(haystack: &str, needles: &[&str]) -> usize {
    needles
        .iter()
        .filter(|needle| haystack.contains(**needle))
        .count()
}

/// Infer `code_change` vs `planning_artifact` from the WP text.
pub fn infer_execution_mode(content: &str, files: &[String]) -> WorkProductKind {
    let mut combined = content.to_string();
    combined.push('\n');
    combined.push_str(&files.join("\n"));
    let planning = contains_any(&combined, &PLANNING_SIGNALS);
    let code = contains_any(&combined, &CODE_SIGNALS);
    if planning > 0 && code == 0 {
        WorkProductKind::PlanningArtifact
    } else {
        WorkProductKind::CodeChange
    }
}

fn path_tokens(content: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for token in content
        .split(|character: char| character.is_whitespace() || "`'\"()[]<>,".contains(character))
    {
        let trimmed = token.trim_end_matches(['.', ':', ';']);
        let known_root = ["src/", "tests/", "test/", "kitty-specs/", "docs/"]
            .iter()
            .any(|root| trimmed.starts_with(root));
        if known_root && trimmed.len() > 4 {
            found.push(trimmed.to_string());
        }
    }
    found.sort();
    found.dedup();
    found
}

/// Infer the `owned_files` globs (Source `infer_owned_files`).
pub fn infer_owned_files(content: &str, slug: &str) -> Vec<String> {
    if infer_execution_mode(content, &[]) == WorkProductKind::PlanningArtifact {
        return vec![format!("kitty-specs/{slug}/**")];
    }
    let mut globs: Vec<String> = Vec::new();
    let mut seen_prefixes: Vec<String> = Vec::new();
    for path in path_tokens(content) {
        let glob = if path.ends_with('/') {
            format!("{path}**")
        } else {
            let last = path.rsplit('/').next().unwrap_or_default();
            if !last.contains('.') {
                format!("{}/**", path.trim_end_matches('/'))
            } else {
                path.clone()
            }
        };
        let parts: Vec<&str> = glob.split('/').collect();
        let mut skip = false;
        for index in 1..parts.len() {
            let candidate = format!("{}/**", parts[..index].join("/"));
            if seen_prefixes.contains(&candidate) {
                skip = true;
                break;
            }
        }
        if !skip {
            seen_prefixes.push(glob.clone());
            globs.push(glob);
        }
    }
    if globs.is_empty() {
        return vec![SRC_FALLBACK_GLOB.to_string()];
    }
    globs
}

fn strip_glob(pattern: &str) -> String {
    match pattern.rsplit_once('/') {
        Some((head, tail)) if tail.contains(['*', '?', '[']) || tail.contains('.') => {
            head.to_string()
        }
        _ => pattern.trim_end_matches('/').to_string(),
    }
}

/// Derive `authoritative_surface` from `owned_files` (Source `infer_authoritative_surface`).
pub fn infer_authoritative_surface(owned_files: &[String]) -> String {
    if owned_files.is_empty() {
        return String::new();
    }
    let stripped: Vec<String> = owned_files
        .iter()
        .map(|pattern| strip_glob(pattern))
        .collect();
    if stripped.len() == 1 {
        let candidate = format!("{}/", stripped[0].trim_end_matches('/'));
        return if owned_files[0].starts_with(&candidate) {
            candidate
        } else {
            owned_files[0].clone()
        };
    }
    let split: Vec<Vec<&str>> = stripped
        .iter()
        .map(|pattern| pattern.split('/').collect())
        .collect();
    let mut common: Vec<&str> = Vec::new();
    for position in 0..split[0].len() {
        let mut value: Option<&str> = None;
        let mut agree = true;
        for parts in &split {
            match parts.get(position) {
                Some(part) => {
                    if let Some(seen) = value {
                        if seen != *part {
                            agree = false;
                            break;
                        }
                    } else {
                        value = Some(part);
                    }
                }
                None => {
                    agree = false;
                    break;
                }
            }
        }
        match (agree, value) {
            (true, Some(part)) => common.push(part),
            _ => break,
        }
    }
    let base = if common.is_empty() {
        split[0].join("/")
    } else {
        common.join("/")
    };
    let candidate = format!("{}/", base.trim_end_matches('/'));
    if owned_files
        .iter()
        .any(|pattern| pattern.starts_with(&candidate))
    {
        candidate
    } else {
        owned_files[0].clone()
    }
}

/// Infer and write the ownership fields for every WP, only where absent.
///
/// The best-effort lane-branch `git diff` discovery degrades to no extra files
/// (Source returns `[]` on any Git error), so a non-Git or branchless corpus
/// produces exactly the body-inferred values.
pub fn backfill_ownership(
    feature_dir: &Path,
    slug: &str,
    dry_run: bool,
) -> Result<usize, RescueError> {
    backfill_ownership_with_files(feature_dir, slug, &BTreeMap::new(), dry_run)
}

/// [`backfill_ownership`] with the actually-changed files per WP code.
///
/// `lane_files` is the resolved `git diff <base>..<lane>` result (Source's
/// `_git_diff_files`). Source prefers those files over the body inference ONLY
/// when the WP's execution mode is being inferred as `code_change`; every other
/// case keeps the body-derived globs. An absent/empty list is "no diff
/// available", which is exactly Source's `[]` on a Git error.
pub fn backfill_ownership_with_files(
    feature_dir: &Path,
    slug: &str,
    lane_files: &BTreeMap<String, Vec<String>>,
    dry_run: bool,
) -> Result<usize, RescueError> {
    // Source takes the Mission write lock around the whole per-WP write seam
    // (`locked_update_frontmatter`), so a field another writer landed is neither
    // lost nor overwritten behind its back.
    let lock = sk_status::mutation::acquire_mission_lock_with(
        feature_dir,
        None,
        sk_kernel::LockWait::Bounded(RESCUE_LOCK_TIMEOUT),
        true,
        false,
    )
    .map_err(|error| RescueError::Writer(error.to_string()))?;
    let written = backfill_ownership_locked(feature_dir, slug, lane_files, dry_run);
    drop(lock);
    written
}

fn backfill_ownership_locked(
    feature_dir: &Path,
    slug: &str,
    lane_files: &BTreeMap<String, Vec<String>>,
    dry_run: bool,
) -> Result<usize, RescueError> {
    let mut written = 0usize;
    for path in wp_files(feature_dir)? {
        let text = std::fs::read_to_string(&path).map_err(|error| {
            RescueError::Unreadable(format!("cannot read {}: {error}", path.display()))
        })?;
        let doc = FrontmatterDoc::parse(&text)?;
        let mapping = doc.mapping()?;
        let has = |key: &str| mapping.contains_key(serde_yaml_ng::Value::String(key.to_string()));
        let has_mode = has("execution_mode");
        let has_files = has("owned_files");
        let has_surface = has("authoritative_surface");
        if has_mode && has_files && has_surface {
            continue;
        }
        // Source `full_content = body`: inference sees the WP body only, never
        // the frontmatter's own path-like text.
        let body = doc.body.clone();
        let code = wp_code(&path).unwrap_or_default();
        let git_files: Vec<String> = lane_files.get(&code).cloned().unwrap_or_default();
        let mut yaml = doc.yaml.clone();
        let mut sets: Vec<(&str, Option<serde_yaml_ng::Value>)> = Vec::new();
        // The mode inferred by THIS run, if any. Source's diff override is gated
        // on `updates.get("execution_mode") == "code_change"`, i.e. only when the
        // mode is being inferred now — an authored `execution_mode` never
        // triggers it.
        let mut inferred_mode: Option<WorkProductKind> = None;
        if !has_mode {
            let inferred = infer_execution_mode(&body, &git_files);
            sets.push(("execution_mode", Some(yaml_scalar(inferred.as_str()))));
            inferred_mode = Some(inferred);
        }
        let mut owned_for_surface: Option<Vec<String>> = None;
        if !has_files {
            let mut owned = infer_owned_files(&body, slug);
            if !git_files.is_empty() && inferred_mode == Some(WorkProductKind::CodeChange) {
                owned = git_files.clone();
            }
            yaml = FrontmatterDoc::set_sequence(&yaml, "owned_files", &owned);
            sets.push(("owned_files", None));
            owned_for_surface = Some(owned);
        }
        if !has_surface {
            let owned_value = match owned_for_surface.clone() {
                Some(owned) => owned,
                None => mapping
                    .get(serde_yaml_ng::Value::String("owned_files".to_string()))
                    .and_then(|value| value.as_sequence())
                    .map(|sequence| {
                        sequence
                            .iter()
                            .filter_map(|item| item.as_str().map(str::to_string))
                            .collect::<Vec<String>>()
                    })
                    .unwrap_or_default(),
            };
            let surface = infer_authoritative_surface(&owned_value);
            yaml = FrontmatterDoc::set_scalar(&yaml, "authoritative_surface", &scalar(&surface));
            sets.push(("authoritative_surface", Some(yaml_scalar(&surface))));
        }
        if let Some(mode) = inferred_mode {
            yaml = FrontmatterDoc::set_scalar(&yaml, "execution_mode", &scalar(mode.as_str()));
        }
        if sets.is_empty() {
            continue;
        }
        // `owned_files` is a sequence, so its expected value is checked by
        // re-parsing the rendered block rather than by a scalar comparison.
        let expected_sets: Vec<(&str, Option<serde_yaml_ng::Value>)> = sets
            .iter()
            .map(|(key, value)| match (*key, &owned_for_surface) {
                ("owned_files", Some(owned)) => (
                    *key,
                    Some(serde_yaml_ng::Value::Sequence(
                        owned.iter().map(|item| yaml_scalar(item)).collect(),
                    )),
                ),
                _ => (*key, value.clone()),
            })
            .collect();
        if !dry_run {
            let edited = verified_edit(&text, &yaml, &expected_sets, &[], true)?;
            // Source `frontmatter.locked_update_frontmatter` -> `atomic_write`.
            write_verified(
                sk_kernel::source_writer::WriterOperation::atomic(),
                &path,
                edited.as_bytes(),
            )?;
        }
        written += 1;
    }
    Ok(written)
}

// ---------------------------------------------------------------------------
// Frontmatter strip
// ---------------------------------------------------------------------------

/// What [`strip_mutable_fields`] did.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StripOutcome {
    /// WP files processed.
    pub wps_processed: usize,
    /// Field instances removed across all WPs.
    pub fields_stripped: usize,
    /// `wp_code -> lane` recorded BEFORE stripping.
    pub lane_records: BTreeMap<String, String>,
    /// Non-fatal per-file warnings.
    pub warnings: Vec<String>,
}

/// Remove every [`MUTABLE_FIELDS`] key from each WP frontmatter (and `tasks.md`).
///
/// The `lane` value is recorded from the same read, before it is removed, so the
/// caller can rebuild state from it. Nothing is written when no mutable field is
/// present.
pub fn strip_mutable_fields(feature_dir: &Path) -> Result<StripOutcome, RescueError> {
    let mut outcome = StripOutcome::default();
    let lock = sk_status::mutation::acquire_mission_lock(
        feature_dir,
        None,
        sk_kernel::LockWait::Bounded(RESCUE_LOCK_TIMEOUT),
    )
    .map_err(|error| RescueError::Writer(error.to_string()))?;
    for path in wp_files(feature_dir)? {
        let code = wp_code(&path).unwrap_or_else(|| mission_slug(&path));
        match strip_one(&path, Some((&mut outcome.lane_records, &code)), true) {
            Ok(stripped) => {
                outcome.wps_processed += 1;
                outcome.fields_stripped += stripped;
            }
            Err(error) => outcome.warnings.push(format!(
                "Cannot read {}: {} — skipping",
                path.file_name().unwrap_or_default().to_string_lossy(),
                error
            )),
        }
    }
    let tasks_md = feature_dir.join(crate::paths::TASKS_FILE_NAME);
    if tasks_md.exists()
        && let Ok(stripped) = strip_one(&tasks_md, None, false)
    {
        outcome.fields_stripped += stripped;
    }
    drop(lock);
    Ok(outcome)
}

fn strip_one(
    path: &Path,
    lane_record: Option<(&mut BTreeMap<String, String>, &str)>,
    is_work_package: bool,
) -> Result<usize, RescueError> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        RescueError::Unreadable(format!("cannot read {}: {error}", path.display()))
    })?;
    let doc = FrontmatterDoc::parse(&text)?;
    let mapping = doc.mapping()?;
    let mut yaml = doc.yaml.clone();
    let mut present: Vec<&str> = Vec::new();
    for field in MUTABLE_FIELDS {
        if mapping.contains_key(serde_yaml_ng::Value::String(field.to_string())) {
            present.push(field);
        }
    }
    if let Some((records, code)) = lane_record
        && let Some(lane) = mapping
            .get(serde_yaml_ng::Value::String("lane".to_string()))
            .and_then(|value| value.as_str())
    {
        records.insert(code.to_string(), lane.to_string());
    }
    if present.is_empty() {
        return Ok(0);
    }
    for field in &present {
        yaml = FrontmatterDoc::remove_key(&yaml, field);
    }
    let edited = verified_edit(&text, &yaml, &[], &present, is_work_package)?;
    // Source `frontmatter.locked_update_frontmatter` -> `atomic_write`.
    write_verified(
        sk_kernel::source_writer::WriterOperation::atomic(),
        path,
        edited.as_bytes(),
    )?;
    Ok(present.len())
}

// ---------------------------------------------------------------------------
// Event-log rebuild (Source `rebuild_state`)
// ---------------------------------------------------------------------------

const MIGRATION_EPOCH: &str = "2026-01-01T00:00:00+00:00";
const LANE_ORDER: [&str; 5] = ["planned", "claimed", "in_progress", "for_review", "done"];
const CHAIN_STEP_SECONDS: i64 = 300;

/// Outcome of one mission's event-log rebuild.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RebuildOutcome {
    /// The mission slug.
    pub mission_slug: String,
    /// Brand-new synthetic events created.
    pub events_generated: usize,
    /// Existing events preserved unchanged.
    pub events_kept: usize,
    /// Existing events that were enriched or corrected.
    pub events_corrected: usize,
    /// Source-level lane disagreements.
    pub conflicts_found: usize,
    /// Non-fatal warnings.
    pub warnings: Vec<String>,
    /// Source `RebuildResult.errors`: failures the rebuild REPORTS instead of
    /// raising (a log-write `OSError`). The counts and warnings collected before
    /// the failure survive on the same result, exactly like the reference.
    pub errors: Vec<String>,
    /// True when there was nothing to rebuild.
    pub skipped: bool,
}

fn resolve_lane_alias(lane: &str) -> String {
    match lane.trim().to_lowercase().as_str() {
        "doing" => "in_progress".to_string(),
        "review" => "for_review".to_string(),
        "complete" => "done".to_string(),
        "cancelled" => "canceled".to_string(),
        other => other.to_string(),
    }
}

fn python_isoformat(instant: sk_kernel::clock::UtcDateTime<sk_kernel::clock::UtcZone>) -> String {
    if instant.timestamp_subsec_micros() == 0 {
        instant.format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
    } else {
        instant.format("%Y-%m-%dT%H:%M:%S%.6f+00:00").to_string()
    }
}

fn shift_seconds(raw: &str, offset: i64) -> String {
    let parsed = sk_kernel::clock::parse_iso(&raw.replace('Z', "+00:00"))
        .unwrap_or_else(|_| sk_kernel::clock::parse_iso(MIGRATION_EPOCH).expect("epoch parses"));
    if offset == 0 {
        return python_isoformat(parsed);
    }
    let system: std::time::SystemTime = parsed.into();
    let shifted = if offset >= 0 {
        system
            .checked_add(std::time::Duration::from_secs(offset.unsigned_abs()))
            .unwrap_or(system)
    } else {
        system
            .checked_sub(std::time::Duration::from_secs(offset.unsigned_abs()))
            .unwrap_or(system)
    };
    python_isoformat(shifted.into())
}

/// The deterministic 26-char Crockford id (Source `_deterministic_id`).
fn deterministic_id(parts: &[&str]) -> String {
    let seed = parts.join("|");
    let hex = sk_kernel::content_digest::sha256_hex(seed.as_bytes());
    let mut value = u128::from_str_radix(&hex[..32], 16).expect("sha256 hex prefix");
    const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut characters = [0u8; 26];
    for slot in characters.iter_mut().rev() {
        *slot = CROCKFORD[(value & 31) as usize];
        value >>= 5;
    }
    String::from_utf8(characters.to_vec()).expect("ASCII alphabet")
}

fn read_event_rows(feature_dir: &Path) -> Vec<Value> {
    let path = feature_dir.join(EVENTS_FILE_NAME);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect()
}

fn read_status_json(feature_dir: &Path) -> Option<JsonMap<String, Value>> {
    let text = std::fs::read_to_string(feature_dir.join(STATUS_FILE_NAME)).ok()?;
    match serde_json::from_str::<Value>(&text).ok()? {
        Value::Object(map) => Some(map),
        _ => None,
    }
}

fn read_frontmatter_lanes(feature_dir: &Path) -> BTreeMap<String, String> {
    let mut lanes = BTreeMap::new();
    let Ok(files) = wp_files(feature_dir) else {
        return lanes;
    };
    for path in files {
        let Some(code) = wp_code(&path) else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(doc) = FrontmatterDoc::parse(&text) else {
            continue;
        };
        let Ok(mapping) = doc.mapping() else {
            continue;
        };
        let raw = mapping
            .get(serde_yaml_ng::Value::String("lane".to_string()))
            .and_then(|value| value.as_str())
            .unwrap_or("planned");
        lanes.insert(code, resolve_lane_alias(raw));
    }
    lanes
}

/// The deterministic timestamp the synthetic events anchor to.
fn derive_migration_timestamp(feature_dir: &Path) -> String {
    let mut candidates: Vec<String> = Vec::new();
    for row in read_event_rows(feature_dir) {
        if let Some(at) = row.get("at").and_then(Value::as_str)
            && !at.is_empty()
        {
            candidates.push(at.to_string());
        }
    }
    if let Some(status) = read_status_json(feature_dir) {
        if let Some(at) = status.get("materialized_at").and_then(Value::as_str)
            && !at.is_empty()
        {
            candidates.push(at.to_string());
        }
        if let Some(packages) = status.get("work_packages").and_then(Value::as_object) {
            for state in packages.values() {
                if let Some(at) = state.get("last_transition_at").and_then(Value::as_str)
                    && !at.is_empty()
                {
                    candidates.push(at.to_string());
                }
            }
        }
    }
    if let Ok(Some(meta)) = load_meta_object(feature_dir)
        && let Some(at) = meta.get("created_at").and_then(Value::as_str)
        && !at.is_empty()
    {
        candidates.push(at.to_string());
    }
    let Some(latest) = candidates.iter().max() else {
        return MIGRATION_EPOCH.to_string();
    };
    shift_seconds(latest, 1)
}

fn lane_path(target: &str) -> Vec<String> {
    if target == "planned" {
        return Vec::new();
    }
    if target == "done" || target == "approved" {
        return LANE_ORDER.iter().map(|lane| lane.to_string()).collect();
    }
    if LANE_ORDER.contains(&target) {
        let mut path = vec!["planned".to_string()];
        for lane in LANE_ORDER.iter().skip(1) {
            path.push(lane.to_string());
            if *lane == target {
                return path;
            }
        }
        return path;
    }
    vec![
        "planned".to_string(),
        "claimed".to_string(),
        "in_progress".to_string(),
        target.to_string(),
    ]
}

fn build_chain(
    slug: &str,
    wp_code: &str,
    wp_id_map: &BTreeMap<String, String>,
    target_lane: &str,
    migration_timestamp: &str,
) -> Vec<Value> {
    let path = lane_path(target_lane);
    if path.is_empty() {
        return Vec::new();
    }
    let work_package_id = wp_id_map.get(wp_code).cloned().unwrap_or_default();
    let mut events = Vec::new();
    for (index, window) in path.windows(2).enumerate() {
        let offset = (path.len() as i64 - 2 - index as i64) * CHAIN_STEP_SECONDS;
        let mut row = JsonMap::new();
        row.insert(
            "event_id".to_string(),
            Value::String(deterministic_id(&[
                slug,
                wp_code,
                &window[0],
                &window[1],
                "chain",
                &index.to_string(),
            ])),
        );
        row.insert("mission_slug".to_string(), Value::String(slug.to_string()));
        row.insert("wp_id".to_string(), Value::String(wp_code.to_string()));
        row.insert("from_lane".to_string(), Value::String(window[0].clone()));
        row.insert("to_lane".to_string(), Value::String(window[1].clone()));
        row.insert(
            "at".to_string(),
            Value::String(shift_seconds(migration_timestamp, -offset)),
        );
        row.insert("actor".to_string(), Value::String("migration".to_string()));
        row.insert("force".to_string(), Value::Bool(true));
        row.insert(
            "execution_mode".to_string(),
            Value::String("unknown".to_string()),
        );
        row.insert(
            "reason".to_string(),
            Value::String("bootstrapped from legacy state".to_string()),
        );
        row.insert("review_ref".to_string(), Value::Null);
        row.insert("evidence".to_string(), Value::Null);
        if !work_package_id.is_empty() {
            row.insert(
                "work_package_id".to_string(),
                Value::String(work_package_id.clone()),
            );
        }
        events.push(Value::Object(row));
    }
    events
}

/// Rebuild one mission's canonical event log from every legacy source.
///
/// Cross-validates the existing log, `status.json` and the frontmatter `lane`
/// fields, then writes a reconciled, deduplicated, identity-enriched log under
/// the one Mission status lock. Deterministic: synthetic ids come from stable
/// inputs and synthetic timestamps anchor to the corpus, never the wall clock.
pub fn rebuild_mission_event_log(
    feature_dir: &Path,
    slug: &str,
    wp_id_map: &BTreeMap<String, String>,
) -> Result<RebuildOutcome, RescueError> {
    let lock = sk_status::mutation::acquire_mission_lock(
        feature_dir,
        None,
        sk_kernel::LockWait::Bounded(RESCUE_LOCK_TIMEOUT),
    )
    .map_err(|error| RescueError::Writer(error.to_string()))?;
    let outcome = rebuild_mission_event_log_locked(feature_dir, slug, wp_id_map);
    drop(lock);
    outcome
}

fn rebuild_mission_event_log_locked(
    feature_dir: &Path,
    slug: &str,
    wp_id_map: &BTreeMap<String, String>,
) -> Result<RebuildOutcome, RescueError> {
    let mut result = RebuildOutcome {
        mission_slug: slug.to_string(),
        ..RebuildOutcome::default()
    };
    let migration_ts = derive_migration_timestamp(feature_dir);
    let existing_events = read_event_rows(feature_dir);
    let status_json = read_status_json(feature_dir);
    let frontmatter_lanes = read_frontmatter_lanes(feature_dir);
    if existing_events.is_empty() && frontmatter_lanes.is_empty() {
        result.skipped = true;
        return Ok(result);
    }

    let mut status_json_lanes: BTreeMap<String, String> = BTreeMap::new();
    let mut status_json_ts: Option<String> = None;
    if let Some(status) = &status_json {
        status_json_ts = status
            .get("materialized_at")
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(packages) = status.get("work_packages").and_then(Value::as_object) {
            for (wp_code, state) in packages {
                let raw = state
                    .get("lane")
                    .and_then(Value::as_str)
                    .unwrap_or("planned");
                status_json_lanes.insert(wp_code.clone(), resolve_lane_alias(raw));
            }
        }
    }

    let mut event_log_lanes: BTreeMap<String, String> = BTreeMap::new();
    let mut event_log_ts: BTreeMap<String, String> = BTreeMap::new();
    let mut per_wp: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for row in &existing_events {
        if let Some(wp_code) = row.get("wp_id").and_then(Value::as_str)
            && !wp_code.is_empty()
        {
            per_wp.entry(wp_code.to_string()).or_default().push(row);
        }
    }
    for (wp_code, rows) in &per_wp {
        let terminal = rows
            .iter()
            .max_by(|left, right| {
                let left_at = left.get("at").and_then(Value::as_str).unwrap_or_default();
                let right_at = right.get("at").and_then(Value::as_str).unwrap_or_default();
                left_at.cmp(right_at)
            })
            .copied();
        if let Some(terminal) = terminal
            && let Some(to_lane) = terminal.get("to_lane").and_then(Value::as_str)
        {
            event_log_lanes.insert(wp_code.clone(), resolve_lane_alias(to_lane));
            event_log_ts.insert(
                wp_code.clone(),
                terminal
                    .get("at")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            );
        }
    }

    let mut all_wp_codes: std::collections::BTreeSet<String> =
        frontmatter_lanes.keys().cloned().collect();
    all_wp_codes.extend(status_json_lanes.keys().cloned());
    all_wp_codes.extend(event_log_lanes.keys().cloned());

    let mut authoritative: BTreeMap<String, String> = BTreeMap::new();
    for wp_code in &all_wp_codes {
        let mut candidates: Vec<(&str, &str, Option<&str>)> = Vec::new();
        if let Some(lane) = event_log_lanes.get(wp_code) {
            candidates.push((
                "event_log",
                lane.as_str(),
                event_log_ts.get(wp_code).map(String::as_str),
            ));
        }
        if let Some(lane) = status_json_lanes.get(wp_code) {
            candidates.push(("status_json", lane.as_str(), status_json_ts.as_deref()));
        }
        if let Some(lane) = frontmatter_lanes.get(wp_code) {
            candidates.push(("frontmatter", lane.as_str(), None));
        }
        let Some(first) = candidates.first() else {
            authoritative.insert(wp_code.clone(), "planned".to_string());
            continue;
        };
        let unique: std::collections::BTreeSet<&str> =
            candidates.iter().map(|(_, lane, _)| *lane).collect();
        if unique.len() == 1 {
            authoritative.insert(wp_code.clone(), first.1.to_string());
            continue;
        }
        result.conflicts_found += 1;
        let winner = {
            let timestamped: Vec<&(&str, &str, Option<&str>)> = candidates
                .iter()
                .filter(|(_, _, timestamp)| timestamp.is_some())
                .collect();
            if let Some(best) = timestamped
                .iter()
                .max_by(|left, right| left.2.cmp(&right.2))
            {
                (best.0, best.1)
            } else {
                let mut chosen = first.0;
                let mut chosen_lane = first.1;
                for preference in ["event_log", "status_json", "frontmatter"] {
                    if let Some((source, lane, _)) = candidates
                        .iter()
                        .find(|(source, _, _)| *source == preference)
                    {
                        chosen = source;
                        chosen_lane = lane;
                        break;
                    }
                }
                (chosen, chosen_lane)
            }
        };
        let losers: Vec<String> = candidates
            .iter()
            .filter(|(source, _, _)| *source != winner.0)
            .map(|(source, lane, _)| format!("{source}={lane}"))
            .collect();
        result.warnings.push(format!(
            "{slug}/{wp_code}: lane conflict ({} vs {}={}). Using {}.",
            losers.join(", "),
            winner.0,
            winner.1,
            winner.0
        ));
        authoritative.insert(wp_code.clone(), winner.1.to_string());
    }

    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut dropped = 0usize;
    let mut enriched_events: Vec<Value> = Vec::new();
    for row in &existing_events {
        let event_id = row
            .get("event_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if !seen.insert(event_id) {
            dropped += 1;
            continue;
        }
        let mut enriched = row.as_object().cloned().unwrap_or_default();
        let mut modified = false;
        let wp_code = row
            .get("wp_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if !enriched.contains_key("work_package_id")
            && let Some(mapped) = wp_id_map.get(&wp_code)
        {
            enriched.insert("work_package_id".to_string(), Value::String(mapped.clone()));
            modified = true;
        }
        if !enriched.contains_key("mission_slug") {
            enriched.insert("mission_slug".to_string(), Value::String(slug.to_string()));
            modified = true;
        }
        if modified {
            result.events_corrected += 1;
        }
        enriched_events.push(Value::Object(enriched));
    }
    if dropped > 0 {
        result.warnings.push(format!(
            "Dropped {dropped} duplicate events (same event_id)."
        ));
    }
    result.events_kept = enriched_events.len();

    let mut corrective: Vec<Value> = Vec::new();
    for (wp_code, auth_lane) in &authoritative {
        let log_lane = event_log_lanes.get(wp_code);
        match log_lane {
            Some(log_lane) if log_lane == auth_lane => {}
            Some(log_lane) => {
                result.warnings.push(format!(
                    "{slug}/{wp_code}: event log says '{log_lane}' but authoritative source says '{auth_lane}'. Emitting corrective synthetic event."
                ));
                let mut row = JsonMap::new();
                row.insert(
                    "event_id".to_string(),
                    Value::String(deterministic_id(&[
                        slug,
                        wp_code,
                        log_lane,
                        auth_lane,
                        "corrective",
                    ])),
                );
                row.insert("mission_slug".to_string(), Value::String(slug.to_string()));
                row.insert("wp_id".to_string(), Value::String(wp_code.clone()));
                row.insert("from_lane".to_string(), Value::String(log_lane.clone()));
                row.insert("to_lane".to_string(), Value::String(auth_lane.clone()));
                row.insert("at".to_string(), Value::String(migration_ts.clone()));
                row.insert("actor".to_string(), Value::String("migration".to_string()));
                row.insert("force".to_string(), Value::Bool(true));
                row.insert(
                    "execution_mode".to_string(),
                    Value::String("unknown".to_string()),
                );
                row.insert(
                    "reason".to_string(),
                    Value::String("reconciled from authoritative source".to_string()),
                );
                row.insert("review_ref".to_string(), Value::Null);
                row.insert("evidence".to_string(), Value::Null);
                if let Some(mapped) = wp_id_map.get(wp_code) {
                    row.insert("work_package_id".to_string(), Value::String(mapped.clone()));
                }
                corrective.push(Value::Object(row));
                result.events_corrected += 1;
            }
            None => {
                let chain = build_chain(slug, wp_code, wp_id_map, auth_lane, &migration_ts);
                if !chain.is_empty() {
                    result.events_generated += chain.len();
                    corrective.extend(chain);
                }
            }
        }
    }

    if enriched_events.is_empty() && corrective.is_empty() {
        result.skipped = true;
        return Ok(result);
    }
    let mut bytes = Vec::new();
    for row in enriched_events.iter().chain(corrective.iter()) {
        bytes.extend_from_slice(sk_status::mutation_json::serialize_event_line(row).as_bytes());
        bytes.push(b'\n');
    }
    // Source `migration.rebuild_state.rebuild_event_log`: the fixed
    // `<name>.tmp` sibling, `open("w")` creation mode, `os.replace`, and the
    // temporary is removed on any failure. A write failure is REPORTED through
    // `RebuildResult.errors` (`Failed to write event log for <slug>: <e>`) — the
    // reference does not re-raise, so the caller keeps the counts/warnings this
    // run already collected.
    if let Err(error) = write_verified(
        sk_kernel::source_writer::WriterOperation::RebuildLog,
        &feature_dir.join(EVENTS_FILE_NAME),
        &bytes,
    ) {
        result
            .errors
            .push(format!("Failed to write event log for {slug}: {error}"));
        return Ok(result);
    }
    Ok(result)
}

// ---------------------------------------------------------------------------
// Lifecycle normalization
// ---------------------------------------------------------------------------

/// One mission's lifecycle normalization outcome.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NormalizeOutcome {
    /// The mission slug.
    pub slug: String,
    /// `normalized`, `skipped` or `error`.
    pub status: &'static str,
    /// The lifecycle state the canonical reducer derived, when available.
    pub lifecycle_state: Option<String>,
    /// What was done.
    pub actions: Vec<String>,
    /// Non-fatal warnings.
    pub warnings: Vec<String>,
    /// The terminal error, when `status == "error"`.
    pub error: Option<String>,
}

fn needs_event_log(feature_dir: &Path) -> bool {
    if feature_dir.join(EVENTS_FILE_NAME).exists() {
        return false;
    }
    feature_dir.join(STATUS_FILE_NAME).exists() || feature_dir.join(TASKS_DIR_NAME).is_dir()
}

fn derived_dir(repo_root: &Path, slug: &str) -> PathBuf {
    sk_status::mission_projection::derived_dir_for(repo_root, slug)
}

fn needs_derived_refresh(feature_dir: &Path, derived: &Path) -> bool {
    // Source `_needs_derived_refresh` checks exactly these three views. The
    // board summary is still WRITTEN by the projection authority; it is simply
    // not part of the freshness predicate (a missing `board-summary.json` alone
    // must not select this migration).
    let views = ["status.json", "progress.json", "lifecycle.json"];
    if views.iter().any(|name| !derived.join(name).exists()) {
        return true;
    }
    let Ok(events_meta) = std::fs::metadata(feature_dir.join(EVENTS_FILE_NAME)) else {
        return false;
    };
    let Ok(events_modified) = events_meta.modified() else {
        return false;
    };
    views.iter().any(|name| {
        std::fs::metadata(derived.join(name))
            .and_then(|meta| meta.modified())
            .map(|modified| events_modified > modified)
            .unwrap_or(true)
    })
}

/// Regenerate every canonical derived view for one mission (status, board,
/// progress, lifecycle) through the one projection authority.
fn refresh_derived_views(
    feature_dir: &Path,
    repo_root: &Path,
    now: sk_kernel::clock::UtcDateTime<sk_kernel::clock::UtcZone>,
) -> Result<sk_status::mission_projection::ProjectionReceipt, RescueError> {
    let derived_root = repo_root.join(sk_status::mission_projection::DERIVED_VIEWS_DIR);
    sk_status::mission_projection::write_mission_projection(feature_dir, &derived_root, now)
        .map_err(|error| RescueError::Status(error.to_string()))
}

/// Normalize lifecycle-relevant mission state for one repo.
///
/// Per mission: read `meta.json` fail-closed, backfill the Mission identity, then
/// — when the log is missing but legacy state exists — backfill WP identities and
/// rebuild `status.events.jsonl`, and finally refresh the canonical derived
/// `status.json` view. A residue folder (no `meta.json` and nothing git tracks)
/// and a mission whose `meta.json` is missing are reported, never given an
/// identity.
pub fn normalize_mission_lifecycle(repo_root: &Path, dry_run: bool) -> Vec<NormalizeOutcome> {
    normalize_mission_lifecycle_at(repo_root, dry_run, sk_kernel::clock::now_utc())
}

/// [`normalize_mission_lifecycle`] with an injected clock (Source threads
/// `now`/`clock` into the derived lifecycle view).
pub fn normalize_mission_lifecycle_at(
    repo_root: &Path,
    dry_run: bool,
    now: sk_kernel::clock::UtcDateTime<sk_kernel::clock::UtcZone>,
) -> Vec<NormalizeOutcome> {
    let mut results = Vec::new();
    let corpus = corpus::MissionCorpus::new(repo_root);
    let candidates = match corpus.lifecycle_candidates() {
        Ok(candidates) => candidates,
        // Source `_discover_candidates` raises on an unreadable `kitty-specs/`;
        // an absent one is the empty corpus.
        Err(_) => return results,
    };
    for candidate in candidates {
        let feature_dir = candidate.path.clone();
        let slug = candidate.slug.clone();
        let mut outcome = NormalizeOutcome {
            slug: slug.clone(),
            status: "skipped",
            lifecycle_state: None,
            actions: Vec::new(),
            warnings: Vec::new(),
            error: None,
        };
        if !candidate.is_mission() {
            // Source #6627: residue is reported with the one actionable notice,
            // never given an identity and never written.
            outcome
                .warnings
                .push(corpus::residue_notice_for(repo_root, &feature_dir));
            results.push(outcome);
            continue;
        }
        let meta = match load_meta_object(&feature_dir) {
            Ok(Some(meta)) => meta,
            Ok(None) => {
                // Source restores the tracked copy (#6673) before deciding the
                // mission is unrepairable; only a copy git cannot give back is a
                // report.
                match tracked_meta::read_tracked_meta(&feature_dir) {
                    Ok(tracked) if tracked.is_restorable() => {
                        if dry_run {
                            outcome
                                .actions
                                .push("Would restore the tracked meta.json from git".to_string());
                            outcome.status = "normalized";
                            results.push(outcome);
                            continue;
                        }
                        match tracked_meta::restore_tracked_meta(&feature_dir) {
                            Ok(true) => {
                                outcome
                                    .actions
                                    .push("Restored the tracked meta.json from git".to_string());
                                match load_meta_object(&feature_dir) {
                                    Ok(Some(meta)) => meta,
                                    Ok(_) => {
                                        outcome
                                            .warnings
                                            .push("meta.json missing; mission skipped".to_string());
                                        results.push(outcome);
                                        continue;
                                    }
                                    Err(error) => {
                                        outcome.status = "error";
                                        outcome.error =
                                            Some(format!("Could not read meta.json: {error}"));
                                        results.push(outcome);
                                        continue;
                                    }
                                }
                            }
                            _ => {
                                outcome.warnings.push(
                                    tracked_meta::tracked_meta_notice(&feature_dir)
                                        .ok()
                                        .flatten()
                                        .map(|notice| notice.notice)
                                        .unwrap_or_else(|| {
                                            "meta.json missing; mission skipped".to_string()
                                        }),
                                );
                                results.push(outcome);
                                continue;
                            }
                        }
                    }
                    Ok(_) => {
                        outcome.warnings.push(
                            tracked_meta::tracked_meta_notice(&feature_dir)
                                .ok()
                                .flatten()
                                .map(|notice| notice.notice)
                                .unwrap_or_else(|| {
                                    "meta.json missing; mission skipped".to_string()
                                }),
                        );
                        results.push(outcome);
                        continue;
                    }
                    Err(error) => {
                        outcome
                            .warnings
                            .push(format!("meta.json missing; tracked copy unknown: {error}"));
                        results.push(outcome);
                        continue;
                    }
                }
            }
            Err(error) => {
                outcome.status = "error";
                outcome.error = Some(format!("Could not read meta.json: {error}"));
                results.push(outcome);
                continue;
            }
        };
        // Source's dry-run branch never reloads `meta` after the (unwritten)
        // identity backfill, so its `mission_id` test still sees the PRE-write
        // document. The live branch reloads it and therefore does propagate the
        // freshly minted id into the WP-identity step.
        let pre_write_mission_id = meta
            .get("mission_id")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string);

        let identity = match backfill_mission_identity(&feature_dir, dry_run) {
            Ok(identity) => identity,
            Err(error) => {
                outcome.status = "error";
                outcome.error = Some(format!("Identity backfill failed: {error}"));
                results.push(outcome);
                continue;
            }
        };
        let mut refresh_derived = false;
        if identity.action == IdentityAction::Wrote {
            outcome
                .actions
                .push("Backfilled mission identity metadata".to_string());
            refresh_derived = true;
        }
        if identity.number_coerced {
            outcome
                .actions
                .push("Normalized legacy mission_number type".to_string());
            refresh_derived = true;
        }

        if needs_event_log(&feature_dir) {
            let mission_id = if dry_run {
                pre_write_mission_id.clone().unwrap_or_default()
            } else {
                identity.mission_id.clone().unwrap_or_default()
            };
            if !mission_id.is_empty() && feature_dir.join(TASKS_DIR_NAME).is_dir() {
                if dry_run {
                    // Source reports the WP identity step it would take BEFORE the
                    // rebuild, on the same dry-run path that writes nothing.
                    outcome.actions.push(
                        "Would backfill missing WP identity fields before rebuilding the event log"
                            .to_string(),
                    );
                } else {
                    match backfill_wp_identity(&feature_dir, &mission_id, dry_run) {
                        Ok(_) => outcome
                            .actions
                            .push("Backfilled missing WP identity fields".to_string()),
                        Err(error) => {
                            outcome.status = "error";
                            outcome.error = Some(format!("WP identity backfill failed: {error}"));
                            results.push(outcome);
                            continue;
                        }
                    }
                }
            }
            if dry_run {
                outcome.actions.push(
                    "Would rebuild status.events.jsonl from legacy mission state".to_string(),
                );
                refresh_derived = true;
            } else {
                match rebuild_mission_event_log(&feature_dir, &slug, &BTreeMap::new()) {
                    // Source `_normalize_event_log`: a REPORTED rebuild error makes
                    // the mission `error` with `"; ".join(rebuild.errors)` and keeps
                    // the rebuild's own warnings; the "Rebuilt …" action is not
                    // recorded on that branch.
                    Ok(rebuild) if !rebuild.errors.is_empty() => {
                        outcome.status = "error";
                        outcome.error = Some(rebuild.errors.join("; "));
                        outcome.warnings.extend(rebuild.warnings);
                        results.push(outcome);
                        continue;
                    }
                    Ok(rebuild) => {
                        // Source always reports the rebuild, including the
                        // `0 synthetic events, 0 corrected` no-write case.
                        outcome.actions.push(format!(
                            "Rebuilt status.events.jsonl ({} synthetic events, {} corrected)",
                            rebuild.events_generated, rebuild.events_corrected
                        ));
                        outcome.warnings.extend(rebuild.warnings);
                        refresh_derived = true;
                    }
                    Err(error) => {
                        outcome.status = "error";
                        outcome.error = Some(format!("event log rebuild failed: {error}"));
                        results.push(outcome);
                        continue;
                    }
                }
            }
        }

        let derived = derived_dir(repo_root, &slug);
        refresh_derived = refresh_derived || needs_derived_refresh(&feature_dir, &derived);
        if refresh_derived {
            if dry_run {
                outcome
                    .actions
                    .push("Would regenerate canonical status/progress/lifecycle views".to_string());
            } else {
                match refresh_derived_views(&feature_dir, repo_root, now) {
                    Ok(_receipt) => {
                        // Source derives the lifecycle preview INSIDE the same
                        // materialization `try`, so a preview failure after a
                        // successful write is a materialization error, not a
                        // warning beside a reported success.
                        match derived_lifecycle_state(&feature_dir, now) {
                            Ok(state) => outcome.lifecycle_state = Some(state),
                            Err(error) => {
                                outcome.status = "error";
                                outcome.error =
                                    Some(format!("Lifecycle materialization failed: {error}"));
                                results.push(outcome);
                                continue;
                            }
                        }
                        // Source's action string carries no view count.
                        outcome.actions.push(
                            "Regenerated canonical status/progress/lifecycle views".to_string(),
                        );
                    }
                    Err(error) => {
                        outcome.status = "error";
                        outcome.error = Some(format!("Lifecycle materialization failed: {error}"));
                        results.push(outcome);
                        continue;
                    }
                }
            }
        }
        if outcome.lifecycle_state.is_none() {
            match derived_lifecycle_state(&feature_dir, now) {
                Ok(state) => outcome.lifecycle_state = Some(state),
                // Source warns and continues: an unavailable preview is not a
                // migration failure, but the warning must reach the operator.
                Err(error) => outcome
                    .warnings
                    .push(format!("Lifecycle preview unavailable: {error}")),
            }
        }
        if !outcome.actions.is_empty() {
            outcome.status = "normalized";
        }
        results.push(outcome);
    }
    results
}

/// The canonical lifecycle state for one mission, from the projection authority.
fn derived_lifecycle_state(
    feature_dir: &Path,
    now: sk_kernel::clock::UtcDateTime<sk_kernel::clock::UtcZone>,
) -> Result<String, RescueError> {
    sk_status::mission_projection::derive_lifecycle(feature_dir, None, now)
        .map(|lifecycle| lifecycle.classification.state.to_string())
        .map_err(|error| RescueError::Status(error.to_string()))
}

// ---------------------------------------------------------------------------
// Runtime cutover (seed -> fail-closed verify -> status_phase flip)
// ---------------------------------------------------------------------------

/// The placement port's answer for one mission (Source `_resolve_primary_home_or_degrade`).
///
/// The port itself lives in the canonical workspace crate; this type carries only
/// its answer so the phase writer can enforce it without a dependency cycle.
/// Source degrades ONLY for `WorkspaceRootNotFound`, `MissionSelectorAmbiguous`,
/// `StatusReadPathNotFound` and `ActionContextError`; every other resolver
/// failure — and every failure on the OWNED branch — is fatal and must reach the
/// caller as an error, never as a silent `Degraded`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlacementPort {
    /// The port resolved the mission's PRIMARY home.
    Resolved(PathBuf),
    /// The UNOWNED resolver failed with one of Source's four degrade classes.
    Degraded(String),
}

/// The owned single-branch facts the cutover leg needs, as a NEUTRAL PORT.
///
/// `sk-mission` must not depend on `sk-workspace` (that edge would close a
/// dependency cycle), so this port carries the facts Source's `OwnedCheckout`
/// fact exposes and nothing else: the canonical repository root, the owned
/// checkout's mission directory, the mission slug and the mission's STORED
/// topology. The caller composes it from the minted ownership fact
/// (`sk_workspace::OwnedCheckout`) — it is never assembled from a directory
/// name, a comment or a privately parsed owner marker, and it holds no copy of
/// the fact's identity state.
#[derive(Clone, Copy, Debug)]
pub struct OwnedMission<'a> {
    repository_root: &'a Path,
    mission_dir: &'a Path,
    mission_slug: &'a str,
    topology: MissionTopology,
}

impl<'a> OwnedMission<'a> {
    /// Compose the port from a caller-held ownership fact's own accessors.
    pub fn new(
        repository_root: &'a Path,
        mission_dir: &'a Path,
        mission_slug: &'a str,
        topology: MissionTopology,
    ) -> Self {
        Self {
            repository_root,
            mission_dir,
            mission_slug,
            topology,
        }
    }

    /// The canonical repository root (the owned checkout's lock root).
    pub fn repository_root(&self) -> &'a Path {
        self.repository_root
    }

    /// The owned checkout's exact mission directory.
    pub fn mission_dir(&self) -> &'a Path {
        self.mission_dir
    }

    /// The mission slug the ownership fact was minted for.
    pub fn mission_slug(&self) -> &'a str {
        self.mission_slug
    }

    /// The mission's durable stored topology.
    pub fn topology(&self) -> MissionTopology {
        self.topology
    }

    /// Source `migration.backfill_runtime_state._runtime_feature_dir`.
    ///
    /// An owned run keeps every phase anchored to the fact's own mission
    /// directory: a leg that names any other directory is refused with the
    /// reference's `OWNED_MISSION_PATH_REFUSED` message instead of being
    /// silently re-rooted. The comparison is the resolved path, exactly like
    /// Source's `feature_dir.resolve() != owned.mission_dir`.
    fn resolve_leg(&self, leg: &Path) -> Result<PathBuf, RescueError> {
        let resolved = std::fs::canonicalize(leg).map_err(|error| {
            RescueError::Unreadable(format!("cannot resolve {}: {error}", leg.display()))
        })?;
        // The fact's directory is already canonical (it was canonicalized when the
        // ownership fact was minted); resolve it once here so a non-canonical
        // spelling of the same directory is never mistaken for a different leg.
        let mission_dir = std::fs::canonicalize(self.mission_dir)
            .unwrap_or_else(|_| self.mission_dir.to_path_buf());
        if resolved != mission_dir {
            return Err(RescueError::Writer(
                "Runtime state must use the selected mission directory.".to_string(),
            ));
        }
        Ok(mission_dir)
    }
}

/// One mission's cutover request.
#[derive(Clone, Copy, Debug)]
pub struct CutoverRequest<'a> {
    /// The PRIMARY leg: the legacy `tasks/` read anchor, the identity read, and
    /// the sole `status_phase` write target.
    pub feature_dir: &'a Path,
    /// The COORD leg the seed events are appended against; `None` collapses both
    /// legs to `feature_dir` (the flat / single-branch case).
    pub status_feature_dir: Option<&'a Path>,
    /// Seed/flip nothing; report what would happen.
    pub dry_run: bool,
    /// The placement port's answer for this mission.
    pub placement: &'a PlacementPort,
    /// The write-seam residue answer, when the corpus walk already classified
    /// this mission. `None` means "decide here" (Source's single-mission path).
    pub residue: Option<bool>,
    /// The owned single-branch context, when the caller holds the ownership
    /// fact. `None` is Source's unowned leg (corpus walk / CLI without an owned
    /// checkout).
    pub owned: Option<OwnedMission<'a>>,
}

/// Per-mission outcome of [`cutover_mission`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CutoverOutcome {
    /// The mission slug.
    pub slug: String,
    /// True only when this run reached the flip with an `ok` verify (or the flip
    /// short-circuited on an already-authoritative phase).
    pub flipped: bool,
    /// True when this `kitty-specs/` child is residue, not a Mission: nothing was
    /// seeded, no `meta.json`/event-log stub was created (#6627).
    pub residue: bool,
    /// True when the flip target already declared a snapshot-authority phase.
    pub already_migrated: bool,
    /// NEW seed events appended this run.
    pub seeded_count: usize,
    /// The fail-closed verify outcome, when verify ran.
    pub verify: Option<RuntimeVerifyOutcome>,
    /// The abort reason (ordering/backfill/verify/placement), when the run did
    /// not flip.
    pub error: Option<String>,
    /// The operator notice when the run left the mission untouched because Git
    /// tracks its `meta.json` but the working tree lost it (#6673).
    pub skipped_notice: Option<String>,
}

impl CutoverOutcome {
    fn aborted(slug: &str, seeded_count: usize, error: String) -> Self {
        Self {
            slug: slug.to_string(),
            flipped: false,
            residue: false,
            already_migrated: false,
            seeded_count,
            verify: None,
            error: Some(error),
            skipped_notice: None,
        }
    }

    /// Source `CutoverResult(slug=…, flipped=False, residue=True)`.
    fn residue(slug: &str) -> Self {
        Self {
            slug: slug.to_string(),
            flipped: false,
            residue: true,
            already_migrated: false,
            seeded_count: 0,
            verify: None,
            error: None,
            skipped_notice: None,
        }
    }
}

/// What the flip decided.
enum Flip {
    /// The phase was written.
    Wrote,
    /// Already at snapshot authority: zero bytes written.
    AlreadyAuthoritative,
    /// The placement port's PRIMARY home disagrees with the write target.
    PlacementMismatch(String),
}

/// The flip target: the canonical mission directory (never a raw alias) on the
/// unowned leg, and the ownership fact's own mission directory on the owned leg
/// (Source `_flip_target`).
fn flip_target(feature_dir: &Path, owned: Option<OwnedMission<'_>>) -> PathBuf {
    match owned {
        Some(owned) => owned.mission_dir().to_path_buf(),
        None => sk_status::mutation_lock::canonicalize_feature_dir(feature_dir),
    }
}

/// The sole writer of `meta.json` `status_phase`.
///
/// Source order, preserved exactly: resolve the write target, then ENFORCE it
/// against the placement port's answer, and only then take the Mission lock,
/// read `meta.json` and short-circuit on an already-authoritative phase. The
/// comparison therefore also precedes the idempotent short-circuit. On the
/// owned leg the lock root is the ownership fact's repository root and the
/// legacy-tolerant `fallback_to_dir_name` key rule applies, exactly like Source's
/// `mission_write_lock(target, repo_root=owned.repository_root,
/// fallback_to_dir_name=True)`.
fn flip_status_phase(
    feature_dir: &Path,
    placement: &PlacementPort,
    owned: Option<OwnedMission<'_>>,
) -> Result<Flip, RescueError> {
    let target = flip_target(feature_dir, owned);
    if let PlacementPort::Resolved(home) = placement
        && *home != target
    {
        return Ok(Flip::PlacementMismatch(format!(
            "_flip_phase refuses to write status_phase for {:?}: the placement port resolved its              PRIMARY home to {}, which does not match the write target {} (fail-closed, FR-001).",
            feature_dir
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            home.display(),
            target.display()
        )));
    }
    let lock = sk_status::mutation::acquire_mission_lock_with(
        &target,
        owned.map(|fact| fact.repository_root()),
        sk_kernel::LockWait::Bounded(RESCUE_LOCK_TIMEOUT),
        // Source `mission_write_lock(..., fallback_to_dir_name=True)`: a legacy
        // mission whose lock key cannot be read is locked on its directory name.
        true,
        false,
    )
    .map_err(|error| RescueError::Writer(error.to_string()))?;
    let outcome = flip_status_phase_locked(&target);
    drop(lock);
    outcome
}

fn flip_status_phase_locked(target: &Path) -> Result<Flip, RescueError> {
    if sk_status::migration_backfill::is_snapshot_authority(target) {
        // Tolerant read + idempotent short-circuit: a re-run writes zero bytes.
        return Ok(Flip::AlreadyAuthoritative);
    }
    let mut meta = load_meta_object(target)?.unwrap_or_default();
    meta.insert(
        STATUS_PHASE_KEY.to_string(),
        Value::String(SNAPSHOT_AUTHORITY_PHASE.to_string()),
    );
    write_meta_object(target, &meta)?;
    Ok(Flip::Wrote)
}

/// Seed -> fail-closed verify -> atomic `status_phase` flip for one mission.
///
/// The flip is structurally unreachable on a non-`ok` verify or a per-mission
/// backfill error, and a tracked-but-lost `meta.json` stops the run before either
/// leg is touched (so no identity-less stub is ever created).
pub fn cutover_mission(request: CutoverRequest<'_>) -> CutoverOutcome {
    let owned = request.owned;
    let mut feature_dir = request.feature_dir;
    let mut status_dir = request.status_feature_dir.unwrap_or(feature_dir);
    // The owned leg resolves BOTH legs against the ownership fact before anything
    // else happens (Source validates the anchors first: before the residue check,
    // the tracked-metadata notice and the first seed write). The reference raises
    // `ActionContextError`; this infallible callback folds the same refusal
    // message into the abort carrier, which is what the corpus walk aborts on.
    let owned_feature_dir;
    let owned_status_dir;
    if let Some(fact) = owned {
        owned_feature_dir = match fact.resolve_leg(feature_dir) {
            Ok(path) => path,
            Err(error) => {
                return CutoverOutcome::aborted(&mission_slug(feature_dir), 0, error.to_string());
            }
        };
        owned_status_dir = match fact.resolve_leg(status_dir) {
            Ok(path) => path,
            Err(error) => {
                return CutoverOutcome::aborted(&mission_slug(feature_dir), 0, error.to_string());
            }
        };
        feature_dir = &owned_feature_dir;
        status_dir = &owned_status_dir;
    }
    let slug = mission_slug(feature_dir);
    // Source order: the write-seam residue check comes FIRST, before the
    // tracked-metadata notice and before any seed or flip, so a residue folder
    // never gets an event log or a one-key `meta.json` stub. An explicit owned
    // context names its Mission, so it skips the check.
    let residue = match request.residue {
        Some(residue) => residue,
        None => {
            let repo_root = corpus::repo_root_for(feature_dir);
            let corpus = corpus::MissionCorpus::new(&repo_root);
            corpus.is_residue(feature_dir)
        }
    };
    if owned.is_none() && residue {
        return CutoverOutcome::residue(&slug);
    }
    match tracked_meta::tracked_meta_notice(feature_dir) {
        Ok(Some(notice)) => {
            return CutoverOutcome {
                slug,
                flipped: false,
                residue: false,
                already_migrated: false,
                seeded_count: 0,
                verify: None,
                error: None,
                skipped_notice: Some(notice.notice),
            };
        }
        Ok(None) => {}
        Err(error) => return CutoverOutcome::aborted(&slug, 0, error.to_string()),
    }
    let seed = match sk_status::migration_backfill::backfill_runtime_state_with_legs_and_root(
        feature_dir,
        status_dir,
        request.dry_run,
        // Source threads the OWNED fact's repository root into the seed's status
        // lock (`resolve_status_lock_root(feature_dir, owned.repository_root)`),
        // so the seed and the flip below share one lock namespace. The unowned
        // corpus/CLI leg has no fact and keeps the mission directory's own root.
        owned.map(|fact| fact.repository_root()),
    ) {
        Ok(seed) => seed,
        Err(error) => return CutoverOutcome::aborted(&slug, 0, error.to_string()),
    };
    if seed.action == RuntimeBackfillAction::Error {
        let error = seed
            .reason
            .clone()
            .unwrap_or_else(|| "backfill failed".to_string());
        return CutoverOutcome::aborted(&slug, seed.seeded_count, error);
    }
    let verify = match sk_status::migration_backfill::verify_runtime_state_with_legs(
        feature_dir,
        status_dir,
    ) {
        Ok(verify) => verify,
        Err(error) => return CutoverOutcome::aborted(&slug, seed.seeded_count, error.to_string()),
    };
    if !verify.ok {
        return CutoverOutcome {
            slug,
            flipped: false,
            residue: false,
            already_migrated: false,
            seeded_count: seed.seeded_count,
            verify: Some(verify),
            error: None,
            skipped_notice: None,
        };
    }
    let already_migrated =
        sk_status::migration_backfill::is_snapshot_authority(&flip_target(feature_dir, owned));
    if request.dry_run {
        return CutoverOutcome {
            slug,
            flipped: false,
            residue: false,
            already_migrated,
            seeded_count: seed.seeded_count,
            verify: Some(verify),
            error: None,
            skipped_notice: None,
        };
    }
    match flip_status_phase(feature_dir, request.placement, owned) {
        Ok(Flip::PlacementMismatch(detail)) => {
            CutoverOutcome::aborted(&slug, seed.seeded_count, detail)
        }
        Ok(Flip::Wrote) | Ok(Flip::AlreadyAuthoritative) => CutoverOutcome {
            slug,
            flipped: true,
            residue: false,
            already_migrated,
            seeded_count: seed.seeded_count,
            verify: Some(verify),
            error: None,
            skipped_notice: None,
        },
        Err(error) => CutoverOutcome::aborted(&slug, seed.seeded_count, error.to_string()),
    }
}

/// Walk `kitty-specs/` and cut over every mission independently, in sorted order.
///
/// `placement` resolves the port's answer per mission; an `Err` is a FATAL
/// resolver failure and aborts that mission's cutover with the error, while
/// `Ok(PlacementPort::Degraded(_))` is Source's documented degrade.
pub fn cutover_repo(
    repo_root: &Path,
    dry_run: bool,
    placement: &dyn Fn(&Path) -> Result<PlacementPort, String>,
) -> Vec<CutoverOutcome> {
    // One walk, one residue probe: the classifier is the same per-Source-view
    // authority the upgrade callbacks use, so a residue child answers `residue`
    // instead of being seeded or given a `meta.json` stub.
    let corpus = corpus::MissionCorpus::new(repo_root);
    let candidates = match corpus.children() {
        Ok(children) => children,
        Err(_) => return Vec::new(),
    };
    candidates
        .into_iter()
        .map(|feature_dir| match placement(&feature_dir) {
            Ok(port) => cutover_mission(CutoverRequest {
                feature_dir: &feature_dir,
                status_feature_dir: None,
                dry_run,
                placement: &port,
                residue: Some(corpus.is_residue(&feature_dir)),
                // Source `cutover_repo` has no owned context: it walks the corpus
                // of a repository checkout, mission by mission.
                owned: None,
            }),
            Err(error) => CutoverOutcome::aborted(&mission_slug(&feature_dir), 0, error),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real repository: Source resolves the canonical root (and the tracked
    /// copy) through a `.git` anchor, and an ambient stale `.git` directory
    /// above the temporary root must not stand in for one.
    fn repo(root: &Path) {
        let status = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .status()
            .expect("git init");
        assert!(status.success(), "git init failed");
    }

    fn mission(root: &Path, slug: &str) -> PathBuf {
        std::fs::create_dir_all(root).expect("root");
        repo(root);
        let dir = root.join("kitty-specs").join(slug);
        std::fs::create_dir_all(dir.join("tasks")).expect("dirs");
        dir
    }

    fn write(path: &Path, text: &str) {
        std::fs::write(path, text).expect("write");
    }

    fn read_meta(dir: &Path) -> JsonMap<String, Value> {
        serde_json::from_str(&std::fs::read_to_string(dir.join("meta.json")).expect("meta"))
            .expect("json")
    }

    #[test]
    fn mints_a_mission_id_once_and_coerces_only_in_the_identity_variant() {
        let temp = tempfile::tempdir().expect("temp");
        let dir = mission(temp.path(), "mission-a");
        write(
            &dir.join("meta.json"),
            "{\n  \"mission_number\": \"042\",\n  \"slug\": \"mission-a\"\n}\n",
        );
        let first = backfill_mission_id(&dir, false).expect("id-only");
        assert_eq!(first.action, IdentityAction::Wrote);
        let minted = first.mission_id.clone().expect("minted");
        assert_eq!(minted.len(), 26);
        // The runner's seam leaves the legacy number untouched.
        assert_eq!(
            read_meta(&dir).get("mission_number"),
            Some(&Value::String("042".into()))
        );
        let again = backfill_mission_identity(&dir, false).expect("identity");
        assert_eq!(again.action, IdentityAction::Wrote);
        assert!(again.number_coerced);
        assert_eq!(
            read_meta(&dir).get("mission_number"),
            Some(&Value::from(42))
        );
        assert_eq!(
            read_meta(&dir).get("mission_id"),
            Some(&Value::String(minted.clone()))
        );
        let third = backfill_mission_identity(&dir, false).expect("idempotent");
        assert_eq!(third.action, IdentityAction::Skip);
        assert_eq!(third.mission_id, Some(minted));
    }

    #[test]
    fn a_sentinel_mission_number_is_refused_rather_than_guessed() {
        let temp = tempfile::tempdir().expect("temp");
        let dir = mission(temp.path(), "mission-a");
        write(
            &dir.join("meta.json"),
            "{\n  \"mission_number\": \"pending\",\n  \"slug\": \"mission-a\"\n}\n",
        );
        let error = backfill_mission_identity(&dir, false).expect_err("refusal");
        assert!(error.to_string().contains("must be int or null"));
    }

    #[test]
    fn identity_backfill_refuses_a_missing_meta_json() {
        let temp = tempfile::tempdir().expect("temp");
        let dir = mission(temp.path(), "mission-a");
        repo(temp.path());
        let outcome = backfill_mission_identity(&dir, false).expect("refusal");
        assert_eq!(outcome.action, IdentityAction::Refused);
        assert_eq!(outcome.reason.as_deref(), Some("meta.json not found"));
        assert!(!dir.join("meta.json").exists());
    }

    #[test]
    fn wp_identity_backfill_sets_the_three_ids_and_preserves_the_document() {
        let temp = tempfile::tempdir().expect("temp");
        let dir = mission(temp.path(), "mission-a");
        let wp = dir.join("tasks/WP01-alpha.md");
        write(
            &wp,
            "---\ntitle: alpha\n# authored comment\nwork_package_id: 01KJ00000000000000000000WP\n---\n\n# WP01 alpha\n",
        );
        let mapping = backfill_wp_identity(&dir, "01KJ00000000000000000000MC", false).expect("wp");
        assert_eq!(
            mapping.get("WP01"),
            Some(&"01KJ00000000000000000000WP".to_string())
        );
        let text = std::fs::read_to_string(&wp).expect("read");
        assert!(text.contains("mission_id: 01KJ00000000000000000000MC"));
        assert!(text.contains("wp_code: WP01"));
        // The immutable identity is never overwritten, and the body survives.
        assert!(text.contains("work_package_id: 01KJ00000000000000000000WP"));
        assert!(text.ends_with("\n# WP01 alpha\n"));
    }

    #[test]
    fn ownership_inference_matches_the_source_heuristics() {
        assert_eq!(
            infer_execution_mode("edit src/foo.rs and tests/foo.rs", &[]),
            WorkProductKind::CodeChange
        );
        assert_eq!(
            infer_execution_mode("write kitty-specs/001/spec.md", &[]),
            WorkProductKind::PlanningArtifact
        );
        assert_eq!(
            infer_owned_files("write kitty-specs/057-x/plan.md", "057-x"),
            vec!["kitty-specs/057-x/**".to_string()]
        );
        assert_eq!(
            infer_owned_files("touches `src/foo/bar.py`", "057-x"),
            vec!["src/foo/bar.py".to_string()]
        );
        assert_eq!(
            infer_owned_files("nothing path-like here", "057-x"),
            vec!["src/**".to_string()]
        );
        assert_eq!(
            infer_authoritative_surface(&["src/foo/bar.py".to_string()]),
            "src/foo/"
        );
    }

    #[test]
    fn ownership_backfill_writes_only_absent_fields() {
        let temp = tempfile::tempdir().expect("temp");
        let dir = mission(temp.path(), "057-x");
        let wp = dir.join("tasks/WP01-alpha.md");
        write(
            &wp,
            "---\ntitle: alpha\n---\n\n# WP01\n\nTouch `src/foo/bar.py`.\n",
        );
        let written = backfill_ownership(&dir, "057-x", false).expect("ownership");
        assert_eq!(written, 1);
        let text = std::fs::read_to_string(&wp).expect("read");
        assert!(text.contains("execution_mode: code_change"), "{text}");
        assert!(text.contains("- src/foo/bar.py"), "{text}");
        assert!(text.contains("authoritative_surface: src/foo/"), "{text}");
        // A second run sees every field present and writes nothing.
        assert_eq!(
            backfill_ownership(&dir, "057-x", false).expect("idempotent"),
            0
        );
    }

    #[test]
    fn strip_removes_the_mutable_fields_and_records_the_lane() {
        let temp = tempfile::tempdir().expect("temp");
        let dir = mission(temp.path(), "mission-a");
        let wp = dir.join("tasks/WP01-alpha.md");
        write(
            &wp,
            "---\nwork_package_id: WP01\nlane: in_progress\nshell_pid: 4242\nagent: claude\nassignee: ana\nprogress: 50\nhistory:\n  - event: started\ntracker_refs: [GH-1]\n---\n\nbody\n",
        );
        let outcome = strip_mutable_fields(&dir).expect("strip");
        assert_eq!(outcome.wps_processed, 1);
        assert_eq!(outcome.fields_stripped, 6);
        assert_eq!(
            outcome.lane_records.get("WP01"),
            Some(&"in_progress".to_string())
        );
        let text = std::fs::read_to_string(&wp).expect("read");
        for removed in [
            "lane:",
            "shell_pid:",
            "agent:",
            "assignee:",
            "progress:",
            "history:",
        ] {
            assert!(!text.contains(removed), "{removed} still present in {text}");
        }
        assert!(text.contains("tracker_refs: [GH-1]"));
        assert_eq!(
            strip_mutable_fields(&dir)
                .expect("idempotent")
                .fields_stripped,
            0
        );
    }

    #[test]
    fn rebuild_generates_the_deterministic_chain_to_done_and_is_stable() {
        let temp = tempfile::tempdir().expect("temp");
        let dir = mission(temp.path(), "mission-b");
        write(
            &dir.join("meta.json"),
            "{\n  \"created_at\": \"2026-02-03T04:05:06+00:00\",\n  \"mission_id\": \"01KJ00000000000000000000MB\"\n}\n",
        );
        write(
            &dir.join("tasks/WP01-beta.md"),
            "---\nlane: approved\n---\n\n# WP01 beta\n",
        );
        write(
            &dir.join(STATUS_FILE_NAME),
            "{\"materialized_at\": \"2026-02-03T05:00:00+00:00\", \"work_packages\": {\"WP01\": {\"lane\": \"approved\", \"last_transition_at\": \"2026-02-03T04:59:00+00:00\"}}}\n",
        );
        let first =
            rebuild_mission_event_log(&dir, "mission-b", &BTreeMap::new()).expect("rebuild");
        assert_eq!(first.events_generated, 4);
        let bytes = std::fs::read(dir.join(EVENTS_FILE_NAME)).expect("log");
        let rows: Vec<Value> = bytes
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).expect("row"))
            .collect();
        let lanes: Vec<&str> = rows
            .iter()
            .map(|row| row["to_lane"].as_str().expect("lane"))
            .collect();
        assert_eq!(lanes, ["claimed", "in_progress", "for_review", "done"]);
        // Source-recorded oracle ids and timestamps for this exact fixture tree.
        let ids: Vec<&str> = rows
            .iter()
            .map(|row| row["event_id"].as_str().expect("id"))
            .collect();
        assert_eq!(
            ids,
            [
                "71YWQAD16T0MDRWS605FWZCKYE",
                "3V90F8NG2FXHPSE38PTEJ534YH",
                "4B21WWF5X48R2YYAMDPM8X68DN",
                "6APQXQQYMGDQFHYRPW20GZ06WK",
            ]
        );
        let ats: Vec<&str> = rows
            .iter()
            .map(|row| row["at"].as_str().expect("at"))
            .collect();
        assert_eq!(
            ats,
            [
                "2026-02-03T04:45:01+00:00",
                "2026-02-03T04:50:01+00:00",
                "2026-02-03T04:55:01+00:00",
                "2026-02-03T05:00:01+00:00",
            ]
        );
        assert_eq!(rows[0]["actor"], "migration");
        assert_eq!(rows[0]["reason"], "bootstrapped from legacy state");
        // Deterministic: a second rebuild over the same corpus is byte-identical.
        let second = rebuild_mission_event_log(&dir, "mission-b", &BTreeMap::new()).expect("again");
        assert_eq!(second.events_generated, 0);
        assert_eq!(
            std::fs::read(dir.join(EVENTS_FILE_NAME)).expect("log"),
            bytes
        );
    }

    #[test]
    fn rebuild_emits_a_corrective_event_when_the_log_disagrees() {
        let temp = tempfile::tempdir().expect("temp");
        let dir = mission(temp.path(), "mission-b");
        write(
            &dir.join("meta.json"),
            "{\n  \"created_at\": \"2026-02-03T04:05:06+00:00\"\n}\n",
        );
        write(
            &dir.join("tasks/WP01-beta.md"),
            "---\nlane: in_progress\n---\n\nbody\n",
        );
        write(
            &dir.join(STATUS_FILE_NAME),
            "{\"materialized_at\": \"2026-02-03T05:00:00+00:00\", \"work_packages\": {\"WP01\": {\"lane\": \"done\"}}}\n",
        );
        let outcome =
            rebuild_mission_event_log(&dir, "mission-b", &BTreeMap::new()).expect("rebuild");
        assert_eq!(outcome.conflicts_found, 1);
        assert_eq!(outcome.events_generated, 4);
        assert!(!outcome.warnings.is_empty());
        assert_eq!(outcome.mission_slug, "mission-b");
    }

    #[test]
    fn cutover_flips_only_after_an_ok_verify_and_is_idempotent() {
        let temp = tempfile::tempdir().expect("temp");
        let dir = mission(temp.path(), "mission-c");
        write(
            &dir.join("meta.json"),
            "{\n  \"created_at\": \"2026-03-04T05:06:07+00:00\",\n  \"mission_id\": \"01KJ00000000000000000000MC\"\n}\n",
        );
        write(
            &dir.join("tasks/WP01-gamma.md"),
            "---\nshell_pid: 4242\nshell_pid_created_at: '2026-03-04T05:00:00+00:00'\nagent: claude\nassignee: ana\n---\n\nbody\n",
        );
        let first = cutover_mission(CutoverRequest {
            feature_dir: &dir,
            status_feature_dir: None,
            dry_run: false,
            placement: &PlacementPort::Degraded("test".to_string()),
            residue: None,
            owned: None,
        });
        assert!(first.flipped, "error: {:?}", first.error);
        assert_eq!(first.seeded_count, 2);
        assert_eq!(
            read_meta(&dir).get(STATUS_PHASE_KEY),
            Some(&Value::String("1".into()))
        );
        let second = cutover_mission(CutoverRequest {
            feature_dir: &dir,
            status_feature_dir: None,
            dry_run: false,
            placement: &PlacementPort::Degraded("test".to_string()),
            residue: None,
            owned: None,
        });
        assert!(second.flipped);
        assert!(second.already_migrated);
        assert_eq!(second.seeded_count, 0);
    }

    #[test]
    fn an_owned_leg_must_be_the_ownership_facts_mission_directory() {
        let temp = tempfile::tempdir().expect("temp");
        let dir = mission(temp.path(), "mission-owned");
        write(
            &dir.join("meta.json"),
            "{\n  \"created_at\": \"2026-05-06T07:08:09+00:00\",\n  \"mission_id\": \"01KJ00000000000000000000MO\"\n}\n",
        );
        write(
            &dir.join("tasks/WP01-omega.md"),
            "---\nshell_pid: 4242\nshell_pid_created_at: '2026-05-06T07:00:00+00:00'\nagent: claude\n---\n\nbody\n",
        );
        let sibling = mission(temp.path(), "mission-other");
        let owned = OwnedMission::new(
            temp.path(),
            &dir,
            "mission-owned",
            MissionTopology::SingleBranch,
        );
        // A leg naming any directory other than the fact's own Mission is refused
        // with Source's `OWNED_MISSION_PATH_REFUSED` message, before any write.
        let refused = cutover_mission(CutoverRequest {
            feature_dir: &sibling,
            status_feature_dir: None,
            dry_run: false,
            placement: &PlacementPort::Degraded("unused".to_string()),
            residue: None,
            owned: Some(owned),
        });
        assert!(!refused.flipped, "{refused:?}");
        assert_eq!(
            refused.error.as_deref(),
            Some("Runtime state must use the selected mission directory.")
        );
        assert!(!read_meta(&dir).contains_key(STATUS_PHASE_KEY));

        // The fact's own directory is accepted, and the owned leg SKIPS the residue
        // classification (Source only consults it when `owned is None`).
        let accepted = cutover_mission(CutoverRequest {
            feature_dir: &dir,
            status_feature_dir: None,
            dry_run: false,
            placement: &PlacementPort::Degraded("unused".to_string()),
            residue: Some(true),
            owned: Some(owned),
        });
        assert!(!accepted.residue, "{accepted:?}");
        assert!(accepted.flipped, "{accepted:?}");
        assert_eq!(
            read_meta(&dir).get(STATUS_PHASE_KEY),
            Some(&Value::String("1".into()))
        );
    }

    #[test]
    fn a_rebuild_write_failure_is_reported_with_its_own_channel() {
        let temp = tempfile::tempdir().expect("temp");
        let dir = mission(temp.path(), "mission-rebuild");
        write(
            &dir.join("meta.json"),
            "{\n  \"created_at\": \"2026-06-07T08:09:10+00:00\",\n  \"mission_id\": \"01KJ00000000000000000000MR\"\n}\n",
        );
        write(
            &dir.join("tasks/WP01-rho.md"),
            "---\nwork_package_id: WP01\nlane: approved\n---\n\nbody\n",
        );
        // The fixed `<name>.tmp` sibling is a directory, so the reference's
        // `open("w")` raises an ordinary OSError that it REPORTS.
        std::fs::create_dir_all(dir.join("status.events.jsonl.tmp")).expect("tmp dir");
        let outcome = rebuild_mission_event_log(&dir, "mission-rebuild", &BTreeMap::new())
            .expect("reported, not raised");
        assert_eq!(outcome.errors.len(), 1, "{outcome:?}");
        assert!(
            outcome.errors[0].starts_with("Failed to write event log for mission-rebuild: "),
            "{outcome:?}"
        );
        assert!(!dir.join(EVENTS_FILE_NAME).exists());

        // The normalizer joins the channel with `"; "` and keeps the mission's
        // actions out of `changes_made` (status `error`).
        let outcomes = normalize_mission_lifecycle(temp.path(), false);
        let normalized = outcomes
            .iter()
            .find(|candidate| candidate.slug == "mission-rebuild")
            .expect("visited");
        assert_eq!(normalized.status, "error", "{normalized:?}");
        assert!(
            normalized.error.as_deref().is_some_and(
                |error| error.starts_with("Failed to write event log for mission-rebuild: ")
            ),
            "{normalized:?}"
        );
    }

    #[test]
    fn cutover_never_flips_a_mission_whose_seed_diverged() {
        let temp = tempfile::tempdir().expect("temp");
        let dir = mission(temp.path(), "mission-z");
        write(
            &dir.join("meta.json"),
            "{\n  \"created_at\": \"2026-04-05T06:07:08+00:00\",\n  \"mission_id\": \"01KJ00000000000000000000MZ\"\n}\n",
        );
        write(
            &dir.join("tasks/WP01-delta.md"),
            "---\nshell_pid: 4242\nshell_pid_created_at: '2026-04-05T06:00:00+00:00'\nagent: claude\n---\n\nbody\n",
        );
        let claim_id = sk_status::migration_backfill::deterministic_ulid(
            "01KJ00000000000000000000MZ|WP01|claim",
        );
        write(
            &dir.join(EVENTS_FILE_NAME),
            &format!(
                "{{\"actor\": \"migration:backfill_runtime_state\", \"at\": \"2026-04-05T06:00:00+00:00\", \"event_id\": \"{claim_id}\", \"evidence\": null, \"execution_mode\": \"worktree\", \"force\": false, \"from_lane\": \"planned\", \"mission_id\": \"01KJ00000000000000000000MZ\", \"mission_slug\": \"mission-z\", \"policy_metadata\": {{\"agent\": \"claude\", \"shell_pid\": 9999, \"shell_pid_created_at\": \"2026-04-05T06:00:00+00:00\"}}, \"reason\": null, \"review_ref\": null, \"to_lane\": \"claimed\", \"wp_id\": \"WP01\"}}\n"
            ),
        );
        let outcome = cutover_mission(CutoverRequest {
            feature_dir: &dir,
            status_feature_dir: None,
            dry_run: false,
            placement: &PlacementPort::Degraded("test".to_string()),
            residue: None,
            owned: None,
        });
        assert!(!outcome.flipped);
        assert!(outcome.error.is_none());
        let verify = outcome.verify.expect("verify ran");
        assert!(!verify.ok);
        assert!(
            verify
                .mismatches
                .iter()
                .any(|mismatch| mismatch.contains("deterministic seed payload diverged"))
        );
        assert!(!read_meta(&dir).contains_key(STATUS_PHASE_KEY));
    }

    #[test]
    fn normalize_backfills_identity_rebuilds_the_log_and_writes_the_derived_view() {
        let temp = tempfile::tempdir().expect("temp");
        let dir = mission(temp.path(), "mission-b");
        write(
            &dir.join("meta.json"),
            "{\n  \"created_at\": \"2026-02-03T04:05:06+00:00\",\n  \"mission_number\": \"007\",\n  \"slug\": \"mission-b\"\n}\n",
        );
        write(
            &dir.join("tasks/WP01-beta.md"),
            "---\nlane: approved\n---\n\n# WP01 beta\n",
        );
        let outcomes = normalize_mission_lifecycle(temp.path(), false);
        assert_eq!(outcomes.len(), 1);
        let outcome = &outcomes[0];
        assert_eq!(outcome.status, "normalized", "outcome: {outcome:?}");
        assert!(
            outcome
                .actions
                .iter()
                .any(|action| action == "Backfilled mission identity metadata")
        );
        assert!(
            outcome
                .actions
                .iter()
                .any(|action| action == "Normalized legacy mission_number type")
        );
        assert!(
            outcome
                .actions
                .iter()
                .any(|action| action.starts_with("Rebuilt status.events.jsonl ("))
        );
        let meta = read_meta(&dir);
        assert!(meta.contains_key("mission_id"));
        assert_eq!(meta.get("mission_number"), Some(&Value::from(7)));
        assert!(dir.join(EVENTS_FILE_NAME).is_file());
        let derived = temp
            .path()
            .join(".kittify")
            .join(DERIVED_DIR_NAME)
            .join("mission-b")
            .join(STATUS_FILE_NAME);
        assert!(derived.is_file(), "derived status.json missing");
        // A second run finds everything normalized and writes nothing.
        let again = normalize_mission_lifecycle(temp.path(), false);
        assert_eq!(again[0].status, "skipped", "second: {:?}", again[0]);
        assert!(again[0].actions.is_empty());
    }

    #[test]
    fn normalize_reports_a_missing_meta_json_without_minting_an_identity() {
        let temp = tempfile::tempdir().expect("temp");
        let dir = mission(temp.path(), "mission-empty");
        write(&dir.join("tasks/WP01-alpha.md"), "---\n---\n\nbody\n");
        let outcomes = normalize_mission_lifecycle(temp.path(), false);
        assert_eq!(outcomes[0].status, "skipped");
        assert!(
            outcomes[0]
                .warnings
                .iter()
                .any(|warning| warning == "meta.json missing; mission skipped")
        );
        assert!(!dir.join("meta.json").exists());
    }
}

#[cfg(test)]
mod frontmatter_patch_tests {
    use super::*;

    const DOC: &str = "---\n# authored header\nwork_package_id: WP01  # keep this note\ntitle: 'quoted: title'\nlane: in_progress\nowned_files:\n- src/**\ndependencies: []\n---\n\n# body stays\n";

    #[test]
    fn a_value_replacement_keeps_the_authored_trailing_comment() {
        let doc = FrontmatterDoc::parse(DOC).expect("parse");
        let yaml = FrontmatterDoc::set_scalar(&doc.yaml, "work_package_id", "WP02");
        let edited = verified_edit(
            DOC,
            &yaml,
            &[("work_package_id", Some(yaml_scalar("WP02")))],
            &[],
            true,
        )
        .expect("verified");
        assert!(
            edited.contains("work_package_id: WP02  # keep this note"),
            "{edited}"
        );
        // Everything else — header comment, quoting, order, body — is untouched.
        assert!(edited.contains("# authored header"));
        assert!(edited.contains("title: 'quoted: title'"));
        assert!(edited.contains("dependencies: []"));
        assert!(edited.ends_with("\n# body stays\n"));
        let order: Vec<&str> = edited
            .split("---\n")
            .nth(1)
            .expect("frontmatter")
            .lines()
            .filter(|line| {
                !line.is_empty() && !line.starts_with([' ', '\t', '#', '-']) && line.contains(':')
            })
            .map(|line| line.split(':').next().unwrap_or_default())
            .collect();
        assert_eq!(
            order,
            [
                "work_package_id",
                "title",
                "dependencies",
                "lane",
                "owned_files"
            ]
        );
    }

    #[test]
    fn a_mutable_field_is_removed_without_touching_the_rest() {
        let doc = FrontmatterDoc::parse(DOC).expect("parse");
        let yaml = FrontmatterDoc::remove_key(&doc.yaml, "lane");
        let edited = verified_edit(DOC, &yaml, &[], &["lane"], true).expect("verified");
        assert!(!edited.contains("lane:"));
        assert!(edited.contains("# authored header"));
        assert!(edited.contains("work_package_id: WP01  # keep this note"));
        assert!(edited.ends_with("\n# body stays\n"));
    }

    #[test]
    fn a_sequence_field_round_trips_through_the_verified_patch() {
        let doc = FrontmatterDoc::parse(DOC).expect("parse");
        let items = vec!["src/feature.py".to_string()];
        let yaml = FrontmatterDoc::set_sequence(&doc.yaml, "owned_files", &items);
        let expected =
            serde_yaml_ng::Value::Sequence(items.iter().map(|item| yaml_scalar(item)).collect());
        let edited = verified_edit(DOC, &yaml, &[("owned_files", Some(expected))], &[], true)
            .expect("verified");
        assert!(
            edited.contains("owned_files:\n- src/feature.py\n"),
            "{edited}"
        );
        assert!(edited.ends_with("\n# body stays\n"));
    }

    #[test]
    fn an_edit_that_would_change_another_key_fails_closed() {
        let doc = FrontmatterDoc::parse(DOC).expect("parse");
        let broken = doc.yaml.replace("title: 'quoted: title'", "title: mangled");
        let error = verified_edit(DOC, &broken, &[], &[], true).expect_err("refusal");
        assert!(error.to_string().contains("title"), "{error}");
    }
}

#[cfg(test)]
mod frontmatter_bom_tests {
    use super::*;

    #[test]
    fn a_bom_prefixed_work_package_parses_and_is_emitted_without_the_bom() {
        let doc = FrontmatterDoc::parse("\u{feff}---\nwork_package_id: WP01\n---\n\nbody\n")
            .expect("parse");
        let rendered = doc.render(true);
        assert!(!rendered.starts_with('\u{feff}'), "{rendered:?}");
        assert_eq!(
            rendered,
            "---\nwork_package_id: WP01\ndependencies: []\n---\n\nbody\n"
        );
    }
}
