//! Mutation routes for [`SafeCommitPlan`](crate::SafeCommitPlan) and
//! [`SafeCommitPhysicalMessagePlan`](crate::SafeCommitPhysicalMessagePlan).
//!
//! Every route runs through the one bounded subprocess seam in
//! [`crate::process`]. The only environment difference is [`IndexTarget`]: the
//! operator's real index (the seam's closed `Local` profile, with every
//! redirecting variable removed) or one [`ScopedIndex`] this crate generated
//! inside the checkout's own git directory. There is no second `Command` helper
//! and no caller-supplied environment.
//!
//! # The ordinary worktree transaction
//!
//! The ordinary route reproduces the reference implementation's order, and the
//! real operator index is mutated exactly where the reference mutates it:
//!
//! 0. The captured operator index lock (`<resolved index endpoint>.lock`) is
//!    acquired, and the requested paths' currently staged state is captured as
//!    a requested-only binary patch. A present but unreadable/corrupt index
//!    fails here, before any mutation.
//! 1. Those requested paths are unstaged against the **current** `HEAD` (or
//!    `git rm --cached` when unborn), then each one is staged with
//!    `git add --force` in one private copy whose body is published to the real
//!    index after every step, so a clean filter or observer running for path
//!    *n* sees path *n-1* already staged. Unrelated staged entries, partial
//!    stages and flags are carried through untouched.
//! 2. Only then is the message payload produced (the reference's `-m` argv
//!    boundary). An unencodable code point or a NUL therefore leaves the
//!    requested paths staged, exactly like the reference's uncaught exception,
//!    and the lock is released without a restore.
//! 3. After that boundary, the reference's `commit --only` prepare step is
//!    reproduced on the real index: the affected paths are re-added once
//!    (`add_remove_files`). Git's own racily-clean rule decides there whether
//!    that re-add skips a stat-clean entry or re-hashes a racily clean one into
//!    the real index; the requested paths keep that real-index state after the
//!    commit, as they do in the reference.
//! 4. The candidate tree is built in a *separate* private index seeded from the
//!    pinned parent (`git read-tree <parent>`, then the same `add --force`, or
//!    `git rm --cached` for index-only removals), so nothing the operator staged
//!    outside the requested paths can enter the commit. An **unborn**
//!    destination seeds from git's own canonical empty tree with
//!    `git read-tree --empty`, so no parent object and no hardcoded empty-tree
//!    id is involved.
//! 5. The repository's own commit-message hooks run before the no-op checks
//!    through `git hook run` with `GIT_INDEX_FILE` pointing at the candidate
//!    index. Never `--no-verify`, and never by executing the hook file directly.
//! 6. `git commit-tree` creates the object from the candidate tree, with the
//!    pinned parent (`-p`), or with no `-p` at all for a genuinely unborn
//!    destination.
//! 7. The destination branch advances through one compare-and-swap that requires
//!    the pinned parent (or the ref's absence). A concurrent writer wins and the
//!    commit is refused with candidate/readback evidence if advancement is
//!    unconfirmed; nothing is guessed and nothing is restored afterwards.
//! 8. A landed ordinary commit does **not** reset the requested paths to the
//!    landed tree: the reference's `commit --only` leaves its real-index
//!    preparation in place, so a stateful clean filter can legitimately leave
//!    the requested path staged as a different pass than the committed blob
//!    (`M` residue). The commit's own changed-path set is still read back; a
//!    commit carrying extra paths (a pre-commit hook that staged tracked files)
//!    is reported landed but non-conforming, with only those extras reconciled
//!    to the landed sha. The pinned-parent and index-deletion routes keep their
//!    preparatory requested-path reset/residue contract.
//!
//! A **completed, non-landing** failure (hook rejection, empty changeset, an
//! empty cleaned message, an artifact or object-creation failure) restores the
//! requested paths the way the reference does: from the *current* index, unstage
//! them against the *current* `HEAD`, replay the captured requested-only patch,
//! and publish that body. It is never a whole-index pre-image, so unrelated
//! staging is preserved and a `HEAD` that moved meanwhile is honoured. A
//! possibly-landed/unconfirmed advance and a landed commit keep their evidence
//! instead; a lost lock or an unconfirmed index install never rewrites the index.
//!
//! # The pinned-parent and index-deletion routes
//!
//! These keep the prepared-message, private-index behaviour: they build the
//! candidate tree before the hooks, assert exact bytes, and do not stage the
//! real operator index. Their pre-landing refusals leave the real index as
//! found; the private index is a temporary file removed on drop.
//!
//! No route retries a commit or resets, deletes or stashes user state it was not
//! asked about.

use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use tempfile::NamedTempFile;

use crate::error::{GitError, SafeCommitLandedKind};
use crate::message::CommitMessage;
use crate::oid::Oid;
use crate::path::{GitPath, strip_line_feed, utf8_field};
use crate::process::{CommandOutput, GitProcess, ScopedIndex};
use crate::refname::BranchName;
use crate::refs::RefChange;
use crate::repository::GitRepository;
use crate::safe_commit::{
    DestinationMatch, ExpectedBlob, SafeCommitCoreOutcome, SafeCommitExecution, SafeCommitRecovery,
    SafeCommitRoute, reflog_message,
};

/// How many times the post-landing, idempotent real-index reconciliation is
/// attempted while another writer holds git's index lock.
const RESYNC_ATTEMPTS: usize = 24;
const RESYNC_PAUSE: Duration = Duration::from_millis(5);

/// The index a route reads or writes: the operator's real index or one private
/// index.
#[derive(Debug, Clone, Copy)]
enum IndexTarget<'a> {
    Operator,
    Scoped(&'a ScopedIndex),
}

impl IndexTarget<'_> {
    fn run<S: AsRef<OsStr>>(
        self,
        process: &GitProcess,
        args: &[S],
    ) -> Result<CommandOutput, GitError> {
        match self {
            Self::Operator => process.run(args),
            Self::Scoped(index) => process.run_index_scoped(args, index),
        }
    }

    fn checked<S: AsRef<OsStr>>(
        self,
        process: &GitProcess,
        args: &[S],
    ) -> Result<CommandOutput, GitError> {
        match self {
            Self::Operator => process.checked(args),
            Self::Scoped(index) => process.checked_index_scoped(args, index),
        }
    }

    fn checked_with_stdin<S: AsRef<OsStr>>(
        self,
        process: &GitProcess,
        args: &[S],
        stdin: &[u8],
    ) -> Result<CommandOutput, GitError> {
        match self {
            Self::Operator => process.checked_with_stdin(args, stdin),
            Self::Scoped(index) => process.checked_with_stdin_index_scoped(args, stdin, index),
        }
    }
}

/// Why one transaction did not produce a commit, classified by what it did to
/// the real operator index.
///
/// This is crate-internal: both public families map it onto their own typed
/// outcome, so no caller can mistake "the message was never produced" for a
/// backend commit failure.
pub(crate) enum TransactionFailure<E> {
    /// The message payload could not be produced at the reference's argv
    /// boundary (an unencodable code point, a NUL, a spawn-time OS error).
    ///
    /// The requested paths stay staged in the real operator index exactly as
    /// the reference leaves them; no hook ran and no artifact was written.
    MessageNotProduced(E),
    /// The produced payload was refused by the one payload authority (a NUL in
    /// the message, or the crate's own byte-length bound). Same index footprint
    /// as `MessageNotProduced`: the requested paths stay staged, because the
    /// reference also crosses its argv boundary after staging.
    ///
    /// The crate's byte-length bound is **not** the kernel's `E2BIG` and is not
    /// claimed to match a reference over-long-message refusal; an over-long
    /// reference payload is a separate, unprobed argv-spawn class.
    MessageRejected(GitError),
    /// The transaction's own typed failure. Its index footprint is already
    /// handled: a completed non-landing failure restored the requested path
    /// staging, a possibly-landed/unconfirmed one preserved its evidence.
    Commit(GitError),
}

impl<E> From<GitError> for TransactionFailure<E> {
    fn from(error: GitError) -> Self {
        Self::Commit(error)
    }
}

pub(crate) fn execute(
    repository: &GitRepository,
    plan: &SafeCommitExecution<'_>,
) -> Result<SafeCommitCoreOutcome, GitError> {
    run::<std::convert::Infallible, fn() -> Result<CommitMessage, std::convert::Infallible>>(
        repository, plan, None,
    )
    .map_err(|failure| match failure {
        TransactionFailure::MessageNotProduced(never) => match never {},
        TransactionFailure::MessageRejected(error) | TransactionFailure::Commit(error) => error,
    })
}

/// The ordinary route with a message supplier.
///
/// The supplier is called at the reference implementation's `-m` argv boundary,
/// after the real operator index has been staged and before any hook runs. A
/// supplier error (or a payload refusal) is reported as such and leaves that
/// stage in place.
pub(crate) fn execute_late<E, F>(
    repository: &GitRepository,
    plan: &SafeCommitExecution<'_>,
    supply: F,
) -> Result<SafeCommitCoreOutcome, TransactionFailure<E>>
where
    F: FnOnce() -> Result<CommitMessage, E>,
{
    run(repository, plan, Some(supply))
}

fn run<E, F>(
    repository: &GitRepository,
    plan: &SafeCommitExecution<'_>,
    supply: Option<F>,
) -> Result<SafeCommitCoreOutcome, TransactionFailure<E>>
where
    F: FnOnce() -> Result<CommitMessage, E>,
{
    if plan.worktree_root() != repository.root() {
        return Err(GitError::InvalidPlan {
            detail: format!(
                "safe-commit plan was prepared for {} but applied to {}",
                plan.worktree_root().display(),
                repository.root().display()
            ),
        }
        .into());
    }
    #[cfg(not(unix))]
    if let Some(message) = plan.prepared_message() {
        if std::str::from_utf8(message).is_err() {
            return Err(GitError::Unsupported {
                feature: "a non-UTF-8 commit message reflog on this platform",
            }
            .into());
        }
    }
    repository.process().ensure_unredirected_write()?;
    ensure_index_authority(repository, plan)?;
    recheck_plan(repository, plan)?;
    repository.require_no_commit_operation()?;
    let mut warnings = Vec::new();
    let sha = create_commit(repository, plan, supply, &mut warnings)?;
    let committed = finish_landed_commit(repository, plan, &sha, &mut warnings)?;
    Ok(outcome(plan, sha, committed, warnings))
}

/// Refuse a plan prepared under a different operator-index authority.
///
/// The plan's whole index reconciliation (and its lock authority) names the
/// index captured when it was prepared. A handle whose `GIT_INDEX_FILE` is
/// different would reconcile a different file, so the apply is refused rather
/// than retargeted.
fn ensure_index_authority(
    repository: &GitRepository,
    plan: &SafeCommitExecution<'_>,
) -> Result<(), GitError> {
    if repository.operator_index() == plan.index_authority() {
        return Ok(());
    }
    let describe = |index: &crate::OperatorIndex| match index.explicit_path() {
        Some(path) => path.display().to_string(),
        None => "<default git-dir index>".to_string(),
    };
    Err(GitError::PlanStale {
        what: "operator index".to_string(),
        detail: format!(
            "planned {} ({}), observed {} ({})",
            describe(plan.index_authority()),
            plan.index_authority().source().as_str(),
            describe(repository.operator_index()),
            repository.operator_index().source().as_str()
        ),
    })
}

