//! Canonical Source typed-model view: property, default, `str` and `repr`.
//!
//! The reference's two body formatters do not read JSON. `profile_sections.
//! format_inline_named_body` computes each step header as
//! `title or description or str(step)`, and `artifact_bodies._append_step_lines`
//! interpolates `getattr(step, "title", str(step))`; both therefore need the real
//! Pydantic model — its declared property *order*, its model defaults, its
//! `str`/`repr` spelling, the enum class/member identity of a `StrEnum` member,
//! and the nested-model `repr` a list of references interpolates.
//!
//! # One schema, one walker
//!
//! This module walks the SAME [`super::schema`] `Field`/`Spec` surface the exact
//! JSON walker ([`super::exact`]) walks: field order, names, defaults and types
//! have exactly one home. It never re-declares a step/reference field roster,
//! never stringifies and re-parses a JSON dump, never invents a `Debug`/
//! marker-name wire form, and never converts a value between phases.
//!
//! The two renderings are deliberately different, because the reference's are:
//!
//! * `str(step)` is Pydantic's `__str__`: `field=repr` pairs joined by one space,
//!   with no class wrapper (`title='' description=None examples=[]`);
//! * `repr(step)` is `__repr__`: `Class(field=repr, ...)`.
//!
//! Nested values follow Python's own rules: a `list` renders through `repr` of
//! each member (`references=[TacticReference(...)]`, an empty list `[]`), and a
//! `StrEnum` member renders as `<Class.MEMBER: 'value'>` — never as its
//! serialized JSON string. Text members use the Kernel's ONE CPython repr
//! formatter, so authored quotes, backslashes, NUL, control/surrogate code
//! points, astral characters and NBSP are exactly CPython 3.11.15's.
//!
//! # Model identity, not a fabricated model
//!
//! A member list yields a model object only when every declared field is present
//! as its declared type or has the model default the reference would hold — the
//! same "an unadmitted document is never made to look model-shaped" rule the
//! exact dump follows. [`model_list`] therefore returns `Ok(None)` for a value
//! the reference's model validation would refuse (the reference never formats
//! such a document), and a member this view cannot represent yet is the distinct
//! [`PayloadFailure::UnsupportedMember`] capability boundary — never a silently
//! wrong repr and never a fake `null`.

use sk_charter::ArtifactKind;
use sk_kernel::PythonString;
use sk_kernel::python_yaml::NodeRef;

use super::exact::{PayloadError, PayloadFailure};
use super::{EnumClass, EnumMember, Model, Spec, schema};

/// One member value of a Source Pydantic model.
#[derive(Clone, Debug)]
pub enum ModelValue {
    /// A `str` member, carrying the authored code points.
    Text(PythonString),
    /// `None`: an optional member, or a model default of `None`.
    None,
    /// A `bool` member (Python spells `True`/`False`).
    Bool(bool),
    /// A `StrEnum` member: its class, member name and canonical value.
    Enum {
        /// The Source enum class name (`ArtifactKind`, `ActorRole`, …).
        class: &'static str,
        /// The declared member name (`TACTIC`, `AGENT`, …).
        member: &'static str,
        /// The member's canonical value (`tactic`, `agent`, …).
        value: &'static str,
    },
    /// A `list[...]` member, in authored order.
    List(Vec<ModelValue>),
    /// A nested model member.
    Object(ModelObject),
}

impl ModelValue {
    /// Python `str()` of this member value.
    #[must_use]
    pub fn python_str(&self) -> PythonString {
        match self {
            Self::Text(text) => text.clone(),
            Self::None => PythonString::from_scalars("None"),
            Self::Bool(flag) => PythonString::from_scalars(bool_text(*flag)),
            Self::Enum { value, .. } => PythonString::from_scalars(value),
            // A `list` has no `__str__`; Python falls back to `repr`.
            Self::List(_) => self.python_repr(),
            Self::Object(object) => object.python_str(),
        }
    }

