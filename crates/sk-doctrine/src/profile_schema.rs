//! The reference's **JSON-schema** error list for an authored agent profile.
//!
//! `charter/offering/agent_profiles/validation.py::validate_agent_profile_yaml`
//! runs `jsonschema.Draft7Validator` over `schemas/agent-profile.schema.yaml`
//! and renders each error as `"<field path>: <validator message>"`, where the
//! path is the dotted `error.path` (or `root` at the document root). The
//! repository uses that list — not the Pydantic message — as a skipped
//! profile's `error_summary` whenever it is non-empty:
//!
//! ```text
//! root: 'name' is a required property; root: {'profile-id': 'bad'} is not valid under any of the given schemas
//! roles: 7 is not of type 'array'
//! max-concurrent-tasks: 0 is less than the minimum of 1
//! ```
//!
//! This module reproduces that rendering from the **canonical object graph** the
//! loader built, so a skip diagnostic reads like the reference instead of like a
//! native error. It is a diagnostic projection, not a second validation
//! authority: whether a profile loads is still decided by
//! [`crate::profile_model::normalize`]. The authored schema is a closed
//! document (26 top-level properties, `additionalProperties: false`, one
//! `anyOf` over `role`/`roles`), so the messages are enumerable; every value is
//! rendered by the shared Kernel `repr()` of its graph node (CPython's own
//! spelling, exact code points, structural cycle markers).
//!
//! Only the fields the schema declares are *entered*: a `$ref` target is read
//! property by property, so a Source-valid cycle in an undeclared nested value
//! is invisible here — it is not a schema error in the reference either.
//!
//! Order matches `jsonschema`'s iteration of the schema mapping, which follows
//! the authored file: `additionalProperties`, `required`, `anyOf`, then
//! `properties` in schema order (recursing into each `$ref` definition).

mod graph;

pub(crate) use graph::Values;

use sk_kernel::PythonString;

use crate::profile_schema::graph::key_is;

/// The two `pattern` constraints the authored schema declares.
#[derive(Clone, Copy)]
enum Pattern {
    KebabId,
    SchemaVersion,
}

/// One top-level property's schema shape.
#[derive(Clone, Copy)]
enum Shape {
    /// `type: string`, with an optional `minLength` and `pattern`.
    Text {
        min_length: Option<usize>,
        pattern: Option<Pattern>,
    },
    /// `type: array` of `type: string` items, with an optional `minItems`.
    TextList { min_items: Option<usize> },
    /// `type: boolean`.
    Flag,
    /// `type: integer` with optional inclusive bounds.
    Integer {
        minimum: Option<i64>,
        maximum: Option<i64>,
    },
    /// A `$ref` into `definitions`.
    Nested(Definition),
}

/// A `definitions` entry this schema uses.
#[derive(Clone, Copy)]
enum Definition {
    Specialization,
    Collaboration,
    SpecializationContext,
    ModeDefault,
    DirectiveReference,
    TacticReference,
    ToolguideReference,
    StyleguideReference,
    SelfReviewProtocol,
    SelfReviewStep,
}

const TEXT: Shape = Shape::Text {
    min_length: None,
    pattern: None,
};
const NON_EMPTY_TEXT: Shape = Shape::Text {
    min_length: Some(1),
    pattern: None,
};
const TEXT_LIST: Shape = Shape::TextList { min_items: None };

/// The top-level properties, in the authored schema's order.
const PROPERTIES: &[(&str, Shape)] = &[
    (
        "profile-id",
        Shape::Text {
            min_length: None,
            pattern: Some(Pattern::KebabId),
        },
    ),
    ("name", NON_EMPTY_TEXT),
    ("description", TEXT),
    (
        "schema-version",
        Shape::Text {
            min_length: None,
            pattern: Some(Pattern::SchemaVersion),
        },
    ),
    ("purpose", NON_EMPTY_TEXT),
    ("role", TEXT),
    ("roles", Shape::TextList { min_items: Some(1) }),
    ("avatar-image", TEXT),
    ("sentinel", Shape::Flag),
    ("tags", TEXT_LIST),
    ("capabilities", TEXT_LIST),
    (
        "routing-priority",
        Shape::Integer {
            minimum: Some(0),
            maximum: Some(100),
        },
    ),
    (
        "max-concurrent-tasks",
        Shape::Integer {
            minimum: Some(1),
            maximum: None,
        },
    ),
    ("applies_to_languages", TEXT_LIST),
    ("model", TEXT),
    ("effort", TEXT),
    ("specialization", Shape::Nested(Definition::Specialization)),
    ("collaboration", Shape::Nested(Definition::Collaboration)),
    ("mode-defaults", Shape::Nested(Definition::ModeDefault)),
    ("initialization-declaration", TEXT),
    (
        "specialization-context",
        Shape::Nested(Definition::SpecializationContext),
    ),
    (
        "directive-references",
        Shape::Nested(Definition::DirectiveReference),
    ),
    (
        "tactic-references",
        Shape::Nested(Definition::TacticReference),
    ),
    (
        "toolguide-references",
        Shape::Nested(Definition::ToolguideReference),
    ),
    (
        "styleguide-references",
        Shape::Nested(Definition::StyleguideReference),
    ),
    (
        "self-review-protocol",
        Shape::Nested(Definition::SelfReviewProtocol),
    ),
];

