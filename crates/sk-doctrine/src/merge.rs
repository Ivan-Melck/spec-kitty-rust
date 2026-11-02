//! The canonical three-layer DRG merge and topology field-merge.
//!
//! Mirrors `src/charter/offering/drg/merge.py`. Precedence is
//! project > org (later-declared pack wins) > built-in. Invalid or ambiguous
//! endpoints, unknown relation labels, kind-drift overrides, duplicate
//! asset/template URNs and `enhances` overlays that would drop a step or strip
//! its I/O contract all fail closed.

use std::collections::{BTreeMap, BTreeSet};

use serde_yaml_ng::{Mapping, Value};

use crate::error::DoctrineError;
use crate::graph::{DrgEdge, DrgGraph, DrgNode, NodeKind, Relation, is_valid_urn};
use crate::org::{OrgEdge, OrgFragment, singular_urn_kind};

const BUILT_IN: &str = "built-in";
const PROJECT: &str = "project";

/// A typed conflict report for layered overrides.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MergeConflict {
    /// The conflict kind (`node_override`, `edge_override`, `unresolved_edge_endpoint`, ...).
    pub kind: String,
    /// The layers that disagree, first the overridden layer.
    pub conflicting_layers: Vec<String>,
    /// The URN or authored token the conflict is about.
    pub target_id: String,
    /// What the merge did: `org_override`, `project_override` or `hard_fail`.
    pub resolution_applied: String,
}

/// The merged graph plus its visible conflicts and warnings.
#[derive(Clone, Debug)]
pub struct MergeReport {
    /// The merged graph.
    pub graph: DrgGraph,
    /// Node/edge override records.
    pub conflicts: Vec<MergeConflict>,
    /// Non-fatal warnings (an accepted override, a forward reference).
    pub warnings: Vec<String>,
}

/// Merge options.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MergeOptions {
    /// When true, a fully-qualified org endpoint may name a node supplied by a
    /// later layer and is reported as a warning if it remains absent. When
    /// false (the default) an endpoint that names no node in any layer is an
    /// error: the native merge never emits a dangling edge silently.
    pub allow_forward_references: bool,
}

/// Overlay the built-in, org and project layers.
pub fn merge_three_layers(
    built_in: &DrgGraph,
    fragments: &[OrgFragment],
    project: Option<&DrgGraph>,
) -> Result<MergeReport, DoctrineError> {
    merge_three_layers_with_options(built_in, fragments, project, MergeOptions::default())
}

