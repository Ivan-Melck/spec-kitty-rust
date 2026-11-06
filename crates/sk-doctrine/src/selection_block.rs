//! Source `charter.activation.context_renderers.selection_block` — the combined
//! eight-kind global-selection prompt block.
//!
//! One owner for the reference's render-pure seam:
//!
//! * the eight `_render_selected_<kind>` sections (paradigm / directive / tactic /
//!   styleguide / toolguide / procedure / agent_profile / mission_step_contract),
//!   in the reference's fixed order, headers and when-doing clauses;
//! * the shared section routine (`_render_selected_artifacts`): per-entry
//!   de-duplication in authored order, the org provenance suffix, the identity
//!   resolved repository key, the repository-lookup catch, the structured
//!   catalogue-miss stanza + warning, the inline body-or-fetch decision at the
//!   per-entry budget, and the fetch + when-doing stanza;
//! * the org provenance map (`_collect_org_source_map`) and the org-required
//!   merge (`_merge_org_required`), computed once per build.
//!
//! Everything the block reads is an existing canonical authority: the resolved
//! [`GovernanceCharterSelection`] projection of the one config reader, the
//! layered [`CanonicalOfferingService`] records, the [`IdentityIndex`], the
//! catalogue-miss diagnosis owner, the typed [`crate::body`] model view behind
//! the one inline body formatter, and the [`crate::text_budget`] per-entry
//! limit. Nothing here is a second schema, repository, roster or parser.
//!
//! # Error phases (Source-faithful)
//!
//! * a **repository read** failure while collecting provenance is the reference's
//!   raising repository property: it propagates, it is never a catalogue miss;
//! * a **body projection** failure outside the lookup catch propagates with its
//!   own cause;
//! * a **lookup** failure (or `None`) is the reference's caught miss: the
//!   structured stanza and warning are rendered and the section continues.

use std::collections::BTreeMap;
use std::path::Path;

use sk_charter::ArtifactKind;
use sk_kernel::PythonString;
use sk_kernel::python_json::PythonJsonValue;

use crate::catalog_diagnosis::{
    diagnose_catalog_miss, emit_catalog_miss_warning, format_catalog_miss_stanza,
};
use crate::context_render::{FetchScope, exact_strip, fetch_stanza_lines_exact, joined};
use crate::identity::IdentityIndex;
use crate::offering::{CanonicalOfferingService, OfferingLoadError};
use crate::project_governance::GovernanceCharterSelection;
use crate::text_budget::{PROFILE_INLINE_BODY_LIMIT_CHARS, budget_estimate};

/// The reference `_SELECTED_*_HEADER` / when-clause roster, in render order.
///
/// The kind order is the reference's own render policy: it is deliberately not
/// the canonical `ArtifactKind` member order.
struct SelectionSection {
    kind: ArtifactKind,
    header: &'static str,
    when_clause: &'static str,
}

const SECTIONS: [SelectionSection; 8] = [
    SelectionSection {
        kind: ArtifactKind::Paradigm,
        header: "Selected paradigms:",
        when_clause: "are about to choose a reasoning frame",
    },
    SelectionSection {
        kind: ArtifactKind::Directive,
        header: "Selected directives:",
        when_clause: ACTION_DOCTRINE_LINK_WHEN,
    },
    SelectionSection {
        kind: ArtifactKind::Tactic,
        header: "Selected tactics:",
        when_clause: ACTION_DOCTRINE_LINK_WHEN,
    },
    SelectionSection {
        kind: ArtifactKind::Styleguide,
        header: "Selected styleguides:",
        when_clause: "are about to write a code comment or styled output",
    },
    SelectionSection {
        kind: ArtifactKind::Toolguide,
        header: "Selected toolguides:",
        when_clause: "are about to invoke a project tool",
    },
    SelectionSection {
        kind: ArtifactKind::Procedure,
        header: "Selected procedures:",
        when_clause: "are about to follow a multi-step workflow",
    },
    SelectionSection {
        kind: ArtifactKind::AgentProfile,
        header: "Selected agent profiles:",
        when_clause: ACTION_DOCTRINE_LINK_WHEN,
    },
    SelectionSection {
        kind: ArtifactKind::MissionStepContract,
        header: "Selected mission step contracts:",
        when_clause: "are about to step a mission action",
    },
];

