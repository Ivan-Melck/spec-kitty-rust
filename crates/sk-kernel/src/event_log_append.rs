//! Locked batch append for append-only JSONL event logs.
//!
//! Upstream `kernel.event_log_append` is an interim writer while the shared
//! event write service takes ownership of each log. The Rust kernel provides
//! the same contract as a foundation primitive so native writers do not invent
//! a second uncoordinated append protocol.
//!
//! # Exact protocol
//!
//! 1. **Anchor and lock path.** The anchor is the log itself, or
//!    [`EventLogAppendOptions::lock_anchor`] for a one-file-per-record family
//!    directory. [`event_log_lock_path`] places the lock at
//!    `<git-common-dir>/spec-kitty-locks/event-logs/<sha256-prefix>.lock`
//!    (32 hex characters over the resolved, case-normalised anchor identity).
//!    The Git common dir is discovered by reading `.git` and the worktree
//!    `commondir` file; **no Git subprocess runs**, so a lock is never taken
//!    while a child process is being spawned and no child can inherit an
//!    ambiguous lock authority. Outside a Git repository the lock is
//!    `<anchor>.event-log.lock`. An unreadable repository topology refuses the
//!    append with [`KernelError::LockUnavailable`] rather than locking where
//!    other writers would not.
//! 2. **Exclusive lock.** One [`FileLock`] with the caller's bounded timeout
//!    (default [`EVENT_LOG_LOCK_TIMEOUT`]). Reentrancy is per thread, matching
//!    the upstream `event_log_lock` context manager, so a writer may hold the
//!    lock across a loop of appends.
//! 3. **Torn-tail policy.** Under the lock, the log's last byte must be `\n`.
//!    The policy is **explicit per log**, never implicit repair:
//!    [`TornTailPolicy::Refuse`] writes nothing and returns
//!    [`KernelError::TornTail`]; [`TornTailPolicy::TruncateToLastNewline`]
//!    cuts the incomplete tail (to the last `\n`, or to zero when none exists)
//!    and fsyncs before appending. The repair itself is not rolled back.
//! 4. **Optional precondition.** If supplied, the current bytes (bounded by
//!    [`EventLogAppendOptions::max_precondition_bytes`]) are handed to the
//!    closure while the lock is held. A refusal propagates and nothing is
//!    written. The closure sees the post-torn-tail-repair bytes.
//! 5. **One unit write.** Every line is validated to contain no `\n` or `\r`;
//!    the batch is encoded as `line\n` per line and written at the current end
//!    of file in one logical unit. The log is opened `O_APPEND` for
//!    interoperability with writers that rely on append semantics, but the
//!    lock is what serialises writers.
//! 6. **Durability and readback.** The unit is `sync_data`ed, then read back
//!    from the pre-append offset and compared byte-for-byte. The unit is
//!    accepted only after a successful readback.
//! 7. **Rollback.** A short write, write error, sync error, readback error, or
//!    readback mismatch truncates the log back to its pre-append size and
//!    fsyncs before returning the typed failure. If the rollback itself fails,
//!    [`KernelError::AppendRollbackFailed`] reports both failures; the log is
//!    then explicitly left in a state that the next writer's torn-tail policy
//!    must handle.
//!
//! The module validates line framing only, never row schema. The event write
//! service remains the validation authority for `status.events.jsonl` and
//! `canonical-events.jsonl`; those logs keep their own Mission status lock and
//! are not appended through this primitive.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::KernelError;
use crate::fs_resolve::{map_resolve_failure, resolve_non_strict};
use crate::locks::{FileLock, anchored_lock_path};

/// Directory under `<git-common-dir>/spec-kitty-locks/` holding per-log locks.
pub const EVENT_LOG_LOCK_SUBDIRECTORY: &str = "event-logs";

/// Lock-file suffix beside the anchor outside a Git repository.
pub const NON_GIT_LOCK_SUFFIX: &str = ".event-log.lock";

