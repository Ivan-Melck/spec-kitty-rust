//! Exact canonical artifact payload — the one schema walker/coercion owner.
//!
//! The reference renders an `--include` artifact payload from a **validated
//! Pydantic model** through
//! `_jsonable_artifact_value` → `model_dump(by_alias=True, mode="json",
//! exclude_none=True)` → `json.dumps(payload, indent=2, sort_keys=True)`
//! (`charter/activation/context_renderers/artifact_bodies.py`). `serde_json`'s
//! value tree cannot carry that domain: a typed `float` field keeps
//! `NaN`/`Infinity`/`-Infinity` and signed zero, and a `str` keeps every code
//! point (a lone surrogate is written as its own `\uXXXX` escape by CPython's
//! `ensure_ascii=True` encoder). So the payload is built directly as a Kernel
//! [`PythonJsonValue`] and rendered by the Kernel's one `json.dumps` encoder.
//!
//! # One walker, two inputs
//!
//! [`Input`] is the only difference between the entry points: an
//! [`Input::Graph`] node (the canonical constructed YAML object graph — the
//! authority for exact code points, `set` table order, `!!pairs`, `!!omap`,
//! `!!binary` and big integers) or an [`Input::Value`] node (a constructed
//! `serde_yaml_ng` tree whose carrier keeps no graph). Every field of
//! [`super::Field`]/[`super::Spec`] is walked exactly once, in
//! [`super::schema`] order, by [`dump_object`]; there is no second model,
//! schema, coercion table or leaf renderer.
//!
//! # Model semantics reproduced
//!
//! * `model_dump(by_alias=True)` — the declared field name *is* the
//!   serialization alias in every one of the nine artifact models.
//! * `exclude_none=True` — a `None` member is absent at every model level.
//! * model defaults — a default-factory list/mapping/`SkillInvocation` is
//!   materialised; a required field with no default is omitted (an unadmitted
//!   document must never be made to look model-shaped).
//! * pydantic lax coercions — the ONE text authority
//!   ([`crate::offering::model::lax_float_text`],
//!   [`crate::offering::model::lax_bool_text`]) and the Kernel graph's
//!   `as_bool_lax`/`as_lax_str` accessors; no private spelling table.
//! * `mode="json"` inside an **unstructured** (`dict[str, Any]`) member — a
//!   non-finite float becomes `null` (pydantic's JSON projection), array `null`s
//!   are kept, mapping `null`s are dropped by `_jsonable_artifact_value`, a
//!   `set` becomes an array in the constructed CPython table order, `!!pairs`
//!   becomes an array of two-element arrays in authored order, `!!omap` becomes
//!   a JSON object, and `!!binary` becomes the UTF-8 text of its bytes. A typed
//!   (`Spec::Float`) float keeps its non-finite value and sign — the two are
//!   deliberately different, because the reference's two phases differ.
//! * mapping **keys** — pydantic's JSON serializer rewrites every key of an
//!   unstructured mapping into a string (`1` → `"1"`, `True` → `"true"`,
//!   `None` → `"None"`, a tuple → its members joined with `,`, …). That codec is
//!   the ONE Kernel authority ([`sk_kernel::model_json`]); this walker never
//!   invents a key spelling and never blanket-refuses a non-`str` key. A key the
//!   reference cannot spell (non-UTF-8 `bytes`, an unpaired surrogate) is a typed
//!   failure, not a silent drop. `null` **values** are dropped while a `None`
//!   **key** keeps its `"None"` spelling — two different rules, kept distinct.
//!
//! # Unrepresentable payloads are refused with their real cause
//!
//! [`exact_payload`], [`exact_payload_value`] and [`exact_json_value`] return a
//! typed [`PayloadError`]/[`PayloadFailure`]: a kind with no typed model here, a
//! non-mapping root, `!!binary` bytes that are not UTF-8 (the reference's
//! `mode="json"` raises `UnicodeDecodeError` there), a mapping key with no UTF-8
//! form (`UnicodeEncodeError`), a mapping key Python never accepts, and — on the
//! bare-tree path only — a transported marker string. The `Option`-returning
//! [`canonical_artifact_body_exact`] family is the retained compatibility view
//! that collapses those causes (`None`); it is not the reporting API. The exact
//! API never silently drops, replaces or stringifies a member, and a nested
//! model-dump failure refuses the whole payload instead of dropping the member.
//!
//! # Known non-exact edges (disclosed, not papered over)
//!
//! * [`Input::Value`] (`canonical_artifact_body_exact_value`) has no transport
//!   markers, so a document whose lossy strings were marker-encoded cannot be
//!   reproduced from a bare tree; the canonical graph
//!   (`canonical_artifact_body_exact`) or the file reader
//!   (`canonical_artifact_body_exact_path`) is required. A payload containing a
//!   marker refuses instead of printing the marker spelling. The bare tree must
//!   also be a *constructed* one: the `!!set`/`!!pairs`/`!!omap`/`!!binary`
//!   shapes are normalized by the canonical construct phase, so a caller holding
//!   only the semantic parser's raw tree must use the graph path instead.
//! * Date/datetime construction belongs to Kernel, including ruamel implicit
//!   timestamps. Their model-JSON presentation uses the shared Kernel adapter:
//!   `T` separator and UTC `Z`, distinct from Python `str()`/isoformat offsets.

use serde_yaml_ng::Value;
use sk_charter::ArtifactKind;
use sk_kernel::PythonString;
use sk_kernel::model_json::{
    ModelJsonKeyError, model_json_mapping_key, model_json_mapping_key_value,
};
use sk_kernel::python_json::{PythonJsonDict, PythonJsonValue};
use sk_kernel::python_utf8::{PythonUtf8DecodeError, PythonUtf8EncodeError, decode_utf8};
use sk_kernel::python_yaml::{
    NodeRef, OMAP_TAG, Object, PAIRS_TAG, SET_TAG, constructed_big_integer, constructed_bytes,
    constructed_mapping, constructed_sequence, constructed_set, constructed_timestamp_text,
    constructed_tuple, is_constructed_timestamp, is_marker_text,
};

use super::{Field, Spec, schema};

use crate::offering::model::{lax_bool_text, lax_float_text};

