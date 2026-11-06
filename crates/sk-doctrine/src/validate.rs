//! Graph-level integrity validation beyond per-field URN checks.
//!
//! Mirrors `src/charter/offering/drg/validator.py`: dangling endpoints,
//! duplicate triples, `requires` cycles, symmetric profile-edge integrity,
//! `specializes_from` acyclicity, and rejects/anti-pattern integrity in both
//! directions.

use std::collections::{BTreeMap, BTreeSet};

use crate::error::DoctrineError;
use crate::graph::{DrgEdge, DrgGraph, NodeKind, Relation};

const PROFILE_EDGE_RELATIONS: [Relation; 2] = [Relation::SpecializesFrom, Relation::DelegatesTo];

/// Return every edge with a source and/or target absent from the node set.
pub fn dangling_endpoints(graph: &DrgGraph) -> Vec<&DrgEdge> {
    let urns = graph.node_urns();
    graph
        .edges()
        .iter()
        .filter(|edge| !urns.contains(edge.source()) || !urns.contains(edge.target()))
        .collect()
}

/// Return each edge that repeats an already-seen `(source, target, relation)`.
pub fn duplicate_edge_triples(graph: &DrgGraph) -> Vec<&DrgEdge> {
    let mut seen: BTreeSet<(&str, &str, Relation)> = BTreeSet::new();
    graph
        .edges()
        .iter()
        .filter(|edge| !seen.insert((edge.source(), edge.target(), edge.relation())))
        .collect()
}

/// Return every graph-integrity error. An empty vector means the graph is valid.
pub fn validate_graph(graph: &DrgGraph) -> Vec<String> {
    let mut errors = Vec::new();
    errors.extend(validate_dangling_references(graph));
    errors.extend(validate_duplicate_edges(graph));
    errors.extend(validate_requires_cycles(graph));
    errors.extend(validate_profile_edges(graph));
    errors.extend(validate_rejects_targets(graph));
    errors.extend(validate_anti_pattern_nodes_are_rejected(graph));
    errors
}

/// Fail closed when the graph has integrity errors.
pub fn assert_valid(graph: &DrgGraph) -> Result<(), DoctrineError> {
    let errors = validate_graph(graph);
    if errors.is_empty() {
        return Ok(());
    }
    Err(DoctrineError::InvalidGraph(format!(
        "{} error(s): {}",
        errors.len(),
        errors.join("; ")
    )))
}

pub(crate) fn validate_dangling_references(graph: &DrgGraph) -> Vec<String> {
    let urns = graph.node_urns();
    let mut errors = Vec::new();
    for edge in dangling_endpoints(graph) {
        if !urns.contains(edge.source()) {
            errors.push(format!(
                "Dangling source: edge ({} --{}--> {}) references non-existent node '{}'",
                edge.source(),
                edge.relation(),
                edge.target(),
                edge.source()
            ));
        }
        if !urns.contains(edge.target()) {
            errors.push(format!(
                "Dangling target: edge ({} --{}--> {}) references non-existent node '{}'",
                edge.source(),
                edge.relation(),
                edge.target(),
                edge.target()
            ));
        }
    }
    errors
}

fn validate_duplicate_edges(graph: &DrgGraph) -> Vec<String> {
    duplicate_edge_triples(graph)
        .into_iter()
        .map(|edge| {
            format!(
                "Duplicate edge: ({} --{}--> {})",
                edge.source(),
                edge.relation(),
                edge.target()
            )
        })
        .collect()
}

fn validate_requires_cycles(graph: &DrgGraph) -> Vec<String> {
    let mut adjacency: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for edge in graph.edges() {
        if edge.relation() == Relation::Requires {
            adjacency
                .entry(edge.source())
                .or_default()
                .push(edge.target());
        }
    }
    cycle_errors(&adjacency, "requires")
}

