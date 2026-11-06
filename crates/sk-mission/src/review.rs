//! Mission review: mode resolution, gates, and the `mission-review-report.md`
//! artifact.
//!
//! This is the Rust projection of `spec-kitty review`:
//!
//! * [`resolve_mode`] keeps the Source precedence (`--mode`, then
//!   `meta.json.baseline_merge_commit`, then lightweight) and the
//!   `MISSION_REVIEW_MODE_MISMATCH` refusal;
//! * gate 1 is the work-package lane check (all terminal) plus the rejected
//!   review-artifact consistency check;
//! * gate 2 is the dead-code scan over caller-supplied changed-symbol evidence.
//!   The Git read itself belongs to the caller because this crate has no Git
//!   dependency; the scan, corpus rules and diagnostics are here and are
//!   byte-for-byte Source-shaped. An absent evidence set is `undeterminable`,
//!   never "no dead code";
//! * gate 3 is the auth/storage BLE001 suppression audit, read from the real
//!   files under the repository root;
//! * gate 4 validates the mission's issue matrix when one exists;
//! * [`MissionReviewReport::write`] writes the Source report frontmatter and
//!   the `## Findings` lines.
//!
//! Nothing in this module invents a verdict: every gate either runs over real
//! input, records `skip` with a named native gap, or fails.

use std::path::{Path, PathBuf};

use sk_kernel::clock::now_utc_iso;
use sk_kernel::read_bounded;

/// Review mode of one `spec-kitty review` invocation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MissionReviewMode {
    /// Consistency check only.
    #[default]
    Lightweight,
    /// Full post-merge release-gate contract.
    PostMerge,
}

impl MissionReviewMode {
    /// The wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lightweight => "lightweight",
            Self::PostMerge => "post-merge",
        }
    }

    /// Parse the CLI spelling (`lightweight` | `post-merge`).
    pub fn parse(value: &str) -> Result<Self, MissionReviewError> {
        match value {
            "lightweight" => Ok(Self::Lightweight),
            "post-merge" => Ok(Self::PostMerge),
            other => Err(MissionReviewError::InvalidMode(other.to_owned())),
        }
    }
}

/// `MISSION_REVIEW_MODE_MISMATCH` refusal body (Source `_mode.py`).
pub const MODE_MISMATCH_BODY: &str = "MISSION_REVIEW_MODE_MISMATCH: --mode post-merge was requested but meta.json.baseline_merge_commit is absent, meaning no merge has been recorded for this mission — neither via `spec-kitty consolidate` nor from a GitHub PR acceptance.";

/// Review failures that are not gate findings.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MissionReviewError {
    /// `--mode` was not a known mode.
    InvalidMode(String),
    /// `--mode post-merge` was requested without a recorded baseline.
    ModeMismatch,
    /// `meta.json` could not be read.
    Metadata(String),
    /// A report artifact could not be written.
    Report(String),
}

impl std::fmt::Display for MissionReviewError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidMode(value) => {
                write!(
                    formatter,
                    "invalid review mode {value:?}: expected lightweight or post-merge"
                )
            }
            Self::ModeMismatch => formatter.write_str(MODE_MISMATCH_BODY),
            Self::Metadata(detail) => write!(formatter, "{detail}"),
            Self::Report(detail) => write!(formatter, "{detail}"),
        }
    }
}

impl std::error::Error for MissionReviewError {}

/// Resolve the review mode (Source precedence, `auto_detected` reported).
pub fn resolve_mode(
    cli_flag: Option<&str>,
    baseline_merge_commit: Option<&str>,
) -> Result<(MissionReviewMode, bool), MissionReviewError> {
    if let Some(flag) = cli_flag {
        let mode = MissionReviewMode::parse(flag)?;
        if mode == MissionReviewMode::PostMerge
            && baseline_merge_commit
                .map(str::trim)
                .unwrap_or_default()
                .is_empty()
        {
            return Err(MissionReviewError::ModeMismatch);
        }
        return Ok((mode, false));
    }
    if baseline_merge_commit
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
    {
        return Ok((MissionReviewMode::PostMerge, true));
    }
    Ok((MissionReviewMode::Lightweight, true))
}

/// A recorded gate execution (Source `GateRecord`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GateRecord {
    /// `gate_1` .. `gate_4`.
    pub id: &'static str,
    /// Gate name.
    pub name: &'static str,
    /// `pass` | `fail` | `skip`.
    pub result: &'static str,
}

impl GateRecord {
    fn exit_code(&self) -> u8 {
        if self.result == "fail" { 1 } else { 0 }
    }
}

/// A typed review finding. `kind` is the Source finding `type`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewFinding {
    /// The Source finding type (`wp_not_done`, `dead_code`, ...).
    pub kind: String,
    /// Free-form fields rendered into the finding line.
    pub fields: Vec<(&'static str, String)>,
}

impl ReviewFinding {
    /// Build a finding from `kind` plus ordered fields.
    pub fn new(kind: &str, fields: &[(&'static str, String)]) -> Self {
        Self {
            kind: kind.to_owned(),
            fields: fields.to_vec(),
        }
    }

    /// Read one field.
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.as_str())
    }

    /// The Source hard-failure classification.
    pub fn is_hard_failure(&self) -> bool {
        matches!(
            self.kind.as_str(),
            "wp_not_done"
                | "rejected_review_artifact"
                | "review_artifact_schema_invalid"
                | "ble001_suppression"
                | "issue_matrix_violation"
                | "dead_code_baseline_missing"
                | "dead_code_undeterminable"
                | "dead_code_evidence_incomplete"
        )
    }

    /// The Source `_format_finding_line` rendering, `None` for unknown kinds.
    pub fn render(&self) -> Option<String> {
        let get = |name: &str| self.field(name).unwrap_or("unknown").to_owned();
        let line = match self.kind.as_str() {
            "wp_not_done" => format!(
                "- **wp_not_done** `{}`: lane is `{}`",
                get("wp_id"),
                get("lane")
            ),
            "dead_code" => format!(
                "- **dead_code** `{}` — `{}`: no non-test callers found",
                get("file"),
                get("symbol")
            ),
            "dead_code_baseline_missing" => format!(
                "- **dead_code_baseline_missing** `{}`: {}",
                get("diagnostic_code"),
                get("remediation")
            ),
            "dead_code_undeterminable" => format!(
                "- **dead_code_undeterminable** `{}`: {}; remediation=`{}`",
                get("diagnostic_code"),
                get("reason"),
                get("remediation")
            ),
            "dead_code_evidence_incomplete" => format!(
                "- **dead_code_evidence_incomplete** `{}`: pr_merge_evidence=`{}`; remediation=`{}`",
                get("diagnostic_code"),
                get("pr_merge_evidence"),
                get("remediation")
            ),
            "dead_code_not_applicable" => format!(
                "- **dead_code_not_applicable** `{}`: unsupported_extensions=`{}`; excluded_test_paths=`{}`; remediation=`{}`",
                get("diagnostic_code"),
                get("unsupported_extensions"),
                get("excluded_test_paths"),
                get("remediation")
            ),
            "ble001_suppression" => format!(
                "- **ble001_suppression** `{}:{}`: `{}`; remediation=`{}`",
                get("file"),
                get("line"),
                get("content"),
                get("remediation")
            ),
            "rejected_review_artifact" => format!(
                "- **rejected_review_artifact** `{}`: lane is `{}`, latest artifact is `{}`; diagnostic_code=`{}`, branch_or_work_package=`{}`, violated_invariant=`{}`, remediation=`{}`",
                get("wp_id"),
                get("lane"),
                get("artifact_path"),
                get("diagnostic_code"),
                get("branch_or_work_package"),
                get("violated_invariant"),
                get("remediation")
            ),
            "review_artifact_schema_invalid" => format!(
                "- **review_artifact_schema_invalid** `{}`: lane is `{}`, latest artifact is `{}`; diagnostic_code=`{}`, branch_or_work_package=`{}`, violated_invariant=`{}`, schema_error=`{}`, remediation=`{}`",
                get("wp_id"),
                get("lane"),
                get("artifact_path"),
                get("diagnostic_code"),
                get("branch_or_work_package"),
                get("violated_invariant"),
                get("schema_error"),
                get("remediation")
            ),
            "issue_matrix_violation" => format!(
                "- **issue_matrix_violation** `{}`: {}",
                get("diagnostic_code"),
                get("message")
            ),
            _ => return None,
        };
        Some(line)
    }
}

