//! Canonical acceptance: gate evaluation, review-evidence population, and the
//! derived `acceptance-matrix.json` artifact.
//!
//! This module is the Rust projection of the Source acceptance surface that
//! `spec-kitty accept` relies on:
//!
//! * [`AcceptanceCriterion`] / [`AcceptanceMatrix`] keep the Source field set
//!   and the Source `overall_verdict` dominance rule (pending dominates pass,
//!   cancellation/displacement invariants fail, nothing tracked stays pending);
//! * [`review_evidence_population`] is the all-or-nothing
//!   review-evidence derivation: every tracked work package must be in
//!   `approved`/`done` **and** carry an event-sourced `approved` verdict,
//!   otherwise no row is touched;
//! * [`evaluate_acceptance`] runs the gates: lanes manifest, terminal work
//!   packages, independent review evidence, owned-file attribution, the
//!   `tasks.md` checkbox gate (WPs-terminal normalization), and the matrix
//!   verdict;
//! * [`iter_unchecked_subtask_rows`] is the fence-aware, `T###`-scoped
//!   unchecked-subtask reader shared with the task-authoring surface.
//!
//! Everything here is pure over caller-supplied evidence except
//! [`AcceptanceMatrix::write_to`], which is the one atomic writer. Nothing here
//! invents a verdict: a missing review, a non-independent reviewer, an
//! unattributed file, or an absent lane manifest is a failing gate, never a
//! silent pass.
//!
//! # Two distinct evaluation surfaces
//!
//! [`evaluate_acceptance`] is the **Native** capability surface described above
//! (including its owned-file attribution gate and the leaner input set). It is
//! deliberately *not* Source policy parity and must never be the default of a
//! Source-compatible `accept` run. [`source`] is the ONE Source-compatible
//! authority (`accept` strict/lenient semantics, the full Source gate set, the
//! Source JSON shape); see that module's documentation.

pub mod source;

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use sk_kernel::atomic_write;
use sk_kernel::python_json::PythonJsonValue;

use crate::actor_identity::actor_identities_independent;

/// The one acceptance-matrix artifact name.
pub const ACCEPTANCE_MATRIX_FILE_NAME: &str = "acceptance-matrix.json";

/// `pass` verdict value.
pub const VERDICT_PASS: &str = "pass";
/// `fail` verdict value.
pub const VERDICT_FAIL: &str = "fail";
/// `pending` verdict value (no evidence yet — never a pass).
pub const VERDICT_PENDING: &str = "pending";

/// The fourth `overall_verdict` value: one or more negative invariants are
/// deferred to post-consolidation verification.
///
/// Acceptance is NOT blocked by a deferral, but the verdict stays
/// distinguishable from a clean `pass` so the mission cannot reach `done` while
/// a deferral is outstanding (Source `matrix.VERDICT_PASS_PENDING_CONSOLIDATION`).
pub const VERDICT_PASS_PENDING_CONSOLIDATION: &str = "pass_pending_consolidation";

/// The empty `finalize-tasks` placeholder row's `description` marker.
///
/// Load-bearing in two directions (Source `matrix.SCAFFOLD_TODO_MARKER`): the
/// scaffold writer stamps it into a contentless placeholder criterion, and
/// [`AcceptanceMatrix::overall_verdict`] reads it back to exempt ONLY that
/// placeholder from the pending-dominates rule. A rename must touch both.
pub const SCAFFOLD_TODO_MARKER: &str = "TODO: replace with a real acceptance criterion";

/// The attribution recorded for operator-controlled evidence (a cancellation
/// attestation), never presented as a reviewer identity.
pub const OPERATOR_EVIDENCE_ACTOR: &str = "operator:attestation";

/// A single acceptance criterion with evidence (Source `AcceptanceCriterion`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AcceptanceCriterion {
    /// Stable criterion id.
    pub criterion_id: String,
    /// Human description.
    pub description: String,
    /// `automated_test` | `manual_qa` | `code_review` | `negative_invariant`.
    pub proof_type: String,
    /// Evidence reference, when judged.
    pub evidence: Option<String>,
    /// `pass` | `fail` | `pending`.
    pub pass_fail: String,
    /// Independent reviewers that produced the evidence.
    pub verified_by: Option<String>,
    /// ISO timestamp of the verification.
    pub verified_at: Option<String>,
    /// Free notes.
    pub notes: Option<String>,
    /// Unknown keys the document carried (Source `extras`, re-emitted last).
    pub extras: Map<String, Value>,
}

impl AcceptanceCriterion {
    /// A pending `code_review` criterion, the only auto-derivable kind.
    pub fn code_review(criterion_id: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            criterion_id: criterion_id.into(),
            description: description.into(),
            proof_type: "code_review".to_owned(),
            evidence: None,
            pass_fail: VERDICT_PENDING.to_owned(),
            verified_by: None,
            verified_at: None,
            notes: None,
            extras: Map::new(),
        }
    }

    /// Is this row auto-derivable from review evidence?
    pub fn is_auto_derivable(&self) -> bool {
        self.proof_type == "code_review"
    }

    /// Read one Source `AcceptanceCriterion.from_dict` row.
    pub fn from_json(object: &Map<String, Value>) -> Self {
        let text = |key: &str| object.get(key).and_then(Value::as_str).map(str::to_owned);
        let known = [
            "criterion_id",
            "description",
            "proof_type",
            "evidence",
            "pass_fail",
            "verified_by",
            "verified_at",
            "notes",
        ];
        Self {
            criterion_id: text("criterion_id").unwrap_or_default(),
            description: text("description").unwrap_or_default(),
            proof_type: text("proof_type").unwrap_or_default(),
            evidence: text("evidence"),
            pass_fail: text("pass_fail").unwrap_or_else(|| VERDICT_PENDING.to_owned()),
            verified_by: text("verified_by"),
            verified_at: text("verified_at"),
            notes: text("notes"),
            extras: extras_of(object, &known),
        }
    }

    /// The Source `to_dict` shape (all eight declared fields, `extras` last).
    pub fn to_json(&self) -> Value {
        let mut map = Map::new();
        map.insert("criterion_id".into(), json!(self.criterion_id));
        map.insert("description".into(), json!(self.description));
        map.insert("proof_type".into(), json!(self.proof_type));
        map.insert("evidence".into(), json!(self.evidence));
        map.insert("pass_fail".into(), json!(self.pass_fail));
        map.insert("verified_by".into(), json!(self.verified_by));
        map.insert("verified_at".into(), json!(self.verified_at));
        map.insert("notes".into(), json!(self.notes));
        for (key, value) in &self.extras {
            map.insert(key.clone(), value.clone());
        }
        Value::Object(map)
    }
}

/// Terminal negative-invariant results (Source `TERMINAL_INVARIANT_RESULTS`).
///
/// Membership is the NI-2 preservation guard: a terminal result is never
/// re-judged, while `deferred_to_consolidation` is NOT terminal and must stay
/// re-judgeable at post-consolidation.
pub const TERMINAL_INVARIANT_RESULTS: [&str; 3] =
    ["confirmed_absent", "still_present", "verification_error"];
/// The scheduled-not-yet-judged state (NI-4).
pub const DEFERRED_TO_CONSOLIDATION: &str = "deferred_to_consolidation";
/// The phase name a deferred invariant is judged at (NI-4 / C4).
pub const POST_CONSOLIDATION_PHASE_NAME: &str = "POST_CONSOLIDATION";
/// `recorded` provenance origin: the judgement names its surface and ref.
pub const PROVENANCE_RECORDED: &str = "recorded";
/// The FR-014 sentinel for pre-schema judgements (omitted on write).
pub const PROVENANCE_LEGACY_UNRECORDED: &str = "legacy_unrecorded";
/// Every legal provenance origin.
pub const PROVENANCE_ORIGINS: [&str; 2] = [PROVENANCE_RECORDED, PROVENANCE_LEGACY_UNRECORDED];

/// A negative invariant row (Source `NegativeInvariant`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NegativeInvariant {
    /// Stable invariant id.
    pub invariant_id: String,
    /// Human description.
    pub description: String,
    /// `grep_absence` | `route_check` | `custom_command`.
    pub verification_method: String,
    /// Pattern (`grep_absence`) or command (`custom_command`).
    pub verification_command: Option<String>,
    /// `confirmed_absent` | `still_present` | `verification_error` | `pending` |
    /// `deferred_to_consolidation`.
    pub result: String,
    /// Evidence text.
    pub evidence: Option<String>,
    /// Optional whitespace-separated repo-relative roots for `grep_absence`.
    pub scope: Option<String>,
    /// The git ref the outcome was established against (NI-1).
    pub verified_ref: Option<String>,
    /// The topology surface value that established the outcome (NI-1).
    pub verified_surface_kind: Option<String>,
    /// Why judgement was postponed (NI-4).
    pub deferred_reason: Option<String>,
    /// The phase the deferral will be judged at (NI-4).
    pub deferred_to_phase: Option<String>,
    /// `recorded` | `legacy_unrecorded`.
    pub provenance_origin: String,
    /// Unknown keys the document carried (Source `extras`, re-emitted last).
    pub extras: Map<String, Value>,
}

impl NegativeInvariant {
    /// A `pending` row for `invariant_id` with the given method/command.
    pub fn pending(
        invariant_id: impl Into<String>,
        description: impl Into<String>,
        verification_method: impl Into<String>,
        verification_command: Option<String>,
    ) -> Self {
        Self {
            invariant_id: invariant_id.into(),
            description: description.into(),
            verification_method: verification_method.into(),
            verification_command,
            result: VERDICT_PENDING.to_owned(),
            evidence: None,
            scope: None,
            verified_ref: None,
            verified_surface_kind: None,
            deferred_reason: None,
            deferred_to_phase: None,
            provenance_origin: PROVENANCE_LEGACY_UNRECORDED.to_owned(),
            extras: Map::new(),
        }
    }

