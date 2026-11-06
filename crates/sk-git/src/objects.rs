//! Bounded, typed object and revision reads.
//!
//! Coordination workflows need to read committed content without checking
//! anything out and without trusting a path as revision syntax:
//!
//! * restoring a coordination surface from the coordination tip
//!   (`<tip>:kitty-specs/<mission>/meta.json`),
//! * reading the `HEAD` baseline of `meta.json` for a demotion guard,
//! * probing whether a seed trailer exists without reading this crate's own
//!   journal,
//! * deciding whether one lane tip is reachable from another via a real merge
//!   base instead of approximating with two `is_ancestor` calls,
//! * reading the committed events log at `HEAD` for a rollback point, where
//!   "the path is not in `HEAD`" must be distinguishable from "there is no
//!   `HEAD` yet" and from "`HEAD` cannot be read"].
//!
//! Every read here is parsed from `-z` plumbing output. A path is passed as a
//! pathspec under `--literal-pathspecs` and then matched *exactly* against the
//! printed path, so a caller cannot smuggle glob magic or revspec syntax, and a
//! prefix-sharing path can never answer for the requested one. Blob bodies are
//! bounded: the size is asked of git before any body is captured, and an
//! over-limit blob is a typed refusal rather than a large allocation.

use std::collections::BTreeMap;

use crate::error::GitError;
use crate::oid::Oid;
use crate::path::GitPath;
use crate::refname::RefName;
use crate::refname::StartPoint;
use crate::repository::GitRepository;

/// A completed full-symbolic-HEAD probe, with exit status separate from
/// successful target text decoding. Spawn/capture failures and malformed successful
/// output are errors, never a completed nonzero/absent answer.
#[derive(Debug, Clone)]
pub struct SymbolicHeadObservation {
    status_code: Option<i32>,
    target: Option<String>,
    stderr: Vec<u8>,
}

impl SymbolicHeadObservation {
    /// The completed process status; `None` means termination by signal.
    pub fn status_code(&self) -> Option<i32> {
        self.status_code
    }
    /// The exact decoded full target after a successful exit. This observation
    /// grants no publication authority; callers requiring `RefName` use the
    /// canonical validating `symbolic_head_ref` adapter.
    pub fn target_text(&self) -> Option<&str> {
        self.target.as_deref()
    }
}

/// A revision operand the reference would accept, validated for argv safety.
///
/// The typed [`StartPoint`] is deliberately narrow: `HEAD`, a full object id, or
/// a *fully-qualified* `refs/...` name. Source call sites also read committed
/// content at a short branch name, a tag name or a remote-tracking spelling
/// (`blob_at(repo, "origin/main", path)`), which `StartPoint` cannot express
/// without coercing every shape into a local branch. `SourceRevision` is that
/// missing seam: a Source-valid single revision Git itself resolves.
///
/// Validation is only argv safety — non-empty, no leading `-` (so the operand
/// can never be read as an option), and no control byte (including NUL). Every
/// other decision (does it resolve? is it a branch, tag, remote ref, tree-ish?)
/// stays with Git, exactly as the reference leaves it, and a failure is the
/// ordinary typed [`GitError`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceRevision(String);

impl SourceRevision {
    /// Validate a Source-compatible revision operand.
    ///
    /// # Errors
    ///
    /// An empty value, a leading `-`, or any control byte (including NUL) is
    /// refused with [`GitError::InvalidPlan`]; nothing is run.
    pub fn parse(value: &str) -> Result<Self, GitError> {
        if value.is_empty() {
            return Err(GitError::InvalidPlan {
                detail: "a Source revision must not be empty".to_string(),
            });
        }
        if value.starts_with('-') {
            return Err(GitError::InvalidPlan {
                detail: format!(
                    "Source revision {value:?} starts with `-` and could be read as an option"
                ),
            });
        }
        if value.bytes().any(|byte| byte < 0x20 || byte == 0x7f) {
            return Err(GitError::InvalidPlan {
                detail: "a Source revision must not contain a control byte".to_string(),
            });
        }
        Ok(Self(value.to_string()))
    }