/// Diagnostics codes shared with Source `MissionReviewDiagnostic`.
pub mod diagnostic {
    /// A work package is not in a terminal lane.
    pub const WP_NOT_DONE: &str = "MISSION_REVIEW_WP_NOT_DONE";
    /// No usable baseline merge commit was recorded.
    pub const LIGHTWEIGHT_MISSING_BASELINE: &str = "MISSION_REVIEW_LIGHTWEIGHT_MISSING_BASELINE";
    /// A legacy mission with no mission id has no dead-code anchor.
    pub const LEGACY_MISSION_DEAD_CODE_SKIP: &str = "MISSION_REVIEW_LEGACY_MISSION_DEAD_CODE_SKIP";
    /// The dead-code scan could not run.
    pub const DEAD_CODE_UNDETERMINABLE: &str = "MISSION_REVIEW_DEAD_CODE_UNDETERMINABLE";
    /// The dead-code scan does not apply to the change set.
    pub const DEAD_CODE_NOT_APPLICABLE: &str = "MISSION_REVIEW_DEAD_CODE_NOT_APPLICABLE";
    /// The recorded baseline anchor is not provably complete.
    pub const DEAD_CODE_EVIDENCE_INCOMPLETE: &str = "MISSION_REVIEW_DEAD_CODE_EVIDENCE_INCOMPLETE";
    /// The issue matrix is missing (no table).
    pub const ISSUE_MATRIX_MISSING: &str = "MISSION_REVIEW_ISSUE_MATRIX_MISSING";
    /// The issue matrix is structurally invalid.
    pub const ISSUE_MATRIX_INVALID: &str = "MISSION_REVIEW_ISSUE_MATRIX_INVALID";
    /// The matrix columns/tables do not match the canonical schema.
    pub const ISSUE_MATRIX_SCHEMA_DRIFT: &str = "MISSION_REVIEW_ISSUE_MATRIX_SCHEMA_DRIFT";
    /// A row's verdict is outside the closed set.
    pub const ISSUE_MATRIX_VERDICT_UNKNOWN: &str = "MISSION_REVIEW_ISSUE_MATRIX_VERDICT_UNKNOWN";
    /// More than one markdown table was found.
    pub const ISSUE_MATRIX_MULTI_TABLE: &str = "MISSION_REVIEW_ISSUE_MATRIX_MULTI_TABLE";
    /// A row's `evidence_ref` is empty.
    pub const ISSUE_MATRIX_EVIDENCE_EMPTY: &str = "MISSION_REVIEW_ISSUE_MATRIX_EVIDENCE_REF_EMPTY";
    /// A `deferred-with-followup` row has no follow-up handle.
    pub const ISSUE_MATRIX_DEFERRED_WITHOUT_HANDLE: &str =
        "MISSION_REVIEW_ISSUE_MATRIX_DEFERRED_WITHOUT_HANDLE";
}

/// Source remediation text for an undeterminable dead-code scan.
const UNDETERMINABLE_REMEDIATION: &str =
    "Verify the baseline commit and Git repository, then rerun `spec-kitty review`.";
/// Source remediation text for a scan that does not apply.
const NOT_APPLICABLE_REMEDIATION: &str = "The dead-code scan only analyzes Python sources. For other languages, add tech-specific review guidance and your own analyzer or test command to your local charter (see 'Extend your charter for an unsupported language').";
/// Source remediation for an incomplete PR anchor.
const PR_MERGE_EVIDENCE_INCOMPLETE_REMEDIATION: &str = "This mission's baseline_merge_commit was recorded from a PR landing whose anchor evidence is not recognized as complete. Re-record the baseline from the PR's landing commit with the operator attestation (`spec-kitty migrate backfill-merge-commit --mission <slug> --merge-commit <sha> --attest-first-landing-commit`), then rerun review.";
/// Source remediation for a PR-accepted mission whose merge was never recorded.
const PR_MERGE_UNRECORDED_REMEDIATION: &str = "This mission was accepted via PR (acceptance_mode: pr): the missing baseline_merge_commit means the PR merge was never recorded, not that the mission never merged. If the PR has merged, record the real merge commit with `spec-kitty migrate backfill-merge-commit --mission <slug> --merge-commit <sha>` (or re-run `spec-kitty accept --mode pr --merge-commit <sha>`); if it has not, merge it, then rerun review with `--mode post-merge`.";
/// Source remediation for a mission that never merged.
const NEVER_MERGED_REMEDIATION: &str = "Run `spec-kitty consolidate` to bake baseline_merge_commit into meta.json, or rerun review with `--mode post-merge` after merge.";

/// Git-derived dead-code evidence supplied by the caller.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DeadCodeEvidence {
    /// Every path changed between the baseline and `HEAD`.
    pub changed_paths: Vec<String>,
    /// Paths the Python-only scan supports.
    pub supported_paths: Vec<String>,
    /// `(symbol, defining path)` for added public symbols.
    pub added_symbols: Vec<(String, String)>,
    /// `(path, source)` for the Python corpus the scan searches.
    pub corpus: Vec<(String, String)>,
    /// `Some(reason)` when discovery could not run at all.
    pub discovery_error: Option<String>,
}

impl DeadCodeEvidence {
    /// Was changed-symbol discovery possible?
    pub fn is_available(&self) -> bool {
        self.discovery_error.is_none()
    }

    /// Sorted, de-duplicated extensions of unsupported changed paths plus the
    /// count of test-only Python paths (Source `_summarize_unsupported`).
    pub fn unsupported_summary(&self) -> (Vec<String>, usize) {
        let supported = self.supported_paths.as_slice();
        let mut extensions: Vec<String> = Vec::new();
        let mut excluded_test_paths = 0;
        for path in &self.changed_paths {
            if supported.contains(path) {
                continue;
            }
            if is_python_path(path) {
                excluded_test_paths += 1;
            } else {
                let extension = extension_of(path);
                if !extensions.contains(&extension) {
                    extensions.push(extension);
                }
            }
        }
        extensions.sort();
        (extensions, excluded_test_paths)
    }
}

