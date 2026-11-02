//! Action-scoped charter context resolution.
//!
//! Mirrors the resolution half of `src/charter/activation/context.py` and the
//! JSON contract ledger in `context_contract.py`. The payload carries the
//! versioned top-level key set; progressive-disclosure bodies, org-charter
//! policy import and the text renderers are explicitly reported as remaining
//! gaps rather than substituted with a fake body.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Serialize;

use sk_charter::{ArtifactKind, CharterBundle, Limits};

use crate::activation::{ActivationState, filter_graph_by_activation};
use crate::error::DoctrineError;
use crate::graph::{DrgGraph, NodeKind, Relation, is_valid_urn, split_urn};
use crate::identity::IdentityIndex;
use crate::ledger::ActivationLedger;
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

/// The delivery mode of a context payload.
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

/// One delivered artifact.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ArtifactEntry {
    /// The bare artifact id.
    pub id: String,
    /// The full DRG URN.
    pub urn: String,
    /// The artifact kind.
    pub kind: NodeKind,
    /// The display label, when one is declared.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Artifact origin (`built-in`, `org`, `project`).
    pub source: String,
    /// The artifact's declaring file, when the identity index knows it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_path: Option<PathBuf>,
    /// A `suggests` link's `when` clause.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
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

/// The project-charter presence block.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct ProjectCharterBlock {
    /// Whether `charter.yaml` is present.
    pub present: bool,
    /// The `charter.yaml` path, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Whether `charter.md` is present.
    pub charter_md_present: bool,
    /// The `charter.md` path, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub charter_md_path: Option<String>,
}

