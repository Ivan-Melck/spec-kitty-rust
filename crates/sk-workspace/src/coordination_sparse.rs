//! Canonical lane sparse-checkout policy and read-only lane drift facts.
//!
//! A lane worktree is configured so that the Mission's coordination event log
//! and its derived snapshot are **absent** from the lane filesystem. That is how
//! a lane process is physically prevented from writing the log: it cannot open a
//! file that does not exist. This module owns the two pieces of that contract the
//! native tree was missing, plus the read-only facts the health surface reports:
//!
//! 1. **The policy** — [`LaneSparsePolicy`] /
//!    [`lane_sparse_checkout_patterns`]: the three non-cone patterns in the
//!    reference's exact order and spelling, composed through the one coordination
//!    directory grammar ([`crate::naming::coord_mission_dir_name`], which is
//!    verbatim and idempotent on an already-suffixed slug, so a legacy `NNN-`
//!    Mission reconstructs the directory that actually exists on disk).
//! 2. **The recognizer** — [`crate::naming::lane_id_for_worktree_dir`]: a
//!    `.worktrees/` entry belongs to this Mission only when re-composing its
//!    parsed lane id through the one worktree builder reproduces the entry name
//!    byte for byte. A `starts_with` check would over-match a same-prefix
//!    impostor (`057-foobar-lane-a` is not `057-foo`'s worktree).
//! 3. **The drift facts** — [`scan_lane_sparse_drift`] returns typed
//!    [`LaneSparseDrift`] values for the parent health owner to render; it does
//!    not define a second finding or report model.
//!
//! # Composition, not re-implementation
//!
//! * the mission directory comes from [`crate::naming`] (never string surgery);
//! * the per-worktree pattern file is read through
//!   [`sk_git::GitRepository::sparse_pattern_file_text_source`] — the reference's own
//!   read: the canonical per-worktree `rev-parse --git-path info/sparse-checkout`
//!   resolution, an ordinary unbounded file read that follows a final symlink, exact
//!   UTF-8, and universal-newline decoding. There is no private Git process, path
//!   guess or pattern parser;
//! * worktree registration is the reference's own observation:
//!   [`sk_git::GitRepository::worktree_porcelain_observation`] returns the literal
//!   `git worktree list --porcelain` text *and* its completion status, so membership
//!   can be the reference's `str(lane.resolve()) in <text>` substring test and a
//!   failed listing can be handled the way the reference handles it;
//! * line splitting and whitespace stripping reuse [`sk_git`]'s Source pattern reader,
//!   which is the canonical `splitlines()` + `strip()` composition.
//!
//! # Read-only contract
//!
//! Nothing here creates `.worktrees`, a lane worktree, a pattern file, a lock or
//! an index refresh, and nothing writes: the reference's writer
//! (`register_lane_sparse_checkout`) and its `--fix` arm belong to the allocation
//! and `doctor workspaces --fix` owners. [`LaneSparsePolicy::pattern_file_text`]
//! only exposes the bytes such a writer must author.
//!
//! # Reference semantics preserved
//!
//! * Only **absent** expected patterns are drift. Comments, blank lines, extra
//!   patterns, reordering and surrounding whitespace are all tolerated, exactly
//!   because the reference compares a set of stripped, non-empty lines.
//! * The missing set is reported sorted, as the reference does.
//! * A `git worktree list` that exits non-zero is the reference's own `except`
//!   arm: the listing is empty, every lane is skipped, and the check reports
//!   healthy. That is reproduced here deliberately (Source catch quirks are not
//!   "improved" into a fail-closed verdict), and the typed error is reserved for
//!   the failures the reference does **not** catch (a spawn/timeout/capture fault,
//!   or a listing that is not valid UTF-8).

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use sk_git::{GitError, GitRepository};

use crate::naming::{WORKTREES_DIRNAME, coord_mission_dir_name};