/// The reference `_ACTION_DOCTRINE_LINK_WHEN` clause.
const ACTION_DOCTRINE_LINK_WHEN: &str = "are about to apply a code change";

/// The reference `_render_selected_artifacts` indent for bodies and stanzas.
const SELECTION_INDENT: &str = "    ";

/// The inputs one prepared global-selection block reads.
///
/// `charter` is the resolved reference `GovernanceCharterConfig`: `None` (or a
/// `None` repository service, which Native always has) renders the reference's
/// empty block without building an identity index or reading org packs.
#[derive(Clone, Copy)]
pub struct SelectionBlockInputs<'a> {
    /// The effective selection projection (`None` = the reference's `None` config).
    pub charter: Option<&'a GovernanceCharterSelection>,
    /// The canonical layered offering repositories.
    pub catalog: &'a CanonicalOfferingService,
    /// The one identity index for this invocation, when the caller holds it.
    pub identity: Option<&'a IdentityIndex>,
    /// The repository root (identity build + org-required read + miss diagnosis).
    pub repo_root: Option<&'a Path>,
    /// Optional read-only fetch scope for `--owned-checkout`/`--mission`.
    pub scope: Option<FetchScope<'a>>,
    /// The caller's warning channel for catalogue misses.
    pub warning_sink: Option<&'a dyn Fn(&str)>,
}

impl std::fmt::Debug for SelectionBlockInputs<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SelectionBlockInputs")
            .field("charter", &self.charter.is_some())
            .field("identity", &self.identity.is_some())
            .field("repo_root", &self.repo_root.is_some())
            .field("scope", &self.scope.is_some())
            .field("warning_sink", &self.warning_sink.is_some())
            .finish_non_exhaustive()
    }
}

/// Why a prepared global-selection block could not be rendered.
///
/// Only the reference's *raising* phases appear here. A repository lookup miss
/// is not an error: it renders the structured miss stanza, exactly like the
/// reference.
#[derive(Debug)]
pub enum SelectionBlockError {
    /// A repository property read (the reference's lazy repository load).
    Offering(OfferingLoadError),
    /// The one identity index could not be built for the requested root.
    Identity(crate::error::DoctrineError),
    /// The org-required selection read failed.
    OrgRequired(crate::error::DoctrineError),
    /// A record has no canonical graph in this crate (a Native projection
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

impl std::fmt::Display for SelectionBlockError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Offering(error) => write!(formatter, "{error}"),
            Self::Identity(error) | Self::OrgRequired(error) => write!(formatter, "{error}"),
            Self::PendingProjection { kind, artifact_id } => write!(
                formatter,
                "global-selection body for {}:{} has no canonical payload in this crate",
                kind.canonical(),
                artifact_id.repr()
            ),
            Self::Payload {
                kind,
                artifact_id,
                cause,
            } => write!(
                formatter,
                "global-selection body for {}:{} was refused: {cause:?}",
                kind.canonical(),
                artifact_id.repr()
            ),
        }
    }
}

impl std::error::Error for SelectionBlockError {}

