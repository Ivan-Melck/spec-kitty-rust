//! Source-compatible **acceptance execution**: matrix population, negative
//! invariant enforcement, locked persistence, and read-only diagnosis.
//!
//! The acceptance *policy* ([`sk_status::acceptance::source`]) is pure: it judges
//! facts it is handed. Collecting those facts is
//! [`crate::acceptance_source`]'s job. Between the two sits the one thing a
//! policy must never do itself: **mutating and persisting the matrix**.
//!
//! This module reproduces the Source order in
//! `acceptance/gates_core._evaluate_acceptance_matrix` and
//! `acceptance/matrix.locked_reread_splice_and_write`:
//!
//! 1. **Resolve the matrix surface once** (coordination-aware). Source's
//!    `_acceptance_matrix_read_dir` reads the coordination worktree when the
//!    Mission routes through coordination; the resolution happens BEFORE the slow
//!    checks and is never re-derived. [`crate::coord::read_coordination_surface`]
//!    is that authority here, including its two fail-closed states: an
//!    unmaterialized coordination worktree and a deleted coordination branch are
//!    reported as *cannot-evaluate*, never as "read the primary copy instead".
//! 2. **Absent matrix ⇒ blocked** (`acceptance_matrix`). The message and the
//!    remediation text are the collector's own (one string authority).
//! 3. **Diagnose ⇒ read-only** (`--diagnose`): the matrix is judged as it stands,
//!    negative invariants are NOT executed, and the policy discloses that with
//!    the `negative_invariants` skipped check. Nothing is written, no lock is
//!    taken.
//! 4. **Mutate leg**: snapshot the matrix, then — *outside* the lock —
//!    auto-derive pending `code_review` rows from the Mission's event-sourced
//!    review evidence and run every pending negative invariant. Both are slow
//!    and the lock must not cover them.
//! 5. **Persist under the Mission lock**: re-read the matrix fresh, splice in
//!    ONLY the rows this run owns, and write. A verdict another writer committed
//!    while this run was checking is therefore preserved, not erased. A lock
//!    timeout fails closed — nothing is written and the run records a
//!    `acceptance_matrix_lock` blocked check.
//! 6. **Judge the FRESH matrix**, never accept's pre-lock snapshot.
//!
//! Ownership (Source spec.md "Row ownership rule"): a judged row replaces the
//! fresh row only when it was `pending` in the snapshot, is no longer `pending`
//! in the judgement, is STILL `pending` in the fresh read, and its
//! judgement-defining fields are unchanged (`proof_type`/`description` for a
//! criterion; `verification_method`/`verification_command`/`scope` for an
//! invariant). A row re-authored underneath the run is a new row, and a stale
//! judgement must not attach to it.

use std::path::{Path, PathBuf};
use std::time::Duration;

use sk_kernel::locks::LockWait;
use sk_status::acceptance::source::{AcceptanceDiagnostic, MatrixEvidence};
use sk_status::acceptance::{
    AcceptanceCriterion, AcceptanceMatrix, InvariantJudgeContext, ReviewEvidence,
    SCAFFOLD_TODO_MARKER, enforce_negative_invariants, review_evidence_population,
};
use sk_status::mutation_lock::{MISSION_WRITE_LOCK_TIMEOUT, acquire_mission_lock};

use crate::acceptance_source::{matrix_missing_message, read_acceptance_matrix};
use crate::error::WorkspaceError;
use crate::workspace::MissionWorkspace;

/// Source `LifecyclePhase.ACCEPT`, the phase a pre-consolidation judgement is
/// stamped with.
pub const ACCEPT_PHASE_NAME: &str = "ACCEPT";

/// Source's auto-derivation note appended to a populated criterion's `notes`.
pub const AUTO_DERIVED_NOTE: &str =
    "Auto-derived from WP review evidence (IC-04 gate-side capture, FR-008).";

/// One matrix-execution request.
///
/// Every field is a collected fact. The caller owns where the review evidence
/// came from (the canonical event log) and which lanes are tracked; this module
/// owns what Source does with them.
#[derive(Clone, Copy)]
pub struct AcceptanceMatrixRequest<'a> {
    /// The Mission workspace (its primary root and recorded topology).
    pub workspace: &'a MissionWorkspace,
    /// Tracked work packages and their canonical lanes.
    pub lanes: &'a [(String, String)],
    /// Event-sourced review verdicts, when the caller resolved any.
    pub review_evidence: &'a [ReviewEvidence],
    /// `true` for the read-only `--diagnose` leg.
    pub diagnose: bool,
    /// Lock wait for the persistence step.
    pub lock_timeout: Duration,
}

