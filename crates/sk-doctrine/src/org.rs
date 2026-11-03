//! Org-pack fragment loading and the project overlay.
//!
//! Mirrors `src/charter/offering/drg/org_pack_loader.py` and
//! `src/charter/offering/drg/project_scan.py`. An org pack's declared nodes and
//! edges are parsed strictly (unknown fields and unknown kinds fail closed), its
//! `body_path` must stay inside the pack root, and augmentation/lineage fields
//! project into fully-qualified overlay edges.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use sk_charter::{ArtifactKind, Limits, PackTier};

use crate::error::DoctrineError;
use crate::graph::{DrgEdge, DrgGraph, DrgNode, NodeKind, Relation, is_valid_urn};

/// The pack descriptor file name.
pub const PACK_DESCRIPTOR: &str = "pack.yaml";
/// The canonical org DRG fragment.
pub const ORG_FRAGMENT: &str = "drg/fragment.yaml";
const MISSION_TYPE_SINGULAR: &str = "mission_type";
const MISSION_TYPE_PLURAL: &str = "mission_types";
const MISSION_STEP_PLURAL: &str = "mission_steps";

/// Every artifact kind may be declared by an org pack except anti-pattern,
/// which has no standalone file form.
pub fn org_node_kinds() -> Vec<&'static str> {
    let mut plurals: Vec<&'static str> = ArtifactKind::ALL
        .iter()
        .filter(|kind| **kind != ArtifactKind::AntiPattern)
        .map(|kind| {
            if *kind == ArtifactKind::MissionStepContract {
                MISSION_STEP_PLURAL
            } else {
                kind.plural()
            }
        })
        .collect();
    plurals.push(MISSION_TYPE_PLURAL);
    plurals.sort_unstable();
    plurals.dedup();
    plurals
}

/// Resolve an authored plural kind to its canonical plural form.
pub fn canonical_org_kind(raw: &str) -> Option<&'static str> {
    if raw == "mission_step_contracts" || raw == MISSION_STEP_PLURAL {
        return Some(MISSION_STEP_PLURAL);
    }
    if raw == MISSION_TYPE_SINGULAR || raw == MISSION_TYPE_PLURAL {
        return Some(MISSION_TYPE_PLURAL);
    }
    let kind = ArtifactKind::from_plural(raw).ok()?;
    if kind == ArtifactKind::AntiPattern {
        return None;
    }
    Some(if kind == ArtifactKind::MissionStepContract {
        MISSION_STEP_PLURAL
    } else {
        kind.plural()
    })
}

/// Map a canonical org plural to its singular URN kind.
pub fn singular_urn_kind(plural: &str) -> Option<&'static str> {
    if plural == MISSION_STEP_PLURAL {
        return Some(NodeKind::MissionStepContract.as_str());
    }
    if plural == MISSION_TYPE_PLURAL {
        return Some(NodeKind::MissionType.as_str());
    }
    ArtifactKind::from_plural(plural)
        .ok()
        .and_then(|kind| NodeKind::from_prefix(kind.canonical()).map(NodeKind::as_str))
}

/// Kind → plural for the augmentation-eligible kinds.
pub fn augmentation_eligible() -> Vec<(ArtifactKind, &'static str)> {
    ArtifactKind::ALL
        .iter()
        .copied()
        .filter(|kind| {
            !matches!(
                kind,
                ArtifactKind::Template | ArtifactKind::Asset | ArtifactKind::AntiPattern
            )
        })
        .map(|kind| {
            let plural = if kind == ArtifactKind::MissionStepContract {
                "mission_step_contracts"
            } else {
                kind.plural()
            };
            (kind, plural)
        })
        .collect()
}

/// One node declared by an org fragment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrgNode {
    /// Authored id.
    pub id: String,
    /// Canonical plural kind.
    pub kind: String,
    /// Optional title/label.
    pub title: Option<String>,
    /// Optional pack-relative body path.
    pub body_path: Option<String>,
    /// Whether the node was discovered from an artifact file rather than
    /// explicitly declared in `nodes:`.
    pub discovered: bool,
}

/// One edge declared by an org fragment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrgEdge {
    /// Authored source endpoint (bare id or fully-qualified URN).
    pub source: String,
    /// Authored target endpoint.
    pub target: String,
    /// Authored relation label.
    pub relation: String,
    /// The author's rationale, when written.
    pub reason: Option<String>,
    /// Machine provenance for a projection edge (never author text).
    pub generated_reason: Option<String>,
    /// Whether this edge was projected from an artifact field.
    pub generated: bool,
}