/// The root `required:` list, in authored order.
const REQUIRED: [&str; 4] = ["profile-id", "name", "purpose", "specialization"];

/// The reference's skip-summary error list for the document `fields` reads.
///
/// Empty when the document satisfies the authored schema. An empty list means
/// "the schema has nothing to say"; the caller then falls back to the model
/// error, exactly as `repository.py` does.
pub(crate) fn agent_profile_schema_errors(fields: &Values<'_>) -> Vec<String> {
    let mut errors: Vec<String> = Vec::new();

    // additionalProperties: false
    push_additional_properties("root", fields, &[], &mut errors);
    // required
    for field in REQUIRED {
        if !fields.contains_key(field) {
            errors.push(required_message("root", field));
        }
    }
    // anyOf: [{required: [role]}, {required: [roles]}]
    if !fields.contains_key("role") && !fields.contains_key("roles") {
        errors.push(format!(
            "root: {} is not valid under any of the given schemas",
            fields.repr()
        ));
    }
    for (name, shape) in PROPERTIES {
        if let Some(value) = fields.field(name) {
            validate_shape(name, *shape, &value, &mut errors);
        }
    }
    errors
}

fn required_message(path: &str, field: &str) -> String {
    format!("{path}: {} is a required property", repr_str(field))
}

fn repr_str(text: &str) -> String {
    PythonString::from_scalars(text).repr()
}

/// `Additional properties are not allowed (...)` for one mapping's extras.
///
/// An empty `allowed` means "only the schema's top-level properties".
fn push_additional_properties(
    path: &str,
    fields: &Values<'_>,
    allowed: &[&str],
    errors: &mut Vec<String>,
) {
    let property_names: Vec<&str> = if allowed.is_empty() {
        PROPERTIES.iter().map(|(name, _)| *name).collect()
    } else {
        allowed.to_vec()
    };
    let mut extras: Vec<PythonString> = fields
        .keys()
        .into_iter()
        .filter(|key| !property_names.iter().any(|name| key_is(key, name)))
        .collect();
    extras.sort();
    if extras.is_empty() {
        return;
    }
    let joined = extras
        .iter()
        .map(PythonString::repr)
        .collect::<Vec<_>>()
        .join(", ");
    let verb = if extras.len() == 1 { "was" } else { "were" };
    errors.push(format!(
        "{path}: Additional properties are not allowed ({joined} {verb} unexpected)"
    ));
}

