//! The canonical resolved Mission definition: Source
//! `specify_cli.mission.MissionConfig` and its nested models.
//!
//! The schema, the field order, the closed domains, the defaults, and the
//! validation diagnostics are reproduced from Source, because acceptance reads
//! these values (declared path conventions, declared required/optional
//! artifacts, domain, mission-type slug) and because a wrong acceptance or a
//! wrong refusal is a defect in either direction.
//!
//! This is **not** the Mission planning identity ([`crate::Mission`], a
//! `kitty-specs/<slug>/meta.json` instance) and **not** a documentation/template
//! type ([`crate::MissionTemplates`]). A definition is a `mission.yaml`
//! document: `name`/`description`/`version`/`domain`/`workflow`/`artifacts`/
//! `paths`/`validation`/`mcp_tools`/`agent_context`/`task_metadata`/`commands`/
//! `task_types`.
//!
//! Strings are [`PythonString`] (code points, unpaired surrogates included), so
//! authored data is neither narrowed nor re-encoded. A `MissionDefinition` is a
//! read-only value; nothing here writes, migrates, or reserializes user YAML.

use std::path::Path;

use sk_kernel::PythonString;
use sk_kernel::python_yaml::{GraphDocument, NodeRef};

use crate::definition::error::{ConfigIssue, DefinitionError, IssueKind, LocPart};
use crate::definition::value::OrderedMap;

/// The twelve accepted root fields, in Source declaration order.
///
/// Order is load-bearing twice: it is the order pydantic reports missing or
/// invalid fields in, and it is the list rendered in the typo hint.
pub const MISSION_ROOT_FIELDS: [&str; 13] = [
    "name",
    "description",
    "version",
    "domain",
    "workflow",
    "artifacts",
    "paths",
    "validation",
    "mcp_tools",
    "agent_context",
    "task_metadata",
    "commands",
    "task_types",
];

/// Retired mission-DSL v1 keys tolerated (and dropped) at the root.
///
/// Source keeps this tolerance for third-party packs and project overrides that
/// still carry inert DSL residue; an otherwise valid mission must not be
/// refused over it.
pub const MISSION_COMPAT_IGNORED_FIELDS: [&str; 7] = [
    "mission",
    "initial",
    "states",
    "transitions",
    "guards",
    "inputs",
    "outputs",
];

/// Canonical path-convention keys (Source `VALID_PATH_KEYS`).
pub const VALID_PATH_KEYS: [&str; 5] = [
    "workspace",
    "tests",
    "deliverables",
    "documentation",
    "data",
];

/// The `version` field's pattern, as Source writes it in its diagnostic.
pub const VERSION_PATTERN: &str = r"^\d+\.\d+\.\d+$";

/// The closed `domain` literal set, in Source declaration order.
pub const MISSION_DOMAINS: [&str; 5] = ["software", "research", "writing", "seo", "other"];

/// Mission domain classification.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MissionDomain {
    /// `software`
    Software,
    /// `research`
    Research,
    /// `writing`
    Writing,
    /// `seo`
    Seo,
    /// `other`
    Other,
}

impl MissionDomain {
    /// The authored token.
    pub fn as_str(self) -> &'static str {
        match self {
            MissionDomain::Software => "software",
            MissionDomain::Research => "research",
            MissionDomain::Writing => "writing",
            MissionDomain::Seo => "seo",
            MissionDomain::Other => "other",
        }
    }

    /// Parse the exact token (no case folding, no aliases).
    pub fn parse(text: &str) -> Option<Self> {
        MISSION_DOMAINS
            .iter()
            .position(|candidate| *candidate == text)
            .map(|index| match index {
                0 => MissionDomain::Software,
                1 => MissionDomain::Research,
                2 => MissionDomain::Writing,
                3 => MissionDomain::Seo,
                _ => MissionDomain::Other,
            })
    }

    /// Whether this is the research domain, whose deliverables live under a
    /// configured prefix instead of the Mission's primary surface.
    pub fn is_research(self) -> bool {
        self == MissionDomain::Research
    }
}

