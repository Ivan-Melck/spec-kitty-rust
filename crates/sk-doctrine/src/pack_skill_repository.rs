//! Canonical pack-skill repository: tier rules, body confinement, limits,
//! sibling-org conflicts, `overrides`/`enhances` folding.
//!
//! Source: `src/charter/offering/pack_skills/{models,validation,repository}.py`
//! (`PackSkillRepository._post_validate`, `_record_org_origin`,
//! `_resolve_augmentations`/`_fold_augmenter`, `validate_pack_skill`,
//! `apply_enhancement`, `skill_limit_violation`).
//!
//! This is the ONE owner of the pack-skill domain model and of the
//! post-validation phase the generic `BaseArtifactRepository` loader cannot
//! express. It sits below both consumers:
//!
//! * [`crate::offering::CanonicalOfferingService`] serves the layered skill
//!   records (and their effective, folded mapping) through its Skill arm;
//! * `sk-install`'s catalog projection re-exports [`PackSkill`],
//!   [`PackSkillRepository`] and the pure rules, then uses the records for
//!   preparation/rendering instead of keeping a parallel parser.
//!
//! # Phases, exactly as the reference orders them
//!
//! 1. **Admission** — every document is read through the crate's ONE lossless
//!    transport/constructor ([`ArtifactDocument`]) and validated by the ONE
//!    whole-model authority ([`crate::offering::model`]); a document that fails
//!    is skipped with a warning, never fatal.
//! 2. **Post-validation** — the tier rules that need the source file:
//!    rendered-name/description limits, reserved built-in prefixes, a
//!    `scripts/` directory, prompt-body existence/UTF-8/frontmatter and the
//!    confined `body_path`. These run at the same three item-entry points as
//!    the reference (built-in load, overlay merge, overlay insert).
//! 3. **Sibling-org conflict** — the same id in two org packs raises a hard
//!    load error (the reference's uncaught `PackSkillConflictError`).
//! 4. **Augmentation** — `overrides`/`enhances` records are removed and folded
//!    into their target id, in sorted-id order, after the whole layer load.
//!    A fold failure is a warning; success changes the target's record,
//!    provenance and source path exactly like `_fold_augmenter`.
//!
//! # ONE exact model, no scalar projection in the data path
//!
//! [`PackSkill`] carries exact Python code points ([`PythonString`]): id,
//! title, description, triggers, body path, expansion, parameters, invocation,
//! tools, version, maintainers and the augmentation targets. Every field is read
//! from the **exact canonical model dump**
//! ([`crate::canonical_artifact_body_exact`]) over the document's own canonical
//! graph — or over the merged graph of a layered record — never through a
//! `String`/`serde` round trip. One record type, one serializer
//! ([`PackSkill::model_dump_json`]) and one mapping carrier
//! ([`PackSkill::exact_mapping`]) feed the repository, the offering include
//! payload and the installer's preparation/hash/render, so a public field cannot
//! diverge from a private sidecar.
//!
//! # What the pinned reference actually admits (observed, not assumed)
//!
//! Pydantic-core refuses a lone surrogate in a **constrained** `str` field
//! (`Field(min_length=…)`: `title`, `description`, `SkillParameter.name`,
//! `SkillExpansion.target`), so those records are skipped with the reference's
//! own warning. An **unconstrained** `str` field (`version`, `body_path`,
//! `parameters[].description`, `parameters[].default`, `expands_to.args`,
//! `triggers`, `tools`, `maintainers`, `invocation.side_effects`) keeps the exact
//! code points: the reference *serves* such a record and fails only when it
//! physically encodes the rendered `SKILL.md`. The repository therefore admits
//! it and keeps the exact data; the late encoding phase belongs to the installer.
//!
//! # Remaining disclosed boundaries
//!
//! * A non-string frontmatter key can never spell a forbidden key, so only
//!   string keys participate in the lower-case forbidden-key check (the
//!   reference `str(key).lower()` cannot produce a bare forbidden name from a
//!   typed key).
//! * Body read errors other than invalid UTF-8 are reported as the per-file
//!   skip the reference's `except (…, OSError)` performs; invalid UTF-8 aborts
//!   the load as the reference's uncaught `UnicodeDecodeError`.
//! * A `body_path` whose code points have no filesystem encoding (a lone
//!   surrogate) is refused with `UnicodeEncodeError` and its canonical encoder
//!   message, the phase the reference's `Path.resolve()` escapes in.
//! * `body_text` returns `None` for an unreadable body instead of raising; the
//!   load phase already validated every prompt body the preparation phase can
//!   reach.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_yaml_ng::{Mapping, Value};
use sk_charter::{ArtifactKind, PackTier};
use sk_kernel::PythonString;
use sk_kernel::python_json::PythonJsonValue;

use crate::body::canonical_artifact_body_exact_value;
use crate::offering::model;
use crate::offering::value::ArtifactDocument;

/// Longest accepted skill id (`SKILL_ID_MAX_LENGTH`).
pub const SKILL_ID_MAX_LENGTH: usize = 64;

/// Longest accepted skill namespace (`SKILL_NAMESPACE_MAX_LENGTH`).
pub const SKILL_NAMESPACE_MAX_LENGTH: usize = 32;

/// Longest rendered skill name the Agent Skills specification accepts.
pub const MAX_SKILL_NAME_LENGTH: usize = 64;

/// Longest rendered skill description (after the `Triggers:` suffix).
pub const MAX_SKILL_DESCRIPTION_LENGTH: usize = 1024;

/// Name prefixes reserved for built-in skills (ADR "Namespaces and trust").
pub const RESERVED_PREFIXES: [&str; 3] = ["spk-", "spec-kitty-", "spec-kitty."];

/// Frontmatter keys that would widen the host tool's permissions.
const FORBIDDEN_FRONTMATTER_KEYS: [&str; 2] = ["allowed-tools", "allowed_tools"];

/// The built-in wrapper target prefix.
pub const BUILTIN_TARGET_PREFIX: &str = "builtin:";

/// The CLI wrapper target prefix.
pub const CLI_TARGET_PREFIX: &str = "cli:";

/// The only program a `cli:` wrapper may name.
pub const CLI_ALLOWED_PROGRAM: &str = "spec-kitty";

/// The record suffix a pack skill is declared with (`*.skill.yaml`).
pub const PACK_SKILL_SUFFIX: &str = ".skill.yaml";

/// The form discriminator of a pack skill.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SkillForm {
    /// Carries a Markdown prompt body (`body_path`).
    Prompt,
    /// Shorthand expanding to an existing command (`expands_to`).
    Wrapper,
}

impl SkillForm {
    /// The canonical token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Prompt => "prompt",
            Self::Wrapper => "wrapper",
        }
    }

    fn parse(token: &str) -> Option<Self> {
        match token {
            "prompt" => Some(Self::Prompt),
            "wrapper" => Some(Self::Wrapper),
            _ => None,
        }
    }
}

/// A wrapper-form skill's expansion target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillExpansion {
    /// `builtin:<command>` or `cli:spec-kitty …`.
    pub target: PythonString,
    /// Plain arguments appended to the expansion.
    pub args: PythonString,
}

/// One declared parameter of a pack skill.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillParameter {
    /// The parameter name.
    pub name: PythonString,
    /// Whether a caller must supply it.
    pub required: bool,
    /// Operator-facing description.
    pub description: PythonString,
    /// Declared default.
    pub default: Option<PythonString>,
}

/// Who may invoke a skill.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillInvocation {
    /// The operator may invoke it.
    pub user_invocable: bool,
    /// The model may invoke it.
    pub model_invocable: bool,
    /// Declared side effects; any declaration forces `model_invocable` off.
    pub side_effects: Vec<PythonString>,
}

impl Default for SkillInvocation {
    fn default() -> Self {
        Self {
            user_invocable: true,
            model_invocable: true,
            side_effects: Vec::new(),
        }
    }
}

