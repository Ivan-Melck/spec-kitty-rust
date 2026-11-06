//! Source-compatible writer operations: one mechanism, explicit policies.
//!
//! The pinned Python reference has four *different* physical write conventions,
//! and which one a file gets is decided by **which function wrote it**, never by
//! the path it happens to have:
//!
//! | operation | Source function | temp artifact | creation mode | equal bytes | fsync |
//! | --- | --- | --- | --- | --- | --- |
//! | [`WriterOperation::Atomic`] | `kernel.atomic.atomic_write` | `.atomic-…​.tmp` (mkstemp) | `0600` | publishes | no |
//! | [`WriterOperation::Generated`] | `runtime.generated_writer.write_generated_file` | `.<name>.tmp<pid>` | `0666 & ~umask` | publishes | no |
//! | [`WriterOperation::TextTruncate`] | `Path.write_text` | none | `0666 & ~umask` | rewrites in place | no |
//! | [`WriterOperation::DerivedJson`] | `status.views._atomic_write_json` | `<stem>.json.tmp` | `0666 & ~umask` | publishes | no |
//! | [`WriterOperation::RebuildLog`] | `migration.rebuild_state.rebuild_event_log` | `<name>.tmp` | `0666 & ~umask` | publishes | no |
//!
//! This module owns those policies as *explicit* values — a caller states the
//! operation it is performing; nothing here infers a policy from a file name or
//! from a mode. All four share one temporary/publication/cleanup authority
//! temporary/publication/cleanup authority (one private `TemporaryArtifact` type
//! over a held descriptor) and one strict encoder
//! ([`crate::python_utf8::encode_python_string`] /
//! [`crate::python_utf8::encode_utf8_scalars`]).
//!
//! The existing Native writers keep their own, deliberately different contracts
//! and are untouched: [`crate::atomic_write`] is no-op stable, preserves an
//! existing mode, flushes/`sync_all`s and synced-parent (durable) and refuses
//! non-regular destinations through the bounded reader;
//! [`crate::atomic_write_with_permissions`] is a managed-mode assertion;
//! [`crate::atomic_write_with_requested_mode`] is the durable-YAML requested-mode
//! policy (`kernel.yaml_io.write_mapping_atomic`). Those are capabilities, not
//! substitutes: none of them may stand in for a Source operation above.
//!
//! ## What is reproduced (observed, not inferred)
//!
//! Each statement below was produced by executing the pinned reference; the
//! receipts are the Source probe rows referenced in the W1 report.
//!
//! * **Atomic** — `str` content is encoded *before* the read-only relaxation and
//!   the temporary file exists, so an encode failure leaves the destination
//!   byte- and mode-identical. A destination is *always* replaced: an existing
//!   writable mode is **not** preserved (`0664` becomes `0600`), and equal bytes
//!   still publish. Only a *no-follow observed* read-only destination is relaxed
//!   (owner-write bit) and its original `S_IMODE` restored best-effort after the
//!   replace; a failed replace also restores it. A destination symlink node is
//!   replaced (its target is not written through), and a FIFO is replaced —
//!   the reference does not refuse those, so neither does this operation. There is
//!   no `fsync`. The installed write observer is notified *after* publication
//!   (and after the mode restore).
//! * **Generated** — an existing destination symlink is refused before anything
//!   is touched, and a pre-existing `.<name>.tmp<pid>` artifact (file *or*
//!   symlink) is refused as unproven, again before any permission change. The
//!   temporary is created at that exact sibling name with `0666 & ~umask`, the
//!   destination's write bits are relaxed with `0o222` only when it exists with
//!   no write bit, and the read-only strip (`& !0o222`) runs in a `finally` so it
//!   happens even when the write failed (including an encode failure, and
//!   including when the destination is a directory). There is no observer
//!   notification and no `fsync`.
//! * **TextTruncate** — the destination is opened (created/truncated) *first* and
//!   then the content is encoded, so an encode failure leaves a zero-byte file
//!   exactly where the reference leaves one. Symlinks are followed, an existing
//!   inode and mode are kept, a new file is `0666 & ~umask`, and the content is
//!   rewritten in place (no temp, no rename).
//! * **DerivedJson** — the temporary is the fixed `<stem>.json.tmp` sibling
//!   (truncated and reused if it already exists), then renamed over the
//!   destination and finally reported to the write observer. A failure leaves the
//!   temporary behind: `_atomic_write_json` has no cleanup path.
//! * **RebuildLog** — `rebuild_state.rebuild_event_log` uses the fixed
//!   `<name>.tmp` sibling opened with `open("w")` (so `0666 & ~umask`, truncating
//!   an existing artifact), renames it into place and removes the temporary in a
//!   `finally`, so *any* failure (including one raised while the content is
//!   serialized) leaves no artifact and an untouched destination. It never
//!   notifies the write observer. It is a separate operation from
//!   [`WriterOperation::DerivedJson`] even though both publish through a fixed
//!   sibling: the cleanup and notification behaviour differ.
//!
//! ## Deliberate limits (not waivers)
//!
//! No operation claims to eliminate external races (a concurrent writer can move
//! the destination between the observation and the rename), to hold an
//! inter-process lock, or to roll anything back. `Windows` read-only replacement
//! needs the same relaxation the reference performs; the directory-fsync,
//! owner/ACL/xattr/hard-link and filesystem-error-text equivalences of the
//! existing Native writers are unchanged and not extended here. The reference's
//! ambient `kernel.atomic.observe_writes` context is reproduced by
//! [`write_observer`]: the notifying operations dispatch through the canonical
//! ambient context *after* publication, and a call that also passes an explicit
//! [`WriteListener`] hears the ambient observer first and the per-call listener
//! second. The receipt records what was actually (not) notified. Those are one
//! ambient authority plus one explicit per-call capability, not two registries.
//!
//! ## Two further Source publication shapes this mechanism does NOT carry
//!
//! Reading the call graph for the in-scope call sites surfaced two more physical
//! writers. Both belong to the canonical status-log authority
//! (`sk_status::mutation_log::append_rows_atomic`), not to the call sites here, so
//! they are reported rather than re-implemented — replacing that authority's
//! durable Native contract from a caller would be a regression, not fidelity:
//!
//! * `specify_cli.migration.rebuild_state.rebuild_event_log` writes
//!   `status.events.jsonl` through the **fixed** sibling `status.events.jsonl.tmp`
//!   opened with `open("w")` (`0666 & ~umask`), then `os.replace`, unlinking the
//!   temporary on failure, with no observer notification and no `fsync`.
//! * `specify_cli.status.store.append_raw_rows_atomic` writes through
//!   `tempfile.mkstemp(prefix=".<name>.", suffix=".tmp")` (`0600`), `fsync`s the
//!   file, replaces, notifies the observer and `fsync`s the directory.
//!
//! The Native authority uses the durable kernel replacement (no-op stable, `fsync`,
//! parent sync, readback, rollback). Its mode and failure footprint therefore still
//! differ from the reference on that one file; see the W1 report's OPEN items.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::PythonString;
use crate::python_utf8::{PythonUtf8EncodeError, encode_python_string, encode_utf8_scalars};