    /// The revision operand, exactly as the caller supplied it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SourceRevision {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The default bound for one blob read (8 MiB).
///
/// The subprocess seam caps one captured stream at 64 MiB for an explicitly
/// bounded read; this default stays deliberately small so a mistaken whole-blob
/// read of a build artefact fails instead of allocating.
pub const DEFAULT_MAX_BLOB_BYTES: u64 = 8 * 1024 * 1024;

/// The largest explicit blob bound this adapter accepts: the canonical status
/// log bound (64 MiB).
///
/// `sk-status` bounds the canonical event log and diary at 64 MiB, so an
/// explicit read must be able to return such a log whole. The bound is plumbed
/// through the subprocess seam as the child's actual capture limit; the read is
/// still bounded and still refuses (never truncates) anything above it. The
/// default stays at [`DEFAULT_MAX_BLOB_BYTES`].
pub const MAX_BLOB_BYTES: u64 = 64 * 1024 * 1024;

/// What a tree entry names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TreeEntryKind {
    /// A regular or executable file (`100644`, `100755`).
    Blob,
    /// A symbolic link committed as a blob (`120000`).
    Symlink,
    /// A submodule gitlink (`160000`); its id names a commit, not a blob.
    Gitlink,
    /// A nested tree (`040000`).
    Tree,
}

impl TreeEntryKind {
    /// A stable machine-readable name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Blob => "blob",
            Self::Symlink => "symlink",
            Self::Gitlink => "gitlink",
            Self::Tree => "tree",
        }
    }

    /// Whether the entry's id names a blob body (`Blob` or `Symlink`).
    pub fn is_blob(self) -> bool {
        matches!(self, Self::Blob | Self::Symlink)
    }

    fn from_ls_tree(mode: &str, kind: &str) -> Result<Self, GitError> {
        match (mode, kind) {
            ("120000", "blob") => Ok(Self::Symlink),
            (_, "blob") => Ok(Self::Blob),
            ("160000", "commit") => Ok(Self::Gitlink),
            ("040000", "tree") => Ok(Self::Tree),
            _ => Err(GitError::InvalidOutput {
                detail: format!("unrecognised tree entry mode {mode:?} / type {kind:?}"),
            }),
        }
    }
}

/// One entry of a tree, resolved at a validated revision and exact path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    path: GitPath,
    kind: TreeEntryKind,
    mode: String,
    oid: Oid,
}

impl TreeEntry {
    /// The repository-relative path, exactly as git printed it.
    pub fn path(&self) -> &GitPath {
        &self.path
    }

    /// What the entry names.
    pub fn kind(&self) -> TreeEntryKind {
        self.kind
    }

    /// The raw git tree mode (`100644`, `120000`, `160000`, `040000`).
    pub fn mode(&self) -> &str {
        &self.mode
    }

    /// The entry's object id.
    pub fn oid(&self) -> &Oid {
        &self.oid
    }
}

/// The distinguished answer to "read this path at `HEAD`".
///
/// A rollback point needs to tell three different absences apart, so this is not
/// an `Option`:
///
/// * [`BlobLookup::Present`] — the exact committed bytes;
/// * [`BlobLookup::Absent`] — `HEAD` exists and the path is not a blob in it
///   (never committed, deleted, a directory, or a submodule);
/// * [`BlobLookup::NoRevision`] — the repository has no `HEAD` commit at all
///   (an unborn branch).
///
/// A `HEAD` that exists but cannot be read (a corrupt ref, a missing object, a
/// failed git invocation) is a [`GitError`], never a silent absence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlobLookup {
    /// The committed blob bytes, exactly as stored.
    Present(Vec<u8>),
    /// `HEAD` resolves, but the path is absent or names no blob.
    Absent,
    /// The repository has no `HEAD` commit (an unborn branch).
    NoRevision,
}