/// One `*.skill.yaml` pack skill: the canonical exact view of the Source
/// `PackSkill` Pydantic model.
///
/// Every text field holds exact Python code points ([`PythonString`]), because a
/// Source-valid record can carry a code unit a Rust `String` cannot (see the
/// module docs). `form` is the closed discriminator; `schema_version` and `id`
/// are grammar-validated ASCII.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackSkill {
    /// Must be `1.0`.
    pub schema_version: PythonString,
    /// The skill id (`skill:<id>`).
    pub id: PythonString,
    /// Human title.
    pub title: PythonString,
    /// Description (before the `Triggers:` suffix).
    pub description: PythonString,
    /// Trigger phrases.
    pub triggers: Vec<PythonString>,
    /// Prompt or wrapper form.
    pub form: SkillForm,
    /// Prompt body path, relative to the record.
    pub body_path: Option<PythonString>,
    /// Wrapper expansion.
    pub expands_to: Option<SkillExpansion>,
    /// Declared parameters.
    pub parameters: Vec<SkillParameter>,
    /// Invocation policy.
    pub invocation: SkillInvocation,
    /// Tool targeting (never rendered).
    pub tools: Vec<PythonString>,
    /// Record version.
    pub version: PythonString,
    /// Maintainers.
    pub maintainers: Vec<PythonString>,
    /// Replaced skill id.
    pub overrides: Option<PythonString>,
    /// Narrowed skill id.
    pub enhances: Option<PythonString>,
}

impl PackSkill {
    /// Whether this record augments another skill instead of standing alone.
    #[must_use]
    pub fn is_augmentation(&self) -> bool {
        self.overrides.is_some() || self.enhances.is_some()
    }

    /// Checked adapter from a constructed YAML value (the installer's read-model
    /// boundary and test callers).
    ///
    /// Admission is the canonical whole-model predicate
    /// ([`crate::offering::model`]); the typed fields are read from the
    /// canonical model dump ([`canonical_artifact_body_exact_value`]), so this
    /// is a view of the ONE model, never a second parser. A plain tree has no
    /// lossless transport markers, so this entry point is a **checked scalar
    /// capability boundary**: it refuses a tree that still carries a marker
    /// spelling instead of reading the marker as data. The repository path does
    /// not use it — it reads the exact graph dump.
    ///
    /// # Errors
    ///
    /// A message when the model rejects the record, or when this tree has no
    /// exact dump (a marker-carrying tree).
    pub fn from_yaml(value: &Value) -> Result<Self, String> {
        if model::admits_pack_skill_value(value).is_none() {
            return Err("the pack-skill model rejects this record".to_owned());
        }
        let dump = canonical_artifact_body_exact_value(ArtifactKind::Skill, value)
            .ok_or_else(|| "the canonical pack-skill model dump is unavailable".to_owned())?;
        Self::from_exact_dump(&dump)
            .ok_or_else(|| "the canonical pack-skill model dump is malformed".to_owned())
    }

    /// Read the exact record out of the canonical `model_dump(mode="json")` view.
    ///
    /// Exposed for tests and for a consumer that already holds a canonical dump
    /// ([`Self::from_yaml`] is the checked YAML-tree adapter). It is the same
    /// projection the repository uses, not a second parser.
    ///
    /// Every text field keeps its exact code points; nothing is scalarised,
    /// replaced or escaped. `None` only when the dump is not the admitted
    /// `PackSkill` shape (it always is, for a dump this crate produced from an
    /// admitted document).
    #[must_use]
    pub fn from_exact_dump(dump: &PythonJsonValue) -> Option<Self> {
        let schema_version = field_string(dump, "schema_version")?;
        let id = field_string(dump, "id")?;
        let title = field_string(dump, "title")?;
        let description = field_string(dump, "description")?;
        let triggers = field_string_list(dump, "triggers")?;
        let form = SkillForm::parse(&field_string(dump, "form")?.to_scalar_string().ok()?)?;
        let body_path = optional_field_string(dump, "body_path")?;
        let expands_to = match field(dump, "expands_to") {
            None | Some(PythonJsonValue::Null) => None,
            Some(value) => Some(SkillExpansion {
                target: field_string(value, "target")?,
                args: field_string(value, "args")?,
            }),
        };
        let parameters = match field(dump, "parameters") {
            None | Some(PythonJsonValue::Null) => Vec::new(),
            Some(PythonJsonValue::Array(items)) => items
                .iter()
                .map(|item| {
                    Some(SkillParameter {
                        name: field_string(item, "name")?,
                        required: field_bool(item, "required")?,
                        description: field_string(item, "description")?,
                        default: optional_field_string(item, "default")?,
                    })
                })
                .collect::<Option<Vec<_>>>()?,
            Some(_) => return None,
        };
        let invocation = match field(dump, "invocation") {
            None | Some(PythonJsonValue::Null) => SkillInvocation::default(),
            Some(value) => SkillInvocation {
                user_invocable: field_bool(value, "user_invocable")?,
                model_invocable: field_bool(value, "model_invocable")?,
                side_effects: field_string_list(value, "side_effects")?,
            },
        };
        let tools = field_string_list(dump, "tools")?;
        let version = field_string(dump, "version")?;
        let maintainers = field_string_list(dump, "maintainers")?;
        let overrides = optional_field_string(dump, "overrides")?;
        let enhances = optional_field_string(dump, "enhances")?;
        Some(Self {
            schema_version,
            id,
            title,
            description,
            triggers,
            form,
            body_path,
            expands_to,
            parameters,
            invocation,
            tools,
            version,
            maintainers,
            overrides,
            enhances,
        })
    }

    /// The canonical `model_dump(mode="json")` value (the Source `record_json`).
    ///
    /// This is the one serializer of the typed record: the installer's
    /// `source_hash` payload and the repository's overlay merge / include
    /// mapping both consume it, so a second record serializer cannot drift.
    /// `None` optional fields are materialised as JSON `null`, exactly like
    /// `model_dump` without `exclude_none`.
    #[must_use]
    pub fn model_dump_json(&self) -> PythonJsonValue {
        let mut fields: BTreeMap<&str, PythonJsonValue> = BTreeMap::new();
        fields.insert("schema_version", json_string(&self.schema_version));
        fields.insert("id", json_string(&self.id));
        fields.insert("title", json_string(&self.title));
        fields.insert("description", json_string(&self.description));
        fields.insert(
            "triggers",
            PythonJsonValue::Array(self.triggers.iter().map(json_string).collect()),
        );
        fields.insert("form", PythonJsonValue::scalar(self.form.as_str()));
        fields.insert("body_path", json_optional_string(self.body_path.as_ref()));
        fields.insert(
            "expands_to",
            match &self.expands_to {
                Some(expansion) => {
                    let mut inner: BTreeMap<&str, PythonJsonValue> = BTreeMap::new();
                    inner.insert("target", json_string(&expansion.target));
                    inner.insert("args", json_string(&expansion.args));
                    PythonJsonValue::Object(
                        inner
                            .into_iter()
                            .map(|(key, value)| (key.to_string(), value))
                            .collect(),
                    )
                }
                None => PythonJsonValue::Null,
            },
        );
        fields.insert(
            "parameters",
            PythonJsonValue::Array(
                self.parameters
                    .iter()
                    .map(|parameter| {
                        let mut inner: BTreeMap<&str, PythonJsonValue> = BTreeMap::new();
                        inner.insert("name", json_string(&parameter.name));
                        inner.insert("required", PythonJsonValue::Bool(parameter.required));
                        inner.insert("description", json_string(&parameter.description));
                        inner.insert("default", json_optional_string(parameter.default.as_ref()));
                        PythonJsonValue::Object(
                            inner
                                .into_iter()
                                .map(|(key, value)| (key.to_string(), value))
                                .collect(),
                        )
                    })
                    .collect(),
            ),
        );
        let mut invocation: BTreeMap<&str, PythonJsonValue> = BTreeMap::new();
        invocation.insert(
            "model_invocable",
            PythonJsonValue::Bool(self.invocation.model_invocable),
        );
        invocation.insert(
            "side_effects",
            PythonJsonValue::Array(
                self.invocation
                    .side_effects
                    .iter()
                    .map(json_string)
                    .collect(),
            ),
        );
        invocation.insert(
            "user_invocable",
            PythonJsonValue::Bool(self.invocation.user_invocable),
        );
        fields.insert(
            "invocation",
            PythonJsonValue::Object(
                invocation
                    .into_iter()
                    .map(|(key, value)| (key.to_string(), value))
                    .collect(),
            ),
        );
        fields.insert(
            "tools",
            PythonJsonValue::Array(self.tools.iter().map(json_string).collect()),
        );
        fields.insert("version", json_string(&self.version));
        fields.insert(
            "maintainers",
            PythonJsonValue::Array(self.maintainers.iter().map(json_string).collect()),
        );
        fields.insert("overrides", json_optional_string(self.overrides.as_ref()));
        fields.insert("enhances", json_optional_string(self.enhances.as_ref()));
        PythonJsonValue::Object(
            fields
                .into_iter()
                .map(|(key, value)| (key.to_string(), value))
                .collect(),
        )
    }

