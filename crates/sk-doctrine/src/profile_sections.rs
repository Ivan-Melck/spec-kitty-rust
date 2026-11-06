//! Source profile-cited prompt sections and the prepared profile block.
//!
//! Mirrors `charter.activation.context_renderers.profile_sections`:
//!
//! * **directive** — profile `directive-references` (code/name/rationale), with the
//!   canonical `DIRECTIVE_NNN` code normalisation;
//! * **tactic** — profile `tactic-references` (id/rationale);
//! * **styleguide / toolguide** — schema-attested references that render
//!   **pointer-only** by design (NFR-001): a header line plus the `--include`
//!   fetch stanza, never an inlined body, even when the body would fit;
//! * **procedure** — reached through the profile *channel*
//!   (`requires`/`specializes_from`/`suggests` walk), not an inline citation list;
//! * **suggested doctrine** — every artefact the channel reaches through
//!   `suggests` (after `requires`-precedence), rendered as a `when`-labelled link,
//!   one section per delivered kind in the reference's fixed order.
//!
//! One owner, no second model: the profile is the existing resolved
//! [`AgentProfile`], the repository is the existing
//! [`CanonicalOfferingService`], the walk is [`crate::query::walk_edges`], the
//! body payload is [`crate::body::exact_payload`] over the merged record's own
//! canonical graph, the fetch stanza is
//! [`crate::context_render::fetch_stanza_lines_exact`], and the per-entry budget
//! is [`crate::text_budget::budget_estimate`] with
//! [`crate::text_budget::PROFILE_INLINE_BODY_LIMIT_CHARS`]. A catalogue miss is a
//! structured stanza plus a warning from [`crate::catalog_diagnosis`] — never a
//! generic placeholder and never a broader abort. A repository-load failure is a
//! miss (the reference catches every exception around `repo.get`); a payload the
//! Native value model cannot project is a typed error, not a fabricated miss.

use std::collections::{BTreeMap, BTreeSet};

use serde_yaml_ng::Value;
use sk_charter::ArtifactKind;
use sk_kernel::PythonString;
use sk_kernel::python_json::{PythonJsonRenderOptions, PythonJsonValue};
use sk_kernel::python_text::is_decimal;

use crate::body::{ModelObject, ModelValue};
use crate::catalog_diagnosis::{
    diagnose_catalog_miss, emit_catalog_miss_warning, format_catalog_miss_stanza,
};
use crate::context::{STATED_DEFAULT_WHEN, link_references, requires_closure};
use crate::context_render::{FetchScope, exact_strip, fetch_stanza_lines_exact};
use crate::graph::{DrgGraph, NodeKind, Relation};
use crate::offering::CanonicalOfferingService;
use crate::profile::AgentProfile;
use crate::text_budget::{PROFILE_INLINE_BODY_LIMIT_CHARS, budget_estimate};

/// The code-change when-clause every profile-cited entry falls back to.
const PROFILE_CODE_CHANGE_WHEN: &str = "are about to apply a code change";
/// The procedure section's when-clause.
const PROFILE_PROCEDURE_WHEN: &str = "are about to run this procedure";
/// Profile-section fetch stanzas and inline bodies indent by four spaces.
const PROFILE_INDENT: &str = "    ";

/// The profile channel relations (`requires`, `specializes_from`, `suggests`).
fn profile_channel_relations() -> BTreeSet<Relation> {
    BTreeSet::from([
        Relation::Requires,
        Relation::SpecializesFrom,
        Relation::Suggests,
    ])
}

/// The kinds a `suggests` edge delivers in the suggested-doctrine section.
///
/// Deliberately excludes `asset`/`anti_pattern` (never delivered) and
/// `agent_profile` (a lineage pass-through hop).
const SUGGESTS_DELIVERED_KINDS: [&str; 6] = [
    "paradigm",
    "directive",
    "tactic",
    "styleguide",
    "toolguide",
    "procedure",
];

/// The deterministic per-kind render order and human section title.
const SUGGESTS_KIND_RENDER: [(&str, &str); 6] = [
    ("paradigm", "Paradigms"),
    ("directive", "Directives"),
    ("tactic", "Tactics"),
    ("styleguide", "Styleguides"),
    ("toolguide", "Toolguides"),
    ("procedure", "Procedures"),
];

