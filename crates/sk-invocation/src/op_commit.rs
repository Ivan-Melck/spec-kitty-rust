//! Best-effort Op-record / closure-spine commits, and the repository-bound
//! state shared with write admission.
//!
//! Source: `specify_cli/invocation/executor.py` (`_read_started_event`,
//! `_commit_op_record`, `_op_commit_kwargs`, `_op_commit_skip_reason`). This is
//! the REAL commit composition over the accepted native skip probes, the
//! canonical `ProtectionPolicy` inputs, and the canonical native commit
//! capability: the Source-judged J guard first, then one message-free exact-ref
//! `SafeCommitPhysicalRequest` publication plan and the late exact-byte payload
//! (`SafeCommitPhysicalPlan::prepare`),
//! then one `apply_safe_physical_commit`. Ordinary born branches, genuinely
//! unborn destinations and non-heads exact refs share that one publisher. It is
//! not an executor, dispatcher, or CLI adapter.
//!
//! # Byte-message bridge
//!
//! The source composes the message as a Python `str` and encodes it only when
//! `subprocess` builds the `git commit -m` argv, using the interpreter's
//! startup filesystem encoding with `surrogateescape` (so `\udc80..\udcff`
//! reach Git as the single bytes `0x80..0xff`). This module keeps the decoded
//! Python code points through composition and resolves the service's captured
//! startup context once ([`crate::python_os_encoding`]); the encoding happens
//! **after** the native target plan (paths, HEAD/tip, protection guard,
//! operation marker) and immediately before the payload guard. Supported normal
//! branches now reach protection before encoding; native progress is not proof
//! of source path semantics, J/P policy reachability or its later staging step.
//! The payload is
//! passed as exact `sk_git::CommitMessage` bytes; Git owns `i18n.commitEncoding`
//! and any object-body conversion, and nothing here re-encodes a message.
//!
//! # What is committed
//!
//! An agent close commits **only** its per-record file
//! (`kitty-ops/<ULID>.jsonl`). A doctor-sweep close commits **only** the
//! append-only closure spine (`kitty-ops/op-closures.jsonl`); a pre-existing
//! archive record is never modified, staged, or committed. The exact path is
//! passed to the canonical target plan, so unrelated staged content, hooks,
//! signing configuration, and the operator's captured index keep their accepted
//! `sk-git` behavior.
//!
//! # Started-record read
//!
//! `_read_started_event` is deliberately **not** `InvocationWriter::read_started`:
//! it reads the first JSON object only, so legacy/raw rows without a v2
//! claim/model still describe their `profile_id`/`action` (missing, null,
//! empty, and other false-y values become `unknown`). A missing, empty,
//! non-object, or undecodable first line is the source's
//! `Invalid invocation record: <id>` failure.
//!
//! The first line is decoded by the private [`crate::legacy_json`] reader, a
//! CPython `json.loads` compatibility profile: bare `NaN`/`Infinity`/
//! `-Infinity` are accepted, nested maps keep Python dict insertion order
//! (duplicate keys keep the first position and the last value), integer tokens
//! stay exact, and float-token overflow/underflow render like Python `float`
//! (`1e400` is `inf`, `1e-400` is a falsy signed zero). `str(value or
//! "unknown")` therefore composes the same commit message the reference
//! composes for those legacy rows. Matching subjects remove the demonstrated
//! message difference; the byte-message tests additionally bind equal
//! author/committer metadata, dates and every other header, so their actual
//! OIDs and complete commit objects are compared. That is exercised-row
//! evidence, not a blanket OID-equality claim.
//! The whole first line is still validated;
//! truncated, trailing-junk and non-object rows stay invalid.
//!
//! # Outcomes
//!
//! The source treats the audit write as durable and the commit as best-effort:
//! a protected destination, a detached checkout, a non-worktree root, a skip,
//! a hook failure, or a landed-but-uncertain commit never removes or rewrites
//! the audited rows. This module returns every one of those as an observable
//! [`OpCommitOutcome`], keeping the typed `sk-git` commit/landed evidence when
//! the backend produced one.
//!
//! # Limits
//!
//! * `_current_branch` in the reference falls back to `git rev-parse
//!   --abbrev-ref HEAD` when `symbolic-ref` answers nothing. The native adapter
//!   exposes the symbolic answer (`GitRepository::current_branch`); the fallback
//!   is not reproduced. A detached HEAD is `None`/`detached HEAD` on both.
//! * A non-UTF-8 record is a generic read failure here; the reference's Python
//!   `UnicodeDecodeError` is not one of the `_read_started_event` caught types.
//! * Git-process errors carry native wording, never a fabricated Python
//!   `repr(Exception)`.
//! * The default git timeout is the source's 30-second probe budget. The
//!   protection reader resolves its own remote-default probe through existing
//!   `sk-workspace` code, which retains that crate's accepted options.
//! * Raw `NaN`/`Infinity` JSON is accepted by Python and by this reader, with
//!   the same commit decision and subject for the exercised rows. Python code
//!   points (including lone surrogates) and nested `repr` use the verified
//!   CPython printability authority. A used top-level low surrogate
//!   `\udc80..\udcff` commits through the late filesystem encoder exactly like
//!   the source's `surrogateescape` boundary; every other used surrogate is
//!   truthfully unencodable (never `Invalid invocation record`). A
//!   source-valid startup context this adapter does not implement (for example
//!   a non-UTF-8, non-ASCII locale charset) is a truthful native capability
//!   failure, and the non-Unix wide-argv representation remains unverified.
//!   The source stages the record before its argv encoding fails while the
//!   native plan precedes staging, so the index-on-error footprint still
//!   differs; this bridge does not claim otherwise.
//!   The reader follows the default 4300-digit integer ceiling and uses the
//!   source interpreter's default 1000-frame recursion budget as a native
//!   operational guard against process loss, not an exact JSON-format/call-stack
//!   acceptance claim. The existing PYTHONINTMAXSTRDIGITS startup setting is
//!   honored; customized recursion settings/runtime Python limit mutations and
//!   caller-dependent source recursion limits are not emulated.
//! * J/P source targeting: the judged destination is the source-normalized
//!   `--short` name and the published destination is the exact physical ref
//!   named by the full symbolic `HEAD`. The source's whitespace-stripped shadow
//!   case commits on that physical ref, and the judged guard refuses
//!   protected/invalid shadow cases before the native plan. Ordinary born
//!   branches, genuinely unborn destinations (a zero-parent root) and exact
//!   non-heads refs (`refs/x`, `refs/heads/HEAD`, `refs/heads/-name`) all travel
//!   through that one exact-ref planner/transaction. A remaining
//!   [`crate::op_target::OpTargetVerdict::SourceAllowedButNativeCannotPublish`] is a genuine
//!   native inability (an unparsable exact ref or a real plan failure), never a
//!   fabricated parent/ref/plan. The source's own tag-disambiguation mismatch
//!   (`heads/topic` vs `topic`) and a branch literally `HEAD` stay Source
//!   step-4 refusals.
//! * Cross-checkout CLAIM composition is not implemented; the started-record
//!   read is local to this writer's record root, like `_read_started_event`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sk_git::{
    BranchName, CommitGuardCapability, CommitMessage, GitError, GitOptions, GitPath, GitRepository,
    ProtectedBranches, RefName, SafeCommitPhysicalOutcome, SafeCommitPhysicalRequest,
};
use sk_workspace::{ProtectionPolicy, WorkspaceError};
use thiserror::Error;