/// One workflow phase (`PhaseConfig`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkflowPhase {
    /// Phase identifier.
    pub name: PythonString,
    /// Phase description.
    pub description: PythonString,
}

/// `WorkflowConfig` — at least one phase.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissionWorkflow {
    /// Ordered phases.
    pub phases: Vec<WorkflowPhase>,
}

/// `ArtifactsConfig`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MissionArtifacts {
    /// Artifacts required for acceptance.
    pub required: Vec<PythonString>,
    /// Optional artifacts and directories.
    pub optional: Vec<PythonString>,
}

/// `ValidationConfig`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MissionValidation {
    /// Validation checks executed for this mission.
    pub checks: Vec<PythonString>,
    /// Whether `validators.py` should be invoked.
    pub custom_validators: bool,
}

/// `MCPToolsConfig`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MissionMcpTools {
    /// Required MCP tools.
    pub required: Vec<PythonString>,
    /// Recommended MCP tools.
    pub recommended: Vec<PythonString>,
    /// Optional MCP tools.
    pub optional: Vec<PythonString>,
}

/// `TaskMetadataConfig`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MissionTaskMetadata {
    /// Required task-metadata fields.
    pub required: Vec<PythonString>,
    /// Optional task-metadata fields.
    pub optional: Vec<PythonString>,
}

/// `CommandConfig` — the prompt is required.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissionCommand {
    /// Command-specific prompt/description.
    pub prompt: PythonString,
}

/// `TaskTypeConfig` — both fields are optional.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MissionTaskType {
    /// Suggested agent role/profile.
    pub agent_role: Option<PythonString>,
    /// Human-readable task-type description.
    pub description: Option<PythonString>,
}

/// A validated Mission definition (`MissionConfig`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissionDefinition {
    /// Mission display name.
    pub name: PythonString,
    /// Mission description.
    pub description: PythonString,
    /// Semver `major.minor.patch`.
    pub version: PythonString,
    /// Domain classification.
    pub domain: MissionDomain,
    /// Workflow phases (non-empty).
    pub workflow: MissionWorkflow,
    /// Required and optional artifacts.
    pub artifacts: MissionArtifacts,
    /// Path conventions, in authored order.
    pub paths: OrderedMap<PythonString>,
    /// Validation settings.
    pub validation: MissionValidation,
    /// MCP tool recommendations.
    pub mcp_tools: Option<MissionMcpTools>,
    /// Agent instructions/personality.
    pub agent_context: Option<PythonString>,
    /// Task metadata definitions.
    pub task_metadata: Option<MissionTaskMetadata>,
    /// Command prompts, in authored order.
    pub commands: Option<OrderedMap<MissionCommand>>,
    /// Task-type metadata, in authored order.
    pub task_types: Option<OrderedMap<MissionTaskType>>,
    /// Source's `model_post_init` unknown-path-key warning, if any.
    pub path_key_warnings: Vec<PythonString>,
}

/// A definition together with the warnings its construction emitted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedDefinition {
    /// The validated definition.
    pub definition: MissionDefinition,
    /// `warnings.warn` messages Source emitted while validating.
    pub warnings: Vec<PythonString>,
}

impl MissionDefinition {
    /// Declared path conventions in authored order.
    pub fn paths(&self) -> &OrderedMap<PythonString> {
        &self.paths
    }

    /// Declared required artifacts.
    pub fn required_artifacts(&self) -> &[PythonString] {
        &self.artifacts.required
    }

    /// Declared optional artifacts.
    pub fn optional_artifacts(&self) -> &[PythonString] {
        &self.artifacts.optional
    }

    /// Declared artifact tokens: required first, then optional, authored order.
    pub fn artifact_names(&self) -> impl Iterator<Item = &PythonString> {
        self.artifacts
            .required
            .iter()
            .chain(self.artifacts.optional.iter())
    }

