//! The coordination transaction store.
//!
//! Upstream writes mission bookkeeping through one chokepoint:
//! `acquire → policy gate → append → materialize → commit → release`, with a
//! durable record of intent so an interrupted run can be reconciled. This module
//! is the native equivalent, and it is a **store**, not a projection:
//!
//! * it resolves the real write surface from the mission's stored topology and
//!   the coordination surface's actual state, and it never mutates while merely
//!   reading;
//! * it materializes (attaches) and seeds a coordination worktree with journals
//!   before every Git or filesystem mutation;
//! * it stages artifacts with byte snapshots and commits them through the
//!   path-scoped safe-commit substrate with the caller-resolved protected set;
//! * it treats an unconfirmed advance as **indeterminate** and a landed commit
//!   as immutable — nothing here ever undoes a commit or overwrites content it
//!   did not observe.
//!
//! # Coordination surface states
//!
//! [`CoordState`] is the four-way answer upstream's shared probe gives:
//!
//! * `Materialized` — the canonical coordination worktree is registered on the
//!   declared branch **and** its mission directory exists;
//! * `Empty` — the worktree is registered on the branch but its mission
//!   directory is absent (a ticket to seed, never a silent primary read);
//! * `Unmaterialized` — the branch exists (locally or on a remote) but no
//!   worktree is registered;
//! * `Deleted` — the declared branch is gone everywhere a read can see.
//!
//! A read resolves `Empty`/`Unmaterialized`/`Deleted` to a *declared primary
//! fallback* without creating anything; only [`CoordTransaction::acquire`] is
//! allowed to materialize, and only a state whose seed-safety precondition holds.
//!
//! # Seeding safety
//!
//! A pre-fix `Empty` surface is seeded from the primary mission directory with
//! the mission's COORD-partition artifacts. Seeding is refused when the
//! coordination branch already carries coordination-only commits that this
//! store has no seed record for: that branch holds state this crate cannot read
//! at the tip (there is no tree read in `sk-git`), and forking it would silently
//! replace committed coordination content.
//!
//! # Locking
//!
//! Every mutation takes the **checkout claim** lock for the checkout it writes,
//! then the canonical Mission status lock (through `sk-mission`'s
//! `lock_mission`, which preserves the primary/coordination alias holds). The
//! claim-before-Mission order is the kernel's documented order; a claim taken
//! while holding a Mission lock is refused by the kernel itself.

use std::any::Any;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sk_git::{
    BranchName, CommitGuardCapability, ExpectedBlob, GitError, GitPath, Oid, ProtectedBranches,
    RefName, SafeCommitRequest, SourceRevision, StartPoint,
};
use sk_kernel::{LockWait, StatusLock, StatusLockHolder};
use sk_mission::routes_through_coordination;

use crate::artifact::{MissionArtifactKind, kind_for_mission_file};
use crate::coordination::is_published_write_short_circuit_kind;
use crate::error::WorkspaceError;
use crate::lifecycle::LifecyclePhase;
use crate::protection::ProtectionPolicy;
use crate::surface::{CoordState, TopologySurface};
use crate::workspace::MissionWorkspace;

/// Directory (under the Git common dir) holding coordination journals.
pub const COORD_JOURNAL_DIRECTORY: &str = "spec-kitty-workspace/coord";

/// Subdirectory (under `spec-kitty-locks/`) holding journal locks.
pub const COORD_JOURNAL_LOCK_SUBDIRECTORY: &str = "workspace";

/// How long a journal writer waits for its cross-process lock.
pub const COORD_JOURNAL_LOCK_TIMEOUT: Duration = Duration::from_secs(30);

/// Default bound for a coordination transaction's claim and Mission locks.
pub const COORD_TRANSACTION_LOCK_TIMEOUT: Duration = Duration::from_secs(30);

/// Commit-trailer key that marks a coordination seed commit.
pub const COORD_SEED_TRAILER: &str = "Spec-Kitty-Coordination-Seed";

/// Largest journal document this crate will read.
pub const MAX_COORD_JOURNAL_BYTES: u64 = 4 * 1024 * 1024;

/// Largest single artifact this crate will seed.
/// Raised from the old 16 MiB local cap to the Git seam's canonical blob bound
/// (64 MiB, the status-log limit) when that seam gained an explicit bounded read:
/// a legitimate `status.events.jsonl` above 16 MiB used to be refused here while
/// the reference reads it whole.
pub const MAX_SEED_FILE_BYTES: u64 = sk_git::MAX_BLOB_BYTES;

/// Largest existing artifact this crate will snapshot for rollback.
pub const MAX_SNAPSHOT_BYTES: u64 = 16 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Read-only surface resolution
// ---------------------------------------------------------------------------

/// A read-only resolution of where a mission's bookkeeping lives right now.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoordSurfaceRead {
    /// The probed coordination state.
    pub state: CoordState,
    /// The declared coordination branch, when the mission records one.
    pub coordination_branch: Option<String>,
    /// The canonical coordination worktree path, when the topology routes
    /// through coordination (whether or not it exists).
    pub coordination_worktree: Option<PathBuf>,
    /// The canonical coordination Mission directory inside that worktree.
    pub coordination_mission_dir: Option<PathBuf>,
    /// The mission directory a reader must use for this state.
    ///
    /// `Materialized` resolves the coordination directory, `Empty` and `None`
    /// resolve the declared primary directory, and `Unmaterialized`/`Deleted`
    /// refuse (see [`read_coordination_surface`]). This is the *read* answer and
    /// creates nothing.
    pub read_dir: PathBuf,
    /// Is [`CoordSurfaceRead::read_dir`] the primary fallback rather than a
    /// materialized coordination surface?
    pub degraded_to_primary: bool,
}

/// Resolve the coordination surface for reading, without any mutation.
///
/// Nothing here creates a worktree, writes a file, or takes a lock.
///
/// # One read authority
///
/// This is the *same* policy [`crate::surface::resolve_artifact_surface`]
/// applies, expressed once here and asserted against that function:
///
/// * `None` (a coordination-less topology) and `Empty` (a registered worktree
///   whose Mission directory is absent) resolve the declared **primary**
///   directory, because the create→first-write window is genuinely primary;
/// * `Materialized` resolves the coordination directory;
/// * `Unmaterialized` **refuses** with
///   [`WorkspaceError::CoordinationWorktreeUnmaterialized`] — the branch is not
///   lost, but its surface does not exist yet, and substituting the primary
///   checkout would let a reader act on emptiness as if it were the document
///   (#4959);
/// * `Deleted` **refuses** with [`WorkspaceError::CoordinationBranchDeleted`] —
///   the loud data-loss failure (#1889 R3).
///
/// The former permissive answer (degrade all three non-materialized states to
/// the primary checkout) let a reader of a *deleted* coordination surface
/// silently read the primary copy. Both readers now answer the same question.
pub fn read_coordination_surface(
    workspace: &MissionWorkspace,
) -> Result<CoordSurfaceRead, WorkspaceError> {
    let probe = workspace.probe_coordination()?;
    // A derived branch (the `meta.json` record was lost) that is absent is NOT
    // data loss: nobody ever recorded it, and the canonical grammar names a
    // branch that was never minted. That is the reference's
    // `CoordBranchUndeclaredAndAbsent`, not `Deleted`.
    if probe.derived && probe.state == CoordState::Deleted {
        return Err(WorkspaceError::CoordRefused {
            code: "COORD_BRANCH_UNDECLARED_AND_ABSENT",
            message: format!(
                "mission {} stores coordination topology {} but records no coordination_branch, and the canonical branch {:?} does not exist; refusing to guess a coordination surface",
                workspace.slug(),
                workspace.topology().as_str(),
                probe.branch.as_deref().unwrap_or("<underived>")
            ),
        });
    }
    let primary_dir = workspace.mission_dir().to_path_buf();
    let coordination_mission_dir = crate::topology::coordination_mission_dir(workspace);
    let resolved = crate::surface::coord_read_surface(
        probe.state,
        workspace.slug(),
        probe.branch.as_deref(),
        &primary_dir,
        coordination_mission_dir.as_deref(),
    )?;
    let degraded_to_primary = resolved.surface_kind != TopologySurface::Coord;
    Ok(CoordSurfaceRead {
        state: probe.state,
        coordination_branch: probe.branch,
        coordination_worktree: probe.worktree,
        coordination_mission_dir,
        read_dir: resolved.path,
        degraded_to_primary,
    })
}

// ---------------------------------------------------------------------------
// Status read surface (including the branch-ref arm)
// ---------------------------------------------------------------------------

/// The reference's read contract for a Mission's status event log.
///
/// `specify_cli.coordination.status_service.EventLogReadContract` names three
/// read sources. Two of them are expressible without creating anything, and
/// this value names exactly those:
///
/// * [`CoordStatusRead::Directory`] — the log belongs to a checkout that is
///   already on disk (`COORDINATION_WORKTREE`, or the `PRIMARY_CHECKOUT`
///   fallback the reference takes when nobody ever recorded a branch);
/// * [`CoordStatusRead::BranchRef`] — the branch exists but no worktree is
///   attached (`COORDINATION_BRANCH_REF`), so the log is read out of the branch
///   tree with no materialization, no lock and no write.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CoordStatusRead {
    /// Read the status artifacts from this Mission directory.
    Directory(PathBuf),
    /// Read the status event log out of this branch ref.
    BranchRef {
        /// The declared or canonically derived coordination branch (short name).
        branch: String,
        /// The coordination Mission directory name inside that branch's tree.
        mission_dir_name: String,
    },
}

impl CoordStatusRead {
    /// The directory arm, when this read resolves to a checkout on disk.
    pub fn directory(&self) -> Option<&Path> {
        match self {
            Self::Directory(path) => Some(path),
            Self::BranchRef { .. } => None,
        }
    }
}

/// Resolve where a **read** of this Mission's status log must come from.
///
/// This is the read-only half of the reference's read contract
/// (`coordination.status_transition._read_contract_from_transaction_target`),
/// kept distinct from [`read_coordination_surface`] on purpose: the reference's
/// status read does **not** refuse `Unmaterialized`/`Deleted`, it falls back to
/// the primary checkout ("reading the dangling ref would report every WP as
/// genesis"). [`read_coordination_surface`] is the placement answer and keeps
/// its refusals.
///
/// Resolution order, exactly as the reference decides it:
///
/// 1. a coordination-less topology reads the declared primary directory
///    (`PRIMARY_CHECKOUT`);
/// 2. an attached coordination worktree reads its Mission directory — even when
///    the directory is absent (`Empty`), because an absent log is empty, not
///    missing;
/// 3. no worktree but an existing **local** branch (`refs/heads/<branch>`, the
///    reference's `branch_exists`) reads the branch ref;
/// 4. otherwise (the branch was deleted, e.g. after a post-merge teardown) the
///    declared primary directory is the authoritative read source.
///
/// Nothing here creates a worktree, writes a file, takes a lock, or seeds.
pub fn read_status_surface(
    workspace: &MissionWorkspace,
) -> Result<CoordStatusRead, WorkspaceError> {
    let primary_dir = workspace.mission_dir().to_path_buf();
    if !routes_through_coordination(workspace.topology()) {
        return Ok(CoordStatusRead::Directory(primary_dir));
    }
    let probe = workspace.probe_coordination()?;
    if probe.worktree.is_some() {
        // `mission_dir` is the coordination Mission directory under the
        // registered worktree; it may legitimately not exist yet.
        let dir = probe.mission_dir.ok_or_else(|| WorkspaceError::CoordRefused {
            code: "COORD_MISSION_DIR_UNRESOLVED",
            message: format!(
                "mission {} has an attached coordination worktree but no resolvable coordination Mission directory",
                workspace.slug()
            ),
        })?;
        return Ok(CoordStatusRead::Directory(dir));
    }
    match probe.branch {
        Some(branch) if probe.local_head => Ok(CoordStatusRead::BranchRef {
            branch,
            mission_dir_name: workspace.coord_mission_dir_name(),
        }),
        _ => Ok(CoordStatusRead::Directory(primary_dir)),
    }
}

/// Read the exact `status.events.jsonl` bytes at the coordination branch ref.
///
/// This is the reference's `git show <destination_ref>:kitty-specs/<dir>/\
/// status.events.jsonl`, expressed through the canonical Git ref/blob reader:
/// the branch is resolved with [`sk_git::GitRepository::read_ref`] and the tree
/// entry is read with [`sk_git::GitRepository::read_blob_at_bounded`], so no
/// worktree, index, lock or temporary file is involved. `Ok(None)` means the
/// branch or the log path does not exist — the reference's non-zero `git show`
/// arm, which reads as "no events".
///
/// A branch that does not exist is *not* an error here: the reference returns an
/// empty event list for it. An unreadable branch, a `GitPath`-invalid Mission
/// directory name, and an over-bound blob do stay typed errors.
pub fn read_status_events_at_branch_ref(
    workspace: &MissionWorkspace,
) -> Result<Option<Vec<u8>>, WorkspaceError> {
    let probe = workspace.probe_coordination()?;
    let Some(branch) = probe.branch else {
        return Ok(None);
    };
    let mission_dir_name = workspace.coord_mission_dir_name();
    let reference = RefName::parse(&format!("refs/heads/{branch}"))?;
    if workspace
        .workspace()
        .repository()
        .read_ref(&reference)?
        .is_none()
    {
        return Ok(None);
    }
    let path = status_log_git_path(&mission_dir_name)?;
    workspace
        .workspace()
        .repository()
        .read_blob_at_bounded(&StartPoint::Ref(reference), &path, MAX_SEED_FILE_BYTES)
        .map_err(WorkspaceError::from)
}

/// The coordination-tree path of a Mission's status event log.
fn status_log_git_path(mission_dir_name: &str) -> Result<GitPath, WorkspaceError> {
    let relative = format!(
        "{}/{mission_dir_name}/{}",
        crate::workspace::SPECS_DIR_NAME,
        sk_status::EVENTS_FILENAME
    );
    GitPath::from_git_bytes(relative.as_bytes()).map_err(WorkspaceError::from)
}

/// Why a ref-parameterised committed-status read could not answer.
///
/// These are the cases in which the reference would have read something and this
/// adapter cannot: they must reach the caller as "the check did not run", never
/// as "there are no events". Every *failure of git itself* is the reference's
/// own caught arm and stays `Ok(None)` below.
#[derive(Debug, thiserror::Error)]
pub enum CoordStatusRefReadError {
    /// The ref text is not admissible as a Git revision argument.
    ///
    /// [`sk_git::SourceRevision`] refuses an empty value, a leading `-` and any
    /// control byte, so it can never be read as an option or split a record. A
    /// revision that Git itself rejects is *not* this case: that is the caught arm
    /// below and reads as "no events", like the reference's exited `git show`.
    #[error("coordination ref {value:?} is not admissible as a Git revision argument")]
    UnsupportedRef {
        /// The offending ref text.
        value: String,
    },
    /// The Mission directory name cannot be a Git tree path.
    #[error("mission directory {detail} cannot be addressed as a Git tree path")]
    MissionDirName {
        /// What was wrong.
        detail: String,
    },
    /// The committed log is larger than this adapter's bounded blob read.
    ///
    /// Source's `git show` is unbounded; reading the over-bound log would mean
    /// inventing a truncation, so this is reported instead.
    #[error("{reference}: the committed status log exceeds the {limit}-byte read bound")]
    OverBound {
        /// The `<ref>:<path>` the read came from.
        reference: String,
        /// The adapter's bound.
        limit: u64,
    },
}