/// Re-read the state the plan recorded, before any mutation.
///
/// The destination is re-read through the same identity the plan pinned: the
/// legacy short label for a short-branch plan, the exact full symbolic `HEAD`
/// for an exact-ref plan. An unborn destination must still be genuinely absent —
/// a ref that appeared between plan and apply is [`GitError::PlanStale`], never
/// an overwrite or a guessed parent.
fn recheck_plan(
    repository: &GitRepository,
    plan: &SafeCommitExecution<'_>,
) -> Result<(), GitError> {
    match plan.destination_match() {
        DestinationMatch::ShortLabel => {
            let observed = repository.current_branch()?;
            if observed.as_deref() != Some(plan.destination_label()) {
                return Err(GitError::SafeCommitHeadMismatch {
                    destination: plan.destination_label().to_string(),
                    observed_head: observed.unwrap_or_else(|| "<detached>".to_string()),
                    worktree_root: plan.worktree_root().to_path_buf(),
                });
            }
        }
        DestinationMatch::ExactRef => {
            let observed = repository.symbolic_head_ref()?;
            if observed.as_ref() != Some(plan.destination_ref()) {
                return Err(GitError::SafeCommitHeadMismatch {
                    destination: plan.destination_label().to_string(),
                    observed_head: observed
                        .map(|observed| observed.as_str().to_string())
                        .unwrap_or_else(|| "<detached>".to_string()),
                    worktree_root: plan.worktree_root().to_path_buf(),
                });
            }
        }
    }
    let tip = repository.read_ref(plan.destination_ref())?;
    match plan.destination_tip() {
        Some(planned) => {
            let tip = tip.ok_or_else(|| GitError::SafeCommitDestinationMissing {
                destination: plan.destination_label().to_string(),
                worktree_root: plan.worktree_root().to_path_buf(),
            })?;
            if &tip != planned {
                return Err(GitError::PlanStale {
                    what: plan.destination_ref().to_string(),
                    detail: format!("planned tip {planned}, observed {tip}"),
                });
            }
            let head_text = repository.head_commit()?.ok_or_else(|| {
                GitError::SafeCommitDestinationMissing {
                    destination: plan.destination_label().to_string(),
                    worktree_root: plan.worktree_root().to_path_buf(),
                }
            })?;
            let head = Oid::parse(&head_text)?;
            if Some(&head) != plan.head() {
                let planned_head = plan
                    .head()
                    .expect("a born destination pins both HEAD and the tip");
                return Err(GitError::PlanStale {
                    what: plan.destination_ref().to_string(),
                    detail: format!("planned HEAD {planned_head}, observed {head}"),
                });
            }
        }
        None => {
            // A destination that was absent at plan time must still be absent,
            // and HEAD must still name no commit: anything else is a race this
            // transaction refuses rather than overwrites.
            if let Some(observed) = tip {
                return Err(GitError::PlanStale {
                    what: plan.destination_label().to_string(),
                    detail: format!("planned an absent destination, observed {observed}"),
                });
            }
            if repository.head_commit()?.is_some() {
                return Err(GitError::PlanStale {
                    what: plan.destination_label().to_string(),
                    detail: "planned an unborn destination, but HEAD now resolves to a commit"
                        .to_string(),
                });
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Build the commit object against a pinned parent, then CAS the branch
// ---------------------------------------------------------------------------

/// Build and land the commit; returns the new commit id.
///
/// `isolate_hooks` selects the pinned-parent behaviour: the tree is written
/// before the hooks run and must be identical after a re-isolation, so hooks can
/// neither add paths nor change requested content.
///
/// The parent is the pinned parent for that route, otherwise the destination
/// commit itself, or `None` for a genuinely **unborn** destination: that route
/// seeds the private index from git's own empty tree and creates a zero-parent
/// object (no `-p`), so no seed, shadow, `main`, zero or sentinel parent is ever
/// invented.
fn create_commit<E, F>(
    repository: &GitRepository,
    plan: &SafeCommitExecution<'_>,
    supply: Option<F>,
    warnings: &mut Vec<String>,
) -> Result<Oid, TransactionFailure<E>>
where
    F: FnOnce() -> Result<CommitMessage, E>,
{
    let process = repository.process();
    if repository.git_version()? < (2, 36, 0) {
        return Err(GitError::Unsupported {
            feature: "git hook run (requires git >= 2.36) for a safe commit",
        }
        .into());
    }

    let isolate_hooks = plan.route() == SafeCommitRoute::PinnedParent;
    let parent: Option<&Oid> = match plan.route() {
        SafeCommitRoute::PinnedParent => plan.expected_parent(),
        SafeCommitRoute::Worktree | SafeCommitRoute::IndexDeletions => plan.head(),
    };

    if isolate_hooks {
        // The pinned-parent route is the legacy short-branch contract; its
        // destination label is a validated `BranchName`.
        let branch =
            BranchName::parse(plan.destination_label()).map_err(|error| GitError::InvalidPlan {
                detail: format!(
                    "the pinned-parent route needs a short branch destination: {error}"
                ),
            })?;
        let checkouts = repository.checkouts_of(&branch)?;
        if checkouts.len() > 1 {
            return Err(GitError::BranchCheckedOut {
                branch: branch.as_str().to_string(),
                worktrees: checkouts,
            }
            .into());
        }
        if !checkouts.iter().any(|path| path == plan.worktree_root()) {
            return Err(GitError::PlanStale {
                what: branch.as_ref_name().to_string(),
                detail: format!(
                    "the destination is not checked out in the worktree safe commit reconciles at {}",
                    plan.worktree_root().display()
                ),
            }
            .into());
        }
    }

    let worktree_route = plan.route() == SafeCommitRoute::Worktree;
    // A late payload is produced at the reference argv boundary, which only the
    // ordinary worktree route reaches; the pinned-parent and index-deletion
    // routes keep their prepared message file.
    if plan.prepared_message().is_none() && !worktree_route {
        return Err(GitError::InvalidPlan {
            detail: "a late commit message is only accepted by the ordinary worktree route"
                .to_string(),
        }
        .into());
    }
    let git_dir = repository.git_dirs()?.git_dir().to_path_buf();
    let index = ScopedIndex::in_git_dir(&git_dir)?;
    let target = IndexTarget::Scoped(&index);
    let cleanup = MessageCleanup::resolve(process, isolate_hooks)?;

    // The ordinary route runs the reference implementation's own order before
    // any hook or artifact work:
    //
    // 1. capture the requested-only binary patch of what is staged for exactly
    //    these paths (the pre-mutation gate);
    // 2. unstage only the currently staged requested paths against the CURRENT
    //    head;
    // 3. `git add --force` each requested path, publishing each step into the
    //    real operator index so a filter or observer for the next path sees the
    //    real stage.
    //
    // Only then is the message payload produced (the reference's `-m` argv
    // boundary), so an unencodable or NUL-carrying message leaves the stage in
    // the real index instead of refusing before it.
    let mut artifact_lock: Option<OperatorIndexLock> = None;
    let mut staged_patch: Option<Vec<u8>> = None;
    let mut published_body: Option<Vec<u8>> = None;
    let mut message_bytes: Option<Vec<u8>> = None;
    // Observed once, while the repository is still healthy: a hook can break
    // HEAD (or the git dir) and make every later probe fail, and the reference's
    // own recovery is best effort in exactly that state.
    let operator_index = if worktree_route {
        Some(repository.operator_index_path()?)
    } else {
        None
    };
    let staging = operator_index.as_deref().map(|index_path| StagingContext {
        repository,
        process,
        git_dir: &git_dir,
        operator_index: index_path,
    });
    if worktree_route {
        let endpoint = operator_index_endpoint(repository)?;
        let shared = shared_repository_tweak(process)?;
        let mut lock = OperatorIndexLock::acquire(
            crate::index_authority::lock_path_for(&endpoint),
            endpoint,
            shared,
        )?;
        let staging = staging
            .as_ref()
            .expect("a worktree route captured the index");
        let patch = capture_requested_patch(staging, plan)?;
        let body = stage_source_order(staging, &index, plan, &patch, &lock)?;
        published_body = Some(body.clone());
        let payload = match supply {
            Some(supply) => {
                let message = match supply() {
                    Ok(message) => message,
                    Err(error) => return Err(TransactionFailure::MessageNotProduced(error)),
                };
                message
                    .validate()
                    .map_err(TransactionFailure::MessageRejected)?;
                #[cfg(not(unix))]
                if std::str::from_utf8(message.as_bytes()).is_err() {
                    return Err(TransactionFailure::MessageRejected(GitError::Unsupported {
                        feature: "a non-UTF-8 commit message reflog on this platform",
                    }));
                }
                message.as_bytes().to_vec()
            }
            None => plan
                .prepared_message()
                .expect("a prepared execution carries its payload")
                .to_vec(),
        };
        // The lock body hooks observe is the prospective index that was just
        // published: the requested stage plus whatever the operator had staged.
        lock.write_body(&body)?;
        artifact_lock = Some(lock);
        staged_patch = Some(patch);
        message_bytes = Some(payload);
    }
    let message_bytes = match message_bytes {
        Some(bytes) => bytes,
        None => plan
            .prepared_message()
            .expect("a non-worktree route is planned with its payload")
            .to_vec(),
    };

    let commit_message = if worktree_route {
        Some(repository.commit_message_path()?)
    } else {
        None
    };
    let message_file = if worktree_route {
        None
    } else {
        Some(write_message_file(&message_bytes, isolate_hooks)?)
    };
    let message_path = match &commit_message {
        Some(path) => path.absolute().to_path_buf(),
        None => message_file
            .as_ref()
            .expect("a non-worktree route writes a private message file")
            .path()
            .to_path_buf(),
    };

    let landing = (|| -> Result<Oid, GitError> {
        // Source's `commit --only` prepare step runs on the real index after the
        // argv boundary and before any hook: `add_remove_files(partial)` re-adds
        // the requested paths under git's own racily-clean rule. Only then is the
        // candidate (false) index seeded from the pinned parent.
        if let (Some(staging), Some(lock)) = (staging.as_ref(), artifact_lock.as_mut()) {
            published_body = Some(prepare_operator_index_after_stage(
                staging, &index, plan, lock,
            )?);
        }
        // The baseline is the parent's tree, or git's own canonical empty tree for
        // a root commit, so the initially-empty and no-op checks below are the same
        // rule for a born and an unborn destination.
        let baseline_tree = seed_index(process, target, parent)?;
        stage_into(process, target, plan)?;
        if !isolate_hooks {
            retire_candidate_index_raciness(&index, plan)?;
        }

        let (intended_tree, initially_empty) = if isolate_hooks {
            (Some(write_tree(process, target)?), false)
        } else {
            (None, write_tree(process, target)? == baseline_tree)
        };

        record_hook_warnings(
            warnings,
            "pre-commit",
            run_commit_hook(process, target, "pre-commit", &[], !isolate_hooks)?,
        );
        if let Some(lock) = &artifact_lock {
            lock.ensure_owned()?;
        }
        // The ordinary route writes the cleaned message only after pre-commit
        // succeeded: pre-commit sees the stale or absent artifact, prepare and
        // commit-msg see the new one. The write still happens for an unchanged
        // changeset, before the no-op refusal (source probe evidence).
        if worktree_route {
            let initial = cleanup.render(process, target, &message_bytes)?;
            write_commit_artifact(&message_path, &message_with_trailing_newline(initial))?;
        }
        // git commit --only runs pre-commit even for an initially empty
        // changeset, but a formatter cannot turn that snapshot into a commit.
        if initially_empty {
            return Err(GitError::SafeCommitNothingToCommit {
                destination: plan.destination_label().to_owned(),
                detail: "the requested snapshot matched HEAD before the pre-commit hook".to_owned(),
            });
        }
        if let Some(message) = &commit_message {
            record_hook_warnings(
                warnings,
                "prepare-commit-msg",
                run_commit_hook(
                    process,
                    target,
                    "prepare-commit-msg",
                    &[message.spelling(), OsStr::new("message")],
                    !isolate_hooks,
                )?,
            );
            if let Some(lock) = &artifact_lock {
                lock.ensure_owned()?;
            }
            record_hook_warnings(
                warnings,
                "commit-msg",
                run_commit_hook(
                    process,
                    target,
                    "commit-msg",
                    &[message.spelling()],
                    !isolate_hooks,
                )?,
            );
        } else {
            cleanup.apply(process, target, &message_path)?;
            record_hook_warnings(
                warnings,
                "prepare-commit-msg",
                run_commit_hook(
                    process,
                    target,
                    "prepare-commit-msg",
                    &[message_path.as_os_str(), OsStr::new("message")],
                    !isolate_hooks,
                )?,
            );
            record_hook_warnings(
                warnings,
                "commit-msg",
                run_commit_hook(
                    process,
                    target,
                    "commit-msg",
                    &[message_path.as_os_str()],
                    !isolate_hooks,
                )?,
            );
        }

        if let Some(lock) = &artifact_lock {
            lock.ensure_owned()?;
        }
        // The object body is cleanup(artifact readback), but the artifact itself is
        // never rewritten: hook edits (including whitespace) stay in the file.
        let final_message = if worktree_route {
            let raw = read_commit_artifact(&message_path)?;
            cleanup.render(process, target, &raw)?
        } else {
            cleanup.apply(process, target, &message_path)?
        };

        let tree = match intended_tree {
            Some(intended) => {
                let tree = seed_stage_and_write_tree(process, target, parent, plan)?;
                if tree != intended {
                    return Err(GitError::SafeCommitHookChangedPaths);
                }
                verify_expected_bytes(process, target, &tree, plan.expected_bytes())?;
                for removal in plan.index_deletions() {
                    if tree_has_path(process, target, &tree, removal)? {
                        return Err(GitError::SafeCommitInvalidRequest {
                            detail: format!(
                                "index-only removal {} is still present in the candidate tree",
                                removal.display_lossy()
                            ),
                        });
                    }
                }
                tree
            }
            None => write_tree(process, target)?,
        };

        if tree == baseline_tree {
            return Err(GitError::SafeCommitNothingToCommit {
                destination: plan.destination_label().to_string(),
                detail: if parent.is_some() {
                    "the candidate tree equals the pinned parent's tree".to_string()
                } else {
                    "the candidate tree is the empty tree a root commit starts from".to_string()
                },
            });
        }

        if final_message.is_empty() {
            return Err(GitError::SafeCommitCommitFailed {
                destination: plan.destination_label().to_owned(),
                status: None,
                detail: "Aborting commit due to empty commit message".to_owned(),
            });
        }
        if let Some(lock) = &artifact_lock {
            lock.ensure_owned()?;
        }
        // A private object-input file, created only after the hooks, carries the
        // already-cleaned bytes to the one `commit-tree` authority. The observable
        // artifact is not used as cleanup scratch space.
        let object_input = if worktree_route {
            Some(write_object_input(&final_message)?)
        } else {
            None
        };
        let object_path = match &object_input {
            Some(file) => file.path(),
            None => message_path.as_path(),
        };
        let (new_sha, commit_warnings) = create_commit_object(
            process,
            target,
            &tree,
            parent,
            object_path,
            isolate_hooks,
            plan,
        )?;
        warnings.extend(commit_warnings);
        // The private index is no longer needed once the commit object exists; it is
        // released when this scope ends (after the stage/restore decision above).

        // Git's own reflog subject for a commit: `commit: <subject>` normally,
        // `commit (initial): <subject>` for the first commit on a ref. The
        // pre-conversion payload bytes are what is recorded; nothing here writes
        // the object body or a lossy rendering.
        let audit_message = if isolate_hooks {
            message_bytes.clone()
        } else {
            let mut message = if parent.is_some() {
                b"commit: ".to_vec()
            } else {
                b"commit (initial): ".to_vec()
            };
            message.extend_from_slice(first_line_bytes(&final_message));
            message
        };
        // One compare-and-swap through the canonical ref transaction: a born
        // destination requires the observed parent, an unborn destination requires
        // the ref to be **absent** (`RefChange::create` writes the zero old value).
        let change = match parent {
            Some(parent) => RefChange::set(
                plan.destination_ref().clone(),
                new_sha.clone(),
                Some(parent.clone()),
            ),
            None => RefChange::create(plan.destination_ref().clone(), new_sha.clone()),
        };
        // Once an object exists, ownership loss must take the same candidate/
        // readback uncertainty path as any other unconfirmed publication failure.
        let advance = artifact_lock
            .as_ref()
            .map_or(Ok(()), OperatorIndexLock::ensure_owned)
            .and_then(|()| {
                repository
                    .plan_ref_transaction_bytes(&reflog_message_bytes(&audit_message), vec![change])
            })
            .and_then(|transaction| repository.apply_ref_transaction_raw(&transaction));
        if let Err(error) = advance {
            let observed = repository.read_ref(plan.destination_ref());
            let reachable = match &observed {
                Ok(Some(tip)) if tip == &new_sha => Ok(true),
                Ok(Some(tip)) => process
                    .allowing(
                        &[
                            "merge-base",
                            "--is-ancestor",
                            new_sha.as_str(),
                            tip.as_str(),
                        ],
                        &[1],
                    )
                    .map(|output| output.status.success()),
                _ => Ok(false),
            };
            if matches!(reachable, Ok(true)) {
                warnings.push(format!("ref advance reported {error}, but readback proves candidate {new_sha} is reachable; finishing reconciliation"));
            } else {
                return Err(GitError::SafeCommitAdvanceUnconfirmed {
                    destination: plan.destination_label().to_owned(),
                    candidate: new_sha,
                    observed_tip: observed.as_ref().ok().and_then(Clone::clone),
                    detail: format!(
                        "expected parent {}; {error}; ref readback: {observed:?}; candidate reachability: {reachable:?}; hooks and concurrent writers may have changed history; do not assume nothing happened",
                        match parent {
                            Some(parent) => parent.to_string(),
                            None => "<absent destination>".to_string(),
                        }
                    ),
                });
            }
        }

        // The owned lock covers hooks, object creation and the compare-and-swap
        // readback conclusion. It is released (by this scope ending) before the
        // existing post-landing index reconciliation, which needs the operator
        // index's own lock.
        Ok(new_sha)
    })();

    match landing {
        Ok(sha) => Ok(sha),
        Err(error) => {
            // A completed, non-landing failure restores the requested paths'
            // pre-call staging: unstage them against the CURRENT head, replay the
            // captured requested-only patch, publish that body. Never the whole
            // index pre-image, and never after a possibly-landed/unknown CAS or a
            // lost lock.
            if let (Some(patch), Some(lock), Some(staging)) = (
                staged_patch.as_ref(),
                artifact_lock.as_ref(),
                staging.as_ref(),
            ) && source_restores_after_failure(&error)
            {
                restore_requested_staging(
                    staging,
                    plan,
                    patch,
                    published_body.as_deref().unwrap_or(&[]),
                    lock,
                    &error,
                )
                .map_err(TransactionFailure::Commit)?;
            }
            Err(TransactionFailure::Commit(error))
        }
    }
}

/// The ordinary route inherits git commit -m's configured cleanup. The pinned
/// route intentionally keeps the baseline's verbatim commit-tree message.
#[derive(Clone, Copy)]
enum MessageCleanup {
    Verbatim,
    Whitespace,
    StripComments,
}

impl MessageCleanup {
    fn resolve(process: &GitProcess, pinned: bool) -> Result<Self, GitError> {
        if pinned {
            return Ok(Self::Verbatim);
        }
        let output = process.allowing(&["config", "--get", "commit.cleanup"], &[1])?;
        if output.status.code() == Some(1) {
            return Ok(Self::Whitespace);
        }
        match strip_line_feed(&output.stdout) {
            b"default" | b"whitespace" | b"scissors" => Ok(Self::Whitespace),
            b"verbatim" => Ok(Self::Verbatim),
            b"strip" => Ok(Self::StripComments),
            other => Err(GitError::SafeCommitInvalidRequest {
                detail: format!("invalid commit.cleanup: {}", String::from_utf8_lossy(other)),
            }),
        }
    }

    /// Clean `raw` according to this policy without writing it anywhere.
    fn render(
        self,
        process: &GitProcess,
        target: IndexTarget<'_>,
        raw: &[u8],
    ) -> Result<Vec<u8>, GitError> {
        match self {
            Self::Verbatim => Ok(raw.to_vec()),
            Self::Whitespace => Ok(target
                .checked_with_stdin(process, &["stripspace"], raw)?
                .stdout),
            Self::StripComments => Ok(target
                .checked_with_stdin(process, &["stripspace", "--strip-comments"], raw)?
                .stdout),
        }
    }

    /// Clean a private message file in place (the pinned and index-deletion
    /// routes). The ordinary route uses [`Self::render`] because its artifact
    /// must keep the hook-written bytes.
    fn apply(
        self,
        process: &GitProcess,
        target: IndexTarget<'_>,
        path: &Path,
    ) -> Result<Vec<u8>, GitError> {
        let raw = sk_kernel::no_follow::read_no_follow(
            path,
            crate::safe_commit::MAX_COMMIT_MESSAGE_LEN as u64 + 1,
        )
        .map_err(|error| GitError::SafeCommitInvalidRequest {
            detail: format!("read commit message: {error}"),
        })?;
        if matches!(self, Self::Verbatim) {
            return Ok(raw);
        }
        let cleaned = self.render(process, target, &raw)?;
        fs::write(path, &cleaned).map_err(|source| GitError::Io {
            context: "clean the safe-commit message",
            source,
        })?;
        Ok(cleaned)
    }
}

/// The operator index's own create-new lock, with owned-identity cleanup.
///
/// The file is Git's resolved final index endpoint plus `.lock`, obtained from
/// the repository's canonical lock getter, not an advisory or spelled-alias lock.
/// It is created only for the ordinary worktree route, after the transaction's
/// state checks and before the first index mutation, and held through the
/// ref compare-and-swap/readback. Cleanup removes the file only when it is the
/// same identity this process created and no replacement has taken the path.
///
/// The same object also publishes the staged body at the **same** resolved
/// endpoint it locked, so a symlinked-alias retarget can never have the stage
/// installed under a target this lock does not cover.
struct OperatorIndexLock {
    path: PathBuf,
    /// The physical index endpoint git itself locks (final symlink component
    /// resolved once, at lock time).
    endpoint: PathBuf,
    // Keep the identity handle open: Windows file identifiers can be reused
    // after the last handle closes. No fabricated zero IDs or mtime heuristics.
    identity: same_file::Handle,
    pid: u32,
    /// Git's `core.sharedRepository` permission adjustment, read once from the
    /// same configuration git itself would read. `0` means "umask only".
    shared: i32,
}

/// True when `path` still names exactly the file behind `identity`.
///
/// A replaced, removed, symlinked or unreadable path is not ours; the caller
/// must preserve it rather than unlink or publish over it.
fn path_is_identity(path: &Path, identity: &same_file::Handle) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
        && same_file::Handle::from_path(path).is_ok_and(|observed| &observed == identity)
}

impl OperatorIndexLock {
    fn acquire(path: PathBuf, endpoint: PathBuf, shared: i32) -> Result<Self, GitError> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|source| {
                if source.kind() == std::io::ErrorKind::AlreadyExists {
                    GitError::SafeCommitIndexLocked {
                        path: path.clone(),
                        detail:
                            "the operator index lock already exists; it is not ours and is left untouched"
                                .to_string(),
                    }
                } else {
                    GitError::SafeCommitIndexLockUnavailable {
                        path: path.clone(),
                        detail: source.to_string(),
                    }
                }
            })?;
        // Transfer the already-open descriptor instead of duplicating it after
        // create_new: EMFILE must not turn successful creation into a leaked
        // lock. The handle also supplies the writable File for its whole life.
        let identity = same_file::Handle::from_file(file)
            .map_err(|source| GitError::SafeCommitIndexLockUnavailable {
                path: path.clone(),
                detail: format!("created the lock, but its identity could not be captured: {source}; a possible leftover at this path is not safe to unlink by name alone"),
            })?;
        let owned = Self {
            path,
            endpoint,
            identity,
            pid: std::process::id(),
            shared,
        };
        set_index_permissions(owned.identity.as_file(), shared).map_err(|source| {
            GitError::SafeCommitIndexLockUnavailable {
                path: owned.path.clone(),
                detail: format!("adjust the owned index lock permissions: {source}"),
            }
        })?;
        Ok(owned)
    }

    fn owns_path(&self) -> bool {
        self.pid == std::process::id() && path_is_identity(&self.path, &self.identity)
    }

    fn ensure_owned(&self) -> Result<(), GitError> {
        if self.owns_path() {
            Ok(())
        } else {
            Err(GitError::SafeCommitIndexLockUnavailable {
                path: self.path.clone(),
                detail: "the captured operator-index lock no longer names this attempt's owned file; preserve the foreign state and do not assume nothing changed".to_string(),
            })
        }
    }

    fn write_body(&mut self, bytes: &[u8]) -> Result<(), GitError> {
        self.ensure_owned()?;
        self.identity
            .as_file_mut()
            .seek(SeekFrom::Start(0))
            .and_then(|_| self.identity.as_file_mut().set_len(0))
            .and_then(|_| self.identity.as_file_mut().write_all(bytes))
            .and_then(|_| self.identity.as_file_mut().flush())
            .map_err(|source| GitError::SafeCommitIndexLockUnavailable {
                path: self.path.clone(),
                detail: source.to_string(),
            })
    }

    /// Install `bytes` as the real operator index at the endpoint this lock owns.
    ///
    /// Ownership is re-checked first, and the captured spelling must still
    /// resolve to the same physical endpoint: a retargeted alias is refused
    /// rather than published under a lock that no longer covers it. The install
    /// itself is a same-directory create-new + rename, so it is atomic and never
    /// crosses a filesystem.
    fn publish(&self, repository: &GitRepository, bytes: &[u8]) -> Result<(), GitError> {
        self.ensure_owned()?;
        let observed = operator_index_endpoint(repository)?;
        if observed != self.endpoint {
            return Err(GitError::SafeCommitIndexPublishFailed {
                path: self.endpoint.clone(),
                detail: format!(
                    "the captured index now resolves to {} instead of the locked endpoint {}; refusing to publish another target's index under this lock",
                    observed.display(),
                    self.endpoint.display()
                ),
            });
        }
        install_index_body(&self.endpoint, bytes, self.shared)
    }
}

impl Drop for OperatorIndexLock {
    fn drop(&mut self) {
        if self.owns_path() {
            let _ = fs::remove_file(&self.path);
        }
        // Missing, unreadable, symlinked or replaced path: never unlink a file
        // on the strength of a name, size/mtime guess or another process's PID.
    }
}

/// The physical operator-index endpoint git itself resolves at lock time.
fn operator_index_endpoint(repository: &GitRepository) -> Result<PathBuf, GitError> {
    Ok(crate::index_authority::resolve_final_symlink(
        &repository.operator_index_path()?,
    ))
}

/// The captured operator-index facts one ordered stage/recovery needs.
///
/// `git_dir` and `operator_index` are observed **before** the first mutation:
/// a hook can break `HEAD` (or the git directory) and make every later probe
/// fail, and the reference's own recovery is best effort in exactly that state.
struct StagingContext<'a> {
    repository: &'a GitRepository,
    process: &'a GitProcess,
    git_dir: &'a Path,
    /// The captured operator index spelling (the raw selector, not the lock
    /// endpoint).
    operator_index: &'a Path,
}