/// Why a prepared profile section could not project one artefact body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProfileSectionsError {
    /// A merged record has no canonical graph in this crate (a Native projection
    /// boundary, never "artifact missing").
    PendingProjection {
        /// The artefact kind.
        kind: ArtifactKind,
        /// The requested artefact id.
        artifact_id: PythonString,
    },
    /// The reference's own model dump refuses this record's value.
    Payload {
        /// The artefact kind.
        kind: ArtifactKind,
        /// The requested artefact id.
        artifact_id: PythonString,
        /// The canonical payload failure.
        cause: crate::body::PayloadFailure,
    },
}

impl std::fmt::Display for ProfileSectionsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PendingProjection { kind, artifact_id } => write!(
                formatter,
                "profile section body for {}:{} has no canonical payload in this crate",
                kind.canonical(),
                artifact_id.repr()
            ),
            Self::Payload {
                kind,
                artifact_id,
                cause,
            } => write!(
                formatter,
                "profile section body for {}:{} was refused: {cause:?}",
                kind.canonical(),
                artifact_id.repr()
            ),
        }
    }
}

impl std::error::Error for ProfileSectionsError {}

/// The inputs one prepared profile block reads.
///
/// `warning_sink` is the caller's delivery channel for catalogue-miss warnings
/// (Source raises a Python warning). `None` renders the stanza but delivers no
/// warning, which is a disclosed Native boundary, not a silent success.
#[derive(Clone, Copy)]
pub struct ProfileSectionsInputs<'a> {
    /// The resolved profile (lineage already composed).
    pub profile: &'a AgentProfile,
    /// The merged DRG the profile channel walks.
    pub graph: &'a DrgGraph,
    /// The canonical layered offering repositories.
    pub catalog: &'a CanonicalOfferingService,
    /// Optional read-only fetch scope for the `--owned-checkout`/`--mission` flags.
    pub scope: Option<FetchScope<'a>>,
    /// The warning delivery sink.
    pub warning_sink: Option<&'a dyn Fn(&str)>,
}

impl std::fmt::Debug for ProfileSectionsInputs<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProfileSectionsInputs")
            .field("profile", &self.profile.id)
            .field("scope", &self.scope.is_some())
            .field("warning_sink", &self.warning_sink.is_some())
            .finish_non_exhaustive()
    }
}

/// Render every profile-channel section the resolved profile attests.
///
/// Sections are joined with one blank line, in the reference's fixed order:
/// directives, tactics, styleguides, toolguides, procedures, suggested doctrine.
/// An empty result means no section has entries (never a fabricated header).
///
/// # Errors
///
/// [`ProfileSectionsError`] when an admitted record's body cannot be projected
/// (a Native capability boundary or the reference's own payload refusal).
pub fn render_profile_sections(
    inputs: &ProfileSectionsInputs<'_>,
) -> Result<PythonString, ProfileSectionsError> {
    let mut blocks: Vec<Vec<PythonString>> = Vec::new();
    for lines in [
        render_directives(inputs)?,
        render_tactics(inputs)?,
        render_styleguides(inputs)?,
        render_toolguides(inputs)?,
        render_procedures(inputs)?,
        render_suggested(inputs)?,
    ] {
        if !lines.is_empty() {
            blocks.push(lines);
        }
    }
    let mut block_texts: Vec<PythonString> = Vec::new();
    for lines in blocks {
        block_texts.push(join_lines(&lines, "\n"));
    }
    Ok(join_lines(&block_texts, "\n\n"))
}

/// The reference `_format_profile_directive_code`.
#[must_use]
pub fn format_profile_directive_code(raw: &PythonString) -> PythonString {
    let text = exact_strip(raw);
    let codepoints = text.codepoints();
    let prefix: Vec<u32> = "DIRECTIVE_".chars().map(u32::from).collect();
    if codepoints.starts_with(&prefix)
        && codepoints.len() > prefix.len()
        && codepoints[prefix.len()..].iter().copied().all(is_decimal)
    {
        return text;
    }
    // `^(\d+)(?:[-_].*)?$`: the longest decimal prefix followed by a separator
    // (with anything after it) or the end of the string.
    let digit_end = codepoints
        .iter()
        .position(|code| !is_decimal(*code))
        .unwrap_or(codepoints.len());
    let separated = digit_end == codepoints.len()
        || (digit_end > 0 && matches!(codepoints[digit_end], 0x2d | 0x5f));
    // Source requires at least one digit; its regex `.` excludes LF (but
    // not CR or the other Python splitlines boundaries).
    if digit_end == 0 || !separated || codepoints[digit_end..].contains(&0x0a) {
        return text;
    }
    let digits = &codepoints[..digit_end];
    let mut unit_text: Vec<u32> = Vec::new();
    if digits.len() < 3 {
        unit_text.extend(std::iter::repeat_n(u32::from(b'0'), 3 - digits.len()));
    }
    unit_text.extend_from_slice(digits);
    let mut code = PythonString::from_scalars("DIRECTIVE_");
    code.append(&PythonString::from_units(unit_text).expect("decimal digits are code points"));
    code
}

