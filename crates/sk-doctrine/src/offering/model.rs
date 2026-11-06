//! The reference's Pydantic models, as admission over constructed YAML.
//!
//! `BaseArtifactRepository._load` does not check a few top-level field shapes: it
//! calls `self._schema.model_validate(data)` on the **whole** model, and Pydantic
//! validates every nested sub-model, every `extra="forbid"` key set, every
//! pattern/`min_length` constraint and every `model_validator`. A document that
//! fails is *skipped* with a warning; the rest of the layer still loads.
//!
//! This module is that validation, with one typed function per kind and per
//! nested model, over the constructed value tree ([`ArtifactDocument`]) rather
//! than over `serde`'s raw view. Coercions follow Pydantic v2's **lax** mode,
//! which is what `model_validate` uses by default:
//!
//! * `str` accepts `str` and UTF-8 `bytes` (the reference's `!!binary` case);
//! * `int` accepts `int`, `bool`, an integral `float` and a numeric string;
//! * `float` accepts `int`, `float`, `bool` and a numeric string;
//! * `bool` accepts `bool`, `0`/`1`, `0.0`/`1.0` and the lax spellings
//!   `true/false/yes/no/on/off/t/f/y/n/1/0`;
//! * an `ArtifactKind` field accepts the canonical singular value only;
//! * a `Literal[...]` field accepts exactly its members;
//! * a list field accepts any sequence (a constructed `!!set` included — the
//!   reference's `set` iterates into a `list[str]`).
//!
//! The authored node itself is never rewritten: only admission is decided here.

use serde_yaml_ng::{Mapping, Value};
use sk_charter::ArtifactKind;
use sk_kernel::PythonString;

use super::value::ArtifactDocument;

/// The mission-type path-convention keys (`VALID_PATH_KEYS`, C-005).
pub(crate) const VALID_PATH_KEYS: [&str; 5] = [
    "workspace",
    "tests",
    "deliverables",
    "documentation",
    "data",
];

/// The reference pattern `^[a-z][a-z0-9-]*$`.
fn lower_kebab(text: &PythonString) -> bool {
    let Ok(text) = text.to_scalar_string() else {
        return false;
    };
    let mut characters = text.chars();
    match characters.next() {
        Some(first) if first.is_ascii_lowercase() => {}
        _ => return false,
    }
    characters.all(|character| {
        character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
    })
}

/// The model's `skill` id grammar: `[a-z][a-z0-9]*(?:-[a-z0-9]+)*`, at most 64
/// characters (`SKILL_ID_PATTERN`/`SKILL_ID_MAX_LENGTH`).
fn skill_id(text: &PythonString) -> bool {
    const MAX_LENGTH: usize = 64;
    let Ok(text) = text.to_scalar_string() else {
        return false;
    };
    if text.chars().count() > MAX_LENGTH {
        return false;
    }
    let mut characters = text.chars();
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

/// The directive model's `id` pattern `^[A-Z][A-Z0-9_-]*$`.
fn directive_id(text: &PythonString) -> bool {
    let Ok(text) = text.to_scalar_string() else {
        return false;
    };
    let mut characters = text.chars();
    match characters.next() {
        Some(first) if first.is_ascii_uppercase() => {}
        _ => return false,
    }
    characters.all(|character| {
        character.is_ascii_uppercase()
            || character.is_ascii_digit()
            || character == '_'
            || character == '-'
    })
}

/// The reference pattern `^[A-Za-z][A-Za-z0-9_-]*$` (`TacticReference.id`).
fn reference_id(text: &PythonString) -> bool {
    let Ok(text) = text.to_scalar_string() else {
        return false;
    };
    let mut characters = text.chars();
    match characters.next() {
        Some(first) if first.is_ascii_alphabetic() => {}
        _ => return false,
    }
    characters
        .all(|character| character.is_ascii_alphanumeric() || character == '_' || character == '-')
}

/// The shell metacharacters a `cli:` expansion target/args may not contain
/// (`_SHELL_METACHARACTERS`).
fn is_shell_metacharacter(character: char) -> bool {
    matches!(
        character,
        ';' | '&' | '|' | '<' | '>' | '$' | '`' | '\n' | '\r'
    )
}

/// The first word of a POSIX-quoted argv text, or `None` when the quoting is
/// unbalanced (CPython `shlex.split` raises `ValueError` there).
///
/// Admission only needs validity and the program word; it does not build the
/// argv. Metacharacter-tainted text is refused before this runs. The scan runs
/// to the end even after the first word, because an unbalanced quote *after* it
/// still makes `shlex.split` fail (so `cli:spec-kitty "unclosed` is refused).
fn cli_argv_first_word(text: &str) -> Option<String> {
    let mut first: Option<String> = None;
    let mut word = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        match quote {
            Some(open) => {
                if character == open {
                    quote = None;
                } else if character == '\\' && open == '"' {
                    word.push(characters.next()?);
                } else {
                    word.push(character);
                }
            }
            None => match character {
                '\'' | '"' => {
                    quote = Some(character);
                    in_word = true;
                }
                '\\' => {
                    word.push(characters.next()?);
                    in_word = true;
                }
                character if character.is_whitespace() => {
                    if in_word && first.is_none() {
                        first = Some(std::mem::take(&mut word));
                    }
                    in_word = false;
                }
                character => {
                    word.push(character);
                    in_word = true;
                }
            },
        }
    }
    if quote.is_some() {
        return None;
    }
    if in_word && first.is_none() {
        first = Some(word);
    }
    first
}

/// The reference pattern `^[^/].*\.md$` (`Toolguide.guide_path`).
fn guide_path(text: &PythonString) -> bool {
    let Ok(text) = text.to_scalar_string() else {
        return false;
    };
    text.len() >= 4 && !text.starts_with('/') && text.ends_with(".md")
}

/// The model's `id`, when the document is admitted.
///
/// The value is the *model's* `id` (the Pydantic `str` coercion of the authored
/// field) — the key `BaseArtifactRepository._load` files a built-in item under.
pub(crate) fn model_id(kind: ArtifactKind, document: &ArtifactDocument) -> Option<PythonString> {
    admits_mapping(kind, document, &document.value)
}

/// Pydantic's lax `bool` spellings for a text value.
///
/// The ONE text authority for the lax coercions: document admission
/// ([`View::boolean`]) and the canonical model dump
/// ([`crate::body`]) must agree on which spellings a model accepts, so the
/// spelling table lives here once instead of in each caller.
pub(crate) fn lax_bool_text(text: &str) -> Option<bool> {
    match text.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "t" | "yes" | "y" | "on" => Some(true),
        "0" | "false" | "f" | "no" | "n" | "off" => Some(false),
        _ => None,
    }
}

/// Pydantic's lax `int` spellings for a text value (underscores allowed).
pub(crate) fn lax_int_text(text: &str) -> Option<i64> {
    let cleaned: String = text.trim().replace('_', "");
    cleaned.parse().ok()
}

/// Pydantic's lax `float` spellings for a text value (underscores, `inf`/`nan`).
pub(crate) fn lax_float_text(text: &str) -> Option<f64> {
    let cleaned: String = text.trim().replace('_', "");
    match cleaned.to_ascii_lowercase().as_str() {
        "nan" | "+nan" | "-nan" => Some(f64::NAN),
        "inf" | "+inf" | "infinity" | "+infinity" => Some(f64::INFINITY),
        "-inf" | "-infinity" => Some(f64::NEG_INFINITY),
        _ => cleaned.parse().ok(),
    }
}

