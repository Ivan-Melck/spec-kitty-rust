//! The phase-1 legacy frontmatter `lane` mirror.
//!
//! Native counterpart of `specify_cli.status.emit._mirror_phase1_frontmatter_lane`
//! (+ `_read_status_phase` / `_legacy_lane_mirror_enabled`) at baseline
//! `9aed3a8012cb5eb22c0128b14d525039f95fea7a`.
//!
//! The event log is the sole authority for lane state; the frontmatter `lane`
//! field is a retiring phase-1 compatibility view. The mirror:
//!
//! - runs only when `meta.json` declares a numeric `status_phase >= 1`
//!   (a phase-2 mission still mirrors; a missing, malformed or non-numeric value
//!   turns it off);
//! - never creates the field — it updates an already-present top-level `lane:`;
//! - is a no-op when the recorded value already equals the canonical lane;
//! - is best-effort in the baseline (an unreadable or ambiguous WP file logs and
//!   skips, never failing the transition);
//! - resolves exactly one `tasks/*.md` file, and skips silently for zero or more
//!   than one.
//!
//! **Preservation.** The baseline round-trips the whole YAML frontmatter through
//! its `FrontmatterManager`. Doing that in Rust would reserialise the user's
//! comments and formatting, so this module performs a byte-local edit instead:
//! it rewrites only the scalar value of the top-level `lane:` line (keeping the
//! author's quoting style, a quoted key, an anchor and any trailing comment) and
//! leaves every other byte untouched. The result is the same field value with no
//! collateral reserialisation.
//!
//! **Validation.** The gate is the real `serde_yaml_ng` parse of the frontmatter
//! block plus the pinned `WPMetadata` rules, not a line-oriented whitelist. Block
//! sequences, aliases, quoted keys and legacy scalar coercions are therefore
//! accepted exactly as the reference reader accepts them; an unknown top-level
//! key (`extra="forbid"`) or an invalid required field still skips the rewrite.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::lane::Lane;
use crate::mutation::MutationError;
use crate::snapshot::META_FILENAME;

/// What the mirror did for one transition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MirrorOutcome {
    /// Phase-1 mode is off (no numeric `status_phase >= 1`).
    Disabled,
    /// No unique WP file, or the file has no `lane:` field: nothing to update.
    NoTarget,
    /// The recorded value already matches the canonical lane.
    Unchanged { path: PathBuf },
    /// The field was updated in place.
    Mirrored { path: PathBuf, lane: Lane },
    /// Best-effort skip with a diagnostic (unreadable WP file, malformed block).
    Skipped { path: PathBuf, reason: String },
}