/// One profile reference row, exactly as authored.
struct ReferenceRow {
    id: PythonString,
    name: PythonString,
    rationale: PythonString,
}

/// Read one profile reference list, preserving authored order and duplicates.
fn reference_rows(profile: &AgentProfile, field: &str, directive: bool) -> Vec<ReferenceRow> {
    profile
        .body
        .list(field)
        .into_iter()
        .map(|item| {
            let read = |key: &str| {
                item.get(key)
                    .and_then(Value::as_str)
                    .map(|text| profile.body.exact_string(text))
                    .unwrap_or_default()
            };
            if directive {
                ReferenceRow {
                    id: read("code"),
                    name: read("name"),
                    rationale: read("rationale"),
                }
            } else {
                ReferenceRow {
                    id: read("id"),
                    name: PythonString::default(),
                    rationale: read("rationale"),
                }
            }
        })
        .collect()
}

fn render_directives(
    inputs: &ProfileSectionsInputs<'_>,
) -> Result<Vec<PythonString>, ProfileSectionsError> {
    let rows = reference_rows(inputs.profile, "directive-references", true);
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let mut lines = vec![header("Profile-Cited Directives", inputs.profile)];
    for row in rows {
        let code = format_profile_directive_code(&row.id);
        let mut entry = PythonString::from_scalars("  - ");
        entry.append(&code);
        entry.push_scalars(": ");
        entry.append(&row.name);
        if !row.rationale.is_empty() {
            entry.push_scalars(" — ");
            entry.append(&row.rationale);
        }
        lines.push(entry);
        lines.extend(render_entry(
            inputs,
            ArtifactKind::Directive,
            "directive",
            &code,
            PROFILE_CODE_CHANGE_WHEN,
            BodyMode::Directive,
        )?);
    }
    Ok(lines)
}

fn render_tactics(
    inputs: &ProfileSectionsInputs<'_>,
) -> Result<Vec<PythonString>, ProfileSectionsError> {
    let rows = reference_rows(inputs.profile, "tactic-references", false);
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let mut lines = vec![header("Profile-Cited Tactics", inputs.profile)];
    for row in rows {
        let id = exact_strip(&row.id);
        lines.push(entry_header(&id, &row.rationale));
        lines.extend(render_entry(
            inputs,
            ArtifactKind::Tactic,
            "tactic",
            &id,
            PROFILE_CODE_CHANGE_WHEN,
            BodyMode::Named,
        )?);
    }
    Ok(lines)
}

fn render_styleguides(
    inputs: &ProfileSectionsInputs<'_>,
) -> Result<Vec<PythonString>, ProfileSectionsError> {
    render_pointer_only(
        inputs,
        "styleguide-references",
        "Profile-Cited Styleguides",
        ArtifactKind::Styleguide,
        "styleguide",
    )
}

fn render_toolguides(
    inputs: &ProfileSectionsInputs<'_>,
) -> Result<Vec<PythonString>, ProfileSectionsError> {
    render_pointer_only(
        inputs,
        "toolguide-references",
        "Profile-Cited Toolguides",
        ArtifactKind::Toolguide,
        "toolguide",
    )
}

fn render_pointer_only(
    inputs: &ProfileSectionsInputs<'_>,
    field: &str,
    title: &str,
    kind: ArtifactKind,
    selector_kind: &str,
) -> Result<Vec<PythonString>, ProfileSectionsError> {
    let rows = reference_rows(inputs.profile, field, false);
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let mut lines = vec![header(title, inputs.profile)];
    for row in rows {
        let id = exact_strip(&row.id);
        lines.push(entry_header(&id, &row.rationale));
        lines.extend(render_entry(
            inputs,
            kind,
            selector_kind,
            &id,
            PROFILE_CODE_CHANGE_WHEN,
            BodyMode::None,
        )?);
    }
    Ok(lines)
}