/// Default bounded wait for the event-log lock.
pub const EVENT_LOG_LOCK_TIMEOUT: Duration = Duration::from_secs(10);

/// Default bound on bytes read for a precondition check.
pub const DEFAULT_MAX_PRECONDITION_BYTES: u64 = 8 * 1024 * 1024;

const TORN_TAIL_SCAN_CHUNK: usize = 8 * 1024;

/// Borrowed precondition closure evaluated under the event-log lock.
type Precondition<'a> = &'a mut dyn FnMut(&[u8]) -> Result<(), KernelError>;

/// What to do when an event log's final line is not newline-terminated.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TornTailPolicy {
    /// Write nothing and return [`KernelError::TornTail`]. The default.
    #[default]
    Refuse,
    /// Cut the incomplete tail back to its last newline (or zero), fsync, then
    /// append. This is an explicit, destructive per-log choice.
    TruncateToLastNewline,
}

/// Options for one event-log append.
///
/// The defaults mirror upstream: create the log, do not create missing
/// parents, follow a final symlink unless `no_follow` is set, use `0o666`
/// before the umask on Unix, refuse a torn tail, wait up to
/// [`EVENT_LOG_LOCK_TIMEOUT`], and bound precondition reads at
/// [`DEFAULT_MAX_PRECONDITION_BYTES`].
#[derive(Clone, Debug)]
pub struct EventLogAppendOptions {
    /// Family-directory anchor for one-file-per-record logs; `None` means the
    /// log itself.
    pub lock_anchor: Option<PathBuf>,
    /// Create the log when it is missing.
    pub create: bool,
    /// Create missing parent directories before placing the lock.
    pub create_parents: bool,
    /// Refuse a final-component symlink at the log path.
    pub no_follow: bool,
    /// Permission bits for a newly created log on Unix (before the umask).
    pub mode: u32,
    /// Explicit per-log torn-tail policy.
    pub torn_tail: TornTailPolicy,
    /// Bounded wait for the event-log lock.
    pub timeout: Duration,
    /// Maximum bytes handed to a precondition closure.
    pub max_precondition_bytes: u64,
    /// Sync the parent directory after creating a new log, so the log's
    /// directory entry is durable before the first append is acknowledged.
    /// Unix only; Windows cannot express a directory fsync through safe
    /// `std::fs`. Deliberate safer divergence from the Python baseline, which
    /// does not sync the new entry.
    pub sync_new_log_directory: bool,
}

impl Default for EventLogAppendOptions {
    fn default() -> Self {
        Self {
            lock_anchor: None,
            create: true,
            create_parents: false,
            no_follow: false,
            mode: 0o666,
            torn_tail: TornTailPolicy::Refuse,
            timeout: EVENT_LOG_LOCK_TIMEOUT,
            max_precondition_bytes: DEFAULT_MAX_PRECONDITION_BYTES,
            sync_new_log_directory: true,
        }
    }
}

/// Append pre-serialised rows as one locked, durable unit.
pub fn append_event_log_lines<S: AsRef<str>>(
    log: &Path,
    lines: &[S],
    options: &EventLogAppendOptions,
) -> Result<(), KernelError> {
    append_impl(log, lines, options, None)
}

/// Append with a precondition evaluated under the lock.
///
/// The closure receives the log's existing bytes; returning an error refuses
/// the append and propagates that error unchanged.
pub fn append_event_log_lines_guarded<S, F>(
    log: &Path,
    lines: &[S],
    options: &EventLogAppendOptions,
    precondition: F,
) -> Result<(), KernelError>
where
    S: AsRef<str>,
    F: FnMut(&[u8]) -> Result<(), KernelError>,
{
    let mut precondition = precondition;
    append_impl(log, lines, options, Some(&mut precondition))
}

