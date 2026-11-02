//! Activation-aware graph filtering and profile gating.
//!
//! Mirrors `src/charter/activation/drg_activation.py` and the three-state
//! allow/deny model of `pack_context.py`. The filter is applied only to
//! charter-mediated resolution: direct doctrine reads still see every artifact.

use std::collections::{BTreeMap, BTreeSet};

use sk_charter::ArtifactKind;

use crate::error::DoctrineError;
use crate::graph::{DrgGraph, NodeKind, Relation};
use crate::identity::IdentityIndex;
use crate::ledger::{ActivationLedger, DenySet, effective_deny};

/// The effective activation state used by the graph filter.
#[derive(Clone, Debug, Default)]
pub struct ActivationState {
    /// The activated plural kinds; `None` means every kind.
    pub activated_kinds: Option<BTreeSet<String>>,
    /// Activated mission types.
    pub mission_types: BTreeSet<String>,
    /// Per-kind allow state, resolved to canonical URNs.
    pub allow: BTreeMap<ArtifactKind, Option<BTreeSet<String>>>,
    /// Per-kind effective deny (after the required floor).
    pub deny: BTreeMap<ArtifactKind, DenySet>,
    /// Activation tokens that resolved to no artifact (never silently applied).
    pub unknown_allow_tokens: BTreeMap<String, Vec<String>>,
}

impl ActivationState {
    /// Build the state from a ledger, resolving every allow token through the
    /// identity index and subtracting the required floor from each deny list.
    pub fn from_ledger(
        ledger: &ActivationLedger,
        identity: &IdentityIndex,
        required_floor: &BTreeMap<ArtifactKind, Vec<String>>,
    ) -> Result<Self, DoctrineError> {
        let mut allow = BTreeMap::new();
        let mut deny = BTreeMap::new();
        let mut unknown_allow_tokens: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for kind in crate::ledger::activatable_kinds() {
            let resolved = match ledger.allow_for(kind) {
                None => None,
                Some(tokens) => {
                    let mut urns = BTreeSet::new();
                    let mut unknown = Vec::new();
                    for token in tokens {
                        match identity.declared_id_checked(kind, token)? {
                            Some(declared) => {
                                urns.insert(format!("{}:{declared}", kind.canonical()));
                            }
                            None => unknown.push(token.clone()),
                        }
                    }
                    if !unknown.is_empty() {
                        unknown_allow_tokens.insert(kind.canonical().to_owned(), unknown);
                    }
                    Some(urns)
                }
            };
            allow.insert(kind, resolved);
            let floor = required_floor.get(&kind).cloned().unwrap_or_default();
            deny.insert(
                kind,
                effective_deny(kind, ledger.deny_for(kind).iter().cloned(), floor, identity),
            );
        }
        Ok(Self {
            activated_kinds: ledger.activated_kinds_set(),
            mission_types: ledger.mission_types(),
            allow,
            deny,
            unknown_allow_tokens,
        })
    }

    /// Whether an artifact node survives the activation filter.
    pub fn admits(&self, kind: &str, artifact_id: &str) -> bool {
        let Some(node_kind) = NodeKind::from_prefix(kind) else {
            return true;
        };
        let Some(artifact_kind) = node_kind.artifact_kind() else {
            // `action`, `mission_type` and glossary nodes are not per-kind gated.
            return true;
        };
        if artifact_id.is_empty() || !artifact_kind.activatable() {
            return true;
        }
        if self.deny.get(&artifact_kind).is_some_and(|deny| {
            !deny.admits(artifact_id) || !deny.admits(&format!("{kind}:{artifact_id}"))
        }) {
            return false;
        }
        if node_kind == NodeKind::MissionStepContract
            && let Some(owner) = artifact_id.split_once('/').map(|(owner, _)| owner)
        {
            return self.mission_types.contains(owner);
        }
        if self
            .activated_kinds
            .as_ref()
            .is_some_and(|activated| !activated.contains(artifact_kind.plural()))
        {
            return false;
        }
        match self.allow.get(&artifact_kind) {
            None | Some(None) => true,
            Some(Some(urns)) => urns.contains(&format!("{kind}:{artifact_id}")),
        }
    }

    /// Whether a profile id survives the `activated_agent_profiles` gate.
    pub fn admits_profile(&self, profile_id: &str) -> bool {
        self.admits(NodeKind::AgentProfile.as_str(), profile_id)
    }

    /// An artifact node's deactivated/spelled deny set.
    pub fn deny_for(&self, kind: ArtifactKind) -> Option<&DenySet> {
        self.deny.get(&kind)
    }
}

/// Return a copy of `graph` limited to activated artifacts, keeping an edge only
/// when both endpoints survive.
pub fn filter_graph_by_activation(graph: &DrgGraph, state: &ActivationState) -> DrgGraph {
    let nodes: Vec<_> = graph
        .nodes()
        .iter()
        .filter(|node| {
            let Some((kind, id)) = node.urn().split_once(':') else {
                return true;
            };
            state.admits(kind, id)
        })
        .cloned()
        .collect();
    let surviving: BTreeSet<&str> = nodes.iter().map(|node| node.urn()).collect();
    let edges: Vec<_> = graph
        .edges()
        .iter()
        .filter(|edge| surviving.contains(edge.source()) && surviving.contains(edge.target()))
        .cloned()
        .collect();
    graph
        .with_parts(nodes, edges)
        .unwrap_or_else(|_| graph.clone())
}

