//! Composed write-observation ledgers: finalize capture, bake capture, status notify.
//!
//! The pinned reference composes three layers around ONE ambient write observer
//! (`kernel.atomic.observe_writes` / `notify_written`, reproduced by
//! [`sk_kernel::source_writer::write_observer`]):
//!
//! 1. **The finalize write ledger** (`cli/commands/agent/mission_finalize_commit.py`).
//!    `begin_write_ledger` installs one observer that records the exact bytes of
//!    every write the run makes, keyed by the *resolved* path (the latest write
//!    wins). The failed-run restore then rewrites or deletes a path **only** while
//!    it still holds the bytes this run wrote: a file finalize never wrote, or one
//!    another writer changed since, is kept and reported.
//! 2. **The mission-number bake capture** (`consolidation/mission_number/bake.py`).
//!    `_locked_assign_mission_number` wraps its single locked `meta.json` update in
//!    `observe_writes(...)` filtered on `path.name == "meta.json"`, so the bake
//!    learns the exact bytes it published (and the bytes before it) without
//!    re-rendering the document. Those bytes feed the compare-and-swap undo.
//! 3. **Status/derived direct notify** (`status/store.py`,
//!    `status/reducer.py::materialize`, `status/views.py::_atomic_write_json`,
//!    `status/lifecycle.py`, `status/progress.py`). Each writer tells the installed
//!    observer `(path, exact bytes)` **after** its own publication (and, for the
//!    append writer, after the file `fsync`), and the snapshot writer only when it
//!    actually replaced the file.
//!
//! # Why this module is a composition, not a new authority
//!
//! * The observer registry is the Kernel's single ambient context
//!   ([`sk_kernel::source_writer::observe_writes`],
//!   [`sk_kernel::source_writer::notify_written`],
//!   [`sk_kernel::WriteContext`]). There is no second thread-local observer, no
//!   per-writer ledger and no private registry here.
//! * The status writes still go through the canonical durable status writer
//!   ([`sk_status::append_rows_atomic`], [`sk_status::materialize_snapshot_file`]);
//!   the ledger never re-implements the JSONL or snapshot policy, and the existing
//!   lock/durability contract is untouched.
//! * `meta.json` still goes through the canonical Mission metadata writer
//!   ([`sk_mission::record_mission_number`]); this module only *observes* it.
//! * What this module adds is the composition the reference performs at its owner
//!   boundary: a ledger bound to a run, the owned-byte restore, the bake capture,
//!   and the post-publication notify for the durable Native writers that keep their
//!   own (non-notifying) contract.
//!
//! # The "owner-boundary notify" policy (explicit, not inferred)
//!
//! Native durable writers ([`sk_kernel::atomic_write`]) keep their no-op-stable,
//! `fsync`-ing, parent-synced contract. The Mission metadata owner now notifies
//! exact computed bytes after its own changed publication/readback; the raw
//! Kernel primitive still does not notify. Status composition below retains a
//! separate readback-boundary capability, not whole Source writer parity. The composed operations here notify **after** the
//! writer has published and verified its own bytes, using the bytes the writer
//! guarantees are now on disk:
//!
//! * status append — [`sk_status::append_rows_atomic`] verifies byte equality by
//!   readback before returning, so the log on disk *is* the published payload;
//! * snapshot materialization — the write reports `changed`, exactly the condition
//!   Source gates `notify_written` on;
//! * `meta.json` — [`sk_mission::record_mission_number`] verifies the readback of
//!   the bytes it wrote.
//!
//! Each notify reports (and the bake capture records) whether the bytes came from
//! an observer that fired inside the writer ([`ObservationSource::Observer`]) or
//! from the writer's readback-verified publication
//! ([`ObservationSource::ReadbackVerified`]). Nothing is predicted, re-rendered or
//! inferred from a path, and a path alone never produces a ledger entry.
//!
//! # Deliberate limits (not waivers)
//!
//! * The restore is best-effort alongside the caller's original failure, like
//!   Source's: a failing restore path is reported, never raised as a replacement
//!   diagnostic (see [`restore_mission_write_scope`]'s `skipped` list).
//! * Locking: the restore runs under the canonical Mission write lock. The
//!   [`restore_mission_write_scope`] door takes a [`MissionLock`] so a caller that
//!   already holds it (the finalize run) proves the hold; the
//!   [`restore_mission_write_scope_locked`] door acquires it.
//! * The status-surface (branch/commit) restore, the `meta.json` single-writer
//!   revert, and CLI wiring are NOT implemented here; those are separate surfaces
//!   (see the crate report).
//! * Cross-process observers, mid-write races, Windows read-only replacement and
//!   the error *wire* format remain OPEN.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::Value;
use sk_kernel::source_writer::{
    WriteObserverError, WriteObserverToken, notify_written, observe_writes_callback,
};
use sk_kernel::{KernelError, LockWait, read_bounded};
use sk_status::MutationError;
use sk_status::append_rows_atomic;
use sk_status::mutation::MissionLock;
use sk_status::mutation_log::{
    AppendOptions, AppendOutcome, materialize_snapshot_file, read_log_text,
};

/// The `meta.json` file name the bake capture filters on (Source `path.name == "meta.json"`).
pub const META_JSON_FILENAME: &str = sk_mission::META_FILE_NAME;

/// Upper bound for one write-scope snapshot/restore file read.
///
/// Source reads the tracked files without a bound. The composed restore applies
/// the crate's bounded-read discipline instead of loading an arbitrarily large
/// user file into memory; an oversized file is reported as skipped, never
/// rewritten from a truncated copy.
pub const MAX_WRITE_SCOPE_FILE_BYTES: u64 = 32 * 1024 * 1024;