/// Source's `_staged_patch_for_paths`: the requested-only binary patch of what
/// is staged for exactly these paths, captured **before** the index is mutated.
///
/// This is also the pre-mutation gate: a present but unreadable/corrupt index
/// fails here, so nothing is staged and the operator index is left untouched.
fn capture_requested_patch(
    ctx: &StagingContext<'_>,
    plan: &SafeCommitExecution<'_>,
) -> Result<Vec<u8>, GitError> {
    let paths = plan.paths();
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let mut args: Vec<OsString> = [
        "diff",
        "--cached",
        "--binary",
        "--no-ext-diff",
        "--no-renames",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    push_paths(&mut args, paths);
    let output = ctx.process.run(&args)?;
    if !output.status.success() {
        return Err(GitError::SafeCommitPatchCaptureFailed {
            worktree_root: plan.worktree_root().to_path_buf(),
            detail: combined_output(&output),
        });
    }
    // The reference reads this child's stdout with `text=True,
    // encoding="utf-8", errors="replace"`, which also applies universal-newline
    // translation. That `str` is what `git apply` later receives on stdin, so
    // the same text is what recovery replays; nothing is stripped.
    Ok(source_text_bytes(&output.stdout))
}

/// Python's UTF-8 `errors="replace"` decode plus universal-newline translation.
fn source_text_bytes(raw: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(raw);
    let bytes = text.as_bytes();
    let mut translated = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\r' => {
                translated.push(b'\n');
                if bytes.get(index + 1) == Some(&b'\n') {
                    index += 1;
                }
            }
            byte => translated.push(byte),
        }
        index += 1;
    }
    translated
}

