//! Source merge-driver bodies: the seven registered `kitty-specs/**`
//! reconciliation drivers (#5119 / C-006).
//!
//! The Source implementation registers seven custom git merge drivers so that
//! both-sides-divergent mission bookkeeping reconciles instead of clobbering or
//! hard-conflicting under the mission→target `git merge --squash`
//! (`specify_cli/lanes/consolidation.py::_MERGE_DRIVERS`,
//! `specify_cli/consolidation/drivers.py::MERGE_DRIVER_BODIES`). This module is
//! the native port of those bodies. The registry here is the single table the
//! Git-config installation, the gitattributes activation and the in-process
//! conflict path all resolve against — there is no second copy.
//!
//! Two callers execute the SAME body, exactly as Source does:
//!
//! 1. the git-invoked subprocess (`spec-kitty merge-driver-<name> %O %A %B`),
//!    which the CLI adapter in `sk-cli` exposes; and
//! 2. the in-process replay used when git left a registered path unmerged
//!    (`specify_cli/consolidation/git_probes.py::RESolve_registered_driver`),
//!    which reads git's own stage-1/2/3 blobs and applies [`run_driver`].
//!
//! Git invokes a driver with `%O %A %B` = base / ours / theirs and expects the
//! merged result written to the `ours` (`%A`) path with exit 0. Under the squash
//! integration `ours` is the target checkout and `theirs` is the mission branch.
//!
//! # Path-injection hardening (#2970 / S2083)
//!
//! Every placeholder is externally supplied. Git materialises the three as
//! sibling temp files in ONE directory for a real merge, so
//! [`MergeDriverInputs::from_paths`] refuses a set that does not share a parent
//! directory before any read/write. An absolute path or `..` escape aimed at a
//! different directory is refused red-first without narrowing any reconciliation
//! rule.
//!
//! # Deliberate limits (named, not hidden)
//!
//! * `meta.json` serialization matches the canonical writer's
//!   `json.dumps(indent=2, ensure_ascii=False, sort_keys=True)` + newline.
//! * The acceptance-matrix driver recomputes `overall_verdict` with the same
//!   dominance rule as the canonical `sk-status` acceptance matrix; the
//!   acceptance matrix reader remains the authority for the row schema.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

mod exact;

use exact::Exact;

#[cfg(test)]
use serde_json::json;
use serde_json::{Map, Value};
use sk_kernel::python_json::{PythonJsonRenderOptions, PythonJsonValue};

// ---------------------------------------------------------------------------
// The registry (#5119 / C-006)
// ---------------------------------------------------------------------------

/// The single merge-driver registry: config identity, `gitattributes` routes,
/// and CLI names. It lives in the Git adapter (the Git-config authority); the
/// driver bodies here consume it rather than keeping a second copy.
pub use sk_git::consolidation::{
    MERGE_DRIVER_CONFIGS as MERGE_DRIVERS, MergeDriverConfig as MergeDriverSpec,
    merge_driver_attribute_lines,
};

/// Resolve a driver by its CLI name (`merge-driver-meta`, ...).
pub fn driver_spec(name: &str) -> Option<&'static MergeDriverSpec> {
    MERGE_DRIVERS
        .iter()
        .find(|spec| spec.driver_name() == Some(name))
}

/// Resolve the driver registered for a repository-relative path.
pub fn driver_spec_for_path(path: &str) -> Option<&'static MergeDriverSpec> {
    MERGE_DRIVERS.iter().find(|spec| {
        spec.patterns
            .iter()
            .any(|pattern| glob_matches(pattern, path))
    })
}

/// A gitattributes-style glob: `**` crosses segments, `*`/`?` do not.
///
/// This matches the subset of `gitattributes(5)` patterns the registry uses:
/// literal segments, `*` within a segment, `?` for one character, and `**`
/// for any number of path segments (including none).
pub fn glob_matches(pattern: &str, path: &str) -> bool {
    let pattern_segments: Vec<&str> = pattern.split('/').collect();
    let path_segments: Vec<&str> = path.split('/').collect();
    match_segments(&pattern_segments, &path_segments)
}

fn match_segments(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => {
            // `**` matches zero or more segments.
            for skip in 0..=path.len() {
                if match_segments(rest, &path[skip..]) {
                    return true;
                }
            }
            false
        }
        Some((segment, rest)) => match path.split_first() {
            None => false,
            Some((candidate, tail)) => {
                segment_matches(segment, candidate) && match_segments(rest, tail)
            }
        },
    }
}

fn segment_matches(pattern: &str, value: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let value: Vec<char> = value.chars().collect();
    segment_matches_chars(&pattern, &value)
}

fn segment_matches_chars(pattern: &[char], value: &[char]) -> bool {
    match pattern.split_first() {
        None => value.is_empty(),
        Some(('*', rest)) => {
            for skip in 0..=value.len() {
                if segment_matches_chars(rest, &value[skip..]) {
                    return true;
                }
            }
            false
        }
        Some(('?', rest)) => match value.split_first() {
            Some((_, tail)) => segment_matches_chars(rest, tail),
            None => false,
        },
        Some((literal, rest)) => match value.split_first() {
            Some((candidate, tail)) if candidate == literal => segment_matches_chars(rest, tail),
            _ => false,
        },
    }
}

// ---------------------------------------------------------------------------
// Driver contract
// ---------------------------------------------------------------------------

/// A driver body failure. `to_string()` is the exact stderr text a caller must
/// echo unchanged (Source [`MergeDriverError`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeDriverError {
    /// `%O`/`%A`/`%B` do not share a parent directory (#2970 / S2083).
    PathInjection {
        /// The resolved placeholder paths, for the diagnostic.
        paths: Vec<PathBuf>,
    },
    /// An unresolvable conflict or invalid input. The message is the exact
    /// operator-facing diagnostic.
    Message(String),
}

impl fmt::Display for MergeDriverError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PathInjection { paths } => write!(
                formatter,
                "refusing merge-driver invocation: %O/%A/%B do not share a parent \
                 directory ({paths:?}) — refused as a possible path-injection attempt (#2970)"
            ),
            Self::Message(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for MergeDriverError {}

impl MergeDriverError {
    fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }
}

/// One driver's three byte inputs. `None` is an absent file.
#[derive(Debug, Clone, Copy, Default)]
pub struct MergeDriverInputs<'a> {
    /// `%O` — the merge base.
    pub base: Option<&'a [u8]>,
    /// `%A` — ours (the target checkout under a squash).
    pub ours: Option<&'a [u8]>,
    /// `%B` — theirs (the mission branch under a squash).
    pub theirs: Option<&'a [u8]>,
}

