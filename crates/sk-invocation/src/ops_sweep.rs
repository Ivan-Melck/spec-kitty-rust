//! `doctor ops --close-stale` — the stale-Op sweep, through the canonical
//! executor.
//!
//! # Source
//!
//! `specify_cli.doctor.ops.close_stale_ops` (`src/specify_cli/doctor/ops.py`),
//! driven by `cli/commands/doctor.py::_run_ops_sweep` and the `ops` command's
//! `--close-stale` / `--threshold` options, against
//! `contracts/doctor-ops-close-stale.md` (Mission
//! `do-dispatch-open-op-lifecycle-01KTSJ2H`).
//!
//! The sweep:
//!
//! 1. lists the open Ops with the **same read-only detector** the report path
//!    uses ([`sk_workspace::doctor::audit_open_ops`]: no `completed` event and
//!    no closure on the append-only spine) — there is no second classifier and
//!    the read-only listing stays unchanged;
//! 2. for each open Op, reads `profile_id` / `started_at` from its first row,
//!    computes the age in hours (`None` when `started_at` is unparseable) and
//!    decides staleness as `threshold == 0 || age is None || age > threshold`;
//! 3. closes a stale Op through
//!    [`ProfileInvocationExecutor::complete_invocation`] with
//!    `outcome = abandoned` and `closed_by = doctor_sweep` — the one sanctioned
//!    close path, which records the closure on the append-only spine
//!    (`kitty-ops/op-closures.jsonl`) and never rewrites the byte-frozen
//!    per-record file (#4397) — then auto-commits that spine and reports the
//!    commit outcome per entry;
//! 4. records a concurrent manual close (`AlreadyClosed`) as
//!    `already_closed` (not a failure), any other per-Op exception as an
//!    `error` entry, and **continues** with the remaining Ops;
//! 5. exits 1 when any entry carries an error or when open-but-fresh Ops
//!    remain, else 0.
//!
//! # What this module does not own
//!
//! - The Op-store layout and the closure reader: [`crate::EVENTS_DIR`],
//!   [`crate::OP_CLOSURES_FILENAME`], [`crate::INDEX_PATH`],
//!   [`crate::closed_invocation_ids`].
//! - The close itself: the executor, the writer's spine append and the Op
//!   commit service.
//! - The `completed`-event scan policy: `sk_workspace::doctor::ops`.
//! - Rendering *policy*: the human lines below are Source's text with Rich
//!   markup stripped; they live here only because the human renderer module
//!   belongs to another lane ([`crate::ops_sweep`] is the single authority for
//!   the sweep, so this is not a second one). The JSON document is rendered by
//!   the canonical Kernel `PythonJsonValue` encoder at `indent=2`, i.e. the
//!   exact text Source hands to `json.dumps(report.to_dict(), indent=2)`.
//!
//! # Deliberate, disclosed boundaries
//!
//! - Source passes an `InvocationSaaSPropagator` so a swept `abandoned`
//!   completion reaches SaaS. The native tree has no hosted propagator at all
//!   (the hosted surfaces are removed from open-core), so nothing is
//!   propagated: this is a real functional gap, not a simulated success.
//! - Source reads the spine **once per sweep** (a snapshot, #4424) and may
//!   therefore append a duplicate spine line when another process closes the
//!   same Op mid-sweep; the native executor re-reads the spine per Op, so that
//!   race is reported `already_closed` instead. No state is corrupted either
//!   way; the counts can differ only in that concurrent race.
//! - A per-Op failure is recorded as the executor error's `Display` text;
//!   Source records `repr(exc)`. Both are error entries, both continue.
//! - An invocation id that is not a 26-character Crockford ULID cannot be
//!   closed through the canonical id boundary and is reported as a per-Op
//!   error. Source's own records are minted as ULIDs; a non-ULID file name is a
//!   corrupt record, and it is refused rather than closed under an invented id.

use std::collections::BTreeSet;
use std::path::Path;

use sk_kernel::clock::{UtcDateTime, UtcZone};
use sk_workspace::doctor::LIFECYCLE_LOG_RELATIVE_PATH;
use sk_workspace::doctor::{DoctorError, OpStoreLayout, audit_open_ops};

