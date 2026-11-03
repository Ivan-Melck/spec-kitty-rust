//! Durable, whole-file-replacement writers for the two canonical JSONL logs.
//!
//! The baseline `status.store.append_raw_rows_atomic` never appends with
//! `O_APPEND` and neither does this module. It reads the existing text, appends
//! the canonical rows, writes a temporary file, fsyncs it, renames it over the
//! log and fsyncs the directory. A concurrent reader therefore observes the
//! complete old log or the complete new log, never a torn one.
//!
//! The writer owns four things the baseline treated as invariants:
//!
//! 1. **Symlink refusal.** A final-component symlink at the log path is refused
//!    before anything is read or replaced (`O_NOFOLLOW` parity).
//! 2. **Malformed-stream refusal.** Every non-blank existing line must be a JSON
//!    object. A corrupt line is reported with its 1-based line number and the
//!    log is left byte-identical.
//! 3. **Newline repair, not truncation.** A log whose last line lacks a newline
//!    gets one inserted before the new rows — the existing bytes are preserved
//!    (baseline parity; no torn-tail truncation, which would delete user data).
//! 4. **Readback and indeterminate recovery.** The replacement is verified by
//!    re-reading the file and comparing bytes. An
//!    [`sk_kernel::KernelError::AtomicCommitIndeterminate`] is recovered by
//!    reading the file back: present-and-correct is success (reported as
//!    [`Durability::IndeterminateRecovered`]), anything else is reported for
//!    recovery and never silently deleted.
//!
//! Rollback is a separate, explicit operation ([`capture_log_snapshot`] /
//! [`restore_log_snapshot`]) so a caller can undo its own batch only while the
//! file still holds exactly the bytes it wrote.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::mutation::MutationError;
use crate::mutation_json::serialize_event_line;
use crate::snapshot::{
    EVENTS_FILENAME, ReplayInput, SNAPSHOT_FILENAME, Snapshot, materialize, render_snapshot_json,
};

/// Upper bound on a status/lifecycle log read or replacement (64 MiB).
///
/// The baseline bounds nothing; a bounded read keeps a corrupted or hostile
/// file from exhausting memory before the write refuses.
pub const MAX_EVENT_LOG_BYTES: u64 = 64 * 1024 * 1024;

/// Options for one atomic batch append.
#[derive(Clone, Debug)]
pub struct AppendOptions {
    /// Create missing parent directories (baseline `mkdir(parents=True)`).
    pub create_parents: bool,
    /// Maximum accepted size of the resulting log, in bytes.
    pub max_bytes: u64,
}

impl Default for AppendOptions {
    fn default() -> Self {
        Self {
            create_parents: true,
            max_bytes: MAX_EVENT_LOG_BYTES,
        }
    }
}

/// Whether the replacement's durability was directly confirmed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Durability {
    /// Temp-file fsync, rename and parent-directory sync all succeeded.
    Confirmed,
    /// The kernel could not confirm the parent-directory sync, but reading the
    /// file back proved the intended content is present. The write happened;
    /// only the crash window is wider.
    IndeterminateRecovered,
}

/// The result of one verified batch append.
#[derive(Clone, Debug)]
pub struct AppendOutcome {
    /// The log that was replaced.
    pub path: PathBuf,
    /// Whether the log existed before this append.
    pub existed_before: bool,
    /// Log size before the append.
    pub previous_bytes: u64,
    /// Log size after the append.
    pub written_bytes: u64,
    /// Number of rows appended.
    pub rows_appended: usize,
    /// Durability of the replacement.
    pub durability: Durability,
}

/// A pre-append copy of a log, for explicit rollback.
#[derive(Clone, Debug)]
pub struct LogSnapshot {
    /// The log's path.
    pub path: PathBuf,
    /// The exact bytes before the write, or `None` when the log was absent.
    pub bytes: Option<Vec<u8>>,
}