    /// The record as a constructed YAML mapping **plus** the transport markers
    /// its lossy strings need (the reference's `{**built_in.model_dump(),
    /// **project_data}` overlay input and the mapping the offering include
    /// payload is rendered from).
    ///
    /// This is a structural projection of [`Self::model_dump_json`], not a text
    /// (de)serialisation: a string whose code points are not all Unicode scalars
    /// is spelled by its injective transport marker and registered in the
    /// returned table, so feeding the pair back through
    /// [`crate::offering::value::exact_merged_graph`] returns the same exact
    /// record. Nothing is replaced, escaped as data, or dropped.
    #[must_use]
    pub fn exact_mapping(&self) -> Option<(Value, BTreeMap<String, PythonString>)> {
        let mut markers = BTreeMap::new();
        let value = json_to_yaml(&self.model_dump_json(), &mut markers)?;
        Some((value, markers))
    }
}

/// The canonical graph of one exact model mapping and its transport markers.
///
/// This is the crate's ONE merged-graph authority
/// ([`crate::offering::value::exact_merged_graph`]) exposed for callers that hold
/// a [`PackSkill::exact_mapping`] pair and must read the exact payload from it
/// (the offering include bridge does the same thing internally). `None` is a
/// capability boundary — the mapping has no faithful canonical form — never
/// "artifact missing".
#[must_use]
pub fn exact_mapping_graph(
    mapping: &Value,
    markers: &BTreeMap<String, PythonString>,
) -> Option<sk_kernel::python_yaml::ObjectGraph> {
    crate::offering::value::exact_merged_graph(mapping, markers)
}

/// One exact Python string as a JSON value.
fn json_string(value: &PythonString) -> PythonJsonValue {
    PythonJsonValue::String(value.clone())
}

/// `str | None` as a JSON value (`null` when absent).
fn json_optional_string(value: Option<&PythonString>) -> PythonJsonValue {
    match value {
        Some(value) => json_string(value),
        None => PythonJsonValue::Null,
    }
}

/// A post-validation failure (the reference's `PackSkillViolation`), or the
/// Python exception it raises when the violation is not a `ValueError` subclass
/// the loader catches (invalid UTF-8 body text).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackSkillViolation {
    message: String,
    fatal_python: Option<&'static str>,
}

impl PackSkillViolation {
    fn violation(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            fatal_python: None,
        }
    }

    fn fatal(python: &'static str, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            fatal_python: Some(python),
        }
    }

    /// The message the reference prints in the skip warning / load error.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The Python exception name when this aborts the load.
    #[must_use]
    pub fn fatal_python(&self) -> Option<&'static str> {
        self.fatal_python
    }
}

impl std::fmt::Display for PackSkillViolation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for PackSkillViolation {}

/// The rendered name a skill renders under: `<namespace>-<id>` or the bare id.
///
/// The exact owner: the marketplace/skill directory name is `namespace-id` in
/// code points, so a namespace that reaches here is appended verbatim (the
/// grammar is checked by [`skill_namespace_violation_exact`] before rendering).
#[must_use]
pub fn rendered_name_exact(
    skill_id: &PythonString,
    namespace: Option<&PythonString>,
) -> PythonString {
    let mut name = PythonString::default();
    if let Some(namespace) = namespace {
        name.append(namespace);
        name.push_scalars("-");
    }
    name.append(skill_id);
    name
}

/// [`rendered_name_exact`] for callers that already hold Rust text.
///
/// A `&str` id/namespace contains only Unicode scalars, so the rendered name is
/// scalar by construction; the conversion cannot lose a code point here.
#[must_use]
pub fn rendered_name(skill_id: &str, namespace: Option<&str>) -> String {
    let id = PythonString::from_scalars(skill_id);
    let namespace = namespace.map(PythonString::from_scalars);
    rendered_name_exact(&id, namespace.as_ref())
        .to_scalar_string()
        .expect("a scalar id and namespace render a scalar name")
}

/// Why the exact code points `value` are not a valid skill namespace.
///
/// The ONE grammar (lowercase ASCII kebab-case, at most
/// [`SKILL_NAMESPACE_MAX_LENGTH`] code points). The namespace becomes a
/// directory-name prefix, so it is refused — never normalised.
#[must_use]
pub fn skill_namespace_violation_exact(value: &PythonString) -> Option<String> {
    if value.codepoints().len() <= SKILL_NAMESPACE_MAX_LENGTH
        && value
            .to_scalar_string()
            .is_ok_and(|text| is_kebab_ascii(&text))
    {
        return None;
    }
    Some(namespace_violation_message(&value.repr()))
}

/// The scalar capability view of [`skill_namespace_violation_exact`].
#[must_use]
pub fn skill_namespace_violation(value: &str) -> Option<String> {
    skill_namespace_violation_exact(&PythonString::from_scalars(value))
}

fn namespace_violation_message(rendered: &str) -> String {
    format!(
        "skill namespace {rendered} is not valid: it must be lowercase ASCII kebab-case \
         (letters and digits joined by single '-', starting with a letter, at most \
         {SKILL_NAMESPACE_MAX_LENGTH} characters)"
    )
}

/// The one rendered-description composer: Python `str.split()` whitespace
/// collapsed, plus `" Triggers: a, b."` when triggers are declared.
#[must_use]
pub fn rendered_description(skill: &PackSkill) -> PythonString {
    let mut description = PythonString::default();
    for (position, run) in python_split_whitespace_codepoints(skill.description.codepoints())
        .into_iter()
        .enumerate()
    {
        if position > 0 {
            description.push_scalars(" ");
        }
        for code in run {
            description
                .push_codepoint(*code)
                .expect("a code point came from a PythonString");
        }
    }
    if !skill.triggers.is_empty() {
        description.push_scalars(" Triggers: ");
        for (position, trigger) in skill.triggers.iter().enumerate() {
            if position > 0 {
                description.push_scalars(", ");
            }
            description.append(trigger);
        }
        description.push_scalars(".");
    }
    description
}

/// Why the rendered name or description exceeds an Agent Skills limit.
///
/// Both limits count Python code points (`len(str)`), and the messages quote the
/// id and name with Python `repr()`.
#[must_use]
pub fn skill_limit_violation(skill: &PackSkill, name: &PythonString) -> Option<String> {
    let name_length = name.codepoints().len();
    if name_length > MAX_SKILL_NAME_LENGTH {
        return Some(format!(
            "skill {}: rendered name {} is {name_length} characters, over the \
             {MAX_SKILL_NAME_LENGTH}-character skill name limit; shorten the skill id or namespace",
            skill.id.repr(),
            name.repr()
        ));
    }
    let description = rendered_description(skill);
    let description_length = description.codepoints().len();
    if description_length > MAX_SKILL_DESCRIPTION_LENGTH {
        return Some(format!(
            "skill {}: rendered description (with its triggers) is {description_length} characters, over the \
             {MAX_SKILL_DESCRIPTION_LENGTH}-character skill description limit; shorten the description or triggers",
            skill.id.repr()
        ));
    }
    None
}

