//! Canonical Pydantic-equivalent profile body: validation, coercion, defaults.
//!
//! The reference loads an agent profile with `AgentProfile.model_validate`, so
//! the value the repository (and every consumer) sees is the *model*, not the
//! authored mapping. That has observable consequences the raw YAML does not:
//!
//! * declared defaults are present (`description: ""`, `routing-priority: 50`,
//!   the six empty `collaboration` lists, `specialization-context: null`, …);
//! * fields are dumped under their kebab-case serialization alias, so
//!   `preferred_model:` reads back as `model:` and `specialization_context:` as
//!   `specialization-context:`;
//! * Pydantic's lax scalars apply (`routing-priority: "57"`, `sentinel: "on"`,
//!   `max-concurrent-tasks: 3.0`), including its bounds and required fields;
//! * a nested model with `extra="ignore"` silently drops an undeclared key,
//!   while the top-level `AgentProfile` (`extra="forbid"`) rejects one.
//!
//! The **input** is always the canonical Kernel object graph
//! ([`crate::profile_schema::Values`], a borrowed view over
//! [`sk_kernel::python_yaml::NodeRef`]): every declared field is read by exact
//! key and coerced from its Python kind. Nothing is projected first, so a
//! Source-valid cycle in a value the model does not enter (an undeclared nested
//! key) is not a refusal, while a cycle or wrong shape in a *declared* field
//! fails here, at the model phase, with the model's own message.
//!
//! Two views are produced from one pass:
//!
//! * `full` — `model_dump(by_alias=True)`, every declared field, defaults
//!   filled. This is what `get()`/`resolve()` expose;
//! * `set` — `model_dump(by_alias=True, exclude_unset=True)`, only the fields
//!   the input mapping actually carried (recursively, including inside list
//!   items). `resolve_profile` merges this view up the lineage, so a parent's
//!   *default* never leaks into a child that did not author the field.
//!
//! Authored strings are written to those views in their lossless transport
//! spelling (plain text, or the injective marker for a value a `String` cannot
//! hold); this module never scalar-projects them, so a Source-valid surrogate
//! identity and a marker-shaped authored string survive validation exactly.
//!
//! # Layer overlay
//!
//! `_merge_profiles` deep-merges a higher layer's authored document into the
//! built-in *model dump* before validation. [`normalize_overlay`] is that read:
//! the two graph roots become one layered [`Values`] stack, so nested mappings
//! merge recursively and every other value is replaced — with no materialised
//! intermediate document (and therefore no cycle refusal).

use std::collections::BTreeMap;

use serde_yaml_ng::{Mapping, Value};
use sk_kernel::PythonString;

use crate::profile::{ProfileBody, lowercase_role};
use crate::profile_schema::Values;
use crate::profile_text;

/// One validated profile body in its two reference views.
pub(crate) struct NormalizedProfile {
    /// `model_dump(by_alias=True, exclude_unset=True)`.
    pub set: ProfileBody,
    /// `model_dump(by_alias=True)`.
    pub full: ProfileBody,
}

/// Every top-level field name and alias the reference model accepts.
///
/// `role` is the deprecated scalar form its `mode="before"` validator consumes;
/// `excluding` is a domain field on `AgentProfile` that the schema model does
/// not declare. Nothing else is accepted (`extra="forbid"`).
const TOP_LEVEL_KEYS: &[&str] = &[
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
    "applies_to_languages",
    "directive-references",
    "directive_references",
    "tactic-references",
    "tactic_references",
    "toolguide-references",
    "toolguide_references",
    "styleguide-references",
    "styleguide_references",
    "self-review-protocol",
    "self_review_protocol",
    "excluding",
    "specializes_from",
    "specializes-from",
    "enhances",
    "overrides",
];

/// Declared defaults and required fields, exactly as `profile.py` declares them.
const SCHEMA_VERSION_PATTERN: &str = "1.0";

