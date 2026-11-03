//! The append-only writer for Op audit records.
//!
//! Source: `specify_cli/invocation/writer.py`. Storage layout:
//!
//! - `<record_root>/kitty-ops/<ULID>.jsonl` — one trail per Op, filename is
//!   the invocation id only (no profile prefix);
//! - `<record_root>/kitty-ops/ops-index.jsonl` — `{invocation_id, profile_id,
//!   started_at}` per successful start, a performance aid only;
//! - `<record_root>/kitty-ops/op-closures.jsonl` — the append-only closure
//!   spine for doctor-sweep closes.
//!
//! Invariants (all from the reference):
//!
//! - `write_started` is an exclusive create (`O_EXCL`, mode `0o600`) taken
//!   under the `kitty-ops` **family** event-log lock, so a concurrent append
//!   never observes an empty trail and a ULID collision is a typed write
//!   error rather than a retry.
//! - `write_completed` and the correlation links are locked appends through
//!   the kernel event-log protocol (`create = false`, no-follow, torn tail
//!   refused, one unit write, fsync + readback). Under the same lock they
//!   verify that the first row is a `started` event, that its embedded
//!   `invocation_id` equals the requested one, and — for `write_completed` —
//!   that no `completed` row exists yet.
//! - The index append is a warning only: a refused index never fails the Op.
//! - No existing line is ever mutated.
//!
//! Deliberate divergences, all documented in `IMPLEMENTATION-REPORT.md`:
//! the created trail is fsynced (Python closes a buffered handle) so a
//! returned start is durable; failure *detail* text from the kernel protocol
//! is rendered from the kernel's typed error through the pinned
//! interim-protocol wording; the Op precondition reads the whole owned trail
//! (no generic 8 MiB ceiling) because the reference reads the whole record and
//! this writer accepted it.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sk_kernel::event_log_append::{
    EVENT_LOG_LOCK_TIMEOUT, EventLogAppendOptions, TornTailPolicy, append_event_log_lines,
    append_event_log_lines_guarded, event_log_lock_path,
};
use sk_kernel::{FileLock, KernelError, clock, resolve_reference_path};

use crate::error::InvocationError;
use crate::error::RecordParseError;
use crate::modes::{RecordedMode, classify_mode_of_work};
use crate::python_json::{PythonJsonObject, python_repr, python_repr_json, python_seconds};
use crate::record::{InvocationId, OpCompletedEvent, OpEvent, OpStartedEvent, parse_op_event};

/// Directory holding every Op artifact.
pub const EVENTS_DIR: &str = "kitty-ops";
/// The lightweight invocation index.
pub const INDEX_PATH: &str = "kitty-ops/ops-index.jsonl";
/// The closure-spine filename under [`EVENTS_DIR`].
pub const OP_CLOSURES_FILENAME: &str = "op-closures.jsonl";
/// Permission bits of a newly created Op artifact (owner read/write only).
pub const RECORD_MODE: u32 = 0o600;

/// Path of the append-only Op-closure spine under `repo_root`.
pub fn op_closures_path(repo_root: &Path) -> PathBuf {
    repo_root.join(EVENTS_DIR).join(OP_CLOSURES_FILENAME)
}

/// The outcome of a successful `started` write.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartedWrite {
    /// The trail file that now holds the `started` line.
    pub path: PathBuf,
    /// A refused index append, if the index could not be updated.
    pub index_warning: Option<IndexWarning>,
}

/// A refused (non-fatal) index append.
///
/// The reference logs a WARNING; this crate has no logging dependency, so the
/// caller receives the message instead of it being printed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexWarning {
    /// The index path that was not updated.
    pub index_path: PathBuf,
    /// The rendered warning message.
    pub message: String,
}

/// A correlation event appended to an existing Op trail.
///
/// Exactly one of `reference` (artifact link) or `sha` (commit link) must be
/// present; both or neither is [`InvocationError::InvalidCorrelationLink`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CorrelationLink {
    /// The artifact kind (`artifact` by default in the reference).
    pub kind: String,
    /// The artifact reference for an `artifact_link`.
    pub reference: Option<String>,
    /// The commit sha for a `commit_link`.
    pub sha: Option<String>,
    /// The event timestamp; the current UTC ISO time when absent.
    pub at: Option<String>,
}

