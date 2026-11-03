//! Commit-or-skip policy for the Op-record auto-commit (#6410).
//!
//! The reference executor force-adds the record it commits, so without its
//! skip policy it can land an `op(...)` commit in a repository that never
//! tracks Op records or that gitignores `kitty-ops/`. This native module
//! is the read-only native port of `specify_cli/invocation/op_commit_policy.py`
//! and answers the two questions that decide whether that commit may happen:
//!
//! * [`op_commit_skip_reason`] — is the exact path about to be committed
//!   gitignored (through the accepted tracked-aware
//!   [`GitRepository::is_path_ignored`] probe), and if not, does the primary
//!   branch track any Op-record-shaped file?
//! * [`probe_primary_tracking`] — resolve the primary branch and list its
//!   `kitty-ops/` tree.
//!
//! The primary branch is resolved WITHOUT current-branch bias: when nothing
//! resolves the answer is "unresolved", never the checked-out branch. Falling
//! back to the current branch would let the first `op(...)` commit on an issue
//! branch satisfy the predicate for the next close on that same branch. The
//! biased primary/default-network helpers are deliberately not reused.
//!
//! # Source semantics carried over
//!
//! * Resolution: `refs/remotes/origin/HEAD` short target (probed as a local
//!   ref, then as an `origin` ref), otherwise the `main`/`master`/`develop`
//!   ladder, skipping a name that was already probed. At most nine resolution
//!   Git calls (one `symbolic-ref`, two for a dangling name, six for the
//!   ladder) before any tree is listed.
//! * Tree shape: only `(?:^|/)kitty-ops/[0-9A-HJKMNP-TV-Z]{26}\.jsonl` counts.
//!   The id is a 26-character upper-case Crockford string, not a decoded ULID;
//!   `ops-index.jsonl`, `op-closures.jsonl` and arbitrary JSON do not count.
//!   Both the local and the `origin` tree may satisfy the question.
//! * Failure taxonomy: a *completed* non-zero symbolic-ref or ref-existence
//!   answer is "absent", because the reference's `_ref_exists` only compares
//!   the exit code. A git that never started, a timeout, or a capture/read
//!   failure is [`OpCommitProbeError`] — never a quiet `false`. The generic
//!   [`GitRepository::symbolic_ref_short_target`] read is stricter (only exit
//!   1 is `None`); this policy maps its other typed completed answer to
//!   "absent" exactly as the reference does.
//! * Best effort: [`OpCommitSkipProbe`] mirrors
//!   `executor._op_commit_skip_reason`. It probes and caches primary tracking
//!   first, then asks [`op_commit_skip_reason`] (whose ignore check precedes the
//!   tracking reasons). A probe failure becomes the reference's
//!   `git probe failed while deciding whether to commit the Op record: …` skip
//!   reason. It is not a success, a commit, or a write refusal. A successful
//!   tracking result stays cached if a later ignore probe fails; a failed
//!   tracking result is not cached, so the next decision retries it.
//!
//! # Limits
//!
//! * The reference uses 30 seconds for every probe. This handle-based API
//!   preserves its caller's GitOptions timeout (default 10 seconds); a composer
//!   requiring the source budget must open its handle with 30 seconds.
//! * Process capture is bounded to 16 MiB per stream by the shared Git seam,
//!   unlike the reference's unbounded capture. Overflow stays a typed probe
//!   failure, never quiet absence or eligibility.
//! * The generic symbolic-ref API takes fully-qualified refs, not the special
//!   HEAD operand. Current-branch queries use a separate existing Git API.
//!
//! * The reference also accepts a bare filesystem root and runs `git -C` per
//!   probe. The native API requires an already-opened [`GitRepository`], so a
//!   not-a-repository root is refused when the handle is opened (for example
//!   [`sk_git::GitError::NotARepository`]). This is not full command parity.
//! * Reason strings and the `check-ignore` operand use the literal
//!   [`GitPath`] spelling. Like the accepted ignore probe, this does not
//!   reproduce Python's `PurePath` normalization of an empty path to `.` or of
//!   trailing separators; Op-record paths use neither form. A non-UTF-8 path is
//!   rendered lossily in the reason text only.
//! * A successful but unusable answer from the generic read (empty symbolic
//!   target, non-UTF-8 ref name, non-commit object id) is a typed probe error
//!   here rather than the reference's truthiness-based answer. Real Git cannot
//!   produce those shapes for these refs.
//! * This module decides only whether to skip. It performs no commit, no
//!   protection preflight, no admission decision, no write and no CLI wiring.

