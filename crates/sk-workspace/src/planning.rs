//! Planning-commit metadata/ancestry guards and dependency integration.
//!
//! A planning commit is the commit that carries `spec.md` / `plan.md` /
//! `tasks/` / `lanes.json` onto a mission's write branch. Three things make it
//! dangerous:
//!
//! 1. **Routing.** It is a PRIMARY-partition commit. It must never transit the
//!    coordination branch (the add/add conflict that made `lanes.json` land in
//!    the wrong place), and it must never land on a protected target.
//! 2. **Destination.** The one write-branch rule is
//!    [`MissionWorkspace::write_branch`]: a `single_branch` mission with a
//!    recorded `meta.json` `mission_branch` writes **that** branch, never the
//!    (possibly protected) target. Using the target instead refused valid work
//!    when the target was protected and silently mis-destined it when it was not.
//! 3. **Ancestry and metadata.** Lanes are cut from the coordination branch,
//!    which is minted *before* planning exists, so the recorded planning commit
//!    is what later connects a lane to its planning lineage. A recorded pin that
//!    is orphaned or absent must be refused rather than merged, and an
//!    uncommitted `meta.json` that drops `coordination_branch` must never be
//!    swept into the commit (that would silently flatten the mission).
//!
//! # Authority
//!
//! Every mutation here takes the crate's one lock protocol
//! ([`crate::coord::CoordLocks`]: kernel checkout-claim **then** canonical
//! Mission lock) and journals a [`CoordRecordKind::Commit`] /
//! [`CoordRecordKind::Integration`] intent *before* touching a ref. Before this,
//! the planning commit and every integration mutated refs with no claim lock, no
//! Mission lock and no journal at all, so they did not serialize against the
//! status writer.
//!
//! # Remaining merge-driver gap (R7, declared not waived)
//!
//! Upstream runs both the planning-artifact merge and the whole dependency loop
//! inside `_ephemeral_merge_driver_activation`
//! (`lanes/consolidation.py:695`): it writes the registry's attribute lines into
//! `$GIT_COMMON_DIR/info/attributes` for the duration of one merge and removes
//! exactly those lines afterwards, so a `kitty-specs/**` bookkeeping conflict is
//! union/field-resolved by `spec-kitty merge-driver-*` (#2709/#2711/#4120)
//! instead of aborting the loop.
//!
//! This crate does not implement that, because the halves it needs do not exist
//! yet and inventing them would be worse than the gap:
//!
//! 1. the **driver commands** (`spec-kitty merge-driver-event-log`, `-meta`,
//!    `-traces`, `-acceptance-matrix`, `-issue-matrix`, `-review-cycle`,
//!    `-decision-index`) are CLI subcommands that no crate implements —
//!    `sk-install::attributes` already records that its plan configures drivers
//!    "but not functional yet";
//! 2. a **git-config write seam**. `sk-git` exposes no config writer, so even
//!    the persistent `merge.<key>.name`/`.driver` definitions cannot be applied
//!    from Rust; `sk-install` emits a
//!    `MergeDriverWiringPlan`-shaped value for that owner instead of shelling
//!    out;
//! 3. an **info/attributes seam** with the exact source semantics: append only
//!    the missing registry lines, remember the appended set, and remove exactly
//!    that set afterwards (unlinking the file only if it was created empty).
//!
//! Precise API proposal (for the owning crates, no kernel change needed):
//! `sk_git::GitRepository::set_local_config(key, value)` +
//! `remove_local_config(key)`; `sk_git::GitRepository::read_info_attributes()`
//! and `write_info_attributes(bytes)` (common-dir `info/attributes`, atomic);
//! and a `sk-workspace::planning::ephemeral_merge_driver_activation(repository)
//! -> MergeDriverActivation` guard returning the appended lines so `Drop` removes
//! exactly them. Until those land, a `kitty-specs/**` conflict aborts and rewinds
//! (fail-closed, no data loss) and this asymmetry is recorded in
//! `SECOND-REMEDIATION-REPORT.md`.
//!
//! # Blob-bound note
//!
//! The Git seam's explicit bounded blob read accepts `sk_git::MAX_BLOB_BYTES`
//! (64 MiB, the canonical status-log limit; the default remains 8 MiB), and that
//! bound is what [`MAX_PLANNING_ARTIFACT_BYTES`] and the seed's
//! `MAX_SEED_FILE_BYTES` use. The former 16 MiB local cap is gone, so a
//! legitimate large event log is read whole instead of being refused.

use std::path::{Path, PathBuf};

use serde_json::Value;
use sk_git::{
    BlobLookup, BranchAdvanceOptions, BranchName, CommitGuardCapability, ExpectedBlob, GitError,
    GitPath, Oid, ProtectedBranches, SafeCommitRequest, StartPoint,
};

use crate::artifact::{MissionArtifactKind, kind_for_mission_file};
use crate::coord::{
    COORD_TRANSACTION_LOCK_TIMEOUT, CoordCommitOutcome, CoordJournal, CoordLocks, CoordPhase,
    CoordRecord, CoordRecordKind, LandedCommit,
};
use crate::error::WorkspaceError;
use crate::lanes::{LanesManifest, LanesStore};
use crate::naming::PLANNING_LANE_ID;
use crate::protection::ProtectionPolicy;
use crate::workspace::{MissionWorkspace, SPECS_DIR_NAME};

/// Largest planning artifact this module will read for its exact-bytes pin.
///
/// Equal to the Git seam's `MAX_BLOB_BYTES`: asking for more would be a promise
/// the seam cannot keep.
pub const MAX_PLANNING_ARTIFACT_BYTES: u64 = sk_git::MAX_BLOB_BYTES;

// ---------------------------------------------------------------------------
// Recorded planning pin
// ---------------------------------------------------------------------------

/// Classification of a recorded `planning_commit_sha` against a target tip.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PinClass {
    /// The recorded commit is an ancestor of (or equal to) the target tip.
    Advanced,
    /// The commit exists here but is not reachable from the target tip (a
    /// mid-mission rewrite). Merging it produces a dead base.
    Orphaned,
    /// The commit object is absent from this repository.
    Foreign,
    /// The question cannot be answered: no recorded pin, or no target tip.
    Indeterminate,
}

impl PinClass {
    /// Is this a pin a lane may merge?
    pub fn is_usable(self) -> bool {
        matches!(self, Self::Advanced | Self::Indeterminate)
    }

    /// The stable wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Advanced => "advanced",
            Self::Orphaned => "orphaned",
            Self::Foreign => "foreign",
            Self::Indeterminate => "indeterminate",
        }
    }
}

/// Classify `recorded` against `target_tip`.
///
/// `target_tip` must be the *planning target-branch tip*, never a lane HEAD:
/// a healthy fresh lane already contains the pin, so classifying against the
/// lane would report `Advanced` for every lane and hide an orphaned pin.
pub fn classify_recorded_pin(
    repository: &sk_git::GitRepository,
    recorded: Option<&str>,
    target_tip: Option<&Oid>,
) -> Result<PinClass, WorkspaceError> {
    let (Some(recorded), Some(target)) = (recorded, target_tip) else {
        return Ok(PinClass::Indeterminate);
    };
    let recorded = Oid::parse(recorded)?;
    if !repository.commit_exists(&recorded)? {
        return Ok(PinClass::Foreign);
    }
    if repository.is_ancestor(&recorded, target)? {
        Ok(PinClass::Advanced)
    } else {
        Ok(PinClass::Orphaned)
    }
}

// ---------------------------------------------------------------------------
// Planning commit
// ---------------------------------------------------------------------------

/// How a planning commit's `meta.json` demotion guard was satisfied.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MetaDemotionGuard {
    /// `meta.json` is not part of this commit.
    NotCommitted,
    /// `HEAD` carries no baseline for the path, so this is a first commit.
    NoHeadBaseline,
    /// A `HEAD` baseline exists and the working copy keeps `coordination_branch`.
    Preserved,
}