impl CorrelationLink {
    /// An `artifact_link` with the reference's default kind.
    pub fn artifact(reference: impl Into<String>) -> Self {
        Self {
            kind: "artifact".to_owned(),
            reference: Some(reference.into()),
            sha: None,
            at: None,
        }
    }

    /// An `artifact_link` with an explicit kind.
    pub fn artifact_kind(kind: impl Into<String>, reference: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            reference: Some(reference.into()),
            sha: None,
            at: None,
        }
    }

    /// A `commit_link`.
    pub fn commit(sha: impl Into<String>) -> Self {
        Self {
            kind: "artifact".to_owned(),
            reference: None,
            sha: Some(sha.into()),
            at: None,
        }
    }

    /// Pin the event timestamp instead of reading the wall clock.
    pub fn at(mut self, at: impl Into<String>) -> Self {
        self.at = Some(at.into());
        self
    }
}

/// Append-only writer for per-invocation audit trails.
#[derive(Clone, Debug)]
pub struct InvocationWriter {
    root: PathBuf,
    dir: PathBuf,
    lock_timeout: Duration,
}

impl InvocationWriter {
    /// A writer for the checkout whose `kitty-ops/` directory holds the records.
    pub fn new(repo_root: impl Into<PathBuf>) -> Self {
        let root = repo_root.into();
        let dir = root.join(EVENTS_DIR);
        Self {
            root,
            dir,
            lock_timeout: EVENT_LOG_LOCK_TIMEOUT,
        }
    }

    /// Override the bounded event-log lock wait (tests and bounded callers).
    pub fn with_lock_timeout(mut self, timeout: Duration) -> Self {
        self.lock_timeout = timeout;
        self
    }

    /// The record root this writer was built for.
    pub fn record_root(&self) -> &Path {
        &self.root
    }

    /// The `kitty-ops` directory this writer uses.
    pub fn events_dir(&self) -> &Path {
        &self.dir
    }

