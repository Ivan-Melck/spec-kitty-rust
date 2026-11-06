//! Authoring-side adapter onto the shared Mission-write lock protocol.
//!
//! Mission authoring (create, plan/tasks, dependency finalization) does not own
//! lock authority. The one canonical door is
//! [`sk_status::mutation::acquire_mission_lock`], which keys on the Mission
//! directory name or the coordination directory name `<slug>-<mid8>`, resolves
//! that key from the canonical primary `meta.json`, and holds
//! `<git-common-dir>/spec-kitty-locks/<key>.status.lock` through the PID-fenced
//! kernel [`StatusLock`](sk_kernel::lock_families::StatusLock).
//!
//! Replacing the retired checkout-local `.kittify/.../mission-write.lock` with
//! this door means:
//!
//! - a Mission's primary and coordination directories serialize each other, and
//!   so do linked worktrees and the main checkout;
//! - a nested authoring step on the same thread re-enters the same lock instead
//!   of deadlocking or taking a second file;
//! - a coordination-routed Mission never writes under a key no other writer
//!   takes;
//! - creation takes the documented **birth** key (the directory name), because
//!   `meta.json` does not exist yet.
//!
//! This module only selects the authoring policy (bounded wait, birth vs
//! post-birth). It contains no lock mechanics of its own.

use std::path::Path;
use std::time::Duration;

use sk_kernel::locks::LockWait;
use sk_status::mutation::{MissionLock, MutationError};

/// Bound for an authoring lock take (the shared baseline 10 s bound).
pub const MISSION_WRITE_LOCK_TIMEOUT: Duration = sk_status::mutation::MISSION_WRITE_LOCK_TIMEOUT;

/// Acquire the Mission write lock for an existing Mission directory.
///
/// `mission_dir` is the Mission's directory (`kitty-specs/<dir-name>`); the key
/// and its primary/coordination aliases are resolved by the shared protocol.
pub fn lock_mission(
    mission_dir: &Path,
    repo_root: &Path,
    wait: LockWait,
) -> Result<MissionLock, MutationError> {
    sk_status::mutation::acquire_mission_lock(mission_dir, Some(repo_root), wait)
}

/// Acquire the create's birth lock for a Mission directory that has no
/// `meta.json` yet.
pub fn lock_mission_birth(
    mission_dir: &Path,
    repo_root: &Path,
    wait: LockWait,
) -> Result<MissionLock, MutationError> {
    sk_status::mutation::acquire_mission_birth_lock(mission_dir, Some(repo_root), wait)
}