/// Validate one admitted record for the tier it came from
/// (`validate_pack_skill`).
///
/// # Errors
///
/// The first violation, in the reference's order (body, limits, then the
/// non-built-in rules).
pub fn validate_pack_skill(
    skill: &PackSkill,
    skill_file: &Path,
    tier: PackTier,
    namespace: Option<&PythonString>,
) -> Result<(), PackSkillViolation> {
    let needs_body = skill.form == SkillForm::Prompt && skill.enhances.is_none();
    let body_text = if needs_body {
        Some(read_body(skill, skill_file)?)
    } else {
        None
    };
    let name = if tier == PackTier::BuiltIn {
        skill.id.clone()
    } else {
        rendered_name_exact(&skill.id, namespace)
    };
    if let Some(limit) = skill_limit_violation(skill, &name) {
        return Err(PackSkillViolation::violation(limit));
    }
    if tier == PackTier::BuiltIn {
        return Ok(());
    }
    for candidate in [&skill.id, &name] {
        if RESERVED_PREFIXES
            .iter()
            .any(|prefix| starts_with_codepoints(candidate, prefix))
        {
            return Err(PackSkillViolation::violation(format!(
                "skill {}: name {} uses a prefix reserved for built-in skills {:?}",
                skill.id.repr(),
                candidate.repr(),
                RESERVED_PREFIXES.to_vec()
            )));
        }
    }
    let parent = skill_file.parent().unwrap_or_else(|| Path::new("."));
    let id_directory = skill.id.to_scalar_string().unwrap_or_default();
    for scripts in [
        parent.join("scripts"),
        parent.join(&id_directory).join("scripts"),
    ] {
        if scripts.is_dir() {
            return Err(PackSkillViolation::violation(format!(
                "skill {}: a 'scripts/' directory is not allowed for org/project skills ({})",
                skill.id.repr(),
                scripts.display()
            )));
        }
    }
    if let Some(body) = body_text {
        let forbidden = forbidden_frontmatter(&body)?;
        if !forbidden.is_empty() {
            return Err(PackSkillViolation::violation(format!(
                "skill {}: permission-widening frontmatter key(s) {forbidden:?} are not allowed for org/project skills",
                skill.id.repr()
            )));
        }
    }
    Ok(())
}

/// Fold an `enhances` record into its base (`apply_enhancement`).
///
/// # Errors
///
/// When the enhancer changes the body/expansion, changes the parameter shape or
/// widens invocation.
pub fn apply_enhancement(
    base: &PackSkill,
    enhancer: &PackSkill,
) -> Result<PackSkill, PackSkillViolation> {
    if (enhancer.form, &enhancer.body_path, &enhancer.expands_to)
        != (base.form, &base.body_path, &base.expands_to)
    {
        return Err(PackSkillViolation::violation(format!(
            "skill {}: 'enhances' may not change the body or expansion of {}",
            enhancer.id.repr(),
            base.id.repr()
        )));
    }
    let shape = |parameters: &[SkillParameter]| -> BTreeMap<PythonString, bool> {
        parameters
            .iter()
            .map(|item| (item.name.clone(), item.required))
            .collect()
    };
    if shape(&base.parameters) != shape(&enhancer.parameters) {
        return Err(PackSkillViolation::violation(format!(
            "skill {}: 'enhances' may only change parameter defaults, not add, remove or re-require parameters",
            enhancer.id.repr()
        )));
    }
    let base_side: BTreeSet<&PythonString> = base.invocation.side_effects.iter().collect();
    let enhancer_side: BTreeSet<&PythonString> = enhancer.invocation.side_effects.iter().collect();
    let widened = (enhancer.invocation.user_invocable && !base.invocation.user_invocable)
        || (enhancer.invocation.model_invocable && !base.invocation.model_invocable)
        || !base_side.is_subset(&enhancer_side);
    if widened {
        return Err(PackSkillViolation::violation(
            "'enhances' may only narrow invocation (never widen it or drop a declared side effect)",
        ));
    }
    let mut invocation = SkillInvocation {
        user_invocable: enhancer.invocation.user_invocable,
        model_invocable: enhancer.invocation.model_invocable,
        side_effects: enhancer.invocation.side_effects.clone(),
    };
    if !invocation.side_effects.is_empty() {
        invocation.model_invocable = false;
    }
    Ok(PackSkill {
        schema_version: base.schema_version.clone(),
        id: base.id.clone(),
        title: enhancer.title.clone(),
        description: enhancer.description.clone(),
        triggers: enhancer.triggers.clone(),
        form: base.form,
        body_path: base.body_path.clone(),
        expands_to: base.expands_to.clone(),
        parameters: enhancer.parameters.clone(),
        invocation,
        tools: enhancer.tools.clone(),
        version: enhancer.version.clone(),
        maintainers: enhancer.maintainers.clone(),
        overrides: None,
        enhances: None,
    })
}

// ---------------------------------------------------------------------------
// Repository
// ---------------------------------------------------------------------------

/// Why the canonical pack-skill repository cannot load at all.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackSkillLoadError {
    message: String,
    fatal: Option<(&'static str, String)>,
}

impl PackSkillLoadError {
    fn conflict(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            fatal: None,
        }
    }

    fn fatal(python: &'static str, message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            message: message.clone(),
            fatal: Some((python, message)),
        }
    }

    /// The Source-shaped cause.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The Python exception name when this aborts the whole load.
    #[must_use]
    pub fn fatal_python(&self) -> Option<(&'static str, &str)> {
        self.fatal
            .as_ref()
            .map(|(python, message)| (*python, message.as_str()))
    }
}

impl std::fmt::Display for PackSkillLoadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for PackSkillLoadError {}

/// One loaded record with the effective (folded) model mapping.
///
/// `mapping` + `markers` are the ONE carrier the offering include payload reads:
/// the exact model dump projected into a constructed mapping, with a transport
/// marker for any string a plain YAML scalar cannot spell. `record` is the exact
/// typed view of the same dump, never a second authority.
#[derive(Clone)]
pub(crate) struct LoadedSkill {
    record: PackSkill,
    mapping: Value,
    markers: BTreeMap<String, PythonString>,
    tier: PackTier,
    source: PathBuf,
    document: ArtifactDocument,
}

/// One admitted record with its exact mapping carrier: the typed record, the
/// marker-carrying model-dump mapping and the transport markers that mapping needs.
type RecordCarrier = (PackSkill, Value, BTreeMap<String, PythonString>);

/// One effective record as the offering service serves it (mapping + source
/// document for the include renderer).
pub(crate) struct SkillOfferingRecord {
    /// The repository key (`obj.id` / the overlay's raw id).
    pub(crate) key: PythonString,
    /// The effective model-dump mapping (marker-carried where needed).
    pub(crate) mapping: Value,
    /// The union of every contributing layer's transport markers.
    pub(crate) markers: BTreeMap<String, PythonString>,
    /// The declaring document (for the lossless read authority).
    pub(crate) document: ArtifactDocument,
}

/// The loaded pack-skill catalog across the built-in, org and project tiers.
///
/// Mirrors the reference's `PackSkillRepository` after `_load()` +
/// `_resolve_augmentations()`: `items` holds the effective records under the
/// reference's repository keys, with the provenance and source path each
/// resolved record should report.
pub struct PackSkillRepository {
    items: BTreeMap<PythonString, LoadedSkill>,
    /// The tier render namespace the reference constructor accepts (the
    /// offering service passes `None`); validated by the canonical grammar.
    namespace: Option<PythonString>,
    warnings: Vec<String>,
}

impl PackSkillRepository {
    /// Load the catalog from the tier directories.
    ///
    /// `built_in_dir`, `org_dirs` (declaration order) and `project_dir` are the
    /// `skills/` leaf of each tier. `namespace` is the constructor argument of
    /// the reference repository; the offering service passes `None` (the
    /// tier-specific render namespace belongs to preparation), exactly as
    /// `CharterOfferingService.skills` does.
    ///
    /// # Errors
    ///
    /// [`PackSkillLoadError`] for a sibling-org id conflict, a Python exception
    /// the reference loader does not catch (unhashable overlay id, invalid
    /// UTF-8 body), or an unreadable directory.
    pub fn load(
        built_in_dir: &Path,
        org_dirs: &[PathBuf],
        project_dir: Option<&Path>,
        namespace: Option<&str>,
    ) -> Result<Self, PackSkillLoadError> {
        let mut repository = Self {
            items: BTreeMap::new(),
            namespace: namespace.map(PythonString::from_scalars),
            warnings: Vec::new(),
        };
        let built_in = repository.load_built_in(built_in_dir)?;
        let mut org_origin: BTreeMap<PythonString, PathBuf> = BTreeMap::new();
        for directory in org_dirs {
            repository.load_overlay(directory, PackTier::Org, &built_in, &mut org_origin)?;
        }
        if let Some(directory) = project_dir {
            repository.load_overlay(directory, PackTier::Project, &built_in, &mut org_origin)?;
        }
        repository.resolve_augmentations();
        Ok(repository)
    }