/// One real reason the reference cannot produce a model-JSON payload.
///
/// Each variant names the phase the reference actually fails in. Nothing here is
/// a generic "unrepresentable" bucket: a caller can tell a pydantic
/// `UnicodeDecodeError` from a key `UnicodeEncodeError`, from an object Python
/// refuses as a mapping key at all, without parsing text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PayloadFailure {
    /// The value is not a mapping, so the reference's model never dumps it.
    NotAMapping,
    /// `bytes`/`!!binary` whose strict UTF-8 decode fails — CPython's
    /// `UnicodeDecodeError` (`mode="json"` raises it while projecting the bytes).
    BytesNotUtf8(PythonUtf8DecodeError),
    /// A mapping key with no UTF-8 form (an unpaired surrogate) — CPython's
    /// `UnicodeEncodeError` (`surrogates not allowed`) from pydantic's own key
    /// codec. The same surrogate is *kept* in a value, so this is a key-only rule.
    KeyNotUtf8(PythonUtf8EncodeError),
    /// An object Python cannot use as a mapping key (`list`, `dict`, `set`, …).
    /// The reference's loader refuses it before any model exists.
    UnsupportedKey {
        /// The Python type name the reference's refusal names.
        python_type_name: &'static str,
    },
    /// A **typed** mapping key that is not text (for example a `int` key in a
    /// `dict[str, Any]` field). The reference's *model validation* refuses it
    /// (`ValidationError`), so the document is not model-shaped; this crate's
    /// typed admission keeps the same refusal.
    TypedKeyNotText {
        /// The Python type name of the refused key.
        python_type_name: &'static str,
    },
    /// The bare-tree carrier replaced authored code points with transport
    /// markers. A Native carrier limitation, **not** a Source refusal: the
    /// canonical graph carries the authored text instead.
    TransportMarker,
    /// A **typed** member does not have its declared shape, so the reference's
    /// *model validation* refuses the document (`ValidationError`) before any dump
    /// exists. `detail` names the member class.
    NotModelShaped {
        /// A short description of the refused typed member.
        detail: &'static str,
    },
    /// A member the canonical **model view** cannot represent yet (a structured
    /// mapping, a typed `float`, an unstructured `Any`, `SkillInvocation`). This
    /// is a Native capability boundary of `str`/`repr` rendering; the exact JSON
    /// dump of the same record is unaffected.
    UnsupportedMember {
        /// A short description of the member class.
        detail: &'static str,
    },
}

impl std::fmt::Display for PayloadFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAMapping => {
                formatter.write_str("the value is not a mapping, so the model never dumps it")
            }
            Self::BytesNotUtf8(error) => {
                formatter.write_str(&sk_kernel::model_json::model_json_bytes_error_text(error))
            }
            Self::KeyNotUtf8(error) => {
                write!(formatter, "a mapping key has no UTF-8 form ({error})")
            }
            Self::UnsupportedKey { python_type_name } => {
                write!(
                    formatter,
                    "unhashable mapping key (unhashable type: '{python_type_name}')"
                )
            }
            Self::TypedKeyNotText { python_type_name } => write!(
                formatter,
                "a typed mapping key is not text (model validation refuses a {python_type_name} key)"
            ),
            Self::TransportMarker => formatter.write_str(
                "the bare-tree carrier replaced authored code points with transport markers",
            ),
            Self::NotModelShaped { detail } => {
                write!(
                    formatter,
                    "model validation refuses the document ({detail})"
                )
            }
            Self::UnsupportedMember { detail } => {
                write!(formatter, "the typed model view cannot render {detail} yet")
            }
        }
    }
}

impl std::error::Error for PayloadFailure {}

impl From<ModelJsonKeyError> for PayloadFailure {
    fn from(error: ModelJsonKeyError) -> Self {
        match error {
            ModelJsonKeyError::BytesNotUtf8(error) => Self::BytesNotUtf8(error),
            ModelJsonKeyError::TextNotUtf8(error) => Self::KeyNotUtf8(error),
            ModelJsonKeyError::NotAKey { python_type_name } => {
                Self::UnsupportedKey { python_type_name }
            }
        }
    }
}

/// Why the exact model-JSON payload could not be produced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PayloadError {
    /// This crate has no typed model for the kind, or the body has no canonical
    /// graph: a Native capability boundary, never a Source refusal.
    NoSchema,
    /// The reference's own dump/projection refuses this value.
    Failed(PayloadFailure),
}

impl PayloadError {
    /// The underlying reference cause, when this is one.
    #[must_use]
    pub fn failure(&self) -> Option<&PayloadFailure> {
        match self {
            Self::NoSchema => None,
            Self::Failed(failure) => Some(failure),
        }
    }
}

impl std::fmt::Display for PayloadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSchema => formatter.write_str("this crate has no model dump for the kind"),
            Self::Failed(failure) => failure.fmt(formatter),
        }
    }
}

impl std::error::Error for PayloadError {}

/// The exact canonical model dump of one admitted artifact document.
///
/// `root` is the document root of the ONE canonical graph (the declaring
/// document's own root). [`PayloadError::NoSchema`] when `kind` has no typed model
/// here, and [`PayloadError::Failed`] with the reference's real cause when the
/// value cannot be dumped faithfully (see [`PayloadFailure`]).
///
/// # Errors
///
/// [`PayloadError`] as above.
pub fn exact_payload(
    kind: ArtifactKind,
    root: NodeRef<'_>,
) -> Result<PythonJsonValue, PayloadError> {
    let fields = schema(kind).ok_or(PayloadError::NoSchema)?;
    if root.as_mapping().is_none() {
        return Err(PayloadError::Failed(PayloadFailure::NotAMapping));
    }
    dump_object(fields, Input::Graph(root)).map_err(PayloadError::Failed)
}

/// The exact canonical model dump of one admitted artifact document.
///
/// If `root` is not a mapping or `kind` has no typed
/// model, exactly like the finite scalar adapter; a payload that cannot be
/// produced faithfully is also `None` (see the module docs).
#[must_use]
pub fn canonical_artifact_body_exact(
    kind: ArtifactKind,
    root: NodeRef<'_>,
) -> Option<PythonJsonValue> {
    exact_payload(kind, root).ok()
}

/// The exact canonical model dump of one constructed YAML tree.
///
/// The same walker as [`exact_payload`] over a tree without graph provenance. It
/// is exact for every case the tree carries (numbers, big-integer tags, and the
/// `!!set`/`!!pairs`/`!!omap`/`!!binary` shapes); a transported marker string
/// refuses because a marker is not the authored code points (see the module
/// docs).
///
/// # Errors
///
/// [`PayloadError`] as [`exact_payload`].
pub fn exact_payload_value(
    kind: ArtifactKind,
    value: &Value,
) -> Result<PythonJsonValue, PayloadError> {
    let fields = schema(kind).ok_or(PayloadError::NoSchema)?;
    if value.as_mapping().is_none() {
        return Err(PayloadError::Failed(PayloadFailure::NotAMapping));
    }
    if contains_marker(value) {
        // The carrier replaced authored code points with transport markers; a
        // marker spelling must never be printed as if it were the data.
        return Err(PayloadError::Failed(PayloadFailure::TransportMarker));
    }
    dump_object(fields, Input::Value(value)).map_err(PayloadError::Failed)
}

