//! Agent-profile loading, lineage composition and profile-channel delivery.
//!
//! Mirrors `src/charter/offering/agent_profiles/repository.py`,
//! `src/charter/offering/agent_profiles/profile.py` and
//! `src/charter/activation/profile_resolution.py`.
//!
//! **Identity is exact.** The reference model's `profile_id` (and every other
//! authored string) is a Python `str`, which may hold code points that a Rust
//! `String` cannot (a lone surrogate produced by a `\uD800`-style escape). The
//! loaded profile therefore keys on [`sk_kernel::PythonString`] and keeps the
//! authored body losslessly: see [`crate::profile_text`] for the transport. A
//! profile is **never** dropped, renamed or refused because its authored string
//! happens to be non-scalar; a scalar compatibility projection is offered
//! explicitly ([`AgentProfile::scalar_id`], [`ProfileBody::scalar_projection`])
//! and it is not the parity authority.
//!
//! Loading is layered exactly as the reference repository does it:
//!
//! * the built-in layer is read first and is the override *base*;
//! * an org or project profile with the same `profile-id` is **field-merged
//!   into** that built-in profile (nested mappings merged recursively, scalars
//!   and lists replaced), then schema-validated — a partial overlay keeps the
//!   built-in fields it does not mention;
//! * a file without a `profile-id` is skipped and recorded, never renamed
//!   after its file stem;
//! * a malformed or schema-invalid profile is skipped and recorded, so one bad
//!   file never blanks its valid siblings;
//! * `applies_to_languages` filters a scoped profile against the project's
//!   active languages (family-expanded), and a filtered id is recorded.
//!
//! `<<` merge keys are resolved before validation (the reference reader
//! flattens them; `serde_yaml_ng`'s self-describing `Value` keeps them
//! literal), with the reference precedence: an explicitly authored key wins,
//! and an earlier merge source wins over a later one.
//!
//! Lineage is read from the DRG (`agent_profile --specializes_from-->
//! agent_profile`) only; a missing parent, a cycle, or more than one declared
//! parent is a typed refusal. The resolving (leaf) profile's `excluding`
//! declaration is applied to the merged result.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_yaml_ng::{Mapping, Value};
use sk_kernel::PythonString;
use sk_kernel::python_yaml::{GraphDocument, NodeRef, ObjectGraph};

use crate::error::DoctrineError;
use crate::graph::{DrgGraph, NodeKind, Relation};
use crate::offering::{ConstructorAbort, NativeOfferingFailure};
use crate::profile_schema::Values;
use crate::profile_text::{self, ProfileTextError};
use crate::query::profile_channel_relations;

/// List-valued profile fields that inherit by union up the lineage chain.
pub const UNION_LIST_FIELDS: [&str; 5] = [
    "capabilities",
    "directive-references",
    "canonical-verbs",
    "mode-defaults",
    "tactic-references",
];

/// Fields rejected on any artifact as a retired inline reference channel.
const FORBIDDEN_INLINE_REFERENCE_FIELDS: [&str; 3] = ["tactic_refs", "paradigm_refs", "applies_to"];

/// Relationship fields retired in the DRG hard cutover.
const RETIRED_RELATIONSHIP_KEYS: [&str; 4] = [
    "specializes_from",
    "specializes-from",
    "enhances",
    "overrides",
];

/// The reference model's default `routing-priority`.
pub const DEFAULT_ROUTING_PRIORITY: i64 = 50;
/// The reference model's default `max-concurrent-tasks`.
pub const DEFAULT_MAX_CONCURRENT_TASKS: i64 = 5;
/// The reference model's default `schema-version`.
pub const DEFAULT_SCHEMA_VERSION: &str = "1.0";

/// Render an exact id for a diagnostic message.
///
/// The scalar projection is used when it exists (so ordinary ids read
/// normally); otherwise the escaped `repr` names the exact code points, which is
/// a truthful diagnostic and never a substituted id.
pub fn profile_id_label(id: &PythonString) -> String {
    match id.to_scalar_string() {
        Ok(scalar) => scalar,
        Err(_) => id.repr(),
    }
}

/// Lower-case a role the way the reference's `Role(value)` does.
///
/// Half-open custom roles use the same pinned CPython full/contextual mapping
/// as invocation routing, including Final Sigma and non-scalar code points.
pub(crate) fn lowercase_role(value: &PythonString) -> PythonString {
    PythonString::from_units(sk_kernel::python_text::lower_codepoints(value.codepoints()))
        .expect("lowercasing valid Python code points keeps them in range")
}

/// An exact Python `int` carried by a profile field.
///
/// The reference model's `int` fields are Python's arbitrary-precision `int`,
/// so a documented value such as `max-concurrent-tasks: 10**30` is valid data.
/// `i64` cannot hold it; projecting it to `i64` would either drop the digits or
/// substitute the declared default, and neither is the Source value. This type
/// is the canonical exact representation for those fields: it keeps the decimal
/// digits, compares exactly, and offers the `i64` projection as an explicit
/// capability view ([`ExactInteger::scalar`]).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExactInteger {
    /// A value `i64` can represent.
    InRange(i64),
    /// A magnitude outside `i64`, kept as exact decimal digits.
    Big {
        /// Whether the value is negative.
        negative: bool,
        /// The decimal magnitude, without a sign and without leading zeros.
        digits: String,
    },
}

impl ExactInteger {
    /// A value from an `i64`.
    pub fn from_i64(value: i64) -> Self {
        Self::InRange(value)
    }

    /// The exact value of an already-constructed YAML node, when it is an int.
    pub fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Number(number) => {
                if let Some(integer) = number.as_i64() {
                    return Some(Self::InRange(integer));
                }
                number.as_u64().map(|unsigned| Self::Big {
                    negative: false,
                    digits: unsigned.to_string(),
                })
            }
            other => crate::profile_construct::constructed_big_integer(other).map(Self::parse),
        }
    }

    /// The exact value of a decimal spelling with an optional sign.
    pub fn parse(text: &str) -> Self {
        match text.parse::<i64>() {
            Ok(value) => Self::InRange(value),
            Err(_) => {
                let (negative, body) = match text.strip_prefix('-') {
                    Some(body) => (true, body),
                    None => (false, text.strip_prefix('+').unwrap_or(text)),
                };
                let digits = body.trim_start_matches('0');
                if digits.is_empty() {
                    return Self::InRange(0);
                }
                Self::Big {
                    negative,
                    digits: digits.to_owned(),
                }
            }
        }
    }

    /// The value as `i64`, when it fits — the explicit scalar capability view.
    ///
    /// `None` is a real capability boundary (the authored value is wider than
    /// `i64`), never a substituted default.
    pub fn scalar(&self) -> Option<i64> {
        match self {
            Self::InRange(value) => Some(*value),
            Self::Big { negative, digits } => {
                let magnitude = digits.parse::<i128>().ok()?;
                let value = if *negative { -magnitude } else { magnitude };
                i64::try_from(value).ok()
            }
        }
    }

    /// The exact decimal spelling (CPython `str(int)`), sign included.
    pub fn decimal(&self) -> String {
        match self {
            Self::InRange(value) => value.to_string(),
            Self::Big { negative, digits } => {
                if *negative {
                    format!("-{digits}")
                } else {
                    digits.clone()
                }
            }
        }
    }

    /// Whether the value is greater than zero.
    pub fn is_positive(&self) -> bool {
        match self {
            Self::InRange(value) => *value > 0,
            Self::Big { negative, digits } => !*negative && digits != "0",
        }
    }

    /// Exact comparison against an `i64` bound.
    pub fn cmp_i64(&self, other: i64) -> std::cmp::Ordering {
        match self {
            Self::InRange(value) => value.cmp(&other),
            Self::Big { negative, .. } => {
                if *negative {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Greater
                }
            }
        }
    }
}

impl std::fmt::Display for ExactInteger {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.decimal())
    }
}

/// Why a profile load aborted: a real Source exception or a Native capability failure.
///
/// The reference's loader catches `(YAMLError, OSError)` per file. Anything else —
/// `KeyError` from an invalid `!!bool`, `ValueError` from `int()`/`float()` or an
/// impossible date, `TypeError` from an unhashable `!!set` key, `AssertionError`
/// from a duplicate `!!omap` key, or `AttributeError` from a non-mapping document —
/// propagates out of the layer load: the command exits nonzero, prints no list and
/// no partial success is observable. This is the canonical [`ConstructorAbort`]
/// carrier, not a second exception enum or class-string map.
///
/// A Native capability/start-up failure (an unusable `PYTHONHASHSEED`) is **not** a
/// Source Python class: CPython dies before user code, so it stays the CF83 typed
/// [`NativeOfferingFailure`] with its real phase, path and Kernel cause.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProfileLoadAbortCause {
    /// The Python exception the reference does not catch.
    Source(ConstructorAbort),
    /// A Native capability/start-up failure with no Source class.
    Native(NativeOfferingFailure),
}