/// Build the canonical views from one authored (possibly layered) document.
pub(crate) fn normalize(fields: &Values<'_>) -> Result<NormalizedProfile, String> {
    if fields.has_non_string_key() {
        return Err("profile keys must be strings".to_owned());
    }
    for key in fields.keys() {
        match key.to_scalar_string() {
            Ok(text) if TOP_LEVEL_KEYS.contains(&text.as_str()) => {}
            Ok(text) => return Err(format!("unknown profile field {text:?}")),
            Err(_) => return Err(format!("unknown profile field {}", key.repr())),
        }
    }

    let mut canon = Canon::new();
    let mut set = Mapping::new();
    let mut full = Mapping::new();

    // profile-id -----------------------------------------------------------
    let value = present(fields, "profile-id", "profile_id")?
        .ok_or_else(|| "'profile-id' is a required property".to_owned())?;
    let profile_id = canon.string(&value, "profile-id")?;
    record(
        &mut set,
        &mut full,
        "profile-id",
        Value::String(profile_id.clone()),
        Value::String(profile_id),
        true,
    );

    // name / description / purpose ----------------------------------------
    let value = present(fields, "name", "name")?
        .ok_or_else(|| "'name' is a required property".to_owned())?;
    let name = canon.string(&value, "name")?;
    record(
        &mut set,
        &mut full,
        "name",
        Value::String(name.clone()),
        Value::String(name),
        true,
    );

    match present(fields, "description", "description")? {
        Some(value) => {
            let text = canon.string(&value, "description")?;
            record(
                &mut set,
                &mut full,
                "description",
                Value::String(text.clone()),
                Value::String(text),
                true,
            );
        }
        None => {
            full.insert("description".into(), Value::String(String::new()));
        }
    }

    match present(fields, "schema-version", "schema_version")? {
        Some(value) => {
            let text = canon.string(&value, "schema-version")?;
            if text != "1.0" && text != "2" {
                return Err("'schema-version' must match ^(1\\.0|2)$".to_owned());
            }
            let value = Value::String(text);
            record(
                &mut set,
                &mut full,
                "schema-version",
                value.clone(),
                value,
                true,
            );
        }
        None => {
            full.insert(
                "schema-version".into(),
                Value::String(SCHEMA_VERSION_PATTERN.to_owned()),
            );
        }
    }

    // roles / deprecated scalar role --------------------------------------
    let roles = build_roles(fields, &mut canon)?;
    record(
        &mut set,
        &mut full,
        "roles",
        Value::Sequence(roles.clone()),
        Value::Sequence(roles),
        true,
    );

    match present(fields, "avatar-image", "avatar_image")? {
        Some(value) if value.is_null() => record(
            &mut set,
            &mut full,
            "avatar-image",
            Value::Null,
            Value::Null,
            true,
        ),
        Some(value) => {
            let text = canon.string(&value, "avatar-image")?;
            let value = Value::String(text);
            record(
                &mut set,
                &mut full,
                "avatar-image",
                value.clone(),
                value,
                true,
            );
        }
        None => {
            full.insert("avatar-image".into(), Value::Null);
        }
    }

    match present(fields, "capabilities", "capabilities")? {
        Some(value) => {
            let items = canon.string_list(&value, "capabilities")?;
            let value = Value::Sequence(items);
            record(
                &mut set,
                &mut full,
                "capabilities",
                value.clone(),
                value,
                true,
            );
        }
        None => {
            full.insert("capabilities".into(), Value::Sequence(Vec::new()));
        }
    }

    match present(fields, "routing-priority", "routing_priority")? {
        Some(value) => {
            let priority = canon.integer(&value, "routing-priority")?;
            let value = match priority {
                LaxInteger::Fits(priority) if (0..=100).contains(&priority) => {
                    Value::Number(priority.into())
                }
                _ => return Err("'routing-priority' must be between 0 and 100".to_owned()),
            };
            record(
                &mut set,
                &mut full,
                "routing-priority",
                value.clone(),
                value,
                true,
            );
        }
        None => {
            full.insert("routing-priority".into(), Value::Number(50.into()));
        }
    }

    match present(fields, "max-concurrent-tasks", "max_concurrent_tasks")? {
        Some(value) => {
            let tasks = canon.integer(&value, "max-concurrent-tasks")?;
            let value = match tasks {
                LaxInteger::Fits(tasks) if tasks > 0 => Value::Number(tasks.into()),
                LaxInteger::Huge {
                    negative: false,
                    digits,
                } => crate::profile_construct::big_integer_value(&digits),
                _ => return Err("'max-concurrent-tasks' must be greater than 0".to_owned()),
            };
            record(
                &mut set,
                &mut full,
                "max-concurrent-tasks",
                value.clone(),
                value,
                true,
            );
        }
        None => {
            full.insert("max-concurrent-tasks".into(), Value::Number(5.into()));
        }
    }

    match present(fields, "model", "preferred_model")? {
        Some(value) if value.is_null() => {
            record(&mut set, &mut full, "model", Value::Null, Value::Null, true)
        }
        Some(value) => {
            let text = canon.string(&value, "model")?;
            let value = Value::String(text);
            record(&mut set, &mut full, "model", value.clone(), value, true);
        }
        None => {
            full.insert("model".into(), Value::Null);
        }
    }

    match present(fields, "effort", "effort")? {
        Some(value) if value.is_null() => record(
            &mut set,
            &mut full,
            "effort",
            Value::Null,
            Value::Null,
            true,
        ),
        Some(value) => {
            let text = canon.string(&value, "effort")?;
            let value = Value::String(text);
            record(&mut set, &mut full, "effort", value.clone(), value, true);
        }
        None => {
            full.insert("effort".into(), Value::Null);
        }
    }

    let value = present(fields, "purpose", "purpose")?
        .ok_or_else(|| "'purpose' is a required property".to_owned())?;
    let purpose = canon.string(&value, "purpose")?;
    record(
        &mut set,
        &mut full,
        "purpose",
        Value::String(purpose.clone()),
        Value::String(purpose),
        true,
    );

    // specialization -------------------------------------------------------
    let value = present(fields, "specialization", "specialization")?
        .ok_or_else(|| "'specialization' is a required property".to_owned())?;
    let (set_value, full_value) = build_specialization(&mut canon, &value)?;
    record(
        &mut set,
        &mut full,
        "specialization",
        set_value,
        full_value,
        true,
    );

    // collaboration --------------------------------------------------------
    match present(fields, "collaboration", "collaboration")? {
        Some(value) => {
            let (set_value, full_value) = build_collaboration(&mut canon, &value)?;
            record(
                &mut set,
                &mut full,
                "collaboration",
                set_value,
                full_value,
                true,
            );
        }
        None => {
            full.insert("collaboration".into(), default_collaboration_full());
        }
    }

    // mode-defaults --------------------------------------------------------
    match present(fields, "mode-defaults", "mode_defaults")? {
        Some(value) => {
            let (set_items, full_items) = build_mode_defaults(&mut canon, &value)?;
            record(
                &mut set,
                &mut full,
                "mode-defaults",
                Value::Sequence(set_items),
                Value::Sequence(full_items),
                true,
            );
        }
        None => {
            full.insert("mode-defaults".into(), Value::Sequence(Vec::new()));
        }
    }

    match present(
        fields,
        "initialization-declaration",
        "initialization_declaration",
    )? {
        Some(value) => {
            let text = canon.string(&value, "initialization-declaration")?;
            let value = Value::String(text);
            record(
                &mut set,
                &mut full,
                "initialization-declaration",
                value.clone(),
                value,
                true,
            );
        }
        None => {
            full.insert(
                "initialization-declaration".into(),
                Value::String(String::new()),
            );
        }
    }

    match present(fields, "sentinel", "sentinel")? {
        Some(value) => {
            let flag = canon.boolean(&value, "sentinel")?;
            let value = Value::Bool(flag);
            record(&mut set, &mut full, "sentinel", value.clone(), value, true);
        }
        None => {
            full.insert("sentinel".into(), Value::Bool(false));
        }
    }

    // specialization-context ----------------------------------------------
    match present(fields, "specialization-context", "specialization_context")? {
        Some(value) if value.is_null() => record(
            &mut set,
            &mut full,
            "specialization-context",
            Value::Null,
            Value::Null,
            true,
        ),
        Some(value) => {
            let (set_value, full_value) = build_specialization_context(&mut canon, &value)?;
            record(
                &mut set,
                &mut full,
                "specialization-context",
                set_value,
                full_value,
                true,
            );
        }
        None => {
            full.insert("specialization-context".into(), Value::Null);
        }
    }

    match present(fields, "applies_to_languages", "applies_to_languages")? {
        Some(value) => {
            let items = canon.string_list(&value, "applies_to_languages")?;
            let value = Value::Sequence(items);
            record(
                &mut set,
                &mut full,
                "applies_to_languages",
                value.clone(),
                value,
                true,
            );
        }
        None => {
            full.insert("applies_to_languages".into(), Value::Sequence(Vec::new()));
        }
    }

    for (field, alias, name, required) in [
        (
            "directive-references",
            "directive-references",
            "directive_references",
            true,
        ),
        (
            "tactic-references",
            "tactic-references",
            "tactic_references",
            false,
        ),
        (
            "toolguide-references",
            "toolguide-references",
            "toolguide_references",
            false,
        ),
        (
            "styleguide-references",
            "styleguide-references",
            "styleguide_references",
            false,
        ),
    ] {
        match present(fields, alias, name)? {
            Some(value) => {
                let (set_items, full_items) =
                    build_references(&mut canon, &value, field, required)?;
                record(
                    &mut set,
                    &mut full,
                    field,
                    Value::Sequence(set_items),
                    Value::Sequence(full_items),
                    true,
                );
            }
            None => {
                full.insert(field.into(), Value::Sequence(Vec::new()));
            }
        }
    }

    match present(fields, "self-review-protocol", "self_review_protocol")? {
        Some(value) if value.is_null() => record(
            &mut set,
            &mut full,
            "self-review-protocol",
            Value::Null,
            Value::Null,
            true,
        ),
        Some(value) => {
            let (set_value, full_value) = build_self_review(&mut canon, &value)?;
            record(
                &mut set,
                &mut full,
                "self-review-protocol",
                set_value,
                full_value,
                true,
            );
        }
        None => {
            full.insert("self-review-protocol".into(), Value::Null);
        }
    }

    match present(fields, "excluding", "excluding")? {
        Some(value) if value.is_null() => record(
            &mut set,
            &mut full,
            "excluding",
            Value::Null,
            Value::Null,
            true,
        ),
        Some(value) => {
            let excluding = canon.excluding(&value)?;
            record(
                &mut set,
                &mut full,
                "excluding",
                excluding.clone(),
                excluding,
                true,
            );
        }
        None => {
            full.insert("excluding".into(), Value::Null);
        }
    }

    let markers = canon.finish();
    Ok(NormalizedProfile {
        set: ProfileBody::from_parts(set, markers.clone()),
        full: ProfileBody::from_parts(full, markers),
    })
}