fn render_procedures(
    inputs: &ProfileSectionsInputs<'_>,
) -> Result<Vec<PythonString>, ProfileSectionsError> {
    let reached = channel_reached(inputs.graph, inputs.profile);
    let prefix = "procedure:";
    let procedure_ids: BTreeSet<PythonString> = reached
        .iter()
        .filter_map(|urn| urn.strip_prefix(prefix))
        .map(PythonString::from_scalars)
        .collect();
    if procedure_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut lines = vec![header("Profile-Resolved Procedures", inputs.profile)];
    for procedure_id in procedure_ids {
        let mut entry = PythonString::from_scalars("  - ");
        entry.append(&procedure_id);
        lines.push(entry);
        lines.extend(render_entry(
            inputs,
            ArtifactKind::Procedure,
            "procedure",
            &procedure_id,
            PROFILE_PROCEDURE_WHEN,
            BodyMode::Named,
        )?);
    }
    Ok(lines)
}

fn render_suggested(
    inputs: &ProfileSectionsInputs<'_>,
) -> Result<Vec<PythonString>, ProfileSectionsError> {
    let reached = channel_reached(inputs.graph, inputs.profile);
    if reached.is_empty() {
        return Ok(Vec::new());
    }
    let delivered: BTreeSet<String> = reached
        .iter()
        .filter(|urn| {
            let kind = kind_of(urn);
            SUGGESTS_DELIVERED_KINDS.contains(&kind)
        })
        .cloned()
        .collect();
    if delivered.is_empty() {
        return Ok(Vec::new());
    }
    let Some(seed) = profile_urn(inputs.profile) else {
        return Ok(Vec::new());
    };
    let seeds: BTreeSet<String> = BTreeSet::from([seed.clone()]);
    let references = profile_channel_references(inputs.graph, &seeds, &reached, &delivered);

    let mut lines: Vec<PythonString> = Vec::new();
    for (kind_token, title) in SUGGESTS_KIND_RENDER {
        let delivered_ids: BTreeSet<String> = delivered
            .iter()
            .filter(|urn| kind_of(urn) == kind_token)
            .map(|urn| bare_id(urn).to_owned())
            .collect();
        if delivered_ids.is_empty() {
            continue;
        }
        let (entries, when_by) = consolidate_kind_entries(&references, &delivered_ids);
        if entries.is_empty() {
            continue;
        }
        let Some(kind) = artifact_kind(kind_token) else {
            continue;
        };
        lines.push(suggested_header(title, inputs.profile));
        for (id, rationale) in entries {
            let mut entry = PythonString::from_scalars("  - ");
            entry.append(&id);
            if !rationale.is_empty() {
                entry.push_scalars(": ");
                entry.append(&rationale);
            }
            lines.push(entry);
            let when = when_by
                .get(&id.to_scalar_string().unwrap_or_default())
                .cloned()
                .flatten()
                .filter(|when| !when.is_empty())
                .unwrap_or_else(|| STATED_DEFAULT_WHEN.to_owned());
            lines.extend(render_entry(
                inputs,
                kind,
                kind_token,
                &id,
                &when,
                BodyMode::None,
            )?);
        }
    }
    Ok(lines)
}

/// The ordered `(id, rationale)` entry list one suggested kind renders.
type KindEntries = Vec<(PythonString, PythonString)>;
/// The per-entry `when` override (`None` falls back to the section clause).
type WhenById = BTreeMap<String, Option<String>>;

/// The reference `_consolidate_kind_entries`.
fn consolidate_kind_entries(
    references: &[crate::context::EdgeReference],
    delivered_ids: &BTreeSet<String>,
) -> (KindEntries, WhenById) {
    let mut entries: KindEntries = Vec::new();
    let mut when_by: WhenById = BTreeMap::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for reference in references {
        if !delivered_ids.contains(&reference.id) {
            continue;
        }
        if seen.insert(reference.id.clone()) {
            entries.push((
                PythonString::from_scalars(&reference.id),
                reference
                    .reason
                    .as_deref()
                    .map(PythonString::from_scalars)
                    .unwrap_or_default(),
            ));
        }
        let current = when_by.get(&reference.id);
        if current.is_none_or(|when| when.as_deref().is_none_or(str::is_empty)) {
            when_by.insert(reference.id.clone(), reference.when.clone());
        }
    }
    (entries, when_by)
}