/// Read the exact `status.events.jsonl` bytes at an arbitrary coordination ref.
///
/// This is the reference's `read_event_log(EventLogReadContract.
/// coordination_branch_ref(..))`: `git show <ref>:kitty-specs/<dir>/\
/// status.events.jsonl`, parameterised by the ref the caller recorded rather than
/// by this checkout's own probed coordination branch. The ref is read through
/// [`sk_git::SourceRevision`], so the reference's revision breadth (a short branch,
/// a tag, a remote-tracking name, `HEAD`, a full object id) is available without
/// coercing it into a local branch.
///
/// `Ok(None)` is the reference's non-zero `git show` arm, which reads as "no
/// events": an unknown revision, an absent log path at that revision, or any other
/// git-level refusal. A Native-only limit (an argv-unsafe revision, an
/// unrepresentable tree path, or the bounded blob read) is a typed error so it can
/// never be mistaken for "no events".
///
/// Read-only: no worktree, index, lock or temporary file is involved.
pub fn read_status_events_at_ref(
    repository: &sk_git::GitRepository,
    coord_ref: &str,
    mission_dir_name: &str,
) -> Result<Option<Vec<u8>>, CoordStatusRefReadError> {
    let revision =
        SourceRevision::parse(coord_ref).map_err(|_| CoordStatusRefReadError::UnsupportedRef {
            value: coord_ref.to_owned(),
        })?;
    let path = GitPath::from_git_bytes(
        format!(
            "{}/{mission_dir_name}/{}",
            crate::workspace::SPECS_DIR_NAME,
            sk_status::EVENTS_FILENAME
        )
        .as_bytes(),
    )
    .map_err(|error| CoordStatusRefReadError::MissionDirName {
        detail: error.to_string(),
    })?;
    match repository.read_blob_at_revision_bounded(&revision, &path, MAX_SEED_FILE_BYTES) {
        Ok(bytes) => Ok(bytes),
        // The one Native-only bound: report it instead of inventing empty.
        Err(GitError::BlobTooLarge { limit, .. }) => Err(CoordStatusRefReadError::OverBound {
            reference: format!("{coord_ref}:{}", path.display_lossy()),
            limit,
        }),
        // Any other git refusal is the reference's caught `git show` arm.
        Err(_) => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// Journal
// ---------------------------------------------------------------------------

/// What kind of mutation a journal record describes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordRecordKind {
    /// Attach the canonical coordination worktree to an existing branch.
    Materialize,
    /// Copy COORD-partition artifacts from the primary checkout and commit them.
    Seed,
    /// Commit staged artifacts on the resolved destination.
    Commit,
    /// Fast-forward a destination to an already-integrated source tip.
    Integration,
    /// Mint the mission's coordination branch at the target tip.
    ///
    /// The reference mints before any bookkeeping write and rolls a
    /// create-minted branch back when the create fails, so the mint needs its
    /// own durable record: [`crate::coord::bootstrap_coordination`] takes the
    /// claim + Mission locks *before* minting and journals both the mint and its
    /// provenance rollback.
    Mint,
}

impl CoordRecordKind {
    /// The stable wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Materialize => "materialize",
            Self::Seed => "seed",
            Self::Commit => "commit",
            Self::Integration => "integration",
            Self::Mint => "mint",
        }
    }
}

/// The lifecycle of one recorded coordination mutation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordPhase {
    /// A mutation was about to start. This is the only phase recovery repairs.
    Intent,
    /// The mutation is proven complete.
    Applied,
    /// The mutation provably did not land.
    Abandoned,
    /// The mutation's outcome cannot be decided from observed Git state.
    Indeterminate,
}

/// One durable coordination record.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CoordRecord {
    /// Mutation kind.
    pub kind: CoordRecordKind,
    /// Phase.
    pub phase: CoordPhase,
    /// Mission slug.
    pub mission_slug: String,
    /// Short operation label (`status_append`, `planning_commit`, ...).
    pub operation: String,
    /// Branch the mutation targets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Checkout the mutation happens in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<PathBuf>,
    /// Branch tip observed before the mutation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_tip: Option<String>,
    /// The immutable commit object this mutation produced or intended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_commit: Option<String>,
    /// ISO-8601 instant of the record.
    pub recorded_at: String,
    /// Free-form detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl CoordRecord {
    /// A new intent record.
    pub fn intent(
        kind: CoordRecordKind,
        mission_slug: &str,
        operation: &str,
        branch: Option<&str>,
        worktree: Option<&Path>,
        observed_tip: Option<&Oid>,
    ) -> Self {
        Self {
            kind,
            phase: CoordPhase::Intent,
            mission_slug: mission_slug.to_owned(),
            operation: operation.to_owned(),
            branch: branch.map(str::to_owned),
            worktree: worktree.map(Path::to_path_buf),
            observed_tip: observed_tip.map(ToString::to_string),
            candidate_commit: None,
            recorded_at: sk_kernel::clock::now_utc_iso(),
            detail: None,
        }
    }

    fn matches_intent(&self, other: &CoordRecord) -> bool {
        self.phase == CoordPhase::Intent
            && self.kind == other.kind
            && self.mission_slug == other.mission_slug
            && self.operation == other.operation
            && self.branch == other.branch
            && self.worktree == other.worktree
    }
}

/// The per-mission coordination journal.
#[derive(Clone, Debug)]
pub struct CoordJournal {
    path: PathBuf,
    lock_path: PathBuf,
}

impl CoordJournal {
    /// A journal for `mission_slug` under the repository's Git common dir.
    pub fn new(common_dir: &Path, mission_slug: &str) -> Self {
        let digest = mission_digest(mission_slug);
        Self {
            path: common_dir
                .join(COORD_JOURNAL_DIRECTORY)
                .join(format!("{digest}.json")),
            lock_path: common_dir
                .join(sk_kernel::locks::LOCK_DIRECTORY)
                .join(COORD_JOURNAL_LOCK_SUBDIRECTORY)
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
    pub fn records(&self) -> Result<Vec<CoordRecord>, WorkspaceError> {
        match sk_kernel::read_bounded(&self.path, MAX_COORD_JOURNAL_BYTES) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).map_err(|error| WorkspaceError::CoordJournal {
                    path: self.path.clone(),
                    detail: error.to_string(),
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

    /// Records still awaiting a proof of outcome.
    pub fn pending(&self) -> Result<Vec<CoordRecord>, WorkspaceError> {
        Ok(self
            .records()?
            .into_iter()
            .filter(|record| record.phase == CoordPhase::Intent)
            .collect())
    }

    /// Record an intent before mutating anything.
    ///
    /// An unresolved older intent of the same shape is **left pending**. It used
    /// to be rewritten to `Abandoned` ("superseded by a newer attempt"), which
    /// claimed a proven outcome this run never proved: a crash between "commit
    /// landed" and "journal resolved" produced a journal that *said* the landed
    /// attempt never happened, and recovery could never see it again. Recovery is
    /// the only place an outcome is proven, and it now decides every pending
    /// record from observed Git state (see [`recover_interrupted`]).
    pub fn begin(&self, record: CoordRecord) -> Result<(), WorkspaceError> {
        let _lock = sk_kernel::FileLock::acquire(&self.lock_path, COORD_JOURNAL_LOCK_TIMEOUT)?;
        let mut records = self.records()?;
        records.push(record);
        self.write_locked(&records)
    }

    /// Resolve the matching intent with its proven outcome.
    pub fn resolve(
        &self,
        intent: &CoordRecord,
        phase: CoordPhase,
        detail: Option<&str>,
        candidate_commit: Option<&Oid>,
    ) -> Result<(), WorkspaceError> {
        let _lock = sk_kernel::FileLock::acquire(&self.lock_path, COORD_JOURNAL_LOCK_TIMEOUT)?;
        let mut records = self.records()?;
        let mut matched = false;
        for record in records.iter_mut().rev() {
            if record.matches_intent(intent) {
                record.phase = phase;
                if let Some(detail) = detail {
                    record.detail = Some(detail.to_owned());
                }
                if let Some(candidate) = candidate_commit {
                    record.candidate_commit = Some(candidate.to_string());
                }
                matched = true;
                break;
            }
        }
        if !matched {
            return Err(WorkspaceError::CoordJournal {
                path: self.path.clone(),
                detail: format!(
                    "no pending {} intent for {} in this journal",
                    intent.kind.as_str(),
                    intent.operation
                ),
            });
        }
        self.write_locked(&records)
    }

    /// Every seed commit this store recorded for `branch`.
    pub fn recorded_seed_commits(&self, branch: &str) -> Result<Vec<Oid>, WorkspaceError> {
        Ok(self
            .recorded_seeds(branch)?
            .into_iter()
            .map(|(commit, _)| commit)
            .collect())
    }

    /// Every recorded `(seed commit, observed tip)` pair for `branch`.
    ///
    /// A caller that wants to undo a mint it made needs the observation the
    /// commit was pinned to: a seed commit whose recorded parent is still the
    /// branch's creation base is provably this mission's own seed and nothing
    /// else, which is exactly the provenance a rollback may rely on.
    pub fn recorded_seeds(&self, branch: &str) -> Result<Vec<(Oid, Option<Oid>)>, WorkspaceError> {
        let mut seeds = Vec::new();
        for record in self.records()? {
            if record.kind != CoordRecordKind::Seed || record.phase != CoordPhase::Applied {
                continue;
            }
            if record.branch.as_deref() != Some(branch) {
                continue;
            }
            let Some(candidate) = record.candidate_commit else {
                continue;
            };
            let observed = match record.observed_tip.as_deref() {
                Some(observed) => Some(Oid::parse(observed)?),
                None => None,
            };
            seeds.push((Oid::parse(&candidate)?, observed));
        }
        Ok(seeds)
    }

    /// Append a record that is already resolved (a rollback receipt).
    pub fn record_resolved(&self, record: CoordRecord) -> Result<(), WorkspaceError> {
        let _lock = sk_kernel::FileLock::acquire(&self.lock_path, COORD_JOURNAL_LOCK_TIMEOUT)?;
        let mut records = self.records()?;
        records.push(record);
        self.write_locked(&records)
    }

    /// Replace the journal with `records` (the caller already holds the lock).
    fn write_locked(&self, records: &[CoordRecord]) -> Result<(), WorkspaceError> {
        let parent = self
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent).map_err(|source| {
            WorkspaceError::Kernel(sk_kernel::KernelError::Io {
                action: "create coordination journal directory",
                path: parent.to_path_buf(),
                source,
            })
        })?;
        let bytes =
            serde_json::to_vec_pretty(records).map_err(|error| WorkspaceError::CoordJournal {
                path: self.path.clone(),
                detail: error.to_string(),
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

// ---------------------------------------------------------------------------
// Locks and ownership proof
// ---------------------------------------------------------------------------

/// The canonical Mission status lock, held through `sk-mission`'s door.
///
/// The concrete guard lives in `sk-status`; this crate does not depend on that
/// crate, so the guard is carried type-erased. Dropping this value releases the
/// lock exactly once, with the alias holds the canonical door registered.
#[must_use = "hold the Mission lock for the duration of the protected transaction"]
pub struct MissionHold {
    key: String,
    path: Option<PathBuf>,
    holder: Option<StatusLockHolder>,
    _guard: Box<dyn Any>,
}

impl MissionHold {
    /// Acquire the Mission status lock for `mission_dir`.
    pub fn acquire(
        mission_dir: &Path,
        repo_root: &Path,
        wait: LockWait,
    ) -> Result<Self, WorkspaceError> {
        let guard = sk_mission::lock_mission(mission_dir, repo_root, wait).map_err(|error| {
            WorkspaceError::CoordRefused {
                code: "MISSION_LOCK_FAILED",
                message: format!(
                    "could not take the canonical Mission lock for {}: {error}",
                    mission_dir.display()
                ),
            }
        })?;
        Ok(Self {
            key: guard.key().to_owned(),
            path: Some(guard.path().to_path_buf()),
            holder: guard.holder(),
            _guard: Box::new(guard),
        })
    }

    /// The Mission lock key the canonical door resolved.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// The actual lock path held by the canonical Mission guard.
    ///
    /// This is the guard's own path (copied off it before boxing), not a
    /// re-derivation from the mission directory name.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The recorded holder sidecar, when one is readable.
    pub fn holder(&self) -> Option<&StatusLockHolder> {
        self.holder.as_ref()
    }
}

/// Evidence that one transaction owned the exact checkout and branch it wrote.
///
/// Minted only from observed facts, after both locks are held. It is the receipt
/// half of "no invented identity or permission": every field is either read from
/// the repository or taken from the canonical Mission/claim lock doors.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnershipProof {
    /// Mission slug.
    pub mission_slug: String,
    /// Declared mission id (ULID).
    pub mission_id: String,
    /// Canonical 8-character disambiguator.
    pub mid8: String,
    /// The stored topology the decision was made from.
    pub topology: String,
    /// The surface being written.
    pub surface: TopologySurface,
    /// The destination branch (short name).
    pub destination_ref: String,
    /// The checkout the write lands in.
    pub worktree_root: PathBuf,
    /// The branch that checkout has checked out, when not detached.
    pub worktree_branch: Option<String>,
    /// The observed destination tip.
    pub observed_tip: Option<Oid>,
    /// The kernel checkout-claim lock file held for this write.
    pub checkout_claim_lock: PathBuf,
    /// The canonical Mission lock key.
    pub mission_lock_key: String,
    /// The Mission lock file, when the shared kernel primitive derives it.
    pub mission_lock_path: Option<PathBuf>,
    /// The Mission lock holder sidecar, when readable.
    pub mission_lock_holder: Option<StatusLockHolder>,
}

impl std::fmt::Debug for MissionHold {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MissionHold")
            .field("key", &self.key)
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

/// A held claim + Mission lock pair, in the kernel's required order.
///
/// This is the crate's **one** lock protocol for a bookkeeping write. It is
/// crate-visible so the planning-commit and integration writers acquire exactly
/// this pair instead of mutating refs without authority.
#[derive(Debug)]
pub(crate) struct CoordLocks {
    mission: MissionHold,
    claim: StatusLock,
}

impl CoordLocks {
    /// Take the checkout claim, then the canonical Mission lock.
    ///
    /// `write_checkout` is the checkout the mutation lands in (the coordination
    /// worktree, a lane worktree, or the primary checkout), so two writers on
    /// the same checkout serialize even when they name different missions.
    pub(crate) fn acquire(
        write_checkout: &Path,
        mission_dir: &Path,
        repo_root: &Path,
        common_dir: &Path,
        timeout: Duration,
    ) -> Result<Self, WorkspaceError> {
        // Claim first: the kernel refuses a claim taken under a Mission lock.
        let claim = sk_kernel::acquire_checkout_claim_lock(
            write_checkout,
            common_dir,
            LockWait::Bounded(timeout),
        )?;
        let mission = MissionHold::acquire(mission_dir, repo_root, LockWait::Bounded(timeout))?;
        Ok(Self { mission, claim })
    }

    /// The kernel checkout-claim lock file held for this write.
    pub(crate) fn claim_path(&self) -> &Path {
        self.claim.path()
    }

    /// The canonical Mission lock held for this write.
    pub(crate) fn mission(&self) -> &MissionHold {
        &self.mission
    }
}

// ---------------------------------------------------------------------------
// Commit evidence
// ---------------------------------------------------------------------------

/// An immutable commit that exists in the repository.
///
/// Nothing in this crate can undo one: a landed commit is evidence, and the only
/// follow-up this module may suggest is the index repair the safe-commit
/// substrate itself names.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LandedCommit {
    /// The commit object.
    pub commit: Oid,
    /// The branch it landed on.
    pub destination: String,
    /// The checkout it was created from.
    pub worktree_root: PathBuf,
    /// Its own changed-path set.
    pub committed_paths: Vec<String>,
    /// The destination tip observed before the commit.
    pub observed_tip: Option<Oid>,
    /// A post-landing repair the substrate reported, when any.
    pub index_repair: Option<String>,
}

/// The typed result of a coordination commit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CoordCommitOutcome {
    /// A commit landed. It is immutable.
    Landed(LandedCommit),
    /// The staged paths already matched the destination tip; nothing was created.
    NoOp {
        /// The destination tip both sides agree on.
        head: Oid,
        /// The destination branch.
        destination: String,
    },
    /// The commit provably did not land and no state was overwritten.
    NotLanded {
        /// The candidate object the substrate created, which is unreachable.
        candidate: Oid,
        /// The observed destination tip, unchanged.
        observed_tip: Option<Oid>,
        /// What the substrate reported.
        detail: String,
    },
    /// The commit may or may not have landed; nothing was undone.
    Indeterminate {
        /// The candidate object the substrate created.
        candidate: Oid,
        /// The destination tip read after the failure.
        observed_tip: Option<Oid>,
        /// What the substrate reported.
        detail: String,
    },
}

impl CoordCommitOutcome {
    /// The landed commit, when there is one.
    pub fn landed(&self) -> Option<&LandedCommit> {
        match self {
            Self::Landed(commit) => Some(commit),
            _ => None,
        }
    }

    /// Is the outcome a proven no-op?
    pub fn is_noop(&self) -> bool {
        matches!(self, Self::NoOp { .. })
    }

    /// Must an operator inspect the branch before retrying?
    pub fn is_indeterminate(&self) -> bool {
        matches!(self, Self::Indeterminate { .. })
    }
}

/// A proven commit outcome plus any journal fault recorded after it.
struct CommitReport {
    outcome: CoordCommitOutcome,
    journal_fault: Option<String>,
}

/// How the write surface was established.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoordEstablishment {
    /// A coordination-less mission writes its primary checkout.
    Primary,
    /// An already-materialized coordination worktree was reused.
    Reused,
    /// A declared coordination branch was attached as a worktree.
    Materialized,
    /// An empty coordination worktree was seeded from the primary checkout.
    Seeded,
    /// Nothing needed establishing (materialization only, no seed content).
    Noop,
}

/// Evidence of a seed operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoordSeedReceipt {
    /// The coordination worktree that was seeded.
    pub worktree: PathBuf,
    /// The coordination branch.
    pub branch: String,
    /// Mission-relative paths this seed carried from the primary checkout.
    pub seeded_paths: Vec<String>,
    /// Mission-relative paths restored from the coordination branch tip.
    pub restored_from_branch: Vec<String>,
    /// Root-checkout copies removed, so the coordination copy is the only home.
    pub restored_root: Vec<String>,
    /// Root-checkout copies left in place, with the reason.
    pub preserved_root: Vec<String>,
    /// Non-fatal observations (a coordinated copy won over a differing root copy).
    pub warnings: Vec<String>,
    /// The seed commit, when any content was carried.
    pub landed: Option<LandedCommit>,
    /// The state observed before the seed.
    pub state_before: CoordState,
    /// A journal fault after a landed seed commit.
    ///
    /// A landed commit is immutable evidence: a failure to record it must never
    /// be reported as a failed seed.
    pub journal_fault: Option<String>,
}

/// What a coordination transaction produced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoordReceipt {
    /// The commit outcome.
    pub outcome: CoordCommitOutcome,
    /// The ownership evidence for the write.
    pub proof: OwnershipProof,
    /// The coordination state observed before the transaction.
    pub state_before: CoordState,
    /// How the write surface was established.
    pub establishment: CoordEstablishment,
    /// The seed operation, when one ran during acquire.
    pub seed: Option<CoordSeedReceipt>,
    /// The coordination journal document.
    pub journal_path: PathBuf,
    /// A journal fault recorded **after** the outcome was proven.
    ///
    /// Reporting is not allowed to invert the outcome: when the commit landed
    /// and the journal write then failed, this names the fault while the
    /// caller still receives [`CoordCommitOutcome::Landed`].
    pub journal_fault: Option<String>,
}

// ---------------------------------------------------------------------------
// The transaction
// ---------------------------------------------------------------------------

/// A coordination transaction: locks, journal, staging, commit, recovery.
///
/// Construction goes through [`CoordTransaction::acquire`]. Dropping an
/// uncommitted transaction restores every artifact this transaction wrote (from
/// in-process byte snapshots only — never `git checkout --`) and marks its
/// journal intent abandoned while the locks are still held.
#[derive(Debug)]
pub struct CoordTransaction<'a> {
    workspace: &'a MissionWorkspace,
    repository: sk_git::GitRepository,
    surface: TopologySurface,
    state_before: CoordState,
    establishment: CoordEstablishment,
    worktree_root: PathBuf,
    feature_dir: PathBuf,
    destination: BranchName,
    operation: String,
    protected: ProtectedBranches,
    journal: CoordJournal,
    journal_intent: CoordRecord,
    observed_tip: Option<Oid>,
    staged: Vec<GitPath>,
    expected_bytes: BTreeMap<String, Vec<u8>>,
    snapshots: BTreeMap<PathBuf, Option<Vec<u8>>>,
    committed: bool,
    seed: Option<CoordSeedReceipt>,
    /// Held for the transaction's lifetime (claim first, then Mission lock).
    _locks: CoordLocks,
    proof: OwnershipProof,
}

impl<'a> CoordTransaction<'a> {
    /// Acquire the transaction for `operation`, establishing the write surface.
    ///
    /// Order: read-only surface resolution → protected-destination refusal →
    /// checkout claim and Mission lock → materialization/seeding (each with its
    /// own journal record) → commit intent → ownership proof.
    pub fn acquire(
        workspace: &'a MissionWorkspace,
        operation: &str,
        policy: &ProtectionPolicy,
    ) -> Result<Self, WorkspaceError> {
        Self::acquire_with_timeout(workspace, operation, policy, COORD_TRANSACTION_LOCK_TIMEOUT)
    }