/// Normalize a body this crate already constructed (a model document).
pub(crate) fn normalize_body(body: &ProfileBody) -> Result<NormalizedProfile, String> {
    let root = body
        .root()
        .ok_or_else(|| "profile body is not representable as a canonical graph".to_owned())?;
    normalize(&Values::single(root))
}

/// Normalize `overlay` (a higher layer's authored document) deep-merged onto
/// `base` (the built-in layer's model dump), as `_merge_profiles` does.
pub(crate) fn normalize_overlay(
    overlay: &Values<'_>,
    base: &ProfileBody,
) -> Result<NormalizedProfile, String> {
    let base_root = base
        .root()
        .ok_or_else(|| "profile body is not representable as a canonical graph".to_owned())?;
    let merged = Values::merged(vec![overlay.node(), base_root])
        .ok_or_else(|| "'profile' must be a mapping".to_owned())?;
    normalize(&merged)
}

/// The reference `AgentProfile.roles` rules, including the scalar `role:` form.
fn build_roles(fields: &Values<'_>, canon: &mut Canon) -> Result<Vec<Value>, String> {
    let scalar_present = fields.contains_key("role");
    let roles_value = fields.field("roles");
    match (scalar_present, roles_value) {
        (true, Some(_)) => Err(
            "declares both the deprecated scalar 'role:' and the canonical 'roles:'; keep 'roles:'"
                .to_owned(),
        ),
        (true, None) => {
            let value = fields.field("role").expect("role presence was checked");
            let exact = canon.exact_value(&value, "role")?;
            let lowered = lowercase_role(&exact);
            if lowered.is_empty() {
                return Err("'role' must be a non-empty string".to_owned());
            }
            Ok(vec![Value::String(canon.spell(&lowered))])
        }
        (false, Some(value)) => {
            let items = value
                .sequence()
                .ok_or_else(|| "'roles' must be a list".to_owned())?;
            if items.is_empty() {
                return Err("'roles' must contain at least one role".to_owned());
            }
            let mut roles = Vec::with_capacity(items.len());
            for item in &items {
                let exact = canon.exact_value(item, "roles")?;
                if exact.is_empty() {
                    return Err("'roles' entries must be non-empty strings".to_owned());
                }
                let lowered = lowercase_role(&exact);
                roles.push(Value::String(canon.spell(&lowered)));
            }
            Ok(roles)
        }
        (false, None) => Err("'roles' is a required property".to_owned()),
    }
}