    /// Python `repr()` of this member value.
    #[must_use]
    pub fn python_repr(&self) -> PythonString {
        match self {
            Self::Text(text) => PythonString::from_scalars(&text.repr()),
            Self::None => PythonString::from_scalars("None"),
            Self::Bool(flag) => PythonString::from_scalars(bool_text(*flag)),
            Self::Enum {
                class,
                member,
                value,
            } => {
                let mut text = PythonString::from_scalars("<");
                text.push_scalars(class);
                text.push_scalars(".");
                text.push_scalars(member);
                text.push_scalars(": ");
                text.append(&enum_value_repr(value));
                text.push_scalars(">");
                text
            }
            Self::List(items) => join_members("[", items.iter().map(Self::python_repr), "]", ", "),
            Self::Object(object) => object.python_repr(),
        }
    }

    /// Python truthiness, which the reference's `or` chains depend on.
    #[must_use]
    pub fn truthy(&self) -> bool {
        match self {
            Self::Text(text) => !text.is_empty(),
            Self::None => false,
            Self::Bool(flag) => *flag,
            // Every declared member value is a non-empty `str`.
            Self::Enum { .. } => true,
            Self::List(items) => !items.is_empty(),
            // A Pydantic `BaseModel` defines neither `__bool__` nor `__len__`.
            Self::Object(_) => true,
        }
    }

    /// The text of a `str` member, or `None` for every other value kind.
    #[must_use]
    pub fn as_text(&self) -> Option<&PythonString> {
        match self {
            Self::Text(text) => Some(text),
            _ => None,
        }
    }
}

/// One Source Pydantic model instance: its class and declared member order.
#[derive(Clone, Debug)]
pub struct ModelObject {
    class: &'static str,
    members: Vec<(&'static str, ModelValue)>,
}

impl ModelObject {
    /// The Source model class name used by `repr`.
    #[must_use]
    pub fn class(&self) -> &'static str {
        self.class
    }