/// Return the parsed `status_phase` from `meta.json`, or `None`.
///
/// Mirrors `_read_status_phase`: a missing/malformed file, a non-numeric string
/// and a `float` value all degrade to `None` (Python does `int(str(value))`,
/// which rejects `"1.0"`).
pub fn read_status_phase(feature_dir: &Path) -> Option<i64> {
    let path = feature_dir.join(META_FILENAME);
    let bytes = sk_kernel::read_bounded(&path, 1024 * 1024).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    let phase = value.get("status_phase")?;
    match phase {
        Value::Number(number) => {
            // A JSON float renders as `1.0` under Python's `str()`, which
            // `int()` rejects. Only an integer token parses.
            if number.is_f64() {
                None
            } else {
                number.as_i64()
            }
        }
        Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// True when the transitional lane mirror is active (`status_phase >= 1`).
pub fn legacy_lane_mirror_enabled(feature_dir: &Path) -> bool {
    read_status_phase(feature_dir).is_some_and(|phase| phase >= 1)
}

/// Mirror `lane` into an already-present frontmatter `lane` field.
pub fn mirror_phase1_frontmatter_lane(
    feature_dir: &Path,
    wp_id: &str,
    lane: Lane,
) -> Result<MirrorOutcome, MutationError> {
    if !legacy_lane_mirror_enabled(feature_dir) {
        return Ok(MirrorOutcome::Disabled);
    }
    let Some(path) = unique_wp_file(feature_dir, wp_id) else {
        return Ok(MirrorOutcome::NoTarget);
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            return Ok(MirrorOutcome::Skipped {
                path,
                reason: format!("unreadable ({error})"),
            });
        }
    };
    if let Err(reason) = validate_wp_frontmatter(&text) {
        // Baseline parity: the mirror is gated on a successful whole-model
        // `WPMetadata` read, so an off-model frontmatter logs and skips instead
        // of rewriting a field the rest of the tooling cannot read.
        return Ok(MirrorOutcome::Skipped { path, reason });
    }
    let Some(edited) = replace_frontmatter_lane(&text, lane) else {
        return Ok(MirrorOutcome::NoTarget);
    };
    if edited == text {
        return Ok(MirrorOutcome::Unchanged { path });
    }
    sk_kernel::atomic_write(&path, edited.as_bytes())?;
    Ok(MirrorOutcome::Mirrored { path, lane })
}

fn unique_wp_file(feature_dir: &Path, wp_id: &str) -> Option<PathBuf> {
    let tasks_dir = feature_dir.join("tasks");
    let mut matches = Vec::new();
    let Ok(entries) = std::fs::read_dir(&tasks_dir) else {
        return None;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if name.eq_ignore_ascii_case("readme.md") {
            continue;
        }
        // Baseline `_match_wp_files` is `^{wp_id}(?:[-_.]|\.md$)` and
        // case-SENSITIVE: `wp01.md` never matches `WP01`.
        let Some(rest) = name.strip_prefix(wp_id) else {
            continue;
        };
        if rest.is_empty() || rest.starts_with(['-', '_', '.']) {
            matches.push(path);
        }
    }
    matches.sort();
    match matches.len() {
        1 => matches.pop(),
        _ => None,
    }
}

/// The top-level keys the pinned `WPMetadata` model accepts (`extra="forbid"`).
const WP_METADATA_KEYS: &[&str] = &[
    "work_package_id",
    "title",
    "dependencies",
    "base_branch",
    "base_commit",
    "created_at",
    "planning_base_branch",
    "merge_target_branch",
    "branch_strategy",
    "requirement_refs",
    "tracker_refs",
    "priority",
    "execution_mode",
    "owned_files",
    "create_intent",
    "authoritative_surface",
    "scope",
    "task_type",
    "subtasks",
    "phase",
    "phases",
    "assignee",
    "agent",
    "model",
    "agent_profile",
    "role",
    "shell_pid",
    "shell_pid_created_at",
    "history",
    "lane",
    "feature_slug",
    "activity_log",
    "review_status",
    "reviewed_by",
    "approved_by",
    "reviewer",
    "reviewer_agent",
    "reviewer_shell_pid",
    "review_feedback",
    "review_feedback_file",
    "subtitle",
    "description",
    "estimated_duration",
    "tags",
    "mission_id",
    "mission_number",
    "mission_slug",
    "status",
    "wp_code",
    "work_package_title",
];

/// Validate the frontmatter exactly as the pinned `WPMetadata` read does.
///
/// The pinned mirror is gated on a successful whole-model `WPMetadata` read
/// (`status/emit.py`): invalid frontmatter logs and the mirror is skipped. This
/// validates the *same* YAML document the reference parses — with the real
/// `serde_yaml_ng` reader (anchors, aliases, flow and block collections, quoted
/// keys) — then applies the model's source rules:
///
/// - `extra="forbid"`: every top-level key must be a declared model field;
/// - required identity `work_package_id: str` matching `^WP\d{2,}$`;
/// - `title` non-blank when present;
/// - `dependencies`/`requirement_refs`/`tracker_refs`/`owned_files`/`tags`/
///   `create_intent` are legacy-coerced collections of scalars;
/// - `lane` resolves through the `doing` alias and refuses the non-display
///   sentinels;
/// - `shell_pid` coerces to an integer and `base_commit` stays a hex SHA.
///
/// A field this validator cannot classify is not silently accepted: an unknown
/// key (the `extra="forbid"` contract) refuses, and the mirror fails safe by
/// skipping the rewrite. The bytes on disk are never reserialised — the caller
/// applies a byte-local edit only after this read succeeds.
fn validate_wp_frontmatter(text: &str) -> Result<(), String> {
    let block = frontmatter_block(text)?;
    // Read-only compatibility for legacy hand-authored files that write a flow
    // collection without the separating space (`subtasks:[]`). The reference
    // dumper always emits the space, and the pinned reader rejects the compact
    // form; normalising only the parse input keeps the authored bytes untouched
    // while still validating through the real YAML reader. Every other shape is
    // parsed exactly as written.
    let normalized = normalize_legacy_flow_spacing(block);
    let document: serde_yaml_ng::Value = serde_yaml_ng::from_str(&normalized)
        .map_err(|error| format!("invalid YAML frontmatter: {error}"))?;
    let serde_yaml_ng::Value::Mapping(mapping) = document else {
        return Err("frontmatter is not a YAML mapping".to_owned());
    };
    let mut work_package_id = false;
    for (key, value) in &mapping {
        let serde_yaml_ng::Value::String(key) = key else {
            return Err(format!("non-string frontmatter key {key:?}"));
        };
        if !WP_METADATA_KEYS.contains(&key.as_str()) {
            return Err(format!("unknown frontmatter key {key:?}"));
        }
        match key.as_str() {
            "work_package_id" => {
                let Some(identity) = value.as_str() else {
                    return Err("work_package_id must be a string".to_owned());
                };
                if !canonical_wp_id(identity) {
                    return Err(format!(
                        "Invalid work_package_id: {identity:?} (must match WP##)"
                    ));
                }
                work_package_id = true;
            }
            "title" => match value {
                serde_yaml_ng::Value::Null => {}
                serde_yaml_ng::Value::String(title) if title.trim().is_empty() => {
                    return Err("title must not be empty".to_owned());
                }
                serde_yaml_ng::Value::String(_) => {}
                other => return Err(format!("title must be a string, got {other:?}")),
            },
            "lane" => validate_lane_value(value)?,
            "shell_pid" => validate_shell_pid(value)?,
            "base_commit" => validate_base_commit(value)?,
            "dependencies" | "requirement_refs" | "tracker_refs" | "owned_files" | "tags"
            | "create_intent" => validate_string_list(key, value)?,
            "subtasks" | "history" => match value {
                serde_yaml_ng::Value::Null => {
                    return Err(format!("{key} must not be null"));
                }
                serde_yaml_ng::Value::Sequence(_) => {}
                _ => return Err(format!("{key} must be a list")),
            },
            // `phases` is `str | None` on the model: a sequence is off-model.
            "phases" => match value {
                serde_yaml_ng::Value::Null | serde_yaml_ng::Value::String(_) => {}
                other => return Err(format!("phases must be a string, got {other:?}")),
            },
            _ => {}
        }
    }
    if !work_package_id {
        return Err("missing work_package_id".to_owned());
    }
    Ok(())
}

/// Insert the YAML separator space after a top-level key that is immediately
/// followed by a flow collection (`subtasks:[]` → `subtasks: []`).
///
/// This only changes the text handed to the parser; the caller's byte-local edit
/// still uses the authored bytes.
fn normalize_legacy_flow_spacing(block: &str) -> String {
    let mut output = String::with_capacity(block.len() + 8);
    for line in block.split_inclusive('\n') {
        let body = line.trim_end_matches(['\r', '\n']);
        let indented = body.starts_with(' ') || body.starts_with('\t');
        if !indented
            && let Some(colon) = body.find(':')
            && !body[..colon].contains(['\'', '"', ' ', '\t'])
            && matches!(body.as_bytes().get(colon + 1), Some(b'[') | Some(b'{'))
            && !body[..colon].is_empty()
        {
            output.push_str(&body[..=colon]);
            output.push(' ');
            output.push_str(&body[colon + 1..]);
            if line.ends_with("\r\n") {
                output.push_str("\r\n");
            } else if line.ends_with('\n') {
                output.push('\n');
            }
            continue;
        }
        output.push_str(line);
    }
    output
}

/// The frontmatter block between the `---` delimiters (delimiters excluded).
fn frontmatter_block(text: &str) -> Result<&str, String> {
    let mut lines = text.split_inclusive('\n');
    let Some(first) = lines.next() else {
        return Err("no YAML frontmatter block".to_owned());
    };
    if first.trim() != "---" {
        return Err("no YAML frontmatter block".to_owned());
    }
    let mut start = first.len();
    let mut end = None;
    for line in lines {
        if line.trim() == "---" {
            end = Some(start);
            break;
        }
        start += line.len();
    }
    let end = end.ok_or_else(|| "unterminated YAML frontmatter block".to_owned())?;
    Ok(&text[first.len()..end])
}

/// The pinned `^WP\d{2,}$` identity rule.
fn canonical_wp_id(value: &str) -> bool {
    let Some(digits) = value.strip_prefix("WP") else {
        return false;
    };
    digits.len() >= 2 && digits.bytes().all(|byte| byte.is_ascii_digit())
}

/// Validate a list-of-strings model field through its legacy coercion.
///
/// `WPMetadata._normalize_legacy_fields` coerces a scalar string into a list
/// (never refusing it), so a readable scalar is accepted; a sequence must hold
/// scalars (pydantic's lax `list[str]` coerces numbers/booleans too).
fn validate_string_list(field: &str, value: &serde_yaml_ng::Value) -> Result<(), String> {
    if matches!(value, serde_yaml_ng::Value::String(_)) {
        return Ok(());
    }
    let serde_yaml_ng::Value::Sequence(items) = value else {
        return Err(format!("{field} must be a list"));
    };
    for item in items {
        match item {
            serde_yaml_ng::Value::String(_)
            | serde_yaml_ng::Value::Number(_)
            | serde_yaml_ng::Value::Bool(_) => {}
            other => return Err(format!("{field} entries must be strings, got {other:?}")),
        }
    }
    Ok(())
}

/// `WPMetadata.coerce_lane`: alias resolution and non-display refusal.
fn validate_lane_value(value: &serde_yaml_ng::Value) -> Result<(), String> {
    let raw = match value {
        serde_yaml_ng::Value::Null => return Ok(()),
        serde_yaml_ng::Value::String(raw) => raw,
        other => return Err(format!("lane must be a string, got {other:?}")),
    };
    if raw.is_empty() {
        return Ok(());
    }
    let canonical = if raw == "doing" { "in_progress" } else { raw };
    match Lane::parse_canonical(canonical) {
        Ok(lane) if !Lane::NON_DISPLAY.contains(&lane) => Ok(()),
        _ => Err(format!("invalid lane value: {raw:?}")),
    }
}

/// `WPMetadata.coerce_shell_pid`: `int()` with `""`/`None` as absent.
fn validate_shell_pid(value: &serde_yaml_ng::Value) -> Result<(), String> {
    match value {
        serde_yaml_ng::Value::Null => Ok(()),
        serde_yaml_ng::Value::String(raw) if raw.is_empty() => Ok(()),
        serde_yaml_ng::Value::String(raw) => raw
            .trim()
            .parse::<i64>()
            .map(|_| ())
            .map_err(|_| format!("shell_pid must be an integer, got {raw:?}")),
        serde_yaml_ng::Value::Number(number) if number.as_i64().is_some() => Ok(()),
        other => Err(format!("shell_pid must be an integer, got {other:?}")),
    }
}

/// `WPMetadata.validate_base_commit`: `unknown` is absent, otherwise 7-40 hex.
fn validate_base_commit(value: &serde_yaml_ng::Value) -> Result<(), String> {
    match value {
        serde_yaml_ng::Value::Null => Ok(()),
        serde_yaml_ng::Value::String(raw) if raw == "unknown" => Ok(()),
        serde_yaml_ng::Value::String(raw) => {
            let valid =
                (7..=40).contains(&raw.len()) && raw.bytes().all(|byte| byte.is_ascii_hexdigit());
            if valid {
                Ok(())
            } else {
                Err(format!("Invalid base_commit: {raw:?} (must be hex SHA)"))
            }
        }
        other => Err(format!("base_commit must be a string, got {other:?}")),
    }
}

/// Rewrite the top-level `lane:` scalar inside the frontmatter block.
///
/// Returns `None` when the file has no frontmatter block or no `lane:` field
/// (both are "nothing to update"); returns the input unchanged when the recorded
/// value already equals `lane`.
///
/// Only the scalar token is replaced. The key spelling (including a quoted
/// `"lane"` key), separation spaces, an anchor, comments, quoting style and the
/// line ending (LF/CRLF) are preserved byte-for-byte; nothing outside the
/// frontmatter block is read. YAML parsing already validated the document, so a
/// mature frontmatter using block sequences or aliases is a supported input.
fn replace_frontmatter_lane(text: &str, lane: Lane) -> Option<String> {
    let canonical = lane.as_str();
    let mut output = String::with_capacity(text.len() + 8);
    let mut lines = text.split_inclusive('\n');
    let first = lines.next()?;
    if first.trim() != "---" {
        return None;
    }
    output.push_str(first);
    let mut inside = true;
    let mut found = false;
    for line in lines {
        if !inside {
            output.push_str(line);
            continue;
        }
        if line.trim() == "---" {
            inside = false;
            output.push_str(line);
            continue;
        }
        let body = line.trim_end_matches(['\r', '\n']);
        let Some((key, colon)) = top_level_key(body) else {
            output.push_str(line);
            continue;
        };
        if key != "lane" {
            output.push_str(line);
            continue;
        }
        found = true;
        let rest = &body[colon + 1..];
        let (token_start, token_end, quote) = lane_value_span(rest);
        if unquote_span(&rest[token_start..token_end]) == canonical {
            output.push_str(line);
            continue;
        }
        output.push_str(&body[..=colon]);
        output.push_str(&rest[..token_start]);
        if token_start == 0 {
            // An absent scalar (or a comment-only one) still needs the
            // canonical separator after the colon.
            output.push(' ');
        }
        if let Some(quote) = quote {
            output.push(quote);
        }
        output.push_str(canonical);
        if let Some(quote) = quote {
            output.push(quote);
        }
        let suffix = &rest[token_end..];
        if token_start == token_end && suffix.trim_start().starts_with('#') {
            // `lane:` with only a trailing comment: keep the comment with the
            // canonical separator the author's line style expects.
            output.push_str("  ");
            output.push_str(suffix.trim_start());
        } else {
            output.push_str(suffix);
        }
        if line.ends_with("\r\n") {
            output.push_str("\r\n");
        } else if line.ends_with('\n') {
            output.push('\n');
        }
    }
    if found { Some(output) } else { None }
}

/// The unquoted top-level key of a frontmatter line and the index of its
/// separating colon. A line that is indented, blank, a comment, or has no
/// top-level colon is not a key line.
fn top_level_key(body: &str) -> Option<(String, usize)> {
    if body.starts_with(' ') || body.starts_with('\t') {
        return None;
    }
    let trimmed = body.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('-') {
        return None;
    }
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (index, character) in body.char_indices() {
        if let Some(active) = quote {
            if active == '"' {
                if escaped {
                    escaped = false;
                } else if character == '\\' {
                    escaped = true;
                } else if character == '"' {
                    quote = None;
                }
            } else if character == '\'' {
                quote = None;
            }
            continue;
        }
        match character {
            '"' | '\'' => quote = Some(character),
            ':' => {
                let raw = body[..index].trim_end();
                if raw.is_empty() {
                    return None;
                }
                let key = unquote_span(raw);
                return (!key.is_empty()).then_some((key.to_owned(), index));
            }
            _ => {}
        }
    }
    None
}

/// The scalar token span of a `lane:` value, plus the author's quote character.
fn lane_value_span(rest: &str) -> (usize, usize, Option<char>) {
    let bytes = rest.as_bytes();
    let mut index = 0;
    while index < bytes.len() && (bytes[index] == b' ' || bytes[index] == b'\t') {
        index += 1;
    }
    // Preserve an explicit anchor (`lane: &lane planned`) by leaving it in the
    // untouched prefix; only the scalar after it is replaced.
    if index < bytes.len() && bytes[index] == b'&' {
        while index < bytes.len() && bytes[index] != b' ' && bytes[index] != b'\t' {
            index += 1;
        }
        while index < bytes.len() && (bytes[index] == b' ' || bytes[index] == b'\t') {
            index += 1;
        }
    }
    if index >= bytes.len() {
        return (rest.len(), rest.len(), None);
    }
    let start = index;
    if bytes[index] == b'#' {
        // A comment-only value is an absent scalar; the comment is preserved
        // and the replacement writes one canonical separator before it.
        return (0, 0, None);
    }
    match bytes[index] {
        b'"' | b'\'' => {
            let quote = bytes[index] as char;
            index += 1;
            let mut escaped = false;
            while index < bytes.len() {
                let byte = bytes[index];
                if quote == '"' {
                    if escaped {
                        escaped = false;
                    } else if byte == b'\\' {
                        escaped = true;
                    } else if byte == b'"' {
                        index += 1;
                        break;
                    }
                } else if byte == b'\'' {
                    if index + 1 < bytes.len() && bytes[index + 1] == b'\'' {
                        index += 2;
                        continue;
                    }
                    index += 1;
                    break;
                }
                index += 1;
            }
            (start, index, Some(quote))
        }
        _ => {
            while index < bytes.len() {
                if bytes[index] == b'#'
                    && index > start
                    && (bytes[index - 1] == b' ' || bytes[index - 1] == b'\t')
                {
                    break;
                }
                index += 1;
            }
            while index > start && (bytes[index - 1] == b' ' || bytes[index - 1] == b'\t') {
                index -= 1;
            }
            (start, index, None)
        }
    }
}

/// Strip one pair of matching quotes from a scalar span.
fn unquote_span(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
    {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phase_meta(dir: &Path, raw: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(META_FILENAME), raw).unwrap();
    }

    #[test]
    fn status_phase_parsing_matches_the_python_int_str_contract() {
        let temp = tempfile::tempdir().unwrap();
        phase_meta(temp.path(), r#"{"status_phase": 2}"#);
        assert_eq!(read_status_phase(temp.path()), Some(2));
        phase_meta(temp.path(), r#"{"status_phase": "1"}"#);
        assert_eq!(read_status_phase(temp.path()), Some(1));
        phase_meta(temp.path(), r#"{"status_phase": " 1 "}"#);
        assert_eq!(read_status_phase(temp.path()), Some(1));
        phase_meta(temp.path(), r#"{"status_phase": 1.0}"#);
        assert_eq!(read_status_phase(temp.path()), None);
        phase_meta(temp.path(), r#"{"status_phase": "1.0"}"#);
        assert_eq!(read_status_phase(temp.path()), None);
        phase_meta(temp.path(), r#"{"status_phase": true}"#);
        assert_eq!(read_status_phase(temp.path()), None);
        phase_meta(temp.path(), r#"{"status_phase": "one"}"#);
        assert_eq!(read_status_phase(temp.path()), None);
        phase_meta(temp.path(), "not json");
        assert_eq!(read_status_phase(temp.path()), None);
    }

    #[test]
    fn the_mirror_updates_only_an_existing_field_and_preserves_everything_else() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("kitty-specs/001-demo");
        phase_meta(&dir, r#"{"status_phase": 1}"#);
        std::fs::create_dir_all(dir.join("tasks")).unwrap();
        let wp = dir.join("tasks/WP01.md");
        let original = "---\n# authored comment\nwork_package_id: WP01\ntitle: Demo\nlane: planned  # phase-1\ndependencies: []\n---\n\nBody lane: keep me\n";
        std::fs::write(&wp, original).unwrap();

        let outcome = mirror_phase1_frontmatter_lane(&dir, "WP01", Lane::Claimed).unwrap();
        assert!(matches!(outcome, MirrorOutcome::Mirrored { .. }));
        let updated = std::fs::read_to_string(&wp).unwrap();
        assert_eq!(
            updated,
            "---\n# authored comment\nwork_package_id: WP01\ntitle: Demo\nlane: claimed  # phase-1\ndependencies: []\n---\n\nBody lane: keep me\n"
        );

        // Idempotent second call.
        let again = mirror_phase1_frontmatter_lane(&dir, "WP01", Lane::Claimed).unwrap();
        assert!(matches!(again, MirrorOutcome::Unchanged { .. }));
        assert_eq!(std::fs::read_to_string(&wp).unwrap(), updated);
    }

    #[test]
    fn a_missing_lane_field_is_never_created_and_phase_zero_is_off() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("kitty-specs/001-demo");
        phase_meta(&dir, r#"{"status_phase": 1}"#);
        std::fs::create_dir_all(dir.join("tasks")).unwrap();
        let wp = dir.join("tasks/WP01.md");
        let original = "---\nwork_package_id: WP01\ntitle: Demo\n---\nbody\n";
        std::fs::write(&wp, original).unwrap();
        assert_eq!(
            mirror_phase1_frontmatter_lane(&dir, "WP01", Lane::Claimed).unwrap(),
            MirrorOutcome::NoTarget
        );
        assert_eq!(std::fs::read_to_string(&wp).unwrap(), original);

        phase_meta(&dir, r#"{"status_phase": 0}"#);
        assert_eq!(
            mirror_phase1_frontmatter_lane(&dir, "WP01", Lane::Claimed).unwrap(),
            MirrorOutcome::Disabled
        );
    }

    #[test]
    fn quoted_values_keep_their_quote_style() {
        let text = "---\nwork_package_id: WP01\nlane: \"planned\"\n---\n";
        assert_eq!(
            replace_frontmatter_lane(text, Lane::Approved).unwrap(),
            "---\nwork_package_id: WP01\nlane: \"approved\"\n---\n"
        );
    }

    #[test]
    fn an_off_model_frontmatter_is_skipped_like_the_baseline_wpmetadata_read() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("kitty-specs/001-demo");
        phase_meta(&dir, r#"{"status_phase": 1}"#);
        std::fs::create_dir_all(dir.join("tasks")).unwrap();
        let wp = dir.join("tasks/WP01.md");

        // An unknown top-level key is `extra="forbid"` in the pinned model.
        let unknown = "---\nwork_package_id: WP01\nlane: planned\nextra: {lane: planned}\n---\n";
        std::fs::write(&wp, unknown).unwrap();
        let outcome = mirror_phase1_frontmatter_lane(&dir, "WP01", Lane::Claimed).unwrap();
        assert!(
            matches!(outcome, MirrorOutcome::Skipped { .. }),
            "{outcome:?}"
        );
        assert_eq!(std::fs::read_to_string(&wp).unwrap(), unknown);

        // A missing required identity is the same validation failure.
        let missing = "---\nlane: planned\n---\n";
        std::fs::write(&wp, missing).unwrap();
        assert!(matches!(
            mirror_phase1_frontmatter_lane(&dir, "WP01", Lane::Claimed).unwrap(),
            MirrorOutcome::Skipped { .. }
        ));
        assert_eq!(std::fs::read_to_string(&wp).unwrap(), missing);

        // The legacy `doing` alias is a VALID model value (it coerces to
        // `in_progress`), so this is a mirrored input, not an off-model skip.
        let aliased = "---\nwork_package_id: WP01\nlane: doing\n---\n";
        std::fs::write(&wp, aliased).unwrap();
        assert!(matches!(
            mirror_phase1_frontmatter_lane(&dir, "WP01", Lane::Claimed).unwrap(),
            MirrorOutcome::Mirrored { .. }
        ));
        assert_eq!(
            std::fs::read_to_string(&wp).unwrap(),
            "---\nwork_package_id: WP01\nlane: claimed\n---\n"
        );

        // A genuinely unknown lane value still refuses the whole model read.
        let bad_lane = "---\nwork_package_id: WP01\nlane: somewhere\n---\n";
        std::fs::write(&wp, bad_lane).unwrap();
        assert!(matches!(
            mirror_phase1_frontmatter_lane(&dir, "WP01", Lane::Claimed).unwrap(),
            MirrorOutcome::Skipped { .. }
        ));
    }

    #[test]
    fn the_wp_file_match_is_case_sensitive_and_allows_the_baseline_separators() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("kitty-specs/001-demo");
        phase_meta(&dir, r#"{"status_phase": 1}"#);
        std::fs::create_dir_all(dir.join("tasks")).unwrap();
        let lower = dir.join("tasks/wp01.md");
        let original = "---\nwork_package_id: WP01\nlane: planned\n---\n";
        std::fs::write(&lower, original).unwrap();
        assert_eq!(
            mirror_phase1_frontmatter_lane(&dir, "WP01", Lane::Claimed).unwrap(),
            MirrorOutcome::NoTarget
        );
        assert_eq!(std::fs::read_to_string(&lower).unwrap(), original);

        let suffixed = dir.join("tasks/WP01-run-state.md");
        std::fs::write(&suffixed, original).unwrap();
        assert!(matches!(
            mirror_phase1_frontmatter_lane(&dir, "WP01", Lane::Claimed).unwrap(),
            MirrorOutcome::Mirrored { .. }
        ));
        assert_eq!(
            std::fs::read_to_string(&suffixed).unwrap(),
            "---\nwork_package_id: WP01\nlane: claimed\n---\n"
        );
        // The lower-case file is never the mirror target.
        assert_eq!(std::fs::read_to_string(&lower).unwrap(), original);
    }

    #[test]
    fn block_sequences_aliases_and_quoted_keys_are_valid_model_input() {
        // Oracle-valid shape: the pinned `WPMetadata` parses this (quoted key,
        // anchor/alias, column-0 block sequence). The previous line whitelist
        // treated the block sequence as unclassified and skipped the mirror.
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("kitty-specs/001-demo");
        phase_meta(&dir, r#"{"status_phase": 1}"#);
        std::fs::create_dir_all(dir.join("tasks")).unwrap();
        let wp = dir.join("tasks/WP01.md");
        let original = "---\nwork_package_id: WP01\n\"title\": \"Demo\"\nsubtasks:\n- T001\n- T002\ndependencies: &deps\n- WP00\nrequirement_refs: *deps\nlane: 'planned'\n---\n\nBody\n";
        std::fs::write(&wp, original).unwrap();

        let outcome = mirror_phase1_frontmatter_lane(&dir, "WP01", Lane::Claimed).unwrap();
        assert!(
            matches!(outcome, MirrorOutcome::Mirrored { .. }),
            "{outcome:?}"
        );
        let updated = std::fs::read_to_string(&wp).unwrap();
        assert_eq!(
            updated,
            original.replace("lane: 'planned'", "lane: 'claimed'")
        );
        assert!(updated.contains("subtasks:\n- T001\n- T002\n"));
        assert!(updated.contains("requirement_refs: *deps\n"));
        assert!(updated.contains("\"title\": \"Demo\"\n"));
    }

    #[test]
    fn a_quoted_lane_key_an_anchor_and_crlf_are_preserved() {
        let text = "---\r\nwork_package_id: WP01\r\n\"lane\" : &phase1 \"planned\"  # filled by hand\r\n---\r\nbody\r\n";
        assert_eq!(
            replace_frontmatter_lane(text, Lane::Approved).unwrap(),
            "---\r\nwork_package_id: WP01\r\n\"lane\" : &phase1 \"approved\"  # filled by hand\r\n---\r\nbody\r\n"
        );
        // A body line that merely looks like a lane assignment is not touched.
        assert_eq!(
            replace_frontmatter_lane(
                "---\nwork_package_id: WP01\nlane: planned\n---\nlane: nope\n",
                Lane::Done
            )
            .unwrap(),
            "---\nwork_package_id: WP01\nlane: done\n---\nlane: nope\n"
        );
        // An empty value with a trailing comment keeps the comment.
        assert_eq!(
            replace_frontmatter_lane(
                "---\nwork_package_id: WP01\nlane:  # set by the operator\n---\n",
                Lane::Claimed
            )
            .unwrap(),
            "---\nwork_package_id: WP01\nlane: claimed  # set by the operator\n---\n"
        );
    }
}
