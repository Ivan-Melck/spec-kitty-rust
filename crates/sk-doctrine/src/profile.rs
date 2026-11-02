//! Agent-profile loading, lineage composition and profile-channel delivery.
//!
//! Mirrors `src/charter/offering/agent_profiles/repository.py` and
//! `src/charter/activation/profile_resolution.py`. Lineage is read from the DRG
//! (`agent_profile --specializes_from--> agent_profile`) only; a missing parent,
//! a cycle, or more than one declared parent is a typed refusal, never a
//! silently chosen first edge.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_yaml_ng::{Mapping, Value};

use crate::error::DoctrineError;
use crate::graph::{DrgGraph, NodeKind, Relation};
use crate::query::profile_channel_relations;

/// List-valued profile fields that inherit by union up the lineage chain.
pub const UNION_LIST_FIELDS: [&str; 5] = [
    "capabilities",
    "directive-references",
    "canonical-verbs",
    "mode-defaults",
    "tactic-references",
];

/// One loaded agent profile.
#[derive(Clone, Debug)]
pub struct AgentProfile {
    /// The profile id.
    pub id: String,
    /// The declaring file.
    pub path: PathBuf,
    /// The profile body after lineage composition.
    pub body: Mapping,
    /// The lineage chain, root first, leaf last.
    pub lineage: Vec<String>,
}

impl AgentProfile {
    /// The display name, when one is declared.
    pub fn name(&self) -> Option<&str> {
        self.body
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
    }

    /// The title, when one is declared.
    pub fn title(&self) -> Option<&str> {
        self.body
            .get("title")
            .and_then(Value::as_str)
            .filter(|title| !title.is_empty())
    }