    /// [`CoordTransaction::acquire`] with an explicit lock bound.
    pub fn acquire_with_timeout(
        workspace: &'a MissionWorkspace,
        operation: &str,
        policy: &ProtectionPolicy,
        timeout: Duration,
    ) -> Result<Self, WorkspaceError> {
        let protected = policy.branch_set()?;
        let primary_root = workspace.workspace().primary_root().to_path_buf();
        let common_dir = workspace.workspace().common_dir().to_path_buf();

        if !routes_through_coordination(workspace.topology()) {
            let destination = workspace.write_branch(&protected)?;
            ensure_unprotected(&destination, &protected, operation)?;
            let locks = CoordLocks::acquire(
                &primary_root,
                workspace.mission_dir(),
                &primary_root,
                &common_dir,
                timeout,
            )?;
            let repository = sk_git::GitRepository::open(&primary_root)?;
            let observed_tip = repository.read_branch_tip(&destination)?;
            ensure_head_is_destination(&repository, &destination, &primary_root)?;
            let proof = ownership_proof(
                workspace,
                TopologySurface::Primary,
                &destination,
                &primary_root,
                &locks,
                observed_tip.clone(),
                &repository,
            );
            let journal = CoordJournal::new(&common_dir, workspace.slug());
            let intent = CoordRecord::intent(
                CoordRecordKind::Commit,
                workspace.slug(),
                operation,
                Some(destination.as_str()),
                Some(&primary_root),
                observed_tip.as_ref(),
            );
            journal.begin(intent.clone())?;
            return Ok(Self {
                workspace,
                repository,
                surface: TopologySurface::Primary,
                state_before: CoordState::None,
                establishment: CoordEstablishment::Primary,
                worktree_root: primary_root.clone(),
                feature_dir: workspace.mission_dir().to_path_buf(),
                destination,
                operation: operation.to_owned(),
                protected,
                journal,
                journal_intent: intent,
                observed_tip,
                staged: Vec::new(),
                expected_bytes: BTreeMap::new(),
                snapshots: BTreeMap::new(),
                committed: false,
                seed: None,
                _locks: locks,
                proof,
            });
        }

        // Coordination-routed mission: resolve the destination (declared, or
        // derived from the canonical grammar when the record was lost) and gate
        // it before any lock, journal, or filesystem mutation.
        let probe = workspace.probe_coordination()?;
        let destination = coordination_destination(workspace, &probe)?;
        ensure_unprotected(&destination.branch, &protected, operation)?;
        let locks = CoordLocks::acquire(
            &destination.worktree,
            workspace.mission_dir(),
            &primary_root,
            &common_dir,
            timeout,
        )?;
        let journal = CoordJournal::new(&common_dir, workspace.slug());
        // Re-probe under the locks. The first probe chose the refusal shape; a
        // concurrent process may have materialized the surface between that
        // probe and the claim, and acting on the stale answer turned its
        // `Materialize` into a spurious Git error (it never adopted it).
        let probe = workspace.probe_coordination()?;
        establish(
            workspace,
            operation,
            protected,
            destination,
            probe,
            journal,
            locks,
        )
    }

    /// The ownership evidence for this write.
    pub fn proof(&self) -> &OwnershipProof {
        &self.proof
    }

    /// The surface being written.
    pub fn surface(&self) -> TopologySurface {
        self.surface
    }

    /// The checkout the write lands in.
    pub fn worktree_root(&self) -> &Path {
        &self.worktree_root
    }

    /// The destination branch.
    pub fn destination(&self) -> &BranchName {
        &self.destination
    }

    /// The mission directory the transaction writes into.
    pub fn feature_dir(&self) -> &Path {
        &self.feature_dir
    }

    /// The coordination state observed before the transaction.
    pub fn state_before(&self) -> CoordState {
        self.state_before
    }

    /// The seed operation performed during acquire, when any.
    pub fn seed(&self) -> Option<&CoordSeedReceipt> {
        self.seed.as_ref()
    }

    /// The journal this transaction records into.
    pub fn journal(&self) -> &CoordJournal {
        &self.journal
    }

    /// Write `bytes` to `relative` inside the write checkout and stage it.
    ///
    /// The path is confined to the worktree (traversal, absolute escapes, and
    /// symlinked parents leaving it are refused), the previous bytes are
    /// snapshotted for rollback, and the exact bytes are pinned into the commit
    /// so a concurrent writer cannot swap the content under it.
    pub fn write_artifact(
        &mut self,
        relative: &Path,
        bytes: &[u8],
    ) -> Result<PathBuf, WorkspaceError> {
        let target = sk_kernel::resolve_within(&self.worktree_root, relative)?;
        if is_nested_worktrees_path(&self.worktree_root, &target) {
            return Err(WorkspaceError::CoordRefused {
                code: "COORD_ARTIFACT_NESTED_WORKTREE",
                message: format!(
                    "refusing to write {} through {}: a coordination artifact may not live under another checkout's .worktrees/",
                    target.display(),
                    self.worktree_root.display()
                ),
            });
        }
        if !self.snapshots.contains_key(&target) {
            let previous = read_optional(&target, MAX_SNAPSHOT_BYTES)?;
            self.snapshots.insert(target.clone(), previous);
        }
        sk_kernel::atomic_write_with_options(
            &target,
            bytes,
            sk_kernel::AtomicWriteOptions {
                create_parents: true,
            },
        )?;
        let git_path = sk_git::normalize_safe_commit_path(&self.worktree_root, &target)?;
        let key = git_path.display_lossy();
        if !self.staged.contains(&git_path) {
            self.staged.push(git_path);
        }
        self.expected_bytes.insert(key, bytes.to_vec());
        Ok(target)
    }

    /// Stage a path the caller wrote out of band.
    ///
    /// **Contract:** this stages bytes the transaction never observed,
    /// snapshotted, or wrote, so an abort/`NotLanded` restores nothing for it and
    /// the commit carries only the pinned parent (not pinned bytes for this
    /// path). The caller owns the file. [`Self::write_artifact`] is the owned
    /// path: it confirms, snapshots and pins the bytes. Upstream's `stage_path`
    /// has the same contract, so this is parity rather than an ownership claim.
    pub fn stage_path(&mut self, path: &Path) -> Result<(), WorkspaceError> {
        let git_path = sk_git::normalize_safe_commit_path(&self.worktree_root, path)?;
        if !path.exists() {
            return Err(WorkspaceError::CoordRefused {
                code: "COORD_STAGE_MISSING",
                message: format!("cannot stage {}: it does not exist", path.display()),
            });
        }
        if !self.staged.contains(&git_path) {
            self.staged.push(git_path);
        }
        Ok(())
    }

    /// The staged repository-relative paths.
    pub fn staged_paths(&self) -> Vec<String> {
        self.staged
            .iter()
            .map(sk_git::GitPath::display_lossy)
            .collect()
    }

    /// Commit the staged paths.
    ///
    /// Consumes the transaction: the journals, locks, and any rollback are then
    /// resolved exactly once.
    pub fn commit(mut self, message: &str) -> Result<CoordReceipt, WorkspaceError> {
        let report = self.commit_staged(message)?;
        self.committed = true;
        Ok(CoordReceipt {
            outcome: report.outcome,
            proof: self.proof.clone(),
            state_before: self.state_before,
            establishment: self.establishment,
            seed: self.seed.clone(),
            journal_path: self.journal.path().to_path_buf(),
            journal_fault: report.journal_fault,
        })
    }

    /// Abandon the transaction without committing, restoring written artifacts.
    pub fn abort(mut self, detail: &str) {
        self.abandon(detail);
        self.committed = true;
    }

    /// Finish a transaction whose purpose was surface establishment, not a commit.
    ///
    /// The commit intent is resolved as applied with the supplied explanation:
    /// materialization/seeding already succeeded, and no artifact commit was ever
    /// requested. Using this instead of dropping keeps the journal free of a
    /// misleading "abandoned commit" record.
    pub fn seal(mut self, detail: &str) -> Result<(), WorkspaceError> {
        self.journal.resolve(
            &self.journal_intent,
            CoordPhase::Applied,
            Some(detail),
            None,
        )?;
        self.committed = true;
        Ok(())
    }

    /// Resolve this transaction's commit intent, recording a journal fault.
    ///
    /// A journal write is *reporting*, not the mutation. When the mutation's
    /// outcome is already proven, a journal failure must not overwrite it: the
    /// record stays pending (so recovery can still reconcile it) and the fault
    /// is handed back to the caller as evidence.
    fn resolve_intent(
        &self,
        phase: CoordPhase,
        detail: Option<&str>,
        candidate: Option<&Oid>,
        fault: &mut Option<String>,
    ) {
        if let Err(error) = self
            .journal
            .resolve(&self.journal_intent, phase, detail, candidate)
            && fault.is_none()
        {
            *fault = Some(error.to_string());
        }
    }