/// Copy the current operator index into one private scoped index.
fn copy_current_operator_index(
    operator: &Path,
    process: &GitProcess,
    index: &ScopedIndex,
) -> Result<(), GitError> {
    match File::open(operator) {
        Ok(mut file) => {
            // Git reads both the entries and the index file timestamp. Replacing
            // the latter with the private copy's creation time changes the
            // equal-second/nanosecond racily-clean decision and filter output.
            let modified = file
                .metadata()
                .and_then(|metadata| metadata.modified())
                .map_err(|source| GitError::Io {
                    context: "stat the operator index snapshot",
                    source,
                })?;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)
                .map_err(|source| GitError::Io {
                    context: "read the operator index snapshot",
                    source,
                })?;
            fs::write(index.path(), bytes).map_err(|source| GitError::Io {
                context: "copy the operator index for the safe-commit stage",
                source,
            })?;
            OpenOptions::new()
                .write(true)
                .open(index.path())
                .and_then(|copy| copy.set_times(fs::FileTimes::new().set_modified(modified)))
                .map_err(|source| GitError::Io {
                    context: "retain the operator index snapshot timestamp",
                    source,
                })
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            IndexTarget::Scoped(index).checked(process, &["read-tree", "--empty"])?;
            Ok(())
        }
        Err(source) => Err(GitError::Io {
            context: "read the operator index for the safe-commit stage",
            source,
        }),
    }
}

/// Source `_unstage_requested_files`: best effort, scoped to the requested
/// paths, against the **current** head (or `rm --cached` when unborn).
///
/// The staged-path probe is `changed_paths(cached=True, pathspecs=...)`; like the
/// reference, a failed probe leaves nothing safe to unstage and the step is
/// skipped rather than turned into a new refusal.
fn unstage_requested(
    process: &GitProcess,
    target: IndexTarget<'_>,
    paths: &[GitPath],
) -> Result<(), GitError> {
    if paths.is_empty() {
        return Ok(());
    }
    let staged = match staged_cached_paths(process, target, paths) {
        Ok(staged) => staged,
        Err(_) => return Ok(()),
    };
    if staged.is_empty() {
        return Ok(());
    }
    if has_head_commit(process, target)? {
        let mut args = base(["restore", "--staged"]);
        push_paths(&mut args, &staged);
        // A completed non-zero return code is ignored exactly like the
        // reference's best-effort unstage; a spawn/capture error is not (the
        // reference's direct subprocess call raises there).
        target.run(process, &args)?;
    } else {
        let mut args = base(["rm", "--cached", "--ignore-unmatch", "-q"]);
        push_paths(&mut args, &staged);
        target.run(process, &args)?;
    }
    Ok(())
}

fn staged_cached_paths(
    process: &GitProcess,
    target: IndexTarget<'_>,
    paths: &[GitPath],
) -> Result<Vec<GitPath>, GitError> {
    let mut args = base([
        "-c",
        "diff.relative=false",
        "diff",
        "--name-only",
        "-z",
        "--no-renames",
        "--cached",
    ]);
    push_paths(&mut args, paths);
    let output = target.checked(process, &args)?;
    parse_paths_z(&output.stdout)
}

/// Source's `_run_git_text(repo, ["rev-parse", "--verify", "HEAD"])` shape:
/// a completed non-zero exit means `None` ("no HEAD"), while a spawn, timeout
/// or capture failure is a real error. Returning `false` for the latter would
/// silently claim "unborn" after git never started.
fn has_head_commit(process: &GitProcess, target: IndexTarget<'_>) -> Result<bool, GitError> {
    let output = target.run(process, &["rev-parse", "--verify", "HEAD"])?;
    Ok(output.status.success())
}

/// Source's stage order, performed on one private scoped index and published to
/// the real operator index after each step.
///
/// The reference mutates the real index at every one of these steps, so a clean
/// filter or observer running for path *n* sees path *n-1* already staged. The
/// private body is therefore installed after the unstage and after each
/// successful `add --force`, under the one owned endpoint lock. A failed `add`
/// (the first one, as in the reference) restores the requested-path staging
/// before the typed staging failure is returned.
fn stage_source_order(
    ctx: &StagingContext<'_>,
    index: &ScopedIndex,
    plan: &SafeCommitExecution<'_>,
    patch: &[u8],
    lock: &OperatorIndexLock,
) -> Result<Vec<u8>, GitError> {
    copy_current_operator_index(ctx.operator_index, ctx.process, index)?;
    let target = IndexTarget::Scoped(index);
    unstage_requested(ctx.process, target, plan.paths())?;
    let mut body = read_scoped_body(index)?;
    lock.publish(ctx.repository, &body)?;
    for path in plan.paths() {
        match stage_paths(ctx.process, target, std::slice::from_ref(path)) {
            Ok(()) => {
                body = read_scoped_body(index)?;
                lock.publish(ctx.repository, &body)?;
            }
            Err((path, detail)) => {
                let failure = GitError::SafeCommitStageFailed { path, detail };
                restore_requested_staging(ctx, plan, patch, &body, lock, &failure)?;
                return Err(failure);
            }
        }
    }
    Ok(body)
}

fn read_scoped_body(index: &ScopedIndex) -> Result<Vec<u8>, GitError> {
    fs::read(index.path()).map_err(|source| GitError::Io {
        context: "read the staged operator index body",
        source,
    })
}

/// Source's `commit --only` prepare step on the **real** operator index.
///
/// git 2.43 `builtin/commit.c` prepares a partial commit by (1) re-adding the
/// affected paths to the real index (`add_remove_files`), (2) refreshing it,
/// (3) writing it out locked, then (4) building the false index from `HEAD` and
/// re-adding the same paths there. Step (1) is where a racily-clean entry is
/// re-hashed through the clean filter: `add_to_index` uses
/// `CE_MATCH_RACY_IS_DIRTY`, so a stat-clean entry is skipped and a racy one is
/// filtered again into the real index. Step (2) cannot add another pass for
/// that path because `fill_stat_cache_info` marked the just-added entry
/// `CE_UPTODATE` (read-cache.c:187-198, 402-407), so `refresh_index` returns it
/// unchanged.
///
/// The private copy is seeded from the body just published to the real index,
/// so the same git operation decides raciness against the same entry data. The
/// resulting body is published back under the one owned endpoint lock.
fn prepare_operator_index_after_stage(
    ctx: &StagingContext<'_>,
    index: &ScopedIndex,
    plan: &SafeCommitExecution<'_>,
    lock: &mut OperatorIndexLock,
) -> Result<Vec<u8>, GitError> {
    copy_current_operator_index(ctx.operator_index, ctx.process, index)?;
    let target = IndexTarget::Scoped(index);
    for path in plan.paths() {
        // `add_remove_files` only hashes a path that still exists in the
        // worktree; for a missing file it removes the index entry, and the
        // ordered stage already removed it. Re-running `git add` on that
        // missing path would also fail with "pathspec did not match".
        let worktree_path = plan.worktree_root().join(path.to_path_buf());
        if fs::symlink_metadata(&worktree_path).is_err() {
            continue;
        }
        stage_paths(ctx.process, target, std::slice::from_ref(path))
            .map_err(|(path, detail)| GitError::SafeCommitStageFailed { path, detail })?;
    }
    let body = read_scoped_body(index)?;
    lock.publish(ctx.repository, &body)?;
    lock.write_body(&body)?;
    Ok(body)
}