    /// The trail path for `invocation_id`.
    ///
    /// The id is validated by its type before any path is composed; the
    /// resolved path must stay inside `kitty-ops`, and a final-component
    /// symbolic link is refused (the reference's `ValueError` boundary).
    pub fn invocation_path(
        &self,
        invocation_id: &InvocationId,
    ) -> Result<PathBuf, InvocationError> {
        let file_name = format!("{invocation_id}.jsonl");
        let candidate = self.dir.join(&file_name);
        if self.dir.exists() {
            match sk_kernel::resolve_within(&self.dir, Path::new(&file_name)) {
                Ok(_) => {}
                Err(KernelError::PathEscape { resolved, .. }) => {
                    return Err(InvocationError::PathRefused(format!(
                        "Refusing to access path outside trusted roots: {}",
                        resolved.display()
                    )));
                }
                Err(_error) => {
                    return Err(InvocationError::PathRefused(format!(
                        "Refusing to access path outside trusted roots: {}",
                        candidate.display()
                    )));
                }
            }
        }
        if fs::symlink_metadata(&candidate)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Err(InvocationError::PathRefused(format!(
                "Invocation record must not be a symbolic link: {}",
                candidate.display()
            )));
        }
        Ok(candidate)
    }

    /// Write the `started` event as an exclusive create and index it.
    ///
    /// A pre-existing trail at the same path is a ULID collision, reported as
    /// [`InvocationError::Write`] with the reference's message.
    pub fn write_started(&self, record: &OpStartedEvent) -> Result<StartedWrite, InvocationError> {
        self.ensure_dir()?;
        let path = self.invocation_path(record.invocation_id())?;
        let line = record.to_jsonl_line();
        {
            let lock_path = event_log_lock_path(&self.dir).map_err(|error| {
                InvocationError::Write(format!(
                    "Failed to write invocation record: {}",
                    render_kernel_refusal(&self.dir, &error)
                ))
            })?;
            let _family_lock =
                FileLock::acquire(&lock_path, self.lock_timeout).map_err(|error| {
                    InvocationError::Write(format!(
                        "Failed to write invocation record: {}",
                        render_kernel_refusal(&self.dir, &error)
                    ))
                })?;
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(RECORD_MODE);
            }
            match options.open(&path) {
                Ok(mut file) => {
                    file.write_all(line.as_bytes())
                        .and_then(|()| file.write_all(b"\n"))
                        .and_then(|()| file.sync_all())
                        .map_err(|error| {
                            InvocationError::Write(format!(
                                "Failed to write invocation record: {error}"
                            ))
                        })?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    return Err(InvocationError::Write(format!(
                        "ULID collision on {} — retry with a new invocation_id",
                        path.display()
                    )));
                }
                Err(error) => {
                    return Err(InvocationError::Write(format!(
                        "Failed to write invocation record: {error}"
                    )));
                }
            }
        }
        let index_warning = self.append_to_index(record);
        Ok(StartedWrite {
            path,
            index_warning,
        })
    }

    /// Append the `completed` event to an existing trail.
    ///
    /// The started-identity and already-completed preconditions are checked
    /// under the family lock; a double close is
    /// [`InvocationError::AlreadyClosed`] and a missing trail is
    /// [`InvocationError::Record`].
    pub fn write_completed(&self, record: &OpCompletedEvent) -> Result<PathBuf, InvocationError> {
        let invocation_id = record.invocation_id();
        let path = self.invocation_path(invocation_id)?;
        let line = record.to_jsonl_line();
        match self.append_validated(&path, invocation_id, &line, true) {
            Ok(()) => Ok(path),
            Err(refusal) => Err(self.completed_error(invocation_id, refusal)),
        }
    }

    /// Append an `artifact_link` or `commit_link` event to a trail.
    ///
    /// A closed Op still accepts links (the reference's `refuse_if_completed`
    /// is false here); the started-identity precondition still applies.
    pub fn append_correlation_link(
        &self,
        invocation_id: &InvocationId,
        link: CorrelationLink,
    ) -> Result<(), InvocationError> {
        let (event, kind, reference, sha) = match (link.reference.as_deref(), link.sha.as_deref()) {
            (Some(reference), None) => (
                "artifact_link",
                Some(link.kind.as_str()),
                Some(reference),
                None,
            ),
            (None, Some(sha)) => ("commit_link", None, None, Some(sha)),
            _ => {
                return Err(InvocationError::InvalidCorrelationLink(
                    "Exactly one of ref or sha must be provided".to_owned(),
                ));
            }
        };
        let path = self.invocation_path(invocation_id)?;
        let at = match link.at {
            Some(at) => at,
            None => clock::now_utc_iso(),
        };
        let mut object = PythonJsonObject::new();
        object.string("event", event);
        object.string("invocation_id", invocation_id.as_str());
        object.string("at", &at);
        if let (Some(kind), Some(reference)) = (kind, reference) {
            object.string("kind", kind);
            object.string("ref", reference);
        } else if let Some(sha) = sha {
            object.string("sha", sha);
        }
        let line = object.finish();
        match self.append_validated(&path, invocation_id, &line, false) {
            Ok(()) => Ok(()),
            Err(refusal) => Err(self.correlation_error(invocation_id, refusal)),
        }
    }

    /// Read and validate the durable `started` event for `invocation_id` at
    /// **this writer's record root**.
    ///
    /// This is not the composed claim reader. The reference's
    /// `_read_op_started_event` first picks the checkout that holds the record
    /// (`locate_op_record_root(repo_root, id, preferred =
    /// resolve_op_record_root(repo_root)) or repo_root`) and only then reads;
    /// this method reads exactly `self.record_root()` and never searches other
    /// checkouts. The messages match the reference readers for a record that
    /// is at this root, and a record opened in a linked worktree is therefore
    /// reported `Op record not found` here. The stage-2 claim reader must
    /// compose `resolve_op_record_root`/`locate_op_record_root` explicitly;
    /// no implicit redirect is invented in this layer.
    ///
    /// Validation is the reference claim reader's: the embedded id must equal
    /// the requested id.
    pub fn read_started(
        &self,
        invocation_id: &InvocationId,
    ) -> Result<OpStartedEvent, InvocationError> {
        let path = self.invocation_path(invocation_id)?;
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(InvocationError::Record(format!(
                    "Op record not found for invocation_id={}",
                    python_repr(invocation_id.as_str())
                )));
            }
            Err(error) => {
                return Err(InvocationError::Record(format!(
                    "Could not read Op record for invocation_id={}: {error}",
                    python_repr(invocation_id.as_str())
                )));
            }
        };
        let Some(first_line) = text.lines().next() else {
            return Err(InvocationError::Record(format!(
                "Op record is empty for invocation_id={}",
                python_repr(invocation_id.as_str())
            )));
        };
        let value: serde_json::Value = serde_json::from_str(first_line).map_err(|error| {
            InvocationError::Record(format!(
                "Could not read Op record for invocation_id={}: {error}",
                python_repr(invocation_id.as_str())
            ))
        })?;
        let event = parse_op_event(&value).map_err(|error| {
            InvocationError::Record(format!(
                "Could not read Op record for invocation_id={}: {error}",
                python_repr(invocation_id.as_str())
            ))
        })?;
        match event {
            OpEvent::Started(started) => {
                if started.invocation_id() != invocation_id {
                    return Err(InvocationError::Record(format!(
                        "Op record invocation_id {} does not match requested {}",
                        python_repr(started.invocation_id().as_str()),
                        python_repr(invocation_id.as_str())
                    )));
                }
                Ok(started)
            }
            OpEvent::Completed(_) => Err(InvocationError::Record(format!(
                "First Op record is not a started event for invocation_id={}",
                python_repr(invocation_id.as_str())
            ))),
        }
    }

    /// Read the recorded mode of an Op at **this writer's record root**.
    ///
    /// Same placement contract as [`Self::read_started`]: the root is the one
    /// this writer was built for, not a located checkout. A record opened in a
    /// linked worktree is `Invocation record not found` here; the stage-2
    /// claim reader composes the locator.
    ///
    /// A first row that is not a JSON object is malformed, never absence:
    /// malformation must not permit evidence. An object without
    /// `mode_of_work` is [`RecordedMode::Absent`], and a present but unknown
    /// value is [`RecordedMode::Undetermined`].
    pub fn read_started_mode(
        &self,
        invocation_id: &InvocationId,
    ) -> Result<RecordedMode, InvocationError> {
        let path = self.invocation_path(invocation_id)?;
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(InvocationError::Record(format!(
                    "Invocation record not found: {invocation_id}"
                )));
            }
            Err(error) => {
                return Err(InvocationError::Record(format!(
                    "Invalid invocation record: {invocation_id} ({error})"
                )));
            }
        };
        let Some(first_line) = text.lines().next() else {
            return Err(InvocationError::Record(format!(
                "Invalid invocation record: {invocation_id}"
            )));
        };
        let value: serde_json::Value = serde_json::from_str(first_line).map_err(|_| {
            InvocationError::Record(format!("Invalid invocation record: {invocation_id}"))
        })?;
        let object = value.as_object().ok_or_else(|| {
            InvocationError::Record(format!("Invalid invocation record: {invocation_id}"))
        })?;
        Ok(classify_mode_of_work(object.get("mode_of_work")))
    }

    /// Invocation ids already closed on the Op-closure spine.
    pub fn closed_ids(&self) -> Result<BTreeSet<InvocationId>, InvocationError> {
        closed_invocation_ids(&self.root)
    }

    fn ensure_dir(&self) -> Result<(), InvocationError> {
        fs::create_dir_all(&self.dir).map_err(|error| {
            InvocationError::Write(format!("Failed to write invocation record: {error}"))
        })
    }

    fn append_to_index(&self, record: &OpStartedEvent) -> Option<IndexWarning> {
        let index_path = self.dir.join("ops-index.jsonl");
        let mut line = PythonJsonObject::new();
        line.string("invocation_id", record.invocation_id().as_str());
        line.string("profile_id", record.profile_id());
        line.string("started_at", record.started_at());
        let line = line.finish();
        let result = fs::create_dir_all(&self.dir)
            .map_err(|error| KernelError::Io {
                action: "create index parent",
                path: self.dir.clone(),
                source: error,
            })
            .and_then(|()| {
                let options = EventLogAppendOptions {
                    no_follow: true,
                    mode: RECORD_MODE,
                    ..EventLogAppendOptions::default()
                };
                append_event_log_lines(&index_path, &[line], &options)
            });
        result.err().map(|error| IndexWarning {
            message: format!(
                "Op index entry for {} not written to {}: {}",
                record.invocation_id(),
                index_path.display(),
                render_kernel_refusal(&index_path, &error)
            ),
            index_path,
        })
    }

    fn append_validated(
        &self,
        path: &Path,
        invocation_id: &InvocationId,
        line: &str,
        refuse_if_completed: bool,
    ) -> Result<(), AppendRefusal> {
        let options = EventLogAppendOptions {
            lock_anchor: Some(self.dir.clone()),
            create: false,
            no_follow: true,
            mode: RECORD_MODE,
            torn_tail: TornTailPolicy::Refuse,
            // Op start accepts the full request, and the reference reads the
            // entire owned trail before closing. A generic 8 MiB precondition
            // cap must not strand a record this writer successfully created.
            max_precondition_bytes: u64::MAX,
            timeout: self.lock_timeout,
            ..EventLogAppendOptions::default()
        };
        let mut refusal: Option<AppendRefusal> = None;
        let result =
            append_event_log_lines_guarded(
                path,
                &[line],
                &options,
                |existing| match check_existing(existing, invocation_id, refuse_if_completed) {
                    Ok(()) => Ok(()),
                    Err(reason) => {
                        refusal = Some(reason);
                        Err(KernelError::GuardedRead {
                            path: path.to_path_buf(),
                            reason: "Op trail precondition refused".to_owned(),
                        })
                    }
                },
            );
        match result {
            Ok(()) => Ok(()),
            Err(error) => match refusal {
                Some(refusal) => Err(refusal),
                None => Err(map_kernel_error(&self.dir, &error)),
            },
        }
    }

    fn completed_error(
        &self,
        invocation_id: &InvocationId,
        refusal: AppendRefusal,
    ) -> InvocationError {
        match refusal {
            AppendRefusal::AlreadyClosed => InvocationError::AlreadyClosed {
                invocation_id: invocation_id.to_string(),
            },
            AppendRefusal::Other { message } => {
                InvocationError::Write(format!("Failed to append completed event: {message}"))
            }
            other => self.record_error(invocation_id, other),
        }
    }

    fn correlation_error(
        &self,
        invocation_id: &InvocationId,
        refusal: AppendRefusal,
    ) -> InvocationError {
        match refusal {
            AppendRefusal::AlreadyClosed => InvocationError::AlreadyClosed {
                invocation_id: invocation_id.to_string(),
            },
            AppendRefusal::Other { message } => {
                InvocationError::Write(format!("Failed to append correlation event: {message}"))
            }
            other => self.record_error(invocation_id, other),
        }
    }

    fn record_error(
        &self,
        invocation_id: &InvocationId,
        refusal: AppendRefusal,
    ) -> InvocationError {
        match refusal {
            AppendRefusal::NotFound => {
                InvocationError::Record(format!("Invocation record not found: {invocation_id}"))
            }
            AppendRefusal::Symlink { message } => InvocationError::Write(format!(
                "Failed to open invocation record safely: {message}"
            )),
            AppendRefusal::TornTail { message } => InvocationError::Record(format!(
                "Invocation record is unreadable: {invocation_id} ({message})"
            )),
            AppendRefusal::Unreadable => {
                InvocationError::Record(format!("Invocation record is unreadable: {invocation_id}"))
            }
            AppendRefusal::NoStarted => InvocationError::Record(format!(
                "Invocation record has no started event: {invocation_id}"
            )),
            AppendRefusal::IdentityMismatch { embedded } => InvocationError::Record(format!(
                "Invocation record identity mismatch: requested={}, embedded={}",
                python_repr(invocation_id.as_str()),
                embedded
                    .as_ref()
                    .map(python_repr_json)
                    .unwrap_or_else(|| "None".to_owned())
            )),
            AppendRefusal::AlreadyClosed => InvocationError::AlreadyClosed {
                invocation_id: invocation_id.to_string(),
            },
            AppendRefusal::Other { message } => {
                InvocationError::Write(format!("Failed to append completed event: {message}"))
            }
        }
    }
}

