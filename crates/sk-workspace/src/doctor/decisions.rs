//! `doctor decisions` — read-only reconciliation between a Mission's
//! `decisions/index.json` and its authoritative `status.events.jsonl`.
//!
//! # Source
//!
//! `specify_cli.cli.commands._decisions_doctor` (the `run_decisions_reconciliation`
//! diagnose half) over `specify_cli.decisions.index_fold` and
//! `specify_cli.decisions.store.load_index`.
//!
//! The diagnosis reports, for one Mission:
//!
//! * `missing_from_index` — decision ids present in the event log but absent
//!   from the index;
//! * `orphaned_in_index` — index entries with no backing event;
//! * `malformed_folds` — decision ids whose event group cannot be folded at all
//!   (an `open → open` conflict, an unfoldable terminal combination, an
//!   unrecognized member, a malformed envelope);
//! * `status_mismatch` — ids present in both whose folded status disagrees with
//!   the recorded one (a stale entry the id-set comparison cannot see).
//!
//! The read-only path always exits 0 in Source ("report only"); a
//! non-clean report is a finding, not a refusal.
//!
//! # The two-surface fork facts
//!
//! Source's `doctor decisions` also runs `specify_cli.decisions.fork` — a
//! two-surface (PRIMARY vs COORDINATION, worktree-or-ref) comparison of both
//! decision streams plus the ledger home — which contributes
//! `DECISION_LOG_FORKED` / `DECISION_LEDGER_ONLY_ON_COORDINATION` findings and
//! widens the orphan rule to the union of every decision id on either surface.
//! That owner is [`crate::decision_fork`]; [`audit_decisions_with_fork`] consumes
//! its report, and [`DecisionsReport`] renders the Source payload shape with the
//! real `forked` / `streams` / `ledger` / `reconcile_steps` keys. A caller that
//! passes no fork report (Source's `repo_root=None` shape) gets the pre-fork
//! single-surface payload and an explicit `unevaluated_findings` disclosure —
//! never a silent "clean" that hides an unrun comparison.
//!
//! # Read-only by construction
//!
//! Nothing here writes, locks, mkdirs, or repairs. `--repair` (Source's mutating
//! arm) is refused by the adapter before this module is reached.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::{Value, json};
use sk_kernel::PythonString;
use sk_kernel::python_json::{PythonJsonValue, is_falsy, parse};
use sk_kernel::python_text::{is_whitespace, splitlines};

use crate::decision_fold::{
    DECISION_OPENED_EVENT, DECISION_RESOLVED_EVENT, fold_events, fold_status,
};
use crate::decision_fork::{
    DecisionsForkReport, LedgerHomeFinding, StreamForkFinding, SurfaceLog,
    ledger_is_coordination_only,
};
use crate::decision_store::{DecisionIndexStore, load_index};
use crate::doctor::DoctorError;

/// The Mission event-log filename the decision rows are appended to.
pub const DECISION_EVENTS_FILE_NAME: &str = "status.events.jsonl";

/// One stale index entry: the recorded status disagrees with the folded log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecisionStatusMismatch {
    /// The decision id both surfaces know.
    pub decision_id: String,
    /// The status recorded in `decisions/index.json`.
    pub index_status: &'static str,
    /// The status the event log folds to.
    pub folded_status: &'static str,
}

impl DecisionStatusMismatch {
    fn to_json(&self) -> Value {
        json!({
            "decision_id": self.decision_id,
            "index_status": self.index_status,
            "folded_status": self.folded_status,
        })
    }
}

/// One Mission's read-only decisions reconciliation report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecisionsAudit {
    /// The Mission handle that was resolved (the directory name).
    pub mission_slug: String,
    /// Decision ids the event log carries, in code-point order.
    pub log_decision_ids: Vec<String>,
    /// Decision ids the index carries, in code-point order.
    pub index_decision_ids: Vec<String>,
    /// Log ids absent from the index.
    pub missing_from_index: Vec<String>,
    /// Index ids with no backing event.
    pub orphaned_in_index: Vec<String>,
    /// Ids whose event group cannot be folded.
    pub malformed_folds: Vec<String>,
    /// Ids whose recorded status is stale.
    pub status_mismatch: Vec<DecisionStatusMismatch>,
}