impl<'a> AcceptanceMatrixRequest<'a> {
    /// A mutating request with Source's bounded lock wait.
    pub fn new(workspace: &'a MissionWorkspace, lanes: &'a [(String, String)]) -> Self {
        Self {
            workspace,
            lanes,
            review_evidence: &[],
            diagnose: false,
            lock_timeout: MISSION_WRITE_LOCK_TIMEOUT,
        }
    }
}

/// What the matrix step established, plus the diagnostics the policy expects to
/// receive from it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AcceptanceMatrixRun {
    /// The surface the gate resolved, when it resolved one.
    pub directory: Option<PathBuf>,
    /// The evidence to hand the acceptance policy.
    pub evidence: MatrixEvidence,
    /// Blocking/unblocking notes the policy would otherwise not know about
    /// (currently only a lock timeout).
    pub activity_issues: Vec<String>,
    /// Blocked checks this step recorded (`acceptance_matrix`,
    /// `acceptance_matrix_cannot_evaluate`, `acceptance_matrix_lock`).
    pub blocked_checks: Vec<AcceptanceDiagnostic>,
    /// Informational skipped checks this step recorded.
    pub skipped_checks: Vec<AcceptanceDiagnostic>,
}

impl AcceptanceMatrixRun {
    fn new(directory: PathBuf) -> Self {
        Self {
            directory: Some(directory),
            evidence: MatrixEvidence::NotRequired,
            activity_issues: Vec::new(),
            blocked_checks: Vec::new(),
            skipped_checks: Vec::new(),
        }
    }
}

/// The four matrix checks `_append_skipped_lane_checks` can record, in order.
const MATRIX_CHECK_LABELS: [(&str, &str); 4] = [
    (
        "acceptance_matrix_presence",
        "Acceptance matrix presence check",
    ),
    (
        "acceptance_matrix_evidence",
        "Acceptance matrix evidence validation",
    ),
    ("negative_invariants", "Negative invariant execution"),
    (
        "acceptance_matrix_verdict",
        "Acceptance matrix verdict evaluation",
    ),
];

/// Record the four skipped lane checks with `reason` (Source
/// `_append_skipped_lane_checks`), optionally including the presence check.
fn append_skipped(checks: &mut Vec<AcceptanceDiagnostic>, reason: &str, include_presence: bool) {
    for (check, label) in MATRIX_CHECK_LABELS
        .iter()
        .skip(if include_presence { 0 } else { 1 })
    {
        checks.push(AcceptanceDiagnostic::new(
            *check,
            format!("{label} skipped: {reason}"),
        ));
    }
}

/// Resolve the coordination-aware matrix surface, mapping Source's two
/// cannot-evaluate states instead of degrading them to the primary copy.
fn matrix_surface(
    workspace: &MissionWorkspace,
) -> Result<Result<PathBuf, (String, String)>, WorkspaceError> {
    match crate::coord::read_coordination_surface(workspace) {
        Ok(read) => Ok(Ok(read.read_dir)),
        Err(WorkspaceError::CoordinationWorktreeUnmaterialized { mission, branch }) => Ok(Err((
            "coordination_worktree_unmaterialized".to_owned(),
            format!(
                "the coordination worktree for mission {mission} on branch {branch} has not been materialized; the coordination surface cannot hold the acceptance matrix yet"
            ),
        ))),
        Err(WorkspaceError::CoordinationBranchDeleted { mission, branch }) => Ok(Err((
            "coordination_branch_deleted".to_owned(),
            format!(
                "mission {mission} records coordination branch {branch}, which no longer exists; refusing to judge the primary copy as if it were the coordination surface"
            ),
        ))),
        Err(error) => Err(error),
    }
}

/// The provenance context a freshly judged invariant is stamped with.
fn judge_context(
    workspace: &MissionWorkspace,
    directory: &Path,
) -> Result<InvariantJudgeContext, WorkspaceError> {
    let repository = workspace.workspace().repository();
    let reference = repository
        .head_commit()
        .map_err(WorkspaceError::Git)?
        .map(|oid| oid.to_string())
        .unwrap_or_else(|| "HEAD".to_owned());
    let surface = if directory == workspace.mission_dir() {
        "primary"
    } else {
        "coord"
    };
    Ok(InvariantJudgeContext {
        reference,
        surface: surface.to_owned(),
        phase: ACCEPT_PHASE_NAME.to_owned(),
    })
}

