//! Scoped, shared-reference-safe cascade over the merged DRG.
//!
//! Mirrors `src/charter/activation/cascade.py`. Every decision branches on
//! graph reachability, never on a specific kind; shared artifacts are never
//! removed by a cascade deactivation.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use sk_charter::ArtifactKind;

use crate::error::DoctrineError;
use crate::graph::{DrgEdge, DrgGraph, NodeKind, Relation};

/// The doctrine reference relations a cascade follows.
pub const REFERENCE_RELATIONS: [Relation; 5] = [
    Relation::Requires,
    Relation::Suggests,
    Relation::Refines,
    Relation::Scope,
    Relation::Instantiates,
];

/// Recovery hint surfaced with the no-cascade warning.
pub const NO_CASCADE_HINT: &str = "Re-run with `--cascade <scope>` (e.g. `--cascade all` for every referenced kind) to activate the referenced artifacts, or run `charter consistency-check` to confirm the activation set is coherent.";

/// An explicit cascade scope: the all-kind shorthand or an explicit kind set.
///
/// Absence of `--cascade` is `None` at call sites and never means all.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CascadeScope {
    is_all: bool,
    kinds: BTreeSet<ArtifactKind>,
}

impl CascadeScope {
    /// The literal CLI token for the all-kind shorthand.
    pub const ALL_TOKEN: &'static str = "all";

    /// The all-kind shorthand (`--cascade all`).
    pub fn all() -> Self {
        Self {
            is_all: true,
            kinds: BTreeSet::new(),
        }
    }

    /// An explicit, non-empty kind set.
    pub fn kinds(kinds: impl IntoIterator<Item = ArtifactKind>) -> Result<Self, DoctrineError> {
        let kinds: BTreeSet<ArtifactKind> = kinds.into_iter().collect();
        if kinds.is_empty() {
            return Err(DoctrineError::InvalidAction {
                action: "<cascade scope>".to_owned(),
            });
        }
        Ok(Self {
            is_all: false,
            kinds,
        })
    }

    /// Parse a raw `--cascade` value.
    ///
    /// `None` or a blank string means no cascade (returns `None`). `all`
    /// selects every referenced kind; otherwise a comma-separated list of
    /// artifact-kind operator tokens is resolved through the canonical kind
    /// taxonomy, so an unknown token fails closed.
    pub fn parse(raw: Option<&str>) -> Result<Option<Self>, DoctrineError> {
        let Some(raw) = raw else {
            return Ok(None);
        };
        let stripped = raw.trim();
        if stripped.is_empty() {
            return Ok(None);
        }
        if stripped.eq_ignore_ascii_case(Self::ALL_TOKEN) {
            return Ok(Some(Self::all()));
        }
        let mut kinds = BTreeSet::new();
        for token in stripped.split(',').map(str::trim).filter(|t| !t.is_empty()) {
            kinds.insert(ArtifactKind::from_operator_token(token)?);
        }
        if kinds.is_empty() {
            return Ok(None);
        }
        Ok(Some(Self {
            is_all: false,
            kinds,
        }))
    }

    /// Whether `kind` is selected.
    pub fn selects(&self, kind: ArtifactKind) -> bool {
        self.is_all || self.kinds.contains(&kind)
    }

    /// Whether this is the all-kind shorthand.
    pub fn is_all(&self) -> bool {
        self.is_all
    }

    /// The explicit kinds (empty for the shorthand).
    pub fn explicit_kinds(&self) -> &BTreeSet<ArtifactKind> {
        &self.kinds
    }
}

/// A single artifact referenced from an activation source.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct ReferencedArtifact {
    /// The artifact kind.
    pub kind: ArtifactKind,
    /// The bare id (URN without its kind prefix).
    pub artifact_id: String,
    /// The full URN.
    pub urn: String,
    /// Whether a reached parent has a `requires` edge to it.
    pub via_requires: bool,
    /// The sorted parent URNs that require it.
    pub required_by: Vec<String>,
}