impl DecisionsAudit {
    /// The two Source findings this slice cannot evaluate, because its owner
    /// (`specify_cli.decisions.fork`) is not ported.
    pub const UNEVALUATED_FINDINGS: [&'static str; 2] = [
        "DECISION_LOG_FORKED",
        "DECISION_LEDGER_ONLY_ON_COORDINATION",
    ];

    /// Whether the single-surface comparison found nothing to report.
    ///
    /// This is Source's `clean` for the no-fork-report shape; it does not claim
    /// the section comparison ran (see [`Self::UNEVALUATED_FINDINGS`]).
    pub fn clean(&self) -> bool {
        self.missing_from_index.is_empty()
            && self.orphaned_in_index.is_empty()
            && self.malformed_folds.is_empty()
            && self.status_mismatch.is_empty()
    }

    /// The one Source remedy sentence for `malformed_folds`, or `None`.
    ///
    /// The text is Source's `_malformed_fold_remedy_text`, including the
    /// inspection pointer, so the JSON and human surfaces cannot drift.
    pub fn malformed_fold_remedy(&self) -> Option<String> {
        if self.malformed_folds.is_empty() {
            return None;
        }
        Some(format!(
            "left in place: the pre-repair index entry is kept unchanged where one existed, \
             otherwise the decision_id stays out of the index (never invented). Inspect with \
             'spec-kitty agent decision list --mission {mission}' and the raw event log \
             (status.events.jsonl).",
            mission = self.mission_slug
        ))
    }

    /// The Source payload fields every shape carries (everything except the
    /// fork-specific keys and the `clean`/findings verdict, which depend on the
    /// fork facts).
    fn base_json(&self) -> Value {
        json!({
            "mission_slug": self.mission_slug,
            "clean": self.clean(),
            "log_decision_ids": self.log_decision_ids,
            "index_decision_ids": self.index_decision_ids,
            "missing_from_index": self.missing_from_index,
            "orphaned_in_index": self.orphaned_in_index,
            "repaired": false,
            "lossy_attribution": Vec::<String>::new(),
            "malformed_folds": self.malformed_folds,
            "status_mismatch": self
                .status_mismatch
                .iter()
                .map(DecisionStatusMismatch::to_json)
                .collect::<Vec<_>>(),
            "remedy": self.malformed_fold_remedy(),
            "findings": Vec::<String>::new(),
            "ledger_copied": false,
        })
    }

    /// The Source-shaped JSON payload for the `repo_root=None` call shape, plus
    /// the additive `unevaluated_findings` disclosure.
    ///
    /// The fork comparison genuinely did not run in this shape, so the two
    /// Source finding codes it would have produced are named as unevaluated —
    /// [`DecisionsReport`] is the shape that carries the real facts instead.
    pub fn to_json(&self) -> Value {
        let mut payload = self.base_json();
        payload["unevaluated_findings"] = json!(Self::UNEVALUATED_FINDINGS);
        payload
    }
}

/// The event log grouped by decision id, in on-disk append order.
struct GroupedEvents {
    order: Vec<PythonString>,
    by_id: BTreeMap<PythonString, Vec<PythonJsonValue>>,
}

/// Diagnose one Mission's index/log divergence (Source's `repo_root=None`
/// shape: no two-surface fork comparison is computed).
///
/// `events_dir` is the Mission's `STATUS_STATE` directory (the coordination
/// surface on a coordination-routing topology) and `ledger_dir` its
/// PRIMARY-partition Mission directory — Source resolves them separately
/// (#4966 AC-D2). `mission_slug` is only used for the report and its remedy.
///
/// A missing event log is an empty log and a missing index is the benign empty
/// index (Source's rule); a *present* but unreadable one is a typed refusal, so
/// a corrupt store can never read as a clean report.
pub fn audit_decisions(
    events_dir: &Path,
    ledger_dir: &Path,
    mission_slug: &str,
) -> Result<DecisionsAudit, DoctorError> {
    audit_decisions_with_fork(events_dir, ledger_dir, mission_slug, None)
}

