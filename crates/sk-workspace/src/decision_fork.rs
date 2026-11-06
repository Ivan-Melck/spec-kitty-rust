//! Read-only decision-stream fork detector and coordination-only-ledger finder.
//!
//! # Source
//!
//! `specify_cli.decisions.fork` (pinned `9aed3a80`), mission
//! ``coord-artifact-single-home-01M3V4BE`` WP17. It is the ONE place that knows
//! how to compare the PRIMARY and COORDINATION copies of a Mission's decision
//! streams (`status.events.jsonl` decision rows and `decisions.events.jsonl`)
//! and its ledger (`decisions/index.json` + `DM-*.md`). `doctor decisions`
//! consumes it (`DECISION_LOG_FORKED`, `DECISION_LEDGER_ONLY_ON_COORDINATION`).
//!
//! # Read-only by construction (Source FR-016 US4.1)
//!
//! Nothing here writes, locks, mkdirs, materializes a worktree, or merges and
//! re-sequences a forked log (Source C-003). The `reconcile_steps` are
//! human-readable guidance, never executed code.
//!
//! # Surface addressing
//!
//! Both surfaces come from the canonical Mission model
//! ([`MissionWorkspace`]): the PRIMARY mission directory, and the coordination
//! worktree/mission-directory names the mint and the probe share
//! ([`MissionWorkspace::coord_worktree_path`],
//! [`MissionWorkspace::coord_mission_dir_name`]). The detector itself is
//! deliberately placement-state-independent, exactly like Source: it reads a
//! **worktree file when one exists**, otherwise the declared **ref**, so a fresh
//! clone with no coordination worktree reports the same fork. That is why it does
//! not consult the materialization probe — the probe answers a different
//! question (whether a *write* surface exists) and would turn inputs Source
//! reads happily into refusals.
//!
//! # One prefix rule
//!
//! Source reuses `specify_cli.coordination.event_prefix.classify_prefix`, never a
//! second fork/prefix classifier. Native keeps the same semantics with the one
//! predicate [`prefix_is_proper`]: a stream is `forked` only when NEITHER surface's
//! event-id sequence is a prefix of the other's; any prefix relation is `prefix`
//! (Source collapses both prefix directions into that single state). The
//! coordination seed merger's classifier (`sk_workspace::coord`) is a private,
//! richer verdict for a different question; unifying the two into one shared
//! module is an explicit parent ask in the CF17 report, not silently duplicated
//! here.
//!
//! # Stated limits (disclosed, not waived)
//!
//! * A ref that exists only as a remote-tracking ref is not read: the ref
//!   fallback resolves **local** branch tips through the Git seam
//!   ([`sk_git::GitRepository::read_branch_tip`]). Source's `git show <branch>`
//!   DWIM would also resolve some remote-only spellings.
//! * A non-UTF-8 event log or `index.json` is a typed refusal; Source's
//!   `read_text(encoding="utf-8")` would raise there too, with a different type.
//! * `DM-*.md` basenames are rendered lossily from non-UTF-8 filesystem names
//!   (Source keeps Python's surrogate-escaped code points).
//! * The report's `topology` is the resolved Mission topology string; Source
//!   reports `meta.json`'s recorded string, which is `null` when the field is
//!   absent (legacy missions).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use sk_git::{BranchName, GitPath, GitRepository, StartPoint};
use sk_kernel::PythonString;
use sk_kernel::python_json::{PythonJsonValue, is_falsy, parse};
use sk_kernel::python_text::{is_whitespace, splitlines};

use crate::decision_fold::{DECISION_OPENED_EVENT, DECISION_RESOLVED_EVENT, python_str};
use crate::workspace::MissionWorkspace;

/// The status event-log stream compared per Mission (Source `STATUS_STREAM`).
pub const STATUS_STREAM: &str = "status.events.jsonl";

/// The decision log stream compared per Mission (Source `DECISION_LOG_STREAM`).
pub const DECISION_LOG_STREAM: &str = "decisions.events.jsonl";

/// The two streams compared, in Source order.
pub const STREAMS: [&str; 2] = [STATUS_STREAM, DECISION_LOG_STREAM];

/// The repository-relative directory Mission artifacts live under.
const SPECS_DIR: &str = "kitty-specs";

/// The decision ledger's sub-directory name.
const DECISIONS_DIR: &str = "decisions";

/// The ledger index document's file name.
const INDEX_FILE_NAME: &str = "index.json";

/// A decision-stream fork/prefix verdict (Source `StreamState`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamState {
    /// Only one side carries the stream (or there is no coordination surface).
    SingleHome,
    /// One side's event-id sequence is a prefix of the other's.
    Prefix,
    /// Neither side's sequence is a prefix of the other's — a real fork.
    Forked,
    /// Both sides are empty, or neither exists.
    Absent,
}