impl std::fmt::Display for ProfileLoadAbortCause {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Source(abort) => std::fmt::Display::fmt(abort, formatter),
            Self::Native(native) => std::fmt::Display::fmt(native, formatter),
        }
    }
}

impl std::error::Error for ProfileLoadAbortCause {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Source(abort) => Some(abort),
            Self::Native(native) => Some(native),
        }
    }
}

/// A profile document that makes the reference crash while it loads.
///
/// The reference's loader catches `(YAMLError, OSError)` per file. Anything else
/// — `KeyError` from an invalid `!!bool`, `ValueError` from `int()`/`float()` or
/// an impossible date, `TypeError` from an unhashable `!!set` key,
/// `AssertionError` from a duplicate `!!omap` key, `AttributeError` from a
/// non-mapping document, or the fatal `PYTHONHASHSEED` line — propagates out of
/// the layer load: the command exits nonzero, prints no list and no partial
/// success is observable. This type carries that phase and cause.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileLoadAbort {
    /// The file whose construction raised.
    pub path: PathBuf,
    /// The typed cause (a real Source class or a Native capability failure).
    pub cause: ProfileLoadAbortCause,
}

impl ProfileLoadAbort {
    /// `ExceptionClass: message`, the last line of the reference's traceback.
    pub fn diagnostic(&self) -> String {
        self.cause.to_string()
    }
}

impl From<ProfileLoadAbort> for DoctrineError {
    fn from(abort: ProfileLoadAbort) -> Self {
        DoctrineError::ProfileLoadAborted {
            path: abort.path,
            cause: Box::new(abort.cause),
        }
    }
}

/// One file the repository refused to load, with the reason.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkippedProfile {
    /// The refused file.
    pub path: PathBuf,
    /// Its declared id, exactly, when one could be read.
    pub exact_profile_id: Option<PythonString>,
    /// The scalar projection of the declared id, when it has one.
    ///
    /// Kept as a separate field so existing scalar diagnostics consumers keep
    /// their shape; a non-scalar id is *not* projected lossily here.
    pub profile_id: Option<String>,
    /// Why it was skipped.
    pub summary: String,
}

impl SkippedProfile {
    /// The declared id's diagnostic label (scalar text or escaped `repr`).
    pub fn profile_id_label(&self) -> Option<String> {
        self.exact_profile_id.as_ref().map(profile_id_label)
    }
}

/// The authored profile body: exact keys and exact scalar values.
///
/// The `Mapping` inside is the semantic *transport* view: every lossy
/// double-quoted scalar is replaced by an invertible marker string, so
/// structure, typing, key identity and merge behaviour keep their exact
/// meaning. Use [`ProfileBody::string`], [`ProfileBody::strings`],
/// [`ProfileBody::keys`] and the other typed accessors to read authored values;
/// [`ProfileBody::shadow`] is deliberately not the data authority.
///
/// The body is *also* the canonical Kernel object graph of that same document
/// ([`ProfileBody::root`]), and that graph — not the tree — is what the schema
/// and model layers read. A body is always a document this crate constructed
/// (a `model_dump` view, or a merged model body); an authored file is read as a
/// [`GraphDocument`] directly, so a Source-valid cycle never has to be
/// materialised into this tree.
#[derive(Clone, Debug, Default)]
pub struct ProfileBody {
    shadow: Mapping,
    markers: BTreeMap<String, PythonString>,
    graph: Option<ObjectGraph>,
}

impl PartialEq for ProfileBody {
    fn eq(&self, other: &Self) -> bool {
        self.shadow == other.shadow && self.markers == other.markers
    }
}

impl ProfileBody {
    /// Build a body from a semantic mapping and its exact scalar table.
    pub fn from_parts(shadow: Mapping, markers: BTreeMap<String, PythonString>) -> Self {
        // The tree is the document this body *is*; the graph is built from it by
        // the ONE canonical constructor, so both views name the same objects.
        let document = Value::Mapping(shadow.clone());
        let graph = sk_kernel::python_yaml::build_from_value_with_markers(
            &document,
            &markers,
            profile_text::DIALECT,
        )
        .ok();
        Self {
            shadow,
            markers,
            graph,
        }
    }

    /// The constructed document's root as a borrowed canonical graph view.
    ///
    /// `None` is an explicit capability boundary: the mapping could not be
    /// canonicalised (a shape this crate never produces).
    pub fn root(&self) -> Option<NodeRef<'_>> {
        self.graph.as_ref().map(ObjectGraph::root_view)
    }

    /// The semantic transport view (markers in place of lossy scalars).
    ///
    /// Exposed for structural work (nested lookups, iteration); an authored
    /// string must be read through [`ProfileBody::string`] or
    /// [`ProfileBody::exact_string`].
    pub fn shadow(&self) -> &Mapping {
        &self.shadow
    }

    /// The exact values the transport recovered, keyed by marker.
    pub fn markers(&self) -> &BTreeMap<String, PythonString> {
        &self.markers
    }

    /// The exact value of a transport scalar (identity for a scalar string).
    pub fn exact_string(&self, value: &str) -> PythonString {
        match self.markers.get(value) {
            Some(exact) => exact.clone(),
            None => PythonString::from_scalars(value),
        }
    }

    /// A value by authored key.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.shadow.get(key)
    }

    /// Whether an authored key is present.
    pub fn contains_key(&self, key: &str) -> bool {
        self.shadow.contains_key(key)
    }

    /// A string field, exactly.
    pub fn string(&self, key: &str) -> Option<PythonString> {
        self.get(key)
            .and_then(Value::as_str)
            .map(|value| self.exact_string(value))
    }

    /// A string field's scalar projection (`None` when it is not a scalar).
    pub fn scalar_string(&self, key: &str) -> Option<String> {
        self.string(key)?.to_scalar_string().ok()
    }

    /// A list field's entries.
    pub fn list(&self, key: &str) -> Vec<&Value> {
        self.get(key)
            .and_then(Value::as_sequence)
            .map(|items| items.iter().collect())
            .unwrap_or_default()
    }

    /// A list field's string entries, exactly.
    pub fn strings(&self, key: &str) -> Vec<PythonString> {
        self.list(key)
            .into_iter()
            .filter_map(Value::as_str)
            .map(|value| self.exact_string(value))
            .collect()
    }

    /// A nested mapping's string field, exactly.
    pub fn nested_string(&self, key: &str, nested: &str) -> Option<PythonString> {
        self.get(key)
            .and_then(Value::as_mapping)
            .and_then(|mapping| mapping.get(nested))
            .and_then(Value::as_str)
            .map(|value| self.exact_string(value))
    }

    /// A nested mapping's list field, exactly.
    pub fn nested_strings(&self, key: &str, nested: &str) -> Vec<PythonString> {
        self.get(key)
            .and_then(Value::as_mapping)
            .and_then(|mapping| mapping.get(nested))
            .and_then(Value::as_sequence)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|value| self.exact_string(value))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// An integer field.
    pub fn integer(&self, key: &str) -> Option<i64> {
        self.get(key).and_then(Value::as_i64)
    }

    /// A boolean field.
    pub fn boolean(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(Value::as_bool)
    }

    /// The authored keys, in document order, exactly.
    pub fn keys(&self) -> Vec<PythonString> {
        self.shadow
            .keys()
            .filter_map(Value::as_str)
            .map(|key| self.exact_string(key))
            .collect()
    }

    /// The authored keys, in document order, as the transport spells them.
    pub fn shadow_keys(&self) -> Vec<String> {
        self.shadow
            .keys()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    }

    /// The scalar projection of the body, when every authored string is a
    /// Unicode scalar sequence.
    ///
    /// This is the explicit compatibility view for existing `serde`-shaped
    /// consumers. It is derived from the exact body — never a second read — and
    /// `None` means "this document has a non-scalar authored string", not
    /// "invalid".
    pub fn scalar_projection(&self) -> Option<Mapping> {
        project_mapping(&self.shadow, &self.markers)
    }

    /// The exact model-JSON view of this body.
    ///
    /// This is the reference's
    /// `_jsonable_artifact_value(model_dump(by_alias=True, mode="json",
    /// exclude_none=True))` for a body that already *is* the model dump: every
    /// authored code point survives (a lone surrogate is not a scalar string, so
    /// [`ProfileBody::scalar_projection`] refuses it while this view carries it),
    /// a `None` member is absent at every depth (`exclude_none=True`), and the
    /// container rules are the shared ones in [`crate::body::exact_json_value`].
    ///
    /// It is the exact **projection** only: the model schema, defaults and
    /// lineage stay with [`crate::profile_model`], which produced this body.
    /// [`PayloadError::NoSchema`] means the body has no canonical graph (a Native
    /// capability boundary); [`PayloadError::Failed`] carries the reference's real
    /// cause (an unpaired-surrogate mapping key, or non-UTF-8 `!!binary` bytes). A
    /// caller turns that into its own refusal rather than printing a substituted
    /// value. A conforming body has text keys only (model validation admitted every
    /// typed mapping), so the key codec is not exercised by normal data.
    ///
    /// # Errors
    ///
    /// [`PayloadError`] as above.
    pub fn model_json_exact(
        &self,
    ) -> Result<sk_kernel::python_json::PythonJsonValue, crate::body::PayloadError> {
        let Some(root) = self.root() else {
            return Err(crate::body::PayloadError::NoSchema);
        };
        crate::body::exact_json_value(root).map_err(crate::body::PayloadError::Failed)
    }

    /// Whether any authored scalar needed exact transport.
    pub fn has_non_scalar_strings(&self) -> bool {
        self.markers
            .values()
            .any(|exact| exact.to_scalar_string().is_err())
    }
}

