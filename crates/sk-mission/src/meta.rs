use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use sk_kernel::{KernelError, read_bounded};

use crate::error::MissionError;

/// Directory under a project root that holds Mission specification trees.
pub const SPECS_DIR_NAME: &str = "kitty-specs";

/// File inside each Mission directory that carries machine-facing identity.
pub const META_FILE_NAME: &str = "meta.json";

/// Default byte budget for one `meta.json` read.
///
/// Real `meta.json` files are a few kilobytes; the cap exists so a hostile or
/// accidental huge file is refused instead of loaded.
pub const DEFAULT_META_BYTE_LIMIT: u64 = 256 * 1024;

/// Describe a JSON value's kind for typed error messages.
pub(crate) fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Read `path` within `limit` bytes and decode it as a JSON object.
///
/// Every key present in the file — including fields this crate does not
/// interpret — is preserved in the returned map. The bytes are never
/// reserialized, so the source file keeps its formatting and comments.
pub(crate) fn load_meta_object(
    path: &Path,
    limit: u64,
) -> Result<Map<String, Value>, MissionError> {
    let bytes = read_bounded(path, limit).map_err(|source| match source {
        KernelError::TooLarge { limit, .. } => MissionError::MetadataTooLarge {
            path: path.to_path_buf(),
            limit,
        },
        other => MissionError::kernel(path, other),
    })?;

    let text = std::str::from_utf8(&bytes).map_err(|error| MissionError::MalformedMetadata {
        path: path.to_path_buf(),
        detail: format!("not valid UTF-8: {error}"),
    })?;

    let value: Value =
        serde_json::from_str(text).map_err(|error| MissionError::MalformedMetadata {
            path: path.to_path_buf(),
            detail: error.to_string(),
        })?;

    match value {
        Value::Object(map) => Ok(map),
        other => Err(MissionError::MetadataNotObject {
            path: path.to_path_buf(),
            kind: json_kind(&other),
        }),
    }
}

/// The bounded `acceptance_history` capacity (Source `HISTORY_CAP`).
pub const ACCEPTANCE_HISTORY_CAP: usize = 20;

/// Write one acceptance entry into `meta.json` — Source
/// `mission_metadata.record_acceptance`.
///
/// Sets `accepted_at` / `accepted_by` / `acceptance_mode`, CLEARS any stale
/// `accepted_from_commit` / `accept_commit`, then sets the ones this call
/// supplies, and appends one entry to the bounded `acceptance_history`. The
/// acceptance commit itself is stamped by [`stamp_accept_commit`] afterwards, as
/// Source does with its second commit.
#[allow(clippy::too_many_arguments)]
pub fn record_acceptance(
    feature_dir: &Path,
    accepted_by: &str,
    mode: &str,
    from_commit: Option<&str>,
    accept_commit: Option<&str>,
    accepted_at: &str,
) -> Result<PathBuf, MissionError> {
    let path = feature_dir.join(META_FILE_NAME);
    let mut meta = load_meta_object(&path, DEFAULT_META_BYTE_LIMIT)?;
    let mut entry = Map::new();
    entry.insert(
        "accepted_at".to_owned(),
        Value::String(accepted_at.to_owned()),
    );
    entry.insert(
        "accepted_by".to_owned(),
        Value::String(accepted_by.to_owned()),
    );
    entry.insert("acceptance_mode".to_owned(), Value::String(mode.to_owned()));
    if let Some(commit) = from_commit {
        entry.insert(
            "accepted_from_commit".to_owned(),
            Value::String(commit.to_owned()),
        );
    }
    if let Some(commit) = accept_commit {
        entry.insert("accept_commit".to_owned(), Value::String(commit.to_owned()));
    }

    meta.insert(
        "accepted_at".to_owned(),
        Value::String(accepted_at.to_owned()),
    );
    meta.insert(
        "accepted_by".to_owned(),
        Value::String(accepted_by.to_owned()),
    );
    meta.insert("acceptance_mode".to_owned(), Value::String(mode.to_owned()));
    meta.remove("accepted_from_commit");
    meta.remove("accept_commit");
    if let Some(commit) = from_commit {
        meta.insert(
            "accepted_from_commit".to_owned(),
            Value::String(commit.to_owned()),
        );
    }
    if let Some(commit) = accept_commit {
        meta.insert("accept_commit".to_owned(), Value::String(commit.to_owned()));
    }

    let mut history = match meta.remove("acceptance_history") {
        Some(Value::Array(entries)) => entries,
        _ => Vec::new(),
    };
    history.push(Value::Object(entry));
    if history.len() > ACCEPTANCE_HISTORY_CAP {
        let excess = history.len() - ACCEPTANCE_HISTORY_CAP;
        history.drain(0..excess);
    }
    meta.insert("acceptance_history".to_owned(), Value::Array(history));
    write_meta_canonical(&path, &meta)?;
    Ok(path)
}

