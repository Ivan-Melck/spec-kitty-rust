//! Action-scoped charter context resolution.
//!
//! Mirrors the resolution and JSON half of `src/charter/activation/context.py`,
//! `context_json.py`, `progressive_disclosure.py`, `context_renderers/
//! artifact_bodies.py` and the contract ledger in `context_contract.py`.
//!
//! The payload is the versioned `charter context --json` envelope:
//!
//! * five typed arrays (`directives`, `tactics`, `styleguides`, `toolguides`,
//!   `procedures`) whose entries carry provenance, an outbound `references[]`
//!   list, a `delivery` cadence (`inline` for the `requires` closure, `link` for
//!   `suggests`-reached artifacts, everything inline under `--include-all`) and
//!   the artifact body when it is inlined;
//! * the top-level `references[]` link set that names every delivered artifact
//!   (`{id, relation, when, reason}`), so the union of inlined and named ids is
//!   the delivered set with no cap;
//! * `all_directives` and `directives_source` from one project-governance
//!   resolution (`catalog_fallback` / `activation`, plus `+charter` and
//!   `+project_local` when those layers contribute);
//! * the real `project_charter` block (paths, bytes, byte-count metadata) and
//!   the imported `org_charter` policy block;
//! * `governance_references` status entries.
//!
//! A non-bootstrap action delivers no action grain, but every payload still
//! carries the governance blocks and diagnostics.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Serialize;

use sk_charter::ArtifactKind;

use crate::activation::{ActivationState, filter_graph_by_activation};
use crate::error::DoctrineError;
use crate::graph::{DrgEdge, DrgGraph, NodeKind, Relation, is_valid_urn, split_urn};
use crate::identity::{IdentityIndex, IdentityLayer};
use crate::ledger::ActivationLedger;
use crate::org_charter::{self, load_org_charter_policy};
use crate::profile::ProfileRepository;
use crate::query::deliver_action_context;

/// The top-level `charter context --json` contract version.
pub const CONTEXT_SCHEMA_VERSION: &str = "1.4.0";

/// The complete set of top-level keys the payload may carry.
pub const CONTEXT_CONTRACT_TOP_LEVEL_KEYS: [&str; 15] = [
    "context_schema_version",
    "action",
    "mode",
    "directives",
    "tactics",
    "styleguides",
    "toolguides",
    "procedures",
    "all_directives",
    "directives_source",
    "diagnostics",
    "references",
    "project_charter",
    "org_charter",
    "governance_references",
];

/// Actions that always use bootstrap delivery.
pub const BOOTSTRAP_ACTIONS: [&str; 4] = ["specify", "plan", "implement", "review"];

/// The `when` text rendered for a `suggests` edge that authored none.
pub const STATED_DEFAULT_WHEN: &str = "Fetch when this artefact's guidance applies to the work at hand (no specific trigger authored yet).";

/// The delivery cadence of one artifact.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Delivery {
    /// Delivered inline (eager `requires` closure, or `--include-all`).
    Inline,
    /// Named as a link with its `when` clause.
    Link,
}

/// One outbound reference entry (`{id, relation, when, reason}`).
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct EdgeReference {
    /// The bare target id.
    pub id: String,
    /// The relation label.
    pub relation: String,
    /// The edge's `when` clause (a stated default for an uncovered `suggests`;
    /// `null` otherwise, matching the reference reference-DTO).
    pub when: Option<String>,
    /// The edge's authored rationale (`null` when none, matching the contract).
    pub reason: Option<String>,
}

impl EdgeReference {
    fn project(edge: &DrgEdge) -> Self {
        // The reference replaces a missing OR empty `when` on a `suggests` edge
        // with the stated default (`if not when and relation is SUGGESTS`).
        let when = edge
            .when()
            .filter(|when| !when.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                (edge.relation() == Relation::Suggests).then(|| STATED_DEFAULT_WHEN.to_owned())
            });
        Self {
            id: bare_id(edge.target()),
            relation: edge.relation().as_str().to_owned(),
            when,
            reason: edge.reason().map(str::to_owned),
        }
    }
}

/// Orthogonal to [`Delivery`]; part of the compact steady-state envelope.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ContextMode {
    /// Action-scoped bootstrap governance is delivered.
    Bootstrap,
    /// The small steady-state payload (no action grain).
    Compact,
}

/// A closed-set context diagnostic code.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DiagnosticCode {
    /// A selection token named no artifact.
    UnknownSelectionDropped,
    /// The catalog default was applied.
    CatalogDefaultApplied,
    /// A fallback branch supplied the set.
    FallbackApplied,
    /// Selections were merged.
    SelectionMerged,
    /// A reference could not be resolved.
    UnresolvedReference,
}

/// One context diagnostic.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Diagnostic {
    /// The closed-set code.
    pub code: DiagnosticCode,
    /// Human text (may be reworded; match on the code).
    pub message: String,
}

/// One delivered typed artifact, in the reference entry shape.
///
/// Serializes to `{id, source, title?, summary?, references, delivery,
/// body?}` — exactly the contract's per-entry keys. The URN, kind and `when`
/// are API data and never appear in the JSON.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ArtifactEntry {
    /// The bare artifact id.
    pub id: String,
    /// The full DRG URN (API data).
    #[serde(skip)]
    pub urn: String,
    /// The artifact kind (API data).
    #[serde(skip)]
    pub kind: NodeKind,
    /// Artifact origin (`built-in`, `org`, `project`).
    pub source: String,
    /// The display label, when one is declared.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The one-line intent/purpose, when declared.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Every outbound DRG edge of this artifact.
    pub references: Vec<EdgeReference>,
    /// The delivery cadence.
    pub delivery: Delivery,
    /// The artifact body, for an inline delivery.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<serde_json::Value>,
    /// A `suggests` link's `when` clause (API data, rendered through
    /// `references[]` in the JSON contract).
    #[serde(skip)]
    pub when: Option<String>,
}

/// One `all_directives` entry: the plain provenance DTO (no disclosure fields).
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct DirectiveEntry {
    /// The bare directive id.
    pub id: String,
    /// Artifact origin (`built-in`, `org`, `project`).
    pub source: String,
    /// The display label, when one is declared.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The directive description, when declared.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// A profile-cited artifact entry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProfileLink {
    /// The profile that cites it.
    pub profile: String,
    /// The cited artifact URN.
    pub urn: String,
    /// The relation by which it is cited.
    pub relation: Relation,
    /// The edge's `when` clause, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
}

/// The project-charter presence/metadata block.
///
/// `present`/`path` describe the authoritative `charter.yaml`; the `charter.md`
/// fields are display companions. When `charter.yaml` exists, `bytes` is always
/// present and the metadata file contributes `hash`, `source_path`,
/// `bundle_schema_version` and `schema_version` when it declares them.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct ProjectCharterBlock {
    /// Whether `charter.yaml` exists.
    pub present: bool,
    /// The repository-relative `charter.yaml` path.
    pub path: String,
    /// Whether `charter.md` is present.
    pub charter_md_present: bool,
    /// The repository-relative `charter.md` path.
    pub charter_md_path: String,
    /// Byte length of `charter.yaml`, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    /// The recorded charter digest, when `metadata.yaml` declares one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    /// The recorded source path, when declared.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    /// The recorded bundle schema version, when declared.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_schema_version: Option<i64>,
    /// The recorded schema version, when declared.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_version: Option<String>,
}

/// One governance policy in the org-charter block.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OrgGovernancePolicy {
    /// The policy field.
    pub field: String,
    /// The policy value.
    pub value: String,
    /// The enforcement level.
    pub enforcement: String,
    /// Always `org` (the block only describes org-tier policy).
    pub source: String,
}

/// One org pack's summary in the org-charter block.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OrgCharterPack {
    /// The pack name (its org name when declared).
    pub pack_name: String,
    /// The pack's governance policies.
    pub governance_policies: Vec<OrgGovernancePolicy>,
    /// The pack's `required_directives`, verbatim.
    pub required_directives: Vec<String>,
}

/// The org-charter block.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct OrgCharterBlock {
    /// Whether any org charter policy is present.
    pub present: bool,
    /// One entry per pack that ships an `org-charter.yaml`.
    pub packs: Vec<OrgCharterPack>,
}

/// One `governance_references` status entry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GovernanceReferenceStatus {
    /// The declared repository-relative path.
    pub path: String,
    /// Whether it exists.
    pub exists: bool,
    /// Whether the path stays inside the repository root.
    pub safe: bool,
    /// A diagnostic when it is unsafe, missing or not a file (`null` otherwise,
    /// matching the reference status DTO).
    pub warning: Option<String>,
}

/// The action-scoped governance payload.
#[derive(Clone, Debug, Serialize)]
pub struct CharterContext {
    /// The top-level contract version.
    pub context_schema_version: String,
    /// The normalized action name.
    pub action: String,
    /// The delivery mode.
    pub mode: ContextMode,
    /// Eager/link directives.
    pub directives: Vec<ArtifactEntry>,
    /// Eager/link tactics.
    pub tactics: Vec<ArtifactEntry>,
    /// Eager/link styleguides.
    pub styleguides: Vec<ArtifactEntry>,
    /// Eager/link toolguides.
    pub toolguides: Vec<ArtifactEntry>,
    /// Eager/link procedures (first-class since schema 1.1.0).
    pub procedures: Vec<ArtifactEntry>,
    /// Every directive exposed by project governance.
    pub all_directives: Vec<DirectiveEntry>,
    /// Which branch resolved the directive set.
    pub directives_source: Option<String>,
    /// Resolution diagnostics, always present.
    pub diagnostics: Vec<Diagnostic>,
    /// The complete link set naming every delivered artifact.
    pub references: Vec<EdgeReference>,
    /// The project-charter block.
    pub project_charter: ProjectCharterBlock,
    /// The org-charter block.
    pub org_charter: OrgCharterBlock,
    /// Governance-reference status entries.
    pub governance_references: Vec<GovernanceReferenceStatus>,
    /// Profile-channel links (`requires`/`specializes_from`/`suggests`).
    ///
    /// API data, not part of the versioned top-level JSON contract.
    #[serde(skip)]
    pub profile_links: Vec<ProfileLink>,
    /// Co-delivered tension pairs and their reconcilers (API data).
    #[serde(skip)]
    pub tension_arbiters: Vec<(String, Vec<String>)>,
    /// Co-delivered tension pairs with no reconciler (API data).
    #[serde(skip)]
    pub unarbitrated_tensions: Vec<(String, String)>,
}

