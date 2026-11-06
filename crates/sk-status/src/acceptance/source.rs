//! The **Source-compatible** acceptance policy (`accept`, strict/lenient).
//!
//! # Why this module exists
//!
//! [`super::evaluate_acceptance`] is a *Native* capability surface: it judges a
//! leaner input set and gates on owned-file attribution, a concept the pinned
//! Source acceptance summary does not have at all. It must not be presented as
//! Source parity, and it must never be the default of a Source-compatible
//! `accept` run. This module is the ONE Source-compatible authority: a pure,
//! deterministic function of explicitly collected evidence, mirroring
//! `src/specify_cli/acceptance/{summary_core,gates_core,matrix}.py` row for row.
//!
//! Grounded by execution of the pinned Source
//! (`9aed3a8012cb5eb22c0128b14d525039f95fea7a`, CPython 3.11.15) — see the task
//! report's Source receipts. Facts the policy depends on are carried, never
//! re-derived from the filesystem here: this module performs no I/O.
//!
//! # Source semantics reproduced here
//!
//! * **No owned-file/attribution gate.** Source's summary has no such concept;
//!   a Source-valid mission with no `owned_files` declaration is accepted.
//! * **`strict_metadata`** (`--lenient` → `false`) relaxes EXACTLY two things:
//!   the work-package metadata issues (agent, plus assignee/shell_pid for the
//!   active lanes `doing`/`in_progress`/`for_review`) and the mission path
//!   conventions (blocking `path_violations` become a non-blocking warning).
//!   Every other check — terminal lanes, `tasks.md`, required artifacts, dirty
//!   tree, lanes manifest, branch gate, acceptance matrix evidence and verdict —
//!   still blocks.
//! * **Cancellation provenance is derived, not assumed** (`reason_source`
//!   explicit wins; blank reason → `synthetic`; a reason starting with
//!   `Force move to ` / `move-task: ` → `synthetic`; anything else authored →
//!   `operator`). The carrier receives the already-resolved
//!   `has_operator_provenance` boolean from the canonical reducer accessor, so
//!   this module never re-implements that rule.
//! * **Decision-ledger availability is a fact, not a verdict.** Source treats a
//!   MISSING `decisions/index.json` as an empty index, so every marker id is
//!   unknown and blocks; the collector reports that as `available: true` with an
//!   empty map. Only a PRESENT ledger that this build cannot decode reports
//!   `available: false`, which yields the explicit cannot-evaluate diagnostic
//!   below rather than a guessed status (Source *raises* on a corrupt ledger).
//! * **Deferral is not a failure**: a `deferred_to_consolidation` negative
//!   invariant yields `pass_pending_consolidation`, which does NOT block
//!   acceptance and is disclosed as a skipped check.
//!
//! # Collection is not policy
//!
//! Facts this module cannot invent (dirty paths after Source's own-write
//! filtering, artifact presence, the canonical matrix judgement, path-convention
//! findings, decision-index statuses) are carried by [`SourceAcceptanceFacts`].
//! When a carrier fact is genuinely unavailable the policy records an explicit
//! **cannot-evaluate** diagnostic — never a silent pass and never a fabricated
//! Source refusal.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use crate::acceptance::{
    AcceptanceCriterion, AcceptanceMatrix, VERDICT_FAIL, VERDICT_PASS, VERDICT_PENDING,
    validate_invariant_provenance,
};
use crate::lane::Lane;
use crate::mutation_readiness::is_acceptable_ending;

/// Source `gates_core.PLANNING_ARTIFACT_ONLY_SKIP_REASON`.
pub const PLANNING_ARTIFACT_ONLY_SKIP_REASON: &str = "planning_artifact_only";

/// Source `summary_core._PATH_CONVENTIONS_NOT_SATISFIED`.
pub const PATH_CONVENTIONS_NOT_SATISFIED: &str = "Path conventions not satisfied.";

/// Source `summary_core._ACTIVE_METADATA_LANES`.
///
/// A work package in one of these lanes is still in flight, so the strict
/// metadata gate requires `assignee` and `shell_pid`. A terminal
/// (`approved`/`done`) work package is exempt.
pub const ACTIVE_METADATA_LANES: [&str; 3] = ["doing", "in_progress", "for_review"];

/// Source `_LEGACY_NOT_DONE_LANES`, in order.
pub const LEGACY_NOT_DONE_LANES: [&str; 5] =
    ["planned", "claimed", "doing", "in_progress", "for_review"];

/// Source `AcceptanceMatrix` fourth verdict (deferral, non-blocking).
///
/// Re-exported from the matrix model's own authority so there is exactly one
/// definition of the verdict vocabulary.
pub use crate::acceptance::VERDICT_PASS_PENDING_CONSOLIDATION;

/// The seven required-artifact basenames Source resolves through the mission
/// type's `expected-artifacts.yaml` for `software-dev`.
///
/// The carrier already supplies the resolved names, so this constant exists only
/// to document where the values come from.
pub const REQUIRED_ARTIFACT_NAMES: [&str; 3] = ["spec.md", "plan.md", "tasks.md"];

/// Asset-fix order messages, exactly as `summary_core._build_recommended_fix_order`
/// emits them, in evaluation order.
const FIX_COMMIT_TREE: &str = "Commit, stash, or discard working tree changes before acceptance.";
const FIX_MISSION_BRANCH: &str =
    "Switch to the mission branch or configured target branch named in the branch failure.";
const FIX_ARTIFACTS: &str = "Restore required mission artifacts before acceptance.";
const FIX_METADATA: &str = "Fix work-package metadata issues.";
const FIX_LANES: &str = "Move all work packages to approved or done.";
const FIX_TASKS: &str = "Complete unchecked items in tasks.md.";
const FIX_CLARIFICATION: &str = "Resolve open NEEDS CLARIFICATION markers.";
const FIX_MATRIX_MISSING: &str = "Create or restore kitty-specs/<mission>/acceptance-matrix.json.";
const FIX_LANES_MANIFEST: &str = "Restore or regenerate kitty-specs/<mission>/lanes.json.";
const FIX_EVIDENCE: &str = "Fill missing acceptance matrix evidence fields.";
const FIX_VERDICT: &str =
    "Resolve pending or failing acceptance matrix criteria and negative invariants.";
const FIX_WORKFLOW: &str = "Add successful GitHub Actions run evidence for workflow changes.";
const FIX_PATHS: &str = "Fix mission path convention violations.";

/// One `{check, detail}` diagnostic (Source `AcceptanceCheckDiagnostic`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AcceptanceDiagnostic {
    /// Stable check id, e.g. `lanes_manifest` or `acceptance_matrix`.
    pub check: String,
    /// Operator-facing detail.
    pub detail: String,
}

impl AcceptanceDiagnostic {
    /// Construct a diagnostic.
    pub fn new(check: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            check: check.into(),
            detail: detail.into(),
        }
    }

    /// The Source `to_dict()` shape.
    pub fn to_json(&self) -> Value {
        json!({"check": self.check, "detail": self.detail})
    }
}