fn build_specialization(canon: &mut Canon, value: &Values<'_>) -> Result<(Value, Value), String> {
    if !value.is_mapping() {
        return Err("'specialization' must be a mapping".to_owned());
    }
    let primary = value
        .field("primary-focus")
        .or_else(|| value.field("primary_focus"))
        .ok_or_else(|| "'specialization.primary-focus' is a required property".to_owned())?;
    let primary = canon.string(&primary, "specialization.primary-focus")?;
    let mut set = Mapping::new();
    let mut full = Mapping::new();
    set.insert("primary-focus".into(), Value::String(primary.clone()));
    full.insert("primary-focus".into(), Value::String(primary));
    for (alias, name) in [
        ("secondary-awareness", "secondary_awareness"),
        ("avoidance-boundary", "avoidance_boundary"),
        ("success-definition", "success_definition"),
    ] {
        match value.field(alias).or_else(|| value.field(name)) {
            Some(field) => {
                let text = canon.string(&field, &format!("specialization.{alias}"))?;
                set.insert(alias.into(), Value::String(text.clone()));
                full.insert(alias.into(), Value::String(text));
            }
            None => {
                full.insert(alias.into(), Value::String(String::new()));
            }
        }
    }
    Ok((Value::Mapping(set), Value::Mapping(full)))
}

