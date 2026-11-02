//! The baseline `run.events.jsonl` journal: locked single-unit appends with
//! torn-tail refusal, and a deterministic parser/replay input.
//!
//! The cursor lock (`state.json.lock`) is the sole writer authority for this
//! journal: every mutation holds it across the event append and the cursor
//! write, matching the upstream engine. No second event-writer lock is
//! introduced, and no event is written outside a run-store transaction.

use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sk_kernel::event_log_append::{EventLogAppendOptions, TornTailPolicy, append_event_log_lines};

use crate::error::RunStoreError;
use crate::model::{MAX_EVENT_LOG_BYTES, now_iso};

pub const MISSION_RUN_STARTED: &str = "MissionRunStarted";
pub const NEXT_STEP_ISSUED: &str = "NextStepIssued";
pub const NEXT_STEP_AUTO_COMPLETED: &str = "NextStepAutoCompleted";
pub const DECISION_INPUT_REQUESTED: &str = "DecisionInputRequested";
pub const DECISION_INPUT_ANSWERED: &str = "DecisionInputAnswered";
pub const MISSION_RUN_COMPLETED: &str = "MissionRunCompleted";
pub const SIGNIFICANCE_EVALUATED: &str = "SignificanceEvaluated";
pub const DECISION_AUTHORITY_DENIED: &str = "DecisionAuthorityDenied";

/// One line of the upstream journal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventRecord {
    pub event_type: String,
    pub timestamp: String,
    pub payload: Value,
}

impl EventRecord {
    pub fn new(event_type: impl Into<String>, payload: Value) -> Self {
        Self {
            event_type: event_type.into(),
            timestamp: now_iso(),
            payload,
        }
    }

    /// The unique event wire line, with recursively sorted keys (upstream
    /// `json.dumps(..., sort_keys=True)`).
    pub fn line(&self) -> Result<String, RunStoreError> {
        // `serde_json::Value`'s map is a BTreeMap by default, so serializing
        // through it recursively sorts keys like upstream `sort_keys=True`.
        let value = serde_json::to_value(self).map_err(|error| RunStoreError::TemplateInvalid {
            message: format!("event record is not serializable: {error}"),
        })?;
        serde_json::to_string(&value).map_err(|error| RunStoreError::TemplateInvalid {
            message: format!("event record is not serializable: {error}"),
        })
    }
}

/// The complete journal plus the byte length of any torn final line.
#[derive(Debug, Clone, PartialEq)]
pub struct Journal {
    pub events: Vec<EventRecord>,
    pub torn_tail_bytes: u64,
}

/// Read and validate the complete rows of a journal.
///
/// A final row without LF is *reported*, never parsed and never silently
/// accepted: the caller decides whether to refuse (query/advance) or truncate
/// it (explicit recovery).
pub fn read_journal(path: &Path) -> Result<Journal, RunStoreError> {
    let bytes = match sk_kernel::read_bounded(path, MAX_EVENT_LOG_BYTES) {
        Ok(bytes) => bytes,
        Err(sk_kernel::KernelError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            return Ok(Journal {
                events: Vec::new(),
                torn_tail_bytes: 0,
            });
        }
        Err(error) => return Err(error.into()),
    };

    let (complete, torn_tail_bytes) = match bytes.last() {
        None => (&bytes[..], 0),
        Some(b'\n') => (&bytes[..], 0),
        Some(_) => {
            let last_lf = bytes.iter().rposition(|byte| *byte == b'\n');
            let split = last_lf.map_or(0, |index| index + 1);
            (&bytes[..split], (bytes.len() - split) as u64)
        }
    };

    let mut events = Vec::new();
    for (index, line) in complete.split(|byte| *byte == b'\n').enumerate() {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        let text = std::str::from_utf8(line).map_err(|error| RunStoreError::EventLogCorrupt {
            path: path.to_path_buf(),
            line: index + 1,
            message: format!("not valid UTF-8: {error}"),
        })?;
        let record: EventRecord =
            serde_json::from_str(text).map_err(|error| RunStoreError::EventLogCorrupt {
                path: path.to_path_buf(),
                line: index + 1,
                message: error.to_string(),
            })?;
        if record.event_type.trim().is_empty() {
            return Err(RunStoreError::EventLogCorrupt {
                path: path.to_path_buf(),
                line: index + 1,
                message: "event_type must be non-empty".to_string(),
            });
        }
        if !record.payload.is_object() {
            return Err(RunStoreError::EventLogCorrupt {
                path: path.to_path_buf(),
                line: index + 1,
                message: "payload must be a JSON object".to_string(),
            });
        }
        events.push(record);
    }

    Ok(Journal {
        events,
        torn_tail_bytes,
    })
}