/// Diagnose one Mission's index/log divergence, optionally against the
/// two-surface fork facts.
///
/// With a [`DecisionsForkReport`] the orphan rule is Source's: an index entry is
/// orphaned only when it is absent from the **union** of every decision id on
/// either surface, any stream. Without one the single-surface rule applies and
/// [`DecisionsReport`] keeps the `unevaluated_findings` disclosure honest.
pub fn audit_decisions_with_fork(
    events_dir: &Path,
    ledger_dir: &Path,
    mission_slug: &str,
    fork: Option<&DecisionsForkReport>,
) -> Result<DecisionsAudit, DoctorError> {
    let events_path = events_dir.join(DECISION_EVENTS_FILE_NAME);
    let grouped = read_decision_events(&events_path)?;
    let index = load_decision_index(ledger_dir)?;

    let mut log_ids: BTreeSet<PythonString> = grouped.by_id.keys().cloned().collect();
    let mut index_ids: BTreeSet<PythonString> = index
        .entries()
        .iter()
        .map(|entry| entry.decision_id().clone())
        .collect();

    let mut malformed_folds: Vec<String> = Vec::new();
    let mut status_mismatch: Vec<DecisionStatusMismatch> = Vec::new();
    for decision_id in &grouped.order {
        let Some(events) = grouped.by_id.get(decision_id) else {
            continue;
        };
        match fold_events(events) {
            Ok(folded) => {
                // Source `index_by_id` is a dict comprehension, so the LAST
                // duplicate entry wins — the store's `status_of` view.
                if let Some(recorded) = index.status_of(decision_id)
                    && recorded != folded.status()
                {
                    status_mismatch.push(DecisionStatusMismatch {
                        decision_id: render_id(decision_id, &events_path)?,
                        index_status: recorded.as_str(),
                        folded_status: folded.status().as_str(),
                    });
                }
            }
            Err(_) => malformed_folds.push(render_id(decision_id, &events_path)?),
        }
    }

    // `sorted(...)` over the id sets: Python's code-point order.
    let log_decision_ids = render_ids(&log_ids, &events_path)?;
    let index_decision_ids = render_ids(&index_ids, &events_path)?;
    let missing = log_ids
        .iter()
        .filter(|id| !index_ids.contains(*id))
        .cloned()
        .collect::<BTreeSet<_>>();
    // Source's orphan universe: every decision id the log holds, plus — when the
    // fork report ran — every id either surface carries on either stream.
    let mut universe = log_ids.clone();
    if let Some(fork) = fork {
        for finding in &fork.streams {
            universe.extend(finding.decisions_only_on_primary.iter().cloned());
            universe.extend(finding.decisions_only_on_coordination.iter().cloned());
            universe.extend(finding.decisions_on_both.iter().cloned());
        }
    }
    let orphaned = index_ids
        .iter()
        .filter(|id| !universe.contains(*id))
        .cloned()
        .collect::<BTreeSet<_>>();

    // Sort the reported folds/mismatches by id like Source's `sorted(...)`.
    malformed_folds.sort();
    status_mismatch.sort_by(|left, right| left.decision_id.cmp(&right.decision_id));

    log_ids.clear();
    index_ids.clear();

    Ok(DecisionsAudit {
        mission_slug: mission_slug.to_owned(),
        log_decision_ids,
        index_decision_ids,
        missing_from_index: render_ids(&missing, &events_path)?,
        orphaned_in_index: render_ids(&orphaned, &events_path)?,
        malformed_folds,
        status_mismatch,
    })
}

