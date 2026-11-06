//! Post-merge retrospective capture — the native port of the pinned Source
//! generator (`retrospective/generator.py` + `retrospective/writer.py`).
//!
//! The record is generated from the Mission's real artifacts in Source's
//! canonical read order (`meta.json`, `spec.md`, `plan.md`, `research.md`,
//! `data-model.md`, `tasks.md`, `tasks/WP*.md`, `status.events.jsonl`,
//! `workflow-failures-log.md`, `analysis-report.md`,
//! `mission-review-report.md`) and written to
//! `kitty-specs/<slug>/retrospective.yaml`. Absent optional artifacts become
//! `gaps` entries, never an error.
//!
//! Every rule here is the Source rule it names, applied to the values the
//! artifacts actually hold: the finding ids are the Source id sequences
//! (`h-`/`n-`/`g-`/`e-`), the findings are sorted by `(category, summary)` byte
//! order, and `findings_status` is `has_findings`/`ran_no_findings` from the
//! actual buckets.
//!
//! The tracer ingestor (FR-007, `retrospective/generator.py::_load_traces` /
//! `_parse_trace_entries` / `_classify_trace_disposition` /
//! `_build_trace_findings`) is implemented: every `traces/*.md` entry becomes one
//! `tooling` finding routed to `helped`/`not_helpful`/`gaps` by its documented
//! disposition markers.
//!
//! Proposal generation is NOT implemented here, and that is Source's own state:
//! `_build_findings` initialises `proposals = []` and never appends to it
//! (finding-derived payload generation is tracked upstream as #3947), so a
//! Source record carries `proposals: []` whatever `generate_proposals` says. The
//! empty list here is therefore the same fact, not a policy decision — the
//! resolved policy still records `generate_proposals` in `policy_source`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Source `GENERATOR_VERSION`.
pub const GENERATOR_VERSION: &str = "1.0";

/// The record file name (`core.constants.RETROSPECTIVE_FILENAME`).
pub const RETROSPECTIVE_FILENAME: &str = "retrospective.yaml";

/// A retrospective capture failure.
#[derive(Debug, thiserror::Error)]
pub enum RetrospectiveError {
    #[error("could not read {path}: {source}")]
    /// An artifact could not be read.
    Read {
        /// The path that could not be read.
        path: String,
        #[source]
        /// The underlying I/O failure.
        source: std::io::Error,
    },
    #[error("could not write {path}: {source}")]
    /// The record could not be written.
    Write {
        /// The path that could not be written.
        path: String,
        #[source]
        /// The underlying I/O failure.
        source: std::io::Error,
    },
    #[error("retrospective record could not be serialized: {0}")]
    /// The record could not be serialized to YAML.
    Serialize(String),
    #[error(
        "mission directory {0} does not exist; there is no artifact surface to generate the retrospective record from"
    )]
    /// The Mission directory does not exist.
    MissionMissing(String),
}

/// One `evidence_refs` entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvidenceRef {
    /// The reference id (`e-001`, ...).
    pub id: String,
    /// `file` or `event_range`.
    pub kind: String,
    /// The repo-relative artifact path.
    pub path: Option<String>,
    /// The `first..last` event-id range of an `event_range` reference.
    pub range: Option<String>,
    /// A URL reference (never emitted by the generator today).
    pub url: Option<String>,
}

/// One `helped`/`not_helpful`/`gaps` finding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    /// The finding id (`h-001` / `n-001` / `g-001`).
    pub id: String,
    /// The Source category (`implementation`, `process`, `doc`, ...).
    pub category: String,
    /// The one-line summary.
    pub summary: String,
    /// The Source detail prose, when the rule records one.
    pub details: Option<String>,
    /// The `e-###` references this finding cites.
    pub evidence_refs: Vec<String>,
}

/// The generated record.
#[derive(Clone, Debug)]
pub struct RetrospectiveRecord {
    /// Always `1`.
    pub schema_version: u32,
    /// The Mission ULID identity.
    pub mission_id: String,
    /// The Mission slug.
    pub mission_slug: String,
    /// The assigned Mission number, when one is recorded.
    pub mission_number: Option<i64>,
    /// The friendly name.
    pub friendly_name: String,
    /// The Mission type, or `""` for a typeless Mission.
    pub mission_type: String,
    /// The target branch.
    pub target_branch: String,
    /// The capture timestamp (also the provenance timestamp).
    pub created_at: String,
    /// The capture actor kind (`runtime`).
    pub created_by_kind: String,
    /// The capture actor id (`spec-kitty-generator`).
    pub created_by_id: String,
    /// The capture actor display name, when recorded.
    pub created_by_display: Option<String>,
    /// `runtime_post_completion` for the merge path.
    pub provenance_kind: String,
    /// The provenance invocation timestamp.
    pub provenance_invoked_at: String,
    /// The resolved policy source map.
    pub policy_source: BTreeMap<String, String>,
    /// `has_findings` or `ran_no_findings`.
    pub findings_status: &'static str,
    /// Work packages that completed without rejection cycles, plus clean ingestors.
    pub helped: Vec<Finding>,
    /// Everything the workflow did not help with (force overrides, friction, ...).
    pub not_helpful: Vec<Finding>,
    /// Missing optional artifacts and open clarification markers.
    pub gaps: Vec<Finding>,
    /// Every evidence reference, in registration order.
    pub evidence_refs: Vec<EvidenceRef>,
    /// The generator version (`1.0`).
    pub generator_version: String,
}