/// Why a validated append refused to write anything.
#[derive(Clone, Debug)]
enum AppendRefusal {
    NotFound,
    Symlink { message: String },
    TornTail { message: String },
    Unreadable,
    NoStarted,
    IdentityMismatch { embedded: Option<serde_json::Value> },
    AlreadyClosed,
    Other { message: String },
}

fn check_existing(
    existing: &[u8],
    invocation_id: &InvocationId,
    refuse_if_completed: bool,
) -> Result<(), AppendRefusal> {
    let text = std::str::from_utf8(existing).map_err(|_| AppendRefusal::Unreadable)?;
    let mut rows: Vec<serde_json::Value> = Vec::new();
    for row in text.lines() {
        if row.trim().is_empty() {
            continue;
        }
        let parsed: serde_json::Value =
            serde_json::from_str(row).map_err(|_| AppendRefusal::Unreadable)?;
        rows.push(parsed);
    }
    let Some(first) = rows.first() else {
        return Err(AppendRefusal::NoStarted);
    };
    let first_object = first.as_object().ok_or(AppendRefusal::Unreadable)?;
    if first_object
        .get("event")
        .and_then(serde_json::Value::as_str)
        != Some("started")
    {
        return Err(AppendRefusal::NoStarted);
    }
    let embedded = first_object.get("invocation_id");
    if embedded.and_then(serde_json::Value::as_str) != Some(invocation_id.as_str()) {
        return Err(AppendRefusal::IdentityMismatch {
            embedded: embedded.cloned(),
        });
    }
    if refuse_if_completed
        && rows
            .iter()
            .filter_map(serde_json::Value::as_object)
            .any(|row| row.get("event").and_then(serde_json::Value::as_str) == Some("completed"))
    {
        return Err(AppendRefusal::AlreadyClosed);
    }
    Ok(())
}