/// Return the lock file every append anchored on `anchor` takes.
///
/// This never runs Git. A missing `.git` marker uses the sibling fallback; a
/// marker that exists but cannot be read or resolved is a hard refusal.
pub fn event_log_lock_path(anchor: &Path) -> Result<PathBuf, KernelError> {
    let resolved =
        resolve_non_strict(anchor).map_err(|failure| map_resolve_failure(anchor, failure))?;
    let search_from = resolved.parent().unwrap_or_else(|| Path::new("."));
    match crate::git_topology::strict_git_common_dir(search_from)? {
        Some(common_dir) if !resolved.starts_with(&common_dir) => anchored_lock_path(
            &resolved,
            &common_dir,
            Some(Path::new(EVENT_LOG_LOCK_SUBDIRECTORY)),
        ),
        // Upstream treats a path inside the common dir as "not a valid project
        // root" and falls back to the sibling lock; never place a lock inside
        // `.git` merely because the anchor lives there.
        Some(_) | None => sibling_lock_path(&resolved, anchor),
    }
}

fn sibling_lock_path(resolved: &Path, anchor: &Path) -> Result<PathBuf, KernelError> {
    let Some(name) = resolved.file_name() else {
        return Err(KernelError::LockUnavailable {
            path: anchor.to_path_buf(),
            reason: "the anchor has no file name for a sibling lock".to_owned(),
        });
    };
    let mut sibling = OsString::from(name);
    sibling.push(NON_GIT_LOCK_SUFFIX);
    Ok(resolved.with_file_name(sibling))
}

fn append_impl<S: AsRef<str>>(
    log: &Path,
    lines: &[S],
    options: &EventLogAppendOptions,
    mut precondition: Option<Precondition<'_>>,
) -> Result<(), KernelError> {
    let payload = encode_unit(lines)?;
    prepare_parent(log, options.create_parents)?;

    let anchor = options.lock_anchor.as_deref().unwrap_or(log);
    let lock_path = event_log_lock_path(anchor)?;
    let _lock = FileLock::acquire(&lock_path, options.timeout)?;

    let existed_before = fs::symlink_metadata(log).is_ok();
    let mut target = open_log(log, options)?;
    if !existed_before && options.sync_new_log_directory {
        sync_log_parent(log)?;
    }
    let pre_size = torn_tail_size(&mut target, log, options.torn_tail)?;

    if let Some(precondition) = precondition.as_mut() {
        let existing = read_whole(&mut target, log, options.max_precondition_bytes)?;
        precondition(&existing)?;
        target
            .seek_to(pre_size)
            .map_err(|source| KernelError::io("seek", log, source))?;
    }

    persist_unit(&mut target, log, &payload, pre_size)
}

fn encode_unit<S: AsRef<str>>(lines: &[S]) -> Result<Vec<u8>, KernelError> {
    if lines.is_empty() {
        return Err(KernelError::InvalidLogLine {
            reason: "the append unit is empty".to_owned(),
        });
    }
    let mut payload = Vec::new();
    for line in lines {
        let line = line.as_ref();
        if line.contains('\n') || line.contains('\r') {
            return Err(KernelError::InvalidLogLine {
                reason: "an event-log line must not contain a line terminator".to_owned(),
            });
        }
        payload.extend_from_slice(line.as_bytes());
        payload.push(b'\n');
    }
    Ok(payload)
}

fn prepare_parent(log: &Path, create_parents: bool) -> Result<(), KernelError> {
    let parent = parent_directory(log);
    if create_parents {
        fs::create_dir_all(parent)
            .map_err(|source| KernelError::io("create event-log parent", parent, source))?;
        return Ok(());
    }
    match fs::metadata(parent) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(KernelError::io(
            "open event-log parent",
            parent,
            io::Error::new(io::ErrorKind::NotADirectory, "parent is not a directory"),
        )),
        Err(source) => Err(KernelError::io("open event-log parent", parent, source)),
    }
}

fn parent_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

/// Make a newly created log's directory entry durable.
#[cfg(unix)]
fn sync_log_parent(log: &Path) -> Result<(), KernelError> {
    let parent = parent_directory(log);
    let directory = File::open(parent)
        .map_err(|source| KernelError::io("open event-log parent directory of", log, source))?;
    directory
        .sync_all()
        .map_err(|source| KernelError::io("sync event-log parent directory of", log, source))
}

