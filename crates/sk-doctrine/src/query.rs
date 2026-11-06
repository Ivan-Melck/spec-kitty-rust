//! Pure DRG traversal primitives.
//!
//! Mirrors `src/charter/offering/drg/query.py` and
//! `src/charter/offering/drg/reachability.py`: generic BFS, the canonical
//! action-context resolution algorithm, transitive reference bucketing, and the
//! two per-channel reachability measures.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::graph::{DrgGraph, NodeKind, Relation};

/// BFS traversal following only edges whose relation is in `relations`.
///
/// `max_depth` is the maximum number of hops from any start node; `None` walks
/// to exhaustion. Start URNs are returned even when the graph does not contain
/// them, matching the reference implementation.
pub fn walk_edges(
    graph: &DrgGraph,
    start_urns: &BTreeSet<String>,
    relations: &BTreeSet<Relation>,
    max_depth: Option<usize>,
) -> BTreeSet<String> {
    if start_urns.is_empty() {
        return BTreeSet::new();
    }
    let mut adjacency: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for edge in graph.edges() {
        if relations.contains(&edge.relation()) {
            adjacency
                .entry(edge.source())
                .or_default()
                .push(edge.target());
        }
    }

    let mut visited: BTreeSet<String> = start_urns.clone();
    let mut queue: VecDeque<(String, usize)> =
        start_urns.iter().map(|urn| (urn.clone(), 0)).collect();
    while let Some((current, depth)) = queue.pop_front() {
        if max_depth.is_some_and(|maximum| depth >= maximum) {
            continue;
        }
        for neighbour in adjacency.get(current.as_str()).into_iter().flatten() {
            if visited.insert((*neighbour).to_owned()) {
                queue.push_back(((*neighbour).to_owned(), depth + 1));
            }
        }
    }
    visited
}

/// The resolved governance context for one action node.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResolvedContext {
    /// Every artifact URN reachable through scope → requires → suggests.
    pub artifact_urns: BTreeSet<String>,
    /// Glossary scope URNs reached by `vocabulary` edges from resolved artifacts.
    pub glossary_scopes: BTreeSet<String>,
    /// Co-delivered tension pairs mapped to reconcilers, sorted.
    pub tension_arbiters: Vec<(String, Vec<String>)>,
    /// Co-delivered tension pairs with no reconciler, sorted.
    pub unarbitrated_tensions: Vec<(String, String)>,
}

/// The two delivery cadences of one action resolution.
///
/// `eager` artifacts are scoped directly or reached through `requires`; they
/// are delivered inline. `suggested` artifacts are reached through `suggests`
/// and are delivered as links (or inline under `include_all`).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ActionDelivery {
    /// Scope plus the transitive `requires` closure.
    pub eager: BTreeSet<String>,
    /// Artifacts reached by `suggests` up to the requested depth.
    pub suggested: BTreeSet<String>,
    /// `eager ∪ suggested`.
    pub all_artifacts: BTreeSet<String>,
    /// Glossary scopes reached by `vocabulary` edges.
    pub glossary_scopes: BTreeSet<String>,
    /// Co-delivered tension pairs mapped to reconcilers.
    pub tension_arbiters: Vec<(String, Vec<String>)>,
    /// Co-delivered tension pairs with no reconciler.
    pub unarbitrated_tensions: Vec<(String, String)>,
}

/// Resolve one action into its eager and suggested delivery sets.
///
/// The algorithm is the single canonical action walk:
///
/// 1. `scope` edges from the action (depth 1);
/// 2. `requires` transitively from the scoped artifacts (eager);
/// 3. `suggests` from the scoped artifacts up to `depth` hops;
/// 4. `vocabulary` edges (depth 1) from every resolved artifact;
/// 5. tension annotations over co-delivered `in_tension_with` pairs.
pub fn deliver_action_context(graph: &DrgGraph, action_urn: &str, depth: usize) -> ActionDelivery {
    let scope = once(Relation::Scope);
    let scoped_walk = walk_edges(
        graph,
        &BTreeSet::from([action_urn.to_owned()]),
        &scope,
        Some(1),
    );
    let scoped_artifacts: BTreeSet<String> = scoped_walk
        .into_iter()
        .filter(|urn| urn != action_urn)
        .collect();

    let required = walk_edges(graph, &scoped_artifacts, &once(Relation::Requires), None);
    let suggested = walk_edges(
        graph,
        &scoped_artifacts,
        &once(Relation::Suggests),
        Some(depth),
    );

    let mut eager = scoped_artifacts;
    eager.extend(required);
    let mut all_artifacts = eager.clone();
    all_artifacts.extend(suggested.clone());

    let vocabulary = walk_edges(graph, &all_artifacts, &once(Relation::Vocabulary), Some(1));
    let glossary_scopes: BTreeSet<String> = vocabulary
        .into_iter()
        .filter(|urn| !all_artifacts.contains(urn))
        .collect();

    let (tension_arbiters, unarbitrated_tensions) = tension_annotations(graph, &all_artifacts);

    ActionDelivery {
        eager,
        suggested,
        all_artifacts,
        glossary_scopes,
        tension_arbiters,
        unarbitrated_tensions,
    }
}