fn map_kernel_error(anchor: &Path, error: &KernelError) -> AppendRefusal {
    match error {
        KernelError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound => {
            AppendRefusal::NotFound
        }
        KernelError::NoFollow { path } => AppendRefusal::Symlink {
            message: symlink_protocol_message(path),
        },
        KernelError::TornTail { path } => AppendRefusal::TornTail {
            message: torn_tail_protocol_message(path),
        },
        other => AppendRefusal::Other {
            message: render_kernel_refusal(anchor, other),
        },
    }
}

fn render_kernel_refusal(anchor: &Path, error: &KernelError) -> String {
    match error {
        KernelError::NoFollow { path } => symlink_protocol_message(path),
        KernelError::TornTail { path } => torn_tail_protocol_message(path),
        KernelError::LockTimeout { path, timeout } => {
            lock_timeout_protocol_message(anchor, path, *timeout)
        }
        KernelError::LockUnavailable { path, reason } => {
            lock_unavailable_protocol_message(path, reason)
        }
        other => other.to_string(),
    }
}

fn symlink_protocol_message(path: &Path) -> String {
    format!(
        "{}: refusing to append through a symbolic link. Error code: EVENT_LOG_SYMLINK.",
        path.display()
    )
}

fn torn_tail_protocol_message(path: &Path) -> String {
    format!(
        "{}: the last line is torn (no trailing newline), so the append was refused. Inspect the file and cut the bytes after its last newline, then retry. Error code: EVENT_LOG_TORN_TAIL.",
        path.display()
    )
}