/// The reference `profile_channel_references` projection.
fn profile_channel_references(
    graph: &DrgGraph,
    seeds: &BTreeSet<String>,
    reached: &BTreeSet<String>,
    delivered: &BTreeSet<String>,
) -> Vec<crate::context::EdgeReference> {
    let inline = requires_closure(graph, seeds);
    let link: BTreeSet<String> = delivered.difference(&inline).cloned().collect();
    let mut bridge: BTreeSet<String> = reached.clone();
    bridge.extend(seeds.iter().cloned());
    link_references(graph, seeds, &link, &bridge)
}

/// Every artefact URN the profile channel reaches (seeds stripped, fail-closed).
fn channel_reached(graph: &DrgGraph, profile: &AgentProfile) -> BTreeSet<String> {
    let Some(seed) = profile_urn(profile) else {
        return BTreeSet::new();
    };
    if graph
        .node(&seed)
        .is_none_or(|node| node.kind() != NodeKind::AgentProfile)
    {
        return BTreeSet::new();
    }
    let seeds: BTreeSet<String> = BTreeSet::from([seed]);
    let visited = crate::query::walk_edges(graph, &seeds, &profile_channel_relations(), None);
    visited.difference(&seeds).cloned().collect()
}

fn profile_urn(profile: &AgentProfile) -> Option<String> {
    let id = profile.id.to_scalar_string().ok()?;
    Some(format!("agent_profile:{id}"))
}

fn kind_of(urn: &str) -> &str {
    urn.split_once(':').map_or("", |(kind, _)| kind)
}

fn bare_id(urn: &str) -> &str {
    urn.split_once(':').map_or(urn, |(_, id)| id)
}

fn artifact_kind(token: &str) -> Option<ArtifactKind> {
    ArtifactKind::from_operator_token(token).ok()
}

/// How one profile entry renders its body.
#[derive(Clone, Copy)]
enum BodyMode {
    /// Pointer-only (`body_fn=None`): always the fetch stanza.
    None,
    /// The reference `format_inline_named_body` (tactic/procedure profile shape).
    Named,
    /// The reference `_format_inline_directive_body`.
    Directive,
}

fn render_entry(
    inputs: &ProfileSectionsInputs<'_>,
    kind: ArtifactKind,
    selector_kind: &str,
    artifact_id: &PythonString,
    when_clause: &str,
    mode: BodyMode,
) -> Result<Vec<PythonString>, ProfileSectionsError> {
    // The Source catch encloses repository lookup only. Pointer-only entries
    // never read/dump a body, including admitted Any values that fail JSON-mode
    // serialization. Do not fabricate a null payload to stand for that laziness.
    match inputs.catalog.artifact_exact(kind, artifact_id) {
        Err(_) | Ok(None) => Ok(miss_lines(inputs, selector_kind, artifact_id, kind)),
        Ok(Some(record)) => {
            let root = record.exact_root();
            let body = match mode {
                BodyMode::None => Vec::new(),
                BodyMode::Named | BodyMode::Directive => {
                    let payload = body_payload(kind, artifact_id, root)?;
                    match mode {
                        BodyMode::Named => {
                            profile_named_body(&payload, step_models(kind, root).as_deref())
                        }
                        BodyMode::Directive => profile_directive_body(&payload),
                        BodyMode::None => unreachable!("pointer mode bypasses body projection"),
                    }
                }
            };
            if !body.is_empty() && budget_estimate(&body) <= PROFILE_INLINE_BODY_LIMIT_CHARS {
                return Ok(body);
            }
            let mut selector = PythonString::from_scalars(selector_kind);
            selector.push_scalars(":");
            selector.append(artifact_id);
            Ok(fetch_stanza_lines_exact(
                &selector,
                &PythonString::from_scalars(when_clause),
                &PythonString::from_scalars(PROFILE_INDENT),
                inputs.scope.as_ref(),
            ))
        }
    }
}

