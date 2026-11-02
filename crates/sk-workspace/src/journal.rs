//! The allocation journal: durable intent before a Git mutation.
//!
//! Lane allocation is not atomic: a branch can be created before the worktree
//! registration succeeds, and a process can die between the two. Re-running the
//! allocator without a record of what was attempted would either fail forever
//! ("branch already exists") or silently re-cut a lane over prior work.
//!
//! The journal records the **intent** before any mutation, then the **outcome**
//! after it. Recovery reads the journal and reconciles it against observed Git
//! state; it never assumes a stale file is authoritative on its own.
//!
//! One journal file exists per mission, under
//! `<git-common-dir>/spec-kitty-workspace/allocations/`. It is written under a
//! kernel file lock with an atomic replacement, so a crash leaves either the
//! previous complete document or the new complete document.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sk_kernel::FileLock;

use crate::error::WorkspaceError;

/// Directory (under the Git common dir) holding allocation journals.
pub const JOURNAL_DIRECTORY: &str = "spec-kitty-workspace/allocations";

/// Subdirectory (under `spec-kitty-locks/`) holding journal locks.
pub const JOURNAL_LOCK_SUBDIRECTORY: &str = "workspace";

/// How long a journal writer waits for the cross-process lock.
pub const JOURNAL_LOCK_TIMEOUT: Duration = Duration::from_secs(30);

/// The lifecycle of one recorded allocation attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalPhase {
    /// A mutation was about to start. This is the only phase recovery repairs.
    Intent,
    /// The branch and worktree both exist as intended.
    Applied,
    /// The attempt failed before mutating, or was abandoned.
    Abandoned,
}

/// One durable allocation record.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AllocationRecord {
    /// Mission slug.
    pub mission_slug: String,
    /// Lane id.
    pub lane_id: String,
    /// Lane branch the attempt creates or attaches.
    pub branch: String,
    /// Absolute worktree path.
    pub path: PathBuf,
    /// Ref the lane is cut from (topology parent, explicit base, or origin ref).
    pub parent_ref: String,
    /// Attempt phase.
    pub phase: JournalPhase,
    /// ISO-8601 instant of the record.
    pub recorded_at: String,
    /// Free-form detail for a failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl AllocationRecord {
    /// A new intent record.
    pub fn intent(
        mission_slug: &str,
        lane_id: &str,
        branch: &str,
        path: &Path,
        parent_ref: &str,
    ) -> Self {
        Self {
            mission_slug: mission_slug.to_owned(),
            lane_id: lane_id.to_owned(),
            branch: branch.to_owned(),
            path: path.to_path_buf(),
            parent_ref: parent_ref.to_owned(),
            phase: JournalPhase::Intent,
            recorded_at: sk_kernel::clock::now_utc_iso(),
            detail: None,
        }
    }
}

/// The per-mission allocation journal.
#[derive(Clone, Debug)]
pub struct AllocationJournal {
    path: PathBuf,
    lock_path: PathBuf,
}

impl AllocationJournal {
    /// A journal for `mission_slug` under the repository's Git common dir.
    pub fn new(common_dir: &Path, mission_slug: &str) -> Self {
        let digest = mission_digest(mission_slug);
        Self {
            path: common_dir
                .join(JOURNAL_DIRECTORY)
                .join(format!("{digest}.json")),
            lock_path: common_dir
                .join(sk_kernel::locks::LOCK_DIRECTORY)
                .join(JOURNAL_LOCK_SUBDIRECTORY)
                .join(format!("{digest}.lock")),
        }
    }

    /// The journal document path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The journal lock path.
    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }

    /// Every record, oldest first. A missing journal is an empty journal.
    pub fn records(&self) -> Result<Vec<AllocationRecord>, WorkspaceError> {
        match sk_kernel::read_bounded(&self.path, 1024 * 1024) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).map_err(|error| WorkspaceError::InvalidArtifact {
                    artifact: "allocation journal",
                    detail: format!("{}: {error}", self.path.display()),
                })
            }
            Err(sk_kernel::KernelError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                Ok(Vec::new())
            }
            Err(error) => Err(WorkspaceError::Kernel(error)),
        }
    }

    /// Records still awaiting an outcome.
    pub fn pending(&self) -> Result<Vec<AllocationRecord>, WorkspaceError> {
        Ok(self
            .records()?
            .into_iter()
            .filter(|record| record.phase == JournalPhase::Intent)
            .collect())
    }

    /// Record an intent before mutating Git.
    pub fn begin(&self, record: AllocationRecord) -> Result<(), WorkspaceError> {
        let _lock = FileLock::acquire(&self.lock_path, JOURNAL_LOCK_TIMEOUT)?;
        let mut records = self.records()?;
        // A new intent for the same lane supersedes an older unresolved intent:
        // the newest observation is the one recovery should reconcile.
        for existing in &mut records {
            if existing.phase == JournalPhase::Intent
                && existing.lane_id == record.lane_id
                && existing.path == record.path
            {
                existing.phase = JournalPhase::Abandoned;
                existing.detail = Some("superseded by a newer allocation attempt".to_owned());
            }
        }
        records.push(record);
        self.write_locked(&records)
    }

    /// Mark the matching intent applied.
    pub fn applied(&self, lane_id: &str, path: &Path, branch: &str) -> Result<(), WorkspaceError> {
        self.resolve_intent(lane_id, path, JournalPhase::Applied, None, Some(branch))
    }

    /// Mark the matching intent abandoned with a reason.
    pub fn abandon(&self, lane_id: &str, path: &Path, detail: &str) -> Result<(), WorkspaceError> {
        self.resolve_intent(lane_id, path, JournalPhase::Abandoned, Some(detail), None)
    }

    fn resolve_intent(
        &self,
        lane_id: &str,
        path: &Path,
        phase: JournalPhase,
        detail: Option<&str>,
        branch: Option<&str>,
    ) -> Result<(), WorkspaceError> {
        let _lock = FileLock::acquire(&self.lock_path, JOURNAL_LOCK_TIMEOUT)?;
        let mut records = self.records()?;
        let mut matched = false;
        for record in &mut records {
            if record.phase == JournalPhase::Intent
                && record.lane_id == lane_id
                && record.path == path
            {
                record.phase = phase;
                if let Some(detail) = detail {
                    record.detail = Some(detail.to_owned());
                }
                if let Some(branch) = branch {
                    record.branch = branch.to_owned();
                }
                matched = true;
            }
        }
        if !matched {
            return Err(WorkspaceError::InvalidArtifact {
                artifact: "allocation journal",
                detail: format!("no pending intent for lane {lane_id} at {}", path.display()),
            });
        }
        self.write_locked(&records)
    }

    /// Replace the journal with `records` (the caller already holds the lock).
    fn write_locked(&self, records: &[AllocationRecord]) -> Result<(), WorkspaceError> {
        let parent = self
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent).map_err(|source| {
            WorkspaceError::Kernel(sk_kernel::KernelError::Io {
                action: "create allocation journal directory",
                path: parent.to_path_buf(),
                source,
            })
        })?;
        let bytes = serde_json::to_vec_pretty(&records).map_err(|error| {
            WorkspaceError::InvalidArtifact {
                artifact: "allocation journal",
                detail: error.to_string(),
            }
        })?;
        sk_kernel::atomic_write(&self.path, &bytes)?;
        Ok(())
    }
}

fn mission_digest(mission_slug: &str) -> String {
    sk_kernel::content_digest::sha256_hex(mission_slug.as_bytes())
        .chars()
        .take(32)
        .collect()
}
