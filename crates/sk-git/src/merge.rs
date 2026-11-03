//! Concluding an in-progress merge, revert, or cherry-pick.
//!
//! A conclusion commits the whole index, so it cannot be path-scoped. This
//! module therefore runs exactly one argv — `git commit --no-edit` or
//! `git commit -m <message>` with `commit.gpgsign=false` — and refuses
//! everything that would change its scope or skip a hook:
//! `--no-verify`, `-a`, a pathspec, `--allow-empty`, and `--amend`.
//!
//! The captured operator index, the checkout, its branch, the operation, its
//! recorded tip, HEAD, and the message are pinned in a plan. Execution
//! re-detects the operation and re-reads the checkout, branch, HEAD and the
//! operation tip, refusing if any moved.
//!
//! **A parent-set match is necessary, not proof of creation.** Hooks are
//! legitimate and still run, so a `post-commit` hook can move the branch (even
//! to a pre-existing commit with the same parents) or create further commits.
//! The created commit is therefore identified only from evidence bound to this
//! invocation: git's own commit summary, or a reflog entry appended to the
//! *pinned* ref after a publication watermark captured before the command ran.
//! The observed `HEAD` is reported but is never accepted as a candidate merely
//! because its parents match. When that evidence does not confirm a clean
//! landing the result is [`GitError::MergeConclusionUnconfirmed`], which carries
//! the provable created commit (if any), the observed `HEAD`, the operation-
//! marker state, and never rolls back or deletes a possibly landed commit.
//!
//! A rejected `pre-commit`/`commit-msg` hook (a completed command with a failure
//! status) is a plain command failure only when the state proves nothing landed:
//! `HEAD` is still the pinned commit and the operation marker still records the
//! pinned tip. A command that started and then failed without a status (timeout,
//! capture bound, readback I/O) may already have landed, so it is reported as
//! unconfirmed evidence instead of an ordinary retryable error.

use std::path::{Path, PathBuf};

use crate::error::GitError;
use crate::index_authority::OperatorIndex;
use crate::oid::Oid;
use crate::process::{CommandOutput, RunAttempt};
use crate::refname::BranchName;
use crate::repo_state::InProgressOperation;
use crate::repository::GitRepository;

/// The operation a conclusion will commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConcludedOperation {
    /// A merge (`MERGE_HEAD`).
    Merge,
    /// A cherry-pick (`CHERRY_PICK_HEAD`).
    CherryPick,
    /// A revert (`REVERT_HEAD`).
    Revert,
}

impl ConcludedOperation {
    /// A stable machine-readable name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::CherryPick => "cherry-pick",
            Self::Revert => "revert",
        }
    }

    fn from_in_progress(operation: InProgressOperation) -> Option<Self> {
        match operation {
            InProgressOperation::Merge => Some(Self::Merge),
            InProgressOperation::CherryPick => Some(Self::CherryPick),
            InProgressOperation::Revert => Some(Self::Revert),
            _ => None,
        }
    }
}

/// A validated plan to conclude one in-progress operation.
///
/// The plan pins the captured operator index, the checkout it was prepared in,
/// that checkout's branch, `HEAD`, the operation tip, and the message. Apply
/// re-proves every one of them before running the single commit command.
#[derive(Debug, Clone)]
pub struct MergeConclusionPlan {
    index_authority: OperatorIndex,
    worktree_root: PathBuf,
    operation: ConcludedOperation,
    expected_head: Oid,
    expected_branch: Option<BranchName>,
    expected_operation_head: Option<Oid>,
    message: Option<String>,
}

impl MergeConclusionPlan {
    /// The captured operator index this plan must conclude.
    pub fn index_authority(&self) -> &OperatorIndex {
        &self.index_authority
    }

    /// The checkout this plan was prepared in.
    pub fn worktree_root(&self) -> &Path {
        &self.worktree_root
    }

    /// The operation that will be concluded.
    pub fn operation(&self) -> ConcludedOperation {
        self.operation
    }

    /// The HEAD the plan observed.
    pub fn expected_head(&self) -> &Oid {
        &self.expected_head
    }

    /// The branch that checkout had checked out, or `None` when detached.
    pub fn branch(&self) -> Option<&BranchName> {
        self.expected_branch.as_ref()
    }

    /// The tip the in-progress operation recorded (`MERGE_HEAD`,
    /// `CHERRY_PICK_HEAD`, or `REVERT_HEAD`), pinned at plan time.
    pub fn expected_operation_head(&self) -> Option<&Oid> {
        self.expected_operation_head.as_ref()
    }

    /// The explicit message, or `None` for `--no-edit`.
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    /// The parent set a conclusion commit created by this plan must have.
    ///
    /// A merge commit records the merge tip as its second parent. A cherry-pick
    /// or revert commit has a single parent (the pinned HEAD); `CHERRY_PICK_HEAD`
    /// and `REVERT_HEAD` record the picked/reverted commit in the message
    /// trailer, not as a parent, so they are pinned and verified but are not part
    /// of the new commit's parent set.
    fn expected_parents(&self) -> Vec<Oid> {
        let mut parents = vec![self.expected_head.clone()];
        if self.operation == ConcludedOperation::Merge
            && let Some(tip) = &self.expected_operation_head
        {
            parents.push(tip.clone());
        }
        parents
    }
}