/// A validated, read-only planning-commit plan.
#[derive(Clone, Debug)]
pub struct PlanningCommitPlan {
    destination: BranchName,
    message: String,
    paths: Vec<GitPath>,
    expected_bytes: Vec<ExpectedBlob>,
    pinned_parent: Oid,
    pin_tip: Option<Oid>,
    pin_class: PinClass,
    meta_guard: MetaDemotionGuard,
    protected: ProtectedBranches,
}

impl PlanningCommitPlan {
    /// The destination branch (the mission's canonical write branch).
    pub fn destination(&self) -> &BranchName {
        &self.destination
    }

    /// The commit message.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The parent the commit is pinned to (the destination tip at plan time).
    pub fn pinned_parent(&self) -> &Oid {
        &self.pinned_parent
    }

    /// The tip the recorded pin was classified against (the mission target).
    pub fn pin_tip(&self) -> Option<&Oid> {
        self.pin_tip.as_ref()
    }

    /// The repository-relative paths.
    pub fn paths(&self) -> &[GitPath] {
        &self.paths
    }

    /// The recorded pin classification.
    pub fn pin_class(&self) -> PinClass {
        self.pin_class
    }

    /// The demotion guard outcome.
    pub fn meta_guard(&self) -> MetaDemotionGuard {
        self.meta_guard
    }

    /// Build the plan for `workspace` without writing anything.
    ///
    /// `paths` are repository-relative POSIX paths (`kitty-specs/<dir>/...`).
    /// `recorded_planning_commit` is the manifest's current pin, if any. The
    /// demotion baseline is read from `HEAD` by this function; a caller-supplied
    /// baseline mapping could silently disable the guard by passing `None`.
    pub fn prepare(
        workspace: &MissionWorkspace,
        paths: &[String],
        message: &str,
        policy: &ProtectionPolicy,
        recorded_planning_commit: Option<&str>,
    ) -> Result<Self, WorkspaceError> {
        let primary_root = workspace.workspace().primary_root().to_path_buf();
        let repository = workspace.workspace().repository();
        let protected = policy.branch_set()?;
        // The ONE write-branch rule, shared with the owned-write path.
        let destination = workspace.write_branch(&protected)?;
        let pinned_parent = repository
            .read_branch_tip(&destination)?
            .ok_or_else(|| WorkspaceError::CoordRefused {
                code: "PLANNING_TARGET_MISSING",
                message: format!(
                    "planning commit destination {destination} does not exist; refusing to create a planning lineage on a branch the mission does not target"
                ),
            })?;
        let observed_head = repository.current_branch()?;
        if observed_head.as_deref() != Some(destination.as_str()) {
            return Err(WorkspaceError::CoordRefused {
                code: "PLANNING_COMMIT_HEAD_MISMATCH",
                message: format!(
                    "the primary checkout has {:?} checked out, not the planning destination {destination}",
                    observed_head.unwrap_or_else(|| "<detached>".to_owned())
                ),
            });
        }
        if protected.contains(&destination) {
            return Err(WorkspaceError::CoordRefused {
                code: crate::policy::PROTECTED_BRANCH_REFUSED,
                message: format!(
                    "refusing the planning commit: destination {destination} is on the resolved protected-branch list"
                ),
            });
        }
        if paths.is_empty() {
            return Err(WorkspaceError::CoordRefused {
                code: "PLANNING_COMMIT_EMPTY",
                message: "a planning commit requires at least one path".to_owned(),
            });
        }

        // Artifact paths arrive with the mission's *stored* directory name
        // (`kitty-specs/<dir>/...`), which is not necessarily the composed
        // `<slug>-<mid8>` name. Classify against the directory that actually
        // appears on disk so the partition and meta guards see the real file.
        let mission_dir_name = workspace
            .mission_dir()
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
            .unwrap_or_else(|| workspace.canonical_mission_dir_name());
        let mut normalized = Vec::with_capacity(paths.len());
        let mut expected_bytes = Vec::new();
        let mut meta_guard = MetaDemotionGuard::NotCommitted;
        for supplied in paths {
            let copied = supplied.clone();
            let git_path = sk_git::normalize_safe_commit_path(&primary_root, Path::new(&copied))?;
            ensure_primary_partition(&git_path, &mission_dir_name)?;
            let absolute = primary_root.join(git_path.to_path_buf());
            let bytes = sk_kernel::read_bounded(&absolute, MAX_PLANNING_ARTIFACT_BYTES)?;
            if is_meta_json(&git_path, &mission_dir_name) {
                meta_guard = check_meta_demotion(repository, &absolute, &git_path)?;
            }
            expected_bytes.push(ExpectedBlob::new(git_path.clone(), bytes));
            normalized.push(git_path);
        }

        let target = BranchName::parse(&workspace.target_branch()?)?;
        let pin_tip = repository.read_branch_tip(&target)?;
        let pin_class =
            classify_recorded_pin(repository, recorded_planning_commit, pin_tip.as_ref())?;
        if !pin_class.is_usable() {
            return Err(WorkspaceError::CoordRefused {
                code: "PLANNING_PIN_UNUSABLE",
                message: format!(
                    "recorded planning commit {} is {} against target {target}; refusing to build a planning commit on a dead pin",
                    recorded_planning_commit.unwrap_or("<none>"),
                    pin_class.as_str()
                ),
            });
        }

        Ok(Self {
            destination,
            message: message.to_owned(),
            paths: normalized,
            expected_bytes,
            pinned_parent,
            pin_tip,
            pin_class,
            meta_guard,
            protected,
        })
    }