    fn load_built_in(
        &mut self,
        directory: &Path,
    ) -> Result<BTreeMap<PythonString, LoadedSkill>, PackSkillLoadError> {
        let mut built_in: BTreeMap<PythonString, LoadedSkill> = BTreeMap::new();
        if !directory.is_dir() {
            return Ok(built_in);
        }
        for file in skill_files(directory)? {
            let Some(document) = self.read_document(&file, "built-in")? else {
                continue;
            };
            let Some(key) = model::model_id(ArtifactKind::Skill, &document) else {
                self.warn_skip("built-in", &file, "the whole-model admission rejected it");
                continue;
            };
            let Some((record, mapping, markers)) = self.record_of(&document, "built-in", &file)?
            else {
                continue;
            };
            if let Err(violation) = self.post_validate(&record, &file, PackTier::BuiltIn) {
                if let Some(python) = violation.fatal_python() {
                    return Err(PackSkillLoadError::fatal(python, violation.message()));
                }
                self.warn_skip("built-in", &file, violation.message());
                continue;
            }
            let loaded = LoadedSkill {
                record,
                mapping,
                markers,
                tier: PackTier::BuiltIn,
                source: file,
                document,
            };
            built_in.insert(key.clone(), loaded.clone());
            self.items.insert(key, loaded);
        }
        Ok(built_in)
    }

    fn load_overlay(
        &mut self,
        directory: &Path,
        tier: PackTier,
        built_in: &BTreeMap<PythonString, LoadedSkill>,
        org_origin: &mut BTreeMap<PythonString, PathBuf>,
    ) -> Result<(), PackSkillLoadError> {
        if !directory.is_dir() {
            return Ok(());
        }
        let layer = tier.as_token();
        for file in skill_files(directory)? {
            let Some(document) = self.read_document(&file, layer)? else {
                continue;
            };
            let Some(raw) = document.value.get("id") else {
                self.warn_skip(layer, &file, "no id");
                continue;
            };
            if !python_truthy(raw) {
                self.warn_skip(layer, &file, "no id");
                continue;
            }
            let raw_key = raw_repository_key(raw, &document)?;
            let shadowed = raw_key
                .as_ref()
                .filter(|key| built_in.contains_key(*key))
                .cloned();
            let (key, record, mapping, markers) = match shadowed {
                Some(key) => {
                    // `_merge_overlay_item`: merge the overlay onto the
                    // *built-in* item's model dump, then re-validate. The two
                    // layers' transport markers are unioned so a built-in field
                    // the overlay inherits still canonicalises to its exact code
                    // points (the marker spelling is content-addressed, so the
                    // union cannot alias another string).
                    let base = &built_in[&key];
                    let merged = merge_mapping(&base.mapping, &document.value);
                    // Admission runs over the merged mapping with the *overlay's*
                    // marker table: the only fields it inspects for the constrained
                    // `str` rule are `title`/`description`/`parameters[].name`, and a
                    // built-in record that carried a surrogate in one of those could
                    // never have loaded (pydantic refuses it), so a lower-tier marker
                    // here can only spell an unconstrained field — where the marker
                    // spelling is indistinguishable from the data for every
                    // admission check (emptiness, literals, grammar). The exact
                    // record itself is read from the union-marked graph below.
                    if model::admits_mapping(ArtifactKind::Skill, &document, &merged).is_none() {
                        self.warn_skip(layer, &file, "the merged record failed admission");
                        continue;
                    }
                    let mut markers = base.markers.clone();
                    for (marker, exact) in document.markers() {
                        markers.insert(marker.clone(), exact.clone());
                    }
                    let Some(graph) = crate::offering::value::exact_merged_graph(&merged, &markers)
                    else {
                        self.warn_skip(layer, &file, "the merged record has no canonical graph");
                        continue;
                    };
                    let Some((record, mapping, record_markers)) =
                        record_of_graph(graph.root_view(), &mut self.warnings, layer, &file)
                    else {
                        continue;
                    };
                    markers.extend(record_markers);
                    (key, record, mapping, markers)
                }
                None => {
                    let Some(key) = model::model_id(ArtifactKind::Skill, &document) else {
                        self.warn_skip(layer, &file, "the whole-model admission rejected it");
                        continue;
                    };
                    let Some((record, mapping, markers)) =
                        self.record_of(&document, layer, &file)?
                    else {
                        continue;
                    };
                    (key, record, mapping, markers)
                }
            };
            if let Err(violation) = self.post_validate(&record, &file, tier) {
                if let Some(python) = violation.fatal_python() {
                    return Err(PackSkillLoadError::fatal(python, violation.message()));
                }
                self.warn_skip(layer, &file, violation.message());
                continue;
            }
            if tier == PackTier::Org {
                let root = directory.to_path_buf();
                let previous = org_origin
                    .entry(record.id.clone())
                    .or_insert_with(|| root.clone());
                if *previous != root {
                    return Err(PackSkillLoadError::conflict(format!(
                        "skill id {} is declared by two org packs ({} and {}); sibling packs must not share a skill id",
                        record.id.repr(),
                        previous.display(),
                        root.display()
                    )));
                }
            }
            let loaded = LoadedSkill {
                record,
                mapping,
                markers,
                tier,
                source: file,
                document,
            };
            self.items.insert(key, loaded);
        }
        Ok(())
    }

    fn read_document(
        &mut self,
        file: &Path,
        layer: &str,
    ) -> Result<Option<ArtifactDocument>, PackSkillLoadError> {
        match ArtifactDocument::read(file) {
            Ok(document) => Ok(Some(document)),
            Err(error) => match error.fatal() {
                Some((python, message)) => Err(PackSkillLoadError::fatal(python, message)),
                None => {
                    self.warn_skip(layer, file, &error.to_string());
                    Ok(None)
                }
            },
        }
    }

    fn record_of(
        &mut self,
        document: &ArtifactDocument,
        layer: &str,
        file: &Path,
    ) -> Result<Option<RecordCarrier>, PackSkillLoadError> {
        Ok(record_of_graph(
            document.root(),
            &mut self.warnings,
            layer,
            file,
        ))
    }

    fn post_validate(
        &self,
        record: &PackSkill,
        file: &Path,
        tier: PackTier,
    ) -> Result<(), PackSkillViolation> {
        validate_pack_skill(record, file, tier, self.namespace.as_ref())
    }

    fn warn_skip(&mut self, layer: &str, file: &Path, detail: &str) {
        self.warnings.push(skip_message(layer, file, detail));
    }

    fn resolve_augmentations(&mut self) {
        let mut augmenters: Vec<(PythonString, LoadedSkill)> = self
            .items
            .iter()
            .filter(|(_, loaded)| loaded.record.is_augmentation())
            .map(|(key, loaded)| (key.clone(), loaded.clone()))
            .collect();
        augmenters.sort_by(|left, right| left.1.record.id.cmp(&right.1.record.id));
        // The reference pops each augmenter under its *model* id, not under a
        // raw overlay key; a raw-keyed leftover stays served there too.
        for (_, loaded) in &augmenters {
            self.items.remove(&loaded.record.id);
        }
        for (_, loaded) in augmenters {
            let target_id = loaded
                .record
                .overrides
                .clone()
                .or_else(|| loaded.record.enhances.clone())
                .unwrap_or_default();
            let target_key = target_id.clone();
            let Some(base) = self.items.get(&target_key).cloned() else {
                self.warn_fold(
                    &loaded,
                    &format!(
                        "target skill {} is not loaded from a lower tier",
                        target_id.repr()
                    ),
                );
                continue;
            };
            if base.tier.precedence_rank() >= loaded.tier.precedence_rank() {
                self.warn_fold(
                    &loaded,
                    &format!(
                        "target {} ({}) must come from a lower tier than {}",
                        target_id.repr(),
                        base.tier.as_token(),
                        loaded.tier.as_token()
                    ),
                );
                continue;
            }
            let resolved = if loaded.record.enhances.is_some() {
                match apply_enhancement(&base.record, &loaded.record) {
                    Ok(resolved) => resolved,
                    Err(violation) => {
                        self.warn_fold(&loaded, violation.message());
                        continue;
                    }
                }
            } else {
                if base.tier == PackTier::BuiltIn {
                    self.warn_fold(
                        &loaded,
                        &format!(
                            "overriding built-in skill {} requires the replaceable-builtins allowlist, which skills do not support yet",
                            target_id.repr()
                        ),
                    );
                    continue;
                }
                let mut replacement = loaded.record.clone();
                replacement.id = target_id.clone();
                replacement.overrides = None;
                replacement
            };
            let source = if loaded.record.overrides.is_some() {
                loaded.source.clone()
            } else {
                base.source.clone()
            };
            let Some((mapping, markers)) = resolved.exact_mapping() else {
                self.warn_fold(
                    &loaded,
                    "the canonical model dump cannot be carried as a mapping",
                );
                continue;
            };
            // A fold keeps the contributed record's markers plus the target's
            // own inherited ones, so a field taken from either side stays exact.
            let mut union = loaded.markers.clone();
            for (marker, exact) in &base.markers {
                union.entry(marker.clone()).or_insert_with(|| exact.clone());
            }
            union.extend(markers);
            self.items.insert(
                target_key,
                LoadedSkill {
                    record: resolved,
                    mapping,
                    markers: union,
                    tier: loaded.tier,
                    source,
                    document: loaded.document.clone(),
                },
            );
        }
    }