    /// Is this a terminal judgement (the NI-2 preservation set)?
    pub fn is_terminal(&self) -> bool {
        TERMINAL_INVARIANT_RESULTS.contains(&self.result.as_str())
    }

    /// Read one Source `NegativeInvariant.from_dict` row.
    pub fn from_json(object: &Map<String, Value>) -> Self {
        let text = |key: &str| object.get(key).and_then(Value::as_str).map(str::to_owned);
        let known = [
            "invariant_id",
            "description",
            "verification_method",
            "verification_command",
            "result",
            "evidence",
            "scope",
            "verified_ref",
            "verified_surface_kind",
            "deferred_reason",
            "deferred_to_phase",
            "provenance_origin",
        ];
        Self {
            invariant_id: text("invariant_id").unwrap_or_default(),
            description: text("description").unwrap_or_default(),
            verification_method: text("verification_method").unwrap_or_default(),
            verification_command: text("verification_command"),
            result: text("result").unwrap_or_else(|| VERDICT_PENDING.to_owned()),
            evidence: text("evidence"),
            scope: text("scope"),
            verified_ref: text("verified_ref"),
            verified_surface_kind: text("verified_surface_kind"),
            deferred_reason: text("deferred_reason"),
            deferred_to_phase: text("deferred_to_phase"),
            provenance_origin: text("provenance_origin")
                .unwrap_or_else(|| PROVENANCE_LEGACY_UNRECORDED.to_owned()),
            extras: extras_of(object, &known),
        }
    }

    /// The Source `to_dict` shape: the five unset optionals are omitted while
    /// `None`, `legacy_unrecorded` is omitted, and `extras` come last.
    pub fn to_json(&self) -> Value {
        let mut map = Map::new();
        map.insert("invariant_id".into(), json!(self.invariant_id));
        map.insert("description".into(), json!(self.description));
        map.insert(
            "verification_method".into(),
            json!(self.verification_method),
        );
        map.insert(
            "verification_command".into(),
            json!(self.verification_command),
        );
        map.insert("result".into(), json!(self.result));
        map.insert("evidence".into(), json!(self.evidence));
        for (key, value) in [
            ("scope", &self.scope),
            ("verified_ref", &self.verified_ref),
            ("verified_surface_kind", &self.verified_surface_kind),
            ("deferred_reason", &self.deferred_reason),
            ("deferred_to_phase", &self.deferred_to_phase),
        ] {
            if let Some(value) = value {
                map.insert(key.into(), json!(value));
            }
        }
        if self.provenance_origin != PROVENANCE_LEGACY_UNRECORDED {
            map.insert("provenance_origin".into(), json!(self.provenance_origin));
        }
        for (key, value) in &self.extras {
            map.insert(key.clone(), value.clone());
        }
        Value::Object(map)
    }
}

fn extras_of(object: &Map<String, Value>, known: &[&str]) -> Map<String, Value> {
    object
        .iter()
        .filter(|(key, _)| !known.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// The derived acceptance matrix (Source `AcceptanceMatrix`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AcceptanceMatrix {
    /// Mission slug written into the artifact.
    pub mission_slug: String,
    /// Mission number as a string, when the mission has one.
    pub mission_number: Option<String>,
    /// Mission type.
    pub mission_type: Option<String>,
    /// Criterion rows.
    pub criteria: Vec<AcceptanceCriterion>,
    /// Negative invariant rows.
    pub negative_invariants: Vec<NegativeInvariant>,
}

impl AcceptanceMatrix {
    /// A matrix with the given rows.
    pub fn new(mission_slug: impl Into<String>, criteria: Vec<AcceptanceCriterion>) -> Self {
        Self {
            mission_slug: mission_slug.into(),
            mission_number: None,
            mission_type: None,
            criteria,
            negative_invariants: Vec::new(),
        }
    }

    /// The Source `overall_verdict` rule, row for row.
    pub fn overall_verdict(&self) -> &'static str {
        let criterion_results: Vec<&str> = self
            .criteria
            .iter()
            .map(|row| row.pass_fail.as_str())
            .collect();
        let invariant_results: Vec<&str> = self
            .negative_invariants
            .iter()
            .map(|row| row.result.as_str())
            .collect();
        if criterion_results.is_empty() && invariant_results.is_empty() {
            return VERDICT_PENDING;
        }
        let allowed_criterion =
            |value: &str| matches!(value, VERDICT_PASS | VERDICT_FAIL | VERDICT_PENDING);
        let allowed_invariant = |value: &str| {
            TERMINAL_INVARIANT_RESULTS.contains(&value)
                || matches!(value, VERDICT_PENDING | DEFERRED_TO_CONSOLIDATION)
        };
        if criterion_results
            .iter()
            .any(|value| !allowed_criterion(value))
            || invariant_results
                .iter()
                .any(|value| !allowed_invariant(value))
        {
            return VERDICT_FAIL;
        }
        if criterion_results.contains(&VERDICT_FAIL)
            || invariant_results
                .iter()
                .any(|value| matches!(*value, "still_present" | "verification_error"))
        {
            return VERDICT_FAIL;
        }
        // A deferred invariant is neither a clean pass nor a failure; it is
        // decided AFTER pending so an unverified criterion still dominates.
        //
        // Pending-dominance uses Source's scaffold exclusion: the contentless
        // `finalize-tasks` placeholder row stops dominating once a REAL criterion
        // has been authored, otherwise every scaffolded mission would be stuck
        // `pending` forever. When every row is an empty placeholder (or there
        // are none), the unfiltered results dominate and the verdict stays
        // `pending`.
        let pending_dominant: Vec<&str> = {
            let real: Vec<&str> = self
                .criteria
                .iter()
                .filter(|row| row.description != SCAFFOLD_TODO_MARKER)
                .map(|row| row.pass_fail.as_str())
                .collect();
            if real.is_empty() {
                criterion_results.clone()
            } else {
                real
            }
        };
        if pending_dominant.contains(&VERDICT_PENDING) || invariant_results.contains(&"pending") {
            return VERDICT_PENDING;
        }
        if invariant_results.contains(&"deferred_to_consolidation") {
            return VERDICT_PASS_PENDING_CONSOLIDATION;
        }
        VERDICT_PASS
    }

    /// The Source `to_dict` shape.
    pub fn to_json(&self) -> Value {
        let mut map = Map::new();
        map.insert("mission_slug".into(), json!(self.mission_slug));
        map.insert(
            "mission_number".into(),
            match &self.mission_number {
                Some(number) => json!(number),
                None => Value::Null,
            },
        );
        map.insert(
            "mission_type".into(),
            match &self.mission_type {
                Some(kind) => json!(kind),
                None => Value::Null,
            },
        );
        map.insert("overall_verdict".into(), json!(self.overall_verdict()));
        map.insert(
            "criteria".into(),
            Value::Array(
                self.criteria
                    .iter()
                    .map(AcceptanceCriterion::to_json)
                    .collect(),
            ),
        );
        map.insert(
            "negative_invariants".into(),
            Value::Array(
                self.negative_invariants
                    .iter()
                    .map(NegativeInvariant::to_json)
                    .collect(),
            ),
        );
        Value::Object(map)
    }

    /// Deterministic pretty bytes (Source: `json.dumps(..., indent=2) + "\n"`).
    ///
    /// Rendered through Kernel's single `json.dumps` encoder, so the non-ASCII
    /// escaping is CPython's (`ensure_ascii=True`), not `serde_json`'s raw UTF-8.
    pub fn to_bytes(&self) -> Vec<u8> {
        let document = self.to_json();
        let object = document
            .as_object()
            .expect("an acceptance matrix always serializes to an object");
        let rendered = ordered_document(object)
            .render_with(Some(2), true)
            .expect("ensure_ascii=True never fails");
        let mut bytes = rendered.into_bytes();
        bytes.push(b'\n');
        bytes
    }

    /// Write `acceptance-matrix.json`, atomically, with readback verification.
    ///
    /// The Source writer routes through the kernel atomic writer so no reader
    /// ever observes a torn file; the readback here additionally proves the
    /// bytes that landed are the bytes that were judged.
    pub fn write_to(&self, feature_dir: &Path) -> std::io::Result<PathBuf> {
        let path = feature_dir.join(ACCEPTANCE_MATRIX_FILE_NAME);
        let bytes = self.to_bytes();
        atomic_write(&path, &bytes).map_err(|error| {
            std::io::Error::other(format!("could not write {}: {error}", path.display()))
        })?;
        let landed = std::fs::read(&path)?;
        if landed != bytes {
            return Err(std::io::Error::other(format!(
                "acceptance matrix readback at {} does not match the judged bytes",
                path.display()
            )));
        }
        Ok(path)
    }

    /// Path the artifact would be written to.
    pub fn path_in(feature_dir: &Path) -> PathBuf {
        feature_dir.join(ACCEPTANCE_MATRIX_FILE_NAME)
    }

    /// Read a matrix document into the typed row set (Source `from_dict`).
    pub fn from_json(document: &Map<String, Value>) -> Self {
        let rows = |key: &str| -> Vec<Map<String, Value>> {
            document
                .get(key)
                .and_then(Value::as_array)
                .map(|items| items.iter().filter_map(Value::as_object).cloned().collect())
                .unwrap_or_default()
        };
        let identity = |key: &str| -> String {
            match document.get(key) {
                Some(Value::String(text)) => text.clone(),
                Some(Value::Number(number)) => number.to_string(),
                _ => String::new(),
            }
        };
        Self {
            mission_slug: identity("mission_slug"),
            mission_number: document
                .get("mission_number")
                .and_then(Value::as_str)
                .map(str::to_owned),
            mission_type: document
                .get("mission_type")
                .and_then(Value::as_str)
                .map(str::to_owned),
            criteria: rows("criteria")
                .iter()
                .map(AcceptanceCriterion::from_json)
                .collect(),
            negative_invariants: rows("negative_invariants")
                .iter()
                .map(NegativeInvariant::from_json)
                .collect(),
        }
    }
}

