//! Mission-template schema and bounded YAML loading.
//!
//! The schema mirrors the subset of the upstream mission template that the
//! deterministic planner consumes. It is deliberately independent of the
//! Python runtime: values are parsed into typed Rust newtypes/enums, and the
//! template is validated before it can be planned from.
//!
//! Loading is bounded and read-only. User-authored YAML is never rewritten.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::error::RuntimeError;
use crate::snapshot::ActorType;

/// Hard upper bound for a mission template file.
pub const MAX_TEMPLATE_BYTES: u64 = 1024 * 1024;

/// The six fixed significance dimensions (V1).
pub const SIGNIFICANCE_DIMENSIONS: [&str; 6] = [
    "user_customer_impact",
    "architectural_system_impact",
    "data_security_compliance_impact",
    "operational_reliability_impact",
    "financial_commercial_impact",
    "cross_team_blast_radius",
];

/// The fixed hard-trigger classes (V1).
pub const HARD_TRIGGER_CLASSES: [&str; 5] = [
    "production_data_destructive",
    "security_privacy_access_control",
    "legal_compliance_regulatory",
    "billing_financial_commitment",
    "architecture_foundation",
];

/// Mission identity metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionMeta {
    pub key: String,
    pub name: String,
    #[serde(deserialize_with = "deserialize_version")]
    pub version: String,
    #[serde(default)]
    pub description: String,
}

/// A deterministic prompt step in the mission DAG.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptStep {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub prompt_template: Option<String>,
    #[serde(default)]
    pub expected_output: Option<String>,
    #[serde(default)]
    pub requires_inputs: Vec<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub raci: Option<RaciAssignment>,
    #[serde(default)]
    pub raci_override_reason: Option<String>,
    #[serde(
        default,
        rename = "agent-profile",
        alias = "agent_profile",
        skip_serializing_if = "Option::is_none"
    )]
    pub agent_profile: Option<String>,
    #[serde(default)]
    pub contract_ref: Option<String>,
}

/// When an audit step is meant to trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerMode {
    Manual,
    PostMerge,
    Both,
}

/// How an audit step affects run progression.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditEnforcement {
    Advisory,
    Blocking,
}

/// Audit checkpoint configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditConfig {
    pub trigger_mode: TriggerMode,
    pub enforcement: AuditEnforcement,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub metadata: Option<Value>,
}

/// Optional significance declaration on an audit step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignificanceBlock {
    pub dimensions: BTreeMap<String, i32>,
    #[serde(default)]
    pub hard_triggers: Vec<String>,
}

/// An audit checkpoint in the mission DAG.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditStep {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub audit: AuditConfig,
    #[serde(default)]
    pub significance: Option<SignificanceBlock>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub raci: Option<RaciAssignment>,
    #[serde(default)]
    pub raci_override_reason: Option<String>,
}

/// One authoring-side RACI role binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RaciRoleSpec {
    pub actor_type: ActorType,
    #[serde(default)]
    pub actor_id: Option<String>,
}

/// Per-step RACI assignment.
///
/// The upstream P0 invariant is enforced structurally here as well: an
/// accountable role is always human.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RaciAssignment {
    pub responsible: RaciRoleSpec,
    pub accountable: RaciRoleSpec,
    #[serde(default)]
    pub consulted: Vec<RaciRoleSpec>,
    #[serde(default)]
    pub informed: Vec<RaciRoleSpec>,
}

/// A validated mission template ready for planning.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MissionTemplate {
    pub mission: MissionMeta,
    #[serde(default)]
    pub steps: Vec<PromptStep>,
    #[serde(default)]
    pub audit_steps: Vec<AuditStep>,
}