/// The finding code the reference emits for every lane sparse-checkout drift.
pub const LANE_SPARSE_CHECKOUT_DRIFT: &str = "LANE_SPARSE_CHECKOUT_DRIFT";

/// The include-everything pattern the policy starts from.
pub const LANE_SPARSE_INCLUDE_ALL: &str = "/*";

/// Basename of the coordination event log a lane must not be able to open.
pub const STATUS_EVENTS_BASENAME: &str = "status.events.jsonl";

/// Basename of the derived status snapshot a lane must not be able to open.
pub const STATUS_SNAPSHOT_BASENAME: &str = "status.json";

/// The canonical lane sparse-checkout policy for one Mission.
///
/// Non-cone mode only: cone mode has no negation patterns, so a caller that
/// applies this policy must switch the lane to `--no-cone` first (the reference
/// writer's step 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneSparsePolicy {
    mission_dir: String,
    patterns: Vec<String>,
}

impl LaneSparsePolicy {
    /// Compose the policy for `mission_slug` with a resolved `mid8`.
    ///
    /// `mission_slug` is consumed verbatim through
    /// [`coord_mission_dir_name`]: an already-suffixed slug is not suffixed
    /// again, and a legacy `NNN-` prefix is **not** stripped (the coordination
    /// grammar reconstructs the directory that exists on disk).
    pub fn for_mission(mission_slug: &str, mid8: &str) -> Self {
        let mission_dir = coord_mission_dir_name(mission_slug, mid8);
        let patterns = vec![
            LANE_SPARSE_INCLUDE_ALL.to_owned(),
            format!("!kitty-specs/{mission_dir}/{STATUS_EVENTS_BASENAME}"),
            format!("!kitty-specs/{mission_dir}/{STATUS_SNAPSHOT_BASENAME}"),
        ];
        Self {
            mission_dir,
            patterns,
        }
    }

    /// The `<slug>-<mid8>` mission directory the exclusions point at.
    pub fn mission_dir(&self) -> &str {
        &self.mission_dir
    }

    /// The patterns, in the reference's order.
    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }

    /// The exact bytes a lane's `info/sparse-checkout` file must contain.
    ///
    /// One pattern per line with a trailing newline — the authored form, not a
    /// normalised or re-serialised one. Provided for the writer owner; this
    /// module never writes it.
    pub fn pattern_file_text(&self) -> String {
        let mut text = String::with_capacity(
            self.patterns.iter().map(String::len).sum::<usize>() + self.patterns.len(),
        );
        for pattern in &self.patterns {
            text.push_str(pattern);
            text.push('\n');
        }
        text
    }
}

/// The canonical lane sparse-checkout patterns for one Mission.
///
/// The reference's `lane_sparse_checkout_patterns(mission_slug, mid8)` returns
/// exactly these three strings in this order:
/// `/*`, `!kitty-specs/<slug>-<mid8>/status.events.jsonl`,
/// `!kitty-specs/<slug>-<mid8>/status.json`.
pub fn lane_sparse_checkout_patterns(mission_slug: &str, mid8: &str) -> Vec<String> {
    LaneSparsePolicy::for_mission(mission_slug, mid8).patterns
}

/// One typed lane sparse-checkout drift fact.
///
/// The three shapes mirror the reference's three findings for this check. The
/// parent health owner renders them (message, remedy, `error_code`,
/// `extra.missing_patterns`); no finding model is copied here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaneSparseDrift {
    /// The lane's per-worktree pattern path could not be resolved or read.
    PolicyPathUnresolvable {
        /// The lane worktree directory.
        lane: PathBuf,
    },
    /// The path resolved, but the pattern file does not exist.
    PolicyFileMissing {
        /// The lane worktree directory.
        lane: PathBuf,
    },
    /// The pattern file exists but is missing expected patterns.
    PatternsMissing {
        /// The lane worktree directory.
        lane: PathBuf,
        /// The absent patterns, sorted (never empty).
        missing: Vec<String>,
    },
}