/// Resolve governance context for an action by walking the graph.
///
/// This is [`deliver_action_context`] collapsed back into its single resolved
/// set, retaining the frozen [`ResolvedContext`] shape.
pub fn resolve_context(graph: &DrgGraph, action_urn: &str, depth: usize) -> ResolvedContext {
    let delivery = deliver_action_context(graph, action_urn, depth);
    ResolvedContext {
        artifact_urns: delivery.all_artifacts,
        glossary_scopes: delivery.glossary_scopes,
        tension_arbiters: delivery.tension_arbiters,
        unarbitrated_tensions: delivery.unarbitrated_tensions,
    }
}

fn once(relation: Relation) -> BTreeSet<Relation> {
    BTreeSet::from([relation])
}

type TensionAnnotations = (Vec<(String, Vec<String>)>, Vec<(String, String)>);

fn tension_annotations(graph: &DrgGraph, all_artifacts: &BTreeSet<String>) -> TensionAnnotations {
    let pairs: BTreeSet<(String, String)> = graph
        .edges()
        .iter()
        .filter(|edge| edge.relation() == Relation::InTensionWith)
        .filter(|edge| {
            all_artifacts.contains(edge.source()) && all_artifacts.contains(edge.target())
        })
        .map(|edge| (edge.source().to_owned(), edge.target().to_owned()))
        .collect();
    if pairs.is_empty() {
        return (Vec::new(), Vec::new());
    }

    let mut reconcilers_by_target: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for edge in graph.edges() {
        if edge.relation() == Relation::ReconcilesTension {
            reconcilers_by_target
                .entry(edge.target())
                .or_default()
                .insert(edge.source());
        }
    }

    let mut arbiters: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut unarbitrated = BTreeSet::new();
    for (source, target) in pairs {
        let source_reconcilers = reconcilers_by_target
            .get(source.as_str())
            .cloned()
            .unwrap_or_default();
        let target_reconcilers = reconcilers_by_target
            .get(target.as_str())
            .cloned()
            .unwrap_or_default();
        let common: BTreeSet<&str> = source_reconcilers
            .intersection(&target_reconcilers)
            .copied()
            .collect();
        if common.is_empty() {
            unarbitrated.insert((source, target));
            continue;
        }
        for arbiter in common {
            let entry = arbiters.entry(arbiter.to_owned()).or_default();
            entry.insert(source.clone());
            entry.insert(target.clone());
        }
    }
    let tension_arbiters = arbiters
        .into_iter()
        .map(|(arbiter, arbitrated)| (arbiter, arbitrated.into_iter().collect()))
        .collect();
    (tension_arbiters, unarbitrated.into_iter().collect())
}

/// Transitive reference closure bucketed by every node kind.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResolvedRefs {
    /// Bare ids bucketed by every kind; a visited kind is never dropped.
    pub by_kind: BTreeMap<NodeKind, Vec<String>>,
    /// `(urn, urn)` for start/visited URNs with no node, sorted.
    pub unresolved: Vec<(String, String)>,
}

impl ResolvedRefs {
    /// Whether every referenced artifact resolved to a node.
    pub fn is_complete(&self) -> bool {
        self.unresolved.is_empty()
    }

    /// Bare ids of `kind`, sorted.
    pub fn ids(&self, kind: NodeKind) -> &[String] {
        self.by_kind.get(&kind).map(Vec::as_slice).unwrap_or(&[])
    }
}