    fn commit_staged(&mut self, message: &str) -> Result<CommitReport, WorkspaceError> {
        if self.staged.is_empty() {
            return Err(WorkspaceError::CoordRefused {
                code: "COORD_EMPTY_CHANGESET",
                message: format!(
                    "{} transaction for mission {} staged no paths; refusing an empty commit",
                    self.operation,
                    self.workspace.slug()
                ),
            });
        }
        let mut request =
            SafeCommitRequest::new(self.destination.clone(), message, self.staged.clone())
                .with_protection(self.protected.clone(), CommitGuardCapability::Standard);
        if let Some(parent) = &self.observed_tip {
            request = request.with_expected_parent(parent.clone());
            let expected: Vec<ExpectedBlob> = self
                .staged
                .iter()
                .filter_map(|path| {
                    self.expected_bytes
                        .get(&path.display_lossy())
                        .map(|bytes| ExpectedBlob::new(path.clone(), bytes.clone()))
                })
                .collect();
            if !expected.is_empty() {
                request = request.with_expected_bytes(expected);
            }
        }

        let mut fault: Option<String> = None;
        match self.repository.safe_commit(&request) {
            Ok(outcome) => {
                let landed = LandedCommit {
                    commit: outcome.commit().clone(),
                    destination: outcome.destination().as_str().to_owned(),
                    worktree_root: outcome.worktree_root().to_path_buf(),
                    committed_paths: outcome
                        .committed_paths()
                        .iter()
                        .map(sk_git::GitPath::display_lossy)
                        .collect(),
                    observed_tip: self.observed_tip.clone(),
                    index_repair: None,
                };
                self.resolve_intent(CoordPhase::Applied, None, Some(&landed.commit), &mut fault);
                Ok(CommitReport {
                    outcome: CoordCommitOutcome::Landed(landed),
                    journal_fault: fault,
                })
            }
            Err(GitError::SafeCommitNothingToCommit { detail, .. }) => {
                let head = match self.repository.read_branch_tip(&self.destination)? {
                    Some(head) => head,
                    None => {
                        return Err(WorkspaceError::CoordRefused {
                            code: "COORD_NOOP_WITHOUT_TIP",
                            message: format!(
                                "safe commit reported a no-op on {} but the branch has no tip: {detail}",
                                self.destination
                            ),
                        });
                    }
                };
                self.resolve_intent(CoordPhase::Applied, Some(&detail), None, &mut fault);
                Ok(CommitReport {
                    outcome: CoordCommitOutcome::NoOp {
                        head,
                        destination: self.destination.as_str().to_owned(),
                    },
                    journal_fault: fault,
                })
            }
            Err(GitError::SafeCommitLanded {
                destination,
                worktree_root,
                detail,
                recovery,
                ..
            }) => {
                let landed = LandedCommit {
                    commit: recovery.commit().clone(),
                    destination,
                    worktree_root,
                    committed_paths: recovery
                        .affected_paths()
                        .iter()
                        .map(sk_git::GitPath::display_lossy)
                        .collect(),
                    observed_tip: self.observed_tip.clone(),
                    index_repair: Some(detail.clone()),
                };
                self.resolve_intent(
                    CoordPhase::Applied,
                    Some(&format!("landed with index repair required: {detail}")),
                    Some(&landed.commit),
                    &mut fault,
                );
                Ok(CommitReport {
                    outcome: CoordCommitOutcome::Landed(landed),
                    journal_fault: fault,
                })
            }
            Err(GitError::SafeCommitAdvanceUnconfirmed {
                candidate,
                observed_tip,
                detail,
                ..
            }) => {
                let current = self.repository.read_branch_tip(&self.destination)?;
                if current.as_ref() == Some(&candidate) {
                    let landed = LandedCommit {
                        commit: candidate,
                        destination: self.destination.as_str().to_owned(),
                        worktree_root: self.worktree_root.clone(),
                        committed_paths: self
                            .staged
                            .iter()
                            .map(sk_git::GitPath::display_lossy)
                            .collect(),
                        observed_tip: self.observed_tip.clone(),
                        index_repair: Some(detail.clone()),
                    };
                    self.resolve_intent(
                        CoordPhase::Applied,
                        Some(&format!("advance confirmed by readback: {detail}")),
                        Some(&landed.commit),
                        &mut fault,
                    );
                    return Ok(CommitReport {
                        outcome: CoordCommitOutcome::Landed(landed),
                        journal_fault: fault,
                    });
                }
                if current == self.observed_tip {
                    // Nothing landed: the branch is exactly where it was. The
                    // artifacts this transaction wrote are restored from their
                    // snapshots, because the write they belong to did not happen.
                    self.resolve_intent(
                        CoordPhase::Abandoned,
                        Some(&detail),
                        Some(&candidate),
                        &mut fault,
                    );
                    self.restore_snapshots();
                    return Ok(CommitReport {
                        outcome: CoordCommitOutcome::NotLanded {
                            candidate,
                            observed_tip: current,
                            detail,
                        },
                        journal_fault: fault,
                    });
                }
                self.resolve_intent(
                    CoordPhase::Indeterminate,
                    Some(&format!(
                        "advance unconfirmed; branch tip is {:?}, substrate reported {:?}: {detail}",
                        current.as_ref().map(ToString::to_string),
                        observed_tip.as_ref().map(ToString::to_string)
                    )),
                    Some(&candidate),
                    &mut fault,
                );
                Ok(CommitReport {
                    outcome: CoordCommitOutcome::Indeterminate {
                        candidate,
                        observed_tip: current,
                        detail,
                    },
                    journal_fault: fault,
                })
            }
            Err(error) => {
                self.resolve_intent(
                    CoordPhase::Abandoned,
                    Some(&error.to_string()),
                    None,
                    &mut fault,
                );
                self.restore_snapshots();
                let error = WorkspaceError::Git(error);
                match fault {
                    Some(fault) => Err(WorkspaceError::CoordRefused {
                        code: "COORD_COMMIT_FAILED_WITH_JOURNAL_FAULT",
                        message: format!(
                            "{error}; the coordination journal could not record the failed attempt either: {fault}"
                        ),
                    }),
                    None => Err(error),
                }
            }
        }
    }

    fn abandon(&mut self, detail: &str) {
        let _ = self.journal.resolve(
            &self.journal_intent,
            CoordPhase::Abandoned,
            Some(detail),
            None,
        );
        self.restore_snapshots();
    }

    fn restore_snapshots(&mut self) {
        let snapshots = std::mem::take(&mut self.snapshots);
        for (path, previous) in snapshots {
            match previous {
                Some(bytes) => {
                    if let Err(error) = sk_kernel::atomic_write_with_options(
                        &path,
                        &bytes,
                        sk_kernel::AtomicWriteOptions {
                            create_parents: true,
                        },
                    ) {
                        // Best effort: the operator sees the transaction failure,
                        // not a second panic from the rollback path.
                        let _ = error;
                    }
                }
                None => {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
    }
}

impl Drop for CoordTransaction<'_> {
    fn drop(&mut self) {
        if !self.committed {
            self.abandon("transaction dropped without commit");
        }
    }
}

// ---------------------------------------------------------------------------
// Materialization and seeding
// ---------------------------------------------------------------------------

/// The branch and write checkout a coordination-routed mission mutates.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CoordDestination {
    /// The branch name to write (declared, or derived from the grammar).
    pub branch_text: String,
    /// The validated branch.
    pub branch: BranchName,
    /// The checkout the write lands in.
    pub worktree: PathBuf,
    /// `true` when `meta.json` lost the record and the canonical grammar
    /// recovered the very branch that exists in Git.
    pub derived: bool,
}

/// Resolve the coordination destination, recovering a lost `meta.json` record.
///
/// The reference never degrades a coordination-routed topology to the primary
/// checkout merely because `coordination_branch` is missing: it derives the
/// canonical name from the same naming grammar the mint used and, **only if
/// that branch exists**, proceeds exactly as if it had been declared
/// (`_derive_branch_for_undeclared`). When it does not exist the answer is a
/// typed `COORD_BRANCH_UNDECLARED_AND_ABSENT` refusal, never a silent primary
/// write and never a "deleted" data-loss label for a branch nobody ever made.
pub(crate) fn coordination_destination(
    workspace: &MissionWorkspace,
    probe: &crate::workspace::CoordinationProbe,
) -> Result<CoordDestination, WorkspaceError> {
    let worktree = workspace
        .coord_worktree_path()
        .ok_or_else(|| WorkspaceError::CoordRefused {
            code: "COORDINATION_WORKTREE_UNRESOLVED",
            message: format!(
                "mission {} routes through coordination but has no canonical worktree path",
                workspace.slug()
            ),
        })?;
    // `probe.branch` already carries the canonically DERIVED name when the
    // record was lost, so a live derived branch is usable exactly like a
    // declared one.
    let Some(branch_text) = workspace
        .coordination_branch()
        .or_else(|| probe.branch.clone())
    else {
        return Err(WorkspaceError::CoordRefused {
            code: "COORD_BRANCH_UNDECLARED_AND_ABSENT",
            message: format!(
                "mission {} stores coordination topology {} but records no coordination_branch, and the canonical branch does not exist; refusing to guess a coordination surface",
                workspace.slug(),
                workspace.topology().as_str()
            ),
        });
    };
    let branch = BranchName::parse(&branch_text)?;
    if probe.state == CoordState::Deleted {
        if probe.derived {
            return Err(WorkspaceError::CoordRefused {
                code: "COORD_BRANCH_UNDECLARED_AND_ABSENT",
                message: format!(
                    "mission {} records no coordination_branch, and the canonical branch {branch_text} does not exist in this repository or on any remote; refusing to guess a coordination surface",
                    workspace.slug()
                ),
            });
        }
        return Err(WorkspaceError::CoordinationBranchDeleted {
            mission: workspace.slug().to_owned(),
            branch: branch_text,
        });
    }
    Ok(CoordDestination {
        branch_text,
        branch,
        worktree,
        derived: probe.derived,
    })
}

/// Where a coordination-aware write of one artifact kind must land.
///
/// This is the write **location** answer, not a transaction: it carries the
/// Mission directory to write into, the checkout that holds it, the surface
/// stamp, and the evidence of what establishing the surface actually did. It is
/// the value [`establish_write_location`] returns so a caller that only needs
/// the location (a status bootstrap, a tracer append) does not have to open a
/// commit transaction it will not commit.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use = "the location is only meaningful once the surface has been established"]
pub struct CoordWriteLocation {
    /// Absolute Mission directory the write must land in.
    pub path: PathBuf,
    /// The checkout holding [`CoordWriteLocation::path`].
    pub surface_root: PathBuf,
    /// Which partition the write targets.
    pub surface: TopologySurface,
    /// The coordination state observed before establishment (`None` for primary).
    pub state_before: CoordState,
    /// What establishing the surface did.
    pub establishment: CoordEstablishment,
    /// The seed operation, when one ran.
    pub seed: Option<CoordSeedReceipt>,
}

impl CoordWriteLocation {
    /// The declared PRIMARY location: no probe, no lock, no side effect.
    fn primary(workspace: &MissionWorkspace) -> Self {
        Self {
            path: workspace.mission_dir().to_path_buf(),
            surface_root: workspace.workspace().primary_root().to_path_buf(),
            surface: TopologySurface::Primary,
            state_before: CoordState::None,
            establishment: CoordEstablishment::Primary,
            seed: None,
        }
    }
}

/// Resolve, materialize, seed or refuse the Mission directory a write of
/// `kind` must land in.
///
/// This is the write-side placement accessor `mission_runtime
/// .PlacementSeam.write_dir` is: the reference's `finalize-tasks` calls
/// `placement_seam(root, slug).write_dir(STATUS_STATE)` to establish the
/// coordination surface **before** its status bootstrap writes, and reads the
/// result only for its side effect. Native callers have no implicit
/// surface-resolving status writer, so they need the resolved directory too.
///
/// Resolution order mirrors the reference, and every arm but the last is
/// side-effect free:
///
/// * a PRIMARY-partition kind, or any kind on a coordination-less topology →
///   the declared primary Mission directory, with **no** coordination probe,
///   lock, journal, or file;
/// * a published/E2 short-circuit kind on a `published` Mission → the primary
///   directory, without probing coordination, so a torn-down coordination
///   branch cannot turn a post-consolidation bookkeeping write into a refusal;
/// * every other COORD-partition kind on a coordination-routing topology →
///   [`crate::coord::establish`]'s materialize/seed, under the one claim +
///   Mission lock pair, returning the coordination Mission directory. A
///   deleted branch, an unmaterialized branch that exists only on a remote, a
///   stray directory, a worktree on the wrong branch, and a forked seed all
///   refuse exactly as the transaction path does — this never falls back to the
///   primary checkout for a coordination write.
///
/// `phase` is the caller's canonically derived
/// [`LifecyclePhase`](crate::lifecycle::LifecyclePhase); it is only consulted
/// for the published short-circuit, which is why it is required rather than
/// optional — a caller that cannot prove `Published` must pass its real phase.
pub fn establish_write_location(
    workspace: &MissionWorkspace,
    kind: MissionArtifactKind,
    phase: LifecyclePhase,
    policy: &ProtectionPolicy,
) -> Result<CoordWriteLocation, WorkspaceError> {
    if kind.is_primary_partition() || !routes_through_coordination(workspace.topology()) {
        return Ok(CoordWriteLocation::primary(workspace));
    }
    if phase == LifecyclePhase::Published && is_published_write_short_circuit_kind(kind) {
        return Ok(CoordWriteLocation::primary(workspace));
    }

    let protected = policy.branch_set()?;
    let primary_root = workspace.workspace().primary_root().to_path_buf();
    let common_dir = workspace.workspace().common_dir().to_path_buf();

    // Same order as `CoordTransaction::acquire`: resolve the destination from a
    // read-only probe, take the claim then the Mission lock, and re-probe under
    // them so a surface another process materialized between the probe and the
    // claim is adopted instead of materialized twice.
    let probe = workspace.probe_coordination()?;
    let destination = coordination_destination(workspace, &probe)?;
    let locks = CoordLocks::acquire(
        &destination.worktree,
        workspace.mission_dir(),
        &primary_root,
        &common_dir,
        COORD_TRANSACTION_LOCK_TIMEOUT,
    )?;
    let journal = CoordJournal::new(&common_dir, workspace.slug());
    let probe = workspace.probe_coordination()?;
    let established = establish_surface(workspace, &protected, destination, &probe, &journal)?;
    drop(locks);
    Ok(CoordWriteLocation {
        path: established.feature_dir,
        surface_root: established.worktree_root,
        surface: TopologySurface::Coord,
        state_before: established.state_before,
        establishment: established.establishment,
        seed: established.seed,
    })
}

/// The surface one establishment produced, before any commit intent exists.
///
/// Shared by [`CoordTransaction::acquire`] (which then mints the commit intent)
/// and [`establish_write_location`] (which returns it as-is), so a
/// materialization/seed can never be implemented twice with different ordering.
pub(crate) struct EstablishedSurface {
    /// The branch the establishment targeted.
    pub destination_branch: BranchName,
    /// The checkout the write lands in.
    pub worktree_root: PathBuf,
    /// The Mission directory inside that checkout.
    pub feature_dir: PathBuf,
    /// The coordination state observed before establishment.
    pub state_before: CoordState,
    /// What establishment did.
    pub establishment: CoordEstablishment,
    /// The seed operation, when one ran.
    pub seed: Option<CoordSeedReceipt>,
}

/// Materialize/seed the resolved coordination surface and mint the commit intent.
///
/// Called with the claim + Mission locks already held, by both
/// [`CoordTransaction::acquire`] and [`bootstrap_coordination`], so the two
/// paths cannot drift on ordering: a bootstrap mint therefore happens under the
/// same authority as the writes that follow it.
fn establish<'a>(
    workspace: &'a MissionWorkspace,
    operation: &str,
    protected: ProtectedBranches,
    destination: CoordDestination,
    probe: crate::workspace::CoordinationProbe,
    journal: CoordJournal,
    locks: CoordLocks,
) -> Result<CoordTransaction<'a>, WorkspaceError> {
    let established = establish_surface(workspace, &protected, destination, &probe, &journal)?;
    begin_commit_intent(workspace, operation, protected, established, journal, locks)
}