/// Auto-derive pending `code_review` rows from the Mission's review evidence.
///
/// Mirrors `populate_criteria_from_review_evidence`: forward-only (a row already
/// judged is never re-judged), the empty `finalize-tasks` placeholder is
/// skipped, and the derivation is all-or-nothing per Mission (the caller's
/// evidence set is only usable when every tracked WP carries an approved,
/// event-sourced review verdict — [`review_evidence_population`] enforces that).
fn populate_criteria(
    criteria: &mut [AcceptanceCriterion],
    lanes: &[(String, String)],
    evidence: &[ReviewEvidence],
) {
    let pending: Vec<&AcceptanceCriterion> = criteria
        .iter()
        .filter(|row| row.pass_fail == "pending" && !is_empty_scaffold(row))
        .collect();
    if pending.is_empty() {
        return;
    }
    let rows = review_evidence_population(criteria, lanes, evidence);
    if rows.is_empty() {
        return;
    }
    let verified_at = sk_kernel::clock::now_utc_iso();
    for (criterion_id, verified_by, evidence_text) in rows {
        let Some(row) = criteria
            .iter_mut()
            .find(|row| row.criterion_id == criterion_id)
        else {
            continue;
        };
        row.pass_fail = "pass".to_owned();
        row.verified_by = Some(verified_by);
        row.verified_at = Some(verified_at.clone());
        row.evidence = Some(evidence_text);
        row.notes = Some(match row.notes.take() {
            Some(notes) if !notes.is_empty() => format!("{notes} {AUTO_DERIVED_NOTE}"),
            _ => AUTO_DERIVED_NOTE.to_owned(),
        });
    }
}

/// Source `_is_empty_scaffold`: the contentless `finalize-tasks` placeholder.
fn is_empty_scaffold(criterion: &AcceptanceCriterion) -> bool {
    criterion.description == SCAFFOLD_TODO_MARKER
}

/// Splice this run's judged rows into the freshly re-read matrix, in place.
///
/// Rows are matched positionally between `snapshot` and `judged` only after the
/// id lookup, so a concurrently reordered matrix keeps each row's own identity.
pub fn splice_owned_rows(
    fresh: &mut AcceptanceMatrix,
    snapshot: &AcceptanceMatrix,
    judged: &AcceptanceMatrix,
) {
    // Criteria: judgement-defining fields are `proof_type` + `description`.
    for judged_row in &judged.criteria {
        let Some(snapshot_row) = snapshot
            .criteria
            .iter()
            .find(|row| row.criterion_id == judged_row.criterion_id)
        else {
            continue;
        };
        let Some(position) = fresh
            .criteria
            .iter()
            .position(|row| row.criterion_id == judged_row.criterion_id)
        else {
            continue;
        };
        let fresh_row = &fresh.criteria[position];
        if snapshot_row.pass_fail != "pending"
            || judged_row.pass_fail == "pending"
            || fresh_row.pass_fail != "pending"
            || snapshot_row.proof_type != fresh_row.proof_type
            || snapshot_row.description != fresh_row.description
        {
            continue;
        }
        let _ = fresh_row;
        fresh.criteria[position] = judged_row.clone();
    }

    // Negative invariants: `verification_method` + `verification_command` + `scope`.
    for judged_row in &judged.negative_invariants {
        let Some(snapshot_row) = snapshot
            .negative_invariants
            .iter()
            .find(|row| row.invariant_id == judged_row.invariant_id)
        else {
            continue;
        };
        let Some(position) = fresh
            .negative_invariants
            .iter()
            .position(|row| row.invariant_id == judged_row.invariant_id)
        else {
            continue;
        };
        let fresh_row = &fresh.negative_invariants[position];
        if snapshot_row.result != "pending"
            || judged_row.result == "pending"
            || fresh_row.result != "pending"
            || snapshot_row.verification_method != fresh_row.verification_method
            || snapshot_row.verification_command != fresh_row.verification_command
            || snapshot_row.scope != fresh_row.scope
        {
            continue;
        }
        fresh.negative_invariants[position] = judged_row.clone();
    }
}