/// Read the decision event rows, grouped by `decision_point_id`.
///
/// Source reads the file to EOF, splits Python-style, parses each non-blank
/// line as JSON, skips a non-decision `event_type`, and skips a falsy
/// `decision_point_id`. Anything it would *crash* on (invalid UTF-8, malformed
/// JSON, a non-object envelope, a truthy non-object payload, a non-string
/// decision id) is a typed refusal here rather than a process abort.
fn read_decision_events(path: &Path) -> Result<GroupedEvents, DoctorError> {
    let bytes = match sk_kernel::read_source_bytes(path) {
        Ok(bytes) => bytes,
        Err(sk_kernel::KernelError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            return Ok(GroupedEvents {
                order: Vec::new(),
                by_id: BTreeMap::new(),
            });
        }
        Err(error) => {
            return Err(DoctorError::DecisionEventsUnreadable {
                path: path.to_path_buf(),
                detail: error.to_string(),
            });
        }
    };
    let text =
        std::str::from_utf8(&bytes).map_err(|error| DoctorError::DecisionEventsUnreadable {
            path: path.to_path_buf(),
            detail: format!("{DECISION_EVENTS_FILE_NAME} is not valid UTF-8: {error}"),
        })?;

    let mut order: Vec<PythonString> = Vec::new();
    let mut by_id: BTreeMap<PythonString, Vec<PythonJsonValue>> = BTreeMap::new();
    for line in splitlines(text) {
        let line = trim_python_whitespace(line);
        if line.is_empty() {
            continue;
        }
        let event = parse(line).map_err(|error| DoctorError::DecisionEventsUnreadable {
            path: path.to_path_buf(),
            detail: format!("a decision event line is not valid JSON: {error}"),
        })?;
        let Some(object) = exact_object(&event) else {
            return Err(DoctorError::DecisionEventsUnreadable {
                path: path.to_path_buf(),
                detail: "a decision event line is not a JSON object".to_owned(),
            });
        };
        let is_decision = matches!(
            object.get("event_type"),
            Some(PythonJsonValue::String(event_type))
                if *event_type == python_string(DECISION_OPENED_EVENT)
                    || *event_type == python_string(DECISION_RESOLVED_EVENT)
        );
        if !is_decision {
            continue;
        }
        // Source: `payload = event.get("payload") or {}` — a falsy payload is
        // the empty object; a truthy non-object raises in Source.
        let payload = match object.get("payload") {
            Some(value) if !is_falsy(value) => {
                exact_object(value).ok_or_else(|| DoctorError::DecisionEventsUnreadable {
                    path: path.to_path_buf(),
                    detail: "a decision event payload is not a JSON object".to_owned(),
                })?
            }
            _ => {
                continue;
            }
        };
        let Some(decision_id) = payload.get("decision_point_id") else {
            continue;
        };
        if is_falsy(decision_id) {
            continue;
        }
        let PythonJsonValue::String(decision_id) = decision_id else {
            return Err(DoctorError::DecisionEventsUnreadable {
                path: path.to_path_buf(),
                detail: "a decision_point_id is not a string".to_owned(),
            });
        };
        if !by_id.contains_key(decision_id) {
            order.push(decision_id.clone());
        }
        by_id.entry(decision_id.clone()).or_default().push(event);
    }
    Ok(GroupedEvents { order, by_id })
}

/// Load the Mission's decision index, mapping both failure classes to one
/// typed refusal (Source re-raises the probe `OSError` and wraps a decode
/// failure; neither is a clean report).
fn load_decision_index(ledger_dir: &Path) -> Result<DecisionIndexStore, DoctorError> {
    load_index(ledger_dir).map_err(|error| DoctorError::DecisionIndexUnreadable {
        path: crate::decision_store::index_path(ledger_dir),
        detail: error.to_string(),
    })
}

/// Render one decision id as scalar text, refusing a lone surrogate rather
/// than inventing an escaped identity.
fn render_id(decision_id: &PythonString, path: &Path) -> Result<String, DoctorError> {
    decision_id
        .to_scalar_string()
        .map_err(|code| DoctorError::DecisionEventsUnreadable {
            path: path.to_path_buf(),
            detail: format!(
                "a decision id carries the unpaired surrogate U+{code:04X}, which has no scalar form"
            ),
        })
}

/// Render a set of ids in Python code-point order.
fn render_ids(ids: &BTreeSet<PythonString>, path: &Path) -> Result<Vec<String>, DoctorError> {
    ids.iter()
        .map(|decision_id| render_id(decision_id, path))
        .collect()
}

/// The canonical reader's object view (the only object representation a
/// decoded document carries).
fn exact_object(value: &PythonJsonValue) -> Option<&sk_kernel::python_json::PythonJsonDict> {
    match value {
        PythonJsonValue::ExactObject(object) => Some(object),
        _ => None,
    }
}

/// An ASCII literal as an exact code-point Python string.
fn python_string(text: &str) -> PythonString {
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

/// The folded status of one id, for callers that only need the comparison.
pub fn folded_status(
    events: &[PythonJsonValue],
) -> Result<crate::decision_model::DecisionStatus, crate::decision_fold::FoldError> {
    fold_status(events)
}

/// `doctor decisions`' full report: the log/index audit plus the fork facts.
///
/// [`DecisionsAudit`] stays the single-surface record (and keeps its own
/// pre-fork payload), so a caller can consume either shape without one silently
/// standing in for the other. Source's `clean` is `base && not findings`.
pub struct DecisionsReport<'a> {
    /// The log/index reconciliation.
    pub audit: DecisionsAudit,
    /// The two-surface fork report, when it was computed.
    pub fork: Option<&'a DecisionsForkReport>,
}