/// The materialize/seed half of [`establish`], with no commit intent.
fn establish_surface(
    workspace: &MissionWorkspace,
    protected: &ProtectedBranches,
    destination: CoordDestination,
    probe: &crate::workspace::CoordinationProbe,
    journal: &CoordJournal,
) -> Result<EstablishedSurface, WorkspaceError> {
    let CoordDestination {
        branch_text,
        branch: destination_branch,
        worktree: coord_root,
        ..
    } = destination;
    let mut establishment = match probe.state {
        CoordState::Materialized => CoordEstablishment::Reused,
        _ => CoordEstablishment::Materialized,
    };
    let mut state_before = probe.state;

    let materialized_root = match probe.state {
        CoordState::Materialized => coord_root.clone(),
        CoordState::Empty => {
            // The canonical root exists and is registered on the declared
            // branch, but its Mission directory is absent. The seed step
            // below records its own intent.
            if probe.worktree.is_none() {
                return Err(WorkspaceError::CoordRefused {
                    code: "COORDINATION_WORKTREE_UNREGISTERED",
                    message: format!(
                        "{} exists but Git does not register it on {}; refusing to seed a stray directory",
                        coord_root.display(),
                        branch_text
                    ),
                });
            }
            coord_root.clone()
        }
        CoordState::Unmaterialized => {
            if let Some(stray) = probe.stray_directory.as_ref() {
                // A directory at the canonical path that Git does not register
                // is a husk of something else. Adopting it (the reference's
                // path-only probe would seed into it) or deleting it are both
                // wrong, and `git worktree add` would fail untyped.
                return Err(WorkspaceError::CoordRefused {
                    code: "COORDINATION_WORKTREE_STRAY_DIRECTORY",
                    message: format!(
                        "{} exists but Git does not register it on {branch_text}; refusing to adopt, seed or delete a directory this store did not create",
                        stray.display()
                    ),
                });
            }
            if !probe.local_head {
                return Err(WorkspaceError::CoordRefused {
                    code: "COORDINATION_BRANCH_NOT_LOCAL",
                    message: format!(
                        "coordination branch {branch_text} resolves only on a remote; a remote-tracking branch cannot be attached as a local worktree"
                    ),
                });
            }
            let repository = workspace.workspace().repository().clone();
            materialize_worktree(&repository, &branch_text, &coord_root, journal, workspace)?;
            state_before = CoordState::Unmaterialized;
            establishment = CoordEstablishment::Materialized;
            coord_root.clone()
        }
        CoordState::None | CoordState::Deleted => {
            return Err(WorkspaceError::CoordinationBranchDeleted {
                mission: workspace.slug().to_owned(),
                branch: branch_text,
            });
        }
    };

    // The worktree is now registered, so the state is either Materialized
    // (mission dir present) or Empty (a seed ticket).
    let feature_dir = materialized_root
        .join(crate::workspace::SPECS_DIR_NAME)
        .join(workspace.coord_mission_dir_name());
    let mut seed = None;
    if !feature_dir.exists() {
        seed = Some(seed_coordination_surface(
            workspace,
            &materialized_root,
            &feature_dir,
            &branch_text,
            protected,
            journal,
            state_before,
        )?);
        establishment = CoordEstablishment::Seeded;
    } else if establishment == CoordEstablishment::Materialized
        && state_before == CoordState::Unmaterialized
    {
        // Materialization produced a full surface: nothing to seed.
        establishment = CoordEstablishment::Noop;
    }

    Ok(EstablishedSurface {
        destination_branch,
        worktree_root: materialized_root,
        feature_dir,
        state_before,
        establishment,
        seed,
    })
}

/// Mint the commit intent for an already established coordination surface.
///
/// The proof and the journal intent belong to the *transaction*, not to the
/// establishment: [`establish_write_location`] establishes a surface without
/// promising a commit, and must not leave a commit record behind that recovery
/// would then have to adjudicate.
fn begin_commit_intent<'a>(
    workspace: &'a MissionWorkspace,
    operation: &str,
    protected: ProtectedBranches,
    established: EstablishedSurface,
    journal: CoordJournal,
    locks: CoordLocks,
) -> Result<CoordTransaction<'a>, WorkspaceError> {
    let EstablishedSurface {
        destination_branch,
        worktree_root: materialized_root,
        feature_dir,
        state_before,
        establishment,
        seed,
    } = established;
    let repository = sk_git::GitRepository::open(&materialized_root)?;
    ensure_head_is_destination(&repository, &destination_branch, &materialized_root)?;
    let observed_tip = repository.read_branch_tip(&destination_branch)?;
    let proof = ownership_proof(
        workspace,
        TopologySurface::Coord,
        &destination_branch,
        &materialized_root,
        &locks,
        observed_tip.clone(),
        &repository,
    );
    let intent = CoordRecord::intent(
        CoordRecordKind::Commit,
        workspace.slug(),
        operation,
        Some(destination_branch.as_str()),
        Some(&materialized_root),
        observed_tip.as_ref(),
    );
    journal.begin(intent.clone())?;
    Ok(CoordTransaction {
        workspace,
        repository,
        surface: TopologySurface::Coord,
        state_before,
        establishment,
        worktree_root: materialized_root,
        feature_dir,
        destination: destination_branch,
        operation: operation.to_owned(),
        protected,
        journal,
        journal_intent: intent,
        observed_tip,
        staged: Vec::new(),
        expected_bytes: BTreeMap::new(),
        snapshots: BTreeMap::new(),
        committed: false,
        seed,
        _locks: locks,
        proof,
    })
}

fn materialize_worktree(
    repository: &sk_git::GitRepository,
    branch: &str,
    path: &Path,
    journal: &CoordJournal,
    workspace: &MissionWorkspace,
) -> Result<(), WorkspaceError> {
    let branch_name = BranchName::parse(branch)?;
    let observed_tip = repository.read_branch_tip(&branch_name)?;
    let intent = CoordRecord::intent(
        CoordRecordKind::Materialize,
        workspace.slug(),
        "materialize_coordination_worktree",
        Some(branch),
        Some(path),
        observed_tip.as_ref(),
    );
    journal.begin(intent.clone())?;
    let plan = match repository.plan_worktree_add(
        path.to_path_buf(),
        &branch_name,
        sk_git::WorktreeStart::ExistingBranch,
    ) {
        Ok(plan) => plan,
        Err(error) => {
            journal.resolve(
                &intent,
                CoordPhase::Abandoned,
                Some(&error.to_string()),
                None,
            )?;
            return Err(WorkspaceError::Git(error));
        }
    };
    match repository.apply_worktree_add(&plan) {
        Ok(_) => {
            journal.resolve(&intent, CoordPhase::Applied, None, None)?;
            Ok(())
        }
        Err(error) => {
            journal.resolve(
                &intent,
                CoordPhase::Abandoned,
                Some(&error.to_string()),
                None,
            )?;
            Err(WorkspaceError::Git(error))
        }
    }
}

/// The append-only event logs the seed prefix-merges rather than overwrites.
///
/// These are the two files the reference treats as streams
/// (`coord_seed._LOG_FILENAMES`): every other COORD-kind file is carried whole
/// or kept from the coordination copy.
const SEED_LOG_FILENAMES: [&str; 2] = [sk_status::EVENTS_FILENAME, "decisions.events.jsonl"];

/// Seed an empty coordination surface from the primary mission directory.
///
/// This is the reference's seed, not a byte copy:
///
/// 1. **Restore from the branch tip first.** Every COORD-kind file the
///    coordination branch already carries is materialized into the worktree, so
///    a post-fix `EMPTY` surface (the Mission directory was removed from an
///    otherwise seeded worktree) is restored from committed content instead of
///    being re-forked.
/// 2. **Validate and prefix-merge the event logs.** Both logs are decoded
///    strictly as UTF-8 (a non-UTF-8 byte is a pathful refusal, never a lossy
///    re-encode), parsed as JSONL, each row must be an object carrying
///    `event_id`, no `event_id` may repeat within one stream, and the root
///    stream must be a *prefix extension* of the coordination stream. A fork is
///    refused (`COORD_SEED_FORK_REFUSED`) naming the first divergent pair; a
///    malformed or duplicate row is refused rather than silently dropped.
/// 3. **Carry root-only non-log files**, keeping the coordination copy when both
///    exist and differ (with a warning), never overwriting committed content.
/// 4. **Commit every COORD-kind file now on disk** with the interop trailer
///    (`Spec-Kitty-Coordination-Seed: <mission_id>`), then **restore the root
///    checkout's single home**: an *untracked* root copy is removed only when
///    the committed coordination content reproduces its bytes (proven
///    duplicate). An untracked copy that differs is authored divergence — it is
///    kept and named in the receipt's `preserved_root`. A staged-new or
///    tracked-modified root copy is preserved and reported too, because
///    discarding an operator's uncommitted bytes is not a recovery step this
///    crate performs.
/// 5. **Leave no authoritative husk.** Any failure — including a refused seed
///    commit — removes the Mission directory this seed created, so
///    [`MissionWorkspace::probe_coordination`] keeps answering `Empty` (a seed
///    ticket) instead of `Materialized` over missing content.
fn seed_coordination_surface(
    workspace: &MissionWorkspace,
    worktree_root: &Path,
    feature_dir: &Path,
    branch: &str,
    protected: &ProtectedBranches,
    journal: &CoordJournal,
    state_before: CoordState,
) -> Result<CoordSeedReceipt, WorkspaceError> {
    let repository = sk_git::GitRepository::open(worktree_root)?;
    let branch_name = BranchName::parse(branch)?;
    let branch_ref =
        sk_git::StartPoint::Ref(sk_git::RefName::parse(branch_name.as_ref_name().as_str())?);
    let observed_tip = repository.read_branch_tip(&branch_name)?;
    let mission_dir_name = workspace.coord_mission_dir_name();

    // 1. Everything COORD-kind the branch tip already carries.
    let mut restored_from_branch: Vec<String> = Vec::new();
    let mut from_tip: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    if observed_tip.is_some() {
        let prefix = git_path(&format!(
            "{}/{mission_dir_name}",
            crate::workspace::SPECS_DIR_NAME
        ))?;
        for path in repository.tree_paths_at(&branch_ref, Some(&prefix))? {
            let Some(relative) = mission_relative(&path.display_lossy(), &mission_dir_name) else {
                continue;
            };
            let lookup = format!(
                "{}/{mission_dir_name}/{relative}",
                crate::workspace::SPECS_DIR_NAME
            );
            let Some(kind) = kind_for_mission_file(&lookup, Some(&mission_dir_name)) else {
                continue;
            };
            if !is_seedable_kind(kind) {
                continue;
            }
            let bytes = repository
                .read_blob_at_bounded(&branch_ref, &path, MAX_SEED_FILE_BYTES)?
                .ok_or_else(|| WorkspaceError::CoordRefused {
                    code: "COORD_SEED_TIP_UNREADABLE",
                    message: format!(
                        "{} is listed at {branch} but its blob could not be read; refusing to seed over content this store cannot reproduce",
                        path.display_lossy()
                    ),
                })?;
            from_tip.insert(relative.clone(), bytes);
            restored_from_branch.push(relative);
        }
    }

    // 2/3. Root-checkout content, per partition and per stream.
    let source_dir = workspace.mission_dir().to_path_buf();
    let mut root_bytes: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for entry in walk_files(&source_dir)? {
        let relative =
            entry
                .strip_prefix(&source_dir)
                .map_err(|_| WorkspaceError::InvalidArtifact {
                    artifact: "coordination seed",
                    detail: format!("{} is not inside {}", entry.display(), source_dir.display()),
                })?;
        let relative_text = to_posix(relative);
        let lookup = format!(
            "{}/{mission_dir_name}/{relative_text}",
            crate::workspace::SPECS_DIR_NAME
        );
        let Some(kind) = kind_for_mission_file(&lookup, Some(&mission_dir_name)) else {
            continue;
        };
        if !is_seedable_kind(kind) {
            continue;
        }
        root_bytes.insert(
            relative_text,
            sk_kernel::read_bounded(&entry, MAX_SEED_FILE_BYTES)?,
        );
    }

    let mut merged: BTreeMap<String, Vec<u8>> = from_tip.clone();
    let mut seeded_paths: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    for name in SEED_LOG_FILENAMES {
        let root_text = match root_bytes.get(name) {
            Some(bytes) => {
                decode_seed_text(bytes, &source_dir.join(name), "root-checkout event log")?
            }
            None => String::new(),
        };
        let coord_text = match merged.get(name) {
            Some(bytes) => {
                decode_seed_text(bytes, &feature_dir.join(name), "coordination event log")?
            }
            None => String::new(),
        };
        let root_lines = non_blank_lines(&root_text);
        let coord_lines = non_blank_lines(&coord_text);
        let root_ids = event_ids_of(&root_lines, name, "root checkout")?;
        let coord_ids = event_ids_of(&coord_lines, name, branch)?;
        match classify_prefix(&root_ids, &coord_ids) {
            PrefixVerdict::Nothing if coord_lines.is_empty() && root_lines.is_empty() => continue,
            PrefixVerdict::Nothing => continue,
            PrefixVerdict::CarryTail { tail_start } => {
                let mut bytes = Vec::new();
                for line in coord_lines.iter().chain(root_lines[tail_start..].iter()) {
                    push_terminated_line(&mut bytes, line);
                }
                merged.insert(name.to_owned(), bytes);
                seeded_paths.push(name.to_owned());
            }
            PrefixVerdict::Fork {
                root: root_row,
                coord: coord_row,
            } => {
                return Err(WorkspaceError::CoordRefused {
                    code: "COORD_SEED_FORK_REFUSED",
                    message: format!(
                        "the root {name} and the {branch} copy have diverged (first divergence: root {root_row:?} vs coordination {coord_row:?}); refusing to fork the event log. Reconcile by hand (or keep the coordination log and re-open the root-only decisions), then retry"
                    ),
                });
            }
        }
    }
    for (relative, bytes) in &root_bytes {
        if SEED_LOG_FILENAMES.contains(&relative.as_str()) {
            continue;
        }
        if let Some(existing) = merged.get(relative) {
            if existing != bytes {
                warnings.push(format!(
                    "{relative}: root and coordination copies differ; keeping the coordination copy"
                ));
            }
            continue;
        }
        merged.insert(relative.clone(), bytes.clone());
        seeded_paths.push(relative.clone());
    }
    seeded_paths.sort();

    let intent = CoordRecord::intent(
        CoordRecordKind::Seed,
        workspace.slug(),
        "seed_coordination_surface",
        Some(branch),
        Some(worktree_root),
        observed_tip.as_ref(),
    );
    journal.begin(intent.clone())?;

    // The canonical Mission directory is the confinement root for every seeded
    // file, created only after the intent is durable and the locks are held.
    let created_dir = !feature_dir.exists();
    let mut guard = SeedDirGuard {
        path: feature_dir.to_path_buf(),
        remove_on_drop: created_dir,
    };
    std::fs::create_dir_all(feature_dir).map_err(|source| {
        WorkspaceError::Kernel(sk_kernel::KernelError::Io {
            action: "create coordination Mission directory",
            path: feature_dir.to_path_buf(),
            source,
        })
    })?;
    let mut written: Vec<(String, Vec<u8>)> = Vec::new();
    for (relative, bytes) in &merged {
        let target = sk_kernel::resolve_within(feature_dir, Path::new(relative))?;
        if target.exists() {
            // Never overwrite content that appeared between the probe and the
            // write: this is the one shape that would silently fork state.
            let detail = format!("{} appeared during seeding", target.display());
            journal.resolve(&intent, CoordPhase::Abandoned, Some(&detail), None)?;
            return Err(WorkspaceError::CoordRefused {
                code: "COORD_SEED_TARGET_EXISTS",
                message: format!(
                    "refusing to seed {}: it already exists in {}",
                    relative,
                    feature_dir.display()
                ),
            });
        }
        sk_kernel::atomic_write_with_options(
            &target,
            bytes,
            sk_kernel::AtomicWriteOptions {
                create_parents: true,
            },
        )?;
        written.push((relative.clone(), bytes.clone()));
    }

    // 4. Commit every COORD-kind file now on disk (they are all this seed's own
    // writes: the directory did not exist before this call).
    let mut fault: Option<String> = None;
    let landed = if written.is_empty() {
        None
    } else {
        let mut paths = Vec::with_capacity(written.len());
        let mut expected = Vec::with_capacity(written.len());
        for (relative, bytes) in &written {
            let path =
                sk_git::normalize_safe_commit_path(worktree_root, feature_dir.join(relative))?;
            expected.push(ExpectedBlob::new(path.clone(), bytes.clone()));
            paths.push(path);
        }
        let message = format!(
            "chore({}): seed coordination surface\n\n{COORD_SEED_TRAILER}: {}",
            workspace.slug(),
            workspace.mission_id()
        );
        let mut request = SafeCommitRequest::new(branch_name.clone(), message, paths.clone())
            .with_protection(protected.clone(), CommitGuardCapability::Standard)
            .with_expected_bytes(expected);
        if let Some(parent) = &observed_tip {
            request = request.with_expected_parent(parent.clone());
        }
        match repository.safe_commit(&request) {
            Ok(outcome) => {
                let landed = LandedCommit {
                    commit: outcome.commit().clone(),
                    destination: outcome.destination().as_str().to_owned(),
                    worktree_root: outcome.worktree_root().to_path_buf(),
                    committed_paths: outcome
                        .committed_paths()
                        .iter()
                        .map(sk_git::GitPath::display_lossy)
                        .collect(),
                    observed_tip: observed_tip.clone(),
                    index_repair: None,
                };
                if let Err(error) =
                    journal.resolve(&intent, CoordPhase::Applied, None, Some(&landed.commit))
                {
                    fault = Some(error.to_string());
                }
                Some(landed)
            }
            Err(GitError::SafeCommitNothingToCommit { detail, .. }) => {
                // The content is already durable at the tip: nothing to write,
                // and the directory is authoritative.
                warnings.push(format!("seed commit skipped: {detail}"));
                if let Err(error) =
                    journal.resolve(&intent, CoordPhase::Applied, Some(&detail), None)
                {
                    fault = Some(error.to_string());
                }
                None
            }
            Err(error) => {
                let detail = error.to_string();
                if let Err(journal_error) =
                    journal.resolve(&intent, CoordPhase::Abandoned, Some(&detail), None)
                {
                    return Err(WorkspaceError::CoordRefused {
                        code: "COORD_SEED_FAILED_WITH_JOURNAL_FAULT",
                        message: format!(
                            "the coordination seed commit failed ({detail}) and the coordination journal could not record the attempt either: {journal_error}"
                        ),
                    });
                }
                return Err(WorkspaceError::CoordRefused {
                    code: "COORD_SEED_COMMIT_REFUSED",
                    message: format!(
                        "the coordination seed commit was refused on {branch}: {detail}. The Mission directory this seed wrote has been removed, so the surface still reads as EMPTY and a retry will seed again"
                    ),
                });
            }
        }
    };
    if landed.is_none()
        && written.is_empty()
        && let Err(error) =
            journal.resolve(&intent, CoordPhase::Applied, Some("nothing to seed"), None)
    {
        fault = Some(error.to_string());
    }
    // The surface (or an intentionally empty Mission directory) is now the
    // authoritative coordination home: keep it.
    guard.remove_on_drop = false;

    // 5. Restore the root checkout to a single home.
    //
    // Only a root copy whose bytes are *provably reproduced* by the durable
    // coordination content is removed. A differing root copy is authored
    // divergence: it is kept, named in `preserved_root`, and the coordination
    // copy stays the committed one. The previous version passed every path the
    // branch tip carried into this step, so an untracked root copy whose bytes
    // differed from the tip (and were committed nowhere) was unlinked while the
    // receipt simultaneously listed it as preserved.
    let restore = restore_root_copies(
        workspace,
        &merged,
        &seeded_paths,
        &restored_from_branch,
        &mut warnings,
    );

    Ok(CoordSeedReceipt {
        worktree: worktree_root.to_path_buf(),
        branch: branch.to_owned(),
        seeded_paths,
        restored_from_branch,
        restored_root: restore.restored,
        preserved_root: restore.preserved,
        warnings,
        landed,
        state_before,
        journal_fault: fault,
    })
}