pub mod write_observer;

pub use write_observer::{
    BoundWriteContext, ObserverCallback, WriteContext, WriteContextError, WriteContextGuard,
    WriteObserverError, WriteObserverToken, bind_write_context, notify_written, observe_writes,
    observe_writes_callback, observing_writes, stop_observing_writes,
};

/// The permission bits `runtime.generated_writer` treats as "writable".
const WRITE_BITS: u32 = 0o222;

/// The creation mode Source's `Path.write_text`/`open(..., "w")` asks for; the
/// process umask is applied by the OS, never read or changed here.
const TEXT_CREATE_MODE: u32 = 0o666;

/// Which Source writer operation a call is performing.
///
/// Explicit by construction: there is no `from_path`, no extension sniffing and
/// no default that silently picks a family.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriterOperation {
    /// `kernel.atomic.atomic_write` (read-only-aware replace, no `fsync`).
    Atomic(AtomicWrite),
    /// `runtime.generated_writer.write_generated_file` (PID sibling + read-only
    /// lifecycle).
    Generated(GeneratedWrite),
    /// `Path.write_text` (open, truncate, write in place).
    TextTruncate,
    /// `status.views._atomic_write_json` (fixed `.json.tmp` sibling + replace).
    DerivedJson,
    /// `migration.rebuild_state.rebuild_event_log` (fixed `.tmp` sibling +
    /// replace, temporary always removed on failure, no notification).
    RebuildLog,
}

impl WriterOperation {
    /// `atomic_write(..., mkdir=False)`.
    pub const fn atomic() -> Self {
        Self::Atomic(AtomicWrite {
            create_parents: false,
        })
    }