impl CharterContext {
    /// The contract keys this payload actually carries.
    pub fn top_level_keys(&self) -> BTreeSet<&'static str> {
        let value = self.to_json();
        value
            .as_object()
            .map(|object| {
                object
                    .keys()
                    .filter_map(|key| {
                        CONTEXT_CONTRACT_TOP_LEVEL_KEYS
                            .iter()
                            .copied()
                            .find(|known| known == key)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Serialize the payload to its JSON contract value.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }
}

/// A context request.
#[derive(Clone, Debug)]
pub struct ContextRequest<'a> {
    /// The action name (`implement`) or action URN (`action:software-dev/implement`).
    pub action: &'a str,
    /// The mission type, used to build a bare action's URN.
    pub mission_type: Option<&'a str>,
    /// Maximum `suggests` hops.
    pub depth: usize,
    /// An optional profile id whose channel is delivered.
    pub profile: Option<&'a str>,
    /// Inline the whole closure (`--include-all`).
    pub include_all: bool,
}

impl<'a> ContextRequest<'a> {
    /// A request for `action` with the compact steady-state depth.
    pub fn new(action: &'a str) -> Self {
        Self {
            action,
            mission_type: None,
            depth: 1,
            profile: None,
            include_all: false,
        }
    }
}

/// The resolved inputs to a context build.
pub struct ContextInputs<'a> {
    /// The merged, unfiltered DRG.
    pub graph: &'a DrgGraph,
    /// The activation ledger.
    pub ledger: &'a ActivationLedger,
    /// The identity index (already scanned).
    pub identity: &'a IdentityIndex,
    /// The profile repository.
    pub profiles: &'a ProfileRepository,
    /// The project root.
    pub project_root: &'a Path,
    /// Required-floor ids per kind (what an org pack mandates).
    pub required_floor: &'a BTreeMap<ArtifactKind, Vec<String>>,
    /// Non-fatal project-layer warnings to surface as diagnostics.
    pub project_warnings: &'a [String],
}

/// Build the action-scoped governance payload.
pub fn build_action_context(
    inputs: &ContextInputs<'_>,
    request: &ContextRequest<'_>,
) -> Result<CharterContext, DoctrineError> {
    let raw_action = request.action.trim();
    if raw_action.is_empty() {
        return Err(DoctrineError::InvalidAction {
            action: request.action.to_owned(),
        });
    }
    let normalized_action = normalize_action_name(raw_action);
    let bootstrap_action = BOOTSTRAP_ACTIONS.contains(&normalized_action.as_str());
    // A bootstrap action is delivered even when the project declares no such
    // action node (or declares several with different mission types): the
    // reference payload then simply carries the governance blocks with an
    // empty action grain. Every other action must resolve to one node.
    let action_urn = match resolve_action_urn(inputs.graph, raw_action, request.mission_type) {
        Ok(urn) => Some(urn),
        Err(error) if bootstrap_action => {
            let _ = error;
            None
        }
        Err(error) => return Err(error),
    };

    let state =
        ActivationState::from_ledger(inputs.ledger, inputs.identity, inputs.required_floor)?;
    let mut diagnostics = Vec::new();
    // `state.unknown_allow_tokens` stays API data (the activation surfaces
    // report it), but it is NOT a payload diagnostic: the reference logs an
    // unknown activation token at WARNING level in `_seed_sets` /
    // `_resolve_directive_base` and never emits `UNKNOWN_SELECTION_DROPPED` for
    // it. Only a charter `selected_*` token that no layer knows produces that
    // diagnostic (one entry, from `_drop_unknown`).
    for warning in inputs.project_warnings {
        diagnostics.push(Diagnostic {
            code: DiagnosticCode::UnresolvedReference,
            message: warning.clone(),
        });
    }

    let filtered = filter_graph_by_activation(inputs.graph, &state);
    let action_declared = action_urn
        .as_deref()
        .is_some_and(|urn| inputs.graph.node(urn).is_some());
    let mode = if bootstrap_action || action_declared {
        ContextMode::Bootstrap
    } else {
        ContextMode::Compact
    };

    let mut delivery = action_urn
        .as_deref()
        .map(|urn| deliver_action_context(&filtered, urn, request.depth))
        .unwrap_or_default();
    // One canonical-root probe drives every charter-config read in this build:
    // the reference anchors the directive/local/charter reads on the same
    // `resolve_canonical_repo_root` facade.
    let charter_config = charter_config_read(inputs.project_root);
    // The authored `governance:` / `directives:` sections as the reference's
    // `load_governance_config` / `load_directives_config` see them. A retired
    // `governance.doctrine` key is a typed HARD refusal (raised here, exactly
    // as the reference's governance-reference read re-raises it); a malformed
    // shape is a best-effort degrade, never invented provenance.
    let charter_sections = charter_sections(inputs.project_root, charter_config.is_available())?;
    // Selected project doctrine widens the action's delivered set: the
    // reference walks the `requires`/`suggests` closure of the activated and
    // org-required directives/tactics/paradigms, minus any artifact another
    // action scopes. Without this an activated styleguide reached only from a
    // paradigm silently disappears from the typed arrays.
    let governance = resolve_directives(
        inputs.graph,
        inputs.identity,
        inputs.ledger,
        &state,
        &charter_sections,
    )?;
    // The reference resolves the project/org seed sets only inside
    // `_load_action_governance_bundle`'s `resolved_type is not None` branch. A
    // typeless or unresolvable action therefore contributes NO doctrine roots:
    // a bootstrap action still carries its governance blocks and an empty
    // action grain. Seeding here without a resolved mission type would deliver
    // the whole catalogue for `charter context --action review` on a typeless
    // project, which the reference never does.
    let seeds = if action_urn.is_some() {
        governance_seeds(
            inputs.project_root,
            inputs.graph,
            inputs.identity,
            inputs.ledger,
            &state,
            &charter_sections,
            charter_config.is_available(),
        )
    } else {
        GovernanceSeeds::default()
    };
    let mut roots: BTreeSet<String> = action_urn.iter().cloned().collect();
    roots.extend(seeds.roots.iter().cloned());

    let mut profile_links = Vec::new();
    if let Some(profile_id) = request.profile {
        if !state.admits_profile(profile_id) {
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::UnresolvedReference,
                message: format!(
                    "profile {profile_id:?} is deactivated for this project; its cited doctrine was not delivered"
                ),
            });
        } else {
            match inputs.profiles.resolve(profile_id, inputs.graph) {
                Ok(profile) => {
                    let urn = format!("agent_profile:{}", profile.id);
                    if inputs.graph.node(&urn).is_some() {
                        delivery.eager.insert(urn.clone());
                        delivery.all_artifacts.insert(urn.clone());
                    }
                    for link in inputs
                        .profiles
                        .channel_deliveries(profile_id, inputs.graph)?
                    {
                        if link.relation == Relation::Suggests {
                            delivery.suggested.insert(link.urn.clone());
                            delivery.all_artifacts.insert(link.urn.clone());
                        } else {
                            delivery.eager.insert(link.urn.clone());
                            delivery.all_artifacts.insert(link.urn.clone());
                        }
                        profile_links.push(ProfileLink {
                            profile: link.profile,
                            urn: link.urn,
                            relation: link.relation,
                            when: link.when,
                        });
                    }
                }
                Err(error) => diagnostics.push(Diagnostic {
                    code: DiagnosticCode::UnresolvedReference,
                    message: format!("profile {profile_id:?} could not be resolved: {error}"),
                }),
            }
        }
    }

    let mut directives = Vec::new();
    let mut tactics = Vec::new();
    let mut styleguides = Vec::new();
    let mut toolguides = Vec::new();
    let mut procedures = Vec::new();
    let mut delivered: BTreeSet<String> = BTreeSet::new();
    let mut references = Vec::new();

    if mode == ContextMode::Bootstrap {
        let inline_urns = requires_closure(&filtered, &roots);
        // The reference `_classify_artifact_urns` delivers the action resolution
        // URNs UNION the selected roots' `requires`/`suggests` closure. The
        // closure includes the roots themselves, so an activated/org-required
        // artifact the action bundle does not otherwise reach (and every
        // catalogue-default root) is delivered from its own root, not only when
        // some other artifact happens to point at it.
        let mut candidates: BTreeSet<String> = delivery.all_artifacts.clone();
        candidates.extend(selected_closure(
            &filtered,
            &seeds.roots,
            action_urn.as_deref(),
        ));
        for urn in &candidates {
            let Some(node) = filtered.node(urn) else {
                continue;
            };
            // Delivered slots are the five typed kinds plus the two
            // reference-only kinds the reference's delivery table flips in:
            // `asset` (ungated) and `glossary_pack` (activation-gated). Every
            // other kind -- notably `paradigm` -- is resolved but never
            // delivered, so it is never a link *target* either.
            let kind = node.kind();
            let deliverable = match kind {
                NodeKind::Asset => true,
                NodeKind::GlossaryPack => state.admits(kind.as_str(), &bare_id(urn)),
                _ => requested_kind(kind),
            };
            if !deliverable {
                continue;
            }
            // The reference's directive slot guard: only the selected /
            // project-local directive set is delivered, even when the action
            // graph reaches another directive.
            if kind == NodeKind::Directive && !seeds.directives.contains(&bare_id(urn)) {
                continue;
            }
            delivered.insert(urn.clone());
        }

        // Typed delivery: each delivered artifact of a typed kind is an entry;
        // the `requires` closure of the action + selected roots is inline and
        // everything else delivered is a link (all inline under --include-all).
        let mut typed: BTreeMap<NodeKind, Vec<ArtifactEntry>> = BTreeMap::new();
        for urn in &delivered {
            let Some(entry) = artifact_entry(
                &filtered,
                inputs.identity,
                urn,
                &inline_urns,
                request.include_all,
            ) else {
                continue;
            };
            typed.entry(entry.kind).or_default().push(entry);
        }
        for (kind, mut entries) in typed {
            sort_entries(&mut entries);
            match kind {
                NodeKind::Directive => directives = entries,
                NodeKind::Tactic => tactics = entries,
                NodeKind::Styleguide => styleguides = entries,
                NodeKind::Toolguide => toolguides = entries,
                NodeKind::Procedure => procedures = entries,
                _ => {}
            }
        }
        // The complete link set over `roots ∪ delivered ∪ bridges`, where the
        // bridges are every URN the action resolution visited (including kinds
        // this payload never delivers) so a pass-through hop still names the
        // artifact on its far side.
        references = link_references(&filtered, &roots, &delivered, &delivery.all_artifacts);
    }

    diagnostics.extend(seeds.diagnostics.clone());
    diagnostics.extend(governance.diagnostics.clone());

    let project_charter = project_charter_block(inputs.project_root);
    let org_charter = org_charter_block(inputs.project_root);
    let governance_references =
        governance_reference_status(inputs.project_root, charter_sections.governance_valid);

    profile_links.sort_by(|left, right| {
        (&left.profile, &left.urn, left.relation, &left.when).cmp(&(
            &right.profile,
            &right.urn,
            right.relation,
            &right.when,
        ))
    });
    profile_links.dedup();

    if mode == ContextMode::Compact {
        directives.clear();
        tactics.clear();
        styleguides.clear();
        toolguides.clear();
        procedures.clear();
        references.clear();
    }

    Ok(CharterContext {
        context_schema_version: CONTEXT_SCHEMA_VERSION.to_owned(),
        action: normalized_action,
        mode,
        directives,
        tactics,
        styleguides,
        toolguides,
        procedures,
        all_directives: governance.entries,
        directives_source: governance.source,
        diagnostics,
        references,
        project_charter,
        org_charter,
        governance_references,
        profile_links,
        tension_arbiters: delivery.tension_arbiters,
        unarbitrated_tensions: delivery.unarbitrated_tensions,
    })
}