/// How many reflog entries are scanned for publication evidence. A bound, not a
/// claim: a hook that appends more than this many entries cannot be bound to the
/// invocation, so no candidate is accepted from the reflog.
const REFLOG_BOUND_ENTRIES: usize = 256;

/// What the operation marker (`MERGE_HEAD`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`)
/// reads back as after a conclusion attempt.
///
/// A present marker is not automatically "untouched": a hook may rewrite it to
/// a different tip. Only [`OperationMarkerState::Unchanged`] proves the operation
/// is still exactly the one the plan pinned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationMarkerState {
    /// The marker is gone: the conclusion consumed the operation.
    Consumed,
    /// The marker still records exactly the tip the plan pinned.
    Unchanged,
    /// The marker records a different tip; a hook rewrote the operation.
    Changed {
        /// The tip the marker now records.
        recorded: Oid,
    },
    /// The marker could not be read back (I/O or format failure).
    Unreadable,
}

/// The publication evidence captured immediately before the commit command.
///
/// The reflog is only creation evidence for entries appended to the *pinned*
/// reference after this boundary. `HEAD` is not used: a hook may check out or
/// move another branch, and the commit this call created is published on the
/// ref the plan pinned.
#[derive(Debug, Clone)]
struct PublicationMark {
    reference: String,
    boundary: ReflogBoundary,
}

#[derive(Debug, Clone)]
enum ReflogBoundary {
    /// The pinned reference had no reflog entries before the invocation, so
    /// every entry observed afterwards was appended during it.
    Empty,
    /// Complete, bounded pre-invocation history, newest first. Its unchanged
    /// suffix anchors the count; matching record bytes alone are ambiguous.
    Snapshot(Vec<Vec<u8>>),
    /// The pre-invocation read reached its cap, so its count is not known.
    Saturated,
    /// The pre-invocation reflog could not be read, so nothing can be bound.
    Unreadable,
}

/// The evidence left by a concluded operation.
///
/// `new_head` is the commit **this call created**, proven from evidence bound to
/// the invocation (git's own summary, or a reflog entry appended after the
/// pre-invocation publication watermark on the pinned ref), not a blind read of
/// `HEAD` after hooks ran and not a parent-set match alone. `apply_merge_conclusion`
/// only returns this value when the created commit is also the current `HEAD` and
/// the operation marker is consumed; every other post-state is
/// [`GitError::MergeConclusionUnconfirmed`].
#[derive(Debug, Clone)]
pub struct MergeConclusionOutcome {
    operation: ConcludedOperation,
    previous_head: Oid,
    new_head: Oid,
    branch: Option<BranchName>,
}

impl MergeConclusionOutcome {
    /// The concluded operation.
    pub fn operation(&self) -> ConcludedOperation {
        self.operation
    }

    /// HEAD before the commit.
    pub fn previous_head(&self) -> &Oid {
        &self.previous_head
    }

    /// HEAD after the commit: the commit this call created, verified.
    pub fn new_head(&self) -> &Oid {
        &self.new_head
    }

    /// The branch the conclusion committed on, when HEAD was not detached.
    pub fn branch(&self) -> Option<&BranchName> {
        self.branch.as_ref()
    }
}

fn read_operation_head(
    path: &std::path::Path,
    operation: ConcludedOperation,
) -> Result<Option<Oid>, GitError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(GitError::Io {
                context: "inspect an in-progress operation head",
                source,
            });
        }
    }
    let bytes = sk_kernel::no_follow::read_no_follow(path, 4096).map_err(|error| GitError::Io {
        context: "read an in-progress operation head",
        source: std::io::Error::other(error.to_string()),
    })?;
    if !bytes.iter().any(|byte| !byte.is_ascii_whitespace()) {
        return Err(GitError::InvalidOutput {
            detail: format!("{} is present but empty", path.display()),
        });
    }
    let text = crate::path::utf8_field(&bytes, "operation head")?;
    let mut tips = text.lines().map(str::trim).filter(|line| !line.is_empty());
    let Some(first) = tips.next() else {
        return Err(GitError::InvalidOutput {
            detail: format!("{} is present but empty", path.display()),
        });
    };
    if tips.next().is_some() {
        return Err(GitError::OperationInProgress {
            operation: format!("octopus {}", operation.as_str()),
        });
    }
    Ok(Some(Oid::parse(first)?))
}