fn validate_profile_edges(graph: &DrgGraph) -> Vec<String> {
    let mut errors = Vec::new();
    let kinds: BTreeMap<&str, NodeKind> = graph
        .nodes()
        .iter()
        .map(|node| (node.urn(), node.kind()))
        .collect();
    for edge in graph.edges() {
        if !PROFILE_EDGE_RELATIONS.contains(&edge.relation()) {
            continue;
        }
        for (endpoint_name, urn) in [("source", edge.source()), ("target", edge.target())] {
            let Some(kind) = kinds.get(urn) else {
                continue;
            };
            if *kind != NodeKind::AgentProfile {
                errors.push(format!(
                    "Profile-edge {endpoint_name} must be an agent_profile: \
                     edge ({} --{}--> {}) has {endpoint_name} '{urn}' of kind '{}'",
                    edge.source(),
                    edge.relation(),
                    edge.target(),
                    kind.as_str()
                ));
            }
        }
    }

    let mut lineage: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for edge in graph.edges() {
        if edge.relation() == Relation::SpecializesFrom {
            lineage
                .entry(edge.source())
                .or_default()
                .push(edge.target());
        }
    }
    errors.extend(cycle_errors(&lineage, "specializes_from lineage"));
    errors
}

fn validate_rejects_targets(graph: &DrgGraph) -> Vec<String> {
    let kinds: BTreeMap<&str, NodeKind> = graph
        .nodes()
        .iter()
        .map(|node| (node.urn(), node.kind()))
        .collect();
    let mut errors = Vec::new();
    for edge in graph.edges() {
        if edge.relation() != Relation::Rejects {
            continue;
        }
        let Some(kind) = kinds.get(edge.target()) else {
            continue;
        };
        if *kind != NodeKind::AntiPattern {
            errors.push(format!(
                "Rejects-edge target must be an anti_pattern node: \
                 edge ({} --{}--> {}) has target '{}' of kind '{}'",
                edge.source(),
                edge.relation(),
                edge.target(),
                edge.target(),
                kind
            ));
        }
    }
    errors
}

fn validate_anti_pattern_nodes_are_rejected(graph: &DrgGraph) -> Vec<String> {
    let mut errors = Vec::new();
    for node in graph.nodes() {
        if node.kind() != NodeKind::AntiPattern {
            continue;
        }
        if graph
            .edges_to(node.urn(), Some(Relation::Rejects))
            .is_empty()
        {
            errors.push(format!(
                "Orphaned anti_pattern node: '{}' is marked anti_pattern but has no inbound rejects edge",
                node.urn()
            ));
        }
    }
    errors
}