/// Append records as one locked unit. The caller must hold the cursor lock.
///
/// Appends route through the kernel's single locked event-log primitive, so the
/// run journal shares the pinned upstream append protocol (event-log lock,
/// torn-tail refusal, one unit write, fsync, readback and rollback) rather than
/// inventing a second writer. A torn journal is pre-checked here so the caller
/// gets the typed byte count as well as the kernel's own refusal.
pub fn append_records(path: &Path, records: &[EventRecord]) -> Result<u64, RunStoreError> {
    if records.is_empty() {
        return Ok(0);
    }
    let torn_bytes = trailing_torn_bytes(path)?;
    if torn_bytes > 0 {
        return Err(RunStoreError::TornEventTail {
            path: path.to_path_buf(),
            torn_bytes,
        });
    }

    let mut lines = Vec::with_capacity(records.len());
    for record in records {
        lines.push(record.line()?);
    }
    let bytes = lines.iter().map(|line| line.len() + 1).sum::<usize>() as u64;

    append_event_log_lines(
        path,
        &lines,
        &EventLogAppendOptions {
            torn_tail: TornTailPolicy::Refuse,
            ..EventLogAppendOptions::default()
        },
    )
    .map_err(|error| match error {
        sk_kernel::KernelError::TornTail { path } => {
            let torn_bytes = trailing_torn_bytes(&path).unwrap_or(0);
            RunStoreError::TornEventTail { path, torn_bytes }
        }
        other => RunStoreError::Kernel(other),
    })?;
    Ok(bytes)
}

/// Count bytes after the last LF, reading the whole log only when the final
/// byte is actually torn (the healthy append path reads one byte).
fn trailing_torn_bytes(path: &Path) -> Result<u64, RunStoreError> {
    let mut file = match OpenOptions::new().read(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(source) => {
            return Err(RunStoreError::Kernel(sk_kernel::KernelError::Io {
                action: "open event journal tail",
                path: path.to_path_buf(),
                source,
            }));
        }
    };
    let size = file
        .metadata()
        .map_err(|source| {
            RunStoreError::Kernel(sk_kernel::KernelError::Io {
                action: "size event journal",
                path: path.to_path_buf(),
                source,
            })
        })?
        .len();
    if size == 0 {
        return Ok(0);
    }
    let mut last = [0u8; 1];
    file.seek(SeekFrom::Start(size - 1))
        .and_then(|_| file.read_exact(&mut last))
        .map_err(|source| {
            RunStoreError::Kernel(sk_kernel::KernelError::Io {
                action: "read event journal tail",
                path: path.to_path_buf(),
                source,
            })
        })?;
    if last[0] == b'\n' {
        return Ok(0);
    }
    let bytes = sk_kernel::read_bounded(path, MAX_EVENT_LOG_BYTES)?;
    Ok(match bytes.iter().rposition(|byte| *byte == b'\n') {
        Some(index) => (bytes.len() - index - 1) as u64,
        None => bytes.len() as u64,
    })
}

/// Truncate the bytes after the final LF. Used only by explicit recovery,
/// which holds the cursor lock. Returns the number of bytes removed.
pub fn truncate_torn_tail(path: &Path) -> Result<u64, RunStoreError> {
    let bytes = match sk_kernel::read_bounded(path, MAX_EVENT_LOG_BYTES) {
        Ok(bytes) => bytes,
        Err(sk_kernel::KernelError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            return Ok(0);
        }
        Err(error) => return Err(error.into()),
    };
    let Some(last) = bytes.last() else {
        return Ok(0);
    };
    if *last == b'\n' {
        return Ok(0);
    }
    let keep = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index + 1);
    let removed = (bytes.len() - keep) as u64;
    let file = OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|source| {
            RunStoreError::Kernel(sk_kernel::KernelError::Io {
                action: "open event journal for recovery",
                path: path.to_path_buf(),
                source,
            })
        })?;
    file.set_len(keep as u64)
        .and_then(|_| file.sync_all())
        .map_err(|source| {
            RunStoreError::Kernel(sk_kernel::KernelError::Io {
                action: "truncate torn event tail",
                path: path.to_path_buf(),
                source,
            })
        })?;
    Ok(removed)
}