impl RetrospectiveRecord {
    /// The Source `_gen_record_to_dict` field order.
    pub fn to_value(&self) -> serde_yaml_ng::Value {
        use serde_yaml_ng::{Mapping, Value as Y};

        fn finding(value: &Finding) -> Y {
            let mut map = Mapping::new();
            map.insert(Y::from("id"), Y::from(value.id.as_str()));
            map.insert(Y::from("category"), Y::from(value.category.as_str()));
            map.insert(Y::from("summary"), Y::from(value.summary.as_str()));
            map.insert(
                Y::from("evidence_refs"),
                Y::Sequence(
                    value
                        .evidence_refs
                        .iter()
                        .map(|id| Y::from(id.as_str()))
                        .collect(),
                ),
            );
            if let Some(details) = &value.details {
                map.insert(Y::from("details"), Y::from(details.as_str()));
            }
            Y::Mapping(map)
        }

        let mut record = Mapping::new();
        record.insert(Y::from("schema_version"), Y::from(self.schema_version));
        record.insert(Y::from("mission_id"), Y::from(self.mission_id.as_str()));
        record.insert(Y::from("mission_slug"), Y::from(self.mission_slug.as_str()));
        match self.mission_number {
            Some(number) => record.insert(Y::from("mission_number"), Y::from(number)),
            None => record.insert(Y::from("mission_number"), Y::Null),
        };
        record.insert(
            Y::from("friendly_name"),
            Y::from(self.friendly_name.as_str()),
        );
        record.insert(Y::from("mission_type"), Y::from(self.mission_type.as_str()));
        record.insert(
            Y::from("target_branch"),
            Y::from(self.target_branch.as_str()),
        );
        record.insert(Y::from("created_at"), Y::from(self.created_at.as_str()));

        let mut actor = Mapping::new();
        actor.insert(Y::from("kind"), Y::from(self.created_by_kind.as_str()));
        actor.insert(Y::from("id"), Y::from(self.created_by_id.as_str()));
        if let Some(display) = &self.created_by_display {
            actor.insert(Y::from("display"), Y::from(display.as_str()));
        }
        record.insert(Y::from("created_by"), Y::Mapping(actor));

        let mut provenance = Mapping::new();
        provenance.insert(Y::from("kind"), Y::from(self.provenance_kind.as_str()));
        provenance.insert(
            Y::from("invoked_at"),
            Y::from(self.provenance_invoked_at.as_str()),
        );
        provenance.insert(
            Y::from("policy_resolved_from"),
            Y::Mapping(policy_source_mapping(&self.policy_source)),
        );
        record.insert(Y::from("provenance"), Y::Mapping(provenance));

        record.insert(
            Y::from("policy_source"),
            Y::Mapping(policy_source_mapping(&self.policy_source)),
        );
        record.insert(Y::from("findings_status"), Y::from(self.findings_status));
        record.insert(
            Y::from("helped"),
            Y::Sequence(self.helped.iter().map(finding).collect()),
        );
        record.insert(
            Y::from("not_helpful"),
            Y::Sequence(self.not_helpful.iter().map(finding).collect()),
        );
        record.insert(
            Y::from("gaps"),
            Y::Sequence(self.gaps.iter().map(finding).collect()),
        );
        record.insert(Y::from("proposals"), Y::Sequence(Vec::new()));
        let refs = self
            .evidence_refs
            .iter()
            .map(|reference| {
                let mut map = Mapping::new();
                map.insert(Y::from("id"), Y::from(reference.id.as_str()));
                map.insert(Y::from("kind"), Y::from(reference.kind.as_str()));
                if let Some(path) = &reference.path {
                    map.insert(Y::from("path"), Y::from(path.as_str()));
                }
                if let Some(range) = &reference.range {
                    map.insert(Y::from("range"), Y::from(range.as_str()));
                }
                if let Some(url) = &reference.url {
                    map.insert(Y::from("url"), Y::from(url.as_str()));
                }
                Y::Mapping(map)
            })
            .collect();
        record.insert(Y::from("evidence_refs"), Y::Sequence(refs));
        record.insert(
            Y::from("generator_version"),
            Y::from(self.generator_version.as_str()),
        );
        record.insert(Y::from("provenance_history"), Y::Sequence(Vec::new()));
        Y::Mapping(record)
    }

    /// The record's YAML body, from the ONE canonical emitter.
    ///
    /// Source writes `kernel.yaml_io.write_mapping_atomic(data, path, width=120)`
    /// — ruamel round-trip dump at 120 columns plus the guarded trailing-space
    /// normalization. `sk_charter::ordered_yaml_width_normalized` is that same
    /// single emitter (emitter + guarded ASCII normalization) and is the only
    /// serializer here: this module performs NO post-processing of its bytes.
    pub fn to_yaml_bytes(&self) -> Vec<u8> {
        sk_charter::ordered_yaml_width_normalized(&self.to_value(), 120)
    }

    /// Deterministic YAML body as text.
    pub fn to_yaml(&self) -> Result<String, RetrospectiveError> {
        String::from_utf8(self.to_yaml_bytes())
            .map_err(|error| RetrospectiveError::Serialize(error.to_string()))
    }
}

/// The capture request — everything is explicit; nothing reads ambient state.
#[derive(Clone, Debug)]
pub struct RetrospectiveRequest<'a> {
    /// The repository root (primary checkout).
    pub repo_root: &'a Path,
    /// `kitty-specs/<slug>`.
    pub feature_dir: &'a Path,
    /// Source `ProvenanceKind`; `runtime_post_completion` for the merge path.
    pub provenance_kind: &'a str,
    /// RFC 3339 timestamp for both `created_at` and `provenance.invoked_at`.
    pub invoked_at: &'a str,
    /// The resolved policy source map (`<default>` for every key by default).
    pub policy_source: BTreeMap<String, String>,
}

/// The fifteen Source policy keys, in `policy.py` declaration order.
///
/// `precedence` belongs here: Source's `_default_source_map` carries it, and a
/// record without it is a different map (the paired protocol test caught exactly
/// that).
pub const POLICY_KEYS: [&str; 15] = [
    "enabled",
    "timing",
    "failure_policy",
    "write_record",
    "generate_proposals",
    "apply_proposals",
    "permissions.write_record",
    "permissions.inspect_mission_artifacts",
    "permissions.propose_glossary_changes",
    "permissions.propose_drg_changes",
    "permissions.propose_doctrine_changes",
    "permissions.apply_low_risk_changes",
    "permissions.apply_structural_changes",
    "precedence",
    "generator",
];

/// The `<default>` policy source map a Source run with no configuration records.
pub fn default_policy_source() -> BTreeMap<String, String> {
    POLICY_KEYS
        .iter()
        .map(|key| ((*key).to_owned(), "<default>".to_owned()))
        .collect()
}

/// The tracer markdown read surface (`traces/*.md`), sorted by file name.
///
/// Returns `(repo-relative path, text)` pairs. A tracer that is not valid UTF-8
/// is skipped rather than fatal (Source's best-effort contract: a corrupt tracer
/// contributes no findings, it does not fail the capture). The label is always
/// `kitty-specs/<slug>/traces/<name>` — an evidence-provenance label, not the
/// path the bytes were physically read through.
pub fn load_traces(feature_dir: &Path) -> Vec<(String, String)> {
    let slug = feature_dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let traces_dir = feature_dir.join("traces");
    let Ok(entries) = std::fs::read_dir(&traces_dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|value| value == "md"))
        .collect();
    paths.sort_by(|left, right| left.file_name().cmp(&right.file_name()));
    let mut traces = Vec::new();
    for path in paths {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        traces.push((format!("kitty-specs/{slug}/traces/{name}"), text));
    }
    traces
}