impl MergeDriverInputs<'_> {
    /// Read the three placeholders, refusing a path-injection escape (#2970).
    ///
    /// The three *resolved* paths must share a parent directory. This is a
    /// zero-cost invariant for every legitimate git invocation (git writes the
    /// three temp files as siblings) and closes the S2083 findings.
    pub fn from_paths(
        base: &Path,
        ours: &Path,
        theirs: &Path,
    ) -> Result<(Self, PathBuf, PathBuf, PathBuf), MergeDriverError> {
        let resolved = [base.to_path_buf(), ours.to_path_buf(), theirs.to_path_buf()];
        let canonical: Vec<PathBuf> = resolved
            .iter()
            .map(|path| path.canonicalize().unwrap_or_else(|_| path.clone()))
            .collect();
        let parents: BTreeSet<PathBuf> = canonical
            .iter()
            .filter_map(|path| path.parent().map(Path::to_path_buf))
            .collect();
        if parents.len() > 1 {
            return Err(MergeDriverError::PathInjection { paths: canonical });
        }
        Ok((
            Self::default(),
            resolved[0].clone(),
            resolved[1].clone(),
            resolved[2].clone(),
        ))
    }
}

/// Is this a real, positive (`>= 1`) integer `mission_number`?
///
/// Source `is_assigned_mission_number`: a real `int`, NOT `bool`, value `>= 1`.
/// The serde value is a boundary view of the exact rule, so a positive arbitrary
/// integer is never rejected by a machine-width bound.
pub fn is_assigned_mission_number(value: Option<&Value>) -> bool {
    exact::assigned_mission_number(value.map(exact_from_serde).as_ref())
}

/// Fully encoded reconciled bytes and an optional stdout notice.
///
/// Every successful scalar preparation or physical publication returns real
/// bytes, never an empty placeholder for an unencoded JSON document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverOutcome {
    /// The complete reconciled content.
    pub bytes: Vec<u8>,
    /// The exact stdout notice, when the body emits one.
    pub notice: Option<String>,
}

/// One private execution plan: retain text until the caller's encode phase.
enum DriverPayload {
    Bytes(Vec<u8>),
    Text(sk_kernel::PythonString),
}
struct DriverPlan {
    payload: DriverPayload,
    notice: Option<String>,
}
impl DriverPlan {
    fn resolved(bytes: Vec<u8>) -> Self {
        Self {
            payload: DriverPayload::Bytes(bytes),
            notice: None,
        }
    }
    fn resolved_text(text: sk_kernel::PythonString) -> Self {
        Self {
            payload: DriverPayload::Text(text),
            notice: None,
        }
    }
    fn with_notice(mut self, notice: impl Into<String>) -> Self {
        self.notice = Some(notice.into());
        self
    }
    fn encode(self) -> Result<DriverOutcome, MergeDriverError> {
        let bytes = match self.payload {
            DriverPayload::Bytes(bytes) => bytes,
            DriverPayload::Text(text) => exact::encode_text(&text)?,
        };
        Ok(DriverOutcome {
            bytes,
            notice: self.notice,
        })
    }
}

/// Run one named driver body over three byte inputs.
///
/// Scalar preparation only: encoding errors occur without filesystem writes.
/// Source-compatible execution uses [`run_driver_into`] / [`run_driver_paths`]
/// so the same private plan is encoded only after the actual destination open.
pub fn run_driver(
    name: &str,
    inputs: MergeDriverInputs<'_>,
) -> Result<DriverOutcome, MergeDriverError> {
    plan_driver(name, inputs)?.encode()
}