use crate::legacy_json::{self, LegacyObject, PythonString, PythonText};
use crate::op_commit_policy::{
    OpCommitProbeError, PrimaryTracking, op_commit_skip_reason, probe_primary_tracking,
};
use crate::op_custody::is_worktree_of;
use crate::op_target::{JudgedDestination, OpTargetVerdict, native_nonpublishable_reason};
use crate::python_os_encoding::{OsEncodeError, OsEncodingContext};
use crate::record::InvocationId;
use crate::writer::{EVENTS_DIR, InvocationWriter, OP_CLOSURES_FILENAME};

/// The source's 30-second bound for the work-tree/commit probe.
pub const OP_GIT_PROBE_TIMEOUT: Duration = Duration::from_secs(30);

/// The ULID-shaped stand-in record the admission preflight judges.
///
/// It is the same path-form as the record the command is about to write, so
/// ignore/path/protection decisions are judged exactly like the real commit.
pub const ADMISSION_PROBE_RECORD: &str = "kitty-ops/00000000000000000000000000.jsonl";

/// The reference's literal outcome when the record root is not a work tree.
const NOT_A_WORKTREE: &str = "not a git worktree";

/// The best-effort outcome of one Op-record auto-commit (#4397 / #6410).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpCommitOutcome {
    committed: bool,
    refusal: Option<String>,
    skipped: Option<String>,
    evidence: Option<OpCommitEvidence>,
}

impl OpCommitOutcome {
    /// A commit the backend confirmed landed.
    pub(crate) fn new_committed(evidence: OpCommitEvidence) -> Self {
        Self {
            committed: true,
            refusal: None,
            skipped: None,
            evidence: Some(evidence),
        }
    }

    /// A best-effort failure; the audited rows stay on disk.
    pub(crate) fn new_refused(reason: impl Into<String>) -> Self {
        Self {
            committed: false,
            refusal: Some(reason.into()),
            skipped: None,
            evidence: None,
        }
    }

    /// A deliberately unattempted commit (ignored path, no primary Op records,
    /// or an undecidable probe).
    pub(crate) fn new_skipped(reason: impl Into<String>) -> Self {
        Self {
            committed: false,
            refusal: None,
            skipped: Some(reason.into()),
            evidence: None,
        }
    }