impl BlobLookup {
    /// A stable machine-readable name.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Present(_) => "present",
            Self::Absent => "absent",
            Self::NoRevision => "no_revision",
        }
    }

    /// The bytes, when present.
    pub fn bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Present(bytes) => Some(bytes),
            _ => None,
        }
    }
}

/// One real index entry from the canonical `ls-files --stage -z` reader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    path: GitPath,
    mode: String,
    oid: Oid,
    stage: u8,
}

impl IndexEntry {
    /// Exact repository-relative path, preserving non-UTF-8 platform bytes.
    pub fn path(&self) -> &GitPath {
        &self.path
    }
    /// Raw Git index mode.
    pub fn mode(&self) -> &str {
        &self.mode
    }
    /// Recorded full object id.
    pub fn oid(&self) -> &Oid {
        &self.oid
    }
    /// Index stage (`0` normal, `1` base, `2` ours, `3` theirs).
    pub fn stage(&self) -> u8 {
        self.stage
    }
}

/// One conflict stage of one path after a stopped merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnmergedStage {
    stage: u8,
    mode: String,
    oid: Oid,
}

impl UnmergedStage {
    /// The conflict stage: `1` base, `2` ours, `3` theirs.
    pub fn stage(&self) -> u8 {
        self.stage
    }

    /// The raw mode recorded for this stage.
    pub fn mode(&self) -> &str {
        &self.mode
    }

    /// The blob id recorded for this stage.
    pub fn oid(&self) -> &Oid {
        &self.oid
    }
}

/// The conflict evidence for one path in a stopped merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnmergedPath {
    path: GitPath,
    stages: Vec<UnmergedStage>,
}

impl UnmergedPath {
    /// The repository-relative path.
    pub fn path(&self) -> &GitPath {
        &self.path
    }

    /// The recorded stages, ordered by stage number.
    pub fn stages(&self) -> &[UnmergedStage] {
        &self.stages
    }
}

impl GitRepository {
    /// The tree entry at exactly `path` in `revision`.
    ///
    /// The revision is a validated [`StartPoint`] (`HEAD`, a full object id, or
    /// a fully-qualified ref). The path is a literal pathspec, matched exactly
    /// against the printed entry, so `a/b` never answers for `a/bc` and a path
    /// named `:(glob)*` stays data. A missing path is `Ok(None)`; a failed git
    /// invocation is an error.
    pub fn tree_entry_at(
        &self,
        revision: &StartPoint,
        path: &GitPath,
    ) -> Result<Option<TreeEntry>, GitError> {
        self.tree_entry_for(revision.operand(), false, path)
    }

    /// The tree entry at exactly `path` in a Source-compatible revision.
    ///
    /// Like [`GitRepository::tree_entry_at`], but the revision is a
    /// [`SourceRevision`] (a short branch, tag, remote-tracking name, `HEAD` or
    /// object id) and the operand is passed after `--end-of-options`, so even a
    /// revision Git would read as an option stays data. The same
    /// `ls-tree` reader, exact-path match and absence rule are used.
    pub fn tree_entry_at_revision(
        &self,
        revision: &SourceRevision,
        path: &GitPath,
    ) -> Result<Option<TreeEntry>, GitError> {
        self.tree_entry_for(revision.as_str(), true, path)
    }

