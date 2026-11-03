//! Agent-profile loading, lineage composition and profile-channel delivery.
//!
//! Mirrors `src/charter/offering/agent_profiles/repository.py` and
//! `src/charter/activation/profile_resolution.py`.
//!
//! Loading is layered exactly as the reference repository does it:
//!
//! * the built-in layer is read first and is the override *base*;
//! * an org or project profile with the same `profile-id` is **field-merged
//!   into** that built-in profile (nested mappings merged recursively, scalars
//!   and lists replaced), then schema-validated — a partial overlay keeps the
//!   built-in fields it does not mention;
//! * a file without a `profile-id` is skipped and recorded, never renamed
//!   after its file stem;
//! * a malformed or schema-invalid profile is skipped and recorded, so one bad
//!   file never blanks its valid siblings;
//! * `applies_to_languages` filters a scoped profile against the project's
//!   active languages (family-expanded), and a filtered id is recorded.
//!
//! Lineage is read from the DRG (`agent_profile --specializes_from-->
//! agent_profile`) only; a missing parent, a cycle, or more than one declared
//! parent is a typed refusal. The resolving (leaf) profile's `excluding`
//! declaration is applied to the merged result.

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

/// Top-level keys the agent-profile schema declares (`extra="forbid"` upstream).
///
/// Every entry is accepted by the reference model: the kebab-case serialization
/// alias AND its Python field name (`populate_by_name=True`), so both spellings
/// load. Omitting a field name here would silently skip a real built-in profile
/// (the reproduced `self-review-protocol` loss).
const PROFILE_KEYS: [&str; 40] = [
    "profile-id",
    "profile_id",
    "name",
    "description",
    "schema-version",
    "schema_version",
    "role",
    "roles",
    "avatar-image",
    "avatar_image",
    "capabilities",
    "routing-priority",
    "routing_priority",
    "max-concurrent-tasks",
    "max_concurrent_tasks",
    "model",
    "preferred_model",
    "effort",
    "purpose",
    "specialization",
    "collaboration",
    "mode-defaults",
    "mode_defaults",
    "initialization-declaration",
    "initialization_declaration",
    "sentinel",
    "specialization-context",
    "specialization_context",
    "self-review-protocol",
    "self_review_protocol",
    "applies_to_languages",
    "directive-references",
    "directive_references",
    "tactic-references",
    "tactic_references",
    "toolguide-references",
    "toolguide_references",
    "styleguide-references",
    "styleguide_references",
    "excluding",
];

/// Relationship fields retired in the DRG hard cutover.
const RETIRED_RELATIONSHIP_KEYS: [&str; 4] = [
    "specializes_from",
    "specializes-from",
    "enhances",
    "overrides",
];

/// One file the repository refused to load, with the reason.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkippedProfile {
    /// The refused file.
    pub path: PathBuf,
    /// Its declared id, when one could be read.
    pub profile_id: Option<String>,
    /// Why it was skipped.
    pub summary: String,
}

/// One loaded agent profile.
#[derive(Clone, Debug)]
pub struct AgentProfile {
    /// The profile id.
    pub id: String,
    /// The declaring file.
    pub path: PathBuf,
    /// The profile body after the layer merge (before lineage composition).
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

    /// The purpose, when one is declared.
    pub fn purpose(&self) -> Option<&str> {
        self.body
            .get("purpose")
            .and_then(Value::as_str)
            .filter(|purpose| !purpose.is_empty())
    }

    /// The primary role (`roles[0]`), when declared.
    pub fn role(&self) -> Option<&str> {
        self.body
            .get("roles")
            .and_then(Value::as_sequence)
            .and_then(|roles| roles.first())
            .and_then(Value::as_str)
            .or_else(|| self.body.get("role").and_then(Value::as_str))
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
    layers: Vec<(usize, PathBuf, ProfileLayer)>,
    profiles: BTreeMap<String, (AgentProfile, usize)>,
    skipped: Vec<SkippedProfile>,
    scope_filtered_ids: BTreeSet<String>,
    active_languages: Option<Vec<String>>,
}

/// The layer a profile was loaded from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileLayer {
    /// `packs/built-in/agent_profiles/`.
    BuiltIn,
    /// A configured org pack.
    Org,
    /// `.kittify/charter-packs/agent_profiles/`.
    Project,
}