/// Project a Source acceptance-matrix document onto `to_dict()` ORDER.
///
/// `AcceptanceMatrix.to_dict()` emits the three `mission_identity_fields`
/// entries (`mission_slug`, `mission_number`, `mission_type`), then
/// `overall_verdict`, `criteria`, `negative_invariants`, then the unknown keys
/// the document carried (`extras`). A criterion row is its eight declared fields
/// (`AcceptanceCriterion.to_dict`); an invariant row is
/// `NegativeInvariant.to_dict`'s typed set with the unset optionals and the
/// `legacy_unrecorded` sentinel omitted.
///
/// This is the ONE order projection: the derived `acceptance-matrix.json`
/// writer and the `merge-driver-acceptance-matrix` body both go through it.
///
/// Extras keep Source's *position* (after the known keys) but `serde_json` keeps
/// an object's keys sorted and cannot recover the file's own order, so several
/// unknown keys are emitted in sorted order. A document with no extras — every
/// canonical matrix — is byte-identical to Source; nothing is ever dropped.
/// The key order Source's dict merge produces, so `extras` can be emitted in
/// that order instead of the sorted order a `serde_json::Map` retains.
///
/// Source's row merge is `dict.fromkeys((*base, *ours, *theirs))`, and pydantic's
/// `model_extra` then preserves that insertion order in `to_dict()`. The driver
/// reads the three sides with the ordered Kernel parser and hands the resulting
/// order here; a key this map does not carry is still emitted (sorted, after the
/// ordered ones), never dropped.
#[derive(Debug, Clone, Default)]
pub struct ExtrasOrder {
    /// The document's own key order (top-level `extras`).
    pub top_level: Vec<String>,
    /// Per-row key order, keyed by the row's own id (`criterion_id` /
    /// `invariant_id`).
    pub rows: std::collections::BTreeMap<String, Vec<String>>,
}

/// Project a merged acceptance matrix with an explicit `extras` ORDER.
pub fn ordered_document_with_order(
    document: &Map<String, Value>,
    order: Option<&ExtrasOrder>,
) -> PythonJsonValue {
    let mut entries: Vec<(String, PythonJsonValue)> = Vec::new();
    for key in ["mission_slug", "mission_number", "mission_type"] {
        entries.push((
            key.to_owned(),
            PythonJsonValue::scalar(&mission_identity_text(document.get(key))),
        ));
    }
    entries.push((
        "overall_verdict".to_owned(),
        PythonJsonValue::scalar(overall_verdict_of(document)),
    ));
    entries.push((
        "criteria".to_owned(),
        row_array_ordered(
            document.get("criteria"),
            criterion_ordered_with,
            order.map(|order| &order.rows),
        ),
    ));
    entries.push((
        "negative_invariants".to_owned(),
        row_array_ordered(
            document.get("negative_invariants"),
            invariant_ordered_with,
            order.map(|order| &order.rows),
        ),
    ));
    append_extras_ordered(
        &mut entries,
        document,
        &[
            "mission_slug",
            "mission_number",
            "mission_type",
            "overall_verdict",
            "criteria",
            "negative_invariants",
        ],
        order.map(|order| order.top_level.as_slice()),
    );
    PythonJsonValue::Object(entries)
}

pub fn ordered_document(document: &Map<String, Value>) -> PythonJsonValue {
    let mut entries: Vec<(String, PythonJsonValue)> = Vec::new();
    for key in ["mission_slug", "mission_number", "mission_type"] {
        entries.push((
            key.to_owned(),
            PythonJsonValue::scalar(&mission_identity_text(document.get(key))),
        ));
    }
    entries.push((
        "overall_verdict".to_owned(),
        PythonJsonValue::scalar(overall_verdict_of(document)),
    ));
    entries.push((
        "criteria".to_owned(),
        row_array(document.get("criteria"), criterion_ordered),
    ));
    entries.push((
        "negative_invariants".to_owned(),
        row_array(document.get("negative_invariants"), invariant_ordered),
    ));
    append_extras(
        &mut entries,
        document,
        &[
            "mission_slug",
            "mission_number",
            "mission_type",
            "overall_verdict",
            "criteria",
            "negative_invariants",
        ],
    );
    PythonJsonValue::Object(entries)
}

fn mission_identity_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        _ => String::new(),
    }
}

fn overall_verdict_of(document: &Map<String, Value>) -> &'static str {
    // The verdict is a pure function of the rows; read them through the same
    // typed `from_dict` this projection writes, so a row missing `pass_fail`
    // defaults to `pending` exactly as Source's dataclass does.
    let matrix = AcceptanceMatrix::from_json(document);
    matrix.overall_verdict()
}

fn row_array_ordered(
    rows: Option<&Value>,
    project: fn(&Map<String, Value>, Option<&[String]>) -> PythonJsonValue,
    orders: Option<&std::collections::BTreeMap<String, Vec<String>>>,
) -> PythonJsonValue {
    match rows {
        Some(Value::Array(items)) => PythonJsonValue::Array(
            items
                .iter()
                .map(|row| match row.as_object() {
                    Some(object) => {
                        let id = row_id_text(object);
                        let order = orders.and_then(|orders| orders.get(&id)).map(Vec::as_slice);
                        project(object, order)
                    }
                    None => PythonJsonValue::from_serde(row),
                })
                .collect(),
        ),
        _ => PythonJsonValue::Array(Vec::new()),
    }
}

fn row_id_text(row: &Map<String, Value>) -> String {
    for key in ["criterion_id", "invariant_id"] {
        if let Some(Value::String(text)) = row.get(key) {
            return text.clone();
        }
    }
    String::new()
}

fn row_array(
    rows: Option<&Value>,
    project: fn(&Map<String, Value>) -> PythonJsonValue,
) -> PythonJsonValue {
    match rows {
        Some(Value::Array(items)) => PythonJsonValue::Array(
            items
                .iter()
                .map(|row| match row.as_object() {
                    Some(object) => project(object),
                    None => PythonJsonValue::from_serde(row),
                })
                .collect(),
        ),
        _ => PythonJsonValue::Array(Vec::new()),
    }
}

fn criterion_ordered(row: &Map<String, Value>) -> PythonJsonValue {
    criterion_ordered_with(row, None)
}

fn criterion_ordered_with(row: &Map<String, Value>, order: Option<&[String]>) -> PythonJsonValue {
    let typed = AcceptanceCriterion::from_json(row).to_json();
    let mut entries: Vec<(String, PythonJsonValue)> = Vec::new();
    for key in [
        "criterion_id",
        "description",
        "proof_type",
        "evidence",
        "pass_fail",
        "verified_by",
        "verified_at",
        "notes",
    ] {
        entries.push((
            key.to_owned(),
            PythonJsonValue::from_serde(&typed.get(key).cloned().unwrap_or(Value::Null)),
        ));
    }
    append_extras_ordered(
        &mut entries,
        row,
        &[
            "criterion_id",
            "description",
            "proof_type",
            "evidence",
            "pass_fail",
            "verified_by",
            "verified_at",
            "notes",
        ],
        order,
    );
    PythonJsonValue::Object(entries)
}

fn invariant_ordered(row: &Map<String, Value>) -> PythonJsonValue {
    invariant_ordered_with(row, None)
}

fn invariant_ordered_with(row: &Map<String, Value>, order: Option<&[String]>) -> PythonJsonValue {
    let typed = NegativeInvariant::from_json(row).to_json();
    let mut entries: Vec<(String, PythonJsonValue)> = Vec::new();
    for key in [
        "invariant_id",
        "description",
        "verification_method",
        "verification_command",
        "result",
        "evidence",
    ] {
        entries.push((
            key.to_owned(),
            PythonJsonValue::from_serde(&typed.get(key).cloned().unwrap_or(Value::Null)),
        ));
    }
    // The typed `to_json` has already applied Source's omission rules (unset
    // optional keys, the `legacy_unrecorded` sentinel), so the remaining known
    // keys are exactly the ones Source emits here.
    for key in [
        "scope",
        "verified_ref",
        "verified_surface_kind",
        "deferred_reason",
        "deferred_to_phase",
        "provenance_origin",
    ] {
        if let Some(value) = typed.get(key) {
            entries.push((key.to_owned(), PythonJsonValue::from_serde(value)));
        }
    }
    append_extras_ordered(
        &mut entries,
        row,
        &[
            "invariant_id",
            "description",
            "verification_method",
            "verification_command",
            "result",
            "evidence",
            "scope",
            "verified_ref",
            "verified_surface_kind",
            "deferred_reason",
            "deferred_to_phase",
            "provenance_origin",
        ],
        order,
    );
    PythonJsonValue::Object(entries)
}

/// Append the unknown keys in the Source dict order first, then any remaining
/// unknown keys (a key the order hint does not carry is still emitted, never
/// dropped).
fn append_extras_ordered(
    entries: &mut Vec<(String, PythonJsonValue)>,
    document: &Map<String, Value>,
    known: &[&str],
    order: Option<&[String]>,
) {
    let mut emitted: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    if let Some(order) = order {
        for key in order {
            if known.contains(&key.as_str()) || !emitted.insert(key.clone()) {
                continue;
            }
            if let Some(value) = document.get(key) {
                entries.push((key.clone(), PythonJsonValue::from_serde(value)));
            }
        }
    }
    // Every unknown key the hint did not carry is still emitted exactly once,
    // after the ordered ones (never dropped, never duplicated).
    for (key, value) in document {
        if known.contains(&key.as_str()) || !emitted.insert(key.clone()) {
            continue;
        }
        entries.push((key.clone(), PythonJsonValue::from_serde(value)));
    }
}