use crate::executor::{CompletionRequest, ExecutorError, ProfileInvocationExecutor};
use crate::op_commit::OpCommitOutcome;
use crate::python_json::{PythonJsonDict, PythonJsonValue, is_falsy, parse, python_float_repr};
use crate::record::{ClosedBy, OpOutcome};
use crate::writer::{EVENTS_DIR, INDEX_PATH, OP_CLOSURES_FILENAME, closed_invocation_ids};

/// `SweepOpEntry.commit` prefix for a sweep close whose spine commit was
/// deliberately skipped (#6410): the closure stays on disk, uncommitted.
pub const SWEEP_COMMIT_SKIPPED_PREFIX: &str = "skipped:";

/// Source's `--threshold` default (`ops(threshold=None)` → `24.0` hours).
pub const DEFAULT_SWEEP_THRESHOLD_HOURS: f64 = 24.0;

/// The per-Op action the sweep took.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SweepAction {
    /// Open but younger than the threshold; never closed.
    None,
    /// Closed as `abandoned` through the canonical executor.
    ClosedAbandoned,
    /// A concurrent close won the race; not a failure.
    AlreadyClosed,
}

impl SweepAction {
    /// Source's `action_taken` spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::ClosedAbandoned => "closed_abandoned",
            Self::AlreadyClosed => "already_closed",
        }
    }
}

/// One per-Op sweep result (`SweepOpEntry`).
#[derive(Clone, Debug, PartialEq)]
pub struct SweepOpEntry {
    /// The Op's invocation id (the record file stem), verbatim.
    pub invocation_id: String,
    /// The record's first-row `profile_id`, `""` when absent/falsy.
    pub profile_id: String,
    /// The record's first-row `started_at`, `""` when absent/falsy.
    pub started_at: String,
    /// Age in hours; `None` when `started_at` is unparseable.
    pub age_hours: Option<f64>,
    /// What the sweep did.
    pub action_taken: SweepAction,
    /// Explainable non-closure when `started_at` could not be parsed.
    pub parse_warning: Option<String>,
    /// The per-Op failure, when the close raised.
    pub error: Option<String>,
    /// The spine-commit outcome, for a close this sweep performed.
    pub commit: Option<String>,
}

impl SweepOpEntry {
    /// Source's `age_hours` text in the human table: `f"{age:.1f}"`, or `?`
    /// when there is no age. Python renders a non-finite age as `nan`/`inf`.
    pub fn age_text(&self) -> String {
        match self.age_hours {
            Some(age) if age.is_finite() => format!("{age:.1}"),
            Some(age) => python_float_repr(age),
            None => "?".to_owned(),
        }
    }

    /// Source's `action_taken` cell: the action, or the error, never both.
    pub fn action_text(&self) -> String {
        match &self.error {
            Some(error) => format!("error: {error}"),
            None => self.action_taken.as_str().to_owned(),
        }
    }

    /// Source's `commit or "-"` cell.
    pub fn commit_text(&self) -> String {
        self.commit.clone().unwrap_or_else(|| "-".to_owned())
    }

    fn to_python_json(&self) -> PythonJsonValue {
        let mut object = PythonJsonDict::new();
        object.insert(
            key("invocation_id"),
            PythonJsonValue::scalar(&self.invocation_id),
        );
        object.insert(key("profile_id"), PythonJsonValue::scalar(&self.profile_id));
        object.insert(key("started_at"), PythonJsonValue::scalar(&self.started_at));
        object.insert(
            key("age_hours"),
            match self.age_hours {
                Some(age) => PythonJsonValue::Float(age),
                None => PythonJsonValue::Null,
            },
        );
        object.insert(
            key("action_taken"),
            PythonJsonValue::scalar(self.action_taken.as_str()),
        );
        if let Some(warning) = &self.parse_warning {
            object.insert(key("parse_warning"), PythonJsonValue::scalar(warning));
        }
        if let Some(error) = &self.error {
            object.insert(key("error"), PythonJsonValue::scalar(error));
        }
        if let Some(commit) = &self.commit {
            object.insert(key("commit"), PythonJsonValue::scalar(commit));
        }
        PythonJsonValue::ExactObject(object)
    }
}