impl<'a> DecisionsReport<'a> {
    /// Pair an audit with its fork report.
    pub fn new(audit: DecisionsAudit, fork: Option<&'a DecisionsForkReport>) -> Self {
        Self { audit, fork }
    }

    /// Source's `findings`: the new WP17 codes, computed from real facts.
    pub fn findings(&self) -> Vec<&'static str> {
        let Some(fork) = self.fork else {
            return Vec::new();
        };
        let mut findings = Vec::new();
        if fork.forked {
            findings.push("DECISION_LOG_FORKED");
        }
        if ledger_is_coordination_only(&fork.ledger) {
            findings.push("DECISION_LEDGER_ONLY_ON_COORDINATION");
        }
        findings
    }

    /// Whether the Mission is clean: the audit's verdict plus the fork findings
    /// (Source `DecisionsReconciliationReport.clean`).
    pub fn clean(&self) -> bool {
        self.audit.clean() && self.findings().is_empty()
    }

    /// The full Source JSON payload.
    ///
    /// With fork facts the payload carries `forked`, `streams`, `ledger` and
    /// `reconcile_steps` (Source's `repo_root` shape) and no
    /// `unevaluated_findings`; without them it carries the disclosure instead.
    pub fn to_json(&self) -> Result<Value, DoctorError> {
        let mut payload = self.audit.base_json();
        payload["clean"] = json!(self.clean());
        payload["findings"] = json!(self.findings());
        match self.fork {
            None => {
                payload["unevaluated_findings"] = json!(DecisionsAudit::UNEVALUATED_FINDINGS);
            }
            Some(fork) => {
                payload["forked"] = json!(fork.forked);
                payload["streams"] = Value::Array(
                    fork.streams
                        .iter()
                        .map(stream_finding_json)
                        .collect::<Result<Vec<_>, DoctorError>>()?,
                );
                payload["ledger"] = ledger_finding_json(&fork.ledger)?;
                payload["reconcile_steps"] = json!(fork.reconcile_steps);
            }
        }
        Ok(payload)
    }
}

/// One stream finding's Source JSON (`_stream_finding_to_json`).
fn stream_finding_json(finding: &StreamForkFinding) -> Result<Value, DoctorError> {
    Ok(json!({
        "stream": finding.stream,
        "state": finding.state.as_str(),
        "primary": surface_log_json(finding.primary.as_ref())?,
        "coordination": surface_log_json(finding.coordination.as_ref())?,
        "decisions_only_on_primary": render_fork_ids(&finding.decisions_only_on_primary)?,
        "decisions_only_on_coordination":
            render_fork_ids(&finding.decisions_only_on_coordination)?,
        "decisions_on_both": render_fork_ids(&finding.decisions_on_both)?,
    }))
}

/// One surface read's Source JSON (`_surface_log_to_json`): source, ref, path and
/// the event COUNT (not the ids).
fn surface_log_json(log: Option<&SurfaceLog>) -> Result<Value, DoctorError> {
    match log {
        None => Ok(Value::Null),
        Some(log) => Ok(json!({
            "source": log.source.as_str(),
            "ref": log.reference,
            "path": log.path,
            "event_count": log.event_ids.len(),
        })),
    }
}

/// The ledger home's Source JSON (`_ledger_finding_to_json`).
fn ledger_finding_json(ledger: &LedgerHomeFinding) -> Result<Value, DoctorError> {
    Ok(json!({
        "state": ledger.state.as_str(),
        "entries_only_on_coordination":
            render_fork_ids(&ledger.entries_only_on_coordination)?,
        "dm_files_only_on_coordination":
            render_fork_ids(&ledger.dm_files_only_on_coordination)?,
    }))
}

