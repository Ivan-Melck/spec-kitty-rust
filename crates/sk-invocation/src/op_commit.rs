//! Best-effort Op-record / closure-spine commits, and the repository-bound
//! state shared with write admission.
//!
//! Source: `specify_cli/invocation/executor.py` (`_read_started_event`,
//! `_commit_op_record`, `_op_commit_kwargs`, `_op_commit_skip_reason`). This is
//! the REAL commit composition over the accepted native skip probes, the
//! canonical `ProtectionPolicy` inputs, and `GitRepository::safe_commit`; it is
//! not an executor, dispatcher, or CLI adapter.
//!
//! # What is committed
//!
//! An agent close commits **only** its per-record file
//! (`kitty-ops/<ULID>.jsonl`). A doctor-sweep close commits **only** the
//! append-only closure spine (`kitty-ops/op-closures.jsonl`); a pre-existing
//! archive record is never modified, staged, or committed. The path is passed
//! to `safe_commit`, so unrelated staged content, hooks, signing configuration,
//! and the operator's captured index keep their accepted `sk-git` behavior.
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
//! * Cross-checkout CLAIM composition is not implemented; the started-record
//!   read is local to this writer's record root, like `_read_started_event`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Map, Value};
use sk_git::{
    BranchName, CommitGuardCapability, GitError, GitOptions, GitPath, GitRepository,
    ProtectedBranches, SafeCommitRequest,
};
use sk_workspace::{ProtectionPolicy, WorkspaceError};
use thiserror::Error;

use crate::op_commit_policy::{
    OpCommitProbeError, PrimaryTracking, op_commit_skip_reason, probe_primary_tracking,
    strip_git_stdout,
};
use crate::op_custody::is_worktree_of;
use crate::python_json::python_repr_json;
use crate::record::InvocationId;
use crate::writer::{EVENTS_DIR, InvocationWriter, OP_CLOSURES_FILENAME};

/// The source's 30-second bound for the work-tree/commit probe.
pub const OP_GIT_PROBE_TIMEOUT: Duration = Duration::from_secs(30);

/// The ULID-shaped stand-in record the admission preflight judges.
///
/// It is the same path-form as the record the command is about to write, so
/// ignore/path/protection decisions are judged exactly like the real commit.
pub const ADMISSION_PROBE_RECORD: &str = "kitty-ops/00000000000000000000000000.jsonl";