/// The whole sweep report (`SweepReport`).
#[derive(Clone, Debug, PartialEq)]
pub struct SweepReport {
    /// Every open Op considered, in record-path order.
    pub open_ops: Vec<SweepOpEntry>,
    /// How many Ops this sweep closed as abandoned.
    pub swept: usize,
    /// How many open Ops were left alone as fresh.
    pub skipped_fresh: usize,
    /// The threshold that was applied, in hours.
    pub threshold_hours: f64,
}

impl SweepReport {
    /// A sweep report for `threshold_hours` with nothing considered yet.
    pub fn new(threshold_hours: f64) -> Self {
        Self {
            open_ops: Vec::new(),
            swept: 0,
            skipped_fresh: 0,
            threshold_hours,
        }
    }

    /// Whether any entry carries a per-Op error.
    pub fn has_errors(&self) -> bool {
        self.open_ops.iter().any(|entry| entry.error.is_some())
    }

    /// Source's exit rule: `1` on a per-Op error or a remaining fresh Op.
    ///
    /// `already_closed` is never a failure.
    pub fn exit_code(&self) -> u8 {
        u8::from(self.has_errors() || self.skipped_fresh > 0)
    }

    /// The repo-relative closure spine, as Source prints it
    /// (`OP_CLOSURES_RELATIVE_PATH.as_posix()`).
    pub fn closure_spine() -> String {
        format!("{EVENTS_DIR}/{OP_CLOSURES_FILENAME}")
    }

    /// Source's `SweepReport.to_dict()`, in declaration order.
    pub fn to_python_json(&self) -> PythonJsonValue {
        let mut object = PythonJsonDict::new();
        object.insert(
            key("open_ops"),
            PythonJsonValue::Array(
                self.open_ops
                    .iter()
                    .map(SweepOpEntry::to_python_json)
                    .collect(),
            ),
        );
        object.insert(
            key("swept"),
            PythonJsonValue::Integer(self.swept.to_string()),
        );
        object.insert(
            key("skipped_fresh"),
            PythonJsonValue::Integer(self.skipped_fresh.to_string()),
        );
        object.insert(
            key("threshold_hours"),
            PythonJsonValue::Float(self.threshold_hours),
        );
        object.insert(
            key("closure_spine"),
            PythonJsonValue::scalar(&Self::closure_spine()),
        );
        PythonJsonValue::ExactObject(object)
    }

    /// `json.dumps(report.to_dict(), indent=2)` — the document Source emits.
    pub fn json_text(&self) -> String {
        self.to_python_json().render(Some(2))
    }

    /// Source's human output as plain lines (Rich markup stripped).
    ///
    /// A caller prints each line with a trailing newline; the empty strings
    /// are the blank lines Source's `console.print` calls produce.
    pub fn render_lines(&self) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        lines.push(String::new());
        lines.push(format!(
            "Ops sweep — threshold {}h: {} closed as abandoned, {} fresh (skipped)",
            python_float_repr(self.threshold_hours),
            self.swept,
            self.skipped_fresh
        ));
        lines.push(String::new());
        if !self.open_ops.is_empty() {
            lines.push(format!(
                "{:<26} {:<10} {:<20} {:>8} {:<16} {}",
                "Invocation ID", "Profile", "Started At", "Age (h)", "Action", "Closure commit"
            ));
            for entry in &self.open_ops {
                lines.push(format!(
                    "{:<26} {:<10} {:<20} {:>8} {:<16} {}",
                    entry.invocation_id,
                    entry.profile_id,
                    entry.started_at,
                    entry.age_text(),
                    entry.action_text(),
                    entry.commit_text()
                ));
            }
        }
        if self.swept > 0 {
            lines.extend(self.commit_summary_lines());
        }
        if self.skipped_fresh > 0 {
            lines.push(String::new());
            lines.push(
                "Fresh open Ops remain — close them with spec-kitty profile-invocation complete, \
                 or re-run with --threshold 0."
                    .to_owned(),
            );
        }
        lines.push(String::new());
        lines
    }

    /// Source's `_print_sweep_commit_summary`: where the closures went,
    /// separating deliberate skips from refusals that need the operator.
    pub fn commit_summary_lines(&self) -> Vec<String> {
        let spine = Self::closure_spine();
        let skipped: Vec<&SweepOpEntry> = self
            .open_ops
            .iter()
            .filter(|entry| {
                entry
                    .commit
                    .as_deref()
                    .is_some_and(|commit| commit.starts_with(SWEEP_COMMIT_SKIPPED_PREFIX))
            })
            .collect();
        let refused: Vec<&SweepOpEntry> = self
            .open_ops
            .iter()
            .filter(|entry| match entry.commit.as_deref() {
                Some(commit) => {
                    commit != "committed" && !commit.starts_with(SWEEP_COMMIT_SKIPPED_PREFIX)
                }
                None => false,
            })
            .collect();
        let mut lines = Vec::new();
        if !skipped.is_empty() {
            // Source: `sorted({commit.removeprefix(prefix) for ...})`, joined `"; "`.
            let reasons: BTreeSet<String> = skipped
                .iter()
                .filter_map(|entry| entry.commit.as_deref())
                .map(|commit| {
                    commit
                        .strip_prefix(SWEEP_COMMIT_SKIPPED_PREFIX)
                        .unwrap_or(commit)
                        .to_owned()
                })
                .collect();
            lines.push(String::new());
            lines.push(format!(
                "Closures kept on disk in the append-only spine {spine}, not committed: {}.",
                reasons.into_iter().collect::<Vec<_>>().join("; ")
            ));
        }
        if !refused.is_empty() {
            lines.push(String::new());
            lines.push(format!(
                "Closures recorded on the append-only spine {spine} but NOT committed (guard \
                 refusal, detached HEAD, or no git worktree). They are uncommitted; commit the \
                 spine yourself if you want it in history."
            ));
        }
        if skipped.is_empty() && refused.is_empty() {
            lines.push(String::new());
            lines.push(format!(
                "Closures recorded on the append-only spine {spine} and committed to the current \
                 branch. They are visible only on branches that carry that commit."
            ));
        }
        lines
    }
}