    /// Apply the plan: one path-scoped safe commit on the write branch.
    ///
    /// The claim + Mission locks are taken **before** the commit, the intent is
    /// journaled, and the destination tip is re-read under the lock so a target
    /// that moved after planning refuses instead of committing against a stale
    /// parent.
    pub fn apply(
        self,
        workspace: &MissionWorkspace,
    ) -> Result<PlanningCommitReceipt, WorkspaceError> {
        let primary_root = workspace.workspace().primary_root().to_path_buf();
        let common_dir = workspace.workspace().common_dir().to_path_buf();
        let locks = CoordLocks::acquire(
            &primary_root,
            workspace.mission_dir(),
            &primary_root,
            &common_dir,
            COORD_TRANSACTION_LOCK_TIMEOUT,
        )?;
        let journal = CoordJournal::new(&common_dir, workspace.slug());
        let intent = CoordRecord::intent(
            CoordRecordKind::Commit,
            workspace.slug(),
            "planning_commit",
            Some(self.destination.as_str()),
            Some(&primary_root),
            Some(&self.pinned_parent),
        );
        journal.begin(intent.clone())?;
        let repository = workspace.workspace().repository();
        let lock_receipt = PlanningLockReceipt {
            checkout_claim_lock: locks.claim_path().to_path_buf(),
            mission_lock_key: locks.mission().key().to_owned(),
            mission_lock_path: locks.mission().path().map(Path::to_path_buf),
        };
        let current = repository.read_branch_tip(&self.destination)?;
        if current.as_ref() != Some(&self.pinned_parent) {
            let detail = format!(
                "destination {} moved from {} to {:?} after planning",
                self.destination,
                self.pinned_parent,
                current.as_ref().map(ToString::to_string)
            );
            let fault = resolve_intent(
                &journal,
                &intent,
                CoordPhase::Abandoned,
                Some(&detail),
                None,
            );
            return Err(WorkspaceError::CoordRefused {
                code: "PLANNING_PLAN_STALE",
                message: match fault {
                    Some(fault) => format!(
                        "{detail}; the coordination journal could not record the abandoned attempt either: {fault}"
                    ),
                    None => detail,
                },
            });
        }
        // The claim + Mission locks stay held through the commit; they are
        // dropped at the end of this function. Releasing them before the
        // mutation would put an unlocked writer on the destination branch.
        let request = SafeCommitRequest::new(
            self.destination.clone(),
            self.message.clone(),
            self.paths.clone(),
        )
        .with_protection(self.protected.clone(), CommitGuardCapability::Standard)
        .with_expected_parent(self.pinned_parent.clone())
        .with_expected_bytes(self.expected_bytes.clone());
        let fault: Option<String>;
        let outcome = match repository.safe_commit(&request) {
            Ok(outcome) => {
                let landed = LandedCommit {
                    commit: outcome.commit().clone(),
                    destination: outcome.destination().as_str().to_owned(),
                    worktree_root: outcome.worktree_root().to_path_buf(),
                    committed_paths: outcome
                        .committed_paths()
                        .iter()
                        .map(GitPath::display_lossy)
                        .collect(),
                    observed_tip: Some(self.pinned_parent.clone()),
                    index_repair: None,
                };
                fault = resolve_intent(
                    &journal,
                    &intent,
                    CoordPhase::Applied,
                    None,
                    Some(&landed.commit),
                );
                CoordCommitOutcome::Landed(landed)
            }
            Err(GitError::SafeCommitNothingToCommit { .. }) => {
                fault = resolve_intent(
                    &journal,
                    &intent,
                    CoordPhase::Applied,
                    Some("nothing to commit"),
                    None,
                );
                CoordCommitOutcome::NoOp {
                    head: self.pinned_parent.clone(),
                    destination: self.destination.as_str().to_owned(),
                }
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
                        .map(GitPath::display_lossy)
                        .collect(),
                    observed_tip: Some(self.pinned_parent.clone()),
                    index_repair: Some(detail.clone()),
                };
                fault = resolve_intent(
                    &journal,
                    &intent,
                    CoordPhase::Applied,
                    Some(&format!("landed with index repair required: {detail}")),
                    Some(&landed.commit),
                );
                CoordCommitOutcome::Landed(landed)
            }
            Err(GitError::SafeCommitAdvanceUnconfirmed {
                candidate, detail, ..
            }) => {
                let current = repository.read_branch_tip(&self.destination)?;
                if current.as_ref() == Some(&candidate) {
                    let landed = LandedCommit {
                        commit: candidate,
                        destination: self.destination.as_str().to_owned(),
                        worktree_root: primary_root,
                        committed_paths: self.paths.iter().map(GitPath::display_lossy).collect(),
                        observed_tip: Some(self.pinned_parent.clone()),
                        index_repair: Some(detail.clone()),
                    };
                    fault = resolve_intent(
                        &journal,
                        &intent,
                        CoordPhase::Applied,
                        Some(&format!("advance confirmed by readback: {detail}")),
                        Some(&landed.commit),
                    );
                    CoordCommitOutcome::Landed(landed)
                } else if current.as_ref() == Some(&self.pinned_parent) {
                    fault = resolve_intent(
                        &journal,
                        &intent,
                        CoordPhase::Abandoned,
                        Some(&detail),
                        Some(&candidate),
                    );
                    CoordCommitOutcome::NotLanded {
                        candidate,
                        observed_tip: current,
                        detail,
                    }
                } else {
                    fault = resolve_intent(
                        &journal,
                        &intent,
                        CoordPhase::Indeterminate,
                        Some(&detail),
                        Some(&candidate),
                    );
                    CoordCommitOutcome::Indeterminate {
                        candidate,
                        observed_tip: current,
                        detail,
                    }
                }
            }
            Err(error) => {
                let detail = error.to_string();
                let fault = resolve_intent(
                    &journal,
                    &intent,
                    CoordPhase::Abandoned,
                    Some(&detail),
                    None,
                );
                let error = WorkspaceError::Git(error);
                return Err(match fault {
                    Some(fault) => WorkspaceError::CoordRefused {
                        code: "PLANNING_COMMIT_FAILED_WITH_JOURNAL_FAULT",
                        message: format!(
                            "{error}; the coordination journal could not record the failed attempt either: {fault}"
                        ),
                    },
                    None => error,
                });
            }
        };
        Ok(PlanningCommitReceipt {
            outcome,
            destination: self.destination,
            pinned_parent: self.pinned_parent,
            pin_class: self.pin_class,
            meta_guard: self.meta_guard,
            paths: self.paths,
            journal_path: journal.path().to_path_buf(),
            journal_fault: fault,
            lock_receipt,
        })
    }
}

/// The locks a planning commit actually held.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanningLockReceipt {
    /// The kernel checkout-claim lock file.
    pub checkout_claim_lock: PathBuf,
    /// The canonical Mission lock key.
    pub mission_lock_key: String,
    /// The Mission lock file, when the guard exposes it.
    pub mission_lock_path: Option<PathBuf>,
}

/// Evidence of one planning commit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanningCommitReceipt {
    /// The commit outcome.
    pub outcome: CoordCommitOutcome,
    /// The write branch.
    pub destination: BranchName,
    /// The parent the commit was pinned to.
    pub pinned_parent: Oid,
    /// The pin classification observed at plan time.
    pub pin_class: PinClass,
    /// The demotion guard outcome.
    pub meta_guard: MetaDemotionGuard,
    /// The committed paths.
    pub paths: Vec<GitPath>,
    /// The coordination journal document.
    pub journal_path: PathBuf,
    /// A journal fault recorded after the outcome was proven.
    pub journal_fault: Option<String>,
    /// The locks this commit held while it ran.
    pub lock_receipt: PlanningLockReceipt,
}

impl PlanningCommitReceipt {
    /// The landed commit, when one landed.
    pub fn landed(&self) -> Option<&LandedCommit> {
        self.outcome.landed()
    }
}

/// Resolve a journal intent, returning the fault instead of failing the mutation.
fn resolve_intent(
    journal: &CoordJournal,
    intent: &CoordRecord,
    phase: CoordPhase,
    detail: Option<&str>,
    candidate: Option<&Oid>,
) -> Option<String> {
    journal
        .resolve(intent, phase, detail, candidate)
        .err()
        .map(|error| error.to_string())
}

fn ensure_primary_partition(path: &GitPath, mission_dir_name: &str) -> Result<(), WorkspaceError> {
    let lookup = path.display_lossy();
    // `meta.json` and unrecognised paths are fail-safe toward PRIMARY, exactly
    // like the upstream classifier.
    if let Some(kind) = kind_for_mission_file(&lookup, Some(mission_dir_name))
        && kind.is_coord_partition()
    {
        return Err(WorkspaceError::CoordRefused {
            code: "PLANNING_PARTITION_REFUSED",
            message: format!(
                "COORD-partition artifact {lookup} reached the planning commit; coordination kinds must commit to the coordination branch"
            ),
        });
    }
    Ok(())
}

fn is_meta_json(path: &GitPath, mission_dir_name: &str) -> bool {
    let display = path.display_lossy();
    display == format!("{SPECS_DIR_NAME}/{mission_dir_name}/meta.json")
}