/// The model `id` when `value` is admitted as `kind`'s document.
///
/// The overlay path validates the *merged* mapping (the reference's
/// `_merge` → `model_validate`), so it cannot simply reuse the authored node.
pub(crate) fn admits_mapping(
    kind: ArtifactKind,
    document: &ArtifactDocument,
    value: &Value,
) -> Option<PythonString> {
    let mapping = value.as_mapping()?;
    let view = View {
        document: Some(document),
    };
    let field = if kind == ArtifactKind::AgentProfile {
        "profile-id"
    } else {
        "id"
    };
    let id = view.string(mapping.get(field)?)?;
    let admitted = match kind {
        ArtifactKind::Directive => view.directive(mapping, &id),
        ArtifactKind::Tactic => view.tactic(mapping, &id),
        ArtifactKind::Styleguide => view.styleguide(mapping, &id),
        ArtifactKind::Toolguide => view.toolguide(mapping, &id),
        ArtifactKind::Paradigm => view.paradigm(mapping, &id),
        ArtifactKind::Procedure => view.procedure(mapping, &id),
        ArtifactKind::GlossaryPack => view.glossary_pack(mapping),
        ArtifactKind::MissionStepContract => view.mission_step_contract(mapping),
        ArtifactKind::Skill => view.pack_skill(mapping, &id),
        _ => false,
    };
    admitted.then_some(id)
}

/// One `MissionType` document, exactly as `MissionType.model_validate` accepts it.
pub(crate) fn admits_mission_type(document: &ArtifactDocument) -> Option<PythonString> {
    let mapping = document.value.as_mapping()?;
    let view = View {
        document: Some(document),
    };
    const KEYS: [&str; 8] = [
        "schema_version",
        "id",
        "display_name",
        "extends",
        "action_sequence",
        "path_conventions",
        "requires_substantive_spec",
        "code_producing",
    ];
    if !view.keys(mapping, &KEYS) {
        return None;
    }
    let id = view.string(mapping.get("id")?)?;
    if !lower_kebab(&id) {
        return None;
    }
    // `schema_version: int = 1`
    if let Some(value) = mapping.get("schema_version")
        && view.int(value).is_none()
    {
        return None;
    }
    view.string(mapping.get("display_name")?)?;
    if let Some(value) = mapping.get("extends")
        && !matches!(value, Value::Null)
        && view.string(value).is_none()
    {
        return None;
    }
    if let Some(value) = mapping.get("action_sequence")
        && !matches!(value, Value::Null)
    {
        let items = value.as_sequence()?;
        if items.is_empty() {
            return None;
        }
        let mut seen = std::collections::BTreeSet::new();
        for item in items {
            let text = view.string(item)?;
            if !seen.insert(text) {
                return None;
            }
        }
    }
    if let Some(value) = mapping.get("path_conventions")
        && !matches!(value, Value::Null)
    {
        let entries = value.as_mapping()?;
        for (key, item) in entries {
            let key = key.as_str()?;
            if !VALID_PATH_KEYS.contains(&key) {
                return None;
            }
            view.string(item)?;
        }
    }
    for field in ["requires_substantive_spec", "code_producing"] {
        if let Some(value) = mapping.get(field)
            && !matches!(value, Value::Null)
            && view.boolean(value).is_none()
        {
            return None;
        }
    }
    Some(id)
}

/// Admission of one `PackSkill` document without a lossless `ArtifactDocument`.
///
/// The installer's checked read-model adapter starts from an already-parsed
/// `serde_yaml_ng::Value`; the model decision must still be the ONE whole-model
/// authority rather than a second parser. Only the string accessor differs: a
/// plain tree has no transport markers, so an authored `str` is its own code
/// points and `!!binary` still follows Pydantic's lax `str` coercion.
///
/// Returns the model `id` when admitted, exactly like [`admits_mapping`].
pub(crate) fn admits_pack_skill_value(value: &Value) -> Option<PythonString> {
    let mapping = value.as_mapping()?;
    let view = View { document: None };
    let id = view.string(mapping.get("id")?)?;
    view.pack_skill(mapping, &id).then_some(id)
}

/// Pydantic lax `str` for a tree without lossless transport markers.
fn plain_string_value(value: &Value) -> Option<PythonString> {
    match value {
        Value::String(text) => Some(PythonString::from_scalars(text)),
        Value::Tagged(_) => crate::profile_construct::constructed_bytes(value)
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .map(|text| PythonString::from_scalars(&text)),
        _ => None,
    }
}

/// Admission helpers over one constructed document (or a plain tree).
struct View<'a> {
    document: Option<&'a ArtifactDocument>,
}

