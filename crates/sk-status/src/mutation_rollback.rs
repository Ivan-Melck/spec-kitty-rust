//! Verified Mission-write rollback: the native counterpart of
//! `specify_cli.status.mission_write`'s `capture_rollback_point` /
//! `appended_event_ids` / `rollback_events_log` / `rollback_status_artifacts`.
//!
//! This is **not** the generic byte-snapshot rollback in
//! [`crate::mutation_log::restore_log_snapshot`]. That primitive restores a
//! single-writer whole-file snapshot and proves only "the file still holds the
//! bytes I wrote". The Mission-write protocol is different and stricter:
//!
//! 1. the capture **requires the registered Mission lock** (passed as
//!    `&MissionLock`, so the hold is a type-level precondition, checked against
//!    the Mission's resolved key);
//! 2. a rollback cuts the log back to the captured size and never extends it;
//! 3. every row after the capture point must be a **whole JSON event row** (a
//!    torn row refuses, it is never partially cut);
//! 4. when the caller knows the event ids it appended, the tail ids must match
//!    them **as a multiset** — a foreign row refuses;
//! 5. no tail row may be **committed at `HEAD`** — a row already in Git refuses
//!    (`HEAD`-unreadable refuses too, fail-closed);
//! 6. a refusal leaves every file byte-identical and is reported as a typed
//!    [`RollbackRefusal`]; only an I/O failure reports "may be partly
//!    rewritten".
//!
//! The committed-at-`HEAD` probe is a Git operation, so it is **injected**
//! (`HeadCommittedProbe`): this crate never runs Git. Passing `None` means the
//! caller cannot prove the tail is uncommitted, which — exactly like the
//! baseline's `HEAD_UNREADABLE` — refuses a non-empty tail rather than cutting
//! it. A caller that can read the committed blob (the Git adapter owner) passes
//! a probe; a caller that cannot gets a safe refusal, never data loss.
//!
//! Indeterminate-write recovery: a write whose outcome is unknown (torn tail,
//! unreadable tail, partially applied replacement) resolves to a refusal with
//! the file preserved. This module never deletes or truncates on ambiguity; the
//! caller inspects and decides.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::mutation::MutationError;
use crate::mutation_lock::{MissionLock, mission_lock_key};
use crate::snapshot::{EVENTS_FILENAME, SNAPSHOT_FILENAME};

/// The prefix of a refusal message the baseline prints (`STATUS_ROLLBACK_REFUSED`).
pub const STATUS_ROLLBACK_REFUSED: &str = "STATUS_ROLLBACK_REFUSED";

/// Why a rollback left the Mission files untouched.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RollbackRefusal {
    /// The event log existed at capture and is gone.
    LogVanished,
    /// The event log is shorter than at capture.
    LogShrank,
    /// The rows after the capture point are not whole JSON event rows.
    TailUnparseable,
    /// The rows after the capture point are not the rows this operation appended.
    TailNotOwned,
    /// The rows after the capture point are already committed at `HEAD`.
    TailAlreadyCommitted,
    /// `HEAD` could not be read to prove the rows are uncommitted.
    HeadUnreadable,
    /// The rollback hit an I/O error before it could finish.
    IoError,
}

impl RollbackRefusal {
    /// The baseline's refusal sentence fragment.
    pub fn as_str(self) -> &'static str {
        match self {
            RollbackRefusal::LogVanished => "the event log existed at capture and is gone",
            RollbackRefusal::LogShrank => "the event log is shorter than at capture",
            RollbackRefusal::TailUnparseable => {
                "the rows after the capture point are not whole JSON event rows"
            }
            RollbackRefusal::TailNotOwned => {
                "the rows after the capture point are not the rows this operation appended"
            }
            RollbackRefusal::TailAlreadyCommitted => {
                "the rows after the capture point are already committed at HEAD"
            }
            RollbackRefusal::HeadUnreadable => {
                "HEAD could not be read to prove the rows are uncommitted"
            }
            RollbackRefusal::IoError => "the rollback hit an I/O error before it could finish",
        }
    }

    /// True when the on-disk state may be partly rewritten (I/O failure only).
    pub fn may_have_mutated(self) -> bool {
        self == RollbackRefusal::IoError
    }
}

/// Result of one rollback attempt. A refusal left every file byte-identical
/// (except [`RollbackRefusal::IoError`], which reports the indeterminate case).
#[derive(Clone, Debug)]
pub struct RollbackOutcome {
    pub rolled_back: bool,
    pub refusal: Option<RollbackRefusal>,
    pub events_path: PathBuf,
}