fn append_extras(
    entries: &mut Vec<(String, PythonJsonValue)>,
    document: &Map<String, Value>,
    known: &[&str],
) {
    for (key, value) in document {
        if known.contains(&key.as_str()) {
            continue;
        }
        entries.push((key.clone(), PythonJsonValue::from_serde(value)));
    }
}

/// One work package's event-sourced review verdict.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewEvidence {
    /// Work package id.
    pub wp_id: String,
    /// Reviewer identity.
    pub reviewer: String,
    /// `approved` | `changes_requested`.
    pub verdict: String,
    /// Approval reference (or `feedback://` URI).
    pub reference: String,
}

/// Derive `pending` `code_review` rows from event-sourced review evidence.
///
/// All-or-nothing per mission, exactly like the Source gate:
///
/// * every tracked work package must be `approved`/`done`; and
/// * every one of them must carry review evidence whose verdict is itself
///   `approved` — a lane-`approved` work package whose recorded verdict is a
///   stale `changes_requested` is *not* proof and leaves every row pending;
/// * a criterion that is already judged is never re-judged (forward-only).
///
/// Returns the rows it actually derived (`criterion_id -> (verified_by,
/// evidence)`), empty when the evidence chain is incomplete.
pub fn review_evidence_population(
    criteria: &[AcceptanceCriterion],
    wp_lanes: &[(String, String)],
    evidence: &[ReviewEvidence],
) -> Vec<(String, String, String)> {
    let pending: Vec<&AcceptanceCriterion> = criteria
        .iter()
        .filter(|row| row.pass_fail == VERDICT_PENDING && row.is_auto_derivable())
        .collect();
    if pending.is_empty() || wp_lanes.is_empty() {
        return Vec::new();
    }
    if wp_lanes
        .iter()
        .any(|(_, lane)| !matches!(lane.as_str(), "approved" | "done"))
    {
        return Vec::new();
    }
    let mut ordered: Vec<&(String, String)> = wp_lanes.iter().collect();
    ordered.sort_by(|left, right| left.0.cmp(&right.0));
    let mut reviewers: Vec<&str> = Vec::new();
    let mut references: Vec<String> = Vec::new();
    for (wp_id, _) in ordered {
        let Some(found) = evidence
            .iter()
            .find(|entry| &entry.wp_id == wp_id && entry.verdict == "approved")
        else {
            return Vec::new();
        };
        if !reviewers.contains(&found.reviewer.as_str()) {
            reviewers.push(&found.reviewer);
        }
        references.push(format!("{wp_id}:{}", found.reference));
    }
    let verified_by = reviewers.join(", ");
    let evidence_text = references.join("; ");
    pending
        .into_iter()
        .map(|row| {
            (
                row.criterion_id.clone(),
                verified_by.clone(),
                evidence_text.clone(),
            )
        })
        .collect()
}

/// One gate's result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GateResult {
    /// The gate ran and passed.
    Pass,
    /// The gate ran and refused.
    Fail,
    /// The gate does not apply (recorded, never silently dropped).
    Skip,
}

impl GateResult {
    /// The wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Skip => "skip",
        }
    }
}

/// A named gate result with the detail a reader needs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AcceptanceGate {
    /// Stable gate id.
    pub id: String,
    /// Human name.
    pub name: String,
    /// Result.
    pub result: GateResult,
    /// Observed detail (never a generic "failed").
    pub detail: String,
}

impl AcceptanceGate {
    fn new(id: &str, name: &str, result: GateResult, detail: impl Into<String>) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            result,
            detail: detail.into(),
        }
    }

    /// The JSON shape used by the `accept --json` payload.
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "result": self.result.as_str(),
            "detail": self.detail,
        })
    }
}

/// One work package's acceptance input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WpAcceptanceInput {
    /// Work package id.
    pub wp_id: String,
    /// Canonical lane from the reduced snapshot.
    pub lane: String,
    /// Implementation actor identity recorded for the work package.
    pub implementer: Option<String>,
    /// Event-sourced review verdict, when one exists.
    pub review: Option<ReviewEvidence>,
    /// Declared owned-file globs.
    pub owned_files: Vec<String>,
    /// Repository-relative files this work package actually changed.
    pub changed_files: Vec<String>,
}

/// The whole acceptance input.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AcceptanceInputs {
    /// Mission slug.
    pub mission_slug: String,
    /// Mission number, when assigned.
    pub mission_number: Option<u32>,
    /// Mission type.
    pub mission_type: Option<String>,
    /// `tasks.md` text, when the artifact exists.
    pub tasks_md: Option<String>,
    /// Whether `lanes.json` was readable.
    pub lanes_present: bool,
    /// Whether the caller could observe each work package's changed files.
    ///
    /// `false` is a failed gate, never an empty change set: an unobserved
    /// attribution cannot prove ownership.
    pub attribution_available: bool,
    /// Work package rows.
    pub work_packages: Vec<WpAcceptanceInput>,
    /// Operator-canceled work packages with recorded provenance.
    pub canceled_with_provenance: Vec<String>,
    /// DECLARED functional requirement ids from `spec.md`.
    ///
    /// Source scaffolds one `pending` criterion per declared requirement id
    /// (`acceptance/matrix.py::scaffold_acceptance_matrix`), so the matrix
    /// proves requirement coverage rather than only restating work packages.
    /// Empty means the spec declared none; the work-package rows are then used.
    pub spec_requirement_ids: Vec<String>,
}

/// How one acceptance evaluation is run.
///
/// Source's `accept --lenient` passes `strict_metadata=False`; the STRICT
/// evaluation is its own surface ([`evaluate_acceptance`]) and is unchanged, so
/// a caller can keep asserting the strict result.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AcceptanceOptions {
    /// Downgrade the checks Source treats as advisory under `--lenient`.
    pub lenient: bool,
}

/// The acceptance report.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AcceptanceReport {
    /// Gates in execution order.
    pub gates: Vec<AcceptanceGate>,
    /// Per-work-package evidence detail.
    pub work_packages: Vec<WpAcceptanceInput>,
    /// The judged matrix.
    pub matrix: AcceptanceMatrix,
    /// Unchecked `tasks.md` rows that materially block acceptance.
    pub unchecked_tasks: Vec<String>,
    /// Non-blocking findings (`--lenient` downgrades), in evaluation order.
    pub warnings: Vec<String>,
}

impl AcceptanceReport {
    /// Every gate passed or explicitly skipped.
    pub fn ok(&self) -> bool {
        !self
            .gates
            .iter()
            .any(|gate| gate.result == GateResult::Fail)
    }

    /// The matrix verdict.
    pub fn verdict(&self) -> &'static str {
        self.matrix.overall_verdict()
    }

    /// The `accept --json` payload.
    pub fn to_json(&self) -> Value {
        json!({
            "mission_slug": self.matrix.mission_slug,
            "mission_number": self.matrix.mission_number,
            "mission_type": self.matrix.mission_type,
            "ok": self.ok(),
            "verdict": self.verdict(),
            "gates": self.gates.iter().map(AcceptanceGate::to_json).collect::<Vec<_>>(),
            "unchecked_tasks": self.unchecked_tasks,
            "criteria": self.matrix.criteria.iter().map(AcceptanceCriterion::to_json).collect::<Vec<_>>(),
            "work_packages": self.work_packages.iter().map(|wp| json!({
                "wp_id": wp.wp_id,
                "lane": wp.lane,
                "implementer": wp.implementer,
                "reviewer": wp.review.as_ref().map(|review| review.reviewer.clone()),
                "review_verdict": wp.review.as_ref().map(|review| review.verdict.clone()),
                "review_reference": wp.review.as_ref().map(|review| review.reference.clone()),
                "owned_files": wp.owned_files,
                "changed_files": wp.changed_files,
            })).collect::<Vec<_>>(),
        })
    }
}

/// Is `path` covered by `globs` (Source owned-file matching)?
///
/// A declared glob is `*` (any run of characters, including `/`) or a literal
/// path, matching the lane planner's own owned-file semantics. An empty glob
/// list owns nothing.
pub fn owned_file_covers(globs: &[String], path: &str) -> bool {
    globs.iter().any(|glob| glob_matches(glob, path))
}

/// Match one owned-file glob against one path.
pub fn glob_matches(glob: &str, path: &str) -> bool {
    if glob.is_empty() {
        return false;
    }
    let mut rest = path;
    let mut parts = glob.split('*').peekable();
    let Some(first) = parts.next() else {
        return glob == path;
    };
    if !rest.starts_with(first) {
        return false;
    }
    rest = &rest[first.len()..];
    while let Some(part) = parts.next() {
        if part.is_empty() {
            continue;
        }
        if parts.peek().is_none() {
            // The final literal segment must be a suffix.
            return rest.ends_with(part) && rest.len() >= part.len();
        }
        match rest.find(part) {
            Some(index) => rest = &rest[index + part.len()..],
            None => return false,
        }
    }
    true
}

/// The unchecked canonical subtask rows of `tasks.md` (Source
/// `iter_unchecked_subtask_rows`).
///
/// Fence-aware and `T###`-scoped: a prose `- [ ]` checkbox without a task id,
/// and every row inside a fenced block, is not an implementation subtask.
pub fn iter_unchecked_subtask_rows(text: &str) -> Vec<String> {
    let mut rows = Vec::new();
    let mut in_code_fence = false;
    for line in text.split('\n') {
        let stripped = line.trim();
        if stripped.starts_with("```") || stripped.starts_with("~~~") {
            in_code_fence = !in_code_fence;
            continue;
        }
        if in_code_fence {
            continue;
        }
        if is_unchecked_subtask_row(stripped) {
            rows.push(stripped.to_owned());
        }
    }
    rows
}