/// The reference's demotion guard, read from the authoritative `HEAD` blob.
///
/// * no `HEAD` baseline for the path (or an unborn `HEAD`) → allowed (a
///   legitimate first commit, including a genuinely flat mission);
/// * a `HEAD` blob that is not valid JSON → refused (fail closed);
/// * a working copy that is not valid JSON → refused;
/// * `HEAD.coordination_branch` **truthy** (present, non-null and non-empty) and
///   the working copy's **falsy** (absent, `null` or `""`) → refused.
///
/// The last rule is the one the previous baseline mapping got wrong: `""` counted
/// as "routed" in one direction only, so `coordination_branch: ""` was a
/// fail-open on the exact guard this exists for.
fn check_meta_demotion(
    repository: &sk_git::GitRepository,
    working_path: &Path,
    git_path: &GitPath,
) -> Result<MetaDemotionGuard, WorkspaceError> {
    let baseline = match repository.read_blob_at_head(git_path) {
        Ok(BlobLookup::Present(bytes)) => bytes,
        Ok(BlobLookup::Absent | BlobLookup::NoRevision) => {
            return Ok(MetaDemotionGuard::NoHeadBaseline);
        }
        Err(error) => {
            return Err(WorkspaceError::CoordRefused {
                code: "META_DEMOTION_GUARD_CORRUPT",
                message: format!(
                    "could not read the HEAD baseline of {}: {error}. Refusing to auto-commit planning artifacts until it is repaired",
                    git_path.display_lossy()
                ),
            });
        }
    };
    let head: Value =
        serde_json::from_slice(&baseline).map_err(|error| WorkspaceError::CoordRefused {
            code: "META_DEMOTION_GUARD_CORRUPT",
            message: format!(
                "{} (HEAD) is not valid JSON: {error}. Refusing to auto-commit planning artifacts until it is repaired",
                git_path.display_lossy()
            ),
        })?;
    let working_bytes = std::fs::read(working_path).map_err(|source| {
        WorkspaceError::Kernel(sk_kernel::KernelError::Io {
            action: "read the working meta.json for the demotion guard",
            path: working_path.to_path_buf(),
            source,
        })
    })?;
    let working: Value =
        serde_json::from_slice(&working_bytes).map_err(|error| WorkspaceError::CoordRefused {
            code: "META_DEMOTION_GUARD_CORRUPT",
            message: format!(
                "{} (working copy) is not valid JSON: {error}. Refusing to auto-commit planning artifacts until it is repaired",
                git_path.display_lossy()
            ),
        })?;
    let head_branch = head.get("coordination_branch");
    let working_branch = working.get("coordination_branch");
    if python_truthy(head_branch) && !python_truthy(working_branch) {
        return Err(WorkspaceError::CoordRefused {
            code: "META_DEMOTION_REFUSED",
            message: format!(
                "the uncommitted {} drops coordination_branch (HEAD had {head_branch:?}, the working copy has {working_branch:?}); refusing to carry a whole-team routing change into the planning commit. Restore the key, or commit the flattened meta.json deliberately",
                git_path.display_lossy()
            ),
        });
    }
    Ok(MetaDemotionGuard::Preserved)
}

/// Python truthiness of a JSON value (`if head_branch and not working_branch`).
fn python_truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_f64().is_some_and(|number| number != 0.0),
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Array(items)) => !items.is_empty(),
        Some(Value::Object(map)) => !map.is_empty(),
    }
}

// ---------------------------------------------------------------------------
// Recording the planning pin
// ---------------------------------------------------------------------------

/// What a pin write does when the manifest already records a pin.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanningPinMode {
    /// Keep an existing pin; only record one when none is recorded.
    ///
    /// The reference freezes the pin once execution has begun
    /// (`_preserve_or_capture_planning_commit_sha`, #3311/#4141): a re-finalize
    /// for an ownership-only amendment must not silently re-capture the current
    /// branch tip and clobber the provenance a lane may already carry a
    /// merge-base against.
    PreserveExisting,
    /// Re-point the pin to the supplied commit (the explicit refresh path).
    Replace,
}

/// What one pin write did.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanningPinReceipt {
    /// The manifest as written.
    pub manifest: LanesManifest,
    /// The pin now recorded.
    pub recorded: Option<Oid>,
    /// The pin the manifest already carried, when it was preserved.
    pub preserved: Option<Oid>,
}

/// Persist a landed planning commit into `lanes.json`, preserving an existing pin.
///
/// The read-modify-write happens **once**, under the store's cross-process lock,
/// so a concurrent finalize cannot interleave between the read and the write. The
/// previous implementation read unlocked, overwrote `planning_commit_sha`
/// unconditionally and wrote under the lock: a lost update, and a clobbered pin.
pub fn record_planning_commit(
    store: &LanesStore,
    commit: &Oid,
) -> Result<PlanningPinReceipt, WorkspaceError> {
    record_planning_commit_with(store, commit, PlanningPinMode::PreserveExisting)
}

/// [`record_planning_commit`] with an explicit pin mode.
pub fn record_planning_commit_with(
    store: &LanesStore,
    commit: &Oid,
    mode: PlanningPinMode,
) -> Result<PlanningPinReceipt, WorkspaceError> {
    let mut preserved = None;
    let mut recorded = None;
    let manifest = store
        .update(|manifest| {
            if let Some(existing) = manifest.planning_commit_sha.as_deref() {
                let existing_oid = Oid::parse(existing)?;
                if mode == PlanningPinMode::PreserveExisting {
                    preserved = Some(existing_oid.clone());
                    recorded = Some(existing_oid);
                    return Ok(manifest.clone());
                }
            }
            let updated = manifest.with_planning_commit(commit);
            recorded = Some(commit.clone());
            Ok(updated)
        })?
        .ok_or_else(|| WorkspaceError::InvalidArtifact {
            artifact: "lanes.json",
            detail: format!("no manifest at {}", store.path().display()),
        })?;
    Ok(PlanningPinReceipt {
        manifest,
        recorded,
        preserved,
    })
}

/// Does `lane_branch` contain `planning_commit`?
pub fn lane_contains_planning_commit(
    repository: &sk_git::GitRepository,
    lane_branch: &str,
    planning_commit: &Oid,
) -> Result<bool, WorkspaceError> {
    let lane_branch = BranchName::parse(lane_branch)?;
    let Some(tip) = repository.read_branch_tip(&lane_branch)? else {
        return Ok(false);
    };
    Ok(repository.is_ancestor(planning_commit, &tip)?)
}

// ---------------------------------------------------------------------------
// Integration: fast-forward or a real pinned-head merge
// ---------------------------------------------------------------------------

/// A real, applied fast-forward ref advance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LandedAdvance {
    /// The branch that moved.
    pub branch: String,
    /// Its previous tip.
    pub previous_tip: Oid,
    /// Its new tip.
    pub new_tip: Oid,
    /// Checked-out worktrees that were resynchronised.
    pub resynced: Vec<PathBuf>,
}

/// A real merge commit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LandedMerge {
    /// The branch that moved.
    pub branch: String,
    /// The `HEAD` the merge started from.
    pub previous_head: Oid,
    /// The merge commit.
    pub new_head: Oid,
    /// The source tip that was merged in.
    pub merged_tip: Oid,
    /// The `sk-git` conclusion diagnostic when the substrate reported the
    /// conclusion as *unconfirmed* although the branch tip proves the landing.
    ///
    /// The merge is real either way; the note keeps the substrate's evidence
    /// (a hook that re-created an operation marker, say) attached to the receipt
    /// instead of discarding it.
    pub unconfirmed_detail: Option<String>,
}

/// What integrating one source tip into a destination did.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IntegrationOutcome {
    /// The destination moved forward to the source tip.
    FastForwarded(LandedAdvance),
    /// A real merge commit landed on the destination.
    Merged(LandedMerge),
    /// The source was already reachable from the destination.
    AlreadyIntegrated {
        /// The destination tip.
        destination_tip: Oid,
        /// The source tip.
        source_tip: Oid,
    },
}

impl IntegrationOutcome {
    /// The applied advance, when this outcome moved the destination.
    pub fn advanced(&self) -> Option<&LandedAdvance> {
        match self {
            Self::FastForwarded(advance) => Some(advance),
            _ => None,
        }
    }

    /// The applied merge, when this outcome created one.
    pub fn merged(&self) -> Option<&LandedMerge> {
        match self {
            Self::Merged(merge) => Some(merge),
            _ => None,
        }
    }