/// Project an actually requested body, outside the repository-lookup catch.
fn body_payload(
    kind: ArtifactKind,
    artifact_id: &PythonString,
    root: Option<sk_kernel::python_yaml::NodeRef<'_>>,
) -> Result<PythonJsonValue, ProfileSectionsError> {
    let Some(root) = root else {
        return Err(ProfileSectionsError::PendingProjection {
            kind,
            artifact_id: artifact_id.clone(),
        });
    };
    match crate::body::exact_payload(kind, root) {
        Ok(payload) => Ok(payload),
        Err(crate::body::PayloadError::NoSchema) => Err(ProfileSectionsError::PendingProjection {
            kind,
            artifact_id: artifact_id.clone(),
        }),
        Err(crate::body::PayloadError::Failed(cause)) => Err(ProfileSectionsError::Payload {
            kind,
            artifact_id: artifact_id.clone(),
            cause,
        }),
    }
}

/// The structured miss stanza plus the canonical warning.
fn miss_lines(
    inputs: &ProfileSectionsInputs<'_>,
    selector_kind: &str,
    artifact_id: &PythonString,
    kind: ArtifactKind,
) -> Vec<PythonString> {
    let diagnosis = diagnose_catalog_miss(artifact_id, inputs.catalog, kind, None);
    if let Some(sink) = inputs.warning_sink {
        let profile_id = inputs
            .profile
            .id
            .to_scalar_string()
            .unwrap_or_else(|_| inputs.profile.id.repr());
        let context = format!("profile:{profile_id}");
        let _ =
            emit_catalog_miss_warning(selector_kind, artifact_id, &diagnosis, Some(&context), sink);
    }
    format_catalog_miss_stanza(selector_kind, artifact_id, &diagnosis, PROFILE_INDENT)
}

fn header(title: &str, profile: &AgentProfile) -> PythonString {
    let mut line = PythonString::from_scalars(title);
    line.push_scalars(" (");
    line.append(&profile.id);
    line.push_scalars("):");
    line
}

fn suggested_header(title: &str, profile: &AgentProfile) -> PythonString {
    let mut line = PythonString::from_scalars("Profile-Suggested ");
    line.push_scalars(title);
    line.push_scalars(" (");
    line.append(&profile.id);
    line.push_scalars("):");
    line
}

fn entry_header(id: &PythonString, rationale: &PythonString) -> PythonString {
    let mut line = PythonString::from_scalars("  - ");
    line.append(id);
    if !rationale.is_empty() {
        line.push_scalars(": ");
        line.append(rationale);
    }
    line
}

fn exact_string_field(payload: &PythonJsonValue, field: &str) -> Option<PythonString> {
    match payload.field(field) {
        Some(PythonJsonValue::String(text)) => Some(text.clone()),
        _ => None,
    }
}

/// The canonical typed step models of a tactic/procedure record.
///
/// `None` when the record has no canonical graph, is not shaped like its Source
/// model (the reference's model validation refuses such a document before any
/// formatter exists), or a member is outside this view's current capability.
/// The payload path then stays the honest rendering of that unadmitted shape;
/// no model is fabricated for it.
fn step_models<'a>(
    kind: ArtifactKind,
    root: Option<sk_kernel::python_yaml::NodeRef<'a>>,
) -> Option<Vec<ModelObject>> {
    let root = root?;
    crate::body::model_list(kind, root, "steps").ok().flatten()
}

/// The reference `format_inline_named_body` (profile tactic/procedure shape).
///
/// Distinct from the include formatter: `Name` is interpolated raw (not stripped),
/// a step header is `title or description or str(step)` evaluated lazily, and
/// procedure entry/exit conditions are not rendered. The step header is read
/// from the canonical typed model view when the record is model-shaped, so the
/// fallback is the Source model's own `str(step)` rather than a JSON rendering.
fn profile_named_body(
    payload: &PythonJsonValue,
    steps: Option<&[ModelObject]>,
) -> Vec<PythonString> {
    let mut body = Vec::new();
    if let Some(name) = exact_string_field(payload, "name").filter(|name| !name.is_empty()) {
        body.push(prefixed("    Name: ", &name));
    }
    if let Some(purpose) = exact_string_field(payload, "purpose") {
        let purpose = exact_strip(&purpose);
        if !purpose.is_empty() {
            body.push(prefixed("    Purpose: ", &purpose));
        }
    }
    if let Some(PythonJsonValue::Array(payload_steps)) = payload.field("steps")
        && !payload_steps.is_empty()
    {
        body.push(PythonString::from_scalars("    Steps:"));
        for (index, payload_step) in payload_steps.iter().enumerate() {
            let model = steps.and_then(|steps| steps.get(index));
            body.extend(profile_step_lines(model, payload_step));
        }
    }
    body
}