    fn warn_fold(&mut self, loaded: &LoadedSkill, detail: &str) {
        self.warnings.push(format!(
            "Skipping invalid {} pack-skill {}: {detail}",
            loaded.tier.as_token(),
            display_name(&loaded.source)
        ));
    }

    /// The loaded record of `skill_id`.
    #[must_use]
    pub fn skill(&self, skill_id: &str) -> Option<&PackSkill> {
        self.get(skill_id)
    }

    /// The loaded record of `skill_id` (the reference's `get`).
    #[must_use]
    pub fn get(&self, skill_id: &str) -> Option<&PackSkill> {
        self.items
            .get(&PythonString::from_scalars(skill_id))
            .map(|loaded| &loaded.record)
    }

    /// The tier `skill_id` resolved from (`provenance_of`).
    #[must_use]
    pub fn tier_of(&self, skill_id: &str) -> Option<PackTier> {
        self.items
            .get(&PythonString::from_scalars(skill_id))
            .map(|loaded| loaded.tier)
    }

    /// The canonical tier token (`built-in`/`org`/`project`).
    #[must_use]
    pub fn provenance_of(&self, skill_id: &str) -> Option<&'static str> {
        self.tier_of(skill_id).map(PackTier::as_token)
    }

    /// The `*.skill.yaml` file `skill_id` came from.
    #[must_use]
    pub fn source_path(&self, skill_id: &str) -> Option<&Path> {
        self.items
            .get(&PythonString::from_scalars(skill_id))
            .map(|loaded| loaded.source.as_path())
    }

    /// The prompt body of a loaded prompt-form skill (`body_text`).
    #[must_use]
    pub fn body_text(&self, skill_id: &str) -> Option<String> {
        let loaded = self.items.get(&PythonString::from_scalars(skill_id))?;
        let body_path = loaded.record.body_path.as_ref()?.to_scalar_string().ok()?;
        let parent = loaded.source.parent().unwrap_or_else(|| Path::new("."));
        std::fs::read_to_string(parent.join(body_path)).ok()
    }

    /// Loader warnings (records that did not load).
    #[must_use]
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Every loaded skill id, sorted.
    #[must_use]
    pub fn ids(&self) -> Vec<String> {
        // The skill id grammar is ASCII kebab-case, so every served id has a
        // scalar spelling; the exact ids stay on [`Self::exact_ids`].
        self.exact_ids()
            .into_iter()
            .filter_map(|id| id.to_scalar_string().ok())
            .collect()
    }

    /// Every loaded skill id, exactly, sorted (the skill id grammar is ASCII, so
    /// this is the same set [`Self::ids`] projects).
    #[must_use]
    pub fn exact_ids(&self) -> Vec<PythonString> {
        let mut ids: Vec<PythonString> = self
            .items
            .values()
            .map(|loaded| loaded.record.id.clone())
            .collect();
        ids.sort();
        ids
    }

    /// The effective records as the offering service serves them.
    pub(crate) fn offering_records(&self) -> Vec<SkillOfferingRecord> {
        self.items
            .iter()
            .map(|(key, loaded)| SkillOfferingRecord {
                key: key.clone(),
                mapping: loaded.mapping.clone(),
                markers: loaded.markers.clone(),
                document: loaded.document.clone(),
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The reference's per-record skip warning (one owner of the wording).
fn skip_message(layer: &str, file: &Path, detail: &str) -> String {
    format!(
        "Skipping invalid {layer} pack-skill {}: {detail}",
        display_name(file)
    )
}

/// Admit one already-constructed graph root into the exact record + carrier.
///
/// The dump is the canonical model dump over the record's **own** graph root
/// ([`crate::canonical_artifact_body_exact`]), so an authored code unit no Rust
/// scalar can hold survives into both the typed record and the include mapping.
/// A payload the exact model cannot project faithfully is a capability boundary:
/// it is reported, never replaced with an approximation.
fn record_of_graph(
    root: sk_kernel::python_yaml::NodeRef<'_>,
    warnings: &mut Vec<String>,
    layer: &str,
    file: &Path,
) -> Option<RecordCarrier> {
    let Some(dump) = crate::canonical_artifact_body_exact(ArtifactKind::Skill, root) else {
        warnings.push(skip_message(
            layer,
            file,
            "the canonical model dump is unavailable",
        ));
        return None;
    };
    let Some(record) = PackSkill::from_exact_dump(&dump) else {
        warnings.push(skip_message(
            layer,
            file,
            "the canonical model dump has no PackSkill shape",
        ));
        return None;
    };
    let Some((mapping, markers)) = record.exact_mapping() else {
        warnings.push(skip_message(
            layer,
            file,
            "the canonical model dump cannot be carried as a mapping",
        ));
        return None;
    };
    Some((record, mapping, markers))
}

fn display_name(file: &Path) -> String {
    file.file_name().map_or_else(
        || file.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

fn skill_files(directory: &Path) -> Result<Vec<PathBuf>, PackSkillLoadError> {
    let mut files = Vec::new();
    collect_skill_files(directory, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_skill_files(
    directory: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), PackSkillLoadError> {
    let entries = std::fs::read_dir(directory).map_err(|error| {
        PackSkillLoadError::conflict(format!("cannot read {}: {error}", directory.display()))
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            PackSkillLoadError::conflict(format!(
                "cannot read an entry of {}: {error}",
                directory.display()
            ))
        })?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
            PackSkillLoadError::conflict(format!("cannot inspect {}: {error}", path.display()))
        })?;
        if metadata.file_type().is_symlink() {
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(PACK_SKILL_SUFFIX))
            {
                files.push(path);
            }
            continue;
        }
        if metadata.is_dir() {
            collect_skill_files(&path, files)?;
            continue;
        }
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(PACK_SKILL_SUFFIX))
        {
            files.push(path);
        }
    }
    Ok(())
}

fn merge_mapping(base: &Value, overlay: &Value) -> Value {
    let (Value::Mapping(base), Value::Mapping(overlay)) = (base, overlay) else {
        return overlay.clone();
    };
    let mut merged = base.clone();
    for (key, value) in overlay {
        merged.insert(key.clone(), value.clone());
    }
    Value::Mapping(merged)
}

fn raw_repository_key(
    raw: &Value,
    document: &ArtifactDocument,
) -> Result<Option<PythonString>, PackSkillLoadError> {
    match raw {
        Value::String(text) => Ok(Some(document.exact_string(text))),
        Value::Tagged(_) => {
            if crate::profile_construct::constructed_bytes(raw).is_some() {
                return Ok(None);
            }
            Err(unhashable_abort(raw))
        }
        Value::Sequence(_) | Value::Mapping(_) => Err(unhashable_abort(raw)),
        _ => Ok(None),
    }
}

fn unhashable_abort(raw: &Value) -> PackSkillLoadError {
    let python_type = match raw {
        Value::Sequence(_) => "list",
        Value::Mapping(_) => "dict",
        Value::Tagged(_) => {
            if crate::profile_construct::constructed_set(raw).is_some() {
                "set"
            } else if crate::profile_construct::constructed_omap(raw).is_some() {
                "ordereddict"
            } else if crate::profile_construct::constructed_pairs(raw).is_some() {
                "list"
            } else {
                "object"
            }
        }
        _ => "object",
    };
    PackSkillLoadError::fatal("TypeError", format!("unhashable type: '{python_type}'"))
}

fn python_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|float| float != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Sequence(items) => !items.is_empty(),
        Value::Mapping(mapping) => !mapping.is_empty(),
        Value::Tagged(tagged) => python_truthy(&tagged.value),
    }
}