/// The reference's false index is written and then turned into a tree by the
/// **same** git process: `fill_stat_cache_info` marked every just-added entry
/// `CE_UPTODATE`, so the later refresh/cache-tree work cannot re-run the clean
/// filter for it. Native writes and reads that private index through separate
/// git processes, where the in-core `CE_UPTODATE` flag is gone and a
/// future-dated entry is racily clean again; `git write-tree` would then add a
/// filter pass the reference never runs (and commit a different pass number).
///
/// Move only the private candidate index file's own timestamp past the newest
/// staged worktree file. Its entries and body are untouched; the tree writer
/// then reuses the just-added entry exactly as the reference's in-process
/// `CE_UPTODATE` does. The operator index is never touched.
fn retire_candidate_index_raciness(
    index: &ScopedIndex,
    plan: &SafeCommitExecution<'_>,
) -> Result<(), GitError> {
    let mut latest = std::time::SystemTime::now();
    for path in plan.paths() {
        let worktree_path = plan.worktree_root().join(path.to_path_buf());
        if let Ok(metadata) = fs::symlink_metadata(worktree_path)
            && let Ok(modified) = metadata.modified()
            && modified > latest
        {
            latest = modified;
        }
    }
    let modified = latest + Duration::from_secs(1);
    let file = OpenOptions::new()
        .write(true)
        .open(index.path())
        .map_err(|source| GitError::Io {
            context: "open the private candidate index to retire raciness",
            source,
        })?;
    file.set_times(fs::FileTimes::new().set_modified(modified))
        .map_err(|source| GitError::Io {
            context: "retire raciness on the private candidate index",
            source,
        })
}

/// Source `_restore_staged_patch`: from the **current** real index, unstage the
/// requested paths against the **current** head, replay the captured
/// requested-only patch, and publish that result.
///
/// Never the whole index pre-image: entries outside the requested paths keep
/// exactly the state the operator's index has right now, including unrelated
/// staging, partial stages and flags. The reference's own restore is best
/// effort about the probe and the unstage; only a rejected patch replay is a
/// recovery failure. Nothing is published when the derived body equals the
/// current index bytes.
fn restore_requested_staging(
    ctx: &StagingContext<'_>,
    plan: &SafeCommitExecution<'_>,
    patch: &[u8],
    last_published: &[u8],
    lock: &OperatorIndexLock,
    original: &GitError,
) -> Result<(), GitError> {
    let recovery = |detail: String| GitError::SafeCommitRecoveryFailed {
        worktree_root: plan.worktree_root().to_path_buf(),
        detail,
        original_failure: original.to_string(),
    };
    let current = match fs::read(ctx.operator_index) {
        Ok(bytes) => bytes,
        // With no captured patch there is nothing to replay or unstage, which is
        // exactly the reference's no-op restore; a current index that cannot be
        // read (a hook that broke the git dir) is not a new recovery failure.
        Err(_) if patch.is_empty() => return Ok(()),
        Err(source) => {
            return Err(recovery(format!(
                "could not read the current operator index to replay the requested patch: {source}"
            )));
        }
    };
    // The owned `.lock` excludes another *locker*, not an out-of-band writer.
    // Restoring is only safe while the operator index still holds the exact body
    // this transaction published last; anything else is foreign current data and
    // is preserved (with the captured patch and the original failure) rather than
    // overwritten by a replay of the old patch.
    if current != last_published {
        return Err(recovery(format!(
            "the operator index changed after this transaction last published it (held lock does not exclude out-of-band writers); foreign/current staging is preserved, the captured requested patch was not replayed, and the last observed endpoint was {}",
            ctx.operator_index.display()
        )));
    }
    let index = ScopedIndex::in_git_dir(ctx.git_dir).map_err(|error| {
        recovery(format!(
            "could not create a private index for the requested-path recovery: {error}"
        ))
    })?;
    fs::write(index.path(), &current).map_err(|source| {
        recovery(format!(
            "could not copy the current operator index for the requested-path recovery: {source}"
        ))
    })?;
    let target = IndexTarget::Scoped(&index);
    unstage_requested(ctx.process, target, plan.paths()).map_err(|error| {
        recovery(format!(
            "could not unstage the requested paths during recovery: {error}"
        ))
    })?;
    // The reference plays the captured patch on the real index, so a rejected
    // replay still leaves the unstage it already performed. Publish exactly what
    // git did to the private copy, then report the replay failure; never claim
    // success because some earlier body could have been installed instead.
    let mut replay_failure = None;
    if !patch.is_empty() {
        let args: Vec<OsString> = ["apply", "--cached", "--whitespace=nowarn", "-"]
            .iter()
            .map(OsString::from)
            .collect();
        match ctx
            .process
            .run_with_stdin_index_scoped(&args, patch, &index)
        {
            Ok(output) if output.status.success() => {}
            Ok(output) => {
                replay_failure = Some(format!(
                    "git apply --cached rejected the requested-file patch: {}",
                    combined_output(&output)
                ));
            }
            Err(error) => replay_failure = Some(error.to_string()),
        }
    }
    let body = fs::read(index.path()).map_err(|source| {
        recovery(format!(
            "could not read the recovered operator index body: {source}"
        ))
    })?;
    if body != current {
        lock.publish(ctx.repository, &body).map_err(|error| {
            recovery(format!(
                "could not publish the recovered requested-path index: {error}"
            ))
        })?;
    }
    match replay_failure {
        Some(detail) => Err(recovery(detail)),
        None => Ok(()),
    }
}

/// Whether a completed failure restores the requested-path staging.
///
/// A possibly-landed/unconfirmed advance and a landed commit keep their
/// evidence instead (nothing is guessed). A lost lock or an unconfirmed install
/// also never rewrites the index: ownership and current data are unproven.
fn source_restores_after_failure(error: &GitError) -> bool {
    !matches!(
        error,
        GitError::SafeCommitAdvanceUnconfirmed { .. }
            | GitError::SafeCommitLanded { .. }
            | GitError::SafeCommitIndexLocked { .. }
            | GitError::SafeCommitIndexLockUnavailable { .. }
            | GitError::SafeCommitIndexPublishFailed { .. }
            | GitError::SafeCommitPatchCaptureFailed { .. }
            | GitError::SafeCommitRecoveryFailed { .. }
    )
}

/// Git's `core.sharedRepository` permission adjustment, read through the same
/// configuration git itself reads.
///
/// The returned integer follows git's `git_config_perm` convention: `0` is
/// `umask`, a positive value is a tweak to OR into the create mode (`0660` for
/// `group`, `0664` for `all`/`everybody`), and a negative value is an explicit
/// octal file mode to restrict to (`-0660`).
fn shared_repository_tweak(process: &GitProcess) -> Result<i32, GitError> {
    let output = process.allowing(&["config", "--get", "core.sharedrepository"], &[1])?;
    if output.status.code() == Some(1) && !output.status.success() {
        return Ok(0);
    }
    if !output.status.success() {
        return Err(GitError::SafeCommitIndexPublishFailed {
            path: PathBuf::from("<operator index>"),
            detail: format!(
                "could not read core.sharedRepository: {}",
                combined_output(&output)
            ),
        });
    }
    // --get collapses a bare key and an empty value to the same LF. Git's
    // boolean reader distinguishes NULL (true) from "" (false), exactly the
    // group/umask distinction git_config_perm needs. Read only the effective
    // value, not an unbounded list of overridden configuration occurrences.
    let raw = strip_line_feed(&output.stdout);
    if raw.is_empty() {
        return shared_repository_boolean(process);
    }
    match parse_shared_repository(raw) {
        Ok(tweak) => Ok(tweak),
        Err(_) if shared_octal_value(raw).is_none() => {
            // git_config_perm delegates a non-octal token to git_config_bool,
            // including Git's numeric/suffix syntax (e.g. 8 or 1k). Keep that
            // grammar with Git instead of maintaining a second bool parser.
            shared_repository_boolean(process)
        }
        Err(detail) => Err(GitError::SafeCommitIndexPublishFailed {
            path: PathBuf::from("<operator index>"),
            detail,
        }),
    }
}

fn shared_repository_boolean(process: &GitProcess) -> Result<i32, GitError> {
    let boolean = process.checked(&["config", "--type=bool", "--get", "core.sharedrepository"])?;
    match strip_line_feed(&boolean.stdout) {
        b"true" => Ok(0o660),
        b"false" => Ok(0),
        _ => Err(GitError::InvalidOutput {
            detail: "git config --type=bool returned neither true nor false".to_owned(),
        }),
    }
}

/// Mirror git's `git_config_perm` parsing for the shared-repository value.
fn parse_shared_repository(raw: &[u8]) -> Result<i32, String> {
    match raw {
        b"umask" => return Ok(0),
        b"group" => return Ok(0o660),
        b"all" | b"world" | b"everybody" => return Ok(0o664),
        _ => {}
    }
    if let Some(mode) = shared_octal_value(raw) {
        return match mode {
            0 => Ok(0),
            1 => Ok(0o660),
            2 => Ok(0o664),
            _ if mode & 0o600 == 0o600 => Ok(-(mode & 0o666)),
            _ => Err(format!(
                "invalid core.sharedRepository owner permissions: {mode:#o}"
            )),
        };
    }
    match raw.to_ascii_lowercase().as_slice() {
        b"true" | b"yes" | b"on" => Ok(0o660),
        b"false" | b"no" | b"off" => Ok(0),
        _ => Err(format!(
            "core.sharedRepository requires Git boolean parsing: {:?}",
            String::from_utf8_lossy(raw)
        )),
    }
}

/// git_config_perm first uses strtol(base=8), then narrows C long to C int.
/// Preserve signs, C whitespace and strtol saturation; only a fully consumed
/// octal value avoids its fallback to Git's own boolean parser.
fn shared_octal_value(raw: &[u8]) -> Option<i32> {
    if raw.is_empty() {
        return Some(0);
    }
    let mut position = 0;
    while raw
        .get(position)
        .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c))
    {
        position += 1;
    }
    let negative = raw.get(position) == Some(&b'-');
    if matches!(raw.get(position), Some(b'+' | b'-')) {
        position += 1;
    }
    let start = position;
    let limit = std::os::raw::c_long::MAX as u64 + u64::from(negative);
    let mut magnitude = 0u64;
    while let Some(byte @ b'0'..=b'7') = raw.get(position) {
        magnitude = magnitude
            .saturating_mul(8)
            .saturating_add(u64::from(byte - b'0'))
            .min(limit);
        position += 1;
    }
    if position == start || position != raw.len() {
        return None;
    }
    let signed = if negative {
        -(i128::from(magnitude))
    } else {
        i128::from(magnitude)
    };
    Some(signed as i32)
}

/// Git's `calc_shared_perm` for a file (not a directory) mode.
#[cfg(any(unix, test))]
fn adjusted_index_permissions(mode: u32, shared: i32) -> u32 {
    let base = mode & 0o7777;
    if shared == 0 {
        return base;
    }
    let mut tweak = shared.unsigned_abs();
    if base & 0o200 == 0 {
        tweak &= !0o222;
    }
    if base & 0o100 != 0 {
        tweak |= (tweak & 0o444) >> 2;
    }
    let adjusted = if shared < 0 {
        (base & !0o777) | tweak
    } else {
        base | tweak
    };
    adjusted & 0o7777
}