    /// Whether the backend confirmed a landed commit.
    pub fn committed(&self) -> bool {
        self.committed
    }

    /// Why the commit was refused or failed, when it did.
    pub fn refusal(&self) -> Option<&str> {
        self.refusal.as_deref()
    }

    /// Why the commit was deliberately not attempted.
    pub fn skipped(&self) -> Option<&str> {
        self.skipped.as_deref()
    }

    /// Typed backend evidence, when the backend produced any.
    pub fn evidence(&self) -> Option<&OpCommitEvidence> {
        self.evidence.as_ref()
    }
}

/// The typed evidence a successful or partially successful native commit leaves.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OpCommitEvidence {
    /// `safe_commit` returned a confirmed outcome on a physical branch that a
    /// short [`BranchName`] can represent exactly.
    Committed {
        /// The landed commit id.
        commit: sk_git::Oid,
        /// The branch the commit landed on.
        destination: BranchName,
        /// The paths the commit carries, read back from the commit.
        committed_paths: Vec<GitPath>,
        /// Non-fatal hook/diagnostic warnings.
        warnings: Vec<String>,
    },
    /// A confirmed commit landed on an exact physical ref that no short
    /// [`BranchName`] can express (`refs/x`, `refs/heads/HEAD`,
    /// `refs/heads/-name`).
    ///
    /// The destination is the real fully-qualified [`RefName`](sk_git::RefName)
    /// the transaction published and compare-and-swapped; it is never a
    /// sanitised label or a fabricated short branch.
    CommittedPhysical {
        /// The landed commit id.
        commit: sk_git::Oid,
        /// The exact fully-qualified ref the commit landed on.
        destination: sk_git::RefName,
        /// Whether the planner observed the destination genuinely absent, so
        /// this landed commit is a zero-parent root published by an
        /// absent-ref create. `false` means it pinned a real destination tip.
        was_unborn: bool,
        /// The paths the commit carries, read back from the commit.
        committed_paths: Vec<GitPath>,
        /// Non-fatal hook/diagnostic warnings.
        warnings: Vec<String>,
    },
    /// A candidate exists, but its publication on the destination is unconfirmed.
    /// This is neither a confirmed commit nor proof that nothing landed.
    AdvanceUnconfirmed {
        /// Immutable candidate created by this attempt.
        candidate: sk_git::Oid,
        /// The exact physical destination ref (`refs/...`) the transaction
        /// pinned, as the backend reported it. This is not inferred or
        /// shortened from the source-judged name; for a born ordinary branch it
        /// reads `refs/heads/<branch>`.
        destination: String,
        /// Destination tip observed after the failure, if readable.
        observed_tip: Option<sk_git::Oid>,
        /// Backend uncertainty and readback diagnostics.
        detail: String,
    },
    /// The commit landed, but a post-landing index/readback step failed.
    ///
    /// The commit object exists and is reachable; nothing was rewound or
    /// deleted. The paths name the index state that needs repair.
    SafeCommitLanded {
        /// The landed commit id.
        commit: sk_git::Oid,
        /// The paths whose real-index state needs repair.
        affected_paths: Vec<GitPath>,
    },
}

impl OpCommitEvidence {
    /// The exact fully-qualified physical ref a confirmed commit landed on.
    ///
    /// This is additive and typed: the legacy [`Self::Committed`] branch
    /// remains a [`BranchName`] and is widened to its real
    /// `refs/heads/...` ref, never a lossy string. Only the successful committed
    /// variants store a destination here. `None` on landed-repair/unconfirmed
    /// evidence does NOT mean nothing landed; inspect its typed commit/candidate.
    pub fn committed_ref(&self) -> Option<sk_git::RefName> {
        match self {
            Self::Committed { destination, .. } => Some(destination.as_ref_name()),
            Self::CommittedPhysical { destination, .. } => Some(destination.clone()),
            Self::AdvanceUnconfirmed { .. } | Self::SafeCommitLanded { .. } => None,
        }
    }

    /// The commit id of either successful committed-evidence variant.
    ///
    /// Landed-repair and unconfirmed variants retain their own typed IDs; this
    /// success accessor returns `None` for them, not proof that nothing landed.
    pub fn committed_commit(&self) -> Option<&sk_git::Oid> {
        match self {
            Self::Committed { commit, .. } | Self::CommittedPhysical { commit, .. } => Some(commit),
            Self::AdvanceUnconfirmed { .. } | Self::SafeCommitLanded { .. } => None,
        }
    }