    /// Did this outcome change the destination at all?
    pub fn mutated(&self) -> bool {
        matches!(self, Self::FastForwarded(_) | Self::Merged(_))
    }
}

/// One dependency lane that was skipped, with the reference's warn-and-continue
/// reason.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkippedDependency {
    /// The dependency lane id.
    pub lane_id: String,
    /// Its canonical branch name.
    pub branch: String,
    /// Why it was skipped.
    pub reason: String,
}

/// One dependency lane's integration result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DependencyIntegration {
    /// The dependency lane id.
    pub lane_id: String,
    /// Its branch.
    pub branch: String,
    /// The outcome.
    pub outcome: IntegrationOutcome,
}

/// Evidence of the all-or-nothing rewind performed after a failed integration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntegrationRollback {
    /// The branch that was rewound.
    pub branch: String,
    /// The tip it was rewound from (the last integration that landed).
    pub from: Oid,
    /// The tip it was restored to (the pre-loop tip).
    pub to: Oid,
    /// The checkout that was resynchronised.
    pub checkout: PathBuf,
}

/// A merge conclusion the substrate could not confirm, with the evidence.
///
/// The merge may have landed. `sk-git` refuses to guess: it reports the commit it
/// created (when provable), the `HEAD` it observed and whether the operation
/// marker is gone, and never resets anything. This crate therefore never aborts
/// the operation and never rewinds the destination on this path: an abort could
/// discard a landed merge, and reporting an ordinary retryable failure after a
/// landed mutation is a false statement about the repository.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnconfirmedMerge {
    /// `merge`, `cherry-pick`, or `revert`.
    pub operation: String,
    /// The commit the conclusion created, when the substrate could prove it.
    /// `None` means unknown — never "nothing landed".
    pub candidate: Option<Oid>,
    /// The `HEAD` the plan pinned.
    pub previous_head: Oid,
    /// `HEAD` as read back after the command and its hooks.
    pub observed_head: Option<Oid>,
    /// Whether the operation marker is gone.
    pub operation_consumed: bool,
    /// The destination tip read back by this crate after the failure.
    pub destination_tip: Option<Oid>,
    /// Proven: the candidate is the destination tip or reachable from it.
    pub landed: bool,
    /// Git's diagnostic plus the substrate's evidence probes.
    pub detail: String,
}

/// The result of integrating a lane's dependencies.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IntegrationReport {
    /// Integrations in execution order.
    pub integrations: Vec<DependencyIntegration>,
    /// Dependencies skipped because their branch does not resolve.
    pub skipped: Vec<SkippedDependency>,
    /// The all-or-nothing rewind, when a later failure undid earlier advances.
    pub rollback: Option<IntegrationRollback>,
    /// The coordination journal this loop recorded into.
    pub journal_path: PathBuf,
    /// A journal fault recorded after the loop's outcome was proven.
    ///
    /// A landed integration is immutable evidence: failing to record it is
    /// reported here instead of turning the whole loop into a failure.
    pub journal_fault: Option<String>,
    /// A merge whose conclusion the substrate could not confirm.
    pub unconfirmed: Option<UnconfirmedMerge>,
}

impl IntegrationReport {
    /// Integrations that actually moved the destination.
    pub fn applied(&self) -> Vec<&DependencyIntegration> {
        self.integrations
            .iter()
            .filter(|entry| entry.outcome.mutated())
            .collect()
    }

    /// Was every recorded dependency integrated (a skip is a warning)?
    pub fn is_complete(&self) -> bool {
        self.unconfirmed.is_none() && self.rollback.is_none()
    }
}

/// A failed integration that still carries the receipts it produced.
///
/// The whole report used to be dropped by `?`, so already-performed real advances
/// were invisible to the caller. This type keeps them, names the rollback that
/// undid them, and still exposes the typed failure.
#[derive(Debug)]
pub struct IntegrationError {
    /// The typed failure.
    ///
    /// Boxed with the report: both together tripped
    /// `clippy::result_large_err` on every integration entry point.
    pub error: Box<WorkspaceError>,
    /// Everything observed before the failure, including the rollback.
    ///
    /// Boxed: the receipt is much larger than the typed failure, and returning
    /// it inline tripped `clippy::result_large_err` on every integration entry
    /// point.
    pub report: Box<IntegrationReport>,
    /// A possibly-landed merge the substrate could not confirm.
    ///
    /// Never rewound, never reported as an ordinary retryable failure: the
    /// evidence is what an operator decides on. Boxed, like the other large
    /// fields, to keep the error type small.
    pub unconfirmed: Option<Box<UnconfirmedMerge>>,
}

impl IntegrationError {
    fn new(error: WorkspaceError, report: IntegrationReport) -> Self {
        Self {
            error: Box::new(error),
            report: Box::new(report),
            unconfirmed: None,
        }
    }

    fn unconfirmed(mut report: IntegrationReport, evidence: UnconfirmedMerge) -> Self {
        let detail = format!(
            "the {} conclusion is unconfirmed: created commit {:?}, observed HEAD {:?}, destination tip {:?}, operation marker consumed: {}; nothing was aborted or rewound. {}",
            evidence.operation,
            evidence.candidate,
            evidence.observed_head,
            evidence.destination_tip,
            evidence.operation_consumed,
            evidence.detail
        );
        report.unconfirmed = Some(evidence.clone());
        Self {
            error: Box::new(WorkspaceError::CoordRefused {
                code: "INTEGRATION_MERGE_UNCONFIRMED",
                message: detail,
            }),
            report: Box::new(report),
            unconfirmed: Some(Box::new(evidence)),
        }
    }
}

impl std::fmt::Display for IntegrationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.report.rollback {
            Some(rollback) => write!(
                formatter,
                "{} (integration rolled {} back from {} to {})",
                self.error, rollback.branch, rollback.from, rollback.to
            ),
            None => write!(formatter, "{}", self.error),
        }
    }
}

impl From<IntegrationError> for WorkspaceError {
    fn from(failure: IntegrationError) -> Self {
        if failure.unconfirmed.is_some() {
            // A possibly-landed merge decides the answer: never the generic
            // rolled-back/refused shape, which would claim the repository is
            // unchanged.
            return *failure.error;
        }
        let report = *failure.report;
        match report.rollback {
            Some(rollback) => WorkspaceError::CoordRefused {
                code: "INTEGRATION_FAILED_AND_ROLLED_BACK",
                message: format!(
                    "{}; {} was rolled back from {} to {}",
                    failure.error, rollback.branch, rollback.from, rollback.to
                ),
            },
            None => *failure.error,
        }
    }
}

/// The checkout one integration writes, resolved from the lane manifest.
struct IntegrationCheckout {
    repository: sk_git::GitRepository,
    checkout: PathBuf,
    destination: BranchName,
}

fn integration_checkout(
    workspace: &MissionWorkspace,
    manifest: &LanesManifest,
    lane_id: &str,
    protected: &ProtectedBranches,
) -> Result<IntegrationCheckout, WorkspaceError> {
    let lane = manifest
        .lanes
        .iter()
        .find(|lane| lane.lane_id == lane_id)
        .ok_or_else(|| WorkspaceError::LaneNotFound {
            wp: lane_id.to_owned(),
        })?;
    let primary_root = workspace.workspace().primary_root().to_path_buf();
    if lane.is_planning() || lane_id == PLANNING_LANE_ID {
        // The planning lane writes the shared primary checkout, whose branch is
        // the mission's CANONICAL write branch (never a protected target when a
        // `single_branch` mission minted a `mission_branch`).
        let destination = workspace.write_branch(protected)?;
        return Ok(IntegrationCheckout {
            repository: sk_git::GitRepository::open(&primary_root)?,
            checkout: primary_root,
            destination,
        });
    }
    let checkout = crate::naming::worktree_path(&primary_root, &manifest.mission_slug, lane_id)?;
    if !checkout.exists() {
        return Err(WorkspaceError::CoordRefused {
            code: "LANE_WORKTREE_MISSING",
            message: format!(
                "cannot integrate dependencies into {lane_id}: {} is not materialized",
                checkout.display()
            ),
        });
    }
    let destination = BranchName::parse(&crate::naming::code_lane_branch_name(
        &manifest.mission_slug,
        lane_id,
    )?)?;
    Ok(IntegrationCheckout {
        repository: sk_git::GitRepository::open(&checkout)?,
        checkout,
        destination,
    })
}