/// Render the combined eight-kind selection block (the reference
/// `_render_selection_block`).
///
/// Returns the empty string when there is no config, no selected id, or no
/// non-empty section — the caller then skips the leading blank line, exactly like
/// the reference.
///
/// # Errors
///
/// See [`SelectionBlockError`]: the reference's raising repository, identity,
/// org-required and body-projection phases.
pub fn render_selection_block(
    inputs: &SelectionBlockInputs<'_>,
) -> Result<PythonString, SelectionBlockError> {
    let Some(charter) = inputs.charter else {
        return Ok(PythonString::default());
    };

    // FR-017: when the caller holds no index and the root is known, one is built
    // for this call (the reference's `IdentityIndex.build(repo_root)`).
    let owned_identity;
    let identity = match (inputs.identity, inputs.repo_root) {
        (Some(identity), _) => Some(identity),
        (None, Some(root)) => {
            owned_identity =
                IdentityIndex::from_project(root).map_err(SelectionBlockError::Identity)?;
            Some(&owned_identity)
        }
        (None, None) => None,
    };

    // The org-required selection read happens ONCE per build (the reference's
    // `_read_org_required_selections`), before any section is rendered.
    let org_required = match inputs.repo_root {
        Some(root) => crate::org_charter::read_org_required_selections_exact(root)
            .map_err(SelectionBlockError::OrgRequired)?,
        None => BTreeMap::new(),
    };

    // Source loads each kind's repository then collects its provenance before
    // advancing to the next kind (not all repository loads before all maps).
    let mut org_maps: Vec<(ArtifactKind, BTreeMap<PythonString, PythonString>)> = Vec::new();
    for section in &SECTIONS {
        load_repository(inputs.catalog, section.kind).map_err(SelectionBlockError::Offering)?;
        let selected = charter.selected(section.kind);
        let catalog_map = collect_org_source_map(inputs.catalog, selected, section.kind, identity)?;
        let merged = merge_org_required(
            catalog_map,
            selected,
            org_required
                .get(&section.kind)
                .map(Vec::as_slice)
                .unwrap_or(&[]),
            section.kind,
            identity,
        );
        org_maps.push((section.kind, merged));
    }

    let mut blocks: Vec<PythonString> = Vec::new();
    for (index, section) in SECTIONS.iter().enumerate() {
        let org_map = org_maps
            .get(index)
            .map(|(_, map)| map)
            .unwrap_or_else(|| unreachable!("one provenance map per section"));
        let lines = render_selected_artifacts(inputs, section, org_map, identity)?;
        if !lines.is_empty() {
            blocks.push(joined(&lines, "\n"));
        }
    }
    Ok(joined(&blocks, "\n\n"))
}

/// Ensure one kind's canonical repository is loaded (the reference's raising
/// lazy property), so its failure escapes before any lookup.
fn load_repository(
    catalog: &CanonicalOfferingService,
    kind: ArtifactKind,
) -> Result<(), OfferingLoadError> {
    if kind == ArtifactKind::AgentProfile {
        catalog.profile_repository().map(|_| ())
    } else {
        catalog.artifact_ids(kind).map(|_| ())
    }
}

/// The reference `_provenance_suffix`.
///
/// `org_source_map` maps an authored token to the contributing pack name; the
/// empty-string sentinel (the repository has no per-pack attribution here)
/// collapses to `(source: org)` so the provenance signal survives.
#[must_use]
pub fn provenance_suffix(
    artifact_id: &PythonString,
    org_source_map: Option<&BTreeMap<PythonString, PythonString>>,
) -> PythonString {
    let Some(map) = org_source_map else {
        return PythonString::default();
    };
    let Some(pack) = map.get(artifact_id) else {
        return PythonString::default();
    };
    let mut suffix = PythonString::from_scalars(" (source: org");
    let stripped = exact_strip(pack);
    if !stripped.is_empty() {
        suffix.push_scalars(", pack: ");
        suffix.append(&stripped);
    }
    suffix.push_scalars(")");
    suffix
}

/// The reference `_selection_id`: the repository key of a selected token.
///
/// The identity index turns a file stem or a declared id into the declared id
/// with selection precedence; a token no index knows falls back to the kind's
/// own normalisation. When there is no index the fallback alone applies.
fn selection_id(
    identity: Option<&IdentityIndex>,
    kind: ArtifactKind,
    token: &PythonString,
) -> PythonString {
    match identity {
        Some(identity) => identity.lookup_key_exact(Some(kind), token, true),
        None => IdentityIndex::unindexed_key_exact(Some(kind), token),
    }
}