/// One selected **text** field from the same schema the exact dump walks.
///
/// The reference's action payload reads model *properties* for metadata
/// (`artifact_to_dict`: `getattr(artifact, "title", None) or getattr(artifact,
/// "name", None)`), while the inline `body` is the JSON-mode model dump. Those
/// are two different phases: a metadata read must not serialize an unrelated
/// unstructured member (a non-UTF-8 `!!binary` inside `structural_lint_config`, a
/// non-string mapping key, …), because the reference never dumps that member to
/// answer `artifact.title`.
///
/// This is the ONE selected-field view over [`schema`]: it walks only the named
/// field and never any other member. It is deliberately limited to the text
/// specs the metadata fields use; a name outside the kind's schema (or a field
/// that is not a text member) is `Ok(None)`, matching `getattr` yielding `None`.
///
/// # Errors
///
/// [`PayloadError::NoSchema`] when this crate has no typed model for `kind` (a
/// Native boundary, not a Source refusal), [`PayloadError::Failed`] with
/// [`PayloadFailure::NotAMapping`] for a non-mapping root, or
/// [`PayloadFailure::NotModelShaped`] for a present field whose value is not its
/// declared text type (the reference's model validation would have refused the
/// document before a dump existed).
pub fn selected_text_field(
    kind: ArtifactKind,
    root: NodeRef<'_>,
    name: &str,
) -> Result<Option<PythonString>, PayloadError> {
    let fields = schema(kind).ok_or(PayloadError::NoSchema)?;
    if root.as_mapping().is_none() {
        return Err(PayloadError::Failed(PayloadFailure::NotAMapping));
    }
    let Some(field) = fields.iter().find(|field| field.name == name) else {
        return Ok(None);
    };
    let input = Input::Graph(root);
    match input.field(field.name).filter(|value| !value.is_null()) {
        None => Ok(default_member(&field.spec).into_text()),
        Some(value) => match &field.spec {
            Spec::Text
            | Spec::TextDefault(_)
            | Spec::OptText
            | Spec::Enum(_)
            | Spec::EnumDefault(_, _) => match value.text() {
                Some(text) => Ok(Some(text)),
                None => Err(PayloadError::Failed(PayloadFailure::NotModelShaped {
                    detail: "a selected text field is not text",
                })),
            },
            _ => Ok(None),
        },
    }
}

/// The exact canonical model dump of one constructed YAML tree.
#[must_use]
pub fn canonical_artifact_body_exact_value(
    kind: ArtifactKind,
    value: &Value,
) -> Option<PythonJsonValue> {
    exact_payload_value(kind, value).ok()
}