impl LaneSparseDrift {
    /// The lane worktree this fact belongs to.
    pub fn lane(&self) -> &Path {
        match self {
            Self::PolicyPathUnresolvable { lane }
            | Self::PolicyFileMissing { lane }
            | Self::PatternsMissing { lane, .. } => lane,
        }
    }

    /// The absent patterns, or `None` for the two file-level facts.
    pub fn missing_patterns(&self) -> Option<&[String]> {
        match self {
            Self::PolicyPathUnresolvable { .. } | Self::PolicyFileMissing { .. } => None,
            Self::PatternsMissing { missing, .. } => Some(missing),
        }
    }
}

/// The result of one read-only lane sparse-checkout scan.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaneSparseScan {
    drifts: Vec<LaneSparseDrift>,
    inspected_lanes: usize,
}

impl LaneSparseScan {
    /// The drift facts, ordered by lane directory name.
    pub fn drifts(&self) -> &[LaneSparseDrift] {
        &self.drifts
    }

    /// How many lane worktrees of this Mission were actually inspected.
    ///
    /// A `.worktrees/` entry that is not this Mission's lane directory, or that
    /// is not a registered Git worktree, is not inspected. Diagnostic only: the
    /// reference's `ok` message does not carry a count.
    pub fn inspected_lanes(&self) -> usize {
        self.inspected_lanes
    }

    /// Whether every inspected lane carries the expected policy.
    pub fn is_clean(&self) -> bool {
        self.drifts.is_empty()
    }
}