    /// The observed destination state, when this evidence recorded it.
    ///
    /// The legacy [`Self::Committed`] variant predates the exact-ref family and
    /// does not carry that fact (`None`); it is not inferred.
    pub fn was_unborn(&self) -> Option<bool> {
        match self {
            Self::CommittedPhysical { was_unborn, .. } => Some(*was_unborn),
            Self::Committed { .. }
            | Self::AdvanceUnconfirmed { .. }
            | Self::SafeCommitLanded { .. } => None,
        }
    }
}

/// Which close path is committing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpClosedBy {
    /// An agent close: commit only the per-record file.
    Agent,
    /// A doctor sweep: commit only the append-only closure spine.
    DoctorSweep,
}

/// How the service resolves the operator protection hatch.
#[derive(Clone, Debug, Default)]
enum HatchOverride {
    /// Read `SPEC_KITTY_ALLOW_PROTECTED_BRANCH_COMMITS` at each resolution.
    #[default]
    Environment,
    /// Use an explicit value (`None` disables the hatch) without touching the
    /// process environment. Test/composition seam only.
    Fixed(Option<String>),
}

/// The opened repository, or the honest reason it could not be opened.
#[derive(Debug)]
pub(crate) enum RepositoryBinding {
    /// A usable work-tree handle with its captured operator-index authority.
    Open(Box<GitRepository>),
    /// Not a work tree (plain directory, bare repository, unreadable HEAD).
    NotAWorkTree,
    /// The probe itself could not run (missing git, spawn/I/O/timeout).
    ProbeFailure(String),
}

/// A failure while resolving the project/record protection inputs.
#[derive(Debug)]
pub(crate) enum ProtectionRead {
    /// The configuration is unreadable/malformed (source
    /// `ProtectionConfigError`).
    InvalidConfig(String),
    /// A non-configuration protection read failed.
    Unavailable(WorkspaceError),
}

/// The source-started record could not be read as the source helper reads it.
#[derive(Debug, Error)]
pub(crate) enum StartedReadError {
    /// Missing/empty/non-object/undecodable first row.
    #[error("Invalid invocation record: {invocation_id}")]
    Invalid {
        /// The requested invocation id.
        invocation_id: String,
    },
    /// The file exists but is not UTF-8 text (a generic read failure in source).
    #[error("Could not read invocation record: {invocation_id}: {detail}")]
    Unreadable {
        /// The requested invocation id.
        invocation_id: String,
        /// Why the bytes were not readable as text.
        detail: String,
    },
}

/// The one message-free exact-physical request builder shared by admission and
/// the real commit.
///
/// The destination is the exact full `RefName` the Source's symbolic `HEAD`
/// names, not a short branch. `ProtectedBranches` is deliberately empty: the
/// source's protection verdict was already decided on the source-judged
/// destination `J`, and `NativeProtectedBranches` cannot express names such as
/// `refs/x`, `refs/heads/HEAD` or `refs/heads/-name` at all. Passing the empty
/// projection preserves the source's actual-only-protection quirk instead of
/// adding a speculative P refusal; the one canonical STANDARD guard still runs
/// in the shared planner. No payload is carried, so no placeholder message
/// reaches the backend.
pub(crate) fn build_safe_physical_commit_target(
    destination: &RefName,
    path: &GitPath,
) -> SafeCommitPhysicalRequest {
    SafeCommitPhysicalRequest::new(destination.clone(), [path.clone()]).with_protection(
        ProtectedBranches::default(),
        CommitGuardCapability::Standard,
    )
}

/// The source's default 30-second Git probe budget.
pub fn op_git_options() -> GitOptions {
    GitOptions {
        timeout: OP_GIT_PROBE_TIMEOUT,
        ..GitOptions::default()
    }
}

/// Repository-bound Op commit and admission state.
///
/// The value owns its `InvocationWriter` and `GitRepository` (so the captured
/// operator index, git program, and timeout are sticky), plus the one
/// primary-tracking cache the source keeps for an executor's lifetime. It
/// holds no self-reference. Every branch and protection decision is re-read on
/// each call; only the primary-tracking answer is cached.
#[derive(Debug)]
pub struct OpCommitService {
    project_root: PathBuf,
    writer: InvocationWriter,
    pub(crate) binding: RepositoryBinding,
    os_encoding: OsEncodingContext,
    primary_tracking: Option<PrimaryTracking>,
    last_op_commit: Option<OpCommitOutcome>,
    hatch: HatchOverride,
}

impl OpCommitService {
    /// Bind to `project_root` and the writer's record root with the source's
    /// 30-second probe budget and the process operator hatch.
    pub fn new(project_root: impl Into<PathBuf>, writer: InvocationWriter) -> Self {
        Self::with_git_options(project_root, writer, &op_git_options())
    }