/// Outcome of a scoped cascade activation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CascadeActivationResult {
    /// Kind → sorted bare ids selected for activation.
    pub activated: BTreeMap<String, Vec<String>>,
    /// Kind → sorted bare ids referenced but outside scope.
    pub skipped_by_scope: BTreeMap<String, Vec<String>>,
    /// Kind → sorted bare ids reached but structurally non-activatable.
    pub not_cascaded_kind_filtered: BTreeMap<String, Vec<String>>,
    /// Kind → deactivated artifacts the walk stopped at.
    pub skipped_deactivated: BTreeMap<String, Vec<ReferencedArtifact>>,
    /// Kind → ids reachable only through a deactivated artifact.
    pub unreached_behind_deactivated: BTreeMap<String, Vec<String>>,
}

/// The data a caller needs to warn when `--cascade` is absent.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NoCascadeReport {
    /// The directly activated source.
    pub source_urn: String,
    /// Kind → sorted bare ids that would have cascaded.
    pub skipped: BTreeMap<String, Vec<String>>,
    /// Kind → sorted bare ids that are structurally non-activatable.
    pub not_cascaded_kind_filtered: BTreeMap<String, Vec<String>>,
    /// Kind → sorted bare ids of referenced deactivated artifacts.
    pub deactivated: BTreeMap<String, Vec<String>>,
    /// Actionable recovery hint.
    pub recovery_hint: String,
}

impl NoCascadeReport {
    /// Whether there is anything for the warning to report.
    pub fn has_skipped(&self) -> bool {
        !self.skipped.values().all(Vec::is_empty)
            || !self.not_cascaded_kind_filtered.values().all(Vec::is_empty)
            || !self.deactivated.values().all(Vec::is_empty)
    }
}

/// A cascade candidate skipped because it is still referenced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SharedSkip {
    /// The shared candidate URN (not deactivated).
    pub urn: String,
    /// A still-active source that continues to reference it.
    pub referencing_active_urn: String,
}

/// Result of a shared-reference-safe cascade deactivation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DeactivationPlan {
    /// Sorted URNs exclusive to the removed target.
    pub deactivate: Vec<String>,
    /// Shared candidates kept, with the still-referencing source named.
    pub skipped_shared: Vec<SharedSkip>,
    /// Kind → sorted bare ids of structurally non-activatable reached nodes.
    pub not_cascaded_kind_filtered: BTreeMap<String, Vec<String>>,
}

fn kind_of_urn(urn: &str) -> Option<ArtifactKind> {
    let prefix = urn.split_once(':').map(|(prefix, _)| prefix).unwrap_or(urn);
    NodeKind::from_prefix(prefix)?.artifact_kind()
}

fn bare_id(urn: &str) -> &str {
    urn.split_once(':').map(|(_, body)| body).unwrap_or(urn)
}

fn reference_adjacency(edges: &[DrgEdge]) -> BTreeMap<&str, Vec<&str>> {
    let mut adjacency: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for edge in edges {
        if REFERENCE_RELATIONS.contains(&edge.relation()) {
            adjacency
                .entry(edge.source())
                .or_default()
                .push(edge.target());
        }
    }
    adjacency
}

fn forward_closure(
    adjacency: &BTreeMap<&str, Vec<&str>>,
    sources: &BTreeSet<String>,
    blocked: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut visited: BTreeSet<String> = sources.clone();
    let mut queue: VecDeque<String> = sources.iter().cloned().collect();
    while let Some(current) = queue.pop_front() {
        for neighbour in adjacency.get(current.as_str()).into_iter().flatten() {
            if visited.insert((*neighbour).to_owned()) && !blocked.contains(*neighbour) {
                queue.push_back((*neighbour).to_owned());
            }
        }
    }
    visited.difference(sources).cloned().collect()
}

