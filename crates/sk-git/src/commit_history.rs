//! Canonical commit-path history: which commits a range holds, and which paths
//! one commit changed.
//!
//! This module is the ONE native read of "what did this commit touch" and
//! "which commits are in this range". It exists because two consumers need the
//! same facts and must not each derive them:
//!
//! * [`crate::GitRepository::merge_base`] answers *whether* two branches share
//!   history;
//! * the expected-divergence classifier (`sk_workspace::coordination_divergence`)
//!   needs the commits unique to one side and, per commit, the changed paths.
//!
//! # Reference behaviour
//!
//! Upstream `kernel/git/listing.py::commit_paths` runs:
//!
//! ```text
//! git -c diff.relative=false show --name-only --format= -z --no-renames \
//!     [--first-parent -m] <commit>
//! ```
//!
//! and parses the `-z` records. [`commit_paths`] issues that same argv and the
//! same record split, so it inherits the reference's semantics rather than
//! approximating them:
//!
//! * `-c diff.relative=false` makes every printed path repository-root
//!   relative even when the process cwd is a subdirectory;
//! * `--no-renames` splits a rename into its delete and add paths, so both are
//!   classified;
//! * `--first-parent -m` makes a **merge commit** list what it brought in
//!   relative to its first parent. Without those flags git prints nothing for a
//!   merge (it shows a combined diff), which reads as "changed nothing" — the
//!   exact silent-empty mistake the reference calls out. `-m` also means a
//!   merge whose first-parent diff is genuinely empty returns no paths, and that
//!   answer is real rather than assumed;
//! * a **root commit** diffs against the empty tree, so it lists every file it
//!   introduced;
//! * an **empty commit** lists nothing, truthfully.
//!
//! Upstream `rev_list(repo, rev_range)` runs `git rev-list <range>`; a
//! `base..tip` range is the same commit set as `git rev-list <tip> ^<base>`, and
//! [`commits_in_range`] issues the two-operand form so both endpoints are
//! validated [`Oid`]s instead of an interpolated range string.
//!
//! # Failure is an error, never an empty answer
//!
//! A missing object, an unreadable commit or a failed `rev-list` is a typed
//! [`GitError`]. Callers that must fail closed (the divergence classifier does)
//! map that error to their own conservative verdict; this module never returns
//! an empty path list or an empty commit list to mean "git could not answer".
//!
//! # Read-only
//!
//! Both reads are pure queries: one bounded [`GitRepository`] child process
//! each, no index refresh (`GIT_OPTIONAL_LOCKS=0` is the seam's fixed policy),
//! no ref, worktree or object write, and no caller-supplied argv. Paths are
//! retained as the exact bytes git printed ([`GitPath`]), never round-tripped
//! through a lossy `String`.

use crate::{GitError, GitPath, GitRepository, Oid};

/// The commits reachable from `tip` and not from `base`, in git's own
/// `rev-list` order (newest first).
///
/// `commits_in_range(repo, base, tip)` is `git rev-list <tip> ^<base>`, i.e. the
/// reference's `rev_list("<base>..<tip>")`. An empty result means every commit
/// reachable from `tip` is also reachable from `base` — a genuinely empty
/// range, not a failed read. A failed read is a [`GitError`].
///
/// Both operands are complete [`Oid`]s, so neither can carry a revspec suffix,
/// a leading `-`, or an abbreviated id into the child's argv.
pub fn commits_in_range(repo: &GitRepository, base: &Oid, tip: &Oid) -> Result<Vec<Oid>, GitError> {
    let exclude = format!("^{}", base.as_str());
    let output = repo
        .process()
        .checked(&["rev-list", tip.as_str(), exclude.as_str()])?;
    let text = crate::path::utf8_field(&output.stdout, "rev-list output")?;
    text.lines()
        .filter(|line| !line.is_empty())
        .map(Oid::parse)
        .collect()
}

/// The paths `commit` changed, relative to its first parent.
///
/// This is the reference's `commit_paths(..., first_parent=True)`: one
/// `git show --name-only --format= -z --no-renames --first-parent -m` record
/// stream, split on NUL and decoded through the one raw path codec
/// ([`GitPath::from_git_bytes`]). A rename contributes both paths; a merge
/// contributes its first-parent diff (possibly nothing, when that diff really is
/// empty); a root commit contributes everything it added.
///
/// The returned order is git's order. Duplicates are not invented or removed:
/// git prints one name per path here.
pub fn commit_paths(repo: &GitRepository, commit: &Oid) -> Result<Vec<GitPath>, GitError> {
    let output = repo.process().checked(&[
        "-c",
        "diff.relative=false",
        "show",
        "--name-only",
        "--format=",
        "-z",
        "--no-renames",
        "--first-parent",
        "-m",
        commit.as_str(),
    ])?;
    output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .map(GitPath::from_git_bytes)
        .collect()
}
