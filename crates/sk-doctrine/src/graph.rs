//! The Doctrine Reference Graph: typed nodes, edges, URNs and the graph value.
//!
//! Mirrors `src/charter/offering/drg/models.py`. URN validity is syntactic
//! only; whether a prefix is a [`NodeKind`] and whether a node exists are
//! deliberately separate checks performed by the merge and the validator.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};

use sk_charter::ArtifactKind;

use crate::error::DoctrineError;

/// Every canonical DRG node kind.
///
/// A superset of [`ArtifactKind`] plus the action, mission-type, glossary and
/// anti-pattern node kinds.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    /// A governance directive.
    Directive,
    /// A reusable tactic.
    Tactic,
    /// A paradigm.
    Paradigm,
    /// A styleguide.
    Styleguide,
    /// A toolguide.
    Toolguide,
    /// A procedure.
    Procedure,
    /// An agent profile.
    AgentProfile,
    /// A mission step contract.
    MissionStepContract,
    /// A mission template.
    Template,
    /// An asset.
    Asset,
    /// A mission action (the action-channel entry point).
    Action,
    /// A runtime glossary scope (retiring).
    GlossaryScope,
    /// A runtime glossary term (retiring).
    Glossary,
    /// A charter-activatable glossary pack.
    GlossaryPack,
    /// A mission type.
    MissionType,
    /// A marked anti-pattern / smell.
    AntiPattern,
    /// A pack skill (a thin entry point into doctrine).
    Skill,
}

impl NodeKind {
    /// Every member, in declaration order.
    pub const ALL: [Self; 17] = [
        Self::Directive,
        Self::Tactic,
        Self::Paradigm,
        Self::Styleguide,
        Self::Toolguide,
        Self::Procedure,
        Self::AgentProfile,
        Self::MissionStepContract,
        Self::Template,
        Self::Asset,
        Self::Action,
        Self::GlossaryScope,
        Self::Glossary,
        Self::GlossaryPack,
        Self::MissionType,
        Self::AntiPattern,
        Self::Skill,
    ];

    /// The canonical snake_case value, which is also the URN prefix.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Directive => "directive",
            Self::Tactic => "tactic",
            Self::Paradigm => "paradigm",
            Self::Styleguide => "styleguide",
            Self::Toolguide => "toolguide",
            Self::Procedure => "procedure",
            Self::AgentProfile => "agent_profile",
            Self::MissionStepContract => "mission_step_contract",
            Self::Template => "template",
            Self::Asset => "asset",
            Self::Action => "action",
            Self::GlossaryScope => "glossary_scope",
            Self::Glossary => "glossary",
            Self::GlossaryPack => "glossary_pack",
            Self::MissionType => "mission_type",
            Self::AntiPattern => "anti_pattern",
            Self::Skill => "skill",
        }
    }

    /// Parse a URN/kind prefix.
    pub fn from_prefix(prefix: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|kind| kind.as_str() == prefix)
    }

    /// The artifact kind this node kind addresses, when it is an artifact kind.
    pub fn artifact_kind(self) -> Option<ArtifactKind> {
        match self {
            Self::Directive => Some(ArtifactKind::Directive),
            Self::Tactic => Some(ArtifactKind::Tactic),
            Self::Paradigm => Some(ArtifactKind::Paradigm),
            Self::Styleguide => Some(ArtifactKind::Styleguide),
            Self::Toolguide => Some(ArtifactKind::Toolguide),
            Self::Procedure => Some(ArtifactKind::Procedure),
            Self::AgentProfile => Some(ArtifactKind::AgentProfile),
            Self::MissionStepContract => Some(ArtifactKind::MissionStepContract),
            Self::Template => Some(ArtifactKind::Template),
            Self::Asset => Some(ArtifactKind::Asset),
            Self::GlossaryPack => Some(ArtifactKind::GlossaryPack),
            Self::AntiPattern => Some(ArtifactKind::AntiPattern),
            Self::Skill => Some(ArtifactKind::Skill),
            Self::Action | Self::GlossaryScope | Self::Glossary | Self::MissionType => None,
        }
    }

    /// Whether this kind can be activated as a live governance rule.
    pub fn activatable(self) -> bool {
        self.artifact_kind().is_some_and(|kind| kind.activatable())
    }
}