fn lock_timeout_protocol_message(anchor: &Path, lock_path: &Path, timeout: Duration) -> String {
    format!(
        "{}: event-log lock {} was not acquired within {}s because another writer holds it. Retry, or stop the other writer. Error code: EVENT_LOG_LOCK_TIMEOUT.",
        anchor.display(),
        lock_path.display(),
        python_seconds(timeout)
    )
}

fn lock_unavailable_protocol_message(anchor: &Path, reason: &str) -> String {
    format!(
        "{}: cannot place the event-log lock because git could not be invoked ({reason}). Make git runnable in this checkout, then retry. Error code: EVENT_LOG_LOCK_UNAVAILABLE.",
        anchor.display()
    )
}

/// Append one closure record to the Op-closure spine; return the path written.
///
/// The spine is the sanctioned durable surface for doctor-sweep closes: the
/// sweep's targets are byte-frozen archive records, so its closure is recorded
/// here as a new append-only line instead of in the per-record file.
pub fn append_op_closure(
    repo_root: &Path,
    record: &OpCompletedEvent,
) -> Result<PathBuf, InvocationError> {
    let path = op_closures_path(repo_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            InvocationError::Write(format!("Failed to write Op closure: {error}"))
        })?;
    }
    let options = EventLogAppendOptions {
        no_follow: true,
        mode: RECORD_MODE,
        ..EventLogAppendOptions::default()
    };
    append_event_log_lines(&path, &[record.to_jsonl_line()], &options).map_err(|error| {
        InvocationError::Write(format!(
            "Failed to write Op closure: {}",
            render_kernel_refusal(&path, &error)
        ))
    })?;
    Ok(path)
}