/// Validate one schema property, pushing reference-worded errors.
fn validate_shape(field: &str, shape: Shape, value: &Values<'_>, errors: &mut Vec<String>) {
    match shape {
        Shape::Text {
            min_length,
            pattern,
        } => {
            let Some(exact) = value.as_str() else {
                errors.push(type_error(field, value, "string"));
                return;
            };
            if let Some(minimum) = min_length
                && exact.codepoints().len() < minimum
            {
                errors.push(format!("{field}: {} is too short", exact.repr()));
            }
            if let Some(pattern) = pattern {
                let scalar = exact.to_scalar_string().unwrap_or_default();
                let (matched, spelling) = match pattern {
                    Pattern::KebabId => (matches_kebab_case(&scalar), "^[a-z][a-z0-9-]*$"),
                    Pattern::SchemaVersion => {
                        (matches!(scalar.as_str(), "1.0" | "2"), "^(1\\.0|2)$")
                    }
                };
                if !matched {
                    errors.push(format!(
                        "{field}: {} does not match {}",
                        exact.repr(),
                        repr_str(spelling)
                    ));
                }
            }
        }
        Shape::TextList { min_items } => {
            let Some(items) = value.sequence() else {
                errors.push(type_error(field, value, "array"));
                return;
            };
            if let Some(minimum) = min_items
                && items.len() < minimum
            {
                let message = if minimum == 1 {
                    "should be non-empty"
                } else {
                    "is too short"
                };
                errors.push(format!("{field}: {} {message}", value.repr()));
            }
            for (position, item) in items.iter().enumerate() {
                if item.as_str().is_none() {
                    errors.push(type_error(&format!("{field}.{position}"), item, "string"));
                }
            }
        }
        Shape::Flag => {
            if value.as_bool().is_none() {
                errors.push(type_error(field, value, "boolean"));
            }
        }
        Shape::Integer { minimum, maximum } => {
            // An exact big integer keeps its digits in the graph; its sign
            // satisfies the schema's bounds without re-projecting them.
            let Some(text) = value.as_int_text() else {
                errors.push(type_error(field, value, "integer"));
                return;
            };
            let Ok(integer) = text.parse::<i64>() else {
                return;
            };
            if let Some(minimum) = minimum
                && integer < minimum
            {
                errors.push(format!(
                    "{field}: {} is less than the minimum of {minimum}",
                    value.repr()
                ));
            }
            if let Some(maximum) = maximum
                && integer > maximum
            {
                errors.push(format!(
                    "{field}: {} is greater than the maximum of {maximum}",
                    value.repr()
                ));
            }
        }
        Shape::Nested(definition) => validate_definition(field, definition, value, errors),
    }
}

/// `%r is not of type %r` with the reference's path prefix.
fn type_error(field: &str, value: &Values<'_>, expected: &str) -> String {
    format!(
        "{field}: {} is not of type {}",
        value.repr(),
        repr_str(expected)
    )
}

fn matches_kebab_case(text: &str) -> bool {
    let mut characters = text.chars();
    match characters.next() {
        Some(first) if first.is_ascii_lowercase() => characters.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        }),
        _ => false,
    }
}

/// The `definitions` entry's `(allowed properties, required properties)`.
fn object_shape(definition: Definition) -> (&'static [&'static str], &'static [&'static str]) {
    match definition {
        Definition::Specialization => (
            &[
                "primary-focus",
                "secondary-awareness",
                "avoidance-boundary",
                "success-definition",
            ],
            &["primary-focus"],
        ),
        Definition::Collaboration => (
            &[
                "handoff-to",
                "handoff-from",
                "works-with",
                "output-artifacts",
                "operating-procedures",
                "canonical-verbs",
            ],
            &[],
        ),
        Definition::SpecializationContext => (
            &[
                "languages",
                "frameworks",
                "file-patterns",
                "domain-keywords",
                "writing-style",
                "complexity-preference",
            ],
            &[],
        ),
        Definition::ModeDefault => (
            &["mode", "description", "use-case"],
            &["mode", "description", "use-case"],
        ),
        Definition::DirectiveReference => (
            &["code", "name", "rationale"],
            &["code", "name", "rationale"],
        ),
        Definition::TacticReference
        | Definition::ToolguideReference
        | Definition::StyleguideReference => (&["id", "rationale"], &["id", "rationale"]),
        Definition::SelfReviewProtocol => (&["steps"], &[]),
        Definition::SelfReviewStep => (&["name", "command", "gate"], &["name", "gate"]),
    }
}

/// Whether the definition's properties are `array` (not `string`).
fn property_lists(definition: Definition) -> bool {
    matches!(
        definition,
        Definition::Collaboration | Definition::SpecializationContext
    )
}

fn validate_definition(
    field: &str,
    definition: Definition,
    value: &Values<'_>,
    errors: &mut Vec<String>,
) {
    // Array-valued definitions are reached through `.items.$ref`.
    let array_shaped = matches!(
        definition,
        Definition::ModeDefault
            | Definition::DirectiveReference
            | Definition::TacticReference
            | Definition::ToolguideReference
            | Definition::StyleguideReference
    );
    if array_shaped {
        let Some(items) = value.sequence() else {
            errors.push(type_error(field, value, "array"));
            return;
        };
        for (position, item) in items.iter().enumerate() {
            validate_object(&format!("{field}.{position}"), definition, item, errors);
        }
        return;
    }
    validate_object(field, definition, value, errors);
}