fn project_mapping(mapping: &Mapping, markers: &BTreeMap<String, PythonString>) -> Option<Mapping> {
    let mut projected = Mapping::new();
    for (key, value) in mapping {
        projected.insert(project_value(key, markers)?, project_value(value, markers)?);
    }
    Some(projected)
}

fn project_value(value: &Value, markers: &BTreeMap<String, PythonString>) -> Option<Value> {
    match value {
        Value::String(text) => match markers.get(text) {
            Some(exact) => Some(Value::String(exact.to_scalar_string().ok()?)),
            None => Some(Value::String(text.clone())),
        },
        Value::Sequence(items) => {
            let mut projected = Vec::with_capacity(items.len());
            for item in items {
                projected.push(project_value(item, markers)?);
            }
            Some(Value::Sequence(projected))
        }
        Value::Mapping(mapping) => Some(Value::Mapping(project_mapping(mapping, markers)?)),
        Value::Tagged(tagged) => {
            let mut projected = (**tagged).clone();
            projected.value = project_value(&tagged.value, markers)?;
            Some(Value::Tagged(Box::new(projected)))
        }
        other => Some(other.clone()),
    }
}

/// One loaded agent profile, with its authored identity intact.
#[derive(Clone, Debug)]
pub struct AgentProfile {
    /// The profile id, exactly as authored.
    pub id: PythonString,
    /// The declaring file.
    pub path: PathBuf,
    /// The profile body after the layer merge (before lineage composition).
    pub body: ProfileBody,
    /// The lineage chain, root first, leaf last (empty before `resolve`).
    pub lineage: Vec<PythonString>,
}

impl AgentProfile {
    /// The reference `model_dump(by_alias=True)` of a loaded or resolved
    /// profile: every declared field, defaults filled.
    ///
    /// `body` carries the `exclude_unset` view used for lineage composition, so
    /// the defaults are reconstructed here rather than stored twice. A loaded
    /// profile is already validated, so this cannot fail; the `expect` documents
    /// that invariant rather than inventing a fallback shape.
    pub fn model_dump(&self) -> ProfileBody {
        crate::profile_model::normalize_body(&self.body)
            .expect("a loaded profile body satisfies the model")
            .full
    }

    /// [`AgentProfile::model_dump`] for the `exclude_unset` view (`= body`).
    pub fn model_dump_unset(&self) -> &ProfileBody {
        &self.body
    }

    /// The scalar projection of the id, when it has one.
    ///
    /// A compatibility view for scalar-only consumers; `None` is a real
    /// capability boundary, never a substituted identity.
    pub fn scalar_id(&self) -> Option<String> {
        self.id.to_scalar_string().ok()
    }

    /// The display name, when one is declared and non-empty.
    pub fn name(&self) -> Option<PythonString> {
        self.body.string("name").filter(|name| !name.is_empty())
    }

    /// The title, when one is declared and non-empty.
    ///
    /// Not part of the reference model (`extra="forbid"`), kept because the
    /// accessor predates it; a document that authors `title:` is skipped by
    /// validation exactly like the reference.
    pub fn title(&self) -> Option<PythonString> {
        self.body.string("title").filter(|title| !title.is_empty())
    }

    /// The purpose, when one is declared and non-empty.
    pub fn purpose(&self) -> Option<PythonString> {
        self.body
            .string("purpose")
            .filter(|purpose| !purpose.is_empty())
    }

    /// The normalized `roles` list (`Role(value)` lower-cases in the reference).
    pub fn roles(&self) -> Vec<PythonString> {
        self.body
            .strings("roles")
            .into_iter()
            .map(|role| lowercase_role(&role))
            .collect()
    }

    /// The primary role (`roles[0]`, normalized), when declared.
    pub fn role(&self) -> Option<PythonString> {
        self.roles()
            .into_iter()
            .next()
            .or_else(|| self.body.string("role").map(|role| lowercase_role(&role)))
    }

    /// The structural sentinel flag.
    pub fn sentinel(&self) -> bool {
        self.body.boolean("sentinel").unwrap_or(false)
    }

    /// `routing-priority` with the reference model's default applied.
    pub fn routing_priority(&self) -> i64 {
        self.body
            .integer("routing-priority")
            .or_else(|| self.body.integer("routing_priority"))
            .unwrap_or(DEFAULT_ROUTING_PRIORITY)
    }

    /// `max-concurrent-tasks` with the reference model's default applied,
    /// **exactly**.
    ///
    /// Python's `int` is arbitrary precision and `max-concurrent-tasks` has no
    /// upper bound, so a documented value may exceed `i64`. This accessor keeps
    /// the authored value whole instead of falling back to the model default
    /// (which would invent a Source value the profile never declared).
    pub fn max_concurrent_tasks_exact(&self) -> ExactInteger {
        match self
            .body
            .get("max-concurrent-tasks")
            .or_else(|| self.body.get("max_concurrent_tasks"))
        {
            Some(value) => ExactInteger::from_value(value)
                .unwrap_or(ExactInteger::InRange(DEFAULT_MAX_CONCURRENT_TASKS)),
            None => ExactInteger::InRange(DEFAULT_MAX_CONCURRENT_TASKS),
        }
    }

    /// `max-concurrent-tasks` as an `i64` — an **explicit scalar capability
    /// view**.
    ///
    /// `None` means the authored value is a Python `int` outside `i64`'s range:
    /// it is a real capability boundary of this view, never a substituted
    /// default. Use [`AgentProfile::max_concurrent_tasks_exact`] for the value.
    pub fn max_concurrent_tasks(&self) -> Option<i64> {
        self.max_concurrent_tasks_exact().scalar()
    }

    /// `schema-version` with the reference model's default applied.
    pub fn schema_version(&self) -> PythonString {
        self.body
            .string("schema-version")
            .or_else(|| self.body.string("schema_version"))
            .unwrap_or_else(|| PythonString::from_scalars(DEFAULT_SCHEMA_VERSION))
    }

    /// The `model:` routing hint.
    pub fn preferred_model(&self) -> Option<PythonString> {
        self.body
            .string("model")
            .or_else(|| self.body.string("preferred_model"))
    }

    /// The `effort:` routing hint.
    pub fn effort(&self) -> Option<PythonString> {
        self.body.string("effort")
    }

    /// The free-form description.
    pub fn description(&self) -> Option<PythonString> {
        self.body.string("description")
    }

    /// `specialization.primary-focus`, when declared.
    pub fn primary_focus(&self) -> Option<PythonString> {
        self.body
            .nested_string("specialization", "primary-focus")
            .or_else(|| self.body.nested_string("specialization", "primary_focus"))
    }

    /// `specialization-context.domain-keywords`.
    pub fn domain_keywords(&self) -> Vec<PythonString> {
        let mut keywords = self
            .body
            .nested_strings("specialization-context", "domain-keywords");
        if keywords.is_empty() {
            keywords = self
                .body
                .nested_strings("specialization_context", "domain_keywords");
        }
        keywords
    }

    /// `collaboration.canonical-verbs`.
    pub fn canonical_verbs(&self) -> Vec<PythonString> {
        self.body.nested_strings("collaboration", "canonical-verbs")
    }

    /// A list field's entries.
    pub fn list(&self, field: &str) -> Vec<&Value> {
        self.body.list(field)
    }

    /// A list field's string entries, exactly.
    pub fn strings(&self, field: &str) -> Vec<PythonString> {
        self.body.strings(field)
    }

    /// `applies_to_languages`, exactly.
    pub fn applies_to_languages(&self) -> Vec<PythonString> {
        self.body.strings("applies_to_languages")
    }

    /// Directive reference codes cited by this profile (including inherited ones).
    pub fn directive_references(&self) -> Vec<PythonString> {
        self.reference_ids("directive-references")
    }

    /// Tactic reference ids cited by this profile (including inherited ones).
    pub fn tactic_references(&self) -> Vec<PythonString> {
        self.reference_ids("tactic-references")
    }