/// Why a whole sweep could not run. Per-Op failures are entries, not errors.
#[derive(Debug, thiserror::Error)]
pub enum OpsSweepError {
    /// The closure spine could not be read, so no Op can be classified.
    #[error("could not read the Op closure spine: {0}")]
    ClosureSpine(String),
    /// The read-only Op listing refused (unreadable store or record).
    #[error(transparent)]
    Audit(#[from] DoctorError),
}

impl OpsSweepError {
    /// The operator-facing refusal code the CLI reports.
    pub fn code(&self) -> &'static str {
        match self {
            Self::ClosureSpine(_) => "OP_CLOSURE_SPINE_UNREADABLE",
            Self::Audit(error) => error.code(),
        }
    }
}

/// The Op-store files that are never per-record Op records.
///
/// Two names come from their owners ([`crate::INDEX_PATH`] and the doctor's
/// lifecycle log); `propagation-errors.jsonl` has no native constant yet and is
/// Source's `doctor.ops._NON_OP_JSONL` literal. ONE list serves both the
/// listing and the sweep.
pub fn non_record_op_files() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = Vec::new();
    if let Some(name) = Path::new(INDEX_PATH)
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
    {
        names.push(name);
    }
    names.push(LIFECYCLE_LOG_RELATIVE_PATH[1]);
    names.push("propagation-errors.jsonl");
    names
}