    /// The structural sentinel flag.
    pub fn sentinel(&self) -> bool {
        self.body
            .get("sentinel")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    /// A list field's entries.
    pub fn list(&self, field: &str) -> Vec<&Value> {
        self.body
            .get(field)
            .and_then(Value::as_sequence)
            .map(|items| items.iter().collect())
            .unwrap_or_default()
    }

    /// Directive reference codes cited by this profile (including inherited ones).
    pub fn directive_references(&self) -> Vec<String> {
        reference_ids(self.list("directive-references"))
    }

    /// Tactic reference ids cited by this profile (including inherited ones).
    pub fn tactic_references(&self) -> Vec<String> {
        reference_ids(self.list("tactic-references"))
    }
}

fn reference_ids(items: Vec<&Value>) -> Vec<String> {
    let mut ids: Vec<String> = items
        .into_iter()
        .filter_map(|item| {
            item.as_str().map(str::to_owned).or_else(|| {
                item.get("code")
                    .or_else(|| item.get("id"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
        })
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// A repository over the built-in, org and project profile layers.
#[derive(Clone, Debug, Default)]
pub struct ProfileRepository {
    layers: Vec<(usize, PathBuf, String)>,
    profiles: BTreeMap<String, (AgentProfile, usize)>,
}

impl ProfileRepository {
    /// Load profiles from layers in overlay order: built-in, org packs in
    /// declaration order, then project. Later layers win for the same id.
    pub fn load(
        built_in_root: &Path,
        org_roots: &[PathBuf],
        project_root: Option<&Path>,
    ) -> Result<Self, DoctrineError> {
        let mut layers: Vec<(usize, PathBuf, String)> = Vec::new();
        layers.push((
            0,
            built_in_root.join("agent_profiles"),
            "built-in".to_owned(),
        ));
        for (position, root) in org_roots.iter().enumerate() {
            layers.push((position + 1, root.join("agent_profiles"), "org".to_owned()));
        }
        if let Some(project) = project_root {
            layers.push((
                org_roots.len() + 1,
                project.join("agent_profiles"),
                "project".to_owned(),
            ));
        }
        let mut repository = Self {
            layers,
            profiles: BTreeMap::new(),
        };
        for (layer, directory, _) in repository.layers.clone() {
            for path in sorted_files(&directory, ".agent.yaml") {
                let text = std::fs::read_to_string(&path)
                    .map_err(|source| DoctrineError::io("read agent profile", &path, source))?;
                let value: Value = serde_yaml_ng::from_str(&text).map_err(|error| {
                    DoctrineError::InvalidProfile {
                        path: path.clone(),
                        message: error.to_string(),
                    }
                })?;
                let Some(mapping) = value.as_mapping() else {
                    return Err(DoctrineError::InvalidProfile {
                        path: path.clone(),
                        message: "expected a YAML mapping".to_owned(),
                    });
                };
                let id = ["profile-id", "profile_id"]
                    .iter()
                    .find_map(|key| mapping.get(*key).and_then(Value::as_str))
                    .filter(|id| !id.is_empty())
                    .map(str::to_owned)
                    .or_else(|| file_stem(&path));
                let Some(id) = id else {
                    return Err(DoctrineError::InvalidProfile {
                        path: path.clone(),
                        message: "missing a non-empty 'profile-id'".to_owned(),
                    });
                };
                let profile = AgentProfile {
                    id: id.clone(),
                    path,
                    body: mapping.clone(),
                    lineage: Vec::new(),
                };
                repository.profiles.insert(id.to_owned(), (profile, layer));
            }
        }
        Ok(repository)
    }

    /// All profile ids, sorted.
    pub fn ids(&self) -> Vec<String> {
        self.profiles.keys().cloned().collect()
    }

    /// The raw (uncomposed) profile record.
    pub fn get(&self, id: &str) -> Option<&AgentProfile> {
        let key = self
            .profiles
            .get_key_value(id)
            .map(|(key, _)| key.clone())
            .or_else(|| {
                self.profiles
                    .iter()
                    .find(|(_, (profile, _))| file_stem(&profile.path).as_deref() == Some(id))
                    .map(|(key, _)| key.clone())
            })?;
        self.profiles.get(&key).map(|(profile, _)| profile)
    }

    /// Resolve `id` with its `specializes_from` lineage composed.
    pub fn resolve(&self, id: &str, graph: &DrgGraph) -> Result<AgentProfile, DoctrineError> {
        let leaf = self
            .get(id)
            .ok_or_else(|| DoctrineError::ProfileNotFound {
                profile: id.to_owned(),
            })?
            .clone();
        let leaf_id = leaf.id.clone();
        let leaf_path = leaf.path.clone();
        let mut chain: Vec<AgentProfile> = vec![leaf];
        let mut visited: BTreeSet<String> = BTreeSet::from([chain[0].id.clone()]);
        loop {
            let current = chain.last().expect("chain is non-empty").id.clone();
            let parents = self.lineage_parents(&current, graph)?;
            let Some(parent_id) = parents.first().cloned() else {
                break;
            };
            if visited.contains(&parent_id) {
                let mut cycle: Vec<String> = visited.iter().cloned().collect();
                cycle.push(parent_id);
                return Err(DoctrineError::LineageCycle { chain: cycle });
            }
            let parent = self
                .get(&parent_id)
                .ok_or_else(|| DoctrineError::MissingLineageParent {
                    profile: current.clone(),
                    parent: parent_id.clone(),
                })?
                .clone();
            visited.insert(parent_id);
            chain.push(parent);
        }

        let mut merged = Mapping::new();
        for profile in chain.iter().rev() {
            merged = union_merge(&merged, &profile.body);
        }
        let lineage: Vec<String> = chain
            .iter()
            .rev()
            .map(|profile| profile.id.clone())
            .collect();
        Ok(AgentProfile {
            id: leaf_id,
            path: leaf_path,
            body: merged,
            lineage,
        })
    }

    fn lineage_parents(
        &self,
        profile_id: &str,
        graph: &DrgGraph,
    ) -> Result<Vec<String>, DoctrineError> {
        let urn = format!("agent_profile:{profile_id}");
        let mut parents: BTreeSet<String> = graph
            .edges_from(&urn, Some(Relation::SpecializesFrom))
            .into_iter()
            .filter_map(|edge| edge.target().strip_prefix("agent_profile:"))
            .map(str::to_owned)
            .collect();
        // Support an authored project field only when the graph has no edge for
        // this profile; the graph remains the authority, and a disagreement is
        // not silently resolved by mixing the two sources.
        if parents.is_empty()
            && let Some(profile) = self.get(profile_id)
            && let Some(value) = profile.body.get("specializes_from")
        {
            match value {
                Value::String(parent) => {
                    parents.insert(parent.clone());
                }
                Value::Sequence(items) => {
                    for item in items {
                        if let Some(parent) = item.as_str() {
                            parents.insert(parent.to_owned());
                        }
                    }
                }
                _ => {}
            }
        }
        if parents.len() > 1 {
            return Err(DoctrineError::AmbiguousLineage {
                profile: profile_id.to_owned(),
                parents: parents.into_iter().collect(),
            });
        }
        Ok(parents.into_iter().collect())
    }

    /// The artefact URNs a profile reaches through the profile channel, with
    /// `when` clauses for `suggests` deliveries.
    pub fn channel_deliveries(
        &self,
        id: &str,
        graph: &DrgGraph,
    ) -> Result<Vec<ProfileDelivery>, DoctrineError> {
        let profile = self.resolve(id, graph)?;
        let urn = format!("agent_profile:{}", profile.id);
        if graph
            .node(&urn)
            .is_none_or(|node| node.kind() != NodeKind::AgentProfile)
        {
            return Ok(Vec::new());
        }
        let relations = profile_channel_relations();
        let mut reached: BTreeSet<String> = BTreeSet::from([urn.clone()]);
        let mut queue: Vec<String> = vec![urn.clone()];
        let mut deliveries = Vec::new();
        while let Some(current) = queue.pop() {
            for edge in graph.edges_from(&current, None) {
                if !relations.contains(&edge.relation()) {
                    continue;
                }
                deliveries.push(ProfileDelivery {
                    profile: profile.id.clone(),
                    urn: edge.target().to_owned(),
                    relation: edge.relation(),
                    when: edge.when().map(str::to_owned),
                });
                if reached.insert(edge.target().to_owned()) {
                    queue.push(edge.target().to_owned());
                }
            }
        }
        deliveries.sort_by(|left, right| {
            (&left.urn, left.relation, &left.when).cmp(&(&right.urn, right.relation, &right.when))
        });
        deliveries.dedup();
        Ok(deliveries)
    }
}

/// One profile-channel delivery.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct ProfileDelivery {
    /// The profile that reaches the artifact.
    pub profile: String,
    /// The reached artifact URN.
    pub urn: String,
    /// The relation by which it is reached.
    pub relation: Relation,
    /// The edge's `when` clause, for `suggests`.
    pub when: Option<String>,
}

fn union_merge(parent: &Mapping, child: &Mapping) -> Mapping {
    let mut merged = parent.clone();
    for (key, child_value) in child {
        let key_name = key.as_str().unwrap_or_default();
        match (merged.get(key), child_value) {
            (Some(Value::Sequence(parent_items)), Value::Sequence(child_items))
                if UNION_LIST_FIELDS.contains(&key_name) =>
            {
                let mut seen: BTreeSet<String> = parent_items.iter().map(item_key).collect();
                let mut union = parent_items.clone();
                for item in child_items {
                    if seen.insert(item_key(item)) {
                        union.push(item.clone());
                    }
                }
                merged.insert(key.clone(), Value::Sequence(union));
            }
            (Some(Value::Mapping(parent_map)), Value::Mapping(child_map)) => {
                let mut nested = parent_map.clone();
                for (nested_key, nested_value) in child_map {
                    nested.insert(nested_key.clone(), nested_value.clone());
                }
                merged.insert(key.clone(), Value::Mapping(nested));
            }
            _ => {
                merged.insert(key.clone(), child_value.clone());
            }
        }
    }
    merged
}

fn item_key(item: &Value) -> String {
    if let Some(mapping) = item.as_mapping() {
        if let Some(code) = mapping.get("code").and_then(Value::as_str) {
            return code.to_owned();
        }
        if let Some(id) = mapping.get("id").and_then(Value::as_str) {
            return id.to_owned();
        }
    }
    match item {
        Value::String(text) => text.clone(),
        other => format!("{other:?}"),
    }
}

fn file_stem(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.split('.').next().unwrap_or(name).to_owned())
        .filter(|stem| !stem.is_empty())
}

fn sorted_files(directory: &Path, suffix: &str) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_files(directory, suffix, &mut files);
    files.sort();
    files
}

fn collect_files(directory: &Path, suffix: &str, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, suffix, files);
        } else if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(suffix))
        {
            files.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{DrgEdge, DrgNode};
    use std::fs;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn graph(edges: Vec<DrgEdge>) -> DrgGraph {
        let mut nodes = vec![
            DrgNode::new(
                "agent_profile:base",
                NodeKind::AgentProfile,
                None,
                Vec::new(),
            )
            .unwrap(),
            DrgNode::new(
                "agent_profile:leaf",
                NodeKind::AgentProfile,
                None,
                Vec::new(),
            )
            .unwrap(),
            DrgNode::new("directive:001", NodeKind::Directive, None, Vec::new()).unwrap(),
        ];
        nodes.sort_by(|left, right| left.urn().cmp(right.urn()));
        DrgGraph::new("1.0", "x", "t", nodes, edges).unwrap()
    }

    #[test]
    fn lineage_composes_references_from_root_to_leaf() {
        let directory = tempfile::tempdir().unwrap();
        let built_in = directory.path().join("built-in");
        write(
            &built_in.join("agent_profiles/base.agent.yaml"),
            "profile-id: base\ndirective-references:\n  - code: 001\ncapabilities: [a]\n",
        );
        write(
            &built_in.join("agent_profiles/leaf.agent.yaml"),
            "profile-id: leaf\ndirective-references:\n  - code: 002\ncapabilities: [b]\npurpose: leaf\n",
        );
        let repository = ProfileRepository::load(&built_in, &[], None).unwrap();
        let graph = graph(vec![
            DrgEdge::new(
                "agent_profile:leaf",
                "agent_profile:base",
                Relation::SpecializesFrom,
            )
            .unwrap(),
        ]);
        let profile = repository.resolve("leaf", &graph).unwrap();
        assert_eq!(profile.lineage, vec!["base".to_owned(), "leaf".to_owned()]);
        assert_eq!(profile.directive_references(), vec!["001", "002"]);
        assert_eq!(
            profile
                .body
                .get("capabilities")
                .unwrap()
                .as_sequence()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(profile.body.get("purpose").unwrap().as_str(), Some("leaf"));
    }

    #[test]
    fn missing_parent_cycle_and_ambiguity_are_refused() {
        let directory = tempfile::tempdir().unwrap();
        let built_in = directory.path().join("built-in");
        write(
            &built_in.join("agent_profiles/leaf.agent.yaml"),
            "profile-id: leaf\n",
        );
        let repository = ProfileRepository::load(&built_in, &[], None).unwrap();
        let missing = graph(vec![
            DrgEdge::new(
                "agent_profile:leaf",
                "agent_profile:absent",
                Relation::SpecializesFrom,
            )
            .unwrap(),
        ]);
        assert!(matches!(
            repository.resolve("leaf", &missing),
            Err(DoctrineError::MissingLineageParent { .. })
        ));

        write(
            &built_in.join("agent_profiles/base.agent.yaml"),
            "profile-id: base\n",
        );
        let repository = ProfileRepository::load(&built_in, &[], None).unwrap();
        let cycle = graph(vec![
            DrgEdge::new(
                "agent_profile:leaf",
                "agent_profile:base",
                Relation::SpecializesFrom,
            )
            .unwrap(),
            DrgEdge::new(
                "agent_profile:base",
                "agent_profile:leaf",
                Relation::SpecializesFrom,
            )
            .unwrap(),
        ]);
        assert!(matches!(
            repository.resolve("leaf", &cycle),
            Err(DoctrineError::LineageCycle { .. })
        ));
        write(
            &built_in.join("agent_profiles/other.agent.yaml"),
            "profile-id: other\n",
        );
        let repository = ProfileRepository::load(&built_in, &[], None).unwrap();
        let graph = DrgGraph::new(
            "1.0",
            "x",
            "t",
            vec![
                DrgNode::new(
                    "agent_profile:leaf",
                    NodeKind::AgentProfile,
                    None,
                    Vec::new(),
                )
                .unwrap(),
                DrgNode::new(
                    "agent_profile:base",
                    NodeKind::AgentProfile,
                    None,
                    Vec::new(),
                )
                .unwrap(),
                DrgNode::new(
                    "agent_profile:other",
                    NodeKind::AgentProfile,
                    None,
                    Vec::new(),
                )
                .unwrap(),
            ],
            vec![
                DrgEdge::new(
                    "agent_profile:leaf",
                    "agent_profile:base",
                    Relation::SpecializesFrom,
                )
                .unwrap(),
                DrgEdge::new(
                    "agent_profile:leaf",
                    "agent_profile:other",
                    Relation::SpecializesFrom,
                )
                .unwrap(),
            ],
        )
        .unwrap();
        assert!(matches!(
            repository.resolve("leaf", &graph),
            Err(DoctrineError::AmbiguousLineage { .. })
        ));
    }

    #[test]
    fn malformed_profile_names_the_file() {
        let directory = tempfile::tempdir().unwrap();
        let built_in = directory.path().join("built-in");
        write(
            &built_in.join("agent_profiles/bad.agent.yaml"),
            "profile-id: [not, a, string]\n",
        );
        let repository = ProfileRepository::load(&built_in, &[], None).unwrap();
        assert!(repository.get("bad").is_some());
        write(
            &built_in.join("agent_profiles/broken.agent.yaml"),
            "profile-id: broken\n  bad: indent\n",
        );
        let error = ProfileRepository::load(&built_in, &[], None).unwrap_err();
        assert!(error.to_string().contains("broken.agent.yaml"));
    }

    #[test]
    fn profile_channel_reports_suggests_when_clauses() {
        let directory = tempfile::tempdir().unwrap();
        let built_in = directory.path().join("built-in");
        write(
            &built_in.join("agent_profiles/reviewer.agent.yaml"),
            "profile-id: reviewer\n",
        );
        let repository = ProfileRepository::load(&built_in, &[], None).unwrap();
        let graph = DrgGraph::new(
            "1.0",
            "x",
            "t",
            vec![
                DrgNode::new(
                    "agent_profile:reviewer",
                    NodeKind::AgentProfile,
                    None,
                    Vec::new(),
                )
                .unwrap(),
                DrgNode::new("tactic:deep-review", NodeKind::Tactic, None, Vec::new()).unwrap(),
            ],
            vec![
                DrgEdge::new(
                    "agent_profile:reviewer",
                    "tactic:deep-review",
                    Relation::Suggests,
                )
                .unwrap()
                .with_when("high-risk review"),
            ],
        )
        .unwrap();
        let deliveries = repository.channel_deliveries("reviewer", &graph).unwrap();
        assert_eq!(deliveries.len(), 1);
        assert_eq!(deliveries[0].when.as_deref(), Some("high-risk review"));
    }
}
