//! Read-only symbolic-ref queries.
//!
//! `git symbolic-ref --short <ref>` answers "what short name does this
//! symbolic ref point at", which is the only way to discover a remote's
//! default branch from local refs (`refs/remotes/origin/HEAD`). It is a
//! read-only query: it writes no ref, index or worktree byte and contacts no
//! remote.
//!
//! The answer taxonomy is Git's, not a caller's: exit 0 is the target, exit 1
//! is "this ref is absent or is not symbolic", and every other exit status is
//! a failed query. The Op commit-or-skip policy is deliberately more tolerant
//! than this generic read (it treats any completed non-zero answer as "no
//! symbolic target"); that policy lives with the Op domain, not here.
//!
//! The command goes through the same bounded [`GitProcess`](crate::process)
//! seam as every other query in this crate: fixed argv, no shell, the handle's
//! repository root and timeout, no prompts, and the captured operator-index
//! binding is left in place.

use std::ffi::OsStr;

use crate::error::GitError;
use crate::path::{strip_line_feed, utf8_field};
use crate::refname::RefName;
use crate::repository::GitRepository;

impl GitRepository {
    /// The short target of a symbolic ref, or `None` when it has none.
    ///
    /// `reference` is a validated fully-qualified ref name, passed as argv
    /// data. The answer is exactly `git symbolic-ref --quiet --short
    /// <reference>`: Git shortens the target the way it normally does
    /// (`refs/heads/main` → `main`, `refs/remotes/origin/main` →
    /// `origin/main`). The target does not have to exist: a dangling symbolic
    /// ref still reports its target, which is what lets a caller distinguish
    /// "no `origin/HEAD`" from "`origin/HEAD` names a branch that is gone".
    ///
    /// # Errors
    ///
    /// Exit status 1 is `Ok(None)` (absent, or not a symbolic ref). Any other
    /// non-zero status is [`GitError::CommandFailed`]; a git executable that
    /// cannot be started, a spawn failure, a timeout, or a capture/read I/O
    /// failure is its corresponding [`GitError`]. A successful answer that is
    /// empty is [`GitError::InvalidOutput`] and one that is not UTF-8 is
    /// [`GitError::NonUtf8`]: a symbolic ref target is a ref name, so an
    /// unusable one is a typed failure rather than a quiet absence.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # fn main() -> Result<(), sk_git::GitError> {
    /// let repository = sk_git::GitRepository::open(".")?;
    /// let origin_head = sk_git::RefName::parse("refs/remotes/origin/HEAD")?;
    /// if let Some(branch) = repository.symbolic_ref_short_target(&origin_head)? {
    ///     println!("origin/HEAD names {branch}");
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn symbolic_ref_short_target(
        &self,
        reference: &RefName,
    ) -> Result<Option<String>, GitError> {
        let output = self.process().allowing(
            &[
                OsStr::new("symbolic-ref"),
                OsStr::new("--quiet"),
                OsStr::new("--short"),
                OsStr::new(reference.as_str()),
            ],
            &[1],
        )?;
        if output.status.code() != Some(0) {
            return Ok(None);
        }
        let raw = strip_line_feed(&output.stdout);
        if raw.is_empty() {
            return Err(GitError::InvalidOutput {
                detail: format!(
                    "git symbolic-ref --short {reference} returned an empty symbolic target"
                ),
            });
        }
        Ok(Some(utf8_field(raw, "symbolic ref target")?))
    }
}