fn normalize_action_name(action: &str) -> String {
    let body = split_urn(action).map(|(_, body)| body).unwrap_or(action);
    body.split_once('/')
        .map(|(_, action)| action)
        .unwrap_or(body)
        .to_owned()
}

fn bare_id(urn: &str) -> String {
    urn.split_once(':')
        .map(|(_, body)| body)
        .unwrap_or(urn)
        .to_owned()
}

fn requested_kind(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Directive
            | NodeKind::Tactic
            | NodeKind::Styleguide
            | NodeKind::Toolguide
            | NodeKind::Procedure
    )
}

fn sort_entries(entries: &mut Vec<ArtifactEntry>) {
    entries.sort_by(|left, right| {
        (left.kind, &left.id, &left.urn).cmp(&(right.kind, &right.id, &right.urn))
    });
    entries.dedup();
}

fn source_label(node: &crate::graph::DrgNode) -> String {
    match node.provenance() {
        Some("project") => "project".to_owned(),
        Some(provenance) if provenance.starts_with("org:") => "org".to_owned(),
        _ => "built-in".to_owned(),
    }
}

fn artifact_entry(
    graph: &DrgGraph,
    identity: &IdentityIndex,
    urn: &str,
    inline: &BTreeSet<String>,
    include_all: bool,
) -> Option<ArtifactEntry> {
    let node = graph.node(urn)?;
    let kind = node.kind();
    if !requested_kind(kind) {
        return None;
    }
    let artifact_kind = kind.artifact_kind()?;
    let body = artifact_document(identity, artifact_kind, &bare_id(urn));
    // The reference `artifact_to_dict` reads the artifact MODEL's `title`
    // (then `name`) and never the DRG node label. The label is graph metadata
    // that may differ from the authored body title (a real org-pack repro),
    // and an unresolvable artifact has no title at all rather than a
    // label-derived one.
    let title = text_field(&body, "title").or_else(|| text_field(&body, "name"));
    let summary = text_field(&body, "intent").or_else(|| text_field(&body, "purpose"));
    let references = graph
        .edges_from(urn, None)
        .into_iter()
        .map(EdgeReference::project)
        .collect();
    let is_inline = include_all || inline.contains(urn);
    Some(ArtifactEntry {
        id: bare_id(urn),
        urn: urn.to_owned(),
        kind,
        source: source_label(node),
        title,
        summary,
        references,
        delivery: if is_inline {
            Delivery::Inline
        } else {
            Delivery::Link
        },
        body: if is_inline { body } else { None },
        when: None,
    })
}

/// Read a typed artifact's body as the reference model dump.
///
/// This is the canonical nested body, not the raw YAML: model defaults the
/// author omitted (empty lists/mappings, a defaulted nested field) are present
/// and `null` optionals are absent, exactly as the reference
/// `model_dump(by_alias=True, mode="json", exclude_none=True)` renders it.
fn artifact_document(
    identity: &IdentityIndex,
    kind: ArtifactKind,
    artifact_id: &str,
) -> Option<serde_json::Value> {
    let path = identity.path_for(kind, artifact_id)?;
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(&text).ok()?;
    crate::body::canonical_artifact_body(kind, &value)
}