fn plan_driver(name: &str, inputs: MergeDriverInputs<'_>) -> Result<DriverPlan, MergeDriverError> {
    match name {
        "merge-driver-event-log" => plan_event_log_driver(inputs),
        "merge-driver-meta" => plan_meta_driver(inputs),
        "merge-driver-traces" => plan_traces_driver(inputs),
        "merge-driver-issue-matrix" => plan_issue_matrix_driver(inputs),
        "merge-driver-acceptance-matrix" => plan_acceptance_matrix_driver(inputs),
        "merge-driver-review-cycle" => plan_review_cycle_driver(inputs),
        "merge-driver-decision-index" => plan_decision_index_driver(inputs),
        other => Err(MergeDriverError::message(format!(
            "unknown merge driver {other:?}; registered drivers are: {}",
            MERGE_DRIVERS
                .iter()
                .filter_map(|spec| spec.command.split_whitespace().nth(1))
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

macro_rules! scalar_driver {
    ($public:ident, $plan:ident) => {
        /// Prepare fully encoded bytes without writing a destination.
        /// Source execution uses `run_driver_into` to keep encoding after open.
        pub fn $public(inputs: MergeDriverInputs<'_>) -> Result<DriverOutcome, MergeDriverError> {
            $plan(inputs)?.encode()
        }
    };
}
scalar_driver!(run_event_log_driver, plan_event_log_driver);
scalar_driver!(run_meta_driver, plan_meta_driver);
scalar_driver!(run_traces_driver, plan_traces_driver);
scalar_driver!(run_issue_matrix_driver, plan_issue_matrix_driver);
scalar_driver!(run_acceptance_matrix_driver, plan_acceptance_matrix_driver);
scalar_driver!(run_review_cycle_driver, plan_review_cycle_driver);
scalar_driver!(run_decision_index_driver, plan_decision_index_driver);

/// Run one named driver against three paths, writing `ours` on success.
///
/// This is the git-invoked entry point (`%O %A %B`).
pub fn run_driver_paths(
    name: &str,
    base: &Path,
    ours: &Path,
    theirs: &Path,
) -> Result<DriverOutcome, MergeDriverError> {
    let (_, base, ours, theirs) = MergeDriverInputs::from_paths(base, ours, theirs)?;
    let base_bytes = read_optional(&base)?;
    let ours_bytes = read_optional(&ours)?;
    let theirs_bytes = read_optional(&theirs)?;
    run_driver_into(
        name,
        MergeDriverInputs {
            base: base_bytes.as_deref(),
            ours: ours_bytes.as_deref(),
            theirs: theirs_bytes.as_deref(),
        },
        &ours,
    )
}

/// Merge byte inputs and publish to an actual Source-compatible destination.
///
/// Shared by path entry and in-process consolidation: text remains unencoded
/// until the destination is opened/truncated. Parent creation is not invented.
pub fn run_driver_into(
    name: &str,
    inputs: MergeDriverInputs<'_>,
    ours: &Path,
) -> Result<DriverOutcome, MergeDriverError> {
    let plan = plan_driver(name, inputs)?;
    // PHYSICAL PHASE, Source-exact: `Path.write_text` opens (creating and
    // TRUNCATING) the destination FIRST and only then encodes the text. A lone
    // surrogate therefore fails with the destination already truncated — never
    // before the open, never a fabricated successful empty write.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(ours)
        .map_err(|error| {
            MergeDriverError::message(format!("could not open {}: {error}", ours.display()))
        })?;
    let outcome = plan.encode()?;
    std::io::Write::write_all(&mut file, &outcome.bytes).map_err(|error| {
        MergeDriverError::message(format!("could not write {}: {error}", ours.display()))
    })?;
    std::io::Write::flush(&mut file).map_err(|error| {
        MergeDriverError::message(format!("could not flush {}: {error}", ours.display()))
    })?;
    Ok(outcome)
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, MergeDriverError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(MergeDriverError::message(format!(
            "could not read {}: {error}",
            path.display()
        ))),
    }
}
/// Boundary: a legacy serde value into the exact model.
///
/// Only the pre-existing serde public surface uses this; driver bodies never do.
fn exact_from_serde(value: &Value) -> Exact {
    match value {
        Value::Null => Exact::Null,
        Value::Bool(flag) => Exact::Bool(*flag),
        Value::Number(number) => {
            let token = number.to_string();
            if token.bytes().any(|byte| matches!(byte, b'.' | b'e' | b'E')) {
                Exact::Float(token.parse::<f64>().unwrap_or(f64::NAN))
            } else {
                Exact::Integer(token)
            }
        }
        Value::String(text) => Exact::scalar(text),
        Value::Array(items) => Exact::Array(items.iter().map(exact_from_serde).collect()),
        Value::Object(map) => {
            let mut object = sk_kernel::python_json::PythonJsonDict::new();
            for (key, value) in map {
                object.insert(
                    sk_kernel::PythonString::from_scalars(key),
                    exact_from_serde(value),
                );
            }
            Exact::ExactObject(object)
        }
    }
}

/// Boundary: the exact model back to a legacy serde value.
fn serde_from_exact(value: &Exact) -> Result<Value, MergeDriverError> {
    value.to_serde().ok_or_else(|| {
        MergeDriverError::message(
            "the merged document holds data a serde_json::Value cannot carry (a lone surrogate or \
             a non-finite float); use the exact driver path"
                .to_owned(),
        )
    })
}

fn serde_map_from_exact(
    object: &sk_kernel::python_json::PythonJsonDict,
) -> Result<Map<String, Value>, MergeDriverError> {
    match serde_from_exact(&Exact::ExactObject(object.clone()))? {
        Value::Object(map) => Ok(map),
        _ => Err(MergeDriverError::message("not an object".to_owned())),
    }
}

/// The ONE Source-`json.dumps` byte encoder below every driver body.
///
/// Kernel owns the ordered value tree, the string encoder and the number
/// formatter; this function only adds the two knobs Source uses
/// (`indent`, `ensure_ascii`) and the trailing newline every driver writer
/// emits. There is no second JSON formatter in this crate.
fn source_json_bytes(
    value: &Value,
    indent: Option<usize>,
    ensure_ascii: bool,
) -> Result<Vec<u8>, MergeDriverError> {
    source_json_value_bytes(&PythonJsonValue::from_serde(value), indent, ensure_ascii)
}

fn source_json_value_bytes(
    value: &PythonJsonValue,
    indent: Option<usize>,
    ensure_ascii: bool,
) -> Result<Vec<u8>, MergeDriverError> {
    // ONE renderer, ONE options struct (J1's `PythonJsonRenderOptions`).
    let rendered = value.render_python_with(PythonJsonRenderOptions {
        indent,
        ensure_ascii,
        ..PythonJsonRenderOptions::default()
    });
    // The TEXT phase may hold a lone surrogate (`ensure_ascii=False` renders it
    // raw); the PHYSICAL phase is where Source's `Path.write_text` raises. The
    // failure is reported here, at the write, never by a lossy replacement and
    // never by panicking.
    let mut text = rendered.to_scalar_string().map_err(|unit| {
        MergeDriverError::message(format!(
            "the rendered document holds the lone surrogate U+{unit:04X} and cannot be written \
             as UTF-8 (Source raises UnicodeEncodeError at the same physical write)"
        ))
    })?;
    text.push('\n');
    Ok(text.into_bytes())
}

// ---------------------------------------------------------------------------
// merge-driver-event-log — status.events.jsonl / decisions.events.jsonl union
// ---------------------------------------------------------------------------

/// A parsed event-log row (a JSON object with a non-empty string `event_id`).
#[derive(Debug, Clone, PartialEq)]
pub struct EventRow {
    /// The event payload.
    pub payload: Map<String, Value>,
}

/// Parse JSONL event rows, requiring a valid `event_id` per non-blank line.
pub fn read_event_rows(
    bytes: Option<&[u8]>,
    source: &str,
) -> Result<Vec<EventRow>, MergeDriverError> {
    // ONE parser (the exact reader); the serde row is this API's boundary view.
    let rows = exact::read_event_rows(bytes, source)?;
    rows.iter()
        .map(|row| {
            Ok(EventRow {
                payload: serde_map_from_exact(row)?,
            })
        })
        .collect()
}

/// Union event payloads, dedupe by `event_id`, sort by timestamp then id.
///
/// The rule lives in the private `exact::merge_event_rows` model; this is its serde boundary view
/// of the same call (a serde map cannot carry a lone surrogate, so a caller that
/// reaches this view has already chosen the scalar surface).
pub fn merge_event_payloads<'a>(
    groups: impl IntoIterator<Item = &'a [EventRow]>,
) -> Result<Vec<EventRow>, MergeDriverError> {
    let groups: Vec<Vec<sk_kernel::python_json::PythonJsonDict>> = groups
        .into_iter()
        .map(|group| {
            group
                .iter()
                .map(
                    |row| match exact_from_serde(&Value::Object(row.payload.clone())) {
                        Exact::ExactObject(object) => object,
                        _ => unreachable!("a serde object converts to an exact object"),
                    },
                )
                .collect::<Vec<_>>()
        })
        .collect();
    let merged = exact::merge_event_rows(groups)?;
    merged
        .iter()
        .map(|row| {
            Ok(EventRow {
                payload: serde_map_from_exact(row)?,
            })
        })
        .collect()
}

/// The linear lifecycle order used by the concurrent-fork detector (#5967).
///
/// `blocked`/`canceled` are non-linear and the non-display sentinels never
/// appear on an edge (Source `_FORWARD_RANK`).
const FORWARD_LANES: [&str; 7] = [
    "planned",
    "claimed",
    "in_progress",
    "for_review",
    "in_review",
    "approved",
    "done",
];

fn forward_rank(lane: &str) -> Option<usize> {
    FORWARD_LANES
        .iter()
        .position(|candidate| *candidate == lane)
}

fn is_decision_destination(from_lane: &str, to_lane: &str) -> bool {
    if to_lane == "canceled" {
        return true;
    }
    match (forward_rank(from_lane), forward_rank(to_lane)) {
        (Some(source), Some(destination)) => destination < source,
        _ => false,
    }
}

/// Detect a concurrent non-forward fork that a union fold must not bury (#5967).
///
/// A fork is reported only when BOTH sides carry non-base transitions departing
/// the same `from_lane` to more than one distinct `to_lane`, and at least one
/// destination is a decision (a cancellation or a backward move). A purely
/// forward fork keeps both rows and is not reported.
pub fn detect_concurrent_fork(
    base: &[EventRow],
    ours: &[EventRow],
    theirs: &[EventRow],
) -> Option<String> {
    let to_exact = |rows: &[EventRow]| -> Vec<sk_kernel::python_json::PythonJsonDict> {
        rows.iter()
            .filter_map(
                |row| match exact_from_serde(&Value::Object(row.payload.clone())) {
                    Exact::ExactObject(object) => Some(object),
                    _ => None,
                },
            )
            .collect()
    };
    exact::detect_concurrent_fork(&to_exact(base), &to_exact(ours), &to_exact(theirs))
}

/// Serialize event rows as deterministic JSONL (`sort_keys=True`, library
/// separators, `ensure_ascii=True`) + newline.
///
/// Source writes each row with `json.dumps(row, sort_keys=True)`. The ordered
/// Kernel tree is built from the (sorted) payload map, so the member order is
/// the same; rendering through Kernel is what makes the non-ASCII escapes match
/// Source (`\u00e9`, not raw UTF-8) — the previous local formatter wrote the
/// UTF-8 bytes `serde_json` produced and was a second, divergent encoder.
pub fn write_event_rows(rows: &[EventRow]) -> Vec<u8> {
    let mut out = Vec::new();
    for row in rows {
        let value = Value::Object(row.payload.clone());
        // `ensure_ascii=True` renders any lone surrogate as its `\udcXX` escape,
        // so the text is ASCII and the physical write cannot fail here.
        out.extend_from_slice(
            &source_json_bytes(&value, None, true)
                .expect("an ASCII-rendered event row is always writable"),
        );
    }
    out
}

/// Merge `status.events.jsonl` conflict inputs using event-log semantics.
fn plan_event_log_driver(inputs: MergeDriverInputs<'_>) -> Result<DriverPlan, MergeDriverError> {
    // The exact tree is the model: values AND keys keep their code points
    // (surrogates included) through the union and the sort.
    let base = exact::read_event_rows(inputs.base, "base")?;
    let ours = exact::read_event_rows(inputs.ours, "ours")?;
    let theirs = exact::read_event_rows(inputs.theirs, "theirs")?;
    if let Some(fork) = exact::detect_concurrent_fork(&base, &ours, &theirs) {
        return Err(MergeDriverError::message(fork));
    }
    let merged = exact::merge_event_rows([base, ours, theirs])?;
    Ok(DriverPlan::resolved_text(exact::render_event_rows(&merged)))
}

// ---------------------------------------------------------------------------
// merge-driver-meta — meta.json field merge (FR-004)
// ---------------------------------------------------------------------------

/// The `acceptance_history` union field.
pub const ACCEPTANCE_HISTORY_FIELD: &str = "acceptance_history";

/// The VCS-lock meta keys (Source `kernel.vcs_lock.VCS_LOCK_META_FIELDS`): a
/// change confined to these keys is a lock stamp, not a content edit.
pub const VCS_LOCK_META_FIELDS: [&str; 2] = ["vcs", "vcs_locked_at"];

/// The coordination-flattening keys that move together.
pub const FLATTEN_COUPLED_KEYS: [&str; 3] = ["coordination_branch", "topology", "flattened"];

/// Target-authoritative `meta.json` keys.
///
/// Acceptance/VCS provenance plus the target-assigned lifecycle/merge fields
/// (`mission_number`, `status`, `baseline_merge_commit`, the `merged_*` block).
pub fn target_authoritative_meta_fields() -> BTreeSet<&'static str> {
    let mut fields: BTreeSet<&'static str> = ACCEPTANCE_PROVENANCE_FIELDS.into_iter().collect();
    fields.extend(["mission_number", "status", "baseline_merge_commit"]);
    fields.extend(MERGE_FIELDS);
    fields
}

/// The UTF-8 text of one optional side (`None` is the empty text).
fn utf8<'a>(bytes: Option<&'a [u8]>, side: &str) -> Result<&'a str, MergeDriverError> {
    match bytes {
        None => Ok(""),
        Some(bytes) => std::str::from_utf8(bytes).map_err(|error| {
            MergeDriverError::message(format!("{side} is not valid UTF-8 ({error})"))
        }),
    }
}