/// Render fork ids as scalar text in code-point order, refusing a lone surrogate
/// rather than inventing an escaped identity.
fn render_fork_ids(ids: &[PythonString]) -> Result<Vec<String>, DoctorError> {
    ids.iter()
        .map(|id| {
            id.to_scalar_string().map_err(|code| DoctorError::ForkIdentity {
                detail: format!(
                    "a decision id carries the unpaired surrogate U+{code:04X}, which has no scalar form"
                ),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision_store::DecisionIndexStore;

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("fixture parent");
        }
        std::fs::write(path, contents).expect("fixture write");
    }

    fn opened_line(id: &str, at: &str) -> String {
        format!(
            r#"{{"event_id":"e-{id}-1","event_type":"DecisionPointOpened","payload":{{"decision_point_id":"{id}","origin_flow":"specify","step_id":"s","input_key":"k","question":"q","options":[],"state_entered_at":"{at}","actor_id":"claude","mission_id":"m","mission_slug":"001-x"}}}}"#
        )
    }

    fn resolved_line(id: &str, outcome: &str, at: &str) -> String {
        format!(
            r#"{{"event_id":"e-{id}-2","event_type":"DecisionPointResolved","payload":{{"decision_point_id":"{id}","terminal_outcome":"{outcome}","state_entered_at":"{at}","resolved_by":"operator"}}}}"#
        )
    }

    fn index_doc(entries: &[&str]) -> String {
        format!(
            r#"{{"version":1,"mission_id":"01MISSION","entries":[{}]}}"#,
            entries.join(",")
        )
    }

    fn index_entry(id: &str, status: &str) -> String {
        format!(
            r#"{{"decision_id":"{id}","origin_flow":"specify","step_id":"s","slot_key":null,"input_key":"k","question":"q","options":[],"status":"{status}","final_answer":null,"rationale":null,"other_answer":false,"summary_json":null,"created_at":"2026-01-01T00:00:00Z","resolved_at":null,"resolved_by":null,"opened_by":"claude","mission_id":"m","mission_slug":"001-x"}}"#
        )
    }

    fn fixture(events: &[String], index: &str) -> tempfile::TempDir {
        let root = tempfile::tempdir().expect("tempdir");
        let events_dir = root.path().join("kitty-specs/001-x");
        if !events.is_empty() {
            write(
                &events_dir.join(DECISION_EVENTS_FILE_NAME),
                &format!("{}\n", events.join("\n")),
            );
        }
        write(&events_dir.join("decisions/index.json"), index);
        fs_create_dir(&events_dir);
        root
    }

    fn fs_create_dir(path: &Path) {
        std::fs::create_dir_all(path).expect("fixture dir");
    }

    fn audit(root: &tempfile::TempDir) -> DecisionsAudit {
        let dir = root.path().join("kitty-specs/001-x");
        audit_decisions(&dir, &dir, "001-x").expect("audit")
    }

    #[test]
    fn a_matching_index_and_log_are_clean() {
        let root = fixture(
            &[opened_line("d1", "2026-01-01T00:00:00+00:00")],
            &index_doc(&[&index_entry("d1", "open")]),
        );
        let report = audit(&root);
        assert!(report.clean(), "{report:?}");
        assert_eq!(report.log_decision_ids, vec!["d1"]);
        assert_eq!(report.index_decision_ids, vec!["d1"]);
        assert_eq!(report.to_json()["repaired"], json!(false));
    }

    #[test]
    fn a_log_decision_missing_from_the_index_is_reported() {
        let root = fixture(
            &[opened_line("d2", "2026-01-01T00:00:00+00:00")],
            &index_doc(&[]),
        );
        let report = audit(&root);
        assert!(!report.clean());
        assert_eq!(report.missing_from_index, vec!["d2"]);
        assert!(report.orphaned_in_index.is_empty());
    }

    #[test]
    fn an_index_entry_without_a_backing_event_is_orphaned() {
        let root = fixture(&[], &index_doc(&[&index_entry("d9", "open")]));
        let report = audit(&root);
        assert_eq!(report.orphaned_in_index, vec!["d9"]);
        assert!(report.missing_from_index.is_empty());
    }

    #[test]
    fn a_stale_status_is_a_mismatch_even_when_the_id_sets_agree() {
        let root = fixture(
            &[
                opened_line("d1", "2026-01-01T00:00:00+00:00"),
                resolved_line("d1", "resolved", "2026-01-02T00:00:00+00:00"),
            ],
            &index_doc(&[&index_entry("d1", "deferred")]),
        );
        let report = audit(&root);
        assert!(!report.clean());
        assert!(report.missing_from_index.is_empty());
        assert_eq!(
            report.status_mismatch,
            vec![DecisionStatusMismatch {
                decision_id: "d1".to_owned(),
                index_status: "deferred",
                folded_status: "resolved",
            }]
        );
        assert!(report.to_json()["status_mismatch"][0]["folded_status"] == json!("resolved"));
    }

    #[test]
    fn an_unfoldable_group_is_malformed_and_never_clean() {
        let root = fixture(
            &[
                opened_line("d1", "2026-01-01T00:00:00+00:00"),
                resolved_line("d1", "resolved", "2026-01-02T00:00:00+00:00"),
                resolved_line("d1", "canceled", "2026-01-03T00:00:00+00:00"),
            ],
            &index_doc(&[&index_entry("d1", "resolved")]),
        );
        let report = audit(&root);
        assert_eq!(report.malformed_folds, vec!["d1"]);
        assert!(!report.clean());
        let payload = report.to_json();
        assert!(payload["remedy"].is_string());
        assert_eq!(
            payload["unevaluated_findings"],
            json!(DecisionsAudit::UNEVALUATED_FINDINGS)
        );
    }

    #[test]
    fn a_missing_log_and_index_are_the_benign_empty_case() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("kitty-specs/001-x");
        fs_create_dir(&dir);
        let report = decisions_audit_empty(&dir);
        assert!(report.clean());
        assert!(report.log_decision_ids.is_empty());
    }

    fn decisions_audit_empty(dir: &Path) -> DecisionsAudit {
        audit_decisions(dir, dir, "001-x").expect("audit")
    }

    #[test]
    fn a_corrupt_index_is_a_typed_refusal_not_a_clean_report() {
        let root = fixture(&[], "{not json");
        let dir = root.path().join("kitty-specs/001-x");
        let error = audit_decisions(&dir, &dir, "001-x").expect_err("refusal");
        assert_eq!(error.code(), "DECISION_INDEX_UNREADABLE");
    }

    #[test]
    fn a_corrupt_event_log_is_a_typed_refusal() {
        let root = fixture(&["{not json".to_owned()], &index_doc(&[]));
        let dir = root.path().join("kitty-specs/001-x");
        let error = audit_decisions(&dir, &dir, "001-x").expect_err("refusal");
        assert_eq!(error.code(), "DECISION_EVENTS_UNREADABLE");
        assert!(error.to_string().contains(DECISION_EVENTS_FILE_NAME));
    }

    #[test]
    fn non_decision_rows_and_blank_lines_are_ignored() {
        let root = fixture(
            &[
                String::new(),
                r#"{"event_type":"WPStatusChanged","payload":{"decision_point_id":"nope"}}"#
                    .to_owned(),
                r#"{"event_type":"DecisionPointOpened","payload":{}}"#.to_owned(),
                opened_line("d1", "2026-01-01T00:00:00+00:00"),
            ],
            &index_doc(&[&index_entry("d1", "open")]),
        );
        let report = audit(&root);
        assert!(report.clean(), "{report:?}");
        assert_eq!(report.log_decision_ids, vec!["d1"]);
    }

    #[test]
    fn the_last_duplicate_index_entry_decides_the_status() {
        let root = fixture(
            &[opened_line("d1", "2026-01-01T00:00:00+00:00")],
            &index_doc(&[&index_entry("d1", "resolved"), &index_entry("d1", "open")]),
        );
        let report = audit(&root);
        assert!(report.clean(), "{report:?}");
    }

    #[test]
    fn the_remedy_names_the_mission_and_the_log() {
        let report = DecisionsAudit {
            mission_slug: "001-x".to_owned(),
            log_decision_ids: Vec::new(),
            index_decision_ids: Vec::new(),
            missing_from_index: Vec::new(),
            orphaned_in_index: Vec::new(),
            malformed_folds: vec!["d1".to_owned()],
            status_mismatch: Vec::new(),
        };
        let remedy = report.malformed_fold_remedy().expect("remedy");
        assert!(remedy.contains("--mission 001-x"));
        assert!(remedy.contains("status.events.jsonl"));
        assert!(remedy.starts_with("left in place:"));
    }

    #[test]
    fn the_index_store_view_is_reused_not_rebuilt() {
        // Compile-time anchor: the audit reads the canonical store type.
        fn takes_store(_: &DecisionIndexStore) {}
        let root = fixture(&[], &index_doc(&[]));
        let dir = root.path().join("kitty-specs/001-x");
        let store = load_decision_index(&dir).expect("store");
        takes_store(&store);
        assert!(store.is_empty());
    }
}