impl ProfileLayer {
    fn as_str(self) -> &'static str {
        match self {
            Self::BuiltIn => "built-in",
            Self::Org => "org",
            Self::Project => "project",
        }
    }
}

impl ProfileRepository {
    /// Load profiles from layers in overlay order: built-in, org packs in
    /// declaration order, then project.
    pub fn load(
        built_in_root: &Path,
        org_roots: &[PathBuf],
        project_root: Option<&Path>,
    ) -> Result<Self, DoctrineError> {
        Self::load_with_languages(built_in_root, org_roots, project_root, None)
    }

    /// Load with an active-language filter (`None` disables the filter).
    pub fn load_with_languages(
        built_in_root: &Path,
        org_roots: &[PathBuf],
        project_root: Option<&Path>,
        active_languages: Option<&[String]>,
    ) -> Result<Self, DoctrineError> {
        let active_languages = active_languages.map(normalize_languages);
        let mut layers: Vec<(usize, PathBuf, ProfileLayer)> = Vec::new();
        layers.push((
            0,
            built_in_root.join("agent_profiles"),
            ProfileLayer::BuiltIn,
        ));
        for (position, root) in org_roots.iter().enumerate() {
            layers.push((position + 1, root.join("agent_profiles"), ProfileLayer::Org));
        }
        if let Some(project) = project_root {
            layers.push((
                org_roots.len() + 1,
                project.join("agent_profiles"),
                ProfileLayer::Project,
            ));
        }
        let mut repository = Self {
            layers,
            profiles: BTreeMap::new(),
            skipped: Vec::new(),
            scope_filtered_ids: BTreeSet::new(),
            active_languages,
        };
        let mut built_in_bodies: BTreeMap<String, Mapping> = BTreeMap::new();
        for (layer, directory, layer_name) in repository.layers.clone() {
            for path in sorted_files(&directory, ".agent.yaml") {
                repository.load_one(&path, layer, layer_name, &mut built_in_bodies);
            }
        }
        Ok(repository)
    }

    fn load_one(
        &mut self,
        path: &Path,
        layer: usize,
        layer_name: ProfileLayer,
        built_in_bodies: &mut BTreeMap<String, Mapping>,
    ) {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => {
                self.skip(path, None, format!("read error: {error}"));
                return;
            }
        };
        let value: Value = match serde_yaml_ng::from_str(&text) {
            Ok(value) => value,
            Err(error) => {
                self.skip(path, None, format!("YAML error: {error}"));
                return;
            }
        };
        if value.is_null() {
            self.skip(path, None, "empty profile file".to_owned());
            return;
        }
        let Some(mapping) = value.as_mapping() else {
            self.skip(path, None, "expected a YAML mapping".to_owned());
            return;
        };
        let declared_id = ["profile-id", "profile_id"]
            .iter()
            .find_map(|key| mapping.get(*key).and_then(Value::as_str))
            .filter(|id| !id.is_empty())
            .map(str::to_owned);
        if let Some(key) = mapping
            .keys()
            .filter_map(Value::as_str)
            .find(|key| RETIRED_RELATIONSHIP_KEYS.contains(key))
        {
            self.skip(
                path,
                declared_id,
                format!(
                    "retired relationship field {key:?}; author lineage as a DRG specializes_from edge"
                ),
            );
            return;
        }
        let Some(profile_id) = declared_id else {
            self.skip(
                path,
                None,
                "missing required 'profile-id' (a profile is never named after its file stem)"
                    .to_owned(),
            );
            return;
        };