use std::borrow::Cow;

use sk_git::{GitError, GitPath, GitRepository, RefName, StartPoint};
use thiserror::Error;

/// Branch names the reference's fallback ladder considers primary, in order.
///
/// Mirrors `specify_cli.core.git_ops.COMMON_PRIMARY_BRANCHES`.
pub const COMMON_PRIMARY_BRANCHES: [&str; 3] = ["main", "master", "develop"];

/// The `origin/HEAD` symbolic ref that names a remote default branch.
const ORIGIN_HEAD_REF: &str = "refs/remotes/origin/HEAD";
/// Prefix stripped from that symbolic target before refs are probed.
const ORIGIN_PREFIX: &str = "origin/";
/// Prefix of a local branch ref.
const LOCAL_BRANCH_PREFIX: &str = "refs/heads/";
/// Prefix of a remote-tracking branch ref.
const ORIGIN_BRANCH_PREFIX: &str = "refs/remotes/origin/";
/// The literal tree pathspec that bounds the Op-record scan.
const OP_EVENTS_PATHSPEC: &[u8] = b"kitty-ops/";
/// The directory component the record path must end with.
const OP_RECORD_DIR: &[u8] = b"kitty-ops/";
/// The record file suffix.
const OP_RECORD_SUFFIX: &[u8] = b".jsonl";
/// The number of Crockford characters in an Op invocation id.
const OP_RECORD_ID_LEN: usize = 26;
/// The unresolved-primary clause shared by the skip reason and its docs.
const UNRESOLVED_PRIMARY: &str = "primary branch unresolved";
/// The best-effort skip prefix used when a probe needed for the decision fails.
const PROBE_FAILED_PREFIX: &str = "git probe failed while deciding whether to commit the Op record";

/// Whether the primary branch tracks Op records, and which ref was asked.
///
/// This is the reference's frozen dataclass: `primary` is the short name of the
/// first resolved ref, or `None` when nothing resolved. `tracked` can be
/// `false` with `primary` set: the ref resolved but its tree holds no
/// Op-record-shaped file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrimaryTracking {
    tracked: bool,
    primary: Option<String>,
}

impl PrimaryTracking {
    /// Build an answer from its two source fields.
    ///
    /// Any combination is representable: unresolved is `(false, None)`, a
    /// resolved primary with no Op record is `(false, Some(name))`.
    pub fn new(tracked: bool, primary: Option<String>) -> Self {
        Self { tracked, primary }
    }

    /// Whether any resolved primary ref tracks an Op-record-shaped file.
    pub fn tracked(&self) -> bool {
        self.tracked
    }

    /// The short name of the resolved primary ref, or `None` when unresolved.
    pub fn primary(&self) -> Option<&str> {
        self.primary.as_deref()
    }
}

/// A Git probe needed for the commit-or-skip decision failed.
///
/// The variant names the phase that failed, so a caller can report which
/// question was undecidable instead of receiving a quiet `false` or an
/// invented commit/refusal. The reference raises one `OpCommitProbeError` for
/// the same cases; here the phase is carried in the type.
#[derive(Debug, Error)]
pub enum OpCommitProbeError {
    /// Reading `refs/remotes/origin/HEAD` could not be completed.
    #[error("git symbolic-ref could not run: {source}")]
    SymbolicRef {
        /// The typed Git failure.
        #[source]
        source: GitError,
    },

