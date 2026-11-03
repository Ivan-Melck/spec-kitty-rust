//! Mutation routes for [`SafeCommitPlan`](crate::SafeCommitPlan).
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
//!    nothing unrelated can enter the commit.
//! 2. Prepare the route's configured/verbatim commit-message pipeline.
//! 3. Run the repository's own commit-message hooks before no-op checks through `git hook run` with
//!    `GIT_INDEX_FILE` pointing at the private index. Never `--no-verify`, and
//!    never by executing the hook file directly.
//! 4. Write the tree and create the commit object with `git commit-tree`,
//!    whose parent is the pinned parent. The commit object therefore carries
//!    exactly the index content that was built against that parent.
//! 5. Advance the destination branch with a compare-and-swap that requires the
//!    pinned parent. A concurrent writer wins and the whole commit is refused
//!    with candidate/readback evidence if advancement is unconfirmed. Hooks may
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
use std::io::Write;
use std::path::Path;
use std::thread;
use std::time::Duration;

use tempfile::NamedTempFile;

use crate::error::{GitError, SafeCommitLandedKind};
use crate::oid::Oid;
use crate::path::{GitPath, strip_line_feed, utf8_field};
use crate::process::{CommandOutput, GitProcess, ScopedIndex};
use crate::refs::RefChange;
use crate::repository::GitRepository;
use crate::safe_commit::{
    ExpectedBlob, SafeCommitOutcome, SafeCommitPlan, SafeCommitRecovery, SafeCommitRoute,
    reflog_message,
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
    plan: &SafeCommitPlan,
) -> Result<SafeCommitOutcome, GitError> {
    repository.process().ensure_unredirected_write()?;
    recheck_plan(repository, plan)?;
    repository.require_no_commit_operation()?;
    let mut warnings = Vec::new();
    let sha = match plan.route() {
        SafeCommitRoute::Worktree | SafeCommitRoute::IndexDeletions => {
            create_commit(repository, plan, plan.head(), false, &mut warnings)?
        }
        SafeCommitRoute::PinnedParent => create_commit(
            repository,
            plan,
            plan.expected_parent().expect("pinned-parent route"),
            true,
            &mut warnings,
        )?,
    };
    let committed = finish_landed_commit(repository, plan, &sha, &mut warnings)?;
    Ok(outcome(plan, sha, committed, warnings))
}