    fn tree_entry_for(
        &self,
        revision: &str,
        end_of_options: bool,
        path: &GitPath,
    ) -> Result<Option<TreeEntry>, GitError> {
        use std::ffi::OsStr;
        if path.is_empty() {
            return Err(GitError::InvalidPlan {
                detail: "a tree lookup needs a non-empty repository-relative path".to_string(),
            });
        }
        let mut args: Vec<&OsStr> = vec![
            OsStr::new("--literal-pathspecs"),
            OsStr::new("ls-tree"),
            OsStr::new("--full-tree"),
            OsStr::new("-z"),
        ];
        if end_of_options {
            args.push(OsStr::new("--end-of-options"));
        }
        args.push(OsStr::new(revision));
        args.push(OsStr::new("--"));
        args.push(path.as_os_str());
        let output = self.process().checked(&args)?;
        for record in output.stdout.split(|byte| *byte == 0) {
            if record.is_empty() {
                continue;
            }
            let entry = parse_ls_tree_record(record)?;
            if &entry.path == path {
                return Ok(Some(entry));
            }
        }
        Ok(None)
    }

    /// Read the blob at exactly `path` in `revision`, bounded.
    ///
    /// `Ok(None)` means the path is absent, or names a tree or submodule
    /// gitlink (neither has a blob body). The size is read with `git cat-file
    /// -s` before the body is captured, so an over-limit blob is
    /// [`GitError::BlobTooLarge`] and no large allocation happens.
    pub fn read_blob_at(
        &self,
        revision: &StartPoint,
        path: &GitPath,
    ) -> Result<Option<Vec<u8>>, GitError> {
        self.read_blob_at_bounded(revision, path, DEFAULT_MAX_BLOB_BYTES)
    }

    /// Read a blob with an explicit byte bound.
    ///
    /// A bound above [`MAX_BLOB_BYTES`] (64 MiB) is refused. The bound is also
    /// the child's output capture limit, so a blob the caller accepted is
    /// returned whole: this seam cannot silently truncate at a smaller cap.
    pub fn read_blob_at_bounded(
        &self,
        revision: &StartPoint,
        path: &GitPath,
        limit: u64,
    ) -> Result<Option<Vec<u8>>, GitError> {
        self.read_blob_for(revision.operand(), false, path, limit)
    }

    /// Read the blob at exactly `path` in a Source-compatible revision, bounded
    /// by [`DEFAULT_MAX_BLOB_BYTES`].
    ///
    /// The Source-shaped sibling of [`GitRepository::read_blob_at`]: the
    /// revision is a [`SourceRevision`], so a short branch, a tag name or a
    /// remote-tracking spelling reads the same way Source's
    /// `kernel.git.blob_at(repo, ref, path)` does. The tree read, exact-path
    /// match, blob-size preflight and bounded body read are shared with the
    /// typed-`StartPoint` route; there is no second object reader.
    pub fn read_blob_at_revision(
        &self,
        revision: &SourceRevision,
        path: &GitPath,
    ) -> Result<Option<Vec<u8>>, GitError> {
        self.read_blob_at_revision_bounded(revision, path, DEFAULT_MAX_BLOB_BYTES)
    }

    /// [`GitRepository::read_blob_at_revision`] with an explicit byte bound.
    pub fn read_blob_at_revision_bounded(
        &self,
        revision: &SourceRevision,
        path: &GitPath,
        limit: u64,
    ) -> Result<Option<Vec<u8>>, GitError> {
        self.read_blob_for(revision.as_str(), true, path, limit)
    }

    fn read_blob_for(
        &self,
        revision: &str,
        end_of_options: bool,
        path: &GitPath,
        limit: u64,
    ) -> Result<Option<Vec<u8>>, GitError> {
        if limit > MAX_BLOB_BYTES {
            return Err(GitError::InvalidPlan {
                detail: format!(
                    "a blob read bound must be at most {MAX_BLOB_BYTES} bytes, got {limit}"
                ),
            });
        }
        let Some(entry) = self.tree_entry_for(revision, end_of_options, path)? else {
            return Ok(None);
        };
        if !entry.kind.is_blob() {
            return Ok(None);
        }
        Ok(Some(self.read_entry_blob(&entry, path, limit)?))
    }