    /// Every warning construction produced, in Source order.
    pub fn warnings(&self) -> &[PythonString] {
        &self.path_key_warnings
    }

    /// Validate one decoded `mission.yaml` document.
    ///
    /// `path` is used only for diagnostics. The document is the canonical object
    /// graph; this walks only the fields Source's model declares, so a
    /// Source-valid **cycle in an ignored compatibility field is never
    /// traversed** and does not turn into a refusal. A cycle in a required field
    /// surfaces here as the model's own issue, in the model phase. The returned
    /// definition is complete and already valid; a document Source refuses yields
    /// [`DefinitionError::InvalidConfig`] with Source's rendered text.
    pub fn from_document(
        path: &Path,
        document: &GraphDocument,
    ) -> Result<LoadedDefinition, DefinitionError> {
        // Source reads `yaml.safe_load(f) or {}`: a falsy document (an empty
        // file, a lone `null`, `[]`, `""`, `0`, `false`) becomes an EMPTY mapping
        // and is reported as missing required fields — not as "not a mapping".
        let root = document.root();
        if !root.is_falsy() && root.as_mapping().is_none() {
            return Err(DefinitionError::NotAMapping {
                path: path.to_path_buf(),
                got: root.python_type_name(),
            });
        }

        let mut issues = Vec::new();
        let mut builder = Builder::new(root);

        let name = builder.required_str("name");
        let description = builder.required_str("description");
        let version = builder.required_version();
        let domain = builder.required_domain();
        let workflow = builder.required_workflow();
        let artifacts = builder.required_artifacts();
        let paths = builder.paths();
        let validation = builder.validation();
        let mcp_tools = builder.mcp_tools();
        let agent_context = builder.optional_str("agent_context");
        let task_metadata = builder.task_metadata();
        let commands = builder.commands();
        let task_types = builder.task_types();
        issues.extend(builder.finish());

        if !issues.is_empty() {
            return Err(DefinitionError::InvalidConfig {
                path: path.to_path_buf(),
                issues,
            });
        }

        let paths = paths.unwrap_or_default();
        let warnings = unknown_path_key_warnings(&paths);
        let definition = MissionDefinition {
            name: name.expect("validated field"),
            description: description.expect("validated field"),
            version: version.expect("validated field"),
            domain: domain.expect("validated field"),
            workflow: workflow.expect("validated field"),
            artifacts: artifacts.expect("validated field"),
            paths,
            validation: validation.unwrap_or_default(),
            mcp_tools,
            agent_context,
            task_metadata,
            commands,
            task_types,
            path_key_warnings: warnings.clone(),
        };
        Ok(LoadedDefinition {
            definition,
            warnings,
        })
    }
}

// Python truthiness for the decoded document (`yaml.safe_load(f) or {}`) is the
// one shared rule: `NodeRef::is_falsy` (and `PythonValue::is_falsy` for the
// legacy domain). `None`, `False`, numeric zero, an empty string/bytes/sequence/
// set/mapping are all falsy, so Source substitutes `{}` and reports missing
// fields instead of a root-type error.
fn unknown_path_key_warnings(paths: &OrderedMap<PythonString>) -> Vec<PythonString> {
    let mut unknown: Vec<PythonString> = paths
        .keys()
        .filter(|key| {
            !VALID_PATH_KEYS
                .iter()
                .any(|valid| python_equals(key, valid))
        })
        .cloned()
        .collect();
    if unknown.is_empty() {
        return Vec::new();
    }
    sort_python_strings(&mut unknown);
    let mut known: Vec<PythonString> = VALID_PATH_KEYS
        .iter()
        .map(|key| PythonString::from_scalars(key))
        .collect();
    sort_python_strings(&mut known);
    let mut message = PythonString::from_scalars("Unknown path conventions: ");
    message.push_scalars(&repr_list(&unknown));
    message.push_scalars(". Known conventions: ");
    message.push_scalars(&repr_list(&known));
    vec![message]
}