/// Inputs to one mission review.
#[derive(Clone, Debug, Default)]
pub struct ReviewInputs {
    /// Repository root (used by the BLE001 audit and the report path).
    pub repo_root: PathBuf,
    /// The mission directory the report is written into.
    pub feature_dir: PathBuf,
    /// Mission slug.
    pub mission_slug: String,
    /// Mission id, when the mission has one.
    pub mission_id: Option<String>,
    /// Human-friendly name.
    pub friendly_name: Option<String>,
    /// `meta.json.baseline_merge_commit`.
    pub baseline_merge_commit: Option<String>,
    /// `meta.json.acceptance_mode`.
    pub acceptance_mode: Option<String>,
    /// `meta.json.pr_merge_evidence`.
    pub pr_merge_evidence: Option<String>,
    /// `(wp_id, lane)` from the canonical status snapshot.
    pub work_packages: Vec<(String, String)>,
    /// `(wp_id, lane, artifact path)` for terminal WPs whose latest review
    /// artifact is a rejection.
    pub rejected_review_artifacts: Vec<(String, String, String)>,
    /// Git-derived dead-code evidence, when the caller could produce it.
    pub dead_code: Option<DeadCodeEvidence>,
}

/// The review report.
#[derive(Clone, Debug, Default)]
pub struct MissionReviewReport {
    /// `pass` | `pass_with_notes` | `fail`.
    pub verdict: &'static str,
    /// Resolved mode.
    pub mode: MissionReviewMode,
    /// Findings in gate order.
    pub findings: Vec<ReviewFinding>,
    /// Gate records for the frontmatter.
    pub gates: Vec<GateRecord>,
    /// Issue-matrix presence: `true`/`false`/`not_applicable`.
    pub issue_matrix_present: String,
    /// Mission-exception presence: `true`/`false`/`not_applicable`.
    pub mission_exception_present: String,
    /// Native gaps that made a Source gate `skip` rather than run.
    pub native_gaps: Vec<String>,
    /// Where the report was written.
    pub report_path: PathBuf,
    /// The exact report bytes.
    pub report_bytes: Vec<u8>,
}

impl MissionReviewReport {
    /// The `review --json` payload.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "verdict": self.verdict,
            "mode": self.mode.as_str(),
            "findings": self.findings.iter().map(|finding| {
                let mut map = serde_json::Map::new();
                map.insert("type".into(), serde_json::json!(finding.kind));
                for (key, value) in &finding.fields {
                    map.insert((*key).into(), serde_json::json!(value));
                }
                serde_json::Value::Object(map)
            }).collect::<Vec<_>>(),
            "gates_recorded": self.gates.iter().map(|gate| serde_json::json!({
                "id": gate.id,
                "name": gate.name,
                "command": format!("spec-kitty review (internal {})", gate.id.replace('_', " ")),
                "exit_code": gate.exit_code(),
                "result": gate.result,
            })).collect::<Vec<_>>(),
            "issue_matrix_present": self.issue_matrix_present,
            "mission_exception_present": self.mission_exception_present,
            "native_gaps": self.native_gaps,
            "report_path": self.report_path.to_string_lossy(),
        })
    }

    /// The exact Source report text.
    pub fn render(&self) -> String {
        let reviewed_at = now_utc_iso();
        let mut lines: Vec<String> = vec![
            "---".to_owned(),
            format!("verdict: {}", self.verdict),
            format!("mode: {}", self.mode.as_str()),
            format!("reviewed_at: {reviewed_at}"),
            format!("findings: {}", self.findings.len()),
        ];
        if self.gates.is_empty() {
            lines.push("gates_recorded: []".to_owned());
        } else {
            lines.push("gates_recorded:".to_owned());
            for gate in &self.gates {
                lines.push(format!("  - id: {}", gate.id));
                lines.push(format!("    name: {}", gate.name));
                lines.push(format!(
                    "    command: spec-kitty review (internal {})",
                    gate.id.replace('_', " ")
                ));
                lines.push(format!("    exit_code: {}", gate.exit_code()));
                lines.push(format!("    result: {}", gate.result));
            }
        }
        lines.push(format!(
            "issue_matrix_present: {}",
            self.issue_matrix_present
        ));
        lines.push(format!(
            "mission_exception_present: {}",
            self.mission_exception_present
        ));
        lines.push("---".to_owned());
        lines.push(String::new());
        if self.findings.is_empty() {
            lines.push("No findings.".to_owned());
        } else {
            lines.push("## Findings".to_owned());
            lines.push(String::new());
            for finding in &self.findings {
                if let Some(line) = finding.render() {
                    lines.push(line);
                }
            }
        }
        let mut text = lines.join("\n");
        text.push('\n');
        text
    }

    /// Write `mission-review-report.md` atomically with readback.
    pub fn write(&mut self) -> Result<PathBuf, MissionReviewError> {
        let path = self.report_path.clone();
        let text = self.render();
        let bytes = text.into_bytes();
        sk_kernel::atomic_write(&path, &bytes).map_err(|error| {
            MissionReviewError::Report(format!("could not write {}: {error}", path.display()))
        })?;
        let landed = std::fs::read(&path).map_err(|error| {
            MissionReviewError::Report(format!("could not read back {}: {error}", path.display()))
        })?;
        if landed != bytes {
            return Err(MissionReviewError::Report(format!(
                "report readback at {} does not match the written bytes",
                path.display()
            )));
        }
        self.report_bytes = bytes;
        Ok(path)
    }
}