/// Remove a Mission directory this seed created when the seed did not complete.
struct SeedDirGuard {
    path: PathBuf,
    remove_on_drop: bool,
}

impl Drop for SeedDirGuard {
    fn drop(&mut self) {
        if self.remove_on_drop {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// The mission-relative remainder of a `kitty-specs/<dir>/<rel>` path.
fn mission_relative(path: &str, mission_dir_name: &str) -> Option<String> {
    let prefix = format!("{}/{mission_dir_name}/", crate::workspace::SPECS_DIR_NAME);
    path.strip_prefix(&prefix).map(str::to_owned)
}

fn git_path(path: &str) -> Result<sk_git::GitPath, WorkspaceError> {
    Ok(sk_git::GitPath::from_git_bytes(path.as_bytes())?)
}

/// Non-blank lines, byte-faithful (terminators retained, nothing stripped).
fn non_blank_lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n')
        .filter(|line| !line.trim().is_empty())
        .collect()
}

/// Append one line, guaranteeing exactly one row terminator.
fn push_terminated_line(out: &mut Vec<u8>, line: &str) {
    let trimmed = line.strip_suffix('\n').unwrap_or(line);
    out.extend_from_slice(trimmed.as_bytes());
    out.push(b'\n');
}

/// The `event_id` of every row, refusing a malformed or duplicate row.
fn event_ids_of(lines: &[&str], filename: &str, side: &str) -> Result<Vec<String>, WorkspaceError> {
    let mut ids: Vec<String> = Vec::with_capacity(lines.len());
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (index, line) in lines.iter().enumerate() {
        let parsed: serde_json::Value =
            serde_json::from_str(line).map_err(|error| WorkspaceError::CoordRefused {
                code: "COORD_SEED_EVENT_LOG_MALFORMED",
                message: format!(
                    "coordination seed cannot classify the {side} {filename:?}: malformed event-log line {}: {line:?} ({error}). Repair the row by hand; it is never silently skipped",
                    index + 1
                ),
            })?;
        let Some(row) = parsed.as_object() else {
            return Err(WorkspaceError::CoordRefused {
                code: "COORD_SEED_EVENT_LOG_MALFORMED",
                message: format!(
                    "coordination seed cannot classify the {side} {filename:?}: line {} is not a JSON object: {line:?}",
                    index + 1
                ),
            });
        };
        let Some(event_id) = row.get("event_id") else {
            return Err(WorkspaceError::CoordRefused {
                code: "COORD_SEED_EVENT_LOG_MALFORMED",
                message: format!(
                    "coordination seed cannot classify the {side} {filename:?}: line {} has no event_id: {line:?}",
                    index + 1
                ),
            });
        };
        let event_id = match event_id {
            serde_json::Value::String(value) => value.clone(),
            other => json_scalar_text(other),
        };
        if !seen.insert(event_id.clone()) {
            return Err(WorkspaceError::CoordRefused {
                code: "COORD_SEED_DUPLICATE_EVENT_ID",
                message: format!(
                    "coordination seed cannot classify the {side} {filename:?}: duplicate event_id within one log stream: {event_id:?}"
                ),
            });
        }
        ids.push(event_id);
    }
    Ok(ids)
}

/// The Python `str()` of a JSON scalar (`event_prefix` does `str(payload["event_id"])`).
fn json_scalar_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "None".to_owned(),
        serde_json::Value::Bool(true) => "True".to_owned(),
        serde_json::Value::Bool(false) => "False".to_owned(),
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// The prefix/fork classification of two event-id streams.
enum PrefixVerdict {
    /// The coordination stream is a proper prefix: carry the root tail.
    CarryTail { tail_start: usize },
    /// The root stream is a prefix of the coordination stream: carry nothing.
    Nothing,
    /// Neither is a prefix of the other.
    Fork {
        root: Option<String>,
        coord: Option<String>,
    },
}

fn classify_prefix(root: &[String], coord: &[String]) -> PrefixVerdict {
    if coord.len() <= root.len() && root[..coord.len()] == *coord {
        if coord.len() >= root.len() {
            return PrefixVerdict::Nothing;
        }
        return PrefixVerdict::CarryTail {
            tail_start: coord.len(),
        };
    }
    if root.len() <= coord.len() && coord[..root.len()] == *root {
        return PrefixVerdict::Nothing;
    }
    let shortest = root.len().min(coord.len());
    let first = (0..shortest)
        .find(|index| root[*index] != coord[*index])
        .unwrap_or(shortest);
    PrefixVerdict::Fork {
        root: root.get(first).cloned(),
        coord: coord.get(first).cloned(),
    }
}

/// Remove the root-checkout copies of every path now committed once.
///
/// The decision is per path and source-backed:
///
/// * an **untracked** root copy is removed only when the durable coordination
///   content *reproduces* its bytes — the bytes are then a proven duplicate, so
///   deleting the second home loses nothing. For a non-log file that means byte
///   equality with the committed coordination copy; for an event log it means
///   every root row is present, in order, in the committed log (`carried` tail
///   or an already-committed prefix);
/// * an untracked root copy that *differs* from the committed coordination copy
///   is **authored divergence** — it is kept and named in `preserved_root`,
///   never deleted. This is the reference's `merge.carried` rule in its safe
///   direction: upstream deletes only what the seed itself carried, and it keeps
///   a refused/unchanged seed's root copy;
/// * a staged-new or tracked-modified copy is preserved and reported: the
///   reference discards those with `git restore --staged` / `git checkout --`,
///   which this crate will not do to an operator's uncommitted bytes;
/// * a tracked *clean* copy is left alone, exactly like the reference (pre-fix
///   history stays untouched).
struct RootRestoreOutcome {
    /// Repository-relative paths whose second home was removed.
    restored: Vec<String>,
    /// Mission-relative paths whose root copy was kept, with its reason in
    /// `warnings`.
    preserved: Vec<String>,
}

fn restore_root_copies(
    workspace: &MissionWorkspace,
    durable: &BTreeMap<String, Vec<u8>>,
    seeded: &[String],
    restored_from_branch: &[String],
    warnings: &mut Vec<String>,
) -> RootRestoreOutcome {
    let mut outcome = RootRestoreOutcome {
        restored: Vec::new(),
        preserved: Vec::new(),
    };
    let primary_root = workspace.workspace().primary_root().to_path_buf();
    let Ok(repository) = sk_git::GitRepository::open(&primary_root) else {
        warnings.push("root restore skipped: the primary checkout could not be opened".to_owned());
        return outcome;
    };
    let statuses = repository
        .status_with(&sk_git::StatusOptions {
            untracked: sk_git::UntrackedMode::All,
            ignored: false,
        })
        .unwrap_or_default();
    // Match the directory that actually holds the mission. `canonical_mission_dir_name()`
    // is the *composed* `<slug>-<mid8>` name, which need not be the name on
    // disk (a pre-083 mission keeps `<slug>`), and `git status` reports the
    // on-disk path. Using the composed name here silently mis-classified every
    // root copy as `clean`, so an untracked copy was never removed.
    let mission_dir_relative = workspace
        .mission_dir()
        .strip_prefix(&primary_root)
        .map(to_posix)
        .ok()
        .or_else(|| {
            workspace.mission_dir().file_name().map(|name| {
                format!(
                    "{}/{}",
                    crate::workspace::SPECS_DIR_NAME,
                    name.to_string_lossy()
                )
            })
        })
        .unwrap_or_else(|| workspace.canonical_mission_dir_name());
    let mut paths: Vec<&String> = seeded.iter().chain(restored_from_branch.iter()).collect();
    paths.sort();
    paths.dedup();
    for relative in paths {
        let root_path = workspace.mission_dir().join(relative);
        let Ok(root_bytes) = std::fs::read(&root_path) else {
            continue;
        };
        let repo_relative = format!("{mission_dir_relative}/{relative}");
        let mut state = "clean";
        for entry in &statuses {
            if entry.path().display_lossy() == repo_relative {
                state = if entry.is_untracked() {
                    "untracked"
                } else if entry.index() == 'A' {
                    "staged_new"
                } else {
                    "dirty"
                };
                break;
            }
        }
        match state {
            "untracked" => {
                if root_copy_is_reproduced(relative, &root_bytes, durable) {
                    if std::fs::remove_file(&root_path).is_ok() {
                        outcome.restored.push(repo_relative);
                    }
                } else {
                    warnings.push(format!(
                        "{relative}: the root copy differs from the committed coordination copy; keeping the root copy (authored divergence), the coordination copy stays the committed one"
                    ));
                    outcome.preserved.push(relative.clone());
                }
            }
            "staged_new" | "dirty" => {
                warnings.push(format!(
                    "{relative}: the root copy is {state}; keeping it rather than discarding uncommitted bytes (the coordination copy is committed)"
                ));
                outcome.preserved.push(relative.clone());
            }
            _ => {}
        }
    }
    outcome
}

/// Does the durable coordination content reproduce this root copy's bytes?
///
/// A non-log file must match byte-for-byte. An event log is reproduced when
/// every root row also appears, in the same order, in the committed log: the
/// seed either carried that tail itself or the root stream is an
/// already-committed prefix of it, so the root rows are recoverable from the
/// coordination copy.
fn root_copy_is_reproduced(
    relative: &str,
    root_bytes: &[u8],
    durable: &BTreeMap<String, Vec<u8>>,
) -> bool {
    let Some(coord_bytes) = durable.get(relative) else {
        return false;
    };
    if root_bytes == coord_bytes.as_slice() {
        return true;
    }
    if !SEED_LOG_FILENAMES.contains(&relative) {
        return false;
    }
    let (Ok(root_text), Ok(coord_text)) = (
        std::str::from_utf8(root_bytes),
        std::str::from_utf8(coord_bytes),
    ) else {
        return false;
    };
    let root_rows = non_blank_lines(root_text);
    let coord_rows = non_blank_lines(coord_text);
    root_rows.len() <= coord_rows.len()
        && root_rows
            .iter()
            .zip(coord_rows.iter())
            .all(|(root, coord)| root.trim_end() == coord.trim_end())
}

/// Decode one seed input as UTF-8, naming the exact path on failure.
///
/// The reference opens both event logs with `encoding="utf-8"`, so a non-UTF-8
/// byte raises; decoding lossily and re-encoding would silently rewrite the
/// operator's bytes as U+FFFD in the coordination commit. Nothing is written
/// when this refuses.
fn decode_seed_text(bytes: &[u8], path: &Path, what: &str) -> Result<String, WorkspaceError> {
    String::from_utf8(bytes.to_vec()).map_err(|error| {
        let offset = error.utf8_error().valid_up_to();
        WorkspaceError::CoordRefused {
            code: "COORD_SEED_EVENT_LOG_NOT_UTF8",
            message: format!(
                "the {what} {} is not valid UTF-8 at byte {offset}: {error}; the reference opens it with encoding='utf-8' and would raise, and a lossy decode would rewrite authored bytes. Repair the file by hand; nothing was written",
                path.display()
            ),
        }
    })
}

fn is_seedable_kind(kind: MissionArtifactKind) -> bool {
    kind.is_coord_partition()
}

fn walk_files(root: &Path) -> Result<Vec<PathBuf>, WorkspaceError> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = std::fs::read_dir(&directory).map_err(|source| {
            WorkspaceError::Kernel(sk_kernel::KernelError::Io {
                action: "read coordination seed source",
                path: directory.clone(),
                source,
            })
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| {
                WorkspaceError::Kernel(sk_kernel::KernelError::Io {
                    action: "read coordination seed source entry",
                    path: directory.clone(),
                    source,
                })
            })?;
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path).map_err(|source| {
                WorkspaceError::Kernel(sk_kernel::KernelError::Io {
                    action: "inspect coordination seed source entry",
                    path: path.clone(),
                    source,
                })
            })?;
            if metadata.file_type().is_symlink() {
                return Err(WorkspaceError::CoordRefused {
                    code: "COORD_SEED_SYMLINK_REFUSED",
                    message: format!(
                        "refusing to seed {}: coordination artifacts must be regular files",
                        path.display()
                    ),
                });
            }
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file() {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

fn to_posix(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn ensure_unprotected(
    destination: &BranchName,
    protected: &ProtectedBranches,
    operation: &str,
) -> Result<(), WorkspaceError> {
    if protected.contains(destination) {
        return Err(WorkspaceError::CoordRefused {
            code: crate::policy::PROTECTED_BRANCH_REFUSED,
            message: format!(
                "refusing {operation}: destination {destination} is on the resolved protected-branch list"
            ),
        });
    }
    Ok(())
}

fn ensure_head_is_destination(
    repository: &sk_git::GitRepository,
    destination: &BranchName,
    worktree: &Path,
) -> Result<(), WorkspaceError> {
    let observed = repository.current_branch()?;
    if observed.as_deref() != Some(destination.as_str()) {
        return Err(WorkspaceError::CoordRefused {
            code: "COORD_WORKTREE_HEAD_MISMATCH",
            message: format!(
                "{} has {:?} checked out, not the transaction destination {destination}; refusing to commit into a differently checked-out tree",
                worktree.display(),
                observed.unwrap_or_else(|| "<detached>".to_owned())
            ),
        });
    }
    Ok(())
}

fn ownership_proof(
    workspace: &MissionWorkspace,
    surface: TopologySurface,
    destination: &BranchName,
    worktree_root: &Path,
    locks: &CoordLocks,
    observed_tip: Option<Oid>,
    repository: &sk_git::GitRepository,
) -> OwnershipProof {
    OwnershipProof {
        mission_slug: workspace.slug().to_owned(),
        mission_id: workspace.mission_id().to_string(),
        mid8: workspace.identity().mid8().to_owned(),
        topology: workspace.topology().as_str().to_owned(),
        surface,
        destination_ref: destination.as_str().to_owned(),
        worktree_root: worktree_root.to_path_buf(),
        worktree_branch: repository.current_branch().ok().flatten(),
        observed_tip,
        checkout_claim_lock: locks.claim.path().to_path_buf(),
        mission_lock_key: locks.mission.key().to_owned(),
        mission_lock_path: locks.mission.path().map(Path::to_path_buf),
        mission_lock_holder: locks.mission.holder().cloned(),
    }
}

fn read_optional(path: &Path, max_bytes: u64) -> Result<Option<Vec<u8>>, WorkspaceError> {
    match sk_kernel::read_bounded(path, max_bytes) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(sk_kernel::KernelError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            Ok(None)
        }
        Err(error) => Err(WorkspaceError::Kernel(error)),
    }
}

fn is_nested_worktrees_path(worktree_root: &Path, target: &Path) -> bool {
    let Ok(relative) = target.strip_prefix(worktree_root) else {
        return false;
    };
    relative
        .components()
        .any(|component| component.as_os_str().to_str() == Some(sk_git::WORKTREES_DIR))
}

// ---------------------------------------------------------------------------
// Creation bootstrap
// ---------------------------------------------------------------------------

/// The result of bootstrapping a coordination surface for a created mission.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoordBootstrapReceipt {
    /// The declared (or canonically derived) coordination branch.
    pub branch: String,
    /// Was the branch minted by this call?
    pub branch_created: bool,
    /// The commit the branch was minted at (the target tip), when created.
    pub creation_base: Option<Oid>,
    /// The ownership evidence for the established surface.
    ///
    /// `None` when the create was **deferred**: the target branch does not
    /// resolve, so there is no ref to mint from and the reference records a
    /// `skipped_reason` instead of minting against nothing.
    pub proof: Option<OwnershipProof>,
    /// The seed operation, when the surface was empty and got seeded.
    pub seed: Option<CoordSeedReceipt>,
    /// The coordination state observed before the surface was established.
    pub state_before: Option<CoordState>,
    /// Why the branch was not materialized, when the create was deferred.
    pub skipped_reason: Option<String>,
    /// Evidence of a mint this call made and then rolled back (reported through
    /// [`CoordBootstrapFailure`] when the call fails).
    pub mint_rolled_back: Option<String>,
    /// A journal fault recorded after the mint or the establishment was proven.
    pub journal_fault: Option<String>,
}

impl CoordBootstrapReceipt {
    /// Was this create deferred because the target branch does not resolve?
    pub fn is_deferred(&self) -> bool {
        self.skipped_reason.is_some()
    }
}

/// Mint-or-validate the coordination branch and establish its write surface.
///
/// This is the workspace-owned half of `mission create` for a coordination
/// topology: the mission manifest and scaffold already exist, and this call makes
/// the coordination branch and worktree real before any bookkeeping write.
///
/// # Order (this used to be wrong)
///
/// 1. resolve the destination (declared, or derived from the canonical grammar);
/// 2. refuse a **protected** destination;
/// 3. take the kernel **checkout-claim** lock, then the canonical **Mission**
///    lock;
/// 4. journal a `Mint` intent and only then mint (or validate) the branch;
/// 5. materialize/seed through the same `establish` the write path uses;
/// 6. seal the establishment.
///
/// Before this order, the branch was minted **before** the locks and before the
/// protection gate: a refused protected destination left a real branch behind,
/// and the mint ran outside the lock authority the report claimed for it.
///
/// # Provenance rollback
///
/// A branch **this call minted** is rolled back when steps 5–6 fail, and only
/// when provenance proves the rollback is safe: the branch must still sit at its
/// creation base, or at a seed commit this store recorded whose own pinned parent
/// was that base. The rollback is journaled as its own `Mint` record and the tip
/// is preserved under `refs/spec-kitty/` before the branch is deleted, so no
/// commit becomes unreachable. A branch that gained any other history is left in
/// place and the failure names it.
///
/// # Deferred create
///
/// A target branch that does not resolve is **not** a hard refusal: the reference
/// returns the canonical branch name with a `skipped_reason` and lets the first
/// real use surface the missing target. This returns
/// [`CoordBootstrapReceipt::is_deferred`] instead of minting against nothing.
pub fn bootstrap_coordination(
    workspace: &MissionWorkspace,
    policy: &ProtectionPolicy,
) -> Result<CoordBootstrapReceipt, WorkspaceError> {
    bootstrap_coordination_attempt(workspace, policy).map_err(WorkspaceError::from)
}

/// The failure of one bootstrap attempt, with the mint-rollback evidence.
#[derive(Debug)]
pub struct CoordBootstrapFailure {
    /// The failure the caller must see.
    ///
    /// Boxed so the `Err` arm of [`bootstrap_coordination_attempt`] stays small.
    pub error: Box<WorkspaceError>,
    /// What happened to a branch this attempt minted, when it minted one.
    pub mint_rolled_back: Option<String>,
}

impl std::fmt::Display for CoordBootstrapFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.mint_rolled_back {
            Some(detail) => write!(formatter, "{} (mint rollback: {detail})", self.error),
            None => write!(formatter, "{}", self.error),
        }
    }
}