/// The org-charter block.
///
/// Org-charter policy import is a remaining gap; the shape stays additive and
/// empty rather than inventing a policy document.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct OrgCharterBlock {
    /// Whether any org charter policy is present.
    pub present: bool,
    /// The imported org policies (empty while the importer is pending).
    pub packs: Vec<serde_json::Value>,
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
    /// Eager directives.
    pub directives: Vec<ArtifactEntry>,
    /// Eager tactics.
    pub tactics: Vec<ArtifactEntry>,
    /// Eager styleguides.
    pub styleguides: Vec<ArtifactEntry>,
    /// Eager toolguides.
    pub toolguides: Vec<ArtifactEntry>,
    /// Eager procedures (first-class since schema 1.1.0).
    pub procedures: Vec<ArtifactEntry>,
    /// Every directive exposed by project governance.
    pub all_directives: Vec<ArtifactEntry>,
    /// Which branch resolved the directive set.
    pub directives_source: Option<String>,
    /// Resolution diagnostics, always present.
    pub diagnostics: Vec<Diagnostic>,
    /// Delivered-but-not-inlined artifacts (links).
    pub references: Vec<ArtifactEntry>,
    /// The project-charter block.
    pub project_charter: ProjectCharterBlock,
    /// The org-charter block.
    pub org_charter: OrgCharterBlock,
    /// Governance-reference status entries.
    pub governance_references: Vec<serde_json::Value>,
    /// Profile-channel links (`requires`/`specializes_from`/`suggests`).
    ///
    /// API data, not part of the versioned top-level JSON contract: callers that
    /// need it read the typed field.
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
    /// Assert that no undeclared top-level key escaped the ledger.
    pub fn top_level_keys(&self) -> BTreeSet<&'static str> {
        let value = serde_json::to_value(self).expect("context serializes");
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
        serde_json::to_value(self).expect("context serializes")
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
    let action_urn = resolve_action_urn(inputs.graph, request.action, request.mission_type)?;
    let action_body = split_urn(&action_urn).map(|(_, body)| body).unwrap_or("");
    let normalized_action = action_body
        .split_once('/')
        .map(|(_, action)| action)
        .unwrap_or(action_body)
        .to_owned();

    let state =
        ActivationState::from_ledger(inputs.ledger, inputs.identity, inputs.required_floor)?;
    let mut diagnostics = Vec::new();
    for (kind, tokens) in &state.unknown_allow_tokens {
        diagnostics.push(Diagnostic {
            code: DiagnosticCode::UnknownSelectionDropped,
            message: format!(
                "activation selection(s) {tokens:?} for kind {kind:?} name no artifact and were dropped"
            ),
        });
    }
    for warning in inputs.project_warnings {
        diagnostics.push(Diagnostic {
            code: DiagnosticCode::UnresolvedReference,
            message: warning.clone(),
        });
    }
    let filtered = filter_graph_by_activation(inputs.graph, &state);
    let delivery = deliver_action_context(&filtered, &action_urn, request.depth);
    let action_declared = inputs.graph.node(&action_urn).is_some();
    let mode = if BOOTSTRAP_ACTIONS.contains(&normalized_action.as_str()) || action_declared {
        ContextMode::Bootstrap
    } else {
        ContextMode::Compact
    };

    let mut eager: BTreeSet<String> = delivery.eager.clone();
    let mut suggested: BTreeMap<String, Option<String>> = delivery
        .suggested
        .iter()
        .map(|urn| (urn.clone(), None))
        .collect();
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
                    if delivery.all_artifacts.contains(&urn) || inputs.graph.node(&urn).is_some() {
                        eager.insert(urn.clone());
                    }
                    for link in inputs
                        .profiles
                        .channel_deliveries(profile_id, inputs.graph)?
                    {
                        if link.relation == Relation::Suggests {
                            suggested
                                .entry(link.urn.clone())
                                .or_insert_with(|| link.when.clone());
                        } else {
                            eager.insert(link.urn.clone());
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
    let mut references = Vec::new();
    let mut all_directives = Vec::new();
    let mut project_directive_seen = false;

    for urn in &eager {
        let Some(entry) = artifact_entry(&filtered, inputs.identity, urn) else {
            continue;
        };
        match entry.kind {
            NodeKind::Directive => directives.push(entry),
            NodeKind::Tactic => tactics.push(entry),
            NodeKind::Styleguide => styleguides.push(entry),
            NodeKind::Toolguide => toolguides.push(entry),
            NodeKind::Procedure => procedures.push(entry),
            _ => references.push(entry),
        }
    }
    for (urn, when) in &suggested {
        let Some(mut entry) = artifact_entry(&filtered, inputs.identity, urn) else {
            continue;
        };
        entry.when = when.clone();
        if request.include_all && requested_kind(entry.kind) {
            match entry.kind {
                NodeKind::Directive => directives.push(entry),
                NodeKind::Tactic => tactics.push(entry),
                NodeKind::Styleguide => styleguides.push(entry),
                NodeKind::Toolguide => toolguides.push(entry),
                NodeKind::Procedure => procedures.push(entry),
                _ => references.push(entry),
            }
        } else {
            references.push(entry);
        }
    }
    for urn in &delivery.glossary_scopes {
        if let Some(entry) = artifact_entry(&filtered, inputs.identity, urn) {
            references.push(entry);
        }
    }

    // Project-tier directives are exposed by `all_directives` even when the
    // action grain cites none of them.
    if mode == ContextMode::Bootstrap {
        let mut seen: BTreeSet<String> = directives.iter().map(|entry| entry.urn.clone()).collect();
        for node in inputs.graph.nodes() {
            if node.kind() == NodeKind::Directive && node.provenance() == Some("project") {
                project_directive_seen = true;
                if seen.insert(node.urn().to_owned())
                    && let Some(entry) = artifact_entry(inputs.graph, inputs.identity, node.urn())
                {
                    all_directives.push(entry);
                }
            }
        }
    }
    let mut action_directives = directives.clone();
    action_directives.append(&mut all_directives);
    sort_entries(&mut action_directives);
    sort_entries(&mut directives);
    sort_entries(&mut tactics);
    sort_entries(&mut styleguides);
    sort_entries(&mut toolguides);
    sort_entries(&mut procedures);
    references.sort_by(|left, right| (&left.urn, &left.when).cmp(&(&right.urn, &right.when)));
    references.dedup();

    let directives_source = if directives.is_empty() && action_directives.is_empty() {
        None
    } else if project_directive_seen {
        Some("action_grain+project_local".to_owned())
    } else {
        Some("action_grain".to_owned())
    };

    let (project_charter, mut bundle_diagnostics) = project_charter_block(inputs.project_root);
    diagnostics.append(&mut bundle_diagnostics);
    profile_links.sort_by(|left, right| {
        (&left.profile, &left.urn, left.relation, &left.when).cmp(&(
            &right.profile,
            &right.urn,
            right.relation,
            &right.when,
        ))
    });
    profile_links.dedup();

    let (directives, tactics, styleguides, toolguides, procedures, references) =
        if mode == ContextMode::Compact {
            (
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            )
        } else {
            (
                directives,
                tactics,
                styleguides,
                toolguides,
                procedures,
                references,
            )
        };

    Ok(CharterContext {
        context_schema_version: CONTEXT_SCHEMA_VERSION.to_owned(),
        action: normalized_action,
        mode,
        directives,
        tactics,
        styleguides,
        toolguides,
        procedures,
        all_directives: action_directives,
        directives_source,
        diagnostics,
        references,
        project_charter,
        org_charter: OrgCharterBlock::default(),
        governance_references: Vec::new(),
        profile_links,
        tension_arbiters: delivery.tension_arbiters,
        unarbitrated_tensions: delivery.unarbitrated_tensions,
    })
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

fn artifact_entry(graph: &DrgGraph, identity: &IdentityIndex, urn: &str) -> Option<ArtifactEntry> {
    let node = graph.node(urn)?;
    let kind = node.kind();
    let id = urn
        .split_once(':')
        .map(|(_, body)| body)
        .unwrap_or(urn)
        .to_owned();
    let source = match node.provenance() {
        Some("project") => "project".to_owned(),
        Some(provenance) if provenance.starts_with("org:") => "org".to_owned(),
        _ => "built-in".to_owned(),
    };
    let body_path = kind
        .artifact_kind()
        .and_then(|kind| identity.path_for(kind, &id).map(Path::to_path_buf));
    Some(ArtifactEntry {
        id,
        urn: urn.to_owned(),
        kind,
        title: node.label().map(str::to_owned),
        source,
        body_path,
        when: None,
    })
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
            id == action || id.ends_with(&format!("/{action}"))
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

fn project_charter_block(project_root: &Path) -> (ProjectCharterBlock, Vec<Diagnostic>) {
    match CharterBundle::load(project_root, &Limits::default()) {
        Ok(bundle) => {
            let charter_yaml_present = bundle.charter_yaml.is_some();
            let charter_md_present = bundle.charter_md.is_some();
            (
                ProjectCharterBlock {
                    present: charter_yaml_present,
                    path: charter_yaml_present.then(|| sk_charter::CHARTER_YAML.to_owned()),
                    charter_md_present,
                    charter_md_path: charter_md_present.then(|| sk_charter::CHARTER_MD.to_owned()),
                },
                Vec::new(),
            )
        }
        Err(error) => (
            ProjectCharterBlock::default(),
            vec![Diagnostic {
                code: DiagnosticCode::UnresolvedReference,
                message: format!("project charter bundle could not be inspected: {error}"),
            }],
        ),
    }
}

/// The pack roots and identity index a context build needs.
pub fn context_roots(
    project_root: &Path,
) -> Result<(PathBuf, Vec<PathBuf>, Option<PathBuf>), DoctrineError> {
    let config = sk_charter::load_project_config(project_root, &sk_charter::Limits::default())?;
    let expansion = sk_charter::PathExpansion::from_env();
    let search = sk_charter::BuiltInSearch::from_env(project_root);
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
                DrgEdge::new("directive:DIRECTIVE_001", "tactic:soft", Relation::Suggests).unwrap(),
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

    #[test]
    fn context_is_deterministic_and_separates_suggests() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let built_in = root.join("packs/built-in");
        write(
            &built_in.join("directives/001-arch.directive.yaml"),
            "id: DIRECTIVE_001\n",
        );
        write(&built_in.join("tactics/hard.tactic.yaml"), "id: hard\n");
        write(&built_in.join("tactics/soft.tactic.yaml"), "id: soft\n");
        let mut identity = IdentityIndex::from_roots(&built_in, &[], None);
        scan_all(&mut identity);
        let profiles = ProfileRepository::load(&built_in, &[], None).unwrap();
        let ledger = ActivationLedger::default();
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
        assert_eq!(first.tactics.len(), 1);
        assert!(first.references.iter().any(|entry| entry.id == "soft"));
        assert!(first.references.iter().any(|entry| entry.id == "terms"));
        assert!(first.references.iter().any(|entry| {
            entry
                .body_path
                .as_deref()
                .is_some_and(|path| path.ends_with("soft.tactic.yaml"))
        }));
        for key in first.to_json().as_object().unwrap().keys() {
            assert!(CONTEXT_CONTRACT_TOP_LEVEL_KEYS.contains(&key.as_str()));
        }
    }

    #[test]
    fn include_all_inlines_suggests_and_profile_channel() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let built_in = root.join("packs/built-in");
        write(
            &built_in.join("agent_profiles/reviewer.agent.yaml"),
            "profile-id: reviewer\n",
        );
        let identity = IdentityIndex::from_roots(&built_in, &[], None);
        let profiles = ProfileRepository::load(&built_in, &[], None).unwrap();
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
        assert!(context.tactics.iter().any(|entry| entry.id == "soft"));
        assert_eq!(context.profile_links.len(), 1);
        assert_eq!(
            context.profile_links[0].when.as_deref(),
            Some("risky change")
        );
    }

    #[test]
    fn activation_filter_removes_deactivated_artifacts() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let built_in = root.join("packs/built-in");
        write(
            &built_in.join("directives/001-arch.directive.yaml"),
            "id: DIRECTIVE_001\n",
        );
        write(&built_in.join("tactics/hard.tactic.yaml"), "id: hard\n");
        let identity = IdentityIndex::from_roots(&built_in, &[], None);
        let profiles = ProfileRepository::load(&built_in, &[], None).unwrap();
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
        assert!(context.tactics.is_empty());
    }

    #[test]
    fn malformed_and_unknown_profiles_are_diagnosed() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let built_in = root.join("packs/built-in");
        write(
            &built_in.join("agent_profiles/reviewer.agent.yaml"),
            "profile-id: reviewer\n",
        );
        let identity = IdentityIndex::from_roots(&built_in, &[], None);
        let profiles = ProfileRepository::load(&built_in, &[], None).unwrap();
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
    fn compact_mode_carries_no_action_grain() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let built_in = root.join("packs/built-in");
        write(
            &built_in.join("directives/001-arch.directive.yaml"),
            "id: DIRECTIVE_001\n",
        );
        let identity = IdentityIndex::from_roots(&built_in, &[], None);
        let profiles = ProfileRepository::load(&built_in, &[], None).unwrap();
        let ledger = ActivationLedger::default();
        let floor = BTreeMap::new();
        let mut graph = graph();
        let nodes = graph.nodes().to_vec();
        let edges = graph.edges().to_vec();
        let mut nodes = nodes;
        nodes.push(
            DrgNode::new(
                "action:research/outline",
                NodeKind::Action,
                None,
                Vec::new(),
            )
            .unwrap(),
        );
        let mut edges = edges;
        edges.push(
            DrgEdge::new(
                "action:research/outline",
                "directive:DIRECTIVE_001",
                Relation::Scope,
            )
            .unwrap(),
        );
        graph = DrgGraph::new("1.0", "x", "t", nodes, edges).unwrap();
        let context = build_action_context(
            &inputs(&graph, &identity, &ledger, &profiles, root, &floor),
            &ContextRequest::new("action:research/unlisted"),
        )
        .unwrap();
        assert_eq!(context.mode, ContextMode::Compact);
        assert!(context.directives.is_empty());
    }
}