/// Read the closure spine in insertion order; `[]` when it is absent.
///
/// Malformed lines and non-completed lines are skipped rather than raising —
/// a corrupt line is a doctor signal, the same tolerance
/// `lifecycle.read_lifecycle_records` carries.
pub fn read_op_closures(repo_root: &Path) -> Result<Vec<OpCompletedEvent>, InvocationError> {
    let path = op_closures_path(repo_root);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(InvocationError::Record(format!(
                "Op closure spine is unreadable: {} ({error})",
                path.display()
            )));
        }
    };
    let mut closures = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match parse_op_event(&value) {
            Ok(OpEvent::Completed(completed)) => closures.push(completed),
            Ok(OpEvent::Started(_))
            | Err(RecordParseError::Legacy(_))
            | Err(RecordParseError::NotLifecycle(_)) => {}
        }
    }
    Ok(closures)
}

/// Invocation ids already closed on the Op-closure spine.
pub fn closed_invocation_ids(repo_root: &Path) -> Result<BTreeSet<InvocationId>, InvocationError> {
    Ok(read_op_closures(repo_root)?
        .into_iter()
        .map(|closure| closure.invocation_id().clone())
        .collect())
}

/// Normalise a correlation reference.
///
/// A ref carrying a URL scheme (`https://…`, `file://…`) is a link, not a
/// filesystem path, and is returned verbatim. Otherwise the ref is resolved
/// (following symlinks, non-strict) and returned repo-relative when it is
/// under `repo_root`, else as the resolved absolute path. An unresolvable ref
/// (symlink loop, interior NUL, an unreadable current directory) is returned
/// verbatim — the reference's `except (OSError, ValueError)`.
///
/// Resolution uses the kernel's reference-path authority
/// (`sk_kernel::resolve_reference_path`), the profile that tolerates missing,
/// dangling and unreadable components and has no total-link cap. It is not the
/// containment/lock resolver, whose fail-closed policy is unchanged.
pub fn normalise_ref(reference: &str, repo_root: &Path) -> String {
    if is_url_scheme(reference) {
        return reference.to_owned();
    }
    let candidate = if Path::new(reference).is_absolute() {
        PathBuf::from(reference)
    } else {
        repo_root.join(reference)
    };
    let Ok(resolved) = resolve_reference_path(&candidate) else {
        return reference.to_owned();
    };
    let root = resolve_reference_path(repo_root).unwrap_or_else(|_| repo_root.to_path_buf());
    match resolved.strip_prefix(&root) {
        Ok(relative) if relative.as_os_str().is_empty() => ".".to_owned(),
        Ok(relative) => relative.to_string_lossy().into_owned(),
        Err(_) => resolved.to_string_lossy().into_owned(),
    }
}