/// Validate one object-shaped definition instance.
fn validate_object(
    path: &str,
    definition: Definition,
    value: &Values<'_>,
    errors: &mut Vec<String>,
) {
    if !value.is_mapping() {
        errors.push(type_error(path, value, "object"));
        return;
    }
    let (allowed, required) = object_shape(definition);
    push_additional_properties(path, value, allowed, errors);
    for field in required {
        if !value.contains_key(field) {
            errors.push(required_message(path, field));
        }
    }
    for key in allowed {
        let Some(item) = value.field(key) else {
            continue;
        };
        let item_path = format!("{path}.{key}");
        if *key == "steps" {
            validate_definition(&item_path, Definition::SelfReviewStep, &item, errors);
            continue;
        }
        let shape = if property_lists(definition) {
            TEXT_LIST
        } else {
            NON_EMPTY_TEXT
        };
        validate_shape(&item_path, shape, &item, errors);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sk_kernel::python_yaml::{Dialect, GraphDocument, decode_graph};

    fn body(text: &str) -> GraphDocument {
        GraphDocument::new(decode_graph(text, Dialect::RuamelSafe12).expect("decode"))
    }

    fn schema_errors(text: &str) -> Vec<String> {
        let document = body(text);
        agent_profile_schema_errors(&Values::single(document.root()))
    }

    #[test]
    fn required_anyof_and_property_messages_match_the_reference_wording() {
        let errors = schema_errors("profile-id: bad\n");
        assert_eq!(
            errors,
            vec![
                "root: 'name' is a required property".to_owned(),
                "root: 'purpose' is a required property".to_owned(),
                "root: 'specialization' is a required property".to_owned(),
                "root: {'profile-id': 'bad'} is not valid under any of the given schemas"
                    .to_owned(),
            ]
        );
    }

    #[test]
    fn scalar_and_list_property_messages_use_python_repr() {
        let errors = schema_errors(
            "profile-id: BAD_1\nname: ''\nroles: 7\nrouting-priority: 500\nmax-concurrent-tasks: 0\nschema-version: '3'\ntitle: nope\n",
        );
        for expected in [
            "root: Additional properties are not allowed ('title' was unexpected)",
            "profile-id: 'BAD_1' does not match '^[a-z][a-z0-9-]*$'",
            "name: '' is too short",
            "roles: 7 is not of type 'array'",
            "routing-priority: 500 is greater than the maximum of 100",
            "max-concurrent-tasks: 0 is less than the minimum of 1",
            "schema-version: '3' does not match '^(1\\\\.0|2)$'",
        ] {
            assert!(
                errors.iter().any(|error| error == expected),
                "{expected:?} in {errors:?}"
            );
        }
    }

    #[test]
    fn nested_definitions_report_the_dotted_path() {
        let errors = schema_errors(
            "profile-id: ok\nname: n\npurpose: p\nroles: [reviewer]\nspecialization: {primary-focus: ''}\n",
        );
        assert!(
            errors
                .iter()
                .any(|error| error == "specialization.primary-focus: '' is too short"),
            "{errors:?}"
        );
        let missing = schema_errors(
            "profile-id: ok\nname: n\npurpose: p\nroles: [reviewer]\nspecialization: {}\n",
        );
        assert!(
            missing
                .iter()
                .any(|error| error == "specialization: 'primary-focus' is a required property"),
            "{missing:?}"
        );
    }

    #[test]
    fn an_undeclared_cyclic_key_is_only_an_extra_key_diagnostic() {
        // `mode-defaults` items are entered property by property and the schema
        // declares `additionalProperties: false`, so the undeclared cyclic key is
        // exactly the reference's one diagnostic: the cycle itself is never
        // reported, and the model (which ignores an undeclared nested key)
        // accepts the profile.
        let errors = schema_errors(
            "profile-id: ok\nname: n\npurpose: p\nroles: [reviewer]\nspecialization: {primary-focus: x}\nmode-defaults: [{mode: m, description: d, use-case: u, extra: &c [*c]}]\n",
        );
        assert_eq!(
            errors,
            vec![
                "mode-defaults.0: Additional properties are not allowed ('extra' was unexpected)"
                    .to_owned()
            ],
            "{errors:?}"
        );
    }

    #[test]
    fn a_cycle_in_a_required_field_is_reported_with_its_exact_shape() {
        let errors = schema_errors(
            "profile-id: ok\nname: &n [*n]\npurpose: p\nroles: [reviewer]\nspecialization: {primary-focus: x}\n",
        );
        assert!(
            errors
                .iter()
                .any(|error| error == "name: [['...']] is not of type 'string'"
                    || error.starts_with("name: ") && error.ends_with("is not of type 'string'")),
            "{errors:?}"
        );
    }
}