/// Why a composed ledger operation refused.
#[derive(Debug, thiserror::Error)]
pub enum WriteLedgerError {
    /// A canonical Kernel read/write primitive refused.
    #[error(transparent)]
    Kernel(#[from] KernelError),

    /// The canonical Mission metadata writer refused.
    #[error(transparent)]
    Mission(#[from] sk_mission::MissionError),

    /// The canonical durable status writer refused.
    #[error(transparent)]
    Status(#[from] MutationError),

    /// The Mission write lock could not be taken (Source's lock is the guard).
    #[error("could not take the Mission write lock for {directory}: {source}")]
    Lock {
        /// Mission directory the lock was resolved for.
        directory: PathBuf,
        /// The canonical lock refusal.
        #[source]
        source: MutationError,
    },

    /// An installed write observer refused *after* its write already landed.
    ///
    /// This keeps the Source exception class (`OSError`-class, `ValueError`-class,
    /// arbitrary custom object, or a class-less message) instead of collapsing it
    /// into a string. The file holds the new bytes: this is a post-publication
    /// failure, so a caller must not treat it as "nothing was written".
    #[error("the write observer installed for {path} failed after the write landed: {source}")]
    Observer {
        /// The path whose bytes were published.
        path: PathBuf,
        /// The typed observer failure.
        #[source]
        source: WriteObserverError,
    },

    /// A writer published bytes whose readback does not match what it reported.
    #[error(
        "{path} holds {actual} bytes after the writer reported {expected}; refusing to record it"
    )]
    ReadbackMismatch {
        /// The published file.
        path: PathBuf,
        /// Byte count the writer reported.
        expected: u64,
        /// Byte count observed on disk.
        actual: u64,
    },

    /// A composed read failed for a reason other than "absent".
    #[error("could not read {path}: {source}")]
    Read {
        /// The file that was read.
        path: PathBuf,
        /// The filesystem refusal.
        #[source]
        source: std::io::Error,
    },
}

impl WriteLedgerError {
    /// Whether the failure happened after the destination already held new bytes.
    ///
    /// Only a post-publication observer failure can say that, exactly like
    /// [`sk_kernel::source_writer::SourceWriteError::published_before_error`].
    pub const fn published_before_error(&self) -> bool {
        match self {
            Self::Observer { .. } => true,
            Self::Mission(error) => error.published_before_error(),
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// The ledger
// ---------------------------------------------------------------------------

/// The exact bytes one run wrote, per resolved path (Source `FinalizeWriteLedger`).
///
/// Every write recorded is a real, published `(path, bytes)` notification. The
/// latest write to one path wins, and the path is keyed by its resolved form, so
/// two spellings of the same file share one entry.
#[derive(Debug, Default)]
pub struct WriteLedger {
    written: Mutex<BTreeMap<PathBuf, Vec<u8>>>,
}

impl WriteLedger {
    /// An empty ledger.
    pub fn new() -> Self {
        Self::default()
    }

    /// Remember that `path` now holds `bytes` (the latest write of the run wins).
    ///
    /// This is the observer callback body: it performs no I/O and never fails.
    pub fn record(&self, path: &Path, bytes: &[u8]) {
        lock(&self.written).insert(resolve_path(path), bytes.to_vec());
    }

    /// The bytes the run last wrote to `path`, when it wrote there at all.
    pub fn bytes_for(&self, path: &Path) -> Option<Vec<u8>> {
        lock(&self.written).get(&resolve_path(path)).cloned()
    }

    /// The text the run last wrote to `path` (Source `text_for`).
    ///
    /// `None` when nothing was written there, or when the recorded bytes are not
    /// UTF-8 — the recorded bytes themselves are always retained verbatim.
    pub fn text_for(&self, path: &Path) -> Option<String> {
        self.bytes_for(path)
            .and_then(|bytes| String::from_utf8(bytes).ok())
    }

    /// Whether the run wrote to `path`.
    pub fn contains(&self, path: &Path) -> bool {
        lock(&self.written).contains_key(&resolve_path(path))
    }

    /// Drop `path` from the ledger (Source's restore does this for bytes it put
    /// back: those are the pre-run state, not something the run wrote).
    pub fn forget(&self, path: &Path) {
        lock(&self.written).remove(&resolve_path(path));
    }

    /// Every recorded `(resolved path, bytes)` pair, sorted by path.
    pub fn entries(&self) -> Vec<(PathBuf, Vec<u8>)> {
        lock(&self.written)
            .iter()
            .map(|(path, bytes)| (path.clone(), bytes.clone()))
            .collect()
    }

    /// The recorded paths, sorted.
    pub fn paths(&self) -> Vec<PathBuf> {
        lock(&self.written).keys().cloned().collect()
    }

    /// How many distinct files the run wrote.
    pub fn len(&self) -> usize {
        lock(&self.written).len()
    }

    /// Whether the run wrote nothing.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// ---------------------------------------------------------------------------
// The run-scoped ledger binding
// ---------------------------------------------------------------------------

thread_local! {
    /// The ledger scopes entered on this thread, innermost last.
    ///
    /// Source keeps the active ledger in its own `ContextVar`
    /// (`_ACTIVE_LEDGER`), separate from the write-observer `ContextVar`. This is
    /// the same split: the observer is the Kernel's ambient context, the active
    /// ledger is consumer state that only [`active_write_ledger`] observes.
    static ACTIVE: std::cell::RefCell<Vec<Arc<WriteLedger>>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// A ledger bound to the current write-observation context.
///
/// Dropping (or [`Self::finish`]) stops the observer this scope installed and
/// restores the previously active ledger. The scope holds the Kernel observer
/// token, so the observer chain is the Kernel's own outer-first chain — a nested
/// scope notifies the outer observer first, then the inner ledger.
/// A live scope owns thread-local state and cannot be dropped on another thread:
///
/// ```compile_fail
/// let session = sk_workspace::begin_write_ledger();
/// std::thread::spawn(move || drop(session));
/// ```
#[must_use = "the ledger only records while the scope is alive"]
pub struct WriteLedgerSession {
    ledger: Arc<WriteLedger>,
    token: Option<WriteObserverToken>,
    _thread_bound: PhantomData<Rc<()>>,
}

impl WriteLedgerSession {
    /// The ledger this scope records into.
    pub fn ledger(&self) -> &Arc<WriteLedger> {
        &self.ledger
    }

    /// Stop recording and restore the previously active ledger.
    pub fn finish(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        let Some(token) = self.token.take() else {
            return;
        };
        // Source `end_write_ledger`: stop observing first, then reset the active
        // ledger. An already-consumed token is ignored rather than panicking.
        let _ = sk_kernel::stop_observing_writes(&token);
        ACTIVE.with(|stack| {
            let mut stack = stack.borrow_mut();
            if let Some(position) = stack
                .iter()
                .rposition(|item| Arc::ptr_eq(item, &self.ledger))
            {
                stack.remove(position);
            }
        });
    }
}

impl Drop for WriteLedgerSession {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Start recording the current context's writes (Source `begin_write_ledger`).
///
/// The observer is installed through the canonical ambient context, so a scope
/// entered inside another one hears every write the outer one hears with the
/// reference's outer-first ordering. The caller is responsible for the context:
/// enter a [`sk_kernel::WriteContext`] first when the run needs an explicitly
/// isolated one.
pub fn begin_write_ledger() -> WriteLedgerSession {
    let ledger = Arc::new(WriteLedger::new());
    let callback_ledger = Arc::clone(&ledger);
    let token = observe_writes_callback(Arc::new(move |path: &Path, bytes: &[u8]| {
        callback_ledger.record(path, bytes);
        Ok(())
    }));
    ACTIVE.with(|stack| stack.borrow_mut().push(Arc::clone(&ledger)));
    WriteLedgerSession {
        ledger,
        token: Some(token),
        _thread_bound: PhantomData,
    }
}

/// The ledger of the run in progress in this thread, if any (Source `active_write_ledger`).
pub fn active_write_ledger() -> Option<Arc<WriteLedger>> {
    ACTIVE.with(|stack| stack.borrow().last().cloned())
}

/// Run `operation` with a ledger scope and return `(ledger, result)`.
///
/// The ledger remains readable after the scope is stopped; that is the shape the
/// finalize run needs (record while writing, restore after the failure).
pub fn with_write_ledger<T, F>(operation: F) -> (Arc<WriteLedger>, T)
where
    F: FnOnce() -> T,
{
    let session = begin_write_ledger();
    let result = operation();
    let ledger = Arc::clone(session.ledger());
    session.finish();
    (ledger, result)
}

// ---------------------------------------------------------------------------
// Notify
// ---------------------------------------------------------------------------

/// Tell the installed observer that `path` now holds `bytes` (post-publication).
///
/// `Ok(true)` when a chain was installed and ran, `Ok(false)` when nothing was
/// installed — the "no listener" state is reported, never invented. A typed
/// observer failure is returned as [`WriteLedgerError::Observer`] and the file
/// already holds the new bytes.
pub fn notify_published(path: &Path, bytes: &[u8]) -> Result<bool, WriteLedgerError> {
    notify_written(path, bytes).map_err(|source| WriteLedgerError::Observer {
        path: path.to_path_buf(),
        source,
    })
}

/// Where the bytes a composed operation recorded came from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservationSource {
    /// An observer installed *inside* the canonical writer heard the write and
    /// supplied the bytes (the reference's shape).
    Observer,
    /// The writer published and verified its own readback; the composed operation
    /// notified (and recorded) exactly those verified bytes at the owner boundary.
    ReadbackVerified,
}

/// What a composed status write told the observer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WriteNotification {
    /// Whether any observer heard the write.
    pub notified: bool,
    /// Byte count of the payload handed to the observer.
    pub bytes: usize,
    /// Where the payload came from.
    pub source: ObservationSource,
}

/// Append status rows through the canonical durable writer, then notify.
///
/// The rows are written by [`sk_status::append_rows_atomic`] with its own lock,
/// whole-file replacement, `fsync` and readback verification — nothing about that
/// contract changes here. After it succeeds, the log's exact published bytes are
/// handed to the installed observer (Source `notify_written` after the replace),
/// and the notification reports [`ObservationSource::ReadbackVerified`]: the
/// writer proved by readback that these are the bytes on disk.
pub fn append_status_rows_observed(
    path: &Path,
    rows: &[Value],
    options: &AppendOptions,
) -> Result<(AppendOutcome, Option<WriteNotification>), WriteLedgerError> {
    let outcome = append_rows_atomic(path, rows, options)?;
    if outcome.rows_appended == 0 {
        // Source `append_raw_rows_atomic` returns before writing when there are
        // no rows: nothing was published, so nothing is notified.
        return Ok((outcome, None));
    }
    let text = read_log_text(path)?;
    let bytes = text.unwrap_or_default().into_bytes();
    if bytes.len() as u64 != outcome.written_bytes {
        return Err(WriteLedgerError::ReadbackMismatch {
            path: path.to_path_buf(),
            expected: outcome.written_bytes,
            actual: bytes.len() as u64,
        });
    }
    let notified = notify_published(path, &bytes)?;
    Ok((
        outcome,
        Some(WriteNotification {
            notified,
            bytes: bytes.len(),
            source: ObservationSource::ReadbackVerified,
        }),
    ))
}

/// Materialize `status.json` through the canonical reducer and notify a change.
///
/// Source's `materialize` notifies only when it actually replaced the file; the
/// Native writer reports that as `changed`. The canonical writer keeps its
/// no-op-stable durability contract untouched; the composed notification carries
/// the file's exact published bytes.
pub fn materialize_status_snapshot_observed(
    feature_dir: &Path,
) -> Result<Option<WriteNotification>, WriteLedgerError> {
    let outcome = materialize_snapshot_file(feature_dir)?;
    if !outcome.changed {
        return Ok(None);
    }
    let bytes = read_bounded(&outcome.path, MAX_WRITE_SCOPE_FILE_BYTES)?;
    let notified = notify_published(&outcome.path, &bytes)?;
    Ok(Some(WriteNotification {
        notified,
        bytes: bytes.len(),
        source: ObservationSource::ReadbackVerified,
    }))
}

// ---------------------------------------------------------------------------
// Mission-number bake capture
// ---------------------------------------------------------------------------

/// The exact `meta.json` bytes one bake wrote, plus the bytes before it.
///
/// Source `_locked_assign_mission_number` returns `(before, written)` read inside
/// its lock hold; a refused bookkeeping commit then restores `before` only while
/// the file still holds `written`. Both byte strings are what the file actually
/// held (readback-verified publication), never a re-render.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetaBakeCapture {
    /// The `meta.json` the bake wrote.
    pub path: PathBuf,
    /// The bytes before the write, or `None` when the file did not exist.
    pub before: Option<Vec<u8>>,
    /// The exact bytes the bake published.
    pub written: Vec<u8>,
    /// Where [`Self::written`] came from.
    pub source: ObservationSource,
}

/// Bake `mission_number` into a Mission's `meta.json` and capture the real bytes.
///
/// The write itself is the canonical [`sk_mission::record_mission_number`] (every
/// field preserved, canonical pretty/sorted JSON, atomic replacement, readback
/// verification). While it runs, a ledger scope records any observer notification
/// the writer makes; when the writer published without notifying (its documented
/// durable-Native contract), the scope notifies and records the writer's
/// readback-verified bytes at this owner boundary — exactly the `(path, bytes)`
/// pair Source's observer receives inside `locked_update_meta`.
pub fn bake_mission_number(
    feature_dir: &Path,
    mission_number: i64,
) -> Result<MetaBakeCapture, WriteLedgerError> {
    let path = feature_dir.join(META_JSON_FILENAME);
    let before = match fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => {
            return Err(WriteLedgerError::Read {
                path: path.clone(),
                source,
            });
        }
    };

    let session = begin_write_ledger();
    let written_path = sk_mission::record_mission_number(feature_dir, mission_number)?;
    let observed = session.ledger().bytes_for(&written_path);
    let (written, source) = match observed {
        Some(bytes) => (bytes, ObservationSource::Observer),
        None => {
            // Owner-boundary notify: the canonical writer verified this readback,
            // so these ARE the published bytes; notify after that publication.
            let bytes = sk_kernel::read_bounded(&written_path, MAX_WRITE_SCOPE_FILE_BYTES)?;
            notify_published(&written_path, &bytes)?;
            (bytes, ObservationSource::ReadbackVerified)
        }
    };
    session.finish();
    Ok(MetaBakeCapture {
        path: written_path,
        before,
        written,
        source,
    })
}

/// What undoing a [`MetaBakeCapture`] did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BakeUndoOutcome {
    /// The captured pre-bake bytes are back on disk.
    Restored,
    /// The file already held the pre-bake bytes; nothing was written.
    AlreadyBefore,
    /// Another writer changed (or removed) the file after the bake: it was left
    /// exactly as that writer left it, and nothing was reverted.
    ChangedByAnotherWriter,
}

/// Put a baked `meta.json` back, only while it still holds what the bake wrote.
///
/// This is Source `_undo_primary_bake`: a compare-and-swap. A field another writer
/// set since is never erased, so the bake can be undone after a refused
/// bookkeeping commit without clobbering a concurrent edit. The captured `written`
/// bytes are the ownership proof — a path alone proves nothing.
pub fn undo_meta_bake(capture: &MetaBakeCapture) -> Result<BakeUndoOutcome, WriteLedgerError> {
    let current = match fs::read(&capture.path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => {
            return Err(WriteLedgerError::Read {
                path: capture.path.clone(),
                source,
            });
        }
    };
    if current.as_ref() == capture.before.as_ref() {
        return Ok(BakeUndoOutcome::AlreadyBefore);
    }
    if current.as_deref() != Some(capture.written.as_slice()) {
        return Ok(BakeUndoOutcome::ChangedByAnotherWriter);
    }
    match &capture.before {
        Some(bytes) => {
            sk_kernel::atomic_write_with_options(
                &capture.path,
                bytes,
                sk_kernel::AtomicWriteOptions {
                    create_parents: true,
                },
            )?;
        }
        None => match fs::remove_file(&capture.path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(WriteLedgerError::Kernel(KernelError::Io {
                    action: "remove",
                    path: capture.path.clone(),
                    source,
                }));
            }
        },
    }
    // The restored bytes are the pre-bake state, not something this run wrote.
    if let Some(ledger) = active_write_ledger() {
        ledger.forget(&capture.path);
    }
    Ok(BakeUndoOutcome::Restored)
}

// ---------------------------------------------------------------------------
// Mission write-scope snapshot / owned-byte restore
// ---------------------------------------------------------------------------
/// A byte snapshot of every tracked file under a Mission directory.
///
/// `meta.json` is excluded on purpose: it has its own single-writer revert door,
/// and a second writer here would contradict that guarantee (Source
/// `_mission_write_scope_files`).
pub type WriteScopeSnapshot = BTreeMap<PathBuf, Vec<u8>>;

/// Snapshot every file under `mission_dir` (Source `_snapshot_mission_write_scope`).
///
/// A missing Mission directory snapshots as empty instead of raising. Paths are
/// kept as the walk found them (no resolution), matching Source: the restore
/// compares the same spellings it snapshotted. A symlink to a regular file is
/// tracked (Source `Path.is_file()` follows it); the walk never descends a
/// symlinked directory (Source's recursive glob does not either).
pub fn snapshot_mission_write_scope(
    mission_dir: &Path,
) -> Result<WriteScopeSnapshot, WriteLedgerError> {
    let mut snapshot = WriteScopeSnapshot::new();
    if !mission_dir.exists() {
        return Ok(snapshot);
    }
    let mut files = Vec::new();
    collect_scope_files(mission_dir, &mut files)?;
    files.sort();
    for path in files {
        let bytes = read_bounded(&path, MAX_WRITE_SCOPE_FILE_BYTES)?;
        snapshot.insert(path, bytes);
    }
    Ok(snapshot)
}

fn collect_scope_files(directory: &Path, out: &mut Vec<PathBuf>) -> Result<(), WriteLedgerError> {
    let entries = fs::read_dir(directory).map_err(|source| {
        WriteLedgerError::Kernel(KernelError::Io {
            action: "list the Mission write scope of",
            path: directory.to_path_buf(),
            source,
        })
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| {
            WriteLedgerError::Kernel(KernelError::Io {
                action: "list the Mission write scope of",
                path: directory.to_path_buf(),
                source,
            })
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| {
            WriteLedgerError::Kernel(KernelError::Io {
                action: "inspect",
                path: path.clone(),
                source,
            })
        })?;
        if file_type.is_dir() {
            collect_scope_files(&path, out)?;
            continue;
        }
        // Source `Path.is_file()` FOLLOWS a symlink, while its recursive walk
        // does not descend a symlinked directory. A symlink to a regular file is
        // therefore part of the tracked scope; a symlink to a directory is not.
        let is_file = file_type.is_file()
            || (file_type.is_symlink()
                && fs::metadata(&path).is_ok_and(|metadata| metadata.is_file()));
        if is_file && path.file_name().and_then(|name| name.to_str()) != Some(META_JSON_FILENAME) {
            out.push(path);
        }
    }
    Ok(())
}

/// The paths a restore could not put back, and why.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WriteScopeRestore {
    /// Files left alone because another writer changed them after this run wrote
    /// them (Source's `kept` list, sorted).
    pub kept: Vec<PathBuf>,
    /// Files this restore failed to read or write, with the failure message.
    /// Best-effort cleanup never replaces the caller's original diagnostic.
    pub skipped: Vec<(PathBuf, String)>,
}

/// Undo this run's writes under `mission_dir`, under the Mission write lock.
///
/// Acquires the canonical Mission write lock for `repo_root`/`mission_dir`, then
/// performs [`restore_mission_write_scope`]. The lock is the one the finalize run
/// also holds; a nested hold on the same thread re-enters it.
pub fn restore_mission_write_scope_locked(
    before: &WriteScopeSnapshot,
    mission_dir: &Path,
    repo_root: &Path,
    written: Option<&WriteLedger>,
    keep: &BTreeSet<PathBuf>,
    keep_under: Option<&Path>,
) -> Result<WriteScopeRestore, WriteLedgerError> {
    let lock = sk_mission::lock_mission(
        mission_dir,
        repo_root,
        LockWait::Bounded(sk_mission::MISSION_WRITE_LOCK_TIMEOUT),
    )
    .map_err(|source| WriteLedgerError::Lock {
        directory: mission_dir.to_path_buf(),
        source,
    })?;
    restore_mission_write_scope(before, mission_dir, &lock, written, keep, keep_under)
}

/// Undo a failed run's tracked writes under `mission_dir` (Source
/// `_restore_mission_write_scope`).
///
/// * a file present in `before` is rewritten to its original bytes;
/// * a file that now exists under `mission_dir` but was absent from `before` (and
///   that this run created) is deleted;
/// * a file in `keep`, or anywhere under `keep_under`, is neither rewritten nor
///   deleted — it belongs to the status branch/commit restore;
/// * with `written` (the ledger of this run's bytes) a file is rewritten or deleted
///   **only** while its bytes still equal what this run wrote there. A file the
///   run never wrote, one another writer changed since, or one another writer
///   deleted is kept and returned in [`WriteScopeRestore::kept`]. Ownership is
///   therefore always confirmed by bytes, never inferred from a path;
/// * without `written` every changed file is undone (the caller's own
///   compare-and-swap, like Source's status-directory restore).
///
/// `lock` proves the Mission write lock is held: every read, comparison and
/// rewrite below runs inside that hold, as in the reference.
pub fn restore_mission_write_scope(
    before: &WriteScopeSnapshot,
    mission_dir: &Path,
    lock: &MissionLock,
    written: Option<&WriteLedger>,
    keep: &BTreeSet<PathBuf>,
    keep_under: Option<&Path>,
) -> Result<WriteScopeRestore, WriteLedgerError> {
    // The lock guard is the proof: every read, comparison and rewrite below runs
    // inside this hold, exactly like Source's `with mission_write_lock(...)`.
    let _held = (lock.key(), lock.path());
    let keep_resolved: BTreeSet<PathBuf> = keep.iter().map(|path| resolve_path(path)).collect();
    let keep_root = keep_under.map(resolve_path);
    let mut restore = WriteScopeRestore::default();

    let mut current = Vec::new();
    if mission_dir.exists() {
        collect_scope_files(mission_dir, &mut current)?;
    }
    let mut paths: BTreeSet<PathBuf> = before.keys().cloned().collect();
    paths.extend(current);

    for path in paths {
        let resolved = resolve_path(&path);
        if keep_resolved.contains(&resolved)
            || keep_root
                .as_ref()
                .is_some_and(|root| resolved.starts_with(root))
        {
            continue;
        }
        let original = before.get(&path);
        let now = match fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(source) => {
                restore.skipped.push((path, source.to_string()));
                continue;
            }
        };
        if now.as_ref() == original {
            continue; // already as it was before the attempt
        }
        if let Some(written) = written {
            let recorded = written.bytes_for(&path);
            if recorded.is_none() || recorded.as_ref() != now.as_ref() {
                // Not what this run wrote (changed, created or deleted by another
                // writer): left alone and reported.
                restore.kept.push(path);
                continue;
            }
        }
        if let Err(error) = undo_one_path(&path, original) {
            restore.skipped.push((path, error.to_string()));
        }
    }
    restore.kept.sort();
    Ok(restore)
}