/// Integrate the recorded planning commit into a lane.
pub fn integrate_planning_commit(
    workspace: &MissionWorkspace,
    manifest: &LanesManifest,
    lane_id: &str,
    planning_commit: &Oid,
    protected: &ProtectedBranches,
) -> Result<IntegrationReport, IntegrationError> {
    let checkout = integration_checkout(workspace, manifest, lane_id, protected)
        .map_err(|error| IntegrationError::new(error, IntegrationReport::default()))?;
    let sources = vec![(
        "planning".to_owned(),
        "the recorded planning commit".to_owned(),
        Some(planning_commit.clone()),
    )];
    run_integrations(
        workspace,
        checkout,
        "integrate_planning_commit",
        sources,
        protected,
    )
}

/// Integrate every approved dependency lane tip into `lane_id`.
///
/// Mirrors the reference (`worktree_allocator._merge_dependency_lane_tips`):
///
/// * dependencies run in `(parallel_group, lane_id)` order — for the planning
///   lane too, which previously sorted by lane id alone;
/// * a dependency branch that does not resolve is **warned and skipped**, not a
///   hard failure (a merged-and-deleted lane is a normal post-mission shape);
/// * a dependency already reachable from the destination is skipped;
/// * a divergent dependency is a **real merge**, not a refusal;
/// * the whole loop is **all-or-nothing**: a later conflict (or any later
///   failure) rewinds the destination to the exact pre-loop tip, so no earlier
///   clean merge survives it. The rewind is reported, and the receipts gathered
///   before the failure are returned with the error.
pub fn integrate_dependency_lanes(
    workspace: &MissionWorkspace,
    manifest: &LanesManifest,
    lane_id: &str,
    protected: &ProtectedBranches,
) -> Result<IntegrationReport, IntegrationError> {
    let empty_report = IntegrationReport::default();
    let lane = manifest
        .lanes
        .iter()
        .find(|lane| lane.lane_id == lane_id)
        .ok_or_else(|| {
            IntegrationError::new(
                WorkspaceError::LaneNotFound {
                    wp: lane_id.to_owned(),
                },
                empty_report.clone(),
            )
        })?;
    if lane.depends_on_lanes.is_empty() {
        return Ok(empty_report);
    }
    let ordered: Vec<&crate::lanes::ExecutionLane> = {
        let mut resolved: Vec<&crate::lanes::ExecutionLane> = lane
            .depends_on_lanes
            .iter()
            .filter_map(|id| manifest.lanes.iter().find(|lane| &lane.lane_id == id))
            .collect();
        resolved.sort_by(|left, right| {
            (left.parallel_group, left.lane_id.as_str())
                .cmp(&(right.parallel_group, right.lane_id.as_str()))
        });
        resolved
    };
    let checkout = integration_checkout(workspace, manifest, lane_id, protected)
        .map_err(|error| IntegrationError::new(error, empty_report.clone()))?;
    let mut sources = Vec::with_capacity(ordered.len());
    for dependency in ordered {
        // The reference resolves EVERY dependency through the one canonical lane
        // grammar with the manifest's target branch
        // (`worktree_allocator._merge_dependency_lane_tips` →
        // `lane_branch_name(mission_slug, lane_id, target_branch=…)`).
        //
        // Using `code_lane_branch_name` here refused a dependency on the
        // PLANNING lane outright (that composer rejects `lane-planning` by
        // design), so a code lane that depends on a planning-artifact WP failed
        // integration for a dependency the reference merges from the target
        // branch. `lane_branch_name` returns `target_branch` for that lane and
        // the code-lane grammar for every other one.
        let branch = crate::naming::lane_branch_name(
            &manifest.mission_slug,
            &dependency.lane_id,
            &manifest.target_branch,
        )
        .map_err(|error| IntegrationError::new(error, IntegrationReport::default()))?;
        let branch_name = BranchName::parse(&branch).map_err(|error| {
            IntegrationError::new(WorkspaceError::Git(error), IntegrationReport::default())
        })?;
        let tip = checkout
            .repository
            .read_branch_tip(&branch_name)
            .map_err(|error| {
                IntegrationError::new(WorkspaceError::Git(error), IntegrationReport::default())
            })?;
        sources.push((dependency.lane_id.clone(), branch, tip));
    }
    run_integrations(
        workspace,
        checkout,
        "integrate_dependency_lanes",
        sources,
        protected,
    )
}