#[cfg(unix)]
fn set_index_permissions(file: &File, shared: i32) -> Result<(), std::io::Error> {
    use std::os::unix::fs::PermissionsExt;
    let base = file.metadata()?.permissions().mode() & 0o7777;
    let adjusted = adjusted_index_permissions(base, shared);
    if adjusted != base {
        file.set_permissions(fs::Permissions::from_mode(adjusted))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn set_index_permissions(_: &File, _: i32) -> Result<(), std::io::Error> {
    Ok(())
}

/// Atomically install `bytes` as the index at one physical endpoint.
///
/// The temporary sibling is created with git's own create-new mode (`0666`
/// masked by the process umask) — not an authored mode, not a copy of the old
/// index's mode and not `NamedTempFile`'s `0600` — then adjusted through git's
/// own `core.sharedRepository` rule, exactly as git's `create_tempfile` does
/// before it renames a lock file. Same directory, so the replace is atomic and
/// never crosses a filesystem.
///
/// The created file's identity is held open for the whole write. Cleanup after
/// a write, permission or rename failure removes the temporary only when the
/// path still names this attempt's file; a foreign replacement, a symlink or a
/// vanished path is preserved (the error names the possible leftover), never
/// unlinked by name or guessed by size/mtime.
fn install_index_body(endpoint: &Path, bytes: &[u8], shared: i32) -> Result<(), GitError> {
    use std::sync::atomic::{AtomicU64, Ordering};
    let owner_pid = std::process::id();

    let directory = match endpoint.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    static NEXT_SERIAL: AtomicU64 = AtomicU64::new(0);
    let failed = |detail: String| GitError::SafeCommitIndexPublishFailed {
        path: endpoint.to_path_buf(),
        detail,
    };
    let mut created = None;
    for _ in 0..32 {
        let serial = NEXT_SERIAL.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let candidate = directory.join(format!(
            ".spec-kitty-index.{}.{nanos}.{serial}.tmp",
            std::process::id()
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => match same_file::Handle::from_file(file) {
                Ok(identity) => {
                    created = Some((candidate, identity));
                    break;
                }
                Err(source) => {
                    return Err(failed(format!(
                        "created the sibling index body at {} but its identity could not be captured: {source}; a possible leftover at that path is not safe to unlink by name alone",
                        candidate.display()
                    )));
                }
            },
            Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(source) => {
                return Err(failed(format!(
                    "could not create a sibling index body: {source}"
                )));
            }
        }
    }
    let Some((temporary, mut identity)) = created else {
        return Err(failed(
            "could not allocate a unique sibling index body".to_string(),
        ));
    };
    if std::process::id() != owner_pid {
        return Err(failed(format!(
            "the sibling {} was created by a different process; preserve its possible staged data",
            temporary.display()
        )));
    }
    let written = identity
        .as_file_mut()
        .write_all(bytes)
        .and_then(|()| identity.as_file_mut().flush())
        .and_then(|()| set_index_permissions(identity.as_file(), shared));
    let owns_temporary =
        || std::process::id() == owner_pid && path_is_identity(&temporary, &identity);
    if let Err(source) = written {
        let detail = format!("could not write the index body: {source}");
        if owns_temporary() {
            let _ = fs::remove_file(&temporary);
        }
        return Err(failed(detail));
    }
    if !owns_temporary() {
        return Err(failed(format!(
            "the sibling index body at {} no longer names this attempt's file; it is preserved, not unlinked",
            temporary.display()
        )));
    }
    if let Err(source) = fs::rename(&temporary, endpoint) {
        let detail = format!(
            "could not install the index body at {}: {source}",
            endpoint.display()
        );
        if owns_temporary() {
            let _ = fs::remove_file(&temporary);
        }
        return Err(failed(detail));
    }
    Ok(())
}

/// Write the ordinary route's message artifact exactly where git's own path
/// spelling points.
///
/// `OpenOptions` follows a symlink and truncates the target, so an authored
/// link is preserved, a dangling link is created through, a read-only target
/// fails, and a directory fails. No atomic rename or no-follow policy is
/// layered on top of git's own behaviour.
fn write_commit_artifact(path: &Path, bytes: &[u8]) -> Result<(), GitError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(|source| GitError::SafeCommitMessageFileUnwritable {
            path: path.to_path_buf(),
            detail: source.to_string(),
        })?;
    file.write_all(bytes)
        .and_then(|_| file.flush())
        .map_err(|source| GitError::SafeCommitMessageFileUnwritable {
            path: path.to_path_buf(),
            detail: source.to_string(),
        })
}

/// Read the artifact back, following symlinks as git does.
fn read_commit_artifact(path: &Path) -> Result<Vec<u8>, GitError> {
    let limit = crate::safe_commit::MAX_COMMIT_MESSAGE_LEN as u64 + 1;
    let file = File::open(path).map_err(|source| GitError::SafeCommitInvalidRequest {
        detail: format!("read commit message: {source}"),
    })?;
    let metadata = file
        .metadata()
        .map_err(|source| GitError::SafeCommitInvalidRequest {
            detail: format!("read commit message: {source}"),
        })?;
    if !metadata.is_file() {
        return Err(GitError::SafeCommitInvalidRequest {
            detail: "read commit message: the artifact is not a regular file".to_string(),
        });
    }
    let mut bytes = Vec::new();
    file.take(limit).read_to_end(&mut bytes).map_err(|source| {
        GitError::SafeCommitInvalidRequest {
            detail: format!("read commit message: {source}"),
        }
    })?;
    if bytes.len() as u64 > limit - 1 {
        return Err(GitError::SafeCommitInvalidRequest {
            detail: format!(
                "the commit message exceeds the {}-byte limit",
                crate::safe_commit::MAX_COMMIT_MESSAGE_LEN
            ),
        });
    }
    Ok(bytes)
}

/// Give a non-empty authored message the single trailing line feed git's
/// cleanup writes. An empty (whitespace-only) message stays zero bytes.
fn message_with_trailing_newline(mut bytes: Vec<u8>) -> Vec<u8> {
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        bytes.push(b'\n');
    }
    bytes
}

/// A private file holding the already-cleaned object body for `commit-tree`.
fn write_object_input(bytes: &[u8]) -> Result<NamedTempFile, GitError> {
    let mut file = NamedTempFile::new().map_err(|source| GitError::Io {
        context: "create the safe-commit object-input file",
        source,
    })?;
    file.write_all(bytes)
        .and_then(|_| file.flush())
        .map_err(|source| GitError::Io {
            context: "write the safe-commit object-input file",
            source,
        })?;
    Ok(file)
}

fn write_message_file(message: &[u8], pinned: bool) -> Result<NamedTempFile, GitError> {
    let mut file = NamedTempFile::new().map_err(|source| GitError::Io {
        context: "create the safe-commit message file",
        source,
    })?;
    let mut bytes = message.to_vec();
    if pinned || !bytes.ends_with(b"\n") {
        bytes.push(b'\n');
    }
    file.write_all(&bytes)
        .and_then(|_| file.flush())
        .map_err(|source| GitError::Io {
            context: "write the safe-commit message file",
            source,
        })?;
    Ok(file)
}

/// The first line of an exact message payload, without a trailing CR.
///
/// This mirrors `str::lines().next()` for valid UTF-8 and keeps raw bytes
/// untouched for payloads that are not text.
fn first_line_bytes(message: &[u8]) -> &[u8] {
    match message.iter().position(|byte| *byte == b'\n') {
        Some(end) => message[..end]
            .strip_suffix(b"\r")
            .unwrap_or(&message[..end]),
        None => message,
    }
}

/// The reflog/audit message for a payload.
///
/// UTF-8 truncation uses the scalar boundaries in [`reflog_message`]; other
/// payloads keep their exact bytes within the same 2000-byte budget. Git owns
/// whitespace normalization (including an empty recorded message); no invented
/// fallback, `U+FFFD`, or escaped `Debug` data replaces the pre-conversion bytes.
fn reflog_message_bytes(message: &[u8]) -> Vec<u8> {
    if let Ok(text) = std::str::from_utf8(message) {
        return reflog_message(text).into_bytes();
    }
    message.iter().take(2000).copied().collect()
}

/// Post-landing work that must know the commit exists.
fn finish_landed_commit(
    repository: &GitRepository,
    plan: &SafeCommitExecution<'_>,
    sha: &Oid,
    warnings: &mut Vec<String>,
) -> Result<Vec<GitPath>, GitError> {
    let process = repository.process();
    let requested = plan.requested_paths();

    // The ordinary `commit --only` route leaves the operator's requested paths
    // exactly as its prepare step left them: the reference never resets them to
    // the landed tree, so a stateful clean filter can legitimately leave the
    // real index on an earlier pass than the commit (an observable `M` on a
    // requested path). Only the pinned-parent and index-deletion routes keep
    // the preparatory reset/residue contract.
    if plan.route() != SafeCommitRoute::Worktree {
        if let Err(error) = resync_paths_with_retry(process, sha, &requested) {
            return Err(landed_error(
                plan,
                sha,
                SafeCommitLandedKind::IndexResyncFailed,
                format!(
                    "the real index could not be resynchronised for the committed paths: {error}"
                ),
                requested,
            ));
        }
        match staged_paths(process, IndexTarget::Operator, sha, &requested) {
            Ok(residue) if !residue.is_empty() => {
                return Err(landed_error(
                    plan,
                    sha,
                    SafeCommitLandedKind::IndexResidue,
                    "the real index still stages committed paths".to_string(),
                    residue,
                ));
            }
            Ok(_) => {}
            Err(error) => {
                return Err(landed_error(
                    plan,
                    sha,
                    SafeCommitLandedKind::IndexResidue,
                    format!("the real index state could not be read back: {error}"),
                    requested,
                ));
            }
        }
    }
    // `post-commit` runs after the ref moved and the index is reconciled.
    match run_hook(
        process,
        IndexTarget::Operator,
        "post-commit",
        &[],
        plan.route() != SafeCommitRoute::PinnedParent,
    ) {
        Ok(_) => {}
        Err(error) => warnings.push(format!(
            "the commit landed but its post-commit hook could not complete: {error}"
        )),
    }
    let committed = match commit_paths(process, sha) {
        Ok(committed) => committed,
        Err(error) => {
            return Err(landed_error(
                plan,
                sha,
                SafeCommitLandedKind::CommitPathReadbackFailed,
                format!("the commit's changed-path set could not be read back: {error}"),
                requested,
            ));
        }
    };
    let extras = extra_paths(&committed, &requested);
    if !extras.is_empty() {
        let detail = match resync_paths_with_retry(process, sha, &extras) {
            Ok(()) => {
                "a commit hook staged them into the commit; their index entries were resynchronised to the landed commit".to_string()
            }
            Err(error) => format!(
                "a commit hook staged them into the commit and their index entries could not be resynchronised: {error}"
            ),
        };
        return Err(landed_error(
            plan,
            sha,
            SafeCommitLandedKind::UnrequestedPaths,
            detail,
            extras,
        ));
    }

    Ok(committed)
}

// ---------------------------------------------------------------------------
// Shared path-scoped index helpers
// ---------------------------------------------------------------------------

/// Seed the private index from the commit's baseline and return the tree that
/// baseline stands for.
///
/// A born destination seeds from the parent commit. An **unborn** destination
/// seeds from git's own canonical empty tree (`read-tree --empty` then
/// `write-tree`), so the object id is whatever the repository's hash algorithm
/// produces — never a hardcoded empty-object constant.
fn seed_index(
    process: &GitProcess,
    target: IndexTarget<'_>,
    parent: Option<&Oid>,
) -> Result<Oid, GitError> {
    match parent {
        Some(parent) => {
            target.checked(process, &["read-tree", parent.as_str()])?;
            tree_of(process, parent)
        }
        None => {
            target.checked(process, &["read-tree", "--empty"])?;
            write_tree(process, target)
        }
    }
}

/// Stage exactly the requested paths (and index-only removals) in the private
/// index. Nothing the operator staged is read.
fn stage_into(
    process: &GitProcess,
    target: IndexTarget<'_>,
    plan: &SafeCommitExecution<'_>,
) -> Result<(), GitError> {
    stage_paths(process, target, plan.paths())
        .map_err(|(path, detail)| GitError::SafeCommitStageFailed { path, detail })?;
    remove_cached_paths(process, target, plan.index_deletions())
}

fn seed_stage_and_write_tree(
    process: &GitProcess,
    target: IndexTarget<'_>,
    parent: Option<&Oid>,
    plan: &SafeCommitExecution<'_>,
) -> Result<Oid, GitError> {
    seed_index(process, target, parent)?;
    stage_into(process, target, plan)?;
    write_tree(process, target)
}

fn write_tree(process: &GitProcess, target: IndexTarget<'_>) -> Result<Oid, GitError> {
    let output = target.checked(process, &["write-tree"])?;
    let text = utf8_field(strip_line_feed(&output.stdout), "tree id")?;
    Oid::parse(text.trim())
}

fn stage_paths(
    process: &GitProcess,
    target: IndexTarget<'_>,
    paths: &[GitPath],
) -> Result<(), (std::path::PathBuf, String)> {
    for path in paths {
        let args = [
            OsString::from("--literal-pathspecs"),
            OsString::from("add"),
            OsString::from("--force"),
            OsString::from("--"),
            path.to_os_string(),
        ];
        match target.run(process, &args) {
            Ok(output) if output.status.success() => {}
            Ok(output) => {
                let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
                return Err((
                    path.to_path_buf(),
                    if detail.is_empty() {
                        "git add failed with no diagnostic".to_string()
                    } else {
                        detail
                    },
                ));
            }
            Err(error) => return Err((path.to_path_buf(), error.to_string())),
        }
    }
    Ok(())
}

fn remove_cached_paths(
    process: &GitProcess,
    target: IndexTarget<'_>,
    paths: &[GitPath],
) -> Result<(), GitError> {
    if paths.is_empty() {
        return Ok(());
    }
    let mut args = base(["rm", "--cached", "--quiet", "--ignore-unmatch"]);
    push_paths(&mut args, paths);
    target.checked(process, &args)?;
    Ok(())
}

fn staged_paths(
    process: &GitProcess,
    target: IndexTarget<'_>,
    commit: &Oid,
    paths: &[GitPath],
) -> Result<Vec<GitPath>, GitError> {
    let mut args = base([
        "diff",
        "--cached",
        "--name-only",
        "-z",
        "--no-renames",
        commit.as_str(),
    ]);
    if !paths.is_empty() {
        push_paths(&mut args, paths);
    }
    let output = target.checked(process, &args)?;
    parse_paths_z(&output.stdout)
}

/// Synchronise exactly these index entries to the immutable landed commit.
///
/// `git reset -- <paths>` uses the landed commit (never a re-read HEAD), which is
/// what keeps the reconciliation correct when another writer advanced the branch
/// again after our compare-and-swap.
fn resync_paths(
    process: &GitProcess,
    target: IndexTarget<'_>,
    commit: &Oid,
    paths: &[GitPath],
) -> Result<(), GitError> {
    if paths.is_empty() {
        return Ok(());
    }
    let mut args = base(["reset", "--quiet", commit.as_str()]);
    push_paths(&mut args, paths);
    target.checked(process, &args)?;
    Ok(())
}

/// Post-landing index reconciliation, bounded-retry on another writer's lock.
///
/// The commit already exists when this runs, so failing because a *different*
/// writer holds git's index lock would report a landed commit as needing repair
/// for a transient condition. Every attempt writes the same idempotent result
/// (these index entries become the landed commit's), touches no other path, and never
/// removes the other writer's lock. A lock held for longer than the bounded
/// window still surfaces as [`GitError::SafeCommitLanded`] with repair evidence.
fn resync_paths_with_retry(
    process: &GitProcess,
    commit: &Oid,
    paths: &[GitPath],
) -> Result<(), GitError> {
    let mut last = None;
    for attempt in 1..=RESYNC_ATTEMPTS {
        match resync_paths(process, IndexTarget::Operator, commit, paths) {
            Ok(()) => return Ok(()),
            Err(error) if index_lock_contention(&error) && attempt < RESYNC_ATTEMPTS => {
                last = Some(error);
                thread::sleep(RESYNC_PAUSE);
            }
            Err(error) => return Err(error),
        }
    }
    Err(last.expect("the retry loop records the last contention error"))
}

fn index_lock_contention(error: &GitError) -> bool {
    match error {
        GitError::CommandFailed { stderr, .. } => {
            stderr.contains("index.lock") || stderr.contains("Unable to create")
        }
        _ => false,
    }
}

fn commit_paths(process: &GitProcess, sha: &Oid) -> Result<Vec<GitPath>, GitError> {
    let output = process.checked(&[
        "-c",
        "diff.relative=false",
        "show",
        "--name-only",
        "--format=",
        "-z",
        "--no-renames",
        sha.as_str(),
    ])?;
    parse_paths_z(&output.stdout)
}

fn parse_paths_z(raw: &[u8]) -> Result<Vec<GitPath>, GitError> {
    let mut paths = Vec::new();
    for record in raw.split(|byte| *byte == 0) {
        if record.is_empty() {
            continue;
        }
        paths.push(GitPath::from_git_bytes(record)?);
    }
    Ok(paths)
}

/// Base argv every path-touching command shares: literal pathspecs first, so a
/// path that looks like pathspec magic stays data.
fn base<const N: usize>(args: [&str; N]) -> Vec<OsString> {
    let mut argv = Vec::with_capacity(N + 4);
    argv.push(OsString::from("--literal-pathspecs"));
    for arg in args {
        argv.push(OsString::from(arg));
    }
    argv
}

fn push_paths(args: &mut Vec<OsString>, paths: &[GitPath]) {
    args.push(OsString::from("--"));
    args.extend(paths.iter().map(GitPath::to_os_string));
}

fn combined_output(output: &CommandOutput) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}\n{stderr}");
    let trimmed = combined.trim();
    if trimmed.is_empty() {
        "git produced no diagnostic".to_string()
    } else {
        trimmed.to_string()
    }
}