impl RollbackOutcome {
    /// The operator text for a refusal (empty when the rollback succeeded).
    pub fn message(&self) -> String {
        let Some(refusal) = self.refusal else {
            return String::new();
        };
        let state = if refusal.may_have_mutated() {
            "may be partly rewritten"
        } else {
            "left unchanged"
        };
        format!(
            "{STATUS_ROLLBACK_REFUSED}: {}; {} {state}. Inspect with: git diff HEAD -- {}",
            refusal.as_str(),
            self.events_path.display(),
            self.events_path.display()
        )
    }
}

/// Pre-operation state of a Mission's status files, captured under the write lock.
#[derive(Clone, Debug)]
pub struct RollbackPoint {
    events_path: PathBuf,
    status_path: PathBuf,
    pre_event_size: u64,
    pre_status_bytes: Option<Vec<u8>>,
    events_existed: bool,
    /// The Mission key the capturing lock held, so a later rollback can verify
    /// it is made under the same authority.
    mission_key: String,
    /// The lock root the point was captured under.
    lock_root: PathBuf,
}

impl RollbackPoint {
    /// The captured event-log path.
    pub fn events_path(&self) -> &Path {
        &self.events_path
    }

    /// The captured `status.json` path.
    pub fn status_path(&self) -> &Path {
        &self.status_path
    }

    /// The captured event-log size in bytes.
    pub fn pre_event_size(&self) -> u64 {
        self.pre_event_size
    }

    /// Whether the event log existed at capture time.
    pub fn events_existed(&self) -> bool {
        self.events_existed
    }

    /// The Mission lock key the capture was made under.
    pub fn mission_key(&self) -> &str {
        &self.mission_key
    }
}

/// A probe that returns the event ids of the log **committed at `HEAD`**.
///
/// `Ok(empty)` means "the path is not in a Git checkout, `HEAD` is unborn, or
/// `HEAD` does not track the log". `Err` means the committed log could not be
/// read (the rollback then refuses with [`RollbackRefusal::HeadUnreadable`], as
/// the baseline's `HEAD_UNREADABLE` does).
pub trait HeadCommittedProbe {
    /// Read the committed event ids for `events_path`.
    fn committed_event_ids(&self, events_path: &Path) -> Result<BTreeSet<String>, String>;
}

fn require_hold(lock: &MissionLock, feature_dir: &Path) -> Result<String, MutationError> {
    // The lock must cover this directory in the same repository, not merely key
    // the same name: two checkouts with an identically named Mission have the
    // same key string but different lock files.
    let covered = feature_dir.starts_with(lock.lock_root())
        || crate::mutation_lock::canonical_repo_root(feature_dir)
            .is_ok_and(|root| root == lock.lock_root());
    let key = covered
        .then(|| mission_lock_key(feature_dir, lock.lock_root()))
        .transpose()?;
    if key.as_deref() != Some(lock.key()) {
        return Err(MutationError::RollbackRequiresHold {
            directory: feature_dir
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| feature_dir.display().to_string()),
        });
    }
    Ok(key.expect("hold verified"))
}

/// Capture the Mission's log size and `status.json` bytes under the write lock.
///
/// The `&MissionLock` is the precondition: a capture taken outside the hold
/// could later be rolled back over another writer's rows. The lock must be the
/// one for `feature_dir` (the held-aware key is compared).
pub fn capture_rollback_point(
    lock: &MissionLock,
    feature_dir: &Path,
) -> Result<RollbackPoint, MutationError> {
    let mission_key = require_hold(lock, feature_dir)?;
    let events_path = feature_dir.join(EVENTS_FILENAME);
    let status_path = feature_dir.join(SNAPSHOT_FILENAME);
    let events_existed = events_path.exists();
    let pre_event_size = if events_existed {
        std::fs::metadata(&events_path)
            .map_err(|error| MutationError::io("read metadata of", &events_path, error))?
            .len()
    } else {
        0
    };
    let pre_status_bytes = match std::fs::read(&status_path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(MutationError::io(
                "read rollback snapshot of",
                &status_path,
                error,
            ));
        }
    };
    Ok(RollbackPoint {
        events_path,
        status_path,
        pre_event_size,
        pre_status_bytes,
        events_existed,
        mission_key,
        lock_root: lock.lock_root().to_path_buf(),
    })
}