impl From<CoordBootstrapFailure> for WorkspaceError {
    fn from(failure: CoordBootstrapFailure) -> Self {
        match failure.mint_rolled_back {
            Some(rollback) => WorkspaceError::CoordBootstrapRolledBack {
                error: failure.error,
                rollback,
            },
            None => *failure.error,
        }
    }
}

/// [`bootstrap_coordination`], keeping the mint-rollback evidence on failure.
pub fn bootstrap_coordination_attempt(
    workspace: &MissionWorkspace,
    policy: &ProtectionPolicy,
) -> Result<CoordBootstrapReceipt, CoordBootstrapFailure> {
    match bootstrap_inner(workspace, policy) {
        Ok(receipt) => Ok(receipt),
        Err(failure) => Err(*failure),
    }
}

/// The failure of one bootstrap attempt, boxed so the `Err` arm stays small.
type BootstrapResult<T> = Result<T, Box<CoordBootstrapFailure>>;

/// Lift a Git failure into a bootstrap failure.
fn lift<T>(result: Result<T, sk_git::GitError>) -> BootstrapResult<T> {
    result.map_err(|error| {
        Box::new(CoordBootstrapFailure {
            error: Box::new(WorkspaceError::Git(error)),
            mint_rolled_back: None,
        })
    })
}

fn bootstrap_inner(
    workspace: &MissionWorkspace,
    policy: &ProtectionPolicy,
) -> BootstrapResult<CoordBootstrapReceipt> {
    let fail = |error: WorkspaceError| {
        Box::new(CoordBootstrapFailure {
            error: Box::new(error),
            mint_rolled_back: None,
        })
    };
    if !routes_through_coordination(workspace.topology()) {
        return Err(fail(WorkspaceError::CoordRefused {
            code: "COORD_BOOTSTRAP_NOT_ROUTED",
            message: format!(
                "mission {} stores topology {} which mints no coordination branch",
                workspace.slug(),
                workspace.topology().as_str()
            ),
        }));
    }

    // The destination is resolved, and its protection checked, BEFORE any lock
    // is taken and before anything is minted or written.
    let probe = workspace.probe_coordination().map_err(fail)?;
    // Unlike the write path, a create is expected to find the declared branch
    // ABSENT: minting it is this call's job. Only a lost `meta.json` record is
    // accepted as derived, and only while the canonical branch it names already
    // exists (a crash between the mint and the record write).
    let destination = match workspace.coordination_branch() {
        Some(branch_text) => {
            let branch = lift(BranchName::parse(&branch_text))?;
            let worktree = workspace.coord_worktree_path().ok_or_else(|| {
                fail(WorkspaceError::CoordRefused {
                    code: "COORDINATION_WORKTREE_UNRESOLVED",
                    message: format!(
                        "mission {} routes through coordination but has no canonical worktree path",
                        workspace.slug()
                    ),
                })
            })?;
            CoordDestination {
                branch_text,
                branch,
                worktree,
                derived: false,
            }
        }
        None => {
            let derived_branch = crate::naming::coord_reconstruct_branch(
                workspace.slug(),
                workspace.identity().mid8(),
            );
            if probe.state == CoordState::Deleted {
                // create has not minted yet: name the branch this call would
                // mint, exactly like the reference's self-describing result.
                return Ok(CoordBootstrapReceipt {
                    branch: derived_branch,
                    branch_created: false,
                    creation_base: None,
                    proof: None,
                    seed: None,
                    state_before: Some(probe.state),
                    skipped_reason: Some(
                        "coordination_branch is undeclared and the canonical branch does not exist"
                            .to_owned(),
                    ),
                    mint_rolled_back: None,
                    journal_fault: None,
                });
            }
            let branch = lift(BranchName::parse(&derived_branch))?;
            let worktree = workspace.coord_worktree_path().ok_or_else(|| {
                fail(WorkspaceError::CoordRefused {
                    code: "COORDINATION_WORKTREE_UNRESOLVED",
                    message: format!(
                        "mission {} routes through coordination but has no canonical worktree path",
                        workspace.slug()
                    ),
                })
            })?;
            CoordDestination {
                branch_text: derived_branch,
                branch,
                worktree,
                derived: true,
            }
        }
    };
    let protected = policy.branch_set().map_err(fail)?;
    ensure_unprotected(&destination.branch, &protected, "mission_create_bootstrap")
        .map_err(fail)?;

    let primary_root = workspace.workspace().primary_root().to_path_buf();
    let common_dir = workspace.workspace().common_dir().to_path_buf();
    let locks = CoordLocks::acquire(
        &destination.worktree,
        workspace.mission_dir(),
        &primary_root,
        &common_dir,
        COORD_TRANSACTION_LOCK_TIMEOUT,
    )
    .map_err(fail)?;
    let journal = CoordJournal::new(&common_dir, workspace.slug());
    let repository = workspace.workspace().repository();

    let target = lift(BranchName::parse(&workspace.target_branch().map_err(fail)?))?;
    let Some(target_tip) = lift(repository.read_branch_tip(&target))? else {
        // Deferred create: there is no ref to mint from. The reference records a
        // `skipped_reason` and lets the first real use surface the missing
        // target; nothing was mutated, so there is nothing to roll back.
        return Ok(CoordBootstrapReceipt {
            branch: destination.branch_text,
            branch_created: false,
            creation_base: None,
            proof: None,
            seed: None,
            state_before: Some(probe.state),
            skipped_reason: Some(format!(
                "target_branch '{target}' does not resolve to a ref"
            )),
            mint_rolled_back: None,
            journal_fault: None,
        });
    };

    // Journal the mint before the ref exists, like every other mutation.
    let intent = CoordRecord::intent(
        CoordRecordKind::Mint,
        workspace.slug(),
        "mint_coordination_branch",
        Some(&destination.branch_text),
        Some(&destination.worktree),
        Some(&target_tip),
    );
    journal.begin(intent.clone()).map_err(fail)?;

    let mut branch_created = false;
    let mut creation_base = None;
    match lift(repository.read_branch_tip(&destination.branch))? {
        None => {
            let plan = repository
                .plan_branch_creation(
                    &destination.branch,
                    sk_git::StartPoint::Oid(target_tip.clone()),
                    "spec-kitty: mint mission coordination branch",
                )
                .map_err(|error| {
                    let _ = journal.resolve(
                        &intent,
                        CoordPhase::Abandoned,
                        Some(&error.to_string()),
                        None,
                    );
                    fail(WorkspaceError::Git(error))
                })?;
            if let Err(error) = repository.apply_branch_creation(&plan) {
                let _ = journal.resolve(
                    &intent,
                    CoordPhase::Abandoned,
                    Some(&error.to_string()),
                    None,
                );
                return Err(fail(WorkspaceError::Git(error)));
            }
            branch_created = true;
            creation_base = Some(target_tip.clone());
        }
        Some(tip) => {
            if tip == target_tip || lift(repository.is_ancestor(&tip, &target_tip))? {
                // At or behind the target: reusable as-is.
            } else if lift(repository.is_ancestor(&target_tip, &tip))? {
                // Ahead of the target: only this store's own seed/integration
                // commits make that provable. Anything else is unknown history.
                let recorded = journal
                    .recorded_seed_commits(&destination.branch_text)
                    .map_err(fail)?;
                let accounted = recorded
                    .iter()
                    .any(|commit| repository.is_ancestor(commit, &tip).unwrap_or(false));
                if !accounted {
                    let _ = journal.resolve(
                        &intent,
                        CoordPhase::Abandoned,
                        Some("reused branch carries unaccounted history"),
                        None,
                    );
                    return Err(fail(WorkspaceError::CoordRefused {
                        code: "COORDINATION_BRANCH_DIVERGED",
                        message: format!(
                            "coordination branch {} is ahead of {target} at {tip} and this store records no seed or integration commit reachable from it; refusing to reuse unaccounted coordination history",
                            destination.branch
                        ),
                    }));
                }
            } else {
                let _ = journal.resolve(
                    &intent,
                    CoordPhase::Abandoned,
                    Some("reused branch diverged from the target"),
                    None,
                );
                return Err(fail(WorkspaceError::CoordRefused {
                    code: "COORDINATION_BRANCH_DIVERGED",
                    message: format!(
                        "coordination branch {} ({tip}) has diverged from target {target} ({target_tip}); refusing to re-fork or reuse it automatically",
                        destination.branch
                    ),
                }));
            }
        }
    }
    // The mint (when one ran) already happened; a journal fault here is
    // reporting, so it is carried on the receipt and never inverts the mint.
    let mut journal_fault = journal
        .resolve(
            &intent,
            CoordPhase::Applied,
            Some(if branch_created {
                "minted at the target tip"
            } else {
                "reused an existing coordination branch"
            }),
            creation_base.as_ref(),
        )
        .err()
        .map(|error| error.to_string());

    // The state before establishment: a fresh mint had no worktree yet.
    let state_before = if branch_created {
        CoordState::Unmaterialized
    } else {
        probe.state
    };
    let receipt = match finish_bootstrap(
        workspace,
        protected,
        destination,
        journal,
        locks,
        state_before,
    ) {
        Ok(receipt) => receipt,
        Err(error) => {
            let rollback = rollback_mint(workspace, creation_base.clone(), branch_created);
            return Err(Box::new(CoordBootstrapFailure {
                error: Box::new(error),
                mint_rolled_back: rollback,
            }));
        }
    };
    Ok(CoordBootstrapReceipt {
        branch_created,
        creation_base,
        journal_fault: journal_fault.take(),
        ..receipt
    })
}

/// Re-probe, materialize/seed the surface, and seal the establishment intent.
fn finish_bootstrap(
    workspace: &MissionWorkspace,
    protected: ProtectedBranches,
    destination: CoordDestination,
    journal: CoordJournal,
    locks: CoordLocks,
    state_before: CoordState,
) -> Result<CoordBootstrapReceipt, WorkspaceError> {
    let branch_text = destination.branch_text.clone();
    // Re-probe under the locks: the mint just changed what Git reports.
    let probe = workspace.probe_coordination()?;
    let txn = establish(
        workspace,
        "mission_create_bootstrap",
        protected,
        destination,
        probe,
        journal,
        locks,
    )?;
    let proof = txn.proof().clone();
    let seed = txn.seed().cloned();
    let mut journal_fault = txn
        .seal("coordination surface established; no artifact commit was requested")
        .err()
        .map(|error| error.to_string());
    if journal_fault.is_none()
        && let Some(seed) = seed.as_ref()
        && let Some(fault) = seed.journal_fault.clone()
    {
        journal_fault = Some(fault);
    }
    Ok(CoordBootstrapReceipt {
        branch: branch_text,
        branch_created: false,
        creation_base: None,
        proof: Some(proof),
        seed,
        state_before: Some(state_before),
        skipped_reason: None,
        mint_rolled_back: None,
        journal_fault: journal_fault.take(),
    })
}