/// The reference `_collect_org_source_map`, computed once per build per kind.
///
/// A repository read failure propagates (the reference's property read is
/// outside the lookup catch). A repository without a `get_provenance` accessor
/// contributes no org entry for that id — never a fabricated label.
fn collect_org_source_map(
    catalog: &CanonicalOfferingService,
    artifact_ids: &[PythonString],
    kind: ArtifactKind,
    identity: Option<&IdentityIndex>,
) -> Result<BTreeMap<PythonString, PythonString>, SelectionBlockError> {
    let mut map: BTreeMap<PythonString, PythonString> = BTreeMap::new();
    if artifact_ids.is_empty() {
        return Ok(map);
    }
    for artifact_id in artifact_ids {
        let label = if kind == ArtifactKind::AgentProfile {
            let repository = catalog
                .profile_repository()
                .map_err(SelectionBlockError::Offering)?;
            repository
                .provenance(&selection_id(identity, kind, artifact_id))
                .map(str::to_owned)
        } else {
            let record = catalog
                .artifact_exact(kind, &selection_id(identity, kind, artifact_id))
                .map_err(SelectionBlockError::Offering)?;
            record
                .as_ref()
                .and_then(|record| record.provenance_label())
                .map(str::to_owned)
        };
        if label.as_deref() == Some("org") {
            // The reference's empty-string sentinel: no per-pack attribution on
            // this layer, so the suffix collapses to `(source: org)`.
            map.insert(artifact_id.clone(), PythonString::default());
        }
    }
    Ok(map)
}

/// The reference `_merge_org_required`: annotate org-required selected ids.
///
/// A selected token and a required token name one artifact when they resolve to
/// the same declared id, whichever of stem or id each one spells. The
/// catalog-derived map wins when both are present; a required id that is not
/// selected is never added to the block.
fn merge_org_required(
    catalog_map: BTreeMap<PythonString, PythonString>,
    selected_ids: &[PythonString],
    required_ids: &[PythonString],
    kind: ArtifactKind,
    identity: Option<&IdentityIndex>,
) -> BTreeMap<PythonString, PythonString> {
    let mut merged = catalog_map;
    if required_ids.is_empty() {
        return merged;
    }
    let required: std::collections::BTreeSet<PythonString> = required_ids
        .iter()
        .map(|required| selection_id(identity, kind, required))
        .collect();
    for selected in selected_ids {
        if !merged.contains_key(selected)
            && required.contains(&selection_id(identity, kind, selected))
        {
            merged.insert(selected.clone(), PythonString::default());
        }
    }
    merged
}

/// The reference `_render_selected_artifacts` for one section kind.
fn render_selected_artifacts(
    inputs: &SelectionBlockInputs<'_>,
    section: &SelectionSection,
    org_source_map: &BTreeMap<PythonString, PythonString>,
    identity: Option<&IdentityIndex>,
) -> Result<Vec<PythonString>, SelectionBlockError> {
    let selected = inputs
        .charter
        .map(|charter| charter.selected(section.kind))
        .unwrap_or(&[]);
    if selected.is_empty() {
        return Ok(Vec::new());
    }
    let mut lines: Vec<PythonString> = vec![PythonString::from_scalars(section.header)];
    let mut seen: std::collections::BTreeSet<PythonString> = std::collections::BTreeSet::new();
    for artifact_id in selected {
        // De-duplicate while preserving authoring order.
        if !seen.insert(artifact_id.clone()) {
            continue;
        }
        let mut header_line = PythonString::from_scalars("  - ");
        header_line.append(artifact_id);
        header_line.append(&provenance_suffix(artifact_id, Some(org_source_map)));
        lines.push(header_line);

        let body = match lookup_body(inputs, section.kind, artifact_id, identity)? {
            Lookup::Missing => {
                lines.extend(miss_lines(inputs, section, artifact_id));
                None
            }
            Lookup::Body(body) => Some(body),
        };
        let Some(body) = body else {
            lines.extend(fetch_lines(section, artifact_id, inputs.scope));
            continue;
        };
        if !body.is_empty() && budget_estimate(&body) <= PROFILE_INLINE_BODY_LIMIT_CHARS {
            lines.extend(body);
        } else {
            lines.extend(fetch_lines(section, artifact_id, inputs.scope));
        }
    }
    Ok(lines)
}