/// One work package's collected Source acceptance facts.
///
/// `path` is repository-relative (Source `WorkPackageState.path`), `title` is
/// raw frontmatter text (Source strips surrounding quotes), and every runtime
/// slot is the reduced snapshot's value, formatted as Source's `str(...)` does.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceWpFacts {
    /// Work package id.
    pub wp_id: String,
    /// Authored title.
    ///
    /// The collector supplies the parsed YAML scalar (quotes already removed); a
    /// raw-text caller may pass the frontmatter text verbatim. Source's
    /// `strip('"')` is applied either way.
    pub title: String,
    /// Repository-relative work-package file path.
    pub path: String,
    /// Canonical lane from the reduced snapshot; `None` when the WP has no entry.
    pub lane: Option<String>,
    /// Live claim slot (`agent`).
    pub agent: Option<String>,
    /// Derived `implementer_of_record` fallback for `agent`.
    pub implementer_of_record: Option<String>,
    /// `assignee` slot.
    pub assignee: Option<String>,
    /// `shell_pid` slot, stringified.
    pub shell_pid: Option<String>,
    /// Cancellation provenance from the canonical accessor
    /// (`status_lanes.has_operator_provenance`).
    pub has_operator_provenance: bool,
    /// Cancellation reason recorded on the canceled event.
    pub cancellation_reason: Option<String>,
    /// Actor recorded on the canceled event.
    pub cancellation_actor: Option<String>,
    /// `last_transition_at` of the canceled work package.
    pub cancellation_at: Option<String>,
}

/// The `lanes.json` read outcome (Source `_resolve_lanes_manifest_or_stop`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LanesManifestFacts {
    /// A readable manifest with the facts the branch gate needs.
    Readable {
        /// `lanes.json` target branch.
        target_branch: String,
        /// Mission branch, allowed in addition to the target for code missions.
        mission_branch: String,
        /// Source `lanes.compute.mission_has_code` over this manifest.
        mission_has_code: bool,
    },
    /// The manifest is genuinely absent.
    Missing {
        /// Source `MissingLanesError` text (names the feature dir + remedies).
        message: String,
    },
    /// The manifest exists but cannot be parsed.
    Corrupt {
        /// Source `CorruptLanesError` text.
        message: String,
    },
    /// No `lanes.json` is consulted at all: the Mission's recorded runtime has
    /// no work-package iteration step, so Source's non-WP acceptance branch
    /// (`acceptance.non_wp_runtime`) never runs the lane gates
    /// (`_check_lane_gates`: manifest → branch → acceptance matrix).
    ///
    /// This is a collected FACT about the Mission, not a missing read: a
    /// non-WP Mission is expected to have no `lanes.json`, and Source neither
    /// requires nor reads one. The variant carries the two things that branch
    /// replaces the lane gates with: the ONE legitimate "the matrix gate never
    /// ran" reason, and the runtime-completeness/guard activity issues that
    /// Source seeds the summary's `activity_issues` with before anything else.
    NotRequired {
        /// Source `gates_core.PLANNING_ARTIFACT_ONLY_SKIP_REASON` — the ONLY
        /// skip reason the pre-stamp guard may bypass on.
        skip_reason: String,
        /// Source `collect_non_wp_runtime_facts`'s activity issues, in Source's
        /// order (missing review action, incomplete runtime, non-missing guard
        /// failures, no declared blocking artifacts).
        activity_issues: Vec<String>,
    },
}

/// `meta.json`'s declared `target_branch`, when present.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MetaTargetBranch {
    /// No `target_branch` is declared.
    Undeclared,
    /// Declared branch name.
    Declared(String),
}

/// Mission path-convention evaluation outcome (`summary_core.evaluate_path_conventions`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PathConventionFacts {
    /// The mission declares no path conventions: Source returns a no-op.
    NotDeclared,
    /// A canonical validator evaluated the declarations.
    Evaluated {
        /// Source's `not path_result.missing_paths` discriminator: when true the
        /// declarations are satisfied and NOTHING blocks (the waiver notice, if
        /// any, becomes the advisory text instead of a formatted finding).
        satisfied: bool,
        /// `validate_mission_paths(...).format_errors()` — the blocking text.
        blocking_text: Option<String>,
        /// `validate_mission_paths(...).format_warnings()` — the advisory text.
        warning_text: Option<String>,
        /// Contracts-waiver notice appended to whichever text is surfaced.
        notice: Option<String>,
        /// Waived artifact tokens (dropped from "Optional artifacts missing").
        dedup_tokens: Vec<String>,
    },
    /// The mission declares path conventions but this build has no canonical
    /// validator to evaluate them.
    ///
    /// Recorded as an explicit cannot-evaluate diagnostic: never a silent pass
    /// (Source would have judged them) and never a fabricated Source refusal.
    Unevaluated {
        /// What is missing and why the verdict cannot be reached.
        detail: String,
    },
}

/// One planning artifact scanned for `NEEDS CLARIFICATION` markers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClarificationScan {
    /// Absolute or repository-relative artifact path, as reported by Source.
    pub path: String,
    /// The already-read UTF-8 text. Source reads it with
    /// `encoding="utf-8-sig"`; the collector is responsible for that decode.
    pub text: String,
    /// Source reads the index freshly for each artifact with linked markers.
    /// The collector captures that exact lookup snapshot; None is for callers
    /// explicitly supplying the global Native facts view instead.
    pub observed_decision_statuses:
        Option<BTreeMap<sk_kernel::PythonString, ClarificationDecisionStatus>>,
}

/// The decision ledger status Source's clarification gate consults.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClarificationDecisionStatus {
    /// Still open: a marker naming it blocks acceptance.
    Open,
    /// Deferred: a marker naming it blocks acceptance.
    Deferred,
    /// Resolved: a marker naming it does not block.
    Resolved,
    /// Canceled: a marker naming it does not block.
    Canceled,
}

impl ClarificationDecisionStatus {
    /// Source `entry.status in {DecisionStatus.OPEN, DecisionStatus.DEFERRED}`.
    pub fn blocks(self) -> bool {
        matches!(self, Self::Open | Self::Deferred)
    }
}

/// The acceptance-matrix gate outcome (`gates_core._evaluate_acceptance_matrix`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MatrixEvidence {
    /// No matrix is ever produced: a planning-artifact-only mission.
    NotRequired,
    /// The gate stopped before it could judge a matrix (blocked lanes manifest,
    /// blocked branch gate, cannot-evaluate surface, lock timeout).
    GateStopped {
        /// Reason recorded on the four skipped matrix checks.
        reason: String,
    },
    /// The gate ran, the matrix is required, and the file is absent.
    Missing {
        /// Source's "required for lane-based features but was not found" message.
        message: String,
        /// The surface the gate resolved; `None` when it never resolved one.
        directory: Option<String>,
    },
    /// The matrix was read and judged.
    Judged {
        /// The matrix the verdict was derived from (already populated/enforced by
        /// the caller's canonical writers on the mutating leg).
        matrix: AcceptanceMatrix,
        /// The surface the matrix was read from.
        directory: Option<String>,
        /// `false` for the read-only diagnose leg, which skips negative-invariant
        /// execution and says so.
        negative_invariants_executed: bool,
    },
}