/// Run one journaled, locked, all-or-nothing integration loop.
fn run_integrations(
    workspace: &MissionWorkspace,
    checkout: IntegrationCheckout,
    operation: &str,
    sources: Vec<(String, String, Option<Oid>)>,
    protected: &ProtectedBranches,
) -> Result<IntegrationReport, IntegrationError> {
    let primary_root = workspace.workspace().primary_root().to_path_buf();
    let common_dir = workspace.workspace().common_dir().to_path_buf();
    let locks = CoordLocks::acquire(
        &checkout.checkout,
        workspace.mission_dir(),
        &primary_root,
        &common_dir,
        COORD_TRANSACTION_LOCK_TIMEOUT,
    )
    .map_err(|error| IntegrationError::new(error, IntegrationReport::default()))?;
    let journal = CoordJournal::new(&common_dir, workspace.slug());
    let pre_loop_tip = match checkout.repository.read_branch_tip(&checkout.destination) {
        Ok(tip) => tip,
        Err(error) => {
            return Err(IntegrationError::new(
                WorkspaceError::Git(error),
                IntegrationReport::default(),
            ));
        }
    };
    let intent = CoordRecord::intent(
        CoordRecordKind::Integration,
        workspace.slug(),
        operation,
        Some(checkout.destination.as_str()),
        Some(&checkout.checkout),
        pre_loop_tip.as_ref(),
    );
    if let Err(error) = journal.begin(intent.clone()) {
        return Err(IntegrationError::new(error, IntegrationReport::default()));
    }

    let mut report = IntegrationReport {
        journal_path: journal.path().to_path_buf(),
        ..IntegrationReport::default()
    };
    let mut last_tip = pre_loop_tip.clone();
    for (lane_id, branch_text, source_tip) in sources {
        let Some(source_tip) = source_tip else {
            // Missing dependency branch: the reference warns and continues.
            report.skipped.push(SkippedDependency {
                lane_id,
                branch: branch_text.clone(),
                reason: format!(
                    "dependency branch {branch_text} does not resolve; it may have been merged and deleted, so this lane will not contain its tip"
                ),
            });
            continue;
        };
        let outcome = integrate_into_checkout(
            &checkout.repository,
            &checkout.destination,
            &source_tip,
            &branch_text,
            protected,
            &format!("chore(spec-kitty): merge {branch_text}"),
        );
        match outcome {
            Ok(outcome) => {
                if let Some(tip) = outcome_tip(&outcome) {
                    last_tip = Some(tip);
                }
                report.integrations.push(DependencyIntegration {
                    lane_id,
                    branch: branch_text,
                    outcome,
                });
            }
            Err(IntegrationFailure::NotLanded(error)) => {
                let rollback = rollback_integrations(
                    &checkout.repository,
                    &checkout.checkout,
                    &checkout.destination,
                    pre_loop_tip.as_ref(),
                );
                report.rollback = rollback.clone();
                let detail = match &rollback {
                    Some(rollback) => format!(
                        "{}; earlier advances were rolled back ({} from {} to {})",
                        error, rollback.branch, rollback.from, rollback.to
                    ),
                    None => error.to_string(),
                };
                // Journaled, not discarded: the fault is evidence and is
                // reported on the receipt *and* in the failure text.
                report.journal_fault = resolve_intent(
                    &journal,
                    &intent,
                    CoordPhase::Abandoned,
                    Some(&detail),
                    pre_loop_tip.as_ref(),
                );
                drop(locks);
                // The step's own typed code is preserved (`INTEGRATION_FAILED`
                // used to flatten every refusal into one opaque code); a journal
                // fault is named in the code and the message.
                let error = match error {
                    WorkspaceError::CoordRefused { code, .. } => {
                        let code = if report.journal_fault.is_some() {
                            "INTEGRATION_FAILED_WITH_JOURNAL_FAULT"
                        } else {
                            code
                        };
                        let message = match &report.journal_fault {
                            Some(fault) => format!(
                                "{detail}; the coordination journal could not record the abandoned integration either: {fault}"
                            ),
                            None => detail,
                        };
                        WorkspaceError::CoordRefused { code, message }
                    }
                    other => match &report.journal_fault {
                        Some(fault) => WorkspaceError::CoordRefused {
                            code: "INTEGRATION_FAILED_WITH_JOURNAL_FAULT",
                            message: format!(
                                "{detail}; the coordination journal could not record the abandoned integration either: {fault}"
                            ),
                        },
                        None => other,
                    },
                };
                return Err(IntegrationError::new(error, report));
            }
            Err(IntegrationFailure::Unconfirmed(evidence)) => {
                // A merge may exist. Never abort it, never rewind the
                // destination: the candidate commit is real evidence and a
                // rewind is exactly the data loss this branch exists to avoid.
                if evidence.landed
                    && let Some(candidate) = evidence.candidate.clone()
                {
                    report.integrations.push(DependencyIntegration {
                        lane_id,
                        branch: branch_text.clone(),
                        outcome: IntegrationOutcome::Merged(LandedMerge {
                            branch: checkout.destination.as_str().to_owned(),
                            previous_head: evidence.previous_head.clone(),
                            new_head: candidate,
                            merged_tip: source_tip.clone(),
                            unconfirmed_detail: Some(evidence.detail.clone()),
                        }),
                    });
                }
                let detail = format!(
                    "integrated dependencies stopped after {branch_text}: {}",
                    evidence.detail
                );
                report.journal_fault = resolve_intent(
                    &journal,
                    &intent,
                    CoordPhase::Indeterminate,
                    Some(&detail),
                    evidence.candidate.as_ref(),
                );
                drop(locks);
                return Err(IntegrationError::unconfirmed(report, *evidence));
            }
        }
    }
    report.journal_fault = resolve_intent(
        &journal,
        &intent,
        CoordPhase::Applied,
        Some("dependency integration loop completed"),
        last_tip.as_ref(),
    );
    drop(locks);
    Ok(report)
}

impl From<WorkspaceError> for IntegrationFailure {
    fn from(error: WorkspaceError) -> Self {
        Self::NotLanded(error)
    }
}

impl From<GitError> for IntegrationFailure {
    fn from(error: GitError) -> Self {
        Self::NotLanded(WorkspaceError::Git(error))
    }
}

/// Why one successive integration step could not produce an outcome.
enum IntegrationFailure {
    /// The destination provably did not move for this step. The loop fails and
    /// rewinds its own earlier advances (all-or-nothing).
    NotLanded(WorkspaceError),
    /// A merge commit may exist. Nothing may be aborted or rewound.
    ///
    /// Boxed: [`UnconfirmedMerge`] is evidence-sized, and returning it inline
    /// tripped `clippy::result_large_err` on every integration entry point.
    Unconfirmed(Box<UnconfirmedMerge>),
}

fn outcome_tip(outcome: &IntegrationOutcome) -> Option<Oid> {
    match outcome {
        IntegrationOutcome::FastForwarded(advance) => Some(advance.new_tip.clone()),
        IntegrationOutcome::Merged(merge) => Some(merge.new_head.clone()),
        _ => None,
    }
}

/// Fast-forward or merge `source_tip` into `destination`, in its own checkout.
fn integrate_into_checkout(
    repository: &sk_git::GitRepository,
    destination: &BranchName,
    source_tip: &Oid,
    source_label: &str,
    protected: &ProtectedBranches,
    message: &str,
) -> Result<IntegrationOutcome, IntegrationFailure> {
    let Some(destination_tip) = repository.read_branch_tip(destination)? else {
        return Err(IntegrationFailure::NotLanded(
            WorkspaceError::CoordRefused {
                code: "INTEGRATION_DESTINATION_MISSING",
                message: format!(
                    "cannot integrate {source_label} into {destination}: the destination branch does not exist"
                ),
            },
        ));
    };
    if source_tip == &destination_tip || repository.is_ancestor(source_tip, &destination_tip)? {
        return Ok(IntegrationOutcome::AlreadyIntegrated {
            destination_tip,
            source_tip: source_tip.clone(),
        });
    }
    if repository.is_ancestor(&destination_tip, source_tip)? {
        return fast_forward(
            repository,
            destination,
            &destination_tip,
            source_tip,
            source_label,
            protected,
            message,
        )
        .map_err(IntegrationFailure::NotLanded);
    }

    // Diverged: a real three-way merge, starting from a pinned HEAD.
    if repository
        .merge_base(&destination_tip, source_tip)?
        .is_none()
    {
        // The reference runs `git merge` here, git refuses an unrelated-history
        // merge, and the loop aborts and resets to its pre-loop ref
        // (`_merge_dependency_lane_tips`, #1915). So this is a FAILURE that
        // rewinds the loop's own earlier advances — never a receipt that leaves
        // them in place while calling the lane unintegrated.
        return Err(IntegrationFailure::NotLanded(
            WorkspaceError::CoordRefused {
                code: "INTEGRATION_UNRELATED_HISTORIES",
                message: format!(
                    "{destination} and {source_label} have no common ancestor; git refuses an unrelated-history merge, so the dependency loop fails and rewinds to its pre-loop tip"
                ),
            },
        ));
    }
    let observed = repository.current_branch()?;
    if observed.as_deref() != Some(destination.as_str()) {
        return Err(IntegrationFailure::NotLanded(
            WorkspaceError::CoordRefused {
                code: "INTEGRATION_CHECKOUT_MISMATCH",
                message: format!(
                    "the integration checkout has {:?} checked out, not {destination}; a merge must run in the checkout that carries the destination branch",
                    observed.unwrap_or_else(|| "<detached>".to_owned())
                ),
            },
        ));
    }
    let plan = repository.plan_merge_start(&StartPoint::Oid(source_tip.clone()))?;
    let start = repository.apply_merge_start(&plan)?;
    match start.status() {
        sk_git::MergeStartStatus::AlreadyUpToDate => Ok(IntegrationOutcome::AlreadyIntegrated {
            destination_tip: start.head().clone(),
            source_tip: source_tip.clone(),
        }),
        sk_git::MergeStartStatus::Conflicted => {
            let conflicts: Vec<String> = start
                .conflict_paths()
                .iter()
                .map(|path| path.display_lossy())
                .collect();
            let _ = repository.merge_abort();
            Err(IntegrationFailure::NotLanded(
                WorkspaceError::CoordRefused {
                    code: "INTEGRATION_MERGE_CONFLICT",
                    message: format!(
                        "merging {source_label} into {destination} conflicts on {}; the half-merge was aborted and the loop will rewind to its pre-loop tip",
                        conflicts.join(", ")
                    ),
                },
            ))
        }
        sk_git::MergeStartStatus::Stopped => {
            let conclusion = repository.plan_merge_conclusion(start.head(), Some(message))?;
            match repository.apply_merge_conclusion(&conclusion) {
                Ok(concluded) => Ok(IntegrationOutcome::Merged(LandedMerge {
                    branch: destination.as_str().to_owned(),
                    previous_head: concluded.previous_head().clone(),
                    new_head: concluded.new_head().clone(),
                    merged_tip: source_tip.clone(),
                    unconfirmed_detail: None,
                })),
                Err(GitError::MergeConclusionUnconfirmed(unconfirmed)) => {
                    // NEVER `merge_abort` on this arm: a hook may have re-created
                    // the operation marker *after* the commit landed, and an
                    // abort there would destroy a landed merge. The substrate
                    // itself never resets anything and refuses to guess.
                    let payload = *unconfirmed;
                    let destination_tip = repository.read_branch_tip(destination).ok().flatten();
                    // `sk-git` supplies only invocation-bound creation evidence,
                    // not a parent-set match alone. Reachability from the
                    // destination tip then proves that candidate landed. Missing
                    // evidence or unreadable ancestry stays unconfirmed.
                    let landed = match (&payload.candidate, &destination_tip) {
                        (Some(candidate), Some(tip)) => {
                            tip == candidate
                                || repository.is_ancestor(candidate, tip).unwrap_or(false)
                        }
                        _ => false,
                    };
                    if landed && payload.operation_consumed {
                        // The destination tip proves the landing even though the
                        // substrate's own post-state probe did not; keep its
                        // evidence on the receipt and continue the loop.
                        return Ok(IntegrationOutcome::Merged(LandedMerge {
                            branch: destination.as_str().to_owned(),
                            previous_head: payload.previous_head.clone(),
                            new_head: payload
                                .candidate
                                .clone()
                                .expect("a landed conclusion has a proven candidate"),
                            merged_tip: source_tip.clone(),
                            unconfirmed_detail: Some(payload.detail.clone()),
                        }));
                    }
                    Err(IntegrationFailure::Unconfirmed(Box::new(
                        UnconfirmedMerge {
                            operation: payload.operation.clone(),
                            candidate: payload.candidate.clone(),
                            previous_head: payload.previous_head.clone(),
                            observed_head: payload.observed_head.clone(),
                            operation_consumed: payload.operation_consumed,
                            destination_tip,
                            landed,
                            detail: format!(
                                "merging {source_label} into {destination} stopped at an unconfirmed {} conclusion: {}",
                                payload.operation, payload.detail
                            ),
                        },
                    )))
                }
                Err(error) => {
                    // Nothing landed (a rejecting `pre-commit` hook leaves the
                    // pinned HEAD, the marker and the index untouched), so the
                    // half-merge is discarded before the loop rewinds.
                    let _ = repository.merge_abort();
                    Err(IntegrationFailure::NotLanded(WorkspaceError::Git(error)))
                }
            }
        }
    }
}