/// Event ids of the whole rows appended after `point`, or `None` when the tail
/// is not whole JSON event rows.
///
/// Read while the Mission write lock is held to learn which rows are this
/// operation's own, then pass them to the rollback as `expected_event_ids` so a
/// rollback made later, under a re-acquired hold, still refuses a foreign row.
pub fn appended_event_ids(point: &RollbackPoint) -> Result<Option<Vec<String>>, MutationError> {
    let bytes = match std::fs::read(&point.events_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Some(Vec::new())),
        Err(error) => {
            return Err(MutationError::io(
                "read appended rows of",
                &point.events_path,
                error,
            ));
        }
    };
    if (bytes.len() as u64) < point.pre_event_size {
        return Ok(None);
    }
    Ok(parse_tail_event_ids(
        &bytes[point.pre_event_size as usize..],
    ))
}

/// Cut the rows appended after `point`, or refuse and leave the log byte-identical.
///
/// `expected_event_ids` must equal the tail's ids as a multiset when supplied.
/// `head` proves the tail is not already committed; `None` refuses a non-empty
/// tail with [`RollbackRefusal::HeadUnreadable`].
pub fn rollback_events_log(
    lock: &MissionLock,
    point: &RollbackPoint,
    expected_event_ids: Option<&[String]>,
    head: Option<&dyn HeadCommittedProbe>,
) -> Result<RollbackOutcome, MutationError> {
    if point.lock_root != lock.lock_root() {
        return Err(MutationError::RollbackRequiresHold {
            directory: point
                .events_path
                .parent()
                .and_then(Path::file_name)
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| point.events_path.display().to_string()),
        });
    }
    require_hold(lock, point.events_path.parent().unwrap_or(Path::new(".")))?;
    Ok(rollback_events_locked(point, expected_event_ids, head))
}

/// Roll the log back, then restore `status.json`, only when the log half succeeded.
pub fn rollback_status_artifacts(
    lock: &MissionLock,
    point: &RollbackPoint,
    expected_event_ids: Option<&[String]>,
    head: Option<&dyn HeadCommittedProbe>,
) -> Result<RollbackOutcome, MutationError> {
    let outcome = rollback_events_log(lock, point, expected_event_ids, head)?;
    if outcome.rolled_back {
        restore_status_bytes(point)?;
    }
    Ok(outcome)
}

fn refuse(events_path: &Path, refusal: RollbackRefusal) -> RollbackOutcome {
    RollbackOutcome {
        rolled_back: false,
        refusal: Some(refusal),
        events_path: events_path.to_path_buf(),
    }
}

fn done(events_path: &Path) -> RollbackOutcome {
    RollbackOutcome {
        rolled_back: true,
        refusal: None,
        events_path: events_path.to_path_buf(),
    }
}