/// A loaded org-tier DRG fragment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrgFragment {
    /// Operator-declared pack name.
    pub pack_name: String,
    /// The resolved pack root.
    pub pack_root: PathBuf,
    /// Declaration-order layer index (1..N).
    pub layer_index: usize,
    /// Nodes in declaration/then-discovery order.
    pub nodes: Vec<OrgNode>,
    /// Edges in authored/then-projected order.
    pub edges: Vec<OrgEdge>,
    /// Non-fatal pack diagnostics (for example a missing `pack.yaml` level declaration).
    pub warnings: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawNode {
    id: String,
    kind: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    body_path: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEdge {
    source: String,
    target: String,
    relation: String,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct RawFragment {
    #[serde(default)]
    pack_name: Option<String>,
    #[serde(default)]
    source_kind: Option<String>,
    #[serde(default)]
    source_ref: Option<String>,
    #[serde(default)]
    layer_index: Option<usize>,
    #[serde(default)]
    provenance_marker: Option<String>,
    #[serde(default)]
    nodes: Vec<RawNode>,
    #[serde(default)]
    edges: Vec<RawEdge>,
}

fn check_pack_level(pack_name: &str, pack_root: &Path) -> Result<Option<String>, DoctrineError> {
    let descriptor = pack_root.join(PACK_DESCRIPTOR);
    if !descriptor.is_file() {
        return Ok(Some(format!(
            "PACK_LEVEL_MISSING: org pack {pack_name:?} at {} has no '{PACK_DESCRIPTOR}'; \
             add one declaring 'pack_level: org' (required once the deprecation window closes)",
            pack_root.display()
        )));
    }
    let parsed = sk_charter::load_pack_descriptor(PackTier::Org, pack_root, &Limits::default())?;
    if parsed.level_inferred {
        return Ok(Some(format!(
            "PACK_LEVEL_MISSING: {} has no 'pack_level'; add 'pack_level: org' \
             (required once the deprecation window closes)",
            descriptor.display()
        )));
    }
    Ok(None)
}

/// Read, parse and validate one org pack's DRG fragment.
///
/// The pack root and `drg/fragment.yaml` must exist: a configured pack that
/// ships no graph is an error, never an empty layer. An unknown kind, an
/// undeclared field, a layer-rule violation or a `body_path` escape fails
/// closed with the pack and file named.
pub fn load_org_pack(
    pack_name: &str,
    pack_root: &Path,
    layer_index: usize,
) -> Result<OrgFragment, DoctrineError> {
    if !pack_root.is_dir() {
        return Err(DoctrineError::OrgPackMissing {
            pack: pack_name.to_owned(),
            path: pack_root.to_path_buf(),
        });
    }
    let mut warnings = Vec::new();
    if let Some(warning) = check_pack_level(pack_name, pack_root)? {
        warnings.push(warning);
    }
    let fragment_path = pack_root.join(ORG_FRAGMENT);
    if !fragment_path.is_file() {
        return Err(DoctrineError::OrgFragmentMissing {
            pack: pack_name.to_owned(),
            path: fragment_path,
        });
    }
    let text = std::fs::read_to_string(&fragment_path)
        .map_err(|source| DoctrineError::io("read org fragment", &fragment_path, source))?;
    let raw: RawFragment =
        serde_yaml_ng::from_str(&text).map_err(|error| DoctrineError::InvalidOrgFragment {
            pack: pack_name.to_owned(),
            path: fragment_path.clone(),
            message: error.to_string(),
        })?;

    let mut nodes = Vec::new();
    let mut seen = BTreeSet::new();
    for raw_node in raw.nodes {
        let Some(kind) = canonical_org_kind(&raw_node.kind) else {
            return Err(DoctrineError::InvalidOrgFragment {
                pack: pack_name.to_owned(),
                path: fragment_path.clone(),
                message: format!(
                    "unknown kind {:?}: not in the canonical org-pack kind universe {:?}",
                    raw_node.kind,
                    org_node_kinds()
                ),
            });
        };
        let node = OrgNode {
            id: raw_node.id,
            kind: kind.to_owned(),
            title: raw_node.title,
            body_path: raw_node.body_path,
            discovered: false,
        };
        validate_node(pack_name, pack_root, &node)?;
        if !seen.insert((node.kind.clone(), node.id.clone())) {
            return Err(DoctrineError::InvalidOrgFragment {
                pack: pack_name.to_owned(),
                path: fragment_path.clone(),
                message: format!(
                    "duplicate node declaration for kind {:?} id {:?}",
                    node.kind, node.id
                ),
            });
        }
        nodes.push(node);
    }

    let mut edges = Vec::new();
    for raw_edge in raw.edges {
        edges.push(OrgEdge {
            source: raw_edge.source,
            target: raw_edge.target,
            relation: raw_edge.relation,
            reason: raw_edge.reason,
            generated_reason: None,
            generated: false,
        });
    }
    edges.extend(collect_augmentation_edges(pack_root));
    edges.extend(collect_governance_scope_edges(
        pack_root,
        &fragment_path,
        pack_name,
    )?);

    for node in collect_artifact_nodes(pack_root, &fragment_path, pack_name)? {
        if seen.insert((node.kind.clone(), node.id.clone())) {
            nodes.push(node);
        }
    }

    Ok(OrgFragment {
        pack_name: pack_name.to_owned(),
        pack_root: pack_root.to_path_buf(),
        layer_index,
        nodes,
        edges,
        warnings,
    })
}

fn validate_node(pack_name: &str, pack_root: &Path, node: &OrgNode) -> Result<(), DoctrineError> {
    for token in [
        Some(node.id.as_str()),
        node.title.as_deref(),
        node.body_path.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if token.contains("src/specify_cli/") || token.contains("specify_cli.") {
            return Err(DoctrineError::LayerRuleViolation {
                pack: pack_name.to_owned(),
                node: node.id.clone(),
                token: token.to_owned(),
            });
        }
    }
    if let Some(body_path) = &node.body_path {
        sk_kernel::resolve_within(pack_root, Path::new(body_path)).map_err(|_| {
            DoctrineError::BodyPathEscape {
                pack: pack_name.to_owned(),
                node: node.id.clone(),
                body_path: body_path.clone(),
                root: pack_root.to_path_buf(),
            }
        })?;
    }
    Ok(())
}

fn collect_artifact_nodes(
    pack_root: &Path,
    fragment_path: &Path,
    pack_name: &str,
) -> Result<Vec<OrgNode>, DoctrineError> {
    let mut nodes = Vec::new();
    for kind in ArtifactKind::ALL {
        if kind == ArtifactKind::AntiPattern || kind.glob_pattern().is_empty() {
            continue;
        }
        let plural = if kind == ArtifactKind::MissionStepContract {
            MISSION_STEP_PLURAL
        } else {
            kind.plural()
        };
        let directory = pack_root.join(kind.plural());
        for path in recursive_files(&directory, kind.glob_pattern()) {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(value) = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&text) else {
                continue;
            };
            let field = if kind == ArtifactKind::AgentProfile {
                "profile-id"
            } else {
                "id"
            };
            let Some(id) = value.get(field).and_then(serde_yaml_ng::Value::as_str) else {
                continue;
            };
            let title = value
                .get("title")
                .or_else(|| value.get("name"))
                .and_then(serde_yaml_ng::Value::as_str)
                .map(str::to_owned);
            let body_path = value
                .get("body_path")
                .and_then(serde_yaml_ng::Value::as_str)
                .map(str::to_owned);
            let node = OrgNode {
                id: id.to_owned(),
                kind: canonical_org_kind(plural).unwrap_or(plural).to_owned(),
                title,
                body_path,
                discovered: true,
            };
            validate_node(pack_name, pack_root, &node).map_err(|error| match error {
                DoctrineError::BodyPathEscape {
                    pack,
                    node,
                    body_path,
                    root,
                } => DoctrineError::BodyPathEscape {
                    pack,
                    node,
                    body_path,
                    root,
                },
                other => DoctrineError::InvalidOrgFragment {
                    pack: pack_name.to_owned(),
                    path: path.clone(),
                    message: other.to_string(),
                },
            })?;
            nodes.push(node);
        }
    }
    let _ = (fragment_path, pack_name);
    nodes.sort_by_key(|node| (node.kind.clone(), node.id.clone()));
    Ok(nodes)
}

fn recursive_files(directory: &Path, glob: &str) -> Vec<PathBuf> {
    let suffix = glob.trim_start_matches('*');
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

fn collect_augmentation_edges(pack_root: &Path) -> Vec<OrgEdge> {
    let fields = [
        ("enhances", Relation::Enhances),
        ("overrides", Relation::Overrides),
        ("specializes_from", Relation::SpecializesFrom),
    ];
    let mut edges = Vec::new();
    for (kind, plural) in augmentation_eligible() {
        for path in recursive_files(&pack_root.join(plural), kind.glob_pattern()) {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(value) = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&text) else {
                continue;
            };
            let Some(art_id) = value.get("id").and_then(serde_yaml_ng::Value::as_str) else {
                continue;
            };
            for (field, relation) in fields {
                let Some(target) = value.get(field).and_then(serde_yaml_ng::Value::as_str) else {
                    continue;
                };
                if target.is_empty() {
                    continue;
                }
                edges.push(OrgEdge {
                    source: format!("{}:{art_id}", kind.canonical()),
                    target: format!("{}:{target}", kind.canonical()),
                    relation: relation.as_str().to_owned(),
                    reason: None,
                    generated_reason: Some(format!(
                        "declared via {}.{field} field",
                        kind.canonical()
                    )),
                    generated: true,
                });
            }
        }
    }
    edges
}

const GOVERNANCE_SCOPE_FIELDS: [(&str, &str); 8] = [
    ("selected_directives", "directive"),
    ("selected_tactics", "tactic"),
    ("selected_paradigms", "paradigm"),
    ("selected_styleguides", "styleguide"),
    ("selected_toolguides", "toolguide"),
    ("selected_procedures", "procedure"),
    ("selected_agent_profiles", "agent_profile"),
    ("selected_mission_step_contracts", "mission_step_contract"),
];

fn collect_governance_scope_edges(
    pack_root: &Path,
    fragment_path: &Path,
    pack_name: &str,
) -> Result<Vec<OrgEdge>, DoctrineError> {
    let directory = pack_root.join(MISSION_TYPE_PLURAL);
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    let mut profiles: Vec<PathBuf> = std::fs::read_dir(&directory)
        .map_err(|source| DoctrineError::io("read mission types", &directory, source))?
        .flatten()
        .map(|entry| entry.path().join("governance-profile.yaml"))
        .filter(|path| path.is_file())
        .collect();
    profiles.sort();
    let mut edges = Vec::new();
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    for profile in profiles {
        let text = std::fs::read_to_string(&profile)
            .map_err(|source| DoctrineError::io("read governance profile", &profile, source))?;
        let value: serde_yaml_ng::Value =
            serde_yaml_ng::from_str(&text).map_err(|error| DoctrineError::InvalidOrgFragment {
                pack: pack_name.to_owned(),
                path: profile.clone(),
                message: error.to_string(),
            })?;
        let raw_type = value
            .get("mission_type")
            .or_else(|| value.get("id"))
            .and_then(serde_yaml_ng::Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| {
                profile
                    .parent()
                    .and_then(|parent| parent.file_name())
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
                    .to_owned()
            });
        let source = format!("{MISSION_TYPE_SINGULAR}:{raw_type}");
        for (field, kind) in GOVERNANCE_SCOPE_FIELDS {
            let Some(list) = value.get(field) else {
                continue;
            };
            let Some(items) = list.as_sequence() else {
                return Err(DoctrineError::InvalidOrgFragment {
                    pack: pack_name.to_owned(),
                    path: profile.clone(),
                    message: format!("{field} must be a list of ids"),
                });
            };
            for item in items {
                let Some(raw_id) = item.as_str() else {
                    return Err(DoctrineError::InvalidOrgFragment {
                        pack: pack_name.to_owned(),
                        path: profile.clone(),
                        message: format!("{field} must contain string ids"),
                    });
                };
                let target = artifact_to_urn(kind, raw_id);
                if seen.insert((source.clone(), target.clone())) {
                    edges.push(OrgEdge {
                        source: source.clone(),
                        target,
                        relation: Relation::Scope.as_str().to_owned(),
                        reason: None,
                        generated_reason: Some(format!(
                            "declared via mission_type governance-profile {field}"
                        )),
                        generated: true,
                    });
                }
            }
        }
        let _ = fragment_path;
    }
    Ok(edges)
}

/// Build a URN for any artifact kind, normalising directive ids.
pub fn artifact_to_urn(kind: &str, raw_id: &str) -> String {
    if kind == "directive" {
        format!("directive:{}", normalize_directive_id(raw_id))
    } else {
        format!("{kind}:{raw_id}")
    }
}

/// Normalise a directive id to `DIRECTIVE_NNN` (or uppercase snake case).
pub fn normalize_directive_id(raw: &str) -> String {
    if raw.starts_with("DIRECTIVE_")
        && raw
            .strip_prefix("DIRECTIVE_")
            .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
    {
        return raw.to_owned();
    }
    let digits: String = raw.chars().take_while(char::is_ascii_digit).collect();
    if !digits.is_empty() {
        return format!("DIRECTIVE_{:0>3}", digits);
    }
    raw.to_uppercase().replace('-', "_")
}

/// A project-tier artifact and its validated identity.
#[derive(Clone, Debug)]
pub struct ProjectArtifact {
    /// The authored file.
    pub path: PathBuf,
    /// The DRG node.
    pub node: DrgNode,
    /// The parsed artifact body.
    pub body: serde_yaml_ng::Value,
}

/// Discover project-tier agent profiles, failing loud on a malformed file.
pub fn walk_project_agent_profile_nodes(
    project_root: &Path,
) -> Result<Vec<DrgNode>, DoctrineError> {
    let project_pack = project_root.join(".kittify/charter-packs");
    let directory = project_pack.join(ArtifactKind::AgentProfile.project_dir());
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    let mut nodes = Vec::new();
    let mut seen: BTreeMap<String, PathBuf> = BTreeMap::new();
    for path in recursive_files(&directory, "*.agent.yaml") {
        let text = std::fs::read_to_string(&path)
            .map_err(|source| DoctrineError::io("read project profile", &path, source))?;
        let value: serde_yaml_ng::Value =
            serde_yaml_ng::from_str(&text).map_err(|error| DoctrineError::InvalidProfile {
                path: path.clone(),
                message: error.to_string(),
            })?;
        let profile_id = ["profile-id", "profile_id"]
            .iter()
            .find_map(|key| value.get(*key).and_then(serde_yaml_ng::Value::as_str))
            .filter(|id| !id.is_empty())
            .ok_or_else(|| DoctrineError::InvalidProfile {
                path: path.clone(),
                message: "missing a non-empty 'profile-id' field".to_owned(),
            })?;
        let urn = format!("agent_profile:{profile_id}");
        if !is_valid_urn(&urn) {
            return Err(DoctrineError::InvalidProfile {
                path: path.clone(),
                message: format!("profile-id {profile_id:?} does not form a valid URN"),
            });
        }
        if let Some(previous) = seen.insert(profile_id.to_owned(), path.clone()) {
            return Err(DoctrineError::InvalidProfile {
                path: path.clone(),
                message: format!(
                    "duplicate project profile-id {profile_id:?}; also declared by {}",
                    previous.display()
                ),
            });
        }
        let label = value
            .get("name")
            .and_then(serde_yaml_ng::Value::as_str)
            .map(str::to_owned);
        nodes.push(
            DrgNode::new(urn, NodeKind::AgentProfile, label, Vec::new())?
                .with_provenance(Some("project".to_owned())),
        );
    }
    Ok(nodes)
}

/// The schema surface of one direct-write project artifact kind.
struct ProjectArtifactSchema {
    /// Required top-level keys.
    required: &'static [&'static str],
    /// Every top-level key the kind's model declares (`extra="forbid"`).
    allowed: &'static [&'static str],
}