impl MissionTemplate {
    /// Validate the whole template, including the combined step/audit DAG.
    ///
    /// Checks, in order: mission metadata, non-emptiness, id/title presence,
    /// unique ids across both step lists, human accountability for supplied
    /// RACI blocks, RACI override-reason pairing, audit significance shape,
    /// duplicate/unknown/self dependencies, and cycles.
    pub fn validate(&self) -> Result<(), RuntimeError> {
        if self.mission.key.trim().is_empty() {
            return Err(RuntimeError::InvalidMission {
                message: "mission.key must be non-empty".to_string(),
            });
        }
        if self.mission.name.trim().is_empty() {
            return Err(RuntimeError::InvalidMission {
                message: "mission.name must be non-empty".to_string(),
            });
        }
        if self.mission.version.trim().is_empty() {
            return Err(RuntimeError::InvalidMission {
                message: "mission.version must be non-empty".to_string(),
            });
        }
        if self.steps.is_empty() && self.audit_steps.is_empty() {
            return Err(RuntimeError::EmptyTemplate);
        }

        let mut ids = BTreeSet::new();
        let mut graph: BTreeMap<String, Vec<String>> = BTreeMap::new();

        for step in &self.steps {
            validate_identity(&step.id, &step.title)?;
            if !ids.insert(step.id.clone()) {
                return Err(RuntimeError::DuplicateStepId {
                    id: step.id.clone(),
                });
            }
            validate_step_constraints(
                step.requires_inputs.iter(),
                &step.id,
                step.raci.as_ref(),
                step.raci_override_reason.as_deref(),
            )?;
            graph.insert(step.id.clone(), step.depends_on.clone());
        }

        for audit in &self.audit_steps {
            validate_identity(&audit.id, &audit.title)?;
            if !ids.insert(audit.id.clone()) {
                return Err(RuntimeError::DuplicateStepId {
                    id: audit.id.clone(),
                });
            }
            validate_step_constraints(
                std::iter::empty(),
                &audit.id,
                audit.raci.as_ref(),
                audit.raci_override_reason.as_deref(),
            )?;
            if let Some(significance) = &audit.significance {
                validate_significance(&audit.id, significance)?;
            }
            graph.insert(audit.id.clone(), audit.depends_on.clone());
        }

        for (step_id, dependencies) in &graph {
            let mut seen = BTreeSet::new();
            for dependency in dependencies {
                if !seen.insert(dependency) {
                    return Err(RuntimeError::DuplicateDependency {
                        step_id: step_id.clone(),
                        dependency: dependency.clone(),
                    });
                }
                if dependency == step_id {
                    return Err(RuntimeError::DependencyCycle {
                        cycle: vec![step_id.clone(), step_id.clone()],
                    });
                }
                if !ids.contains(dependency) {
                    return Err(RuntimeError::UnknownDependency {
                        step_id: step_id.clone(),
                        dependency: dependency.clone(),
                    });
                }
            }
        }

        detect_template_cycle(&graph)
    }
}