    /// The declared members, in model declaration order.
    #[must_use]
    pub fn members(&self) -> &[(&'static str, ModelValue)] {
        &self.members
    }

    /// One declared member's value, by model field name.
    #[must_use]
    pub fn field(&self, name: &str) -> Option<&ModelValue> {
        self.members()
            .iter()
            .find(|(field, _)| *field == name)
            .map(|(_, value)| value)
    }

    /// Python `str()` of the model: `field=repr` pairs joined by one space.
    #[must_use]
    pub fn python_str(&self) -> PythonString {
        let mut text = PythonString::default();
        for (position, (name, value)) in self.members.iter().enumerate() {
            if position > 0 {
                text.push_scalars(" ");
            }
            text.push_scalars(name);
            text.push_scalars("=");
            text.append(&value.python_repr());
        }
        text
    }

    /// Python `repr()` of the model: `Class(field=repr, ...)`.
    #[must_use]
    pub fn python_repr(&self) -> PythonString {
        let pairs = self.members.iter().map(|(name, value)| {
            let mut pair = PythonString::from_scalars(name);
            pair.push_scalars("=");
            pair.append(&value.python_repr());
            pair
        });
        let mut text = PythonString::from_scalars(self.class());
        text.append(&join_members("(", pairs, ")", ", "));
        text
    }
}

/// The typed models of one `list[Model]` schema field of an artifact record.
///
/// `field` names the record's list field (`"steps"`); its declaration, element
/// model and field order all come from the ONE schema, never from a local roster.
///
/// * `Err(PayloadError::NoSchema)` — this crate has no typed model for `kind`;
/// * `Ok(None)` — the record is not shaped like that Source model (a missing
///   required member, a wrong value type, or a non-list value), which the
///   reference's model validation refuses before any formatter exists;
/// * `Err(PayloadError::Failed(PayloadFailure::UnsupportedMember))` — a member
///   this view cannot represent yet (a `dict`/`float`/`Any`/`SkillInvocation`
///   member), a Native capability boundary.
///
/// # Errors
///
/// [`PayloadError`] as above.
pub fn model_list(
    kind: ArtifactKind,
    root: NodeRef<'_>,
    field: &str,
) -> Result<Option<Vec<ModelObject>>, PayloadError> {
    let fields = schema(kind).ok_or(PayloadError::NoSchema)?;
    if root.as_mapping().is_none() {
        return Ok(None);
    }
    let Some(declared) = fields.iter().find(|member| member.name == field) else {
        return Ok(None);
    };
    let (model, defaulted) = match &declared.spec {
        Spec::Models(model) => (*model, true),
        Spec::RequiredModels(model) => (*model, false),
        _ => return Ok(None),
    };
    let Some(value) = root.lookup(field).filter(|value| !value.is_null()) else {
        // `default_factory=list` is an empty model list; a required list is not.
        return Ok(defaulted.then(Vec::new));
    };
    let Some(items) = sequence_members(value) else {
        return Ok(None);
    };
    let mut objects = Vec::with_capacity(items.len());
    for item in items {
        match model_object(model, item)? {
            Some(object) => objects.push(object),
            None => return Ok(None),
        }
    }
    Ok(Some(objects))
}

/// The model members of one list-like graph node, in the authority's order.
fn sequence_members(node: NodeRef<'_>) -> Option<Vec<NodeRef<'_>>> {
    let items = node
        .as_sequence()
        .or_else(|| node.as_tuple())
        .or_else(|| node.as_set())?;
    Some(items.iter().map(|id| node.member(*id)).collect())
}

/// One nested model instance, or `None` when the value is not model-shaped.
fn model_object(
    model: &'static Model,
    node: NodeRef<'_>,
) -> Result<Option<ModelObject>, PayloadError> {
    if node.as_mapping().is_none() {
        return Ok(None);
    }
    let mut members = Vec::with_capacity(model.fields.len());
    for field in model.fields {
        let present = node.lookup(field.name).filter(|value| !value.is_null());
        let value = match present {
            Some(value) => match model_value(&field.spec, value)? {
                Some(value) => value,
                None => return Ok(None),
            },
            None => match default_value(&field.spec)? {
                Some(value) => value,
                // A required member with no model default: no model exists.
                None => return Ok(None),
            },
        };
        members.push((field.name, value));
    }
    Ok(Some(ModelObject {
        class: model.class,
        members,
    }))
}

/// One present member's value as its declared model type.
fn model_value(spec: &Spec, value: NodeRef<'_>) -> Result<Option<ModelValue>, PayloadError> {
    Ok(match spec {
        Spec::Text | Spec::TextDefault(_) | Spec::OptText => {
            value.as_lax_str().map(ModelValue::Text)
        }
        Spec::Enum(class) | Spec::EnumDefault(class, _) => enum_member(class, value),
        Spec::BooleanDefault(_) => value.as_bool_lax().map(ModelValue::Bool),
        Spec::Kind => enum_member(&super::ARTIFACT_KIND, value),
        Spec::TextList | Spec::RequiredTextList | Spec::OptTextList | Spec::TextListDefault(_) => {
            text_list(value)
        }
        Spec::Models(_) | Spec::RequiredModels(_) => model_list_value(spec, value)?,
        Spec::OptModel(model) => model_object(model, value)?.map(ModelValue::Object),
        Spec::Float | Spec::Map | Spec::OptAny | Spec::SkillInvocation => {
            return Err(unsupported_member(spec));
        }
    })
}

/// The model default materialised for an absent/null member.
fn default_value(spec: &Spec) -> Result<Option<ModelValue>, PayloadError> {
    Ok(match spec {
        Spec::TextDefault(default) => Some(ModelValue::Text(PythonString::from_scalars(default))),
        Spec::EnumDefault(class, member) => Some(ModelValue::Enum {
            class: class.class,
            member: member.name,
            value: member.value,
        }),
        Spec::BooleanDefault(flag) => Some(ModelValue::Bool(*flag)),
        Spec::TextList | Spec::Models(_) => Some(ModelValue::List(Vec::new())),
        Spec::TextListDefault(items) => Some(ModelValue::List(
            items
                .iter()
                .map(|item| ModelValue::Text(PythonString::from_scalars(item)))
                .collect(),
        )),
        Spec::OptText | Spec::OptTextList | Spec::OptModel(_) | Spec::OptAny => {
            Some(ModelValue::None)
        }
        Spec::Text
        | Spec::Float
        | Spec::Kind
        | Spec::Enum(_)
        | Spec::RequiredTextList
        | Spec::RequiredModels(_) => None,
        Spec::Map | Spec::SkillInvocation => return Err(unsupported_member(spec)),
    })
}

/// A `list[str]` member, or `None` when a member is not text.
fn text_list(value: NodeRef<'_>) -> Option<ModelValue> {
    let items = sequence_members(value)?;
    let mut members = Vec::with_capacity(items.len());
    for item in items {
        members.push(ModelValue::Text(item.as_lax_str()?));
    }
    Some(ModelValue::List(members))
}

/// A `list[Model]` member, or `None` when the list is not model-shaped.
fn model_list_value(spec: &Spec, value: NodeRef<'_>) -> Result<Option<ModelValue>, PayloadError> {
    let (Spec::Models(model) | Spec::RequiredModels(model)) = spec else {
        return Ok(None);
    };
    let Some(items) = sequence_members(value) else {
        return Ok(None);
    };
    let mut members = Vec::with_capacity(items.len());
    for item in items {
        match model_object(model, item)? {
            Some(object) => members.push(ModelValue::Object(object)),
            None => return Ok(None),
        }
    }
    Ok(Some(ModelValue::List(members)))
}

/// A `StrEnum` member of `class`, or `None` when the value is not one of its
/// declared members (the reference's model validation refuses it).
fn enum_member(class: &'static EnumClass, value: NodeRef<'_>) -> Option<ModelValue> {
    let text = value.as_lax_str()?;
    let member: &'static EnumMember = class.member_for_text(&text)?;
    Some(ModelValue::Enum {
        class: class.class,
        member: member.name,
        value: member.value,
    })
}

/// The typed-member capability boundary, naming the member class.
fn unsupported_member(spec: &Spec) -> PayloadError {
    let detail = match spec {
        Spec::Float => "a typed `float` member",
        Spec::Map => "a structured mapping member",
        Spec::OptAny => "an unstructured `Any` member",
        Spec::SkillInvocation => "the `SkillInvocation` member",
        _ => "this member",
    };
    PayloadError::Failed(PayloadFailure::UnsupportedMember { detail })
}

/// Python's `True`/`False` spelling.
fn bool_text(flag: bool) -> &'static str {
    if flag { "True" } else { "False" }
}

/// The repr of a `StrEnum` member's canonical value (`'tactic'`).
fn enum_value_repr(value: &'static str) -> PythonString {
    PythonString::from_scalars(&PythonString::from_scalars(value).repr())
}

/// `open + separator-joined members + close` (Python bracket/call spelling).
fn join_members(
    open: &str,
    members: impl Iterator<Item = PythonString>,
    close: &str,
    separator: &str,
) -> PythonString {
    let mut text = PythonString::from_scalars(open);
    for (position, member) in members.enumerate() {
        if position > 0 {
            text.push_scalars(separator);
        }
        text.append(&member);
    }
    text.push_scalars(close);
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::exact_payload_value;
    use crate::offering::value::ArtifactDocument;
    use sk_kernel::python_json::PythonJsonValue;

    /// One canonical-graph document read through the crate's lossless transport.
    struct Fixture {
        directory: tempfile::TempDir,
        document: ArtifactDocument,
    }

    impl Fixture {
        fn read(source: &str) -> Self {
            let directory = tempfile::tempdir().expect("fixture directory");
            let path = directory.path().join("artifact.yaml");
            std::fs::write(&path, source).expect("fixture write");
            let document = ArtifactDocument::read(&path).expect("artifact read");
            Self {
                directory,
                document,
            }
        }

        fn root(&self) -> NodeRef<'_> {
            self.document.root()
        }
    }