/// One admitted lookup's outcome: an inline body, or the reference's caught miss.
enum Lookup {
    Missing,
    Body(Vec<PythonString>),
}

/// Resolve one selected token to its inline body.
///
/// The reference's catch encloses the repository lookup only: a repository
/// failure is a miss (`artifact = None`), while a body/projection failure outside
/// that catch escapes.
fn lookup_body(
    inputs: &SelectionBlockInputs<'_>,
    kind: ArtifactKind,
    artifact_id: &PythonString,
    identity: Option<&IdentityIndex>,
) -> Result<Lookup, SelectionBlockError> {
    let key = selection_id(identity, kind, artifact_id);
    if kind == ArtifactKind::AgentProfile {
        let repository = match inputs.catalog.profile_repository() {
            Ok(repository) => repository,
            Err(_) => return Ok(Lookup::Missing),
        };
        let Some(profile) = repository.get(&key) else {
            return Ok(Lookup::Missing);
        };
        let dump = profile.model_dump();
        let payload = match dump.model_json_exact() {
            Ok(payload) => payload,
            Err(crate::body::PayloadError::NoSchema) => {
                return Err(SelectionBlockError::PendingProjection {
                    kind,
                    artifact_id: artifact_id.clone(),
                });
            }
            Err(crate::body::PayloadError::Failed(cause)) => {
                return Err(SelectionBlockError::Payload {
                    kind,
                    artifact_id: artifact_id.clone(),
                    cause,
                });
            }
        };
        return Ok(Lookup::Body(inline_body(kind, &payload, None)));
    }
    let record = match inputs.catalog.artifact_exact(kind, &key) {
        Ok(Some(record)) => record,
        Ok(None) | Err(_) => return Ok(Lookup::Missing),
    };
    let Some(root) = record.exact_root() else {
        return Err(SelectionBlockError::PendingProjection {
            kind,
            artifact_id: artifact_id.clone(),
        });
    };
    let fields = crate::context_render::inline_body_field_names(kind.canonical());
    let payload = match crate::body::selected_payload_fields(kind, root, &fields) {
        Ok(payload) => payload,
        Err(crate::body::PayloadError::NoSchema) => {
            return Err(SelectionBlockError::PendingProjection {
                kind,
                artifact_id: artifact_id.clone(),
            });
        }
        Err(crate::body::PayloadError::Failed(cause)) => {
            return Err(SelectionBlockError::Payload {
                kind,
                artifact_id: artifact_id.clone(),
                cause,
            });
        }
    };
    let steps = if matches!(kind, ArtifactKind::Tactic | ArtifactKind::Procedure) {
        crate::body::model_list(kind, root, "steps").map_err(|error| match error {
            crate::body::PayloadError::NoSchema => SelectionBlockError::PendingProjection {
                kind,
                artifact_id: artifact_id.clone(),
            },
            crate::body::PayloadError::Failed(cause) => SelectionBlockError::Payload {
                kind,
                artifact_id: artifact_id.clone(),
                cause,
            },
        })?
    } else {
        None
    };
    Ok(Lookup::Body(inline_body(kind, &payload, steps.as_deref())))
}

/// The one inline body formatter (the reference `_format_inline_*_body`).
fn inline_body(
    kind: ArtifactKind,
    payload: &PythonJsonValue,
    steps: Option<&[crate::body::ModelObject]>,
) -> Vec<PythonString> {
    crate::context_render::include_body_lines_exact_with_steps(kind.canonical(), payload, steps)
}