/// Everything the Source acceptance policy is a function of.
///
/// Every field is a collected fact. Nothing here is inferred from an ambient
/// environment, and no default makes an uncollected fact look benign: callers
/// construct the struct explicitly (see the collector in `sk-workspace`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceAcceptanceFacts {
    /// Mission slug as the operator/accept surface names it.
    pub mission_slug: String,
    /// Mission number, when assigned.
    pub mission_number: Option<String>,
    /// Mission type.
    pub mission_type: Option<String>,
    /// Repository root (display text).
    pub repo_root: String,
    /// Primary mission directory (display text).
    pub feature_dir: String,
    /// `tasks/` directory (display text).
    pub tasks_dir: String,
    /// Checkout root the run is reading (display text).
    pub worktree_root: String,
    /// Primary repository root behind a worktree (display text).
    pub primary_repo_root: String,
    /// Current branch; `None` for a detached HEAD or an unreadable checkout.
    pub branch: Option<String>,
    /// `lanes.json` read outcome.
    pub lanes_manifest: LanesManifestFacts,
    /// `meta.json`'s declared target branch.
    pub meta_target_branch: MetaTargetBranch,
    /// Work packages in `tasks/*.md` order.
    pub work_packages: Vec<SourceWpFacts>,
    /// Whether `status.events.jsonl` exists.
    pub status_events_present: bool,
    /// Whether the reduced snapshot carried any work package.
    pub status_snapshot_present: bool,
    /// `tasks.md` text, when the artifact exists.
    pub tasks_md: Option<String>,
    /// Planning artifacts scanned for clarification markers.
    pub clarifications: Vec<ClarificationScan>,
    /// Decision-ledger statuses by decision id.
    ///
    /// An id absent from this map is *unknown*, which Source treats exactly like
    /// an open decision (blocking) — that is a collection property, not a
    /// policy choice.
    pub decision_statuses: BTreeMap<sk_kernel::PythonString, ClarificationDecisionStatus>,
    /// Whether the collector resolved the decision ledger.
    ///
    /// `true` means required lookups were completed (including Source's empty
    /// missing index), OR no linked marker required an index read. It does NOT
    /// assert an unconsulted on-disk ledger is readable. Source collection
    /// propagates read/probe errors; false remains an explicit Native diagnostic
    /// input capability, never the Source collector's error fallback.
    pub decision_statuses_available: bool,
    /// Required artifacts that are absent, repository-relative as Source reports them.
    pub missing_required_artifacts: Vec<String>,
    /// Optional artifacts that are absent, repository-relative.
    pub missing_optional_artifacts: Vec<String>,
    /// Mission path-convention outcome.
    pub path_conventions: PathConventionFacts,
    /// Display text of the dirty paths Source's dirty gate preserved.
    pub git_dirty: Vec<String>,
    /// Acceptance-matrix gate outcome.
    pub matrix: MatrixEvidence,
}

/// `accept --lenient` (Source `strict_metadata=False`).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SourceAcceptanceOptions {
    /// Downgrade the two checks Source treats as advisory.
    pub lenient: bool,
}

impl SourceAcceptanceOptions {
    /// Source's `strict_metadata` flag (`not lenient`).
    pub fn strict_metadata(self) -> bool {
        !self.lenient
    }
}

/// One work package row of the summary (Source `WorkPackageState`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceWpSummary {
    /// Work package id.
    pub wp_id: String,
    /// Lane bucket (`planned` when the WP has no lane entry).
    pub lane: String,
    /// Title with surrounding quotes stripped.
    pub title: String,
    /// Repository-relative path.
    pub path: String,
    /// Whether the reduced snapshot had a lane for this WP.
    pub has_lane_entry: bool,
    /// The canonical lane, when present.
    pub latest_lane: Option<String>,
    /// The four metadata slots Source reports, in Source's key order.
    pub metadata: BTreeMap<String, Option<String>>,
    /// Cancellation provenance.
    pub has_operator_provenance: bool,
}

/// One `canceled_wps` entry (Source NFR-003 shape).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanceledWp {
    /// Work package id.
    pub wp_id: String,
    /// Operator-authored cancellation reason.
    pub reason: String,
    /// Actor recorded on the cancellation.
    pub actor: String,
    /// `last_transition_at` of the cancellation.
    pub at: String,
}

impl CanceledWp {
    /// The Source `to_dict()` shape.
    pub fn to_json(&self) -> Value {
        json!({
            "wp_id": self.wp_id,
            "reason": self.reason,
            "actor": self.actor,
            "at": self.at,
        })
    }
}

/// The Source-compatible acceptance summary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceAcceptanceSummary {
    /// Mission slug.
    pub mission_slug: String,
    /// Mission number, when assigned.
    pub mission_number: Option<String>,
    /// Mission type.
    pub mission_type: Option<String>,
    /// Current branch.
    pub branch: Option<String>,
    /// Repository root (display text).
    pub repo_root: String,
    /// Primary mission directory (display text).
    pub feature_dir: String,
    /// `tasks/` directory (display text).
    pub tasks_dir: String,
    /// Checkout root (display text).
    pub worktree_root: String,
    /// Primary repository root (display text).
    pub primary_repo_root: String,
    /// Lane buckets, in Source's canonical lane order.
    pub lanes: Vec<(String, Vec<String>)>,
    /// Work-package rows.
    pub work_packages: Vec<SourceWpSummary>,
    /// Strict-metadata issues.
    pub metadata_issues: Vec<String>,
    /// Lane/manifest/matrix/branch/artifact-can-not-evaluate issues.
    pub activity_issues: Vec<String>,
    /// Unchecked `tasks.md` rows after Source's terminal-lane normalization.
    pub unchecked_tasks: Vec<String>,
    /// Planning artifacts carrying blocking clarification markers.
    pub needs_clarification: Vec<String>,
    /// Missing required artifacts.
    pub missing_artifacts: Vec<String>,
    /// Missing optional artifacts, after waiver dedup.
    pub optional_missing: Vec<String>,
    /// Dirty paths.
    pub git_dirty: Vec<String>,
    /// Blocking path-convention text (empty under `--lenient`).
    pub path_violations: Vec<String>,
    /// Non-blocking warnings, in Source's order.
    pub warnings: Vec<String>,
    /// Informational skipped checks.
    pub skipped_checks: Vec<AcceptanceDiagnostic>,
    /// Blocking checks recorded by the lane gates.
    pub blocked_checks: Vec<AcceptanceDiagnostic>,
    /// Source's fixed-order remediation list.
    pub recommended_fix_order: Vec<String>,
    /// Accept-eligible (operator-provenance) cancellations.
    pub canceled_wps: Vec<CanceledWp>,
    /// The surface the matrix gate judged, when it judged one.
    pub acceptance_matrix_dir: Option<String>,
    /// The ONE legitimate reason the matrix gate never ran.
    pub acceptance_matrix_gate_skip_reason: Option<String>,
    /// The judged matrix verdict (`pass` / `fail` / `pending` /
    /// `pass_pending_consolidation`), when a matrix was judged.
    ///
    /// Adapter convenience for rendering `Acceptance: ok (<verdict>)` without
    /// re-deriving the verdict; Source's `to_dict()` carries it only inside the
    /// matrix artifact, so it is deliberately absent from [`Self::to_json`].
    pub matrix_verdict: Option<String>,
}