fn project_artifact_schema(kind: ArtifactKind) -> Option<ProjectArtifactSchema> {
    let schema = match kind {
        ArtifactKind::Directive => ProjectArtifactSchema {
            required: &["id", "schema_version", "title", "intent", "enforcement"],
            allowed: &[
                "id",
                "schema_version",
                "title",
                "intent",
                "enforcement",
                "scope",
                "procedures",
                "integrity_rules",
                "validation_criteria",
                "explicit_allowances",
                "references",
            ],
        },
        ArtifactKind::Tactic => ProjectArtifactSchema {
            required: &["id", "schema_version", "name", "steps"],
            allowed: &[
                "id",
                "schema_version",
                "name",
                "purpose",
                "steps",
                "failure_modes",
                "applies_to_languages",
                "references",
                "notes",
            ],
        },
        ArtifactKind::Styleguide => ProjectArtifactSchema {
            required: &["id", "schema_version", "title", "scope", "principles"],
            allowed: &[
                "id",
                "schema_version",
                "title",
                "scope",
                "principles",
                "patterns",
                "anti_patterns",
                "tooling",
                "quality_test",
                "applies_to_languages",
                "references",
                "structural_lint_config",
            ],
        },
        ArtifactKind::Procedure => ProjectArtifactSchema {
            required: &[
                "schema_version",
                "id",
                "name",
                "purpose",
                "entry_condition",
                "exit_condition",
                "steps",
            ],
            allowed: &[
                "schema_version",
                "id",
                "name",
                "purpose",
                "entry_condition",
                "exit_condition",
                "steps",
                "anti_patterns",
                "applies_to_languages",
                "notes",
                "references",
            ],
        },
        ArtifactKind::AgentProfile => ProjectArtifactSchema {
            required: &["profile-id", "name", "roles", "purpose", "specialization"],
            allowed: &[
                "profile-id",
                "name",
                "description",
                "schema-version",
                "roles",
                "avatar-image",
                "capabilities",
                "routing-priority",
                "max-concurrent-tasks",
                "model",
                "effort",
                "purpose",
                "specialization",
                "collaboration",
                "mode-defaults",
                "initialization-declaration",
                "sentinel",
                "specialization-context",
                "applies_to_languages",
                "directive-references",
                "tactic-references",
                "toolguide-references",
                "styleguide-references",
                "self-review-protocol",
                "excluding",
            ],
        },
        _ => return None,
    };
    Some(schema)
}