fn rollback_events_locked(
    point: &RollbackPoint,
    expected_event_ids: Option<&[String]>,
    head: Option<&dyn HeadCommittedProbe>,
) -> RollbackOutcome {
    if !point.events_existed && !point.events_path.exists() {
        return done(&point.events_path);
    }
    let mut file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&point.events_path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return if point.events_existed {
                refuse(&point.events_path, RollbackRefusal::LogVanished)
            } else {
                done(&point.events_path)
            };
        }
        Err(_) => return refuse(&point.events_path, RollbackRefusal::IoError),
    };
    let size = match file.metadata() {
        Ok(metadata) => metadata.len(),
        Err(_) => return refuse(&point.events_path, RollbackRefusal::IoError),
    };
    if size < point.pre_event_size {
        return refuse(&point.events_path, RollbackRefusal::LogShrank);
    }
    let tail = {
        use std::io::{Read, Seek, SeekFrom};
        if file.seek(SeekFrom::Start(point.pre_event_size)).is_err() {
            return refuse(&point.events_path, RollbackRefusal::IoError);
        }
        let mut tail = Vec::with_capacity((size - point.pre_event_size) as usize);
        if file.read_to_end(&mut tail).is_err() {
            return refuse(&point.events_path, RollbackRefusal::IoError);
        }
        tail
    };
    let Some(tail_ids) = parse_tail_event_ids(&tail) else {
        return refuse(&point.events_path, RollbackRefusal::TailUnparseable);
    };
    if let Some(expected) = expected_event_ids
        && multiset(&tail_ids) != multiset(expected)
    {
        return refuse(&point.events_path, RollbackRefusal::TailNotOwned);
    }
    if !tail_ids.is_empty() {
        let Some(probe) = head else {
            // No committed-log probe: the tail's Git state is unknown, so a cut
            // could delete committed rows. Fail closed exactly like
            // `HEAD_UNREADABLE`.
            return refuse(&point.events_path, RollbackRefusal::HeadUnreadable);
        };
        let committed = match probe.committed_event_ids(&point.events_path) {
            Ok(committed) => committed,
            Err(_) => return refuse(&point.events_path, RollbackRefusal::HeadUnreadable),
        };
        if tail_ids.iter().any(|id| committed.contains(id)) {
            return refuse(&point.events_path, RollbackRefusal::TailAlreadyCommitted);
        }
    }
    if !point.events_existed {
        drop(file);
        return match std::fs::remove_file(&point.events_path) {
            Ok(()) => done(&point.events_path),
            Err(error) if error.kind() == io::ErrorKind::NotFound => done(&point.events_path),
            Err(_) => refuse(&point.events_path, RollbackRefusal::IoError),
        };
    }
    if size == point.pre_event_size {
        return done(&point.events_path);
    }
    if file.set_len(point.pre_event_size).is_err() {
        return refuse(&point.events_path, RollbackRefusal::IoError);
    }
    if file.sync_all().is_err() {
        return refuse(&point.events_path, RollbackRefusal::IoError);
    }
    match file.metadata() {
        Ok(metadata) if metadata.len() == point.pre_event_size => done(&point.events_path),
        _ => refuse(&point.events_path, RollbackRefusal::IoError),
    }
}

fn restore_status_bytes(point: &RollbackPoint) -> Result<(), MutationError> {
    match &point.pre_status_bytes {
        Some(bytes) => {
            sk_kernel::atomic_write_with_options(
                &point.status_path,
                bytes,
                sk_kernel::AtomicWriteOptions {
                    create_parents: true,
                },
            )?;
            Ok(())
        }
        None => match std::fs::remove_file(&point.status_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(MutationError::io("remove", &point.status_path, error)),
        },
    }
}

/// Event ids of the whole JSON rows in `tail`, or `None` when it is not whole rows.
///
/// Blank lines are legal (the atomic append inserts a leading newline when the
/// log lacked a trailing one); a last row without its newline is torn.
fn parse_tail_event_ids(tail: &[u8]) -> Option<Vec<String>> {
    if !tail.is_empty() && !tail.ends_with(b"\n") {
        return None;
    }
    let mut ids = Vec::new();
    for raw in tail.split(|byte| *byte == b'\n') {
        if raw.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let row: Value = serde_json::from_slice(raw).ok()?;
        let event_id = row.get("event_id").and_then(Value::as_str)?;
        ids.push(event_id.to_owned());
    }
    Some(ids)
}

fn multiset(ids: &[String]) -> BTreeMap<&str, usize> {
    let mut counts = BTreeMap::new();
    for id in ids {
        *counts.entry(id.as_str()).or_insert(0) += 1;
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_parsing_requires_whole_rows() {
        assert_eq!(
            parse_tail_event_ids(b"{\"event_id\": \"A\"}\n\n{\"event_id\": \"B\"}\n"),
            Some(vec!["A".to_owned(), "B".to_owned()])
        );
        assert_eq!(parse_tail_event_ids(b""), Some(Vec::new()));
        assert_eq!(parse_tail_event_ids(b"{\"event_id\": \"A\"}"), None);
        assert_eq!(parse_tail_event_ids(b"not json\n"), None);
        assert_eq!(parse_tail_event_ids(b"{\"row\": 1}\n"), None);
    }

    #[test]
    fn refusal_messages_match_the_baseline_shape() {
        let outcome = RollbackOutcome {
            rolled_back: false,
            refusal: Some(RollbackRefusal::TailAlreadyCommitted),
            events_path: PathBuf::from("/tmp/x/status.events.jsonl"),
        };
        let message = outcome.message();
        assert!(message.starts_with("STATUS_ROLLBACK_REFUSED: the rows after the capture point are already committed at HEAD;"));
        assert!(message.contains("left unchanged"));
        assert!(message.contains("git diff HEAD -- /tmp/x/status.events.jsonl"));
        assert!(!RollbackRefusal::TailNotOwned.may_have_mutated());
        assert!(RollbackRefusal::IoError.may_have_mutated());
    }
}