impl SourceAcceptanceSummary {
    /// Lane bucket ids for `lane`, in bucket order.
    pub fn lane(&self, lane: &str) -> &[String] {
        self.lanes
            .iter()
            .find(|(name, _)| name == lane)
            .map(|(_, ids)| ids.as_slice())
            .unwrap_or(&[])
    }

    fn provenance_by_wp(&self) -> BTreeMap<&str, bool> {
        self.work_packages
            .iter()
            .map(|wp| (wp.wp_id.as_str(), wp.has_operator_provenance))
            .collect()
    }

    /// Source `AcceptanceSummary.all_done`.
    ///
    /// Every tracked work package is at an acceptable ending AND the mission
    /// delivered something (an all-canceled mission is not complete).
    pub fn all_done(&self) -> bool {
        let provenance = self.provenance_by_wp();
        let mut saw_wp = false;
        let mut delivered_something = false;
        for (lane, wp_ids) in &self.lanes {
            for wp_id in wp_ids {
                saw_wp = true;
                let has_provenance = provenance.get(wp_id.as_str()).copied().unwrap_or(false);
                if !acceptable_ending(lane, has_provenance) {
                    return false;
                }
                if acceptable_ending(lane, false) {
                    delivered_something = true;
                }
            }
        }
        if !saw_wp {
            return true;
        }
        delivered_something
    }

    /// Source `AcceptanceSummary.ok`.
    pub fn ok(&self) -> bool {
        self.all_done()
            && self.metadata_issues.is_empty()
            && self.activity_issues.is_empty()
            && self.unchecked_tasks.is_empty()
            && self.needs_clarification.is_empty()
            && self.missing_artifacts.is_empty()
            && self.git_dirty.is_empty()
            && self.path_violations.is_empty()
    }

    fn lane_blockers(&self) -> Vec<String> {
        let provenance = self.provenance_by_wp();
        let mut blockers = Vec::new();
        for (lane, hint) in ACTIONABLE_LANE_BLOCKER_HINTS {
            for wp_id in self.lane(lane) {
                if lane == "canceled" && provenance.get(wp_id.as_str()).copied().unwrap_or(false) {
                    continue;
                }
                blockers.push(format!("{wp_id}: canonical lane is '{lane}'; {hint}."));
            }
        }
        blockers
    }

    fn delivered_nothing_blockers(&self) -> Vec<String> {
        let provenance = self.provenance_by_wp();
        let mut saw_wp = false;
        let mut delivered_something = false;
        for (lane, wp_ids) in &self.lanes {
            for wp_id in wp_ids {
                saw_wp = true;
                let has_provenance = provenance.get(wp_id.as_str()).copied().unwrap_or(false);
                if !acceptable_ending(lane, has_provenance) {
                    return Vec::new();
                }
                if acceptable_ending(lane, false) {
                    delivered_something = true;
                }
            }
        }
        if saw_wp && !delivered_something {
            return vec![
                "mission delivered nothing: every work package is canceled; none reached approved or done"
                    .to_owned(),
            ];
        }
        Vec::new()
    }

    /// Source `AcceptanceSummary.outstanding`, in Source's bucket order.
    pub fn outstanding(&self) -> Vec<(String, Vec<String>)> {
        let mut buckets: Vec<(String, Vec<String>)> = Vec::new();
        let mut not_done: Vec<String> = Vec::new();
        for lane in LEGACY_NOT_DONE_LANES {
            not_done.extend(self.lane(lane).iter().cloned());
        }
        buckets.push(("not_done".to_owned(), not_done));
        buckets.push(("lane_blockers".to_owned(), self.lane_blockers()));
        buckets.push((
            "delivered_nothing".to_owned(),
            self.delivered_nothing_blockers(),
        ));
        buckets.push(("metadata".to_owned(), self.metadata_issues.clone()));
        buckets.push(("activity".to_owned(), self.activity_issues.clone()));
        buckets.push(("unchecked_tasks".to_owned(), self.unchecked_tasks.clone()));
        buckets.push((
            "needs_clarification".to_owned(),
            self.needs_clarification.clone(),
        ));
        buckets.push((
            "missing_artifacts".to_owned(),
            self.missing_artifacts.clone(),
        ));
        buckets.push(("git_dirty".to_owned(), self.git_dirty.clone()));
        buckets.push(("path_violations".to_owned(), self.path_violations.clone()));
        buckets
            .into_iter()
            .filter(|(_, details)| !details.is_empty())
            .collect()
    }