        let merged = if layer_name != ProfileLayer::BuiltIn {
            match built_in_bodies.get(&profile_id) {
                Some(base) => deep_merge(base, mapping),
                None => mapping.clone(),
            }
        } else {
            mapping.clone()
        };
        if let Err(error) = validate_profile(&merged) {
            self.skip(path, Some(profile_id), error);
            return;
        }
        let applies = merged
            .get("applies_to_languages")
            .and_then(Value::as_sequence)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if !applies_to_languages_match(&applies, self.active_languages.as_deref()) {
            self.scope_filtered_ids.insert(profile_id);
            return;
        }
        let profile = AgentProfile {
            id: profile_id.clone(),
            path: path.to_path_buf(),
            body: merged.clone(),
            lineage: Vec::new(),
        };
        if layer_name == ProfileLayer::BuiltIn {
            built_in_bodies.insert(profile_id.clone(), merged);
        }
        self.scope_filtered_ids.remove(&profile_id);
        self.profiles.insert(profile_id, (profile, layer));
    }

    fn skip(&mut self, path: &Path, profile_id: Option<String>, summary: String) {
        self.skipped.push(SkippedProfile {
            path: path.to_path_buf(),
            profile_id,
            summary,
        });
    }

    /// Profiles that were refused, in load order.
    pub fn skipped(&self) -> &[SkippedProfile] {
        &self.skipped
    }

    /// Profile ids present but filtered out by `applies_to_languages`.
    pub fn scope_filtered_ids(&self) -> &BTreeSet<String> {
        &self.scope_filtered_ids
    }

    /// All profile ids, sorted.
    pub fn ids(&self) -> Vec<String> {
        self.profiles.keys().cloned().collect()
    }

    /// The raw (uncomposed) profile record.
    pub fn get(&self, id: &str) -> Option<&AgentProfile> {
        self.profiles.get(id).map(|(profile, _)| profile)
    }

    /// The provenance layer of `id`.
    pub fn layer_of(&self, id: &str) -> Option<ProfileLayer> {
        self.profiles
            .get(id)
            .and_then(|(_, layer)| self.layers.iter().find(|(index, _, _)| index == layer))
            .map(|(_, _, layer)| *layer)
    }

    /// The provenance layer label of `id`.
    pub fn provenance(&self, id: &str) -> Option<&'static str> {
        self.layer_of(id).map(ProfileLayer::as_str)
    }

    /// Resolve `id` with its `specializes_from` lineage composed and its own
    /// `excluding` declaration applied.
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
        if let Some(excluding) = chain[0].body.get("excluding") {
            merged = apply_excluding(merged, excluding);
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
        // An authored project field is only a compatibility fallback when the
        // graph has no edge; the graph remains the authority.
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

/// Field-level merge of a higher layer's profile data into its built-in base.
///
/// Nested mappings merge recursively; every other value (including a list)
/// is replaced by the higher layer's value. This is the reference
/// `_merge_profiles` deep merge, not the lineage union merge.
fn deep_merge(base: &Mapping, overlay: &Mapping) -> Mapping {
    let mut merged = base.clone();
    for (key, overlay_value) in overlay {
        match (merged.get(key), overlay_value) {
            (Some(Value::Mapping(base_map)), Value::Mapping(overlay_map)) => {
                merged.insert(
                    key.clone(),
                    Value::Mapping(deep_merge(base_map, overlay_map)),
                );
            }
            _ => {
                merged.insert(key.clone(), overlay_value.clone());
            }
        }
    }
    merged
}

/// The reference `AgentProfile` schema's load-time requirements.
fn validate_profile(mapping: &Mapping) -> Result<(), String> {
    let allowed: BTreeSet<&str> = PROFILE_KEYS.into_iter().collect();
    for key in mapping.keys() {
        let Some(key) = key.as_str() else {
            return Err("profile keys must be strings".to_owned());
        };
        if !allowed.contains(key) && !RETIRED_RELATIONSHIP_KEYS.contains(&key) {
            return Err(format!("unknown profile field {key:?}"));
        }
    }
    for (field, rule) in [
        ("name", "a non-empty string"),
        ("purpose", "a non-empty string"),
    ] {
        if mapping
            .get(field)
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            return Err(format!("{field:?} must be {rule}"));
        }
    }
    if mapping
        .get("profile-id")
        .or_else(|| mapping.get("profile_id"))
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        return Err("'profile-id' must be a non-empty string".to_owned());
    }
    let has_role = mapping.get("role").is_some();
    let has_roles = mapping.get("roles").is_some();
    if has_role && has_roles {
        return Err(
            "declares both the deprecated scalar 'role:' and the canonical 'roles:'; keep 'roles:'"
                .to_owned(),
        );
    }
    let roles = match (has_roles, has_role) {
        (true, _) => mapping
            .get("roles")
            .and_then(Value::as_sequence)
            .cloned()
            .unwrap_or_default(),
        (false, true) => vec![mapping.get("role").cloned().unwrap_or(Value::Null)],
        (false, false) => Vec::new(),
    };
    if roles.is_empty() || roles.iter().any(|role| role.as_str().is_none()) {
        return Err("'roles' must be a non-empty list of strings".to_owned());
    }
    let Some(specialization) = mapping.get("specialization").and_then(Value::as_mapping) else {
        return Err("'specialization' must be a mapping with a 'primary-focus'".to_owned());
    };
    // `specialization.primary-focus` (alias) or `primary_focus` (populate-by-name).
    let primary_focus = specialization
        .get("primary-focus")
        .or_else(|| specialization.get("primary_focus"))
        .and_then(Value::as_str);
    if primary_focus.is_none_or(str::is_empty) {
        return Err("'specialization.primary-focus' must be a non-empty string".to_owned());
    }
    if let Some(priority) = mapping
        .get("routing-priority")
        .or_else(|| mapping.get("routing_priority"))
    {
        let Some(priority) = priority.as_i64() else {
            return Err("'routing-priority' must be an integer".to_owned());
        };
        if !(0..=100).contains(&priority) {
            return Err("'routing-priority' must be between 0 and 100".to_owned());
        }
    }
    if let Some(tasks) = mapping
        .get("max-concurrent-tasks")
        .or_else(|| mapping.get("max_concurrent_tasks"))
    {
        let Some(tasks) = tasks.as_i64() else {
            return Err("'max-concurrent-tasks' must be an integer".to_owned());
        };
        if tasks <= 0 {
            return Err("'max-concurrent-tasks' must be greater than 0".to_owned());
        }
    }
    Ok(())
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
                // The reference `_union_merge` updates nested sections one
                // level deep; only the layer merge (`_merge_profiles`) recurses.
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