/// `^-\s*\[\s*\]\s*T\p{Nd}{3,}\b` (Source `UNCHECKED_SUBTASK_ROW`).
fn is_unchecked_subtask_row(line: &str) -> bool {
    let mut chars = line.char_indices().peekable();
    match chars.next() {
        Some((_, '-')) => {}
        _ => return false,
    }
    let mut offset = skip_whitespace(line, 1);
    if !line[offset..].starts_with('[') {
        return false;
    }
    offset += 1;
    offset = skip_whitespace(line, offset);
    if !line[offset..].starts_with(']') {
        return false;
    }
    offset += 1;
    offset = skip_whitespace(line, offset);
    let rest = &line[offset..];
    let Some(stripped) = rest.strip_prefix('T') else {
        return false;
    };
    let digits: String = stripped
        .chars()
        .take_while(|value| is_decimal_digit(*value))
        .collect();
    if digits.chars().count() < 3 {
        return false;
    }
    // `\b` after `\d{3,}`: the next character must not be a word character.
    match stripped[digits.len()..].chars().next() {
        Some(next) => !is_word_char(next),
        None => true,
    }
}

fn skip_whitespace(text: &str, from: usize) -> usize {
    let mut offset = from;
    for (index, value) in text[from..].char_indices() {
        if !value.is_whitespace() {
            return from + index;
        }
        offset = from + index + value.len_utf8();
    }
    offset
}

/// Python's `\w`-adjacent notion: alphanumeric or underscore.
fn is_word_char(value: char) -> bool {
    value.is_alphanumeric() || value == '_'
}

/// Unicode numeric (Python's `\d` is `Nd`; Rust's `is_numeric` is the `N`
/// superset, so a non-decimal numeric codepoint is the only widening here).
fn is_decimal_digit(value: char) -> bool {
    value.is_numeric()
}

/// Does a lane value count as a terminal ending for acceptance?
///
/// Delegates to the one acceptable-ending authority
/// ([`crate::mutation_readiness::is_acceptable_ending`]) through the canonical
/// lane parser: an unrecognised lane value is never a terminal ending.
pub fn acceptance_ending(lane: &str, has_provenance: bool) -> bool {
    match crate::lane::Lane::parse_canonical(lane) {
        Ok(parsed) => crate::mutation_readiness::is_acceptable_ending(parsed, has_provenance),
        Err(_) => false,
    }
}

/// Derive the DECLARED requirement ids from `spec.md` content.
///
/// Source (`requirement_mapping._declared_ids`) recognises exactly four
/// declared shapes, doc-wide (not scoped to a section heading), and captures
/// only the FIRST id on each line so a table row's description-cell citation is
/// not counted as a second declaration:
///
/// 1. a table row whose id cell leads (`| FR-001 | ... |`, optional `**`/`~~`);
/// 2. a heading naming the id (`### FR-001`, `### FR-001: Title`);
/// 3. a bulleted/numbered item (`- FR-001`, `1. **FR-001**`);
/// 4. a bold-led definition (`**FR-001** ...`).
///
/// Kinds are the software-dev set `FR|NFR|SC|C`, case-insensitive, optionally
/// followed by one lowercase letter (`FR-003a`). The returned ids are canonical
/// (uppercase kind, digits verbatim, lowercase suffix), sorted and deduplicated.
/// A compound tail (`FR-008-mandated`, `FR-002.3`) drops the whole token rather
/// than truncating it to a well-formed prefix. HTML comments are blanked first
/// (position-preserving), so a commented-out declaration is not a declaration.
pub fn parse_declared_requirement_ids(spec_md: &str) -> Vec<String> {
    let blanked = blank_html_comments(spec_md);
    let mut found: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for line in blanked.lines() {
        let Some((raw_id, end)) = first_declared_id(line) else {
            continue;
        };
        if is_compound_tail(line, end) {
            continue;
        }
        if let Some(canonical) = canonical_requirement_id(&raw_id) {
            found.insert(canonical);
        }
    }
    found.into_iter().collect()
}