/// Acceptance provenance keys the target (accepted-newer) side owns.
pub const ACCEPTANCE_PROVENANCE_FIELDS: [&str; 7] = [
    "accepted_at",
    "accepted_by",
    "accepted_from_commit",
    "acceptance_mode",
    "accept_commit",
    "vcs",
    "vcs_locked_at",
];

/// The canonical `merged_*` block.
pub const MERGE_FIELDS: [&str; 6] = [
    "merged_at",
    "merged_by",
    "merged_into",
    "merged_strategy",
    "merged_push",
    "merged_commit",
];
/// Field-merge two `meta.json` payloads for `merge-driver-meta`.
///
/// Empty/absent `base` selects the Source **two-way** rule (the consolidation
/// pipeline's mission→target opt-out via
/// `SPEC_KITTY_META_MERGE_TWO_WAY`, because a squash records no ancestry); a
/// non-empty base uses per-unit three-way and only a genuine both-sides conflict
/// falls back to the target-authoritative/mission-authoritative precedence.
pub fn reconcile_meta_payloads(
    ours: &Map<String, Value>,
    theirs: &Map<String, Value>,
    base: Option<&Map<String, Value>>,
) -> Map<String, Value> {
    let to_exact = |map: &Map<String, Value>| match exact_from_serde(&Value::Object(map.clone())) {
        Exact::ExactObject(object) => object,
        _ => sk_kernel::python_json::PythonJsonDict::new(),
    };
    let ours = to_exact(ours);
    let theirs = to_exact(theirs);
    let base = base.map(to_exact);
    let merged = exact::reconcile_meta(
        &ours,
        &theirs,
        base.as_ref(),
        &[
            &FLATTEN_COUPLED_KEYS,
            &MERGE_FIELDS,
            &[
                "accepted_at",
                "accepted_by",
                "accepted_from_commit",
                "acceptance_mode",
                "accept_commit",
            ],
        ],
        &target_authoritative_meta_fields()
            .into_iter()
            .collect::<Vec<_>>(),
    );
    serde_map_from_exact(&merged).unwrap_or_default()
}