/// Re-read the state the plan recorded, before any mutation.
fn recheck_plan(repository: &GitRepository, plan: &SafeCommitPlan) -> Result<(), GitError> {
    let observed = repository.current_branch()?;
    if observed.as_deref() != Some(plan.destination().as_str()) {
        return Err(GitError::SafeCommitHeadMismatch {
            destination: plan.destination().as_str().to_string(),
            observed_head: observed.unwrap_or_else(|| "<detached>".to_string()),
            worktree_root: plan.worktree_root().to_path_buf(),
        });
    }
    let tip = repository
        .read_branch_tip(plan.destination())?
        .ok_or_else(|| GitError::SafeCommitDestinationMissing {
            destination: plan.destination().as_str().to_string(),
            worktree_root: plan.worktree_root().to_path_buf(),
        })?;
    if &tip != plan.destination_tip() {
        return Err(GitError::PlanStale {
            what: plan.destination().as_ref_name().to_string(),
            detail: format!("planned tip {}, observed {tip}", plan.destination_tip()),
        });
    }
    let head_text =
        repository
            .head_commit()?
            .ok_or_else(|| GitError::SafeCommitDestinationMissing {
                destination: plan.destination().as_str().to_string(),
                worktree_root: plan.worktree_root().to_path_buf(),
            })?;
    let head = Oid::parse(&head_text)?;
    if &head != plan.head() {
        return Err(GitError::PlanStale {
            what: plan.destination().as_ref_name().to_string(),
            detail: format!("planned HEAD {}, observed {head}", plan.head()),
        });
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
fn create_commit(
    repository: &GitRepository,
    plan: &SafeCommitPlan,
    parent: &Oid,
    isolate_hooks: bool,
    warnings: &mut Vec<String>,
) -> Result<Oid, GitError> {
    let process = repository.process();
    if repository.git_version()? < (2, 36, 0) {
        return Err(GitError::Unsupported {
            feature: "git hook run (requires git >= 2.36) for a safe commit",
        });
    }

    if plan.route() == SafeCommitRoute::PinnedParent {
        let checkouts = repository.checkouts_of(plan.destination())?;
        if checkouts.len() > 1 {
            return Err(GitError::BranchCheckedOut {
                branch: plan.destination().as_str().to_string(),
                worktrees: checkouts,
            });
        }
        if !checkouts.iter().any(|path| path == plan.worktree_root()) {
            return Err(GitError::PlanStale {
                what: plan.destination().as_ref_name().to_string(),
                detail: format!(
                    "the destination is not checked out in the worktree safe commit reconciles at {}",
                    plan.worktree_root().display()
                ),
            });
        }
    }

    let index = ScopedIndex::in_git_dir(repository.git_dirs()?.git_dir())?;
    let target = IndexTarget::Scoped(&index);
    let message_file = write_message_file(plan.message(), isolate_hooks)?;
    let message_path = message_file.path().to_path_buf();
    let cleanup = MessageCleanup::resolve(process, isolate_hooks)?;

    let intended_tree = if isolate_hooks {
        Some(seed_stage_and_write_tree(process, target, parent, plan)?)
    } else {
        seed_and_stage(process, target, parent, plan)?;
        None
    };

    record_hook_warnings(
        warnings,
        "pre-commit",
        run_commit_hook(process, target, "pre-commit", &[])?,
    );
    cleanup.apply(process, target, &message_path)?;
    record_hook_warnings(
        warnings,
        "prepare-commit-msg",
        run_commit_hook(
            process,
            target,
            "prepare-commit-msg",
            &[message_path.as_os_str(), OsStr::new("message")],
        )?,
    );
    record_hook_warnings(
        warnings,
        "commit-msg",
        run_commit_hook(process, target, "commit-msg", &[message_path.as_os_str()])?,
    );

    let final_message = cleanup.apply(process, target, &message_path)?;

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

    if tree == tree_of(process, parent)? {
        return Err(GitError::SafeCommitNothingToCommit {
            destination: plan.destination().as_str().to_string(),
            detail: "the candidate tree equals the pinned parent's tree".to_string(),
        });
    }

    if final_message.is_empty() {
        return Err(GitError::SafeCommitCommitFailed {
            destination: plan.destination().as_str().to_owned(),
            status: None,
            detail: "Aborting commit due to empty commit message".to_owned(),
        });
    }
    let new_sha = create_commit_object(
        process,
        target,
        &tree,
        parent,
        &message_path,
        plan.destination().as_str(),
    )?;
    // The private index is no longer needed: the commit object exists.
    drop(index);

    let audit_message = if isolate_hooks {
        String::from_utf8_lossy(&final_message).into_owned()
    } else {
        let subject = process.checked(&["show", "-s", "--format=%s", new_sha.as_str()])?;
        format!(
            "commit: {}",
            String::from_utf8_lossy(strip_line_feed(&subject.stdout))
        )
    };
    let advance = repository
        .plan_ref_transaction(
            &reflog_message(&audit_message),
            vec![RefChange::set(
                plan.destination().as_ref_name(),
                new_sha.clone(),
                Some(parent.clone()),
            )],
        )
        .and_then(|transaction| repository.apply_ref_transaction(&transaction));
    if let Err(error) = advance {
        let observed = repository.read_branch_tip(plan.destination());
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
                destination: plan.destination().as_str().to_owned(),
                candidate: new_sha,
                observed_tip: observed.as_ref().ok().and_then(Clone::clone),
                detail: format!(
                    "expected parent {parent}; {error}; ref readback: {observed:?}; candidate reachability: {reachable:?}; hooks and concurrent writers may have changed history; do not assume nothing happened"
                ),
            });
        }
    }

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
        let cleaned = match self {
            Self::Verbatim => return Ok(raw),
            Self::Whitespace => {
                target
                    .checked_with_stdin(process, &["stripspace"], &raw)?
                    .stdout
            }
            Self::StripComments => {
                target
                    .checked_with_stdin(process, &["stripspace", "--strip-comments"], &raw)?
                    .stdout
            }
        };
        std::fs::write(path, &cleaned).map_err(|source| GitError::Io {
            context: "clean the safe-commit message",
            source,
        })?;
        Ok(cleaned)
    }
}