/// Stamp `accept_commit` into `meta.json` and the newest history entry — Source
/// `acceptance._stamp_accept_commit`. `Ok(None)` when there is no `meta.json`.
pub fn stamp_accept_commit(
    feature_dir: &Path,
    accept_commit: &str,
) -> Result<Option<PathBuf>, MissionError> {
    let path = feature_dir.join(META_FILE_NAME);
    if !path.exists() {
        return Ok(None);
    }
    let mut meta = load_meta_object(&path, DEFAULT_META_BYTE_LIMIT)?;
    meta.insert(
        "accept_commit".to_owned(),
        Value::String(accept_commit.to_owned()),
    );
    if let Some(Value::Array(history)) = meta.get_mut("acceptance_history")
        && let Some(Value::Object(entry)) = history.last_mut()
    {
        entry.insert(
            "accept_commit".to_owned(),
            Value::String(accept_commit.to_owned()),
        );
    }
    write_meta_canonical(&path, &meta)?;
    Ok(Some(path))
}

/// The one canonical `meta.json` write (sorted keys, two-space pretty, readback).
fn write_meta_canonical(path: &Path, meta: &Map<String, Value>) -> Result<(), MissionError> {
    let mut text =
        serde_json::to_string_pretty(meta).map_err(|error| MissionError::MalformedMetadata {
            path: path.to_path_buf(),
            detail: error.to_string(),
        })?;
    text.push('\n');
    let bytes = text.into_bytes();
    publish_meta_bytes(
        path,
        &bytes,
        "meta.json readback does not match the recorded acceptance bytes",
    )
}

/// Keep one publication/readback/notification authority for metadata writers.
fn publish_meta_bytes(path: &Path, bytes: &[u8], readback_error: &str) -> Result<(), MissionError> {
    let changed = sk_kernel::atomic_write(path, bytes)
        .map_err(|source| MissionError::kernel(path, source))?;
    let landed = sk_kernel::read_bounded(path, DEFAULT_META_BYTE_LIMIT)
        .map_err(|source| MissionError::kernel(path, source))?;
    if landed != bytes {
        return Err(MissionError::MalformedMetadata {
            path: path.to_path_buf(),
            detail: readback_error.to_owned(),
        });
    }
    if changed {
        sk_kernel::source_writer::notify_written(path, bytes).map_err(|source| {
            MissionError::WriteObserver {
                path: path.to_path_buf(),
                source: Box::new(source),
            }
        })?;
    }
    Ok(())
}

/// Record ONLY `mission_number` into a Mission's `meta.json`.
///
/// This is Source's mission-number bake write
/// (`consolidation/mission_number/bake.py::_locked_assign_mission_number`): every
/// existing field (including extension fields) is preserved, only `mission_number`
/// is set, and the file keeps the canonical bytes (sorted keys, two-space pretty
/// JSON, one trailing newline). The write is atomic and read back; a readback
/// mismatch is an error, never a silent success.
///
/// The caller owns the git side: Source commits exactly this path on the MISSION
/// branch (`git commit --only -- <rel_meta>`) and fast-forwards the branch ref.
pub fn record_mission_number(
    feature_dir: &Path,
    mission_number: i64,
) -> Result<PathBuf, MissionError> {
    let path = feature_dir.join(META_FILE_NAME);
    if mission_number < 1 {
        return Err(MissionError::MalformedMetadata {
            path,
            detail: format!("mission_number {mission_number} is not a positive integer"),
        });
    }
    let mut meta = load_meta_object(&path, DEFAULT_META_BYTE_LIMIT)?;
    meta.insert(
        "mission_number".to_owned(),
        Value::Number(mission_number.into()),
    );
    let mut text =
        serde_json::to_string_pretty(&meta).map_err(|error| MissionError::MalformedMetadata {
            path: path.clone(),
            detail: error.to_string(),
        })?;
    text.push('\n');
    let bytes = text.into_bytes();
    publish_meta_bytes(
        &path,
        &bytes,
        "meta.json readback does not match the recorded mission_number bytes",
    )?;
    Ok(path)
}