/// Compare an authored string with an ASCII token by decoded code points.
///
/// `PythonString` deliberately has no `PartialEq<str>`: a comparison against a
/// Rust `str` would silently assume the authored value is scalar-only.
pub(crate) fn python_equals(value: &PythonString, text: &str) -> bool {
    let units = value.codepoints();
    units.len() == text.chars().count()
        && units
            .iter()
            .zip(text.chars())
            .all(|(code, character)| *code == u32::from(character))
}

/// Python `sorted()` order for `str`: by code point, not by UTF-8 bytes.
pub(crate) fn sort_python_strings(values: &mut [PythonString]) {
    values.sort_by(|left, right| left.codepoints().cmp(right.codepoints()));
}

/// Python `repr()` of a list of strings, e.g. `['data', 'tests']`.
pub fn repr_list(values: &[PythonString]) -> String {
    let mut out = String::from("[");
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        out.push_str(&value.repr());
    }
    out.push(']');
    out
}

// `str(value)` for a decoded value is the ONE graph text authority
// (`NodeRef::python_str`). This module no longer carries a private `str()`/
// `repr()` walker for diagnostics.

/// Collects field results and issues in Source order.
struct Builder<'a> {
    root: NodeRef<'a>,
    issues: Vec<ConfigIssue>,
}

impl<'a> Builder<'a> {
    fn new(root: NodeRef<'a>) -> Self {
        Self {
            root,
            issues: Vec::new(),
        }
    }