    /// Probing whether a resolved candidate ref exists could not be completed.
    #[error("git rev-parse could not run for {reference}: {source}")]
    RefExistence {
        /// The candidate ref whose existence was being probed.
        reference: RefName,
        /// The typed Git failure.
        #[source]
        source: GitError,
    },

    /// Listing the `kitty-ops/` tree of a resolved primary ref failed.
    #[error("listing kitty-ops/ on {reference} failed: {source}")]
    TreeListing {
        /// The resolved primary ref whose tree was being read.
        reference: RefName,
        /// The typed Git failure.
        #[source]
        source: GitError,
    },

    /// The exact-path ignore probe failed.
    #[error("git check-ignore {path} failed: {source}")]
    IgnoreProbe {
        /// The path spelling the probe was asked about.
        path: String,
        /// The typed Git failure.
        #[source]
        source: GitError,
    },
}

/// Whether `path` has the per-Op record shape.
///
/// Mirrors the reference's `OP_RECORD_PATH_RE` search:
/// `(?:^|/)kitty-ops/[0-9A-HJKMNP-TV-Z]{26}\.jsonl$`. The directory may appear
/// at any depth; the id is exactly 26 upper-case Crockford characters
/// (`I`, `L`, `O` and `U` excluded) and is never decoded as a ULID, so
/// `Z`×26 and `0`×26 count. A single trailing newline also matches, because
/// Python's `$` anchor matches before one final newline; that quirk is
/// preserved deliberately.
///
/// The bytes are the exact Git path bytes (lossily decoded only on a
/// non-Unix platform, where a non-ASCII/non-UTF-8 spelling cannot form this
/// ASCII shape anyway).
pub fn is_op_record_path(path: &GitPath) -> bool {
    let spelling: Cow<'_, [u8]> = raw_path_bytes(path);
    let spelling = spelling.strip_suffix(b"\n").unwrap_or(&spelling);
    let Some(stem) = spelling.strip_suffix(OP_RECORD_SUFFIX) else {
        return false;
    };
    let Some(id_start) = stem.len().checked_sub(OP_RECORD_ID_LEN) else {
        return false;
    };
    let Ok(id) = std::str::from_utf8(&stem[id_start..]) else {
        return false;
    };
    if crate::InvocationId::parse(id).is_err() {
        return false;
    }
    let Some(dir_start) = id_start.checked_sub(OP_RECORD_DIR.len()) else {
        return false;
    };
    if &stem[dir_start..id_start] != OP_RECORD_DIR {
        return false;
    }
    dir_start == 0 || stem[dir_start - 1] == b'/'
}

/// The exact platform bytes of a Git path.
fn raw_path_bytes(path: &GitPath) -> Cow<'_, [u8]> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Cow::Borrowed(path.as_os_str().as_bytes())
    }
    #[cfg(not(unix))]
    {
        Cow::Owned(path.display_lossy().into_owned().into_bytes())
    }
}