    /// Source `AcceptanceSummary.failed_checks`.
    pub fn failed_checks(&self) -> Vec<AcceptanceDiagnostic> {
        self.outstanding()
            .into_iter()
            .flat_map(|(check, details)| {
                details
                    .into_iter()
                    .map(move |detail| AcceptanceDiagnostic::new(check.clone(), detail))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// The Source `AcceptanceSummary.to_dict()` / `accept --json` shape.
    pub fn to_json(&self) -> Value {
        let mut lanes = Map::new();
        for (lane, wp_ids) in &self.lanes {
            lanes.insert(lane.clone(), json!(wp_ids));
        }
        let work_packages: Vec<Value> = self
            .work_packages
            .iter()
            .map(|wp| {
                let mut metadata = Map::new();
                for (key, value) in &wp.metadata {
                    metadata.insert(
                        key.clone(),
                        match value {
                            Some(text) => json!(text),
                            None => Value::Null,
                        },
                    );
                }
                json!({
                    "id": wp.wp_id,
                    "lane": wp.lane,
                    "title": wp.title,
                    "path": wp.path,
                    "latest_lane": wp.latest_lane,
                    "has_lane_entry": wp.has_lane_entry,
                    "metadata": Value::Object(metadata),
                })
            })
            .collect();
        json!({
            "mission_slug": self.mission_slug,
            "mission_number": self.mission_number,
            "mission_type": self.mission_type,
            "branch": self.branch,
            "repo_root": self.repo_root,
            "feature_dir": self.feature_dir,
            "tasks_dir": self.tasks_dir,
            "worktree_root": self.worktree_root,
            "primary_repo_root": self.primary_repo_root,
            "lanes": Value::Object(lanes),
            "work_packages": work_packages,
            "metadata_issues": self.metadata_issues,
            "activity_issues": self.activity_issues,
            "unchecked_tasks": self.unchecked_tasks,
            "needs_clarification": self.needs_clarification,
            "missing_artifacts": self.missing_artifacts,
            "optional_missing": self.optional_missing,
            "git_dirty": self.git_dirty,
            "path_violations": self.path_violations,
            "warnings": self.warnings,
            "failed_checks": self.failed_checks().iter().map(AcceptanceDiagnostic::to_json).collect::<Vec<_>>(),
            "skipped_checks": self.skipped_checks.iter().map(AcceptanceDiagnostic::to_json).collect::<Vec<_>>(),
            "blocked_checks": self.blocked_checks.iter().map(AcceptanceDiagnostic::to_json).collect::<Vec<_>>(),
            "recommended_fix_order": self.recommended_fix_order,
            "canceled_wps": self.canceled_wps.iter().map(CanceledWp::to_json).collect::<Vec<_>>(),
            "all_done": self.all_done(),
            "ok": self.ok(),
        })
    }
}

/// The actionable lane hints, in Source's declaration order.
const ACTIONABLE_LANE_BLOCKER_HINTS: [(&str, &str); 3] = [
    (
        "in_review",
        "review is still in progress; complete the review and move the work package to approved or done",
    ),
    (
        "blocked",
        "work package is blocked; resolve the blocker and move the work package to approved or done",
    ),
    (
        "canceled",
        "work package is canceled; operator-authored cancellation provenance required \u{2014} reopen or replace it, then move the work package to approved or done",
    ),
];

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

/// Source `str.strip('"')` for a title.
fn strip_title_quotes(title: &str) -> String {
    title.trim_matches('"').to_owned()
}

/// Source `is_acceptable_ending` over a lane string, through the canonical
/// [`is_acceptable_ending`] truth table.
fn acceptable_ending(lane: &str, has_provenance: bool) -> bool {
    match lane.parse::<Lane>() {
        Ok(parsed) => is_acceptable_ending(parsed, has_provenance),
        // A lane value the canonical model does not know is not acceptable —
        // exactly Source's fall-through for any non-{approved,done,canceled} value.
        Err(_) => false,
    }
}

/// Source `_runtime_metadata_field`.
fn metadata_field(primary: Option<&String>, fallback: Option<&String>) -> Option<String> {
    if let Some(value) = primary.filter(|value| !value.is_empty()) {
        return Some(value.clone());
    }
    fallback.filter(|value| !value.is_empty()).cloned()
}

fn skipped_checks(reason: &str, include_presence: bool) -> Vec<AcceptanceDiagnostic> {
    MATRIX_CHECK_LABELS
        .iter()
        .skip(if include_presence { 0 } else { 1 })
        .map(|(check, label)| {
            AcceptanceDiagnostic::new(*check, format!("{label} skipped: {reason}"))
        })
        .collect()
}

/// Validate the matrix evidence rows (Source `validate_matrix_evidence`).
fn validate_matrix_evidence(matrix: &AcceptanceMatrix) -> Vec<String> {
    let mut errors = Vec::new();
    for criterion in &matrix.criteria {
        if !matches!(
            criterion.pass_fail.as_str(),
            VERDICT_PASS | VERDICT_FAIL | VERDICT_PENDING
        ) {
            errors.push(format!(
                "{}: pass_fail must be one of fail, pass, pending; got {}",
                criterion.criterion_id,
                sk_kernel::python_repr(&criterion.pass_fail)
            ));
        }
        errors.extend(validate_manual_evidence(criterion));
    }
    for invariant in &matrix.negative_invariants {
        errors.extend(validate_invariant_provenance(invariant));
    }
    errors
}

/// Source `validate_manual_evidence`: a manual QA row needs evidence, a
/// timestamp and an identity.
fn validate_manual_evidence(criterion: &AcceptanceCriterion) -> Vec<String> {
    if criterion.proof_type != "manual_qa" {
        return Vec::new();
    }
    let mut errors = Vec::new();
    if criterion.evidence.as_deref().is_none_or(str::is_empty) {
        errors.push(format!(
            "{}: manual QA requires evidence (URL/screenshot)",
            criterion.criterion_id
        ));
    }
    if criterion.verified_at.as_deref().is_none_or(str::is_empty) {
        errors.push(format!(
            "{}: manual QA requires verified_at timestamp",
            criterion.criterion_id
        ));
    }
    if criterion.verified_by.as_deref().is_none_or(str::is_empty) {
        errors.push(format!(
            "{}: manual QA requires verified_by identity",
            criterion.criterion_id
        ));
    }
    errors
}

/// Derive the clarification-id markers Source scans for
/// (`summary_core._iter_clarification_decision_ids`).
///
/// A marker is `[NEEDS CLARIFICATION: ...]` followed later on the SAME line by a
/// `<!-- decision_id: <id> -->` comment. A marker with no such comment is prose
/// and never blocks.
pub fn clarification_decision_ids(text: &str) -> Vec<String> {
    let mut decision_ids = Vec::new();
    for line in sk_kernel::python_text::splitlines(text) {
        let Some(marker_start) = line.find("[NEEDS CLARIFICATION:") else {
            continue;
        };
        let Some(relative_end) = line[marker_start..].find(']') else {
            continue;
        };
        let marker_end = marker_start + relative_end;
        let Some(relative_comment) = line[marker_end..].find("<!--") else {
            continue;
        };
        let comment_start = marker_end + relative_comment;
        let Some(relative_close) = line[comment_start + 4..].find("-->") else {
            continue;
        };
        let comment_end = comment_start + 4 + relative_close;
        let comment_body = &line[comment_start + 4..comment_end];
        let Some(relative_id) = comment_body.find("decision_id:") else {
            continue;
        };
        let decision_id_text =
            comment_body[relative_id + "decision_id:".len()..].trim_matches(|character: char| {
                sk_kernel::python_text::is_whitespace(u32::from(character))
            });
        if let Some(decision_id) = decision_body_id(decision_id_text) {
            decision_ids.push(decision_id);
        }
    }
    decision_ids
}

/// The first whitespace-delimited token, when non-empty.
fn decision_body_id(text: &str) -> Option<String> {
    let first = text
        .split(|character: char| sk_kernel::python_text::is_whitespace(u32::from(character)))
        .find(|part| !part.is_empty())?;
    if first.is_empty() {
        None
    } else {
        Some(first.to_owned())
    }
}

/// Source `_missing_artifacts` display order, plus the label helper.
fn normalize_path_token(token: &str) -> &str {
    token.trim().trim_matches('/')
}

/// The inputs Source's fix-order table reads (one bundle instead of nine
/// positional arguments, so a caller cannot silently swap two of them).
struct FixOrderInputs<'a> {
    lanes: &'a [(String, Vec<String>)],
    metadata_issues: &'a [String],
    activity_issues: &'a [String],
    unchecked_tasks: &'a [String],
    needs_clarification: &'a [String],
    missing_artifacts: &'a [String],
    git_dirty: &'a [String],
    path_violations: &'a [String],
    blocked_checks: &'a [AcceptanceDiagnostic],
}

/// Build the recommended fix order (Source `_build_recommended_fix_order`).
fn recommended_fix_order(inputs: &FixOrderInputs<'_>) -> Vec<String> {
    let has_check = |check: &str| inputs.blocked_checks.iter().any(|item| item.check == check);
    let has_issue = |needle: &str| {
        inputs
            .activity_issues
            .iter()
            .any(|issue| issue.contains(needle))
    };
    let non_terminal_lane = inputs
        .lanes
        .iter()
        .any(|(lane, wp_ids)| !wp_ids.is_empty() && !acceptable_ending(lane, false));
    let mut fixes: Vec<&str> = Vec::new();
    if !inputs.git_dirty.is_empty() {
        fixes.push(FIX_COMMIT_TREE);
    }
    if has_check("mission_branch") {
        fixes.push(FIX_MISSION_BRANCH);
    }
    if !inputs.missing_artifacts.is_empty() {
        fixes.push(FIX_ARTIFACTS);
    }
    if !inputs.metadata_issues.is_empty() {
        fixes.push(FIX_METADATA);
    }
    if non_terminal_lane {
        fixes.push(FIX_LANES);
    }
    if !inputs.unchecked_tasks.is_empty() {
        fixes.push(FIX_TASKS);
    }
    if !inputs.needs_clarification.is_empty() {
        fixes.push(FIX_CLARIFICATION);
    }
    if has_check("acceptance_matrix") {
        fixes.push(FIX_MATRIX_MISSING);
    }
    if has_check("lanes_manifest") {
        fixes.push(FIX_LANES_MANIFEST);
    }
    if has_issue("Evidence:") {
        fixes.push(FIX_EVIDENCE);
    }
    if has_issue("Acceptance matrix verdict is") {
        fixes.push(FIX_VERDICT);
    }
    if has_issue("Workflow run evidence required") {
        fixes.push(FIX_WORKFLOW);
    }
    if !inputs.path_violations.is_empty() {
        fixes.push(FIX_PATHS);
    }
    fixes.into_iter().map(str::to_owned).collect()
}

/// Evaluate the Source acceptance summary over collected facts.
///
/// The returned summary is the exact object Source's `collect_feature_summary`
/// produces for the same facts, including failure ordering.
pub fn evaluate_source_acceptance(
    facts: &SourceAcceptanceFacts,
    options: SourceAcceptanceOptions,
) -> SourceAcceptanceSummary {
    let strict_metadata = options.strict_metadata();

    // ---- 1. Lane buckets, in canonical lane order -------------------------
    let mut lanes: Vec<(String, Vec<String>)> = Lane::CANONICAL
        .iter()
        .map(|lane| (lane.as_str().to_owned(), Vec::new()))
        .collect();

    // ---- 2. Per-work-package state, metadata issues, cancellations --------
    let mut metadata_issues: Vec<String> = Vec::new();
    // Source seeds the summary with the mission-type branch's own issues FIRST:
    // a non-WP Mission's runtime-completeness/guard issues precede every
    // canonical-state, lane-gate or matrix issue (`collect_feature_summary`
    // extends `activity_issues` with `non_wp.activity_issues` where it takes
    // that branch).
    let mut activity_issues: Vec<String> = match &facts.lanes_manifest {
        LanesManifestFacts::NotRequired {
            activity_issues, ..
        } => activity_issues.clone(),
        _ => Vec::new(),
    };
    let mut canceled_wps: Vec<CanceledWp> = Vec::new();
    let mut work_packages: Vec<SourceWpSummary> = Vec::new();
    let mut all_packages_acceptable = !facts.work_packages.is_empty();
    for wp in &facts.work_packages {
        let canonical_lane = wp.lane.clone();
        let bucket_lane = canonical_lane
            .clone()
            .unwrap_or_else(|| "planned".to_owned());
        let agent = metadata_field(wp.agent.as_ref(), wp.implementer_of_record.as_ref());
        let assignee = metadata_field(wp.assignee.as_ref(), None);
        let shell_pid = metadata_field(wp.shell_pid.as_ref(), None);

        let mut metadata: BTreeMap<String, Option<String>> = BTreeMap::new();
        metadata.insert("lane".to_owned(), canonical_lane.clone());
        metadata.insert("agent".to_owned(), agent.clone());
        metadata.insert("assignee".to_owned(), assignee.clone());
        metadata.insert("shell_pid".to_owned(), shell_pid.clone());

        if strict_metadata {
            if agent.is_none() {
                metadata_issues.push(format!(
                    "{}: missing agent in canonical runtime state (never claimed; run `spec-kitty agent tasks move-task {} --to {} --agent <name>` to repair)",
                    wp.wp_id, wp.wp_id, bucket_lane
                ));
            }
            let active = canonical_lane
                .as_deref()
                .is_some_and(|lane| ACTIVE_METADATA_LANES.contains(&lane));
            if active {
                if assignee.is_none() {
                    metadata_issues.push(format!(
                        "{}: missing assignee in canonical runtime state",
                        wp.wp_id
                    ));
                }
                if shell_pid.is_none() {
                    metadata_issues.push(format!(
                        "{}: missing shell_pid in canonical runtime state",
                        wp.wp_id
                    ));
                }
            }
        }

        if canonical_lane
            .as_deref()
            .is_some_and(|lane| lane == "canceled")
            && wp.has_operator_provenance
        {
            canceled_wps.push(CanceledWp {
                wp_id: wp.wp_id.clone(),
                reason: wp.cancellation_reason.clone().unwrap_or_default(),
                actor: wp.cancellation_actor.clone().unwrap_or_default(),
                at: wp.cancellation_at.clone().unwrap_or_default(),
            });
        }

        if !acceptable_ending(&bucket_lane, wp.has_operator_provenance) {
            all_packages_acceptable = false;
        }

        match lanes.iter_mut().find(|(lane, _)| lane == &bucket_lane) {
            Some((_, wp_ids)) => wp_ids.push(wp.wp_id.clone()),
            None => lanes
                .iter_mut()
                .find(|(lane, _)| lane == "planned")
                .expect("planned bucket always exists")
                .1
                .push(wp.wp_id.clone()),
        }

        work_packages.push(SourceWpSummary {
            wp_id: wp.wp_id.clone(),
            lane: bucket_lane,
            title: strip_title_quotes(&wp.title),
            path: wp.path.clone(),
            has_lane_entry: canonical_lane.is_some(),
            latest_lane: canonical_lane,
            metadata,
            has_operator_provenance: wp.has_operator_provenance,
        });
    }

    // ---- 3. Canonical state + WP readiness (Source `_collect_snapshot_wps`
    //         then `_validate_wp_readiness`) ---------------------------------
    //
    // A non-WP Mission never gathers work packages at all, so it never runs
    // `_collect_snapshot_wps`/`_validate_wp_readiness` — those activity issues
    // are not Source's for this branch (`collect_non_wp_runtime_facts` supplies
    // its own runtime-completeness issues instead).
    let non_wp = matches!(facts.lanes_manifest, LanesManifestFacts::NotRequired { .. });
    let canonical_state_missing = !facts.status_events_present || !facts.status_snapshot_present;
    if canonical_state_missing && !non_wp {
        activity_issues.push(format!(
            "No canonical state found for mission '{}'. Cannot validate acceptance without status.events.jsonl. Run 'spec-kitty agent mission finalize-tasks --mission {}' to bootstrap the event log.",
            facts.mission_slug, facts.mission_slug
        ));
    }
    if facts.status_events_present && facts.status_snapshot_present {
        for wp in &facts.work_packages {
            match wp.lane.as_deref() {
                None => activity_issues.push(format!(
                    "{}: no canonical state found in status.events.jsonl",
                    wp.wp_id
                )),
                Some(lane) => {
                    if !acceptable_ending(lane, wp.has_operator_provenance) {
                        activity_issues.push(format!(
                            "{}: canonical lane is '{lane}', expected 'approved' or 'done'",
                            wp.wp_id
                        ));
                    }
                }
            }
        }
    }

    // ---- 4. tasks.md checkboxes, normalized by terminal lanes ------------
    // Source `_find_unchecked_tasks` reports the sentinel for a missing file and
    // delegates the row grammar to the canonical fence-aware reader.
    let unchecked_all = match &facts.tasks_md {
        Some(text) => crate::acceptance::iter_unchecked_subtask_rows(text),
        None => vec!["tasks.md missing".to_owned()],
    };
    let tasks_missing = unchecked_all == ["tasks.md missing".to_owned()];
    let unchecked_tasks = if non_wp || tasks_missing || all_packages_acceptable {
        // Source's non-WP branch sets `unchecked_tasks = []` unconditionally:
        // a Mission with no work packages has no lane-normalized task rows.
        Vec::new()
    } else {
        unchecked_all
    };

    // ---- 5. Clarification markers ---------------------------------------
    let mut needs_clarification: Vec<String> = Vec::new();
    for scan in &facts.clarifications {
        let ids = clarification_decision_ids(&scan.text);
        if ids.is_empty() {
            continue;
        }
        if scan.observed_decision_statuses.is_none() && !facts.decision_statuses_available {
            activity_issues.push(format!(
                "Clarification markers in {} cannot be resolved: no decision ledger is readable, so `[NEEDS CLARIFICATION]` markers naming {} cannot be judged open or resolved",
                scan.path,
                ids.join(", ")
            ));
            continue;
        }
        let statuses = scan
            .observed_decision_statuses
            .as_ref()
            .unwrap_or(&facts.decision_statuses);
        let blocking = ids.iter().any(|decision_id| {
            match statuses.get(&sk_kernel::PythonString::from_scalars(decision_id)) {
                // Source: an unknown id is treated exactly like an open decision.
                None => true,
                Some(status) => status.blocks(),
            }
        });
        if blocking {
            needs_clarification.push(scan.path.clone());
        }
    }

    // ---- 6. Path conventions (the ONE strict-only relaxation #2) ---------
    let mut path_violations: Vec<String> = Vec::new();
    let mut path_convention_warning: Option<String> = None;
    let mut dedup_tokens: Vec<String> = Vec::new();
    match &facts.path_conventions {
        PathConventionFacts::NotDeclared => {}
        PathConventionFacts::Evaluated {
            satisfied,
            blocking_text,
            warning_text,
            notice,
            dedup_tokens: waived,
        } => {
            dedup_tokens = waived.clone();
            let with_notice = |text: String| match notice {
                Some(notice) => format!("{text}\n{notice}"),
                None => text,
            };
            if *satisfied {
                // Source: `if not path_result.missing_paths: return [], waiver_notice, waived`.
                path_convention_warning = notice.clone();
            } else if strict_metadata {
                path_violations.push(with_notice(
                    blocking_text
                        .clone()
                        .unwrap_or_else(|| PATH_CONVENTIONS_NOT_SATISFIED.to_owned()),
                ));
            } else {
                path_convention_warning =
                    Some(with_notice(warning_text.clone().unwrap_or_else(|| {
                        PATH_CONVENTIONS_NOT_SATISFIED.to_owned()
                    })));
            }
        }
        PathConventionFacts::Unevaluated { detail } => {
            activity_issues.push(format!(
                "Mission path conventions cannot be evaluated: {detail}"
            ));
        }
    }

    // ---- 7. Warnings ------------------------------------------------------
    let optional_missing: Vec<String> = facts
        .missing_optional_artifacts
        .iter()
        .filter(|entry| {
            !dedup_tokens
                .iter()
                .any(|token| normalize_path_token(token) == normalize_path_token(entry))
        })
        .cloned()
        .collect();
    let mut warnings: Vec<String> = Vec::new();
    if !optional_missing.is_empty() {
        warnings.push(format!(
            "Optional artifacts missing: {}",
            optional_missing.join(", ")
        ));
    }
    if !path_violations.is_empty() {
        warnings.push(PATH_CONVENTIONS_NOT_SATISFIED.to_owned());
    } else if let Some(text) = &path_convention_warning {
        warnings.push(text.clone());
    }

    // ---- 8. Lane gates: manifest -> branch -> matrix ----------------------
    let mut skipped_checks: Vec<AcceptanceDiagnostic> = Vec::new();
    let mut blocked_checks: Vec<AcceptanceDiagnostic> = Vec::new();
    let mut matrix_dir: Option<String> = None;
    let mut matrix_skip_reason: Option<String> = None;
    let mut matrix_verdict: Option<String> = None;

    match &facts.lanes_manifest {
        LanesManifestFacts::Missing { message } => {
            activity_issues.push(message.clone());
            blocked_checks.push(AcceptanceDiagnostic::new("lanes_manifest", message.clone()));
            skipped_checks.extend(skipped_checks_include_presence("lanes.json is missing"));
        }
        LanesManifestFacts::Corrupt { message } => {
            activity_issues.push(message.clone());
            blocked_checks.push(AcceptanceDiagnostic::new("lanes_manifest", message.clone()));
            skipped_checks.extend(skipped_checks_include_presence(
                "lanes.json is corrupt or malformed",
            ));
        }
        LanesManifestFacts::NotRequired { skip_reason, .. } => {
            // Source's non-WP branch skips `_check_lane_gates` wholesale:
            // no manifest/branch/matrix check, no skipped matrix diagnostics,
            // only the ONE legitimate "never ran" skip reason. Its activity
            // issues were seeded at the top of this function.
            matrix_skip_reason = Some(skip_reason.clone());
        }
        LanesManifestFacts::Readable {
            target_branch,
            mission_branch,
            mission_has_code,
        } => {
            let mut proceed = true;
            if let MetaTargetBranch::Declared(declared) = &facts.meta_target_branch
                && declared != target_branch
            {
                let message = format!(
                    "Acceptance target branch mismatch: meta.json targets {declared}, lanes.json targets {target_branch}"
                );
                activity_issues.push(message.clone());
                blocked_checks.push(AcceptanceDiagnostic::new("mission_branch", message));
                skipped_checks.extend(skipped_checks_include_presence(
                    "meta.json target_branch does not match lanes.json target_branch",
                ));
                proceed = false;
            }
            if proceed {
                let mut allowed = vec![target_branch.clone()];
                if *mission_has_code {
                    allowed.push(mission_branch.clone());
                }
                let branch_allowed = facts
                    .branch
                    .as_deref()
                    .is_some_and(|branch| allowed.iter().any(|name| name == branch));
                if !branch_allowed {
                    let allowed_label = {
                        let mut names: Vec<&str> = allowed.iter().map(String::as_str).collect();
                        names.sort_unstable();
                        names.join(", ")
                    };
                    let current_label = facts.branch.as_deref().unwrap_or("detached HEAD");
                    let message = format!(
                        "Acceptance must run on mission or target branch ({allowed_label}), not {current_label}"
                    );
                    activity_issues.push(message.clone());
                    blocked_checks.push(AcceptanceDiagnostic::new("mission_branch", message));
                    skipped_checks.extend(skipped_checks_include_presence(
                        "current branch is neither mission branch nor target branch",
                    ));
                    proceed = false;
                }
                if proceed && !*mission_has_code {
                    skipped_checks.extend(skipped_checks_include_presence(
                        "planning_artifact-only missions do not produce acceptance-matrix.json",
                    ));
                    matrix_skip_reason = Some(PLANNING_ARTIFACT_ONLY_SKIP_REASON.to_owned());
                    proceed = false;
                }
            }
            if proceed {
                match &facts.matrix {
                    MatrixEvidence::Judged {
                        matrix,
                        directory,
                        negative_invariants_executed,
                    } => {
                        matrix_dir = directory.clone();
                        matrix_verdict = Some(matrix.overall_verdict().to_owned());
                        if !negative_invariants_executed && !matrix.negative_invariants.is_empty() {
                            skipped_checks.push(AcceptanceDiagnostic::new(
                                "negative_invariants",
                                "Negative invariant execution skipped: diagnose mode is read-only",
                            ));
                        }
                        judge_matrix(matrix, &mut activity_issues, &mut skipped_checks);
                    }
                    MatrixEvidence::Missing { message, directory } => {
                        matrix_dir = directory.clone();
                        activity_issues.push(message.clone());
                        blocked_checks.push(AcceptanceDiagnostic::new(
                            "acceptance_matrix",
                            message.clone(),
                        ));
                        skipped_checks.extend(skipped_checks_no_presence(
                            "acceptance-matrix.json is missing",
                        ));
                    }
                    MatrixEvidence::GateStopped { reason } => {
                        skipped_checks.extend(skipped_checks_include_presence(reason));
                    }
                    MatrixEvidence::NotRequired => {
                        // Only reachable for a code mission through a caller bug;
                        // record it rather than silently skipping the matrix.
                        let detail = "Mission has code lanes but the acceptance-matrix gate reported no matrix is required";
                        activity_issues.push(detail.to_owned());
                        blocked_checks.push(AcceptanceDiagnostic::new(
                            "acceptance_matrix_cannot_evaluate",
                            detail,
                        ));
                        skipped_checks.extend(skipped_checks_include_presence(
                            "the acceptance-matrix gate did not run",
                        ));
                    }
                }
            }
        }
    }

    // ---- 9. Recommended fix order ----------------------------------------
    let recommended_fix_order = recommended_fix_order(&FixOrderInputs {
        lanes: &lanes,
        metadata_issues: &metadata_issues,
        activity_issues: &activity_issues,
        unchecked_tasks: &unchecked_tasks,
        needs_clarification: &needs_clarification,
        missing_artifacts: &facts.missing_required_artifacts,
        git_dirty: &facts.git_dirty,
        path_violations: &path_violations,
        blocked_checks: &blocked_checks,
    });

    SourceAcceptanceSummary {
        mission_slug: facts.mission_slug.clone(),
        mission_number: facts.mission_number.clone(),
        mission_type: facts.mission_type.clone(),
        branch: facts.branch.clone(),
        repo_root: facts.repo_root.clone(),
        feature_dir: facts.feature_dir.clone(),
        tasks_dir: facts.tasks_dir.clone(),
        worktree_root: facts.worktree_root.clone(),
        primary_repo_root: facts.primary_repo_root.clone(),
        lanes,
        work_packages,
        metadata_issues,
        activity_issues,
        unchecked_tasks,
        needs_clarification,
        missing_artifacts: facts.missing_required_artifacts.clone(),
        optional_missing,
        git_dirty: facts.git_dirty.clone(),
        path_violations,
        warnings,
        skipped_checks,
        blocked_checks,
        recommended_fix_order,
        canceled_wps,
        acceptance_matrix_dir: matrix_dir,
        acceptance_matrix_gate_skip_reason: matrix_skip_reason,
        matrix_verdict,
    }
}

fn skipped_checks_include_presence(reason: &str) -> Vec<AcceptanceDiagnostic> {
    skipped_checks(reason, true)
}

fn skipped_checks_no_presence(reason: &str) -> Vec<AcceptanceDiagnostic> {
    skipped_checks(reason, false)
}

/// Judge one matrix (Source `_judge_acceptance_matrix`).
fn judge_matrix(
    matrix: &AcceptanceMatrix,
    activity_issues: &mut Vec<String>,
    skipped_checks: &mut Vec<AcceptanceDiagnostic>,
) {
    for error in validate_matrix_evidence(matrix) {
        activity_issues.push(format!("Evidence: {error}"));
    }
    let verdict = matrix.overall_verdict();
    match verdict {
        VERDICT_FAIL => activity_issues.push(
            "Acceptance matrix verdict is 'fail' — negative invariants or criteria not satisfied"
                .to_owned(),
        ),
        VERDICT_PENDING => activity_issues.push(
            "Acceptance matrix verdict is 'pending' — criteria or invariants have not been verified"
                .to_owned(),
        ),
        VERDICT_PASS_PENDING_CONSOLIDATION => skipped_checks.push(AcceptanceDiagnostic::new(
            "negative_invariants_deferred",
            "One or more negative invariants are deferred to post-consolidation verification; acceptance is not blocked, but this loop does not verify them — the post-consolidation op (or PR CI) must, before the mission is done.",
        )),
        _ => {}
    }
}

/// Source's terminal-invariant vocabulary, re-exported for callers that build
/// carrier matrices.
pub use crate::acceptance::TERMINAL_INVARIANT_RESULTS as TERMINAL_INVARIANTS;