/// A failure that stopped the scan from answering at all.
///
/// Distinct from a drift fact: this means "the check did not run", never "the
/// lanes are healthy".
#[derive(Debug, thiserror::Error)]
pub enum LaneSparseError {
    /// A directory read failed.
    #[error("could not read the lane worktree directory {path}: {source}")]
    Io {
        /// The directory involved.
        path: PathBuf,
        /// The operating-system error.
        #[source]
        source: std::io::Error,
    },
    /// The shared worktree-registration or pattern observation failed.
    ///
    /// This is the failure the reference does **not** catch (its `except` covers
    /// only a non-zero `git worktree list` / `rev-parse` exit). It is reported, never
    /// turned into a guessed drift finding or a healthy row.
    #[error("could not observe the lane sparse-checkout state: {0}")]
    Git(#[from] GitError),
    /// A lane's pattern-file observation failed where the reference also refuses.
    ///
    /// The reference checks `Path.exists()` (which ignores `ENOENT`/`ENOTDIR`/`ELOOP`
    /// but re-raises any other stat error) and then `read_text()`. A stat error, a
    /// read error or a decode error therefore escapes its check rather than producing
    /// a finding; the canonical facade reports the same phase as a typed error, and it
    /// must not become `LANE_SPARSE_CHECKOUT_DRIFT`.
    #[error("could not read the sparse-checkout pattern file of {lane}: {source}")]
    PatternUnreadable {
        /// The lane worktree the file belongs to.
        lane: PathBuf,
        /// The reader's own cause.
        #[source]
        source: GitError,
    },
}

/// Scan every lane worktree of one Mission for sparse-checkout drift.
///
/// Returns an empty, clean scan when `<repo>/.worktrees/` does not exist (the
/// reference returns no findings in that case) and when `git worktree list` exits
/// non-zero (the reference's own `except` arm — see the module docs). Read-only: no
/// worktree is materialized, no pattern file is written, no lock is taken and no
/// index is refreshed.
///
/// `mission_slug` must be the Mission's recorded slug and `mid8` its resolved
/// disambiguator (the reference resolves it as `resolve_mid8(...) or
/// mission_id[:8]`); both are inputs because resolving identity is the Mission
/// owner's job, not this module's.
pub fn scan_lane_sparse_drift(
    repo: &GitRepository,
    mission_slug: &str,
    mid8: &str,
) -> Result<LaneSparseScan, LaneSparseError> {
    let policy = LaneSparsePolicy::for_mission(mission_slug, mid8);
    let expected: BTreeSet<&str> = policy.patterns().iter().map(String::as_str).collect();

    let worktrees_dir = repo.root().join(WORKTREES_DIRNAME);
    if !worktrees_dir.exists() {
        return Ok(LaneSparseScan::default());
    }

    let mut names: Vec<std::ffi::OsString> = Vec::new();
    let entries = std::fs::read_dir(&worktrees_dir).map_err(|source| LaneSparseError::Io {
        path: worktrees_dir.clone(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| LaneSparseError::Io {
            path: worktrees_dir.clone(),
            source,
        })?;
        names.push(entry.file_name());
    }
    names.sort_by(|left, right| directory_name_sort_key(left).cmp(&directory_name_sort_key(right)));

    // The reference's own registration observation: one `git worktree list
    // --porcelain`, its literal text, and its completion status. A non-zero exit is
    // the reference's `except CalledProcessError: wt_list = ""` arm, which skips
    // every lane; a spawn/timeout/capture fault or a non-UTF-8 listing is the
    // failure the reference does *not* catch, and stays typed here.
    let observation = repo.worktree_porcelain_observation()?;
    let listing = if observation.succeeded() {
        observation.stdout_text()?.to_owned()
    } else {
        String::new()
    };

    let mut scan = LaneSparseScan::default();
    if listing.is_empty() {
        // Nothing is registered (or the listing failed, exactly as the reference).
        return Ok(scan);
    }

    for name in &names {
        if !is_mission_lane_dir(name, mission_slug) {
            continue;
        }
        let lane_dir = worktrees_dir.join(name);
        // `str(lane_dir.resolve()) not in wt_list` — the reference's substring test
        // over the porcelain text. A path that cannot be resolved cannot be a
        // registered worktree either.
        let Ok(resolved) = std::fs::canonicalize(&lane_dir) else {
            continue;
        };
        if !listing.contains(&resolved.display().to_string()) {
            continue;
        }
        scan.inspected_lanes += 1;
        if let Some(drift) = scan_one_lane(&lane_dir, &expected)? {
            scan.drifts.push(drift);
        }
    }
    Ok(scan)
}

/// The pattern-file facts for one lane worktree, or `None` when it is healthy.
fn scan_one_lane(
    lane_dir: &Path,
    expected: &BTreeSet<&str>,
) -> Result<Option<LaneSparseDrift>, LaneSparseError> {
    let lane = || lane_dir.to_path_buf();
    let Ok(lane_repo) = GitRepository::open(lane_dir) else {
        // The reference reports a failed `git -C <lane> rev-parse --git-path`
        // this way; a lane whose repository cannot be opened cannot answer it.
        return Ok(Some(LaneSparseDrift::PolicyPathUnresolvable {
            lane: lane(),
        }));
    };
    let patterns = match lane_repo.sparse_pattern_file_patterns_source() {
        // Present: the reference's `{line.strip() for line in read_text().splitlines()
        // if line.strip()}` set, already composed by the canonical Source reader.
        Ok(Some(patterns)) => patterns,
        // `Path.exists()` was false.
        Ok(None) => return Ok(Some(LaneSparseDrift::PolicyFileMissing { lane: lane() })),
        // The reference's caught arm: `rev-parse --git-path` exited non-zero.
        Err(GitError::CommandFailed { .. }) => {
            return Ok(Some(LaneSparseDrift::PolicyPathUnresolvable {
                lane: lane(),
            }));
        }
        // Everything else is a read/decode failure the reference lets escape its
        // check. It must not become a drift finding or a healthy row.
        Err(source) => {
            return Err(LaneSparseError::PatternUnreadable {
                lane: lane(),
                source,
            });
        }
    };

    let mut missing: Vec<String> = expected
        .iter()
        .filter(|pattern| !patterns.iter().any(|present| present == **pattern))
        .map(|pattern| (*pattern).to_owned())
        .collect();
    if missing.is_empty() {
        return Ok(None);
    }
    missing.sort();
    Ok(Some(LaneSparseDrift::PatternsMissing {
        lane: lane(),
        missing,
    }))
}

/// Python `str.strip()`: strip code points where `str.isspace()` is true.
#[cfg(test)]
fn strip_python_whitespace(text: &str) -> &str {
    use sk_kernel::python_text::is_whitespace;
    let start = text
        .char_indices()
        .find(|(_, character)| !is_whitespace(*character as u32))
        .map(|(index, _)| index)
        .unwrap_or(text.len());
    let end = text
        .char_indices()
        .rev()
        .find(|(_, character)| !is_whitespace(*character as u32))
        .map(|(index, character)| index + character.len_utf8())
        .unwrap_or(start);
    &text[start..end.max(start)]
}

/// Whether a `.worktrees/` entry name is a lane-worktree directory of this
/// Mission, decided by recomposition on the directory name's raw bytes.
fn is_mission_lane_dir(name: &OsStr, mission_slug: &str) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        crate::naming::lane_id_for_worktree_dir_bytes(name.as_bytes(), mission_slug).is_some()
    }
    #[cfg(not(unix))]
    {
        name.to_str().is_some_and(|name| {
            crate::naming::lane_id_for_worktree_dir(name, mission_slug).is_some()
        })
    }
}