/// One structured tracer entry: the bold lead (summary) and its full span (body).
///
/// Source's `_TRACE_ENTRY_RE` (`(?m)^\s*(?:\d+\.|[-*])\s+\*\*(.+?)\*\*`) is matched
/// here without a regex engine: a line beginning (after indentation) with
/// `N.`, `-` or `*`, then whitespace, then a `**…**` span on the same line. An
/// entry runs until the next entry's start or the end of the text.
pub fn parse_trace_entries(text: &str) -> Vec<(String, String)> {
    let mut starts: Vec<(usize, usize, usize)> = Vec::new();
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        if let Some((summary_start, summary_end)) = trace_entry_summary(line) {
            // The scan reports LINE offsets; the entry spans index the document.
            starts.push((offset, offset + summary_start, offset + summary_end));
        }
        offset += line.len();
    }
    let mut entries = Vec::new();
    for (position, (item_start, summary_start, summary_end)) in starts.iter().enumerate() {
        let summary = &text[*summary_start..*summary_end];
        if summary.trim().is_empty() {
            continue;
        }
        let body_end = starts
            .get(position + 1)
            .map(|(next_start, _, _)| *next_start)
            .unwrap_or(text.len());
        entries.push((
            summary.trim().to_owned(),
            text[*item_start..body_end].trim().to_owned(),
        ));
    }
    entries
}

/// `(summary_start, summary_end)` byte offsets within `line`, when it starts an entry.
fn trace_entry_summary(line: &str) -> Option<(usize, usize)> {
    let bytes = line.as_bytes();
    let mut cursor = 0usize;
    while cursor < bytes.len() && matches!(bytes[cursor], b' ' | b'\t' | b'\r') {
        cursor += 1;
    }
    if cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
        let mut after_digits = cursor;
        while after_digits < bytes.len() && bytes[after_digits].is_ascii_digit() {
            after_digits += 1;
        }
        if after_digits < bytes.len() && bytes[after_digits] == b'.' {
            cursor = after_digits + 1;
        } else {
            return None;
        }
    } else if cursor < bytes.len() && matches!(bytes[cursor], b'-' | b'*') {
        cursor += 1;
    } else {
        return None;
    }
    let before_space = cursor;
    while cursor < bytes.len() && matches!(bytes[cursor], b' ' | b'\t') {
        cursor += 1;
    }
    if cursor == before_space || !line[cursor..].starts_with("**") {
        return None;
    }
    let summary_start = cursor + 2;
    let rest = &line[summary_start..];
    let end = rest.find("**")?;
    if end == 0 {
        return None;
    }
    Some((summary_start, summary_start + end))
}

/// Gap markers take precedence over helped markers; anything else is friction.
pub const TRACE_GAP_MARKERS: [&str; 5] = [
    "candidate gap",
    "candidate-gap",
    "open (",
    "open)",
    "**open",
];
/// The documented "this helped" markers.
pub const TRACE_HELPED_MARKERS: [&str; 6] = [
    "no gap",
    "expected",
    "worked as designed",
    "**fixed",
    "fixed —",
    "fixed -",
];

/// `gap` / `helped` / `not_helpful`, exactly as Source routes a tracer entry.
pub fn classify_trace_disposition(body: &str) -> &'static str {
    let lowered = body.to_lowercase();
    if TRACE_GAP_MARKERS
        .iter()
        .any(|marker| lowered.contains(marker))
    {
        return "gap";
    }
    if TRACE_HELPED_MARKERS
        .iter()
        .any(|marker| lowered.contains(marker))
    {
        return "helped";
    }
    "not_helpful"
}

/// The `(helped, not_helpful, gaps)` findings a set of tracer documents produces.
///
/// The same builder [`generate_retrospective`] uses (there is no second copy), so
/// a caller that only needs the tracer findings gets exactly the record's rows.
pub fn trace_findings(traces: &[(String, String)]) -> (Vec<Finding>, Vec<Finding>, Vec<Finding>) {
    let mut helped: Vec<Finding> = Vec::new();
    let mut not_helpful: Vec<Finding> = Vec::new();
    let mut gaps: Vec<Finding> = Vec::new();
    let mut counters: BTreeMap<String, u32> = BTreeMap::new();
    let mut registry = EvidenceRegistry::new();
    append_trace_findings(
        &mut helped,
        &mut not_helpful,
        &mut gaps,
        &mut counters,
        &mut registry,
        traces,
    );
    (helped, not_helpful, gaps)
}

#[allow(clippy::too_many_arguments)]
fn append_trace_findings(
    helped: &mut Vec<Finding>,
    not_helpful: &mut Vec<Finding>,
    gaps: &mut Vec<Finding>,
    counters: &mut BTreeMap<String, u32>,
    registry: &mut EvidenceRegistry,
    traces: &[(String, String)],
) {
    for (trace_rel, text) in traces {
        for (summary, body) in parse_trace_entries(text) {
            let evidence = registry.add_file(trace_rel);
            let summary: String = format!("Tracer: {summary}").chars().take(200).collect();
            let details: String = body.chars().take(1000).collect();
            let (prefix, target) = match classify_trace_disposition(&body) {
                "helped" => ("h", &mut *helped),
                "gap" => ("g", &mut *gaps),
                _ => ("n", &mut *not_helpful),
            };
            target.push(finding(
                counters,
                prefix,
                "tooling",
                summary,
                Some(details),
                vec![evidence],
            ));
        }
    }
}

/// The `policy_source` / `policy_resolved_from` mapping in SOURCE's order.
///
/// Source's `_default_source_map` is a dict in `policy.py` declaration order and
/// its YAML dumper keeps insertion order, so the record's maps are in declaration
/// order — not sorted. A `BTreeMap` cannot express that, so the declared keys are
/// emitted in [`POLICY_KEYS`] order and any extra key follows, sorted.
fn policy_source_mapping(policy: &BTreeMap<String, String>) -> serde_yaml_ng::Mapping {
    use serde_yaml_ng::{Mapping, Value as Y};
    let mut mapping = Mapping::new();
    for key in POLICY_KEYS {
        if let Some(value) = policy.get(key) {
            mapping.insert(Y::from(key), Y::from(value.as_str()));
        }
    }
    for (key, value) in policy {
        if POLICY_KEYS.contains(&key.as_str()) {
            continue;
        }
        mapping.insert(Y::from(key.as_str()), Y::from(value.as_str()));
    }
    mapping
}

fn read_optional(path: &Path) -> Result<Option<String>, RetrospectiveError> {
    match std::fs::read(path) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => Ok(Some(text)),
            Err(_) => Ok(Some(
                String::from_utf8_lossy(&std::fs::read(path).map_err(|source| {
                    RetrospectiveError::Read {
                        path: path.display().to_string(),
                        source,
                    }
                })?)
                .into_owned(),
            )),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(RetrospectiveError::Read {
            path: path.display().to_string(),
            source,
        }),
    }
}