impl StreamState {
    /// The Source wire token.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SingleHome => "single_home",
            Self::Prefix => "prefix",
            Self::Forked => "forked",
            Self::Absent => "absent",
        }
    }
}

/// Where the decision ledger lives (Source `LedgerState`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LedgerState {
    /// Only the PRIMARY ledger exists.
    Primary,
    /// Only the coordination-branch ledger exists (a pre-fix Mission).
    CoordinationOnly,
    /// Both surfaces hold ledger content.
    Both,
    /// Neither surface holds any ledger content.
    Absent,
}

impl LedgerState {
    /// The Source wire token.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::CoordinationOnly => "coordination_only",
            Self::Both => "both",
            Self::Absent => "absent",
        }
    }
}

/// Which surface a read came from (Source `Literal["primary", "coordination"]`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForkSurface {
    /// The repository-root checkout's Mission directory.
    Primary,
    /// The coordination surface.
    Coordination,
}

impl ForkSurface {
    /// The Source wire token.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Coordination => "coordination",
        }
    }
}

/// Where a surface's bytes were read from (Source `Literal["worktree", "ref"]`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceSource {
    /// A file present in a checkout.
    Worktree,
    /// A blob read from the declared ref.
    Ref,
}

impl SurfaceSource {
    /// The Source wire token.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Worktree => "worktree",
            Self::Ref => "ref",
        }
    }
}

/// One surface's read of one event-log stream (Source `SurfaceLog`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SurfaceLog {
    /// Which surface this is.
    pub surface: ForkSurface,
    /// Where the bytes came from.
    pub source: SurfaceSource,
    /// The ref consulted, when the surface declares one.
    pub reference: Option<String>,
    /// The path Source reports (a checkout path, or the repository-relative ref
    /// path). A display identity only, exactly as in Source's report.
    pub path: String,
    /// The `event_id` of every decision-bearing row, in file order.
    pub event_ids: Vec<PythonString>,
}

/// The fork/prefix verdict for one stream, both surfaces (Source
/// `StreamForkFinding`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamForkFinding {
    /// The stream's file name.
    pub stream: String,
    /// The verdict.
    pub state: StreamState,
    /// The PRIMARY read.
    pub primary: Option<SurfaceLog>,
    /// The COORDINATION read, when a coordination surface is declared.
    pub coordination: Option<SurfaceLog>,
    /// Decision ids only PRIMARY carries, in code-point order.
    pub decisions_only_on_primary: Vec<PythonString>,
    /// Decision ids only COORDINATION carries, in code-point order.
    pub decisions_only_on_coordination: Vec<PythonString>,
    /// Decision ids both carry, in code-point order.
    pub decisions_on_both: Vec<PythonString>,
}

/// Where the decision ledger lives (Source `LedgerHomeFinding`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerHomeFinding {
    /// The home verdict.
    pub state: LedgerState,
    /// Index entries only the coordination ledger holds.
    pub entries_only_on_coordination: Vec<PythonString>,
    /// `DM-*.md` files only the coordination ledger holds.
    pub dm_files_only_on_coordination: Vec<PythonString>,
}

/// The full `doctor decisions` fork report for one Mission (Source
/// `DecisionsForkReport`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecisionsForkReport {
    /// The resolved Mission slug.
    pub mission_slug: String,
    /// The recorded topology, when the Mission records one.
    pub topology: Option<String>,
    /// One finding per compared stream, in Source order.
    pub streams: Vec<StreamForkFinding>,
    /// The ledger home.
    pub ledger: LedgerHomeFinding,
    /// Whether any stream is genuinely forked.
    pub forked: bool,
    /// Human-readable reconciliation guidance — never executed (Source C-003).
    pub reconcile_steps: Vec<String>,
}