    /// `atomic_write(..., mkdir=True)`.
    pub const fn atomic_creating_parents() -> Self {
        Self::Atomic(AtomicWrite {
            create_parents: true,
        })
    }

    /// `write_generated_file(..., read_only=read_only)`.
    pub const fn generated(read_only: bool) -> Self {
        Self::Generated(GeneratedWrite { read_only })
    }

    /// Whether this operation publishes through a temporary file.
    pub const fn uses_temporary(self) -> bool {
        !matches!(self, Self::TextTruncate)
    }

    /// Whether Source notifies the installed write observer for this operation.
    pub const fn notifies_observer(self) -> bool {
        matches!(self, Self::Atomic(_) | Self::DerivedJson)
    }

    /// The Source function this operation reproduces (for carriers and receipts).
    pub const fn source_function(self) -> &'static str {
        match self {
            Self::Atomic(_) => "kernel.atomic.atomic_write",
            Self::Generated(_) => "specify_cli.runtime.generated_writer.write_generated_file",
            Self::TextTruncate => "pathlib.Path.write_text",
            Self::DerivedJson => "specify_cli.status.views._atomic_write_json",
            Self::RebuildLog => "specify_cli.migration.rebuild_state.rebuild_event_log",
        }
    }
}

/// Options of [`WriterOperation::Atomic`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AtomicWrite {
    /// Source `mkdir=True`: create missing parent directories first.
    pub create_parents: bool,
}

/// Options of [`WriterOperation::Generated`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GeneratedWrite {
    /// Leave the file read-only (`mode & !0o222`) after the write, in a
    /// `finally`-equivalent position.
    pub read_only: bool,
}

/// The content a Source writer accepts.
///
/// `str` and `bytes` are distinct Source branches, and a Python `str` may hold
/// code units (lone surrogates) that Rust text cannot — those must reach the
/// encoder as code units instead of being refused early or replaced.
#[derive(Clone, Copy, Debug)]
pub enum SourcePayload<'a> {
    /// Exact retained bytes (`bytes` content).
    Bytes(&'a [u8]),
    /// Rust text, encoded by the shared strict encoder.
    Text(&'a str),
    /// Exact CPython code units, encoded by the shared strict encoder.
    Python(&'a PythonString),
}

impl SourcePayload<'_> {
    /// The bytes the writer will publish, encoded by the one shared authority.
    fn encoded(&self) -> Result<Vec<u8>, PythonUtf8EncodeError> {
        match self {
            Self::Bytes(bytes) => Ok(bytes.to_vec()),
            Self::Text(text) => Ok(encode_utf8_scalars(text)),
            Self::Python(value) => encode_python_string(value),
        }
    }
}

/// An explicit per-call writer that must learn exactly which bytes a successful
/// operation published.
///
/// This is a Native *extra* capability, not the Source observer registry. Source
/// installs its observer ambiently with `kernel.atomic.observe_writes`; that
/// canonical context is [`write_observer`] and every notifying operation always
/// consults it first. A per-call listener is consulted second, so passing one
/// cannot suppress or reorder the ambient notification. A call where neither the
/// ambient context nor a per-call listener hears the write reports
/// [`WriteObservation::NoListener`] instead of pretending the notification
/// happened.
pub trait WriteListener {
    /// Called after the destination holds `bytes`.
    ///
    /// A failure is propagated to the caller, exactly as a raising Source
    /// observer propagates *after* the replace landed: the destination keeps the
    /// new content.
    fn written(&mut self, path: &Path, bytes: &[u8]) -> Result<(), WriteListenerFailure>;
}

/// Why a [`WriteListener`] refused to record a published write.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WriteListenerFailure {
    message: String,
}

impl WriteListenerFailure {
    /// A failure carrying the listener's own message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// The listener's message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for WriteListenerFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for WriteListenerFailure {}

/// What happened to the Source write-observer notification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteObservation {
    /// The operation notifies and a listener recorded the published bytes.
    Notified,
    /// The operation notifies, but neither the ambient context nor a per-call
    /// listener heard the write.
    NoListener,
    /// The Source operation never notifies an observer (`write_text`,
    /// `write_generated_file`).
    NotApplicable,
}

/// The observable outcome of a successful writer operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WriteReceipt {
    /// The operation that ran.
    pub operation: WriterOperation,
    /// Published content length.
    pub bytes: usize,
    /// The temporary artifact that was published (`None` for
    /// [`WriterOperation::TextTruncate`]).
    pub temporary: Option<PathBuf>,
    /// The executing process id whose sibling name a generated write used.
    pub temporary_pid: Option<u32>,
    /// What the write observer was told.
    pub observation: WriteObservation,
}