fn write_message_file(message: &str, pinned: bool) -> Result<NamedTempFile, GitError> {
    let mut file = NamedTempFile::new().map_err(|source| GitError::Io {
        context: "create the safe-commit message file",
        source,
    })?;
    let bytes = if pinned || !message.ends_with('\n') {
        format!("{message}\n")
    } else {
        message.to_owned()
    };
    file.write_all(bytes.as_bytes())
        .and_then(|_| file.flush())
        .map_err(|source| GitError::Io {
            context: "write the safe-commit message file",
            source,
        })?;
    Ok(file)
}

/// Post-landing work that must know the commit exists.
fn finish_landed_commit(
    repository: &GitRepository,
    plan: &SafeCommitPlan,
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
    match run_hook(process, IndexTarget::Operator, "post-commit", &[]) {
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

fn seed_and_stage(
    process: &GitProcess,
    target: IndexTarget<'_>,
    parent: &Oid,
    plan: &SafeCommitPlan,
) -> Result<(), GitError> {
    target.checked(process, &["read-tree", parent.as_str()])?;
    stage_paths(process, target, plan.paths())
        .map_err(|(path, detail)| GitError::SafeCommitStageFailed { path, detail })?;
    remove_cached_paths(process, target, plan.index_deletions())
}

fn seed_stage_and_write_tree(
    process: &GitProcess,
    target: IndexTarget<'_>,
    parent: &Oid,
    plan: &SafeCommitPlan,
) -> Result<Oid, GitError> {
    seed_and_stage(process, target, parent, plan)?;
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
    plan: &SafeCommitPlan,
    sha: &Oid,
    kind: SafeCommitLandedKind,
    detail: String,
    affected: Vec<GitPath>,
) -> GitError {
    GitError::SafeCommitLanded {
        kind,
        destination: plan.destination().as_str().to_string(),
        worktree_root: plan.worktree_root().to_path_buf(),
        detail,
        recovery: Box::new(SafeCommitRecovery::new(sha.clone(), affected)),
    }
}

fn outcome(
    plan: &SafeCommitPlan,
    commit: Oid,
    committed_paths: Vec<GitPath>,
    warnings: Vec<String>,
) -> SafeCommitOutcome {
    SafeCommitOutcome::new(
        commit,
        plan.destination().clone(),
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
) -> Result<CommandOutput, GitError> {
    run_hook(process, target, hook, extra)
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
    let output = target.run(process, &args)?;
    if output.status.success() {
        return Ok(output);
    }
    Err(GitError::SafeCommitHookRejected {
        hook: hook.to_string(),
        detail: combined_output(&output),
    })
}

fn create_commit_object(
    process: &GitProcess,
    target: IndexTarget<'_>,
    tree: &Oid,
    parent: &Oid,
    message_path: &Path,
    destination: &str,
) -> Result<Oid, GitError> {
    let mut args: Vec<OsString> = vec![OsString::from("commit-tree")];
    if commit_is_signed(process)? {
        args.push(OsString::from("-S"));
    }
    args.push(OsString::from(tree.as_str()));
    args.push(OsString::from("-p"));
    args.push(OsString::from(parent.as_str()));
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
    let text = utf8_field(strip_line_feed(&output.stdout), "commit id")?;
    Oid::parse(text.trim())
}

/// Translate `commit.gpgsign` into `commit-tree`'s explicit option, exactly as
/// upstream's ordinary and pinned-parent routes do: unset means unsigned, a non-boolean
/// value is a hard refusal.
fn commit_is_signed(process: &GitProcess) -> Result<bool, GitError> {
    let output = process.allowing(&["config", "--bool", "--get", "commit.gpgsign"], &[1])?;
    if output.status.code() == Some(1) {
        return Ok(false);
    }
    Ok(strip_line_feed(&output.stdout) == b"true")
}