/// Close every open Op older than `threshold_hours` as abandoned.
///
/// `now` is the caller's clock reading (Source passes `now_utc()`); it is a
/// parameter so the staleness decision is testable without faking a close time.
/// A `threshold_hours` of `0.0` closes every open Op, an unparseable
/// `started_at` is treated as stale with a `parse_warning`, and a `NaN`
/// threshold closes nothing (Source's `age > nan` is always false).
pub fn close_stale_ops(
    repo_root: &Path,
    threshold_hours: f64,
    now: UtcDateTime<UtcZone>,
) -> Result<SweepReport, OpsSweepError> {
    let mut report = SweepReport::new(threshold_hours);

    let closed = closed_invocation_ids(repo_root)
        .map_err(|error| OpsSweepError::ClosureSpine(error.to_string()))?;
    let closed_ids: BTreeSet<String> = closed
        .into_iter()
        .map(|invocation_id| invocation_id.as_str().to_owned())
        .collect();

    let ops_dir = repo_root.join(EVENTS_DIR);
    let non_record = non_record_op_files();
    let layout = OpStoreLayout::new(&ops_dir, OP_CLOSURES_FILENAME, &non_record, &closed_ids);
    let audit = audit_open_ops(&layout)?;
    if audit.open_ops.is_empty() {
        return Ok(report);
    }

    // The record root is the caller's project root, exactly like Source's
    // `ProfileInvocationExecutor(repo_root, …)`.
    let mut executor = ProfileInvocationExecutor::new(repo_root, repo_root);
    for path in &audit.open_ops {
        let invocation_id = match record_stem(path) {
            Ok(stem) => stem,
            Err(reason) => {
                report.open_ops.push(unnameable_record_entry(path, reason));
                continue;
            }
        };
        let (profile_id, started_at) = read_started_fields(path);
        let age = age_hours(&started_at, now);
        let parse_warning = age
            .is_none()
            .then(|| format!("unparseable started_at {started_at:?}; treated as stale"));
        let mut entry = SweepOpEntry {
            invocation_id: invocation_id.clone(),
            profile_id,
            started_at,
            age_hours: age,
            action_taken: SweepAction::None,
            parse_warning,
            error: None,
            commit: None,
        };

        if !is_stale(threshold_hours, age) {
            report.skipped_fresh += 1;
            report.open_ops.push(entry);
            continue;
        }

        let request = CompletionRequest {
            invocation_id: &invocation_id,
            outcome: OpOutcome::Abandoned,
            evidence_ref: None,
            artifact_refs: &[],
            commit_sha: None,
            closed_by: ClosedBy::DoctorSweep,
        };
        match executor.complete_invocation(&request) {
            Ok(outcome) => {
                entry.action_taken = SweepAction::ClosedAbandoned;
                report.swept += 1;
                entry.commit = outcome.op_commit.as_ref().map(commit_text);
            }
            Err(error) if is_already_closed(&error) => {
                entry.action_taken = SweepAction::AlreadyClosed;
            }
            Err(error) => entry.error = Some(error.to_string()),
        }
        report.open_ops.push(entry);
    }
    Ok(report)
}

/// Source's staleness rule, including its non-finite/normalising behaviour.
fn is_stale(threshold_hours: f64, age: Option<f64>) -> bool {
    threshold_hours == 0.0 || age.is_none() || age.is_some_and(|age| age > threshold_hours)
}

/// Whether a close refused because the Op is already closed.
///
/// The executor reports the spine-membership race as its own variant and the
/// per-record `completed` row as the writer's error; Source treats both as
/// `AlreadyClosedError`.
fn is_already_closed(error: &ExecutorError) -> bool {
    matches!(
        error,
        ExecutorError::AlreadyClosed(_)
            | ExecutorError::Invocation(crate::InvocationError::AlreadyClosed { .. })
    )
}

/// Source's `_read_started_fields`: `(profile_id, started_at)` from row 1.
///
/// Best-effort in Source's sense — a missing file, an empty file, an
/// unparseable first row or a non-object first row yields `("", "")`. A value
/// is rendered with Python `str()`, and a falsy value (`or ""`) becomes `""`.
/// A record that is not UTF-8 yields `("", "")` here; Source raises inside its
/// reader, and the record is a corrupt one either way.
fn read_started_fields(path: &Path) -> (String, String) {
    let Ok(bytes) = std::fs::read(path) else {
        return (String::new(), String::new());
    };
    let Ok(text) = String::from_utf8(bytes) else {
        return (String::new(), String::new());
    };
    let Some(first_line) = sk_kernel::python_text::splitlines(&text).first().copied() else {
        return (String::new(), String::new());
    };
    let Ok(row) = parse(first_line) else {
        return (String::new(), String::new());
    };
    let PythonJsonValue::ExactObject(object) = row else {
        return (String::new(), String::new());
    };
    (
        python_str_field(&object, "profile_id"),
        python_str_field(&object, "started_at"),
    )
}

/// `str(row.get(name) or "")`.
fn python_str_field(object: &PythonJsonDict, name: &str) -> String {
    match object.get(name) {
        None => String::new(),
        Some(value) if is_falsy(value) => String::new(),
        Some(value) => python_str(value),
    }
}