/// A failed writer operation.
///
/// The variants keep the reference's own failure *classes* apart: `ValueError`
/// for the generated writer's refusals, `OSError` for I/O, `UnicodeEncodeError`
/// for encoding, and the observer's own failure. They are not collapsed into one
/// opaque string, because Source callers catch them differently.
#[derive(Debug)]
pub enum SourceWriteError {
    /// `UnicodeEncodeError` from the strict encoder.
    Encode {
        /// Destination the write was for.
        path: PathBuf,
        /// The CPython-shaped encode failure.
        source: PythonUtf8EncodeError,
    },
    /// An `OSError` from one writer phase.
    Io {
        /// Human-readable phase (for example `"create temporary file for"`).
        action: &'static str,
        /// Path the phase acted on (temporary or destination, as Source names it).
        path: PathBuf,
        /// The underlying error.
        source: io::Error,
    },
    /// `write_generated_file` refused a symlink destination (`ValueError`).
    SymlinkRefused {
        /// The symlink destination.
        path: PathBuf,
    },
    /// `write_generated_file` refused a pre-existing sibling artifact
    /// (`ValueError`).
    UnprovenArtifact {
        /// The artifact that already existed.
        path: PathBuf,
    },
    /// The write observer failed after publication.
    Observer {
        /// The destination that already holds the new bytes.
        path: PathBuf,
        /// The message-shaped failure (always present, for compatibility).
        failure: WriteListenerFailure,
        /// The original typed cause when the observer declared one; `None` for a
        /// legacy message-only [`WriteListenerFailure`].
        cause: Option<write_observer::WriteObserverError>,
    },
}

impl SourceWriteError {
    /// Whether Source would raise this as an `OSError` subclass.
    ///
    /// A typed ambient observer that raised an `OSError` is classified as one
    /// (an `except OSError` caller catches it); a message-only listener failure
    /// declares no class and is never guessed into this one.
    pub fn is_os_error(&self) -> bool {
        match self {
            Self::Io { .. } => true,
            Self::Observer {
                cause: Some(cause), ..
            } => cause.is_os_error(),
            _ => false,
        }
    }

    /// Whether Source would raise this as a `ValueError` (which an
    /// `except OSError` caller does **not** catch).
    pub fn is_value_error(&self) -> bool {
        match self {
            Self::SymlinkRefused { .. } | Self::UnprovenArtifact { .. } => true,
            Self::Observer {
                cause: Some(cause), ..
            } => cause.is_value_error(),
            _ => false,
        }
    }

    /// The original typed observer cause, when one was declared.
    pub fn observer_cause(&self) -> Option<&write_observer::WriteObserverError> {
        match self {
            Self::Observer { cause, .. } => cause.as_ref(),
            _ => None,
        }
    }

    /// Whether the destination already holds the new content when this error is
    /// returned (only a post-publication observer failure can say that).
    pub const fn published_before_error(&self) -> bool {
        matches!(self, Self::Observer { .. })
    }
}

impl std::fmt::Display for SourceWriteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Encode { source, .. } => write!(formatter, "{source}"),
            Self::Io {
                action,
                path,
                source,
            } => write!(formatter, "{action} {}: {source}", path.display()),
            Self::SymlinkRefused { path } => write!(
                formatter,
                "write_generated_file refuses to write through a symlink: {} \
                 (the managed generated-file tree is expected to contain only \
                 regular files; replace or remove the symlink before writing)",
                path.display()
            ),
            Self::UnprovenArtifact { path } => write!(
                formatter,
                "Refusing to replace an unproven atomic writer artifact: {}",
                path.display()
            ),
            Self::Observer { failure, .. } => write!(formatter, "{failure}"),
        }
    }
}

impl std::error::Error for SourceWriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Encode { source, .. } => Some(source),
            Self::Io { source, .. } => Some(source),
            Self::Observer {
                cause: Some(cause), ..
            } => Some(cause),
            Self::Observer {
                failure,
                cause: None,
                ..
            } => Some(failure),
            Self::SymlinkRefused { .. } | Self::UnprovenArtifact { .. } => None,
        }
    }
}