/// Put `path` back to `original`, or remove it when the attempt created it.
///
/// The restored bytes are the pre-run state, not something this run wrote, so the
/// path is dropped from the active ledger — Source does the same so a later
/// restore/report never treats its own rollback as a run write.
fn undo_one_path(path: &Path, original: Option<&Vec<u8>>) -> Result<(), WriteLedgerError> {
    match original {
        None => match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(WriteLedgerError::Kernel(KernelError::Io {
                action: "remove",
                path: path.to_path_buf(),
                source,
            })),
        },
        Some(bytes) => {
            sk_kernel::atomic_write_with_options(
                path,
                bytes,
                sk_kernel::AtomicWriteOptions {
                    create_parents: true,
                },
            )?;
            if let Some(ledger) = active_write_ledger() {
                ledger.forget(path);
            }
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Resolve a path the way Source's `Path.resolve()` does for this use.
///
/// `fs::canonicalize` requires the leaf to exist; Source's `strict=False`
/// resolution resolves the existing prefix and keeps a missing leaf's name. That
/// is reproduced here rather than dropped, so a path another writer deleted still
/// keys to the same canonical entry.
fn resolve_path(path: &Path) -> PathBuf {
    if let Ok(canonical) = fs::canonicalize(path) {
        return canonical;
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => match fs::canonicalize(parent) {
            Ok(parent) => parent.join(name),
            Err(_) => path.to_path_buf(),
        },
        _ => path.to_path_buf(),
    }
}

/// Poison-tolerant lock: one observer panic must not brick the ledger.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sk_kernel::source_writer::{
        SourcePayload, WriteObservation, WriterOperation, observe_writes, write_source,
    };
    use std::sync::Mutex as StdMutex;
    use tempfile::{TempDir, tempdir};

    /// One `(path, notified bytes, bytes already on disk)` observation.
    type Observation = (PathBuf, Vec<u8>, Vec<u8>);
    /// A `(path, bytes)` observation.
    type Written = (PathBuf, Vec<u8>);

    fn init_git(root: &Path) {
        let status = std::process::Command::new("git")
            .args(["init", "--quiet", "--initial-branch=main"])
            .current_dir(root)
            .status()
            .expect("git must be available for the mission-lock fixture");
        assert!(status.success(), "git init failed: {status:?}");
    }

    fn mission_fixture() -> (TempDir, PathBuf) {
        let root = tempdir().expect("temp root");
        init_git(root.path());
        let mission_dir = root.path().join("kitty-specs").join("001-demo");
        fs::create_dir_all(&mission_dir).expect("mission dir");
        fs::write(
            mission_dir.join(META_JSON_FILENAME),
            b"{\n  \"mission_slug\": \"001-demo\"\n}\n",
        )
        .expect("meta.json");
        (root, mission_dir)
    }

    // --- 1. real status write notification payload equals written bytes ---------

    #[test]
    fn status_append_notifies_the_exact_published_bytes_after_publication() {
        let dir = tempdir().expect("temp dir");
        let log = dir.path().join("status.events.jsonl");
        let observed: Arc<StdMutex<Vec<Observation>>> = Arc::new(StdMutex::new(Vec::new()));
        let sink = Arc::clone(&observed);
        let observer_log = log.clone();
        // The observer reads the file at notify time: that proves the notification
        // happens AFTER the replacement landed, not instead of it.
        let token = observe_writes(move |path: &Path, bytes: &[u8]| {
            let on_disk = fs::read(&observer_log).unwrap_or_default();
            sink.lock()
                .expect("sink")
                .push((path.to_path_buf(), bytes.to_vec(), on_disk));
            Ok(())
        });

        let rows = vec![json!({"event_id": "01HXYZ", "wp_id": "WP01", "to_lane": "claimed"})];
        let (outcome, notification) =
            append_status_rows_observed(&log, &rows, &AppendOptions::default())
                .expect("append must succeed");
        sk_kernel::stop_observing_writes(&token).expect("stop observer");

        let disk = fs::read(&log).expect("log on disk");
        assert_eq!(outcome.rows_appended, 1);
        assert_eq!(outcome.written_bytes as usize, disk.len());
        let notification = notification.expect("a notified append");
        assert!(notification.notified);
        assert_eq!(notification.bytes, disk.len());
        assert_eq!(notification.source, ObservationSource::ReadbackVerified);

        let recorded = observed.lock().expect("observed");
        assert_eq!(recorded.len(), 1, "exactly one notification: {recorded:?}");
        let (path, payload, on_disk) = &recorded[0];
        assert_eq!(path, &log);
        assert_eq!(payload, &disk, "payload must equal the written bytes");
        assert_eq!(
            on_disk, &disk,
            "the file already held the bytes when notified"
        );
    }

    #[test]
    fn an_empty_status_append_publishes_and_notifies_nothing() {
        let dir = tempdir().expect("temp dir");
        let log = dir.path().join("status.events.jsonl");
        let hits = Arc::new(StdMutex::new(0usize));
        let sink = Arc::clone(&hits);
        let token = observe_writes(move |_path: &Path, _bytes: &[u8]| {
            *sink.lock().expect("sink") += 1;
            Ok(())
        });
        let (outcome, notification) =
            append_status_rows_observed(&log, &[], &AppendOptions::default())
                .expect("no-op append");
        sk_kernel::stop_observing_writes(&token).expect("stop observer");
        assert_eq!(outcome.rows_appended, 0);
        assert!(notification.is_none());
        assert!(!log.exists());
        assert_eq!(*hits.lock().expect("hits"), 0);
    }

    // --- 2. nested observer ordering / ledger scopes ----------------------------

    #[test]
    fn nested_observers_run_outer_first_and_nested_ledgers_both_record() {
        let order: Arc<StdMutex<Vec<&'static str>>> = Arc::new(StdMutex::new(Vec::new()));
        let outer_order = Arc::clone(&order);
        let outer = observe_writes(move |_path: &Path, _bytes: &[u8]| {
            outer_order.lock().expect("order").push("outer");
            Ok(())
        });

        let outer_ledger = begin_write_ledger();
        let inner_ledger = begin_write_ledger();
        assert!(
            Arc::ptr_eq(
                &active_write_ledger().expect("an active ledger"),
                inner_ledger.ledger()
            ),
            "the innermost scope is the active ledger"
        );
        let order_sink = Arc::clone(&order);
        let inner = observe_writes(move |_path: &Path, _bytes: &[u8]| {
            order_sink.lock().expect("order").push("inner");
            Ok(())
        });

        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("nested.txt");
        assert!(
            notify_published(&path, b"nested").expect("notify"),
            "a chain was installed"
        );

        assert_eq!(
            *order.lock().expect("order"),
            vec!["outer", "inner"],
            "the reference chain calls the captured outer observer first"
        );
        assert_eq!(
            outer_ledger.ledger().bytes_for(&path).as_deref(),
            Some(&b"nested"[..])
        );
        assert_eq!(
            inner_ledger.ledger().bytes_for(&path).as_deref(),
            Some(&b"nested"[..])
        );

        sk_kernel::stop_observing_writes(&inner).expect("stop inner");
        sk_kernel::stop_observing_writes(&outer).expect("stop outer");
        inner_ledger.finish();
        outer_ledger.finish();
        assert!(active_write_ledger().is_none());
    }

    #[test]
    fn a_ledger_scope_stops_recording_when_it_finishes() {
        let session = begin_write_ledger();
        let path = Path::new("/tmp/cf5-ledger-scope.txt");
        notify_published(path, b"first").expect("notify");
        let ledger = Arc::clone(session.ledger());
        session.finish();
        notify_published(path, b"second").expect("notify after finish");
        assert_eq!(ledger.len(), 1);
        assert_eq!(ledger.bytes_for(path).as_deref(), Some(&b"first"[..]));
    }

    // --- 3. mission-number bake metadata capture --------------------------------

    #[test]
    fn bake_records_the_real_published_meta_bytes_and_notifies_them() {
        let (_root, mission_dir) = mission_fixture();
        let meta_path = mission_dir.join(META_JSON_FILENAME);
        let original = fs::read(&meta_path).expect("original meta");

        let seen: Arc<StdMutex<Vec<Written>>> = Arc::new(StdMutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let token = observe_writes(move |path: &Path, bytes: &[u8]| {
            if path.file_name().and_then(|name| name.to_str()) == Some(META_JSON_FILENAME) {
                sink.lock()
                    .expect("sink")
                    .push((path.to_path_buf(), bytes.to_vec()));
            }
            Ok(())
        });

        let capture = bake_mission_number(&mission_dir, 7).expect("the bake must succeed");
        sk_kernel::stop_observing_writes(&token).expect("stop observer");

        let disk = fs::read(&meta_path).expect("baked meta");
        assert_eq!(capture.path, meta_path);
        assert_eq!(capture.before.as_deref(), Some(&original[..]));
        assert_eq!(capture.written, disk, "the capture is the published bytes");
        assert_ne!(capture.before.as_deref(), Some(&disk[..]));
        assert_eq!(capture.source, ObservationSource::Observer);

        let value: Value = serde_json::from_slice(&capture.written).expect("valid JSON");
        assert_eq!(value.get("mission_number"), Some(&json!(7)));
        assert_eq!(
            value.get("mission_slug"),
            Some(&json!("001-demo")),
            "every other field is preserved"
        );

        let seen = seen.lock().expect("seen");
        assert_eq!(seen.len(), 1, "one meta.json notification: {seen:?}");
        assert_eq!(seen[0].0, meta_path);
        assert_eq!(seen[0].1, disk);
    }

    #[test]
    fn undo_meta_bake_restores_only_while_the_file_still_holds_the_baked_bytes() {
        let (_root, mission_dir) = mission_fixture();
        let meta_path = mission_dir.join(META_JSON_FILENAME);
        let original = fs::read(&meta_path).expect("original meta");

        let capture = bake_mission_number(&mission_dir, 3).expect("bake");
        assert_eq!(
            undo_meta_bake(&capture).expect("undo"),
            BakeUndoOutcome::Restored
        );
        assert_eq!(fs::read(&meta_path).expect("restored"), original);
        assert_eq!(
            undo_meta_bake(&capture).expect("second undo"),
            BakeUndoOutcome::AlreadyBefore,
            "a second undo is a no-op, not a second write"
        );

        let capture = bake_mission_number(&mission_dir, 4).expect("bake again");
        let mut value: Value = serde_json::from_slice(&capture.written).expect("baked JSON");
        value["status"] = json!("operator edit");
        let mut foreign = serde_json::to_vec_pretty(&value).expect("render");
        foreign.push(b'\n');
        fs::write(&meta_path, &foreign).expect("foreign edit");
        assert_eq!(
            undo_meta_bake(&capture).expect("undo after edit"),
            BakeUndoOutcome::ChangedByAnotherWriter,
            "a concurrent field is never erased by the bake undo"
        );
        assert_eq!(fs::read(&meta_path).expect("foreign kept"), foreign);
    }

    #[test]
    fn materialize_notifies_only_a_real_snapshot_change() {
        let feature_dir = tempdir().expect("temp dir");
        let snapshot_path = feature_dir
            .path()
            .join(sk_status::snapshot::SNAPSHOT_FILENAME);
        let seen: Arc<StdMutex<Vec<Written>>> = Arc::new(StdMutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let token = observe_writes(move |path: &Path, bytes: &[u8]| {
            sink.lock()
                .expect("sink")
                .push((path.to_path_buf(), bytes.to_vec()));
            Ok(())
        });

        let first = materialize_status_snapshot_observed(feature_dir.path())
            .expect("first materialization")
            .expect("the first write replaced the file");
        assert!(first.notified);
        assert_eq!(first.source, ObservationSource::ReadbackVerified);
        let second = materialize_status_snapshot_observed(feature_dir.path())
            .expect("second materialization");
        assert!(second.is_none(), "a no-op materialization notifies nothing");
        sk_kernel::stop_observing_writes(&token).expect("stop observer");

        let disk = fs::read(&snapshot_path).expect("snapshot on disk");
        let seen = seen.lock().expect("seen");
        assert_eq!(seen.len(), 1, "one replacement, one notification: {seen:?}");
        assert_eq!(seen[0].0, snapshot_path);
        assert_eq!(seen[0].1, disk);
        assert_eq!(first.bytes, disk.len());
    }

    // --- 4. finalize owned-byte recovery / foreign-content preservation ---------

    #[test]
    fn restore_reverts_owned_bytes_deletes_own_creations_and_keeps_foreign_content() {
        let (root, mission_dir) = mission_fixture();
        let owned = mission_dir.join("tasks.md");
        let created = mission_dir.join("issue-matrix.md");
        let foreign_created = mission_dir.join("foreign.md");
        let rewritten = mission_dir.join("spec.md");
        let nested_dir = mission_dir.join("tasks");
        fs::create_dir_all(&nested_dir).expect("tasks dir");
        let nested = nested_dir.join("WP01.md");
        fs::write(&owned, b"original tasks\n").expect("seed tasks");
        fs::write(&rewritten, b"original spec\n").expect("seed spec");
        fs::write(&nested, b"original wp\n").expect("seed wp");

        let before = snapshot_mission_write_scope(&mission_dir).expect("snapshot");
        assert!(before.contains_key(&owned));
        assert!(before.contains_key(&nested));
        assert!(!before.contains_key(&mission_dir.join(META_JSON_FILENAME)));

        let session = begin_write_ledger();
        // Finalize-owned rewrite: written and announced at the owner boundary.
        sk_kernel::atomic_write(&owned, b"finalize tasks\n").expect("write tasks");
        notify_published(&owned, b"finalize tasks\n").expect("notify tasks");
        sk_kernel::atomic_write(&nested, b"finalize wp\n").expect("write wp");
        notify_published(&nested, b"finalize wp\n").expect("notify wp");
        // Finalize-owned creation.
        sk_kernel::atomic_write(&created, b"finalize matrix\n").expect("write matrix");
        notify_published(&created, b"finalize matrix\n").expect("notify matrix");
        // A foreign writer's creation (never announced, never in the ledger).
        sk_kernel::atomic_write(&foreign_created, b"foreign only\n").expect("write foreign");
        // This run's rewrite, then another writer's later edit of the same file.
        sk_kernel::atomic_write(&rewritten, b"finalize spec\n").expect("write spec");
        notify_published(&rewritten, b"finalize spec\n").expect("notify spec");
        sk_kernel::atomic_write(&rewritten, b"foreign spec edit\n").expect("foreign edit");
        let ledger = Arc::clone(session.ledger());
        session.finish();

        let restore = restore_mission_write_scope_locked(
            &before,
            &mission_dir,
            root.path(),
            Some(&ledger),
            &BTreeSet::new(),
            None,
        )
        .expect("restore must run");

        assert_eq!(
            fs::read(&owned).expect("tasks restored"),
            b"original tasks\n"
        );
        assert_eq!(fs::read(&nested).expect("wp restored"), b"original wp\n");
        assert!(!created.exists(), "a file this run created is removed");
        assert_eq!(
            fs::read(&foreign_created).expect("foreign kept"),
            b"foreign only\n"
        );
        assert_eq!(
            fs::read(&rewritten).expect("foreign edit kept"),
            b"foreign spec edit\n"
        );
        assert!(restore.kept.contains(&foreign_created));
        assert!(restore.kept.contains(&rewritten));
        assert!(!restore.kept.contains(&owned));
        assert!(restore.skipped.is_empty(), "no path skipped: {restore:?}");
    }

    #[test]
    fn restore_leaves_a_file_another_writer_deleted_alone() {
        let (root, mission_dir) = mission_fixture();
        let owned = mission_dir.join("tasks.md");
        fs::write(&owned, b"original\n").expect("seed");
        let before = snapshot_mission_write_scope(&mission_dir).expect("snapshot");

        let session = begin_write_ledger();
        sk_kernel::atomic_write(&owned, b"finalize\n").expect("write");
        notify_published(&owned, b"finalize\n").expect("notify");
        let ledger = Arc::clone(session.ledger());
        session.finish();
        fs::remove_file(&owned).expect("foreign delete");

        let restore = restore_mission_write_scope_locked(
            &before,
            &mission_dir,
            root.path(),
            Some(&ledger),
            &BTreeSet::new(),
            None,
        )
        .expect("restore must run");
        assert!(!owned.exists(), "a foreign delete is not undone");
        assert!(restore.kept.contains(&owned));
    }

    #[test]
    fn restore_leaves_a_kept_status_path_alone() {
        let (root, mission_dir) = mission_fixture();
        let status_file = mission_dir.join("status.json");
        fs::write(&status_file, b"original\n").expect("seed");
        let before = snapshot_mission_write_scope(&mission_dir).expect("snapshot");

        let session = begin_write_ledger();
        sk_kernel::atomic_write(&status_file, b"kept commit\n").expect("write");
        notify_published(&status_file, b"kept commit\n").expect("notify");
        let ledger = Arc::clone(session.ledger());
        session.finish();

        let mut keep = BTreeSet::new();
        keep.insert(fs::canonicalize(&status_file).expect("canonical"));
        let restore = restore_mission_write_scope_locked(
            &before,
            &mission_dir,
            root.path(),
            Some(&ledger),
            &keep,
            None,
        )
        .expect("restore must run");
        assert_eq!(fs::read(&status_file).expect("kept"), b"kept commit\n");
        assert!(restore.kept.is_empty());
    }

    // --- 5. no listener / typed observer errors --------------------------------z

    #[test]
    fn notify_without_an_observer_reports_no_listener() {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("nothing.txt");
        assert!(!notify_published(&path, b"bytes").expect("no chain is not a failure"));
        // The canonical writer reports the same state instead of pretending a
        // notification happened.
        let receipt = write_source(
            WriterOperation::atomic(),
            &path,
            SourcePayload::Bytes(b"bytes"),
            None,
        )
        .expect("write");
        assert_eq!(receipt.observation, WriteObservation::NoListener);
    }

    #[test]
    fn a_typed_observer_failure_propagates_after_the_write_landed() {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("observed.txt");
        let token = observe_writes(|_path: &Path, _bytes: &[u8]| {
            Err(WriteObserverError::os(std::io::Error::other(
                "observer disk full",
            )))
        });
        let error = notify_published(&path, b"payload").expect_err("the observer refuses");
        sk_kernel::stop_observing_writes(&token).expect("stop observer");
        assert!(error.published_before_error());
        match error {
            WriteLedgerError::Observer {
                path: failed,
                source,
            } => {
                assert_eq!(failed, path);
                assert!(source.is_os_error(), "the OSError class is preserved");
                assert_eq!(source.message(), "observer disk full");
            }
            other => panic!("expected a typed observer failure, got {other:?}"),
        }
    }

    #[test]
    fn ledger_keeps_the_latest_write_per_resolved_path() {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("latest.txt");
        let ledger = WriteLedger::new();
        ledger.record(&path, b"first");
        ledger.record(&path, b"second");
        assert_eq!(ledger.len(), 1);
        assert_eq!(ledger.bytes_for(&path).as_deref(), Some(&b"second"[..]));
        assert_eq!(ledger.text_for(&path).as_deref(), Some("second"));
        ledger.forget(&path);
        assert!(ledger.is_empty());
        assert!(ledger.bytes_for(&path).is_none());
    }
}
