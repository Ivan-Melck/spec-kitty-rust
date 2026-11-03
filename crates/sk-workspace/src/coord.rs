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
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sk_git::{
    BranchName, CommitGuardCapability, ExpectedBlob, GitError, GitPath, Oid, ProtectedBranches,
    SafeCommitRequest,
};
use sk_kernel::{LockWait, StatusLock, StatusLockHolder};
use sk_mission::routes_through_coordination;

use crate::artifact::{MissionArtifactKind, kind_for_mission_file};
use crate::error::WorkspaceError;
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
pub const MAX_SEED_FILE_BYTES: u64 = 16 * 1024 * 1024;

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
    /// `Materialized` resolves the coordination directory; `Empty`,
    /// `Unmaterialized`, `Deleted` and `None` resolve the declared primary
    /// directory. This is the *read* answer and creates nothing.
    pub read_dir: PathBuf,
    /// Is [`CoordSurfaceRead::read_dir`] the primary fallback rather than a
    /// materialized coordination surface?
    pub degraded_to_primary: bool,
}

/// Resolve the coordination surface for reading, without any mutation.
///
/// Nothing here creates a worktree, writes a file, or takes a lock: an `Empty`
/// or `Unmaterialized` surface is reported as such and the caller is given the
/// declared primary directory instead.
pub fn read_coordination_surface(
    workspace: &MissionWorkspace,
) -> Result<CoordSurfaceRead, WorkspaceError> {
    let probe = workspace.probe_coordination()?;
    let primary_dir = workspace.mission_dir().to_path_buf();
    let coordination_mission_dir = crate::topology::coordination_mission_dir(workspace);
    let materialized_dir = coordination_mission_dir
        .as_ref()
        .filter(|_| probe.state == CoordState::Materialized)
        .cloned();
    let degraded_to_primary = materialized_dir.is_none();
    Ok(CoordSurfaceRead {
        state: probe.state,
        coordination_branch: probe.branch,
        coordination_worktree: probe.worktree,
        coordination_mission_dir,
        read_dir: materialized_dir.unwrap_or(primary_dir),
        degraded_to_primary,
    })
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
}