/// The sibling artifact `write_generated_file` would use in *this* process.
///
/// Source `generated_temporary_path`: `f".{path.name}.tmp{os.getpid()}"`. The pid
/// is the executing process's actual id, never a synthetic or remembered one.
pub fn generated_temporary_path(path: &Path) -> PathBuf {
    let pid = std::process::id();
    let mut name = std::ffi::OsString::from(".");
    name.push(path.file_name().unwrap_or_default());
    name.push(format!(".tmp{pid}"));
    path.with_file_name(name)
}

/// The sibling artifact `_atomic_write_json` writes through.
///
/// Source `path.with_suffix(".json.tmp")`, including its own rule for names with
/// no suffix and for a trailing dot.
pub fn derived_json_temporary_path(path: &Path) -> PathBuf {
    let name = path.file_name().unwrap_or_default();
    // Python 3.11 treats a final dot as NO suffix (foo. -> foo..json.tmp).
    // Work in the platform's actual filename encoding; no Unicode projection.
    let stem = if path
        .extension()
        .is_some_and(|extension| !extension.is_empty())
    {
        path.file_stem().unwrap_or(name)
    } else {
        name
    };
    let mut sibling = stem.to_os_string();
    sibling.push(".json.tmp");
    path.with_file_name(sibling)
}

/// The sibling artifact `rebuild_event_log` writes through.
///
/// Source `feature_dir / f"{name}.tmp"`.
pub fn rebuild_log_temporary_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

/// Write `payload` to `path` with the policy of exactly one Source operation.
///
/// `listener` is consulted only for the operations Source notifies
/// ([`WriterOperation::Atomic`], [`WriterOperation::DerivedJson`]); the receipt
/// says which of the three observation states happened.
pub fn write_source(
    operation: WriterOperation,
    path: &Path,
    payload: SourcePayload<'_>,
    listener: Option<&mut dyn WriteListener>,
) -> Result<WriteReceipt, SourceWriteError> {
    match operation {
        WriterOperation::Atomic(options) => write_atomic(path, payload, options, listener),
        WriterOperation::Generated(options) => write_generated(path, payload, options),
        WriterOperation::TextTruncate => write_text(path, payload),
        WriterOperation::DerivedJson => write_derived_json(path, payload, listener),
        WriterOperation::RebuildLog => write_rebuild_log(path, payload),
    }
}

// ---------------------------------------------------------------------------
// The four policies
// ---------------------------------------------------------------------------

fn write_atomic(
    path: &Path,
    payload: SourcePayload<'_>,
    options: AtomicWrite,
    listener: Option<&mut dyn WriteListener>,
) -> Result<WriteReceipt, SourceWriteError> {
    if options.create_parents {
        fs::create_dir_all(parent_directory(path))
            .map_err(|source| io_error("create parent of", parent_directory(path), source))?;
    }
    // Source encodes BEFORE the read-only relaxation and before the temp exists.
    let raw = payload
        .encoded()
        .map_err(|source| SourceWriteError::Encode {
            path: path.to_path_buf(),
            source,
        })?;

    let restore_mode = clear_readonly_for_replace(path)?;

    let mut temporary = TemporaryArtifact::create_sibling(parent_directory(path))?;
    let published = temporary
        .write_all(&raw)
        .and_then(|()| temporary.publish(path));
    let outcome = match published {
        Ok(()) => {
            if let Some(mode) = restore_mode {
                restore_mode_best_effort(path, mode);
            }
            Ok(())
        }
        Err(error) => {
            // Source unlinks the failed temporary first, then restores the mode.
            temporary.discard();
            if let Some(mode) = restore_mode {
                restore_mode_best_effort(path, mode);
            }
            Err(error)
        }
    };

    let temporary_path = temporary.path().to_path_buf();
    outcome?;
    let observation = notify(listener, path, &raw)?;
    Ok(WriteReceipt {
        operation: WriterOperation::Atomic(options),
        bytes: raw.len(),
        temporary: Some(temporary_path),
        temporary_pid: None,
        observation,
    })
}