fn extra_paths(committed: &[GitPath], requested: &[GitPath]) -> Vec<GitPath> {
    let mut extras: Vec<GitPath> = committed
        .iter()
        .filter(|path| !requested.contains(path))
        .cloned()
        .collect();
    extras.sort();
    extras.dedup();
    extras
}

fn landed_error(
    plan: &SafeCommitExecution<'_>,
    sha: &Oid,
    kind: SafeCommitLandedKind,
    detail: String,
    affected: Vec<GitPath>,
) -> GitError {
    GitError::SafeCommitLanded {
        kind,
        destination: plan.destination_label().to_string(),
        worktree_root: plan.worktree_root().to_path_buf(),
        detail,
        recovery: Box::new(SafeCommitRecovery::new(sha.clone(), affected)),
    }
}

fn outcome(
    plan: &SafeCommitExecution<'_>,
    commit: Oid,
    committed_paths: Vec<GitPath>,
    warnings: Vec<String>,
) -> SafeCommitCoreOutcome {
    SafeCommitCoreOutcome::new(
        commit,
        plan.is_unborn(),
        plan.worktree_root().to_path_buf(),
        committed_paths,
        warnings,
    )
}

// ---------------------------------------------------------------------------
// Tree inspection, hooks, and commit-object creation
// ---------------------------------------------------------------------------

fn tree_of(process: &GitProcess, commit: &Oid) -> Result<Oid, GitError> {
    let output = process.checked(&["show", "-s", "--format=%T", commit.as_str()])?;
    let text = utf8_field(strip_line_feed(&output.stdout), "tree id")?;
    Oid::parse(text.trim())
}

fn tree_has_path(
    process: &GitProcess,
    target: IndexTarget<'_>,
    tree: &Oid,
    path: &GitPath,
) -> Result<bool, GitError> {
    let output = target.checked(
        process,
        &[
            OsStr::new("--literal-pathspecs"),
            OsStr::new("ls-tree"),
            OsStr::new("--full-tree"),
            OsStr::new("-z"),
            OsStr::new(tree.as_str()),
            OsStr::new("--"),
            path.as_os_str(),
        ],
    )?;
    let mut records = output.stdout.split(|byte| *byte == 0);
    Ok(records.any(|record| !record.is_empty()))
}

fn blob_at(
    process: &GitProcess,
    target: IndexTarget<'_>,
    tree: &Oid,
    path: &GitPath,
) -> Result<Oid, GitError> {
    let mut spec = OsString::from(tree.as_str());
    spec.push(":");
    spec.push(path.as_os_str());
    let output = target.checked(
        process,
        &[
            OsString::from("rev-parse"),
            OsString::from("--verify"),
            spec,
        ],
    )?;
    let text = utf8_field(strip_line_feed(&output.stdout), "blob id")?;
    Oid::parse(text.trim())
}

fn hash_bytes(
    process: &GitProcess,
    target: IndexTarget<'_>,
    bytes: &[u8],
) -> Result<Oid, GitError> {
    let output =
        target.checked_with_stdin(process, &["hash-object", "--stdin", "--no-filters"], bytes)?;
    let text = utf8_field(strip_line_feed(&output.stdout), "blob id")?;
    Oid::parse(text.trim())
}

fn verify_expected_bytes(
    process: &GitProcess,
    target: IndexTarget<'_>,
    tree: &Oid,
    expected: &[ExpectedBlob],
) -> Result<(), GitError> {
    for blob in expected {
        let actual = blob_at(process, target, tree, blob.path()).map_err(|error| {
            GitError::SafeCommitExpectedBytesMismatch {
                path: blob.path().display_lossy(),
                detail: format!("the staged blob could not be inspected: {error}"),
            }
        })?;
        let expected_oid = hash_bytes(process, target, blob.bytes())?;
        if actual != expected_oid {
            return Err(GitError::SafeCommitExpectedBytesMismatch {
                path: blob.path().display_lossy(),
                detail: format!(
                    "staged blob {actual} differs from the hash of the expected raw bytes {expected_oid}"
                ),
            });
        }
    }
    Ok(())
}

fn run_commit_hook(
    process: &GitProcess,
    target: IndexTarget<'_>,
    hook: &str,
    extra: &[&OsStr],
    no_editor: bool,
) -> Result<CommandOutput, GitError> {
    run_hook(process, target, hook, extra, no_editor)
}

/// A hook's non-empty stderr on an otherwise successful run is an operator
/// warning (upstream's warn-mode commit guard prints there and exits 0).
fn record_hook_warnings(warnings: &mut Vec<String>, hook: &str, output: CommandOutput) {
    if output.stderr.is_empty() {
        return;
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stderr.is_empty() {
        warnings.push(format!(
            "{hook} hook reported on a successful commit: {stderr}"
        ));
    }
}

/// Run one hook through `git hook run` (never by executing the hook file
/// ourselves) and reject a non-zero result.
fn run_hook(
    process: &GitProcess,
    target: IndexTarget<'_>,
    hook: &str,
    extra: &[&OsStr],
    no_editor: bool,
) -> Result<CommandOutput, GitError> {
    let mut args: Vec<OsString> = vec![
        OsString::from("hook"),
        OsString::from("run"),
        OsString::from("--ignore-missing"),
        OsString::from(hook),
    ];
    if !extra.is_empty() {
        args.push(OsString::from("--"));
        args.extend(extra.iter().map(|value| value.to_os_string()));
    }
    let index = match target {
        IndexTarget::Operator => None,
        IndexTarget::Scoped(index) => Some(index),
    };
    let output = process.run_commit_hook(&args, index, no_editor)?;
    if output.status.success() {
        return Ok(output);
    }
    Err(GitError::SafeCommitHookRejected {
        hook: hook.to_string(),
        detail: combined_output(&output),
    })
}

/// Create the commit object from the candidate tree.
///
/// `parent` is `None` for a genuinely unborn destination, and then no `-p` is
/// passed at all: `git commit-tree <tree>` creates the zero-parent root commit.
fn create_commit_object(
    process: &GitProcess,
    target: IndexTarget<'_>,
    tree: &Oid,
    parent: Option<&Oid>,
    message_path: &Path,
    honor_signing: bool,
    plan: &SafeCommitExecution<'_>,
) -> Result<(Oid, Vec<String>), GitError> {
    let destination = plan.destination_label();
    let mut warnings = Vec::new();
    let mut args: Vec<OsString> = vec![OsString::from("commit-tree")];
    if honor_signing && commit_is_signed(process)? {
        args.push(OsString::from("-S"));
    }
    args.push(OsString::from(tree.as_str()));
    if let Some(parent) = parent {
        args.push(OsString::from("-p"));
        args.push(OsString::from(parent.as_str()));
    }
    args.push(OsString::from("-F"));
    args.push(message_path.as_os_str().to_os_string());
    let output = target.run(process, &args)?;
    if !output.status.success() {
        return Err(GitError::SafeCommitCommitFailed {
            destination: destination.to_owned(),
            status: output.status.code(),
            detail: combined_output(&output),
        });
    }
    if !output.stderr.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if !stderr.is_empty() {
            warnings.push(format!(
                "commit-tree reported on a successful commit: {stderr}"
            ));
        }
    }
    let text = utf8_field(strip_line_feed(&output.stdout), "commit id")?;
    Ok((Oid::parse(text.trim())?, warnings))
}