/// Whether a constructed tree still carries a lossless-transport marker.
///
/// The canonical graph holds the authored code points instead, so this only
/// applies to the bare-tree carrier.
fn contains_marker(value: &Value) -> bool {
    match value {
        Value::String(text) => is_marker_text(text),
        Value::Sequence(items) => items.iter().any(contains_marker),
        Value::Mapping(mapping) => mapping
            .iter()
            .any(|(key, value)| contains_marker(key) || contains_marker(value)),
        Value::Tagged(tagged) => contains_marker(&tagged.value),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

/// The exact `_jsonable_artifact_value` projection of one canonical graph node.
///
/// This is the projection the reference applies to a model dump that has **no**
/// [`super::Field`] schema in this crate (today: the agent-profile model dump).
/// It is the same [`any_graph`] walker the structured path uses for an
/// unstructured member, so there is no second projector: every code point
/// survives, a `null` member is dropped from a mapping but kept in an array, a
/// `set`/`tuple` becomes an array in the constructed order, `!!pairs`/`!!omap`/
/// `!!binary` follow their constructed rules, a non-finite float becomes `null`
/// (pydantic's `mode="json"` projection), and a mapping key takes the Kernel's
/// model-JSON key codec — while a typed `Spec::Float` field never comes here.
///
/// A model dump's mappings are keyed by text already (`model_dump(mode="json")`
/// stringified every unstructured key, and model validation refused a non-text
/// typed key), so a *conforming* body projects without touching the key codec.
///
/// # Errors
///
/// The reference's real cause ([`PayloadFailure`]) when a value or a key has no
/// faithful JSON form. The caller must treat that as its own refusal, never as an
/// absent artifact.
pub fn exact_json_value(node: NodeRef<'_>) -> Result<PythonJsonValue, PayloadFailure> {
    any_graph(node)
}

/// The exact canonical model dump of one artifact file.
///
/// The file is read through the crate's ONE lossless transport, constructor and
/// object graph (`crate::offering::value::ArtifactDocument`) — never a private
/// parser — and then walked by [`canonical_artifact_body_exact`]. `None` for a
/// document the reference skips (unreadable/syntax/construct) as well as for the
/// faithful-refusal cases above; a caller that must distinguish the read phases
/// goes through the offering repository instead of this convenience.
#[must_use]
pub fn canonical_artifact_body_exact_path(
    kind: ArtifactKind,
    path: &std::path::Path,
) -> Option<PythonJsonValue> {
    let document = crate::offering::value::ArtifactDocument::read(path).ok()?;
    canonical_artifact_body_exact(kind, document.root())
}

/// One dump input: a canonical graph node or a constructed YAML node.
#[derive(Clone, Copy)]
enum Input<'a> {
    Graph(NodeRef<'a>),
    Value(&'a Value),
}

/// One walked member: a value, or "the model has no member here".
///
/// `Omit` is the reference's `exclude_none`/no-default outcome. A field that is
/// *present* but cannot be read as its declared type is inadmissible input, so
/// the member is omitted rather than invented; the whole payload only refuses
/// when a value itself has no faithful JSON form.
enum Member {
    Value(PythonJsonValue),
    Omit,
}

impl Member {
    /// The member's text value, when it materialised as a string.
    fn into_text(self) -> Option<PythonString> {
        match self {
            Self::Value(PythonJsonValue::String(text)) => Some(text),
            Self::Value(_) | Self::Omit => None,
        }
    }
}

impl<'a> Input<'a> {
    fn is_null(self) -> bool {
        match self {
            Self::Graph(node) => node.is_null(),
            Self::Value(value) => value.is_null(),
        }
    }

    fn field(self, name: &str) -> Option<Self> {
        match self {
            Self::Graph(node) => node.lookup(name).map(Self::Graph),
            Self::Value(value) => value.as_mapping()?.get(name).map(Self::Value),
        }
    }

    /// The reference's lax `str`: a `str` (exact code points) or UTF-8 `bytes`.
    fn text(self) -> Option<PythonString> {
        match self {
            Self::Graph(node) => node.as_lax_str(),
            Self::Value(value) => match value {
                Value::String(text) => Some(PythonString::from_scalars(text)),
                Value::Tagged(_) => constructed_bytes(value)
                    .and_then(|bytes| std::str::from_utf8(&bytes).ok().map(str::to_owned))
                    .map(|text| PythonString::from_scalars(&text)),
                _ => None,
            },
        }
    }

    /// The reference's lax `float` (bool → 1.0/0.0, int → f64, text spellings).
    fn float(self) -> Option<f64> {
        match self {
            Self::Graph(node) => {
                if let Some(flag) = node.as_bool() {
                    return Some(if flag { 1.0 } else { 0.0 });
                }
                if let Some(digits) = node.as_int_text() {
                    return digits.parse().ok();
                }
                if let Some(number) = node.as_float() {
                    return Some(number);
                }
                let text = node.as_str()?.to_scalar_string().ok()?;
                lax_float_text(&text)
            }
            Self::Value(value) => match value {
                Value::Bool(flag) => Some(if *flag { 1.0 } else { 0.0 }),
                Value::Number(number) => number.as_f64(),
                Value::String(_) => {
                    let text = self.text()?.to_scalar_string().ok()?;
                    lax_float_text(&text)
                }
                _ => None,
            },
        }
    }

    /// The reference's lax `bool`.
    fn boolean(self) -> Option<bool> {
        match self {
            Self::Graph(node) => node.as_bool_lax(),
            Self::Value(value) => match value {
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
                Value::String(text) => lax_bool_text(text),
                _ => None,
            },
        }
    }

    /// An `ArtifactKind` field's canonical `mode="json"` spelling.
    fn kind_name(self) -> Option<&'static str> {
        let text = self.text()?.to_scalar_string().ok()?;
        ArtifactKind::from_canonical(&text)
            .ok()
            .map(ArtifactKind::canonical)
    }

    /// The constructed members of a list-like value, in the authority's order.
    ///
    /// A `list`, `tuple` and `set` each expose their constructed member order (a
    /// `set`'s is CPython's table order, from the Kernel set-order authority). A
    /// value of any other shape has no members here — never an empty list.
    fn sequence(self) -> Option<Vec<Self>> {
        match self {
            Self::Graph(node) => node
                .as_sequence()
                .or_else(|| node.as_tuple())
                .or_else(|| node.as_set())
                .map(|items| {
                    items
                        .iter()
                        .map(|id| Self::Graph(node.member(*id)))
                        .collect()
                }),
            Self::Value(value) => {
                constructed_sequence(value).map(|items| items.iter().map(Self::Value).collect())
            }
        }
    }

    /// The authored entries of a mapping-like value (`dict` or `!!omap`).
    fn mapping(self) -> Option<Vec<(Self, Self)>> {
        match self {
            Self::Graph(node) => node.as_mapping().map(|entries| {
                entries
                    .iter()
                    .map(|(key, value)| {
                        (
                            Self::Graph(node.member(*key)),
                            Self::Graph(node.member(*value)),
                        )
                    })
                    .collect()
            }),
            Self::Value(value) => constructed_mapping(value).map(|entries| {
                entries
                    .iter()
                    .map(|(key, value)| (Self::Value(key), Self::Value(value)))
                    .collect()
            }),
        }
    }

    /// `to_jsonable_python` over a validated model's unstructured member,
    /// followed by `_jsonable_artifact_value`'s container rules.
    ///
    /// A typed (`Spec::Float`) field never comes here: its non-finite value is
    /// preserved. Inside unstructured content pydantic's JSON projection turns a
    /// non-finite float into `null`, keeps array `null`s, leaves mapping `null`s
    /// for the dict comprehension to drop, and spells every mapping key through
    /// [`sk_kernel::model_json`].
    fn any(self) -> Result<PythonJsonValue, PayloadFailure> {
        match self {
            Self::Graph(node) => any_graph(node),
            Self::Value(value) => any_value(value),
        }
    }

    /// The `dict[str, Any] | None` root: a **typed** mapping of an unstructured
    /// payload.
    ///
    /// The field's own keys are a typed `dict[str, ...]`, so pydantic validates
    /// them (`ValidationError` for a non-text key) before any dump exists — that
    /// is admission, not the key codec, and it stays unchanged here. The
    /// members' *values* are the unstructured domain and take the model-JSON
    /// projection. A non-mapping value keeps the unstructured projection exactly
    /// as before.
    fn typed_any_mapping(self) -> Result<PythonJsonValue, PayloadFailure> {
        let Some(entries) = self.mapping() else {
            return self.any();
        };
        let mut normalized = Vec::with_capacity(entries.len());
        for (key, value) in entries {
            let Some(key) = key.model_text() else {
                return Err(PayloadFailure::TypedKeyNotText {
                    python_type_name: key.python_type_name(),
                });
            };
            // Typed key normalization is model validation, before JSON-mode
            // serialization of Any values. An overwritten value is not dumped.
            PythonJsonDict::assign_entry(&mut normalized, key, value);
        }
        let members = normalized
            .into_iter()
            .map(|(key, value)| Ok((key, value.any()?)))
            .collect::<Result<Vec<_>, PayloadFailure>>()?;
        Ok(exact_object(members))
    }

    /// The Python type name of this value (for a typed-key refusal).
    fn python_type_name(self) -> &'static str {
        match self {
            Self::Graph(node) => node.python_type_name(),
            Self::Value(value) => value_python_type_name(value),
        }
    }

    /// The reference's lax `str` for a **typed** mapping key, which is the same
    /// admission a `Spec::Text` field uses: text, or `bytes` decoded as UTF-8.
    fn model_text(self) -> Option<PythonString> {
        self.text()
    }
}

/// The Python type name of one constructed tree node.
fn value_python_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(number) => {
            if constructed_big_integer(value).is_some()
                || number.as_i64().is_some()
                || number.as_u64().is_some()
            {
                "int"
            } else {
                "float"
            }
        }
        Value::String(_) => "str",
        Value::Sequence(_) => "list",
        Value::Mapping(_) => "dict",
        Value::Tagged(tagged) => {
            if constructed_bytes(value).is_some() {
                return "bytes";
            }
            if is_constructed_timestamp(value) {
                return "datetime";
            }
            if constructed_set(value).is_some() {
                return "set";
            }
            if constructed_tuple(value).is_some() {
                return "tuple";
            }
            if tagged.tag == OMAP_TAG {
                return "ordereddict";
            }
            "str"
        }
    }
}