/// Field-merge conflicting `meta.json` blobs; write the result to `ours`.
fn plan_meta_driver(inputs: MergeDriverInputs<'_>) -> Result<DriverPlan, MergeDriverError> {
    let base = exact::parse_document(inputs.base, "base", "meta.json")?;
    let base_present = !base.is_empty();
    // A base-aware merge refuses a blank side: `{}` would read as "deleted every
    // key" and silently drop the other side's fields (#5460).
    if base_present {
        for (side, bytes) in [("ours", inputs.ours), ("theirs", inputs.theirs)] {
            if bytes.is_some_and(is_blank) {
                return Err(MergeDriverError::message(format!(
                    "{side}: meta.json is empty; refusing a base-aware merge that would \
                     treat it as deleting every key"
                )));
            }
        }
    }
    let ours = exact::parse_document(inputs.ours, "ours", "meta.json")?;
    let theirs = exact::parse_document(inputs.theirs, "theirs", "meta.json")?;
    let merged = exact::reconcile_meta(
        &ours,
        &theirs,
        base_present.then_some(&base),
        &[
            &FLATTEN_COUPLED_KEYS,
            &MERGE_FIELDS,
            &[
                "accepted_at",
                "accepted_by",
                "accepted_from_commit",
                "acceptance_mode",
                "accept_commit",
            ],
        ],
        &target_authoritative_meta_fields()
            .into_iter()
            .collect::<Vec<_>>(),
    );
    // `mission_metadata.write_meta`: `indent=2, ensure_ascii=False,
    // sort_keys=True` + newline — rendered by the ONE encoder, encoded last.
    Ok(DriverPlan::resolved_text(exact::render(
        &exact::Exact::ExactObject(merged),
        Some(2),
        false,
        true,
    )))
}

fn is_blank(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_ok_and(|text| {
        text.chars()
            .all(|character| sk_kernel::python_text::is_whitespace(u32::from(character)))
    })
}

// ---------------------------------------------------------------------------
// merge-driver-traces — traces/*.md section-granularity union (#4894)
// ---------------------------------------------------------------------------

fn trace_section_id(line: &str) -> Option<String> {
    let rest = line.strip_prefix("<!--")?.trim_start();
    let body = rest.strip_prefix("section:")?.trim_end();
    let body = body.strip_suffix("-->")?;
    Some(body.trim_end().to_owned())
}

fn is_trace_boundary(line: &str) -> bool {
    if trace_section_id(line).is_some() {
        return true;
    }
    let hashes = line.chars().take_while(|value| *value == '#').count();
    if !(1..=6).contains(&hashes) {
        return false;
    }
    let rest = &line[hashes..];
    rest.chars().next().is_some_and(char::is_whitespace) && !rest.trim().is_empty()
}

fn trace_fence(line: &str) -> Option<(char, usize)> {
    let character = line.chars().next()?;
    if character != '`' && character != '~' {
        return None;
    }
    let run = line.chars().take_while(|value| *value == character).count();
    (run >= 3).then_some((character, run))
}

fn split_trace_blocks(text: &str) -> Vec<Vec<&str>> {
    let mut blocks: Vec<Vec<&str>> = vec![Vec::new()];
    let mut open_fence: Option<(char, usize)> = None;
    for line in text.lines() {
        if open_fence.is_none() && is_trace_boundary(line) {
            blocks.push(Vec::new());
        }
        if let Some((character, run)) = trace_fence(line) {
            match open_fence {
                None => open_fence = Some((character, run)),
                Some((open, minimum)) if open == character && run >= minimum => open_fence = None,
                Some(_) => {}
            }
        }
        blocks
            .last_mut()
            .expect("the block list always has one open block")
            .push(line);
    }
    blocks.retain(|block| !block.is_empty());
    blocks
}

fn render_trace_blocks<'a>(blocks: impl IntoIterator<Item = &'a [&'a str]>) -> String {
    let mut lines: Vec<&str> = Vec::new();
    for block in blocks {
        lines.extend(block.iter().copied());
    }
    if lines.is_empty() {
        String::new()
    } else {
        let mut text = lines.join("\n");
        text.push('\n');
        text
    }
}

/// Union two append-only trace documents at section granularity.
pub fn union_trace_texts(ours: &str, theirs: &str) -> String {
    let ours_blocks = split_trace_blocks(ours);
    let mut seen: BTreeSet<Vec<&str>> = ours_blocks.iter().cloned().collect();
    let mut merged_blocks: Vec<Vec<&str>> = ours_blocks;
    for block in split_trace_blocks(theirs) {
        if seen.contains(&block) {
            continue;
        }
        seen.insert(block.clone());
        merged_blocks.push(block);
    }
    render_trace_blocks(merged_blocks.iter().map(|block| block.as_slice()))
}

fn trace_block_key<'a>(block: &'a [&'a str], ordinal: usize) -> (String, String, usize) {
    let first = block.first().copied().unwrap_or_default();
    match trace_section_id(first) {
        Some(id) => ("id".to_owned(), id, ordinal),
        None => ("line".to_owned(), first.to_owned(), ordinal),
    }
}

fn trace_blocks_with_ordinal(text: &str) -> Vec<(Vec<&str>, usize)> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut found = Vec::new();
    for block in split_trace_blocks(text) {
        let first = block.first().copied().unwrap_or_default();
        let key = match trace_section_id(first) {
            Some(id) => format!("id:{id}"),
            None => format!("line:{first}"),
        };
        let ordinal = counts.get(&key).copied().unwrap_or(0);
        counts.insert(key, ordinal + 1);
        found.push((block, ordinal));
    }
    found
}

fn index_trace_blocks(text: &str) -> BTreeMap<(String, String, usize), Vec<&str>> {
    let mut indexed = BTreeMap::new();
    for (block, ordinal) in trace_blocks_with_ordinal(text) {
        indexed
            .entry(trace_block_key(&block, ordinal))
            .or_insert(block);
    }
    indexed
}

fn drop_stale_theirs_blocks(base: &str, ours: &str, theirs: &str) -> String {
    let base_by_key = index_trace_blocks(base);
    let ours_by_key = index_trace_blocks(ours);
    let mut kept: Vec<Vec<&str>> = Vec::new();
    for (block, ordinal) in trace_blocks_with_ordinal(theirs) {
        let key = trace_block_key(&block, ordinal);
        let base_block = base_by_key.get(&key);
        let ours_block = ours_by_key.get(&key);
        let theirs_unchanged = base_block.is_some_and(|base_block| &block == base_block);
        let ours_diverged = ours_block
            .is_some_and(|ours_block| base_block.is_none_or(|base_block| ours_block != base_block));
        if theirs_unchanged && ours_diverged {
            continue;
        }
        kept.push(block);
    }
    render_trace_blocks(kept.iter().map(|block| block.as_slice()))
}

/// Union conflicting `traces/*.md` documents, dropping theirs' stale sections.
fn plan_traces_driver(inputs: MergeDriverInputs<'_>) -> Result<DriverPlan, MergeDriverError> {
    let base = utf8(inputs.base, "base")?;
    let ours = utf8(inputs.ours, "ours")?;
    let theirs = utf8(inputs.theirs, "theirs")?;
    let filtered = drop_stale_theirs_blocks(base, ours, theirs);
    Ok(DriverPlan::resolved(
        union_trace_texts(ours, &filtered).into_bytes(),
    ))
}