    /// Reference ids in authored order (de-duplicated by first occurrence).
    ///
    /// The reference model keeps `*-references` in document order; sorting them
    /// here would silently reorder authored data, so order is preserved and only
    /// an exact duplicate is dropped.
    fn reference_ids(&self, field: &str) -> Vec<PythonString> {
        let ids: Vec<PythonString> = self
            .list(field)
            .into_iter()
            .filter_map(|item| {
                item.as_str()
                    .map(|text| self.body.exact_string(text))
                    .or_else(|| {
                        item.get("code")
                            .or_else(|| item.get("id"))
                            .and_then(Value::as_str)
                            .map(|text| self.body.exact_string(text))
                    })
            })
            .collect();
        let mut seen: Vec<PythonString> = Vec::with_capacity(ids.len());
        for id in ids {
            if !seen.contains(&id) {
                seen.push(id);
            }
        }
        seen
    }
}

/// A repository over the built-in, org and project profile layers.
#[derive(Clone, Debug, Default)]
pub struct ProfileRepository {
    layers: Vec<(usize, PathBuf, ProfileLayer)>,
    profiles: BTreeMap<PythonString, (AgentProfile, usize)>,
    skipped: Vec<SkippedProfile>,
    scope_filtered_ids: BTreeSet<String>,
    scope_filtered_exact_ids: BTreeSet<PythonString>,
    active_languages: Option<Vec<String>>,
}

/// The layer a profile was loaded from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileLayer {
    /// `packs/built-in/agent_profiles/`.
    BuiltIn,
    /// A configured org pack.
    Org,
    /// `.kittify/charter-packs/agent_profiles/`.
    Project,
}

impl ProfileLayer {
    /// The reference provenance label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BuiltIn => "built-in",
            Self::Org => "org",
            Self::Project => "project",
        }
    }
}

impl ProfileRepository {
    /// Load profiles from layers in overlay order: built-in, org packs in
    /// declaration order, then project.
    pub fn load(
        built_in_root: &Path,
        org_roots: &[PathBuf],
        project_root: Option<&Path>,
    ) -> Result<Self, DoctrineError> {
        Self::load_with_languages(built_in_root, org_roots, project_root, None)
    }

    /// Load with an active-language filter (`None` disables the filter).
    ///
    /// `*_root` arguments are *pack* roots: the profile directory of a layer is
    /// `<root>/agent_profiles`, exactly as the reference resolves
    /// `built_in_dir(ArtifactKind.AGENT_PROFILE)`.
    pub fn load_with_languages(
        built_in_root: &Path,
        org_roots: &[PathBuf],
        project_root: Option<&Path>,
        active_languages: Option<&[String]>,
    ) -> Result<Self, DoctrineError> {
        let org_directories: Vec<PathBuf> = org_roots
            .iter()
            .map(|root| root.join("agent_profiles"))
            .collect();
        let project_directory = project_root.map(|root| root.join("agent_profiles"));
        Self::load_profile_directories(
            &built_in_root.join("agent_profiles"),
            &org_directories,
            project_directory.as_deref(),
            active_languages,
        )
    }

    /// Load from explicit per-layer *profile directories*.
    ///
    /// This is the same loader with the layer directories supplied directly, for
    /// a caller whose directory is not a pack root — the reference's legacy
    /// `.kittify/profiles` invocation overlay is such a directory (`project_dir`
    /// points straight at it). No second parser, scan rule or merge policy is
    /// involved: [`ProfileRepository::load_with_languages`] is this function plus
    /// the `agent_profiles` suffix.
    pub fn load_profile_directories(
        built_in_dir: &Path,
        org_dirs: &[PathBuf],
        project_dir: Option<&Path>,
        active_languages: Option<&[String]>,
    ) -> Result<Self, DoctrineError> {
        let active_languages = active_languages.map(normalize_languages);
        let mut layers: Vec<(usize, PathBuf, ProfileLayer)> = Vec::new();
        layers.push((0, built_in_dir.to_path_buf(), ProfileLayer::BuiltIn));
        for (position, directory) in org_dirs.iter().enumerate() {
            layers.push((position + 1, directory.clone(), ProfileLayer::Org));
        }
        if let Some(project) = project_dir {
            layers.push((
                org_dirs.len() + 1,
                project.to_path_buf(),
                ProfileLayer::Project,
            ));
        }
        let mut repository = Self {
            layers,
            profiles: BTreeMap::new(),
            skipped: Vec::new(),
            scope_filtered_ids: BTreeSet::new(),
            scope_filtered_exact_ids: BTreeSet::new(),
            active_languages,
        };
        let mut built_in_bodies: BTreeMap<PythonString, ProfileBody> = BTreeMap::new();
        for (layer, directory, layer_name) in repository.layers.clone() {
            for path in sorted_files(&directory, ".agent.yaml") {
                repository.load_one(&path, layer, layer_name, &mut built_in_bodies)?;
            }
        }
        Ok(repository)
    }