fn any_graph(node: NodeRef<'_>) -> Result<PythonJsonValue, PayloadFailure> {
    Ok(match node.object() {
        Object::Null => PythonJsonValue::Null,
        Object::Bool(flag) => PythonJsonValue::Bool(*flag),
        Object::Int(digits) => PythonJsonValue::Integer(digits.clone()),
        Object::Float { value, .. } => json_float(*value),
        Object::Str(text) => PythonJsonValue::String(text.clone()),
        Object::Bytes(bytes) => {
            let text = decode_utf8(bytes).map_err(PayloadFailure::BytesNotUtf8)?;
            PythonJsonValue::String(PythonString::from_scalars(text))
        }
        Object::Date(text) => PythonJsonValue::String(PythonString::from_scalars(text)),
        Object::DateTime(text) => PythonJsonValue::String(datetime_isoformat(text)),
        Object::Seq(items) | Object::Tuple(items) | Object::Set(items) => PythonJsonValue::Array(
            items
                .iter()
                .map(|id| any_graph(node.member(*id)))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Object::Map(entries) | Object::OrderedDict(entries) => {
            let mut members = Vec::with_capacity(entries.len());
            for (key, value) in entries {
                // The ONE model-JSON key codec, not a local key table.
                let key = model_json_mapping_key(node.member(*key))?;
                members.push((key, any_graph(node.member(*value))?));
            }
            exact_object(members)
        }
        Object::Pairs(entries) => PythonJsonValue::Array(
            entries
                .iter()
                .map(|(key, value)| {
                    Ok(PythonJsonValue::Array(vec![
                        any_graph(node.member(*key))?,
                        any_graph(node.member(*value))?,
                    ]))
                })
                .collect::<Result<Vec<_>, PayloadFailure>>()?,
        ),
    })
}

fn any_value(value: &Value) -> Result<PythonJsonValue, PayloadFailure> {
    Ok(match value {
        Value::Null => PythonJsonValue::Null,
        Value::Bool(flag) => PythonJsonValue::Bool(*flag),
        Value::Number(number) => number_value(number),
        // `to_jsonable_python` maps a non-finite float inside unstructured
        // content to `null`; `number_value` keeps the int/float split.
        Value::String(text) => {
            if is_marker_text(text) {
                // A transported marker is not the authored code points.
                return Err(PayloadFailure::TransportMarker);
            }
            PythonJsonValue::String(PythonString::from_scalars(text))
        }
        Value::Sequence(items) => {
            PythonJsonValue::Array(items.iter().map(any_value).collect::<Result<Vec<_>, _>>()?)
        }
        Value::Mapping(_) => {
            let Some(entries) = constructed_mapping(value) else {
                return Err(PayloadFailure::NotAMapping);
            };
            let mut members = Vec::with_capacity(entries.len());
            for (key, item) in entries {
                let key = model_json_mapping_key_value(key)?;
                members.push((key, any_value(item)?));
            }
            exact_object(members)
        }
        Value::Tagged(tagged) => {
            if let Some(digits) = constructed_big_integer(value) {
                return Ok(PythonJsonValue::Integer(digits.to_owned()));
            }
            if let Some(bytes) = constructed_bytes(value) {
                let text = decode_utf8(&bytes).map_err(PayloadFailure::BytesNotUtf8)?;
                return Ok(PythonJsonValue::String(PythonString::from_scalars(text)));
            }
            if is_constructed_timestamp(value) {
                let Some(text) = constructed_timestamp_text(value) else {
                    return Err(PayloadFailure::NotAMapping);
                };
                let rendered = if text.contains(' ') && text.contains(':') {
                    datetime_isoformat(&text)
                } else {
                    PythonString::from_scalars(&text)
                };
                return Ok(PythonJsonValue::String(rendered));
            }
            if let Some(items) = constructed_tuple(value) {
                return Ok(PythonJsonValue::Array(
                    items.iter().map(any_value).collect::<Result<Vec<_>, _>>()?,
                ));
            }
            if let Some(members) = constructed_set(value) {
                return Ok(PythonJsonValue::Array(
                    members
                        .iter()
                        .map(any_value)
                        .collect::<Result<Vec<_>, _>>()?,
                ));
            }
            if tagged.tag == OMAP_TAG || tagged.tag == PAIRS_TAG {
                // A constructed tree stores `!!omap` as a mapping and `!!pairs`
                // as real two-element tuples; the semantic parser's own typed
                // form (before this tree's construct phase normalises it) is a
                // sequence of single-entry mappings. Both shapes are read here,
                // so the raw carrier cannot silently become an array of objects.
                let Some(items) = tagged.value.as_sequence() else {
                    return any_value(&tagged.value);
                };
                let entries = items
                    .iter()
                    .map(single_entry)
                    .collect::<Option<Vec<_>>>()
                    .ok_or(PayloadFailure::NotAMapping)?;
                if tagged.tag == OMAP_TAG {
                    let mut members = Vec::with_capacity(entries.len());
                    for (key, item) in entries {
                        members.push((model_json_mapping_key_value(key)?, any_value(item)?));
                    }
                    return Ok(exact_object(members));
                }
                return Ok(PythonJsonValue::Array(
                    entries
                        .into_iter()
                        .map(|(key, item)| {
                            Ok(PythonJsonValue::Array(vec![
                                any_value(key)?,
                                any_value(item)?,
                            ]))
                        })
                        .collect::<Result<Vec<_>, PayloadFailure>>()?,
                ));
            }
            if tagged.tag == SET_TAG {
                // The set-order authority runs in the construct phase. A tree
                // that still carries the authored mapping shape has no CPython
                // table order, so the payload refuses instead of guessing one.
                return Err(PayloadFailure::NotAMapping);
            }
            any_value(&tagged.value)?
        }
    })
}

/// The `(key, value)` of one `!!omap`/`!!pairs` entry, in either carrier shape.
///
/// A constructed entry is a real two-element tuple; the semantic parser's typed
/// form is a single-entry mapping. Any other shape has no entry to read.
fn single_entry(item: &Value) -> Option<(&Value, &Value)> {
    if let Some(members) = constructed_tuple(item)
        && members.len() == 2
    {
        return Some((&members[0], &members[1]));
    }
    let mapping = item.as_mapping()?;
    let mut entries = mapping.iter();
    let (key, value) = entries.next()?;
    entries.next().is_none().then_some((key, value))
}

/// A JSON number for one parsed YAML number (its lexical kind decides int/float).
///
/// A float here is inside unstructured content, so its JSON projection applies
/// ([`json_float`]): a non-finite value is `null`, exactly like pydantic's
/// `to_jsonable_python`. A typed `Spec::Float` never comes through here.
fn number_value(number: &serde_yaml_ng::Number) -> PythonJsonValue {
    if let Some(value) = number.as_u64() {
        return PythonJsonValue::Integer(value.to_string());
    }
    if let Some(value) = number.as_i64() {
        return PythonJsonValue::Integer(value.to_string());
    }
    json_float(number.as_f64().unwrap_or_default())
}

/// A `mode="json"` float inside unstructured content: a non-finite value is
/// `null` there (pydantic's JSON projection), unlike a typed `float` field.
fn json_float(value: f64) -> PythonJsonValue {
    if value.is_finite() {
        PythonJsonValue::Float(value)
    } else {
        PythonJsonValue::Null
    }
}

/// Pydantic model-JSON presentation, not Python's own isoformat spelling.
fn datetime_isoformat(text: &str) -> PythonString {
    PythonString::from_scalars(&sk_kernel::clock::model_json_datetime_text(text))
}

/// One JSON object from authored entries, dropping `null` values.
///
/// `_jsonable_artifact_value` is `{str(key): recurse(value) … if value is not
/// None}`, so a `null` member is absent at every depth.
fn exact_object(members: Vec<(PythonString, PythonJsonValue)>) -> PythonJsonValue {
    let mut object = PythonJsonDict::new();
    for (key, value) in members {
        object.insert(key, value);
    }
    // Model JSON key collisions are resolved before _jsonable_artifact_value
    // drops nulls. A last null removes the member; a first null keeps its slot
    // when a later collision replaces it with a non-null value.
    object.retain(|_, value| !matches!(value, PythonJsonValue::Null));
    PythonJsonValue::ExactObject(object)
}

/// Project only requested typed model fields using the canonical schema/walker.
///
/// Inline body callers read selected properties, not the full model JSON dump:
/// an unrelated Any member must not be serialized just to render title/scope.
/// Requested members retain their declared dump semantics; this is not a generic
/// Any-property view or permission to reconstruct model identities from JSON.
pub(crate) fn selected_payload_fields(
    kind: ArtifactKind,
    root: NodeRef<'_>,
    names: &[&str],
) -> Result<PythonJsonValue, PayloadError> {
    let fields = schema(kind).ok_or(PayloadError::NoSchema)?;
    if root.as_mapping().is_none() {
        return Err(PayloadError::Failed(PayloadFailure::NotAMapping));
    }
    dump_object_fields(
        fields.iter().filter(|field| names.contains(&field.name)),
        Input::Graph(root),
    )
    .map_err(PayloadError::Failed)
}

/// The canonical model dump of one mapping, in `schema` field order.
fn dump_object(fields: &[Field], input: Input<'_>) -> Result<PythonJsonValue, PayloadFailure> {
    dump_object_fields(fields.iter(), input)
}

fn dump_object_fields<'a>(
    fields: impl IntoIterator<Item = &'a Field>,
    input: Input<'_>,
) -> Result<PythonJsonValue, PayloadFailure> {
    let mut object = PythonJsonDict::new();
    for field in fields {
        let value = input.field(field.name).filter(|value| !value.is_null());
        let member = match value {
            None => default_member(&field.spec),
            Some(value) => dump_member(&field.spec, value)?,
        };
        if let Member::Value(value) = member {
            object.insert(PythonString::from_scalars(field.name), value);
        }
    }
    Ok(PythonJsonValue::ExactObject(object))
}