fn partition_artifacts(
    urns: &BTreeSet<String>,
) -> (Vec<ReferencedArtifact>, Vec<ReferencedArtifact>) {
    let mut activatable = Vec::new();
    let mut filtered = Vec::new();
    for urn in urns {
        let Some(kind) = kind_of_urn(urn) else {
            continue;
        };
        let record = ReferencedArtifact {
            kind,
            artifact_id: bare_id(urn).to_owned(),
            urn: urn.clone(),
            via_requires: false,
            required_by: Vec::new(),
        };
        if kind.activatable() {
            activatable.push(record);
        } else {
            filtered.push(record);
        }
    }
    activatable.sort_by(|left, right| {
        (left.kind.canonical(), &left.artifact_id)
            .cmp(&(right.kind.canonical(), &right.artifact_id))
    });
    filtered.sort_by(|left, right| {
        (left.kind.canonical(), &left.artifact_id)
            .cmp(&(right.kind.canonical(), &right.artifact_id))
    });
    (activatable, filtered)
}

fn referenced_artifacts(
    graph: &DrgGraph,
    source_urn: &str,
) -> (Vec<ReferencedArtifact>, Vec<ReferencedArtifact>) {
    let adjacency = reference_adjacency(graph.edges());
    let urns = forward_closure(
        &adjacency,
        &BTreeSet::from([source_urn.to_owned()]),
        &BTreeSet::new(),
    );
    partition_artifacts(&urns)
}

fn bucket_ids(refs: &[ReferencedArtifact]) -> BTreeMap<String, Vec<String>> {
    let mut buckets: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for record in refs {
        buckets
            .entry(record.kind.canonical().to_owned())
            .or_default()
            .push(record.artifact_id.clone());
    }
    for ids in buckets.values_mut() {
        ids.sort();
    }
    buckets
}

fn bucket_records(refs: &[ReferencedArtifact]) -> BTreeMap<String, Vec<ReferencedArtifact>> {
    let mut buckets: BTreeMap<String, Vec<ReferencedArtifact>> = BTreeMap::new();
    for record in refs {
        buckets
            .entry(record.kind.canonical().to_owned())
            .or_default()
            .push(record.clone());
    }
    buckets
}

fn requires_parents(
    edges: &[DrgEdge],
    parents: &BTreeSet<String>,
) -> BTreeMap<String, Vec<String>> {
    let mut required_by: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for edge in edges {
        if edge.relation() == Relation::Requires && parents.contains(edge.source()) {
            required_by
                .entry(edge.target().to_owned())
                .or_default()
                .insert(edge.source().to_owned());
        }
    }
    required_by
        .into_iter()
        .map(|(target, sources)| (target, sources.into_iter().collect()))
        .collect()
}

/// Compute scoped cascade activation targets for an activation state.
///
/// The denied set is the state's **effective** deny (`D* = D − R`), so a stale
/// deny of an org-required artifact can never stop a cascade at that artifact.
pub fn cascade_activation_targets_for_state(
    graph: &DrgGraph,
    source_urn: &str,
    scope: &CascadeScope,
    state: &crate::activation::ActivationState,
    include_deactivated: bool,
) -> CascadeActivationResult {
    cascade_activation_targets(
        graph,
        source_urn,
        scope,
        &state.deactivated_urns(),
        include_deactivated,
    )
}

/// The no-cascade report for an activation state (floor-netted deny).
pub fn referenced_but_not_cascaded_for_state(
    graph: &DrgGraph,
    source_urn: &str,
    state: &crate::activation::ActivationState,
) -> NoCascadeReport {
    referenced_but_not_cascaded(graph, source_urn, &state.deactivated_urns())
}