    fn load_one(
        &mut self,
        path: &Path,
        layer: usize,
        layer_name: ProfileLayer,
        built_in_bodies: &mut BTreeMap<PythonString, ProfileBody>,
    ) -> Result<(), ProfileLoadAbort> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.skip(path, None, format!("YAML/read error: {error}"));
                return Ok(());
            }
        };
        // `yaml.load(path)` uses the canonical ruamel byte Reader, including
        // UTF-16 BOM admission and caught ReaderError decode failures. This is
        // distinct from the identity index's strict UTF-8 text probe.
        let text = match sk_kernel::python_yaml::read_document_text(&bytes, profile_text::DIALECT) {
            Ok(text) => text,
            Err(error) => return self.read_failure(path, error),
        };
        // ONE construction: the canonical Kernel object graph. Anchor/alias
        // sites become real graph links, so a Source-valid cycle (or shared
        // node) survives instead of being expanded or refused by a tree
        // projection. Transport, dialect, tag construction and merge-key
        // flattening are the same phases the reference's safe loader runs.
        let document = match sk_kernel::python_yaml::decode_graph(&text, profile_text::DIALECT) {
            Ok(graph) => GraphDocument::new(graph),
            Err(error) => return self.read_failure(path, error),
        };
        let fields = Values::single(document.root());
        if fields.is_null() {
            self.skip(
                path,
                None,
                "Empty profile file (no YAML document)".to_owned(),
            );
            return Ok(());
        }
        if !fields.is_mapping() {
            // `reject_inline_refs(data, …)` calls `data.get("id", "?")`, so a
            // document whose root is not a mapping raises `AttributeError`
            // inside the loader. The reference does not catch it: the whole
            // repository load aborts and no profile is listed.
            return Err(ProfileLoadAbort {
                path: path.to_path_buf(),
                cause: ProfileLoadAbortCause::Source(ConstructorAbort {
                    python: "AttributeError",
                    message: format!("'{}' object has no attribute 'get'", fields.type_name()),
                }),
            });
        };
        // The reference reads `data.get("profile-id") or data.get("profile_id")`
        // and requires the result to be truthy; a non-string id still reaches
        // model validation, where it is a type error.
        let declared_id = ["profile-id", "profile_id"]
            .iter()
            .find_map(|key| fields.field(key))
            .filter(|value| !value.is_falsy())
            .and_then(|value| value.as_str().cloned())
            .filter(|id| !id.is_empty());
        if RETIRED_RELATIONSHIP_KEYS
            .iter()
            .any(|key| fields.contains_key(key))
        {
            // The authored schema has no such property, so the reference's
            // summary is the JSON-schema additional-properties sentence.
            let summary = crate::profile_schema::agent_profile_schema_errors(&fields).join("; ");
            self.skip(path, declared_id, summary);
            return Ok(());
        }
        if let Some(field) = FORBIDDEN_INLINE_REFERENCE_FIELDS
            .into_iter()
            .find(|field| fields.contains_key(field))
        {
            self.skip(
                path,
                declared_id,
                format!(
                    "Forbidden inline-reference field '{field}'. Remove {field} from YAML; add edge \
                     {{source: agent_profile:?, target: <target-kind>:<target-id>, relation: requires}} \
                     to packs/built-in/agent_profile.graph.yaml"
                ),
            );
            return Ok(());
        }
        let Some(profile_id) = declared_id else {
            // `validate_agent_profile_yaml(data)` drives the wording; the
            // missing-id fallback is the reference's own sentence.
            let schema_errors = crate::profile_schema::agent_profile_schema_errors(&fields);
            let summary = if schema_errors.is_empty() {
                "Missing required 'profile-id'".to_owned()
            } else {
                schema_errors.join("; ")
            };
            self.skip(path, None, summary);
            return Ok(());
        };

        // Layer overlay: the reference deep-merges the raw overlay onto the
        // built-in *model dump* (every declared field with its default), then
        // validates the result. A fresh profile is validated on its own. The
        // merge is a read-time layer stack over the two graphs, so a cycle in an
        // ignored nested value is never materialised.
        let normalized = match (
            layer_name != ProfileLayer::BuiltIn,
            built_in_bodies.get(&profile_id),
        ) {
            (true, Some(base)) => crate::profile_model::normalize_overlay(&fields, base),
            _ => crate::profile_model::normalize(&fields),
        };
        let normalized = match normalized {
            Ok(normalized) => normalized,
            Err(error) => {
                // The reference's summary is the JSON-schema error list when it
                // has one, and the model error otherwise.
                let schema_errors = crate::profile_schema::agent_profile_schema_errors(&fields);
                let summary = if schema_errors.is_empty() {
                    error
                } else {
                    schema_errors.join("; ")
                };
                self.skip(path, Some(profile_id), summary);
                return Ok(());
            }
        };
        let applies: Vec<PythonString> = normalized.set.strings("applies_to_languages");
        if !applies_to_languages_match_exact(&applies, self.active_languages.as_deref()) {
            self.scope_filtered_ids
                .insert(profile_id_label(&profile_id));
            self.scope_filtered_exact_ids.insert(profile_id.clone());
            return Ok(());
        }
        let profile = AgentProfile {
            id: profile_id.clone(),
            path: path.to_path_buf(),
            body: normalized.set.clone(),
            lineage: Vec::new(),
        };
        if layer_name == ProfileLayer::BuiltIn {
            built_in_bodies.insert(profile_id.clone(), normalized.set);
        }
        self.scope_filtered_ids
            .remove(&profile_id_label(&profile_id));
        self.scope_filtered_exact_ids.remove(&profile_id);
        self.profiles.insert(profile_id, (profile, layer));
        Ok(())
    }

    /// Classify a read-phase failure the way the reference's loader does.
    ///
    /// A transport/constructor error the reference raises as a `YAMLError`
    /// skips this file; a bare Python exception aborts the whole load.
    fn read_failure(
        &mut self,
        path: &Path,
        error: ProfileTextError,
    ) -> Result<(), ProfileLoadAbort> {
        match error {
            ProfileTextError::Runtime { cause, message } => Err(ProfileLoadAbort {
                path: path.to_path_buf(),
                cause: ProfileLoadAbortCause::Source(ConstructorAbort {
                    python: cause.python_name(),
                    message,
                }),
            }),
            ProfileTextError::HashSeed(error) => Err(ProfileLoadAbort {
                path: path.to_path_buf(),
                cause: ProfileLoadAbortCause::Native(NativeOfferingFailure::ActivationSetOrder {
                    phase: "agent profile read".to_owned(),
                    path: path.to_path_buf(),
                    cause: error,
                }),
            }),
            other => {
                self.skip(path, None, format!("YAML/read error: {other}"));
                Ok(())
            }
        }
    }

    fn skip(&mut self, path: &Path, profile_id: Option<PythonString>, summary: String) {
        self.skipped.push(SkippedProfile {
            path: path.to_path_buf(),
            exact_profile_id: profile_id.clone(),
            profile_id: profile_id.and_then(|id| id.to_scalar_string().ok()),
            summary,
        });
    }

    /// Profiles that were refused, in load order.
    pub fn skipped(&self) -> &[SkippedProfile] {
        &self.skipped
    }

    /// Profile ids present but filtered out by `applies_to_languages` (scalar
    /// projection; see [`ProfileRepository::scope_filtered_exact_ids`]).
    pub fn scope_filtered_ids(&self) -> &BTreeSet<String> {
        &self.scope_filtered_ids
    }

    /// Profile ids present but filtered out by `applies_to_languages`, exactly.
    pub fn scope_filtered_exact_ids(&self) -> &BTreeSet<PythonString> {
        &self.scope_filtered_exact_ids
    }

    /// All profile ids, sorted.
    pub fn ids(&self) -> Vec<PythonString> {
        self.profiles.keys().cloned().collect()
    }

    /// All profile ids as their scalar projections.
    ///
    /// A compatibility view for scalar-only consumers (`Vec<String>` JSON
    /// arrays, CLI listings). It is *not* the identity authority: ids that have
    /// no scalar projection are omitted here and remain present in [`Self::ids`],
    /// so a caller that needs every profile uses the exact API.
    pub fn scalar_ids(&self) -> Vec<String> {
        self.profiles
            .keys()
            .filter_map(|id| id.to_scalar_string().ok())
            .collect()
    }

    /// The raw (uncomposed) profile record.
    pub fn get(&self, id: &PythonString) -> Option<&AgentProfile> {
        self.profiles.get(id).map(|(profile, _)| profile)
    }

    /// The raw record for a scalar id (a convenience over [`Self::get`]).
    pub fn get_str(&self, id: &str) -> Option<&AgentProfile> {
        self.get(&PythonString::from_scalars(id))
    }

    /// The provenance layer of `id`.
    pub fn layer_of(&self, id: &PythonString) -> Option<ProfileLayer> {
        self.profiles
            .get(id)
            .and_then(|(_, layer)| self.layers.iter().find(|(index, _, _)| index == layer))
            .map(|(_, _, layer)| *layer)
    }

    /// The provenance layer label of `id`.
    pub fn provenance(&self, id: &PythonString) -> Option<&'static str> {
        self.layer_of(id).map(ProfileLayer::as_str)
    }

    /// Resolve `id` with its `specializes_from` lineage composed and its own
    /// `excluding` declaration applied.
    ///
    /// A non-scalar id cannot appear in the `String`-keyed DRG, so its lineage
    /// has no parent — exactly what the reference observes when its exact
    /// `agent_profile:<id>` URN matches no edge. That is the authored graph's
    /// answer, not a refused capability: the profile itself resolves normally.
    pub fn resolve(
        &self,
        id: &PythonString,
        graph: &DrgGraph,
    ) -> Result<AgentProfile, DoctrineError> {
        let leaf = self
            .get(id)
            .ok_or_else(|| DoctrineError::ProfileNotFound {
                profile: profile_id_label(id),
            })?
            .clone();
        let leaf_id = leaf.id.clone();
        let leaf_path = leaf.path.clone();
        let mut chain: Vec<AgentProfile> = vec![leaf];
        let mut visited: BTreeSet<PythonString> = BTreeSet::from([chain[0].id.clone()]);
        loop {
            let current = chain.last().expect("chain is non-empty").id.clone();
            let parents = self.lineage_parents(&current, graph)?;
            let Some(parent_id) = parents.first().cloned() else {
                break;
            };
            if visited.contains(&parent_id) {
                let mut cycle: Vec<String> = visited.iter().map(profile_id_label).collect();
                cycle.push(profile_id_label(&parent_id));
                return Err(DoctrineError::LineageCycle { chain: cycle });
            }
            let parent = self
                .get(&parent_id)
                .ok_or_else(|| DoctrineError::MissingLineageParent {
                    profile: profile_id_label(&current),
                    parent: profile_id_label(&parent_id),
                })?
                .clone();
            visited.insert(parent_id);
            chain.push(parent);
        }

        let mut merged = ProfileBody::default();
        for profile in chain.iter().rev() {
            merged = union_merge(&merged, &profile.body);
        }
        if let Some(excluding) = chain[0].body.get("excluding") {
            merged = apply_excluding(merged, excluding);
        }
        // The reference re-validates the merged mapping (`model_validate`) and
        // returns the model, so `excluding` can never remove a required field
        // silently and the resolved body is canonical.
        let merged = crate::profile_model::normalize_body(&merged).map_err(|message| {
            DoctrineError::InvalidProfile {
                path: leaf_path.clone(),
                message,
            }
        })?;
        let lineage: Vec<PythonString> = chain
            .iter()
            .rev()
            .map(|profile| profile.id.clone())
            .collect();
        Ok(AgentProfile {
            id: leaf_id,
            path: leaf_path,
            body: merged.set,
            lineage,
        })
    }

    /// Resolve a scalar id (a convenience over [`Self::resolve`]).
    pub fn resolve_str(&self, id: &str, graph: &DrgGraph) -> Result<AgentProfile, DoctrineError> {
        self.resolve(&PythonString::from_scalars(id), graph)
    }

    fn lineage_parents(
        &self,
        profile_id: &PythonString,
        graph: &DrgGraph,
    ) -> Result<Vec<PythonString>, DoctrineError> {
        // `_lineage_parent` builds the URN from the exact Python id and reads
        // the graph; a graph URN is ASCII-only (its model rejects anything
        // else), so a non-scalar id can never name a node and has no parent.
        // The reference then sorts the targets and returns the *first* — it does
        // not refuse a profile that declares several parents, and it never
        // consults a retired inline relationship field.
        let Ok(scalar) = profile_id.to_scalar_string() else {
            return Ok(Vec::new());
        };
        let urn = format!("agent_profile:{scalar}");
        let mut parents: Vec<PythonString> = graph
            .edges_from(&urn, Some(Relation::SpecializesFrom))
            .into_iter()
            .map(|edge| {
                // `_profile_id_from_urn` splits at the first ':' only; it does
                // not verify the prefix, so an authored edge to another kind
                // still yields the same target id the reference derives.
                let target = edge.target();
                let body = target.split_once(':').map_or(target, |(_, body)| body);
                PythonString::from_scalars(body)
            })
            .collect();
        parents.sort();
        parents.truncate(1);
        Ok(parents)
    }

    /// The artefact URNs a profile reaches through the profile channel, with
    /// `when` clauses for `suggests` deliveries.
    pub fn channel_deliveries(
        &self,
        id: &PythonString,
        graph: &DrgGraph,
    ) -> Result<Vec<ProfileDelivery>, DoctrineError> {
        let profile = self.resolve(id, graph)?;
        let Ok(scalar) = profile.id.to_scalar_string() else {
            return Ok(Vec::new());
        };
        let urn = format!("agent_profile:{scalar}");
        if graph
            .node(&urn)
            .is_none_or(|node| node.kind() != NodeKind::AgentProfile)
        {
            return Ok(Vec::new());
        }
        let relations = profile_channel_relations();
        let mut reached: BTreeSet<String> = BTreeSet::from([urn.clone()]);
        let mut queue: Vec<String> = vec![urn.clone()];
        let mut deliveries = Vec::new();
        while let Some(current) = queue.pop() {
            for edge in graph.edges_from(&current, None) {
                if !relations.contains(&edge.relation()) {
                    continue;
                }
                deliveries.push(ProfileDelivery {
                    profile: scalar.clone(),
                    urn: edge.target().to_owned(),
                    relation: edge.relation(),
                    when: edge.when().map(str::to_owned),
                });
                if reached.insert(edge.target().to_owned()) {
                    queue.push(edge.target().to_owned());
                }
            }
        }
        deliveries.sort_by(|left, right| {
            (&left.urn, left.relation, &left.when).cmp(&(&right.urn, right.relation, &right.when))
        });
        deliveries.dedup();
        Ok(deliveries)
    }

    /// [`Self::channel_deliveries`] for a scalar id.
    pub fn channel_deliveries_str(
        &self,
        id: &str,
        graph: &DrgGraph,
    ) -> Result<Vec<ProfileDelivery>, DoctrineError> {
        self.channel_deliveries(&PythonString::from_scalars(id), graph)
    }
}