    const TACTIC_STEPS: &str = "id: probe-tactic\nschema_version: '1.0'\nname: Probe Tactic\nsteps:\n  - title: ''\n  - title: ''\n    description: ''\n  - title: T\n    examples: [one, two]\n    references:\n      - name: Architectural Integrity\n        type: directive\n        id: DIRECTIVE_001\n        when: always\n";

    fn step_strs(value: &Fixture) -> Vec<(String, String)> {
        model_list(ArtifactKind::Tactic, value.root(), "steps")
            .expect("typed tactic schema")
            .expect("model-shaped steps")
            .iter()
            .map(|step| {
                (
                    step.python_str().to_scalar_string().expect("scalar"),
                    step.python_repr().to_scalar_string().expect("scalar"),
                )
            })
            .collect()
    }

    #[test]
    fn defaults_and_model_order_come_from_the_one_schema() {
        let value = Fixture::read(TACTIC_STEPS);
        let steps = model_list(ArtifactKind::Tactic, value.root(), "steps")
            .expect("schema")
            .expect("model");
        assert_eq!(
            steps[0]
                .members()
                .iter()
                .map(|(name, _)| *name)
                .collect::<Vec<_>>(),
            vec!["title", "description", "examples", "references"]
        );
        assert_eq!(steps[0].class(), "TacticStep");
    }