/// Load and validate a mission template from `path`.
///
/// The file is size-checked before and after reading, must be UTF-8, and is
/// parsed as YAML. A top-level `mission:` mapping is the canonical shape; a
/// legacy shorthand with top-level `key`/`name`/`version` is accepted and
/// normalised in memory only.
pub fn load_template(path: &Path) -> Result<MissionTemplate, RuntimeError> {
    let text = read_bounded_utf8(path, MAX_TEMPLATE_BYTES)?;
    let root: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&text).map_err(|error| RuntimeError::Malformed {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;

    let root = match root {
        serde_yaml_ng::Value::Null => serde_yaml_ng::Value::Mapping(Default::default()),
        serde_yaml_ng::Value::Mapping(map) => serde_yaml_ng::Value::Mapping(map),
        _ => {
            return Err(RuntimeError::Malformed {
                path: path.to_path_buf(),
                message: "mission template must be a YAML mapping".to_string(),
            });
        }
    };

    let normalised = if root
        .as_mapping()
        .is_some_and(|map| map.contains_key("mission"))
    {
        root
    } else {
        normalise_shorthand(root.as_mapping().expect("mapping checked above"), path)
    };

    let template: MissionTemplate =
        serde_yaml_ng::from_value(normalised).map_err(|error| RuntimeError::Malformed {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
    template.validate()?;
    Ok(template)
}

fn normalise_shorthand(map: &serde_yaml_ng::Mapping, path: &Path) -> serde_yaml_ng::Value {
    let parent_name = path
        .parent()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "mission".to_string());

    let key = stringish(field(map, "key"))
        .or_else(|| stringish(field(map, "name")))
        .unwrap_or(parent_name);
    let name = stringish(field(map, "name")).unwrap_or_else(|| key.clone());
    let version = field(map, "version")
        .cloned()
        .unwrap_or_else(|| serde_yaml_ng::Value::String("1.0.0".to_string()));
    let description = field(map, "description")
        .cloned()
        .unwrap_or_else(|| serde_yaml_ng::Value::String(String::new()));

    let mut mission = serde_yaml_ng::Mapping::new();
    mission.insert(
        serde_yaml_ng::Value::String("key".to_string()),
        serde_yaml_ng::Value::String(key),
    );
    mission.insert(
        serde_yaml_ng::Value::String("name".to_string()),
        serde_yaml_ng::Value::String(name),
    );
    mission.insert(serde_yaml_ng::Value::String("version".to_string()), version);
    mission.insert(
        serde_yaml_ng::Value::String("description".to_string()),
        description,
    );

    let mut normalised = serde_yaml_ng::Mapping::new();
    normalised.insert(
        serde_yaml_ng::Value::String("mission".to_string()),
        serde_yaml_ng::Value::Mapping(mission),
    );
    normalised.insert(
        serde_yaml_ng::Value::String("steps".to_string()),
        field(map, "steps")
            .cloned()
            .unwrap_or_else(|| serde_yaml_ng::Value::Sequence(Vec::new())),
    );
    normalised.insert(
        serde_yaml_ng::Value::String("audit_steps".to_string()),
        field(map, "audit_steps")
            .cloned()
            .unwrap_or_else(|| serde_yaml_ng::Value::Sequence(Vec::new())),
    );
    serde_yaml_ng::Value::Mapping(normalised)
}

fn field<'a>(map: &'a serde_yaml_ng::Mapping, key: &str) -> Option<&'a serde_yaml_ng::Value> {
    map.get(key)
}

fn stringish(value: Option<&serde_yaml_ng::Value>) -> Option<String> {
    match value? {
        serde_yaml_ng::Value::String(text) if !text.is_empty() => Some(text.clone()),
        _ => None,
    }
}

pub(crate) fn read_bounded_utf8(path: &Path, limit: u64) -> Result<String, RuntimeError> {
    let metadata = fs::metadata(path).map_err(|source| RuntimeError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.is_file() {
        return Err(RuntimeError::NotAFile {
            path: path.to_path_buf(),
        });
    }
    if metadata.len() > limit {
        return Err(RuntimeError::TooLarge {
            path: path.to_path_buf(),
            limit,
        });
    }

    let bytes = sk_kernel::read_bounded(path, limit).map_err(|error| match error {
        sk_kernel::KernelError::TooLarge { .. } => RuntimeError::TooLarge {
            path: path.to_path_buf(),
            limit,
        },
        sk_kernel::KernelError::Io { source, .. } => RuntimeError::Io {
            path: path.to_path_buf(),
            source,
        },
        other => RuntimeError::Malformed {
            path: path.to_path_buf(),
            message: other.to_string(),
        },
    })?;
    if bytes.len() as u64 > limit {
        return Err(RuntimeError::TooLarge {
            path: path.to_path_buf(),
            limit,
        });
    }
    String::from_utf8(bytes).map_err(|_| RuntimeError::NotUtf8 {
        path: path.to_path_buf(),
    })
}

fn validate_identity(id: &str, title: &str) -> Result<(), RuntimeError> {
    if id.trim().is_empty() {
        return Err(RuntimeError::InvalidStep {
            step_id: id.to_string(),
            message: "id must be non-empty".to_string(),
        });
    }
    if title.trim().is_empty() {
        return Err(RuntimeError::InvalidStep {
            step_id: id.to_string(),
            message: "title must be non-empty".to_string(),
        });
    }
    Ok(())
}

fn validate_step_constraints<'a>(
    required_inputs: impl Iterator<Item = &'a String>,
    step_id: &str,
    raci: Option<&RaciAssignment>,
    override_reason: Option<&str>,
) -> Result<(), RuntimeError> {
    for input in required_inputs {
        if input.trim().is_empty() {
            return Err(RuntimeError::InvalidStep {
                step_id: step_id.to_string(),
                message: "requires_inputs entries must be non-empty".to_string(),
            });
        }
    }

    match (raci, override_reason) {
        (Some(assignment), Some(reason)) if !reason.trim().is_empty() => {
            if assignment.accountable.actor_type != ActorType::Human {
                return Err(RuntimeError::RaciAccountableNotHuman {
                    step_id: step_id.to_string(),
                    actor_type: assignment.accountable.actor_type,
                });
            }
        }
        (Some(_), Some(_)) => {
            return Err(RuntimeError::RaciOverrideReason {
                step_id: step_id.to_string(),
                message: "explicit raci block requires a non-empty raci_override_reason"
                    .to_string(),
            });
        }
        (Some(_), None) => {
            return Err(RuntimeError::RaciOverrideReason {
                step_id: step_id.to_string(),
                message: "explicit raci block requires a non-empty raci_override_reason"
                    .to_string(),
            });
        }
        (None, Some(_)) => {
            return Err(RuntimeError::RaciOverrideReason {
                step_id: step_id.to_string(),
                message: "raci_override_reason provided without a raci block".to_string(),
            });
        }
        (None, None) => {}
    }
    Ok(())
}