    /// Prove that a `HEAD` which does not resolve to a commit is genuinely
    /// unborn rather than corrupt.
    ///
    /// `git rev-parse --quiet` answers exit 1 both for an unborn branch and for
    /// a `HEAD` whose content is malformed, so the symbolic target is inspected:
    /// it must be a valid `refs/...` name, and that ref must be absent. Anything
    /// else is [`GitError::UnreadableHead`].
    fn require_unborn_head(&self) -> Result<(), GitError> {
        let Some(reference) = self.symbolic_head_ref()? else {
            return Err(GitError::UnreadableHead {
                detail: "HEAD is detached but does not resolve to a commit".to_string(),
            });
        };
        match self.read_ref(&reference)? {
            None => Ok(()),
            Some(oid) => Err(GitError::UnreadableHead {
                detail: format!(
                    "HEAD names {reference}, which exists at {oid} but does not resolve to a commit"
                ),
            }),
        }
    }

    /// The symbolic target of `HEAD` as a validated full ref name.
    ///
    /// This is the one full `HEAD` authority: `git symbolic-ref --quiet HEAD`
    /// uses three answers — exit 0 prints the target, exit 1 means `HEAD` is
    /// detached, and any other status means `HEAD` exists but names no usable
    /// ref (for example `ref: refs/heads/`, for which git exits 128 with
    /// `fatal: No such ref: HEAD`). The last case, a target that is not UTF-8,
    /// and a target that fails [`RefName`](crate::RefName) validation are all
    /// [`GitError::UnreadableHead`]: a corrupt `HEAD` is never reported as
    /// absent, detached, or a plain command failure.
    ///
    /// The result is the **full** target (`refs/heads/<name>`,
    /// `refs/spec-kitty/lane-tip/<name>`, ...), never a shortened label and
    /// never a widened `RefName` for `HEAD` itself. It is also the unborn-HEAD
    /// proof: a symbolic target whose ref does not exist is an unborn
    /// branch/namespace, not corruption. Use
    /// [`GitRepository::current_branch`](crate::GitRepository::current_branch)
    /// for git's `--short` label.
    pub fn symbolic_head_ref(&self) -> Result<Option<RefName>, GitError> {
        let observed = self.observe_symbolic_head()?;
        match observed.status_code() {
            Some(0) => observed
                .target
                .map(|text| {
                    RefName::parse(&text).map_err(|error| GitError::UnreadableHead {
                        detail: format!("HEAD names an invalid ref ({text:?}): {error}"),
                    })
                })
                .transpose(),
            Some(1) => Ok(None),
            _ => {
                let stderr = String::from_utf8_lossy(&observed.stderr).trim().to_string();
                Err(GitError::UnreadableHead {
                    detail: format!(
                        "git symbolic-ref --quiet HEAD failed (status {:?}): {}",
                        observed.status_code(),
                        if stderr.is_empty() {
                            "no error output".to_string()
                        } else {
                            stderr
                        }
                    ),
                })
            }
        }
    }

    /// Observe the one canonical full HEAD probe without conflating a completed
    /// nonzero exit with failed validation of successful stdout. The strict
    /// [`Self::symbolic_head_ref`] API retains its existing error mapping.
    pub fn observe_symbolic_head(&self) -> Result<SymbolicHeadObservation, GitError> {
        let output = self.process().run(&["symbolic-ref", "--quiet", "HEAD"])?;
        let target = if output.status.success() {
            let raw = crate::path::strip_line_feed(&output.stdout);
            if raw.is_empty() {
                return Err(GitError::UnreadableHead {
                    detail: "git symbolic-ref printed an empty HEAD target".to_string(),
                });
            }
            let text = std::str::from_utf8(raw).map_err(|_| GitError::UnreadableHead {
                detail: "HEAD names a ref that is not valid UTF-8".to_string(),
            })?;
            Some(text.to_owned())
        } else {
            None
        };
        Ok(SymbolicHeadObservation {
            status_code: output.status.code(),
            target,
            stderr: output.stderr,
        })
    }

