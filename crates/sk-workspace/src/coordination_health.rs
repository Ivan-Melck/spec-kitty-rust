//! Read-only coordination + Git-health diagnosis (`spec-kitty doctor coordination`).
//!
//! This module is the native counterpart of the reference's
//! `cli/commands/_coordination_doctor.py` **read-only** arm. It answers the
//! four questions that arm asks, in the reference's order:
//!
//! 1. is the installed Git new enough for the non-cone sparse surface
//!    (`GIT_VERSION_UNDECIDABLE` / `GIT_VERSION_TOO_OLD` / `ok`)?
//! 2. is any `.worktrees/` content tracked in the repository index?
//! 3. for every Mission that declares a `coordination_branch`: does its
//!    coordination worktree exist, sit on that branch, stay clean, and keep up
//!    with its own branch tip?
//! 4. (opt-in, `--check-staleness`) has the coordination **branch** fallen
//!    behind the branch it publishes onto?
//!
//! # Composition, not re-implementation
//!
//! Every fact is read through an existing canonical authority:
//!
//! * Git version, ref tips, ancestry, `HEAD`, status and the index all come
//!   from [`sk_git::GitRepository`] — no private Git process, parser or branch
//!   roster;
//! * remote-branch presence comes from the shared `remote_branch_lookup`
//!   primitive (the same tri-state the write path uses);
//! * the coordination worktree path is composed by [`crate::naming`] (the one
//!   name grammar), never by string surgery on a directory name;
//! * `meta.json` is read by the crate's existing classify-never-crash reader
//!   ([`crate::doctor::meta`]), so a corrupt or absent manifest is skipped the
//!   way the reference's `load_meta(on_malformed="none")` skips it;
//! * `--mission` selection uses [`sk_mission::select_mission`], so handles
//!   resolve exactly as every other command resolves them.
//!
//! # Read-only contract
//!
//! Nothing here materializes a worktree, seeds a coordination surface, writes
//! `meta.json`, backfills topology, fast-forwards a branch, reverts a commit,
//! takes a lock, or refreshes the index (`--no-optional-locks`). The
//! reference's `--fix` arm is a mutation owner that does not exist in this
//! lane; it is refused by the adapter before any read.
//!
//! # Checks that cannot be composed yet are declared, never dropped
//!
//! Two reference checks have no canonical native owner yet, so this module
//! reports them as [`UnavailableCheck`]s instead of inventing a verdict:
//!
//! * the lane sparse-checkout drift check needs the coordination sparse
//!   **pattern** policy and the lane-worktree recomposition recognizer
//!   (`coordination.lane_sparse_checkout_patterns` /
//!   `lanes.branch_naming.lane_id_for_worktree_dir`);
//! * the stranded-`done` re-verification needs the consolidation
//!   `pending_coord_reconcile` marker reader and the coordination coherence
//!   reducer (`consolidation.state.iter_pending_coord_reconcile_markers` /
//!   `coordination.coherence.coord_incoherent_done_wps`).
//!
//! A report carrying any unavailable check is **incomplete**: the adapter
//! refuses it rather than printing a partial report that could be read as a
//! passing health check. `--check-staleness` adds a third unavailable check
//! (the `missions._create.is_expected_coordination_divergence` exemption); when
//! a coordination branch neither leads nor trails its target, no `DIVERGED`
//! finding is emitted, because that verdict belongs to the exemption's owner.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use sk_git::{BranchName, GitError, GitRepository, RemoteBranchLookup};
use sk_mission::MissionError;

use crate::doctor::meta::{python_truthy, read_meta};
use crate::error::WorkspaceError;
use crate::naming::{WORKTREES_DIRNAME, coord_dir_name};
use crate::workspace::ProjectWorkspace;

/// The minimum Git version the non-cone sparse surface requires (RR-01).
pub const MIN_GIT_VERSION: (u32, u32) = (2, 25);

/// Git could not be asked for its version at all.
pub const GIT_VERSION_UNDETECTABLE: &str = "GIT_VERSION_UNDETECTABLE";
/// Git is older than [`MIN_GIT_VERSION`].
pub const GIT_VERSION_TOO_OLD: &str = "GIT_VERSION_TOO_OLD";
/// Files under `.worktrees/` are tracked in the index.
pub const TRACKED_WORKTREES_CONTENT: &str = "TRACKED_WORKTREES_CONTENT";
/// `meta.json` declares a coordination branch but not the identity to derive
/// the worktree from.
pub const COORDINATION_META_INCOMPLETE: &str = "COORDINATION_META_INCOMPLETE";
/// The declared coordination branch does not exist locally or on a remote.
pub const COORDINATION_WORKTREE_NEVER_CREATED: &str = "COORDINATION_WORKTREE_NEVER_CREATED";
/// The coordination branch exists but its worktree is not materialized.
pub const COORDINATION_WORKTREE_MISSING: &str = "COORDINATION_WORKTREE_MISSING";
/// The coordination worktree is checked out on another branch (or detached).
pub const COORDINATION_WORKTREE_BRANCH_MISMATCH: &str = "COORDINATION_WORKTREE_BRANCH_MISMATCH";
/// The coordination worktree has uncommitted changes.
pub const COORDINATION_WORKTREE_DIRTY: &str = "COORDINATION_WORKTREE_DIRTY";
/// The coordination worktree trails its own branch tip (cleanly fast-forwardable).
pub const COORDINATION_WORKTREE_STALE: &str = "COORDINATION_WORKTREE_STALE";
/// The coordination branch trails the target branch (cleanly fast-forwardable).
pub const COORDINATION_BRANCH_STALE_VS_TARGET: &str = "COORDINATION_BRANCH_STALE_VS_TARGET";
/// The coordination branch and the target branch have diverged.
pub const COORDINATION_BRANCH_DIVERGED_VS_TARGET: &str = "COORDINATION_BRANCH_DIVERGED_VS_TARGET";
/// A lane worktree is missing the expected sparse-checkout policy.
pub const LANE_SPARSE_CHECKOUT_DRIFT: &str = crate::coordination_sparse::LANE_SPARSE_CHECKOUT_DRIFT;
/// A live coordination strand that `--fix` can revert.
pub const COORDINATION_STRANDED_COORD_REVERT: &str = "COORDINATION_STRANDED_COORD_REVERT";
/// A live coordination strand whose recorded worktree is gone.
pub const COORDINATION_STRANDED_COORD_REVERT_STUCK: &str =
    "COORDINATION_STRANDED_COORD_REVERT_STUCK";
/// A `pending_coord_reconcile` marker that is not a usable repair input.
pub const COORDINATION_RECONCILE_MARKER_UNPARSEABLE: &str =
    "COORDINATION_RECONCILE_MARKER_UNPARSEABLE";
/// A marker whose Mission cannot be resolved to a planning directory.
pub const COORDINATION_RECONCILE_MISSION_UNRESOLVABLE: &str =
    "COORDINATION_RECONCILE_MISSION_UNRESOLVABLE";

/// Recovery hint for a live strand (reference text).
pub const STRANDED_COORD_REVERT_HINT: &str = "Run `spec-kitty doctor coordination --fix` to revert the stranded coordination `done` commit(s) and clear the reconcile marker.";

/// Manual-recovery hint when the recorded coordination worktree is gone.
pub const STRANDED_COORD_REVERT_STUCK_HINT: &str = "The coordination worktree recorded for this strand no longer exists, so `--fix` cannot revert it. Recreate the coordination worktree (see the worktree hints from `spec-kitty doctor coordination`) then re-run `--fix`, or clear the stale `pending_coord_reconcile` marker after manually reconciling the coordination ref.";