/// The model default materialised for an absent/null field, if it has one.
fn default_member(spec: &Spec) -> Member {
    match spec {
        Spec::TextDefault(default) => Member::Value(PythonJsonValue::scalar(default)),
        Spec::BooleanDefault(flag) => Member::Value(PythonJsonValue::Bool(*flag)),
        // A `StrEnum` default is its member's canonical value.
        Spec::EnumDefault(_, member) => Member::Value(PythonJsonValue::scalar(member.value)),
        Spec::TextList | Spec::Models(_) => Member::Value(PythonJsonValue::Array(Vec::new())),
        Spec::TextListDefault(items) => Member::Value(PythonJsonValue::Array(
            items
                .iter()
                .map(|item| PythonJsonValue::scalar(item))
                .collect(),
        )),
        Spec::Map => Member::Value(PythonJsonValue::ExactObject(PythonJsonDict::new())),
        Spec::SkillInvocation => Member::Value(default_skill_invocation()),
        Spec::Text
        | Spec::OptText
        | Spec::Float
        | Spec::Kind
        | Spec::Enum(_)
        | Spec::OptTextList
        | Spec::RequiredTextList
        | Spec::RequiredModels(_)
        | Spec::OptModel(_)
        | Spec::OptAny => Member::Omit,
    }
}

/// One present field's value, or `Omit` when it is not readable as its type.
fn dump_member(spec: &Spec, value: Input<'_>) -> Result<Member, PayloadFailure> {
    let member = match spec {
        Spec::Text
        | Spec::TextDefault(_)
        | Spec::OptText
        | Spec::Enum(_)
        | Spec::EnumDefault(_, _) => match value.text() {
            Some(text) => Member::Value(PythonJsonValue::String(text)),
            None => Member::Omit,
        },
        // A typed float keeps the model's non-finite value and its sign.
        Spec::Float => match value.float() {
            Some(number) => Member::Value(PythonJsonValue::Float(number)),
            None => Member::Omit,
        },
        Spec::BooleanDefault(default) => {
            Member::Value(PythonJsonValue::Bool(value.boolean().unwrap_or(*default)))
        }
        Spec::Kind => match value.kind_name() {
            Some(kind) => Member::Value(PythonJsonValue::scalar(kind)),
            None => Member::Omit,
        },
        Spec::TextList | Spec::RequiredTextList | Spec::OptTextList => match value.sequence() {
            Some(members) => match text_array(members) {
                Some(items) => Member::Value(items),
                None => Member::Omit,
            },
            None => Member::Omit,
        },
        Spec::TextListDefault(items) => match value.sequence() {
            Some(members) => match text_array(members) {
                Some(array) => Member::Value(array),
                None => Member::Omit,
            },
            None => Member::Value(PythonJsonValue::Array(
                items
                    .iter()
                    .map(|item| PythonJsonValue::scalar(item))
                    .collect(),
            )),
        },
        Spec::Map => match string_map(value) {
            Some(object) => Member::Value(object),
            None => Member::Omit,
        },
        // The only `OptAny` field today is `structural_lint_config: dict[str,
        // Any] | None`: a TYPED mapping root over an unstructured domain. Its own
        // keys are admitted by the model (not spelled by the key codec), and its
        // values take the model-JSON projection.
        Spec::OptAny => Member::Value(value.typed_any_mapping()?),
        Spec::Models(nested) | Spec::RequiredModels(nested) => {
            let Some(members) = value.sequence() else {
                return Ok(Member::Omit);
            };
            Member::Value(model_array(nested.fields, members)?)
        }
        Spec::OptModel(nested) => {
            if value.mapping().is_some() {
                Member::Value(dump_object(nested.fields, value)?)
            } else {
                // Not a mapping: the reference's `model_dump` never sees this
                // (the model refuses it), so the unstructured projection is the
                // honest rendering of whatever the node actually holds.
                Member::Value(value.any()?)
            }
        }
        Spec::SkillInvocation => {
            if value.mapping().is_some() {
                let invocation =
                    dump_skill_invocation(value).ok_or(PayloadFailure::NotModelShaped {
                        detail: "`SkillInvocation` has a member the model does not accept",
                    })?;
                Member::Value(invocation)
            } else {
                Member::Value(value.any()?)
            }
        }
    };
    Ok(member)
}

/// A `list[str]`'s exact strings; `None` when a member is not a string.
fn text_array(members: Vec<Input<'_>>) -> Option<PythonJsonValue> {
    Some(PythonJsonValue::Array(
        members
            .into_iter()
            .map(|member| Some(PythonJsonValue::String(member.text()?)))
            .collect::<Option<Vec<_>>>()?,
    ))
}

/// A `dict[str, str]`'s exact keys and values.
fn string_map(value: Input<'_>) -> Option<PythonJsonValue> {
    let entries = value.mapping()?;
    Some(exact_object(
        entries
            .into_iter()
            .map(|(key, value)| Some((key.text()?, PythonJsonValue::String(value.text()?))))
            .collect::<Option<Vec<_>>>()?,
    ))
}

/// A `list[Model]`, each element dumped through the same nested schema.
fn model_array(
    fields: &[Field],
    members: Vec<Input<'_>>,
) -> Result<PythonJsonValue, PayloadFailure> {
    Ok(PythonJsonValue::Array(
        members
            .into_iter()
            .map(|member| {
                if member.mapping().is_some() {
                    dump_object(fields, member)
                } else {
                    member.any()
                }
            })
            .collect::<Result<Vec<_>, _>>()?,
    ))
}