impl GitRepository {
    /// Prepare a merge/revert/cherry-pick conclusion.
    ///
    /// Refuses when no concludeable operation is in progress, when a rebase,
    /// sequencer run, squash, or held index lock is present (each has different
    /// semantics this primitive does not claim), when HEAD moved from
    /// `expected_head`, or when an explicit message is empty or contains NUL.
    ///
    /// The plan also pins this checkout, its branch, and the operation tip, so
    /// apply refuses a plan prepared against different state.
    pub fn plan_merge_conclusion(
        &self,
        expected_head: &Oid,
        message: Option<&str>,
    ) -> Result<MergeConclusionPlan, GitError> {
        let operations = self.in_progress_operations()?;
        let mut concludeable = Vec::new();
        for operation in &operations {
            if let Some(concludeable_operation) = ConcludedOperation::from_in_progress(*operation) {
                concludeable.push(concludeable_operation);
            }
        }
        if concludeable.is_empty() {
            if operations.is_empty() {
                return Err(GitError::NoOperationInProgress);
            }
            return Err(GitError::OperationInProgress {
                operation: operations
                    .iter()
                    .map(|operation| operation.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            });
        }
        if concludeable.len() > 1 {
            return Err(GitError::OperationInProgress {
                operation: "multiple in-progress operations".to_string(),
            });
        }
        let operation = concludeable[0];
        let current_head = self.current_head()?;
        if current_head.as_ref() != Some(expected_head) {
            return Err(GitError::PlanStale {
                what: "HEAD".to_string(),
                detail: format!(
                    "expected {expected_head}, observed {}",
                    current_head
                        .as_ref()
                        .map(Oid::to_string)
                        .unwrap_or_else(|| "unborn".to_string())
                ),
            });
        }
        let expected_operation_head = self.operation_head(operation)?;
        if operation == ConcludedOperation::Merge && expected_operation_head.is_none() {
            // A merge conclusion without MERGE_HEAD is not a merge commit; the
            // pinned parent set (HEAD plus the merge tip) would be unprovable.
            return Err(GitError::InvalidOutput {
                detail: "MERGE_HEAD is absent while a merge marker is present".to_string(),
            });
        }
        if let Some(message) = message {
            if message.trim().is_empty() {
                return Err(GitError::InvalidPlan {
                    detail: "an explicit conclusion message must not be empty".to_string(),
                });
            }
            if message.contains('\0') {
                return Err(GitError::InvalidPlan {
                    detail: "a conclusion message must not contain NUL".to_string(),
                });
            }
        }
        Ok(MergeConclusionPlan {
            index_authority: self.operator_index().clone(),
            worktree_root: self.root().to_path_buf(),
            operation,
            expected_head: expected_head.clone(),
            expected_branch: self
                .current_branch()?
                .map(|name| BranchName::parse(&name))
                .transpose()?,
            expected_operation_head,
            message: message.map(str::to_string),
        })
    }

    /// Commit the operation after proving the planned state is unchanged.
    ///
    /// The argv is fixed: no `-a`, no `--no-verify`, no pathspec, no
    /// `--allow-empty`, no `--amend`; hooks and the configured author are left
    /// to git. The returned `new_head` is the commit this call created, proven
    /// by this child's summary or a non-amend publication strictly newer than
    /// its validated reflog boundary. Matching parents are necessary, not
    /// creation proof — never a blind post-hook `HEAD` read.
    ///
    /// A rejected `pre-commit`/`commit-msg` hook is returned as a command
    /// failure and the operation stays in progress for the caller's abort path.
    /// A commit that landed but whose post-state is not the pinned one (a hook
    /// moved `HEAD`, re-created the operation marker, or the readback failed) is
    /// [`GitError::MergeConclusionUnconfirmed`], carrying the created commit and
    /// the observed state. Nothing is rolled back either way.
    pub fn apply_merge_conclusion(
        &self,
        plan: &MergeConclusionPlan,
    ) -> Result<MergeConclusionOutcome, GitError> {
        // The captured operator-index authority is compared first, before any
        // other state probe and long before the commit command: a plan prepared
        // on a different index handle must never commit that handle's tree.
        if self.operator_index() != plan.index_authority() {
            return Err(GitError::PlanStale {
                what: "operator index".to_string(),
                detail: format!(
                    "planned {:?}, observed {:?}",
                    plan.index_authority().explicit_path(),
                    self.operator_index().explicit_path()
                ),
            });
        }
        if self.root() != plan.worktree_root {
            return Err(GitError::PlanStale {
                what: "worktree root".to_string(),
                detail: format!(
                    "planned {}, observed {}",
                    plan.worktree_root.display(),
                    self.root().display()
                ),
            });
        }
        let operations = self.in_progress_operations()?;
        let detected: Vec<ConcludedOperation> = operations
            .iter()
            .filter_map(|operation| ConcludedOperation::from_in_progress(*operation))
            .collect();
        if detected.len() != 1 || detected[0] != plan.operation {
            return Err(GitError::PlanStale {
                what: "in-progress operation".to_string(),
                detail: format!(
                    "planned {}, observed {:?}",
                    plan.operation.as_str(),
                    operations
                        .iter()
                        .map(|operation| operation.as_str())
                        .collect::<Vec<_>>()
                ),
            });
        }
        let current_head = self.current_head()?;
        if current_head.as_ref() != Some(&plan.expected_head) {
            return Err(GitError::PlanStale {
                what: "HEAD".to_string(),
                detail: format!(
                    "planned {}, observed {}",
                    plan.expected_head,
                    current_head
                        .as_ref()
                        .map(Oid::to_string)
                        .unwrap_or_else(|| "unborn".to_string())
                ),
            });
        }
        let current_branch = self
            .current_branch()?
            .map(|name| BranchName::parse(&name))
            .transpose()?;
        if current_branch.as_ref() != plan.branch() {
            return Err(GitError::PlanStale {
                what: "checked-out branch".to_string(),
                detail: format!(
                    "planned {:?}, observed {:?}",
                    plan.expected_branch, current_branch
                ),
            });
        }
        let current_operation_head = self.operation_head(plan.operation)?;
        if current_operation_head != plan.expected_operation_head {
            return Err(GitError::PlanStale {
                what: format!("{} tip", plan.operation.as_str()),
                detail: format!(
                    "planned {}, observed {}",
                    plan.expected_operation_head
                        .as_ref()
                        .map(Oid::to_string)
                        .unwrap_or_else(|| "absent".to_string()),
                    current_operation_head
                        .as_ref()
                        .map(Oid::to_string)
                        .unwrap_or_else(|| "absent".to_string())
                ),
            });
        }

        let mut args: Vec<&str> = vec!["-c", "commit.gpgsign=false", "commit"];
        if let Some(message) = &plan.message {
            args.push("-m");
            args.push(message);
        } else {
            args.push("--no-edit");
        }
        // Capture the publication boundary before anything runs: only reflog
        // entries appended to this pinned reference afterwards can be evidence
        // for a commit this call published.
        let mark = self.publication_mark(plan);
        let attempt = self.process().run_attempt(&args);

        // Collect the post-state first, then identify the created commit from
        // evidence bound to this invocation. Every probe failure is retained as
        // evidence rather than dropped.
        let mut notes: Vec<String> = Vec::new();
        let output = match attempt {
            RunAttempt::NotStarted(error) => {
                // The process never started: nothing can have changed, so this
                // is the ordinary retryable failure it claims to be.
                return Err(error);
            }
            RunAttempt::Completed(output) => Some(output),
            RunAttempt::StartedFailure(error) => {
                // The child ran and did not finish normally: the commit object
                // and its ref update may already exist. Never report this as a
                // plain "nothing landed" failure.
                notes.push(format!(
                    "the commit process started and then failed ({}); its post-state is reported below and nothing is assumed",
                    describe_process_failure(&error)
                ));
                None
            }
        };
        let observed_head = match self.current_head() {
            Ok(head) => head,
            Err(error) => {
                notes.push(format!("HEAD could not be read back: {error}"));
                None
            }
        };
        let marker = self.observe_operation_marker(plan, &mut notes);
        let expected_parents = plan.expected_parents();
        let candidate = self.discover_conclusion_commit(
            output.as_ref().map(|output| output.stdout.as_slice()),
            &expected_parents,
            observed_head.as_ref(),
            &mark,
            &mut notes,
        );

        if let Some(output) = &output {
            let confirmed = output.status.success()
                && candidate.is_some()
                && observed_head.as_ref() == candidate.as_ref()
                && marker == OperationMarkerState::Consumed;
            if confirmed {
                let branch = match self.current_branch() {
                    Ok(name) => name.and_then(|name| BranchName::parse(&name).ok()),
                    Err(error) => {
                        notes.push(format!(
                            "the checked-out branch could not be read back: {error}"
                        ));
                        return Err(self.unconfirmed_conclusion(
                            plan,
                            candidate,
                            observed_head,
                            &marker,
                            Some(output),
                            &notes,
                        ));
                    }
                };
                return Ok(MergeConclusionOutcome {
                    operation: plan.operation,
                    previous_head: plan.expected_head.clone(),
                    new_head: candidate.expect("a confirmed conclusion has a created commit"),
                    branch,
                });
            }

            // Nothing landed: the command completed with a failure status, HEAD
            // is exactly the pinned commit, the operation marker still records
            // the pinned tip, and no commit this call created can be identified.
            // That is a plain refusal (a rejecting `pre-commit` or `commit-msg`
            // hook), and the caller may conclude again after fixing it.
            let nothing_landed = !output.status.success()
                && candidate.is_none()
                && observed_head.as_ref() == Some(&plan.expected_head)
                && marker == OperationMarkerState::Unchanged;
            if nothing_landed {
                return Err(self.process().failed(&args, output));
            }
        }
        Err(self.unconfirmed_conclusion(
            plan,
            candidate,
            observed_head,
            &marker,
            output.as_ref(),
            &notes,
        ))
    }

    /// Read the operation marker and classify it against the pinned tip.
    fn observe_operation_marker(
        &self,
        plan: &MergeConclusionPlan,
        notes: &mut Vec<String>,
    ) -> OperationMarkerState {
        match self.operation_head(plan.operation) {
            Ok(None) => OperationMarkerState::Consumed,
            Ok(Some(recorded)) => {
                if plan.expected_operation_head.as_ref() == Some(&recorded) {
                    OperationMarkerState::Unchanged
                } else {
                    OperationMarkerState::Changed { recorded }
                }
            }
            Err(error) => {
                notes.push(format!(
                    "the {} marker could not be read back: {error}",
                    plan.operation.as_str()
                ));
                OperationMarkerState::Unreadable
            }
        }
    }

    /// Build the typed "may have landed, state not confirmed" failure.
    fn unconfirmed_conclusion(
        &self,
        plan: &MergeConclusionPlan,
        candidate: Option<Oid>,
        observed_head: Option<Oid>,
        marker: &OperationMarkerState,
        output: Option<&CommandOutput>,
        notes: &[String],
    ) -> GitError {
        let mut parts: Vec<String> = Vec::new();
        if let Some(output) = output {
            if output.status.success() {
                parts.push("git commit reported success".to_string());
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                parts.push(format!(
                    "git commit failed (status {:?}): {}",
                    output.status.code(),
                    if stderr.is_empty() {
                        "no error output".to_string()
                    } else {
                        stderr
                    }
                ));
            }
        }
        parts.extend(notes.iter().cloned());
        GitError::MergeConclusionUnconfirmed(Box::new(crate::error::MergeConclusionUnconfirmed {
            operation: plan.operation.as_str().to_string(),
            worktree_root: self.root().to_path_buf(),
            candidate,
            previous_head: plan.expected_head.clone(),
            observed_head,
            operation_consumed: matches!(marker, OperationMarkerState::Consumed),
            marker: marker.clone(),
            detail: parts.join("; "),
        }))
    }

    /// Identify the commit a conclusion command created, if it can be proven.
    ///
    /// The only accepted evidence is bound to this invocation: git's own commit
    /// summary printed by the child, or a reflog entry appended to the pinned
    /// reference after the pre-invocation publication watermark. A parent set is
    /// checked as a necessary condition, never as creation proof: the observed
    /// `HEAD` is not accepted merely because its parents match, because a hook
    /// can point the branch at a pre-existing identical merge.
    fn discover_conclusion_commit(
        &self,
        stdout: Option<&[u8]>,
        expected_parents: &[Oid],
        observed_head: Option<&Oid>,
        mark: &PublicationMark,
        notes: &mut Vec<String>,
    ) -> Option<Oid> {
        let Some(stdout) = stdout else {
            notes.push(
                "the commit command did not finish, so no commit summary is available".to_string(),
            );
            let candidate = self.reflog_conclusion_candidate(expected_parents, mark, notes);
            self.note_observed_head(observed_head, expected_parents, candidate.as_ref(), notes);
            return candidate;
        };
        let abbrevs = summary_commit_abbrevs(stdout);
        if abbrevs.is_empty() {
            notes.push("git printed no commit summary to identify the created commit".to_string());
        }
        for abbrev in abbrevs {
            match self.resolve_commit_object(&abbrev) {
                Ok(Some(oid)) => match self.commit_parents_match(&oid, expected_parents) {
                    Ok(true) => return Some(oid),
                    Ok(false) => notes.push(format!(
                        "git's summary names {oid}, whose parents are not the pinned set"
                    )),
                    Err(error) => {
                        notes.push(format!("the parents of {oid} could not be read: {error}"));
                    }
                },
                Ok(None) => notes.push(format!(
                    "git's summary {abbrev:?} does not resolve to a commit"
                )),
                Err(error) => {
                    notes.push(format!(
                        "git's summary {abbrev:?} could not be resolved: {error}"
                    ));
                }
            }
        }

        // The reflog is creation evidence only for entries appended to the
        // pinned reference after the publication boundary captured before the
        // command ran. Entries that predate the invocation (for example a prior
        // identical merge) are never accepted.
        let candidate = self.reflog_conclusion_candidate(expected_parents, mark, notes);
        self.note_observed_head(observed_head, expected_parents, candidate.as_ref(), notes);
        candidate
    }

    /// Record what the observed `HEAD` proves — and what it does not.
    ///
    /// A parent-set match is a necessary condition, never proof that this call
    /// created the commit: a hook can point the branch at a pre-existing
    /// identical merge. This note keeps that distinction visible in the
    /// unconfirmed evidence.
    fn note_observed_head(
        &self,
        observed_head: Option<&Oid>,
        expected_parents: &[Oid],
        candidate: Option<&Oid>,
        notes: &mut Vec<String>,
    ) {
        let Some(head) = observed_head else {
            return;
        };
        match self.commit_parents_match(head, expected_parents) {
            Ok(true) if candidate != Some(head) => notes.push(format!(
                "HEAD {head} has the pinned parent set, but a parent match is not proof that this call created it"
            )),
            Ok(true) => {}
            Ok(false) => notes.push(format!("HEAD {head} does not have the pinned parent set")),
            Err(error) => notes.push(format!(
                "the parents of HEAD {head} could not be read: {error}"
            )),
        }
    }

    /// The publication boundary for the plan's pinned reference.
    fn publication_mark(&self, plan: &MergeConclusionPlan) -> PublicationMark {
        let reference = match &plan.expected_branch {
            Some(branch) => format!("refs/heads/{}", branch.as_str()),
            None => "HEAD".to_string(),
        };
        self.publication_mark_for(&reference)
    }

    /// Capture bounded history so a duplicate newest record cannot move the
    /// boundary. A capped read is not an exact count and cannot bind fallback.
    fn publication_mark_for(&self, reference: &str) -> PublicationMark {
        let boundary = match self.reflog_records(reference, REFLOG_BOUND_ENTRIES) {
            Ok(records) if records.is_empty() => ReflogBoundary::Empty,
            Ok(records) if records.len() < REFLOG_BOUND_ENTRIES => {
                ReflogBoundary::Snapshot(records)
            }
            Ok(_) => ReflogBoundary::Saturated,
            Err(_) => ReflogBoundary::Unreadable,
        };
        PublicationMark {
            reference: reference.to_string(),
            boundary,
        }
    }

    /// Raw `%H%x00%gs` reflog records for `reference`, newest first.
    fn reflog_records(&self, reference: &str, limit: usize) -> Result<Vec<Vec<u8>>, GitError> {
        let limit = limit.to_string();
        let args = [
            "log",
            "-g",
            "--format=%H%x00%gs",
            "-n",
            limit.as_str(),
            reference,
        ];
        let output = self.process().run(&args)?;
        if !output.status.success() {
            return Err(self.process().failed(&args, &output));
        }
        Ok(output
            .stdout
            .split(|byte| *byte == b'\n')
            .filter(|record| !record.is_empty())
            .map(<[u8]>::to_vec)
            .collect())
    }

    /// Find the conclusion commit from a reflog entry appended by this call.
    ///
    /// Only entries newer than the complete pre-invocation snapshot are
    /// considered. Its exact unchanged suffix and count must fit in the bounded
    /// read; rewritten or saturated history cannot bind a candidate. The parent
    /// check remains necessary, not creation proof on its own.
    fn reflog_conclusion_candidate(
        &self,
        expected_parents: &[Oid],
        mark: &PublicationMark,
        notes: &mut Vec<String>,
    ) -> Option<Oid> {
        if matches!(mark.boundary, ReflogBoundary::Unreadable) {
            notes.push(format!(
                "the {} reflog could not be read before the conclusion, so its entries cannot be bound to this call",
                mark.reference
            ));
            return None;
        }
        let records = match self.reflog_records(&mark.reference, REFLOG_BOUND_ENTRIES) {
            Ok(records) => records,
            Err(error) => {
                notes.push(format!("the reflog could not be searched: {error}"));
                return None;
            }
        };
        // Validate history before considering candidates. Count alone could
        // rebind rewritten history; the newest equal record could be a hook's
        // duplicate. Require the entire captured history as an unchanged suffix.
        let newer_records = match &mark.boundary {
            ReflogBoundary::Snapshot(before) => {
                let Some(newer) = reflog_entries_after_snapshot(&records, before) else {
                    notes.push(format!(
                        "the {} reflog no longer contains its pre-invocation entry sequence in the bounded search, so its entries are not creation evidence for this call",
                        mark.reference
                    ));
                    return None;
                };
                newer
            }
            ReflogBoundary::Empty => records.as_slice(),
            ReflogBoundary::Saturated => {
                notes.push(format!("the {} pre-invocation reflog reached the search bound, so its entry count cannot bind creation evidence", mark.reference));
                return None;
            }
            ReflogBoundary::Unreadable => return None,
        };
        for record in newer_records {
            let Some(separator) = record.iter().position(|byte| *byte == 0) else {
                continue;
            };
            let id_text = match std::str::from_utf8(&record[..separator]) {
                Ok(text) => text.trim(),
                Err(_) => continue,
            };
            let subject = String::from_utf8_lossy(&record[separator + 1..]);
            if !subject.starts_with("commit") || subject.starts_with("commit (amend):") {
                // A post-commit hook may amend the conclusion. That publication
                // is not the original commit whose identity this call reports.
                continue;
            }
            let Ok(oid) = Oid::parse(id_text) else {
                continue;
            };
            match self.commit_parents_match(&oid, expected_parents) {
                Ok(true) => return Some(oid),
                Ok(false) => {}
                Err(error) => notes.push(format!(
                    "the parents of reflog commit {oid} could not be read: {error}"
                )),
            }
        }
        notes.push(format!(
            "no {} non-amend reflog entry appended by this call names a commit with the pinned parents",
            mark.reference
        ));
        None
    }

    /// Resolve an abbreviated commit id printed by git to a full object id.
    fn resolve_commit_object(&self, abbrev: &str) -> Result<Option<Oid>, GitError> {
        let spec = format!("{abbrev}^{{commit}}");
        let output = self
            .process()
            .allowing(&["rev-parse", "--verify", "--quiet", &spec], &[1])?;
        if output.status.code() != Some(0) {
            return Ok(None);
        }
        let text =
            crate::path::utf8_field(crate::path::strip_line_feed(&output.stdout), "commit id")?;
        let text = text.trim();
        if text.is_empty() {
            return Ok(None);
        }
        Oid::parse(text).map(Some)
    }

    /// The parent ids of one commit, read from real object data.
    fn commit_parents(&self, commit: &Oid) -> Result<Vec<Oid>, GitError> {
        let args = ["rev-list", "--parents", "-n", "1", commit.as_str()];
        let output = self.process().run(&args)?;
        if !output.status.success() {
            return Err(self.process().failed(&args, &output));
        }
        let text = crate::path::utf8_field(&output.stdout, "commit parents")?;
        let line = text.lines().next().unwrap_or_default().trim();
        let mut fields = line.split_whitespace();
        if fields.next() != Some(commit.as_str()) {
            return Err(GitError::InvalidOutput {
                detail: format!("git rev-list --parents printed {line:?} for {commit}"),
            });
        }
        fields.map(Oid::parse).collect()
    }

    /// Whether one commit's parent set is exactly `expected` (order-insensitive).
    fn commit_parents_match(&self, commit: &Oid, expected: &[Oid]) -> Result<bool, GitError> {
        let parents = self.commit_parents(commit)?;
        Ok(parents.len() == expected.len()
            && parents.iter().all(|parent| expected.contains(parent)))
    }

    /// The commit an in-progress operation recorded, read from its marker.
    ///
    /// `MERGE_HEAD` may hold several lines for an octopus merge; this adapter
    /// does not claim octopus semantics, so more than one recorded tip is
    /// [`GitError::OperationInProgress`] rather than a silently chosen one.
    pub(crate) fn operation_head(
        &self,
        operation: ConcludedOperation,
    ) -> Result<Option<Oid>, GitError> {
        let marker = match operation {
            ConcludedOperation::Merge => "MERGE_HEAD",
            ConcludedOperation::CherryPick => "CHERRY_PICK_HEAD",
            ConcludedOperation::Revert => "REVERT_HEAD",
        };
        let path = self.git_dirs()?.git_dir().join(marker);
        read_operation_head(&path, operation)
    }

    /// The merge tip recorded by a stopped merge, when one is in progress.
    pub fn merge_head(&self) -> Result<Option<Oid>, GitError> {
        self.operation_head(ConcludedOperation::Merge)
    }

    /// HEAD as a validated object id, or `None` in an unborn repository.
    fn current_head(&self) -> Result<Option<Oid>, GitError> {
        match self.head_commit()? {
            Some(text) => Ok(Some(Oid::parse(&text)?)),
            None => Ok(None),
        }
    }
}

/// A short description of a bounded-process failure that happened after the
/// child process started.
fn describe_process_failure(error: &GitError) -> String {
    match error {
        GitError::Timeout { timeout, .. } => format!("timed out after {timeout:?}"),
        GitError::OutputTooLarge { stream, limit } => {
            format!("the captured {stream} exceeded the {limit}-byte bound")
        }
        other => other.to_string(),
    }
}

/// Every abbreviated commit id git printed in its `git commit` summary.
///
/// The summary line is `[<ref> <abbrev>] <subject>`, `[detached HEAD <abbrev>]
/// <subject>`, or `[<ref> (root-commit) <abbrev>] <subject>`. git routes hook
/// stdout to **stderr**, so this stream carries git's own summary for the commit
/// this invocation created; should a noisy line ever reach stdout, the first
/// header whose commit carries the pinned parent set wins, so a later line
/// cannot select the candidate. Each header is resolved to a real commit.
fn summary_commit_abbrevs(stdout: &[u8]) -> Vec<String> {
    let mut found = Vec::new();
    for line in stdout.split(|byte| *byte == b'\n') {
        if line.first() != Some(&b'[') {
            continue;
        }
        let Some(close) = line.iter().position(|byte| *byte == b']') else {
            continue;
        };
        let inner = &line[1..close];
        let token = inner
            .split(|byte| *byte == b' ')
            .next_back()
            .unwrap_or_default();
        if token.len() < 4 || token.len() > 64 || !token.iter().all(u8::is_ascii_hexdigit) {
            continue;
        }
        found.push(String::from_utf8_lossy(token).into_owned());
    }
    found
}

/// Locate appended entries only when both counts are exact and the complete
/// previous history is still the suffix. A capped read may have lost history.
fn reflog_entries_after_snapshot<'a>(
    current: &'a [Vec<u8>],
    before: &[Vec<u8>],
) -> Option<&'a [Vec<u8>]> {
    if current.len() >= REFLOG_BOUND_ENTRIES || before.len() >= REFLOG_BOUND_ENTRIES {
        return None;
    }
    let position = current.len().checked_sub(before.len())?;
    (current[position..] == *before).then_some(&current[..position])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    #[test]
    fn conclusion_refuses_a_different_operator_index_before_mutating() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let run_git = |args: &[&str], index: Option<&std::path::Path>| {
            let mut command = Command::new("git");
            command
                .current_dir(root)
                .args(args)
                .env_remove("GIT_INDEX_FILE");
            if let Some(index) = index {
                command.env("GIT_INDEX_FILE", index);
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            output.stdout
        };
        run_git(&["init", "-q", "-b", "work"], None);
        run_git(&["config", "user.name", "Fixture"], None);
        run_git(&["config", "user.email", "fixture@example.invalid"], None);
        run_git(&["config", "commit.gpgsign", "false"], None);
        fs::write(root.join("base.txt"), "base\n").unwrap();
        run_git(&["add", "base.txt"], None);
        run_git(&["commit", "-qm", "base"], None);
        run_git(&["checkout", "-qb", "side"], None);
        fs::write(root.join("side.txt"), "side\n").unwrap();
        run_git(&["add", "side.txt"], None);
        run_git(&["commit", "-qm", "side"], None);
        run_git(&["checkout", "-q", "work"], None);
        run_git(&["merge", "--no-ff", "--no-commit", "side"], None);
        let repository = GitRepository::open(root).unwrap();
        let head = repository.current_head().unwrap().unwrap();
        let plan = repository
            .plan_merge_conclusion(&head, Some("merge side"))
            .unwrap();
        let custom = root.join("custom-index");
        run_git(&["read-tree", "HEAD"], Some(&custom));
        let default_before = fs::read(root.join(".git/index")).unwrap();
        let custom_before = fs::read(&custom).unwrap();
        let operation_before = fs::read(root.join(".git/MERGE_HEAD")).unwrap();
        let mut other = GitRepository::open(root).unwrap();
        other.replace_index_authority_for_test(&custom);
        let error = other.apply_merge_conclusion(&plan).unwrap_err();
        assert!(
            matches!(error, GitError::PlanStale { ref what, .. } if what == "operator index"),
            "{error:?}"
        );
        assert_eq!(other.current_head().unwrap(), Some(head));
        assert_eq!(fs::read(root.join(".git/index")).unwrap(), default_before);
        assert_eq!(fs::read(&custom).unwrap(), custom_before);
        assert_eq!(
            fs::read(root.join(".git/MERGE_HEAD")).unwrap(),
            operation_before
        );
    }

    fn git_in(root: &std::path::Path, args: &[&str]) -> Vec<u8> {
        let output = Command::new("git")
            .current_dir(root)
            .args(args)
            .env("HOME", root.join("home"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("TMPDIR", root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    #[test]
    fn reflog_snapshot_counts_distinguish_duplicates_but_never_widen_rewritten_history() {
        let before = vec![
            b"reset".to_vec(),
            b"old commit".to_vec(),
            b"reset".to_vec(),
            b"base".to_vec(),
        ];
        let mut current = vec![b"reset".to_vec(), b"new commit".to_vec()];
        current.extend(before.clone());
        assert_eq!(
            reflog_entries_after_snapshot(&current, &before),
            Some(&current[..2])
        );
        assert_eq!(
            reflog_entries_after_snapshot(&before, &before),
            Some(&before[..0])
        );
        let mut rewritten = current.clone();
        rewritten[2] = b"different boundary".to_vec();
        assert!(reflog_entries_after_snapshot(&rewritten, &before).is_none());
        assert!(reflog_entries_after_snapshot(&before[..1], &before).is_none());
        let capped = vec![b"reset".to_vec(); REFLOG_BOUND_ENTRIES];
        assert!(reflog_entries_after_snapshot(&capped, &before).is_none());
        assert!(reflog_entries_after_snapshot(&capped, &capped).is_none());
    }

    #[test]
    fn summary_abbreviations_are_read_from_gits_commit_summary() {
        assert_eq!(
            summary_commit_abbrevs(b"[main 0345778a] Merge branch 'side'\n"),
            vec!["0345778a".to_string()]
        );
        assert_eq!(
            summary_commit_abbrevs(b"[detached HEAD abc1234] msg\n"),
            vec!["abc1234".to_string()]
        );
        assert_eq!(
            summary_commit_abbrevs(b"[main (root-commit) abc1234] msg\n"),
            vec!["abc1234".to_string()]
        );
        assert!(
            summary_commit_abbrevs(b"nothing to commit, working tree clean\n").is_empty(),
            "a refusal has no created commit to name"
        );
        assert!(
            summary_commit_abbrevs(b"[main xyz] msg\n").is_empty(),
            "a non-hex token is not an object id"
        );
    }

    #[test]
    fn the_reflog_fallback_names_only_a_commit_with_the_pinned_parent_set() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("home")).unwrap();
        fs::create_dir_all(root.join("config")).unwrap();
        git_in(root, &["init", "-q", "-b", "main"]);
        git_in(root, &["config", "user.name", "Fixture"]);
        git_in(root, &["config", "user.email", "fixture@example.invalid"]);
        git_in(root, &["config", "commit.gpgsign", "false"]);
        fs::write(root.join("base.txt"), "base\n").unwrap();
        git_in(root, &["add", "base.txt"]);
        git_in(root, &["commit", "-qm", "base"]);
        let repository = GitRepository::open(root).unwrap();
        let base = repository.current_head().unwrap().unwrap();

        let mark = repository.publication_mark_for("refs/heads/main");
        fs::write(root.join("child.txt"), "child\n").unwrap();
        git_in(root, &["add", "child.txt"]);
        git_in(root, &["commit", "-qm", "child"]);
        let child = repository.current_head().unwrap().unwrap();

        let mut notes = Vec::new();
        assert_eq!(
            repository.reflog_conclusion_candidate(std::slice::from_ref(&base), &mark, &mut notes),
            Some(child),
            "the reflog entry appended by the invocation names the commit with the pinned parent: {notes:?}"
        );
        let unrelated = Oid::parse(&"0".repeat(40)).unwrap();
        let mut notes = Vec::new();
        assert_eq!(
            repository.reflog_conclusion_candidate(&[unrelated], &mark, &mut notes),
            None,
            "an unrelated parent set never answers"
        );
    }

    #[test]
    fn reflog_entries_older_than_the_publication_boundary_are_not_creation_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("home")).unwrap();
        fs::create_dir_all(root.join("config")).unwrap();
        git_in(root, &["init", "-q", "-b", "main"]);
        git_in(root, &["config", "user.name", "Fixture"]);
        git_in(root, &["config", "user.email", "fixture@example.invalid"]);
        git_in(root, &["config", "commit.gpgsign", "false"]);
        fs::write(root.join("base.txt"), "base\n").unwrap();
        git_in(root, &["add", "base.txt"]);
        git_in(root, &["commit", "-qm", "base"]);
        let repository = GitRepository::open(root).unwrap();
        let base = repository.current_head().unwrap().unwrap();

        fs::write(root.join("child.txt"), "child\n").unwrap();
        git_in(root, &["add", "child.txt"]);
        git_in(root, &["commit", "-qm", "child"]);
        let child = repository.current_head().unwrap().unwrap();

        // The boundary is captured *after* the historical commit, so that entry
        // predates this call and must never be answered.
        let mark = repository.publication_mark_for("refs/heads/main");
        let mut notes = Vec::new();
        assert_eq!(
            repository.reflog_conclusion_candidate(std::slice::from_ref(&base), &mark, &mut notes),
            None,
            "a pre-invocation reflog entry is not creation evidence: {notes:?}"
        );
        assert_eq!(repository.current_head().unwrap(), Some(child));

        // A reflog rewritten after the boundary cannot be bound at all.
        git_in(root, &["reflog", "expire", "--expire=now", "--all"]);
        let mut notes = Vec::new();
        assert_eq!(
            repository.reflog_conclusion_candidate(std::slice::from_ref(&base), &mark, &mut notes),
            None,
            "a rewritten reflog cannot be bound to the invocation: {notes:?}"
        );
        assert!(
            notes
                .iter()
                .any(|note| note.contains("no longer contains its pre-invocation entry")),
            "the unbound reflog is explained: {notes:?}"
        );
    }
}