/// Roll a branch this attempt minted back, using recorded provenance only.
///
/// The branch is deleted only when provenance proves that everything on it beyond
/// the creation base is this store's own seed commit; the tip is preserved under
/// a `refs/spec-kitty/` bookkeeping ref first, so no commit becomes unreachable,
/// and the rollback is journaled as its own `Mint` record.
fn rollback_mint(
    workspace: &MissionWorkspace,
    creation_base: Option<Oid>,
    branch_created: bool,
) -> Option<String> {
    if !branch_created {
        return None;
    }
    let branch_text = workspace.coordination_branch().unwrap_or_else(|| {
        crate::naming::coord_reconstruct_branch(workspace.slug(), workspace.identity().mid8())
    });
    let branch = BranchName::parse(&branch_text).ok()?;
    let repository = workspace.workspace().repository();
    let tip = repository.read_branch_tip(&branch).ok().flatten();
    let journal = CoordJournal::new(workspace.workspace().common_dir(), workspace.slug());
    let seeds = journal
        .recorded_seeds(&branch_text)
        .ok()
        .unwrap_or_default();
    let rollback_intent = CoordRecord::intent(
        CoordRecordKind::Mint,
        workspace.slug(),
        "rollback_coordination_branch_mint",
        Some(&branch_text),
        None,
        tip.as_ref(),
    );
    let _ = journal.begin(rollback_intent.clone());
    let finish = |detail: String, tip: Option<&Oid>| {
        let _ = journal.resolve(&rollback_intent, CoordPhase::Abandoned, Some(&detail), tip);
        detail
    };

    let Some(tip) = tip else {
        return Some(finish(
            "the minted branch no longer exists; nothing to roll back".to_owned(),
            None,
        ));
    };
    let own_seed = seeds.iter().any(|(commit, observed)| {
        *commit == tip
            && creation_base
                .as_ref()
                .is_some_and(|base| observed.as_ref() == Some(base))
    });
    if creation_base.as_ref() != Some(&tip) && !own_seed {
        return Some(finish(
            format!(
                "left {} in place: it carries history this create cannot prove is its own (tip {tip})",
                branch.as_str()
            ),
            Some(&tip),
        ));
    }

    // A failed establishment may have materialized the coordination worktree on
    // this very branch. A branch checked out in any worktree cannot be deleted,
    // so remove a worktree this attempt created *first*, and only while it is
    // provably clean and still on the minted tip. Re-taking the same claim +
    // Mission locks the failed attempt used keeps a concurrent writer from
    // adopting the checkout in between. Anything else leaves the branch (and the
    // worktree) in place and says so.
    let primary_root = workspace.workspace().primary_root().to_path_buf();
    let common_dir = workspace.workspace().common_dir().to_path_buf();
    let write_checkout = workspace
        .coord_worktree_path()
        .unwrap_or_else(|| primary_root.clone());
    let _held = match CoordLocks::acquire(
        &write_checkout,
        workspace.mission_dir(),
        &primary_root,
        &common_dir,
        COORD_TRANSACTION_LOCK_TIMEOUT,
    ) {
        Ok(locks) => locks,
        Err(error) => {
            return Some(finish(
                format!(
                    "left {} in place: could not re-take the claim and Mission locks for the rollback ({error})",
                    branch.as_str()
                ),
                Some(&tip),
            ));
        }
    };
    if let Some(worktree) = workspace.coord_worktree_path() {
        match repository.plan_worktree_removal(
            &worktree,
            Some(&tip),
            Some(&branch),
            sk_git::WorktreeDiscardPolicy::RefuseDirty,
        ) {
            Ok(plan) => {
                if let Err(error) = repository.apply_worktree_removal(&plan) {
                    return Some(finish(
                        format!(
                            "left {} in place: the coordination worktree at {} could not be removed ({error})",
                            branch.as_str(),
                            worktree.display()
                        ),
                        Some(&tip),
                    ));
                }
            }
            // Nothing was materialized before the failure: there is no checkout
            // to release, and the branch can still be deleted below.
            Err(GitError::WorktreeNotRegistered { .. }) => {}
            Err(error) => {
                return Some(finish(
                    format!(
                        "left {} in place: the coordination worktree at {} could not be proven removable ({error})",
                        branch.as_str(),
                        worktree.display()
                    ),
                    Some(&tip),
                ));
            }
        }
    }

    let Ok(preserve) = sk_git::RefName::parse(&format!(
        "refs/spec-kitty/coord-rollback/{}-{}",
        workspace.slug(),
        workspace.identity().mid8()
    )) else {
        return Some(finish(
            format!(
                "left {} in place: no bookkeeping ref could be composed for its tip",
                branch.as_str()
            ),
            Some(&tip),
        ));
    };
    let options = sk_git::BranchDeletionOptions::default()
        .preserve_tip_at(preserve)
        .allow_unique_commits("rolling back a coordination branch this create minted");
    // The destination was proven unprotected before the mint, so no protection
    // override is needed (and none is granted here).
    let protected = ProtectedBranches::default();
    let plan = match repository.plan_branch_deletion(
        &branch,
        &protected,
        creation_base.as_ref(),
        options,
        "spec-kitty: roll back a coordination branch a failed create minted",
    ) {
        Ok(plan) => plan,
        Err(error) => {
            return Some(finish(
                format!(
                    "left {} in place: the guarded deletion refused ({error})",
                    branch.as_str()
                ),
                Some(&tip),
            ));
        }
    };
    match repository.apply_branch_deletion(&plan) {
        Ok(outcome) => Some(finish(
            format!(
                "deleted {} ({}) after preserving its tip under a bookkeeping ref",
                branch.as_str(),
                outcome.tip()
            ),
            Some(&tip),
        )),
        Err(error) => Some(finish(
            format!(
                "left {} in place: the guarded deletion failed ({error})",
                branch.as_str()
            ),
            Some(&tip),
        )),
    }
}

// ---------------------------------------------------------------------------
// Recovery
// ---------------------------------------------------------------------------

/// One automatic reconciliation performed by [`recover_interrupted`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoordRecoveryRepair {
    /// Record kind.
    pub kind: CoordRecordKind,
    /// Operation label.
    pub operation: String,
    /// The phase recovery proved.
    pub phase: CoordPhase,
    /// Why the outcome is what it is.
    pub detail: String,
}

/// One pending record that cannot be reconciled automatically.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoordRecoveryConflict {
    /// The pending record.
    pub record: CoordRecord,
    /// Why it is inconsistent, naming the immutable commit when there is one.
    pub detail: String,
}

/// The outcome of reconciling a mission's coordination journal.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CoordRecoveryReport {
    /// Records reconciled automatically.
    pub repairs: Vec<CoordRecoveryRepair>,
    /// Records that need an operator decision.
    pub conflicts: Vec<CoordRecoveryConflict>,
}

impl CoordRecoveryReport {
    /// Did recovery prove every pending record's outcome?
    pub fn is_clean(&self) -> bool {
        self.conflicts.is_empty()
    }
}

/// Reconcile pending coordination records against observed Git state.
///
/// # Reachability decides, not equality
///
/// * a recorded candidate that **is reachable from** the branch tip → `Applied`.
///   Equality is only the special case: a seed or commit that landed and was then
///   built on by another writer (the ordinary crash-then-continue shape) used to
///   be a permanent conflict with no operator action that cleared it, because
///   only `Integration` records were allowed to use ancestry. Every kind now
///   decides by reachability;
/// * a candidate that is *not* reachable and a branch tip unchanged from the
///   recorded observation → `Abandoned` (nothing landed);
/// * anything else → a conflict naming both tips, with nothing modified.
///
/// The whole pass runs under the crate's claim + Mission locks on the mission's
/// write checkout, and every tip, worktree registration and pending record is
/// read after they are held. A verdict therefore cannot be invalidated by a
/// writer that lands between the proof and the resolve (#R5).
///
/// Materialization and mint intents are reconciled from the worktree registry and
/// the ref itself; a stray directory is reported, never deleted, and a stored
/// candidate is never discarded.
///
/// `Indeterminate` records are terminal by design: this function repairs
/// `Intent` records only, and it never rewrites an unresolved intent into a
/// proven outcome it did not prove (see [`CoordJournal::begin`]).
pub fn recover_interrupted(
    workspace: &MissionWorkspace,
) -> Result<CoordRecoveryReport, WorkspaceError> {
    let primary_root = workspace.workspace().primary_root().to_path_buf();
    let common_dir = workspace.workspace().common_dir().to_path_buf();
    // Recovery is a writer decision, so it takes the crate's one lock protocol
    // (checkout claim, then canonical Mission lock) on the SAME checkout every
    // transaction for this mission takes — the coordination worktree when the
    // topology routes through it, otherwise the primary root. Without this, a
    // concurrently landing writer could invalidate a verdict between the proof
    // and the resolve, and the verdict would be stale rather than serialized.
    let write_checkout = workspace
        .coord_worktree_path()
        .unwrap_or_else(|| primary_root.clone());
    let _locks = CoordLocks::acquire(
        &write_checkout,
        workspace.mission_dir(),
        &primary_root,
        &common_dir,
        COORD_TRANSACTION_LOCK_TIMEOUT,
    )?;
    // Every observation below is read *under* the hold: the pending set, the
    // branch tips and the worktree registry.
    let journal = CoordJournal::new(&common_dir, workspace.slug());
    let repository = workspace.workspace().repository().clone();
    let mut report = CoordRecoveryReport::default();
    for record in journal.pending()? {
        let Some(branch_text) = record.branch.as_deref() else {
            report.conflicts.push(CoordRecoveryConflict {
                detail: "pending record names no branch".to_owned(),
                record,
            });
            continue;
        };
        let branch = BranchName::parse(branch_text)?;
        let tip = repository.read_branch_tip(&branch)?;
        match record.kind {
            CoordRecordKind::Materialize => {
                let Some(path) = record.worktree.as_deref() else {
                    report.conflicts.push(CoordRecoveryConflict {
                        detail: "pending materialization names no worktree".to_owned(),
                        record,
                    });
                    continue;
                };
                let registered = repository
                    .worktrees()?
                    .into_iter()
                    .any(|worktree| same_path(worktree.path(), path));
                if registered {
                    report.repairs.push(CoordRecoveryRepair {
                        kind: record.kind,
                        operation: record.operation.clone(),
                        phase: CoordPhase::Applied,
                        detail: format!("{} is registered on {branch_text}", path.display()),
                    });
                    journal.resolve(&record, CoordPhase::Applied, None, None)?;
                } else if !path.exists() {
                    report.repairs.push(CoordRecoveryRepair {
                        kind: record.kind,
                        operation: record.operation.clone(),
                        phase: CoordPhase::Abandoned,
                        detail: format!("{} does not exist; nothing landed", path.display()),
                    });
                    journal.resolve(
                        &record,
                        CoordPhase::Abandoned,
                        Some("worktree absent"),
                        None,
                    )?;
                } else {
                    report.conflicts.push(CoordRecoveryConflict {
                        detail: format!(
                            "{} exists but is not registered on {branch_text}; refusing to delete or adopt it",
                            path.display()
                        ),
                        record,
                    });
                }
            }
            CoordRecordKind::Mint => {
                // The mutation is "the branch resolves at or behind the recorded
                // mint base" (the candidate names it when the mint was recorded
                // as applied, otherwise the observation). A reused branch that
                // sits *behind* the target is a success, not a conflict: the
                // postcondition is "the branch exists and carries no history
                // the target does not", and an older tip satisfies it.
                let observed = match record.observed_tip.as_deref() {
                    Some(observed) => Some(Oid::parse(observed)?),
                    None => None,
                };
                let present_at_or_behind_base = match (&tip, &observed) {
                    (Some(tip), Some(observed)) => {
                        tip == observed || repository.is_ancestor(tip, observed)?
                    }
                    _ => false,
                };
                if present_at_or_behind_base {
                    report.repairs.push(CoordRecoveryRepair {
                        kind: record.kind,
                        operation: record.operation.clone(),
                        phase: CoordPhase::Applied,
                        detail: format!("{branch_text} exists at or behind the recorded mint base"),
                    });
                    journal.resolve(&record, CoordPhase::Applied, None, observed.as_ref())?;
                } else if tip.is_none() {
                    report.repairs.push(CoordRecoveryRepair {
                        kind: record.kind,
                        operation: record.operation.clone(),
                        phase: CoordPhase::Abandoned,
                        detail: format!("{branch_text} does not exist; the mint never landed"),
                    });
                    journal.resolve(
                        &record,
                        CoordPhase::Abandoned,
                        Some("minted branch absent"),
                        None,
                    )?;
                } else {
                    report.conflicts.push(CoordRecoveryConflict {
                        detail: format!(
                            "cannot decide the mint of {branch_text}: the branch tip is {}, the recorded base was {}",
                            tip.as_ref().map(ToString::to_string).unwrap_or_else(|| "<none>".to_owned()),
                            record.observed_tip.as_deref().unwrap_or("<none>")
                        ),
                        record,
                    });
                }
            }
            CoordRecordKind::Seed | CoordRecordKind::Commit | CoordRecordKind::Integration => {
                let candidate = match record.candidate_commit.as_deref() {
                    Some(candidate) => Some(Oid::parse(candidate)?),
                    None => None,
                };
                let observed = match record.observed_tip.as_deref() {
                    Some(observed) => Some(Oid::parse(observed)?),
                    None => None,
                };
                let reachable = match (&candidate, &tip) {
                    (Some(candidate), Some(tip)) => repository.is_ancestor(candidate, tip)?,
                    _ => false,
                };
                if let (Some(candidate), true) = (&candidate, reachable) {
                    let detail = if Some(candidate) == tip.as_ref() {
                        format!("candidate {candidate} is the branch tip")
                    } else {
                        format!(
                            "candidate {candidate} landed and was built on; it is reachable from {}",
                            tip.as_ref().map(ToString::to_string).unwrap_or_default()
                        )
                    };
                    report.repairs.push(CoordRecoveryRepair {
                        kind: record.kind,
                        operation: record.operation.clone(),
                        phase: CoordPhase::Applied,
                        detail,
                    });
                    journal.resolve(&record, CoordPhase::Applied, None, Some(candidate))?;
                    continue;
                }
                if tip == observed {
                    let detail = match &candidate {
                        Some(candidate) => format!(
                            "branch tip {} is unchanged from the recorded observation; candidate {candidate} never landed",
                            tip.as_ref()
                                .map(ToString::to_string)
                                .unwrap_or_else(|| "<none>".to_owned())
                        ),
                        None => "no candidate was recorded and the branch did not move".to_owned(),
                    };
                    report.repairs.push(CoordRecoveryRepair {
                        kind: record.kind,
                        operation: record.operation.clone(),
                        phase: CoordPhase::Abandoned,
                        detail,
                    });
                    journal.resolve(
                        &record,
                        CoordPhase::Abandoned,
                        Some("branch tip unchanged"),
                        candidate.as_ref(),
                    )?;
                    continue;
                }
                report.conflicts.push(CoordRecoveryConflict {
                    detail: format!(
                        "cannot decide: branch {branch_text} tip is {}, the recorded observation was {}, the candidate was {} and is not reachable from the tip",
                        tip.as_ref().map(ToString::to_string).unwrap_or_else(|| "<none>".to_owned()),
                        record.observed_tip.as_deref().unwrap_or("<none>"),
                        record.candidate_commit.as_deref().unwrap_or("<none>")
                    ),
                    record,
                });
            }
        }
    }
    Ok(report)
}

fn same_path(left: &Path, right: &Path) -> bool {
    std::fs::canonicalize(left).unwrap_or_else(|_| left.to_path_buf())
        == std::fs::canonicalize(right).unwrap_or_else(|_| right.to_path_buf())
}