/// `^[A-Za-z][A-Za-z0-9+.-]+://` — two or more scheme characters so a Windows
/// drive letter (`C:/x`) is never mistaken for a URL scheme.
fn is_url_scheme(value: &str) -> bool {
    let bytes = value.as_bytes();
    let Some(first) = bytes.first() else {
        return false;
    };
    if !first.is_ascii_alphabetic() {
        return false;
    }
    let mut index = 1;
    while index < bytes.len()
        && (bytes[index].is_ascii_alphanumeric() || matches!(bytes[index], b'+' | b'.' | b'-'))
    {
        index += 1;
    }
    index >= 2 && value[index..].starts_with("://")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_scheme_detection_matches_the_reference_regex() {
        for url in [
            "https://example.com/x",
            "file:///tmp/x",
            "s3://bucket/key",
            "a1+.-://x",
        ] {
            assert!(is_url_scheme(url), "{url}");
        }
        for path in [
            "C:/x",
            "C:\\x",
            "docs/x.md",
            "./x",
            "://x",
            "a://",
            "evidence/x",
        ] {
            assert!(!is_url_scheme(path), "{path}");
        }
    }

    #[test]
    fn closure_spine_path_is_under_kitty_ops() {
        assert_eq!(
            op_closures_path(Path::new("/repo")),
            Path::new("/repo/kitty-ops/op-closures.jsonl")
        );
    }

    #[test]
    fn correlation_link_constructors_encode_the_event_kind() {
        assert_eq!(CorrelationLink::artifact("spec.md").kind, "artifact");
        assert_eq!(
            CorrelationLink::artifact_kind("mission", "kitty-specs/x").kind,
            "mission"
        );
        assert!(CorrelationLink::commit("abc123").reference.is_none());
    }

    // The two wrappers below are only reachable through a kernel refusal that
    // a deterministic filesystem fixture cannot force (a TOCTOU symlink, or an
    // append I/O failure), so the rendered message is asserted at the seam
    // that owns it. The kernel's own refusal text is asserted elsewhere.
    #[test]
    fn a_symbolic_link_refusal_renders_the_safe_open_wrapper() {
        let writer = InvocationWriter::new("/repo");
        let id = InvocationId::parse("01ARZ3NDEKTSV4RRFFQ69G5FAW").unwrap();
        let refusal = map_kernel_error(
            writer.events_dir(),
            &KernelError::NoFollow {
                path: PathBuf::from("/repo/kitty-ops/record.jsonl"),
            },
        );
        assert_eq!(
            writer.record_error(&id, refusal).to_string(),
            "Failed to open invocation record safely: /repo/kitty-ops/record.jsonl: refusing to \
             append through a symbolic link. Error code: EVENT_LOG_SYMLINK."
        );
    }

    #[test]
    fn a_kernel_append_failure_renders_the_correlation_wrapper() {
        let writer = InvocationWriter::new("/repo");
        let id = InvocationId::parse("01ARZ3NDEKTSV4RRFFQ69G5FAW").unwrap();
        let refusal = map_kernel_error(
            writer.events_dir(),
            &KernelError::Io {
                action: "open append target",
                path: PathBuf::from("/repo/kitty-ops/record.jsonl"),
                source: std::io::Error::from_raw_os_error(13),
            },
        );
        let rendered = writer.correlation_error(&id, refusal).to_string();
        assert!(
            rendered.starts_with("Failed to append correlation event: "),
            "{rendered}"
        );
        assert!(
            rendered.contains("failed to open append target /repo/kitty-ops/record.jsonl"),
            "{rendered}"
        );
    }
}