/// One profile step's header line and (when distinct) its description sub-line.
///
/// Source:
///
/// ```text
/// header = title or description or str(step)
/// if title.strip() and description.strip(): emit the description sub-line
/// ```
fn profile_step_lines(model: Option<&ModelObject>, payload: &PythonJsonValue) -> Vec<PythonString> {
    let mut lines = Vec::new();
    let (title, description, header) = match model {
        Some(step) => {
            // Source truthiness (`title or description or str(step)`) decides the
            // header; the text then stands in for it.
            let title = step.field("title");
            let description = step.field("description");
            let header = title
                .filter(|title| title.truthy())
                .or_else(|| description.filter(|description| description.truthy()))
                .map_or_else(|| step.python_str(), ModelValue::python_str);
            (
                title.and_then(ModelValue::as_text).cloned(),
                description.and_then(ModelValue::as_text).cloned(),
                header,
            )
        }
        // No Source model for this member: the reference's model validation
        // refuses such a record, so there is no `str(step)` to render. Keep the
        // payload's own title/description policy for that unadmitted shape.
        None => {
            let title = exact_string_field(payload, "title");
            let description = exact_string_field(payload, "description");
            let header = title
                .clone()
                .filter(|title| !title.is_empty())
                .or_else(|| {
                    description
                        .clone()
                        .filter(|description| !description.is_empty())
                })
                .unwrap_or_else(|| payload.render_python_with(PythonJsonRenderOptions::default()));
            (title, description, header)
        }
    };
    lines.push(prefixed("      - ", &header));
    if let (Some(title), Some(description)) = (title, description)
        && !exact_strip(&title).is_empty()
        && !exact_strip(&description).is_empty()
    {
        lines.push(prefixed("        ", &exact_strip(&description)));
    }
    lines
}

/// The reference `_format_inline_directive_body`.
fn profile_directive_body(payload: &PythonJsonValue) -> Vec<PythonString> {
    let mut body = Vec::new();
    if let Some(intent) = exact_string_field(payload, "intent") {
        let intent = exact_strip(&intent);
        if !intent.is_empty() {
            body.push(prefixed("    Intent: ", &intent));
        }
    }
    if let Some(scope) = exact_string_field(payload, "scope") {
        let scope = exact_strip(&scope);
        if !scope.is_empty() {
            body.push(prefixed("    Scope: ", &scope));
        }
    }
    for (field, label) in [
        ("procedures", "Procedures"),
        ("integrity_rules", "Integrity rules"),
        ("validation_criteria", "Validation criteria"),
    ] {
        let Some(PythonJsonValue::Array(items)) = payload.field(field) else {
            continue;
        };
        if items.is_empty() {
            continue;
        }
        body.push(PythonString::from_scalars(&format!("    {label}:")));
        for item in items {
            match item {
                PythonJsonValue::String(text) => body.push(prefixed("      - ", text)),
                other => body.push(prefixed(
                    "      - ",
                    &other.render_python_with(PythonJsonRenderOptions::default()),
                )),
            }
        }
    }
    body
}

fn prefixed(prefix: &str, value: &PythonString) -> PythonString {
    let mut line = PythonString::from_scalars(prefix);
    line.append(value);
    line
}

fn join_lines(lines: &[PythonString], separator: &str) -> PythonString {
    let mut text = PythonString::default();
    for (position, line) in lines.iter().enumerate() {
        if position > 0 {
            text.push_scalars(separator);
        }
        text.append(line);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directive_codes_normalise_like_the_source_formatter() {
        let code = |raw: &str| {
            format_profile_directive_code(&PythonString::from_scalars(raw))
                .to_scalar_string()
                .unwrap()
        };
        assert_eq!(code("010"), "DIRECTIVE_010");
        assert_eq!(code("DIRECTIVE_003"), "DIRECTIVE_003");
        assert_eq!(code("025-boy-scout-rule"), "DIRECTIVE_025");
        assert_eq!(code("  044 "), "DIRECTIVE_044");
        assert_eq!(code("12_x"), "DIRECTIVE_012");
        assert_eq!(code("12a-3"), "12a-3");
    }
}