/// Hint for an unparseable marker (reference text).
pub const MARKER_UNPARSEABLE_HINT: &str = "A `pending_coord_reconcile` marker could not be parsed into repair inputs (missing coord_ref/captured_sha/coord_worktree or an empty strand). Inspect `.kittify/runtime/merge/<mission_id>/state.json` and clear or repair the marker.";

/// Hint for a marker whose Mission handle cannot be resolved (reference text).
pub const MARKER_UNRESOLVABLE_MISSION_HINT: &str = "A `pending_coord_reconcile` marker names a mission that could not be resolved to a planning directory (unsafe or ambiguous handle). Run `spec-kitty doctor identity --json` and disambiguate before re-running `--fix`.";

/// Recovery command the reference names for workspace/worktree issues.
pub const WORKSPACE_RECOVERY_CMD: &str = "spec-kitty doctor workspaces --fix";

/// Hint for a coordination branch that was never created (or was deleted).
pub const COORD_BRANCH_ABSENT_HINT: &str = "Flatten the mission: remove the `coordination_branch` key from meta.json (the coordination topology was never activated). Then run `spec-kitty migrate backfill-topology` to re-derive and persist the topology.";

/// Hint for a diverged coordination branch.
///
/// Reserved for the owner of [`UnavailableCheck::ExpectedCoordinationDivergence`]:
/// this lane cannot classify a diverged pair, so it never emits the matching
/// finding and this text stays unreferenced by a verdict.
pub const COORD_DIVERGED_VS_TARGET_HINT: &str = "Inspect and reconcile manually; `spec-kitty doctor coordination --fix` will refuse to mutate a diverged coordination branch.";

/// How a finding is classified. Mirrors the reference's `severity` strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Nothing to report.
    Ok,
    /// Something to look at; the command still exits 0.
    Warning,
    /// Something is broken; the command exits 1.
    Error,
}

impl Severity {
    /// The stable machine-readable label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

impl std::fmt::Display for Severity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One doctor finding, in the reference's stable shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoordinationFinding {
    severity: Severity,
    message: String,
    next_step: Option<String>,
    code: Option<&'static str>,
    extra: Map<String, Value>,
}

impl CoordinationFinding {
    /// The severity class.
    pub fn severity(&self) -> Severity {
        self.severity
    }

    /// The human-readable finding text (reference spelling).
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The reference's remediation line, when it has one.
    pub fn next_step(&self) -> Option<&str> {
        self.next_step.as_deref()
    }

    /// The stable error code, when the finding carries one.
    pub fn code(&self) -> Option<&'static str> {
        self.code
    }

    /// The finding's stable `extra` payload (exact reference keys).
    pub fn extra(&self) -> &Map<String, Value> {
        &self.extra
    }

    /// Project the finding into the reference's JSON object.
    pub fn to_json(&self) -> Value {
        json!({
            "severity": self.severity.as_str(),
            "message": self.message,
            "next_step": self.next_step,
            "error_code": self.code,
            "extra": Value::Object(self.extra.clone()),
        })
    }

    fn healthy(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Ok,
            message: message.into(),
            next_step: None,
            code: None,
            extra: Map::new(),
        }
    }

    fn warning(
        message: impl Into<String>,
        next_step: impl Into<String>,
        code: &'static str,
    ) -> Self {
        Self {
            severity: Severity::Warning,
            message: message.into(),
            next_step: Some(next_step.into()),
            code: Some(code),
            extra: Map::new(),
        }
    }

    fn error(message: impl Into<String>, next_step: impl Into<String>, code: &'static str) -> Self {
        Self {
            severity: Severity::Error,
            message: message.into(),
            next_step: Some(next_step.into()),
            code: Some(code),
            extra: Map::new(),
        }
    }

    fn with_extra(mut self, key: &str, value: Value) -> Self {
        self.extra.insert(key.to_owned(), value);
        self
    }
}

/// A reference check this pass could not run.
///
/// Every check now has a canonical owner and the reference's own observation, so this
/// is *dynamic*, not a standing declaration: it is added to a report only when an
/// observation the check needs failed, and a report carrying one is incomplete (the
/// adapter refuses it rather than printing a partial report that could be read as a
/// passing health check).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnavailableCheck {
    /// A Source-compatible observation could not be read at all.
    ///
    /// The reference would have read it (or caught git's failure at its own call
    /// site); this adapter hit a failure the reference does not have — currently a
    /// `git worktree list --porcelain` listing that is not valid UTF-8, or a
    /// capture/timeout/spawn fault, plus the bounded blob read for an over-bound
    /// committed status log. Reporting the check as healthy in any of those cases
    /// would be an invented verdict.
    SourceObservationUnreadable,
}

impl UnavailableCheck {
    /// A stable identifier for the missing check.
    pub fn check(self) -> &'static str {
        match self {
            Self::SourceObservationUnreadable => "source-observation-unreadable",
        }
    }

    /// The capability that has to answer before the check can run.
    pub fn owner(self) -> &'static str {
        match self {
            Self::SourceObservationUnreadable => {
                "the Source-compatible Git read seam (`worktree_porcelain_observation`, `sparse_pattern_file_patterns_source`, `read_blob_at_revision_bounded`); its typed failure is not a Source-caught arm"
            }
        }
    }

    /// Why the check cannot be composed (one sentence).
    pub fn detail(self) -> &'static str {
        match self {
            Self::SourceObservationUnreadable => {
                "the observation the reference would have read failed outside the reference's own catch arms (for example a non-UTF-8 porcelain listing, a capture boundary, or an over-bound committed log); treating that as 'nothing to report' would report an unread repository as healthy"
            }
        }
    }
}

/// The requested `doctor coordination` surface.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CoordinationHealthRequest {
    /// Also report coordination-branch-vs-target staleness (`--check-staleness`).
    pub check_staleness: bool,
    /// Scope the per-Mission checks to one handle (`--mission`).
    pub mission: Option<String>,
}

impl CoordinationHealthRequest {
    /// A baseline read-only request (no staleness, every Mission).
    pub fn new() -> Self {
        Self::default()
    }

    /// Also report coordination-branch-vs-target staleness.
    pub fn with_staleness(mut self) -> Self {
        self.check_staleness = true;
        self
    }

    /// Scope the per-Mission checks to one handle.
    pub fn with_mission(mut self, handle: impl Into<String>) -> Self {
        self.mission = Some(handle.into());
        self
    }
}

/// The result of one read-only coordination-health pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoordinationHealthReport {
    findings: Vec<CoordinationFinding>,
    unavailable: Vec<UnavailableCheck>,
}

impl CoordinationHealthReport {
    /// The findings, in the reference's order.
    pub fn findings(&self) -> &[CoordinationFinding] {
        &self.findings
    }

    /// The checks this pass could not compose.
    pub fn unavailable(&self) -> &[UnavailableCheck] {
        &self.unavailable
    }

    /// Whether every requested check ran.
    pub fn is_complete(&self) -> bool {
        self.unavailable.is_empty()
    }

    /// Whether any finding is an `error` (the reference's exit-1 trigger).
    pub fn has_error(&self) -> bool {
        self.findings
            .iter()
            .any(|finding| finding.severity == Severity::Error)
    }

    /// The reference's exit code — **only** when every requested check ran.
    ///
    /// `None` means "this report is incomplete, so it must not be presented as
    /// a health verdict at all".
    pub fn exit_code(&self) -> Option<u8> {
        if self.is_complete() {
            Some(u8::from(self.has_error()))
        } else {
            None
        }
    }

    /// The finding payloads as the reference's JSON array.
    pub fn to_json(&self) -> Value {
        Value::Array(
            self.findings
                .iter()
                .map(CoordinationFinding::to_json)
                .collect(),
        )
    }
}