/// Windows cannot open a directory as a `File`, so the creation durability
/// claim is explicitly platform-limited (see the crate README).
#[cfg(not(unix))]
fn sync_log_parent(_log: &Path) -> Result<(), KernelError> {
    Ok(())
}

fn open_log(log: &Path, options: &EventLogAppendOptions) -> Result<File, KernelError> {
    let mut open = OpenOptions::new();
    open.read(true).append(true).create(options.create);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open.mode(options.mode);
    }
    let file = if options.no_follow {
        crate::no_follow::open_no_follow(log, &mut open)?
    } else {
        open.open(log)
            .map_err(|source| KernelError::io("open event log", log, source))?
    };
    let metadata = file
        .metadata()
        .map_err(|source| KernelError::io("inspect event log", log, source))?;
    if !metadata.is_file() {
        return Err(KernelError::io(
            "open event log",
            log,
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "event log is not a regular file",
            ),
        ));
    }
    Ok(file)
}

fn torn_tail_size<T: AppendTarget>(
    target: &mut T,
    log: &Path,
    policy: TornTailPolicy,
) -> Result<u64, KernelError> {
    let size = target
        .seek_to(0)
        .and_then(|_| target.target_len())
        .map_err(|source| KernelError::io("inspect event log", log, source))?;
    if size == 0 {
        return Ok(0);
    }
    let last = read_last_byte(target, size)
        .map_err(|source| KernelError::io("read event-log tail", log, source))?;
    if last == Some(b'\n') {
        return Ok(size);
    }
    match policy {
        TornTailPolicy::Refuse => Err(KernelError::TornTail {
            path: log.to_path_buf(),
        }),
        TornTailPolicy::TruncateToLastNewline => truncate_to_last_newline(target, log),
    }
}

fn read_last_byte<T: AppendTarget>(target: &mut T, size: u64) -> io::Result<Option<u8>> {
    target.seek_to(size - 1)?;
    let mut byte = [0_u8; 1];
    let read = target.read_some(&mut byte)?;
    Ok((read == 1).then_some(byte[0]))
}

fn truncate_to_last_newline<T: AppendTarget>(
    target: &mut T,
    log: &Path,
) -> Result<u64, KernelError> {
    let size = target
        .target_len()
        .map_err(|source| KernelError::io("inspect event log", log, source))?;
    let mut end = size;
    let mut buffer = vec![0_u8; TORN_TAIL_SCAN_CHUNK];
    while end > 0 {
        let start = end.saturating_sub(TORN_TAIL_SCAN_CHUNK as u64);
        let length = (end - start) as usize;
        target
            .seek_to(start)
            .map_err(|source| KernelError::io("seek event-log tail", log, source))?;
        let mut filled = 0;
        while filled < length {
            let read = target
                .read_some(&mut buffer[filled..length])
                .map_err(|source| KernelError::io("read event-log tail", log, source))?;
            if read == 0 {
                break;
            }
            filled += read;
        }
        if let Some(index) = buffer[..filled].iter().rposition(|byte| *byte == b'\n') {
            let new_size = start + index as u64 + 1;
            target
                .truncate(new_size)
                .and_then(|_| target.sync_data())
                .map_err(|source| KernelError::io("repair torn event-log tail", log, source))?;
            return Ok(new_size);
        }
        end = start;
    }
    target
        .truncate(0)
        .and_then(|_| target.sync_data())
        .map_err(|source| KernelError::io("repair torn event-log tail", log, source))?;
    Ok(0)
}