impl fmt::Display for NodeKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The typed edge relations of the DRG.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    /// A hard dependency; traversed transitively by action context.
    Requires,
    /// A soft recommendation; traversed to a bounded depth.
    Suggests,
    /// The relation previously used for a profile's operating procedure.
    Applies,
    /// Action or mission type to the governance that applies.
    Scope,
    /// Artifact to a glossary scope.
    Vocabulary,
    /// Action to the template it produces.
    Instantiates,
    /// Supersession (legacy; retained for hand-authored fragments).
    Replaces,
    /// A runtime handoff between profiles.
    DelegatesTo,
    /// Static profile/artifact lineage.
    SpecializesFrom,
    /// An org overlay that field-merges into a built-in.
    Enhances,
    /// An org overlay that fully replaces a built-in.
    Overrides,
    /// A refinement of a parent or built-in.
    Refines,
    /// A symmetric, non-transitive tension pair.
    InTensionWith,
    /// A directional reconciliation of one side of a tension pair.
    ReconcilesTension,
    /// A good artifact rejecting a named anti-pattern.
    Rejects,
}

impl Relation {
    /// Every member, in declaration order.
    pub const ALL: [Self; 15] = [
        Self::Requires,
        Self::Suggests,
        Self::Applies,
        Self::Scope,
        Self::Vocabulary,
        Self::Instantiates,
        Self::Replaces,
        Self::DelegatesTo,
        Self::SpecializesFrom,
        Self::Enhances,
        Self::Overrides,
        Self::Refines,
        Self::InTensionWith,
        Self::ReconcilesTension,
        Self::Rejects,
    ];

    /// The canonical snake_case value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Requires => "requires",
            Self::Suggests => "suggests",
            Self::Applies => "applies",
            Self::Scope => "scope",
            Self::Vocabulary => "vocabulary",
            Self::Instantiates => "instantiates",
            Self::Replaces => "replaces",
            Self::DelegatesTo => "delegates_to",
            Self::SpecializesFrom => "specializes_from",
            Self::Enhances => "enhances",
            Self::Overrides => "overrides",
            Self::Refines => "refines",
            Self::InTensionWith => "in_tension_with",
            Self::ReconcilesTension => "reconciles_tension",
            Self::Rejects => "rejects",
        }
    }

    /// Parse a canonical relation value.
    pub fn from_value(value: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|rel| rel.as_str() == value)
    }

    /// The relation a fragment alias resolves to, if any.
    ///
    /// `extends` is overlay-inheritance language and means **lineage**
    /// (`specializes_from`); it must never be aliased onto the inert `applies`
    /// sink.
    pub fn from_alias(alias: &str) -> Option<Self> {
        match alias {
            "extends" => Some(Self::SpecializesFrom),
            other => Self::from_value(other),
        }
    }
}

impl fmt::Display for Relation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for Relation {
    type Err = DoctrineError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::from_value(value).ok_or_else(|| DoctrineError::UnknownRelation {
            relation: value.to_owned(),
            origin: "relation token".to_owned(),
        })
    }
}

/// Whether `value` is a syntactically valid DRG URN.
///
/// Anchored, no spaces: a lowercase/underscore kind prefix, one colon, then a
/// non-empty body of ASCII alphanumerics, `_`, `/`, `.` or `-`.
pub fn is_valid_urn(value: &str) -> bool {
    let Some((prefix, body)) = value.split_once(':') else {
        return false;
    };
    if body.contains(':') || body.is_empty() {
        return false;
    }
    !prefix.is_empty()
        && prefix
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
        && body
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'/' | b'.' | b'-'))
}

/// Split a valid URN into `(prefix, body)`.
pub fn split_urn(urn: &str) -> Option<(&str, &str)> {
    let (prefix, body) = urn.split_once(':')?;
    if body.is_empty() || body.contains(':') {
        return None;
    }
    Some((prefix, body))
}

/// A single addressable doctrine node.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DrgNode {
    urn: String,
    kind: NodeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provenance: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tags: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawNode {
    urn: String,
    kind: NodeKind,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    provenance: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}

