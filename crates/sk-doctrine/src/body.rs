//! Canonical artifact-body dumps.
//!
//! Mirrors `src/charter/activation/context_renderers/artifact_bodies.py`'s
//! `_jsonable_artifact_value`: the reference renders a typed artifact body with
//! `model_dump(by_alias=True, mode="json", exclude_none=True)` over the real
//! offering Pydantic model. That dump is not the raw YAML: every field with a
//! model default is present even when the author omitted it (an empty list or
//! mapping), optional fields set to `null` are absent, and nested models are
//! dumped recursively with their own defaults.
//!
//! The schemas below are the field/default surface of the pinned offering
//! models (`charter/offering/{directives,tactics,styleguides,toolguides,
//! procedures,paradigms,glossary_packs,pack_skills}/models.py` and
//! `charter/offering/missions/step_contracts.py`). They are presentation
//! metadata, not a second domain model: nothing here validates or stores
//! artifacts, and the YAML file remains the single authored authority.
//! Whole-document admission is the crate's existing owner
//! ([`crate::offering::model`]); a dump is only the projection of a document
//! that owner admits (the reference dumps a repository-validated model
//! instance, never raw authored fields).
//!
//! # One schema, one walker
//!
//! CF38 moved the walk/coercion **implementation** into [`exact`], which emits
//! the exact Kernel [`sk_kernel::python_json::PythonJsonValue`] payload (typed
//! non-finite floats, exact code points, constructed `set`/`pairs`/`omap`
//! order). The `Field`/`Spec` metadata below is still the ONE schema surface
//! both entry points read, and [`canonical_artifact_body`] is now explicitly the
//! **finite scalar adapter**: it projects that exact payload into
//! `serde_json::Value`, and therefore refuses (`None`) a payload `serde_json`
//! cannot carry (a non-finite float, a lone surrogate) instead of silently
//! dropping a member; arbitrary-width integers survive because the workspace
//! enables `serde_json`'s `arbitrary_precision`. Consumers that need
//! the full domain must call the exact API.
//!
//! # Model values, not authored tokens
//!
//! Where the reference model *coerces* a value the dump carries the coerced
//! value: a lax `float`/`bool` spelling becomes a JSON number/boolean (through
//! the ONE text authority in [`crate::offering::model`]), an `ArtifactKind`
//! field becomes its canonical spelling, and `SkillInvocation`'s `mode="before"`
//! rule (a declared `side_effects` list forces `model_invocable` off) is
//! applied. A *required* field with no model default is not materialised when
//! absent: a required field has no default to dump, so an unadmitted document
//! cannot be made to look model-shaped here.
//!
//! # The model view beside the dump
//!
//! The reference's body formatters also read the **model object**, not only its
//! JSON dump: `format_inline_named_body` falls back to `str(step)` and
//! `_append_step_lines` interpolates `getattr(step, "title", str(step))`. The
//! [`model_view`] leaf renders that same typed model — declared property order,
//! model defaults, `str`/`repr` spelling, `StrEnum` class/member identity and
//! nested-model `repr` — from the same [`Field`]/[`Spec`] surface, so the two
//! projections can never disagree about a field, a default or a type.

use serde_yaml_ng::Value;
use sk_charter::ArtifactKind;

mod exact;
mod model_view;

pub(crate) use exact::selected_payload_fields;
pub use exact::{
    PayloadError, PayloadFailure, canonical_artifact_body_exact,
    canonical_artifact_body_exact_path, canonical_artifact_body_exact_value, exact_json_value,
    exact_payload, exact_payload_value, selected_text_field,
};
pub(crate) use model_view::{ModelObject, ModelValue, model_list};

/// One field of a reference model dump.
pub(crate) struct Field {
    /// The model field name (identical to its serialization alias in the five
    /// artifact models, which use `by_alias=True` for the dump).
    pub(crate) name: &'static str,
    /// How the field's value is canonicalized.
    pub(crate) spec: Spec,
}