/// Compute scoped cascade activation targets for `source_urn`.
pub fn cascade_activation_targets(
    graph: &DrgGraph,
    source_urn: &str,
    scope: &CascadeScope,
    deactivated_urns: &BTreeSet<String>,
    include_deactivated: bool,
) -> CascadeActivationResult {
    let adjacency = reference_adjacency(graph.edges());
    let sources = BTreeSet::from([source_urn.to_owned()]);
    let full_urns = forward_closure(&adjacency, &sources, &BTreeSet::new());
    let (activatable, kind_filtered) = partition_artifacts(&full_urns);
    let denied: BTreeSet<String> = if include_deactivated {
        BTreeSet::new()
    } else {
        deactivated_urns.difference(&sources).cloned().collect()
    };
    let reached = if denied.is_empty() {
        full_urns.clone()
    } else {
        forward_closure(&adjacency, &sources, &denied)
    };
    let mut parents = reached.clone();
    parents.insert(source_urn.to_owned());
    let required_by = requires_parents(graph.edges(), &parents);

    let mut in_scope = Vec::new();
    let mut out_of_scope = Vec::new();
    let mut deactivated = Vec::new();
    let mut unreached = Vec::new();
    for mut record in activatable {
        if !scope.selects(record.kind) {
            out_of_scope.push(record);
        } else if denied.contains(&record.urn) {
            record.required_by = required_by.get(&record.urn).cloned().unwrap_or_default();
            record.via_requires = !record.required_by.is_empty();
            deactivated.push(record);
        } else if reached.contains(&record.urn) {
            in_scope.push(record);
        } else {
            unreached.push(record);
        }
    }

    CascadeActivationResult {
        activated: bucket_ids(&in_scope),
        skipped_by_scope: bucket_ids(&out_of_scope),
        not_cascaded_kind_filtered: bucket_ids(&kind_filtered),
        skipped_deactivated: bucket_records(&deactivated),
        unreached_behind_deactivated: bucket_ids(&unreached),
    }
}

/// Return the referenced-but-not-cascaded artifacts for the no-cascade warning.
pub fn referenced_but_not_cascaded(
    graph: &DrgGraph,
    source_urn: &str,
    deactivated_urns: &BTreeSet<String>,
) -> NoCascadeReport {
    let (activatable, kind_filtered) = referenced_artifacts(graph, source_urn);
    let skipped: Vec<ReferencedArtifact> = activatable
        .iter()
        .filter(|record| !deactivated_urns.contains(&record.urn))
        .cloned()
        .collect();
    let deactivated: Vec<ReferencedArtifact> = activatable
        .iter()
        .filter(|record| deactivated_urns.contains(&record.urn))
        .cloned()
        .collect();
    NoCascadeReport {
        source_urn: source_urn.to_owned(),
        skipped: bucket_ids(&skipped),
        not_cascaded_kind_filtered: bucket_ids(&kind_filtered),
        deactivated: bucket_ids(&deactivated),
        recovery_hint: NO_CASCADE_HINT.to_owned(),
    }
}