fn write_generated(
    path: &Path,
    payload: SourcePayload<'_>,
    options: GeneratedWrite,
) -> Result<WriteReceipt, SourceWriteError> {
    // Source refuses a symlink destination before it touches anything.
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(SourceWriteError::SymlinkRefused {
                path: path.to_path_buf(),
            });
        }
        Ok(_) | Err(_) => {}
    }

    let temporary_path = generated_temporary_path(path);
    // `tmp.exists() or tmp.is_symlink()`: a broken symlink is refused too.
    if fs::symlink_metadata(&temporary_path).is_ok() {
        return Err(SourceWriteError::UnprovenArtifact {
            path: temporary_path,
        });
    }

    let existing_mode = destination_mode(path)?;
    #[cfg(unix)]
    if let Some(mode) = existing_mode {
        relax_write_bits(path, mode)?;
    }
    #[cfg(not(unix))]
    let _ = existing_mode;

    let mut temporary = TemporaryArtifact::create_named(temporary_path)?;
    let written = payload
        .encoded()
        .map_err(|source| SourceWriteError::Encode {
            path: path.to_path_buf(),
            source,
        })
        .and_then(|raw| temporary.write_all(&raw).map(|()| raw.len()));
    let published = written.and_then(|bytes| temporary.publish(path).map(|()| bytes));

    // Source's `finally`: strip the write bits even when the write failed, and
    // even when the destination is a directory.
    if options.read_only {
        strip_write_bits(path);
    }

    let bytes = published?;
    let temporary_path = temporary.path().to_path_buf();
    Ok(WriteReceipt {
        operation: WriterOperation::Generated(options),
        bytes,
        temporary: Some(temporary_path),
        temporary_pid: Some(std::process::id()),
        observation: WriteObservation::NotApplicable,
    })
}

fn write_text(path: &Path, payload: SourcePayload<'_>) -> Result<WriteReceipt, SourceWriteError> {
    // Source `Path.write_text` opens (creating/truncating) the destination
    // BEFORE it encodes, so an encode failure leaves a zero-byte file.
    let mut file = open_truncating(path)?;
    let raw = payload
        .encoded()
        .map_err(|source| SourceWriteError::Encode {
            path: path.to_path_buf(),
            source,
        })?;
    file.write_all(&raw)
        .map_err(|source| io_error("write", path, source))?;
    file.flush()
        .map_err(|source| io_error("flush", path, source))?;
    drop(file);
    Ok(WriteReceipt {
        operation: WriterOperation::TextTruncate,
        bytes: raw.len(),
        temporary: None,
        temporary_pid: None,
        observation: WriteObservation::NotApplicable,
    })
}

fn write_derived_json(
    path: &Path,
    payload: SourcePayload<'_>,
    listener: Option<&mut dyn WriteListener>,
) -> Result<WriteReceipt, SourceWriteError> {
    // `_atomic_write_json` has no cleanup path: the artifact stays behind when a
    // later phase fails. Source also truncates and reuses an existing artifact.
    let temporary_path = derived_json_temporary_path(path);
    write_fixed_sibling(
        WriterOperation::DerivedJson,
        path,
        temporary_path,
        payload,
        false,
        listener,
    )
}

fn write_rebuild_log(
    path: &Path,
    payload: SourcePayload<'_>,
) -> Result<WriteReceipt, SourceWriteError> {
    // `rebuild_event_log` unlinks the fixed sibling in a `finally`, so no
    // artifact survives any failure, and it never notifies an observer.
    let temporary_path = rebuild_log_temporary_path(path);
    write_fixed_sibling(
        WriterOperation::RebuildLog,
        path,
        temporary_path,
        payload,
        true,
        None,
    )
}

/// The shared "fixed sibling temporary, then rename" phase.
///
/// `cleanup_on_failure` separates the two Source operations that use it:
/// `rebuild_event_log` removes the artifact in a `finally`;
/// `_atomic_write_json` has no cleanup path at all.
fn write_fixed_sibling(
    operation: WriterOperation,
    path: &Path,
    temporary_path: PathBuf,
    payload: SourcePayload<'_>,
    cleanup_on_failure: bool,
    listener: Option<&mut dyn WriteListener>,
) -> Result<WriteReceipt, SourceWriteError> {
    let mut temporary = TemporaryArtifact::create_named(temporary_path)?;
    if !cleanup_on_failure {
        temporary = temporary.keep_on_failure();
    }
    let raw = payload
        .encoded()
        .map_err(|source| SourceWriteError::Encode {
            path: path.to_path_buf(),
            source,
        })?;
    temporary.write_all(&raw)?;
    temporary.publish(path)?;
    let temporary_path = temporary.path().to_path_buf();
    let observation = if operation.notifies_observer() {
        notify(listener, path, &raw)?
    } else {
        WriteObservation::NotApplicable
    };
    Ok(WriteReceipt {
        operation,
        bytes: raw.len(),
        temporary: Some(temporary_path),
        temporary_pid: None,
        observation,
    })
}