/// One profile-channel delivery.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct ProfileDelivery {
    /// The profile that reaches the artifact.
    pub profile: String,
    /// The reached artifact URN.
    pub urn: String,
    /// The relation by which it is reached.
    pub relation: Relation,
    /// The edge's `when` clause, for `suggests`.
    pub when: Option<String>,
}

/// Keep every exact value the merged body can still reference.
fn merge_marker_tables(
    first: &BTreeMap<String, PythonString>,
    second: &BTreeMap<String, PythonString>,
) -> BTreeMap<String, PythonString> {
    let mut merged = first.clone();
    for (marker, exact) in second {
        merged.insert(marker.clone(), exact.clone());
    }
    merged
}

/// Lineage union merge of a parent's and a child's model bodies.
///
/// This merges two **model documents** this crate constructed, not authored
/// input, so it stays a document-level operation; the merged body is
/// re-validated through the canonical graph by
/// [`crate::profile_model::normalize_body`].
fn union_merge(parent: &ProfileBody, child: &ProfileBody) -> ProfileBody {
    ProfileBody::from_parts(
        union_merge_mapping(parent.shadow(), child.shadow()),
        merge_marker_tables(parent.markers(), child.markers()),
    )
}

fn union_merge_mapping(parent: &Mapping, child: &Mapping) -> Mapping {
    let mut merged = parent.clone();
    for (key, child_value) in child {
        let key_name = key.as_str().unwrap_or_default();
        match (merged.get(key), child_value) {
            (Some(parent_value), _) if UNION_LIST_FIELDS.contains(&key_name) => {
                // A constructed `!!set`/`!!pairs` is a Python set/list of the same
                // items, so it unions exactly like an authored list (the reference
                // iterates the constructed value). The union itself is a plain
                // list, which is what the reference's merge stores.
                let parent_items = crate::profile_construct::constructed_sequence(parent_value);
                let child_items = crate::profile_construct::constructed_sequence(child_value);
                match (parent_items, child_items) {
                    (Some(parent_items), Some(child_items)) => {
                        let mut seen: BTreeSet<String> =
                            parent_items.iter().map(item_key).collect();
                        let mut union = parent_items.clone();
                        for item in child_items {
                            if seen.insert(item_key(item)) {
                                union.push(item.clone());
                            }
                        }
                        merged.insert(key.clone(), Value::Sequence(union));
                    }
                    _ => {
                        merged.insert(key.clone(), child_value.clone());
                    }
                }
            }
            (Some(parent_value), Value::Mapping(child_map))
                if crate::profile_construct::constructed_mapping(parent_value).is_some() =>
            {
                let parent_map = crate::profile_construct::constructed_mapping(parent_value)
                    .expect("guarded by the match arm")
                    .clone();
                // The reference `_union_merge` updates nested sections one
                // level deep; only the layer merge (`_merge_profiles`) recurses.
                let mut nested = parent_map.clone();
                for (nested_key, nested_value) in child_map {
                    nested.insert(nested_key.clone(), nested_value.clone());
                }
                merged.insert(key.clone(), Value::Mapping(nested));
            }
            _ => {
                merged.insert(key.clone(), child_value.clone());
            }
        }
    }
    merged
}

/// Apply a leaf profile's `excluding` declaration to its merged body.
///
/// A list form removes whole fields; a mapping form removes the named values
/// from the list field they name (matched by the reference item key). Merged
/// values keep their transport markers, so an exact (non-scalar) reference is
/// matched exactly rather than by a lossy projection.
fn apply_excluding(merged: ProfileBody, excluding: &Value) -> ProfileBody {
    let mut shadow = merged.shadow().clone();
    match excluding {
        Value::Sequence(fields) => {
            for field in fields {
                if let Some(field) = field.as_str() {
                    shadow.remove(field);
                }
            }
        }
        Value::Mapping(fields) => {
            for (field, values) in fields {
                let Some(field) = field.as_str() else {
                    continue;
                };
                let Some(values) = values.as_sequence() else {
                    continue;
                };
                let remove: BTreeSet<String> = values
                    .iter()
                    .map(|value| match value {
                        Value::String(text) => text.clone(),
                        other => format!("{other:?}"),
                    })
                    .collect();
                let Some(items) = shadow.get(field).and_then(Value::as_sequence) else {
                    continue;
                };
                let kept: Vec<Value> = items
                    .iter()
                    .filter(|item| !remove.contains(&item_key(item)))
                    .cloned()
                    .collect();
                shadow.insert(field.into(), Value::Sequence(kept));
            }
        }
        _ => {}
    }
    // Only the excluded entries disappear; a marker for a value that is no
    // longer referenced is harmless but prune the unused ones so two
    // semantically equal bodies also compare equal.
    let used = used_markers(&Value::Mapping(shadow.clone()));
    let markers = merged
        .markers()
        .iter()
        .filter(|(marker, _)| used.contains(*marker))
        .map(|(marker, exact)| (marker.clone(), exact.clone()))
        .collect();
    ProfileBody::from_parts(shadow, markers)
}

/// Every transport marker still referenced by `value`.
fn used_markers(value: &Value) -> BTreeSet<String> {
    let mut used = BTreeSet::new();
    collect_markers(value, &mut used);
    used
}