/// Walk `relations` from `start_urns`, bucketing reachable nodes by kind.
///
/// Every [`NodeKind`] is represented in [`ResolvedRefs::by_kind`], so no kind
/// silently falls out of the result. Unknown start URNs are recorded in
/// [`ResolvedRefs::unresolved`] rather than raising, matching the frozen
/// reference contract.
pub fn resolve_transitive_refs(
    graph: &DrgGraph,
    start_urns: &BTreeSet<String>,
    relations: &BTreeSet<Relation>,
    max_depth: Option<usize>,
) -> ResolvedRefs {
    let visited = walk_edges(graph, start_urns, relations, max_depth);
    let mut by_kind: BTreeMap<NodeKind, Vec<String>> = NodeKind::ALL
        .iter()
        .map(|kind| (*kind, Vec::new()))
        .collect();
    let mut unresolved = Vec::new();
    for urn in visited {
        let Some(node) = graph.node(&urn) else {
            unresolved.push((urn.clone(), urn));
            continue;
        };
        let bare = urn.split_once(':').map(|(_, body)| body).unwrap_or(&urn);
        by_kind
            .entry(node.kind())
            .or_default()
            .push(bare.to_owned());
    }
    for ids in by_kind.values_mut() {
        ids.sort();
    }
    unresolved.sort();
    ResolvedRefs {
        by_kind,
        unresolved,
    }
}

/// Every `action` node URN: the action-channel seed set.
pub fn action_seed_urns(graph: &DrgGraph) -> BTreeSet<String> {
    graph
        .nodes()
        .iter()
        .filter(|node| node.kind() == NodeKind::Action)
        .map(|node| node.urn().to_owned())
        .collect()
}

/// Every `agent_profile` node URN: the profile-channel seed set.
pub fn agent_profile_seed_urns(graph: &DrgGraph) -> BTreeSet<String> {
    graph
        .nodes()
        .iter()
        .filter(|node| node.kind() == NodeKind::AgentProfile)
        .map(|node| node.urn().to_owned())
        .collect()
}

/// The relations the profile entry vector follows.
///
/// Deliberately excludes `scope`: profiles carry zero outbound `scope` edges, so
/// folding this channel into [`resolve_context`] would silently measure zero.
pub fn profile_channel_relations() -> BTreeSet<Relation> {
    BTreeSet::from([
        Relation::Requires,
        Relation::SpecializesFrom,
        Relation::Suggests,
    ])
}

/// Artefact URNs reachable through the action channel at `depth`.
pub fn action_channel_reachable(
    graph: &DrgGraph,
    seeds: &BTreeSet<String>,
    depth: usize,
) -> BTreeSet<String> {
    let mut reached = BTreeSet::new();
    for seed in seeds {
        reached.extend(resolve_context(graph, seed, depth).artifact_urns);
    }
    reached
}