fn text_field(body: &Option<serde_json::Value>, field: &str) -> Option<String> {
    body.as_ref()?
        .get(field)
        .and_then(serde_json::Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .map(str::to_owned)
}

/// The complete link set naming every delivered artifact.
///
/// Mirrors the reference `progressive_disclosure.link_references`: one entry per
/// `(target id, relation)` for every edge whose source is a traversal root, a
/// delivered artifact, or an action-resolution *bridge* URN, and whose target is
/// delivered. Sources are walked in sorted order and edges in graph order, which
/// is the reference's deterministic order; deduplication is by `(id, relation)`.
fn link_references(
    graph: &DrgGraph,
    roots: &BTreeSet<String>,
    delivered: &BTreeSet<String>,
    bridge_urns: &BTreeSet<String>,
) -> Vec<EdgeReference> {
    let mut sources: BTreeSet<&str> = BTreeSet::new();
    sources.extend(roots.iter().map(String::as_str));
    sources.extend(delivered.iter().map(String::as_str));
    sources.extend(bridge_urns.iter().map(String::as_str));
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    let mut references = Vec::new();
    for source in sources {
        for edge in graph.edges_from(source, None) {
            if !delivered.contains(edge.target()) {
                continue;
            }
            let key = (bare_id(edge.target()), edge.relation().as_str().to_owned());
            if !seen.insert(key) {
                continue;
            }
            references.push(EdgeReference::project(edge));
        }
    }
    references
}

/// The traversal seeds selected project doctrine contributes.
#[derive(Default)]
struct GovernanceSeeds {
    /// Directive/tactic/paradigm URNs the closure starts from.
    roots: BTreeSet<String>,
    /// The delivered directive id set (the activation selection, or the
    /// catalogue default, plus project-local directives). The reference's
    /// directive slot guard admits no other directive.
    directives: BTreeSet<String>,
    /// Resolution diagnostics produced while seeding.
    diagnostics: Vec<Diagnostic>,
}

/// Resolve the activated + org-required directive/tactic/paradigm seeds.
///
/// Mirrors the reference `_seed_sets`: an absent activation key falls back to
/// `_graph_and_catalog_default_ids` — every node of that kind the merged graph
/// declares (built-in, org AND project) unioned with the built-in catalogue —
/// an explicitly empty list stays empty, org `required_<kind>` unions on top,
/// and the effective deny subtracts last (it never holds a required id). An
/// absent key is the default project state, so excluding org/project graph
/// nodes there silently under-delivered org-pack governance. Project-local
/// directives declared in `charter.yaml` join the directive seeds only when the
/// project has an explicit directive allow list and a loadable `directives:`
/// section, exactly as the reference guards it.
fn governance_seeds(
    project_root: &Path,
    graph: &DrgGraph,
    identity: &IdentityIndex,
    ledger: &ActivationLedger,
    state: &ActivationState,
    charter_sections: &CharterConfigSections,
    resolution_available: bool,
) -> GovernanceSeeds {
    let mut roots = BTreeSet::new();
    let mut directive_ids: BTreeSet<String> = BTreeSet::new();
    let mut diagnostics = Vec::new();
    for (kind, node_kind) in [
        (ArtifactKind::Directive, NodeKind::Directive),
        (ArtifactKind::Tactic, NodeKind::Tactic),
        (ArtifactKind::Paradigm, NodeKind::Paradigm),
    ] {
        // `_graph_and_catalog_default_ids`: the catalogue default is every id
        // the ACTIVE graph declares for this kind, unioned with the built-in
        // catalogue. In a merged production graph the built-in layer already
        // supplies the catalogue, so the graph ids are the union; a node's
        // provenance never excludes it.
        let graph_ids: Vec<String> = graph
            .nodes()
            .iter()
            .filter(|node| node.kind() == node_kind)
            .map(|node| bare_id(node.urn()))
            .collect();
        // The built-in catalogue alone is what the resolver's diagnostic
        // reports ("using built-in catalog default (N ...)"), exactly as the
        // reference's `_resolve_paradigm_base` counts `offering_catalog`.
        let catalog_len = graph
            .nodes()
            .iter()
            .filter(|node| node.kind() == node_kind)
            .filter(|node| {
                // A raw built-in graph carries no provenance; a merged one tags
                // it "built-in".
                !node.provenance().is_some_and(|provenance| {
                    provenance == "project" || provenance.starts_with("org:")
                })
            })
            .count();
        let mut warnings = Vec::new();
        let mut ids: Vec<String> = match ledger.allow_for(kind) {
            None => {
                // The paradigm catalogue-default diagnostic is emitted by the
                // REFERENCE RESOLVER (`_resolve_paradigm_base`), not by the
                // action-bundle seed walk. When the canonical-root gate makes
                // that resolution fail, the reference reports no diagnostic at
                // all, so the seed path must not invent this one either. The
                // catalogue roots themselves are unchanged (the seed walk is a
                // separate seam that does not need Git).
                if kind == ArtifactKind::Paradigm
                    && resolution_available
                    && charter_sections.governance_valid
                    && charter_sections.directives_valid
                {
                    diagnostics.push(Diagnostic {
                        code: DiagnosticCode::CatalogDefaultApplied,
                        message: format!(
                            "No activated paradigm set configured; using built-in catalog default ({catalog_len} paradigms)."
                        ),
                    });
                }
                graph_ids
            }
            Some(tokens) => tokens
                .iter()
                .map(|token| identity.activation_id(kind, token, &mut warnings))
                .collect(),
        };
        for token in
            org_charter::read_org_required_tokens(project_root, kind, false).unwrap_or_default()
        {
            ids.push(identity.activation_id(kind, &token, &mut warnings));
        }
        // Unknown activation tokens are logged by the reference, never added
        // to the payload diagnostics (`_drop_unknown` is for charter
        // selections only). `warnings` is intentionally not surfaced here.
        let _ = warnings;
        let denied = state.deny_for(kind);
        for id in ids {
            let urn = format!("{}:{id}", kind.canonical());
            let denied = denied.is_some_and(|deny| !deny.admits(&id) || !deny.admits(&urn));
            if !denied {
                if kind == ArtifactKind::Directive {
                    directive_ids.insert(id.clone());
                }
                roots.insert(urn);
            }
        }
    }
    if ledger.allow_for(ArtifactKind::Directive).is_some() && charter_sections.directives_valid {
        for local in charter_sections.local_directives() {
            directive_ids.insert(local.id.clone());
            roots.insert(format!("directive:{}", local.id));
        }
    }
    GovernanceSeeds {
        roots,
        directives: directive_ids,
        diagnostics,
    }
}

/// The `requires` closure of `roots`, including the roots themselves.
fn requires_closure(graph: &DrgGraph, roots: &BTreeSet<String>) -> BTreeSet<String> {
    crate::query::walk_edges(graph, roots, &BTreeSet::from([Relation::Requires]), None)
}

/// The `requires`/`suggests` closure of the selected roots, minus artifacts a
/// *different* action scopes.
///
/// Mirrors the reference `_classify_artifact_urns` closure contribution: the
/// start URNs themselves are part of the closure (`walk_edges` visits its
/// roots), so a selected root is delivered even when no other artifact points
/// at it. Only the four closure kinds widen; a procedure/asset reached from a
/// selected root is not delivered by this seam. The ownership gate applies to
/// the roots too, exactly as the reference applies it to every closure member.
fn selected_closure(
    graph: &DrgGraph,
    roots: &BTreeSet<String>,
    action_urn: Option<&str>,
) -> BTreeSet<String> {
    if roots.is_empty() {
        return BTreeSet::new();
    }
    let forked: BTreeSet<String> = crate::query::walk_edges(
        graph,
        roots,
        &BTreeSet::from([Relation::Requires, Relation::Suggests]),
        None,
    );
    let mut owners: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for edge in graph.edges() {
        if edge.relation() == Relation::Scope && edge.source().starts_with("action:") {
            owners
                .entry(edge.target())
                .or_default()
                .insert(edge.source());
        }
    }
    forked
        .into_iter()
        .filter(|urn| {
            let kind = urn
                .split_once(':')
                .and_then(|(prefix, _)| NodeKind::from_prefix(prefix));
            matches!(
                kind,
                Some(
                    NodeKind::Directive
                        | NodeKind::Tactic
                        | NodeKind::Styleguide
                        | NodeKind::Toolguide
                )
            )
        })
        .filter(|urn| match owners.get(urn.as_str()) {
            None => true,
            Some(sources) => action_urn.is_some_and(|action| sources.contains(action)),
        })
        .collect()
}

/// The result of one project-governance directive resolution.
struct DirectiveResolution {
    entries: Vec<DirectiveEntry>,
    source: Option<String>,
    diagnostics: Vec<Diagnostic>,
}

/// Whether the reference's charter-config reads can run for a project root.
///
/// The reference (`charter.activation.sync::_charter_read_root`) anchors
/// `load_directives_config`, `load_governance_config` and
/// `resolve_project_governance` on
/// `charter.resolution.resolve_canonical_repo_root`, whose primitive is
/// `kernel.git_topology.git_common_dir` — the same canonical kernel seam this
/// crate already uses for lock placement, not a second Git resolver. Outside a
/// Git repository that facade raises `NotInsideRepositoryError`.
///
/// `context_json._load_project_directives_with_diagnostics` catches resolver
/// failures broadly, and `_load_governance_charter_config` is best-effort with
/// an equally broad catch. An ordinary non-repository and a genuine topology
/// failure therefore take the SAME documented degrade path in the reference;
/// the variants stay distinct here only so a later caller cannot mistake a
/// discovery failure for "this project simply is not a Git checkout".
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CharterConfigRead {
    /// The canonical root resolved; the charter-config reads run normally.
    Available,
    /// The root is not inside a Git repository (the ordinary non-Git input).
    NotARepository,
    /// The canonical root could not be discovered for another reason.
    Unavailable,
}

impl CharterConfigRead {
    fn is_available(self) -> bool {
        self == Self::Available
    }
}

fn charter_config_read(project_root: &Path) -> CharterConfigRead {
    match sk_kernel::git_topology::git_common_dir(project_root) {
        Ok(_) => CharterConfigRead::Available,
        Err(sk_kernel::KernelError::NotAGitRepository { .. }) => CharterConfigRead::NotARepository,
        Err(_) => CharterConfigRead::Unavailable,
    }
}

/// The authored charter sections the reference's charter-config readers see.
///
/// The reference runs two independent, broadly-caught reads before it builds
/// `all_directives` / `directives_source`:
///
/// * `sync.load_directives_config` over the `directives:` section
///   (`DirectivesConfig`, a Pydantic model), and
/// * `resolver.resolve_project_governance`, whose first step is
///   `sync.load_governance_config` over the `governance:` section
///   (`GovernanceConfig`) after `require_canonical_governance` refuses the
///   retired `doctrine` selection key.
///
/// A malformed shape in either is caught by
/// `context_json._load_project_directives_with_diagnostics`, which returns the
/// directly-loaded local ids, a `null` source and no diagnostics. A retired
/// key is NOT caught there: the separate governance-reference read re-raises
/// it and the whole payload build fails. Both behaviours are modelled here so
/// the source is either resolved or explicitly refused, never invented.
struct CharterConfigSections {
    /// The parsed `charter.yaml` document, when the canonical root allowed a
    /// read. `None` on a non-Git root (both reference reads raise before they
    /// can return a local directive).
    charter: Option<serde_yaml_ng::Value>,
    /// Whether `governance:` passes the reference `GovernanceConfig` shape.
    governance_valid: bool,
    /// Whether `directives:` passes the reference `DirectivesConfig` shape.
    directives_valid: bool,
}

impl CharterConfigSections {
    /// The directly-loaded local directives, when that read succeeded.
    fn local_directives(&self) -> Vec<LocalDirective> {
        if !self.directives_valid {
            return Vec::new();
        }
        local_directives(&self.charter)
    }
}

/// Read the authored charter sections for a project root.
///
/// The retired-key refusal is raised here, from the same charter read the
/// reference's governance-reference seam performs, so a payload can never
/// succeed while honouring or silently ignoring `governance.doctrine`.
fn charter_sections(
    project_root: &Path,
    resolution_available: bool,
) -> Result<CharterConfigSections, DoctrineError> {
    if !resolution_available {
        // Every reference charter-config read shares the canonical-root gate,
        // so a non-Git root degrades (see `resolve_directives`) instead of
        // refusing: nothing was loaded, and the reference's governance
        // reference read is best-effort over the same gate.
        return Ok(CharterConfigSections {
            charter: None,
            governance_valid: false,
            directives_valid: false,
        });
    }
    let charter = match read_charter_document(project_root) {
        CharterDocument::Parsed(document) => Some(document),
        CharterDocument::Absent => None,
        CharterDocument::Unreadable => {
            // `load_charter_yaml` raises on malformed YAML, so BOTH reference
            // reads fail and the documented degrade applies (the file is still
            // described by `project_charter`, which reads its bytes directly).
            return Ok(CharterConfigSections {
                charter: None,
                governance_valid: false,
                directives_valid: false,
            });
        }
    };
    require_canonical_governance(&charter, project_root)?;
    let governance_valid = governance_section_valid(&charter);
    let directives_valid = directives_section_valid(&charter);
    Ok(CharterConfigSections {
        charter,
        governance_valid,
        directives_valid,
    })
}

/// Refuse the retired `governance.doctrine` selection key.
///
/// Mirrors `sync.require_canonical_governance`: only a `governance` MAPPING is
/// checked (a non-mapping `governance:` is a shape failure the resolver
/// catches and degrades on, not a retired-key refusal). The reference raises
/// `ActiveCharterConfigError` / `ACTIVE_CHARTER_CONFIG_INVALID` naming the
/// charter path, the key, `spec-kitty upgrade` and the cutover runbook.
fn require_canonical_governance(
    charter: &Option<serde_yaml_ng::Value>,
    project_root: &Path,
) -> Result<(), DoctrineError> {
    let Some(governance) = charter
        .as_ref()
        .and_then(|charter| charter.get("governance"))
    else {
        return Ok(());
    };
    if governance.as_mapping().is_some() && governance.get("doctrine").is_some() {
        return Err(DoctrineError::RetiredGovernanceSelection {
            path: project_root.join(sk_charter::CHARTER_YAML),
            runbook: crate::error::RETIRED_GOVERNANCE_RUNBOOK,
        });
    }
    Ok(())
}

fn is_string_sequence(value: &serde_yaml_ng::Value) -> bool {
    value
        .as_sequence()
        .is_some_and(|items| items.iter().all(serde_yaml_ng::Value::is_string))
}

/// The list fields `GovernanceCharterConfig` validates as `list[str]`.
const GOVERNANCE_STRING_LISTS: [&str; 10] = [
    "selected_paradigms",
    "selected_directives",
    "selected_tactics",
    "selected_styleguides",
    "selected_toolguides",
    "selected_procedures",
    "selected_agent_profiles",
    "selected_mission_step_contracts",
    "selected_glossary_packs",
    "selected_assets",
];

/// Whether `governance:` passes the reference `GovernanceConfig` shape.
///
/// This is a structural surface, not a full Pydantic port: the shapes the
/// reference rejects outright (a non-mapping section, a scalar `charter`, a
/// scalar `selected_directives`, a `null` list) degrade the resolution, so the
/// native read must not pretend it resolved. Unknown keys are ignored, exactly
/// as Pydantic's default `extra="ignore"` does.
fn governance_section_valid(charter: &Option<serde_yaml_ng::Value>) -> bool {
    let Some(governance) = charter
        .as_ref()
        .and_then(|charter| charter.get("governance"))
    else {
        // No governance section (or no charter file) is an empty config.
        return true;
    };
    if governance.is_null() {
        // `_load_charter_yaml_section` returns `None` for an explicit null
        // section, which `load_governance_config` treats as absent.
        return true;
    }
    let Some(governance) = governance.as_mapping() else {
        return false;
    };
    for section in [
        "testing",
        "quality",
        "commits",
        "performance",
        "branch_strategy",
    ] {
        if let Some(value) = governance.get(section)
            && !value.is_mapping()
        {
            return false;
        }
    }
    if let Some(value) = governance.get("charter") {
        let Some(charter_block) = value.as_mapping() else {
            return false;
        };
        for key in GOVERNANCE_STRING_LISTS {
            if let Some(value) = charter_block.get(key)
                && !is_string_sequence(value)
            {
                return false;
            }
        }
        for key in [
            "available_tools",
            "authority_paths",
            "governance_references",
        ] {
            if let Some(value) = charter_block.get(key)
                && !is_string_sequence(value)
            {
                return false;
            }
        }
        if let Some(value) = charter_block.get("template_set")
            && !value.is_string()
            && !value.is_null()
        {
            return false;
        }
    }
    if let Some(value) = governance.get("activations") {
        let Some(items) = value.as_sequence() else {
            return false;
        };
        if !items.iter().all(serde_yaml_ng::Value::is_mapping) {
            return false;
        }
    }
    if let Some(value) = governance.get("retrospective")
        && !value.is_null()
        && !value.is_mapping()
    {
        return false;
    }
    true
}

/// Whether the `directives:` section passes the reference `DirectivesConfig`
/// shape.
///
/// The compiled charter authors `directives: {directives: [{id, title, ...}]}`.
/// Pydantic requires the section to be a mapping, the `directives` field to be
/// a list, and each entry to be a mapping with the two REQUIRED string fields
/// `id` and `title`; an unknown key is ignored. A shape failure means
/// `load_directives_config` raises, which is part of the resolver-failure
/// degrade (never a silent partial load).
fn directives_section_valid(charter: &Option<serde_yaml_ng::Value>) -> bool {
    let Some(section) = charter
        .as_ref()
        .and_then(|charter| charter.get("directives"))
    else {
        return true;
    };
    if section.is_null() {
        return true;
    }
    let Some(section) = section.as_mapping() else {
        return false;
    };
    let Some(items) = section.get("directives") else {
        return true;
    };
    let Some(items) = items.as_sequence() else {
        return false;
    };
    items.iter().all(|item| {
        let Some(item) = item.as_mapping() else {
            return false;
        };
        if !item.get("id").is_some_and(serde_yaml_ng::Value::is_string)
            || !item
                .get("title")
                .is_some_and(serde_yaml_ng::Value::is_string)
        {
            return false;
        }
        for key in ["description", "severity"] {
            if let Some(value) = item.get(key)
                && !value.is_string()
            {
                return false;
            }
        }
        if let Some(value) = item.get("references")
            && !is_string_sequence(value)
        {
            return false;
        }
        true
    })
}

/// A project-local `directives:` entry from `charter.yaml`.
#[derive(Clone, Debug, Default)]
struct LocalDirective {
    id: String,
    title: Option<String>,
    description: Option<String>,
}

/// Resolve the effective directive set and its provenance label.
///
/// Mirrors the reference resolver: the base is the activated set (or the
/// built-in catalog default when the key is absent), the charter's
/// `selected_directives` union onto it (`+charter`), and the project-local
/// `charter.yaml` `directives:` section unions on last (`+project_local`).
/// Unknown selections are dropped with a diagnostic instead of erasing the
/// resolution.
///
/// A shape failure in either charter section is the reference's
/// resolver-failure degrade: the DIRECTLY LOADED local directives are returned
/// with a `null` source label and NO diagnostics (never the activation base
/// plus an invented provenance). A retired `governance.doctrine` key was
/// already refused in [`charter_sections`].
fn resolve_directives(
    graph: &DrgGraph,
    identity: &IdentityIndex,
    ledger: &ActivationLedger,
    state: &ActivationState,
    sections: &CharterConfigSections,
) -> Result<DirectiveResolution, DoctrineError> {
    if !sections.governance_valid || !sections.directives_valid {
        // `context_json._load_project_directives_with_diagnostics` catches a
        // `load_governance_config`/`resolve_project_governance` failure and
        // returns exactly the directly-loaded set (empty when
        // `load_directives_config` failed too), `directives_source: None`
        // (provenance unavailable is the documented operator signal) and NO
        // resolution diagnostics. This is a degrade, not a refusal: typed
        // delivery, links and every other governance block stay intact.
        let local = sections.local_directives();
        return Ok(DirectiveResolution {
            entries: local
                .iter()
                .map(|entry| local_directive_entry(&entry.id, entry))
                .collect(),
            source: None,
            diagnostics: Vec::new(),
        });
    }
    let mut diagnostics = Vec::new();
    let charter = &sections.charter;
    let local = sections.local_directives();
    let local_ids: BTreeSet<String> = local.iter().map(|entry| entry.id.clone()).collect();
    let catalog_ids: BTreeSet<String> = graph
        .nodes()
        .iter()
        .filter(|node| node.kind() == NodeKind::Directive && node.provenance() == Some("built-in"))
        .map(|node| crate::org::normalize_directive_id(&bare_id(node.urn())))
        .collect();

    let activated = ledger.allow_for(ArtifactKind::Directive);
    let (mut resolved, mut base_source) = match activated {
        None => {
            let mut base: Vec<String> = catalog_ids.iter().cloned().collect();
            base.sort();
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::CatalogDefaultApplied,
                message: format!(
                    "No activated directive set configured; using built-in catalog default ({} directives).",
                    base.len()
                ),
            });
            (base, "catalog_fallback".to_owned())
        }
        Some(tokens) => {
            let mut warnings = Vec::new();
            let mut base: BTreeSet<String> = BTreeSet::new();
            for token in tokens {
                let declared =
                    identity.activation_id(ArtifactKind::Directive, token, &mut warnings);
                // A deny that names this artifact removes it from the base
                // (the reference `PackContext` nets the allow list by D*).
                let denied = state
                    .deny_for(ArtifactKind::Directive)
                    .is_some_and(|deny| !deny.admits(&declared));
                if !denied {
                    base.insert(declared);
                }
            }
            // Unknown activation tokens are WARNINGS in the reference
            // (`_resolve_directive_base` logs them); they are not
            // `UNKNOWN_SELECTION_DROPPED` diagnostics, which are reserved for
            // charter `selected_*` tokens (`_drop_unknown`).
            let _ = warnings;
            (base.into_iter().collect(), "activation".to_owned())
        }
    };

    // NOTE: `resolve_project_governance` (the JSON entrypoint's resolver) reads
    // the charter's own selection only; the org `required_*` overlay is applied
    // by the *renderer* helper, not here. Org requirements still form the
    // required floor that protects them from deny, which is a separate seam.
    let selected = selected_directives(charter);
    if !selected.is_empty() {
        // Mirrors `_resolve_selected_directives` + `_report_charter_selection`:
        // resolved ids are de-duplicated in token order, and the merged
        // diagnostic reports the SELECTION (not the base) when nothing was
        // added.
        let mut selection: Vec<String> = Vec::new();
        for token in &selected {
            let declared = identity
                .selection_id(ArtifactKind::Directive, token)
                .or_else(|| {
                    let legacy = crate::org::normalize_directive_id(token);
                    (local_ids.contains(&legacy) || catalog_ids.contains(&legacy)).then_some(legacy)
                });
            match declared {
                Some(declared) => {
                    if !selection.contains(&declared) {
                        selection.push(declared);
                    }
                }
                None => diagnostics.push(Diagnostic {
                    code: DiagnosticCode::UnknownSelectionDropped,
                    message: format!(
                        "Unknown selected directive '{token}' dropped: not found in built-in, org packs (charter_packs.org.packs) or .kittify/charter-packs/; see 'spec-kitty charter list'."
                    ),
                }),
            }
        }
        if !selection.is_empty() {
            let added: Vec<String> = selection
                .iter()
                .filter(|id| !resolved.contains(id))
                .cloned()
                .collect();
            diagnostics.push(charter_selection_merged_diagnostic(
                "directive",
                &selection,
                &added,
                &base_source,
            ));
            for declared in added {
                resolved.push(declared);
            }
            base_source = format!("{base_source}+charter");
        }
    }

    if !local_ids.is_empty() {
        let local_order: Vec<String> = local.iter().map(|entry| entry.id.clone()).collect();
        let added: Vec<String> = local_order
            .iter()
            .filter(|id| !resolved.contains(id))
            .cloned()
            .collect();
        diagnostics.push(project_local_merged_diagnostic(
            &local_order,
            &added,
            &base_source,
        ));
        for entry in &local {
            if !resolved.contains(&entry.id) {
                resolved.push(entry.id.clone());
            }
        }
        base_source = format!("{base_source}+project_local");
    }

    // The effective deny applies after every union (activation base, charter
    // selection, project-local); it never holds a required-floor id.
    let denied = state.deny_for(ArtifactKind::Directive);
    resolved.retain(|id| denied.is_none_or(|deny| deny.admits(id)));

    let local_by_id: BTreeMap<&str, &LocalDirective> = local
        .iter()
        .map(|entry| (entry.id.as_str(), entry))
        .collect();
    let entries = resolved
        .iter()
        .map(|id| match local_by_id.get(id.as_str()) {
            Some(local) => local_directive_entry(id, local),
            None => {
                let body = artifact_document(identity, ArtifactKind::Directive, id);
                DirectiveEntry {
                    id: id.clone(),
                    source: identity
                        .source_layer(ArtifactKind::Directive, id)
                        .map(IdentityLayer::as_str)
                        .unwrap_or("built-in")
                        .to_owned(),
                    title: text_field(&body, "title").or_else(|| text_field(&body, "name")),
                    summary: text_field(&body, "intent").or_else(|| text_field(&body, "purpose")),
                }
            }
        })
        .collect();

    Ok(DirectiveResolution {
        entries,
        source: Some(base_source),
        diagnostics,
    })
}