fn build_collaboration(canon: &mut Canon, value: &Values<'_>) -> Result<(Value, Value), String> {
    if !value.is_mapping() {
        return Err("'collaboration' must be a mapping".to_owned());
    }
    build_string_list_sections(
        canon,
        value,
        "collaboration",
        &[
            ("handoff-to", "handoff_to"),
            ("handoff-from", "handoff_from"),
            ("works-with", "works_with"),
            ("output-artifacts", "output_artifacts"),
            ("operating-procedures", "operating_procedures"),
            ("canonical-verbs", "canonical_verbs"),
        ],
    )
}

fn build_specialization_context(
    canon: &mut Canon,
    value: &Values<'_>,
) -> Result<(Value, Value), String> {
    if !value.is_mapping() {
        return Err("'specialization-context' must be a mapping".to_owned());
    }
    build_string_list_sections(
        canon,
        value,
        "specialization-context",
        &[
            ("languages", "languages"),
            ("frameworks", "frameworks"),
            ("file-patterns", "file_patterns"),
            ("domain-keywords", "domain_keywords"),
            ("writing-style", "writing_style"),
            ("complexity-preference", "complexity_preference"),
        ],
    )
}

fn build_string_list_sections(
    canon: &mut Canon,
    mapping: &Values<'_>,
    section: &str,
    fields: &[(&str, &str)],
) -> Result<(Value, Value), String> {
    let mut set = Mapping::new();
    let mut full = Mapping::new();
    for (alias, name) in fields {
        match mapping.field(alias).or_else(|| mapping.field(name)) {
            Some(field) => {
                let items = canon.string_list(&field, &format!("{section}.{alias}"))?;
                let value = Value::Sequence(items);
                set.insert((*alias).into(), value.clone());
                full.insert((*alias).into(), value);
            }
            None => {
                full.insert((*alias).into(), Value::Sequence(Vec::new()));
            }
        }
    }
    Ok((Value::Mapping(set), Value::Mapping(full)))
}

fn default_collaboration_full() -> Value {
    let mut full = Mapping::new();
    for field in [
        "handoff-to",
        "handoff-from",
        "works-with",
        "output-artifacts",
        "operating-procedures",
        "canonical-verbs",
    ] {
        full.insert(field.into(), Value::Sequence(Vec::new()));
    }
    Value::Mapping(full)
}

fn build_mode_defaults(
    canon: &mut Canon,
    value: &Values<'_>,
) -> Result<(Vec<Value>, Vec<Value>), String> {
    let items = value
        .sequence()
        .ok_or_else(|| "'mode-defaults' must be a list".to_owned())?;
    let mut set = Vec::with_capacity(items.len());
    let mut full = Vec::with_capacity(items.len());
    for (position, item) in items.iter().enumerate() {
        if !item.is_mapping() {
            return Err(format!("'mode-defaults.{position}' must be a mapping"));
        }
        let mut set_item = Mapping::new();
        let mut full_item = Mapping::new();
        for field in ["mode", "description", "use-case"] {
            let name = field.replace('-', "_");
            let value = item
                .field(field)
                .or_else(|| item.field(name.as_str()))
                .ok_or_else(|| {
                    format!("'mode-defaults.{position}.{field}' is a required property")
                })?;
            let text = canon.string(&value, &format!("mode-defaults.{position}.{field}"))?;
            set_item.insert(field.into(), Value::String(text.clone()));
            full_item.insert(field.into(), Value::String(text));
        }
        set.push(Value::Mapping(set_item));
        full.push(Value::Mapping(full_item));
    }
    Ok((set, full))
}

fn build_references(
    canon: &mut Canon,
    value: &Values<'_>,
    field: &str,
    directive: bool,
) -> Result<(Vec<Value>, Vec<Value>), String> {
    let items = value
        .sequence()
        .ok_or_else(|| format!("'{field}' must be a list"))?;
    let required: &[&str] = if directive {
        &["code", "name", "rationale"]
    } else {
        &["id", "rationale"]
    };
    let mut set = Vec::with_capacity(items.len());
    let mut full = Vec::with_capacity(items.len());
    for (position, item) in items.iter().enumerate() {
        if !item.is_mapping() {
            return Err(format!("'{field}.{position}' must be a mapping"));
        }
        let mut set_item = Mapping::new();
        let mut full_item = Mapping::new();
        for key in required {
            let value = item
                .field(key)
                .ok_or_else(|| format!("'{field}.{position}.{key}' is a required property"))?;
            let text = canon.string(&value, &format!("{field}.{position}.{key}"))?;
            set_item.insert((*key).into(), Value::String(text.clone()));
            full_item.insert((*key).into(), Value::String(text));
        }
        set.push(Value::Mapping(set_item));
        full.push(Value::Mapping(full_item));
    }
    Ok((set, full))
}