/// A coordination-health diagnosis that could not answer at all.
#[derive(Debug, thiserror::Error)]
pub enum CoordinationHealthError {
    /// The workspace or Git adapter refused the read.
    #[error(transparent)]
    Workspace(#[from] WorkspaceError),
    /// A `--mission` handle did not resolve to exactly one Mission.
    #[error(transparent)]
    Mission(#[from] MissionError),
    /// The reconcile-marker sweep could not finish (Source's escaped phase).
    #[error(transparent)]
    MarkerSweep(#[from] crate::consolidation::state::ConsolidationSweepError),
    /// A marker's committed-ref re-verification failed (Source raises here too).
    #[error("could not re-verify the reconcile marker in {path}: {source}")]
    CoordDoneRead {
        /// The persisted record the marker came from.
        path: PathBuf,
        /// The reader's own cause.
        #[source]
        source: crate::coherence::CoordDoneReadError,
    },
    /// A marker's text field cannot be spelled as Rust text.
    #[error(
        "a `pending_coord_reconcile` marker in {path} carries text that is not representable in Rust (code point U+{codepoint:04X})"
    )]
    UnrepresentableText {
        /// The persisted record the marker came from.
        path: PathBuf,
        /// The offending code point.
        codepoint: u32,
    },
    /// The Mission inventory needed to resolve a marker's planning directory failed.
    #[error("could not {what}: {source}")]
    Git {
        /// Which question failed.
        what: &'static str,
        /// Git's own diagnostic.
        #[source]
        source: GitError,
    },
    /// A filesystem read the diagnosis depends on failed.
    #[error("could not {context} at {path}: {source}")]
    Io {
        /// What was being done.
        context: &'static str,
        /// The path involved.
        path: PathBuf,
        /// The operating-system error.
        #[source]
        source: std::io::Error,
    },
}

/// Run the read-only coordination-health diagnosis.
///
/// Findings follow the reference's order: the repository-level checks first (git
/// version, tracked `.worktrees/` hygiene, stranded coordination strands), then one
/// Mission block per Mission under `kitty-specs/` in name order, each contributing
/// its coordination-worktree findings, its lane sparse-checkout findings and (with
/// `check_staleness`) its branch-vs-target finding.
///
/// The report is [`CoordinationHealthReport::is_complete`] unless a check's own
/// observation failed; see [`UnavailableCheck`] for the one such case.
pub fn coordination_health(
    workspace: &ProjectWorkspace,
    request: &CoordinationHealthRequest,
) -> Result<CoordinationHealthReport, CoordinationHealthError> {
    let repository = workspace.repository();
    let mut findings = Vec::new();
    let mut unavailable = Vec::new();

    findings.push(check_git_version(repository));
    findings.extend(check_tracked_worktrees(repository)?);
    findings.extend(check_stranded_coord_revert(workspace)?);

    for mission_dir in mission_dirs(workspace, request.mission.as_deref())? {
        let meta = read_meta(&mission_dir);
        let Some(meta) = meta.object() else {
            // The reference loads `meta.json` with `on_malformed="none"` and
            // skips anything it cannot read; a corrupt manifest is not this
            // check's finding to invent.
            continue;
        };
        let checks = check_mission(workspace, &mission_dir, meta, request.check_staleness)?;
        findings.extend(checks.findings);
        unavailable.extend(checks.unavailable);
    }

    Ok(CoordinationHealthReport {
        findings,
        unavailable,
    })
}

/// What one Mission's checks produced: findings plus any observation that failed.
struct MissionChecks {
    findings: Vec<CoordinationFinding>,
    unavailable: Vec<UnavailableCheck>,
}

/// RR-01: is the installed Git new enough for the non-cone sparse surface?
fn check_git_version(repository: &GitRepository) -> CoordinationFinding {
    match repository.git_version() {
        Ok((major, minor, _patch)) => {
            if (major, minor) < MIN_GIT_VERSION {
                let (required_major, required_minor) = MIN_GIT_VERSION;
                CoordinationFinding::error(
                    format!(
                        "git {major}.{minor} is older than the required {required_major}.{required_minor}. Sparse-checkout exclusion of status files requires the modern non-cone surface."
                    ),
                    "Upgrade git to >= 2.25 — see https://git-scm.com/downloads.",
                    GIT_VERSION_TOO_OLD,
                )
                .with_extra("detected", json!(format!("{major}.{minor}")))
            } else {
                CoordinationFinding::healthy(format!(
                    "git {major}.{minor} satisfies the >= {}.{} requirement.",
                    MIN_GIT_VERSION.0, MIN_GIT_VERSION.1
                ))
            }
        }
        Err(_) => CoordinationFinding::error(
            format!(
                "Could not detect git version. spec-kitty requires git >= {}.{}.",
                MIN_GIT_VERSION.0, MIN_GIT_VERSION.1
            ),
            "Install or upgrade git to >= 2.25.",
            GIT_VERSION_UNDETECTABLE,
        ),
    }
}

/// FR-035 (#1772 Bug 0): is any `.worktrees/` content tracked in the index?
fn check_tracked_worktrees(
    repository: &GitRepository,
) -> Result<Vec<CoordinationFinding>, CoordinationHealthError> {
    let entries = repository
        .index_entries(&[OsStr::new(WORKTREES_DIRNAME)])
        .map_err(|source| CoordinationHealthError::Git {
            what: "list the tracked paths under `.worktrees/`",
            source,
        })?;
    let tracked: Vec<&sk_git::GitPath> = entries
        .iter()
        .map(sk_git::IndexEntry::path)
        .filter(|path| is_under_worktrees(&path.to_path_buf()))
        .collect();

    if tracked.is_empty() {
        return Ok(vec![CoordinationFinding::healthy(format!(
            "No tracked content under {WORKTREES_DIRNAME}/."
        ))]);
    }

    let preview: Vec<String> = tracked
        .iter()
        .take(10)
        .map(|path| path.display_lossy())
        .collect();
    let truncated = tracked.len() > preview.len();
    let more = if truncated {
        format!(" (+{} more)", tracked.len() - preview.len())
    } else {
        String::new()
    };
    Ok(vec![
        CoordinationFinding::error(
            format!(
                "{} tracked file(s) under {WORKTREES_DIRNAME}/ — this is execution scratch space and must never be committed. Tracked content here drives the #1772 merge-staging failures.{more}",
                tracked.len()
            ),
            format!(
                "Remove it from version control: `git rm -r --cached {WORKTREES_DIRNAME}/` then commit, and ensure `{WORKTREES_DIRNAME}/` is gitignored."
            ),
            TRACKED_WORKTREES_CONTENT,
        )
        .with_extra("tracked", json!(preview))
        .with_extra("tracked_count", json!(tracked.len()))
        .with_extra("truncated", json!(truncated)),
    ])
}

/// The reference's path-shape predicate: does any component name `.worktrees`?
fn is_under_worktrees(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == OsStr::new(WORKTREES_DIRNAME))
}

/// The Mission directories to scan, exactly as the reference resolves them.
///
/// Without a handle: every direct child directory of `kitty-specs/`, in name
/// order. With a handle: the single directory [`sk_mission::select_mission`]
/// maps it to — the same selector every other command uses, so an unknown or
/// ambiguous handle is an error and never a silent fallback.
fn mission_dirs(
    workspace: &ProjectWorkspace,
    mission: Option<&str>,
) -> Result<Vec<PathBuf>, CoordinationHealthError> {
    if let Some(handle) = mission {
        let missions = workspace.missions()?;
        let selected = sk_mission::select_mission(&missions, handle)?;
        return Ok(vec![selected.spec_dir().to_path_buf()]);
    }

    let specs_dir = workspace.specs_dir();
    let metadata = match std::fs::metadata(&specs_dir) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(CoordinationHealthError::Io {
                context: "inspect the specs directory",
                path: specs_dir,
                source,
            });
        }
    };
    if !metadata.is_dir() {
        return Ok(Vec::new());
    }

    let mut dirs = Vec::new();
    let entries = std::fs::read_dir(&specs_dir).map_err(|source| CoordinationHealthError::Io {
        context: "list the specs directory",
        path: specs_dir.clone(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| CoordinationHealthError::Io {
            context: "list the specs directory",
            path: specs_dir.clone(),
            source,
        })?;
        let file_type = entry
            .file_type()
            .map_err(|source| CoordinationHealthError::Io {
                context: "inspect a specs entry",
                path: entry.path(),
                source,
            })?;
        if file_type.is_dir() {
            dirs.push(entry.path());
        }
    }
    dirs.sort();
    Ok(dirs)
}