/// Run the mission review over real inputs.
///
/// Returns the report (already written). A `fail` verdict is *returned*, not an
/// error: the process exit code is the caller's decision.
pub fn review_mission(
    inputs: &ReviewInputs,
    mode_flag: Option<&str>,
) -> Result<MissionReviewReport, MissionReviewError> {
    let (mode, _auto) = resolve_mode(mode_flag, inputs.baseline_merge_commit.as_deref())?;
    let mut findings: Vec<ReviewFinding> = Vec::new();
    let mut gates: Vec<GateRecord> = Vec::new();
    let native_gaps: Vec<String> = Vec::new();

    // Gate 1: work-package lanes, then rejected review artifacts.
    let findings_before = findings.len();
    let non_done: Vec<(&String, &String)> = inputs
        .work_packages
        .iter()
        .filter(|(_, lane)| lane != "done")
        .map(|(wp, lane)| (wp, lane))
        .collect();
    for (wp_id, lane) in &non_done {
        findings.push(ReviewFinding::new(
            "wp_not_done",
            &[("wp_id", (*wp_id).clone()), ("lane", (*lane).clone())],
        ));
    }
    for (wp_id, lane, artifact) in &inputs.rejected_review_artifacts {
        findings.push(ReviewFinding::new(
            "rejected_review_artifact",
            &[
                ("wp_id", wp_id.clone()),
                ("lane", lane.clone()),
                ("artifact_path", artifact.clone()),
                (
                    "diagnostic_code",
                    "MISSION_REVIEW_REJECTED_REVIEW_ARTIFACT".to_owned(),
                ),
                ("branch_or_work_package", wp_id.clone()),
                (
                    "violated_invariant",
                    "terminal_work_package_has_rejected_review".to_owned(),
                ),
                (
                    "remediation",
                    "Re-review the work package, or record an approved verdict in the status event log.".to_owned(),
                ),
            ],
        ));
    }
    gates.push(GateRecord {
        id: "gate_1",
        name: "wp_lane_check",
        result: if findings.len() > findings_before {
            "fail"
        } else {
            "pass"
        },
    });

    // Gate 2: dead-code scan.
    let outcome = scan_dead_code(inputs, &mut findings);
    gates.push(GateRecord {
        id: "gate_2",
        name: "dead_code_scan",
        result: match outcome {
            DeadCodeOutcome::NotApplicable => "skip",
            DeadCodeOutcome::Scanned { findings_added } => {
                if findings_added {
                    "fail"
                } else {
                    "pass"
                }
            }
            DeadCodeOutcome::Unscanned => "fail",
        },
    });

    // Gate 3: auth/storage BLE001 audit.
    let ble_findings = collect_ble001_findings(&inputs.repo_root);
    for finding in &ble_findings {
        findings.push(ReviewFinding::new(
            "ble001_suppression",
            &[
                ("file", finding.file.clone()),
                ("line", finding.line.to_string()),
                ("content", finding.suppression.clone()),
                ("remediation", finding.remediation.clone()),
            ],
        ));
    }
    gates.push(GateRecord {
        id: "gate_3",
        name: "ble001_audit",
        result: if ble_findings.is_empty() {
            "pass"
        } else {
            "fail"
        },
    });

    // Gate 4 / report inputs: the issue matrix.
    //
    // Source requires a matrix only when the mission DECLARES a gating issue
    // reference. The reference classifier below is that authority, so a missing
    // matrix with no gating reference is genuinely `not_applicable`, while a
    // declared gating reference without a row/verdict is a real finding.
    let matrix = inputs.feature_dir.join("issue-matrix.md");
    let matrix_json = inputs.feature_dir.join("issue-matrix.json");
    let mut issue_matrix_present = "not_applicable".to_owned();
    if mode == MissionReviewMode::PostMerge {
        let gating = crate::issue_matrix::gating_issue_numbers(&inputs.feature_dir);
        if matrix_json.is_file() {
            match crate::issue_matrix::validate_issue_matrix_json(&matrix_json) {
                Ok(rows) => {
                    issue_matrix_present = "true".to_owned();
                    for number in &gating {
                        if !rows.contains(number) {
                            findings.push(ReviewFinding::new(
                                "issue_matrix_missing",
                                &[
                                    ("issue_ref", number.clone()),
                                    (
                                        "message",
                                        format!(
                                            "gating issue {number} has no issue-matrix.json row"
                                        ),
                                    ),
                                ],
                            ));
                        }
                    }
                }
                Err(diagnostics) => {
                    issue_matrix_present = "false".to_owned();
                    for (code, message) in diagnostics {
                        findings.push(ReviewFinding::new(
                            "issue_matrix_violation",
                            &[("diagnostic_code", code), ("message", message)],
                        ));
                    }
                }
            }
        } else if matrix.is_file() {
            match validate_issue_matrix(&matrix) {
                Ok(rows) => {
                    issue_matrix_present = "true".to_owned();
                    if rows > 0 {
                        // Validated rows are evidence, not a finding.
                    }
                }
                Err(diagnostics) => {
                    issue_matrix_present = "false".to_owned();
                    for (code, message) in diagnostics {
                        findings.push(ReviewFinding::new(
                            "issue_matrix_violation",
                            &[("diagnostic_code", code), ("message", message)],
                        ));
                    }
                }
            }
        } else if !gating.is_empty() {
            issue_matrix_present = "false".to_owned();
            for number in &gating {
                findings.push(ReviewFinding::new(
                    "issue_matrix_missing",
                    &[
                        ("issue_ref", number.clone()),
                        (
                            "message",
                            format!(
                                "the mission declares gating issue {number} but no issue matrix exists"
                            ),
                        ),
                    ],
                ));
            }
        }
    }
    let mission_exception_present = if mode == MissionReviewMode::PostMerge {
        if inputs.feature_dir.join("mission-exception.md").is_file() {
            "true"
        } else {
            "false"
        }
        .to_owned()
    } else {
        "not_applicable".to_owned()
    };

    let hard_failure = findings.iter().any(ReviewFinding::is_hard_failure);
    let verdict = if hard_failure {
        "fail"
    } else if !findings.is_empty() {
        "pass_with_notes"
    } else {
        "pass"
    };

    let mut report = MissionReviewReport {
        verdict,
        mode,
        findings,
        gates,
        issue_matrix_present,
        mission_exception_present,
        native_gaps,
        report_path: inputs.feature_dir.join("mission-review-report.md"),
        report_bytes: Vec::new(),
    };
    report.write()?;
    Ok(report)
}

/// What the dead-code gate did.
enum DeadCodeOutcome {
    /// Ran to completion; `findings_added` reports whether it found symbols.
    Scanned { findings_added: bool },
    /// The change set holds nothing the Python-only scan supports.
    NotApplicable,
    /// The scan could not run (no baseline, or no discovery).
    Unscanned,
}

/// Source `scan_dead_code`, over caller-supplied evidence.
fn scan_dead_code(inputs: &ReviewInputs, findings: &mut Vec<ReviewFinding>) -> DeadCodeOutcome {
    let baseline = inputs
        .baseline_merge_commit
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let Some(_baseline) = baseline else {
        if let Some(mission_id) = &inputs.mission_id {
            let pr_accepted = inputs
                .acceptance_mode
                .as_deref()
                .map(str::trim)
                .is_some_and(|value| value.eq_ignore_ascii_case("pr"));
            let (reason, remediation) = if pr_accepted {
                (
                    "pr_accepted_merge_unrecorded",
                    PR_MERGE_UNRECORDED_REMEDIATION,
                )
            } else {
                (
                    "never_merged_via_spec_kitty_merge",
                    NEVER_MERGED_REMEDIATION,
                )
            };
            findings.push(ReviewFinding::new(
                "dead_code_baseline_missing",
                &[
                    (
                        "diagnostic_code",
                        diagnostic::LIGHTWEIGHT_MISSING_BASELINE.to_owned(),
                    ),
                    ("mission_id", mission_id.clone()),
                    ("mission_slug", inputs.mission_slug.clone()),
                    ("reason", reason.to_owned()),
                    ("remediation", remediation.to_owned()),
                ],
            ));
        }
        return DeadCodeOutcome::Unscanned;
    };

    let evidence_value = inputs
        .pr_merge_evidence
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    if !evidence_value.is_empty()
        && !matches!(
            evidence_value,
            "merge-commit-parent-attested" | "corpus-parent-attested"
        )
    {
        findings.push(ReviewFinding::new(
            "dead_code_evidence_incomplete",
            &[
                (
                    "diagnostic_code",
                    diagnostic::DEAD_CODE_EVIDENCE_INCOMPLETE.to_owned(),
                ),
                ("pr_merge_evidence", evidence_value.to_owned()),
                (
                    "remediation",
                    PR_MERGE_EVIDENCE_INCOMPLETE_REMEDIATION.to_owned(),
                ),
            ],
        ));
        return DeadCodeOutcome::Unscanned;
    }

    let Some(evidence) = &inputs.dead_code else {
        findings.push(ReviewFinding::new(
            "dead_code_undeterminable",
            &[
                (
                    "diagnostic_code",
                    diagnostic::DEAD_CODE_UNDETERMINABLE.to_owned(),
                ),
                (
                    "reason",
                    "changed-symbol discovery is unavailable for this invocation".to_owned(),
                ),
                ("remediation", UNDETERMINABLE_REMEDIATION.to_owned()),
            ],
        ));
        return DeadCodeOutcome::Unscanned;
    };
    if let Some(error) = &evidence.discovery_error {
        findings.push(ReviewFinding::new(
            "dead_code_undeterminable",
            &[
                (
                    "diagnostic_code",
                    diagnostic::DEAD_CODE_UNDETERMINABLE.to_owned(),
                ),
                ("reason", error.clone()),
                ("remediation", UNDETERMINABLE_REMEDIATION.to_owned()),
            ],
        ));
        return DeadCodeOutcome::Unscanned;
    }
    if evidence.supported_paths.is_empty() {
        let (extensions, excluded_test_paths) = evidence.unsupported_summary();
        findings.push(ReviewFinding::new(
            "dead_code_not_applicable",
            &[
                (
                    "diagnostic_code",
                    diagnostic::DEAD_CODE_NOT_APPLICABLE.to_owned(),
                ),
                ("unsupported_extensions", extensions.join(", ")),
                ("excluded_test_paths", excluded_test_paths.to_string()),
                ("remediation", NOT_APPLICABLE_REMEDIATION.to_owned()),
            ],
        ));
        return DeadCodeOutcome::NotApplicable;
    }

    let mut dead: Vec<(String, String)> = Vec::new();
    for (symbol, defined_in) in &evidence.added_symbols {
        let has_caller = evidence.corpus.iter().any(|(path, source)| {
            path != defined_in && !is_test_only_path(path) && symbol_in_source(source, symbol)
        });
        if !has_caller {
            dead.push((symbol.clone(), defined_in.clone()));
        }
    }
    let found = !dead.is_empty();
    for (symbol, file) in dead {
        findings.push(ReviewFinding::new(
            "dead_code",
            &[("symbol", symbol), ("file", file)],
        ));
    }
    DeadCodeOutcome::Scanned {
        findings_added: found,
    }
}