/// Execute the acceptance-matrix step for `request`.
///
/// # Errors
///
/// A genuine failure (unreadable matrix, unreadable status surface, Git probe
/// failure) is typed. Source's own "cannot evaluate" states are *reported*
/// (activity issue + blocked check), not raised: an unmaterialized coordination
/// surface is a fact about the Mission, not a crash.
pub fn execute_acceptance_matrix(
    request: &AcceptanceMatrixRequest<'_>,
) -> Result<AcceptanceMatrixRun, WorkspaceError> {
    let workspace = request.workspace;
    let feature_dir = workspace.mission_dir().to_path_buf();
    let directory = match matrix_surface(workspace)? {
        Ok(directory) => directory,
        Err((reason, detail)) => {
            let mut run = AcceptanceMatrixRun::new(feature_dir);
            let message = format!(
                "Acceptance matrix cannot be evaluated ({reason}): {detail} [surface={}]",
                workspace.topology().as_str()
            );
            run.activity_issues.push(message.clone());
            run.blocked_checks.push(AcceptanceDiagnostic::new(
                "acceptance_matrix_cannot_evaluate",
                message,
            ));
            append_skipped(&mut run.skipped_checks, &reason, true);
            run.directory = None;
            return Ok(run);
        }
    };

    let mut run = AcceptanceMatrixRun::new(directory.clone());
    let Some(pre_lock) = read_acceptance_matrix(&directory)? else {
        let message = matrix_missing_message(&directory);
        run.activity_issues.push(message.clone());
        run.blocked_checks
            .push(AcceptanceDiagnostic::new("acceptance_matrix", message));
        append_skipped(
            &mut run.skipped_checks,
            "acceptance-matrix.json is missing",
            false,
        );
        run.evidence = MatrixEvidence::Missing {
            message: matrix_missing_message(&directory),
            directory: Some(directory.display().to_string()),
        };
        return Ok(run);
    };

    if request.diagnose {
        if !pre_lock.negative_invariants.is_empty() {
            run.skipped_checks.push(AcceptanceDiagnostic::new(
                "negative_invariants",
                "Negative invariant execution skipped: diagnose mode is read-only",
            ));
        }
        run.evidence = MatrixEvidence::Judged {
            matrix: pre_lock,
            directory: Some(directory.display().to_string()),
            negative_invariants_executed: false,
        };
        return Ok(run);
    }

    // ---- Slow phase, OUTSIDE the lock ------------------------------------
    let snapshot = pre_lock.clone();
    let mut judged = pre_lock;
    populate_criteria(&mut judged.criteria, request.lanes, request.review_evidence);
    if !judged.negative_invariants.is_empty() {
        let context = judge_context(workspace, &directory)?;
        judged.negative_invariants = enforce_negative_invariants(
            workspace.workspace().primary_root(),
            &judged.negative_invariants,
            Some(&context),
        );
    }

    // ---- Locked re-read + splice + write ---------------------------------
    let lock = match acquire_mission_lock(
        &feature_dir,
        Some(workspace.workspace().primary_root()),
        LockWait::Bounded(request.lock_timeout),
    ) {
        Ok(lock) => lock,
        Err(error) => {
            let message = format!(
                "Acceptance matrix lock timed out while recording accept's judgement: {error}"
            );
            run.activity_issues.push(message.clone());
            run.blocked_checks
                .push(AcceptanceDiagnostic::new("acceptance_matrix_lock", message));
            append_skipped(
                &mut run.skipped_checks,
                "mission status lock timed out",
                true,
            );
            run.evidence = MatrixEvidence::GateStopped {
                reason: "mission status lock timed out".to_owned(),
            };
            return Ok(run);
        }
    };
    let mut fresh = read_acceptance_matrix(&directory)?.unwrap_or_else(|| {
        // The matrix vanished between the pre-lock read and this re-read: start
        // from an empty, schema-valid matrix rather than crashing. The splice
        // below still inserts exactly the rows this run owns.
        AcceptanceMatrix::new(workspace.slug().to_owned(), Vec::new())
    });
    splice_owned_rows(&mut fresh, &snapshot, &judged);
    fresh.write_to(&directory).map_err(|source| {
        WorkspaceError::Kernel(sk_kernel::KernelError::Io {
            action: "write the acceptance matrix",
            path: AcceptanceMatrix::path_in(&directory),
            source,
        })
    })?;
    drop(lock);

    // ---- Judge the FRESH matrix (never the pre-lock snapshot) ------------
    run.evidence = MatrixEvidence::Judged {
        matrix: fresh,
        directory: Some(directory.display().to_string()),
        negative_invariants_executed: true,
    };
    Ok(run)
}

/// The read-only `--diagnose` leg, for callers that only want the evidence.
pub fn diagnose_acceptance_matrix(
    request: &AcceptanceMatrixRequest<'_>,
) -> Result<AcceptanceMatrixRun, WorkspaceError> {
    execute_acceptance_matrix(&AcceptanceMatrixRequest {
        diagnose: true,
        ..*request
    })
}

/// Adapter: a run as the policy's matrix evidence.
pub fn matrix_evidence(run: &AcceptanceMatrixRun) -> MatrixEvidence {
    run.evidence.clone()
}