/// One Source `StrEnum` a model member carries, in declaration order.
///
/// The JSON dump spells a member by its [`EnumMember::value`]; only the model
/// view needs the class/member identity `repr` interpolates. This is display
/// metadata on the ONE schema, never a second model or a second kind roster.
pub(crate) struct EnumClass {
    /// The Source enum class name (`ArtifactKind`, `ActorRole`, …).
    pub(crate) class: &'static str,
    /// The declared members, in the reference's declaration order.
    pub(crate) members: &'static [EnumMember],
}

impl EnumClass {
    /// Exact enum-value lookup shared by admission and typed property projection.
    pub(crate) fn member_for_text(
        &self,
        text: &sk_kernel::PythonString,
    ) -> Option<&'static EnumMember> {
        self.members.iter().find(|member| {
            text.codepoints()
                .iter()
                .copied()
                .eq(member.value.chars().map(u32::from))
        })
    }
}

/// One declared `StrEnum` member.
pub(crate) struct EnumMember {
    /// The member name (`TACTIC`, `LENIENT_ADHERENCE`, …).
    pub(crate) name: &'static str,
    /// The member's canonical value (`tactic`, `lenient-adherence`, …).
    pub(crate) value: &'static str,
}

/// One Source Pydantic model: its class name and declared field surface.
///
/// A nested model needs its class name for `repr` (`TacticReference(...)`); the
/// field surface is the same [`Field`] table the exact walker dumps, so a model
/// declares its members exactly once.
pub(crate) struct Model {
    /// The Source model class name.
    pub(crate) class: &'static str,
    /// The model's declared fields, in declaration order.
    pub(crate) fields: &'static [Field],
}