    /// Bind with explicit Git options (a test/composition seam).
    ///
    /// The selected options apply to repository opening and every later Git
    /// probe made through this handle. A non-work-tree root is represented as
    /// the internal `RepositoryBinding::NotAWorkTree` state, not a construction failure.
    pub fn with_git_options(
        project_root: impl Into<PathBuf>,
        writer: InvocationWriter,
        options: &GitOptions,
    ) -> Self {
        let binding = match GitRepository::open_with_options(writer.record_root(), options) {
            Ok(repository) => RepositoryBinding::Open(Box::new(repository)),
            Err(GitError::NotARepository { .. })
            | Err(GitError::BareRepository { .. })
            | Err(GitError::UnreadableHead { .. }) => RepositoryBinding::NotAWorkTree,
            Err(error) => RepositoryBinding::ProbeFailure(error.to_string()),
        };
        Self {
            project_root: project_root.into(),
            writer,
            binding,
            // Captured once, like the reference interpreter's startup decision;
            // only the close path reads it, after the target preflight.
            os_encoding: OsEncodingContext::from_environment(),
            primary_tracking: None,
            last_op_commit: None,
            hatch: HatchOverride::Environment,
        }
    }

    /// Use an explicit operator-hatch value instead of the process environment.
    ///
    /// `Some("1" | "true" | "yes")` enables the source's operator hatch;
    /// `None` disables it. This never mutates the process environment.
    pub fn with_hatch(mut self, hatch: Option<&str>) -> Self {
        self.hatch = HatchOverride::Fixed(hatch.map(str::to_owned));
        self
    }

    /// Restore process-environment hatch resolution.
    pub fn with_environment_hatch(mut self) -> Self {
        self.hatch = HatchOverride::Environment;
        self
    }

    /// The project root whose protection policy participates in the union.
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    /// The checkout whose `kitty-ops/` receives and commits the record.
    pub fn record_root(&self) -> &Path {
        self.writer.record_root()
    }

    /// The writer this service was bound to.
    pub fn writer(&self) -> &InvocationWriter {
        &self.writer
    }

    /// The primary-tracking cache, when a probe has already succeeded.
    pub fn primary_tracking(&self) -> Option<&PrimaryTracking> {
        self.primary_tracking.as_ref()
    }

    /// The most recent commit outcome, mirroring the executor's
    /// `last_op_commit` visibility. `None` before the first close.
    pub fn last_op_commit(&self) -> Option<&OpCommitOutcome> {
        self.last_op_commit.as_ref()
    }

    pub(crate) fn open_repository(&self) -> Option<&GitRepository> {
        match &self.binding {
            RepositoryBinding::Open(repository) => Some(repository.as_ref()),
            RepositoryBinding::NotAWorkTree | RepositoryBinding::ProbeFailure(_) => None,
        }
    }

    pub(crate) fn probe_failure(&self) -> Option<&str> {
        match &self.binding {
            RepositoryBinding::ProbeFailure(reason) => Some(reason),
            _ => None,
        }
    }

    /// Whether the record root is a worktree of the project root.
    ///
    /// This is the source preflight's step 3. It is a cheap read-only check for
    /// the normal same-root/linked-worktree cases and an honest `false` for a
    /// mismatched root.
    pub(crate) fn roots_are_linked(&self) -> bool {
        is_worktree_of(&self.project_root, self.record_root())
    }

    /// The source skip decision with the executor-lifetime primary cache.
    ///
    /// A probe failure becomes the source's best-effort skip reason; it is
    /// never a commit, a success, or a write refusal. A failed tracking probe
    /// is retried on the next call; a successful tracking answer survives a
    /// later ignore-probe failure.
    pub(crate) fn skip_reason(&mut self, path: &GitPath) -> Option<String> {
        let repository = match &self.binding {
            RepositoryBinding::Open(repository) => repository,
            RepositoryBinding::NotAWorkTree | RepositoryBinding::ProbeFailure(_) => return None,
        };
        let tracking = match &self.primary_tracking {
            Some(cached) => cached.clone(),
            None => match probe_primary_tracking(repository) {
                Ok(fresh) => {
                    self.primary_tracking = Some(fresh.clone());
                    fresh
                }
                Err(error) => return Some(probe_failed(&error)),
            },
        };
        match op_commit_skip_reason(repository, path, &tracking) {
            Ok(reason) => reason,
            Err(error) => Some(probe_failed(&error)),
        }
    }

    /// The source's protection verdict, decided on the **judged** name `J`.
    ///
    /// The project-root and record-checkout policies are both resolved and
    /// OR-ed exactly like `preflight_commit` step 6c. The verdict is a boolean
    /// fact: the Op never forces `J` through `BranchName` or
    /// `NativeProtectedBranches`, so a judged name such as `refs/x` is still
    /// judged by the reference's own string rule.
    pub(crate) fn judged_protection(
        &self,
        judged: &JudgedDestination,
    ) -> Result<bool, ProtectionRead> {
        let project = self.resolve_policy(&self.project_root)?;
        let record = self.resolve_policy(self.record_root())?;
        Ok(project.is_protected(judged.as_str()) || record.is_protected(judged.as_str()))
    }