fn validate_significance(step_id: &str, block: &SignificanceBlock) -> Result<(), RuntimeError> {
    let expected: BTreeSet<&str> = SIGNIFICANCE_DIMENSIONS.into_iter().collect();
    let actual: BTreeSet<&str> = block.dimensions.keys().map(String::as_str).collect();
    if actual != expected {
        let missing: Vec<&&str> = expected.difference(&actual).collect();
        let extra: Vec<&&str> = actual.difference(&expected).collect();
        return Err(RuntimeError::InvalidSignificance {
            step_id: step_id.to_string(),
            message: format!(
                "dimensions must be exactly the six V1 dimensions (missing {missing:?}, unexpected {extra:?})"
            ),
        });
    }
    for (name, score) in &block.dimensions {
        if !(0..=3).contains(score) {
            return Err(RuntimeError::InvalidSignificance {
                step_id: step_id.to_string(),
                message: format!("dimension {name:?} score {score} is outside 0..=3"),
            });
        }
    }
    let known: BTreeSet<&str> = HARD_TRIGGER_CLASSES.into_iter().collect();
    for trigger in &block.hard_triggers {
        if !known.contains(trigger.as_str()) {
            return Err(RuntimeError::InvalidSignificance {
                step_id: step_id.to_string(),
                message: format!("unknown hard-trigger class {trigger:?}"),
            });
        }
    }
    Ok(())
}

fn detect_template_cycle(graph: &BTreeMap<String, Vec<String>>) -> Result<(), RuntimeError> {
    let mut state: BTreeMap<String, u8> = BTreeMap::new();
    let mut stack: Vec<String> = Vec::new();

    fn visit(
        node: &str,
        graph: &BTreeMap<String, Vec<String>>,
        state: &mut BTreeMap<String, u8>,
        stack: &mut Vec<String>,
    ) -> Result<(), RuntimeError> {
        match state.get(node).copied().unwrap_or(0) {
            2 => return Ok(()),
            1 => {
                let start = stack.iter().position(|item| item == node).unwrap_or(0);
                let mut cycle = stack[start..].to_vec();
                cycle.push(node.to_string());
                return Err(RuntimeError::DependencyCycle { cycle });
            }
            _ => {}
        }

        state.insert(node.to_string(), 1);
        stack.push(node.to_string());
        if let Some(dependencies) = graph.get(node) {
            for dependency in dependencies {
                visit(dependency, graph, state, stack)?;
            }
        }
        stack.pop();
        state.insert(node.to_string(), 2);
        Ok(())
    }

    for node in graph.keys() {
        visit(node, graph, &mut state, &mut stack)?;
    }
    Ok(())
}

fn deserialize_version<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Version {
        String(String),
        Signed(i64),
        Unsigned(u64),
        Float(f64),
        Bool(bool),
    }

    match Version::deserialize(deserializer)? {
        Version::String(text) => Ok(text),
        Version::Signed(value) => Ok(value.to_string()),
        Version::Unsigned(value) => Ok(value.to_string()),
        Version::Float(value) => Ok(value.to_string()),
        Version::Bool(value) => Ok(value.to_string()),
    }
}