impl View<'_> {
    /// `extra="forbid"`: every key is a declared field.
    fn keys(&self, mapping: &Mapping, allowed: &[&str]) -> bool {
        mapping
            .keys()
            .all(|key| key.as_str().is_some_and(|text| allowed.contains(&text)))
    }

    /// Pydantic lax `str`: `str`, or UTF-8 `bytes`.
    fn string(&self, value: &Value) -> Option<PythonString> {
        match self.document {
            Some(document) => document.python_string_value(value),
            None => plain_string_value(value),
        }
    }

    /// A **constrained** `str` field (`Field(min_length=…)`).
    ///
    /// Pydantic-core validates a constrained `str` with its unicode check, so a
    /// lone surrogate unit is refused (`string_unicode`) and the record is
    /// skipped. An *unconstrained* `str` field keeps the exact code points — the
    /// pinned reference serves such a record and fails only when it physically
    /// encodes the rendered text — so this predicate is applied to the
    /// constrained fields only (`title`, `description`, `SkillParameter.name`;
    /// `SkillExpansion.target` is already scalarised below for its ASCII shape
    /// checks).
    fn constrained_string(&self, value: &Value) -> Option<PythonString> {
        let text = self.string(value)?;
        let has_surrogate = text
            .codepoints()
            .iter()
            .any(|code| matches!(code, 0xD800..=0xDFFF));
        (!has_surrogate).then_some(text)
    }

    fn string_field(&self, mapping: &Mapping, key: &str) -> bool {
        mapping
            .get(key)
            .is_some_and(|value| self.string(value).is_some())
    }

    /// `str | None`
    fn optional_string(&self, mapping: &Mapping, key: &str) -> bool {
        match mapping.get(key) {
            None => true,
            Some(Value::Null) => true,
            Some(value) => self.string(value).is_some(),
        }
    }

    /// `list[str]` (`None` when absent, per `default_factory`).
    fn string_list(&self, mapping: &Mapping, key: &str) -> bool {
        match mapping.get(key) {
            None => true,
            Some(Value::Null) => false,
            Some(value) => self.sequence_of_strings(value),
        }
    }

    fn sequence_of_strings(&self, value: &Value) -> bool {
        crate::profile_construct::constructed_sequence(value)
            .is_some_and(|items| items.iter().all(|item| self.string(item).is_some()))
    }

    /// `list[str] = Field(min_length=1)` — present values must be non-empty.
    fn non_empty_string_list(&self, mapping: &Mapping, key: &str) -> bool {
        mapping.get(key).is_some_and(|value| {
            crate::profile_construct::constructed_sequence(value).is_some_and(|items| {
                !items.is_empty() && items.iter().all(|item| self.string(item).is_some())
            })
        })
    }

    /// `list[X]` of mappings validated by `each`, `min_length` when required.
    fn sequence_of_mappings(
        &self,
        mapping: &Mapping,
        key: &str,
        required: bool,
        min_length: bool,
        each: impl Fn(&Self, &Mapping) -> bool,
    ) -> bool {
        match mapping.get(key) {
            None => !required,
            Some(value) => {
                let Some(items) = crate::profile_construct::constructed_sequence(value) else {
                    return false;
                };
                if min_length && items.is_empty() {
                    return false;
                }
                items
                    .iter()
                    .all(|item| item.as_mapping().is_some_and(|fields| each(self, fields)))
            }
        }
    }

    /// `dict[str, str]`
    fn string_map(&self, mapping: &Mapping, key: &str) -> bool {
        match mapping.get(key) {
            None => true,
            Some(Value::Null) => false,
            Some(value) => value.as_mapping().is_some_and(|entries| {
                entries
                    .iter()
                    .all(|(key, item)| self.string(key).is_some() && self.string(item).is_some())
            }),
        }
    }

    /// `dict[str, Any] | None`: keys use the same lax string admission.
    fn optional_mapping(&self, mapping: &Mapping, key: &str) -> bool {
        match mapping.get(key) {
            None | Some(Value::Null) => true,
            Some(value) => value
                .as_mapping()
                .is_some_and(|entries| entries.keys().all(|key| self.string(key).is_some())),
        }
    }

    /// An `ArtifactKind` field: the canonical singular value only.
    fn kind(&self, value: &Value) -> bool {
        self.string(value).is_some_and(|text| {
            text.to_scalar_string()
                .ok()
                .and_then(|text| ArtifactKind::from_canonical(&text).ok())
                .is_some()
        })
    }

    /// An `ArtifactKind` field with Pydantic's **exact** enum-value semantics.
    ///
    /// The reference's enum members are matched by value, with no trimming,
    /// case folding or hyphen aliasing, so `agent-profile` and `Agent_Profile`
    /// are rejected while `agent_profile` is accepted. Membership is decided
    /// against the one kind authority ([`ArtifactKind::ALL`]), never a private
    /// kind table.
    fn canonical_kind(&self, value: &Value) -> bool {
        self.string(value).is_some_and(|text| {
            text.to_scalar_string().is_ok_and(|text| {
                ArtifactKind::ALL
                    .iter()
                    .any(|kind| kind.canonical() == text)
            })
        })
    }

    /// Pydantic lax `bool`.
    fn boolean(&self, value: &Value) -> Option<bool> {
        match value {
            Value::Bool(flag) => Some(*flag),
            Value::Number(number) => {
                let float = number.as_f64()?;
                if float == 1.0 {
                    Some(true)
                } else if float == 0.0 {
                    Some(false)
                } else {
                    None
                }
            }
            Value::String(_) => {
                let text = self.string(value)?.to_scalar_string().ok()?;
                lax_bool_text(&text)
            }
            _ => None,
        }
    }

    /// Pydantic lax `int`.
    fn int(&self, value: &Value) -> Option<i64> {
        match value {
            Value::Bool(flag) => Some(i64::from(*flag)),
            Value::Number(number) => {
                if let Some(integer) = number.as_i64() {
                    return Some(integer);
                }
                let float = number.as_f64()?;
                // A non-integral or non-finite float is not an `int`.
                if float.is_finite() && float.fract() == 0.0 && float.abs() <= 9.2e18 {
                    Some(float as i64)
                } else {
                    None
                }
            }
            Value::String(_) => {
                let text = self.string(value)?.to_scalar_string().ok()?;
                lax_int_text(&text)
            }
            _ => None,
        }
    }

    /// Pydantic lax `float`.
    fn float(&self, value: &Value) -> Option<f64> {
        match value {
            Value::Bool(flag) => Some(if *flag { 1.0 } else { 0.0 }),
            Value::Number(number) => number.as_f64(),
            Value::String(_) => {
                let text = self.string(value)?.to_scalar_string().ok()?;
                lax_float_text(&text)
            }
            _ => None,
        }
    }

    // ------------------------------------------------------------------
    // Directive + references (`charter/offering/directives/models.py`)
    // ------------------------------------------------------------------

    /// `Directive.model_validate`.
    ///
    /// The field set is `extra="forbid"`; `id` is the model's uppercase pattern
    /// (`populate_by_name=True` adds no alias here because every alias is the
    /// field's own name), `schema_version` is the `^1\.0$` string, `title`/
    /// `intent` are required text, `enforcement` is the closed
    /// `Enforcement` value set, and the list fields default to empty. The model
    /// validator rejects `lenient-adherence` without a non-empty
    /// `explicit_allowances` list — the reference's one directive-specific rule.
    fn directive(&self, mapping: &Mapping, id: &PythonString) -> bool {
        const KEYS: [&str; 11] = [
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
        ];
        if !self.keys(mapping, &KEYS)
            || !directive_id(id)
            || !self.schema_version_1(mapping)
            || !self.string_field(mapping, "title")
            || !self.string_field(mapping, "intent")
            || !self.optional_string(mapping, "scope")
        {
            return false;
        }
        let Some(enforcement) = mapping
            .get("enforcement")
            .and_then(|value| self.string(value))
            .and_then(|text| text.to_scalar_string().ok())
        else {
            return false;
        };
        if crate::body::ENFORCEMENT
            .member_for_text(&PythonString::from_scalars(&enforcement))
            .is_none()
        {
            return false;
        }
        if !self.string_list(mapping, "procedures")
            || !self.string_list(mapping, "integrity_rules")
            || !self.string_list(mapping, "validation_criteria")
            || !self.string_list(mapping, "explicit_allowances")
            || !self.sequence_of_mappings(mapping, "references", false, false, |view, reference| {
                view.directive_reference(reference)
            })
        {
            return false;
        }
        if enforcement == "lenient-adherence" {
            // `validate_lenient_adherence` runs after field validation, so a
            // present-but-wrong `explicit_allowances` already failed above.
            return mapping
                .get("explicit_allowances")
                .and_then(crate::profile_construct::constructed_sequence)
                .is_some_and(|items| !items.is_empty());
        }
        true
    }

    /// `DirectiveReference`: `{type: ArtifactKind, id: str, when?: str, reason?: str}`.
    ///
    /// This model has no `references.…`-specific alias or pattern; `id` is any
    /// text and only the kind is exact-value closed.
    fn directive_reference(&self, reference: &Mapping) -> bool {
        const KEYS: [&str; 4] = ["type", "id", "when", "reason"];
        self.keys(reference, &KEYS)
            && reference
                .get("type")
                .is_some_and(|value| self.canonical_kind(value))
            && self.string_field(reference, "id")
            && self.optional_string(reference, "when")
            && self.optional_string(reference, "reason")
    }

    // ------------------------------------------------------------------
    // Tactic + nested models
    // ------------------------------------------------------------------

    fn tactic(&self, mapping: &Mapping, id: &PythonString) -> bool {
        const KEYS: [&str; 9] = [
            "id",
            "schema_version",
            "name",
            "purpose",
            "steps",
            "failure_modes",
            "applies_to_languages",
            "references",
            "notes",
        ];
        self.keys(mapping, &KEYS)
            && lower_kebab(id)
            && self.schema_version_1(mapping)
            && self.string_field(mapping, "name")
            && self.optional_string(mapping, "purpose")
            && self.optional_string(mapping, "notes")
            && self.string_list(mapping, "failure_modes")
            && self.string_list(mapping, "applies_to_languages")
            && self.sequence_of_mappings(mapping, "steps", true, true, |view, step| {
                view.tactic_step(step)
            })
            && self.sequence_of_mappings(mapping, "references", false, false, |view, reference| {
                view.tactic_reference(reference)
            })
    }

    fn tactic_step(&self, step: &Mapping) -> bool {
        const KEYS: [&str; 4] = ["title", "description", "examples", "references"];
        self.keys(step, &KEYS)
            && self.string_field(step, "title")
            && self.optional_string(step, "description")
            && self.string_list(step, "examples")
            && self.sequence_of_mappings(step, "references", false, false, |view, reference| {
                view.tactic_reference(reference)
            })
    }

    fn tactic_reference(&self, reference: &Mapping) -> bool {
        const KEYS: [&str; 4] = ["name", "type", "id", "when"];
        self.keys(reference, &KEYS)
            && self.string_field(reference, "name")
            && reference.get("type").is_some_and(|value| self.kind(value))
            && reference
                .get("id")
                .is_some_and(|value| self.string(value).is_some_and(|text| reference_id(&text)))
            && self.string_field(reference, "when")
    }

    // ------------------------------------------------------------------
    // Styleguide + nested models
    // ------------------------------------------------------------------

    fn styleguide(&self, mapping: &Mapping, id: &PythonString) -> bool {
        const KEYS: [&str; 12] = [
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
        ];
        self.keys(mapping, &KEYS)
            && lower_kebab(id)
            && self.schema_version_1(mapping)
            && self.string_field(mapping, "title")
            && mapping.get("scope").is_some_and(|value| {
                self.string(value).is_some_and(|text| {
                    crate::body::STYLEGUIDE_SCOPE.member_for_text(&text).is_some()
                })
            })
            && self.non_empty_string_list(mapping, "principles")
            // `Field(default_factory=list, min_length=1)`: the default is not
            // validated, but a *present* list must be non-empty.
            && self.sequence_of_mappings(mapping, "patterns", false, true, |view, item| {
                view.pattern(item)
            })
            && self.sequence_of_mappings(mapping, "anti_patterns", false, true, |view, item| {
                view.styleguide_anti_pattern(item)
            })
            && self.string_map(mapping, "tooling")
            && self.optional_string(mapping, "quality_test")
            && self.string_list(mapping, "applies_to_languages")
            && self.string_list(mapping, "references")
            && self.optional_mapping(mapping, "structural_lint_config")
    }

    fn pattern(&self, item: &Mapping) -> bool {
        const KEYS: [&str; 4] = ["name", "description", "bad_example", "good_example"];
        self.keys(item, &KEYS)
            && self.string_field(item, "name")
            && self.string_field(item, "description")
            && self.optional_string(item, "bad_example")
            && self.optional_string(item, "good_example")
    }

    fn styleguide_anti_pattern(&self, item: &Mapping) -> bool {
        const KEYS: [&str; 4] = ["name", "description", "bad_example", "good_example"];
        self.keys(item, &KEYS)
            && self.string_field(item, "name")
            && self.string_field(item, "description")
            && self.string_field(item, "bad_example")
            && self.string_field(item, "good_example")
    }

    // ------------------------------------------------------------------
    // Toolguide
    // ------------------------------------------------------------------

    fn toolguide(&self, mapping: &Mapping, id: &PythonString) -> bool {
        const KEYS: [&str; 10] = [
            "id",
            "schema_version",
            "tool",
            "title",
            "guide_path",
            "summary",
            "commands",
            "applies_to_languages",
            "last_updated",
            "references",
        ];
        self.keys(mapping, &KEYS)
            && lower_kebab(id)
            && self.schema_version_1(mapping)
            && self.string_field(mapping, "tool")
            && self.string_field(mapping, "title")
            && self.string_field(mapping, "summary")
            && mapping
                .get("guide_path")
                .is_some_and(|value| self.string(value).is_some_and(|text| guide_path(&text)))
            && self.string_list(mapping, "commands")
            && self.string_list(mapping, "applies_to_languages")
            && self.string_list(mapping, "references")
            && self.optional_string(mapping, "last_updated")
    }

    // ------------------------------------------------------------------
    // Paradigm
    // ------------------------------------------------------------------

    fn paradigm(&self, mapping: &Mapping, id: &PythonString) -> bool {
        const KEYS: [&str; 6] = [
            "schema_version",
            "id",
            "name",
            "summary",
            "directive_refs",
            "references",
        ];
        self.keys(mapping, &KEYS)
            && lower_kebab(id)
            && self.schema_version_1(mapping)
            && self.string_field(mapping, "name")
            && self.string_field(mapping, "summary")
            && self.string_list(mapping, "directive_refs")
            && self.sequence_of_mappings(mapping, "references", false, false, |view, item| {
                const KEYS: [&str; 2] = ["type", "id"];
                view.keys(item, &KEYS)
                    && item.get("type").is_some_and(|value| view.kind(value))
                    && view.string_field(item, "id")
            })
    }

    // ------------------------------------------------------------------
    // Procedure + nested models
    // ------------------------------------------------------------------

    fn procedure(&self, mapping: &Mapping, id: &PythonString) -> bool {
        const KEYS: [&str; 11] = [
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
        ];
        self.keys(mapping, &KEYS)
            && lower_kebab(id)
            && self.schema_version_1(mapping)
            && self.string_field(mapping, "name")
            && self.string_field(mapping, "purpose")
            && self.string_field(mapping, "entry_condition")
            && self.string_field(mapping, "exit_condition")
            && self.sequence_of_mappings(mapping, "steps", true, true, |view, step| {
                view.procedure_step(step)
            })
            && self.sequence_of_mappings(mapping, "anti_patterns", false, false, |view, item| {
                const KEYS: [&str; 2] = ["name", "description"];
                view.keys(item, &KEYS)
                    && item.get("name").is_some_and(|value| {
                        view.string(value).is_some_and(|text| !text.is_empty())
                    })
                    && item.get("description").is_some_and(|value| {
                        view.string(value).is_some_and(|text| !text.is_empty())
                    })
            })
            && self.string_list(mapping, "applies_to_languages")
            && self.optional_string(mapping, "notes")
            && self.sequence_of_mappings(mapping, "references", false, false, |view, item| {
                const KEYS: [&str; 3] = ["type", "id", "reason"];
                view.keys(item, &KEYS)
                    && item.get("type").is_some_and(|value| view.kind(value))
                    && view.string_field(item, "id")
                    && view.optional_string(item, "reason")
            })
    }

    fn procedure_step(&self, step: &Mapping) -> bool {
        const KEYS: [&str; 5] = ["title", "description", "actor", "on_success", "on_failure"];
        self.keys(step, &KEYS)
            && self.string_field(step, "title")
            && self.optional_string(step, "description")
            && match step.get("actor") {
                None => true,
                Some(value) => self
                    .string(value)
                    .is_some_and(|text| crate::body::ACTOR_ROLE.member_for_text(&text).is_some()),
            }
            && self.optional_string(step, "on_success")
            && self.optional_string(step, "on_failure")
    }

    // ------------------------------------------------------------------
    // Glossary pack + terms
    // ------------------------------------------------------------------

    fn glossary_pack(&self, mapping: &Mapping) -> bool {
        const KEYS: [&str; 4] = ["id", "provenance", "terms", "description"];
        if !self.keys(mapping, &KEYS)
            || !self.string_field(mapping, "provenance")
            || !self.optional_string(mapping, "description")
        {
            return false;
        }
        let Some(terms) = mapping.get("terms") else {
            return false;
        };
        let Some(items) = crate::profile_construct::constructed_sequence(terms) else {
            return false;
        };
        if items.is_empty() {
            return false;
        }
        let mut surfaces: Vec<PythonString> = Vec::with_capacity(items.len());
        for term in items {
            let Some(fields) = term.as_mapping() else {
                return false;
            };
            if !self.glossary_term(fields) {
                return false;
            }
            let Some(surface) = fields.get("surface").and_then(|value| self.string(value)) else {
                return false;
            };
            if surfaces.contains(&surface) {
                return false;
            }
            surfaces.push(surface);
        }
        true
    }

    fn glossary_term(&self, term: &Mapping) -> bool {
        const KEYS: [&str; 9] = [
            "surface",
            "definition",
            "confidence",
            "status",
            "see_also",
            "introduced_in_mission",
            "synonyms_to_avoid",
            "aliases",
            "banned_synonyms",
        ];
        self.keys(term, &KEYS)
            && self.string_field(term, "surface")
            && self.string_field(term, "definition")
            && term
                .get("confidence")
                .is_some_and(|value| self.float(value).is_some())
            && self.string_field(term, "status")
            && self.string_list(term, "see_also")
            && self.optional_string(term, "introduced_in_mission")
            && self.string_list(term, "synonyms_to_avoid")
            && self.string_list(term, "aliases")
            && self.string_list(term, "banned_synonyms")
    }

    // ------------------------------------------------------------------
    // Mission step contract + nested models
    // ------------------------------------------------------------------

    fn mission_step_contract(&self, mapping: &Mapping) -> bool {
        const KEYS: [&str; 6] = [
            "id",
            "schema_version",
            "action",
            "mission",
            "steps",
            "gates",
        ];
        if !self.keys(mapping, &KEYS)
            || !self.string_field(mapping, "schema_version")
            || !self.string_field(mapping, "action")
            || !self.string_field(mapping, "mission")
        {
            return false;
        }
        if !self.sequence_of_mappings(mapping, "steps", true, true, |view, step| {
            view.step_contract_step(step)
        }) {
            return false;
        }
        if !self.unique_step_ids(mapping) {
            return false;
        }
        self.sequence_of_mappings(mapping, "gates", false, false, |view, gate| {
            view.gate_binding(gate)
        })
    }

    fn unique_step_ids(&self, mapping: &Mapping) -> bool {
        let Some(items) = mapping
            .get("steps")
            .and_then(crate::profile_construct::constructed_sequence)
        else {
            return false;
        };
        let mut ids: Vec<PythonString> = Vec::with_capacity(items.len());
        for item in items {
            let Some(id) = item
                .as_mapping()
                .and_then(|fields| fields.get("id"))
                .and_then(|value| self.string(value))
            else {
                return false;
            };
            if ids.contains(&id) {
                return false;
            }
            ids.push(id);
        }
        true
    }

    fn step_contract_step(&self, step: &Mapping) -> bool {
        const KEYS: [&str; 6] = [
            "id",
            "description",
            "command",
            "inputs",
            "delegates_to",
            "guidance",
        ];
        self.keys(step, &KEYS)
            && self.string_field(step, "id")
            && self.string_field(step, "description")
            && self.optional_string(step, "command")
            && self.optional_string(step, "guidance")
            && self.sequence_of_mappings(step, "inputs", false, false, |view, input| {
                const KEYS: [&str; 3] = ["flag", "source", "optional"];
                view.keys(input, &KEYS)
                    && input.get("flag").is_some_and(|value| {
                        view.string(value).is_some_and(|text| !text.is_empty())
                    })
                    && input.get("source").is_some_and(|value| {
                        view.string(value).is_some_and(|text| !text.is_empty())
                    })
                    && match input.get("optional") {
                        None => true,
                        Some(value) => view.boolean(value).is_some(),
                    }
            })
            && match step.get("delegates_to") {
                None | Some(Value::Null) => true,
                Some(value) => value.as_mapping().is_some_and(|delegates| {
                    const KEYS: [&str; 2] = ["kind", "candidates"];
                    self.keys(delegates, &KEYS)
                        && delegates.get("kind").is_some_and(|value| self.kind(value))
                        && self.non_empty_string_list(delegates, "candidates")
                }),
            }
    }

    fn gate_binding(&self, gate: &Mapping) -> bool {
        const KEYS: [&str; 6] = [
            "on_transition",
            "handler",
            "handler_kind",
            "schema_version",
            "fail_open",
            "provenance",
        ];
        self.keys(gate, &KEYS)
            && self.string_field(gate, "on_transition")
            && self.string_field(gate, "handler")
            && match gate.get("handler_kind") {
                None => true,
                Some(value) => self.string(value).is_some_and(|text| {
                    matches!(
                        text.to_scalar_string().ok().as_deref(),
                        Some("mission_step_contract") | Some("asset")
                    )
                }),
            }
            && self.string_field(gate, "schema_version")
            && match gate.get("fail_open") {
                None => true,
                Some(value) => self.boolean(value).is_some(),
            }
            && self.optional_string(gate, "provenance")
    }

    // ------------------------------------------------------------------
    // Pack skill + nested models (`charter/offering/pack_skills/models.py`)
    // ------------------------------------------------------------------

    /// `PackSkill.model_validate`: field set, `Literal["1.0"]`, id grammar,
    /// form discriminator with its `body_path`/`expands_to` exclusivity, the
    /// wrapper argv guards, augmentation exclusivity and unique parameters.
    ///
    /// Tier, namespace and body-file rules are the repository's `_post_validate`
    /// (they need the source file and its tier); they are not model admission
    /// and are deliberately not decided here.
    fn pack_skill(&self, mapping: &Mapping, id: &PythonString) -> bool {
        const KEYS: [&str; 15] = [
            "schema_version",
            "id",
            "title",
            "description",
            "triggers",
            "form",
            "body_path",
            "expands_to",
            "parameters",
            "invocation",
            "tools",
            "version",
            "maintainers",
            "overrides",
            "enhances",
        ];
        if !self.keys(mapping, &KEYS) || !skill_id(id) {
            return false;
        }
        // `schema_version: Literal["1.0"]`
        if mapping
            .get("schema_version")
            .and_then(|value| self.string(value))
            .and_then(|text| text.to_scalar_string().ok())
            .as_deref()
            != Some("1.0")
        {
            return false;
        }
        // `title`/`description`: `Field(min_length=1)` (constrained `str`).
        for field in ["title", "description"] {
            let present = mapping
                .get(field)
                .and_then(|value| self.constrained_string(value))
                .is_some_and(|text| !text.is_empty());
            if !present {
                return false;
            }
        }
        // `form: Literal["prompt", "wrapper"]`
        let Some(form) = mapping
            .get("form")
            .and_then(|value| self.string(value))
            .and_then(|text| text.to_scalar_string().ok())
        else {
            return false;
        };
        if form != "prompt" && form != "wrapper" {
            return false;
        }
        if !self.string_list(mapping, "triggers")
            || !self.string_list(mapping, "tools")
            || !self.string_list(mapping, "maintainers")
            || !self.optional_string(mapping, "body_path")
            || !self.optional_string(mapping, "version")
            || !self.optional_string(mapping, "overrides")
            || !self.optional_string(mapping, "enhances")
        {
            return false;
        }
        let expands_present = match mapping.get("expands_to") {
            None | Some(Value::Null) => false,
            Some(value) => match value.as_mapping() {
                Some(fields) => {
                    if !self.skill_expansion(fields) {
                        return false;
                    }
                    true
                }
                None => return false,
            },
        };
        if !self.sequence_of_mappings(mapping, "parameters", false, false, |view, parameter| {
            view.skill_parameter(parameter)
        }) {
            return false;
        }
        match mapping.get("invocation") {
            // `Field(default_factory=SkillInvocation)`: absent takes the default.
            None => {}
            // Not optional: an explicit null is refused, not defaulted.
            Some(Value::Null) => return false,
            Some(value) => match value.as_mapping() {
                Some(fields) => {
                    if !self.skill_invocation(fields) {
                        return false;
                    }
                }
                None => return false,
            },
        }
        let body_present = mapping
            .get("body_path")
            .is_some_and(|value| !value.is_null());
        match form.as_str() {
            "prompt" => {
                if !body_present || expands_present {
                    return false;
                }
            }
            _ => {
                if !expands_present || body_present {
                    return false;
                }
            }
        }
        let augmentation = |field: &str| {
            mapping
                .get(field)
                .filter(|value| !value.is_null())
                .and_then(|value| self.string(value))
        };
        let overrides = augmentation("overrides");
        let enhances = augmentation("enhances");
        if overrides.is_some() && enhances.is_some() {
            return false;
        }
        if overrides.as_ref() == Some(id) || enhances.as_ref() == Some(id) {
            return false;
        }
        match mapping
            .get("parameters")
            .and_then(crate::profile_construct::constructed_sequence)
        {
            None => true,
            Some(items) => {
                let mut names: Vec<PythonString> = Vec::with_capacity(items.len());
                for item in items {
                    let Some(name) = item
                        .as_mapping()
                        .and_then(|fields| fields.get("name"))
                        .and_then(|value| self.string(value))
                    else {
                        return false;
                    };
                    if names.contains(&name) {
                        return false;
                    }
                    names.push(name);
                }
                true
            }
        }
    }

    /// `SkillExpansion`: `target` shape and the same guard on `args`.
    fn skill_expansion(&self, expansion: &Mapping) -> bool {
        const KEYS: [&str; 2] = ["target", "args"];
        if !self.keys(expansion, &KEYS)
            || !self.optional_string(expansion, "args")
            || expansion.get("args").is_some_and(Value::is_null)
        {
            return false;
        }
        let Some(target) = expansion
            .get("target")
            .and_then(|value| self.string(value))
            .and_then(|text| text.to_scalar_string().ok())
        else {
            return false;
        };
        if target.is_empty() {
            return false;
        }
        if let Some(rest) = target.strip_prefix("builtin:") {
            // A `builtin:` target names an existing command; only non-empty counts.
            return !rest.trim().is_empty();
        }
        let Some(rest) = target.strip_prefix("cli:") else {
            return false;
        };
        if rest.chars().any(is_shell_metacharacter) {
            return false;
        }
        let Some(first) = cli_argv_first_word(rest) else {
            return false;
        };
        if first != "spec-kitty" {
            return false;
        }
        // The renderer appends `args` to the same ```bash`` line.
        match expansion.get("args").filter(|value| !value.is_null()) {
            None => true,
            Some(value) => self.string(value).is_some_and(|text| {
                !text
                    .to_scalar_string()
                    .unwrap_or_default()
                    .chars()
                    .any(is_shell_metacharacter)
            }),
        }
    }

    /// `SkillParameter`.
    fn skill_parameter(&self, parameter: &Mapping) -> bool {
        const KEYS: [&str; 4] = ["name", "required", "description", "default"];
        self.keys(parameter, &KEYS)
            && parameter
                .get("name")
                // `name: str = Field(min_length=1)`: a constrained `str`.
                .and_then(|value| self.constrained_string(value))
                .is_some_and(|text| !text.is_empty())
            && match parameter.get("required") {
                None => true,
                Some(value) => self.boolean(value).is_some(),
            }
            && match parameter.get("description") {
                None => true,
                Some(value) => self.string(value).is_some(),
            }
            && self.optional_string(parameter, "default")
    }

    /// `SkillInvocation` and its `mode="before"` `model_invocable` rewrite.
    fn skill_invocation(&self, invocation: &Mapping) -> bool {
        const KEYS: [&str; 3] = ["user_invocable", "model_invocable", "side_effects"];
        if !self.keys(invocation, &KEYS) || !self.string_list(invocation, "side_effects") {
            return false;
        }
        if let Some(value) = invocation.get("user_invocable")
            && self.boolean(value).is_none()
        {
            return false;
        }
        // Declared side effects force `model_invocable=False` in the model, so the
        // authored value is replaced before field validation (an authored invalid
        // spelling is therefore accepted in exactly that case, as in Pydantic).
        let forced = invocation
            .get("side_effects")
            .and_then(crate::profile_construct::constructed_sequence)
            .is_some_and(|items| !items.is_empty());
        if forced {
            return true;
        }
        match invocation.get("model_invocable") {
            None => true,
            Some(value) => self.boolean(value).is_some(),
        }
    }

    /// `schema_version: str = Field(pattern=r"^1\.0$")`.
    fn schema_version_1(&self, mapping: &Mapping) -> bool {
        mapping
            .get("schema_version")
            .and_then(|value| self.string(value))
            .is_some_and(|text| text.to_scalar_string().ok().as_deref() == Some("1.0"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(text: &str) -> ArtifactDocument {
        let directory = tempfile::tempdir().expect("tempdir");
        let path: std::path::PathBuf = directory.path().join("doc.yaml");
        std::fs::write(&path, text).expect("write");
        ArtifactDocument::read(&path).expect("read")
    }

    fn admitted(kind: ArtifactKind, text: &str) -> bool {
        model_id(kind, &document(text)).is_some()
    }

    #[test]
    fn nested_step_contract_rules_are_enforced() {
        let valid = "schema_version: '1.0'\nid: c\naction: a\nmission: m\nsteps:\n  - id: one\n    description: d\n";
        assert!(admitted(ArtifactKind::MissionStepContract, valid));
        let duplicate = valid.replace(
            "  - id: one\n    description: d\n",
            "  - id: one\n    description: d\n  - id: one\n    description: e\n",
        );
        assert!(!admitted(ArtifactKind::MissionStepContract, &duplicate));
        let bad_gate = format!(
            "{valid}gates:\n  - on_transition: a->b\n    handler: h\n    handler_kind: nope\n    schema_version: '1.0'\n"
        );
        assert!(!admitted(ArtifactKind::MissionStepContract, &bad_gate));
        let step_extra = valid.replace("    description: d", "    description: d\n    extra: 1");
        assert!(!admitted(ArtifactKind::MissionStepContract, &step_extra));
        let no_candidates = "schema_version: '1.0'\nid: c\naction: a\nmission: m\nsteps:\n  - id: one\n    description: d\n    delegates_to:\n      kind: tactic\n      candidates: []\n";
        assert!(!admitted(ArtifactKind::MissionStepContract, no_candidates));
    }

    #[test]
    fn directive_admission_follows_the_reference_model() {
        const VALID: &str = "schema_version: '1.0'\nid: DIRECTIVE_900\ntitle: T\nintent: I\nenforcement: required\n";
        assert!(admitted(ArtifactKind::Directive, VALID));
        // Optional enrichment fields, the closed `Enforcement` value set and a
        // typed reference with its optional curated edge metadata.
        let rich = "schema_version: '1.0'\nid: DIRECTIVE-901_X\ntitle: T\nintent: I\nenforcement: advisory\nscope: s\nprocedures: [a]\nintegrity_rules: [b]\nvalidation_criteria: [c]\nexplicit_allowances: [d]\nreferences:\n  - type: tactic\n    id: any-id\n    when: w\n    reason: r\n";
        assert!(admitted(ArtifactKind::Directive, rich));
        // `!!binary` ids are UTF-8 bytes, which the lax `str` coercion accepts.
        assert!(admitted(
            ArtifactKind::Directive,
            "schema_version: '1.0'\nid: !!binary |\n  RE1fOTAw\ntitle: T\nintent: I\nenforcement: required\n"
        ));
        // `list[str]` accepts a constructed `!!set`, exactly like the reference.
        assert!(admitted(
            ArtifactKind::Directive,
            &format!("{VALID}procedures: !!set\n  ? a\n  ? b\n")
        ));
        // The lenient-adherence model validator: allowances are required and
        // must be non-empty once the fields validated.
        let lenient = "schema_version: '1.0'\nid: DIRECTIVE_902\ntitle: T\nintent: I\nenforcement: lenient-adherence\n";
        assert!(!admitted(ArtifactKind::Directive, lenient));
        assert!(admitted(
            ArtifactKind::Directive,
            &format!("{lenient}explicit_allowances: [x]\n")
        ));
        assert!(!admitted(
            ArtifactKind::Directive,
            &format!("{lenient}explicit_allowances: []\n")
        ));
        // A null `scope` is the documented `str | None` default.
        assert!(admitted(
            ArtifactKind::Directive,
            &format!("{VALID}scope: null\n")
        ));
        let rejected = [
            // required fields
            "schema_version: '1.0'\nid: DIRECTIVE_903\ntitle: T\nintent: I\n".to_owned(),
            "schema_version: '1.0'\nid: DIRECTIVE_903\ntitle: T\nenforcement: required\n".to_owned(),
            // `schema_version` is the `^1\.0$` string, not a number or bool
            "schema_version: 1.0\nid: DIRECTIVE_903\ntitle: T\nintent: I\nenforcement: required\n"
                .to_owned(),
            "schema_version: 1\nid: DIRECTIVE_903\ntitle: T\nintent: I\nenforcement: required\n"
                .to_owned(),
            "schema_version: true\nid: DIRECTIVE_903\ntitle: T\nintent: I\nenforcement: required\n"
                .to_owned(),
            // the id pattern is upper-case only, and `$` is end-of-text
            "schema_version: '1.0'\nid: directive_903\ntitle: T\nintent: I\nenforcement: required\n"
                .to_owned(),
            "schema_version: '1.0'\nid: 903\ntitle: T\nintent: I\nenforcement: required\n".to_owned(),
            "schema_version: '1.0'\nid: \"DIRECTIVE_903\\n\"\ntitle: T\nintent: I\nenforcement: required\n"
                .to_owned(),
            // the enforcement enum matches by value only
            "schema_version: '1.0'\nid: DIRECTIVE_903\ntitle: T\nintent: I\nenforcement: REQUIRED\n"
                .to_owned(),
            "schema_version: '1.0'\nid: DIRECTIVE_903\ntitle: T\nintent: I\nenforcement: 1\n"
                .to_owned(),
            // `str` does not coerce numbers, and list fields refuse an explicit null
            format!("{VALID}scope: 5\n"),
            format!("{}title: 123\nintent: I\nenforcement: required\n", "schema_version: '1.0'\nid: DIRECTIVE_903\n"),
            format!("{VALID}procedures: null\n"),
            format!("{VALID}procedures: abc\n"),
            // `extra="forbid"` at both levels
            format!("{VALID}extra: 1\n"),
            format!("{VALID}references:\n  - type: tactic\n    id: x\n    extra: 1\n"),
            // the reference kind is the exact canonical value, not its aliases
            format!("{VALID}references:\n  - type: agent-profile\n    id: x\n"),
            format!("{VALID}references:\n  - type: Agent_Profile\n    id: x\n"),
            format!("{VALID}references:\n  - type: directives\n    id: x\n"),
            // required nested members
            format!("{VALID}references:\n  - type: tactic\n"),
            format!("{VALID}references:\n  - type: tactic\n    id: 5\n"),
            format!("{VALID}references:\n  - null\n"),
        ];
        for case in rejected {
            assert!(!admitted(ArtifactKind::Directive, &case), "{case}");
        }
    }

    #[test]
    fn nested_tactic_rules_are_enforced() {
        let valid = "schema_version: '1.0'\nid: t\nname: n\nsteps:\n  - title: s\n";
        assert!(admitted(ArtifactKind::Tactic, valid));
        let bad_reference = format!(
            "{valid}references:\n  - name: r\n    type: tactic\n    id: 'Bad_ID!'\n    when: w\n"
        );
        assert!(!admitted(ArtifactKind::Tactic, &bad_reference));
        let step_extra = valid.replace("  - title: s", "  - title: s\n    extra: 1");
        assert!(!admitted(ArtifactKind::Tactic, &step_extra));
        let empty_steps = "schema_version: '1.0'\nid: t\nname: n\nsteps: []\n";
        assert!(!admitted(ArtifactKind::Tactic, empty_steps));
    }

    #[test]
    fn mission_type_admission_follows_the_reference_model() {
        let base = "schema_version: 1\nid: acme-x\ndisplay_name: S\n";
        assert_eq!(
            admits_mission_type(&document(base)).and_then(|id| id.to_scalar_string().ok()),
            Some("acme-x".to_owned())
        );
        for accepted in [
            "schema_version: '1'\nid: acme-x\ndisplay_name: S\n",
            "schema_version: true\nid: acme-x\ndisplay_name: S\n",
            "schema_version: 1.0\nid: acme-x\ndisplay_name: S\n",
            "schema_version: 1\nid: acme-x\ndisplay_name: S\nrequires_substantive_spec: 'yes'\n",
            "schema_version: 1\nid: acme-x\ndisplay_name: S\npath_conventions: {}\n",
            "schema_version: 1\nid: acme-x\ndisplay_name: S\nextends: plan\n",
        ] {
            assert!(
                admits_mission_type(&document(accepted)).is_some(),
                "{accepted}"
            );
        }
        for rejected in [
            "schema_version: 1.5\nid: acme-x\ndisplay_name: S\n",
            "schema_version: 'x'\nid: acme-x\ndisplay_name: S\n",
            "schema_version: 1\nid: acme-x\ndisplay_name: S\npath_conventions:\n  bogus: p\n",
            "schema_version: 1\nid: acme-x\ndisplay_name: S\npath_conventions:\n  workspace: 5\n",
            "schema_version: 1\nid: acme-x\ndisplay_name: S\nextends: 123\n",
            "schema_version: 1\nid: acme-x\ndisplay_name: S\naction_sequence: [a, a]\n",
            "schema_version: 1\nid: acme-x\ndisplay_name: S\naction_sequence: []\n",
            "schema_version: 1\nid: AcmeX\ndisplay_name: S\n",
            "schema_version: 1\nid: acme-x\n",
            "schema_version: 1\nid: acme-x\ndisplay_name: S\ntemplate_set: {}\n",
        ] {
            assert!(
                admits_mission_type(&document(rejected)).is_none(),
                "{rejected}"
            );
        }
    }

    #[test]
    fn glossary_pack_admission_follows_the_reference_model() {
        let term =
            "terms:\n  - surface: s\n    definition: d\n    confidence: 0.5\n    status: active\n";
        let valid = format!("id: pack\nprovenance: built-in\n{term}");
        assert!(admitted(ArtifactKind::GlossaryPack, &valid));
        // A lax numeric `float` spelling and the optional term lists.
        let coerced = "id: pack\nprovenance: built-in\nterms:\n  - surface: s\n    definition: d\n    confidence: '0_5.5'\n    status: active\n    see_also: []\n    aliases: [a]\n".to_owned();
        assert!(admitted(ArtifactKind::GlossaryPack, &coerced));
        for rejected in [
            // `terms` is required with `min_length=1`.
            "id: pack\nprovenance: built-in\n".to_owned(),
            "id: pack\nprovenance: built-in\nterms: []\n".to_owned(),
            // duplicate `surface` values are a model validator error.
            "id: pack\nprovenance: built-in\nterms:\n  - surface: s\n    definition: d\n    confidence: 1\n    status: a\n  - surface: s\n    definition: e\n    confidence: 1\n    status: a\n".to_owned(),
            // `extra="forbid"` at both levels, and required term fields.
            format!("{valid}extra: 1\n"),
            "id: pack\nprovenance: built-in\nterms:\n  - surface: s\n    definition: d\n    confidence: nope\n    status: a\n".to_owned(),
            "id: pack\nprovenance: built-in\nterms:\n  - surface: s\n    definition: d\n    status: a\n".to_owned(),
        ] {
            assert!(!admitted(ArtifactKind::GlossaryPack, &rejected), "{rejected}");
        }
        // `id` has no pattern on this model.
        assert!(admitted(
            ArtifactKind::GlossaryPack,
            &valid.replace("id: pack", "id: Not A Kebab ID")
        ));
    }

    #[test]
    fn paradigm_admission_follows_the_reference_model() {
        let valid =
            "schema_version: '1.0'\nid: p\nname: P\nsummary: S\ndirective_refs: [DIRECTIVE_001]\n";
        assert!(admitted(ArtifactKind::Paradigm, valid));
        let reference = format!("{valid}references:\n  - type: directive\n    id: DIRECTIVE_001\n");
        assert!(admitted(ArtifactKind::Paradigm, &reference));
        for rejected in [
            // `type` is the canonical `ArtifactKind` value, not an id or a plural.
            valid
                .replace("directive_refs: [DIRECTIVE_001]", "")
                .to_string()
                + "references:\n  - type: DIRECTIVE_001\n    id: x\n",
            valid.to_owned() + "references:\n  - type: directives\n    id: x\n",
            // id pattern, required fields, extra key.
            valid.replace("id: p", "id: P"),
            valid.replace("summary: S\n", ""),
            valid.to_owned() + "inline_refs: [x]\n",
        ] {
            assert!(!admitted(ArtifactKind::Paradigm, &rejected), "{rejected}");
        }
    }

    #[test]
    fn pack_skill_admission_follows_the_reference_model() {
        const PROMPT: &str = "schema_version: '1.0'\nid: deploy-helper\ntitle: Deploy helper\ndescription: Runs the deploy.\nform: prompt\nbody_path: deploy-helper.skill.md\n";
        const WRAPPER: &str = "schema_version: '1.0'\nid: release-wrapper\ntitle: R\ndescription: D\nform: wrapper\nexpands_to:\n  target: builtin:spec-kitty.consolidate\n";
        assert!(admitted(ArtifactKind::Skill, PROMPT));
        assert!(admitted(ArtifactKind::Skill, WRAPPER));
        // The model defaults, a lax `bool`, a `cli:` argv and a declaration that
        // forces `model_invocable` off are all admitted.
        let rich = "schema_version: '1.0'\nid: triage\ntitle: T\ndescription: D\nform: prompt\nbody_path: triage.skill.md\ntriggers: [a, b]\nparameters:\n  - name: scope\n    required: 'yes'\n    description: Which issues\n    default: 14d\ninvocation:\n  user_invocable: 0\n  model_invocable: not-a-bool\n  side_effects: [gh-write]\ntools: [bash]\nversion: 2.0.0\nmaintainers: ['@core']\noverrides: other\n";
        assert!(admitted(ArtifactKind::Skill, rich));
        let cli_wrapper = "schema_version: '1.0'\nid: land\ntitle: L\ndescription: D\nform: wrapper\nexpands_to:\n  target: cli:spec-kitty consolidate --mission 'm'\n  args: --dry-run\n";
        assert!(admitted(ArtifactKind::Skill, cli_wrapper));
        // The forced rewrite replaces an authored *invalid* `model_invocable`
        // spelling, exactly like the Pydantic `mode="before"` validator.
        let without_effects = rich.replace("  side_effects: [gh-write]\n", "");
        assert!(!admitted(ArtifactKind::Skill, &without_effects));
        for rejected in [
            // unknown key / `schema_version` is a `Literal["1.0"]` string.
            PROMPT.to_owned() + "extra: 1\n",
            PROMPT.replace("schema_version: '1.0'", "schema_version: 1.0"),
            PROMPT.replace("schema_version: '1.0'", "schema_version: '2.0'"),
            // id grammar and length.
            PROMPT.replace("id: deploy-helper", "id: Deploy"),
            PROMPT.replace("id: deploy-helper", "id: deploy--helper"),
            PROMPT.replace("id: deploy-helper", "id: deploy-"),
            PROMPT.replace("id: deploy-helper", &format!("id: {}", "a".repeat(65))),
            // non-empty title/description.
            PROMPT.replace("title: Deploy helper", "title: ''"),
            PROMPT.replace("description: Runs the deploy.", "description: ''"),
            // form discriminator and its exclusivity.
            PROMPT.replace("form: prompt", "form: shell"),
            PROMPT.replace("body_path: deploy-helper.skill.md\n", ""),
            PROMPT.to_owned() + "expands_to:\n  target: builtin:x\n",
            WRAPPER.replace(
                "expands_to:\n  target: builtin:spec-kitty.consolidate\n",
                "",
            ),
            WRAPPER.to_owned() + "body_path: body.md\n",
            // `expands_to` target/args guards.
            WRAPPER.replace("builtin:spec-kitty.consolidate", "spec-kitty.consolidate"),
            WRAPPER.replace("builtin:spec-kitty.consolidate", "'builtin:   '"),
            WRAPPER.replace("builtin:spec-kitty.consolidate", "cli:git status"),
            WRAPPER.replace("builtin:spec-kitty.consolidate", "cli:spec-kitty; rm -rf /"),
            WRAPPER.replace(
                "builtin:spec-kitty.consolidate",
                "cli:spec-kitty \"unclosed",
            ),
            WRAPPER.replace(
                "  target: builtin:spec-kitty.consolidate\n",
                "  target: cli:spec-kitty consolidate\n  args: $(rm -rf /)\n",
            ),
            // augmentation exclusivity/self-reference.
            rich.replace("overrides: other", "overrides: other\nenhances: base"),
            rich.replace("overrides: other", "overrides: triage"),
            // duplicate parameter names.
            rich.replace("  - name: scope\n", "  - name: scope\n  - name: scope\n"),
            // a non-optional nested model refuses an explicit null.
            PROMPT.to_owned() + "invocation: null\n",
            PROMPT.to_owned() + "parameters: null\n",
        ] {
            assert!(!admitted(ArtifactKind::Skill, &rejected), "{rejected}");
        }
    }
}