impl CoordRecordKind {
    /// The stable wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Materialize => "materialize",
            Self::Seed => "seed",
            Self::Commit => "commit",
            Self::Integration => "integration",
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
    pub fn begin(&self, record: CoordRecord) -> Result<(), WorkspaceError> {
        let _lock = sk_kernel::FileLock::acquire(&self.lock_path, COORD_JOURNAL_LOCK_TIMEOUT)?;
        let mut records = self.records()?;
        // A newer intent for the same shape supersedes an unresolved older one:
        // the newest observation is what recovery reconciles.
        for existing in &mut records {
            if existing.matches_intent(&record) {
                existing.phase = CoordPhase::Abandoned;
                existing.detail = Some("superseded by a newer attempt".to_owned());
            }
        }
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
        let mut commits = Vec::new();
        for record in self.records()? {
            if record.kind != CoordRecordKind::Seed || record.phase != CoordPhase::Applied {
                continue;
            }
            if record.branch.as_deref() != Some(branch) {
                continue;
            }
            if let Some(candidate) = record.candidate_commit {
                commits.push(Oid::parse(&candidate)?);
            }
        }
        Ok(commits)
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
    pub fn derived_path(&self) -> Option<&Path> {
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
#[derive(Debug)]
struct CoordLocks {
    mission: MissionHold,
    claim: StatusLock,
}

impl CoordLocks {
    fn acquire(
        write_checkout: &Path,
        mission_dir: &Path,
        repo_root: &Path,
        common_dir: &Path,
        _mission_key: &str,
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
    /// Mission-relative paths carried over.
    pub seeded_paths: Vec<String>,
    /// The seed commit, when any content was carried.
    pub landed: Option<LandedCommit>,
    /// The state observed before the seed.
    pub state_before: CoordState,
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
    /// checkout claim and Mission lock → journal intent → materialization/seeding
    /// (with their own journal records) → ownership proof.
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
        let probe = workspace.probe_coordination()?;
        let protected = policy.branch_set()?;
        let primary_root = workspace.workspace().primary_root().to_path_buf();
        let common_dir = workspace.workspace().common_dir().to_path_buf();
        let mission_key = workspace.canonical_mission_dir_name();

        let routes = routes_through_coordination(workspace.topology());
        if !routes {
            let destination = workspace.write_branch(&protected)?;
            ensure_unprotected(&destination, &protected, operation)?;
            let locks = CoordLocks::acquire(
                &primary_root,
                workspace.mission_dir(),
                &primary_root,
                &common_dir,
                &mission_key,
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

        // Coordination-routed mission.
        let Some(branch_text) = workspace.coordination_branch() else {
            return Err(WorkspaceError::CoordRefused {
                code: "COORDINATION_BRANCH_UNDECLARED",
                message: format!(
                    "mission {} stores a coordination-routing topology but records no coordination_branch",
                    workspace.slug()
                ),
            });
        };
        let destination = BranchName::parse(&branch_text)?;
        if let CoordState::Deleted = probe.state {
            return Err(WorkspaceError::CoordinationBranchDeleted {
                mission: workspace.slug().to_owned(),
                branch: branch_text,
            });
        }
        ensure_unprotected(&destination, &protected, operation)?;
        let coord_root =
            workspace
                .coord_worktree_path()
                .ok_or_else(|| WorkspaceError::CoordRefused {
                    code: "COORDINATION_WORKTREE_UNRESOLVED",
                    message: format!(
                        "mission {} routes through coordination but has no canonical worktree path",
                        workspace.slug()
                    ),
                })?;
        let locks = CoordLocks::acquire(
            &coord_root,
            workspace.mission_dir(),
            &primary_root,
            &common_dir,
            &mission_key,
            timeout,
        )?;
        let journal = CoordJournal::new(&common_dir, workspace.slug());
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
                if !probe.local_head {
                    return Err(WorkspaceError::CoordRefused {
                        code: "COORDINATION_BRANCH_NOT_LOCAL",
                        message: format!(
                            "coordination branch {branch_text} resolves only on a remote; a remote-tracking branch cannot be attached as a local worktree"
                        ),
                    });
                }
                let repository = workspace.workspace().repository().clone();
                materialize_worktree(&repository, &branch_text, &coord_root, &journal, workspace)?;
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

        // Re-probe after materialization: the worktree is now registered, so the
        // state is either Materialized (mission dir present) or Empty.
        let feature_dir = materialized_root
            .join(crate::workspace::SPECS_DIR_NAME)
            .join(workspace.canonical_mission_dir_name());
        let mut seed = None;
        if !feature_dir.exists() {
            seed = Some(seed_coordination_surface(
                workspace,
                &materialized_root,
                &feature_dir,
                &branch_text,
                &protected,
                &journal,
                state_before,
            )?);
            establishment = CoordEstablishment::Seeded;
        } else if establishment == CoordEstablishment::Materialized
            && state_before == CoordState::Unmaterialized
        {
            // Materialization produced a full surface: nothing to seed.
            establishment = CoordEstablishment::Noop;
        }

        let repository = sk_git::GitRepository::open(&materialized_root)?;
        ensure_head_is_destination(&repository, &destination, &materialized_root)?;
        let observed_tip = repository.read_branch_tip(&destination)?;
        let proof = ownership_proof(
            workspace,
            TopologySurface::Coord,
            &destination,
            &materialized_root,
            &locks,
            observed_tip.clone(),
            &repository,
        );
        let intent = CoordRecord::intent(
            CoordRecordKind::Commit,
            workspace.slug(),
            operation,
            Some(destination.as_str()),
            Some(&materialized_root),
            observed_tip.as_ref(),
        );
        journal.begin(intent.clone())?;
        Ok(Self {
            workspace,
            repository,
            surface: TopologySurface::Coord,
            state_before,
            establishment,
            worktree_root: materialized_root,
            feature_dir,
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
            seed,
            _locks: locks,
            proof,
        })
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

    /// Stage a path the caller wrote out of band (no rollback snapshot).
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
        let outcome = self.commit_staged(message)?;
        self.committed = true;
        Ok(CoordReceipt {
            outcome,
            proof: self.proof.clone(),
            state_before: self.state_before,
            establishment: self.establishment,
            seed: self.seed.clone(),
            journal_path: self.journal.path().to_path_buf(),
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

    fn commit_staged(&mut self, message: &str) -> Result<CoordCommitOutcome, WorkspaceError> {
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

        let intent = self.journal_intent.clone();
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
                self.journal
                    .resolve(&intent, CoordPhase::Applied, None, Some(&landed.commit))?;
                Ok(CoordCommitOutcome::Landed(landed))
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
                self.journal
                    .resolve(&intent, CoordPhase::Applied, Some(&detail), None)?;
                Ok(CoordCommitOutcome::NoOp {
                    head,
                    destination: self.destination.as_str().to_owned(),
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
                self.journal.resolve(
                    &intent,
                    CoordPhase::Applied,
                    Some(&format!("landed with index repair required: {detail}")),
                    Some(&landed.commit),
                )?;
                Ok(CoordCommitOutcome::Landed(landed))
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
                    self.journal.resolve(
                        &intent,
                        CoordPhase::Applied,
                        Some(&format!("advance confirmed by readback: {detail}")),
                        Some(&landed.commit),
                    )?;
                    return Ok(CoordCommitOutcome::Landed(landed));
                }
                if current == self.observed_tip {
                    // Nothing landed: the branch is exactly where it was. The
                    // artifacts this transaction wrote are restored from their
                    // snapshots, because the write they belong to did not happen.
                    self.journal.resolve(
                        &intent,
                        CoordPhase::Abandoned,
                        Some(&detail),
                        Some(&candidate),
                    )?;
                    self.restore_snapshots();
                    return Ok(CoordCommitOutcome::NotLanded {
                        candidate,
                        observed_tip: current,
                        detail,
                    });
                }
                self.journal.resolve(
                    &intent,
                    CoordPhase::Indeterminate,
                    Some(&format!(
                        "advance unconfirmed; branch tip is {:?}, substrate reported {:?}: {detail}",
                        current.as_ref().map(ToString::to_string),
                        observed_tip.as_ref().map(ToString::to_string)
                    )),
                    Some(&candidate),
                )?;
                Ok(CoordCommitOutcome::Indeterminate {
                    candidate,
                    observed_tip: current,
                    detail,
                })
            }
            Err(error) => {
                self.journal.resolve(
                    &intent,
                    CoordPhase::Abandoned,
                    Some(&error.to_string()),
                    None,
                )?;
                self.restore_snapshots();
                Err(WorkspaceError::Git(error))
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

/// Seed an empty coordination surface from the primary mission directory.
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
    let observed_tip = repository.read_branch_tip(&branch_name)?;

    // Seed-safety: the branch may not already carry coordination-only commits
    // this store cannot account for. Reading them at the tip needs a tree read
    // `sk-git` does not expose, so the fail-closed answer is a refusal.
    let exclusive = repository.unique_commits(&branch_name, &[], None)?;
    if !exclusive.is_empty() {
        let recorded = journal.recorded_seed_commits(branch)?;
        let known_seed = match &observed_tip {
            Some(tip) => recorded.iter().any(|commit| {
                commit == tip || repository.is_ancestor(commit, tip).unwrap_or(false)
            }),
            None => false,
        };
        let code = if known_seed {
            "COORD_SURFACE_RESTORE_REQUIRED"
        } else {
            "COORD_SEED_FORK_REFUSED"
        };
        return Err(WorkspaceError::CoordRefused {
            code,
            message: format!(
                "coordination branch {branch} already carries {} coordination-only commit(s) but its worktree has no Mission directory; refusing to re-seed over committed coordination state (a branch-tip restore needs a tree read that sk-git does not expose)",
                exclusive.len()
            ),
        });
    }

    // Only COORD-partition artifacts are carried; PRIMARY planning artifacts
    // stay in the primary checkout.
    let source_dir = workspace.mission_dir().to_path_buf();
    let mission_dir_name = workspace.canonical_mission_dir_name();
    let mut carried: Vec<(String, Vec<u8>)> = Vec::new();
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
            "{}/{}/{}",
            crate::workspace::SPECS_DIR_NAME,
            mission_dir_name,
            relative_text
        );
        let Some(kind) = kind_for_mission_file(&lookup, Some(&mission_dir_name)) else {
            continue;
        };
        if !is_seedable_kind(kind) {
            continue;
        }
        let bytes = sk_kernel::read_bounded(&entry, MAX_SEED_FILE_BYTES)?;
        carried.push((relative_text, bytes));
    }

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
    // file. `resolve_within` requires it to exist, so it is created here — after
    // the intent is durable and while the locks are held.
    std::fs::create_dir_all(feature_dir).map_err(|source| {
        WorkspaceError::Kernel(sk_kernel::KernelError::Io {
            action: "create coordination Mission directory",
            path: feature_dir.to_path_buf(),
            source,
        })
    })?;

    let mut seeded_paths = Vec::new();
    for (relative, bytes) in &carried {
        let target = sk_kernel::resolve_within(feature_dir, Path::new(relative))?;
        if target.exists() {
            // Never overwrite content that appeared between the probe and the
            // write: this is the one shape that would silently fork state.
            journal.resolve(
                &intent,
                CoordPhase::Abandoned,
                Some(&format!("{} appeared during seeding", target.display())),
                None,
            )?;
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
        seeded_paths.push(relative.clone());
    }

    let landed = if carried.is_empty() {
        None
    } else {
        let paths = carried
            .iter()
            .map(|(relative, _)| {
                sk_git::normalize_safe_commit_path(worktree_root, feature_dir.join(relative))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let expected = paths
            .iter()
            .zip(carried.iter())
            .map(|(path, (_, bytes))| ExpectedBlob::new(path.clone(), bytes.clone()))
            .collect::<Vec<_>>();
        let message = format!(
            "chore(spec-kitty): seed coordination surface\n\n{COORD_SEED_TRAILER}: {}",
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
                journal.resolve(&intent, CoordPhase::Applied, None, Some(&landed.commit))?;
                Some(landed)
            }
            Err(error) => {
                journal.resolve(
                    &intent,
                    CoordPhase::Abandoned,
                    Some(&error.to_string()),
                    None,
                )?;
                // The files we wrote were new; remove only those we created.
                for (relative, _) in &carried {
                    if let Ok(target) = sk_kernel::resolve_within(feature_dir, Path::new(relative))
                    {
                        let _ = std::fs::remove_file(target);
                    }
                }
                return Err(WorkspaceError::Git(error));
            }
        }
    };
    if landed.is_none() {
        journal.resolve(&intent, CoordPhase::Applied, Some("nothing to seed"), None)?;
    }
    Ok(CoordSeedReceipt {
        worktree: worktree_root.to_path_buf(),
        branch: branch.to_owned(),
        seeded_paths,
        landed,
        state_before,
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
        mission_lock_path: locks.mission.derived_path().map(Path::to_path_buf),
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
    /// The declared coordination branch.
    pub branch: String,
    /// Was the branch minted by this call?
    pub branch_created: bool,
    /// The commit the branch was minted at (the target tip), when created.
    pub creation_base: Option<Oid>,
    /// The ownership evidence for the established surface.
    pub proof: OwnershipProof,
    /// The seed operation, when the surface was empty and got seeded.
    pub seed: Option<CoordSeedReceipt>,
    /// The coordination state observed before bootstrapping.
    pub state_before: CoordState,
}

/// Mint-or-validate the coordination branch and establish its write surface.
///
/// This is the workspace-owned half of `mission create` for a coordination
/// topology: the mission manifest and scaffold already exist, and this call makes
/// the coordination branch and worktree real before any bookkeeping write. It is
/// journaled, takes the canonical claim + Mission locks, and refuses rather than
/// guessing when the branch history is not provably this mission's own.
///
/// * an absent branch is created at the target tip (`branch_created = true`);
/// * a branch at or behind the target is reused as-is;
/// * a branch ahead of the target is reused only when a seed or integration
///   commit this store recorded is reachable from its tip;
/// * a diverged branch, or one carrying unaccounted coordination-only commits,
///   is refused.
pub fn bootstrap_coordination(
    workspace: &MissionWorkspace,
    policy: &ProtectionPolicy,
) -> Result<CoordBootstrapReceipt, WorkspaceError> {
    if !routes_through_coordination(workspace.topology()) {
        return Err(WorkspaceError::CoordRefused {
            code: "COORD_BOOTSTRAP_NOT_ROUTED",
            message: format!(
                "mission {} stores topology {} which mints no coordination branch",
                workspace.slug(),
                workspace.topology().as_str()
            ),
        });
    }
    let Some(branch_text) = workspace.coordination_branch() else {
        return Err(WorkspaceError::CoordRefused {
            code: "COORDINATION_BRANCH_UNDECLARED",
            message: format!(
                "mission {} stores a coordination-routing topology but records no coordination_branch",
                workspace.slug()
            ),
        });
    };
    let branch = BranchName::parse(&branch_text)?;
    let repository = workspace.workspace().repository();
    let target = BranchName::parse(&workspace.target_branch()?)?;
    let target_tip =
        repository
            .read_branch_tip(&target)?
            .ok_or_else(|| WorkspaceError::CoordRefused {
                code: "COORD_BOOTSTRAP_TARGET_MISSING",
                message: format!(
                    "target branch {target} does not resolve; refusing to mint a coordination branch from nothing"
                ),
            })?;
    let journal = CoordJournal::new(workspace.workspace().common_dir(), workspace.slug());

    let mut branch_created = false;
    let mut creation_base = None;
    match repository.read_branch_tip(&branch)? {
        None => {
            let plan = repository.plan_branch_creation(
                &branch,
                sk_git::StartPoint::Oid(target_tip.clone()),
                "spec-kitty: mint mission coordination branch",
            )?;
            repository.apply_branch_creation(&plan)?;
            branch_created = true;
            creation_base = Some(target_tip.clone());
        }
        Some(tip) => {
            if tip == target_tip || repository.is_ancestor(&tip, &target_tip)? {
                // At or behind the target: reusable as-is.
            } else if repository.is_ancestor(&target_tip, &tip)? {
                // Ahead of the target: only this store's own seed/integration
                // commits make that provable. Anything else is unknown history.
                let recorded = journal.recorded_seed_commits(&branch_text)?;
                let accounted = recorded
                    .iter()
                    .any(|commit| repository.is_ancestor(commit, &tip).unwrap_or(false));
                if !accounted {
                    return Err(WorkspaceError::CoordRefused {
                        code: "COORDINATION_BRANCH_DIVERGED",
                        message: format!(
                            "coordination branch {branch} is ahead of {target} at {tip} and this store records no seed or integration commit reachable from it; refusing to reuse unaccounted coordination history"
                        ),
                    });
                }
            } else {
                return Err(WorkspaceError::CoordRefused {
                    code: "COORDINATION_BRANCH_DIVERGED",
                    message: format!(
                        "coordination branch {branch} ({tip}) has diverged from target {target} ({target_tip}); refusing to re-fork or reuse it automatically"
                    ),
                });
            }
        }
    }

    let txn = CoordTransaction::acquire(workspace, "mission_create_bootstrap", policy)?;
    let state_before = txn.state_before();
    let proof = txn.proof().clone();
    let seed = txn.seed().cloned();
    txn.seal("coordination surface established; no artifact commit was requested")?;
    Ok(CoordBootstrapReceipt {
        branch: branch_text,
        branch_created,
        creation_base,
        proof,
        seed,
        state_before,
    })
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
/// * a recorded candidate that equals the branch tip → `Applied` (the commit
///   exists; it is never undone);
/// * an unchanged branch tip → `Abandoned` (nothing landed);
/// * anything else → a conflict naming both tips, with nothing modified.
///
/// Materialization intents are reconciled from the worktree registry and the
/// Mission directory; a stray directory is reported, never deleted.
pub fn recover_interrupted(
    workspace: &MissionWorkspace,
) -> Result<CoordRecoveryReport, WorkspaceError> {
    let journal = CoordJournal::new(workspace.workspace().common_dir(), workspace.slug());
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
            CoordRecordKind::Seed | CoordRecordKind::Commit | CoordRecordKind::Integration => {
                let candidate = match record.candidate_commit.as_deref() {
                    Some(candidate) => Some(Oid::parse(candidate)?),
                    None => None,
                };
                let observed = match record.observed_tip.as_deref() {
                    Some(observed) => Some(Oid::parse(observed)?),
                    None => None,
                };
                match (&candidate, &tip) {
                    (Some(candidate), Some(tip)) if candidate == tip => {
                        report.repairs.push(CoordRecoveryRepair {
                            kind: record.kind,
                            operation: record.operation.clone(),
                            phase: CoordPhase::Applied,
                            detail: format!("candidate {candidate} is the branch tip"),
                        });
                        journal.resolve(&record, CoordPhase::Applied, None, Some(candidate))?;
                    }
                    (Some(candidate), Some(tip))
                        if record.kind == CoordRecordKind::Integration
                            && repository.is_ancestor(candidate, tip)? =>
                    {
                        report.repairs.push(CoordRecoveryRepair {
                            kind: record.kind,
                            operation: record.operation.clone(),
                            phase: CoordPhase::Applied,
                            detail: format!("source {candidate} is reachable from {tip}"),
                        });
                        journal.resolve(&record, CoordPhase::Applied, None, Some(candidate))?;
                    }
                    (None, _) if tip == observed && candidate.is_none() => {
                        report.repairs.push(CoordRecoveryRepair {
                            kind: record.kind,
                            operation: record.operation.clone(),
                            phase: CoordPhase::Abandoned,
                            detail: "no candidate was recorded and the branch did not move"
                                .to_owned(),
                        });
                        journal.resolve(
                            &record,
                            CoordPhase::Abandoned,
                            Some("no candidate and unchanged tip"),
                            None,
                        )?;
                    }
                    (_, tip) if *tip == observed => {
                        report.repairs.push(CoordRecoveryRepair {
                            kind: record.kind,
                            operation: record.operation.clone(),
                            phase: CoordPhase::Abandoned,
                            detail: format!(
                                "branch tip {} is unchanged from the recorded observation; the candidate never landed",
                                tip.as_ref().map(ToString::to_string).unwrap_or_else(|| "<none>".to_owned())
                            ),
                        });
                        journal.resolve(
                            &record,
                            CoordPhase::Abandoned,
                            Some("branch tip unchanged"),
                            candidate.as_ref(),
                        )?;
                    }
                    _ => {
                        report.conflicts.push(CoordRecoveryConflict {
                            detail: format!(
                                "cannot decide: branch {branch_text} tip is {}, the recorded observation was {}, the candidate was {}",
                                tip.as_ref().map(ToString::to_string).unwrap_or_else(|| "<none>".to_owned()),
                                record.observed_tip.as_deref().unwrap_or("<none>"),
                                record.candidate_commit.as_deref().unwrap_or("<none>")
                            ),
                            record,
                        });
                    }
                }
            }
        }
    }
    Ok(report)
}

fn same_path(left: &Path, right: &Path) -> bool {
    std::fs::canonicalize(left).unwrap_or_else(|_| left.to_path_buf())
        == std::fs::canonicalize(right).unwrap_or_else(|_| right.to_path_buf())
}
