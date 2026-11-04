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
//! # The path-scoped index transaction
//!
//! Both routes build the candidate tree in a private index seeded from a pinned
//! parent (`HEAD` for an ordinary commit, the caller's exact parent for the
//! pinned-parent route):
//!
//! 1. `git read-tree <parent>`, then stage exactly the requested paths
//!    (`git add --force`, or `git rm --cached --ignore-unmatch` for index-only
//!    removals). Nothing the operator staged in the real index is read, so
//!    nothing unrelated can enter the commit. An **unborn** destination (the
//!    exact-ref route with a genuinely absent ref) seeds from git's own
//!    canonical empty tree with `git read-tree --empty`, so no parent object and
//!    no hardcoded empty-tree id is involved.
//! 2. Prepare the route's configured/verbatim commit-message pipeline.
//! 3. Run the repository's own commit-message hooks before no-op checks through `git hook run` with
//!    `GIT_INDEX_FILE` pointing at the private index. Never `--no-verify`, and
//!    never by executing the hook file directly.
//! 4. Write the tree and create the commit object with `git commit-tree`,
//!    whose parent is the pinned parent. The commit object therefore carries
//!    exactly the index content that was built against that parent. For an
//!    unborn destination no `-p` is passed at all: the object is a zero-parent
//!    root commit.
//! 5. Advance the destination branch with a compare-and-swap that requires the
//!    pinned parent; for an unborn destination the compare-and-swap requires the
//!    ref to be **absent** (`RefChange::create`), so a ref that appeared after
//!    planning is refused rather than overwritten. A concurrent writer wins and
//!    the whole commit is refused with candidate/readback evidence if advancement
//!    is unconfirmed. Hooks may
//!    themselves change history; no blind retry or user-state reset follows. This is what makes the transaction safe
//!    under contention: a commit built against an old `HEAD` can never land on a
//!    branch that has moved on.
//! 6. Only now touch the real index: `git reset --quiet <landed-sha> -- <paths>` for
//!    exactly the requested paths, then verify there is no residue. A failure
//!    here is [`GitError::SafeCommitLanded`] — the commit exists and is reachable,
//!    and the affected paths plus the repair argv are reported.
//! 7. Read the commit's own changed-path set and refuse a commit carrying extra
//!    paths (a pre-commit hook that stages tracked files). Extras are
//!    resynchronised to the landed commit first.
//!
//! The pinned-parent route adds hook isolation for callers that asserted exact
//! raw bytes: the tree is written before the hooks, rebuilt after them, and must
//! be identical, so no hook can add paths or change requested content.
//!
//! No route retries a commit. No route resets, deletes, or stashes user state it
//! was not asked about. A pre-landing refusal leaves the real index exactly as
//! found; the private index is a temporary file removed on drop.

use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use tempfile::NamedTempFile;

use crate::error::{GitError, SafeCommitLandedKind};
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