/// The reference's `_coordination_identity` outcome for one manifest.
enum CoordIdentity {
    /// The Mission records no declared coordination branch: nothing to check.
    NotCoordinated,
    /// It declares one but not the identity the worktree name needs.
    Incomplete,
    /// The declared branch plus the identity used to compose the worktree path.
    Complete {
        coord_branch: String,
        slug: String,
        mission_id: String,
    },
}

/// Read the coordination identity from `meta.json` with Python truthiness.
fn coord_identity(meta: &Map<String, Value>) -> CoordIdentity {
    let Some(Value::String(coord_branch)) = meta.get("coordination_branch") else {
        return CoordIdentity::NotCoordinated;
    };
    if coord_branch.is_empty() {
        return CoordIdentity::NotCoordinated;
    }
    let coord_branch = coord_branch.clone();

    // `meta.get("mission_slug") or meta.get("slug")` — Python `or`, i.e.
    // truthiness, not presence.
    let slug_value = match meta.get("mission_slug") {
        Some(value) if python_truthy(value) => Some(value),
        _ => meta.get("slug"),
    };
    let slug = match slug_value {
        Some(Value::String(slug)) => slug.clone(),
        _ => return CoordIdentity::Incomplete,
    };
    // `mission_slug` must be a string; a truthy non-string is incomplete even
    // when a string `slug` fallback exists.
    if !matches!(meta.get("mission_slug"), None | Some(Value::String(_))) {
        return CoordIdentity::Incomplete;
    }
    let mission_id = match meta.get("mission_id") {
        Some(Value::String(mission_id)) => mission_id.clone(),
        _ => return CoordIdentity::Incomplete,
    };
    // The reference reaches the same warning one step later, through
    // `if not mission_slug or not mission_id:` — a blank string is `False`
    // there, so both halves must be non-empty before a worktree name is built.
    if slug.is_empty() || mission_id.is_empty() {
        return CoordIdentity::Incomplete;
    }

    CoordIdentity::Complete {
        coord_branch,
        slug,
        mission_id,
    }
}

/// The mid8 the coordination worktree name is composed with.
///
/// The reference's `_resolve_coord_short` is `resolve_mid8(...) or
/// mission_id[:8]`, and `resolve_mid8` returns the FIRST EIGHT CODE POINTS of
/// the declared `mission_id` whenever one is present (the slug's own tail is
/// only trusted when it provably matches). So the declared identity governs,
/// exactly as here.
fn coord_short(mission_id: &str) -> String {
    mission_id.chars().take(8).collect()
}

/// Every coordination finding for one Mission, plus any failed observation.
fn check_mission(
    workspace: &ProjectWorkspace,
    mission_dir: &Path,
    meta: &Map<String, Value>,
    check_staleness: bool,
) -> Result<MissionChecks, CoordinationHealthError> {
    let (coord_branch, slug, mission_id) = match coord_identity(meta) {
        CoordIdentity::NotCoordinated => {
            return Ok(MissionChecks {
                findings: Vec::new(),
                unavailable: Vec::new(),
            });
        }
        CoordIdentity::Incomplete => {
            return Ok(MissionChecks {
                findings: vec![CoordinationFinding::warning(
                    "meta.json carries coordination_branch but is missing mission_slug/mission_id; coord worktree health cannot be verified.",
                    "Run `spec-kitty doctor identity --json` for details.",
                    COORDINATION_META_INCOMPLETE,
                )],
                unavailable: Vec::new(),
            });
        }
        CoordIdentity::Complete {
            coord_branch,
            slug,
            mission_id,
        } => (coord_branch, slug, mission_id),
    };

    let repository = workspace.repository();
    let short = coord_short(&mission_id);
    let worktree = workspace
        .primary_root()
        .join(WORKTREES_DIRNAME)
        .join(coord_dir_name(&slug, &short));

    let mut findings = if worktree.exists() {
        check_materialized_worktree(workspace, &worktree, &coord_branch)?
    } else {
        check_unmaterialized_worktree(workspace, &worktree, &coord_branch, &slug, &short)?
    };

    // Lane sparse-checkout drift (Source `_check_lane_sparse_checkout_drift`).
    let mut unavailable = Vec::new();
    match check_lane_sparse_drift(workspace, &slug, &short) {
        Ok(lane_findings) => findings.extend(lane_findings),
        // A failed registration/pattern observation is "this check did not run",
        // never "the lanes are healthy".
        Err(_) => unavailable.push(UnavailableCheck::SourceObservationUnreadable),
    }

    if check_staleness {
        findings.extend(check_staleness_vs_target(repository, meta, &coord_branch)?);
    }

    // The reference attaches the manifest path to the never-created finding so
    // its `--fix` arm can find the mission. This lane never fixes, but the
    // payload is part of the finding's stable shape.
    for finding in &mut findings {
        if finding.code == Some(COORDINATION_WORKTREE_NEVER_CREATED) {
            finding.extra.insert(
                "meta_path".to_owned(),
                json!(mission_dir.join("meta.json").display().to_string()),
            );
        }
    }

    Ok(MissionChecks {
        findings,
        unavailable,
    })
}

/// The reference's `_check_lane_sparse_checkout_drift`.
///
/// Composes [`crate::coordination_sparse`]: the one policy owner, the recomposition
/// recognizer, the per-worktree pattern-file read and the typed worktree roster.
/// This function only renders its typed facts into the reference's findings.
fn check_lane_sparse_drift(
    workspace: &ProjectWorkspace,
    mission_slug: &str,
    mid8: &str,
) -> Result<Vec<CoordinationFinding>, crate::coordination_sparse::LaneSparseError> {
    let scan = crate::coordination_sparse::scan_lane_sparse_drift(
        workspace.repository(),
        mission_slug,
        mid8,
    )?;
    let repair_hint = format!("Run `{WORKSPACE_RECOVERY_CMD}` to restore.");
    let mut findings = Vec::new();
    for drift in scan.drifts() {
        let lane = drift.lane().display();
        let finding = match drift.missing_patterns() {
            None if matches!(
                drift,
                crate::coordination_sparse::LaneSparseDrift::PolicyFileMissing { .. }
            ) =>
            {
                CoordinationFinding::warning(
                    format!(
                        "Lane worktree {lane} is missing the sparse-checkout policy that excludes status files."
                    ),
                    repair_hint.clone(),
                    LANE_SPARSE_CHECKOUT_DRIFT,
                )
            }
            None => CoordinationFinding::warning(
                format!("Could not resolve sparse-checkout path for {lane}."),
                format!("Run `{WORKSPACE_RECOVERY_CMD}`."),
                LANE_SPARSE_CHECKOUT_DRIFT,
            ),
            Some(missing) => CoordinationFinding::warning(
                format!(
                    "Lane worktree {lane} sparse-checkout is missing {} expected pattern(s): {}.",
                    missing.len(),
                    python_repr_list(missing)
                ),
                repair_hint.clone(),
                LANE_SPARSE_CHECKOUT_DRIFT,
            )
            .with_extra("missing_patterns", json!(missing)),
        };
        findings.push(finding);
    }
    if findings.is_empty() {
        findings.push(CoordinationFinding::healthy(
            "All lane worktrees carry the expected sparse-checkout policy.",
        ));
    }
    Ok(findings)
}