/// Python `str()` for a decoded JSON value.
fn python_str(value: &PythonJsonValue) -> String {
    match value {
        PythonJsonValue::String(text) => text.to_scalar_string().unwrap_or_else(|_| text.repr()),
        PythonJsonValue::Null => "None".to_owned(),
        PythonJsonValue::Bool(true) => "True".to_owned(),
        PythonJsonValue::Bool(false) => "False".to_owned(),
        PythonJsonValue::Integer(digits) | PythonJsonValue::Number(digits) => digits.clone(),
        PythonJsonValue::Float(number) => python_float_repr(*number),
        // `str(list)`/`str(dict)` are the same text as `repr(...)` for these
        // containers; the canonical CPython repr walker renders it.
        other => crate::python_json::python_repr_value(other),
    }
}

/// Source's `_age_hours`: hours from `started_at` to `now`, `None` when the
/// timestamp cannot be parsed. A naive timestamp is UTC (the Kernel parser's
/// documented extension, which Source applies explicitly).
fn age_hours(started_at: &str, now: UtcDateTime<UtcZone>) -> Option<f64> {
    let started = sk_kernel::clock::parse_iso(started_at).ok()?;
    let delta = now - started;
    // `total_seconds() / 3600.0`, at Python's microsecond resolution.
    Some(match delta.num_microseconds() {
        Some(micros) => micros as f64 / 3_600_000_000.0,
        None => delta.num_seconds() as f64 / 3600.0,
    })
}

/// Source's `entry.commit` text for a close this sweep performed.
fn commit_text(outcome: &OpCommitOutcome) -> String {
    if outcome.committed() {
        "committed".to_owned()
    } else if let Some(reason) = outcome.skipped() {
        format!("{SWEEP_COMMIT_SKIPPED_PREFIX}{reason}")
    } else {
        format!(
            "not_committed:{}",
            outcome.refusal().unwrap_or("unknown refusal")
        )
    }
}

/// The invocation id a record path names (`Path.stem`), or why the name cannot
/// be one.
fn record_stem(path: &Path) -> Result<String, &'static str> {
    let Some(name) = path.file_name().and_then(std::ffi::OsStr::to_str) else {
        return Err("the record name is not valid UTF-8");
    };
    name.strip_suffix(".jsonl")
        .map(str::to_owned)
        .ok_or("the record name has no .jsonl suffix")
}

fn unnameable_record_entry(path: &Path, reason: &str) -> SweepOpEntry {
    let stem = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    SweepOpEntry {
        invocation_id: stem,
        profile_id: String::new(),
        started_at: String::new(),
        age_hours: None,
        action_taken: SweepAction::None,
        parse_warning: None,
        error: Some(reason.to_owned()),
        commit: None,
    }
}