impl<'de> Deserialize<'de> for DrgNode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = RawNode::deserialize(deserializer)?;
        Self::new(raw.urn, raw.kind, raw.label, raw.tags)
            .map(|node| node.with_provenance(raw.provenance))
            .map_err(serde::de::Error::custom)
    }
}

impl DrgNode {
    /// Construct a node, validating its URN and its prefix/kind agreement.
    pub fn new(
        urn: impl Into<String>,
        kind: NodeKind,
        label: Option<String>,
        tags: Vec<String>,
    ) -> Result<Self, DoctrineError> {
        let urn = urn.into();
        if !is_valid_urn(&urn) {
            return Err(DoctrineError::InvalidUrn { value: urn });
        }
        let prefix = urn
            .split_once(':')
            .map(|(prefix, _)| prefix)
            .unwrap_or("")
            .to_owned();
        if prefix != kind.as_str() {
            return Err(DoctrineError::UrnKindMismatch { urn, prefix, kind });
        }
        Ok(Self {
            urn,
            kind,
            label,
            provenance: None,
            tags,
        })
    }

    /// The node URN.
    pub fn urn(&self) -> &str {
        &self.urn
    }

    /// The node kind.
    pub fn kind(&self) -> NodeKind {
        self.kind
    }

    /// The optional human-readable label.
    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    /// The merge-time provenance marker (`built-in`, `org:<name>`, `project`).
    pub fn provenance(&self) -> Option<&str> {
        self.provenance.as_deref()
    }

    /// Free-form tags (for example `smell`).
    pub fn tags(&self) -> &[String] {
        &self.tags
    }

    /// The bare id (the URN body).
    pub fn id(&self) -> &str {
        split_urn(&self.urn).map(|(_, body)| body).unwrap_or("")
    }

    /// Attach or replace the provenance marker.
    pub fn with_provenance(mut self, provenance: Option<String>) -> Self {
        self.provenance = provenance;
        self
    }

    /// Replace only the label, retaining kind and identity.
    pub fn with_label(mut self, label: Option<String>) -> Self {
        self.label = label;
        self
    }
}

/// A typed, directed relationship between two nodes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DrgEdge {
    source: String,
    target: String,
    relation: Relation,
    #[serde(skip_serializing_if = "Option::is_none")]
    when: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provenance: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEdge {
    source: String,
    target: String,
    relation: Relation,
    #[serde(default)]
    when: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    provenance: Option<String>,
}

impl<'de> Deserialize<'de> for DrgEdge {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = RawEdge::deserialize(deserializer)?;
        Self::new(raw.source, raw.target, raw.relation)
            .map(|edge| {
                let edge = match raw.when {
                    Some(when) => edge.with_when(when),
                    None => edge,
                };
                let edge = match raw.reason {
                    Some(reason) => edge.with_reason(reason),
                    None => edge,
                };
                edge.with_provenance(raw.provenance)
            })
            .map_err(serde::de::Error::custom)
    }
}

impl DrgEdge {
    /// Construct an edge, validating both endpoint URNs.
    pub fn new(
        source: impl Into<String>,
        target: impl Into<String>,
        relation: Relation,
    ) -> Result<Self, DoctrineError> {
        let source = source.into();
        let target = target.into();
        for value in [&source, &target] {
            if !is_valid_urn(value) {
                return Err(DoctrineError::InvalidUrn {
                    value: value.clone(),
                });
            }
        }
        Ok(Self {
            source,
            target,
            relation,
            when: None,
            reason: None,
            provenance: None,
        })
    }

    /// The source endpoint URN.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The target endpoint URN.
    pub fn target(&self) -> &str {
        &self.target
    }

    /// The relation label.
    pub fn relation(&self) -> Relation {
        self.relation
    }

    /// An optional `when` clause surfaced as a link condition.
    pub fn when(&self) -> Option<&str> {
        self.when.as_deref()
    }

    /// The author's rationale, when one was written.
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }

    /// The merge-time provenance marker.
    pub fn provenance(&self) -> Option<&str> {
        self.provenance.as_deref()
    }

    /// Attach an optional `when` clause.
    pub fn with_when(mut self, when: impl Into<String>) -> Self {
        self.when = Some(when.into());
        self
    }

    /// Attach the author's rationale.
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    /// Attach or replace the provenance marker.
    pub fn with_provenance(mut self, provenance: Option<String>) -> Self {
        self.provenance = provenance;
        self
    }
}