/// Overlay the layers with explicit options.
pub fn merge_three_layers_with_options(
    built_in: &DrgGraph,
    fragments: &[OrgFragment],
    project: Option<&DrgGraph>,
    options: MergeOptions,
) -> Result<MergeReport, DoctrineError> {
    let mut merged_nodes: Vec<DrgNode> = built_in
        .nodes()
        .iter()
        .map(|node| node.clone().with_provenance(Some(BUILT_IN.to_owned())))
        .collect();
    let mut node_index: BTreeMap<String, usize> = merged_nodes
        .iter()
        .enumerate()
        .map(|(position, node)| (node.urn().to_owned(), position))
        .collect();
    let built_in_kinds: BTreeMap<String, NodeKind> = built_in
        .nodes()
        .iter()
        .map(|node| (node.urn().to_owned(), node.kind()))
        .collect();
    let built_in_urns: BTreeSet<String> = built_in
        .node_urns()
        .into_iter()
        .map(str::to_owned)
        .collect();
    let mut merged_edges: Vec<DrgEdge> = built_in
        .edges()
        .iter()
        .map(|edge| edge.clone().with_provenance(Some(BUILT_IN.to_owned())))
        .collect();
    let mut edge_index: BTreeMap<(String, String, Relation), usize> = merged_edges
        .iter()
        .enumerate()
        .map(|(position, edge)| {
            (
                (
                    edge.source().to_owned(),
                    edge.target().to_owned(),
                    edge.relation(),
                ),
                position,
            )
        })
        .collect();
    let mut conflicts = Vec::new();
    let mut warnings = Vec::new();
    let mut org_urns: Vec<DrgNode> = Vec::new();

    for fragment in fragments {
        warnings.extend(fragment.warnings.iter().cloned());
        let marker = format!("org:{}", fragment.pack_name);
        let mut local: BTreeMap<String, String> = BTreeMap::new();
        for node in &fragment.nodes {
            let Some(singular) = singular_urn_kind(&node.kind) else {
                return Err(DoctrineError::InvalidOrgFragment {
                    pack: fragment.pack_name.clone(),
                    path: fragment.pack_root.join(crate::org::ORG_FRAGMENT),
                    message: format!("unmappable node kind {:?}", node.kind),
                });
            };
            let urn = format!("{singular}:{}", node.id);
            if !is_valid_urn(&urn) {
                return Err(DoctrineError::InvalidOrgFragment {
                    pack: fragment.pack_name.clone(),
                    path: fragment.pack_root.join(crate::org::ORG_FRAGMENT),
                    message: format!("node id {:?} does not form a valid URN", node.id),
                });
            }
            local.insert(node.id.clone(), urn.clone());
            let kind = NodeKind::from_prefix(singular)
                .ok_or(DoctrineError::InvalidUrn { value: urn.clone() })?;
            let minted = DrgNode::new(urn.clone(), kind, node.title.clone(), Vec::new())?
                .with_provenance(Some(marker.clone()));
            match node_index.get(&urn).copied() {
                Some(position) if merged_nodes[position].provenance() == Some(marker.as_str()) => {
                    // Same pack declaring one URN twice: the first stays.
                    continue;
                }
                Some(position) if built_in_kinds.contains_key(&urn) => {
                    let built_in_kind = built_in_kinds[&urn];
                    if built_in_kind != kind {
                        return Err(DoctrineError::KindDriftOverride {
                            urn,
                            built_in: built_in_kind,
                            attempted: kind,
                        });
                    }
                    let previous = merged_nodes[position]
                        .provenance()
                        .unwrap_or(BUILT_IN)
                        .to_owned();
                    conflicts.push(MergeConflict {
                        kind: "node_override".to_owned(),
                        conflicting_layers: vec![previous.clone(), marker.clone()],
                        target_id: urn.clone(),
                        resolution_applied: "org_override".to_owned(),
                    });
                    warnings.push(format!(
                        "{marker} overrides {previous} node {urn:?} in place; a per-repo replaceable-builtins policy decides whether this is sanctioned"
                    ));
                    merged_nodes[position] = minted;
                }
                Some(position) => {
                    let previous = merged_nodes[position]
                        .provenance()
                        .unwrap_or("unknown")
                        .to_owned();
                    if previous != marker {
                        conflicts.push(MergeConflict {
                            kind: "node_override".to_owned(),
                            conflicting_layers: vec![previous, marker.clone()],
                            target_id: urn.clone(),
                            resolution_applied: "org_override".to_owned(),
                        });
                        warnings.push(format!(
                            "{marker} overrides node {urn:?} contributed by an earlier org pack (later-declared org pack wins)"
                        ));
                        merged_nodes[position] = minted;
                    }
                }
                None => {
                    node_index.insert(urn.clone(), merged_nodes.len());
                    merged_nodes.push(minted);
                }
            }
            org_urns.push(merged_nodes[node_index[&urn]].clone());
        }

        for edge in &fragment.edges {
            let minted = bridge_org_edge(edge, &local, &built_in_urns, &fragment.pack_name)?;
            let triple = (
                minted.source().to_owned(),
                minted.target().to_owned(),
                minted.relation(),
            );
            match edge_index.get(&triple).copied() {
                Some(position) => {
                    let previous = merged_edges[position]
                        .provenance()
                        .unwrap_or("unknown")
                        .to_owned();
                    if previous != marker {
                        merged_edges[position] = minted;
                        warnings.push(format!(
                            "{marker} restates edge ({} --{}--> {}); the later pack's record wins",
                            triple.0, triple.2, triple.1
                        ));
                    }
                }
                None => {
                    edge_index.insert(triple, merged_edges.len());
                    merged_edges.push(minted);
                }
            }
        }
    }

    if let Some(project) = project {
        for node in project.nodes() {
            if let Some(position) = node_index.get(node.urn()).copied() {
                let previous = merged_nodes[position]
                    .provenance()
                    .unwrap_or("unknown")
                    .to_owned();
                conflicts.push(MergeConflict {
                    kind: "node_override".to_owned(),
                    conflicting_layers: vec![previous.clone(), PROJECT.to_owned()],
                    target_id: node.urn().to_owned(),
                    resolution_applied: "project_override".to_owned(),
                });
                warnings.push(format!(
                    "project doctrine overrides {previous} node {:?}",
                    node.urn()
                ));
                merged_nodes[position] = node.clone().with_provenance(Some(PROJECT.to_owned()));
            } else {
                node_index.insert(node.urn().to_owned(), merged_nodes.len());
                merged_nodes.push(node.clone().with_provenance(Some(PROJECT.to_owned())));
            }
        }
        for edge in project.edges() {
            merged_edges.push(edge.clone().with_provenance(Some(PROJECT.to_owned())));
        }
    }

    // Deferred endpoint existence: a qualified org endpoint may have named a
    // project node. A dangling edge is never emitted under the default options.
    let final_urns: BTreeSet<&str> = merged_nodes.iter().map(DrgNode::urn).collect();
    let mut dangling = Vec::new();
    for edge in &merged_edges {
        if !final_urns.contains(edge.source()) || !final_urns.contains(edge.target()) {
            dangling.push(edge.clone());
        }
    }
    if !dangling.is_empty() {
        let listed = dangling
            .iter()
            .map(|edge| {
                format!(
                    "({} --{}--> {})",
                    edge.source(),
                    edge.relation(),
                    edge.target()
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        if options.allow_forward_references {
            warnings.push(format!(
                "unresolved DRG edge endpoint(s) remain after the merge: {listed}"
            ));
        } else {
            return Err(DoctrineError::UnresolvedEndpoint {
                raw: listed,
                origin: "merged graph".to_owned(),
            });
        }
    }

    let mut candidates: Vec<DrgNode> = built_in.nodes().to_vec();
    candidates.extend(org_urns);
    if let Some(project) = project {
        candidates.extend(project.nodes().iter().cloned());
    }
    if let Some((urn, count)) = duplicate_urns(&candidates).into_iter().next() {
        let kind =
            NodeKind::from_prefix(urn.split_once(':').map(|(prefix, _)| prefix).unwrap_or(""))
                .unwrap_or(NodeKind::Asset);
        return Err(DoctrineError::DuplicateUrn { kind, urn, count });
    }

    let graph = built_in.with_parts(merged_nodes, merged_edges)?;
    Ok(MergeReport {
        graph,
        conflicts,
        warnings,
    })
}

fn duplicate_urns(nodes: &[DrgNode]) -> BTreeMap<String, usize> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for node in nodes {
        if matches!(node.kind(), NodeKind::Asset | NodeKind::Template) {
            *counts.entry(node.urn().to_owned()).or_default() += 1;
        }
    }
    counts.retain(|_, count| *count > 1);
    counts
}

/// Resolve one fragment-authored edge endpoint to a canonical URN.
///
/// One policy for both endpoints:
/// 1. a fragment-local bare id (the nearest scope);
/// 2. a fully-qualified `<kind>:<id>` URN (accepted verbatim);
/// 3. a bare id resolvable against the **built-in** layer by its id half —
///    exactly one match succeeds, more than one is ambiguous.
pub fn resolve_edge_endpoint(
    raw: &str,
    local: &BTreeMap<String, String>,
    built_in_urns: &BTreeSet<String>,
) -> Result<String, DoctrineError> {
    if let Some(urn) = local.get(raw) {
        return Ok(urn.clone());
    }
    if let Some((prefix, _)) = raw.split_once(':')
        && NodeKind::from_prefix(prefix).is_some()
    {
        if is_valid_urn(raw) {
            return Ok(raw.to_owned());
        }
        return Err(DoctrineError::InvalidUrn {
            value: raw.to_owned(),
        });
    }
    let matches: Vec<String> = built_in_urns
        .iter()
        .filter(|urn| urn.split_once(':').map(|(_, body)| body) == Some(raw))
        .cloned()
        .collect();
    match matches.len() {
        1 => Ok(matches[0].clone()),
        0 => Err(DoctrineError::UnresolvedEndpoint {
            raw: raw.to_owned(),
            origin: "org fragment".to_owned(),
        }),
        _ => Err(DoctrineError::AmbiguousEndpoint {
            raw: raw.to_owned(),
            matches,
        }),
    }
}

/// Mint a canonical DRG edge from a fragment edge.
pub fn bridge_org_edge(
    edge: &OrgEdge,
    local: &BTreeMap<String, String>,
    built_in_urns: &BTreeSet<String>,
    pack: &str,
) -> Result<DrgEdge, DoctrineError> {
    let relation =
        Relation::from_alias(&edge.relation).ok_or_else(|| DoctrineError::UnknownRelation {
            relation: edge.relation.clone(),
            origin: format!("org:{pack}"),
        })?;
    let source = resolve_edge_endpoint(&edge.source, local, built_in_urns)
        .map_err(|error| annotate_endpoint(error, pack, &edge.source))?;
    let target = resolve_edge_endpoint(&edge.target, local, built_in_urns)
        .map_err(|error| annotate_endpoint(error, pack, &edge.target))?;
    let minted =
        DrgEdge::new(source, target, relation)?.with_provenance(Some(format!("org:{pack}")));
    Ok(match &edge.reason {
        Some(reason) => minted.with_reason(reason),
        None => minted,
    })
}

fn annotate_endpoint(error: DoctrineError, pack: &str, raw: &str) -> DoctrineError {
    match error {
        DoctrineError::UnresolvedEndpoint { .. } => DoctrineError::UnresolvedEndpoint {
            raw: raw.to_owned(),
            origin: format!("org:{pack}"),
        },
        other => other,
    }
}

/// The field-merge semantics for topology-bearing artifacts (FR-029).
///
/// `overrides` replaces wholesale. `enhances` merges field-by-field while
/// preserving the action-sequence backbone: an overlay may omit the sequence
/// (the base ordering is preserved) or supply it, but every identifiable base
/// step must remain and a step's declared input/output contract may not be
/// stripped.
pub fn merge_topology_artifact(
    base: &Value,
    overlay: &Value,
    mode: Relation,
) -> Result<Value, DoctrineError> {
    if mode == Relation::Overrides {
        return Ok(overlay.clone());
    }
    if mode != Relation::Enhances {
        return Err(DoctrineError::TopologyModeInvalid { relation: mode });
    }
    let base_mapping = base
        .as_mapping()
        .ok_or_else(|| DoctrineError::InvalidGraphDocument {
            path: std::path::PathBuf::from("<topology>"),
            message: "base artifact must be a mapping".to_owned(),
        })?;
    let overlay_mapping =
        overlay
            .as_mapping()
            .ok_or_else(|| DoctrineError::InvalidGraphDocument {
                path: std::path::PathBuf::from("<topology>"),
                message: "overlay artifact must be a mapping".to_owned(),
            })?;
    let mut merged: Mapping = base_mapping.clone();
    for (key, value) in overlay_mapping {
        merged.insert(key.clone(), value.clone());
    }
    for sequence_field in ["steps", "actions"] {
        let Some(base_sequence) = base_mapping.get(sequence_field) else {
            continue;
        };
        let Some(base_sequence) = base_sequence.as_sequence() else {
            continue;
        };
        let Some(overlay_sequence) = overlay_mapping.get(sequence_field) else {
            merged.insert(
                sequence_field.into(),
                Value::Sequence(base_sequence.clone()),
            );
            continue;
        };
        let Some(overlay_sequence) = overlay_sequence.as_sequence() else {
            return Err(DoctrineError::InvalidGraphDocument {
                path: std::path::PathBuf::from("<topology>"),
                message: format!("enhances overlay set {sequence_field:?} to a non-list"),
            });
        };
        let base_by_id: BTreeMap<String, &Value> = base_sequence
            .iter()
            .filter_map(|step| step_id(step).map(|id| (id, step)))
            .collect();
        let overlay_ids: BTreeSet<String> = overlay_sequence.iter().filter_map(step_id).collect();
        let dropped: Vec<String> = base_by_id
            .keys()
            .filter(|id| !overlay_ids.contains(*id))
            .cloned()
            .collect();
        if !dropped.is_empty() {
            return Err(DoctrineError::TopologyStepDropped {
                field: sequence_field.to_owned(),
                dropped,
            });
        }
        let mut merged_sequence = Vec::with_capacity(overlay_sequence.len());
        for step in overlay_sequence {
            merged_sequence.push(merge_step(step, &base_by_id, sequence_field)?);
        }
        merged.insert(sequence_field.into(), Value::Sequence(merged_sequence));
    }
    Ok(Value::Mapping(merged))
}

fn step_id(step: &Value) -> Option<String> {
    let mapping = step.as_mapping()?;
    for key in ["id", "title"] {
        if let Some(value) = mapping.get(key).and_then(Value::as_str)
            && !value.is_empty()
        {
            return Some(value.to_owned());
        }
    }
    None
}

fn merge_step(
    step: &Value,
    base_by_id: &BTreeMap<String, &Value>,
    sequence_field: &str,
) -> Result<Value, DoctrineError> {
    let Some(id) = step_id(step) else {
        return Ok(step.clone());
    };
    let Some(base_step) = base_by_id.get(&id) else {
        return Ok(step.clone());
    };
    let (Some(step_mapping), Some(base_mapping)) = (step.as_mapping(), base_step.as_mapping())
    else {
        return Ok(step.clone());
    };
    let mut merged = base_mapping.clone();
    for (key, value) in step_mapping {
        merged.insert(key.clone(), value.clone());
    }
    for io_field in ["inputs", "outputs"] {
        let base_io = base_mapping.get(io_field);
        let base_has = base_io.is_some_and(|value| match value {
            Value::Sequence(items) => !items.is_empty(),
            Value::Mapping(map) => !map.is_empty(),
            Value::Null => false,
            _ => true,
        });
        if base_has && step_mapping.contains_key(io_field) {
            let overlay_empty = step_mapping.get(io_field).is_some_and(|value| match value {
                Value::Sequence(items) => items.is_empty(),
                Value::Mapping(map) => map.is_empty(),
                Value::Null => true,
                _ => false,
            });
            if overlay_empty {
                return Err(DoctrineError::TopologyIoStripped {
                    field: sequence_field.to_owned(),
                    step: id,
                    io_field: io_field.to_owned(),
                });
            }
        }
    }
    Ok(Value::Mapping(merged))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::org::{OrgEdge, OrgFragment, OrgNode, load_org_pack};
    use std::fs;
    use std::path::PathBuf;

    fn node(urn: &str, kind: NodeKind) -> DrgNode {
        DrgNode::new(urn, kind, None, Vec::new()).unwrap()
    }

    fn built_in() -> DrgGraph {
        DrgGraph::new(
            "1.0",
            "x",
            "built-in",
            vec![
                node("directive:001", NodeKind::Directive),
                node("styleguide:plain", NodeKind::Styleguide),
                node("tactic:hard", NodeKind::Tactic),
            ],
            vec![DrgEdge::new("directive:001", "tactic:hard", Relation::Requires).unwrap()],
        )
        .unwrap()
    }

    fn fragment(nodes: Vec<OrgNode>, edges: Vec<OrgEdge>) -> OrgFragment {
        OrgFragment {
            pack_name: "acme".to_owned(),
            pack_root: PathBuf::from("/tmp/acme"),
            layer_index: 1,
            nodes,
            edges,
            warnings: Vec::new(),
        }
    }

    fn org_node(id: &str, kind: &str) -> OrgNode {
        OrgNode {
            id: id.to_owned(),
            kind: kind.to_owned(),
            title: None,
            body_path: None,
            discovered: false,
        }
    }

    #[test]
    fn org_nodes_get_provenance_and_label_override() {
        let report = merge_three_layers(
            &built_in(),
            &[fragment(vec![org_node("001", "directives")], vec![])],
            None,
        )
        .unwrap();
        let node = report.graph.node("directive:001").expect("node");
        assert_eq!(node.provenance(), Some("org:acme"));
        assert!(
            report
                .conflicts
                .iter()
                .any(|conflict| conflict.resolution_applied == "org_override")
        );
    }

    #[test]
    fn later_org_pack_wins_and_bridge_resolves_bare_endpoints() {
        let mut first = fragment(vec![org_node("x", "directives")], vec![]);
        first.pack_name = "first".to_owned();
        let mut second = fragment(vec![org_node("x", "directives")], vec![]);
        second.pack_name = "second".to_owned();
        second.layer_index = 2;
        let report = merge_three_layers(&built_in(), &[first, second], None).unwrap();
        assert_eq!(
            report.graph.node("directive:x").unwrap().provenance(),
            Some("org:second")
        );

        let report = merge_three_layers(
            &built_in(),
            &[fragment(
                vec![org_node("x", "directives")],
                vec![OrgEdge {
                    source: "x".to_owned(),
                    target: "plain".to_owned(),
                    relation: "requires".to_owned(),
                    reason: Some("because".to_owned()),
                    generated_reason: None,
                    generated: false,
                }],
            )],
            None,
        )
        .unwrap();
        let edge = report
            .graph
            .edges()
            .iter()
            .find(|edge| edge.source() == "directive:x")
            .unwrap();
        assert_eq!(edge.target(), "styleguide:plain");
        assert_eq!(edge.reason(), Some("because"));
    }

    #[test]
    fn endpoint_ambiguity_and_unknown_relation_are_refused() {
        let mut graph = built_in();
        let nodes = graph.nodes().to_vec();
        let mut nodes_all = nodes.clone();
        nodes_all.push(node("tactic:plain", NodeKind::Tactic));
        graph = graph.with_parts(nodes_all, graph.edges().to_vec()).unwrap();
        let error = merge_three_layers(
            &graph,
            &[fragment(
                vec![org_node("x", "directives")],
                vec![OrgEdge {
                    source: "x".to_owned(),
                    target: "plain".to_owned(),
                    relation: "requires".to_owned(),
                    reason: None,
                    generated_reason: None,
                    generated: false,
                }],
            )],
            None,
        )
        .unwrap_err();
        assert!(matches!(error, DoctrineError::AmbiguousEndpoint { .. }));

        let error = merge_three_layers(
            &built_in(),
            &[fragment(
                vec![org_node("x", "directives")],
                vec![OrgEdge {
                    source: "x".to_owned(),
                    target: "plain".to_owned(),
                    relation: "wibbles".to_owned(),
                    reason: None,
                    generated_reason: None,
                    generated: false,
                }],
            )],
            None,
        )
        .unwrap_err();
        assert!(matches!(error, DoctrineError::UnknownRelation { .. }));
    }

    #[test]
    fn unresolved_qualified_endpoint_is_refused_unless_forward_refs_allowed() {
        let fragments = [fragment(
            vec![org_node("x", "directives")],
            vec![OrgEdge {
                source: "x".to_owned(),
                target: "tactic:later".to_owned(),
                relation: "requires".to_owned(),
                reason: None,
                generated_reason: None,
                generated: false,
            }],
        )];
        assert!(merge_three_layers(&built_in(), &fragments, None).is_err());
        let report = merge_three_layers_with_options(
            &built_in(),
            &fragments,
            None,
            MergeOptions {
                allow_forward_references: true,
            },
        )
        .unwrap();
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("unresolved DRG edge endpoint"))
        );
    }

    #[test]
    fn kind_drift_and_duplicate_asset_urns_are_refused() {
        let mut first = built_in();
        let nodes = first.nodes().to_vec();
        let mut with_asset = nodes.clone();
        with_asset.push(node("asset:logo", NodeKind::Asset));
        first = first
            .with_parts(with_asset, first.edges().to_vec())
            .unwrap();
        let error = merge_three_layers(
            &first,
            &[fragment(vec![org_node("logo", "assets")], vec![])],
            None,
        )
        .unwrap_err();
        assert!(matches!(error, DoctrineError::DuplicateUrn { .. }));
    }

    #[test]
    fn loaded_pack_merges_end_to_end() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join("drg")).unwrap();
        fs::write(root.join("pack.yaml"), "name: acme\npack_level: org\n").unwrap();
        fs::write(
            root.join("drg/fragment.yaml"),
            "nodes:\n  - id: 001\n    kind: directives\nedges:\n  - source: 001\n    target: hard\n    relation: requires\n",
        )
        .unwrap();
        let fragment = load_org_pack("acme", root, 1).unwrap();
        let report = merge_three_layers(&built_in(), &[fragment], None).unwrap();
        assert!(
            report
                .graph
                .edges()
                .iter()
                .any(|edge| edge.source() == "directive:001" && edge.target() == "tactic:hard")
        );
    }

    fn yaml(text: &str) -> Value {
        serde_yaml_ng::from_str(text).unwrap()
    }

    #[test]
    fn topology_enhance_preserves_steps_and_io() {
        let base = yaml(
            "steps:\n  - id: spec\n    inputs: [intent]\n    outputs: [spec]\n  - id: plan\n    inputs: [spec]\n    outputs: [plan]\n",
        );
        let overlay = yaml("steps:\n  - id: spec\n    note: tighter\n  - id: plan\n");
        let merged = merge_topology_artifact(&base, &overlay, Relation::Enhances).unwrap();
        let steps = merged.get("steps").unwrap().as_sequence().unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(
            steps[0].get("inputs").unwrap().as_sequence().unwrap().len(),
            1
        );

        let dropped = yaml("steps:\n  - id: spec\n");
        assert!(matches!(
            merge_topology_artifact(&base, &dropped, Relation::Enhances),
            Err(DoctrineError::TopologyStepDropped { .. })
        ));

        let stripped = yaml("steps:\n  - id: spec\n    inputs: []\n  - id: plan\n");
        assert!(matches!(
            merge_topology_artifact(&base, &stripped, Relation::Enhances),
            Err(DoctrineError::TopologyIoStripped { .. })
        ));

        let replaced = yaml("steps:\n  - id: only\n");
        let merged = merge_topology_artifact(&base, &replaced, Relation::Overrides).unwrap();
        assert_eq!(merged.get("steps").unwrap().as_sequence().unwrap().len(), 1);
    }
}