/// Is `symbol` referenced as a whole word in `source`?
///
/// Source's `symbol in source` substring test is preserved: a longer identifier
/// that merely contains the symbol still counts as a reference there, and this
/// scan must not become *stricter* than the reference it is ported from.
fn symbol_in_source(source: &str, symbol: &str) -> bool {
    source.contains(symbol)
}

fn is_python_path(path: &str) -> bool {
    extension_of(path) == ".py"
}

fn extension_of(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rfind('.') {
        Some(index) if index > 0 => name[index..].to_ascii_lowercase(),
        _ => "(no extension)".to_owned(),
    }
}

/// Source `_is_test_only_path`: a `test`/`tests` directory segment, or a
/// `test_*.py` / `*_test.py` / `conftest.py` file name.
pub fn is_test_only_path(path: &str) -> bool {
    let lowered = path.to_ascii_lowercase();
    let mut parts: Vec<&str> = lowered.split('/').collect();
    let name = parts.pop().unwrap_or_default();
    if parts.iter().any(|part| matches!(*part, "test" | "tests")) {
        return true;
    }
    if name == "conftest.py" {
        return true;
    }
    name.ends_with(".py") && (name.starts_with("test_") || name.ends_with("_test.py"))
}

/// A scoped auth/storage BLE001 suppression finding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ble001Finding {
    /// Repository-relative file.
    pub file: String,
    /// 1-based line number.
    pub line: usize,
    /// The suppression line (stripped).
    pub suppression: String,
    /// The recorded reason (empty for a bare broad handler).
    pub reason: String,
    /// Source remediation text.
    pub remediation: String,
}

/// Source `_BLE001_REMEDIATION`.
pub const BLE001_REMEDIATION: &str = "Add a specific safety reason after '# noqa: BLE001' that names the boundary, translation, logging, downgrade, or cleanup behavior; otherwise narrow the exception type.";

/// Files the audit is scoped to (Source `_AUTH_STORAGE_BLE001_COMMAND_FILES`).
const AUTH_STORAGE_COMMAND_FILES: &[&str] = &[
    "src/specify_cli/cli/commands/auth.py",
    "src/specify_cli/cli/commands/_auth_doctor.py",
    "src/specify_cli/cli/commands/_auth_login.py",
    "src/specify_cli/cli/commands/_auth_logout.py",
    "src/specify_cli/cli/commands/_auth_status.py",
];

/// Source `_AUTH_STORAGE_BLE001_AUTH_PREFIX`.
const AUTH_STORAGE_AUTH_PREFIX: &str = "src/specify_cli/auth/";

/// Source `_GENERIC_BLE001_REASONS`.
const GENERIC_BLE001_REASONS: &[&str] = &[
    "all exceptions",
    "ble001",
    "blanket",
    "broad catch",
    "broad exception",
    "catch all",
    "catchall",
    "exception",
    "fixme",
    "generic",
    "ignore",
    "ignored",
    "noqa",
    "suppress",
    "suppression",
    "temp",
    "temporary",
    "todo",
];

/// Collect unjustified BLE001 suppressions under the real repository root.
pub fn collect_ble001_findings(repo_root: &Path) -> Vec<Ble001Finding> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    let auth_dir = repo_root.join(AUTH_STORAGE_AUTH_PREFIX);
    if auth_dir.is_dir() {
        collect_python_files(&auth_dir, &mut candidates);
    }
    for relative in AUTH_STORAGE_COMMAND_FILES {
        let path = repo_root.join(relative);
        if path.is_file() {
            candidates.push(path);
        }
    }
    candidates.sort();
    candidates.dedup();

    let mut findings = Vec::new();
    for path in candidates {
        let Ok(bytes) = read_bounded(&path, 8 * 1024 * 1024) else {
            continue;
        };
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        let relative = path
            .strip_prefix(repo_root)
            .map(|value| value.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"));
        for (index, line) in text.lines().enumerate() {
            if let Some(finding) = audit_ble001_line(&relative, index + 1, line) {
                findings.push(finding);
            }
        }
    }
    findings
}

fn collect_python_files(directory: &Path, into: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            collect_python_files(&path, into);
        } else if path.extension().is_some_and(|value| value == "py") {
            into.push(path);
        }
    }
}

/// Audit one line (Source `audit_auth_storage_ble001_line`).
pub fn audit_ble001_line(file: &str, line_number: usize, line: &str) -> Option<Ble001Finding> {
    let normalized = file.replace('\\', "/");
    let scoped = normalized.starts_with(AUTH_STORAGE_AUTH_PREFIX)
        || AUTH_STORAGE_COMMAND_FILES
            .iter()
            .any(|candidate| *candidate == normalized);
    if !scoped {
        return None;
    }
    let reason = ble001_reason_from_line(line);
    let finding = match reason {
        None => {
            if !is_broad_exception_handler(line) {
                return None;
            }
            String::new()
        }
        Some(reason) => {
            if !is_generic_ble001_reason(&reason) {
                return None;
            }
            reason
        }
    };
    Some(Ble001Finding {
        file: normalized,
        line: line_number,
        suppression: line.trim().to_owned(),
        reason: finding,
        remediation: BLE001_REMEDIATION.to_owned(),
    })
}