/// Translate `commit.gpgsign` into `commit-tree`'s explicit option, exactly as
/// upstream's pinned-parent route does (ordinary routes force unsigned): unset means unsigned, a non-boolean
/// value is a hard refusal.
fn commit_is_signed(process: &GitProcess) -> Result<bool, GitError> {
    let output = process.allowing(&["config", "--bool", "--get", "commit.gpgsign"], &[1])?;
    if output.status.code() == Some(1) {
        return Ok(false);
    }
    Ok(strip_line_feed(&output.stdout) == b"true")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn acquiring_the_lock_needs_no_duplicate_descriptor() {
        const CHILD: &str = "SK_INDEX_LOCK_DESCRIPTOR_CHILD";
        if let Some(path) = std::env::var_os(CHILD) {
            assert_eq!(
                std::env::current_dir().unwrap(),
                PathBuf::from(std::env::var_os("HOME").unwrap())
            );
            let mut fillers = Vec::new();
            loop {
                match File::open("/dev/null") {
                    Ok(file) => fillers.push(file),
                    Err(error) => {
                        assert_eq!(
                            error.raw_os_error(),
                            Some(24),
                            "bounded child reaches EMFILE"
                        );
                        break;
                    }
                }
            }
            drop(
                fillers
                    .pop()
                    .expect("leave exactly one descriptor for create_new"),
            );
            let path = PathBuf::from(path);
            let result = OperatorIndexLock::acquire(path.clone(), path.clone(), 0);
            drop(fillers); // permit the normal path-identity check during Drop
            let lock = result.expect("identity must use the created descriptor, not dup it");
            lock.ensure_owned().unwrap();
            drop(lock);
            assert!(!path.exists(), "the acquired lock is released normally");
            return;
        }
        let sandbox = tempfile::tempdir().unwrap();
        let home = sandbox.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let mut child = std::process::Command::new("prlimit");
        child
            .args(["--nofile=64:64", "--"])
            .arg(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "safe_commit_transaction::tests::acquiring_the_lock_needs_no_duplicate_descriptor",
                "--nocapture",
            ])
            .current_dir(&home)
            .env_clear()
            .env(
                "PATH",
                std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
            )
            .env("HOME", &home)
            .env(CHILD, sandbox.path().join("index.lock"));
        for variable in [
            "TMPDIR",
            "XDG_CONFIG_HOME",
            "XDG_CACHE_HOME",
            "XDG_DATA_HOME",
            "XDG_STATE_HOME",
        ] {
            let directory = sandbox.path().join(variable);
            fs::create_dir_all(&directory).unwrap();
            child.env(variable, directory);
        }
        let result = child
            .output()
            .expect("Linux FD regression requires prlimit");
        assert!(
            result.status.success(),
            "FD regression child: {} {}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }

    #[test]
    fn owned_index_lock_identity_survives_our_body_write() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("index.lock");
        let mut lock = OperatorIndexLock::acquire(path.clone(), path.clone(), 0).unwrap();
        lock.write_body(b"owned identity test body").unwrap();
        lock.ensure_owned().unwrap();
        drop(lock);
        assert!(!path.exists());
    }

    #[test]
    fn an_existing_foreign_lock_is_never_owned_or_removed() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("index.lock");
        fs::write(&path, b"foreign").unwrap();
        assert!(matches!(
            OperatorIndexLock::acquire(path.clone(), path.clone(), 0),
            Err(GitError::SafeCommitIndexLocked { .. })
        ));
        assert_eq!(fs::read(path).unwrap(), b"foreign");
    }

    #[test]
    fn an_inherited_pid_cannot_release_the_owners_index_lock() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("index.lock");
        let mut lock = OperatorIndexLock::acquire(path.clone(), path.clone(), 0).unwrap();
        lock.write_body(b"owner").unwrap();
        // Deterministic ownership-fence control, NOT a real-fork certification.
        lock.pid = std::process::id().wrapping_add(1);
        assert!(lock.ensure_owned().is_err());
        drop(lock);
        assert_eq!(fs::read(path).unwrap(), b"owner");
    }

    #[test]
    fn same_contents_and_size_cannot_identify_a_replacement_as_ours() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("index.lock");
        let original = temp.path().join("original-lock");
        let mut lock = OperatorIndexLock::acquire(path.clone(), path.clone(), 0).unwrap();
        lock.write_body(b"same bytes").unwrap();
        fs::rename(&path, &original).unwrap();
        fs::write(&path, b"same bytes").unwrap();
        assert!(lock.ensure_owned().is_err());
        drop(lock);
        assert_eq!(fs::read(path).unwrap(), b"same bytes");
        assert_eq!(fs::read(original).unwrap(), b"same bytes");
    }

    #[test]
    fn index_body_install_replaces_the_endpoint_and_leaves_no_sibling() {
        let temp = tempfile::tempdir().unwrap();
        let endpoint = temp.path().join("index");
        fs::write(&endpoint, b"old body").unwrap();
        install_index_body(&endpoint, b"new body", 0).unwrap();
        assert_eq!(fs::read(&endpoint).unwrap(), b"new body");
        let names: Vec<_> = fs::read_dir(temp.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("index")]);
    }

    #[cfg(unix)]
    #[test]
    fn index_body_install_applies_the_shared_repository_mode() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let endpoint = temp.path().join("index");
        install_index_body(&endpoint, b"body", -0o660).unwrap();
        assert_eq!(
            fs::metadata(&endpoint).unwrap().permissions().mode() & 0o777,
            0o660
        );
    }

    #[test]
    fn a_replaced_temporary_is_not_this_attempts_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("body.tmp");
        fs::write(&path, b"same bytes").unwrap();
        let identity = same_file::Handle::from_path(&path).unwrap();
        assert!(path_is_identity(&path, &identity));
        let original = temp.path().join("original.tmp");
        fs::rename(&path, &original).unwrap();
        fs::write(&path, b"same bytes").unwrap();
        assert!(
            !path_is_identity(&path, &identity),
            "equal size and bytes on a replaced inode are not ownership"
        );
        let link = temp.path().join("link.tmp");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&original, &link).unwrap();
        #[cfg(not(unix))]
        fs::copy(&original, &link).unwrap();
        assert!(
            !path_is_identity(&link, &identity),
            "a symlink to our file is not the owned path"
        );
    }

    #[test]
    fn shared_repository_modes_follow_git_calc_shared_perm() {
        // Base modes stand in for different process umasks: 0644 (022),
        // 0640 (027), 0600 (077), 0666 (000). The adjustment is git's
        // `calc_shared_perm` (path.c) for a regular file.
        let cases = [
            (0o644, 0, 0o644),
            (0o600, 0, 0o600),
            (0o644, 0o660, 0o664),
            (0o640, 0o660, 0o660),
            (0o600, 0o660, 0o660),
            (0o644, 0o664, 0o664),
            (0o600, 0o664, 0o664),
            (0o640, -0o660, 0o660),
            (0o600, -0o660, 0o660),
            (0o644, -0o666, 0o666),
            (0o666, -0o660, 0o660),
        ];
        for (base, shared, expected) in cases {
            assert_eq!(
                adjusted_index_permissions(base, shared),
                expected,
                "base {base:04o}, shared {shared}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn permissions_use_the_held_file_not_a_replaced_temporary_name() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("temporary");
        let retained = root.path().join("our-held-file");
        let file = File::create(&path).unwrap();
        fs::rename(&path, &retained).unwrap();
        fs::write(&path, b"foreign").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        set_index_permissions(&file, -0o666).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(fs::read(&path).unwrap(), b"foreign");
        assert_eq!(
            fs::metadata(retained).unwrap().permissions().mode() & 0o777,
            0o666
        );
    }

    #[test]
    fn shared_repository_config_values_mirror_git_config_perm() {
        assert_eq!(parse_shared_repository(b"").unwrap(), 0);
        assert_eq!(parse_shared_repository(b"umask").unwrap(), 0);
        assert_eq!(parse_shared_repository(b"group").unwrap(), 0o660);
        assert_eq!(parse_shared_repository(b"all").unwrap(), 0o664);
        assert_eq!(parse_shared_repository(b"true").unwrap(), 0o660);
        assert_eq!(parse_shared_repository(b"false").unwrap(), 0);
        assert_eq!(parse_shared_repository(b"0660").unwrap(), -0o660);
        assert_eq!(parse_shared_repository(b"0666").unwrap(), -0o666);
        assert_eq!(parse_shared_repository(b"1").unwrap(), 0o660);
        assert_eq!(parse_shared_repository(b"2").unwrap(), 0o664);
        assert_eq!(parse_shared_repository(b"0600").unwrap(), -0o600);
        assert_eq!(parse_shared_repository(b"+0660").unwrap(), -0o660);
        assert_eq!(parse_shared_repository(b" \t0660").unwrap(), -0o660);
        assert_eq!(parse_shared_repository(b"-1").unwrap(), -0o666);
        assert_eq!(
            parse_shared_repository(b"0777777777777777777777777777777777").unwrap(),
            -0o666
        );
        assert!(parse_shared_repository(b"not-a-mode").is_err());
    }

    #[test]
    fn the_first_line_mirrors_str_lines_for_utf8() {
        assert_eq!(first_line_bytes(b"subject\nbody\n"), b"subject");
        assert_eq!(first_line_bytes(b"subject\r\nbody"), b"subject");
        assert_eq!(first_line_bytes(b"subject"), b"subject");
        assert_eq!(
            first_line_bytes(b"subject\r"),
            b"subject\r",
            "a lone trailing CR is not a line ending for str::lines either"
        );
        assert_eq!(first_line_bytes(b"\nbody"), b"");
    }

    #[test]
    fn utf8_reflog_bytes_use_the_canonical_text_truncation() {
        for text in [
            "commit: op(\u{00a0}): a",
            "commit: subject",
            "pinned message\n\nbody",
            "   ",
            "",
        ] {
            assert_eq!(
                reflog_message_bytes(text.as_bytes()),
                reflog_message(text).into_bytes(),
                "the UTF-8 and byte routes share text truncation, including whitespace"
            );
        }
    }

    #[test]
    fn raw_reflog_bytes_are_exact_and_bounded() {
        assert_eq!(
            reflog_message_bytes(b"commit: op(\x80): rewritten"),
            b"commit: op(\x80): rewritten".to_vec()
        );
        assert_eq!(reflog_message_bytes(b" \t "), b" \t ".to_vec());
        let long = vec![0xffu8; 2_100];
        assert_eq!(reflog_message_bytes(&long), vec![0xffu8; 2_000]);
    }
}