    fn resolve_policy(&self, root: &Path) -> Result<ProtectionPolicy, ProtectionRead> {
        let policy = match &self.hatch {
            HatchOverride::Environment => ProtectionPolicy::resolve(root),
            HatchOverride::Fixed(value) => {
                ProtectionPolicy::resolve_with_hatch(root, value.as_deref())
            }
        };
        policy.map_err(|error| match error {
            WorkspaceError::ProtectionConfig { detail, .. } => {
                // The Op boundary reports the canonical policy's source-facing
                // detail, not the workspace adapter's extra path/type wrapper.
                ProtectionRead::InvalidConfig(detail)
            }
            other => ProtectionRead::Unavailable(other),
        })
    }

    /// The source `_commit_op_record` composition.
    ///
    /// The audit row is assumed already durably written by the caller. This
    /// method never writes, rewinds, deletes, or truncates audit data; it reads
    /// the first started object, decides skip/branch/protection exactly like
    /// the reference, and asks the real `safe_commit` for the one path.
    pub fn commit_op_record(
        &mut self,
        invocation_id: &InvocationId,
        closed_by: OpClosedBy,
    ) -> OpCommitOutcome {
        let outcome = self.commit_op_record_unrecorded(invocation_id, closed_by);
        self.last_op_commit = Some(outcome.clone());
        outcome
    }

    fn commit_op_record_unrecorded(
        &mut self,
        invocation_id: &InvocationId,
        closed_by: OpClosedBy,
    ) -> OpCommitOutcome {
        let started = match read_started_object(&self.writer, invocation_id) {
            Ok(started) => started,
            Err(error) => return OpCommitOutcome::new_refused(error.to_string()),
        };
        let profile = started_field(&started, "profile_id");
        let action = started_field(&started, "action");
        let (relative_path, message) =
            match self.commit_target(invocation_id, closed_by, &profile, &action) {
                Ok(target) => target,
                Err(reason) => return OpCommitOutcome::new_refused(reason),
            };

        // Source `_current_branch` runs before the skip probe; a binding that
        // is not a work tree reports the source's literal outcome, while a
        // detached work tree is decided after the skip below.
        let judged = match &self.binding {
            RepositoryBinding::Open(_) => match self.judged_destination() {
                Ok(judged) => judged,
                Err(error) => return OpCommitOutcome::new_refused(error.to_string()),
            },
            RepositoryBinding::NotAWorkTree => {
                return OpCommitOutcome::new_refused(NOT_A_WORKTREE);
            }
            RepositoryBinding::ProbeFailure(reason) => {
                return OpCommitOutcome::new_refused(reason.clone());
            }
        };

        let path = match git_path_from_relative(&relative_path) {
            Ok(path) => path,
            Err(error) => return OpCommitOutcome::new_refused(error.to_string()),
        };
        // #6410: skip before the detached refusal, so a detached checkout in a
        // repository that keeps no Op records skips quietly.
        if let Some(reason) = self.skip_reason(&path) {
            return OpCommitOutcome::new_skipped(reason);
        }
        let Some(judged) = judged else {
            return OpCommitOutcome::new_refused("detached HEAD");
        };

        // Source preflight order 1 → 3 → 4 → 5 → 6a/6b → 6c, then (only if the
        // source allows) the native publication plan. A source path refusal
        // still admits the audit write but refuses the actual commit; the two
        // verdicts are not conflated.
        match self.judge_record_target(&judged, &path) {
            OpTargetVerdict::SourceRefuses(error) => {
                OpCommitOutcome::new_refused(error.to_string())
            }
            OpTargetVerdict::SourcePathRefuses(error) => {
                OpCommitOutcome::new_refused(error.to_string())
            }
            OpTargetVerdict::Protected { judged } => OpCommitOutcome::new_refused(
                crate::op_custody::OpWriteRefusal::protected_branch(&judged).reason,
            ),
            OpTargetVerdict::InvalidConfig { detail } => OpCommitOutcome::new_refused(detail),
            OpTargetVerdict::Repository(error) => OpCommitOutcome::new_refused(error.to_string()),
            OpTargetVerdict::Protection(error) => OpCommitOutcome::new_refused(error.to_string()),
            OpTargetVerdict::SourceAllowedButNativeCannotPublish { judged, cause } => {
                OpCommitOutcome::new_refused(native_nonpublishable_reason(&judged, &cause))
            }
            OpTargetVerdict::Ready { target, plan } => {
                let plan = *plan;
                // Late OS encoding of the exact Python code points, then the
                // existing native payload guard through the only plan a byte
                // commit accepts. The plan's destination is the exact physical
                // ref `P`, never the judged name.
                let payload = match self.os_encoding.encode(message.codepoints()) {
                    Ok(payload) => payload,
                    Err(error) => {
                        return OpCommitOutcome::new_refused(message_encoding_refusal(
                            &error,
                            &self.os_encoding.label(),
                        ));
                    }
                };
                let message_plan = match plan.prepare(CommitMessage::from_bytes(payload)) {
                    Ok(plan) => plan,
                    Err(failure) => return commit_error_outcome(failure.into_error()),
                };
                let repository = self
                    .open_repository()
                    .expect("a resolved target names an open repository");
                commit_outcome_for(
                    repository.apply_safe_physical_commit(&message_plan),
                    target.judged().as_str(),
                )
            }
        }
    }