    fn read_entry_blob(
        &self,
        entry: &TreeEntry,
        path: &GitPath,
        limit: u64,
    ) -> Result<Vec<u8>, GitError> {
        let size_output = self
            .process()
            .checked(&["cat-file", "-s", entry.oid.as_str()])?;
        let size_text = crate::path::utf8_field(
            crate::path::strip_line_feed(&size_output.stdout),
            "blob size",
        )?;
        let size = size_text
            .trim()
            .parse::<u64>()
            .map_err(|_| GitError::InvalidOutput {
                detail: format!("git cat-file -s printed a non-numeric size {size_text:?}"),
            })?;
        if size > limit {
            return Err(GitError::BlobTooLarge {
                path: path.display_lossy(),
                limit,
            });
        }
        let body = self
            .process()
            .checked_with_output_limit(&["cat-file", "blob", entry.oid.as_str()], limit)?;
        if body.stdout.len() as u64 != size {
            return Err(GitError::InvalidOutput {
                detail: format!(
                    "git cat-file blob returned {} bytes for a {size}-byte blob",
                    body.stdout.len()
                ),
            });
        }
        Ok(body.stdout)
    }

    /// Read a blob at `HEAD` with the distinguished absence answer.
    ///
    /// This is the seam a rollback point should use to snapshot a committed file
    /// (for example the events log):
    ///
    /// ```no_run
    /// # fn main() -> Result<(), sk_git::GitError> {
    /// # let repository = sk_git::GitRepository::open(".")?;
    /// let path = sk_git::GitPath::from_git_bytes(
    ///     b".kittify/events/status.events.jsonl",
    /// )?;
    /// let rollback_point = match repository.read_blob_at_head(&path)? {
    ///     // Exact committed bytes; persist them as the rollback point.
    ///     sk_git::BlobLookup::Present(bytes) => Some(bytes),
    ///     // HEAD exists; the log was never committed (or was deleted).
    ///     sk_git::BlobLookup::Absent => None,
    ///     // Unborn repository: there is no commit to read a rollback point from.
    ///     sk_git::BlobLookup::NoRevision => None,
    /// };
    /// let _ = rollback_point;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// An unreadable `HEAD` (corrupt ref, missing object, failed git call) is an
    /// error; it is never reported as [`BlobLookup::Absent`]. An over-limit blob
    /// is [`GitError::BlobTooLarge`].
    pub fn read_blob_at_head(&self, path: &GitPath) -> Result<BlobLookup, GitError> {
        self.read_blob_at_head_bounded(path, DEFAULT_MAX_BLOB_BYTES)
    }

    /// [`GitRepository::read_blob_at_head`] with an explicit byte bound.
    pub fn read_blob_at_head_bounded(
        &self,
        path: &GitPath,
        limit: u64,
    ) -> Result<BlobLookup, GitError> {
        if limit > MAX_BLOB_BYTES {
            return Err(GitError::InvalidPlan {
                detail: format!(
                    "a blob read bound must be at most {MAX_BLOB_BYTES} bytes, got {limit}"
                ),
            });
        }
        let Some(head_text) = self.head_commit()? else {
            self.require_unborn_head()?;
            return Ok(BlobLookup::NoRevision);
        };
        let head = StartPoint::Oid(Oid::parse(&head_text)?);
        match self.tree_entry_at(&head, path)? {
            Some(entry) if entry.kind.is_blob() => {
                let bytes = self.read_entry_blob(&entry, path, limit)?;
                Ok(BlobLookup::Present(bytes))
            }
            Some(_) | None => Ok(BlobLookup::Absent),
        }
    }