/// Resolve the existing refs of the primary branch (local, then `origin`).
///
/// Resolution is the reference's, exactly: the `refs/remotes/origin/HEAD` short
/// target (with `origin/` stripped) is probed as `refs/heads/<name>` and
/// `refs/remotes/origin/<name>`; when neither exists, or when the symbolic read
/// has no usable answer, `main`/`master`/`develop` are tried in order, skipping
/// the already-probed name. Returns an empty vector when nothing resolves. It
/// never falls back to the checked-out branch.
///
/// At most nine Git calls are made before any tree is listed: one
/// `symbolic-ref`, two for a dangling name, and six for the ladder.
///
/// # Errors
///
/// A symbolic-ref read, a ref-existence probe, or its underlying git process
/// that could not be completed (never started, timed out, capture/read
/// failure) is [`OpCommitProbeError`]. A completed non-zero Git answer is
/// "this ref does not exist", not an error.
pub fn resolve_primary_refs(
    repository: &GitRepository,
) -> Result<Vec<RefName>, OpCommitProbeError> {
    let origin_head =
        RefName::parse(ORIGIN_HEAD_REF).expect("the origin/HEAD ref name is a constant");
    let symbolic = match repository.symbolic_ref_short_target(&origin_head) {
        Ok(target) => target,
        // The reference treats every completed non-zero symbolic-ref answer as
        // "no usable target" and falls through to the ladder.
        Err(GitError::CommandFailed { .. }) => None,
        Err(source) => return Err(OpCommitProbeError::SymbolicRef { source }),
    };
    let probed = symbolic.and_then(|target| {
        // Python strips the whole stdout before removeprefix. Unicode
        // whitespace is legal in ref names, so changing this order changes
        // real branch selection. Leave the generic Git read byte-faithful.
        let target = target.trim_matches(|character: char| {
            character.is_whitespace() || matches!(character, '\u{1c}'..='\u{1f}')
        });
        let name = target.strip_prefix(ORIGIN_PREFIX).unwrap_or(target);
        if name.is_empty() {
            None
        } else {
            Some(name.to_string())
        }
    });

    if let Some(name) = probed.as_deref() {
        let refs = existing_refs(repository, name)?;
        if !refs.is_empty() {
            return Ok(refs);
        }
    }

    for candidate in COMMON_PRIMARY_BRANCHES {
        if probed.as_deref() == Some(candidate) {
            continue;
        }
        let refs = existing_refs(repository, candidate)?;
        if !refs.is_empty() {
            return Ok(refs);
        }
    }
    Ok(Vec::new())
}

/// The existing local and `origin` refs of one short branch name.
fn existing_refs(
    repository: &GitRepository,
    name: &str,
) -> Result<Vec<RefName>, OpCommitProbeError> {
    let mut refs = Vec::new();
    for prefix in [LOCAL_BRANCH_PREFIX, ORIGIN_BRANCH_PREFIX] {
        let Ok(reference) = RefName::parse(&format!("{prefix}{name}")) else {
            // Invalid ref syntax cannot name an existing ref. Length is not
            // guessed here: canonical RefName validation has no arbitrary cap;
            // Git/OS operational limits remain backend probe outcomes.
            continue;
        };
        if ref_exists(repository, &reference)? {
            refs.push(reference);
        }
    }
    Ok(refs)
}

/// Whether `reference` peels to a commit, under the reference's answer rules.
fn ref_exists(repository: &GitRepository, reference: &RefName) -> Result<bool, OpCommitProbeError> {
    match repository.resolve_commit(&StartPoint::Ref(reference.clone())) {
        Ok(_) => Ok(true),
        // Every completed non-zero answer is "absent" in the reference,
        // including a signalled child and an unexpected exit code. Only the
        // process-level failures below are probe errors.
        Err(GitError::RefMissing { .. } | GitError::CommandFailed { .. }) => Ok(false),
        Err(source) => Err(OpCommitProbeError::RefExistence {
            reference: reference.clone(),
            source,
        }),
    }
}

