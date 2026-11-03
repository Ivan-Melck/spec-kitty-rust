//! Ignore-rule queries.
//!
//! This is a read-only probe over the repository's ignore rules. It answers
//! whether one repository-relative path *matches an ignore rule*, which is a
//! different question from whether Git currently treats a file as "ignored":
//! a tracked file keeps its index entry even when a rule matches its name. The
//! probe therefore asks `git check-ignore --no-index`, so tracked membership
//! cannot hide a matching rule.
//!
//! The command goes through the same bounded [`GitProcess`](crate::process)
//! seam as every other query in this crate: fixed argv, no shell, the handle's
//! repository root and timeout, no prompts, and the captured operator index
//! binding is left in place. `check-ignore` does not write the index, status or
//! work tree, and no process-wide environment is changed.

use std::ffi::OsStr;

use crate::error::GitError;
use crate::path::GitPath;
use crate::repository::GitRepository;

impl GitRepository {
    /// Whether `path` matches an ignore rule of this repository.
    ///
    /// `path` is repository-relative and is passed as raw `OsStr` data after a
    /// `--` separator, so a leading `-`, spaces, a colon, or (on Unix) a
    /// non-UTF-8 name cannot be parsed as an option or round-tripped through a
    /// lossy string. The path is not required to exist: `check-ignore` answers
    /// for a missing path too, which is what callers probing a not-yet-written
    /// record need.
    ///
    /// The operand is literal Git path data. Unlike Python's `PurePath`
    /// adapter, this method does not turn an empty spelling into `.` or strip
    /// trailing separators; those queries can therefore differ from that
    /// adapter while matching Git exactly. Op-record paths use neither form.
    ///
    /// The probe uses `--no-index` deliberately. Plain `git check-ignore`
    /// reports "not ignored" for any path that is in the index, so it would
    /// hide an ignore rule for a tracked-but-ignored path. This method always
    /// answers the rule-matching question and never consults tracked
    /// membership. It does not change the handle's captured operator-index
    /// authority; that binding remains in place for every other operation.
    ///
    /// # Errors
    ///
    /// Exit status 0 is `Ok(true)` and status 1 is `Ok(false)`. Any other exit
    /// status is [`GitError::CommandFailed`]; a git executable that cannot be
    /// started, a spawn failure, a timeout, or a capture/read I/O failure is
    /// its corresponding [`GitError`] variant. A failed probe is never reported
    /// as `Ok(false)`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # fn main() -> Result<(), sk_git::GitError> {
    /// let repository = sk_git::GitRepository::open(".")?;
    /// let path = sk_git::GitPath::from_git_bytes(b"build/generated.log")?;
    /// if repository.is_path_ignored(&path)? {
    ///     println!("{} matches an ignore rule", path.display_lossy());
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn is_path_ignored(&self, path: &GitPath) -> Result<bool, GitError> {
        let output = self.process().allowing(
            &[
                OsStr::new("check-ignore"),
                OsStr::new("-q"),
                OsStr::new("--no-index"),
                OsStr::new("--"),
                path.as_os_str(),
            ],
            &[1],
        )?;
        Ok(output.status.code() == Some(0))
    }
}