/// Source `_ble001_reason_from_line`: the text after `BLE001`, minus any
/// following `, CODE123` selectors and leading decoration.
fn ble001_reason_from_line(line: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    let hash = find_noqa(&lower)?;
    let body_start = hash + find_after(&lower[hash..], "noqa:")?;
    let body = &line[body_start..];
    let ble = find_word(body, "BLE001")?;
    let after = &body[ble + "BLE001".len()..];
    // Strip `, CODE123` selectors: `\s*,\s*[A-Z]{1,4}\d{3}\b`.
    let mut rest = after;
    loop {
        let trimmed = rest.trim_start();
        if !trimmed.starts_with(',') {
            break;
        }
        let after_comma = trimmed[1..].trim_start();
        let code_len: usize = after_comma
            .chars()
            .take_while(|value| value.is_ascii_uppercase())
            .count();
        if !(1..=4).contains(&code_len) {
            break;
        }
        let digits: usize = after_comma[code_len..]
            .chars()
            .take_while(|value| value.is_ascii_digit())
            .count();
        if digits != 3 {
            break;
        }
        rest = &after_comma[code_len + digits..];
    }
    let trimmed = rest
        .trim_start_matches(|value: char| {
            value.is_whitespace() || matches!(value, '-' | '–' | '—' | ':')
        })
        .trim();
    Some(trimmed.to_owned())
}

fn find_noqa(lower: &str) -> Option<usize> {
    let mut search = 0;
    while let Some(index) = lower[search..].find('#') {
        let absolute = search + index;
        let rest = lower[absolute + 1..].trim_start();
        if rest.starts_with("noqa") {
            return Some(absolute);
        }
        search = absolute + 1;
    }
    None
}

fn find_after(text: &str, needle: &str) -> Option<usize> {
    text.find(needle).map(|index| index + needle.len())
}

fn find_word(text: &str, needle: &str) -> Option<usize> {
    let mut search = 0;
    while let Some(index) = text[search..].find(needle) {
        let absolute = search + index;
        let before_ok = absolute == 0
            || !text[..absolute]
                .chars()
                .next_back()
                .is_some_and(is_word_char);
        let after = absolute + needle.len();
        let after_ok =
            after >= text.len() || !text[after..].chars().next().is_some_and(is_word_char);
        if before_ok && after_ok {
            return Some(absolute);
        }
        search = absolute + 1;
    }
    None
}

/// Source `_is_broad_exception_handler`.
///
/// `^\s*except\s+(?:Exception(?:\s+as\s+\w+)?|\([^#\n]*\bException\b[^#\n]*\)(?:\s+as\s+\w+)?)\s*:`
fn is_broad_exception_handler(line: &str) -> bool {
    let trimmed = line.trim_start();
    let Some(rest) = trimmed.strip_prefix("except") else {
        return false;
    };
    if !rest.starts_with([' ', '\t']) {
        return false;
    }
    let rest = rest.trim_start();
    let tail = if let Some(after) = rest.strip_prefix("Exception") {
        if after
            .chars()
            .next()
            .is_some_and(|value| is_word_char(value) || value == '(')
        {
            return false;
        }
        after
    } else if rest.starts_with('(') {
        let Some(end) = rest.find(')') else {
            return false;
        };
        let inner = &rest[1..end];
        if inner.contains('#') || !contains_word(inner, "Exception") {
            return false;
        }
        &rest[end + 1..]
    } else {
        return false;
    };
    // Optional `as <name>`, then the colon.
    let tail = tail.trim_start();
    let tail = match tail.strip_prefix("as") {
        Some(after) if after.starts_with([' ', '\t']) => {
            let after = after.trim_start();
            let name_len: usize = after
                .chars()
                .take_while(|value| is_word_char(*value))
                .map(char::len_utf8)
                .sum();
            if name_len == 0 {
                return false;
            }
            &after[name_len..]
        }
        _ => tail,
    };
    tail.trim_start().starts_with(':')
}

fn contains_word(text: &str, needle: &str) -> bool {
    let mut search = 0;
    while let Some(index) = text[search..].find(needle) {
        let absolute = search + index;
        let before_ok = absolute == 0
            || !text[..absolute]
                .chars()
                .next_back()
                .is_some_and(is_word_char);
        let after = absolute + needle.len();
        let after_ok =
            after >= text.len() || !text[after..].chars().next().is_some_and(is_word_char);
        if before_ok && after_ok {
            return true;
        }
        search = absolute + 1;
    }
    false
}

/// Source `_is_generic_ble001_reason`: punctuation/case-insensitive comparison
/// against the generic set.
fn is_generic_ble001_reason(reason: &str) -> bool {
    let mut normalized = String::new();
    for value in reason.chars() {
        if value.is_alphanumeric() {
            normalized.extend(value.to_lowercase());
        } else if !normalized.ends_with(' ') {
            normalized.push(' ');
        }
    }
    let normalized = normalized.trim().to_owned();
    normalized.is_empty() || GENERIC_BLE001_REASONS.contains(&normalized.as_str())
}

fn is_word_char(value: char) -> bool {
    value.is_alphanumeric() || value == '_'
}

/// The allowed issue-matrix verdicts (Source `IssueMatrixVerdict`).
pub use crate::issue_matrix::ISSUE_MATRIX_VERDICTS;

/// Validate the markdown issue matrix, returning the row count.
///
/// The rules are the Source markdown-validator rules: exactly one table, at
/// least a header and a separator row, the mandatory `issue`/`verdict`/
/// `evidence_ref` columns present, no column outside the mandatory + named
/// optional vocabulary, a non-empty `evidence_ref`, a verdict in the closed set,
/// and a follow-up handle on a `deferred-with-followup` row. Diagnostics are
/// `(code, message)` pairs; they are never collapsed into one generic code.
pub fn validate_issue_matrix(path: &Path) -> Result<usize, Vec<(String, String)>> {
    let bytes = read_bounded(path, 4 * 1024 * 1024).map_err(|error| {
        vec![(
            diagnostic::ISSUE_MATRIX_INVALID.to_owned(),
            format!("could not read {}: {error}", path.display()),
        )]
    })?;
    let text = String::from_utf8(bytes).map_err(|error| {
        vec![(
            diagnostic::ISSUE_MATRIX_INVALID.to_owned(),
            format!("{} is not valid UTF-8: {error}", path.display()),
        )]
    })?;
    validate_issue_matrix_text(&text)
}

/// Named optional issue-matrix columns (Source `NAMED_OPTIONAL_COLUMNS`).
pub const ISSUE_MATRIX_OPTIONAL_COLUMNS: &[&str] =
    &["title", "scope", "wp", "fr", "nfr", "sc", "repo"];