/// How one model field is canonicalized.
pub(crate) enum Spec {
    /// Required scalar; absent/null is omitted (an invalid artifact).
    Text,
    /// Required scalar with a model default (a `str` field that has one).
    TextDefault(&'static str),
    /// A `StrEnum` member; `mode="json"` dumps its authored value text, exactly
    /// like [`Spec::Text`], while the model view keeps the class identity.
    Enum(&'static EnumClass),
    /// A `StrEnum` member with a model default member (`ProcedureStep.actor`).
    EnumDefault(&'static EnumClass, &'static EnumMember),
    /// Optional scalar; `null` is dropped by `exclude_none`.
    OptText,
    /// Required lax `float`; dumped as the coerced JSON number.
    Float,
    /// `bool` with a model default; dumped as the coerced JSON boolean.
    BooleanDefault(bool),
    /// An `ArtifactKind` field; dumped as its canonical value (`mode="json"`).
    Kind,
    /// `list[str]` with a `default_factory=list` default.
    TextList,
    /// `list[str]` with a non-empty model default (`PackSkill.tools`).
    TextListDefault(&'static [&'static str]),
    /// `list[str] | None = None`; absent/null is dropped by `exclude_none`.
    OptTextList,
    /// Required `list[str]` (`Field(min_length=1)`, no default).
    RequiredTextList,
    /// `dict[..., ...]` with a `default_factory=dict` default.
    Map,
    /// Optional unstructured value; `null` dropped, nested nulls stripped.
    OptAny,
    /// `list[Model]` with a `default_factory=list` default.
    Models(&'static Model),
    /// Required `list[Model]` (`Field(min_length=1)`, no default).
    RequiredModels(&'static Model),
    /// `Model | None = None`; absent/null is dropped by `exclude_none`.
    OptModel(&'static Model),
    /// `SkillInvocation` (default factory + the forced `model_invocable` rule).
    SkillInvocation,
}

// ---------------------------------------------------------------------------
// Source `StrEnum` identities
//
// One declaration per enum the nine artifact models carry. `ArtifactKind`
// derives from the workspace ArtifactKind authority (including Source enum
// member identity); `ActorRole`, `Enforcement` and
// `StyleguideScope` are declared inline in their artifact models and have no
// other Native home. `LENIENT_ADHERENCE` shows why the member name is declared
// rather than derived from the value.
// ---------------------------------------------------------------------------

const ARTIFACT_KIND: EnumClass = EnumClass {
    class: "ArtifactKind",
    members: &artifact_kind_members(),
};

/// Derive display metadata from the one kind taxonomy, never a second roster.
const fn artifact_kind_members() -> [EnumMember; ArtifactKind::ALL.len()] {
    let mut members = [const {
        EnumMember {
            name: "",
            value: "",
        }
    }; ArtifactKind::ALL.len()];
    let mut index = 0;
    while index < ArtifactKind::ALL.len() {
        let kind = ArtifactKind::ALL[index];
        members[index] = EnumMember {
            name: kind.python_member_name(),
            value: kind.canonical(),
        };
        index += 1;
    }
    members
}

const ACTOR_ROLE_HUMAN: EnumMember = EnumMember {
    name: "HUMAN",
    value: "human",
};
const ACTOR_ROLE_AGENT: EnumMember = EnumMember {
    name: "AGENT",
    value: "agent",
};
const ACTOR_ROLE_SYSTEM: EnumMember = EnumMember {
    name: "SYSTEM",
    value: "system",
};
pub(crate) const ACTOR_ROLE: EnumClass = EnumClass {
    class: "ActorRole",
    members: &[ACTOR_ROLE_HUMAN, ACTOR_ROLE_AGENT, ACTOR_ROLE_SYSTEM],
};

pub(crate) const ENFORCEMENT: EnumClass = EnumClass {
    class: "Enforcement",
    members: &[
        EnumMember {
            name: "REQUIRED",
            value: "required",
        },
        EnumMember {
            name: "LENIENT_ADHERENCE",
            value: "lenient-adherence",
        },
        EnumMember {
            name: "ADVISORY",
            value: "advisory",
        },
    ],
};

pub(crate) const STYLEGUIDE_SCOPE: EnumClass = EnumClass {
    class: "StyleguideScope",
    members: &[
        EnumMember {
            name: "CODE",
            value: "code",
        },
        EnumMember {
            name: "DOCS",
            value: "docs",
        },
        EnumMember {
            name: "ARCHITECTURE",
            value: "architecture",
        },
        EnumMember {
            name: "TESTING",
            value: "testing",
        },
        EnumMember {
            name: "OPERATIONS",
            value: "operations",
        },
        EnumMember {
            name: "GLOSSARY",
            value: "glossary",
        },
    ],
};

const DIRECTIVE_REFERENCE_FIELDS: &[Field] = &[
    Field {
        name: "type",
        spec: Spec::Enum(&ARTIFACT_KIND),
    },
    Field {
        name: "id",
        spec: Spec::Text,
    },
    Field {
        name: "when",
        spec: Spec::OptText,
    },
    Field {
        name: "reason",
        spec: Spec::OptText,
    },
];

const DIRECTIVE: &[Field] = &[
    Field {
        name: "id",
        spec: Spec::Text,
    },
    Field {
        name: "schema_version",
        spec: Spec::Text,
    },
    Field {
        name: "title",
        spec: Spec::Text,
    },
    Field {
        name: "intent",
        spec: Spec::Text,
    },
    Field {
        name: "enforcement",
        spec: Spec::Enum(&ENFORCEMENT),
    },
    Field {
        name: "scope",
        spec: Spec::OptText,
    },
    Field {
        name: "procedures",
        spec: Spec::TextList,
    },
    Field {
        name: "integrity_rules",
        spec: Spec::TextList,
    },
    Field {
        name: "validation_criteria",
        spec: Spec::TextList,
    },
    Field {
        name: "explicit_allowances",
        spec: Spec::TextList,
    },
    Field {
        name: "references",
        spec: Spec::Models(&DIRECTIVE_REFERENCE),
    },
];

const TACTIC_REFERENCE_FIELDS: &[Field] = &[
    Field {
        name: "name",
        spec: Spec::Text,
    },
    Field {
        name: "type",
        spec: Spec::Enum(&ARTIFACT_KIND),
    },
    Field {
        name: "id",
        spec: Spec::Text,
    },
    Field {
        name: "when",
        spec: Spec::Text,
    },
];

const TACTIC_STEP_FIELDS: &[Field] = &[
    Field {
        name: "title",
        spec: Spec::Text,
    },
    Field {
        name: "description",
        spec: Spec::OptText,
    },
    Field {
        name: "examples",
        spec: Spec::TextList,
    },
    Field {
        name: "references",
        spec: Spec::Models(&TACTIC_REFERENCE),
    },
];

const TACTIC: &[Field] = &[
    Field {
        name: "id",
        spec: Spec::Text,
    },
    Field {
        name: "schema_version",
        spec: Spec::Text,
    },
    Field {
        name: "name",
        spec: Spec::Text,
    },
    Field {
        name: "purpose",
        spec: Spec::OptText,
    },
    Field {
        name: "steps",
        spec: Spec::RequiredModels(&TACTIC_STEP),
    },
    Field {
        name: "failure_modes",
        spec: Spec::TextList,
    },
    Field {
        name: "applies_to_languages",
        spec: Spec::TextList,
    },
    Field {
        name: "references",
        spec: Spec::Models(&TACTIC_REFERENCE),
    },
    Field {
        name: "notes",
        spec: Spec::OptText,
    },
];

const STYLEGUIDE_PATTERN_FIELDS: &[Field] = &[
    Field {
        name: "name",
        spec: Spec::Text,
    },
    Field {
        name: "description",
        spec: Spec::Text,
    },
    Field {
        name: "bad_example",
        spec: Spec::OptText,
    },
    Field {
        name: "good_example",
        spec: Spec::OptText,
    },
];

const STYLEGUIDE_ANTI_PATTERN_FIELDS: &[Field] = &[
    Field {
        name: "name",
        spec: Spec::Text,
    },
    Field {
        name: "description",
        spec: Spec::Text,
    },
    Field {
        name: "bad_example",
        spec: Spec::Text,
    },
    Field {
        name: "good_example",
        spec: Spec::Text,
    },
];

const STYLEGUIDE: &[Field] = &[
    Field {
        name: "id",
        spec: Spec::Text,
    },
    Field {
        name: "schema_version",
        spec: Spec::Text,
    },
    Field {
        name: "title",
        spec: Spec::Text,
    },
    Field {
        name: "scope",
        spec: Spec::Enum(&STYLEGUIDE_SCOPE),
    },
    Field {
        name: "principles",
        spec: Spec::TextList,
    },
    Field {
        name: "patterns",
        spec: Spec::Models(&STYLEGUIDE_PATTERN),
    },
    Field {
        name: "anti_patterns",
        spec: Spec::Models(&STYLEGUIDE_ANTI_PATTERN),
    },
    Field {
        name: "tooling",
        spec: Spec::Map,
    },
    Field {
        name: "quality_test",
        spec: Spec::OptText,
    },
    Field {
        name: "applies_to_languages",
        spec: Spec::TextList,
    },
    Field {
        name: "references",
        spec: Spec::TextList,
    },
    Field {
        name: "structural_lint_config",
        spec: Spec::OptAny,
    },
];

const TOOLGUIDE: &[Field] = &[
    Field {
        name: "id",
        spec: Spec::Text,
    },
    Field {
        name: "schema_version",
        spec: Spec::Text,
    },
    Field {
        name: "tool",
        spec: Spec::Text,
    },
    Field {
        name: "title",
        spec: Spec::Text,
    },
    Field {
        name: "guide_path",
        spec: Spec::Text,
    },
    Field {
        name: "summary",
        spec: Spec::Text,
    },
    Field {
        name: "commands",
        spec: Spec::TextList,
    },
    Field {
        name: "applies_to_languages",
        spec: Spec::TextList,
    },
    Field {
        name: "last_updated",
        spec: Spec::OptText,
    },
    Field {
        name: "references",
        spec: Spec::TextList,
    },
];

const PROCEDURE_REFERENCE_FIELDS: &[Field] = &[
    Field {
        name: "type",
        spec: Spec::Enum(&ARTIFACT_KIND),
    },
    Field {
        name: "id",
        spec: Spec::Text,
    },
    Field {
        name: "reason",
        spec: Spec::OptText,
    },
];

const PROCEDURE_STEP_FIELDS: &[Field] = &[
    Field {
        name: "title",
        spec: Spec::Text,
    },
    Field {
        name: "description",
        spec: Spec::OptText,
    },
    Field {
        name: "actor",
        spec: Spec::EnumDefault(&ACTOR_ROLE, &ACTOR_ROLE_AGENT),
    },
    Field {
        name: "on_success",
        spec: Spec::OptText,
    },
    Field {
        name: "on_failure",
        spec: Spec::OptText,
    },
];

const PROCEDURE_ANTI_PATTERN_FIELDS: &[Field] = &[
    Field {
        name: "name",
        spec: Spec::Text,
    },
    Field {
        name: "description",
        spec: Spec::Text,
    },
];

const PROCEDURE: &[Field] = &[
    Field {
        name: "schema_version",
        spec: Spec::Text,
    },
    Field {
        name: "id",
        spec: Spec::Text,
    },
    Field {
        name: "name",
        spec: Spec::Text,
    },
    Field {
        name: "purpose",
        spec: Spec::Text,
    },
    Field {
        name: "entry_condition",
        spec: Spec::Text,
    },
    Field {
        name: "exit_condition",
        spec: Spec::Text,
    },
    Field {
        name: "steps",
        spec: Spec::RequiredModels(&PROCEDURE_STEP),
    },
    Field {
        name: "anti_patterns",
        spec: Spec::Models(&PROCEDURE_ANTI_PATTERN),
    },
    Field {
        name: "applies_to_languages",
        spec: Spec::TextList,
    },
    Field {
        name: "notes",
        spec: Spec::OptText,
    },
    Field {
        name: "references",
        spec: Spec::Models(&PROCEDURE_REFERENCE),
    },
];

// ---------------------------------------------------------------------------
// Glossary pack (`charter/offering/glossary_packs/models.py`)
// ---------------------------------------------------------------------------

const GLOSSARY_TERM_FIELDS: &[Field] = &[
    Field {
        name: "surface",
        spec: Spec::Text,
    },
    Field {
        name: "definition",
        spec: Spec::Text,
    },
    Field {
        name: "confidence",
        spec: Spec::Float,
    },
    Field {
        name: "status",
        spec: Spec::Text,
    },
    // `list[str] | None = None`: unauthored optional lists stay absent.
    Field {
        name: "see_also",
        spec: Spec::OptTextList,
    },
    Field {
        name: "introduced_in_mission",
        spec: Spec::OptText,
    },
    Field {
        name: "synonyms_to_avoid",
        spec: Spec::OptTextList,
    },
    Field {
        name: "aliases",
        spec: Spec::OptTextList,
    },
    Field {
        name: "banned_synonyms",
        spec: Spec::OptTextList,
    },
];

const GLOSSARY_PACK: &[Field] = &[
    Field {
        name: "id",
        spec: Spec::Text,
    },
    Field {
        name: "provenance",
        spec: Spec::Text,
    },
    // `terms: list[GlossaryTerm] = Field(min_length=1)` — required, no default.
    Field {
        name: "terms",
        spec: Spec::RequiredModels(&GLOSSARY_TERM),
    },
    Field {
        name: "description",
        spec: Spec::OptText,
    },
];

// ---------------------------------------------------------------------------
// Paradigm (`charter/offering/paradigms/models.py`)
// ---------------------------------------------------------------------------

const PARADIGM_REFERENCE_FIELDS: &[Field] = &[
    Field {
        name: "type",
        spec: Spec::Kind,
    },
    Field {
        name: "id",
        spec: Spec::Text,
    },
];

const PARADIGM: &[Field] = &[
    Field {
        name: "schema_version",
        spec: Spec::Text,
    },
    Field {
        name: "id",
        spec: Spec::Text,
    },
    Field {
        name: "name",
        spec: Spec::Text,
    },
    Field {
        name: "summary",
        spec: Spec::Text,
    },
    Field {
        name: "directive_refs",
        spec: Spec::TextList,
    },
    Field {
        name: "references",
        spec: Spec::Models(&PARADIGM_REFERENCE),
    },
];

// ---------------------------------------------------------------------------
// Pack skill (`charter/offering/pack_skills/models.py`)
// ---------------------------------------------------------------------------

const SKILL_PARAMETER_FIELDS: &[Field] = &[
    Field {
        name: "name",
        spec: Spec::Text,
    },
    Field {
        name: "required",
        spec: Spec::BooleanDefault(false),
    },
    Field {
        name: "description",
        spec: Spec::TextDefault(""),
    },
    Field {
        name: "default",
        spec: Spec::OptText,
    },
];

const SKILL_EXPANSION_FIELDS: &[Field] = &[
    Field {
        name: "target",
        spec: Spec::Text,
    },
    Field {
        name: "args",
        spec: Spec::TextDefault(""),
    },
];

const PACK_SKILL: &[Field] = &[
    Field {
        name: "schema_version",
        spec: Spec::Text,
    },
    Field {
        name: "id",
        spec: Spec::Text,
    },
    Field {
        name: "title",
        spec: Spec::Text,
    },
    Field {
        name: "description",
        spec: Spec::Text,
    },
    Field {
        name: "triggers",
        spec: Spec::TextList,
    },
    Field {
        name: "form",
        spec: Spec::Text,
    },
    Field {
        name: "body_path",
        spec: Spec::OptText,
    },
    Field {
        name: "expands_to",
        spec: Spec::OptModel(&SKILL_EXPANSION),
    },
    Field {
        name: "parameters",
        spec: Spec::Models(&SKILL_PARAMETER),
    },
    // `invocation: SkillInvocation = Field(default_factory=SkillInvocation)`.
    Field {
        name: "invocation",
        spec: Spec::SkillInvocation,
    },
    // `tools: list[str] = Field(default_factory=lambda: ["*"])`.
    Field {
        name: "tools",
        spec: Spec::TextListDefault(&["*"]),
    },
    Field {
        name: "version",
        spec: Spec::TextDefault("1.0.0"),
    },
    Field {
        name: "maintainers",
        spec: Spec::TextList,
    },
    Field {
        name: "overrides",
        spec: Spec::OptText,
    },
    Field {
        name: "enhances",
        spec: Spec::OptText,
    },
];

// ---------------------------------------------------------------------------
// Mission step contract (`charter/offering/missions/step_contracts.py`)
// ---------------------------------------------------------------------------

const MISSION_STEP_CONTRACT_INPUT_FIELDS: &[Field] = &[
    Field {
        name: "flag",
        spec: Spec::Text,
    },
    Field {
        name: "source",
        spec: Spec::Text,
    },
    Field {
        name: "optional",
        spec: Spec::BooleanDefault(false),
    },
];

const DELEGATES_TO_FIELDS: &[Field] = &[
    Field {
        name: "kind",
        spec: Spec::Kind,
    },
    // `candidates: list[str] = Field(min_length=1)` — required, no default.
    Field {
        name: "candidates",
        spec: Spec::RequiredTextList,
    },
];

const MISSION_STEP_CONTRACT_STEP_FIELDS: &[Field] = &[
    Field {
        name: "id",
        spec: Spec::Text,
    },
    Field {
        name: "description",
        spec: Spec::Text,
    },
    Field {
        name: "command",
        spec: Spec::OptText,
    },
    Field {
        name: "inputs",
        spec: Spec::Models(&MISSION_STEP_CONTRACT_INPUT),
    },
    Field {
        name: "delegates_to",
        spec: Spec::OptModel(&DELEGATES_TO),
    },
    Field {
        name: "guidance",
        spec: Spec::OptText,
    },
];

const GATE_BINDING_FIELDS: &[Field] = &[
    Field {
        name: "on_transition",
        spec: Spec::Text,
    },
    Field {
        name: "handler",
        spec: Spec::Text,
    },
    Field {
        name: "handler_kind",
        spec: Spec::TextDefault("mission_step_contract"),
    },
    Field {
        name: "schema_version",
        spec: Spec::Text,
    },
    Field {
        name: "fail_open",
        spec: Spec::BooleanDefault(true),
    },
    Field {
        name: "provenance",
        spec: Spec::OptText,
    },
];

const MISSION_STEP_CONTRACT: &[Field] = &[
    Field {
        name: "id",
        spec: Spec::Text,
    },
    Field {
        name: "schema_version",
        spec: Spec::Text,
    },
    Field {
        name: "action",
        spec: Spec::Text,
    },
    Field {
        name: "mission",
        spec: Spec::Text,
    },
    Field {
        name: "steps",
        spec: Spec::RequiredModels(&MISSION_STEP_CONTRACT_STEP),
    },
    Field {
        name: "gates",
        spec: Spec::Models(&GATE_BINDING),
    },
];

// ---------------------------------------------------------------------------
// Source model identities
//
// The class name `repr` interpolates, beside the ONE field table above. A
// nested model is referenced through its [`Model`], so a class name and its
// field surface have exactly one declaration each.
// ---------------------------------------------------------------------------

const DIRECTIVE_REFERENCE: Model = Model {
    class: "DirectiveReference",
    fields: DIRECTIVE_REFERENCE_FIELDS,
};
const TACTIC_REFERENCE: Model = Model {
    class: "TacticReference",
    fields: TACTIC_REFERENCE_FIELDS,
};
const TACTIC_STEP: Model = Model {
    class: "TacticStep",
    fields: TACTIC_STEP_FIELDS,
};
const STYLEGUIDE_PATTERN: Model = Model {
    class: "Pattern",
    fields: STYLEGUIDE_PATTERN_FIELDS,
};
const STYLEGUIDE_ANTI_PATTERN: Model = Model {
    class: "AntiPattern",
    fields: STYLEGUIDE_ANTI_PATTERN_FIELDS,
};
const PROCEDURE_REFERENCE: Model = Model {
    class: "ProcedureReference",
    fields: PROCEDURE_REFERENCE_FIELDS,
};
const PROCEDURE_STEP: Model = Model {
    class: "ProcedureStep",
    fields: PROCEDURE_STEP_FIELDS,
};
const PROCEDURE_ANTI_PATTERN: Model = Model {
    class: "ProcedureAntiPattern",
    fields: PROCEDURE_ANTI_PATTERN_FIELDS,
};
const GLOSSARY_TERM: Model = Model {
    class: "GlossaryTerm",
    fields: GLOSSARY_TERM_FIELDS,
};
const PARADIGM_REFERENCE: Model = Model {
    class: "ParadigmReference",
    fields: PARADIGM_REFERENCE_FIELDS,
};
const SKILL_PARAMETER: Model = Model {
    class: "SkillParameter",
    fields: SKILL_PARAMETER_FIELDS,
};
const SKILL_EXPANSION: Model = Model {
    class: "SkillExpansion",
    fields: SKILL_EXPANSION_FIELDS,
};
const MISSION_STEP_CONTRACT_INPUT: Model = Model {
    class: "MissionStepInput",
    fields: MISSION_STEP_CONTRACT_INPUT_FIELDS,
};
const DELEGATES_TO: Model = Model {
    class: "DelegatesTo",
    fields: DELEGATES_TO_FIELDS,
};
const MISSION_STEP_CONTRACT_STEP: Model = Model {
    class: "MissionStepContractStep",
    fields: MISSION_STEP_CONTRACT_STEP_FIELDS,
};
const GATE_BINDING: Model = Model {
    class: "GateBinding",
    fields: GATE_BINDING_FIELDS,
};

/// The model-dump field surface for `kind`, or `None` for an untyped kind.
pub(crate) fn schema(kind: ArtifactKind) -> Option<&'static [Field]> {
    match kind {
        ArtifactKind::Directive => Some(DIRECTIVE),
        ArtifactKind::Tactic => Some(TACTIC),
        ArtifactKind::Styleguide => Some(STYLEGUIDE),
        ArtifactKind::Toolguide => Some(TOOLGUIDE),
        ArtifactKind::Procedure => Some(PROCEDURE),
        ArtifactKind::GlossaryPack => Some(GLOSSARY_PACK),
        ArtifactKind::Paradigm => Some(PARADIGM),
        ArtifactKind::Skill => Some(PACK_SKILL),
        ArtifactKind::MissionStepContract => Some(MISSION_STEP_CONTRACT),
        _ => None,
    }
}

/// The reference model dump of one parsed artifact document (finite adapter).
///
/// This is the **finite scalar capability view** of the one exact walker in
/// [`exact`]: it dumps the exact payload and then projects it into
/// `serde_json::Value`. Its signature and every finite result are unchanged, so
/// the CF32 include arms that consume it keep working while the parent re-wires
/// them onto [`exact::canonical_artifact_body_exact`].
///
/// A payload `serde_json` cannot carry refuses (`None`) instead of dropping a
/// member: a typed non-finite `float` (`NaN`/`Infinity`/`-Infinity`) or a lone
/// surrogate code point. Arbitrary-width integers survive the projection because
/// the workspace enables `serde_json`'s `arbitrary_precision`.
/// That is a documented limitation of *this adapter*, not of the exact payload —
/// the reference prints `NaN`/`Infinity` and escapes the surrogate.
///
/// Returns `None` when the document is not a mapping or the kind has no typed
/// model dump; a caller then omits `body`, matching `_jsonable_artifact_value`'s
/// `if body:` guard.
#[must_use]
pub fn canonical_artifact_body(kind: ArtifactKind, value: &Value) -> Option<serde_json::Value> {
    exact::canonical_artifact_body_exact_value(kind, value)?.to_serde()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Value {
        serde_yaml_ng::from_str(text).unwrap()
    }

    #[test]
    fn directive_body_materialises_empty_defaults_and_drops_nulls() {
        let body = canonical_artifact_body(
            ArtifactKind::Directive,
            &parse("id: DIRECTIVE_001\nschema_version: '1.0'\ntitle: T\nintent: I\nenforcement: required\nscope: null\n"),
        )
        .unwrap();
        assert_eq!(body["procedures"], serde_json::json!([]));
        assert_eq!(body["integrity_rules"], serde_json::json!([]));
        assert_eq!(body["validation_criteria"], serde_json::json!([]));
        assert_eq!(body["explicit_allowances"], serde_json::json!([]));
        assert_eq!(body["references"], serde_json::json!([]));
        assert!(body.get("scope").is_none());
    }

    #[test]
    fn tactic_steps_and_references_are_dumped_as_nested_models() {
        let body = canonical_artifact_body(
            ArtifactKind::Tactic,
            &parse(
                "id: t\nschema_version: '1.0'\nname: T\nsteps:\n  - title: one\n  - title: two\n    examples: [x]\n    references:\n      - name: r\n        type: directive\n        id: DIRECTIVE_001\n        when: never\n",
            ),
        )
        .unwrap();
        assert_eq!(body["failure_modes"], serde_json::json!([]));
        assert_eq!(body["applies_to_languages"], serde_json::json!([]));
        assert_eq!(body["references"], serde_json::json!([]));
        assert_eq!(body["steps"][0]["examples"], serde_json::json!([]));
        assert_eq!(body["steps"][0]["references"], serde_json::json!([]));
        assert_eq!(
            body["steps"][1]["references"],
            serde_json::json!([{
                "name": "r",
                "type": "directive",
                "id": "DIRECTIVE_001",
                "when": "never"
            }])
        );
        assert!(body.get("purpose").is_none());
        assert!(body.get("notes").is_none());
    }

    #[test]
    fn procedure_step_actor_defaults_to_agent() {
        let body = canonical_artifact_body(
            ArtifactKind::Procedure,
            &parse(
                "schema_version: '1.0'\nid: p\nname: P\npurpose: p\nentry_condition: e\nexit_condition: x\nsteps:\n  - title: one\n",
            ),
        )
        .unwrap();
        assert_eq!(body["steps"][0]["actor"], "agent");
        assert_eq!(body["anti_patterns"], serde_json::json!([]));
        assert_eq!(body["applies_to_languages"], serde_json::json!([]));
        assert_eq!(body["references"], serde_json::json!([]));
    }

    #[test]
    fn untyped_kinds_have_no_model_dump() {
        assert!(canonical_artifact_body(ArtifactKind::Asset, &parse("id: a")).is_none());
    }
}