    #[test]
    fn model_str_and_repr_are_the_pinned_source_forms() {
        let value = Fixture::read(TACTIC_STEPS);
        assert_eq!(
            step_strs(&value),
            vec![
                (
                    "title='' description=None examples=[] references=[]".to_owned(),
                    "TacticStep(title='', description=None, examples=[], references=[])"
                        .to_owned(),
                ),
                (
                    "title='' description='' examples=[] references=[]".to_owned(),
                    "TacticStep(title='', description='', examples=[], references=[])"
                        .to_owned(),
                ),
                (
                    "title='T' description=None examples=['one', 'two'] references=[TacticReference(name='Architectural Integrity', type=<ArtifactKind.DIRECTIVE: 'directive'>, id='DIRECTIVE_001', when='always')]".to_owned(),
                    "TacticStep(title='T', description=None, examples=['one', 'two'], references=[TacticReference(name='Architectural Integrity', type=<ArtifactKind.DIRECTIVE: 'directive'>, id='DIRECTIVE_001', when='always')])".to_owned(),
                ),
            ]
        );
    }

    #[test]
    fn procedure_step_defaults_keep_the_enum_identity() {
        let value = Fixture::read(
            "schema_version: '1.0'\nid: probe-procedure\nname: Probe Procedure\npurpose: p\nentry_condition: e\nexit_condition: x\nsteps:\n  - title: p1\n  - title: p2\n    description: d2\n    actor: human\n    on_failure: retry\n",
        );
        let steps = model_list(ArtifactKind::Procedure, value.root(), "steps")
            .expect("schema")
            .expect("model");
        assert_eq!(
            steps[0].python_str().to_scalar_string().unwrap(),
            "title='p1' description=None actor=<ActorRole.AGENT: 'agent'> on_success=None on_failure=None"
        );
        assert_eq!(
            steps[1].python_repr().to_scalar_string().unwrap(),
            "ProcedureStep(title='p2', description='d2', actor=<ActorRole.HUMAN: 'human'>, on_success=None, on_failure='retry')"
        );
        // The exact JSON keeps the member value, never the enum repr.
        let payload = exact_payload_value(
            ArtifactKind::Procedure,
            &serde_yaml_ng::from_str(
                "schema_version: '1.0'\nid: p\nname: P\npurpose: p\nentry_condition: e\nexit_condition: x\nsteps:\n  - title: p1\n",
            )
            .expect("bare fixture"),
        )
        .expect("exact payload");
        let steps = payload.field("steps").expect("steps");
        let PythonJsonValue::Array(items) = steps else {
            panic!("steps is an array");
        };
        assert_eq!(
            items[0].field("actor").and_then(PythonJsonValue::to_serde),
            Some(serde_json::json!("agent"))
        );
    }