    /// Every path in `revision`, optionally restricted to one literal prefix.
    ///
    /// The prefix is a literal pathspec, so it matches a directory and its
    /// descendants but never a sibling whose name merely starts with the same
    /// bytes. Paths are returned in git's tree order with exact bytes retained.
    pub fn tree_paths_at(
        &self,
        revision: &StartPoint,
        prefix: Option<&GitPath>,
    ) -> Result<Vec<GitPath>, GitError> {
        self.tree_paths_for(revision.operand(), false, prefix)
    }

    /// Every path in a Source-compatible revision, optionally restricted to one
    /// literal prefix.
    ///
    /// The Source-shaped sibling of [`GitRepository::tree_paths_at`], sharing
    /// the same `ls-tree -r --name-only -z` reader. Source call sites pass a
    /// short coordination branch name here, which a [`StartPoint`] cannot
    /// express without local-branch coercion.
    pub fn tree_paths_at_revision(
        &self,
        revision: &SourceRevision,
        prefix: Option<&GitPath>,
    ) -> Result<Vec<GitPath>, GitError> {
        self.tree_paths_for(revision.as_str(), true, prefix)
    }

    fn tree_paths_for(
        &self,
        revision: &str,
        end_of_options: bool,
        prefix: Option<&GitPath>,
    ) -> Result<Vec<GitPath>, GitError> {
        let mut args: Vec<std::ffi::OsString> = vec![
            "--literal-pathspecs".into(),
            "ls-tree".into(),
            "-r".into(),
            "--name-only".into(),
            "-z".into(),
            "--full-tree".into(),
        ];
        if end_of_options {
            args.push("--end-of-options".into());
        }
        args.push(revision.into());
        if let Some(prefix) = prefix
            && !prefix.is_empty()
        {
            args.push("--".into());
            args.push(prefix.to_os_string());
        }
        let output = self.process().checked(&args)?;
        let mut paths = Vec::new();
        for record in output.stdout.split(|byte| *byte == 0) {
            if record.is_empty() {
                continue;
            }
            paths.push(GitPath::from_git_bytes(record)?);
        }
        Ok(paths)
    }

    /// The best common ancestor of two commits, or `None` when they are
    /// unrelated.
    ///
    /// A real `git merge-base` answer replaces the two-`is_ancestor`
    /// approximation: `None` means "no common ancestor exists", not "one of the
    /// two is not reachable from the other".
    pub fn merge_base(&self, left: &Oid, right: &Oid) -> Result<Option<Oid>, GitError> {
        let output = self
            .process()
            .allowing(&["merge-base", left.as_str(), right.as_str()], &[1])?;
        match output.status.code() {
            Some(0) => {
                let text = crate::path::utf8_field(
                    crate::path::strip_line_feed(&output.stdout),
                    "merge base",
                )?;
                let line = text.lines().next().unwrap_or_default().trim();
                if line.is_empty() {
                    return Err(GitError::InvalidOutput {
                        detail: "git merge-base succeeded without printing a commit".to_string(),
                    });
                }
                Ok(Some(Oid::parse(line)?))
            }
            Some(1) => Ok(None),
            _ => Err(self
                .process()
                .failed(&["merge-base", left.as_str(), right.as_str()], &output)),
        }
    }