/// A decision-stream diagnosis could not be completed.
#[derive(Debug, thiserror::Error)]
pub enum ForkError {
    /// The workspace authorities refused an address (a Mission path, an identity).
    #[error(transparent)]
    Workspace(#[from] crate::WorkspaceError),
    /// The Git seam refused a read.
    #[error(transparent)]
    Git(#[from] sk_git::GitError),
    /// A declared branch name is not usable as a Git branch.
    #[error("the declared ref {reference:?} is not a usable branch name")]
    InvalidReference {
        /// The refused reference.
        reference: String,
    },
    /// A surface file could not be read as Source reads it (UTF-8 text).
    #[error("could not read {path}: {detail}")]
    SurfaceUnreadable {
        /// The surface path (or repository-relative ref path).
        path: String,
        /// The underlying refusal.
        detail: String,
    },
    /// A resolvable coordination ref could not be *read* (Source
    /// `LedgerProbeError`): reporting it as an empty ledger would let teardown
    /// destroy the only copy, so it is distinct from genuine absence.
    #[error("could not read the decisions ledger on coordination ref {reference:?}: {detail}")]
    LedgerProbe {
        /// The coordination ref.
        reference: String,
        /// The underlying refusal.
        detail: String,
    },
}

/// The coordination side's addressing for one Mission.
struct CoordSide {
    mission_dir: PathBuf,
    branch: String,
    mission_subdir: String,
}

/// The coordination side Source `_resolve_coord_side` derives, or `None`.
fn fork_coord_side(workspace: &MissionWorkspace) -> Option<CoordSide> {
    // Source's defensive gate: an undeclared coordination branch or an
    // absent mid8 means there is no coordination surface to compare
    // against, so every stream/ledger degrades to PRIMARY-only — never a
    // fork and never `coordination_only`.
    let branch = workspace.coordination_branch()?;
    if branch.trim().is_empty() {
        return None;
    }
    if workspace.identity().mid8().is_empty() {
        return None;
    }
    Some(CoordSide {
        mission_dir: workspace
            .coord_worktree_path()?
            .join(SPECS_DIR)
            .join(workspace.coord_mission_dir_name()),
        branch,
        mission_subdir: workspace.coord_mission_dir_name(),
    })
}

/// Detect decision-stream forks and the ledger home for one Mission.
///
/// Source `detect_decision_forks`: read-only, never merges, never materializes.
/// An unreadable coordination ledger degrades to the PRIMARY-side verdict plus a
/// warning step (Source catches `LedgerProbeError` here and nowhere else).
pub fn detect_decision_forks(
    workspace: &MissionWorkspace,
) -> Result<DecisionsForkReport, ForkError> {
    let mission_slug = workspace.slug().to_owned();
    let topology = Some(workspace.topology().as_str().to_owned()).filter(|text| !text.is_empty());
    let primary_dir = workspace.mission_dir().to_path_buf();
    let target_branch = workspace
        .target_branch()
        .ok()
        .map(|branch| branch.trim().to_owned())
        .filter(|branch| !branch.is_empty());
    let coord = fork_coord_side(workspace);

    let mut streams = Vec::with_capacity(STREAMS.len());
    let mut reconcile_steps: Vec<String> = Vec::new();
    for stream in STREAMS {
        let (finding, steps) = one_stream(
            workspace.workspace().repository(),
            stream,
            &primary_dir,
            target_branch.as_deref(),
            coord.as_ref(),
        )?;
        streams.push(finding);
        reconcile_steps.extend(steps);
    }

    let ledger = match coordination_only_ledger(workspace) {
        Ok(ledger) => ledger,
        Err(ForkError::LedgerProbe { reference, detail }) => {
            reconcile_steps.push(format!(
                "the coordination ledger could not be read, so only the PRIMARY ledger is reported: \
                 could not read the decisions ledger on coordination ref {reference:?}: {detail}"
            ));
            primary_only_ledger(&primary_dir)?
        }
        Err(other) => return Err(other),
    };
    reconcile_steps.extend(ledger_reconcile_steps(
        &ledger,
        &mission_slug,
        &primary_dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        coord.as_ref(),
    ));

    let forked = streams
        .iter()
        .any(|finding| finding.state == StreamState::Forked);
    Ok(DecisionsForkReport {
        mission_slug,
        topology,
        streams,
        ledger,
        forked,
        reconcile_steps,
    })
}

/// Detect a pre-fix Mission whose ledger was committed only on the coordination
/// branch (Source `coordination_only_ledger`, FR-009c).
///
/// The coordination side always reads the **branch tip**, never the worktree
/// copy: the worktree may be stale and is not what teardown destroys.
pub fn coordination_only_ledger(
    workspace: &MissionWorkspace,
) -> Result<LedgerHomeFinding, ForkError> {
    let primary_dir = workspace.mission_dir().to_path_buf();
    let (primary_entries, primary_dm) = ledger_contents_worktree(&primary_dir)?;
    let mut coord_entries: BTreeSet<PythonString> = BTreeSet::new();
    let mut coord_dm: BTreeSet<PythonString> = BTreeSet::new();
    if let Some(coord) = fork_coord_side(workspace) {
        let (entries, dm) = ledger_contents_ref(workspace.workspace().repository(), &coord)?;
        coord_entries = entries;
        coord_dm = dm;
    }
    Ok(classify_ledger(
        primary_entries,
        primary_dm,
        coord_entries,
        coord_dm,
    ))
}

/// Whether the ledger needs rescuing to PRIMARY (Source contract rules 4/6):
/// `coordination_only`, or `both` with coordination-only residue.
pub fn ledger_is_coordination_only(ledger: &LedgerHomeFinding) -> bool {
    ledger.state == LedgerState::CoordinationOnly
        || (ledger.state == LedgerState::Both
            && (!ledger.entries_only_on_coordination.is_empty()
                || !ledger.dm_files_only_on_coordination.is_empty()))
}

/// One stream, both surfaces (Source `_one_stream`).
fn one_stream(
    repository: &GitRepository,
    stream: &str,
    primary_dir: &Path,
    target_branch: Option<&str>,
    coord: Option<&CoordSide>,
) -> Result<(StreamForkFinding, Vec<String>), ForkError> {
    let mut steps: Vec<String> = Vec::new();
    let primary_name = primary_dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (primary_log, primary_ids) = build_surface(
        repository,
        ForkSurface::Primary,
        stream,
        primary_dir,
        target_branch,
        &primary_name,
    )?;
    let coord_read = match coord {
        Some(coord) => Some(build_surface(
            repository,
            ForkSurface::Coordination,
            stream,
            &coord.mission_dir,
            Some(&coord.branch),
            &coord.mission_subdir,
        )?),
        None => None,
    };

    let coord_event_ids = coord_read.as_ref().map(|(log, _)| log.event_ids.as_slice());
    let state = stream_state(&primary_log.event_ids, coord_event_ids);
    let coord_ids = coord_read
        .as_ref()
        .map(|(_, ids)| ids.clone())
        .unwrap_or_default();

    let primary_set: BTreeSet<PythonString> = primary_ids.iter().cloned().collect();
    let coord_set: BTreeSet<PythonString> = coord_ids.iter().cloned().collect();
    let finding = StreamForkFinding {
        stream: stream.to_owned(),
        state,
        primary: Some(primary_log),
        coordination: coord_read.map(|(log, _)| log),
        decisions_only_on_primary: primary_set.difference(&coord_set).cloned().collect(),
        decisions_only_on_coordination: coord_set.difference(&primary_set).cloned().collect(),
        decisions_on_both: primary_set.intersection(&coord_set).cloned().collect(),
    };
    if finding.state == StreamState::Forked {
        steps.push(fork_reconcile_text(stream, &finding));
    }
    Ok((finding, steps))
}

/// Classify one stream from its two event-id sequences.
fn stream_state(primary_ids: &[PythonString], coord_ids: Option<&[PythonString]>) -> StreamState {
    let Some(coord_ids) = coord_ids else {
        return if primary_ids.is_empty() {
            StreamState::Absent
        } else {
            StreamState::SingleHome
        };
    };
    if primary_ids.is_empty() && coord_ids.is_empty() {
        return StreamState::Absent;
    }
    if primary_ids.is_empty() || coord_ids.is_empty() {
        return StreamState::SingleHome;
    }
    if is_prefix(coord_ids, primary_ids) || is_prefix(primary_ids, coord_ids) {
        StreamState::Prefix
    } else {
        StreamState::Forked
    }
}

/// Whether `left` is a prefix of `right` (including equality).
fn is_prefix(left: &[PythonString], right: &[PythonString]) -> bool {
    left.len() <= right.len() && right[..left.len()] == *left
}

/// Source `_fork_reconcile_text`, verbatim in substance.
fn fork_reconcile_text(stream: &str, finding: &StreamForkFinding) -> String {
    let only_primary = if finding.decisions_only_on_primary.is_empty() {
        "none".to_owned()
    } else {
        render_ids(&finding.decisions_only_on_primary)
    };
    let only_coord = if finding.decisions_only_on_coordination.is_empty() {
        "none".to_owned()
    } else {
        render_ids(&finding.decisions_only_on_coordination)
    };
    format!(
        "{stream} is forked: decisions only on PRIMARY = [{only_primary}]; decisions only on \
         COORDINATION = [{only_coord}]. This is never auto-merged (C-003) -- inspect both copies \
         by hand, decide which decisions to keep, and record the kept ones yourself on the \
         correct surface (never run --repair expecting it to merge or re-sequence a fork)."
    )
}

/// Read one stream from one surface (Source `_build_surface`).
fn build_surface(
    repository: &GitRepository,
    surface: ForkSurface,
    stream: &str,
    mission_dir: &Path,
    reference: Option<&str>,
    mission_subdir: &str,
) -> Result<(SurfaceLog, Vec<PythonString>), ForkError> {
    let (text, source, path) =
        surface_text(repository, mission_dir, reference, mission_subdir, stream)?;
    let rows = parse_jsonl_tolerant(&text);
    let (event_ids, decision_ids) = extract_ids(stream, &rows);
    Ok((
        SurfaceLog {
            surface,
            source,
            reference: reference.map(str::to_owned),
            path,
            event_ids,
        },
        decision_ids,
    ))
}

/// Read a surface file: the checkout copy when present, else the declared ref
/// (Source `_surface_text`).
fn surface_text(
    repository: &GitRepository,
    mission_dir: &Path,
    reference: Option<&str>,
    mission_subdir: &str,
    filename: &str,
) -> Result<(String, SurfaceSource, String), ForkError> {
    let candidate = mission_dir.join(filename);
    if candidate.exists() {
        let text = read_utf8_file(&candidate)?;
        return Ok((
            text,
            SurfaceSource::Worktree,
            candidate.to_string_lossy().into_owned(),
        ));
    }
    if let Some(reference) = reference {
        let relpath = format!("{SPECS_DIR}/{mission_subdir}/{filename}");
        if let Some(text) = read_at_ref(repository, reference, &relpath)? {
            return Ok((text, SurfaceSource::Ref, relpath));
        }
    }
    Ok((
        String::new(),
        SurfaceSource::Worktree,
        candidate.to_string_lossy().into_owned(),
    ))
}

/// Read a UTF-8 text file, refusing a non-UTF-8 or unreadable one by name.
fn read_utf8_file(path: &Path) -> Result<String, ForkError> {
    let bytes = std::fs::read(path).map_err(|error| ForkError::SurfaceUnreadable {
        path: path.to_string_lossy().into_owned(),
        detail: error.to_string(),
    })?;
    String::from_utf8(bytes).map_err(|error| ForkError::SurfaceUnreadable {
        path: path.to_string_lossy().into_owned(),
        detail: format!("not valid UTF-8: {error}"),
    })
}

/// Read a repository-relative blob from a declared branch, `None` when the
/// branch or path is genuinely absent.
fn read_at_ref(
    repository: &GitRepository,
    reference: &str,
    relpath: &str,
) -> Result<Option<String>, ForkError> {
    let Some(start) = ref_start_point(repository, reference)? else {
        return Ok(None);
    };
    let path = GitPath::from_git_bytes(relpath.as_bytes())?;
    let Some(bytes) = repository.read_blob_at(&start, &path)? else {
        return Ok(None);
    };
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|error| ForkError::SurfaceUnreadable {
            path: relpath.to_owned(),
            detail: format!("not valid UTF-8: {error}"),
        })
}

/// The `StartPoint` for a declared branch, `None` when the branch does not
/// resolve locally (genuine absence, not a failed probe).
fn ref_start_point(
    repository: &GitRepository,
    reference: &str,
) -> Result<Option<StartPoint>, ForkError> {
    let branch = BranchName::parse(reference).map_err(|_| ForkError::InvalidReference {
        reference: reference.to_owned(),
    })?;
    match repository.read_branch_tip(&branch)? {
        Some(_) => Ok(Some(StartPoint::Ref(branch.as_ref_name()))),
        None => Ok(None),
    }
}

/// Parse JSONL tolerantly: a malformed line is skipped and counted, exactly like
/// Source `_parse_jsonl_tolerant` (never crash, never silently drop a good row).
fn parse_jsonl_tolerant(text: &str) -> Vec<PythonJsonValue> {
    let mut rows = Vec::new();
    for raw_line in splitlines(text) {
        let line = trim_python_whitespace(raw_line);
        if line.is_empty() {
            continue;
        }
        if let Ok(value) = parse(line)
            && matches!(value, PythonJsonValue::ExactObject(_))
        {
            rows.push(value);
        }
    }
    rows
}

/// `(event_ids, decision_ids)` for the decision-bearing rows of one stream
/// (Source `_extract_ids`).
fn extract_ids(stream: &str, rows: &[PythonJsonValue]) -> (Vec<PythonString>, Vec<PythonString>) {
    let mut event_ids = Vec::new();
    let mut decision_ids = Vec::new();
    for row in rows {
        let Some(object) = exact_object(row) else {
            continue;
        };
        let decision_id = if stream == STATUS_STREAM {
            status_row_decision_id(object)
        } else {
            decision_log_row_decision_id(object)
        };
        let Some(decision_id) = decision_id else {
            continue;
        };
        if let Some(event_id) = object.get("event_id") {
            event_ids.push(python_str(event_id));
        }
        decision_ids.push(decision_id);
    }
    (event_ids, decision_ids)
}

/// `status.events.jsonl`: a decision row's `decision_point_id`.
fn status_row_decision_id(object: &sk_kernel::python_json::PythonJsonDict) -> Option<PythonString> {
    let event_type = object.get("event_type")?;
    let PythonJsonValue::String(event_type) = event_type else {
        return None;
    };
    let opened = event_type == &python_text(DECISION_OPENED_EVENT);
    let resolved = event_type == &python_text(DECISION_RESOLVED_EVENT);
    if !opened && !resolved {
        return None;
    }
    payload_truthy_id(object, "decision_point_id")
}

/// `decisions.events.jsonl`: a row's `decision_id`.
fn decision_log_row_decision_id(
    object: &sk_kernel::python_json::PythonJsonDict,
) -> Option<PythonString> {
    payload_truthy_id(object, "decision_id")
}

/// A truthy payload id, `str()`-rendered (Source `str(value) if value else None`).
fn payload_truthy_id(
    object: &sk_kernel::python_json::PythonJsonDict,
    key: &str,
) -> Option<PythonString> {
    let payload = object.get("payload").and_then(exact_object)?;
    let value = payload.get(key)?;
    if is_falsy(value) {
        return None;
    }
    Some(python_str(value))
}

/// The PRIMARY ledger read, straight off the repository-root checkout
/// (Source `_ledger_contents_worktree`).
fn ledger_contents_worktree(
    mission_dir: &Path,
) -> Result<(BTreeSet<PythonString>, BTreeSet<PythonString>), ForkError> {
    let decisions_dir = mission_dir.join(DECISIONS_DIR);
    let index_file = decisions_dir.join(INDEX_FILE_NAME);
    let entries = if index_file.exists() {
        index_entry_ids_from_text(&read_utf8_file(&index_file)?)
    } else {
        BTreeSet::new()
    };
    let mut dm_files = BTreeSet::new();
    if decisions_dir.exists() {
        let read_dir =
            std::fs::read_dir(&decisions_dir).map_err(|error| ForkError::SurfaceUnreadable {
                path: decisions_dir.to_string_lossy().into_owned(),
                detail: error.to_string(),
            })?;
        for entry in read_dir {
            let entry = entry.map_err(|error| ForkError::SurfaceUnreadable {
                path: decisions_dir.to_string_lossy().into_owned(),
                detail: error.to_string(),
            })?;
            if let Some(name) = dm_file_name(&entry.file_name().to_string_lossy()) {
                dm_files.insert(name);
            }
        }
    }
    Ok((entries, dm_files))
}

/// The COORDINATION ledger read — always the branch tip (Source
/// `_ledger_contents_ref`).
fn ledger_contents_ref(
    repository: &GitRepository,
    coord: &CoordSide,
) -> Result<(BTreeSet<PythonString>, BTreeSet<PythonString>), ForkError> {
    let index_relpath = format!(
        "{SPECS_DIR}/{}/{DECISIONS_DIR}/{INDEX_FILE_NAME}",
        coord.mission_subdir
    );
    let entries = match read_at_ref(repository, &coord.branch, &index_relpath)
        .map_err(|error| ledger_probe_error(&coord.branch, error))?
    {
        Some(text) => index_entry_ids_from_text(&text),
        None => BTreeSet::new(),
    };
    let tree_relpath = format!("{SPECS_DIR}/{}/{DECISIONS_DIR}", coord.mission_subdir);
    let mut dm_files = BTreeSet::new();
    for name in child_names_at_ref(repository, &coord.branch, &tree_relpath)
        .map_err(|error| ledger_probe_error(&coord.branch, error))?
    {
        if let Some(name) = dm_file_name(&name) {
            dm_files.insert(name);
        }
    }
    Ok((entries, dm_files))
}

/// A failed coordination-ledger read is Source's `LedgerProbeError`.
fn ledger_probe_error(reference: &str, error: ForkError) -> ForkError {
    ForkError::LedgerProbe {
        reference: reference.to_owned(),
        detail: error.to_string(),
    }
}

/// The PRIMARY-only ledger verdict (Source `_primary_only_ledger`).
fn primary_only_ledger(mission_dir: &Path) -> Result<LedgerHomeFinding, ForkError> {
    let (entries, dm) = ledger_contents_worktree(mission_dir)?;
    Ok(classify_ledger(
        entries,
        dm,
        BTreeSet::new(),
        BTreeSet::new(),
    ))
}

/// Names of the files directly under `tree_relpath` at the branch tip.
///
/// A ref that does not resolve, or a resolvable ref whose directory is absent,
/// is genuine absence (`[]`); only a failed read on a resolvable ref errors.
fn child_names_at_ref(
    repository: &GitRepository,
    reference: &str,
    tree_relpath: &str,
) -> Result<Vec<String>, ForkError> {
    let Some(start) = ref_start_point(repository, reference)? else {
        return Ok(Vec::new());
    };
    let prefix = GitPath::from_git_bytes(tree_relpath.as_bytes())?;
    let depth = Path::new(tree_relpath).components().count();
    let mut names = Vec::new();
    for path in repository.tree_paths_at(&start, Some(&prefix))? {
        let child = path.to_path_buf();
        if child.components().count() == depth + 1
            && let Some(name) = child.file_name()
        {
            names.push(name.to_string_lossy().into_owned());
        }
    }
    names.sort();
    Ok(names)
}

/// The `decision_id`s an `index.json` document holds, tolerantly (Source
/// `_index_entry_ids_from_text`): unreadable/malformed content is an empty set.
fn index_entry_ids_from_text(text: &str) -> BTreeSet<PythonString> {
    let mut ids = BTreeSet::new();
    if text.trim().is_empty() {
        return ids;
    }
    let Ok(document) = parse(text) else {
        return ids;
    };
    let PythonJsonValue::ExactObject(document) = document else {
        return ids;
    };
    let Some(PythonJsonValue::Array(entries)) = document.get("entries") else {
        return ids;
    };
    for entry in entries {
        if let Some(object) = exact_object(entry)
            && let Some(value) = object.get("decision_id")
        {
            ids.insert(python_str(value));
        }
    }
    ids
}

/// The `DM-<id>.md` basename of a filesystem name, or `None` (Source
/// `_is_dm_file`).
fn dm_file_name(name: &str) -> Option<PythonString> {
    if name.starts_with("DM-") && name.ends_with(".md") {
        Some(PythonString::from_scalars(name))
    } else {
        None
    }
}

/// Classify the ledger home from both surfaces (Source `_classify_ledger`).
fn classify_ledger(
    primary_entries: BTreeSet<PythonString>,
    primary_dm: BTreeSet<PythonString>,
    coord_entries: BTreeSet<PythonString>,
    coord_dm: BTreeSet<PythonString>,
) -> LedgerHomeFinding {
    let primary_has = !primary_entries.is_empty() || !primary_dm.is_empty();
    let coord_has = !coord_entries.is_empty() || !coord_dm.is_empty();
    let state = match (primary_has, coord_has) {
        (false, false) => LedgerState::Absent,
        (false, true) => LedgerState::CoordinationOnly,
        (true, false) => LedgerState::Primary,
        (true, true) => LedgerState::Both,
    };
    LedgerHomeFinding {
        state,
        entries_only_on_coordination: coord_entries
            .difference(&primary_entries)
            .cloned()
            .collect(),
        dm_files_only_on_coordination: coord_dm.difference(&primary_dm).cloned().collect(),
    }
}

/// The coordination-only-ledger remedy (Source `_ledger_reconcile_steps`).
fn ledger_reconcile_steps(
    ledger: &LedgerHomeFinding,
    mission_slug: &str,
    primary_dir_name: &str,
    coord: Option<&CoordSide>,
) -> Vec<String> {
    if !ledger_is_coordination_only(ledger) {
        return Vec::new();
    }
    let branch_note = coord
        .map(|coord| format!(" (branch {:?})", coord.branch))
        .unwrap_or_default();
    vec![format!(
        "decisions ledger exists only on the coordination branch{branch_note} for {mission_slug}: \
         run `spec-kitty doctor decisions --mission {mission_slug} --repair` to copy \
         decisions/index.json entries and DM-*.md files into the PRIMARY ledger \
         (kitty-specs/{primary_dir_name}/decisions/); it creates no commit -- commit the copy \
         yourself with `spec-kitty spec-commit -m \"...\" \
         kitty-specs/{primary_dir_name}/decisions/` or `spec-kitty accept`."
    )]
}

/// Render ids in code-point order, comma-separated (the report text shape).
fn render_ids(ids: &[PythonString]) -> String {
    ids.iter()
        .map(|id| {
            id.to_scalar_string()
                .unwrap_or_else(|code| format!("\\u{code:04x}"))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The canonical reader's object view.
fn exact_object(value: &PythonJsonValue) -> Option<&sk_kernel::python_json::PythonJsonDict> {
    match value {
        PythonJsonValue::ExactObject(object) => Some(object),
        _ => None,
    }
}

/// An ASCII literal as an exact code-point Python string.
fn python_text(text: &str) -> PythonString {
    PythonString::from_scalars(text)
}

/// Python `str.strip()` — strip code points where `str.isspace()` is true.
fn trim_python_whitespace(text: &str) -> &str {
    let start = text
        .char_indices()
        .find(|(_, character)| !is_whitespace(u32::from(*character)))
        .map(|(index, _)| index)
        .unwrap_or(text.len());
    let end = text
        .char_indices()
        .rev()
        .find(|(_, character)| !is_whitespace(u32::from(*character)))
        .map(|(index, character)| index + character.len_utf8())
        .unwrap_or(start);
    &text[start..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(values: &[&str]) -> Vec<PythonString> {
        values
            .iter()
            .map(|value| PythonString::from_scalars(value))
            .collect()
    }

    #[test]
    fn stream_state_matches_source_rules() {
        assert_eq!(stream_state(&[], None), StreamState::Absent);
        assert_eq!(stream_state(&ids(&["a"]), None), StreamState::SingleHome);
        assert_eq!(stream_state(&[], Some(&[])), StreamState::Absent);
        assert_eq!(
            stream_state(&ids(&["a"]), Some(&[])),
            StreamState::SingleHome
        );
        assert_eq!(
            stream_state(&ids(&["a", "b"]), Some(&ids(&["a"]))),
            StreamState::Prefix
        );
        assert_eq!(
            stream_state(&ids(&["a"]), Some(&ids(&["a", "b"]))),
            StreamState::Prefix
        );
        assert_eq!(
            stream_state(&ids(&["a", "b"]), Some(&ids(&["a", "c"]))),
            StreamState::Forked
        );
    }

    #[test]
    fn ledger_classification_matches_source_rules() {
        let empty = BTreeSet::new();
        assert_eq!(
            classify_ledger(empty.clone(), empty.clone(), empty.clone(), empty.clone()).state,
            LedgerState::Absent
        );
        assert_eq!(
            classify_ledger(
                BTreeSet::new(),
                BTreeSet::new(),
                BTreeSet::from([PythonString::from_scalars("d1")]),
                BTreeSet::new()
            )
            .state,
            LedgerState::CoordinationOnly
        );
        let both = classify_ledger(
            BTreeSet::from([PythonString::from_scalars("d1")]),
            BTreeSet::new(),
            BTreeSet::from([
                PythonString::from_scalars("d1"),
                PythonString::from_scalars("d2"),
            ]),
            BTreeSet::from([PythonString::from_scalars("DM-d2.md")]),
        );
        assert_eq!(both.state, LedgerState::Both);
        assert!(ledger_is_coordination_only(&both));
        assert_eq!(
            both.entries_only_on_coordination,
            vec![PythonString::from_scalars("d2")]
        );
    }

    #[test]
    fn the_index_id_reader_is_tolerant_and_exact() {
        let text = r#"{"version":1,"mission_id":"m","entries":[{"decision_id":"d1"},{"decision_id":"d2"}]}"#;
        assert_eq!(
            index_entry_ids_from_text(text),
            BTreeSet::from([
                PythonString::from_scalars("d1"),
                PythonString::from_scalars("d2")
            ])
        );
        for bad in ["", "   ", "{not json", "[]", r#"{"entries":{}}"#] {
            assert!(index_entry_ids_from_text(bad).is_empty(), "{bad:?}");
        }
    }

    #[test]
    fn jsonl_rows_are_parsed_tolerantly_and_ids_extracted_per_stream() {
        let status = concat!(
            "{\"event_id\":\"e1\",\"event_type\":\"DecisionPointOpened\",\"payload\":{\"decision_point_id\":\"d1\"}}\n",
            "not json\n",
            "{\"event_id\":\"e2\",\"event_type\":\"WPStatusChanged\",\"payload\":{\"decision_point_id\":\"nope\"}}\n",
            "{\"event_type\":\"DecisionPointResolved\",\"payload\":{\"decision_point_id\":\"\"}}\n"
        );
        let rows = parse_jsonl_tolerant(status);
        assert_eq!(rows.len(), 3);
        let (event_ids, decision_ids) = extract_ids(STATUS_STREAM, &rows);
        assert_eq!(event_ids, ids(&["e1"]));
        assert_eq!(decision_ids, ids(&["d1"]));

        let decision_log = "{\"event_id\":\"x1\",\"payload\":{\"decision_id\":\"d9\"}}\n";
        let (event_ids, decision_ids) =
            extract_ids(DECISION_LOG_STREAM, &parse_jsonl_tolerant(decision_log));
        assert_eq!(event_ids, ids(&["x1"]));
        assert_eq!(decision_ids, ids(&["d9"]));
    }

    #[test]
    fn a_fork_message_names_both_sides_and_refuses_auto_merge() {
        let finding = StreamForkFinding {
            stream: STATUS_STREAM.to_owned(),
            state: StreamState::Forked,
            primary: None,
            coordination: None,
            decisions_only_on_primary: ids(&["p1"]),
            decisions_only_on_coordination: ids(&["c1", "c2"]),
            decisions_on_both: Vec::new(),
        };
        let text = fork_reconcile_text(STATUS_STREAM, &finding);
        assert!(text.contains("decisions only on PRIMARY = [p1]"));
        assert!(text.contains("decisions only on COORDINATION = [c1, c2]"));
        assert!(text.contains("never auto-merged (C-003)"));
    }

    #[test]
    fn the_ledger_remedy_names_the_branch_and_the_mission() {
        let ledger = LedgerHomeFinding {
            state: LedgerState::CoordinationOnly,
            entries_only_on_coordination: ids(&["d1"]),
            dm_files_only_on_coordination: Vec::new(),
        };
        let steps = ledger_reconcile_steps(
            &ledger,
            "001-x",
            "001-x",
            Some(&CoordSide {
                mission_dir: PathBuf::from("/tmp/coord"),
                branch: "kitty/mission-001-x-01KN2371".to_owned(),
                mission_subdir: "001-x-01KN2371".to_owned(),
            }),
        );
        assert_eq!(steps.len(), 1);
        assert!(steps[0].contains("--mission 001-x --repair"));
        assert!(steps[0].contains("kitty/mission-001-x-01KN2371"));
        assert!(steps[0].contains("creates no commit"));
        assert!(
            ledger_reconcile_steps(
                &LedgerHomeFinding {
                    state: LedgerState::Primary,
                    entries_only_on_coordination: Vec::new(),
                    dm_files_only_on_coordination: Vec::new(),
                },
                "001-x",
                "001-x",
                None
            )
            .is_empty()
        );
    }
}