/// Python `str(list_of_str)` — `['a', 'b']`, with the one canonical repr per item.
fn python_repr_list(items: &[String]) -> String {
    let rendered: Vec<String> = items.iter().map(|item| python_repr(item)).collect();
    format!("[{}]", rendered.join(", "))
}

/// Python `repr()` of an optional string — `repr(text)`, or `None`.
fn python_repr_opt(text: Option<&str>) -> String {
    text.map(python_repr).unwrap_or_else(|| "None".to_owned())
}

/// The reference's `_check_stranded_coord_revert`.
///
/// Enumerates the persisted `pending_coord_reconcile` markers through the canonical
/// consolidation state owner (never a re-implemented runtime-path scan) and
/// re-verifies each one against the **committed** coordination ref through the
/// canonical committed-DONE reducer. Marker presence alone is never a finding.
fn check_stranded_coord_revert(
    workspace: &ProjectWorkspace,
) -> Result<Vec<CoordinationFinding>, CoordinationHealthError> {
    let states = crate::consolidation::state::iter_pending_coord_reconcile_markers_source(
        workspace.primary_root(),
    )?;
    let mut findings = Vec::new();
    for state in &states {
        if let Some(finding) = finding_for_reconcile_marker(workspace, state)? {
            findings.push(finding);
        }
    }
    Ok(findings)
}