// ---------------------------------------------------------------------------
// One temporary/publication/cleanup authority
// ---------------------------------------------------------------------------

/// A held-descriptor temporary artifact with explicit publication and cleanup.
///
/// Every operation except [`WriterOperation::TextTruncate`] goes through this
/// type: the descriptor is held while the payload is written, the rename is the
/// only publication step, and a failure removes the artifact unless the Source
/// operation leaves it behind ([`Self::keep_on_failure`]).
#[derive(Debug)]
struct TemporaryArtifact {
    path: PathBuf,
    file: Option<File>,
    /// Remove the artifact when dropped. Source `_atomic_write_json` clears it.
    armed: bool,
}

impl TemporaryArtifact {
    /// mkstemp-shaped sibling: `.atomic-<unique>.tmp`, mode `0600`.
    fn create_sibling(directory: &Path) -> Result<Self, SourceWriteError> {
        let named = tempfile::Builder::new()
            .prefix(".atomic-")
            .suffix(".tmp")
            .tempfile_in(directory)
            .map_err(|source| io_error("create temporary file in", directory, source))?;
        let (file, path) = named
            .keep()
            .map_err(|error| io_error("create temporary file in", directory, error.error))?;
        Ok(Self {
            path,
            file: Some(file),
            armed: true,
        })
    }

    /// A named artifact created `0666 & ~umask`, truncating any existing file.
    fn create_named(path: PathBuf) -> Result<Self, SourceWriteError> {
        let file = open_truncating(&path)?;
        Ok(Self {
            path,
            file: Some(file),
            armed: true,
        })
    }

    /// Leave the artifact behind on failure (Source `_atomic_write_json`).
    fn keep_on_failure(mut self) -> Self {
        self.armed = false;
        self
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn write_all(&mut self, bytes: &[u8]) -> Result<(), SourceWriteError> {
        let path = self.path.clone();
        self.file
            .as_mut()
            .ok_or_else(|| {
                io_error(
                    "write temporary file for",
                    &path,
                    io::Error::other("closed"),
                )
            })?
            .write_all(bytes)
            .map_err(|source| io_error("write temporary file for", &path, source))
    }

    /// Close the descriptor (a `close` flush, never an `fsync`) and rename over
    /// the destination (Source `Path.replace` / `os.replace`). No `fsync` of file
    /// or directory: the reference does not sync these writers.
    fn publish(&mut self, destination: &Path) -> Result<(), SourceWriteError> {
        self.close()?;
        fs::rename(&self.path, destination)
            .map_err(|source| io_error("replace", destination, source))?;
        self.armed = false;
        Ok(())
    }

    /// Close and remove the artifact, suppressing errors like Source's
    /// `contextlib.suppress(OSError)`.
    fn discard(&mut self) {
        let _ = self.close();
        self.armed = false;
        let _ = fs::remove_file(&self.path);
    }

    fn close(&mut self) -> Result<(), SourceWriteError> {
        let Some(mut file) = self.file.take() else {
            return Ok(());
        };
        let path = self.clone_path();
        // Source's `with open(...)` closes (flushes) the stream; it never fsyncs.
        file.flush()
            .map_err(|source| io_error("close temporary file for", &path, source))
    }

    fn clone_path(&self) -> PathBuf {
        self.path.clone()
    }
}

impl Drop for TemporaryArtifact {
    fn drop(&mut self) {
        let _ = self.close();
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

// ---------------------------------------------------------------------------
// Shared phases
// ---------------------------------------------------------------------------

fn open_truncating(path: &Path) -> Result<File, SourceWriteError> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Source `io.open(path, "w")` asks for 0666 and lets the OS apply umask.
        options.mode(TEXT_CREATE_MODE);
    }
    options
        .open(path)
        .map_err(|source| io_error("open", path, source))
}

fn io_error(action: &'static str, path: &Path, source: io::Error) -> SourceWriteError {
    SourceWriteError::Io {
        action,
        path: path.to_path_buf(),
        source,
    }
}

fn parent_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn notify(
    listener: Option<&mut dyn WriteListener>,
    path: &Path,
    bytes: &[u8],
) -> Result<WriteObservation, SourceWriteError> {
    // Canonical ambient context first (Source order), then the explicit
    // per-call listener. A raising ambient observer skips the later listener,
    // exactly as a raising Source chain skips its inner observers.
    let ambient_heard = write_observer::notify_written(path, bytes).map_err(|cause| {
        SourceWriteError::Observer {
            path: path.to_path_buf(),
            failure: WriteListenerFailure::new(cause.message()),
            cause: Some(cause),
        }
    })?;
    let per_call_heard = match listener {
        None => false,
        Some(listener) => {
            listener
                .written(path, bytes)
                .map_err(|failure| SourceWriteError::Observer {
                    path: path.to_path_buf(),
                    failure,
                    cause: None,
                })?;
            true
        }
    };
    if ambient_heard || per_call_heard {
        Ok(WriteObservation::Notified)
    } else {
        Ok(WriteObservation::NoListener)
    }
}

/// Source `_clear_readonly_for_replace`: no-follow, masked, owner-write only.
///
/// Returns the original `S_IMODE` when the owner-write bit had to be restored.
/// A symlink, an absent destination, or an already-writable destination returns
/// `None` (Source leaves those untouched).
#[cfg(unix)]
fn clear_readonly_for_replace(path: &Path) -> Result<Option<u32>, SourceWriteError> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(io_error("inspect destination", path, source)),
    };
    if metadata.file_type().is_symlink() {
        return Ok(None);
    }
    let original = metadata.permissions().mode() & 0o7777;
    if original & 0o200 != 0 {
        return Ok(None);
    }
    fs::set_permissions(path, fs::Permissions::from_mode(original | 0o200))
        .map_err(|source| io_error("relax read-only mode of", path, source))?;
    Ok(Some(original))
}