    #[test]
    fn artifact_kind_member_identity_is_pinned_to_the_kind_roster() {
        // Every member of the ONE `ArtifactKind` roster, in declaration order, as
        // the pinned oracle child spells its repr.
        let expected = [
            ("DIRECTIVE", "directive"),
            ("TACTIC", "tactic"),
            ("STYLEGUIDE", "styleguide"),
            ("TOOLGUIDE", "toolguide"),
            ("PARADIGM", "paradigm"),
            ("PROCEDURE", "procedure"),
            ("AGENT_PROFILE", "agent_profile"),
            ("MISSION_STEP_CONTRACT", "mission_step_contract"),
            ("TEMPLATE", "template"),
            ("ASSET", "asset"),
            ("GLOSSARY_PACK", "glossary_pack"),
            ("ANTI_PATTERN", "anti_pattern"),
            ("SKILL", "skill"),
        ];
        for (member, value) in expected {
            let document = Fixture::read(&format!(
                "id: probe-tactic\nschema_version: '1.0'\nname: Probe Tactic\nsteps:\n  - title: T\n    references:\n      - name: r\n        type: {value}\n        id: x\n        when: w\n"
            ));
            assert!(
                step_strs(&document)[0]
                    .0
                    .contains(&format!("type=<ArtifactKind.{member}: '{value}'>")),
                "{member}"
            );
        }
    }

    #[test]
    fn authored_text_keeps_the_kernel_python_repr() {
        // Source `repr()` of the same authored text: quotes, backslash, NUL,
        // astral, no-break space and the next-line control point
        // (`q'uote"back\slash\x00nul<astral>astral\xa0cs\x85nel`).
        let document = Fixture::read(
            "id: probe-tactic\nschema_version: '1.0'\nname: Probe Tactic\nsteps:\n  - title: \"q'uote\\\"back\\\\slash\\x00nul\\U0001F642astral\\u00A0cs\\x85nel\"\n",
        );
        assert_eq!(
            step_strs(&document)[0].0,
            "title='q\\'uote\"back\\\\slash\\x00nul\u{1f642}astral\\xa0cs\\x85nel' description=None examples=[] references=[]"
        );
    }

    #[test]
    fn a_required_member_without_a_default_is_not_model_shaped() {
        let value =
            Fixture::read("id: t\nschema_version: '1.0'\nname: T\nsteps:\n  - description: only\n");
        assert!(
            model_list(ArtifactKind::Tactic, value.root(), "steps")
                .expect("schema")
                .is_none()
        );
        // A non-mapping value is not a model either.
        let value =
            Fixture::read("id: t\nschema_version: '1.0'\nname: T\nsteps:\n  - just a string\n");
        assert!(
            model_list(ArtifactKind::Tactic, value.root(), "steps")
                .expect("schema")
                .is_none()
        );
    }

    #[test]
    fn an_unrepresentable_member_is_a_typed_capability_boundary() {
        let value = Fixture::read(
            "id: g\nprovenance: p\nterms:\n  - surface: s\n    definition: d\n    confidence: 0.5\n    status: active\n",
        );
        let error =
            model_list(ArtifactKind::GlossaryPack, value.root(), "terms").expect_err("unsupported");
        assert!(error.to_string().contains("typed `float` member"));
        // The exact dump of the same record is unaffected by the view boundary.
        let bare = serde_yaml_ng::from_str(
            "id: g\nprovenance: p\nterms:\n  - surface: s\n    definition: d\n    confidence: 0.5\n    status: active\n",
        )
        .expect("bare fixture");
        assert!(
            exact_payload_value(ArtifactKind::GlossaryPack, &bare)
                .expect("exact payload")
                .field("terms")
                .is_some()
        );
    }

    #[test]
    fn an_untyped_kind_has_no_model_view() {
        let value = Fixture::read("id: a\n");
        assert!(
            model_list(ArtifactKind::Asset, value.root(), "steps")
                .expect_err("untyped kind")
                .to_string()
                .contains("no model dump")
        );
        let _ = &value.directory;
    }
}