/// Iterative DFS cycle detection over a relation subgraph.
///
/// Cycles are reported once per back edge, in sorted start-node order, so the
/// diagnostics are deterministic and no recursion depth is attacker-controlled.
fn cycle_errors(adjacency: &BTreeMap<&str, Vec<&str>>, label: &str) -> Vec<String> {
    #[derive(Clone, Copy, PartialEq)]
    enum Colour {
        White,
        Gray,
        Black,
    }

    let mut nodes: BTreeSet<&str> = adjacency.keys().copied().collect();
    for targets in adjacency.values() {
        nodes.extend(targets.iter().copied());
    }
    let mut colours: BTreeMap<&str, Colour> =
        nodes.iter().map(|node| (*node, Colour::White)).collect();
    let mut errors = Vec::new();

    for start in nodes {
        if colours.get(start) != Some(&Colour::White) {
            continue;
        }
        let mut path: Vec<&str> = vec![start];
        let mut stack: Vec<(&str, usize)> = vec![(start, 0)];
        colours.insert(start, Colour::Gray);
        while let Some((node, next_child)) = stack.last_mut() {
            let neighbours = adjacency.get(*node).map(Vec::as_slice).unwrap_or(&[]);
            if *next_child >= neighbours.len() {
                colours.insert(*node, Colour::Black);
                stack.pop();
                path.pop();
                continue;
            }
            let neighbour = neighbours[*next_child];
            *next_child += 1;
            match colours.get(neighbour) {
                Some(Colour::Gray) => {
                    if let Some(position) = path.iter().position(|entry| *entry == neighbour) {
                        let mut cycle: Vec<&str> = path[position..].to_vec();
                        cycle.push(neighbour);
                        errors.push(format!("Cycle in {label}: {}", cycle.join(" -> ")));
                    }
                }
                Some(Colour::White) => {
                    colours.insert(neighbour, Colour::Gray);
                    path.push(neighbour);
                    stack.push((neighbour, 0));
                }
                _ => {}
            }
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{DrgEdge, DrgGraph, DrgNode, NodeKind, Relation};
    use std::path::PathBuf;

    fn graph(nodes: Vec<DrgNode>, edges: Vec<DrgEdge>) -> DrgGraph {
        DrgGraph::new("1.0", "2026-01-01T00:00:00Z", "test", nodes, edges).expect("graph")
    }

    fn node(urn: &str, kind: NodeKind) -> DrgNode {
        DrgNode::new(urn, kind, None, Vec::new()).expect("node")
    }

    #[test]
    fn dangling_and_duplicate_are_reported() {
        let graph = graph(
            vec![node("directive:a", NodeKind::Directive)],
            vec![
                DrgEdge::new("directive:a", "directive:missing", Relation::Requires).unwrap(),
                DrgEdge::new("directive:a", "directive:missing", Relation::Requires).unwrap(),
            ],
        );
        let errors = validate_graph(&graph);
        assert!(errors.iter().any(|error| error.contains("Dangling target")));
        assert!(errors.iter().any(|error| error.contains("Duplicate edge")));
        assert!(assert_valid(&graph).is_err());
    }

    #[test]
    fn requires_cycle_is_reported() {
        let graph = graph(
            vec![
                node("directive:a", NodeKind::Directive),
                node("directive:b", NodeKind::Directive),
            ],
            vec![
                DrgEdge::new("directive:a", "directive:b", Relation::Requires).unwrap(),
                DrgEdge::new("directive:b", "directive:a", Relation::Requires).unwrap(),
            ],
        );
        let errors = validate_graph(&graph);
        assert!(
            errors
                .iter()
                .any(|error| error.contains("Cycle in requires"))
        );
    }

    #[test]
    fn lineage_must_connect_profiles_and_be_acyclic() {
        let graph = graph(
            vec![
                node("agent_profile:a", NodeKind::AgentProfile),
                node("agent_profile:b", NodeKind::AgentProfile),
                node("tactic:t", NodeKind::Tactic),
                node("anti_pattern:x", NodeKind::AntiPattern),
            ],
            vec![
                DrgEdge::new("agent_profile:a", "tactic:t", Relation::SpecializesFrom).unwrap(),
                DrgEdge::new(
                    "agent_profile:a",
                    "agent_profile:b",
                    Relation::SpecializesFrom,
                )
                .unwrap(),
                DrgEdge::new(
                    "agent_profile:b",
                    "agent_profile:a",
                    Relation::SpecializesFrom,
                )
                .unwrap(),
                DrgEdge::new("agent_profile:a", "anti_pattern:x", Relation::Rejects).unwrap(),
            ],
        );
        let errors = validate_graph(&graph);
        assert!(
            errors
                .iter()
                .any(|error| error.contains("must be an agent_profile"))
        );
        assert!(
            errors
                .iter()
                .any(|error| error.contains("Cycle in specializes_from lineage"))
        );
        assert!(
            !errors
                .iter()
                .any(|error| error.contains("Orphaned anti_pattern"))
        );
    }

    #[test]
    fn rejects_and_orphans_are_checked_both_ways() {
        let graph = graph(
            vec![
                node("directive:a", NodeKind::Directive),
                node("directive:b", NodeKind::Directive),
                node("anti_pattern:orphan", NodeKind::AntiPattern),
            ],
            vec![DrgEdge::new("directive:a", "directive:b", Relation::Rejects).unwrap()],
        );
        let errors = validate_graph(&graph);
        assert!(
            errors
                .iter()
                .any(|error| error.contains("Rejects-edge target must be an anti_pattern"))
        );
        assert!(
            errors
                .iter()
                .any(|error| error.contains("Orphaned anti_pattern"))
        );
    }

    #[test]
    fn valid_graph_has_no_errors() {
        let graph = graph(
            vec![
                node("directive:a", NodeKind::Directive),
                node("anti_pattern:smell", NodeKind::AntiPattern),
            ],
            vec![DrgEdge::new("directive:a", "anti_pattern:smell", Relation::Rejects).unwrap()],
        );
        assert_eq!(validate_graph(&graph), Vec::<String>::new());
        assert_eq!(
            DrgGraph::new("1.0", "x", "y", Vec::new(), Vec::new())
                .unwrap()
                .schema_version(),
            "1.0"
        );
        let _ = PathBuf::new();
    }
}