    /// The exact repository-relative path and commit message code points.
    ///
    /// The profile/action text is Python `str(value)`, which may still contain
    /// surrogate code units from a used top-level string: composition keeps
    /// those code points and never encodes per field. The caller encodes the
    /// whole message **after** the native target plan. This fixes measured
    /// guard-before-encoding cases, not source path/J-P reachability or its
    /// staging-before-encoding failure footprint.
    fn commit_target(
        &self,
        invocation_id: &InvocationId,
        closed_by: OpClosedBy,
        profile: &PythonText,
        action: &PythonText,
    ) -> Result<(PathBuf, PythonString), String> {
        let first_eight: String = invocation_id.as_str().chars().take(8).collect();
        match closed_by {
            OpClosedBy::DoctorSweep => Ok((
                Path::new(EVENTS_DIR).join(OP_CLOSURES_FILENAME),
                op_message_codepoints(profile, action, &first_eight, " (doctor sweep)"),
            )),
            OpClosedBy::Agent => {
                let path = self
                    .writer
                    .invocation_path(invocation_id)
                    .map_err(|error| error.to_string())?;
                let relative = path.strip_prefix(self.record_root()).map_err(|_| {
                    format!("Invalid invocation record: {}", invocation_id.as_str())
                })?;
                Ok((
                    relative.to_path_buf(),
                    op_message_codepoints(profile, action, &first_eight, ""),
                ))
            }
        }
    }
}

/// Map one physical backend outcome to the source-visible Op outcome.
///
/// There is one publisher/one outcome adapter. A representable `refs/heads/<P>`
/// keeps the legacy [`OpCommitEvidence::Committed`] shape (short
/// [`BranchName`]) so existing consumers do not silently lose the field they
/// read; an exact ref no short name can express becomes
/// [`OpCommitEvidence::CommittedPhysical`] with the real full `RefName` and the
/// planner-observed root fact. Neither branch fabricates or sanitises a name.
/// A protected refusal names the source-judged destination `J`, not the
/// physical `P` the request carried.
fn commit_outcome_for(
    result: Result<SafeCommitPhysicalOutcome, GitError>,
    judged: &str,
) -> OpCommitOutcome {
    match result {
        Ok(outcome) => {
            let evidence = match outcome.destination_ref().short_branch() {
                Some(destination) => OpCommitEvidence::Committed {
                    commit: outcome.commit().clone(),
                    destination,
                    committed_paths: outcome.committed_paths().to_vec(),
                    warnings: outcome.warnings().to_vec(),
                },
                None => OpCommitEvidence::CommittedPhysical {
                    commit: outcome.commit().clone(),
                    destination: outcome.destination_ref().clone(),
                    was_unborn: outcome.was_unborn(),
                    committed_paths: outcome.committed_paths().to_vec(),
                    warnings: outcome.warnings().to_vec(),
                },
            };
            OpCommitOutcome::new_committed(evidence)
        }
        Err(GitError::SafeCommitProtectedBranch { .. }) => OpCommitOutcome::new_refused(
            crate::op_custody::OpWriteRefusal::protected_branch(judged).reason,
        ),
        Err(error) => commit_error_outcome(error),
    }
}

/// Map one backend error to the source-visible Op outcome.
fn commit_error_outcome(error: GitError) -> OpCommitOutcome {
    match error {
        GitError::SafeCommitProtectedBranch { destination, .. } => OpCommitOutcome::new_refused(
            crate::op_custody::OpWriteRefusal::protected_branch(&destination).reason,
        ),
        error => {
            let evidence = match &error {
                GitError::SafeCommitLanded { recovery, .. } => {
                    Some(OpCommitEvidence::SafeCommitLanded {
                        commit: recovery.commit().clone(),
                        affected_paths: recovery.affected_paths().to_vec(),
                    })
                }
                GitError::SafeCommitAdvanceUnconfirmed {
                    candidate,
                    destination,
                    observed_tip,
                    detail,
                } => Some(OpCommitEvidence::AdvanceUnconfirmed {
                    candidate: candidate.clone(),
                    destination: destination.clone(),
                    observed_tip: observed_tip.clone(),
                    detail: detail.clone(),
                }),
                _ => None,
            };
            let mut outcome = OpCommitOutcome::new_refused(error.to_string());
            outcome.evidence = evidence;
            outcome
        }
    }
}