fn validate_project_artifact(
    path: &Path,
    kind: ArtifactKind,
    value: &serde_yaml_ng::Value,
) -> Result<(), DoctrineError> {
    let Some(schema) = project_artifact_schema(kind) else {
        return Ok(());
    };
    let Some(mapping) = value.as_mapping() else {
        return Err(DoctrineError::InvalidProfile {
            path: path.to_path_buf(),
            message: "expected a YAML mapping".to_owned(),
        });
    };
    let mut problems = Vec::new();
    for key in mapping.keys() {
        let Some(key) = key.as_str() else {
            problems.push("a non-string top-level key".to_owned());
            continue;
        };
        if !schema.allowed.contains(&key) {
            problems.push(format!("unknown field {key:?}"));
        }
    }
    for key in schema.required {
        match mapping.get(*key) {
            None | Some(serde_yaml_ng::Value::Null) => {
                problems.push(format!("missing required field {key:?}"));
            }
            Some(serde_yaml_ng::Value::String(text)) if text.trim().is_empty() => {
                problems.push(format!("required field {key:?} is empty"));
            }
            Some(
                serde_yaml_ng::Value::String(_)
                | serde_yaml_ng::Value::Mapping(_)
                | serde_yaml_ng::Value::Sequence(_),
            ) => {}
            Some(_) => problems.push(format!("required field {key:?} has the wrong type")),
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(DoctrineError::InvalidProfile {
            path: path.to_path_buf(),
            message: problems.join("; "),
        })
    }
}

/// Scan the five direct-write project artifact kinds.
///
/// Each file is contained under the project pack root (a symlink that escapes
/// it is refused at this boundary) and validated against its kind's schema
/// before its identity is minted, exactly as the reference `project_scan`
/// does. A malformed file is never silently accepted as a project node.
pub fn scan_project_artifacts(project_root: &Path) -> Result<Vec<ProjectArtifact>, DoctrineError> {
    let project_pack = project_root.join(".kittify/charter-packs");
    let pack_root = project_pack
        .canonicalize()
        .unwrap_or_else(|_| project_pack.clone());
    let mut artifacts = Vec::new();
    let mut seen: BTreeMap<String, PathBuf> = BTreeMap::new();
    for kind in [
        ArtifactKind::Directive,
        ArtifactKind::Tactic,
        ArtifactKind::Styleguide,
        ArtifactKind::Procedure,
        ArtifactKind::AgentProfile,
    ] {
        let directory = project_pack.join(kind.project_dir());
        for path in recursive_files(&directory, kind.glob_pattern()) {
            let resolved = path
                .canonicalize()
                .map_err(|source| DoctrineError::io("resolve project artifact", &path, source))?;
            if !resolved.starts_with(&pack_root) {
                return Err(DoctrineError::InvalidProfile {
                    path: path.clone(),
                    message: format!(
                        "escapes the project pack root {} (resolved to {})",
                        pack_root.display(),
                        resolved.display()
                    ),
                });
            }
            let text = std::fs::read_to_string(&resolved)
                .map_err(|source| DoctrineError::io("read project artifact", &path, source))?;
            let value: serde_yaml_ng::Value =
                serde_yaml_ng::from_str(&text).map_err(|error| DoctrineError::InvalidProfile {
                    path: path.clone(),
                    message: error.to_string(),
                })?;
            validate_project_artifact(&path, kind, &value)?;
            let field = if kind == ArtifactKind::AgentProfile {
                "profile-id"
            } else {
                "id"
            };
            let Some(id) = value.get(field).and_then(serde_yaml_ng::Value::as_str) else {
                return Err(DoctrineError::InvalidProfile {
                    path: path.clone(),
                    message: format!("missing a non-empty {field:?} field"),
                });
            };
            let urn = artifact_to_urn(kind.canonical(), id);
            if let Some(previous) = seen.insert(urn.clone(), path.clone()) {
                return Err(DoctrineError::InvalidProfile {
                    path: path.clone(),
                    message: format!(
                        "duplicate project artifact {urn}; also declared by {}",
                        previous.display()
                    ),
                });
            }
            let label = value
                .get("name")
                .or_else(|| value.get("title"))
                .and_then(serde_yaml_ng::Value::as_str)
                .map(str::to_owned);
            let node = DrgNode::new(
                urn,
                NodeKind::from_prefix(kind.canonical()).unwrap(),
                label,
                Vec::new(),
            )?
            .with_provenance(Some("project".to_owned()));
            artifacts.push(ProjectArtifact {
                path,
                node,
                body: value,
            });
        }
    }
    Ok(artifacts)
}

/// Project authored profile references into edges, reporting unresolved ones.
pub fn project_reference_edges(
    artifacts: &[ProjectArtifact],
    available: &[DrgNode],
) -> (Vec<DrgEdge>, Vec<String>) {
    let universe: BTreeSet<&str> = available.iter().map(DrgNode::urn).collect();
    let mut edges: BTreeMap<(String, String, Relation), DrgEdge> = BTreeMap::new();
    let mut warnings = Vec::new();
    for artifact in artifacts {
        if artifact.node.kind() != NodeKind::AgentProfile {
            continue;
        }
        // The four reference families project exactly as the extractor does:
        // directive and tactic references are eager `requires`; toolguide and
        // styleguide references are link-delivered `suggests` (so cascade
        // closure and profile-channel semantics follow the authored intent).
        // Directive edges are minted bare; the other families carry the
        // authored rationale.
        let mut candidates: Vec<(String, Relation, Option<String>)> = Vec::new();
        for (field, kind, relation, carries_reason, id_field) in [
            (
                "directive-references",
                "directive",
                Relation::Requires,
                false,
                "code",
            ),
            (
                "tactic-references",
                "tactic",
                Relation::Requires,
                true,
                "id",
            ),
            (
                "styleguide-references",
                "styleguide",
                Relation::Suggests,
                true,
                "id",
            ),
            (
                "toolguide-references",
                "toolguide",
                Relation::Suggests,
                true,
                "id",
            ),
        ] {
            let Some(items) = artifact.body.get(field) else {
                continue;
            };
            let Some(items) = items.as_sequence() else {
                return (
                    Vec::new(),
                    vec![format!(
                        "{} declares {field:?} as a non-list; author it as a list of references",
                        artifact.node.urn()
                    )],
                );
            };
            for item in items {
                let id = item
                    .get(id_field)
                    .and_then(serde_yaml_ng::Value::as_str)
                    .or_else(|| item.as_str());
                let Some(id) = id.filter(|id| !id.is_empty()) else {
                    continue;
                };
                let reason = carries_reason
                    .then(|| {
                        item.get("rationale")
                            .and_then(serde_yaml_ng::Value::as_str)
                            .map(str::to_owned)
                    })
                    .flatten();
                candidates.push((artifact_to_urn(kind, id), relation, reason));
            }
        }
        if let Some(procedures) = artifact
            .body
            .get("collaboration")
            .and_then(|value| value.get("operating-procedures"))
        {
            for item in procedures.as_sequence().into_iter().flatten() {
                if let Some(id) = item.as_str() {
                    candidates.push((artifact_to_urn("procedure", id), Relation::Requires, None));
                }
            }
        }
        for (target, relation, reason) in candidates {
            if !universe.contains(target.as_str()) {
                warnings.push(format!(
                    "{} references unresolved {target}; author or install that artifact, then repeat charter activate --cascade all.",
                    artifact.node.urn()
                ));
                continue;
            }
            let mut edge = DrgEdge::new(artifact.node.urn(), target, relation)
                .expect("project reference endpoints are valid URNs")
                .with_provenance(Some("project".to_owned()));
            if let Some(reason) = reason {
                edge = edge.with_reason(&reason);
            }
            edges.insert(
                (
                    edge.source().to_owned(),
                    edge.target().to_owned(),
                    edge.relation(),
                ),
                edge,
            );
        }
    }
    warnings.sort();
    warnings.dedup();
    (edges.into_values().collect(), warnings)
}

/// Build the project overlay graph from authored project doctrine.
///
/// `wildcard behaviour` (the `available` node universe) is only consulted to
/// decide which reference targets exist; unresolved references are reported, not
/// invented as phantom nodes.
pub fn project_layer(
    project_root: &Path,
    available: &[DrgNode],
) -> Result<(Option<DrgGraph>, Vec<String>), DoctrineError> {
    let project_pack = project_root.join(".kittify/charter-packs");
    if !project_pack.is_dir() {
        return Ok((None, Vec::new()));
    }
    let mut nodes = walk_project_agent_profile_nodes(project_root)?;
    let artifacts = scan_project_artifacts(project_root)?;
    let mut seen: BTreeSet<String> = nodes.iter().map(|node| node.urn().to_owned()).collect();
    for artifact in &artifacts {
        if seen.insert(artifact.node.urn().to_owned()) {
            nodes.push(artifact.node.clone());
        }
    }
    let (edges, warnings) = project_reference_edges(&artifacts, available);
    if nodes.is_empty() && edges.is_empty() {
        return Ok((None, warnings));
    }
    let graph = DrgGraph::new("1.0", "project", "project-scan", nodes, edges)?;
    Ok((Some(graph), warnings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn pack() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write(&root.join("pack.yaml"), "name: acme\npack_level: org\n");
        write(
            &root.join("drg/fragment.yaml"),
            r#"
pack_name: acme
nodes:
  - id: sox
    kind: directives
    title: SOX
edges:
  - source: sox
    target: styleguide:plain
    relation: refines
"#,
        );
        write(
            &root.join("directives/001-arch.directive.yaml"),
            "id: ARCH_001\ntitle: Architecture\n",
        );
        write(
            &root.join("tactics/002-t.tactic.yaml"),
            "id: T_002\noverrides: 001-arch\n",
        );
        directory
    }

    #[test]
    fn loads_authored_and_discovered_nodes_and_projection_edges() {
        let directory = pack();
        let fragment = load_org_pack("acme", directory.path(), 1).unwrap();
        assert_eq!(fragment.pack_name, "acme");
        assert!(
            fragment
                .nodes
                .iter()
                .any(|node| node.id == "sox" && node.kind == "directives")
        );
        assert!(
            fragment
                .nodes
                .iter()
                .any(|node| node.id == "ARCH_001" && node.discovered)
        );
        assert!(fragment.edges.iter().any(|edge| {
            edge.generated
                && edge.relation == "overrides"
                && edge.source == "tactic:T_002"
                && edge.target == "tactic:001-arch"
        }));
    }

    #[test]
    fn unknown_kind_and_unknown_field_fail_closed() {
        let directory = tempfile::tempdir().unwrap();
        write(
            &directory.path().join("pack.yaml"),
            "name: acme\npack_level: org\n",
        );
        write(
            &directory.path().join("drg/fragment.yaml"),
            "nodes:\n  - id: x\n    kind: wibbles\n",
        );
        let error = load_org_pack("acme", directory.path(), 1).unwrap_err();
        assert!(error.to_string().contains("unknown kind"));

        write(
            &directory.path().join("drg/fragment.yaml"),
            "nodes:\n  - id: x\n    kind: directives\n    surprise: true\n",
        );
        let error = load_org_pack("acme", directory.path(), 1).unwrap_err();
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn missing_fragment_and_body_escape_fail_closed() {
        let directory = tempfile::tempdir().unwrap();
        write(
            &directory.path().join("pack.yaml"),
            "name: acme\npack_level: org\n",
        );
        assert!(matches!(
            load_org_pack("acme", directory.path(), 1),
            Err(DoctrineError::OrgFragmentMissing { .. })
        ));
        write(
            &directory.path().join("drg/fragment.yaml"),
            "nodes:\n  - id: x\n    kind: directives\n    body_path: ../escape.md\n",
        );
        assert!(matches!(
            load_org_pack("acme", directory.path(), 1),
            Err(DoctrineError::BodyPathEscape { .. })
        ));
    }

    #[test]
    fn layer_rule_violation_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        write(
            &directory.path().join("pack.yaml"),
            "name: acme\npack_level: org\n",
        );
        write(
            &directory.path().join("drg/fragment.yaml"),
            "nodes:\n  - id: x\n    kind: directives\n    body_path: src/specify_cli/x.py\n",
        );
        assert!(matches!(
            load_org_pack("acme", directory.path(), 1),
            Err(DoctrineError::LayerRuleViolation { .. })
        ));
    }

    #[test]
    fn governance_scope_selections_project_to_scope_edges() {
        let directory = pack();
        write(
            &directory
                .path()
                .join("mission_types/software-dev/governance-profile.yaml"),
            "selected_directives: [001-arch]\nselected_tactics: [002-t]\n",
        );
        let fragment = load_org_pack("acme", directory.path(), 1).unwrap();
        assert!(fragment.edges.iter().any(|edge| {
            edge.generated
                && edge.relation == "scope"
                && edge.source == "mission_type:software-dev"
                && edge.target == "directive:DIRECTIVE_001"
        }));
        assert!(
            fragment
                .edges
                .iter()
                .any(|edge| { edge.relation == "scope" && edge.target == "tactic:002-t" })
        );
    }

    #[test]
    fn project_profiles_and_artifacts_form_a_layer() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write(
            &root.join(".kittify/charter-packs/agent_profiles/p.agent.yaml"),
            "profile-id: p\nname: P\npurpose: review\nroles: [reviewer]\nspecialization:\n  primary-focus: review\ndirective-references:\n  - code: 001-arch\n    rationale: architecture\n",
        );
        write(
            &root.join(".kittify/charter-packs/directive/001-arch.directive.yaml"),
            "id: 001-arch\nschema_version: '1.0'\ntitle: Architecture\nintent: Keep seams\nenforcement: required\n",
        );
        let available = vec![
            DrgNode::new(
                "directive:DIRECTIVE_001",
                NodeKind::Directive,
                None,
                Vec::new(),
            )
            .unwrap(),
        ];
        let (graph, warnings) = project_layer(root, &available).unwrap();
        let graph = graph.unwrap();
        assert!(graph.node("agent_profile:p").is_some());
        assert!(warnings.is_empty());
        assert!(
            graph
                .edges()
                .iter()
                .any(|edge| edge.source() == "agent_profile:p")
        );
    }

    #[test]
    fn project_profile_reference_relations_match_the_extractor() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write(
            &root.join(".kittify/charter-packs/agent_profiles/p.agent.yaml"),
            "profile-id: p\nname: P\npurpose: review\nroles: [reviewer]\nspecialization:\n  primary-focus: review\ndirective-references:\n  - code: directive-a\ntactic-references:\n  - id: tactic-a\n    rationale: how\nstyleguide-references:\n  - id: style-a\n    rationale: style\ntoolguide-references:\n  - id: tool-a\n    rationale: tool\ncollaboration:\n  operating-procedures:\n    - procedure-a\n",
        );
        for (path, body) in [
            (
                "directive/directive-a.directive.yaml",
                "id: DIRECTIVE_A\nschema_version: '1.0'\ntitle: t\nintent: i\nenforcement: required\n".to_owned(),
            ),
            (
                "tactics/tactic-a.tactic.yaml",
                "id: tactic-a\nschema_version: '1.0'\nname: t\nsteps:\n  - title: one\n".to_owned(),
            ),
            (
                "styleguides/style-a.styleguide.yaml",
                "id: style-a\nschema_version: '1.0'\ntitle: t\nscope: code\nprinciples: [p]\n".to_owned(),
            ),
            (
                "procedures/procedure-a.procedure.yaml",
                "schema_version: '1.0'\nid: procedure-a\nname: p\npurpose: why\nentry_condition: e\nexit_condition: x\nsteps:\n  - title: one\n".to_owned(),
            ),
        ] {
            write(&root.join(".kittify/charter-packs").join(path), &body);
        }
        let available = vec![
            DrgNode::new(
                "directive:DIRECTIVE_A",
                NodeKind::Directive,
                None,
                Vec::new(),
            )
            .unwrap(),
            DrgNode::new("tactic:tactic-a", NodeKind::Tactic, None, Vec::new()).unwrap(),
            DrgNode::new("styleguide:style-a", NodeKind::Styleguide, None, Vec::new()).unwrap(),
            DrgNode::new("toolguide:tool-a", NodeKind::Toolguide, None, Vec::new()).unwrap(),
            DrgNode::new(
                "procedure:procedure-a",
                NodeKind::Procedure,
                None,
                Vec::new(),
            )
            .unwrap(),
        ];
        let artifacts = scan_project_artifacts(root).unwrap();
        let (edges, warnings) = project_reference_edges(&artifacts, &available);
        assert!(warnings.is_empty(), "{warnings:?}");
        let find = |target: &str| {
            edges
                .iter()
                .find(|edge| edge.target() == target)
                .unwrap_or_else(|| panic!("no edge to {target}"))
        };
        assert_eq!(find("directive:DIRECTIVE_A").relation(), Relation::Requires);
        assert_eq!(find("directive:DIRECTIVE_A").reason(), None);
        assert_eq!(find("tactic:tactic-a").relation(), Relation::Requires);
        assert_eq!(find("tactic:tactic-a").reason(), Some("how"));
        assert_eq!(find("styleguide:style-a").relation(), Relation::Suggests);
        assert_eq!(find("styleguide:style-a").reason(), Some("style"));
        assert_eq!(find("toolguide:tool-a").relation(), Relation::Suggests);
        assert_eq!(find("toolguide:tool-a").reason(), Some("tool"));
        assert_eq!(find("procedure:procedure-a").relation(), Relation::Requires);
        assert!(
            !edges
                .iter()
                .any(|edge| edge.relation() == Relation::Suggests
                    && edge.target().starts_with("procedure:"))
        );
    }

    #[test]
    fn project_artifact_schema_and_containment_are_enforced() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write(
            &root.join(".kittify/charter-packs/directive/001-arch.directive.yaml"),
            "id: 001-arch\n",
        );
        let error = scan_project_artifacts(root).unwrap_err();
        assert!(matches!(error, DoctrineError::InvalidProfile { .. }));
        assert!(error.to_string().contains("missing required field"));

        write(
            &root.join(".kittify/charter-packs/directive/001-arch.directive.yaml"),
            "id: 001-arch\nschema_version: '1.0'\ntitle: t\nintent: i\nenforcement: required\nsurprise: 1\n",
        );
        let error = scan_project_artifacts(root).unwrap_err();
        assert!(error.to_string().contains("unknown field"));

        #[cfg(unix)]
        {
            let outside = directory.path().join("outside.directive.yaml");
            write(
                &outside,
                "id: OUTSIDE\nschema_version: '1.0'\ntitle: t\nintent: i\nenforcement: required\n",
            );
            std::fs::remove_file(
                root.join(".kittify/charter-packs/directive/001-arch.directive.yaml"),
            )
            .unwrap();
            std::os::unix::fs::symlink(
                &outside,
                root.join(".kittify/charter-packs/directive/outside.directive.yaml"),
            )
            .unwrap();
            let error = scan_project_artifacts(root).unwrap_err();
            assert!(error.to_string().contains("escapes the project pack root"));
        }
    }

    #[test]
    fn malformed_project_profile_names_the_file() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write(
            &root.join(".kittify/charter-packs/agent_profiles/bad.agent.yaml"),
            "name: no id\n",
        );
        let error = walk_project_agent_profile_nodes(root).unwrap_err();
        assert!(error.to_string().contains("bad.agent.yaml"));
    }
}