pub(crate) fn execute(
    repository: &GitRepository,
    plan: &SafeCommitExecution<'_>,
) -> Result<SafeCommitCoreOutcome, GitError> {
    if plan.worktree_root() != repository.root() {
        return Err(GitError::InvalidPlan {
            detail: format!(
                "safe-commit plan was prepared for {} but applied to {}",
                plan.worktree_root().display(),
                repository.root().display()
            ),
        });
    }
    #[cfg(not(unix))]
    if std::str::from_utf8(plan.message_bytes()).is_err() {
        return Err(GitError::Unsupported {
            feature: "a non-UTF-8 commit message reflog on this platform",
        });
    }
    repository.process().ensure_unredirected_write()?;
    ensure_index_authority(repository, plan)?;
    recheck_plan(repository, plan)?;
    repository.require_no_commit_operation()?;
    let mut warnings = Vec::new();
    let sha = create_commit(repository, plan, &mut warnings)?;
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
fn create_commit(
    repository: &GitRepository,
    plan: &SafeCommitExecution<'_>,
    warnings: &mut Vec<String>,
) -> Result<Oid, GitError> {
    let process = repository.process();
    if repository.git_version()? < (2, 36, 0) {
        return Err(GitError::Unsupported {
            feature: "git hook run (requires git >= 2.36) for a safe commit",
        });
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
            });
        }
        if !checkouts.iter().any(|path| path == plan.worktree_root()) {
            return Err(GitError::PlanStale {
                what: branch.as_ref_name().to_string(),
                detail: format!(
                    "the destination is not checked out in the worktree safe commit reconciles at {}",
                    plan.worktree_root().display()
                ),
            });
        }
    }

    let worktree_route = plan.route() == SafeCommitRoute::Worktree;
    let index = ScopedIndex::in_git_dir(repository.git_dirs()?.git_dir())?;
    let target = IndexTarget::Scoped(&index);
    let cleanup = MessageCleanup::resolve(process, isolate_hooks)?;

    // The ordinary route holds git's own lock for the captured operator index
    // while it writes the shared COMMIT_EDITMSG and runs the hooks. The lock
    // body is a valid prospective index (requested blobs plus whatever the
    // operator had staged), exactly the file git's own `commit --only` exposes
    // to hooks. It is built in a private copy and never mutates the operator
    // index.
    let mut artifact_lock: Option<OperatorIndexLock> = None;
    let commit_message = if worktree_route {
        let message_path = repository.commit_message_path()?;
        let mut lock = OperatorIndexLock::acquire(repository.operator_index_lock_path()?)?;
        let body = prospective_operator_index(repository, process, &index, plan)?;
        lock.write_body(&body)?;
        artifact_lock = Some(lock);
        Some(message_path)
    } else {
        None
    };
    let message_file = if worktree_route {
        None
    } else {
        Some(write_message_file(plan.message_bytes(), isolate_hooks)?)
    };
    let message_path = match &commit_message {
        Some(path) => path.absolute().to_path_buf(),
        None => message_file
            .as_ref()
            .expect("a non-worktree route writes a private message file")
            .path()
            .to_path_buf(),
    };

    // The baseline is the parent's tree, or git's own canonical empty tree for
    // a root commit, so the initially-empty and no-op checks below are the same
    // rule for a born and an unborn destination.
    let baseline_tree = seed_index(process, target, parent)?;
    stage_into(process, target, plan)?;

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
        let initial = cleanup.render(process, target, plan.message_bytes())?;
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
    // The private index is no longer needed: the commit object exists.
    drop(index);

    // Git's own reflog subject for a commit: `commit: <subject>` normally,
    // `commit (initial): <subject>` for the first commit on a ref. The
    // pre-conversion payload bytes are what is recorded; nothing here writes
    // the object body or a lossy rendering.
    let audit_message = if isolate_hooks {
        plan.message_bytes().to_vec()
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
    // readback conclusion. It is released before the existing post-landing
    // index reconciliation, which needs the operator index's own lock.
    drop(artifact_lock.take());

    Ok(new_sha)
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
/// The file is git's canonical `<captured index>.lock`, not an advisory lock.
/// It is created only for the ordinary worktree route, after the transaction's
/// state checks and before any hook or artifact write, and held through the
/// ref compare-and-swap/readback. Cleanup removes the file only when it is the
/// same identity this process created and no replacement has taken the path.
struct OperatorIndexLock {
    path: PathBuf,
    // Keep the identity handle open: Windows file identifiers can be reused
    // after the last handle closes. No fabricated zero IDs or mtime heuristics.
    identity: same_file::Handle,
    pid: u32,
}

impl OperatorIndexLock {
    fn acquire(path: PathBuf) -> Result<Self, GitError> {
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
        Ok(Self {
            path,
            identity,
            pid: std::process::id(),
        })
    }

    fn owns_path(&self) -> bool {
        self.pid == std::process::id()
            && fs::symlink_metadata(&self.path)
                .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
            && same_file::Handle::from_path(&self.path)
                .is_ok_and(|observed| observed == self.identity)
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
            .write_all(bytes)
            .and_then(|_| self.identity.as_file_mut().flush())
            .map_err(|source| GitError::SafeCommitIndexLockUnavailable {
                path: self.path.clone(),
                detail: source.to_string(),
            })
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

/// Build the prospective operator index body in a private copy.
///
/// The operator's index is read and copied, never mutated. The requested paths
/// are staged into the copy with the same literal-pathspec `add --force` helper
/// the candidate tree uses, so unrelated staged entries survive and the lock
/// observable by hooks is a real DIRC index rather than an empty semaphore.
fn prospective_operator_index(
    repository: &GitRepository,
    process: &GitProcess,
    index: &ScopedIndex,
    plan: &SafeCommitExecution<'_>,
) -> Result<Vec<u8>, GitError> {
    let operator = repository.operator_index_path()?;
    match fs::read(&operator) {
        Ok(bytes) => fs::write(index.path(), bytes).map_err(|source| GitError::Io {
            context: "copy the operator index for the safe-commit lock",
            source,
        })?,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            IndexTarget::Scoped(index).checked(process, &["read-tree", "--empty"])?;
        }
        Err(source) => {
            return Err(GitError::Io {
                context: "read the operator index for the safe-commit lock",
                source,
            });
        }
    }
    // The source unstages the requested paths before re-adding them, so a
    // folded rename pair has an independently addressable index entry. This
    // runs only on the private copy; the operator's index is never touched.
    let target = IndexTarget::Scoped(index);
    if plan.head().is_some() {
        let mut args = base(["restore", "--staged"]);
        push_paths(&mut args, plan.paths());
        // Best-effort exactly like the source: a path with no staged change is
        // not an error for this preparatory step.
        let _ = target.run(process, &args);
    } else {
        let mut args = base(["rm", "--cached", "--ignore-unmatch", "-q"]);
        push_paths(&mut args, plan.paths());
        // The source also treats a failed unborn unstage as best effort and
        // lets the following `add --force` replace the entry.
        let _ = target.run(process, &args);
    }
    stage_paths(process, target, plan.paths())
        .map_err(|(path, detail)| GitError::SafeCommitStageFailed { path, detail })?;
    fs::read(index.path()).map_err(|source| GitError::Io {
        context: "read the prospective operator index body",
        source,
    })
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

    if let Err(error) = resync_paths_with_retry(process, sha, &requested) {
        return Err(landed_error(
            plan,
            sha,
            SafeCommitLandedKind::IndexResyncFailed,
            format!("the real index could not be resynchronised for the committed paths: {error}"),
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
            let result = OperatorIndexLock::acquire(path.clone());
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
        let mut lock = OperatorIndexLock::acquire(path.clone()).unwrap();
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
            OperatorIndexLock::acquire(path.clone()),
            Err(GitError::SafeCommitIndexLocked { .. })
        ));
        assert_eq!(fs::read(path).unwrap(), b"foreign");
    }

    #[test]
    fn an_inherited_pid_cannot_release_the_owners_index_lock() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("index.lock");
        let mut lock = OperatorIndexLock::acquire(path.clone()).unwrap();
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
        let mut lock = OperatorIndexLock::acquire(path.clone()).unwrap();
        lock.write_body(b"same bytes").unwrap();
        fs::rename(&path, &original).unwrap();
        fs::write(&path, b"same bytes").unwrap();
        assert!(lock.ensure_owned().is_err());
        drop(lock);
        assert_eq!(fs::read(path).unwrap(), b"same bytes");
        assert_eq!(fs::read(original).unwrap(), b"same bytes");
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