/// Artefact URNs reachable through the profile channel.
///
/// Fail-closed: an empty seed set reaches nothing rather than the whole graph.
pub fn profile_channel_reachable(graph: &DrgGraph, seeds: &BTreeSet<String>) -> BTreeSet<String> {
    if seeds.is_empty() {
        return BTreeSet::new();
    }
    walk_edges(graph, seeds, &profile_channel_relations(), None)
        .difference(seeds)
        .cloned()
        .collect()
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

    fn fixtures() -> DrgGraph {
        DrgGraph::new(
            "1.0",
            "2026-01-01T00:00:00Z",
            "test",
            vec![
                node("action:software-dev/implement", NodeKind::Action),
                node("directive:scope", NodeKind::Directive),
                node("tactic:hard", NodeKind::Tactic),
                node("tactic:soft-1", NodeKind::Tactic),
                node("tactic:soft-2", NodeKind::Tactic),
                node("glossary_scope:terms", NodeKind::GlossaryScope),
                node("directive:a", NodeKind::Directive),
                node("directive:b", NodeKind::Directive),
                node("agent_profile:reconciler", NodeKind::AgentProfile),
                node("anti_pattern:smell", NodeKind::AntiPattern),
            ],
            vec![
                edge(
                    "action:software-dev/implement",
                    "directive:scope",
                    Relation::Scope,
                ),
                edge("directive:scope", "tactic:hard", Relation::Requires),
                edge("directive:scope", "tactic:soft-1", Relation::Suggests),
                edge("tactic:soft-1", "tactic:soft-2", Relation::Suggests),
                edge(
                    "directive:scope",
                    "glossary_scope:terms",
                    Relation::Vocabulary,
                ),
                edge("directive:a", "directive:b", Relation::InTensionWith),
                edge(
                    "agent_profile:reconciler",
                    "directive:a",
                    Relation::ReconcilesTension,
                ),
                edge(
                    "agent_profile:reconciler",
                    "directive:b",
                    Relation::ReconcilesTension,
                ),
                edge(
                    "agent_profile:reconciler",
                    "tactic:hard",
                    Relation::Requires,
                ),
                edge(
                    "agent_profile:reconciler",
                    "anti_pattern:smell",
                    Relation::Rejects,
                ),
            ],
        )
        .unwrap()
    }

    #[test]
    fn context_walks_scope_requires_and_bounded_suggests() {
        let graph = fixtures();
        let shallow = resolve_context(&graph, "action:software-dev/implement", 1);
        assert!(shallow.artifact_urns.contains("directive:scope"));
        assert!(shallow.artifact_urns.contains("tactic:hard"));
        assert!(shallow.artifact_urns.contains("tactic:soft-1"));
        assert!(!shallow.artifact_urns.contains("tactic:soft-2"));
        assert!(shallow.glossary_scopes.contains("glossary_scope:terms"));
        assert!(!shallow.artifact_urns.contains("directive:a"));

        let deep = resolve_context(&graph, "action:software-dev/implement", 2);
        assert!(deep.artifact_urns.contains("tactic:soft-2"));
        assert!(deep.tension_arbiters.is_empty());
    }

    #[test]
    fn unknown_action_resolves_to_empty_not_everything() {
        let context = resolve_context(&fixtures(), "action:missing", 2);
        assert!(context.artifact_urns.is_empty());
        assert!(context.glossary_scopes.is_empty());
    }

    #[test]
    fn tension_pairs_are_annotated_only_when_co_delivered() {
        let mut edges = vec![
            edge("action:x", "directive:a", Relation::Scope),
            edge("action:x", "directive:b", Relation::Scope),
            edge("directive:a", "directive:b", Relation::InTensionWith),
            edge(
                "agent_profile:r",
                "directive:a",
                Relation::ReconcilesTension,
            ),
            edge(
                "agent_profile:r",
                "directive:b",
                Relation::ReconcilesTension,
            ),
        ];
        let mut nodes = vec![
            node("action:x", NodeKind::Action),
            node("directive:a", NodeKind::Directive),
            node("directive:b", NodeKind::Directive),
            node("agent_profile:r", NodeKind::AgentProfile),
        ];
        let graph = DrgGraph::new("1.0", "x", "t", nodes.clone(), edges.clone()).unwrap();
        let context = resolve_context(&graph, "action:x", 1);
        assert_eq!(
            context.tension_arbiters,
            vec![(
                "agent_profile:r".to_owned(),
                vec!["directive:a".to_owned(), "directive:b".to_owned()]
            )]
        );
        assert!(context.unarbitrated_tensions.is_empty());

        edges.pop();
        nodes.pop();
        let unarbitrated = DrgGraph::new("1.0", "x", "t", nodes, edges).unwrap();
        let context = resolve_context(&unarbitrated, "action:x", 1);
        assert_eq!(
            context.unarbitrated_tensions,
            vec![("directive:a".to_owned(), "directive:b".to_owned())]
        );
    }

    #[test]
    fn transitive_refs_bucket_every_kind_and_record_unresolved() {
        let refs = resolve_transitive_refs(
            &fixtures(),
            &BTreeSet::from(["agent_profile:reconciler".to_owned()]),
            &profile_channel_relations(),
            None,
        );
        assert!(refs.ids(NodeKind::Tactic).contains(&"hard".to_owned()));
        assert!(refs.by_kind.contains_key(&NodeKind::AntiPattern));
        assert!(
            !refs
                .ids(NodeKind::AntiPattern)
                .contains(&"smell".to_owned())
        );
        assert!(refs.is_complete());
        for kind in NodeKind::ALL {
            assert!(refs.by_kind.contains_key(&kind));
        }

        let missing = resolve_transitive_refs(
            &fixtures(),
            &BTreeSet::from(["directive:absent".to_owned()]),
            &once(Relation::Requires),
            None,
        );
        assert_eq!(
            missing.unresolved,
            vec![("directive:absent".to_owned(), "directive:absent".to_owned())]
        );
    }

    #[test]
    fn channels_are_distinct_and_fail_closed() {
        let graph = fixtures();
        let actions = action_seed_urns(&graph);
        assert!(action_channel_reachable(&graph, &actions, 1).contains("tactic:hard"));
        let profiles = agent_profile_seed_urns(&graph);
        assert!(profile_channel_reachable(&graph, &profiles).contains("tactic:hard"));
        assert!(profile_channel_reachable(&graph, &BTreeSet::new()).is_empty());
    }
}