fn build_self_review(canon: &mut Canon, value: &Values<'_>) -> Result<(Value, Value), String> {
    if !value.is_mapping() {
        return Err("'self-review-protocol' must be a mapping".to_owned());
    }
    let mut set = Mapping::new();
    let mut full = Mapping::new();
    match value.field("steps") {
        Some(steps) => {
            let items = steps
                .sequence()
                .ok_or_else(|| "'self-review-protocol.steps' must be a list".to_owned())?;
            let mut set_steps = Vec::with_capacity(items.len());
            let mut full_steps = Vec::with_capacity(items.len());
            for (position, item) in items.iter().enumerate() {
                if !item.is_mapping() {
                    return Err(format!(
                        "'self-review-protocol.steps.{position}' must be a mapping"
                    ));
                }
                let mut set_step = Mapping::new();
                let mut full_step = Mapping::new();
                for key in ["name", "gate"] {
                    let value = item.field(key).ok_or_else(|| {
                        format!(
                            "'self-review-protocol.steps.{position}.{key}' is a required property"
                        )
                    })?;
                    let text = canon.string(
                        &value,
                        &format!("self-review-protocol.steps.{position}.{key}"),
                    )?;
                    set_step.insert(key.into(), Value::String(text.clone()));
                    full_step.insert(key.into(), Value::String(text));
                }
                match item.field("command") {
                    Some(command) if command.is_null() => {
                        set_step.insert("command".into(), Value::Null);
                        full_step.insert("command".into(), Value::Null);
                    }
                    Some(command) => {
                        let text = canon.string(
                            &command,
                            &format!("self-review-protocol.steps.{position}.command"),
                        )?;
                        set_step.insert("command".into(), Value::String(text.clone()));
                        full_step.insert("command".into(), Value::String(text));
                    }
                    None => {
                        full_step.insert("command".into(), Value::Null);
                    }
                }
                set_steps.push(Value::Mapping(set_step));
                full_steps.push(Value::Mapping(full_step));
            }
            set.insert("steps".into(), Value::Sequence(set_steps));
            full.insert("steps".into(), Value::Sequence(full_steps));
        }
        None => {
            full.insert("steps".into(), Value::Sequence(Vec::new()));
        }
    }
    Ok((Value::Mapping(set), Value::Mapping(full)))
}

/// Record a field in both views. `present` false means the input did not carry
/// the field, so only the full (defaulted) view gets it.
fn record(
    set: &mut Mapping,
    full: &mut Mapping,
    key: &str,
    set_value: Value,
    full_value: Value,
    present: bool,
) {
    full.insert(key.into(), full_value);
    if present {
        set.insert(key.into(), set_value);
    }
}

/// Find a field by alias or Python name, refusing the both-spellings case the
/// reference's `populate_by_name` rejects.
fn present<'g>(fields: &Values<'g>, alias: &str, name: &str) -> Result<Option<Values<'g>>, String> {
    let alias_value = fields.field(alias);
    let name_value = if name == alias {
        None
    } else {
        fields.field(name)
    };
    match (alias_value, name_value) {
        (Some(_), Some(_)) => Err(format!(
            "cannot specify both {alias:?} and {name:?} for the same field"
        )),
        (Some(value), None) | (None, Some(value)) => Ok(Some(value)),
        (None, None) => Ok(None),
    }
}

/// Exact-string canonicalizer: authored strings travel as their transport
/// spelling; a transformed string (a lowercased role) is re-encoded with the
/// same injective marker scheme, so it can never collide with authored data.
///
/// The exact value always comes from the graph, so no marker table is consulted
/// on the way in: a marker is a pure function of the exact code points.
struct Canon {
    added: BTreeMap<String, PythonString>,
}

impl Canon {
    fn new() -> Self {
        Self {
            added: BTreeMap::new(),
        }
    }

    fn spell(&mut self, exact: &PythonString) -> String {
        match exact.to_scalar_string() {
            Ok(text) if !profile_text::is_marker_text(&text) => text,
            _ => {
                let marker = profile_text::marker_for_exact(exact);
                self.added.insert(marker.clone(), exact.clone());
                marker
            }
        }
    }