fn probe_failed(error: &OpCommitProbeError) -> String {
    format!("git probe failed while deciding whether to commit the Op record: {error}")
}

/// `op(<profile>): <action> [<short id>]<suffix>` as Python `str` composition.
///
/// The whole message stays a sequence of Python code points, exactly like the
/// reference f-string: a verbatim field contributes its code units (including a
/// lone surrogate), a rendered field contributes its canonical `repr`, and no
/// field is encoded on its own. Encoding is the caller's one late step.
fn op_message_codepoints(
    profile: &PythonText,
    action: &PythonText,
    first_eight: &str,
    suffix: &str,
) -> PythonString {
    let mut message = PythonString::from_scalars("op(");
    profile.append_to(&mut message);
    message.push_scalars("): ");
    action.append_to(&mut message);
    message.push_scalars(" [");
    message.push_scalars(first_eight);
    message.push_scalars("]");
    message.push_scalars(suffix);
    message
}

/// A used string the captured filesystem encoding cannot represent.
///
/// The reference builds the same Python `str` and fails at its own argv
/// encoding boundary with `UnicodeEncodeError`. The native answer is the same
/// decision with a truthful native wording: it names the measured context and
/// the first failing code point. It is never `Invalid invocation record` (the
/// row is valid), never an invented Python exception, and never a replacement
/// character; a source-valid context this adapter does not implement is
/// reported as that limitation instead of a fabricated refusal.
fn message_encoding_refusal(error: &OsEncodeError, context: &str) -> String {
    match error {
        OsEncodeError::Unencodable { code, position } => format!(
            "Op commit message is not encodable by the source's filesystem encoding ({context}): code point U+{code:04X} at index {position}"
        ),
        OsEncodeError::UnsupportedContext { detail } => format!(
            "Op commit message cannot be encoded: this native adapter does not implement the source-valid filesystem-encoding context ({detail})"
        ),
    }
}

/// Decompose a repository-relative path into the exact `GitPath` bytes.
pub(crate) fn git_path_from_relative(path: &Path) -> Result<GitPath, GitError> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        GitPath::from_git_bytes(path.as_os_str().as_bytes())
    }
    #[cfg(not(unix))]
    {
        GitPath::from_git_bytes(path.to_string_lossy().as_bytes())
    }
}

/// Read the first started JSON object exactly as `executor._read_started_event`.
///
/// Missing/empty/non-object/undecodable rows are `Invalid invocation record`.
/// The first line follows Python's `str.splitlines()` boundaries, not only
/// `\n`/`\r\n`.
pub(crate) fn read_started_object(
    writer: &InvocationWriter,
    invocation_id: &InvocationId,
) -> Result<LegacyObject, StartedReadError> {
    let invalid = || StartedReadError::Invalid {
        invocation_id: invocation_id.as_str().to_owned(),
    };
    let path = writer
        .invocation_path(invocation_id)
        .map_err(|_| invalid())?;
    let bytes = fs::read(path).map_err(|_| invalid())?;
    let text = String::from_utf8(bytes).map_err(|error| StartedReadError::Unreadable {
        invocation_id: invocation_id.as_str().to_owned(),
        detail: error.to_string(),
    })?;
    let first = python_first_line(&text).ok_or_else(invalid)?;
    legacy_json::parse_object(first).ok_or_else(invalid)
}

/// The first line as Python's `str.splitlines()` defines a boundary.
fn python_first_line(text: &str) -> Option<&str> {
    if text.is_empty() {
        return None;
    }
    for (index, character) in text.char_indices() {
        if matches!(
            character,
            '\n' | '\r'
                | '\u{0b}'
                | '\u{0c}'
                | '\u{1c}'
                | '\u{1d}'
                | '\u{1e}'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        ) {
            return Some(&text[..index]);
        }
    }
    Some(text)
}

/// `str(started.get(key) or "unknown")`.
///
/// Absent/null/empty/false/zero/empty collections are `unknown`; strings are
/// verbatim Python code points (a lone surrogate is preserved, not rejected, and
/// is encoded only by [`OpCommitService::commit_op_record`]'s late filesystem
/// encoder); integers stay exact; floats use CPython `repr` including `nan`,
/// `inf` and `-inf`; collections use the shared Python `repr` formatter in the
/// order `json.loads` produced (Python dict insertion order, last duplicate
/// value in the first position). Nonfinite and ordered maps come from the
/// private [`crate::legacy_json`] tree, not `serde_json::Value`.
pub(crate) fn started_field(started: &LegacyObject, key: &str) -> PythonText {
    match started.get(key) {
        Some(value) if !legacy_json::is_falsy(value) => legacy_json::python_str(value),
        _ => PythonText::Rendered("unknown".to_owned()),
    }
}