/// An ASCII literal as an exact code-point Python string key.
fn key(name: &str) -> sk_kernel::PythonString {
    sk_kernel::PythonString::from_scalars(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(invocation_id: &str, age: Option<f64>, action: SweepAction) -> SweepOpEntry {
        SweepOpEntry {
            invocation_id: invocation_id.to_owned(),
            profile_id: "implementer-ivan".to_owned(),
            started_at: "2026-11-01T00:00:00+00:00".to_owned(),
            age_hours: age,
            action_taken: action,
            parse_warning: None,
            error: None,
            commit: Some("committed".to_owned()),
        }
    }

    #[test]
    fn the_staleness_rule_matches_source_including_non_finite_thresholds() {
        assert!(is_stale(24.0, Some(25.0)));
        assert!(
            !is_stale(24.0, Some(24.0)),
            "strictly older than, not equal"
        );
        assert!(!is_stale(24.0, Some(0.5)));
        assert!(is_stale(0.0, Some(0.0)), "threshold 0 closes everything");
        assert!(is_stale(0.0, None), "an unparseable age is stale");
        assert!(is_stale(-1.0, Some(0.0)));
        assert!(
            !is_stale(f64::NAN, Some(1.0e9)),
            "Python's `age > nan` is always false"
        );
        assert!(
            !is_stale(f64::INFINITY, Some(1.0e9)),
            "an infinite threshold closes nothing"
        );
        assert!(is_stale(f64::NEG_INFINITY, Some(0.0)));
    }

    #[test]
    fn age_hours_handles_offsets_naive_timestamps_and_garbage() {
        let now = sk_kernel::clock::parse_iso("2026-11-02T00:00:00+00:00").unwrap();
        assert_eq!(age_hours("2026-11-01T00:00:00+00:00", now), Some(24.0));
        assert_eq!(
            age_hours("2026-11-01T01:00:00+02:00", now),
            Some(25.0),
            "offsets normalise to UTC"
        );
        assert_eq!(
            age_hours("2026-11-01T00:00:00", now),
            Some(24.0),
            "a naive timestamp is UTC"
        );
        assert_eq!(age_hours("not-a-timestamp", now), None);
        assert_eq!(age_hours("", now), None);
    }

    #[test]
    fn the_parse_warning_is_sources_text_with_python_repr() {
        let report_age = age_hours("garbage", sk_kernel::clock::now_utc());
        assert!(report_age.is_none());
        let started_at = "garbage";
        let warning = format!("unparseable started_at {started_at:?}; treated as stale");
        assert_eq!(
            warning,
            "unparseable started_at \"garbage\"; treated as stale"
        );
    }

    #[test]
    fn the_exit_rule_is_one_for_errors_or_remaining_fresh_ops() {
        let mut report = SweepReport::new(24.0);
        assert_eq!(report.exit_code(), 0);
        report
            .open_ops
            .push(entry("01A", Some(30.0), SweepAction::ClosedAbandoned));
        report.swept = 1;
        assert_eq!(
            report.exit_code(),
            0,
            "already_closed/closed is not a failure"
        );

        report
            .open_ops
            .push(entry("01B", Some(1.0), SweepAction::None));
        report.skipped_fresh = 1;
        assert_eq!(report.exit_code(), 1, "a remaining fresh Op exits 1");

        let mut errored = SweepReport::new(24.0);
        let mut bad = entry("01C", Some(99.0), SweepAction::None);
        bad.error = Some("boom".to_owned());
        errored.open_ops.push(bad);
        assert!(errored.has_errors());
        assert_eq!(errored.exit_code(), 1);
    }

    #[test]
    fn the_json_document_has_sources_key_order_and_conditional_members() {
        let mut report = SweepReport::new(24.0);
        report
            .open_ops
            .push(entry("01A", Some(3.25), SweepAction::ClosedAbandoned));
        let mut fresh = entry("01B", None, SweepAction::None);
        fresh.commit = None;
        fresh.parse_warning = Some("unparseable started_at \"\"; treated as stale".to_owned());
        fresh.error = Some("boom".to_owned());
        report.open_ops.push(fresh);
        report.swept = 1;
        report.skipped_fresh = 0;

        let text = report.json_text();
        assert!(
            text.starts_with(
                "{\n  \"open_ops\": [\n    {\n      \"invocation_id\": \"01A\",\n      \"profile_id\": \"implementer-ivan\","
            ),
            "{text}"
        );
        assert!(text.contains("\"age_hours\": 3.25"), "{text}");
        assert!(
            text.contains("\"action_taken\": \"closed_abandoned\""),
            "{text}"
        );
        assert!(text.contains("\"commit\": \"committed\""), "{text}");
        assert!(
            text.contains(
                "\"parse_warning\": \"unparseable started_at \\\"\\\"; treated as stale\""
            ),
            "{text}"
        );
        assert!(text.contains("\"error\": \"boom\""), "{text}");
        assert!(text.ends_with(
            "  \"swept\": 1,\n  \"skipped_fresh\": 0,\n  \"threshold_hours\": 24.0,\n  \"closure_spine\": \"kitty-ops/op-closures.jsonl\"\n}"
        ), "{text}");
        // The `commit` member is absent on the fresh entry.
        let fresh_object = &report.open_ops[1];
        assert_eq!(fresh_object.commit, None);
    }

    #[test]
    fn a_nan_threshold_renders_the_python_nan_token() {
        let report = SweepReport::new(f64::NAN);
        assert!(report.json_text().contains("\"threshold_hours\": NaN"));
        assert!(
            report
                .render_lines()
                .iter()
                .any(|line| line.contains("threshold nanh")),
            "{:?}",
            report.render_lines()
        );
    }

    #[test]
    fn the_human_lines_carry_the_sweep_counts_table_and_remedies() {
        let mut report = SweepReport::new(24.0);
        let mut closed = entry("01A", Some(30.0), SweepAction::ClosedAbandoned);
        closed.commit = Some("skipped:the record path is ignored".to_owned());
        report.open_ops.push(closed);
        let mut fresh = entry("01B", Some(0.5), SweepAction::None);
        fresh.commit = None;
        report.open_ops.push(fresh);
        report.swept = 1;
        report.skipped_fresh = 1;

        let lines = report.render_lines();
        assert!(
            lines.iter().any(|line| line
                == "Ops sweep — threshold 24.0h: 1 closed as abandoned, 1 fresh (skipped)")
        );
        assert!(lines.iter().any(|line| line.contains("Started At")));
        assert!(
            lines
                .iter()
                .any(|line| line.contains("01A") && line.contains("30.0"))
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("01B") && line.contains("0.5"))
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("skipped:the record path is ignored"))
        );
        assert!(lines.iter().any(|line| line.contains(
            "Fresh open Ops remain — close them with spec-kitty profile-invocation complete"
        )));
    }

    #[test]
    fn the_commit_summary_separates_skips_from_refusals() {
        let mut skipped = SweepReport::new(24.0);
        let mut entry_a = entry("01A", Some(30.0), SweepAction::ClosedAbandoned);
        entry_a.commit = Some("skipped:zeta".to_owned());
        let mut entry_b = entry("01B", Some(30.0), SweepAction::ClosedAbandoned);
        entry_b.commit = Some("skipped:alpha".to_owned());
        skipped.open_ops.push(entry_a);
        skipped.open_ops.push(entry_b);
        skipped.swept = 2;
        let lines = skipped.commit_summary_lines();
        assert!(
            lines
                .iter()
                .any(|line| line.contains("not committed: alpha; zeta.")),
            "{lines:?}"
        );

        let mut refused = SweepReport::new(24.0);
        let mut entry_c = entry("01C", Some(30.0), SweepAction::ClosedAbandoned);
        entry_c.commit = Some("not_committed:protected branch 'main'".to_owned());
        refused.open_ops.push(entry_c);
        refused.swept = 1;
        let lines = refused.commit_summary_lines();
        assert!(
            lines.iter().any(|line| line.contains("but NOT committed")),
            "{lines:?}"
        );

        let mut clean = SweepReport::new(24.0);
        clean
            .open_ops
            .push(entry("01D", Some(30.0), SweepAction::ClosedAbandoned));
        clean.swept = 1;
        let lines = clean.commit_summary_lines();
        assert!(
            lines
                .iter()
                .any(|line| line.contains("and committed to the current branch")),
            "{lines:?}"
        );
    }

    #[test]
    fn started_fields_follow_python_str_and_falsy_coercion() {
        let temp = tempfile::tempdir().expect("tempdir");
        let record = temp.path().join("01A.jsonl");
        std::fs::write(
            &record,
            "{\"event\": \"started\", \"profile_id\": \"\", \"started_at\": 0}\n",
        )
        .unwrap();
        assert_eq!(
            read_started_fields(&record),
            (String::new(), String::new()),
            "empty string and 0 are falsy (`or \"\"`)"
        );

        std::fs::write(
            &record,
            "{\"event\": \"started\", \"profile_id\": 7, \"started_at\": \"2026-11-01T00:00:00+00:00\"}\n",
        )
        .unwrap();
        assert_eq!(
            read_started_fields(&record),
            ("7".to_owned(), "2026-11-01T00:00:00+00:00".to_owned()),
            "a non-string scalar renders with str()"
        );

        std::fs::write(&record, "not json\n").unwrap();
        assert_eq!(read_started_fields(&record), (String::new(), String::new()));

        std::fs::write(&record, "[]\n").unwrap();
        assert_eq!(
            read_started_fields(&record),
            (String::new(), String::new()),
            "a non-object first row is (\"\", \"\")"
        );

        std::fs::write(&record, "").unwrap();
        assert_eq!(
            read_started_fields(&record),
            (String::new(), String::new()),
            "an empty record has no first row"
        );
        assert_eq!(
            read_started_fields(&temp.path().join("absent.jsonl")),
            (String::new(), String::new())
        );
    }

    #[test]
    fn non_record_names_come_from_the_owners() {
        let names = non_record_op_files();
        assert!(names.contains(&"ops-index.jsonl"));
        assert!(names.contains(&"lifecycle.jsonl"));
        assert!(names.contains(&"propagation-errors.jsonl"));
        assert_eq!(SweepReport::closure_spine(), "kitty-ops/op-closures.jsonl");
    }
}