/// Apply a leaf profile's `excluding` declaration to its merged body.
///
/// A list form removes whole fields; a mapping form removes the named values
/// from the list field they name (matched by the reference item key).
fn apply_excluding(mut merged: Mapping, excluding: &Value) -> Mapping {
    match excluding {
        Value::Sequence(fields) => {
            for field in fields {
                if let Some(field) = field.as_str() {
                    merged.remove(field);
                }
            }
        }
        Value::Mapping(fields) => {
            for (field, values) in fields {
                let Some(field) = field.as_str() else {
                    continue;
                };
                let Some(values) = values.as_sequence() else {
                    continue;
                };
                let remove: BTreeSet<String> = values
                    .iter()
                    .map(|value| match value {
                        Value::String(text) => text.clone(),
                        other => format!("{other:?}"),
                    })
                    .collect();
                let Some(items) = merged.get(field).and_then(Value::as_sequence) else {
                    continue;
                };
                let kept: Vec<Value> = items
                    .iter()
                    .filter(|item| !remove.contains(&item_key(item)))
                    .cloned()
                    .collect();
                merged.insert(field.into(), Value::Sequence(kept));
            }
        }
        _ => {}
    }
    merged
}

/// Normalize a language list: lowercase, de-duplicated, non-empty.
pub fn normalize_languages(values: &[String]) -> Vec<String> {
    let mut normalized = Vec::new();
    for value in values {
        let item = value.trim().to_lowercase();
        if !item.is_empty() && !normalized.contains(&item) {
            normalized.push(item);
        }
    }
    normalized
}