// ---------------------------------------------------------------------------
// Row-aware, base-aware (3-way) matrix merge (FR-008)
// ---------------------------------------------------------------------------

/// 3-way, row-aware reconciliation of an `issue-matrix.json` document.
pub fn reconcile_issue_matrix_documents(
    base: &Map<String, Value>,
    ours: &Map<String, Value>,
    theirs: &Map<String, Value>,
) -> Result<Value, MergeDriverError> {
    let to_exact = |map: &Map<String, Value>| match exact_from_serde(&Value::Object(map.clone())) {
        Exact::ExactObject(object) => object,
        _ => sk_kernel::python_json::PythonJsonDict::new(),
    };
    let merged = exact::union_issue_matrix(&to_exact(base), &to_exact(ours), &to_exact(theirs))?;
    serde_from_exact(&merged)
}

/// Row-aware, 3-way merge of `issue-matrix.json`.
fn plan_issue_matrix_driver(inputs: MergeDriverInputs<'_>) -> Result<DriverPlan, MergeDriverError> {
    let base = exact::parse_document(inputs.base, "base", "issue-matrix.json")?;
    let ours = exact::parse_document(inputs.ours, "ours", "issue-matrix.json")?;
    let theirs = exact::parse_document(inputs.theirs, "theirs", "issue-matrix.json")?;
    let merged = exact::union_issue_matrix(&base, &ours, &theirs)?;
    // `json.dumps(merged, indent=2, sort_keys=True)` + newline.
    Ok(DriverPlan::resolved_text(exact::render(
        &merged,
        Some(2),
        true,
        true,
    )))
}

/// The acceptance-matrix `overall_verdict` dominance rule.
///
/// Mirrors `sk_status::acceptance::AcceptanceMatrix::overall_verdict` exactly:
/// invalid rows fail, a fail/still-present/verification-error fails, and pending
/// dominates a deferred-to-consolidation row.
pub fn acceptance_overall_verdict(document: &Map<String, Value>) -> &'static str {
    match exact_from_serde(&Value::Object(document.clone())) {
        Exact::ExactObject(object) => exact::acceptance_verdict(&object),
        _ => "pending",
    }
}

/// 3-way, row-aware reconciliation of an `acceptance-matrix.json` document.
pub fn reconcile_acceptance_matrix_documents(
    base: &Map<String, Value>,
    ours: &Map<String, Value>,
    theirs: &Map<String, Value>,
) -> Result<Value, MergeDriverError> {
    let to_exact = |map: &Map<String, Value>| match exact_from_serde(&Value::Object(map.clone())) {
        Exact::ExactObject(object) => object,
        _ => sk_kernel::python_json::PythonJsonDict::new(),
    };
    let merged = exact::acceptance_document(&to_exact(base), &to_exact(ours), &to_exact(theirs))?;
    serde_from_exact(&merged)
}

// ---------------------------------------------------------------------------
// merge-driver-review-cycle — verbatim collision embed, non-aborting
// ---------------------------------------------------------------------------
const CONFLICT_MARKER_OURS: &str = "<<<<<<< ours";
const CONFLICT_MARKER_SEP: &str = "=======";
const CONFLICT_MARKER_THEIRS: &str = ">>>>>>> theirs";

/// Row-aware, 3-way merge of `acceptance-matrix.json`.
fn plan_acceptance_matrix_driver(
    inputs: MergeDriverInputs<'_>,
) -> Result<DriverPlan, MergeDriverError> {
    let base = exact::parse_document(inputs.base, "base", "acceptance-matrix.json")?;
    let ours = exact::parse_document(inputs.ours, "ours", "acceptance-matrix.json")?;
    let theirs = exact::parse_document(inputs.theirs, "theirs", "acceptance-matrix.json")?;
    let merged = exact::acceptance_document(&base, &ours, &theirs)?;
    // `json.dumps(merged, indent=2)` + newline: `sort_keys=False` keeps
    // `AcceptanceMatrix.to_dict()` order, extras in the sides' own order.
    Ok(DriverPlan::resolved_text(exact::render(
        &merged,
        Some(2),
        true,
        false,
    )))
}

/// Reconcile a `review-cycle-N.md` collision: best-effort, non-aborting.
///
/// Identical content (including both sides absent) resolves cleanly with no
/// notice. Otherwise both raw documents are embedded VERBATIM behind git-style
/// conflict markers — never blended, interleaved or renumbered — and the driver
/// still succeeds (`FR-014/D-PLAN-6`: the `.md` render is non-authoritative read
/// prose), returning a notice so the caller can surface the collision.
///
/// Source reads UTF-8 text with universal newlines before comparison. Decode
/// failures must leave the output untouched, not embed invalid input as success.
fn plan_review_cycle_driver(inputs: MergeDriverInputs<'_>) -> Result<DriverPlan, MergeDriverError> {
    let read = |bytes: Option<&[u8]>, side: &str| {
        std::str::from_utf8(bytes.unwrap_or_default())
            .map(|text| sk_kernel::python_text::universal_newlines(text).into_owned())
            .map_err(|error| {
                MergeDriverError::message(format!(
                    "{side}: review cycle is not valid UTF-8 ({error})"
                ))
            })
    };
    let ours = read(inputs.ours, "ours")?;
    let theirs = read(inputs.theirs, "theirs")?;
    if ours == theirs {
        return Ok(DriverPlan::resolved(ours.into_bytes()));
    }
    // Source embeds `<<<<<<< ours\n<ours>\n=======\n<theirs>\n>>>>>>> theirs` with
    // the raw documents (their own trailing newline included) and NO trailing
    // newline after the closing marker.
    let mut embedding = Vec::new();
    embedding.extend_from_slice(CONFLICT_MARKER_OURS.as_bytes());
    embedding.push(b'\n');
    embedding.extend_from_slice(ours.as_bytes());
    embedding.extend_from_slice(b"\n");
    embedding.extend_from_slice(CONFLICT_MARKER_SEP.as_bytes());
    embedding.push(b'\n');
    embedding.extend_from_slice(theirs.as_bytes());
    embedding.extend_from_slice(b"\n");
    embedding.extend_from_slice(CONFLICT_MARKER_THEIRS.as_bytes());
    Ok(DriverPlan::resolved(embedding).with_notice(
        "review-cycle collision embedded verbatim (both verdicts kept; resolve by hand)",
    ))
}

/// Exact raw union, BEFORE the driver's typed `DecisionIndex` projection.
///
/// Carries Python code-point strings, arbitrary integers and nonfinite floats
/// through the one union rule; no version default or filesystem write is added.
pub fn union_decision_index_exact(
    ours: &sk_kernel::python_json::PythonJsonDict,
    theirs: &sk_kernel::python_json::PythonJsonDict,
) -> Result<sk_kernel::python_json::PythonJsonValue, MergeDriverError> {
    exact::union_decision_entries(ours, theirs)
}