/// `SkillInvocation.model_dump(...)`, including its `mode="before"` rule.
///
/// `SkillInvocation._force_model_invocable_off` rewrites the model whenever
/// `side_effects` is declared, so the dump carries `model_invocable: false` even
/// if the author wrote `true`. Absent fields take the model defaults.
fn dump_skill_invocation(input: Input<'_>) -> Option<PythonJsonValue> {
    let field = |name: &str| input.field(name).filter(|value| !value.is_null());
    let user_invocable = field("user_invocable")
        .and_then(Input::boolean)
        .unwrap_or(true);
    let declared_side_effects = field("side_effects")
        .and_then(Input::sequence)
        .is_some_and(|members| !members.is_empty());
    let side_effects = match field("side_effects").and_then(Input::sequence) {
        Some(members) => text_array(members)?,
        None => PythonJsonValue::Array(Vec::new()),
    };
    let model_invocable = if declared_side_effects {
        false
    } else {
        field("model_invocable")
            .and_then(Input::boolean)
            .unwrap_or(true)
    };
    Some(exact_object(vec![
        (
            PythonString::from_scalars("user_invocable"),
            PythonJsonValue::Bool(user_invocable),
        ),
        (
            PythonString::from_scalars("model_invocable"),
            PythonJsonValue::Bool(model_invocable),
        ),
        (PythonString::from_scalars("side_effects"), side_effects),
    ]))
}