/// Validate issue-matrix markdown without touching the filesystem.
pub fn validate_issue_matrix_text(text: &str) -> Result<usize, Vec<(String, String)>> {
    let lines: Vec<&str> = text.lines().collect();
    let mut diagnostics: Vec<(String, String)> = Vec::new();
    let mut tables: Vec<(usize, usize)> = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        if !looks_like_table_row(lines[index]) {
            index += 1;
            continue;
        }
        let start = index;
        while index < lines.len() && looks_like_table_row(lines[index]) {
            index += 1;
        }
        tables.push((start, index));
    }
    if tables.is_empty() {
        return Err(vec![(
            diagnostic::ISSUE_MATRIX_MISSING.to_owned(),
            "issue matrix contains no markdown table".to_owned(),
        )]);
    }
    if tables.len() > 1 {
        return Err(vec![(
            diagnostic::ISSUE_MATRIX_MULTI_TABLE.to_owned(),
            format!(
                "issue matrix contains {} markdown tables; exactly one is allowed",
                tables.len()
            ),
        )]);
    }
    let (start, end) = tables[0];
    let block = &lines[start..end];
    if block.len() < 2 {
        return Err(vec![(
            diagnostic::ISSUE_MATRIX_SCHEMA_DRIFT.to_owned(),
            "issue matrix table has fewer than 2 rows (header + separator expected)".to_owned(),
        )]);
    }
    let canonical: Vec<String> = split_cells(block[0])
        .iter()
        .map(|cell| normalize_header(cell))
        .collect();
    let mandatory = ["issue", "verdict", "evidence_ref"];
    let missing: Vec<&str> = mandatory
        .iter()
        .copied()
        .filter(|name| !canonical.iter().any(|header| header == name))
        .collect();
    if !missing.is_empty() {
        diagnostics.push((
            diagnostic::ISSUE_MATRIX_SCHEMA_DRIFT.to_owned(),
            format!("mandatory column(s) missing: {}", missing.join(", ")),
        ));
        return Err(diagnostics);
    }
    let unknown: Vec<&String> = canonical
        .iter()
        .filter(|header| {
            !mandatory.contains(&header.as_str())
                && !ISSUE_MATRIX_OPTIONAL_COLUMNS.contains(&header.as_str())
        })
        .collect();
    if !unknown.is_empty() {
        diagnostics.push((
            diagnostic::ISSUE_MATRIX_SCHEMA_DRIFT.to_owned(),
            format!(
                "unknown column(s) not in the mandatory or named-optional vocabulary: {}",
                unknown
                    .iter()
                    .map(|value| value.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }
    let column_of = |name: &str| canonical.iter().position(|header| header == name);
    let mut rows = 0usize;
    for raw_line in block.iter().skip(2) {
        if !looks_like_table_row(raw_line) || is_separator_row(raw_line) {
            continue;
        }
        let cells = split_cells(raw_line);
        for cell in &cells {
            if cell.contains("<<<<<<<") || cell.contains(">>>>>>>") || cell.contains("=======") {
                diagnostics.push((
                    diagnostic::ISSUE_MATRIX_INVALID.to_owned(),
                    "row carries a merge-conflict marker".to_owned(),
                ));
            }
        }
        let cell = |name: &str| -> String {
            column_of(name)
                .and_then(|position| cells.get(position))
                .map(|value| strip_backticks(value))
                .unwrap_or_default()
        };
        let issue = cell("issue");
        let verdict = cell("verdict").trim().to_lowercase();
        let evidence = cell("evidence_ref");
        if evidence.is_empty() {
            diagnostics.push((
                diagnostic::ISSUE_MATRIX_EVIDENCE_EMPTY.to_owned(),
                format!("row for issue {issue:?}: evidence_ref is empty"),
            ));
        }
        if !ISSUE_MATRIX_VERDICTS.contains(&verdict.as_str()) {
            diagnostics.push((
                diagnostic::ISSUE_MATRIX_VERDICT_UNKNOWN.to_owned(),
                format!("row for issue {issue:?}: verdict {verdict:?} is not in the allowed set"),
            ));
        } else {
            if verdict == "deferred-with-followup" && !has_followup_handle(&evidence) {
                diagnostics.push((
                    diagnostic::ISSUE_MATRIX_DEFERRED_WITHOUT_HANDLE.to_owned(),
                    format!(
                        "row for issue {issue:?}: verdict is deferred-with-followup but evidence_ref contains no follow-up handle (expected '#NNN' or 'Follow-up:')"
                    ),
                ));
            }
            rows += 1;
        }
    }
    if diagnostics.is_empty() {
        Ok(rows)
    } else {
        Err(diagnostics)
    }
}

/// Source deferred-row rule: `#\d+` anywhere, or the `Follow-up:` marker.
fn has_followup_handle(evidence: &str) -> bool {
    evidence.contains("Follow-up:") || contains_hash_number(evidence)
}

fn contains_hash_number(text: &str) -> bool {
    let bytes: Vec<char> = text.chars().collect();
    for (index, value) in bytes.iter().enumerate() {
        if *value == '#' {
            let digits = bytes[index + 1..]
                .iter()
                .take_while(|next| next.is_ascii_digit())
                .count();
            if digits > 0 {
                return true;
            }
        }
    }
    false
}

/// Source `_strip_backticks`.
fn strip_backticks(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('`') && trimmed.ends_with('`') {
        return trimmed[1..trimmed.len() - 1].to_owned();
    }
    trimmed.to_owned()
}

fn looks_like_table_row(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('|') && trimmed.len() >= 2
}

fn is_separator_row(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('|')
        && trimmed
            .chars()
            .all(|value| matches!(value, '|' | '-' | ':' | ' ' | '\t'))
}

fn split_cells(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    let mut parts: Vec<String> = trimmed
        .split('|')
        .map(|cell| cell.trim().to_owned())
        .collect();
    if parts.first().is_some_and(String::is_empty) {
        parts.remove(0);
    }
    if parts.last().is_some_and(String::is_empty) {
        parts.pop();
    }
    parts
}

fn normalize_header(cell: &str) -> String {
    let lowered = cell.trim().to_lowercase();
    match lowered.as_str() {
        "evidence ref" => "evidence_ref",
        "wp_id" => "wp",
        other => other,
    }
    .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, text).unwrap();
    }

    fn base_inputs() -> (tempfile::TempDir, ReviewInputs) {
        let temp = tempfile::tempdir().unwrap();
        let repo_root = temp.path().to_path_buf();
        let feature_dir = repo_root.join("kitty-specs/demo");
        std::fs::create_dir_all(&feature_dir).unwrap();
        write(&feature_dir.join("tasks.md"), "- [x] T001 done\n");
        let inputs = ReviewInputs {
            repo_root,
            feature_dir,
            mission_slug: "demo".to_owned(),
            mission_id: Some("01KN2371WNZX28ME4MN97B67PJ".to_owned()),
            friendly_name: Some("Demo".to_owned()),
            baseline_merge_commit: None,
            acceptance_mode: None,
            pr_merge_evidence: None,
            work_packages: vec![("WP01".to_owned(), "done".to_owned())],
            rejected_review_artifacts: Vec::new(),
            dead_code: None,
        };
        (temp, inputs)
    }

    #[test]
    fn mode_resolution_follows_source_precedence() {
        assert_eq!(
            resolve_mode(None, Some("abc")).unwrap(),
            (MissionReviewMode::PostMerge, true)
        );
        assert_eq!(
            resolve_mode(None, None).unwrap(),
            (MissionReviewMode::Lightweight, true)
        );
        assert_eq!(
            resolve_mode(Some("lightweight"), Some("abc")).unwrap(),
            (MissionReviewMode::Lightweight, false)
        );
        assert_eq!(
            resolve_mode(Some("post-merge"), Some("abc")).unwrap(),
            (MissionReviewMode::PostMerge, false)
        );
        assert_eq!(
            resolve_mode(Some("post-merge"), None).unwrap_err(),
            MissionReviewError::ModeMismatch
        );
    }

    #[test]
    fn lightweight_review_notes_a_missing_baseline_without_passing_the_gate() {
        let (temp, inputs) = base_inputs();
        write(&inputs.feature_dir.join("spec.md"), "# Spec\n");
        let report = review_mission(&inputs, Some("lightweight")).unwrap();
        assert_eq!(report.verdict, "fail");
        assert!(
            report
                .findings
                .iter()
                .any(|finding| finding.kind == "dead_code_baseline_missing")
        );
        assert!(
            temp.path()
                .join("kitty-specs/demo/mission-review-report.md")
                .is_file()
        );
    }

    #[test]
    fn a_non_done_work_package_fails_gate_one() {
        let (temp, mut inputs) = base_inputs();
        inputs.work_packages = vec![("WP01".to_owned(), "for_review".to_owned())];
        let report = review_mission(&inputs, Some("lightweight")).unwrap();
        assert_eq!(report.verdict, "fail");
        assert_eq!(report.gates[0].result, "fail");
        let text = String::from_utf8(report.report_bytes.clone()).unwrap();
        assert!(text.contains("**wp_not_done** `WP01`: lane is `for_review`"));
        drop(temp);
    }

    #[test]
    fn a_post_merge_scan_evidence_set_produces_a_real_verdict() {
        let (temp, mut inputs) = base_inputs();
        inputs.baseline_merge_commit = Some("a".repeat(40));
        write(&inputs.feature_dir.join("spec.md"), "# Spec\n");
        inputs.dead_code = Some(DeadCodeEvidence {
            changed_paths: vec!["src/demo.py".to_owned()],
            supported_paths: vec!["src/demo.py".to_owned()],
            added_symbols: vec![("used_symbol".to_owned(), "src/demo.py".to_owned())],
            corpus: vec![
                (
                    "src/demo.py".to_owned(),
                    "def used_symbol():\n    pass\n".to_owned(),
                ),
                ("src/caller.py".to_owned(), "used_symbol()\n".to_owned()),
            ],
            discovery_error: None,
        });
        let report = review_mission(&inputs, None).unwrap();
        assert_eq!(report.mode, MissionReviewMode::PostMerge);
        assert_eq!(report.verdict, "pass");
        assert_eq!(report.gates[1].result, "pass");
        drop(temp);
    }

    #[test]
    fn an_unreferenced_symbol_is_a_dead_code_finding() {
        let (_temp, mut inputs) = base_inputs();
        inputs.baseline_merge_commit = Some("a".repeat(40));
        inputs.dead_code = Some(DeadCodeEvidence {
            changed_paths: vec!["src/demo.py".to_owned()],
            supported_paths: vec!["src/demo.py".to_owned()],
            added_symbols: vec![("orphan".to_owned(), "src/demo.py".to_owned())],
            corpus: vec![(
                "src/demo.py".to_owned(),
                "def orphan():\n    pass\n".to_owned(),
            )],
            discovery_error: None,
        });
        let report = review_mission(&inputs, None).unwrap();
        // Source's hard-failure set does NOT include a `dead_code` finding: the
        // gate records `fail` while the mission verdict is `pass_with_notes`.
        // This port keeps that exact asymmetry.
        assert_eq!(report.gates[1].result, "fail");
        assert_eq!(report.verdict, "pass_with_notes");
        assert!(report.findings.iter().any(
            |finding| finding.kind == "dead_code" && finding.field("symbol") == Some("orphan")
        ));
    }

    #[test]
    fn a_non_python_change_set_is_not_applicable_and_skips() {
        let (_temp, mut inputs) = base_inputs();
        inputs.baseline_merge_commit = Some("a".repeat(40));
        inputs.dead_code = Some(DeadCodeEvidence {
            changed_paths: vec!["docs/readme.md".to_owned(), "tests/test_demo.py".to_owned()],
            supported_paths: Vec::new(),
            added_symbols: Vec::new(),
            corpus: Vec::new(),
            discovery_error: None,
        });
        let report = review_mission(&inputs, None).unwrap();
        assert_eq!(report.gates[1].result, "skip");
        assert_eq!(report.verdict, "pass_with_notes");
    }

    #[test]
    fn a_missing_evidence_set_is_undeterminable_not_green() {
        let (_temp, mut inputs) = base_inputs();
        inputs.baseline_merge_commit = Some("a".repeat(40));
        inputs.dead_code = None;
        let report = review_mission(&inputs, None).unwrap();
        assert_eq!(report.gates[1].result, "fail");
        assert_eq!(report.verdict, "fail");
        assert!(
            report
                .findings
                .iter()
                .any(|finding| finding.kind == "dead_code_undeterminable")
        );
    }

    #[test]
    fn an_incomplete_pr_anchor_refuses_to_scan() {
        let (_temp, mut inputs) = base_inputs();
        inputs.baseline_merge_commit = Some("a".repeat(40));
        inputs.pr_merge_evidence = Some("guess".to_owned());
        let report = review_mission(&inputs, None).unwrap();
        assert!(
            report
                .findings
                .iter()
                .any(|finding| finding.kind == "dead_code_evidence_incomplete")
        );
        assert_eq!(report.verdict, "fail");
    }

    #[test]
    fn ble001_audit_reads_real_scoped_files() {
        let temp = tempfile::tempdir().unwrap();
        write(
            &temp.path().join("src/specify_cli/auth/store.py"),
            "try:\n    pass\nexcept Exception:  # noqa: BLE001\n    pass\nexcept ValueError:  # noqa: BLE001 - narrow, specific reason\n    pass\n",
        );
        let findings = collect_ble001_findings(temp.path());
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].line, 3);
        assert_eq!(findings[0].reason, "");
    }

    #[test]
    fn ble001_ignores_unscoped_files_and_justified_reasons() {
        let temp = tempfile::tempdir().unwrap();
        write(
            &temp.path().join("src/specify_cli/status/store.py"),
            "except Exception:  # noqa: BLE001\n",
        );
        assert!(collect_ble001_findings(temp.path()).is_empty());
        assert_eq!(
            ble001_reason_from_line(
                "except Exception:  # noqa: BLE001, E501 - clean up on the socket boundary"
            ),
            Some("clean up on the socket boundary".to_owned())
        );
        assert!(is_generic_ble001_reason("TODO"));
        assert!(is_generic_ble001_reason("Broad-Exception!"));
        assert!(!is_generic_ble001_reason("clean up on the socket boundary"));
    }

    #[test]
    fn issue_matrix_validation_refuses_unknown_verdicts() {
        let ok = "| Issue | Verdict | Evidence Ref |\n|---|---|---|\n| #1 | fixed | abc123 |\n";
        assert_eq!(validate_issue_matrix_text(ok).unwrap(), 1);
        let bad = "| Issue | Verdict | Evidence Ref |\n|---|---|---|\n| #1 | maybe | abc123 |\n";
        assert!(validate_issue_matrix_text(bad).is_err());
        let missing = "| Issue | Verdict |\n|---|---|\n| #1 | fixed |\n";
        assert!(validate_issue_matrix_text(missing).is_err());
    }

    #[test]
    fn the_report_frontmatter_matches_the_source_shape() {
        let (_temp, inputs) = base_inputs();
        let report = review_mission(&inputs, Some("lightweight")).unwrap();
        let text = String::from_utf8(report.report_bytes.clone()).unwrap();
        assert!(text.starts_with("---\nverdict: fail\nmode: lightweight\nreviewed_at: "));
        assert!(text.contains("gates_recorded:\n  - id: gate_1\n    name: wp_lane_check\n    command: spec-kitty review (internal gate 1)\n    exit_code: 0\n    result: pass"));
        assert!(text.contains("issue_matrix_present: not_applicable"));
        assert!(text.contains("mission_exception_present: not_applicable"));
        assert!(text.contains("## Findings"));
    }
}