/// Whether a profile scoped to `artifact_languages` loads for `active`.
///
/// Unscoped profiles always load; `None` active disables filtering; an
/// explicitly empty/`unknown` active set admits no scoped profile; `any`/`all`
/// sentinels make the profile unscoped (defense in depth against a file that
/// bypassed authoring validation). Family expansion and the reserved/sentinel
/// token sets are the canonical `language_scope` authorities, never a local
/// copy.
pub fn applies_to_languages_match(
    artifact_languages: &[String],
    active_languages: Option<&[String]>,
) -> bool {
    let raw: BTreeSet<String> = normalize_languages(artifact_languages)
        .into_iter()
        .collect();
    if raw.is_empty() {
        return true;
    }
    let artifact_scope: BTreeSet<String> = raw
        .into_iter()
        .filter(|token| !crate::language_scope::RESERVED_LANGUAGE_TOKENS.contains(&token.as_str()))
        .collect();
    if artifact_scope.is_empty() {
        return false;
    }
    if artifact_scope
        .iter()
        .all(|token| crate::language_scope::SENTINEL_LANGUAGE_TOKENS.contains(&token.as_str()))
    {
        return true;
    }
    let Some(active) = active_languages else {
        return true;
    };
    let active_scope: BTreeSet<String> = normalize_languages(active)
        .into_iter()
        .filter(|token| !crate::language_scope::RESERVED_LANGUAGE_TOKENS.contains(&token.as_str()))
        .collect();
    if active_scope.is_empty() {
        return false;
    }
    let expanded: BTreeSet<String> =
        crate::language_scope::expand_language_families(active_scope.iter().map(String::as_str))
            .into_iter()
            .collect();
    !artifact_scope.is_disjoint(&expanded)
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

    fn profile(id: &str, body: &str) -> String {
        format!(
            "profile-id: {id}\nname: {id}\npurpose: p\nroles: [reviewer]\nspecialization:\n  primary-focus: review\n{body}"
        )
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
    fn review_regression_all_pinned_builtin_profiles_load() {
        let built_in = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs/built-in");
        let profiles = ProfileRepository::load(&built_in, &[], None).unwrap();
        assert!(profiles.skipped().is_empty(), "{:?}", profiles.skipped());
        assert_eq!(profiles.ids().len(), 26);
        for id in [
            "drupal-dries",
            "frontend-freddy",
            "java-jenny",
            "node-norris",
            "python-pedro",
        ] {
            assert!(profiles.get(id).is_some(), "missing {id}");
        }
    }

    #[test]
    fn same_id_overlay_deep_merges_into_the_built_in_profile() {
        let directory = tempfile::tempdir().unwrap();
        let built_in = directory.path().join("built-in");
        let org = directory.path().join("org");
        write(
            &built_in.join("agent_profiles/ivan.agent.yaml"),
            &profile(
                "ivan",
                "description: kept\ncapabilities: [a, b]\nspecialization-context:\n  languages: [python]\n  frameworks: [django]\n",
            ),
        );
        write(
            &org.join("agent_profiles/ivan.agent.yaml"),
            "profile-id: ivan\ndescription: Org\nspecialization-context:\n  frameworks: [flask]\n",
        );
        let repository = ProfileRepository::load(&built_in, &[org], None).unwrap();
        let loaded = repository.get("ivan").unwrap();
        assert_eq!(loaded.name(), Some("ivan"));
        assert_eq!(
            loaded.body.get("description").and_then(Value::as_str),
            Some("Org")
        );
        assert_eq!(loaded.list("capabilities").len(), 2);
        let context = loaded.body.get("specialization-context").unwrap();
        assert_eq!(
            context
                .get("languages")
                .unwrap()
                .as_sequence()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            context.get("frameworks").unwrap().as_sequence().unwrap()[0].as_str(),
            Some("flask")
        );
        assert_eq!(repository.provenance("ivan"), Some("org"));
    }

    #[test]
    fn missing_ids_and_invalid_schemas_are_skipped_not_renamed() {
        let directory = tempfile::tempdir().unwrap();
        let built_in = directory.path().join("built-in");
        write(
            &built_in.join("agent_profiles/valid.agent.yaml"),
            &profile("valid", ""),
        );
        write(
            &built_in.join("agent_profiles/stem-named.agent.yaml"),
            "name: No id\npurpose: p\nroles: [reviewer]\nspecialization:\n  primary-focus: x\n",
        );
        write(
            &built_in.join("agent_profiles/unknown-field.agent.yaml"),
            &profile("unknown-field", "mystery: true\n"),
        );
        write(
            &built_in.join("agent_profiles/both-roles.agent.yaml"),
            "profile-id: both-roles\nname: B\npurpose: p\nrole: reviewer\nroles: [reviewer]\nspecialization:\n  primary-focus: x\n",
        );
        let repository = ProfileRepository::load(&built_in, &[], None).unwrap();
        assert!(repository.get("valid").is_some());
        assert!(repository.get("stem-named").is_none());
        assert!(repository.get("unknown-field").is_none());
        assert!(repository.get("both-roles").is_none());
        let summaries: Vec<&str> = repository
            .skipped()
            .iter()
            .map(|skip| skip.summary.as_str())
            .collect();
        assert!(
            summaries
                .iter()
                .any(|summary| summary.contains("profile-id"))
        );
        assert!(
            summaries
                .iter()
                .any(|summary| summary.contains("unknown profile field"))
        );
        assert!(summaries.iter().any(|summary| summary.contains("both")));
    }

    #[test]
    fn language_scoping_filters_and_records_the_drop() {
        let directory = tempfile::tempdir().unwrap();
        let built_in = directory.path().join("built-in");
        write(
            &built_in.join("agent_profiles/python.agent.yaml"),
            &profile("python", "applies_to_languages: [python]\n"),
        );
        write(
            &built_in.join("agent_profiles/jvm.agent.yaml"),
            &profile("jvm", "applies_to_languages: [jvm]\n"),
        );
        write(
            &built_in.join("agent_profiles/unscoped.agent.yaml"),
            &profile("unscoped", ""),
        );
        let active = vec!["kotlin".to_owned()];
        let repository =
            ProfileRepository::load_with_languages(&built_in, &[], None, Some(&active)).unwrap();
        assert!(repository.get("jvm").is_some());
        assert!(repository.get("unscoped").is_some());
        assert!(repository.get("python").is_none());
        assert!(repository.scope_filtered_ids().contains("python"));

        let unknown = vec!["unknown".to_owned()];
        let repository =
            ProfileRepository::load_with_languages(&built_in, &[], None, Some(&unknown)).unwrap();
        assert!(repository.get("jvm").is_none());
        assert!(repository.get("unscoped").is_some());
    }

    #[test]
    fn lineage_composes_references_from_root_to_leaf() {
        let directory = tempfile::tempdir().unwrap();
        let built_in = directory.path().join("built-in");
        write(
            &built_in.join("agent_profiles/base.agent.yaml"),
            &profile(
                "base",
                "directive-references:\n  - code: 001\ncapabilities: [a]\n",
            ),
        );
        write(
            &built_in.join("agent_profiles/leaf.agent.yaml"),
            &profile(
                "leaf",
                "directive-references:\n  - code: 002\ncapabilities: [b]\ndescription: leaf\n",
            ),
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
        assert_eq!(profile.list("capabilities").len(), 2);
        assert_eq!(profile.purpose(), Some("p"));
        assert_eq!(
            profile.body.get("description").unwrap().as_str(),
            Some("leaf")
        );
    }

    #[test]
    fn excluding_removes_fields_or_values_from_the_merged_result() {
        let directory = tempfile::tempdir().unwrap();
        let built_in = directory.path().join("built-in");
        write(
            &built_in.join("agent_profiles/base.agent.yaml"),
            &profile("base", "capabilities: [a, b]\ndescription: inherited\n"),
        );
        write(
            &built_in.join("agent_profiles/list.agent.yaml"),
            &profile("list", "excluding: [description]\n"),
        );
        write(
            &built_in.join("agent_profiles/map.agent.yaml"),
            "profile-id: map\nname: map\npurpose: p\nroles: [reviewer]\nspecialization:\n  primary-focus: review\ncapabilities: [a, b]\nexcluding:\n  capabilities: [a]\n",
        );
        let repository = ProfileRepository::load(&built_in, &[], None).unwrap();
        let graph = DrgGraph::new(
            "1.0",
            "x",
            "t",
            vec![
                DrgNode::new(
                    "agent_profile:base",
                    NodeKind::AgentProfile,
                    None,
                    Vec::new(),
                )
                .unwrap(),
                DrgNode::new(
                    "agent_profile:list",
                    NodeKind::AgentProfile,
                    None,
                    Vec::new(),
                )
                .unwrap(),
                DrgNode::new(
                    "agent_profile:map",
                    NodeKind::AgentProfile,
                    None,
                    Vec::new(),
                )
                .unwrap(),
            ],
            vec![
                DrgEdge::new(
                    "agent_profile:list",
                    "agent_profile:base",
                    Relation::SpecializesFrom,
                )
                .unwrap(),
                DrgEdge::new(
                    "agent_profile:map",
                    "agent_profile:base",
                    Relation::SpecializesFrom,
                )
                .unwrap(),
            ],
        )
        .unwrap();
        let list = repository.resolve("list", &graph).unwrap();
        assert!(list.body.get("description").is_none());
        let mapped = repository.resolve("map", &graph).unwrap();
        let capabilities: Vec<&str> = mapped
            .list("capabilities")
            .into_iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(capabilities, ["b"]);
    }

    #[test]
    fn missing_parent_cycle_and_ambiguity_are_refused() {
        let directory = tempfile::tempdir().unwrap();
        let built_in = directory.path().join("built-in");
        write(
            &built_in.join("agent_profiles/leaf.agent.yaml"),
            &profile("leaf", ""),
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
            &profile("base", ""),
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
            &profile("other", ""),
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
    fn profile_channel_reports_suggests_when_clauses() {
        let directory = tempfile::tempdir().unwrap();
        let built_in = directory.path().join("built-in");
        write(
            &built_in.join("agent_profiles/reviewer.agent.yaml"),
            &profile("reviewer", ""),
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