/// `_load_wp_files`: `tasks/WP*.md`, sorted by file name.
fn load_wp_files(feature_dir: &Path) -> Result<Vec<(String, String)>, RetrospectiveError> {
    let directory = feature_dir.join("tasks");
    let mut entries: Vec<PathBuf> = match std::fs::read_dir(&directory) {
        Ok(entries) => entries.flatten().map(|entry| entry.path()).collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(RetrospectiveError::Read {
                path: directory.display().to_string(),
                source,
            });
        }
    };
    entries.sort();
    let mut files = Vec::new();
    for path in entries {
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if !name.starts_with("WP") || !name.ends_with(".md") {
            continue;
        }
        if let Some(text) = read_optional(&path)? {
            files.push((name.to_owned(), text));
        }
    }
    Ok(files)
}

/// `_load_events`: the status log's JSON rows, skipping unparsable lines the way
/// Source's loader does (it drops a malformed row and keeps going).
fn load_events(feature_dir: &Path) -> Result<Vec<Value>, RetrospectiveError> {
    let path = feature_dir.join("status.events.jsonl");
    let Some(text) = read_optional(&path)? else {
        return Ok(Vec::new());
    };
    Ok(text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(Value::is_object)
        .collect())
}

fn text_field<'a>(event: &'a Value, key: &str) -> &'a str {
    event.get(key).and_then(Value::as_str).unwrap_or("")
}

fn event_wp_id(event: &Value) -> String {
    let direct = event.get("wp_id").and_then(Value::as_str).unwrap_or("");
    if !direct.is_empty() {
        return direct.to_owned();
    }
    event
        .get("payload")
        .and_then(Value::as_object)
        .and_then(|payload| payload.get("wp_id"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned()
}

/// Source `REVIEW_FEEDBACK_SENTINELS`.
const REVIEW_FEEDBACK_SENTINELS: [&str; 3] = [
    "force-override",
    "action-review-claim",
    "workflow-review-claim",
];

/// Source `SYNTHETIC_REVIEW_REF_PREFIXES`.
const SYNTHETIC_REVIEW_REF_PREFIXES: [&str; 2] = ["review:", "approval:"];

fn is_synthetic_review_ref(value: &str) -> bool {
    let stripped = value.trim();
    SYNTHETIC_REVIEW_REF_PREFIXES
        .iter()
        .any(|prefix| stripped.starts_with(prefix) && stripped.len() > prefix.len())
}

fn is_non_resolvable_review_ref(value: &str) -> bool {
    REVIEW_FEEDBACK_SENTINELS.contains(&value) || is_synthetic_review_ref(value)
}

/// Source `_REWORK_TARGETS` and `_BACKWARD_REWORK_MOVES`.
fn is_backward_rework_move(event: &Value) -> bool {
    let from = text_field(event, "from_lane");
    let to = text_field(event, "to_lane");
    let targets: &[&str] = &["planned", "claimed", "in_progress"];
    match from {
        "for_review" | "in_review" | "approved" | "done" => targets.contains(&to),
        "in_progress" => matches!(to, "planned" | "claimed"),
        _ => false,
    }
}

fn is_changes_requested(verdict: Option<&Value>) -> bool {
    verdict.and_then(Value::as_str) == Some("changes_requested")
}

fn is_documented_pointer(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|text| !text.trim().is_empty() && !is_non_resolvable_review_ref(text.trim()))
}

fn has_documented_review_feedback(event: &Value) -> bool {
    if is_documented_pointer(event.get("review_ref")) {
        return true;
    }
    match event.get("evidence") {
        Some(Value::Object(evidence)) => match evidence.get("review") {
            Some(Value::Object(review)) => {
                is_documented_pointer(review.get("reference"))
                    && is_changes_requested(review.get("verdict"))
            }
            _ => false,
        },
        Some(Value::String(text)) => !text.trim().is_empty(),
        _ => false,
    }
}

fn is_documented_review_rejection(event: &Value) -> bool {
    is_backward_rework_move(event) && has_documented_review_feedback(event)
}

fn is_review_claim(event: &Value) -> bool {
    event
        .get("review_ref")
        .and_then(Value::as_str)
        .is_some_and(|value| {
            matches!(
                value.trim(),
                "action-review-claim" | "workflow-review-claim"
            )
        })
}

fn is_lane_friction_event(event: &Value) -> bool {
    is_backward_rework_move(event)
        && !is_documented_review_rejection(event)
        && !is_review_claim(event)
}

fn count_events(events: &[Value], predicate: impl Fn(&Value) -> bool) -> BTreeMap<String, u64> {
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    for event in events {
        let wp_id = event_wp_id(event);
        if wp_id.is_empty() || !predicate(event) {
            continue;
        }
        *counts.entry(wp_id).or_default() += 1;
    }
    counts
}

/// Source `_BOOTSTRAP_ACTORS`.
const BOOTSTRAP_ACTORS: [&str; 3] = ["finalize-tasks", "bootstrap", "migrate"];

fn is_force_override_event(event: &Value) -> bool {
    if event.get("force").and_then(Value::as_bool) != Some(true) {
        return false;
    }
    let actor = text_field(event, "actor").to_lowercase();
    if BOOTSTRAP_ACTORS.contains(&actor.as_str()) {
        return false;
    }
    if text_field(event, "from_lane") == text_field(event, "to_lane") {
        return false;
    }
    !(is_documented_review_rejection(event) || is_review_claim(event))
}

/// Source `_ARBITER_MARKERS`.
const ARBITER_MARKERS: [&str; 4] = [
    "arbiter",
    "arbiter-override",
    "arbiter override",
    "override arbiter",
];

fn event_note(event: &Value) -> String {
    let note = text_field(event, "note");
    if !note.is_empty() {
        return note.to_lowercase();
    }
    text_field(event, "reason").to_lowercase()
}

fn is_arbiter_event(event: &Value) -> bool {
    let note = event_note(event);
    ARBITER_MARKERS
        .iter()
        .any(|marker| note.contains(&marker.to_lowercase()))
}

/// Source `_is_reviewer_self_approval_event`.
fn is_reviewer_self_approval_event(event: &Value) -> bool {
    let kind = text_field(event, "event_type");
    if kind == "ReviewerSelfApproval" {
        return true;
    }
    event
        .get("payload")
        .and_then(Value::as_object)
        .and_then(|payload| payload.get("event_type"))
        .and_then(Value::as_str)
        == Some("ReviewerSelfApproval")
}

/// Source `_undocumented_reentries` / `_detect_implementation_cycles`: a WP's
/// `in_progress` re-entries that no documented rejection explains.
///
/// The first `planned`/`claimed -> in_progress` entry is the normal cycle. A
/// documented review rejection licenses the next re-entry, and a rejection that
/// lands in `in_progress` itself (`in_review -> in_progress`) *is* that re-entry,
/// so it consumes its own licence.
fn detect_implementation_cycles(events: &[Value]) -> BTreeMap<String, u64> {
    let mut by_wp: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for event in events {
        let wp_id = event_wp_id(event);
        if !wp_id.is_empty() {
            by_wp.entry(wp_id).or_default().push(event);
        }
    }
    let mut counts = BTreeMap::new();
    for (wp_id, wp_events) in by_wp {
        let mut entries = 0u64;
        let mut undocumented = 0u64;
        let mut licensed = false;
        for event in wp_events {
            if BOOTSTRAP_ACTORS.contains(&text_field(event, "actor").to_lowercase().as_str()) {
                continue;
            }
            if is_documented_review_rejection(event) {
                licensed = text_field(event, "to_lane") != "in_progress";
                continue;
            }
            if matches!(text_field(event, "from_lane"), "planned" | "claimed")
                && text_field(event, "to_lane") == "in_progress"
            {
                entries += 1;
                if entries > 1 && !licensed {
                    undocumented += 1;
                }
                licensed = false;
            }
        }
        if undocumented > 0 {
            counts.insert(wp_id, undocumented);
        }
    }
    counts
}

fn detect_done_wps(events: &[Value]) -> BTreeSet<String> {
    events
        .iter()
        .filter(|event| matches!(text_field(event, "to_lane"), "done" | "approved"))
        .map(event_wp_id)
        .filter(|wp_id| !wp_id.is_empty())
        .collect()
}

/// Source `_event_id_range_for`.
fn event_id_range_for(events: &[Value], wp_id: &str, predicate: impl Fn(&Value) -> bool) -> String {
    let ids: Vec<String> = events
        .iter()
        .filter(|event| event_wp_id(event) == wp_id && predicate(event))
        .map(|event| text_field(event, "event_id").to_owned())
        .collect();
    match ids.len() {
        0 => String::new(),
        1 => ids[0].clone(),
        _ => format!("{}..{}", ids[0], ids[ids.len() - 1]),
    }
}

/// Source `_EvidenceRegistry`.
struct EvidenceRegistry {
    next: u32,
    map: BTreeMap<String, String>,
    refs: Vec<EvidenceRef>,
}

impl EvidenceRegistry {
    fn new() -> Self {
        Self {
            next: 0,
            map: BTreeMap::new(),
            refs: Vec::new(),
        }
    }

    fn add_file(&mut self, rel_path: &str) -> String {
        if let Some(id) = self.map.get(rel_path) {
            return id.clone();
        }
        self.next += 1;
        let id = format!("e-{:03}", self.next);
        self.map.insert(rel_path.to_owned(), id.clone());
        self.refs.push(EvidenceRef {
            id: id.clone(),
            kind: "file".to_owned(),
            path: Some(rel_path.to_owned()),
            range: None,
            url: None,
        });
        id
    }

    fn add_event_range(&mut self, rel_path: &str, range: &str, range_key: &str) -> String {
        let key = format!("{rel_path}:{range_key}");
        if let Some(id) = self.map.get(&key) {
            return id.clone();
        }
        self.next += 1;
        let id = format!("e-{:03}", self.next);
        self.map.insert(key, id.clone());
        self.refs.push(EvidenceRef {
            id: id.clone(),
            kind: "event_range".to_owned(),
            path: Some(rel_path.to_owned()),
            range: Some(range.to_owned()),
            url: None,
        });
        id
    }
}

/// Source `_next_finding_id`.
fn next_finding_id(counters: &mut BTreeMap<String, u32>, prefix: &str) -> String {
    let counter = counters.entry(prefix.to_owned()).or_insert(0);
    *counter += 1;
    format!("{prefix}-{:03}", counter)
}

fn finding(
    counters: &mut BTreeMap<String, u32>,
    prefix: &str,
    category: &str,
    summary: impl Into<String>,
    details: Option<String>,
    evidence_refs: Vec<String>,
) -> Finding {
    Finding {
        id: next_finding_id(counters, prefix),
        category: category.to_owned(),
        summary: summary.into(),
        details,
        evidence_refs,
    }
}

/// A report artifact's leading YAML frontmatter mapping, or `None`.
fn leading_frontmatter(text: &str) -> Option<serde_json::Map<String, Value>> {
    if !text.starts_with("---") {
        return None;
    }
    let lines: Vec<&str> = text.lines().collect();
    let closing = (1..lines.len()).find(|index| lines[*index].trim() == "---")?;
    let body = lines[1..closing].join("\n");
    let parsed: serde_yaml_ng::Value = serde_yaml_ng::from_str(&body).ok()?;
    let yaml = serde_json::to_value(parsed).ok()?;
    match yaml {
        Value::Object(map) => Some(map),
        _ => None,
    }
}

/// Source `_report_findings_count`.
fn report_findings_count(frontmatter: Option<&serde_json::Map<String, Value>>) -> Option<u64> {
    let findings = frontmatter?.get("findings")?;
    match findings {
        Value::Array(items) => Some(items.len() as u64),
        Value::Number(number) => number.as_u64(),
        _ => None,
    }
}

/// Source `_spec_declares_domain_entities`.
fn spec_declares_domain_entities(spec_text: &str) -> bool {
    let mut in_section = false;
    let mut found_bullet = false;
    for line in spec_text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            let heading = trimmed.trim_start_matches('#').trim();
            if in_section {
                break;
            }
            in_section = heading.eq_ignore_ascii_case("key entities");
            continue;
        }
        if in_section {
            let bullet = trimmed
                .strip_prefix("- ")
                .or_else(|| trimmed.strip_prefix("* "));
            if let Some(rest) = bullet
                && rest.starts_with("**")
                && !rest.starts_with("**[")
            {
                found_bullet = true;
                break;
            }
        }
    }
    found_bullet
}