fn read_whole<T: AppendTarget>(
    target: &mut T,
    log: &Path,
    max_bytes: u64,
) -> Result<Vec<u8>, KernelError> {
    let size = target
        .target_len()
        .map_err(|source| KernelError::io("inspect event log", log, source))?;
    if size > max_bytes {
        return Err(KernelError::TooLarge {
            path: log.to_path_buf(),
            limit: max_bytes,
        });
    }
    target
        .seek_to(0)
        .map_err(|source| KernelError::io("seek event log", log, source))?;
    let mut bytes = vec![0_u8; size as usize];
    let mut filled = 0;
    while filled < bytes.len() {
        let read = target
            .read_some(&mut bytes[filled..])
            .map_err(|source| KernelError::io("read event log", log, source))?;
        if read == 0 {
            break;
        }
        filled += read;
    }
    bytes.truncate(filled);
    Ok(bytes)
}

fn persist_unit<T: AppendTarget>(
    target: &mut T,
    log: &Path,
    payload: &[u8],
    pre_size: u64,
) -> Result<(), KernelError> {
    let result = write_unit(target, log, payload, pre_size);
    match result {
        Ok(()) => Ok(()),
        Err(cause) => rollback(target, log, pre_size, cause),
    }
}

fn write_unit<T: AppendTarget>(
    target: &mut T,
    log: &Path,
    payload: &[u8],
    pre_size: u64,
) -> Result<(), KernelError> {
    target
        .seek_to(pre_size)
        .map_err(|source| KernelError::io("seek event-log append", log, source))?;
    let mut written = 0;
    while written < payload.len() {
        let count = target
            .write_some(&payload[written..])
            .map_err(|source| KernelError::io("append to", log, source))?;
        if count == 0 {
            return Err(KernelError::ShortWrite {
                path: log.to_path_buf(),
                expected: payload.len(),
                written,
            });
        }
        written += count;
    }

    target
        .sync_data()
        .map_err(|source| KernelError::io("sync appended", log, source))?;
    target
        .seek_to(pre_size)
        .map_err(|source| KernelError::io("seek event-log readback", log, source))?;

    let mut readback = vec![0_u8; payload.len()];
    let mut filled = 0;
    while filled < readback.len() {
        let count = target
            .read_some(&mut readback[filled..])
            .map_err(|source| KernelError::io("read back", log, source))?;
        if count == 0 {
            break;
        }
        filled += count;
    }
    if filled != payload.len() || readback[..filled] != *payload {
        return Err(KernelError::ReadbackMismatch {
            path: log.to_path_buf(),
        });
    }
    Ok(())
}

fn rollback<T: AppendTarget>(
    target: &mut T,
    log: &Path,
    pre_size: u64,
    cause: KernelError,
) -> Result<(), KernelError> {
    let rollback = target
        .truncate(pre_size)
        .and_then(|_| target.sync_data())
        .and_then(|_| target.seek_to(pre_size).map(|_| ()));
    match rollback {
        Ok(()) => Err(cause),
        Err(error) => Err(KernelError::AppendRollbackFailed {
            path: log.to_path_buf(),
            reason: format!("{cause}; rollback failed: {error}"),
        }),
    }
}

trait AppendTarget {
    fn target_len(&mut self) -> io::Result<u64>;
    fn seek_to(&mut self, position: u64) -> io::Result<u64>;
    fn write_some(&mut self, bytes: &[u8]) -> io::Result<usize>;
    fn read_some(&mut self, bytes: &mut [u8]) -> io::Result<usize>;
    fn truncate(&mut self, size: u64) -> io::Result<()>;
    fn sync_data(&mut self) -> io::Result<()>;
}

impl AppendTarget for File {
    fn target_len(&mut self) -> io::Result<u64> {
        self.metadata().map(|metadata| metadata.len())
    }

    fn seek_to(&mut self, position: u64) -> io::Result<u64> {
        self.seek(SeekFrom::Start(position))
    }

    fn write_some(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.write(bytes)
    }

    fn read_some(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.read(bytes)
    }

    fn truncate(&mut self, size: u64) -> io::Result<()> {
        self.set_len(size)
    }