/// Record `baseline_merge_commit` into a Mission's `meta.json`.
///
/// This is the one durable write consolidation performs on Mission metadata.
/// It re-reads the current object (every existing field, including extension
/// fields, is preserved), sets the marker to the real landed commit, and writes
/// the same canonical bytes the Mission creator writes: sorted keys, two-space
/// pretty JSON, one trailing newline. The write is atomic and read back; a
/// readback mismatch is an error, never a silent success.
///
/// `commit` is a full object id as `git rev-parse` printed it; an empty or
/// obviously short value is refused rather than recorded.
pub fn record_baseline_merge_commit(
    feature_dir: &Path,
    commit: &str,
) -> Result<PathBuf, MissionError> {
    let commit = commit.trim();
    if commit.len() < 7 || !commit.chars().all(|value| value.is_ascii_hexdigit()) {
        return Err(MissionError::MalformedMetadata {
            path: feature_dir.join(META_FILE_NAME),
            detail: format!("baseline merge commit {commit:?} is not a hex object id"),
        });
    }
    let path = feature_dir.join(META_FILE_NAME);
    let mut meta = load_meta_object(&path, DEFAULT_META_BYTE_LIMIT)?;
    meta.insert(
        "baseline_merge_commit".to_owned(),
        Value::String(commit.to_owned()),
    );
    let mut text =
        serde_json::to_string_pretty(&meta).map_err(|error| MissionError::MalformedMetadata {
            path: path.clone(),
            detail: error.to_string(),
        })?;
    text.push('\n');
    let bytes = text.into_bytes();
    publish_meta_bytes(
        &path,
        &bytes,
        "meta.json readback does not match the recorded baseline bytes",
    )?;
    Ok(path)
}

/// Clear the consolidation markers a rolled-back landing recorded.
///
/// Source's rollback door "rebaselines the consolidation record": nothing of the
/// failed attempt remains, so the next run reads the target's current tip as its
/// pre-mutation baseline. Only the consolidation-owned keys are removed; every
/// other field is preserved and the canonical bytes/readback contract is kept.
pub fn record_baseline_merge_commit_clear(feature_dir: &Path) -> Result<PathBuf, MissionError> {
    let path = feature_dir.join(META_FILE_NAME);
    if !path.is_file() {
        return Ok(path);
    }
    let mut meta = load_meta_object(&path, DEFAULT_META_BYTE_LIMIT)?;
    let mut changed = false;
    for key in ["baseline_merge_commit", "merged_commit", "merged_at"] {
        if meta.remove(key).is_some() {
            changed = true;
        }
    }
    if !changed {
        return Ok(path);
    }
    let mut text =
        serde_json::to_string_pretty(&meta).map_err(|error| MissionError::MalformedMetadata {
            path: path.clone(),
            detail: error.to_string(),
        })?;
    text.push('\n');
    let bytes = text.into_bytes();
    publish_meta_bytes(
        &path,
        &bytes,
        "meta.json readback does not match the cleared consolidation bytes",
    )?;
    Ok(path)
}