/// A validated DRG graph document (`graph.yaml`).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DrgGraph {
    schema_version: String,
    generated_at: String,
    generated_by: String,
    nodes: Vec<DrgNode>,
    edges: Vec<DrgEdge>,
    #[serde(skip)]
    index: BTreeMap<String, usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGraph {
    schema_version: String,
    generated_at: String,
    generated_by: String,
    nodes: Vec<DrgNode>,
    edges: Vec<DrgEdge>,
}

impl<'de> Deserialize<'de> for DrgGraph {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = RawGraph::deserialize(deserializer)?;
        Self::new(
            raw.schema_version,
            raw.generated_at,
            raw.generated_by,
            raw.nodes,
            raw.edges,
        )
        .map_err(serde::de::Error::custom)
    }
}

impl DrgGraph {
    /// Construct a graph, refusing a schema version other than `1.0` and
    /// duplicate node URNs.
    pub fn new(
        schema_version: impl Into<String>,
        generated_at: impl Into<String>,
        generated_by: impl Into<String>,
        nodes: Vec<DrgNode>,
        edges: Vec<DrgEdge>,
    ) -> Result<Self, DoctrineError> {
        let schema_version = schema_version.into();
        if schema_version != "1.0" {
            return Err(DoctrineError::InvalidGraphDocument {
                path: std::path::PathBuf::from("<in-memory>"),
                message: format!("schema_version must be \"1.0\", got {schema_version:?}"),
            });
        }
        let mut index = BTreeMap::new();
        for (position, node) in nodes.iter().enumerate() {
            if index.insert(node.urn().to_owned(), position).is_some() {
                return Err(DoctrineError::InvalidGraphDocument {
                    path: std::path::PathBuf::from("<in-memory>"),
                    message: format!("duplicate node URN {:?}", node.urn()),
                });
            }
        }
        Ok(Self {
            schema_version,
            generated_at: generated_at.into(),
            generated_by: generated_by.into(),
            nodes,
            edges,
            index,
        })
    }

    /// The graph document schema version.
    pub fn schema_version(&self) -> &str {
        &self.schema_version
    }

    /// The `generated_at` stamp.
    pub fn generated_at(&self) -> &str {
        &self.generated_at
    }

    /// The `generated_by` stamp.
    pub fn generated_by(&self) -> &str {
        &self.generated_by
    }

    /// Nodes in document order.
    pub fn nodes(&self) -> &[DrgNode] {
        &self.nodes
    }

    /// Edges in document order.
    pub fn edges(&self) -> &[DrgEdge] {
        &self.edges
    }

    /// Every node URN.
    pub fn node_urns(&self) -> BTreeSet<&str> {
        self.nodes.iter().map(DrgNode::urn).collect()
    }

    /// Look up a node by URN.
    pub fn node(&self, urn: &str) -> Option<&DrgNode> {
        self.index.get(urn).map(|position| &self.nodes[*position])
    }

    /// Outgoing edges from `urn`, optionally filtered by relation.
    pub fn edges_from(&self, urn: &str, relation: Option<Relation>) -> Vec<&DrgEdge> {
        self.edges
            .iter()
            .filter(|edge| {
                edge.source() == urn && relation.is_none_or(|wanted| edge.relation() == wanted)
            })
            .collect()
    }

    /// Incoming edges to `urn`, optionally filtered by relation.
    pub fn edges_to(&self, urn: &str, relation: Option<Relation>) -> Vec<&DrgEdge> {
        self.edges
            .iter()
            .filter(|edge| {
                edge.target() == urn && relation.is_none_or(|wanted| edge.relation() == wanted)
            })
            .collect()
    }

    /// Replace the node set/edges while keeping the document stamps.
    pub fn with_parts(
        &self,
        nodes: Vec<DrgNode>,
        edges: Vec<DrgEdge>,
    ) -> Result<Self, DoctrineError> {
        Self::new(
            self.schema_version.clone(),
            self.generated_at.clone(),
            self.generated_by.clone(),
            nodes,
            edges,
        )
    }
}