fn collect_markers(value: &Value, used: &mut BTreeSet<String>) {
    match value {
        Value::String(text) => {
            if text.starts_with('\u{fdd0}') && text.ends_with('\u{fdef}') {
                used.insert(text.clone());
            }
        }
        Value::Sequence(items) => {
            for item in items {
                collect_markers(item, used);
            }
        }
        Value::Mapping(mapping) => {
            for (key, item) in mapping {
                collect_markers(key, used);
                collect_markers(item, used);
            }
        }
        Value::Tagged(tagged) => collect_markers(&tagged.value, used),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

/// Normalize a language list: lowercase, de-duplicated, non-empty.
pub fn normalize_languages(values: &[String]) -> Vec<String> {
    let mut normalized = Vec::new();
    for value in values {
        let item = value.trim().to_lowercase();
        if !item.is_empty() && !normalized.contains(&item) {
            normalized.push(item);
        }
    }
    normalized
}

/// Whether a profile scoped to `artifact_languages` loads for `active`.
///
/// Unscoped profiles always load; `None` active disables filtering; an
/// explicitly empty/`unknown` active set admits no scoped profile; `any`/`all`
/// sentinels make the profile unscoped (defense in depth against a file that
/// bypassed authoring validation). Family expansion and the reserved/sentinel
/// token sets are the canonical `language_scope` authorities, never a local
/// copy.
pub fn applies_to_languages_match(
    artifact_languages: &[String],
    active_languages: Option<&[String]>,
) -> bool {
    let exact: Vec<PythonString> = artifact_languages
        .iter()
        .map(|language| PythonString::from_scalars(language))
        .collect();
    applies_to_languages_match_exact(&exact, active_languages)
}

/// [`applies_to_languages_match`] over exact authored code points.
///
/// The scope tokens themselves are matched through their scalar projection: the
/// language vocabulary is a closed ASCII token set, so a non-scalar scope token
/// can never match an active language (which is the reference's own string
/// comparison result).
pub fn applies_to_languages_match_exact(
    artifact_languages: &[PythonString],
    active_languages: Option<&[String]>,
) -> bool {
    let raw: BTreeSet<String> = normalize_languages(
        &artifact_languages
            .iter()
            .filter_map(|token| token.to_scalar_string().ok())
            .collect::<Vec<_>>(),
    )
    .into_iter()
    .collect();
    if raw.is_empty() {
        return artifact_languages.is_empty();
    }
    let artifact_scope: BTreeSet<String> = raw
        .into_iter()
        .filter(|token| !crate::language_scope::RESERVED_LANGUAGE_TOKENS.contains(&token.as_str()))
        .collect();
    if artifact_scope.is_empty() {
        return false;
    }
    if artifact_scope
        .iter()
        .all(|token| crate::language_scope::SENTINEL_LANGUAGE_TOKENS.contains(&token.as_str()))
    {
        return true;
    }
    let Some(active) = active_languages else {
        return true;
    };
    let active_scope: BTreeSet<String> = normalize_languages(active)
        .into_iter()
        .filter(|token| !crate::language_scope::RESERVED_LANGUAGE_TOKENS.contains(&token.as_str()))
        .collect();
    if active_scope.is_empty() {
        return false;
    }
    let expanded: BTreeSet<String> =
        crate::language_scope::expand_language_families(active_scope.iter().map(String::as_str))
            .into_iter()
            .collect();
    !artifact_scope.is_disjoint(&expanded)
}

fn item_key(item: &Value) -> String {
    if let Some(mapping) = item.as_mapping() {
        if let Some(code) = mapping.get("code").and_then(Value::as_str) {
            return code.to_owned();
        }
        if let Some(id) = mapping.get("id").and_then(Value::as_str) {
            return id.to_owned();
        }
    }
    match item {
        Value::String(text) => text.clone(),
        other => format!("{other:?}"),
    }
}

fn sorted_files(directory: &Path, suffix: &str) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_files(directory, suffix, &mut files);
    files.sort();
    files
}

fn collect_files(directory: &Path, suffix: &str, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, suffix, files);
        } else if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(suffix))
        {
            files.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{DrgEdge, DrgNode};
    use std::fs;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn profile(id: &str, body: &str) -> String {
        format!(
            "profile-id: {id}\nname: {id}\npurpose: p\nroles: [reviewer]\nspecialization:\n  primary-focus: review\n{body}"
        )
    }

    fn scalar(value: Option<PythonString>) -> Option<String> {
        value.and_then(|value| value.to_scalar_string().ok())
    }

    fn graph(edges: Vec<DrgEdge>) -> DrgGraph {
        let mut nodes = vec![
            DrgNode::new(
                "agent_profile:base",
                NodeKind::AgentProfile,
                None,
                Vec::new(),
            )
            .unwrap(),
            DrgNode::new(
                "agent_profile:leaf",
                NodeKind::AgentProfile,
                None,
                Vec::new(),
            )
            .unwrap(),
            DrgNode::new("directive:001", NodeKind::Directive, None, Vec::new()).unwrap(),
        ];
        nodes.sort_by(|left, right| left.urn().cmp(right.urn()));
        DrgGraph::new("1.0", "x", "t", nodes, edges).unwrap()
    }

    #[test]
    fn review_regression_all_pinned_builtin_profiles_load() {
        let built_in = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs/built-in");
        let profiles = ProfileRepository::load(&built_in, &[], None).unwrap();
        assert!(profiles.skipped().is_empty(), "{:?}", profiles.skipped());
        assert_eq!(profiles.ids().len(), 26);
        for id in [
            "drupal-dries",
            "frontend-freddy",
            "java-jenny",
            "node-norris",
            "python-pedro",
        ] {
            assert!(profiles.get_str(id).is_some(), "missing {id}");
        }
    }

    #[test]
    fn same_id_overlay_deep_merges_into_the_built_in_profile() {
        let directory = tempfile::tempdir().unwrap();
        let built_in = directory.path().join("built-in");
        let org = directory.path().join("org");
        write(
            &built_in.join("agent_profiles/ivan.agent.yaml"),
            &profile(
                "ivan",
                "description: kept\ncapabilities: [a, b]\nspecialization-context:\n  languages: [python]\n  frameworks: [django]\n",
            ),
        );
        write(
            &org.join("agent_profiles/ivan.agent.yaml"),
            "profile-id: ivan\ndescription: Org\nspecialization-context:\n  frameworks: [flask]\n",
        );
        let repository = ProfileRepository::load(&built_in, &[org], None).unwrap();
        let loaded = repository.get_str("ivan").unwrap();
        assert_eq!(scalar(loaded.name()).as_deref(), Some("ivan"));
        assert_eq!(
            loaded
                .description()
                .as_ref()
                .and_then(|d| scalar(Some(d.clone())))
                .as_deref(),
            Some("Org")
        );
        assert_eq!(loaded.list("capabilities").len(), 2);
        let context = loaded.body.get("specialization-context").unwrap();
        assert_eq!(
            context
                .get("languages")
                .unwrap()
                .as_sequence()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            context.get("frameworks").unwrap().as_sequence().unwrap()[0].as_str(),
            Some("flask")
        );
        assert_eq!(
            repository.provenance(&PythonString::from_scalars("ivan")),
            Some("org")
        );
    }

    #[test]
    fn missing_ids_and_invalid_schemas_are_skipped_not_renamed() {
        let directory = tempfile::tempdir().unwrap();
        let built_in = directory.path().join("built-in");
        write(
            &built_in.join("agent_profiles/valid.agent.yaml"),
            &profile("valid", ""),
        );
        write(
            &built_in.join("agent_profiles/stem-named.agent.yaml"),
            "name: No id\npurpose: p\nroles: [reviewer]\nspecialization:\n  primary-focus: x\n",
        );
        write(
            &built_in.join("agent_profiles/unknown-field.agent.yaml"),
            &profile("unknown-field", "mystery: true\n"),
        );
        write(
            &built_in.join("agent_profiles/both-roles.agent.yaml"),
            "profile-id: both-roles\nname: B\npurpose: p\nrole: reviewer\nroles: [reviewer]\nspecialization:\n  primary-focus: x\n",
        );
        let repository = ProfileRepository::load(&built_in, &[], None).unwrap();
        assert!(repository.get_str("valid").is_some());
        assert!(repository.get_str("stem-named").is_none());
        assert!(repository.get_str("unknown-field").is_none());
        assert!(repository.get_str("both-roles").is_none());
        let summaries: Vec<&str> = repository
            .skipped()
            .iter()
            .map(|skip| skip.summary.as_str())
            .collect();
        assert!(
            summaries
                .iter()
                .any(|summary| summary.contains("profile-id"))
        );
        assert!(
            summaries.iter().any(|summary| summary
                .contains("Additional properties are not allowed ('mystery' was unexpected)")),
            "the reference's JSON-schema sentence is the skip summary: {summaries:?}"
        );
        assert!(
            summaries.iter().any(|summary| summary.contains("role")),
            "the both-role model error keeps its own sentence: {summaries:?}"
        );
    }

    #[test]
    fn language_scoping_filters_and_records_the_drop() {
        let directory = tempfile::tempdir().unwrap();
        let built_in = directory.path().join("built-in");
        write(
            &built_in.join("agent_profiles/python.agent.yaml"),
            &profile("python", "applies_to_languages: [python]\n"),
        );
        write(
            &built_in.join("agent_profiles/jvm.agent.yaml"),
            &profile("jvm", "applies_to_languages: [jvm]\n"),
        );
        write(
            &built_in.join("agent_profiles/unscoped.agent.yaml"),
            &profile("unscoped", ""),
        );
        let active = vec!["kotlin".to_owned()];
        let repository =
            ProfileRepository::load_with_languages(&built_in, &[], None, Some(&active)).unwrap();
        assert!(repository.get_str("jvm").is_some());
        assert!(repository.get_str("unscoped").is_some());
        assert!(repository.get_str("python").is_none());
        assert!(repository.scope_filtered_ids().contains("python"));

        let unknown = vec!["unknown".to_owned()];
        let repository =
            ProfileRepository::load_with_languages(&built_in, &[], None, Some(&unknown)).unwrap();
        assert!(repository.get_str("jvm").is_none());
        assert!(repository.get_str("unscoped").is_some());
    }

    #[test]
    fn lineage_composes_references_from_root_to_leaf() {
        let directory = tempfile::tempdir().unwrap();
        let built_in = directory.path().join("built-in");
        write(
            &built_in.join("agent_profiles/base.agent.yaml"),
            &profile(
                "base",
                "directive-references:\n  - code: '001'\n    name: one\n    rationale: base\ncapabilities: [a]\n",
            ),
        );
        write(
            &built_in.join("agent_profiles/leaf.agent.yaml"),
            &profile(
                "leaf",
                "directive-references:\n  - code: '002'\n    name: two\n    rationale: leaf\ncapabilities: [b]\ndescription: leaf\n",
            ),
        );
        let repository = ProfileRepository::load(&built_in, &[], None).unwrap();
        let graph = graph(vec![
            DrgEdge::new(
                "agent_profile:leaf",
                "agent_profile:base",
                Relation::SpecializesFrom,
            )
            .unwrap(),
        ]);
        let profile = repository.resolve_str("leaf", &graph).unwrap();
        assert_eq!(
            profile
                .lineage
                .iter()
                .filter_map(|id| id.to_scalar_string().ok())
                .collect::<Vec<_>>(),
            vec!["base".to_owned(), "leaf".to_owned()]
        );
        assert_eq!(
            profile
                .directive_references()
                .iter()
                .filter_map(|id| id.to_scalar_string().ok())
                .collect::<Vec<_>>(),
            vec!["001".to_owned(), "002".to_owned()]
        );
        assert_eq!(profile.list("capabilities").len(), 2);
        assert_eq!(scalar(profile.purpose()).as_deref(), Some("p"));
        assert_eq!(
            profile
                .description()
                .and_then(|d| d.to_scalar_string().ok())
                .as_deref(),
            Some("leaf")
        );
    }

    #[test]
    fn excluding_removes_fields_or_values_from_the_merged_result() {
        let directory = tempfile::tempdir().unwrap();
        let built_in = directory.path().join("built-in");
        write(
            &built_in.join("agent_profiles/base.agent.yaml"),
            &profile("base", "capabilities: [a, b]\ndescription: inherited\n"),
        );
        write(
            &built_in.join("agent_profiles/list.agent.yaml"),
            &profile("list", "excluding: [description]\n"),
        );
        write(
            &built_in.join("agent_profiles/map.agent.yaml"),
            "profile-id: map\nname: map\npurpose: p\nroles: [reviewer]\nspecialization:\n  primary-focus: review\ncapabilities: [a, b]\nexcluding:\n  capabilities: [a]\n",
        );
        let repository = ProfileRepository::load(&built_in, &[], None).unwrap();
        let graph = DrgGraph::new(
            "1.0",
            "x",
            "t",
            vec![
                DrgNode::new(
                    "agent_profile:base",
                    NodeKind::AgentProfile,
                    None,
                    Vec::new(),
                )
                .unwrap(),
                DrgNode::new(
                    "agent_profile:list",
                    NodeKind::AgentProfile,
                    None,
                    Vec::new(),
                )
                .unwrap(),
                DrgNode::new(
                    "agent_profile:map",
                    NodeKind::AgentProfile,
                    None,
                    Vec::new(),
                )
                .unwrap(),
            ],
            vec![
                DrgEdge::new(
                    "agent_profile:list",
                    "agent_profile:base",
                    Relation::SpecializesFrom,
                )
                .unwrap(),
                DrgEdge::new(
                    "agent_profile:map",
                    "agent_profile:base",
                    Relation::SpecializesFrom,
                )
                .unwrap(),
            ],
        )
        .unwrap();
        let list = repository.resolve_str("list", &graph).unwrap();
        assert!(list.body.get("description").is_none());
        let mapped = repository.resolve_str("map", &graph).unwrap();
        let capabilities: Vec<&str> = mapped
            .list("capabilities")
            .into_iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(capabilities, ["b"]);
    }

    #[test]
    fn missing_parent_cycle_and_ambiguity_are_refused() {
        let directory = tempfile::tempdir().unwrap();
        let built_in = directory.path().join("built-in");
        write(
            &built_in.join("agent_profiles/leaf.agent.yaml"),
            &profile("leaf", ""),
        );
        let repository = ProfileRepository::load(&built_in, &[], None).unwrap();
        let missing = graph(vec![
            DrgEdge::new(
                "agent_profile:leaf",
                "agent_profile:absent",
                Relation::SpecializesFrom,
            )
            .unwrap(),
        ]);
        assert!(matches!(
            repository.resolve_str("leaf", &missing),
            Err(DoctrineError::MissingLineageParent { .. })
        ));

        write(
            &built_in.join("agent_profiles/base.agent.yaml"),
            &profile("base", ""),
        );
        let repository = ProfileRepository::load(&built_in, &[], None).unwrap();
        let cycle = graph(vec![
            DrgEdge::new(
                "agent_profile:leaf",
                "agent_profile:base",
                Relation::SpecializesFrom,
            )
            .unwrap(),
            DrgEdge::new(
                "agent_profile:base",
                "agent_profile:leaf",
                Relation::SpecializesFrom,
            )
            .unwrap(),
        ]);
        assert!(matches!(
            repository.resolve_str("leaf", &cycle),
            Err(DoctrineError::LineageCycle { .. })
        ));
        write(
            &built_in.join("agent_profiles/other.agent.yaml"),
            &profile("other", ""),
        );
        let repository = ProfileRepository::load(&built_in, &[], None).unwrap();
        let graph = DrgGraph::new(
            "1.0",
            "x",
            "t",
            vec![
                DrgNode::new(
                    "agent_profile:leaf",
                    NodeKind::AgentProfile,
                    None,
                    Vec::new(),
                )
                .unwrap(),
                DrgNode::new(
                    "agent_profile:base",
                    NodeKind::AgentProfile,
                    None,
                    Vec::new(),
                )
                .unwrap(),
                DrgNode::new(
                    "agent_profile:other",
                    NodeKind::AgentProfile,
                    None,
                    Vec::new(),
                )
                .unwrap(),
            ],
            vec![
                DrgEdge::new(
                    "agent_profile:leaf",
                    "agent_profile:base",
                    Relation::SpecializesFrom,
                )
                .unwrap(),
                DrgEdge::new(
                    "agent_profile:leaf",
                    "agent_profile:other",
                    Relation::SpecializesFrom,
                )
                .unwrap(),
            ],
        )
        .unwrap();
        // The reference sorts the declared parents and takes the first; it does
        // not refuse a profile that declares more than one.
        let resolved = repository.resolve_str("leaf", &graph).unwrap();
        assert_eq!(
            resolved
                .lineage
                .iter()
                .filter_map(|id| id.to_scalar_string().ok())
                .collect::<Vec<_>>(),
            vec!["base".to_owned(), "leaf".to_owned()]
        );
    }

    #[test]
    fn profile_channel_reports_suggests_when_clauses() {
        let directory = tempfile::tempdir().unwrap();
        let built_in = directory.path().join("built-in");
        write(
            &built_in.join("agent_profiles/reviewer.agent.yaml"),
            &profile("reviewer", ""),
        );
        let repository = ProfileRepository::load(&built_in, &[], None).unwrap();
        let graph = DrgGraph::new(
            "1.0",
            "x",
            "t",
            vec![
                DrgNode::new(
                    "agent_profile:reviewer",
                    NodeKind::AgentProfile,
                    None,
                    Vec::new(),
                )
                .unwrap(),
                DrgNode::new("tactic:deep-review", NodeKind::Tactic, None, Vec::new()).unwrap(),
            ],
            vec![
                DrgEdge::new(
                    "agent_profile:reviewer",
                    "tactic:deep-review",
                    Relation::Suggests,
                )
                .unwrap()
                .with_when("high-risk review"),
            ],
        )
        .unwrap();
        let deliveries = repository
            .channel_deliveries_str("reviewer", &graph)
            .unwrap();
        assert_eq!(deliveries.len(), 1);
        assert_eq!(deliveries[0].when.as_deref(), Some("high-risk review"));
    }

    #[test]
    fn a_source_valid_non_scalar_id_loads_with_its_exact_code_points() {
        let directory = tempfile::tempdir().unwrap();
        let built_in = directory.path().join("built-in");
        write(
            &built_in.join("agent_profiles/author.agent.yaml"),
            "profile-id: \"author\\ud800\"\nname: Source-valid profile\nroles: [Reviewer]\npurpose: Preserve authored routing identities.\nspecialization:\n  primary-focus: Review\ncollaboration:\n  canonical-verbs: [check]\nspecialization-context:\n  domain-keywords: [widget]\n",
        );
        let repository = ProfileRepository::load(&built_in, &[], None).unwrap();
        assert!(
            repository.skipped().is_empty(),
            "{:?}",
            repository.skipped()
        );
        assert_eq!(repository.ids().len(), 1);
        let id = &repository.ids()[0];
        assert_eq!(id.codepoints(), &[97, 117, 116, 104, 111, 114, 0xd800]);
        let profile = repository.get(id).unwrap();
        assert_eq!(profile.scalar_id(), None);
        assert_eq!(
            scalar(profile.name()).as_deref(),
            Some("Source-valid profile")
        );
        // The reference lower-cases `Role("Reviewer")`.
        assert_eq!(scalar(profile.role()).as_deref(), Some("reviewer"));
        assert_eq!(
            profile
                .domain_keywords()
                .iter()
                .filter_map(|k| k.to_scalar_string().ok())
                .collect::<Vec<_>>(),
            vec!["widget".to_owned()]
        );
        // A non-scalar id keeps the authored body losslessly but has no scalar
        // projection.
        assert!(profile.body.scalar_projection().is_none());
        assert!(profile.body.has_non_scalar_strings());
    }
}