    /// The exact Python value of a `str` field.
    ///
    /// The reference model's `str` has already been constructed from the YAML
    /// node, so a `!!binary` value is **bytes** here and Pydantic's lax `str`
    /// decodes them as UTF-8; a timestamp is a `date`/`datetime` object and is
    /// refused with the reference's `string_type` outcome.
    fn exact_value(&mut self, value: &Values<'_>, field: &str) -> Result<PythonString, String> {
        if let Some(exact) = value.as_str() {
            return Ok(exact.clone());
        }
        if let Some(bytes) = value.as_bytes() {
            let text = std::str::from_utf8(bytes).map_err(|_| {
                format!(
                    "{field}: Input should be a valid string, unable to parse raw data as a \
                     unicode string"
                )
            })?;
            return Ok(PythonString::from_scalars(text));
        }
        // A constructed timestamp is a Python `date`/`datetime` object, which the
        // reference model's `str` rejects with the same `string_type` outcome as
        // any other non-string.
        Err(format!("{field}: Input should be a valid string"))
    }

    /// A `str` field's transport spelling (decoded bytes are re-spelled).
    fn string(&mut self, value: &Values<'_>, field: &str) -> Result<String, String> {
        let exact = self.exact_value(value, field)?;
        Ok(self.spell(&exact))
    }

    /// A `list[str]` (or union list branch) field's items.
    ///
    /// A constructed `!!set` is a real Python set and Pydantic's lax `list[...]`
    /// accepts it, so it takes the list branch exactly like the reference.
    fn string_list(&mut self, value: &Values<'_>, field: &str) -> Result<Vec<Value>, String> {
        let items = value
            .sequence()
            .ok_or_else(|| format!("{field}: expected a list"))?;
        let mut strings = Vec::with_capacity(items.len());
        for item in &items {
            strings.push(Value::String(self.string(item, field)?));
        }
        Ok(strings)
    }

    /// Pydantic v2's lax integer coercion for an already constructed value.
    fn integer(&mut self, value: &Values<'_>, field: &str) -> Result<LaxInteger, String> {
        if let Some(text) = value.as_int_text() {
            return Ok(parse_decimal_integer(text));
        }
        if let Some(flag) = value.as_bool() {
            return Ok(LaxInteger::Fits(i64::from(flag)));
        }
        if let Some(float) = value.as_float() {
            return float_integer(float, field);
        }
        if let Some(text) = value.as_str() {
            let scalar = text.to_scalar_string().map_err(|_| {
                format!(
                    "{field}: expected an integer, unable to parse {}",
                    text.repr()
                )
            })?;
            return parse_lax_integer(&scalar).ok_or_else(|| {
                format!("{field}: expected an integer, unable to parse {scalar:?}")
            });
        }
        if let Some(bytes) = value.as_bytes() {
            let text = std::str::from_utf8(bytes).map_err(|_| {
                format!(
                    "{field}: expected an integer, unable to parse raw data as a unicode string"
                )
            })?;
            return parse_lax_integer(text)
                .ok_or_else(|| format!("{field}: expected an integer, unable to parse {text:?}"));
        }
        if value.is_timestamp() {
            return Err(format!("{field}: Input should be a valid integer"));
        }
        Err(format!("{field}: expected an integer"))
    }

    /// Pydantic v2's lax boolean coercion for an already constructed value.
    fn boolean(&mut self, value: &Values<'_>, field: &str) -> Result<bool, String> {
        if let Some(flag) = value.as_bool() {
            return Ok(flag);
        }
        if let Some(text) = value.as_int_text() {
            return match text {
                "0" => Ok(false),
                "1" => Ok(true),
                _ => Err(format!("{field}: expected a boolean")),
            };
        }
        if let Some(float) = value.as_float() {
            if float == 0.0 {
                return Ok(false);
            }
            if float == 1.0 {
                return Ok(true);
            }
            return Err(format!("{field}: expected a boolean"));
        }
        let text = if let Some(text) = value.as_str() {
            text.to_scalar_string()
                .map_err(|_| format!("{field}: expected a boolean"))?
        } else if let Some(bytes) = value.as_bytes() {
            std::str::from_utf8(bytes)
                .map_err(|_| format!("{field}: expected a boolean"))?
                .to_owned()
        } else {
            return Err(format!("{field}: expected a boolean"));
        };
        let lowered = text.to_lowercase();
        match lowered.as_str() {
            "true" | "yes" | "on" | "1" | "t" | "y" => Ok(true),
            "false" | "no" | "off" | "0" | "f" | "n" => Ok(false),
            _ => Err(format!("{field}: expected a boolean")),
        }
    }