/// Compute a shared-reference-safe cascade deactivation plan.
///
/// A candidate is exclusive — and therefore safe to deactivate — iff it is
/// unreachable from **every** other still-activated source once `target_urn` is
/// removed. Shared candidates are recorded with the lowest still-referencing
/// source named and are never removed.
pub fn deactivation_plan(
    graph: &DrgGraph,
    target_urn: &str,
    scope: &CascadeScope,
    active_urns: &BTreeSet<String>,
) -> DeactivationPlan {
    let adjacency = reference_adjacency(graph.edges());
    let (activatable, kind_filtered) = referenced_artifacts(graph, target_urn);
    let candidates: BTreeSet<String> = activatable
        .iter()
        .filter(|record| scope.selects(record.kind))
        .map(|record| record.urn.clone())
        .collect();

    let remaining: Vec<String> = active_urns
        .iter()
        .filter(|urn| urn.as_str() != target_urn)
        .cloned()
        .collect();
    let reachable_by_source: BTreeMap<&str, BTreeSet<String>> = remaining
        .iter()
        .map(|source| {
            (
                source.as_str(),
                forward_closure(
                    &adjacency,
                    &BTreeSet::from([source.clone()]),
                    &BTreeSet::new(),
                ),
            )
        })
        .collect();

    let mut deactivate = Vec::new();
    let mut skipped_shared = Vec::new();
    for candidate in candidates {
        let referencing: Vec<&str> = reachable_by_source
            .iter()
            .filter(|(_, reached)| reached.contains(&candidate))
            .map(|(source, _)| *source)
            .collect();
        if let Some(source) = referencing.first() {
            skipped_shared.push(SharedSkip {
                urn: candidate,
                referencing_active_urn: (*source).to_owned(),
            });
        } else {
            deactivate.push(candidate);
        }
    }
    deactivate.sort();
    skipped_shared.sort_by(|left, right| left.urn.cmp(&right.urn));
    DeactivationPlan {
        deactivate,
        skipped_shared,
        not_cascaded_kind_filtered: bucket_ids(&kind_filtered),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{DrgEdge, DrgGraph, DrgNode, NodeKind, Relation};

    fn node(urn: &str, kind: NodeKind) -> DrgNode {
        DrgNode::new(urn, kind, None, Vec::new()).unwrap()
    }

    fn edge(source: &str, target: &str, relation: Relation) -> DrgEdge {
        DrgEdge::new(source, target, relation).unwrap()
    }

    fn graph() -> DrgGraph {
        DrgGraph::new(
            "1.0",
            "x",
            "t",
            vec![
                node("agent_profile:root", NodeKind::AgentProfile),
                node("agent_profile:other", NodeKind::AgentProfile),
                node("directive:shared", NodeKind::Directive),
                node("tactic:exclusive", NodeKind::Tactic),
                node("template:t", NodeKind::Template),
                node("directive:denied", NodeKind::Directive),
                node("directive:behind", NodeKind::Directive),
            ],
            vec![
                edge("agent_profile:root", "directive:shared", Relation::Requires),
                edge("agent_profile:root", "tactic:exclusive", Relation::Requires),
                edge("agent_profile:root", "directive:denied", Relation::Suggests),
                edge("agent_profile:root", "template:t", Relation::Instantiates),
                edge(
                    "agent_profile:other",
                    "directive:shared",
                    Relation::Requires,
                ),
                edge("directive:denied", "directive:behind", Relation::Requires),
            ],
        )
        .unwrap()
    }

    #[test]
    fn scope_parsing_is_explicit_and_fail_closed() {
        assert!(CascadeScope::parse(None).unwrap().is_none());
        assert!(CascadeScope::parse(Some("  ")).unwrap().is_none());
        assert!(CascadeScope::parse(Some("all")).unwrap().unwrap().is_all());
        let scope = CascadeScope::parse(Some("agent-profile,tactic"))
            .unwrap()
            .unwrap();
        assert!(scope.selects(ArtifactKind::AgentProfile));
        assert!(scope.selects(ArtifactKind::Tactic));
        assert!(!scope.selects(ArtifactKind::Directive));
        let error = CascadeScope::parse(Some("mission-type")).unwrap_err();
        assert!(error.to_string().contains("mission-type"));
    }

    #[test]
    fn cascade_activation_reports_every_bucket() {
        let denied = BTreeSet::from(["directive:denied".to_owned()]);
        let result = cascade_activation_targets(
            &graph(),
            "agent_profile:root",
            &CascadeScope::all(),
            &denied,
            false,
        );
        assert!(result.activated["directive"].contains(&"shared".to_owned()));
        assert_eq!(
            result.not_cascaded_kind_filtered["template"],
            vec!["t".to_owned()]
        );
        assert_eq!(
            result
                .skipped_deactivated
                .get("directive")
                .map(|records| records.iter().map(|r| r.urn.as_str()).collect::<Vec<_>>()),
            Some(vec!["directive:denied"])
        );
        assert_eq!(
            result.unreached_behind_deactivated["directive"],
            vec!["behind".to_owned()]
        );

        let scoped = cascade_activation_targets(
            &graph(),
            "agent_profile:root",
            &CascadeScope::kinds([ArtifactKind::Tactic]).unwrap(),
            &BTreeSet::new(),
            false,
        );
        assert!(scoped.activated.contains_key("tactic"));
        assert!(scoped.skipped_by_scope["directive"].contains(&"shared".to_owned()));
    }

    #[test]
    fn deactivation_keeps_shared_references() {
        let active = BTreeSet::from([
            "agent_profile:root".to_owned(),
            "agent_profile:other".to_owned(),
        ]);
        let plan = deactivation_plan(
            &graph(),
            "agent_profile:root",
            &CascadeScope::all(),
            &active,
        );
        assert_eq!(
            plan.deactivate,
            vec![
                "directive:behind".to_owned(),
                "directive:denied".to_owned(),
                "tactic:exclusive".to_owned()
            ]
        );
        assert_eq!(
            plan.skipped_shared,
            vec![SharedSkip {
                urn: "directive:shared".to_owned(),
                referencing_active_urn: "agent_profile:other".to_owned(),
            }]
        );
        assert!(plan.not_cascaded_kind_filtered.contains_key("template"));
    }

    #[test]
    fn floor_netted_state_never_stops_a_cascade_at_a_required_artifact() {
        use crate::activation::ActivationState;
        use crate::identity::IdentityIndex;
        use sk_charter::ArtifactKind;

        let directory = tempfile::tempdir().unwrap();
        let built_in = directory.path().join("built-in");
        std::fs::create_dir_all(built_in.join("directives")).unwrap();
        std::fs::write(
            built_in.join("directives/001-arch.directive.yaml"),
            "id: DIRECTIVE_001\n",
        )
        .unwrap();
        std::fs::write(
            built_in.join("directives/002-style.directive.yaml"),
            "id: DIRECTIVE_002\n",
        )
        .unwrap();
        let mut identity = IdentityIndex::from_roots(&built_in, &[], None);
        crate::identity::scan_all(&mut identity);
        let mut ledger = crate::ledger::ActivationLedger::default();
        // A stale deny of an org-required directive.
        ledger
            .deny
            .insert(ArtifactKind::Directive, vec!["001-arch".to_owned()]);
        let floor = BTreeMap::from([(ArtifactKind::Directive, vec!["DIRECTIVE_001".to_owned()])]);
        let state = ActivationState::from_ledger(&ledger, &identity, &floor).unwrap();
        assert!(state.deactivated_urns().is_empty());

        let graph = DrgGraph::new(
            "1.0",
            "x",
            "t",
            vec![
                node("agent_profile:root", NodeKind::AgentProfile),
                node("directive:DIRECTIVE_001", NodeKind::Directive),
            ],
            vec![edge(
                "agent_profile:root",
                "directive:DIRECTIVE_001",
                Relation::Requires,
            )],
        )
        .unwrap();
        let result = cascade_activation_targets_for_state(
            &graph,
            "agent_profile:root",
            &CascadeScope::all(),
            &state,
            false,
        );
        assert!(
            result.activated["directive"].contains(&"DIRECTIVE_001".to_owned()),
            "{result:?}"
        );
        assert!(!result.skipped_deactivated.contains_key("directive"));
    }

    #[test]
    fn no_cascade_report_distinguishes_reasons() {
        let report = referenced_but_not_cascaded(
            &graph(),
            "agent_profile:root",
            &BTreeSet::from(["directive:denied".to_owned()]),
        );
        assert!(report.has_skipped());
        assert!(report.skipped.contains_key("directive"));
        assert!(report.not_cascaded_kind_filtered.contains_key("template"));
        assert!(report.deactivated.contains_key("directive"));
    }
}