/// Source `_NEEDS_CLARIFICATION_RE`.
fn needs_clarification_lines(spec_text: &str) -> Vec<usize> {
    let lowered = spec_text.to_lowercase();
    let needle = "[needs clarification:";
    let mut lines = Vec::new();
    let mut search_from = 0usize;
    while let Some(offset) = lowered[search_from..].find(needle) {
        let absolute = search_from + offset;
        let line_number = spec_text[..absolute].matches('\n').count() + 1;
        lines.push(line_number);
        search_from = absolute + needle.len();
    }
    lines
}

/// The `FR-*` ids `spec.md` declares, through the canonical declared-requirement
/// parser `sk_status::acceptance::parse_declared_requirement_ids`.
fn declared_fr_ids(spec_text: &str) -> BTreeSet<String> {
    sk_status::acceptance::parse_declared_requirement_ids(spec_text)
        .into_iter()
        .filter(|id| id.starts_with("FR-"))
        .collect()
}

/// Source `_collect_fr_references` + `_find_unmapped_frs`.
fn unmapped_frs(wp_files: &[(String, String)], declared: &BTreeSet<String>) -> Vec<String> {
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    for (_, text) in wp_files {
        for id in declared {
            if mentions_requirement_id(text, id) {
                referenced.insert(id.clone());
            }
        }
    }
    declared.difference(&referenced).cloned().collect()
}