/// The reference `_local_directive_entry` DTO: id + `project` origin + the
/// authored display fields (never a title fallback).
fn local_directive_entry(id: &str, local: &LocalDirective) -> DirectiveEntry {
    DirectiveEntry {
        id: id.to_owned(),
        source: "project".to_owned(),
        title: local.title.clone(),
        summary: local.description.clone(),
    }
}

/// The reference `_report_charter_selection` diagnostic.
///
/// `selection` is the de-duplicated resolved charter selection, `added` the
/// subset beyond the base. The "none added" branch names the SELECTION, not
/// the base set (a wording difference the previous lenient check missed).
fn charter_selection_merged_diagnostic(
    noun: &str,
    selection: &[String],
    added: &[String],
    base_source: &str,
) -> Diagnostic {
    if !added.is_empty() {
        return Diagnostic {
            code: DiagnosticCode::SelectionMerged,
            message: format!(
                "Charter selected {} {noun}(s) beyond the {base_source} base: {}.",
                added.len(),
                added.join(", ")
            ),
        };
    }
    Diagnostic {
        code: DiagnosticCode::SelectionMerged,
        message: format!(
            "All {} charter-selected {noun}(s) already present in the {base_source} base; none added: {}.",
            selection.len(),
            selection.join(", ")
        ),
    }
}

/// The reference `_resolve_directives_selection` project-local diagnostic.
fn project_local_merged_diagnostic(
    local: &[String],
    added: &[String],
    base_source: &str,
) -> Diagnostic {
    if !added.is_empty() {
        return Diagnostic {
            code: DiagnosticCode::SelectionMerged,
            message: format!(
                "Merged {} project-local directive(s) onto the {base_source} set: {}.",
                added.len(),
                added.join(", ")
            ),
        };
    }
    Diagnostic {
        code: DiagnosticCode::SelectionMerged,
        message: format!(
            "All {} project-local directive(s) already present in the {base_source} set; none added: {}.",
            local.len(),
            local.join(", ")
        ),
    }
}

/// The authored charter document, distinguishing "absent" from "unparseable".
///
/// The reference's `_load_charter_yaml_section` returns `None` for an absent
/// file (or no section) but lets a malformed YAML document raise, which the
/// two best-effort readers catch and degrade on. Treating an unparseable file
/// as absent would invent a resolved catalogue provenance instead.
enum CharterDocument {
    /// No charter file (or the path could not be resolved inside the root).
    Absent,
    /// A parsed document (any YAML shape; non-mapping roots read as no section).
    Parsed(serde_yaml_ng::Value),
    /// The file exists but could not be read or parsed.
    Unreadable,
}