    fn lookup(&self, field: &str) -> Option<NodeRef<'a>> {
        self.root.lookup(field)
    }

    fn push(&mut self, loc: Vec<LocPart>, kind: IssueKind) {
        self.issues.push(ConfigIssue { loc, kind });
    }

    fn field_loc(field: &'static str) -> Vec<LocPart> {
        vec![LocPart::Field(PythonString::from_scalars(field))]
    }

    fn required_str(&mut self, field: &'static str) -> Option<PythonString> {
        let loc = Self::field_loc(field);
        match self.lookup(field) {
            None => {
                self.push(loc, IssueKind::Missing);
                None
            }
            Some(value) => match value.as_lax_str() {
                Some(text) => Some(text),
                None => {
                    self.push(loc, IssueKind::NotString);
                    None
                }
            },
        }
    }

    fn optional_str(&mut self, field: &'static str) -> Option<PythonString> {
        let loc = Self::field_loc(field);
        match self.lookup(field) {
            None => None,
            Some(value) if value.is_null() => None,
            Some(value) => match value.as_lax_str() {
                Some(text) => Some(text),
                None => {
                    self.push(loc, IssueKind::NotString);
                    None
                }
            },
        }
    }

    fn required_version(&mut self) -> Option<PythonString> {
        let text = self.required_str("version")?;
        let scalar = text.to_scalar_string().ok();
        let matches = match scalar {
            Some(scalar) => fullmatch_version(&scalar),
            // An unpaired surrogate can never satisfy the ASCII pattern.
            None => false,
        };
        if matches {
            Some(text)
        } else {
            self.push(
                Self::field_loc("version"),
                IssueKind::Pattern(VERSION_PATTERN),
            );
            None
        }
    }

    fn required_domain(&mut self) -> Option<MissionDomain> {
        let loc = Self::field_loc("domain");
        match self.lookup("domain") {
            None => {
                self.push(loc, IssueKind::Missing);
                None
            }
            Some(value) => {
                match value
                    .as_str()
                    .and_then(|text| text.to_scalar_string().ok())
                    .as_deref()
                {
                    Some(scalar) => match MissionDomain::parse(scalar) {
                        Some(domain) => Some(domain),
                        None => {
                            self.push(loc, IssueKind::NotADomain);
                            None
                        }
                    },
                    None => {
                        self.push(loc, IssueKind::NotADomain);
                        None
                    }
                }
            }
        }
    }

    fn required_workflow(&mut self) -> Option<MissionWorkflow> {
        let loc = Self::field_loc("workflow");
        let value = match self.lookup("workflow") {
            None => {
                self.push(loc, IssueKind::Missing);
                return None;
            }
            Some(value) => value,
        };
        let Some(_entries) = value.as_mapping() else {
            self.push(loc, IssueKind::NotDictionary(Some("WorkflowConfig")));
            return None;
        };
        let phases_loc = self.child_loc("workflow", "phases");
        let phases_value = value.lookup("phases");
        let phases = match phases_value {
            None => {
                self.push(phases_loc, IssueKind::Missing);
                return None;
            }
            Some(phases) => match phases.as_sequence() {
                Some(items) => items,
                None => {
                    self.push(phases_loc, IssueKind::NotList);
                    return None;
                }
            },
        };
        if phases.is_empty() {
            self.push(phases_loc, IssueKind::ListTooShort { min: 1, got: 0 });
            return None;
        }
        let mut parsed = Vec::with_capacity(phases.len());
        let mut failed = false;
        for (index, phase) in phases.iter().enumerate() {
            match self.phase(index, value.view(*phase)) {
                Some(parsed_phase) => parsed.push(parsed_phase),
                None => failed = true,
            }
        }
        self.check_extras(value, &["phases"], "workflow");
        if failed {
            None
        } else {
            Some(MissionWorkflow { phases: parsed })
        }
    }

    fn phase(&mut self, index: usize, value: NodeRef<'a>) -> Option<WorkflowPhase> {
        let base = self.child_loc("workflow", "phases");
        let mut loc_prefix = base.clone();
        loc_prefix.push(LocPart::Index(index));
        let Some(_entries) = value.as_mapping() else {
            self.push(loc_prefix, IssueKind::NotDictionary(Some("PhaseConfig")));
            return None;
        };
        let name = self.nested_str(value, &loc_prefix, "name");
        let description = self.nested_str(value, &loc_prefix, "description");
        self.check_extras_at(value, &["name", "description"], &loc_prefix);
        match (name, description) {
            (Some(name), Some(description)) => Some(WorkflowPhase { name, description }),
            _ => None,
        }
    }

    fn nested_str(
        &mut self,
        node: NodeRef<'a>,
        prefix: &[LocPart],
        field: &'static str,
    ) -> Option<PythonString> {
        let mut loc = prefix.to_vec();
        loc.push(LocPart::Field(PythonString::from_scalars(field)));
        match node.lookup(field) {
            None => {
                self.push(loc, IssueKind::Missing);
                None
            }
            Some(value) => match value.as_lax_str() {
                Some(text) => Some(text),
                None => {
                    self.push(loc, IssueKind::NotString);
                    None
                }
            },
        }
    }

    fn required_artifacts(&mut self) -> Option<MissionArtifacts> {
        let loc = Self::field_loc("artifacts");
        let value = match self.lookup("artifacts") {
            None => {
                self.push(loc, IssueKind::Missing);
                return None;
            }
            Some(value) => value,
        };
        let Some(_entries) = value.as_mapping() else {
            self.push(loc, IssueKind::NotDictionary(Some("ArtifactsConfig")));
            return None;
        };
        let required = self
            .str_list(value, "artifacts", "required")
            .unwrap_or_default();
        let optional = self
            .str_list(value, "artifacts", "optional")
            .unwrap_or_default();
        self.check_extras(value, &["required", "optional"], "artifacts");
        Some(MissionArtifacts { required, optional })
    }

    fn str_list(
        &mut self,
        node: NodeRef<'a>,
        parent: &'static str,
        field: &'static str,
    ) -> Option<Vec<PythonString>> {
        let loc = self.child_loc(parent, field);
        match node.lookup(field) {
            None => Some(Vec::new()),
            Some(value) => match value.as_sequence() {
                Some(items) => {
                    let mut out = Vec::with_capacity(items.len());
                    let mut failed = false;
                    for (index, item) in items.iter().enumerate() {
                        match value.view(*item).as_lax_str() {
                            Some(text) => out.push(text),
                            None => {
                                let mut item_loc = loc.clone();
                                item_loc.push(LocPart::Index(index));
                                self.push(item_loc, IssueKind::NotString);
                                failed = true;
                            }
                        }
                    }
                    if failed { None } else { Some(out) }
                }
                None => {
                    self.push(loc, IssueKind::NotList);
                    None
                }
            },
        }
    }

    fn paths(&mut self) -> Option<OrderedMap<PythonString>> {
        let loc = Self::field_loc("paths");
        match self.lookup("paths") {
            None => Some(OrderedMap::new()),
            Some(value) => match value.as_mapping() {
                Some(entries) => {
                    let mut map = OrderedMap::new();
                    let mut failed = false;
                    for (key, item) in entries {
                        let key = value.view(*key);
                        let item = value.view(*item);
                        let key_loc = match key.as_str() {
                            Some(text) => {
                                let mut key_loc = loc.clone();
                                key_loc.push(LocPart::Field(text.clone()));
                                key_loc
                            }
                            None => {
                                let mut key_loc = loc.clone();
                                key_loc.push(LocPart::Field(key.python_str()));
                                key_loc.push(LocPart::KeyMarker);
                                self.push(key_loc, IssueKind::NotString);
                                failed = true;
                                continue;
                            }
                        };
                        match item.as_lax_str() {
                            Some(text) => {
                                if let Some(key_text) = key.as_str() {
                                    map.insert(key_text.clone(), text);
                                }
                            }
                            None => {
                                self.push(key_loc, IssueKind::NotString);
                                failed = true;
                            }
                        }
                    }
                    if failed { None } else { Some(map) }
                }
                None => {
                    self.push(loc, IssueKind::NotDictionary(None));
                    None
                }
            },
        }
    }

    fn validation(&mut self) -> Option<MissionValidation> {
        let loc = Self::field_loc("validation");
        match self.lookup("validation") {
            None => Some(MissionValidation::default()),
            Some(value) if value.is_null() => {
                self.push(loc, IssueKind::NotDictionary(Some("ValidationConfig")));
                None
            }
            Some(value) => match value.as_mapping() {
                Some(_entries) => {
                    let checks = self
                        .str_list(value, "validation", "checks")
                        .unwrap_or_default();
                    let custom_validators =
                        self.bool_field(value, "validation", "custom_validators");
                    self.check_extras(value, &["checks", "custom_validators"], "validation");
                    Some(MissionValidation {
                        checks,
                        custom_validators: custom_validators.unwrap_or(false),
                    })
                }
                None => {
                    self.push(loc, IssueKind::NotDictionary(Some("ValidationConfig")));
                    None
                }
            },
        }
    }

    fn bool_field(
        &mut self,
        node: NodeRef<'a>,
        parent: &'static str,
        field: &'static str,
    ) -> Option<bool> {
        let loc = self.child_loc(parent, field);
        match node.lookup(field) {
            None => Some(false),
            Some(value) => match value.as_bool_lax() {
                Some(flag) => Some(flag),
                None => {
                    self.push(loc, IssueKind::NotBoolean);
                    None
                }
            },
        }
    }

    fn mcp_tools(&mut self) -> Option<MissionMcpTools> {
        let loc = Self::field_loc("mcp_tools");
        match self.lookup("mcp_tools") {
            None => None,
            Some(value) if value.is_null() => None,
            Some(value) => match value.as_mapping() {
                Some(_entries) => {
                    let required = self
                        .str_list(value, "mcp_tools", "required")
                        .unwrap_or_default();
                    let recommended = self
                        .str_list(value, "mcp_tools", "recommended")
                        .unwrap_or_default();
                    let optional = self
                        .str_list(value, "mcp_tools", "optional")
                        .unwrap_or_default();
                    self.check_extras(value, &["required", "recommended", "optional"], "mcp_tools");
                    Some(MissionMcpTools {
                        required,
                        recommended,
                        optional,
                    })
                }
                None => {
                    self.push(loc, IssueKind::NotDictionary(Some("MCPToolsConfig")));
                    None
                }
            },
        }
    }

    fn task_metadata(&mut self) -> Option<MissionTaskMetadata> {
        let loc = Self::field_loc("task_metadata");
        match self.lookup("task_metadata") {
            None => None,
            Some(value) if value.is_null() => None,
            Some(value) => match value.as_mapping() {
                Some(_entries) => {
                    let required = self
                        .str_list(value, "task_metadata", "required")
                        .unwrap_or_default();
                    let optional = self
                        .str_list(value, "task_metadata", "optional")
                        .unwrap_or_default();
                    self.check_extras(value, &["required", "optional"], "task_metadata");
                    Some(MissionTaskMetadata { required, optional })
                }
                None => {
                    self.push(loc, IssueKind::NotDictionary(Some("TaskMetadataConfig")));
                    None
                }
            },
        }
    }

    fn commands(&mut self) -> Option<OrderedMap<MissionCommand>> {
        let loc = Self::field_loc("commands");
        match self.lookup("commands") {
            None => None,
            Some(value) if value.is_null() => None,
            Some(value) => match value.as_mapping() {
                Some(entries) => {
                    let mut map = OrderedMap::new();
                    let mut failed = false;
                    for (key, item) in entries {
                        let key = value.view(*key);
                        let item = value.view(*item);
                        let Some(key_text) = key.as_str() else {
                            let mut key_loc = loc.clone();
                            key_loc.push(LocPart::Field(key.python_str()));
                            key_loc.push(LocPart::KeyMarker);
                            self.push(key_loc, IssueKind::NotString);
                            failed = true;
                            continue;
                        };
                        let mut value_loc = loc.clone();
                        value_loc.push(LocPart::Field(key_text.clone()));
                        let Some(_inner) = item.as_mapping() else {
                            self.push(value_loc, IssueKind::NotDictionary(Some("CommandConfig")));
                            failed = true;
                            continue;
                        };
                        let prompt = self.nested_str(item, &value_loc, "prompt");
                        self.check_extras_at(item, &["prompt"], &value_loc);
                        match prompt {
                            Some(prompt) => {
                                map.insert(key_text.clone(), MissionCommand { prompt });
                            }
                            None => failed = true,
                        }
                    }
                    if failed { None } else { Some(map) }
                }
                None => {
                    self.push(loc, IssueKind::NotDictionary(None));
                    None
                }
            },
        }
    }

    fn task_types(&mut self) -> Option<OrderedMap<MissionTaskType>> {
        let loc = Self::field_loc("task_types");
        match self.lookup("task_types") {
            None => None,
            Some(value) if value.is_null() => None,
            Some(value) => match value.as_mapping() {
                Some(entries) => {
                    let mut map = OrderedMap::new();
                    let mut failed = false;
                    for (key, item) in entries {
                        let key = value.view(*key);
                        let item = value.view(*item);
                        let Some(key_text) = key.as_str() else {
                            let mut key_loc = loc.clone();
                            key_loc.push(LocPart::Field(key.python_str()));
                            key_loc.push(LocPart::KeyMarker);
                            self.push(key_loc, IssueKind::NotString);
                            failed = true;
                            continue;
                        };
                        let mut value_loc = loc.clone();
                        value_loc.push(LocPart::Field(key_text.clone()));
                        let Some(_inner) = item.as_mapping() else {
                            self.push(value_loc, IssueKind::NotDictionary(Some("TaskTypeConfig")));
                            failed = true;
                            continue;
                        };
                        let agent_role = self.optional_nested_str(item, &value_loc, "agent_role");
                        let description = self.optional_nested_str(item, &value_loc, "description");
                        self.check_extras_at(item, &["agent_role", "description"], &value_loc);
                        map.insert(
                            key_text.clone(),
                            MissionTaskType {
                                agent_role,
                                description,
                            },
                        );
                    }
                    if failed { None } else { Some(map) }
                }
                None => {
                    self.push(loc, IssueKind::NotDictionary(None));
                    None
                }
            },
        }
    }

    fn optional_nested_str(
        &mut self,
        node: NodeRef<'a>,
        prefix: &[LocPart],
        field: &'static str,
    ) -> Option<PythonString> {
        let mut loc = prefix.to_vec();
        loc.push(LocPart::Field(PythonString::from_scalars(field)));
        match node.lookup(field) {
            None => None,
            Some(value) if value.is_null() => None,
            Some(value) => match value.as_lax_str() {
                Some(text) => Some(text),
                None => {
                    self.push(loc, IssueKind::NotString);
                    None
                }
            },
        }
    }

    fn child_loc(&self, parent: &'static str, field: &'static str) -> Vec<LocPart> {
        vec![
            LocPart::Field(PythonString::from_scalars(parent)),
            LocPart::Field(PythonString::from_scalars(field)),
        ]
    }

    fn check_extras(&mut self, node: NodeRef<'a>, allowed: &[&str], parent: &'static str) {
        let prefix = vec![LocPart::Field(PythonString::from_scalars(parent))];
        self.check_extras_at(node, allowed, &prefix);
    }

    fn check_extras_at(&mut self, node: NodeRef<'a>, allowed: &[&str], prefix: &[LocPart]) {
        let Some(entries) = node.as_mapping() else {
            return;
        };
        for (key, _) in entries {
            let key = node.view(*key);
            let Some(text) = key.as_str() else {
                continue;
            };
            let scalar = text.to_scalar_string();
            let known = match &scalar {
                Ok(scalar) => allowed.iter().any(|candidate| candidate == scalar),
                Err(_) => false,
            };
            if known {
                continue;
            }
            let mut loc = prefix.to_vec();
            loc.push(LocPart::Field(text.clone()));
            self.push(loc, IssueKind::Extra);
        }
    }

    /// Root-level extra fields, after every declared field's issues.
    ///
    /// Walks the authored root keys only. `MISSION_COMPAT_IGNORED_FIELDS` are
    /// skipped without ever being traversed, so an ignored field's value —
    /// including a cycle Source also ignores — is not inspected here.
    fn finish(mut self) -> Vec<ConfigIssue> {
        let mut extras = Vec::new();
        let Some(entries) = self.root.as_mapping() else {
            self.issues.extend(extras);
            return self.issues;
        };
        for (key, _) in entries {
            let key = self.root.view(*key);
            let Some(text) = key.as_str() else {
                // A non-string root key is an extra input pydantic reports by
                // its rendered form.
                extras.push(ConfigIssue {
                    loc: vec![LocPart::Field(key.python_str())],
                    kind: IssueKind::Extra,
                });
                continue;
            };
            let scalar = text.to_scalar_string();
            let is_root = match &scalar {
                Ok(scalar) => MISSION_ROOT_FIELDS.iter().any(|field| field == scalar),
                Err(_) => false,
            };
            let is_compat = match &scalar {
                Ok(scalar) => MISSION_COMPAT_IGNORED_FIELDS
                    .iter()
                    .any(|ignored| ignored == scalar),
                Err(_) => false,
            };
            if is_root || is_compat {
                continue;
            }
            extras.push(ConfigIssue {
                loc: vec![LocPart::Field(text.clone())],
                kind: IssueKind::Extra,
            });
        }
        self.issues.extend(extras);
        self.issues
    }
}

/// `^\d+\.\d+\.\d+$` with full-match semantics (pydantic's pattern check).
fn fullmatch_version(text: &str) -> bool {
    let parts: Vec<&str> = text.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}