/// Read a log as text, refusing a final-component symlink.
///
/// Returns `Ok(None)` when the file is absent. Blank content is returned as an
/// empty string. Invalid UTF-8 and oversized files are typed refusals.
pub fn read_log_text(path: &Path) -> Result<Option<String>, MutationError> {
    refuse_symlink(path)?;
    match sk_kernel::no_follow::read_text_no_follow(path, MAX_EVENT_LOG_BYTES) {
        Ok(text) => Ok(Some(text)),
        Err(sk_kernel::KernelError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

/// Parse a log's text into JSON object rows, failing closed on the first
/// non-blank line that is not a JSON object.
pub fn parse_log_rows(path: &Path, text: &str) -> Result<Vec<Value>, MutationError> {
    let mut rows = Vec::new();
    for (index, line) in crate::snapshot::universal_lines(text).enumerate() {
        let line_number = index + 1;
        let stripped = line.trim();
        if stripped.is_empty() {
            continue;
        }
        let value: Value =
            serde_json::from_str(stripped).map_err(|error| MutationError::MalformedStream {
                path: path.to_path_buf(),
                line: line_number,
                message: error.to_string(),
            })?;
        if crate::mutation_json::has_unrepresentable_float(&value) {
            return Err(MutationError::MalformedStream {
                path: path.to_path_buf(),
                line: line_number,
                message: "non-finite JSON floats are unsupported".to_owned(),
            });
        }
        if !value.is_object() {
            return Err(MutationError::MalformedStream {
                path: path.to_path_buf(),
                line: line_number,
                message: "expected a JSON object".to_owned(),
            });
        }
        rows.push(value);
    }
    Ok(rows)
}

/// Read and parse a whole log; a missing log is an empty stream.
pub fn read_log_rows(path: &Path) -> Result<Vec<Value>, MutationError> {
    let Some(text) = read_log_text(path)? else {
        return Ok(Vec::new());
    };
    parse_log_rows(path, &text)
}

/// Append rows as one durable, whole-file replacement, then verify by readback.
///
/// Each row is sanitized and encoded through
/// [`crate::mutation_json::serialize_event_line`] before it is written, exactly
/// like the baseline `append_raw_rows_atomic`. Nothing is written when `rows`
/// is empty, when the log path is a symlink, when the existing stream is
/// malformed, or when the resulting size would exceed
/// [`AppendOptions::max_bytes`].
pub fn append_rows_atomic(
    path: &Path,
    rows: &[Value],
    options: &AppendOptions,
) -> Result<AppendOutcome, MutationError> {
    append_rows_with_commit(path, rows, options, &mut |path, bytes, create_parents| {
        sk_kernel::atomic_write_with_options(
            path,
            bytes,
            sk_kernel::AtomicWriteOptions { create_parents },
        )
    })
}

/// The replacement seam used by [`append_rows_atomic`].
///
/// Production passes the kernel's atomic replacement. Tests inject a failing or
/// misbehaving commit to exercise the readback/indeterminate recovery paths
/// (the kernel's own durability primitive is tested the same way, with an
/// injected parent-sync failure).
///
/// `commit` receives `(path, intended_bytes, create_parents)` and returns the
/// kernel's "content changed" answer.
pub type CommitFn<'a> =
    &'a mut dyn FnMut(&Path, &[u8], bool) -> Result<bool, sk_kernel::KernelError>;

/// Append rows using an explicit replacement seam. See [`CommitFn`].
#[doc(hidden)]
pub fn append_rows_with_commit(
    path: &Path,
    rows: &[Value],
    options: &AppendOptions,
    commit: CommitFn<'_>,
) -> Result<AppendOutcome, MutationError> {
    if rows.is_empty() {
        return Ok(AppendOutcome {
            path: path.to_path_buf(),
            existed_before: path.exists(),
            previous_bytes: 0,
            written_bytes: 0,
            rows_appended: 0,
            durability: Durability::Confirmed,
        });
    }

    refuse_symlink(path)?;
    let existing = read_log_text(path)?;
    let existed_before = existing.is_some();
    let mut content = existing.unwrap_or_default();
    // Reject a corrupt stream *before* replacing it, so a refusal never leaves
    // the caller's rows on disk next to bytes no reader can parse.
    parse_log_rows(path, &content)?;
    let previous_bytes = content.len() as u64;
    if !content.is_empty() && !content.ends_with('\n') {
        // Baseline behaviour: insert the missing terminator, keep every byte.
        content.push('\n');
    }
    for row in rows {
        if !row.is_object() || crate::mutation_json::has_unrepresentable_float(row) {
            return Err(MutationError::InvalidRow {
                reason: "each appended row must be a JSON object with finite floating-point values"
                    .to_owned(),
            });
        }
        content.push_str(&serialize_event_line(row));
        content.push('\n');
    }
    let written_bytes = content.len() as u64;
    if written_bytes > options.max_bytes {
        return Err(MutationError::Kernel(sk_kernel::KernelError::TooLarge {
            path: path.to_path_buf(),
            limit: options.max_bytes,
        }));
    }

    let durability = match commit(path, content.as_bytes(), options.create_parents) {
        Ok(_) => Durability::Confirmed,
        Err(error) if error.mutation_may_have_committed() => {
            // The rename may have landed even though the parent sync failed.
            // Read the file back instead of assuming either outcome.
            match read_log_text(path) {
                Ok(Some(actual)) if actual == content => Durability::IndeterminateRecovered,
                Ok(Some(actual)) => {
                    return Err(MutationError::CommitIndeterminate {
                        path: path.to_path_buf(),
                        detail: format!(
                            "kernel reported indeterminate commit and the log on disk is {} bytes, not the intended {written_bytes}",
                            actual.len()
                        ),
                    });
                }
                Ok(None) => {
                    return Err(MutationError::CommitIndeterminate {
                        path: path.to_path_buf(),
                        detail: "kernel reported indeterminate commit and the log is now absent"
                            .to_owned(),
                    });
                }
                Err(read_error) => {
                    return Err(MutationError::CommitIndeterminate {
                        path: path.to_path_buf(),
                        detail: format!(
                            "kernel reported indeterminate commit and the readback failed: {read_error}"
                        ),
                    });
                }
            }
        }
        Err(error) => return Err(error.into()),
    };

    verify_readback(path, &content)?;

    Ok(AppendOutcome {
        path: path.to_path_buf(),
        existed_before,
        previous_bytes,
        written_bytes,
        rows_appended: rows.len(),
        durability,
    })
}

fn verify_readback(path: &Path, expected: &str) -> Result<(), MutationError> {
    let actual = read_log_text(path).map_err(|error| error.after_commit(path))?;
    let actual = actual.unwrap_or_default();
    if actual == expected {
        return Ok(());
    }
    Err(MutationError::ReadbackMismatch {
        path: path.to_path_buf(),
        expected_bytes: expected.len() as u64,
        actual_bytes: actual.len() as u64,
    })
}

/// Require every expected row to be readable back from `path`.
///
/// Matching mirrors the baseline `_event_matches_expected`: the `event_id` must
/// exist, and when the expected row carries `mission_slug`, `wp_id` or
/// `to_lane`, the persisted row must agree. This is the post-append proof the
/// emit path publishes, on top of the byte-level readback.
pub fn verify_event_rows(path: &Path, expected: &[Value]) -> Result<(), MutationError> {
    if expected.is_empty() {
        return Ok(());
    }
    let rows = read_log_rows(path)?;
    for want in expected {
        let event_id = want
            .get("event_id")
            .and_then(Value::as_str)
            .ok_or_else(|| MutationError::InvalidRow {
                reason: "an expected row has no string event_id".to_owned(),
            })?;
        let found = rows.iter().any(|row| row_matches(row, want));
        if !found {
            return Err(MutationError::EventMissing {
                path: path.to_path_buf(),
                event_id: event_id.to_owned(),
            });
        }
    }
    Ok(())
}

fn row_matches(row: &Value, want: &Value) -> bool {
    if row.get("event_id") != want.get("event_id") {
        return false;
    }
    for field in ["mission_slug", "wp_id", "to_lane"] {
        if let Some(expected) = want.get(field)
            && row.get(field) != Some(expected)
        {
            return false;
        }
    }
    match want.get("mission_id") {
        Some(expected) if !expected.is_null() => row.get("mission_id") == Some(expected),
        _ => true,
    }
}

/// Capture a log's exact bytes for a later explicit rollback.
pub fn capture_log_snapshot(path: &Path) -> Result<LogSnapshot, MutationError> {
    refuse_symlink(path)?;
    match sk_kernel::no_follow::read_no_follow(path, MAX_EVENT_LOG_BYTES) {
        Ok(bytes) => Ok(LogSnapshot {
            path: path.to_path_buf(),
            bytes: Some(bytes),
        }),
        Err(sk_kernel::KernelError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            Ok(LogSnapshot {
                path: path.to_path_buf(),
                bytes: None,
            })
        }
        Err(error) => Err(error.into()),
    }
}

/// Restore a captured log, but only while it still holds `expected_current`.
///
/// Guards the two failure modes a blind rollback creates: clobbering another
/// writer's rows, and deleting a file that is no longer the one this caller
/// wrote. Returns `true` when the previous state was restored, `false` when the
/// log already matched the snapshot (nothing to do).
pub fn restore_log_snapshot(
    snapshot: &LogSnapshot,
    expected_current: &[u8],
) -> Result<bool, MutationError> {
    refuse_symlink(&snapshot.path)?;
    let current = match sk_kernel::no_follow::read_no_follow(&snapshot.path, MAX_EVENT_LOG_BYTES) {
        Ok(bytes) => bytes,
        Err(sk_kernel::KernelError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            Vec::new()
        }
        Err(error) => return Err(error.into()),
    };
    if snapshot.bytes.as_deref() == Some(current.as_slice()) {
        return Ok(false);
    }
    if current != expected_current {
        return Err(MutationError::RollbackRefused {
            path: snapshot.path.clone(),
            reason: format!(
                "the log no longer holds the {} bytes this operation wrote (it holds {} bytes); another writer's content is preserved",
                expected_current.len(),
                current.len()
            ),
        });
    }
    match &snapshot.bytes {
        Some(bytes) => {
            sk_kernel::atomic_write(&snapshot.path, bytes)?;
        }
        None => match fs::remove_file(&snapshot.path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(MutationError::io("remove", snapshot.path.clone(), error));
            }
        },
    }
    Ok(true)
}

/// The result of materializing `status.json`.
#[derive(Clone, Debug)]
pub struct MaterializeOutcome {
    /// The snapshot file that was (or already was) current.
    pub path: PathBuf,
    /// Whether the bytes changed (the kernel write is no-op stable).
    pub changed: bool,
    /// The reduced snapshot.
    pub snapshot: Snapshot,
}

/// Materialize `status.json` from the log and write it atomically.
///
/// This is the baseline `reducer.materialize`: read the diary through the full
/// consumer replay ([`crate::snapshot::materialize`], which applies the CLI
/// projections and schema-generation rules), render it, and replace the file
/// only when the bytes differ. A mission with no log materializes an empty
/// snapshot at the current generation, exactly as the baseline does.
pub fn materialize_snapshot_file(feature_dir: &Path) -> Result<MaterializeOutcome, MutationError> {
    let input = ReplayInput::read_dir(feature_dir)?;
    let snapshot = materialize(&input)?;
    let text = render_snapshot_json(&snapshot)?;
    let path = feature_dir.join(SNAPSHOT_FILENAME);
    let changed = sk_kernel::atomic_write_with_options(
        &path,
        text.as_bytes(),
        sk_kernel::AtomicWriteOptions {
            create_parents: true,
        },
    )?;
    Ok(MaterializeOutcome {
        path,
        changed,
        snapshot,
    })
}

/// The canonical mission log path inside a feature directory.
pub fn events_path(feature_dir: &Path) -> PathBuf {
    feature_dir.join(EVENTS_FILENAME)
}

fn refuse_symlink(path: &Path) -> Result<(), MutationError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(MutationError::SymlinkRefused {
            path: path.to_path_buf(),
        }),
        _ => Ok(()),
    }
}