/// Scalar serde capability view of [`union_decision_index_exact`].
/// Non-scalar/nonfinite results cannot be represented by this facade; the exact
/// API and actual physical driver do not inherit that capability limitation.
pub fn union_decision_index(
    ours: &Map<String, Value>,
    theirs: &Map<String, Value>,
) -> Result<Map<String, Value>, MergeDriverError> {
    let to_exact = |map: &Map<String, Value>| match exact_from_serde(&Value::Object(map.clone())) {
        Exact::ExactObject(object) => object,
        _ => sk_kernel::python_json::PythonJsonDict::new(),
    };
    let merged = union_decision_index_exact(&to_exact(ours), &to_exact(theirs))?;
    match serde_from_exact(&merged)? {
        Value::Object(map) => Ok(map),
        _ => Err(MergeDriverError::message(
            "the merged decision index is not an object".to_owned(),
        )),
    }
}

/// Union `decisions/index.json` entries keyed by `decision_id`.
fn plan_decision_index_driver(
    inputs: MergeDriverInputs<'_>,
) -> Result<DriverPlan, MergeDriverError> {
    let ours = exact::parse_document(inputs.ours, "ours", "decisions/index.json")?;
    let theirs = exact::parse_document(inputs.theirs, "theirs", "decisions/index.json")?;
    let merged = exact::union_decision_index(&ours, &theirs)?;
    // `json.dumps(..., sort_keys=True, indent=2)` + newline: document keys are
    // sorted while the `entries` ARRAY keeps the `(created_at, decision_id)`
    // order `save_index` writes.
    Ok(DriverPlan::resolved_text(exact::render(
        &merged,
        Some(2),
        true,
        true,
    )))
}
#[cfg(test)]
mod tests {
    use super::*;