    /// Read index entries for literal pathspecs (empty means all paths).
    ///
    /// Uses this repository's canonical process, captured index and configured
    /// timeout. Names are `--full-name` and NUL-delimited; glob magic is data,
    /// matching Source `index_entries(..., glob=False, tags=False)`.
    pub fn index_entries(
        &self,
        pathspecs: &[&std::ffi::OsStr],
    ) -> Result<Vec<IndexEntry>, GitError> {
        use std::ffi::OsStr;
        let mut args = vec![
            OsStr::new("--literal-pathspecs"),
            OsStr::new("ls-files"),
            OsStr::new("--full-name"),
            OsStr::new("--stage"),
            OsStr::new("-z"),
            OsStr::new("--"),
        ];
        args.extend_from_slice(pathspecs);
        let output = self.process().checked(&args)?;
        output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|record| !record.is_empty())
            .map(|record| parse_index_record(record, false))
            .collect()
    }

    /// Every unmerged index entry, grouped by path.
    ///
    /// Read from `git ls-files -u -z`, so paths with spaces, tabs, newlines or
    /// non-UTF-8 bytes survive exactly. This is the conflict evidence a stopped
    /// merge leaves behind; an empty result means every entry is at stage 0.
    pub fn unmerged_paths(&self) -> Result<Vec<UnmergedPath>, GitError> {
        let output = self.process().checked(&["ls-files", "-u", "-z"])?;
        let mut grouped: BTreeMap<GitPath, Vec<UnmergedStage>> = BTreeMap::new();
        for record in output.stdout.split(|byte| *byte == 0) {
            if record.is_empty() {
                continue;
            }
            let IndexEntry {
                path,
                mode,
                oid,
                stage,
            } = parse_index_record(record, true)?;
            grouped
                .entry(path)
                .or_default()
                .push(UnmergedStage { stage, mode, oid });
        }
        Ok(grouped
            .into_iter()
            .map(|(path, mut stages)| {
                stages.sort_by_key(UnmergedStage::stage);
                UnmergedPath { path, stages }
            })
            .collect())
    }
}

// One stage-record parser for normal-index inspection and stopped-merge evidence.
fn parse_index_record(record: &[u8], unmerged: bool) -> Result<IndexEntry, GitError> {
    let command = if unmerged { "-u" } else { "--stage" };
    let label = if unmerged {
        "unmerged index header"
    } else {
        "index header"
    };
    let Some(tab) = record.iter().position(|byte| *byte == b'\t') else {
        return Err(GitError::InvalidOutput {
            detail: format!(
                "malformed git ls-files {command} record {:?}",
                String::from_utf8_lossy(record)
            ),
        });
    };
    let header = crate::path::utf8_field(&record[..tab], label)?;
    let path = GitPath::from_git_bytes(&record[tab + 1..])?;
    let mut fields = header.split(' ');
    let mode = fields.next().unwrap_or_default().to_string();
    let oid = fields.next().unwrap_or_default();
    let stage = fields.next().unwrap_or_default();
    if mode.is_empty() || oid.is_empty() || stage.is_empty() {
        return Err(GitError::InvalidOutput {
            detail: format!("malformed {label} {header:?}"),
        });
    }
    let stage_number = stage.parse::<u8>().map_err(|_| GitError::InvalidOutput {
        detail: format!(
            "{}stage {stage:?} is not a number",
            if unmerged { "unmerged " } else { "index " }
        ),
    })?;
    Ok(IndexEntry {
        path,
        mode,
        oid: Oid::parse(oid)?,
        stage: stage_number,
    })
}

fn parse_ls_tree_record(record: &[u8]) -> Result<TreeEntry, GitError> {
    let Some(tab) = record.iter().position(|byte| *byte == b'\t') else {
        return Err(GitError::InvalidOutput {
            detail: format!(
                "malformed git ls-tree record {:?}",
                String::from_utf8_lossy(record)
            ),
        });
    };
    let header = crate::path::utf8_field(&record[..tab], "ls-tree header")?;
    let path = GitPath::from_git_bytes(&record[tab + 1..])?;
    let mut fields = header.split(' ');
    let mode = fields.next().unwrap_or_default().to_string();
    let kind = fields.next().unwrap_or_default().to_string();
    let oid = fields.next().unwrap_or_default();
    if mode.is_empty() || kind.is_empty() || oid.is_empty() {
        return Err(GitError::InvalidOutput {
            detail: format!("malformed ls-tree header {header:?}"),
        });
    }
    Ok(TreeEntry {
        path,
        kind: TreeEntryKind::from_ls_tree(&mode, &kind)?,
        mode,
        oid: Oid::parse(oid)?,
    })
}