/// The DRG URNs of every denied artifact, across kinds.
pub fn deactivated_urns(ledger: &ActivationLedger, identity: &IdentityIndex) -> BTreeSet<String> {
    let mut urns = BTreeSet::new();
    for kind in crate::ledger::activatable_kinds() {
        let deny =
            crate::ledger::resolve_deny(kind, ledger.deny_for(kind).iter().cloned(), identity);
        urns.extend(deny.urns);
    }
    urns
}

/// Edges of a node that the cascade follows (`requires`/`suggests`/`refines`/...).
pub fn reference_edges<'a>(graph: &'a DrgGraph, urn: &str) -> Vec<&'a crate::graph::DrgEdge> {
    graph
        .edges_from(urn, None)
        .into_iter()
        .filter(|edge| crate::cascade::REFERENCE_RELATIONS.contains(&edge.relation()))
        .collect()
}

/// The relation a mission-type statically applies to its governance.
pub fn is_mission_scope_edge(edge: &crate::graph::DrgEdge) -> bool {
    edge.relation() == Relation::Scope && edge.source().starts_with("mission_type:")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{DrgEdge, DrgNode};
    use crate::identity::scan_all;

    fn graph() -> DrgGraph {
        DrgGraph::new(
            "1.0",
            "x",
            "t",
            vec![
                DrgNode::new("agent_profile:a", NodeKind::AgentProfile, None, Vec::new()).unwrap(),
                DrgNode::new(
                    "directive:DIRECTIVE_001",
                    NodeKind::Directive,
                    None,
                    Vec::new(),
                )
                .unwrap(),
                DrgNode::new("tactic:t", NodeKind::Tactic, None, Vec::new()).unwrap(),
            ],
            vec![
                DrgEdge::new(
                    "agent_profile:a",
                    "directive:DIRECTIVE_001",
                    Relation::Requires,
                )
                .unwrap(),
            ],
        )
        .unwrap()
    }

    #[test]
    fn kind_allow_deny_and_three_state_behave() {
        let directory = tempfile::tempdir().unwrap();
        let built_in = directory.path().join("built-in");
        std::fs::create_dir_all(built_in.join("directives")).unwrap();
        std::fs::write(
            built_in.join("directives/001-arch.directive.yaml"),
            "id: DIRECTIVE_001\n",
        )
        .unwrap();
        let mut identity = IdentityIndex::from_roots(&built_in, &[], None);
        scan_all(&mut identity);

        let mut ledger = ActivationLedger::default();
        ledger
            .allow
            .insert(ArtifactKind::Directive, Some(vec!["001-arch".to_owned()]));
        let state = ActivationState::from_ledger(&ledger, &identity, &BTreeMap::new()).unwrap();
        assert!(state.admits("directive", "DIRECTIVE_001"));
        assert!(!state.admits("directive", "DIRECTIVE_002"));
        assert!(state.admits("tactic", "t"));
        let filtered = filter_graph_by_activation(&graph(), &state);
        assert!(filtered.node("directive:DIRECTIVE_001").is_some());
        assert!(filtered.node("tactic:t").is_some());

        let mut deny_ledger = ActivationLedger::default();
        deny_ledger
            .deny
            .insert(ArtifactKind::Directive, vec!["001-arch".to_owned()]);
        let state =
            ActivationState::from_ledger(&deny_ledger, &identity, &BTreeMap::new()).unwrap();
        assert!(!state.admits("directive", "DIRECTIVE_001"));
        let filtered = filter_graph_by_activation(&graph(), &state);
        assert!(filtered.node("directive:DIRECTIVE_001").is_none());
        assert!(filtered.edges().is_empty());
    }

    #[test]
    fn non_activatable_and_mission_step_nodes_use_their_own_gates() {
        let directory = tempfile::tempdir().unwrap();
        let built_in = directory.path().join("built-in");
        std::fs::create_dir_all(&built_in).unwrap();
        let identity = IdentityIndex::from_roots(&built_in, &[], None);
        let mut allow = BTreeMap::new();
        allow.insert(ArtifactKind::Tactic, Some(Vec::new()));
        let ledger = ActivationLedger {
            activated_kinds: Some(vec!["agent_profiles".to_owned()]),
            mission_type_activations: Some(vec!["software-dev".to_owned()]),
            allow,
            ..ActivationLedger::default()
        };
        let state = ActivationState::from_ledger(&ledger, &identity, &BTreeMap::new()).unwrap();
        // A template is structurally non-activatable: no kind/allow gate applies.
        assert!(state.admits("template", "t"));
        // A mission step follows its owning mission type, not activated_kinds.
        assert!(state.admits("mission_step_contract", "software-dev/spec"));
        assert!(!state.admits("mission_step_contract", "documentation/write"));
        // An activatable kind outside activated_kinds is still filtered.
        assert!(!state.admits("tactic", "hard"));
    }

    #[test]
    fn kind_level_gate_and_unknown_tokens_are_reported() {
        let directory = tempfile::tempdir().unwrap();
        let built_in = directory.path().join("built-in");
        std::fs::create_dir_all(&built_in).unwrap();
        let identity = IdentityIndex::from_roots(&built_in, &[], None);
        let mut allow = BTreeMap::new();
        allow.insert(ArtifactKind::Directive, Some(vec!["missing".to_owned()]));
        let ledger = ActivationLedger {
            activated_kinds: Some(vec!["agent_profiles".to_owned()]),
            allow,
            ..ActivationLedger::default()
        };
        let state = ActivationState::from_ledger(&ledger, &identity, &BTreeMap::new()).unwrap();
        assert!(!state.admits("directive", "001"));
        assert!(state.admits("agent_profile", "a"));
        assert_eq!(
            state.unknown_allow_tokens["directive"],
            vec!["missing".to_owned()]
        );
    }
}