/// The `SkillInvocation` model defaults (no authored mapping).
fn default_skill_invocation() -> PythonJsonValue {
    exact_object(vec![
        (
            PythonString::from_scalars("user_invocable"),
            PythonJsonValue::Bool(true),
        ),
        (
            PythonString::from_scalars("model_invocable"),
            PythonJsonValue::Bool(true),
        ),
        (
            PythonString::from_scalars("side_effects"),
            PythonJsonValue::Array(Vec::new()),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::canonical_artifact_body;
    use crate::context_render::include_full_payload_lines_exact;
    use crate::offering::value::ArtifactDocument;
    use sk_kernel::python_yaml::marker_for_exact;

    fn read_fixture(text: &str) -> ArtifactDocument {
        let directory = std::env::temp_dir().join(format!(
            "cf38-exact-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&directory).expect("fixture directory");
        let path = directory.join("artifact.yaml");
        std::fs::write(&path, text).expect("fixture write");
        ArtifactDocument::read(&path).expect("artifact read")
    }

    /// The reference `Full artifact:` block of one canonical-graph document.
    fn graph_payload(kind: ArtifactKind, source: &str) -> Vec<String> {
        let document = read_fixture(source);
        let payload = canonical_artifact_body_exact(kind, document.root()).expect("exact payload");
        include_full_payload_lines_exact(&payload)
    }

    fn exact_value_payload(kind: ArtifactKind, value: &Value) -> Option<Vec<String>> {
        canonical_artifact_body_exact_value(kind, value)
            .map(|payload| include_full_payload_lines_exact(&payload))
    }

    fn assert_lines(lines: &[String], expected: &[&str]) {
        for line in expected {
            assert!(
                lines.iter().any(|item| item == line),
                "missing line {line:?} in\n{}",
                lines.join("\n")
            );
        }
    }

    const GLOSSARY_EDGES: &str = "\
id: probe-core
provenance: probe
terms:
  - surface: nan-term
    definition: nan confidence
    confidence: .nan
    status: active
  - surface: posinf-term
    definition: positive infinity
    confidence: .inf
    status: active
  - surface: negzero-term
    definition: signed zero
    confidence: -0.0
    status: active
  - surface: big-term
    definition: large float
    confidence: 1.2345678901234568e+300
    status: active
  - surface: surrogate-term
    definition: \"lone \\ud800 unit\"
    confidence: 0.5
    status: active
";

    #[test]
    fn typed_floats_and_exact_codepoints_survive_the_exact_payload() {
        // Every expected line below is the pinned Source child's own text
        // (`/tmp/cf38-source-probe.out`: model_dump(mode="json") then
        // json.dumps(indent=2, sort_keys=True, ensure_ascii=True), prefixed the
        // way the reference's `_format_full_artifact_payload_body` prefixes it).
        let lines = graph_payload(ArtifactKind::GlossaryPack, GLOSSARY_EDGES);
        assert_eq!(
            lines.first().map(String::as_str),
            Some("    Full artifact:")
        );
        assert_lines(
            &lines,
            &[
                "            \"confidence\": NaN,",
                "            \"confidence\": Infinity,",
                "            \"confidence\": -0.0,",
                "            \"confidence\": 1.2345678901234568e+300,",
                "            \"definition\": \"lone \\ud800 unit\",",
                "            \"surface\": \"surrogate-term\"",
            ],
        );
    }

    #[test]
    fn signed_infinity_is_exact_where_the_reader_carries_it() {
        // The bare carrier keeps the authored `-.inf` sign, and the walker
        // renders it exactly: `-Infinity`.
        let value: Value = serde_yaml_ng::from_str(
            "id: p\nprovenance: p\nterms:\n  - surface: s\n    definition: d\n    confidence: -.inf\n    status: active\n",
        )
        .expect("carrier parse");
        let lines = exact_value_payload(ArtifactKind::GlossaryPack, &value).expect("payload");
        assert_lines(&lines, &["            \"confidence\": -Infinity,"]);

        // The parent fixed the canonical constructor's double sign. The real
        // graph and bare carrier now agree with Source, not the old bug pin.
        let graph_lines = graph_payload(
            ArtifactKind::GlossaryPack,
            "id: p\nprovenance: p\nterms:\n  - surface: s\n    definition: d\n    confidence: -.inf\n    status: active\n",
        );
        assert_lines(&graph_lines, &["            \"confidence\": -Infinity,"]);
    }

    #[test]
    fn finite_scalar_adapter_keeps_its_finite_output_and_refuses_a_lossy_payload() {
        let finite = read_fixture(
            "id: p\nprovenance: p\nterms:\n  - surface: s\n    definition: d\n    confidence: 0.5\n    status: active\n",
        );
        let exact = canonical_artifact_body_exact(ArtifactKind::GlossaryPack, finite.root())
            .expect("exact payload");
        let adapter = canonical_artifact_body(ArtifactKind::GlossaryPack, &finite.value)
            .expect("finite adapter");
        assert_eq!(
            serde_json::Value::Object(adapter.as_object().expect("object").clone()),
            exact.to_serde().expect("scalar projection"),
            "the finite adapter is the scalar projection of the exact payload"
        );

        let lossy = read_fixture(GLOSSARY_EDGES);
        assert!(
            canonical_artifact_body_exact(ArtifactKind::GlossaryPack, lossy.root()).is_some(),
            "the exact payload carries the non-finite domain"
        );
        assert!(
            canonical_artifact_body(ArtifactKind::GlossaryPack, &lossy.value).is_none(),
            "the scalar adapter refuses instead of dropping a member"
        );
    }

    const STYLEGUIDE_CONSTRUCTED: &str = "\
id: probe-style
schema_version: '1.0'
title: Probe
scope: code
principles: [one]
tooling:
  runner: \"\\ud800-exact\"
structural_lint_config:
  set_value: !!set {alpha: null}
  pairs_value: !!pairs [{a: b}, {a: c}]
  omap_value: !!omap [{x: 1}, {y: 2}]
  bytes_value: !!binary SGVsbG8=
  list_nulls: [1, null, 2]
  map_nulls: {kept: 1, dropped: null}
  date_value: 2020-01-01
  datetime_value: 2020-01-01T01:02:03Z
  nested: {deep: [.nan, -.inf]}
  huge_int: 123456789012345678901234567890
";

    #[test]
    fn unstructured_constructed_values_follow_the_reference_projection() {
        let lines = graph_payload(ArtifactKind::Styleguide, STYLEGUIDE_CONSTRUCTED);
        assert_lines(
            &lines,
            &[
                "          \"bytes_value\": \"Hello\",",
                "          \"date_value\": \"2020-01-01\",",
                "          \"huge_int\": 123456789012345678901234567890,",
                "          \"list_nulls\": [",
                "            1,",
                "            null,",
                "            2",
                "          ],",
                "          \"map_nulls\": {",
                "            \"kept\": 1",
                "          },",
                "          \"nested\": {",
                "            \"deep\": [",
                "              null,",
                "              null",
                "            ]",
                "          },",
                "          \"omap_value\": {",
                "            \"x\": 1,",
                "            \"y\": 2",
                "          },",
                "          \"pairs_value\": [",
                "            [",
                "              \"a\",",
                "              \"b\"",
                "            ],",
                "            [",
                "              \"a\",",
                "              \"c\"",
                "            ]",
                "          ],",
                "          \"set_value\": [",
                "            \"alpha\"",
                "          ],",
                "        \"tooling\": {",
                "          \"runner\": \"\\ud800-exact\"",
                "        }",
                "        \"anti_patterns\": [],",
                "        \"patterns\": [],",
            ],
        );
        // `_jsonable_artifact_value` drops mapping nulls at every depth.
        assert!(!lines.iter().any(|line| line.contains("\"dropped\"")));
        // Source constructs a datetime, then Pydantic model_dump(mode="json")
        // spells aware UTC as Z. The constructor/resolver must not be changed.
        assert_lines(
            &lines,
            &["          \"datetime_value\": \"2020-01-01T01:02:03Z\","],
        );
    }

    #[test]
    fn multi_member_sets_keep_the_constructed_authority_order() {
        let document = read_fixture(&STYLEGUIDE_CONSTRUCTED.replace(
            "set_value: !!set {alpha: null}",
            "set_value: !!set {alpha: null, beta: null, gamma: null}",
        ));
        let payload = canonical_artifact_body_exact(ArtifactKind::Styleguide, document.root())
            .expect("exact payload");
        let lint = payload
            .field("structural_lint_config")
            .expect("structural_lint_config");
        let PythonJsonValue::Array(members) = lint.field("set_value").expect("set_value") else {
            panic!("a set is an array");
        };
        let authority = document
            .root()
            .lookup("structural_lint_config")
            .and_then(|node| node.lookup("set_value"))
            .and_then(NodeRef::as_set)
            .expect("constructed set");
        let root = document.root();
        let expected: Vec<String> = authority
            .iter()
            .filter_map(|id| root.member(*id).as_str())
            .map(|text| text.to_scalar_string().expect("ascii"))
            .collect();
        let rendered: Vec<String> = members
            .iter()
            .map(|member| {
                member
                    .to_serde()
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .expect("scalar member")
            })
            .collect();
        assert_eq!(
            rendered, expected,
            "set order is the authority's, never sorted or guessed"
        );
    }

    #[test]
    fn markers_refuse_the_payload_instead_of_printing_marker_text() {
        let marker = marker_for_exact(
            &PythonString::from_codepoints(&[0x61, 0xd800]).expect("exact python string"),
        );
        let term = Value::Mapping(
            [
                ("surface", Value::String("s".to_owned())),
                ("definition", Value::String(marker.clone())),
                ("confidence", Value::from(0.5_f64)),
                ("status", Value::String("active".to_owned())),
            ]
            .into_iter()
            .map(|(key, value)| (Value::String(key.to_owned()), value))
            .collect(),
        );
        let value = Value::Mapping(
            [
                ("id", Value::String("p".to_owned())),
                ("provenance", Value::String("p".to_owned())),
                ("terms", Value::Sequence(vec![term])),
            ]
            .into_iter()
            .map(|(key, value)| (Value::String(key.to_owned()), value))
            .collect(),
        );
        assert!(
            canonical_artifact_body_exact_value(ArtifactKind::GlossaryPack, &value).is_none(),
            "a transported marker is not the authored code points"
        );
        assert!(canonical_artifact_body(ArtifactKind::GlossaryPack, &value).is_none());
    }

    #[test]
    fn model_defaults_and_nested_nulls_follow_the_reference() {
        let directive = graph_payload(
            ArtifactKind::Directive,
            "id: DIRECTIVE_001\nschema_version: '1.0'\ntitle: T\nintent: I\nenforcement: required\nscope: null\n",
        );
        assert_lines(
            &directive,
            &[
                "        \"explicit_allowances\": [],",
                "        \"integrity_rules\": [],",
                "        \"procedures\": [],",
                "        \"references\": [],",
                "        \"validation_criteria\": []",
            ],
        );
        assert!(!directive.iter().any(|line| line.contains("\"scope\"")));

        let skill = graph_payload(
            ArtifactKind::Skill,
            "schema_version: '1.0'\nid: s\ntitle: T\ndescription: D\nform: prompt\nparameters:\n  - name: scope\ninvocation:\n  user_invocable: true\n  model_invocable: true\n  side_effects: [gh-write]\n",
        );
        assert_lines(
            &skill,
            &[
                "          \"model_invocable\": false,",
                "          \"user_invocable\": true",
                "          \"side_effects\": [",
                "            \"gh-write\"",
                "          ],",
                "            \"required\": false",
                "            \"description\": \"\",",
                "            \"name\": \"scope\",",
                "        \"tools\": [",
                "          \"*\"",
                "        ],",
                "        \"version\": \"1.0.0\"",
            ],
        );
    }

    #[test]
    fn big_integers_and_untyped_roots_are_exact() {
        let lines = graph_payload(
            ArtifactKind::Styleguide,
            "id: s\nschema_version: '1.0'\ntitle: T\nscope: code\nprinciples: [p]\nstructural_lint_config:\n  huge_int: 123456789012345678901234567890\n",
        );
        assert_lines(
            &lines,
            &["          \"huge_int\": 123456789012345678901234567890"],
        );
        assert!(
            canonical_artifact_body_exact_value(
                ArtifactKind::Asset,
                &serde_yaml_ng::from_str("id: a").expect("fixture")
            )
            .is_none()
        );
        assert!(
            canonical_artifact_body_exact_value(
                ArtifactKind::Paradigm,
                &serde_yaml_ng::from_str("- a").expect("fixture")
            )
            .is_none()
        );
    }
}