fn read_body(skill: &PackSkill, skill_file: &Path) -> Result<String, PackSkillViolation> {
    let parent = skill_file.parent().unwrap_or_else(|| Path::new("."));
    let root = parent.canonicalize().map_err(|error| {
        PackSkillViolation::violation(format!(
            "skill {}: cannot resolve {}: {error}",
            skill.id.repr(),
            parent.display()
        ))
    })?;
    let authored = skill.body_path.clone().unwrap_or_default();
    // A path is a filesystem string: the reference's `Path.resolve()` encodes it
    // with the filesystem codec and raises `UnicodeEncodeError` for a surrogate
    // unit. That phase escapes the loader's per-file catch, so it is fatal here.
    let relative = authored.to_scalar_string().map_err(|_| {
        let message = sk_kernel::python_utf8::encode_python_string(&authored)
            .expect_err("a surrogate-bearing path has no UTF-8 form");
        PackSkillViolation::fatal("UnicodeEncodeError", message.to_string())
    })?;
    // `Path.resolve()` is non-strict: the containment check runs before the
    // existence check, so `../secret.md` is an escape even when the target is
    // absent. Resolve lexically over the canonical parent, following every
    // existing symlink component (a symlink target inside is allowed, outside
    // escapes).
    let resolved = resolve_non_strict(&root, Path::new(&relative));
    if !resolved.starts_with(&root) {
        return Err(PackSkillViolation::violation(format!(
            "skill {}: body_path {} escapes the skill directory",
            skill.id.repr(),
            authored.repr()
        )));
    }
    if !resolved.is_file() {
        return Err(PackSkillViolation::violation(format!(
            "skill {}: body file {} not found",
            skill.id.repr(),
            authored.repr()
        )));
    }
    let bytes = std::fs::read(&resolved).map_err(|error| {
        PackSkillViolation::violation(format!(
            "skill {}: cannot read body {}: {error}",
            skill.id.repr(),
            authored.repr()
        ))
    })?;
    String::from_utf8(bytes).map_err(|error| {
        PackSkillViolation::fatal(
            "ValueError",
            format!(
                "'utf-8' codec can't decode body {} of skill {}: {error}",
                authored.repr(),
                skill.id.repr()
            ),
        )
    })
}

/// Non-strict `Path.resolve()`: normalise `.`/`..`, follow existing symlink
/// components, and leave a missing tail unresolved.
fn resolve_non_strict(root: &Path, relative: &Path) -> PathBuf {
    use std::path::Component;
    let mut resolved = if relative.is_absolute() {
        PathBuf::new()
    } else {
        root.to_path_buf()
    };
    for component in relative.components() {
        match component {
            Component::Prefix(prefix) => resolved = PathBuf::from(prefix.as_os_str()),
            Component::RootDir => resolved = PathBuf::from(std::path::MAIN_SEPARATOR.to_string()),
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = resolved.pop();
            }
            Component::Normal(name) => {
                resolved.push(name);
                if let Ok(canonical) = resolved.canonicalize() {
                    resolved = canonical;
                }
            }
        }
    }
    resolved
}

fn forbidden_frontmatter(body: &str) -> Result<Vec<String>, PackSkillViolation> {
    let lines = python_splitlines(body);
    let Some(first) = lines.first() else {
        return Ok(Vec::new());
    };
    if python_strip(first) != "---" {
        return Ok(Vec::new());
    }
    let mut end = None;
    for (index, line) in lines.iter().enumerate().skip(1) {
        if python_strip(line) == "---" {
            end = Some(index);
            break;
        }
    }
    let Some(end) = end else {
        return Err(PackSkillViolation::violation(
            "body frontmatter is not closed",
        ));
    };
    let block = lines[1..end].join("\n");
    let value = construct_yaml(&block).map_err(|error| {
        PackSkillViolation::violation(format!("body frontmatter is not valid YAML: {error}"))
    })?;
    let mapping = match value {
        Value::Null => return Ok(Vec::new()),
        Value::Mapping(mapping) => mapping,
        _ => {
            return Err(PackSkillViolation::violation(
                "body frontmatter must be a mapping",
            ));
        }
    };
    let mut forbidden: Vec<String> = mapping
        .keys()
        .filter_map(Value::as_str)
        .map(str::to_lowercase)
        .filter(|key| FORBIDDEN_FRONTMATTER_KEYS.contains(&key.as_str()))
        .collect();
    forbidden.sort();
    Ok(forbidden)
}

fn construct_yaml(text: &str) -> Result<Value, String> {
    let lossless = crate::profile_text::transport(text).map_err(|error| error.to_string())?;
    let mut value: Value =
        serde_yaml_ng::from_str(lossless.shadow()).map_err(|error| error.to_string())?;
    crate::profile_text::verify_markers(&value, lossless.markers())
        .map_err(|error| error.to_string())?;
    crate::profile_text::resolve_merge_keys(&mut value).map_err(|error| error.to_string())?;
    crate::profile_construct::construct(&mut value, lossless.markers())
        .map_err(|error| error.to_string())?;
    // Build the one canonical graph too, so a marker-carrying string is
    // resolved through the same authority as every other document read.
    Ok(value)
}

/// Project a canonical model dump into a constructed YAML mapping.
///
/// A string whose code points are all Unicode scalars becomes a plain
/// `Value::String`; a string with a surrogate unit is spelled by its injective
/// transport marker and registered in `markers`, so the pair
/// `(value, markers)` re-constructs the same exact record through the canonical
/// graph builder. This is a structural projection: no text is serialised and
/// re-parsed, and no member is dropped or invented (a member shape the model
/// dump never produces returns `None` for the whole payload).
fn json_to_yaml(
    value: &PythonJsonValue,
    markers: &mut BTreeMap<String, PythonString>,
) -> Option<Value> {
    Some(match value {
        PythonJsonValue::Null => Value::Null,
        PythonJsonValue::Bool(flag) => Value::Bool(*flag),
        PythonJsonValue::String(text) => Value::String(marker_carried(text, markers)),
        PythonJsonValue::Array(items) => Value::Sequence(
            items
                .iter()
                .map(|item| json_to_yaml(item, markers))
                .collect::<Option<Vec<_>>>()?,
        ),
        PythonJsonValue::Object(entries) => {
            let mut mapping = Mapping::new();
            for (key, value) in entries {
                mapping.insert(Value::String(key.clone()), json_to_yaml(value, markers)?);
            }
            Value::Mapping(mapping)
        }
        PythonJsonValue::ExactObject(dict) => {
            let mut mapping = Mapping::new();
            for (key, value) in dict.iter() {
                let key = match key.to_scalar_string() {
                    Ok(text) => text,
                    Err(_) => {
                        let marker = sk_kernel::python_yaml::marker_for_exact(key);
                        markers.insert(marker.clone(), key.clone());
                        marker
                    }
                };
                mapping.insert(Value::String(key), json_to_yaml(value, markers)?);
            }
            Value::Mapping(mapping)
        }
        // A `PackSkill` dump has no numeric member; a number here is a shape this
        // projection cannot carry, so the payload refuses instead of stringifying.
        PythonJsonValue::Integer(_) | PythonJsonValue::Number(_) | PythonJsonValue::Float(_) => {
            return None;
        }
    })
}

/// The YAML spelling of one exact string: itself when scalar-representable,
/// else its injective transport marker (registered in `markers`).
fn marker_carried(text: &PythonString, markers: &mut BTreeMap<String, PythonString>) -> String {
    match text.to_scalar_string() {
        Ok(scalar) => scalar,
        Err(_) => {
            let marker = sk_kernel::python_yaml::marker_for_exact(text);
            markers.insert(marker.clone(), text.clone());
            marker
        }
    }
}

fn field<'a>(dump: &'a PythonJsonValue, key: &str) -> Option<&'a PythonJsonValue> {
    match dump {
        PythonJsonValue::ExactObject(dict) => dict.get(key),
        PythonJsonValue::Object(entries) => entries
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value),
        _ => None,
    }
}

/// One exact Python string field.
fn field_string(dump: &PythonJsonValue, key: &str) -> Option<PythonString> {
    match field(dump, key)? {
        PythonJsonValue::String(text) => Some(text.clone()),
        _ => None,
    }
}

/// One `str | None` field (`None`/absent both mean "absent").
fn optional_field_string(dump: &PythonJsonValue, key: &str) -> Option<Option<PythonString>> {
    match field(dump, key) {
        None | Some(PythonJsonValue::Null) => Some(None),
        Some(PythonJsonValue::String(text)) => Some(Some(text.clone())),
        Some(_) => None,
    }
}