fn fast_forward(
    repository: &sk_git::GitRepository,
    destination: &BranchName,
    destination_tip: &Oid,
    source_tip: &Oid,
    source_label: &str,
    protected: &ProtectedBranches,
    message: &str,
) -> Result<IntegrationOutcome, WorkspaceError> {
    let options = BranchAdvanceOptions::default().reset_clean_checkouts();
    let plan = match repository.plan_branch_advance(
        destination,
        source_tip.clone(),
        Some(destination_tip.clone()),
        protected,
        message,
        &options,
    ) {
        Ok(plan) => plan,
        Err(GitError::ProtectedTarget { .. }) => {
            return Err(WorkspaceError::CoordRefused {
                code: crate::policy::PROTECTED_BRANCH_REFUSED,
                message: format!(
                    "refusing to integrate {source_label}: destination {destination} is protected"
                ),
            });
        }
        Err(GitError::BranchCheckedOut { worktrees, .. }) => {
            return Err(WorkspaceError::CoordRefused {
                code: "INTEGRATION_CHECKOUT_DIRTY",
                message: format!(
                    "refusing to integrate {source_label}: {destination} is checked out in {}; the advance cannot resynchronise those checkouts",
                    worktrees
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            });
        }
        Err(GitError::DirtyWorktree { path, entries }) => {
            return Err(WorkspaceError::CoordRefused {
                code: "INTEGRATION_CHECKOUT_DIRTY",
                message: format!(
                    "refusing to integrate {source_label}: {} holds local changes ({}); commit or stash first",
                    path.display(),
                    entries.join(", ")
                ),
            });
        }
        Err(error) => return Err(WorkspaceError::Git(error)),
    };
    let outcome = repository.apply_branch_advance(&plan)?;
    Ok(IntegrationOutcome::FastForwarded(LandedAdvance {
        branch: outcome.branch().as_str().to_owned(),
        previous_tip: outcome
            .previous()
            .cloned()
            .unwrap_or_else(|| destination_tip.clone()),
        new_tip: outcome.new_tip().clone(),
        resynced: outcome.resynced().to_vec(),
    }))
}

/// Rewind the destination to its exact pre-loop tip (#1915 all-or-nothing).
///
/// The rewind is an explicit, audited non-fast-forward branch advance with the
/// checkout resynchronised, so the worktree matches the restored ref. The commits
/// it discards are ones this loop created moments earlier; the receipt names both
/// tips so nothing is silent.
fn rollback_integrations(
    repository: &sk_git::GitRepository,
    checkout: &Path,
    destination: &BranchName,
    pre_loop_tip: Option<&Oid>,
) -> Option<IntegrationRollback> {
    let current = repository.read_branch_tip(destination).ok().flatten()?;
    let target = pre_loop_tip?.clone();
    if current == target {
        return None;
    }
    let options = BranchAdvanceOptions::default()
        .reset_clean_checkouts()
        .allow_non_fast_forward();
    let plan = repository
        .plan_branch_advance(
            destination,
            target.clone(),
            Some(current.clone()),
            &ProtectedBranches::default(),
            "spec-kitty: roll back a failed dependency integration",
            &options,
        )
        .ok()?;
    if repository.apply_branch_advance(&plan).is_err() {
        return None;
    }
    Some(IntegrationRollback {
        branch: destination.as_str().to_owned(),
        from: current,
        to: target,
        checkout: checkout.to_path_buf(),
    })
}

/// A planning manifest's recorded pin, resolved for consumers.
pub fn recorded_pin(manifest: &LanesManifest) -> Result<Option<Oid>, WorkspaceError> {
    match manifest.planning_commit_sha.as_deref() {
        Some(sha) => Ok(Some(Oid::parse(sha)?)),
        None => Ok(None),
    }
}

/// Paths a planning commit may carry, in repository-relative POSIX form.
pub fn planning_paths(directory: &Path, entries: &[&str]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| {
            let joined = directory.join(entry);
            joined
                .components()
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/")
        })
        .collect()
}

/// The kinds a planning commit is allowed to carry (PRIMARY partition).
pub fn is_planning_kind(kind: MissionArtifactKind) -> bool {
    kind.is_primary_partition()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_journal_fault_is_returned_never_discarded() {
        let temp = tempfile::tempdir().unwrap();
        let journal = CoordJournal::new(temp.path(), "demo");
        let intent = CoordRecord::intent(
            CoordRecordKind::Integration,
            "demo",
            "integrate_dependency_lanes",
            Some("kitty/mission-demo-lane-b"),
            Some(temp.path()),
            None,
        );
        journal.begin(intent.clone()).unwrap();
        // Break the document *after* the intent is durable, exactly like a crash
        // between the mutation and its record.
        let path = journal.path().to_path_buf();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        let fault = resolve_intent(&journal, &intent, CoordPhase::Applied, Some("done"), None);
        assert!(fault.is_some(), "the fault is evidence, not a dropped `_`");
        assert!(journal.pending().is_err(), "the document is unreadable");
    }
}