/// The source's admission-probe commit message.
pub(crate) const ADMISSION_PROBE_MESSAGE: &str = "op(admission probe)";

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
    /// `safe_commit` returned a confirmed outcome.
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
    /// A candidate exists, but its publication on the destination is unconfirmed.
    /// This is neither a confirmed commit nor proof that nothing landed.
    AdvanceUnconfirmed {
        /// Immutable candidate created by this attempt.
        candidate: sk_git::Oid,
        /// Requested destination branch.
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
    /// The current branch could not be represented as a `BranchName` for the
    /// Git guard input.
    BranchName(GitError),
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

/// The one request builder shared by admission and the real commit.
///
/// `protected` is the **project-root OR record-checkout** decision projected
/// onto the destination branch. Passing the destination only when the union
/// protects it keeps the guard's canonical decision exact without re-parsing
/// irrelevant protected names (an unrelated malformed name must not turn into
/// a new configuration refusal).
pub(crate) fn build_safe_commit_request(
    destination: &BranchName,
    message: &str,
    path: &GitPath,
    protected: ProtectedBranches,
) -> SafeCommitRequest {
    SafeCommitRequest::new(destination.clone(), message, [path.clone()])
        .with_protection(protected, CommitGuardCapability::Standard)
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
    /// [`RepositoryBinding::NotAWorkTree`], not a construction failure.
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

    /// Source `_current_branch` strips symbolic stdout; the generic Git API
    /// deliberately retains legal whitespace in the ref name.
    pub(crate) fn current_branch(&self) -> Result<Option<String>, GitError> {
        let Some(repository) = self.open_repository() else {
            return Ok(None);
        };
        repository.current_branch().map(|branch| {
            branch.and_then(|branch| {
                let branch = strip_git_stdout(&branch);
                (!branch.is_empty()).then(|| branch.to_owned())
            })
        })
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

    /// Whether the current branch has a commit (source preflight step 5).
    ///
    /// `false` also covers a failed destination probe: the source's
    /// `_destination_ref_exists` treats it as absent, so admission admits it
    /// rather than inventing a refusal.
    pub(crate) fn has_destination_commit(&self) -> bool {
        self.open_repository()
            .and_then(|repository| repository.head_commit().ok().flatten())
            .is_some()
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

    /// Resolve the union of the project-root and record-checkout policies.
    pub(crate) fn resolve_protected(
        &self,
        branch: &str,
    ) -> Result<ProtectedBranches, ProtectionRead> {
        let project = self.resolve_policy(&self.project_root)?;
        let record = self.resolve_policy(self.record_root())?;
        if !(project.is_protected(branch) || record.is_protected(branch)) {
            return Ok(ProtectedBranches::default());
        }
        let destination = BranchName::parse(branch).map_err(ProtectionRead::BranchName)?;
        Ok(ProtectedBranches::new([destination]))
    }

    fn resolve_policy(&self, root: &Path) -> Result<ProtectionPolicy, ProtectionRead> {
        let policy = match &self.hatch {
            HatchOverride::Environment => ProtectionPolicy::resolve(root),
            HatchOverride::Fixed(value) => {
                ProtectionPolicy::resolve_with_hatch(root, value.as_deref())
            }
        };
        policy.map_err(|error| match error {
            WorkspaceError::ProtectionConfig { .. } => {
                ProtectionRead::InvalidConfig(error.to_string())
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

        let current_branch = match &self.binding {
            RepositoryBinding::Open(_) => match self.current_branch() {
                Ok(branch) => branch,
                Err(error) => return OpCommitOutcome::new_refused(error.to_string()),
            },
            // The source checks these before the skip decision: a non-worktree
            // root is reported directly and a detached HEAD is decided after
            // the skip probe below.
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
        let Some(branch) = current_branch else {
            return OpCommitOutcome::new_refused("detached HEAD");
        };
        if !self.roots_are_linked() {
            return OpCommitOutcome::new_refused(format!(
                "safe_commit: {} is not a git worktree. Pass a resolved worktree path.",
                self.record_root().display()
            ));
        }

        let protected = match self.resolve_protected(&branch) {
            Ok(protected) => protected,
            Err(ProtectionRead::InvalidConfig(reason)) => {
                return OpCommitOutcome::new_refused(reason);
            }
            Err(ProtectionRead::Unavailable(error)) => {
                return OpCommitOutcome::new_refused(error.to_string());
            }
            Err(ProtectionRead::BranchName(error)) => {
                return OpCommitOutcome::new_refused(error.to_string());
            }
        };
        let destination = match BranchName::parse(&branch) {
            Ok(destination) => destination,
            Err(error) => return OpCommitOutcome::new_refused(error.to_string()),
        };
        let request = build_safe_commit_request(&destination, &message, &path, protected);
        let repository = self
            .open_repository()
            .expect("a branch was read from an open repository");
        match repository.safe_commit(&request) {
            Ok(outcome) => OpCommitOutcome::new_committed(OpCommitEvidence::Committed {
                commit: outcome.commit().clone(),
                destination: outcome.destination().clone(),
                committed_paths: outcome.committed_paths().to_vec(),
                warnings: outcome.warnings().to_vec(),
            }),
            Err(GitError::SafeCommitProtectedBranch { destination, .. }) => {
                OpCommitOutcome::new_refused(
                    crate::op_custody::OpWriteRefusal::protected_branch(&destination).reason,
                )
            }
            Err(error) => {
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

    /// The exact repository-relative path and commit message for this close.
    fn commit_target(
        &self,
        invocation_id: &InvocationId,
        closed_by: OpClosedBy,
        profile: &str,
        action: &str,
    ) -> Result<(PathBuf, String), String> {
        let first_eight: String = invocation_id.as_str().chars().take(8).collect();
        match closed_by {
            OpClosedBy::DoctorSweep => Ok((
                Path::new(EVENTS_DIR).join(OP_CLOSURES_FILENAME),
                format!("op({profile}): {action} [{first_eight}] (doctor sweep)"),
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
                    format!("op({profile}): {action} [{first_eight}]"),
                ))
            }
        }
    }
}

fn probe_failed(error: &OpCommitProbeError) -> String {
    format!("git probe failed while deciding whether to commit the Op record: {error}")
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
) -> Result<Map<String, Value>, StartedReadError> {
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
    let value: Value = serde_json::from_str(first).map_err(|_| invalid())?;
    value.as_object().cloned().ok_or_else(invalid)
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
/// verbatim; numbers and booleans use Python text. Collections use the existing
/// Python-repr formatter, but native JSON maps sort keys (not Python insertion
/// order); nonfinite JSON tokens are not accepted. These remain legacy-input
/// differences, not full `str()` equivalence for arbitrary raw objects.
pub(crate) fn started_field(started: &Map<String, Value>, key: &str) -> String {
    match started.get(key) {
        None | Some(Value::Null) => "unknown".to_owned(),
        Some(Value::String(text)) if text.is_empty() => "unknown".to_owned(),
        Some(Value::Bool(false)) => "unknown".to_owned(),
        Some(Value::Number(number)) if number.as_f64() == Some(0.0) => "unknown".to_owned(),
        Some(Value::Array(items)) if items.is_empty() => "unknown".to_owned(),
        Some(Value::Object(entries)) if entries.is_empty() => "unknown".to_owned(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => python_repr_json(other),
    }
}