fn read_charter_document(project_root: &Path) -> CharterDocument {
    // The charter file is user-authored: resolve it under the project root so a
    // symlink cannot redirect the governance read outside the checkout.
    let Ok(path) = sk_kernel::resolve_within(project_root, Path::new(sk_charter::CHARTER_YAML))
    else {
        return CharterDocument::Absent;
    };
    if !path.is_file() {
        return CharterDocument::Absent;
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return CharterDocument::Unreadable;
    };
    match serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&text) {
        Ok(document) => CharterDocument::Parsed(document),
        Err(_) => CharterDocument::Unreadable,
    }
}

fn local_directives(charter: &Option<serde_yaml_ng::Value>) -> Vec<LocalDirective> {
    let Some(charter) = charter else {
        return Vec::new();
    };
    // The authored section is `directives: {directives: [...]}` in the
    // compiled charter. Whether that shape is the reference `DirectivesConfig`
    // is `directives_section_valid`'s job; this reader only extracts entries
    // from an accepted section.
    let Some(section) = charter.get("directives") else {
        return Vec::new();
    };
    let Some(items) = section
        .get("directives")
        .and_then(|value| value.as_sequence())
    else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let id = item.get("id").and_then(serde_yaml_ng::Value::as_str)?;
            if id.trim().is_empty() {
                return None;
            }
            Some(LocalDirective {
                id: id.trim().to_owned(),
                title: item
                    .get("title")
                    .and_then(serde_yaml_ng::Value::as_str)
                    .map(str::to_owned),
                description: item
                    .get("description")
                    .and_then(serde_yaml_ng::Value::as_str)
                    .map(str::to_owned),
            })
        })
        .collect()
}