    fn sync_data(&mut self) -> io::Result<()> {
        File::sync_data(self)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io;

    use super::*;

    struct Faulty {
        file: File,
        fail_after: Option<usize>,
        written: usize,
        corrupt_readback: bool,
        fail_truncate: bool,
    }

    impl Faulty {
        fn new(file: File) -> Self {
            Self {
                file,
                fail_after: None,
                written: 0,
                corrupt_readback: false,
                fail_truncate: false,
            }
        }
    }

    impl AppendTarget for Faulty {
        fn target_len(&mut self) -> io::Result<u64> {
            self.file.metadata().map(|metadata| metadata.len())
        }

        fn seek_to(&mut self, position: u64) -> io::Result<u64> {
            self.file.seek(SeekFrom::Start(position))
        }

        fn write_some(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if let Some(limit) = self.fail_after
                && self.written >= limit
            {
                return Err(io::Error::new(
                    io::ErrorKind::StorageFull,
                    "injected ENOSPC",
                ));
            }
            let allowed = self
                .fail_after
                .map_or(bytes.len(), |limit| (limit - self.written).min(bytes.len()));
            let count = self.file.write(&bytes[..allowed])?;
            self.written += count;
            Ok(count)
        }

        fn read_some(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            if self.corrupt_readback {
                let count = bytes.len().min(4);
                bytes[..count].fill(b'X');
                return Ok(count);
            }
            self.file.read(bytes)
        }

        fn truncate(&mut self, size: u64) -> io::Result<()> {
            if self.fail_truncate {
                return Err(io::Error::other("injected truncate failure"));
            }
            self.file.set_len(size)
        }

        fn sync_data(&mut self) -> io::Result<()> {
            File::sync_data(&self.file)
        }
    }

    fn append_file(path: &Path) -> File {
        OpenOptions::new()
            .read(true)
            .append(true)
            .open(path)
            .expect("open append")
    }

    #[test]
    fn short_write_rolls_back_to_the_pre_append_size() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("log");
        fs::write(&path, b"good\n").unwrap();
        let mut target = Faulty::new(append_file(&path));
        target.fail_after = Some(3);
        let pre_size = target.target_len().unwrap();

        let error = persist_unit(&mut target, &path, b"injected\n", pre_size).unwrap_err();
        assert!(matches!(error, KernelError::Io { .. }), "{error}");
        assert_eq!(fs::read(&path).unwrap(), b"good\n");
    }

    #[test]
    fn readback_mismatch_rolls_back_to_the_pre_append_size() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("log");
        fs::write(&path, b"good\n").unwrap();
        let mut target = Faulty::new(append_file(&path));
        target.corrupt_readback = true;
        let pre_size = target.target_len().unwrap();

        let error = persist_unit(&mut target, &path, b"injected\n", pre_size).unwrap_err();
        assert!(
            matches!(error, KernelError::ReadbackMismatch { .. }),
            "{error}"
        );
        assert_eq!(fs::read(&path).unwrap(), b"good\n");
    }

    #[test]
    fn a_failed_rollback_reports_that_a_mutation_may_have_committed() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("log");
        fs::write(&path, b"good\n").unwrap();
        let mut target = Faulty::new(append_file(&path));
        target.fail_after = Some(3);
        target.fail_truncate = true;
        let pre_size = target.target_len().unwrap();

        let error = persist_unit(&mut target, &path, b"injected\n", pre_size).unwrap_err();
        assert!(
            matches!(error, KernelError::AppendRollbackFailed { .. }),
            "{error}"
        );
        assert!(error.mutation_may_have_committed());
        // Non-vacuity: the injected short write did leave a partial unit on disk.
        assert_eq!(fs::read(&path).unwrap(), b"good\ninj");
    }

    #[test]
    fn successful_unit_is_durable_and_verified() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("log");
        fs::write(&path, b"good\n").unwrap();
        let mut target = Faulty::new(append_file(&path));
        let pre_size = target.target_len().unwrap();

        persist_unit(&mut target, &path, b"next\n", pre_size).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"good\nnext\n");
    }
}