#[cfg(not(unix))]
fn clear_readonly_for_replace(path: &Path) -> Result<Option<u32>, SourceWriteError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(io_error("inspect destination", path, source)),
    };
    if metadata.file_type().is_symlink() || !metadata.permissions().readonly() {
        return Ok(None);
    }
    let mut writable = metadata.permissions();
    writable.set_readonly(false);
    fs::set_permissions(path, writable)
        .map_err(|source| io_error("relax read-only mode of", path, source))?;
    Ok(Some(1))
}

#[cfg(unix)]
fn restore_mode_best_effort(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn restore_mode_best_effort(path: &Path, _mode: u32) {
    if let Ok(metadata) = fs::metadata(path) {
        let mut permissions = metadata.permissions();
        permissions.set_readonly(true);
        let _ = fs::set_permissions(path, permissions);
    }
}

#[cfg(unix)]
fn destination_mode(path: &Path) -> Result<Option<u32>, SourceWriteError> {
    use std::os::unix::fs::PermissionsExt;
    match fs::metadata(path) {
        Ok(metadata) => Ok(Some(metadata.permissions().mode() & 0o7777)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(io_error("inspect destination", path, source)),
    }
}

#[cfg(not(unix))]
fn destination_mode(path: &Path) -> Result<Option<u32>, SourceWriteError> {
    match fs::metadata(path) {
        Ok(_) => Ok(Some(0)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(io_error("inspect destination", path, source)),
    }
}

/// Source relaxes *all* write bits (`| 0o222`), not the owner bit only, and only
/// when the destination currently has none.
#[cfg(unix)]
fn relax_write_bits(path: &Path, mode: u32) -> Result<(), SourceWriteError> {
    use std::os::unix::fs::PermissionsExt;
    if mode & WRITE_BITS != 0 {
        return Ok(());
    }
    fs::set_permissions(path, fs::Permissions::from_mode(mode | WRITE_BITS))
        .map_err(|source| io_error("relax write mode of", path, source))
}

#[cfg(not(unix))]
fn relax_write_bits(path: &Path, _mode: u32) -> Result<(), SourceWriteError> {
    let metadata =
        fs::metadata(path).map_err(|source| io_error("inspect destination", path, source))?;
    if !metadata.permissions().readonly() {
        return Ok(());
    }
    let mut relaxed = metadata.permissions();
    relaxed.set_readonly(false);
    fs::set_permissions(path, relaxed)
        .map_err(|source| io_error("relax write mode of", path, source))
}

#[cfg(unix)]
fn strip_write_bits(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(metadata) = fs::metadata(path) {
        let mode = metadata.permissions().mode() & 0o7777;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode & !WRITE_BITS));
    }
}

#[cfg(not(unix))]
fn strip_write_bits(path: &Path) {
    if let Ok(metadata) = fs::metadata(path) {
        let mut permissions = metadata.permissions();
        permissions.set_readonly(true);
        let _ = fs::set_permissions(path, permissions);
    }
}