/// Record a verified PR landing as the Mission's review baseline.
///
/// Source's `record_pr_merge_baseline_for_mission` writes the canonical
/// `baseline_merge_commit` (= the landing commit's first parent) through the same
/// writer the local-merge path uses, then stamps the provenance pair
/// `pr_merge_commit` / `pr_merge_evidence`. Every field is set-once: a re-run
/// against an already-recorded Mission leaves the document byte-stable, so a
/// completed acceptance is never rewritten.
///
/// Returns `(path, changed)`.
pub fn record_pr_merge_baseline(
    feature_dir: &Path,
    baseline_merge_commit: &str,
    pr_merge_commit: &str,
    anchor_evidence: &str,
) -> Result<(PathBuf, bool), MissionError> {
    for (label, value) in [
        ("baseline merge commit", baseline_merge_commit),
        ("PR merge commit", pr_merge_commit),
    ] {
        let value = value.trim();
        if value.len() < 7 || !value.chars().all(|candidate| candidate.is_ascii_hexdigit()) {
            return Err(MissionError::MalformedMetadata {
                path: feature_dir.join(META_FILE_NAME),
                detail: format!("{label} {value:?} is not a hex object id"),
            });
        }
    }
    let path = feature_dir.join(META_FILE_NAME);
    let mut meta = load_meta_object(&path, DEFAULT_META_BYTE_LIMIT)?;
    let mut changed = false;
    let mut set_once = |meta: &mut serde_json::Map<String, Value>, key: &str, value: Value| {
        if meta.contains_key(key) {
            return;
        }
        meta.insert(key.to_owned(), value);
        changed = true;
    };
    set_once(
        &mut meta,
        "baseline_merge_commit",
        Value::String(baseline_merge_commit.trim().to_owned()),
    );
    set_once(
        &mut meta,
        "merged_commit",
        Value::String(pr_merge_commit.trim().to_owned()),
    );
    set_once(
        &mut meta,
        "merged_at",
        Value::String(sk_kernel::clock::now_utc_iso()),
    );
    set_once(
        &mut meta,
        "pr_merge_commit",
        Value::String(pr_merge_commit.trim().to_owned()),
    );
    set_once(
        &mut meta,
        "pr_merge_evidence",
        Value::String(anchor_evidence.to_owned()),
    );
    if !changed {
        return Ok((path, false));
    }
    let mut text =
        serde_json::to_string_pretty(&meta).map_err(|error| MissionError::MalformedMetadata {
            path: path.clone(),
            detail: error.to_string(),
        })?;
    text.push('\n');
    let bytes = text.into_bytes();
    publish_meta_bytes(
        &path,
        &bytes,
        "meta.json readback does not match the recorded PR baseline bytes",
    )?;
    Ok((path, true))
}

/// Record the consolidation metadata the landing phase owns: the baseline the
/// run started from, the commit it landed, the merge timestamp and the assigned
/// mission number.
///
/// This is the same writer as [`record_baseline_merge_commit`] extended with the
/// keys Source's done bookkeeping projects (`done_bookkeeping.py` +
/// `phase_bookkeeping.py`): `baseline_merge_commit`, `merged_commit`,
/// `merged_at`, `mission_number` and `status_phase`. Every other key — including
/// keys this crate does not interpret — is preserved, and the JSON body stays
/// key-sorted like the rest of `meta.json`.
pub fn record_consolidation_metadata(
    feature_dir: &Path,
    baseline: &str,
    merged_commit: &str,
    merged_at: &str,
    mission_number: i64,
) -> Result<PathBuf, MissionError> {
    for (label, value) in [
        ("baseline merge commit", baseline),
        ("merged commit", merged_commit),
    ] {
        let value = value.trim();
        if value.len() < 7 || !value.chars().all(|candidate| candidate.is_ascii_hexdigit()) {
            return Err(MissionError::MalformedMetadata {
                path: feature_dir.join(META_FILE_NAME),
                detail: format!("{label} {value:?} is not a hex object id"),
            });
        }
    }
    if merged_at.trim().is_empty() {
        return Err(MissionError::MalformedMetadata {
            path: feature_dir.join(META_FILE_NAME),
            detail: "the merge timestamp must not be empty".to_owned(),
        });
    }
    if mission_number < 1 {
        return Err(MissionError::MalformedMetadata {
            path: feature_dir.join(META_FILE_NAME),
            detail: format!("mission_number {mission_number} is not a positive integer"),
        });
    }
    let path = feature_dir.join(META_FILE_NAME);
    let mut meta = load_meta_object(&path, DEFAULT_META_BYTE_LIMIT)?;
    meta.insert(
        "baseline_merge_commit".to_owned(),
        Value::String(baseline.trim().to_owned()),
    );
    meta.insert(
        "merged_commit".to_owned(),
        Value::String(merged_commit.trim().to_owned()),
    );
    meta.insert(
        "merged_at".to_owned(),
        Value::String(merged_at.trim().to_owned()),
    );
    meta.insert(
        "mission_number".to_owned(),
        Value::Number(mission_number.into()),
    );
    if !meta.contains_key("status_phase") {
        meta.insert("status_phase".to_owned(), Value::String("1".to_owned()));
    }
    let mut text =
        serde_json::to_string_pretty(&meta).map_err(|error| MissionError::MalformedMetadata {
            path: path.clone(),
            detail: error.to_string(),
        })?;
    text.push('\n');
    let bytes = text.into_bytes();
    publish_meta_bytes(
        &path,
        &bytes,
        "meta.json readback does not match the recorded consolidation bytes",
    )?;
    Ok(path)
}
