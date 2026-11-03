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
//! procedures}/models.py`). They are presentation metadata, not a second
//! domain model: nothing here validates or stores artifacts, and the YAML file
//! remains the single authored authority.

use serde_yaml_ng::Value;
use sk_charter::ArtifactKind;

/// One field of a reference model dump.
struct Field {
    /// The model field name (identical to its serialization alias in the five
    /// artifact models, which use `by_alias=True` for the dump).
    name: &'static str,
    /// How the field's value is canonicalized.
    spec: Spec,
}

/// How one model field is canonicalized.
enum Spec {
    /// Required scalar; absent/null is omitted (an invalid artifact).
    Text,
    /// Required scalar with a model default (a `str` field that has one).
    TextDefault(&'static str),
    /// Optional scalar; `null` is dropped by `exclude_none`.
    OptText,
    /// `list[str]` with a `default_factory=list` default.
    TextList,
    /// `dict[..., ...]` with a `default_factory=dict` default.
    Map,
    /// Optional unstructured value; `null` dropped, nested nulls stripped.
    OptAny,
    /// `list[Model]` with a `default_factory=list` default.
    Models(&'static [Field]),
}

const DIRECTIVE_REFERENCE: &[Field] = &[
    Field {
        name: "type",
        spec: Spec::Text,
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
        spec: Spec::Text,
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
        spec: Spec::Models(DIRECTIVE_REFERENCE),
    },
];

const TACTIC_REFERENCE: &[Field] = &[
    Field {
        name: "name",
        spec: Spec::Text,
    },
    Field {
        name: "type",
        spec: Spec::Text,
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

const TACTIC_STEP: &[Field] = &[
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
        spec: Spec::Models(TACTIC_REFERENCE),
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
        spec: Spec::Models(TACTIC_STEP),
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
        spec: Spec::Models(TACTIC_REFERENCE),
    },
    Field {
        name: "notes",
        spec: Spec::OptText,
    },
];

const STYLEGUIDE_PATTERN: &[Field] = &[
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

const STYLEGUIDE_ANTI_PATTERN: &[Field] = &[
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
        spec: Spec::Text,
    },
    Field {
        name: "principles",
        spec: Spec::TextList,
    },
    Field {
        name: "patterns",
        spec: Spec::Models(STYLEGUIDE_PATTERN),
    },
    Field {
        name: "anti_patterns",
        spec: Spec::Models(STYLEGUIDE_ANTI_PATTERN),
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

const PROCEDURE_REFERENCE: &[Field] = &[
    Field {
        name: "type",
        spec: Spec::Text,
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

const PROCEDURE_STEP: &[Field] = &[
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
        spec: Spec::TextDefault("agent"),
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

const PROCEDURE_ANTI_PATTERN: &[Field] = &[
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
        spec: Spec::Models(PROCEDURE_STEP),
    },
    Field {
        name: "anti_patterns",
        spec: Spec::Models(PROCEDURE_ANTI_PATTERN),
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
        spec: Spec::Models(PROCEDURE_REFERENCE),
    },
];

/// The model-dump field surface for `kind`, or `None` for an untyped kind.
fn schema(kind: ArtifactKind) -> Option<&'static [Field]> {
    match kind {
        ArtifactKind::Directive => Some(DIRECTIVE),
        ArtifactKind::Tactic => Some(TACTIC),
        ArtifactKind::Styleguide => Some(STYLEGUIDE),
        ArtifactKind::Toolguide => Some(TOOLGUIDE),
        ArtifactKind::Procedure => Some(PROCEDURE),
        _ => None,
    }
}

/// The reference model dump of one parsed artifact document.
///
/// Returns `None` when the document is not a mapping or the kind has no typed
/// model dump; a caller then omits `body`, matching `_jsonable_artifact_value`'s
/// `if body:` guard.
pub fn canonical_artifact_body(kind: ArtifactKind, value: &Value) -> Option<serde_json::Value> {
    let fields = schema(kind)?;
    let mapping = value.as_mapping()?;
    Some(dump_object(fields, mapping))
}

fn dump_object(fields: &[Field], mapping: &serde_yaml_ng::Mapping) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    for field in fields {
        let value = mapping.get(field.name).filter(|value| !value.is_null());
        let Some(value) = value else {
            // Missing/null: a default-factory field materialises its empty
            // container; an optional field is dropped by `exclude_none`.
            match field.spec {
                Spec::TextDefault(default) => {
                    out.insert(
                        field.name.to_owned(),
                        serde_json::Value::String(default.to_owned()),
                    );
                }
                Spec::TextList => {
                    out.insert(field.name.to_owned(), serde_json::Value::Array(Vec::new()));
                }
                Spec::Map => {
                    out.insert(
                        field.name.to_owned(),
                        serde_json::Value::Object(serde_json::Map::new()),
                    );
                }
                Spec::Models(_) => {
                    out.insert(field.name.to_owned(), serde_json::Value::Array(Vec::new()));
                }
                Spec::Text | Spec::OptText | Spec::OptAny => {}
            }
            continue;
        };
        let dumped = match &field.spec {
            Spec::Text | Spec::TextDefault(_) | Spec::OptText => scalar(value),
            Spec::TextList => list_of_text(value),
            Spec::Map => json_value(value),
            Spec::OptAny => json_value(value),
            Spec::Models(nested) => list_of_models(nested, value),
        };
        if !dumped.is_null() {
            out.insert(field.name.to_owned(), dumped);
        }
    }
    // A valid model has no undeclared keys (`extra="forbid"`), so an authored
    // key outside the schema cannot occur. Keeping it would invent a field the
    // reference dump never carries, so the canonical dump is schema-only.
    serde_json::Value::Object(out)
}

fn list_of_models(fields: &[Field], value: &Value) -> serde_json::Value {
    let Some(items) = value.as_sequence() else {
        return serde_json::Value::Array(Vec::new());
    };
    serde_json::Value::Array(
        items
            .iter()
            .map(|item| match item.as_mapping() {
                Some(mapping) => dump_object(fields, mapping),
                None => json_value(item),
            })
            .collect(),
    )
}

fn list_of_text(value: &Value) -> serde_json::Value {
    let Some(items) = value.as_sequence() else {
        return serde_json::Value::Array(Vec::new());
    };
    serde_json::Value::Array(items.iter().map(scalar).collect())
}

fn scalar(value: &Value) -> serde_json::Value {
    match value {
        Value::Null => serde_json::Value::Null,
        Value::Bool(flag) => serde_json::Value::Bool(*flag),
        Value::Number(_) => json_value(value),
        Value::String(text) => serde_json::Value::String(text.clone()),
        _ => json_value(value),
    }
}

/// Convert an arbitrary YAML value to JSON, stripping `null`s the way
/// `_jsonable_artifact_value` does for unstyped nested content.
fn json_value(value: &Value) -> serde_json::Value {
    match value {
        Value::Null => serde_json::Value::Null,
        Value::Bool(flag) => serde_json::Value::Bool(*flag),
        Value::Number(number) => {
            if let Some(value) = number.as_u64() {
                serde_json::Value::Number(value.into())
            } else if let Some(value) = number.as_i64() {
                serde_json::Value::Number(value.into())
            } else {
                serde_json::Number::from_f64(number.as_f64().unwrap_or_default())
                    .map(serde_json::Value::Number)
                    .unwrap_or(serde_json::Value::Null)
            }
        }
        Value::String(text) => serde_json::Value::String(text.clone()),
        Value::Sequence(items) => serde_json::Value::Array(
            items
                .iter()
                .map(json_value)
                .filter(|item| !item.is_null())
                .collect(),
        ),
        Value::Mapping(mapping) => {
            let object: serde_json::Map<String, serde_json::Value> = mapping
                .iter()
                .filter_map(|(key, value)| {
                    let key = match key {
                        Value::String(text) => text.clone(),
                        other => other.as_str()?.to_owned(),
                    };
                    let value = json_value(value);
                    (!value.is_null()).then_some((key, value))
                })
                .collect();
            serde_json::Value::Object(object)
        }
        Value::Tagged(tagged) => json_value(&tagged.value),
    }
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