/// A requirement id mention with a non-identifier boundary on both sides.
fn mentions_requirement_id(text: &str, requirement_id: &str) -> bool {
    let mut search_from = 0usize;
    while let Some(offset) = text[search_from..].find(requirement_id) {
        let start = search_from + offset;
        let end = start + requirement_id.len();
        let before_ok = text[..start]
            .chars()
            .next_back()
            .is_none_or(|character| !(character.is_alphanumeric() || character == '_'));
        let after_ok = text[end..]
            .chars()
            .next()
            .is_none_or(|character| !(character.is_alphanumeric() || character == '_'));
        if before_ok && after_ok {
            return true;
        }
        search_from = start + requirement_id.len();
    }
    false
}

fn paired_read(
    repo_root: &Path,
    feature_dir: &Path,
    name: &str,
) -> Result<Option<String>, RetrospectiveError> {
    let _ = repo_root;
    read_optional(&feature_dir.join(name))
}

fn relative(feature_dir: &Path, name: &str) -> String {
    let slug = feature_dir
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    format!("kitty-specs/{slug}/{name}")
}

/// Generate the record.
pub fn generate_retrospective(
    request: &RetrospectiveRequest<'_>,
) -> Result<RetrospectiveRecord, RetrospectiveError> {
    if !request.feature_dir.is_dir() {
        return Err(RetrospectiveError::MissionMissing(
            request.feature_dir.display().to_string(),
        ));
    }
    let feature_dir = request.feature_dir;

    let meta_text = paired_read(request.repo_root, feature_dir, "meta.json")?;
    let meta: serde_json::Map<String, Value> = meta_text
        .as_deref()
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .and_then(|value| match value {
            Value::Object(map) => Some(map),
            _ => None,
        })
        .unwrap_or_default();

    let spec_text = paired_read(request.repo_root, feature_dir, "spec.md")?.unwrap_or_default();
    let plan_text = paired_read(request.repo_root, feature_dir, "plan.md")?.unwrap_or_default();
    let tasks_text = paired_read(request.repo_root, feature_dir, "tasks.md")?.unwrap_or_default();
    let research_text = paired_read(request.repo_root, feature_dir, "research.md")?;
    let data_model_text = paired_read(request.repo_root, feature_dir, "data-model.md")?;
    let workflow_failures_text =
        paired_read(request.repo_root, feature_dir, "workflow-failures-log.md")?;
    let analysis_report_text = paired_read(request.repo_root, feature_dir, "analysis-report.md")?;
    let review_report_text =
        paired_read(request.repo_root, feature_dir, "mission-review-report.md")?;

    let wp_files = load_wp_files(feature_dir)?;
    let events = load_events(feature_dir)?;

    let spec_rel = relative(feature_dir, "spec.md");
    let meta_rel = relative(feature_dir, "meta.json");
    let plan_rel = relative(feature_dir, "plan.md");
    let tasks_rel = relative(feature_dir, "tasks.md");
    let events_rel = relative(feature_dir, "status.events.jsonl");
    let workflow_failures_rel = relative(feature_dir, "workflow-failures-log.md");
    let analysis_report_rel = relative(feature_dir, "analysis-report.md");
    let review_report_rel = relative(feature_dir, "mission-review-report.md");

    let mut registry = EvidenceRegistry::new();
    if !meta.is_empty() {
        registry.add_file(&meta_rel);
    }
    if !spec_text.is_empty() {
        registry.add_file(&spec_rel);
    }
    if !plan_text.is_empty() {
        registry.add_file(&plan_rel);
    }
    if !tasks_text.is_empty() {
        registry.add_file(&tasks_rel);
    }
    let mut wp_evidence_ids: BTreeMap<String, String> = BTreeMap::new();
    for (name, _) in &wp_files {
        wp_evidence_ids.insert(
            name.clone(),
            registry.add_file(&relative(feature_dir, &format!("tasks/{name}"))),
        );
    }
    if !events.is_empty() {
        registry.add_file(&events_rel);
    }

    // ---- classification (`_build_findings`) ----
    let mut counters: BTreeMap<String, u32> = BTreeMap::new();
    let rejection_counts = count_events(&events, is_documented_review_rejection);
    let lane_friction_counts = count_events(&events, is_lane_friction_event);
    let impl_cycle_counts = detect_implementation_cycles(&events);
    let done_wps = detect_done_wps(&events);
    let mut helped: Vec<Finding> = Vec::new();
    let mut not_helpful: Vec<Finding> = Vec::new();
    let mut gaps: Vec<Finding> = Vec::new();

    let has_ingestor_content = workflow_failures_text.is_some()
        || analysis_report_text.is_some()
        || review_report_text.is_some();
    if !(rejection_counts.is_empty() && lane_friction_counts.is_empty()) || has_ingestor_content {
        for wp_id in &done_wps {
            if rejection_counts.get(wp_id).copied().unwrap_or(0) != 0
                || lane_friction_counts.get(wp_id).copied().unwrap_or(0) != 0
                || impl_cycle_counts.get(wp_id).copied().unwrap_or(0) != 0
            {
                continue;
            }
            let file_name = format!("{wp_id}.md");
            let refs = match wp_evidence_ids.get(&file_name) {
                Some(id) => vec![id.clone()],
                None if !events.is_empty() => vec![registry.add_file(&events_rel)],
                None => Vec::new(),
            };
            if refs.is_empty() {
                continue;
            }
            helped.push(finding(
                &mut counters,
                "h",
                "implementation",
                format!("{wp_id} completed without rejection cycles"),
                None,
                refs,
            ));
        }
    }

    for (wp_id, count) in &rejection_counts {
        let range = event_id_range_for(&events, wp_id, is_documented_review_rejection);
        let evidence = registry.add_event_range(
            &events_rel,
            if range.is_empty() {
                "rejection"
            } else {
                &range
            },
            &format!("rejection_{wp_id}"),
        );
        not_helpful.push(finding(
            &mut counters,
            "n",
            "review_loop",
            format!("{wp_id} required {count} rejection cycle(s) before approval"),
            Some(format!(
                "WP {wp_id} was sent back to an earlier lane with documented review feedback {count} time(s)."
            )),
            vec![evidence],
        ));
    }

    for (wp_id, count) in &lane_friction_counts {
        let range = event_id_range_for(&events, wp_id, is_lane_friction_event);
        let evidence = registry.add_event_range(
            &events_rel,
            if range.is_empty() {
                "lane_friction"
            } else {
                &range
            },
            &format!("lane_friction_{wp_id}"),
        );
        not_helpful.push(finding(
            &mut counters,
            "n",
            "process",
            format!("{wp_id} had {count} lane bounce(s) before approval"),
            Some(format!(
                "WP {wp_id} moved backward across workflow lanes {count} time(s) outside the documented reviewer-feedback flow."
            )),
            vec![evidence],
        ));
    }

    for (wp_id, count) in &count_events(&events, is_force_override_event) {
        let range = event_id_range_for(&events, wp_id, is_force_override_event);
        let evidence = registry.add_event_range(
            &events_rel,
            if range.is_empty() {
                "force_override"
            } else {
                &range
            },
            &format!("force_override_{wp_id}"),
        );
        not_helpful.push(finding(
            &mut counters,
            "n",
            "process",
            format!("{wp_id} required {count} --force override(s) during workflow"),
            Some(format!(
                "WP {wp_id} required {count} operator-driven --force lane transition(s). Force overrides typically indicate the runtime guard failed or the operator routed around it; investigate the underlying cause."
            )),
            vec![evidence],
        ));
    }

    for (wp_id, count) in &count_events(&events, is_reviewer_self_approval_event) {
        let range = event_id_range_for(&events, wp_id, is_reviewer_self_approval_event);
        let evidence = registry.add_event_range(
            &events_rel,
            if range.is_empty() {
                "reviewer_self_approval"
            } else {
                &range
            },
            &format!("reviewer_self_approval_{wp_id}"),
        );
        let payload = events
            .iter()
            .find(|event| event_wp_id(event) == *wp_id && is_reviewer_self_approval_event(event))
            .and_then(|event| event.get("payload"))
            .cloned()
            .unwrap_or(Value::Null);
        let intended = payload
            .get("intended_reviewer")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or("unknown");
        let actor = payload
            .get("implementing_actor")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or("unknown");
        let reason = payload
            .get("failure_reason")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or("reviewer_failed");
        not_helpful.push(finding(
            &mut counters,
            "n",
            "process",
            format!("{wp_id} used self-review fallback instead of independent review"),
            Some(format!(
                "WP {wp_id} recorded {count} ReviewerSelfApproval event(s): {actor} self-reviewed after intended reviewer {intended} failed ({reason}). Treat this as a review-independence gap."
            )),
            vec![evidence],
        ));
    }

    for (wp_id, count) in &count_events(&events, is_arbiter_event) {
        let range = event_id_range_for(&events, wp_id, is_arbiter_event);
        let evidence = registry.add_event_range(
            &events_rel,
            if range.is_empty() { "arbiter" } else { &range },
            &format!("arbiter_{wp_id}"),
        );
        let recurring = if *count > 1 {
            " Recurring use suggests the normal review path is blocked and the underlying policy/guard may need adjustment."
        } else {
            ""
        };
        gaps.push(finding(
            &mut counters,
            "g",
            "process",
            format!("Arbiter override needed for {wp_id} ({count}x)"),
            Some(format!(
                "WP {wp_id} required {count} arbiter override(s). Arbiter overrides are an escape hatch.{recurring}"
            )),
            vec![evidence],
        ));
    }

    for (wp_id, count) in &impl_cycle_counts {
        let evidence = registry.add_event_range(
            &events_rel,
            "implementation_cycles",
            &format!("impl_cycles_{wp_id}"),
        );
        not_helpful.push(finding(
            &mut counters,
            "n",
            "implementation",
            format!(
                "{wp_id} re-entered in_progress {count} time(s) without a documented review rejection"
            ),
            Some(format!(
                "WP {wp_id} went back into implementation {count} time(s) with no documented review rejection before it. That rework was not captured by the review loop; the WP scope or contract may need refinement."
            )),
            vec![evidence],
        ));
    }

    for line_number in needs_clarification_lines(&spec_text) {
        let evidence = registry.add_file(&spec_rel);
        gaps.push(finding(
            &mut counters,
            "g",
            "spec_quality",
            format!("Unresolved clarification marker in spec.md (line {line_number})"),
            Some(format!(
                "Found '[NEEDS CLARIFICATION:' marker at line {line_number} of spec.md. This indicates a specification gap that was not resolved before implementation."
            )),
            vec![evidence],
        ));
    }

    if !spec_text.is_empty() && !wp_files.is_empty() {
        let declared = declared_fr_ids(&spec_text);
        for fr_id in unmapped_frs(&wp_files, &declared) {
            let evidence = registry.add_file(&spec_rel);
            gaps.push(finding(
                &mut counters,
                "g",
                "spec_quality",
                format!("{fr_id} defined in spec.md has no WP coverage"),
                Some(format!(
                    "Requirement {fr_id} appears in spec.md but is not referenced by any work package task file. It may be unimplemented."
                )),
                vec![evidence],
            ));
        }
    }

    if !spec_text.is_empty() {
        if research_text.is_none() {
            let evidence = registry.add_file(&spec_rel);
            gaps.push(finding(
                &mut counters,
                "g",
                "doc",
                "research.md absent",
                Some(
                    "spec.md is present but research.md is missing. Research artifacts help future maintainers understand design decisions."
                        .to_owned(),
                ),
                vec![evidence],
            ));
        }
        if data_model_text.is_none() && spec_declares_domain_entities(&spec_text) {
            let evidence = registry.add_file(&spec_rel);
            gaps.push(finding(
                &mut counters,
                "g",
                "doc",
                "data-model.md absent",
                Some(
                    "spec.md declares domain entities but data-model.md is missing. A data model document clarifies domain entity relationships."
                        .to_owned(),
                ),
                vec![evidence],
            ));
        }
    }

    if let Some(text) = &workflow_failures_text {
        let failures: Vec<&str> = text
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with("- [ ] FAIL:") || line.starts_with("### FAIL:"))
            .collect();
        if failures.is_empty() {
            let evidence = registry.add_file(&workflow_failures_rel);
            helped.push(finding(
                &mut counters,
                "h",
                "process",
                "workflow-failures-log.md present with no recorded failures",
                Some(
                    "A workflow-failures-log.md file is present but contains no structured failure entries (lines starting with '- [ ] FAIL:' or '### FAIL:'). This may indicate all workflows completed cleanly."
                        .to_owned(),
                ),
                vec![evidence],
            ));
        } else {
            for line in failures {
                let evidence = registry.add_file(&workflow_failures_rel);
                not_helpful.push(finding(
                    &mut counters,
                    "n",
                    "process",
                    line.chars().take(200).collect::<String>(),
                    Some(format!(
                        "Workflow failure recorded in workflow-failures-log.md. Entry: {line}"
                    )),
                    vec![evidence],
                ));
            }
        }
    }

    if let Some(text) = &analysis_report_text {
        let frontmatter = leading_frontmatter(text);
        let count = report_findings_count(frontmatter.as_ref());
        let verdict = frontmatter
            .as_ref()
            .and_then(|map| map.get("verdict"))
            .and_then(Value::as_str);
        let evidence = registry.add_file(&analysis_report_rel);
        match count {
            Some(count) if !matches!(verdict, None | Some("unknown")) => {
                if count > 0 {
                    not_helpful.push(finding(
                        &mut counters,
                        "n",
                        "doc",
                        format!("analysis-report.md records {count} finding(s)"),
                        Some(format!(
                            "The analysis-report.md artifact for this mission records {count} finding(s) in its structured frontmatter. Review them to understand documented issues and decisions."
                        )),
                        vec![evidence],
                    ));
                } else {
                    helped.push(finding(
                        &mut counters,
                        "h",
                        "doc",
                        "analysis-report.md present with no recorded findings",
                        Some(
                            "An analysis-report.md artifact is present and its frontmatter records zero findings with a resolved verdict — the analysis raised nothing."
                                .to_owned(),
                        ),
                        vec![evidence],
                    ));
                }
            }
            Some(_) => not_helpful.push(finding(
                &mut counters,
                "n",
                "doc",
                "analysis-report.md present; findings not machine-readable",
                Some(
                    "An analysis-report.md artifact is present for this mission and its frontmatter carries a machine-readable findings count, but no resolved verdict (the verdict key is absent or records the legacy 'unknown' marker), so the count is not asserted; review it manually."
                        .to_owned(),
                ),
                vec![evidence],
            )),
            None => not_helpful.push(finding(
                &mut counters,
                "n",
                "doc",
                "analysis-report.md present; findings not machine-readable",
                Some(
                    "An analysis-report.md artifact is present for this mission, but its leading frontmatter does not carry a machine-readable findings count; review it manually."
                        .to_owned(),
                ),
                vec![evidence],
            )),
        }
    }

    if let Some(text) = &review_report_text {
        let frontmatter = leading_frontmatter(text);
        let count = report_findings_count(frontmatter.as_ref());
        let evidence = registry.add_file(&review_report_rel);
        match count {
            Some(count) if count > 0 => not_helpful.push(finding(
                &mut counters,
                "n",
                "review_loop",
                format!("mission-review-report.md records {count} finding(s)"),
                Some(format!(
                    "The mission-review-report.md artifact for this mission records {count} finding(s) in its frontmatter. Review them to understand documented review outcomes."
                )),
                vec![evidence],
            )),
            Some(_) => helped.push(finding(
                &mut counters,
                "h",
                "review_loop",
                "mission-review-report.md present with no findings",
                Some(
                    "A mission-review-report.md artifact is present and its frontmatter records zero findings — the review pass raised nothing."
                        .to_owned(),
                ),
                vec![evidence],
            )),
            None => not_helpful.push(finding(
                &mut counters,
                "n",
                "review_loop",
                "mission-review-report.md present; findings not machine-readable",
                Some(
                    "A mission-review-report.md artifact is present for this mission, but its leading frontmatter does not carry a machine-readable findings count; review it manually."
                        .to_owned(),
                ),
                vec![evidence],
            )),
        }
    }

    // FR-007: tracer entries, routed by disposition into the same buckets. This
    // is Source's own position — `_build_ingestor_findings` calls the tracer
    // builder LAST, so the finding id sequence after the report ingestors is the
    // same sequence Source assigns.
    let traces = load_traces(feature_dir);
    if !traces.is_empty() {
        append_trace_findings(
            &mut helped,
            &mut not_helpful,
            &mut gaps,
            &mut counters,
            &mut registry,
            &traces,
        );
    }

    let by_category_summary = |left: &Finding, right: &Finding| {
        (left.category.as_str(), left.summary.as_str())
            .cmp(&(right.category.as_str(), right.summary.as_str()))
    };
    helped.sort_by(by_category_summary);
    not_helpful.sort_by(by_category_summary);
    gaps.sort_by(by_category_summary);

    let findings_status = if helped.is_empty() && not_helpful.is_empty() && gaps.is_empty() {
        "ran_no_findings"
    } else {
        "has_findings"
    };

    let mission_slug = meta
        .get("mission_slug")
        .or_else(|| meta.get("slug"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| {
            feature_dir
                .file_name()
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
    let mission_id = meta
        .get("mission_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let friendly_name = meta
        .get("friendly_name")
        .or_else(|| meta.get("name"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| mission_slug.clone());
    let mission_type = meta
        .get("mission_type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let target_branch = meta
        .get("target_branch")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| "main".to_owned());
    let mission_number = match meta.get("mission_number") {
        Some(Value::Number(number)) => number.as_i64(),
        Some(Value::String(text)) if text.chars().all(|character| character.is_ascii_digit()) => {
            text.parse().ok()
        }
        _ => None,
    };

    Ok(RetrospectiveRecord {
        schema_version: 1,
        mission_id,
        mission_slug,
        mission_number,
        friendly_name,
        mission_type,
        target_branch,
        created_at: request.invoked_at.to_owned(),
        created_by_kind: "runtime".to_owned(),
        created_by_id: "spec-kitty-generator".to_owned(),
        created_by_display: Some("Spec Kitty Generator".to_owned()),
        provenance_kind: request.provenance_kind.to_owned(),
        provenance_invoked_at: request.invoked_at.to_owned(),
        policy_source: request.policy_source.clone(),
        findings_status,
        helped,
        not_helpful,
        gaps,
        evidence_refs: registry.refs,
        generator_version: GENERATOR_VERSION.to_owned(),
    })
}

/// Write the record atomically to `kitty-specs/<slug>/retrospective.yaml`.
pub fn write_retrospective_record(
    record: &RetrospectiveRecord,
    feature_dir: &Path,
) -> Result<PathBuf, RetrospectiveError> {
    let path = feature_dir.join(RETROSPECTIVE_FILENAME);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| RetrospectiveError::Write {
            path: parent.display().to_string(),
            source,
        })?;
    }
    // Phase order, as Source's `_atomic_write_yaml`: serialize the WHOLE text
    // first (a serialization failure leaves nothing on disk), then publish it
    // atomically with the file fsynced and the parent directory flushed.
    let bytes = record.to_yaml_bytes();
    #[cfg(unix)]
    let published = sk_kernel::atomic_write_with_requested_mode(&path, &bytes, 0o644);
    // The pre-existing non-Unix writer remains a distinct, uncertified
    // platform capability; this Unix mode proof does not promote it.
    #[cfg(not(unix))]
    let published = sk_kernel::atomic_write(&path, &bytes);
    published.map_err(|source| RetrospectiveError::Write {
        path: path.display().to_string(),
        source: std::io::Error::other(source.to_string()),
    })?;
    Ok(path)
}

/// `true` when the Mission already holds a retrospective record (Source's capture
/// idempotence: a second capture is a no-op, only the commit heals).
pub fn retrospective_exists(feature_dir: &Path) -> bool {
    feature_dir.join(RETROSPECTIVE_FILENAME).is_file()
}