fn blank_html_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 4..];
        match after.find("-->") {
            Some(end) => {
                for character in rest[start..start + 4 + end + 3].chars() {
                    out.push(if character == '\n' { '\n' } else { ' ' });
                }
                rest = &after[end + 3..];
            }
            None => {
                for character in rest[start..].chars() {
                    out.push(if character == '\n' { '\n' } else { ' ' });
                }
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

#[derive(Clone, Copy)]
enum DeclaredShape {
    TableRow,
    Heading,
    ListItem,
    BoldLead,
}

const DECLARED_SHAPES: [DeclaredShape; 4] = [
    DeclaredShape::TableRow,
    DeclaredShape::Heading,
    DeclaredShape::ListItem,
    DeclaredShape::BoldLead,
];

fn strip_declared_prefix(line: &str, kind: DeclaredShape) -> Option<String> {
    let trimmed = line.trim_start();
    let rest = match kind {
        DeclaredShape::TableRow => {
            let rest = trimmed.strip_prefix('|')?.trim_start();
            let rest = rest
                .strip_prefix("**")
                .or_else(|| rest.strip_prefix("~~"))
                .unwrap_or(rest);
            let rest = rest
                .strip_prefix("**")
                .or_else(|| rest.strip_prefix("~~"))
                .unwrap_or(rest);
            rest.to_owned()
        }
        DeclaredShape::Heading => {
            let hashes = trimmed.chars().take_while(|value| *value == '#').count();
            if !(1..=6).contains(&hashes) {
                return None;
            }
            trimmed[hashes..].trim_start().to_owned()
        }
        DeclaredShape::ListItem => {
            if let Some(rest) = trimmed
                .strip_prefix("- ")
                .or_else(|| trimmed.strip_prefix("* "))
            {
                rest.trim_start().to_owned()
            } else {
                let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
                if digits == 0 {
                    return None;
                }
                trimmed[digits..]
                    .strip_prefix(". ")?
                    .trim_start()
                    .to_owned()
            }
        }
        DeclaredShape::BoldLead => trimmed.strip_prefix("**")?.to_owned(),
    };
    Some(
        rest.strip_prefix("**")
            .unwrap_or(&rest)
            .trim_start()
            .to_owned(),
    )
}

fn first_declared_id(line: &str) -> Option<(String, usize)> {
    for shape in DECLARED_SHAPES {
        let Some(rest) = strip_declared_prefix(line, shape) else {
            continue;
        };
        let offset = line.len() - rest.len();
        if let Some((id, consumed)) = match_requirement_core(&rest) {
            return Some((id, offset + consumed));
        }
    }
    None
}

fn match_requirement_core(text: &str) -> Option<(String, usize)> {
    let upper = text.to_ascii_uppercase();
    let kind = ["NFR", "SC", "FR", "C"]
        .into_iter()
        .find(|kind| upper.starts_with(kind))?;
    if !upper
        .as_bytes()
        .get(kind.len())
        .is_some_and(|byte| *byte == b'-')
    {
        return None;
    }
    let mut index = kind.len() + 1;
    let digits_start = index;
    while text.as_bytes().get(index).is_some_and(u8::is_ascii_digit) {
        index += 1;
    }
    if index == digits_start {
        return None;
    }
    let digits = &text[digits_start..index];
    let mut suffix = "";
    if let Some(character) = text.chars().nth(index)
        && character.is_ascii_lowercase()
    {
        suffix = &text[index..index + 1];
        index += 1;
    }
    let next = text[index..].chars().next();
    if next.is_some_and(|character| character.is_alphanumeric() || character == '_') {
        return None;
    }
    Some((format!("{kind}-{digits}{suffix}"), index))
}

fn is_compound_tail(text: &str, end: usize) -> bool {
    let mut characters = text[end..].chars();
    let Some(separator) = characters.next() else {
        return false;
    };
    if separator != '-' && separator != '.' {
        return false;
    }
    characters
        .next()
        .is_some_and(|character| character.is_ascii_alphanumeric())
}

fn canonical_requirement_id(raw: &str) -> Option<String> {
    match_requirement_core(raw).map(|(id, _)| id)
}

/// Evaluate acceptance over the supplied evidence.
///
/// Gate order is stable and every refusal names what it observed; no gate ever
/// consults a value it did not receive.
pub fn evaluate_acceptance(inputs: &AcceptanceInputs) -> AcceptanceReport {
    evaluate_acceptance_with(inputs, AcceptanceOptions::default())
}

/// Evaluate with explicit options (Source's `strict_metadata` switch).
pub fn evaluate_acceptance_with(
    inputs: &AcceptanceInputs,
    options: AcceptanceOptions,
) -> AcceptanceReport {
    let mut gates = Vec::new();

    // Gate 1: lanes manifest.
    gates.push(AcceptanceGate::new(
        "lanes_manifest",
        "lanes manifest",
        if inputs.lanes_present {
            GateResult::Pass
        } else {
            GateResult::Fail
        },
        if inputs.lanes_present {
            "lanes.json is present".to_owned()
        } else {
            "lanes.json is missing; run tasks and finalize-tasks before accepting".to_owned()
        },
    ));

    // Gate 2: every tracked work package is terminal.
    let non_terminal: Vec<String> = inputs
        .work_packages
        .iter()
        .filter(|wp| {
            !acceptance_ending(
                &wp.lane,
                inputs.canceled_with_provenance.contains(&wp.wp_id),
            )
        })
        .map(|wp| format!("{}={}", wp.wp_id, wp.lane))
        .collect();
    let tracked = !inputs.work_packages.is_empty();
    gates.push(AcceptanceGate::new(
        "wp_lane_terminal",
        "work package terminal lanes",
        if !tracked {
            GateResult::Fail
        } else if non_terminal.is_empty() {
            GateResult::Pass
        } else {
            GateResult::Fail
        },
        if !tracked {
            "no work package is tracked in the status snapshot".to_owned()
        } else if non_terminal.is_empty() {
            format!(
                "all {} work package(s) are in a terminal lane",
                inputs.work_packages.len()
            )
        } else {
            format!("not terminal: {}", non_terminal.join(", "))
        },
    ));

    // Gate 3: independent, approved review evidence per work package.
    let mut review_failures: Vec<String> = Vec::new();
    for wp in &inputs.work_packages {
        // A canceled work package carries operator provenance instead of a
        // review: cancellation is never read as an approval, and it owes no
        // review evidence.
        if wp.lane == "canceled" && inputs.canceled_with_provenance.contains(&wp.wp_id) {
            continue;
        }
        let Some(review) = &wp.review else {
            review_failures.push(format!("{}: no event-sourced review verdict", wp.wp_id));
            continue;
        };
        if review.verdict != "approved" {
            review_failures.push(format!(
                "{}: recorded verdict is {} (not approved)",
                wp.wp_id, review.verdict
            ));
            continue;
        }
        if !actor_identities_independent(wp.implementer.as_deref(), Some(review.reviewer.as_str()))
        {
            review_failures.push(format!(
                "{}: reviewer {} is not independent of implementer {}",
                wp.wp_id,
                review.reviewer,
                wp.implementer.as_deref().unwrap_or("<unrecorded>")
            ));
            continue;
        }
        if review.reference.trim().is_empty() {
            review_failures.push(format!(
                "{}: approved review records no reference",
                wp.wp_id
            ));
        }
    }
    gates.push(AcceptanceGate::new(
        "review_evidence",
        "independent review evidence",
        if tracked && review_failures.is_empty() {
            GateResult::Pass
        } else {
            GateResult::Fail
        },
        if !tracked {
            "no work package is tracked in the status snapshot".to_owned()
        } else if review_failures.is_empty() {
            format!(
                "{} work package(s) carry an approved, independent review",
                inputs.work_packages.len()
            )
        } else {
            review_failures.join("; ")
        },
    ));

    // Gate 4: owned-file attribution.
    let mut unattributed: Vec<String> = Vec::new();
    for wp in &inputs.work_packages {
        for path in &wp.changed_files {
            if !owned_file_covers(&wp.owned_files, path) {
                unattributed.push(format!("{}:{}", wp.wp_id, path));
            }
        }
    }
    // Source's acceptance summary has NO owned-file attribution gate: it never
    // attributes a work package's changed files to its `owned_files` and never
    // refuses on that basis. The adapter added one, so under `--lenient`
    // (`AcceptanceOptions::lenient`, Source's `strict_metadata=False`) it is
    // advisory — reported as a warning, never a fabricated refusal — while the
    // strict evaluation keeps it (that strict surface is distinct and preserved).
    let mut warnings: Vec<String> = Vec::new();
    let attribution_issue = if !inputs.attribution_available {
        Some(
            "the changed files of each work package could not be observed (no readable Git checkout)"
                .to_owned(),
        )
    } else if !unattributed.is_empty() {
        Some(format!(
            "files outside the owning work package: {}",
            unattributed.join(", ")
        ))
    } else {
        None
    };
    if options.lenient
        && let Some(issue) = &attribution_issue
    {
        warnings.push(format!(
            "lenient: {issue} (advisory; Source's acceptance summary has no owned-file attribution gate)"
        ));
    }
    gates.push(AcceptanceGate::new(
        "owned_file_evidence",
        "owned-file attribution",
        if attribution_issue.is_some() && !options.lenient {
            GateResult::Fail
        } else {
            GateResult::Pass
        },
        if !inputs.attribution_available {
            "the changed files of each work package could not be observed (no readable Git checkout)"
                .to_owned()
        } else if unattributed.is_empty() {
            format!(
                "every changed file is owned by its work package ({} file(s))",
                inputs
                    .work_packages
                    .iter()
                    .map(|wp| wp.changed_files.len())
                    .sum::<usize>()
            )
        } else {
            format!("files outside the owning work package: {}", unattributed.join(", "))
        },
    ));

    // Gate 5: tasks.md checkboxes, normalized by terminal lanes (FR-009).
    let unchecked_all = match &inputs.tasks_md {
        Some(text) => iter_unchecked_subtask_rows(text),
        None => vec!["tasks.md missing".to_owned()],
    };
    let tasks_missing = unchecked_all == ["tasks.md missing".to_owned()];
    let all_terminal = tracked && non_terminal.is_empty();
    let unchecked: Vec<String> = if tasks_missing || all_terminal {
        Vec::new()
    } else {
        unchecked_all
    };
    gates.push(AcceptanceGate::new(
        "tasks_checkbox",
        "tasks.md checkboxes",
        if tasks_missing || !unchecked.is_empty() {
            GateResult::Fail
        } else {
            GateResult::Pass
        },
        if tasks_missing {
            "tasks.md is missing".to_owned()
        } else if unchecked.is_empty() {
            if all_terminal {
                "every work package is terminal, so unticked checkboxes do not block".to_owned()
            } else {
                "no unchecked canonical subtask rows".to_owned()
            }
        } else {
            format!(
                "{} unchecked subtask row(s): {}",
                unchecked.len(),
                unchecked.join(" | ")
            )
        },
    ));

    // Matrix: Source scaffolds one `pending` criterion per DECLARED requirement
    // id from `spec.md`; when the spec declares none, the observed work-package
    // rows stand in. Both are the same `code_review` row shape and are populated
    // from review evidence below.
    let mut matrix = AcceptanceMatrix::new(inputs.mission_slug.clone(), Vec::new());
    matrix.mission_number = inputs.mission_number.map(|number| number.to_string());
    matrix.mission_type = inputs.mission_type.clone();
    if !inputs.spec_requirement_ids.is_empty() {
        for requirement_id in &inputs.spec_requirement_ids {
            matrix.criteria.push(AcceptanceCriterion::code_review(
                requirement_id.clone(),
                format!("{requirement_id} is satisfied and verified"),
            ));
        }
    }
    for wp in &inputs.work_packages {
        let mut row = AcceptanceCriterion::code_review(
            format!("wp_review:{}", wp.wp_id),
            format!(
                "{} is reviewed and approved by an independent reviewer",
                wp.wp_id
            ),
        );
        let has_provenance = inputs.canceled_with_provenance.contains(&wp.wp_id);
        if !acceptance_ending(&wp.lane, has_provenance) {
            row.pass_fail = VERDICT_FAIL.to_owned();
            row.notes = Some(format!("lane is {}", wp.lane));
        } else if wp.lane == "canceled" {
            // A cancellation is terminal only because an operator provenance
            // record exists; that record is the evidence, so cite it rather than
            // inventing a review.
            row.pass_fail = VERDICT_PASS.to_owned();
            row.evidence = Some(format!("{}:canceled_with_operator_provenance", wp.wp_id));
            row.verified_by = Some(OPERATOR_EVIDENCE_ACTOR.to_owned());
            row.verified_at = Some(sk_kernel::clock::now_utc_iso());
            row.notes = Some("canceled with operator provenance".to_owned());
        }
        matrix.criteria.push(row);
    }
    let wp_lanes: Vec<(String, String)> = inputs
        .work_packages
        .iter()
        .map(|wp| (wp.wp_id.clone(), wp.lane.clone()))
        .collect();
    let evidence: Vec<ReviewEvidence> = inputs
        .work_packages
        .iter()
        .filter_map(|wp| wp.review.clone())
        .collect();
    // Review-evidence population is gated on the review gate passing: a
    // `changes_requested` verdict or a non-independent reviewer is not approval
    // evidence, so no row may be stamped `pass` from it. Source keeps such rows
    // pending and the matrix gate holds acceptance closed.
    let derived = if review_failures.is_empty() {
        review_evidence_population(&matrix.criteria, &wp_lanes, &evidence)
    } else {
        Vec::new()
    };
    if !derived.is_empty() {
        let verified_at = sk_kernel::clock::now_utc_iso();
        let derived_notes = "auto-derived from event-sourced review evidence";
        for row in &mut matrix.criteria {
            if let Some((_, verified_by, evidence_text)) = derived
                .iter()
                .find(|(criterion_id, _, _)| criterion_id == &row.criterion_id)
            {
                row.pass_fail = VERDICT_PASS.to_owned();
                row.verified_by = Some(verified_by.clone());
                row.verified_at = Some(verified_at.clone());
                row.evidence = Some(evidence_text.clone());
                row.notes = Some(derived_notes.to_owned());
            }
        }
    }
    let verdict = matrix.overall_verdict();
    gates.push(AcceptanceGate::new(
        "acceptance_matrix",
        "acceptance matrix verdict",
        match verdict {
            VERDICT_PASS => GateResult::Pass,
            _ => GateResult::Fail,
        },
        format!(
            "overall_verdict={verdict} over {} criterion row(s)",
            matrix.criteria.len()
        ),
    ));

    AcceptanceReport {
        gates,
        work_packages: inputs.work_packages.clone(),
        matrix,
        unchecked_tasks: unchecked,
        warnings,
    }
}

// ---------------------------------------------------------------------------
// Negative-invariant enforcement (Source `enforce_negative_invariants`)
// ---------------------------------------------------------------------------

/// The surface a judgement is established against (Source `GateExecutionContext`).
///
/// Provenance, not decoration: a freshly judged terminal result is stamped with
/// this ref and surface so it is attributable and never silently re-judged from
/// a surface that cannot hold the subject.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvariantJudgeContext {
    /// The git ref the judgement is established against.
    pub reference: String,
    /// The `TopologySurface` value (e.g. `primary` / `coord` / `consolidated`).
    pub surface: String,
    /// The lifecycle phase NAME (e.g. `ACCEPT`, `POST_CONSOLIDATION`).
    pub phase: String,
}

/// Run every negative invariant, exactly as Source does.
///
/// * a TERMINAL result (`confirmed_absent` / `still_present` /
///   `verification_error`) is preserved verbatim, provenance included (NI-2/C3);
/// * `deferred_to_consolidation` is preserved too — it is judged by the
///   post-consolidation op, never re-judged here (NI-4);
/// * a `pending` row whose scoped `grep_absence` subject cannot exist on this
///   surface defers instead of reporting a false `still_present` (NI-3/C4/C9);
/// * a freshly judged terminal result is stamped `recorded` with the context's
///   surface + ref (NI-1).
///
/// `grep_absence` searches the repo with `grep -r -I --exclude=…` (the exact
/// Source argv), `custom_command` runs the configured command through `sh -c`,
/// and `route_check` is NOT implemented in Source either — it stays `pending`
/// with the unknown-method evidence, which is Source-observable behaviour, not a
/// refusal invented here.
pub fn enforce_negative_invariants(
    repo_root: &Path,
    invariants: &[NegativeInvariant],
    context: Option<&InvariantJudgeContext>,
) -> Vec<NegativeInvariant> {
    invariants
        .iter()
        .map(|invariant| {
            if invariant.is_terminal() || invariant.result == DEFERRED_TO_CONSOLIDATION {
                return invariant.clone();
            }
            if let Some(context) = context
                && should_defer(repo_root, invariant, context)
            {
                return defer_invariant(invariant, context);
            }
            stamp_provenance(judge_invariant(repo_root, invariant), context)
        })
        .collect()
}

fn should_defer(
    repo_root: &Path,
    invariant: &NegativeInvariant,
    context: &InvariantJudgeContext,
) -> bool {
    if context.phase == POST_CONSOLIDATION_PHASE_NAME {
        return false;
    }
    if invariant.verification_method != "grep_absence" {
        return false;
    }
    let Some(scope) = invariant.scope.as_deref().map(str::trim) else {
        return false;
    };
    if scope.is_empty() {
        return false;
    }
    scope
        .split_whitespace()
        .any(|root| !repo_root.join(root).exists())
}

fn defer_invariant(
    invariant: &NegativeInvariant,
    context: &InvariantJudgeContext,
) -> NegativeInvariant {
    let scope = invariant.scope.clone().unwrap_or_default();
    let mut deferred = invariant.clone();
    deferred.result = DEFERRED_TO_CONSOLIDATION.to_owned();
    deferred.evidence = Some(format!(
        "Scoped subject {} is absent on the {} surface pre-consolidation; deferred to post-consolidation verification.",
        sk_kernel::python_json::python_repr(&scope),
        context.surface
    ));
    deferred.deferred_reason = Some(format!(
        "Scoped path(s) {} do not exist on the {} surface at ref {}; judging here would report a false still_present (FR-003). Deferred to the post-consolidation verification op.",
        sk_kernel::python_json::python_repr(&scope),
        context.surface,
        sk_kernel::python_json::python_repr(&context.reference)
    ));
    deferred.deferred_to_phase = Some(POST_CONSOLIDATION_PHASE_NAME.to_owned());
    deferred
}

fn stamp_provenance(
    invariant: NegativeInvariant,
    context: Option<&InvariantJudgeContext>,
) -> NegativeInvariant {
    let Some(context) = context else {
        return invariant;
    };
    if !invariant.is_terminal() {
        return invariant;
    }
    let mut stamped = invariant;
    stamped.provenance_origin = PROVENANCE_RECORDED.to_owned();
    stamped.verified_ref = Some(context.reference.clone());
    stamped.verified_surface_kind = Some(context.surface.clone());
    stamped
}

fn judge_invariant(repo_root: &Path, invariant: &NegativeInvariant) -> NegativeInvariant {
    match invariant.verification_method.as_str() {
        "grep_absence" => judge_grep_absence(repo_root, invariant),
        "custom_command" => judge_custom_command(repo_root, invariant),
        _ => {
            // Unknown method (including the declared-but-unimplemented
            // `route_check`): Source leaves the row `pending` and says why.
            let mut row = invariant.clone();
            row.result = VERDICT_PENDING.to_owned();
            row.evidence = Some(format!(
                "Unknown verification method: {}",
                invariant.verification_method
            ));
            row
        }
    }
}

fn judge_grep_absence(repo_root: &Path, invariant: &NegativeInvariant) -> NegativeInvariant {
    let Some(pattern) = invariant
        .verification_command
        .as_deref()
        .filter(|pattern| !pattern.is_empty())
    else {
        let mut row = invariant.clone();
        row.result = VERDICT_PENDING.to_owned();
        row.evidence = Some("No grep pattern specified in verification_command".to_owned());
        return row;
    };
    // Source's exact argv: `grep -r -I --exclude=acceptance-matrix.json
    // --exclude-dir=.git -- <pattern> <roots…>`; an unscoped row searches `.`.
    let roots: Vec<String> = match invariant.scope.as_deref() {
        Some(scope) if !scope.trim().is_empty() => {
            scope.split_whitespace().map(str::to_owned).collect()
        }
        _ => vec![".".to_owned()],
    };
    let mut command = std::process::Command::new("grep");
    command
        .arg("-r")
        .arg("-I")
        .arg("--exclude=acceptance-matrix.json")
        .arg("--exclude-dir=.git")
        .arg("--")
        .arg(pattern)
        .args(&roots)
        .current_dir(repo_root)
        .stdin(std::process::Stdio::null());
    let output = match command.output() {
        Ok(output) => output,
        Err(error) => {
            let mut row = invariant.clone();
            row.result = "verification_error".to_owned();
            row.evidence = Some(format!("grep failed to start: {error}"));
            return row;
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let code = output.status.code();
    let mut row = invariant.clone();
    match code {
        Some(1) => {
            row.result = "confirmed_absent".to_owned();
            row.evidence = Some("grep found zero matches".to_owned());
        }
        Some(0) => {
            let mut lines: Vec<&str> = stdout
                .trim()
                .lines()
                .filter(|line| !line.trim().is_empty())
                .collect();
            if lines.is_empty() {
                // GNU grep >= 3.5 reports a binary-file match on stderr with
                // empty stdout; fall back so the evidence names what it saw.
                lines = stderr
                    .trim()
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .collect();
            }
            let matches: Vec<&str> = if lines.is_empty() {
                vec!["(no grep output captured)"]
            } else {
                lines.into_iter().take(5).collect()
            };
            row.result = "still_present".to_owned();
            row.evidence = Some(format!("grep found matches: {}", matches.join("; ")));
        }
        other => {
            let details = if stderr.trim().is_empty() {
                stdout.trim()
            } else {
                stderr.trim()
            };
            let details: String = details.chars().take(500).collect();
            row.result = "verification_error".to_owned();
            row.evidence = Some(format!(
                "grep verification failed (exit {}): {details}",
                other.map_or_else(|| "signal".to_owned(), |code| code.to_string())
            ));
        }
    }
    row
}

fn judge_custom_command(repo_root: &Path, invariant: &NegativeInvariant) -> NegativeInvariant {
    let Some(command_line) = invariant
        .verification_command
        .as_deref()
        .filter(|command| !command.is_empty())
    else {
        let mut row = invariant.clone();
        row.result = VERDICT_PENDING.to_owned();
        row.evidence = Some("No command specified in verification_command".to_owned());
        return row;
    };
    // Source runs a configured command through an explicit POSIX shell; it never
    // uses `shell=True` from a string it did not quote itself.
    let output = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(command_line)
        .current_dir(repo_root)
        .stdin(std::process::Stdio::null())
        .output();
    let mut row = invariant.clone();
    match output {
        Err(error) => {
            row.result = "still_present".to_owned();
            row.evidence = Some(format!("Command failed to start: {error}"));
        }
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            if output.status.success() {
                row.result = "confirmed_absent".to_owned();
                row.evidence = Some(format!(
                    "Command exited 0: {}",
                    first_chars(stdout.trim(), 200)
                ));
            } else {
                let code = output
                    .status
                    .code()
                    .map_or_else(|| "signal".to_owned(), |code| code.to_string());
                row.result = "still_present".to_owned();
                row.evidence = Some(format!(
                    "Command exited {code}: {}",
                    first_chars(stderr.trim(), 200)
                ));
            }
        }
    }
    row
}

fn first_chars(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

/// Validate a row's provenance contract (Source `validate_invariant_provenance`).
///
/// `recorded` requires BOTH `verified_ref` and `verified_surface_kind` for a
/// terminal result (NI-1); an out-of-domain `result`/`provenance_origin` is an
/// error; a non-terminal row is exempt because it has no judgement to attribute.
pub fn validate_invariant_provenance(invariant: &NegativeInvariant) -> Vec<String> {
    let mut errors = Vec::new();
    if !TERMINAL_INVARIANT_RESULTS.contains(&invariant.result.as_str())
        && invariant.result != DEFERRED_TO_CONSOLIDATION
        && invariant.result != VERDICT_PENDING
    {
        let mut allowed = TERMINAL_INVARIANT_RESULTS.to_vec();
        allowed.push(VERDICT_PENDING);
        allowed.push(DEFERRED_TO_CONSOLIDATION);
        allowed.sort_unstable();
        errors.push(format!(
            "{}: result must be one of {}; got {}",
            invariant.invariant_id,
            allowed.join(", "),
            sk_kernel::python_json::python_repr(&invariant.result)
        ));
        return errors;
    }
    if !PROVENANCE_ORIGINS.contains(&invariant.provenance_origin.as_str()) {
        let mut allowed = PROVENANCE_ORIGINS.to_vec();
        allowed.sort_unstable();
        errors.push(format!(
            "{}: provenance_origin must be one of {}; got {}",
            invariant.invariant_id,
            allowed.join(", "),
            sk_kernel::python_json::python_repr(&invariant.provenance_origin)
        ));
        return errors;
    }
    if invariant.is_terminal()
        && invariant.provenance_origin == PROVENANCE_RECORDED
        && (invariant.verified_ref.is_none() || invariant.verified_surface_kind.is_none())
    {
        errors.push(format!(
            "{}: a recorded {} result requires both verified_ref and verified_surface_kind (NI-1)",
            invariant.invariant_id,
            sk_kernel::python_json::python_repr(&invariant.result)
        ));
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wp(wp_id: &str, lane: &str, reviewer: Option<&str>) -> WpAcceptanceInput {
        WpAcceptanceInput {
            wp_id: wp_id.to_owned(),
            lane: lane.to_owned(),
            implementer: Some("claude:opus:implementer".to_owned()),
            review: reviewer.map(|reviewer| ReviewEvidence {
                wp_id: wp_id.to_owned(),
                reviewer: reviewer.to_owned(),
                verdict: "approved".to_owned(),
                reference: format!("review-cycle-1:{wp_id}:approved"),
            }),
            owned_files: vec!["src/*".to_owned()],
            changed_files: vec!["src/lib.rs".to_owned()],
        }
    }

    fn inputs(work_packages: Vec<WpAcceptanceInput>) -> AcceptanceInputs {
        AcceptanceInputs {
            mission_slug: "demo".to_owned(),
            mission_number: Some(7),
            mission_type: Some("software-dev".to_owned()),
            tasks_md: Some("- [x] T001 done\n- [ ] prose checkbox\n".to_owned()),
            lanes_present: true,
            attribution_available: true,
            work_packages,
            canceled_with_provenance: Vec::new(),
            spec_requirement_ids: Vec::new(),
        }
    }

    #[test]
    fn declared_requirement_ids_are_the_source_declared_shapes() {
        let spec = "\
# Demo\n\
\n\
| Requirement | Detail |\n\
| --- | --- |\n\
| FR-001 | table row |\n\
| **NFR-002** | bold table row |\n\
\n\
### FR-003: headed requirement\n\
\n\
- FR-004 bullet\n\
1. **FR-005** numbered\n\
**FR-006**: bold lead\n\
- FR-007a suffixed\n\
- FR-008-mandated compound tail is not an id\n\
- FR-009.1 dotted sub-requirement is not an id\n\
See FR-021's default behaviour in prose.\n\
<!-- - FR-010 commented out -->\n\
- SC-011 success criterion\n\
- C-012 constraint\n\
- **FR-013 / FR-014**: only the first id on a line is declared\n";
        assert_eq!(
            parse_declared_requirement_ids(spec),
            vec![
                "C-012", "FR-001", "FR-003", "FR-004", "FR-005", "FR-006", "FR-007a", "FR-013",
                "NFR-002", "SC-011",
            ]
        );
    }

    #[test]
    fn declared_requirements_become_pending_criteria_and_review_populates_them() {
        let mut input = inputs(vec![wp("WP01", "done", Some("codex:gpt-5:reviewer"))]);
        input.spec_requirement_ids = vec!["FR-001".to_owned(), "FR-002".to_owned()];
        let report = evaluate_acceptance(&input);
        let ids: Vec<&str> = report
            .matrix
            .criteria
            .iter()
            .map(|row| row.criterion_id.as_str())
            .collect();
        assert!(ids.contains(&"FR-001"), "{ids:?}");
        assert!(ids.contains(&"FR-002"), "{ids:?}");
        let fr_rows: Vec<&AcceptanceCriterion> = report
            .matrix
            .criteria
            .iter()
            .filter(|row| row.criterion_id.starts_with("FR-"))
            .collect();
        assert!(
            fr_rows.iter().all(|row| row.pass_fail == VERDICT_PASS),
            "{fr_rows:?}"
        );
        assert_eq!(report.verdict(), VERDICT_PASS);
    }

    #[test]
    fn a_full_chain_passes_and_derives_the_matrix() {
        let report = evaluate_acceptance(&inputs(vec![wp(
            "WP01",
            "done",
            Some("codex:gpt-5:reviewer"),
        )]));
        assert!(report.ok(), "{:?}", report.gates);
        assert_eq!(report.verdict(), VERDICT_PASS);
        let row = &report.matrix.criteria[0];
        assert_eq!(row.pass_fail, VERDICT_PASS);
        assert_eq!(row.verified_by.as_deref(), Some("codex:gpt-5:reviewer"));
        assert_eq!(
            row.evidence.as_deref(),
            Some("WP01:review-cycle-1:WP01:approved")
        );
        assert!(report.unchecked_tasks.is_empty());
    }

    #[test]
    fn a_self_review_is_not_independence() {
        let report = evaluate_acceptance(&inputs(vec![wp(
            "WP01",
            "done",
            Some("claude:opus:implementer"),
        )]));
        assert!(!report.ok());
        let gate = report
            .gates
            .iter()
            .find(|gate| gate.id == "review_evidence")
            .expect("review gate");
        assert_eq!(gate.result, GateResult::Fail);
        assert!(gate.detail.contains("not independent"), "{}", gate.detail);
        assert_eq!(report.verdict(), VERDICT_PENDING);
    }

    #[test]
    fn a_missing_review_leaves_the_matrix_pending() {
        let report = evaluate_acceptance(&inputs(vec![wp("WP01", "approved", None)]));
        assert!(!report.ok());
        assert_eq!(report.verdict(), VERDICT_PENDING);
    }

    #[test]
    fn a_stale_rejection_never_becomes_approval_proof() {
        let mut package = wp("WP01", "done", Some("codex:gpt-5:reviewer"));
        package.review.as_mut().unwrap().verdict = "changes_requested".to_owned();
        let report = evaluate_acceptance(&inputs(vec![package]));
        assert!(!report.ok());
        assert_eq!(report.verdict(), VERDICT_PENDING);
        assert_eq!(report.matrix.criteria[0].pass_fail, VERDICT_PENDING);
    }

    #[test]
    fn an_unowned_changed_file_fails_attribution() {
        let mut package = wp("WP01", "done", Some("codex:gpt-5:reviewer"));
        package.changed_files.push("docs/readme.md".to_owned());
        let report = evaluate_acceptance(&inputs(vec![package]));
        assert!(!report.ok());
        let gate = report
            .gates
            .iter()
            .find(|gate| gate.id == "owned_file_evidence")
            .expect("owned gate");
        assert!(gate.detail.contains("docs/readme.md"));
    }

    #[test]
    fn unchecked_rows_are_fence_aware_and_task_scoped() {
        let text = "- [ ] T001 real\n- [ ] prose\n```\n- [ ] T002 fenced\n```\n- [] T003 tight\n- [ ] T12 short\n";
        assert_eq!(
            iter_unchecked_subtask_rows(text),
            vec!["- [ ] T001 real".to_owned(), "- [] T003 tight".to_owned()]
        );
    }

    #[test]
    fn terminal_work_packages_normalize_unticked_boxes() {
        let mut input = inputs(vec![wp("WP01", "done", Some("codex:gpt-5:reviewer"))]);
        input.tasks_md = Some("- [ ] T001 unticked\n".to_owned());
        let report = evaluate_acceptance(&input);
        assert!(report.ok(), "{:?}", report.gates);
        assert!(report.unchecked_tasks.is_empty());
    }

    #[test]
    fn a_non_terminal_work_package_keeps_unticked_boxes() {
        let mut input = inputs(vec![wp("WP01", "for_review", Some("codex:gpt-5:reviewer"))]);
        input.tasks_md = Some("- [ ] T001 unticked\n".to_owned());
        let report = evaluate_acceptance(&input);
        assert!(!report.ok());
        assert_eq!(
            report.unchecked_tasks,
            vec!["- [ ] T001 unticked".to_owned()]
        );
    }

    #[test]
    fn a_canceled_work_package_without_provenance_is_not_terminal() {
        let mut input = inputs(vec![wp("WP01", "canceled", Some("codex:gpt-5:reviewer"))]);
        assert!(!evaluate_acceptance(&input).ok());
        input.canceled_with_provenance = vec!["WP01".to_owned()];
        assert!(evaluate_acceptance(&input).ok());
    }

    #[test]
    fn the_matrix_writer_is_atomic_and_reads_back_exactly() {
        let temp = tempfile::tempdir().unwrap();
        let matrix = AcceptanceMatrix::new(
            "demo",
            vec![AcceptanceCriterion::code_review("WP01", "reviewed")],
        );
        let path = matrix.write_to(temp.path()).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes, matrix.to_bytes());
        assert!(
            String::from_utf8(bytes)
                .unwrap()
                .contains("\"overall_verdict\": \"pending\"")
        );
    }

    #[test]
    fn owned_globs_match_paths_and_nothing_more() {
        assert!(glob_matches("src/*", "src/lib.rs"));
        assert!(glob_matches("*.rs", "src/lib.rs"));
        assert!(glob_matches(
            "kitty-specs/m/tasks.md",
            "kitty-specs/m/tasks.md"
        ));
        assert!(!glob_matches("src/*.py", "src/lib.rs"));
        assert!(!glob_matches("", "src/lib.rs"));
    }
}