/// Whether any of `refs` tracks a `kitty-ops/<id>.jsonl` file.
///
/// Each ref's `kitty-ops/` subtree is listed with the literal pathspec, in
/// order, and the first Op-record-shaped path wins. An empty `refs` list is
/// `false`; this function does not resolve anything.
///
/// # Errors
///
/// A tree listing that could not be completed is
/// [`OpCommitProbeError::TreeListing`]. A failed listing is never "no records".
pub fn primary_tracks_op_records(
    repository: &GitRepository,
    refs: &[RefName],
) -> Result<bool, OpCommitProbeError> {
    let pathspec =
        GitPath::from_git_bytes(OP_EVENTS_PATHSPEC).expect("the kitty-ops/ pathspec is a constant");
    for reference in refs {
        let paths = repository
            .tree_paths_at(&StartPoint::Ref(reference.clone()), Some(&pathspec))
            .map_err(|source| OpCommitProbeError::TreeListing {
                reference: reference.clone(),
                source,
            })?;
        if paths.iter().any(is_op_record_path) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Resolve the primary branch and ask whether it tracks Op records.
///
/// This is [`resolve_primary_refs`] followed by [`primary_tracks_op_records`],
/// with the short primary name taken from the first resolved ref. No result is
/// cached here; [`OpCommitSkipProbe`] owns the caller-scoped cache.
///
/// # Errors
///
/// Any resolution or tree-listing probe failure is [`OpCommitProbeError`].
pub fn probe_primary_tracking(
    repository: &GitRepository,
) -> Result<PrimaryTracking, OpCommitProbeError> {
    let refs = resolve_primary_refs(repository)?;
    if refs.is_empty() {
        return Ok(PrimaryTracking::new(false, None));
    }
    let tracked = primary_tracks_op_records(repository, &refs)?;
    Ok(PrimaryTracking::new(
        tracked,
        Some(primary_short_name(&refs[0])),
    ))
}

/// The short branch name of a resolved primary ref.
///
/// The reference chains `removeprefix("refs/heads/")` and
/// `removeprefix("refs/remotes/origin/")`; this mirrors that exactly, including
/// the second strip after a successful first one.
fn primary_short_name(reference: &RefName) -> String {
    let name = reference
        .as_str()
        .strip_prefix(LOCAL_BRANCH_PREFIX)
        .unwrap_or(reference.as_str());
    name.strip_prefix(ORIGIN_BRANCH_PREFIX)
        .unwrap_or(name)
        .to_string()
}

/// The reason to skip the Op-record auto-commit, or `None` to commit it.
///
/// The ignore check comes first, exactly as in the reference: a gitignored
/// exact path is skipped even when the primary branch tracks Op records. Then
/// an untracked primary is skipped either as unresolved or as a named primary
/// with no Op records under `kitty-ops/`.
///
/// `path` is the record path about to be committed (repository-relative). Its
/// literal Git spelling is used for the ignore probe and in the reason string;
/// the Op-record shape is deliberately not re-validated here, matching the
/// reference, which accepts any relative path.
///
/// # Errors
///
/// A failed ignore probe is [`OpCommitProbeError::IgnoreProbe`]. A failure is
/// never turned into `None` (commit) or `Some` (a made-up skip); the best-effort
/// mapping belongs to [`OpCommitSkipProbe`].
pub fn op_commit_skip_reason(
    repository: &GitRepository,
    path: &GitPath,
    tracking: &PrimaryTracking,
) -> Result<Option<String>, OpCommitProbeError> {
    let spelling = path.display_lossy();
    if repository
        .is_path_ignored(path)
        .map_err(|source| OpCommitProbeError::IgnoreProbe {
            path: spelling.clone(),
            source,
        })?
    {
        return Ok(Some(format!(
            "{spelling} matches an ignore rule in this repository"
        )));
    }
    if !tracking.tracked {
        return Ok(Some(tracking_reason(tracking)));
    }
    Ok(None)
}

/// The tracking-based skip clause, matching the reference's wording.
fn tracking_reason(tracking: &PrimaryTracking) -> String {
    match tracking.primary() {
        None => format!("{UNRESOLVED_PRIMARY}, so it cannot be shown to track Op records"),
        Some(primary) => {
            format!("the primary branch ({primary}) tracks no Op records under kitty-ops/")
        }
    }
}

/// The reference's best-effort skip reason for a failed probe.
fn probe_failed_reason(error: &OpCommitProbeError) -> String {
    format!("{PROBE_FAILED_PREFIX}: {error}")
}

/// A caller-owned cache for the commit-or-skip decision.
///
/// Mirrors `executor._op_commit_skip_reason`: the primary-tracking question is
/// probed and cached **first**, then [`op_commit_skip_reason`] is asked, whose
/// ignore check precedes the tracking-based reasons. A probe failure becomes
/// the reference's best-effort skip reason and is never a commit, a success or
/// a write refusal.
///
/// The cache is per value, not global: tracking is computed at most once after
/// a success and reused for the rest of that value's life. A successful
/// tracking result survives a later ignore-probe failure (the tracking answer
/// was still valid); a failed tracking probe is not cached, so the next call
/// retries it. A retired value cannot leak an answer into another checkout.
///
/// # Example
///
/// ```no_run
/// # fn main() -> Result<(), sk_git::GitError> {
/// # let repository = sk_git::GitRepository::open(".")?;
/// # let record = sk_git::GitPath::from_git_bytes(b"kitty-ops/01ARZ3NDEKTSV4RRFFQ69G5FAV.jsonl")?;
/// let mut probe = sk_invocation::OpCommitSkipProbe::new(&repository);
/// match probe.skip_reason(&record) {
///     // Commit is eligible: the exact path is not ignored and the primary
///     // branch tracks Op records.
///     None => {}
///     // Skip (or best-effort skip after a probe failure); report the reason.
///     Some(reason) => println!("skipping the Op-record commit: {reason}"),
/// }
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct OpCommitSkipProbe<'a> {
    repository: &'a GitRepository,
    tracking: Option<PrimaryTracking>,
}

impl<'a> OpCommitSkipProbe<'a> {
    /// A probe cache for one repository handle.
    pub fn new(repository: &'a GitRepository) -> Self {
        Self {
            repository,
            tracking: None,
        }
    }

    /// The cached tracking answer, if a primary probe already succeeded.
    ///
    /// `None` before the first decision and after a failed tracking probe.
    pub fn tracking(&self) -> Option<&PrimaryTracking> {
        self.tracking.as_ref()
    }

    /// The reason to skip the auto-commit, or `None` when it is eligible.
    ///
    /// This never returns a typed error: a failed probe is the reference's
    /// best-effort skip reason, because the commit is best-effort and an
    /// undecidable question must not turn into an unwanted commit.
    pub fn skip_reason(&mut self, path: &GitPath) -> Option<String> {
        let tracking = match &self.tracking {
            Some(cached) => cached.clone(),
            None => match probe_primary_tracking(self.repository) {
                Ok(fresh) => {
                    self.tracking = Some(fresh.clone());
                    fresh
                }
                Err(error) => return Some(probe_failed_reason(&error)),
            },
        };
        match op_commit_skip_reason(self.repository, path, &tracking) {
            Ok(reason) => reason,
            // Keep the successful tracking cache: only the ignore question
            // failed, and the tracking answer remains the source's answer.
            Err(error) => Some(probe_failed_reason(&error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git_path(raw: &str) -> GitPath {
        GitPath::from_git_bytes(raw.as_bytes()).expect("valid path")
    }

    #[test]
    fn crockford_ids_are_not_decoded_ulids() {
        assert!(is_op_record_path(&git_path(
            "kitty-ops/ZZZZZZZZZZZZZZZZZZZZZZZZZZ.jsonl"
        )));
        assert!(is_op_record_path(&git_path(
            "kitty-ops/00000000000000000000000000.jsonl"
        )));
    }

    #[test]
    fn the_directory_anchor_is_a_component_boundary() {
        let id = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
        assert!(is_op_record_path(&git_path(&format!(
            "x/kitty-ops/{id}.jsonl"
        ))));
        assert!(!is_op_record_path(&git_path(&format!(
            "xkitty-ops/{id}.jsonl"
        ))));
        assert!(!is_op_record_path(&git_path(&format!(
            "kitty-opsx/{id}.jsonl"
        ))));
    }

    #[test]
    fn primary_short_names_mirror_the_chained_strips() {
        assert_eq!(
            primary_short_name(&RefName::parse("refs/heads/main").unwrap()),
            "main"
        );
        assert_eq!(
            primary_short_name(&RefName::parse("refs/remotes/origin/main").unwrap()),
            "main"
        );
        assert_eq!(
            primary_short_name(&RefName::parse("refs/heads/refs/remotes/origin/x").unwrap()),
            "x"
        );
    }
}