fn field_bool(dump: &PythonJsonValue, key: &str) -> Option<bool> {
    match field(dump, key)? {
        PythonJsonValue::Bool(flag) => Some(*flag),
        _ => None,
    }
}

/// One `list[str]` field (`None`/absent is the model's empty default).
fn field_string_list(dump: &PythonJsonValue, key: &str) -> Option<Vec<PythonString>> {
    match field(dump, key) {
        None | Some(PythonJsonValue::Null) => Some(Vec::new()),
        Some(PythonJsonValue::Array(items)) => items
            .iter()
            .map(|item| match item {
                PythonJsonValue::String(text) => Some(text.clone()),
                _ => None,
            })
            .collect(),
        Some(_) => None,
    }
}

/// Python `str.splitlines()` (no argument).
fn python_splitlines(text: &str) -> Vec<&str> {
    let characters: Vec<(usize, char)> = text.char_indices().collect();
    let mut lines = Vec::new();
    let mut start = 0usize;
    let mut index = 0usize;
    while index < characters.len() {
        let (offset, character) = characters[index];
        let end = offset;
        let skip = match character {
            '\n' | '\r' | '\u{0b}' | '\u{0c}' | '\u{1c}' | '\u{1d}' | '\u{1e}' | '\u{85}'
            | '\u{2028}' | '\u{2029}' => {
                if character == '\r'
                    && characters
                        .get(index + 1)
                        .is_some_and(|(_, next)| *next == '\n')
                {
                    2
                } else {
                    1
                }
            }
            _ => {
                index += 1;
                continue;
            }
        };
        lines.push(&text[start..end]);
        index += skip;
        start = characters
            .get(index)
            .map_or_else(|| text.len(), |(offset, _)| *offset);
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// Python `str.split()` whitespace: Unicode White_Space plus the C0 information
/// separators that Unicode excludes.
fn is_python_space(character: char) -> bool {
    character.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&character)
}

fn python_strip(text: &str) -> &str {
    text.trim_matches(is_python_space)
}

#[cfg(test)]
fn python_split_whitespace(text: &str) -> Vec<&str> {
    text.split(is_python_space)
        .filter(|part| !part.is_empty())
        .collect()
}

/// [`is_python_space`] for one exact code unit.
fn is_python_space_codepoint(code: u32) -> bool {
    char::from_u32(code).is_some_and(is_python_space)
}

/// Python `str.split()` (no argument) over exact code units.
fn python_split_whitespace_codepoints(text: &[u32]) -> Vec<&[u32]> {
    let mut runs = Vec::new();
    let mut start: Option<usize> = None;
    for (position, code) in text.iter().enumerate() {
        if is_python_space_codepoint(*code) {
            if let Some(begin) = start.take() {
                runs.push(&text[begin..position]);
            }
        } else if start.is_none() {
            start = Some(position);
        }
    }
    if let Some(begin) = start {
        runs.push(&text[begin..]);
    }
    runs
}

/// Whether the exact code points of `candidate` start with the ASCII `prefix`.
///
/// Byte-prefix comparison is not usable here: one authored code point may be a
/// surrogate unit, which has no UTF-8 spelling.
fn starts_with_codepoints(candidate: &PythonString, prefix: &str) -> bool {
    let prefix: Vec<u32> = prefix.chars().map(u32::from).collect();
    candidate.codepoints().len() >= prefix.len() && candidate.codepoints().starts_with(&prefix)
}

/// `[a-z][a-z0-9]*(?:-[a-z0-9]+)*` over Rust text (the namespace grammar body).
fn is_kebab_ascii(value: &str) -> bool {
    is_kebab(value)
}

fn is_kebab(value: &str) -> bool {
    let mut characters = value.chars();
    match characters.next() {
        Some(first) if first.is_ascii_lowercase() => {}
        _ => return false,
    }
    let mut previous_dash = false;
    for character in characters {
        if character == '-' {
            if previous_dash {
                return false;
            }
            previous_dash = true;
        } else if character.is_ascii_lowercase() || character.is_ascii_digit() {
            previous_dash = false;
        } else {
            return false;
        }
    }
    !previous_dash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(text: &str) -> ArtifactDocument {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("doc.skill.yaml");
        std::fs::write(&path, text).expect("write");
        ArtifactDocument::read(&path).expect("read")
    }

    #[test]
    fn from_yaml_reads_the_canonical_model_dump() {
        let value: Value = serde_yaml_ng::from_str(
            "schema_version: '1.0'\nid: triage\ntitle: T\ndescription: D\nform: prompt\nbody_path: triage.md\nparameters:\n  - name: scope\n    required: 'yes'\ninvocation:\n  side_effects: [gh-write]\n",
        )
        .expect("yaml");
        let skill = PackSkill::from_yaml(&value).expect("admitted");
        assert_eq!(skill.id.to_scalar_string().expect("scalar"), "triage");
        assert_eq!(skill.form, SkillForm::Prompt);
        assert!(skill.parameters[0].required);
        assert!(!skill.invocation.model_invocable);
        assert_eq!(skill.tools.len(), 1);
        assert_eq!(skill.tools[0].to_scalar_string().expect("scalar"), "*");
        assert_eq!(skill.version.to_scalar_string().expect("scalar"), "1.0.0");
    }

    #[test]
    fn frontmatter_and_python_whitespace_follow_the_reference() {
        assert_eq!(
            python_splitlines("a\r\nb\rc\u{0b}d"),
            vec!["a", "b", "c", "d"]
        );
        assert_eq!(
            python_split_whitespace(" a\u{1c}b  c "),
            vec!["a", "b", "c"]
        );
        let body = "---\nAllowed-Tools: [bash]\n---\nbody\n";
        assert_eq!(
            forbidden_frontmatter(body).expect("valid"),
            vec!["allowed-tools".to_owned()]
        );
        assert!(forbidden_frontmatter("---\nkey: [\n---\n").is_err());
        assert!(forbidden_frontmatter("---\n- a\n---\n").is_err());
    }

    #[test]
    fn document_admission_and_the_exact_dump_agree_on_an_org_record() {
        let doc = document(
            "schema_version: '1.0'\nid: deploy\ntitle: T\ndescription: D\nform: prompt\nbody_path: deploy.md\n",
        );
        assert!(model::model_id(ArtifactKind::Skill, &doc).is_some());
        let dump =
            crate::canonical_artifact_body_exact(ArtifactKind::Skill, doc.root()).expect("dump");
        assert!(PackSkill::from_exact_dump(&dump).is_some());
        // The exact dump feeds the mapping carrier, and the carrier's markers
        // re-construct the same record through the canonical graph builder.
        let record = PackSkill::from_exact_dump(&dump).expect("record");
        let (mapping, markers) = record.exact_mapping().expect("mapping");
        let graph = crate::offering::value::exact_merged_graph(&mapping, &markers).expect("graph");
        let again = crate::canonical_artifact_body_exact(ArtifactKind::Skill, graph.root_view())
            .expect("dump again");
        assert_eq!(again, dump);
    }

    #[test]
    fn an_unconstrained_field_keeps_a_surrogate_and_a_constrained_one_does_not() {
        // `parameters[].default` is an unconstrained `str | None`: the reference
        // admits it and keeps the exact code unit.
        let unconstrained = document(
            "schema_version: '1.0'\nid: keep-exact\ntitle: T\ndescription: D\nform: prompt\nbody_path: keep-exact.md\nparameters:\n  - name: p\n    default: \"d\\ud800\"\n",
        );
        assert!(model::model_id(ArtifactKind::Skill, &unconstrained).is_some());
        let dump = crate::canonical_artifact_body_exact(ArtifactKind::Skill, unconstrained.root())
            .expect("dump");
        let record = PackSkill::from_exact_dump(&dump).expect("record");
        assert_eq!(
            record.parameters[0]
                .default
                .as_ref()
                .expect("default")
                .codepoints(),
            &[0x64, 0xd800]
        );
        // `title` is `Field(min_length=1)`: pydantic-core refuses the record.
        let constrained = document(
            "schema_version: '1.0'\nid: drop-exact\ntitle: \"T\\ud800\"\ndescription: D\nform: prompt\nbody_path: drop-exact.md\n",
        );
        assert!(model::model_id(ArtifactKind::Skill, &constrained).is_none());
    }
}