    fn object(pairs: &[(&str, Value)]) -> Map<String, Value> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect()
    }

    #[test]
    fn the_registry_is_the_seven_source_drivers_in_order() {
        let names: Vec<&str> = MERGE_DRIVERS
            .iter()
            .filter_map(|spec| spec.command.split_whitespace().nth(1))
            .collect();
        assert_eq!(
            names,
            vec![
                "merge-driver-event-log",
                "merge-driver-meta",
                "merge-driver-traces",
                "merge-driver-acceptance-matrix",
                "merge-driver-issue-matrix",
                "merge-driver-review-cycle",
                "merge-driver-decision-index",
            ]
        );
        assert_eq!(
            merge_driver_attribute_lines(),
            vec![
                "kitty-specs/**/status.events.jsonl merge=spec-kitty-event-log",
                "kitty-specs/**/decisions.events.jsonl merge=spec-kitty-event-log",
                "kitty-specs/**/meta.json merge=spec-kitty-meta",
                "kitty-specs/**/traces/*.md merge=spec-kitty-traces",
                "kitty-specs/**/acceptance-matrix.json merge=spec-kitty-acceptance-matrix",
                "kitty-specs/**/issue-matrix.json merge=spec-kitty-issue-matrix",
                "kitty-specs/**/tasks/*/review-cycle-*.md merge=spec-kitty-review-cycle",
                "kitty-specs/**/decisions/index.json merge=spec-kitty-decision-index",
            ]
        );
    }

    #[test]
    fn globs_match_the_registry_patterns_and_not_neighbours() {
        assert!(glob_matches(
            "kitty-specs/**/meta.json",
            "kitty-specs/demo-01ABC/meta.json"
        ));
        assert!(glob_matches(
            "kitty-specs/**/meta.json",
            "kitty-specs/a/b/c/meta.json"
        ));
        assert!(!glob_matches(
            "kitty-specs/**/meta.json",
            "kitty-specs/demo-01ABC/meta.json.bak"
        ));
        assert!(glob_matches(
            "kitty-specs/**/tasks/*/review-cycle-*.md",
            "kitty-specs/demo/tasks/WP01/review-cycle-3.md"
        ));
        assert!(!glob_matches(
            "kitty-specs/**/tasks/*/review-cycle-*.md",
            "kitty-specs/demo/tasks/WP01/notes.md"
        ));
        assert_eq!(
            driver_spec_for_path("kitty-specs/demo/status.events.jsonl")
                .map(|spec| spec.config_key),
            Some("spec-kitty-event-log")
        );
    }

    #[test]
    fn meta_two_way_prefers_the_target_for_target_authoritative_fields() {
        let ours = object(&[
            ("accepted_at", json!("2026-01-02T00:00:00Z")),
            ("status", json!("merged")),
            ("mission_number", json!(42)),
            ("slug", json!("target-slug")),
            (
                "acceptance_history",
                json!([{"accepted_by": "b", "accepted_at": "2"}]),
            ),
        ]);
        let theirs = object(&[
            ("accepted_at", json!("2026-01-01T00:00:00Z")),
            ("status", json!("accepted")),
            ("mission_number", Value::Null),
            ("slug", json!("mission-slug")),
            ("target_branch", json!("main")),
            (
                "acceptance_history",
                json!([{"accepted_by": "a", "accepted_at": "1"}]),
            ),
        ]);
        let merged = reconcile_meta_payloads(&ours, &theirs, None);
        assert_eq!(
            merged.get("accepted_at"),
            Some(&json!("2026-01-02T00:00:00Z"))
        );
        assert_eq!(merged.get("status"), Some(&json!("merged")));
        assert_eq!(merged.get("mission_number"), Some(&json!(42)));
        assert_eq!(merged.get("slug"), Some(&json!("mission-slug")));
        assert_eq!(merged.get("target_branch"), Some(&json!("main")));
        assert_eq!(
            merged.get("acceptance_history"),
            Some(&json!([
                {"accepted_by": "a", "accepted_at": "1"},
                {"accepted_by": "b", "accepted_at": "2"},
            ]))
        );
    }

    #[test]
    fn meta_three_way_keeps_a_one_sided_removal_and_unions_history() {
        let base = object(&[
            ("slug", json!("s")),
            ("discarded_at", json!("old")),
            ("merged_at", json!("x")),
        ]);
        let ours = object(&[("slug", json!("s")), ("merged_at", json!("target"))]);
        let theirs = object(&[("slug", json!("s")), ("merged_at", json!("mission"))]);
        let merged = reconcile_meta_payloads(&ours, &theirs, Some(&base));
        assert_eq!(merged.get("discarded_at"), None, "ours' removal survives");
        assert_eq!(merged.get("merged_at"), Some(&json!("target")));
    }

    #[test]
    fn meta_three_way_refuses_a_blank_side() {
        let error = run_meta_driver(MergeDriverInputs {
            base: Some(b"{\"slug\":\"s\"}"),
            ours: Some(b"   \n"),
            theirs: Some(b"{\"slug\":\"s\"}"),
        })
        .unwrap_err();
        assert!(error.to_string().contains("meta.json is empty"), "{error}");
    }

    #[test]
    fn event_log_unions_dedupes_and_detects_a_buried_cancel() {
        let base = br#"{"event_id":"e1","at":"1","wp_id":"WP01","from_lane":"planned","to_lane":"in_progress"}
"#;
        let ours = br#"{"event_id":"e1","at":"1","wp_id":"WP01","from_lane":"planned","to_lane":"in_progress"}
{"event_id":"e2","at":"2","wp_id":"WP01","from_lane":"in_progress","to_lane":"canceled"}
"#;
        let theirs = br#"{"event_id":"e1","at":"1","wp_id":"WP01","from_lane":"planned","to_lane":"in_progress"}
{"event_id":"e3","at":"3","wp_id":"WP01","from_lane":"in_progress","to_lane":"for_review"}
"#;
        let error = run_event_log_driver(MergeDriverInputs {
            base: Some(base),
            ours: Some(ours),
            theirs: Some(theirs),
        })
        .unwrap_err();
        assert!(
            error.to_string().contains("concurrent transitions"),
            "{error}"
        );

        let forward_theirs = br#"{"event_id":"e3","at":"3","wp_id":"WP01","from_lane":"planned","to_lane":"for_review"}
"#;
        let merged = run_event_log_driver(MergeDriverInputs {
            base: Some(base),
            ours: Some(ours),
            theirs: Some(forward_theirs),
        })
        .unwrap();
        let merged = merged.bytes;
        let text = String::from_utf8(merged).unwrap();
        // Source writes rows with `json.dumps(row, sort_keys=True)`: the default
        // `", "` / `": "` separators, not a compact stream.
        assert!(text.contains("\"event_id\": \"e3\""), "{text}");
        assert_eq!(text.lines().count(), 3);
    }

    #[test]
    fn event_log_refuses_two_payloads_for_one_id() {
        let ours = br#"{"event_id":"e1","at":"1","value":1}
"#;
        let theirs = br#"{"event_id":"e1","at":"1","value":2}
"#;
        let error = run_event_log_driver(MergeDriverInputs {
            base: None,
            ours: Some(ours),
            theirs: Some(theirs),
        })
        .unwrap_err();
        assert!(
            error.to_string().contains("Conflicting payloads"),
            "{error}"
        );
    }

    #[test]
    fn traces_union_is_section_granular_and_keeps_both_sides() {
        let ours = "## A\n```\n## not a heading\n```\n## B\nours body\n";
        let theirs = "## A\n```\n## not a heading\n```\n## C\ntheirs body\n";
        let merged = union_trace_texts(ours, theirs);
        assert!(merged.contains("## A"));
        assert!(merged.contains("## B"));
        assert!(merged.contains("## C"));
        assert!(merged.contains("## not a heading"));
        assert_eq!(merged.matches("## A").count(), 1);
    }

    #[test]
    fn review_cycle_embeds_both_verdicts_without_blending() {
        let merged = run_review_cycle_driver(MergeDriverInputs {
            base: None,
            ours: Some(b"verdict: pass"),
            theirs: Some(b"verdict: fail"),
        })
        .unwrap();
        // The review-cycle driver is TEXT, not JSON: its bytes are already final.
        let text = String::from_utf8(merged.bytes.clone()).unwrap();
        assert!(text.starts_with(CONFLICT_MARKER_OURS));
        assert!(text.contains("verdict: pass"));
        assert!(text.contains("verdict: fail"));
        assert!(merged.notice.is_some());

        let same = run_review_cycle_driver(MergeDriverInputs {
            base: None,
            ours: Some(b"same"),
            theirs: Some(b"same"),
        })
        .unwrap();
        assert_eq!(same.bytes, b"same");
        assert!(same.notice.is_none());
    }

    #[test]
    fn issue_matrix_rows_merge_by_canonical_issue_ref() {
        let ours = json!({"rows": {"GH-7": {"verdict": "pass"}}});
        let theirs = json!({"rows": {"#7": {"evidence": "x"}, "#9": {"verdict": "unknown"}}});
        let merged = reconcile_issue_matrix_documents(
            &Map::new(),
            ours.as_object().unwrap(),
            theirs.as_object().unwrap(),
        )
        .unwrap();
        let rows = merged.get("rows").unwrap().as_object().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows.get("#7").unwrap().get("verdict"), Some(&json!("pass")));
        assert_eq!(
            rows.get("#9").unwrap().get("verdict"),
            Some(&json!("unknown"))
        );
    }

    #[test]
    fn acceptance_matrix_merges_disjoint_rows_and_recomputes_the_verdict() {
        let ours = json!({
            "mission_slug": "demo",
            "criteria": [{"criterion_id": "FR-001", "description": "Verify FR-001", "proof_type": "code_review", "pass_fail": "pass"}],
            "negative_invariants": [],
        });
        let theirs = json!({
            "mission_slug": "demo",
            "criteria": [{"criterion_id": "FR-002", "description": "Verify FR-002", "proof_type": "code_review", "pass_fail": "pending"}],
            "negative_invariants": [],
        });
        let merged = reconcile_acceptance_matrix_documents(
            &Map::new(),
            ours.as_object().unwrap(),
            theirs.as_object().unwrap(),
        )
        .unwrap();
        assert_eq!(merged.get("overall_verdict"), Some(&json!("pending")));
        assert_eq!(merged.get("criteria").unwrap().as_array().unwrap().len(), 2);
    }

    #[test]
    fn acceptance_matrix_fails_closed_on_two_authored_verdicts() {
        let base = json!({"criteria": [{"criterion_id": "FR-001", "pass_fail": "pending"}]});
        let ours = json!({"criteria": [{"criterion_id": "FR-001", "pass_fail": "pass"}]});
        let theirs = json!({"criteria": [{"criterion_id": "FR-001", "pass_fail": "fail"}]});
        let error = reconcile_acceptance_matrix_documents(
            base.as_object().unwrap(),
            ours.as_object().unwrap(),
            theirs.as_object().unwrap(),
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("diverged on both sides"),
            "{error}"
        );
    }

    #[test]
    fn decision_index_unions_with_terminal_beating_open() {
        let ours = json!({"entries": [{"decision_id": "D1", "status": "open", "created_at": "1"}]});
        let theirs =
            json!({"entries": [{"decision_id": "D1", "status": "resolved", "created_at": "1"}]});
        let merged =
            union_decision_index(ours.as_object().unwrap(), theirs.as_object().unwrap()).unwrap();
        let entries = merged.get("entries").unwrap().as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].get("status"), Some(&json!("resolved")));
    }

    #[test]
    fn path_injection_is_refused_before_any_read() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let base = first.path().join("base");
        let ours = first.path().join("ours");
        let theirs = second.path().join("theirs");
        let error = MergeDriverInputs::from_paths(&base, &ours, &theirs).unwrap_err();
        assert!(matches!(error, MergeDriverError::PathInjection { .. }));
    }
}