/// The structured catalogue-miss stanza plus the canonical warning.
fn miss_lines(
    inputs: &SelectionBlockInputs<'_>,
    section: &SelectionSection,
    artifact_id: &PythonString,
) -> Vec<PythonString> {
    let diagnosis =
        diagnose_catalog_miss(artifact_id, inputs.catalog, section.kind, inputs.repo_root);
    if let Some(sink) = inputs.warning_sink {
        let _ = emit_catalog_miss_warning(
            section.kind.canonical(),
            artifact_id,
            &diagnosis,
            None,
            sink,
        );
    }
    format_catalog_miss_stanza(
        section.kind.canonical(),
        artifact_id,
        &diagnosis,
        SELECTION_INDENT,
    )
}

/// The canonical fetch + when-doing stanza for one selected id.
fn fetch_lines(
    section: &SelectionSection,
    artifact_id: &PythonString,
    scope: Option<FetchScope<'_>>,
) -> Vec<PythonString> {
    let mut selector = PythonString::from_scalars(section.kind.canonical());
    selector.push_scalars(":");
    selector.append(artifact_id);
    fetch_stanza_lines_exact(
        &selector,
        &PythonString::from_scalars(section.when_clause),
        &PythonString::from_scalars(SELECTION_INDENT),
        scope.as_ref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> PythonString {
        PythonString::from_scalars(value)
    }

    fn map(entries: &[(&str, &str)]) -> BTreeMap<PythonString, PythonString> {
        entries
            .iter()
            .map(|(key, value)| (text(key), text(value)))
            .collect()
    }

    #[test]
    fn provenance_suffix_matches_the_reference_shapes() {
        assert_eq!(
            provenance_suffix(&text("x"), None)
                .to_scalar_string()
                .unwrap(),
            ""
        );
        assert_eq!(
            provenance_suffix(&text("x"), Some(&BTreeMap::new()))
                .to_scalar_string()
                .unwrap(),
            ""
        );
        assert_eq!(
            provenance_suffix(&text("x"), Some(&map(&[("x", "")])))
                .to_scalar_string()
                .unwrap(),
            " (source: org)"
        );
        assert_eq!(
            provenance_suffix(&text("x"), Some(&map(&[("x", "example-org")])))
                .to_scalar_string()
                .unwrap(),
            " (source: org, pack: example-org)"
        );
        assert_eq!(
            provenance_suffix(&text("x"), Some(&map(&[("x", "   ")])))
                .to_scalar_string()
                .unwrap(),
            " (source: org)"
        );
        assert_eq!(
            provenance_suffix(&text("y"), Some(&map(&[("x", "")])))
                .to_scalar_string()
                .unwrap(),
            ""
        );
    }

    #[test]
    fn unindexed_selection_ids_keep_unknown_tokens_and_normalise_directives() {
        assert_eq!(
            selection_id(None, ArtifactKind::Directive, &text("3-locality"))
                .to_scalar_string()
                .unwrap(),
            "DIRECTIVE_003"
        );
        assert_eq!(
            selection_id(None, ArtifactKind::Tactic, &text("  keep  "))
                .to_scalar_string()
                .unwrap(),
            "  keep  "
        );
        let surrogate = PythonString::from_codepoints(&[0x64, 0xd800]).expect("exact");
        assert_eq!(
            selection_id(None, ArtifactKind::Tactic, &surrogate).codepoints(),
            surrogate.codepoints()
        );
        assert_eq!(
            selection_id(None, ArtifactKind::Directive, &surrogate).codepoints(),
            &[0x44, 0xd800],
            "a non-scalar directive token is uppercased and keeps every code point"
        );
        assert_eq!(
            selection_id(None, ArtifactKind::Directive, &text("c-built-in"))
                .to_scalar_string()
                .unwrap(),
            "C_BUILT_IN"
        );
    }
}