/// The ordering key for directory names.
///
/// The reference sorts `Path` objects, which for one parent directory compares
/// the decoded names. On Unix the raw bytes are compared, preserving non-UTF-8
/// names exactly; Python's `surrogateescape` decoding can order two DIFFERENT
/// non-UTF-8 names differently from that, so a mixed invalid-UTF-8 set may be
/// listed in a different order than the reference's — the fact set is identical.
fn directory_name_sort_key(name: &OsStr) -> Cow<'_, [u8]> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Cow::Borrowed(name.as_bytes())
    }
    #[cfg(not(unix))]
    {
        Cow::Owned(name.to_string_lossy().into_owned().into_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_matches_the_reference_strings() {
        let policy = LaneSparsePolicy::for_mission("cf39-mission", "01KNXQS9");
        assert_eq!(policy.mission_dir(), "cf39-mission-01KNXQS9");
        assert_eq!(
            policy.patterns(),
            [
                "/*",
                "!kitty-specs/cf39-mission-01KNXQS9/status.events.jsonl",
                "!kitty-specs/cf39-mission-01KNXQS9/status.json",
            ]
        );
        assert_eq!(
            policy.pattern_file_text(),
            "/*\n!kitty-specs/cf39-mission-01KNXQS9/status.events.jsonl\n\
             !kitty-specs/cf39-mission-01KNXQS9/status.json\n"
        );
        assert_eq!(
            lane_sparse_checkout_patterns("cf39-mission", "01KNXQS9"),
            policy.patterns()
        );
    }

    #[test]
    fn policy_suffix_is_idempotent_and_legacy_prefix_is_verbatim() {
        // Already suffixed: no double suffix.
        assert_eq!(
            LaneSparsePolicy::for_mission("cf39-mission-01KNXQS9", "01KNXQS9").mission_dir(),
            "cf39-mission-01KNXQS9"
        );
        // Legacy `NNN-` slug: the coordination grammar keeps the prefix, because
        // the on-disk directory was created with it.
        assert_eq!(
            LaneSparsePolicy::for_mission("057-cf39", "01KNXQS9").mission_dir(),
            "057-cf39-01KNXQS9"
        );
    }

    #[test]
    fn stripping_uses_python_whitespace() {
        assert_eq!(strip_python_whitespace("  /*\t"), "/*");
        assert_eq!(strip_python_whitespace("\u{1c}!a\u{3000}"), "!a");
        assert_eq!(strip_python_whitespace("\u{a0}"), "");
        assert_eq!(strip_python_whitespace(""), "");
        assert_eq!(strip_python_whitespace("a b"), "a b");
    }
}