/// The charter's `governance.charter.selected_directives`, when readable.
///
/// The retired `governance.doctrine` key is NEVER a fallback: it is refused in
/// [`require_canonical_governance`] before any reader runs.
fn selected_directives(charter: &Option<serde_yaml_ng::Value>) -> Vec<String> {
    let Some(charter) = charter else {
        return Vec::new();
    };
    let selection = charter
        .get("governance")
        .and_then(|governance| governance.get("charter"));
    selection
        .and_then(|selection| selection.get("selected_directives"))
        .and_then(|value| value.as_sequence())
        .map(|items| {
            items
                .iter()
                .filter_map(serde_yaml_ng::Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn project_charter_block(project_root: &Path) -> ProjectCharterBlock {
    let charter_yaml = project_root.join(sk_charter::CHARTER_YAML);
    let charter_md = project_root.join(sk_charter::CHARTER_MD);
    let present = charter_yaml.is_file();
    let mut block = ProjectCharterBlock {
        present,
        path: sk_charter::CHARTER_YAML.to_owned(),
        charter_md_present: charter_md.is_file(),
        charter_md_path: sk_charter::CHARTER_MD.to_owned(),
        ..ProjectCharterBlock::default()
    };
    if !present {
        return block;
    }
    block.bytes = std::fs::metadata(&charter_yaml).ok().map(|meta| meta.len());
    let Ok(metadata_path) =
        sk_kernel::resolve_within(project_root, Path::new(".kittify/charter/metadata.yaml"))
    else {
        return block;
    };
    let Ok(text) = std::fs::read_to_string(&metadata_path) else {
        return block;
    };
    let Ok(metadata) = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&text) else {
        return block;
    };
    block.hash = metadata
        .get("charter_hash")
        .and_then(serde_yaml_ng::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    block.source_path = metadata
        .get("source_path")
        .and_then(serde_yaml_ng::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    block.bundle_schema_version = metadata
        .get("bundle_schema_version")
        .and_then(serde_yaml_ng::Value::as_i64);
    block.schema_version = metadata
        .get("schema_version")
        .and_then(serde_yaml_ng::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    block
}

/// Import the configured org packs' `org-charter.yaml` policies into the JSON
/// block shape (the reference `load_org_charter_json_block`).
pub fn org_charter_block(project_root: &Path) -> OrgCharterBlock {
    let Ok(roots) = org_charter::configured_org_roots(project_root) else {
        return OrgCharterBlock::default();
    };
    let mut packs = Vec::new();
    for (declared, root) in roots {
        if !root.is_dir() {
            continue;
        }
        let Ok(Some(policy)) = load_org_charter_policy(&root) else {
            continue;
        };
        packs.push(OrgCharterPack {
            // The reference falls back to the pack DIRECTORY basename
            // (`getattr(policy, "org_name", None) or org_root.name`), never the
            // configured display name.
            pack_name: policy.org_name.clone().unwrap_or_else(|| {
                root.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(declared.as_str())
                    .to_owned()
            }),
            governance_policies: policy
                .governance_policies
                .iter()
                .map(|policy| OrgGovernancePolicy {
                    field: policy.field.clone(),
                    value: policy.value.clone(),
                    enforcement: policy.enforcement.clone(),
                    source: "org".to_owned(),
                })
                .collect(),
            required_directives: policy.required_for(ArtifactKind::Directive).to_vec(),
        });
    }
    OrgCharterBlock {
        present: !packs.is_empty(),
        packs,
    }
}

fn governance_reference_status(
    project_root: &Path,
    governance_valid: bool,
) -> Vec<GovernanceReferenceStatus> {
    // The reference's `_load_governance_charter_config` is best-effort: it
    // swallows the canonical-root failure AND every malformed-governance
    // shape, collapsing to the default (empty) selection. Only the retired
    // shapes (a `governance.doctrine` key or a retired pack field) re-raise —
    // and the retired key was already refused in `charter_sections`. So a
    // non-Git root and a malformed governance section both report no
    // governance references, even when `charter.yaml` carries them on disk;
    // the written file is still described by `project_charter`, which reads
    // the root directly.
    if !governance_valid || !charter_config_read(project_root).is_available() {
        return Vec::new();
    }
    let CharterDocument::Parsed(charter) = read_charter_document(project_root) else {
        return Vec::new();
    };
    let references = charter
        .get("governance")
        .and_then(|governance| governance.get("charter"))
        .and_then(|selection| selection.get("governance_references"))
        .and_then(|value| value.as_sequence())
        .map(|items| {
            items
                .iter()
                .filter_map(serde_yaml_ng::Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let root = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let mut statuses = Vec::new();
    for raw in references {
        let path = raw.trim();
        if path.is_empty() {
            continue;
        }
        let relative = Path::new(path);
        let unsafe_reason = if relative.is_absolute() {
            Some(format!(
                "Unsafe governance reference {path}: paths must be repository-relative, not absolute."
            ))
        } else if relative
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            Some(format!(
                "Unsafe governance reference {path}: parent-directory traversal is not allowed."
            ))
        } else {
            let candidate = root.join(relative);
            let resolved = candidate
                .canonicalize()
                .unwrap_or_else(|_| candidate.clone());
            (!resolved.starts_with(&root)).then(|| {
                format!(
                    "Unsafe governance reference {path}: resolved path escapes the repository root."
                )
            })
        };
        if let Some(warning) = unsafe_reason {
            statuses.push(GovernanceReferenceStatus {
                path: path.to_owned(),
                exists: false,
                safe: false,
                warning: Some(warning),
            });
            continue;
        }
        let candidate = root.join(relative);
        let exists = candidate.exists();
        let warning = if !exists {
            Some(format!(
                "Missing governance reference {path}. Create it under the repository root or remove it from governance_references in .kittify/charter/charter.md."
            ))
        } else if !candidate.is_file() {
            Some(format!(
                "Governance reference {path} is not a file. Point governance_references at a markdown or text document under the repository root."
            ))
        } else {
            None
        };
        statuses.push(GovernanceReferenceStatus {
            path: path.to_owned(),
            exists,
            safe: true,
            warning,
        });
    }
    statuses
}

/// Normalize an action token to a full `action:<mission>/<step>` URN.
pub fn resolve_action_urn(
    graph: &DrgGraph,
    action: &str,
    mission_type: Option<&str>,
) -> Result<String, DoctrineError> {
    if action.starts_with("action:") {
        if !is_valid_urn(action) {
            return Err(DoctrineError::InvalidAction {
                action: action.to_owned(),
            });
        }
        return Ok(action.to_owned());
    }
    if action.contains(':') || action.trim().is_empty() {
        return Err(DoctrineError::InvalidAction {
            action: action.to_owned(),
        });
    }
    if let Some(mission_type) = mission_type {
        let urn = format!("action:{mission_type}/{action}");
        if !is_valid_urn(&urn) {
            return Err(DoctrineError::InvalidAction {
                action: action.to_owned(),
            });
        }
        return Ok(urn);
    }
    let matches: Vec<String> = graph
        .nodes()
        .iter()
        .filter(|node| node.kind() == NodeKind::Action)
        .filter(|node| {
            let id = node.urn().split_once(':').map(|(_, id)| id).unwrap_or("");
            id.split('/').next_back() == Some(action)
        })
        .map(|node| node.urn().to_owned())
        .collect();
    match matches.len() {
        1 => Ok(matches[0].clone()),
        0 => Err(DoctrineError::InvalidAction {
            action: action.to_owned(),
        }),
        _ => Err(DoctrineError::InvalidAction {
            action: format!(
                "{action:?} is ambiguous without --mission-type; candidates: {}",
                matches.join(", ")
            ),
        }),
    }
}

/// The pack roots and identity index a context build needs.
pub fn context_roots(
    project_root: &Path,
) -> Result<(PathBuf, Vec<PathBuf>, Option<PathBuf>), DoctrineError> {
    context_roots_with_packs_root(project_root, org_charter::packs_root_override().as_deref())
}

/// [`context_roots`] resolved against an explicit packs-root override.
///
/// `packs_root` accepts the documented parent (`<...>/packs`) or the built-in
/// leaf (`<...>/packs/built-in`), normalized through the one shared helper, so
/// the context build and the org required floor always resolve the same tiers.
pub fn context_roots_with_packs_root(
    project_root: &Path,
    packs_root: Option<&Path>,
) -> Result<(PathBuf, Vec<PathBuf>, Option<PathBuf>), DoctrineError> {
    let config = sk_charter::load_project_config(project_root, &sk_charter::Limits::default())?;
    let expansion = sk_charter::PathExpansion::from_env();
    let search = match packs_root {
        Some(root) => sk_charter::BuiltInSearch::from_env(project_root)
            .with_packs_root_env(org_charter::normalize_packs_root(root)),
        None => org_charter::built_in_search(project_root),
    };
    let roots = sk_charter::PackRoots::resolve(project_root, &config, &expansion, Some(&search))?;
    let built_in = roots
        .built_in
        .ok_or(DoctrineError::PackRootUnavailable { tier: "built-in" })?;
    let org = roots
        .organizations
        .iter()
        .map(|pack| pack.root.clone())
        .collect();
    Ok((built_in, org, roots.project))
}

/// Discover the artifact kinds the context contract currently delivers.
pub fn typed_kinds() -> [ArtifactKind; 5] {
    [
        ArtifactKind::Directive,
        ArtifactKind::Tactic,
        ArtifactKind::Styleguide,
        ArtifactKind::Toolguide,
        ArtifactKind::Procedure,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{DrgEdge, DrgNode};
    use crate::identity::{IdentityIndex, scan_all};
    use crate::profile::ProfileRepository;
    use std::fs;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// Make `root` a real Git checkout.
    ///
    /// The reference resolves `charter.yaml` through
    /// `charter.resolution.resolve_canonical_repo_root`, so the directive
    /// resolution, its provenance label and the governance-reference status
    /// are Git-dependent contracts: a bare temporary directory would exercise
    /// the documented non-Git degrade path instead. Tests that assert those
    /// metadata blocks need the realistic setup; deny/delivery assertions do
    /// not depend on it and stay unchanged.
    fn git_root(root: &Path) {
        let output = std::process::Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(root)
            .output()
            .expect("run git init");
        assert!(
            output.status.success(),
            "git init failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn profile_yaml(id: &str) -> String {
        format!(
            "profile-id: {id}\nname: {id}\npurpose: review\nroles: [reviewer]\nspecialization:\n  primary-focus: review\n"
        )
    }

    fn graph() -> DrgGraph {
        DrgGraph::new(
            "1.0",
            "x",
            "built-in",
            vec![
                DrgNode::new(
                    "action:software-dev/implement",
                    NodeKind::Action,
                    None,
                    Vec::new(),
                )
                .unwrap(),
                DrgNode::new(
                    "directive:DIRECTIVE_001",
                    NodeKind::Directive,
                    Some("Arch".to_owned()),
                    Vec::new(),
                )
                .unwrap()
                .with_provenance(Some("built-in".to_owned())),
                DrgNode::new(
                    "directive:DIRECTIVE_002",
                    NodeKind::Directive,
                    Some("Style".to_owned()),
                    Vec::new(),
                )
                .unwrap()
                .with_provenance(Some("built-in".to_owned())),
                DrgNode::new("tactic:hard", NodeKind::Tactic, None, Vec::new())
                    .unwrap()
                    .with_provenance(Some("built-in".to_owned())),
                DrgNode::new("tactic:soft", NodeKind::Tactic, None, Vec::new())
                    .unwrap()
                    .with_provenance(Some("built-in".to_owned())),
                DrgNode::new(
                    "glossary_scope:terms",
                    NodeKind::GlossaryScope,
                    None,
                    Vec::new(),
                )
                .unwrap(),
                DrgNode::new(
                    "agent_profile:reviewer",
                    NodeKind::AgentProfile,
                    None,
                    Vec::new(),
                )
                .unwrap(),
            ],
            vec![
                DrgEdge::new(
                    "action:software-dev/implement",
                    "directive:DIRECTIVE_001",
                    Relation::Scope,
                )
                .unwrap(),
                DrgEdge::new("directive:DIRECTIVE_001", "tactic:hard", Relation::Requires).unwrap(),
                DrgEdge::new("directive:DIRECTIVE_001", "tactic:soft", Relation::Suggests)
                    .unwrap()
                    .with_when("risky change"),
                DrgEdge::new(
                    "directive:DIRECTIVE_001",
                    "glossary_scope:terms",
                    Relation::Vocabulary,
                )
                .unwrap(),
                DrgEdge::new("agent_profile:reviewer", "tactic:soft", Relation::Suggests)
                    .unwrap()
                    .with_when("risky change"),
            ],
        )
        .unwrap()
    }

    fn inputs<'a>(
        graph: &'a DrgGraph,
        identity: &'a IdentityIndex,
        ledger: &'a ActivationLedger,
        profiles: &'a ProfileRepository,
        root: &'a Path,
        floor: &'a BTreeMap<ArtifactKind, Vec<String>>,
    ) -> ContextInputs<'a> {
        ContextInputs {
            graph,
            ledger,
            identity,
            profiles,
            project_root: root,
            required_floor: floor,
            project_warnings: &[],
        }
    }

    fn fixture(root: &Path) -> IdentityIndex {
        let built_in = root.join("packs/built-in");
        write(
            &built_in.join("directives/001-arch.directive.yaml"),
            "id: DIRECTIVE_001\nschema_version: '1.0'\ntitle: Arch\nintent: Keep seams\nscope: x\n",
        );
        write(
            &built_in.join("directives/002-style.directive.yaml"),
            "id: DIRECTIVE_002\nschema_version: '1.0'\ntitle: Style\nintent: Be clear\n",
        );
        write(
            &built_in.join("tactics/hard.tactic.yaml"),
            "id: hard\nschema_version: '1.0'\nname: Hard\npurpose: do it\nsteps:\n  - title: one\n",
        );
        write(
            &built_in.join("tactics/soft.tactic.yaml"),
            "id: soft\nschema_version: '1.0'\nname: Soft\npurpose: maybe\nsteps:\n  - title: one\n",
        );
        let mut identity = IdentityIndex::from_roots(&built_in, &[], None);
        scan_all(&mut identity);
        identity
    }

    #[test]
    fn context_is_deterministic_and_separates_inline_from_link() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let identity = fixture(root);
        let profiles = ProfileRepository::load(&root.join("packs/built-in"), &[], None).unwrap();
        // An explicit selection keeps the catalogue defaults out of the seed
        // set, so the `suggests`-reached tactic is a link, not a root.
        let mut ledger = ActivationLedger::default();
        ledger
            .allow
            .insert(ArtifactKind::Directive, Some(vec!["001-arch".to_owned()]));
        ledger
            .allow
            .insert(ArtifactKind::Tactic, Some(vec!["hard".to_owned()]));
        let floor = BTreeMap::new();
        let graph = graph();
        let request = ContextRequest::new("action:software-dev/implement");
        let first = build_action_context(
            &inputs(&graph, &identity, &ledger, &profiles, root, &floor),
            &request,
        )
        .unwrap();
        let second = build_action_context(
            &inputs(&graph, &identity, &ledger, &profiles, root, &floor),
            &request,
        )
        .unwrap();
        assert_eq!(first.to_json(), second.to_json());
        assert_eq!(first.mode, ContextMode::Bootstrap);
        assert_eq!(first.directives.len(), 1);
        assert_eq!(first.directives[0].delivery, Delivery::Inline);
        assert_eq!(first.directives[0].title.as_deref(), Some("Arch"));
        assert_eq!(first.directives[0].summary.as_deref(), Some("Keep seams"));
        // The inline body is the real artifact document.
        assert_eq!(
            first.directives[0]
                .body
                .as_ref()
                .and_then(|body| body.get("scope"))
                .and_then(|value| value.as_str()),
            Some("x")
        );
        // The explicit tactic allow list gates `soft` out of the filtered graph,
        // so the `suggests` edge to it is not delivered at all.
        assert_eq!(first.tactics.len(), 1);
        assert_eq!(first.tactics[0].id, "hard");
        assert_eq!(first.tactics[0].delivery, Delivery::Inline);
        assert!(first.tactics[0].body.is_some());
        // The top-level link set names only *delivered* targets. A glossary
        // scope reached by `vocabulary` is not a delivered bundle artifact in the
        // reference (the legacy scope kind is excluded from the delivery table),
        // so it is not named here either.
        assert!(first.references.iter().all(|reference| {
            first
                .tactics
                .iter()
                .chain(first.directives.iter())
                .any(|entry| entry.id == reference.id)
        }));
        for key in first.to_json().as_object().unwrap().keys() {
            assert!(CONTEXT_CONTRACT_TOP_LEVEL_KEYS.contains(&key.as_str()));
        }
        let entry = &first.to_json()["directives"][0];
        let mut keys: Vec<&str> = entry
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "body",
                "delivery",
                "id",
                "references",
                "source",
                "summary",
                "title"
            ]
        );
    }

    #[test]
    fn include_all_inlines_suggests_and_profile_channel() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let identity = fixture(root);
        write(
            &root.join("packs/built-in/agent_profiles/reviewer.agent.yaml"),
            &profile_yaml("reviewer"),
        );
        let profiles = ProfileRepository::load(&root.join("packs/built-in"), &[], None).unwrap();
        let ledger = ActivationLedger::default();
        let floor = BTreeMap::new();
        let graph = graph();
        let request = ContextRequest {
            action: "action:software-dev/implement",
            mission_type: None,
            depth: 1,
            profile: Some("reviewer"),
            include_all: true,
        };
        let context = build_action_context(
            &inputs(&graph, &identity, &ledger, &profiles, root, &floor),
            &request,
        )
        .unwrap();
        let soft = context
            .tactics
            .iter()
            .find(|entry| entry.id == "soft")
            .unwrap();
        assert_eq!(soft.delivery, Delivery::Inline);
        assert!(soft.body.is_some());
        assert_eq!(context.profile_links.len(), 1);
        assert_eq!(
            context.profile_links[0].when.as_deref(),
            Some("risky change")
        );
    }

    #[test]
    fn bare_bootstrap_action_succeeds_without_a_unique_node() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let identity = fixture(root);
        // `directives_source`/`all_directives` are Git-dependent reference
        // contracts (see `git_root`).
        git_root(root);
        let profiles = ProfileRepository::load(&root.join("packs/built-in"), &[], None).unwrap();
        let ledger = ActivationLedger::default();
        let floor = BTreeMap::new();
        let mut graph = graph();
        let nodes = graph.nodes().to_vec();
        let edges = graph.edges().to_vec();
        let mut nodes = nodes;
        nodes.push(DrgNode::new("action:plan/review", NodeKind::Action, None, Vec::new()).unwrap());
        nodes.push(
            DrgNode::new(
                "action:software-dev/review",
                NodeKind::Action,
                None,
                Vec::new(),
            )
            .unwrap(),
        );
        graph = DrgGraph::new("1.0", "x", "t", nodes, edges).unwrap();
        let context = build_action_context(
            &inputs(&graph, &identity, &ledger, &profiles, root, &floor),
            &ContextRequest::new("review"),
        )
        .unwrap();
        assert_eq!(context.mode, ContextMode::Bootstrap);
        assert_eq!(context.action, "review");
        assert!(context.directives.is_empty());
        // Governance blocks are still resolved.
        assert!(context.directives_source.is_some());
        assert!(!context.all_directives.is_empty());
    }

    #[test]
    fn activation_filter_removes_deactivated_artifacts() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let identity = fixture(root);
        let profiles = ProfileRepository::load(&root.join("packs/built-in"), &[], None).unwrap();
        let mut ledger = ActivationLedger::default();
        ledger
            .deny
            .insert(ArtifactKind::Tactic, vec!["hard".to_owned()]);
        let floor = BTreeMap::new();
        let graph = graph();
        let context = build_action_context(
            &inputs(&graph, &identity, &ledger, &profiles, root, &floor),
            &ContextRequest::new("action:software-dev/implement"),
        )
        .unwrap();
        assert!(context.tactics.iter().all(|entry| entry.id != "hard"));
    }

    #[test]
    fn malformed_and_unknown_profiles_are_diagnosed() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let identity = fixture(root);
        write(
            &root.join("packs/built-in/agent_profiles/reviewer.agent.yaml"),
            &profile_yaml("reviewer"),
        );
        let profiles = ProfileRepository::load(&root.join("packs/built-in"), &[], None).unwrap();
        let ledger = ActivationLedger::default();
        let floor = BTreeMap::new();
        let graph = graph();
        let request = ContextRequest {
            action: "action:software-dev/implement",
            mission_type: None,
            depth: 1,
            profile: Some("nobody"),
            include_all: false,
        };
        let context = build_action_context(
            &inputs(&graph, &identity, &ledger, &profiles, root, &floor),
            &request,
        )
        .unwrap();
        assert!(
            context
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == DiagnosticCode::UnresolvedReference)
        );
    }

    #[test]
    fn compact_mode_carries_no_action_grain_but_keeps_governance() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let identity = fixture(root);
        // The governance provenance asserted below resolves only through the
        // reference's canonical (Git) root gate.
        git_root(root);
        let profiles = ProfileRepository::load(&root.join("packs/built-in"), &[], None).unwrap();
        let ledger = ActivationLedger::default();
        let floor = BTreeMap::new();
        let context = build_action_context(
            &inputs(&graph(), &identity, &ledger, &profiles, root, &floor),
            &ContextRequest::new("action:research/unlisted"),
        )
        .unwrap();
        assert_eq!(context.mode, ContextMode::Compact);
        assert!(context.directives.is_empty());
        assert!(context.references.is_empty());
        assert!(!context.all_directives.is_empty());
        assert_eq!(
            context.directives_source.as_deref(),
            Some("catalog_fallback")
        );
    }

    #[test]
    fn project_governance_labels_and_local_directives() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let identity = fixture(root);
        // The charter selection/local-directive union is a Git-dependent
        // reference contract (see `git_root`).
        git_root(root);
        let profiles = ProfileRepository::load(&root.join("packs/built-in"), &[], None).unwrap();
        let mut ledger = ActivationLedger::default();
        ledger
            .allow
            .insert(ArtifactKind::Directive, Some(vec!["001-arch".to_owned()]));
        let floor = BTreeMap::new();
        write(
            &root.join(sk_charter::CHARTER_YAML),
            "governance:\n  charter:\n    selected_directives: [DIRECTIVE_002, NOPE]\ndirectives:\n  directives:\n    - id: DIRECTIVE_009\n      title: Local\n      description: Authored here\n",
        );
        let context = build_action_context(
            &inputs(&graph(), &identity, &ledger, &profiles, root, &floor),
            &ContextRequest::new("action:software-dev/implement"),
        )
        .unwrap();
        assert_eq!(
            context.directives_source.as_deref(),
            Some("activation+charter+project_local")
        );
        let ids: Vec<&str> = context
            .all_directives
            .iter()
            .map(|entry| entry.id.as_str())
            .collect();
        assert_eq!(ids, vec!["DIRECTIVE_001", "DIRECTIVE_002", "DIRECTIVE_009"]);
        let local = context
            .all_directives
            .iter()
            .find(|entry| entry.id == "DIRECTIVE_009")
            .unwrap();
        assert_eq!(local.source, "project");
        assert_eq!(local.title.as_deref(), Some("Local"));
        assert!(
            context
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == DiagnosticCode::UnknownSelectionDropped)
        );
    }

    #[test]
    fn project_charter_block_always_carries_paths_and_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let block = project_charter_block(root);
        assert!(!block.present);
        assert_eq!(block.path, sk_charter::CHARTER_YAML);
        assert_eq!(block.charter_md_path, sk_charter::CHARTER_MD);

        write(&root.join(sk_charter::CHARTER_YAML), "charter:\n  x: 1\n");
        write(
            &root.join(".kittify/charter/metadata.yaml"),
            "charter_hash: sha256:abc\nsource_path: .kittify/charter/charter.md\nbundle_schema_version: 2\nschema_version: 1.0.0\n",
        );
        let block = project_charter_block(root);
        assert!(block.present);
        assert_eq!(block.bytes, Some(16));
        assert_eq!(block.hash.as_deref(), Some("sha256:abc"));
        assert_eq!(
            block.source_path.as_deref(),
            Some(".kittify/charter/charter.md")
        );
        assert_eq!(block.bundle_schema_version, Some(2));
        assert_eq!(block.schema_version.as_deref(), Some("1.0.0"));
    }

    #[test]
    fn org_charter_block_summarises_configured_packs() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let pack = root.join("packs/acme");
        write(
            &pack.join(org_charter::ORG_CHARTER),
            "org_name: Acme\nrequired_directives: [DIRECTIVE_001]\ngovernance_policies:\n  - field: commits\n    value: signed\n",
        );
        write(
            &root.join("packs/built-in/pack.yaml"),
            "name: built-in\npack_level: built-in\n",
        );
        write(
            &root.join(".kittify/config.yaml"),
            &format!(
                "charter_packs:\n  org:\n    packs:\n      - name: acme\n        local_path: {}\n",
                pack.display()
            ),
        );
        let block = org_charter_block(root);
        assert!(block.present);
        assert_eq!(block.packs.len(), 1);
        assert_eq!(block.packs[0].pack_name, "Acme");
        assert_eq!(
            block.packs[0].required_directives,
            vec!["DIRECTIVE_001".to_owned()]
        );
        assert_eq!(block.packs[0].governance_policies[0].source, "org");
    }

    #[test]
    fn action_resolution_refuses_unknown_and_ambiguous_tokens() {
        let graph = graph();
        assert_eq!(
            resolve_action_urn(&graph, "implement", None).unwrap(),
            "action:software-dev/implement"
        );
        assert!(matches!(
            resolve_action_urn(&graph, "nope", None),
            Err(DoctrineError::InvalidAction { .. })
        ));
        assert!(matches!(
            resolve_action_urn(&graph, "not a urn", Some("m")),
            Err(DoctrineError::InvalidAction { .. })
        ));
    }

    #[test]
    fn governance_references_report_unsafe_and_missing_paths() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        // The reference reads the charter's governance selection through the
        // canonical (Git) root gate; without it the best-effort loader reports
        // no references at all.
        git_root(root);
        write(&root.join("docs/context.md"), "x");
        write(
            &root.join(sk_charter::CHARTER_YAML),
            "governance:\n  charter:\n    governance_references: [docs/context.md, missing.md, ../escape.md, /abs.md]\n",
        );
        let statuses = governance_reference_status(root, true);
        assert_eq!(statuses.len(), 4);
        assert!(statuses[0].exists && statuses[0].safe);
        assert!(!statuses[1].exists && statuses[1].safe);
        assert!(!statuses[2].safe);
        assert!(!statuses[3].safe);
    }

    #[test]
    fn typed_kinds_match_the_five_typed_arrays() {
        let kinds = typed_kinds();
        assert_eq!(kinds.len(), 5);
        assert!(kinds.contains(&ArtifactKind::Procedure));
    }

    #[test]
    fn explicit_empty_directive_allow_denies_directives_but_not_the_tactic_catalogue() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let identity = fixture(root);
        let profiles = ProfileRepository::load(&root.join("packs/built-in"), &[], None).unwrap();
        let mut ledger = ActivationLedger::default();
        // An explicit, present, EMPTY directive allow list deactivates every
        // directive — the one state that stays empty (a distinct state from an
        // absent key). It must not admit the filtered-out directive by falling
        // back to the unfiltered graph.
        ledger
            .allow
            .insert(ArtifactKind::Directive, Some(Vec::new()));
        let floor = BTreeMap::new();
        let context = build_action_context(
            &inputs(&graph(), &identity, &ledger, &profiles, root, &floor),
            &ContextRequest::new("action:software-dev/implement"),
        )
        .unwrap();
        assert!(context.directives.is_empty());
        assert!(
            context
                .references
                .iter()
                .all(|reference| reference.id != "DIRECTIVE_001"
                    && reference.id != "DIRECTIVE_002")
        );
        // The tactic key is ABSENT, and the three-state collapse is per kind:
        // it resolves to the catalogue default, so the built-in tactics are
        // selection roots even though the directive that `requires` them was
        // deactivated. The reference `_seed_sets` computes each kind
        // independently, so asserting them away here would be wrong, not a
        // deny guard.
        let ids: Vec<&str> = context
            .tactics
            .iter()
            .map(|entry| entry.id.as_str())
            .collect();
        assert_eq!(ids, vec!["hard", "soft"]);
        assert!(
            context
                .tactics
                .iter()
                .all(|entry| entry.delivery == Delivery::Inline)
        );
    }

    #[test]
    fn context_roots_accept_the_built_in_leaf_packs_root() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        // Not `<ancestor>/packs/built-in`: only the explicit packs root can
        // resolve the built-in tier, so a leaf value must be normalized.
        let packs = root.join("packstore");
        write(
            &packs.join("built-in/pack.yaml"),
            "name: built-in\npack_level: built-in\n",
        );
        let org = packs.join("acme");
        write(&org.join("pack.yaml"), "name: acme\npack_level: org\n");
        write(&org.join("org-charter.yaml"), "org_name: Acme\n");
        write(
            &root.join(".kittify/config.yaml"),
            &format!(
                "charter_packs:\n  org:\n    packs:\n      - name: acme\n        local_path: {}\n",
                org.display()
            ),
        );

        // The CLI adapter accepts either the packs parent or the built-in leaf
        // and normalizes it. The doctrine root read must agree, otherwise the
        // org floor/context sees a different tier than the adapter.
        let (parent_built_in, parent_org, parent_project) =
            context_roots_with_packs_root(root, Some(&packs)).expect("parent root");
        let (leaf_built_in, leaf_org, leaf_project) =
            context_roots_with_packs_root(root, Some(&packs.join("built-in"))).expect("leaf root");
        assert_eq!(leaf_built_in, parent_built_in);
        assert_eq!(leaf_org, parent_org);
        assert_eq!(leaf_project, parent_project);
        assert_eq!(
            leaf_built_in,
            packs.join("built-in").canonicalize().unwrap()
        );
        assert_eq!(leaf_org.len(), 1);
        assert!(leaf_project.is_none());
    }
}