/// The reference's `_finding_for_reconcile_marker`: exactly one outcome per marker.
///
/// `None` is reserved for a genuinely stale marker (the committed ref re-derives
/// coherent). Every other terminal path yields a finding, and an input the caller
/// cannot make sense of is a warning rather than a silent drop.
fn finding_for_reconcile_marker(
    workspace: &ProjectWorkspace,
    state: &crate::consolidation::state::ConsolidationState,
) -> Result<Option<CoordinationFinding>, CoordinationHealthError> {
    let record_path = marker_record_path(workspace, state);
    let mission_slug_text = marker_text(state, "mission_slug");
    let mission_id_text = marker_text(state, "mission_id");
    let mission_id_value = json!(mission_id_text.clone());
    let mission_slug_value = json!(mission_slug_text.clone());

    let Some(marker) = state.pending_coord_reconcile() else {
        // The enumerator only yields records with a non-null marker.
        return Ok(None);
    };
    let Some(parsed) = state.parse_pending_coord_reconcile() else {
        return Ok(Some(
            CoordinationFinding::warning(
                format!(
                    "Mission {} carries a `pending_coord_reconcile` marker that could not be parsed into repair inputs.",
                    python_repr_opt(mission_slug_text.as_deref())
                ),
                MARKER_UNPARSEABLE_HINT,
                COORDINATION_RECONCILE_MARKER_UNPARSEABLE,
            )
            .with_extra("mission_id", mission_id_value)
            .with_extra("mission_slug", mission_slug_value),
        ));
    };
    let _ = marker;

    let Some(mission_slug) = mission_slug_text.as_deref() else {
        return Ok(Some(
            CoordinationFinding::warning(
                "A `pending_coord_reconcile` marker is present but its mission carries no `mission_slug` to resolve a planning directory.",
                MARKER_UNRESOLVABLE_MISSION_HINT,
                COORDINATION_RECONCILE_MISSION_UNRESOLVABLE,
            )
            .with_extra("mission_id", mission_id_value)
            .with_extra("mission_slug", mission_slug_value),
        ));
    };

    let Some(feature_dir) = planning_read_dir(workspace, mission_slug)? else {
        return Ok(Some(
            CoordinationFinding::warning(
                format!(
                    "Mission {} on a `pending_coord_reconcile` marker could not be resolved to a planning directory.",
                    python_repr(mission_slug)
                ),
                MARKER_UNRESOLVABLE_MISSION_HINT,
                COORDINATION_RECONCILE_MISSION_UNRESOLVABLE,
            )
            .with_extra("mission_id", mission_id_value)
            .with_extra("mission_slug", mission_slug_value),
        ));
    };

    let candidate_wps = parsed.stranded_wp_ids_as_strings().map_err(|codepoint| {
        CoordinationHealthError::UnrepresentableText {
            path: record_path.clone(),
            codepoint,
        }
    })?;
    let remaining = crate::coherence::coord_incoherent_done_wps(
        parsed.coord_ref(),
        &candidate_wps,
        workspace.primary_root(),
        &feature_dir,
    )
    .map_err(|source| CoordinationHealthError::CoordDoneRead {
        path: record_path.clone(),
        source,
    })?;
    if remaining.is_empty() {
        // Stale marker: the committed ref re-derives coherent. No finding.
        return Ok(None);
    }

    let strand_shas = match state.marker_strand_shas() {
        None => Value::Null,
        Some(shas) => Value::Array(
            shas.iter()
                .map(|sha| {
                    sha.to_scalar_string()
                        .map(Value::String)
                        .map_err(|codepoint| CoordinationHealthError::UnrepresentableText {
                            path: record_path.clone(),
                            codepoint,
                        })
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
    };
    let mut finding = if std::path::Path::new(parsed.coord_worktree()).exists() {
        CoordinationFinding::error(
            format!(
                "Coordination ref {} for mission {} still reduces WP(s) {} to `done` after a merge rollback (expected `approved`).",
                python_repr(parsed.coord_ref()),
                python_repr(mission_slug),
                python_repr_list(&remaining)
            ),
            STRANDED_COORD_REVERT_HINT,
            COORDINATION_STRANDED_COORD_REVERT,
        )
    } else {
        // A live strand whose recorded worktree is gone is STILL a committed-ref
        // split-brain: it stays an `error` (a `warning` would exit 0 and hide it)
        // and carries a manual-recovery remedy instead of looping `--fix`.
        CoordinationFinding::error(
            format!(
                "Coordination ref {} for mission {} still strands WP(s) {} at `done`, but its coordination worktree {} no longer exists — `--fix` cannot revert it.",
                python_repr(parsed.coord_ref()),
                python_repr(mission_slug),
                python_repr_list(&remaining),
                python_repr(parsed.coord_worktree())
            ),
            STRANDED_COORD_REVERT_STUCK_HINT,
            COORDINATION_STRANDED_COORD_REVERT_STUCK,
        )
    };
    for (key, value) in [
        ("mission_id", mission_id_value),
        ("mission_slug", mission_slug_value),
        ("coord_ref", json!(parsed.coord_ref())),
        ("captured_sha", json!(parsed.captured_sha())),
        ("coord_worktree", json!(parsed.coord_worktree())),
        ("candidate_wps", json!(candidate_wps)),
        ("stranded_wp_ids", json!(remaining)),
        ("strand_shas", strand_shas),
    ] {
        finding = finding.with_extra(key, value);
    }
    Ok(Some(finding))
}

/// The persisted record a marker came from, for diagnostics only.
fn marker_record_path(
    workspace: &ProjectWorkspace,
    state: &crate::consolidation::state::ConsolidationState,
) -> PathBuf {
    match marker_text(state, "mission_id") {
        Some(mission_id) => {
            crate::consolidation::state::state_path(workspace.primary_root(), &mission_id)
        }
        None => crate::consolidation::state::merge_runtime_dir(workspace.primary_root()),
    }
}

/// One record field as Rust text, when it is a JSON string that can be spelled.
fn marker_text(
    state: &crate::consolidation::state::ConsolidationState,
    field: &str,
) -> Option<String> {
    let value = state.field(field)?;
    match value {
        sk_kernel::python_json::PythonJsonValue::String(text) => text.to_scalar_string().ok(),
        _ => None,
    }
}

/// Resolve a marker's Mission handle to its planning directory.
///
/// The reference folds a bare human slug to the on-disk `<slug>-<mid8>` directory
/// through its canonical read-path resolver, and *raises* for an unsafe or ambiguous
/// handle (the caller turns that into a warning). `None` here is exactly those two
/// refusal cases.
///
/// The canonical selector matches the directory name, mid8 and mission id; a handle
/// that names no discovered Mission falls back to the literal `kitty-specs/<handle>`
/// directory — the reference's own lenient answer, whose log simply does not exist.
fn planning_read_dir(
    workspace: &ProjectWorkspace,
    handle: &str,
) -> Result<Option<PathBuf>, CoordinationHealthError> {
    if handle.is_empty()
        || handle.trim() != handle
        || handle.contains('/')
        || handle.contains('\\')
        || matches!(handle, "." | "..")
    {
        return Ok(None);
    }
    let missions = workspace.missions()?;
    let mut matches: Vec<&sk_mission::Mission> = missions
        .iter()
        .filter(|mission| mission.slug().as_str() == handle)
        .collect();
    if matches.is_empty() {
        // A bare human slug (`meta.json`'s `mission_slug`, else `slug`).
        matches = missions
            .iter()
            .filter(|mission| {
                ["mission_slug", "slug"].iter().any(|field| {
                    mission
                        .meta_field(field)
                        .and_then(Value::as_str)
                        .is_some_and(|value| !value.is_empty() && value == handle)
                })
            })
            .collect();
    }
    match matches.len() {
        0 => Ok(Some(workspace.specs_dir().join(handle))),
        1 => Ok(Some(matches[0].spec_dir().to_path_buf())),
        _ => Ok(None),
    }
}

/// Findings for a coordination worktree that exists on disk.
fn check_materialized_worktree(
    workspace: &ProjectWorkspace,
    worktree: &Path,
    coord_branch: &str,
) -> Result<Vec<CoordinationFinding>, CoordinationHealthError> {
    let mut findings = Vec::new();
    let worktree_repo =
        GitRepository::open(worktree).map_err(|source| CoordinationHealthError::Git {
            what: "open the coordination worktree as a Git repository",
            source,
        })?;

    if let Some(finding) = worktree_head_finding(&worktree_repo, worktree, coord_branch)? {
        findings.push(finding);
    }
    if let Some(finding) = worktree_dirty_finding(&worktree_repo, worktree)? {
        findings.push(finding);
    }
    if let Some(finding) =
        worktree_stale_finding(workspace, &worktree_repo, worktree, coord_branch)?
    {
        findings.push(finding);
    }

    if findings.is_empty() {
        findings.push(CoordinationFinding::healthy(format!(
            "Coordination worktree {} is healthy.",
            worktree.display()
        )));
    }
    Ok(findings)
}

/// Findings for a declared coordination branch whose worktree is absent.
fn check_unmaterialized_worktree(
    workspace: &ProjectWorkspace,
    worktree: &Path,
    coord_branch: &str,
    slug: &str,
    short: &str,
) -> Result<Vec<CoordinationFinding>, CoordinationHealthError> {
    let branch = parse_branch(coord_branch);
    let local_tip = match &branch {
        Some(branch) => workspace
            .repository()
            .read_branch_tip(branch)
            .map_err(|source| CoordinationHealthError::Git {
                what: "read the coordination branch tip",
                source,
            })?,
        // A declared branch that is not a valid ref name cannot exist; the
        // reference's `rev-parse` fails for it too, so it takes the
        // never-created path rather than aborting the sweep.
        None => None,
    };

    if !coord_branch_exists(workspace, branch.as_ref(), coord_branch)? {
        return Ok(vec![CoordinationFinding::warning(
            format!(
                "Coordination worktree {} is missing for mission {} and the declared coordination branch {} does not exist in git (never created or deleted).",
                worktree.display(),
                python_repr(slug),
                python_repr(coord_branch)
            ),
            COORD_BRANCH_ABSENT_HINT,
            COORDINATION_WORKTREE_NEVER_CREATED,
        )]);
    }

    // #5113 / FR-014: a branch that exists ONLY as a remote-tracking ref is
    // never auto-materialized by `--fix`, so leading with the bare `--fix`
    // command would loop the operator back to a refusal.
    let next_step = if local_tip.is_some() {
        format!(
            "Run: `spec-kitty doctor coordination --mission {slug} --fix` (or, manually: `git -C {} worktree add {} {coord_branch}`)",
            workspace.primary_root().display(),
            worktree.display()
        )
    } else {
        format!(
            "Run in order: `git -C {} fetch`, then `git -C {} branch {coord_branch} origin/{coord_branch}`, then `spec-kitty doctor coordination --mission {slug} --fix`.",
            workspace.primary_root().display(),
            workspace.primary_root().display()
        )
    };

    Ok(vec![
        CoordinationFinding::warning(
            format!(
                "Coordination worktree {} is missing for mission {} (the branch {} exists).",
                worktree.display(),
                python_repr(slug),
                python_repr(coord_branch)
            ),
            next_step,
            COORDINATION_WORKTREE_MISSING,
        )
        .with_extra(
            "recovery_args",
            recovery_args(workspace, worktree, coord_branch),
        )
        .with_extra("mission_slug", json!(slug))
        .with_extra("mid8", json!(short))
        .with_extra("coord_branch", json!(coord_branch)),
    ])
}

/// The reference's `_coord_branch_exists`: local head, then the whole
/// `refs/remotes/` name roster, then the shared remote authority.
fn coord_branch_exists(
    workspace: &ProjectWorkspace,
    branch: Option<&BranchName>,
    coord_branch: &str,
) -> Result<bool, CoordinationHealthError> {
    let Some(branch) = branch else {
        return Ok(false);
    };
    let repository = workspace.repository();
    if repository
        .read_branch_tip(branch)
        .map_err(|source| CoordinationHealthError::Git {
            what: "read the coordination branch tip",
            source,
        })?
        .is_some()
    {
        return Ok(true);
    }
    // The reference scans every `refs/remotes/` line and compares the branch part
    // (after the FIRST `/`, i.e. the remote name) with the coordination branch. A
    // FAILED scan is not a verdict: it falls through to the remote lookup below,
    // exactly as the reference does (only its own `except OSError` short-circuits,
    // and that one answers "present").
    if remote_tracking_ref_exists(repository, coord_branch).unwrap_or(false) {
        return Ok(true);
    }
    // The last arm consults the remote itself: a branch the checkout never
    // fetched (single-branch/shallow/CI clone) or a pruned tracking ref is
    // still present, not "never created". HIT and ERROR both count as present.
    match repository.remote_branch_lookup(branch) {
        Ok(RemoteBranchLookup::Hit { .. } | RemoteBranchLookup::Error { .. }) => Ok(true),
        Ok(RemoteBranchLookup::CleanMiss | RemoteBranchLookup::NoRemote) => Ok(false),
        // An unusable answer is not proof of absence; keep the reference's
        // fail-closed direction rather than reporting data loss.
        Err(_) => Ok(true),
    }
}

/// Does any remote-tracking ref name the branch?
///
/// The reference scans every `refs/remotes/<remote>/<branch>` line before it
/// contacts a remote. This is that same local answer through the canonical
/// roster (all remote-tracking refs, one `for-each-ref`, including a symbolic
/// remote `HEAD`), so an orphan tracking ref of an unconfigured remote is present
/// here exactly as it is there.
fn remote_tracking_ref_exists(
    repository: &GitRepository,
    coord_branch: &str,
) -> Result<bool, CoordinationHealthError> {
    let roster =
        repository
            .remote_tracking_roster()
            .map_err(|source| CoordinationHealthError::Git {
                what: "list the remote-tracking refs",
                source,
            })?;
    Ok(roster.contains_branch(coord_branch))
}

/// The reference's `recovery_args` payload (the raw manual `worktree add`).
fn recovery_args(workspace: &ProjectWorkspace, worktree: &Path, coord_branch: &str) -> Value {
    json!([
        "git",
        "-C",
        workspace.primary_root().display().to_string(),
        "worktree",
        "add",
        worktree.display().to_string(),
        coord_branch,
    ])
}

/// The reference's `_coord_worktree_head_finding`.
fn worktree_head_finding(
    worktree_repo: &GitRepository,
    worktree: &Path,
    coord_branch: &str,
) -> Result<Option<CoordinationFinding>, CoordinationHealthError> {
    let actual = actual_head(worktree_repo).map_err(|source| CoordinationHealthError::Git {
        what: "read the coordination worktree HEAD",
        source,
    })?;
    let expected = format!("refs/heads/{coord_branch}");
    if actual == expected {
        return Ok(None);
    }
    Ok(Some(CoordinationFinding::warning(
        format!(
            "Coordination worktree {} is on {}, expected {}.",
            worktree.display(),
            python_repr(&actual),
            python_repr(coord_branch)
        ),
        format!(
            "Inspect the worktree, then check out the coordination branch there: `git -C {} checkout {coord_branch}`.",
            worktree.display()
        ),
        COORDINATION_WORKTREE_BRANCH_MISMATCH,
    )))
}

/// The worktree's checked-out ref, or the reference's `<detached>` marker.
///
/// The reference reads `git -C <worktree> symbolic-ref HEAD` and treats any
/// failure (detached HEAD, a corrupt `HEAD`, a directory that is not a
/// worktree at all) as `<detached>`; this maps the canonical full-HEAD read
/// onto exactly that taxonomy.
fn actual_head(worktree_repo: &GitRepository) -> Result<String, GitError> {
    match worktree_repo.symbolic_head_ref() {
        Ok(Some(reference)) => Ok(reference.as_str().to_owned()),
        Ok(None) => Ok("<detached>".to_owned()),
        Err(GitError::UnreadableHead { .. }) => Ok("<detached>".to_owned()),
        Err(error) => Err(error),
    }
}

/// The reference's `_coord_worktree_dirty_finding`.
fn worktree_dirty_finding(
    worktree_repo: &GitRepository,
    worktree: &Path,
) -> Result<Option<CoordinationFinding>, CoordinationHealthError> {
    // Source: `status_entries(worktree, untracked=None)` — untracked-file reporting
    // is decided by the repository's own `status.showUntrackedFiles`.
    let entries =
        worktree_repo
            .status_source_observed()
            .map_err(|source| CoordinationHealthError::Git {
                what: "read the coordination worktree status",
                source,
            })?;
    if entries.is_empty() {
        return Ok(None);
    }
    Ok(Some(CoordinationFinding::warning(
        format!(
            "Coordination worktree {} has uncommitted changes.",
            worktree.display()
        ),
        "Commit or discard the changes inside the coord worktree before next implement/review.",
        COORDINATION_WORKTREE_DIRTY,
    )))
}

/// The reference's `_coord_worktree_stale_finding`.
fn worktree_stale_finding(
    workspace: &ProjectWorkspace,
    worktree_repo: &GitRepository,
    worktree: &Path,
    coord_branch: &str,
) -> Result<Option<CoordinationFinding>, CoordinationHealthError> {
    let Some(worktree_head) = read_head_commit(worktree_repo)? else {
        return Ok(None);
    };
    let branch_tip = read_branch_tip_text(workspace.repository(), coord_branch)?;
    let Some(branch_tip) = branch_tip else {
        return Ok(None);
    };
    if !strictly_behind(workspace.repository(), &worktree_head, &branch_tip)? {
        return Ok(None);
    }
    Ok(Some(CoordinationFinding::warning(
        format!(
            "Coordination worktree {} is behind the coord branch {} tip (fast-forward available).",
            worktree.display(),
            python_repr(coord_branch)
        ),
        format!(
            "Run `{WORKSPACE_RECOVERY_CMD}` to refresh it (fast-forwards stale coord worktrees)."
        ),
        COORDINATION_WORKTREE_STALE,
    )))
}

/// The reference's `_coord_branch_stale_vs_target_finding`.
///
/// Three decidable arms: a coordination branch that strictly trails its target is a
/// fast-forward candidate; one that strictly leads it is by design; a pair that is
/// neither is classified through the canonical expected-divergence owner before any
/// `DIVERGED` verdict is emitted.
fn check_staleness_vs_target(
    repository: &GitRepository,
    meta: &Map<String, Value>,
    coord_branch: &str,
) -> Result<Vec<CoordinationFinding>, CoordinationHealthError> {
    // Source takes the raw value: no trimming, only "not a string" / "empty".
    let target_branch = match meta.get("target_branch") {
        Some(Value::String(target)) if !target.is_empty() => target.clone(),
        _ => return Ok(Vec::new()),
    };
    let coord_sha = read_branch_tip_text(repository, coord_branch)?;
    let target_sha = read_branch_tip_text(repository, &target_branch)?;
    let (Some(coord_sha), Some(target_sha)) = (coord_sha, target_sha) else {
        return Ok(Vec::new());
    };
    if coord_sha == target_sha {
        return Ok(Vec::new());
    }
    if strictly_behind(repository, &target_sha, &coord_sha)? {
        // Coordination branch strictly ahead: it carries only its own
        // coordination commits (e.g. the status log a create seeds).
        return Ok(Vec::new());
    }
    if strictly_behind(repository, &coord_sha, &target_sha)? {
        return Ok(vec![CoordinationFinding::warning(
            format!(
                "Coordination branch {} is behind target branch {} (fast-forward available).",
                python_repr(coord_branch),
                python_repr(&target_branch)
            ),
            "Run `spec-kitty doctor coordination --fix` to fast-forward it.",
            COORDINATION_BRANCH_STALE_VS_TARGET,
        )]);
    }
    // Genuinely diverged: the canonical exemption decides whether the extra
    // coordination history is this Mission's own bookkeeping (expected) or not.
    let mission_dir_name = match meta.get("mission_slug") {
        Some(Value::String(slug)) if python_truthy(&Value::String(slug.clone())) => {
            Some(slug.clone())
        }
        _ => match meta.get("slug") {
            Some(Value::String(slug)) if python_truthy(&Value::String(slug.clone())) => {
                Some(slug.clone())
            }
            _ => None,
        },
    };
    if let Some(mission_dir_name) = mission_dir_name
        && let (Ok(coordination), Ok(target)) = (
            BranchName::parse(coord_branch),
            BranchName::parse(&target_branch),
        )
    {
        let request = crate::coordination_divergence::ExpectedCoordinationDivergenceRequest::new(
            &coordination,
            &target,
            &mission_dir_name,
        );
        if crate::coordination_divergence::is_expected_coordination_divergence(repository, &request)
        {
            return Ok(Vec::new());
        }
    }
    Ok(vec![CoordinationFinding::warning(
        format!(
            "Coordination branch {} has diverged from target branch {} and cannot be fast-forwarded automatically.",
            python_repr(coord_branch),
            python_repr(&target_branch)
        ),
        COORD_DIVERGED_VS_TARGET_HINT,
        COORDINATION_BRANCH_DIVERGED_VS_TARGET,
    )])
}

/// The `HEAD` commit of a checkout, or `None` when Git has no answer.
///
/// The reference's `_rev_parse` maps every failure to `""` (no finding), so a
/// read that `sk-git` classifies as a failure is treated as "unreadable", not
/// as a health failure of the mission.
fn read_head_commit(repository: &GitRepository) -> Result<Option<String>, CoordinationHealthError> {
    match repository.head_commit() {
        Ok(commit) => Ok(commit),
        Err(_) => Ok(None),
    }
}

/// A branch tip as text, or `None` when the branch is absent or unreadable.
fn read_branch_tip_text(
    repository: &GitRepository,
    branch: &str,
) -> Result<Option<String>, CoordinationHealthError> {
    let Some(branch) = parse_branch(branch) else {
        return Ok(None);
    };
    match repository.read_branch_tip(&branch) {
        Ok(tip) => Ok(tip.map(|oid| oid.as_str().to_owned())),
        Err(source) => Err(CoordinationHealthError::Git {
            what: "read a branch tip",
            source,
        }),
    }
}

/// Is `ancestor` a STRICT ancestor of `descendant`?
///
/// Equal ids, blank ids and diverged pairs are all `false`, exactly like the
/// reference's `_is_ff_candidate` call sites.
fn strictly_behind(
    repository: &GitRepository,
    ancestor: &str,
    descendant: &str,
) -> Result<bool, CoordinationHealthError> {
    if ancestor.is_empty() || descendant.is_empty() || ancestor == descendant {
        return Ok(false);
    }
    let (Ok(ancestor), Ok(descendant)) =
        (sk_git::Oid::parse(ancestor), sk_git::Oid::parse(descendant))
    else {
        return Ok(false);
    };
    repository
        .is_ancestor(&ancestor, &descendant)
        .map_err(|source| CoordinationHealthError::Git {
            what: "compare two commits",
            source,
        })
}

/// A declared branch name, when it is a valid ref name at all.
fn parse_branch(branch: &str) -> Option<BranchName> {
    BranchName::parse(branch).ok()
}

/// The reference quotes these values with Python `repr` (`!r`), which uses
/// single quotes and escapes non-printable code points.
///
/// That quoting rule is the Kernel's one canonical CPython `repr` authority,
/// never a hand-rolled `{:?}`: `PythonString::from_scalars(text)` is the same
/// formatter the Op writer path uses, so a branch name containing a quote or a
/// control character renders exactly like the reference.
fn python_repr(text: &str) -> String {
    sk_kernel::PythonString::from_scalars(text).repr()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(value: Value) -> Map<String, Value> {
        value
            .as_object()
            .cloned()
            .expect("object fixture for the meta reader")
    }

    #[test]
    fn identity_skips_an_undeclared_coordination_branch() {
        for value in [
            json!({"mission_id": "01KNXQS9ATWWFXS3K5ZJ9E5001"}),
            json!({"coordination_branch": ""}),
            json!({"coordination_branch": 7}),
            json!({"coordination_branch": null}),
        ] {
            assert!(
                matches!(
                    coord_identity(&meta(value.clone())),
                    CoordIdentity::NotCoordinated
                ),
                "should skip: {value}"
            );
        }
    }

    #[test]
    fn identity_reports_incomplete_metadata() {
        for value in [
            json!({"coordination_branch": "b"}),
            json!({"coordination_branch": "b", "mission_slug": "s"}),
            json!({"coordination_branch": "b", "slug": "s"}),
            json!({"coordination_branch": "b", "mission_slug": "s", "mission_id": 5}),
            json!({"coordination_branch": "b", "mission_slug": 5, "slug": "s", "mission_id": "01KNXQS9ATWWFXS3K5ZJ9E5001"}),
        ] {
            assert!(
                matches!(
                    coord_identity(&meta(value.clone())),
                    CoordIdentity::Incomplete
                ),
                "should be incomplete: {value}"
            );
        }
    }

    #[test]
    fn identity_prefers_mission_slug_over_slug_by_truthiness() {
        let complete = coord_identity(&meta(json!({
            "coordination_branch": "kitty/mission-x",
            "mission_slug": "primary-slug",
            "slug": "fallback-slug",
            "mission_id": "01KNXQS9ATWWFXS3K5ZJ9E5001",
        })));
        match complete {
            CoordIdentity::Complete {
                coord_branch, slug, ..
            } => {
                assert_eq!(coord_branch, "kitty/mission-x");
                assert_eq!(slug, "primary-slug");
            }
            _ => panic!("expected a complete identity"),
        }

        // An empty/absent `mission_slug` falls through to `slug`, and an empty
        // `slug` makes the identity incomplete rather than blank.
        assert!(matches!(
            coord_identity(&meta(json!({
                "coordination_branch": "b",
                "mission_slug": "",
                "slug": "fallback-slug",
                "mission_id": "01KNXQS9ATWWFXS3K5ZJ9E5001",
            }))),
            CoordIdentity::Complete { ref slug, .. } if slug == "fallback-slug"
        ));
        assert!(matches!(
            coord_identity(&meta(json!({
                "coordination_branch": "b",
                "mission_slug": "",
                "slug": "",
                "mission_id": "01KNXQS9ATWWFXS3K5ZJ9E5001",
            }))),
            CoordIdentity::Incomplete
        ));
    }

    #[test]
    fn the_short_id_is_the_first_eight_code_points_of_the_declared_id() {
        assert_eq!(coord_short("01KNXQS9ATWWFXS3K5ZJ9E5001"), "01KNXQS9");
        assert_eq!(coord_short("short"), "short");
        assert_eq!(coord_short(""), "");
        // Code points, not bytes: an astral character is one code point.
        assert_eq!(coord_short("\u{1F600}ABCDEFGHI"), "\u{1F600}ABCDEFG");
    }

    #[test]
    fn the_worktrees_predicate_matches_a_whole_path_segment_only() {
        assert!(is_under_worktrees(Path::new(".worktrees/x/file.md")));
        assert!(is_under_worktrees(Path::new("a/.worktrees/b")));
        assert!(!is_under_worktrees(Path::new(".worktreesx/file.md")));
        assert!(!is_under_worktrees(Path::new("x.worktrees/file.md")));
        assert!(!is_under_worktrees(Path::new("plain/file.md")));
    }

    #[test]
    fn findings_project_to_the_reference_payload() {
        let finding = CoordinationFinding::warning("m", "n", COORDINATION_WORKTREE_DIRTY)
            .with_extra("coord_branch", json!("b"));
        let payload = finding.to_json();
        assert_eq!(payload["severity"], json!("warning"));
        assert_eq!(payload["message"], json!("m"));
        assert_eq!(payload["next_step"], json!("n"));
        assert_eq!(payload["error_code"], json!("COORDINATION_WORKTREE_DIRTY"));
        assert_eq!(payload["extra"]["coord_branch"], json!("b"));

        let healthy = CoordinationFinding::healthy("fine");
        let payload = healthy.to_json();
        assert_eq!(payload["severity"], json!("ok"));
        assert_eq!(payload["next_step"], json!(null));
        assert_eq!(payload["error_code"], json!(null));
        assert_eq!(payload["extra"], json!({}));
    }

    #[test]
    fn an_incomplete_report_has_no_exit_code() {
        let complete = CoordinationHealthReport {
            findings: vec![CoordinationFinding::healthy("fine")],
            unavailable: Vec::new(),
        };
        assert_eq!(complete.exit_code(), Some(0));

        let errored = CoordinationHealthReport {
            findings: vec![CoordinationFinding::error(
                "bad",
                "fix",
                TRACKED_WORKTREES_CONTENT,
            )],
            unavailable: Vec::new(),
        };
        assert_eq!(errored.exit_code(), Some(1));

        let incomplete = CoordinationHealthReport {
            findings: vec![CoordinationFinding::healthy("fine")],
            unavailable: vec![UnavailableCheck::SourceObservationUnreadable],
        };
        assert_eq!(incomplete.exit_code(), None);
        assert!(!incomplete.is_complete());
    }

    #[test]
    fn every_unavailable_check_names_an_owner() {
        let check = UnavailableCheck::SourceObservationUnreadable;
        assert!(!check.check().is_empty());
        assert!(!check.owner().is_empty(), "{check:?} must name its owner");
        assert!(!check.detail().is_empty());
    }
}