    /// The `excluding` union: `list[str] | dict[str, list[str]] | None`.
    fn excluding(&mut self, value: &Values<'_>) -> Result<Value, String> {
        if value.is_mapping() {
            let mut fields = Mapping::new();
            for key in value.keys() {
                let key = key
                    .to_scalar_string()
                    .map_err(|_| "'excluding' keys must be strings".to_owned())?;
                let item = value
                    .field(&key)
                    .ok_or_else(|| "'excluding' keys must be strings".to_owned())?;
                let items = self.string_list(&item, &format!("excluding.{key}"))?;
                fields.insert(key.into(), Value::Sequence(items));
            }
            return Ok(Value::Mapping(fields));
        }
        if let Some(items) = value.sequence() {
            let mut fields = Vec::with_capacity(items.len());
            for item in &items {
                fields.push(Value::String(self.string(item, "excluding")?));
            }
            return Ok(Value::Sequence(fields));
        }
        Err("'excluding' must be a list or a mapping of lists".to_owned())
    }

    fn finish(self) -> BTreeMap<String, PythonString> {
        self.added
    }
}

/// A Pydantic `int` field value: an `i64` or a Python big integer.
///
/// Python's `int` is arbitrary precision. Only `max-concurrent-tasks` can
/// accept a magnitude outside `i64` (it has no upper bound), so a big value
/// keeps its exact decimal digits instead of being rounded or refused.
enum LaxInteger {
    Fits(i64),
    Huge { negative: bool, digits: String },
}

/// An already-constructed Python `int`, from its canonical decimal text.
fn parse_decimal_integer(text: &str) -> LaxInteger {
    match text.parse::<i64>() {
        Ok(value) => LaxInteger::Fits(value),
        Err(_) => LaxInteger::Huge {
            negative: text.starts_with('-'),
            digits: text.to_owned(),
        },
    }
}

/// Pydantic's lax `int` coercion of a finite Python `float`.
fn float_integer(float: f64, field: &str) -> Result<LaxInteger, String> {
    if !float.is_finite() {
        return Err(format!("{field}: expected a finite number"));
    }
    if float.fract() != 0.0 {
        return Err(format!(
            "{field}: expected an integer, got a fractional number"
        ));
    }
    if float > i64::MAX as f64 {
        // Python's `int()` keeps the exact integer value of a large finite
        // float; `{:.0}` prints the same exact decimal.
        return Ok(LaxInteger::Huge {
            negative: false,
            digits: format!("{float:.0}"),
        });
    }
    if float < i64::MIN as f64 {
        return Ok(LaxInteger::Huge {
            negative: true,
            digits: format!("{float:.0}"),
        });
    }
    Ok(LaxInteger::Fits(float as i64))
}

/// Pydantic v2's lax integer coercion of a **string**.
///
/// Booleans, integral floats and trimmed numeric strings with `_` digit
/// separators and an optional `.0` fraction are accepted; scientific notation
/// is not (`int()` semantics). A decimal magnitude outside `i64` stays exact.
fn parse_lax_integer(text: &str) -> Option<LaxInteger> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (sign, body) = match trimmed.as_bytes().first() {
        Some(b'+') => (1i64, &trimmed[1..]),
        Some(b'-') => (-1i64, &trimmed[1..]),
        _ => (1i64, trimmed),
    };
    let (digits, exponent) = body
        .split_once(['.', 'e', 'E'])
        .map_or((body, None), |(digits, rest)| {
            (digits, Some((body[digits.len()..].chars().next(), rest)))
        });
    // Only a `.0…` fraction is integral; scientific notation is refused.
    if let Some((separator, rest)) = exponent
        && (separator != Some('.') || !rest.chars().all(|c| c == '0'))
    {
        return None;
    }
    if digits.is_empty() {
        return None;
    }
    let mut value = String::with_capacity(digits.len());
    let mut previous_underscore = true;
    for character in digits.chars() {
        if character == '_' {
            if previous_underscore {
                return None;
            }
            previous_underscore = true;
            continue;
        }
        if !character.is_ascii_digit() {
            return None;
        }
        previous_underscore = false;
        value.push(character);
    }
    if previous_underscore {
        return None;
    }
    if let Ok(parsed) = value.parse::<i64>() {
        return Some(LaxInteger::Fits(sign * parsed));
    }
    let significand = value.trim_start_matches('0');
    let digits = if significand.is_empty() {
        "0"
    } else {
        significand
    };
    let negative = sign < 0 && digits != "0";
    Some(LaxInteger::Huge {
        negative,
        digits: if negative {
            format!("-{digits}")
        } else {
            digits.to_owned()
        },
    })
}
